//! GLU's image functions: gluScaleImage, and gluBuild1DMipmaps and
//! gluBuild2DMipmaps, which scale an image to a power of two no larger
//! than the largest texture and give OpenGL it and each level below it
//! down to one texel.
//!
//! As in the SGI sample implementation that Microsoft's GLU 1.2 is, an
//! image is read into 16-bit components (`read`), with OpenGL's unpack
//! state (alignment, row length, rows and pixels skipped), scaled there
//! with a box filter that wraps around the edges (`scale`, or `halve`
//! for exactly half in both directions), and written back in the type
//! asked for with the pack state (`write`). Each mipmap level is scaled
//! from the one before it. The byte order and bit order of the pixel
//! store aren't kept by the built-in OPENGL32, and are the defaults
//! here.

use super::super::opengl32::{self as gl, MAX_TEXTURE_SIZE};
use super::{GLU_INVALID_ENUM, GLU_INVALID_VALUE, GLU_OUT_OF_MEMORY, log_once};
use crate::cpu::Cpu;

// Formats.
const GL_COLOR_INDEX: u32 = 0x1900;
const GL_STENCIL_INDEX: u32 = 0x1901;
const GL_DEPTH_COMPONENT: u32 = 0x1902;
const GL_RGB: u32 = 0x1907;
const GL_RGBA: u32 = 0x1908;
const GL_LUMINANCE_ALPHA: u32 = 0x190A;
const GL_BGR_EXT: u32 = 0x80E0;
const GL_BGRA_EXT: u32 = 0x80E1;

// Types.
const GL_BYTE: u32 = 0x1400;
const GL_UNSIGNED_BYTE: u32 = 0x1401;
const GL_SHORT: u32 = 0x1402;
const GL_UNSIGNED_SHORT: u32 = 0x1403;
const GL_INT: u32 = 0x1404;
const GL_UNSIGNED_INT: u32 = 0x1405;
const GL_FLOAT: u32 = 0x1406;
const GL_BITMAP: u32 = 0x1A00;

/// Whether GLU takes `format`: the formats from GL_COLOR_INDEX to
/// GL_LUMINANCE_ALPHA, and GL_BGR_EXT and GL_BGRA_EXT.
fn legal_format(format: u32) -> bool {
    matches!(
        format,
        GL_COLOR_INDEX..=GL_LUMINANCE_ALPHA | GL_BGR_EXT | GL_BGRA_EXT
    )
}

/// Whether GLU takes `kind`: GL_BYTE to GL_FLOAT, and GL_BITMAP.
fn legal_type(kind: u32) -> bool {
    matches!(kind, GL_BYTE..=GL_FLOAT | GL_BITMAP)
}

/// The components of a pixel of `format`.
pub fn elements(format: u32) -> u32 {
    match format {
        GL_LUMINANCE_ALPHA => 2,
        GL_RGB | GL_BGR_EXT => 3,
        GL_RGBA | GL_BGRA_EXT => 4,
        _ => 1,
    }
}

/// Whether the components of `format` are indices, which keep their
/// values instead of being taken to the range of 16 bits.
fn is_index(format: u32) -> bool {
    matches!(format, GL_COLOR_INDEX | GL_STENCIL_INDEX)
}

/// The most components an image GLU scales may have. Windows' GLU
/// allocates two bytes for each component of the image it reads and of
/// the one it makes, and returns GLU_OUT_OF_MEMORY when it can't.
const MAX_COMPONENTS: u64 = 0x400_0000;

/// Whether an image of `w` by `h` of `format` has more components than
/// GLU allocates (`MAX_COMPONENTS`).
fn too_big(w: u32, h: u32, format: u32) -> bool {
    u64::from(w) * u64::from(h) * u64::from(elements(format)) > MAX_COMPONENTS
}

