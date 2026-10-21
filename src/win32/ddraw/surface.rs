//! Surfaces: IDirectDrawSurface to IDirectDrawSurface7. A surface is a
//! rectangle of pixels in video memory or in the process's memory, with
//! its pixel format, color keys, palette and clipper, and the surfaces
//! attached to it.
//!
//! A complex surface is made at once with surfaces of its own: a flip
//! chain (the front buffer and its back buffers) or a mipmap's levels.
//! The surfaces after the first, its root, have no references until the
//! program asks for them, and their counts don't make them go: they go
//! with the root. Flip swaps the pixels of the chain's surfaces around,
//! and shows the front buffer's.
//!
//! The primary surface is the picture on the screen, at the start of
//! video memory: the desktop in a window, the mode's picture in full
//! screen. Lock gives the program its pixels in its view of video memory
//! (`display.rs`), or a system memory surface's in its own pages; GetDC
//! gives it a GDI memory DC with a DIB section at those pixels.

use super::com::{self, CLASS_E_NOAGGREGATION, Guid};
use super::{
    DD_OK, DDERR_CANNOTATTACHSURFACE, DDERR_CANNOTDETACHSURFACE, DDERR_CANTCREATEDC,
    DDERR_CANTDUPLICATE, DDERR_DCALREADYCREATED, DDERR_INVALIDCAPS, DDERR_INVALIDOBJECT,
    DDERR_INVALIDPARAMS, DDERR_INVALIDPIXELFORMAT, DDERR_INVALIDRECT, DDERR_MOREDATA,
    DDERR_NOCLIPPERATTACHED, DDERR_NOCOLORKEY, DDERR_NOCOOPERATIVELEVELSET, DDERR_NODC,
    DDERR_NOEXCLUSIVEMODE, DDERR_NOOVERLAYHW, DDERR_NOPALETTEATTACHED, DDERR_NOTAOVERLAYSURFACE,
    DDERR_NOTFLIPPABLE, DDERR_NOTFOUND, DDERR_NOTLOCKED, DDERR_OUTOFVIDEOMEMORY,
    DDERR_PRIMARYSURFACEALREADYEXISTS, DDERR_SURFACEALREADYATTACHED, DDERR_SURFACEBUSY,
    DDERR_SURFACELOST, DDERR_WRONGMODE, Format, Object, V_SURFACE1, V_SURFACE4, V_SURFACE7,
    display, palette, state,
};
use crate::cpu::Cpu;
use crate::win32::gdi32;

// DDSURFACEDESC's flags.
pub const DDSD_CAPS: u32 = 0x1;
pub const DDSD_HEIGHT: u32 = 0x2;
pub const DDSD_WIDTH: u32 = 0x4;
pub const DDSD_PITCH: u32 = 0x8;
pub const DDSD_BACKBUFFERCOUNT: u32 = 0x20;
pub const DDSD_ZBUFFERBITDEPTH: u32 = 0x40;
pub const DDSD_LPSURFACE: u32 = 0x800;
pub const DDSD_PIXELFORMAT: u32 = 0x1000;
pub const DDSD_CKDESTOVERLAY: u32 = 0x2000;
pub const DDSD_MIPMAPCOUNT: u32 = 0x2_0000;
pub const DDSD_REFRESHRATE: u32 = 0x4_0000;
pub const DDSD_LINEARSIZE: u32 = 0x8_0000;

// DDSCAPS' flags.
pub const DDSCAPS_BACKBUFFER: u32 = 0x4;
pub const DDSCAPS_COMPLEX: u32 = 0x8;
pub const DDSCAPS_FLIP: u32 = 0x10;
pub const DDSCAPS_FRONTBUFFER: u32 = 0x20;
pub const DDSCAPS_OFFSCREENPLAIN: u32 = 0x40;
pub const DDSCAPS_OVERLAY: u32 = 0x80;
pub const DDSCAPS_PALETTE: u32 = 0x100;
pub const DDSCAPS_PRIMARYSURFACE: u32 = 0x200;
pub const DDSCAPS_SYSTEMMEMORY: u32 = 0x800;
pub const DDSCAPS_TEXTURE: u32 = 0x1000;
pub const DDSCAPS_3DDEVICE: u32 = 0x2000;
pub const DDSCAPS_VIDEOMEMORY: u32 = 0x4000;
pub const DDSCAPS_VISIBLE: u32 = 0x8000;
pub const DDSCAPS_ZBUFFER: u32 = 0x2_0000;
pub const DDSCAPS_MIPMAP: u32 = 0x40_0000;
pub const DDSCAPS_LOCALVIDMEM: u32 = 0x1000_0000;
const DDSCAPS2_TEXTUREMANAGE: u32 = 0x10;
const DDSCAPS2_CUBEMAP: u32 = 0x200;
const DDSCAPS2_MIPMAPSUBLEVEL: u32 = 0x1_0000;
const DDSCAPS2_D3DTEXTUREMANAGE: u32 = 0x2_0000;
const DDSCAPS2_VOLUME: u32 = 0x20_0000;

// DDPIXELFORMAT's flags.
pub const DDPF_ALPHAPIXELS: u32 = 0x1;
pub const DDPF_FOURCC: u32 = 0x4;
pub const DDPF_PALETTEINDEXED8: u32 = 0x20;
pub const DDPF_RGB: u32 = 0x40;
pub const DDPF_ZBUFFER: u32 = 0x400;

// The color keys' flags.
pub const DDCKEY_COLORSPACE: u32 = 0x1;
pub const DDCKEY_DESTBLT: u32 = 0x2;
pub const DDCKEY_DESTOVERLAY: u32 = 0x4;
pub const DDCKEY_SRCBLT: u32 = 0x8;
pub const DDCKEY_SRCOVERLAY: u32 = 0x10;

/// The color keys, by their places in DDSURFACEDESC.
pub const KEY_DEST_OVERLAY: usize = 0;
pub const KEY_DEST_BLT: usize = 1;
pub const KEY_SRC_OVERLAY: usize = 2;
pub const KEY_SRC_BLT: usize = 3;

/// Where a surface's pixels are.
pub const MEM_NONE: u32 = 0;
pub const MEM_SYSTEM: u32 = 1;
pub const MEM_VIDEO: u32 = 2;

/// The compressed formats, which only system memory has.
const DXT: [&[u8; 4]; 5] = [b"DXT1", b"DXT2", b"DXT3", b"DXT4", b"DXT5"];

/// A DDSURFACEDESC or DDSURFACEDESC2, as its fields are.
#[derive(Clone, Debug, Default)]
pub struct Desc {
    pub flags: u32,
    pub height: u32,
    pub width: u32,
    /// The pitch, or the linear size.
    pub pitch: u32,
    pub backbuffers: u32,
    /// The mipmap count, the refresh rate or DDSURFACEDESC's Z buffer
    /// depth: they share their place.
    pub mipmaps: u32,
    pub refresh: u32,
    pub surface: u32,
    /// The keys, low and high, at their places (`KEY_`).
    pub keys: [(u32, u32); 4],
    pub format: Format,
    pub caps: [u32; 4],
}

impl Desc {
    pub fn read(cpu: &mut Cpu, at: u32, size: u32) -> Desc {
        let r = |cpu: &mut Cpu, offset: u32| cpu.bus.guest_read_32(at + offset);
        let mut keys = [(0, 0); 4];
        for (i, key) in keys.iter_mut().enumerate() {
            let offset = 0x28 + 8 * i as u32;
            *key = (r(cpu, offset), r(cpu, offset + 4));
        }
        let mut caps = [r(cpu, 0x68), 0, 0, 0];
        if size >= 124 {
            for (i, c) in caps.iter_mut().enumerate().skip(1) {
                *c = r(cpu, 0x68 + 4 * i as u32);
            }
        }
        Desc {
            flags: r(cpu, 0x04),
            height: r(cpu, 0x08),
            width: r(cpu, 0x0C),
            pitch: r(cpu, 0x10),
            backbuffers: r(cpu, 0x14),
            mipmaps: r(cpu, 0x18),
            refresh: r(cpu, 0x18),
            surface: r(cpu, 0x24),
            keys,
            format: read_format(cpu, at + 0x48),
            caps,
        }
    }

    /// Write it at `at`, `size` bytes of it, the rest zeroed.
    pub fn write(&self, cpu: &mut Cpu, at: u32, size: u32) {
        cpu.bus.guest_fill(at, size as usize, 0);
        let w = |cpu: &mut Cpu, offset: u32, value: u32| cpu.bus.guest_write_32(at + offset, value);
        w(cpu, 0x00, size);
        w(cpu, 0x04, self.flags);
        w(cpu, 0x08, self.height);
        w(cpu, 0x0C, self.width);
        w(cpu, 0x10, self.pitch);
        w(cpu, 0x14, self.backbuffers);
        w(cpu, 0x18, self.mipmaps | self.refresh);
        w(cpu, 0x24, self.surface);
        for (i, key) in self.keys.iter().enumerate() {
            let offset = 0x28 + 8 * i as u32;
            w(cpu, offset, key.0);
            w(cpu, offset + 4, key.1);
        }
        write_format(cpu, at + 0x48, &self.format);
        let caps = if size >= 124 { 4 } else { 1 };
        for (i, c) in self.caps.iter().take(caps).enumerate() {
            w(cpu, 0x68 + 4 * i as u32, *c);
        }
    }
}

