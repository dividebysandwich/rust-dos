//! Textures: IDirect3DTexture and 2, interfaces of a surface, and
//! IDirect3DDevice7's Load. A texture is a surface with DDSCAPS_TEXTURE;
//! a device names it by a handle (GetHandle) in Direct3D 5's render
//! state TEXTUREHANDLE and in execute buffers.
//!
//! Load copies a texture's pixels, palette and color keys into another,
//! level by level of their mipmaps. IDirect3DDevice7's Load is ported
//! from Wine's device.c.

use super::super::surface::{self, DDSCAPS_MIPMAP, KEY_SRC_BLT};
use super::super::{
    DDERR_ALREADYINITIALIZED, DDERR_INVALIDOBJECT, DDERR_INVALIDPARAMS, DDERR_NOPALETTEATTACHED,
    state,
};
use super::device::{self, HANDLE_TEXTURE};
use super::{D3D_OK, D3DERR_TEXTURE_LOAD_FAILED, probe, read_dwords};
use crate::cpu::Cpu;

/// DDSCAPS2_MIPMAPSUBLEVEL and DDSCAPS2_CUBEMAP.
const MIPMAPSUBLEVEL: u32 = 0x1_0000;
const CUBEMAP: u32 = 0x200;

/// The surface of the texture interface `this`.
fn texture_of(cpu: &mut Cpu, this: u32) -> Result<u32, u32> {
    let (_, id) = super::super::this(cpu, this).ok_or(DDERR_INVALIDOBJECT)?;
    state(cpu).surface(id).ok_or(DDERR_INVALIDOBJECT)?;
    Ok(id)
}

/// Initialize(this, device, surface): it was.
pub fn initialize(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(DDERR_ALREADYINITIALIZED)
}

/// Unload(this).
pub fn unload(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(texture_of(cpu, args[0]));
    Some(D3D_OK)
}

/// GetHandle(this, device, out): the texture's handle on the device.
pub fn get_handle(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, device, out] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let id = hr!(texture_of(cpu, this));
    let Ok((_, did)) = device::device_of(cpu, device) else {
        return Some(DDERR_INVALIDPARAMS);
    };
    if !probe(cpu, out, 4, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let h = device::get(cpu, did).map_or(0, |d| d.handle_of(HANDLE_TEXTURE, id));
    cpu.bus.guest_write_32(out, h);
    Some(D3D_OK)
}

/// PaletteChanged(this, start, count): the palette is read when the
/// texture is drawn with.
pub fn palette_changed(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(texture_of(cpu, args[0]));
    Some(D3D_OK)
}

/// The next level of the mipmap `id`.
pub fn next_level(cpu: &mut Cpu, id: u32) -> Option<u32> {
    let s = state(cpu).surface(id)?.clone();
    if s.caps[0] & DDSCAPS_MIPMAP == 0 {
        return None;
    }
    s.attached.iter().map(|a| a.0).find(|&a| {
        state(cpu).surface(a).is_some_and(|l| {
            l.caps[0] & DDSCAPS_MIPMAP != 0 && l.width <= s.width && l.height <= s.height
        })
    })
}

/// The palette object of a surface, if it has one.
fn palette(cpu: &mut Cpu, id: u32) -> Option<u32> {
    let pal = state(cpu).surface(id)?.palette;
    if pal == 0 {
        return None;
    }
    super::super::this(cpu, pal).map(|(_, p)| p)
}

/// Copy the palette entries of `src`'s palette into `dest`'s.
fn copy_palette(cpu: &mut Cpu, dest: u32, src: u32) {
    let Some(entries) = state(cpu)
        .object(src)
        .and_then(|o| o.palette.as_ref().map(|p| p.entries.clone()))
    else {
        return;
    };
    if let Some(p) = state(cpu).object_mut(dest).and_then(|o| o.palette.as_mut()) {
        let n = p.entries.len().min(entries.len());
        p.entries[..n].copy_from_slice(&entries[..n]);
    }
}

