//! INT 13h — Disk BIOS services.
//!
//! Units 00h/01h are the floppy drives A: and B:, 80h and up the hard-disk
//! drives in drive-letter order. Drives mounted from disk images have
//! sectors: reads and writes go to the image, by cylinder, head and sector
//! of its geometry.
//!
//! Drives mounted from host directories have no sectors. Most DOS games
//! only touch this vector for disk-presence copy-protection checks (e.g.,
//! F117's DSWAP.EXE reading a known sector to verify the install disk is in
//! the drive), and returning an error makes those checks fail and the game
//! loop with an "insert disk" prompt. So for those drives the standard
//! operations report success without transferring anything, which is
//! enough for simple presence checks but not for schemes that hash the
//! data.

use std::rc::Rc;

use crate::cpu::{Cpu, CpuFlags};
use crate::disk::{DriveKind, FLOPPY_DRIVES};
use crate::diskimage::{DiskImage, SECTOR_SIZE, STATUS_BAD_COMMAND, STATUS_SECTOR_NOT_FOUND};
use iced_x86::Register;

/// BIOS data area: status of the last floppy and hard disk operations.
const BDA_FLOPPY_STATUS: usize = 0x0441;
const BDA_DISK_STATUS: usize = 0x0474;

/// Status 06h: the disk was changed (AH=16h).
const STATUS_CHANGED: u8 = 0x06;

/// Map a BIOS drive number to the DOS drive (0=A:) behind it. Units 00h/01h
/// are the floppies mounted on A:/B:; 80h+n is the n-th hard-disk-type mount
/// in drive-letter order. Anything else (including probes like DL=FFh, which
/// F117's DSWAP.EXE uses to find where the BIOS rejects drives) is absent.
fn bios_drive(cpu: &Cpu, dl: u8) -> Option<u8> {
    if dl < 0x80 {
        (dl < FLOPPY_DRIVES && cpu.bus.disk.drive_kind(dl) == Some(DriveKind::Floppy)).then_some(dl)
    } else {
        cpu.bus
            .disk
            .drives_of_kind(DriveKind::HardDisk)
            .get((dl - 0x80) as usize)
            .copied()
    }
}

/// Floppy units the BIOS has, whether or not a disk is in them: a mount on
/// B: alone makes an empty A: unit too.
fn floppy_count(cpu: &Cpu) -> u8 {
    cpu.bus.disk.floppy_units()
}

/// Finish a call with `status` in AH (and the BIOS data area, for AH=01h):
/// CF set if it isn't 0. Status 0x01 = "bad command" covers most of the
/// failure paths; 0xAA = "drive not ready" is more appropriate when the
/// drive number is invalid (so callers know to try a different drive), and
/// 0x80 ("timeout") is what an empty floppy unit produces.
fn finish(cpu: &mut Cpu, dl: u8, status: u8) {
    cpu.set_reg8(Register::AH, status);
    cpu.set_cpu_flag(CpuFlags::CF, status != 0);
    let slot = if dl < 0x80 { BDA_FLOPPY_STATUS } else { BDA_DISK_STATUS };
    cpu.bus.write_8(slot, status);
}

fn not_present_status(dl: u8) -> u8 {
    if dl < 0x80 { 0x80 } else { 0xAA }
}

/// The first sector of a CHS transfer: CH = cylinder bits 0-7, CL bits 6-7
/// = cylinder bits 8-9 and bits 0-5 = sector, DH = head.
fn transfer_start(cpu: &Cpu, disk: &DiskImage) -> Option<u64> {
    let cl = cpu.get_reg8(Register::CL);
    let cylinder = cpu.get_reg8(Register::CH) as u32 | ((cl as u32 & 0xC0) << 2);
    disk.chs_to_lba(cylinder, cpu.get_reg8(Register::DH) as u32, (cl & 0x3F) as u32)
}

