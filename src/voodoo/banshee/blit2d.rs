//! The Banshee's and Voodoo3's 2D engine: screen to screen and host to
//! screen blits (stretched or not, with colour conversion), rectangle
//! fills, lines, polylines and polygon fills, through four ternary
//! raster operations chosen by the source and destination colour keys.
//!
//! A port of 86Box's vid_voodoo_banshee_blitter.c (Sarah Walker, PCem;
//! GPLv2), with the register layout of 3dfx's Voodoo3 specification
//! (revision 1.0, chapter 7). The engine runs a command when its last
//! word arrives, so it is never busy when the host looks.

use crate::voodoo::mem::Vram;

const CMD_NOP: u32 = 0;
const CMD_SCREEN_TO_SCREEN: u32 = 1;
const CMD_SCREEN_TO_SCREEN_STRETCH: u32 = 2;
const CMD_HOST_TO_SCREEN: u32 = 3;
const CMD_HOST_TO_SCREEN_STRETCH: u32 = 4;
const CMD_RECTFILL: u32 = 5;
const CMD_LINE: u32 = 6;
const CMD_POLYLINE: u32 = 7;
const CMD_POLYFILL: u32 = 8;

const COMMAND_INITIATE: u32 = 1 << 8;
const COMMAND_INC_X_START: u32 = 1 << 10;
const COMMAND_INC_Y_START: u32 = 1 << 11;
const COMMAND_STIPPLE_LINE: u32 = 1 << 12;
const COMMAND_PATTERN_MONO: u32 = 1 << 13;
const COMMAND_DX: u32 = 1 << 14;
const COMMAND_DY: u32 = 1 << 15;
const COMMAND_TRANS_MONO: u32 = 1 << 16;
const COMMAND_CLIP_SEL: u32 = 1 << 23;

const CMDEXTRA_SRC_COLORKEY: u32 = 1 << 0;
const CMDEXTRA_DST_COLORKEY: u32 = 1 << 1;
const CMDEXTRA_FORCE_PAT_ROW0: u32 = 1 << 3;

const STRIDE_MASK: u32 = 0x1FFF;
const COL_MASK: u32 = 0xF << 16;
const COL_1BPP: u32 = 0;
const COL_8BPP: u32 = 1 << 16;
const COL_16BPP: u32 = 3 << 16;
const COL_24BPP: u32 = 4 << 16;
const COL_32BPP: u32 = 5 << 16;
const COL_YUYV: u32 = 8 << 16;
const COL_UYVY: u32 = 9 << 16;
const SRC_BYTE_SWIZZLE: u32 = 1 << 20;
const SRC_WORD_SWIZZLE: u32 = 1 << 21;
const PACKING_MASK: u32 = 3 << 22;
const PACKING_STRIDE: u32 = 0;
const PACKING_BYTE: u32 = 1 << 22;
const PACKING_WORD: u32 = 2 << 22;
const PACKING_DWORD: u32 = 3 << 22;

/// How a colour key compares a pixel.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Key {
    K8,
    K16,
    K32,
}

/// A clip rectangle: x in [x_min, x_max), y in [y_min, y_max).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Clip {
    pub x_min: i32,
    pub x_max: i32,
    pub y_min: i32,
    pub y_max: i32,
}

crate::state_fields!(Clip {
    x_min,
    x_max,
    y_min,
    y_max
});

/// Where a line of source pixels comes from: the card's memory from a
/// row's address, or the host data gathered so far from an offset.
#[derive(Clone, Copy)]
enum Source {
    Vram(u32),
    Host(usize),
}

/// The 2D engine's registers and the state of the command it runs.
#[derive(Clone, Debug)]
pub struct Engine {
    pub clip: [Clip; 2],
    pub clip0_min: u32,
    pub clip0_max: u32,
    pub clip1_min: u32,
    pub clip1_max: u32,
    pub dst_base: u32,
    pub dst_tiled: bool,
    pub dst_format: u32,
    pub dst_stride: u32,
    pub src_base: u32,
    pub src_tiled: bool,
    pub src_format: u32,
    pub src_stride: u32,
    pub src_bpp: u32,
    pub src_colorkey_min: u32,
    pub src_colorkey_max: u32,
    pub dst_colorkey_min: u32,
    pub dst_colorkey_max: u32,
    pub bres_error0: u32,
    pub bres_error1: u32,
    bres_error_0: i32,
    pub rop: u32,
    rops: [u8; 4],
    pub command_extra: u32,
    pub line_stipple: u32,
    pub line_style: u32,
    line_rep_cnt: u32,
    line_bit_mask_size: u32,
    line_pix_pos: u32,
    line_bit_pos: u32,
    /// colorPattern 0-63, as written.
    pub pattern: [u32; 64],
    pub src_size: u32,
    src_size_x: i32,
    src_size_y: i32,
    pub src_xy: u32,
    src_x: i32,
    src_y: i32,
    pub color_back: u32,
    pub color_fore: u32,
    pub dst_size: u32,
    dst_size_x: i32,
    dst_size_y: i32,
    pub dst_xy: u32,
    dst_x: i32,
    dst_y: i32,
    pub command: u32,
    launch_pending: bool,
    patoff_x: i32,
    patoff_y: i32,
    cur_y: i32,
    src_stride_src: u32,
    src_stride_dest: u32,
    host_data_size_src: u32,
    host_data_size_dest: u32,
    host_data: Vec<u8>,
    host_data_count: usize,
    lx: [i32; 2],
    ly: [i32; 2],
    rx: [i32; 2],
    ry: [i32; 2],
    dx: [i32; 2],
    dy: [i32; 2],
    x_inc: [i32; 2],
    error: [i32; 2],
    lx_cur: i32,
    rx_cur: i32,
    /// Something was drawn since the display last looked.
    pub drawn: bool,
}

/// The longest row of host data: a stride of 8191 bytes and the start's
/// offset in its dword, with room for the dword that overflows it.
const HOST_DATA: usize = 0x2000 + 16;

