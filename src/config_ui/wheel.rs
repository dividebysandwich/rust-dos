//! The action wheel (padmap.rs) over the picture: a ring around the
//! middle with a slice for each item, from the top clockwise, the one the
//! stick points at lit; and on the ring the items' names, each in a box of
//! its own. The game shows through the ring and around it.

use super::draw::{self, Grid, Layout, Rgb};
use crate::padmap::WheelView;
use crate::video::Frame;

/// The longest an item's name is shown.
const NAME_COLS: usize = 18;
/// How opaque the ring is, and its slice that is chosen, of 256.
const RING_ALPHA: u32 = 120;
const CHOSEN_ALPHA: u32 = 190;
/// The space around a name in its box, in pixels.
const PAD_X: usize = 6;
const PAD_Y: usize = 4;

/// Blend `color` over the pixel at (`x`, `y`).
fn tint(frame: &mut Frame, x: usize, y: usize, color: Rgb, alpha: u32) {
    let at = (y * frame.width as usize + x) * 3;
    if let Some(px) = frame.rgb.get_mut(at..at + 3) {
        let mixed = draw::blend([px[0], px[1], px[2]], color, alpha);
        px.copy_from_slice(&mixed);
    }
}

/// The item a direction from the middle (`dx` right, `dy` down) is in,
/// of `count`, as `PadMapper` picks them: from the top, clockwise.
fn item_at(dx: f32, dy: f32, count: usize) -> usize {
    let step = std::f32::consts::TAU / count as f32;
    let angle = dx.atan2(-dy).rem_euclid(std::f32::consts::TAU);
    ((angle + step / 2.0) / step) as usize % count
}

pub fn draw(frame: &mut Frame, view: &WheelView) {
    let count = view.items.len();
    if count == 0 {
        return;
    }
    let layout = Layout::for_frame(frame.width as usize, frame.height as usize);
    let (cols, rows) = (layout.cols, layout.rows);
    if cols < 24 || rows < 8 {
        return;
    }
    let (width, height) = (frame.width as usize, frame.height as usize);
    // The ring, in pixels, around the panel's middle.
    let (cx, cy) = ((layout.x + cols * 8 / 2) as f32, (layout.y + rows * layout.cell_h / 2) as f32);
    let outer = (width.min(height) as f32 * 0.46).min(cy - 2.0);
    let inner = outer * 0.3;
    let (x0, x1) = ((cx - outer).max(0.0) as usize, ((cx + outer) as usize + 1).min(width));
    let (y0, y1) = ((cy - outer).max(0.0) as usize, ((cy + outer) as usize + 1).min(height));
    for y in y0..y1 {
        for x in x0..x1 {
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            let d = (dx * dx + dy * dy).sqrt();
            if d < inner || d > outer {
                continue;
            }
            let chosen = view.selected == Some(item_at(dx, dy, count));
            // The chosen slice lit, with a bright rim.
            let (color, alpha) = match chosen {
                true if d > outer - 3.0 || d < inner + 2.0 => (draw::BRIGHT, CHOSEN_ALPHA),
                true => (draw::SELECT, CHOSEN_ALPHA),
                false => (draw::PANEL, RING_ALPHA),
            };
            tint(frame, x, y, color, alpha);
        }
    }

    // The names, each in its box, half way across the ring.
    let mut g = Grid::new(cols, rows);
    let middle = (inner + outer) / 2.0;
    for (i, name) in view.items.iter().enumerate() {
        let angle = i as f32 / count as f32 * std::f32::consts::TAU;
        let name: String = if name.is_empty() { (i + 1).to_string() } else { name.chars().take(NAME_COLS).collect() };
        let len = name.chars().count();
        let (px, py) = (cx + middle * angle.sin(), cy - middle * angle.cos());
        let col = ((px - layout.x as f32) / 8.0 - len as f32 / 2.0).round().clamp(0.0, (cols - len) as f32) as usize;
        let row = ((py - layout.y as f32) / layout.cell_h as f32 - 0.5).round().clamp(0.0, (rows - 1) as f32) as usize;
        let chosen = view.selected == Some(i);
        let back = if chosen { draw::SELECT } else { draw::FIELD };
        // The box: the name's cells and a margin around them.
        let left = (layout.x + col * 8).saturating_sub(PAD_X);
        let top = (layout.y + row * layout.cell_h).saturating_sub(PAD_Y);
        let right = (layout.x + (col + len) * 8 + PAD_X).min(width);
        let bottom = (layout.y + (row + 1) * layout.cell_h + PAD_Y).min(height);
        for y in top..bottom {
            for x in left..right {
                let edge = y == top || y + 1 == bottom || x == left || x + 1 == right;
                let (color, alpha) = if edge { (if chosen { draw::BRIGHT } else { draw::BORDER }, 256) } else { (back, 256) };
                tint(frame, x, y, color, alpha);
            }
        }
        g.background(col, row, len, back);
        g.text(col, row, &name, if chosen { draw::BRIGHT } else { draw::TEXT });
        draw::render_area(&g, &layout, frame, 256, (col..col + len, row..row + 1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_slices_are_the_mappers() {
        assert_eq!(item_at(0.0, -1.0, 4), 0, "the top");
        assert_eq!(item_at(1.0, 0.0, 4), 1, "the right");
        assert_eq!(item_at(0.0, 1.0, 4), 2);
        assert_eq!(item_at(-1.0, -0.1, 4), 3);
    }
}
