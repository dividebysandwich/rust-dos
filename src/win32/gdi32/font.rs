//! Fonts and text. GDI here has raster fonts only, all drawn from the
//! glyphs of the VGA's ROM fonts (8x8, 8x14, 8x16), scaled to the size a
//! font asks for. A font of variable pitch has each glyph as wide as its
//! lit columns and a column between glyphs. The glyphs are code page 437's,
//! so ANSI text is drawn with the glyph of the same character, or of its
//! letter without the accent, or a question mark.
//!
//! System and MS Sans Serif, the faces dialogs and window frames use, move
//! on as far as Windows 98's own raster fonts do instead: each character
//! has the width it has in VGASYS.FON or SSERIFE.FON, and its ROM glyph is
//! squeezed or stretched to fit. Dialogs size themselves from these widths
//! (GdiGetCharDimensions), so a dialog's size and the way its text wraps
//! come out as on Windows.

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

/// A size of a face that Windows 98 has as a raster font: its metrics in
/// pixels and how far each character from 20h to FFh moves on, as
/// VGASYS.FON and SSERIFE.FON have them at 96 dots an inch.
struct Raster {
    face: usize,
    height: i32,
    ascent: i32,
    internal: i32,
    /// tmAveCharWidth.
    average: i32,
    /// The character drawn for a byte the font doesn't have.
    default: u8,
    widths: [u8; 224],
}

