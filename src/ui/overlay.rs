//! Docked overlay window: a topmost popup anchored to the taskbar, painting
//! the two usage rows directly (the same bars the reference image shows).
//!
//! WS_POPUP with WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST —
//! NOACTIVATE means it never steals focus but still delivers clicks.

use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, HBRUSH, PAINTSTRUCT};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::ui::layout::LayoutMetrics;
use crate::ui::render;
use crate::ui::taskbar::TaskbarPos;
use crate::ui::theme::Rgb;
use crate::usage::model::DisplayRow;

/// Data painted on WM_PAINT. Stashed in the overlay window's GWLP_USERDATA so
/// the raw wndproc can reach it without a global.
struct OverlayState {
    metrics: LayoutMetrics,
    rows: Vec<DisplayRow>,
    row_colors: Vec<(Rgb, bool)>,
    bg_color: Rgb,
}

pub struct Overlay {
    hwnd: HWND,
    state: Box<OverlayState>,
}

impl Overlay {
    pub fn new(
        parent_hwnd: HWND,
        metrics: LayoutMetrics,
        x_offset: i32,
    ) -> Result<Self, String> {
        let taskbar_pos = TaskbarPos::current();
        let width = metrics.measured_width(2);
        let height = metrics.measured_height(2);
        let (x, y) = taskbar_pos.overlay_position(width, height, x_offset);

        let mut state = Box::new(OverlayState {
            metrics,
            rows: Vec::new(),
            row_colors: Vec::new(),
            bg_color: Rgb(30, 30, 30),
        });

        unsafe {
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
                w!("ClaudeUsageWidgetOverlay"),
                w!("Claude Usage"),
                WS_POPUP | WS_VISIBLE,
                x,
                y,
                width,
                height,
                Some(parent_hwnd),
                None,
                Default::default(),
                None,
            )
            .map_err(|e| format!("CreateWindowExW failed: {e:?}"))?;

            SetWindowLongPtrW(hwnd, GWLP_USERDATA, state.as_mut() as *mut OverlayState as isize);

            Ok(Self { hwnd, state })
        }
    }

    /// Update the displayed rows/colors and force a repaint.
    pub fn update(&mut self, rows: Vec<DisplayRow>, row_colors: Vec<(Rgb, bool)>, bg_color: Rgb) {
        self.state.rows = rows;
        self.state.row_colors = row_colors;
        self.state.bg_color = bg_color;

        let width = self.state.metrics.measured_width(self.state.rows.len());
        let height = self.state.metrics.measured_height(self.state.rows.len());
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                None,
                0,
                0,
                width,
                height,
                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOZORDER,
            );
            let _ = InvalidateRect(Some(self.hwnd), None, true);
        }
    }

    pub fn show(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
            );
        }
    }

    // Consumed by the fullscreen-app / taskbar-autohide detection that isn't
    // wired up yet; unused until then.
    #[allow(dead_code)]
    pub fn hide(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    /// Reposition against the taskbar's current rect (called on
    /// WM_DISPLAYCHANGE / WM_SETTINGCHANGE / a periodic poll).
    // Not wired to any message handler yet — the overlay is created once at
    // its correct position and never revalidated against taskbar moves.
    #[allow(dead_code)]
    pub fn reposition(&self, x_offset: i32) {
        let taskbar_pos = TaskbarPos::current();
        let width = self.state.metrics.measured_width(self.state.rows.len().max(2));
        let height = self.state.metrics.measured_height(self.state.rows.len().max(2));
        let (x, y) = taskbar_pos.overlay_position(width, height, x_offset);
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                x,
                y,
                width,
                height,
                SWP_NOACTIVATE,
            );
        }
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

pub fn register_window_class() -> Result<(), String> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::HICON;

    unsafe {
        let wnd_class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: Default::default(),
            hIcon: HICON::default(),
            hCursor: LoadCursorW(None, IDC_ARROW).ok().unwrap_or_default(),
            hbrBackground: HBRUSH::default(),
            lpszMenuName: PCWSTR::null(),
            lpszClassName: w!("ClaudeUsageWidgetOverlay"),
        };

        if RegisterClassW(&wnd_class) == 0 {
            return Err("Failed to register overlay window class".to_string());
        }

        Ok(())
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
            if let Some(state) = (ptr as *mut OverlayState).as_ref() {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                if !hdc.is_invalid() {
                    let layouts = crate::ui::layout::layout_rows(&state.metrics, &state.rows);
                    render::paint_rows(
                        hdc,
                        &state.metrics,
                        &state.rows,
                        &layouts,
                        &state.row_colors,
                        state.bg_color,
                    );
                }
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            // The overlay has no access to TrayApp state; its owner (the
            // hidden tray window, set via CreateWindowExW's parent param)
            // does, so bounce the click over as a message.
            if let Ok(parent) = GetParent(hwnd) {
                let _ = PostMessageW(
                    Some(parent),
                    crate::ui::tray::WM_APP_SHOW_DETAILS,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
            LRESULT(0)
        }
        WM_DESTROY => LRESULT(0), // overlay destruction does not quit the app
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn overlay_module_compiles() {
        // Real window creation needs a live desktop; covered by manual checklist.
    }
}
