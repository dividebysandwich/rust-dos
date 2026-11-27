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
//!   format, a stretch, a rectangle within blocks). On Windows 98 SE a
//!   whole DXT1 to DXT3 blit gave DD_OK. On Windows 11, DXT1 to DXT3 or
//!   DXT5, DXT3 to DXT1, R5G6B5 and A8R8G8B8 to DXT1, and A8R8G8B8 to
//!   DXT3 gave DD_OK whole, between rectangles within blocks and shrunk
//!   (the same probe, #882). Here `compress` does it with
//!   `video::dxt`'s encoder, whose blocks are a good S3TC encoder's
//!   rather than Microsoft's (#880).
//!   The blocks the destination's rectangle cuts are decoded, given the
//!   new pixels, and compressed again. An RGB source is stretched as the
//!   RGB blits stretch, a DXT source as a blit out of DXT does. Into
//!   DXT2 or DXT4 the colours of a source that isn't premultiplied are
//!   multiplied by their alpha, as those formats hold them (Rage Rally
//!   compresses its A8R8G8B8 textures into DXT2 so). What Windows does
//!   there wasn't probed. A premultiplied source going to a format that
//!   isn't fails with E_NOTIMPL, as it does decoding to a format with
//!   alpha, and so does a source of 8 bits. A fill of a DXT surface
//!   fails with E_NOTIMPL on Windows too.

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
    /// Copy blocks between two surfaces of one DXT format, or compress
    /// what can't be copied whole.
    Copy,
    /// Compress an RGB source, or a DXT one of another format, into the
    /// DXT destination.
    Compress,
    /// Succeed without drawing: an 8-bit palette destination.
    Nothing,
}

