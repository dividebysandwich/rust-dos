//! Text: DrawTextA and the other functions that lay text out, wsprintfA,
//! and the character functions of the ANSI code page.

use super::{FALSE, TRUE, bytes_at, read_words, user, write_words};
use crate::cpu::Cpu;
use crate::win32::convert::{CodePage, decompose};
use crate::win32::gdi32::{self, Rect, font};
use crate::win32::task::{self, Next, Task};

pub const DT_CENTER: u32 = 0x0001;
pub const DT_RIGHT: u32 = 0x0002;
pub const DT_VCENTER: u32 = 0x0004;
pub const DT_BOTTOM: u32 = 0x0008;
pub const DT_WORDBREAK: u32 = 0x0010;
pub const DT_SINGLELINE: u32 = 0x0020;
pub const DT_EXPANDTABS: u32 = 0x0040;
pub const DT_TABSTOP: u32 = 0x0080;
pub const DT_NOCLIP: u32 = 0x0100;
pub const DT_EXTERNALLEADING: u32 = 0x0200;
pub const DT_CALCRECT: u32 = 0x0400;
pub const DT_NOPREFIX: u32 = 0x0800;
pub const DT_EDITCONTROL: u32 = 0x2000;
pub const DT_PATH_ELLIPSIS: u32 = 0x4000;
pub const DT_END_ELLIPSIS: u32 = 0x8000;
pub const DT_MODIFYSTRING: u32 = 0x1_0000;
pub const DT_WORD_ELLIPSIS: u32 = 0x4_0000;

const ETO_OPAQUE: u32 = 2;
const ETO_CLIPPED: u32 = 4;

/// The text tasks' place in `task::TASKS`.
const TASK_TEXT: u16 = 17;

/// The width of `text` on the DC, in its logical units.
fn width_of(cpu: &mut Cpu, hdc: u32, text: &[u8]) -> i32 {
    font::text_extent(cpu, hdc, text).map_or(0, |s| s.0)
}

/// A line DrawTextA lays out: its characters (prefixes taken out), where
/// the prefixed character is, and its width.
struct Line {
    chars: Vec<u8>,
    prefix: Option<usize>,
    width: i32,
}

/// The width of `chars` with their tabs expanded to `tab` units.
fn tabbed_width(cpu: &mut Cpu, hdc: u32, chars: &[u8], expand: bool, tab: i32) -> i32 {
    if !expand || tab <= 0 {
        return width_of(cpu, hdc, chars);
    }
    let mut x = 0;
    let mut segments = chars.split(|&c| c == b'\t').peekable();
    while let Some(seg) = segments.next() {
        x += width_of(cpu, hdc, seg);
        if segments.peek().is_some() {
            x = (x / tab + 1) * tab;
        }
    }
    x
}

/// Take the next line of `text` from `*pos`, as DrawTextA breaks lines:
/// at line ends (unless DT_SINGLELINE) and, with DT_WORDBREAK, where a
/// word doesn't fit in `width`.
fn next_line(
    cpu: &mut Cpu,
    hdc: u32,
    text: &[u8],
    pos: &mut usize,
    flags: u32,
    width: i32,
    tab: i32,
) -> Line {
    let expand = flags & DT_EXPANDTABS != 0;
    let prefixes = flags & DT_NOPREFIX == 0;
    // The line's characters, and for each where it is in `text`.
    let mut chars = Vec::new();
    let mut from = Vec::new();
    let mut prefix = None;
    let mut i = *pos;
    while i < text.len() {
        let c = text[i];
        if (c == b'\r' || c == b'\n') && flags & DT_SINGLELINE == 0 {
            break;
        }
        if c == b'&' && prefixes && i + 1 < text.len() {
            i += 1;
            if text[i] != b'&' && prefix.is_none() {
                prefix = Some(chars.len());
            }
            chars.push(text[i]);
            from.push(i);
            i += 1;
            continue;
        }
        chars.push(c);
        from.push(i);
        i += 1;
    }
    let end = i;
    let mut line_width = tabbed_width(cpu, hdc, &chars, expand, tab);
    let mut next = end;
    if flags & DT_WORDBREAK != 0 && line_width > width && !chars.is_empty() {
        // How many characters fit.
        let mut fit = 0;
        while fit < chars.len() && tabbed_width(cpu, hdc, &chars[..fit + 1], expand, tab) <= width {
            fit += 1;
        }
        let (len, used) = word_break(&chars, fit, flags);
        if len < chars.len() {
            next = from.get(used).copied().unwrap_or(end);
            chars.truncate(len);
            if prefix.is_some_and(|p| p >= len) {
                prefix = None;
            }
            line_width = tabbed_width(cpu, hdc, &chars, expand, tab);
        }
    }
    let ellipsis = flags & (DT_END_ELLIPSIS | DT_PATH_ELLIPSIS | DT_WORD_ELLIPSIS) != 0;
    if ellipsis && line_width > width {
        while !chars.is_empty() {
            chars.pop();
            let mut with = chars.clone();
            with.extend_from_slice(b"...");
            line_width = tabbed_width(cpu, hdc, &with, expand, tab);
            if line_width <= width {
                break;
            }
        }
        chars.extend_from_slice(b"...");
        line_width = tabbed_width(cpu, hdc, &chars, expand, tab);
        if prefix.is_some_and(|p| p >= chars.len() - 3) {
            prefix = None;
        }
    }
    // A line end goes with the line.
    if next == end && end < text.len() {
        if text[end] == b'\r' {
            next += 1;
            if text.get(next) == Some(&b'\n') {
                next += 1;
            }
        } else if text[end] == b'\n' {
            next += 1;
        }
    }
    *pos = next;
    Line {
        chars,
        prefix,
        width: line_width,
    }
}

