//! Fonts and text. GDI here has raster fonts only, all drawn from the
//! glyphs of the VGA's ROM fonts (8x8, 8x14, 8x16), scaled to the size a
//! font asks for. A font of variable pitch has each glyph as wide as its
//! lit columns and a column between glyphs. The glyphs are code page 437's,
//! so ANSI text is drawn with the glyph of the same character, or of its
//! letter without the accent, or a question mark.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use super::dc::{self, Dc, OPAQUE};
use super::draw::{Canvas, Fill, value_for};
use super::{CB_AT, CB_LIST, CB_PARAM, CB_PROC};
use super::{
    ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER, FALSE, Font, GDI_ERROR, Kind, Rect, TRUE, add,
    gdi, init, set_error,
};
use crate::cpu::Cpu;
use crate::win32::convert::{CodePage, decompose};
use crate::win32::task::{Next, Task};
use crate::win32::user32::{read_words, write_words};

static GLYPHS_8: &[u8] = include_bytes!("../../video/assets/IBM_VGA_8x8.bin");
static GLYPHS_14: &[u8] = include_bytes!("../../video/assets/IBM_EGA_8x14.bin");
static GLYPHS_16: &[u8] = include_bytes!("../../video/assets/IBM_VGA_8x16.bin");

/// Glyphs of ANSI characters that code page 437 has none of, drawn 8x16
/// in the VGA font's style, after its 256: the copyright and registered
/// signs, which programs put in their title screens and about boxes.
const EXTRA_GLYPHS: &[(u16, [u8; 16])] = &[
    (
        0xA9,
        [
            0, 0, 0x3C, 0x42, 0x99, 0xA5, 0xA1, 0xA1, 0xA5, 0x99, 0x42, 0x3C, 0, 0, 0, 0,
        ],
    ),
    (
        0xAE,
        [
            0, 0, 0x3C, 0x42, 0xB9, 0xA5, 0xA5, 0xB9, 0xA9, 0xA5, 0x42, 0x3C, 0, 0, 0, 0,
        ],
    ),
];

pub const ANSI_CHARSET: u8 = 0;
pub const DEFAULT_CHARSET: u8 = 1;
pub const SYMBOL_CHARSET: u8 = 2;
pub const OEM_CHARSET: u8 = 255;

const FIXED_PITCH: u8 = 1;
const VARIABLE_PITCH: u8 = 2;
const FF_ROMAN: u8 = 0x10;
const FF_SWISS: u8 = 0x20;
const FF_MODERN: u8 = 0x30;
const FF_FAMILY: u8 = 0xF0;

const TA_UPDATECP: u32 = 1;
const TA_RIGHT: u32 = 2;
const TA_CENTER: u32 = 6;
const TA_BOTTOM: u32 = 8;
const TA_BASELINE: u32 = 24;

const ETO_OPAQUE: u32 = 2;
const ETO_CLIPPED: u32 = 4;

const RASTER_FONTTYPE: u32 = 1;

/// A face GDI has: its name, pitch and family, character set, the cell
/// height it has when no height is asked for, and its weight.
struct Face {
    name: &'static str,
    pitch_family: u8,
    charset: u8,
    cell: i32,
    weight: i32,
}

const FACES: &[Face] = &[
    Face {
        name: "System",
        pitch_family: FF_SWISS | VARIABLE_PITCH,
        charset: ANSI_CHARSET,
        cell: 16,
        weight: 700,
    },
    Face {
        name: "Fixedsys",
        pitch_family: FF_MODERN | FIXED_PITCH,
        charset: ANSI_CHARSET,
        cell: 15,
        weight: 400,
    },
    Face {
        name: "Terminal",
        pitch_family: FF_MODERN | FIXED_PITCH,
        charset: OEM_CHARSET,
        cell: 12,
        weight: 400,
    },
    Face {
        name: "MS Sans Serif",
        pitch_family: FF_SWISS | VARIABLE_PITCH,
        charset: ANSI_CHARSET,
        cell: 13,
        weight: 400,
    },
    Face {
        name: "MS Serif",
        pitch_family: FF_ROMAN | VARIABLE_PITCH,
        charset: ANSI_CHARSET,
        cell: 13,
        weight: 400,
    },
    Face {
        name: "Courier",
        pitch_family: FF_MODERN | FIXED_PITCH,
        charset: ANSI_CHARSET,
        cell: 13,
        weight: 400,
    },
    Face {
        name: "Small Fonts",
        pitch_family: FF_SWISS | VARIABLE_PITCH,
        charset: ANSI_CHARSET,
        cell: 11,
        weight: 400,
    },
    Face {
        name: "Arial",
        pitch_family: FF_SWISS | VARIABLE_PITCH,
        charset: ANSI_CHARSET,
        cell: 16,
        weight: 400,
    },
    Face {
        name: "Times New Roman",
        pitch_family: FF_ROMAN | VARIABLE_PITCH,
        charset: ANSI_CHARSET,
        cell: 16,
        weight: 400,
    },
    Face {
        name: "Courier New",
        pitch_family: FF_MODERN | FIXED_PITCH,
        charset: ANSI_CHARSET,
        cell: 16,
        weight: 400,
    },
];

const SYSTEM_FACE: usize = 0;
const TERMINAL_FACE: usize = 2;
const SANS_FACE: usize = 3;
const SERIF_FACE: usize = 4;
const COURIER_FACE: usize = 5;

