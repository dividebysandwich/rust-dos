//! The current raster position (OpenGL 1.1, section 2.12) and what is
//! drawn at it: glBitmap (section 3.6.4) and glCopyPixels (section
//! 4.3.3).
//!
//! glRasterPos puts a point through the modelview and projection
//! matrices as a vertex goes, and the position is valid if the point is
//! inside the view volume. Its window coordinates come from the viewport
//! and the depth range, unrounded; with it the raster position keeps
//! the point's distance from the eye, the current color (lit with
//! GL_LIGHTING on) and each unit's texture coordinates through the
//! texture matrix. An invalid position keeps the rest as it was, and
//! glBitmap and glCopyPixels then draw nothing.
//!
//! What they draw are fragments that aren't a primitive's
//! (`draw::draw_fragments`): rectangles of pixels drawn through the
//! card's texturing, fog and per-fragment operations, on both paths.

use super::draw::{self, Fragments};
use super::{GL_INVALID_ENUM, GL_INVALID_OPERATION, GL_INVALID_VALUE, UNITS, ctx, error, matrix};
use super::{IDENTITY, texture};
use crate::cpu::Cpu;

/// The W32 section's version that has the raster position, in the
/// contexts and in what glPushAttrib keeps. An older state's contexts
/// load with the initial one, as no glRasterPos had set one there.
pub const RASTER_SAVED: u16 = 98;

// glCopyPixels' types.
const GL_COLOR: u32 = 0x1800;
const GL_DEPTH: u32 = 0x1801;
const GL_STENCIL: u32 = 0x1802;

/// The current raster position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Raster {
    /// x, y and z in window coordinates, and w in clip coordinates.
    pub window: [f32; 4],
    pub valid: bool,
    /// The distance from the eye, for fog.
    pub distance: f32,
    pub color: [f32; 4],
    /// Each unit's texture coordinates.
    pub tex: [[f32; 4]; UNITS],
}

/// The initial raster position: valid, at the window's origin, white,
/// with texture coordinates (0, 0, 0, 1).
impl Default for Raster {
    fn default() -> Raster {
        Raster {
            window: [0.0, 0.0, 0.0, 1.0],
            valid: true,
            distance: 0.0,
            color: [1.0; 4],
            tex: [[0.0, 0.0, 0.0, 1.0]; UNITS],
        }
    }
}

crate::state_fields!(Raster {
    window,
    valid,
    distance,
    color,
    tex
});

/// The values glGet gives of the raster position's state `p`, and
/// whether they are colors; None for another parameter. `unit` is the
/// active texture unit.
pub fn get(r: &Raster, p: u32, unit: usize) -> Option<(Vec<f64>, bool)> {
    let floats = |v: &[f32]| v.iter().map(|&x| x as f64).collect::<Vec<_>>();
    Some(match p {
        // GL_CURRENT_RASTER_COLOR, GL_CURRENT_RASTER_INDEX (1 in RGBA
        // mode, as it starts), GL_CURRENT_RASTER_TEXTURE_COORDS,
        // GL_CURRENT_RASTER_POSITION, GL_CURRENT_RASTER_POSITION_VALID,
        // GL_CURRENT_RASTER_DISTANCE.
        0x0B04 => (floats(&r.color), true),
        0x0B05 => (vec![1.0], false),
        0x0B06 => (floats(&r.tex[unit]), false),
        0x0B07 => (floats(&r.window), false),
        0x0B08 => (vec![r.valid as u32 as f64], false),
        0x0B09 => (vec![r.distance as f64], false),
        _ => return None,
    })
}