/// The bytes of a component of `kind` (GL_BITMAP has bits).
fn type_size(kind: u32) -> u32 {
    match kind {
        GL_BYTE | GL_UNSIGNED_BYTE | GL_BITMAP => 1,
        GL_SHORT | GL_UNSIGNED_SHORT => 2,
        _ => 4,
    }
}

/// Where an image's pixels are, from its address: the first's byte
/// and, for GL_BITMAP, its bit; the bytes from a row to the next; and
/// the bytes from the first to the end of the last.
#[derive(Debug, PartialEq)]
struct Layout {
    start: u32,
    bit: u32,
    row: u32,
    len: u32,
}

/// The layout of an image of `w` by `h` of `format` and `kind`, with
/// the pixel store `store` (alignment, row length, rows and pixels
/// skipped). Rows are padded to the alignment, as in the sample
/// implementation.
fn layout(store: [u32; 4], w: u32, h: u32, format: u32, kind: u32) -> Option<Layout> {
    let [align, row_length, skip_rows, skip_pixels] = store;
    let n = elements(format);
    let per_line = if row_length > 0 { row_length } else { w };
    let pad = |bytes: u32| bytes.checked_next_multiple_of(align.max(1));
    if kind == GL_BITMAP {
        let row = pad(per_line.checked_mul(n)?.div_ceil(8))?;
        let first = skip_pixels.checked_mul(n)?;
        let start = skip_rows.checked_mul(row)?.checked_add(first / 8)?;
        let bit = first % 8;
        let last = (bit + w.checked_mul(n)?).div_ceil(8);
        let len = start
            .checked_add(h.saturating_sub(1).checked_mul(row)?)?
            .checked_add(last)?;
        return Some(Layout {
            start,
            bit,
            row,
            len,
        });
    }
    let group = n * type_size(kind);
    let row = pad(per_line.checked_mul(group)?)?;
    let start = skip_rows
        .checked_mul(row)?
        .checked_add(skip_pixels.checked_mul(group)?)?;
    let len = start
        .checked_add(h.saturating_sub(1).checked_mul(row)?)?
        .checked_add(w.checked_mul(group)?)?;
    Some(Layout {
        start,
        bit: 0,
        row,
        len,
    })
}

/// A component of `kind` from its bytes, as 16 bits; an index as it
/// is.
fn to_16(b: &[u8], kind: u32, index: bool) -> u16 {
    let u = |n: usize| -> u32 {
        let mut v = [0u8; 4];
        v[..n].copy_from_slice(&b[..n]);
        u32::from_le_bytes(v)
    };
    match kind {
        GL_UNSIGNED_BYTE if index => b[0] as u16,
        GL_UNSIGNED_BYTE => b[0] as u16 * 257,
        GL_BYTE if index => b[0] as i8 as u16,
        GL_BYTE => (b[0] as i8).max(0) as u16 * 516,
        GL_UNSIGNED_SHORT => u(2) as u16,
        GL_SHORT if index => u(2) as u16,
        GL_SHORT => (u(2) as u16 as i16).max(0) as u16 * 2,
        GL_UNSIGNED_INT if index => u(4) as u16,
        GL_UNSIGNED_INT => (u(4) >> 16) as u16,
        GL_INT if index => u(4) as u16,
        GL_INT => ((u(4) as i32).max(0) >> 15) as u16,
        _ => {
            let f = f32::from_bits(u(4));
            if index {
                f as i32 as u16
            } else {
                (f * 65535.0).clamp(0.0, 65535.0) as u16
            }
        }
    }
}