/// AH=02h/03h/04h on a disk image: read into, write from or verify the
/// buffer at ES:BX, AL sectors from the CHS address. AL returns the sectors
/// transferred.
fn transfer(cpu: &mut Cpu, dl: u8, drive: u8, disk: &Rc<DiskImage>, ah: u8) {
    let count = cpu.get_al() as usize;
    let result = match transfer_start(cpu, disk) {
        _ if count == 0 => Err(STATUS_BAD_COMMAND),
        None => Err(STATUS_SECTOR_NOT_FOUND),
        Some(lba) => {
            let addr = cpu.get_physical_addr(cpu.es(), cpu.bx());
            let mut data = vec![0u8; count * SECTOR_SIZE];
            match ah {
                0x02 => disk.read(lba, &mut data).map(|()| {
                    for (i, &b) in data.iter().enumerate() {
                        cpu.bus.write_8(addr + i, b);
                    }
                }),
                0x03 => {
                    for (i, b) in data.iter_mut().enumerate() {
                        *b = cpu.bus.read_8(addr + i);
                    }
                    disk.write(lba, &data)
                }
                _ => disk.read(lba, &mut data),
            }
            .map(|()| lba)
        }
    };
    match result {
        Ok(lba) => {
            if ah != 0x04 {
                cpu.bus.sector_activity(drive, lba, count as u32, ah == 0x03);
            }
            cpu.set_reg8(Register::AL, count as u8);
            finish(cpu, dl, 0);
        }
        Err(status) => {
            cpu.set_reg8(Register::AL, 0);
            finish(cpu, dl, status);
        }
    }
}

/// AH=08h on a disk image: its geometry, with the cylinder count cut to
/// what CHS can address.
fn image_parameters(cpu: &mut Cpu, dl: u8, disk: &DiskImage) {
    let g = disk.geometry();
    let max_cylinder = g.cylinders.clamp(1, 1024) - 1;
    cpu.set_reg8(Register::CH, max_cylinder as u8);
    cpu.set_reg8(Register::CL, (g.sectors as u8 & 0x3F) | ((max_cylinder >> 2) as u8 & 0xC0));
    cpu.set_reg8(Register::DH, (g.heads - 1) as u8);
    if dl < 0x80 {
        cpu.set_reg8(Register::BL, disk.bios_type());
        cpu.set_reg8(Register::DL, floppy_count(cpu));
        cpu.set_es(0xF000);
        cpu.set_di(crate::bios::DISKETTE_PARAMS);
    } else {
        let count = cpu.bus.disk.drives_of_kind(DriveKind::HardDisk).len();
        cpu.set_reg8(Register::DL, count as u8);
    }
    cpu.set_reg8(Register::AL, 0);
}

