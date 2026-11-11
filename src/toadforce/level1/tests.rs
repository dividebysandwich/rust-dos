//! Level 1's commands on the card: what they set, what they reject, and
//! what `CLEAR` writes.

use super::encode;
use super::states::*;
use super::*;
use crate::toadforce::command::{self, header};
use crate::toadforce::regs;
use crate::toadforce::{Live, ToadForce};
use crate::video::crt::CrtTiming;

const MEMORY: u32 = 32 << 20;

struct Rig {
    card: ToadForce,
    memory: Vec<u8>,
}

impl Rig {
    /// A card at level 1.
    fn new() -> Self {
        let mut rig = Rig {
            card: ToadForce::new(MEMORY, 1),
            memory: vec![0; MEMORY as usize],
        };
        rig.write(regs::LEVEL, 1);
        assert_eq!(rig.read(regs::LEVEL), 1);
        rig
    }

    fn live() -> Live {
        Live {
            ns: 0,
            vga: CrtTiming::VGA_400,
        }
    }

    fn read(&self, offset: u32) -> u32 {
        self.card.read(offset, Self::live())
    }

    fn write(&mut self, offset: u32, value: u32) {
        self.card
            .write(offset, value, &mut self.memory, Self::live());
    }

    /// Run `words`, and give the error they latched (0 for none),
    /// cleared for the next run.
    fn run(&mut self, words: &[u32]) -> u32 {
        self.card.submit(words, &mut self.memory, Self::live());
        let error = self.read(regs::ERROR);
        self.write(regs::ERROR, 0);
        error
    }

    fn word(&self, at: u32) -> u32 {
        let at = at as usize;
        u32::from_le_bytes(self.memory[at..at + 4].try_into().unwrap())
    }

    fn half(&self, at: u32) -> u32 {
        let at = at as usize;
        u16::from_le_bytes(self.memory[at..at + 2].try_into().unwrap()) as u32
    }
}

fn words(build: impl FnOnce(&mut Vec<u32>)) -> Vec<u32> {
    let mut out = Vec::new();
    build(&mut out);
    out
}

fn target(format: Format, depth: Option<Format>) -> Target {
    Target {
        colour: Surface {
            offset: 0x10_0000,
            pitch: 64 * format.bytes(),
            width: 64,
            height: 32,
            format,
        },
        depth: depth.map(|format| Depth {
            offset: 0x20_0000,
            pitch: 64 * format.bytes(),
            format,
        }),
    }
}

fn texture(format: Format, levels: u32) -> Texture {
    let mut at = 0x40_0000;
    let images = (0..levels)
        .map(|level| {
            let (w, h) = ((64u32 >> level).max(1), (32u32 >> level).max(1));
            let pitch = format.line_bytes(w) as u32;
            let image = (at, pitch);
            at += format.image_bytes(w, h, pitch) as u32;
            image
        })
        .collect();
    Texture {
        format,
        width: 64,
        height: 32,
        levels,
        cube: false,
        palette: (format == Format::P8).then_some(0x30_0000),
        key: None,
        images,
    }
}

const BAD_VALUE: u32 = regs::Error::BadValue as u32;
const BAD_RANGE: u32 = regs::Error::BadRange as u32;
const BAD_STATE: u32 = regs::Error::BadState as u32;
const BAD_HANDLE: u32 = regs::Error::BadHandle as u32;
const BAD_LENGTH: u32 = regs::Error::BadLength as u32;
const BAD_OPCODE: u32 = regs::Error::BadOpcode as u32;

#[test]
fn level_0_has_none_of_the_commands() {
    let mut rig = Rig::new();
    let state = words(|w| encode::render_state(w, &[(RS_ZENABLE, 1)]));
    assert_eq!(rig.run(&state), 0);
    rig.write(regs::LEVEL, 0);
    assert_eq!(rig.run(&state), BAD_OPCODE);
    // The capability table is the level's.
    assert_eq!(rig.read(regs::CAPS), 5);
    rig.write(regs::LEVEL, 1);
    assert_eq!(rig.read(regs::CAPS), 20);
    assert_eq!(rig.read(regs::CAPS + 4 * 6), 2048);
    assert_eq!(rig.read(regs::CAPS + 4), 4096);
}

