//! What the HAL device says it can do: D3DDEVICEDESC (Direct3D 3, 5
//! and 6's sizes) and D3DDEVICEDESC7. What every HAL here has is in
//! this file; what differs from card to card comes from the card's HAL
//! (`hal::Description`): the depths and sizes it draws, the formats,
//! the stage operations and the filters. The rest is done by Direct3D
//! on the processor (the "HEL" description: transform, lighting and
//! clipping).

use super::hal::{self, Description, Offer};
use super::{IID_HAL, IID_TNLHAL, write_dwords, write_f32};
use crate::cpu::Cpu;

/// The sizes of D3DDEVICEDESC: Direct3D 3's, 5's and 6's.
pub const DESC_V1: u32 = 172;
pub const DESC_V2: u32 = 204;
pub const DESC_V3: u32 = 252;
pub const DESC7: u32 = 236;

pub const DESCRIPTION: &[u8] = b"Microsoft Direct3D Hardware acceleration through Direct3D HAL\0";
pub const NAME: &[u8] = b"Direct3D HAL\0";

/// The same of the T&L HAL device, which a card that transforms and
/// lights vertices has too (Direct3D 7).
pub const TNL_DESCRIPTION: &[u8] =
    b"Microsoft Direct3D Hardware Transform and Lighting acceleration capable device\0";
pub const TNL_NAME: &[u8] = b"Direct3D T&L HAL\0";

/// D3DDEVCAPS_HWTRANSFORMANDLIGHT.
const DEVCAPS_TNL: u32 = 0x1_0000;

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
fn prim_caps(d: &Description) -> [u32; 13] {
    // D3DPMISCCAPS_: masking z, culling none, clockwise, counterclockwise.
    let misc = 0x2 | 0x10 | 0x20 | 0x40;
    // D3DPRASTERCAPS_: dither, z test, subpixel, vertex and table fog,
    // mipmap LOD bias, z bias, range fog, and the card's own.
    let raster = 0x1 | 0x10 | 0x20 | 0x80 | 0x100 | 0x2000 | 0x4000 | 0x1_0000 | d.raster;
    // D3DPCMPCAPS_: all eight.
    let cmp = 0xFF;
    // D3DPBLENDCAPS_: zero, one, the source's color and its inverse,
    // the source's alpha and its inverse, the destination's alpha and
    // its inverse, the destination's color and its inverse, and the
    // saturated source alpha.
    let src_blend = 0x1 | 0x2 | 0x4 | 0x8 | 0x10 | 0x20 | 0x100 | 0x200 | 0x400 | d.blend;
    // The destination has no factor that sets both (BOTHSRCALPHA and
    // BOTHINVSRCALPHA, 0x800 and 0x1000).
    let dest_blend = 0x1 | 0x2 | 0x4 | 0x8 | 0x10 | 0x20 | d.blend & 0x7FF;
    // D3DPSHADECAPS_: Gouraud color, specular, alpha, fog.
    let shade = 0x8 | 0x200 | 0x4000 | 0x8_0000 | 0x2 | 0x80;
    // D3DPTEXTURECAPS_: perspective, powers of 2, alpha, transparency,
    // repeat not scaled by size, alpha in palettes, and the card's own,
    // color key blending among them where it has it.
    let texture = 0x1 | 0x2 | 0x4 | 0x8 | 0x40 | 0x80 | d.texture | d.texture7;
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
        | 0x200_0000
        | d.filter;
    // D3DPTBLENDCAPS_: decal, modulate, decal alpha, modulate alpha,
    // copy and add.
    let blend = 0x1 | 0x2 | 0x4 | 0x8 | 0x40 | 0x80;
    // D3DPTADDRESSCAPS_: wrap, mirror, clamp, independent u and v.
    let address = 0x1 | 0x2 | 0x4 | 0x10 | d.address;
    [
        misc, raster, cmp, src_blend, dest_blend, cmp, shade, texture, filter, blend, address, 0, 0,
    ]
}

/// The blend stages and the textures at once, as the two words of
/// wMaxTextureBlendStages and wMaxSimultaneousTextures.
fn stages(d: &Description, offer: Offer) -> u32 {
    // A card with a stage for each texture unit says 0 stages.
    let stages = if d.stages == 0 { offer.units } else { d.stages };
    stages | offer.units << 16
}