pub fn handle(cpu: &mut Cpu) {
    let ah = cpu.get_ah();
    let al = cpu.get_al();
    let dl = cpu.get_dl();
    let drive = bios_drive(cpu, dl);
    let image = drive.and_then(|d| cpu.bus.disk.bios_image(d));

    match ah {
        // AH=00h Reset Disk System — always succeed, even for invalid drives
        // (real BIOS resets the whole controller, not a specific drive).
        0x00 => finish(cpu, dl, 0),

        // AH=01h Get Status of Last Operation.
        0x01 => {
            let slot = if dl < 0x80 { BDA_FLOPPY_STATUS } else { BDA_DISK_STATUS };
            let status = cpu.bus.read_8(slot);
            cpu.set_reg8(Register::AL, 0);
            cpu.set_reg8(Register::AH, status);
            cpu.set_cpu_flag(CpuFlags::CF, status != 0);
        }

        // AH=02h Read, 03h Write and 04h Verify Sector(s).
        0x02..=0x04 => match (drive, &image) {
            (Some(drive), Some(disk)) => transfer(cpu, dl, drive, disk, ah),
            (None, _) => finish(cpu, dl, not_present_status(dl)),
            // A read-only mount looks like a write-protected disk.
            (Some(d), None) if ah == 0x03 && !cpu.bus.disk.is_writable(d) => finish(cpu, dl, 0x03),
            (Some(_), None) => {
                if ah == 0x02 {
                    cpu.bus.log_string(&format!(
                        "[BIOS] INT 13h Read Sectors: DL={:02X} count={} (no disk image)",
                        dl, al
                    ));
                }
                cpu.set_reg8(Register::AL, al); // sectors transferred
                finish(cpu, dl, 0);
            }
        },

        // AH=05h Format Track: the image's sectors stay; a write-protected
        // disk refuses.
        0x05 => match (drive, &image) {
            (None, _) => finish(cpu, dl, not_present_status(dl)),
            (Some(_), Some(disk)) if !disk.writable() => finish(cpu, dl, 0x03),
            (Some(d), None) if !cpu.bus.disk.is_writable(d) => finish(cpu, dl, 0x03),
            _ => finish(cpu, dl, 0),
        },

        // AH=08h Get Drive Parameters. DL returns the number of drives of
        // that class, and for floppies ES:DI the diskette parameter table.
        0x08 => {
            if let Some(disk) = &image {
                image_parameters(cpu, dl, disk);
            } else if dl < 0x80 {
                let count = floppy_count(cpu);
                if dl < count {
                    // Floppy: 1.44M (2 heads, 18 sectors, 80 cylinders)
                    cpu.set_reg8(Register::CH, 79);
                    cpu.set_reg8(Register::CL, 18);
                    cpu.set_reg8(Register::DH, 1);
                    cpu.set_reg8(Register::BL, 4); // 1.44M
                    cpu.set_reg8(Register::AL, 0);
                    cpu.set_es(0xF000);
                    cpu.set_di(crate::bios::DISKETTE_PARAMS);
                } else {
                    // No such unit: zeroed geometry, callers check DL.
                    cpu.set_cx(0);
                    cpu.set_reg8(Register::DH, 0);
                    cpu.set_reg8(Register::BL, 0);
                }
                cpu.set_reg8(Register::DL, count);
            } else {
                if drive.is_none() {
                    finish(cpu, dl, 0x01);
                    return;
                }
                let count = cpu.bus.disk.drives_of_kind(DriveKind::HardDisk).len();
                cpu.set_reg8(Register::CH, 0xFF);
                cpu.set_reg8(Register::CL, 0x3F | 0xC0);
                cpu.set_reg8(Register::DH, 15);
                cpu.set_reg8(Register::DL, count as u8);
                cpu.set_reg8(Register::BL, 0);
            }
            finish(cpu, dl, 0);
        }

        // AH=0Ch Seek, 0Dh Reset Hard Disk, 10h Test Drive Ready, 11h
        // Recalibrate, 14h Controller Diagnostic, 17h/18h Set Media Type for
        // Format: nothing to do on a drive that's there.
        0x0C | 0x0D | 0x10 | 0x11 | 0x14 | 0x17 | 0x18 => {
            let status = if drive.is_some() { 0 } else { not_present_status(dl) };
            finish(cpu, dl, status);
        }

        // AH=15h Get Disk Type.
        //   AH = 00 for no drive, 01 for floppy w/o change-line,
        //   02 for floppy w/ change-line, 03 for hard disk, which returns
        //   its sector count in CX:DX.
        0x15 => {
            let kind = match drive {
                // A floppy unit without a disk is still there.
                _ if dl < 0x80 => if dl < floppy_count(cpu) { 0x02 } else { 0 },
                None => 0,
                Some(_) => 0x03,
            };
            if kind == 0x03
                && let Some(disk) = &image
            {
                let sectors = disk.geometry().total().min(u32::MAX as u64) as u32;
                cpu.set_cx((sectors >> 16) as u16);
                cpu.set_dx(sectors as u16);
            }
            cpu.set_reg8(Register::AH, kind);
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }

        // AH=16h Detect Disk Change: 06h once after another disk went in.
        0x16 => match drive {
            None => finish(cpu, dl, not_present_status(dl)),
            Some(d) => {
                let status = if cpu.bus.disk.take_media_changed(d) { STATUS_CHANGED } else { 0 };
                finish(cpu, dl, status);
            }
        },

        _ => {
            cpu.bus.log_string(&format!(
                "[BIOS] Unhandled INT 13h AH={:02X} AL={:02X} DL={:02X}",
                ah, al, dl
            ));
            finish(cpu, dl, 0x01);
        }
    }
}