pub fn read_format(cpu: &mut Cpu, at: u32) -> Format {
    let r = |cpu: &mut Cpu, offset: u32| cpu.bus.guest_read_32(at + offset);
    Format {
        flags: r(cpu, 4),
        fourcc: r(cpu, 8),
        bits: r(cpu, 12),
        r: r(cpu, 16),
        g: r(cpu, 20),
        b: r(cpu, 24),
        a: r(cpu, 28),
    }
}

pub fn write_format(cpu: &mut Cpu, at: u32, f: &Format) {
    for (i, v) in [32, f.flags, f.fourcc, f.bits, f.r, f.g, f.b, f.a]
        .into_iter()
        .enumerate()
    {
        cpu.bus.guest_write_32(at + 4 * i as u32, v);
    }
}

/// The format of a depth of the display's.
pub fn rgb_format(bpp: u32) -> Format {
    match bpp {
        8 => Format {
            flags: DDPF_PALETTEINDEXED8 | DDPF_RGB,
            bits: 8,
            ..Format::default()
        },
        16 => Format {
            flags: DDPF_RGB,
            bits: 16,
            r: 0xF800,
            g: 0x07E0,
            b: 0x001F,
            ..Format::default()
        },
        _ => Format {
            flags: DDPF_RGB,
            bits: bpp,
            r: 0xFF_0000,
            g: 0x00_FF00,
            b: 0x00_00FF,
            ..Format::default()
        },
    }
}

/// The size a DDSURFACEDESC at `at` says it has, if it is the one the
/// interface version takes: DDSURFACEDESC up to 3, DDSURFACEDESC2 from
/// 4 on.
pub fn desc_size(cpu: &mut Cpu, at: u32, version: u32) -> Result<u32, u32> {
    if at == 0 || !cpu.bus.guest_probe(at, 4, false) {
        return Err(DDERR_INVALIDPARAMS);
    }
    let size = cpu.bus.guest_read_32(at);
    let wanted = if version >= 4 { 124 } else { 108 };
    if size != wanted || !cpu.bus.guest_probe(at, size as usize, true) {
        return Err(DDERR_INVALIDPARAMS);
    }
    Ok(size)
}

/// Data SetPrivateData stored.
#[derive(Clone, Debug, Default)]
pub struct Private {
    pub tag: Vec<u8>,
    pub flags: u32,
    pub data: Vec<u8>,
    /// The surface's uniqueness value when it was stored.
    pub uniqueness: u32,
}

/// What a surface has of its own.
#[derive(Clone, Debug, Default)]
pub struct Surface {
    /// DDSCAPS2's four words.
    pub caps: [u32; 4],
    pub width: u32,
    pub height: u32,
    /// Bytes a row, or for a compressed format the size.
    pub pitch: u32,
    pub format: Format,
    /// Where its pixels are (`MEM_`): the address in the process, or the
    /// offset in video memory.
    pub memory: u32,
    pub bits: u32,
    /// Whether its pixels are DirectDraw's to free: not SetSurfaceDesc's.
    pub owns_bits: bool,
    pub lost: bool,
    pub locked: bool,
    /// GetDC's DC, its bitmap, and the bitmap it had before.
    pub dc: u32,
    pub dc_bitmap: u32,
    pub dc_old: u32,
    pub keys: [(u32, u32); 4],
    /// Which keys are set: a bit for each place.
    pub key_set: u32,
    /// The interfaces of its palette and clipper, which it holds.
    pub palette: u32,
    pub clipper: u32,
    /// The surfaces attached to it, and the interface of each that it
    /// holds, 0 for one of its complex surface.
    pub attached: Vec<(u32, u32)>,
    /// The root of the complex surface it is part of, or 0; the root
    /// has the others.
    pub root: u32,
    pub children: Vec<u32>,
    /// The version of the interfaces it was made through: 1, 4 or 7.
    pub version: u32,
    /// The front buffer's number of back buffers.
    pub backbuffers: u32,
    /// The mipmap levels from this one on.
    pub mipmaps: u32,
    pub private: Vec<Private>,
    pub uniqueness: u32,
    pub priority: u32,
    pub lod: u32,
}

impl Surface {
    pub fn bytes_per_pixel(&self) -> u32 {
        self.format.bits.div_ceil(8)
    }

    pub fn compressed(&self) -> bool {
        self.format.flags & DDPF_FOURCC != 0
    }

    /// The bytes of its pixels.
    pub fn size(&self) -> u32 {
        if self.compressed() {
            self.pitch
        } else {
            self.pitch * self.height
        }
    }

    pub fn is_primary(&self) -> bool {
        self.caps[0] & DDSCAPS_PRIMARYSURFACE != 0
    }

    /// Its description, as GetSurfaceDesc gives it.
    pub fn desc(&self, size: u32) -> Desc {
        let mut flags = DDSD_CAPS | DDSD_HEIGHT | DDSD_WIDTH | DDSD_PIXELFORMAT;
        flags |= if self.compressed() {
            DDSD_LINEARSIZE
        } else {
            DDSD_PITCH
        };
        if self.backbuffers > 0 {
            flags |= DDSD_BACKBUFFERCOUNT;
        }
        if self.caps[0] & DDSCAPS_MIPMAP != 0 {
            flags |= DDSD_MIPMAPCOUNT;
        }
        let mut zbits = 0;
        if self.format.flags & DDPF_ZBUFFER != 0 && size < 124 {
            flags |= DDSD_ZBUFFERBITDEPTH;
            zbits = self.format.bits;
        }
        for i in 0..4 {
            if self.key_set & 1 << i != 0 {
                flags |= DDSD_CKDESTOVERLAY << i;
            }
        }
        Desc {
            flags,
            height: self.height,
            width: self.width,
            pitch: self.pitch,
            backbuffers: self.backbuffers,
            mipmaps: if flags & DDSD_MIPMAPCOUNT != 0 {
                self.mipmaps
            } else {
                zbits
            },
            keys: self.keys,
            format: self.format,
            caps: self.caps,
            ..Desc::default()
        }
    }
}

/// The surface `this` is an interface of: its vtable and id.
pub fn surface_of(cpu: &mut Cpu, this: u32) -> Result<(usize, u32), u32> {
    let (vtable, id) = super::this(cpu, this).ok_or(DDERR_INVALIDOBJECT)?;
    if state(cpu).surface(id).is_none() {
        return Err(DDERR_INVALIDOBJECT);
    }
    Ok((vtable, id))
}

fn surface(cpu: &mut Cpu, id: u32) -> Option<&mut Surface> {
    state(cpu).surface_mut(id)
}

/// The surface `this` is, with an error if it is lost.
fn usable(cpu: &mut Cpu, this: u32) -> Result<(usize, u32), u32> {
    let (vtable, id) = surface_of(cpu, this)?;
    if state(cpu).surface(id).is_some_and(|s| s.lost) {
        return Err(DDERR_SURFACELOST);
    }
    Ok((vtable, id))
}

/// The surface interface vtable of the version a surface was made
/// through.
pub fn vtable_of(version: u32) -> usize {
    match version {
        4 => V_SURFACE4,
        7 => V_SURFACE7,
        _ => V_SURFACE1,
    }
}

/// A new uniqueness value.
fn next_uniqueness(cpu: &mut Cpu) -> u32 {
    let s = state(cpu);
    s.uniqueness = s.uniqueness.wrapping_add(1).max(1);
    s.uniqueness
}

/// The surface `id` changed: its uniqueness value with it.
pub fn changed(cpu: &mut Cpu, id: u32) {
    let value = next_uniqueness(cpu);
    if let Some(s) = surface(cpu, id) {
        s.uniqueness = value;
    }
}

// Pixel memory.

/// Where the program sees a surface's pixels: None if they aren't
/// there.
pub fn address(cpu: &mut Cpu, s: &Surface) -> Option<u32> {
    match s.memory {
        MEM_SYSTEM => Some(s.bits),
        MEM_VIDEO => Some(display::view(cpu)? + s.bits),
        _ => None,
    }
}

/// Read `len` bytes of a surface's pixels from byte `offset`.
pub fn read_bytes(cpu: &mut Cpu, s: &Surface, offset: u32, data: &mut [u8]) {
    match s.memory {
        MEM_VIDEO => {
            let start = (s.bits + offset) as usize;
            let vram = &cpu.bus.vbe.vram;
            let end = (start + data.len()).min(vram.len());
            if start < end {
                data[..end - start].copy_from_slice(&vram[start..end]);
            }
        }
        MEM_SYSTEM => cpu.bus.guest_read_bytes(s.bits + offset, data),
        _ => data.fill(0),
    }
}