/// A 16-bit component as `kind`'s bytes.
fn from_16(v: u16, kind: u32, index: bool) -> Vec<u8> {
    let v32 = u32::from(v);
    match kind {
        GL_UNSIGNED_BYTE | GL_BYTE if index => vec![v as u8],
        GL_UNSIGNED_BYTE => vec![(v >> 8) as u8],
        GL_BYTE => vec![(v / 516) as u8],
        GL_UNSIGNED_SHORT => v.to_le_bytes().to_vec(),
        GL_SHORT if index => v.to_le_bytes().to_vec(),
        GL_SHORT => (v / 2).to_le_bytes().to_vec(),
        GL_UNSIGNED_INT | GL_INT if index => v32.to_le_bytes().to_vec(),
        GL_UNSIGNED_INT => (v32 * 65537).to_le_bytes().to_vec(),
        GL_INT => (v32 * 32768).to_le_bytes().to_vec(),
        _ if index => (v as f32).to_le_bytes().to_vec(),
        _ => (v as f32 / 65535.0).to_le_bytes().to_vec(),
    }
}

/// The components of an image whose bytes from its first pixel's are
/// `bytes`, laid out as `l`, as 16 bits each, row by row.
fn decode(bytes: &[u8], l: &Layout, w: u32, h: u32, format: u32, kind: u32) -> Vec<u16> {
    let n = elements(format);
    let index = is_index(format);
    let mut out = Vec::with_capacity((w * h * n) as usize);
    for y in 0..h {
        let row = (y * l.row) as usize;
        if kind == GL_BITMAP {
            for i in 0..w * n {
                let bit = l.bit + i;
                let byte = bytes[row + (bit / 8) as usize];
                let set = byte & (0x80 >> (bit % 8)) != 0;
                out.push(match (set, index) {
                    (false, _) => 0,
                    (true, true) => 1,
                    (true, false) => 0xFFFF,
                });
            }
            continue;
        }
        let size = type_size(kind) as usize;
        for i in 0..(w * n) as usize {
            out.push(to_16(&bytes[row + i * size..], kind, index));
        }
    }
    out
}

/// Write the components `data` into `bytes`, laid out as `l`: the
/// other bytes, a row's padding, stay.
fn encode(bytes: &mut [u8], l: &Layout, w: u32, h: u32, format: u32, kind: u32, data: &[u16]) {
    let n = elements(format);
    let index = is_index(format);
    let mut src = data.iter();
    for y in 0..h {
        let row = (y * l.row) as usize;
        if kind == GL_BITMAP {
            for i in 0..w * n {
                let v = *src.next().unwrap_or(&0);
                let set = if index { v & 1 != 0 } else { v > 32767 };
                let bit = l.bit + i;
                let at = row + (bit / 8) as usize;
                let mask = 0x80 >> (bit % 8);
                bytes[at] = if set {
                    bytes[at] | mask
                } else {
                    bytes[at] & !mask
                };
            }
            continue;
        }
        let size = type_size(kind) as usize;
        for i in 0..(w * n) as usize {
            let v = *src.next().unwrap_or(&0);
            let at = row + i * size;
            bytes[at..at + size].copy_from_slice(&from_16(v, kind, index));
        }
    }
}

/// The components of the program's image at `at`, None where it can't
/// be read.
fn read(cpu: &mut Cpu, store: [u32; 4], img: (u32, u32, u32, u32), at: u32) -> Option<Vec<u16>> {
    let (w, h, format, kind) = img;
    let l = layout(store, w, h, format, kind)?;
    let len = l.len - l.start;
    let start = at.checked_add(l.start)?;
    if at == 0 || len > 0x400_0000 || !cpu.bus.guest_probe(start, len as usize, false) {
        return None;
    }
    let mut bytes = vec![0u8; len as usize];
    cpu.bus.guest_read_bytes(start, &mut bytes);
    Some(decode(&bytes, &l, w, h, format, kind))
}

/// Write `data` as the program's image at `at`; false where it can't
/// be written.
fn write(cpu: &mut Cpu, store: [u32; 4], img: (u32, u32, u32, u32), at: u32, data: &[u16]) -> bool {
    let (w, h, format, kind) = img;
    let Some(l) = layout(store, w, h, format, kind) else {
        return false;
    };
    let len = l.len - l.start;
    let Some(start) = at.checked_add(l.start) else {
        return false;
    };
    if at == 0 || len > 0x400_0000 || !cpu.bus.guest_probe(start, len as usize, true) {
        return false;
    }
    let mut bytes = vec![0u8; len as usize];
    cpu.bus.guest_read_bytes(start, &mut bytes);
    encode(&mut bytes, &l, w, h, format, kind, data);
    cpu.bus.guest_write_bytes(start, &bytes);
    true
}