/// A LOGFONTA, 60 bytes.
pub fn logfont(
    height: i32,
    width: i32,
    weight: i32,
    charset: u8,
    pitch: u8,
    face: &str,
) -> Vec<u8> {
    let mut b = vec![0u8; 60];
    b[0..4].copy_from_slice(&height.to_le_bytes());
    b[4..8].copy_from_slice(&width.to_le_bytes());
    b[16..20].copy_from_slice(&weight.to_le_bytes());
    b[23] = charset;
    b[27] = pitch;
    let name = face.as_bytes();
    let n = name.len().min(31);
    b[28..28 + n].copy_from_slice(&name[..n]);
    b
}

/// The fields of a LOGFONTA that choosing a font looks at.
struct Log {
    height: i32,
    width: i32,
    weight: i32,
    italic: bool,
    underline: bool,
    strike: bool,
    charset: u8,
    pitch_family: u8,
    face: String,
}

impl Log {
    fn of(b: &[u8]) -> Log {
        let int = |i: usize| i32::from_le_bytes(b[i..i + 4].try_into().unwrap());
        let name = &b[28..60];
        let end = name.iter().position(|&c| c == 0).unwrap_or(32);
        Log {
            height: int(0),
            width: int(4),
            weight: int(16),
            italic: b[20] != 0,
            underline: b[21] != 0,
            strike: b[22] != 0,
            charset: b[23],
            pitch_family: b[27],
            face: String::from_utf8_lossy(&name[..end]).into_owned(),
        }
    }
}

/// The face a logical font gets.
fn choose_face(log: &Log) -> usize {
    if let Some(i) = FACES
        .iter()
        .position(|f| f.name.eq_ignore_ascii_case(&log.face))
    {
        return i;
    }
    if log.charset == OEM_CHARSET {
        return TERMINAL_FACE;
    }
    if log.pitch_family & 3 == FIXED_PITCH || log.pitch_family & FF_FAMILY == FF_MODERN {
        return COURIER_FACE;
    }
    if log.pitch_family & FF_FAMILY == FF_ROMAN {
        return SERIF_FACE;
    }
    if log.face.is_empty() && log.height == 0 {
        return SYSTEM_FACE;
    }
    SANS_FACE
}

/// What a font's glyphs are made from and their size.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    base: u8,
    cell: i32,
    glyph_width: i32,
    bold: bool,
    fixed: bool,
}

/// A glyph: its rows' lit pixels as bit masks (bit 0 the leftmost), how
/// far it moves on and where its pixels start.
struct Glyph {
    rows: Vec<u64>,
    advance: i32,
    offset: i32,
}

struct Glyphs {
    glyphs: Vec<Glyph>,
}

thread_local! {
    static CACHE: RefCell<HashMap<Key, Rc<Glyphs>>> = RefCell::new(HashMap::new());
}

fn make_glyphs(key: Key) -> Glyphs {
    let (data, base_h) = match key.base {
        8 => (GLYPHS_8, 8),
        14 => (GLYPHS_14, 14),
        _ => (GLYPHS_16, 16),
    };
    let gw = key.glyph_width.clamp(1, 60);
    let mut glyphs = Vec::with_capacity(256 + EXTRA_GLYPHS.len());
    for g in 0..256 + EXTRA_GLYPHS.len() {
        let mut rows = Vec::with_capacity(key.cell as usize);
        for ty in 0..key.cell {
            let byte = match g.checked_sub(256) {
                Some(e) => EXTRA_GLYPHS[e].1[ty as usize * 16 / key.cell as usize],
                None => {
                    let sy = (ty as usize * base_h) / key.cell as usize;
                    data.get(g * base_h + sy).copied().unwrap_or(0)
                }
            };
            let mut bits = 0u64;
            for tx in 0..gw {
                let sx = (tx * 8) / gw;
                if byte & (0x80 >> sx) != 0 {
                    bits |= 1 << tx;
                }
            }
            if key.bold {
                bits |= bits << 1;
            }
            rows.push(bits);
        }
        let all = rows.iter().fold(0u64, |a, &r| a | r);
        let width = gw + key.bold as i32;
        let (advance, offset) = if key.fixed {
            (width, 0)
        } else if all == 0 {
            // A space: about half the glyph width.
            ((gw * 3 + 4) / 8 + 1, 0)
        } else {
            let left = all.trailing_zeros() as i32;
            let right = 63 - all.leading_zeros() as i32;
            (right - left + 1 + (gw / 8).max(1), -left)
        };
        glyphs.push(Glyph {
            rows,
            advance,
            offset,
        });
    }
    Glyphs { glyphs }
}

fn glyphs_of(key: Key) -> Rc<Glyphs> {
    CACHE.with(|c| {
        c.borrow_mut()
            .entry(key)
            .or_insert_with(|| Rc::new(make_glyphs(key)))
            .clone()
    })
}

/// A font as GDI realizes it on the screen.
#[derive(Clone)]
pub struct Realized {
    pub face: &'static str,
    pub height: i32,
    pub ascent: i32,
    pub descent: i32,
    pub internal_leading: i32,
    pub weight: i32,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub charset: u8,
    pub pitch_family: u8,
    glyphs: Rc<Glyphs>,
}

impl Realized {
    fn oem(&self) -> bool {
        self.charset == OEM_CHARSET
    }

