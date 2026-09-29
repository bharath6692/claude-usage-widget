//! Detail popup: a click-through panel showing every usage window (including
//! per-model ones the docked bars have no room for), `extra_usage` credits,
//! and the last successful poll — everything the tray tooltip can't fit.
//!
//! Dismisses on losing activation (click outside) or Esc, like a standard
//! Win32 popup menu.

use windows::core::w;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect, InvalidateRect,
    SetBkMode, SetTextColor, DT_LEFT, DT_NOCLIP, HBRUSH, HGDIOBJ, PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use chrono::{DateTime, Utc};

use crate::creds::AuthProblem;
use crate::ui::taskbar::TaskbarPos;
use crate::usage::model::{format_remaining, UsageSnapshot};

const VK_ESCAPE: u32 = 0x1B;
const LINE_HEIGHT: i32 = 20;
const PADDING: i32 = 10;
const WIDTH: i32 = 460;

struct PopupState {
    lines: Vec<String>,
}

pub struct Popup {
    hwnd: HWND,
    state: Box<PopupState>,
}

impl Popup {
    pub fn new(parent_hwnd: HWND) -> Result<Self, String> {
        let mut state = Box::new(PopupState { lines: Vec::new() });

        unsafe {
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                w!("ClaudeUsageWidgetPopup"),
                w!("Claude Usage Details"),
                WS_POPUP | WS_BORDER,
                0,
                0,
                WIDTH,
                LINE_HEIGHT * 4,
                Some(parent_hwnd),
                None,
                Default::default(),
                None,
            )
            .map_err(|e| format!("CreateWindowExW failed: {e:?}"))?;

            SetWindowLongPtrW(hwnd, GWLP_USERDATA, state.as_mut() as *mut PopupState as isize);

            Ok(Self { hwnd, state })
        }
    }

    /// Build the content lines from live state and show near the taskbar.
    pub fn show(
        &mut self,
        snapshot: Option<&UsageSnapshot>,
        auth: Option<&AuthProblem>,
        last_poll: Option<DateTime<Utc>>,
        throttled: bool,
    ) {
        self.state.lines = build_lines(snapshot, auth, last_poll, throttled);
        let height = PADDING * 2 + LINE_HEIGHT * self.state.lines.len() as i32;

        let taskbar = TaskbarPos::current();
        let (x, base_y) = taskbar.overlay_position(WIDTH, height, 0);
        let y = base_y - LINE_HEIGHT; // small gap above the bars/tray row

        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                x,
                y,
                WIDTH,
                height,
                SWP_NOACTIVATE,
            );
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            let _ = SetForegroundWindow(self.hwnd);
            let _ = InvalidateRect(Some(self.hwnd), None, true);
        }
    }

    // No caller hides the popup programmatically yet — it only closes itself
    // (WM_ACTIVATE/Esc, handled in wndproc); kept for a future "Settings
    // changed, dismiss any open popup" path.
    #[allow(dead_code)]
    pub fn hide(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }
}

impl Drop for Popup {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// Turn live state into the exact lines painted, so formatting is testable
/// without a window.
fn build_lines(
    snapshot: Option<&UsageSnapshot>,
    auth: Option<&AuthProblem>,
    last_poll: Option<DateTime<Utc>>,
    throttled: bool,
) -> Vec<String> {
    let now = Utc::now();
    let mut lines = Vec::new();
    lines.push("Claude Usage - Details".to_string());
    lines.push(String::new());

    if let Some(problem) = auth {
        lines.push(format!("[!] {}", problem.message()));
        lines.push(problem.remediation().to_string());
        lines.push(String::new());
    }

    if let Some(snap) = snapshot {
        for w in &snap.windows {
            let expired = w.resets_at.map(|r| now >= r).unwrap_or(false);
            let percent = if expired { 0.0 } else { w.percent };
            let resets_text = w
                .resets_at
                .map(|r| r.format("%Y-%m-%d %H:%M UTC").to_string())
                .unwrap_or_else(|| "-".to_string());
            let remaining = w.resets_at.and_then(|r| {
                r.signed_duration_since(now)
                    .to_std()
                    .ok()
                    .filter(|d| !d.is_zero())
            });
            let remaining_text = format_remaining(remaining);
            let sev = w
                .severity
                .as_deref()
                .map(|s| format!(" [{s}]"))
                .unwrap_or_default();
            lines.push(format!(
                "{:<14}{:>4.0}%{}  resets {} ({})",
                w.kind.label(),
                percent,
                sev,
                resets_text,
                remaining_text
            ));
        }

        if let Some(extra) = &snap.extra_usage {
            if extra.enabled {
                lines.push(String::new());
                let used = extra.used_credits.unwrap_or(0.0);
                let limit = extra.monthly_limit.unwrap_or(0.0);
                let currency = extra.currency.as_deref().unwrap_or("USD");
                let util = extra.utilization.unwrap_or(0.0);
                lines.push(format!(
                    "Extra usage: {util:.0}%  ({used:.2}/{limit:.2} {currency})"
                ));
            }
        }
    } else if auth.is_none() {
        lines.push("Waiting for first poll...".to_string());
    }

    lines.push(String::new());
    if throttled {
        lines.push("(currently rate-limited by Anthropic)".to_string());
    }
    let last_poll_text = last_poll
        .map(|t| t.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| "never".to_string());
    lines.push(format!("Last successful poll: {last_poll_text}"));
    lines.push(String::new());
    lines.push("Esc or click outside to close".to_string());

    lines
}

pub fn register_window_class() -> Result<(), String> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::HICON;

