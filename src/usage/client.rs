//! HTTPS client for the undocumented `oauth/usage` endpoint.

use std::path::Path;
use std::time::Duration;

use chrono::Utc;

use crate::creds::AuthProblem;
use crate::usage::model::{RawUsage, UsageSnapshot};

pub const ENDPOINT: &str = "https://api.anthropic.com/api/oauth/usage";
const OAUTH_BETA: &str = "oauth-2025-04-20";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// Used only if the installed Claude Code version cannot be determined.
/// The `claude-code/` *prefix* is what matters — without it the endpoint drops
/// you into an aggressively throttled bucket that 429s almost immediately.
const FALLBACK_CC_VERSION: &str = "2.1.220";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// Server rejected the token: treated as needing a real re-login.
    Auth(AuthProblem),
    Throttled { retry_after: Option<Duration> },
    Network(String),
    /// Response arrived but did not contain any recognizable usage windows.
    /// Carries a truncated body so a shape change can be diagnosed from the log.
    Parse { detail: String, body: String },
}

/// Read the installed Claude Code version so the User-Agent tracks whatever the
/// user actually has, rather than a constant that silently ages.
pub fn detect_claude_code_version(claude_dir: &Path) -> String {
    let path = claude_dir.join(".last-update-result.json");
    let parsed = std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|v| {
            v.get("version_to")
                .and_then(|s| s.as_str())
                .map(str::to_owned)
        })
        .filter(|v| !v.is_empty());
    parsed.unwrap_or_else(|| FALLBACK_CC_VERSION.to_string())
}

pub struct UsageClient {
    agent: ureq::Agent,
    endpoint: String,
    user_agent: String,
}

impl UsageClient {
    pub fn new(claude_code_version: &str) -> Self {
        // http_status_as_error(false) so a 429 comes back as a normal response
        // and we can read its Retry-After header instead of losing it in an Err.
        //
        // The TLS provider must be named explicitly: ureq still defaults to
        // Rustls even when only the native-tls feature is compiled in, and
        // panics at first use. NativeTls resolves to schannel on Windows, which
        // is the point — it avoids building `ring` under mingw.
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(REQUEST_TIMEOUT))
            .tls_config(
                ureq::tls::TlsConfig::builder()
                    .provider(ureq::tls::TlsProvider::NativeTls)
                    // Trust the Windows certificate store rather than ureq's
                    // bundled webpki roots. Required for schannel to validate at
                    // all, and it is also what lets the widget work behind a
                    // corporate TLS-inspecting proxy, whose root CA is installed
                    // in the OS store but absent from any bundled root set.
                    .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                    .build(),
            )
            .build();
        Self {
            agent: config.into(),
            endpoint: ENDPOINT.to_string(),
            user_agent: format!("claude-code/{claude_code_version}"),
        }
    }

    /// Point at a local test server. Used by the integration tests.
    #[allow(dead_code)]
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = endpoint.into();
        self
    }

    pub fn fetch(&self, token: &str) -> Result<UsageSnapshot, FetchError> {
        let mut response = self
            .agent
            .get(&self.endpoint)
            .header("Authorization", format!("Bearer {token}"))
            .header("anthropic-beta", OAUTH_BETA)
            .header("User-Agent", &self.user_agent)
            .header("Content-Type", "application/json")
            .call()
            .map_err(|e| FetchError::Network(e.to_string()))?;

        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<u64>().ok())
            .map(Duration::from_secs);

        let body = response
            .body_mut()
            .read_to_string()
            .map_err(|e| FetchError::Network(e.to_string()))?;

        classify(status, retry_after, &body)
    }
}

/// Response handling, split out so every status branch is unit-testable
/// without standing up a server.
pub fn classify(
    status: u16,
    retry_after: Option<Duration>,
    body: &str,
) -> Result<UsageSnapshot, FetchError> {
    match status {
        200 => {
            let raw: RawUsage = serde_json::from_str(body).map_err(|e| FetchError::Parse {
                detail: e.to_string(),
                body: truncate(body),
            })?;
            raw.into_snapshot(Utc::now()).ok_or_else(|| FetchError::Parse {
                detail: "no recognizable usage windows in response".into(),
                body: truncate(body),
            })
        }
        // The token was accepted at parse time but the server rejects it, so it
        // was revoked or scoped out. Only a real re-login fixes that.
        401 | 403 => Err(FetchError::Auth(AuthProblem::ReloginRequired)),
        429 => Err(FetchError::Throttled { retry_after }),
        s => Err(FetchError::Network(format!("HTTP {s}"))),
    }
}

fn truncate(body: &str) -> String {
    const LIMIT: usize = 600;
    if body.len() <= LIMIT {
        return body.to_string();
    }
    let mut end = LIMIT;
    while end > 0 && !body.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &body[..end])
}

