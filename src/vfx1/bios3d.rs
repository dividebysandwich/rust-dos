//! The "3D BIOS" of shutter glasses, as Kasan's 3DMax and the LCDBIOS of
//! StereoGraphics' SimulEyes and others had it, for the VFX1 to show what
//! games draw for them (`[vfx1] stereo_bios`): INT 10h AX=4ED0h finds it
//! (DX=3344h), and AX=4FD0h turns its interlaced display (BX=3D01h on,
//! 3D00h off) and the glasses (3D03h on, 3D02h off) on and off. With them
//! on, a game draws the left eye on the even lines and the right on the
//! odd ones, in whatever mode it set: Descent 2 in 800x600 with its
//! LCDBIOS and SimulEyes settings, more than twice as many pixels to an
//! eye as the VFX1's own pages.

use crate::cpu::Cpu;

/// The install check and its answer, and the control function.
const INSTALLED: u16 = 0x4ED0;
const ANSWER: u16 = 0x3344;
const CONTROL: u16 = 0x4FD0;

/// The bits of `Vfx1::glasses`.
pub const INTERLACE: u8 = 1;
pub const GLASSES: u8 = 2;

/// A call to INT 10h for the 3D BIOS: true when it took it.
pub fn handle(cpu: &mut Cpu) -> bool {
    let ax = cpu.ax();
    if ax != INSTALLED && ax != CONTROL {
        return false;
    }
    let bx = cpu.bx();
    let Some(vfx1) = cpu.bus.vfx1.as_mut().filter(|v| v.stereo_bios) else {
        return false;
    };
    if ax == INSTALLED {
        cpu.set_dx(ANSWER);
        return true;
    }
    let (bit, on) = match bx {
        0x3D00 => (INTERLACE, false),
        0x3D01 => (INTERLACE, true),
        0x3D02 => (GLASSES, false),
        0x3D03 => (GLASSES, true),
        // Others there were (3D08h, 3D09h) are left alone.
        _ => (0, false),
    };
    vfx1.glasses = if on {
        vfx1.glasses | bit
    } else {
        vfx1.glasses & !bit
    };
    // As the VESA BIOS answers a function it has.
    cpu.set_ax(0x004F);
    true
}
