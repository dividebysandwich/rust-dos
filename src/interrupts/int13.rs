//! INT 13h — Disk BIOS services.
//!
//! We don't emulate a real disk controller. Most DOS games only touch this
//! vector for disk-presence copy-protection checks (e.g., F117's DSWAP.EXE
//! reading a known sector to verify the install disk is in the drive).
//! Returning an error — which is what we used to do — makes those checks fail
//! and the game loops with an "insert disk" prompt.
//!
//! This stub reports success for the standard operations. It does NOT return
//! real sector content, so copy-protection schemes that hash the data still
//! fail. For simple presence checks it's usually enough.

use crate::cpu::{Cpu, CpuFlags};
use iced_x86::Register;

/// A drive number is considered valid if it matches one of the ranges a real
/// BIOS would respond to: 0x00..0x03 for floppies (A: through D:) or
/// 0x80..0x83 for hard disks. Anything outside those ranges must produce an
/// error — some copy-protection / disk-swap programs (e.g. F117's DSWAP.EXE)
/// explicitly probe with DL=0xFF to detect where the BIOS rejects them, and
/// if we lie and report success they end up constructing garbage drive paths
/// like "@:\vgame.exe" from the bogus DL.
fn is_valid_drive(dl: u8) -> bool {
    dl < 0x04 || (0x80..=0x83).contains(&dl)
}

/// Standard INT 13h error return: CF=1, AH = status code.
/// Status 0x01 = "bad command" and covers most of the failure paths we care
/// about. Status 0xAA = "drive not ready" is more appropriate when the drive
/// number is invalid (so callers know to try a different drive).
fn return_error(cpu: &mut Cpu, status: u8) {
    cpu.set_reg8(Register::AH, status);
    cpu.set_cpu_flag(CpuFlags::CF, true);
}

pub fn handle(cpu: &mut Cpu) {
    let ah = cpu.get_ah();
    let al = cpu.get_al();
    let dl = cpu.get_dl();

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

        // AH=02h Read Sector(s). We don't actually back any media, so we
        // report success for valid drive numbers with an unchanged buffer
        // (enough for presence checks), and an error for everything else.
        0x02 => {
            cpu.bus.log_string(&format!(
                "[BIOS] INT 13h Read Sectors: DL={:02X} count={} (stubbed)",
                dl, al
            ));
            if !is_valid_drive(dl) {
                return_error(cpu, 0xAA); // drive not ready
                return;
            }
            cpu.set_reg8(Register::AH, 0);
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }

        // AH=03h Write Sector(s). Silently ignore for valid drives.
        0x03 => {
            if !is_valid_drive(dl) {
                return_error(cpu, 0xAA);
                return;
            }
            cpu.set_reg8(Register::AH, 0);
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }

        // AH=04h Verify Sector(s).
        0x04 => {
            if !is_valid_drive(dl) {
                return_error(cpu, 0xAA);
                return;
            }
            cpu.set_reg8(Register::AH, 0);
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }

        // AH=08h Get Drive Parameters.
        0x08 => {
            if !is_valid_drive(dl) {
                return_error(cpu, 0x01);
                return;
            }
            if dl < 0x80 {
                // Floppy: 1.44M (2 heads, 18 sectors, 80 cylinders)
                cpu.set_reg8(Register::CH, 79);
                cpu.set_reg8(Register::CL, 18);
                cpu.set_reg8(Register::DH, 1);
                cpu.set_reg8(Register::DL, 1); // one floppy drive
                cpu.set_reg8(Register::BL, 4); // 1.44M
            } else {
                // Hard disk
                cpu.set_reg8(Register::CH, 0xFF);
                cpu.set_reg8(Register::CL, 0x3F | 0xC0);
                cpu.set_reg8(Register::DH, 15);
                cpu.set_reg8(Register::DL, 1);
                cpu.set_reg8(Register::BL, 0);
            }
            cpu.set_reg8(Register::AH, 0);
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }

        // AH=15h Get Disk Type.
        //   AH = 00 for no drive, 01 for floppy w/o change-line,
        //   02 for floppy w/ change-line, 03 for hard disk.
        0x15 => {
            let kind = if !is_valid_drive(dl) {
                0
            } else if dl < 0x80 {
                0x02
            } else {
                0x03
            };
            cpu.set_reg8(Register::AH, kind);
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }

        // AH=16h Detect Disk Change. Returns AH=0 (no change) for valid drives.
        0x16 => {
            if !is_valid_drive(dl) {
                return_error(cpu, 0x01);
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
