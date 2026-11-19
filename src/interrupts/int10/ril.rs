//! INT 10h AH=F0h-F5h and FAh, the EGA Register Interface Library (RIL).
//!
//! On a real PC the mouse driver (MOUSE.COM, MOUSE.SYS) or EGA.SYS hooks
//! INT 10h with these functions, so that programs and the mouse driver can
//! share the EGA's write-only registers: a program writes a register
//! through the RIL, and the driver keeps a copy it can read back and
//! restore. rust-dos's mouse driver is built into its DOS, so INT 10h
//! answers them there; on a booted system, which has its own mouse driver
//! or none, the BIOS leaves them alone as a real one does. The emulated
//! card keeps every register it was given, so a read comes from the
//! card's own registers rather than from a copy.
//!
//! A register is named by its group, the port number in DX (or in a
//! table): the CRT controller (00h, 25 registers), the sequencer (08h, 5),
//! the graphics controller (10h, 9) and the attribute controller (18h, 20)
//! have an index; the Miscellaneous Output (20h), Feature Control (28h)
//! and the EGA's Graphics 1 and 2 Position registers (30h, 38h) are single
//! registers. A function leaves the card's index and attribute flip-flop
//! as the program had them, so a program that mixes RIL calls with its
//! own port writes keeps its selection.
//!
//! AH=F6h and F7h (revert to and define the default registers) aren't
//! here: they keep tables of the driver's own, which save states would
//! have to carry, and no program is known to call them (#1470).

use crate::cpu::Cpu;
use crate::video::bios::{RIL_VERSION, ROM_SEGMENT};
use iced_x86::Register;

/// A group of registers, by the port number the RIL names it with.
#[derive(Clone, Copy)]
enum Group {
    Crtc,
    Sequencer,
    Graphics,
    Attribute,
    Misc,
    Feature,
    Graphics1Position,
    Graphics2Position,
}

impl Group {
    fn from_port(port: u16) -> Option<Group> {
        Some(match port {
            0x00 => Group::Crtc,
            0x08 => Group::Sequencer,
            0x10 => Group::Graphics,
            0x18 => Group::Attribute,
            0x20 => Group::Misc,
            0x28 => Group::Feature,
            0x30 => Group::Graphics1Position,
            0x38 => Group::Graphics2Position,
            _ => return None,
        })
    }

    /// The number of registers behind the index; 0 for a single register.
    fn count(self) -> u8 {
        match self {
            Group::Crtc => 25,
            Group::Sequencer => 5,
            Group::Graphics => 9,
            Group::Attribute => 20,
            _ => 0,
        }
    }
}

/// The CRT controller's index port, 3B4h or 3D4h, from the BIOS data.
fn crtc_port(cpu: &mut Cpu) -> u16 {
    cpu.bus.guest_read_16(0x0463)
}

/// Read register `index` of `group`; the index is ignored for a single
/// register. None for an index past the group's registers.
fn read(cpu: &mut Cpu, group: Group, index: u8) -> Option<u8> {
    if group.count() != 0 && index >= group.count() {
        return None;
    }
    let vga = &cpu.bus.vga;
    let i = index as usize;
    Some(match group {
        Group::Crtc => vga.crtc_regs[i],
        Group::Sequencer => vga.sequencer_regs[i],
        Group::Graphics => vga.graphics_regs[i],
        Group::Attribute => vga.attribute_regs[i],
        Group::Misc => vga.misc_output_reg,
        // The card keeps no Feature Control bits, and the BIOS leaves them
        // 0; the emulated card has no port 3CAh to read them back from.
        Group::Feature => 0,
        // The EGA's positions, which IBM says to program to 0 and 1, are
        // write-only and do nothing on the emulated card.
        Group::Graphics1Position => 0,
        Group::Graphics2Position => 1,
    })
}

