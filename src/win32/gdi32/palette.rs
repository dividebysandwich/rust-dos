//! Logical palettes and the system palette.
//!
//! In a mode of 32, 24 or 16 bits a pixel a palette only says what
//! PALETTEINDEX colors and DIB_PAL_COLORS tables are: realizing one
//! changes nothing on the screen.
//!
//! On a desktop of 256 colours (`win32_depth=8`), and in an 8-bit mode
//! DirectDraw set, the screen's pixels are indices into the system
//! palette, which the DAC holds, as with a palette display driver of
//! Windows 3.1 and 95. Its 20 static colors are at its ends (2 with
//! SYSPAL_NOSTATIC), and realizing a logical palette maps each of its
//! entries to an entry of the system palette:
//!
//! - In the foreground (the palette USER's SelectPalette made the
//!   primary one, selected into a DC of the foreground window without
//!   bForceBackground), every entry but the static ones is freed first.
//!   In the background, only the entries the same palette took last
//!   time that no other palette maps to.
//! - An entry with PC_EXPLICIT is the system palette's entry its low
//!   word names.
//! - One with PC_RESERVED takes a free entry of its own, which no other
//!   palette's entry maps to and AnimatePalette changes.
//! - One with PC_NOCOLLAPSE takes a free entry.
//! - The others take an entry of the same color if there is one, else a
//!   free entry.
//! - When no entry is free, the nearest color of those taken.
//!
//! RealizePalette returns how many entries' mappings changed, and when
//! the primary palette changed any in a window's DC, USER sends
//! WM_PALETTECHANGED to the top-level windows (as Wine's
//! UserRealizePalette does). Realizing the palette realized last again
//! does nothing until its entries change.
//!
//! On such a device an RGB color takes the nearest static color, with
//! one exception: a solid brush of a color that isn't one of the 20
//! static ones fills with an 8x8 ordered dither of the 16 VGA colors
//! (`dither_levels`), even when the realized palette has the color. So
//! FillRect, PatBlt and the inside of Rectangle dither, on the screen
//! and in a DDB. Pens, text and its opaque background, SetPixel, and
//! PALETTEINDEX and PALETTERGB colors don't, nor anything drawn in a
//! DIB section. The pattern's origin is the DC's (the window's), moved
//! by the brush origin.

use super::{
    DEFAULT_PALETTE, ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER, FALSE, Kind, Palette, TRUE,
    add, dc, gdi, init, set_error, static_colors,
};
use crate::cpu::Cpu;
use crate::win32::user32::{read_words, write_words};

pub const PC_RESERVED: u8 = 1;
pub const PC_EXPLICIT: u8 = 2;
pub const PC_NOCOLLAPSE: u8 = 4;
pub const SYSPAL_ERROR: u32 = 0;
pub const SYSPAL_STATIC: u32 = 1;
pub const SYSPAL_NOSTATIC: u32 = 2;
const CLR_INVALID: u32 = 0xFFFF_FFFF;

/// What an entry of the system palette is.
pub const USE_FREE: u8 = 0;
pub const USE_STATIC: u8 = 1;
/// A logical palette's color, which others of the same color share.
pub const USE_SHARED: u8 = 2;
/// A PC_RESERVED entry's, which is its palette's alone.
pub const USE_RESERVED: u8 = 3;

fn entry_of(w: u32) -> [u8; 4] {
    w.to_le_bytes()
}

fn word_of(e: [u8; 4]) -> u32 {
    u32::from_le_bytes(e)
}

/// An entry's color as a COLORREF.
fn rgb(e: [u8; 4]) -> u32 {
    e[0] as u32 | (e[1] as u32) << 8 | (e[2] as u32) << 16
}

/// Whether the system palette's entry `i` is a static color under a
/// SetSystemPaletteUse setting: 20 are with SYSPAL_STATIC, black and
/// white with SYSPAL_NOSTATIC.
fn is_static(static_use: u32, i: usize) -> bool {
    if static_use == SYSPAL_NOSTATIC {
        i == 0 || i == 255
    } else {
        !(10..246).contains(&i)
    }
}

/// What the system palette's entries are with nothing realized.
pub fn static_uses(static_use: u32) -> Vec<u8> {
    (0..256)
        .map(|i| {
            if is_static(static_use, i) {
                USE_STATIC
            } else {
                USE_FREE
            }
        })
        .collect()
}

/// Whether the screen GDI draws on is a palette device, its pixels
/// indices into the system palette: a desktop of 256 colours, or an
/// 8-bit mode DirectDraw set (`screen::gdi_mode`).
pub fn palette_device(cpu: &Cpu) -> bool {
    crate::win32::screen::gdi_mode(cpu).is_some_and(|m| m.bpp == 8)
}

/// The system palette's colors, as COLORREFs: the colors of a desktop of
/// 256 colours.
pub fn system_colors(cpu: &mut Cpu) -> Vec<u32> {
    init(cpu);
    gdi(cpu).system_palette.iter().map(|&e| rgb(e)).collect()
}

/// Put the system palette in the DAC, on a palette device
/// (`palette_device`): in an 8-bit mode DirectDraw set too, as GDI's
/// palette manager on Windows 95 and 98 sets the hardware's palette
/// whatever set the mode.
pub fn show_system_palette(cpu: &mut Cpu) {
    show_system_entries(cpu, 0, 255);
}

/// Put the system palette's entries `first..=last` in the DAC, as
/// `show_system_palette` does.
fn show_system_entries(cpu: &mut Cpu, first: usize, last: usize) {
    if !palette_device(cpu) {
        return;
    }
    let colors = system_colors(cpu);
    crate::win32::screen::set_colors(cpu, first, &colors[first..=last.min(255)]);
}

/// How colors become the pixels of a palette device (`Surface::device`)
/// in a DC: through its logical palette's entries, mapped to the system
/// palette, and the static colors.
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceMap {
    /// The system palette's index of each of the logical palette's
    /// entries, and their colors.
    pub map: Vec<u8>,
    pub logical: Vec<u32>,
    /// The static colors, and their indices.
    pub statics: Vec<(u32, u8)>,
    /// Whether the logical palette is the stock DEFAULT_PALETTE.
    pub default: bool,
}

