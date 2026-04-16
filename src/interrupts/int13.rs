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

pub fn handle(cpu: &mut Cpu) {
    let ah = cpu.get_ah();
    let al = cpu.get_al();
    let dl = cpu.get_dl();

    match ah {
        // AH=00h Reset Disk System — always succeed.
        0x00 => {
            cpu.set_reg8(Register::AH, 0); // status = 0 (no error)
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }

        // AH=01h Get Status of Last Operation.
        //   Returns AL = last status byte (0 = success).
        0x01 => {
            cpu.set_reg8(Register::AL, 0);
            cpu.set_reg8(Register::AH, 0);
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }

        // AH=02h Read Sector(s).
        //   AL = count of sectors
        //   CH = track, CL = sector (bits 0-5) + high track bits (6-7)
        //   DH = head, DL = drive
        //   ES:BX -> buffer
        // We report success and do NOT modify the buffer — enough to pass
        // simple "is the disk present" checks but not content-verifying
        // copy protection.
        0x02 => {
            cpu.bus.log_string(&format!(
                "[BIOS] INT 13h Read Sectors: DL={:02X} count={} (stubbed)",
                dl, al
            ));
            cpu.set_reg8(Register::AH, 0);
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }

        // AH=03h Write Sector(s). Silently ignore — we don't back any media.
        0x03 => {
            cpu.set_reg8(Register::AH, 0);
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }

        // AH=04h Verify Sector(s). Always succeed.
        0x04 => {
            cpu.set_reg8(Register::AH, 0);
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }

        // AH=08h Get Drive Parameters.
        //   Returns:
        //     CH = low 8 bits of max cylinder
        //     CL = bits 0-5 max sector, bits 6-7 high 2 bits of max cyl
        //     DH = max head
        //     DL = number of drives of this type
        //     BL = drive type (floppy: 01=360K, 02=1.2M, 03=720K, 04=1.44M)
        //     ES:DI -> Disk Parameter Table (we leave it alone)
        // Report a standard 1.44M floppy geometry (C:H:S = 79:1:18) for
        // floppies, or a plausible hard disk (C:H:S = 1023:15:63) for HDDs.
        0x08 => {
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
                cpu.set_reg8(Register::CL, 0x3F | 0xC0); // max sec 63, top cyl bits
                cpu.set_reg8(Register::DH, 15);
                cpu.set_reg8(Register::DL, 1); // one HDD
                cpu.set_reg8(Register::BL, 0);
            }
            cpu.set_reg8(Register::AH, 0);
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }

        // AH=15h Get Disk Type.
        //   Returns AH = 01 for floppy without change-line, 02 for floppy with
        //   change-line, 03 for hard disk, 00 for no such drive.
        0x15 => {
            let kind = if dl < 0x80 { 0x02 } else { 0x03 };
            cpu.set_reg8(Register::AH, kind);
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }

        // AH=16h Detect Disk Change. Returns AH=0 (no change).
        0x16 => {
            cpu.set_reg8(Register::AH, 0);
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }

        _ => {
            cpu.bus.log_string(&format!(
                "[BIOS] Unhandled INT 13h AH={:02X} AL={:02X} DL={:02X}",
                ah, al, dl
            ));
            cpu.set_cpu_flag(CpuFlags::CF, true);
            cpu.set_reg8(Register::AH, 0x01); // invalid function
        }
    }
}
