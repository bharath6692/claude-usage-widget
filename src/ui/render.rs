//! Minimal GDI rendering: paint rows into a window DC.
//!
//! No DWM, no DComposition, no advanced DIB — just basic Win32 painting.

use windows::Win32::Foundation::{RECT, COLORREF};
use windows::Win32::Graphics::Gdi::{
    CreateSolidBrush, DeleteObject, DrawTextW, FillRect, SetBkMode, SetTextColor, HGDIOBJ,
    DT_LEFT, DT_SINGLELINE, DT_VCENTER, TRANSPARENT,
};

use crate::ui::layout::{CellRect, LayoutMetrics, RowLayout};
use crate::ui::theme::Rgb;
use crate::usage::model::{format_remaining, DisplayRow};

pub fn paint_rows(
    hdc: windows::Win32::Graphics::Gdi::HDC,
    metrics: &LayoutMetrics,
    rows: &[DisplayRow],
    layouts: &[RowLayout],
    row_colors: &[(Rgb, bool)],
    bg_color: Rgb,
) -> (i32, i32) {
    let width = metrics.measured_width(rows.len());
    let height = metrics.measured_height(rows.len());

    if width <= 0 || height <= 0 {
        return (width, height);
    }

    // Clear background
    let bg_rect = RECT {
        left: 0,
        top: 0,
        right: width,
        bottom: height,
    };
    unsafe {
        let color_ref = COLORREF(bg_color.to_colorref());
        let brush = CreateSolidBrush(color_ref);
        let _ = FillRect(hdc, &bg_rect, brush);
        let _ = DeleteObject(HGDIOBJ(brush.0));
    }

    // Render rows
    for (i, (row, layout)) in rows.iter().zip(layouts.iter()).enumerate() {
        let (color, is_dimmed) = row_colors.get(i).copied().unwrap_or((Rgb(200, 200, 200), false));
        let text_color = if is_dimmed { Rgb(140, 140, 140) } else { Rgb(230, 230, 230) };

        let filled = ((row.percent / 100.0) * 10.0).round() as usize;
        for (j, cell) in layout.cells.iter().enumerate() {
            let cell_color = if j < filled { color } else { Rgb(60, 60, 60) };
            paint_cell(hdc, cell, cell_color);
        }

        paint_text(hdc, &layout.label, &row.label, text_color);
        paint_text(hdc, &layout.percent, &format!("{:.0}%", row.percent), text_color);
        paint_text(hdc, &layout.remaining, &format_remaining(row.remaining), text_color);
    }

    (width, height)
}

fn paint_text(hdc: windows::Win32::Graphics::Gdi::HDC, rect: &CellRect, text: &str, color: Rgb) {
    unsafe {
        let _ = SetTextColor(hdc, COLORREF(color.to_colorref()));
        let _ = SetBkMode(hdc, TRANSPARENT);

        let mut wide: Vec<u16> = text.encode_utf16().collect();
        let mut rect_win = RECT {
            left: rect.x,
            top: rect.y,
            right: rect.right(),
            bottom: rect.bottom(),
        };
        DrawTextW(hdc, &mut wide, &mut rect_win, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
    }
}

fn paint_cell(hdc: windows::Win32::Graphics::Gdi::HDC, rect: &CellRect, color: Rgb) {
    unsafe {
        let brush = CreateSolidBrush(COLORREF(color.to_colorref()));
        let rect_win = RECT {
            left: rect.x,
            top: rect.y,
            right: rect.right(),
            bottom: rect.bottom(),
        };
        let _ = FillRect(hdc, &rect_win, brush);
        let _ = DeleteObject(HGDIOBJ(brush.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paint_rows_returns_positive_dimensions() {
        // Basic smoke test — actual rendering requires a window DC
        let m = LayoutMetrics::for_dpi(1.0);
        let rows = vec![DisplayRow {
            label: "5h".into(),
            percent: 50.0,
            provisional: false,
            remaining: None,
        }];
        let layouts = crate::ui::layout::layout_rows(&m, &rows);
        let colors = vec![(Rgb(76, 175, 80), false)];
        let (w, h) = paint_rows(
            unsafe { std::mem::zeroed() }, // NULL DC is OK for dimension test
            &m,
            &rows,
            &layouts,
            &colors,
            Rgb(255, 255, 255),
        );
        assert!(w > 0);
        assert!(h > 0);
    }
}