/// Copy the rectangle `rect` (left, top, right, bottom) of the surface
/// `src` to `point` in `dest`, which has as many bits a pixel.
fn copy_rect(cpu: &mut Cpu, dest: u32, src: u32, point: (u32, u32), rect: [u32; 4]) {
    let (Some(d), Some(s)) = (
        state(cpu).surface(dest).cloned(),
        state(cpu).surface(src).cloned(),
    ) else {
        return;
    };
    if d.format.bits != s.format.bits || d.compressed() || s.compressed() {
        return;
    }
    let bpp = s.bytes_per_pixel();
    let [left, top, right, bottom] = rect;
    let right = right.min(s.width);
    let bottom = bottom.min(s.height);
    if left >= right || top >= bottom || point.0 >= d.width || point.1 >= d.height {
        return;
    }
    let w = (right - left).min(d.width - point.0);
    let h = (bottom - top).min(d.height - point.1);
    let mut row = vec![0; (w * bpp) as usize];
    for y in 0..h {
        surface::read_bytes(cpu, &s, (top + y) * s.pitch + left * bpp, &mut row);
        surface::write_bytes(cpu, &d, (point.1 + y) * d.pitch + point.0 * bpp, &row);
    }
    surface::changed(cpu, dest);
}

/// Load(this, source) of IDirect3DTexture and 2: the source's pixels,
/// level by level, its palette's entries and its source color key.
pub fn load(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, source] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let dest = hr!(texture_of(cpu, this));
    let Ok(src) = texture_of(cpu, source) else {
        return Some(DDERR_INVALIDPARAMS);
    };
    Some(match load_surfaces(cpu, dest, src) {
        Ok(()) => D3D_OK,
        Err(e) => e,
    })
}

/// Load the texture `src` into `dest`, as IDirect3DTexture's Load and
/// an execute buffer's TEXTURELOAD do.
pub fn load_surfaces(cpu: &mut Cpu, dest: u32, src: u32) -> Result<(), u32> {
    if dest == src {
        return Ok(());
    }
    let (mut d, mut s) = (Some(dest), Some(src));
    while let (Some(dl), Some(sl)) = (d, s) {
        let pal_src = palette(cpu, sl);
        let pal = palette(cpu, dl);
        if let Some(ps) = pal_src {
            let Some(p) = pal else {
                return Err(DDERR_NOPALETTEATTACHED);
            };
            copy_palette(cpu, p, ps);
        }
        let (Some(a), Some(b)) = (
            state(cpu).surface(dl).cloned(),
            state(cpu).surface(sl).cloned(),
        ) else {
            return Err(D3DERR_TEXTURE_LOAD_FAILED);
        };
        if (a.width, a.height) != (b.width, b.height) {
            return Err(D3DERR_TEXTURE_LOAD_FAILED);
        }
        if b.key_set & 1 << KEY_SRC_BLT != 0
            && let Some(t) = state(cpu).surface_mut(dl)
        {
            t.keys[KEY_SRC_BLT] = b.keys[KEY_SRC_BLT];
            t.key_set |= 1 << KEY_SRC_BLT;
        }
        copy_rect(cpu, dl, sl, (0, 0), [0, 0, b.width, b.height]);
        d = next_level(cpu, dl);
        s = next_level(cpu, sl);
    }
    Ok(())
}

/// Whether the levels of the mipmap `dest` are some of `src`'s: at least
/// one of the same size, and none that `src` has no level of its size
/// for.
fn is_mip_level_subset(cpu: &mut Cpu, dest: u32, src: u32) -> bool {
    let size = |cpu: &mut Cpu, id: u32| state(cpu).surface(id).map(|s| (s.width, s.height));
    let mut found = false;
    let (mut d, mut s) = (Some(dest), Some(src));
    while let (Some(dl), Some(sl)) = (d, s) {
        if size(cpu, dl) == size(cpu, sl) {
            found = true;
            d = next_level(cpu, dl);
        }
        s = next_level(cpu, sl);
    }
    d.is_none() && found
}