impl Default for Engine {
    fn default() -> Self {
        Self {
            clip: [Clip::default(); 2],
            clip0_min: 0,
            clip0_max: 0,
            clip1_min: 0,
            clip1_max: 0,
            dst_base: 0,
            dst_tiled: false,
            dst_format: 0,
            dst_stride: 0,
            src_base: 0,
            src_tiled: false,
            src_format: 0,
            src_stride: 0,
            src_bpp: 1,
            src_colorkey_min: 0,
            src_colorkey_max: 0,
            dst_colorkey_min: 0,
            dst_colorkey_max: 0,
            bres_error0: 0,
            bres_error1: 0,
            bres_error_0: 0,
            rop: 0,
            rops: [0; 4],
            command_extra: 0,
            line_stipple: 0,
            line_style: 0,
            line_rep_cnt: 0,
            line_bit_mask_size: 0,
            line_pix_pos: 0,
            line_bit_pos: 0,
            pattern: [0; 64],
            src_size: 0,
            src_size_x: 0,
            src_size_y: 0,
            src_xy: 0,
            src_x: 0,
            src_y: 0,
            color_back: 0,
            color_fore: 0,
            dst_size: 0,
            dst_size_x: 0,
            dst_size_y: 0,
            dst_xy: 0,
            dst_x: 0,
            dst_y: 0,
            command: 0,
            launch_pending: false,
            patoff_x: 0,
            patoff_y: 0,
            cur_y: 0,
            src_stride_src: 0,
            src_stride_dest: 0,
            host_data_size_src: 0,
            host_data_size_dest: 0,
            host_data: vec![0; HOST_DATA],
            host_data_count: 0,
            lx: [0; 2],
            ly: [0; 2],
            rx: [0; 2],
            ry: [0; 2],
            dx: [0; 2],
            dy: [0; 2],
            x_inc: [0; 2],
            error: [0; 2],
            lx_cur: 0,
            rx_cur: 0,
            drawn: false,
        }
    }
}

// The registers; the state of a command under way starts over.
crate::state_fields!(Engine {
    clip, clip0_min, clip0_max, clip1_min, clip1_max, dst_base, dst_tiled, dst_format, dst_stride,
    src_base, src_tiled, src_format, src_stride, src_bpp, src_colorkey_min, src_colorkey_max,
    dst_colorkey_min, dst_colorkey_max, bres_error0, bres_error1, rop, rops, command_extra,
    line_stipple, line_style, pattern, src_size, src_xy, color_back, color_fore, dst_size, dst_xy,
    command,
} skip {
    bres_error_0, line_rep_cnt, line_bit_mask_size, line_pix_pos, line_bit_pos, src_size_x,
    src_size_y, src_x, src_y, dst_size_x, dst_size_y, dst_x, dst_y, launch_pending, patoff_x,
    patoff_y, cur_y, src_stride_src, src_stride_dest, host_data_size_src, host_data_size_dest,
    host_data, host_data_count, lx, ly, rx, ry, dx, dy, x_inc, error, lx_cur, rx_cur, drawn,
});

/// The signed 13-bit x and y of an XY register.
fn xy(value: u32) -> (i32, i32) {
    (((value << 19) as i32) >> 19, ((value << 3) as i32) >> 19)
}

fn step(negative: bool) -> i32 {
    if negative { -1 } else { 1 }
}

/// A YUYV pair as two RGB pixels (red in the low byte, as 86Box has it).
fn yuyv_rgb(src: u32) -> [u32; 2] {
    let b = src.to_le_bytes();
    let (y1, cr, y2, cb) = (
        b[0] as i32,
        b[1] as i32 - 0x80,
        b[2] as i32,
        b[3] as i32 - 0x80,
    );
    let dr = (359 * cr) >> 8;
    let dg = (88 * cb + 183 * cr) >> 8;
    let db = (453 * cb) >> 8;
    let px = |y: i32| -> (u32, u32, u32) {
        (
            (y + dr).clamp(0, 255) as u32,
            (y - dg).clamp(0, 255) as u32,
            (y + db).clamp(0, 255) as u32,
        )
    };
    let ((r1, g1, b1), (r2, g2, b2)) = (px(y1), px(y2));
    [r1 | g1 << 8 | b1 << 16, r2 | g2 << 8 | b2 << 16]
}

impl Engine {
    fn mono_pattern(&self, row: i32) -> u8 {
        (self.pattern[((row & 7) >> 2) as usize] >> (8 * (row & 3))) as u8
    }

    fn pattern_byte(&self, i: usize) -> u8 {
        (self.pattern[(i >> 2) & 63] >> (8 * (i & 3))) as u8
    }

    /// The colour pattern's pixel `i` (0-63) at the destination's depth.
    fn pattern_pixel(&self, i: usize) -> u32 {
        match self.dst_format & COL_MASK {
            COL_8BPP => self.pattern_byte(i) as u32,
            COL_16BPP => {
                self.pattern_byte(2 * i) as u32 | (self.pattern_byte(2 * i + 1) as u32) << 8
            }
            COL_24BPP => (0..3).fold(0, |v, b| {
                v | (self.pattern_byte(3 * i + b) as u32) << (8 * b)
            }),
            _ => self.pattern[i],
        }
    }

    /// The pattern's colour at (`pat_x`, `pat_y`), with `mask` the mono
    /// pattern's row.
    fn pattern_at(&self, pat_x: i32, pat_y: i32, mask: u8) -> u32 {
        if self.command & COMMAND_PATTERN_MONO != 0 {
            if mask & (1 << (7 - (pat_x & 7))) != 0 {
                self.color_fore
            } else {
                self.color_back
            }
        } else {
            self.pattern_pixel(((pat_x & 7) + (pat_y & 7) * 8) as usize)
        }
    }

    fn colorkey(&self, value: u32, src: bool, key: Key) -> bool {
        let (min, max, bit) = if src {
            (
                self.src_colorkey_min,
                self.src_colorkey_max,
                CMDEXTRA_SRC_COLORKEY,
            )
        } else {
            (
                self.dst_colorkey_min,
                self.dst_colorkey_max,
                CMDEXTRA_DST_COLORKEY,
            )
        };
        if self.command_extra & bit == 0 {
            return false;
        }
        let within = |shift: u32, mask: u32| {
            let (v, lo, hi) = (
                (value >> shift) & mask,
                (min >> shift) & mask,
                (max >> shift) & mask,
            );
            v >= lo && v <= hi
        };
        match key {
            Key::K8 => within(0, 0xFF),
            Key::K16 => within(11, 0x1F) && within(5, 0x3F) && within(0, 0x1F),
            Key::K32 => within(16, 0xFF) && within(8, 0xFF) && within(0, 0xFF),
        }
    }

    /// The ternary raster operation the colour keys pick, on `dest`,
    /// `src` and `pattern` (86Box's `MIX`).
    fn mix(&self, dest: u32, src: u32, pattern: u32, src_key: Key, dst_key: Key) -> u32 {
        let mut n = 0;
        if self.colorkey(src, true, src_key) {
            n |= 2;
        }
        if self.colorkey(dest, false, dst_key) {
            n |= 1;
        }
        rop3(self.rops[n], dest, src, pattern)
    }

    /// The byte address of byte `x` of row `y` of the destination or the
    /// source, linear or in tiles of 128 bytes by 32 rows.
    fn addr(&self, x: i32, y: i32, src: bool, src_stride: u32, mask: u32) -> u32 {
        let (stride, base, tiled) = if src {
            (src_stride, self.src_base, self.src_tiled)
        } else {
            (self.dst_stride, self.dst_base, self.dst_tiled)
        };
        let (x, y) = (x as u32, y as u32);
        if tiled {
            base.wrapping_add(x & 127)
                .wrapping_add((x >> 7).wrapping_mul(128 * 32))
                .wrapping_add((y & 31) * 128)
                .wrapping_add((y >> 5).wrapping_mul(stride))
                & mask
        } else {
            base.wrapping_add(x).wrapping_add(y.wrapping_mul(stride)) & mask
        }
    }

