//! Read-only access to Claude Code's OAuth credential file.
//!
//! This module deliberately never writes. Refreshing the token ourselves would
//! rotate the refresh token and race Claude Code's own writes to the file;
//! losing that race logs the user out. Claude Code refreshes the file whenever
//! it runs, which is the only time usage can change anyway.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use zeroize::Zeroizing;

/// A reason the widget cannot fetch usage, each with distinct remediation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthProblem {
    /// No credential file, or no `claudeAiOauth` key (API-key / Bedrock users).
    NotSignedIn,
    /// Access token expired but the refresh token is still good: self-heals.
    AccessTokenExpired,
    /// Refresh token expired, or the server rejected the token (401/403).
    ReloginRequired,
}

impl AuthProblem {
    /// Short form for the taskbar overlay, where horizontal space is scarce.
    pub fn short(&self) -> &'static str {
        match self {
            AuthProblem::NotSignedIn => "Not signed in to claude.ai",
            AuthProblem::AccessTokenExpired => "Claude sign-in expired",
            AuthProblem::ReloginRequired => "Claude sign-in expired",
        }
    }

    /// Full one-line message for tooltips and toasts.
    pub fn message(&self) -> &'static str {
        match self {
            AuthProblem::NotSignedIn => "Not signed in to claude.ai — run `claude` and complete login",
            AuthProblem::AccessTokenExpired => {
                "Claude sign-in expired — open Claude Code to refresh it"
            }
            AuthProblem::ReloginRequired => {
                "Claude sign-in expired — run `claude /login` to sign in again"
            }
        }
    }

    /// The exact command the user should run, for the detail popup.
    pub fn remediation(&self) -> &'static str {
        match self {
            AuthProblem::NotSignedIn => "claude",
            AuthProblem::AccessTokenExpired => "claude",
            AuthProblem::ReloginRequired => "claude /login",
        }
    }
}

impl fmt::Display for AuthProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

/// Usable credentials. The token is zeroized on drop and redacted from `Debug`
/// so it can never reach the log file through a stray `{:?}`.
pub struct Credentials {
    token: Zeroizing<String>,
    pub subscription_type: Option<String>,
    pub rate_limit_tier: Option<String>,
    pub expires_at_ms: i64,
}

impl Credentials {
    pub fn token(&self) -> &str {
        &self.token
    }
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("token", &"<redacted>")
            .field("subscription_type", &self.subscription_type)
            .field("rate_limit_tier", &self.rate_limit_tier)
            .field("expires_at_ms", &self.expires_at_ms)
            .finish()
    }
}

#[derive(Deserialize)]
struct CredentialFile {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<OauthBlock>,
}

#[derive(Deserialize)]
struct OauthBlock {
    #[serde(rename = "accessToken")]
    access_token: Option<String>,
    #[serde(rename = "expiresAt")]
    expires_at: Option<i64>,
    #[serde(rename = "refreshTokenExpiresAt")]
    refresh_token_expires_at: Option<i64>,
    #[serde(rename = "subscriptionType")]
    subscription_type: Option<String>,
    #[serde(rename = "rateLimitTier")]
    rate_limit_tier: Option<String>,
}

/// `%USERPROFILE%\.claude\.credentials.json`
pub fn default_path() -> PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".claude").join(".credentials.json")
}

/// Load credentials, classifying every failure into an actionable `AuthProblem`.
///
/// `now_ms` is injected so the expiry branches are unit-testable.
pub fn load(path: &Path, now_ms: i64) -> Result<Credentials, AuthProblem> {
    let raw = std::fs::read_to_string(path).map_err(|_| AuthProblem::NotSignedIn)?;
    parse(&raw, now_ms)
}

