//! The action wheel (padmap.rs) over the picture: its items in a ring
//! around the middle, from the top clockwise, the one the stick points at
//! highlighted. Only the items' own cells are drawn, so the game shows
//! between them.

use super::draw::{self, Grid, Layout};
use crate::padmap::WheelView;
use crate::video::Frame;

/// The longest an item's name is shown.
const NAME_COLS: usize = 18;

pub fn draw(frame: &mut Frame, view: &WheelView) {
    if view.items.is_empty() {
        return;
    }
    let layout = Layout::for_frame(frame.width as usize, frame.height as usize);
    let (cols, rows) = (layout.cols, layout.rows);
    if cols < 24 || rows < 8 {
        return;
    }
    let mut g = Grid::new(cols, rows);
    let (cx, cy) = (cols as f32 / 2.0, rows as f32 / 2.0);
    // A cell is twice as high as it is wide: a ring, not an oval.
    let ry = (cy - 2.0).min(7.0);
    let rx = (ry * 2.2).min(cx - NAME_COLS as f32 / 2.0 - 2.0);
    let count = view.items.len();
    let mut areas = Vec::new();
    for (i, name) in view.items.iter().enumerate() {
        let angle = i as f32 / count as f32 * std::f32::consts::TAU;
        let name = if name.is_empty() { (i + 1).to_string() } else { name.chars().take(NAME_COLS).collect() };
        let text = format!(" {} ", name);
        let width = text.chars().count();
        let col = ((cx + rx * angle.sin()) - width as f32 / 2.0).round().clamp(0.0, (cols - width) as f32) as usize;
        let row = (cy - ry * angle.cos()).round().clamp(0.0, (rows - 1) as f32) as usize;
        let selected = view.selected == Some(i);
        if selected {
            g.background(col, row, width, draw::SELECT);
        }
        g.text(col, row, &text, if selected { draw::BRIGHT } else { draw::TEXT });
        areas.push((col..col + width, row..row + 1));
    }
    // The middle: where the stick points from.
    let mid = (cx as usize - 1, cy as usize);
    g.text(mid.0, mid.1, "\u{2022}", draw::KEY);
    areas.push((mid.0..mid.0 + 1, mid.1..mid.1 + 1));
    for area in areas {
        draw::render_area(&g, &layout, frame, draw::PANEL_ALPHA, area);
    }
}
