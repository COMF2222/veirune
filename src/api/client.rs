#![allow(dead_code)]

use super::error::{ApiError, Result};
use super::models::Me;
use crate::config::Settings;
use parking_lot::RwLock;
use serde::de::DeserializeOwned;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

pub const API_ROOT: &str = "https://api.soundcloud.com";

#[derive(serde::Deserialize)]
struct CachedStream {
    playlist_path: String,
    bitrate_kbps: u32,
}

impl CachedStream {
    fn streams(self, server: &str) -> Option<super::endpoints::StreamUrls> {
        let ticket = self.playlist_path.strip_prefix("/v1/media/")?.strip_suffix("/index.m3u8")?;
        if ticket.len() != 43 || !ticket.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')) {
            return None;
        }
        let url = format!("{server}{}", self.playlist_path);
        let (aac, mp3) = match self.bitrate_kbps {
            160 => (Some(url), None),
            128 => (None, Some(url)),
            _ => return None,
        };
        Some(super::endpoints::StreamUrls { hls_aac_160: aac, hls_mp3_128: mp3, preview_mp3_128: None })
    }
}

/// In-flight requests and rate-limit state, shown in the top bar.
///
/// The counter spans a whole logical attempt, retries and back-off waits
/// included, so a request the API is making us wait for still reads as busy.
#[derive(Debug, Default)]
pub struct NetActivity {
    in_flight: AtomicU32,
    /// When the oldest current request started, as ms since `epoch`.
    started_ms: AtomicU64,
    /// When the 429 cool-down ends, as ms since `epoch`; 0 when clear.
    cooldown_until_ms: AtomicU64,
    epoch: RwLock<Option<Instant>>,
}

impl NetActivity {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            epoch: RwLock::new(Some(Instant::now())),
            ..Default::default()
        })
    }

    fn now_ms(&self) -> u64 {
        let epoch = self.epoch.read().unwrap_or_else(Instant::now);
        epoch.elapsed().as_millis() as u64
    }

    /// Mark a logical request as started.
    pub fn begin(&self) {
        if self.in_flight.fetch_add(1, Ordering::SeqCst) == 0 {
            self.started_ms.store(self.now_ms(), Ordering::SeqCst);
        }
    }

    pub fn end(&self) {
        let _ = self
            .in_flight
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                Some(n.saturating_sub(1))
            });
    }

    /// Something has been in flight for longer than `threshold`.
    pub fn busy(&self, threshold: Duration) -> bool {
        if self.in_flight.load(Ordering::SeqCst) == 0 {
            return false;
        }
        let started = self.started_ms.load(Ordering::SeqCst);
        self.now_ms().saturating_sub(started) >= threshold.as_millis() as u64
    }

    pub fn in_flight(&self) -> u32 {
        self.in_flight.load(Ordering::SeqCst)
    }

    /// Note a 429: requests hold off until `wait` has passed.
    pub fn rate_limited(&self, wait: Duration) {
        let until = self.now_ms() + wait.as_millis() as u64;
        self.cooldown_until_ms.fetch_max(until, Ordering::SeqCst);
    }

    /// Remaining cool-down, if the API asked us to wait.
    pub fn cooldown_left(&self) -> Option<Duration> {
        let until = self.cooldown_until_ms.load(Ordering::SeqCst);
        if until == 0 {
            return None;
        }
        let now = self.now_ms();
        (until > now).then(|| Duration::from_millis(until - now))
    }
}

/// Rate-limit-aware HTTP client for api.soundcloud.com.
pub struct ApiClient {
    http: reqwest::Client,
    client_id: RwLock<Option<String>>,
    oauth: RwLock<Option<String>>,
    session: RwLock<std::sync::Weak<crate::auth::Session>>,
    /// Whether to answer from the offline demo library instead of the network.
    ///
    /// Atomic rather than a plain `bool` because the client is shared behind an
    /// `Arc` (the player holds one) and registering an application mid-session
    /// has to switch the app to the live API without rebuilding everything
    /// that points at it.
    demo: std::sync::atomic::AtomicBool,
    activity: Arc<NetActivity>,
    media_unavailable_until: RwLock<Option<Instant>>,
}

