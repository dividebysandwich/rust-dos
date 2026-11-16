//! D3DCAPS8: what the HAL device says it can do, as a Banshee's or
//! Voodoo3's DirectX 8 driver said it with Direct3D's software vertex
//! processing: textures of powers of 2 up to 256 with an aspect ratio
//! of up to 8, one texture unit on a Banshee and two on a Voodoo3, no
//! stencil, cube or volume textures, and vertex shaders 1.1 run on the
//! processor, but no pixel shaders.
//!
//! A ToadForce's are what its HAL describes (`hal::description`): four
//! textures at once in eight stages, textures up to 2048, stencil,
//! anisotropic filtering, every blend factor and the blend operations,
//! and transform and lighting in the card, for which its device takes
//! hardware and mixed vertex processing too. Vertex shaders run on the
//! processor on it as well, so a device with hardware vertex
//! processing alone has none.
//!
//! A device's own caps (IDirect3DDevice8::GetDeviceCaps) add what its
//! software vertex processing does that the card can't: geometry
//! blending of 4 matrices, indexed among 256, and tweening. Windows
//! reports a MaxVertexBlendMatrixIndex of 255 for a software device and
//! MaxVertexBlendMatrices of 4 (Wine bug 39057, from runs of its tests
//! on Windows). The adapter's caps (IDirect3D8::GetDeviceCaps) are the
//! HAL's, and the Voodoo3 blends nothing.

use super::{D3D_OK, D3DERR_INVALIDCALL};
use crate::cpu::Cpu;
use crate::win32::ddraw::d3d::hal;

/// The user's clip planes.
pub const MAX_CLIP_PLANES: u32 = 6;
/// The card draws points one pixel wide.
pub const MAX_POINT_SIZE: f32 = 1.0;

/// D3DCAPS8's size.
const SIZE: usize = 53 * 4;

/// The card of the display adapter that Direct3D 8 has its HAL device
/// on: a ToadForce with level 1, a Banshee or a Voodoo3. None on a
/// machine with another adapter (a Voodoo Graphics or Voodoo2 add-on
/// card has no Direct3D 8 driver).
pub fn card(cpu: &Cpu) -> Option<hal::Kind> {
    hal::adapter(cpu).map(|offer| offer.kind)
}

/// Whose caps `words` gives: the adapter's, or those of a device made
/// with the D3DCREATE_ flags `behavior`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Whose {
    Adapter,
    Device { behavior: u32 },
}

