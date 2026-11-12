//! An offline-owner-signed address book, independent of the old VPS.
use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::{
    sync::OnceLock,
    time::{Duration, Instant},
};

const ADDRESS_BOOK: &str = "https://veirune.comf.workers.dev/service.json";
const PUBLIC_KEY: &str = include_str!("service-endpoint-public-key.txt");
const MAX_BYTES: usize = 8192;

#[derive(Clone, Deserialize, Serialize)]
pub struct Envelope {
    schema: u8,
    payload: String,
    signature: String,
}
#[derive(Clone, Deserialize)]
pub struct Announcement {
    service: String,
    pub serial: u64,
    pub origin: String,
    #[serde(default)]
    previous: Vec<String>,
}
struct Routing {
    announcement: Option<Announcement>,
    attempted: Option<Instant>,
}

fn accepts(old: &Announcement, next: &Announcement) -> Result<()> {
    anyhow::ensure!(next.serial >= old.serial, "Endpoint rollback rejected");
    anyhow::ensure!(
        next.serial != old.serial || next.origin == old.origin,
        "Conflicting endpoint sequence"
    );
    Ok(())
}

fn manages(origin: &str, entry: Option<&Announcement>) -> bool {
    matches!(
        origin,
        super::CURRENT_SERVER_ORIGIN | super::LEGACY_SERVER_ORIGIN
    ) || entry.is_some_and(|entry| {
        entry.origin == origin || entry.previous.iter().any(|old| old == origin)
    })
}
static ROUTING: OnceLock<RwLock<Routing>> = OnceLock::new();
static FETCH: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static CHANGED: tokio::sync::Notify = tokio::sync::Notify::const_new();
pub async fn changed() {
    CHANGED.notified().await;
}

fn cache_path() -> Option<std::path::PathBuf> {
    crate::config::app_paths()
        .ok()
        .map(|paths| paths.root.join("service-endpoint.json"))
}

pub fn public_origin(value: &str) -> Result<String> {
    let checked = super::checked_server_url(value)?;
    let url = url::Url::parse(&checked)?;
    anyhow::ensure!(
        url.scheme() == "https" && url.port().is_none_or(|port| port == 443),
        "Invalid public endpoint"
    );
    let host = url.host_str().context("Endpoint has no host")?;
    anyhow::ensure!(
        !matches!(host, "localhost" | "::1")
            && !host.ends_with(".localhost")
            && !host.ends_with(".local"),
        "Local endpoints cannot be announced"
    );
    if let Ok(address) = host.parse::<std::net::IpAddr>() {
        let private = match address {
            std::net::IpAddr::V4(ip) => {
                ip.is_private()
                    || ip.is_loopback()
                    || ip.is_link_local()
                    || ip.is_unspecified()
                    || ip.is_multicast()
                    || ip.is_broadcast()
                    || ip.octets()[0] == 0
            }
            std::net::IpAddr::V6(ip) => {
                ip.is_loopback()
                    || ip.is_unspecified()
                    || ip.is_multicast()
                    || ip.is_unique_local()
                    || ip.is_unicast_link_local()
            }
        };
        anyhow::ensure!(!private, "Local endpoints cannot be announced");
    }
    Ok(checked)
}

fn verify_with_key(bytes: &[u8], public: &[u8]) -> Result<Announcement> {
    anyhow::ensure!(
        bytes.len() <= MAX_BYTES,
        "Endpoint announcement is too large"
    );
    let value: Envelope = serde_json::from_slice(bytes)?;
    anyhow::ensure!(value.schema == 1, "Unsupported endpoint announcement");
    let payload = STANDARD.decode(value.payload)?;
    let signature = STANDARD.decode(value.signature)?;
    // RFC 5480 SubjectPublicKeyInfo for an uncompressed P-256 public key.
    const PREFIX: &[u8] = &[
        0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08,
        0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
    ];
    anyhow::ensure!(
        public.len() == 91 && public.starts_with(PREFIX),
        "Invalid endpoint verification key"
    );
    ring::signature::UnparsedPublicKey::new(
        &ring::signature::ECDSA_P256_SHA256_ASN1,
        &public[PREFIX.len()..],
    )
    .verify(&payload, &signature)
    .map_err(|_| anyhow::anyhow!("Invalid endpoint signature"))?;
    let mut announcement: Announcement = serde_json::from_slice(&payload)?;
    anyhow::ensure!(
        announcement.service == "veirune-main"
            && announcement.serial > 0
            && announcement.serial < (1u64 << 53)
            && announcement.previous.len() <= 16,
        "Invalid endpoint identity"
    );
    announcement.origin = public_origin(&announcement.origin)?;
    for previous in &announcement.previous {
        public_origin(previous)?;
    }
    Ok(announcement)
}

