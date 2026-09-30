//! The S3 ViRGE's 3D engine, the S3d Engine's 3D lines and triangles
//! behind the memory-mapped registers at B000h-B1FFh (lines) and
//! B400h-B5FFh (triangles): Gouraud shaded, lit or unlit textured, with or
//! without perspective correction, Z buffered, fogged and alpha blended,
//! into 8, 16 (ZRGB1555) or 24 bit video memory. Direct3D's HAL on
//! Windows 95 draws through it with S3's driver.
//!
//! Like the 2D engine (`virge`) it draws a command completely when the
//! register that starts it is written, so it is never busy.
//!
//! Ported from DOSBox-X's vga_s3d.cpp, which follows the S3 ViRGE databook
//! (DB019-B, sections 15.4.5-15.4.8 and 19.4) and the structure of 86Box's
//! vid_s3_virge.c for the edge walk, the sub-pixel pre-step, texture
//! addressing and perspective division. It keeps DOSBox-X's deliberate
//! differences from 86Box: Z compare 000b draws without a Z test; Z start
//! and deltas are all S16.15; 8 bit destinations get the blue channel;
//! palettized and Blend4 texels are decoded; M2TPP and M8TPP interpolate
//! two mipmap levels and V2TPP filters vertically; flat textures use
//! DEST_SRC_STR's source stride; Gouraud triangles have no fog; and every
//! video memory access is bounds checked, with pixels outside the 11-bit
//! coordinate space discarded.

// Command Set register (B100h, B500h) fields.
const CMD_AE: u32 = 1 << 0;
const CMD_HC: u32 = 1 << 1;
const CMD_FE: u32 = 1 << 17;
const CMD_ZUP: u32 = 1 << 23;
const CMD_TWE: u32 = 1 << 26;
const CMD_3D: u32 = 1 << 31;

fn cmd_dest_fmt(c: u32) -> u32 {
    c >> 2 & 7
}
fn cmd_tex_fmt(c: u32) -> u32 {
    c >> 5 & 7
}
fn cmd_mip_size(c: u32) -> u32 {
    c >> 8 & 15
}
fn cmd_filter(c: u32) -> u32 {
    c >> 12 & 7
}
fn cmd_tex_blend(c: u32) -> u32 {
    c >> 15 & 3
}
fn cmd_abc(c: u32) -> u32 {
    c >> 18 & 3
}
fn cmd_zb_comp(c: u32) -> u32 {
    c >> 20 & 7
}
fn cmd_zb_mode(c: u32) -> u32 {
    c >> 24 & 3
}
/// The command: bits 30-27.
pub fn cmd_command(c: u32) -> u32 {
    c >> 27 & 15
}

const GOURAUD_TRI: u32 = 0;
const LIT_TEX_TRI: u32 = 1;
const UNLIT_TEX_TRI: u32 = 2;
const LIT_TEX_TRI_PERSP: u32 = 5;
const UNLIT_TEX_TRI_PERSP: u32 = 6;
const LINE: u32 = 8;
const NOP: u32 = 15;

const TEX_ARGB8888: u32 = 0;
const TEX_ARGB4444: u32 = 1;
const TEX_ARGB1555: u32 = 2;
const TEX_ALPHA4BLEND4: u32 = 3;
const TEX_BLEND4_LO: u32 = 4;
const TEX_BLEND4_HI: u32 = 5;
const TEX_PAL8: u32 = 6;
const TEX_YUV: u32 = 7;

/// The engine's registers as written, by (offset & 1FCh) / 4 within the 3D
/// line block (`line`) and the 3D triangle block (`tri`). Everything is
/// decoded when a command runs.
#[derive(Clone, Debug)]
pub struct S3d {
    pub line: [u32; 0x80],
    pub tri: [u32; 0x80],
    /// Commands drawn, for the debugger: lines, then triangles.
    pub lines: u64,
    pub triangles: u64,
}

impl Default for S3d {
    fn default() -> Self {
        Self { line: [0; 0x80], tri: [0; 0x80], lines: 0, triangles: 0 }
    }
}

crate::state_fields!(S3d { line, tri } skip { lines, triangles });

/// What a command draws into: video memory, and the DAC's colours for
/// palettized textures in a 16 or 24 bit destination.
pub struct Target<'a> {
    pub vram: &'a mut [u8],
    pub palette: &'a dyn Fn(u8) -> (u8, u8, u8),
}

/// Registers with the same mnemonic in the line and triangle blocks are one
/// register with two addresses: Z_BASE, DEST_BASE, CLIP_L_R, CLIP_T_B,
/// DEST_SRC_STR, Z_STRIDE, FOG_CLR and CMD_SET.
fn is_shared(off: u32) -> bool {
    matches!(off, 0xD4 | 0xD8 | 0xDC | 0xE0 | 0xE4 | 0xE8 | 0xF4 | 0x100)
}

#[derive(Clone, Copy, Default)]
struct Color {
    r: i32,
    g: i32,
    b: i32,
    a: i32,
}

/// The values interpolated across a triangle: colours S8.7, Z S16.15, and
/// the texture coordinates, W and the mipmap level D.
#[derive(Clone, Copy, Default)]
struct Attr {
    r: i32,
    g: i32,
    b: i32,
    a: i32,
    z: i32,
    u: i32,
    v: i32,
    w: i32,
    d: i32,
}

impl Attr {
    fn add(&mut self, d: &Attr) {
        self.r = self.r.wrapping_add(d.r);
        self.g = self.g.wrapping_add(d.g);
        self.b = self.b.wrapping_add(d.b);
        self.a = self.a.wrapping_add(d.a);
        self.z = self.z.wrapping_add(d.z);
        self.u = self.u.wrapping_add(d.u);
        self.v = self.v.wrapping_add(d.v);
        self.w = self.w.wrapping_add(d.w);
        self.d = self.d.wrapping_add(d.d);
    }

