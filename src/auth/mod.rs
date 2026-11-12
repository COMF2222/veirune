//! SoundCloud OAuth 2.1, exactly as the API Guide specifies it.
//!
//! Two flows, both against `https://secure.soundcloud.com`:
//!
//! * **Authorization Code + PKCE** ([`authorize`]) for anything touching the
//!   account — likes, playlists, the feed, `/me`.
//! * **Client Credentials** ([`client_credentials`]) for public browsing:
//!   search, `/resolve`, and playback of public tracks. No user session, so
//!   the app is useful before anyone signs in.
//!
//! ## Why a client secret is required
//!
//! PKCE normally lets a native app skip the secret. SoundCloud does not:
//!
//! > All clients are currently treated as confidential rather than public,
//! > meaning a secret is required to obtain a token.
//!
//! So the token exchange sends `client_secret` as well as the PKCE verifier,
//! and Veirune can ship **no** credentials of its own: each user needs an
//! application. [`register`] gets them one without their leaving the app — one
//! browser sign-in registers it and hands back the keys, which also proves the
//! account has the Artist Pro subscription SoundCloud requires for API access.
//! The keys go to the OS keyring, never to a file in the repo.
//!
//! ## Token lifetime
//!
//! Access tokens last about an hour and refresh tokens are **single-use**, so
//! a refresh must persist the new pair before anything else can use it;
//! [`Session`] serialises refreshes behind one lock so two callers racing a
//! 401 cannot burn the same refresh token twice.

#![allow(dead_code)]

pub mod register;
pub mod endpoint;

use anyhow::{Context, Result};
use base64::Engine;
use rand::RngCore;
use std::net::TcpListener;
use std::sync::Arc;

/// Loopback port the browser is redirected back to.
///
/// The redirect URI must match the registered one **exactly**, so this is
/// only the default: [`AppCredentials::redirect_uri`] carries whatever the
/// user registered.
pub const AUTH_PORT: u16 = 41317;
pub const DEFAULT_REDIRECT_URI: &str = "http://127.0.0.1:41317/callback";

pub const AUTH_ROOT: &str = "https://secure.soundcloud.com";

pub const AUTH_SERVICE: &str = "com.fastcloud.tokens";
pub const LEGACY_SERVER_ORIGIN: &str = "https://77.223.107.226";
pub const CURRENT_SERVER_ORIGIN: &str = "https://89.125.126.32";
pub fn replacement_server_origin(previous: &str) -> Option<&'static str> {
    (previous == LEGACY_SERVER_ORIGIN).then_some(CURRENT_SERVER_ORIGIN)
}
pub const USER_ENTRY: &str = "oauth";
pub const APP_ENTRY: &str = "oauth_app";
pub const CLIENT_ID_ENTRY: &str = "client_id";
pub const CLIENT_SECRET_ENTRY: &str = "client_secret";
pub const REDIRECT_URI_ENTRY: &str = "redirect_uri";
pub const REMOTE_ENTRY: &str = "approval_server";
pub const REMOTE_CLIENT_ENTRY: &str = "approval_client_id";
pub const REMOTE_REDIRECT_ENTRY: &str = "approval_redirect_uri";
pub const REMOTE_USER_ENTRY: &str = "approval_oauth";
pub const ADMIN_SERVER_ENTRY: &str = "approval_admin_server";

/// Refresh this long before the token actually expires, so a request never
/// starts with a token that dies mid-flight.
const REFRESH_SKEW_SECS: i64 = 120;

/// How long to wait for the browser to come back.
const CALLBACK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

// ===========================================================================
// PKCE
// ===========================================================================

/// A PKCE verifier and its S256 challenge.
///
/// The guide asks for `code_challenge_method=S256`; `plain` is what OAuth 2.0
/// tolerated and 2.1 removed.
#[derive(Debug, Clone)]
pub struct Pkce {
    pub verifier: String,
}

impl Pkce {
    /// 43–128 characters of unreserved ASCII, per RFC 7636. 32 random bytes
    /// base64url-encode to 43, the shortest the spec allows.
    pub fn generate() -> Self {
        let mut bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        Self {
            verifier: b64(&bytes),
        }
    }

    /// `BASE64URL(SHA256(verifier))`, no padding.
    pub fn challenge(&self) -> String {
        use sha2::Digest;
        b64(&sha2::Sha256::digest(self.verifier.as_bytes()))
    }
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

// ===========================================================================
// Credentials
// ===========================================================================

/// The user's own registered application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppCredentials {
    pub client_id: String,
    pub client_secret: String,
    /// Must match the app registration character for character.
    pub redirect_uri: String,
    pub server_url: Option<String>,
}

impl AppCredentials {
    pub fn new(client_id: String, client_secret: String, redirect_uri: Option<String>) -> Self {
        Self {
            client_id,
            client_secret,
            redirect_uri: redirect_uri.unwrap_or_else(|| DEFAULT_REDIRECT_URI.to_owned()),
            server_url: None,
        }
    }

    /// From the environment (CI, scripts, `FASTCLOUD_REDIRECT_URI` optional).
    pub fn from_env() -> Option<Self> {
        Some(Self::new(
            std::env::var("FASTCLOUD_CLIENT_ID").ok()?,
            std::env::var("FASTCLOUD_CLIENT_SECRET").ok()?,
            std::env::var("FASTCLOUD_REDIRECT_URI").ok(),
        ))
    }

    /// From the OS keyring, where Settings writes them.
    pub fn from_keyring() -> Option<Self> {
        Some(Self::new(
            read_secret(CLIENT_ID_ENTRY)?,
            read_secret(CLIENT_SECRET_ENTRY)?,
            read_secret(REDIRECT_URI_ENTRY),
        ))
    }

