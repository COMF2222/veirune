use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Only broker-owned capabilities cross the renderer bridge. Neither the
/// listener's OAuth token nor SoundCloud's license token belongs in the UI.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WebPlaybackSource {
    pub playlist_path: String,
    pub license_paths: BTreeMap<String, String>,
    pub duration_ms: u64,
    pub bitrate_kbps: u32,
}

pub fn resource_path(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("/v1/media/web/") else {
        return false;
    };
    let Some((ticket, resource)) = rest.split_once('/') else {
        return false;
    };
    if ticket.len() != 43
        || !ticket
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return false;
    }
    matches!(
        resource,
        "index.m3u8" | "license/widevine" | "license/playready" | "license/fairplay" | "started"
    ) || resource.strip_prefix("asset/").is_some_and(|index| {
        !index.is_empty()
            && index.len() <= 4
            && index.bytes().all(|b| b.is_ascii_digit())
            && index.parse::<usize>().is_ok_and(|n| n < 4096)
    })
}

impl WebPlaybackSource {
    pub fn valid(&self) -> bool {
        let Some(base) = self.playlist_path.strip_suffix("/index.m3u8") else {
            return false;
        };
        resource_path(&self.playlist_path)
            && self.duration_ms > 0
            && self.duration_ms <= 24 * 3600 * 1000
            && (32..=320).contains(&self.bitrate_kbps)
            && self.license_paths.iter().all(|(system, path)| {
                let suffix = match system.as_str() {
                    "com.widevine.alpha" => "widevine",
                    "com.microsoft.playready" => "playready",
                    "com.apple.fps" => "fairplay",
                    _ => return false,
                };
                path == &format!("{base}/license/{suffix}")
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn media_bridge_accepts_only_bounded_broker_capabilities() {
        let base = format!("/v1/media/web/{}", "a".repeat(43));
        assert!(resource_path(&format!("{base}/index.m3u8")));
        assert!(resource_path(&format!("{base}/asset/4095")));
        for suffix in [
            "asset/4096",
            "asset/../0",
            "asset/0?url=evil",
            "license/unknown",
            "../../me",
        ] {
            assert!(!resource_path(&format!("{base}/{suffix}")));
        }
        assert!(!resource_path("https://example.org/index.m3u8"));
    }
    #[test]
    fn a_license_cannot_belong_to_a_different_playback_ticket() {
        let base = format!("/v1/media/web/{}", "a".repeat(43));
        let mut source = WebPlaybackSource {
            playlist_path: format!("{base}/index.m3u8"),
            license_paths: BTreeMap::from([(
                "com.widevine.alpha".into(),
                format!("{base}/license/widevine"),
            )]),
            duration_ms: 127000,
            bitrate_kbps: 160,
        };
        assert!(source.valid());
        source.license_paths.insert(
            "com.widevine.alpha".into(),
            format!("/v1/media/web/{}/license/widevine", "b".repeat(43)),
        );
        assert!(!source.valid());
    }
}