    /// Plus `d` times `n`/32 (a sub-pixel step).
    fn add_fraction(&self, d: &Attr, n: i32) -> Attr {
        let f = |base: i32, delta: i32| base.wrapping_add(delta.wrapping_mul(n) >> 5);
        Attr {
            r: f(self.r, d.r),
            g: f(self.g, d.g),
            b: f(self.b, d.b),
            a: f(self.a, d.a),
            z: f(self.z, d.z),
            u: f(self.u, d.u),
            v: f(self.v, d.v),
            w: f(self.w, d.w),
            d: f(self.d, d.d),
        }
    }

    /// Plus `d` times `n`.
    fn add_times(&mut self, d: &Attr, n: i32) {
        let f = |base: i32, delta: i32| base.wrapping_add(delta.wrapping_mul(n));
        *self = Attr {
            r: f(self.r, d.r),
            g: f(self.g, d.g),
            b: f(self.b, d.b),
            a: f(self.a, d.a),
            z: f(self.z, d.z),
            u: f(self.u, d.u),
            v: f(self.v, d.v),
            w: f(self.w, d.w),
            d: f(self.d, d.d),
        };
    }
}

fn clamp8(x: i32) -> i32 {
    x.clamp(0, 255)
}

fn lo16s(v: u32) -> i32 {
    v as u16 as i16 as i32
}

fn hi16s(v: u32) -> i32 {
    (v >> 16) as u16 as i16 as i32
}

/// Everything a pixel needs, decoded once per command.
struct Context<'a, 'b> {
    t: &'a mut Target<'b>,
    memsize: u32,
    memmask: u32,
    bypp: u32,
    dest_base: u32,
    dest_stride: u32,
    z_base: u32,
    z_stride: u32,
    hc: bool,
    clip_l: i32,
    clip_r: i32,
    clip_t: i32,
    clip_b: i32,
    textured: bool,
    lit: bool,
    persp: bool,
    palettized: bool,
    tex_blend: u32,
    fog: bool,
    abc: u32,
    zmode: u32,
    zcomp: u32,
    zup: bool,
    ztest: bool,
    fog_rgb: (i32, i32, i32),
    tex_fmt: u32,
    tex_bypp: u32,
    filter: u32,
    max_d: i32,
    wrap: bool,
    tex_level_base: [u32; 10],
    flat_stride: u32,
    tbu: i32,
    tbv: i32,
    tex_bdr_clr: u32,
    color0: Color,
    color1: Color,
}

impl<'a, 'b> Context<'a, 'b> {
    fn new(t: &'a mut Target<'b>, cmd: u32, regs: &[u32; 0x80]) -> Option<Self> {
        let memsize = t.vram.len() as u32;
        if memsize == 0 {
            return None;
        }
        let bypp = match cmd_dest_fmt(cmd) {
            0 => 1,
            1 => 2,
            2 => 3,
            _ => return None,
        };
        // Bits 21-3 for 4 MB, bit 22 with more.
        let bmask = if memsize > 4 << 20 { 0x7F_FFF8 } else { 0x3F_FFF8 };
        let reg = |off: usize| regs[off >> 2];
        let zmode = cmd_zb_mode(cmd);
        let zcomp = cmd_zb_comp(cmd);
        let fog = reg(0xF4);
        Some(Context {
            memsize,
            memmask: memsize.next_power_of_two() - 1,
            bypp,
            z_base: reg(0xD4) & bmask,
            dest_base: reg(0xD8) & bmask,
            clip_r: (reg(0xDC) & 0x7FF) as i32,
            clip_l: (reg(0xDC) >> 16 & 0x7FF) as i32,
            clip_b: (reg(0xE0) & 0x7FF) as i32,
            clip_t: (reg(0xE0) >> 16 & 0x7FF) as i32,
            dest_stride: reg(0xE4) >> 16 & 0xFF8,
            z_stride: reg(0xE8) & 0xFF8,
            hc: cmd & CMD_HC != 0,
            fog_rgb: ((fog >> 16 & 0xFF) as i32, (fog >> 8 & 0xFF) as i32, (fog & 0xFF) as i32),
            // Z buffering is on with mode 00b and a compare other than 000b
            // (15.4.6); MUX buffering needs a 16 bit destination (15.4.7).
            ztest: zmode == 0 && zcomp != 0,
            zmode: if (zmode == 1 || zmode == 2) && bypp != 2 { 3 } else { zmode },
            zcomp,
            zup: cmd & CMD_ZUP != 0,
            abc: cmd_abc(cmd),
            textured: false,
            lit: false,
            persp: false,
            palettized: false,
            tex_blend: 2,
            fog: false,
            tex_fmt: 0,
            tex_bypp: 1,
            filter: 0,
            max_d: 0,
            wrap: false,
            tex_level_base: [0; 10],
            flat_stride: 0,
            tbu: 0,
            tbv: 0,
            tex_bdr_clr: 0,
            color0: Color::default(),
            color1: Color::default(),
            t,
        })
    }

