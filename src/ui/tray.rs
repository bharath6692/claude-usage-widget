//! Win32 shell: window, tray icon, and context menu wiring.
//!
//! This module makes no decisions of its own — it renders [`menu::MenuNode`]
//! trees into a real `HMENU`, turns an [`icon::IconPlan`] into an `HICON`, and
//! forwards selections to [`menu::handle_selection`]. Everything that decides
//! *what* to show lives in `menu.rs` / `icon.rs` / `usage/`, where it can be
//! unit tested without a message loop.

use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DispatchMessageW,
    GetCursorPos, GetMessageW, LoadCursorW, PostMessageW, PostQuitMessage, RegisterClassW,
    SetForegroundWindow, SetMenuItemInfoW, SetWindowLongPtrW, TrackPopupMenuEx, TranslateMessage,
    CW_USEDEFAULT, GWLP_USERDATA, HMENU, IDC_ARROW, MENUITEMINFOW, MFT_STRING, MF_GRAYED,
    MF_POPUP, MF_SEPARATOR, MF_STRING, MIIM_STATE, MSG, TPM_RIGHTBUTTON, TPM_RETURNCMD,
    WM_APP, WM_COMMAND, WM_DESTROY, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WNDCLASSW, WS_EX_TOOLWINDOW,
    WS_OVERLAPPEDWINDOW,
};

use crate::config::{DisplayMode, Settings};
use crate::ui::{icon, layout, menu, overlay, popup, theme};
use crate::usage::alerts::{Alert, AlertState};
use crate::usage::client::UsageClient;
use crate::usage::client::RetryPolicy;
use crate::usage::model::project;
use crate::usage::poller::{self, PollOutcome, PollerConfig, PollerHandle, SharedState};

pub const WM_APP_USAGE: u32 = WM_APP + 1;
/// Posted by the overlay window (a distinct hwnd) when clicked, since it has
/// no direct access to `TrayApp`. `WM_APP + 2` is the tray icon's own
/// callback message (see `base_nid`), so this must not collide with it.
pub const WM_APP_SHOW_DETAILS: u32 = WM_APP + 3;
const TRAY_ICON_UID: u32 = 1;

/// Everything the window procedure needs. Owned via a raw pointer stashed in
/// `GWLP_USERDATA` — the message loop is single-threaded, so this is exactly
/// as unsafe as a `&mut self` method and no more.
pub struct TrayApp {
    hwnd: HWND,
    current_icon: Option<windows::Win32::UI::WindowsAndMessaging::HICON>,
    settings: Settings,
    shared: Arc<Mutex<SharedState>>,
    poller: Option<PollerHandle>,
    exe_path: PathBuf,
    overlay: Option<overlay::Overlay>,
    popup: Option<popup::Popup>,
    alerts: AlertState,
}

impl TrayApp {
    /// (worst primary %, has an auth problem, snapshot is stale).
    ///
    /// Staleness is "no good poll in ~3 intervals" (`UsageSnapshot::is_stale`),
    /// deliberately distinct from `throttled` (SharedState.throttled): a 429
    /// is an immediate, expected event that resolves on its own, whereas
    /// staleness means the numbers on screen might just be wrong by now.
    fn worst_percent_and_flags(&self) -> (f64, bool, bool) {
        let guard = self.shared.lock().expect("shared state poisoned");
        let worst = guard
            .snapshot
            .as_ref()
            .map(|s| s.worst_primary_percent())
            .unwrap_or(0.0);
        let stale = guard
            .snapshot
            .as_ref()
            .map(|s| s.is_stale(Utc::now(), self.settings.poll_interval()))
            .unwrap_or(false);
        (worst, guard.auth.is_some(), stale)
    }

    fn tooltip_text(&self) -> String {
        let guard = self.shared.lock().expect("shared state poisoned");
        if let Some(problem) = &guard.auth {
            return format!("Claude usage — {}", problem.short());
        }
        let Some(snapshot) = &guard.snapshot else {
            return "Claude usage — waiting for first poll".into();
        };
        let mut parts = Vec::new();
        for w in snapshot.primary() {
            parts.push(format!("{} {:.0}%", w.kind.label(), w.percent));
        }
        if guard.throttled {
            parts.push("(throttled)".into());
        }
        let mut tip = format!("Claude usage — {}", parts.join(" · "));
        tip.truncate(127); // szTip is a fixed [u16; 128] buffer, room for the null.
        tip
    }