    /// Environment first (explicit beats stored), then the keyring.
    pub fn discover() -> Option<Self> {
        Self::from_env().or_else(Self::from_remote_keyring).or_else(|| {
            let direct = Self::from_keyring()?;
            let Some(remote) = read_secret(ADMIN_SERVER_ENTRY)
                .and_then(|origin| direct.for_owned_server(&origin)) else { return Some(direct) };
            delete_secret(REMOTE_USER_ENTRY).ok()?;
            remote.save_remote().ok()?;
            Some(remote)
        })
    }

    fn for_owned_server(&self, origin: &str) -> Option<Self> {
        if self.server_url.is_some() || !matches!(origin, LEGACY_SERVER_ORIGIN | CURRENT_SERVER_ORIGIN) {
            return None;
        }
        Some(Self { client_id:self.client_id.clone(), client_secret:String::new(),
            redirect_uri:self.redirect_uri.clone(), server_url:Some(CURRENT_SERVER_ORIGIN.into()) })
    }

    pub fn from_remote_keyring() -> Option<Self> {
        let mut creds = Self {
            client_id: read_secret(REMOTE_CLIENT_ENTRY)?,
            client_secret: String::new(),
            redirect_uri: read_secret(REMOTE_REDIRECT_ENTRY)?,
            server_url: Some(read_secret(REMOTE_ENTRY)?),
        };
        if let Some(server) = creds.server_url.as_deref().and_then(replacement_server_origin) {
            // The former VPS is unavailable. Require a fresh grant on our replacement
            // server without deleting per-account settings or personal caches.
            delete_secret(REMOTE_USER_ENTRY).ok()?;
            creds.server_url = Some(server.into());
            creds.save_remote().ok()?;
        }
        Some(creds)
    }

    pub async fn from_server(server_url: &str) -> Result<Self> {
        let requested = checked_server_url(server_url)?;
        if requested.starts_with("https://") {let _=endpoint::refresh(false).await;}
        let server_url = endpoint::current(&requested);
        let response = reqwest::Client::new()
            .get(format!("{server_url}/v1/config"))
            .send().await.context("contact approval server")?
            .error_for_status().context("read approval server configuration")?;
        let body: serde_json::Value = response.json().await?;
        let client_id = body["client_id"].as_str().filter(|value| !value.is_empty())
            .context("server returned no client ID")?;
        let redirect_uri = body["redirect_uri"].as_str().context("server returned no redirect URI")?;
        let creds = Self {
            client_id: client_id.to_owned(), client_secret: String::new(),
            redirect_uri: redirect_uri.to_owned(), server_url: Some(if endpoint::managed(&requested) {CURRENT_SERVER_ORIGIN.to_owned()} else {server_url}),
        };
        let redirect = url::Url::parse(&creds.redirect_uri).context("invalid server OAuth redirect")?;
        anyhow::ensure!(redirect.scheme() == "http"
            && matches!(redirect.host_str(), Some("127.0.0.1" | "localhost" | "::1"))
            && redirect.path() == "/callback" && redirect.port().is_some()
            && redirect.query().is_none() && redirect.fragment().is_none(),
            "server OAuth redirect must be a loopback /callback URI");
        Ok(creds)
    }

    pub fn save_remote(&self) -> Result<()> {
        let server_url = self.server_url.as_deref().context("missing server URL")?;
        write_secret(REMOTE_CLIENT_ENTRY, &self.client_id)?;
        write_secret(REMOTE_REDIRECT_ENTRY, &self.redirect_uri)?;
        write_secret(REMOTE_ENTRY, server_url)?;
        write_secret(ADMIN_SERVER_ENTRY, server_url)?;
        Ok(())
    }

    pub fn save_to_keyring(&self) -> Result<()> {
        write_secret(CLIENT_ID_ENTRY, &self.client_id)?;
        write_secret(CLIENT_SECRET_ENTRY, &self.client_secret)?;
        write_secret(REDIRECT_URI_ENTRY, &self.redirect_uri)?;
        Ok(())
    }

    pub fn forget() -> Result<()> {
        for entry in [CLIENT_ID_ENTRY, CLIENT_SECRET_ENTRY, REDIRECT_URI_ENTRY] {
            delete_secret(entry)?;
        }
        Ok(())
    }

    /// The browser URL that starts the authorization-code flow.
    pub fn authorize_url(&self, pkce: &Pkce, state: &str) -> String {
        let mut url = format!("{AUTH_ROOT}/authorize");
        let query = [
            ("client_id", self.client_id.as_str()),
            ("redirect_uri", self.redirect_uri.as_str()),
            ("response_type", "code"),
            ("code_challenge", &pkce.challenge()),
            ("code_challenge_method", "S256"),
            ("state", state),
        ];
        for (i, (key, value)) in query.iter().enumerate() {
            url.push(if i == 0 { '?' } else { '&' });
            url.push_str(key);
            url.push('=');
            url.push_str(&form_encode(value));
        }
        url
    }

    /// `Authorization: Basic base64(id:secret)`, the only form the
    /// client-credentials grant accepts.
    fn basic_auth(&self) -> String {
        let raw = format!("{}:{}", self.client_id, self.client_secret);
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(raw)
        )
    }

    /// The loopback port to listen on, read back out of the redirect URI so a
    /// user who registered a different port still works.
    pub fn callback_port(&self) -> u16 {
        self.redirect_uri
            .split("//")
            .nth(1)
            .and_then(|rest| rest.split('/').next())
            .and_then(|host| host.rsplit(':').next())
            .and_then(|port| port.parse().ok())
            .unwrap_or(AUTH_PORT)
    }

    /// Whether the redirect URI is a loopback address this app can serve.
    /// A custom scheme (`fastcloud://…`) would need OS registration instead.
    pub fn redirect_is_loopback(&self) -> bool {
        self.redirect_uri.starts_with("http://127.0.0.1")
            || self.redirect_uri.starts_with("http://localhost")
            || self.redirect_uri.starts_with("http://[::1]")
    }
}

