//! Notification bookkeeping: what to toast, and — more importantly — what not to.
//!
//! Transient conditions (staleness, throttling, network blips) deliberately
//! raise no toast. Training people to dismiss notifications is how the real
//! warnings get ignored.
//!
//! Written and fully tested during M1/M2 groundwork but not yet wired into
//! `tray.rs` — that wiring (actually firing toasts on `Alert`) is M5's job.
#![allow(dead_code)]

use std::collections::HashMap;

use chrono::{DateTime, Duration as ChronoDuration, Utc};

use crate::creds::AuthProblem;
use crate::usage::model::UsageSnapshot;

/// How long before an unresolved auth problem warns again.
const AUTH_REARM_HOURS: i64 = 4;

#[derive(Debug, Clone, PartialEq)]
pub enum Alert {
    Threshold {
        label: String,
        percent: f64,
        threshold: u8,
    },
    Auth(AuthProblem),
}

#[derive(Debug, Clone)]
struct AuthNotice {
    problem: AuthProblem,
    last_notified: DateTime<Utc>,
}

#[derive(Debug, Default)]
pub struct AlertState {
    /// Threshold-crossing key -> that window's reset time, used for pruning.
    fired: HashMap<String, DateTime<Utc>>,
    auth: Option<AuthNotice>,
}

