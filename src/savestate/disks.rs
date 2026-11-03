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

/// `saved` of the drive with the key `key`. A save deletes both kinds
/// first (`delete_copies`), but an older Rust-DOS deletes only the whole
/// copy before it writes one: where both are there, the changes are of an
/// earlier save, and the copy is the state's.
pub fn saved_of(state: &Path, key: &str) -> Option<SavedDisk> {
    let copy = copy_path_of(state, key);
    if copy.is_file() {
        return Some(SavedDisk::Whole(copy));
    }
    let changes = changes_path_of(state, key);
    changes.is_file().then_some(SavedDisk::Changes(changes))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A state an older Rust-DOS saved over one of this one's has its
    /// whole copy beside the changes of the earlier save: the copy is the
    /// state's.
    #[test]
    fn a_whole_copy_wins_over_changes_left_from_an_earlier_save() {
        let dir = crate::test_dir::path("saved_disks");
        std::fs::create_dir_all(&dir).unwrap();
        let state = dir.join("slot1.state");
        delete_copies(&state);
        assert_eq!(saved(&state, 2), None);
        let changes = changes_path(&state, 2);
        std::fs::write(&changes, b"changes").unwrap();
        assert_eq!(saved(&state, 2), Some(SavedDisk::Changes(changes)));
        let copy = copy_path(&state, 2);
        std::fs::write(&copy, b"copy").unwrap();
        assert_eq!(saved(&state, 2), Some(SavedDisk::Whole(copy)));
        delete_copies(&state);
        assert_eq!(saved(&state, 2), None);
    }
}
