//! SetDeviceGammaRamp and GetDeviceGammaRamp: the display's gamma ramp,
//! which games set for brightness.
//!
//! On a Banshee or Voodoo3 the ramp is the card's desktop lookup table,
//! which the video processor runs 16- and 32-bit pixels through when
//! its bypass is off, as 3dfx's driver had it. A mode set turns the
//! bypass back on (`screen.rs`), so a ramp lasts until the mode
//! changes. Other adapters, and 8-bit modes whose table is the palette,
//! have no ramp: SetDeviceGammaRamp fails there.

use crate::cpu::Cpu;
use crate::voodoo::banshee as card;

/// The ramp's size: 256 red, then green, then blue words.
const SIZE: usize = 3 * 256 * 2;

/// The desktop's video processor configuration, if its pixels go
/// through the lookup table that has the ramp (16 bits or more).
fn ramp_config(cpu: &Cpu) -> Option<u32> {
    let cfg = cpu.bus.banshee()?.reg(card::VID_PROC_CFG);
    let format = cfg >> 18 & 7;
    (format != 0 && cfg & card::VIDPROC_DESKTOP_CLUT_SEL == 0).then_some(cfg)
}

/// SetDeviceGammaRamp(hdc, ramp).
pub fn set_device_gamma_ramp(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let ramp = args[1];
    let Some(cfg) = ramp_config(cpu) else {
        return Some(0);
    };
    if ramp == 0 || !cpu.bus.guest_probe(ramp, SIZE, false) {
        return Some(0);
    }
    let mut bytes = [0u8; SIZE];
    cpu.bus.guest_read_bytes(ramp, &mut bytes);
    let word = |i: usize| u16::from_le_bytes([bytes[2 * i], bytes[2 * i + 1]]) as u32;
    let mut identity = true;
    for i in 0..256 {
        let [r, g, b] = [word(i), word(256 + i), word(512 + i)].map(|v| v >> 8);
        identity &= r == i as u32 && g == i as u32 && b == i as u32;
        cpu.bus.voodoo_write_32(card::DAC_ADDR, i as u32);
        cpu.bus
            .voodoo_write_32(card::DAC_DATA, r << 16 | g << 8 | b);
    }
    // The identity ramp is the table bypassed, as after a mode set.
    let cfg = if identity {
        cfg | card::VIDPROC_DESKTOP_CLUT_BYPASS
    } else {
        cfg & !card::VIDPROC_DESKTOP_CLUT_BYPASS
    };
    cpu.bus.voodoo_write_32(card::VID_PROC_CFG, cfg);
    cpu.bus.vga.mark_dirty_full();
    Some(1)
}

/// GetDeviceGammaRamp(hdc, ramp): the lookup table, or the identity
/// while it is bypassed.
pub fn get_device_gamma_ramp(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let ramp = args[1];
    let Some(cfg) = ramp_config(cpu) else {
        return Some(0);
    };
    if ramp == 0 || !cpu.bus.guest_probe(ramp, SIZE, true) {
        return Some(0);
    }
    let bypass = cfg & card::VIDPROC_DESKTOP_CLUT_BYPASS != 0;
    let mut words = [0u16; 3 * 256];
    for i in 0..256 {
        let rgb = if bypass {
            (i as u32) * 0x01_0101
        } else {
            cpu.bus.voodoo_write_32(card::DAC_ADDR, i as u32);
            cpu.bus.voodoo_read_32(card::DAC_DATA)
        };
        for (c, shift) in [16, 8, 0].into_iter().enumerate() {
            words[256 * c + i] = (rgb >> shift & 0xFF) as u16 * 0x101;
        }
    }
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    cpu.bus.guest_write_bytes(ramp, &bytes);
    Some(1)
}
