//! Shadow buffers: 24- and 32-bit render targets and depth buffers on a
//! Banshee or Voodoo3.
//!
//! The card's 3D engine draws 16-bit 565 pixels and 16-bit depth only.
//! For a render target of 24 or 32 bits, or a depth buffer of 24 or 32,
//! the HAL takes a 16-bit block of video memory beside the surface, its
//! shadow, and the card draws there; the surface keeps its own format,
//! which is what the program sees. The two are made the same when one
//! of them changed:
//!
//! - Before the card draws, a shadow behind its surface (which the
//!   program wrote: Lock, Blt, GetDC or a Flip changed its uniqueness
//!   value) is loaded from it, each channel's top bits.
//! - When the program can see the surface (EndScene, Lock, GetDC, Blt
//!   and BltFast from or to it, Flip), a shadow the card drew into is
//!   stored into it: each pixel whose 16 bits differ from the top bits
//!   of the surface's pixel gets the 16-bit value widened by repeating
//!   its bits, and the others keep theirs.
//!
//! A Clear writes its exact value into the surface and the 16-bit one
//! into the shadow, so cleared colors read back as they were given, as
//! do pixels the program wrote and the card didn't draw over. What the
//! card draws has 16-bit precision.

use super::super::surface::{self, DDPF_ZBUFFER, MEM_VIDEO, Surface};
use super::super::{display, state};
use crate::cpu::Cpu;
use crate::win32::glide::hw;
use crate::win32::screen;

/// The shadow of the surface `surface`: its block of video memory at
/// `at`, `len` bytes, rows `width * 2` bytes apart; the surface's
/// uniqueness value when the two were last the same; and whether the
/// card drew into the shadow since (`ahead`).
#[derive(Clone, Debug, Default)]
pub struct Shadow {
    pub surface: u32,
    pub at: u32,
    pub len: u32,
    pub uniqueness: u32,
    pub ahead: bool,
}

crate::state_fields!(Shadow {
    surface,
    at,
    len,
    uniqueness,
    ahead
});

fn shadows(cpu: &mut Cpu) -> &mut Vec<Shadow> {
    &mut state(cpu).hal.shadows
}

/// Whether the surface `s` is drawn through a shadow: a render target
/// or depth buffer of more than 16 bits in video memory, on a Banshee
/// or Voodoo3.
fn shadowed(cpu: &mut Cpu, s: &Surface) -> bool {
    hw::banshee(cpu) && s.memory == MEM_VIDEO && !s.lost && matches!(s.format.bits, 24 | 32)
}

/// Where the card draws the surface `id`: its block of the card's
/// memory and the bytes a row; its shadow's for a 24- or 32-bit one,
/// loaded from it first if the program changed it. None if it isn't in
/// the card's memory or there is no room for the shadow.
pub fn place(cpu: &mut Cpu, id: u32) -> Option<(u32, u32)> {
    let s = state(cpu).surface(id)?.clone();
    if !shadowed(cpu, &s) {
        return surface::card_memory(cpu, id);
    }
    let len = s.width * s.height * 2;
    let i = match find(cpu, id, len) {
        Some(i) => i,
        None => {
            let at = display::alloc(cpu, len).ok()?;
            shadows(cpu).push(Shadow {
                surface: id,
                at,
                len,
                uniqueness: s.uniqueness.wrapping_sub(1),
                ahead: false,
            });
            shadows(cpu).len() - 1
        }
    };
    let sh = shadows(cpu)[i].clone();
    if !sh.ahead && sh.uniqueness != s.uniqueness {
        load(cpu, &s, sh.at);
        shadows(cpu)[i].uniqueness = s.uniqueness;
    }
    Some((sh.at, s.width * 2))
}

/// The shadow of `id`, if it has one whose block is still there and
/// of `len` bytes; one that isn't goes.
fn find(cpu: &mut Cpu, id: u32, len: u32) -> Option<usize> {
    let i = shadows(cpu).iter().position(|sh| sh.surface == id)?;
    let sh = shadows(cpu)[i].clone();
    let there = state(cpu)
        .display
        .blocks
        .iter()
        .any(|&(at, l)| at == sh.at && l >= sh.len);
    if there && sh.len == len {
        return Some(i);
    }
    if there {
        display::free(cpu, sh.at);
    }
    shadows(cpu).remove(i);
    None
}

/// The card is drawing into the shadow of `id`, if it has one.
pub fn drawn(cpu: &mut Cpu, id: u32) {
    if let Some(sh) = shadows(cpu).iter_mut().find(|sh| sh.surface == id) {
        sh.ahead = true;
    }
}

