//! Claude Usage Widget — Windows taskbar monitor for Claude rate-limit windows.

#![windows_subsystem = "windows"]

mod autostart;
mod config;
mod creds;
mod ui;
mod update;
mod usage;

use chrono::Utc;

use crate::usage::client::{detect_claude_code_version, RetryPolicy, UsageClient};
use crate::usage::model::{format_remaining, project};
use crate::usage::poller::{poll_once, PollOutcome};

const CELLS: usize = 10;

fn bar(percent: f64) -> String {
    let filled = ((percent / 100.0) * CELLS as f64).round().clamp(0.0, CELLS as f64) as usize;
    let mut s = String::new();
    for i in 0..CELLS {
        s.push(if i < filled { '\u{2588}' } else { '\u{2591}' });
    }
    s
}

/// M1 diagnostic path: one fetch, printed to the console, then exit. Kept
/// behind a flag as the fastest way to check the raw data against `/usage`
/// without waiting on the tray UI.
fn print_once(client: &UsageClient, creds_path: &std::path::Path) {
    let outcome = poll_once(client, creds_path, Utc::now().timestamp_millis());
    match outcome {
        PollOutcome::Ok(snapshot) => {
            let now = Utc::now();
            println!("--- overlay rows ---");
            for row in project(&snapshot, now, true) {
                let flag = if row.provisional { "  (provisional)" } else { "" };
                println!(
                    "{:>3} {} {:>3.0}% \u{b7} {}{}",
                    row.label,
                    bar(row.percent),
                    row.percent,
                    format_remaining(row.remaining),
                    flag
                );
            }
            println!("\nworst primary: {:.0}%", snapshot.worst_primary_percent());
        }
        PollOutcome::Auth(problem) => {
            println!("AUTH PROBLEM : {problem}");
            println!("remediation  : {}", problem.remediation());
        }
        PollOutcome::Throttled { retry_after } => {
            println!("THROTTLED    : retry_after={retry_after:?}");
        }
        PollOutcome::Failed(detail) => {
            println!("FAILED       : {detail}");
        }
    }
}

fn init_logging() {
    let log_path = config::log_path();
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(&log_path) {
        let _ = simplelog::WriteLogger::init(
            simplelog::LevelFilter::Info,
            simplelog::Config::default(),
            file,
        );
    }
}

fn main() {
    let settings_path = config::settings_path();
    let settings = config::load(&settings_path);

    let claude_dir = creds::default_path()
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default();
    let version = detect_claude_code_version(&claude_dir);
    let creds_path = settings.credentials_path_or_default();
    let client = UsageClient::new(&version);

    let print_only = std::env::args().any(|a| a == "--print-once");
    if print_only {
        println!("settings      : {}", settings_path.display());
        println!("credentials   : {}", creds_path.display());
        println!("user-agent    : claude-code/{version}");
        println!("poll interval : {}s\n", settings.poll_interval().as_secs());
        print_once(&client, &creds_path);
        return;
    }

    init_logging();
    log::info!("starting, user-agent=claude-code/{version}");

    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
    }

    let exe_path = std::env::current_exe().unwrap_or_default();
    if settings.start_with_windows {
        // Reconcile on every start too: covers the case where the user moved
        // the exe since the value was written.
        autostart::apply(true, &exe_path);
    }

    let run_config = ui::tray::RunConfig {
        settings,
        client,
        credentials_path: creds_path,
        retry: RetryPolicy::default(),
        exe_path,
    };

    if let Err(e) = ui::tray::run(run_config) {
        log::error!("tray app exited with error: {e}");
    }
}
