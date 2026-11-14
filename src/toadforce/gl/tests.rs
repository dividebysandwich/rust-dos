//! The OpenGL host against the software renderer: each scene is drawn
//! through the card twice, by the software renderer and through the
//! host with what it drew read back, and the two memories are compared
//! within the bounds of "Where the OpenGL renderer may differ" in
//! `docs/toadforce.md`. Each scene prints the pixels that differ and
//! the largest difference of a channel.
//!
//! They need a desktop OpenGL context:
//! `cargo test --lib toadforce::gl -- --ignored --test-threads=1`.

use super::ToadForceGl;
use crate::toadforce::format::{Format, Rect, Surface};
use crate::toadforce::level1::encode;
use crate::toadforce::level1::states::*;
use crate::toadforce::level1::*;
use crate::toadforce::regs;
use crate::toadforce::{Live, ToadForce};
use crate::video::crt::CrtTiming;
use crate::video::shader::Glsl;
use crate::voodoo::gl::tests::test_context;
use glow::HasContext;
use std::rc::Rc;

const MEMORY: u32 = 32 << 20;
const COLOUR: u32 = 0x10_0000;
const DEPTH: u32 = 0x20_0000;
const TEXTURE: u32 = 0x30_0000;
/// A second render target, which a scene samples as a texture.
const SECOND: u32 = 0x60_0000;
const SIZE: u32 = 64;

/// Position with 1 / w, two colours and two coordinate sets of four
/// numbers: 14 words.
const FVF: u32 = 0x004 | 0x040 | 0x080 | 0x200 | 2 << 16 | 2 << 18;

/// A vertex of layout `FVF`.
#[derive(Clone, Copy)]
struct V {
    x: f32,
    y: f32,
    z: f32,
    rhw: f32,
    diffuse: u32,
    specular: u32,
    /// The first set, and the second, which has the same unless a
    /// scene says otherwise.
    t: [f32; 4],
    t2: Option<[f32; 4]>,
}

fn v(x: f32, y: f32, diffuse: u32, u: f32, t: f32) -> V {
    V {
        x,
        y,
        // A depth no buffer's step is half-way to.
        z: 0.375,
        rhw: 1.0,
        diffuse,
        specular: 0,
        t: [u, t, 0.0, 1.0],
        t2: None,
    }
}

fn words(vertices: &[V]) -> Vec<u32> {
    let mut out = Vec::new();
    for v in vertices {
        out.extend([v.x, v.y, v.z, v.rhw].map(f32::to_bits));
        out.extend([v.diffuse, v.specular]);
        out.extend(v.t.map(f32::to_bits));
        out.extend(v.t2.unwrap_or(v.t).map(f32::to_bits));
    }
    out
}

/// The square from (`x`, `y`) with sides of `size`, as two triangles,
/// with the texture `repeat` times over it. Its corners are a quarter
/// of a pixel off the pixels' centres, so no centre is on an edge and
/// no texel's edge on a centre.
fn quad(x: f32, y: f32, size: f32, diffuse: u32, repeat: f32) -> Vec<V> {
    let (x, y) = (x + 0.25, y + 0.25);
    let corner = |right: bool, down: bool| {
        let (dx, dy) = (right as u8 as f32, down as u8 as f32);
        v(
            x + dx * size,
            y + dy * size,
            diffuse,
            dx * repeat,
            dy * repeat,
        )
    };
    vec![
        corner(false, false),
        corner(true, false),
        corner(false, true),
        corner(true, false),
        corner(true, true),
        corner(false, true),
    ]
}

/// The time of every access of the tests.
pub(super) fn live() -> Live {
    Rig::live()
}

/// A card with a 64x64 render target and a viewport of all of it.
struct Rig {
    card: ToadForce,
    memory: Vec<u8>,
    format: Format,
    depth: Option<Format>,
}

impl Rig {
    fn new(format: Format, depth: Option<Format>) -> Self {
        let mut rig = Rig {
            card: ToadForce::new(MEMORY, 1),
            memory: vec![0; MEMORY as usize],
            format,
            depth,
        };
        rig.card
            .write(regs::LEVEL, 1, &mut rig.memory, Self::live());
        let target = rig.target(COLOUR);
        let viewport = Viewport {
            x: 0,
            y: 0,
            width: SIZE,
            height: SIZE,
            min_z: 0.0,
            max_z: 1.0,
        };
        rig.run(|w| {
            encode::render_target(w, Some(&target));
            encode::viewport(w, &viewport);
        });
        // The triangles of the scenes face either way.
        rig.states(&[(RS_CULLMODE, 1)]);
        rig
    }

    fn live() -> Live {
        Live {
            ns: 0,
            vga: CrtTiming::VGA_400,
        }
    }

    fn surface(&self, offset: u32) -> Surface {
        Surface {
            offset,
            pitch: SIZE * self.format.bytes() + 8,
            width: SIZE,
            height: SIZE,
            format: self.format,
        }
    }

    /// The render target at `offset`, with the rig's depth buffer.
    fn target(&self, offset: u32) -> Target {
        Target {
            colour: self.surface(offset),
            depth: self.depth.map(|format| Depth {
                offset: DEPTH,
                pitch: SIZE * format.bytes(),
                format,
            }),
        }
    }

    /// Run the commands `build` writes, which latch no error.
    fn run(&mut self, build: impl FnOnce(&mut Vec<u32>)) {
        let mut words = Vec::new();
        build(&mut words);
        self.card.submit(&words, &mut self.memory, Self::live());
        assert_eq!(self.card.read(regs::ERROR, Self::live()), 0);
    }

    fn states(&mut self, pairs: &[(u32, u32)]) {
        self.run(|w| encode::render_state(w, pairs));
    }

    fn stage(&mut self, stage: u32, pairs: &[(u32, u32)]) {
        self.run(|w| encode::stage_state(w, stage, pairs));
    }

    /// Draw `vertices` as a list of triangles.
    fn draw(&mut self, vertices: &[V]) {
        let words = words(vertices);
        self.run(|w| encode::draw(w, 4, FVF, vertices.len() as u32, &words));
    }

    fn clear(&mut self, flags: u32, colour: u32, z: f32, stencil: u32, rects: &[Rect]) {
        self.run(|w| encode::clear(w, flags, colour, z, stencil, rects));
    }

    /// Define the texture `t` as `handle` and put it on `stage`, which
    /// takes its colour and alpha.
    fn texture(&mut self, stage: u32, handle: u32, t: &Texture) {
        self.run(|w| {
            encode::define_texture(w, handle, t);
            encode::set_texture(w, stage, handle);
        });
        self.stage(
            stage,
            &[
                (TSS_COLOROP, OP_SELECT_ARG1),
                (TSS_COLORARG1, ARG_TEXTURE),
                (TSS_ALPHAOP, OP_SELECT_ARG1),
                (TSS_ALPHAARG1, ARG_TEXTURE),
            ],
        );
    }

    /// Write the levels of a texture of `format` (and its six faces
    /// with `cube`) from `at` on, each texel from `texel(level, face,
    /// x, y)`, and give its description.
    fn texels(
        &mut self,
        at: u32,
        format: Format,
        size: u32,
        levels: u32,
        cube: bool,
        texel: impl Fn(u32, u32, u32, u32) -> u32,
    ) -> Texture {
        let bytes = format.bytes();
        let mut images = Vec::new();
        let mut offset = at;
        for level in 0..levels {
            let side = (size >> level).max(1);
            for face in 0..if cube { 6 } else { 1 } {
                images.push((offset, side * bytes));
                for y in 0..side {
                    for x in 0..side {
                        let value = texel(level, face, x, y).to_le_bytes();
                        let to = (offset + (y * side + x) * bytes) as usize;
                        self.memory[to..to + bytes as usize]
                            .copy_from_slice(&value[..bytes as usize]);
                    }
                }
                offset += side * side * bytes;
            }
        }
        Texture {
            format,
            width: size,
            height: size,
            levels,
            cube,
            images,
            ..Texture::default()
        }
    }
}

/// A pattern with no two neighbours alike: a colour for each texel of
/// each level and face.
fn pattern(level: u32, face: u32, x: u32, y: u32) -> u32 {
    let n = x
        .wrapping_mul(0x9E37_79B9)
        .wrapping_add(y.wrapping_mul(0x85EB_CA6B))
        .wrapping_add((level * 7 + face).wrapping_mul(0xC2B2_AE35));
    n ^ n >> 15
}

/// A smooth ramp: red across, green down, blue by the level and the
/// face, alpha across and down.
fn ramp(side: u32) -> impl Fn(u32, u32, u32, u32) -> u32 {
    move |level, face, x, y| {
        let side = (side >> level).max(1);
        let (r, g) = (x * 255 / side.max(2), y * 255 / side.max(2));
        let b = 40 * level + 30 * face;
        let a = 255 - (x + y) * 255 / (2 * side);
        a << 24 | r << 16 | g << 8 | b
    }
}

/// How far the host may be from the software renderer.
#[derive(Clone, Copy)]
struct Bound {
    /// The largest difference of a channel of the colour buffer, in
    /// 255ths, or in steps of a 5-6-5 target's fields.
    channel: u32,
    /// The pixels that may differ by more: those a triangle's edge or
    /// a texel's edge runs through.
    outliers: u32,
    /// The largest difference of a depth, in the buffer's steps.
    depth: u32,
    /// The largest mean over the colour buffers of each pixel's largest
    /// difference of a channel, for what the host draws in its own way
    /// (anisotropic filtering), where a pixel says little.
    mean: Option<f64>,
}

/// One in 255 of a channel, or one step of a 16-bit target, and
/// nothing else.
const CLOSE: Bound = Bound {
    channel: 1,
    outliers: 0,
    depth: 1,
    mean: None,
};

/// What a scene is drawn on.
pub(super) struct Bench {
    gl: Rc<glow::Context>,
    /// Keep 5-6-5 targets in 8 bits a channel.
    wide: std::cell::Cell<bool>,
    /// Make hosts as on a GPU without anisotropic filtering, so the
    /// shader samples an anisotropic texture (`shader::native`).
    flat: std::cell::Cell<bool>,
}

/// A bench on a new context. The window that keeps the context stays
/// in the test's own variable, which is dropped after the bench and
/// the hosts made on it.
macro_rules! bench {
    ($bench:ident, $title:literal) => {
        let context = test_context($title);
        let $bench = Bench {
            gl: Rc::new(context.gl),
            wide: std::cell::Cell::new(false),
            flat: std::cell::Cell::new(false),
        };
    };
}

impl Bench {
    pub(super) fn host(&self) -> Box<ToadForceGl> {
        let mut host = ToadForceGl::new(Rc::clone(&self.gl), Glsl::Gl150).unwrap();
        if self.wide.get() {
            host.keep_wide();
        }
        if self.flat.get() {
            host.anisotropy = 0.0;
        }
        Box::new(host)
    }

    /// Draw `scene` in software and through the host on targets of
    /// `format`, and compare card memory within `bound`. Gives the
    /// draws the host declined.
    fn compare(
        &self,
        name: &str,
        format: Format,
        depth: Option<Format>,
        bound: Bound,
        scene: impl Fn(&mut Rig),
    ) -> u64 {
        let mut software = Rig::new(format, depth);
        scene(&mut software);
        let mut hosted = Rig::new(format, depth);
        hosted.card.set_host(Some(self.host()), &mut hosted.memory);
        scene(&mut hosted);
        hosted.card.settle(&mut hosted.memory, None);
        let declined = hosted.card.describe(Rig::live())["host"]["declined_draws"]
            .as_u64()
            .unwrap();

        // The colour buffers, channel by channel.
        let (mut differing, mut largest, mut outliers, mut sum) = (0, 0, 0, 0u64);
        let mut first = None;
        for offset in [COLOUR, SECOND] {
            let surface = software.surface(offset);
            for y in 0..SIZE {
                for x in 0..SIZE {
                    let at = surface.at(x, y);
                    let pixel = |rig: &Rig| {
                        let mut word = [0; 4];
                        let bytes = format.bytes() as usize;
                        word[..bytes].copy_from_slice(&rig.memory[at..at + bytes]);
                        u32::from_le_bytes(word)
                    };
                    let (a, b) = (pixel(&software), pixel(&hosted));
                    if a == b {
                        continue;
                    }
                    let fields: &[(u32, u32)] = match format {
                        Format::R5G6B5 => &[(11, 31), (5, 63), (0, 31)],
                        _ => &[(24, 255), (16, 255), (8, 255), (0, 255)],
                    };
                    let worst = fields
                        .iter()
                        .map(|&(shift, mask)| (a >> shift & mask).abs_diff(b >> shift & mask))
                        .max()
                        .unwrap();
                    differing += 1;
                    sum += worst as u64;
                    largest = largest.max(worst);
                    if worst > bound.channel {
                        outliers += 1;
                        first.get_or_insert((x, y, a, b));
                    }
                }
            }
        }
        // The depth buffer, with the stencil.
        let (mut depth_differing, mut depth_largest, mut stencil_differing) = (0, 0, 0);
        if let Some(depth) = depth {
            let bytes = depth.bytes() as usize;
            for i in 0..(SIZE * SIZE) as usize {
                let at = DEPTH as usize + i * bytes;
                let value = |rig: &Rig| {
                    let mut word = [0; 4];
                    word[..bytes].copy_from_slice(&rig.memory[at..at + bytes]);
                    u32::from_le_bytes(word)
                };
                let (a, b) = (value(&software), value(&hosted));
                let (a, b, sa, sb) = match depth {
                    Format::D16 => (a, b, 0, 0),
                    _ => (a >> 8, b >> 8, a & 0xFF, b & 0xFF),
                };
                if a != b {
                    if a.abs_diff(b) > bound.depth && depth_largest <= bound.depth {
                        println!("  depth at {i}: software {a}, host {b}");
                    }
                    depth_differing += 1;
                    depth_largest = depth_largest.max(a.abs_diff(b));
                }
                stencil_differing += (sa != sb) as u32;
            }
        }
        let mean = sum as f64 / (2 * SIZE * SIZE) as f64;
        println!(
            "{name} ({format:?}{}): {differing} pixels differ, largest {largest}, \
             mean {mean:.3}, {outliers} over {}; depth {depth_differing} differ, \
             largest {depth_largest}; stencil {stencil_differing} differ; {declined} declined",
            if self.wide.get() { ", 8-bit" } else { "" },
            bound.channel,
        );
        if let Some((x, y, a, b)) = first {
            println!("  first over the bound at ({x}, {y}): software {a:08X}, host {b:08X}");
        }
        match bound.mean {
            Some(most) => assert!(mean <= most, "{name}: colour"),
            None => assert!(outliers <= bound.outliers, "{name}: colour"),
        }
        assert!(depth_largest <= bound.depth, "{name}: depth");
        assert_eq!(stencil_differing, 0, "{name}: stencil");
        declined
    }