/// Write bytes of a surface's pixels at byte `offset`.
pub fn write_bytes(cpu: &mut Cpu, s: &Surface, offset: u32, data: &[u8]) {
    match s.memory {
        MEM_VIDEO => {
            let start = (s.bits + offset) as usize;
            let vram = &mut cpu.bus.vbe.vram;
            let end = (start + data.len()).min(vram.len());
            if start < end {
                vram[start..end].copy_from_slice(&data[..end - start]);
                display::written(cpu, start as u32, (end - start) as u32);
            }
        }
        MEM_SYSTEM => cpu.bus.guest_write_bytes(s.bits + offset, data),
        _ => {}
    }
}

/// Give the surface `id` its pixels: video memory unless it asked for
/// system memory or video memory is full and it didn't ask for that.
/// The primary surface is the picture on the screen.
fn allocate(cpu: &mut Cpu, id: u32) -> Result<(), u32> {
    let Some(s) = state(cpu).surface(id).cloned() else {
        return Err(DDERR_INVALIDOBJECT);
    };
    let size = s.size();
    let (memory, bits) = if s.is_primary() {
        let (w, h, bpp, pitch) = display::current(cpu);
        if (w, h, bpp, pitch) != (s.width, s.height, s.format.bits, s.pitch) {
            return Err(DDERR_WRONGMODE);
        }
        (MEM_VIDEO, 0)
    } else if s.caps[0] & DDSCAPS_SYSTEMMEMORY != 0 {
        let bits = crate::win32::kernel32::alloc_pages(cpu, size).ok_or(com::E_OUTOFMEMORY)?;
        (MEM_SYSTEM, bits)
    } else {
        (MEM_VIDEO, display::alloc(cpu, size)?)
    };
    if let Some(s) = surface(cpu, id) {
        s.memory = memory;
        s.bits = bits;
        s.owns_bits = true;
        s.lost = false;
    }
    Ok(())
}

/// Let go of the surface `id`'s pixels, unless another surface has them
/// too (DuplicateSurface's).
fn free_memory(cpu: &mut Cpu, id: u32) {
    let Some(s) = state(cpu).surface(id).cloned() else {
        return;
    };
    let shared = state(cpu)
        .ids(|o| {
            o.surface
                .as_ref()
                .is_some_and(|t| t.memory == s.memory && t.bits == s.bits)
        })
        .len()
        > 1;
    if !shared && s.owns_bits {
        match s.memory {
            MEM_SYSTEM if s.bits != 0 => crate::win32::kernel32::free_pages(cpu, s.bits),
            MEM_VIDEO => display::free(cpu, s.bits),
            _ => {}
        }
    }
    if let Some(s) = surface(cpu, id) {
        s.memory = MEM_NONE;
        s.bits = 0;
    }
}

/// The mode changed or went: video memory is the new mode's, and the
/// surfaces that had pixels there are lost.
pub fn video_memory_lost(cpu: &mut Cpu) {
    for id in state(cpu).ids(|o| o.surface.as_ref().is_some_and(|s| s.memory == MEM_VIDEO)) {
        if let Some(s) = surface(cpu, id) {
            s.memory = MEM_NONE;
            s.bits = 0;
            s.lost = true;
        }
    }
}

/// The DirectDraw object of the surface `id` went: it has no pixels.
pub fn orphan(cpu: &mut Cpu, id: u32) {
    if state(cpu).surface(id).is_none() {
        return;
    }
    end_dc(cpu, id);
    free_memory(cpu, id);
    if let Some(s) = surface(cpu, id) {
        s.lost = true;
        s.locked = false;
    }
}

/// The surface `id` goes: the rest of its complex surface with it, and
/// what it holds is let go.
pub fn destroyed(cpu: &mut Cpu, id: u32) {
    let Some(s) = state(cpu).surface(id).cloned() else {
        return;
    };
    end_dc(cpu, id);
    free_memory(cpu, id);
    for &(_, held) in &s.attached {
        if held != 0 {
            super::release(cpu, held);
        }
    }
    for held in [s.palette, s.clipper] {
        if held != 0 {
            super::release(cpu, held);
        }
    }
    for p in &s.private {
        if p.flags & DDSPD_IUNKNOWNPOINTER != 0 && p.data.len() == 4 {
            let iface = u32::from_le_bytes([p.data[0], p.data[1], p.data[2], p.data[3]]);
            super::release(cpu, iface);
        }
    }
    // Others it is attached to let it go.
    for other in state(cpu).ids(|o| {
        o.surface
            .as_ref()
            .is_some_and(|t| t.attached.iter().any(|a| a.0 == id))
    }) {
        if let Some(t) = surface(cpu, other) {
            t.attached.retain(|a| a.0 != id);
        }
    }
    for child in s.children {
        super::destroy(cpu, child);
    }
}

// CreateSurface.

const PITCH_ALIGN: u32 = 4;

/// The pitch of `width` pixels of `bits` bits.
fn pitch_of(width: u32, bits: u32) -> u32 {
    (width * bits).div_ceil(8).next_multiple_of(PITCH_ALIGN)
}

/// Check a pixel format a program gave: the ones surfaces can have.
fn check_format(f: &Format, caps: &[u32; 4]) -> Result<Format, u32> {
    if f.flags & DDPF_FOURCC != 0 {
        let code = f.fourcc.to_le_bytes();
        if !DXT.iter().any(|d| **d == code) {
            return Err(DDERR_INVALIDPIXELFORMAT);
        }
        return Ok(Format {
            flags: DDPF_FOURCC,
            fourcc: f.fourcc,
            ..Format::default()
        });
    }
    if f.flags & DDPF_ZBUFFER != 0 {
        if caps[0] & DDSCAPS_ZBUFFER == 0 || ![16, 24, 32].contains(&f.bits) {
            return Err(DDERR_INVALIDPIXELFORMAT);
        }
        return Ok(z_format(f.bits));
    }
    if f.flags & !(DDPF_RGB | DDPF_PALETTEINDEXED8 | DDPF_ALPHAPIXELS) != 0 {
        return Err(DDERR_INVALIDPIXELFORMAT);
    }
    if f.flags & DDPF_PALETTEINDEXED8 != 0 {
        return if f.bits == 8 {
            Ok(rgb_format(8))
        } else {
            Err(DDERR_INVALIDPIXELFORMAT)
        };
    }
    if f.flags & DDPF_RGB == 0 || ![16, 24, 32].contains(&f.bits) {
        return Err(DDERR_INVALIDPIXELFORMAT);
    }
    Ok(Format {
        flags: f.flags & (DDPF_RGB | DDPF_ALPHAPIXELS),
        fourcc: 0,
        a: if f.flags & DDPF_ALPHAPIXELS != 0 {
            f.a
        } else {
            0
        },
        ..*f
    })
}

/// A Z buffer's format of `bits` bits.
fn z_format(bits: u32) -> Format {
    Format {
        flags: DDPF_ZBUFFER,
        bits,
        g: if bits == 32 {
            0xFFFF_FFFF
        } else {
            (1u32 << bits.min(31)) - 1
        },
        ..Format::default()
    }
}

/// CreateSurface(this, desc, out, outer).
pub fn create_surface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, desc_at, out, outer] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (vtable, dd) = hr!(super::ddraw::ddraw_of(cpu, this));
    let version = super::version(vtable);
    if outer != 0 {
        return Some(CLASS_E_NOAGGREGATION);
    }
    if out == 0 || !cpu.bus.guest_probe(out, 4, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    cpu.bus.guest_write_32(out, 0);
    let size = hr!(desc_size(cpu, desc_at, version));
    let level = super::ddraw::ddraw(cpu, dd).map_or(0, |d| d.level);
    if level == 0 {
        return Some(DDERR_NOCOOPERATIVELEVELSET);
    }
    let desc = Desc::read(cpu, desc_at, size);
    let ids = hr!(make(cpu, dd, version, level, &desc));
    let root = ids[0];
    let surface_version = match version {
        1 | 2 => 1,
        v => v,
    };
    let hr = super::give(cpu, root, vtable_of(surface_version), out);
    if hr != DD_OK {
        super::destroy(cpu, root);
        return Some(hr);
    }
    if version >= 4 {
        super::add_ref(cpu, this);
        if let Some(o) = state(cpu).object_mut(root) {
            o.holds_creator = true;
        }
    }
    Some(DD_OK)
}

