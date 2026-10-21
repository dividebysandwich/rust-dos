//! What the HAL device says it can do: D3DDEVICEDESC (Direct3D 3, 5
//! and 6's sizes) and D3DDEVICEDESC7, and the texture and depth buffer
//! formats. They describe a Voodoo as 3dfx's Direct3D driver did: 16-bit
//! render targets and depth buffers, textures from 1 to 256 pixels a
//! side in powers of 2 with an aspect ratio of up to 8, one texture unit
//! on a Banshee and two on a Voodoo3, and the rest done by Direct3D on
//! the processor (the "HEL" description: transform, lighting and
//! clipping).

use super::super::Format;
use super::super::surface::{DDPF_ALPHAPIXELS, DDPF_PALETTEINDEXED8, DDPF_RGB, DDPF_ZBUFFER};
use super::{IID_HAL, write_dwords, write_f32};
use crate::cpu::Cpu;

/// The sizes of D3DDEVICEDESC: Direct3D 3's, 5's and 6's.
pub const DESC_V1: u32 = 172;
pub const DESC_V2: u32 = 204;
pub const DESC_V3: u32 = 252;
pub const DESC7: u32 = 236;

pub const DESCRIPTION: &[u8] = b"Microsoft Direct3D Hardware acceleration through Direct3D HAL\0";
pub const NAME: &[u8] = b"Direct3D HAL\0";

// D3DDEVICEDESC's flags.
const D3DDD_COLORMODEL: u32 = 0x1;
const D3DDD_DEVCAPS: u32 = 0x2;
const D3DDD_TRANSFORMCAPS: u32 = 0x4;
const D3DDD_LIGHTINGCAPS: u32 = 0x8;
const D3DDD_BCLIPPING: u32 = 0x10;
const D3DDD_LINECAPS: u32 = 0x20;
const D3DDD_TRICAPS: u32 = 0x40;
const D3DDD_DEVICERENDERBITDEPTH: u32 = 0x80;
const D3DDD_DEVICEZBUFFERBITDEPTH: u32 = 0x100;
const D3DDD_MAXBUFFERSIZE: u32 = 0x200;
const D3DDD_MAXVERTEXCOUNT: u32 = 0x400;

const D3DCOLOR_RGB: u32 = 2;

/// D3DDEVCAPS_: floating point TL vertices, execute buffers, TL vertices
/// and textures in system memory, textures in video memory, DrawPrimitive
/// with TL vertices, rendering after a flip, DrawPrimitives2, and a
/// hardware rasterizer.
const DEVCAPS: u32 = 0x1 | 0x10 | 0x40 | 0x100 | 0x200 | 0x400 | 0x800 | 0x2000 | 0x8_0000;

/// D3DPRIMCAPS's words after its size, for lines and triangles.
fn prim_caps() -> [u32; 13] {
    // D3DPMISCCAPS_: masking z, culling none, clockwise, counterclockwise.
    let misc = 0x2 | 0x10 | 0x20 | 0x40;
    // D3DPRASTERCAPS_: dither, z test, subpixel, vertex and table fog,
    // mipmap LOD bias, z bias, range fog, w buffer, w fog.
    let raster =
        0x1 | 0x10 | 0x20 | 0x80 | 0x100 | 0x2000 | 0x4000 | 0x1_0000 | 0x4_0000 | 0x10_0000;
    // D3DPCMPCAPS_: all eight.
    let cmp = 0xFF;
    // D3DPBLENDCAPS_: zero, one, the source's color and its inverse,
    // the source's alpha and its inverse, the destination's alpha and
    // its inverse, the destination's color and its inverse, and the
    // saturated source alpha.
    let src_blend = 0x1 | 0x2 | 0x4 | 0x8 | 0x10 | 0x20 | 0x100 | 0x200 | 0x400;
    let dest_blend = 0x1 | 0x2 | 0x4 | 0x8 | 0x10 | 0x20;
    // D3DPSHADECAPS_: Gouraud color, specular, alpha, fog.
    let shade = 0x8 | 0x200 | 0x4000 | 0x8_0000 | 0x2 | 0x80;
    // D3DPTEXTURECAPS_: perspective, powers of 2, alpha, transparency,
    // repeat not scaled by size, alpha in palettes, color key blending.
    let texture = 0x1 | 0x2 | 0x4 | 0x8 | 0x40 | 0x80 | 0x1000;
    // D3DPTFILTERCAPS_: the old nearest, linear and mipmapped ones, and
    // point and linear minification, magnification and mip filtering.
    let filter = 0x1
        | 0x2
        | 0x4
        | 0x8
        | 0x10
        | 0x20
        | 0x100
        | 0x200
        | 0x1_0000
        | 0x2_0000
        | 0x100_0000
        | 0x200_0000;
    // D3DPTBLENDCAPS_: decal, modulate, decal alpha, modulate alpha,
    // copy and add.
    let blend = 0x1 | 0x2 | 0x4 | 0x8 | 0x40 | 0x80;
    // D3DPTADDRESSCAPS_: wrap, mirror, clamp, independent u and v.
    let address = 0x1 | 0x2 | 0x4 | 0x10;
    [
        misc, raster, cmp, src_blend, dest_blend, cmp, shade, texture, filter, blend, address, 0, 0,
    ]
}

/// D3DTEXTUREOPCAPS_: disable, select argument 1 and 2, modulate and
/// modulate 2x, add, and blending by diffuse, texture and current alpha.
const TEXTURE_OPS: u32 = 0x1 | 0x2 | 0x4 | 0x8 | 0x10 | 0x40 | 0x400 | 0x800 | 0x4000;

/// DDBD_16: the render target's and depth buffer's depth.
const DDBD_16: u32 = 0x400;