pub fn checked_server_url(value: &str) -> Result<String> {
    let url = url::Url::parse(value.trim()).context("invalid server URL")?;
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
    anyhow::ensure!(url.scheme() == "https" || (url.scheme() == "http" && local),
        "approval server requires HTTPS");
    anyhow::ensure!(url.username().is_empty() && url.password().is_none()
        && url.query().is_none() && url.fragment().is_none() && url.path() == "/",
        "use the server origin without path, credentials or query");
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

pub fn saved_server_url() -> Option<String> {
    read_secret(ADMIN_SERVER_ENTRY).or_else(|| read_secret(REMOTE_ENTRY))
        .map(|origin| endpoint::current(replacement_server_origin(&origin).unwrap_or(&origin)))
}

pub fn save_server_url(value: &str) -> Result<String> {
    let url = checked_server_url(value)?;
    write_secret(ADMIN_SERVER_ENTRY, &url)?;
    Ok(endpoint::current(&url))
}

fn read_secret(key: &str) -> Option<String> {
    keyring::Entry::new(AUTH_SERVICE, key)
        .ok()?
        .get_password()
        .ok()
        .filter(|value| !value.is_empty())
}

fn write_secret(key: &str, value: &str) -> Result<()> {
    keyring::Entry::new(AUTH_SERVICE, key)
        .with_context(|| format!("open keyring entry {key}"))?
        .set_password(value)
        .with_context(|| format!("store {key}"))
}

fn delete_secret(key: &str) -> Result<()> {
    let entry = keyring::Entry::new(AUTH_SERVICE, key).context("open keyring entry")?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(anyhow::anyhow!("keyring: {e}")),
    }
}

// ===========================================================================
// Tokens
// ===========================================================================

/// Which flow minted a token. A client-credentials token cannot read `/me`,
/// so the UI needs to know which one it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Grant {
    /// Signed in: the account's own data is reachable.
    User,
    /// App-only: public resources.
    App,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Tokens {
    pub access_token: String,
    /// Empty when the response carried none (client credentials may not).
    #[serde(default)]
    pub refresh_token: String,
    #[serde(default)]
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default = "user_grant")]
    pub grant: Grant,
}

fn user_grant() -> Grant {
    Grant::User
}

impl Tokens {
    pub(crate) fn from_response(body: &serde_json::Value, grant: Grant) -> Result<Self> {
        let access_token = body
            .get("access_token")
            .and_then(|v| v.as_str())
            .filter(|token| !token.is_empty())
            .context("token response carried no access_token")?
            .to_owned();
        let expires_in = body
            .get("expires_in")
            .and_then(|v| v.as_i64())
            .unwrap_or(3600);
        Ok(Self {
            access_token,
            refresh_token: body
                .get("refresh_token")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_owned(),
            expires_at: Some(chrono::Utc::now().timestamp() + expires_in),
            scope: body
                .get("scope")
                .and_then(|v| v.as_str())
                .map(str::to_owned),
            grant,
        })
    }

    /// Expired, or close enough that a request would race the expiry.
    pub fn expired(&self) -> bool {
        self.expires_at
            .map(|at| chrono::Utc::now().timestamp() >= at - REFRESH_SKEW_SECS)
            .unwrap_or(true)
    }

    pub fn can_refresh(&self) -> bool {
        !self.refresh_token.is_empty()
    }

    // ===== Keyring persistence =====

    pub fn save(&self) -> Result<()> {
        let entry = if self.grant == Grant::User { USER_ENTRY } else { APP_ENTRY };
        write_secret(entry, &serde_json::to_string(self)?)
    }

    fn save_for(&self, remote: bool) -> Result<()> {
        if remote { write_secret(REMOTE_USER_ENTRY, &serde_json::to_string(self)?) }
        else { self.save() }
    }

    fn load_for(remote: bool) -> Result<Option<Self>> {
        if remote { Self::load_entry(REMOTE_USER_ENTRY) } else { Self::load() }
    }

    pub fn load() -> Result<Option<Self>> {
        let legacy = Self::load_entry(USER_ENTRY)?;
        if legacy.as_ref().is_some_and(|tokens| {
            tokens.grant == Grant::User && (!tokens.expired() || tokens.can_refresh())
        }) {
            return Ok(legacy);
        }
        Ok(Self::load_entry(APP_ENTRY)?.or(legacy))
    }

    fn load_entry(key: &str) -> Result<Option<Self>> {
        let entry = keyring::Entry::new(AUTH_SERVICE, key).context("keyring entry")?;
        match entry.get_password() {
            Ok(json) => Ok(Some(serde_json::from_str(&json).context("invalid stored OAuth token metadata")?)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(anyhow::anyhow!("keyring: {e}")),
        }
    }

    pub fn delete() -> Result<()> {
        delete_secret(USER_ENTRY)
    }

    pub fn delete_all() -> Result<()> {
        Self::delete()?;
        delete_secret(APP_ENTRY)
    }
}

// ===========================================================================
// Session
// ===========================================================================

/// Holds the live tokens and keeps them fresh.
///
/// Refresh tokens are single-use, so every refresh goes through one async
/// mutex: two views hitting a stale token at once would otherwise both post
/// the same refresh token and the second would be rejected, signing the user
/// out for no reason.
pub struct Session {
    creds: AppCredentials,
    tokens: Arc<tokio::sync::Mutex<Option<Tokens>>>,
    client: Arc<crate::api::ApiClient>,
    lifecycle: Arc<tokio::sync::watch::Sender<u64>>,
    signing_out: Arc<std::sync::atomic::AtomicBool>,
}

impl Session {
    #[cfg(test)]
    pub(crate) async fn with_test_user(creds: AppCredentials, client: Arc<crate::api::ApiClient>) -> Arc<Self> {
        let session = Self::new(creds, client);
        *session.tokens.lock().await = Some(Tokens {
            access_token: "test-user-token".into(), refresh_token: String::new(),
            expires_at: Some(chrono::Utc::now().timestamp() + 3600), scope: None, grant: Grant::User,
        });
        session
    }
    pub fn media_server_url(&self) -> Option<String> {
        self.creds.server_url.as_deref().map(endpoint::current).or_else(|| {
            saved_server_url().and_then(|value| checked_server_url(&value).ok())
        })
    }