/// Where a line of `chars` breaks when `fit` of them fit: how many it
/// keeps and how many it uses up, as Windows breaks words.
fn word_break(chars: &[u8], fit: usize, flags: u32) -> (usize, usize) {
    let len = chars.len();
    if fit >= len {
        return (len, len);
    }
    let mut p = fit;
    let word_fits = if fit == 0 {
        true
    } else if chars[p] == b' ' {
        p -= 1;
        true
    } else {
        while p > 0 {
            p -= 1;
            if chars[p] == b' ' {
                break;
            }
        }
        p != 0 || chars[0] == b' '
    };
    if word_fits {
        if flags & (DT_RIGHT | DT_CENTER) == 0 || chars[p] != b' ' {
            p += 1;
        }
        let next_is_space = p < len && chars[p] == b' ';
        return (p, p + next_is_space as usize);
    }
    if flags & (DT_EDITCONTROL | DT_WORD_ELLIPSIS | DT_PATH_ELLIPSIS) == DT_EDITCONTROL {
        return (fit.max(1), fit.max(1));
    }
    // The whole word, which runs past the width.
    let mut p = fit;
    while p < len && chars[p] != b' ' {
        p += 1;
    }
    (p, p + (p < len) as usize)
}

/// Draw the line `line` at (`x`, `y`) on the DC, its tabs expanded.
#[allow(clippy::too_many_arguments)]
fn draw_line(
    cpu: &mut Cpu,
    hdc: u32,
    line: &Line,
    (x, y): (i32, i32),
    flags: u32,
    rect: Rect,
    tab: i32,
    ascent: i32,
) {
    let options = if flags & DT_NOCLIP != 0 {
        0
    } else {
        ETO_CLIPPED
    };
    let clip = (flags & DT_NOCLIP == 0).then_some(rect);
    let expand = flags & DT_EXPANDTABS != 0 && tab > 0;
    let mut xseg = x;
    let mut start = 0;
    let chars = &line.chars;
    loop {
        let end = if expand {
            chars[start..]
                .iter()
                .position(|&c| c == b'\t')
                .map_or(chars.len(), |p| start + p)
        } else {
            chars.len()
        };
        let seg = &chars[start..end];
        font::draw_text(cpu, hdc, (xseg, y), options, Some(rect), seg, None);
        if let Some(p) = line.prefix.filter(|&p| p >= start && p < end) {
            let left = xseg + width_of(cpu, hdc, &chars[start..p]);
            let w = width_of(cpu, hdc, &chars[p..p + 1]);
            underscore(cpu, hdc, left, y + ascent + 1, w, clip);
        }
        if end >= chars.len() {
            break;
        }
        let w = width_of(cpu, hdc, seg);
        xseg += (w / tab + 1) * tab;
        start = end + 1;
    }
}

/// Underline the prefixed character, `w` wide at (`x`, `y`), in the text
/// color.
fn underscore(cpu: &mut Cpu, hdc: u32, x: i32, y: i32, w: i32, clip: Option<Rect>) {
    let Some(dc) = gdi32::gdi(cpu).dc(hdc).cloned() else {
        return;
    };
    if let Some(r) = clip
        && !(y >= r.top && y < r.bottom)
    {
        return;
    }
    let a = dc.to_device(x, y);
    let b = dc.to_device(x + w, y);
    gdi32::draw::device_line(cpu, hdc, a, b, dc.attrs.text_color);
}

