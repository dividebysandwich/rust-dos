//! SetDeviceGammaRamp and GetDeviceGammaRamp: the display's gamma ramp,
//! which games set for brightness. The display's side, and the adapters
//! that have one, are `screen::gamma_ramp`'s: SetDeviceGammaRamp fails
//! where there is none.

use crate::cpu::Cpu;
use crate::win32::screen::{self, GammaRamp};

/// The ramp's size: 256 red, then green, then blue words.
const SIZE: usize = 3 * 256 * 2;

/// SetDeviceGammaRamp(hdc, ramp).
pub fn set_device_gamma_ramp(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[1];
    if !screen::has_gamma_ramp(cpu) {
        return Some(0);
    }
    if at == 0 || !cpu.bus.guest_probe(at, SIZE, false) {
        return Some(0);
    }
    let mut bytes = [0u8; SIZE];
    cpu.bus.guest_read_bytes(at, &mut bytes);
    let ramp: GammaRamp =
        std::array::from_fn(|i| u16::from_le_bytes([bytes[2 * i], bytes[2 * i + 1]]));
    Some(screen::set_gamma_ramp(cpu, &ramp) as u32)
}

/// GetDeviceGammaRamp(hdc, ramp): the lookup table, or the identity
/// while it is bypassed.
pub fn get_device_gamma_ramp(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[1];
    let Some(ramp) = screen::gamma_ramp(cpu) else {
        return Some(0);
    };
    if at == 0 || !cpu.bus.guest_probe(at, SIZE, true) {
        return Some(0);
    }
    let bytes: Vec<u8> = ramp.iter().flat_map(|w| w.to_le_bytes()).collect();
    cpu.bus.guest_write_bytes(at, &bytes);
    Some(1)
}