impl DeviceMap {
    /// The map of the logical palette `hpal`: as it was realized, or for
    /// one that wasn't, its colors' nearest in the system palette.
    pub fn new(cpu: &mut Cpu, hpal: u32) -> DeviceMap {
        init(cpu);
        let state = gdi(cpu);
        let system = &state.system_palette;
        let uses = &state.system_use;
        let statics: Vec<(u32, u8)> = (0..256)
            .filter(|&i| uses.get(i) == Some(&USE_STATIC))
            .map(|i| (rgb(system[i]), i as u8))
            .collect();
        let default = hpal == state.stock(DEFAULT_PALETTE);
        let (logical, mut map) = match state.palette(hpal) {
            Some(p) => (
                p.entries.iter().map(|&e| rgb(e)).collect::<Vec<u32>>(),
                p.mapping.clone(),
            ),
            None => (Vec::new(), Vec::new()),
        };
        if map.len() != logical.len() {
            let taken: Vec<(u32, u8)> = (0..256)
                .filter(|&i| uses.get(i).is_some_and(|&u| u != USE_FREE))
                .map(|i| (rgb(system[i]), i as u8))
                .collect();
            map = logical.iter().map(|&c| nearest_of(&taken, c)).collect();
        }
        DeviceMap {
            map,
            logical,
            statics,
            default,
        }
    }

    /// PALETTEINDEX(`i`): the logical palette's entry, wrapping around
    /// its end.
    pub fn index(&self, i: usize) -> u32 {
        match self.map.len() {
            0 => 0,
            n => self.map[i % n] as u32,
        }
    }

    /// An RGB color, as pens, brushes and text draw it: the nearest
    /// static color.
    pub fn solid(&self, color: u32) -> u32 {
        nearest_of(&self.statics, color) as u32
    }

    /// A solid brush of the RGB color `color`, as Windows 95's display
    /// drivers realize one on a palette device: one of the 20 static
    /// colors is solid (None), any other an 8x8 pattern of the 16 VGA
    /// colors (`dither_levels`), each as its static entry, rows from the
    /// top.
    pub fn dither(&self, color: u32) -> Option<Vec<u32>> {
        if static_colors().contains(&color) {
            return None;
        }
        let level = |l: u8| [0u32, 0x80, 0xFF][l as usize];
        let rgb = |l: [u8; 3]| level(l[0]) | level(l[1]) << 8 | level(l[2]) << 16;
        Some(dither_levels(color).iter().map(|&l| self.solid(rgb(l))).collect())
    }

    /// PALETTERGB: the logical palette's nearest entry.
    pub fn palette_rgb(&self, color: u32) -> u32 {
        if self.logical.is_empty() {
            return self.solid(color);
        }
        self.index(super::surface::nearest(&self.logical, color) as usize)
    }

    /// A DIB's or bitmap's color `color` in a blit: the nearest of the
    /// logical palette's colors and the static ones. `hint` is the
    /// pixel's index in its color table: where the logical palette has
    /// the same color at the same index, that entry, so a DIB whose
    /// table is the palette keeps its indices even where colors repeat.
    pub fn blit_color(&self, color: u32, hint: Option<u32>) -> u32 {
        if let Some(i) = hint
            && self.logical.get(i as usize) == Some(&color)
        {
            return self.map[i as usize] as u32;
        }
        let mut best = (u32::MAX, 0);
        let candidates = self
            .logical
            .iter()
            .zip(&self.map)
            .map(|(&c, &i)| (c, i))
            .chain(self.statics.iter().copied());
        for (c, i) in candidates {
            let d = distance(c, color);
            if d < best.0 {
                best = (d, i as u32);
                if d == 0 {
                    break;
                }
            }
        }
        best.1
    }

    /// A pixel of a DIB or bitmap of more than 8 bits a pixel, of the
    /// color `color`, in a blit, StretchDIBits or SetDIBitsToDevice.
    ///
    /// Windows 98 maps these through an inverse color table of 5 bits a
    /// channel: the pixel's color counts as the cell of the 32x32x32
    /// cube its top 5 bits of red, green and blue name, so every color
    /// of a cell draws the same. Each cell takes:
    ///
    /// - with a logical palette selected, the entry nearest (sum of the
    ///   squared differences) to the cell's middle, 8 c + 4 for each
    ///   5-bit channel c; the first entry of those equally near. Only the
    ///   logical palette's entries count, not the static colors.
    /// - with the default palette, one of the 16 VGA colors: the nearest
    ///   to the cell's corner, 8 c, the later of those equally near (in
    ///   the order black, the 6 dark colors, silver, grey, the 6 bright
    ///   ones, white), with 255 in a color's channel counting as 250.
    ///   The four other static colors aren't drawn.
    ///
    /// Read back pixel by pixel on Windows 98 SE, every one of the 32768
    /// cells through the halftone palette, a palette of 64 colors and the
    /// default palette, with StretchDIBits, SetDIBitsToDevice and BitBlt
    /// from a DIB section alike. The 250 is fitted: 249 and 250 give
    /// every cell, 248 and 251 to 255 miss 99 to 456 of them, all near
    /// silver. A DIB of 8 bits or fewer goes by `blit_color` instead.
    pub fn deep_color(&self, color: u32) -> u32 {
        let cell = |s: u32| (color >> s) & 0xF8;
        if self.default && self.logical.len() == VGA_OF_DEFAULT.len() + 4 {
            let corner = cell(0) | cell(8) << 8 | cell(16) << 16;
            let full = |c: u32| {
                let ch = |s: u32| match (c >> s) & 0xFF {
                    0xFF => 250,
                    v => v,
                };
                ch(0) | ch(8) << 8 | ch(16) << 16
            };
            let mut best = (u32::MAX, 0);
            for &i in &VGA_OF_DEFAULT {
                let d = distance(full(self.logical[i]), corner);
                if d <= best.0 {
                    best = (d, i);
                }
            }
            return self.map[best.1] as u32;
        }
        if self.logical.is_empty() {
            return self.solid(color);
        }
        let middle = (cell(0) + 4) | (cell(8) + 4) << 8 | (cell(16) + 4) << 16;
        let mut best = (u32::MAX, 0);
        for (i, &c) in self.logical.iter().enumerate() {
            let d = distance(c, middle);
            if d < best.0 {
                best = (d, i);
            }
        }
        self.map[best.1] as u32
    }
}