/// DrawTextExA's work on the text `text` (None for a null pointer) of
/// `count` characters (-1: to its 0): the height it drew, and the
/// rectangle back with DT_CALCRECT.
fn draw_text_core(
    cpu: &mut Cpu,
    hdc: u32,
    text: Option<Vec<u8>>,
    count: i32,
    rect: &mut Rect,
    flags: u32,
    params: Option<(i32, i32, i32)>,
) -> (i32, u32) {
    let Some(m) = font::metrics(cpu, hdc) else {
        return (0, 0);
    };
    let lh = m.height;
    let mut flags = flags;
    if flags & DT_SINGLELINE != 0 {
        flags &= !DT_WORDBREAK;
    }
    let empty_rect = |rect: &mut Rect, flags: u32| {
        if flags & DT_CALCRECT != 0 {
            rect.right = rect.left;
            rect.bottom = if flags & DT_SINGLELINE != 0 {
                rect.top + lh
            } else {
                rect.top
            };
        }
    };
    let text = match text {
        None if count > 0 => return (0, 0),
        None if count == 0 => return (lh, 0),
        None => {
            empty_rect(rect, flags);
            return (lh, 0);
        }
        Some(t) => t,
    };
    if count == 0 {
        return (lh, 0);
    }
    if text.is_empty() {
        empty_rect(rect, flags);
        return (lh, 0);
    }
    // A mapping mode whose y runs up lays the lines out downwards from
    // the top all the same.
    let dc = gdi32::gdi(cpu).dc(hdc).cloned().unwrap_or_default();
    let invert = (dc.attrs.window_ext.1 > 0) != (dc.attrs.viewport_ext.1 > 0);
    let (lmargin, rmargin, tab_length) = params.unwrap_or((0, 0, 0));
    let mut x = rect.left;
    if flags & (DT_CENTER | DT_RIGHT) == 0 {
        x += lmargin;
    }
    let width = rect.right - rect.left - lmargin - rmargin;
    let tab = if flags & DT_EXPANDTABS != 0 {
        let stops = if flags & DT_TABSTOP != 0 && params.is_some() {
            tab_length
        } else if flags & DT_TABSTOP != 0 {
            ((flags >> 8) & 0xFF) as i32
        } else {
            8
        };
        m.average_width * stops.max(1)
    } else {
        0
    };
    if flags & DT_CALCRECT != 0 {
        flags |= DT_NOCLIP;
    }
    let mut y = rect.top;
    let mut max_width = 0;
    let mut pos = 0;
    let mut drawn = 0u32;
    loop {
        let last = flags & DT_NOCLIP == 0
            && if invert {
                y - lh < rect.bottom
            } else {
                y + lh > rect.bottom
            };
        let before = pos;
        let line = next_line(cpu, hdc, &text, &mut pos, flags, width, tab);
        let lx = if flags & DT_CENTER != 0 {
            (rect.left + rect.right - line.width) / 2
        } else if flags & DT_RIGHT != 0 {
            rect.right - line.width
        } else {
            x
        };
        let mut ly = y;
        if flags & DT_SINGLELINE != 0 {
            if flags & DT_VCENTER != 0 {
                ly = rect.top + (rect.bottom - rect.top) / 2 - lh / 2;
            } else if flags & DT_BOTTOM != 0 {
                ly = rect.bottom - lh;
            }
        }
        if flags & DT_CALCRECT == 0 {
            draw_line(cpu, hdc, &line, (lx, ly), flags, *rect, tab, m.ascent);
        } else {
            max_width = max_width.max(line.width);
        }
        y += if invert { -lh } else { lh };
        drawn += line.chars.len() as u32;
        if pos >= text.len() || last || pos == before {
            break;
        }
    }
    if flags & DT_CALCRECT != 0 {
        rect.right = rect.left + max_width + lmargin + rmargin;
        rect.bottom = y;
    }
    (y - rect.top, drawn)
}

/// Draw `text` in `rect` of the DC by DrawTextA's `flags`, for USER's
/// own painting: the height it drew.
pub fn draw_text_bytes(cpu: &mut Cpu, hdc: u32, text: &[u8], rect: &mut Rect, flags: u32) -> i32 {
    draw_text_core(
        cpu,
        hdc,
        Some(text.to_vec()),
        text.len() as i32,
        rect,
        flags,
        None,
    )
    .0
}

/// The text DrawTextA was given: `count` bytes, or with -1 to its 0.
fn text_arg(cpu: &mut Cpu, at: u32, count: i32) -> Option<Option<Vec<u8>>> {
    if at == 0 {
        return Some(None);
    }
    if count < 0 {
        return Some(Some(bytes_at(cpu, at)?));
    }
    if !cpu.bus.guest_probe(at, count as usize, false) {
        return None;
    }
    let mut b = vec![0u8; count as usize];
    cpu.bus.guest_read_bytes(at, &mut b);
    (!cpu.bus.guest_faulted()).then_some(Some(b))
}

/// DrawTextA(hdc, text, count, rect, flags).
pub fn draw_text(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    draw_text_ex(cpu, &[args[0], args[1], args[2], args[3], args[4], 0])
}

/// DrawTextExA(hdc, text, count, rect, flags, params).
pub fn draw_text_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [hdc, at, count, rect_at, flags, dtp] = args.try_into().unwrap();
    let count = count as i32;
    let params = if dtp != 0 {
        let w = read_words(cpu, dtp, 5);
        if w[0] != 20 {
            return Some(0);
        }
        Some((w[2] as i32, w[3] as i32, w[1] as i32))
    } else {
        None
    };
    let text = text_arg(cpu, at, count)?;
    let mut rect = Rect::read(cpu, rect_at);
    let (height, drawn) = draw_text_core(cpu, hdc, text, count, &mut rect, flags, params);
    if flags & DT_CALCRECT != 0 {
        rect.write(cpu, rect_at);
    }
    if dtp != 0 && height != 0 {
        cpu.bus.guest_write_32(dtp + 16, drawn);
    }
    Some(height as u32)
}

