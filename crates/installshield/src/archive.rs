//! InstallShield 3 archives: one file (`_SETUP.1`, or a `.Z` inside a
//! self-extractor) with a 255-byte header region, the members' data, then
//! a directory table and a file table, which the header points at. The
//! layout follows the readings of unshieldv3 (Apache-2.0) and idecomp, as
//! the refurbished-dinosaurs toolkit's reader does; no code of either is
//! used. Members are stored, or compressed with PKWARE's DCL (`explode`).
//!
//! The archive records no checksum, so a compressed member is checked to
//! expand to exactly its size, end with the end code and leave no whole
//! byte of its data after it, and a stored one by its size.

use crate::groups::Membership;
use crate::member;
use crate::{
    EntryInfo, Error, Limits, Member, PathConflict, ReadSeek, Result, SkipKind, Skipped, fold,
    held_reason, invalid, latin1, relative_path,
};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom, Write};

pub(crate) const SIGNATURE: u32 = 0x8c65_5d13;
const HEADER_FIELDS_SIZE: u8 = 0x3a;
pub(crate) const HEADER_SIZE: u64 = 5 + HEADER_FIELDS_SIZE as u64;
const DIRECTORY_FIXED_SIZE: usize = 6;
const FILE_FIXED_SIZE: usize = 30;
const SPLIT_FLAGS: u16 = 0x3;
const FLAG_STORED: u16 = 0x10;
const FLAG_INVALID: u16 = 0x20;
const FLAG_SPANS_PARTS: u16 = 0x100;

/// How to read one member.
#[derive(Clone, Debug)]
struct Plan {
    path: String,
    size: u64,
    stored: bool,
    offset: u64,
    stored_size: u64,
}

/// An InstallShield 3 archive. Everything its header and tables declare is
/// checked by `open`; members are decoded when they are read.
pub struct Archive {
    stream: RefCell<Box<dyn ReadSeek>>,
    members: Vec<Member>,
    by_path: HashMap<String, usize>,
    readable: HashMap<usize, usize>,
    plans: Vec<Plan>,
    contested: HashSet<String>,
    skipped: Vec<Skipped>,
    conflicts: Vec<PathConflict>,
}

/// A table read into memory, whose entries are checked to lie in it.
struct Table<'a> {
    data: &'a [u8],
    name: &'static str,
}

impl Table<'_> {
    fn require(&self, offset: usize, length: usize) -> Result<usize> {
        if offset
            .checked_add(length)
            .is_none_or(|end| end > self.data.len())
        {
            return invalid(format!(
                "InstallShield 3 {} is truncated: an entry runs past its {} bytes.",
                self.name,
                self.data.len()
            ));
        }
        Ok(offset)
    }

    fn u8(&self, offset: usize) -> Result<u8> {
        Ok(self.data[self.require(offset, 1)?])
    }

    fn u16(&self, offset: usize) -> Result<u16> {
        let at = self.require(offset, 2)?;
        Ok(u16::from_le_bytes([self.data[at], self.data[at + 1]]))
    }

    fn u32(&self, offset: usize) -> Result<u32> {
        let at = self.require(offset, 4)?;
        Ok(u32::from_le_bytes([
            self.data[at],
            self.data[at + 1],
            self.data[at + 2],
            self.data[at + 3],
        ]))
    }

    /// `size` single-byte characters, then a NUL, all in the entry.
    fn name(&self, offset: usize, size: usize, what: &str) -> Result<String> {
        self.require(offset, size + 1)?;
        if self.data[offset + size] != 0 {
            return invalid(format!(
                "InstallShield 3 {} at byte {} of the {} has no terminator after its {} bytes.",
                what, offset, self.name, size
            ));
        }
        let bytes = &self.data[offset..offset + size];
        if bytes.contains(&0) {
            return invalid(format!(
                "InstallShield 3 {} at byte {} of the {} holds a NUL.",
                what, offset, self.name
            ));
        }
        Ok(latin1(bytes))
    }

    fn advance(&self, offset: usize, size: usize) -> Result<usize> {
        self.require(offset, size)?;
        Ok(offset + size)
    }
}

