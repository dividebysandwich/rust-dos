//! IDirectDrawGammaControl: a surface's gamma ramp, which Direct3D
//! programs set on the primary surface for brightness. The ramp is kept
//! and given back, and the primary surface's is the display's where it
//! has one (`screen::gamma_ramp`): it is set there, and read from there,
//! so that after a mode set, which brings the identity back, the ramp
//! given back is the one the screen shows, as on Windows, where the
//! primary's ramp is the display driver's.

use super::{DD_OK, DDERR_INVALIDOBJECT, DDERR_INVALIDPARAMS, state};
use crate::cpu::Cpu;
use crate::win32::screen;

/// DDGAMMARAMP's size: 256 red, green and blue words.
const SIZE: u32 = 3 * 256 * 2;

fn surface_of(cpu: &mut Cpu, this: u32) -> Result<u32, u32> {
    let (_, id) = super::this(cpu, this).ok_or(DDERR_INVALIDOBJECT)?;
    state(cpu).surface(id).ok_or(DDERR_INVALIDOBJECT)?;
    Ok(id)
}

/// The identity ramp: each of the 256 values to itself, in 16 bits.
fn identity() -> Vec<u16> {
    (0..3 * 256).map(|i| (i % 256) as u16 * 0x101).collect()
}

/// GetGammaRamp(this, flags, ramp).
pub fn get_gamma_ramp(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, _, ramp] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let id = hr!(surface_of(cpu, this));
    if ramp == 0 || !cpu.bus.guest_probe(ramp, SIZE as usize, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let surface = state(cpu).surface(id);
    let primary = surface.is_some_and(|s| s.is_primary());
    let kept = surface.map(|s| s.gamma.clone()).unwrap_or_default();
    let gamma = match primary.then(|| screen::gamma_ramp(cpu)).flatten() {
        Some(display) => display.to_vec(),
        None if kept.len() == 3 * 256 => kept,
        None => identity(),
    };
    let bytes: Vec<u8> = gamma.iter().flat_map(|v| v.to_le_bytes()).collect();
    cpu.bus.guest_write_bytes(ramp, &bytes);
    Some(DD_OK)
}

/// SetGammaRamp(this, flags, ramp).
pub fn set_gamma_ramp(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, _, ramp] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let id = hr!(surface_of(cpu, this));
    if ramp == 0 || !cpu.bus.guest_probe(ramp, SIZE as usize, false) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let mut bytes = vec![0; SIZE as usize];
    cpu.bus.guest_read_bytes(ramp, &mut bytes);
    let gamma: screen::GammaRamp =
        std::array::from_fn(|i| u16::from_le_bytes([bytes[2 * i], bytes[2 * i + 1]]));
    let Some(s) = state(cpu).surface_mut(id) else {
        return Some(DDERR_INVALIDOBJECT);
    };
    s.gamma = gamma.to_vec();
    if s.is_primary() {
        screen::set_gamma_ramp(cpu, &gamma);
    }
    Some(DD_OK)
}