/// glRasterPos with the object coordinates `p` (z 0 and w 1 where the
/// call gives none).
pub fn raster_pos(cpu: &mut Cpu, p: [f32; 4]) {
    let Some(c) = ctx(cpu) else {
        return;
    };
    let modelview = c.modelview.last().copied().unwrap_or(IDENTITY);
    let projection = c.projection.last().copied().unwrap_or(IDENTITY);
    let eye = matrix::apply(&modelview, p);
    let [x, y, z, w] = matrix::apply(&projection, eye).map(f64::from);
    // Inside the view volume, -w <= x, y, z <= w; a number that isn't
    // one is outside.
    if ![x, y, z].iter().all(|&v| -w <= v && v <= w) {
        c.raster.valid = false;
        return;
    }
    let [vx, vy, vw, vh] = c.viewport.map(f64::from);
    let [near, far] = c.depth_range.map(f64::from);
    let window = [
        vx + (x / w + 1.0) * vw / 2.0,
        vy + (y / w + 1.0) * vh / 2.0,
        near + (z / w + 1.0) * (far - near) / 2.0,
    ];
    let e = if eye[3] != 0.0 && eye[3] != 1.0 {
        eye.map(|v| v / eye[3])
    } else {
        eye
    };
    let distance = (e[0] * e[0] + e[1] * e[1] + e[2] * e[2]).sqrt();
    let color = draw::lit_color(c, p, c.normal, c.color).unwrap_or(c.color);
    let tex = std::array::from_fn(|u| match c.texture_matrix[u].last() {
        Some(m) if *m != IDENTITY => matrix::apply(m, c.tex_coord[u]),
        _ => c.tex_coord[u],
    });
    *c.raster = Raster {
        window: [
            window[0] as f32,
            window[1] as f32,
            window[2] as f32,
            w as f32,
        ],
        valid: true,
        distance,
        color: color.map(|v| v.clamp(0.0, 1.0)),
        tex,
    };
}

/// The window coordinate a pixel rectangle's first column or row at
/// `at` starts with: that of the pixel whose centre is the first at or
/// after it (section 3.6.5's conversion to fragments, zoom 1).
fn first_pixel(at: f32) -> i32 {
    (at - 0.5).ceil() as i32
}

/// The runs of lit bits of a bitmap's row, as (first, length).
fn runs(row: &[u8], width: u32) -> Vec<(i32, i32)> {
    let lit = |x: u32| row[(x / 8) as usize] & (0x80 >> (x % 8)) != 0;
    let mut out = Vec::new();
    let mut x = 0;
    while x < width {
        if !lit(x) {
            x += 1;
            continue;
        }
        let start = x;
        while x < width && lit(x) {
            x += 1;
        }
        out.push((start as i32, (x - start) as i32));
    }
    out
}

/// glBitmap(width, height, xorig, yorig, xmove, ymove, bitmap): a
/// fragment at each lit bit, the bitmap's lower left corner at the
/// raster position less the origin, rounded down; then the raster
/// position moves by (xmove, ymove). With the raster position invalid
/// it is ignored, the move too.
pub fn bitmap(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (width, height) = (args[0] as i32, args[1] as i32);
    let [xorig, yorig, xmove, ymove] = std::array::from_fn(|i| super::f(args, 2 + i));
    if width < 0 || height < 0 {
        error(cpu, GL_INVALID_VALUE);
        return Some(0);
    }
    let Some(r) = ctx(cpu).map(|c| *c.raster) else {
        return Some(0);
    };
    if !r.valid {
        return Some(0);
    }
    let (width, height) = (width as u32, height as u32);
    let rows = if width > 0 && height > 0 && args[6] != 0 {
        super::lists::bitmap_rows(cpu, args[6], width, height)
    } else {
        None
    };
    if let Some(bits) = rows {
        // Mesa adds the same small amount, so that a position a hair
        // under a whole number, as a transform may leave one, takes
        // that number's pixel.
        const EPSILON: f32 = 1e-4;
        let x = (r.window[0] - xorig + EPSILON).floor() as i32;
        let y = (r.window[1] - yorig + EPSILON).floor() as i32;
        let color = super::super::ddraw::d3d::pipeline::color(r.color);
        let stride = width.div_ceil(8) as usize;
        let rects: Vec<Fragments> = bits
            .chunks_exact(stride)
            .enumerate()
            .flat_map(|(row, line)| {
                runs(line, width)
                    .into_iter()
                    .map(move |(first, len)| Fragments {
                        x: x + first,
                        y: y + row as i32,
                        width: len,
                        height: 1,
                        color,
                        z: r.window[2],
                    })
            })
            .collect();
        draw::draw_fragments(cpu, &rects, &r.tex, r.distance);
    }
    if let Some(c) = ctx(cpu) {
        c.raster.window[0] += xmove;
        c.raster.window[1] += ymove;
    }
    Some(0)
}

/// The runs of equal values in a row of pixels, as (first, length,
/// value).
fn equal_runs<T: Copy + PartialEq>(row: &[T]) -> Vec<(i32, i32, T)> {
    let mut out: Vec<(i32, i32, T)> = Vec::new();
    for (x, &v) in row.iter().enumerate() {
        match out.last_mut() {
            Some((_, len, last)) if *last == v => *len += 1,
            _ => out.push((x as i32, 1, v)),
        }
    }
    out
}

