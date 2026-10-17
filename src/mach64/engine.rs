//! The mach64's draw engine: rectangles (fills, blits, colour expansion,
//! patterns, polygon fills, the scaler's stretched images), lines and the
//! 3D engine's trapezoids, each carried out at once when the register
//! that starts it is written. An operation that takes its source from the
//! host waits for the data written to HOST_DATA, and only then is the
//! engine busy.
//!
//! The data path is [RRG] chapter 4's: the source picked for the
//! foreground or background by the mono source, mixed with the
//! destination, kept from the destination by the colour compare and the
//! write mask, inside the scissors. 24-bit pixels are drawn as three
//! bytes of 8 bits, the colours' bytes rotating through them.

use super::regs::*;
use super::tri::{Palettes, Pipe, Scaler};
use super::{Mach64, Target};
use crate::savestate::{Reader, Result, State, Writer};

/// What a register write started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// A rectangle: a fill or a blit (DST_HEIGHT_WIDTH, DST_X_WIDTH,
    /// DST_WIDTH).
    Fill,
    /// A line, or a trapezoid with DRAW_TRAP (DST_BRES_LNTH).
    Line,
    /// A context loaded from the memory (CONTEXT_LOAD_CNTL).
    Context,
    /// The 3D RAGE II's engine defaults (DP_SET_GUI_ENGINE).
    SetGuiEngine,
}

/// The size of a pixel in the memory, as the DP_*_PIX_WIDTH codes give
/// it: 24-bit pixels are bytes, YUV and AYUV 32-bit words.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Size {
    Bit,
    Nibble,
    #[default]
    Byte,
    Word,
    Dword,
}

impl Size {
    pub fn of(code: u32) -> Size {
        match code & 0xF {
            0 => Size::Bit,
            1 => Size::Nibble,
            3 | 4 | 15 => Size::Word,
            6 | 11 | 14 => Size::Dword,
            _ => Size::Byte,
        }
    }

    pub fn bits(self) -> u32 {
        match self {
            Size::Bit => 1,
            Size::Nibble => 4,
            Size::Byte => 8,
            Size::Word => 16,
            Size::Dword => 32,
        }
    }
}

/// The video memory as the engine addresses it: by pixel, in pixels of a
/// size, wrapping at its end. Sub-byte pixels are taken from the high
/// bits of a byte first, or from the low bits with `lsb_first`
/// (DP_BYTE_PIX_ORDER).
pub struct Memory<'a> {
    pub vram: &'a mut [u8],
    pub lsb_first: bool,
}

impl Memory<'_> {
    pub fn read(&self, at: u32, size: Size) -> u32 {
        let wrap = self.vram.len() - 1;
        let byte = |i: u32| self.vram[i as usize & wrap] as u32;
        match size {
            Size::Byte => byte(at),
            Size::Word => byte(at << 1) | byte((at << 1) + 1) << 8,
            Size::Dword => (0..4).map(|i| byte((at << 2) + i) << (8 * i)).sum(),
            Size::Nibble => byte(at >> 1) >> self.sub_shift(at, 4) & 0xF,
            Size::Bit => byte(at >> 3) >> self.sub_shift(at, 1) & 1,
        }
    }

    pub fn write(&mut self, at: u32, size: Size, value: u32) {
        let wrap = self.vram.len() - 1;
        match size {
            Size::Byte => self.vram[at as usize & wrap] = value as u8,
            Size::Word => {
                for i in 0..2 {
                    self.vram[((at << 1) + i) as usize & wrap] = (value >> (8 * i)) as u8;
                }
            }
            Size::Dword => {
                for i in 0..4 {
                    self.vram[((at << 2) + i) as usize & wrap] = (value >> (8 * i)) as u8;
                }
            }
            Size::Nibble | Size::Bit => {
                let bits = size.bits();
                let shift = self.sub_shift(at, bits);
                let byte = &mut self.vram[(at / (8 / bits)) as usize & wrap];
                let mask = (((1u32 << bits) - 1) << shift) as u8;
                *byte = *byte & !mask | ((value << shift) as u8 & mask);
            }
        }
    }

    /// Where in its byte sub-byte pixel `at` of `bits` bits is.
    fn sub_shift(&self, at: u32, bits: u32) -> u32 {
        let per = 8 / bits;
        let index = at % per;
        if self.lsb_first {
            index * bits
        } else {
            (per - 1 - index) * bits
        }
    }
}

/// The sources DP_SRC picks for the foreground and the background.
const SRC_BKGD: u32 = 0;
const SRC_FRGD: u32 = 1;
const SRC_HOST: u32 = 2;
const SRC_BLIT: u32 = 3;
const SRC_PATTERN: u32 = 4;
/// And for the mono source, which picks between them.
const MONO_PATTERN: u32 = 1;
const MONO_HOST: u32 = 2;
const MONO_BLIT: u32 = 3;

/// DST_CNTL's bits.
const Y_MAJOR: u32 = 1 << 2;
const X_TILE: u32 = 1 << 3;
const Y_TILE: u32 = 1 << 4;
const LAST_PEL: u32 = 1 << 5;
const POLYGON: u32 = 1 << 6;
const ROTATE_24: u32 = 1 << 7;
const BRES_SIGN: u32 = 1 << 11;
const RIGHT_EDGE_OFF: u32 = 1 << 12;
const TRAIL_X_DIR: u32 = 1 << 13;
const TRAP_FILL_DIR: u32 = 1 << 14;
const TRAIL_BRES_SIGN: u32 = 1 << 15;
const BRES_SIGN_AUTO: u32 = 1 << 17;
/// SRC_CNTL's.
const SRC_PATTERN_ON: u32 = 1;
const SRC_ROTATE: u32 = 2;
const SRC_LINEAR: u32 = 4;
const SRC_BYTE_ALIGN: u32 = 8;
const SRC_LINE_X_DIR: u32 = 0x10;
const SRC_TRACK_DST: u32 = 0x80;

/// Everything about an operation that comes from the registers when it
/// starts.
#[derive(Clone, Debug, Default)]
struct Setup {
    dst_size: Size,
    src_size: Size,
    host_size: Size,
    lsb_first: bool,
    /// Where the surfaces start, in pixels of their size, and how many
    /// pixels a row is.
    dst_base: u32,
    dst_pitch: i32,
    src_base: u32,
    src_pitch: i32,
    /// The source's start counted in bits: the mono source's and the
    /// polygon boundary's.
    src_bits: u32,
    mix_fg: u32,
    mix_bg: u32,
    src_fg: u32,
    src_bg: u32,
    mono: u32,
    /// PAT_REG0 and PAT_REG1's bytes: the mono pattern's rows, or the
    /// colour patterns' pixels.
    pattern: [u8; 8],
    scissors: (i32, i32, i32, i32),
    x_dir: i32,
    y_dir: i32,
    dst_cntl: u32,
    src_cntl: u32,
    pat_cntl: u32,
    host_cntl: u32,
    frgd: u32,
    bkgd: u32,
    write_mask: u32,
    chain_mask: u32,
    compare: (u32, u32, u32),
    /// SCALE_3D_FCN: the scaler (1) or the 3D engine (2, 3).
    function: u32,
}