impl ApiClient {
    pub fn new(client_id: Option<String>, demo: bool) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(concat!("fastcloud/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_else(|error| {
                log::warn!("could not configure HTTP client, using defaults: {error}");
                reqwest::Client::new()
            });
        Self {
            http,
            client_id: RwLock::new(client_id),
            oauth: RwLock::new(None),
            session: RwLock::new(std::sync::Weak::new()),
            demo: std::sync::atomic::AtomicBool::new(demo),
            activity: NetActivity::new(),
            media_unavailable_until: RwLock::new(None),
        }
    }

    pub fn demo() -> Self {
        Self::new(None, true)
    }

    /// Shared network state for the top bar's spinner.
    pub fn activity(&self) -> Arc<NetActivity> {
        self.activity.clone()
    }

    pub fn set_oauth(&self, token: String) {
        *self.oauth.write() = Some(token);
    }

    pub fn oauth_token(&self) -> Option<String> {
        self.oauth.read().clone()
    }

    /// Route SoundCloud resources through the configured broker. A broker
    /// failure stays visible rather than silently using a blocked local route.
    pub async fn relay_credentials(&self) -> Result<Option<(String, String)>> {
        let session = self.session.read().upgrade();
        if let Some(session) = session && session.media_server_url().is_some() {
            let token = session.access_token().await.map_err(ApiError::Other)?;
            let server=session.media_server_url().ok_or(ApiError::Unauthorized)?;
            return Ok(Some((server, token)));
        }
        Ok(None)
    }

    /// Shared audio when a broker is configured, including the owner's account.
    /// Older brokers retain direct playback; an owner denial never falls back.
    pub async fn playback_streams(&self, urn: &str) -> Result<super::endpoints::StreamUrls> {
        self.resolve_playback_streams(urn, Duration::from_secs(8), false).await
    }

    /// A preview may need a first-time search for a verified full recording.
    /// Keep ordinary songs fast; the server stores shared matches for later plays.
    pub async fn playback_streams_for_track(&self, track: &super::models::Track) -> Result<super::endpoints::StreamUrls> {
        let timeout = Duration::from_secs(if track.is_snippet() { 40 } else { 8 });
        self.resolve_playback_streams(&track.urn(), timeout, track.is_snippet()).await
    }

    pub async fn web_playback_source(&self, track: &super::models::Track) -> Result<Option<super::web_playback::WebPlaybackSource>> {
        if self.is_demo() { return Ok(None); }
        let session = self.session.read().upgrade();
        let Some(session) = session else { return Ok(None) };
        let Some(server) = session.media_server_url() else { return Ok(None) };
        let token = session.access_token().await.map_err(ApiError::Other)?;
        let server=session.media_server_url().unwrap_or(server);
        let response = self.http.post(format!("{server}/v1/media/web-resolve"))
            .header(reqwest::header::AUTHORIZATION, format!("OAuth {token}"))
            .timeout(Duration::from_secs(40)).json(&serde_json::json!({"urn":track.urn(),"drm":if cfg!(target_os="macos") {"fairplay"} else {"widevine"}})).send().await?;
        let status = response.status().as_u16();
        if matches!(status, 404 | 422) { return Ok(None); }
        if !(200..300).contains(&status) {
            return Err(ApiError::from_http(status,response.text().await.unwrap_or_default(),None));
        }
        let source: super::web_playback::WebPlaybackSource = response.json().await?;
        if !source.valid() { return Err(ApiError::Bad("Invalid web playback capability".into())); }
        Ok(Some(source))
    }

    /// Renderer transport is restricted to broker media capabilities. CDM
    /// challenges and responses remain opaque and are never logged or saved.
    pub async fn web_playback_resource(&self, path: &str, post: bool, body: Vec<u8>) -> Result<(u16, String, Vec<u8>)> {
        if !super::web_playback::resource_path(path) || body.len() > 1024*1024
            || post && !path.contains("/license/") && !path.ends_with("/started")
        { return Err(ApiError::Bad("Unsupported playback resource".into())); }
        let session = self.session.read().upgrade().ok_or(ApiError::Unauthorized)?;
        let server = session.media_server_url().ok_or_else(|| ApiError::Bad("Playback server is unavailable".into()))?;
        let request = self.http.request(if post {reqwest::Method::POST} else {reqwest::Method::GET},format!("{server}{path}"))
            .timeout(Duration::from_secs(25)).header(reqwest::header::CONTENT_TYPE,"application/octet-stream");
        let response = if post {request.body(body)} else {request}.send().await?;
        let status = response.status().as_u16();
        let mime = response.headers().get(reqwest::header::CONTENT_TYPE).and_then(|h| h.to_str().ok()).unwrap_or("application/octet-stream").to_owned();
        if response.content_length().is_some_and(|length| length > 8*1024*1024) { return Err(ApiError::Bad("Playback resource is too large".into())); }
        let mut response = response;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if bytes.len().saturating_add(chunk.len()) > 8*1024*1024 { return Err(ApiError::Bad("Playback resource is too large".into())); }
            bytes.extend_from_slice(&chunk);
        }
        Ok((status,mime,bytes))
    }

    async fn resolve_playback_streams(&self, urn: &str, timeout: Duration, require_full: bool) -> Result<super::endpoints::StreamUrls> {
        let session = self.session.read().upgrade();
        let available = self.media_unavailable_until.read().is_none_or(|until| until <= Instant::now());
        if (available || require_full) && !self.is_demo()
            && let Some(session) = session
            && let Some(server) = session.media_server_url()
        {
            let token = session.access_token().await.map_err(ApiError::Other)?;
            let server=session.media_server_url().unwrap_or(server);
            let response = self.http.post(format!("{server}/v1/media/resolve"))
                .header(reqwest::header::AUTHORIZATION, format!("OAuth {token}"))
                .timeout(timeout)
                .json(&serde_json::json!({ "urn": urn }))
                .send().await;
            match response {
                Ok(response) if response.status().is_success() => {
                    if let Ok(body) = response.json::<CachedStream>().await
                        && let Some(streams) = body.streams(&server)
                    {
                        return Ok(streams);
                    }
                    *self.media_unavailable_until.write() = Some(Instant::now() + Duration::from_secs(30));
                }
                Ok(response) if response.status() == reqwest::StatusCode::FORBIDDEN => {
                    let delay = response.headers().get("Retry-After").and_then(|v| v.to_str().ok())
                        .and_then(|v| v.parse::<u64>().ok()).map(|seconds| seconds.saturating_mul(1000));
                    let body = response.text().await.unwrap_or_default();
                    if body.to_ascii_lowercase().contains("disabled by the owner") {
                        return Err(ApiError::Other(anyhow::anyhow!("Access to Veirune was disabled by the owner.")));
                    }
                    return Err(ApiError::from_http(403, body, delay));
                }
                Ok(response) if response.status() == reqwest::StatusCode::UNAUTHORIZED => {
                    return Err(ApiError::Unauthorized);
                }
                Ok(response) if response.status() == reqwest::StatusCode::UNPROCESSABLE_ENTITY => {}
                Ok(response) if !response.status().is_success() => {
                    let status = response.status().as_u16();
                    let delay = response.headers().get("Retry-After").and_then(|v| v.to_str().ok())
                        .and_then(|v| v.parse::<u64>().ok()).map(|seconds| seconds.saturating_mul(1000));
                    let error = ApiError::from_http(status, response.text().await.unwrap_or_default(), delay);
                    if matches!(error, ApiError::RateLimited { .. }) {
                        return Err(error);
                    }
                    *self.media_unavailable_until.write() = Some(Instant::now() + Duration::from_secs(30));
                }
                _ => {
                    // Avoid an eight-second pause on every song while a broker
                    // is offline or has not received the media update yet.
                    *self.media_unavailable_until.write() = Some(Instant::now() + Duration::from_secs(30));
                }
            }
        }
        let enc = crate::auth::form_encode(urn);
        let mut streams: super::endpoints::StreamUrls = self.get(&format!("/tracks/{enc}/streams"), &[]).await?;
        if let Some((server, _)) = self.relay_credentials().await? {
            for value in [&mut streams.hls_aac_160, &mut streams.hls_mp3_128, &mut streams.preview_mp3_128] {
                if let Some(path) = value {
                    *path = relay_asset_url(&server, path)?;
                }
            }
        }
        Ok(streams)
    }

    pub fn clear_oauth(&self) {
        *self.oauth.write() = None;
    }

    pub fn attach_session(&self, session: &Arc<crate::auth::Session>) {
        *self.session.write() = Arc::downgrade(session);
    }

    pub fn set_client_id(&self, id: String) {
        *self.client_id.write() = Some(id);
    }

    pub fn is_demo(&self) -> bool {
        self.demo.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Leave (or enter) demo mode. Registering an application is the way out of
    /// it without a restart.
    pub fn set_demo(&self, demo: bool) {
        self.demo.store(demo, std::sync::atomic::Ordering::Relaxed);
    }

    /// Perform a GET request with automatic 429 retry honoring Retry-After.
    pub async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T> {
        let url = format!("{}{}", API_ROOT, path);
        self.get_url(&url, query).await
    }

    pub async fn get_url<T: DeserializeOwned>(
        &self,
        url: &str,
        query: &[(&str, String)],
    ) -> Result<T> {
        self.request_json(reqwest::Method::GET, url, query, None)
            .await
    }

    pub async fn post<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
        body: Option<serde_json::Value>,
    ) -> Result<T> {
        let url = format!("{}{}", API_ROOT, path);
        self.request_json(reqwest::Method::POST, &url, query, body)
            .await
    }

    pub async fn put<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
        body: Option<serde_json::Value>,
    ) -> Result<T> {
        let url = format!("{}{}", API_ROOT, path);
        self.request_json(reqwest::Method::PUT, &url, query, body)
            .await
    }

    pub async fn delete(&self, path: &str, query: &[(&str, String)]) -> Result<()> {
        let url = format!("{}{}", API_ROOT, path);
        self.activity.begin();
        let out = self.delete_inner(&url, query).await;
        self.activity.end();
        out
    }

    async fn delete_inner(&self, url: &str, query: &[(&str, String)]) -> Result<()> {
        self.await_cooldown().await;
        let resp = self.send(reqwest::Method::DELETE, url, query, None).await?;
        let status = resp.status();
        if status.is_success() || status == reqwest::StatusCode::NOT_FOUND {
            Ok(())
        } else {
            Err(ApiError::Http {
                status: status.as_u16(),
                body: resp.text().await.unwrap_or_default(),
            })
        }
    }

    pub async fn get_me(&self) -> Result<Me> {
        self.get("/me", &[]).await
    }

    /// Perform a raw GET and return bytes (used for cover art).
    pub async fn raw_get(&self, url: &str) -> Result<bytes::Bytes> {
        let request = if let Some((server, token)) = self.relay_credentials().await? {
            self.http.post(format!("{server}/v1/soundcloud/artwork"))
                .header(reqwest::header::AUTHORIZATION, format!("OAuth {token}"))
                .json(&serde_json::json!({"url": url}))
        } else { self.http.get(url) };
        let resp = request.send().await.map_err(ApiError::from)?;
        let status = resp.status();
        if !status.is_success() {
            return Err(ApiError::Http {
                status: status.as_u16(),
                body: String::new(),
            });
        }
        Ok(resp.bytes().await?)
    }

    /// Upload one file without buffering it in RAM. The multipart body is
    /// rebuilt after a 429 because reqwest bodies are intentionally one-shot.
    pub async fn post_multipart_file<T: DeserializeOwned>(
        &self,
        path: &str,
        fields: &[(String, String)],
        file_field: &str,
        file_path: &Path,
    ) -> Result<T> {
        let url = format!("{}{}", API_ROOT, path);
        self.activity.begin();
        let out = self
            .post_multipart_file_inner(&url, fields, file_field, file_path)
            .await;
        self.activity.end();
        out
    }

    async fn post_multipart_file_inner<T: DeserializeOwned>(
        &self,
        url: &str,
        fields: &[(String, String)],
        file_field: &str,
        file_path: &Path,
    ) -> Result<T> {
        let mut attempt = 0u32;
        loop {
            self.await_cooldown().await;
            let session = self.session.read().upgrade();
            if let Some(session) = session {
                session.access_token().await.map_err(ApiError::Other)?;
            }
            let mut form = reqwest::multipart::Form::new();
            for (name, value) in fields {
                form = form.text(name.clone(), value.clone());
            }
            form = form
                .file(file_field.to_owned(), file_path)
                .await
                .map_err(anyhow::Error::from)?;
            let routed = if let Some((server, _)) = self.relay_credentials().await? {
                relay_api_url(&server, url)?
            } else { url.to_owned() };
            let mut request = self
                .http
                .post(routed)
                .timeout(Duration::from_secs(3600))
                .header(reqwest::header::ACCEPT, "application/json; charset=utf-8")
                .multipart(form);
            if let Some(token) = self.oauth.read().as_ref() {
                request = request.header(reqwest::header::AUTHORIZATION, format!("OAuth {token}"));
            }
            let response = request.send().await.map_err(ApiError::from)?;
            if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS && attempt < 3 {
                let wait_ms = response
                    .headers()
                    .get("Retry-After")
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.parse::<u64>().ok())
                    .map(|seconds| seconds * 1000)
                    .unwrap_or(2_000)
                    .min(30_000);
                self.activity.rate_limited(Duration::from_millis(wait_ms));
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(wait_ms)).await;
                continue;
            }
            let status = response.status();
            if status == reqwest::StatusCode::UNAUTHORIZED {
                return Err(ApiError::Unauthorized);
            }
            let bytes = response.bytes().await?;
            if !status.is_success() {
                return Err(ApiError::Http {
                    status: status.as_u16(),
                    body: String::from_utf8_lossy(&bytes).into_owned(),
                });
            }
            return decode_response(&bytes);
        }
    }

    /// Resolve a redirecting API resource (currently track previews) to the
    /// final CDN URL while keeping OAuth on the API request itself.
    pub async fn redirected_url(&self, path: &str, query: &[(String, String)]) -> Result<String> {
        let url = format!("{}{}", API_ROOT, path);
        self.activity.begin();
        self.await_cooldown().await;
        let pairs: Vec<_> = query
            .iter()
            .map(|(key, value)| (key.as_str(), value.clone()))
            .collect();
        let response = self.send(reqwest::Method::GET, &url, &pairs, None).await;
        self.activity.end();
        let response = response?;
        if !response.status().is_success() {
            return Err(ApiError::Http {
                status: response.status().as_u16(),
                body: response.text().await.unwrap_or_default(),
            });
        }
        Ok(response.url().to_string())
    }

    async fn request_json<T: DeserializeOwned>(
        &self,
        method: reqwest::Method,
        url: &str,
        query: &[(&str, String)],
        body: Option<serde_json::Value>,
    ) -> Result<T> {
        // One `begin`/`end` pair for the whole attempt, retries included, so
        // the interface can say "waiting for SoundCloud" while we back off.
        self.activity.begin();
        let out = self.request_json_inner(method, url, query, body).await;
        self.activity.end();
        out
    }

    async fn request_json_inner<T: DeserializeOwned>(
        &self,
        method: reqwest::Method,
        url: &str,
        query: &[(&str, String)],
        body: Option<serde_json::Value>,
    ) -> Result<T> {
        let mut attempt = 0u32;
        loop {
            self.await_cooldown().await;
            let resp = match self.send(method.clone(), url, query, body.clone()).await {
                Ok(response) => response,
                Err(error) if retry_read(&method, attempt, &error) => {
                    attempt += 1;
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    continue;
                }
                Err(error) => return Err(error),
            };
            let status = resp.status();
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                let retry_after = resp
                    .headers()
                    .get("Retry-After")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok());
                let wait_ms = retry_after.map(|s| s.saturating_mul(1000)).unwrap_or(30_000).clamp(1000, 86_400_000);
                self.activity.rate_limited(Duration::from_millis(wait_ms));
                // Playback owns delayed recovery. Holding this request open
                // for repeated 429s hides the limit from the queue controller.
                return Err(ApiError::RateLimited {retry_after_ms:Some(wait_ms)});
            }
            if status == reqwest::StatusCode::UNAUTHORIZED {
                return Err(ApiError::Unauthorized);
            }
            if method == reqwest::Method::GET
                && status.is_server_error()
                && status != reqwest::StatusCode::GATEWAY_TIMEOUT
                && attempt < 1
            {
                // One quick retry can recover a transient server error. A
                // gateway timeout has already kept the UI waiting, so return
                // it and let the page's retry timer try again later.
                let wait_ms = 500u64 * (1 << attempt);
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(wait_ms)).await;
                continue;
            }
            if !status.is_success() {
                let delay = resp.headers().get("Retry-After").and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok()).map(|seconds| seconds.saturating_mul(1000));
                return Err(ApiError::from_http(status.as_u16(), resp.text().await.unwrap_or_default(), delay));
            }
            let result = match resp.bytes().await {
                Ok(bytes) => decode_response(&bytes),
                Err(error) => Err(ApiError::Network(error)),
            };
            if let Err(error) = &result {
                if retry_read(&method, attempt, error) {
                    attempt += 1;
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    continue;
                }
            }
            return result;
        }
    }

    /// Hold off while a 429 cool-down is running.
    async fn await_cooldown(&self) {
        if let Some(left) = self.activity.cooldown_left() {
            tokio::time::sleep(left).await;
        }
    }

    async fn send(
        &self,
        method: reqwest::Method,
        url: &str,
        query: &[(&str, String)],
        body: Option<serde_json::Value>,
    ) -> Result<reqwest::Response> {
        // The session serializes refreshes; the weak link avoids a reference
        // cycle while ensuring playback and every catalogue request stay fresh.
        let session = self.session.read().upgrade();
        if let Some(session) = session {
            session.access_token().await.map_err(ApiError::Other)?;
        }
        let routed = if let Some((server, _)) = self.relay_credentials().await? {
            relay_api_url(&server, url)?
        } else { url.to_owned() };
        let mut req = self.http.request(method, routed);
        req = req.header(reqwest::header::ACCEPT, "application/json; charset=utf-8");
        if !query.is_empty() {
            req = req.query(query);
        }
        // SoundCloud wants `Authorization: OAuth <token>`, not Bearer
        // (API Guide → Authentication).
        if let Some(token) = self.oauth.read().as_ref() {
            req = req.header(reqwest::header::AUTHORIZATION, format!("OAuth {token}"));
        }
        if let Some(body) = body {
            req = req.json(&body);
        }
        req.send().await.map_err(ApiError::from)
    }
}

