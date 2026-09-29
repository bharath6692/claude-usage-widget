//! Dynamic tray icon.
//!
//! Split as the design calls for: [`IconPlan`] is a pure, testable projection
//! of app state into "what should the icon look like"; [`render`] is the thin
//! GDI shell that paints exactly that plan into an `HICON`. Nothing below
//! `render` makes a decision — it only draws one.

use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, DIB_RGB_COLORS, HBITMAP,
};
use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, DestroyIcon, GetSystemMetrics, HICON, ICONINFO, SM_CXSMICON};

use crate::ui::theme::{self, Rgb};

/// What the icon should communicate. Independent of pixels or DPI.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IconPlan {
    /// 0.0..=1.0 fraction of the ring to fill.
    pub fraction: f64,
    pub color: Rgb,
    /// Overrides the ring with a flat warning glyph instead of a fill level.
    pub warn: bool,
    /// Numbers may be old; drawn at reduced opacity so it reads as "unsure".
    pub dim: bool,
}

pub fn plan_for(worst_percent: f64, has_auth_problem: bool, stale: bool) -> IconPlan {
    IconPlan {
        fraction: (worst_percent / 100.0).clamp(0.0, 1.0),
        color: theme::worst_state_color(worst_percent, has_auth_problem),
        warn: has_auth_problem,
        dim: stale,
    }
}

/// Render `plan` as a small square HICON (a filled ring / pie, or an amber
/// warning glyph). Caller owns the returned handle and must `DestroyIcon` it.
pub fn render(plan: &IconPlan) -> windows::core::Result<HICON> {
    // SM_CXSMICON tracks the system's actual small-icon size (16 at 100% DPI,
    // larger on high-DPI displays where Explorer scales the tray up).
    let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16);

    unsafe {
        let dc = CreateCompatibleDC(None);
        let mut bits_ptr: *mut core::ffi::c_void = std::ptr::null_mut();

        let mut bmi = BITMAPINFO::default();
        bmi.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size,
            biHeight: -size, // negative: top-down DIB, matches how we index rows
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };

        let color_bmp: HBITMAP =
            CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits_ptr, None, 0)?;
        let pixels = std::slice::from_raw_parts_mut(bits_ptr.cast::<u32>(), (size * size) as usize);
        paint(pixels, size, plan);

        // 1bpp mask, all zero = fully opaque everywhere the color bitmap has
        // real alpha; CreateIconIndirect only needs a same-sized placeholder
        // here since we already carry per-pixel alpha in the color bitmap.
        let mut mask_bmi = BITMAPINFO::default();
        mask_bmi.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size,
            biHeight: -size,
            biPlanes: 1,
            biBitCount: 1,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        let mut mask_bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let mask_bmp: HBITMAP =
            CreateDIBSection(Some(dc), &mask_bmi, DIB_RGB_COLORS, &mut mask_bits, None, 0)?;

        let icon_info = ICONINFO {
            fIcon: true.into(),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask_bmp,
            hbmColor: color_bmp,
        };
        let icon = CreateIconIndirect(&icon_info);

        let _ = DeleteObject(color_bmp.into());
        let _ = DeleteObject(mask_bmp.into());
        let _ = DeleteDC(dc);

        icon
    }
}