/// The default palette's entries of the 16 VGA colors, in their VGA
/// order: all but its entries 8, 9, 10 and 11 (the four colors of the
/// 20 static ones that aren't VGA colors).
const VGA_OF_DEFAULT: [usize; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 12, 13, 14, 15, 16, 17, 18, 19];

/// The ordered dither's thresholds, 0 to 63, rows from the top.
const BAYER: [[u8; 8]; 8] = [
    [0, 32, 8, 40, 2, 34, 10, 42],
    [48, 16, 56, 24, 50, 18, 58, 26],
    [12, 44, 4, 36, 14, 46, 6, 38],
    [60, 28, 52, 20, 62, 30, 54, 22],
    [3, 35, 11, 43, 1, 33, 9, 41],
    [51, 19, 59, 27, 49, 17, 57, 25],
    [15, 47, 7, 39, 13, 45, 5, 37],
    [63, 31, 55, 23, 61, 29, 53, 21],
];

/// The 8x8 pattern Windows 95's display drivers make of a solid brush of
/// `color` on a palette device: for each pixel, rows from the top, its
/// red, green and blue as levels 0, 1 and 2 (0, 0x80 and 0xFF), which
/// make one of the 16 VGA colors.
///
/// A channel `v` counts as `h` = 2 * ((v + 1) / 4) sixty-fourths of
/// 0x80, so 0 to 128. A color with no channel above 64 mixes the dark
/// colors: a pixel's channel is 1 where its threshold is at least
/// 64 - h. Otherwise the brightest channel's excess, k = h - 64, is how
/// many pixels take bright colors (thresholds from 64 - k): each channel
/// gives the dark pixels as much of it as they hold, min(h, 64 - k),
/// and the rest, at half weight (a bright level is two dark ones), to
/// the bright pixels. Measured on Windows 95 OSR2 with a Voodoo 3's
/// driver: this reproduces every pixel of 1075 brushes, among them all
/// 256 levels of red, green, blue and grey.
fn dither_levels(color: u32) -> [[u8; 3]; 64] {
    let h = [0, 8, 16].map(|s| (((color >> s) & 0xFF) + 1) / 4 * 2);
    let k = h.iter().max().unwrap().saturating_sub(64);
    let dark = h.map(|v| v.min(64 - k));
    let bright = [0, 1, 2].map(|c| (h[c] - dark[c]) / 2);
    let mut out = [[0u8; 3]; 64];
    for (i, px) in out.iter_mut().enumerate() {
        let m = BAYER[i / 8][i % 8] as u32;
        *px = if m >= 64 - k {
            bright.map(|b| if m >= 64 - b { 2 } else { 0 })
        } else {
            dark.map(|d| (m >= 64 - k - d) as u8)
        };
    }
    out
}

fn distance(a: u32, b: u32) -> u32 {
    let d = |s: u32| ((a >> s) & 0xFF).abs_diff((b >> s) & 0xFF);
    let (r, g, b) = (d(0), d(8), d(16));
    r * r + g * g + b * b
}

/// The index of the nearest color of `table` (color, index) to `color`.
fn nearest_of(table: &[(u32, u8)], color: u32) -> u8 {
    let mut best = (u32::MAX, 0);
    for &(c, i) in table {
        let d = distance(c, color);
        if d < best.0 {
            best = (d, i);
            if d == 0 {
                break;
            }
        }
    }
    best.1
}