/// Split out from [`load`] so tests can exercise it without touching the disk.
pub fn parse(raw: &str, now_ms: i64) -> Result<Credentials, AuthProblem> {
    let file: CredentialFile = serde_json::from_str(raw).map_err(|_| AuthProblem::NotSignedIn)?;
    let oauth = file.claude_ai_oauth.ok_or(AuthProblem::NotSignedIn)?;

    let token = oauth
        .access_token
        .filter(|t| !t.is_empty())
        .ok_or(AuthProblem::NotSignedIn)?;

    // Checked before the access token: a dead refresh token needs `claude /login`,
    // whereas a merely expired access token fixes itself when Claude Code runs.
    if let Some(refresh_expiry) = oauth.refresh_token_expires_at {
        if now_ms >= refresh_expiry {
            return Err(AuthProblem::ReloginRequired);
        }
    }

    let expires_at_ms = oauth.expires_at.unwrap_or(0);
    if now_ms >= expires_at_ms {
        return Err(AuthProblem::AccessTokenExpired);
    }

    Ok(Credentials {
        token: Zeroizing::new(token),
        subscription_type: oauth.subscription_type,
        rate_limit_tier: oauth.rate_limit_tier,
        expires_at_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_000_000_000_000;

    fn json(access: &str, expires: i64, refresh_expires: Option<i64>) -> String {
        let refresh = match refresh_expires {
            Some(v) => format!(r#","refreshTokenExpiresAt":{v}"#),
            None => String::new(),
        };
        format!(
            r#"{{"claudeAiOauth":{{"accessToken":"{access}","expiresAt":{expires},"subscriptionType":"team","rateLimitTier":"default_raven"{refresh}}}}}"#
        )
    }

    #[test]
    fn valid_credentials_parse() {
        let c = parse(&json("tok", NOW + 60_000, Some(NOW + 999_000)), NOW).unwrap();
        assert_eq!(c.token(), "tok");
        assert_eq!(c.subscription_type.as_deref(), Some("team"));
        assert_eq!(c.rate_limit_tier.as_deref(), Some("default_raven"));
    }

    #[test]
    fn missing_oauth_block_is_not_signed_in() {
        // API-key and Bedrock users have a file without claudeAiOauth.
        assert_eq!(
            parse(r#"{"other":1}"#, NOW).unwrap_err(),
            AuthProblem::NotSignedIn
        );
    }

    #[test]
    fn malformed_json_is_not_signed_in() {
        assert_eq!(parse("not json", NOW).unwrap_err(), AuthProblem::NotSignedIn);
    }

    #[test]
    fn empty_token_is_not_signed_in() {
        assert_eq!(
            parse(&json("", NOW + 60_000, None), NOW).unwrap_err(),
            AuthProblem::NotSignedIn
        );
    }

    #[test]
    fn expired_access_token_is_self_healing() {
        assert_eq!(
            parse(&json("tok", NOW - 1, Some(NOW + 999_000)), NOW).unwrap_err(),
            AuthProblem::AccessTokenExpired
        );
    }

    #[test]
    fn expired_refresh_token_outranks_expired_access_token() {
        // Both expired: the user must actually re-login, so say that and not
        // the misleading "open Claude Code" message.
        assert_eq!(
            parse(&json("tok", NOW - 1, Some(NOW - 1)), NOW).unwrap_err(),
            AuthProblem::ReloginRequired
        );
    }

    #[test]
    fn missing_refresh_expiry_falls_back_to_access_expiry() {
        assert_eq!(
            parse(&json("tok", NOW - 1, None), NOW).unwrap_err(),
            AuthProblem::AccessTokenExpired
        );
    }

    #[test]
    fn token_is_redacted_in_debug_output() {
        let c = parse(&json("supersecret", NOW + 60_000, None), NOW).unwrap();
        let rendered = format!("{c:?}");
        assert!(!rendered.contains("supersecret"), "token leaked: {rendered}");
        assert!(rendered.contains("<redacted>"));
    }

    #[test]
    fn each_problem_has_distinct_remediation() {
        assert_eq!(AuthProblem::AccessTokenExpired.remediation(), "claude");
        assert_eq!(AuthProblem::ReloginRequired.remediation(), "claude /login");
        assert_ne!(
            AuthProblem::AccessTokenExpired.message(),
            AuthProblem::ReloginRequired.message()
        );
    }
}
