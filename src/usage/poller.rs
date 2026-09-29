//! Background polling thread.
//!
//! Owns no UI. It publishes into a shared [`SharedState`] and invokes a callback
//! so the UI layer can repaint; that keeps the whole data path testable and
//! lets M1 run headless.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::creds::{self, AuthProblem};
use crate::usage::client::{FetchError, RetryPolicy, UsageClient};
use crate::usage::model::UsageSnapshot;

#[derive(Debug, Clone, PartialEq)]
pub enum PollOutcome {
    Ok(Box<UsageSnapshot>),
    Auth(AuthProblem),
    Throttled { retry_after: Option<Duration> },
    Failed(String),
}

/// Everything the UI needs to draw, guarded by one mutex.
#[derive(Debug, Default)]
pub struct SharedState {
    pub snapshot: Option<UsageSnapshot>,
    pub auth: Option<AuthProblem>,
    pub throttled: bool,
    pub last_ok: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

impl SharedState {
    /// True when there is nothing trustworthy to show. Drives the M3
    /// overlay's "replace the bars with a warning line" decision; unused by
    /// the tray icon itself, which distinguishes auth/stale more finely.
    #[allow(dead_code)]
    pub fn has_problem(&self) -> bool {
        self.auth.is_some() || self.snapshot.is_none()
    }
}

/// One poll attempt: read credentials, then fetch. Credential problems
/// short-circuit before any network call.
pub fn poll_once(client: &UsageClient, creds_path: &Path, now_ms: i64) -> PollOutcome {
    let credentials = match creds::load(creds_path, now_ms) {
        Ok(c) => c,
        Err(problem) => return PollOutcome::Auth(problem),
    };

    match client.fetch(credentials.token()) {
        Ok(snapshot) => PollOutcome::Ok(Box::new(snapshot)),
        Err(FetchError::Auth(problem)) => PollOutcome::Auth(problem),
        Err(FetchError::Throttled { retry_after }) => PollOutcome::Throttled { retry_after },
        Err(FetchError::Network(detail)) => PollOutcome::Failed(detail),
        Err(FetchError::Parse { detail, body }) => {
            // The endpoint is undocumented: log the body so a shape change can be
            // diagnosed from a teammate's log rather than guessed at.
            log::error!("usage response did not parse ({detail}); body: {body}");
            PollOutcome::Failed(detail)
        }
    }
}

/// Fold an outcome into the shared state. Pure w.r.t. the mutex so it is tested
/// directly.
pub fn apply(state: &mut SharedState, outcome: &PollOutcome, now: DateTime<Utc>) {
    match outcome {
        PollOutcome::Ok(snapshot) => {
            state.snapshot = Some((**snapshot).clone());
            state.auth = None;
            state.throttled = false;
            state.last_ok = Some(now);
            state.last_error = None;
        }
        PollOutcome::Auth(problem) => {
            state.auth = Some(problem.clone());
            state.throttled = false;
            state.last_error = Some(problem.message().to_string());
        }
        PollOutcome::Throttled { .. } => {
            // Throttling says nothing about the numbers we already have, so the
            // last good snapshot stays on screen.
            state.throttled = true;
            state.last_error = Some("rate limited by Anthropic".into());
        }
        PollOutcome::Failed(detail) => {
            state.throttled = false;
            state.last_error = Some(detail.clone());
        }
    }
}

pub enum Command {
    RefreshNow,
    SetInterval(Duration),
    Shutdown,
}

pub struct PollerHandle {
    tx: Sender<Command>,
    thread: Option<JoinHandle<()>>,
}

impl PollerHandle {
    pub fn refresh_now(&self) {
        let _ = self.tx.send(Command::RefreshNow);
    }

    pub fn set_interval(&self, interval: Duration) {
        let _ = self.tx.send(Command::SetInterval(interval));
    }