#[test]
fn the_states_start_at_their_reset_values_and_a_level_write_brings_them_back() {
    let mut rig = Rig::new();
    let fresh = Level1::default();
    assert_eq!(rig.card.level1, fresh);
    assert_eq!(fresh.state(RS_ZFUNC), 4);
    assert_eq!(fresh.state_f(RS_FOGEND), 1.0);
    assert_eq!(fresh.stage(0, TSS_COLOROP), OP_MODULATE);
    assert_eq!(fresh.stage(1, TSS_COLOROP), OP_DISABLE);
    assert_eq!(fresh.stage(5, TSS_TEXCOORDINDEX), 5);
    assert_eq!(fresh.transforms[TRANSFORM_VIEW as usize][5], 1.0);

    let set = words(|w| {
        encode::render_state(w, &[(RS_ZENABLE, 1), (RS_FOGEND, 80.0f32.to_bits())]);
        encode::stage_state(w, 3, &[(TSS_COLOROP, OP_LERP), (TSS_COLOR_SCALE, 4)]);
        encode::set_texture(w, 0, 0);
    });
    assert_eq!(rig.run(&set), 0);
    let state = &rig.card.level1;
    assert_eq!(state.state(RS_ZENABLE), 1);
    assert_eq!(state.state_f(RS_FOGEND), 80.0);
    assert_eq!(state.stage(3, TSS_COLOROP), OP_LERP);
    assert_eq!(state.stage(3, TSS_COLOR_SCALE), 4);
    assert_eq!(state.stage(2, TSS_COLOROP), OP_DISABLE);

    rig.write(regs::LEVEL, 1);
    assert_eq!(rig.card.level1, fresh);
}

#[test]
fn a_bad_pair_sets_none_of_its_commands_states() {
    let mut rig = Rig::new();
    for pairs in [
        [(RS_ZENABLE, 1), (RS_ZFUNC, 9)],
        [(RS_ZENABLE, 1), (10, 0)],
        [(RS_ZENABLE, 1), (RS_FOGSTART, f32::NAN.to_bits())],
        [(RS_ZENABLE, 1), (RENDER_STATES as u32, 0)],
    ] {
        assert_eq!(
            rig.run(&words(|w| encode::render_state(w, &pairs))),
            BAD_VALUE
        );
        assert_eq!(rig.card.level1.state(RS_ZENABLE), 0, "{pairs:?}");
    }
    let stage = words(|w| encode::stage_state(w, 8, &[(TSS_COLOROP, OP_MODULATE)]));
    assert_eq!(rig.run(&stage), BAD_VALUE);
    let op = words(|w| encode::stage_state(w, 0, &[(TSS_ALPHAOP, OP_PREMODULATE)]));
    assert_eq!(rig.run(&op), BAD_VALUE);
    // A pair cut in half is a bad length, and so is a list of none.
    assert_eq!(
        rig.run(&[header(SET_RENDER_STATE, 4), RS_ZENABLE, 1, RS_ZFUNC]),
        BAD_LENGTH
    );
    assert_eq!(rig.run(&[header(SET_RENDER_STATE, 1)]), BAD_LENGTH);
    assert_eq!(rig.card.level1, Level1::default());
}