    /// The glyph of the byte `b` of the font's character set.
    fn glyph(&self, b: u8) -> &Glyph {
        let g = if self.oem() { b as u16 } else { ansi_glyph(b) };
        &self.glyphs.glyphs[g as usize]
    }

    /// How far the byte `b` moves on, in device units.
    pub fn advance(&self, b: u8) -> i32 {
        self.glyph(b).advance
    }

    pub fn average_width(&self) -> i32 {
        self.advance(b'x')
    }

    pub fn max_width(&self) -> i32 {
        (0x20..=0xFFu8).map(|b| self.advance(b)).max().unwrap_or(0)
    }
}

/// The glyph that shows the ANSI byte `b`: code page 437's, or one of
/// `EXTRA_GLYPHS` after them.
fn ansi_glyph(b: u8) -> u16 {
    if (0x20..0x7F).contains(&b) {
        return b as u16;
    }
    let c = CodePage::Ansi.char(b);
    // Code page 437's glyphs below 20h are the control characters', but
    // the paragraph and section signs (14h and 15h) are ANSI's too.
    if let Some(g) = CodePage::Oem.byte(c)
        && (g >= 0x20 || c >= 0xA0)
    {
        return g as u16;
    }
    if let Some(i) = EXTRA_GLYPHS.iter().position(|&(e, _)| e == c) {
        return 256 + i as u16;
    }
    let g = match c {
        0xA0 => 0xFF,
        0x2018 | 0x2019 | 0xB4 => b'\'',
        0x201C | 0x201D | 0xA8 => b'"',
        0x2013 | 0x2014 | 0xAD | 0xAF => b'-',
        0x2022 => 0x07,
        0xD7 => b'x',
        _ => match decompose(c) {
            Some((base, _)) if base < 0x80 => base as u8,
            _ => b'?',
        },
    };
    g as u16
}

/// Realize the logical font `log` for heights in device units.
fn realize(log: &Log, device_height: i32, device_width: i32) -> Realized {
    let fi = choose_face(log);
    let face = &FACES[fi];
    let (cell, internal) = match device_height {
        0 => (face.cell, (face.cell + 3) / 6),
        h if h < 0 => {
            let em = -h;
            let lead = ((em + 3) / 6).max(1);
            (em + lead, lead)
        }
        h => (h, (h + 3) / 6),
    };
    let cell = cell.clamp(4, 128);
    let internal = internal.min(cell - 1);
    let base: u8 = if cell <= 9 {
        8
    } else if cell <= 14 {
        14
    } else {
        16
    };
    let glyph_width = if device_width > 0 {
        (device_width * 8 / 7).max(1)
    } else {
        ((8 * cell + base as i32 / 2) / base as i32).max(1)
    };
    let weight = if log.weight == 0 {
        face.weight
    } else {
        log.weight
    };
    let fixed = face.pitch_family & 3 == FIXED_PITCH;
    let key = Key {
        base,
        cell,
        glyph_width,
        bold: weight >= 600,
        fixed,
    };
    let descent = ((cell * 4 + 8) / 16).max(1);
    Realized {
        face: face.name,
        height: cell,
        ascent: cell - descent,
        descent,
        internal_leading: internal,
        weight,
        italic: log.italic,
        underline: log.underline,
        strike: log.strike,
        charset: face.charset,
        pitch_family: face.pitch_family,
        glyphs: glyphs_of(key),
    }
}

/// The font selected into the DC `dc`, realized.
pub fn realized_of(cpu: &mut Cpu, dc: &Dc) -> Realized {
    let log = gdi(cpu)
        .font(dc.attrs.font)
        .map(|f| f.log.clone())
        .unwrap_or_else(|| logfont(0, 0, 0, 0, 0, "System"));
    let log = Log::of(&log);
    let height = {
        let (_, y0) = dc.to_device(0, 0);
        let (_, y1) = dc.to_device(0, log.height.abs());
        (y1 - y0).abs() * log.height.signum()
    };
    let width = dc.width_to_device(log.width.abs());
    realize(&log, height, width)
}

/// The font of the DC `hdc`, or None with the error set.
pub fn font_of(cpu: &mut Cpu, hdc: u32) -> Option<(Dc, Realized)> {
    let Some(dc) = gdi(cpu).dc(hdc).cloned() else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return None;
    };
    let f = realized_of(cpu, &dc);
    Some((dc, f))
}

/// A length along x from device units to the DC's logical ones.
fn to_logical_width(dc: &Dc, w: i32) -> i32 {
    let (a, _) = dc.to_logical(0, 0);
    let (b, _) = dc.to_logical(w, 0);
    (b - a).abs()
}

fn to_logical_height(dc: &Dc, h: i32) -> i32 {
    let (_, a) = dc.to_logical(0, 0);
    let (_, b) = dc.to_logical(0, h);
    (b - a).abs()
}

/// The device advances of `text` on the DC: the glyphs' and the DC's
/// extra space between characters and at breaks.
fn advances(dc: &Dc, f: &Realized, text: &[u8]) -> Vec<i32> {
    let extra = dc.width_to_device(dc.attrs.char_extra);
    let (mut breaks_left, mut break_extra) = (dc.attrs.break_count, dc.attrs.break_extra);
    text.iter()
        .map(|&b| {
            let mut a = f.advance(b) + extra;
            if b == b' ' && breaks_left > 0 {
                let share = break_extra / breaks_left;
                a += share;
                break_extra -= share;
                breaks_left -= 1;
            }
            a
        })
        .collect()
}

