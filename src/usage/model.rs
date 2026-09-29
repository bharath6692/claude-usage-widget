//! Parsing and display projection for the `oauth/usage` payload.
//!
//! The endpoint is undocumented, so every field is optional and unknown fields
//! are ignored. The normalized `limits[]` array is preferred over the typed
//! `five_hour`/`seven_day` objects because it carries server-computed
//! `severity`/`is_active` and grows to cover per-model windows
//! (`weekly_opus`, `weekly_sonnet`, `weekly_cowork`) that only some plans have.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowKind {
    Session,
    WeeklyAll,
    WeeklyOpus,
    WeeklySonnet,
    WeeklyCowork,
    Other(String),
}

impl WindowKind {
    fn from_limit_kind(kind: &str) -> Self {
        match kind {
            "session" => WindowKind::Session,
            "weekly_all" => WindowKind::WeeklyAll,
            "weekly_opus" => WindowKind::WeeklyOpus,
            "weekly_sonnet" => WindowKind::WeeklySonnet,
            "weekly_cowork" => WindowKind::WeeklyCowork,
            other => WindowKind::Other(other.to_string()),
        }
    }

    /// Compact label for the overlay, matching the reference design.
    pub fn label(&self) -> String {
        match self {
            WindowKind::Session => "5h".into(),
            WindowKind::WeeklyAll => "7d".into(),
            WindowKind::WeeklyOpus => "7d Opus".into(),
            WindowKind::WeeklySonnet => "7d Sonnet".into(),
            WindowKind::WeeklyCowork => "7d Cowork".into(),
            WindowKind::Other(k) => k.clone(),
        }
    }

    /// Sort key so the two primary rows are always first and in a stable order.
    fn order(&self) -> u8 {
        match self {
            WindowKind::Session => 0,
            WindowKind::WeeklyAll => 1,
            WindowKind::WeeklyOpus => 2,
            WindowKind::WeeklySonnet => 3,
            WindowKind::WeeklyCowork => 4,
            WindowKind::Other(_) => 5,
        }
    }

