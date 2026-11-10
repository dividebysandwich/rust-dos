//! Blt and BltFast with a DXT surface as the source or the destination,
//! as DirectX 7's DirectDraw does them on Windows 98 SE. A probe
//! program run there (see "DXT surfaces" in
//! `docs/builtin-windows-architecture.md`) showed:
//!
//! - From a DXT surface to an RGB one of 16, 24 or 32 bits, Blt and
//!   BltFast decode the source (`video::dxt`, DirectDraw's rules) and
//!   copy, stretch and mirror it. Each channel keeps its top bits; a
//!   32-bit destination gets the alpha in its top byte whether its
//!   format has alpha or not, and a 16-bit one only in its alpha bits.
//!   The stretch samples at the middle of each destination pixel, in
//!   16.16 fixed point, which differs from the RGB blits' stretch.
//! - DXT2 and DXT4, whose colours are premultiplied, don't go to a
//!   format with alpha: E_NOTIMPL.
//! - To an 8-bit palette surface the blit succeeds and writes nothing.
//! - A source colour key fails with E_NOTIMPL when the source has one,
//!   and a blit asking for a key the surface hasn't got fails with
//!   DDERR_INVALIDPARAMS. A destination colour key fails with
//!   DDERR_INVALIDPARAMS.
//! - A DXT destination takes the blocks of a source of its format, when
//!   the rectangles are the same size and on block boundaries. Windows
//!   compresses whatever else is blitted to a DXT surface (another
//!   format, a stretch, a rectangle within blocks), with its own
//!   encoder; that isn't done here and fails with E_NOTIMPL. A fill of
//!   a DXT surface fails with E_NOTIMPL on Windows too.

use super::surface::{self, DDPF_ALPHAPIXELS, DDPF_PALETTEINDEXED8, DDPF_RGB, Surface};
use super::{DDERR_INVALIDPARAMS, DDERR_UNSUPPORTED, Format};
use crate::cpu::Cpu;
use crate::video::dxt::{self, Rgba, Rules};
use crate::win32::gdi32::Rect;

/// What a blit between a source and a destination of which one or both
/// are DXT surfaces does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Plan {
    /// Decode the DXT source into the RGB destination.
    Decode,
    /// Copy blocks between two surfaces of one DXT format.
    Copy,
    /// Succeed without drawing: an 8-bit palette destination.
    Nothing,
}

/// The plan for a blit from `s` to `d`, one of them DXT, or the error
/// Windows gives for it.
pub(super) fn plan(s: &Surface, d: &Surface) -> Result<Plan, u32> {
    let Some(from) = surface::dxt_format(&s.format) else {
        // Into a DXT surface from an RGB one: compression.
        return Err(DDERR_UNSUPPORTED);
    };
    if d.compressed() {
        return match surface::dxt_format(&d.format) {
            Some(to) if to == from => Ok(Plan::Copy),
            _ => Err(DDERR_UNSUPPORTED),
        };
    }
    let f = &d.format;
    if f.flags & DDPF_PALETTEINDEXED8 != 0 {
        return Ok(Plan::Nothing);
    }
    if f.flags & DDPF_RGB == 0 || ![16, 24, 32].contains(&f.bits) {
        return Err(DDERR_UNSUPPORTED);
    }
    if from.premultiplied() && f.flags & DDPF_ALPHAPIXELS != 0 && f.a != 0 {
        return Err(DDERR_UNSUPPORTED);
    }
    Ok(Plan::Decode)
}

/// The error of a blit from the DXT surface `s` through colour keys,
/// if it asks for any: `src` and `dst` say whether it asks for a source
/// key and a destination key, and `src_set` whether the source has
/// one where it is asked for (a key given with the blit is set).
pub(super) fn keys(src: bool, src_set: bool, dst: bool) -> Result<(), u32> {
    if dst || src && !src_set {
        return Err(DDERR_INVALIDPARAMS);
    }
    if src {
        return Err(DDERR_UNSUPPORTED);
    }
    Ok(())
}

