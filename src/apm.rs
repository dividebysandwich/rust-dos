//! Advanced Power Management 1.2, as a booted system finds it through
//! INT 15h AH=53h and its protected-mode entry point: connecting in real
//! mode or 16-bit or 32-bit protected mode, the CPU idle call that halts
//! the processor until an interrupt (Windows 95 calls it when it has
//! nothing to do), and turning the machine off, which ends the booted
//! system (`boot::power_off`). DOSBox-X's bios.cpp answers the same.

use crate::cpu::{Cpu, CpuFlags, CpuState};
use iced_x86::Register;

/// The protected-mode entry point in the BIOS's segment: a far-call
/// service, then RETF (`bios::install`).
pub const ENTRY: u16 = 0x1304;

/// How the system connected to APM, if it has.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Connection {
    #[default]
    None,
    Real,
    Protected16,
    Protected32,
}

crate::state_enum!(Connection { Connection::None, Connection::Real, Connection::Protected16, Connection::Protected32 });

/// Error codes, in AH with CF set.
const ALREADY_REAL: u8 = 0x02;
const NOT_CONNECTED: u8 = 0x03;
const ALREADY_16: u8 = 0x05;
const ALREADY_32: u8 = 0x07;
const BAD_DEVICE: u8 = 0x09;
const BAD_VALUE: u8 = 0x0A;
const NO_EVENTS: u8 = 0x80;
const NOT_PRESENT: u8 = 0x86;

fn ok(cpu: &mut Cpu) {
    cpu.set_cpu_flag(CpuFlags::CF, false);
}

fn fail(cpu: &mut Cpu, code: u8) {
    cpu.set_reg8(Register::AH, code);
    cpu.set_cpu_flag(CpuFlags::CF, true);
}

/// INT 15h AH=53h, and the protected-mode entry point, with the function
/// in AL: results in the registers and CF.
pub fn call(cpu: &mut Cpu) {
    let Some(boot) = cpu.bus.boot.as_mut() else {
        // Only a booted system has the power to manage.
        fail(cpu, NOT_PRESENT);
        return;
    };
    let connection = boot.apm;
    let connected = connection != Connection::None;
    let (al, bx, cx) = (cpu.get_al(), cpu.bx(), cpu.cx());
    let segment = 0xF000u16;
    match al {
        // Installation check: version 1.2, "PM", 16-bit and 32-bit
        // protected mode interfaces.
        0x00 => {
            cpu.set_ax(0x0102);
            cpu.set_bx(0x504D);
            cpu.set_cx(0x0003);
            ok(cpu);
        }
        0x01..=0x03 if bx != 0 => fail(cpu, BAD_DEVICE),
        0x01..=0x03 if connected => fail(
            cpu,
            match connection {
                Connection::Real => ALREADY_REAL,
                Connection::Protected16 => ALREADY_16,
                _ => ALREADY_32,
            },
        ),
        0x01..=0x03 => {
            let connection = match al {
                0x01 => Connection::Real,
                0x02 => Connection::Protected16,
                _ => Connection::Protected32,
            };
            if let Some(boot) = cpu.bus.boot.as_mut() {
                boot.apm = connection;
            }
            match al {
                // 16-bit: the code segment, the entry point's offset, the
                // data segment and their lengths.
                0x02 => {
                    cpu.set_ax(segment);
                    cpu.set_bx(ENTRY);
                    cpu.set_cx(segment);
                    cpu.set_si(0xFFFF);
                    cpu.set_di(0xFFFF);
                }
                // 32-bit: the 32-bit code segment, the entry point, the
                // 16-bit code and the data segment, their lengths.
                0x03 => {
                    cpu.set_ax(segment);
                    cpu.set_ebx(ENTRY as u32);
                    cpu.set_cx(segment);
                    cpu.set_dx(segment);
                    cpu.set_esi(0xFFFF_FFFF);
                    cpu.set_di(0xFFFF);
                }
                _ => {}
            }
            cpu.bus.log_string(&format!("[APM] Connected: {:?}", connection));
            ok(cpu);
        }
        0x04 if bx != 0 => fail(cpu, BAD_DEVICE),
        _ if !connected => fail(cpu, NOT_CONNECTED),
        0x04 => {
            if let Some(boot) = cpu.bus.boot.as_mut() {
                boot.apm = Connection::None;
            }
            ok(cpu);
        }
        // CPU idle: halt until the next interrupt, which the system would
        // otherwise spin waiting for.
        0x05 => {
            if cpu.get_cpu_flag(CpuFlags::IF) {
                cpu.state = CpuState::Halted;
            }
            ok(cpu);
        }
        // CPU busy.
        0x06 => ok(cpu),
        // Set power state of all devices: standby and suspend come back at
        // once; off turns the booted machine off.
        0x07 if bx != 0x0001 => fail(cpu, BAD_DEVICE),
        0x07 => match cx {
            0x0001 | 0x0002 | 0x0004 | 0x0005 => {
                cpu.set_reg8(Register::AH, 0);
                ok(cpu);
            }
            0x0003 => {
                ok(cpu);
                crate::boot::power_off(cpu, "the system turned the power off (APM)");
            }
            _ => fail(cpu, BAD_VALUE),
        },
        // Enable or disable power management (08h), for a device (0Dh),
        // engage or disengage it (0Fh).
        0x08 | 0x0D | 0x0F if bx != 0x0000 && bx != 0x0001 && bx != 0xFFFF => fail(cpu, BAD_DEVICE),
        0x08 | 0x0D | 0x0F if cx > 1 => fail(cpu, BAD_VALUE),
        0x08 | 0x0D | 0x0F => ok(cpu),
        // Power status: on AC power, no battery.
        0x0A if bx != 0x0001 && bx != 0x8001 => fail(cpu, BAD_DEVICE),
        0x0A => {
            cpu.set_bx(0x01FF);
            cpu.set_cx(0x80FF);
            cpu.set_dx(0xFFFF);
            cpu.set_si(0);
            ok(cpu);
        }
        // Power management events: none.
        0x0B => fail(cpu, NO_EVENTS),
        // Power state: ready.
        0x0C => {
            cpu.set_cx(0);
            ok(cpu);
        }
        // Driver version: 1.2 at most.
        0x0E => {
            let minor = if cpu.get_reg8(Register::CH) == 1 { cpu.get_reg8(Register::CL).min(2) } else { 2 };
            cpu.set_ax(0x0100 | minor as u16);
            ok(cpu);
        }
        // Capabilities: no batteries, no wake-ups.
        0x10 => {
            cpu.set_reg8(Register::BL, 0);
            cpu.set_cx(0);
            ok(cpu);
        }
        _ => fail(cpu, 0x0C),
    }
}

