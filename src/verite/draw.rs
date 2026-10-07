//! What Rendition's microcode draws: triangles, fans and strips of
//! RRedline's vertex types into the destination buffer, textured from the
//! current texture with perspective correction, shaded by the vertices'
//! colour or intensity, chroma-keyed and blended; block copies and fills.
//!
//! The vertices' values are 16.16 fixed point (RRedline Programming
//! Guide, "Choosing a Vertex Type"): X and Y in pixels, U and V in texels,
//! Q the perspective divisor, I an intensity of 0-255, K a packed RGB.
//! The state commands' meanings come from what Tomb Raider's Vérité
//! version sends, beside the names DOSBox's Rendition fork gives them.

/// The state the drawing commands use.
#[derive(Clone, Debug)]
pub struct DrawState {
    /// Where pixels go (1004h), bytes a line (143Bh, coded), and the
    /// clip (100Eh, 100Fh: width and height).
    pub dst_base: u32,
    pub dst_stride: u32,
    pub width: u32,
    pub height: u32,
    /// The current texture (4000h): base, bytes a line, the masks U and V
    /// wrap with, and what U and V are multiplied by into texels (16.16;
    /// 5028h, 5029h too).
    pub tex_base: u32,
    pub tex_stride: u32,
    pub tex_mask_u: u32,
    pub tex_mask_v: u32,
    pub scale_u: u32,
    pub scale_v: u32,
    /// How pixels get their colour (1231h, RRedline's VL_SetSrcFunc): 0 the
    /// vertices' colour, 1 the texture's, 3 the texture's times the
    /// vertices' colour or intensity.
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
    /// The colour of vertices without one (3013h, ARGB).
    pub fg: u32,
}

impl Default for DrawState {
    fn default() -> Self {
        Self {
            dst_base: 0,
            dst_stride: 1280,
            width: 640,
            height: 480,
            tex_base: 0,
            tex_stride: 512,
            tex_mask_u: 0xFF,
            tex_mask_v: 0xFF,
            scale_u: 0x10000,
            scale_v: 0x10000,
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
            fg: 0x00FF_FFFF,
        }
    }
}

crate::state_fields!(DrawState {
    dst_base, dst_stride, width, height, tex_base, tex_stride, tex_mask_u, tex_mask_v, scale_u, scale_v, src_mode,
    clamp_u, clamp_v,
    max_u, max_v, chroma, chroma_colour, chroma_mask, blend, blend_src, blend_dst, alpha, fg
});

/// A stride as the microcode takes it: the sum of two powers of two, the
/// low nibble's 2^(n+7) and, if not 0, the high nibble's 2^(n+5).
pub fn stride(code: u32) -> u32 {
    let (high, low) = (code >> 4 & 15, code & 15);
    let high = if high != 0 { 1 << (high + 5) } else { 0 };
    high + (1 << (low + 7))
}

/// A vertex, its attributes as floats.
#[derive(Clone, Copy, Debug, Default)]
pub struct Vertex {
    pub x: f32,
    pub y: f32,
    pub u: f32,
    pub v: f32,
    pub q: f32,
    /// Colour, 0-255 a channel, and intensity.
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub i: f32,
    pub has_colour: bool,
}

fn fixed(word: u32) -> f32 {
    word as i32 as f32 / 65536.0
}