fn verify(bytes: &[u8]) -> Result<Announcement> {
    verify_with_key(bytes, &STANDARD.decode(PUBLIC_KEY.trim())?)
}

fn routing() -> &'static RwLock<Routing> {
    ROUTING.get_or_init(|| {
        let announcement = cache_path()
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| verify(&bytes).ok());
        RwLock::new(Routing {
            announcement,
            attempted: None,
        })
    })
}

pub fn managed(origin: &str) -> bool {
    manages(origin, routing().read().announcement.as_ref())
}

pub fn current(origin: &str) -> String {
    if !managed(origin) {
        return origin.to_owned();
    }
    routing()
        .read()
        .announcement
        .as_ref()
        .map_or_else(|| origin.to_owned(), |entry| entry.origin.clone())
}

/// Public discovery never receives OAuth, cookies, or SoundCloud credentials.
/// A bad/offline address book leaves the last verified address in use.
pub async fn refresh(force: bool) -> Result<bool> {
    let _guard = FETCH.lock().await;
    if !force
        && routing()
            .read()
            .attempted
            .is_some_and(|last| last.elapsed() < Duration::from_secs(45))
    {
        return Ok(false);
    }
    routing().write().attempted = Some(Instant::now());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(4))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let mut response = client
        .get(ADDRESS_BOOK)
        .header(reqwest::header::CACHE_CONTROL, "no-cache")
        .send()
        .await?
        .error_for_status()?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        anyhow::ensure!(
            bytes.len() + chunk.len() <= MAX_BYTES,
            "Endpoint announcement is too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    let next = verify(&bytes)?;
    let changed = {
        let mut routing = routing().write();
        if let Some(old) = routing.announcement.as_ref() {
            accepts(old, &next)?;
            if next.serial == old.serial {
                return Ok(false);
            }
        }
        let before = routing
            .announcement
            .as_ref()
            .map_or(super::CURRENT_SERVER_ORIGIN, |entry| entry.origin.as_str());
        let changed = before != next.origin;
        if let Some(path) = cache_path() {
            let temporary = path.with_extension("json.part");
            std::fs::write(&temporary, &bytes)?;
            if cfg!(target_os = "windows") && path.exists() {
                std::fs::remove_file(&path)?;
            }
            std::fs::rename(temporary, path)?;
        }
        routing.announcement = Some(next);
        changed
    };
    if changed {
        CHANGED.notify_one();
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn announced_addresses_cannot_target_local_services_or_embed_credentials() {
        for value in [
            "http://example.com",
            "https://127.0.0.1",
            "https://10.0.0.1",
            "https://localhost",
            "https://user@example.com",
            "https://example.com/path",
            "https://example.com:8080",
        ] {
            assert!(public_origin(value).is_err(), "{value}");
        }
        assert_eq!(
            public_origin("https://89.125.126.32/").unwrap(),
            "https://89.125.126.32"
        );
    }
    #[test]
    fn signed_fixture_is_verified_and_tampering_is_rejected() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("endpoint-fixture.json")).unwrap();
        let public = STANDARD
            .decode(fixture["publicKey"].as_str().unwrap())
            .unwrap();
        let mut envelope = fixture["envelope"].clone();
        assert_eq!(
            verify_with_key(&serde_json::to_vec(&envelope).unwrap(), &public)
                .unwrap()
                .serial,
            2
        );
        envelope["payload"] = serde_json::Value::String(STANDARD.encode(
            b"{\"service\":\"veirune-main\",\"serial\":3,\"origin\":\"https://evil.example\"}",
        ));
        assert!(verify_with_key(&serde_json::to_vec(&envelope).unwrap(), &public).is_err());
    }

    #[test]
    fn the_pinned_product_key_verifies_the_initial_website_announcement() {
        assert_eq!(
            verify(include_bytes!("endpoint-initial.json"))
                .unwrap()
                .origin,
            super::super::CURRENT_SERVER_ORIGIN
        );
    }

    #[test]
    fn old_addresses_are_managed_but_custom_servers_are_not_replaced() {
        let entry = Announcement {
            service: "veirune-main".into(),
            serial: 3,
            origin: "https://new.example".into(),
            previous: vec!["https://previous.example".into()],
        };
        assert!(manages(super::super::CURRENT_SERVER_ORIGIN, Some(&entry)));
        assert!(manages("https://previous.example", Some(&entry)));
        assert!(!manages("https://custom.example", Some(&entry)));
        let older = Announcement {
            serial: 2,
            ..entry.clone()
        };
        assert!(accepts(&entry, &older).is_err());
        let conflict = Announcement {
            origin: "https://other.example".into(),
            ..entry.clone()
        };
        assert!(accepts(&entry, &conflict).is_err());
        assert!(accepts(&entry, &entry).is_ok());
    }
}