/// TabbedTextOutA's and GetTabbedTextExtentA's work: the width and
/// height of the text with its tabs at the tab stops, from `origin`.
#[allow(clippy::too_many_arguments)]
pub fn tabbed(
    cpu: &mut Cpu,
    hdc: u32,
    (mut x, y): (i32, i32),
    text: &[u8],
    stops: &[i32],
    origin: i32,
    draw: bool,
) -> u32 {
    let start = x;
    let (mut stops, default) = match stops.len() {
        0 => (
            &[][..],
            8 * font::metrics(cpu, hdc).map_or(0, |m| m.average_width),
        ),
        1 => (&[][..], stops[0]),
        _ => (
            stops,
            8 * font::metrics(cpu, hdc).map_or(0, |m| m.average_width),
        ),
    };
    let mut height = font::metrics(cpu, hdc).map_or(0, |m| m.height);
    let mut rest = text;
    let opaque = gdi32::gdi(cpu)
        .dc(hdc)
        .is_some_and(|d| d.attrs.bk_mode == gdi32::dc::OPAQUE);
    while !rest.is_empty() {
        let mut x0 = x;
        let tabs = rest.iter().take_while(|&&c| c == b'\t').count();
        let len = rest[tabs..].iter().take_while(|&&c| c != b'\t').count();
        let seg = &rest[tabs..tabs + len];
        let (cx, cy) = font::text_extent(cpu, hdc, seg).unwrap_or((0, 0));
        height = cy;
        if tabs > 0 {
            let i = tabs;
            let mut placed = false;
            while stops.len() >= i {
                let p = stops[0];
                if origin + p.abs() > x {
                    let stop = stops[i - 1];
                    if stop >= 0 {
                        x0 = origin + stop;
                        x = x0 + cx;
                        placed = true;
                        break;
                    } else if origin - stop - cx > x {
                        x = origin - stop;
                        x0 = x - cx;
                        placed = true;
                        break;
                    }
                }
                stops = &stops[1..];
            }
            if !placed && stops.len() < i {
                if default > 0 {
                    x0 = origin + ((x - origin) / default + i as i32) * default;
                    x = x0 + cx;
                } else if default < 0 {
                    x = origin + ((x - origin + cx) / -default + i as i32) * -default;
                    x0 = x - cx;
                }
            }
        } else {
            x += cx;
        }
        if draw {
            let r = Rect::new(x0, y, x, y + cy);
            let options = if opaque { ETO_OPAQUE } else { 0 };
            font::draw_text(cpu, hdc, (x0, y), options, Some(r), seg, None);
        }
        rest = &rest[tabs + len..];
    }
    super::make_long(x - start, height)
}

fn read_stops(cpu: &mut Cpu, count: u32, at: u32) -> Vec<i32> {
    if at == 0 || count as i32 <= 0 {
        return Vec::new();
    }
    read_words(cpu, at, count.min(0x1000) as usize)
        .into_iter()
        .map(|v| v as i32)
        .collect()
}

/// TabbedTextOutA(hdc, x, y, text, count, stops, positions, origin).
pub fn tabbed_text_out(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [hdc, x, y, at, count, n, stops, origin] = args.try_into().unwrap();
    let text = text_arg(cpu, at, count as i32)?.unwrap_or_default();
    let stops = read_stops(cpu, n, stops);
    Some(tabbed(
        cpu,
        hdc,
        (x as i32, y as i32),
        &text,
        &stops,
        origin as i32,
        true,
    ))
}

/// GetTabbedTextExtentA(hdc, text, count, stops, positions).
pub fn get_tabbed_text_extent(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [hdc, at, count, n, stops] = args.try_into().unwrap();
    let text = text_arg(cpu, at, count as i32)?.unwrap_or_default();
    let stops = read_stops(cpu, n, stops);
    Some(tabbed(cpu, hdc, (0, 0), &text, &stops, 0, false))
}

/// Draw `text` at (`x`, `y`) grayed, as a disabled control's text: in
/// the gray text color.
fn draw_gray(cpu: &mut Cpu, hdc: u32, (x, y): (i32, i32), text: &[u8], prefix: bool) {
    let gray = user(cpu).color(super::sysparams::COLOR_GRAYTEXT);
    let old = gdi32::dc::with(cpu, hdc, |d| {
        std::mem::replace(&mut d.attrs.text_color, gray)
    });
    let mode = gdi32::dc::with(cpu, hdc, |d| {
        std::mem::replace(&mut d.attrs.bk_mode, gdi32::dc::TRANSPARENT)
    });
    let mut r = Rect::new(x, y, x + 0x4000, y + 0x4000);
    let flags = DT_NOCLIP | DT_SINGLELINE | if prefix { 0 } else { DT_NOPREFIX };
    draw_text_core(
        cpu,
        hdc,
        Some(text.to_vec()),
        text.len() as i32,
        &mut r,
        flags,
        None,
    );
    gdi32::dc::with(cpu, hdc, |d| {
        if let Some(c) = old {
            d.attrs.text_color = c;
        }
        if let Some(m) = mode {
            d.attrs.bk_mode = m;
        }
    });
}

/// The kinds of the text tasks.
const GRAY_STRING: u32 = 0;
const DRAW_STATE: u32 = 1;

/// GrayStringA(hdc, brush, proc, data, count, x, y, cx, cy).
pub fn gray_string(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [hdc, _brush, proc, data, count, x, y, _cx, _cy] = args.try_into().unwrap();
    if proc != 0 {
        // The callback draws, on the DC itself.
        return task::start(cpu, TASK_TEXT, vec![GRAY_STRING, proc, hdc, data, count], 0);
    }
    let Some(text) = text_arg(cpu, data, if count == 0 { -1 } else { count as i32 })? else {
        return Some(FALSE);
    };
    draw_gray(cpu, hdc, (x as i32, y as i32), &text, false);
    Some(TRUE)
}