/// The size of `text` on the DC `hdc`, in its logical units.
pub fn text_extent(cpu: &mut Cpu, hdc: u32, text: &[u8]) -> Option<(i32, i32)> {
    let (dc, f) = font_of(cpu, hdc)?;
    let w: i32 = advances(&dc, &f, text).iter().sum();
    Some((to_logical_width(&dc, w), to_logical_height(&dc, f.height)))
}

/// The metrics of the DC's font that USER lays text out with, in the
/// DC's logical units.
#[derive(Clone, Copy, Debug, Default)]
pub struct Metrics {
    pub height: i32,
    pub ascent: i32,
    pub average_width: i32,
}

/// The metrics of the font of the DC `hdc`.
pub fn metrics(cpu: &mut Cpu, hdc: u32) -> Option<Metrics> {
    let (dc, f) = font_of(cpu, hdc)?;
    Some(Metrics {
        height: to_logical_height(&dc, f.height),
        ascent: to_logical_height(&dc, f.ascent),
        average_width: to_logical_width(&dc, f.average_width()),
    })
}

/// TEXTMETRICA's bytes for the font.
fn text_metric(dc: &Dc, f: &Realized) -> Vec<u8> {
    let lw = |w: i32| to_logical_width(dc, w);
    let lh = |h: i32| to_logical_height(dc, h);
    let mut b = Vec::with_capacity(56);
    for v in [
        lh(f.height),
        lh(f.ascent),
        lh(f.descent),
        lh(f.internal_leading),
        0,
        lw(f.average_width()),
        lw(f.max_width()),
        f.weight,
        0,
        96,
        96,
    ] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    let first = if f.oem() { 1 } else { 0x20 };
    // TMPF_FIXED_PITCH says the pitch is variable.
    let pitch = (f.pitch_family & FF_FAMILY) | (f.pitch_family & 3 == VARIABLE_PITCH) as u8;
    b.extend_from_slice(&[
        first,
        0xFF,
        if f.oem() { 0xFE } else { 0x80 },
        0x20,
        f.italic as u8,
        f.underline as u8,
        f.strike as u8,
        pitch,
        f.charset,
    ]);
    b.resize(56, 0);
    b
}

/// The text of a W function as ANSI.
fn ansi_of_wide(cpu: &mut Cpu, at: u32, count: u32) -> Option<Vec<u8>> {
    if !cpu.bus.guest_probe(at, 2 * count as usize, false) {
        return None;
    }
    let mut raw = vec![0u8; 2 * count as usize];
    cpu.bus.guest_read_bytes(at, &mut raw);
    if cpu.bus.guest_faulted() {
        return None;
    }
    Some(
        raw.chunks(2)
            .map(|c| {
                let w = u16::from_le_bytes([c[0], c[1]]);
                CodePage::Ansi.byte(w).unwrap_or(b'?')
            })
            .collect(),
    )
}

fn bytes_of(cpu: &mut Cpu, at: u32, count: u32) -> Option<Vec<u8>> {
    if !cpu.bus.guest_probe(at, count as usize, false) {
        return None;
    }
    let mut text = vec![0u8; count as usize];
    cpu.bus.guest_read_bytes(at, &mut text);
    (!cpu.bus.guest_faulted()).then_some(text)
}

/// CreateFontA(height, width, escapement, orientation, weight, italic,
/// underline, strikeout, charset, out, clip, quality, pitch, face).
pub fn create_font(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mut b = vec![0u8; 60];
    for i in 0..5 {
        b[4 * i..4 * i + 4].copy_from_slice(&args[i].to_le_bytes());
    }
    for i in 0..8 {
        b[20 + i] = args[5 + i] as u8;
    }
    if args[13] != 0 {
        let mut name = vec![0u8; 32];
        cpu.bus.guest_read_bytes(args[13], &mut name);
        let end = name.iter().position(|&c| c == 0).unwrap_or(31).min(31);
        b[28..28 + end].copy_from_slice(&name[..end]);
    }
    Some(add(
        cpu,
        Kind::Font {
            font: Font { log: b },
        },
    ))
}

/// CreateFontIndirectA(logfont).
pub fn create_font_indirect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    let mut b = vec![0u8; 60];
    cpu.bus.guest_read_bytes(args[0], &mut b);
    if cpu.bus.guest_faulted() {
        return None;
    }
    // The face name ends within its 32 bytes.
    b[59] = 0;
    Some(add(
        cpu,
        Kind::Font {
            font: Font { log: b },
        },
    ))
}

/// GetTextMetricsA(hdc, metrics).
pub fn get_text_metrics(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((dc, f)) = font_of(cpu, args[0]) else {
        return Some(FALSE);
    };
    let tm = text_metric(&dc, &f);
    cpu.bus.guest_write_bytes(args[1], &tm);
    Some(TRUE)
}

fn write_size(cpu: &mut Cpu, at: u32, (w, h): (i32, i32)) {
    write_words(cpu, at, &[w as u32, h as u32]);
}

/// GetTextExtentPoint32A(hdc, text, count, size).
pub fn get_text_extent_point32(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if (args[2] as i32) < 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    let text = bytes_of(cpu, args[1], args[2])?;
    let Some(size) = text_extent(cpu, args[0], &text) else {
        return Some(FALSE);
    };
    write_size(cpu, args[3], size);
    Some(TRUE)
}