/// Copy the levels of `src` into those of `dest` of the same sizes,
/// with the palette's entries and the color keys.
fn copy_mipmap_chain(cpu: &mut Cpu, dest: u32, src: u32, point: (u32, u32), rect: [u32; 4]) {
    if let (Some(ps), Some(p)) = (palette(cpu, src), palette(cpu, dest)) {
        copy_palette(cpu, p, ps);
    }
    if let Some(sk) = state(cpu).surface(src).map(|s| (s.keys, s.key_set))
        && let Some(t) = state(cpu).surface_mut(dest)
    {
        for i in 0..4 {
            if sk.1 & 1 << i != 0 {
                t.keys[i] = sk.0[i];
                t.key_set |= 1 << i;
            }
        }
    }
    let (mut d, mut s) = (Some(dest), Some(src));
    let (mut point, mut rect) = (point, rect);
    while let (Some(dl), Some(sl)) = (d, s) {
        let same = state(cpu).surface(dl).map(|x| (x.width, x.height))
            == state(cpu).surface(sl).map(|x| (x.width, x.height));
        if same {
            copy_rect(cpu, dl, sl, point, rect);
            d = next_level(cpu, dl);
        }
        s = next_level(cpu, sl);
        point = (point.0 / 2, point.1 / 2);
        rect = [
            rect[0] / 2,
            rect[1] / 2,
            rect[2].div_ceil(2),
            rect[3].div_ceil(2),
        ];
    }
}

/// Load(this, dest, point, src, rect, flags) of IDirect3DDevice7: the
/// rectangle of `src` (all of it if NULL) to `point` of `dest` (its
/// corner if NULL), in the levels of `dest` that `src` has.
pub fn load7(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, dest, point_at, src, rect_at, _] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    hr!(device::device_of(cpu, this));
    let (Ok(dest), Ok(src)) = (texture_of(cpu, dest), texture_of(cpu, src)) else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (Some(d), Some(s)) = (
        state(cpu).surface(dest).cloned(),
        state(cpu).surface(src).cloned(),
    ) else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let rect = if rect_at != 0 {
        if !probe(cpu, rect_at, 16, false) {
            return Some(DDERR_INVALIDPARAMS);
        }
        read_dwords::<4>(cpu, rect_at).map(|v| v as i32)
    } else {
        [0, 0, s.width as i32, s.height as i32]
    };
    let point = if point_at != 0 {
        if !probe(cpu, point_at, 8, false) {
            return Some(DDERR_INVALIDPARAMS);
        }
        read_dwords::<2>(cpu, point_at).map(|v| v as i32)
    } else {
        [0, 0]
    };
    let [left, top, right, bottom] = rect;
    let (sw, sh) = (s.width as i64, s.height as i64);
    let bad = left >= right
        || top >= bottom
        || right as i64 > sw
        || bottom as i64 > sh
        || point[0] as i64 + (right - left) as i64 > sw
        || point[1] as i64 + (bottom - top) as i64 > sh
        || d.width > s.width
        || d.height > s.height
        || left < 0
        || top < 0
        || point[0] < 0
        || point[1] < 0;
    if bad {
        return Some(DDERR_INVALIDPARAMS);
    }
    if (s.caps[1] | d.caps[1]) & MIPMAPSUBLEVEL != 0 {
        return Some(DDERR_INVALIDPARAMS);
    }
    if (s.caps[1] | d.caps[1]) & CUBEMAP != 0 {
        // There are no cube maps.
        return Some(DDERR_INVALIDPARAMS);
    }
    if !is_mip_level_subset(cpu, dest, src) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let rect = [left as u32, top as u32, right as u32, bottom as u32];
    copy_mipmap_chain(cpu, dest, src, (point[0] as u32, point[1] as u32), rect);
    Some(D3D_OK)
}