/// The power of two nearest `value`, as the sample implementation's
/// nearestPower has it: a value from 1.5 times a power of two up goes
/// to the next one.
pub fn nearest_power(mut value: u32) -> u32 {
    let mut i = 1;
    loop {
        match value {
            0 | 1 => return i,
            3 => return i * 4,
            _ => {
                value >>= 1;
                i *= 2;
            }
        }
    }
}

/// An image of `w` by `h` of `n` components a pixel at half its size
/// in both directions, each pixel the rounded average of four.
fn halve(n: usize, w: usize, h: usize, data: &[u16]) -> Vec<u16> {
    let (nw, nh) = (w / 2, h / 2);
    let mut out = Vec::with_capacity(nw * nh * n);
    for y in 0..nh {
        for x in 0..nw {
            for k in 0..n {
                let at = |xx: usize, yy: usize| u32::from(data[(yy * w + xx) * n + k]);
                let sum = at(2 * x, 2 * y)
                    + at(2 * x + 1, 2 * y)
                    + at(2 * x, 2 * y + 1)
                    + at(2 * x + 1, 2 * y + 1);
                out.push(((sum + 2) / 4) as u16);
            }
        }
    }
    out
}

/// The span of source pixels an output pixel at `i` covers, scaling
/// `size_in` to `size_out`: a box of the ratio's width when shrinking,
/// one pixel's when growing.
fn span(i: usize, size_in: usize, size_out: usize) -> (f32, f32) {
    let conv = size_in as f32 / size_out as f32;
    let center = conv * (i as f32 + 0.5);
    let half = if size_in > size_out { conv / 2.0 } else { 0.5 };
    (center - half, center + half)
}

/// The image `data` of `win` by `hin`, `n` components a pixel, scaled
/// to `wout` by `hout` as the sample implementation's scale_internal
/// does: each output pixel the average of the source area its box
/// covers, by how much of each pixel it covers, the box wrapping around
/// the edges.
pub fn scale(n: usize, win: usize, hin: usize, data: &[u16], wout: usize, hout: usize) -> Vec<u16> {
    if win == wout * 2 && hin == hout * 2 {
        return halve(n, win, hin, data);
    }
    let mut out = Vec::with_capacity(wout * hout * n);
    let mut totals = vec![0f32; n];
    for i in 0..hout {
        let (lowy, highy) = span(i, hin, hout);
        for j in 0..wout {
            let (lowx, highx) = span(j, win, wout);
            totals.fill(0.0);
            let mut area = 0f32;
            let mut y = lowy;
            let mut yint = y.floor();
            while y < highy {
                let yindex = (yint as i64).rem_euclid(hin as i64) as usize;
                let ypercent = if highy < yint + 1.0 {
                    highy - y
                } else {
                    yint + 1.0 - y
                };
                let mut x = lowx;
                let mut xint = x.floor();
                while x < highx {
                    let xindex = (xint as i64).rem_euclid(win as i64) as usize;
                    let xpercent = if highx < xint + 1.0 {
                        highx - x
                    } else {
                        xint + 1.0 - x
                    };
                    let percent = xpercent * ypercent;
                    area += percent;
                    let at = (yindex * win + xindex) * n;
                    for (k, t) in totals.iter_mut().enumerate() {
                        *t += f32::from(data[at + k]) * percent;
                    }
                    xint += 1.0;
                    x = xint;
                }
                yint += 1.0;
                y = yint;
            }
            for t in &totals {
                out.push((t / area + 0.5) as u16);
            }
        }
    }
    out
}