#[test]
fn transforms_lights_and_planes_are_kept_as_sent() {
    let mut rig = Rig::new();
    let matrix: [f32; 16] = std::array::from_fn(|i| i as f32 + 0.5);
    let light = Light {
        kind: LIGHT_SPOT,
        diffuse: [1.0, 0.5, 0.25, 1.0],
        specular: [0.1, 0.2, 0.3, 0.4],
        ambient: [0.0; 4],
        position: [1.0, 2.0, 3.0],
        direction: [0.0, 0.0, 1.0],
        range: 100.0,
        falloff: 1.0,
        attenuation: [1.0, 0.0, 0.5],
        theta: 0.5,
        phi: 1.0,
        spot_model: 1,
    };
    let material = Material {
        diffuse: [0.1, 0.2, 0.3, 0.4],
        ambient: [0.5; 4],
        specular: [1.0; 4],
        emissive: [0.0; 4],
        power: 20.0,
    };
    let viewport = Viewport {
        x: 8,
        y: 4,
        width: 640,
        height: 480,
        min_z: 0.0,
        max_z: 1.0,
    };
    let set = words(|w| {
        encode::transform(w, TRANSFORM_TEXTURE + 7, &matrix);
        encode::light(w, 7, &light);
        encode::enable_light(w, 7, true);
        encode::material(w, &material);
        encode::clip_plane(w, 5, &[0.0, 1.0, 0.0, -2.0]);
        encode::viewport(w, &viewport);
        encode::scissor(w, true, [1, 2, 3, 4]);
    });
    assert_eq!(rig.run(&set), 0);
    let state = &rig.card.level1;
    assert_eq!(state.transforms[13], matrix);
    assert_eq!(state.transforms[0][0], 1.0);
    assert_eq!(state.lights[7], light);
    assert_eq!(state.lights_on, 1 << 7);
    assert_eq!(state.material, material);
    assert_eq!(state.clip_planes[5], [0.0, 1.0, 0.0, -2.0]);
    assert_eq!(state.viewport, viewport);
    assert_eq!((state.scissor_on, state.scissor), (true, [1, 2, 3, 4]));

    // What they reject.
    let run = |rig: &mut Rig, build: &dyn Fn(&mut Vec<u32>)| rig.run(&words(build));
    assert_eq!(
        run(&mut rig, &|w| encode::transform(w, 14, &matrix)),
        BAD_VALUE
    );
    assert_eq!(run(&mut rig, &|w| encode::light(w, 8, &light)), BAD_VALUE);
    let unkind = Light { kind: 4, ..light };
    assert_eq!(run(&mut rig, &|w| encode::light(w, 0, &unkind)), BAD_VALUE);
    let far = Light {
        range: f32::INFINITY,
        ..light
    };
    assert_eq!(run(&mut rig, &|w| encode::light(w, 0, &far)), BAD_VALUE);
    // A light never set can't be turned on, and can be turned off.
    assert_eq!(
        run(&mut rig, &|w| encode::enable_light(w, 2, true)),
        BAD_STATE
    );
    assert_eq!(run(&mut rig, &|w| encode::enable_light(w, 2, false)), 0);
    assert_eq!(run(&mut rig, &|w| encode::enable_light(w, 7, false)), 0);
    assert_eq!(rig.card.level1.lights_on, 0);
    let deep = Viewport {
        max_z: 1.5,
        ..viewport
    };
    assert_eq!(run(&mut rig, &|w| encode::viewport(w, &deep)), BAD_VALUE);
    let wide = Viewport {
        x: 2000,
        width: 100,
        ..viewport
    };
    assert_eq!(run(&mut rig, &|w| encode::viewport(w, &wide)), BAD_VALUE);
    let inverted = Viewport {
        min_z: 0.75,
        max_z: 0.25,
        ..viewport
    };
    assert_eq!(
        run(&mut rig, &|w| encode::viewport(w, &inverted)),
        BAD_VALUE
    );
    assert_eq!(rig.card.level1.viewport, viewport);
    // A scissor rectangle is held to the largest target, as a viewport.
    for (enable, rect) in [
        (2, [0, 0, 1, 1]),
        (1, [2000, 0, 100, 1]),
        (1, [0, u32::MAX, 0, 2]),
    ] {
        let words = [
            header(SET_SCISSOR, 6),
            enable,
            rect[0],
            rect[1],
            rect[2],
            rect[3],
        ];
        assert_eq!(rig.run(&words), BAD_VALUE, "{enable} {rect:?}");
    }
    assert_eq!(rig.card.level1.scissor, [1, 2, 3, 4]);
}