/// Draw directly into a top-down, premultiplied-alpha 32bpp pixel buffer.
/// A ring: `dim` lowers alpha, `warn` swaps the ring for a solid triangle.
fn paint(pixels: &mut [u32], size: i32, plan: &IconPlan) {
    let center = size as f64 / 2.0;
    let outer_r = size as f64 / 2.0 - 1.0;
    let inner_r = outer_r * 0.55;
    let alpha_scale: f64 = if plan.dim { 0.55 } else { 1.0 };
    let Rgb(r, g, b) = plan.color;

    let sweep_end = plan.fraction * std::f64::consts::TAU;

    for y in 0..size {
        for x in 0..size {
            let dx = x as f64 + 0.5 - center;
            let dy = y as f64 + 0.5 - center;
            let dist = (dx * dx + dy * dy).sqrt();

            let mut alpha = 0.0f64;

            if plan.warn {
                // Solid filled circle reads as a clear "different from usual" glyph.
                if dist <= outer_r {
                    alpha = 1.0;
                }
            } else if dist <= outer_r && dist >= inner_r {
                // atan2 with y flipped so angle 0 is "up" and increases clockwise,
                // matching how the fraction should sweep visually.
                let mut angle = (-dy).atan2(dx) - std::f64::consts::FRAC_PI_2;
                if angle < 0.0 {
                    angle += std::f64::consts::TAU;
                }
                if angle <= sweep_end {
                    alpha = 1.0;
                } else {
                    // Faint full-ring track so an empty widget isn't invisible.
                    alpha = 0.18;
                }
            }

            alpha *= alpha_scale;
            let a = (alpha * 255.0).round().clamp(0.0, 255.0) as u32;
            // Premultiplied BGRA, since that is what CreateIconIndirect expects
            // for a 32bpp color bitmap with a real alpha channel.
            let pr = (r as u32 * a) / 255;
            let pg = (g as u32 * a) / 255;
            let pb = (b as u32 * a) / 255;
            pixels[(y * size + x) as usize] = (a << 24) | (pr << 16) | (pg << 8) | pb;
        }
    }
}

pub fn destroy(icon: HICON) {
    unsafe {
        let _ = DestroyIcon(icon);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_fraction_tracks_percent() {
        assert_eq!(plan_for(0.0, false, false).fraction, 0.0);
        assert_eq!(plan_for(50.0, false, false).fraction, 0.5);
        assert_eq!(plan_for(100.0, false, false).fraction, 1.0);
    }

    #[test]
    fn fraction_is_clamped_to_a_valid_range() {
        assert_eq!(plan_for(150.0, false, false).fraction, 1.0);
        assert_eq!(plan_for(-10.0, false, false).fraction, 0.0);
    }

    #[test]
    fn auth_problem_forces_the_warning_glyph_regardless_of_percent() {
        let plan = plan_for(5.0, true, false);
        assert!(plan.warn);
        assert_eq!(plan.color, theme::AMBER_WARN);
    }

    #[test]
    fn color_follows_the_shared_ramp_when_no_auth_problem() {
        assert_eq!(plan_for(96.0, false, false).color, theme::RED);
        assert_eq!(plan_for(10.0, false, false).color, theme::GREEN);
    }

    #[test]
    fn stale_sets_the_dim_flag_independent_of_warn() {
        assert!(plan_for(10.0, false, true).dim);
        assert!(!plan_for(10.0, false, false).dim);
    }

    #[test]
    fn paint_produces_a_fully_transparent_corner_and_an_opaque_ring_pixel() {
        let size = 16;
        let mut pixels = vec![0u32; (size * size) as usize];
        let plan = IconPlan {
            fraction: 0.5,
            color: Rgb(255, 0, 0),
            warn: false,
            dim: false,
        };
        paint(&mut pixels, size, &plan);

        // Corner is outside the ring entirely: must be fully transparent.
        assert_eq!(pixels[0] >> 24, 0);

        // Somewhere along the ring must be opaque; otherwise the icon would
        // render as an invisible square, which is worse than any wrong color.
        assert!(pixels.iter().any(|p| (p >> 24) == 255));
    }

    #[test]
    fn dim_reduces_alpha_versus_the_same_plan_undimmed() {
        let size = 16;
        let plan_bright = IconPlan {
            fraction: 1.0,
            color: Rgb(255, 0, 0),
            warn: true,
            dim: false,
        };
        let plan_dim = IconPlan {
            fraction: 1.0,
            color: Rgb(255, 0, 0),
            warn: true,
            dim: true,
        };
        let mut bright = vec![0u32; (size * size) as usize];
        let mut dim = vec![0u32; (size * size) as usize];
        paint(&mut bright, size, &plan_bright);
        paint(&mut dim, size, &plan_dim);

        let center = (size / 2 * size + size / 2) as usize;
        assert!(bright[center] >> 24 > dim[center] >> 24);
    }
}