    pub fn is_remote(&self) -> bool {
        self.creds.server_url.is_some()
    }

    pub fn new(creds: AppCredentials, client: Arc<crate::api::ApiClient>) -> Arc<Self> {
        let session = Arc::new(Self {
            creds,
            tokens: Arc::new(tokio::sync::Mutex::new(None)),
            client: client.clone(),
            lifecycle: Arc::new(tokio::sync::watch::channel(0).0),
            signing_out: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });
        client.attach_session(&session);
        session
    }

    /// Bind a background operation to its own API client while sharing the
    /// single refresh lock. It cannot replace the foreground client's OAuth.
    pub fn for_client(&self, client: Arc<crate::api::ApiClient>) -> Arc<Self> {
        let session = Arc::new(Self {
            creds: self.creds.clone(), tokens: self.tokens.clone(), client: client.clone(),
            lifecycle:self.lifecycle.clone(), signing_out:self.signing_out.clone(),
        });
        client.attach_session(&session);
        session
    }

    /// Adopt a saved grant without making startup wait for SoundCloud. Token
    /// refresh and validation happen on the first resource request instead.
    pub async fn resume(&self) -> Result<Option<Grant>> {
        let Some(saved) = Tokens::load_for(self.creds.server_url.is_some())? else { return Ok(None) };
        if saved.expired() && !saved.can_refresh() {
            // Retain the user grant in the keyring until an explicit sign-in
            // replaces it. A public fallback must never erase the account.
            return Ok(None);
        }
        let grant = saved.grant;
        if !saved.expired() {
            self.client.set_oauth(saved.access_token.clone());
        }
        *self.tokens.lock().await = Some(saved);
        Ok(Some(grant))
    }

    /// A valid access token, refreshing or re-minting as needed. Also pushes
    /// it into the API client, so callers never have to.
    pub async fn access_token(&self) -> Result<String> {
        anyhow::ensure!(!self.signing_out.load(std::sync::atomic::Ordering::SeqCst), "SoundCloud session is signing out");
        let mut lifecycle = self.lifecycle.subscribe();
        tokio::select! {
            biased;
            _ = lifecycle.changed() => anyhow::bail!("SoundCloud session was signed out"),
            result = self.access_token_active() => result,
        }
    }

    async fn access_token_active(&self) -> Result<String> {
        let mut held = self.tokens.lock().await;
        anyhow::ensure!(!self.signing_out.load(std::sync::atomic::Ordering::SeqCst), "SoundCloud session is signing out");
        if let Some(tokens) = held.as_ref().filter(|t| !t.expired()) {
            self.client.set_oauth(tokens.access_token.clone());
            return Ok(tokens.access_token.clone());
        }
        // Keep the previous pair until a refresh succeeds. A network failure
        // must not discard a user's session or downgrade it to an app token.
        let fresh = match held.as_ref() {
            Some(tokens) if tokens.can_refresh() => {
                match self.refresh(&tokens.refresh_token, tokens.grant).await {
                    Ok(new) => new,
                    // A refresh token is single-use: if it was already spent
                    // an app-only token still gets public browsing working.
                    Err(e) if tokens.grant == Grant::App => {
                        log::warn!("app token refresh failed ({e}); minting a new one");
                        self.mint_app_token().await?
                    }
                    Err(e) => return Err(e),
                }
            }
            Some(tokens) if tokens.grant == Grant::User => {
                anyhow::bail!(
                    "Your SoundCloud session expired. Sign in again in Settings → Account."
                );
            }
            Some(_) | None => self.mint_app_token().await?,
        };
        let token = fresh.access_token.clone();
        fresh.save_for(self.creds.server_url.is_some())?;
        *held = Some(fresh);
        self.client.set_oauth(token.clone());
        Ok(token)
    }

    /// Which grant is in force right now.
    pub async fn grant(&self) -> Option<Grant> {
        self.tokens.lock().await.as_ref().map(|t| t.grant)
    }

    pub async fn signed_in(&self) -> bool {
        self.grant().await == Some(Grant::User)
    }

    /// Run the full browser flow and keep the resulting user tokens.
    pub async fn sign_in(&self) -> Result<()> {
        self.sign_in_notifying(|_| {}).await
    }

    pub async fn sign_in_notifying(&self, notify: impl FnOnce(String)) -> Result<()> {
        let tokens = authorize_notifying(&self.creds, notify).await?;
        // The authorization-code exchange itself proves this is a user grant.
        // Persist it before any resource request: /me can be slow or return a
        // temporary 504, and closing the app during that wait used to lose the
        // otherwise successful login.
        tokens.save_for(self.creds.server_url.is_some())?;
        self.client.set_oauth(tokens.access_token.clone());
        let mut held = self.tokens.lock().await;
        *held = Some(tokens);
        Ok(())
    }

    async fn clear_local_grant(&self) -> Option<Tokens> {
        self.signing_out.store(true, std::sync::atomic::Ordering::SeqCst);
        self.lifecycle.send_modify(|epoch| *epoch = epoch.wrapping_add(1));
        let previous = self.tokens.lock().await.take();
        self.client.clear_oauth();
        self.signing_out.store(false, std::sync::atomic::Ordering::SeqCst);
        previous
    }

