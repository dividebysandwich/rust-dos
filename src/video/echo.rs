//! What a video BIOS service changed in the VGA's registers, written out
//! again through the I/O ports. The BIOS's services here set the registers
//! directly, but a virtual-8086 monitor such as Windows' VDD keeps its own
//! copy of them from the port writes it traps, as a real BIOS makes them.
//! After a service in V86 mode the BIOS writes the changes to the ports
//! once more (`bios::PORT_ACCESSES`), for the monitor to see; to the VGA
//! they are the values it already holds.

use super::vga::VgaCard;
use crate::bios::PortAccess;

/// The VGA's registers, as a program reaches them through the ports.
#[derive(Clone)]
pub struct Registers {
    misc: u8,
    sequencer: [u8; 5],
    sequencer_index: u8,
    graphics: [u8; 9],
    graphics_index: u8,
    crtc: [u8; 25],
    crtc_index: u8,
    attributes: [u8; 21],
    attribute_flip_flop: bool,
    dac_mask: u8,
    palette: Vec<u8>,
    dac_write_index: u8,
    dac_read_index: u8,
    dac_reading: bool,
}

impl Registers {
    pub fn of(vga: &VgaCard) -> Self {
        Registers {
            misc: vga.misc_output_reg,
            sequencer: vga.sequencer_regs,
            sequencer_index: vga.sequencer_index,
            graphics: vga.graphics_regs,
            graphics_index: vga.graphics_index,
            crtc: vga.crtc_regs,
            crtc_index: vga.crtc_index,
            attributes: vga.attribute_regs,
            attribute_flip_flop: vga.attribute_flip_flop,
            dac_mask: vga.dac_mask,
            palette: vga.palette.clone(),
            dac_write_index: vga.dac_write_index,
            dac_read_index: vga.dac_read_index,
            dac_reading: vga.dac_state == 3,
        }
    }
}

impl Registers {
    /// Registers that differ from `after` in every value, for writing all
    /// of `after`'s: a DOS box's registers Windows keeps aren't the card's.
    pub fn unknown(after: &Registers) -> Registers {
        let mut r = after.clone();
        r.misc = !r.misc;
        r.sequencer.iter_mut().chain(&mut r.graphics).chain(&mut r.crtc).chain(&mut r.attributes).for_each(|v| *v = !*v);
        r.dac_mask = !r.dac_mask;
        r.palette.iter_mut().for_each(|v| *v = !*v & 0x3F);
        r
    }

    /// Have the CRTC registers `indices` written whatever the card held,
    /// as a DOS box Windows keeps has its own.
    pub fn forget_crtc(&mut self, after: &Registers, indices: &[usize]) {
        for &i in indices {
            self.crtc[i] = !after.crtc[i];
        }
    }

    /// Put the registers back as they were.
    fn restore(&self, vga: &mut VgaCard) {
        vga.misc_output_reg = self.misc;
        vga.sequencer_regs = self.sequencer;
        vga.sequencer_index = self.sequencer_index;
        vga.graphics_regs = self.graphics;
        vga.graphics_index = self.graphics_index;
        vga.crtc_regs = self.crtc;
        vga.crtc_index = self.crtc_index;
        vga.attribute_regs = self.attributes;
        vga.attribute_flip_flop = self.attribute_flip_flop;
        vga.dac_mask = self.dac_mask;
        vga.palette.clone_from(&self.palette);
        vga.dac_write_index = self.dac_write_index;
        vga.dac_read_index = self.dac_read_index;
    }
}

/// All of the display but its memory: the VGA's registers, the S3's, the
/// VESA mode and what the screen shows. A video BIOS service for a DOS box
/// whose video ports Windows traps (one in a window) leaves the card as it
/// was: Windows keeps that machine's display itself, from the port writes
/// the service makes (`writes`).
pub struct Display {
    registers: Registers,
    attribute_index: u8,
    dac_state: u8,
    dac_step: u8,
    dac_8bit: bool,
    latched_start_addr: usize,
    fixed_timing: Option<super::crt::CrtTiming>,
    s3: super::s3::S3,
    vbe: (Option<super::vbe::VbeMode>, Option<u32>, bool, u32, u32, u32, u32, u8),
    video_mode: super::VideoMode,
}