    fn dst_key(&self) -> Key {
        match self.dst_format & COL_MASK {
            COL_8BPP => Key::K8,
            COL_16BPP => Key::K16,
            _ => Key::K32,
        }
    }

    fn src_key(&self) -> Key {
        match self.src_format & COL_MASK {
            COL_8BPP => Key::K8,
            COL_16BPP => Key::K16,
            _ => Key::K32,
        }
    }

    /// Write `src` at destination pixel (`x`, `y`) through the raster
    /// operation, with the pattern at (`pat_x`, `pat_y`) (86Box's `PLOT`).
    #[allow(clippy::too_many_arguments)]
    fn plot(
        &mut self,
        ram: &Vram,
        mask: u32,
        x: i32,
        y: i32,
        pat_x: i32,
        pat_y: i32,
        pattern_mask: u8,
        src: u32,
        src_key: Key,
    ) {
        let pattern = self.pattern_at(pat_x, pat_y, pattern_mask);
        self.put(ram, mask, x, y, src, pattern, src_key);
    }

    /// The raster operation's result at destination pixel (`x`, `y`).
    #[allow(clippy::too_many_arguments)]
    fn put(&mut self, ram: &Vram, mask: u32, x: i32, y: i32, src: u32, pattern: u32, src_key: Key) {
        let bytes = match self.dst_format & COL_MASK {
            COL_8BPP => 1,
            COL_16BPP => 2,
            COL_24BPP => 3,
            COL_32BPP => 4,
            _ => return,
        };
        let addr = self.addr(x * bytes, y, false, 0, mask);
        let dest = read(ram, addr, bytes as u32, mask);
        let value = self.mix(dest, src, pattern, src_key, self.dst_key());
        write(ram, addr, bytes as u32, value, mask);
        self.drawn = true;
    }

    /// A register of the 2D space written (at byte `addr & 1FCh`);
    /// writes to the launch area and the command register run commands
    /// (`voodoo_2d_reg_writel`).
    pub fn write(&mut self, ram: &Vram, addr: u32, val: u32) {
        let mask = ram.bytes() as u32 - 1;
        match addr & 0x1FC {
            0x08 => {
                self.clip0_min = val;
                self.clip[0].x_min = (val & 0xFFF) as i32;
                self.clip[0].y_min = ((val >> 16) & 0xFFF) as i32;
            }
            0x0C => {
                self.clip0_max = val;
                self.clip[0].x_max = (val & 0xFFF) as i32;
                self.clip[0].y_max = ((val >> 16) & 0xFFF) as i32;
            }
            0x10 => {
                self.dst_base = val & 0xFF_FFFF;
                self.dst_tiled = val & 0x8000_0000 != 0;
                self.update_dst_stride();
            }
            0x14 => {
                self.dst_format = val;
                self.update_dst_stride();
            }
            0x18 => self.src_colorkey_min = val & 0xFF_FFFF,
            0x1C => self.src_colorkey_max = val & 0xFF_FFFF,
            0x20 => self.dst_colorkey_min = val & 0xFF_FFFF,
            0x24 => self.dst_colorkey_max = val & 0xFF_FFFF,
            0x28 => {
                self.bres_error0 = val;
                self.bres_error_0 = (val & 0xFFFF) as i32;
            }
            0x2C => self.bres_error1 = val,
            0x30 => {
                self.rop = val;
                self.rops[1] = val as u8;
                self.rops[2] = (val >> 8) as u8;
                self.rops[3] = (val >> 16) as u8;
            }
            0x34 => {
                self.src_base = val & 0xFF_FFFF;
                self.src_tiled = val & 0x8000_0000 != 0;
                self.update_src_stride();
            }
            0x38 => self.command_extra = val,
            0x3C => self.line_stipple = val,
            0x40 => {
                self.line_style = val;
                self.line_rep_cnt = val & 0xFF;
                self.line_bit_mask_size = (val >> 8) & 0x1F;
                self.line_pix_pos = (val >> 16) & 0xFF;
                self.line_bit_pos = (val >> 24) & 0x1F;
            }
            0x44 => self.pattern[0] = val,
            0x48 => self.pattern[1] = val,
            0x4C => {
                self.clip1_min = val;
                self.clip[1].x_min = (val & 0xFFF) as i32;
                self.clip[1].y_min = ((val >> 16) & 0xFFF) as i32;
            }
            0x50 => {
                self.clip1_max = val;
                self.clip[1].x_max = (val & 0xFFF) as i32;
                self.clip[1].y_max = ((val >> 16) & 0xFFF) as i32;
            }
            0x54 => {
                self.src_format = val;
                self.update_src_stride();
                self.src_bpp = match val & COL_MASK {
                    COL_1BPP => 1,
                    COL_8BPP => 8,
                    COL_24BPP => 24,
                    COL_32BPP | COL_YUYV | COL_UYVY => 32,
                    _ => 16,
                };
            }
            0x58 => {
                self.src_size = val;
                self.src_size_x = (val & 0x1FFF) as i32;
                self.src_size_y = ((val >> 16) & 0x1FFF) as i32;
                self.update_packing();
            }
            0x5C => {
                self.src_xy = val;
                (self.src_x, self.src_y) = xy(val);
                self.update_packing();
            }
            0x60 => self.color_back = val,
            0x64 => self.color_fore = val,
            0x68 => {
                self.dst_size = val;
                self.dst_size_x = (val & 0x1FFF) as i32;
                self.dst_size_y = ((val >> 16) & 0x1FFF) as i32;
                self.update_packing();
            }
            0x6C => {
                self.dst_xy = val;
                (self.dst_x, self.dst_y) = xy(val);
            }
            0x70 => {
                self.command = val;
                self.launch_pending = true;
                match val & 0xF {
                    CMD_POLYFILL => {
                        self.launch();
                        if val & COMMAND_INITIATE != 0 {
                            self.dst_xy = self.src_xy;
                            self.dst_x = self.src_x;
                            self.dst_y = self.src_y;
                        }
                        self.polyfill_start();
                    }
                    CMD_HOST_TO_SCREEN | CMD_HOST_TO_SCREEN_STRETCH => {}
                    _ if val & COMMAND_INITIATE != 0 => self.run(ram, mask, None),
                    _ => {}
                }
            }
            0x80..=0xFC => self.launch_area(ram, mask, val),
            0x100..=0x1FC => self.pattern[((addr >> 2) & 63) as usize] = val,
            _ => {}
        }
    }