    /// `compare` on a 32-bit target with a 16-bit depth buffer, with
    /// nothing declined.
    fn close(&self, name: &str, scene: impl Fn(&mut Rig)) {
        self.within(name, 1, scene);
    }

    /// `close` with the same colours. Where no texture is sampled the
    /// card works out the colours of a line's and a point's pixels and
    /// the host fills them in unchanged, so they agree on any GPU.
    fn exact(&self, name: &str, scene: impl Fn(&mut Rig)) {
        self.within(name, 0, scene);
    }

    /// `close` with channels at most `channel` apart.
    fn within(&self, name: &str, channel: u32, scene: impl Fn(&mut Rig)) {
        let bound = Bound { channel, ..CLOSE };
        let declined = self.compare(name, Format::A8R8G8B8, Some(Format::D16), bound, scene);
        assert_eq!(declined, 0, "{name}");
    }

    /// The bound of a sample of `pattern`, whose neighbours differ by
    /// up to all of a channel's range. A cube map is the host's filter
    /// to sample: with the linear filter (2) it rounds each weight
    /// where the card cuts it off, and it works the face's coordinates
    /// out in its own arithmetic. The shader samples the others with
    /// the card's weights, which are one of 256 apart where a
    /// coordinate comes out a little to the other side of a step.
    fn sampled(&self, name: &str, filter: u32, cube: bool, scene: impl Fn(&mut Rig)) {
        let channel = match (filter, cube) {
            (2, true) => 3,
            (2, false) => 2,
            _ => 1,
        };
        self.within(name, channel, scene);
    }
}

const WHITE: u32 = 0xFFFF_FFFF;

#[test]
#[ignore = "requires a desktop OpenGL context"]
fn triangles_without_textures_agree() {
    bench!(bench, "toadforce triangles");
    // SAFETY: the bench's context is current.
    let renderer = unsafe { bench.gl.get_parameter_string(glow::RENDERER) };
    println!("the host's renderer: {renderer}");
    println!(
        "5-6-5 targets in the host's format: {}",
        bench.host().packed()
    );
    let gouraud = |rig: &mut Rig| {
        rig.clear(1, 0xFF10_2030, 1.0, 0, &[]);
        // Whole coordinates: centres on the edges, by the fill rule.
        rig.draw(&[
            v(2.0, 3.0, 0xFFFF_0000, 0.0, 0.0),
            v(60.0, 10.0, 0x8000_FF00, 0.0, 0.0),
            v(8.0, 58.0, 0x4000_00FF, 0.0, 0.0),
        ]);
        // Two triangles that share an edge, with different w.
        let mut pair = quad(20.0, 20.0, 30.0, 0xC0FF_FF00, 1.0);
        pair[1].rhw = 0.25;
        pair[3].rhw = 0.25;
        pair[4].diffuse = 0xFF00_FFFF;
        rig.draw(&pair);
    };
    for format in [Format::A8R8G8B8, Format::X8R8G8B8, Format::R5G6B5] {
        assert_eq!(bench.compare("gouraud", format, None, CLOSE, gouraud), 0);
    }
    // Pixel centres at half coordinates, and flat shading from the
    // first and from the last vertex.
    for last in [0, 1] {
        bench.close("flat", |rig| {
            rig.states(&[
                (RS_SHADEMODE, 1),
                (RS_PIXEL_CENTRE, 1),
                (RS_FLAT_VERTEX, last),
            ]);
            rig.draw(&[
                v(2.5, 3.5, 0xFFFF_0000, 0.0, 0.0),
                v(60.5, 10.5, 0xFF00_FF00, 0.0, 0.0),
                v(8.5, 58.5, 0xFF00_00FF, 0.0, 0.0),
            ]);
            gouraud(rig);
        });
    }
}

/// `quad` at one depth.
fn quad_z(x: f32, y: f32, size: f32, diffuse: u32, z: f32) -> Vec<V> {
    let mut q = quad(x, y, size, diffuse, 1.0);
    q.iter_mut().for_each(|v| v.z = z);
    q
}

/// A square of all the target with a colour at each corner.
fn gradient(rig: &mut Rig, corners: [u32; 4]) {
    let mut q = quad(-1.0, -1.0, 66.0, WHITE, 1.0);
    for v in &mut q {
        let corner = (v.t[0] as usize) + 2 * (v.t[1] as usize);
        v.diffuse = corners[corner];
    }
    rig.draw(&q);
}

/// How far a depth may be off in a buffer of `depth`: a step of 16
/// bits, and of 24 what single precision leaves of them.
fn close(depth: Format) -> Bound {
    Bound {
        depth: if depth == Format::D16 { 1 } else { 4 },
        ..CLOSE
    }
}

#[test]
#[ignore = "requires a desktop OpenGL context"]
fn depth_tests_and_the_bias_agree() {
    bench!(bench, "toadforce depth");
    for depth in [Format::D16, Format::D24S8, Format::D24X8] {
        for func in 1..=8 {
            let name = format!("depth func {func} {depth:?}");
            let scene = |rig: &mut Rig| {
                rig.clear(CLEAR_COLOUR | CLEAR_DEPTH, 0xFF20_2020, 0.75, 0, &[]);
                rig.states(&[(RS_ZENABLE, 1), (RS_ZFUNC, func)]);
                // Nearer than the clear, at its depth, and beyond it.
                rig.draw(&quad_z(2.0, 2.0, 28.0, 0xFFFF_0000, 0.25));
                rig.draw(&quad_z(18.0, 18.0, 28.0, 0xFF00_FF00, 0.75));
                rig.draw(&quad_z(34.0, 4.0, 26.0, 0xFF00_00FF, 0.875));
                // A slope through all of them, which doesn't write.
                rig.states(&[(RS_ZWRITEENABLE, 0)]);
                let mut slope = quad(0.0, 30.0, 62.0, 0xFFFF_FF00, 1.0);
                slope.iter_mut().for_each(|v| v.z = 0.05 + 0.9 * v.t[0]);
                rig.draw(&slope);
                rig.states(&[(RS_ZWRITEENABLE, 1)]);
                slope.iter_mut().for_each(|v| v.y -= 28.0);
                rig.draw(&slope);
            };
            assert_eq!(
                bench.compare(&name, Format::X8R8G8B8, Some(depth), close(depth), scene),
                0
            );
        }
        // The same depth passes "less" only with a bias, which stops
        // at the nearest depth.
        let name = format!("depth bias {depth:?}");
        let scene = |rig: &mut Rig| {
            rig.clear(CLEAR_COLOUR | CLEAR_DEPTH, 0xFF20_2020, 1.0, 0, &[]);
            rig.states(&[(RS_ZENABLE, 1), (RS_ZFUNC, 2)]);
            rig.draw(&quad_z(2.0, 2.0, 50.0, 0xFFFF_0000, 0.625));
            rig.draw(&quad_z(6.0, 6.0, 20.0, 0xFF00_FF00, 0.625));
            for (bias, x) in [(1, 20.0), (5, 30.0), (16, 40.0)] {
                rig.states(&[(RS_ZBIAS, bias)]);
                rig.draw(&quad_z(x, 20.0, 9.0, 0xFF00_00FF + (bias << 12), 0.625));
            }
            rig.draw(&quad_z(4.0, 54.0, 8.0, 0xFFFF_FFFF, 0.0));
            rig.draw(&quad_z(8.0, 56.0, 8.0, 0xFF80_8080, 0.0));
        };
        assert_eq!(
            bench.compare(&name, Format::X8R8G8B8, Some(depth), close(depth), scene),
            0
        );
    }
}

/// The bound of a blend into a target of `format`: one in 255, a step
/// of a 5-6-5 target in the host's format, and a few steps of one the
/// host keeps in 8 bits, whose layers are rounded only when read back.
fn blended(bench: &Bench, format: Format) -> Bound {
    let wide = format == Format::R5G6B5 && !bench.host().packed();
    Bound {
        channel: if wide { 3 } else { 1 },
        ..CLOSE
    }
}

#[test]
#[ignore = "requires a desktop OpenGL context"]
fn blends_and_write_masks_agree() {
    bench!(bench, "toadforce blend");
    let pairs = [
        (5, 6),
        (2, 2),
        (1, 3),
        (9, 1),
        (3, 4),
        (7, 8),
        (10, 9),
        (11, 2),
        (12, 1),
        (13, 1),
        (2, 6),
    ];
    let layers = |rig: &mut Rig| {
        rig.clear(CLEAR_COLOUR, 0x6020_4060, 1.0, 0, &[]);
        gradient(rig, [0xFFFF_0000, 0x8000_FF00, 0x4000_00FF, 0x00FF_FFFF]);
    };
    let over = |rig: &mut Rig| {
        let mut q = quad(6.0, 6.0, 50.0, WHITE, 1.0);
        for v in &mut q {
            let (right, down) = (v.t[0] as u32, v.t[1] as u32);
            v.diffuse =
                (0x30 + 0xC0 * right) << 24 | 0xFF_8020 >> (8 * down) | 0xC0 << (16 * right);
        }
        rig.draw(&q);
    };
    for format in [Format::A8R8G8B8, Format::X8R8G8B8, Format::R5G6B5] {
        let bound = blended(&bench, format);
        for (src, dst) in pairs {
            let name = format!("blend {src} {dst}");
            let scene = |rig: &mut Rig| {
                layers(rig);
                rig.states(&[
                    (RS_ALPHABLENDENABLE, 1),
                    (RS_SRCBLEND, src),
                    (RS_DESTBLEND, dst),
                ]);
                over(rig);
            };
            assert_eq!(bench.compare(&name, format, None, bound, scene), 0);
        }
        for op in 2..=5 {
            let name = format!("blend op {op}");
            let scene = |rig: &mut Rig| {
                layers(rig);
                rig.states(&[
                    (RS_ALPHABLENDENABLE, 1),
                    (RS_SRCBLEND, 5),
                    (RS_DESTBLEND, 2),
                    (RS_BLENDOP, op),
                ]);
                over(rig);
            };
            assert_eq!(bench.compare(&name, format, None, bound, scene), 0);
        }
        // The channels the mask leaves, with a blend and without, and
        // the clip rectangle of a scissor and of a small viewport.
        for mask in [0x1, 0x6, 0x8, 0xB, 0x0] {
            for blend in [0, 1] {
                let name = format!("mask {mask:X} blend {blend}");
                let scene = |rig: &mut Rig| {
                    layers(rig);
                    rig.states(&[(RS_COLORWRITEENABLE, mask), (RS_ALPHABLENDENABLE, blend)]);
                    rig.states(&[(RS_SRCBLEND, 5), (RS_DESTBLEND, 6)]);
                    over(rig);
                    rig.states(&[(RS_COLORWRITEENABLE, 0xF)]);
                    rig.run(|w| encode::scissor(w, true, [10, 12, 30, 20]));
                    over(rig);
                    let viewport = Viewport {
                        x: 20,
                        y: 8,
                        width: 30,
                        height: 40,
                        min_z: 0.0,
                        max_z: 1.0,
                    };
                    rig.run(|w| {
                        encode::scissor(w, false, [0; 4]);
                        encode::viewport(w, &viewport);
                    });
                    rig.states(&[(RS_SRCBLEND, 2), (RS_DESTBLEND, 2)]);
                    over(rig);
                };
                assert_eq!(bench.compare(&name, format, None, bound, scene), 0);
            }
        }
    }
}

/// The states of a filter: 1 for the nearest texel, 2 for linear.
fn filter(rig: &mut Rig, stage: u32, mag: u32, min: u32, mip: u32) {
    rig.stage(
        stage,
        &[
            (TSS_MAGFILTER, mag),
            (TSS_MINFILTER, min),
            (TSS_MIPFILTER, mip),
        ],
    );
}

/// A texture of every format, 8 texels a side, at `TEXTURE`.
fn formats(rig: &mut Rig) -> Vec<Texture> {
    use Format::*;
    let mut out = Vec::new();
    for format in [
        A8, L8, A8L8, R5G6B5, X1R5G5B5, A1R5G5B5, A4R4G4B4, X8R8G8B8, A8R8G8B8, V8U8, L6V5U5,
        X8L8V8U8,
    ] {
        out.push(rig.texels(
            TEXTURE + 0x1000 * out.len() as u32,
            format,
            8,
            1,
            false,
            pattern,
        ));
    }
    // A palette's 256 colours after its texels.
    let mut p8 = rig.texels(TEXTURE + 0x10000, P8, 8, 1, false, pattern);
    for i in 0..256u32 {
        let at = (TEXTURE + 0x11000 + 4 * i) as usize;
        rig.memory[at..at + 4].copy_from_slice(&pattern(3, 0, i, 7).to_le_bytes());
    }
    p8.palette = Some(TEXTURE + 0x11000);
    out.push(p8);
    // Blocks of any bytes are blocks.
    for (i, format) in [DXT1, DXT2, DXT3, DXT4, DXT5].into_iter().enumerate() {
        let at = TEXTURE + 0x20000 + 0x1000 * i as u32;
        for b in 0..64u32 {
            rig.memory[(at + b) as usize] = (pattern(1, i as u32, b, 3) >> 9) as u8;
        }
        out.push(Texture {
            format,
            width: 8,
            height: 8,
            levels: 1,
            images: vec![(at, format.line_bytes(8) as u32)],
            ..Texture::default()
        });
    }
    out
}