    fn setup_texture(&mut self, cmd: u32, tri: &[u32; 0x80]) {
        const TEX_BYTES: [u32; 8] = [4, 2, 2, 1, 1, 1, 1, 2];
        let reg = |off: usize| tri[off >> 2];
        self.tex_fmt = cmd_tex_fmt(cmd);
        self.tex_bypp = TEX_BYTES[self.tex_fmt as usize];
        self.filter = cmd_filter(cmd);
        self.max_d = cmd_mip_size(cmd).min(9) as i32;
        self.wrap = cmd & CMD_TWE != 0;
        self.palettized = self.tex_fmt == TEX_PAL8;
        // Palettized texels can only be used unfiltered (15.4.8.1).
        if self.palettized {
            self.filter = if self.filter < 4 { 0 } else { 4 };
        }
        if self.filter == 7 {
            self.filter = 4;
        }
        // Mipmap levels are stored largest first, each right after the
        // one before; level n is 2^n x 2^n texels.
        let bmask = if self.memsize > 4 << 20 { 0x7F_FFF8 } else { 0x3F_FFF8 };
        let mut base = reg(0xEC) & bmask;
        for lv in (0..=9).rev() {
            self.tex_level_base[lv] = base;
            if lv as i32 <= self.max_d {
                base = base.wrapping_add((1u32 << (2 * lv)) * self.tex_bypp);
            }
        }
        // Flat textures may have their own row pitch.
        self.flat_stride = if self.filter >= 4 { reg(0xE4) & 0xFF8 } else { 0 };
        // TBU/TBV are (4+s).(16-s); the U/V accumulators are (4+s).(27-s).
        self.tbu = ((reg(0x108) & 0xF_FFFF) << 11) as i32;
        self.tbv = ((reg(0x104) & 0xF_FFFF) << 11) as i32;
        self.tex_bdr_clr = reg(0xF0) & 0xFF_FFFF;
        let color = |c: u32| Color { r: (c >> 16 & 0xFF) as i32, g: (c >> 8 & 0xFF) as i32, b: (c & 0xFF) as i32, a: 255 };
        self.color0 = color(reg(0xF8));
        self.color1 = color(reg(0xFC));
    }

    fn byte(&self, addr: u32) -> u32 {
        self.t.vram.get((addr & self.memmask) as usize).copied().unwrap_or(0) as u32
    }

    fn word(&self, addr: u32) -> u32 {
        self.byte(addr) | self.byte(addr.wrapping_add(1)) << 8
    }

    fn dword(&self, addr: u32) -> u32 {
        self.word(addr) | self.word(addr.wrapping_add(2)) << 16
    }

    /// Blend4: the 4-bit texel interpolates from COLOR0 (0) to COLOR1 (15).
    fn blend4(&self, t: i32, o: &mut Color) {
        let (c0, c1) = (self.color0, self.color1);
        o.r = c0.r + (c1.r - c0.r) * t / 15;
        o.g = c0.g + (c1.g - c0.g) * t / 15;
        o.b = c0.b + (c1.b - c0.b) * t / 15;
    }

    fn decode_texel(&self, val: u32, pairval: u32, odd: bool) -> Color {
        let mut o = Color::default();
        match self.tex_fmt {
            TEX_ARGB8888 => {
                o = Color { b: (val & 0xFF) as i32, g: (val >> 8 & 0xFF) as i32, r: (val >> 16 & 0xFF) as i32, a: (val >> 24 & 0xFF) as i32 }
            }
            TEX_ARGB4444 => {
                o = Color {
                    b: ((val & 0xF) * 0x11) as i32,
                    g: ((val >> 4 & 0xF) * 0x11) as i32,
                    r: ((val >> 8 & 0xF) * 0x11) as i32,
                    a: ((val >> 12 & 0xF) * 0x11) as i32,
                }
            }
            TEX_ARGB1555 => o = rgb1555(val, if val & 0x8000 != 0 { 255 } else { 0 }),
            TEX_ALPHA4BLEND4 => {
                // The databook doesn't say which nibble is alpha; the high
                // one, where the other formats have it.
                self.blend4((val & 0xF) as i32, &mut o);
                o.a = ((val >> 4 & 0xF) * 0x11) as i32;
            }
            TEX_BLEND4_LO => {
                self.blend4((val & 0xF) as i32, &mut o);
                o.a = 255;
            }
            TEX_BLEND4_HI => {
                self.blend4((val >> 4 & 0xF) as i32, &mut o);
                o.a = 255;
            }
            TEX_PAL8 => {
                // The index goes in the blue channel, to reach an 8 bit
                // destination unchanged; other destinations (which the
                // databook doesn't allow) get the DAC's colour.
                let idx = (val & 0xFF) as u8;
                if self.bypp == 1 {
                    o = Color { r: 0, g: 0, b: idx as i32, a: 255 };
                } else {
                    let (r, g, b) = (self.t.palette)(idx);
                    o = Color { r: r as i32, g: g as i32, b: b as i32, a: 255 };
                }
            }
            _ => {
                // YU/YV: Y in each texel's low byte, U (even texels) or V
                // (odd) in the high byte; BT.601.
                let y = (val & 0xFF) as i32 - 16;
                let uu = ((if odd { pairval } else { val }) >> 8 & 0xFF) as i32 - 128;
                let vv = ((if odd { val } else { pairval }) >> 8 & 0xFF) as i32 - 128;
                o.r = clamp8((298 * y + 409 * vv + 128) >> 8);
                o.g = clamp8((298 * y - 100 * uu - 208 * vv + 128) >> 8);
                o.b = clamp8((298 * y + 516 * uu + 128) >> 8);
                o.a = 255;
            }
        }
        o
    }

    /// Texel (`ui`, `vi`) of mipmap level `level`.
    fn fetch_texel(&self, level: i32, ui: i32, vi: i32) -> Color {
        let size = 1i32 << level;
        let (ui, vi) = (ui & (size - 1), vi & (size - 1));
        let row = if self.flat_stride != 0 { self.flat_stride } else { size as u32 * self.tex_bypp };
        let addr = self.tex_level_base[level as usize]
            .wrapping_add(vi as u32 * row)
            .wrapping_add(ui as u32 * self.tex_bypp)
            & self.memmask;
        let (val, pairval) = match self.tex_bypp {
            4 => (self.dword(addr), 0),
            2 => {
                let pair = if self.tex_fmt == TEX_YUV { self.word((addr ^ 2) & self.memmask) } else { 0 };
                (self.word(addr), pair)
            }
            _ => (self.byte(addr), 0),
        };
        self.decode_texel(val, pairval, ui & 1 != 0)
    }

