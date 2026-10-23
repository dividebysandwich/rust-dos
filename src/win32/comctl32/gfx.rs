//! Drawing for the controls, on GDI32's and USER32's own functions:
//! rectangles in brushes and colors, frames, edges, lines and text.

use crate::cpu::Cpu;
use crate::win32::gdi32::{self, Rect};
use crate::win32::user32::{paint, sysparams, text};

pub const CLR_NONE: u32 = 0xFFFF_FFFF;
pub const CLR_DEFAULT: u32 = 0xFF00_0000;

pub const COLOR_SCROLLBAR: u32 = 0;
pub const COLOR_WINDOW: u32 = 5;
pub const COLOR_WINDOWFRAME: u32 = 6;
pub const COLOR_WINDOWTEXT: u32 = 8;
pub const COLOR_HIGHLIGHT: u32 = 13;
pub const COLOR_HIGHLIGHTTEXT: u32 = 14;
pub const COLOR_BTNFACE: u32 = 15;
pub const COLOR_BTNSHADOW: u32 = 16;
pub const COLOR_GRAYTEXT: u32 = 17;
pub const COLOR_BTNTEXT: u32 = 18;
pub const COLOR_BTNHIGHLIGHT: u32 = 20;
pub const COLOR_3DDKSHADOW: u32 = 21;
pub const COLOR_3DLIGHT: u32 = 22;
pub const COLOR_INFOTEXT: u32 = 23;
pub const COLOR_INFOBK: u32 = 24;

/// The system color `index`.
pub fn sys_color(cpu: &mut Cpu, index: u32) -> u32 {
    crate::win32::user32::user(cpu).color(index)
}

/// The system color `index`'s brush.
pub fn sys_brush(cpu: &mut Cpu, index: u32) -> u32 {
    sysparams::brush(cpu, index)
}

/// Fill `r` with `brush`.
pub fn fill(cpu: &mut Cpu, hdc: u32, r: Rect, brush: u32) {
    paint::fill(cpu, hdc, r, brush);
}

/// Fill `r` with the color `color` (a COLORREF).
pub fn fill_rgb(cpu: &mut Cpu, hdc: u32, r: Rect, color: u32) {
    let brush = gdi32::create_solid_brush(cpu, &[color]).unwrap_or(0);
    if brush != 0 {
        paint::fill(cpu, hdc, r, brush);
        gdi32::delete_object(cpu, &[brush]);
    }
}

/// Fill `r` with the system color `index`.
pub fn fill_sys(cpu: &mut Cpu, hdc: u32, r: Rect, index: u32) {
    paint::fill_color(cpu, hdc, r, index);
}

/// A frame a unit wide inside `r`, in `brush` (FrameRect).
pub fn frame(cpu: &mut Cpu, hdc: u32, r: Rect, brush: u32) {
    if r.is_empty() {
        return;
    }
    for side in [
        Rect::new(r.left, r.top, r.right, r.top + 1),
        Rect::new(r.left, r.bottom - 1, r.right, r.bottom),
        Rect::new(r.left, r.top, r.left + 1, r.bottom),
        Rect::new(r.right - 1, r.top, r.right, r.bottom),
    ] {
        paint::fill(cpu, hdc, side, brush);
    }
}

/// DrawEdge(hdc, r, kind, flags): the rectangle inside the edge.
pub fn edge(cpu: &mut Cpu, hdc: u32, r: Rect, kind: u32, flags: u32) -> Rect {
    paint::edge(cpu, hdc, r, kind, flags).1
}

/// DrawTextA's drawing of `text` in `r`: the height drawn. `r` gets
/// the rectangle DT_CALCRECT measures.
pub fn draw_text(cpu: &mut Cpu, hdc: u32, text: &[u8], r: &mut Rect, flags: u32) -> i32 {
    text::draw_text_bytes(cpu, hdc, text, r, flags)
}

/// The size of `text` in the DC's font.
pub fn text_size(cpu: &mut Cpu, hdc: u32, text: &[u8]) -> (i32, i32) {
    gdi32::font::text_extent(cpu, hdc, text).unwrap_or((0, 0))
}

/// SelectObject(hdc, object): the one before.
pub fn select(cpu: &mut Cpu, hdc: u32, object: u32) -> u32 {
    gdi32::dc::select_object(cpu, &[hdc, object]).unwrap_or(0)
}

pub fn set_text_color(cpu: &mut Cpu, hdc: u32, color: u32) -> u32 {
    gdi32::dc::set_text_color(cpu, &[hdc, color]).unwrap_or(0)
}

pub fn set_bk_color(cpu: &mut Cpu, hdc: u32, color: u32) -> u32 {
    gdi32::dc::set_bk_color(cpu, &[hdc, color]).unwrap_or(0)
}

pub fn set_bk_mode(cpu: &mut Cpu, hdc: u32, mode: u32) -> u32 {
    gdi32::dc::set_bk_mode(cpu, &[hdc, mode]).unwrap_or(0)
}

/// A line from (x0, y0) to (x1, y1), its last point left out, in the
/// DC's pen.
pub fn line(cpu: &mut Cpu, hdc: u32, (x0, y0): (i32, i32), (x1, y1): (i32, i32)) {
    gdi32::draw::move_to_ex(cpu, &[hdc, x0 as u32, y0 as u32, 0]);
    gdi32::draw::line_to(cpu, &[hdc, x1 as u32, y1 as u32]);
}

/// The stock object `index` (GetStockObject).
pub fn stock(cpu: &mut Cpu, index: u32) -> u32 {
    gdi32::get_stock_object(cpu, &[index]).unwrap_or(0)
}

/// The font controls use when they are given none: the system's.
pub fn default_font(cpu: &mut Cpu) -> u32 {
    // DEFAULT_GUI_FONT, as the controls of COMCTL32 4.70 take it.
    stock(cpu, 17)
}

/// The height of the DC's font, with its leading (tmHeight).
pub fn font_height(cpu: &mut Cpu, hdc: u32) -> i32 {
    gdi32::font::metrics(cpu, hdc).map_or(13, |m| m.height)
}

/// MulDiv(a, b, c), rounded as Windows rounds it; -1 for c of 0.
pub fn mul_div(a: i32, b: i32, c: i32) -> i32 {
    if c == 0 {
        return -1;
    }
    let p = a as i64 * b as i64;
    let c = c as i64;
    let q = if (p < 0) != (c < 0) {
        (p - c / 2) / c
    } else {
        (p + c / 2) / c
    };
    q as i32
}

/// DrawFocusRect(hdc, r): the dotted frame of a focused item, XORed
/// onto the logical rectangle `r`.
pub fn focus_rect(cpu: &mut Cpu, hdc: u32, r: Rect) {
    let Some(d) = gdi32::gdi(cpu)
        .dc(hdc)
        .map(|dc| dc.rect_to_device(&r).ordered())
    else {
        return;
    };
    gdi32::draw::focus_device_rect(cpu, hdc, d);
}