/// glCopyPixels(x, y, width, height, type): the rectangle of the color
/// buffer (GL_COLOR) or the depth buffer (GL_DEPTH) at (x, y) drawn
/// again at the raster position, as glDrawPixels would draw what
/// glReadPixels read: a color copy's fragments have the pixels' colors
/// and the raster position's depth, a depth copy's the depths and the
/// raster color. Both have the raster position's texture coordinates.
/// The rectangle is read whole before any of it is drawn. Pixels off
/// the buffers aren't drawn (their values are undefined). There is no
/// stencil buffer, so GL_STENCIL is GL_INVALID_OPERATION.
///
/// Neither the pixel transfer operations (#1382) nor the pixel zoom
/// (#1053) are there: a copy is drawn with a scale of 1 and a bias of
/// 0, at a zoom of 1.
pub fn copy_pixels(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [x, y, width, height, kind] = std::array::from_fn(|i| args[i] as i32);
    let kind = kind as u32;
    if width < 0 || height < 0 {
        error(cpu, GL_INVALID_VALUE);
        return Some(0);
    }
    let Some(c) = ctx(cpu) else {
        return Some(0);
    };
    let e = match kind {
        GL_COLOR => None,
        GL_DEPTH if c.depth == 0 => Some(GL_INVALID_OPERATION),
        GL_DEPTH => None,
        GL_STENCIL => Some(GL_INVALID_OPERATION),
        _ => Some(GL_INVALID_ENUM),
    };
    if let Some(e) = e {
        error(cpu, e);
        return Some(0);
    }
    let r = *c.raster;
    let (buffer_w, buffer_h) = (c.width as i32, c.height as i32);
    if !r.valid || width == 0 || height == 0 {
        return Some(0);
    }
    // The part of the rectangle on the buffers.
    let (left, bottom) = (x.max(0), y.max(0));
    let (right, top) = (
        x.saturating_add(width).min(buffer_w),
        y.saturating_add(height).min(buffer_h),
    );
    if right <= left || top <= bottom {
        return Some(0);
    }
    let (w, h) = ((right - left) as u32, (top - bottom) as u32);
    // Where the rectangle's first pixel goes.
    let to_x = first_pixel(r.window[0]) + (left - x);
    let to_y = first_pixel(r.window[1]) + (bottom - y);
    draw::flush_waiting(cpu);
    let mut rects = Vec::new();
    let mut add = |row: u32, first: i32, len: i32, color: u32, z: f32| {
        rects.push(Fragments {
            x: to_x + first,
            y: to_y + row as i32,
            width: len,
            height: 1,
            color,
            z,
        });
    };
    if kind == GL_COLOR {
        let Some(pixels) = texture::read_frame(cpu, left, bottom, w, h) else {
            return Some(0);
        };
        for (row, line) in pixels.chunks_exact(w as usize).enumerate() {
            for (first, len, [red, g, b, _]) in equal_runs(line) {
                // The color buffer has no alpha: its pixels' is 1.
                let color = u32::from_be_bytes([0xFF, red, g, b]);
                add(row as u32, first, len, color, r.window[2]);
            }
        }
    } else {
        let Some(depths) = texture::read_depth(cpu, left, bottom, w, h) else {
            return Some(0);
        };
        let color = super::super::ddraw::d3d::pipeline::color(r.color);
        for (row, line) in depths.chunks_exact(w as usize).enumerate() {
            for (first, len, z) in equal_runs(line) {
                add(row as u32, first, len, color, f32::from(z) / 65535.0);
            }
        }
    }
    draw::draw_fragments(cpu, &rects, &r.tex, r.distance);
    Some(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bitmaps_rows_are_runs_of_lit_bits() {
        assert_eq!(runs(&[0b1100_0111, 0b1000_0000], 9), [(0, 2), (5, 4)]);
        assert_eq!(runs(&[0xFF], 3), [(0, 3)]);
        assert!(runs(&[0], 8).is_empty());
    }

    #[test]
    fn a_rows_equal_pixels_are_one_run() {
        assert_eq!(
            equal_runs(&[1, 1, 2, 2, 2, 1]),
            [(0, 2, 1), (2, 3, 2), (5, 1, 1)]
        );
    }

    /// A pixel rectangle's pixel takes the window's pixel whose centre
    /// it covers: from 10 to 10.5 that is the tenth, past it the next.
    #[test]
    fn a_rectangle_starts_at_the_first_centre_it_covers() {
        assert_eq!(first_pixel(10.0), 10);
        assert_eq!(first_pixel(10.5), 10);
        assert_eq!(first_pixel(10.6), 11);
        assert_eq!(first_pixel(-0.25), 0);
    }
}