fn relay_api_url(server: &str, raw: &str) -> Result<String> {
    let url = url::Url::parse(raw).map_err(anyhow::Error::from)?;
    if url.scheme() != "https" || url.host_str() != Some("api.soundcloud.com")
        || url.port().is_some_and(|port| port != 443)
        || !url.username().is_empty() || url.password().is_some() || url.fragment().is_some()
    {
        return Err(ApiError::Other(anyhow::anyhow!("Only official SoundCloud API URLs can use the server relay.")));
    }
    let query = url.query().map(|query| format!("?{query}")).unwrap_or_default();
    Ok(format!("{server}/v1/soundcloud/api{}{query}", url.path()))
}

fn relay_asset_url(server: &str, path: &str) -> Result<String> {
    let ticket = path.strip_prefix("/v1/soundcloud/asset/");
    if ticket.is_none_or(|ticket| ticket.len() != 43 || !ticket.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))) {
        return Err(ApiError::Other(anyhow::anyhow!("Server returned an invalid SoundCloud audio link.")));
    }
    Ok(format!("{server}{path}"))
}

fn retry_read(method: &reqwest::Method, attempt: u32, error: &ApiError) -> bool {
    method == reqwest::Method::GET
        && attempt == 0
        && match error {
            ApiError::Network(error) => {
                error.is_connect() || error.is_timeout() || error.is_request() || error.is_body()
            }
            ApiError::Json(error) => error.is_eof(),
            _ => false,
        }
}