/// GetTextExtentPoint32W(hdc, text, count, size).
pub fn get_text_extent_point32_w(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if (args[2] as i32) < 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    let text = ansi_of_wide(cpu, args[1], args[2])?;
    let Some(size) = text_extent(cpu, args[0], &text) else {
        return Some(FALSE);
    };
    write_size(cpu, args[3], size);
    Some(TRUE)
}

/// GetTextExtentExPointA(hdc, text, count, max, fit, dx, size).
pub fn get_text_extent_ex_point(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [hdc, at, count, max, fit, dx, size] = args.try_into().unwrap();
    if (count as i32) < 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    let text = bytes_of(cpu, at, count)?;
    let Some((dc, f)) = font_of(cpu, hdc) else {
        return Some(FALSE);
    };
    let mut total = 0;
    let mut extents = Vec::with_capacity(text.len());
    let mut fits = 0;
    for a in advances(&dc, &f, &text) {
        total += a;
        let l = to_logical_width(&dc, total);
        if l <= max as i32 {
            fits += 1;
        }
        extents.push(l as u32);
    }
    if fit != 0 {
        cpu.bus.guest_write_32(fit, fits);
    }
    if dx != 0 {
        write_words(cpu, dx, &extents);
    }
    write_size(
        cpu,
        size,
        (
            to_logical_width(&dc, total),
            to_logical_height(&dc, f.height),
        ),
    );
    Some(TRUE)
}

/// Draw `text` at the logical point (`x`, `y`) of the DC `hdc` as
/// ExtTextOutA does: `options` and `rect` (logical) are its own, `dx` the
/// logical advances, if given.
pub fn draw_text(
    cpu: &mut Cpu,
    hdc: u32,
    (x, y): (i32, i32),
    options: u32,
    rect: Option<Rect>,
    text: &[u8],
    dx: Option<&[i32]>,
) -> bool {
    let Some((dc, f)) = font_of(cpu, hdc) else {
        return false;
    };
    let align = dc.attrs.text_align;
    let mut advance = advances(&dc, &f, text);
    if let Some(dx) = dx {
        for (a, &d) in advance.iter_mut().zip(dx) {
            *a = dc.width_to_device(d);
        }
    }
    let width: i32 = advance.iter().sum();
    let (px, py) = if align & TA_UPDATECP != 0 {
        dc.to_device(dc.attrs.pos.0, dc.attrs.pos.1)
    } else {
        dc.to_device(x, y)
    };
    let left = match align & TA_CENTER {
        TA_CENTER => px - width / 2,
        TA_RIGHT => px - width,
        _ => px,
    };
    let top = match align & TA_BASELINE {
        TA_BASELINE => py - f.ascent,
        TA_BOTTOM => py - f.height,
        _ => py,
    };
    if align & TA_UPDATECP != 0 {
        let lw = to_logical_width(&dc, width);
        let delta = match align & TA_CENTER {
            TA_CENTER => 0,
            TA_RIGHT => -lw,
            _ => lw,
        };
        let pos = (dc.attrs.pos.0 + delta, dc.attrs.pos.1);
        dc::with(cpu, hdc, |d| d.attrs.pos = pos);
    }
    let rect = rect.map(|r| dc.rect_to_device(&r));
    let text_box = Rect::new(left, top, left + width, top + f.height);
    let mut bounds = text_box;
    if let Some(r) = rect
        && options & ETO_OPAQUE != 0
    {
        bounds = bounds.union(&r);
    }
    if let Some(r) = rect
        && options & ETO_CLIPPED != 0
    {
        bounds = bounds.intersect(&r);
    }
    if bounds.is_empty() {
        return true;
    }
    let Some(mut c) = Canvas::open(cpu, hdc, bounds) else {
        return true;
    };
    let (ox, oy) = c.origin;
    let clip = match rect {
        Some(r) if options & ETO_CLIPPED != 0 => Some(r.offset(ox, oy)),
        _ => None,
    };
    let within = |r: Rect| match clip {
        Some(cr) => r.intersect(&cr),
        None => r,
    };
    let bk = value_for(cpu, &dc, &c.surface, dc.attrs.bk_color);
    let fg = value_for(cpu, &dc, &c.surface, dc.attrs.text_color);
    if let Some(r) = rect
        && options & ETO_OPAQUE != 0
    {
        c.fill(within(r.offset(ox, oy)), &Fill::Solid(bk), dc::R2_COPYPEN);
    }
    if dc.attrs.bk_mode == OPAQUE {
        c.fill(
            within(text_box.offset(ox, oy)),
            &Fill::Solid(bk),
            dc::R2_COPYPEN,
        );
    }
    let allowed = |x: i32, y: i32| clip.is_none_or(|r| r.contains(x, y));
    let mut gx = left + ox;
    let gy = top + oy;
    for (&b, &a) in text.iter().zip(&advance) {
        let g = f.glyph(b);
        for (row, &bits) in g.rows.iter().enumerate() {
            if bits == 0 {
                continue;
            }
            let sy = gy + row as i32;
            // Italic leans the glyph right above its baseline.
            let lean = if f.italic {
                (f.ascent - row as i32) / 3
            } else {
                0
            };
            let mut m = bits;
            while m != 0 {
                let bit = m.trailing_zeros() as i32;
                m &= m - 1;
                let sx = gx + g.offset + bit + lean;
                if allowed(sx, sy) {
                    c.plot(sx, sy, fg, dc::R2_COPYPEN);
                }
            }
        }
        gx += a;
    }
    let mut line = |row: i32| {
        let sy = gy + row;
        for sx in left + ox..left + ox + width {
            if allowed(sx, sy) {
                c.plot(sx, sy, fg, dc::R2_COPYPEN);
            }
        }
    };
    if f.underline {
        line((f.ascent + 1).min(f.height - 1));
    }
    if f.strike {
        line(f.ascent - (f.ascent - f.internal_leading) / 3);
    }
    c.close(cpu);
    true
}