    /// A register of the 2D space read (the ones 86Box lets programs read
    /// back; the others read all ones).
    pub fn read(&self, addr: u32) -> u32 {
        match addr & 0x1FC {
            0x08 => self.clip0_min,
            0x0C => self.clip0_max,
            0x10 => self.dst_base | (self.dst_tiled as u32) << 31,
            0x14 => self.dst_format,
            0x34 => self.src_base | (self.src_tiled as u32) << 31,
            0x38 => self.command_extra,
            0x5C => self.src_xy,
            0x60 => self.color_back,
            0x64 => self.color_fore,
            0x68 => self.dst_size,
            0x6C => self.dst_xy,
            0x70 => self.command,
            _ => 0xFFFF_FFFF,
        }
    }

    fn update_dst_stride(&mut self) {
        let stride = self.dst_format & STRIDE_MASK;
        self.dst_stride = if self.dst_tiled {
            stride * 128 * 32
        } else {
            stride
        };
    }

    fn update_src_stride(&mut self) {
        let stride = self.src_format & STRIDE_MASK;
        self.src_stride = if self.src_tiled {
            stride * 128 * 32
        } else {
            stride
        };
        self.update_packing();
    }

    /// The source's row strides and the host data a row takes, from the
    /// packing (86Box's `update_src_stride`).
    fn update_packing(&mut self) {
        let bpp = match self.src_format & COL_MASK {
            COL_1BPP => 1,
            COL_8BPP => 8,
            COL_16BPP => 16,
            COL_24BPP => 24,
            COL_32BPP | COL_YUYV | COL_UYVY => 32,
            _ => 16,
        };
        let (sx, dx) = (self.src_size_x as u32, self.dst_size_x as u32);
        let packed = |bits: u32, unit: u32| bits.div_ceil(unit) * (unit / 8);
        match self.src_format & PACKING_MASK {
            PACKING_STRIDE => {
                self.src_stride_src = self.src_stride;
                self.src_stride_dest = self.src_stride;
                self.host_data_size_src = (sx * bpp).div_ceil(8);
                self.host_data_size_dest = (dx * bpp).div_ceil(8);
            }
            packing => {
                let unit = match packing {
                    PACKING_BYTE => 8,
                    PACKING_WORD => 16,
                    PACKING_DWORD => 32,
                    _ => unreachable!(),
                };
                self.src_stride_src = packed(sx * bpp, unit);
                self.src_stride_dest = packed(dx * bpp, unit);
                self.host_data_size_src = self.src_stride_src;
                self.host_data_size_dest = self.src_stride_dest;
            }
        }
    }

    /// A command begins: the coordinates and pattern offsets from the
    /// registers (`banshee_do_2d_launch`).
    fn launch(&mut self) {
        self.launch_pending = false;
        self.rops[0] = (self.command >> 24) as u8;
        self.patoff_x = ((self.command >> 17) & 7) as i32;
        self.patoff_y = ((self.command >> 20) & 7) as i32;
        self.cur_y = 0;
        (self.dst_x, self.dst_y) = xy(self.dst_xy);
        (self.src_x, self.src_y) = xy(self.src_xy);
        self.host_data_count = 0;
    }

    /// Run the command, with `data` a word of host data for the host to
    /// screen blits (`banshee_do_2d_blit`).
    fn run(&mut self, ram: &Vram, mask: u32, data: Option<u32>) {
        if self.launch_pending {
            self.launch();
        }
        match self.command & 0xF {
            CMD_NOP => {}
            CMD_SCREEN_TO_SCREEN => self.screen_to_screen(ram, mask),
            CMD_SCREEN_TO_SCREEN_STRETCH => self.screen_to_screen_stretch(ram, mask),
            CMD_HOST_TO_SCREEN => {
                if let Some(data) = data {
                    self.host_to_screen(ram, mask, data, false);
                }
            }
            CMD_HOST_TO_SCREEN_STRETCH => {
                if let Some(data) = data {
                    self.host_to_screen(ram, mask, data, true);
                }
            }
            CMD_RECTFILL => self.rectfill(ram, mask),
            CMD_LINE => self.line(ram, mask, true),
            CMD_POLYLINE => self.line(ram, mask, false),
            _ => {}
        }
    }

    /// A write to the launch area: the next coordinates or host data of
    /// the command.
    fn launch_area(&mut self, ram: &Vram, mask: u32, val: u32) {
        if self.launch_pending {
            self.launch();
        }
        match self.command & 0xF {
            CMD_SCREEN_TO_SCREEN => {
                self.src_xy = val;
                (self.src_x, self.src_y) = xy(val);
                self.screen_to_screen(ram, mask);
            }
            CMD_HOST_TO_SCREEN => self.host_to_screen(ram, mask, val, false),
            CMD_HOST_TO_SCREEN_STRETCH => self.host_to_screen(ram, mask, val, true),
            CMD_RECTFILL | CMD_LINE | CMD_POLYLINE => {
                self.dst_xy = val;
                (self.dst_x, self.dst_y) = xy(val);
                match self.command & 0xF {
                    CMD_RECTFILL => self.rectfill(ram, mask),
                    CMD_LINE => self.line(ram, mask, true),
                    _ => self.line(ram, mask, false),
                }
            }
            CMD_POLYFILL => self.polyfill_continue(ram, mask, val),
            _ => {}
        }
    }

    /// The destination's start after a command, if the command asks for
    /// it (`end_command`).
    fn end_command(&mut self) {
        if self.command & COMMAND_INC_X_START != 0 {
            self.dst_xy = (self.dst_xy & !0xFFFF) | (self.dst_x as u32 & 0xFFFF);
        }
        if self.command & COMMAND_INC_Y_START != 0 {
            self.dst_xy = (self.dst_xy & 0xFFFF) | (self.dst_y as u32) << 16;
        }
    }

    fn clip_of_command(&self) -> Clip {
        self.clip[(self.command & COMMAND_CLIP_SEL != 0) as usize]
    }

    fn use_pattern_trans(&self) -> bool {
        self.command & (COMMAND_PATTERN_MONO | COMMAND_TRANS_MONO)
            == COMMAND_PATTERN_MONO | COMMAND_TRANS_MONO
    }

    fn rectfill(&mut self, ram: &Vram, mask: u32) {
        let clip = self.clip_of_command();
        let mut dst_y = self.dst_y;
        let force_row0 = self.command_extra & CMDEXTRA_FORCE_PAT_ROW0 != 0;
        let mut pat_y = if force_row0 {
            0
        } else {
            self.patoff_y + self.dst_y
        };
        let trans = self.use_pattern_trans();
        let (sx, sy) = (
            step(self.command & COMMAND_DX != 0),
            step(self.command & COMMAND_DY != 0),
        );
        for _ in 0..self.dst_size_y {
            if dst_y >= clip.y_min && dst_y < clip.y_max {
                let mut dst_x = self.dst_x;
                let mut pat_x = self.patoff_x + self.dst_x;
                let pattern_mask = self.mono_pattern(pat_y);
                for _ in 0..self.dst_size_x {
                    let shown = !trans || pattern_mask & (1 << (7 - (pat_x & 7))) != 0;
                    if dst_x >= clip.x_min && dst_x < clip.x_max && shown {
                        self.plot(
                            ram,
                            mask,
                            dst_x,
                            dst_y,
                            pat_x,
                            pat_y,
                            pattern_mask,
                            self.color_fore,
                            Key::K32,
                        );
                    }
                    dst_x += sx;
                    pat_x += sx;
                }
            }
            dst_y += sy;
            if !force_row0 {
                pat_y += sy;
            }
        }
        self.end_command();
    }