fn decode_response<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    // Reposts and follows can succeed with 201/204 and no JSON body.
    // Only types accepting null (Value, (), Option) accept this success;
    // a missing track or collection still fails loudly.
    let bytes = if bytes.iter().all(u8::is_ascii_whitespace) {
        b"null"
    } else {
        bytes
    };
    Ok(serde_json::from_slice(bytes)?)
}

pub type SharedClient = Arc<ApiClient>;

pub fn from_settings(settings: &Settings, demo: bool) -> SharedClient {
    Arc::new(ApiClient::new(settings.client_id.clone(), demo))
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn media_resolution_distinguishes_track_denial_quota_and_owner_denial() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for (status, body, kind) in [
            (403, r#"{"error":"Track unavailable"}"#, "track"),
            (403, r#"{"error":"Access to Fastcloud was disabled by the owner"}"#, "owner"),
            (403, r#"{"error":"Rate limit exceeded"}"#, "quota"),
            (502, r#"{"error":"SoundCloud returned HTTP 429: rate limit"}"#, "quota"),
            (429, r#"{"error":"Slow down"}"#, "quota"),
            (422, "{}", "relay-quota"),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let client = Arc::new(ApiClient::new(None, false));
            let _session = crate::auth::Session::with_test_user(crate::auth::AppCredentials {
                client_id:"test".into(), client_secret:String::new(), redirect_uri:String::new(),
                server_url:Some(format!("http://{}", listener.local_addr().unwrap())),
            }, client.clone()).await;
            let server = tokio::spawn(async move {
                let responses = if status == 422 {vec![(422,body),(429,r#"{"error":"Slow down"}"#)]} else {vec![(status,body)]};
                for (status,body) in responses {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let mut request = [0;8192]; socket.read(&mut request).await.unwrap();
                    let reply = format!("HTTP/1.1 {status} Fixture\r\nRetry-After: 12\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
                    socket.write_all(reply.as_bytes()).await.unwrap();
                }
            });
            let error = tokio::time::timeout(Duration::from_secs(5),client.playback_streams("soundcloud:tracks:42")).await.unwrap().unwrap_err();
            match kind {
                "track" => assert!(matches!(error, ApiError::Http {status:403,..})),
                "owner" => assert!(matches!(error, ApiError::Other(_))),
                _ => assert!(matches!(error, ApiError::RateLimited {retry_after_ms:Some(12000)})),
            }
            server.await.unwrap();
        }
    }

    use super::*;

    #[test]
    fn relay_preserves_encoded_urns_and_pagination_and_rejects_external_urls() {
        let server = "https://broker.example";
        assert_eq!(relay_api_url(server, "https://api.soundcloud.com/tracks/soundcloud%3Atracks%3A42?cursor=a%2Bb").unwrap(),
            "https://broker.example/v1/soundcloud/api/tracks/soundcloud%3Atracks%3A42?cursor=a%2Bb");
        for raw in ["http://api.soundcloud.com/me", "https://evil.example/me", "https://api.soundcloud.com:444/me", "https://user@api.soundcloud.com/me", "https://api.soundcloud.com/me#fragment"] {
            assert!(relay_api_url(server, raw).is_err());
        }
        for raw in ["https://evil.example/audio", "/v1/soundcloud/asset/../me", "/v1/soundcloud/asset/short"] {
            assert!(relay_asset_url(server, raw).is_err());
        }
    }

    #[tokio::test]
    async fn server_session_routes_profile_paginated_library_and_mutations_without_local_soundcloud() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let broker = format!("http://{}", listener.local_addr().unwrap());
        let client = Arc::new(ApiClient::new(None, false));
        let _session = crate::auth::Session::with_test_user(crate::auth::AppCredentials {
            client_id: "test-client".into(), client_secret: String::new(),
            redirect_uri: "http://127.0.0.1:41317/callback".into(), server_url: Some(broker),
        }, client.clone()).await;
        let task = tokio::spawn(async move {
            for expected in ["GET /v1/soundcloud/api/me ", "GET /v1/soundcloud/api/me/likes/tracks?cursor=a%2Bb ", "PUT /v1/soundcloud/api/playlists/soundcloud%3Aplaylists%3A42 ", "DELETE /v1/soundcloud/api/likes/tracks/soundcloud%3Atracks%3A42 "] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = vec![0; 8192];
                let size = socket.read(&mut bytes).await.unwrap();
                let request = String::from_utf8_lossy(&bytes[..size]);
                assert!(request.starts_with(expected), "{request}");
                assert!(request.to_lowercase().contains("authorization: oauth test-user-token"));
                socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").await.unwrap();
            }
        });
        client.get::<serde_json::Value>("/me", &[]).await.unwrap();
        client.get_url::<serde_json::Value>("https://api.soundcloud.com/me/likes/tracks?cursor=a%2Bb", &[]).await.unwrap();
        client.put::<serde_json::Value>("/playlists/soundcloud%3Aplaylists%3A42", &[], Some(serde_json::json!({"playlist":{"title":"mine"}}))).await.unwrap();
        client.delete("/likes/tracks/soundcloud%3Atracks%3A42", &[]).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), task).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn non_cacheable_audio_and_artwork_use_the_broker_too() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let broker = format!("http://{}", listener.local_addr().unwrap());
        let client = Arc::new(ApiClient::new(None, false));
        let _session = crate::auth::Session::with_test_user(crate::auth::AppCredentials {
            client_id: "test-client".into(), client_secret: String::new(),
            redirect_uri: "http://127.0.0.1:41317/callback".into(), server_url: Some(broker.clone()),
        }, client.clone()).await;
        let ticket = "a".repeat(43);
        let body = format!(r#"{{"hls_aac_160_url":"/v1/soundcloud/asset/{ticket}"}}"#);
        let task = tokio::spawn(async move {
            for (expected, status, body) in [
                ("POST /v1/media/resolve ", "422 Unprocessable Entity", "{}"),
                ("GET /v1/soundcloud/api/tracks/soundcloud%3Atracks%3A42/streams ", "200 OK", body.as_str()),
                ("POST /v1/soundcloud/artwork ", "200 OK", "image"),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = vec![0; 8192];
                let size = socket.read(&mut bytes).await.unwrap();
                let request = String::from_utf8_lossy(&bytes[..size]);
                assert!(request.starts_with(expected), "{request}");
                assert!(request.to_lowercase().contains("authorization: oauth test-user-token"));
                let response = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let streams = client.playback_streams("soundcloud:tracks:42").await.unwrap();
        assert_eq!(streams.hls_aac_160, Some(format!("{broker}/v1/soundcloud/asset/{}", "a".repeat(43))));
        assert_eq!(client.raw_get("https://i1.sndcdn.com/cover.jpg").await.unwrap().as_ref(), b"image");
        tokio::time::timeout(Duration::from_secs(5), task).await.unwrap().unwrap();
    }

    #[test]
    fn shared_stream_stays_on_the_configured_broker_and_preserves_bitrate() {
        let body = CachedStream { playlist_path: format!("/v1/media/{}/index.m3u8", "a".repeat(43)), bitrate_kbps: 160 };
        let streams = body.streams("https://broker.example").unwrap();
        assert_eq!(streams.best_full_with_bitrate(), Some((format!("https://broker.example/v1/media/{}/index.m3u8", "a".repeat(43)).as_str(), 160)));
        for path in ["https://elsewhere.example/track", "/v1/media/../../index.m3u8", "/v1/media/test/index.m3u8?token=x"] {
            assert!(CachedStream { playlist_path: path.into(), bitrate_kbps: 160 }.streams("https://broker.example").is_none());
        }
    }

    #[test]
    fn an_empty_success_is_valid_for_writes_but_not_for_tracks() {
        assert_eq!(
            decode_response::<serde_json::Value>(b"").unwrap(),
            serde_json::Value::Null
        );
        assert!(decode_response::<crate::api::models::Track>(b"").is_err());
        assert!(decode_response::<serde_json::Value>(b"{").is_err());
    }

    #[test]
    fn retry_only_incomplete_reads_and_never_mutations_or_invalid_data() {
        let incomplete =
            ApiError::Json(serde_json::from_str::<serde_json::Value>("{").unwrap_err());
        let malformed =
            ApiError::Json(serde_json::from_str::<serde_json::Value>("{bad}").unwrap_err());
        assert!(retry_read(&reqwest::Method::GET, 0, &incomplete));
        assert!(!retry_read(&reqwest::Method::GET, 1, &incomplete));
        assert!(!retry_read(&reqwest::Method::POST, 0, &incomplete));
        assert!(!retry_read(&reqwest::Method::GET, 0, &malformed));
        assert!(!retry_read(
            &reqwest::Method::GET,
            0,
            &ApiError::Unauthorized
        ));
    }

    #[tokio::test]
    async fn truncated_get_is_retried_and_activity_is_balanced() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for body in ["{", r#"{"ok":true}"#] {
                let (mut connection, _) = listener.accept().await.unwrap();
                let mut request = [0; 4096];
                connection.read(&mut request).await.unwrap();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                connection.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let client = ApiClient::new(None, false);
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            client.get_url::<serde_json::Value>(&format!("http://{address}/streams"), &[]),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(client.activity.in_flight(), 0);
        server.await.unwrap();
    }

    #[test]
    fn activity_counts_overlapping_requests() {
        let net = NetActivity::new();
        assert_eq!(net.in_flight(), 0);
        assert!(!net.busy(Duration::ZERO));
        net.begin();
        net.begin();
        assert_eq!(net.in_flight(), 2);
        // Busy right away at a zero threshold, quiet at a long one.
        assert!(net.busy(Duration::ZERO));
        assert!(!net.busy(Duration::from_secs(5)));
        net.end();
        assert_eq!(net.in_flight(), 1);
        net.end();
        assert_eq!(net.in_flight(), 0);
        assert!(!net.busy(Duration::ZERO));
        // Never underflows.
        net.end();
        assert_eq!(net.in_flight(), 0);
    }

    #[test]
    fn cooldown_reports_time_left_and_keeps_the_longest() {
        let net = NetActivity::new();
        assert!(net.cooldown_left().is_none());
        net.rate_limited(Duration::from_secs(30));
        let left = net.cooldown_left().expect("cooling down");
        assert!(left <= Duration::from_secs(30) && left > Duration::from_secs(25));
        // A shorter wait must not shorten an existing one.
        net.rate_limited(Duration::from_secs(1));
        let left = net.cooldown_left().expect("still cooling down");
        assert!(left > Duration::from_secs(25));
    }

    /// Registering an application has to switch a running app to the live API,
    /// and the player holds this client behind an `Arc` — so demo mode is a
    /// shared flag that can be turned off in place rather than a field fixed at
    /// construction.
    #[test]
    fn demo_mode_can_be_left_without_rebuilding_the_client() {
        let client = ApiClient::demo();
        assert!(client.is_demo());
        client.set_demo(false);
        assert!(!client.is_demo(), "a registration could not take effect");
        // …and a shared reference is enough to do it, which is the point.
        let shared = Arc::new(client);
        let other = shared.clone();
        other.set_demo(true);
        assert!(shared.is_demo());
        other.set_demo(false);
        assert!(!shared.is_demo());
        // A client built for the live API starts out of demo mode.
        assert!(!ApiClient::new(Some("id".into()), false).is_demo());
    }
}