/// Exponential backoff for repeated throttling. Pure, so it is tested directly.
#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    pub base: Duration,
    pub max: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            base: Duration::from_secs(180),
            max: Duration::from_secs(3600),
        }
    }
}

impl RetryPolicy {
    /// How long to wait before the next attempt.
    ///
    /// A server-supplied `Retry-After` always wins, but is floored at `base`:
    /// the endpoint sometimes suggests a delay shorter than our own poll floor,
    /// and honoring that would just earn another 429.
    pub fn delay(&self, consecutive_failures: u32, retry_after: Option<Duration>) -> Duration {
        if let Some(hinted) = retry_after {
            return hinted.max(self.base).min(self.max);
        }
        if consecutive_failures <= 1 {
            return self.base;
        }
        let shift = (consecutive_failures - 1).min(16);
        let scaled = self
            .base
            .checked_mul(1u32 << shift)
            .unwrap_or(self.max);
        scaled.min(self.max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::model::WindowKind;

    const TEAM: &str = include_str!("../../tests/fixtures/usage_team.json");

    #[test]
    fn ok_response_parses_into_snapshot() {
        let snap = classify(200, None, TEAM).unwrap();
        assert_eq!(snap.window(&WindowKind::Session).unwrap().percent, 15.0);
    }

    #[test]
    fn unauthorized_maps_to_relogin_required() {
        // 401 means the token was revoked server-side even though expiresAt
        // still looked fine locally, so "open Claude Code" would be wrong advice.
        assert_eq!(
            classify(401, None, ""),
            Err(FetchError::Auth(AuthProblem::ReloginRequired))
        );
        assert_eq!(
            classify(403, None, ""),
            Err(FetchError::Auth(AuthProblem::ReloginRequired))
        );
    }

    #[test]
    fn too_many_requests_carries_retry_after() {
        assert_eq!(
            classify(429, Some(Duration::from_secs(42)), ""),
            Err(FetchError::Throttled {
                retry_after: Some(Duration::from_secs(42))
            })
        );
        assert_eq!(
            classify(429, None, ""),
            Err(FetchError::Throttled { retry_after: None })
        );
    }

    #[test]
    fn server_error_is_network_not_auth() {
        assert!(matches!(classify(500, None, ""), Err(FetchError::Network(_))));
    }

    #[test]
    fn unparseable_body_reports_the_body_for_diagnosis() {
        let err = classify(200, None, "<html>nope</html>").unwrap_err();
        match err {
            FetchError::Parse { body, .. } => assert!(body.contains("nope")),
            other => panic!("expected Parse, got {other:?}"),
        }
    }

    #[test]
    fn empty_json_object_is_a_parse_error_with_body() {
        // Valid JSON, but a shape we do not recognize: must not silently show 0%.
        match classify(200, None, "{}").unwrap_err() {
            FetchError::Parse { detail, .. } => assert!(detail.contains("recognizable")),
            other => panic!("expected Parse, got {other:?}"),
        }
    }

    #[test]
    fn oversized_body_is_truncated_on_char_boundary() {
        let body = "é".repeat(2000);
        match classify(200, None, &body).unwrap_err() {
            FetchError::Parse { body, .. } => assert!(body.len() <= 610),
            other => panic!("expected Parse, got {other:?}"),
        }
    }

    #[test]
    fn backoff_grows_then_caps() {
        let p = RetryPolicy::default();
        assert_eq!(p.delay(0, None), Duration::from_secs(180));
        assert_eq!(p.delay(1, None), Duration::from_secs(180));
        assert_eq!(p.delay(2, None), Duration::from_secs(360));
        assert_eq!(p.delay(3, None), Duration::from_secs(720));
        assert_eq!(p.delay(99, None), Duration::from_secs(3600));
    }

    #[test]
    fn retry_after_is_honored_but_floored_at_the_poll_floor() {
        let p = RetryPolicy::default();
        // Honoring a 5s hint would immediately earn another 429.
        assert_eq!(
            p.delay(1, Some(Duration::from_secs(5))),
            Duration::from_secs(180)
        );
        assert_eq!(
            p.delay(1, Some(Duration::from_secs(600))),
            Duration::from_secs(600)
        );
        assert_eq!(
            p.delay(1, Some(Duration::from_secs(99_999))),
            Duration::from_secs(3600)
        );
    }

    #[test]
    fn version_detection_falls_back_when_file_is_absent() {
        let dir = std::env::temp_dir().join("cuw-no-such-dir-xyz");
        assert_eq!(detect_claude_code_version(&dir), FALLBACK_CC_VERSION);
    }
}