    fn src_byte(&self, ram: &Vram, src: Source, offset: usize, mask: u32) -> u8 {
        match src {
            Source::Vram(addr) => ram.byte((addr.wrapping_add(offset as u32) & mask) as usize),
            Source::Host(base) => self.host_data.get(base + offset).copied().unwrap_or(0),
        }
    }

    fn src_word(&self, ram: &Vram, src: Source, offset: usize, bytes: usize, mask: u32) -> u32 {
        (0..bytes).fold(0, |v, i| {
            v | (self.src_byte(ram, src, offset + i, mask) as u32) << (8 * i)
        })
    }

    /// The source pixel at byte `offset` of the row converted to the
    /// destination's format where they differ, YUV left as it is; and
    /// whether a transparent mono pixel leaves the destination alone.
    fn src_pixel(
        &self,
        ram: &Vram,
        src: Source,
        offset: usize,
        src_x: i32,
        mask: u32,
    ) -> (u32, bool) {
        let mut transparent = false;
        let data = match self.src_format & COL_MASK {
            COL_1BPP => {
                let byte = self.src_byte(ram, src, offset, mask);
                let set = byte & (0x80 >> (src_x & 7)) != 0;
                if self.command & COMMAND_TRANS_MONO != 0 {
                    transparent = !set;
                }
                if set {
                    self.color_fore
                } else {
                    self.color_back
                }
            }
            COL_8BPP => self.src_byte(ram, src, offset, mask) as u32,
            COL_16BPP => {
                let v = self.src_word(ram, src, offset, 2, mask);
                let (r, g, b) = (v >> 11, (v >> 5) & 0x3F, v & 0x1F);
                ((r << 3) | (r >> 2)) << 16 | ((g << 2) | (g >> 4)) << 8 | ((b << 3) | (b >> 2))
            }
            COL_UYVY => {
                let v = self.src_word(ram, src, offset, 4, mask);
                ((v & 0xFF00) >> 8)
                    | ((v & 0xFF) << 8)
                    | ((v & 0xFF00_0000) >> 8)
                    | ((v & 0xFF_0000) << 8)
            }
            _ => self.src_word(ram, src, offset, 4, mask),
        };
        let format = self.src_format & COL_MASK;
        let yuv = matches!(format, COL_YUYV | COL_UYVY);
        if self.dst_format & COL_MASK == COL_16BPP && format != COL_1BPP && !yuv {
            let (r, g, b) = (data >> 16 & 0xFF, (data >> 8) & 0xFF, data & 0xFF);
            return ((b >> 3) | ((g >> 2) << 5) | ((r >> 3) << 11), transparent);
        }
        (data, transparent)
    }

    /// Plot a converted source pixel: a YUV pair as two pixels, the
    /// second at the next x. Returns how far x moved beyond the first.
    #[allow(clippy::too_many_arguments)]
    fn plot_converted(
        &mut self,
        ram: &Vram,
        mask: u32,
        x: i32,
        y: i32,
        pat: (i32, i32, u8),
        data: u32,
        transparent: bool,
        next: i32,
    ) -> i32 {
        let src_key = self.src_key();
        let (pat_x, pat_y, pattern_mask) = pat;
        let emit = |e: &mut Self, x: i32, value: u32| {
            if !transparent {
                e.plot(ram, mask, x, y, pat_x, pat_y, pattern_mask, value, src_key);
            }
        };
        if matches!(self.src_format & COL_MASK, COL_YUYV | COL_UYVY) {
            let [a, b] = yuyv_rgb(data);
            match self.dst_format & COL_MASK {
                COL_24BPP | COL_32BPP => {
                    emit(self, x, a);
                    emit(self, x + next, b);
                }
                COL_16BPP => {
                    let to565 = |c: u32| {
                        ((c & 0xFF) >> 3)
                            | (((c >> 8) & 0xFF) >> 2) << 5
                            | (((c >> 16) & 0xFF) >> 3) << 11
                    };
                    emit(self, x, to565(a));
                    emit(self, x + next, to565(b));
                }
                _ => return 0,
            }
            return next;
        }
        emit(self, x, data);
        0
    }

    /// One row of a screen to screen or host to screen blit, from `src`
    /// (`do_screen_to_screen_line`).
    fn copy_line(
        &mut self,
        ram: &Vram,
        mask: u32,
        src: Source,
        use_x_dir: bool,
        mut src_x: i32,
        src_tiled: bool,
    ) {
        let clip = self.clip_of_command();
        let dst_y = self.dst_y;
        let pat_y = if self.command_extra & CMDEXTRA_FORCE_PAT_ROW0 != 0 {
            0
        } else {
            self.patoff_y + self.dst_y
        };
        let trans = self.use_pattern_trans();
        let same = self.src_format & COL_MASK == self.dst_format & COL_MASK;
        let sx = if use_x_dir {
            step(self.command & COMMAND_DX != 0)
        } else {
            1
        };
        let bytes = match self.dst_format & COL_MASK {
            COL_8BPP => 1,
            COL_16BPP => 2,
            COL_24BPP => 3,
            _ => 4,
        };
        if dst_y >= clip.y_min && dst_y < clip.y_max {
            let mut dst_x = self.dst_x;
            let mut pat_x = self.patoff_x + self.dst_x;
            let pattern_mask = self.mono_pattern(pat_y);
            for _ in 0..self.dst_size_x {
                let shown = !trans || pattern_mask & (1 << (7 - (pat_x & 7))) != 0;
                let mut real = ((src_x as i64 * self.src_bpp as i64) >> 3) as i32;
                if src_tiled {
                    real = (real & 127) + (real >> 7) * 128 * 32;
                }
                if dst_x >= clip.x_min && dst_x < clip.x_max && shown {
                    if same {
                        let value = self.src_word(ram, src, real as usize, bytes, mask);
                        let src_key = self.src_key();
                        self.plot(
                            ram,
                            mask,
                            dst_x,
                            dst_y,
                            pat_x,
                            pat_y,
                            pattern_mask,
                            value,
                            src_key,
                        );
                    } else {
                        let (data, transparent) =
                            self.src_pixel(ram, src, real as usize, src_x, mask);
                        dst_x += self.plot_converted(
                            ram,
                            mask,
                            dst_x,
                            dst_y,
                            (pat_x, pat_y, pattern_mask),
                            data,
                            transparent,
                            sx,
                        );
                    }
                }
                src_x += sx;
                dst_x += sx;
                pat_x += sx;
            }
        }
        let sy = step(self.command & COMMAND_DY != 0);
        self.src_y += sy;
        self.dst_y += sy;
    }