    /// The border colour, stored in the texel format.
    fn border_texel(&self) -> Color {
        self.decode_texel(self.tex_bdr_clr, self.tex_bdr_clr, false)
    }

    /// Level `level` at `u`, `v` (1.0 = 1 << 27), with 1 tap (nearest), 2
    /// (a vertical pair, V2TPP) or 4 (bilinear).
    fn sample_level(&self, level: i32, u: i32, v: i32, taps: u32) -> Color {
        let shift = 27 - level;
        // Without wrapping, beyond the texture is the border colour
        // (15.4.8.3).
        if !self.wrap && (u | v) as u32 & 0xF800_0000 != 0 {
            return self.border_texel();
        }
        let (ui, vi) = (u >> shift, v >> shift);
        if taps == 1 {
            return self.fetch_texel(level, ui, vi);
        }
        // The 8 bits below the texel position.
        let du = (u >> (shift - 8)) & 0xFF;
        let dv = (v >> (shift - 8)) & 0xFF;
        let last = (1 << level) - 1;
        // Without wrapping, neighbours past the edge repeat it rather than
        // bleeding the border colour into the last row and column.
        let ui1 = if self.wrap || ui < last { ui + 1 } else { ui };
        let vi1 = if self.wrap || vi < last { vi + 1 } else { vi };
        if taps == 2 {
            let (t0, t2) = (self.fetch_texel(level, ui, vi), self.fetch_texel(level, ui, vi1));
            let mix = |a: i32, b: i32| (a * (256 - dv) + b * dv) >> 8;
            return Color { r: mix(t0.r, t2.r), g: mix(t0.g, t2.g), b: mix(t0.b, t2.b), a: mix(t0.a, t2.a) };
        }
        let t0 = self.fetch_texel(level, ui, vi);
        let t1 = self.fetch_texel(level, ui1, vi);
        let t2 = self.fetch_texel(level, ui, vi1);
        let t3 = self.fetch_texel(level, ui1, vi1);
        let (w0, w1, w2, w3) = ((256 - du) * (256 - dv), du * (256 - dv), (256 - du) * dv, du * dv);
        let mix = |a: i32, b: i32, c: i32, d: i32| (a * w0 + b * w1 + c * w2 + d * w3) >> 16;
        Color {
            r: mix(t0.r, t1.r, t2.r, t3.r),
            g: mix(t0.g, t1.g, t2.g, t3.g),
            b: mix(t0.b, t1.b, t2.b, t3.b),
            a: mix(t0.a, t1.a, t2.a, t3.a),
        }
    }

    fn sample_texture(&self, at: &Attr) -> Color {
        let (u, v) = if self.persp {
            // U and V are premultiplied by W. The shift is the original
            // ViRGE's and ViRGE/VX's (86Box has 8 for the ViRGE/DX on).
            let mut w: i64 = 0;
            if at.w > 0 {
                w = ((1i64 << 46) / at.w as i64).min(0x7FFF_FFFF);
            }
            let shift = 12 + self.max_d;
            (
                ((at.u as i64 * w) >> shift) as i32 + self.tbu,
                ((at.v as i64 * w) >> shift) as i32 + self.tbv,
            )
        } else {
            (at.u.wrapping_add(self.tbu), at.v.wrapping_add(self.tbv))
        };
        // The mipmap level from D's integer part (S4.27).
        let mut level = self.max_d;
        let mut dfrac = 0;
        if self.filter < 4 && at.d >= 0 {
            level = self.max_d - (at.d >> 27 & 0xF);
            dfrac = at.d >> 19 & 0xFF;
            if level < 0 {
                level = 0;
                dfrac = 0;
            }
        }
        match self.filter {
            // M1TPP, 1TPP: the nearest texel.
            0 | 4 => self.sample_level(level, u, v, 1),
            5 => self.sample_level(level, u, v, 2),
            // M4TPP, 4TPP: bilinear.
            2 | 6 => self.sample_level(level, u, v, 4),
            // M2TPP, M8TPP: levels D and D+1, interpolated.
            1 | 3 => {
                let taps = if self.filter == 1 { 1 } else { 4 };
                let mut o = self.sample_level(level, u, v, taps);
                if level > 0 && dfrac != 0 {
                    let o2 = self.sample_level(level - 1, u, v, taps);
                    o.r += ((o2.r - o.r) * dfrac) >> 8;
                    o.g += ((o2.g - o.g) * dfrac) >> 8;
                    o.b += ((o2.b - o.b) * dfrac) >> 8;
                    o.a += ((o2.a - o.a) * dfrac) >> 8;
                }
                o
            }
            _ => self.sample_level(level, u, v, 1),
        }
    }

    fn write_word(&mut self, addr: u32, value: u16) {
        let len = self.t.vram.len();
        for (i, byte) in value.to_le_bytes().into_iter().enumerate() {
            if let Some(b) = self.t.vram.get_mut((addr as usize + i) % len) {
                *b = byte;
            }
        }
    }

