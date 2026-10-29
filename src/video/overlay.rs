//! What the screen shows on top of the video card's picture: the blinking
//! text cursor and the mouse pointer (the mouse driver's, or the cursor
//! of the built-in Windows), which the front ends draw over each frame
//! (see `render_screen`).

use super::Frame;
use crate::bus::Bus;
use crate::win32::user32::cursor::{self, CursorImage, PIXEL_SET, PIXEL_XOR};

/// Draw the text mode cursor, if `cursor_visible` (the blink phase), the
/// mouse pointer while the driver shows it and the built-in Windows'
/// cursor while Windows shows it over `frame`, which is the picture of
/// `bus`'s current video mode.
pub fn draw_cursors(frame: &mut Frame, bus: &Bus, cursor_visible: bool) {
    if let Some((image, rect)) = windows_cursor(frame, bus) {
        draw_windows_cursor(frame, image, rect);
    }
    // The 3dfx card's picture has neither of the others.
    if bus.voodoo_output() {
        return;
    }
    let width = frame.width as usize;
    if let Some((columns, rows)) = text_cursor(frame, bus, cursor_visible) {
        for y in rows {
            for x in columns.clone() {
                let at = (y * width + x) * 4;
                frame.mark_overlay(x, y);
                frame.rgba[at..at + 3].fill(0xDD);
            }
        }
    }
    if let Some((x, y)) = mouse_origin(frame, bus) {
        draw_default_mouse_cursor(frame, x, y);
    }
}

/// Rows that drawing the cursors may touch. Use the same clipped geometry
/// as drawing, so their old rows can be restored after a move or a blink.
pub fn cursor_rows(frame: &Frame, bus: &Bus, cursor_visible: bool) -> std::ops::Range<usize> {
    let mut rows = 0..0;
    if let Some((_, r)) = windows_cursor(frame, bus) {
        let h = frame.height as i32;
        rows = r.y.clamp(0, h) as usize..(r.y + r.h).clamp(0, h) as usize;
    }
    if bus.voodoo_output() {
        return rows;
    }
    if let Some((_, text)) = text_cursor(frame, bus, cursor_visible) {
        rows = super::compose::union(rows, text);
    }
    if let Some((x, y)) = mouse_origin(frame, bus) {
        let (w, h) = (frame.width as i32, frame.height as i32);
        if x < w && x + 16 > 0 {
            let first = y.clamp(0, h) as usize;
            let end = (y + CURSOR_ARROW.len() as i32).clamp(0, h) as usize;
            rows = super::compose::union(rows, first..end);
        }
    }
    rows
}

/// Where the built-in Windows' cursor goes on `frame`: its top left
/// corner, its height and how many frame pixels across and down a pixel
/// of it takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CursorRect {
    x: i32,
    y: i32,
    h: i32,
    scale: (i32, i32),
}

/// The cursor of the built-in Windows, if the screen shows one: its
/// picture and where its hot spot is, in the screen's pixels.
/// Windows draws it on the desktop and over DirectDraw's modes, which a
/// Voodoo3 or Banshee shows in its extended modes; Glide's picture on a
/// 3dfx card has none.
fn shown_windows_cursor(bus: &Bus) -> Option<(&CursorImage, (i32, i32))> {
    let glide = bus.banshee().is_none() || bus.win32.glide_open();
    if bus.voodoo_output() && glide {
        return None;
    }
    cursor::shown(&bus.win32)
}

/// The cursor of the built-in Windows, if it shows one, and where it
/// goes on `frame`. Its place and its pixels are in the pixels of
/// Windows' screen (the desktop's or DirectDraw's mode), which the frame
/// may show larger: a 320x200 mode twice as high, or twice as wide too.
fn windows_cursor<'a>(frame: &Frame, bus: &'a Bus) -> Option<(&'a CursorImage, CursorRect)> {
    let (image, (x, y)) = shown_windows_cursor(bus)?;
    let (screen_w, screen_h) = cursor::screen_size(&bus.win32);
    let (screen_w, screen_h) = (screen_w.max(1), screen_h.max(1));
    let (frame_w, frame_h) = (frame.width as i32, frame.height as i32);
    let scale = ((frame_w / screen_w).max(1), (frame_h / screen_h).max(1));
    let x = (x as i64 * frame_w as i64 / screen_w as i64) as i32;
    let y = (y as i64 * frame_h as i64 / screen_h as i64) as i32;
    let rect = CursorRect {
        x: x - image.hotspot.0 * scale.0,
        y: y - image.hotspot.1 * scale.1,
        h: image.height * scale.1,
        scale,
    };
    Some((image, rect))
}