impl Archive {
    /// Open the archive at `path` on the host.
    pub fn open_path(path: &std::path::Path, limits: &Limits) -> Result<Archive> {
        let file = std::fs::File::open(path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                Error::NotFound(format!("{}: no such file", path.display()))
            }
            _ => Error::Io(e),
        })?;
        Archive::open(Box::new(std::io::BufReader::new(file)), limits)
    }

    /// Open the archive `stream` holds from its start to its end.
    pub fn open(mut stream: Box<dyn ReadSeek>, limits: &Limits) -> Result<Archive> {
        let length = stream.seek(SeekFrom::End(0))?;
        if length < HEADER_SIZE {
            return invalid("The file is too short to be an InstallShield 3 archive.");
        }
        let mut header = [0u8; HEADER_SIZE as usize];
        stream.seek(SeekFrom::Start(0))?;
        stream.read_exact(&mut header)?;
        let u16_at = |at: usize| u16::from_le_bytes([header[at], header[at + 1]]);
        let u32_at = |at: usize| {
            u32::from_le_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]]) as u64
        };
        if u32_at(0) as u32 != SIGNATURE {
            return invalid("The file isn't an InstallShield 3 archive.");
        }
        if header[4] != HEADER_FIELDS_SIZE {
            return Err(Error::Unsupported(format!(
                "InstallShield 3 archive header declares {} bytes of fields; only {:#x} is read.",
                header[4], HEADER_FIELDS_SIZE
            )));
        }
        let flags = u16_at(10);
        let file_count = u16_at(12) as usize;
        let total_parts = header[30];
        let part = header[31];
        let directories_offset = u32_at(41);
        let directories_size = u32_at(45);
        let directory_count = u16_at(49) as usize;
        let files_offset = u32_at(51);
        let files_size = u32_at(55);

        // A split set's first part declares the part count, and later
        // parts declare 0. A set of one part holds all of its data, which
        // each entry confirms by naming part 1 below.
        let split = flags & SPLIT_FLAGS != 0;
        if (split && (part != 1 || total_parts != 1)) || (!split && total_parts > 1) {
            return Err(Error::Unsupported(format!(
                "{} (flags {:#06x}, part {}, {} parts); only unsplit archives and split archives of one part are read.",
                if split {
                    "InstallShield 3 archive is one part of a split archive"
                } else {
                    "InstallShield 3 archive sets no split flag but declares more than one part"
                },
                flags,
                part,
                total_parts
            )));
        }
        if file_count > limits.max_files {
            return invalid(format!(
                "InstallShield 3 archive declares {} files, more than the limit of {}.",
                file_count, limits.max_files
            ));
        }
        if directories_size + files_size > limits.max_header_bytes {
            return invalid(format!(
                "InstallShield 3 archive's tables are {} bytes, more than the limit of {}.",
                directories_size + files_size,
                limits.max_header_bytes
            ));
        }
        let mut region = |offset: u64, size: u64, what: &str| -> Result<Vec<u8>> {
            if offset < HEADER_SIZE || size > length || offset > length - size {
                return invalid(format!(
                    "InstallShield 3 archive's {} lies outside the {}-byte archive.",
                    what, length
                ));
            }
            let mut bytes = vec![0u8; size as usize];
            stream.seek(SeekFrom::Start(offset))?;
            stream.read_exact(&mut bytes)?;
            Ok(bytes)
        };
        let directory_bytes = region(directories_offset, directories_size, "directory table")?;
        let file_bytes = region(files_offset, files_size, "file table")?;

        let table = Table {
            data: &directory_bytes,
            name: "directory table",
        };
        let mut directories: Vec<(String, usize)> = Vec::new();
        let mut offset = 0;
        let mut declared = 0usize;
        for index in 0..directory_count {
            let files = table.u16(offset)? as usize;
            let entry_size = table.u16(offset + 2)? as usize;
            let name_size = table.u16(offset + 4)? as usize;
            if entry_size < DIRECTORY_FIXED_SIZE + name_size + 1 {
                return invalid(format!(
                    "InstallShield 3 directory {} declares an entry of {} bytes, too short for its name.",
                    index, entry_size
                ));
            }
            directories.push((
                table.name(offset + DIRECTORY_FIXED_SIZE, name_size, "directory name")?,
                files,
            ));
            offset = table.advance(offset, entry_size)?;
            declared += files;
        }
        if offset != directory_bytes.len() {
            return invalid(format!(
                "InstallShield 3 directory table holds {} bytes of entries, but the header declares {}.",
                offset,
                directory_bytes.len()
            ));
        }
        if declared != file_count {
            return invalid(format!(
                "InstallShield 3 directories hold {} files, but the header declares {}.",
                declared, file_count
            ));
        }

        let mut skipped = Vec::new();
        let mut plans: Vec<(Plan, Member)> = Vec::new();
        let mut by_path: HashMap<String, Vec<usize>> = HashMap::new();
        let mut order: Vec<String> = Vec::new();
        let mut expanded_total = 0u64;
        let mut directory = 0usize;
        let mut directory_left = directories.first().map_or(0, |d| d.1);
        let table = Table {
            data: &file_bytes,
            name: "file table",
        };
        let mut offset = 0;
        for index in 0..file_count {
            while directory_left == 0 {
                directory += 1;
                // The counts add up to the file count, checked above.
                directory_left = directories.get(directory).map_or(1, |d| d.1);
            }
            directory_left -= 1;

            let last_part = table.u8(offset)?;
            let directory_index = table.u16(offset + 1)? as usize;
            let size = table.u32(offset + 3)? as u64;
            let stored_size = table.u32(offset + 7)? as u64;
            let data_offset = table.u32(offset + 11)? as u64;
            let entry_size = table.u16(offset + 23)? as usize;
            let flags = table.u16(offset + 25)?;
            let first_part = table.u8(offset + 28)?;
            let name_size = table.u8(offset + 29)? as usize;
            if entry_size < FILE_FIXED_SIZE + name_size + 1 {
                return invalid(format!(
                    "InstallShield 3 file {} declares an entry of {} bytes, too short for its name.",
                    index, entry_size
                ));
            }
            let name = table.name(offset + FILE_FIXED_SIZE, name_size, "file name")?;
            offset = table.advance(offset, entry_size)?;

            // The file table names each entry's directory, and the
            // directory table's counts place the entries in order. Readers
            // of the format use one or the other, so they must agree.
            if directory_index != directory {
                return invalid(format!(
                    "InstallShield 3 file {} names directory {}, but the directory table's file counts place it in directory {}.",
                    index, directory_index, directory
                ));
            }
            let directory_name = directories
                .get(directory)
                .map(|d| d.0.clone())
                .unwrap_or_default();
            let combined = if directory_name.is_empty() {
                name.clone()
            } else {
                format!("{}\\{}", directory_name, name)
            };
            let path = match relative_path(&combined) {
                Ok(path) => Some(path),
                // An entry that isn't listed is never written out.
                Err(_) if flags & FLAG_INVALID != 0 => None,
                Err(e) => {
                    return invalid(format!(
                        "InstallShield 3 file {} has a path that isn't portable: {}.",
                        index, e
                    ));
                }
            };
            if flags & FLAG_INVALID != 0 {
                skipped.push(Skipped {
                    index,
                    path,
                    kind: SkipKind::MarkedInvalid,
                    reason: "The archive marks the file invalid.".to_string(),
                });
                continue;
            }
            let path = path.unwrap_or_default();
            if flags & FLAG_SPANS_PARTS != 0 {
                return Err(Error::Unsupported(format!(
                    "InstallShield 3 file {} at '{}' spans archive parts; only members held whole in one part are read.",
                    index, path
                )));
            }
            // Only a split archive's entries name their parts. In a set of
            // one part, any other part would hold data this file doesn't.
            if split && (first_part != 1 || last_part != 1) {
                return invalid(format!(
                    "InstallShield 3 file {} at '{}' lies in parts {} to {}, but the archive is a split archive of one part.",
                    index, path, first_part, last_part
                ));
            }
            let stored = flags & FLAG_STORED != 0;
            if stored && stored_size != size {
                return invalid(format!(
                    "InstallShield 3 file {} at '{}' is stored uncompressed, but declares {} stored and {} expanded bytes.",
                    index, path, stored_size, size
                ));
            }
            if data_offset < HEADER_SIZE
                || stored_size > length
                || data_offset > length - stored_size
            {
                return invalid(format!(
                    "InstallShield 3 file {} at '{}' lies outside the {}-byte archive.",
                    index, path, length
                ));
            }
            let entry = EntryInfo {
                index,
                directory_index: directory,
                directory: directory_name.replace('\\', "/"),
                name: name.replace('\\', "/"),
                groups: Membership::default(),
            };
            let member = Member {
                path: path.clone(),
                size,
                entry,
                shared_by: Vec::new(),
            };
            let key = fold(&path);
            if !by_path.contains_key(&key) {
                order.push(key.clone());
            }
            by_path.entry(key).or_default().push(plans.len());
            plans.push((
                Plan {
                    path,
                    size,
                    stored,
                    offset: data_offset,
                    stored_size,
                },
                member,
            ));
            if size > limits.max_expanded_bytes - expanded_total {
                return invalid(format!(
                    "InstallShield 3 archive expands to more than the limit of {} bytes.",
                    limits.max_expanded_bytes
                ));
            }
            expanded_total += size;
        }
        if offset != file_bytes.len() {
            return invalid(format!(
                "InstallShield 3 file table holds {} bytes of entries, but the header declares {}.",
                offset,
                file_bytes.len()
            ));
        }

        let mut archive = Archive {
            stream: RefCell::new(stream),
            members: Vec::new(),
            by_path: HashMap::new(),
            readable: HashMap::new(),
            plans: Vec::new(),
            contested: HashSet::new(),
            skipped: Vec::new(),
            conflicts: Vec::new(),
        };
        for (i, (plan, member)) in plans.iter().enumerate() {
            archive.readable.insert(member.entry.index, i);
            archive.plans.push(plan.clone());
        }
        // The archive has no links or checksums, so entries at one path
        // are different files.
        for key in &order {
            let at = &by_path[key];
            if let [only] = at[..] {
                archive.by_path.insert(key.clone(), only);
                archive.members.push(plans[only].1.clone());
                continue;
            }
            let held = held_reason(
                &at.iter()
                    .map(|&i| plans[i].1.entry.index)
                    .collect::<Vec<_>>(),
            );
            for &i in at {
                skipped.push(Skipped {
                    index: plans[i].1.entry.index,
                    path: Some(plans[i].1.path.clone()),
                    kind: SkipKind::PathHeldByDifferentFiles,
                    reason: held.clone(),
                });
            }
            archive.conflicts.push(PathConflict {
                path: plans[at[0]].1.path.clone(),
                files: at.iter().map(|&i| plans[i].1.clone()).collect(),
                stored_outside: Vec::new(),
            });
            archive.contested.insert(key.clone());
        }
        skipped.sort_by_key(|s| s.index);
        archive.skipped = skipped;
        archive.members.sort_by_key(|m| fold(&m.path));
        archive.conflicts.sort_by_key(|c| fold(&c.path));
        Ok(archive)
    }

    /// The listed members, by path in any case. They have no file groups.
    pub fn members(&self) -> &[Member] {
        &self.members
    }

    /// The entries that aren't listed, in table order.
    pub fn skipped(&self) -> &[Skipped] {
        &self.skipped
    }

    /// The paths more than one entry names, by path.
    pub fn conflicts(&self) -> &[PathConflict] {
        &self.conflicts
    }

    /// The listed member at `path`, in any case.
    pub fn find(&self, path: &str) -> Option<&Member> {
        let path = relative_path(path).ok()?;
        let plan = *self.by_path.get(&fold(&path))?;
        self.members
            .iter()
            .find(|m| self.readable.get(&m.entry.index) == Some(&plan))
    }

    /// The member at `path`, in any case, expanded and checked.
    pub fn read(&self, path: &str) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        self.copy(path, &mut out)?;
        Ok(out)
    }

    /// The member at `path` written to `out` as it is expanded. An error
    /// can come after some of it is written.
    pub fn copy(&self, path: &str, out: &mut dyn Write) -> Result<u64> {
        let folded = relative_path(path)
            .map(|p| fold(&p))
            .map_err(|e| Error::NotFound(format!("{}: {}", path, e)))?;
        match self.by_path.get(&folded) {
            Some(&plan) => self.decode(plan, out),
            None if self.contested.contains(&folded) => Err(Error::NotFound(format!(
                "{}: the archive holds different files at this path; each is read by its entry's index",
                path
            ))),
            None => Err(Error::NotFound(format!(
                "{}: the archive has no such file",
                path
            ))),
        }
    }

    /// The file of entry `index` of the file table, expanded and checked.
    pub fn read_entry(&self, index: usize) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        self.copy_entry(index, &mut out)?;
        Ok(out)
    }

    /// `read_entry`, written to `out` as it is expanded.
    pub fn copy_entry(&self, index: usize, out: &mut dyn Write) -> Result<u64> {
        match self.readable.get(&index) {
            Some(&plan) => self.decode(plan, out),
            None => Err(Error::NotFound(format!(
                "InstallShield 3 archive file {} holds no file the reader reads.",
                index
            ))),
        }
    }

    fn decode(&self, plan: usize, out: &mut dyn Write) -> Result<u64> {
        let plan = &self.plans[plan];
        let mut stream = self.stream.borrow_mut();
        member::decode_archive(
            &mut **stream,
            &plan.path,
            plan.offset,
            plan.stored_size,
            plan.size,
            !plan.stored,
            out,
        )
    }
}