/// The source column (or row) of each of `dst` destination pixels
/// stretched from `src`: the middle of each destination pixel, in 16.16
/// fixed point with the step rounded down.
fn samples(src: u32, dst: u32) -> Vec<u32> {
    let step = ((src as u64) << 16) / dst as u64;
    (0..dst as u64)
        .map(|i| ((step / 2 + i * step) >> 16).min(src as u64 - 1) as u32)
        .collect()
}

/// The 8-bit value `v` in the channel `mask` of a pixel: its top bits.
fn channel(v: u8, mask: u32) -> u32 {
    if mask == 0 {
        return 0;
    }
    let bits = mask.count_ones().min(8);
    ((v as u32) >> (8 - bits) << mask.trailing_zeros()) & mask
}

/// A decoded pixel in the RGB format `f`.
fn to_pixel([r, g, b, a]: Rgba, f: &Format) -> u32 {
    let colour = f.r | f.g | f.b;
    let alpha = if f.flags & DDPF_ALPHAPIXELS != 0 && f.a != 0 {
        f.a
    } else if f.bits == 32 && colour & 0xFF00_0000 == 0 {
        // Windows writes the alpha into the top byte of a 32-bit
        // destination without alpha too.
        0xFF00_0000
    } else {
        0
    };
    channel(r, f.r) | channel(g, f.g) | channel(b, f.b) | channel(a, alpha)
}

/// The pixels of the rectangle `r` of the DXT surface `s`, stretched to
/// `dw` by `dh` and mirrored as `mirror` says, in the format of the
/// destination `d`: `d`'s bytes a pixel each, row after row.
pub(super) fn decode(
    cpu: &mut Cpu,
    s: &Surface,
    r: &Rect,
    (dw, dh): (u32, u32),
    mirror: [bool; 2],
    d: &Surface,
) -> Vec<u8> {
    let Some(format) = surface::dxt_format(&s.format) else {
        return Vec::new();
    };
    let (left, top, right, bottom) = (r.left as u32, r.top as u32, r.right as u32, r.bottom as u32);
    // The rows of blocks the rectangle is in.
    let row_bytes = format.row_bytes(s.width);
    let first = top / 4;
    let rows = (bottom - 1) / 4 + 1 - first;
    let mut data = vec![0u8; rows as usize * row_bytes];
    surface::read_bytes(cpu, s, first * row_bytes as u32, &mut data);
    let top_in = top - first * 4;
    let pixels = dxt::decode_rect(
        format,
        Rules::DirectDraw,
        &data,
        s.width,
        [left, top_in, right, top_in + (bottom - top)],
    );
    let (sw, sh) = (right - left, bottom - top);
    let mut xs = samples(sw, dw);
    let mut ys = samples(sh, dh);
    if mirror[0] {
        xs.reverse();
    }
    if mirror[1] {
        ys.reverse();
    }
    let bytes = d.bytes_per_pixel() as usize;
    let mut out = Vec::with_capacity(dw as usize * dh as usize * bytes);
    for &y in &ys {
        let row = &pixels[(y * sw) as usize..][..sw as usize];
        for &x in &xs {
            let value = to_pixel(row[x as usize], &d.format);
            out.extend_from_slice(&value.to_le_bytes()[..bytes]);
        }
    }
    out
}