/// Write `value` to register `index` of `group` through the card's ports,
/// so the write has the effect a program's own would.
fn write(cpu: &mut Cpu, group: Group, index: u8, value: u8) {
    if group.count() != 0 && index >= group.count() {
        return;
    }
    let crtc = crtc_port(cpu);
    match group {
        Group::Crtc | Group::Sequencer | Group::Graphics => {
            let (port, saved) = match group {
                Group::Crtc => (crtc, cpu.bus.vga.crtc_index),
                Group::Sequencer => (0x3C4, cpu.bus.vga.sequencer_index),
                _ => (0x3CE, cpu.bus.vga.graphics_index),
            };
            cpu.bus.io_write(port, index);
            cpu.bus.io_write(port + 1, value);
            let vga = &mut cpu.bus.vga;
            match group {
                Group::Crtc => vga.crtc_index = saved,
                Group::Sequencer => vga.sequencer_index = saved,
                _ => vga.graphics_index = saved,
            }
        }
        Group::Attribute => {
            let saved = (cpu.bus.vga.attribute_index, cpu.bus.vga.attribute_flip_flop);
            // Reading Input Status 1 puts the flip-flop at the address;
            // bit 5 keeps the palette with the display, which shows it.
            cpu.bus.io_read(crtc + 6);
            cpu.bus.io_write(0x3C0, index | 0x20);
            cpu.bus.io_write(0x3C0, value);
            (cpu.bus.vga.attribute_index, cpu.bus.vga.attribute_flip_flop) = saved;
        }
        Group::Misc => cpu.bus.io_write(0x3C2, value),
        Group::Feature => cpu.bus.io_write(crtc + 6, value),
        Group::Graphics1Position => cpu.bus.io_write(0x3CC, value),
        Group::Graphics2Position => cpu.bus.io_write(0x3CA, value),
    }
}

/// The linear address of ES:(BX + offset), the offset wrapping in its
/// segment.
fn table(cpu: &Cpu, offset: u16) -> u32 {
    (cpu.es() as u32) * 16 + cpu.bx().wrapping_add(offset) as u32
}

fn log_bad_group(cpu: &mut Cpu, port: u16) {
    let line = format!("[BIOS] EGA RIL: no register group {port:02X}h");
    cpu.bus.log_string(&line);
}

/// INT 10h AH=F0h-F5h and FAh. False for another AH.
pub(super) fn handle(cpu: &mut Cpu) -> bool {
    let ah = cpu.get_ah();
    if !matches!(ah, 0xF0..=0xF5 | 0xFA) {
        return false;
    }
    let (bl, bh) = (cpu.get_reg8(Register::BL), cpu.get_reg8(Register::BH));
    let port = cpu.dx();
    let group = Group::from_port(port);
    match (ah, group) {
        // F0h: read one register: BL the index; returns it in BL.
        (0xF0, Some(group)) => {
            if let Some(value) = read(cpu, group, bl) {
                cpu.set_reg8(Register::BL, value);
            }
        }
        // F1h: write one register: BL the index and BH the value, or BL
        // the value of a single register.
        (0xF1, Some(group)) => {
            if group.count() == 0 {
                write(cpu, group, 0, bl);
            } else {
                write(cpu, group, bl, bh);
            }
        }
        // F2h and F3h: read or write CL registers from CH on, at ES:BX.
        (0xF2 | 0xF3, Some(group)) => {
            let (first, count) = (cpu.get_reg8(Register::CH), cpu.get_reg8(Register::CL));
            for i in 0..count {
                let index = first.wrapping_add(i);
                let at = table(cpu, i as u16);
                if ah == 0xF2 {
                    if let Some(value) = read(cpu, group, index) {
                        cpu.bus.guest_write_8(at, value);
                    }
                } else {
                    let value = cpu.bus.guest_read_8(at);
                    write(cpu, group, index, value);
                }
            }
        }
        (0xF0..=0xF3, None) => log_bad_group(cpu, port),
        // F4h and F5h: read or write a set of CX registers, each four
        // bytes at ES:BX: the group's port (a word), the index and the
        // value.
        (0xF4 | 0xF5, _) => {
            for i in 0..cpu.cx() {
                let record = i.wrapping_mul(4);
                let lo = cpu.bus.guest_read_8(table(cpu, record));
                let hi = cpu.bus.guest_read_8(table(cpu, record.wrapping_add(1)));
                let port = u16::from_le_bytes([lo, hi]);
                let Some(group) = Group::from_port(port) else {
                    log_bad_group(cpu, port);
                    continue;
                };
                let index = cpu.bus.guest_read_8(table(cpu, record.wrapping_add(2)));
                let at = table(cpu, record.wrapping_add(3));
                if ah == 0xF4 {
                    if let Some(value) = read(cpu, group, index) {
                        cpu.bus.guest_write_8(at, value);
                    }
                } else {
                    let value = cpu.bus.guest_read_8(at);
                    write(cpu, group, index, value);
                }
            }
        }
        // FAh: interrogate the driver: ES:BX at its version, two bytes
        // (major, minor); BX stays 0 without a RIL.
        _ => {
            cpu.set_es(ROM_SEGMENT);
            cpu.set_reg16(Register::BX, RIL_VERSION);
        }
    }
    true
}