/// The text tasks' steps: a callback that draws.
pub fn step(_cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    match (t.var(0), t.step) {
        (GRAY_STRING, 0) => Next::call(t.var(1), &[t.var(2), t.var(3), t.var(4)], 1),
        (DRAW_STATE, 0) => Next::call(
            t.var(1),
            &[t.var(2), t.var(3), t.var(4), t.var(5), t.var(6)],
            1,
        ),
        _ => Next::Done((value != 0) as u32),
    }
}

const DST_COMPLEX: u32 = 0;
const DST_TEXT: u32 = 1;
const DST_PREFIXTEXT: u32 = 2;
const DST_ICON: u32 = 3;
const DST_BITMAP: u32 = 4;
const DSS_DISABLED: u32 = 0x20;

/// DrawStateA(hdc, brush, proc, lparam, wparam, x, y, cx, cy, flags).
pub fn draw_state(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [hdc, _brush, proc, lp, wp, x, y, cx, cy, flags] = args.try_into().unwrap();
    let (x, y) = (x as i32, y as i32);
    match flags & 0xF {
        DST_TEXT | DST_PREFIXTEXT => {
            if lp == 0 {
                return Some(FALSE);
            }
            let text = text_arg(cpu, lp, if wp == 0 { -1 } else { wp as i32 })?.unwrap_or_default();
            let prefix = flags & 0xF == DST_PREFIXTEXT;
            if flags & DSS_DISABLED != 0 {
                draw_gray(cpu, hdc, (x, y), &text, prefix);
            } else {
                let mut r = Rect::new(x, y, x + 0x4000, y + 0x4000);
                let f = DT_NOCLIP | DT_SINGLELINE | if prefix { 0 } else { DT_NOPREFIX };
                draw_text_core(
                    cpu,
                    hdc,
                    Some(text.clone()),
                    text.len() as i32,
                    &mut r,
                    f,
                    None,
                );
            }
            Some(TRUE)
        }
        DST_ICON => {
            let args = [hdc, x as u32, y as u32, lp, cx, cy, 0, 0, 3];
            super::paint::draw_icon_ex(cpu, &args)
        }
        DST_BITMAP => Some(draw_bitmap(cpu, hdc, lp, (x, y), (cx as i32, cy as i32)) as u32),
        DST_COMPLEX if proc != 0 => task::start(
            cpu,
            TASK_TEXT,
            vec![DRAW_STATE, proc, hdc, lp, wp, cx, cy],
            0,
        ),
        _ => Some(FALSE),
    }
}

/// Draw the bitmap `bitmap` at (`x`, `y`) on the DC: `size` of it, or
/// all of it for 0.
pub fn draw_bitmap(
    cpu: &mut Cpu,
    hdc: u32,
    bitmap: u32,
    (x, y): (i32, i32),
    size: (i32, i32),
) -> bool {
    let Some(b) = gdi32::gdi(cpu).bitmap(bitmap).map(|b| (b.width, b.height)) else {
        return false;
    };
    let (w, h) = if size.0 == 0 || size.1 == 0 { b } else { size };
    let Some(mem) = gdi32::dc::create_compatible_dc(cpu, &[hdc]) else {
        return false;
    };
    let old = gdi32::dc::select_object(cpu, &[mem, bitmap]).unwrap_or(0);
    let args = [
        hdc,
        x as u32,
        y as u32,
        w as u32,
        h as u32,
        mem,
        0,
        0,
        0x00CC_0020,
    ];
    gdi32::draw::bit_blt(cpu, &args);
    if old != 0 {
        gdi32::dc::select_object(cpu, &[mem, old]);
    }
    gdi32::dc::delete_dc(cpu, &[mem]);
    true
}

// wsprintfA.

/// The most wsprintfA writes, with its 0.
const WSPRINTF_MAX: usize = 1024;

