#![allow(dead_code)]

use anyhow::{Context, Result};
use std::sync::Arc;

const MAX_SEGMENT_BYTES: usize = 8 * 1024 * 1024;
const PREFETCH_SEGMENTS: usize = 2;

type SegmentQueue = Arc<tokio::sync::Mutex<tokio::sync::mpsc::Receiver<Result<Vec<u8>>>>>;

/// A small ordered queue of compressed audio, filled independently of decoding.
/// Dropping the owner cancels its network task even if a receiver is awaiting it.
pub struct SegmentPrefetch {
    pub queue: SegmentQueue,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for SegmentPrefetch {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// A minimal M3U8 (HLS) playlist parser: media segments + optional init segment.
#[derive(Debug, Clone, Default)]
pub struct MediaPlaylist {
    /// Leading `#EXT-X-MAP` init segment URI (fMP4).
    pub init_uri: Option<String>,
    /// Segment URIs in play order.
    pub segments: Vec<String>,
    /// Exact `#EXTINF` duration for each segment, in seconds.
    pub segment_durations: Vec<f64>,
    /// Target duration in seconds (used for buffering heuristics).
    pub target_duration: Option<f64>,
}

impl MediaPlaylist {
    pub fn duration_ms(&self) -> Option<u64> {
        if self.segments.is_empty() {
            return None;
        }
        let fallback = self.target_duration.filter(|seconds| *seconds > 0.0).unwrap_or(10.0);
        Some(self.segments.iter().enumerate().map(|(index, _)| {
            let seconds = self.segment_durations.get(index).copied().filter(|seconds| *seconds > 0.0).unwrap_or(fallback);
            (seconds * 1000.0).round() as u64
        }).sum())
    }

    /// Pick the segment containing `target_ms` and return its absolute start.
    /// Seeking can then download only that segment onward instead of replaying
    /// the entire stream from byte zero.
    pub fn seek_point(&self, target_ms: u64) -> (usize, u64) {
        if self.segments.is_empty() {
            return (0, 0);
        }
        let fallback_ms = self
            .target_duration
            .map(|seconds| (seconds * 1000.0).round() as u64)
            .filter(|duration| *duration > 0)
            .unwrap_or(10_000);
        let mut start_ms = 0u64;
        for index in 0..self.segments.len() {
            let duration_ms = self
                .segment_durations
                .get(index)
                .map(|seconds| (seconds * 1000.0).round() as u64)
                .filter(|duration| *duration > 0)
                .unwrap_or(fallback_ms);
            if target_ms < start_ms.saturating_add(duration_ms) {
                return (index, start_ms);
            }
            start_ms = start_ms.saturating_add(duration_ms);
        }
        let last = self.segments.len() - 1;
        let last_duration = self
            .segment_durations
            .get(last)
            .map(|seconds| (seconds * 1000.0).round() as u64)
            .filter(|duration| *duration > 0)
            .unwrap_or(fallback_ms);
        (last, start_ms.saturating_sub(last_duration))
    }
}

pub fn parse(content: &str, base_url: &str) -> Result<MediaPlaylist> {
    anyhow::ensure!(
        content.trim_start().starts_with("#EXTM3U"),
        "invalid HLS playlist"
    );
    let mut pl = MediaPlaylist::default();
    let mut next_duration = None;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with("#EXT-X-TARGETDURATION:") {
            pl.target_duration = line.split(':').nth(1).and_then(|v| v.trim().parse().ok());
        } else if line.starts_with("#EXT-X-MAP:") {
            let uri = attribute_value(line, "URI").context("#EXT-X-MAP without URI")?;
            pl.init_uri = Some(absolute(&uri, base_url)?);
        } else if line.starts_with("#EXTINF:") {
            next_duration = line
                .strip_prefix("#EXTINF:")
                .and_then(|value| value.split(',').next())
                .and_then(|value| value.trim().parse::<f64>().ok());
        } else if !line.starts_with('#') && next_duration.is_some() {
            pl.segments.push(absolute(line, base_url)?);
            pl.segment_durations
                .push(next_duration.take().unwrap_or_default());
        }
    }
    Ok(pl)
}

fn attribute_value(line: &str, key: &str) -> Option<String> {
    // #EXT-X-MAP:URI="init.mp4",BYTERANGE="..."
    let needle = format!("{key}=\"");
    let start = line.find(&needle)? + needle.len();
    let rest = &line[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn absolute(uri: &str, base: &str) -> Result<String> {
    if uri.starts_with("http://") || uri.starts_with("https://") {
        Ok(uri.to_string())
    } else if let Ok(base_url) = url::Url::parse(base) {
        Ok(base_url.join(uri)?.to_string())
    } else {
        anyhow::bail!("cannot resolve relative URI {uri:?} against {base:?}")
    }
}

/// Streaming HLS segment fetcher with disk cache support.
pub struct HlsDownloader {
    http: reqwest::Client,
    oauth: parking_lot::RwLock<Option<String>>,
    pub cache: std::sync::Arc<super::cache::AudioCache>,
}

impl HlsDownloader {
    pub fn prefetch(
        self: &Arc<Self>,
        urn: String,
        urls: Vec<String>,
        runtime: &tokio::runtime::Handle,
    ) -> SegmentPrefetch {
        let (sender, receiver) = tokio::sync::mpsc::channel(PREFETCH_SEGMENTS);
        let downloader = self.clone();
        let task = runtime.spawn(async move {
            for url in urls {
                // Reserve before downloading: queued plus in-flight segments
                // never exceed two, including while playback is paused.
                let Ok(permit) = sender.reserve().await else {
                    return;
                };
                let mut attempts = 0;
                let result = loop {
                    let result = downloader.segment(&urn, &url).await;
                    if let Err(error) = &result
                        && attempts < 2
                        && retry_segment(error)
                    {
                        attempts += 1;
                        // Keep the decoded buffer playing while a brief broker restart completes.
                        tokio::time::sleep(std::time::Duration::from_secs(attempts)).await;
                        continue;
                    }
                    break result;
                };
                let failed = result.is_err();
                permit.send(result);
                if failed {
                    return;
                }
            }
        });
        SegmentPrefetch {
            queue: Arc::new(tokio::sync::Mutex::new(receiver)),
            task,
        }
    }
    pub fn new(http: reqwest::Client, cache: std::sync::Arc<super::cache::AudioCache>) -> Self {
        Self {
            http,
            cache,
            oauth: parking_lot::RwLock::new(None),
        }
    }

    pub fn set_oauth(&self, token: Option<String>) {
        *self.oauth.write() = token;
    }

    fn request(&self, url: &str) -> reqwest::RequestBuilder {
        let mut request = self.http.get(url).timeout(std::time::Duration::from_secs(20));
        // Stream URLs now start on the API host and redirect to a CDN.
        // Authorize that first hop only; reqwest strips it on cross-host redirects.
        if needs_oauth(url)
            && let Some(token) = self.oauth.read().as_ref()
        {
            request = request.header(reqwest::header::AUTHORIZATION, format!("OAuth {token}"));
        }
        request
    }

    /// Fetch and parse a media playlist.
    pub async fn playlist(&self, url: &str) -> Result<MediaPlaylist> {
        if let Some(path) = local_file(url) {
            let text = std::fs::read_to_string(path)?;
            return parse(&text, url);
        }
        let response = self.request(url).send().await?;
        let text = audio_response(response)?.text().await?;
        parse(&text, url)
    }

    /// Download one segment, using the disk cache when possible.
    pub async fn segment(&self, track_urn: &str, url: &str) -> Result<Vec<u8>> {
        if let Some(path) = local_file(url) {
            return Ok(std::fs::read(path)?);
        }
        let cached = tokio::task::spawn_blocking({
            let cache = self.cache.clone();
            let track_urn = track_urn.to_owned();
            let url = url.to_owned();
            move || cache.get_segment(&track_urn, &url)
        })
        .await
        .unwrap_or(None);
        if let Some(hit) = cached {
            if validate_segment(&hit).is_ok() { return Ok(hit); }
        }
        let resp = audio_response(self.request(url).send().await?)?;
        let status = resp.status();
        if !status.is_success() {
            anyhow::bail!("segment fetch {status}: {url}");
        }
        let bytes = read_segment(resp).await?;
        self.cache_segment(track_urn, url, &bytes).await;
        Ok(bytes)
    }

    /// Download the fMP4 init segment (cached).
    pub async fn init_segment(&self, track_urn: &str, url: &str) -> Result<Vec<u8>> {
        if let Some(path) = local_file(url) {
            return Ok(std::fs::read(path)?);
        }
        let cached = tokio::task::spawn_blocking({
            let cache = self.cache.clone();
            let track_urn = track_urn.to_owned();
            let url = url.to_owned();
            move || cache.get_segment(&track_urn, &url)
        })
        .await
        .unwrap_or(None);
        if let Some(hit) = cached {
            if validate_segment(&hit).is_ok() { return Ok(hit); }
        }
        let resp = audio_response(self.request(url).send().await?)?;
        let bytes = read_segment(resp).await?;
        self.cache_segment(track_urn, url, &bytes).await;
        Ok(bytes)
    }

    async fn cache_segment(&self, track_urn: &str, url: &str, bytes: &[u8]) {
        let cache = self.cache.clone();
        let track_urn = track_urn.to_owned();
        let url = url.to_owned();
        let bytes = bytes.to_vec();
        let _ =
            tokio::task::spawn_blocking(move || cache.put_segment(&track_urn, &url, &bytes)).await;
    }
}

fn audio_response(response: reqwest::Response) -> Result<reqwest::Response> {
    let status = response.status();
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let retry_after_ms = response.headers().get("Retry-After").and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok()).map(|seconds| seconds.saturating_mul(1000));
        return Err(crate::api::error::ApiError::RateLimited {retry_after_ms}.into());
    }
    let path = response.url().path();
    let ticket = |value: &str| value.len() == 43
        && value.bytes().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'_' | b'-'));
    let media = path.strip_prefix("/v1/media/").and_then(|rest| rest.split_once('/'))
        .is_some_and(|(key, name)| ticket(key) && !name.is_empty() && !name.contains('/'));
    let relay = path.strip_prefix("/v1/soundcloud/asset/").is_some_and(ticket);
    // Older brokers used 401 for lost playback tickets. These GET endpoints
    // use a capability, not the user's login; a fresh stream resolves a new one.
    if (media && matches!(status.as_u16(), 401 | 410)) || (relay && status.as_u16() == 410) {
        return Err(crate::api::error::ApiError::PlaybackLinkExpired.into());
    }
    Ok(response.error_for_status()?)
}