impl AlertState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn current_auth(&self) -> Option<&AuthProblem> {
        self.auth.as_ref().map(|a| &a.problem)
    }

    /// Handle a successful poll. Clears any standing auth warning and returns
    /// alerts for thresholds crossed since the last poll.
    pub fn on_snapshot(
        &mut self,
        snapshot: &UsageSnapshot,
        thresholds: &[u8],
        now: DateTime<Utc>,
    ) -> Vec<Alert> {
        self.auth = None;
        self.prune(now);

        let mut alerts = Vec::new();
        for window in &snapshot.windows {
            // A window whose reset already passed reads as 0% locally, so it
            // cannot have crossed anything.
            if window.resets_at.map(|r| now >= r).unwrap_or(false) {
                continue;
            }
            for &threshold in thresholds {
                if window.percent < f64::from(threshold) {
                    continue;
                }
                // Keyed on the reset time so the next window re-arms naturally.
                let key = format!(
                    "{:?}|{}|{}",
                    window.kind,
                    window
                        .resets_at
                        .map(|r| r.timestamp())
                        .unwrap_or_default(),
                    threshold
                );
                if self.fired.contains_key(&key) {
                    continue;
                }
                self.fired.insert(
                    key,
                    window.resets_at.unwrap_or(now + ChronoDuration::days(7)),
                );
                alerts.push(Alert::Threshold {
                    label: window.kind.label(),
                    percent: window.percent,
                    threshold,
                });
            }
        }
        alerts
    }

    /// Handle a failed poll caused by credentials. Warns on entry into the
    /// state, then at most once every `AUTH_REARM_HOURS` while it persists.
    pub fn on_auth_problem(&mut self, problem: &AuthProblem, now: DateTime<Utc>) -> Vec<Alert> {
        let should_notify = match &self.auth {
            None => true,
            // A *different* problem is new information: warn again immediately.
            Some(prev) if prev.problem != *problem => true,
            Some(prev) => now.signed_duration_since(prev.last_notified)
                >= ChronoDuration::hours(AUTH_REARM_HOURS),
        };

        if should_notify {
            self.auth = Some(AuthNotice {
                problem: problem.clone(),
                last_notified: now,
            });
            vec![Alert::Auth(problem.clone())]
        } else {
            // Keep the standing problem, but do not re-notify.
            if let Some(existing) = &mut self.auth {
                existing.problem = problem.clone();
            }
            Vec::new()
        }
    }

    /// Drop bookkeeping for windows that have already rolled over.
    fn prune(&mut self, now: DateTime<Utc>) {
        self.fired.retain(|_, resets_at| *resets_at > now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::model::{RawUsage, UsageSnapshot};

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    /// Snapshot with one session window at `percent`, resetting at `resets_at`.
    fn snap(percent: f64, resets_at: &str) -> UsageSnapshot {
        let json = format!(
            r#"{{"limits":[{{"kind":"session","percent":{percent},"resets_at":"{resets_at}","is_active":true}}]}}"#
        );
        serde_json::from_str::<RawUsage>(&json)
            .unwrap()
            .into_snapshot(at("2026-07-29T08:00:00Z"))
            .unwrap()
    }

    const T: [u8; 2] = [80, 95];

    #[test]
    fn crossing_a_threshold_fires_once() {
        let mut state = AlertState::new();
        let s = snap(85.0, "2026-07-29T12:00:00Z");
        let now = at("2026-07-29T08:00:00Z");

        let first = state.on_snapshot(&s, &T, now);
        assert_eq!(
            first,
            vec![Alert::Threshold {
                label: "5h".into(),
                percent: 85.0,
                threshold: 80
            }]
        );

        // Same window, still above: silent.
        assert!(state.on_snapshot(&s, &T, now).is_empty());
    }

    #[test]
    fn each_threshold_fires_independently() {
        let mut state = AlertState::new();
        let now = at("2026-07-29T08:00:00Z");

        assert_eq!(
            state.on_snapshot(&snap(85.0, "2026-07-29T12:00:00Z"), &T, now).len(),
            1
        );
        // Climbing past 95 is new information even though 80 already fired.
        let second = state.on_snapshot(&snap(96.0, "2026-07-29T12:00:00Z"), &T, now);
        assert_eq!(
            second,
            vec![Alert::Threshold {
                label: "5h".into(),
                percent: 96.0,
                threshold: 95
            }]
        );
    }

    #[test]
    fn below_threshold_is_silent() {
        let mut state = AlertState::new();
        let now = at("2026-07-29T08:00:00Z");
        assert!(state
            .on_snapshot(&snap(79.9, "2026-07-29T12:00:00Z"), &T, now)
            .is_empty());
    }

    #[test]
    fn a_new_window_rearms_the_same_threshold() {
        let mut state = AlertState::new();
        let now = at("2026-07-29T08:00:00Z");
        assert_eq!(
            state.on_snapshot(&snap(85.0, "2026-07-29T12:00:00Z"), &T, now).len(),
            1
        );
        // Different resets_at means a different window instance.
        let later = at("2026-07-29T13:00:00Z");
        assert_eq!(
            state.on_snapshot(&snap(85.0, "2026-07-29T17:00:00Z"), &T, later).len(),
            1
        );
    }

    #[test]
    fn expired_window_cannot_cross_since_it_reads_as_zero() {
        let mut state = AlertState::new();
        // Reset already passed, so project() shows 0%; alerting 85% would lie.
        let now = at("2026-07-29T13:00:00Z");
        assert!(state
            .on_snapshot(&snap(85.0, "2026-07-29T12:00:00Z"), &T, now)
            .is_empty());
    }

    #[test]
    fn stale_threshold_keys_are_pruned() {
        let mut state = AlertState::new();
        state.on_snapshot(&snap(85.0, "2026-07-29T12:00:00Z"), &T, at("2026-07-29T08:00:00Z"));
        assert_eq!(state.fired.len(), 1);
        // Long past that window's reset: bookkeeping must not accumulate forever.
        state.on_snapshot(&snap(10.0, "2026-08-05T12:00:00Z"), &T, at("2026-08-01T00:00:00Z"));
        assert_eq!(state.fired.len(), 0);
    }

    #[test]
    fn auth_problem_warns_on_entry_then_stays_quiet() {
        let mut state = AlertState::new();
        let now = at("2026-07-29T08:00:00Z");

        assert_eq!(
            state.on_auth_problem(&AuthProblem::AccessTokenExpired, now),
            vec![Alert::Auth(AuthProblem::AccessTokenExpired)]
        );
        // Every subsequent poll must not re-toast.
        assert!(state
            .on_auth_problem(&AuthProblem::AccessTokenExpired, at("2026-07-29T08:03:00Z"))
            .is_empty());
        assert!(state
            .on_auth_problem(&AuthProblem::AccessTokenExpired, at("2026-07-29T11:59:00Z"))
            .is_empty());
    }

    #[test]
    fn unresolved_auth_problem_rearms_after_four_hours() {
        let mut state = AlertState::new();
        state.on_auth_problem(&AuthProblem::AccessTokenExpired, at("2026-07-29T08:00:00Z"));
        assert_eq!(
            state.on_auth_problem(&AuthProblem::AccessTokenExpired, at("2026-07-29T12:00:00Z")),
            vec![Alert::Auth(AuthProblem::AccessTokenExpired)]
        );
    }

    #[test]
    fn escalating_to_a_different_problem_warns_immediately() {
        let mut state = AlertState::new();
        let now = at("2026-07-29T08:00:00Z");
        state.on_auth_problem(&AuthProblem::AccessTokenExpired, now);
        // "open Claude Code" just became wrong advice; say so without waiting 4h.
        assert_eq!(
            state.on_auth_problem(&AuthProblem::ReloginRequired, at("2026-07-29T08:03:00Z")),
            vec![Alert::Auth(AuthProblem::ReloginRequired)]
        );
    }

    #[test]
    fn successful_poll_clears_the_auth_warning() {
        let mut state = AlertState::new();
        state.on_auth_problem(&AuthProblem::ReloginRequired, at("2026-07-29T08:00:00Z"));
        assert!(state.current_auth().is_some());

        state.on_snapshot(&snap(10.0, "2026-07-29T12:00:00Z"), &T, at("2026-07-29T08:05:00Z"));
        assert!(state.current_auth().is_none());
    }

    #[test]
    fn recovery_then_relapse_warns_again() {
        let mut state = AlertState::new();
        state.on_auth_problem(&AuthProblem::AccessTokenExpired, at("2026-07-29T08:00:00Z"));
        state.on_snapshot(&snap(10.0, "2026-07-29T12:00:00Z"), &T, at("2026-07-29T08:05:00Z"));
        assert_eq!(
            state.on_auth_problem(&AuthProblem::AccessTokenExpired, at("2026-07-29T08:10:00Z")),
            vec![Alert::Auth(AuthProblem::AccessTokenExpired)]
        );
    }
}
