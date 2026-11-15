//! The card's PCI configuration space (chapter 2 of
//! `docs/toadforce-spec.md`): one function with a type 00h header and
//! three memory BARs.

use super::regs::{REGISTERS_SIZE, RING_SIZE};

pub const VENDOR_ID: u16 = 0x1234;
pub const DEVICE_ID: u16 = 0x7F01;
pub const SUBSYSTEM_VENDOR_ID: u16 = 0x1234;
/// "TF".
pub const SUBSYSTEM_ID: u16 = 0x5446;

/// Where the emulator puts the BARs, as a PCI BIOS would have: card
/// memory where the VESA BIOS's linear frame buffer is on the other
/// cards, with room for its largest size, and the registers and the ring
/// after it.
pub const INITIAL_MEMORY: u32 = crate::video::vbe::LFB_BASE as u32;
pub const INITIAL_REGISTERS: u32 = 0xE800_0000;
pub const INITIAL_RING: u32 = 0xE810_0000;

/// The interrupt line after reset.
pub const IRQ: u8 = 11;

/// The command register's I/O space and memory space enables.
const COMMAND_IO: u16 = 1 << 0;
const COMMAND_MEMORY: u16 = 1 << 1;

/// One of the card's windows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bar {
    Registers = 0,
    Memory = 1,
    Ring = 2,
}

#[derive(Clone, Debug)]
pub struct Config {
    command: u16,
    /// The BARs' base addresses, without their type bits.
    bars: [u32; 3],
    /// The BARs' sizes: the registers', card memory's and the ring's.
    sizes: [u32; 3],
    pub interrupt_line: u8,
}

crate::state_fields!(Config {
    command,
    bars,
    sizes,
    interrupt_line
});

impl Config {
    /// The configuration of a card with `memory` bytes of card memory, as
    /// a PCI BIOS leaves the boot display adapter: decoding on, and the
    /// BARs assigned.
    pub fn new(memory: u32) -> Self {
        Config {
            command: COMMAND_IO | COMMAND_MEMORY,
            bars: [INITIAL_REGISTERS, INITIAL_MEMORY, INITIAL_RING],
            sizes: [REGISTERS_SIZE, memory, RING_SIZE],
            interrupt_line: IRQ,
        }
    }

    /// Where `bar` is and how long, while the card decodes memory and
    /// the BAR has an address.
    pub fn window(&self, bar: Bar) -> Option<(u32, u32)> {
        let (base, size) = (self.bars[bar as usize], self.sizes[bar as usize]);
        (self.command & COMMAND_MEMORY != 0 && base != 0).then_some((base, size))
    }

    /// The offset of physical address `addr` in `bar`, if it is in it.
    #[inline]
    pub fn offset_in(&self, bar: Bar, addr: u32) -> Option<u32> {
        let (base, size) = self.window(bar)?;
        let offset = addr.wrapping_sub(base);
        (offset < size).then_some(offset)
    }

    /// A doubleword of configuration space.
    fn dword(&self, reg: u8) -> u32 {
        match reg {
            0x00 => (DEVICE_ID as u32) << 16 | VENDOR_ID as u32,
            0x04 => self.command as u32,
            // A VGA-compatible display controller, revision 0.
            0x08 => 0x0300_0000,
            0x10 => self.bars[0],
            // Prefetchable.
            0x14 => self.bars[1] | 0x08,
            0x18 => self.bars[2],
            0x2C => (SUBSYSTEM_ID as u32) << 16 | SUBSYSTEM_VENDOR_ID as u32,
            // INTA#.
            0x3C => 0x0100 | self.interrupt_line as u32,
            _ => 0,
        }
    }

    /// A byte of configuration space.
    pub fn read(&self, reg: u8) -> u8 {
        (self.dword(reg & 0xFC) >> (8 * (reg & 3))) as u8
    }