impl Setup {
    fn new(card: &Mach64) -> Setup {
        let r = |reg| card.reg(reg);
        let width = r(DP_PIX_WIDTH);
        let (dst_size, src_size) = (Size::of(width), Size::of(width >> 8));
        let pixels = |off_pitch: u32, size: Size| (off_pitch & 0xF_FFFF) * 64 / size.bits();
        let pitch = |off_pitch: u32| ((off_pitch >> 22 & 0x3FF) * 8) as i32;
        let mut pattern = [0; 8];
        pattern[..4].copy_from_slice(&r(PAT_REG0).to_le_bytes());
        pattern[4..].copy_from_slice(&r(PAT_REG1).to_le_bytes());
        Setup {
            dst_size,
            src_size,
            host_size: Size::of(width >> 16),
            lsb_first: width & 1 << 24 != 0,
            dst_base: pixels(r(DST_OFF_PITCH), dst_size),
            dst_pitch: pitch(r(DST_OFF_PITCH)),
            src_base: pixels(r(SRC_OFF_PITCH), src_size),
            src_pitch: pitch(r(SRC_OFF_PITCH)),
            src_bits: pixels(r(SRC_OFF_PITCH), Size::Bit),
            mix_fg: r(DP_MIX) >> 16 & 0x1F,
            mix_bg: r(DP_MIX) & 0x1F,
            src_fg: r(DP_SRC) >> 8 & 7,
            src_bg: r(DP_SRC) & 7,
            mono: r(DP_SRC) >> 16 & 3,
            pattern,
            scissors: (
                card.signed(SC_LEFT, 13),
                card.signed(SC_RIGHT, 13),
                card.signed(SC_TOP, 15),
                card.signed(SC_BOTTOM, 15),
            ),
            x_dir: if r(DST_CNTL) & 1 != 0 { 1 } else { -1 },
            y_dir: if r(DST_CNTL) & 2 != 0 { 1 } else { -1 },
            dst_cntl: r(DST_CNTL),
            src_cntl: r(SRC_CNTL),
            pat_cntl: r(PAT_CNTL),
            host_cntl: r(HOST_CNTL),
            frgd: r(DP_FRGD_CLR),
            bkgd: r(DP_BKGD_CLR),
            write_mask: r(DP_WRITE_MSK),
            chain_mask: r(DP_CHAIN_MSK),
            compare: (r(CLR_CMP_CLR), r(CLR_CMP_MSK), r(CLR_CMP_CNTL)),
            function: r(SCALE_3D_CNTL) >> 6 & 3,
        }
    }

    /// Whether the operation takes its source, or the mono source, from
    /// HOST_DATA.
    fn takes_host(&self) -> bool {
        self.src_fg == SRC_HOST || self.src_bg == SRC_HOST || self.mono == MONO_HOST
    }

    /// The width of a host data element: a bit for a mono source, else a
    /// host pixel.
    fn host_bits(&self) -> u32 {
        if self.mono == MONO_HOST {
            1
        } else {
            self.host_size.bits()
        }
    }

    fn inside(&self, x: i32, y: i32) -> bool {
        let (left, right, top, bottom) = self.scissors;
        x >= left && x <= right && y >= top && y <= bottom
    }

    /// Where destination pixel `x`, `y` is.
    fn dst_at(&self, x: i32, y: i32) -> u32 {
        self.dst_base.wrapping_add((y * self.dst_pitch + x) as u32)
    }

    /// The mono pattern's bit at `x`, `y`: rows of a byte each, from bit 7
    /// (or bit 0 with the low bits first).
    fn pattern_bit(&self, x: i32, y: i32) -> bool {
        let bit = (x & 7) as u32;
        let bit = if self.lsb_first { bit } else { 7 - bit };
        self.pattern[(y & 7) as usize] >> bit & 1 != 0
    }

    /// The pattern source's colour at `x`, `y`: the 4x2 or 8x1 colour
    /// pattern, else the foreground.
    fn pattern_colour(&self, x: i32, y: i32) -> u32 {
        if self.pat_cntl & 2 != 0 {
            self.pattern[(4 * (y & 1) + (x & 3)) as usize] as u32
        } else if self.pat_cntl & 4 != 0 {
            self.pattern[(x & 7) as usize] as u32
        } else {
            self.frgd
        }
    }

    /// Whether the colour compare keeps the destination where `src` would
    /// go over `dst`. Against the destination only the pixel's bits are
    /// compared: drivers write a 16-bit key twice into the register.
    fn compare_keeps(&self, src: u32, dst: u32) -> bool {
        let (colour, mask, cntl) = self.compare;
        let source = cntl >> 24 & 3;
        let bits = self.dst_size.bits();
        let pixel_mask = if source != 0 || bits >= 32 {
            u32::MAX
        } else {
            (1 << bits) - 1
        };
        let mask = mask & pixel_mask;
        let value = if source == 0 { dst } else { src };
        match cntl & 7 {
            1 => true,
            4 => value & mask != colour & mask,
            5 => value & mask == colour & mask,
            _ => false,
        }
    }

    /// Whether the colour compare is against the destination, as the 3D
    /// engine's pixels are tested before the pipe.
    fn compares_destination(&self) -> bool {
        self.compare.2 >> 24 & 3 == 0
    }

    /// Mix `mix` of source `s` and destination `d`: the 16 logical
    /// operations, and 17h the average of the two, its carries broken
    /// where DP_CHAIN_MSK has a 1. The saturating mixes (10h-16h,
    /// 18h-1Fh) take the source.
    fn mix(&self, mix: u32, s: u32, d: u32) -> u32 {
        match mix {
            0x0 => !d,
            0x1 => 0,
            0x2 => u32::MAX,
            0x3 => d,
            0x4 => !s,
            0x5 => s ^ d,
            0x6 => !(s ^ d),
            0x7 => s,
            0x8 => !(s & d),
            0x9 => !s | d,
            0xA => s | !d,
            0xB => s | d,
            0xC => s & d,
            0xD => s & !d,
            0xE => !s & d,
            0xF => !(s | d),
            0x17 => self.average(s, d),
            _ => s,
        }
    }

    fn average(&self, s: u32, d: u32) -> u32 {
        let bits = self.dst_size.bits().clamp(8, 32);
        let mut chain = self.chain_mask & 0x7FFF | 0x8000;
        if bits < 16 {
            chain |= 1 << (bits - 1);
        }
        let (mut out, mut from) = (0u32, 0);
        for bit in 0..bits {
            if chain >> (bit & 15) & 1 != 0 || bit == bits - 1 {
                let width = bit - from + 1;
                let mask = if width >= 32 {
                    u32::MAX
                } else {
                    (1 << width) - 1
                };
                let sum = ((s >> from) & mask) as u64 + ((d >> from) & mask) as u64;
                out |= ((sum >> 1) as u32 & mask) << from;
                from = bit + 1;
            }
        }
        out
    }

    /// Write `src` at `x`, `y` through the data path: the scissors, the
    /// colour compare, the foreground or background mix, the write mask
    /// (its byte `rot` for a 24-bit pixel's).
    fn put(&self, mem: &mut Memory, x: i32, y: i32, foreground: bool, src: u32, rot: Option<u32>) {
        if !self.inside(x, y) {
            return;
        }
        let at = self.dst_at(x, y);
        let dst = mem.read(at, self.dst_size);
        if self.compare_keeps(src, dst) {
            return;
        }
        let mask = rot.map_or(self.write_mask, |r| self.write_mask >> (8 * r) & 0xFF);
        let out = self.mix(if foreground { self.mix_fg } else { self.mix_bg }, src, dst);
        mem.write(at, self.dst_size, out & mask | dst & !mask);
    }

    /// Write what the 3D pipe or the scaler made of `dst` at `at`: through
    /// the write mask only.
    fn put_shaded(&self, mem: &mut Memory, at: u32, dst: u32, out: u32) {
        mem.write(
            at,
            self.dst_size,
            out & self.write_mask | dst & !self.write_mask,
        );
    }

    /// Where source pixel `sx`, `sy` is in units of `size` from `base`:
    /// the `sx`th of a linear source.
    fn src_at(&self, base: u32, sx: i32, sy: i32) -> u32 {
        if self.src_cntl & SRC_LINEAR != 0 {
            base.wrapping_add(sx as u32)
        } else {
            base.wrapping_add((sy * self.src_pitch + sx) as u32)
        }
    }

    /// The colour source `which` gives at destination `x`, `y` and source
    /// `sx`, `sy`, the byte `rot` of a 24-bit pixel.
    #[allow(clippy::too_many_arguments)]
    fn source(
        &self,
        mem: &Memory,
        which: u32,
        x: i32,
        y: i32,
        sx: i32,
        sy: i32,
        host: u32,
        rot: Option<u32>,
    ) -> u32 {
        let byte = |colour: u32| rot.map_or(colour, |r| colour >> (8 * r) & 0xFF);
        match which {
            SRC_HOST => host,
            SRC_BLIT => mem.read(self.src_at(self.src_base, sx, sy), self.src_size),
            SRC_FRGD => byte(self.frgd),
            SRC_PATTERN => self.pattern_colour(x, y),
            _ => byte(self.bkgd),
        }
    }

