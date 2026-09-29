//! Color ramp shared by the tray icon, overlay, and popup.
//!
//! Thresholds are copied verbatim from the existing
//! `~/.claude/statusline-command.ps1` so the widget and the statusline never
//! disagree about what a given percentage means.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub const fn to_colorref(self) -> u32 {
        // Win32 COLORREF is 0x00BBGGRR, the reverse of the usual RGB order.
        (self.0 as u32) | ((self.1 as u32) << 8) | ((self.2 as u32) << 16)
    }
}

pub const GREEN: Rgb = Rgb(60, 200, 80);
pub const YELLOW: Rgb = Rgb(220, 200, 0);
pub const ORANGE: Rgb = Rgb(240, 140, 30);
pub const RED: Rgb = Rgb(220, 60, 60);
pub const AMBER_WARN: Rgb = Rgb(230, 160, 20);
// Consumed by a future theme-aware overlay background; unused until then.
#[allow(dead_code)]
pub const EMPTY_GRAY: Rgb = Rgb(60, 60, 60);
#[allow(dead_code)]
pub const DIM_GRAY: Rgb = Rgb(140, 140, 140);

/// `<50` green, `<75` yellow, `<90` orange, `>=90` red.
pub fn ramp_color(percent: f64) -> Rgb {
    if percent < 50.0 {
        GREEN
    } else if percent < 75.0 {
        YELLOW
    } else if percent < 90.0 {
        ORANGE
    } else {
        RED
    }
}

// Consumed by a future theme-aware overlay background, which will read HKCU
// Personalize to match the taskbar's own light/dark state; unused until then.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode {
    Dark,
    Light,
}

/// The tray icon's badge/base color for the worst-case state, independent of
/// light/dark mode (the ramp colors are chosen to read on both).
pub fn worst_state_color(worst_percent: f64, has_auth_problem: bool) -> Rgb {
    if has_auth_problem {
        AMBER_WARN
    } else {
        ramp_color(worst_percent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ramp_matches_statusline_thresholds() {
        assert_eq!(ramp_color(0.0), GREEN);
        assert_eq!(ramp_color(49.9), GREEN);
        assert_eq!(ramp_color(50.0), YELLOW);
        assert_eq!(ramp_color(74.9), YELLOW);
        assert_eq!(ramp_color(75.0), ORANGE);
        assert_eq!(ramp_color(89.9), ORANGE);
        assert_eq!(ramp_color(90.0), RED);
        assert_eq!(ramp_color(100.0), RED);
    }

    #[test]
    fn auth_problem_overrides_the_ramp_with_amber() {
        assert_eq!(worst_state_color(5.0, true), AMBER_WARN);
        assert_eq!(worst_state_color(96.0, true), AMBER_WARN);
        assert_eq!(worst_state_color(96.0, false), RED);
    }

    #[test]
    fn colorref_packs_bgr_not_rgb() {
        // Win32 COLORREF is 0x00BBGGRR — pure red must land in the low byte.
        assert_eq!(Rgb(255, 0, 0).to_colorref(), 0x0000_00FF);
        assert_eq!(Rgb(0, 255, 0).to_colorref(), 0x0000_FF00);
        assert_eq!(Rgb(0, 0, 255).to_colorref(), 0x00FF_0000);
    }
}
