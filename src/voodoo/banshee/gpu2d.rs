//! The commands of a Banshee's or Voodoo3's 2D engine that the OpenGL
//! renderer draws, where bytes they touch are newer there than in the
//! card's memory: rectangle fills and screen to screen copies whose
//! pixels are one value or the source's (`blit2d::Plan`). A fill of one
//! of the 3D buffers, or a copy between colour buffers, goes into the
//! renderer's pictures of them at its resolution, and the rows it draws
//! stay newer on the GPU (`gpu_draw_buffer`). One into other memory is
//! a 2D operation in the renderer's copy of the memory (`gpu_blit`),
//! from a colour buffer's picture or from that memory. The engine draws
//! everything else in the card's memory, after the bytes it touches are
//! read back (`banshee_2d_write`).

use super::blit2d::{Op, Plan, Surface};
use crate::voodoo::blit::rgb888;
use crate::voodoo::mirror::{
    Blit, BlitSource, BufferCopy, Command, Convert, Fill, LayoutKey, Plane,
};
use crate::voodoo::raster::Stride;
use crate::voodoo::{NONE, Voodoo};

/// Where a surface of the engine's lies among the 3D buffers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Place {
    /// A colour buffer, by its word offset (as `Draw`'s `dest`).
    Colour(u32),
    /// The auxiliary buffer.
    Aux,
    /// Elsewhere: in no buffer, or in a buffer's bytes otherwise than as
    /// its pixels.
    Elsewhere,
}

/// Where pixels `rect` (left, top, right, bottom) of `surface` lie: in
/// one of `key`'s buffers if the surface is that buffer's pixels, 16 bits
/// each from its first byte, its rows as the buffer's, and the rectangle
/// is within the buffer.
fn place(key: &LayoutKey, surface: &Surface, [_, _, right, bottom]: [u32; 4]) -> Place {
    let same = |base: u32, stride: Stride| {
        surface.base == base
            && surface.bpp == 2
            && surface.tiled == stride.tiled
            && surface.stride as usize == stride.words * 2
    };
    let colour = key.color.iter().filter(|&&offs| offs != NONE);
    let colour = colour.map(|&offs| (offs, key.stride, Place::Colour(offs / 2)));
    let aux = (key.aux != NONE).then_some((key.aux, key.aux_stride, Place::Aux));
    let within = right <= key.width && bottom <= key.height;
    colour
        .chain(aux)
        .find(|&(base, stride, _)| same(base, stride))
        .filter(|_| within)
        .map_or(Place::Elsewhere, |(_, _, place)| place)
}

fn plane(surface: &Surface) -> Plane {
    Plane {
        start: surface.base,
        stride: surface.stride,
        bpp: surface.bpp,
        tiled: surface.tiled,
    }
}

impl Voodoo {
    /// Draw on the GPU the command that writing `data` to 2D register
    /// `reg` runs, whose footprint is `spans`, if bytes there are newer on
    /// the GPU and it can be drawn there exactly. True if it was: the
    /// engine then stands where the command leaves it. False leaves the
    /// write to the caller, with nothing changed.
    ///
    /// Where the program reads or writes those bytes (they are hot), the
    /// command is left to the CPU: drawn there it costs less than on the
    /// GPU with a read back after it.
    pub(super) fn banshee_2d_gpu(
        &mut self,
        reg: u32,
        data: u32,
        spans: &[Option<(u32, u32)>; 2],
    ) -> bool {
        let tracker = &self.gpu.get_mut().tracker;
        let touched = || spans.iter().flatten().copied();
        let newer = touched().any(|(a, b)| tracker.buffers_in(a, b) || tracker.gpu_in(a, b));
        if !newer || touched().any(|(a, b)| tracker.hot_in(a, b)) {
            return false;
        }
        let size = self.fbi.ram.bytes() as u32;
        let plan = self
            .banshee
            .as_ref()
            .and_then(|b| b.engine.plan(reg, data, size));
        let (Some(plan), Some(key)) = (plan, self.mirror_layout()) else {
            return false;
        };
        if !self.gpu_draw_plan(&key, &plan) {
            return false;
        }
        if let Some(b) = &mut self.banshee {
            b.engine.skip(reg, data);
        }
        self.gpu.get_mut().engine += 1;
        self.display_dirty = true;
        true
    }

