//! Taskbar positioning via SHAppBarMessage.

use windows::Win32::Foundation::{RECT, LPARAM};
use windows::Win32::UI::Shell::{SHAppBarMessage, ABM_GETTASKBARPOS, ABM_GETSTATE, ABS_AUTOHIDE, APPBARDATA};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

#[derive(Debug, Clone, Copy)]
pub struct TaskbarPos {
    pub rect: RECT,
    pub edge: u32,
    // Consumed once the overlay hides itself when the taskbar auto-hides;
    // unused until that's wired up.
    #[allow(dead_code)]
    pub auto_hide: bool,
}

impl TaskbarPos {
    pub fn current() -> Self {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

            let mut data = APPBARDATA {
                cbSize: std::mem::size_of::<APPBARDATA>() as u32,
                hWnd: Default::default(),
                uCallbackMessage: 0,
                uEdge: 0,
                rc: RECT { left: 0, top: 0, right: 0, bottom: 0 },
                lParam: LPARAM(0),
            };

            SHAppBarMessage(ABM_GETTASKBARPOS, &mut data);
            let taskbar_rect = data.rc;
            let edge = data.uEdge;

            let mut state_data = APPBARDATA {
                cbSize: std::mem::size_of::<APPBARDATA>() as u32,
                hWnd: Default::default(),
                uCallbackMessage: 0,
                uEdge: 0,
                rc: RECT { left: 0, top: 0, right: 0, bottom: 0 },
                lParam: LPARAM(0),
            };
            let state = SHAppBarMessage(ABM_GETSTATE, &mut state_data) as u32;
            let auto_hide = (state & ABS_AUTOHIDE) != 0;

            CoUninitialize();

            Self {
                rect: taskbar_rect,
                edge,
                auto_hide,
            }
        }
    }

    // Exercised by the unit tests below; no production caller needs the
    // taskbar's width yet (only its height, for vertical centering).
    #[allow(dead_code)]
    pub fn width(&self) -> i32 {
        self.rect.right - self.rect.left
    }

    pub fn height(&self) -> i32 {
        self.rect.bottom - self.rect.top
    }

    pub fn is_right(&self) -> bool {
        self.edge == 2
    }

    pub fn overlay_position(&self, overlay_width: i32, overlay_height: i32, x_offset: i32) -> (i32, i32) {
        if self.is_right() {
            let x = self.rect.left - overlay_width - x_offset;
            let y = self.rect.top + (self.height() - overlay_height) / 2;
            (x, y)
        } else {
            let x = self.rect.right - overlay_width - x_offset;
            let y = self.rect.top - overlay_height - 2;
            (x, y)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taskbar_right_edge_position() {
        let tb = TaskbarPos {
            rect: RECT { left: 1800, top: 0, right: 1920, bottom: 1080 },
            edge: 2,
            auto_hide: false,
        };
        let (x, _y) = tb.overlay_position(200, 40, 0);
        assert!(x < tb.rect.left);
    }

    #[test]
    fn taskbar_dimensions() {
        let tb = TaskbarPos {
            rect: RECT { left: 0, top: 1050, right: 1920, bottom: 1080 },
            edge: 3,
            auto_hide: false,
        };
        assert_eq!(tb.width(), 1920);
        assert_eq!(tb.height(), 30);
    }
}
