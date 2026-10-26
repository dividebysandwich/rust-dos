//! Drives: which there are and their kinds, from DOS's, and their free
//! space as the built-in Windows sees it.

use super::path::put_string;
use super::{
    ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_DRIVE, ERROR_PATH_NOT_FOUND, FALSE, TRUE, set_error,
    string_at,
};
use crate::cpu::Cpu;
use crate::disk::{DriveKind, LASTDRIVE, drive_letter, old_space};

/// GetDriveType's answers.
const DRIVE_NO_ROOT_DIR: u32 = 1;
const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;
const DRIVE_CDROM: u32 = 5;
const DRIVE_RAMDISK: u32 = 6;

/// The drives there are (0 for A:), in order.
fn drives(cpu: &Cpu) -> impl Iterator<Item = u8> + '_ {
    (0..LASTDRIVE).filter(|&d| cpu.bus.disk.is_mounted(d))
}

/// A bit for each drive there is, A: the lowest.
pub(super) fn get_logical_drives(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(drives(cpu).fold(0, |bits, d| bits | 1 << d))
}

/// GetLogicalDriveStringsA: the root directory of each drive there is
/// ("A:\", "C:\"), each ending in a NUL and the list in one more, at
/// `args[1]`, and the list's length without that last NUL. If the list
/// doesn't fit in `args[0]` bytes, nothing is written and the size it
/// needs, the last NUL counted, is the answer.
pub(super) fn get_logical_drive_strings(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let list: String = drives(cpu)
        .map(|d| format!("{}:\\\0", drive_letter(d)))
        .collect();
    put_string(cpu, args[1], args[0], &list)
}

/// The drive (0 for A:) of the root directory `at` names: "C:\", "C:" or
/// "C:/", or the current drive's with NULL, "\" or "/". Err(None) if it
/// names no root directory, and None if a page isn't there.
fn root(cpu: &mut Cpu, at: u32) -> Option<Result<u8, ()>> {
    if at == 0 {
        return Some(Ok(cpu.bus.disk.get_current_drive()));
    }
    let path = string_at(cpu, at)?;
    let bytes = path.as_bytes();
    Some(match bytes {
        [b'\\' | b'/'] => Ok(cpu.bus.disk.get_current_drive()),
        [letter, b':', rest @ ..]
            if letter.is_ascii_alphabetic() && matches!(rest, [] | [b'\\' | b'/']) =>
        {
            Ok(letter.to_ascii_uppercase() - b'A')
        }
        _ => Err(()),
    })
}

pub(super) fn get_drive_type(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Ok(drive) = root(cpu, args[0])? else {
        return Some(DRIVE_NO_ROOT_DIR);
    };
    Some(match cpu.bus.disk.drive_kind(drive) {
        None => DRIVE_NO_ROOT_DIR,
        Some(DriveKind::Floppy) => DRIVE_REMOVABLE,
        Some(DriveKind::HardDisk) => DRIVE_FIXED,
        Some(DriveKind::CdRom) => DRIVE_CDROM,
        // Z: and the other drives held in memory.
        Some(DriveKind::Virtual) => DRIVE_RAMDISK,
    })
}

/// The free space of the root directory `args[0]` names: sectors per
/// cluster, bytes per sector, free and total clusters, of the drive as the
/// built-in Windows sees it (`windows_free_space`), in larger clusters
/// past 65535 and below 2 GB in all (`old_space`), as Windows 95 and 98
/// give them.
pub(super) fn get_disk_free_space(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let drive = match root(cpu, args[0])? {
        Ok(drive) => drive,
        Err(()) => {
            set_error(cpu, ERROR_PATH_NOT_FOUND);
            return Some(FALSE);
        }
    };
    let Ok((sectors, free, bytes, total)) = cpu
        .bus
        .disk
        .windows_free_space(drive + 1)
        .map(|(spc, free, bps, total)| old_space(spc, free, bps, total))
    else {
        set_error(cpu, ERROR_INVALID_DRIVE);
        return Some(FALSE);
    };
    for (at, value) in args[1..].iter().zip([sectors, bytes, free, total]) {
        if *at != 0 {
            cpu.bus.guest_write_32(*at, value as u32);
        }
    }
    Some(TRUE)
}

/// GetVolumeInformation's file system flags: FS_CASE_IS_PRESERVED and
/// FS_UNICODE_STORED_ON_DISK, what Windows 95's VFAT gives, and the
/// latter alone for a CD's Joliet names.
const FAT_FLAGS: u32 = 0x2 | 0x4;
const CDFS_FLAGS: u32 = 0x4;

/// GetVolumeInformationA: the volume of the root directory `args[0]`
/// names. Its label (`args[1]`, `args[2]` bytes), serial number
/// (`args[3]`), longest name (`args[4]`), file system flags (`args[5]`)
/// and file system's name (`args[6]`, `args[7]` bytes), each only where
/// its pointer isn't NULL. The label is DOS's for the drive, a CD's the
/// one its check compares; the serial number is the disk image's, or 0
/// for a host folder.
pub(super) fn get_volume_information(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let drive = match root(cpu, args[0])? {
        Ok(drive) => drive,
        Err(()) => {
            set_error(cpu, ERROR_PATH_NOT_FOUND);
            return Some(FALSE);
        }
    };
    let Some(kind) = cpu.bus.disk.drive_kind(drive) else {
        set_error(cpu, ERROR_INVALID_DRIVE);
        return Some(FALSE);
    };
    let label = cpu.bus.disk.volume_label(drive).unwrap_or_default();
    let serial = cpu.bus.disk.volume_serial(drive).unwrap_or(0);
    let (fs, flags) = match kind {
        DriveKind::CdRom => ("CDFS", CDFS_FLAGS),
        _ => ("FAT", FAT_FLAGS),
    };
    for (at, size, s) in [(args[1], args[2], label.as_str()), (args[6], args[7], fs)] {
        // put_string gives the size it needs, NUL included, when the
        // buffer is too small, and the length it wrote otherwise.
        if at != 0 && put_string(cpu, at, size, s)? >= size {
            set_error(cpu, ERROR_INSUFFICIENT_BUFFER);
            return Some(FALSE);
        }
    }
    for (at, value) in [(args[3], serial), (args[4], 255), (args[5], flags)] {
        if at != 0 {
            cpu.bus.guest_write_32(at, value);
        }
    }
    Some(TRUE)
}