/// Make the surfaces `desc` describes for the DirectDraw object `dd`:
/// their ids, the root first.
fn make(cpu: &mut Cpu, dd: u32, version: u32, level: u32, desc: &Desc) -> Result<Vec<u32>, u32> {
    let mut caps = if desc.flags & DDSD_CAPS != 0 {
        desc.caps
    } else {
        [0; 4]
    };
    if caps[0] & DDSCAPS_SYSTEMMEMORY != 0 && caps[0] & DDSCAPS_VIDEOMEMORY != 0 {
        return Err(DDERR_INVALIDCAPS);
    }
    if caps[0] & DDSCAPS_BACKBUFFER != 0
        || caps[1] & (DDSCAPS2_CUBEMAP | DDSCAPS2_VOLUME) != 0
        || caps[0] & DDSCAPS_FRONTBUFFER != 0 && version >= 7
    {
        return Err(DDERR_INVALIDCAPS);
    }
    if caps[0] & DDSCAPS_OVERLAY != 0 {
        return Err(DDERR_NOOVERLAYHW);
    }
    let managed = caps[1] & (DDSCAPS2_TEXTUREMANAGE | DDSCAPS2_D3DTEXTUREMANAGE) != 0;
    if managed && caps[0] & (DDSCAPS_SYSTEMMEMORY | DDSCAPS_VIDEOMEMORY) != 0 {
        return Err(DDERR_INVALIDCAPS);
    }
    let flip = caps[0] & DDSCAPS_FLIP != 0;
    let backbuffers = if flip {
        if caps[0] & DDSCAPS_COMPLEX == 0 {
            return Err(DDERR_INVALIDCAPS);
        }
        if desc.flags & DDSD_BACKBUFFERCOUNT == 0 || desc.backbuffers == 0 {
            return Err(DDERR_INVALIDPARAMS);
        }
        desc.backbuffers
    } else {
        0
    };
    let primary = caps[0] & DDSCAPS_PRIMARYSURFACE != 0;
    let (width, height, format, pitch);
    if primary {
        if desc.flags & (DDSD_WIDTH | DDSD_HEIGHT) != 0 {
            return Err(DDERR_INVALIDPARAMS);
        }
        let exists = state(cpu)
            .ids(|o| o.owner == dd && o.surface.as_ref().is_some_and(|s| s.is_primary()))
            .len()
            > 0;
        if exists {
            return Err(DDERR_PRIMARYSURFACEALREADYEXISTS);
        }
        if flip && level & super::ddraw::DDSCL_EXCLUSIVE == 0 {
            return Err(DDERR_NOEXCLUSIVEMODE);
        }
        let (w, h, bpp, p) = display::current(cpu);
        (width, height, format, pitch) = (w, h, rgb_format(bpp), p);
        // It is the screen, but says it is in system memory if it was
        // asked to be.
        caps[0] &= !DDSCAPS_FRONTBUFFER;
        caps[0] |= DDSCAPS_VISIBLE;
    } else {
        if desc.flags & DDSD_WIDTH == 0 || desc.flags & DDSD_HEIGHT == 0 {
            return Err(DDERR_INVALIDPARAMS);
        }
        if desc.width == 0 || desc.height == 0 || desc.width > 0x4000 || desc.height > 0x4000 {
            return Err(DDERR_INVALIDPARAMS);
        }
        let f = if desc.flags & DDSD_PIXELFORMAT != 0 {
            check_format(&desc.format, &caps)?
        } else if caps[0] & DDSCAPS_ZBUFFER != 0 {
            if desc.flags & DDSD_ZBUFFERBITDEPTH == 0 || ![16, 24, 32].contains(&desc.mipmaps) {
                return Err(DDERR_INVALIDPIXELFORMAT);
            }
            z_format(desc.mipmaps)
        } else {
            rgb_format(display::current(cpu).2)
        };
        if f.flags & DDPF_FOURCC != 0 {
            // Compressed formats are in system memory, and managed.
            if caps[0] & DDSCAPS_SYSTEMMEMORY == 0 && !managed {
                return Err(DDERR_INVALIDPARAMS);
            }
        }
        (width, height, format) = (desc.width, desc.height, f);
        pitch = if f.flags & DDPF_FOURCC != 0 {
            let block = if f.fourcc == u32::from_le_bytes(*b"DXT1") {
                8
            } else {
                16
            };
            width.div_ceil(4).max(1) * height.div_ceil(4).max(1) * block
        } else {
            pitch_of(width, f.bits)
        };
        if managed {
            caps[0] |= DDSCAPS_SYSTEMMEMORY;
        }
    }
    // The mipmap levels.
    let mipmap = caps[0] & DDSCAPS_MIPMAP != 0;
    let levels = if mipmap && caps[0] & DDSCAPS_COMPLEX != 0 {
        if desc.flags & DDSD_MIPMAPCOUNT != 0 {
            if desc.mipmaps == 0 {
                return Err(DDERR_INVALIDPARAMS);
            }
            desc.mipmaps
        } else {
            32 - width.min(height).leading_zeros()
        }
    } else {
        1
    };
    let mut base = Surface {
        caps,
        width,
        height,
        pitch,
        format,
        version: match version {
            1 | 2 => 1,
            v => v,
        },
        mipmaps: if mipmap { levels } else { 0 },
        ..Surface::default()
    };
    for i in 0..4 {
        if desc.flags & DDSD_CKDESTOVERLAY << i != 0 {
            base.keys[i] = desc.keys[i];
            base.key_set |= 1 << i;
        }
    }
    // The surfaces: the root, then the back buffers or the levels.
    let mut list = vec![base.clone()];
    if flip {
        list[0].caps[0] |= DDSCAPS_FRONTBUFFER;
        list[0].backbuffers = backbuffers;
        for i in 0..backbuffers {
            let mut back = base.clone();
            back.caps[0] &= !(DDSCAPS_PRIMARYSURFACE | DDSCAPS_VISIBLE);
            if i == 0 {
                back.caps[0] |= DDSCAPS_BACKBUFFER;
            }
            list.push(back);
        }
    }
    for level in 1..levels {
        let mut sub = base.clone();
        sub.width = (width >> level).max(1);
        sub.height = (height >> level).max(1);
        sub.pitch = pitch_of(sub.width, format.bits);
        sub.mipmaps = levels - level;
        if version >= 7 {
            sub.caps[1] |= DDSCAPS2_MIPMAPSUBLEVEL;
        }
        list.push(sub);
    }
    let pid = super::current_pid(cpu);
    let mut ids = Vec::new();
    for s in list {
        let id = super::add_object(
            cpu,
            Object {
                pid,
                owner: dd,
                creator: 0,
                surface: Some(s),
                ..Object::default()
            },
        );
        ids.push(id);
    }
    let creator = super::iface(cpu, dd, super::vtable_for_version(version));
    let root = ids[0];
    for (i, &id) in ids.iter().enumerate() {
        if let Some(o) = state(cpu).object_mut(id) {
            o.creator = creator;
        }
        let next = ids.get(i + 1).copied();
        if let Some(s) = surface(cpu, id) {
            if i > 0 {
                s.root = root;
            }
            if let Some(next) = next {
                s.attached.push((next, 0));
            }
        }
    }
    if let Some(s) = surface(cpu, root) {
        s.children = ids[1..].to_vec();
    }
    // Their pixels: video memory, unless there is none and they didn't
    // ask for it.
    let wants_video = caps[0] & DDSCAPS_VIDEOMEMORY != 0;
    for &id in &ids {
        let mut result = allocate(cpu, id);
        if result == Err(DDERR_OUTOFVIDEOMEMORY) && !wants_video && !primary {
            if let Some(s) = surface(cpu, id) {
                s.caps[0] |= DDSCAPS_SYSTEMMEMORY;
            }
            result = allocate(cpu, id);
        }
        if let Err(e) = result {
            super::destroy(cpu, root);
            return Err(e);
        }
        if let Some(s) = surface(cpu, id) {
            let system = s.caps[0] & DDSCAPS_SYSTEMMEMORY != 0;
            if s.memory == MEM_VIDEO && !system {
                s.caps[0] |= DDSCAPS_VIDEOMEMORY | DDSCAPS_LOCALVIDMEM;
            } else if s.memory == MEM_SYSTEM && !managed {
                s.caps[0] |= DDSCAPS_SYSTEMMEMORY;
            }
        }
        changed(cpu, id);
    }
    Ok(ids)
}

/// DuplicateSurface(this, surface, out): a surface with the same pixels.
pub fn duplicate_surface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, src, out] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (vtable, dd) = hr!(super::ddraw::ddraw_of(cpu, this));
    let (src_vtable, id) = hr!(surface_of(cpu, src));
    if out == 0 || !cpu.bus.guest_probe(out, 4, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let Some(s) = state(cpu).surface(id).cloned() else {
        return Some(DDERR_INVALIDOBJECT);
    };
    if s.is_primary() || s.root != 0 || !s.children.is_empty() {
        return Some(DDERR_CANTDUPLICATE);
    }
    let copy = Surface {
        attached: Vec::new(),
        palette: 0,
        clipper: 0,
        dc: 0,
        dc_bitmap: 0,
        dc_old: 0,
        locked: false,
        private: Vec::new(),
        ..s
    };
    let creator = super::iface(cpu, dd, vtable);
    let new = super::add_object(
        cpu,
        Object {
            owner: dd,
            creator,
            surface: Some(copy),
            ..Object::default()
        },
    );
    Some(super::give(cpu, new, src_vtable, out))
}

// The methods.

/// Overlay methods: no surface is an overlay.
pub fn not_overlay(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(surface_of(cpu, args[0]));
    Some(DDERR_NOTAOVERLAYSURFACE)
}