impl Display {
    pub fn of(bus: &crate::bus::Bus) -> Self {
        let (vga, vbe) = (&bus.vga, &bus.vbe);
        Display {
            registers: Registers::of(vga),
            attribute_index: vga.attribute_index,
            dac_state: vga.dac_state,
            dac_step: vga.dac_step,
            dac_8bit: vga.dac_8bit,
            latched_start_addr: vga.latched_start_addr,
            fixed_timing: vga.fixed_timing(),
            s3: vga.s3.clone(),
            vbe: (vbe.mode, vbe.lfb_base, vbe.lfb, vbe.bank, vbe.pitch, vbe.start, vbe.latched_start, vbe.start_high),
            video_mode: bus.video_mode,
        }
    }

    pub fn restore(self, bus: &mut crate::bus::Bus) {
        let vga = &mut bus.vga;
        self.registers.restore(vga);
        vga.attribute_index = self.attribute_index;
        vga.dac_state = self.dac_state;
        vga.dac_step = self.dac_step;
        vga.dac_8bit = self.dac_8bit;
        vga.latched_start_addr = self.latched_start_addr;
        vga.set_fixed_timing(self.fixed_timing);
        vga.s3 = self.s3;
        vga.mark_dirty_full();
        let vbe = &mut bus.vbe;
        (vbe.mode, vbe.lfb_base, vbe.lfb, vbe.bank, vbe.pitch, vbe.start, vbe.latched_start, vbe.start_high) = self.vbe;
        bus.video_mode = self.video_mode;
    }
}

/// The port accesses that set the registers that differ between `before`
/// and `after` to their values in `after`, in the order a BIOS's mode set
/// makes them, leaving the index registers, the attribute flip-flop and
/// the DAC's index as `after` has them.
pub fn writes(before: &Registers, after: &Registers) -> Vec<PortAccess> {
    use PortAccess::{In, Out};
    let mut out = Vec::new();
    let color = after.misc & 1 != 0;
    let (crtc_port, status_port) = if color { (0x3D4, 0x3DA) } else { (0x3B4, 0x3BA) };
    if before.misc != after.misc {
        out.push(Out(0x3C2, after.misc));
    }
    indexed(&mut out, 0x3C4, &before.sequencer, &after.sequencer, after.sequencer_index);
    // The timing registers 00h-07h take writes only with the protection
    // bit of 11h clear.
    let crtc_changed = |i: usize| before.crtc[i] != after.crtc[i];
    if (0..=7).any(crtc_changed) && after.crtc[0x11] & 0x80 != 0 {
        out.push(Out(crtc_port, 0x11));
        out.push(Out(crtc_port + 1, after.crtc[0x11] & 0x7F));
        let mut unprotected = before.crtc;
        unprotected[0x11] = !after.crtc[0x11];
        indexed(&mut out, crtc_port, &unprotected, &after.crtc, after.crtc_index);
    } else {
        indexed(&mut out, crtc_port, &before.crtc, &after.crtc, after.crtc_index);
    }
    indexed(&mut out, 0x3CE, &before.graphics, &after.graphics, after.graphics_index);
    if before.attributes != after.attributes {
        out.push(In(status_port));
        for (i, (&old, &new)) in before.attributes.iter().zip(&after.attributes).enumerate() {
            if old != new {
                out.push(Out(0x3C0, i as u8));
                out.push(Out(0x3C0, new));
            }
        }
        // Palette access off again: the screen shows.
        out.push(Out(0x3C0, 0x20));
        if !after.attribute_flip_flop {
            out.push(In(status_port));
        }
    }
    if before.dac_mask != after.dac_mask {
        out.push(Out(0x3C6, after.dac_mask));
    }
    let changed: Vec<usize> = (0..256).filter(|&i| before.palette[3 * i..3 * i + 3] != after.palette[3 * i..3 * i + 3]).collect();
    if !changed.is_empty() {
        let mut next = None;
        for &i in &changed {
            if next != Some(i) {
                out.push(Out(0x3C8, i as u8));
            }
            for &component in &after.palette[3 * i..3 * i + 3] {
                out.push(Out(0x3C9, component));
            }
            next = Some(i + 1);
        }
        out.push(if after.dac_reading { Out(0x3C7, after.dac_read_index) } else { Out(0x3C8, after.dac_write_index) });
    }
    out
}