    /// The mono source's choice at a pixel: the foreground or not.
    fn foreground(&self, mem: &Memory, x: i32, y: i32, sx: i32, sy: i32, host: u32) -> bool {
        match self.mono {
            MONO_HOST => host & 1 != 0,
            MONO_PATTERN => self.pattern_bit(x, y),
            MONO_BLIT => mem.read(self.src_at(self.src_bits, sx, sy), Size::Bit) != 0,
            _ => true,
        }
    }
}

/// Counts that never run out: no wrapping.
const ENDLESS: i32 = i32::MAX;

/// A rectangle in progress: where its walk is.
#[derive(Clone, Debug, Default)]
struct Rect {
    setup: Setup,
    x0: i32,
    y0: i32,
    width: i32,
    rows_left: i32,
    col: i32,
    row: i32,
    /// The source's position from its start, the start, and how many
    /// pixels and rows are left before it wraps.
    src_x: i32,
    src_y: i32,
    src_x0: i32,
    src_y0: i32,
    src_cols_left: i32,
    src_rows_left: i32,
    /// SRC_X and SRC_Y, and their second starts (SRC_X_START and
    /// SRC_Y_START) for a rotating pattern.
    src_starts: (i32, i32, i32, i32),
    widths: (i32, i32),
    heights: (i32, i32),
    rot0: u32,
    rot: u32,
    inside_polygon: bool,
    /// The row just ended, for the host data's byte alignment.
    row_ended: bool,
}

impl Rect {
    fn new(card: &Mach64) -> Rect {
        let setup = Setup::new(card);
        let wrap = |n: u32| if n == 0 { ENDLESS } else { n as i32 };
        let widths = (wrap(card.reg(SRC_WIDTH1)), wrap(card.reg(SRC_WIDTH2)));
        let heights = (wrap(card.reg(SRC_HEIGHT1)), wrap(card.reg(SRC_HEIGHT2)));
        let x0 = card.signed(DST_X, 13);
        let src_starts = (
            card.signed(SRC_X, 13),
            card.signed(SRC_Y, 15),
            card.signed(SRC_X_START, 13),
            card.signed(SRC_Y_START, 15),
        );
        Rect {
            x0,
            y0: card.signed(DST_Y, 15),
            width: (card.reg(DST_WIDTH) & 0x1FFF) as i32,
            rows_left: (card.reg(DST_HEIGHT) & 0x7FFF) as i32,
            src_x0: src_starts.0,
            src_y0: src_starts.1,
            src_cols_left: if setup.src_cntl & SRC_LINEAR != 0 {
                ENDLESS
            } else {
                widths.0
            },
            src_rows_left: if setup.src_cntl & SRC_PATTERN_ON != 0 {
                heights.0
            } else {
                ENDLESS
            },
            src_starts,
            widths,
            heights,
            // The byte a 24-bit row starts with: DST_24_ROT, and where in
            // its pixel the first byte is.
            rot0: ((card.reg(DST_CNTL) >> 8 & 7) * 4 + (x0 & 3) as u32) % 3,
            rot: ((card.reg(DST_CNTL) >> 8 & 7) * 4 + (x0 & 3) as u32) % 3,
            setup,
            ..Default::default()
        }
    }

    fn empty(&self) -> bool {
        self.width <= 0 || self.rows_left <= 0
    }

    /// One pixel, with host data `host`; true once the rectangle is done.
    fn pixel(
        &mut self,
        mem: &mut Memory,
        palettes: &Palettes,
        scaler: Option<&Scaler>,
        host: u32,
    ) -> bool {
        let s = &self.setup;
        let rot = (s.dst_cntl & ROTATE_24 != 0).then_some(self.rot);
        let (x, y) = (self.x0 + self.col * s.x_dir, self.y0 + self.row * s.y_dir);
        let (mut sx, mut sy) = if s.src_cntl & SRC_LINEAR != 0 {
            (self.src_x, 0)
        } else {
            (self.src_x0 + self.src_x, self.src_y0 + self.src_y)
        };
        if s.src_cntl & SRC_TRACK_DST != 0 {
            (sx, sy) = (x, y);
        }
        let foreground = match rot {
            // The pattern's pixels are whole 24-bit pixels.
            Some(_) if s.mono == MONO_PATTERN => {
                s.pattern_bit((x - if x < 0 { 2 } else { 0 }) / 3, y)
            }
            _ => s.foreground(mem, x, y, sx, sy, host),
        };
        let mut draw = true;
        // Polygon fill: the boundary's bits toggle the fill along a row;
        // the closing pixel is left out with DST_POLYGON_RTEDGE_DIS.
        if s.dst_cntl & POLYGON != 0 {
            let was = self.inside_polygon;
            let boundary = mem.read(
                s.src_bits.wrapping_add((sy * s.src_pitch + sx) as u32),
                Size::Bit,
            ) != 0;
            self.inside_polygon ^= boundary;
            draw = was || self.inside_polygon;
            if boundary && was && !self.inside_polygon && s.dst_cntl & RIGHT_EDGE_OFF != 0 {
                draw = false;
            }
        }
        if draw {
            match scaler {
                Some(scaler) if s.inside(x, y) => {
                    let at = s.dst_at(x, y);
                    let dst = mem.read(at, s.dst_size);
                    if let Some(out) = scaler.pixel(mem, palettes, self.col, self.row, dst) {
                        s.put_shaded(mem, at, dst, out);
                    }
                }
                Some(_) => {}
                None => {
                    let which = if foreground { s.src_fg } else { s.src_bg };
                    let src = s.source(mem, which, x, y, sx, sy, host, rot);
                    s.put(mem, x, y, foreground, src, rot);
                }
            }
        }
        self.advance()
    }

    /// On to the next pixel; true when the rectangle is done.
    fn advance(&mut self) -> bool {
        let s = &self.setup;
        let linear = s.src_cntl & SRC_LINEAR != 0;
        let rotating = s.src_cntl & (SRC_PATTERN_ON | SRC_ROTATE) == SRC_PATTERN_ON | SRC_ROTATE;
        if linear {
            self.src_x += 1;
        } else {
            self.src_x += s.x_dir;
            self.src_cols_left -= 1;
            if self.src_cols_left <= 0 {
                self.src_x = 0;
                self.src_cols_left = self.widths.0;
                if rotating {
                    self.src_x0 = self.src_starts.2;
                    self.src_cols_left = self.widths.1;
                }
            }
        }
        self.rot = if s.x_dir > 0 {
            (self.rot + 1) % 3
        } else {
            (self.rot + 2) % 3
        };
        self.col += 1;
        if self.col < self.width {
            return false;
        }
        // The end of a row.
        self.col = 0;
        self.row += 1;
        self.rot = self.rot0;
        self.inside_polygon = false;
        self.row_ended = true;
        self.src_x0 = self.src_starts.0;
        if linear {
            self.src_cols_left = ENDLESS;
            // SRC_BYTE_ALIGN: a 1- or 4-bit source starts each row at a
            // byte.
            let bits = if s.mono == MONO_BLIT {
                1
            } else {
                s.src_size.bits()
            };
            if s.src_cntl & SRC_BYTE_ALIGN != 0 && bits < 8 {
                let per = 8 / bits;
                let base = if bits == 1 { s.src_bits } else { s.src_base };
                let at = base.wrapping_add(self.src_x as u32);
                self.src_x = (at.div_ceil(per) * per).wrapping_sub(base) as i32;
            }
        } else {
            self.src_x = 0;
            self.src_cols_left = self.widths.0;
            self.src_y += s.y_dir;
            self.src_rows_left -= 1;
            if self.src_rows_left <= 0 {
                self.src_y = 0;
                self.src_rows_left = self.heights.0;
                if rotating {
                    self.src_y0 = self.src_starts.3;
                    self.src_rows_left = self.heights.1;
                }
            }
        }
        self.rows_left -= 1;
        self.rows_left <= 0
    }

    /// DST_X_TILE and DST_Y_TILE: the destination moves past the
    /// rectangle when it is done.
    fn tile(&self, card: &mut Mach64) {
        let s = &self.setup;
        if s.dst_cntl & X_TILE != 0 {
            let x = card.signed(DST_X, 13) + s.x_dir * (card.reg(DST_WIDTH) & 0x1FFF) as i32;
            card.set_reg(DST_X, x as u32);
        }
        if s.dst_cntl & Y_TILE != 0 {
            let y = card.signed(DST_Y, 15) + s.y_dir * (card.reg(DST_HEIGHT) & 0x7FFF) as i32;
            card.set_reg(DST_Y, y as u32);
        }
    }
}