/// Format `format` with the arguments at `args`, as wsprintfA (or
/// wsprintfW, `wide`) does: its characters, without the 0.
fn wsprintf_core(cpu: &mut Cpu, format: &[u16], mut args: u32, wide: bool) -> Option<Vec<u16>> {
    let mut out: Vec<u16> = Vec::new();
    let mut i = 0;
    let mut next_arg = |cpu: &mut Cpu| {
        let v = cpu.bus.guest_read_32(args);
        args += 4;
        v
    };
    while i < format.len() && out.len() < WSPRINTF_MAX - 1 {
        let c = format[i];
        i += 1;
        if c != b'%' as u16 {
            out.push(c);
            continue;
        }
        if format.get(i) == Some(&(b'%' as u16)) {
            out.push(c);
            i += 1;
            continue;
        }
        let (mut left, mut zero, mut hex_prefix) = (false, false, false);
        while let Some(&f) = format.get(i) {
            match f {
                0x2D => left = true,
                0x23 => hex_prefix = true,
                0x30 => zero = true,
                _ => break,
            }
            i += 1;
        }
        // A width or precision past what the output holds pads no more
        // than it can.
        let mut width = 0usize;
        while let Some(&d) = format.get(i).filter(|&&d| (0x30..=0x39).contains(&d)) {
            width = (width * 10 + (d - 0x30) as usize).min(WSPRINTF_MAX);
            i += 1;
        }
        let mut precision = None;
        if format.get(i) == Some(&0x2E) {
            i += 1;
            let mut p = 0usize;
            while let Some(&d) = format.get(i).filter(|&&d| (0x30..=0x39).contains(&d)) {
                p = (p * 10 + (d - 0x30) as usize).min(WSPRINTF_MAX);
                i += 1;
            }
            precision = Some(p);
        }
        // The size: 'l' and 'w' make characters and strings wide, 'h'
        // narrow; numbers are 32 bits in any case.
        let mut wide_arg = wide;
        match format.get(i) {
            Some(0x6C | 0x77) => {
                wide_arg = true;
                i += 1;
            }
            Some(0x68) => {
                wide_arg = false;
                i += 1;
            }
            _ => {}
        }
        let Some(&t) = format.get(i) else {
            break;
        };
        i += 1;
        let mut number = false;
        let mut text: Vec<u16> = match t as u8 {
            b'c' => vec![char_arg(next_arg(cpu), wide_arg)],
            b'C' => vec![char_arg(next_arg(cpu), !wide_arg)],
            b's' => {
                let at = next_arg(cpu);
                string_arg(cpu, at, wide_arg)?
            }
            b'S' => {
                let at = next_arg(cpu);
                string_arg(cpu, at, !wide_arg)?
            }
            b'd' | b'i' | b'u' | b'x' | b'X' => {
                number = true;
                let v = next_arg(cpu);
                let mut s = match t as u8 {
                    b'd' | b'i' => (v as i32).unsigned_abs().to_string(),
                    b'u' => v.to_string(),
                    b'x' => format!("{:x}", v),
                    _ => format!("{:X}", v),
                };
                if let Some(p) = precision.filter(|&p| p > s.len()) {
                    s = "0".repeat(p - s.len()) + &s;
                }
                if hex_prefix && matches!(t as u8, b'x' | b'X') {
                    s.insert_str(0, if t == b'x' as u16 { "0x" } else { "0X" });
                }
                if matches!(t as u8, b'd' | b'i') && (v as i32) < 0 {
                    s.insert(0, '-');
                }
                s.encode_utf16().collect()
            }
            _ => vec![t],
        };
        if !number && let Some(p) = precision {
            text.truncate(p);
        }
        if text.len() < width {
            let pad = width - text.len();
            if left {
                text.extend(std::iter::repeat_n(b' ' as u16, pad));
            } else if zero && number {
                let at = (text.first() == Some(&(b'-' as u16))) as usize;
                text.splice(at..at, std::iter::repeat_n(b'0' as u16, pad));
            } else {
                text.splice(0..0, std::iter::repeat_n(b' ' as u16, pad));
            }
        }
        out.extend(text);
    }
    out.truncate(WSPRINTF_MAX - 1);
    Some(out)
}

fn char_arg(v: u32, wide: bool) -> u16 {
    if wide {
        v as u16
    } else {
        CodePage::Ansi.char(v as u8)
    }
}

/// A %s argument's characters: "(null)" for a null pointer.
fn string_arg(cpu: &mut Cpu, at: u32, wide: bool) -> Option<Vec<u16>> {
    if at == 0 {
        return Some("(null)".encode_utf16().collect());
    }
    if wide {
        let mut out = Vec::new();
        let mut p = at;
        loop {
            let c = cpu.bus.guest_read_16(p);
            if cpu.bus.guest_faulted() {
                return None;
            }
            if c == 0 || out.len() >= WSPRINTF_MAX {
                break;
            }
            out.push(c);
            p += 2;
        }
        Some(out)
    } else {
        Some(
            bytes_at(cpu, at)?
                .into_iter()
                .map(|b| CodePage::Ansi.char(b))
                .collect(),
        )
    }
}

/// Write the ANSI result of wsprintfA to `buffer`: its length.
fn put_ansi(cpu: &mut Cpu, buffer: u32, text: &[u16]) -> u32 {
    let mut b: Vec<u8> = text
        .iter()
        .map(|&c| CodePage::Ansi.byte(c).unwrap_or(b'?'))
        .collect();
    let n = b.len() as u32;
    b.push(0);
    cpu.bus.guest_write_bytes(buffer, &b);
    n
}

fn put_wide(cpu: &mut Cpu, buffer: u32, text: &[u16]) -> u32 {
    let mut b: Vec<u8> = text.iter().flat_map(|c| c.to_le_bytes()).collect();
    b.extend([0, 0]);
    cpu.bus.guest_write_bytes(buffer, &b);
    text.len() as u32
}

fn ansi_format(cpu: &mut Cpu, at: u32) -> Option<Vec<u16>> {
    Some(
        bytes_at(cpu, at)?
            .into_iter()
            .map(|b| CodePage::Ansi.char(b))
            .collect(),
    )
}