#[test]
#[ignore = "requires a desktop OpenGL context"]
fn textures_of_every_format_agree() {
    bench!(bench, "toadforce formats");
    for which in 0..18 {
        for linear in [1, 2] {
            let scene = |rig: &mut Rig| {
                let t = formats(rig).swap_remove(which);
                rig.texture(0, 1, &t);
                filter(rig, 0, linear, linear, 0);
                rig.clear(CLEAR_COLOUR, 0xFF10_1010, 1.0, 0, &[]);
                rig.draw(&quad(4.0, 4.0, 48.0, WHITE, 1.0));
            };
            let format = formats(&mut Rig::new(Format::A8R8G8B8, None))[which].format;
            bench.sampled(&format!("{format:?} filter {linear}"), linear, false, scene);
        }
    }
}

#[test]
#[ignore = "requires a desktop OpenGL context"]
fn address_modes_and_mip_levels_agree() {
    bench!(bench, "toadforce sampling");
    // Coordinates from before the texture to after it, twice over.
    let modes = [(1, 1), (2, 2), (3, 3), (4, 4), (1, 3), (2, 4)];
    for (across, down) in modes {
        for linear in [1, 2] {
            let scene = |rig: &mut Rig| {
                let t = rig.texels(TEXTURE, Format::A8R8G8B8, 8, 1, false, pattern);
                rig.texture(0, 1, &t);
                filter(rig, 0, linear, linear, 0);
                rig.stage(
                    0,
                    &[
                        (TSS_ADDRESSU, across),
                        (TSS_ADDRESSV, down),
                        (TSS_BORDERCOLOR, 0x80FF_8040),
                    ],
                );
                let mut q = quad(2.0, 2.0, 60.0, WHITE, 1.0);
                q.iter_mut().for_each(|v| {
                    v.t = [v.t[0] * 3.75 - 1.25, v.t[1] * 3.75 - 1.5, 0.0, 1.0];
                });
                rig.draw(&q);
            };
            let name = format!("address {across} {down} filter {linear}");
            bench.sampled(&name, linear, false, scene);
        }
    }
    // A texture of 32 with all its levels, a different pattern in
    // each, over squares of 32, 16, 8 and 4 pixels: a pixel covers 1,
    // 2, 4 and 8 texels.
    let squares = |rig: &mut Rig| {
        rig.clear(CLEAR_COLOUR, 0xFF10_1010, 1.0, 0, &[]);
        rig.draw(&quad(0.0, 0.0, 32.0, WHITE, 1.0));
        rig.draw(&quad(34.0, 0.0, 16.0, WHITE, 1.0));
        rig.draw(&quad(34.0, 18.0, 8.0, WHITE, 1.0));
        rig.draw(&quad(44.0, 18.0, 4.0, WHITE, 1.0));
    };
    let levels = |level: u32, _: u32, x: u32, y: u32| {
        // Smooth in a level, so a level off shows and a texel off
        // doesn't.
        ramp(32)(level, 0, x, y) ^ 0x40 << (8 * (level % 3))
    };
    for (mag, min, mip) in [
        (1, 1, 1),
        (2, 2, 1),
        (1, 1, 2),
        (2, 2, 2),
        (2, 1, 0),
        (1, 2, 0),
    ] {
        for (bias, first) in [(0.0f32, 0), (1.0, 0), (-1.0, 0), (0.0, 2)] {
            let scene = |rig: &mut Rig| {
                let t = rig.texels(TEXTURE, Format::A8R8G8B8, 32, 6, false, levels);
                rig.texture(0, 1, &t);
                filter(rig, 0, mag, min, mip);
                rig.stage(
                    0,
                    &[
                        (TSS_MIPMAPLODBIAS, bias.to_bits()),
                        (TSS_MAXMIPLEVEL, first),
                        (TSS_ADDRESSU, 3),
                        (TSS_ADDRESSV, 3),
                    ],
                );
                squares(rig);
            };
            bench.close(
                &format!("mip {mag} {min} {mip} bias {bias} first {first}"),
                scene,
            );
        }
    }
    // `MAXMIPLEVEL` with two filters, and a linear magnification with
    // the nearest texel of a smaller level: the host's own filter
    // changes over elsewhere than the card, and the shader's reads
    // don't.
    for (mag, min, mip) in [(2, 1, 1), (2, 1, 2), (1, 2, 1)] {
        bench.close(&format!("mip {mag} {min} {mip} first 1"), |rig| {
            let t = rig.texels(TEXTURE, Format::A8R8G8B8, 32, 6, false, levels);
            rig.texture(0, 1, &t);
            filter(rig, 0, mag, min, mip);
            rig.stage(0, &[(TSS_MAXMIPLEVEL, 1)]);
            squares(rig);
        });
    }
}

/// A square with its corners on whole coordinates.
fn whole_quad(x: f32, y: f32, size: f32, repeat: f32) -> Vec<V> {
    quad(x - 0.25, y - 0.25, size, WHITE, repeat)
}

#[test]
#[ignore = "requires a desktop OpenGL context"]
fn the_shader_samples_as_the_card_does() {
    bench!(bench, "toadforce shader sampling");
    let exact = Bound {
        channel: 0,
        ..CLOSE
    };
    // A texture drawn a texel to a pixel, and two pixels to a texel,
    // from whole coordinates, as a program draws its pictures: every
    // pixel is on the edge of two texels, and the nearest texel is the
    // card's or its neighbour (`tf_fixed`).
    for (mag, min) in [(1, 1), (2, 2), (2, 1), (1, 2)] {
        for address in [1, 3] {
            let name = format!("a texel to a pixel, filter {mag} {min} address {address}");
            let scene = |rig: &mut Rig| {
                let t = rig.texels(TEXTURE, Format::A8R8G8B8, 32, 1, false, pattern);
                rig.texture(0, 1, &t);
                filter(rig, 0, mag, min, 0);
                rig.stage(0, &[(TSS_ADDRESSU, address), (TSS_ADDRESSV, address)]);
                rig.clear(CLEAR_COLOUR, 0xFF10_1010, 1.0, 0, &[]);
                rig.draw(&whole_quad(0.0, 0.0, 32.0, 1.0));
                rig.draw(&whole_quad(32.0, 0.0, 32.0, 0.5));
                rig.draw(&whole_quad(0.0, 32.0, 16.0, 1.0));
                rig.draw(&whole_quad(16.0, 32.0, 48.0, 3.0));
            };
            let declined = bench.compare(&name, Format::A8R8G8B8, None, exact, scene);
            assert_eq!(declined, 0, "{name}");
        }
    }
    // A square that goes into the distance, where the level of detail
    // changes from pixel to pixel, with each way of taking the levels
    // and a bias. The texture is smooth in a level and another colour
    // in the next, so a level off shows and a weight off doesn't.
    let levels =
        |level: u32, _: u32, x: u32, y: u32| ramp(32)(level, 0, x, y) ^ 0x40 << (8 * (level % 3));
    for (mag, min, mip) in [(2, 2, 2), (2, 2, 1), (1, 1, 1), (2, 1, 2), (1, 2, 1)] {
        for bias in [0.0f32, 0.4] {
            let name = format!("into the distance, filter {mag} {min} {mip} bias {bias}");
            let scene = |rig: &mut Rig| {
                let t = rig.texels(TEXTURE, Format::A8R8G8B8, 32, 6, false, levels);
                rig.texture(0, 1, &t);
                filter(rig, 0, mag, min, mip);
                rig.stage(
                    0,
                    &[
                        (TSS_MIPMAPLODBIAS, bias.to_bits()),
                        (TSS_ADDRESSU, 2),
                        (TSS_ADDRESSV, 2),
                    ],
                );
                rig.clear(CLEAR_COLOUR, 0xFF10_1010, 1.0, 0, &[]);
                let mut q = quad(2.0, 2.0, 60.0, WHITE, 6.0);
                for v in &mut q {
                    if v.t[1] == 0.0 {
                        v.rhw = 0.125;
                    }
                }
                rig.draw(&q);
            };
            let declined = bench.compare(&name, Format::A8R8G8B8, None, CLOSE, scene);
            assert_eq!(declined, 0, "{name}");
        }
    }
    // A texture of 32 by 4 with all six levels, so the last four are a
    // texel high and the last a texel wide too, wrapped going into the
    // distance: each level's own size, and a level of 1 texel.
    for mip in [1, 2] {
        let name = format!("32 by 4 into the distance, mip {mip}");
        let scene = |rig: &mut Rig| {
            let mut images = Vec::new();
            let mut at = TEXTURE;
            for level in 0..6 {
                let (w, h) = ((32u32 >> level).max(1), (4u32 >> level).max(1));
                images.push((at, w * 4));
                for y in 0..h {
                    for x in 0..w {
                        let texel = levels(level, 0, x, y);
                        let to = (at + (y * w + x) * 4) as usize;
                        rig.memory[to..to + 4].copy_from_slice(&texel.to_le_bytes());
                    }
                }
                at += w * h * 4;
            }
            let t = Texture {
                format: Format::A8R8G8B8,
                width: 32,
                height: 4,
                levels: 6,
                images,
                ..Texture::default()
            };
            rig.texture(0, 1, &t);
            filter(rig, 0, 2, 2, mip);
            rig.clear(CLEAR_COLOUR, 0xFF10_1010, 1.0, 0, &[]);
            let mut q = quad(2.0, 2.0, 60.0, WHITE, 6.0);
            for v in &mut q {
                if v.t[1] == 0.0 {
                    v.rhw = 0.125;
                }
            }
            rig.draw(&q);
        };
        let declined = bench.compare(&name, Format::A8R8G8B8, None, CLOSE, scene);
        assert_eq!(declined, 0, "{name}");
    }
    // A filtered texture into a dithered 5-6-5 target, where a sample
    // one of 255 off is a step of the target at the pixels whose
    // dither it tips over.
    for wide in [false, true] {
        if !wide && !bench.host().packed() {
            continue;
        }
        bench.wide.set(wide);
        for dither in [0, 1] {
            let name = format!("565 filtered, dither {dither}");
            let scene = |rig: &mut Rig| {
                let t = rig.texels(TEXTURE, Format::A8R8G8B8, 32, 6, false, ramp(32));
                rig.texture(0, 1, &t);
                filter(rig, 0, 2, 2, 2);
                rig.states(&[(RS_DITHERENABLE, dither)]);
                rig.clear(CLEAR_COLOUR, 0xFF10_1010, 1.0, 0, &[]);
                rig.draw(&whole_quad(0.0, 0.0, 48.0, 1.0));
                rig.draw(&whole_quad(48.0, 0.0, 16.0, 1.0));
                rig.draw(&whole_quad(0.0, 48.0, 64.0, 2.0));
            };
            let declined = bench.compare(&name, Format::R5G6B5, None, exact, scene);
            assert_eq!(declined, 0, "{name}");
        }
    }
    bench.wide.set(false);
}

/// The corners of a square whose coordinates are directions on the
/// face `face` of a cube.
fn cube_quad(face: u32) -> Vec<V> {
    let mut q = quad(4.0, 4.0, 48.0, WHITE, 1.0);
    for v in &mut q {
        let (a, b) = (v.t[0] * 1.6 - 0.8, v.t[1] * 1.6 - 0.8);
        let sign = if face.is_multiple_of(2) { 1.0 } else { -1.0 };
        let d = match face / 2 {
            0 => [sign, a, b],
            1 => [a, sign, b],
            _ => [a, b, sign],
        };
        // A direction's length doesn't matter.
        v.t = [d[0] * 3.0, d[1] * 3.0, d[2] * 3.0, 1.0];
    }
    q
}

#[test]
#[ignore = "requires a desktop OpenGL context"]
fn cube_maps_projection_and_colour_keys_agree() {
    bench!(bench, "toadforce cube");
    for face in 0..6 {
        for linear in [1, 2] {
            bench.sampled(
                &format!("cube face {face} filter {linear}"),
                linear,
                true,
                |rig| {
                    let t = rig.texels(TEXTURE, Format::R5G6B5, 8, 1, true, pattern);
                    rig.texture(0, 1, &t);
                    filter(rig, 0, linear, linear, 0);
                    rig.stage(0, &[(TSS_ADDRESSU, 3), (TSS_ADDRESSV, 3)]);
                    rig.draw(&cube_quad(face));
                },
            );
        }
    }
    // Coordinates divided by their fourth number, which differs from
    // corner to corner.
    for linear in [1, 2] {
        bench.sampled(
            &format!("projected filter {linear}"),
            linear,
            false,
            |rig| {
                let t = rig.texels(TEXTURE, Format::A8R8G8B8, 8, 1, false, pattern);
                rig.texture(0, 1, &t);
                filter(rig, 0, linear, linear, 0);
                rig.stage(0, &[(TSS_TEXTURETRANSFORMFLAGS, TRANSFORM_PROJECTED)]);
                let mut q = quad(4.0, 4.0, 48.0, WHITE, 1.0);
                for v in &mut q {
                    let q = 1.0 + v.t[0] + 2.0 * v.t[1];
                    v.t = [v.t[0] * q, v.t[1] * q, 0.0, q];
                }
                rig.draw(&q);
            },
        );
    }
    // Every other texel has the key's value.
    let keyed = |level: u32, face: u32, x: u32, y: u32| {
        if (x + y).is_multiple_of(2) {
            0xF81F
        } else {
            pattern(level, face, x, y) & 0xF7DF
        }
    };
    for (linear, on) in [(1, 1), (2, 1), (1, 0)] {
        bench.sampled(
            &format!("colour key filter {linear} on {on}"),
            linear,
            false,
            |rig| {
                let mut t = rig.texels(TEXTURE, Format::R5G6B5, 8, 1, false, keyed);
                t.key = Some(0xF81F);
                rig.texture(0, 1, &t);
                filter(rig, 0, linear, linear, 0);
                rig.states(&[(RS_COLORKEYENABLE, on)]);
                rig.clear(CLEAR_COLOUR, 0xFF10_4010, 1.0, 0, &[]);
                rig.draw(&quad(4.0, 4.0, 48.0, WHITE, 1.0));
            },
        );
    }
    // The texture's alpha against a reference, with every function.
    for func in 1..=8 {
        bench.close(&format!("alpha test {func}"), |rig| {
            let t = rig.texels(TEXTURE, Format::A8R8G8B8, 8, 1, false, pattern);
            rig.texture(0, 1, &t);
            rig.states(&[
                (RS_ALPHATESTENABLE, 1),
                (RS_ALPHAFUNC, func),
                (RS_ALPHAREF, 0x80),
            ]);
            rig.states(&[(RS_ZENABLE, 1)]);
            rig.clear(CLEAR_COLOUR | CLEAR_DEPTH, 0xFF10_4010, 1.0, 0, &[]);
            rig.draw(&quad(4.0, 4.0, 48.0, WHITE, 1.0));
        });
    }
}