/// Whether a Bresenham error counts as positive: a zero error does
/// unless the sign bit says otherwise, or with BRES_SIGN_AUTO when the
/// minor direction is negative.
fn positive(err: i32, setup: &Setup, sign_bit: u32) -> bool {
    if err != 0 {
        return err > 0;
    }
    if setup.dst_cntl & BRES_SIGN_AUTO != 0 {
        return if setup.dst_cntl & Y_MAJOR != 0 {
            setup.x_dir < 0
        } else {
            setup.y_dir < 0
        };
    }
    setup.dst_cntl & sign_bit == 0
}

/// A line in progress.
struct Line {
    setup: Setup,
    x: i32,
    y: i32,
    src_x: i32,
    src_y: i32,
    left: i32,
    err: i32,
    inc: i32,
    dec: i32,
    /// A shaded line: its colours from the 3D pipe, which steps with it.
    pipe: Option<Pipe>,
}

impl Line {
    fn new(card: &Mach64) -> Line {
        let setup = Setup::new(card);
        let shaded = setup.function >= 2 && setup.src_fg == 5;
        Line {
            x: card.signed(DST_X, 13),
            y: card.signed(DST_Y, 15),
            src_x: card.signed(SRC_X, 13),
            src_y: card.signed(SRC_Y, 15),
            left: (card.reg(DST_BRES_LNTH) & 0x7FFF) as i32,
            err: card.signed(DST_BRES_ERR, 18),
            inc: card.signed(DST_BRES_INC, 18),
            dec: card.signed(DST_BRES_DEC, 18),
            pipe: shaded.then(|| Pipe::new(card)),
            setup,
        }
    }

    /// One pixel; true once the line is done. Its last pixel is drawn
    /// with DST_LAST_PEL; a polygon outline draws one pixel a scanline,
    /// no further left than the left scissor.
    fn pixel(&mut self, mem: &mut Memory, palettes: &Palettes, host: u32) -> bool {
        let s = &self.setup;
        let y_major = s.dst_cntl & Y_MAJOR != 0;
        let positive = positive(self.err, s, BRES_SIGN);
        let mut x = self.x;
        let mut draw = self.left > 1 || s.dst_cntl & LAST_PEL != 0;
        if s.dst_cntl & POLYGON != 0 {
            draw &= y_major || positive;
            x = x.max(s.scissors.0);
        }
        if draw {
            match &mut self.pipe {
                Some(pipe) => {
                    if s.inside(x, self.y) && self.y >= 0 {
                        let at = s.dst_at(x, self.y);
                        let dst = mem.read(at, s.dst_size);
                        pipe.span(0, s.x_dir);
                        let keep = s.compares_destination() && s.compare_keeps(0, dst);
                        if let Some(out) = (!keep)
                            .then(|| pipe.pixel(mem, palettes, x, self.y, dst))
                            .flatten()
                        {
                            s.put_shaded(mem, at, dst, out);
                        }
                    }
                }
                None => {
                    let foreground = s.foreground(mem, x, self.y, self.src_x, self.src_y, host);
                    let which = if foreground { s.src_fg } else { s.src_bg };
                    let src = s.source(mem, which, x, self.y, self.src_x, self.src_y, host, None);
                    s.put(mem, x, self.y, foreground, src, None);
                }
            }
        }
        self.left -= 1;
        if self.left <= 0 {
            return true;
        }
        self.src_x += if s.src_cntl & SRC_LINE_X_DIR != 0 {
            1
        } else {
            -1
        };
        let (x0, y0) = (self.x, self.y);
        if y_major {
            self.y += s.y_dir;
        } else {
            self.x += s.x_dir;
        }
        if positive {
            self.err += self.dec;
            if y_major {
                self.x += s.x_dir;
            } else {
                self.y += s.y_dir;
            }
        } else {
            self.err += self.inc;
        }
        if let Some(pipe) = &mut self.pipe {
            if self.x != x0 {
                pipe.step_x();
            }
            if self.y != y0 {
                pipe.step_y();
            }
        }
        false
    }

    /// Where the line stopped: DST_X and DST_Y on its last pixel, the
    /// error, and a shaded line's interpolators.
    fn finish(&self, card: &mut Mach64) {
        card.set_reg(DST_X, self.x as u32);
        card.set_reg(DST_Y, self.y as u32);
        card.set_reg(DST_BRES_ERR, self.err as u32);
        if let Some(pipe) = &self.pipe {
            pipe.write_back(card);
        }
    }
}

/// An edge of a trapezoid: where it is, its Bresenham terms, its
/// direction and whether a zero error is negative.
struct Edge {
    x: i32,
    err: i32,
    inc: i32,
    dec: i32,
    dir: i32,
    zero_negative: bool,
}

impl Edge {
    fn positive(&self) -> bool {
        self.err > 0 || (self.err == 0 && !self.zero_negative)
    }

    /// Down a scanline: INC once, then while the error is positive a
    /// pixel across and DEC, so an edge flatter than 45 degrees moves
    /// several pixels. Returns the pixels it moved. Terms whose DEC
    /// isn't negative step once.
    fn step(&mut self) -> i32 {
        let x0 = self.x;
        self.err += self.inc;
        for _ in 0..8192 {
            if !self.positive() {
                break;
            }
            self.x += self.dir;
            self.err += self.dec;
            if self.dec >= 0 {
                break;
            }
        }
        self.x - x0
    }

    /// An edge whose error is still positive when the trapezoid starts
    /// hasn't been stepped into place: it is, as each scanline's is,
    /// before the first row. Returns the pixels it moved.
    fn settle(&mut self) -> i32 {
        let mut moved = 0;
        while moved < 8192 && self.positive() && self.dec < 0 {
            self.x += self.dir;
            self.err += self.dec;
            moved += 1;
        }
        moved
    }
}

