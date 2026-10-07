//! What Rendition's microcode draws: triangles, fans and strips of
//! RRedline's vertex types into the destination buffer, textured from the
//! current texture with perspective correction, point sampled or filtered,
//! shaded by the vertices' colour or intensity, chroma-keyed, Z-buffered,
//! fogged and blended; block copies and fills.
//!
//! The vertices' values are 16.16 fixed point (RRedline Programming
//! Guide, "Choosing a Vertex Type"): X and Y in pixels, U and V in texels,
//! Q the perspective divisor, I an intensity of 0-255, K a packed RGB, A
//! and F alpha and fog of 0-255, Z a depth of 0-65535. The state commands'
//! meanings come from Rendition's library (VLIB), which Tomb Raider's
//! Vérité version links whole, and from what the game sends.

/// RRedline's pixel formats (`V_PIXFMT_*`), as the library numbers them.
pub mod fmt {
    pub const P332: u32 = 1;
    pub const I8: u32 = 2;
    pub const A8: u32 = 3;
    pub const P565: u32 = 4;
    pub const P4444: u32 = 5;
    pub const P1555: u32 = 6;
    pub const P8888: u32 = 7;
    /// 4 bits a texel into a palette of 16 565, 4444 or 1555 entries.
    pub const I4_565: u32 = 8;
    pub const I4_4444: u32 = 9;
    pub const I4_1555: u32 = 10;
}

/// The state the drawing commands use.
#[derive(Clone, Debug)]
pub struct DrawState {
    /// Where pixels go (1004h), bytes a line (143Bh, coded), the clip
    /// (100Eh, 100Fh: width and height) and the pixel format (1006h).
    pub dst_base: u32,
    pub dst_stride: u32,
    pub width: u32,
    pub height: u32,
    pub dst_format: u32,
    /// The current texture (4000h): base, bytes a line, its last U and V
    /// (its size less 1: Quake's surfaces and skins aren't powers of two),
    /// and what U and V are multiplied by into texels (16.16; 5028h, 5029h
    /// too); its pixel format (1030h), whether its colours
    /// are BGR (16B7h), and the palette of the 4-bit formats (7020h).
    pub tex_base: u32,
    pub tex_stride: u32,
    pub tex_last_u: u32,
    pub tex_last_v: u32,
    pub scale_u: u32,
    pub scale_v: u32,
    pub src_format: u32,
    pub src_bgr: bool,
    pub palette: Vec<u32>,
    /// Bilinear filtering (13B2h, VL_SetSrcFilter), and what is added to
    /// the texel coordinates (602Ah, 602Bh: VL_SetSOffset, VL_SetTOffset;
    /// signed 16.16).
    pub filter: bool,
    pub s_offset: u32,
    pub t_offset: u32,
    /// How pixels get their colour (1231h, RRedline's VL_SetSrcFunc): 0 the
    /// vertices' colour, 1 the texture's, 2 the texture's over the
    /// vertices' by its alpha, 3 the texture's times the vertices'.
    pub src_mode: u32,
    /// Texture clamping (17BEh, 183Fh) to the largest U and V (1038h,
    /// 1839h).
    pub clamp_u: bool,
    pub clamp_v: bool,
    pub max_u: u32,
    pub max_v: u32,
    /// Chroma keying (15B5h): texels of this colour (1017h) under this
    /// mask (101Ah) aren't drawn.
    pub chroma: bool,
    pub chroma_colour: u32,
    pub chroma_mask: u32,
    /// Blending (89C6h) with the source's and the destination's factors
    /// (1241h, 1442h: RRedline's VL_SetBlendSrcFunc and VL_SetBlendDstFunc,
    /// numbered in the order the Programming Guide lists them), and the
    /// alpha of vertices without one (2055h, bits 16-23... of 0-255).
    pub blend: bool,
    pub blend_src: u32,
    pub blend_dst: u32,
    pub alpha: u32,
    /// Whether the destination is read for blending (1CCCh, set to
    /// VL_SetDstRdDisable's not), and the colour used when it isn't
    /// (1015h, ARGB).
    pub dst_read: bool,
    pub dst_colour: u32,
    /// The colour of vertices without one (3013h, ARGB).
    pub fg: u32,
    /// The Z buffer (1010h, 183Ch: base and coded bytes a line), its
    /// comparison (1643h, VL_SetZBufMode: 0 always) and writes (1844h),
    /// and the Z of vertices without one (205Ah, 16.16).
    pub z_base: u32,
    pub z_stride: u32,
    pub z_mode: u32,
    pub z_write: bool,
    pub z: u32,
    /// Fog (AA47h) of this colour (1016h, RGB), and the fog of vertices
    /// without one (205Bh, 16.16: 255 none, 0 all fog).
    pub fog: bool,
    pub fog_colour: u32,
    pub fog_default: u32,
}