#[test]
fn clear_writes_each_buffer_its_flag_names() {
    let mut rig = Rig::new();
    let t = target(Format::R5G6B5, Some(Format::D24S8));
    let (colour, depth) = (t.colour, t.depth_surface().unwrap());
    // No target yet.
    let all = CLEAR_COLOUR | CLEAR_DEPTH | CLEAR_STENCIL;
    let clear_all = words(|w| encode::clear(w, all, 0xFF80_FF40, 0.5, 0x5A, &[]));
    assert_eq!(rig.run(&clear_all), BAD_STATE);
    assert_eq!(rig.run(&words(|w| encode::render_target(w, Some(&t)))), 0);
    assert_eq!(rig.card.level1.target, Some(t));

    assert_eq!(rig.run(&clear_all), 0);
    // 80h, FFh and 40h in 5, 6 and 5 bits.
    let pixel = 0x10 << 11 | 0x3F << 5 | 0x08;
    assert_eq!(rig.half(colour.at(0, 0) as u32), pixel);
    assert_eq!(rig.half(colour.at(63, 31) as u32), pixel);
    assert_eq!(rig.half(colour.at(63, 31) as u32 + 2), 0);
    // Half of FFFFFFh, rounded, above the stencil's byte.
    let z = 0x80_0000 << 8 | 0x5A;
    assert_eq!(rig.word(depth.at(0, 0) as u32), z);
    assert_eq!(rig.word(depth.at(63, 31) as u32), z);

    // The stencil alone, in a rectangle: the depth stays.
    let rect = Rect {
        left: 2,
        top: 1,
        width: 4,
        height: 2,
    };
    let stencil = words(|w| encode::clear(w, CLEAR_STENCIL, 0, 0.0, 0x33, &[rect]));
    assert_eq!(rig.run(&stencil), 0);
    assert_eq!(rig.word(depth.at(2, 1) as u32), 0x80_0000 << 8 | 0x33);
    assert_eq!(rig.word(depth.at(5, 2) as u32), 0x80_0000 << 8 | 0x33);
    assert_eq!(rig.word(depth.at(6, 2) as u32), z);
    assert_eq!(rig.word(depth.at(1, 1) as u32), z);
    // The depth alone: the stencil stays.
    let deep = words(|w| encode::clear(w, CLEAR_DEPTH, 0, 1.0, 0, &[rect]));
    assert_eq!(rig.run(&deep), 0);
    assert_eq!(rig.word(depth.at(2, 1) as u32), 0xFF_FFFF << 8 | 0x33);
    assert_eq!(rig.half(colour.at(2, 1) as u32), pixel);

    // What it rejects.
    let out = Rect { left: 61, ..rect };
    let outside = words(|w| encode::clear(w, CLEAR_COLOUR, 0, 0.0, 0, &[rect, out]));
    assert_eq!(rig.run(&outside), BAD_RANGE);
    let empty = Rect { width: 0, ..rect };
    let none = words(|w| encode::clear(w, CLEAR_COLOUR, 0, 0.0, 0, &[empty]));
    assert_eq!(rig.run(&none), BAD_RANGE);
    for (flags, z, stencil) in [(0, 0.0, 0), (8, 0.0, 0), (all, 1.25, 0), (all, 0.0, 256)] {
        let bad = words(|w| encode::clear(w, flags, 0, z, stencil, &[]));
        assert_eq!(rig.run(&bad), BAD_VALUE, "{flags} {z} {stencil}");
    }
    // A 16-bit depth buffer has no stencil, and no buffer no depth.
    let t16 = target(Format::X8R8G8B8, Some(Format::D16));
    rig.run(&words(|w| encode::render_target(w, Some(&t16))));
    assert_eq!(rig.run(&stencil), BAD_STATE);
    assert_eq!(rig.run(&deep), 0);
    assert_eq!(rig.half(t16.depth.unwrap().offset + 2 * (64 + 2)), 0xFFFF);
    let colour_only = words(|w| encode::clear(w, CLEAR_COLOUR, 0x1234_5678, 0.0, 0, &[]));
    assert_eq!(rig.run(&colour_only), 0);
    assert_eq!(rig.word(t16.colour.offset), 0x0034_5678);
    // A depth clear of D24X8 writes 0 below the depth, and it has no
    // stencil either.
    let t24 = target(Format::R5G6B5, Some(Format::D24X8));
    let d24 = t24.depth_surface().unwrap();
    rig.memory[d24.at(2, 1)..d24.at(6, 1)].fill(0xA5);
    rig.run(&words(|w| encode::render_target(w, Some(&t24))));
    assert_eq!(rig.run(&deep), 0);
    assert_eq!(rig.word(d24.at(2, 1) as u32), 0xFF_FFFF << 8);
    assert_eq!(rig.word(d24.at(5, 2) as u32), 0xFF_FFFF << 8);
    assert_eq!(rig.run(&stencil), BAD_STATE);
    let bare = target(Format::A8R8G8B8, None);
    rig.run(&words(|w| encode::render_target(w, Some(&bare))));
    assert_eq!(rig.run(&deep), BAD_STATE);
    // Ranges come before the state: a rectangle outside the target is
    // a bad range even when the target has no depth buffer to clear.
    let deep_out = words(|w| encode::clear(w, CLEAR_DEPTH, 0, 1.0, 0, &[out]));
    assert_eq!(rig.run(&deep_out), BAD_RANGE);
    assert_eq!(rig.run(&colour_only), 0);
    assert_eq!(rig.word(bare.colour.offset), 0x1234_5678);
    rig.run(&words(|w| encode::render_target(w, None)));
    assert_eq!(rig.card.level1.target, None);
    assert_eq!(rig.run(&colour_only), BAD_STATE);
}

