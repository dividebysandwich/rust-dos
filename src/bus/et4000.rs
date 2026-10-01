//! The bus's side of the Tseng ET4000 (`video::et4000`): its KEY, segment
//! select and Sierra DAC ports, its CRTC registers past the VGA's, and the
//! bank the window at A0000h shows.

use super::Bus;
use crate::video::et4000::Effect;

impl Bus {
    /// Whether the card is the ET4000.
    #[inline]
    pub(crate) fn et4000(&self) -> bool {
        self.vga.adapter.is_et4000()
    }

    /// Write one of the ET4000's ports; false if `port` is the VGA's to
    /// handle after all.
    pub(crate) fn et4000_write_port(&mut self, port: u16, value: u8) -> bool {
        let color = self.vga.misc_output_reg & 0x01 != 0;
        let chip = &mut self.vga.et4000;
        match port {
            0x3BF => chip.herc_compat = value,
            0x3D8 if color => chip.write_mode_control(value),
            0x3B8 if !color => chip.write_mode_control(value),
            0x3CD => chip.segment = value,
            0x3C5 if matches!(self.vga.sequencer_index, 0x06 | 0x07) => chip.write_seq(self.vga.sequencer_index, value),
            0x3C6 => {
                if !chip.dac_write_mask(value) {
                    return false;
                }
                // Into or out of HiColor: the picture's shape changes.
                self.vga.invalidate_timing();
                self.vga.mark_dirty_full();
            }
            0x3C7..=0x3C9 => {
                chip.dac_touched();
                return false;
            }
            _ => return false,
        }
        true
    }

    /// Read one of the ET4000's ports, or None for the VGA's.
    pub(crate) fn et4000_read_port(&mut self, port: u16) -> Option<u8> {
        let chip = &mut self.vga.et4000;
        match port {
            0x3BF => Some(chip.herc_compat),
            0x3CD => Some(chip.segment),
            0x3C5 if matches!(self.vga.sequencer_index, 0x06 | 0x07) => Some(chip.read_seq(self.vga.sequencer_index)),
            0x3C6 => Some(chip.dac_read_mask(self.vga.dac_mask)),
            0x3C7..=0x3C9 => {
                chip.dac_touched();
                None
            }
            _ => None,
        }
    }

    pub(crate) fn et4000_crtc_write(&mut self, index: u8, value: u8) {
        if index == 0x33 {
            // Latched at the retrace with the Start Address registers.
            self.sync_display();
        }
        let protect = self.vga.crtc_regs[0x11] & 0x80 != 0;
        match self.vga.et4000.write_crtc(index, value, protect) {
            Effect::None => {}
            Effect::Timing => {
                self.vga.invalidate_timing();
                self.vga.mark_dirty_full();
            }
            Effect::Start => self.note_display_start(),
        }
    }

    pub(crate) fn et4000_crtc_read(&self, index: u8) -> u8 {
        self.vga.et4000.read_crtc(index)
    }

    /// Where a byte of the window at A0000h, `offset` into it, is for the
    /// VGA's memory logic: in the bank Segment Select gives writes or
    /// reads.
    #[inline]
    pub(crate) fn et4000_window(&self, offset: usize, write: bool) -> usize {
        let chip = &self.vga.et4000;
        if chip.banks_on(self.vga.graphics_regs[0x06]) { chip.bank(write) * 0x10000 + offset } else { offset }
    }

    /// Set the chip's registers back as a BIOS mode set does.
    pub(crate) fn et4000_program_standard(&mut self) {
        self.vga.et4000.reset_for_mode();
        self.vga.invalidate_timing();
    }
}