/// TextOutA(hdc, x, y, text, count).
pub fn text_out(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let text = bytes_of(cpu, args[3], args[4])?;
    let pos = (args[1] as i32, args[2] as i32);
    Some(draw_text(cpu, args[0], pos, 0, None, &text, None) as u32)
}

/// TextOutW(hdc, x, y, text, count).
pub fn text_out_w(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let text = ansi_of_wide(cpu, args[3], args[4])?;
    let pos = (args[1] as i32, args[2] as i32);
    Some(draw_text(cpu, args[0], pos, 0, None, &text, None) as u32)
}

fn ext_text_out_of(cpu: &mut Cpu, args: &[u32], text: Vec<u8>) -> Option<u32> {
    let [hdc, x, y, options, rect, _, count, dx] = args.try_into().unwrap();
    let rect = (rect != 0).then(|| Rect::read(cpu, rect));
    let dx = (dx != 0).then(|| {
        read_words(cpu, dx, count as usize)
            .into_iter()
            .map(|d| d as i32)
            .collect::<Vec<_>>()
    });
    if cpu.bus.guest_faulted() {
        return None;
    }
    let pos = (x as i32, y as i32);
    Some(draw_text(cpu, hdc, pos, options, rect, &text, dx.as_deref()) as u32)
}

/// ExtTextOutA(hdc, x, y, options, rect, text, count, dx).
pub fn ext_text_out(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let text = if args[5] == 0 {
        Vec::new()
    } else {
        bytes_of(cpu, args[5], args[6])?
    };
    ext_text_out_of(cpu, args, text)
}

/// ExtTextOutW(hdc, x, y, options, rect, text, count, dx).
pub fn ext_text_out_w(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let text = if args[5] == 0 {
        Vec::new()
    } else {
        ansi_of_wide(cpu, args[5], args[6])?
    };
    ext_text_out_of(cpu, args, text)
}

/// GetCharWidthA(hdc, first, last, widths), and the 32 and W forms: a W
/// function's characters are Unicode's.
pub fn get_char_width(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (first, last) = (args[1], args[2]);
    if last < first || last - first > 0x1_0000 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    let Some((dc, f)) = font_of(cpu, args[0]) else {
        return Some(FALSE);
    };
    let widths: Vec<u32> = (first..=last)
        .map(|c| {
            let b = if c < 0x100 {
                c as u8
            } else {
                CodePage::Ansi.byte(c as u16).unwrap_or(b'?')
            };
            to_logical_width(&dc, f.advance(b)) as u32
        })
        .collect();
    write_words(cpu, args[3], &widths);
    Some(TRUE)
}

/// GetCharABCWidthsA(hdc, first, last, abc): TrueType fonts' only.
pub fn get_char_abc_widths(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if gdi(cpu).dc(args[0]).is_none() {
        set_error(cpu, ERROR_INVALID_HANDLE);
    }
    Some(FALSE)
}

/// GetTextFaceA(hdc, count, name): with a buffer the characters it
/// copied, without one the bytes the name takes.
pub fn get_text_face(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((_, f)) = font_of(cpu, args[0]) else {
        return Some(0);
    };
    let name = f.face.as_bytes();
    if args[2] == 0 {
        return Some(name.len() as u32 + 1);
    }
    let count = args[1] as usize;
    if count == 0 {
        return Some(0);
    }
    let n = name.len().min(count - 1);
    let mut out = name[..n].to_vec();
    out.push(0);
    cpu.bus.guest_write_bytes(args[2], &out);
    Some(n as u32)
}

/// GetTextCharset(hdc).
pub fn get_text_charset(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    match font_of(cpu, args[0]) {
        Some((_, f)) => Some(f.charset as u32),
        None => Some(DEFAULT_CHARSET as u32),
    }
}

/// GetTextCharsetInfo(hdc, signature, flags).
pub fn get_text_charset_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let charset = match font_of(cpu, args[0]) {
        Some((_, f)) => f.charset,
        None => return Some(DEFAULT_CHARSET as u32),
    };
    if args[1] != 0 {
        let csb = CHARSETS.iter().find(|c| c.0 == charset).map_or(0, |c| c.2);
        write_words(cpu, args[1], &[0, 0, 0, 0, csb, 0]);
    }
    Some(charset as u32)
}

/// The character sets: theirs, their code page and their bit of a
/// FONTSIGNATURE's code pages.
const CHARSETS: &[(u8, u32, u32)] = &[
    (0, 1252, 0x1),
    (238, 1250, 0x2),
    (204, 1251, 0x4),
    (161, 1253, 0x8),
    (162, 1254, 0x10),
    (177, 1255, 0x20),
    (178, 1256, 0x40),
    (186, 1257, 0x80),
    (222, 874, 0x1_0000),
    (128, 932, 0x2_0000),
    (134, 936, 0x4_0000),
    (129, 949, 0x8_0000),
    (136, 950, 0x10_0000),
    (2, 42, 0x8000_0000),
];

