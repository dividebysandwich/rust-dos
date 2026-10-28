//! Blt and BltFast between surfaces in a Banshee's or Voodoo3's memory,
//! and fills of them, done by the OpenGL renderer in its copy of the
//! card's memory (`voodoo::gpu`) while it draws the card's picture. What
//! it can't do (a source in system memory, the add-on cards) stays in
//! `blt::run`, which does the same pixel by pixel.

use super::state;
use super::surface::{self, DDPF_PALETTEINDEXED8, DDPF_RGB, MEM_VIDEO, Surface};
use crate::cpu::Cpu;
use crate::voodoo::mirror::{self, BlitSource, Convert, Key, Plane};
use crate::win32::gdi32::Rect;

/// A blit, as `blt::run` has it.
pub(super) struct Blit<'a> {
    pub dst_id: u32,
    pub dst: &'a Surface,
    /// The rectangle the source is stretched onto, and the parts of it
    /// drawn, within the surface.
    pub area: Rect,
    pub rects: &'a [Rect],
    /// The source surface and its rectangle, or a fill of `fill`.
    pub src: Option<(u32, Rect)>,
    pub fill: u32,
    pub src_key: Option<(u32, u32)>,
    pub dst_key: Option<(u32, u32)>,
    pub mirror: [bool; 2],
}

/// The surface `id`'s pixels in the card's memory: where they start, and
/// the bytes a row.
fn plane(cpu: &mut Cpu, id: u32, s: &Surface) -> Option<Plane> {
    if s.memory != MEM_VIDEO {
        return None;
    }
    let (start, stride) = surface::card_memory(cpu, id)?;
    Some(Plane {
        start,
        stride,
        bpp: s.bytes_per_pixel(),
    })
}

/// The colour key `key` of the surface `s`, as `blt::keyed` compares it.
fn key(key: (u32, u32), s: &Surface) -> Key {
    let f = &s.format;
    let rgb = f.flags & DDPF_RGB != 0 && f.flags & DDPF_PALETTEINDEXED8 == 0;
    Key {
        low: key.0,
        high: key.1,
        masks: if rgb { [f.r, f.g, f.b] } else { [0; 3] },
    }
}

fn corners(r: &Rect) -> [i32; 4] {
    [r.left, r.top, r.right, r.bottom]
}

/// Do `b` on the GPU, if it can be: true if it was. The caller marks
/// the destination changed.
pub(super) fn run(cpu: &mut Cpu, b: &Blit) -> bool {
    if !cpu.bus.voodoo.as_ref().is_some_and(|v| v.gpu_ready()) {
        return false;
    }
    let Some(dst) = plane(cpu, b.dst_id, b.dst) else {
        return false;
    };
    let card = cpu.bus.voodoo.as_ref();
    if card.is_some_and(|v| v.gpu_color_buffer(dst.start, dst.stride).is_some()) {
        return into_buffer(cpu, b, dst);
    }
    let (src, convert, src_key) = match b.src {
        None => (BlitSource::Fill(b.fill), Convert::None, None),
        Some((src_id, rect)) => {
            let Some(s) = state(cpu).surface(src_id).cloned() else {
                return false;
            };
            let Some(from) = plane(cpu, src_id, &s) else {
                return false;
            };
            let rect = corners(&rect);
            let src_key = b.src_key.map(|k| key(k, &s));
            let card = cpu.bus.voodoo.as_ref();
            // A buffer the 3D engine draws comes from the renderer's
            // picture of it, 8 bits a channel, without the software
            // renderer drawing it first; keys compare 5-6-5 values.
            let f = &s.format;
            let rgb565 = (f.bits, f.r, f.g, f.b) == (16, 0xF800, 0x07E0, 0x001F);
            match card.and_then(|v| v.gpu_color_buffer(from.start, from.stride)) {
                Some(buffer) if rgb565 && src_key.is_none() => {
                    (BlitSource::Buffer { buffer, rect }, Convert::Narrow, None)
                }
                Some(_) => return false,
                None => (
                    BlitSource::Memory { plane: from, rect },
                    Convert::None,
                    src_key,
                ),
            }
        }
    };
    let blit = mirror::Blit {
        dst,
        area: corners(&b.area),
        rects: b.rects.iter().map(corners).collect(),
        src,
        convert,
        mirror: b.mirror,
        src_key,
        dst_key: b.dst_key.map(|k| key(k, b.dst)),
    };
    cpu.bus.voodoo.as_mut().is_some_and(|v| v.gpu_blit(blit))
}

/// `b` into a colour buffer the 3D engine draws (`dst`), so that the
/// software renderer needn't draw what the engine held back to have the
/// buffer to draw into. A fill is the engine's fastfill, as the HAL's
/// Clear; the pixels from a surface in system memory are queued after
/// the engine's drawing and go to the renderer's picture of the buffer
/// (`Voodoo::write_buffer_pixels`). A destination key needs the
/// buffer's pixels, and stays on the CPU, as do other sources.
fn into_buffer(cpu: &mut Cpu, b: &Blit, dst: Plane) -> bool {
    if dst.bpp != 2 || b.dst_key.is_some() {
        return false;
    }
    let Some((src_id, sr)) = b.src else {
        // A fill is the 3D engine's fastfill, as the HAL's Clear.
        let rects: Vec<[i32; 4]> = b.rects.iter().map(corners).collect();
        return super::d3d::hal::card_fill(cpu, b.dst, b.dst_id, &rects, b.fill as u16);
    };
    let Some(s) = state(cpu).surface(src_id).cloned() else {
        return false;
    };
    if s.memory != surface::MEM_SYSTEM || s.bytes_per_pixel() != 2 {
        return false;
    }
    let mut data = vec![0u8; s.pitch as usize * sr.height().max(0) as usize];
    surface::read_bytes(cpu, &s, sr.top as u32 * s.pitch, &mut data);
    let (dw, dh) = (b.area.width() as i64, b.area.height() as i64);
    let (sw, sh) = (sr.width() as i64, sr.height() as i64);
    let src_key = b.src_key.map(|k| super::blt::Key::new(k, &s));
    let mut pixels = Vec::new();
    for r in b.rects {
        for y in r.top..r.bottom {
            let mut v = (y - b.area.top) as i64;
            if b.mirror[1] {
                v = dh - 1 - v;
            }
            let row = (v * sh / dh) as usize * s.pitch as usize;
            for x in r.left..r.right {
                let mut u = (x - b.area.left) as i64;
                if b.mirror[0] {
                    u = dw - 1 - u;
                }
                let at = row + (sr.left as i64 + u * sw / dw) as usize * 2;
                let value = u16::from_le_bytes([data[at], data[at + 1]]);
                if src_key.as_ref().is_some_and(|k| k.holds(value as u32)) {
                    continue;
                }
                let to = dst.start + y as u32 * dst.stride + x as u32 * 2;
                pixels.push((to, value));
            }
        }
    }
    if pixels.is_empty() {
        return true;
    }
    cpu.bus
        .voodoo
        .as_mut()
        .is_some_and(|v| v.write_buffer_pixels(&pixels))
}
