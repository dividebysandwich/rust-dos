//! INT 13h — Disk BIOS services.
//!
//! Units 00h/01h are the floppy drives A: and B:, 80h and up the hard-disk
//! drives in drive-letter order, but for the disks mounted by number, which
//! take theirs first (`DiskController::hard_disk_units`). Drives mounted
//! from disk images have
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

/// Map a BIOS drive number to the drive (0=A:) behind it. Units 00h/01h
/// are the floppies mounted on A:/B: or as 0/1; 80h+n is the n-th hard
/// disk unit (`DiskController::hard_disk_units`). Anything else (including
/// probes like DL=FFh, which F117's DSWAP.EXE uses to find where the BIOS
/// rejects drives) is absent.
fn bios_drive(cpu: &Cpu, dl: u8) -> Option<u8> {
    // A booted system's units are the disk images (`boot::unit_drive`).
    if cpu.bus.boot.is_some() {
        return crate::boot::unit_drive(&cpu.bus, dl);
    }
    if dl < 0x80 {
        let drive = (dl < FLOPPY_DRIVES).then(|| cpu.bus.disk.floppy_unit(dl))?;
        (cpu.bus.disk.drive_kind(drive) == Some(DriveKind::Floppy)).then_some(drive)
    } else {
        cpu.bus.disk.hard_disk_units(|_| true).get((dl - 0x80) as usize).copied()
    }
}

/// Floppy units the BIOS has, whether or not a disk is in them: a mount on
/// B: alone makes an empty A: unit too.
fn floppy_count(cpu: &Cpu) -> u8 {
    if cpu.bus.boot.is_some() { crate::boot::FLOPPY_UNITS } else { cpu.bus.disk.floppy_units() }
}