#[rustfmt::skip]
const RASTERS: &[Raster] = &[
    // System 10 (16 pixels).
    Raster {
        face: SYSTEM_FACE,
        height: 16,
        ascent: 13,
        internal: 3,
        average: 7,
        default: 128,
        widths: [
            4, 4, 6, 8, 8, 11, 9, 4, 4, 4, 6, 8, 4, 4, 4, 4,
            8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 4, 4, 8, 8, 8, 8,
            14, 8, 10, 9, 10, 9, 8, 10, 10, 4, 7, 9, 8, 12, 10, 10,
            9, 10, 10, 9, 8, 10, 8, 14, 9, 10, 9, 4, 4, 4, 5, 8,
            5, 8, 8, 7, 8, 8, 4, 8, 8, 4, 4, 7, 4, 12, 8, 8,
            8, 8, 5, 8, 4, 8, 8, 10, 8, 8, 8, 5, 4, 5, 5, 4,
            4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4,
            4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4,
            9, 4, 8, 8, 8, 8, 4, 8, 5, 10, 5, 7, 8, 4, 10, 8,
            5, 8, 4, 4, 5, 8, 7, 4, 5, 4, 5, 7, 11, 11, 11, 8,
            8, 8, 8, 8, 8, 8, 13, 9, 9, 9, 9, 9, 4, 4, 4, 4,
            10, 10, 10, 10, 10, 10, 10, 8, 10, 10, 10, 10, 10, 10, 9, 8,
            8, 8, 8, 8, 8, 8, 12, 7, 8, 8, 8, 8, 4, 4, 4, 4,
            8, 8, 8, 8, 8, 8, 8, 6, 8, 8, 8, 8, 8, 8, 8, 8,
        ],
    },
    // MS Sans Serif 8 (13 pixels).
    Raster {
        face: SANS_FACE,
        height: 13,
        ascent: 11,
        internal: 2,
        average: 5,
        default: 129,
        widths: [
            3, 3, 5, 7, 6, 8, 6, 2, 3, 3, 4, 6, 3, 3, 3, 5,
            6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 3, 3, 6, 6, 6, 6,
            11, 7, 7, 7, 8, 7, 6, 8, 8, 3, 5, 7, 6, 9, 8, 8,
            7, 8, 8, 7, 7, 8, 7, 11, 7, 7, 7, 3, 5, 3, 6, 6,
            3, 6, 6, 6, 6, 6, 3, 6, 6, 2, 2, 6, 2, 8, 6, 6,
            6, 6, 3, 5, 3, 6, 6, 8, 5, 5, 5, 4, 2, 4, 7, 3,
            6, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3,
            3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3,
            3, 3, 6, 6, 6, 6, 2, 6, 3, 9, 4, 6, 6, 3, 8, 6,
            4, 6, 3, 3, 3, 6, 6, 3, 3, 3, 4, 6, 8, 8, 8, 6,
            7, 7, 7, 7, 7, 7, 10, 7, 7, 7, 7, 7, 3, 3, 3, 3,
            8, 8, 8, 8, 8, 8, 8, 6, 8, 8, 8, 8, 8, 7, 7, 6,
            6, 6, 6, 6, 6, 6, 10, 6, 6, 6, 6, 6, 2, 4, 4, 4,
            6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 5, 6, 5,
        ],
    },
    // MS Sans Serif 10 (16 pixels).
    Raster {
        face: SANS_FACE,
        height: 16,
        ascent: 13,
        internal: 3,
        average: 7,
        default: 129,
        widths: [
            3, 3, 5, 7, 7, 12, 9, 3, 4, 4, 5, 7, 3, 4, 3, 4,
            7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 3, 3, 7, 7, 7, 7,
            14, 9, 9, 9, 10, 9, 8, 10, 10, 3, 7, 8, 7, 11, 10, 10,
            9, 10, 10, 9, 9, 10, 9, 13, 8, 9, 8, 4, 4, 4, 7, 7,
            4, 8, 8, 7, 8, 8, 3, 8, 7, 3, 3, 7, 3, 11, 7, 8,
            8, 8, 4, 7, 3, 7, 7, 9, 6, 7, 6, 5, 3, 5, 9, 4,
            7, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4,
            4, 3, 3, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4,
            3, 3, 7, 7, 7, 7, 3, 7, 4, 8, 5, 7, 7, 4, 8, 7,
            4, 7, 4, 4, 4, 7, 7, 3, 4, 4, 5, 7, 9, 9, 9, 7,
            9, 9, 9, 9, 9, 9, 13, 9, 9, 9, 9, 9, 3, 3, 3, 3,
            9, 10, 10, 10, 10, 10, 10, 7, 10, 10, 10, 10, 10, 9, 9, 8,
            8, 8, 8, 8, 8, 8, 13, 7, 8, 8, 8, 8, 3, 3, 3, 3,
            8, 7, 8, 8, 8, 8, 8, 7, 8, 7, 7, 7, 7, 7, 8, 7,
        ],
    },
    // MS Sans Serif 12 (20 pixels).
    Raster {
        face: SANS_FACE,
        height: 20,
        ascent: 16,
        internal: 4,
        average: 8,
        default: 129,
        widths: [
            4, 4, 6, 9, 9, 14, 11, 3, 5, 5, 6, 9, 4, 5, 4, 4,
            9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 4, 4, 9, 9, 9, 9,
            16, 11, 11, 11, 12, 11, 10, 13, 12, 5, 8, 10, 9, 13, 11, 12,
            10, 12, 12, 11, 9, 12, 11, 15, 11, 11, 10, 4, 4, 4, 7, 9,
            5, 9, 9, 8, 9, 9, 5, 9, 9, 3, 3, 8, 3, 13, 9, 9,
            9, 9, 5, 8, 5, 9, 7, 11, 7, 7, 8, 5, 5, 5, 9, 5,
            9, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5,
            5, 4, 4, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5,
            4, 4, 9, 9, 9, 9, 5, 9, 5, 11, 6, 9, 9, 5, 11, 9,
            5, 9, 5, 5, 5, 8, 8, 4, 5, 5, 6, 9, 12, 12, 12, 9,
            11, 11, 11, 11, 11, 11, 16, 11, 11, 11, 11, 11, 5, 5, 5, 5,
            12, 11, 12, 12, 12, 12, 12, 9, 12, 12, 12, 12, 12, 11, 10, 10,
            9, 9, 9, 9, 9, 9, 15, 8, 9, 9, 9, 9, 3, 3, 3, 3,
            9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 7, 9, 7,
        ],
    },
    // MS Sans Serif 14 (24 pixels).
    Raster {
        face: SANS_FACE,
        height: 24,
        ascent: 19,
        internal: 6,
        average: 9,
        default: 129,
        widths: [
            5, 5, 7, 10, 10, 15, 13, 4, 6, 6, 7, 11, 5, 6, 5, 5,
            10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 5, 5, 11, 11, 11, 10,
            19, 13, 12, 13, 13, 13, 12, 14, 14, 4, 9, 12, 10, 16, 14, 15,
            12, 15, 13, 12, 12, 13, 13, 18, 14, 12, 12, 5, 5, 5, 9, 10,
            6, 10, 11, 10, 11, 11, 4, 11, 11, 4, 4, 9, 4, 16, 11, 11,
            11, 11, 6, 9, 4, 11, 9, 14, 10, 9, 9, 6, 4, 6, 11, 6,
            10, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6,
            6, 5, 5, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6,
            5, 5, 10, 10, 10, 10, 4, 10, 6, 14, 7, 10, 11, 6, 14, 10,
            6, 11, 5, 5, 6, 9, 10, 5, 6, 5, 7, 10, 15, 15, 15, 10,
            13, 13, 13, 13, 13, 13, 19, 13, 13, 13, 13, 13, 4, 4, 4, 6,
            13, 14, 15, 15, 15, 15, 15, 11, 15, 13, 13, 13, 13, 12, 13, 11,
            10, 10, 10, 10, 10, 10, 17, 10, 11, 11, 11, 11, 4, 4, 4, 4,
            11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 9, 11, 9,
        ],
    },
    // MS Sans Serif 18 (29 pixels).
    Raster {
        face: SANS_FACE,
        height: 29,
        ascent: 23,
        internal: 5,
        average: 12,
        default: 129,
        widths: [
            6, 6, 8, 13, 13, 22, 16, 4, 8, 8, 10, 14, 6, 8, 6, 7,
            13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 6, 6, 14, 14, 14, 12,
            24, 15, 16, 17, 17, 16, 15, 18, 17, 6, 12, 16, 13, 20, 18, 19,
            16, 19, 17, 16, 16, 17, 15, 22, 17, 16, 14, 7, 7, 7, 11, 13,
            8, 13, 14, 12, 14, 14, 6, 14, 13, 6, 6, 12, 6, 20, 13, 14,
            14, 14, 8, 12, 6, 13, 11, 18, 11, 11, 11, 9, 6, 9, 14, 8,
            13, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8,
            8, 6, 6, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8,
            6, 6, 13, 13, 13, 13, 6, 13, 8, 20, 8, 13, 14, 8, 20, 13,
            8, 14, 7, 7, 8, 13, 12, 6, 8, 7, 8, 13, 18, 18, 18, 12,
            15, 15, 15, 15, 15, 15, 24, 17, 16, 16, 16, 16, 6, 6, 6, 6,
            17, 18, 19, 19, 19, 19, 19, 14, 19, 17, 17, 17, 17, 16, 16, 15,
            13, 13, 13, 13, 13, 13, 21, 12, 14, 14, 14, 14, 6, 6, 6, 6,
            14, 13, 14, 14, 14, 14, 14, 14, 14, 13, 13, 13, 13, 11, 14, 11,
        ],
    },
    // MS Sans Serif 24 (37 pixels).
    Raster {
        face: SANS_FACE,
        height: 37,
        ascent: 29,
        internal: 5,
        average: 16,
        default: 129,
        widths: [
            9, 9, 11, 18, 18, 28, 22, 7, 11, 11, 12, 19, 9, 10, 9, 9,
            18, 16, 18, 18, 18, 18, 18, 18, 18, 18, 9, 9, 19, 19, 19, 17,
            30, 22, 21, 23, 23, 21, 20, 25, 23, 8, 16, 21, 18, 26, 24, 25,
            21, 25, 22, 21, 20, 23, 22, 31, 21, 22, 20, 9, 9, 9, 15, 18,
            11, 18, 18, 16, 18, 17, 9, 18, 18, 7, 7, 16, 7, 27, 18, 18,
            18, 18, 11, 16, 9, 18, 15, 23, 15, 15, 16, 11, 7, 11, 19, 11,
            18, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11,
            11, 9, 9, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11,
            9, 9, 18, 18, 18, 18, 7, 18, 11, 24, 12, 18, 19, 10, 24, 16,
            11, 19, 9, 9, 11, 18, 17, 9, 11, 9, 12, 18, 23, 23, 23, 17,
            22, 22, 22, 22, 22, 22, 32, 23, 21, 21, 21, 21, 8, 8, 8, 8,
            23, 24, 25, 25, 25, 25, 25, 19, 25, 23, 23, 23, 23, 22, 21, 19,
            18, 18, 18, 18, 18, 18, 29, 16, 17, 17, 17, 17, 7, 7, 7, 7,
            18, 18, 18, 18, 18, 18, 18, 19, 18, 18, 18, 18, 18, 15, 18, 15,
        ],
    },
];