    /// One pixel through the pipeline of the databook's figure 15-7.
    fn pixel(&mut self, x: i32, y: i32, at: &Attr) {
        // 11-bit unsigned coordinates; anything outside is dropped.
        if x as u32 > 2047 || y as u32 > 2047 {
            return;
        }
        if self.hc && (x < self.clip_l || x > self.clip_r || y < self.clip_t || y > self.clip_b) {
            return;
        }
        let dest = self.dest_base as u64 + y as u64 * self.dest_stride as u64 + x as u64 * self.bypp as u64;
        if dest + self.bypp as u64 > self.memsize as u64 {
            return;
        }
        let dest = dest as u32;
        let zs = z16(at.z);
        let mut z_addr = 0;
        if self.ztest {
            let za = self.z_base as u64 + y as u64 * self.z_stride as u64 + x as u64 * 2;
            if za + 2 > self.memsize as u64 {
                return;
            }
            z_addr = za as u32;
            if !z_pass(self.zcomp, zs, self.word(z_addr)) {
                return;
            }
        } else if self.zmode == 1 {
            // MUX buffering's Z pass (15.4.7): bit 15 marks a Z value.
            let d = self.word(dest);
            let zs15 = zs >> 1;
            if d & 0x8000 != 0 && !z_pass(self.zcomp, zs15, d & 0x7FFF) {
                return;
            }
            self.write_word(dest, (zs15 | 0x8000) as u16);
            return;
        } else if self.zmode == 2 {
            // MUX buffering's draw pass: only where the Z value matches.
            let d = self.word(dest);
            if d & 0x8000 == 0 || zs >> 1 != d & 0x7FFF {
                return;
            }
        }
        // The vertex colour.
        let src = Color { r: clamp8(at.r >> 7), g: clamp8(at.g >> 7), b: clamp8(at.b >> 7), a: clamp8(at.a >> 7) };
        let mut col = src;
        if self.textured {
            col = self.sample_texture(at);
            if self.lit && !self.palettized {
                match self.tex_blend {
                    // Complex reflection: add, saturating.
                    0 => {
                        col.r = clamp8(col.r + src.r);
                        col.g = clamp8(col.g + src.g);
                        col.b = clamp8(col.b + src.b);
                    }
                    // Modulate.
                    1 => {
                        col.r = (col.r * src.r) >> 8;
                        col.g = (col.g * src.g) >> 8;
                        col.b = (col.b * src.b) >> 8;
                    }
                    // Decal.
                    _ => {}
                }
            }
        }
        if !self.palettized {
            if self.fog {
                // The source alpha mixes the pixel with the fog colour.
                let a = src.a;
                let (fr, fg, fb) = self.fog_rgb;
                col.r = (col.r * a + fr * (255 - a)) / 255;
                col.g = (col.g * a + fg * (255 - a)) / 255;
                col.b = (col.b * a + fb * (255 - a)) / 255;
            }
            if (self.abc == 2 || self.abc == 3) && self.bypp != 1 {
                // 10b: the alpha at this stage; 11b: the source alpha.
                let a = if self.abc == 3 { src.a } else { clamp8(col.a) };
                let d = if self.bypp == 2 {
                    rgb1555(self.word(dest), 0)
                } else {
                    Color { b: self.byte(dest) as i32, g: self.byte(dest + 1) as i32, r: self.byte(dest + 2) as i32, a: 0 }
                };
                col.r = (col.r * a + d.r * (255 - a)) / 255;
                col.g = (col.g * a + d.g * (255 - a)) / 255;
                col.b = (col.b * a + d.b * (255 - a)) / 255;
            }
        }
        let (r, g, b) = (clamp8(col.r) as u32, clamp8(col.g) as u32, clamp8(col.b) as u32);
        let d = dest as usize;
        match self.bypp {
            1 => self.t.vram[d] = b as u8,
            // ZRGB1555: bit 15, MUX buffering's Z flag, is written 0.
            2 => self.write_word(dest, (b >> 3 | (g >> 3) << 5 | (r >> 3) << 10) as u16),
            _ => {
                self.t.vram[d] = b as u8;
                self.t.vram[d + 1] = g as u8;
                self.t.vram[d + 2] = r as u8;
            }
        }
        if self.ztest && self.zup {
            self.write_word(z_addr, zs as u16);
        }
    }

    /// One of a triangle's two parts, walking up from `y` (15.4.5.2): the
    /// first along side 01, the second along 12. `x1` runs along side 02.
    #[allow(clippy::too_many_arguments)]
    fn tri_part(&mut self, dx_: &Attr, dy_: &Attr, xdir: i32, y: &mut i32, x1: &mut i32, mut x2: i32, dx1: i32, dx2: i32, ycount: i32, base: &mut Attr) {
        for _ in 0..ycount.min(2048) {
            let visible = *y as u32 <= 2047 && (!self.hc || (*y >= self.clip_t && *y <= self.clip_b));
            if visible {
                let mut x = x1.wrapping_add((1 << 20) - 1) >> 20;
                let mut xe = x2.wrapping_add((1 << 20) - 1) >> 20;
                if xdir < 0 {
                    x -= 1;
                    xe -= 1;
                }
                if x != xe && ((xdir > 0 && x < xe) || (xdir < 0 && x > xe)) {
                    // The sub-pixel step from the edge to the first pixel, in
                    // 1/32 pixels.
                    let pre = x1.wrapping_sub(1) >> 15;
                    let dx = if xdir > 0 { ((31 - pre) & 0x1F) + 1 } else { pre & 0x1F };
                    let mut at = base.add_fraction(dx_, dx);
                    let mut draw = true;
                    if self.hc {
                        let mut skip = 0;
                        if xdir > 0 {
                            if x > self.clip_r || xe <= self.clip_l {
                                draw = false;
                            } else {
                                xe = xe.min(self.clip_r + 1);
                                if x < self.clip_l {
                                    skip = self.clip_l - x;
                                    x = self.clip_l;
                                }
                            }
                        } else if x < self.clip_l || xe >= self.clip_r {
                            draw = false;
                        } else {
                            xe = xe.max(self.clip_l - 1);
                            if x > self.clip_r {
                                skip = x - self.clip_r;
                                x = self.clip_r;
                            }
                        }
                        if skip > 0 {
                            at.add_times(dx_, skip);
                        }
                    }
                    if draw {
                        let count = if xdir > 0 { xe - x } else { x - xe }.min(4096);
                        for _ in 0..count {
                            self.pixel(x, *y, &at);
                            at.add(dx_);
                            x += xdir;
                        }
                    }
                }
            }
            *x1 = x1.wrapping_add(dx1);
            x2 = x2.wrapping_add(dx2);
            base.add(dy_);
            *y -= 1;
        }
    }
}

