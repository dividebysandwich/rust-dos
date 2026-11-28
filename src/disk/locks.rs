//! Locks of byte ranges of open files (INT 21h AH=5Ch), as SHARE keeps
//! them in DOS, and VSHARE in Windows 95 and 98, where sharing is always
//! there.
//!
//! A lock belongs to the open file it was made through (its System File
//! Table entry) and to the process that made it. A lock may have no byte
//! that another lock of the same file has, whoever holds that one, its
//! own holder included. Only its holder takes it off again, through the
//! same entry and with the same offset and length. Reads and writes
//! through another entry, or by another process, of bytes a lock has fail
//! with error 21h (lock violation). A lock goes when its entry closes (the
//! last handle referring to it) and when its process ends.
//!
//! Inside the built-in Windows a Win16 task holds its locks by its current
//! PDB's selector (`TASK_OWNER`), as in Windows 3.1, where KERNEL makes
//! each task's PDB the running process when it switches tasks, and a task
//! can switch to another PDB with AH=50h. The selector keeps the tasks
//! apart also in a save state from before the PDBs were in DOS's memory,
//! where every task makes its DOS calls in the process Windows runs in.
//!
//! A Win32 process holds its locks, kernel32's LockFile's as well as
//! those of its DOS calls, by its process id. Windows 95 and 98 keep one
//! set of locks for each file in VSHARE, which DOS programs, Win16 tasks
//! and Win32 processes all go through, so a lock of any of them keeps the
//! others from the bytes.

use super::DiskController;

/// What a lock is held by: a DOS process's PSP segment, with this bit
/// set a Win16 task's PDB selector, or a Win32 process's id, which is
/// 0FFF00000h or more (`Win32::new_id`). A selector can have the value of
/// a segment, and the bit keeps the two apart.
pub const TASK_OWNER: u32 = 0x1_0000;

/// The most locks there are at once. SHARE keeps as many as its /L
/// switch says (20 by default) and VSHARE as many as its heap holds; a lock
/// past them fails with error 21h, as SHARE's does. The limit keeps a
/// program that locks without unlocking from filling the host's memory.
const MAX_LOCKS: usize = 4096;

/// Error 21h, lock violation.
pub const LOCK_VIOLATION: u8 = 0x21;

/// A locked range of a file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Lock {
    /// The file (`OpenFile::key`), which other entries opened on it have
    /// too.
    key: u64,
    /// The entry it was made through.
    sft: u16,
    owner: u32,
    /// Its first and last byte.
    first: u32,
    last: u32,
}

crate::state_fields!(Lock {
    key,
    sft,
    owner,
    first,
    last
});

/// The first and last byte of `len` bytes from `offset`, as SHARE takes
/// them: none for no bytes or for bytes past the 4 GB a 32-bit offset
/// reaches.
fn range(offset: u32, len: u32) -> Option<(u32, u32)> {
    let last = offset.checked_add(len.checked_sub(1)?)?;
    Some((offset, last))
}

impl DiskController {
    /// INT 21h AX=5C00h: lock `len` bytes from `offset` of the file open
    /// at `sft` for `owner`.
    pub fn lock_region(&mut self, sft: u16, owner: u32, offset: u32, len: u32) -> Result<(), u8> {
        let key = self.lockable(sft)?;
        let (first, last) = range(offset, len).ok_or(LOCK_VIOLATION)?;
        let taken = self
            .locks
            .iter()
            .any(|l| l.key == key && l.first <= last && first <= l.last);
        if taken || self.locks.len() >= MAX_LOCKS {
            return Err(LOCK_VIOLATION);
        }
        self.locks.push(Lock {
            key,
            sft,
            owner,
            first,
            last,
        });
        Ok(())
    }

    /// INT 21h AX=5C01h: take off the lock of exactly `len` bytes from
    /// `offset` that `owner` made through `sft`.
    pub fn unlock_region(&mut self, sft: u16, owner: u32, offset: u32, len: u32) -> Result<(), u8> {
        let key = self.lockable(sft)?;
        let (first, last) = range(offset, len).ok_or(LOCK_VIOLATION)?;
        let wanted = Lock {
            key,
            sft,
            owner,
            first,
            last,
        };
        let at = self
            .locks
            .iter()
            .position(|l| *l == wanted)
            .ok_or(LOCK_VIOLATION)?;
        self.locks.remove(at);
        Ok(())
    }