/// wsprintfA(buffer, format, ...): cdecl, the arguments on the stack.
pub fn wsprintf(cpu: &mut Cpu, _args: &[u32]) -> Option<u32> {
    let sp = cpu.esp();
    let buffer = cpu.bus.guest_read_32(sp + 4);
    let format = cpu.bus.guest_read_32(sp + 8);
    let format = ansi_format(cpu, format)?;
    let text = wsprintf_core(cpu, &format, sp + 12, false)?;
    Some(put_ansi(cpu, buffer, &text))
}

/// wvsprintfA(buffer, format, args).
pub fn wvsprintf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let format = ansi_format(cpu, args[1])?;
    let text = wsprintf_core(cpu, &format, args[2], false)?;
    Some(put_ansi(cpu, args[0], &text))
}

fn wide_format(cpu: &mut Cpu, at: u32) -> Option<Vec<u16>> {
    string_arg(cpu, at, true)
}

/// wsprintfW(buffer, format, ...).
pub fn wsprintf_w(cpu: &mut Cpu, _args: &[u32]) -> Option<u32> {
    let sp = cpu.esp();
    let buffer = cpu.bus.guest_read_32(sp + 4);
    let format = cpu.bus.guest_read_32(sp + 8);
    let format = wide_format(cpu, format)?;
    let text = wsprintf_core(cpu, &format, sp + 12, true)?;
    Some(put_wide(cpu, buffer, &text))
}

/// wvsprintfW(buffer, format, args).
pub fn wvsprintf_w(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let format = wide_format(cpu, args[1])?;
    let text = wsprintf_core(cpu, &format, args[2], true)?;
    Some(put_wide(cpu, args[0], &text))
}

// The character functions.

/// The character of the ANSI byte `b`.
fn ansi_char(b: u8) -> char {
    char::from_u32(CodePage::Ansi.char(b) as u32).unwrap_or('?')
}

/// The ANSI byte of `c`'s upper or lower case, or `b` itself when it has
/// none in the code page.
pub(crate) fn ansi_case(b: u8, upper: bool) -> u8 {
    let c = ansi_char(b);
    let mapped: Vec<char> = if upper {
        c.to_uppercase().collect()
    } else {
        c.to_lowercase().collect()
    };
    match mapped[..] {
        [m] if (m as u32) < 0x1_0000 => CodePage::Ansi.byte(m as u16).unwrap_or(b),
        _ => b,
    }
}

fn wide_case(c: u16, upper: bool) -> u16 {
    let Some(ch) = char::from_u32(c as u32) else {
        return c;
    };
    let mapped: Vec<char> = if upper {
        ch.to_uppercase().collect()
    } else {
        ch.to_lowercase().collect()
    };
    match mapped[..] {
        [m] if (m as u32) < 0x1_0000 => m as u16,
        _ => c,
    }
}

/// CharUpperA and CharLowerA: a character in the low word, or a string
/// changed where it is.
fn case_a(cpu: &mut Cpu, at: u32, upper: bool) -> Option<u32> {
    if at < 0x1_0000 {
        return Some(ansi_case(at as u8, upper) as u32);
    }
    let bytes = bytes_at(cpu, at)?;
    let new: Vec<u8> = bytes.iter().map(|&b| ansi_case(b, upper)).collect();
    cpu.bus.guest_write_bytes(at, &new);
    Some(at)
}

pub fn char_upper(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    case_a(cpu, args[0], true)
}

pub fn char_lower(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    case_a(cpu, args[0], false)
}

fn case_buff_a(cpu: &mut Cpu, at: u32, len: u32, upper: bool) -> Option<u32> {
    if at == 0 {
        return Some(0);
    }
    if !cpu.bus.guest_probe(at, len as usize, true) {
        return None;
    }
    let mut b = vec![0u8; len as usize];
    cpu.bus.guest_read_bytes(at, &mut b);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let new: Vec<u8> = b.iter().map(|&c| ansi_case(c, upper)).collect();
    cpu.bus.guest_write_bytes(at, &new);
    Some(len)
}

pub fn char_upper_buff(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    case_buff_a(cpu, args[0], args[1], true)
}

pub fn char_lower_buff(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    case_buff_a(cpu, args[0], args[1], false)
}

fn case_w(cpu: &mut Cpu, at: u32, upper: bool) -> Option<u32> {
    if at < 0x1_0000 {
        return Some(wide_case(at as u16, upper) as u32);
    }
    let mut p = at;
    loop {
        let c = cpu.bus.guest_read_16(p);
        if cpu.bus.guest_faulted() {
            return None;
        }
        if c == 0 {
            break;
        }
        cpu.bus.guest_write_16(p, wide_case(c, upper));
        p += 2;
    }
    Some(at)
}

pub fn char_upper_w(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    case_w(cpu, args[0], true)
}

pub fn char_lower_w(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    case_w(cpu, args[0], false)
}

fn case_buff_w(cpu: &mut Cpu, at: u32, len: u32, upper: bool) -> Option<u32> {
    if at == 0 {
        return Some(0);
    }
    if !cpu
        .bus
        .guest_probe(at, (len as usize).saturating_mul(2), true)
    {
        return None;
    }
    for i in 0..len {
        let p = at.wrapping_add(i.wrapping_mul(2));
        let c = cpu.bus.guest_read_16(p);
        cpu.bus.guest_write_16(p, wide_case(c, upper));
    }
    (!cpu.bus.guest_faulted()).then_some(len)
}