/// The raster size of the face `fi` a font gets, if the face has any: the
/// one as high as `device_height`, or with characters as high for a
/// negative one, and true; or else the one nearest the `cell` the font
/// is drawn at, and false.
fn raster_of(fi: usize, device_height: i32, cell: i32) -> Option<(usize, bool)> {
    let sizes = || (0..RASTERS.len()).filter(|&i| RASTERS[i].face == fi);
    let r = |i: usize| &RASTERS[i];
    let exact = match device_height {
        0 => sizes().find(|&i| r(i).height == FACES[fi].cell),
        h if h < 0 => sizes().find(|&i| r(i).height - r(i).internal == -h),
        h => sizes().find(|&i| r(i).height == h),
    };
    match exact {
        Some(i) => Some((i, true)),
        None => sizes()
            .min_by_key(|&i| (r(i).height - cell).abs())
            .map(|i| (i, false)),
    }
}

/// How far the byte `b` moves on in the raster size `r` drawn `cell`
/// pixels high.
fn raster_width(r: &Raster, b: u8, cell: i32) -> i32 {
    let c = if b < 0x20 { r.default } else { b };
    let w = r.widths[(c - 0x20) as usize] as i32;
    ((w * cell + r.height / 2) / r.height).max(1)
}

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
    /// The raster size (in RASTERS) whose widths the glyphs take.
    raster: Option<usize>,
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
    /// The glyphs are the font's ANSI characters', not code page 437's.
    ansi: bool,
}