/// The changed registers of an index/data pair at `port` and `port + 1`,
/// then the index `after` leaves.
fn indexed(out: &mut Vec<PortAccess>, port: u16, before: &[u8], after: &[u8], index: u8) {
    let mut any = false;
    for (i, (&old, &new)) in before.iter().zip(after).enumerate() {
        if old != new {
            out.push(PortAccess::Out(port, i as u8));
            out.push(PortAccess::Out(port + 1, new));
            any = true;
        }
    }
    if any {
        out.push(PortAccess::Out(port, index));
    }
}

#[cfg(test)]
mod tests {
    use super::PortAccess::{In, Out};
    use super::*;

    fn registers() -> Registers {
        let mut vga = VgaCard::new();
        vga.misc_output_reg = 0x67;
        Registers::of(&vga)
    }

    #[test]
    fn nothing_changed_nothing_written() {
        assert!(writes(&registers(), &registers()).is_empty());
    }

    #[test]
    fn changed_registers_then_the_index_left() {
        let before = registers();
        let mut after = before.clone();
        after.sequencer[2] = 0x0F;
        after.sequencer_index = 0x04;
        after.crtc[0x0E] = 0x12;
        after.crtc_index = 0x0F;
        assert_eq!(
            writes(&before, &after),
            [Out(0x3C4, 2), Out(0x3C5, 0x0F), Out(0x3C4, 4), Out(0x3D4, 0x0E), Out(0x3D5, 0x12), Out(0x3D4, 0x0F)]
        );
    }

    #[test]
    fn protected_timing_registers_are_unlocked_first() {
        let mut before = registers();
        before.crtc[0x11] = 0x8E;
        let mut after = before.clone();
        after.crtc[0x01] = !before.crtc[0x01];
        assert_eq!(
            writes(&before, &after),
            [Out(0x3D4, 0x11), Out(0x3D5, 0x0E), Out(0x3D4, 0x01), Out(0x3D5, after.crtc[0x01]), Out(0x3D4, 0x11), Out(0x3D5, 0x8E), Out(0x3D4, 0)]
        );
    }

    #[test]
    fn attributes_between_flip_flop_resets_and_the_dac_in_runs() {
        let before = registers();
        let mut after = before.clone();
        after.attributes[3] = 0x3B;
        after.palette[3 * 5..3 * 7].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
        after.palette[3 * 9..3 * 10].copy_from_slice(&[7, 8, 9]);
        after.dac_write_index = 0x10;
        assert_eq!(
            writes(&before, &after),
            [
                In(0x3DA), Out(0x3C0, 3), Out(0x3C0, 0x3B), Out(0x3C0, 0x20), In(0x3DA),
                Out(0x3C8, 5), Out(0x3C9, 1), Out(0x3C9, 2), Out(0x3C9, 3), Out(0x3C9, 4), Out(0x3C9, 5), Out(0x3C9, 6),
                Out(0x3C8, 9), Out(0x3C9, 7), Out(0x3C9, 8), Out(0x3C9, 9),
                Out(0x3C8, 0x10),
            ]
        );
    }
}