/// The guard band: left, top, right, bottom.
pub const GUARD_BAND: [f32; 4] = [-2048.0, -2048.0, 2048.0, 2048.0];

/// Write the device's description, `size` bytes of D3DDEVICEDESC at
/// `at` (the caller has probed them): the hardware's (`hal`), or what
/// Direct3D does in software for it.
pub fn write_desc(cpu: &mut Cpu, at: u32, size: u32, hal: bool, tmus: u32) {
    cpu.bus.guest_fill(at, size as usize, 0);
    let mut words = vec![0u32; 252 / 4];
    words[0] = size;
    if hal {
        words[1] = D3DDD_COLORMODEL
            | D3DDD_DEVCAPS
            | D3DDD_TRANSFORMCAPS
            | D3DDD_LIGHTINGCAPS
            | D3DDD_BCLIPPING
            | D3DDD_LINECAPS
            | D3DDD_TRICAPS
            | D3DDD_DEVICERENDERBITDEPTH
            | D3DDD_DEVICEZBUFFERBITDEPTH
            | D3DDD_MAXBUFFERSIZE
            | D3DDD_MAXVERTEXCOUNT;
        words[2] = D3DCOLOR_RGB;
        words[3] = DEVCAPS;
        words[4] = 8;
        words[7] = 16;
        for base in [0x2C / 4, 0x64 / 4] {
            words[base] = 56;
            words[base + 1..base + 14].copy_from_slice(&prim_caps());
        }
        words[0x9C / 4] = DDBD_16;
        words[0xA0 / 4] = DDBD_16;
        words[0xA8 / 4] = 0xFFFF;
        // Direct3D 5's: the textures' sizes.
        words[0xAC / 4] = 1;
        words[0xB0 / 4] = 1;
        words[0xB4 / 4] = 256;
        words[0xB8 / 4] = 256;
        // Direct3D 6's.
        words[0xCC / 4] = 256;
        words[0xD0 / 4] = 8;
        words[0xD4 / 4] = 1;
        for (i, g) in GUARD_BAND.iter().enumerate() {
            words[0xD8 / 4 + i] = g.to_bits();
        }
        words[0xF0 / 4] = tmus;
        words[0xF4 / 4] = TEXTURE_OPS;
        words[0xF8 / 4] = tmus | tmus << 16;
    } else {
        words[1] = D3DDD_COLORMODEL | D3DDD_TRANSFORMCAPS | D3DDD_LIGHTINGCAPS | D3DDD_BCLIPPING;
        words[2] = D3DCOLOR_RGB;
        // D3DTRANSFORMCAPS_CLIP, clipping, and the lights: point, spot,
        // directional and parallel point, in the RGB model, as many as
        // the program likes.
        words[4] = 8;
        words[5] = 0x1;
        words[6] = 1;
        words[7] = 16;
        words[8] = 0xF;
        words[9] = 0x1;
        words[10] = 0;
        for base in [0x2C / 4, 0x64 / 4] {
            words[base] = 56;
        }
    }
    let n = (size.min(252) / 4) as usize;
    write_dwords(cpu, at, &words[..n]);
}

/// Write D3DDEVICEDESC7 at `at` (the caller has probed it).
pub fn write_desc7(cpu: &mut Cpu, at: u32, tmus: u32) {
    let mut words = vec![0u32; DESC7 as usize / 4];
    words[0] = DEVCAPS;
    for base in [1, 0x3C / 4] {
        words[base] = 56;
        words[base + 1..base + 14].copy_from_slice(&prim_caps());
    }
    words[0x74 / 4] = DDBD_16;
    words[0x78 / 4] = DDBD_16;
    words[0x7C / 4] = 1;
    words[0x80 / 4] = 1;
    words[0x84 / 4] = 256;
    words[0x88 / 4] = 256;
    words[0x8C / 4] = 256;
    words[0x90 / 4] = 8;
    words[0x94 / 4] = 1;
    for (i, g) in GUARD_BAND.iter().enumerate() {
        words[0x98 / 4 + i] = g.to_bits();
    }
    words[0xB0 / 4] = tmus;
    words[0xB4 / 4] = TEXTURE_OPS;
    words[0xB8 / 4] = tmus | tmus << 16;
    words[0xBC / 4] = 0xFFFF_FFFF;
    write_dwords(cpu, at, &words);
    write_f32(cpu, at + 0xC0, 65536.0);
    IID_HAL.write(cpu, at + 0xC4);
    // Six clip planes of the user's, no vertex blending; the vertex
    // processing caps: texture generation, material sources, vertex
    // fog, directional and positional lights and the local viewer.
    cpu.bus.guest_write_32(at + 0xD4, 6);
    cpu.bus.guest_write_32(at + 0xD8, 0x3E);
}

/// The texture formats: 565, 1555, 4444 and 8-bit palettized.
pub fn texture_formats() -> Vec<Format> {
    let rgb = |a: u32, r: u32, g: u32, b: u32| Format {
        flags: DDPF_RGB | if a != 0 { DDPF_ALPHAPIXELS } else { 0 },
        fourcc: 0,
        bits: 16,
        r,
        g,
        b,
        a,
    };
    vec![
        rgb(0, 0xF800, 0x07E0, 0x001F),
        rgb(0x8000, 0x7C00, 0x03E0, 0x001F),
        rgb(0xF000, 0x0F00, 0x00F0, 0x000F),
        Format {
            flags: DDPF_RGB | DDPF_PALETTEINDEXED8,
            bits: 8,
            ..Format::default()
        },
    ]
}

/// The depth buffer formats: 16 bits.
pub fn z_formats() -> Vec<Format> {
    vec![Format {
        flags: DDPF_ZBUFFER,
        bits: 16,
        g: 0xFFFF,
        ..Format::default()
    }]
}