thread_local! {
    static CACHE: RefCell<HashMap<Key, Rc<Glyphs>>> = RefCell::new(HashMap::new());
}

/// The rows of the ROM glyph `g`, scaled to the key's cell height and
/// `gw` columns.
fn rom_rows(key: &Key, g: usize, gw: i32) -> Vec<u64> {
    let (data, base_h) = match key.base {
        8 => (GLYPHS_8, 8),
        14 => (GLYPHS_14, 14),
        _ => (GLYPHS_16, 16),
    };
    let mut rows = Vec::with_capacity(key.cell as usize);
    for ty in 0..key.cell {
        let sy = (ty as usize * base_h) / key.cell as usize;
        let byte = data.get(g * base_h + sy).copied().unwrap_or(0);
        let mut bits = 0u64;
        for tx in 0..gw {
            let sx = (tx * 8) / gw;
            if byte & (0x80 >> sx) != 0 {
                bits |= 1 << tx;
            }
        }
        rows.push(bits);
    }
    rows
}

/// The glyphs of a raster size: each byte's ROM glyph, its lit columns
/// squeezed or stretched to the character's width less the column
/// between characters. A bold face's widths have its extra column in
/// them; a font made bold moves on a column more.
fn raster_glyphs(key: Key, r: &Raster) -> Glyphs {
    let native_bold = FACES[r.face].weight >= 600;
    let made_bold = key.bold && !native_bold;
    let glyphs = (0..256usize)
        .map(|b| {
            let w = raster_width(r, b as u8, key.cell);
            let mut rows = rom_rows(&key, ansi_glyph(b as u8) as usize, 8);
            let all = rows.iter().fold(0u64, |a, &r| a | r);
            if all != 0 {
                let left = all.trailing_zeros() as i32;
                let lit = 64 - all.leading_zeros() as i32 - left;
                // A row holds 64 columns: past 60, as for the other
                // faces, the glyph keeps its advance but no wider.
                let body = (w - 1 - native_bold as i32).clamp(1, 60);
                for bits in &mut rows {
                    let mut out = 0u64;
                    for tx in 0..body {
                        // The source columns this one covers, at least one.
                        let from = left + tx * lit / body;
                        let to = (left + (tx + 1) * lit / body).max(from + 1);
                        if *bits >> from & ((1 << (to - from)) - 1) != 0 {
                            out |= 1 << tx;
                        }
                    }
                    if key.bold {
                        out |= out << 1;
                    }
                    *bits = out;
                }
            }
            Glyph {
                rows,
                advance: w + made_bold as i32,
                offset: 0,
            }
        })
        .collect();
    Glyphs { glyphs, ansi: true }
}