    pub fn shutdown(mut self) {
        let _ = self.tx.send(Command::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub struct PollerConfig {
    pub client: UsageClient,
    pub credentials_path: PathBuf,
    pub interval: Duration,
    pub retry: RetryPolicy,
}

/// Start polling. `on_update` runs on the poller thread after each attempt and
/// should do nothing but signal the UI thread.
pub fn spawn<F>(
    config: PollerConfig,
    state: Arc<Mutex<SharedState>>,
    on_update: F,
) -> PollerHandle
where
    F: Fn(&PollOutcome) + Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("usage-poller".into())
        .spawn(move || {
            let PollerConfig {
                client,
                credentials_path,
                mut interval,
                retry,
            } = config;
            let mut consecutive_throttles = 0u32;

            loop {
                let now_ms = Utc::now().timestamp_millis();
                let outcome = poll_once(&client, &credentials_path, now_ms);

                let wait = match &outcome {
                    PollOutcome::Throttled { retry_after } => {
                        consecutive_throttles = consecutive_throttles.saturating_add(1);
                        let delay = retry.delay(consecutive_throttles, *retry_after);
                        log::warn!(
                            "throttled ({consecutive_throttles}x), next attempt in {}s",
                            delay.as_secs()
                        );
                        delay
                    }
                    _ => {
                        consecutive_throttles = 0;
                        interval
                    }
                };

                if let Ok(mut guard) = state.lock() {
                    apply(&mut guard, &outcome, Utc::now());
                }
                on_update(&outcome);

                match rx.recv_timeout(wait) {
                    Ok(Command::RefreshNow) | Err(RecvTimeoutError::Timeout) => {}
                    Ok(Command::SetInterval(next)) => interval = next,
                    Ok(Command::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        })
        .expect("spawn poller thread");

    PollerHandle {
        tx,
        thread: Some(thread),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::model::RawUsage;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn sample_snapshot() -> UsageSnapshot {
        serde_json::from_str::<RawUsage>(include_str!("../../tests/fixtures/usage_team.json"))
            .unwrap()
            .into_snapshot(at("2026-07-29T08:00:00Z"))
            .unwrap()
    }

    #[test]
    fn missing_credentials_short_circuits_before_any_network_call() {
        // Endpoint points nowhere reachable; a network attempt would error
        // differently, proving the credential check ran first.
        let client = UsageClient::new("0.0.0").with_endpoint("http://127.0.0.1:1/never");
        let outcome = poll_once(&client, Path::new("C:/nope/absent.json"), 0);
        assert_eq!(outcome, PollOutcome::Auth(AuthProblem::NotSignedIn));
    }

    #[test]
    fn success_populates_state_and_clears_errors() {
        let mut state = SharedState {
            auth: Some(AuthProblem::ReloginRequired),
            throttled: true,
            last_error: Some("boom".into()),
            ..Default::default()
        };
        let now = at("2026-07-29T08:05:00Z");

        apply(&mut state, &PollOutcome::Ok(Box::new(sample_snapshot())), now);

        assert!(state.snapshot.is_some());
        assert!(state.auth.is_none());
        assert!(!state.throttled);
        assert_eq!(state.last_ok, Some(now));
        assert!(state.last_error.is_none());
        assert!(!state.has_problem());
    }

    #[test]
    fn auth_failure_records_the_problem() {
        let mut state = SharedState::default();
        apply(
            &mut state,
            &PollOutcome::Auth(AuthProblem::AccessTokenExpired),
            at("2026-07-29T08:00:00Z"),
        );
        assert_eq!(state.auth, Some(AuthProblem::AccessTokenExpired));
        assert!(state.has_problem());
    }

    #[test]
    fn throttling_preserves_the_last_good_snapshot() {
        let mut state = SharedState::default();
        let first = at("2026-07-29T08:00:00Z");
        apply(&mut state, &PollOutcome::Ok(Box::new(sample_snapshot())), first);

        apply(
            &mut state,
            &PollOutcome::Throttled { retry_after: None },
            at("2026-07-29T08:03:00Z"),
        );

        // Being rate limited says nothing about the numbers themselves.
        assert!(state.snapshot.is_some());
        assert!(state.throttled);
        assert_eq!(state.last_ok, Some(first));
        assert!(!state.has_problem());
    }

    #[test]
    fn network_failure_preserves_the_last_good_snapshot() {
        let mut state = SharedState::default();
        let first = at("2026-07-29T08:00:00Z");
        apply(&mut state, &PollOutcome::Ok(Box::new(sample_snapshot())), first);
        apply(
            &mut state,
            &PollOutcome::Failed("dns".into()),
            at("2026-07-29T08:03:00Z"),
        );
        assert!(state.snapshot.is_some());
        assert_eq!(state.last_ok, Some(first));
        assert_eq!(state.last_error.as_deref(), Some("dns"));
    }

    #[test]
    fn recovering_from_auth_clears_the_problem_flag() {
        let mut state = SharedState::default();
        apply(
            &mut state,
            &PollOutcome::Auth(AuthProblem::AccessTokenExpired),
            at("2026-07-29T08:00:00Z"),
        );
        assert!(state.has_problem());

        apply(
            &mut state,
            &PollOutcome::Ok(Box::new(sample_snapshot())),
            at("2026-07-29T08:05:00Z"),
        );
        assert!(!state.has_problem());
    }
}
