//! DirectDraw's objects as other built-in DLLs use them: amstream's
//! DirectDraw media stream (dshow/stream.rs) makes its samples' surfaces
//! here, offscreen plain surfaces in the display's format, and draws each
//! picture of the movie into one, converted to the surface's format.

use super::directdraw::{self, DDSCL_NORMAL};
use super::surface::{
    self, DDSCAPS_OFFSCREENPLAIN, DDSD_CAPS, DDSD_HEIGHT, DDSD_PIXELFORMAT, DDSD_WIDTH, Desc,
};
use super::{DDERR_INVALIDPARAMS, DDERR_SURFACEBUSY, DDERR_SURFACELOST, Format, V_DD1, state};
use crate::cpu::Cpu;
use std::collections::HashMap;

/// A new DirectDraw object of the display, as DirectDrawCreate(NULL)
/// makes it: its IDirectDraw, with one reference. None when the heap is
/// full.
pub fn create_ddraw(cpu: &mut Cpu) -> Option<u32> {
    let id = directdraw::new_ddraw(cpu, true, false);
    let at = super::iface(cpu, id, V_DD1);
    if at == 0 {
        super::destroy(cpu, id);
        return None;
    }
    super::add_ref(cpu, at);
    Some(at)
}

/// The IDirectDraw of the DirectDraw object that `this`, any of its
/// interfaces, is, with a reference taken: what a stream keeps of the
/// object the program gave it.
pub fn ddraw_interface(cpu: &mut Cpu, this: u32) -> Result<u32, u32> {
    let (_, id) = directdraw::ddraw_of(cpu, this)?;
    let at = super::iface(cpu, id, V_DD1);
    if at == 0 {
        return Err(crate::win32::com::E_OUTOFMEMORY);
    }
    super::add_ref(cpu, at);
    Ok(at)
}

/// Whether `this` is an interface of a DirectDraw object.
pub fn is_ddraw(cpu: &mut Cpu, this: u32) -> bool {
    directdraw::ddraw_of(cpu, this).is_ok()
}

/// The pixel format of the display, which surfaces made without one
/// have.
pub fn display_format(cpu: &mut Cpu) -> Format {
    surface::rgb_format(super::display::current(cpu).2)
}

/// Write a DDSURFACEDESC (or DDSURFACEDESC2, by its dwSize) of an
/// offscreen plain surface `width` by `height` in `format` at `at`.
pub fn write_desc(
    cpu: &mut Cpu,
    at: u32,
    width: u32,
    height: u32,
    format: &Format,
) -> Result<(), u32> {
    let size = surface::desc_size(cpu, at, 1)?;
    let desc = Desc {
        flags: DDSD_CAPS | DDSD_WIDTH | DDSD_HEIGHT | DDSD_PIXELFORMAT,
        width,
        height,
        format: *format,
        caps: [DDSCAPS_OFFSCREENPLAIN, 0, 0, 0],
        ..Desc::default()
    };
    desc.write(cpu, at, size);
    Ok(())
}

/// The width and height a DDSURFACEDESC at `at` gives (0 for one it
/// doesn't), and its pixel format, if it gives one.
pub fn read_desc(cpu: &mut Cpu, at: u32) -> Result<(u32, u32, Option<Format>), u32> {
    let size = surface::desc_size(cpu, at, 1)?;
    let d = Desc::read(cpu, at, size);
    let w = if d.flags & DDSD_WIDTH != 0 {
        d.width
    } else {
        0
    };
    let h = if d.flags & DDSD_HEIGHT != 0 {
        d.height
    } else {
        0
    };
    let f = (d.flags & DDSD_PIXELFORMAT != 0).then_some(d.format);
    Ok((w, h, f))
}

/// An offscreen plain surface `width` by `height` in `format`, or the
/// display's, of the DirectDraw object of the interface `ddraw`, as
/// IDirectDraw::CreateSurface makes it: its IDirectDrawSurface, with one
/// reference.
pub fn offscreen_surface(
    cpu: &mut Cpu,
    ddraw: u32,
    (width, height): (u32, u32),
    format: Option<Format>,
) -> Result<u32, u32> {
    let (_, dd) = directdraw::ddraw_of(cpu, ddraw)?;
    let mut desc = Desc {
        flags: DDSD_CAPS | DDSD_WIDTH | DDSD_HEIGHT,
        width,
        height,
        caps: [DDSCAPS_OFFSCREENPLAIN, 0, 0, 0],
        ..Desc::default()
    };
    if let Some(f) = format {
        desc.flags |= DDSD_PIXELFORMAT;
        desc.format = f;
    }
    // The level matters only to primary surfaces and flipping chains.
    let ids = surface::make(cpu, dd, 1, DDSCL_NORMAL, &desc)?;
    let root = ids[0];
    let at = super::iface(cpu, root, surface::vtable_of(1));
    if at == 0 {
        super::destroy(cpu, root);
        return Err(crate::win32::com::E_OUTOFMEMORY);
    }
    super::add_ref(cpu, at);
    Ok(at)
}

/// Whether `this` is an interface of a surface.
pub fn is_surface(cpu: &mut Cpu, this: u32) -> bool {
    surface::surface_of(cpu, this).is_ok()
}

/// The width and height of the surface `this`.
pub fn surface_size(cpu: &mut Cpu, this: u32) -> Option<(u32, u32)> {
    let (_, id) = surface::surface_of(cpu, this).ok()?;
    let s = state(cpu).surface(id)?;
    Some((s.width, s.height))
}