/// Two textures on stages 0 and 1, the second with the vertex's
/// second coordinate set, a gradient for the vertices' colours, and a
/// texture factor.
fn two_textures(rig: &mut Rig) -> Vec<V> {
    let a = rig.texels(TEXTURE, Format::A8R8G8B8, 8, 1, false, pattern);
    let b = rig.texels(TEXTURE + 0x1000, Format::A4R4G4B4, 4, 1, false, pattern);
    rig.texture(0, 1, &a);
    rig.texture(1, 2, &b);
    rig.stage(1, &[(TSS_TEXCOORDINDEX, 1)]);
    rig.states(&[(RS_TEXTUREFACTOR, 0x80C0_4020)]);
    let mut q = quad(4.0, 4.0, 48.0, WHITE, 1.0);
    for v in &mut q {
        let (right, down) = (v.t[0] as u32, v.t[1] as u32);
        v.diffuse = (0x40 + 0xBF * down) << 24 | 0x40_80FF >> (8 * right) | 0x30 << (16 * down);
        v.specular = 0x2010_4080 << (4 * right);
        v.t2 = Some([v.t[1] * 1.5, v.t[0] * 0.75, 0.0, 1.0]);
    }
    q
}

#[test]
#[ignore = "requires a desktop OpenGL context"]
fn stage_operations_agree() {
    bench!(bench, "toadforce stages");
    let ops = (2..=26).filter(|op| !matches!(op, 17 | 22 | 23));
    for op in ops {
        // Texture and what came before, with the factor as the third
        // argument, at each scale, and with arguments turned over.
        for (scale, flags) in [(1, 0), (2, ARG_COMPLEMENT), (4, ARG_ALPHA_REPLICATE)] {
            bench.close(&format!("op {op} scale {scale} flags {flags:X}"), |rig| {
                let q = two_textures(rig);
                rig.stage(
                    1,
                    &[
                        (TSS_COLOROP, op),
                        (TSS_COLORARG1, ARG_TEXTURE | flags),
                        (TSS_COLORARG2, ARG_CURRENT),
                        (TSS_COLORARG0, ARG_TFACTOR),
                        (TSS_ALPHAOP, op),
                        (TSS_ALPHAARG1, ARG_DIFFUSE),
                        (TSS_ALPHAARG2, ARG_TEXTURE | flags & ARG_COMPLEMENT),
                        (TSS_ALPHAARG0, ARG_TFACTOR),
                        (TSS_COLOR_SCALE, scale),
                        (TSS_ALPHA_SCALE, scale),
                    ],
                );
                rig.draw(&q);
            });
        }
    }
    // Four stages with four textures: the second into the temporary
    // register, the third from it and the specular colour, the fourth
    // by the factor's alpha.
    bench.close("four stages", |rig| {
        let q = two_textures(rig);
        let c = rig.texels(TEXTURE + 0x2000, Format::R5G6B5, 8, 1, false, pattern);
        let d = rig.texels(TEXTURE + 0x3000, Format::A8L8, 4, 1, false, pattern);
        rig.texture(2, 3, &c);
        rig.texture(3, 4, &d);
        rig.stage(
            1,
            &[
                (TSS_COLOROP, 7),
                (TSS_COLORARG2, ARG_DIFFUSE),
                (TSS_RESULTARG, ARG_TEMP),
            ],
        );
        // A coordinate set of its own for each, though the vertex has
        // two.
        rig.stage(
            2,
            &[
                (TSS_TEXTURETRANSFORMFLAGS, 2),
                (TSS_TEXCOORDINDEX, 0),
                (TSS_COLOROP, 25),
                (TSS_COLORARG1, ARG_TEXTURE),
                (TSS_COLORARG2, ARG_TEMP),
                (TSS_COLORARG0, ARG_SPECULAR),
                (TSS_ALPHAOP, 10),
                (TSS_ALPHAARG1, ARG_CURRENT),
                (TSS_ALPHAARG2, ARG_TEXTURE),
            ],
        );
        rig.stage(
            3,
            &[
                (TSS_TEXTURETRANSFORMFLAGS, 2),
                (TSS_TEXCOORDINDEX, 1),
                (TSS_COLOROP, 14),
                (TSS_COLORARG1, ARG_TEXTURE),
                (TSS_COLORARG2, ARG_CURRENT),
                (TSS_ALPHAOP, 7),
                (TSS_ALPHAARG1, ARG_TEXTURE),
                (TSS_ALPHAARG2, ARG_TEMP | ARG_COMPLEMENT),
            ],
        );
        rig.draw(&q);
    });
}

#[test]
#[ignore = "requires a desktop OpenGL context"]
fn bump_mapping_fog_and_the_specular_colour_agree() {
    bench!(bench, "toadforce bump");
    // The offsets of a bump texture move a smooth texture, with its
    // luminance and without.
    for (op, format) in [
        (22, Format::V8U8),
        (22, Format::X8L8V8U8),
        (23, Format::L6V5U5),
        (23, Format::X8L8V8U8),
    ] {
        bench.close(&format!("bump {op} {format:?}"), |rig| {
            let bumps = rig.texels(TEXTURE, format, 8, 1, false, pattern);
            let smooth = rig.texels(TEXTURE + 0x1000, Format::A8R8G8B8, 16, 1, false, ramp(16));
            rig.texture(0, 1, &bumps);
            rig.texture(1, 2, &smooth);
            // The offsets of the nearest texel: between two, the
            // signed bytes are blended as unsigned ones, and the
            // smallest difference of a weight can turn 127 to -128.
            filter(rig, 0, 1, 1, 0);
            filter(rig, 1, 2, 2, 0);
            let matrix = [0.04f32, -0.02, 0.03, 0.05];
            let mut states = vec![(TSS_COLOROP, op)];
            for (i, m) in matrix.iter().enumerate() {
                states.push((TSS_BUMPENVMAT00 + i as u32, m.to_bits()));
            }
            states.push((TSS_BUMPENVLSCALE, 0.75f32.to_bits()));
            states.push((TSS_BUMPENVLOFFSET, 0.125f32.to_bits()));
            rig.stage(0, &states);
            rig.stage(1, &[(TSS_ADDRESSU, 3), (TSS_ADDRESSV, 3)]);
            rig.draw(&quad(4.0, 4.0, 48.0, WHITE, 1.0));
        });
    }
    // Fog by the specular colour's alpha, and by depth and by w in
    // each mode, over one colour.
    let foggy = |rig: &mut Rig| {
        let mut q = quad(2.0, 2.0, 60.0, 0xFF40_C080, 1.0);
        for v in &mut q {
            v.specular = (0x10 + 0xE0 * v.t[0] as u32) << 24;
            v.z = 0.1 + 0.8 * v.t[1];
            v.rhw = 1.0 / (1.0 + 7.0 * v.t[0]);
        }
        rig.states(&[(RS_FOGENABLE, 1), (RS_FOGCOLOR, 0x00A0_2060)]);
        rig.draw(&q);
    };
    bench.close("fog vertex", foggy);
    for mode in 1..=3 {
        for by_w in [0, 1] {
            bench.close(&format!("fog table {mode} by w {by_w}"), |rig| {
                let (start, end, density) = match by_w {
                    0 => (0.2f32, 0.8f32, 2.5f32),
                    _ => (2.0, 7.0, 0.3),
                };
                rig.states(&[
                    (RS_FOGTABLEMODE, mode),
                    (RS_FOG_BY_W, by_w),
                    (RS_FOGSTART, start.to_bits()),
                    (RS_FOGEND, end.to_bits()),
                    (RS_FOGDENSITY, density.to_bits()),
                ]);
                foggy(rig);
            });
        }
    }
    // The specular colour added after the stages.
    bench.close("specular add", |rig| {
        let q = two_textures(rig);
        rig.states(&[(RS_SPECULARENABLE, 1)]);
        rig.draw(&q);
    });
}

#[test]
#[ignore = "requires a desktop OpenGL context"]
fn stencil_operations_agree() {
    bench!(bench, "toadforce stencil");
    for op in 1..=8 {
        let scene = |rig: &mut Rig| {
            let all = CLEAR_COLOUR | CLEAR_DEPTH | CLEAR_STENCIL;
            rig.clear(all, 0xFF10_1010, 1.0, 0xFE, &[]);
            // The stencil alone of a corner, and the depth alone of
            // another.
            let corner = [Rect {
                left: 40,
                top: 0,
                width: 24,
                height: 24,
            }];
            rig.clear(CLEAR_STENCIL, 0, 0.0, 0x01, &corner);
            let other = [Rect {
                left: 0,
                top: 40,
                width: 24,
                height: 24,
            }];
            rig.clear(CLEAR_DEPTH, 0, 0.5, 0, &other);
            rig.states(&[
                (RS_ZENABLE, 1),
                (RS_STENCILENABLE, 1),
                (RS_STENCILFUNC, 8),
                (RS_STENCILPASS, op),
                (RS_STENCILZFAIL, op % 8 + 1),
                (RS_STENCILREF, 0x35),
            ]);
            // What passes, then what fails the depth test behind it.
            rig.draw(&quad_z(2.0, 2.0, 44.0, 0xFFFF_0000, 0.25));
            rig.draw(&quad_z(10.0, 10.0, 50.0, 0xFF00_FF00, 0.75));
            // What fails the stencil test, by a mask, and writes a
            // part of the bits.
            rig.states(&[
                (RS_STENCILFUNC, 3),
                (RS_STENCILMASK, 0x0F),
                (RS_STENCILWRITEMASK, 0x3C),
                (RS_STENCILFAIL, (op + 2) % 8 + 1),
            ]);
            rig.draw(&quad_z(0.0, 20.0, 62.0, 0xFF00_00FF, 0.125));
            // The stencil test without the depth test.
            rig.states(&[(RS_ZENABLE, 0), (RS_STENCILFUNC, 6), (RS_STENCILREF, 0xFF)]);
            rig.draw(&quad_z(20.0, 0.0, 30.0, 0xFFFF_FF00, 0.875));
        };
        let bound = close(Format::D24S8);
        let name = format!("stencil op {op}");
        assert_eq!(
            bench.compare(&name, Format::X8R8G8B8, Some(Format::D24S8), bound, scene),
            0
        );
    }
}

#[test]
#[ignore = "requires a desktop OpenGL context"]
fn sixteen_bit_targets_agree() {
    bench!(bench, "toadforce 565");
    let scene = |rig: &mut Rig, dither: u32, blend: u32| {
        rig.clear(CLEAR_COLOUR, 0xFF20_4060, 1.0, 0, &[]);
        rig.states(&[(RS_DITHERENABLE, dither)]);
        gradient(rig, [0xFFFF_0000, 0x8000_FF00, 0x4000_00FF, 0x00FF_FFFF]);
        let q = two_textures(rig);
        rig.states(&[
            (RS_ALPHABLENDENABLE, blend),
            (RS_SRCBLEND, 5),
            (RS_DESTBLEND, 6),
        ]);
        rig.draw(&q);
        // Layer over layer.
        for i in 0..4 {
            rig.draw(&quad(10.0 + 6.0 * i as f32, 30.0, 30.0, 0x60FF_C040, 1.0));
        }
    };
    // In the host's format where it passes the host's test, and in 8
    // bits a channel as on a host where it doesn't.
    for wide in [false, true] {
        if !wide && !bench.host().packed() {
            println!("the host's 5-6-5 format doesn't pass: only 8 bits a channel compared");
            continue;
        }
        bench.wide.set(wide);
        for dither in [0, 1] {
            for blend in [0, 1] {
                let bound = match blend {
                    0 => CLOSE,
                    _ => blended(&bench, Format::R5G6B5),
                };
                let name = format!("565 dither {dither} blend {blend}");
                let scene = |rig: &mut Rig| scene(rig, dither, blend);
                assert_eq!(
                    bench.compare(&name, Format::R5G6B5, Some(Format::D16), bound, scene),
                    0
                );
            }
        }
    }
}