#[test]
fn a_render_target_is_checked_before_it_is_taken() {
    let mut rig = Rig::new();
    let good = target(Format::R5G6B5, Some(Format::D16));
    let set = |rig: &mut Rig, t: &Target| rig.run(&words(|w| encode::render_target(w, Some(t))));
    assert_eq!(set(&mut rig, &good), 0);
    let mut t = good;
    t.colour.format = Format::P8;
    assert_eq!(set(&mut rig, &t), BAD_VALUE);
    let mut t = good;
    t.colour.width = 4096;
    t.colour.pitch = 8192;
    assert_eq!(set(&mut rig, &t), BAD_VALUE);
    let mut t = good;
    t.depth.as_mut().unwrap().format = Format::R5G6B5;
    assert_eq!(set(&mut rig, &t), BAD_VALUE);
    let mut t = good;
    t.depth.as_mut().unwrap().pitch = 127;
    assert_eq!(set(&mut rig, &t), BAD_VALUE);
    let mut t = good;
    t.depth.as_mut().unwrap().offset = MEMORY - 64;
    assert_eq!(set(&mut rig, &t), BAD_RANGE);
    let mut t = good;
    t.colour.offset = MEMORY - 64;
    assert_eq!(set(&mut rig, &t), BAD_RANGE);
    assert_eq!(rig.card.level1.target, Some(good));
}

fn free_now(rig: &mut Rig, handle: u32) -> u32 {
    rig.run(&words(|w| encode::free_texture(w, handle)))
}