/// The halftone palette's colors: what Windows 98 SE's
/// CreateHalftonePalette gives on a desktop of 256 colours, read back
/// with GetPaletteEntries. The 20 static colors are at its ends, greys
/// and a few other colors at 10 to 28 and 233 to 245, and between them
/// the 6x6x6 cube of the levels 0, 51, ... 255 in Windows' own order,
/// less the four of one channel 0, one 51 and one 255.
#[rustfmt::skip]
pub const HALFTONE: [u32; 256] = [
    0x000000, 0x000080, 0x008000, 0x008080, 0x800000, 0x800080, 0x808000, 0xC0C0C0,
    0xC0DCC0, 0xF0CAA6, 0x040404, 0x080808, 0x0C0C0C, 0x111111, 0x161616, 0x1C1C1C,
    0x222222, 0x292929, 0x555555, 0x4D4D4D, 0x424242, 0x393939, 0x807CFF, 0x5050FF,
    0x9300D6, 0xFFECCC, 0xC6D6EF, 0xD6E7E7, 0x90A9AD, 0x000033, 0x000066, 0x000099,
    0x0000CC, 0x003300, 0x003333, 0x003366, 0x003399, 0x0033CC, 0x0033FF, 0x006600,
    0x006633, 0x006666, 0x006699, 0x0066CC, 0x0066FF, 0x009900, 0x009933, 0x009966,
    0x009999, 0x0099CC, 0x0099FF, 0x00CC00, 0x00CC33, 0x00CC66, 0x00CC99, 0x00CCCC,
    0x00CCFF, 0x00FF66, 0x00FF99, 0x00FFCC, 0x330000, 0x330033, 0x330066, 0x330099,
    0x3300CC, 0x3300FF, 0x333300, 0x333333, 0x333366, 0x333399, 0x3333CC, 0x3333FF,
    0x336600, 0x336633, 0x336666, 0x336699, 0x3366CC, 0x3366FF, 0x339900, 0x339933,
    0x339966, 0x339999, 0x3399CC, 0x3399FF, 0x33CC00, 0x33CC33, 0x33CC66, 0x33CC99,
    0x33CCCC, 0x33CCFF, 0x33FF33, 0x33FF66, 0x33FF99, 0x33FFCC, 0x33FFFF, 0x660000,
    0x660033, 0x660066, 0x660099, 0x6600CC, 0x6600FF, 0x663300, 0x663333, 0x663366,
    0x663399, 0x6633CC, 0x6633FF, 0x666600, 0x666633, 0x666666, 0x666699, 0x6666CC,
    0x669900, 0x669933, 0x669966, 0x669999, 0x6699CC, 0x6699FF, 0x66CC00, 0x66CC33,
    0x66CC99, 0x66CCCC, 0x66CCFF, 0x66FF00, 0x66FF33, 0x66FF99, 0x66FFCC, 0xCC00FF,
    0xFF00CC, 0x999900, 0x993399, 0x990099, 0x9900CC, 0x990000, 0x993333, 0x990066,
    0x9933CC, 0x9900FF, 0x996600, 0x996633, 0x993366, 0x996699, 0x9966CC, 0x9933FF,
    0x999933, 0x999966, 0x999999, 0x9999CC, 0x9999FF, 0x99CC00, 0x99CC33, 0x66CC66,
    0x99CC99, 0x99CCCC, 0x99CCFF, 0x99FF00, 0x99FF33, 0x99CC66, 0x99FF99, 0x99FFCC,
    0x99FFFF, 0xCC0000, 0x990033, 0xCC0066, 0xCC0099, 0xCC00CC, 0x993300, 0xCC3333,
    0xCC3366, 0xCC3399, 0xCC33CC, 0xCC33FF, 0xCC6600, 0xCC6633, 0x996666, 0xCC6699,
    0xCC66CC, 0x9966FF, 0xCC9900, 0xCC9933, 0xCC9966, 0xCC9999, 0xCC99CC, 0xCC99FF,
    0xCCCC00, 0xCCCC33, 0xCCCC66, 0xCCCC99, 0xCCCCCC, 0xCCCCFF, 0xCCFF00, 0xCCFF33,
    0x99FF66, 0xCCFF99, 0xCCFFCC, 0xCCFFFF, 0xCC0033, 0xFF0066, 0xFF0099, 0xCC3300,
    0xFF3333, 0xFF3366, 0xFF3399, 0xFF33CC, 0xFF33FF, 0xFF6600, 0xFF6633, 0xCC6666,
    0xFF6699, 0xFF66CC, 0xCC66FF, 0xFF9900, 0xFF9933, 0xFF9966, 0xFF9999, 0xFF99CC,
    0xFF99FF, 0xFFCC00, 0xFFCC33, 0xFFCC66, 0xFFCC99, 0xFFCCCC, 0xFFCCFF, 0xFFFF33,
    0xCCFF66, 0xFFFF99, 0xFFFFCC, 0x6666FF, 0x66FF66, 0x66FFFF, 0xFF6666, 0xFF66FF,
    0xFFFF66, 0x2100A5, 0x5F5F5F, 0x777777, 0x868686, 0x969696, 0xCBCBCB, 0xB2B2B2,
    0xD7D7D7, 0xDDDDDD, 0xE3E3E3, 0xEAEAEA, 0xF1F1F1, 0xF8F8F8, 0xF0FBFF, 0xA4A0A0,
    0x808080, 0x0000FF, 0x00FF00, 0x00FFFF, 0xFF0000, 0xFF00FF, 0xFFFF00, 0xFFFFFF,
];

/// The halftone palette's colors (`HALFTONE`).
pub fn halftone_colors() -> Vec<u32> {
    HALFTONE.to_vec()
}

/// CreatePalette(logpalette).
pub fn create_palette(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[0];
    if at == 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    let head = cpu.bus.guest_read_32(at);
    let count = (head >> 16) as usize;
    let entries = read_words(cpu, at + 4, count);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let palette = Palette {
        entries: entries.into_iter().map(entry_of).collect(),
        mapping: Vec::new(),
    };
    Some(add(cpu, Kind::Palette { palette }))
}

/// CreateHalftonePalette(hdc).
pub fn create_halftone_palette(cpu: &mut Cpu, _args: &[u32]) -> Option<u32> {
    Some(add(cpu, palette_of(&halftone_colors())))
}

/// A palette of the colors `colors` (as COLORREFs), with no flags.
fn palette_of(colors: &[u32]) -> Kind {
    let palette = Palette {
        entries: colors
            .iter()
            .map(|&c| [c as u8, (c >> 8) as u8, (c >> 16) as u8, 0])
            .collect(),
        mapping: Vec::new(),
    };
    Kind::Palette { palette }
}

/// A new palette of the colors `colors` for the system's own drawing
/// (`gdi32::add_system`): the end of the process running when it is
/// made leaves it, and its maker deletes it.
pub fn system_palette_of(cpu: &mut Cpu, colors: &[u32]) -> u32 {
    super::add_system(cpu, palette_of(colors))
}

/// GetPaletteEntries(hpal, start, count, entries): how many it copied,
/// or with no `entries` how many the palette has.
pub fn get_palette_entries(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (handle, start, count, out) = (args[0], args[1] as usize, args[2] as usize, args[3]);
    let Some(p) = gdi(cpu).palette(handle).cloned() else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0);
    };
    let n = p.entries.len();
    if out == 0 {
        return Some(n as u32);
    }
    if start >= n {
        return Some(0);
    }
    let end = (start + count).min(n);
    let words: Vec<u32> = p.entries[start..end].iter().map(|&e| word_of(e)).collect();
    write_words(cpu, out, &words);
    Some(words.len() as u32)
}

/// The palette `handle` changed or went: realizing it maps it again.
pub fn unrealize(cpu: &mut Cpu, handle: u32) {
    let state = gdi(cpu);
    if state.last_realized == handle {
        state.last_realized = 0;
    }
}

