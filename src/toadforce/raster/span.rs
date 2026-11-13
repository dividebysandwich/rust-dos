//! The steps of a draw run over the pixels of a row, 64 at a time.
//!
//! Every value of a pixel comes from the pixel's own coordinates, so a
//! row gives the same pixels whichever thread draws it and wherever a
//! run of 64 starts.

use super::combine::{self, Op};
use super::math;
use super::mem::Mem;
use super::pipeline::{Arg, DrawState, Fog, REGS, Reg, Step};
use super::sample::{Sampler, cube_face};
use super::setup::{Prim, colour};
use super::texel::{self, A, B, G, R, Rgba};
use crate::toadforce::format::Format;
use crate::toadforce::level1::depth_value;
use crate::toadforce::level1::states::STAGES;

/// The pixels worked on at once.
pub const N: usize = 64;

/// The fragments of up to `N` pixels of a row.
pub struct Chunk {
    /// The fragments still alive, bit `i` for pixel `i`.
    mask: u64,
    /// w, the reciprocal of what `Prim::q` interpolates.
    w: [f32; N],
    regs: [[Rgba; N]; REGS],
    /// What the last `Bump` adds to the next sample's coordinates, and
    /// multiplies the sample by.
    bump: [[f32; 2]; N],
    lum: [f32; N],
}

impl Chunk {
    pub fn new() -> Self {
        Chunk {
            mask: 0,
            w: [0.0; N],
            regs: [[[0; 4]; N]; REGS],
            bump: [[0.0; 2]; N],
            lum: [1.0; N],
        }
    }
}

impl Default for Chunk {
    fn default() -> Self {
        Self::new()
    }
}

/// A colour as a pixel of `format`, a render target's: each channel
/// rounded to the nearest value the format holds.
#[inline]
pub fn pack(format: Format, c: Rgba) -> u32 {
    match format {
        Format::R5G6B5 => {
            let to = |v: u8, max: u32| (v as u32 * max + 127) / 255;
            to(c[R], 31) << 11 | to(c[G], 63) << 5 | to(c[B], 31)
        }
        Format::X8R8G8B8 => texel::to_word(c) & 0x00FF_FFFF,
        _ => texel::to_word(c),
    }
}

/// A draw's state with card memory: what a thread draws rows with.
pub struct Ctx<'a> {
    mem: Mem<'a>,
    st: &'a DrawState,
    samplers: [Option<Sampler<'a>>; STAGES],
}

impl<'a> Ctx<'a> {
    pub fn new(mem: Mem<'a>, st: &'a DrawState) -> Self {
        Ctx {
            mem,
            st,
            samplers: std::array::from_fn(|stage| {
                st.tex[stage].as_ref().map(|tex| Sampler::new(mem, tex))
            }),
        }
    }

    /// Draw the pixels of `p` in row `y`.
    pub fn row(&self, p: &Prim, y: i32, chunk: &mut Chunk) {
        let Some((first, end)) = p.span(y) else {
            return;
        };
        let mut x = first;
        while x < end {
            let n = ((end - x) as usize).min(N);
            self.chunk(p, x, y, n, chunk);
            x += n as i32;
        }
    }

