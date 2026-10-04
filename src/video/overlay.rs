//! What the screen shows on top of the video card's picture: the blinking
//! text cursor and the mouse pointer, which the front ends draw over each
//! frame (see `render_screen`).

use super::Frame;
use crate::bus::Bus;

/// Draw the text mode cursor, if `cursor_visible` (the blink phase), and the
/// mouse pointer while the driver shows it over `frame`, which is the
/// picture of `bus`'s current video mode.
pub fn draw_cursors(frame: &mut Frame, bus: &Bus, cursor_visible: bool) {
    // The 3dfx card's picture has neither.
    if bus.voodoo_output() {
        return;
    }
    let (width, height) = (frame.width, frame.height);
    let buffer = &mut frame.rgb[..];
    let frame_w = width as usize;

    // The text mode cursor, at the active page's cursor position (BDA
    // 0450h, two bytes a page). Its shape is the CRTC's on an EGA or VGA
    // (Cursor Start and End, 0Ah and 0Bh), in the font's lines: BDA 0460h
    // keeps the shape a program asked for, which the BIOS turns from a
    // CGA's 8 lines into the font's (0607h is 0D0Eh in a 16-line font).
    // The CGA's is in its 8 lines.
    if let Some(geometry) = super::text::geometry(bus) {
        let (cursor_col, cursor_row, cursor_shape) = if bus.boot.is_some() && bus.vga.adapter.ega_bios() {
            crtc_cursor(bus, geometry.cols)
        } else {
            let page = bus.read_8(0x0462).min(7) as usize;
            let shape = if bus.vga.adapter.ega_bios() { crtc_shape(bus) } else { bus.read_16(0x0460) };
            (bus.read_8(0x0450 + page * 2) as usize, bus.read_8(0x0451 + page * 2) as usize, shape)
        };
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
        let (virt_w, virt_h) = bus.mouse.virtual_screen(bus);
        let sx = (bus.mouse.x as i64 * width as i64 / virt_w as i64) as i32;
        let sy = (bus.mouse.y as i64 * height as i64 / virt_h as i64) as i32;
        draw_default_mouse_cursor(frame, sx, sy);
    }
}

/// Whether `draw_cursors` may draw anything (a text cursor, the mouse's).
pub fn draws_cursors(bus: &Bus) -> bool {
    !bus.voodoo_output() && (super::text::geometry(bus).is_some() || (bus.mouse.installed && bus.mouse.hide_counter <= 0))
}

/// What `draw_cursors` draws from, as one number: where it would draw the
/// same, the number is the same.
pub fn cursors_key(bus: &Bus, cursor_visible: bool) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bus.voodoo_output().hash(&mut h);
    if let Some(g) = super::text::geometry(bus) {
        (g.cols, g.rows, g.font_h, g.x_scale, g.y_scale, g.cell_w(), g.cell_h()).hash(&mut h);
        if bus.boot.is_some() && bus.vga.adapter.ega_bios() {
            crtc_cursor(bus, g.cols).hash(&mut h);
        } else {
            let page = bus.read_8(0x0462).min(7) as usize;
            let shape = if bus.vga.adapter.ega_bios() { crtc_shape(bus) } else { bus.read_16(0x0460) };
            (bus.read_8(0x0450 + page * 2), bus.read_8(0x0451 + page * 2), shape).hash(&mut h);
        }
        cursor_visible.hash(&mut h);
    }
    if bus.mouse.installed && bus.mouse.hide_counter <= 0 {
        (bus.mouse.virtual_screen(bus), bus.mouse.x, bus.mouse.y).hash(&mut h);
    }
    h.finish()
}

/// The cursor as the CRTC has it on a booted machine, whose BIOS data area
/// may not be the one the machine on the screen has (a DOS box's under
/// Windows): its column and row from the Cursor Location registers (0Eh,
/// 0Fh) less the Start Address (0Ch, 0Dh), and its shape (0Ah, 0Bh) as BDA
/// 0460h keeps it.
fn crtc_cursor(bus: &Bus, cols: usize) -> (usize, usize, u16) {
    let crtc = &bus.vga.crtc_regs;
    let location = (crtc[0x0E] as usize) << 8 | crtc[0x0F] as usize;
    let start = (crtc[0x0C] as usize) << 8 | crtc[0x0D] as usize;
    let offset = location.wrapping_sub(start) & 0x3FFF;
    let cols = cols.max(1);
    (offset % cols, offset / cols, crtc_shape(bus))
}

/// The cursor's shape in the CRTC's Cursor Start and End (0Ah, 0Bh), as BDA
/// 0460h keeps one.
fn crtc_shape(bus: &Bus) -> u16 {
    let crtc = &bus.vga.crtc_regs;
    (crtc[0x0A] as u16) << 8 | (crtc[0x0B] & 0x1F) as u16
}

/// Convert mouse coordinates in the picture's pixels (`frame`'s, as the
/// front end's window or canvas shows it) into the driver's virtual
/// coordinate system (see `MouseState::virtual_extent`).
pub fn frame_to_mouse(bus: &Bus, frame: &Frame, (x, y): (i32, i32)) -> (i32, i32) {
    let (w, h) = (frame.width as i32, frame.height as i32);
    let px = x.clamp(0, w - 1);
    let py = y.clamp(0, h - 1);

    let (virt_w, virt_h) = bus.mouse.virtual_screen(bus);

    let vx = (px as i64 * virt_w as i64 / w as i64) as i32;
    let vy = (py as i64 * virt_h as i64 / h as i64) as i32;
    (vx.clamp(0, virt_w - 1), vy.clamp(0, virt_h - 1))
}

/// Where the driver's virtual pixel (`x`, `y`) is on the picture, in
/// frame pixels: the middle of it (see `frame_to_mouse`).
pub fn mouse_to_frame(bus: &Bus, frame: &Frame, (x, y): (i32, i32)) -> (f64, f64) {
    let (virt_w, virt_h) = bus.mouse.virtual_screen(bus);
    let fx = (x as f64 + 0.5) * frame.width as f64 / virt_w.max(1) as f64;
    let fy = (y as f64 + 0.5) * frame.height as f64 / virt_h.max(1) as f64;
    (fx.clamp(0.0, frame.width as f64 - 0.5), fy.clamp(0.0, frame.height as f64 - 0.5))
}

/// A motion of (`dx`, `dy`) frame pixels in the mouse driver's virtual
/// pixels (see `frame_to_mouse`).
pub fn frame_motion_to_mouse(bus: &Bus, frame: &Frame, (dx, dy): (f64, f64)) -> (f64, f64) {
    let (virt_w, virt_h) = bus.mouse.virtual_screen(bus);
    (dx * virt_w as f64 / frame.width.max(1) as f64, dy * virt_h as f64 / frame.height.max(1) as f64)
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