    fn screen_to_screen(&mut self, ram: &Vram, mask: u32) {
        for row in 0..self.dst_size_y {
            self.cur_y = row;
            let addr = self.addr(0, self.src_y, true, self.src_stride_dest, mask);
            self.copy_line(
                ram,
                mask,
                Source::Vram(addr),
                true,
                self.src_x,
                self.src_tiled,
            );
        }
        self.cur_y = self.dst_size_y;
        self.end_command();
    }

    /// One row of a stretched blit: the source's pixels stepped by
    /// Bresenham errors to the destination's width, and its row to its
    /// height when `src_y` (`do_screen_to_screen_stretch_line`).
    fn stretch_line(
        &mut self,
        ram: &Vram,
        mask: u32,
        src: Source,
        mut src_x: i32,
        advance_src_y: bool,
    ) {
        let clip = self.clip_of_command();
        let dst_y = self.dst_y;
        let pat_y = if self.command_extra & CMDEXTRA_FORCE_PAT_ROW0 != 0 {
            0
        } else {
            self.patoff_y + self.dst_y
        };
        let trans = self.use_pattern_trans();
        let same = self.src_format & COL_MASK == self.dst_format & COL_MASK;
        if dst_y >= clip.y_min && dst_y < clip.y_max {
            let mut dst_x = self.dst_x;
            let pattern_mask = self.mono_pattern(pat_y);
            let mut error_x = self.dst_size_x / 2;
            let first = self.patoff_x + self.dst_x;
            for pat_x in (first..).take(self.dst_size_x.max(0) as usize) {
                let shown = !trans || pattern_mask & (1 << (7 - (pat_x & 7))) != 0;
                if dst_x >= clip.x_min && dst_x < clip.x_max && shown {
                    if same {
                        let bytes = match self.dst_format & COL_MASK {
                            COL_8BPP => 1,
                            COL_16BPP => 2,
                            COL_24BPP => 3,
                            COL_32BPP => 4,
                            _ => 0,
                        };
                        if bytes > 0 {
                            let value = self.src_word(
                                ram,
                                src,
                                (src_x * bytes) as usize,
                                bytes as usize,
                                mask,
                            );
                            let key = self.dst_key();
                            self.plot(
                                ram,
                                mask,
                                dst_x,
                                dst_y,
                                pat_x,
                                pat_y,
                                pattern_mask,
                                value,
                                key,
                            );
                        }
                    } else {
                        let real = ((src_x as i64 * self.src_bpp as i64) >> 3) as usize;
                        let (data, transparent) = self.src_pixel(ram, src, real, src_x, mask);
                        dst_x += self.plot_converted(
                            ram,
                            mask,
                            dst_x,
                            dst_y,
                            (pat_x, pat_y, pattern_mask),
                            data,
                            transparent,
                            1,
                        );
                    }
                }
                error_x -= self.src_size_x;
                while error_x < 0 && self.dst_size_x > 0 {
                    error_x += self.dst_size_x;
                    src_x += 1;
                }
                dst_x += 1;
            }
        }
        self.bres_error_0 -= self.src_size_y;
        while self.bres_error_0 < 0 && self.dst_size_y > 0 {
            self.bres_error_0 += self.dst_size_y;
            if advance_src_y {
                self.src_y += step(self.command & COMMAND_DY != 0);
            }
        }
        self.dst_y += step(self.command & COMMAND_DY != 0);
    }

    fn screen_to_screen_stretch(&mut self, ram: &Vram, mask: u32) {
        for row in 0..self.dst_size_y {
            self.cur_y = row;
            let addr = self.addr(0, self.src_y, true, self.src_stride_src, mask);
            self.stretch_line(ram, mask, Source::Vram(addr), self.src_x, true);
        }
        self.cur_y = self.dst_size_y;
        self.end_command();
    }

    /// A dword of host data for a host to screen blit: rows run as their
    /// data is complete (`banshee_do_host_to_screen_blt` and its stretched
    /// form).
    fn host_to_screen(&mut self, ram: &Vram, mask: u32, mut data: u32, stretch: bool) {
        if self.src_format & SRC_BYTE_SWIZZLE != 0 {
            data = data.swap_bytes();
        }
        if self.src_format & SRC_WORD_SWIZZLE != 0 {
            data = data.rotate_left(16);
        }
        let mono = self.src_format & COL_MASK == COL_1BPP;
        let count = self.host_data_count;
        if count + 4 > self.host_data.len() {
            // A row longer than any stride: start the row over.
            self.host_data_count = 0;
            return;
        }
        self.host_data[count..count + 4].copy_from_slice(&data.to_le_bytes());
        self.host_data_count += 4;
        if self.src_format & PACKING_MASK == PACKING_STRIDE {
            let last_byte = if mono && !stretch {
                ((self.src_x & 31) + self.dst_size_x + 7) >> 3
            } else if stretch {
                (self.src_x & 3) + self.host_data_size_src as i32
            } else {
                (self.src_x & 3) + self.host_data_size_dest as i32
            };
            if self.host_data_count as i32 >= last_byte {
                if self.cur_y < self.dst_size_y {
                    self.host_row(ram, mask, stretch, true);
                }
                let stride = (self.src_format & STRIDE_MASK) as i32;
                self.src_x += if mono { stride << 3 } else { stride };
                self.host_data_count = 0;
            }
        } else {
            let stride = if stretch {
                self.src_stride_src
            } else {
                self.src_stride_dest
            } as usize;
            if stride == 0 {
                self.host_data_count = 0;
                return;
            }
            while self.host_data_count >= stride {
                self.host_data_count -= stride;
                if self.cur_y < self.dst_size_y {
                    self.host_row(ram, mask, stretch, false);
                }
                if self.host_data_count > 0 {
                    let left = data >> ((4 - self.host_data_count) * 8);
                    self.host_data[0..4].copy_from_slice(&left.to_le_bytes());
                }
            }
        }
    }

    /// A row of host data is complete: draw it.
    fn host_row(&mut self, ram: &Vram, mask: u32, stretch: bool, strided: bool) {
        let mono = self.src_format & COL_MASK == COL_1BPP;
        let (src, src_x) = if !strided {
            (Source::Host(0), 0)
        } else if mono {
            (
                Source::Host(((self.src_x >> 3) & 3) as usize),
                self.src_x & 7,
            )
        } else {
            (Source::Host((self.src_x & 3) as usize), 0)
        };
        if stretch {
            self.stretch_line(ram, mask, src, src_x, false);
        } else {
            self.copy_line(ram, mask, src, false, src_x, false);
        }
        self.cur_y += 1;
        if self.cur_y == self.dst_size_y {
            self.end_command();
        }
    }