impl Default for DrawState {
    fn default() -> Self {
        Self {
            dst_base: 0,
            dst_stride: 1280,
            width: 640,
            height: 480,
            dst_format: fmt::P565,
            tex_base: 0,
            tex_stride: 512,
            tex_last_u: 0xFF,
            tex_last_v: 0xFF,
            scale_u: 0x10000,
            scale_v: 0x10000,
            src_format: fmt::P565,
            src_bgr: false,
            palette: vec![0; 16],
            filter: false,
            s_offset: 0,
            t_offset: 0,
            src_mode: 0,
            clamp_u: false,
            clamp_v: false,
            max_u: u32::MAX,
            max_v: u32::MAX,
            chroma: false,
            chroma_colour: 0,
            chroma_mask: u32::MAX,
            blend: false,
            blend_src: 7,
            blend_dst: 6,
            alpha: 0xFF,
            dst_read: true,
            dst_colour: 0,
            fg: 0x00FF_FFFF,
            z_base: 0,
            z_stride: 1280,
            z_mode: 0,
            z_write: false,
            z: 0,
            fog: false,
            fog_colour: 0,
            fog_default: 255 << 16,
        }
    }
}

crate::state_fields!(DrawState {
    dst_base, dst_stride, width, height, dst_format, tex_base, tex_stride, tex_last_u, tex_last_v, scale_u, scale_v,
    src_format, src_bgr, palette, filter, s_offset, t_offset, src_mode, clamp_u, clamp_v, max_u, max_v, chroma,
    chroma_colour, chroma_mask, blend, blend_src, blend_dst, alpha, dst_read, dst_colour, fg, z_base, z_stride,
    z_mode, z_write, z, fog, fog_colour, fog_default
});