    /// Rebuild the tray icon + tooltip from current state. Called after every
    /// poll result and after any settings change that could affect display.
    fn refresh_icon(&mut self) {
        let (worst, has_auth, stale) = self.worst_percent_and_flags();
        let plan = icon::plan_for(worst, has_auth, stale);

        let new_icon = match icon::render(&plan) {
            Ok(h) => h,
            Err(e) => {
                log::error!("failed to render tray icon: {e}");
                return;
            }
        };

        let tip = self.tooltip_text();
        let mut nid = base_nid(self.hwnd);
        nid.uFlags = NIF_ICON | NIF_TIP;
        nid.hIcon = new_icon;
        set_tip(&mut nid, &tip);

        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
        }

        if let Some(old) = self.current_icon.replace(new_icon) {
            icon::destroy(old);
        }
    }

    /// Create/destroy the overlay window to match `settings.display_mode`,
    /// then (if present) repaint it from the latest snapshot. Called after
    /// every poll result and after any menu change that could affect it.
    fn sync_overlay(&mut self) {
        match self.settings.display_mode {
            DisplayMode::Docked => {
                if self.overlay.is_none() {
                    match overlay::Overlay::new(self.hwnd, layout::LayoutMetrics::for_dpi(1.0), self.settings.overlay_x_offset) {
                        Ok(o) => self.overlay = Some(o),
                        Err(e) => log::error!("failed to create overlay window: {e}"),
                    }
                }
            }
            DisplayMode::TrayOnly => {
                self.overlay = None; // Drop destroys the window.
            }
        }

        let Some(overlay) = &mut self.overlay else { return };

        let guard = self.shared.lock().expect("shared state poisoned");
        let now = Utc::now();
        let is_stale = guard
            .snapshot
            .as_ref()
            .map(|s| s.is_stale(now, self.settings.poll_interval()))
            .unwrap_or(false);

        let rows = guard
            .snapshot
            .as_ref()
            .map(|s| project(s, now, true))
            .unwrap_or_default();
        let row_colors: Vec<(theme::Rgb, bool)> = rows
            .iter()
            .map(|r| (theme::ramp_color(r.percent), is_stale || r.provisional))
            .collect();
        drop(guard);

        overlay.update(rows, row_colors, theme::Rgb(30, 30, 30));
        overlay.show();
    }

    /// Show the detail popup, creating it on first use. Triggered by the
    /// "Usage details..." menu item, a tray-icon double-click, or a click on
    /// the docked overlay.
    fn show_popup(&mut self) {
        if self.popup.is_none() {
            match popup::Popup::new(self.hwnd) {
                Ok(p) => self.popup = Some(p),
                Err(e) => {
                    log::error!("failed to create popup window: {e}");
                    return;
                }
            }
        }
        let Some(popup) = &mut self.popup else { return };

        let guard = self.shared.lock().expect("shared state poisoned");
        popup.show(guard.snapshot.as_ref(), guard.auth.as_ref(), guard.last_ok, guard.throttled);
    }

    /// Feed the latest poll outcome through `AlertState` and toast anything it
    /// returns. Mirrors `poller::apply`'s auth/snapshot exclusivity: a poll
    /// either failed on auth (snapshot stays whatever it was) or it
    /// succeeded (auth is cleared) — never both from the same poll.
    fn process_alerts(&mut self) {
        if !self.settings.alerts_enabled {
            return;
        }
        let guard = self.shared.lock().expect("shared state poisoned");
        let now = Utc::now();
        let fired = if let Some(problem) = &guard.auth {
            self.alerts.on_auth_problem(problem, now)
        } else if let Some(snapshot) = &guard.snapshot {
            self.alerts.on_snapshot(snapshot, &self.settings.thresholds, now)
        } else {
            Vec::new()
        };
        drop(guard);

        for alert in fired {
            self.fire_alert_balloon(&alert);
        }
    }

    fn fire_alert_balloon(&self, alert: &Alert) {
        let (title, text, is_warning) = match alert {
            Alert::Threshold { label, percent, threshold } => (
                "Claude usage threshold".to_string(),
                format!("{label} usage is at {percent:.0}% (crossed {threshold}%)"),
                *threshold >= 90,
            ),
            Alert::Auth(problem) => (
                "Claude sign-in issue".to_string(),
                problem.message().to_string(),
                true,
            ),
        };
        fire_balloon(self.hwnd, &title, &text, is_warning);
    }

    /// Blocking on the UI thread is deliberate: this is a manual, rare,
    /// short-timeout action, and the project has no async runtime by design.
    fn check_for_updates(&self) {
        let outcome = crate::update::check(
            self.settings.update_manifest_url.as_deref(),
            env!("CARGO_PKG_VERSION"),
        );
        let (title, text, is_warning) = match outcome {
            crate::update::UpdateCheckOutcome::NotConfigured => (
                "Check for updates",
                "No update location is configured for this build.".to_string(),
                false,
            ),
            crate::update::UpdateCheckOutcome::UpToDate { current } => (
                "Check for updates",
                format!("You're up to date (v{current})."),
                false,
            ),
            crate::update::UpdateCheckOutcome::Available { current, latest, notes, .. } => (
                "Update available",
                match notes {
                    Some(n) => format!("v{latest} is available (you have v{current}). {n}"),
                    None => format!("v{latest} is available (you have v{current})."),
                },
                false,
            ),
            crate::update::UpdateCheckOutcome::Failed(detail) => (
                "Check for updates",
                format!("Could not check for updates: {detail}"),
                true,
            ),
        };
        fire_balloon(self.hwnd, title, &text, is_warning);
    }

    fn show_context_menu(&mut self) {
        let (_, has_auth, _) = self.worst_percent_and_flags();
        let nodes = menu::build(&self.settings, has_auth);
        let hmenu = build_hmenu(&nodes);

        let mut pt = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut pt);
            // Required so the menu reliably dismisses on outside click/Esc.
            let _ = SetForegroundWindow(self.hwnd);
            let id = TrackPopupMenuEx(
                hmenu,
                (TPM_RIGHTBUTTON | TPM_RETURNCMD).0,
                pt.x,
                pt.y,
                self.hwnd,
                None,
            );
            let _ = PostMessageW(Some(self.hwnd), WM_NULL, WPARAM(0), LPARAM(0));
            let _ = DestroyMenu(hmenu);

            if id.0 != 0 {
                self.handle_command(id.0 as u16);
            }
        }
    }

    fn handle_command(&mut self, id: u16) {
        let action = menu::handle_selection(id, &mut self.settings);
        if let Err(e) = crate::config::save(&crate::config::settings_path(), &self.settings) {
            log::warn!("failed to save settings: {e}");
        }

        match action {
            menu::MenuAction::None => {}
            menu::MenuAction::RefreshNow => {
                if let Some(p) = &self.poller {
                    p.refresh_now();
                }
            }
            menu::MenuAction::ShowDetails => {
                self.show_popup();
            }
            menu::MenuAction::IntervalChanged(d) => {
                if let Some(p) = &self.poller {
                    p.set_interval(d);
                }
            }
            menu::MenuAction::AutostartChanged(enabled) => {
                crate::autostart::apply(enabled, &self.exe_path);
            }
            menu::MenuAction::OpenPath(path) => {
                open_in_explorer(&path);
            }
            menu::MenuAction::ShowAbout => {
                show_about_balloon(self.hwnd);
            }
            menu::MenuAction::CheckForUpdates => {
                self.check_for_updates();
            }
            menu::MenuAction::Quit => unsafe {
                PostQuitMessage(0);
            },
        }

        // Toggling alerts/thresholds/display mode changes what the menu itself
        // should show next time, so nothing else to do here — build() reads
        // settings fresh on every open.
        self.refresh_icon();
        self.sync_overlay();
    }
}