fn make_glyphs(key: Key) -> Glyphs {
    if let Some(i) = key.raster {
        return raster_glyphs(key, &RASTERS[i]);
    }
    let gw = key.glyph_width.clamp(1, 60);
    let mut glyphs = Vec::with_capacity(256);
    for g in 0..256usize {
        let mut rows = rom_rows(&key, g, gw);
        if key.bold {
            for bits in &mut rows {
                *bits |= *bits << 1;
            }
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
    Glyphs {
        glyphs,
        ansi: false,
    }
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
    average: i32,
    glyphs: Rc<Glyphs>,
}

impl Realized {
    fn oem(&self) -> bool {
        self.charset == OEM_CHARSET
    }

    /// The glyph of the byte `b` of the font's character set.
    fn glyph(&self, b: u8) -> &Glyph {
        let g = if self.oem() || self.glyphs.ansi {
            b
        } else {
            ansi_glyph(b)
        };
        &self.glyphs.glyphs[g as usize]
    }

    /// How far the byte `b` moves on, in device units.
    pub fn advance(&self, b: u8) -> i32 {
        self.glyph(b).advance
    }

    pub fn average_width(&self) -> i32 {
        self.average
    }

    pub fn max_width(&self) -> i32 {
        (0x20..=0xFFu8).map(|b| self.advance(b)).max().unwrap_or(0)
    }
}

/// The code page 437 glyph that shows the ANSI byte `b`.
fn ansi_glyph(b: u8) -> u8 {
    if (0x20..0x7F).contains(&b) {
        return b;
    }
    let c = CodePage::Ansi.char(b);
    if let Some(g) = CodePage::Oem.byte(c)
        && g >= 0x20
    {
        return g;
    }
    match c {
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
    }
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
    let mut cell = cell.clamp(4, 128);
    let mut internal = internal.min(cell - 1);
    // A face Windows has as raster fonts gets the widths of one of its
    // sizes, and the metrics too if it is one of them. Windows doesn't
    // stretch those to a width asked for, so neither does this.
    let raster = raster_of(fi, device_height, cell);
    let mut ascent = None;
    if let Some((i, true)) = raster {
        let r = &RASTERS[i];
        (cell, internal) = (r.height, r.internal);
        ascent = Some(r.ascent);
    }
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
        // The raster faces' glyphs don't depend on it: one cache entry.
        glyph_width: if raster.is_some() { 0 } else { glyph_width },
        bold: weight >= 600,
        fixed,
        raster: raster.map(|(i, _)| i),
    };
    let glyphs = glyphs_of(key);
    let average = match raster {
        Some((i, _)) => {
            let r = &RASTERS[i];
            let made_bold = key.bold && face.weight < 600;
            ((r.average * cell + r.height / 2) / r.height).max(1) + made_bold as i32
        }
        None => glyphs.glyphs[b'x' as usize].advance,
    };
    let descent = match ascent {
        Some(a) => cell - a,
        None => ((cell * 4 + 8) / 16).max(1),
    };
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
        average,
        glyphs,
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
        assert_eq!(ansi_glyph(b'A'), b'A');
        // e acute is 82h in code page 437.
        assert_eq!(ansi_glyph(0xE9), 0x82);
        // A tilde has no glyph of its own: the plain letter.
        assert_eq!(ansi_glyph(0xC3), b'A');
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
    fn big_ms_sans_serif_glyphs_stay_in_their_rows() {
        // 128 pixels scales the 37 pixel size's widths past the 64
        // columns a glyph row holds. The glyph is drawn no wider than
        // the other faces' widest, but moves on as far.
        let log = Log::of(&logfont(128, 0, 700, 0, 0, "MS Sans Serif"));
        let f = realize(&log, 128, 0);
        assert!(f.advance(b'W') > 64);
        let all = f.glyph(b'W').rows.iter().fold(0, |a, &r| a | r);
        assert_ne!(all, 0);
        assert!(all >> 62 == 0, "no lit column wraps around");
    }

    #[test]
    fn fixed_pitch_fonts_have_one_width() {
        let log = Log::of(&logfont(0, 0, 0, 0, 0, "Courier"));
        let f = realize(&log, 0, 0);
        assert_eq!(f.advance(b'i'), f.advance(b'W'));
    }
}
