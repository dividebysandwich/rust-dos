//! D3DCAPS8: what the HAL device says it can do, as a Banshee's or
//! Voodoo3's DirectX 8 driver said it with Direct3D's software vertex
//! processing: textures of powers of 2 up to 256 with an aspect ratio
//! of up to 8, one texture unit on a Banshee and two on a Voodoo3, no
//! stencil, cube or volume textures, and vertex shaders 1.1 run on the
//! processor, but no pixel shaders.

use super::{D3D_OK, D3DERR_INVALIDCALL};
use crate::cpu::Cpu;

/// The user's clip planes.
pub const MAX_CLIP_PLANES: u32 = 6;
/// The card draws points one pixel wide.
pub const MAX_POINT_SIZE: f32 = 1.0;

/// D3DCAPS8's size.
const SIZE: usize = 53 * 4;

/// The texture units of the card that is the display adapter.
pub fn tmus(cpu: &Cpu) -> u32 {
    match cpu.bus.vga.adapter.voodoo_board() {
        Some(crate::voodoo::Board::Banshee) => 1,
        _ => 2,
    }
}

/// D3DCAPS8's words.
pub fn words(cpu: &Cpu) -> [u32; 53] {
    let tmus = tmus(cpu);
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
    // bias, z bias, range fog, w buffer, w fog.
    w[9] = 0x1 | 0x10 | 0x80 | 0x100 | 0x2000 | 0x4000 | 0x1_0000 | 0x4_0000 | 0x10_0000;
    // D3DPCMPCAPS_: all eight, for depth and alpha.
    w[10] = 0xFF;
    w[13] = 0xFF;
    // D3DPBLENDCAPS_: as Direct3D 7's HAL has them.
    w[11] = 0x1 | 0x2 | 0x4 | 0x8 | 0x10 | 0x20 | 0x100 | 0x200 | 0x400;
    w[12] = 0x1 | 0x2 | 0x4 | 0x8 | 0x10 | 0x20;
    // D3DPSHADECAPS_: Gouraud color, specular, alpha blending, fog.
    w[14] = 0x8 | 0x200 | 0x4000 | 0x8_0000;
    // D3DPTEXTURECAPS_: perspective, powers of 2, alpha, alpha in
    // palettes, mipmaps.
    w[15] = 0x1 | 0x2 | 0x4 | 0x80 | 0x4000;
    // D3DPTFILTERCAPS_: point and linear minification, mipmap and
    // magnification filters.
    w[16] = 0x100 | 0x200 | 0x1_0000 | 0x2_0000 | 0x100_0000 | 0x200_0000;
    // D3DPTADDRESSCAPS_: wrap, mirror, clamp, independent u and v.
    w[19] = 0x1 | 0x2 | 0x4 | 0x10;
    // D3DLINECAPS_: texture, z test, blending, alpha test, fog.
    w[21] = 0x1F;
    w[22] = 256;
    w[23] = 256;
    w[25] = 256;
    w[26] = 8;
    w[27] = 1;
    w[28] = 65536.0f32.to_bits();
    // The guard band, as Direct3D 7's HAL has it.
    for (i, g) in [-2048.0f32, -2048.0, 2048.0, 2048.0].iter().enumerate() {
        w[29 + i] = g.to_bits();
    }
    // Eight sets of texture coordinates.
    w[35] = 8;
    // D3DTEXOPCAPS_: what the card's combine units do (hal::can_do):
    // disable, select argument 1 and 2, modulate, add, subtract, and
    // blending by the diffuse and the texture alpha.
    w[36] = 0x1 | 0x2 | 0x4 | 0x8 | 0x40 | 0x200 | 0x800 | 0x1000;
    w[37] = tmus;
    w[38] = tmus;
    // D3DVTXPCAPS_: texture generation, material sources, directional
    // and positional lights, the local viewer.
    w[39] = 0x1 | 0x2 | 0x8 | 0x10 | 0x20;
    w[40] = 8;
    w[41] = MAX_CLIP_PLANES;
    w[44] = MAX_POINT_SIZE.to_bits();
    w[45] = 0xF_FFFF;
    w[46] = 0xFF_FFFF;
    w[47] = 16;
    w[48] = 256;
    // vs.1.1, 96 constants; no pixel shaders.
    w[49] = 0xFFFE_0101;
    w[50] = 96;
    w
}

/// Write D3DCAPS8 at `at`.
pub fn write(cpu: &mut Cpu, at: u32) -> u32 {
    if at == 0 || !cpu.bus.guest_probe(at, SIZE, true) {
        return D3DERR_INVALIDCALL;
    }
    let w = words(cpu);
    for (i, v) in w.iter().enumerate() {
        cpu.bus.guest_write_32(at + 4 * i as u32, *v);
    }
    D3D_OK
}