/// A trapezoid (DRAW_TRAP): from DST_Y down DST_BRES_LNTH scanlines,
/// between the leading edge (DST_X, the DST_BRES terms) and the trailing
/// edge (TRAIL_X in DST_HEIGHT, the TRAIL_BRES terms), filled flat with
/// the foreground source, or by the 3D engine. The registers are where
/// it stopped afterwards: ATI's Direct3D HAL draws a triangle's second
/// trapezoid by loading only the new trailing edge.
fn trapezoid(card: &mut Mach64, mem: &mut Memory, palettes: &Palettes) {
    let s = Setup::new(card);
    let edge = |x: i32, err: usize, dir_bit: u32, sign_bit: u32| Edge {
        x,
        err: card.signed(err, 18),
        inc: card.signed(err + 1, 18),
        dec: card.signed(err + 2, 18),
        dir: if s.dst_cntl & dir_bit != 0 { 1 } else { -1 },
        zero_negative: s.dst_cntl & sign_bit != 0,
    };
    let mut lead = edge(card.signed(DST_X, 13), DST_BRES_ERR, 1, BRES_SIGN);
    let mut trail = edge(
        card.signed(DST_HEIGHT, 13),
        TRAIL_BRES_ERR,
        TRAIL_X_DIR,
        TRAIL_BRES_SIGN,
    );
    let mut y = card.signed(DST_Y, 15);
    let rows = (card.reg(DST_BRES_LNTH) & 0x7FFF) as i32;
    // TRAP_FILL_DIR: the trailing edge is on the right.
    let trail_right = s.dst_cntl & TRAP_FILL_DIR != 0;
    let mut pipe = (s.function >= 2).then(|| Pipe::new(card));
    let fill_src = s.src_fg;
    // The span never leaves the pitch or the scissors.
    let left = s.scissors.0.max(0);
    let right = if s.dst_pitch > 0 {
        s.scissors.1.min(s.dst_pitch - 1)
    } else {
        s.scissors.1
    };
    trail.settle();
    for _ in 0..lead.settle() {
        if let Some(pipe) = &mut pipe {
            pipe.step_x();
        }
    }
    for _ in 0..rows {
        let (lo, mut hi) = if trail_right {
            (lead.x, trail.x)
        } else {
            (trail.x, lead.x)
        };
        // The right edge's pixel is drawn, unless DST_POLYGON_RTEDGE_DIS
        // leaves it out, or it is a vertical trailing edge's (ATI3DCIF's
        // text quads end one past their glyph on one).
        if s.dst_cntl & RIGHT_EDGE_OFF != 0 || (trail_right && trail.inc == 0) {
            hi -= 1;
        }
        let (from, to) = (lo.max(left), hi.min(right));
        if from <= to && y >= s.scissors.2 && y <= s.scissors.3 && y >= 0 {
            match &mut pipe {
                Some(pipe) => {
                    // The interpolators count from the leading edge, in
                    // its direction.
                    pipe.span((from - lead.x) * lead.dir, lead.dir);
                    for x in from..=to {
                        let at = s.dst_at(x, y);
                        let dst = mem.read(at, s.dst_size);
                        let keep = s.compares_destination() && s.compare_keeps(0, dst);
                        if let Some(out) = (!keep)
                            .then(|| pipe.pixel(mem, palettes, x, y, dst))
                            .flatten()
                        {
                            // A foreground source of the background or
                            // foreground colour stamps it where the pipe
                            // drew (ATI3DCIF's sprite masks).
                            let out = match fill_src {
                                SRC_BKGD => s.bkgd,
                                SRC_FRGD => s.frgd,
                                _ => out,
                            };
                            s.put_shaded(mem, at, dst, out);
                        }
                        pipe.span_step();
                    }
                }
                None => {
                    for x in from..=to {
                        let src = s.source(mem, fill_src, x, y, x, y, 0, None);
                        s.put(mem, x, y, true, src, None);
                    }
                }
            }
        }
        let moved = lead.step();
        trail.step();
        if let Some(pipe) = &mut pipe {
            pipe.step_y();
            for _ in 0..moved.abs() {
                pipe.step_x();
            }
        }
        y += s.y_dir;
    }
    card.set_reg(DST_X, lead.x as u32);
    card.set_reg(DST_Y, y as u32);
    card.set_reg(
        DST_HEIGHT,
        card.reg(DST_HEIGHT) & !0x1FFF | trail.x as u32 & 0x1FFF,
    );
    card.set_reg(DST_BRES_ERR, lead.err as u32);
    card.set_reg(TRAIL_BRES_ERR, trail.err as u32);
    if let Some(pipe) = &pipe {
        pipe.write_back(card);
    }
}

/// An operation that waits for host data.
enum Waiting {
    Rect(Box<Rect>),
    Line(Box<Line>),
}

pub struct Engine {
    /// The 3D RAGE II's texture palette, RGB888.
    pub palette: Vec<u32>,
    waiting: Option<Waiting>,
    /// Operations started, for the debugger.
    pub ops: u64,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for Engine {
    /// A copy without the operation waiting for host data.
    fn clone(&self) -> Self {
        Self {
            palette: self.palette.clone(),
            waiting: None,
            ops: self.ops,
        }
    }
}

impl Engine {
    pub fn new() -> Engine {
        Engine {
            palette: vec![0; 256],
            waiting: None,
            ops: 0,
        }
    }

    /// Back to idle, the operation in progress dropped.
    pub fn reset(&mut self) {
        self.waiting = None;
    }

    /// Whether an operation waits for host data.
    pub fn busy(&self) -> bool {
        self.waiting.is_some()
    }

    /// Start `op` with the registers of `card`, drawing into `target`.
    /// True if it drew.
    pub fn start(&mut self, op: Op, card: &mut Mach64, target: &mut Target) -> bool {
        // A new operation drops one that waited for host data.
        self.waiting = None;
        self.ops += 1;
        match op {
            Op::Context => return self.load_context(card, target),
            Op::SetGuiEngine => {
                set_gui_engine(card, target);
                return false;
            }
            _ => {}
        }
        let lsb_first = card.reg(DP_PIX_WIDTH) & 1 << 24 != 0;
        let texture = card.rage2.then_some(&self.palette[..]);
        let palettes = Palettes {
            dac: target.dac,
            dac_8bit: target.dac_8bit,
            texture,
        };
        let mut mem = Memory {
            vram: target.vram,
            lsb_first,
        };
        match op {
            Op::Fill => {
                let mut rect = Rect::new(card);
                card.trace(|| {
                    format!(
                        "fill {}x{} at {},{}",
                        rect.width, rect.rows_left, rect.x0, rect.y0
                    )
                });
                if rect.empty() {
                    return false;
                }
                if rect.setup.takes_host() {
                    self.waiting = Some(Waiting::Rect(Box::new(rect)));
                    return false;
                }
                let scaler = if rect.setup.function == 1 {
                    Scaler::new(card)
                } else {
                    None
                };
                if rect.setup.function == 1 && scaler.is_none() {
                    return false;
                }
                while !rect.pixel(&mut mem, &palettes, scaler.as_ref(), 0) {}
                rect.tile(card);
                true
            }
            Op::Line if card.reg(DST_BRES_LNTH) & 0x8000 != 0 => {
                card.trace(|| "trapezoid".into());
                trapezoid(card, &mut mem, &palettes);
                true
            }
            Op::Line => {
                let mut line = Line::new(card);
                if line.left <= 0 {
                    return false;
                }
                if line.setup.takes_host() {
                    self.waiting = Some(Waiting::Line(Box::new(line)));
                    return false;
                }
                while !line.pixel(&mut mem, &palettes, 0) {}
                line.finish(card);
                true
            }
            Op::Context | Op::SetGuiEngine => false,
        }
    }

    /// `bits` of host data for the operation waiting for it: its elements
    /// (a pixel, or a bit of a mono source) one after another. True if it
    /// drew.
    pub fn host_data(
        &mut self,
        data: u32,
        bits: u32,
        card: &mut Mach64,
        target: &mut Target,
    ) -> bool {
        let Some(mut waiting) = self.waiting.take() else {
            return false;
        };
        let texture = card.rage2.then_some(&self.palette[..]);
        let palettes = Palettes {
            dac: target.dac,
            dac_8bit: target.dac_8bit,
            texture,
        };
        let (setup, rect_op) = match &waiting {
            Waiting::Rect(rect) => (&rect.setup, true),
            Waiting::Line(line) => (&line.setup, false),
        };
        let mut mem = Memory {
            vram: target.vram,
            lsb_first: setup.lsb_first,
        };
        let elements = host_elements(setup, data, bits, !rect_op);
        let align = rect_op && setup.host_cntl & 1 != 0 && setup.host_bits() < 8;
        let mut done = false;
        let mut previous_byte = None;
        for (element, byte) in elements {
            // HOST_BYTE_ALIGN: a row of 1- or 4-bit host data ends its
            // byte.
            if let Waiting::Rect(rect) = &mut waiting
                && std::mem::take(&mut rect.row_ended)
                && align
                && previous_byte == Some(byte)
            {
                rect.row_ended = true;
                continue;
            }
            previous_byte = Some(byte);
            done = match &mut waiting {
                Waiting::Rect(rect) => rect.pixel(&mut mem, &palettes, None, element),
                Waiting::Line(line) => line.pixel(&mut mem, &palettes, element),
            };
            if done {
                break;
            }
        }
        if done {
            match &waiting {
                Waiting::Rect(rect) => rect.tile(card),
                Waiting::Line(line) => line.finish(card),
            }
        } else {
            self.waiting = Some(waiting);
        }
        true
    }

