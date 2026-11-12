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
}

/// One in 255 of a channel, or one step of a 16-bit target, and
/// nothing else.
const CLOSE: Bound = Bound {
    channel: 1,
    outliers: 0,
    depth: 1,
};

/// What a scene is drawn on.
struct Bench {
    gl: Rc<glow::Context>,
    /// Keep 5-6-5 targets in 8 bits a channel.
    wide: std::cell::Cell<bool>,
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
        };
    };
}

impl Bench {
    fn host(&self) -> Box<ToadForceGl> {
        let mut host = ToadForceGl::new(Rc::clone(&self.gl), Glsl::Gl150).unwrap();
        if self.wide.get() {
            host.keep_wide();
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
        let (mut differing, mut largest, mut outliers) = (0, 0, 0);
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
        println!(
            "{name} ({format:?}{}): {differing} pixels differ, largest {largest}, \
             {outliers} over {}; depth {depth_differing} differ, largest {depth_largest}; \
             stencil {stencil_differing} differ; {declined} declined",
            if self.wide.get() { ", 8-bit" } else { "" },
            bound.channel,
        );
        if let Some((x, y, a, b)) = first {
            println!("  first over the bound at ({x}, {y}): software {a:08X}, host {b:08X}");
        }
        assert!(outliers <= bound.outliers, "{name}: colour");
        assert!(depth_largest <= bound.depth, "{name}: depth");
        assert_eq!(stencil_differing, 0, "{name}: stencil");
        declined
    }

    /// `compare` on a 32-bit target with a 16-bit depth buffer, with
    /// nothing declined.
    fn close(&self, name: &str, scene: impl Fn(&mut Rig)) {
        self.within(name, 1, scene);
    }

    /// `close` with channels at most `channel` apart.
    fn within(&self, name: &str, channel: u32, scene: impl Fn(&mut Rig)) {
        let bound = Bound { channel, ..CLOSE };
        let declined = self.compare(name, Format::A8R8G8B8, Some(Format::D16), bound, scene);
        assert_eq!(declined, 0, "{name}");
    }

    /// The bound of a sample of `pattern`, whose neighbours differ by
    /// up to all of a channel's range: with the linear filter (2) the
    /// host rounds each weight where the card cuts it off, and a cube
    /// map's face coordinates are worked out in the host's arithmetic.
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
        (2, 1, 1),
    ] {
        for (bias, first) in [(0.0f32, 0), (1.0, 0), (-1.0, 0), (0.0, 2)] {
            if first > 0 && mag != min {
                continue;
            }
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
    // What the host declines: `MAXMIPLEVEL` with two filters.
    let declined = bench.compare(
        "mip first level, two filters",
        Format::A8R8G8B8,
        None,
        CLOSE,
        |rig| {
            let t = rig.texels(TEXTURE, Format::A8R8G8B8, 32, 6, false, levels);
            rig.texture(0, 1, &t);
            filter(rig, 0, 2, 1, 1);
            rig.stage(0, &[(TSS_MAXMIPLEVEL, 1)]);
            squares(rig);
        },
    );
    assert_eq!(declined, 4);
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
            // Lines are the software renderer's, over what the host
            // drew.
            let line = [
                v(0.0, 20.5, WHITE, 0.0, 0.0),
                v(60.0, 24.5, 0xFF00_FFFF, 0.0, 0.0),
            ];
            let words = words(&line);
            rig.run(|w| encode::draw(w, 2, FVF, 2, &words));
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