/// TranslateCharsetInfo(source, info, flags).
pub fn translate_charset_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (src, out, flags) = (args[0], args[1], args[2]);
    let found = match flags {
        1 => CHARSETS.iter().find(|c| c.0 as u32 == src),
        2 => CHARSETS.iter().find(|c| c.1 == src),
        3 => {
            let bits = cpu.bus.guest_read_32(src);
            CHARSETS.iter().find(|c| bits & c.2 != 0)
        }
        _ => None,
    };
    let Some(&(charset, cp, bit)) = found else {
        return Some(FALSE);
    };
    write_words(cpu, out, &[charset as u32, cp, 0, 0, 0, 0, bit, 0]);
    Some(TRUE)
}

/// GetCharacterPlacementA(hdc, text, count, max, results, flags): the
/// glyphs are the characters, in their order.
pub fn get_character_placement(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [hdc, at, count, _, results, _] = args.try_into().unwrap();
    let text = bytes_of(cpu, at, count)?;
    let Some((dc, f)) = font_of(cpu, hdc) else {
        return Some(0);
    };
    let adv = advances(&dc, &f, &text);
    let total: i32 = adv.iter().sum();
    if results != 0 {
        let r = read_words(cpu, results, 9);
        let n = (r[7] as usize).min(text.len());
        if r[1] != 0 {
            cpu.bus.guest_write_bytes(r[1], &text[..n]);
        }
        let seq: Vec<u32> = (0..n as u32).collect();
        if r[2] != 0 {
            write_words(cpu, r[2], &seq);
        }
        if r[3] != 0 {
            let dx: Vec<u32> = adv[..n]
                .iter()
                .map(|&a| to_logical_width(&dc, a) as u32)
                .collect();
            write_words(cpu, r[3], &dx);
        }
        if r[4] != 0 {
            let mut pos = 0;
            let caret: Vec<u32> = adv[..n]
                .iter()
                .map(|&a| {
                    let p = pos;
                    pos += a;
                    to_logical_width(&dc, p) as u32
                })
                .collect();
            write_words(cpu, r[4], &caret);
        }
        if r[6] != 0 {
            let glyphs: Vec<u16> = text[..n].iter().map(|&b| b as u16).collect();
            let bytes: Vec<u8> = glyphs.iter().flat_map(|g| g.to_le_bytes()).collect();
            cpu.bus.guest_write_bytes(r[6], &bytes);
        }
        cpu.bus.guest_write_32(results + 28, n as u32);
        cpu.bus.guest_write_32(results + 32, n as u32);
    }
    let w = to_logical_width(&dc, total) as u32 & 0xFFFF;
    let h = to_logical_height(&dc, f.height) as u32 & 0xFFFF;
    Some(w | h << 16)
}

/// GetFontData and GetGlyphOutlineA: TrueType fonts' only.
pub fn get_font_data(_cpu: &mut Cpu, _args: &[u32]) -> Option<u32> {
    Some(GDI_ERROR)
}

/// GetRasterizerCaps(status, size): no TrueType.
pub fn get_rasterizer_caps(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] != 0 && args[1] >= 6 {
        cpu.bus.guest_write_bytes(args[0], &[6, 0, 0, 0, 0, 0]);
    }
    Some(TRUE)
}

/// AddFontResourceA(file): the fonts it added. The file's fonts aren't
/// drawn; a font that asks for their face gets one GDI has.
pub fn add_font_resource(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(name) = crate::win32::user32::string_at(cpu, args[0]) else {
        return Some(0);
    };
    init(cpu);
    gdi(cpu).font_files.push(name);
    Some(1)
}

/// RemoveFontResourceA(file).
pub fn remove_font_resource(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(name) = crate::win32::user32::string_at(cpu, args[0]) else {
        return Some(FALSE);
    };
    let files = &mut gdi(cpu).font_files;
    match files.iter().position(|f| f.eq_ignore_ascii_case(&name)) {
        Some(i) => {
            files.remove(i);
            Some(TRUE)
        }
        None => Some(FALSE),
    }
}

/// Scratch space a font enumeration's callback gets: ENUMLOGFONTEXA
/// and NEWTEXTMETRICEXA.
const ENUM_SCRATCH: u32 = 188 + 96;

/// Start enumerating the faces `faces` with their character sets, each
/// item a face index and a character set (face | charset << 8).
fn start_enum(cpu: &mut Cpu, items: Vec<u32>, proc: u32, param: u32) -> Option<u32> {
    if items.is_empty() {
        return Some(1);
    }
    let mut vars = vec![super::CB_FONTS, proc, param, 0];
    vars.extend(items);
    crate::win32::task::start(cpu, super::TASK_CALLBACK, vars, ENUM_SCRATCH)
}

/// The faces whose name is `family`, or all of them with none.
fn faces_named(family: Option<&str>) -> Vec<usize> {
    (0..FACES.len())
        .filter(|&i| family.is_none_or(|f| f.eq_ignore_ascii_case(FACES[i].name)))
        .collect()
}