    /// Draw the `n` pixels of `p` from (`x`, `y`) to the right.
    fn chunk(&self, p: &Prim, x: i32, y: i32, n: usize, c: &mut Chunk) {
        let st = self.st;
        c.mask = if n == N { u64::MAX } else { (1u64 << n) - 1 };
        // Where the pixels are from the planes' origin.
        let fx = x as f32 - p.ox;
        let fy = y as f32 - p.oy;
        if p.q.flat() {
            c.w[..n].fill(1.0 / p.q.at);
        } else {
            for i in 0..n {
                c.w[i] = 1.0 / p.q.eval(fx + i as f32, fy);
            }
        }
        for step in &st.steps {
            match *step {
                Step::Depth {
                    func,
                    write,
                    stencil,
                } => self.depth(p, x, y, n, c, func, write, stencil),
                Step::Colours { specular } => {
                    match p.flat {
                        Some((diffuse, spec)) => {
                            c.regs[Reg::Diffuse as usize][..n].fill(diffuse);
                            c.regs[Reg::Specular as usize][..n].fill(spec);
                        }
                        None => {
                            for i in 0..n {
                                let (px, w) = (fx + i as f32, c.w[i]);
                                c.regs[Reg::Diffuse as usize][i] = colour(&p.diffuse, px, fy, w);
                                if specular {
                                    c.regs[Reg::Specular as usize][i] =
                                        colour(&p.specular, px, fy, w);
                                }
                            }
                        }
                    }
                    c.regs[Reg::Current as usize] = c.regs[Reg::Diffuse as usize];
                    c.regs[Reg::Temp as usize][..n].fill([0; 4]);
                    c.regs[Reg::Factor as usize][..n].fill(st.values.factor);
                }
                Step::Sample {
                    stage,
                    slot,
                    projected,
                    cube,
                    keyed,
                    bumped,
                } => self.sample(
                    p,
                    (fx, fy),
                    n,
                    c,
                    stage as usize,
                    slot as usize,
                    projected,
                    cube,
                    keyed,
                    bumped,
                ),
                Step::Bump { stage, luminance } => {
                    let m = st.values.bump[stage as usize];
                    for i in 0..n {
                        let t = c.regs[Reg::Texture as usize][i];
                        let du = t[R] as i8 as f32 / 128.0;
                        let dv = t[G] as i8 as f32 / 128.0;
                        c.bump[i] = [m[0] * du + m[2] * dv, m[1] * du + m[3] * dv];
                        c.lum[i] = if luminance {
                            (t[B] as f32 / 255.0 * m[4] + m[5]).clamp(0.0, 1.0)
                        } else {
                            1.0
                        };
                    }
                }
                Step::Combine {
                    op,
                    args,
                    by,
                    colour,
                    alpha,
                    scale,
                    to,
                } => combine(c, n, op, args, by, colour, alpha, scale as i32, to),
                Step::SpecularAdd => {
                    for i in 0..n {
                        let s = c.regs[Reg::Specular as usize][i];
                        let cur = &mut c.regs[Reg::Current as usize][i];
                        for ch in [B, G, R] {
                            cur[ch] = cur[ch].saturating_add(s[ch]);
                        }
                    }
                }
                Step::Fog(fog) => {
                    let v = &st.values;
                    for i in 0..n {
                        let f = match fog {
                            Fog::Vertex => c.regs[Reg::Specular as usize][i][A] as i32,
                            Fog::Table { mode, by_w } => {
                                let d = if by_w {
                                    c.w[i]
                                } else {
                                    p.z.eval(fx + i as f32, fy).clamp(0.0, 1.0)
                                };
                                let f = match mode {
                                    3 => (v.fog_end - d) / (v.fog_end - v.fog_start),
                                    1 => math::exp_neg(v.fog_density * d),
                                    _ => {
                                        let e = v.fog_density * d;
                                        math::exp_neg(e * e)
                                    }
                                };
                                // A factor that is no number fogs all.
                                (f.clamp(0.0, 1.0) * 255.0 + 0.5) as i32
                            }
                        };
                        let cur = &mut c.regs[Reg::Current as usize][i];
                        for ch in [B, G, R] {
                            cur[ch] = combine::fog(cur[ch], v.fog_colour[ch], f);
                        }
                    }
                }
                Step::AlphaTest { func } => {
                    for i in 0..n {
                        let alpha = c.regs[Reg::Current as usize][i][A] as u32;
                        if !combine::compare(func, alpha, st.values.alpha_ref) {
                            c.mask &= !(1 << i);
                        }
                    }
                }
                Step::Dither => {
                    for i in 0..n {
                        let cur = &mut c.regs[Reg::Current as usize][i];
                        *cur = combine::dither(*cur, x + i as i32, y);
                    }
                }
                Step::Write { blend, mask } => {
                    let t = &st.target.colour;
                    let bytes = t.format.bytes();
                    let row = t.offset as usize + y as usize * t.pitch as usize;
                    let plain = blend.is_none() && mask == 0xF;
                    for i in 0..n {
                        if c.mask >> i & 1 == 0 {
                            continue;
                        }
                        let at = row + (x as usize + i) * bytes as usize;
                        let mut src = c.regs[Reg::Current as usize][i];
                        if !plain {
                            let dst = texel::decode(t.format, self.mem.pixel(at, bytes));
                            if let Some(b) = blend {
                                let fs = combine::factor(b.src, src, dst);
                                let fd = combine::factor(b.dst, src, dst);
                                src = combine::blend(b.op, src, dst, fs, fd);
                            }
                            // The mask's bits are red, green, blue,
                            // alpha from bit 0.
                            for (bit, ch) in [R, G, B, A].into_iter().enumerate() {
                                if mask >> bit & 1 == 0 {
                                    src[ch] = dst[ch];
                                }
                            }
                        }
                        self.mem.set_pixel(at, bytes, pack(t.format, src));
                    }
                }
            }
            if c.mask == 0 {
                return;
            }
        }
    }