fn rgb1555(val: u32, a: i32) -> Color {
    Color {
        b: ((val & 0x1F) << 3 | (val & 0x1C) >> 2) as i32,
        g: ((val & 0x3E0) >> 2 | (val & 0x380) >> 7) as i32,
        r: ((val & 0x7C00) >> 7 | (val & 0x7000) >> 12) as i32,
        a,
    }
}

fn z_pass(comp: u32, zs: u32, zzb: u32) -> bool {
    match comp {
        0 => false,
        1 => zs > zzb,
        2 => zs == zzb,
        3 => zs >= zzb,
        4 => zs < zzb,
        5 => zs != zzb,
        6 => zs <= zzb,
        _ => true,
    }
}

/// S16.15 as a 16-bit depth.
fn z16(z: i32) -> u32 {
    if z < 0 { 0 } else { ((z as u32) >> 15).min(0xFFFF) }
}

impl S3d {
    /// Write a register of the line (B000h-B1FFh) or triangle
    /// (B400h-B5FFh) block, `len` bytes of it; false if `port` isn't one.
    /// A command runs when CMD_SET is written without autoexecute, or with
    /// it when the last register (17Ch) is. True in `drew` if one ran.
    pub fn write(&mut self, port: u16, val: u32, len: u8, t: &mut Target<'_>, drew: &mut bool) -> bool {
        let blk = port & 0xFE00;
        if blk != 0xB000 && blk != 0xB400 {
            return false;
        }
        let is_line = blk == 0xB000;
        let off = (port & 0x1FF) as u32;
        let regs = if is_line { &mut self.line } else { &mut self.tri };
        let reg = &mut regs[(off >> 2) as usize];
        // Byte and word writes merge into the doubleword.
        if len == 4 && off & 3 == 0 {
            *reg = val;
        } else {
            let sh = (off & 3) * 8;
            let msk = (if len >= 4 { 0xFFFF_FFFFu32 } else { (1u32 << (len as u32 * 8)) - 1 }) << sh;
            *reg = (*reg & !msk) | ((val << sh) & msk);
        }
        let value = *reg;
        let aoff = off & 0x1FC;
        if is_shared(aoff) {
            self.line[(aoff >> 2) as usize] = value;
            self.tri[(aoff >> 2) as usize] = value;
        }
        // Only once the register's last byte is written.
        if (off & 3) + (len as u32) < 4 {
            return true;
        }
        if aoff == 0x100 {
            if value & CMD_AE == 0 {
                *drew |= self.execute(value, t);
            }
        } else if aoff == 0x17C {
            let cmd = if is_line { self.line[0x40] } else { self.tri[0x40] };
            if cmd & CMD_AE != 0 {
                let command = cmd_command(cmd);
                if is_line == (command == LINE) {
                    *drew |= self.execute(cmd, t);
                }
            }
        }
        true
    }

    /// A register of the line or triangle block read back, if `port` is
    /// one.
    pub fn read(&self, port: u16, len: u8) -> Option<u32> {
        let blk = port & 0xFE00;
        if blk != 0xB000 && blk != 0xB400 {
            return None;
        }
        let off = (port & 0x1FF) as u32;
        let regs = if blk == 0xB000 { &self.line } else { &self.tri };
        let v = regs[(off >> 2) as usize] >> ((off & 3) * 8);
        Some(if len < 4 { v & ((1u32 << (len as u32 * 8)) - 1) } else { v })
    }

    pub fn reset(&mut self) {
        self.line = [0; 0x80];
        self.tri = [0; 0x80];
    }

    /// Run command `cmd`; true if it was one that draws (the S3D DONE
    /// status bit follows).
    fn execute(&mut self, cmd: u32, t: &mut Target<'_>) -> bool {
        if cmd & CMD_3D == 0 {
            return false;
        }
        match cmd_command(cmd) {
            NOP => return false,
            LINE => {
                self.lines += 1;
                self.draw_line(t);
            }
            GOURAUD_TRI | LIT_TEX_TRI | UNLIT_TEX_TRI | LIT_TEX_TRI_PERSP | UNLIT_TEX_TRI_PERSP => {
                self.triangles += 1;
                self.draw_triangle(t);
            }
            _ => return false,
        }
        true
    }

    fn draw_triangle(&self, t: &mut Target<'_>) {
        let tri = &self.tri;
        let reg = |off: usize| tri[off >> 2];
        let cmd = reg(0x100);
        let Some(mut c) = Context::new(t, cmd, tri) else { return };
        match cmd_command(cmd) {
            GOURAUD_TRI => {}
            LIT_TEX_TRI | LIT_TEX_TRI_PERSP => {
                c.textured = true;
                c.lit = true;
            }
            UNLIT_TEX_TRI | UNLIT_TEX_TRI_PERSP => c.textured = true,
            _ => return,
        }
        if c.textured {
            c.persp = matches!(cmd_command(cmd), LIT_TEX_TRI_PERSP | UNLIT_TEX_TRI_PERSP);
            c.tex_blend = cmd_tex_blend(cmd);
            c.setup_texture(cmd, tri);
            c.fog = cmd & CMD_FE != 0;
        }
        let dx = Attr {
            b: lo16s(reg(0x13C)),
            g: hi16s(reg(0x13C)),
            r: lo16s(reg(0x140)),
            a: hi16s(reg(0x140)),
            w: reg(0x10C) as i32,
            d: reg(0x118) as i32,
            v: reg(0x11C) as i32,
            u: reg(0x120) as i32,
            z: reg(0x154) as i32,
        };
        let dy = Attr {
            b: lo16s(reg(0x144)),
            g: hi16s(reg(0x144)),
            r: lo16s(reg(0x148)),
            a: hi16s(reg(0x148)),
            w: reg(0x110) as i32,
            d: reg(0x124) as i32,
            v: reg(0x128) as i32,
            u: reg(0x12C) as i32,
            z: reg(0x158) as i32,
        };
        let mut base = Attr {
            b: (reg(0x14C) & 0xFFFF) as i32,
            g: (reg(0x14C) >> 16) as i32,
            r: (reg(0x150) & 0xFFFF) as i32,
            a: (reg(0x150) >> 16) as i32,
            w: reg(0x114) as i32,
            d: reg(0x130) as i32,
            v: reg(0x134) as i32,
            u: reg(0x138) as i32,
            z: reg(0x15C) as i32,
        };
        let ycnt = reg(0x17C);
        let ty12 = (ycnt & 0x7FF) as i32;
        let ty01 = (ycnt >> 16 & 0x7FF) as i32;
        let xdir = if ycnt & 0x8000_0000 != 0 { 1 } else { -1 };
        let mut y = (reg(0x178) & 0x7FF) as i32;
        let mut x1 = reg(0x174) as i32;
        let dx02 = reg(0x170) as i32;
        c.tri_part(&dx, &dy, xdir, &mut y, &mut x1, reg(0x16C) as i32, dx02, reg(0x168) as i32, ty01, &mut base);
        c.tri_part(&dx, &dy, xdir, &mut y, &mut x1, reg(0x164) as i32, dx02, reg(0x160) as i32, ty12, &mut base);
    }