#[test]
fn textures_are_defined_bound_and_freed_by_handle() {
    let mut rig = Rig::new();
    let mips = texture(Format::DXT1, 7);
    let paletted = texture(Format::P8, 1);
    let define = |rig: &mut Rig, handle: u32, t: &Texture| {
        rig.run(&words(|w| encode::define_texture(w, handle, t)))
    };
    assert_eq!(define(&mut rig, 1, &mips), 0);
    assert_eq!(define(&mut rig, 4095, &paletted), 0);
    let keyed = Texture {
        key: Some(0xF81F),
        ..texture(Format::R5G6B5, 2)
    };
    assert_eq!(define(&mut rig, 7, &keyed), 0);
    assert_eq!(rig.card.level1.textures[&7], keyed);
    assert_eq!(free_now(&mut rig, 7), 0);
    assert_eq!(rig.card.level1.textures[&1], mips);
    assert_eq!(mips.level_size(6), (1, 1));
    // A row of 16 blocks, then 8 rows of them.
    assert_eq!(mips.images[0].1, 16 * 8);
    assert_eq!(mips.images[1].0 - mips.images[0].0, 8 * 16 * 8);

    let bind = |rig: &mut Rig, stage: u32, handle: u32| {
        rig.run(&words(|w| encode::set_texture(w, stage, handle)))
    };
    assert_eq!(bind(&mut rig, 0, 1), 0);
    assert_eq!(bind(&mut rig, 7, 1), 0);
    assert_eq!(bind(&mut rig, 3, 4095), 0);
    assert_eq!(rig.card.level1.texture(7), Some(&mips));
    assert_eq!(rig.card.level1.texture(1), None);
    assert_eq!(bind(&mut rig, 1, 2), BAD_HANDLE);
    assert_eq!(bind(&mut rig, 8, 1), BAD_VALUE);
    assert_eq!(bind(&mut rig, 1, 4096), BAD_VALUE);

    // A handle defined again is the new texture, where it is bound too.
    assert_eq!(define(&mut rig, 1, &paletted), 0);
    assert_eq!(rig.card.level1.texture(0), Some(&paletted));
    // Freed, it leaves its stages.
    let free = |rig: &mut Rig, handle: u32| rig.run(&words(|w| encode::free_texture(w, handle)));
    assert_eq!(free(&mut rig, 1), 0);
    assert_eq!(rig.card.level1.bound, [0, 0, 0, 4095, 0, 0, 0, 0]);
    assert_eq!(free(&mut rig, 1), BAD_HANDLE);
    assert_eq!(free(&mut rig, 0), BAD_VALUE);
    assert_eq!(bind(&mut rig, 3, 0), 0);
    assert_eq!(rig.card.level1.bound, [0; 8]);
}

#[test]
fn a_texture_the_card_cant_take_is_rejected() {
    let mut rig = Rig::new();
    let good = texture(Format::A8R8G8B8, 3);
    let define = |rig: &mut Rig, t: &Texture| rig.run(&words(|w| encode::define_texture(w, 9, t)));
    let change = |edit: &dyn Fn(&mut Texture)| {
        let mut t = good.clone();
        edit(&mut t);
        t
    };
    for (what, t) in [
        ("a depth format", change(&|t| t.format = Format::D16)),
        ("no power of two", change(&|t| t.width = 48)),
        ("too large", change(&|t| t.width = 4096)),
        ("a palette without P8", change(&|t| t.palette = Some(0))),
        ("a short pitch", change(&|t| t.images[1].1 = 127)),
        ("a cube that isn't square", change(&|t| t.cube = true)),
    ] {
        assert_eq!(define(&mut rig, &t), BAD_VALUE, "{what}");
    }
    // More levels than the size has, none, and images that don't match.
    let mut over = texture(Format::A8R8G8B8, 7);
    over.levels = 8;
    over.images.push((0, 4));
    assert_eq!(define(&mut rig, &over), BAD_VALUE);
    let mut short = good.clone();
    short.levels = 2;
    assert_eq!(define(&mut rig, &short), BAD_VALUE);
    let mut levelless = texture(Format::A8R8G8B8, 1);
    levelless.levels = 0;
    assert_eq!(define(&mut rig, &levelless), BAD_VALUE);
    let unpaletted = Texture {
        palette: None,
        ..texture(Format::P8, 1)
    };
    assert_eq!(define(&mut rig, &unpaletted), BAD_VALUE);
    assert_eq!(
        rig.run(&words(|w| encode::define_texture(w, 0, &good))),
        BAD_VALUE
    );
    // A colour key is a pixel value of the format, and blocks have
    // none.
    let wide_key = change(&|t| {
        t.format = Format::R5G6B5;
        t.key = Some(0x1_0000);
    });
    assert_eq!(define(&mut rig, &wide_key), BAD_VALUE);
    let mut blocks = texture(Format::DXT3, 1);
    blocks.key = Some(0);
    assert_eq!(define(&mut rig, &blocks), BAD_VALUE);
    // Images and a palette past card memory.
    let past = change(&|t| t.images[2].0 = MEMORY - 16 * 4 * 8 + 4);
    assert_eq!(define(&mut rig, &past), BAD_RANGE);
    let palette = Texture {
        palette: Some(MEMORY - 1020),
        ..texture(Format::P8, 1)
    };
    assert_eq!(define(&mut rig, &palette), BAD_RANGE);
    assert!(rig.card.level1.textures.is_empty());

    // A cube map has six faces a level.
    let mut cube = texture(Format::R5G6B5, 1);
    cube.height = 64;
    cube.cube = true;
    cube.images = (0..6)
        .map(|face| (0x40_0000 + face * 0x2000, 128))
        .collect();
    assert_eq!(define(&mut rig, &cube), 0);
    assert_eq!(rig.card.level1.textures[&9].image(0, 5), (0x40_A000, 128));
}