/// The protected-mode entry point (`ENTRY`).
pub fn pm_entry(cpu: &mut Cpu) {
    call(cpu);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apm(cpu: &mut Cpu, ax: u16, bx: u16, cx: u16) -> (bool, u8) {
        cpu.set_ax(ax);
        cpu.set_bx(bx);
        cpu.set_cx(cx);
        call(cpu);
        (cpu.get_cpu_flag(CpuFlags::CF), cpu.get_ah())
    }

    #[test]
    fn a_booted_system_connects_idles_and_turns_off() {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        assert_eq!(apm(&mut cpu, 0x5300, 0, 0), (true, NOT_PRESENT), "not without a booted system");
        cpu.bus.boot = Some(crate::boot::BootState::default());
        assert_eq!(apm(&mut cpu, 0x5300, 0, 0), (false, 0x01));
        assert_eq!((cpu.ax(), cpu.bx(), cpu.cx()), (0x0102, 0x504D, 0x0003));
        assert_eq!(apm(&mut cpu, 0x5305, 0, 0), (true, NOT_CONNECTED));
        assert_eq!(apm(&mut cpu, 0x5303, 0, 0), (false, 0xF0));
        assert_eq!((cpu.ax(), cpu.ebx(), cpu.cx(), cpu.dx()), (0xF000, ENTRY as u32, 0xF000, 0xF000));
        assert_eq!(apm(&mut cpu, 0x5301, 0, 0).1, ALREADY_32);
        cpu.set_cpu_flag(CpuFlags::IF, true);
        assert!(!apm(&mut cpu, 0x5305, 0, 0).0);
        assert_eq!(cpu.state, CpuState::Halted, "idle halts");
        cpu.state = CpuState::Running;
        assert_eq!(apm(&mut cpu, 0x530B, 0, 0), (true, NO_EVENTS));
        assert!(!apm(&mut cpu, 0x5307, 1, 3).0);
        assert_eq!(cpu.state, CpuState::RebootShell, "off");
    }
}
