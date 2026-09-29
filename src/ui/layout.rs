//! Pure layout geometry: rows → cell rects, measured widths across DPI scales.
//!
//! Zero UI dependencies — fully testable.

use crate::usage::model::DisplayRow;

#[derive(Debug, Clone)]
pub struct LayoutMetrics {
    pub dpi_scale: f64,
    // Consumed once the renderer selects a custom font instead of the DC
    // default; unused until then.
    #[allow(dead_code)]
    pub font_height_px: i32,
    pub cell_width_px: i32,
    pub cell_spacing_px: i32,
    pub row_height_px: i32,
    pub row_spacing_px: i32,
    pub padding_x_px: i32,
    pub padding_top_px: i32,
    pub padding_bottom_px: i32,
}

impl LayoutMetrics {
    pub fn for_dpi(dpi_scale: f64) -> Self {
        Self {
            dpi_scale,
            font_height_px: (11.0 * dpi_scale).round() as i32,
            cell_width_px: (12.0 * dpi_scale).round() as i32,
            cell_spacing_px: (1.0 * dpi_scale).round() as i32,
            row_height_px: (18.0 * dpi_scale).round() as i32,
            row_spacing_px: (2.0 * dpi_scale).round() as i32,
            padding_x_px: (6.0 * dpi_scale).round() as i32,
            padding_top_px: (3.0 * dpi_scale).round() as i32,
            padding_bottom_px: (3.0 * dpi_scale).round() as i32,
        }
    }

    /// Width is currently fixed regardless of row count (both docked rows are
    /// the same width); the parameter stays for when that stops being true.
    pub fn measured_width(&self, _num_rows: usize) -> i32 {
        let label_width = (20.0 * self.dpi_scale).round() as i32;
        let cells = 10;
        let cell_row_width = cells * self.cell_width_px + (cells - 1) * self.cell_spacing_px;
        let percent_width = (24.0 * self.dpi_scale).round() as i32;
        let remaining_width = (48.0 * self.dpi_scale).round() as i32;
        label_width + cell_row_width + 4 + percent_width + 4 + remaining_width + 2 * self.padding_x_px
    }

    pub fn measured_height(&self, num_rows: usize) -> i32 {
        if num_rows == 0 {
            return 0;
        }
        let rows_height = num_rows as i32 * self.row_height_px
            + (num_rows - 1).max(0) as i32 * self.row_spacing_px;
        rows_height + self.padding_top_px + self.padding_bottom_px
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl CellRect {
    pub fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self { x, y, width, height }
    }

    pub fn right(&self) -> i32 {
        self.x + self.width
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.height
    }
}

#[derive(Debug, Clone)]
pub struct RowLayout {
    // Kept for symmetry/debugging; the renderer paints from the per-element
    // rects (label/cells/percent/remaining) rather than this directly.
    #[allow(dead_code)]
    pub y: i32,
    pub label: CellRect,
    pub cells: Vec<CellRect>,
    pub percent: CellRect,
    pub remaining: CellRect,
}

pub fn layout_rows(metrics: &LayoutMetrics, rows: &[DisplayRow]) -> Vec<RowLayout> {
    let mut result = Vec::new();
    let mut y = metrics.padding_top_px;

    for _ in rows {
        let mut x = metrics.padding_x_px;

        let label_rect = CellRect::new(x, y, (20.0 * metrics.dpi_scale).round() as i32, metrics.row_height_px);
        x = label_rect.right() + 2;

        let mut cells = Vec::new();
        for _ in 0..10 {
            let cell_rect = CellRect::new(x, y + 2, metrics.cell_width_px, metrics.cell_width_px);
            cells.push(cell_rect);
            x = cell_rect.right() + metrics.cell_spacing_px;
        }
        x -= metrics.cell_spacing_px;
        x += 4;

        let percent_rect = CellRect::new(x, y, (24.0 * metrics.dpi_scale).round() as i32, metrics.row_height_px);
        x = percent_rect.right() + 4;

        let remaining_rect = CellRect::new(x, y, (48.0 * metrics.dpi_scale).round() as i32, metrics.row_height_px);

        result.push(RowLayout {
            y,
            label: label_rect,
            cells,
            percent: percent_rect,
            remaining: remaining_rect,
        });

        y += metrics.row_height_px + metrics.row_spacing_px;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_scale_with_dpi() {
        let m100 = LayoutMetrics::for_dpi(1.0);
        let m150 = LayoutMetrics::for_dpi(1.5);
        assert!(m150.font_height_px > m100.font_height_px);
    }

    #[test]
    fn measured_width_positive() {
        let m = LayoutMetrics::for_dpi(1.0);
        assert!(m.measured_width(2) > 200);
    }

    #[test]
    fn layout_produces_ten_cells() {
        let m = LayoutMetrics::for_dpi(1.0);
        let rows = vec![DisplayRow {
            label: "5h".into(),
            percent: 50.0,
            provisional: false,
            remaining: None,
        }];
        let layouts = layout_rows(&m, &rows);
        assert_eq!(layouts[0].cells.len(), 10);
    }
}