fn open_in_explorer(path: &std::path::Path) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if !path.exists() {
        let _ = std::fs::write(path, "");
    }
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        windows::Win32::UI::Shell::ShellExecuteW(
            None,
            windows::core::PCWSTR::null(),
            PCWSTR(wide.as_ptr()),
            windows::core::PCWSTR::null(),
            windows::core::PCWSTR::null(),
            windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL,
        );
    }
}

fn show_about_balloon(hwnd: HWND) {
    fire_balloon(
        hwnd,
        "Claude Usage Widget",
        &format!("v{} — {}", env!("CARGO_PKG_VERSION"), env!("CARGO_PKG_DESCRIPTION")),
        false,
    );
}

/// Fire a Win32 tray balloon toast (`NIF_INFO`). Used for the About panel,
/// threshold/auth alerts, and the update check result.
fn fire_balloon(hwnd: HWND, title: &str, text: &str, is_warning: bool) {
    let mut nid = base_nid(hwnd);
    nid.uFlags = windows::Win32::UI::Shell::NIF_INFO;
    set_wide(&mut nid.szInfoTitle, title);
    set_wide(&mut nid.szInfo, text);
    nid.dwInfoFlags = if is_warning {
        windows::Win32::UI::Shell::NIIF_WARNING
    } else {
        windows::Win32::UI::Shell::NIIF_INFO
    };
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

fn set_wide(buf: &mut [u16], s: &str) {
    buf.iter_mut().for_each(|c| *c = 0);
    let wide: Vec<u16> = s.encode_utf16().collect();
    let n = wide.len().min(buf.len() - 1);
    buf[..n].copy_from_slice(&wide[..n]);
}

fn set_tip(nid: &mut NOTIFYICONDATAW, tip: &str) {
    set_wide(&mut nid.szTip, tip);
}

fn base_nid(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut nid = NOTIFYICONDATAW::default();
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = TRAY_ICON_UID;
    nid.uCallbackMessage = WM_APP_USAGE + 1; // distinct from the poller's WM_APP_USAGE
    nid
}

/// Render a [`menu::MenuNode`] tree into a real popup menu. Caller owns the
/// returned `HMENU`; destroying the root also destroys attached submenus.
fn build_hmenu(nodes: &[menu::MenuNode]) -> HMENU {
    let hmenu = unsafe { CreatePopupMenu().expect("CreatePopupMenu") };
    append_items(hmenu, nodes);
    hmenu
}

fn append_items(hmenu: HMENU, nodes: &[menu::MenuNode]) {
    for node in nodes {
        match node {
            menu::MenuNode::Separator => unsafe {
                let _ = AppendMenuW(hmenu, MF_SEPARATOR, 0, PCWSTR::null());
            },
            menu::MenuNode::Action { id, label, enabled } => unsafe {
                let flags = MF_STRING | if *enabled { Default::default() } else { MF_GRAYED };
                let wide = to_wide(label);
                let _ = AppendMenuW(hmenu, flags, *id as usize, PCWSTR(wide.as_ptr()));
            },
            menu::MenuNode::Toggle {
                id,
                label,
                checked,
                enabled,
            } => unsafe {
                let flags = MF_STRING | if *enabled { Default::default() } else { MF_GRAYED };
                let wide = to_wide(label);
                let _ = AppendMenuW(hmenu, flags, *id as usize, PCWSTR(wide.as_ptr()));
                if *checked {
                    check_item(hmenu, *id);
                }
            },
            menu::MenuNode::Submenu { label, items } => unsafe {
                let sub = CreatePopupMenu().expect("CreatePopupMenu (submenu)");
                append_items(sub, items);
                let wide = to_wide(label);
                let _ = AppendMenuW(
                    hmenu,
                    MF_STRING | MF_POPUP,
                    sub.0 as usize,
                    PCWSTR(wide.as_ptr()),
                );
            },
        }
    }
}

fn check_item(hmenu: HMENU, id: u16) {
    let mut info = MENUITEMINFOW {
        cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
        fMask: MIIM_STATE,
        fType: MFT_STRING,
        fState: windows::Win32::UI::WindowsAndMessaging::MFS_CHECKED,
        ..Default::default()
    };
    unsafe {
        let _ = SetMenuItemInfoW(hmenu, id as u32, false, &mut info);
    }
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let ptr = windows::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(hwnd, GWLP_USERDATA);
    let app = (ptr as *mut TrayApp).as_mut();

    match msg {
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        m if m == WM_APP_USAGE => {
            if let Some(app) = app {
                app.refresh_icon();
                app.sync_overlay();
                app.process_alerts();
            }
            LRESULT(0)
        }
        m if m == WM_APP_SHOW_DETAILS => {
            if let Some(app) = app {
                app.show_popup();
            }
            LRESULT(0)
        }
        m if m == base_nid(hwnd).uCallbackMessage => {
            let event = lparam.0 as u32;
            if let Some(app) = app {
                match event {
                    WM_RBUTTONUP => app.show_context_menu(),
                    WM_LBUTTONUP => {
                        if let Some(p) = &app.poller {
                            p.refresh_now();
                        }
                    }
                    WM_LBUTTONDBLCLK => app.show_popup(),
                    _ => {}
                }
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            if let Some(app) = app {
                app.handle_command((wparam.0 & 0xFFFF) as u16);
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn register_class(hinstance: windows::Win32::Foundation::HMODULE) -> PCWSTR {
    let class_name = w!("ClaudeUsageWidgetTrayWindow");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        hInstance: hinstance.into(),
        lpszClassName: class_name,
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW).unwrap_or_default() },
        ..Default::default()
    };
    unsafe {
        RegisterClassW(&wc);
    }
    class_name
}

pub struct RunConfig {
    pub settings: Settings,
    pub client: UsageClient,
    pub credentials_path: PathBuf,
    pub retry: RetryPolicy,
    pub exe_path: PathBuf,
}

/// Create the hidden owner window, add the tray icon, spawn the poller (whose
/// update callback needs `hwnd`, hence built here rather than passed in), and
/// pump messages until Quit is selected. Blocks the calling thread.
pub fn run(config: RunConfig) -> windows::core::Result<()> {
    let hinstance = unsafe { GetModuleHandleW(None)? };
    let class_name = register_class(hinstance.into());
    if let Err(e) = overlay::register_window_class() {
        log::error!("failed to register overlay window class: {e}");
    }
    if let Err(e) = popup::register_window_class() {
        log::error!("failed to register popup window class: {e}");
    }

    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class_name,
            w!("Claude Usage Widget"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            0,
            0,
            None,
            None,
            Some(hinstance.into()),
            None,
        )?
    };

    let shared = Arc::new(Mutex::new(SharedState::default()));
    let poller_config = PollerConfig {
        client: config.client,
        credentials_path: config.credentials_path,
        interval: config.settings.poll_interval(),
        retry: config.retry,
    };
    let poller = poller::spawn(poller_config, shared.clone(), notify_hwnd_on_update(hwnd));

    let mut app = Box::new(TrayApp {
        hwnd,
        current_icon: None,
        settings: config.settings,
        shared,
        poller: Some(poller),
        exe_path: config.exe_path,
        overlay: None,
        popup: None,
        alerts: AlertState::new(),
    });

    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, app.as_mut() as *mut TrayApp as isize);
    }

    let mut nid = base_nid(hwnd);
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    set_tip(&mut nid, "Claude usage — waiting for first poll");
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &nid);
    }
    app.refresh_icon();
    app.sync_overlay();

    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).into() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    let mut delete_nid = base_nid(hwnd);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &delete_nid);
    }
    let _ = &mut delete_nid; // silence unused-mut when NIM_DELETE ignores flags

    if let Some(icon) = app.current_icon.take() {
        icon::destroy(icon);
    }
    if let Some(poller) = app.poller.take() {
        poller.shutdown();
    }

    Ok(())
}