/// The image `data` of `format` as red, green, blue and alpha bytes for
/// OpenGL: a component's 16 bits to 8, an index as it is up to 255.
fn texels(data: &[u16], format: u32) -> Vec<[u8; 4]> {
    let n = elements(format) as usize;
    let index = is_index(format);
    data.chunks_exact(n)
        .map(|p| {
            let byte = |i: u32| {
                let v = u32::from(p[i as usize]);
                if index {
                    v.min(255) as u8
                } else {
                    ((v * 255 + 32767) / 65535) as u8
                }
            };
            gl::rgba_of(format, byte)
        })
        .collect()
}

/// The checks gluBuild1DMipmaps and gluBuild2DMipmaps make of an
/// image's size, format and type: the error to return, if any.
fn check(w: i32, h: i32, format: u32, kind: u32) -> Option<u32> {
    if w < 1 || h < 1 {
        return Some(GLU_INVALID_VALUE);
    }
    if !legal_format(format) || !legal_type(kind) {
        return Some(GLU_INVALID_ENUM);
    }
    if matches!(format, GL_STENCIL_INDEX | GL_DEPTH_COMPONENT) {
        return Some(GLU_INVALID_ENUM);
    }
    None
}

/// The levels gluBuild2DMipmaps gives OpenGL for an image of `w` by
/// `h`: from the nearest power of two in each direction, no larger
/// than `max`, halved down to 1 by 1, each direction staying at 1 once
/// it is there.
pub fn level_sizes(w: u32, h: u32, max: u32) -> Vec<(u32, u32)> {
    let (mut nw, mut nh) = (nearest_power(w).min(max), nearest_power(h).min(max));
    let levels = nw.max(nh).trailing_zeros();
    let mut out = Vec::new();
    for _ in 0..=levels {
        out.push((nw, nh));
        nw = (nw / 2).max(1);
        nh = (nh / 2).max(1);
    }
    out
}

/// gluBuild2DMipmaps(target, components, width, height, format, type,
/// data): 0, or a GLU error.
pub fn build_2d_mipmaps(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [target, components, width, height, format, kind, at] = args[..7] else {
        return Some(0);
    };
    if let Some(e) = check(width as i32, height as i32, format, kind) {
        return Some(e);
    }
    if too_big(width, height, format) {
        return Some(GLU_OUT_OF_MEMORY);
    }
    let Some(store) = gl::ctx(cpu).map(|c| c.unpack) else {
        // Without a context OpenGL's functions do nothing.
        return Some(0);
    };
    let Some(mut data) = read(cpu, store, (width, height, format, kind), at) else {
        return Some(GLU_INVALID_VALUE);
    };
    if target != gl::GL_TEXTURE_2D {
        // glTexImage2D's error, for each level.
        gl::error(cpu, gl::GL_INVALID_ENUM);
        return Some(0);
    }
    let n = elements(format) as usize;
    let (mut w, mut h) = (width, height);
    for (level, (nw, nh)) in level_sizes(width, height, MAX_TEXTURE_SIZE)
        .into_iter()
        .enumerate()
    {
        if (w, h) != (nw, nh) {
            data = scale(n, w as usize, h as usize, &data, nw as usize, nh as usize);
            (w, h) = (nw, nh);
        }
        gl::tex_image_rgba(cpu, level as u32, components, w, h, texels(&data, format));
    }
    Some(0)
}

/// gluBuild1DMipmaps(target, components, width, format, type, data): 0,
/// or a GLU error. OPENGL32 has no 1D textures, so the levels go
/// nowhere, and the first call says so in the log.
pub fn build_1d_mipmaps(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [_, _, width, format, kind, _] = args[..6] else {
        return Some(0);
    };
    if let Some(e) = check(width as i32, 1, format, kind) {
        return Some(e);
    }
    log_once(
        cpu,
        u16::MAX,
        "gluBuild1DMipmaps: OPENGL32 has no 1D textures, the levels are dropped",
    );
    Some(0)
}