/// Draw the built-in Windows' cursor `image` at `rect`: a pixel of it
/// sets the frame's or XORs it with its color.
fn draw_windows_cursor(frame: &mut Frame, image: &CursorImage, rect: CursorRect) {
    let (w, h) = (frame.width as i32, frame.height as i32);
    let (sx, sy) = rect.scale;
    for y in 0..rect.h {
        let fy = rect.y + y;
        if !(0..h).contains(&fy) {
            continue;
        }
        for x in 0..image.width * sx {
            let fx = rect.x + x;
            if !(0..w).contains(&fx) {
                continue;
            }
            let p = image.pixel(x / sx, y / sy);
            let rgb = [p as u8, (p >> 8) as u8, (p >> 16) as u8];
            let at = (fy * w + fx) as usize * 4;
            let to = &mut frame.rgba[at..at + 3];
            match p & 0xFF00_0000 {
                PIXEL_SET => to.copy_from_slice(&rgb),
                PIXEL_XOR => to.iter_mut().zip(rgb).for_each(|(t, c)| *t ^= c),
                _ => {}
            }
        }
    }
}

fn mouse_origin(frame: &Frame, bus: &Bus) -> Option<(i32, i32)> {
    if !bus.mouse.installed || bus.mouse.hide_counter > 0 {
        return None;
    }
    let (virt_w, virt_h) = bus.mouse.virtual_screen(bus);
    Some((
        (bus.mouse.x as i64 * frame.width as i64 / virt_w as i64) as i32,
        (bus.mouse.y as i64 * frame.height as i64 / virt_h as i64) as i32,
    ))
}

// The CRTC's shape is in font scanlines. The BIOS data area keeps the
// requested CGA shape where the card has no EGA/VGA cursor registers.
fn text_cursor(
    frame: &Frame,
    bus: &Bus,
    visible: bool,
) -> Option<(std::ops::Range<usize>, std::ops::Range<usize>)> {
    if !visible {
        return None;
    }
    let geometry = super::text::geometry(bus)?;
    let (col, row, shape) = if bus.boot.is_some() && bus.vga.adapter.ega_bios() {
        crtc_cursor(bus, geometry.cols)
    } else {
        let page = bus.read_8(0x0462).min(7) as usize;
        let shape = if bus.vga.adapter.ega_bios() {
            crtc_shape(bus)
        } else {
            bus.read_16(0x0460)
        };
        (
            bus.read_8(0x0450 + page * 2) as usize,
            bus.read_8(0x0451 + page * 2) as usize,
            shape,
        )
    };
    let start = (shape >> 8) as u8;
    if start & 0x20 != 0 || col >= geometry.cols || row >= geometry.rows {
        return None;
    }
    let max_scan = geometry.font_h.saturating_sub(1) as u8;
    let first = (start & 0x1F).min(max_scan) as usize;
    let end = (shape as u8).min(max_scan) as usize;
    let x = col * geometry.cell_w();
    let y = row * geometry.cell_h();
    let columns = x.min(frame.width as usize)..(x + geometry.cell_w()).min(frame.width as usize);
    let rows = (y + first * geometry.y_scale).min(frame.height as usize)
        ..(y + (end + 1) * geometry.y_scale).min(frame.height as usize);
    (!columns.is_empty() && !rows.is_empty()).then_some((columns, rows))
}

/// Whether `draw_cursors` may draw anything (a text cursor, the mouse's,
/// Windows').
pub fn draws_cursors(bus: &Bus) -> bool {
    let dos = super::text::geometry(bus).is_some()
        || (bus.mouse.installed && bus.mouse.hide_counter <= 0);
    (!bus.voodoo_output() && dos) || draws_windows_cursor(bus)
}

/// Whether `draw_cursors` draws the built-in Windows' cursor, which then
/// stands for the host's pointer over the picture.
pub fn draws_windows_cursor(bus: &Bus) -> bool {
    shown_windows_cursor(bus).is_some()
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
    if let Some((image, at)) = shown_windows_cursor(bus) {
        (cursor::screen_size(&bus.win32), at, image).hash(&mut h);
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
            let idx = (y * w + x) as usize * 4;
            frame.mark_overlay(x as usize, y as usize);
            let (r, g, b) = if cell == 1 {
                (0xFF, 0xFF, 0xFF)
            } else {
                (0x00, 0x00, 0x00)
            };
            frame.rgba[idx] = r;
            frame.rgba[idx + 1] = g;
            frame.rgba[idx + 2] = b;
        }
    }
}