    /// Local sign-out works offline. Remote revocation uses only the old grant.
    pub async fn sign_out(&self) -> Result<()> {
        let previous = self.clear_local_grant().await;
        if self.creds.server_url.is_some() { delete_secret(REMOTE_USER_ENTRY)?; }
        else { Tokens::delete()?; }
        if let Some(previous) = previous.filter(|tokens| tokens.grant == Grant::User) {
            let url = self.media_server_url().map_or_else(
                || format!("{}/sign-out",crate::api::client::API_ROOT),
                |server| format!("{server}/v1/soundcloud/api/sign-out"));
            tokio::spawn(async move {
                let result = reqwest::Client::new().post(url)
                    .header(reqwest::header::AUTHORIZATION,format!("OAuth {}",previous.access_token))
                    .timeout(std::time::Duration::from_secs(10)).send().await;
                if result.is_err() || result.is_ok_and(|response| !response.status().is_success()) {
                    log::warn!("Remote sign-out unavailable; the local session has been cleared");
                }
            });
        }
        Ok(())
    }

    /// Revoke the app grant as well as deleting this installation's tokens.
    pub async fn disconnect(&self) -> Result<()> {
        crate::api::endpoints::disconnect(&self.client)
            .await
            .context("disconnect Veirune from SoundCloud")?;
        self.tokens.lock().await.take();
        self.client.clear_oauth();
        Tokens::delete_all()?;
        Ok(())
    }

    async fn mint_app_token(&self) -> Result<Tokens> {
        anyhow::ensure!(self.creds.server_url.is_none(), "sign in to your SoundCloud account first");
        client_credentials(&self.creds).await
    }

    async fn refresh(&self, refresh_token: &str, grant: Grant) -> Result<Tokens> {
        let mut refreshed = if let Some(server_url) = &self.creds.server_url {
            remote_token(server_url, "/v1/oauth/refresh", &serde_json::json!({
                "refresh_token": refresh_token
            })).await?
        } else {
            refresh(&self.creds, refresh_token).await?
        };
        // The response does not say which flow it came from; keep ours.
        refreshed.grant = grant;
        Ok(refreshed)
    }
}

// ===========================================================================
// Flows
// ===========================================================================

/// Authorization Code + PKCE:
///
/// 1. open the browser at `secure.soundcloud.com/authorize`,
/// 2. capture the loopback redirect,
/// 3. exchange the code (with the verifier) for tokens.
pub async fn authorize(creds: &AppCredentials) -> Result<Tokens> {
    authorize_notifying(creds, |_| {}).await
}

async fn authorize_notifying(
    creds: &AppCredentials,
    notify: impl FnOnce(String),
) -> Result<Tokens> {
    anyhow::ensure!(
        creds.redirect_is_loopback(),
        "the registered redirect URI ({}) is not a loopback address; \
         register http://127.0.0.1:{AUTH_PORT}/callback instead",
        creds.redirect_uri
    );
    let pkce = Pkce::generate();
    let state = random_state();
    let port = creds.callback_port();

    // Bound before the browser opens: a port already taken must fail here,
    // not after the user has typed their password.
    let listener = TcpListener::bind(("127.0.0.1", port))
        .with_context(|| format!("listen on 127.0.0.1:{port} for the OAuth redirect"))?;
    listener.set_nonblocking(true)?;

    let url = creds.authorize_url(&pkce, &state);
    notify(url.clone());
    tokio::task::spawn_blocking(move || webbrowser::open(&url))
        .await
        .context("start the browser task")?
        .context("open SoundCloud in your default browser")?;

    let (code, returned_state) = wait_for_callback(listener).await?;
    anyhow::ensure!(
        constant_time_eq(&returned_state, &state),
        "OAuth state mismatch: the redirect did not come from our request"
    );
    if let Some(server_url) = &creds.server_url {
        remote_token(server_url, "/v1/oauth/exchange", &serde_json::json!({
            "code": code, "verifier": pkce.verifier
        })).await
    } else {
        exchange_code(creds, &code, &pkce.verifier).await
    }
}

async fn remote_token(server_url: &str, path: &str, body: &serde_json::Value) -> Result<Tokens> {
    if endpoint::managed(server_url) { let _=endpoint::refresh(false).await; }
    let server_url=endpoint::current(server_url);
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20)).build()?;
    let response = http.post(format!("{server_url}{path}"))
        .json(body).send().await.context("contact approval server")?;
    let status = response.status();
    let value: serde_json::Value = response.json().await.context("read approval server response")?;
    if !status.is_success() {
        anyhow::bail!("approval server: {}", value["error"].as_str()
            .or_else(|| value["message"].as_str()).unwrap_or("request failed"));
    }
    if status == reqwest::StatusCode::ACCEPTED {
        anyhow::ensure!(path == "/v1/oauth/exchange", "unexpected pending token refresh");
        let ticket = value["ticket"].as_str().filter(|ticket| ticket.len() == 43)
            .context("approval server returned no pending ticket")?;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(14 * 60);
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            anyhow::ensure!(tokio::time::Instant::now() < deadline,
                "approval timed out; connect again to request access");
            let response = http.post(format!("{server_url}/v1/oauth/pending"))
                .json(&serde_json::json!({ "ticket": ticket }))
                .send().await;
            let Ok(response) = response else { continue; };
            let status = response.status();
            if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                continue;
            }
            let answer: serde_json::Value = match response.json().await {
                Ok(answer) => answer,
                Err(_) => continue,
            };
            if status == reqwest::StatusCode::ACCEPTED { continue; }
            if !status.is_success() {
                anyhow::bail!("approval server: {}", answer["error"].as_str()
                    .unwrap_or("approval request failed"));
            }
            return Tokens::from_response(&answer, Grant::User);
        }
    }
    Tokens::from_response(&value, Grant::User)
}