#[test]
#[ignore = "requires a desktop OpenGL context"]
fn the_host_and_the_processor_take_turns() {
    bench!(bench, "toadforce turns");
    for format in [Format::A8R8G8B8, Format::R5G6B5] {
        let scene = |rig: &mut Rig| {
            // The first clear is the processor's; the host has the
            // target after its first draw.
            rig.clear(CLEAR_COLOUR | CLEAR_DEPTH, 0xFF20_2020, 1.0, 0, &[]);
            rig.states(&[(RS_ZENABLE, 1)]);
            rig.draw(&quad_z(2.0, 2.0, 40.0, 0xFFFF_0000, 0.625));
            // A line in a state the host declines (`SRCALPHASAT` for
            // the destination) is the software renderer's, over what
            // the host drew.
            let line = [
                v(0.0, 20.5, WHITE, 0.0, 0.0),
                v(60.0, 24.5, 0xFF00_FFFF, 0.0, 0.0),
            ];
            let words = words(&line);
            rig.states(&[
                (RS_ALPHABLENDENABLE, 1),
                (RS_SRCBLEND, 2),
                (RS_DESTBLEND, 11),
            ]);
            rig.run(|w| encode::draw(w, 2, FVF, 2, &words));
            rig.states(&[(RS_ALPHABLENDENABLE, 0)]);
            rig.draw(&quad_z(20.0, 12.0, 30.0, 0xFF00_FF00, 0.25));
            // A second target, cleared and drawn by the host, and
            // sampled as a texture.
            let second = rig.target(SECOND);
            rig.run(|w| encode::render_target(w, Some(&second)));
            rig.clear(CLEAR_COLOUR | CLEAR_DEPTH, 0xFF00_0040, 1.0, 0, &[]);
            gradient(rig, [0xFFFF_0000, 0xFF00_FF00, 0xFF00_00FF, 0xFFFF_FFFF]);
            rig.clear(CLEAR_COLOUR | CLEAR_DEPTH, 0xFF40_0000, 0.75, 0, &[]);
            rig.draw(&quad_z(8.0, 8.0, 40.0, 0xFFC0_C040, 0.625));
            let corner = [Rect {
                left: 4,
                top: 40,
                width: 20,
                height: 16,
            }];
            rig.clear(CLEAR_COLOUR, 0xFF12_3456, 0.0, 0, &corner);
            let first = rig.target(COLOUR);
            rig.run(|w| encode::render_target(w, Some(&first)));
            let surface = rig.surface(SECOND);
            let t = Texture {
                format: surface.format,
                width: SIZE,
                height: SIZE,
                levels: 1,
                images: vec![(surface.offset, surface.pitch)],
                ..Texture::default()
            };
            rig.texture(0, 1, &t);
            rig.states(&[(RS_ZENABLE, 0)]);
            rig.draw(&quad(30.0, 30.0, 32.0, WHITE, 1.0));
            // The host's clears of a target it has.
            rig.clear(CLEAR_DEPTH, 0, 0.25, 0, &corner);
            rig.clear(CLEAR_COLOUR, 0x8033_6699, 0.0, 0, &corner);
        };
        let declined = bench.compare("turns", format, Some(Format::D16), CLOSE, scene);
        assert_eq!(declined, 1);
    }
}

/// The processor writes `bytes` to card memory at `at`, as the bus
/// does it: the card notes them where the host has newer pixels.
fn poke(rig: &mut Rig, at: u32, bytes: &[u8]) {
    let len = bytes.len() as u32;
    if rig.card.ahead() {
        rig.card.writing(at, len);
    }
    rig.memory[at as usize..][..bytes.len()].copy_from_slice(bytes);
    rig.card.wrote(at, len);
}

/// A program that locks its back buffer and writes into it between
/// the host's draws, and never reads it: a block of pixels, a row, and
/// single pixels over what the host drew, then depths in a block of
/// the depth buffer, and draws over and beside both.
fn written_between_draws(rig: &mut Rig) {
    let surface = rig.surface(COLOUR);
    let bytes = rig.format.bytes();
    let pixel = |x: u32, y: u32| {
        let p = match bytes {
            2 => (x << 11 | y << 5 | (x ^ y) & 31) & 0xFFFF,
            _ => 0xFF00_0000 | x << 18 | y << 10 | ((x * y) & 0xFF),
        };
        p.to_le_bytes()
    };
    let put = |rig: &mut Rig, x: u32, y: u32, width: u32| {
        let row: Vec<u8> = (x..x + width)
            .flat_map(|x| pixel(x, y)[..bytes as usize].to_vec())
            .collect();
        poke(rig, surface.at(x, y) as u32, &row);
    };
    rig.clear(CLEAR_COLOUR | CLEAR_DEPTH, 0xFF20_2020, 1.0, 0, &[]);
    rig.states(&[(RS_ZENABLE, 1)]);
    rig.draw(&quad_z(2.0, 2.0, 40.0, 0xFFFF_0000, 0.625));
    for y in 8..24 {
        put(rig, 30, y, 20);
    }
    put(rig, 0, 60, SIZE);
    for i in 0..6 {
        put(rig, 3 + 9 * i, 50 + i, 1);
    }
    rig.draw(&quad_z(36.0, 12.0, 20.0, 0xFF00_FF00, 0.5));
    // Depths the host tests the next quad against: near in a block's
    // left half, far in its right.
    for y in 30..46 {
        let row: Vec<u8> = (0..24u16)
            .flat_map(|x| if x < 12 { 0x1000u16 } else { 0xF000 }.to_le_bytes())
            .collect();
        poke(rig, DEPTH + (y * SIZE + 4) * 2, &row);
    }
    rig.draw(&quad_z(0.0, 28.0, 32.0, 0xFF40_80FF, 0.5));
    for y in 40..44 {
        put(rig, 20, y, 30);
    }
}

#[test]
#[ignore = "requires a desktop OpenGL context"]
fn what_the_program_writes_into_a_target_goes_to_the_hosts_copy() {
    bench!(bench, "toadforce written");
    for format in [Format::A8R8G8B8, Format::X8R8G8B8, Format::R5G6B5] {
        for wide in [false, true] {
            if format == Format::R5G6B5 && !wide && !bench.host().packed() {
                println!("the host's 5-6-5 format doesn't pass: only 8 bits a channel compared");
                continue;
            }
            bench.wide.set(wide);
            let name = format!("written {format:?} wide {wide}");
            let scene = written_between_draws;
            let declined = bench.compare(&name, format, Some(Format::D16), CLOSE, scene);
            assert_eq!(declined, 0, "{name}");
            // Nothing came back to card memory on the way, and what
            // the program wrote is what it reads.
            let mut hosted = Rig::new(format, Some(Format::D16));
            hosted.card.set_host(Some(bench.host()), &mut hosted.memory);
            written_between_draws(&mut hosted);
            let status = hosted.card.describe(Rig::live());
            assert_eq!(status["host"]["read_backs"], 0, "{name}: {status}");
            assert!(
                status["host"]["writes"].as_u64() >= Some(2),
                "{name}: {status}"
            );
            // A read of the program's is answered from the host's
            // pixels, fetched once, and is what a read-back brings.
            let surface = hosted.surface(COLOUR);
            let places = [(4, 4), (40, 14), (10, 60), (25, 41), (50, 30)];
            let read = |rig: &Rig, x, y| {
                let at = surface.at(x, y);
                let ahead = rig.card.read_ahead(at as u32, format.bytes(), &rig.memory);
                ahead.unwrap_or_else(|| {
                    let mut word = [0; 4];
                    let bytes = format.bytes() as usize;
                    word[..bytes].copy_from_slice(&rig.memory[at..at + bytes]);
                    u32::from_le_bytes(word)
                })
            };
            let before = places.map(|(x, y)| read(&hosted, x, y));
            let status = hosted.card.describe(Rig::live());
            assert_eq!(status["host"]["read_backs_for"]["lock"], 1, "{name}");
            hosted.card.settle(&mut hosted.memory, None);
            let after = places.map(|(x, y)| read(&hosted, x, y));
            assert_eq!(before, after, "{name}");
        }
    }
}

/// A scene over a picture drawn beforehand, as a game with fixed
/// backgrounds draws each frame: the picture's depths, which the
/// processor wrote into a second depth buffer once, are copied into
/// the target's depth buffer with a `BLT` that names both as a colour
/// format of their size, and the scene is drawn against them.
fn depth_put_back(rig: &mut Rig, frames: u32) {
    let depth = rig.depth.unwrap();
    let named = match depth.bytes() {
        2 => Format::R5G6B5,
        _ => Format::X8R8G8B8,
    };
    let buffer = |offset| Surface {
        offset,
        pitch: SIZE * depth.bytes(),
        width: SIZE,
        height: SIZE,
        format: named,
    };
    let (target, kept) = (buffer(DEPTH), buffer(SECOND));
    for y in 0..SIZE {
        let row: Vec<u8> = (0..SIZE)
            .flat_map(|x| {
                let z: u32 = if (x / 8 + y / 8) % 2 == 0 {
                    0x2000
                } else {
                    0xE000
                };
                let z = if depth.bytes() == 2 { z } else { z << 16 };
                z.to_le_bytes()[..depth.bytes() as usize].to_vec()
            })
            .collect();
        poke(rig, kept.at(0, y) as u32, &row);
    }
    rig.states(&[(RS_ZENABLE, 1)]);
    let all = rect(0, 0, SIZE, SIZE);
    for frame in 0..frames {
        rig.clear(CLEAR_COLOUR | CLEAR_DEPTH, 0xFF20_2020, 1.0, 0, &[]);
        rig.draw(&quad_z(2.0, 2.0, 20.0, 0xFFFF_0000, 0.25));
        rig.run(|w| crate::toadforce::command::encode::blt(w, &target, &all, &kept, &all, None));
        let x = 4.0 + 6.0 * frame as f32;
        rig.draw(&quad_z(x, 8.0, 44.0, 0xFF00_FF00, 0.5));
        // A part of the kept depths alone, over what the quad left.
        let part = rect(40, 40, 16, 16);
        rig.run(|w| crate::toadforce::command::encode::blt(w, &target, &part, &kept, &part, None));
        rig.draw(&quad_z(36.0, 36.0, 24.0, 0xFF40_80FF, 0.75));
    }
}

#[test]
#[ignore = "requires a desktop OpenGL context"]
fn a_depth_buffer_copied_from_another_stays_on_the_host() {
    bench!(bench, "toadforce depth copy");
    let depths = [Format::D16, Format::D24S8, Format::D24X8];
    for depth in depths {
        let name = format!("depth copy {depth:?}");
        let scene = |rig: &mut Rig| depth_put_back(rig, 3);
        let declined = bench.compare(&name, Format::X8R8G8B8, Some(depth), CLOSE, scene);
        assert_eq!(declined, 0, "{name}");
        // The kept depths are copied to the host once for the three
        // frames, and nothing is read back.
        let counts = |frames| {
            let mut hosted = Rig::new(Format::X8R8G8B8, Some(depth));
            hosted.card.set_host(Some(bench.host()), &mut hosted.memory);
            depth_put_back(&mut hosted, frames);
            let status = hosted.card.describe(Rig::live());
            assert_eq!(status["host"]["read_backs"], 0, "{name}: {status}");
            status["host"]["counts"]["surface_uploads"]
                .as_u64()
                .unwrap()
        };
        assert_eq!(counts(3), counts(1), "{name}");
    }
}

fn rect(left: u32, top: u32, width: u32, height: u32) -> Rect {
    Rect {
        left,
        top,
        width,
        height,
    }
}

/// A frame whose surfaces stay on the host from its clear to its end:
/// a clear of a target the host has no copy of, a write of the
/// processor beside a target in a page they share, the faces of a cube
/// map and a level of a mipmapped texture as render targets with the
/// screen's depth buffer, the textures sampled, and fills and copies
/// between the surfaces.
fn on_the_host(rig: &mut Rig) {
    let format = rig.format;
    let cube = rig.texels(TEXTURE, format, 32, 2, true, pattern);
    let mipped = rig.texels(TEXTURE + 0x8_0000, format, 32, 2, false, pattern);
    let screen = rig.target(COLOUR);
    rig.clear(CLEAR_COLOUR | CLEAR_DEPTH, 0xFF20_2020, 1.0, 0, &[]);
    rig.states(&[(RS_ZENABLE, 1)]);
    rig.draw(&quad_z(2.0, 2.0, 60.0, 0xFFFF_0000, 0.625));
    // The byte after the colour buffer's last is in its last page.
    let after = rig.surface(COLOUR).end() as u32;
    assert!(!after.is_multiple_of(crate::toadforce::gpu::PAGE));
    poke(rig, after, &[0x77]);
    rig.draw(&quad_z(30.0, 30.0, 30.0, 0xFF00_00FF, 0.5));

    // The faces, each cleared and drawn into, with the top left part
    // of the screen's depth buffer.
    let image = |texture: &Texture, level: u32, face: u32| {
        let (offset, pitch) = texture.image(level, face);
        let (width, height) = texture.level_size(level);
        Surface {
            offset,
            pitch,
            width,
            height,
            format,
        }
    };
    for face in 0..6 {
        let target = Target {
            colour: image(&cube, 0, face),
            depth: screen.depth,
        };
        rig.run(|w| encode::render_target(w, Some(&target)));
        let colour = 0xFF00_0040 + face * 0x28_1400;
        rig.clear(CLEAR_COLOUR | CLEAR_DEPTH, colour, 1.0, 0, &[]);
        rig.draw(&quad_z(3.0 + face as f32, 5.0, 18.0, 0xFF40_C0FF, 0.25));
    }
    // The second level of the other texture, without a depth buffer.
    let level = Target {
        colour: image(&mipped, 1, 0),
        depth: None,
    };
    rig.run(|w| encode::render_target(w, Some(&level)));
    rig.states(&[(RS_ZENABLE, 0)]);
    rig.clear(CLEAR_COLOUR, 0xFFE0_E020, 1.0, 0, &[rect(0, 0, 16, 16)]);
    rig.draw(&quad(4.0, 4.0, 8.0, 0xFF10_8010, 1.0));

    // The screen again: the depth the faces left is tested against.
    rig.run(|w| encode::render_target(w, Some(&screen)));
    rig.states(&[(RS_ZENABLE, 1)]);
    rig.draw(&quad_z(0.0, 0.0, 40.0, 0xFF00_FF00, 0.375));
    rig.states(&[(RS_ZENABLE, 0)]);
    // The cube map, a face in each square.
    rig.texture(0, 1, &cube);
    filter(rig, 0, 1, 1, 0);
    rig.stage(0, &[(TSS_ADDRESSU, 3), (TSS_ADDRESSV, 3)]);
    for face in 0..6 {
        let mut q = cube_quad(face);
        for v in &mut q {
            v.x = (v.x - 4.0) / 4.0 + 2.0 + 10.0 * face as f32;
            v.y = (v.y - 4.0) / 4.0 + 44.0;
        }
        rig.draw(&q);
    }
    // The level drawn into, alone and under the first level.
    rig.texture(0, 1, &mipped);
    rig.stage(
        0,
        &[(TSS_ADDRESSU, 1), (TSS_ADDRESSV, 1), (TSS_MAXMIPLEVEL, 1)],
    );
    rig.draw(&quad(44.0, 2.0, 16.0, WHITE, 1.0));
    rig.stage(0, &[(TSS_MAXMIPLEVEL, 0)]);
    filter(rig, 0, 1, 1, 1);
    rig.draw(&quad(44.0, 20.0, 8.0, WHITE, 1.0));
    filter(rig, 0, 1, 1, 0);

    // Fills and copies: into the screen, from it to a surface card
    // memory has, within it, and from card memory into it.
    let (colour, second) = (rig.surface(COLOUR), rig.surface(SECOND));
    let face = image(&cube, 0, 2);
    let all = rect(0, 0, SIZE, SIZE);
    rig.run(|w| {
        use crate::toadforce::command::encode::{blt, fill};
        fill(w, &colour, &rect(4, 30, 20, 6), 0x1234_5678);
        blt(w, &second, &all, &colour, &all, None);
        blt(
            w,
            &colour,
            &rect(40, 26, 12, 12),
            &colour,
            &rect(0, 0, 12, 12),
            None,
        );
        blt(
            w,
            &colour,
            &rect(26, 30, 8, 8),
            &face,
            &rect(2, 4, 8, 8),
            None,
        );
        blt(
            w,
            &second,
            &rect(8, 8, 16, 16),
            &face,
            &rect(0, 0, 16, 16),
            None,
        );
    });
    // From card memory through a colour key, which is one of the
    // source's pixels: the screen keeps its own there. The source is
    // the level of the mipped texture that nothing drew into.
    let face = image(&mipped, 0, 0);
    let bytes = face.format.bytes() as usize;
    let mut key = [0u8; 4];
    let at = face.at(3, 5);
    key[..bytes].copy_from_slice(&rig.memory[at..at + bytes]);
    let key = u32::from_le_bytes(key);
    rig.run(|w| {
        use crate::toadforce::command::encode::blt;
        blt(
            w,
            &colour,
            &rect(2, 40, 12, 12),
            &face,
            &rect(0, 0, 12, 12),
            Some(key),
        );
    });
    // A draw into the surface copied to, and a part of it copied back.
    let other = rig.target(SECOND);
    rig.run(|w| encode::render_target(w, Some(&other)));
    rig.run(|w| encode::set_texture(w, 0, 0));
    rig.stage(
        0,
        &[(TSS_COLORARG1, ARG_DIFFUSE), (TSS_ALPHAARG1, ARG_DIFFUSE)],
    );
    rig.draw(&quad(30.0, 4.0, 20.0, 0xFFA0_50F0, 1.0));
    rig.run(|w| encode::render_target(w, Some(&screen)));
    rig.run(|w| {
        use crate::toadforce::command::encode::blt;
        blt(
            w,
            &colour,
            &rect(52, 52, 10, 10),
            &second,
            &rect(28, 2, 10, 10),
            None,
        );
    });
}