    /// CONTEXT_LOAD_CNTL: load the registers a context in the top of the
    /// memory holds (the ones its first dword's mask bits pick), and with
    /// command 2 or 3 start a fill or a line; its dword 1Ch is the next
    /// context to load. The 256-byte contexts count down from the top.
    fn load_context(&mut self, card: &mut Mach64, target: &mut Target) -> bool {
        const LOADED: [usize; 28] = [
            0,
            0,
            DST_OFF_PITCH,
            DST_Y_X,
            DST_HEIGHT_WIDTH,
            DST_BRES_ERR,
            DST_BRES_INC,
            DST_BRES_DEC,
            SRC_OFF_PITCH,
            SRC_Y_X,
            SRC_HEIGHT1_WIDTH1,
            SRC_Y_X_START,
            SRC_HEIGHT2_WIDTH2,
            PAT_REG0,
            PAT_REG1,
            SC_LEFT_RIGHT,
            SC_TOP_BOTTOM,
            DP_BKGD_CLR,
            DP_FRGD_CLR,
            DP_WRITE_MSK,
            DP_CHAIN_MSK,
            DP_PIX_WIDTH,
            DP_MIX,
            DP_SRC,
            CLR_CMP_CLR,
            CLR_CMP_MSK,
            CLR_CMP_CNTL,
            GUI_TRAJ_CNTL,
        ];
        let mut cntl = card.reg(CONTEXT_LOAD_CNTL);
        if cntl & 1 << 31 != 0 {
            return false;
        }
        let contexts = (target.vram.len() >> 8) as u32;
        let dword =
            |vram: &[u8], at: usize| u32::from_le_bytes(vram[at..at + 4].try_into().unwrap());
        let mut drawn = false;
        for _ in 0..1024 {
            let command = cntl >> 16 & 3;
            if command == 0 {
                break;
            }
            let at = ((contexts - 1 - (cntl & 0x7FFF) % contexts) * 256) as usize;
            let mask = dword(target.vram, at);
            card.set_reg(CONTEXT_MASK, mask);
            for (i, &reg) in LOADED.iter().enumerate().skip(2) {
                if mask & 1 << i == 0 {
                    continue;
                }
                let value = dword(target.vram, at + 4 * i);
                if reg == DST_HEIGHT_WIDTH {
                    // Kept without starting the fill early.
                    card.set_reg(DST_HEIGHT, value & 0x7FFF);
                    card.set_reg(DST_WIDTH, value >> 16);
                    card.set_reg(
                        DST_BRES_LNTH,
                        card.reg(DST_BRES_LNTH) & !0x1FFF | value >> 16 & 0x1FFF,
                    );
                } else {
                    card.load_reg(reg, value, target);
                }
            }
            drawn |= match command {
                2 => self.start(Op::Fill, card, target),
                3 => self.start(Op::Line, card, target),
                _ => false,
            };
            cntl = dword(target.vram, at + 0x70);
        }
        card.set_reg(CONTEXT_LOAD_CNTL, cntl & 0x8003_FFFF);
        drawn
    }

    pub fn describe(&self) -> serde_json::Value {
        serde_json::json!({ "busy": self.busy(), "operations": self.ops })
    }
}

/// The elements one HOST_DATA write of `bits` carries, in the order the
/// engine takes them, each with the byte of the write it came from.
/// Whole bytes and more go lowest first (highest first right to left,
/// which lines never are); within a byte, sub-byte elements go from the
/// high bits unless DP_BYTE_PIX_ORDER (the other way right to left).
/// HOST_BIG_ENDIAN_EN swaps 16 and 32-bit data's bytes first.
fn host_elements(s: &Setup, data: u32, bits: u32, line: bool) -> Vec<(u32, u32)> {
    let mut width = s.host_bits();
    let left_to_right = line || s.x_dir > 0;
    let mut data = data;
    if s.host_cntl & 2 != 0 && width >= 16 {
        data = if width == 16 {
            (data & 0x00FF_00FF) << 8 | (data & 0xFF00_FF00) >> 8
        } else {
            data.swap_bytes()
        };
    }
    let bytes = bits / 8;
    let mut out = Vec::with_capacity(32);
    if width >= 8 {
        let mut count = bytes / (width / 8);
        if count == 0 {
            count = 1;
            width = bits;
        }
        for i in 0..count {
            let index = if left_to_right { i } else { count - 1 - i };
            let value = if width >= 32 {
                data
            } else {
                data >> (index * width) & ((1 << width) - 1)
            };
            out.push((value, index * width / 8));
        }
    } else {
        let per = 8 / width;
        let high_first = s.lsb_first != left_to_right;
        for b in 0..bytes {
            let byte = data >> (8 * if left_to_right { b } else { bytes - 1 - b }) & 0xFF;
            for k in 0..per {
                let position = if high_first { per - 1 - k } else { k };
                out.push((byte >> (position * width) & ((1 << width) - 1), b));
            }
        }
    }
    out
}

/// The 3D RAGE II's DP_SET_GUI_ENGINE: the engine set up for a common
/// operation in one write, as the GT-B driver does before its fills and
/// blits. Bits 5-3 the depth (and the source's with bit 6), 8-7 the
/// destination's offset in 2 MB, 13-10 its pitch from a table (doubled
/// with bit 14), bit 15 the source the same; 17-16 a source size, and
/// 23-20 one of a table of sources, mixes and trajectories (0 and 14
/// not known). As 86Box has it.
fn set_gui_engine(card: &mut Mach64, target: &mut Target) {
    const PITCHES: [u32; 16] = [
        320, 320, 352, 384, 640, 800, 896, 512, 1024, 1152, 1280, 400, 832, 1600, 448, 2048,
    ];
    const SOURCE_SIZES: [Option<u32>; 4] = [
        Some(0x0008_0008),
        Some(0x0020_0001),
        Some(0x0018_0008),
        None,
    ];
    const DRAWING: [Option<(u32, u32, u32)>; 16] = [
        None,
        Some((0x100, 0x07_0003, 0x23)),
        Some((0x200, 0x7_0007, 3)),
        Some((0x2_0100, 0x7_0007, 3)),
        Some((0x100, 0x7_0007, 3)),
        Some((0x1_0100, 0x7_0007, 0x0100_0003)),
        Some((0x100, 0x7_0007, 3)),
        Some((0x300, 0x7_0007, 0x3_0003)),
        Some((0x300, 0x7_0007, 0)),
        Some((0x300, 0x7_0007, 1)),
        Some((0x300, 0x7_0007, 2)),
        Some((0x300, 0x7_0007, 3)),
        Some((0x2_0100, 0x7_0003, 0x1004_001B)),
        Some((0x2_0100, 0x7_0003, 0x0004_001B)),
        None,
        Some((0x300, 0x7_0007, 0x0004_001B)),
    ];
    let v = card.reg(DP_SET_GUI_ENGINE);
    for reg in [
        DST_Y,
        DST_X,
        DST_HEIGHT,
        DST_WIDTH,
        SRC_Y,
        SRC_X,
        SRC_Y_START,
        SRC_X_START,
        SC_TOP,
        SC_LEFT,
    ] {
        card.set_reg(reg, 0);
    }
    // The scissors are signed: the widest positive ones.
    card.set_reg(SC_BOTTOM, 0x3FFF);
    card.set_reg(SC_RIGHT, 0x0FFF);
    card.set_reg(DP_WRITE_MSK, u32::MAX);
    card.set_reg(CLR_CMP_CLR, 0);
    card.set_reg(CLR_CMP_CNTL, 0);
    card.set_reg(SRC_CNTL, card.reg(SRC_CNTL) & !(3 << 13 | 1 << 5 | 1 << 12));
    card.set_reg(DST_CNTL, card.reg(DST_CNTL) & !(7 << 13));
    let depth = v >> 3 & 7;
    let source_depth = if v & 1 << 6 != 0 { depth << 8 } else { 0 };
    card.set_reg(
        DP_PIX_WIDTH,
        card.reg(DP_PIX_WIDTH) & 1 << 13 | depth | source_depth,
    );
    let pitch = PITCHES[(v >> 10 & 0xF) as usize] * if v & 1 << 14 != 0 { 2 } else { 1 };
    let off_pitch = (0x4_0000 * (v >> 7 & 3)) & 0xF_FFFF | (pitch / 8) << 22;
    card.set_reg(DST_OFF_PITCH, off_pitch);
    card.set_reg(SRC_OFF_PITCH, if v & 1 << 15 != 0 { off_pitch } else { 0 });
    if let Some(size) = SOURCE_SIZES[(v >> 16 & 3) as usize] {
        card.set_reg(SRC_HEIGHT1, size & 0xFFFF);
        card.set_reg(SRC_WIDTH1, size >> 16);
        card.set_reg(SRC_HEIGHT2, size & 0xFFFF);
        card.set_reg(SRC_WIDTH2, size >> 16);
    }
    if let Some((src, mix, trajectory)) = DRAWING[(v >> 20 & 0xF) as usize] {
        card.set_reg(DP_SRC, src);
        card.set_reg(DP_MIX, mix);
        card.load_reg(GUI_TRAJ_CNTL, trajectory, target);
    }
}

impl State for Engine {
    fn save(&self, w: &mut Writer) {
        self.palette.save(w);
    }