/// The program is to see the surface `id`: what the card drew into its
/// shadow goes into it.
pub fn expose(cpu: &mut Cpu, id: u32) {
    let Some(i) = shadows(cpu)
        .iter()
        .position(|sh| sh.surface == id && sh.ahead)
    else {
        return;
    };
    let at = shadows(cpu)[i].at;
    shadows(cpu)[i].ahead = false;
    let Some(s) = state(cpu).surface(id).cloned() else {
        return;
    };
    if shadowed(cpu, &s) {
        store(cpu, &s, at);
        surface::changed(cpu, id);
    }
    let value = state(cpu).surface(id).map_or(0, |s| s.uniqueness);
    shadows(cpu)[i].uniqueness = value;
}

/// Every shadow the card drew into goes into its surface.
pub fn expose_all(cpu: &mut Cpu) {
    let ids: Vec<u32> = shadows(cpu).iter().map(|sh| sh.surface).collect();
    for id in ids {
        expose(cpu, id);
    }
}

/// Fill `rects` of the shadowed surface `id` (all of it with none):
/// the surface with `exact`, its shadow with the 16-bit `value`. False
/// if `id` has no shadow, and nothing was filled.
pub fn fill(cpu: &mut Cpu, id: u32, rects: &[[i32; 4]], exact: u32, value: u16) -> bool {
    let Some(s) = state(cpu).surface(id).cloned() else {
        return false;
    };
    if !shadowed(cpu, &s) {
        return false;
    }
    // What the card drew outside the rectangles stays.
    expose(cpu, id);
    let Some((at, pitch)) = place(cpu, id) else {
        return false;
    };
    let whole = [[0, 0, s.width as i32, s.height as i32]];
    let rects = if rects.is_empty() { &whole[..] } else { rects };
    let bpp = s.bytes_per_pixel();
    let mask = keep_mask(&s);
    for r in rects {
        let x0 = r[0].clamp(0, s.width as i32) as u32;
        let x1 = r[2].clamp(0, s.width as i32) as u32;
        let y0 = r[1].clamp(0, s.height as i32) as u32;
        let y1 = r[3].clamp(0, s.height as i32) as u32;
        if x1 <= x0 {
            continue;
        }
        let n = (x1 - x0) as usize;
        let shadow_row: Vec<u8> = (0..n).flat_map(|_| value.to_le_bytes()).collect();
        let mut row = vec![0u8; n * bpp as usize];
        for y in y0..y1 {
            let offset = y * s.pitch + x0 * bpp;
            surface::read_bytes(cpu, &s, offset, &mut row);
            for px in row.chunks_mut(bpp as usize) {
                let old = get(px);
                put(px, old & mask | exact & !mask);
            }
            surface::write_bytes(cpu, &s, offset, &row);
            screen::write(cpu, at + y * pitch + x0 * 2, &shadow_row);
        }
    }
    surface::changed(cpu, id);
    let value = state(cpu).surface(id).map_or(0, |s| s.uniqueness);
    if let Some(sh) = shadows(cpu).iter_mut().find(|sh| sh.surface == id) {
        sh.uniqueness = value;
        sh.ahead = false;
    }
    true
}

/// The surface `id` went: its shadow with it.
pub fn forget(cpu: &mut Cpu, id: u32) {
    if let Some(i) = shadows(cpu).iter().position(|sh| sh.surface == id) {
        let sh = shadows(cpu).remove(i);
        display::free(cpu, sh.at);
    }
}

/// The HAL lets go: every shadow goes, into its surface first.
pub fn forget_all(cpu: &mut Cpu) {
    expose_all(cpu);
    for sh in std::mem::take(shadows(cpu)) {
        display::free(cpu, sh.at);
    }
}

/// The bits of a pixel of `s` outside its color or depth: they stay.
fn keep_mask(s: &Surface) -> u32 {
    let f = &s.format;
    if f.flags & DDPF_ZBUFFER != 0 {
        !f.g
    } else {
        !(f.r | f.g | f.b)
    }
}

/// The surface into its shadow at `at`: each channel's top 5, 6 and 5
/// bits, or depth's top 16.
fn load(cpu: &mut Cpu, s: &Surface, at: u32) {
    let bpp = s.bytes_per_pixel() as usize;
    let mut row = vec![0u8; s.width as usize * bpp];
    let mut out = vec![0u8; s.width as usize * 2];
    for y in 0..s.height {
        surface::read_bytes(cpu, s, y * s.pitch, &mut row);
        for (px, o) in row.chunks(bpp).zip(out.chunks_mut(2)) {
            o.copy_from_slice(&narrow(s, get(px)).to_le_bytes());
        }
        screen::write(cpu, at + y * s.width * 2, &out);
    }
}