    /// The depth and stencil tests of the pixels alive, and what they
    /// write.
    #[allow(clippy::too_many_arguments)]
    fn depth(
        &self,
        p: &Prim,
        x: i32,
        y: i32,
        n: usize,
        c: &mut Chunk,
        func: u32,
        write: bool,
        stencil: Option<super::pipeline::Stencil>,
    ) {
        let v = &self.st.values;
        // The lowering has the step only with a depth buffer.
        let Some(d) = self.st.target.depth else {
            return;
        };
        let bytes = d.format.bytes();
        // Where the depth is in a pixel.
        let (shift, z_mask) = if d.format == Format::D16 {
            (0, 0xFFFF)
        } else {
            (8, 0xFFFF_FF00)
        };
        let row = d.offset as usize + y as usize * d.pitch as usize;
        let (fx, fy) = (x as f32 - p.ox, y as f32 - p.oy);
        for i in 0..n {
            if c.mask >> i & 1 == 0 {
                continue;
            }
            let at = row + (x as usize + i) * bytes as usize;
            let z = p.z.eval(fx + i as f32, fy).clamp(0.0, 1.0);
            // A depth that is no number is the nearest.
            let z = if z.is_nan() { 0.0 } else { z };
            let new = (depth_value(d.format, z) << shift).saturating_sub(v.z_bias) & z_mask;
            let old = self.mem.pixel(at, bytes);
            let z_passes = combine::compare(func, new, old & z_mask);
            let mut out = old;
            let mut passes = z_passes;
            if let Some(s) = stencil {
                let value = old & 0xFF;
                let s_passes = combine::compare(
                    s.func,
                    v.stencil_ref & v.stencil_mask,
                    value & v.stencil_mask,
                );
                let op = match (s_passes, z_passes) {
                    (false, _) => s.fail,
                    (true, false) => s.zfail,
                    (true, true) => s.pass,
                };
                let next = combine::stencil_op(op, value, v.stencil_ref);
                out = out & !v.stencil_write | next & v.stencil_write;
                passes &= s_passes;
            }
            if passes && write {
                out = out & !z_mask | new;
            }
            if out != old {
                self.mem.set_pixel(at, bytes, out);
            }
            if !passes {
                c.mask &= !(1 << i);
            }
        }
    }