/// Initialize(this, ddraw, desc).
pub fn initialize(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(surface_of(cpu, args[0]));
    Some(super::DDERR_ALREADYINITIALIZED)
}

/// GetCaps(this, caps): DDSCAPS, or DDSCAPS2 from IDirectDrawSurface4.
pub fn get_caps(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (vtable, id) = hr!(surface_of(cpu, args[0]));
    let words = if super::version(vtable) >= 4 { 4 } else { 1 };
    let at = args[1];
    if at == 0 || !cpu.bus.guest_probe(at, 4 * words, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let caps = state(cpu).surface(id).map(|s| s.caps).unwrap_or_default();
    for (i, c) in caps.iter().take(words).enumerate() {
        cpu.bus.guest_write_32(at + 4 * i as u32, *c);
    }
    Some(DD_OK)
}

/// GetSurfaceDesc(this, desc).
pub fn get_surface_desc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (vtable, id) = hr!(surface_of(cpu, args[0]));
    let size = hr!(desc_size(cpu, args[1], super::version(vtable)));
    let Some(s) = state(cpu).surface(id).cloned() else {
        return Some(DDERR_INVALIDOBJECT);
    };
    s.desc(size).write(cpu, args[1], size);
    Some(DD_OK)
}

/// GetPixelFormat(this, format).
pub fn get_pixel_format(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, id) = hr!(surface_of(cpu, args[0]));
    let at = args[1];
    if at == 0 || !cpu.bus.guest_probe(at, 32, true) || cpu.bus.guest_read_32(at) != 32 {
        return Some(DDERR_INVALIDPARAMS);
    }
    let format = state(cpu).surface(id).map(|s| s.format).unwrap_or_default();
    write_format(cpu, at, &format);
    Some(DD_OK)
}

/// IsLost(this).
pub fn is_lost(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(usable(cpu, args[0]));
    Some(DD_OK)
}

/// Restore(this): pixels again for a lost surface, the rest of its
/// complex surface with it.
pub fn restore(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, id) = hr!(surface_of(cpu, args[0]));
    Some(match restore_surface(cpu, id) {
        Ok(()) => DD_OK,
        Err(e) => e,
    })
}

fn restore_surface(cpu: &mut Cpu, id: u32) -> Result<(), u32> {
    let Some(o) = state(cpu).object(id) else {
        return Err(DDERR_INVALIDOBJECT);
    };
    if o.owner == 0 {
        return Err(DDERR_SURFACELOST);
    }
    let Some(s) = o.surface.clone() else {
        return Err(DDERR_INVALIDOBJECT);
    };
    let mut list = vec![id];
    list.extend(&s.children);
    for id in list {
        if state(cpu).surface(id).is_some_and(|s| s.lost) {
            allocate(cpu, id)?;
            changed(cpu, id);
        }
    }
    Ok(())
}

/// RestoreAllSurfaces(this): the DirectDraw object's.
pub fn restore_all_surfaces(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, dd) = hr!(super::ddraw::ddraw_of(cpu, args[0]));
    for id in state(cpu).ids(|o| o.owner == dd && o.surface.as_ref().is_some_and(|s| s.root == 0)) {
        hr!(restore_surface(cpu, id));
    }
    Some(DD_OK)
}

/// Lock(this, rect, desc, flags, event).
pub fn lock(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, rect_at, desc_at, _flags, _] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(surface_of(cpu, this));
    if desc_at == 0 || !cpu.bus.guest_probe(desc_at, 4, false) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let size = cpu.bus.guest_read_32(desc_at);
    if (size != 108 && size != 124) || !cpu.bus.guest_probe(desc_at, size as usize, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let Some(s) = state(cpu).surface(id).cloned() else {
        return Some(DDERR_INVALIDOBJECT);
    };
    if s.lost {
        return Some(DDERR_SURFACELOST);
    }
    let rect = if rect_at != 0 {
        let r = gdi32::Rect::read(cpu, rect_at);
        let inside = r.left >= 0
            && r.top >= 0
            && r.left <= r.right
            && r.top <= r.bottom
            && r.right as u32 <= s.width
            && r.bottom as u32 <= s.height;
        if !inside {
            cpu.bus.guest_write_32(desc_at + 0x24, 0);
            return Some(DDERR_INVALIDPARAMS);
        }
        Some(r)
    } else {
        None
    };
    if s.locked || s.dc != 0 {
        return Some(DDERR_SURFACEBUSY);
    }
    let Some(mut at) = address(cpu, &s) else {
        return Some(com::E_OUTOFMEMORY);
    };
    if let Some(r) = rect
        && !s.compressed()
    {
        at += r.top as u32 * s.pitch + r.left as u32 * s.bytes_per_pixel();
    }
    let mut desc = s.desc(size);
    desc.surface = at;
    desc.write(cpu, desc_at, size);
    if let Some(s) = surface(cpu, id) {
        s.locked = true;
    }
    Some(DD_OK)
}

/// Unlock(this, rect): IDirectDrawSurface's takes the pointer instead.
pub fn unlock(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, id) = hr!(surface_of(cpu, args[0]));
    let Some(s) = surface(cpu, id) else {
        return Some(DDERR_INVALIDOBJECT);
    };
    if !s.locked {
        return Some(DDERR_NOTLOCKED);
    }
    s.locked = false;
    changed(cpu, id);
    Some(DD_OK)
}

/// GetBltStatus(this, flags).
pub fn get_blt_status(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(surface_of(cpu, args[0]));
    Some(match args[1] {
        1 | 2 => DD_OK,
        _ => DDERR_INVALIDPARAMS,
    })
}

/// GetFlipStatus(this, flags).
pub fn get_flip_status(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, id) = hr!(surface_of(cpu, args[0]));
    if args[1] != 1 && args[1] != 2 {
        return Some(DDERR_INVALIDPARAMS);
    }
    let flippable = state(cpu)
        .surface(id)
        .is_some_and(|s| s.caps[0] & DDSCAPS_FLIP != 0);
    Some(if flippable { DD_OK } else { DDERR_NOTFLIPPABLE })
}

// Attached surfaces.

/// AddAttachedSurface(this, surface).
pub fn add_attached_surface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, other] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(surface_of(cpu, this));
    let Ok((_, other_id)) = surface_of(cpu, other) else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (Some(a), Some(b)) = (
        state(cpu).surface(id).cloned(),
        state(cpu).surface(other_id).cloned(),
    ) else {
        return Some(DDERR_INVALIDOBJECT);
    };
    if id == other_id {
        return Some(DDERR_CANNOTATTACHSURFACE);
    }
    if a.attached.iter().any(|x| x.0 == other_id) || b.attached.iter().any(|x| x.0 == id) {
        return Some(DDERR_SURFACEALREADYATTACHED);
    }
    let z = b.caps[0] & DDSCAPS_ZBUFFER != 0;
    if a.version >= 7 && !z {
        return Some(DDERR_CANNOTATTACHSURFACE);
    }
    if a.width != b.width || a.height != b.height {
        return Some(DDERR_CANNOTATTACHSURFACE);
    }
    if a.caps[0] & DDSCAPS_ZBUFFER != 0 {
        return Some(DDERR_CANNOTATTACHSURFACE);
    }
    super::add_ref(cpu, other);
    if let Some(s) = surface(cpu, id) {
        s.attached.push((other_id, other));
    }
    Some(DD_OK)
}

/// DeleteAttachedSurface(this, flags, surface).
pub fn delete_attached_surface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, _, other] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(surface_of(cpu, this));
    let Ok((_, other_id)) = surface_of(cpu, other) else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let Some(s) = surface(cpu, id) else {
        return Some(DDERR_INVALIDOBJECT);
    };
    let Some(i) = s.attached.iter().position(|a| a.0 == other_id) else {
        return Some(DDERR_CANNOTDETACHSURFACE);
    };
    let (_, held) = s.attached[i];
    if held == 0 {
        return Some(DDERR_CANNOTDETACHSURFACE);
    }
    s.attached.remove(i);
    super::release(cpu, held);
    Some(DD_OK)
}

/// The surfaces attached to the surface `id` whose caps have all of
/// `caps`.
pub fn attached_matching(cpu: &mut Cpu, id: u32, caps: &[u32; 4]) -> Vec<u32> {
    let list = state(cpu)
        .surface(id)
        .map(|s| s.attached.clone())
        .unwrap_or_default();
    list.into_iter()
        .map(|a| a.0)
        .filter(|&a| {
            state(cpu)
                .surface(a)
                .is_some_and(|s| (0..4).all(|i| s.caps[i] & caps[i] == caps[i]))
        })
        .collect()
}