#[test]
#[ignore = "requires a desktop OpenGL context"]
fn surfaces_stay_on_the_host_through_a_frame() {
    bench!(bench, "toadforce frame");
    for format in [Format::A8R8G8B8, Format::X8R8G8B8, Format::R5G6B5] {
        for wide in [false, true] {
            if format == Format::R5G6B5 && !wide && !bench.host().packed() {
                println!("the host's 5-6-5 format doesn't pass: only 8 bits a channel compared");
                continue;
            }
            bench.wide.set(wide);
            let name = format!("frame {format:?} wide {wide}");
            let declined = bench.compare(&name, format, Some(Format::D16), CLOSE, on_the_host);
            assert_eq!(declined, 0, "{name}");
            // Nothing of it came back to card memory on the way.
            let mut hosted = Rig::new(format, Some(Format::D16));
            hosted.card.set_host(Some(bench.host()), &mut hosted.memory);
            // The host's test of its 5-6-5 format copied some.
            let before = hosted.card.describe(Rig::live());
            let before = &before["host"]["counts"]["surface_uploads"];
            on_the_host(&mut hosted);
            let status = hosted.card.describe(Rig::live());
            assert_eq!(status["host"]["read_backs"], 0, "{name}: {status}");
            let host = &status["host"]["counts"];
            assert_eq!(&host["surface_uploads"], before, "{name}: {status}");
            assert!(
                host["texture_converts"].as_u64() >= Some(7),
                "{name}: {status}"
            );
        }
    }
}

/// A target the host drew into, copied from into the larger surface it
/// is the top half of, which the host has no copy of yet: the host's
/// copy of the target gives way to one of the larger surface, which
/// the copy then reads from.
fn into_the_whole(rig: &mut Rig) {
    let whole = rig.surface(SECOND);
    let top = Surface {
        height: SIZE / 2,
        ..whole
    };
    let target = Target {
        colour: top,
        depth: None,
    };
    rig.run(|w| encode::render_target(w, Some(&target)));
    rig.clear(CLEAR_COLOUR, 0xFF20_4060, 1.0, 0, &[]);
    rig.draw(&quad(4.0, 2.0, 20.0, 0xFFC0_4020, 1.0));
    rig.run(|w| {
        use crate::toadforce::command::encode::blt;
        blt(
            w,
            &whole,
            &rect(8, 40, 24, 16),
            &top,
            &rect(2, 0, 24, 16),
            None,
        );
    });
}

#[test]
#[ignore = "requires a desktop OpenGL context"]
fn a_copy_into_the_surface_a_target_is_part_of_agrees() {
    bench!(bench, "toadforce copy into the whole");
    let declined = bench.compare(
        "into the whole",
        Format::A8R8G8B8,
        None,
        CLOSE,
        into_the_whole,
    );
    assert_eq!(declined, 0);
    // The copy read from a texture the host has: one deleted with the
    // copy of the target would be an error, whatever pixels a driver
    // still finds in it.
    // SAFETY: the bench's context is current.
    let error = unsafe { bench.gl.get_error() };
    assert_eq!(error, glow::NO_ERROR);
}

/// A texture of 32 with all its levels on stage 0: one smooth picture,
/// each level of it half the size of the one before.
fn mipped(rig: &mut Rig) {
    let levels = |level: u32, _: u32, x: u32, y: u32| ramp(32)(0, 0, x << level, y << level);
    let t = rig.texels(TEXTURE, Format::A8R8G8B8, 32, 6, false, levels);
    rig.texture(0, 1, &t);
}

/// Squares where a pixel covers 1, 2 and 4 texels of a texture of 32,
/// and 1.28, a level of detail between 0 and 0.5: four draws.
fn sizes(rig: &mut Rig) {
    rig.clear(CLEAR_COLOUR, 0xFF10_1010, 1.0, 0, &[]);
    rig.draw(&quad(0.0, 0.0, 32.0, WHITE, 1.0));
    rig.draw(&quad(34.0, 0.0, 16.0, WHITE, 1.0));
    rig.draw(&quad(34.0, 18.0, 8.0, WHITE, 1.0));
    rig.draw(&quad(0.0, 34.0, 25.0, WHITE, 1.0));
}

#[test]
#[ignore = "requires a desktop OpenGL context"]
fn the_states_the_host_declines_are_drawn_in_software() {
    bench!(bench, "toadforce declines");
    let declines = |name: &str, draws: u64, scene: &dyn Fn(&mut Rig)| {
        let declined = bench.compare(name, Format::A8R8G8B8, None, CLOSE, scene);
        assert_eq!(declined, draws, "{name}");
    };
    // Bump offsets on a cube map.
    declines("bump on a cube map", 1, &|rig| {
        let bumps = rig.texels(TEXTURE, Format::V8U8, 8, 1, false, pattern);
        let cube = rig.texels(TEXTURE + 0x1000, Format::R5G6B5, 8, 1, true, pattern);
        rig.texture(0, 1, &bumps);
        rig.texture(1, 2, &cube);
        let matrix = [0.04f32, -0.02, 0.03, 0.05];
        let mut states = vec![(TSS_COLOROP, 22)];
        for (i, m) in matrix.iter().enumerate() {
            states.push((TSS_BUMPENVMAT00 + i as u32, m.to_bits()));
        }
        rig.stage(0, &states);
        rig.draw(&cube_quad(1));
    });
    // `SRCALPHASAT` for the destination.
    declines("SRCALPHASAT for the destination", 1, &|rig| {
        rig.clear(CLEAR_COLOUR, 0x6020_4060, 1.0, 0, &[]);
        rig.states(&[
            (RS_ALPHABLENDENABLE, 1),
            (RS_SRCBLEND, 5),
            (RS_DESTBLEND, 11),
        ]);
        rig.draw(&quad(6.0, 6.0, 50.0, 0x80C0_4020, 1.0));
    });
    // What the host's own filter does another way than the card, and
    // the shader's reads of the texels don't: a linear magnification
    // filter with a nearest minification filter and mip levels, where
    // OpenGL magnifies further than the card.
    for mip in [1, 2] {
        declines(&format!("mag 2 min 1 mip {mip}"), 0, &|rig| {
            mipped(rig);
            filter(rig, 0, 2, 1, mip);
            sizes(rig);
        });
    }
    // A colour key on a texture blended between levels, looked up in
    // the lower of the two.
    declines("colour key between levels", 0, &|rig| {
        // Every other texel of each level has the key's value.
        let keyed = |level: u32, face: u32, x: u32, y: u32| {
            if (x + y + level).is_multiple_of(2) {
                0xF81F
            } else {
                pattern(level, face, x, y) & 0xF7DF
            }
        };
        let mut t = rig.texels(TEXTURE, Format::R5G6B5, 32, 6, false, keyed);
        t.key = Some(0xF81F);
        rig.texture(0, 1, &t);
        filter(rig, 0, 1, 1, 2);
        rig.states(&[(RS_COLORKEYENABLE, 1)]);
        sizes(rig);
    });
    // One texture on two stages, with different addressing and with
    // the same: a stage's states are the shader's numbers.
    for (clamp, draws) in [(3, 0), (1, 0)] {
        declines(
            &format!("one texture twice, address {clamp}"),
            draws,
            &|rig| {
                let t = rig.texels(TEXTURE, Format::A8R8G8B8, 8, 1, false, pattern);
                rig.texture(0, 1, &t);
                rig.texture(1, 2, &t);
                rig.stage(
                    1,
                    &[
                        (TSS_COLOROP, OP_MODULATE),
                        (TSS_COLORARG2, ARG_CURRENT),
                        (TSS_ADDRESSU, clamp),
                    ],
                );
                rig.draw(&quad(4.0, 4.0, 48.0, WHITE, 2.5));
            },
        );
    }
}

#[test]
#[ignore = "requires a desktop OpenGL context"]
fn anisotropy_and_a_linearly_filtered_bump_texture_agree() {
    bench!(bench, "toadforce anisotropy");
    // A square that goes into the distance, with each filter and as
    // much anisotropy as the card takes. The host's GPU filters
    // anisotropically in its own way; the software renderer picks the
    // level by the smaller side. Compared by the mean difference, which
    // was 1.1 to 1.5 on a GeForce RTX 4080: a level off over much of
    // the square would be several times that.
    for anisotropy in [1, 4, 8] {
        let name = format!("anisotropy {anisotropy}");
        let bound = Bound {
            channel: 255,
            mean: Some(2.0),
            ..CLOSE
        };
        let scene = |rig: &mut Rig| {
            mipped(rig);
            filter(rig, 0, 2, 3, 2);
            // Mirrored, so the picture has no edge where it repeats.
            rig.stage(
                0,
                &[
                    (TSS_MAXANISOTROPY, anisotropy),
                    (TSS_ADDRESSU, 2),
                    (TSS_ADDRESSV, 2),
                ],
            );
            rig.clear(CLEAR_COLOUR, 0xFF10_1010, 1.0, 0, &[]);
            let mut q = quad(2.0, 2.0, 60.0, WHITE, 6.0);
            for v in &mut q {
                if v.t[1] == 0.0 {
                    v.rhw = 0.125;
                }
            }
            rig.draw(&q);
        };
        let declined = bench.compare(&name, Format::A8R8G8B8, None, bound, scene);
        assert_eq!(declined, 0, "{name}");
        // A GPU without the filter: the shader takes the card's level
        // by the smaller side, as on any other square going into the
        // distance.
        bench.flat.set(true);
        let flat = format!("{name} without the host's filter");
        let declined = bench.compare(&flat, Format::A8R8G8B8, None, CLOSE, scene);
        assert_eq!(declined, 0, "{flat}");
        bench.flat.set(false);
    }
    // One texture on two stages, anisotropic on one: where the host
    // has the filter, its own filter samples one stage and the shader
    // reads the other, through one set of the host texture's
    // parameters, so the draw is the software renderer's. Without the
    // filter the shader reads both.
    let twice = |rig: &mut Rig| {
        let t = rig.texels(TEXTURE, Format::A8R8G8B8, 32, 6, false, ramp(32));
        rig.texture(0, 1, &t);
        rig.texture(1, 1, &t);
        filter(rig, 0, 2, 3, 2);
        filter(rig, 1, 2, 2, 2);
        rig.stage(0, &[(TSS_MAXANISOTROPY, 8)]);
        rig.stage(1, &[(TSS_COLOROP, OP_MODULATE), (TSS_COLORARG2, ARG_CURRENT)]);
        rig.clear(CLEAR_COLOUR, 0xFF10_1010, 1.0, 0, &[]);
        rig.draw(&quad(4.0, 4.0, 48.0, WHITE, 2.0));
    };
    let anisotropic = bench.host().anisotropy > 0.0;
    let name = "one texture twice, anisotropic once";
    let declined = bench.compare(name, Format::A8R8G8B8, None, CLOSE, twice);
    assert_eq!(declined, anisotropic as u64, "{name}");
    bench.flat.set(true);
    let name = "one texture twice, without the host's filter";
    let declined = bench.compare(name, Format::A8R8G8B8, None, CLOSE, twice);
    assert_eq!(declined, 0, "{name}");
    bench.flat.set(false);
    // Offsets from a bump texture filtered between texels, all of one
    // sign, so the blend of the signed bytes as unsigned ones is the
    // blend of the offsets.
    let smooth_offsets = |_: u32, _: u32, x: u32, y: u32| (x * 6) | (y * 6) << 8 | 0xFF << 16;
    for format in [Format::V8U8, Format::X8L8V8U8] {
        let name = format!("linear bump {format:?}");
        bench.sampled(&name, 2, false, |rig| {
            let bumps = rig.texels(TEXTURE, format, 8, 1, false, smooth_offsets);
            let smooth = rig.texels(TEXTURE + 0x1000, Format::A8R8G8B8, 16, 1, false, ramp(16));
            rig.texture(0, 1, &bumps);
            rig.texture(1, 2, &smooth);
            filter(rig, 0, 2, 2, 0);
            filter(rig, 1, 2, 2, 0);
            let matrix = [0.4f32, -0.2, 0.3, 0.5];
            let mut states = vec![(TSS_COLOROP, 23)];
            for (i, m) in matrix.iter().enumerate() {
                states.push((TSS_BUMPENVMAT00 + i as u32, m.to_bits()));
            }
            states.push((TSS_BUMPENVLSCALE, 0.75f32.to_bits()));
            states.push((TSS_BUMPENVLOFFSET, 0.125f32.to_bits()));
            rig.stage(0, &states);
            rig.stage(1, &[(TSS_ADDRESSU, 3), (TSS_ADDRESSV, 3)]);
            rig.draw(&quad(4.0, 4.0, 48.0, WHITE, 1.0));
        });
    }
}