async fn read_segment(mut response: reqwest::Response) -> Result<Vec<u8>> {
    anyhow::ensure!(
        response.content_length().is_none_or(|size| size <= MAX_SEGMENT_BYTES as u64),
        "Audio segment exceeds the size limit"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        anyhow::ensure!(chunk.len() <= MAX_SEGMENT_BYTES.saturating_sub(bytes.len()), "Audio segment exceeds the size limit");
        bytes.extend_from_slice(&chunk);
    }
    validate_segment(&bytes)?;
    Ok(bytes)
}

fn validate_segment(bytes: &[u8]) -> Result<()> {
    anyhow::ensure!(!bytes.is_empty(), "Audio server returned an empty segment");
    // Some gateways return an error page with HTTP 200. Never cache it as
    // audio or let the decoder report an opaque format-probe failure later.
    let prefix = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]);
    let prefix = prefix.trim_start_matches('\u{feff}').trim_start().to_ascii_lowercase();
    anyhow::ensure!(
        !prefix.starts_with("<!doctype html") && !prefix.starts_with("<html")
            && !prefix.starts_with("<?xml") && !prefix.starts_with("{\"error")
            && !prefix.starts_with("#extm3u"),
        "Audio server returned a page or playlist instead of an audio segment"
    );
    Ok(())
}