    fn step_line(&mut self) {
        if self.line_pix_pos == self.line_rep_cnt {
            self.line_pix_pos = 0;
            if self.line_bit_pos == self.line_bit_mask_size {
                self.line_bit_pos = 0;
            } else {
                self.line_bit_pos += 1;
            }
        } else {
            self.line_pix_pos += 1;
        }
    }

    fn line_pixel(&mut self, ram: &Vram, mask: u32, x: i32, y: i32, clip: Clip, stipple: u32) {
        let on = stipple & (1 << (self.line_bit_pos & 31)) != 0;
        let shown = self.command & COMMAND_TRANS_MONO == 0 || on;
        if y >= clip.y_min && y < clip.y_max && x >= clip.x_min && x < clip.x_max && shown {
            let pattern = if on { self.color_fore } else { self.color_back };
            self.put(ram, mask, x, y, self.color_fore, pattern, Key::K32);
        }
    }

    /// A line from the source's XY to the destination's, by Bresenham's
    /// algorithm, with the last pixel unless it is a polyline's
    /// (`banshee_do_line`).
    fn line(&mut self, ram: &Vram, mask: u32, draw_last: bool) {
        let clip = self.clip_of_command();
        let dx = (self.dst_x - self.src_x).abs();
        let dy = (self.dst_y - self.src_y).abs();
        let x_inc = if self.dst_x > self.src_x { 1 } else { -1 };
        let y_inc = if self.dst_y > self.src_y { 1 } else { -1 };
        let (mut x, mut y) = (self.src_x, self.src_y);
        let stipple = if self.command & COMMAND_STIPPLE_LINE != 0 {
            self.line_stipple
        } else {
            !0
        };
        if dx > dy {
            let mut error = dx / 2;
            while x != self.dst_x {
                self.line_pixel(ram, mask, x, y, clip, stipple);
                error -= dy;
                if error < 0 {
                    error += dx;
                    y += y_inc;
                }
                x += x_inc;
                self.step_line();
            }
        } else {
            let mut error = dy / 2;
            while y != self.dst_y {
                self.line_pixel(ram, mask, x, y, clip, stipple);
                error -= dx;
                if error < 0 {
                    error += dy;
                    x += x_inc;
                }
                y += y_inc;
                self.step_line();
            }
        }
        if draw_last {
            self.line_pixel(ram, mask, x, y, clip, stipple);
        }
        self.src_xy = (x as u32 & 0xFFFF) | (y as u32) << 16;
        self.src_x = x;
        self.src_y = y;
    }

    fn polyfill_start(&mut self) {
        self.lx = [self.src_x; 2];
        self.ly = [self.src_y; 2];
        self.rx = [self.dst_x; 2];
        self.ry = [self.dst_y; 2];
        self.lx_cur = self.src_x;
        self.rx_cur = self.dst_x;
    }

    /// The next vertex of a polygon fill, on the left or the right edge
    /// whichever is higher, and the spans down to it
    /// (`banshee_polyfill_continue`).
    fn polyfill_continue(&mut self, ram: &Vram, mask: u32, data: u32) {
        let clip = self.clip_of_command();
        let trans = self.use_pattern_trans();
        let mut y = self.ly[0].max(self.ry[0]);
        let (vx, vy) = xy(data);
        let side = if self.ry[1] >= self.ly[1] { 0 } else { 1 };
        if side == 0 {
            self.lx[1] = vx;
            self.ly[1] = vy;
            self.dx[0] = (self.lx[1] - self.lx[0]).abs();
            self.dy[0] = (self.ly[1] - self.ly[0]).abs();
            self.x_inc[0] = if self.lx[1] > self.lx[0] { 1 } else { -1 };
            self.error[0] = self.dy[0] / 2;
        } else {
            self.rx[1] = vx;
            self.ry[1] = vy;
            self.dx[1] = (self.rx[1] - self.rx[0]).abs();
            self.dy[1] = (self.ry[1] - self.ry[0]).abs();
            self.x_inc[1] = if self.rx[1] > self.rx[0] { 1 } else { -1 };
            self.error[1] = self.dy[1] / 2;
        }
        let y_end = self.ly[1].min(self.ry[1]);
        while y < y_end {
            if y >= clip.y_min && y < clip.y_max {
                let pat_y = if self.command_extra & CMDEXTRA_FORCE_PAT_ROW0 != 0 {
                    0
                } else {
                    self.patoff_y + y
                };
                let pattern_mask = self.mono_pattern(pat_y);
                for x in self.lx_cur..self.rx_cur {
                    let pat_x = self.patoff_x + x;
                    let shown = !trans || pattern_mask & (1 << (7 - (pat_x & 7))) != 0;
                    if x >= clip.x_min && x < clip.x_max && shown {
                        self.plot(
                            ram,
                            mask,
                            x,
                            y,
                            pat_x,
                            pat_y,
                            pattern_mask,
                            self.color_fore,
                            Key::K32,
                        );
                    }
                }
            }
            for e in 0..2 {
                self.error[e] -= self.dx[e];
                while self.error[e] < 0 && self.dy[e] > 0 {
                    self.error[e] += self.dy[e];
                    if e == 0 {
                        self.lx_cur += self.x_inc[0];
                    } else {
                        self.rx_cur += self.x_inc[1];
                    }
                }
            }
            y += 1;
        }
        if self.ry[1] == self.ly[1] {
            self.lx[0] = self.lx[1];
            self.ly[0] = self.ly[1];
            self.rx[0] = self.rx[1];
            self.ry[0] = self.ry[1];
        } else if self.ry[1] >= self.ly[1] {
            self.lx[0] = self.lx[1];
            self.ly[0] = self.ly[1];
        } else {
            self.rx[0] = self.rx[1];
            self.ry[0] = self.ry[1];
        }
    }
}

/// A ternary raster operation: each of the eight bits of `rop` says
/// whether a combination of pattern, source and destination bits makes
/// a 1.
pub fn rop3(rop: u8, dest: u32, src: u32, pattern: u32) -> u32 {
    let mut result = 0;
    for bit in 0..8 {
        if rop & (1 << bit) != 0 {
            let p = if bit & 4 != 0 { pattern } else { !pattern };
            let s = if bit & 2 != 0 { src } else { !src };
            let d = if bit & 1 != 0 { dest } else { !dest };
            result |= p & s & d;
        }
    }
    result
}

/// `bytes` (1-4) of the memory at byte address `addr`, little-endian.
pub fn read(ram: &Vram, addr: u32, bytes: u32, mask: u32) -> u32 {
    if bytes == 2 && addr & 1 == 0 {
        return ram.get(((addr & mask) >> 1) as usize) as u32;
    }
    if bytes == 4 && addr & 3 == 0 {
        let at = ((addr & mask) >> 1) as usize;
        return ram.get(at) as u32 | (ram.get(at + 1) as u32) << 16;
    }
    (0..bytes).fold(0, |v, i| {
        v | (ram.byte((addr.wrapping_add(i) & mask) as usize) as u32) << (8 * i)
    })
}

