//! What the screen shows on top of the video card's picture: the blinking
//! text cursor and the mouse pointer, which the front ends draw over each
//! frame (see `render_screen`).

use super::Frame;
use crate::bus::Bus;

/// Draw the text mode cursor, if `cursor_visible` (the blink phase), and the
/// mouse pointer while the driver shows it over `frame`, which is the
/// picture of `bus`'s current video mode.
pub fn draw_cursors(frame: &mut Frame, bus: &Bus, cursor_visible: bool) {
    let (width, height) = (frame.width, frame.height);
    let buffer = &mut frame.rgb[..];
    let frame_w = width as usize;

    // The text mode cursor, at the active page's cursor position (BDA
    // 0450h, two bytes a page) and with the shape in BDA 0460h, whose
    // scanlines count in the font's rows.
    if let Some(geometry) = super::text::geometry(bus) {
        let page = bus.read_8(0x0462).min(7) as usize;
        let cursor_col = bus.read_8(0x0450 + page * 2) as usize;
        let cursor_row = bus.read_8(0x0451 + page * 2) as usize;
        let cursor_shape = bus.read_16(0x0460);
        let start_scan = (cursor_shape >> 8) as u8;
        let end_scan = (cursor_shape & 0xFF) as u8;
        // Bit 5 of Start Scanline indicates "Invisible" in VGA hardware
        let is_hidden = (start_scan & 0x20) != 0;

        if cursor_visible && !is_hidden && cursor_col < geometry.cols && cursor_row < geometry.rows {
            let (cell_w, cell_h) = (geometry.cell_w(), geometry.cell_h());
            let max_scan = geometry.font_h.saturating_sub(1) as u8;
            let scan_start = (start_scan & 0x1F).min(max_scan) as usize;
            let scan_end = end_scan.min(max_scan) as usize;
            let x0 = cursor_col * cell_w;
            for y in scan_start * geometry.y_scale..(scan_end + 1) * geometry.y_scale {
                let draw_y = cursor_row * cell_h + y;
                for draw_x in x0..x0 + cell_w {
                    let idx = (draw_y * frame_w + draw_x) * 3;
                    if draw_x < frame_w && idx + 2 < buffer.len() {
                        buffer[idx..idx + 3].fill(0xDD);
                    }
                }
            }
        }
    }

    // Draw Mouse Cursor (software overlay) when visible and installed.
    // The driver stores the cursor in virtual coords; map those to
    // screen pixels over the same virtual extent the host pointer spans.
    if bus.mouse.installed && bus.mouse.hide_counter <= 0 {
        let (virt_w, virt_h) = bus.mouse.virtual_extent(bus.display_size());
        let sx = (bus.mouse.x as i64 * width as i64 / virt_w as i64) as i32;
        let sy = (bus.mouse.y as i64 * height as i64 / virt_h as i64) as i32;
        draw_default_mouse_cursor(frame, sx, sy);
    }
}

/// Convert mouse coordinates in the picture's pixels (`frame`'s, as the
/// front end's window or canvas shows it) into the driver's virtual
/// coordinate system (see `MouseState::virtual_extent`).
pub fn frame_to_mouse(bus: &Bus, frame: &Frame, (x, y): (i32, i32)) -> (i32, i32) {
    let (w, h) = (frame.width as i32, frame.height as i32);
    let px = x.clamp(0, w - 1);
    let py = y.clamp(0, h - 1);

    let (virt_w, virt_h) = bus.mouse.virtual_extent(bus.display_size());

    let vx = (px as i64 * virt_w as i64 / w as i64) as i32;
    let vy = (py as i64 * virt_h as i64 / h as i64) as i32;
    (vx.clamp(0, virt_w - 1), vy.clamp(0, virt_h - 1))
}

/// Classic Microsoft-style arrow cursor as a 16x16 bitmap. 1 = white pixel,
/// 2 = black outline, 0 = transparent. Hotspot is (0,0).
const CURSOR_ARROW: [[u8; 16]; 16] = [
    [2,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
    [2,2,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
    [2,1,2,0,0,0,0,0,0,0,0,0,0,0,0,0],
    [2,1,1,2,0,0,0,0,0,0,0,0,0,0,0,0],
    [2,1,1,1,2,0,0,0,0,0,0,0,0,0,0,0],
    [2,1,1,1,1,2,0,0,0,0,0,0,0,0,0,0],
    [2,1,1,1,1,1,2,0,0,0,0,0,0,0,0,0],
    [2,1,1,1,1,1,1,2,0,0,0,0,0,0,0,0],
    [2,1,1,1,1,1,1,1,2,0,0,0,0,0,0,0],
    [2,1,1,1,1,1,2,2,2,2,0,0,0,0,0,0],
    [2,1,1,2,1,1,2,0,0,0,0,0,0,0,0,0],
    [2,1,2,0,2,1,1,2,0,0,0,0,0,0,0,0],
    [2,2,0,0,2,1,1,2,0,0,0,0,0,0,0,0],
    [0,0,0,0,0,2,1,1,2,0,0,0,0,0,0,0],
    [0,0,0,0,0,2,1,1,2,0,0,0,0,0,0,0],
    [0,0,0,0,0,0,2,2,2,0,0,0,0,0,0,0],
];

fn draw_default_mouse_cursor(frame: &mut Frame, origin_x: i32, origin_y: i32) {
    let (w, h) = (frame.width as i32, frame.height as i32);
    for (row_idx, row) in CURSOR_ARROW.iter().enumerate() {
        for (col_idx, &cell) in row.iter().enumerate() {
            if cell == 0 {
                continue;
            }
            let x = origin_x + col_idx as i32;
            let y = origin_y + row_idx as i32;
            if x < 0 || y < 0 || x >= w || y >= h {
                continue;
            }
            let idx = (y * w + x) as usize * 3;
            let (r, g, b) = if cell == 1 {
                (0xFF, 0xFF, 0xFF)
            } else {
                (0x00, 0x00, 0x00)
            };
            frame.rgb[idx] = r;
            frame.rgb[idx + 1] = g;
            frame.rgb[idx + 2] = b;
        }
    }
}