/// Play the recording the environment's `TOADFORCE_CAPTURE` names
/// (`toadforce::capture`) through both renderers, and print the draws
/// they draw differently (`replay`). It passes: it is a tool for
/// finding out why a program's pictures differ.
#[test]
#[ignore = "requires a desktop OpenGL context and a recording"]
fn replay_a_capture() {
    let Ok(path) = std::env::var("TOADFORCE_CAPTURE") else {
        return;
    };
    let rec = crate::toadforce::capture::Recording::read(std::path::Path::new(&path)).unwrap();
    bench!(bench, "replay");
    if std::env::var("TOADFORCE_CAPTURE_WIDE").is_ok() {
        bench.wide.set(true);
    }
    super::replay::replay(&bench, &rec);
}

/// A target drawn into with an alpha of its own, then sampled through
/// a texture of 8 bits of alpha over its bytes and blended by that
/// alpha: what the host fills the texture with from its copy must be
/// what a read-back and a decode of the bytes give, alpha too. The
/// clear before the draw has every channel masked off for draws, which
/// a `CLEAR` doesn't heed.
fn alpha_of_a_sampled_target(rig: &mut Rig) {
    let surface = rig.surface(SECOND);
    let target = Target {
        colour: surface,
        depth: None,
    };
    rig.run(|w| encode::render_target(w, Some(&target)));
    rig.states(&[(RS_COLORWRITEENABLE, 0)]);
    rig.clear(CLEAR_COLOUR, 0x8020_4060, 1.0, 0, &[]);
    rig.states(&[(RS_COLORWRITEENABLE, 0xF)]);
    rig.draw(&quad(4.0, 4.0, 40.0, 0x40C0_8040, 1.0));
    let texture = Texture {
        format: Format::A8R8G8B8,
        width: SIZE,
        height: SIZE,
        levels: 1,
        images: vec![(surface.offset, surface.pitch)],
        ..Texture::default()
    };
    let screen = rig.target(COLOUR);
    rig.run(|w| encode::render_target(w, Some(&screen)));
    rig.clear(CLEAR_COLOUR, 0xFF10_1010, 1.0, 0, &[]);
    rig.texture(0, 1, &texture);
    filter(rig, 0, 1, 1, 0);
    rig.states(&[
        (RS_ALPHABLENDENABLE, 1),
        (RS_SRCBLEND, 5),
        (RS_DESTBLEND, 6),
    ]);
    rig.draw(&quad(0.0, 0.0, 64.0, WHITE, 1.0));
}

#[test]
#[ignore = "requires a desktop OpenGL context"]
fn a_target_sampled_with_the_alpha_of_its_bytes_agrees() {
    bench!(bench, "toadforce alpha of a sampled target");
    for format in [Format::X8R8G8B8, Format::A8R8G8B8] {
        let name = format!("alpha of a sampled target {format:?}");
        let declined = bench.compare(&name, format, None, CLOSE, alpha_of_a_sampled_target);
        assert_eq!(declined, 0, "{name}");
        // The texture came from the host's copy, with nothing read
        // back.
        let mut hosted = Rig::new(format, None);
        hosted.card.set_host(Some(bench.host()), &mut hosted.memory);
        alpha_of_a_sampled_target(&mut hosted);
        let status = hosted.card.describe(Rig::live());
        assert_eq!(status["host"]["read_backs"], 0, "{name}: {status}");
        let converts = status["host"]["counts"]["texture_converts"].as_u64();
        assert!(converts >= Some(1), "{name}: {status}");
    }
}

/// Draw `vertices` as primitives of the kind `primitive`.
fn draw_as(rig: &mut Rig, primitive: u32, vertices: &[V]) {
    let words = words(vertices);
    rig.run(|w| encode::draw(w, primitive, FVF, vertices.len() as u32, &words));
}

/// The line from (`x0`, `y0`) to (`x1`, `y1`): the colour, the depth,
/// w and the coordinates all change along it.
fn line(x0: f32, y0: f32, x1: f32, y1: f32) -> [V; 2] {
    // Coordinates that put no texel's edge on an end of the line.
    let mut a = v(x0, y0, 0xFFFF_2010, 0.013, 0.021);
    let mut b = v(x1, y1, 0x6020_E0FF, 3.007, 2.011);
    (a.z, b.z) = (0.25, 0.8125);
    b.rhw = 0.25;
    a.specular = 0x2000_0000;
    b.specular = 0xE000_0000;
    [a, b]
}

/// Lines from one point into every octant, with ends on pixel
/// centres, between them and off the target; lines of less than a
/// pixel and of no length; and a strip.
fn lines(rig: &mut Rig) {
    let mut list = Vec::new();
    for (dx, dy) in [
        (20.0, 0.0),
        (20.0, 7.0),
        (20.0, 20.0),
        (7.3, 20.0),
        (0.0, 20.5),
        (-7.0, 20.0),
        (-20.0, 20.0),
        (-20.0, 6.75),
        (-20.5, 0.0),
        (-40.0, -9.0),
        (-20.0, -20.0),
        (-6.5, -40.0),
        (0.0, -20.0),
        (7.0, -19.5),
        (20.0, -20.0),
        (40.0, -7.25),
    ] {
        list.extend(line(31.0, 30.5, 31.0 + dx, 30.5 + dy));
    }
    list.extend(line(0.3, 60.7, 63.6, 62.2));
    // The short ones with one w: a line whose 1 / w falls to a
    // quarter inside a pixel reaches 0 within half a pixel of it,
    // where the host has no change of the coordinates to pick a mip
    // level by ("Where the OpenGL renderer may differ").
    let short = [
        line(3.0, 3.0, 3.0, 3.0),
        line(5.5, 3.5, 5.75, 3.5),
        line(8.25, 3.0, 9.0, 3.5),
    ];
    for mut ends in short {
        ends[1].rhw = 1.0;
        list.extend(ends);
    }
    draw_as(rig, 2, &list);
    let strip = [line(2.0, 50.0, 30.0, 58.0), line(61.0, 44.0, 40.0, 63.0)].concat();
    draw_as(rig, 3, &strip);
}

/// A viewport of a part of the target, or all of it.
fn viewport(rig: &mut Rig, x: u32, y: u32, width: u32, height: u32) {
    let viewport = Viewport {
        x,
        y,
        width,
        height,
        min_z: 0.0,
        max_z: 1.0,
    };
    rig.run(|w| encode::viewport(w, &viewport));
}

#[test]
#[ignore = "requires a desktop OpenGL context"]
fn lines_agree() {
    bench!(bench, "toadforce lines");
    // Which pixels: both places of the pixel centres, with and
    // without the last pixel, shaded and flat from either end.
    for centre in [0, 1] {
        for last in [0, 1] {
            for (shade, flat_last) in [(2, 0), (1, 0), (1, 1)] {
                let name = format!("lines centre {centre} last {last} shade {shade}/{flat_last}");
                bench.exact(&name, |rig| {
                    rig.clear(1, 0xFF10_2030, 1.0, 0, &[]);
                    rig.states(&[
                        (RS_PIXEL_CENTRE, centre),
                        (RS_LASTPIXEL, last),
                        (RS_SHADEMODE, shade),
                        (RS_FLAT_VERTEX, flat_last),
                    ]);
                    lines(rig);
                });
            }
        }
    }
    for format in [Format::X8R8G8B8, Format::R5G6B5] {
        // The host interpolates a square's depth in single precision
        // and rounds it by itself, as a triangle's: 2 steps of 24
        // bits over these lines.
        let bound = Bound {
            channel: 0,
            depth: 2,
            ..CLOSE
        };
        let declined = bench.compare("lines", format, Some(Format::D24S8), bound, |rig| {
            rig.clear(3, 0xFF10_2030, 1.0, 0, &[]);
            rig.states(&[(RS_ZENABLE, 1)]);
            lines(rig);
        });
        assert_eq!(declined, 0);
    }
    // Cut by a viewport and by the scissor rectangle.
    bench.exact("lines in a viewport", |rig| {
        rig.clear(1, 0xFF10_2030, 1.0, 0, &[]);
        viewport(rig, 9, 14, 40, 31);
        lines(rig);
        viewport(rig, 0, 0, SIZE, SIZE);
    });
    bench.exact("lines in a scissor rectangle", |rig| {
        rig.clear(1, 0xFF10_2030, 1.0, 0, &[]);
        rig.run(|w| encode::scissor(w, true, [20, 5, 30, 44]));
        lines(rig);
        rig.run(|w| encode::scissor(w, false, [0; 4]));
    });
    // Behind and in front of a surface, with the depth written.
    bench.exact("lines depth-tested", |rig| {
        rig.clear(3, 0xFF10_2030, 1.0, 0, &[]);
        rig.states(&[(RS_ZENABLE, 1)]);
        rig.draw(&quad_z(0.0, 0.0, 64.0, 0xFF40_4040, 0.51));
        lines(rig);
        // And over the lines, a surface behind some of them.
        rig.draw(&quad_z(10.0, 10.0, 44.0, 0xFF00_8000, 0.4));
    });
    bench.exact("lines blended", |rig| {
        rig.clear(1, 0xFF10_2030, 1.0, 0, &[]);
        rig.draw(&quad(8.0, 8.0, 40.0, 0xFF80_C020, 1.0));
        rig.states(&[
            (RS_ALPHABLENDENABLE, 1),
            (RS_SRCBLEND, 5),
            (RS_DESTBLEND, 6),
        ]);
        lines(rig);
    });
    // The alpha test, by the alpha that changes along a line.
    bench.exact("lines alpha-tested", |rig| {
        rig.clear(1, 0xFF10_2030, 1.0, 0, &[]);
        rig.states(&[
            (RS_ALPHATESTENABLE, 1),
            (RS_ALPHAREF, 0xB1),
            (RS_ALPHAFUNC, 5),
        ]);
        lines(rig);
    });
    // Fog by the specular colour's alpha, by depth and by w.
    bench.exact("lines fog vertex", |rig| {
        rig.states(&[(RS_FOGENABLE, 1), (RS_FOGCOLOR, 0x00A0_2060)]);
        lines(rig);
    });
    for by_w in [0, 1] {
        bench.exact(&format!("lines fog table by w {by_w}"), |rig| {
            let (start, end) = match by_w {
                0 => (0.2f32, 0.9f32),
                _ => (1.0, 4.5),
            };
            rig.states(&[
                (RS_FOGENABLE, 1),
                (RS_FOGCOLOR, 0x00A0_2060),
                (RS_FOGTABLEMODE, 3),
                (RS_FOG_BY_W, by_w),
                (RS_FOGSTART, start.to_bits()),
                (RS_FOGEND, end.to_bits()),
            ]);
            lines(rig);
        });
    }
    // Lines whose 1 / w falls to a quarter in less than a pixel: the
    // host gets the pixel's own values at every corner of its square.
    // No texture here: a mip filter would pick level 0 for such a
    // pixel on the host, and fog by w differs where the centre has no
    // positive 1 / w (#1003).
    bench.exact("lines whose w runs out", |rig| {
        rig.clear(3, 0xFF10_2030, 1.0, 0, &[]);
        rig.states(&[
            (RS_ZENABLE, 1),
            (RS_FOGENABLE, 1),
            (RS_FOGCOLOR, 0x00A0_2060),
        ]);
        let short = [
            line(5.5, 3.5, 5.75, 3.5),
            line(8.25, 3.0, 9.0, 3.5),
            line(20.0, 20.0, 21.5, 20.75),
            line(30.5, 30.5, 30.0, 32.25),
        ]
        .concat();
        draw_as(rig, 2, &short);
    });
    // A line whose last pixel's centre is past its end, where its
    // 1 / w is below 0: the software renderer draws the pixel, so the
    // host has to as well.
    bench.exact("a line whose w runs out past its end", |rig| {
        rig.clear(3, 0xFF10_2030, 1.0, 0, &[]);
        rig.states(&[(RS_ZENABLE, 1), (RS_PIXEL_CENTRE, 0), (RS_LASTPIXEL, 1)]);
        let mut ends = line(40.0, 40.0, 60.0, 49.7);
        ends[1].rhw = 1e-4;
        draw_as(rig, 2, &ends);
    });
    // A depth bias moves a line's depth as a triangle's (`ZBIAS` is
    // every fragment's): lines in the plane of a surface, drawn where
    // only a nearer depth passes, show with a bias and not without.
    for bias in [0, 1, 16] {
        bench.exact(&format!("lines with a depth bias of {bias}"), |rig| {
            rig.clear(3, 0xFF10_2030, 1.0, 0, &[]);
            rig.states(&[(RS_ZENABLE, 1), (RS_ZFUNC, 2)]);
            rig.draw(&quad_z(0.0, 0.0, 64.0, 0xFF40_4040, 0.375));
            rig.states(&[(RS_ZBIAS, bias)]);
            let mut flat = Vec::new();
            for ends in [
                line(4.0, 6.0, 60.0, 6.0),
                line(4.0, 10.0, 60.0, 31.5),
                line(12.25, 4.0, 20.75, 60.0),
            ] {
                flat.extend(ends.map(|mut v| {
                    (v.z, v.rhw) = (0.375, 1.0);
                    v
                }));
            }
            draw_as(rig, 2, &flat);
            // And lines that go through the surface.
            lines(rig);
            rig.states(&[(RS_ZBIAS, 0)]);
        });
    }
    // A texture along the lines, with perspective: the nearest texel,
    // the linear filter, and mip levels by the coordinates' change
    // from a pixel to the next. A sample is a weight of 1/256 off
    // where the host's interpolation puts a coordinate on the other
    // side of a step, which is 1 in 255 of a channel on this texture
    // ("A sample of a 2D texture" in `docs/toadforce.md`).
    for (mag, min, mip) in [(1, 1, 0), (2, 2, 0), (2, 2, 1), (2, 2, 2)] {
        let name = format!("lines textured {mag} {min} {mip}");
        bench.within(&name, 1, |rig| {
            rig.clear(1, 0xFF10_2030, 1.0, 0, &[]);
            mipped(rig);
            filter(rig, 0, mag, min, mip);
            rig.stage(0, &[(TSS_COLOROP, OP_MODULATE), (TSS_ALPHAOP, OP_MODULATE)]);
            lines(rig);
        });
    }
    // Lines going into the distance: w grows 16 times along them, so
    // a pixel covers more and more texels and the mip level changes
    // along the line. Each level of the texture has its own blue.
    for (min, mip) in [(1, 1), (2, 1), (2, 2)] {
        let name = format!("lines into the distance, filter {min} mip {mip}");
        bench.within(&name, 1, |rig| {
            rig.clear(1, 0xFF10_2030, 1.0, 0, &[]);
            let t = rig.texels(TEXTURE, Format::A8R8G8B8, 32, 6, false, ramp(32));
            rig.texture(0, 1, &t);
            filter(rig, 0, 2, min, mip);
            let mut far = Vec::new();
            for (x0, y0, x1, y1) in [
                (2.0, 4.0, 62.0, 9.5),
                (60.5, 14.0, 3.0, 30.0),
                (6.0, 34.0, 20.5, 62.0),
                (40.0, 61.0, 61.0, 36.5),
                (30.0, 62.5, 34.0, 33.0),
            ] {
                let mut ends = line(x0, y0, x1, y1);
                ends[1].rhw = 1.0 / 16.0;
                ends[1].t = [5.37, 3.71, 0.0, 1.0];
                far.extend(ends);
            }
            draw_as(rig, 2, &far);
        });
    }
}

