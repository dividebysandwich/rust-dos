//! Drives: which there are, their kinds and their free space, from DOS's.

use super::{ERROR_INVALID_DRIVE, ERROR_PATH_NOT_FOUND, FALSE, TRUE, set_error, string_at};
use crate::cpu::Cpu;
use crate::disk::{DriveKind, LASTDRIVE};

/// GetDriveType's answers.
const DRIVE_NO_ROOT_DIR: u32 = 1;
const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;
const DRIVE_CDROM: u32 = 5;
const DRIVE_RAMDISK: u32 = 6;

/// A bit for each drive there is, A: the lowest.
pub(super) fn get_logical_drives(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let disk = &cpu.bus.disk;
    Some(
        (0..LASTDRIVE)
            .filter(|&d| disk.is_mounted(d))
            .fold(0, |bits, d| bits | 1 << d),
    )
}

/// GetLogicalDriveStringsA: the root directory of each drive there is
/// ("A:\", "C:\"), each ending in a NUL and the list in one more, at
/// `args[1]`, and the list's length without that last NUL. If the list
/// doesn't fit in `args[0]` bytes, nothing is written and the size it
/// needs, the last NUL counted, is the answer.
pub(super) fn get_logical_drive_strings(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (size, at) = (args[0], args[1]);
    let disk = &cpu.bus.disk;
    let mut list: Vec<u8> = (0..LASTDRIVE)
        .filter(|&d| disk.is_mounted(d))
        .flat_map(|d| [b'A' + d, b':', b'\\', 0])
        .collect();
    let length = list.len() as u32;
    if length + 1 > size {
        return Some(length + 1);
    }
    list.push(0);
    cpu.bus.guest_write_bytes(at, &list);
    Some(length)
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
/// cluster, bytes per sector, free and total clusters, as INT 21h AH=36h
/// gives them, which keeps them below 65536 clusters as Windows 95 does.
pub(super) fn get_disk_free_space(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let drive = match root(cpu, args[0])? {
        Ok(drive) => drive,
        Err(()) => {
            set_error(cpu, ERROR_PATH_NOT_FOUND);
            return Some(FALSE);
        }
    };
    let Ok((sectors, free, bytes, total)) = cpu.bus.disk.get_disk_free_space(drive + 1) else {
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