    unsafe {
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: Default::default(),
            hIcon: HICON::default(),
            hCursor: LoadCursorW(None, IDC_ARROW).ok().unwrap_or_default(),
            hbrBackground: HBRUSH::default(),
            lpszMenuName: PCWSTR::null(),
            lpszClassName: w!("ClaudeUsageWidgetPopup"),
        };

        if RegisterClassW(&wc) == 0 {
            return Err("Failed to register popup window class".to_string());
        }

        Ok(())
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
            if let Some(state) = (ptr as *mut PopupState).as_ref() {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                if !hdc.is_invalid() {
                    let mut client_rect = RECT::default();
                    let _ = GetClientRect(hwnd, &mut client_rect);
                    let bg = CreateSolidBrush(COLORREF(0x0020_2020));
                    let _ = FillRect(hdc, &client_rect, bg);
                    let _ = DeleteObject(HGDIOBJ(bg.0));

                    let _ = SetBkMode(hdc, TRANSPARENT);
                    let _ = SetTextColor(hdc, COLORREF(0x00E0_E0E0));

                    for (i, line) in state.lines.iter().enumerate() {
                        // An empty spacer line has nothing to draw, and
                        // DrawTextW's zero-length dangling-pointer buffer
                        // (Vec::new()'s as_ptr()) crashes user32.dll — skip
                        // the GDI call entirely rather than feed it one.
                        if line.is_empty() {
                            continue;
                        }
                        let mut wide: Vec<u16> = line.encode_utf16().collect();
                        let mut rect = RECT {
                            left: PADDING,
                            top: PADDING + i as i32 * LINE_HEIGHT,
                            right: WIDTH - PADDING,
                            bottom: PADDING + (i as i32 + 1) * LINE_HEIGHT,
                        };
                        DrawTextW(hdc, &mut wide, &mut rect, DT_LEFT | DT_NOCLIP);
                    }
                }
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        WM_ACTIVATE => {
            let activate_state = (wparam.0 & 0xFFFF) as u32;
            if activate_state == 0 {
                // WA_INACTIVE
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            if wparam.0 as u32 == VK_ESCAPE {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
            LRESULT(0)
        }
        WM_DESTROY => LRESULT(0), // popup destruction does not quit the app
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
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
    fn no_data_shows_waiting_message() {
        let lines = build_lines(None, None, None, false);
        assert!(lines.iter().any(|l| l.contains("Waiting for first poll")));
    }

    #[test]
    fn auth_problem_shows_message_and_remediation() {
        let lines = build_lines(None, Some(&AuthProblem::ReloginRequired), None, false);
        assert!(lines.iter().any(|l| l.contains("[!]")));
        assert!(lines
            .iter()
            .any(|l| l.contains(AuthProblem::ReloginRequired.remediation())));
    }

    #[test]
    fn snapshot_lists_every_window() {
        let snap = sample_snapshot();
        let lines = build_lines(Some(&snap), None, None, false);
        assert!(lines.iter().any(|l| l.contains("5h")));
        assert!(lines.iter().any(|l| l.contains("7d")));
    }

    #[test]
    fn throttled_note_is_shown() {
        let lines = build_lines(None, None, None, true);
        assert!(lines.iter().any(|l| l.contains("rate-limited")));
    }

    #[test]
    fn last_poll_timestamp_is_formatted() {
        let lines = build_lines(None, None, Some(at("2026-07-29T08:05:00Z")), false);
        assert!(lines.iter().any(|l| l.contains("2026-07-29 08:05:00 UTC")));
    }

    #[test]
    fn no_last_poll_shows_never() {
        let lines = build_lines(None, None, None, false);
        assert!(lines.iter().any(|l| l.contains("never")));
    }
}
