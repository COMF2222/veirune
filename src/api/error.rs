#![allow(dead_code)]

use thiserror::Error;

pub type Result<T> = std::result::Result<T, ApiError>;

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("rate limited (HTTP 429), retry after {retry_after_ms:?}")]
    RateLimited { retry_after_ms: Option<u64> },

    #[error("Audio link expired; start the track again")]
    PlaybackLinkExpired,

    #[error("unauthorized: token expired or revoked (HTTP 401)")]
    Unauthorized,

    #[error("HTTP {status}: {body}")]
    Http { status: u16, body: String },

    #[error("bad request: {0}")]
    Bad(String),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl ApiError {
    /// Classify error payloads from SoundCloud and older broker versions alike.
    pub fn from_http(status: u16, body: String, retry_after_ms: Option<u64>) -> Self {
        let text = body.to_ascii_lowercase();
        if status == 403 && text.contains("disabled by the owner") {
            return Self::Other(anyhow::anyhow!("Access to Veirune was disabled by the owner."));
        }
        if status == 429 || matches!(status, 403 | 502) &&
            ["rate limit", "rate_limit", "ratelimit", "http 429"].iter().any(|value| text.contains(value)) {
            return Self::RateLimited { retry_after_ms };
        }
        if status == 401 { return Self::Unauthorized; }
        Self::Http { status, body }
    }

    pub fn user_message(&self) -> String {
        match self {
            Self::PlaybackLinkExpired => "The audio connection was interrupted. Retry from the current position.".into(),
            Self::Unauthorized => {
                "Your SoundCloud session expired. Sign in again in Settings → Account.".into()
            }
            Self::Http { status: 404, .. } => {
                "This item is no longer available on SoundCloud.".into()
            }
            Self::Http { status: 403, .. } => {
                "SoundCloud does not allow access to this item with this account.".into()
            }
            Self::Http {
                status: 500..=599, ..
            } => "SoundCloud is temporarily unavailable. Please try again shortly.".into(),
            Self::Network(_) => {
                "Could not reach SoundCloud. Check your connection and try again.".into()
            }
            Self::RateLimited { .. } => {
                "SoundCloud's request limit was reached. Waiting before retrying.".into()
            }
            Self::Json(_) => {
                "SoundCloud returned data that could not be read. Please retry.".into()
            }
            _ => self.to_string(),
        }
    }

    pub fn status(&self) -> Option<u16> {
        match self {
            ApiError::Http { status, .. } => Some(*status),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_broker_and_forbidden_quota_errors_are_request_limits() {
        for (status, body) in [(502, r#"{"error":"SoundCloud profile lookup returned HTTP 429: Rate limit exceeded"}"#),
            (403, r#"{"errors":[{"error_message":"Rate limit exceeded"}]}"#)] {
            assert!(matches!(ApiError::from_http(status, body.into(), Some(12000)),
                ApiError::RateLimited { retry_after_ms: Some(12000) }));
        }
        assert!(matches!(ApiError::from_http(403, r#"{"error":"Track is private"}"#.into(), None),
            ApiError::Http {status:403,..}));
    }
}
