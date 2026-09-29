//! Manual "Check for updates" — fetch a small `version.json`, compare semver,
//! and report the result. Notify-only: never downloads or installs anything.

use std::time::Duration;

use serde::Deserialize;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq)]
pub enum UpdateCheckOutcome {
    /// No `update_manifest_url` is configured in settings.
    NotConfigured,
    UpToDate {
        current: String,
    },
    Available {
        current: String,
        latest: String,
        notes: Option<String>,
        download_url: Option<String>,
    },
    Failed(String),
}

#[derive(Debug, Deserialize, Default)]
struct RawManifest {
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    download_url: Option<String>,
}

/// Fetch `manifest_url` and compare its `version` against `current_version`.
/// A malformed or unreachable manifest fails closed (`Failed`), never
/// falsely reports an update.
pub fn check(manifest_url: Option<&str>, current_version: &str) -> UpdateCheckOutcome {
    let Some(url) = manifest_url else {
        return UpdateCheckOutcome::NotConfigured;
    };

    let agent = build_agent();
    let response = match agent.get(url).call() {
        Ok(r) => r,
        Err(e) => return UpdateCheckOutcome::Failed(e.to_string()),
    };

    let status = response.status().as_u16();
    if status != 200 {
        return UpdateCheckOutcome::Failed(format!("HTTP {status}"));
    }

    let mut response = response;
    let body = match response.body_mut().read_to_string() {
        Ok(b) => b,
        Err(e) => return UpdateCheckOutcome::Failed(e.to_string()),
    };

    let manifest = match parse_manifest(&body) {
        Ok(m) => m,
        Err(e) => return UpdateCheckOutcome::Failed(format!("bad manifest: {e}")),
    };

    let Some(latest) = manifest.version else {
        return UpdateCheckOutcome::Failed("manifest has no version field".to_string());
    };

    if is_newer(&latest, current_version) {
        UpdateCheckOutcome::Available {
            current: current_version.to_string(),
            latest,
            notes: manifest.notes,
            download_url: manifest.download_url,
        }
    } else {
        UpdateCheckOutcome::UpToDate {
            current: current_version.to_string(),
        }
    }
}

fn build_agent() -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(REQUEST_TIMEOUT))
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::NativeTls)
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .build();
    config.into()
}

/// Tolerates a leading UTF-8 BOM, which Windows PowerShell 5.1 writes for
/// `-Encoding utf8` and serde_json otherwise rejects.
fn parse_manifest(body: &str) -> serde_json::Result<RawManifest> {
    serde_json::from_str(body.trim_start_matches('\u{feff}'))
}

/// Parse the leading `X.Y.Z` from a version string, ignoring any
/// pre-release/build suffix (`0.2.0-beta` -> `(0, 2, 0)`).
fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let mut parts = s.trim().split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch_raw = parts.next()?;
    let patch_digits: String = patch_raw.chars().take_while(|c| c.is_ascii_digit()).collect();
    let patch = patch_digits.parse().ok()?;
    Some((major, minor, patch))
}

/// True only when `remote` parses and is strictly greater than `current`.
/// Any parse failure fails closed (`false`) — a broken manifest must never
/// nag the user with a phantom update.
fn is_newer(remote: &str, current: &str) -> bool {
    match (parse_version(remote), parse_version(current)) {
        (Some(r), Some(c)) => r > c,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_configured_when_url_is_none() {
        assert_eq!(check(None, "0.1.0"), UpdateCheckOutcome::NotConfigured);
    }

    #[test]
    fn parses_plain_semver() {
        assert_eq!(parse_version("1.2.3"), Some((1, 2, 3)));
    }

    #[test]
    fn parses_semver_with_prerelease_suffix() {
        assert_eq!(parse_version("1.2.3-beta.1"), Some((1, 2, 3)));
    }

    #[test]
    fn rejects_malformed_version() {
        assert_eq!(parse_version("not-a-version"), None);
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn newer_major_minor_patch_all_detected() {
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("0.1.2", "0.1.1"));
    }

    #[test]
    fn equal_or_older_is_not_newer() {
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
    }

    #[test]
    fn malformed_remote_version_fails_closed() {
        // A broken manifest must never claim an update is available.
        assert!(!is_newer("garbage", "0.1.0"));
    }

    #[test]
    fn manifest_with_utf8_bom_parses() {
        let m = parse_manifest("\u{feff}{\"version\": \"0.2.0\"}").unwrap();
        assert_eq!(m.version.as_deref(), Some("0.2.0"));
    }
}