    /// A 3D line: 2D line drawing's X start, delta and ends, bottom up,
    /// with Z and colour stepped once a pixel along the major axis (as
    /// Mesa's s3v driver computes them).
    fn draw_line(&self, t: &mut Target<'_>) {
        let line = &self.line;
        let reg = |off: usize| line[off >> 2];
        let cmd = reg(0x100);
        let Some(mut c) = Context::new(t, cmd, line) else { return };
        c.fog = cmd & CMD_FE != 0;
        let step = Attr {
            b: lo16s(reg(0x144)),
            g: hi16s(reg(0x144)),
            r: lo16s(reg(0x148)),
            a: hi16s(reg(0x148)),
            z: reg(0x158) as i32,
            ..Attr::default()
        };
        let mut at = Attr {
            b: (reg(0x14C) & 0xFFFF) as i32,
            g: (reg(0x14C) >> 16) as i32,
            r: (reg(0x150) & 0xFFFF) as i32,
            a: (reg(0x150) >> 16) as i32,
            z: reg(0x15C) as i32,
            ..Attr::default()
        };
        // END1 is the last pixel (top scanline), END0 the first.
        let end1 = reg(0x16C) as u16 as i16 as i32;
        let end0 = (reg(0x16C) >> 16) as u16 as i16 as i32;
        let xdelta = reg(0x170) as i32;
        let mut xf = reg(0x174) as i32;
        let mut y = (reg(0x178) & 0x7FF) as i32;
        let ycount = (reg(0x17C) & 0x7FF) as i32;
        let xdir = if reg(0x17C) & 0x8000_0000 != 0 { 1 } else { -1 };
        let (lo, hi) = (end0.min(end1), end0.max(end1));
        if ycount <= 1 {
            // One scanline, END0 through END1.
            let mut x = end0;
            for _ in 0..4096 {
                c.pixel(x, y, &at);
                at.add(&step);
                if x == end1 {
                    break;
                }
                x += if end1 > x { 1 } else { -1 };
            }
        } else if (-(1 << 20)..=(1 << 20)).contains(&xdelta) {
            // Y major: a pixel a scanline.
            for _ in 0..ycount {
                let x = xf >> 20;
                if x >= lo && x <= hi {
                    c.pixel(x, y, &at);
                    at.add(&step);
                }
                xf = xf.wrapping_add(xdelta);
                y -= 1;
            }
        } else {
            // X major: a run a scanline, up to the accumulator.
            let mut x = end0;
            for n in (1..=ycount).rev() {
                let xto = if n == 1 { end1 } else { xf >> 20 };
                let mut guard = 4096;
                while guard > 0 && if xdir > 0 { x <= xto } else { x >= xto } {
                    guard -= 1;
                    if x >= lo && x <= hi {
                        c.pixel(x, y, &at);
                        at.add(&step);
                    }
                    x += xdir;
                }
                xf = xf.wrapping_add(xdelta);
                y -= 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STRIDE: u32 = 640 * 2;

    fn palette(_: u8) -> (u8, u8, u8) {
        (0, 0, 0)
    }

    fn pixel16(vram: &[u8], x: u32, y: u32) -> u16 {
        let at = (y * STRIDE + x * 2) as usize;
        u16::from_le_bytes([vram[at], vram[at + 1]])
    }

    /// Write the triangle block's register `off`.
    fn tri(s: &mut S3d, vram: &mut [u8], off: u16, value: u32) -> bool {
        let mut drew = false;
        let mut t = Target { vram, palette: &palette };
        assert!(s.write(0xB400 + off, value, 4, &mut t, &mut drew));
        drew
    }

    /// A Gouraud triangle whose sides are vertical: the rectangle x 10-19,
    /// 10 scanlines up from y 50, in a colour (8-bit components).
    fn rectangle(s: &mut S3d, vram: &mut [u8], (r, g, b): (u32, u32, u32), z: u32, cmd: u32) -> bool {
        tri(s, vram, 0xE4, STRIDE << 16);
        tri(s, vram, 0x14C, (g << 7) << 16 | b << 7);
        tri(s, vram, 0x150, (255 << 7) << 16 | r << 7);
        tri(s, vram, 0x15C, z << 15);
        tri(s, vram, 0x174, 10 << 20);
        tri(s, vram, 0x170, 0);
        tri(s, vram, 0x16C, 20 << 20);
        tri(s, vram, 0x168, 0);
        tri(s, vram, 0x178, 50);
        // TY01 = 10, TY12 = 0, left to right.
        tri(s, vram, 0x17C, 0x8000_0000 | 10 << 16);
        tri(s, vram, 0x100, cmd)
    }

    /// 3D, Gouraud, 16 bits per pixel, no Z buffer.
    const GOURAUD16: u32 = CMD_3D | 1 << 2 | 3 << 24;

    #[test]
    fn a_gouraud_triangle_fills_its_span_in_rgb1555() {
        let mut vram = vec![0u8; 1 << 20];
        let mut s = S3d::default();
        assert!(rectangle(&mut s, &mut vram, (255, 0, 0), 0, GOURAUD16));
        assert_eq!(s.triangles, 1);
        // Red is bits 14-10.
        assert_eq!(pixel16(&vram, 10, 50), 0x7C00);
        assert_eq!(pixel16(&vram, 19, 41), 0x7C00);
        // Outside the span and above the last scanline.
        assert_eq!(pixel16(&vram, 9, 50), 0);
        assert_eq!(pixel16(&vram, 20, 50), 0);
        assert_eq!(pixel16(&vram, 15, 40), 0);
        assert_eq!(pixel16(&vram, 15, 51), 0);
    }

    #[test]
    fn autoexecute_draws_at_the_y_count() {
        let mut vram = vec![0u8; 1 << 20];
        let mut s = S3d::default();
        // Armed by CMD_SET, drawn at each write of 17Ch.
        assert!(!rectangle(&mut s, &mut vram, (0, 0, 255), 0, GOURAUD16 | CMD_AE));
        assert_eq!(pixel16(&vram, 12, 45), 0);
        assert!(tri(&mut s, &mut vram, 0x17C, 0x8000_0000 | 10 << 16));
        assert_eq!(pixel16(&vram, 12, 45), 0x001F);
        // A line command in the triangle block doesn't run.
        tri(&mut s, &mut vram, 0x100, CMD_3D | CMD_AE | LINE << 27 | 1 << 2);
        assert!(!tri(&mut s, &mut vram, 0x17C, 0x8000_0000 | 10 << 16));
    }

    #[test]
    fn the_z_buffer_keeps_the_nearer_pixel() {
        let mut vram = vec![0u8; 1 << 20];
        let mut s = S3d::default();
        // Z buffer at 512 KB, compare "less", updated.
        tri(&mut s, &mut vram, 0xD4, 0x8_0000);
        tri(&mut s, &mut vram, 0xE8, STRIDE);
        let zless = CMD_3D | 1 << 2 | 4 << 20 | CMD_ZUP;
        for at in (0x8_0000..0x10_0000).step_by(2) {
            vram[at] = 0xFF;
            vram[at + 1] = 0xFF;
        }
        rectangle(&mut s, &mut vram, (255, 0, 0), 100, zless);
        assert_eq!(pixel16(&vram, 15, 45), 0x7C00);
        // Farther: rejected.
        rectangle(&mut s, &mut vram, (0, 255, 0), 200, zless);
        assert_eq!(pixel16(&vram, 15, 45), 0x7C00);
        // Nearer: drawn.
        rectangle(&mut s, &mut vram, (0, 0, 255), 50, zless);
        assert_eq!(pixel16(&vram, 15, 45), 0x001F);
    }

    #[test]
    fn source_alpha_blends_with_the_destination() {
        let mut vram = vec![0u8; 1 << 20];
        let mut s = S3d::default();
        rectangle(&mut s, &mut vram, (255, 0, 0), 0, GOURAUD16);
        // Blue at alpha 128 (of 255) over red: half and half.
        tri(&mut s, &mut vram, 0x150, (128 << 7) << 16);
        tri(&mut s, &mut vram, 0x14C, 255 << 7);
        tri(&mut s, &mut vram, 0x100, GOURAUD16 | 3 << 18);
        let p = pixel16(&vram, 15, 45);
        let (r, b) = (p >> 10 & 0x1F, p & 0x1F);
        assert!((14..=16).contains(&r) && (14..=16).contains(&b), "{:04X}", p);
    }

    #[test]
    fn registers_read_back_and_shared_ones_are_one() {
        let mut vram = vec![0u8; 1 << 16];
        let mut s = S3d::default();
        let mut t = Target { vram: &mut vram, palette: &palette };
        let mut drew = false;
        // A byte written into the triangle block's DEST_BASE shows in the
        // line block's too.
        s.write(0xB4D9, 0x12, 1, &mut t, &mut drew);
        assert_eq!(s.read(0xB0D8, 4), Some(0x1200));
        assert_eq!(s.read(0xB0D9, 1), Some(0x12));
        // Not the 3D engine's.
        assert_eq!(s.read(0xA4D8, 4), None);
        assert!(!s.write(0xA4D8, 0, 4, &mut t, &mut drew));
    }

    #[test]
    fn textures_decode_their_formats() {
        let mut vram = vec![0u8; 16];
        let mut t = Target { vram: &mut vram, palette: &|i| (i, i, i) };
        let regs = [0u32; 0x80];
        let mut c = Context::new(&mut t, CMD_3D | 1 << 2, &regs).unwrap();
        c.tex_fmt = TEX_ARGB1555;
        let o = c.decode_texel(0xFC00, 0, false);
        assert_eq!((o.r, o.g, o.b, o.a), (255, 0, 0, 255));
        c.tex_fmt = TEX_ARGB4444;
        let o = c.decode_texel(0x80F0, 0, false);
        assert_eq!((o.r, o.g, o.b, o.a), (0, 255, 0, 0x88));
        c.tex_fmt = TEX_PAL8;
        let o = c.decode_texel(0x40, 0, false);
        assert_eq!((o.r, o.g, o.b), (0x40, 0x40, 0x40));
    }
}