/// Copy the blocks of the rectangle `sr` of the DXT surface `s` to the
/// rectangle `dr` of `d`, of the same format. E_NOTIMPL, as this doesn't
/// compress, unless the rectangles are the same size and on block
/// boundaries (or reach the right and bottom edges of their surfaces,
/// for a mipmap level smaller than a block).
pub(super) fn copy(
    cpu: &mut Cpu,
    s: &Surface,
    sr: &Rect,
    d: &Surface,
    dr: &Rect,
) -> Result<(), u32> {
    let Some(format) = surface::dxt_format(&s.format) else {
        return Err(DDERR_UNSUPPORTED);
    };
    let whole = |r: &Rect, x: &Surface| {
        r.left % 4 == 0
            && r.top % 4 == 0
            && (r.right % 4 == 0 || r.right as u32 == x.width)
            && (r.bottom % 4 == 0 || r.bottom as u32 == x.height)
    };
    let same = sr.width() == dr.width() && sr.height() == dr.height();
    // A rectangle reaching a surface's edge within a block has to reach
    // the other's within one too.
    let ends =
        (sr.right % 4 == 0) == (dr.right % 4 == 0) && (sr.bottom % 4 == 0) == (dr.bottom % 4 == 0);
    if !same || !ends || !whole(sr, s) || !whole(dr, d) {
        return Err(DDERR_UNSUPPORTED);
    }
    let bytes = format.block_bytes() as u32;
    let (s_row, d_row) = (
        format.row_bytes(s.width) as u32,
        format.row_bytes(d.width) as u32,
    );
    let across = (sr.width() as u32).div_ceil(4) * bytes;
    let mut row = vec![0u8; across as usize];
    for i in 0..(sr.height() as u32).div_ceil(4) {
        let from = (sr.top as u32 / 4 + i) * s_row + sr.left as u32 / 4 * bytes;
        let to = (dr.top as u32 / 4 + i) * d_row + dr.left as u32 / 4 * bytes;
        surface::read_bytes(cpu, s, from, &mut row);
        surface::write_bytes(cpu, d, to, &row);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The probe's stretches of 8 pixels on Windows 98 SE.
    #[test]
    fn a_stretch_samples_the_middle_of_each_pixel() {
        assert_eq!(samples(8, 8), [0, 1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(samples(8, 4), [1, 3, 5, 7]);
        assert_eq!(samples(8, 6)[3..], [4, 5, 7]);
        assert_eq!(samples(8, 5)[3..], [5, 7]);
        assert_eq!(samples(8, 3)[2], 6);
        assert_eq!(samples(8, 12), [0, 0, 1, 2, 2, 3, 4, 4, 5, 6, 6, 7]);
        assert_eq!(samples(8, 16)[..4], [0, 0, 1, 1]);
    }

    fn rgb(bits: u32, r: u32, g: u32, b: u32, a: u32) -> Format {
        Format {
            flags: DDPF_RGB | if a != 0 { DDPF_ALPHAPIXELS } else { 0 },
            bits,
            r,
            g,
            b,
            a,
            ..Format::default()
        }
    }

    /// The probe's pixels on Windows 98 SE: (173, 173, 8) opaque, and
    /// DXT3's alpha 0x11 and 0x88.
    #[test]
    fn pixels_keep_the_top_bits_of_each_channel() {
        let p = [173, 173, 8, 255];
        assert_eq!(to_pixel(p, &rgb(16, 0xF800, 0x07E0, 0x001F, 0)), 0xAD61);
        assert_eq!(to_pixel(p, &rgb(16, 0x7C00, 0x03E0, 0x001F, 0)), 0x56A1);
        assert_eq!(
            to_pixel(p, &rgb(16, 0x7C00, 0x03E0, 0x001F, 0x8000)),
            0xD6A1
        );
        assert_eq!(
            to_pixel(p, &rgb(16, 0x0F00, 0x00F0, 0x000F, 0xF000)),
            0xFAA0
        );
        assert_eq!(to_pixel(p, &rgb(24, 0xFF_0000, 0xFF00, 0xFF, 0)), 0xADAD08);
        let a8r8g8b8 = rgb(32, 0xFF_0000, 0xFF00, 0xFF, 0xFF00_0000);
        let x8r8g8b8 = rgb(32, 0xFF_0000, 0xFF00, 0xFF, 0);
        assert_eq!(to_pixel(p, &a8r8g8b8), 0xFFAD_AD08);
        assert_eq!(to_pixel([0, 0, 0, 0], &x8r8g8b8), 0);
        assert_eq!(to_pixel([90, 90, 16, 0x11], &x8r8g8b8), 0x115A_5A10);
        let a1 = rgb(16, 0x7C00, 0x03E0, 0x001F, 0x8000);
        assert_eq!(to_pixel([0, 0, 0, 0x77], &a1), 0);
        assert_eq!(to_pixel([0, 0, 0, 0x88], &a1), 0x8000);
    }
}