/// The plan for a blit from `s` to `d`, one of them DXT, or the error
/// Windows gives for it.
pub(super) fn plan(s: &Surface, d: &Surface) -> Result<Plan, u32> {
    if let Some(to) = surface::dxt_format(&d.format) {
        let from = surface::dxt_format(&s.format);
        if from == Some(to) {
            return Ok(Plan::Copy);
        }
        let rgb = s.format.flags & DDPF_RGB != 0
            && s.format.flags & DDPF_PALETTEINDEXED8 == 0
            && [16, 24, 32].contains(&s.format.bits);
        // Premultiplied colours go only to a premultiplied format.
        let fits = from.map_or(rgb, |f| !f.premultiplied() || to.premultiplied());
        if !fits {
            return Err(DDERR_UNSUPPORTED);
        }
        return Ok(Plan::Compress);
    }
    let Some(from) = surface::dxt_format(&s.format) else {
        return Err(DDERR_UNSUPPORTED);
    };
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

/// The source column (or row) of the destination pixels `pixels` of
/// `dst` stretched from `src`, mirrored if `mirror` says so: the middle
/// of each destination pixel, in 16.16 fixed point with the step rounded
/// down.
fn samples(src: u32, dst: u32, pixels: std::ops::Range<u32>, mirror: bool) -> Vec<u32> {
    let step = ((src as u64) << 16) / dst as u64;
    pixels
        .map(|i| {
            let i = if mirror { dst - 1 - i } else { i } as u64;
            ((step / 2 + i * step) >> 16).min(src as u64 - 1) as u32
        })
        .collect()
}

/// The 8-bit value `v` in the channel `mask` of a pixel: its top bits,
/// or in a channel of more than 8 bits, its top 8.
fn channel(v: u8, mask: u32) -> u32 {
    if mask == 0 {
        return 0;
    }
    let bits = mask.count_ones();
    let v = v as u32;
    let top = if bits >= 8 {
        v << (bits - 8)
    } else {
        v >> (8 - bits)
    };
    (top << mask.trailing_zeros()) & mask
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
/// the destination's rectangle `dst` and mirrored as `mirror` says,
/// decoded: of the part `area` of `dst` only, row after row.
fn decoded(
    cpu: &mut Cpu,
    s: &Surface,
    r: &Rect,
    dst: &Rect,
    area: &Rect,
    mirror: [bool; 2],
) -> Vec<Rgba> {
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
    let (dw, dh) = (dst.width() as u32, dst.height() as u32);
    let (x0, y0) = ((area.left - dst.left) as u32, (area.top - dst.top) as u32);
    let (aw, ah) = (area.width() as u32, area.height() as u32);
    let xs = samples(sw, dw, x0..x0 + aw, mirror[0]);
    let ys = samples(sh, dh, y0..y0 + ah, mirror[1]);
    let mut out = Vec::with_capacity(aw as usize * ah as usize);
    for &y in &ys {
        let row = &pixels[(y * sw) as usize..][..sw as usize];
        out.extend(xs.iter().map(|&x| row[x as usize]));
    }
    out
}

/// The pixels of the rectangle `r` of the DXT surface `s`, stretched to
/// the destination's rectangle `dst` and mirrored as `mirror` says, in
/// the format of the destination `d`: `d`'s bytes a pixel each, row
/// after row, of the part `area` of `dst` only.
pub(super) fn decode(
    cpu: &mut Cpu,
    s: &Surface,
    r: &Rect,
    dst: &Rect,
    area: &Rect,
    mirror: [bool; 2],
    d: &Surface,
) -> Vec<u8> {
    let bytes = d.bytes_per_pixel() as usize;
    let pixels = decoded(cpu, s, r, dst, area, mirror);
    let mut out = Vec::with_capacity(pixels.len() * bytes);
    for p in pixels {
        out.extend_from_slice(&to_pixel(p, &d.format).to_le_bytes()[..bytes]);
    }
    out
}

/// The 8-bit value of the channel `mask` of the pixel `value`: its bits
/// repeated below themselves, so that the channel's largest value is
/// 255, or of a channel of more than 8 bits, its top 8. 0 for a
/// channel the format has no bits for: a program may create an RGB
/// surface with a mask of 0.
fn widen(value: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let bits = mask.count_ones();
    let v = (value & mask) >> mask.trailing_zeros();
    if bits >= 8 {
        return (v >> (bits - 8)) as u8;
    }
    let (mut out, mut filled) = (0u32, 0);
    while filled < 8 {
        out = out << bits | v;
        filled += bits;
    }
    (out >> (filled - 8)) as u8
}

/// The RGB pixel `value` of the format `f` as red, green, blue and
/// alpha, opaque where the format has no alpha.
fn from_pixel(value: u32, f: &Format) -> Rgba {
    let alpha = if f.flags & DDPF_ALPHAPIXELS != 0 && f.a != 0 {
        widen(value, f.a)
    } else {
        255
    };
    [
        widen(value, f.r),
        widen(value, f.g),
        widen(value, f.b),
        alpha,
    ]
}

/// The pixels of the rectangle `r` of the RGB surface `s`, stretched to
/// the destination's rectangle `dst` and mirrored as `mirror` says, as
/// the RGB blits stretch (`blt::run`): of the part `area` of `dst`
/// only, row after row.
fn rgb_pixels(
    cpu: &mut Cpu,
    s: &Surface,
    r: &Rect,
    dst: &Rect,
    area: &Rect,
    mirror: [bool; 2],
) -> Vec<Rgba> {
    let bytes = s.bytes_per_pixel() as usize;
    let (sw, sh) = (r.width() as i64, r.height() as i64);
    let (dw, dh) = (dst.width() as i64, dst.height() as i64);
    // The source's column (or row) under the destination's `i`.
    let under = |i: i32, start: i32, dn: i64, sn: i64, flip: bool| {
        let u = (i - start) as i64;
        let u = if flip { dn - 1 - u } else { u };
        u * sn / dn
    };
    let columns: Vec<usize> = (area.left..area.right)
        .map(|x| under(x, dst.left, dw, sw, mirror[0]) as usize)
        .collect();
    // The source's rectangle, read once: a stretch up reads its rows
    // more than once, and a ToadForce reads back the rows of a surface
    // in its memory at once (`surface::read_rows`).
    let len = sw as usize * bytes;
    let mut data = vec![0u8; len * sh as usize];
    let first = r.top as u32 * s.pitch + r.left as u32 * bytes as u32;
    surface::read_rows(cpu, s, first, len, &mut data);
    let mut out = Vec::with_capacity(columns.len() * area.height() as usize);
    for y in area.top..area.bottom {
        let v = under(y, dst.top, dh, sh, mirror[1]) as usize;
        let row = &data[v * len..][..len];
        out.extend(columns.iter().map(|&u| {
            let mut v = [0u8; 4];
            v[..bytes].copy_from_slice(&row[u * bytes..][..bytes]);
            from_pixel(u32::from_le_bytes(v), &s.format)
        }));
    }
    out
}

/// Compress the pixels of the rectangle `sr` of `s` into the DXT
/// surface `d`, stretched to its rectangle `dst` and mirrored as
/// `mirror` says, where the rectangles `rects` (the parts of `dst` a
/// clipper leaves on the surface) allow: the blocks they cut are
/// decoded, given the new pixels and compressed again.
pub(super) fn compress(
    cpu: &mut Cpu,
    s: &Surface,
    sr: &Rect,
    d: &Surface,
    dst: &Rect,
    rects: &[Rect],
    mirror: [bool; 2],
) -> Result<(), u32> {
    let Some(format) = surface::dxt_format(&d.format) else {
        return Err(DDERR_UNSUPPORTED);
    };
    let Some(area) = rects.iter().copied().reduce(|a, b| bounding(&a, &b)) else {
        return Ok(());
    };
    let mut pixels = if s.compressed() {
        decoded(cpu, s, sr, dst, &area, mirror)
    } else {
        rgb_pixels(cpu, s, sr, dst, &area, mirror)
    };
    let premultiplied = surface::dxt_format(&s.format).is_some_and(|f| f.premultiplied());
    if format.premultiplied() && !premultiplied {
        for p in &mut pixels {
            for c in 0..3 {
                p[c] = ((p[c] as u32 * p[3] as u32 + 127) / 255) as u8;
            }
        }
    }
    let aw = area.width() as usize;
    // The blocks under `area`, a row of them at a time.
    let bytes = format.block_bytes();
    let d_row = format.row_bytes(d.width) as u32;
    let (bx0, bx1) = (area.left as u32 / 4, (area.right as u32 - 1) / 4);
    let (by0, by1) = (area.top as u32 / 4, (area.bottom as u32 - 1) / 4);
    let mut row = vec![0u8; (bx1 - bx0 + 1) as usize * bytes];
    for by in by0..=by1 {
        let at = by * d_row + bx0 * bytes as u32;
        surface::read_bytes(cpu, d, at, &mut row);
        for (block, bx) in row.chunks_mut(bytes).zip(bx0..) {
            let mut texels = dxt::decode_block(format, Rules::S3tc, block);
            let (x, y) = (bx as i32 * 4, by as i32 * 4);
            let cell = Rect::new(x, y, x + 4, y + 4);
            let mut touched = false;
            for r in rects {
                let part = r.intersect(&cell);
                for y in part.top..part.bottom {
                    for x in part.left..part.right {
                        let from = (y - area.top) as usize * aw + (x - area.left) as usize;
                        texels[(y % 4 * 4 + x % 4) as usize] = pixels[from];
                        touched = true;
                    }
                }
            }
            if touched {
                // A mipmap level under 4 pixels a side is one block whose
                // texels past its edge are never shown: they repeat the
                // ones at the edge, so that the encoder fits its colours
                // to the pixels shown only.
                let (w, h) = (d.width as i32 - x, d.height as i32 - y);
                if w < 4 || h < 4 {
                    let shown = texels;
                    for (i, t) in texels.iter_mut().enumerate() {
                        let (tx, ty) = ((i % 4) as i32, (i / 4) as i32);
                        let (sx, sy) = (tx.min(w - 1), ty.min(h - 1));
                        *t = shown[(sy * 4 + sx) as usize];
                    }
                }
                block.copy_from_slice(&dxt::encode_block(format, &texels));
            }
        }
        surface::write_bytes(cpu, d, at, &row);
    }
    Ok(())
}

/// The smallest rectangle that holds both `a` and `b`.
fn bounding(a: &Rect, b: &Rect) -> Rect {
    Rect::new(
        a.left.min(b.left),
        a.top.min(b.top),
        a.right.max(b.right),
        a.bottom.max(b.bottom),
    )
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
    let rows = (sr.height() as u32).div_ceil(4);
    // Every row is read before any is written: the source and the
    // destination may be one surface, the rectangles overlapping.
    let mut data = vec![0u8; (rows * across) as usize];
    let first = sr.top as u32 / 4 * s_row + sr.left as u32 / 4 * bytes;
    surface::will_read_rows(cpu, s, first, s_row, rows, across as usize);
    for (row, i) in data.chunks_mut(across as usize).zip(0u32..) {
        let from = (sr.top as u32 / 4 + i) * s_row + sr.left as u32 / 4 * bytes;
        surface::read_bytes(cpu, s, from, row);
    }
    for (row, i) in data.chunks(across as usize).zip(0u32..) {
        let to = (dr.top as u32 / 4 + i) * d_row + dr.left as u32 / 4 * bytes;
        surface::write_bytes(cpu, d, to, row);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// All the samples of a stretch from `src` to `dst`.
    fn all(src: u32, dst: u32) -> Vec<u32> {
        samples(src, dst, 0..dst, false)
    }

    /// The probe's stretches of 8 pixels on Windows 98 SE.
    #[test]
    fn a_stretch_samples_the_middle_of_each_pixel() {
        assert_eq!(all(8, 8), [0, 1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(all(8, 4), [1, 3, 5, 7]);
        assert_eq!(all(8, 6)[3..], [4, 5, 7]);
        assert_eq!(all(8, 5)[3..], [5, 7]);
        assert_eq!(all(8, 3)[2], 6);
        assert_eq!(all(8, 12), [0, 0, 1, 2, 2, 3, 4, 4, 5, 6, 6, 7]);
        assert_eq!(all(8, 16)[..4], [0, 0, 1, 1]);
    }

    /// The samples of part of a destination, as a clipper leaves it, are
    /// those of the whole stretch there, mirrored too.
    #[test]
    fn part_of_a_stretch_samples_as_the_whole_does() {
        let whole = all(8, 12);
        assert_eq!(samples(8, 12, 3..9, false), whole[3..9]);
        let mut mirrored = whole.clone();
        mirrored.reverse();
        assert_eq!(samples(8, 12, 0..12, true), mirrored);
        assert_eq!(samples(8, 12, 3..9, true), mirrored[3..9]);
        // A rectangle reaching far outside the surface.
        let huge = samples(8, 0x4_0000, 0x2_0000..0x2_0002, false);
        assert_eq!(huge, [4, 4]);
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

    /// A channel of more than 8 bits gets the value in its top 8.
    #[test]
    fn a_wide_channel_gets_the_value_in_its_top_bits() {
        let a2r10g10b10 = rgb(32, 0x3FF0_0000, 0x000F_FC00, 0x3FF, 0xC000_0000);
        assert_eq!(
            to_pixel([0xFF, 0x80, 0x01, 0xFF], &a2r10g10b10),
            0xC000_0000 | 0x3FC << 20 | 0x200 << 10 | 0x004
        );
    }

    /// A pixel read for compression has each channel's bits repeated
    /// to 8, and a channel with a mask of 0 reads as 0.
    #[test]
    fn a_pixel_is_read_with_its_bits_repeated() {
        let a1r5g5b5 = rgb(16, 0x7C00, 0x03E0, 0x001F, 0x8000);
        assert_eq!(from_pixel(0xFC1F, &a1r5g5b5), [255, 0, 255, 255]);
        assert_eq!(from_pixel(0x4210, &a1r5g5b5), [132, 132, 132, 0]);
        let a4r4g4b4 = rgb(16, 0x0F00, 0x00F0, 0x000F, 0xF000);
        assert_eq!(from_pixel(0x8A5F, &a4r4g4b4), [0xAA, 0x55, 0xFF, 0x88]);
        let no_masks = rgb(16, 0, 0, 0, 0);
        assert_eq!(from_pixel(0xFFFF, &no_masks), [0, 0, 0, 255]);
    }
}