    fn load(&mut self, r: &mut Reader) -> Result<()> {
        self.palette.load(r)?;
        self.palette.resize(256, 0);
        self.waiting = None;
        Ok(())
    }
}

/// For the tests: a card whose engine draws into `vram`.
#[cfg(test)]
pub(crate) fn write(card: &mut Mach64, vram: &mut [u8], reg: usize, value: u32) {
    let mut target = Target {
        vram,
        dac: &[],
        dac_8bit: false,
    };
    card.load_reg(reg, value, &mut target);
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 64;

    /// A card set up for an 8-bit 64-pixel-wide screen at 0.
    fn card() -> (Mach64, Vec<u8>) {
        let mut card = Mach64::new(false);
        let mut vram = vec![0u8; super::super::MEMORY as usize];
        for (reg, value) in [
            (DST_OFF_PITCH, (W as u32 / 8) << 22),
            (SRC_OFF_PITCH, (W as u32 / 8) << 22),
            (DP_PIX_WIDTH, 0x2020202),
            (SC_LEFT_RIGHT, 0x0FFF << 16),
            (SC_TOP_BOTTOM, 0x3FFF << 16),
            (DP_MIX, 0x0007_0007),
            (DP_SRC, 0x100),
            (DST_CNTL, 3),
        ] {
            write(&mut card, &mut vram, reg, value);
        }
        (card, vram)
    }

    fn at(vram: &[u8], x: usize, y: usize) -> u8 {
        vram[y * W + x]
    }

    #[test]
    fn a_fill_in_the_foreground_colour() {
        let (mut card, mut vram) = card();
        write(&mut card, &mut vram, DP_FRGD_CLR, 0x42);
        write(&mut card, &mut vram, DST_Y_X, 3 << 16 | 2);
        write(&mut card, &mut vram, DST_HEIGHT_WIDTH, 4 << 16 | 2);
        assert_eq!(at(&vram, 3, 2), 0x42);
        assert_eq!(at(&vram, 6, 3), 0x42);
        assert_eq!(at(&vram, 7, 3), 0);
        assert_eq!(at(&vram, 3, 4), 0);
    }

    #[test]
    fn the_scissors_and_the_write_mask() {
        let (mut card, mut vram) = card();
        write(&mut card, &mut vram, DP_FRGD_CLR, 0xFF);
        write(&mut card, &mut vram, DP_WRITE_MSK, 0x0F);
        write(&mut card, &mut vram, SC_LEFT_RIGHT, 5 << 16 | 2);
        write(&mut card, &mut vram, DST_Y_X, 0);
        write(&mut card, &mut vram, DST_HEIGHT_WIDTH, 10 << 16 | 1);
        assert_eq!(
            (
                at(&vram, 1, 0),
                at(&vram, 2, 0),
                at(&vram, 5, 0),
                at(&vram, 6, 0)
            ),
            (0, 0x0F, 0x0F, 0)
        );
    }

    #[test]
    fn a_blit_right_to_left_copies_overlapping_rows_whole() {
        let (mut card, mut vram) = card();
        for (x, pixel) in vram[..8].iter_mut().enumerate() {
            *pixel = x as u8 + 1;
        }
        // Source 0..8, destination 2..10, from the right.
        write(&mut card, &mut vram, DP_SRC, 0x300);
        write(&mut card, &mut vram, DST_CNTL, 2);
        write(&mut card, &mut vram, SRC_Y_X, 7 << 16);
        write(&mut card, &mut vram, SRC_HEIGHT1_WIDTH1, 8 << 16 | 1);
        write(&mut card, &mut vram, DST_Y_X, 9 << 16);
        write(&mut card, &mut vram, DST_HEIGHT_WIDTH, 8 << 16 | 1);
        assert_eq!(&vram[..10], &[1, 2, 1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn mono_host_data_expands_to_colours() {
        let (mut card, mut vram) = card();
        write(&mut card, &mut vram, DP_SRC, 0x2_0100);
        write(&mut card, &mut vram, DP_FRGD_CLR, 7);
        write(&mut card, &mut vram, DP_BKGD_CLR, 1);
        write(&mut card, &mut vram, DST_Y_X, 0);
        write(&mut card, &mut vram, DST_HEIGHT_WIDTH, 8 << 16 | 1);
        assert!(card.engine.busy());
        // The first byte, from its high bit: 1010 0001.
        write(&mut card, &mut vram, HOST_DATA0, 0xA1);
        assert!(!card.engine.busy());
        assert_eq!(&vram[..8], &[7, 1, 7, 1, 1, 1, 1, 7]);
    }

    #[test]
    fn the_mono_pattern_by_rows() {
        let (mut card, mut vram) = card();
        write(&mut card, &mut vram, DP_SRC, 0x1_0100);
        write(&mut card, &mut vram, DP_FRGD_CLR, 9);
        write(&mut card, &mut vram, PAT_REG0, 0x0000_80FF);
        write(&mut card, &mut vram, PAT_CNTL, 1);
        write(&mut card, &mut vram, DST_Y_X, 0);
        write(&mut card, &mut vram, DST_HEIGHT_WIDTH, 8 << 16 | 2);
        assert!((0..8).all(|x| at(&vram, x, 0) == 9));
        assert_eq!((at(&vram, 0, 1), at(&vram, 1, 1)), (9, 0));
    }

    #[test]
    fn a_line_with_and_without_its_last_pixel() {
        let (mut card, mut vram) = card();
        write(&mut card, &mut vram, DP_FRGD_CLR, 5);
        // From 0,0 four pixels right and one down: x major.
        write(&mut card, &mut vram, DST_Y_X, 0);
        write(&mut card, &mut vram, DST_BRES_ERR, (-4i32) as u32);
        write(&mut card, &mut vram, DST_BRES_INC, 2);
        write(&mut card, &mut vram, DST_BRES_DEC, (-6i32) as u32);
        write(&mut card, &mut vram, DST_BRES_LNTH, 5);
        assert_eq!(at(&vram, 0, 0), 5);
        assert_eq!(at(&vram, 4, 1), 0, "no last pixel");
        assert_eq!(card.reg(DST_X), 4);
        write(&mut card, &mut vram, DST_CNTL, 3 | LAST_PEL);
        write(&mut card, &mut vram, DST_Y_X, 0);
        write(&mut card, &mut vram, DST_BRES_ERR, (-4i32) as u32);
        write(&mut card, &mut vram, DST_BRES_LNTH, 5);
        assert_eq!(at(&vram, 4, 1), 5);
    }

    #[test]
    fn colour_compare_against_the_destination() {
        let (mut card, mut vram) = card();
        vram[1] = 3;
        write(&mut card, &mut vram, DP_FRGD_CLR, 8);
        // Draw only where the destination is 3.
        write(&mut card, &mut vram, CLR_CMP_CLR, 3);
        write(&mut card, &mut vram, CLR_CMP_MSK, 0xFF);
        write(&mut card, &mut vram, CLR_CMP_CNTL, 4);
        write(&mut card, &mut vram, DST_Y_X, 0);
        write(&mut card, &mut vram, DST_HEIGHT_WIDTH, 3 << 16 | 1);
        assert_eq!(&vram[..3], &[0, 8, 0]);
    }

    #[test]
    fn a_24_bit_fill_rotates_the_colour_s_bytes() {
        let (mut card, mut vram) = card();
        write(&mut card, &mut vram, DP_FRGD_CLR, 0x11_2233);
        write(&mut card, &mut vram, DST_CNTL, 3 | ROTATE_24);
        write(&mut card, &mut vram, DST_Y_X, 3 << 16);
        write(&mut card, &mut vram, DST_HEIGHT_WIDTH, 6 << 16 | 1);
        assert_eq!(&vram[3..9], &[0x33, 0x22, 0x11, 0x33, 0x22, 0x11]);
    }

    #[test]
    fn a_flat_trapezoid_between_its_edges() {
        let (mut card, mut vram) = card();
        write(&mut card, &mut vram, DP_FRGD_CLR, 4);
        // Lead edge vertical at x=2, trail edge at x=6 moving right a
        // pixel a row; trailing edge on the right.
        write(
            &mut card,
            &mut vram,
            DST_CNTL,
            3 | TRAIL_X_DIR | TRAP_FILL_DIR,
        );
        write(&mut card, &mut vram, DST_Y_X, 2 << 16);
        write(&mut card, &mut vram, DST_BRES_ERR, (-1i32) as u32);
        write(&mut card, &mut vram, DST_BRES_INC, 0);
        write(&mut card, &mut vram, DST_BRES_DEC, (-1i32) as u32);
        write(&mut card, &mut vram, TRAIL_BRES_ERR, (-1i32) as u32);
        write(&mut card, &mut vram, TRAIL_BRES_INC, 1);
        write(&mut card, &mut vram, TRAIL_BRES_DEC, (-1i32) as u32);
        // Trailing X 6 (DST_HEIGHT), three rows.
        write(&mut card, &mut vram, DST_HEIGHT, 6);
        write(&mut card, &mut vram, DST_BRES_LNTH, 0x8000 | 3);
        assert_eq!(at(&vram, 1, 0), 0);
        assert_eq!(
            (at(&vram, 2, 0), at(&vram, 6, 0), at(&vram, 7, 0)),
            (4, 4, 0)
        );
        assert_eq!((at(&vram, 7, 1), at(&vram, 8, 1)), (4, 0));
        assert_eq!(card.reg(DST_Y), 3, "where the next trapezoid goes on");
        assert_eq!(card.reg(DST_HEIGHT) & 0x1FFF, 9);
    }

    /// One 3D row of `n` pixels from `x` on row 0, the leading edge on the
    /// left at `x` and a vertical trailing edge one past the last pixel.
    fn span(card: &mut Mach64, vram: &mut [u8], x: u32, n: u32) {
        write(card, vram, DST_CNTL, 3 | TRAIL_X_DIR | TRAP_FILL_DIR);
        write(card, vram, DST_Y_X, x << 16);
        for reg in [DST_BRES_ERR, DST_BRES_INC, TRAIL_BRES_ERR, TRAIL_BRES_INC] {
            write(
                card,
                vram,
                reg,
                if reg == DST_BRES_ERR || reg == TRAIL_BRES_ERR {
                    (-1i32) as u32
                } else {
                    0
                },
            );
        }
        write(card, vram, DST_BRES_DEC, (-1i32) as u32);
        write(card, vram, TRAIL_BRES_DEC, (-1i32) as u32);
        write(card, vram, DST_HEIGHT, x + n);
        write(card, vram, DST_BRES_LNTH, 0x8000 | 1);
    }

    fn word(vram: &[u8], at: usize) -> u16 {
        u16::from_le_bytes([vram[at], vram[at + 1]])
    }

    #[test]
    fn a_gouraud_span_steps_its_colour() {
        let (mut card, mut vram) = card();
        // 16 bits a pixel, Gouraud shading; red from 255 down 16 a pixel.
        write(&mut card, &mut vram, DP_PIX_WIDTH, 4);
        write(&mut card, &mut vram, DP_SRC, 0x500);
        write(&mut card, &mut vram, SCALE_3D_CNTL, 3 << 6);
        write(&mut card, &mut vram, RED_START, 255 << 16);
        write(
            &mut card,
            &mut vram,
            RED_X_INC,
            (-16i32 << 16) as u32 & 0x1FF_FFFF,
        );
        span(&mut card, &mut vram, 0, 4);
        let reds: Vec<u16> = (0..4).map(|x| word(&vram, 2 * x) >> 11).collect();
        assert_eq!(reds, [31, 29, 27, 25]);
        assert_eq!(word(&vram, 8), 0, "the trailing edge's pixel");
    }

    #[test]
    fn a_textured_span_samples_the_map() {
        let (mut card, mut vram) = card();
        // A 4x4 map of 32-bit texels at 64 KB: texel x is grey 40*(x+1).
        let base = 0x1_0000;
        for y in 0..4 {
            for x in 0..4 {
                let at = base + (y * 4 + x) * 4;
                vram[at..at + 4].copy_from_slice(&[40 * (x as u8 + 1); 4]);
            }
        }
        write(&mut card, &mut vram, DP_PIX_WIDTH, 6 << 28 | 6);
        write(&mut card, &mut vram, DP_SRC, 0x500);
        // Textured, no mipmaps; the map 4 texels square.
        write(&mut card, &mut vram, SCALE_3D_CNTL, 2 << 6 | 1 << 24);
        write(&mut card, &mut vram, TEX_SIZE_PITCH, 0x222);
        write(&mut card, &mut vram, TEX_0_OFF + 2, base as u32);
        // A texel is 256.0: start in the middle of the first, step one.
        write(&mut card, &mut vram, S_START, 128 << 16);
        write(&mut card, &mut vram, S_XINC_START, 256 << 16);
        write(&mut card, &mut vram, T_START, 128 << 16);
        span(&mut card, &mut vram, 0, 4);
        let reds: Vec<u8> = (0..4).map(|x| vram[4 * x + 2]).collect();
        assert_eq!(reds, [40, 80, 120, 160]);
    }

    #[test]
    fn the_z_buffer_keeps_the_nearer_pixel() {
        let (mut card, mut vram) = card();
        write(&mut card, &mut vram, DP_PIX_WIDTH, 4);
        write(&mut card, &mut vram, DP_SRC, 0x500);
        write(&mut card, &mut vram, SCALE_3D_CNTL, 3 << 6);
        // Z at 1 MB, 64 pixels a row; less passes, and is written.
        write(&mut card, &mut vram, Z_OFF_PITCH, 8 << 22 | 0x2_0000);
        write(&mut card, &mut vram, Z_CNTL, 0x111);
        vram[0x10_0000..0x10_0100].fill(0xFF);
        write(&mut card, &mut vram, Z_START, 100 << 12);
        write(&mut card, &mut vram, RED_START, 255 << 16);
        span(&mut card, &mut vram, 0, 2);
        assert_eq!(word(&vram, 0x10_0000), 100);
        // Farther and blue: hidden.
        write(&mut card, &mut vram, Z_START, 200 << 12);
        write(&mut card, &mut vram, RED_START, 0);
        write(&mut card, &mut vram, BLUE_START, 255 << 16);
        span(&mut card, &mut vram, 0, 2);
        assert_eq!(word(&vram, 0) >> 11, 31);
        assert_eq!(word(&vram, 0) & 31, 0);
    }

    #[test]
    fn the_scaler_stretches_an_image() {
        let (mut card, mut vram) = card();
        // A 2x1 image of 8-bit grey (Y8) texels at 64 KB.
        vram[0x1_0000] = 10;
        vram[0x1_0001] = 200;
        write(&mut card, &mut vram, DP_PIX_WIDTH, 8 << 28 | 8);
        write(&mut card, &mut vram, SCALE_3D_CNTL, 1 << 6 | 1 << 8);
        write(&mut card, &mut vram, TEX_0_OFF, 0x1_0000);
        write(&mut card, &mut vram, SCALE_WIDTH, 2);
        write(&mut card, &mut vram, SCALE_HEIGHT, 1);
        write(&mut card, &mut vram, SCALE_Y_PITCH, 2);
        // Half a texel a pixel: twice as wide.
        write(&mut card, &mut vram, SCALE_X_INC, 0x8000);
        write(&mut card, &mut vram, DST_Y_X, 0);
        write(&mut card, &mut vram, DST_HEIGHT_WIDTH, 4 << 16 | 1);
        assert_eq!(&vram[..4], &[10, 10, 200, 200]);
    }

    #[test]
    fn host_data_in_its_order() {
        let mut setup = Setup {
            mono: MONO_HOST,
            x_dir: 1,
            ..Default::default()
        };
        let bits: Vec<u32> = host_elements(&setup, 0x0180, 16, false)
            .into_iter()
            .map(|(v, _)| v)
            .collect();
        assert_eq!(bits[..8], [1, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(bits[8..], [0, 0, 0, 0, 0, 0, 0, 1]);
        setup.mono = 0;
        setup.host_size = Size::Word;
        let words: Vec<u32> = host_elements(&setup, 0x1234_5678, 32, false)
            .into_iter()
            .map(|(v, _)| v)
            .collect();
        assert_eq!(words, [0x5678, 0x1234]);
    }
}