/// SetPaletteEntries(hpal, start, count, entries).
pub fn set_palette_entries(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (handle, start, count, at) = (args[0], args[1] as usize, args[2] as usize, args[3]);
    init(cpu);
    if handle == gdi(cpu).stock(DEFAULT_PALETTE) {
        return Some(0);
    }
    let Some(n) = gdi(cpu).palette(handle).map(|p| p.entries.len()) else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0);
    };
    if start >= n {
        return Some(0);
    }
    let end = (start + count).min(n);
    let words = read_words(cpu, at, end - start);
    if cpu.bus.guest_faulted() {
        return None;
    }
    if let Some(super::Object {
        kind: Kind::Palette { palette },
        ..
    }) = gdi(cpu).get_mut(handle)
    {
        for (i, w) in words.iter().enumerate() {
            palette.entries[start + i] = entry_of(*w);
        }
    }
    unrealize(cpu, handle);
    Some(words.len() as u32)
}

/// ResizePalette(hpal, entries).
pub fn resize_palette(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let n = args[1] as usize;
    if n == 0 || n > 0x1000 {
        return Some(FALSE);
    }
    match gdi(cpu).get_mut(args[0]) {
        Some(super::Object {
            kind: Kind::Palette { palette },
            ..
        }) => {
            palette.entries.resize(n, [0; 4]);
            palette.mapping.clear();
            unrealize(cpu, args[0]);
            Some(TRUE)
        }
        _ => {
            set_error(cpu, ERROR_INVALID_HANDLE);
            Some(FALSE)
        }
    }
}

/// A palette is deleted: it is no longer the primary one or the one
/// realized last.
pub fn palette_deleted(cpu: &mut Cpu, handle: u32) {
    let state = gdi(cpu);
    if state.primary_palette == handle {
        state.primary_palette = 0;
    }
    // Its entries stay taken until the next foreground realization, but
    // no longer as its own.
    for o in state.system_owner.iter_mut().filter(|o| **o == handle) {
        *o = 0;
    }
    unrealize(cpu, handle);
}

/// AnimatePalette(hpal, start, count, entries): only the entries that
/// are PC_RESERVED change. On a palette device, the entries of the
/// system palette they took change with them, and the screen at once.
pub fn animate_palette(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (handle, start, count, at) = (args[0], args[1] as usize, args[2] as usize, args[3]);
    let Some(n) = gdi(cpu).palette(handle).map(|p| p.entries.len()) else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    };
    if start >= n {
        return Some(FALSE);
    }
    let end = (start + count).min(n);
    let words = read_words(cpu, at, end - start);
    // The system palette's entries that changed, lowest and highest.
    let mut changed: Option<(usize, usize)> = None;
    let state = gdi(cpu);
    let mut system = std::mem::take(&mut state.system_palette);
    let uses = std::mem::take(&mut state.system_use);
    let owners = std::mem::take(&mut state.system_owner);
    if let Some(super::Object {
        kind: Kind::Palette { palette },
        ..
    }) = state.get_mut(handle)
    {
        for (i, w) in words.iter().enumerate() {
            let e = &mut palette.entries[start + i];
            if e[3] & PC_RESERVED == 0 {
                continue;
            }
            *e = entry_of(*w);
            if let Some(&m) = palette.mapping.get(start + i)
                && uses.get(m as usize) == Some(&USE_RESERVED)
                && owners.get(m as usize) == Some(&handle)
            {
                let m = m as usize;
                system[m][..3].copy_from_slice(&e[..3]);
                changed = Some(changed.map_or((m, m), |(lo, hi)| (lo.min(m), hi.max(m))));
            }
        }
    }
    state.system_palette = system;
    state.system_use = uses;
    state.system_owner = owners;
    if let Some((first, last)) = changed {
        show_system_entries(cpu, first, last);
    }
    Some(TRUE)
}

/// SelectPalette(hdc, hpal, background): the palette it had. As USER's
/// SelectPalette, it makes the palette the primary one when it isn't
/// selected as a background palette into a DC of the foreground window
/// or one of its children.
pub fn select_palette(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hdc, handle, background) = (args[0], args[1], args[2] & 0xFFFF != 0);
    if gdi(cpu).palette(handle).is_none() {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0);
    }
    let old = dc::with(cpu, hdc, |d| {
        std::mem::replace(&mut d.attrs.palette, handle)
    });
    let Some(old) = old else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0);
    };
    let hwnd = gdi(cpu).dc(hdc).map_or(0, |d| d.hwnd);
    if !background && handle != gdi(cpu).stock(DEFAULT_PALETTE) && hwnd != 0 {
        let user = &cpu.bus.win32.user;
        let foreground = user.foreground_window();
        if foreground != 0
            && (foreground == hwnd || user.is_descendant(foreground, hwnd))
            && gdi(cpu).primary_palette != handle
        {
            // A palette realized last in the background is realized
            // again, in the foreground.
            gdi(cpu).primary_palette = handle;
            unrealize(cpu, handle);
        }
    }
    dc::free_if_deleted(cpu, old);
    Some(old)
}