    /// The file open at `sft` if it can be locked: a file, not a device,
    /// which has no sharing record in SHARE, so that a lock of it fails.
    fn lockable(&self, sft: u16) -> Result<u64, u8> {
        let open = self.open_files.get(&sft).ok_or(0x06)?;
        match open.data {
            super::OpenData::Device(_) => Err(LOCK_VIOLATION),
            _ => Ok(open.key()),
        }
    }

    /// Whether a read (`write` false) of `count` bytes, or a write, at the
    /// position of the file open at `sft`, by `owner`, reaches bytes that
    /// a lock other than its own has: one made through another entry or
    /// by another process. A read is of the bytes up to the end of the
    /// file, and one from the end or past it reads none. A write of no
    /// bytes, which cuts the file or makes it longer, is checked for the
    /// 64 KB from the position, as DOS does.
    pub fn io_locked(&self, sft: u16, owner: u32, count: u32, write: bool) -> bool {
        self.locked_from(sft, owner, count, write, || self.position(sft))
    }

    /// `io_locked` for I/O from `position` rather than from the file's
    /// own, as an FCB's record gives it.
    pub fn io_locked_at(
        &self,
        sft: u16,
        owner: u32,
        position: u64,
        count: u32,
        write: bool,
    ) -> bool {
        self.locked_from(sft, owner, count, write, || Some(position))
    }

    /// `io_locked`, asking for the position only when another's lock of
    /// the file could be in the way.
    fn locked_from(
        &self,
        sft: u16,
        owner: u32,
        count: u32,
        write: bool,
        position: impl FnOnce() -> Option<u64>,
    ) -> bool {
        if self.locks.is_empty() {
            return false;
        }
        let Some(open) = self.open_files.get(&sft) else {
            return false;
        };
        let key = open.key();
        let mut others = self
            .locks
            .iter()
            .filter(|l| l.key == key && !(l.sft == sft && l.owner == owner))
            .peekable();
        if others.peek().is_none() {
            return false;
        }
        let Some(position) = position() else {
            return false;
        };
        let at = position.min(u32::MAX as u64) as u32;
        let count = if write {
            if count == 0 { 0xFFFF } else { count }
        } else {
            let size = self.file_size(sft).unwrap_or(0);
            count.min(size.saturating_sub(at))
        };
        let Some(len) = count.checked_sub(1) else {
            return false;
        };
        let (first, last) = (at, at.saturating_add(len));
        others.any(|l| l.first <= last && first <= l.last)
    }

    /// The locks made through `sft`, which is closing, go.
    pub(super) fn forget_locks(&mut self, sft: u16) {
        self.locks.retain(|l| l.sft != sft);
    }

    /// The locks `owner` holds go: its process or task ended.
    pub fn release_locks(&mut self, owner: u32) {
        self.locks.retain(|l| l.owner != owner);
    }

    /// The locks there are, for a save state.
    pub fn locks(&self) -> &[Lock] {
        &self.locks
    }

    /// Whether any byte of any file is locked. Without, the I/O checks
    /// have nothing to look at, nor need to know who asks.
    pub fn has_locks(&self) -> bool {
        !self.locks.is_empty()
    }

    /// The locks a save state has, of the files it opened again. Those of
    /// a file the load couldn't open, whose entry stays free, go, as they
    /// would have when it closed. Each takes the key of the file open
    /// again in its entry: a state saved by an older build has keys made
    /// another way.
    pub fn set_locks(&mut self, mut locks: Vec<Lock>) {
        locks.retain_mut(|l| match self.open_files.get(&l.sft) {
            Some(file) => {
                l.key = file.key();
                true
            }
            None => false,
        });
        self.locks = locks;
    }

    /// Give the locks made through the entry `sft` the key `key`, which
    /// the file open there has after a rename moved it
    /// (`moved_files`).
    pub(super) fn rekey_locks(&mut self, sft: u16, key: u64) {
        for lock in self.locks.iter_mut().filter(|l| l.sft == sft) {
            lock.key = key;
        }
    }
}