/// Write `bytes` (1-4) of `value` at byte address `addr`.
pub fn write(ram: &Vram, addr: u32, bytes: u32, value: u32, mask: u32) {
    if bytes == 2 && addr & 1 == 0 {
        ram.set(((addr & mask) >> 1) as usize, value as u16);
        return;
    }
    if bytes == 4 && addr & 3 == 0 {
        let at = ((addr & mask) >> 1) as usize;
        ram.set(at, value as u16);
        ram.set(at + 1, (value >> 16) as u16);
        return;
    }
    for i in 0..bytes {
        ram.set_byte(
            (addr.wrapping_add(i) & mask) as usize,
            (value >> (8 * i)) as u8,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> (Engine, Vram) {
        (Engine::default(), Vram::new(1 << 20))
    }

    /// A 16-bit destination of 1024 pixels a row, clip 0 the whole of it.
    fn setup_16(e: &mut Engine, ram: &Vram) {
        e.write(ram, 0x14, 3 << 16 | 2048);
        e.write(ram, 0x10, 0);
        e.write(ram, 0x08, 0);
        e.write(ram, 0x0C, 0x0FFF_0FFF);
    }

    #[test]
    fn raster_operations() {
        assert_eq!(rop3(0xCC, 0x12, 0x34, 0x56), 0x34, "source copy");
        assert_eq!(rop3(0xF0, 0x12, 0x34, 0x56), 0x56, "pattern copy");
        assert_eq!(rop3(0xAA, 0x12, 0x34, 0x56), 0x12, "destination");
        assert_eq!(rop3(0x66, 0x0F, 0xFF, 0), 0xF0, "source xor destination");
        assert_eq!(rop3(0x00, 0x12, 0x34, 0x56), 0);
    }

    #[test]
    fn rectangle_fill_in_16_bits() {
        let (mut e, ram) = engine();
        setup_16(&mut e, &ram);
        e.write(&ram, 0x64, 0xF800);
        e.write(&ram, 0x68, 3 << 16 | 4);
        e.write(&ram, 0x6C, 2 << 16 | 1);
        // Rectangle fill, pattern copy, initiate.
        e.write(&ram, 0x70, 0xF0 << 24 | COMMAND_INITIATE | CMD_RECTFILL);
        // Pattern copy with a mono pattern of all zeroes: colorBack.
        assert_eq!(ram.get(2 * 1024 + 1), 0);
        e.write(&ram, 0x70, 0xCC << 24 | COMMAND_INITIATE | CMD_RECTFILL);
        for y in 2..5 {
            for x in 1..5 {
                assert_eq!(ram.get(y * 1024 + x), 0xF800, "pixel {} {}", x, y);
            }
        }
        assert_eq!(ram.get(2 * 1024 + 5), 0);
        assert_eq!(ram.get(5 * 1024 + 1), 0);
        assert!(e.drawn);
    }

    #[test]
    fn fills_clip() {
        let (mut e, ram) = engine();
        setup_16(&mut e, &ram);
        e.write(&ram, 0x0C, 3 << 16 | 3);
        e.write(&ram, 0x64, 0x1234);
        e.write(&ram, 0x68, 10 << 16 | 10);
        e.write(&ram, 0x6C, 0);
        e.write(&ram, 0x70, 0xCC << 24 | COMMAND_INITIATE | CMD_RECTFILL);
        assert_eq!(ram.get(2 * 1024 + 2), 0x1234);
        assert_eq!(ram.get(3 * 1024 + 2), 0);
        assert_eq!(ram.get(2 * 1024 + 3), 0);
    }

    #[test]
    fn screen_to_screen_copy() {
        let (mut e, ram) = engine();
        setup_16(&mut e, &ram);
        for x in 0..4 {
            ram.set(x, 0x100 + x as u16);
        }
        e.write(&ram, 0x54, 3 << 16 | 2048);
        e.write(&ram, 0x34, 0);
        e.write(&ram, 0x5C, 0);
        e.write(&ram, 0x68, 1 << 16 | 4);
        e.write(&ram, 0x6C, 10 << 16 | 20);
        e.write(
            &ram,
            0x70,
            0xCC << 24 | COMMAND_INITIATE | CMD_SCREEN_TO_SCREEN,
        );
        for x in 0..4 {
            assert_eq!(ram.get(10 * 1024 + 20 + x), 0x100 + x as u16);
        }
    }

    #[test]
    fn host_to_screen_mono_expands_through_the_colours() {
        let (mut e, ram) = engine();
        setup_16(&mut e, &ram);
        e.write(&ram, 0x54, COL_1BPP | PACKING_DWORD);
        e.write(&ram, 0x60, 0x0000);
        e.write(&ram, 0x64, 0xFFFF);
        e.write(&ram, 0x5C, 0);
        e.write(&ram, 0x68, 2 << 16 | 8);
        e.write(&ram, 0x6C, 0);
        e.write(&ram, 0x70, 0xCC << 24 | CMD_HOST_TO_SCREEN);
        // A row of 8 one-bit pixels is a dword; 10100101.
        e.write(&ram, 0x80, 0xA5);
        e.write(&ram, 0x80, 0xFF);
        let row: Vec<u16> = (0..8).map(|x| ram.get(x)).collect();
        assert_eq!(row, [0xFFFF, 0, 0xFFFF, 0, 0, 0xFFFF, 0, 0xFFFF]);
        assert!((0..8).all(|x| ram.get(1024 + x) == 0xFFFF));
    }

    #[test]
    fn tiled_destination_addresses() {
        let (mut e, ram) = engine();
        // Tiled, two tiles a row.
        e.write(&ram, 0x14, 3 << 16 | 2);
        e.write(&ram, 0x10, 0x8000_0000);
        assert_eq!(e.dst_stride, 2 * 4096);
        let mask = ram.bytes() as u32 - 1;
        // Byte 130 of row 33: the second tile across, the second tile row.
        assert_eq!(e.addr(130, 33, false, 0, mask), 2 + 4096 + 128 + 2 * 4096);
    }

    #[test]
    fn line_draws_both_ends() {
        let (mut e, ram) = engine();
        setup_16(&mut e, &ram);
        e.write(&ram, 0x64, 0x00FF);
        e.write(&ram, 0x5C, 1 << 16 | 1);
        e.write(&ram, 0x6C, 1 << 16 | 4);
        e.write(&ram, 0x70, 0xCC << 24 | COMMAND_INITIATE | CMD_LINE);
        assert!((1..=4).all(|x| ram.get(1024 + x) == 0x00FF));
        assert_eq!(ram.get(1024 + 5), 0);
    }
}