/// gluScaleImage(format, widthin, heightin, typein, datain, widthout,
/// heightout, typeout, dataout): 0, or a GLU error. An image of no
/// size scales to nothing.
pub fn scale_image(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [
        format,
        win,
        hin,
        kind_in,
        at_in,
        wout,
        hout,
        kind_out,
        at_out,
    ] = args[..9]
    else {
        return Some(0);
    };
    let sizes = [win, hin, wout, hout].map(|v| v as i32);
    if sizes.contains(&0) {
        return Some(0);
    }
    if sizes.iter().any(|&v| v < 0) {
        return Some(GLU_INVALID_VALUE);
    }
    if !legal_format(format) || !legal_type(kind_in) || !legal_type(kind_out) {
        return Some(GLU_INVALID_ENUM);
    }
    if too_big(win, hin, format) || too_big(wout, hout, format) {
        return Some(GLU_OUT_OF_MEMORY);
    }
    let (unpack, pack) = gl::ctx(cpu).map_or(([4, 0, 0, 0], [4, 0, 0, 0]), |c| (c.unpack, c.pack));
    let Some(data) = read(cpu, unpack, (win, hin, format, kind_in), at_in) else {
        return Some(GLU_INVALID_VALUE);
    };
    let n = elements(format) as usize;
    let out = scale(
        n,
        win as usize,
        hin as usize,
        &data,
        wout as usize,
        hout as usize,
    );
    if !write(cpu, pack, (wout, hout, format, kind_out), at_out, &out) {
        return Some(GLU_INVALID_VALUE);
    }
    Some(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GL_LUMINANCE: u32 = 0x1909;

    #[test]
    fn sizes_go_to_the_nearest_power_of_two() {
        let got: Vec<u32> = [1, 2, 3, 5, 6, 7, 12, 100, 192, 640]
            .map(nearest_power)
            .to_vec();
        assert_eq!(got, [1, 2, 4, 4, 8, 8, 16, 128, 256, 512]);
    }

    #[test]
    fn levels_shrink_to_one_texel_within_the_largest_texture() {
        assert_eq!(
            level_sizes(640, 100, 256),
            [
                (256, 128),
                (128, 64),
                (64, 32),
                (32, 16),
                (16, 8),
                (8, 4),
                (4, 2),
                (2, 1),
                (1, 1)
            ]
        );
        assert_eq!(level_sizes(1, 1, 256), [(1, 1)]);
    }

    #[test]
    fn halving_averages_four_pixels_rounded() {
        // A 2x2 grey image: 0, 1, 2, 4 average to 7 / 4, rounded to 2.
        assert_eq!(scale(1, 2, 2, &[0, 1, 2, 4], 1, 1), [2]);
        // Two components, each on its own.
        let rg = [10, 100, 20, 100, 30, 100, 40, 100];
        assert_eq!(scale(2, 2, 2, &rg, 1, 1), [25, 100]);
    }

    #[test]
    fn a_row_of_one_halves_along_its_length() {
        assert_eq!(scale(1, 4, 1, &[0, 100, 200, 300], 2, 1), [50, 250]);
    }

    #[test]
    fn growing_takes_the_pixel_under_each_center_and_wraps_at_edges() {
        // Two pixels to four: each output's one-pixel box straddles two
        // source pixels, the outer ones wrapping to the other side.
        let out = scale(1, 2, 1, &[0, 1000], 4, 1);
        assert_eq!(out, [250, 250, 750, 750]);
    }

    #[test]
    fn shrinking_by_three_halves_weighs_partial_pixels() {
        // Three to two: each output covers 1.5 pixels.
        let out = scale(1, 3, 1, &[0, 300, 600], 2, 1);
        assert_eq!(out, [100, 500]);
    }

    #[test]
    fn components_go_to_16_bits_and_back() {
        assert_eq!(to_16(&[255], GL_UNSIGNED_BYTE, false), 65535);
        assert_eq!(to_16(&[1], GL_UNSIGNED_BYTE, false), 257);
        assert_eq!(from_16(65535, GL_UNSIGNED_BYTE, false), [255]);
        assert_eq!(to_16(&1.0f32.to_le_bytes(), GL_FLOAT, false), 65535);
        assert_eq!(to_16(&[0x80], GL_BYTE, false), 0, "negative clamps");
        assert_eq!(to_16(&[7], GL_UNSIGNED_BYTE, true), 7, "an index stays");
    }

    #[test]
    fn rows_are_padded_to_the_alignment_and_skips_count() {
        // 3 RGB bytes a pixel, 5 pixels: 15 bytes padded to 16.
        let l = layout([4, 0, 0, 0], 5, 2, GL_RGB, GL_UNSIGNED_BYTE).unwrap();
        assert_eq!(
            l,
            Layout {
                start: 0,
                bit: 0,
                row: 16,
                len: 31
            }
        );
        // A row length of 8 pixels, a row and two pixels skipped.
        let l = layout([1, 8, 1, 2], 4, 2, GL_LUMINANCE, GL_UNSIGNED_BYTE).unwrap();
        assert_eq!(
            l,
            Layout {
                start: 10,
                bit: 0,
                row: 8,
                len: 22
            }
        );
        // A bitmap of 10 bits a row is two bytes.
        let l = layout([1, 0, 0, 3], 10, 1, GL_COLOR_INDEX, GL_BITMAP).unwrap();
        assert_eq!(
            l,
            Layout {
                start: 0,
                bit: 3,
                row: 2,
                len: 2
            }
        );
    }

    #[test]
    fn decoding_and_encoding_keep_the_padding() {
        let l = layout([4, 0, 0, 0], 1, 2, GL_RGB, GL_UNSIGNED_BYTE).unwrap();
        let bytes = [1, 2, 3, 0xAA, 4, 5, 6];
        let data = decode(&bytes, &l, 1, 2, GL_RGB, GL_UNSIGNED_BYTE);
        assert_eq!(data, [257, 514, 771, 1028, 1285, 1542]);
        let mut out = [0xFFu8; 7];
        encode(&mut out, &l, 1, 2, GL_RGB, GL_UNSIGNED_BYTE, &data);
        assert_eq!(out, [1, 2, 3, 0xFF, 4, 5, 6]);
    }

    #[test]
    fn texels_take_the_components_of_their_format() {
        assert_eq!(texels(&[65535, 0, 0], GL_RGB), [[255, 0, 0, 255]]);
        assert_eq!(
            texels(&[0, 0, 65535, 32768], GL_BGRA_EXT),
            [[255, 0, 0, 128]]
        );
        assert_eq!(texels(&[32768], GL_LUMINANCE), [[128, 128, 128, 255]]);
    }

    #[test]
    fn the_checks_are_windows_order() {
        assert_eq!(check(0, 4, 0, 0), Some(GLU_INVALID_VALUE));
        assert_eq!(
            check(4, 4, 0x1234, GL_UNSIGNED_BYTE),
            Some(GLU_INVALID_ENUM)
        );
        assert_eq!(check(4, 4, GL_RGB, 0x1234), Some(GLU_INVALID_ENUM));
        assert_eq!(
            check(4, 4, GL_DEPTH_COMPONENT, GL_UNSIGNED_BYTE),
            Some(GLU_INVALID_ENUM)
        );
        assert_eq!(check(4, 4, GL_BGRA_EXT, GL_BITMAP), None);
    }

    #[test]
    fn an_image_too_big_to_allocate_is_refused_before_it_is_read() {
        assert!(!too_big(4096, 4096, GL_RGBA));
        assert!(too_big(4097, 4096, GL_RGBA));
        // A row length shorter than the width keeps the bytes few, but
        // not the components.
        assert!(too_big(65536, 65536, GL_LUMINANCE));
    }
}