pub fn char_upper_buff_w(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    case_buff_w(cpu, args[0], args[1], true)
}

pub fn char_lower_buff_w(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    case_buff_w(cpu, args[0], args[1], false)
}

/// CharNextA(p): the next character, or `p` at the string's end.
pub fn char_next(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let p = args[0];
    let c = cpu.bus.guest_read_8(p);
    Some(if c != 0 { p + 1 } else { p })
}

/// CharPrevA(start, p).
pub fn char_prev(_cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (start, p) = (args[0], args[1]);
    Some(if p > start { p - 1 } else { start })
}

/// CharNextExA(codepage, p, flags).
pub fn char_next_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    char_next(cpu, &[args[1]])
}

/// CharPrevExA(codepage, start, p, flags).
pub fn char_prev_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    char_prev(cpu, &[args[1], args[2]])
}

pub fn char_next_w(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let p = args[0];
    let c = cpu.bus.guest_read_16(p);
    Some(if c != 0 { p + 2 } else { p })
}

pub fn char_prev_w(_cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (start, p) = (args[0], args[1]);
    Some(if p > start { p - 2 } else { start })
}

fn test_a(args: &[u32], f: fn(char) -> bool) -> Option<u32> {
    Some(f(ansi_char(args[0] as u8)) as u32)
}

fn test_w(args: &[u32], f: fn(char) -> bool) -> Option<u32> {
    Some(char::from_u32(args[0] & 0xFFFF).is_some_and(f) as u32)
}

pub fn is_char_alpha(_cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    test_a(args, char::is_alphabetic)
}

pub fn is_char_alpha_numeric(_cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    test_a(args, char::is_alphanumeric)
}

pub fn is_char_upper(_cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    test_a(args, char::is_uppercase)
}

pub fn is_char_lower(_cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    test_a(args, char::is_lowercase)
}

pub fn is_char_alpha_w(_cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    test_w(args, char::is_alphabetic)
}

pub fn is_char_alpha_numeric_w(_cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    test_w(args, char::is_alphanumeric)
}

pub fn is_char_upper_w(_cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    test_w(args, char::is_uppercase)
}

pub fn is_char_lower_w(_cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    test_w(args, char::is_lowercase)
}

/// The byte of `c` in `to`, or the nearest it has: the letter without
/// its accent, or a question mark.
fn best_fit(c: u16, to: CodePage) -> u8 {
    if let Some(b) = to.byte(c) {
        return b;
    }
    match decompose(c) {
        Some((base, _)) => to.byte(base).unwrap_or(b'?'),
        None => b'?',
    }
}

fn convert(cpu: &mut Cpu, src: u32, dst: u32, len: Option<u32>, to_oem: bool) -> Option<u32> {
    if src == 0 || dst == 0 {
        return Some(FALSE);
    }
    let bytes = match len {
        Some(n) => {
            if !cpu.bus.guest_probe(src, n as usize, false) {
                return None;
            }
            let mut b = vec![0u8; n as usize];
            cpu.bus.guest_read_bytes(src, &mut b);
            b
        }
        None => {
            let mut b = bytes_at(cpu, src)?;
            b.push(0);
            b
        }
    };
    let (from, to) = if to_oem {
        (CodePage::Ansi, CodePage::Oem)
    } else {
        (CodePage::Oem, CodePage::Ansi)
    };
    let out: Vec<u8> = bytes
        .iter()
        .map(|&b| {
            if b < 0x80 {
                b
            } else {
                best_fit(from.char(b), to)
            }
        })
        .collect();
    cpu.bus.guest_write_bytes(dst, &out);
    Some(TRUE)
}

/// CharToOemA(src, dst).
pub fn char_to_oem(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    convert(cpu, args[0], args[1], None, true)
}

/// OemToCharA(src, dst).
pub fn oem_to_char(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    convert(cpu, args[0], args[1], None, false)
}

/// CharToOemBuffA(src, dst, len).
pub fn char_to_oem_buff(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    convert(cpu, args[0], args[1], Some(args[2]), true)
}

/// OemToCharBuffA(src, dst, len).
pub fn oem_to_char_buff(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    convert(cpu, args[0], args[1], Some(args[2]), false)
}

/// Write `text` to `at` as a nul-terminated string (for tests' helpers).
#[allow(dead_code)]
fn write_words_of(cpu: &mut Cpu, at: u32, words: &[u32]) {
    write_words(cpu, at, words);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_break_at_spaces() {
        // "aaa bbb": 5 characters fit, so the line keeps "aaa " and
        // the space is used up.
        assert_eq!(word_break(b"aaa bbb", 5, 0), (4, 4));
        // A word longer than the line goes whole.
        assert_eq!(word_break(b"aaaaaa bb", 3, 0), (6, 7));
        // An edit control breaks it.
        assert_eq!(word_break(b"aaaaaa bb", 3, DT_EDITCONTROL), (3, 3));
    }

    #[test]
    fn ansi_case_maps_accented_letters() {
        assert_eq!(ansi_case(b'a', true), b'A');
        assert_eq!(ansi_case(0xE9, true), 0xC9);
        assert_eq!(ansi_case(0xFF, true), 0x9F);
        assert_eq!(ansi_case(b'1', true), b'1');
    }
}