/// The shadow at `at` into the surface, its values widened.
fn store(cpu: &mut Cpu, s: &Surface, at: u32) {
    let bpp = s.bytes_per_pixel() as usize;
    let mask = keep_mask(s);
    let mut row = vec![0u8; s.width as usize * bpp];
    let mut shadow_row = vec![0u8; s.width as usize * 2];
    for y in 0..s.height {
        screen::read(cpu, at + y * s.width * 2, &mut shadow_row);
        surface::read_bytes(cpu, s, y * s.pitch, &mut row);
        for (px, v) in row.chunks_mut(bpp).zip(shadow_row.chunks(2)) {
            // A pixel whose 16 bits the card left as they were keeps its
            // own, which may have more.
            let v = u16::from_le_bytes([v[0], v[1]]);
            let old = get(px);
            if narrow(s, old) != v {
                put(px, old & mask | widen(s, v) & !mask);
            }
        }
        surface::write_bytes(cpu, s, y * s.pitch, &row);
    }
}

/// A pixel of 1 to 4 bytes, little-endian.
fn get(px: &[u8]) -> u32 {
    px.iter().rev().fold(0, |v, &b| v << 8 | b as u32)
}

fn put(px: &mut [u8], value: u32) {
    for (i, b) in px.iter_mut().enumerate() {
        *b = (value >> (8 * i)) as u8;
    }
}

/// The `bits` top bits of the field `mask` selects of `value`.
fn top(value: u32, mask: u32, bits: u32) -> u32 {
    if mask == 0 {
        return 0;
    }
    let width = mask.count_ones();
    let v = (value & mask) >> mask.trailing_zeros();
    if width >= bits {
        v >> (width - bits)
    } else {
        widen_bits(v, width, bits)
    }
}

/// `v` of `from` bits as `to` bits, its bits repeated below it.
fn widen_bits(v: u32, from: u32, to: u32) -> u32 {
    if from == 0 {
        return 0;
    }
    let mut out = 0u64;
    let mut have = 0;
    while have < to {
        out = out << from | v as u64;
        have += from;
    }
    (out >> (have - to)) as u32
}

/// A pixel of `s` as the card's 16 bits: 565, or depth.
fn narrow(s: &Surface, value: u32) -> u16 {
    let f = &s.format;
    if f.flags & DDPF_ZBUFFER != 0 {
        return top(value, f.g, 16) as u16;
    }
    (top(value, f.r, 5) << 11 | top(value, f.g, 6) << 5 | top(value, f.b, 5)) as u16
}

/// The card's 16 bits as a pixel of `s`.
fn widen(s: &Surface, v: u16) -> u32 {
    let f = &s.format;
    let field = |v: u32, from: u32, mask: u32| {
        if mask == 0 {
            return 0;
        }
        widen_bits(v, from, mask.count_ones()) << mask.trailing_zeros()
    };
    let v = v as u32;
    if f.flags & DDPF_ZBUFFER != 0 {
        return field(v, 16, f.g);
    }
    field(v >> 11, 5, f.r) | field(v >> 5 & 0x3F, 6, f.g) | field(v & 0x1F, 5, f.b)
}

/// A 32-bit color of `s` (ARGB, 8 bits each) as its pixel, and as the
/// card's 565: Clear's two values.
pub fn color_values(s: &Surface, argb: u32) -> (u32, u16) {
    let f = &s.format;
    let field = |shift: u32, mask: u32| {
        if mask == 0 {
            return 0;
        }
        top(argb >> shift & 0xFF, 0xFF, mask.count_ones()) << mask.trailing_zeros()
    };
    let pixel = field(16, f.r) | field(8, f.g) | field(0, f.b);
    let [b, g, r, _] = argb.to_le_bytes().map(u32::from);
    (pixel, (r >> 3 << 11 | g >> 2 << 5 | b >> 3) as u16)
}

/// A depth of 0 to 1 as a pixel of the depth buffer `s`, and as the
/// card's 16 bits.
pub fn depth_values(s: &Surface, z: f32) -> (u32, u16) {
    let z = z.clamp(0.0, 1.0) as f64;
    let mask = s.format.g;
    let width = mask.count_ones();
    let max = if width >= 32 {
        u32::MAX as f64
    } else {
        ((1u64 << width) - 1) as f64
    };
    let exact = ((z * max) as u32) << mask.trailing_zeros().min(31) & mask;
    // The top 16 bits, as the shadow is loaded.
    (exact, narrow(s, exact))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_widen_by_repeating() {
        assert_eq!(widen_bits(0x1F, 5, 8), 0xFF);
        assert_eq!(widen_bits(0x10, 5, 8), 0x84);
        assert_eq!(widen_bits(0xFFFF, 16, 24), 0xFF_FFFF);
        assert_eq!(widen_bits(0x8000, 16, 24), 0x80_0080);
    }
}