/// EnumFontFamiliesA(hdc, family, proc, param), and EnumFontsA.
pub fn enum_font_families(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if gdi(cpu).dc(args[0]).is_none() {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0);
    }
    let family = if args[1] == 0 {
        None
    } else {
        crate::win32::user32::string_at(cpu, args[1])
    };
    let items = faces_named(family.as_deref())
        .into_iter()
        .map(|i| i as u32 | (FACES[i].charset as u32) << 8)
        .collect();
    start_enum(cpu, items, args[2], args[3])
}

/// EnumFontFamiliesExA(hdc, logfont, proc, param, flags): the faces of
/// the LOGFONT's name and character set, either of which may be any.
pub fn enum_font_families_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if gdi(cpu).dc(args[0]).is_none() {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0);
    }
    let mut b = vec![0u8; 60];
    cpu.bus.guest_read_bytes(args[1], &mut b);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let log = Log::of(&b);
    let family = (!log.face.is_empty()).then_some(log.face.as_str());
    let items = faces_named(family)
        .into_iter()
        .filter(|&i| log.charset == DEFAULT_CHARSET || FACES[i].charset == log.charset)
        .map(|i| i as u32 | (FACES[i].charset as u32) << 8)
        .collect();
    start_enum(cpu, items, args[2], args[3])
}

/// The next face of a font enumeration (gdi32::callback_step's): its
/// ENUMLOGFONTEXA and NEWTEXTMETRICEXA to the callback.
pub fn enum_callback(cpu: &mut Cpu, t: &mut Task, at: usize) -> Next {
    let Some(&item) = t.vars.get(CB_LIST + at) else {
        return Next::Done(1);
    };
    t.set(CB_AT, at as u32 + 1);
    let face = &FACES[(item & 0xFF) as usize % FACES.len()];
    let charset = (item >> 8) as u8;
    let mut log_bytes = logfont(
        face.cell,
        0,
        face.weight,
        charset,
        face.pitch_family,
        face.name,
    );
    log_bytes[24] = 1; // OUT_STRING_PRECIS
    log_bytes[25] = 2; // CLIP_STROKE_PRECIS
    log_bytes[26] = 2; // PROOF_QUALITY
    let f = realize(&Log::of(&log_bytes), face.cell, 0);
    log_bytes[4..8].copy_from_slice(&f.average_width().to_le_bytes());
    let mut elf = log_bytes;
    let mut full = face.name.as_bytes().to_vec();
    full.resize(64, 0);
    elf.extend_from_slice(&full);
    let mut style = if face.weight >= 600 {
        b"Bold".to_vec()
    } else {
        b"Regular".to_vec()
    };
    style.resize(32, 0);
    elf.extend_from_slice(&style);
    let mut script = if charset == OEM_CHARSET {
        b"OEM/DOS".to_vec()
    } else {
        b"Western".to_vec()
    };
    script.resize(32, 0);
    elf.extend_from_slice(&script);
    let identity = Dc::new(dc::DISPLAY);
    let mut ntm = text_metric(&identity, &f);
    // ntmFlags (NTM_REGULAR or NTM_BOLD), ntmSizeEM, ntmCellHeight,
    // ntmAvgWidth, then the FONTSIGNATURE.
    let flags: u32 = if face.weight >= 600 { 0x20 } else { 0x40 };
    for v in [
        flags,
        (f.height - f.internal_leading) as u32,
        f.height as u32,
        f.average_width() as u32,
    ] {
        ntm.extend_from_slice(&v.to_le_bytes());
    }
    ntm.resize(96, 0);
    let s = t.scratch;
    cpu.bus.guest_write_bytes(s, &elf);
    cpu.bus.guest_write_bytes(s + 188, &ntm);
    Next::call(
        t.var(CB_PROC),
        &[s, s + 188, RASTER_FONTTYPE, t.var(CB_PARAM)],
        1,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ansi_text_gets_code_page_437_glyphs() {
        assert_eq!(ansi_glyph(b'A'), b'A' as u16);
        // e acute is 82h in code page 437.
        assert_eq!(ansi_glyph(0xE9), 0x82);
        // A tilde has no glyph of its own: the plain letter.
        assert_eq!(ansi_glyph(0xC3), b'A' as u16);
        // The section sign is 15h; the copyright sign isn't in 437.
        assert_eq!(ansi_glyph(0xA7), 0x15);
        assert_eq!(ansi_glyph(0xA9), 256);
    }

    #[test]
    fn system_font_is_16_high() {
        let log = Log::of(&logfont(0, 0, 0, 0, 0, "System"));
        let f = realize(&log, 0, 0);
        assert_eq!(f.height, 16);
        assert_eq!(f.ascent + f.descent, 16);
        assert!(f.weight >= 600);
        assert!(f.advance(b'W') > f.advance(b'i'));
    }

    #[test]
    fn negative_heights_are_the_characters_height() {
        let log = Log::of(&logfont(-11, 0, 400, 0, 0, "MS Sans Serif"));
        let f = realize(&log, -11, 0);
        assert_eq!(f.height, 13);
        assert_eq!(f.internal_leading, 2);
    }

    #[test]
    fn fixed_pitch_fonts_have_one_width() {
        let log = Log::of(&logfont(0, 0, 0, 0, 0, "Courier"));
        let f = realize(&log, 0, 0);
        assert_eq!(f.advance(b'i'), f.advance(b'W'));
    }
}