/// GetAttachedSurface(this, caps, out).
pub fn get_attached_surface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, caps_at, out] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (vtable, id) = hr!(surface_of(cpu, this));
    let words = if super::version(vtable) >= 4 { 4 } else { 1 };
    if caps_at == 0 || !cpu.bus.guest_probe(caps_at, 4 * words, false) {
        return Some(DDERR_INVALIDPARAMS);
    }
    if out == 0 || !cpu.bus.guest_probe(out, 4, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let mut caps = [0; 4];
    for (i, c) in caps.iter_mut().take(words).enumerate() {
        *c = cpu.bus.guest_read_32(caps_at + 4 * i as u32);
    }
    cpu.bus.guest_write_32(out, 0);
    match attached_matching(cpu, id, &caps).first() {
        Some(&found) => Some(super::give(cpu, found, vtable, out)),
        None => Some(DDERR_NOTFOUND),
    }
}

// Flip.

const DDFLIP_WAIT: u32 = 0x1;
const DDFLIP_NOVSYNC: u32 = 0x8;

/// The surfaces of the flip chain whose front buffer is `front`, the
/// front first.
fn chain(cpu: &mut Cpu, front: u32) -> Vec<u32> {
    let mut list = vec![front];
    let mut at = front;
    while let Some(next) = state(cpu)
        .surface(at)
        .and_then(|s| s.attached.iter().find(|a| a.1 == 0).map(|a| a.0))
    {
        if list.contains(&next) {
            break;
        }
        list.push(next);
        at = next;
    }
    list
}

/// Flip(this, target, flags): the front buffer takes the pixels of the
/// back buffer (or of `target`), the back buffers the next ones', and
/// the last the front buffer's. A primary front buffer is shown from
/// the next frame. With a flip still to show, it waits for that frame
/// with DDFLIP_WAIT.
pub fn flip(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, target, flags] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(usable(cpu, this));
    let Some(s) = state(cpu).surface(id).cloned() else {
        return Some(DDERR_INVALIDOBJECT);
    };
    if s.caps[0] & DDSCAPS_FRONTBUFFER == 0 || s.backbuffers == 0 {
        return Some(DDERR_NOTFLIPPABLE);
    }
    let list = chain(cpu, id);
    let target_id = if target != 0 {
        let Ok((_, t)) = surface_of(cpu, target) else {
            return Some(DDERR_INVALIDPARAMS);
        };
        if t == id || !list.contains(&t) {
            return Some(DDERR_INVALIDPARAMS);
        }
        Some(t)
    } else {
        None
    };
    let busy = list
        .iter()
        .any(|&m| state(cpu).surface(m).is_some_and(|s| s.locked || s.dc != 0));
    if busy {
        return Some(DDERR_SURFACEBUSY);
    }
    // A flip shown at a frame waits for the frame of the last.
    let primary = s.is_primary();
    if primary && flags & DDFLIP_NOVSYNC == 0 {
        let (timing, now) = display::timing(cpu);
        let pending = state(cpu).display.flip_at;
        if now < pending {
            if flags & DDFLIP_WAIT == 0 {
                return Some(super::DDERR_WASSTILLDRAWING);
            }
            cpu.set_eip(cpu.eip().wrapping_sub(4));
            return super::ddraw::wait_until(cpu, pending, DD_OK);
        }
        state(cpu).display.flip_at = timing.next_display(now);
    }
    // The pixels go round.
    let order: Vec<u32> = match target_id {
        Some(t) => vec![id, t],
        None => list.clone(),
    };
    let memory: Vec<(u32, u32)> = order
        .iter()
        .filter_map(|&m| state(cpu).surface(m).map(|s| (s.memory, s.bits)))
        .collect();
    let n = order.len();
    for (i, &m) in order.iter().enumerate() {
        let (memory, bits) = memory[(i + 1) % n];
        if let Some(s) = surface(cpu, m) {
            s.memory = memory;
            s.bits = bits;
        }
        changed(cpu, m);
    }
    if primary {
        let (_, bits) = memory[1 % n];
        display::show(cpu, bits);
    }
    Some(DD_OK)
}

/// FlipToGDISurface(this): the picture at the start of video memory,
/// GDI's, is shown again.
pub fn flip_to_gdi_surface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, dd) = hr!(super::ddraw::ddraw_of(cpu, args[0]));
    let fronts = state(cpu).ids(|o| {
        o.owner == dd
            && o.surface
                .as_ref()
                .is_some_and(|s| s.is_primary() && s.backbuffers > 0)
    });
    for front in fronts {
        let list = chain(cpu, front);
        let gdi = list.iter().copied().find(|&m| {
            state(cpu)
                .surface(m)
                .is_some_and(|s| s.memory == MEM_VIDEO && s.bits == 0)
        });
        if let Some(g) = gdi
            && g != front
        {
            let a = state(cpu).surface(front).map(|s| (s.memory, s.bits));
            let b = state(cpu).surface(g).map(|s| (s.memory, s.bits));
            if let (Some(a), Some(b)) = (a, b) {
                if let Some(s) = surface(cpu, front) {
                    (s.memory, s.bits) = b;
                }
                if let Some(s) = surface(cpu, g) {
                    (s.memory, s.bits) = a;
                }
            }
        }
    }
    if display::owns_display(cpu) {
        display::show(cpu, 0);
    }
    Some(DD_OK)
}

/// GetGDISurface(this, out): the surface whose pixels GDI draws on.
pub fn get_gdi_surface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, out] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, dd) = hr!(super::ddraw::ddraw_of(cpu, this));
    if out == 0 || !cpu.bus.guest_probe(out, 4, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let found = state(cpu)
        .ids(|o| {
            o.owner == dd
                && o.surface.as_ref().is_some_and(|s| {
                    s.memory == MEM_VIDEO && s.bits == 0 && s.caps[0] & DDSCAPS_FLIP != 0
                        || s.is_primary() && s.memory == MEM_VIDEO && s.bits == 0
                })
        })
        .first()
        .copied();
    let Some(id) = found else {
        cpu.bus.guest_write_32(out, 0);
        return Some(DDERR_NOTFOUND);
    };
    let version = state(cpu).surface(id).map_or(1, |s| s.version);
    Some(super::give(cpu, id, vtable_of(version), out))
}

// Color keys.

/// The key's place of a DDCKEY_ flag.
fn key_place(flags: u32) -> Option<usize> {
    match flags & !DDCKEY_COLORSPACE {
        DDCKEY_DESTOVERLAY => Some(KEY_DEST_OVERLAY),
        DDCKEY_DESTBLT => Some(KEY_DEST_BLT),
        DDCKEY_SRCOVERLAY => Some(KEY_SRC_OVERLAY),
        DDCKEY_SRCBLT => Some(KEY_SRC_BLT),
        _ => None,
    }
}

/// SetColorKey(this, flags, key).
pub fn set_color_key(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, flags, key] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(usable(cpu, this));
    let Some(place) = key_place(flags) else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let value = if key != 0 {
        if !cpu.bus.guest_probe(key, 8, false) {
            return Some(DDERR_INVALIDPARAMS);
        }
        let low = cpu.bus.guest_read_32(key);
        let high = cpu.bus.guest_read_32(key + 4);
        Some(if flags & DDCKEY_COLORSPACE != 0 {
            (low, high)
        } else {
            (low, low)
        })
    } else {
        None
    };
    let Some(s) = surface(cpu, id) else {
        return Some(DDERR_INVALIDOBJECT);
    };
    match value {
        Some(v) => {
            s.keys[place] = v;
            s.key_set |= 1 << place;
        }
        None => s.key_set &= !(1 << place),
    }
    Some(DD_OK)
}

/// GetColorKey(this, flags, key).
pub fn get_color_key(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, flags, key] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(usable(cpu, this));
    let Some(place) = key_place(flags) else {
        return Some(DDERR_INVALIDPARAMS);
    };
    if flags & DDCKEY_COLORSPACE != 0 || key == 0 || !cpu.bus.guest_probe(key, 8, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let Some(s) = state(cpu).surface(id) else {
        return Some(DDERR_INVALIDOBJECT);
    };
    if s.key_set & 1 << place == 0 {
        return Some(DDERR_NOCOLORKEY);
    }
    let (low, high) = s.keys[place];
    cpu.bus.guest_write_32(key, low);
    cpu.bus.guest_write_32(key + 4, high);
    Some(DD_OK)
}

// The palette and the clipper.

/// SetPalette(this, palette).
pub fn set_palette(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, pal] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(usable(cpu, this));
    let Some(s) = state(cpu).surface(id).cloned() else {
        return Some(DDERR_INVALIDOBJECT);
    };
    if pal != 0 {
        let palette_object = super::this(cpu, pal).filter(|&(v, _)| v == super::V_PALETTE);
        if palette_object.is_none() {
            return Some(DDERR_INVALIDPARAMS);
        }
    }
    if s.format.flags & DDPF_PALETTEINDEXED8 == 0 {
        return Some(DDERR_INVALIDPIXELFORMAT);
    }
    if pal != 0 {
        super::add_ref(cpu, pal);
    }
    if s.palette != 0 {
        super::release(cpu, s.palette);
    }
    if let Some(s) = surface(cpu, id) {
        s.palette = pal;
    }
    if pal != 0 && s.is_primary() {
        palette::show(cpu, pal);
    }
    Some(DD_OK)
}

