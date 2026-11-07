//! Change notifications: FindFirstChangeNotificationA's handle, which a
//! wait finds signaled once the directory it watches has changed in the
//! ways its filter names, until FindNextChangeNotification asks for the
//! next change.
//!
//! Windows learns of a change from the file system as it happens. The
//! built-in Windows has no such hook on its drives, whose host folders
//! other programs may change too, so it looks instead: the handle keeps a
//! fingerprint of what its filter watches in the directory (the names,
//! sizes, attributes and times of its entries, and of its subdirectories'
//! with `subtree`), and a wait that includes the handle lists the
//! directory again and compares (`poll`). The change is seen at the next
//! wait for it, which is when a program can see it on Windows too.

use super::path::{ERROR_FILE_NOT_FOUND, path_arg};
use super::{ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER, FALSE, TRUE, set_error};
use super::{INVALID_HANDLE_VALUE, find_entries, full_path};
use crate::cpu::Cpu;
use crate::win32::object::{self, Kind};

const FILE_NOTIFY_CHANGE_FILE_NAME: u32 = 0x01;
const FILE_NOTIFY_CHANGE_DIR_NAME: u32 = 0x02;
const FILE_NOTIFY_CHANGE_ATTRIBUTES: u32 = 0x04;
const FILE_NOTIFY_CHANGE_SIZE: u32 = 0x08;
const FILE_NOTIFY_CHANGE_LAST_WRITE: u32 = 0x10;
/// The changes Windows 95 takes: the ones above and LAST_ACCESS (20h),
/// CREATION (40h) and SECURITY (100h), which DOS's drives don't keep and
/// so never change here.
const FILE_NOTIFY_VALID: u32 = 0x17F;

const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;

/// How deep `subtree` looks below the directory, and how many entries it
/// takes in all, so that watching a drive's root doesn't list the whole
/// drive at every wait.
const MAX_DEPTH: usize = 8;
const MAX_ENTRIES: usize = 4096;

/// The fingerprint of what `filter` watches in the directory `dir` (a
/// full DOS path), and below it with `subtree`; None if it isn't there.
fn fingerprint(cpu: &Cpu, dir: &str, subtree: bool, filter: u32) -> Option<u64> {
    let mut hash = Fnv::default();
    let mut dirs = vec![(dir.trim_end_matches('\\').to_string(), 0)];
    let mut seen = 0;
    while let Some((at, depth)) = dirs.pop() {
        // A drive's root keeps its backslash ("C:\").
        let list = if at.ends_with(':') {
            format!("{}\\", at)
        } else {
            at.clone()
        };
        let mut entries = match find_entries(cpu, &list, "*.*") {
            Ok(entries) => entries,
            // The watched directory itself has gone: that is a change,
            // of its name in its parent's listing.
            Err(_) if depth == 0 => return None,
            Err(_) => continue,
        };
        entries.retain(|e| e.name != "." && e.name != "..");
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        hash.write(at.as_bytes());
        for entry in entries.iter().take(MAX_ENTRIES.saturating_sub(seen)) {
            seen += 1;
            let directory = entry.attributes & FILE_ATTRIBUTE_DIRECTORY != 0;
            let name_kind = if directory {
                FILE_NOTIFY_CHANGE_DIR_NAME
            } else {
                FILE_NOTIFY_CHANGE_FILE_NAME
            };
            if filter & name_kind != 0 {
                hash.write(&[directory as u8]);
                hash.write(entry.name.to_ascii_uppercase().as_bytes());
            }
            if filter & FILE_NOTIFY_CHANGE_ATTRIBUTES != 0 {
                hash.write(&entry.attributes.to_le_bytes());
            }
            if filter & FILE_NOTIFY_CHANGE_SIZE != 0 && !directory {
                hash.write(&entry.size.to_le_bytes());
            }
            if filter & FILE_NOTIFY_CHANGE_LAST_WRITE != 0 {
                hash.write(&entry.date.to_le_bytes());
                hash.write(&entry.time.to_le_bytes());
            }
            if subtree && directory && depth < MAX_DEPTH {
                dirs.push((format!("{}\\{}", at, entry.name), depth + 1));
            }
        }
    }
    Some(hash.0)
}