fn retry_segment(error: &anyhow::Error) -> bool {
    error.downcast_ref::<reqwest::Error>().is_some_and(|error| {
        error.is_timeout() || error.is_connect() || error.is_body()
            || error.status().is_some_and(|status| status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS)
    })
}

fn local_file(url: &str) -> Option<std::path::PathBuf> {
    let parsed = url::Url::parse(url).ok()?;
    if parsed.scheme() != "file" {
        return None;
    }
    let path = parsed.to_file_path().ok()?.canonicalize().ok()?;
    let root = crate::config::app_paths()
        .ok()?
        .root
        .join("offline")
        .canonicalize()
        .ok()?;
    path.starts_with(root).then_some(path)
}

fn needs_oauth(url: &str) -> bool {
    url::Url::parse(url)
        .is_ok_and(|url| url.scheme() == "https" && url.host_str() == Some("api.soundcloud.com"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn lost_playback_tickets_are_distinct_from_account_authentication() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let directory = tempfile::tempdir().unwrap();
        let cache = Arc::new(super::super::cache::AudioCache::new(directory.path().to_path_buf(), 1024).unwrap());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            for status in [401, 410, 401, 410, 401] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0; 4096];
                socket.read(&mut request).await.unwrap();
                socket.write_all(format!("HTTP/1.1 {status} Error\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}").as_bytes()).await.unwrap();
            }
        });
        let downloader = HlsDownloader::new(reqwest::Client::new(), cache);
        let path = format!("{origin}/v1/media/{}", "a".repeat(43));
        let error = downloader.playlist(&format!("{path}/index.m3u8")).await.unwrap_err();
        assert!(error.to_string().contains("Audio link"), "{error}");
        let error = downloader.segment("track", &format!("{path}/part.seg")).await.unwrap_err();
        assert!(error.to_string().contains("Audio link"), "{error}");
        let error = downloader.init_segment("track", &format!("{path}/init.seg")).await.unwrap_err();
        assert!(error.to_string().contains("Audio link"), "{error}");
        let error = downloader.playlist(&format!("{origin}/v1/soundcloud/asset/{}", "b".repeat(43))).await.unwrap_err();
        assert!(error.to_string().contains("Audio link"), "{error}");
        let error = downloader.playlist(&format!("{origin}/v1/soundcloud/api/me")).await.unwrap_err();
        assert_eq!(error.downcast_ref::<reqwest::Error>().unwrap().status(), Some(reqwest::StatusCode::UNAUTHORIZED));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn cached_error_pages_are_refetched_and_http_200_errors_are_not_cached() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let directory = tempfile::tempdir().unwrap();
        let cache = Arc::new(super::super::cache::AudioCache::new(directory.path().to_path_buf(), 1024).unwrap());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let good = format!("{base}/good");
        let bad = format!("{base}/bad");
        cache.put_segment("track", &good, b"<!doctype html><html>gateway error</html>");
        let server = tokio::spawn(async move {
            for body in [b"audio fixture".as_slice(), b"<html>unavailable</html>".as_slice()] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0; 4096];
                socket.read(&mut request).await.unwrap();
                let header = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                socket.write_all(header.as_bytes()).await.unwrap();
                socket.write_all(body).await.unwrap();
            }
        });
        let downloader = HlsDownloader::new(reqwest::Client::new(), cache.clone());
        assert_eq!(downloader.segment("track", &good).await.unwrap(), b"audio fixture");
        assert_eq!(cache.get_segment("track", &good).unwrap(), b"audio fixture");
        assert!(downloader.segment("track", &bad).await.unwrap_err().to_string().contains("instead of an audio segment"));
        assert!(cache.get_segment("track", &bad).is_none());
        assert!(validate_segment(&[]).is_err());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn prefetch_delivers_first_segment_before_slow_next_one_and_bounds_lookahead() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (seen, mut requests) = tokio::sync::mpsc::unbounded_channel();
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let worker_gate = gate.clone();
        let server = tokio::spawn(async move {
            for index in 0..4 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0; 4096];
                socket.read(&mut request).await.unwrap();
                seen.send(index).unwrap();
                if index > 0 { worker_gate.acquire().await.unwrap().forget(); }
                let body = format!("segment-{index}");
                let response = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let downloader = Arc::new(HlsDownloader::new(reqwest::Client::new(), Arc::new(super::super::cache::AudioCache::disabled())));
        let prefetch = downloader.prefetch("track".into(), (0..4).map(|index| format!("{base}/{index}")).collect(), &tokio::runtime::Handle::current());
        assert_eq!(requests.recv().await, Some(0));
        // A blocked second download must not hold back the completed first one.
        let first = tokio::time::timeout(std::time::Duration::from_secs(2), async { prefetch.queue.lock().await.recv().await.unwrap().unwrap() }).await.unwrap();
        assert_eq!(first, b"segment-0");
        assert_eq!(requests.recv().await, Some(1));
        gate.add_permits(1);
        // The next request starts while the consumer is still playing its
        // first audio; the decoder never has to initiate that cold request.
        assert_eq!(requests.recv().await, Some(2));
        assert!(requests.try_recv().is_err());
        gate.add_permits(1);
        // Reserve-before-fetch bounds both ready and in-flight data to two.
        assert!(tokio::time::timeout(std::time::Duration::from_millis(80), requests.recv()).await.is_err());
        assert_eq!(prefetch.queue.lock().await.recv().await.unwrap().unwrap(), b"segment-1");
        assert_eq!(requests.recv().await, Some(3));
        gate.add_permits(1);
        assert_eq!(prefetch.queue.lock().await.recv().await.unwrap().unwrap(), b"segment-2");
        assert_eq!(prefetch.queue.lock().await.recv().await.unwrap().unwrap(), b"segment-3");
        assert!(prefetch.queue.lock().await.recv().await.is_none());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn replacing_prefetch_cancels_an_inflight_request_and_wakes_the_old_receiver() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (began, waiting) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            socket.read(&mut request).await.unwrap();
            // The body stays stalled until the aborted download closes TCP.
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n").await.unwrap();
            began.send(()).unwrap();
            // Aborting a response with an unread body may close TCP with a
            // reset (notably on macOS) rather than a graceful EOF.
            match socket.read(&mut request).await {
                Ok(0) => {}
                Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
                other => panic!("Expected the cancelled download to close TCP, got {other:?}"),
            }
        });
        let downloader = Arc::new(HlsDownloader::new(reqwest::Client::new(), Arc::new(super::super::cache::AudioCache::disabled())));
        let prefetch = downloader.prefetch("old-track".into(), vec![format!("{base}/segment")], &tokio::runtime::Handle::current());
        let queue = prefetch.queue.clone();
        waiting.await.unwrap();
        assert!(matches!(queue.lock().await.try_recv(), Err(tokio::sync::mpsc::error::TryRecvError::Empty)));
        drop(prefetch);
        assert!(tokio::time::timeout(std::time::Duration::from_secs(2), async { queue.lock().await.recv().await }).await.unwrap().is_none());
        tokio::time::timeout(std::time::Duration::from_secs(2), server).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn prefetch_retries_transient_failures_but_never_retries_owner_denial() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            for status in ["503 Service Unavailable", "200 OK", "403 Forbidden"] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0; 4096];
                socket.read(&mut request).await.unwrap();
                let response = format!("HTTP/1.1 {status}\r\nContent-Length: 5\r\nConnection: close\r\n\r\naudio");
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let downloader = Arc::new(HlsDownloader::new(reqwest::Client::new(), Arc::new(super::super::cache::AudioCache::disabled())));
        let prefetch = downloader.prefetch("track".into(), vec![format!("{base}/first"), format!("{base}/denied")], &tokio::runtime::Handle::current());
        assert_eq!(prefetch.queue.lock().await.recv().await.unwrap().unwrap(), b"audio");
        let failure = prefetch.queue.lock().await.recv().await.unwrap().unwrap_err();
        assert_eq!(failure.downcast_ref::<reqwest::Error>().unwrap().status(), Some(reqwest::StatusCode::FORBIDDEN));
        assert!(prefetch.queue.lock().await.recv().await.is_none());
        server.await.unwrap();
    }

    const PL: &str = "#EXTM3U\n\
        #EXT-X-TARGETDURATION:10\n\
        #EXT-X-MAP:URI=\"init.mp4\"\n\
        #EXTINF:9.9,\n\
        seg1.ts\n\
        #EXTINF:9.9,\n\
        https://cdn.example/seg2.ts\n\
        #EXT-X-ENDLIST\n";

    #[test]
    fn parse_absolute_and_relative() {
        let pl = parse(PL, "https://media.example/hls/playlist.m3u8").unwrap();
        assert_eq!(
            pl.init_uri.as_deref(),
            Some("https://media.example/hls/init.mp4")
        );
        assert_eq!(pl.segments.len(), 2);
        assert_eq!(pl.segments[0], "https://media.example/hls/seg1.ts");
        assert_eq!(pl.segments[1], "https://cdn.example/seg2.ts");
        assert_eq!(pl.segment_durations, vec![9.9, 9.9]);
        assert_eq!(pl.duration_ms(), Some(19_800));
        assert_eq!(pl.target_duration, Some(10.0));
    }

    #[test]
    fn parse_no_map() {
        let pl = parse("#EXTM3U\n#EXTINF:5.0,\na.mp4\n", "https://x/y.m3u8").unwrap();
        assert!(pl.init_uri.is_none());
        assert_eq!(pl.segments, vec!["https://x/a.mp4"]);
    }

    #[test]
    fn seek_starts_at_the_segment_containing_the_target() {
        let pl = parse(PL, "https://media.example/hls/playlist.m3u8").unwrap();
        assert_eq!(pl.seek_point(0), (0, 0));
        assert_eq!(pl.seek_point(9_899), (0, 0));
        assert_eq!(pl.seek_point(9_900), (1, 9_900));
        assert_eq!(pl.seek_point(15_000), (1, 9_900));
    }

    #[tokio::test]
    async fn saved_playlist_plays_from_app_offline_directory() {
        let root = crate::config::app_paths().unwrap().root.join("offline");
        std::fs::create_dir_all(&root).unwrap();
        let directory = tempfile::tempdir_in(root).unwrap();
        std::fs::write(directory.path().join("segment-0000.bin"), b"saved audio").unwrap();
        std::fs::write(
            directory.path().join("playlist.m3u8"),
            "#EXTM3U\n#EXTINF:3.0,\nsegment-0000.bin\n#EXT-X-ENDLIST\n",
        )
        .unwrap();
        let url = url::Url::from_file_path(directory.path().join("playlist.m3u8"))
            .unwrap()
            .to_string();
        let downloader = HlsDownloader::new(
            reqwest::Client::new(),
            std::sync::Arc::new(super::super::cache::AudioCache::disabled()),
        );
        let playlist = downloader.playlist(&url).await.unwrap();
        assert_eq!(playlist.segments.len(), 1);
        assert_eq!(
            downloader
                .segment("track", &playlist.segments[0])
                .await
                .unwrap(),
            b"saved audio"
        );
    }
}