/// GetPalette(this, out).
pub fn get_palette(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, out] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(usable(cpu, this));
    if out == 0 || !cpu.bus.guest_probe(out, 4, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let pal = state(cpu).surface(id).map_or(0, |s| s.palette);
    if pal == 0 {
        cpu.bus.guest_write_32(out, 0);
        return Some(DDERR_NOPALETTEATTACHED);
    }
    super::add_ref(cpu, pal);
    cpu.bus.guest_write_32(out, pal);
    Some(DD_OK)
}

/// SetClipper(this, clipper).
pub fn set_clipper(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, clipper] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(surface_of(cpu, this));
    if clipper != 0 {
        let ok = super::this(cpu, clipper).is_some_and(|(v, _)| v == super::V_CLIPPER);
        if !ok {
            return Some(DDERR_INVALIDPARAMS);
        }
        super::add_ref(cpu, clipper);
    }
    let old = state(cpu).surface(id).map_or(0, |s| s.clipper);
    if old != 0 {
        super::release(cpu, old);
    }
    if let Some(s) = surface(cpu, id) {
        s.clipper = clipper;
    }
    Some(DD_OK)
}

/// GetClipper(this, out).
pub fn get_clipper(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, out] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(surface_of(cpu, this));
    if out == 0 || !cpu.bus.guest_probe(out, 4, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let clipper = state(cpu).surface(id).map_or(0, |s| s.clipper);
    if clipper == 0 {
        return Some(DDERR_NOCLIPPERATTACHED);
    }
    super::add_ref(cpu, clipper);
    cpu.bus.guest_write_32(out, clipper);
    Some(DD_OK)
}

// GDI.

const BI_BITFIELDS: u32 = 3;

/// The colors of an 8-bit surface for GDI: its palette's, the primary
/// surface's, or the display's.
fn gdi_colors(cpu: &mut Cpu, s: &Surface) -> Vec<u32> {
    let pal = if s.palette != 0 {
        s.palette
    } else {
        let primary = state(cpu)
            .ids(|o| {
                o.surface
                    .as_ref()
                    .is_some_and(|t| t.is_primary() && t.palette != 0)
            })
            .first()
            .copied();
        primary
            .and_then(|p| state(cpu).surface(p))
            .map_or(0, |t| t.palette)
    };
    if let Some(colors) = palette::colors(cpu, pal) {
        return colors;
    }
    let shift = if cpu.bus.vga.dac_8bit { 0 } else { 2 };
    (0..256)
        .map(|i| {
            let p = &cpu.bus.vga.palette[i * 3..i * 3 + 3];
            let [r, g, b] = [p[0] << shift, p[1] << shift, p[2] << shift];
            r as u32 | (g as u32) << 8 | (b as u32) << 16
        })
        .collect()
}

/// GetDC(this, out): a memory DC with a DIB section on the pixels.
pub fn get_dc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, out] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(usable(cpu, this));
    if out == 0 || !cpu.bus.guest_probe(out, 4, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let Some(s) = state(cpu).surface(id).cloned() else {
        return Some(DDERR_INVALIDOBJECT);
    };
    if s.dc != 0 {
        return Some(DDERR_DCALREADYCREATED);
    }
    if s.locked {
        return Some(DDERR_SURFACEBUSY);
    }
    let gdi_format = s.format.flags & (DDPF_RGB | DDPF_PALETTEINDEXED8) != 0
        && s.format.flags & (DDPF_FOURCC | DDPF_ZBUFFER) == 0
        && [8, 16, 24, 32].contains(&s.format.bits);
    let at = address(cpu, &s);
    let (true, Some(at)) = (gdi_format, at) else {
        cpu.bus.guest_write_32(out, 0);
        return Some(DDERR_CANTCREATEDC);
    };
    let bpp = s.format.bits;
    let colors = if bpp == 8 {
        gdi_colors(cpu, &s)
    } else {
        Vec::new()
    };
    let compression = if bpp == 16 { BI_BITFIELDS } else { 0 };
    let mut header = vec![0u8; 40];
    let words = [
        40,
        s.width,
        (s.height as i32).wrapping_neg() as u32,
        1 | bpp << 16,
        compression,
        s.size(),
        0,
        0,
        colors.len() as u32,
        0,
    ];
    for (i, w) in words.iter().enumerate() {
        header[4 * i..4 * i + 4].copy_from_slice(&w.to_le_bytes());
    }
    let bitmap = gdi32::Bitmap {
        width: s.width as i32,
        height: s.height as i32,
        bpp: bpp as u16,
        planes: 1,
        stride: s.pitch,
        dib: Some(gdi32::Dib {
            bits: at,
            header,
            colors,
            masks: [s.format.r, s.format.g, s.format.b],
            top_down: true,
            compression,
            section: 0,
            offset: 0,
        }),
        ..gdi32::Bitmap::default()
    };
    let handle = gdi32::add(cpu, gdi32::Kind::Bitmap { bitmap });
    let hdc = gdi32::dc::create_compatible_dc(cpu, &[0]).unwrap_or(0);
    if handle == 0 || hdc == 0 {
        if handle != 0 {
            gdi32::delete_object_now(cpu, handle);
        }
        cpu.bus.guest_write_32(out, 0);
        return Some(DDERR_CANTCREATEDC);
    }
    let old = gdi32::dc::select_object(cpu, &[hdc, handle]).unwrap_or(0);
    if let Some(s) = surface(cpu, id) {
        s.dc = hdc;
        s.dc_bitmap = handle;
        s.dc_old = old;
    }
    cpu.bus.guest_write_32(out, hdc);
    Some(DD_OK)
}

/// Let go of the surface `id`'s DC, if it has one.
fn end_dc(cpu: &mut Cpu, id: u32) {
    let Some(s) = state(cpu).surface(id).cloned() else {
        return;
    };
    if s.dc == 0 {
        return;
    }
    if s.dc_old != 0 {
        gdi32::dc::select_object(cpu, &[s.dc, s.dc_old]);
    }
    gdi32::dc::delete_dc(cpu, &[s.dc]);
    gdi32::delete_object_now(cpu, s.dc_bitmap);
    if let Some(s) = surface(cpu, id) {
        s.dc = 0;
        s.dc_bitmap = 0;
        s.dc_old = 0;
    }
}

/// ReleaseDC(this, dc).
pub fn release_dc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, dc] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(surface_of(cpu, this));
    let has = state(cpu).surface(id).map_or(0, |s| s.dc);
    if has == 0 || has != dc {
        return Some(DDERR_NODC);
    }
    end_dc(cpu, id);
    changed(cpu, id);
    Some(DD_OK)
}

/// GetSurfaceFromDC(this, dc, out).
pub fn get_surface_from_dc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, dc, out] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (vtable, _) = hr!(super::ddraw::ddraw_of(cpu, this));
    if out == 0 || !cpu.bus.guest_probe(out, 4, true) {
        return Some(com::E_INVALIDARG);
    }
    cpu.bus.guest_write_32(out, 0);
    let found = state(cpu)
        .ids(|o| o.surface.as_ref().is_some_and(|s| s.dc == dc && dc != 0))
        .first()
        .copied();
    let Some(id) = found else {
        return Some(DDERR_NOTFOUND);
    };
    let wanted = if vtable == super::V_DD7 {
        V_SURFACE7
    } else {
        V_SURFACE1
    };
    Some(super::give(cpu, id, wanted, out))
}

// IDirectDrawSurface2 to 7.

/// GetDDInterface(this, out): the interface it was made through.
pub fn get_dd_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, out] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(surface_of(cpu, this));
    if out == 0 || !cpu.bus.guest_probe(out, 4, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let (owner, creator) = state(cpu)
        .object(id)
        .map_or((0, 0), |o| (o.owner, o.creator));
    if owner == 0 || creator == 0 {
        return Some(DDERR_INVALIDOBJECT);
    }
    super::add_ref(cpu, creator);
    cpu.bus.guest_write_32(out, creator);
    Some(DD_OK)
}

/// PageLock and PageUnlock(this, flags): pages are always there.
pub fn page_lock(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(surface_of(cpu, args[0]));
    Some(DD_OK)
}