/// FNV-1a, 64 bits: the same fingerprint on every host and in every run,
/// so it can go in a save state.
struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Self {
        Fnv(0xCBF2_9CE4_8422_2325)
    }
}

impl Fnv {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 ^ b as u64).wrapping_mul(0x0100_0000_01B3);
        }
        // A separator, so that "AB" + "C" isn't "A" + "BC".
        self.0 = (self.0 ^ 0xFF).wrapping_mul(0x0100_0000_01B3);
    }
}

/// The fingerprint a directory that isn't there has.
const GONE: u64 = 0;

/// FindFirstChangeNotificationA(path, subtree, filter): a handle that is
/// signaled once the directory `path` changes as `filter` says, or with
/// `subtree` a directory below it does. INVALID_HANDLE_VALUE with
/// ERROR_PATH_NOT_FOUND for no path, ERROR_INVALID_PARAMETER for a filter
/// with no change or one Windows doesn't know, and ERROR_FILE_NOT_FOUND
/// for a directory that isn't there.
pub(super) fn find_first(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (subtree, filter) = (args[1] != 0, args[2]);
    let Some(path) = path_arg(cpu, args[0])? else {
        return Some(INVALID_HANDLE_VALUE);
    };
    if filter == 0 || filter & !FILE_NOTIFY_VALID != 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(INVALID_HANDLE_VALUE);
    }
    let dir = full_path(cpu, &path);
    if !cpu.bus.disk.is_directory(&dir) {
        set_error(cpu, ERROR_FILE_NOT_FOUND);
        return Some(INVALID_HANDLE_VALUE);
    }
    let seen = fingerprint(cpu, &dir, subtree, filter).unwrap_or(GONE);
    let kind = Kind::Change {
        dir,
        subtree,
        filter,
        seen,
        signaled: false,
    };
    Some(object::create(cpu, kind, None))
}

/// FindNextChangeNotification(handle): the handle is reset, to be
/// signaled by the next change after the one that signaled it.
pub(super) fn find_next(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = object::resolve(cpu, args[0]);
    match id.and_then(|id| object::objects(cpu).get_mut(id)) {
        Some(object::Object {
            kind: Kind::Change { signaled, .. },
            ..
        }) => {
            *signaled = false;
            Some(TRUE)
        }
        _ => {
            set_error(cpu, ERROR_INVALID_HANDLE);
            Some(FALSE)
        }
    }
}

/// FindCloseChangeNotification(handle): the handle goes.
pub(super) fn find_close(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if !matches!(object::kind_of(cpu, args[0]), Some(Kind::Change { .. })) {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    }
    object::close(cpu, args[0]);
    Some(TRUE)
}

/// Look again at the directories of the change notifications among the
/// objects `ids`, which a wait is about to test, and signal the ones
/// whose directory has changed since they last looked.
pub fn poll(cpu: &mut Cpu, ids: &[u32]) {
    for &id in ids {
        let Some(Kind::Change {
            dir,
            subtree,
            filter,
            seen,
            signaled: false,
        }) = cpu.bus.win32.objects.get(id).map(|o| &o.kind)
        else {
            continue;
        };
        let (dir, subtree, filter, seen) = (dir.clone(), *subtree, *filter, *seen);
        let now = fingerprint(cpu, &dir, subtree, filter).unwrap_or(GONE);
        if now == seen {
            continue;
        }
        if let Some(object::Object {
            kind: Kind::Change { seen, signaled, .. },
            ..
        }) = cpu.bus.win32.objects.get_mut(id)
        {
            *seen = now;
            *signaled = true;
        }
    }
}
