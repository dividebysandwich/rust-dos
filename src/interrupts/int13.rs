//! INT 13h — Disk BIOS services.
//!
//! We don't emulate a real disk controller. Most DOS games only touch this
//! vector for disk-presence copy-protection checks (e.g., F117's DSWAP.EXE
//! reading a known sector to verify the install disk is in the drive).
//! Returning an error — which is what we used to do — makes those checks fail
//! and the game loops with an "insert disk" prompt.
//!
//! This stub reports success for the standard operations on drives that are
//! mounted (floppy-type A:/B: and hard-disk-type drives) and errors for the
//! rest. It does NOT return real sector content, so copy-protection schemes
//! that hash the data still fail. For simple presence checks it's usually
//! enough.

use crate::cpu::{Cpu, CpuFlags};
use crate::disk::DriveKind;
use iced_x86::Register;

/// Map a BIOS drive number to the DOS drive (0=A:) behind it. Units 00h/01h
/// are floppy-type mounts on A:/B:; 80h+n is the n-th hard-disk-type mount
/// in drive-letter order. Anything else (including probes like DL=FFh, which
/// F117's DSWAP.EXE uses to find where the BIOS rejects drives) is absent.
fn bios_drive(cpu: &Cpu, dl: u8) -> Option<u8> {
    if dl < 0x80 {
        (dl < 2 && cpu.bus.disk.drive_kind(dl) == Some(DriveKind::Floppy)).then_some(dl)
    } else {
        cpu.bus
            .disk
            .drives_of_kind(DriveKind::HardDisk)
            .get((dl - 0x80) as usize)
            .copied()
    }
}

fn floppy_count(cpu: &Cpu) -> u8 {
    (0..2)
        .filter(|&d| cpu.bus.disk.drive_kind(d) == Some(DriveKind::Floppy))
        .count() as u8
}

/// Standard INT 13h error return: CF=1, AH = status code.
/// Status 0x01 = "bad command" and covers most of the failure paths we care
/// about. Status 0xAA = "drive not ready" is more appropriate when the drive
/// number is invalid (so callers know to try a different drive), and 0x80
/// ("timeout") is what an empty floppy unit produces.
fn return_error(cpu: &mut Cpu, status: u8) {
    cpu.set_reg8(Register::AH, status);
    cpu.set_cpu_flag(CpuFlags::CF, true);
}

fn not_present_status(dl: u8) -> u8 {
    if dl < 0x80 { 0x80 } else { 0xAA }
}

pub fn handle(cpu: &mut Cpu) {
    let ah = cpu.get_ah();
    let al = cpu.get_al();
    let dl = cpu.get_dl();
    let drive = bios_drive(cpu, dl);

    match ah {
        // AH=00h Reset Disk System — always succeed, even for invalid drives
        // (real BIOS resets the whole controller, not a specific drive).
        0x00 => {
            cpu.set_reg8(Register::AH, 0);
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }

        // AH=01h Get Status of Last Operation.
        0x01 => {
            cpu.set_reg8(Register::AL, 0);
            cpu.set_reg8(Register::AH, 0);
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }

        // AH=02h Read Sector(s). We don't back mounted folders with sector
        // images, so present drives report success with an unchanged buffer
        // (enough for presence checks) and absent drives fail.
        0x02 => {
            cpu.bus.log_string(&format!(
                "[BIOS] INT 13h Read Sectors: DL={:02X} count={} (stubbed)",
                dl, al
            ));
            if drive.is_none() {
                return_error(cpu, not_present_status(dl));
                return;
            }
            cpu.set_reg8(Register::AH, 0);
            cpu.set_reg8(Register::AL, al); // sectors transferred
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }

        // AH=03h Write Sector(s). Silently ignored on present drives; a
        // read-only mount looks like a write-protected disk.
        0x03 => {
            let Some(d) = drive else {
                return_error(cpu, not_present_status(dl));
                return;
            };
            if !cpu.bus.disk.is_writable(d) {
                return_error(cpu, 0x03); // write protected
                return;
            }
            cpu.set_reg8(Register::AH, 0);
            cpu.set_reg8(Register::AL, al);
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }

        // AH=04h Verify Sector(s).
        0x04 => {
            if drive.is_none() {
                return_error(cpu, not_present_status(dl));
                return;
            }
            cpu.set_reg8(Register::AH, 0);
            cpu.set_reg8(Register::AL, al);
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }

        // AH=08h Get Drive Parameters. DL returns the number of drives of
        // that class.
        0x08 => {
            if dl < 0x80 {
                let count = floppy_count(cpu);
                if drive.is_some() {
                    // Floppy: 1.44M (2 heads, 18 sectors, 80 cylinders)
                    cpu.set_reg8(Register::CH, 79);
                    cpu.set_reg8(Register::CL, 18);
                    cpu.set_reg8(Register::DH, 1);
                    cpu.set_reg8(Register::BL, 4); // 1.44M
                } else {
                    // No such unit: zeroed geometry, callers check DL.
                    cpu.cx = 0;
                    cpu.set_reg8(Register::DH, 0);
                    cpu.set_reg8(Register::BL, 0);
                }
                cpu.set_reg8(Register::DL, count);
            } else {
                if drive.is_none() {
                    return_error(cpu, 0x01);
                    return;
                }
                let count = cpu.bus.disk.drives_of_kind(DriveKind::HardDisk).len();
                cpu.set_reg8(Register::CH, 0xFF);
                cpu.set_reg8(Register::CL, 0x3F | 0xC0);
                cpu.set_reg8(Register::DH, 15);
                cpu.set_reg8(Register::DL, count as u8);
                cpu.set_reg8(Register::BL, 0);
            }
            cpu.set_reg8(Register::AH, 0);
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }

        // AH=15h Get Disk Type.
        //   AH = 00 for no drive, 01 for floppy w/o change-line,
        //   02 for floppy w/ change-line, 03 for hard disk.
        0x15 => {
            let kind = match drive {
                None => 0,
                Some(_) if dl < 0x80 => 0x02,
                Some(_) => 0x03,
            };
            cpu.set_reg8(Register::AH, kind);
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }

        // AH=16h Detect Disk Change. Returns AH=0 (no change) for present drives.
        0x16 => {
            if drive.is_none() {
                return_error(cpu, not_present_status(dl));
                return;
            }
            cpu.set_reg8(Register::AH, 0);
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }

        _ => {
            cpu.bus.log_string(&format!(
                "[BIOS] Unhandled INT 13h AH={:02X} AL={:02X} DL={:02X}",
                ah, al, dl
            ));
            return_error(cpu, 0x01);
        }
    }
}