#[test]
fn the_state_is_saved_with_the_card() {
    let mut rig = Rig::new();
    let t = target(Format::X8R8G8B8, Some(Format::D24X8));
    let set = words(|w| {
        encode::render_target(w, Some(&t));
        encode::render_state(w, &[(RS_ALPHAREF, 77), (RS_PIXEL_CENTRE, 1)]);
        encode::stage_state(w, 6, &[(TSS_RESULTARG, ARG_TEMP)]);
        encode::define_texture(w, 12, &texture(Format::L8, 2));
        encode::set_texture(w, 2, 12);
        encode::transform(w, TRANSFORM_PROJECTION, &[2.0; 16]);
    });
    assert_eq!(rig.run(&set), 0);

    use crate::savestate::{Reader, State, Writer};
    let mut w = Writer::new();
    rig.card.save(&mut w);
    let mut loaded = ToadForce::new(MEMORY, 1);
    loaded.load(&mut Reader::new(&w.buf)).unwrap();
    assert_eq!(loaded.level1, rig.card.level1);
    assert_eq!(loaded.level(), 1);
    assert_eq!(loaded.level1.bound[2], 12);
    assert_eq!(loaded.level1.target, Some(t));
}

#[test]
fn the_decoder_knows_every_command_the_level_lists() {
    let listed = [
        (SET_RENDER_STATE, 3),
        (SET_STAGE_STATE, 4),
        (SET_TRANSFORM, 18),
        (SET_VIEWPORT, 7),
        (SET_SCISSOR, 6),
        (SET_MATERIAL, 18),
        (SET_LIGHT, 29),
        (ENABLE_LIGHT, 3),
        (SET_CLIP_PLANE, 6),
        (SET_RENDER_TARGET, 9),
        (CLEAR, 5),
        (DEFINE_TEXTURE, 11),
        (FREE_TEXTURE, 2),
        (SET_TEXTURE, 3),
    ];
    for opcode in 0x0010..0x0100u16 {
        let least = listed.iter().find(|l| l.0 == opcode).map(|l| l.1);
        assert_eq!(lengths(opcode).map(|l| l.0), least, "{opcode:#X}");
        assert_eq!(command::lengths(0, opcode), None, "{opcode:#X} at level 0");
        // Zeroes of the least length decode or are rejected, and don't
        // panic.
        if let Some(least) = least {
            let mut zeroes = vec![0; least];
            zeroes[0] = header(opcode, least);
            let _ = command::decode(&zeroes, 1, MEMORY);
        }
    }
}