    /// The stage's texture at each pixel alive, into `Texture`.
    #[allow(clippy::too_many_arguments)]
    fn sample(
        &self,
        p: &Prim,
        (fx, fy): (f32, f32),
        n: usize,
        c: &mut Chunk,
        stage: usize,
        slot: usize,
        projected: Option<u8>,
        cube: bool,
        keyed: bool,
        bumped: Option<bool>,
    ) {
        // The lowering has the step only with a texture.
        let Some(s) = &self.samplers[stage] else {
            return;
        };
        let tex = s.tex;
        let pl = &p.tex[slot];
        // What the coordinates are divided by: 1 / w, or the
        // coordinate a projected texture names.
        let den = match projected {
            Some(k) => &pl[k as usize],
            None => &p.q,
        };
        let (tw, th) = (tex.texture.width as f32, tex.texture.height as f32);
        let needs_lod = tex.needs_lod();
        let aniso = (tex.anisotropy * tex.anisotropy) as f32;
        for i in 0..n {
            if c.mask >> i & 1 == 0 {
                continue;
            }
            let px = fx + i as f32;
            let inv = match projected {
                Some(_) => 1.0 / den.eval(px, fy),
                None => c.w[i],
            };
            let mut u = pl[0].eval(px, fy) * inv;
            let mut v = pl[1].eval(px, fy) * inv;
            // How a coordinate changes from this pixel to the next
            // right and the next down.
            let change = |k: usize, value: f32| {
                (
                    (pl[k].dx - value * den.dx) * inv,
                    (pl[k].dy - value * den.dy) * inv,
                )
            };
            // The square of the texels a pixel covers along x and y.
            let mut face = 0;
            let (rx, ry);
            if cube {
                let r = pl[2].eval(px, fy) * inv;
                if needs_lod {
                    let d = [change(0, u), change(1, v), change(2, r)];
                    let major = u.abs().max(v.abs()).max(r.abs());
                    let scale = tw / (2.0 * major);
                    rx = (d[0].0 * d[0].0 + d[1].0 * d[1].0 + d[2].0 * d[2].0) * scale * scale;
                    ry = (d[0].1 * d[0].1 + d[1].1 * d[1].1 + d[2].1 * d[2].1) * scale * scale;
                } else {
                    (rx, ry) = (0.0, 0.0);
                }
                (face, u, v) = cube_face(u, v, r);
            } else if needs_lod {
                let (du, dv) = (change(0, u), change(1, v));
                rx = du.0 * du.0 * tw * tw + dv.0 * dv.0 * th * th;
                ry = du.1 * du.1 * tw * tw + dv.1 * dv.1 * th * th;
            } else {
                (rx, ry) = (0.0, 0.0);
            }
            let lod = if needs_lod {
                let (major, minor) = (rx.max(ry), rx.min(ry));
                // With anisotropy the level follows the smaller side,
                // as far as the ratio allowed.
                0.5 * math::log2(minor.max(major / aniso))
            } else {
                f32::NEG_INFINITY
            };
            if bumped.is_some() {
                u += c.bump[i][0];
                v += c.bump[i][1];
            }
            if keyed && s.keyed(face, u, v, lod) {
                c.mask &= !(1 << i);
                continue;
            }
            let mut t = s.sample(face, u, v, lod);
            if bumped == Some(true) {
                for ch in [B, G, R] {
                    t[ch] = (t[ch] as f32 * c.lum[i] + 0.5) as u8;
                }
            }
            c.regs[Reg::Texture as usize][i] = t;
        }
    }
}

/// A stage operation on the first `n` fragments.
#[allow(clippy::too_many_arguments)]
fn combine(
    c: &mut Chunk,
    n: usize,
    op: Op,
    args: [Arg; 3],
    by: Option<Arg>,
    colour: bool,
    alpha: bool,
    scale: i32,
    to: Reg,
) {
    let reads = op.reads();
    for i in 0..n {
        let fetch = |arg: Arg| {
            let mut v = c.regs[arg.reg as usize][i];
            if arg.complement {
                v = v.map(|x| 255 - x);
            }
            if arg.replicate {
                v = [v[A]; 4];
            }
            v
        };
        let get = |k: usize| if reads[k] { fetch(args[k]) } else { [0; 4] };
        let (a, b, d) = (get(0), get(1), get(2));
        let t = by.map_or(0, |by| fetch(by)[A] as i32);
        let mut out = c.regs[to as usize][i];
        if op == Op::DotProduct3 {
            let dot = combine::clamp(combine::dot3(a, b) * scale);
            if colour {
                (out[B], out[G], out[R]) = (dot, dot, dot);
            }
            if alpha {
                out[A] = dot;
            }
        } else {
            let ch = |ch: usize| {
                let v = combine::channel(op, a[ch] as i32, b[ch] as i32, d[ch] as i32, t);
                combine::clamp(v * scale)
            };
            if colour {
                (out[B], out[G], out[R]) = (ch(B), ch(G), ch(R));
            }
            if alpha {
                out[A] = ch(A);
            }
        }
        c.regs[to as usize][i] = out;
    }
}
