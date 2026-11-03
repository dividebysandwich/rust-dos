//! The disks of a booted system's state files. Its memory holds what it
//! read of its disks, so a state of it is only whole with the disks as they
//! were. Each disk the system has goes beside the state file, and loading
//! the state puts it back, unless the disk's journal still reaches back to
//! the state (the same run, `DiskImage::revert_to`):
//!
//! - A disk whose changes go to a delta file over an image left as it is
//!   (`-overlay`, `diskdelta`) keeps only its changes: a copy of the delta
//!   file, `slot1.C.rdelta` for `slot1.state` (`slot1.2.rdelta` for the
//!   disk mounted as 2). Its header names the image they are over, with
//!   the image's size and a hash of its start and end, and a load puts the
//!   changes back only over that image, under a delta file again.
//! - Any other disk is copied whole: `slot1.C.img`. On a filesystem that
//!   shares data between files (btrfs, XFS), a copy takes no time and no
//!   room.

use crate::cpu::Cpu;
use crate::disk::{DRIVE_SLOTS, drive_key, drive_name};
use crate::diskimage::SavedDisk;
use std::path::{Path, PathBuf};

/// Where the copy of the whole disk in `drive` goes for the state file
/// `state`.
pub fn copy_path(state: &Path, drive: u8) -> PathBuf {
    copy_path_of(state, &drive_key(drive))
}

/// `copy_path` of the drive with the key `key` (`C`, or `2` for a disk
/// mounted by number).
pub fn copy_path_of(state: &Path, key: &str) -> PathBuf {
    state.with_extension(format!("{}.img", key))
}

/// Where the changes of the disk in `drive` go for the state file `state`,
/// if they go to a delta file.
pub fn changes_path(state: &Path, drive: u8) -> PathBuf {
    changes_path_of(state, &drive_key(drive))
}

/// `changes_path` of the drive with the key `key`.
pub fn changes_path_of(state: &Path, key: &str) -> PathBuf {
    state.with_extension(format!("{}.rdelta", key))
}

/// What the state file `state` keeps of the disk in `drive`, if anything.
pub fn saved(state: &Path, drive: u8) -> Option<SavedDisk> {
    saved_of(state, &drive_key(drive))
}

/// `saved` of the drive with the key `key`.
pub fn saved_of(state: &Path, key: &str) -> Option<SavedDisk> {
    let changes = changes_path_of(state, key);
    if changes.is_file() {
        return Some(SavedDisk::Changes(changes));
    }
    let copy = copy_path_of(state, key);
    copy.is_file().then_some(SavedDisk::Whole(copy))
}

/// Whether the disk in `drive` keeps a journal: a booted system's, which
/// its states keep.
pub fn journaling(cpu: &Cpu, drive: u8) -> bool {
    cpu.bus.disk.bios_image(drive).is_some_and(|disk| disk.journaling())
}

/// The drives whose disks keep journals: a booted system's.
fn journaled(cpu: &Cpu) -> Vec<u8> {
    (0..DRIVE_SLOTS).filter(|&d| journaling(cpu, d)).collect()
}

/// Write the disks of the booted system beside the state file `state`, as
/// they are now, when its state was just taken. Returns what the log
/// should say of them: the disks copied whole that are big enough for the
/// copy to matter.
pub fn save_copies(cpu: &Cpu, state: &Path) -> Result<Vec<String>, String> {
    if let Some(dir) = state.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
    }
    let mut notes = Vec::new();
    for drive in journaled(cpu) {
        let disk = cpu.bus.disk.bios_image(drive).expect("a journaled drive has a disk");
        let saved = disk.save(&copy_path(state, drive), &changes_path(state, drive))?;
        let mb = (disk.sectors() * crate::diskimage::SECTOR_SIZE as u64) >> 20;
        // A disk made in memory (a shared folder's) is mostly holes.
        if matches!(saved, SavedDisk::Whole(_)) && disk.memory().is_none() && mb >= 64 {
            notes.push(format!(
                "drive {}'s disk is written in place, so the state has a copy of all of it \
                 ({} MB): mounted with -overlay DIR, it would keep only the disk's changes",
                drive_name(drive),
                mb
            ));
        }
    }
    Ok(notes)
}

/// Offer the drives the disks kept beside the state file `state`, for
/// loading it: once it has mounted them as it had them (`withdraw` after).
pub fn offer_copies(cpu: &mut Cpu, state: &Path) {
    cpu.bus.disk.copies_from = Some(state.to_path_buf());
}

/// Take back the disks offered that a load didn't use.
pub fn withdraw(cpu: &mut Cpu) {
    cpu.bus.disk.copies_from = None;
    for drive in 0..DRIVE_SLOTS {
        if let Some(disk) = cpu.bus.disk.bios_image(drive) {
            disk.offer_replacement(None);
        }
    }
}

/// Delete the disks kept beside the state file `state`.
pub fn delete_copies(state: &Path) {
    for drive in 0..DRIVE_SLOTS {
        let _ = std::fs::remove_file(copy_path(state, drive));
        let _ = std::fs::remove_file(changes_path(state, drive));
    }
}