/// Realize the palette of the DC `hdc` on a palette device: how many of
/// its entries' mappings changed, and whether it is the primary palette.
fn realize(cpu: &mut Cpu, hdc: u32) -> (u32, bool) {
    let hpal = gdi(cpu).dc(hdc).map_or(0, |d| d.attrs.palette);
    let state = gdi(cpu);
    if hpal == state.stock(DEFAULT_PALETTE) {
        return (0, false);
    }
    let Some(palette) = state.palette(hpal).cloned() else {
        return (0, false);
    };
    if state.last_realized == hpal && palette.mapping.len() == palette.entries.len() {
        return (0, false);
    }
    state.last_realized = hpal;
    let primary = hpal == state.primary_palette;
    // How many entries of the other palettes' mappings point at each
    // entry of the system palette.
    let mut shared = [0u32; 256];
    for o in state.objects.iter().flatten() {
        if let Kind::Palette { palette } = &o.kind {
            for &m in &palette.mapping {
                shared[m as usize] += 1;
            }
        }
    }
    for &m in &palette.mapping {
        shared[m as usize] -= 1;
    }
    let mut uses = std::mem::take(&mut state.system_use);
    let mut owners = std::mem::take(&mut state.system_owner);
    owners.resize(256, 0);
    let mut system = std::mem::take(&mut state.system_palette);
    let before = system.clone();
    for i in 0..256 {
        // In the foreground every entry but the static ones is freed; in
        // the background, those the palette's last realization took
        // that no other palette maps to.
        let free = if primary {
            uses[i] != USE_STATIC
        } else {
            owners[i] == hpal && shared[i] == 0
        };
        if free {
            uses[i] = USE_FREE;
            owners[i] = 0;
        }
    }
    // Free entries are taken from the bottom up.
    let mut next_free = 0usize;
    let mut take = |uses: &mut [u8], system: &mut [[u8; 4]], e: [u8; 4], how: u8| {
        while next_free < 256 && uses[next_free] != USE_FREE {
            next_free += 1;
        }
        if next_free == 256 {
            return None;
        }
        uses[next_free] = how;
        owners[next_free] = hpal;
        system[next_free] = [e[0], e[1], e[2], 0];
        Some(next_free as u8)
    };
    let nearest_taken = |uses: &[u8], system: &[[u8; 4]], color: u32| {
        let taken: Vec<(u32, u8)> = (0..256)
            .filter(|&i| uses[i] == USE_STATIC || uses[i] == USE_SHARED)
            .map(|i| (rgb(system[i]), i as u8))
            .collect();
        nearest_of(&taken, color)
    };
    let mut mapping = Vec::with_capacity(palette.entries.len());
    for &e in &palette.entries {
        let color = rgb(e);
        let index = if e[3] & PC_EXPLICIT != 0 {
            e[0]
        } else if e[3] & (PC_RESERVED | PC_NOCOLLAPSE) != 0 {
            let how = if e[3] & PC_RESERVED != 0 {
                USE_RESERVED
            } else {
                USE_SHARED
            };
            take(&mut uses, &mut system, e, how)
                .unwrap_or_else(|| nearest_taken(&uses, &system, color))
        } else {
            let same = (0..256).find(|&i| {
                (uses[i] == USE_STATIC || uses[i] == USE_SHARED) && rgb(system[i]) == color
            });
            match same {
                Some(i) => i as u8,
                None => take(&mut uses, &mut system, e, USE_SHARED)
                    .unwrap_or_else(|| nearest_taken(&uses, &system, color)),
            }
        };
        mapping.push(index);
    }
    let changed = mapping
        .iter()
        .enumerate()
        .filter(|&(i, m)| palette.mapping.get(i) != Some(m))
        .count() as u32;
    // In a mode DirectDraw set, its palette may have put other colors in
    // the DAC: all of the system palette goes there.
    let differs = |i: &usize| system[*i] != before[*i];
    let shown = if crate::win32::ddraw::owns_display(cpu) {
        Some((0, 255))
    } else {
        (0..256).find(differs).zip((0..256).rev().find(differs))
    };
    let state = gdi(cpu);
    state.system_use = uses;
    state.system_owner = owners;
    state.system_palette = system;
    if let Some(super::Object {
        kind: Kind::Palette { palette },
        ..
    }) = state.get_mut(hpal)
    {
        palette.mapping = mapping;
    }
    if let Some((first, last)) = shown {
        show_system_entries(cpu, first, last);
    }
    (changed, primary)
}

/// RealizePalette(hdc): how many entries of its palette went into the
/// system palette. On a screen without a palette, none. When the primary
/// palette changed mappings in a window's DC, the top-level windows get
/// WM_PALETTECHANGED.
pub fn realize_palette(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let hdc = args[0];
    let Some(hwnd) = gdi(cpu).dc(hdc).map(|d| d.hwnd) else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0xFFFF_FFFF);
    };
    if !palette_device(cpu) {
        return Some(0);
    }
    let (changed, primary) = realize(cpu, hdc);
    if changed == 0 || !primary || hwnd == 0 {
        return Some(changed);
    }
    let vars = vec![super::CB_PALETTE_CHANGED, changed, hwnd, 0];
    crate::win32::task::start(cpu, super::TASK_CALLBACK, vars, 0)
}

/// RealizePalette for the system's own drawing, which runs outside any
/// call of the program's (as MCIAVI's drawing runs in a thread of its
/// own): the top-level windows get WM_PALETTECHANGED in their queues.
pub fn realize_posted(cpu: &mut Cpu, hdc: u32) {
    let Some(hwnd) = gdi(cpu).dc(hdc).map(|d| d.hwnd) else {
        return;
    };
    if !palette_device(cpu) {
        return;
    }
    let (changed, primary) = realize(cpu, hdc);
    if changed != 0 && primary && hwnd != 0 {
        use crate::win32::user32::message::{HWND_BROADCAST, WM_PALETTECHANGED, post_message};
        post_message(cpu, &[HWND_BROADCAST, WM_PALETTECHANGED, hwnd, 0]);
    }
}

/// GetSystemPaletteEntries(hdc, start, count, entries).
pub fn get_system_palette_entries(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (start, count, out) = (args[1] as usize, args[2] as usize, args[3]);
    init(cpu);
    let system = gdi(cpu).system_palette.clone();
    if out == 0 {
        return Some(system.len() as u32);
    }
    if start >= system.len() {
        return Some(0);
    }
    let end = (start + count).min(system.len());
    let words: Vec<u32> = system[start..end].iter().map(|&e| word_of(e)).collect();
    write_words(cpu, out, &words);
    Some(words.len() as u32)
}

/// GetSystemPaletteUse(hdc).
pub fn get_system_palette_use(cpu: &mut Cpu, _args: &[u32]) -> Option<u32> {
    init(cpu);
    if !palette_device(cpu) {
        return Some(SYSPAL_STATIC);
    }
    Some(gdi(cpu).static_use)
}