/// Write the device's description, `size` bytes of D3DDEVICEDESC at
/// `at` (the caller has probed them): the hardware's, as `offer`'s card
/// has it, or with none what Direct3D does in software for it.
pub fn write_desc(cpu: &mut Cpu, at: u32, size: u32, offer: Option<Offer>) {
    cpu.bus.guest_fill(at, size as usize, 0);
    let mut words = vec![0u32; 252 / 4];
    words[0] = size;
    if let Some(offer) = offer {
        let d = hal::description(offer.kind);
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
            words[base + 1..base + 14].copy_from_slice(&prim_caps(&d));
        }
        words[0x9C / 4] = d.target_depths;
        words[0xA0 / 4] = d.z_depths;
        words[0xA8 / 4] = 0xFFFF;
        // Direct3D 5's: the textures' sizes.
        words[0xAC / 4] = 1;
        words[0xB0 / 4] = 1;
        words[0xB4 / 4] = d.texture_max;
        words[0xB8 / 4] = d.texture_max;
        // Direct3D 6's.
        words[0xCC / 4] = d.texture_max;
        words[0xD0 / 4] = d.texture_aspect;
        words[0xD4 / 4] = d.anisotropy;
        for (i, g) in d.guard_band.iter().enumerate() {
            words[0xD8 / 4 + i] = g.to_bits();
        }
        words[0xEC / 4] = d.stencil;
        words[0xF0 / 4] = offer.units;
        words[0xF4 / 4] = d.ops;
        words[0xF8 / 4] = stages(&d, offer);
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

/// Write D3DDEVICEDESC7 at `at` (the caller has probed it), as
/// `offer`'s card has it: its HAL device's, or with `tnl` its T&L
/// HAL device's, which says the card transforms and lights and how
/// many lights it has on at once.
pub fn write_desc7(cpu: &mut Cpu, at: u32, offer: Offer, tnl: bool) {
    let d = hal::description(offer.kind);
    let mut words = vec![0u32; DESC7 as usize / 4];
    words[0] = DEVCAPS | if tnl { DEVCAPS_TNL } else { 0 };
    for base in [1, 0x3C / 4] {
        words[base] = 56;
        words[base + 1..base + 14].copy_from_slice(&prim_caps(&d));
    }
    words[0x74 / 4] = d.target_depths;
    words[0x78 / 4] = d.z_depths;
    words[0x7C / 4] = 1;
    words[0x80 / 4] = 1;
    words[0x84 / 4] = d.texture_max;
    words[0x88 / 4] = d.texture_max;
    words[0x8C / 4] = d.texture_max;
    words[0x90 / 4] = d.texture_aspect;
    words[0x94 / 4] = d.anisotropy;
    for (i, g) in d.guard_band.iter().enumerate() {
        words[0x98 / 4 + i] = g.to_bits();
    }
    words[0xAC / 4] = d.stencil;
    words[0xB0 / 4] = offer.units;
    words[0xB4 / 4] = d.ops;
    words[0xB8 / 4] = stages(&d, offer);
    // The lights on at once: the card's where it lights, any number
    // where the processor does.
    let lights = hal::tnl_lights(offer.kind).filter(|_| tnl);
    words[0xBC / 4] = lights.unwrap_or(0xFFFF_FFFF);
    write_dwords(cpu, at, &words);
    write_f32(cpu, at + 0xC0, 65536.0);
    let guid = if tnl { IID_TNLHAL } else { IID_HAL };
    guid.write(cpu, at + 0xC4);
    // wMaxUserClipPlanes and wMaxVertexBlendMatrices: six clip planes
    // of the user's, and the world matrices a card that transforms
    // blends a vertex between. A card that doesn't reports no vertex
    // blending, as the Voodoo3's driver did. Then the vertex processing
    // caps: texture generation, material sources, vertex fog,
    // directional and positional lights and the local viewer.
    let blend = match hal::transforms(offer.kind) {
        true => crate::toadforce::caps::BLEND_MATRICES,
        false => 0,
    };
    cpu.bus.guest_write_32(at + 0xD4, 6 | blend << 16);
    cpu.bus.guest_write_32(at + 0xD8, 0x3F);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voodoo::Board;

    const DDBD_16: u32 = 0x400;
    const VOODOO: hal::Kind = hal::Kind::Voodoo;

    fn voodoo3() -> Offer {
        Offer {
            kind: VOODOO,
            units: 2,
        }
    }

    const AT: u32 = 0x2_0000;

    fn read(cpu: &mut Cpu, offset: u32) -> u32 {
        cpu.bus.guest_read_32(AT + offset)
    }

    /// 3dfx's Voodoo3 driver for Direct3D 7 reports 16-bit render
    /// targets and depth buffers alone, so a program that picks its
    /// depths from the description draws in 16 bits.
    #[test]
    fn a_voodoo3_reports_16_bit_targets_and_depth_buffers_alone() {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        cpu.bus.configure_voodoo(Some(Board::Voodoo3));
        assert!(crate::win32::glide::hw::banshee(&cpu));

        write_desc7(&mut cpu, AT, voodoo3(), false);
        assert_eq!(read(&mut cpu, 0x74), DDBD_16, "render target depths");
        assert_eq!(read(&mut cpu, 0x78), DDBD_16, "depth buffer depths");
        // Six clip planes, and no vertex blending on a card that
        // doesn't transform.
        assert_eq!(read(&mut cpu, 0xD4), 6, "clip planes and blend matrices");

        write_desc(&mut cpu, AT, DESC_V3, Some(voodoo3()));
        assert_eq!(read(&mut cpu, 0x9C), DDBD_16, "render target depths");
        assert_eq!(read(&mut cpu, 0xA0), DDBD_16, "depth buffer depths");

        let z = hal::description(VOODOO).z_formats;
        let z: Vec<u32> = z.iter().map(|f| f.bits).collect();
        assert_eq!(z, [16]);
    }

    /// 3dfx's Voodoo3 Direct3D 7 driver's dwTextureOpCaps, as
    /// Giants: Citizen Kabuto's renderer got them on Windows 98 SE.
    const VOODOO3_DRIVER: u32 = 0x1E_FA5F;

    #[test]
    fn the_texture_op_caps_are_what_validate_device_accepts() {
        // D3DTOP_DISABLE (1) to D3DTOP_LERP (26), on each card, for a
        // device of Direct3D 7 and one of Direct3D 8, whose caps have
        // the operations Direct3D 8 added too.
        for kind in [VOODOO, hal::Kind::ToadForce] {
            let d = hal::description(kind);
            for (version, ops) in [(7, d.ops), (8, d.ops | d.ops8)] {
                for op in 1..=26 {
                    assert_eq!(
                        ops & 1 << (op - 1) != 0,
                        hal::can_do(kind, op, version),
                        "{kind:?}, Direct3D {version}: D3DTOP {op}"
                    );
                }
                assert!(!hal::can_do(kind, 0, version) && !hal::can_do(kind, 27, version));
            }
        }
    }

    /// What a ToadForce's descriptions say against what the card
    /// does: its capability table (`toadforce::caps`) and the states
    /// its HAL sends.
    #[test]
    fn a_toadforces_caps_are_what_the_card_does() {
        let d = hal::description(hal::Kind::ToadForce);
        // Table fog by w and by z and anisotropy, and no w buffer:
        // `ZENABLE` has no value for one.
        assert_eq!(d.raster & 0x4_0000, 0, "D3DPRASTERCAPS_WBUFFER");
        assert_eq!(d.raster & 0x30_0000, 0x30_0000, "WFOG and ZFOG");
        // Projected coordinates and cube maps, and no blend for a
        // colour-keyed texel.
        assert_eq!(d.texture & 0xC00, 0xC00, "PROJECTED and CUBEMAP");
        assert_eq!((d.texture | d.texture7) & 0x1000, 0, "COLORKEYBLEND");
        // Direct3D 7 has the operations up to DOTPRODUCT3 (24), and
        // Direct3D 8 adds MULTIPLYADD and LERP.
        assert_eq!(d.ops & 0xFF00_0000, 0);
        assert_eq!(d.ops8, 0x300_0000);
        // Every blend factor as the source's, and as the destination's
        // all but the two that set both.
        let caps = prim_caps(&d);
        assert_eq!(caps[3] & 0x1FFF, 0x1FFF, "source factors");
        assert_eq!(caps[4] & 0x1FFF, 0x7FF, "destination factors");
    }

    #[test]
    fn the_texture_op_caps_are_the_drawn_part_of_3dfxs() {
        // Disable, select argument 1 and 2, modulate, add, subtract, and
        // blending by diffuse, texture and current alpha. Not ADDSMOOTH
        // (0x400), which 3dfx's driver doesn't report, or MODULATE2X
        // (0x10) and BLENDTEXTUREALPHAPM (0x4000), which the HAL doesn't
        // draw.
        let ops = hal::description(VOODOO).ops;
        assert_eq!(ops, 0x9A4F);
        assert_eq!(ops & !VOODOO3_DRIVER, 0);
    }

    /// A program that makes its depth buffer with a depth the ToadForce
    /// reports, by DDSD_ZBUFFERBITDEPTH, gets one the card draws with:
    /// 24 isn't among them, as a 24-bit buffer has no card format.
    #[test]
    fn every_depth_buffer_depth_a_toadforce_reports_is_one_its_hal_takes() {
        use super::super::super::Format;
        use super::super::super::surface::DDPF_ZBUFFER;
        let d = hal::description(hal::Kind::ToadForce);
        // DDBD_16, DDBD_24 and DDBD_32.
        for (bit, bits) in [(0x400, 16), (0x200, 24), (0x100, 32)] {
            let format = Format {
                flags: DDPF_ZBUFFER,
                bits,
                ..Format::default()
            };
            let takes = hal::toadforce::takes_depth(&format, true);
            assert_eq!(d.z_depths & bit != 0, takes, "{bits} bits");
        }
        for f in &d.z_formats {
            assert!(hal::toadforce::takes_depth(f, true), "{f:?}");
        }
    }
}