/// Bridges the poller thread's callback to the UI thread: it must only ever
/// `PostMessage`, never touch UI state directly (that state lives on the UI
/// thread and is owned by `TrayApp`, not `Send` across for good reason).
pub fn notify_hwnd_on_update(hwnd: HWND) -> impl Fn(&PollOutcome) + Send + 'static {
    let raw = hwnd.0 as isize;
    move |_outcome: &PollOutcome| {
        let hwnd = HWND(raw as *mut core::ffi::c_void);
        unsafe {
            let _ = PostMessageW(Some(hwnd), WM_APP_USAGE, WPARAM(0), LPARAM(0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_strings_are_null_terminated() {
        let w = to_wide("hi");
        assert_eq!(w, vec![b'h' as u16, b'i' as u16, 0]);
    }

    #[test]
    fn set_wide_truncates_to_buffer_and_stays_null_terminated() {
        let mut buf = [1u16; 4];
        set_wide(&mut buf, "abcdef");
        assert_eq!(buf, [b'a' as u16, b'b' as u16, b'c' as u16, 0]);
    }

    #[test]
    fn set_wide_clears_leftover_bytes_from_a_shorter_string() {
        let mut buf = [7u16; 6];
        set_wide(&mut buf, "ab");
        assert_eq!(buf, [b'a' as u16, b'b' as u16, 0, 0, 0, 0]);
    }
}
