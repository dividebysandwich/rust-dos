//! What the screen shows on top of the video card's picture: the blinking
//! text cursor and the mouse pointer, which the front ends draw over each
//! frame (see `render_screen`).

use super::{Frame, VideoMode};
use crate::bus::Bus;

/// Draw the text mode cursor, if `cursor_visible` (the blink phase), and the
/// mouse pointer while the driver shows it over `frame`, which is the
/// picture of `bus`'s current video mode.
pub fn draw_cursors(frame: &mut Frame, bus: &Bus, cursor_visible: bool) {
    let (width, height) = (frame.width, frame.height);
    let buffer = &mut frame.rgb[..];
    let frame_w = width as usize;

    // Draw the Cursor (Overlay)
    // Only draw the hardware cursor in Text Modes!
    let current_mode = bus.video_mode;
    let is_text_mode = matches!(
        current_mode,
        VideoMode::Text80x25
            | VideoMode::Text80x25Color
            | VideoMode::Text40x25
            | VideoMode::Text40x25Color
    );
    if is_text_mode {
        // Read Cursor Position from BDA
        let cursor_col = bus.read_8(0x0450) as usize;
        let cursor_row = bus.read_8(0x0451) as usize;

        // Read Cursor Shape from BDA
        let cursor_shape = bus.read_16(0x0460);
        let start_scan = (cursor_shape >> 8) as u8;
        let end_scan = (cursor_shape & 0xFF) as u8;

        // Bit 5 of Start Scanline indicates "Invisible" in VGA hardware
        let is_hidden = (start_scan & 0x20) != 0;

        // Determine Cell Width based on Mode
        // 40-col modes have 16px wide characters (scaled 2x)
        let (cell_width, max_cols) = match current_mode {
            VideoMode::Text40x25 | VideoMode::Text40x25Color => (16, 40),
            _ => (8, 80),
        };
        // Cell height and visible rows come from BDA so 80x43 / 80x50
        // modes draw the cursor at the correct Y when programs like
        // Norton Commander load the 8x8 font.
        let cell_height = bus.read_16(0x0485) as usize;
        let cell_height = if cell_height == 0 { 16 } else { cell_height };
        let total_rows = bus.read_8(0x0484) as usize + 1;

        if cursor_visible
            && !is_hidden
            && cursor_col < max_cols
            && cursor_row < total_rows
        {
            // Calculate screen coordinates
            let start_x = cursor_col * cell_width;
            let start_y = cursor_row * cell_height;

            // Clamp scanlines to the active cell height - 1.
            let max_scan = cell_height.saturating_sub(1) as u8;
            let scan_start = (start_scan & 0x1F).min(max_scan) as usize;
            let scan_end = end_scan.min(max_scan) as usize;

            if scan_start <= scan_end {
                for y_off in scan_start..=scan_end {
                    for x_off in 0..cell_width {
                        let draw_x = start_x + x_off;
                        let draw_y = start_y + y_off;

                        // Safety Check
                        let idx = (draw_y * frame_w + draw_x) * 3;
                        if idx + 2 < buffer.len() {
                            buffer[idx] = 0xDD;
                            buffer[idx + 1] = 0xDD;
                            buffer[idx + 2] = 0xDD;
                        }
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
