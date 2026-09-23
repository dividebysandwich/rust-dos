//! INT 2Fh — DOS multiplex interrupt.
//!
//! Only the MSCDEX CD-ROM extension (AH=15h) is implemented, backed by
//! drives mounted with type `cdrom`. Every other function leaves the
//! registers untouched, which callers read as "not installed" (AL stays 00h
//! for the usual install checks, AX stays 1687h for the DPMI check, ...).

use crate::cpu::Cpu;
use crate::disk::DriveKind;

/// MSCDEX version reported by AX=150Ch (2.23).
const MSCDEX_VERSION: u16 = 0x0217;

/// Device driver request status words.
const STATUS_DONE: u16 = 0x0100;
const STATUS_UNKNOWN_COMMAND: u16 = 0x8103;

/// IOCTL "device status": door closed and unlocked, cooked and raw reads,
/// data and audio, HSG and Red Book addressing.
const DEVICE_STATUS: u32 = 0x0216;
/// Volume size in 2048-byte sectors, matching the 36h cluster count.
const VOLUME_SECTORS: u32 = 0xFFFF;

pub fn handle(cpu: &mut Cpu) {
    if cpu.get_ah() == 0x15 {
        mscdex(cpu, cpu.get_al());
    }
}

fn mscdex(cpu: &mut Cpu, function: u8) {
    let cd_drives = cpu.bus.disk.drives_of_kind(DriveKind::CdRom);
    // Without CD-ROM drives MSCDEX isn't loaded: leave everything untouched.
    if cd_drives.is_empty() {
        return;
    }

    match function {
        // Installation check: BX = number of CD-ROM drives, CX = first one (0=A)
        0x00 => {
            cpu.bx = cd_drives.len() as u16;
            cpu.cx = cd_drives[0] as u16;
        }
        // CD-ROM drive check: CX = drive. BX=ADADh marks MSCDEX; AX is
        // nonzero when the drive is a CD-ROM.
        0x0B => {
            let is_cd = cpu.cx < 26 && cd_drives.contains(&(cpu.cx as u8));
            cpu.ax = if is_cd { 0x5AD8 } else { 0 };
            cpu.bx = 0xADAD;
        }
        // MSCDEX version: BX = major/minor
        0x0C => cpu.bx = MSCDEX_VERSION,
        // Get CD-ROM drive letters: one byte (0=A) per drive at ES:BX
        0x0D => {
            let addr = cpu.get_physical_addr(cpu.es, cpu.bx);
            for (i, &drive) in cd_drives.iter().enumerate() {
                cpu.bus.write_8(addr + i, drive);
            }
        }
        // Send device driver request: CX = drive, ES:BX -> request header
        0x10 => device_request(cpu),
        _ => {
            cpu.bus.log_string(&format!(
                "[MSCDEX] Unhandled INT 2Fh AX={:04X}",
                cpu.ax
            ));
        }
    }
}

/// Answer the handful of device driver requests that make sense for a
/// folder-backed CD: open/close and a few IOCTL input queries. Raw sector
/// reads and audio aren't available.
fn device_request(cpu: &mut Cpu) {
    let req = cpu.get_physical_addr(cpu.es, cpu.bx);
    let command = cpu.bus.read_8(req + 2);
    let status = match command {
        // IOCTL input: transfer buffer far pointer at +0Eh; its first byte
        // is the control code.
        0x03 => {
            let buf_off = cpu.bus.read_16(req + 0x0E);
            let buf_seg = cpu.bus.read_16(req + 0x10);
            let buf = cpu.get_physical_addr(buf_seg, buf_off);
            match cpu.bus.read_8(buf) {
                0x06 => {
                    cpu.bus.write_32(buf + 1, DEVICE_STATUS);
                    STATUS_DONE
                }
                0x07 => {
                    cpu.bus.write_8(buf + 1, 0); // cooked mode
                    cpu.bus.write_16(buf + 2, 2048); // sector size
                    STATUS_DONE
                }
                0x08 => {
                    cpu.bus.write_32(buf + 1, VOLUME_SECTORS);
                    STATUS_DONE
                }
                0x09 => {
                    cpu.bus.write_8(buf + 1, 1); // media not changed
                    STATUS_DONE
                }
                _ => STATUS_UNKNOWN_COMMAND,
            }
        }
        // Input flush, device open, device close: nothing to do
        0x07 | 0x0D | 0x0E => STATUS_DONE,
        _ => STATUS_UNKNOWN_COMMAND,
    };
    cpu.bus.write_16(req + 3, status);
}
