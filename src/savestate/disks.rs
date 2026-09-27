//! The disks of a booted system's state files. Its memory holds what it
//! read of its disks, so a state of it is only whole with the disks as they
//! were: a copy of each disk image the system has goes beside the state
//! file (`slot1.state` has `slot1.C.img`, and `slot1.2.img` for the disk
//! mounted as 2), and loading the state puts it
//! back, unless the disk's journal still reaches back to the state (the
//! same run, `DiskImage::revert_to`). On a filesystem that shares data
//! between files (btrfs, XFS), a copy takes no time and no room.

use crate::cpu::Cpu;
use crate::disk::{DRIVE_SLOTS, drive_key};
use std::path::{Path, PathBuf};

/// Where the copy of the disk in `drive` goes for the state file `state`.
pub fn copy_path(state: &Path, drive: u8) -> PathBuf {
    state.with_extension(format!("{}.img", drive_key(drive)))
}

/// The drives whose disks keep journals: a booted system's.
fn journaled(cpu: &Cpu) -> Vec<u8> {
    (0..DRIVE_SLOTS).filter(|&d| cpu.bus.disk.bios_image(d).is_some_and(|disk| disk.journaling())).collect()
}

/// Copy the disks of the booted system beside the state file `state`, as
/// they are now, when its state was just taken.
pub fn save_copies(cpu: &Cpu, state: &Path) -> Result<(), String> {
    if let Some(dir) = state.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
    }
    for drive in journaled(cpu) {
        let disk = cpu.bus.disk.bios_image(drive).expect("a journaled drive has a disk");
        disk.copy_to(&copy_path(state, drive))?;
    }
    Ok(())
}

/// Offer the drives the copies of disks beside the state file `state`, for
/// loading it: once it has mounted them as it had them (`withdraw` after).
pub fn offer_copies(cpu: &mut Cpu, state: &Path) {
    cpu.bus.disk.copies_from = Some(state.to_path_buf());
}

/// Take back the copies offered that a load didn't use.
pub fn withdraw(cpu: &mut Cpu) {
    cpu.bus.disk.copies_from = None;
    for drive in 0..DRIVE_SLOTS {
        if let Some(disk) = cpu.bus.disk.bios_image(drive) {
            disk.offer_replacement(None);
        }
    }
}

/// Delete the copies of disks beside the state file `state`.
pub fn delete_copies(state: &Path) {
    for drive in 0..DRIVE_SLOTS {
        let _ = std::fs::remove_file(copy_path(state, drive));
    }
}