    /// Draw `plan` on the GPU, in the layout `key` the recording has.
    fn gpu_draw_plan(&mut self, key: &LayoutKey, plan: &Plan) -> bool {
        let [left, top, right, bottom] = plan.rect;
        let (width, height) = (right - left, bottom - top);
        // The source's pixels, as a rectangle like the destination's.
        let src = match plan.op {
            Op::Copy { src, from: [x, y] } => Some((src, [x, y, x + width, y + height])),
            Op::Fill(_) => None,
        };
        let from = src.map(|(surface, rect)| place(key, &surface, rect));
        // A `Fill`'s and a `BufferCopy`'s rectangle.
        let rect = [left, right, top, bottom];
        match (place(key, &plan.dst, plan.rect), plan.op) {
            (Place::Colour(dest), Op::Fill(value)) => {
                let fill = Fill {
                    dest,
                    rect,
                    color: Some(rgb888(value as u16)),
                    aux: None,
                    alpha_planes: false,
                    pattern: None,
                };
                self.gpu_draw_buffer(Command::Fill(fill), Some(dest), (top, bottom))
            }
            (Place::Aux, Op::Fill(value)) => {
                // The renderer clears its depth buffer through a colour
                // buffer's picture, which has it.
                let Some(&colour) = key.color.iter().find(|&&offs| offs != NONE) else {
                    return false;
                };
                let fill = Fill {
                    dest: colour / 2,
                    rect,
                    color: None,
                    aux: Some(value as u16),
                    alpha_planes: false,
                    pattern: None,
                };
                self.gpu_draw_buffer(Command::Fill(fill), None, (top, bottom))
            }
            (Place::Colour(to), Op::Copy { from: at, .. }) => {
                let Some(Place::Colour(from)) = from else {
                    return false;
                };
                let copy = BufferCopy {
                    from,
                    to,
                    src: at,
                    rect,
                };
                self.gpu_draw_buffer(Command::Copy(copy), Some(to), (top, bottom))
            }
            (Place::Aux, Op::Copy { .. }) => false,
            (Place::Elsewhere, op) => {
                let i = |v: u32| v as i32;
                let area = [left, top, right, bottom].map(i);
                let (source, convert) = match (op, src, from) {
                    (Op::Fill(value), _, _) => (BlitSource::Fill(value), Convert::None),
                    // The renderer's picture is 8 bits a channel.
                    (_, Some((_, rect)), Some(Place::Colour(buffer))) => {
                        let rect = rect.map(i);
                        (BlitSource::Buffer { buffer, rect }, Convert::Narrow)
                    }
                    (_, Some((surface, rect)), Some(Place::Elsewhere)) => {
                        let (plane, rect) = (plane(&surface), rect.map(i));
                        (BlitSource::Memory { plane, rect }, Convert::None)
                    }
                    _ => return false,
                };
                self.gpu_blit(Blit {
                    dst: plane(&plan.dst),
                    area,
                    rects: vec![area],
                    src: source,
                    convert,
                    mirror: [false; 2],
                    src_key: None,
                    dst_key: None,
                })
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::blit2d::tests::Numbers;
    use super::super::render3d::tests::{A, B, float, gpu_only, mirrored, rectangle, reg3d};
    use super::super::render3d::{
        AUX_BUFFER_ADDR, AUX_BUFFER_STRIDE, COL_BUFFER_ADDR, COL_BUFFER_STRIDE,
    };
    use crate::voodoo::gpu::{GpuLink, Model, ModelRenderer};
    use crate::voodoo::mirror::Frame;
    use crate::voodoo::raster::Stride;
    use crate::voodoo::regs::{
        CLIP_LEFT_RIGHT, CLIP_LOW_Y_HIGH_Y, FASTFILL_CMD, FBZ_MODE, FSTART_Z, ZA_COLOR,
    };
    use crate::voodoo::{Now, Voodoo};
    use std::cell::RefCell;
    use std::rc::Rc;

    const AUX: u32 = 0x40_0000;
    /// Memory outside the 3D buffers, 16 bits a pixel and 1280 bytes a
    /// row (or 10 tiles), and 32 bits a pixel and 2560 bytes (20 tiles).
    const DESKTOP: u32 = 0x60_0000;
    const WIDE: u32 = 0x70_0000;

    /// The 2D engine's dstBaseAddr and dstFormat for a surface at `base`
    /// with `bpp` bytes a pixel and 640 pixels a row, linear or tiled.
    fn surface(base: u32, bpp: u32, tiled: bool) -> (u32, u32) {
        let format = if bpp == 2 { 3 << 16 } else { 5 << 16 };
        match tiled {
            true => (base | 1 << 31, format | (640 * bpp / 128)),
            false => (base, format | (640 * bpp)),
        }
    }

    /// A card whose renderer is `model`, as `gpu_card` has it: colour
    /// buffers of 640 by 64 at A and B and a depth buffer at `AUX`, in
    /// tiles if `tiled`.
    pub(crate) fn model_card(model: &Rc<RefCell<Model>>, tiled: bool) -> Voodoo {
        card_with(Box::new(ModelRenderer(Rc::clone(model))), tiled)
    }

    /// The same card with `link` for its renderer.
    fn card_with(link: Box<dyn GpuLink>, tiled: bool) -> Voodoo {
        let mut v = mirrored();
        v.set_software_picture(false);
        v.set_gpu_link(Some(link));
        v.set_gpu_reads(true);
        let stride = if tiled { 1 << 15 | 10 } else { 1280 };
        reg3d(&mut v, COL_BUFFER_STRIDE, stride);
        reg3d(&mut v, AUX_BUFFER_ADDR, AUX);
        reg3d(&mut v, AUX_BUFFER_STRIDE, stride);
        reg3d(&mut v, CLIP_LEFT_RIGHT, 640);
        reg3d(&mut v, CLIP_LOW_Y_HIGH_Y, 64);
        reg3d(&mut v, ZA_COLOR, 0xFFFF);
        v
    }

    /// A frame drawn into `buffer` as `draw_frame` draws it, but left
    /// unshown, by the renderer, which gets it.
    pub(crate) fn frame(v: &mut Voodoo, model: &Rc<RefCell<Model>>, buffer: u32) {
        if let Some(frame) = unshown_frame(v, buffer) {
            model.borrow_mut().apply(&frame.commands);
        }
    }

    /// A frame drawn into `buffer` as `draw_frame` draws it, but left
    /// unshown: what the renderer is to draw of it.
    fn unshown_frame(v: &mut Voodoo, buffer: u32) -> Option<Frame> {
        reg3d(v, COL_BUFFER_ADDR, buffer);
        reg3d(v, FBZ_MODE, 1 << 10);
        reg3d(v, FASTFILL_CMD, 0);
        reg3d(v, FBZ_MODE, 1 | 1 << 4 | 3 << 5 | 1 << 9 | 1 << 10);
        float(v, FSTART_Z, 1000.0);
        rectangle(v, (0.0, 0.0, 640.0, 64.0), (200.0, 30.0, 60.0));
        v.take_mirror()
    }

    /// A fill or a copy (`command`, a raster operation and a command
    /// number) of `size` pixels at `to` of `dst`, from `from` of `src`,
    /// within the whole of clip 0.
    #[allow(clippy::too_many_arguments)]
    fn command_2d(
        v: &mut Voodoo,
        command: u32,
        dst: (u32, u32),
        src: (u32, u32),
        to: (u32, u32),
        from: (u32, u32),
        size: (u32, u32),
        colour: u32,
    ) {
        for (reg, value) in [
            (0x08, 0),
            (0x0C, 0x0FFF_0FFF),
            (0x10, dst.0),
            (0x14, dst.1),
            (0x34, src.0),
            (0x54, src.1),
            (0x64, colour),
            (0x5C, from.1 << 16 | from.0),
            (0x68, size.1 << 16 | size.0),
            (0x6C, to.1 << 16 | to.0),
            (0x70, command | 1 << 8),
        ] {
            v.banshee_2d_write(reg, value);
        }
    }

    const FILL: u32 = 0xCC << 24 | 5;
    const COPY: u32 = 0xCC << 24 | 1;

    /// Pixel (`x`, `y`) of the 16-bit surface at `base` in the memory, its
    /// rows as the buffers' (`tiled`), read back first.
    fn memory_pixel(v: &Voodoo, base: u32, tiled: bool, (x, y): (u32, u32)) -> u16 {
        let stride = Stride { words: if tiled { 10 * 2048 } else { 640 }, tiled };
        let at = base as usize / 2 + stride.row(y as usize) + stride.col(x as usize);
        v.frame_buffer().get(at)
    }

    fn status(v: &Voodoo) -> serde_json::Value {
        v.describe(Now::default())
    }

    #[test]
    fn a_buffer_filled_every_frame_stays_on_the_gpu() {
        // A game that clears its buffers with the 2D engine before it
        // draws them (Moto Racer): the fill goes into the renderer's
        // picture of the buffer, nothing is read back, and the frames
        // after it are the renderer's alone. Read back, the memory has
        // the renderer's last frame, and a fill after it.
        for tiled in [false, true] {
            let model = Rc::default();
            let mut v = model_card(&model, tiled);
            let a = surface(A, 2, tiled);
            for n in 1..=4 {
                command_2d(&mut v, FILL, a, a, (0, 0), (0, 0), (640, 64), 0x1234);
                frame(&mut v, &model, A);
                assert_eq!(gpu_only(&v), 3 * n, "frame {n}, tiled {tiled}");
            }
            command_2d(&mut v, FILL, a, a, (8, 4), (0, 0), (2, 1), 0xF800);
            // The first fill came before the renderer drew anything, and
            // went into the memory.
            let s = status(&v);
            assert_eq!(s["gpu_read_backs"], 0, "tiled {tiled}: {s}");
            assert_eq!(s["gpu_2d"]["engine_commands"], 4, "tiled {tiled}: {s}");
            for (x, y) in [(0, 0), (7, 4), (10, 4), (639, 63)] {
                let picture = model.borrow().pixel(Some(A / 2), x, y);
                assert_eq!(memory_pixel(&v, A, tiled, (x, y)), picture, "{x} {y}");
            }
            for x in [8, 9] {
                assert_eq!(memory_pixel(&v, A, tiled, (x, 4)), 0xF800, "{x}");
            }
        }
    }

    #[test]
    fn a_depth_buffer_cleared_by_the_2d_engine_stays_on_the_gpu() {
        let model = Rc::default();
        let mut v = model_card(&model, true);
        frame(&mut v, &model, A);
        let aux = surface(AUX, 2, true);
        command_2d(&mut v, FILL, aux, aux, (0, 0), (0, 0), (640, 64), 0xFFFF);
        frame(&mut v, &model, A);
        assert_eq!(gpu_only(&v), 6);
        command_2d(&mut v, FILL, aux, aux, (0, 0), (0, 0), (640, 2), 0xABCD);
        assert_eq!(status(&v)["gpu_read_backs"], 0);
        assert_eq!(memory_pixel(&v, AUX, true, (5, 1)), 0xABCD);
        let depth = model.borrow().pixel(None, 5, 2);
        assert_eq!(memory_pixel(&v, AUX, true, (5, 2)), depth);
    }

    #[test]
    fn a_buffer_copied_to_the_desktop_every_frame_stays_on_the_gpu() {
        // A windowed game copies its back buffer to the desktop every
        // frame: the copy is drawn in the renderer's copy of the memory
        // from its picture of the buffer, and the buffer's frames stay
        // the renderer's. Read back, the desktop has the last frame.
        for tiled in [false, true] {
            let model = Rc::default();
            let mut v = model_card(&model, tiled);
            let (a, desktop) = (surface(A, 2, tiled), surface(DESKTOP, 2, tiled));
            for n in 1..=3 {
                frame(&mut v, &model, A);
                command_2d(&mut v, COPY, desktop, a, (0, 0), (0, 0), (640, 64), 0);
                assert_eq!(gpu_only(&v), 3 * n, "frame {n}, tiled {tiled}");
            }
            let s = status(&v);
            assert_eq!(s["gpu_read_backs"], 0, "tiled {tiled}: {s}");
            assert_eq!(s["gpu_2d"]["blits"], 3, "tiled {tiled}: {s}");
            for (x, y) in [(0, 0), (300, 40), (639, 63)] {
                let picture = model.borrow().pixel(Some(A / 2), x, y);
                assert_eq!(memory_pixel(&v, DESKTOP, tiled, (x, y)), picture, "{x} {y}");
            }
        }
    }

    #[test]
    fn a_copy_between_buffers_stays_on_the_gpu() {
        let model = Rc::default();
        let mut v = model_card(&model, false);
        // The recording's layout has both buffers from the second frame
        // into A on: the first into B read A back.
        for buffer in [A, B, A] {
            frame(&mut v, &model, buffer);
        }
        let read_backs = status(&v)["gpu_read_backs"].clone();
        let (a, b) = (surface(A, 2, false), surface(B, 2, false));
        command_2d(&mut v, COPY, b, a, (100, 10), (20, 5), (200, 30), 0);
        assert_eq!(status(&v)["gpu_read_backs"], read_backs);
        assert_eq!(status(&v)["gpu_2d"]["engine_commands"], 1);
        let picture = |dest: u32, x, y| model.borrow().pixel(Some(dest / 2), x, y);
        let (a, b) = (picture(A, 20, 5), picture(A, 219, 34));
        let outside = picture(B, 99, 10);
        assert_eq!(memory_pixel(&v, B, false, (100, 10)), a);
        assert_eq!(memory_pixel(&v, B, false, (299, 39)), b);
        assert_eq!(memory_pixel(&v, B, false, (99, 10)), outside);
    }

    #[test]
    fn commands_the_gpu_cant_draw_read_back_and_draw_in_memory() {
        // An exclusive-or copy, and a fill through a transparent mono
        // pattern, from and into a buffer only the renderer drew: they
        // read back what they touch, as without the GPU path, and draw
        // what the engine draws in the memory read back.
        let ops = [
            (0x66 << 24 | 1, surface(DESKTOP, 2, false), (0, 0)),
            (0xF0 << 24 | 1 << 13 | 1 << 16 | 5, surface(A, 2, false), (0, 0)),
            // A copy into a buffer from other memory.
            (COPY, surface(A, 2, false), (0, 0)),
        ];
        for (n, (command, dst, from)) in ops.into_iter().enumerate() {
            let models: [Rc<RefCell<Model>>; 2] = Default::default();
            let mut cards = models.each_ref().map(|m| model_card(m, false));
            for (v, model) in cards.iter_mut().zip(&models) {
                frame(v, model, A);
                for i in 0..64 {
                    v.banshee_2d_write(0x100 + 4 * i, 0x5A5A_0F0F ^ i);
                }
            }
            let src = if n == 2 { surface(WIDE, 2, false) } else { surface(A, 2, false) };
            let [v, sw] = &mut cards;
            sw.gpu_sync_all();
            for v in [v, sw] {
                command_2d(v, command, dst, src, (10, 3), from, (100, 20), 0x7E0);
            }
            let [v, sw] = &cards;
            assert_eq!(status(v)["gpu_2d"]["engine_commands"], 0, "{n}");
            assert!(status(v)["gpu_read_backs"].as_u64() > Some(0), "{n}");
            assert!(v.frame_buffer().to_bytes() == sw.frame_buffer().to_bytes(), "{n}");
        }
    }

    /// The registers for a fill or a copy at random, between `surfaces`
    /// (dstBaseAddr and dstFormat values): the command, then words for the
    /// launch area.
    fn random_command(n: &mut Numbers, surfaces: &[(u32, u32)]) -> Vec<(u32, u32)> {
        let pick = |n: &mut Numbers| surfaces[n.below(surfaces.len() as u32) as usize];
        let (dst, src) = (pick(n), pick(n));
        let mut words = vec![(0x10, dst.0), (0x14, dst.1), (0x34, src.0), (0x54, src.1)];
        for clip in [0x08, 0x4C] {
            let (min, max) = match n.below(2) {
                0 => (0, 64 << 16 | 640),
                _ => {
                    let (x, y) = (n.below(300), n.below(60));
                    (x | y << 16, (x + n.below(500)) | (y + n.below(60)) << 16)
                }
            };
            words.extend([(clip, min), (clip + 4, max)]);
        }
        let rops = [0xCC, 0xCC, 0xCC, 0xF0, 0x66, 0x00];
        let rop = |n: &mut Numbers| rops[n.below(rops.len() as u32) as usize];
        words.push((0x30, rop(n) | rop(n) << 8 | rop(n) << 16));
        words.push((0x38, if n.below(5) == 0 { n.below(4) } else { 0 }));
        words.push((0x64, n.next()));
        // The whole of a buffer, a rectangle within one, or one that may
        // reach past it.
        let (to, from, size) = match n.below(3) {
            0 => (0, 0, 64 << 16 | 640),
            1 => {
                let at = |n: &mut Numbers| n.below(560) | n.below(56) << 16;
                (at(n), at(n), (1 + n.below(80)) | (1 + n.below(8)) << 16)
            }
            _ => (n.xy(660, 70), n.xy(660, 70), n.below(300) | n.below(70) << 16),
        };
        words.extend([(0x5C, from), (0x68, size), (0x6C, to)]);
        let mut flags = 1 << 8;
        for flag in [1 << 14, 1 << 15, 1 << 23, 1 << 10, 1 << 11] {
            if n.below(4) == 0 {
                flags |= flag;
            }
        }
        let kind = if n.below(2) == 0 { 5 } else { 1 };
        words.push((0x70, rop(n) << 24 | flags | kind));
        for _ in 0..n.below(3) {
            words.push((0x80 + 4 * n.below(32), n.xy(660, 70)));
        }
        words
    }

    /// A card, and what draws a frame into one of its buffers with the
    /// card's renderer.
    type Drawn = (Voodoo, Box<dyn FnMut(&mut Voodoo, u32)>);

    /// Fills and copies at random between the 3D buffers, and memory
    /// outside them in 16 and 32 bits, linear and tiled, on a card
    /// whose buffers only the renderer drew, and on one that read them
    /// back before each command, which the engine draws in its memory.
    /// Read back, the memory of the two ends up the same, reads of
    /// parts of it between the commands or not. With `buffers_only`,
    /// the commands are only between the two colour buffers. How many
    /// of the commands the renderer drew.
    fn random_commands(
        seed: u64,
        cases: u32,
        buffers_only: bool,
        mut card: impl FnMut(bool) -> Drawn,
    ) -> u64 {
        let mut n = Numbers(seed);
        let mut on_gpu = 0;
        for case in 0..cases {
            let tiled = n.below(2) == 0;
            let mut cards = [card(tiled), card(tiled)];
            for (v, draw) in &mut cards {
                for buffer in [A, B, A] {
                    draw(v, buffer);
                }
            }
            let surfaces = [
                surface(A, 2, tiled),
                surface(B, 2, tiled),
                surface(AUX, 2, tiled),
                surface(DESKTOP, 2, n.below(2) == 0),
                surface(WIDE, 4, n.below(2) == 0),
                surface(WIDE, 2, n.below(2) == 0),
            ];
            let surfaces = if buffers_only { &surfaces[..2] } else { &surfaces[..] };
            for _ in 0..10 {
                // Now and then a frame into a buffer, by the renderer
                // alone: what was read back cools first.
                if n.below(3) == 0 {
                    let buffer = if n.below(2) == 0 { A } else { B };
                    for (v, draw) in &mut cards {
                        v.gpu.get_mut().tracker.cool();
                        draw(v, buffer);
                    }
                }
                let [(v, _), (sw, _)] = &mut cards;
                let words = random_command(&mut n, surfaces);
                sw.gpu_sync_all();
                for &(reg, value) in &words {
                    v.banshee_2d_write(reg, value);
                    sw.banshee_2d_write(reg, value);
                }
                if n.below(4) == 0 {
                    let at = n.below(0x80_0000);
                    v.gpu_sync(at, at + n.below(0x2000));
                }
            }
            let [(v, _), (sw, _)] = &cards;
            on_gpu += status(v)["gpu_2d"]["engine_commands"].as_u64().unwrap();
            let (a, b) = (v.frame_buffer().to_bytes(), sw.frame_buffer().to_bytes());
            if a != b {
                let at = (0..a.len()).find(|&i| a[i] != b[i]).unwrap();
                panic!("case {case}: byte {at:#x}, {:#x} against {:#x}", a[at], b[at]);
            }
        }
        on_gpu
    }

    #[test]
    fn commands_on_the_gpu_draw_what_the_engine_draws() {
        let on_gpu = random_commands(5020, 40, false, |tiled| {
            let model: Rc<RefCell<Model>> = Rc::default();
            let v = model_card(&model, tiled);
            (v, Box::new(move |v, buffer| frame(v, &model, buffer)))
        });
        assert!(on_gpu > 60, "{on_gpu}");
    }

    /// The same with OpenGL's renderer, at 1 and 2 times the card's size
    /// and with samples: the fills and copies between the buffers in its
    /// buffers, those into the memory in its copy of the memory; then
    /// commands between the buffers alone, which are fewer at random.
    /// Needs
    /// OpenGL 3.3: `cargo test --lib gl_commands_on_the_gpu -- --ignored`.
    #[cfg(feature = "voodoo-gl")]
    #[test]
    #[ignore]
    fn gl_commands_on_the_gpu_draw_what_the_engine_draws() {
        use crate::video::shader::Glsl;
        use crate::voodoo::gl::{SharedGl, VoodooGl, tests::test_context};

        /// The renderer, destroyed with the card's drawer.
        struct Owned(Rc<glow::Context>, Rc<RefCell<Option<VoodooGl>>>);
        impl Drop for Owned {
            fn drop(&mut self) {
                if let Some(r) = self.1.borrow_mut().take() {
                    r.destroy(&self.0);
                }
            }
        }

        let context = test_context("2d on the gpu");
        let gl = Rc::new(context.gl);
        for (scale, samples) in [(1, 1), (2, 1), (2, 4)] {
            let seed = 5021 + 10 * scale as u64 + samples as u64;
            let card = |tiled| {
                let renderer = VoodooGl::new(&gl, Glsl::Gl150, scale, samples, 1).unwrap();
                let renderer = Rc::new(RefCell::new(Some(renderer)));
                let link = SharedGl {
                    gl: Rc::clone(&gl),
                    renderer: Rc::clone(&renderer),
                };
                let v = card_with(Box::new(link), tiled);
                let owned = Owned(Rc::clone(&gl), renderer);
                let draw = move |v: &mut Voodoo, buffer| {
                    let Some(frame) = unshown_frame(v, buffer) else { return };
                    if let Some(r) = owned.1.borrow_mut().as_mut() {
                        r.run_early(&owned.0, &frame.commands);
                    }
                };
                (v, Box::new(draw) as Box<dyn FnMut(&mut Voodoo, u32)>)
            };
            let mut card = card;
            let on_gpu = random_commands(seed, 12, false, &mut card);
            assert!(on_gpu > 20, "scale {scale}, samples {samples}: {on_gpu}");
            let between = random_commands(seed + 1, 30, true, &mut card);
            assert!(between > 50, "scale {scale}, samples {samples}: {between}");
        }
    }
}