/// Draw a picture into the surface `this` with its top left corner at
/// `at`, clipped to the surface: `rows` from the top, of 24-bit pixels in
/// a DIB's order (blue, green, red), `size` wide and high and `stride`
/// bytes apart. An 8-bit surface takes the nearest colours of its
/// palette, or else of the primary surface's of its DirectDraw object.
pub fn put_picture(
    cpu: &mut Cpu,
    this: u32,
    at: (u32, u32),
    rows: &[u8],
    size: (u32, u32),
    stride: usize,
) -> Result<(), u32> {
    let (_, id) = surface::surface_of(cpu, this)?;
    let s = state(cpu).surface(id).cloned().ok_or(DDERR_INVALIDPARAMS)?;
    if s.lost {
        return Err(DDERR_SURFACELOST);
    }
    if s.locked {
        return Err(DDERR_SURFACEBUSY);
    }
    if s.compressed() || !matches!(s.format.bits, 8 | 15 | 16 | 24 | 32) {
        return Err(DDERR_INVALIDPARAMS);
    }
    let (x, y) = at;
    let w = size.0.min(s.width.saturating_sub(x)) as usize;
    let h = size.1.min(s.height.saturating_sub(y)) as usize;
    if w == 0 || h == 0 {
        return Ok(());
    }
    let palette = if s.format.bits == 8 {
        Some(palette_for(cpu, id, &s))
    } else {
        None
    };
    let bpp = s.bytes_per_pixel() as usize;
    let mut out = vec![0u8; w * bpp * h];
    let mut nearest: HashMap<u32, u8> = HashMap::new();
    for (row, dst) in out.chunks_exact_mut(w * bpp).enumerate() {
        let Some(src) = rows.get(row * stride..row * stride + w * 3) else {
            break;
        };
        for (px, d) in src.as_chunks::<3>().0.iter().zip(dst.chunks_exact_mut(bpp)) {
            let (b, g, r) = (px[0], px[1], px[2]);
            match &palette {
                Some(colors) => {
                    let key = (r as u32) << 16 | (g as u32) << 8 | b as u32;
                    d[0] = *nearest
                        .entry(key)
                        .or_insert_with(|| closest(colors, r, g, b));
                }
                None => {
                    let v = pack(&s.format, r, g, b);
                    d.copy_from_slice(&v.to_le_bytes()[..bpp]);
                }
            }
        }
    }
    let offset = y * s.pitch + x * bpp as u32;
    surface::write_rows(cpu, &s, offset, w * bpp, &out);
    surface::changed(cpu, id);
    Ok(())
}

/// A pixel of the RGB format `f`.
fn pack(f: &Format, r: u8, g: u8, b: u8) -> u32 {
    let channel = |mask: u32, c: u8| {
        if mask == 0 {
            return 0;
        }
        let shift = mask.trailing_zeros();
        let bits = (mask >> shift).count_ones().min(8);
        ((c as u32) >> (8 - bits)) << shift & mask
    };
    channel(f.r, r) | channel(f.g, g) | channel(f.b, b)
}

/// The colours an 8-bit surface's pixels stand for, as COLORREFs: its
/// palette's, or the primary surface's of its DirectDraw object, or the
/// first 256 of a 3-3-2 cube when neither has one.
fn palette_for(cpu: &mut Cpu, id: u32, s: &surface::Surface) -> Vec<u32> {
    if let Some(c) = super::palette::colors(cpu, s.palette) {
        return c;
    }
    let owner = state(cpu).object(id).map_or(0, |o| o.owner);
    let primary = state(cpu)
        .objects
        .iter()
        .flatten()
        .filter(|o| o.owner == owner)
        .filter_map(|o| o.surface.as_ref())
        .find(|p| p.is_primary())
        .map(|p| p.palette);
    if let Some(c) = primary.and_then(|p| super::palette::colors(cpu, p)) {
        return c;
    }
    (0..256u32)
        .map(|i| {
            let r = (i >> 5) * 255 / 7;
            let g = (i >> 2 & 7) * 255 / 7;
            let b = (i & 3) * 255 / 3;
            r | g << 8 | b << 16
        })
        .collect()
}

/// The entry of `colors` (COLORREFs) nearest to the colour.
fn closest(colors: &[u32], r: u8, g: u8, b: u8) -> u8 {
    let d = |c: u32| {
        let dr = (c & 0xFF) as i32 - r as i32;
        let dg = (c >> 8 & 0xFF) as i32 - g as i32;
        let db = (c >> 16 & 0xFF) as i32 - b as i32;
        dr * dr + dg * dg + db * db
    };
    (0..colors.len().min(256))
        .min_by_key(|&i| d(colors[i]))
        .unwrap_or(0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::win32::ddraw::surface::rgb_format;

    #[test]
    fn pixels_are_packed_in_the_formats_masks() {
        assert_eq!(pack(&rgb_format(16), 0xFF, 0, 0), 0xF800);
        assert_eq!(pack(&rgb_format(16), 0, 0xFF, 0), 0x07E0);
        assert_eq!(pack(&rgb_format(16), 0x08, 0x04, 0xFF), 0x0821 | 0x1F);
        assert_eq!(pack(&rgb_format(32), 1, 2, 3), 0x01_0203);
        let f555 = Format {
            r: 0x7C00,
            g: 0x03E0,
            b: 0x001F,
            ..rgb_format(16)
        };
        assert_eq!(pack(&f555, 0xFF, 0xFF, 0xFF), 0x7FFF);
    }

    #[test]
    fn the_nearest_palette_entry_is_taken() {
        let colors = [0x00_0000, 0x00_00FF, 0x00_FF00, 0xFF_FFFF];
        assert_eq!(closest(&colors, 250, 10, 0), 1);
        assert_eq!(closest(&colors, 0, 200, 30), 2);
        assert_eq!(closest(&colors, 200, 200, 200), 3);
    }
}