    /// The two windows shown in the taskbar overlay; the rest live in the popup.
    pub fn is_primary(&self) -> bool {
        matches!(self, WindowKind::Session | WindowKind::WeeklyAll)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageWindow {
    pub kind: WindowKind,
    pub percent: f64,
    pub resets_at: Option<DateTime<Utc>>,
    pub is_active: bool,
    pub severity: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExtraUsage {
    pub enabled: bool,
    pub utilization: Option<f64>,
    pub used_credits: Option<f64>,
    pub monthly_limit: Option<f64>,
    pub currency: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageSnapshot {
    pub windows: Vec<UsageWindow>,
    pub extra_usage: Option<ExtraUsage>,
    pub fetched_at: DateTime<Utc>,
}

impl UsageSnapshot {
    /// Look up one window by kind. The M4 detail popup's primary consumer;
    /// unused in production code until that milestone wires it in.
    #[allow(dead_code)]
    pub fn window(&self, kind: &WindowKind) -> Option<&UsageWindow> {
        self.windows.iter().find(|w| &w.kind == kind)
    }

    pub fn primary(&self) -> impl Iterator<Item = &UsageWindow> {
        self.windows.iter().filter(|w| w.kind.is_primary())
    }

    /// Highest percentage across the primary windows — what the tray icon shows.
    pub fn worst_primary_percent(&self) -> f64 {
        self.primary().map(|w| w.percent).fold(0.0, f64::max)
    }

    /// A snapshot is stale once we have missed roughly three polls.
    pub fn is_stale(&self, now: DateTime<Utc>, interval: Duration) -> bool {
        let age = now.signed_duration_since(self.fetched_at);
        match age.to_std() {
            Ok(age) => age > interval.saturating_mul(3),
            Err(_) => false, // clock skew put fetched_at in the future
        }
    }
}

/// What the UI actually draws for one row.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayRow {
    pub label: String,
    pub percent: f64,
    /// True when we inferred 0% locally because the window's reset time passed
    /// but we have not confirmed it with a fresh poll yet. Rendered dimmed.
    pub provisional: bool,
    pub remaining: Option<Duration>,
}

/// Project a snapshot into drawable rows at time `now`.
///
/// Windows whose `resets_at` has passed are shown as 0% and flagged provisional.
/// This is what keeps the widget honest while the machine sits idle: usage only
/// moves when Claude Code runs, but windows still reset on their own schedule.
pub fn project(snapshot: &UsageSnapshot, now: DateTime<Utc>, primary_only: bool) -> Vec<DisplayRow> {
    snapshot
        .windows
        .iter()
        .filter(|w| !primary_only || w.kind.is_primary())
        .map(|w| {
            let expired = w.resets_at.map(|r| now >= r).unwrap_or(false);
            let remaining = w.resets_at.and_then(|r| {
                r.signed_duration_since(now)
                    .to_std()
                    .ok()
                    .filter(|d| !d.is_zero())
            });
            DisplayRow {
                label: w.kind.label(),
                percent: if expired { 0.0 } else { w.percent },
                provisional: expired,
                remaining,
            }
        })
        .collect()
}

/// `4h` / `26m` / `2d3h` / `now`
pub fn format_remaining(remaining: Option<Duration>) -> String {
    let Some(d) = remaining else {
        return "now".into();
    };
    let total = d.as_secs();
    let days = total / 86_400;
    let hours = (total % 86_400) / 3_600;
    let mins = (total % 3_600) / 60;
    if days > 0 {
        format!("{days}d{hours}h")
    } else if hours > 0 {
        format!("{hours}h{mins:02}m")
    } else if mins > 0 {
        format!("{mins}m")
    } else {
        "now".into()
    }
}

// ---------------------------------------------------------------------------
// Wire format
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
pub struct RawUsage {
    #[serde(default)]
    limits: Vec<RawLimit>,
    #[serde(default)]
    five_hour: Option<RawWindow>,
    #[serde(default)]
    seven_day: Option<RawWindow>,
    #[serde(default)]
    seven_day_opus: Option<RawWindow>,
    #[serde(default)]
    seven_day_sonnet: Option<RawWindow>,
    #[serde(default)]
    seven_day_cowork: Option<RawWindow>,
    #[serde(default)]
    extra_usage: Option<RawExtraUsage>,
}

#[derive(Debug, Deserialize)]
struct RawLimit {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    percent: Option<f64>,
    #[serde(default)]
    severity: Option<String>,
    #[serde(default)]
    resets_at: Option<String>,
    #[serde(default)]
    is_active: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct RawWindow {
    #[serde(default)]
    utilization: Option<f64>,
    #[serde(default)]
    resets_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawExtraUsage {
    #[serde(default)]
    is_enabled: Option<bool>,
    #[serde(default)]
    utilization: Option<f64>,
    #[serde(default)]
    used_credits: Option<f64>,
    #[serde(default)]
    monthly_limit: Option<f64>,
    #[serde(default)]
    currency: Option<String>,
}

fn parse_ts(raw: &Option<String>) -> Option<DateTime<Utc>> {
    raw.as_deref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
}

impl RawUsage {
    /// Convert the wire payload into a snapshot.
    ///
    /// Returns `None` when neither `limits[]` nor any typed window is present —
    /// the caller logs the raw body so we can adapt to a shape change quickly.
    pub fn into_snapshot(self, fetched_at: DateTime<Utc>) -> Option<UsageSnapshot> {
        let mut windows: Vec<UsageWindow> = self
            .limits
            .iter()
            .filter(|l| !l.kind.is_empty())
            .map(|l| UsageWindow {
                kind: WindowKind::from_limit_kind(&l.kind),
                percent: l.percent.unwrap_or(0.0),
                resets_at: parse_ts(&l.resets_at),
                is_active: l.is_active.unwrap_or(false),
                severity: l.severity.clone(),
            })
            .collect();

        // Fall back to the typed fields for any window `limits[]` did not cover.
        let typed = [
            (WindowKind::Session, &self.five_hour),
            (WindowKind::WeeklyAll, &self.seven_day),
            (WindowKind::WeeklyOpus, &self.seven_day_opus),
            (WindowKind::WeeklySonnet, &self.seven_day_sonnet),
            (WindowKind::WeeklyCowork, &self.seven_day_cowork),
        ];
        for (kind, raw) in typed {
            let Some(raw) = raw else { continue };
            let Some(pct) = raw.utilization else { continue };
            if windows.iter().any(|w| w.kind == kind) {
                continue;
            }
            windows.push(UsageWindow {
                kind,
                percent: pct,
                resets_at: parse_ts(&raw.resets_at),
                is_active: false,
                severity: None,
            });
        }

        if windows.is_empty() {
            return None;
        }

        windows.sort_by_key(|w| w.kind.order());

        Some(UsageSnapshot {
            windows,
            extra_usage: self.extra_usage.map(|e| ExtraUsage {
                enabled: e.is_enabled.unwrap_or(false),
                utilization: e.utilization,
                used_credits: e.used_credits,
                monthly_limit: e.monthly_limit,
                currency: e.currency,
            }),
            fetched_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn snapshot_from(fixture: &str) -> UsageSnapshot {
        let raw: RawUsage = serde_json::from_str(fixture).expect("fixture parses");
        raw.into_snapshot(at("2026-07-29T08:00:00Z"))
            .expect("fixture yields windows")
    }

    #[test]
    fn parses_team_payload_from_limits_array() {
        let snap = snapshot_from(include_str!("../../tests/fixtures/usage_team.json"));
        assert_eq!(snap.windows.len(), 2);

        let five = snap.window(&WindowKind::Session).unwrap();
        assert_eq!(five.percent, 15.0);
        assert_eq!(five.resets_at, Some(at("2026-07-29T12:00:00.331570Z")));
        assert!(!five.is_active);

        let seven = snap.window(&WindowKind::WeeklyAll).unwrap();
        assert_eq!(seven.percent, 68.0);
        assert!(seven.is_active);
        assert_eq!(seven.severity.as_deref(), Some("normal"));
    }

    #[test]
    fn parses_per_model_windows_for_plans_that_have_them() {
        let snap = snapshot_from(include_str!("../../tests/fixtures/usage_max_opus.json"));
        let opus = snap.window(&WindowKind::WeeklyOpus).unwrap();
        assert_eq!(opus.percent, 91.0);
        assert_eq!(opus.severity.as_deref(), Some("critical"));
        // Primary rows stay first regardless of the order the server sent them.
        assert_eq!(snap.windows[0].kind, WindowKind::Session);
        assert_eq!(snap.windows[1].kind, WindowKind::WeeklyAll);
    }

    #[test]
    fn falls_back_to_typed_fields_when_limits_absent() {
        let snap = snapshot_from(include_str!("../../tests/fixtures/usage_missing_limits.json"));
        assert_eq!(snap.window(&WindowKind::Session).unwrap().percent, 22.0);
        assert_eq!(snap.window(&WindowKind::WeeklyAll).unwrap().percent, 55.0);
    }

    #[test]
    fn unknown_limit_kinds_survive_as_other() {
        let raw: RawUsage = serde_json::from_str(
            r#"{"limits":[{"kind":"weekly_newthing","percent":5,"resets_at":null}]}"#,
        )
        .unwrap();
        let snap = raw.into_snapshot(at("2026-07-29T08:00:00Z")).unwrap();
        assert_eq!(
            snap.windows[0].kind,
            WindowKind::Other("weekly_newthing".into())
        );
        // Forward-compatible, but not promoted into the two overlay rows.
        assert!(!snap.windows[0].kind.is_primary());
    }

    #[test]
    fn empty_payload_yields_no_snapshot() {
        let raw: RawUsage = serde_json::from_str("{}").unwrap();
        assert!(raw.into_snapshot(at("2026-07-29T08:00:00Z")).is_none());
    }

    #[test]
    fn unknown_top_level_fields_are_ignored() {
        // The endpoint is undocumented; new keys must not break parsing.
        let raw: RawUsage = serde_json::from_str(
            r#"{"brand_new_key":{"nested":true},"five_hour":{"utilization":1.0}}"#,
        )
        .unwrap();
        assert!(raw.into_snapshot(at("2026-07-29T08:00:00Z")).is_some());
    }

    #[test]
    fn past_reset_zeroes_the_row_and_marks_it_provisional() {
        let snap = snapshot_from(include_str!("../../tests/fixtures/usage_team.json"));
        // 7d resets at 09:00, 5h at 12:00. At 10:00 only the weekly has rolled.
        let rows = project(&snap, at("2026-07-29T10:00:00Z"), true);

        let five = rows.iter().find(|r| r.label == "5h").unwrap();
        assert_eq!(five.percent, 15.0);
        assert!(!five.provisional);

        let seven = rows.iter().find(|r| r.label == "7d").unwrap();
        assert_eq!(seven.percent, 0.0);
        assert!(seven.provisional);
        assert_eq!(seven.remaining, None);
    }

    #[test]
    fn project_primary_only_excludes_per_model_windows() {
        let snap = snapshot_from(include_str!("../../tests/fixtures/usage_max_opus.json"));
        assert_eq!(project(&snap, at("2026-07-29T08:00:00Z"), true).len(), 2);
        assert!(project(&snap, at("2026-07-29T08:00:00Z"), false).len() > 2);
    }

    #[test]
    fn worst_primary_percent_ignores_per_model_windows() {
        // Opus sits at 91% but is not a primary row, so the tray icon shows 68%.
        let snap = snapshot_from(include_str!("../../tests/fixtures/usage_max_opus.json"));
        assert_eq!(snap.worst_primary_percent(), 68.0);
    }

    #[test]
    fn staleness_triggers_after_three_missed_polls() {
        let snap = snapshot_from(include_str!("../../tests/fixtures/usage_team.json"));
        let interval = Duration::from_secs(180);
        assert!(!snap.is_stale(at("2026-07-29T08:08:00Z"), interval)); // 8m < 9m
        assert!(snap.is_stale(at("2026-07-29T08:10:00Z"), interval)); // 10m > 9m
    }

    #[test]
    fn clock_skew_does_not_report_stale() {
        let snap = snapshot_from(include_str!("../../tests/fixtures/usage_team.json"));
        assert!(!snap.is_stale(at("2026-07-29T07:00:00Z"), Duration::from_secs(180)));
    }

    #[test]
    fn remaining_formats_across_magnitudes() {
        assert_eq!(format_remaining(Some(Duration::from_secs(0))), "now");
        assert_eq!(format_remaining(Some(Duration::from_secs(90))), "1m");
        assert_eq!(format_remaining(Some(Duration::from_secs(3_960))), "1h06m");
        assert_eq!(format_remaining(Some(Duration::from_secs(183_600))), "2d3h");
        assert_eq!(format_remaining(None), "now");
    }
}