/// D3DCAPS8's words, the adapter's or a device's.
pub fn words(cpu: &Cpu, whose: Whose) -> [u32; 53] {
    // `check_device` and `device::create` let only a machine with a card
    // here ask.
    let offer = hal::adapter(cpu).unwrap_or(hal::Offer {
        kind: hal::Kind::Voodoo,
        units: 2,
    });
    let (kind, tmus) = (offer.kind, offer.units);
    let card = hal::description(kind);
    // The vertex processing Direct3D does on the processor is a
    // device's unless the card alone does its: hardware vertex
    // processing, where the card transforms and nothing runs vertex
    // shaders.
    let software = match whose {
        Whose::Adapter => false,
        Whose::Device { behavior } => {
            behavior & super::device::CREATE_HARDWARE_VERTEXPROCESSING == 0
        }
    };
    let mut w = [0u32; 53];
    // D3DDEVTYPE_HAL, adapter 0.
    w[0] = 1;
    // D3DCAPS_READ_SCANLINE; D3DCAPS2_CANRENDERWINDOWED.
    w[2] = 0x2_0000;
    w[3] = 0x8_0000;
    // D3DPRESENT_INTERVAL_IMMEDIATE and ONE.
    w[5] = 0x8000_0000 | 1;
    // D3DDEVCAPS_: floating point TL vertices, execute buffers, TL
    // vertices and textures in system memory, textures in video memory,
    // DrawPrimitive with TL vertices, rendering after a flip,
    // DrawPrimitives2 and 2EX, and a hardware rasterizer.
    w[7] = 0x1 | 0x10 | 0x40 | 0x100 | 0x200 | 0x400 | 0x800 | 0x2000 | 0x8000 | 0x8_0000;
    // D3DPMISCCAPS_: masking z, culling none, clockwise and
    // counterclockwise.
    w[8] = 0x2 | 0x10 | 0x20 | 0x40;
    // D3DPRASTERCAPS_: dither, z test, vertex and table fog, mipmap LOD
    // bias, z bias, range fog, and what the card adds: fog by w on
    // both cards, and a w buffer on a Voodoo.
    w[9] = 0x1 | 0x10 | 0x80 | 0x100 | 0x2000 | 0x4000 | 0x1_0000;
    w[9] |= card.raster | card.raster8;
    // D3DPCMPCAPS_: all eight, for depth and alpha.
    w[10] = 0xFF;
    w[13] = 0xFF;
    // D3DPBLENDCAPS_: as Direct3D 7's HAL has them.
    w[11] = 0x1 | 0x2 | 0x4 | 0x8 | 0x10 | 0x20 | 0x100 | 0x200 | 0x400 | card.blend;
    w[12] = 0x1 | 0x2 | 0x4 | 0x8 | 0x10 | 0x20 | card.blend & 0x7FF;
    // D3DPSHADECAPS_: Gouraud color, specular, alpha blending, fog.
    w[14] = 0x8 | 0x200 | 0x4000 | 0x8_0000;
    // D3DPTEXTURECAPS_: perspective, powers of 2, alpha, alpha in
    // palettes, mipmaps, and what the card adds. A card with cube
    // textures has them with mipmaps (MIPCUBEMAP, 0x1_0000) and with
    // edges that are powers of 2 (CUBEMAP_POW2, 0x2_0000), which is
    // what CreateCubeTexture makes.
    w[15] = 0x1 | 0x2 | 0x4 | 0x80 | 0x4000 | card.texture;
    if card.texture & hal::TEXTURECAPS_CUBEMAP != 0 {
        w[15] |= 0x1_0000 | 0x2_0000;
    }
    // D3DPTFILTERCAPS_: point and linear minification, mipmap and
    // magnification filters.
    w[16] = 0x100 | 0x200 | 0x1_0000 | 0x2_0000 | 0x100_0000 | 0x200_0000 | card.filter;
    // CubeTextureFilterCaps: a card filters a cube texture's faces as
    // it does a texture, and a caps of 0 would tell a program that it
    // can't filter them at all.
    if card.texture & hal::TEXTURECAPS_CUBEMAP != 0 {
        w[17] = w[16];
    }
    // D3DPTADDRESSCAPS_: wrap, mirror, clamp, independent u and v.
    w[19] = 0x1 | 0x2 | 0x4 | 0x10 | card.address;
    // D3DLINECAPS_: texture, z test, blending, alpha test, fog.
    w[21] = 0x1F;
    w[22] = card.texture_max;
    w[23] = card.texture_max;
    w[25] = card.texture_max;
    w[26] = card.texture_aspect;
    w[27] = card.anisotropy;
    w[28] = 65536.0f32.to_bits();
    // The guard band, as Direct3D 7's HAL has it.
    for (i, g) in card.guard_band.iter().enumerate() {
        w[29 + i] = g.to_bits();
    }
    w[34] = card.stencil;
    // Eight sets of texture coordinates.
    w[35] = 8;
    // D3DTEXOPCAPS_: the operations the HAL draws, which ValidateDevice
    // accepts (hal::can_do).
    w[36] = card.ops | card.ops8;
    w[37] = if card.stages == 0 { tmus } else { card.stages };
    w[38] = tmus;
    // D3DVTXPCAPS_: texture generation, material sources, directional
    // and positional lights, the local viewer.
    w[39] = 0x1 | 0x2 | 0x8 | 0x10 | 0x20;
    w[40] = 8;
    w[41] = MAX_CLIP_PLANES;
    if hal::transforms(kind) {
        // D3DDEVCAPS_HWTRANSFORMANDLIGHT. D3DPMISCCAPS_: the color
        // write mask (0x80), D3DTA_TEMP (TSSARGTEMP, 0x400) and the
        // blend operations (0x800), which the card's HAL draws.
        w[7] |= 0x1_0000;
        w[8] |= 0x80 | 0x400 | 0x800;
        // The matrices the card blends a vertex between, which it
        // doesn't pick by index.
        w[42] = crate::toadforce::caps::BLEND_MATRICES;
    }
    if software {
        // D3DVTXPCAPS_TWEENING; MaxVertexBlendMatrices and
        // MaxVertexBlendMatrixIndex.
        w[39] |= 0x40;
        w[42] = 4;
        w[43] = 255;
    }
    w[44] = MAX_POINT_SIZE.to_bits();
    w[45] = 0xF_FFFF;
    w[46] = 0xFF_FFFF;
    w[47] = 16;
    w[48] = 256;
    // vs.1.1 with 96 constants where Direct3D runs vertex shaders,
    // which is on the processor: every device of a card that doesn't
    // transform, and the adapter's caps of one. No pixel shaders.
    if software || !hal::transforms(kind) {
        w[49] = 0xFFFE_0101;
        w[50] = 96;
    }
    w
}

/// Write D3DCAPS8 at `at`: the adapter's or a device's.
pub fn write(cpu: &mut Cpu, at: u32, whose: Whose) -> u32 {
    if at == 0 || !cpu.bus.guest_probe(at, SIZE, true) {
        return D3DERR_INVALIDCALL;
    }
    let w = words(cpu, whose);
    for (i, v) in w.iter().enumerate() {
        cpu.bus.guest_write_32(at + 4 * i as u32, *v);
    }
    D3D_OK
}