/// `FVF` with a point size after the position.
const FVF_SIZED: u32 = FVF | 0x020;

#[test]
#[ignore = "requires a desktop OpenGL context"]
fn points_agree() {
    bench!(bench, "toadforce points");
    // At pixel centres, on their edges and corners, between them, and
    // partly and wholly off the target.
    let places = [
        (10.0, 12.0),
        (20.5, 12.5),
        (30.25, 12.75),
        (40.5, 12.0),
        (50.0, 12.5),
        (0.0, 40.0),
        (63.5, 63.5),
        (30.0, 40.25),
        (-9.0, 30.0),
        (31.999, 51.001),
    ];
    let points = |rig: &mut Rig| {
        let list: Vec<V> = places
            .iter()
            .enumerate()
            .map(|(i, &(x, y))| {
                let mut p = v(x, y, 0xC010_F020 + 0x15_0011 * i as u32, 0.3, 0.6);
                p.z = 0.1 + 0.08 * i as f32;
                p.rhw = 1.0 / (1.0 + i as f32);
                p
            })
            .collect();
        draw_as(rig, 1, &list);
    };
    for centre in [0, 1] {
        for size in [0.25f32, 1.0, 1.5, 2.0, 3.0, 7.25, 100.0] {
            bench.exact(&format!("points centre {centre} size {size}"), |rig| {
                rig.clear(1, 0xFF10_2030, 1.0, 0, &[]);
                rig.states(&[(RS_PIXEL_CENTRE, centre), (RS_POINTSIZE, size.to_bits())]);
                points(rig);
            });
        }
    }
    // The limits of the size, and a size in each vertex.
    bench.exact("points size limits", |rig| {
        rig.clear(1, 0xFF10_2030, 1.0, 0, &[]);
        rig.states(&[
            (RS_POINTSIZE_MIN, 2.5f32.to_bits()),
            (RS_POINTSIZE_MAX, 6.0f32.to_bits()),
        ]);
        for size in [1.0f32, 4.0, 30.0] {
            rig.states(&[(RS_POINTSIZE, size.to_bits())]);
            points(rig);
        }
    });
    bench.exact("points sized by the vertex", |rig| {
        rig.clear(1, 0xFF10_2030, 1.0, 0, &[]);
        rig.states(&[(RS_POINTSIZE, 9.0f32.to_bits())]);
        let mut list = Vec::new();
        for (i, &(x, y)) in places.iter().enumerate() {
            let mut one = words(&[v(x, y, 0xFFE0_4080, 0.0, 0.0)]);
            one.insert(4, (0.75 * (i + 1) as f32).to_bits());
            list.extend(one);
        }
        rig.run(|w| encode::draw(w, 1, FVF_SIZED, places.len() as u32, &list));
    });
    // Depth-tested, blended and fogged.
    bench.exact("points depth-tested and blended", |rig| {
        rig.clear(3, 0xFF10_2030, 1.0, 0, &[]);
        rig.states(&[(RS_ZENABLE, 1), (RS_POINTSIZE, 9.0f32.to_bits())]);
        rig.draw(&quad_z(0.0, 0.0, 64.0, 0xFF40_4040, 0.45));
        rig.states(&[
            (RS_ALPHABLENDENABLE, 1),
            (RS_SRCBLEND, 5),
            (RS_DESTBLEND, 6),
            (RS_FOGENABLE, 1),
            (RS_FOGCOLOR, 0x00A0_2060),
            (RS_FOGTABLEMODE, 3),
            (RS_FOGSTART, 0.05f32.to_bits()),
            (RS_FOGEND, 0.6f32.to_bits()),
        ]);
        points(rig);
    });
    // A texture: one texel all over a point, and all of the texture
    // over a point sprite.
    for sprite in [0, 1] {
        for filtered in [1, 2] {
            let name = format!("points textured sprite {sprite} filter {filtered}");
            // A weight of 1/256 off, as any sample of a 2D texture.
            bench.within(&name, 1, |rig| {
                rig.clear(1, 0xFF10_2030, 1.0, 0, &[]);
                let t = rig.texels(TEXTURE, Format::A8R8G8B8, 16, 1, false, ramp(16));
                rig.texture(0, 1, &t);
                filter(rig, 0, filtered, filtered, 0);
                rig.stage(0, &[(TSS_COLOROP, OP_MODULATE)]);
                rig.states(&[
                    (RS_POINTSPRITEENABLE, sprite),
                    (RS_POINTSIZE, 12.5f32.to_bits()),
                ]);
                points(rig);
            });
        }
    }
}

#[test]
#[ignore = "requires a desktop OpenGL context"]
fn triangles_as_lines_and_as_points_agree() {
    bench!(bench, "toadforce fill modes");
    for fill in [2, 1] {
        for cull in [1, 2, 3] {
            for shade in [2, 1] {
                let name = format!("fill {fill} cull {cull} shade {shade}");
                // A weight of 1/256 off, as any sample of a 2D
                // texture.
                bench.within(&name, 1, |rig| {
                    rig.clear(3, 0xFF10_2030, 1.0, 0, &[]);
                    let t = rig.texels(TEXTURE, Format::A8R8G8B8, 16, 1, false, ramp(16));
                    rig.texture(0, 1, &t);
                    filter(rig, 0, 2, 2, 0);
                    rig.stage(0, &[(TSS_COLOROP, OP_MODULATE)]);
                    rig.states(&[
                        (RS_ZENABLE, 1),
                        (RS_FILLMODE, fill),
                        (RS_CULLMODE, cull),
                        (RS_SHADEMODE, shade),
                        (RS_POINTSIZE, 3.0f32.to_bits()),
                    ]);
                    // Two triangles that share an edge, facing one
                    // way, with w and the colours changing.
                    let mut pair = quad(6.0, 6.0, 30.0, 0xC0FF_FF00, 1.0);
                    pair[1].rhw = 0.25;
                    pair[3].rhw = 0.25;
                    pair[4].diffuse = 0xFF00_FFFF;
                    rig.draw(&pair);
                    // One facing the other way, partly off the target.
                    rig.draw(&[
                        v(40.0, 30.0, 0xFFFF_0000, 0.0, 0.0),
                        v(44.5, 70.0, 0xFF00_FF00, 0.0, 1.0),
                        v(72.0, 36.0, 0xFF00_00FF, 1.0, 0.0),
                    ]);
                    // A fan and a strip.
                    let fan = [
                        v(20.0, 50.0, 0xFFFF_FFFF, 0.5, 0.5),
                        v(8.0, 44.0, 0xFFFF_0000, 0.0, 0.0),
                        v(30.0, 42.0, 0xFF00_FF00, 1.0, 0.0),
                        v(34.0, 58.0, 0xFF00_00FF, 1.0, 1.0),
                        v(10.0, 60.0, 0xFFFF_FF00, 0.0, 1.0),
                    ];
                    draw_as(rig, 6, &fan);
                    draw_as(rig, 5, &fan);
                });
            }
        }
    }
}

/// Level 0's `FILL` and `BLT` alone, on a surface the host has ahead
/// and between it and card memory: values and pixels with bits their
/// format doesn't define, which a fill writes and a copy keeps
/// (chapter 9 of `docs/toadforce-spec.md`). Nothing here is drawn, so
/// the host has no rounding of its own to add.
fn fills_and_copies(rig: &mut Rig) {
    use crate::toadforce::command::encode::{blt, fill};
    let (colour, second) = (rig.surface(COLOUR), rig.surface(SECOND));
    let all = rect(0, 0, SIZE, SIZE);
    let bytes = rig.format.bytes();
    // The second surface in card memory, every bit of it used.
    for y in 0..SIZE {
        let row: Vec<u8> = (0..SIZE * bytes)
            .map(|i| (i * 37 + y * 101 + (i / bytes) * 59) as u8)
            .collect();
        poke(rig, second.at(0, y) as u32, &row);
    }
    // A clear puts the colour buffer on the host, and a fill of all of
    // it gives every bit a value that is the same either way.
    rig.clear(CLEAR_COLOUR, 0xFF00_0000, 1.0, 0, &[]);
    rig.run(|w| fill(w, &colour, &all, 0x5A12_3456));
    let key = {
        let at = second.at(3, 2);
        let mut word = [0u8; 4];
        word[..bytes as usize].copy_from_slice(&rig.memory[at..at + bytes as usize]);
        u32::from_le_bytes(word)
    };
    rig.run(|w| {
        fill(w, &colour, &rect(4, 4, 20, 6), 0xA5FE_DCBA);
        fill(w, &colour, &rect(10, 8, 30, 30), 0x00FF_00FF);
        fill(w, &colour, &rect(63, 63, 1, 1), 0xFFFF_FFFF);
        // Within the host's surface, apart.
        blt(
            w,
            &colour,
            &rect(44, 44, 16, 16),
            &colour,
            &rect(2, 2, 16, 16),
            None,
        );
        // From card memory into it, and through a key.
        blt(
            w,
            &colour,
            &rect(0, 46, 16, 16),
            &second,
            &rect(8, 8, 16, 16),
            None,
        );
        blt(
            w,
            &colour,
            &rect(20, 44, 12, 12),
            &second,
            &rect(0, 0, 12, 12),
            Some(key),
        );
        // From it into card memory's surface.
        blt(
            w,
            &second,
            &rect(32, 0, 24, 24),
            &colour,
            &rect(0, 0, 24, 24),
            None,
        );
        // Within it over itself, stretched, and keyed from it: the
        // processor's, after a read-back.
        blt(
            w,
            &colour,
            &rect(6, 6, 20, 20),
            &colour,
            &rect(2, 2, 20, 20),
            None,
        );
        blt(
            w,
            &second,
            &rect(0, 32, 32, 32),
            &colour,
            &rect(0, 0, 16, 16),
            None,
        );
        blt(
            w,
            &second,
            &rect(40, 40, 16, 16),
            &colour,
            &rect(0, 0, 16, 16),
            Some(0x5A12_3456),
        );
        // And a fill after them, on whichever side the surface is.
        fill(w, &colour, &rect(30, 2, 8, 3), 0x8001_7FFE);
    });
}

#[test]
#[ignore = "requires a desktop OpenGL context"]
fn fills_and_copies_have_the_bytes_the_processor_writes() {
    bench!(bench, "toadforce fills and copies");
    let exact = Bound {
        channel: 0,
        outliers: 0,
        ..CLOSE
    };
    for format in [Format::A8R8G8B8, Format::X8R8G8B8, Format::R5G6B5] {
        for wide in [false, true] {
            if format == Format::R5G6B5 && !wide && !bench.host().packed() {
                println!("the host's 5-6-5 format doesn't pass: only 8 bits a channel compared");
                continue;
            }
            bench.wide.set(wide);
            let name = format!("fills and copies {format:?} wide {wide}");
            bench.compare(&name, format, None, exact, fills_and_copies);
        }
    }
}