/// A vertex of type `fields` (`cmd::vertex_fields`) from its words.
pub fn vertex(fields: &str, words: &[u32]) -> Vertex {
    let mut v = Vertex { q: 1.0, i: 255.0, ..Default::default() };
    let mut at = 0;
    let mut chars = fields.chars().peekable();
    while let Some(c) = chars.next() {
        let word = words.get(at).copied().unwrap_or(0);
        at += 1;
        match c {
            'x' => v.x = fixed(word),
            'y' => v.y = fixed(word),
            'u' => v.u = fixed(word),
            'v' => v.v = fixed(word),
            'q' => v.q = fixed(word),
            'i' => v.i = fixed(word),
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

fn read16(vram: &[u8], at: u32) -> u32 {
    let at = (at as usize % vram.len().max(2)) & !1;
    u16::from_le_bytes([vram[at], vram[at + 1]]) as u32
}

fn write16(vram: &mut [u8], at: u32, value: u32) {
    let at = (at as usize % vram.len().max(2)) & !1;
    vram[at..at + 2].copy_from_slice(&(value as u16).to_le_bytes());
}

fn rgb565(r: f32, g: f32, b: f32) -> u32 {
    let c = |v: f32, bits: u32| (v.clamp(0.0, 255.0) as u32) >> (8 - bits);
    c(r, 5) << 11 | c(g, 6) << 5 | c(b, 5)
}

fn from565(p: u32) -> (f32, f32, f32) {
    let (r, g, b) = (p >> 11 & 31, p >> 5 & 63, p & 31);
    ((r << 3 | r >> 2) as f32, (g << 2 | g >> 4) as f32, (b << 3 | b >> 2) as f32)
}

impl DrawState {
    /// A texel at u, v (scaled into texels), clamped or wrapped.
    fn texel(&self, vram: &[u8], u: f32, v: f32) -> u32 {
        let mut u = (u * fixed(self.scale_u)).floor() as i32;
        let mut v = (v * fixed(self.scale_v)).floor() as i32;
        if self.clamp_u {
            u = u.clamp(0, self.max_u.min(0xFFFF) as i32);
        }
        if self.clamp_v {
            v = v.clamp(0, self.max_v.min(0xFFFF) as i32);
        }
        let u = u as u32 & self.tex_mask_u;
        let v = v as u32 & self.tex_mask_v;
        read16(vram, self.tex_base + v * self.tex_stride + u * 2)
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
                let lerp = |f: fn(&Vertex) -> f32| w0 * f(a) + w1 * f(b) + w2 * f(c);
                let (mut r, mut g, mut bl);
                if textured {
                    let q = lerp(|v| v.q);
                    let (u, v) = if q != 0.0 {
                        (lerp(|v| v.u * v.q) / q, lerp(|v| v.v * v.q) / q)
                    } else {
                        (lerp(|v| v.u), lerp(|v| v.v))
                    };
                    let texel = self.texel(vram, u, v);
                    if self.chroma && texel & self.chroma_mask & 0xFFFF == self.chroma_colour & self.chroma_mask & 0xFFFF {
                        continue;
                    }
                    (r, g, bl) = from565(texel);
                    // Modulated by the vertices' colour, or intensity.
                    if self.src_mode == 3 {
                        if a.has_colour {
                            r *= lerp(|v| v.r) / 255.0;
                            g *= lerp(|v| v.g) / 255.0;
                            bl *= lerp(|v| v.b) / 255.0;
                        } else {
                            let i = lerp(|v| v.i) / 255.0;
                            r *= i;
                            g *= i;
                            bl *= i;
                        }
                    }
                } else if a.has_colour {
                    (r, g, bl) = (lerp(|v| v.r), lerp(|v| v.g), lerp(|v| v.b));
                } else {
                    (r, g, bl) = self.fg_colour();
                }
                self.plot(vram, px, py, (r, g, bl));
            }
        }
    }

    /// The colour of vertices without one.
    fn fg_colour(&self) -> (f32, f32, f32) {
        ((self.fg >> 16 & 0xFF) as f32, (self.fg >> 8 & 0xFF) as f32, (self.fg & 0xFF) as f32)
    }

    /// A pixel drawn: blended, if blending is on.
    fn plot(&self, vram: &mut [u8], x: i32, y: i32, (mut r, mut g, mut b): (f32, f32, f32)) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let at = self.dst_base + y as u32 * self.dst_stride + x as u32 * 2;
        if self.blend {
            let dst = from565(read16(vram, at));
            let a = self.alpha as f32 / 255.0;
            // The destination has no alpha: 1.
            let src_factor = |d: f32| match self.blend_src {
                0 => d / 255.0,
                1 => 1.0 - d / 255.0,
                2 => a,
                3 => 1.0 - a,
                4 => 1.0,
                5 | 6 => 0.0,
                // MIN(src alpha, 1 - dst alpha), dst alpha being 1.
                8 => 0.0,
                9 => 1.0,
                _ => 1.0,
            };
            let dst_factor = |c: f32| match self.blend_dst {
                0 => c / 255.0,
                1 => 1.0 - c / 255.0,
                2 => a,
                3 => 1.0 - a,
                4 => 1.0,
                5 | 6 => 0.0,
                _ => 1.0,
            };
            r = src_factor(dst.0) * r + dst_factor(r) * dst.0;
            g = src_factor(dst.1) * g + dst_factor(g) * dst.1;
            b = src_factor(dst.2) * b + dst_factor(b) * dst.2;
        }
        write16(vram, at, rgb565(r, g, b));
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
    fn strides_are_sums_of_powers_of_two() {
        assert_eq!(stride(0x51), 1280);
        assert_eq!(stride(0x02), 512);
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
}