/// Wait for the browser to hit the loopback redirect, then answer it.
async fn wait_for_callback(listener: TcpListener) -> Result<(String, String)> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::from_std(listener)?;
    let deadline = tokio::time::Instant::now() + CALLBACK_TIMEOUT;
    loop {
        let accept = tokio::time::timeout_at(deadline, listener.accept())
            .await
            .map_err(|_| anyhow::anyhow!("timed out waiting for the browser to come back"))?;
        let (mut stream, _) = accept?;

        let mut buf = [0u8; 8192];
        let read = stream.read(&mut buf).await?;
        let request = String::from_utf8_lossy(&buf[..read]);
        let target = request
            .lines()
            .next()
            .and_then(|line| line.split(' ').nth(1))
            .unwrap_or_default();
        let params = query_params(target);

        // Browsers ask for /favicon.ico off the same origin; ignore anything
        // that is not the redirect itself and keep listening.
        if params.is_empty() && !target.starts_with("/callback") {
            let _ = stream.write_all(b"HTTP/1.1 204 No Content\r\n\r\n").await;
            continue;
        }

        let code = params.get("code").cloned();
        let state = params.get("state").cloned().unwrap_or_default();
        let error = params.get("error").cloned();
        let description = params.get("error_description").cloned();

        let body = match (&code, &error) {
            (Some(_), _) => page(
                "Signed in",
                "Veirune has your authorization. You can close this tab.",
            ),
            (None, Some(error)) => page(
                "Authorization failed",
                &format!(
                    "SoundCloud said: {}{}",
                    error,
                    description
                        .as_deref()
                        .map(|d| format!(" — {d}"))
                        .unwrap_or_default()
                ),
            ),
            (None, None) => page(
                "Nothing to do",
                "This request carried no authorization code.",
            ),
        };
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.flush().await;

        if let Some(error) = error {
            anyhow::bail!(
                "authorization denied: {error}{}",
                description.map(|d| format!(" ({d})")).unwrap_or_default()
            );
        }
        return Ok((code.context("the redirect carried no code")?, state));
    }
}

/// Exchange an authorization code for tokens.
pub async fn exchange_code(creds: &AppCredentials, code: &str, verifier: &str) -> Result<Tokens> {
    let body = post_token(
        creds,
        &[
            ("grant_type", "authorization_code"),
            ("client_id", &creds.client_id),
            ("client_secret", &creds.client_secret),
            ("redirect_uri", &creds.redirect_uri),
            ("code_verifier", verifier),
            ("code", code),
        ],
        None,
    )
    .await?;
    Tokens::from_response(&body, Grant::User)
}

/// Renew an access token. Refresh tokens are single-use, so the pair this
/// returns must be persisted before anything else uses it.
pub async fn refresh(creds: &AppCredentials, refresh_token: &str) -> Result<Tokens> {
    let body = post_token(
        creds,
        &[
            ("grant_type", "refresh_token"),
            ("client_id", &creds.client_id),
            ("client_secret", &creds.client_secret),
            ("refresh_token", refresh_token),
        ],
        None,
    )
    .await?;
    Tokens::from_response(&body, Grant::User)
}

/// App-only access for public resources. Rate-limited hard (50 per 12 h per
/// app, 30 per hour per IP), so the result is cached like any other token.
pub async fn client_credentials(creds: &AppCredentials) -> Result<Tokens> {
    let body = post_token(
        creds,
        &[("grant_type", "client_credentials")],
        Some(creds.basic_auth()),
    )
    .await?;
    Tokens::from_response(&body, Grant::App)
}

async fn post_token(
    creds: &AppCredentials,
    form: &[(&str, &str)],
    basic: Option<String>,
) -> Result<serde_json::Value> {
    let _ = creds;
    let http = reqwest::Client::new();
    let mut request = http
        .post(format!("{AUTH_ROOT}/oauth/token"))
        .header(reqwest::header::ACCEPT, "application/json; charset=utf-8")
        .form(form);
    if let Some(basic) = basic {
        request = request.header(reqwest::header::AUTHORIZATION, basic);
    }
    let response = request.send().await.context("token request")?;
    let status = response.status();
    let body: serde_json::Value = response.json().await.unwrap_or_default();
    if !status.is_success() {
        // Say what SoundCloud said, not just the code: the usual causes are a
        // redirect URI that does not match, or a spent refresh token.
        let detail = body
            .get("error_description")
            .or_else(|| body.get("error"))
            .and_then(|v| v.as_str())
            .unwrap_or("no detail");
        anyhow::bail!("token request failed ({status}): {detail}");
    }
    Ok(body)
}

/// Bring the client up: resume a session, else app-only, else nothing.
pub async fn start(client: Arc<crate::api::ApiClient>) -> Option<Arc<Session>> {
    let creds = AppCredentials::discover()?;
    let session = Session::new(creds, client);
    match session.resume().await {
        Ok(Some(grant)) => log::info!("resumed a {grant:?} session"),
        Err(error) => {
            log::warn!("could not read the saved session: {error}");
            return None;
        }
        Ok(None) => match session.access_token().await {
            Ok(_) => log::info!("running app-only (public resources)"),
            Err(e) => {
                log::warn!("no usable token: {e}");
                return None;
            }
        },
    }
    Some(session)
}

// ===========================================================================
// Helpers
// ===========================================================================

fn page(title: &str, message: &str) -> String {
    format!(
        "<!doctype html><meta charset=utf-8><title>Veirune</title>\
         <body style=\"background:#121212;color:#fff;font:16px/1.5 system-ui;\
         display:grid;place-items:center;height:100vh;margin:0\">\
         <div style=\"text-align:center\"><h1 style=\"color:#f50\">{title}</h1>\
         <p>{message}</p></div>"
    )
}