    /// A byte of configuration space written. True if a window moved or
    /// the card's decoding changed.
    pub fn write(&mut self, reg: u8, value: u8) -> bool {
        match reg {
            0x04 => {
                let old = self.command;
                self.command = (value as u16) & (COMMAND_IO | COMMAND_MEMORY);
                old != self.command
            }
            0x10..=0x1B => {
                let bar = ((reg - 0x10) / 4) as usize;
                let shift = 8 * (reg & 3) as u32;
                let old = self.bars[bar];
                let written = old & !(0xFF << shift) | (value as u32) << shift;
                // The bits below the window's size read as 0, which is
                // how a system sizes it.
                self.bars[bar] = written & !(self.sizes[bar] - 1);
                old != self.bars[bar]
            }
            0x3C => {
                self.interrupt_line = value;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read32(c: &Config, reg: u8) -> u32 {
        (0..4).map(|i| (c.read(reg + i) as u32) << (8 * i)).sum()
    }

    fn write32(c: &mut Config, reg: u8, value: u32) {
        for i in 0..4 {
            c.write(reg + i, (value >> (8 * i)) as u8);
        }
    }

    #[test]
    fn the_header_is_the_cards() {
        let c = Config::new(64 << 20);
        assert_eq!(read32(&c, 0x00), 0x7F01_1234);
        assert_eq!(read32(&c, 0x04), 0x0000_0003);
        assert_eq!(read32(&c, 0x08), 0x0300_0000);
        assert_eq!(read32(&c, 0x10), INITIAL_REGISTERS);
        assert_eq!(read32(&c, 0x14), INITIAL_MEMORY | 0x08);
        assert_eq!(read32(&c, 0x18), INITIAL_RING);
        assert_eq!(read32(&c, 0x1C), 0);
        assert_eq!(read32(&c, 0x2C), 0x5446_1234);
        assert_eq!(read32(&c, 0x30), 0);
        assert_eq!(read32(&c, 0x3C), 0x0000_010B);
        // No expansion ROM to size or place, no fourth BAR, and the
        // read-only fields stay what they are.
        let mut c = c;
        for reg in [0x00, 0x08, 0x1C, 0x2C, 0x30, 0x34] {
            let before = read32(&c, reg);
            write32(&mut c, reg, u32::MAX);
            assert_eq!(read32(&c, reg), before, "{reg:02X}h");
        }
        // The interrupt pin isn't the system's to write.
        write32(&mut c, 0x3C, 0xFFFF_FF05);
        assert_eq!(read32(&c, 0x3C), 0x0000_0105);
    }

    #[test]
    fn the_bars_tell_their_sizes_and_move() {
        for (memory, mask) in [(32u32 << 20, 0xFE00_0000u32), (128 << 20, 0xF800_0000)] {
            let mut c = Config::new(memory);
            write32(&mut c, 0x10, u32::MAX);
            write32(&mut c, 0x14, u32::MAX);
            write32(&mut c, 0x18, u32::MAX);
            assert_eq!(read32(&c, 0x10), 0xFFFF_0000);
            assert_eq!(read32(&c, 0x14), mask | 0x08);
            assert_eq!(read32(&c, 0x18), 0xFFF0_0000);
        }
        let mut c = Config::new(64 << 20);
        write32(&mut c, 0x14, 0xC000_0000);
        assert_eq!(c.window(Bar::Memory), Some((0xC000_0000, 64 << 20)));
        assert_eq!(c.offset_in(Bar::Memory, 0xC3FF_FFFF), Some(0x03FF_FFFF));
        assert_eq!(c.offset_in(Bar::Memory, 0xC400_0000), None);
        assert_eq!(c.offset_in(Bar::Memory, 0xBFFF_FFFF), None);
        assert_eq!(
            c.offset_in(Bar::Registers, INITIAL_REGISTERS + 0x18),
            Some(0x18)
        );
        assert_eq!(
            c.offset_in(Bar::Ring, INITIAL_RING + 0xF_FFFF),
            Some(0xF_FFFF)
        );
    }

    #[test]
    fn the_windows_decode_while_memory_space_is_on() {
        let mut c = Config::new(64 << 20);
        assert!(c.write(0x04, 0x01));
        assert_eq!(c.window(Bar::Registers), None);
        assert_eq!(c.offset_in(Bar::Memory, INITIAL_MEMORY), None);
        // The other bits of the command register aren't there.
        assert!(c.write(0x04, 0xFF));
        assert_eq!(read32(&c, 0x04), 3);
        assert!(c.window(Bar::Ring).is_some());
        // A BAR at 0 has no address yet.
        write32(&mut c, 0x10, 0);
        assert_eq!(c.window(Bar::Registers), None);
        // The interrupt line is the system's to write.
        c.write(0x3C, 5);
        assert_eq!(c.interrupt_line, 5);
        assert_eq!(read32(&c, 0x3C), 0x0105);
    }
}