/// SetSystemPaletteUse(hdc, use): the use it had. A screen without a
/// palette keeps its static colors. On a palette device, SYSPAL_NOSTATIC
/// leaves black and white the only static colors, and SYSPAL_STATIC
/// puts the 20 back.
pub fn set_system_palette_use(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    init(cpu);
    if !palette_device(cpu) {
        return Some(SYSPAL_STATIC);
    }
    let new = args[1];
    if new != SYSPAL_STATIC && new != SYSPAL_NOSTATIC {
        return Some(SYSPAL_ERROR);
    }
    let state = gdi(cpu);
    let old = state.static_use;
    if old == new {
        return Some(old);
    }
    state.static_use = new;
    for i in 0..256 {
        let (was, now) = (is_static(old, i), is_static(new, i));
        if was && !now {
            state.system_use[i] = USE_FREE;
            state.system_owner[i] = 0;
        } else if now && !was {
            let c = static_colors()[if i < 10 { i } else { i - 236 }];
            state.system_palette[i] = entry_of(c);
            state.system_use[i] = USE_STATIC;
            state.system_owner[i] = 0;
        }
    }
    state.last_realized = 0;
    show_system_palette(cpu);
    Some(old)
}

/// GetNearestPaletteIndex(hpal, color).
pub fn get_nearest_palette_index(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(p) = gdi(cpu).palette(args[0]).cloned() else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(CLR_INVALID);
    };
    let colors: Vec<u32> = p.entries.iter().map(|&e| rgb(e)).collect();
    if colors.is_empty() {
        return Some(CLR_INVALID);
    }
    Some(super::surface::nearest(&colors, args[1] & 0xFF_FFFF))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halftone_palette_has_the_static_colors_and_the_cube() {
        let c = halftone_colors();
        let statics = static_colors();
        assert_eq!(c[..10], statics[..10]);
        assert_eq!(c[246..], statics[10..]);
        let cube = (0..216u32)
            .map(|i| (i % 6 * 51) | (i / 6 % 6 * 51) << 8 | (i / 36 * 51) << 16)
            .filter(|color| c.contains(color))
            .count();
        assert_eq!(cube, 212);
        // Windows 98's own order: greys, then other colours, then the
        // cube from (51, 0, 0).
        assert_eq!(c[10..14], [0x040404, 0x080808, 0x0C0C0C, 0x111111]);
        assert_eq!(c[22], 0x807CFF);
        assert_eq!(c[29], 0x000033);
    }

    /// The map of the palette `colors`, realized on a
    /// desktop of 256 colours, or of the default palette with none.
    fn map_of(colors: Option<&[u32]>) -> DeviceMap {
        let (mut cpu, hdc) = palette_machine();
        init(&mut cpu);
        let hpal = match colors {
            Some(colors) => add(&mut cpu, palette_of(colors)),
            None => gdi(&mut cpu).stock(DEFAULT_PALETTE),
        };
        select(&mut cpu, hdc, hpal);
        realize(&mut cpu, hdc);
        DeviceMap::new(&mut cpu, hpal)
    }

    /// A 24-bit pixel's colour, through `map`, as a colour.
    fn deep(map: &DeviceMap, colors: &[u32], color: u32) -> u32 {
        let i = map.deep_color(color) as usize;
        colors[map.map.iter().position(|&m| m as usize == i).unwrap()]
    }

    /// Windows 98 draws grey 127 of a 24-bit DIB as the halftone
    /// palette's grey 128 and grey 128 as 134, the entries nearest to
    /// their cells' middles (124 and 132).
    #[test]
    fn a_deep_pixel_takes_the_entry_nearest_its_cells_middle() {
        let colors = halftone_colors();
        let map = map_of(Some(&colors));
        let grey = |v: u32| v | v << 8 | v << 16;
        assert_eq!(deep(&map, &colors, grey(127)), grey(128));
        assert_eq!(deep(&map, &colors, grey(128)), grey(134));
        assert_eq!(deep(&map, &colors, grey(0)), grey(4));
        assert_eq!(deep(&map, &colors, 0x40FF40), 0x33FF33);
    }

    /// With the default palette Windows 98 draws a 24-bit DIB in the 16
    /// VGA colours: grey 64 to 167 as grey 128 (64 and 160 are as near
    /// black and silver), (168, 248, 248) as white though silver is
    /// nearer, and never the four other static colours.
    #[test]
    fn a_deep_pixel_takes_a_vga_colour_with_the_default_palette() {
        let colors = static_colors().to_vec();
        let map = map_of(None);
        let grey = |v: u32| v | v << 8 | v << 16;
        for (v, want) in [(63, 0), (64, 128), (167, 128), (168, 192), (224, 255)] {
            assert_eq!(deep(&map, &colors, grey(v)), grey(want), "grey {v}");
        }
        assert_eq!(deep(&map, &colors, 0xF8F8A8), 0xFFFFFF);
        assert_eq!(deep(&map, &colors, 0xF0FBFF), 0xFFFFFF);
        assert_eq!(deep(&map, &colors, 0xA4A0A0), 0x808080);
    }

    /// A machine with a desktop of 256 colours and a memory DC, which
    /// realizes its palette in the background.
    fn palette_machine() -> (Box<Cpu>, u32) {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        cpu.bus.win32.desktop_depth = crate::config::DesktopDepth::Palette;
        let hdc = dc::create_compatible_dc(&mut cpu, &[0]).unwrap();
        (cpu, hdc)
    }

    /// A palette of PC_RESERVED entries of the colors `colors`.
    fn reserved_palette(cpu: &mut Cpu, colors: impl Iterator<Item = u32>) -> u32 {
        let entries = colors.map(|c| entry_of(c | (PC_RESERVED as u32) << 24));
        let palette = Palette {
            entries: entries.collect(),
            mapping: Vec::new(),
        };
        add(cpu, Kind::Palette { palette })
    }

    fn select(cpu: &mut Cpu, hdc: u32, hpal: u32) {
        dc::with(cpu, hdc, |d| d.attrs.palette = hpal);
    }

    /// A fade in the background (SetPaletteEntries, then RealizePalette)
    /// gives back the entries the palette took the last time, so every
    /// frame gets entries of its own.
    #[test]
    fn a_background_realization_reuses_its_own_entries() {
        let (mut cpu, hdc) = palette_machine();
        let hpal = reserved_palette(&mut cpu, (0..200).map(|i| i << 8));
        select(&mut cpu, hdc, hpal);
        for frame in 1..4u32 {
            if let Some(super::super::Object {
                kind: Kind::Palette { palette },
                ..
            }) = gdi(&mut cpu).get_mut(hpal)
            {
                for (i, e) in palette.entries.iter_mut().enumerate() {
                    e[2] = frame as u8;
                    e[1] = i as u8;
                }
            }
            unrealize(&mut cpu, hpal);
            let (_, primary) = realize(&mut cpu, hdc);
            assert!(!primary);
            let state = gdi(&mut cpu);
            let mapping = state.palette(hpal).unwrap().mapping.clone();
            for (i, &m) in mapping.iter().enumerate() {
                assert_eq!(state.system_use[m as usize], USE_RESERVED, "frame {frame}");
                assert_eq!(state.system_palette[m as usize][2], frame as u8);
                assert_eq!(state.system_palette[m as usize][1], i as u8);
            }
        }
    }

    /// AnimatePalette changes the system entries a palette took only
    /// while they are still its own.
    #[test]
    fn animate_palette_leaves_another_palettes_entries() {
        let (mut cpu, hdc) = palette_machine();
        let a = reserved_palette(&mut cpu, [0x010101, 0x020202].into_iter());
        let b = reserved_palette(&mut cpu, [0x030303, 0x040404].into_iter());
        select(&mut cpu, hdc, a);
        realize(&mut cpu, hdc);
        let taken = gdi(&mut cpu).palette(a).unwrap().mapping.clone();
        // B becomes primary: every entry is freed, and B takes A's.
        gdi(&mut cpu).primary_palette = b;
        select(&mut cpu, hdc, b);
        realize(&mut cpu, hdc);
        assert_eq!(gdi(&mut cpu).palette(b).unwrap().mapping, taken);
        let at = 0x10000;
        write_words(&mut cpu, at, &[0x0105_0505, 0x0106_0606]);
        assert_eq!(animate_palette(&mut cpu, &[a, 0, 2, at]), Some(TRUE));
        let colors = system_colors(&mut cpu);
        let shown: Vec<u32> = taken.iter().map(|&m| colors[m as usize]).collect();
        assert_eq!(shown, [0x030303, 0x040404]);
        assert_eq!(animate_palette(&mut cpu, &[b, 0, 2, at]), Some(TRUE));
        let colors = system_colors(&mut cpu);
        let shown: Vec<u32> = taken.iter().map(|&m| colors[m as usize]).collect();
        assert_eq!(shown, [0x050505, 0x060606]);
    }

    #[test]
    fn the_static_colors_are_at_the_ends() {
        let u = static_uses(SYSPAL_STATIC);
        assert_eq!(u.iter().filter(|&&x| x == USE_STATIC).count(), 20);
        assert_eq!(
            (u[9], u[10], u[245], u[246]),
            (USE_STATIC, USE_FREE, USE_FREE, USE_STATIC)
        );
        let u = static_uses(SYSPAL_NOSTATIC);
        assert_eq!(u.iter().filter(|&&x| x == USE_STATIC).count(), 2);
    }

    /// The system palette's indices Windows 95 (OSR2, a Voodoo 3's
    /// driver, 640x480 in 256 colours) filled an 8x8 cell with, from a
    /// FillRect with a solid brush, read back with GetDIBits.
    const MEASURED_0_0_200: [[u8; 8]; 4] = [
        [4, 252, 4, 252, 4, 252, 4, 252],
        [252, 4, 252, 4, 252, 4, 252, 4],
        [4, 252, 4, 252, 4, 252, 4, 252],
        [252, 252, 252, 4, 252, 252, 252, 4],
    ];
    const MEASURED_200_100_50: [[u8; 8]; 8] = [
        [3, 249, 248, 249, 3, 249, 248, 249],
        [249, 248, 251, 248, 249, 248, 251, 248],
        [248, 249, 248, 249, 248, 249, 248, 249],
        [251, 249, 249, 248, 251, 249, 251, 248],
        [3, 249, 248, 249, 3, 249, 248, 249],
        [249, 248, 251, 248, 249, 248, 251, 248],
        [248, 249, 248, 249, 248, 249, 248, 249],
        [251, 249, 251, 248, 251, 249, 251, 248],
    ];

    #[test]
    fn a_brush_dithers_as_windows_95_draws_it() {
        let (mut cpu, _) = palette_machine();
        let map = DeviceMap::new(&mut cpu, 0);
        let blue = map.dither(0xC8_0000).unwrap();
        for (i, &v) in blue.iter().enumerate() {
            assert_eq!(v, MEASURED_0_0_200[i / 8 % 4][i % 8] as u32, "(0,0,200) at {i}");
        }
        let brown = map.dither(0x32_64C8).unwrap();
        for (i, &v) in brown.iter().enumerate() {
            assert_eq!(v, MEASURED_200_100_50[i / 8][i % 8] as u32, "(200,100,50) at {i}");
        }
    }

    #[test]
    fn the_static_colors_and_the_extremes_are_solid() {
        let (mut cpu, _) = palette_machine();
        let map = DeviceMap::new(&mut cpu, 0);
        for &c in static_colors() {
            assert_eq!(map.dither(c), None, "{c:06X}");
        }
        // A grey of 254 isn't static: 62 pixels white, the 2 of the
        // lowest thresholds grey.
        let near_white = map.dither(0xFE_FEFE).unwrap();
        assert_eq!(near_white.iter().filter(|&&v| v == 255).count(), 62);
        assert_eq!(near_white.iter().filter(|&&v| v == 248).count(), 2);
        assert_eq!(dither_levels(0x01_0101), [[0, 0, 0]; 64]);
    }

    #[test]
    fn a_dark_grey_lights_its_share_of_pixels() {
        // (64, 64, 64): h = 32 of 64, so half the pixels are 0x80 grey.
        let lit = dither_levels(0x40_4040).iter().filter(|&&l| l == [1, 1, 1]).count();
        assert_eq!(lit, 32);
    }
}