/// `?a=1&b=2` from a request target, percent-decoded.
fn query_params(target: &str) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let Some(query) = target.split('?').nth(1) else {
        return out;
    };
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        out.insert(form_decode(key), form_decode(value));
    }
    out
}

/// Percent-encode for a query value (`application/x-www-form-urlencoded`).
pub(crate) fn form_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn form_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn random_state() -> String {
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    b64(&bytes)
}

/// Compare without leaking where two strings differ.
fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_direct_owner_credentials_use_a_fresh_owned_broker_grant() {
        let direct = AppCredentials::new("owner-client".into(),"private-secret".into(),None);
        for origin in [LEGACY_SERVER_ORIGIN,CURRENT_SERVER_ORIGIN] {
            let migrated = direct.for_owned_server(origin).unwrap();
            assert_eq!(migrated.server_url.as_deref(),Some(CURRENT_SERVER_ORIGIN));
            assert!(migrated.client_secret.is_empty());
            assert_eq!(migrated.client_id,direct.client_id);
            assert_eq!(migrated.redirect_uri,direct.redirect_uri);
        }
        assert_eq!(direct.client_secret,"private-secret");
        assert!(direct.for_owned_server("https://custom.example").is_none());
        let remote = AppCredentials {server_url:Some("https://custom.example".into()),..direct};
        assert!(remote.for_owned_server(LEGACY_SERVER_ORIGIN).is_none());
    }

    #[tokio::test]
    async fn offline_sign_out_cancels_refresh_and_clears_the_grant_without_waiting_for_the_network() {
        use tokio::io::AsyncReadExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = Arc::new(crate::api::ApiClient::demo());
        let creds = AppCredentials {client_id:"fixture".into(),client_secret:String::new(),
            redirect_uri:DEFAULT_REDIRECT_URI.into(),server_url:Some(format!("http://{}",listener.local_addr().unwrap()))};
        let session = Session::with_test_user(creds,client.clone()).await;
        {
            let mut tokens = session.tokens.lock().await;
            let tokens = tokens.as_mut().unwrap();
            tokens.expires_at = Some(0);
            tokens.refresh_token = "fixture-refresh".into();
        }
        let waiting = tokio::spawn({let session=session.clone(); async move {session.access_token().await}});
        let (mut socket,_) = listener.accept().await.unwrap();
        let mut buffer=[0;1024];
        assert!(socket.read(&mut buffer).await.unwrap()>0);
        let previous = tokio::time::timeout(std::time::Duration::from_secs(1),session.clear_local_grant()).await.unwrap();
        assert_eq!(previous.unwrap().grant,Grant::User);
        assert!(waiting.await.unwrap().is_err());
        assert_eq!(session.grant().await,None);
        assert!(client.oauth_token().is_none());
        // This state-only helper deliberately leaves the user's real keyring untouched.
    }

    #[test]
    fn replacement_server_only_migrates_the_retired_owned_origin() {
        assert_eq!(replacement_server_origin(LEGACY_SERVER_ORIGIN), Some(CURRENT_SERVER_ORIGIN));
        for origin in [CURRENT_SERVER_ORIGIN, "https://example.com", "https://77.223.107.226.example.com"] {
            assert_eq!(replacement_server_origin(origin), None);
        }
    }

    #[tokio::test]
    async fn an_expired_user_session_is_not_silently_downgraded_or_discarded() {
        let client = Arc::new(crate::api::ApiClient::demo());
        let session = Session::new(
            AppCredentials::new("test".into(), "test".into(), None),
            client,
        );
        *session.tokens.lock().await = Some(Tokens {
            access_token: "test".into(),
            refresh_token: String::new(),
            expires_at: Some(0),
            scope: None,
            grant: Grant::User,
        });
        assert!(session.access_token().await.is_err());
        assert_eq!(session.grant().await, Some(Grant::User));
        assert!(session.access_token().await.is_err());
    }

    #[tokio::test]
    async fn concurrent_requests_reuse_a_fresh_user_token() {
        let client = Arc::new(crate::api::ApiClient::demo());
        let session = Session::new(
            AppCredentials::new("test".into(), "test".into(), None),
            client,
        );
        *session.tokens.lock().await = Some(Tokens {
            access_token: "test".into(),
            refresh_token: String::new(),
            expires_at: Some(chrono::Utc::now().timestamp() + 3600),
            scope: None,
            grant: Grant::User,
        });
        let (a, b) = tokio::join!(session.access_token(), session.access_token());
        assert_eq!(a.unwrap(), b.unwrap());
        assert_eq!(session.grant().await, Some(Grant::User));
    }

    #[tokio::test]
    async fn background_client_shares_refresh_lock_without_replacing_foreground_oauth() {
        let foreground = Arc::new(crate::api::ApiClient::demo());
        foreground.set_oauth("foreground-token".into());
        let session = Session::with_test_user(AppCredentials::new("test".into(), "test".into(), None), foreground.clone()).await;
        let background = Arc::new(crate::api::ApiClient::demo());
        let isolated = session.for_client(background.clone());
        assert_eq!(isolated.access_token().await.unwrap(), "test-user-token");
        assert_eq!(foreground.oauth_token().as_deref(), Some("foreground-token"));
        assert_eq!(background.oauth_token().as_deref(), Some("test-user-token"));
        assert!(Arc::ptr_eq(&session.tokens, &isolated.tokens));
        session.tokens.lock().await.take();
        assert!(isolated.tokens.lock().await.is_none());
    }

    fn creds() -> AppCredentials {
        AppCredentials::new("id-123".into(), "secret-456".into(), None)
    }

    /// OAuth 2.1 requires S256; the verifier must never be sent as the
    /// challenge (which `plain` did, and which this used to do).
    #[test]
    fn the_challenge_is_the_sha256_of_the_verifier() {
        let pkce = Pkce::generate();
        assert_eq!(pkce.verifier.len(), 43, "43 is RFC 7636's minimum");
        assert!(
            pkce.verifier
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
        );
        let challenge = pkce.challenge();
        assert_ne!(challenge, pkce.verifier, "that would be `plain`");
        assert_eq!(challenge.len(), 43, "SHA-256 base64url is 43 characters");
        assert!(!challenge.contains('='), "no padding");
        // Same verifier, same challenge; different verifier, different one.
        assert_eq!(challenge, pkce.challenge());
        assert_ne!(challenge, Pkce::generate().challenge());
    }

    /// The known-answer vector from RFC 7636 appendix B.
    #[test]
    fn the_challenge_matches_the_rfc_vector() {
        let pkce = Pkce {
            verifier: "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into(),
        };
        assert_eq!(
            pkce.challenge(),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn the_authorize_url_carries_every_required_parameter() {
        let pkce = Pkce::generate();
        let url = creds().authorize_url(&pkce, "state-789");
        assert!(url.starts_with("https://secure.soundcloud.com/authorize?"));
        for expected in [
            "client_id=id-123",
            "response_type=code",
            "code_challenge_method=S256",
            "state=state-789",
            &format!("code_challenge={}", pkce.challenge()),
            // The redirect URI is percent-encoded, as a query value must be.
            "redirect_uri=http%3A%2F%2F127.0.0.1%3A41317%2Fcallback",
        ] {
            assert!(url.contains(expected), "{expected} missing from {url}");
        }
        // The verifier itself must never leave the process.
        assert!(!url.contains(&pkce.verifier));
    }

    /// Only HTTP Basic is accepted for the client-credentials grant.
    #[test]
    fn basic_auth_is_base64_of_id_and_secret() {
        // base64("id-123:secret-456")
        assert_eq!(creds().basic_auth(), "Basic aWQtMTIzOnNlY3JldC00NTY=");
    }

    #[test]
    fn the_callback_port_comes_from_the_redirect_uri() {
        assert_eq!(creds().callback_port(), AUTH_PORT);
        let custom = AppCredentials::new(
            "id".into(),
            "secret".into(),
            Some("http://127.0.0.1:9999/oauth".into()),
        );
        assert_eq!(custom.callback_port(), 9999);
        assert!(custom.redirect_is_loopback());
        let scheme = AppCredentials::new(
            "id".into(),
            "secret".into(),
            Some("fastcloud://callback".into()),
        );
        assert!(!scheme.redirect_is_loopback());
        // An unparsable port falls back rather than panicking.
        assert_eq!(scheme.callback_port(), AUTH_PORT);
    }

    #[test]
    fn tokens_come_out_of_a_response_with_their_grant() {
        let body = serde_json::json!({
            "access_token": "a",
            "refresh_token": "r",
            "expires_in": 3600,
            "scope": "non-expiring",
        });
        let t = Tokens::from_response(&body, Grant::User).unwrap();
        assert_eq!(t.access_token, "a");
        assert_eq!(t.refresh_token, "r");
        assert_eq!(t.scope.as_deref(), Some("non-expiring"));
        assert_eq!(t.grant, Grant::User);
        assert!(!t.expired());
        assert!(t.can_refresh());

        // An app token may come back without a refresh token.
        let app = Tokens::from_response(
            &serde_json::json!({ "access_token": "b", "expires_in": 3600 }),
            Grant::App,
        )
        .unwrap();
        assert!(!app.can_refresh());
        assert_eq!(app.grant, Grant::App);

        // A response with no token at all is an error, not an empty token.
        assert!(Tokens::from_response(&serde_json::json!({}), Grant::App).is_err());
        assert!(
            Tokens::from_response(&serde_json::json!({ "access_token": "" }), Grant::App).is_err()
        );
    }

    /// A token about to expire counts as expired, so no request starts with
    /// one that dies mid-flight.
    #[test]
    fn tokens_expire_early_by_the_refresh_skew() {
        let mut t = Tokens::from_response(
            &serde_json::json!({ "access_token": "a", "expires_in": 3600 }),
            Grant::User,
        )
        .unwrap();
        assert!(!t.expired());
        t.expires_at = Some(chrono::Utc::now().timestamp() + REFRESH_SKEW_SECS - 1);
        assert!(t.expired(), "inside the skew window");
        t.expires_at = None;
        assert!(t.expired(), "an unknown expiry is treated as expired");
    }

    #[test]
    fn redirect_parameters_are_decoded() {
        let params = query_params("/callback?code=abc%2Ddef&state=x%20y");
        assert_eq!(params.get("code").unwrap(), "abc-def");
        assert_eq!(params.get("state").unwrap(), "x y");
        // A denial comes back as an error instead of a code.
        let denied = query_params("/callback?error=access_denied&error_description=User+said+no");
        assert_eq!(denied.get("error").unwrap(), "access_denied");
        assert_eq!(denied.get("error_description").unwrap(), "User said no");
        assert!(query_params("/favicon.ico").is_empty());
    }

    #[test]
    fn form_encoding_round_trips() {
        for value in [
            "http://127.0.0.1:41317/callback",
            "a b+c",
            "%",
            "простой",
            "",
        ] {
            assert_eq!(form_decode(&form_encode(value)), value, "{value}");
        }
    }

    #[test]
    fn state_comparison_rejects_mismatches() {
        let state = random_state();
        assert!(constant_time_eq(&state, &state.clone()));
        assert!(!constant_time_eq(&state, "short"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq(&state, ""));
    }
}