/// A stride as the microcode takes it: the sum of the high nibble's and
/// the low nibble's parts, as Rendition's library (VLIB) turns a code
/// back into bytes.
pub fn stride(code: u32) -> u32 {
    const HIGH: [u32; 16] = [0, 16, 32, 64, 128, 1024, 2048, 4096, 0, 0, 0, 0, 0, 0, 0, 0];
    const LOW: [u32; 16] = [0, 256, 512, 1024, 2048, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    HIGH[(code >> 4 & 15) as usize] + LOW[(code & 15) as usize]
}

/// A vertex, its attributes as floats.
#[derive(Clone, Copy, Debug, Default)]
pub struct Vertex {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub u: f32,
    pub v: f32,
    pub q: f32,
    /// Colour, 0-255 a channel, alpha and fog.
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
    pub f: f32,
    pub has_colour: bool,
    pub has_alpha: bool,
    pub has_fog: bool,
    pub has_z: bool,
}

fn fixed(word: u32) -> f32 {
    word as i32 as f32 / 65536.0
}

/// A vertex of type `fields` (`cmd::vertex_fields`) from its words.
pub fn vertex(fields: &str, words: &[u32]) -> Vertex {
    let mut v = Vertex { q: 1.0, ..Default::default() };
    let mut at = 0;
    let mut chars = fields.chars().peekable();
    while let Some(c) = chars.next() {
        let word = words.get(at).copied().unwrap_or(0);
        at += 1;
        match c {
            'x' => v.x = fixed(word),
            'y' => v.y = fixed(word),
            'z' => (v.z, v.has_z) = (fixed(word), true),
            'u' => v.u = fixed(word),
            'v' => v.v = fixed(word),
            'q' => v.q = fixed(word),
            'a' => (v.a, v.has_alpha) = (fixed(word), true),
            'f' => (v.f, v.has_fog) = (fixed(word), true),
            // Monochrome: all three channels.
            'i' => {
                let i = fixed(word);
                (v.r, v.g, v.b, v.has_colour) = (i, i, i, true);
            }
            'k' => {
                v.r = (word >> 16 & 0xFF) as f32;
                v.g = (word >> 8 & 0xFF) as f32;
                v.b = (word & 0xFF) as f32;
                v.has_colour = true;
            }
            'r' => {
                // R, G and B, a word each.
                v.r = fixed(word);
                v.g = fixed(words.get(at).copied().unwrap_or(0));
                v.b = fixed(words.get(at + 1).copied().unwrap_or(0));
                v.has_colour = true;
                at += 2;
                chars.next();
                chars.next();
            }
            _ => {}
        }
    }
    v
}

fn wrap(vram: &[u8], at: u32, bytes: usize) -> usize {
    (at as usize % vram.len().max(bytes)) & !(bytes - 1)
}

fn read8(vram: &[u8], at: u32) -> u32 {
    vram[wrap(vram, at, 1)] as u32
}

fn read16(vram: &[u8], at: u32) -> u32 {
    let at = wrap(vram, at, 2);
    u16::from_le_bytes([vram[at], vram[at + 1]]) as u32
}

fn read32(vram: &[u8], at: u32) -> u32 {
    let at = wrap(vram, at, 4);
    u32::from_le_bytes([vram[at], vram[at + 1], vram[at + 2], vram[at + 3]])
}

fn write16(vram: &mut [u8], at: u32, value: u32) {
    let at = wrap(vram, at, 2);
    vram[at..at + 2].copy_from_slice(&(value as u16).to_le_bytes());
}

/// A pixel of `bits` bits written.
fn write(vram: &mut [u8], at: u32, bits: u32, value: u32) {
    match bits {
        8 => {
            let at = wrap(vram, at, 1);
            vram[at] = value as u8;
        }
        32 => {
            let at = wrap(vram, at, 4);
            vram[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        _ => write16(vram, at, value),
    }
}

/// The bits of a pixel of `format`.
pub fn bits(format: u32) -> u32 {
    match format {
        fmt::I4_565..=fmt::I4_1555 => 4,
        fmt::P332..=fmt::A8 => 8,
        fmt::P8888 => 32,
        _ => 16,
    }
}

/// A channel of `n` bits widened to 0-255.
fn widen(value: u32, n: u32) -> f32 {
    let value = value & ((1 << n) - 1);
    (match n {
        1 => value * 255,
        2 => value * 0x55,
        3 => value << 5 | value << 2 | value >> 1,
        4 => value * 17,
        5 => value << 3 | value >> 2,
        6 => value << 2 | value >> 4,
        _ => value,
    }) as f32
}

/// A colour, RGBA of 0-255.
pub type Rgba = [f32; 4];

/// A pixel of `format` as RGBA; colours without alpha have 255, alphas
/// without colour are white. The 4-bit formats look their texel up in
/// `palette`.
pub fn decode(format: u32, raw: u32, palette: &[u32]) -> Rgba {
    let (r, g, b, a) = match format {
        fmt::P332 => (widen(raw >> 5, 3), widen(raw >> 2, 3), widen(raw, 2), 255.0),
        fmt::I8 => {
            let i = (raw & 0xFF) as f32;
            (i, i, i, 255.0)
        }
        fmt::A8 => (255.0, 255.0, 255.0, (raw & 0xFF) as f32),
        fmt::P4444 => (widen(raw >> 8, 4), widen(raw >> 4, 4), widen(raw, 4), widen(raw >> 12, 4)),
        fmt::P1555 => (widen(raw >> 10, 5), widen(raw >> 5, 5), widen(raw, 5), widen(raw >> 15, 1)),
        fmt::P8888 => (widen(raw >> 16, 8), widen(raw >> 8, 8), widen(raw, 8), widen(raw >> 24, 8)),
        fmt::I4_565..=fmt::I4_1555 => {
            let entry = palette.get(raw as usize & 15).copied().unwrap_or(0) & 0xFFFF;
            return decode(format - fmt::I4_565 + fmt::P565, entry, palette);
        }
        _ => (widen(raw >> 11, 5), widen(raw >> 5, 6), widen(raw, 5), 255.0),
    };
    [r, g, b, a]
}

/// RGBA as a pixel of `format`.
pub fn encode(format: u32, [r, g, b, a]: Rgba) -> u32 {
    let c = |v: f32, n: u32| (v.clamp(0.0, 255.0) as u32) >> (8 - n);
    match format {
        fmt::P332 => c(r, 3) << 5 | c(g, 3) << 2 | c(b, 2),
        fmt::I8 => c((r + g + b) / 3.0, 8),
        fmt::A8 => c(a, 8),
        fmt::P4444 => c(a, 4) << 12 | c(r, 4) << 8 | c(g, 4) << 4 | c(b, 4),
        fmt::P1555 => c(a, 1) << 15 | c(r, 5) << 10 | c(g, 5) << 5 | c(b, 5),
        fmt::P8888 => c(a, 8) << 24 | c(r, 8) << 16 | c(g, 8) << 8 | c(b, 8),
        _ => c(r, 5) << 11 | c(g, 6) << 5 | c(b, 5),
    }
}

fn argb(colour: u32) -> Rgba {
    [(colour >> 16 & 0xFF) as f32, (colour >> 8 & 0xFF) as f32, (colour & 0xFF) as f32, (colour >> 24) as f32]
}

fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

impl DrawState {
    /// The raw texel at whole texel coordinates, clamped or wrapped.
    fn texel(&self, vram: &[u8], mut u: i32, mut v: i32) -> u32 {
        if self.clamp_u {
            u = u.clamp(0, self.max_u.min(0xFFFF) as i32);
        }
        if self.clamp_v {
            v = v.clamp(0, self.max_v.min(0xFFFF) as i32);
        }
        // Wrapped around the size, which is a mask for powers of two.
        let u = u.rem_euclid(self.tex_last_u as i32 + 1) as u32;
        let v = v.rem_euclid(self.tex_last_v as i32 + 1) as u32;
        let line = self.tex_base.wrapping_add(v * self.tex_stride);
        match bits(self.src_format) {
            // The even texel in the low nibble.
            4 => read8(vram, line + u / 2) >> (4 * (u & 1)) & 15,
            8 => read8(vram, line + u),
            32 => read32(vram, line + u * 4),
            _ => read16(vram, line + u * 2),
        }
    }

    /// Whether a raw texel is the chroma key.
    fn keyed(&self, raw: u32) -> bool {
        let n = bits(self.src_format);
        let mask = self.chroma_mask & if n == 32 { u32::MAX } else { (1 << n) - 1 };
        self.chroma && raw & mask == self.chroma_colour & mask
    }

    /// A raw texel's colour.
    fn colour(&self, raw: u32) -> Rgba {
        let mut c = decode(self.src_format, raw, &self.palette);
        if self.src_bgr {
            c.swap(0, 2);
        }
        c
    }

    /// The texture's colour at u, v (texels before scaling), point sampled
    /// or filtered from the four texels around it; `None` where the chroma
    /// key hides it.
    fn sample(&self, vram: &[u8], u: f32, v: f32) -> Option<Rgba> {
        let s = u * fixed(self.scale_u) + fixed(self.s_offset);
        let t = v * fixed(self.scale_v) + fixed(self.t_offset);
        let (s0, t0) = (s.floor(), t.floor());
        let (x, y) = (s0 as i32, t0 as i32);
        if !self.filter {
            let raw = self.texel(vram, x, y);
            return (!self.keyed(raw)).then(|| self.colour(raw));
        }
        // Keyed texels leave the filter: the others are weighted up to
        // make the whole, unless the keyed ones weigh half or more.
        let (fs, ft) = (s - s0, t - t0);
        let taps = [(0, 0, (1.0 - fs) * (1.0 - ft)), (1, 0, fs * (1.0 - ft)), (0, 1, (1.0 - fs) * ft), (1, 1, fs * ft)];
        let mut sum = [0.0; 4];
        let mut weight = 0.0;
        for (dx, dy, w) in taps {
            let raw = self.texel(vram, x + dx, y + dy);
            if self.keyed(raw) {
                continue;
            }
            let c = self.colour(raw);
            for k in 0..4 {
                sum[k] += w * c[k];
            }
            weight += w;
        }
        (weight > 0.5).then(|| sum.map(|c| c / weight))
    }

    /// Whether a pixel of depth `new` passes over `old`: the mode's bits
    /// are the comparisons that pass, 1 less, 2 equal, 4 greater (Quake's
    /// 6 draws what is as near or nearer, its depths being 1/Z); 0 tests
    /// nothing.
    fn z_pass(&self, new: u32, old: u32) -> bool {
        let relation = match new.cmp(&old) {
            std::cmp::Ordering::Less => 1,
            std::cmp::Ordering::Equal => 2,
            std::cmp::Ordering::Greater => 4,
        };
        self.z_mode == 0 || self.z_mode & relation != 0
    }

    /// Where pixel x, y's depth is.
    fn z_at(&self, x: i32, y: i32) -> u32 {
        self.z_base.wrapping_add(y as u32 * self.z_stride + x as u32 * 2)
    }

    /// Whether a pixel of depth `depth` (16.16) at x, y passes the Z test.
    fn z_test(&self, vram: &[u8], x: i32, y: i32, depth: u32) -> bool {
        self.z_mode == 0 || self.z_pass(depth, read16(vram, self.z_at(x, y)))
    }

    /// A pixel drawn with its depth: tested, plotted and its depth kept.
    fn plot_z(&self, vram: &mut [u8], x: i32, y: i32, depth: u32, colour: Rgba) {
        if self.z_test(vram, x, y, depth) && self.plot(vram, x, y, colour) && self.z_write {
            write16(vram, self.z_at(x, y), depth);
        }
    }

    /// A triangle.
    pub fn triangle(&self, vram: &mut [u8], a: &Vertex, b: &Vertex, c: &Vertex) {
        let area = (b.x - a.x) * (c.y - a.y) - (c.x - a.x) * (b.y - a.y);
        if area == 0.0 || !area.is_finite() {
            return;
        }
        let x0 = a.x.min(b.x).min(c.x).floor().max(0.0) as i32;
        let x1 = a.x.max(b.x).max(c.x).ceil().min(self.width as f32) as i32;
        let y0 = a.y.min(b.y).min(c.y).floor().max(0.0) as i32;
        let y1 = a.y.max(b.y).max(c.y).ceil().min(self.height as f32) as i32;
        let textured = self.src_mode != 0;
        let z_on = self.z_mode != 0 || self.z_write;
        // Attributes the vertices lack take the state's defaults.
        let fg = argb(self.fg);
        let alpha = self.alpha as f32;
        let fog = fixed(self.fog_default);
        let z = fixed(self.z);
        let colour_of = |v: &Vertex| if v.has_colour { [v.r, v.g, v.b] } else { [fg[0], fg[1], fg[2]] };
        let (ca, cb, cc) = (colour_of(a), colour_of(b), colour_of(c));
        let alpha_of = |v: &Vertex| if v.has_alpha { v.a } else { alpha };
        let fog_of = |v: &Vertex| if v.has_fog { v.f } else { fog };
        let z_of = |v: &Vertex| if v.has_z { v.z } else { z };
        for py in y0..y1 {
            let fy = py as f32 + 0.5;
            for px in x0..x1 {
                let fx = px as f32 + 0.5;
                // Barycentric weights, with the top-left rule's ties to
                // the edges that have area on the inside.
                let w0 = ((b.x - fx) * (c.y - fy) - (c.x - fx) * (b.y - fy)) / area;
                let w1 = ((c.x - fx) * (a.y - fy) - (a.x - fx) * (c.y - fy)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let blend3 = |pa: f32, pb: f32, pc: f32| w0 * pa + w1 * pb + w2 * pc;
                let lerp = |f: &dyn Fn(&Vertex) -> f32| blend3(f(a), f(b), f(c));
                // Depth: linear across the screen, kept 16.0.
                let depth = if z_on { lerp(&z_of).clamp(0.0, 65535.0) as u32 } else { 0 };
                if !self.z_test(vram, px, py, depth) {
                    continue;
                }
                let colour = [0, 1, 2].map(|k| blend3(ca[k], cb[k], cc[k]));
                let alpha = lerp(&alpha_of);
                let mut pixel = [colour[0], colour[1], colour[2], alpha];
                if textured {
                    let q = lerp(&|v| v.q);
                    let (u, v) = if q != 0.0 {
                        (lerp(&|v| v.u * v.q) / q, lerp(&|v| v.v * v.q) / q)
                    } else {
                        (lerp(&|v| v.u), lerp(&|v| v.v))
                    };
                    let Some(texel) = self.sample(vram, u, v) else { continue };
                    pixel = match self.src_mode {
                        // Decal: the texture over the colour by its alpha.
                        2 => {
                            let t = texel[3] / 255.0;
                            [mix(colour[0], texel[0], t), mix(colour[1], texel[1], t), mix(colour[2], texel[2], t), alpha]
                        }
                        // Modulate.
                        3 => [0, 1, 2, 3].map(|k| pixel[k] * texel[k] / 255.0),
                        _ => texel,
                    };
                }
                if self.fog {
                    let f = (lerp(&fog_of) / 255.0).clamp(0.0, 1.0);
                    let fc = argb(self.fog_colour);
                    for k in 0..3 {
                        pixel[k] = mix(fc[k], pixel[k], f);
                    }
                }
                if self.plot(vram, px, py, pixel) && self.z_write {
                    write16(vram, self.z_at(px, py), depth);
                }
            }
        }
    }

    /// The colour of vertices without one.
    fn fg_colour(&self) -> Rgba {
        let mut c = argb(self.fg);
        c[3] = self.alpha as f32;
        c
    }

    /// A pixel drawn, blended if blending is on; whether it was inside.
    fn plot(&self, vram: &mut [u8], x: i32, y: i32, mut src: Rgba) -> bool {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return false;
        }
        let n = bits(self.dst_format);
        let at = self.dst_base + y as u32 * self.dst_stride + x as u32 * n / 8;
        if self.blend {
            let dst = if self.dst_read {
                let raw = match n {
                    8 => read8(vram, at),
                    32 => read32(vram, at),
                    _ => read16(vram, at),
                };
                decode(self.dst_format, raw, &[])
            } else {
                argb(self.dst_colour)
            };
            let (sa, da) = (src[3] / 255.0, dst[3] / 255.0);
            let src_factor = |k: usize| match self.blend_src {
                0 => dst[k] / 255.0,
                1 => 1.0 - dst[k] / 255.0,
                2 => sa,
                3 => 1.0 - sa,
                4 => da,
                5 => 1.0 - da,
                6 => 0.0,
                8 => sa.min(1.0 - da),
                9 => 1.0 - sa.min(1.0 - da),
                _ => 1.0,
            };
            let dst_factor = |k: usize| match self.blend_dst {
                0 => src[k] / 255.0,
                1 => 1.0 - src[k] / 255.0,
                2 => sa,
                3 => 1.0 - sa,
                4 => da,
                5 => 1.0 - da,
                6 => 0.0,
                _ => 1.0,
            };
            src = [0, 1, 2, 3].map(|k| src_factor(k) * src[k] + dst_factor(k) * dst[k]);
        }
        write(vram, at, n, encode(self.dst_format, src));
        true
    }

    /// A rectangle of the colour of vertices without one: its width and
    /// height, then its centre, 16.16.
    pub fn rectangle(&self, vram: &mut [u8], w: u32, h: u32, x: u32, y: u32) {
        let (w, h, x, y) = (fixed(w), fixed(h), fixed(x), fixed(y));
        let (x0, y0) = ((x - w / 2.0).round() as i32, (y - h / 2.0).round() as i32);
        let colour = self.fg_colour();
        for py in y0..y0 + h.round() as i32 {
            for px in x0..x0 + w.round() as i32 {
                self.plot(vram, px, py, colour);
            }
        }
    }

    /// A line between two whole pixels (X in the high 16 bits, Y in the
    /// low), both ends drawn, in the colour of vertices without one.
    pub fn int_line(&self, vram: &mut [u8], a: u32, b: u32) {
        let (mut x, mut y) = ((a >> 16) as i16 as i32, a as i16 as i32);
        let (x1, y1) = ((b >> 16) as i16 as i32, b as i16 as i32);
        let (dx, dy) = ((x1 - x).abs(), -(y1 - y).abs());
        let (sx, sy) = (if x < x1 { 1 } else { -1 }, if y < y1 { 1 } else { -1 });
        let mut err = dx + dy;
        let colour = self.fg_colour();
        for _ in 0..4096 {
            self.plot(vram, x, y, colour);
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }

    /// Quake's spans, textured with perspective and Z-buffered: how S/Z,
    /// T/Z, 1/Z and the depth change a pixel to the right, then spans of
    /// their first X and Y (high and low 16 bits), how many pixels, and
    /// those values at the first pixel (as vQuake writes Quake's spans).
    /// S/Z and T/Z over 1/Z are U and V; the depth is Quake's, 1/Z in
    /// 16.16.
    pub fn spans(&self, vram: &mut [u8], steps: &[u32], spans: &[u32]) {
        let step = |i: usize| steps[i] as i32 as f32;
        for span in spans.as_chunks::<6>().0 {
            let (x0, y, count) = ((span[0] >> 16) as i16 as i32, (span[0] & 0xFFFF) as i32, span[1] as i32);
            let first = |i: usize| span[2 + i] as i32 as f32;
            for i in 0..count.min(4096) {
                let at = |k: usize| first(k) + i as f32 * step(k);
                let zi = at(2);
                if zi == 0.0 {
                    continue;
                }
                let depth = (at(3) / 65536.0).clamp(0.0, 65535.0) as u32;
                let Some(texel) = self.sample(vram, at(0) / zi, at(1) / zi) else { continue };
                self.plot_z(vram, x0 + i, y, depth, texel);
            }
        }
    }

    /// A particle: a square at X, Y (high and low 16 bits) of width and
    /// height (the same), its depth (16.16) and its colour (RGB).
    pub fn particle(&self, vram: &mut [u8], words: &[u32]) {
        let (x0, y0) = ((words[0] >> 16) as i16 as i32, words[0] as i16 as i32);
        let (w, h) = ((words[1] >> 16) as i32, (words[1] & 0xFFFF) as i32);
        let depth = (words[2] >> 16).min(0xFFFF);
        let mut colour = argb(words[3]);
        colour[3] = 255.0;
        for y in y0..y0 + h {
            for x in x0..x0 + w {
                self.plot_z(vram, x, y, depth, colour);
            }
        }
    }

    /// A rectangle of 8-bit indices, each the current texture's texel at
    /// that U (V 0): Quake's 256 colours turned into the destination's.
    /// At X, Y (high and low 16 bits) of width and height (the same), its
    /// lines padded to words.
    pub fn lookup(&self, vram: &mut [u8], at: u32, size: u32, indices: &[u8]) {
        let (x0, y0) = ((at >> 16) as i32, (at & 0xFFFF) as i32);
        let (w, h) = ((size >> 16) as usize, (size & 0xFFFF) as usize);
        let n = bits(self.dst_format);
        for (row, line) in indices.chunks(w.next_multiple_of(4).max(4)).take(h).enumerate() {
            let y = y0 + row as i32;
            for (col, &index) in line.iter().take(w).enumerate() {
                let x = x0 + col as i32;
                let raw = self.texel(vram, index as i32, 0);
                if self.keyed(raw) || x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
                    continue;
                }
                let at = self.dst_base + y as u32 * self.dst_stride + x as u32 * n / 8;
                write(vram, at, n, encode(self.dst_format, self.colour(raw)));
            }
        }
    }

    /// Copy a block from the current texture's buffer to the destination:
    /// source X and Y, width and height, destination X and Y (16 bits
    /// each, high and low).
    pub fn bitblt(&self, vram: &mut [u8], src: u32, size: u32, dst: u32) {
        let (sx, sy) = (src >> 16, src & 0xFFFF);
        let (w, h) = (size >> 16, size & 0xFFFF);
        let (dx, dy) = (dst >> 16, dst & 0xFFFF);
        for y in 0..h {
            for x in 0..w {
                let texel = read16(vram, self.tex_base + (sy + y) * self.tex_stride + (sx + x) * 2);
                if self.chroma && texel & self.chroma_mask & 0xFFFF == self.chroma_colour & self.chroma_mask & 0xFFFF {
                    continue;
                }
                write16(vram, self.dst_base + (dy + y) * self.dst_stride + (dx + x) * 2, texel);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strides_are_sums_of_two_parts() {
        assert_eq!(stride(0x51), 1280);
        assert_eq!(stride(0x02), 512);
        // Quake's surfaces.
        assert_eq!(stride(0x20), 32);
        assert_eq!(stride(0x41), 384);
        assert_eq!(stride(0x53), 2048);
    }

    #[test]
    fn a_flat_triangle_fills_its_inside() {
        let mut vram = vec![0u8; 1280 * 480];
        let s = DrawState::default();
        let v = |x: f32, y: f32| Vertex { x, y, r: 255.0, has_colour: true, q: 1.0, ..Default::default() };
        s.triangle(&mut vram, &v(0.0, 0.0), &v(16.0, 0.0), &v(0.0, 16.0));
        assert_eq!(read16(&vram, 2 * 2 + 2 * 1280), 0xF800);
        assert_eq!(read16(&vram, 15 * 2 + 15 * 1280), 0);
    }

    fn from565(p: u32) -> (f32, f32, f32) {
        let [r, g, b, _] = decode(fmt::P565, p, &[]);
        (r, g, b)
    }

    const RED: u32 = 0xF800;
    const BLUE: u32 = 0x001F;

    /// A 16x16 triangle over the top left, its vertices `v`.
    fn corner(s: &DrawState, vram: &mut [u8], v: Vertex) {
        let at = |x: f32, y: f32| Vertex { x, y, ..v };
        s.triangle(vram, &at(0.0, 0.0), &at(16.0, 0.0), &at(0.0, 16.0));
    }

    fn flat(r: f32, b: f32, z: f32) -> Vertex {
        Vertex { r, b, z, q: 1.0, has_colour: true, has_z: true, ..Default::default() }
    }

    #[test]
    fn the_z_buffer_keeps_the_nearer_pixel() {
        let mut vram = vec![0u8; 0x10_0000];
        // Z at 512 KB, cleared to the farthest; less passes (mode 1).
        let s = DrawState { z_base: 0x8_0000, z_mode: 1, z_write: true, ..Default::default() };
        for at in (0x8_0000..0x8_0000 + 1280 * 16).step_by(2) {
            write16(&mut vram, at, 0xFFFF);
        }
        corner(&s, &mut vram, flat(255.0, 0.0, 100.0));
        corner(&s, &mut vram, flat(0.0, 255.0, 200.0));
        assert_eq!(read16(&vram, 2 * 2 + 2 * 1280), RED, "the farther triangle is hidden");
        assert_eq!(read16(&vram, 0x8_0000 + 2 * 2 + 2 * 1280), 100);
        corner(&s, &mut vram, flat(0.0, 255.0, 50.0));
        assert_eq!(read16(&vram, 2 * 2 + 2 * 1280), BLUE, "the nearer one is drawn");
        // Without writes the buffer stays.
        let s = DrawState { z_write: false, z_mode: 0, ..s };
        corner(&s, &mut vram, flat(255.0, 0.0, 1000.0));
        assert_eq!(read16(&vram, 0x8_0000 + 2 * 2 + 2 * 1280), 50);
        assert_eq!(read16(&vram, 2 * 2 + 2 * 1280), RED, "always passes");
    }

    #[test]
    fn fog_mixes_in_its_colour() {
        let mut vram = vec![0u8; 1280 * 480];
        let s = DrawState { fog: true, fog_colour: 0x0000_00FF, ..Default::default() };
        let v = |f: f32| Vertex { f, has_fog: true, ..flat(255.0, 0.0, 0.0) };
        corner(&s, &mut vram, v(255.0));
        assert_eq!(read16(&vram, 4 + 2 * 1280), RED, "none");
        corner(&s, &mut vram, v(0.0));
        assert_eq!(read16(&vram, 4 + 2 * 1280), BLUE, "all fog");
        corner(&s, &mut vram, v(128.0));
        let (r, _, b) = from565(read16(&vram, 4 + 2 * 1280));
        assert!((120.0..136.0).contains(&r) && (120.0..136.0).contains(&b), "half: {} {}", r, b);
    }

    /// A texture at 256 KB of two texels a line, `left` and `right`.
    fn two_texels(format: u32, left: u32, right: u32) -> (DrawState, Vec<u8>) {
        let mut vram = vec![0u8; 0x8_0000];
        let n = bits(format) / 8;
        for line in 0..2u32 {
            write(&mut vram, 0x4_0000 + line * 512, n * 8, left);
            write(&mut vram, 0x4_0000 + line * 512 + n, n * 8, right);
        }
        let s = DrawState {
            tex_base: 0x4_0000,
            tex_last_u: 1,
            tex_last_v: 1,
            clamp_u: true,
            clamp_v: true,
            max_u: 1,
            max_v: 1,
            src_format: format,
            src_mode: 1,
            ..Default::default()
        };
        (s, vram)
    }

    #[test]
    fn bilinear_filtering_mixes_neighbouring_texels() {
        let (mut s, vram) = two_texels(fmt::P565, RED, BLUE);
        assert_eq!(s.sample(&vram, 0.5, 0.0), Some(decode(fmt::P565, RED, &[])), "point sampled");
        s.filter = true;
        let mid = s.sample(&vram, 0.5, 0.0).unwrap();
        assert!((mid[0] - 127.5).abs() < 1.0 && (mid[2] - 127.5).abs() < 1.0, "{:?}", mid);
        // The offset of half a texel back puts texel centres on whole texels.
        s.s_offset = (-0.5f32 * 65536.0) as i32 as u32;
        assert_eq!(s.sample(&vram, 0.5, 0.0), Some(decode(fmt::P565, RED, &[])));
    }

    #[test]
    fn texture_alpha_blends() {
        // 4444: red at half alpha over blue, by the source's alpha.
        let (mut s, mut vram) = two_texels(fmt::P4444, 0x8F00, 0x8F00);
        s.blend = true;
        (s.blend_src, s.blend_dst) = (2, 3);
        for x in 0..16 {
            for y in 0..16 {
                write16(&mut vram, x * 2 + y * 1280, BLUE);
            }
        }
        let v = Vertex { q: 1.0, ..Default::default() };
        corner(&s, &mut vram, v);
        let (r, g, b) = from565(read16(&vram, 4 + 2 * 1280));
        assert!((r - 136.0).abs() < 10.0 && g == 0.0 && (b - 119.0).abs() < 10.0, "{} {} {}", r, g, b);
    }

    #[test]
    fn formats_decode() {
        assert_eq!(decode(fmt::P1555, 0x8000 | 31 << 10, &[]), [255.0, 0.0, 0.0, 255.0]);
        assert_eq!(decode(fmt::P1555, 31, &[]), [0.0, 0.0, 255.0, 0.0]);
        assert_eq!(decode(fmt::P8888, 0x80FF_4020, &[]), [255.0, 64.0, 32.0, 128.0]);
        assert_eq!(decode(fmt::A8, 0x40, &[]), [255.0, 255.0, 255.0, 64.0]);
        assert_eq!(decode(fmt::P332, 0xE0, &[]), [255.0, 0.0, 0.0, 255.0]);
        let palette: Vec<u32> = (0..16).map(|i| if i == 3 { BLUE } else { 0 }).collect();
        assert_eq!(decode(fmt::I4_565, 3, &palette), [0.0, 0.0, 255.0, 255.0]);
        for format in [fmt::P565, fmt::P4444, fmt::P1555, fmt::P8888] {
            let c = [255.0, 0.0, 255.0, 255.0];
            assert_eq!(decode(format, encode(format, c), &[]), c, "format {}", format);
        }
    }

    #[test]
    fn four_bit_texels_take_the_low_nibble_first() {
        let (mut s, mut vram) = two_texels(fmt::P565, 0, 0);
        s.src_format = fmt::I4_565;
        vram[0x4_0000] = 0x21;
        assert_eq!(s.texel(&vram, 0, 0), 1);
        assert_eq!(s.texel(&vram, 1, 0), 2);
    }
}