/// SetSurfaceDesc(this, desc, flags): a system memory surface takes the
/// program's pixels.
pub fn set_surface_desc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, desc_at, flags] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (vtable, id) = hr!(surface_of(cpu, this));
    if flags != 0 {
        return Some(DDERR_INVALIDPARAMS);
    }
    let size = hr!(desc_size(cpu, desc_at, super::version(vtable)));
    let desc = Desc::read(cpu, desc_at, size);
    let Some(s) = state(cpu).surface(id).cloned() else {
        return Some(DDERR_INVALIDOBJECT);
    };
    let allowed =
        DDSD_LPSURFACE | DDSD_PITCH | DDSD_WIDTH | DDSD_HEIGHT | DDSD_PIXELFORMAT | DDSD_CAPS;
    if desc.flags & !allowed != 0 || desc.flags & DDSD_LPSURFACE == 0 || desc.surface == 0 {
        return Some(DDERR_INVALIDPARAMS);
    }
    if s.memory != MEM_SYSTEM || s.root != 0 || !s.children.is_empty() {
        return Some(DDERR_INVALIDPARAMS);
    }
    let format = if desc.flags & DDSD_PIXELFORMAT != 0 {
        hr!(check_format(&desc.format, &s.caps))
    } else {
        s.format
    };
    let width = if desc.flags & DDSD_WIDTH != 0 {
        desc.width
    } else {
        s.width
    };
    let height = if desc.flags & DDSD_HEIGHT != 0 {
        desc.height
    } else {
        s.height
    };
    let pitch = if desc.flags & DDSD_PITCH != 0 {
        desc.pitch
    } else {
        pitch_of(width, format.bits)
    };
    if width == 0 || height == 0 || pitch < (width * format.bits).div_ceil(8) {
        return Some(DDERR_INVALIDPARAMS);
    }
    free_memory(cpu, id);
    if let Some(s) = surface(cpu, id) {
        s.memory = MEM_SYSTEM;
        s.bits = desc.surface;
        s.owns_bits = false;
        s.width = width;
        s.height = height;
        s.pitch = pitch;
        s.format = format;
    }
    changed(cpu, id);
    Some(DD_OK)
}

const DDSPD_IUNKNOWNPOINTER: u32 = 0x1;
const DDSPD_VOLATILE: u32 = 0x2;

/// SetPrivateData(this, tag, data, size, flags).
pub fn set_private_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, tag, data, size, flags] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(surface_of(cpu, this));
    let Some(tag) = Guid::read(cpu, tag) else {
        return Some(DDERR_INVALIDPARAMS);
    };
    if flags & !(DDSPD_IUNKNOWNPOINTER | DDSPD_VOLATILE) != 0 {
        return Some(DDERR_INVALIDPARAMS);
    }
    let bytes = if flags & DDSPD_IUNKNOWNPOINTER != 0 {
        if size != 4 || data == 0 {
            return Some(DDERR_INVALIDPARAMS);
        }
        // The data is the interface pointer itself, which it holds if it
        // is DirectDraw's.
        super::add_ref(cpu, data);
        data.to_le_bytes().to_vec()
    } else {
        if size > 0 && (data == 0 || !cpu.bus.guest_probe(data, size as usize, false)) {
            return Some(DDERR_INVALIDPARAMS);
        }
        let mut bytes = vec![0; size as usize];
        cpu.bus.guest_read_bytes(data, &mut bytes);
        bytes
    };
    remove_private(cpu, id, &tag.0);
    let uniqueness = state(cpu).surface(id).map_or(0, |s| s.uniqueness);
    if let Some(s) = surface(cpu, id) {
        s.private.push(Private {
            tag: tag.0.to_vec(),
            flags,
            data: bytes,
            uniqueness,
        });
    }
    Some(DD_OK)
}

/// Take the private data `tag` away, letting go of an interface it
/// holds: whether there was any.
fn remove_private(cpu: &mut Cpu, id: u32, tag: &[u8]) -> bool {
    let Some(s) = surface(cpu, id) else {
        return false;
    };
    let Some(i) = s.private.iter().position(|p| p.tag == tag) else {
        return false;
    };
    let p = s.private.remove(i);
    if p.flags & DDSPD_IUNKNOWNPOINTER != 0 && p.data.len() == 4 {
        let iface = u32::from_le_bytes([p.data[0], p.data[1], p.data[2], p.data[3]]);
        super::release(cpu, iface);
    }
    true
}

/// GetPrivateData(this, tag, buffer, size).
pub fn get_private_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, tag, buffer, size_at] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(surface_of(cpu, this));
    let Some(tag) = Guid::read(cpu, tag) else {
        return Some(DDERR_INVALIDPARAMS);
    };
    if size_at == 0 || !cpu.bus.guest_probe(size_at, 4, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let Some(s) = state(cpu).surface(id) else {
        return Some(DDERR_INVALIDOBJECT);
    };
    let uniqueness = s.uniqueness;
    let Some(p) = s.private.iter().find(|p| p.tag == tag.0).cloned() else {
        return Some(DDERR_NOTFOUND);
    };
    if p.flags & DDSPD_VOLATILE != 0 && p.uniqueness != uniqueness {
        return Some(super::DDERR_EXPIRED);
    }
    let room = cpu.bus.guest_read_32(size_at);
    cpu.bus.guest_write_32(size_at, p.data.len() as u32);
    if room < p.data.len() as u32 {
        return Some(DDERR_MOREDATA);
    }
    if !p.data.is_empty() {
        if buffer == 0 || !cpu.bus.guest_probe(buffer, p.data.len(), true) {
            return Some(DDERR_INVALIDPARAMS);
        }
        cpu.bus.guest_write_bytes(buffer, &p.data);
    }
    Some(DD_OK)
}

/// FreePrivateData(this, tag).
pub fn free_private_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, id) = hr!(surface_of(cpu, args[0]));
    let Some(tag) = Guid::read(cpu, args[1]) else {
        return Some(DDERR_INVALIDPARAMS);
    };
    Some(if remove_private(cpu, id, &tag.0) {
        DD_OK
    } else {
        DDERR_NOTFOUND
    })
}

/// GetUniquenessValue(this, out).
pub fn get_uniqueness_value(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, id) = hr!(surface_of(cpu, args[0]));
    let value = state(cpu).surface(id).map_or(0, |s| s.uniqueness);
    if !com::write_out(cpu, args[1], value) {
        return Some(DDERR_INVALIDPARAMS);
    }
    Some(DD_OK)
}

/// ChangeUniquenessValue(this).
pub fn change_uniqueness_value(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, id) = hr!(surface_of(cpu, args[0]));
    changed(cpu, id);
    Some(DD_OK)
}

/// Whether the surface `id` is a texture that DirectDraw manages:
/// SetPriority's and SetLOD's.
fn managed(cpu: &mut Cpu, id: u32) -> bool {
    state(cpu)
        .surface(id)
        .is_some_and(|s| s.caps[1] & (DDSCAPS2_TEXTUREMANAGE | DDSCAPS2_D3DTEXTUREMANAGE) != 0)
}

/// SetPriority(this, priority).
pub fn set_priority(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, id) = hr!(surface_of(cpu, args[0]));
    if !managed(cpu, id) {
        return Some(DDERR_INVALIDOBJECT);
    }
    if let Some(s) = surface(cpu, id) {
        s.priority = args[1];
    }
    Some(DD_OK)
}

/// GetPriority(this, out).
pub fn get_priority(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, id) = hr!(surface_of(cpu, args[0]));
    if !managed(cpu, id) {
        return Some(DDERR_INVALIDOBJECT);
    }
    let value = state(cpu).surface(id).map_or(0, |s| s.priority);
    if !com::write_out(cpu, args[1], value) {
        return Some(DDERR_INVALIDPARAMS);
    }
    Some(DD_OK)
}

/// SetLOD(this, lod).
pub fn set_lod(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, id) = hr!(surface_of(cpu, args[0]));
    if !managed(cpu, id) {
        return Some(DDERR_INVALIDOBJECT);
    }
    if let Some(s) = surface(cpu, id) {
        s.lod = args[1];
    }
    Some(DD_OK)
}

/// GetLOD(this, out).
pub fn get_lod(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, id) = hr!(surface_of(cpu, args[0]));
    if !managed(cpu, id) {
        return Some(DDERR_INVALIDOBJECT);
    }
    let value = state(cpu).surface(id).map_or(0, |s| s.lod);
    if !com::write_out(cpu, args[1], value) {
        return Some(DDERR_INVALIDPARAMS);
    }
    Some(DD_OK)
}

/// The rectangle of a surface.
pub fn bounds(s: &Surface) -> gdi32::Rect {
    gdi32::Rect::new(0, 0, s.width as i32, s.height as i32)
}

/// A rectangle a method was given, or the whole surface without one;
/// `DDERR_INVALIDRECT` if it is empty or reaches out of the surface.
pub fn rect_in(cpu: &mut Cpu, s: &Surface, at: u32) -> Result<gdi32::Rect, u32> {
    if at == 0 {
        return Ok(bounds(s));
    }
    if !cpu.bus.guest_probe(at, 16, false) {
        return Err(DDERR_INVALIDPARAMS);
    }
    let r = gdi32::Rect::read(cpu, at);
    if r.left < 0
        || r.top < 0
        || r.left >= r.right
        || r.top >= r.bottom
        || r.right > s.width as i32
        || r.bottom > s.height as i32
    {
        return Err(DDERR_INVALIDRECT);
    }
    Ok(r)
}

crate::state_fields!(Private {
    tag,
    flags,
    data,
    uniqueness
});
crate::state_fields!(Surface {
    caps,
    width,
    height,
    pitch,
    format,
    memory,
    bits,
    owns_bits,
    lost,
    locked,
    dc,
    dc_bitmap,
    dc_old,
    keys,
    key_set,
    palette,
    clipper,
    attached,
    root,
    children,
    version,
    backbuffers,
    mipmaps,
    private,
    uniqueness,
    priority,
    lod
});