/// The hard disk units the BIOS has, 80h up.
fn hard_disk_count(cpu: &Cpu) -> usize {
    if cpu.bus.boot.is_some() {
        crate::boot::hard_disk_drives(&cpu.bus).len()
    } else {
        cpu.bus.disk.hard_disk_units(|_| true).len()
    }
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
    cpu.bus.guest_write_8(slot as u32, status);
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
            let buffer = cpu.real_linear(cpu.es(), cpu.bx());
            move_sectors(cpu, disk, lba, count, buffer, ah == 0x03, ah == 0x04).map(|()| lba)
        }
    };
    match result {
        Ok(lba) => {
            if ah != 0x04 {
                cpu.bus.sector_activity(drive, lba, count as u32, ah == 0x03);
            }
            if ah == 0x02 {
                crate::ide::int13::read(cpu, dl, lba, count, true);
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

/// Read `count` sectors from `lba` into the buffer at linear address
/// `buffer`, or with `write` write them from it; with `verify` just read
/// them.
fn move_sectors(cpu: &mut Cpu, disk: &DiskImage, lba: u64, count: usize, buffer: u32, write: bool, verify: bool) -> Result<(), u8> {
    let mut data = vec![0u8; count * SECTOR_SIZE];
    if write {
        cpu.bus.guest_read_bytes(buffer, &mut data);
        // A page of the buffer isn't there: the service runs again once it
        // is, and nothing goes to the disk before.
        if cpu.bus.guest_faulted() {
            return Ok(());
        }
        disk.write(lba, &data)
    } else {
        disk.read(lba, &mut data)?;
        if !verify {
            cpu.bus.guest_write_bytes(buffer, &data);
        }
        Ok(())
    }
}

/// AH=41h, 42h-44h, 47h and 48h: the extensions (EDD 1.1) of the hard
/// disks with images, which address sectors by number. DOSBox-X's
/// bios_disk.cpp has them the same.
fn extensions(cpu: &mut Cpu, dl: u8, drive: Option<u8>, image: Option<&Rc<DiskImage>>, ah: u8) {
    let (Some(drive), Some(disk), true) = (drive, image, dl >= 0x80) else {
        finish(cpu, dl, 0x01);
        return;
    };
    let packet = cpu.real_linear(cpu.ds(), cpu.si());
    match ah {
        // Installation check: version 2.1 (EDD 1.1) with the disk access
        // functions.
        0x41 if cpu.bx() == 0x55AA => {
            cpu.set_bx(0xAA55);
            cpu.set_cx(0x0001);
            cpu.set_reg8(Register::AH, 0x21);
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }
        0x41 => finish(cpu, dl, 0x01),
        // Read, write and verify with the disk address packet at DS:SI:
        // its sectors, the buffer and the first sector's number.
        0x42..=0x44 => {
            let count = cpu.bus.guest_read_16(packet + 2) as usize;
            let (offset, segment) = (cpu.bus.guest_read_16(packet + 4), cpu.bus.guest_read_16(packet + 6));
            let lba = cpu.bus.guest_read_32(packet + 8) as u64 | (cpu.bus.guest_read_32(packet + 12) as u64) << 32;
            let buffer = if (offset, segment) == (0xFFFF, 0xFFFF) && cpu.bus.guest_read_8(packet) >= 0x18 {
                cpu.bus.guest_read_32(packet + 16)
            } else {
                cpu.real_linear(segment, offset)
            };
            let result = if ah == 0x43 && !disk.writable() {
                Err(0x03)
            } else if lba + count as u64 > disk.sectors() {
                Err(STATUS_SECTOR_NOT_FOUND)
            } else {
                move_sectors(cpu, disk, lba, count, buffer, ah == 0x43, ah == 0x44)
            };
            match result {
                Ok(()) => {
                    if ah != 0x44 && count > 0 {
                        cpu.bus.sector_activity(drive, lba, count as u32, ah == 0x43);
                    }
                    if ah == 0x42 {
                        crate::ide::int13::read(cpu, dl, lba, count, false);
                    }
                    finish(cpu, dl, 0);
                }
                Err(status) => {
                    cpu.bus.guest_write_16(packet + 2, 0);
                    finish(cpu, dl, status);
                }
            }
        }
        // Seek: nothing to do.
        0x47 => finish(cpu, dl, 0),
        // The drive's parameters, in the buffer at DS:SI, whose size the
        // caller puts in its first word.
        0x48 => {
            let size = cpu.bus.guest_read_16(packet);
            if size < 0x1A {
                finish(cpu, dl, 0x01);
                return;
            }
            let g = disk.geometry();
            let mut info = Vec::with_capacity(0x1E);
            info.extend_from_slice(&(if size >= 0x1E { 0x1Eu16 } else { 0x1A }).to_le_bytes());
            info.extend_from_slice(&0x0002u16.to_le_bytes()); // the geometry is valid
            info.extend_from_slice(&g.cylinders.to_le_bytes());
            info.extend_from_slice(&g.heads.to_le_bytes());
            info.extend_from_slice(&g.sectors.to_le_bytes());
            info.extend_from_slice(&disk.sectors().to_le_bytes());
            info.extend_from_slice(&(SECTOR_SIZE as u16).to_le_bytes());
            if size >= 0x1E {
                info.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes()); // no EDD configuration
            }
            cpu.bus.guest_write_bytes(packet, &info);
            finish(cpu, dl, 0);
        }
        _ => finish(cpu, dl, 0x01),
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
        let count = hard_disk_count(cpu);
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
        0x00 => {
            crate::ide::int13::reset(cpu, dl);
            finish(cpu, dl, 0);
        }

        // AH=01h Get Status of Last Operation.
        0x01 => {
            let slot = if dl < 0x80 { BDA_FLOPPY_STATUS } else { BDA_DISK_STATUS };
            let status = cpu.bus.guest_read_8(slot as u32);
            cpu.set_reg8(Register::AL, 0);
            cpu.set_reg8(Register::AH, status);
            cpu.set_cpu_flag(CpuFlags::CF, status != 0);
        }

        // AH=02h Read, 03h Write and 04h Verify Sector(s).
        0x02..=0x04 => match (drive, &image) {
            (Some(drive), Some(disk)) => transfer(cpu, dl, drive, disk, ah),
            (None, _) => finish(cpu, dl, not_present_status(dl)),
            // A booted system's floppy unit without a disk.
            (Some(_), None) if cpu.bus.boot.is_some() => finish(cpu, dl, not_present_status(dl)),
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
                let count = hard_disk_count(cpu);
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

        0x41..=0x44 | 0x47 | 0x48 => extensions(cpu, dl, drive, image.as_ref(), ah),

        _ => {
            cpu.bus.log_string(&format!(
                "[BIOS] Unhandled INT 13h AH={:02X} AL={:02X} DL={:02X}",
                ah, al, dl
            ));
            finish(cpu, dl, 0x01);
        }
    }
}
