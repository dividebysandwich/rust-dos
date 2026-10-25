//! InstallShield 5 and 6 cabinet sets: a header (`data1.hdr`, or the start
//! of `data1.cab`) with a cabinet descriptor and a file table, and volumes
//! (`data1.cab`, `data2.cab`...) whose headers record the first and last
//! file each holds, a split one's parts included. The layout, both forms
//! of compressed data, the files stored outside the cabinet and the
//! obfuscation follow Unshield's reading of the format
//! (https://github.com/twogood/unshield, MIT), as the refurbished-dinosaurs
//! toolkit's reader does.
//!
//! Major version 0 is read with version 5's layout, as Unshield reads it,
//! with two differences: its file descriptors end after the data offset,
//! and its files are split across volumes only when their flag says so.

use crate::groups::{FileGroup, GroupTable, Membership};
use crate::member::{self, Plan, Segment, Source};
use crate::{
    Compression, EntryInfo, Error, Folder, Limits, Lookup, Member, OutsideFile, OutsideStatus,
    PathConflict, ReadSeek, Result, SkipKind, Skipped, fold, held_reason, invalid, latin1,
    relative_path,
};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom, Write};

pub(crate) const SIGNATURE: u32 = 0x2863_5349;
const MICROSOFT_CABINET: u32 = 0x4643_534d;
const COMMON_HEADER_SIZE: usize = 20;
const DESCRIPTOR_FIELDS_SIZE: u64 = 0x30;
const VERSION6_DESCRIPTOR_SIZE: u64 = 0x57;
const VERSION5_DESCRIPTOR_SIZE: u64 = 0x3a;
/// Unshield reads a version 0 descriptor only up to its data offset, with
/// no MD5 after it.
const VERSION0_DESCRIPTOR_SIZE: u64 = 0x2a;
const MAX_VOLUME: u32 = 255;
const FLAG_INVALID: u16 = 8;
const FLAG_COMPRESSED: u16 = 4;
const FLAG_OBFUSCATED: u16 = 2;
const FLAG_SPLIT: u16 = 1;
const LINK_PREVIOUS: u8 = 1;
/// How far ahead of what is asked a `.cab`'s header is read.
const MIN_READ: u64 = 4096;

/// The header bytes a set is opened from. A `.hdr` is read whole. A `.cab`
/// that holds the header is also volume 1, and its header's structures
/// can lie anywhere before its member data (Unshield reads the whole file
/// and doesn't bound them by the descriptor's size), so it is read forward
/// from its start only as far as the structures asked for reach, and never
/// past the header limit.
pub(crate) struct HeaderReader {
    stream: Option<Box<dyn ReadSeek>>,
    data: Vec<u8>,
    length: u64,
    limit: u64,
    max_name: usize,
    description: String,
}

impl HeaderReader {
    fn new(
        stream: Box<dyn ReadSeek>,
        length: u64,
        name: &str,
        cab: bool,
        limits: &Limits,
    ) -> Result<HeaderReader> {
        let description = if cab {
            format!("cabinet '{}'", name)
        } else {
            format!("header file '{}'", name)
        };
        let mut reader = HeaderReader {
            stream: Some(stream),
            data: Vec::new(),
            length,
            limit: limits.max_header_bytes,
            max_name: limits.max_name_bytes,
            description,
        };
        if !cab {
            if length > limits.max_header_bytes {
                return invalid(format!(
                    "InstallShield header is {} bytes, more than the limit of {}.",
                    length, limits.max_header_bytes
                ));
            }
            reader.load(length)?;
        }
        Ok(reader)
    }

    pub(crate) fn require(&mut self, offset: u64, count: u64, what: &str) -> Result<()> {
        let end = match offset.checked_add(count) {
            Some(end) if end <= self.length => end,
            _ => {
                return invalid(format!(
                    "InstallShield header is truncated: its {} lies past the end of the {}-byte {}.",
                    what, self.length, self.description
                ));
            }
        };
        if end > self.limit {
            return invalid(format!(
                "InstallShield header's {} ends {} bytes into {}, past the header limit of {} bytes.",
                what, end, self.description, self.limit
            ));
        }
        self.load(end)
    }

    fn slice(&mut self, offset: u64, count: usize) -> Result<&[u8]> {
        self.require(offset, count as u64, "data")?;
        Ok(&self.data[offset as usize..offset as usize + count])
    }

    pub(crate) fn u8(&mut self, offset: u64) -> Result<u8> {
        Ok(self.slice(offset, 1)?[0])
    }

    pub(crate) fn u16(&mut self, offset: u64) -> Result<u16> {
        let b = self.slice(offset, 2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub(crate) fn u32(&mut self, offset: u64) -> Result<u32> {
        let b = self.slice(offset, 4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub(crate) fn u64(&mut self, offset: u64) -> Result<u64> {
        let b = self.slice(offset, 8)?;
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(b);
        Ok(u64::from_le_bytes(bytes))
    }

    /// A NUL-terminated name, as ISO 8859-1. The terminator is looked for
    /// only within the name limit, so no name costs more than that to read.
    pub(crate) fn string(&mut self, offset: u64, what: &str) -> Result<String> {
        self.require(offset, 1, what)?;
        let window = (self.max_name as u64 + 1).min(self.length - offset);
        // Within the header limit; a name that would need bytes past it
        // fails on the limit below.
        let searched = window.min(self.limit.saturating_sub(offset));
        self.load(offset + searched)?;
        let bytes = &self.data[offset as usize..(offset + searched) as usize];
        if let Some(end) = bytes.iter().position(|&b| b == 0) {
            return Ok(latin1(&bytes[..end]));
        }
        if searched < window {
            self.require(offset, window, what)?;
        }
        if window <= self.max_name as u64 {
            return invalid(format!(
                "InstallShield header is truncated: a {} has no terminator.",
                what
            ));
        }
        invalid(format!(
            "InstallShield header has a {} longer than the limit of {} bytes.",
            what, self.max_name
        ))
    }

    /// Reads up to `end`, which is at most the smaller of the length and
    /// the limit. Each read stops `MIN_READ` bytes past `end` at most, so
    /// little of the member data after a `.cab`'s header is read.
    fn load(&mut self, end: u64) -> Result<()> {
        let filled = self.data.len() as u64;
        if end <= filled {
            return Ok(());
        }
        let available = self.length.min(self.limit);
        let size = end.max(filled + MIN_READ).min(available);
        let Some(stream) = self.stream.as_mut() else {
            return invalid(format!(
                "InstallShield {} ended before its declared length.",
                self.description
            ));
        };
        let mut more = vec![0u8; (size - filled) as usize];
        stream.seek(SeekFrom::Start(filled))?;
        if let Err(e) = stream.read_exact(&mut more) {
            return if e.kind() == std::io::ErrorKind::UnexpectedEof {
                invalid(format!(
                    "InstallShield {} ended before its declared length.",
                    self.description
                ))
            } else {
                Err(e.into())
            };
        }
        self.data.extend_from_slice(&more);
        Ok(())
    }
}

/// What a volume's header records: the files it holds first and last, and
/// where their data is in it.
#[derive(Clone, Copy, Debug)]
struct VolumeHeader {
    length: u64,
    first_index: u64,
    last_index: u64,
    first_offset: u64,
    first_expanded: u64,
    first_compressed: u64,
    last_offset: u64,
    last_expanded: u64,
    last_compressed: u64,
}

/// A file descriptor of the file table.
#[derive(Clone, Copy, Debug)]
struct Descriptor {
    flags: u16,
    expanded: u64,
    compressed: u64,
    data_offset: u64,
    md5: Option<[u8; 16]>,
    name_offset: u32,
    directory: u16,
    link_previous: u32,
    link_flags: u8,
    volume: u16,
}

/// An entry's place: its index, directory and name.
#[derive(Clone, Debug)]
struct EntryName {
    index: usize,
    directory_index: usize,
    directory: String,
    name: String,
}

/// One file a path holds, from the first entry that holds it (`listed`).
/// `segments` is None when it is stored outside the cabinet. `skipped`
/// holds the records of its other entries (and its first, when it is
/// outside), each with the reason to give when its path holds other files
/// too (None keeps the record as it is).
struct File {
    path: String,
    size: u64,
    index: usize,
    data_index: usize,
    compressed: bool,
    obfuscated: bool,
    md5: Option<[u8; 16]>,
    segments: Option<Vec<Segment>>,
    listed: EntryName,
    shared_by: Vec<EntryName>,
    skipped: Vec<(Skipped, Option<String>)>,
    /// The bytes the cabinet stores for it: compressed or not.
    stored_size: u64,
    lookup_path: Option<String>,
    found_length: Option<u64>,
    outside_status: Option<OutsideStatus>,
    member: Option<Member>,
}

/// An InstallShield cabinet set of major version 0, 5 or 6, opened from
/// its header. Everything the header declares is checked by `open`;
/// members are decoded when they are read.
pub struct Cabinet {
    folder: Box<dyn Folder>,
    /// The folder of the header in `folder`, with a `/` after it, or empty.
    dir: String,
    /// What the volumes' names start with: the header's name up to its
    /// first dot or digit (`data` of `data1.hdr`).
    prefix: String,
    major: u32,
    compression: Compression,
    members: Vec<Member>,
    /// The listed members' plans, by their folded paths.
    by_path: HashMap<String, usize>,
    /// What each readable entry reads, by its index.
    readable: HashMap<usize, usize>,
    plans: Vec<Plan>,
    contested: HashSet<String>,
    skipped: Vec<Skipped>,
    conflicts: Vec<PathConflict>,
    outside: Vec<OutsideFile>,
    groups: Vec<FileGroup>,
    group_problem: Option<String>,
}

impl Cabinet {
    /// Open the set whose header is the file `path` on the host: a `.hdr`,
    /// or a `.cab` that starts with the header.
    pub fn open_path(
        path: &std::path::Path,
        compression: Compression,
        limits: &Limits,
    ) -> Result<Cabinet> {
        let dir = path
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_default();
        let dir = if dir.as_os_str().is_empty() {
            std::path::PathBuf::from(".")
        } else {
            dir
        };
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or_else(|| Error::NotFound(format!("{}: no such file", path.display())))?;
        Cabinet::open(Box::new(crate::HostFolder(dir)), &name, compression, limits)
    }

    /// Open the set whose header is `header` in `folder` (a relative path
    /// with `/` between its parts): a `.hdr`, or a `.cab` that starts with
    /// the header. The volumes, and files kept outside them, are looked for
    /// in the header's folder.
    pub fn open(
        folder: Box<dyn Folder>,
        header: &str,
        compression: Compression,
        limits: &Limits,
    ) -> Result<Cabinet> {
        let header = header.replace('\\', "/");
        let (dir, name) = match header.rfind('/') {
            Some(at) => (header[..=at].to_string(), header[at + 1..].to_string()),
            None => (String::new(), header.clone()),
        };
        let end = name
            .find(|c: char| c == '.' || c.is_ascii_digit())
            .unwrap_or(name.len());
        let prefix = name[..end].to_string();
        if prefix.is_empty() {
            return invalid(format!(
                "InstallShield header name '{}' has no volume prefix.",
                name
            ));
        }
        let (stream, length) = match folder.lookup(&header)? {
            Lookup::Found(stream, length) => (stream, length),
            Lookup::Missing => {
                return Err(Error::NotFound(format!(
                    "InstallShield header {} doesn't exist.",
                    header
                )));
            }
            Lookup::Several => {
                return invalid(format!(
                    "Several files match InstallShield header name '{}'.",
                    header
                ));
            }
        };
        let cab = name.to_ascii_lowercase().ends_with(".cab");
        let mut reader = HeaderReader::new(stream, length, &name, cab, limits)?;
        let mut cabinet = Cabinet {
            folder,
            dir,
            prefix,
            major: 0,
            compression,
            members: Vec::new(),
            by_path: HashMap::new(),
            readable: HashMap::new(),
            plans: Vec::new(),
            contested: HashSet::new(),
            skipped: Vec::new(),
            conflicts: Vec::new(),
            outside: Vec::new(),
            groups: Vec::new(),
            group_problem: None,
        };
        cabinet.read_header(&mut reader, limits)?;
        Ok(cabinet)
    }

    /// The major version the header's version word gives: 0, 5 or 6.
    pub fn major_version(&self) -> u32 {
        self.major
    }

    /// How compressed members are read, as the set was opened.
    pub fn compression(&self) -> Compression {
        self.compression
    }

    /// The listed members, by path in any case.
    pub fn members(&self) -> &[Member] {
        &self.members
    }

    /// The entries of the file table that aren't listed, in table order.
    pub fn skipped(&self) -> &[Skipped] {
        &self.skipped
    }

    /// The paths that hold different files, by path.
    pub fn conflicts(&self) -> &[PathConflict] {
        &self.conflicts
    }

    /// The files the cabinet keeps outside its volumes, in table order,
    /// with what was found beside the header.
    pub fn outside_files(&self) -> &[OutsideFile] {
        &self.outside
    }

    /// The file groups the cabinet descriptor lists, in the order Unshield
    /// reads them.
    pub fn file_groups(&self) -> &[FileGroup] {
        &self.groups
    }

    /// Why the file groups didn't read whole, if they didn't. Every entry's
    /// membership is then undetermined.
    pub fn file_group_problem(&self) -> Option<&str> {
        self.group_problem.as_deref()
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
    /// can come after some of it is written: the last check is made when
    /// it has all been expanded.
    pub fn copy(&self, path: &str, out: &mut dyn Write) -> Result<u64> {
        let folded = relative_path(path)
            .map(|p| fold(&p))
            .map_err(|e| Error::NotFound(format!("{}: {}", path, e)))?;
        match self.by_path.get(&folded) {
            Some(&plan) => self.decode(plan, out),
            None if self.contested.contains(&folded) => Err(Error::NotFound(format!(
                "{}: the cabinet holds different files at this path; each is read by its entry's index",
                path
            ))),
            None => Err(Error::NotFound(format!(
                "{}: the cabinet has no such file",
                path
            ))),
        }
    }

    /// The file the entry `index` of the file table holds, expanded and
    /// checked: a listed member's entry or one sharing its data, a file of
    /// a path that holds different files, or a file kept outside the
    /// cabinet that was found beside the header.
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
                "InstallShield cabinet file {} holds no file the reader reads.",
                index
            ))),
        }
    }

    fn decode(&self, plan: usize, out: &mut dyn Write) -> Result<u64> {
        let plan = &self.plans[plan];
        let open = |volume: u32| -> Result<Box<dyn ReadSeek>> {
            match &plan.source {
                Source::Outside { path, stored } => {
                    let (stream, length) =
                        self.find_file(path, "file stored outside the cabinet")?;
                    if length > *stored {
                        return invalid(format!(
                            "InstallShield file stored outside the cabinet at '{}' is longer than when the set was opened.",
                            path
                        ));
                    }
                    Ok(stream)
                }
                Source::Volumes(_) => Ok(self.open_volume(volume)?.0),
            }
        };
        member::decode_cabinet(plan, self.compression, &open, out)
    }

    fn find_file(&self, path: &str, what: &str) -> Result<(Box<dyn ReadSeek>, u64)> {
        match self.folder.lookup(&format!("{}{}", self.dir, path))? {
            Lookup::Found(stream, length) => Ok((stream, length)),
            Lookup::Missing => Err(Error::NotFound(format!(
                "InstallShield {} {} was not found.",
                what, path
            ))),
            Lookup::Several => invalid(format!(
                "Several files match the name of InstallShield {} {}.",
                what, path
            )),
        }
    }

    fn open_volume(&self, volume: u32) -> Result<(Box<dyn ReadSeek>, u64)> {
        self.find_file(
            &format!("{}{}.cab", self.prefix, volume),
            &format!("volume {}", volume),
        )
        .map_err(|e| match e {
            Error::NotFound(_) => Error::NotFound(format!(
                "InstallShield volume {} ({}{}.cab) was not found.",
                volume, self.prefix, volume
            )),
            e => e,
        })
    }

    fn read_header(&mut self, reader: &mut HeaderReader, limits: &Limits) -> Result<()> {
        let signature = reader.u32(0)?;
        if signature == MICROSOFT_CABINET {
            return invalid("The file is a Microsoft cabinet, not an InstallShield cabinet.");
        }
        if signature != SIGNATURE {
            return invalid("The file isn't an InstallShield cabinet header.");
        }
        let version = reader.u32(4)?;
        self.major = major_version(version).ok_or_else(|| {
            Error::Unsupported(format!(
                "InstallShield cabinet version word {:#010x} follows neither encoding the reader knows.",
                version
            ))
        })?;
        if !matches!(self.major, 0 | 5 | 6) {
            return Err(Error::Unsupported(format!(
                "InstallShield cabinet version word {:#010x} reads as major version {}; only 0, 5 and 6 are read.",
                version, self.major
            )));
        }
        let v5_layout = self.major != 6;

        let descriptor = reader.u32(12)? as u64;
        // As in Unshield, the declared descriptor size only has to be
        // nonzero: it bounds nothing the reader reads.
        if reader.u32(16)? == 0 {
            return invalid("InstallShield header has no cabinet descriptor.");
        }
        reader.require(descriptor, DESCRIPTOR_FIELDS_SIZE, "cabinet descriptor")?;
        let table = descriptor + reader.u32(descriptor + 0x0c)? as u64;
        let directory_count = reader.u32(descriptor + 0x1c)? as u64;
        let file_count = reader.u32(descriptor + 0x28)? as u64;
        let descriptors_offset = reader.u32(descriptor + 0x2c)? as u64;
        if file_count > limits.max_files as u64 {
            return invalid(format!(
                "InstallShield cabinet declares {} files, more than the limit of {}.",
                file_count, limits.max_files
            ));
        }
        // The directories' part of the file table has to be in the header
        // before their names are read, which also bounds the vector below.
        reader.require(table, 4 * directory_count, "file table")?;
        let file_count = file_count as usize;

        let mut directories: Vec<Option<String>> = vec![None; directory_count as usize];
        let mut portable: Vec<Option<String>> = vec![None; directory_count as usize];

        let mut descriptors = Vec::with_capacity(file_count);
        for index in 0..file_count {
            let d = if v5_layout {
                let at = table + reader.u32(table + 4 * (directory_count + index as u64))? as u64;
                let size = if self.major == 0 {
                    VERSION0_DESCRIPTOR_SIZE
                } else {
                    VERSION5_DESCRIPTOR_SIZE
                };
                reader.require(at, size, "file descriptor")?;
                Descriptor {
                    flags: reader.u16(at + 8)?,
                    expanded: reader.u32(at + 10)? as u64,
                    compressed: reader.u32(at + 14)? as u64,
                    data_offset: reader.u32(at + 0x26)? as u64,
                    md5: None,
                    name_offset: reader.u32(at)?,
                    directory: reader.u16(at + 4)?,
                    link_previous: 0,
                    link_flags: 0,
                    volume: 0,
                }
            } else {
                let at = table + descriptors_offset + index as u64 * VERSION6_DESCRIPTOR_SIZE;
                reader.require(at, VERSION6_DESCRIPTOR_SIZE, "file descriptor")?;
                let mut md5 = [0u8; 16];
                for (i, b) in md5.iter_mut().enumerate() {
                    *b = reader.u8(at + 0x1a + i as u64)?;
                }
                Descriptor {
                    flags: reader.u16(at)?,
                    expanded: reader.u64(at + 2)?,
                    compressed: reader.u64(at + 0x0a)?,
                    data_offset: reader.u64(at + 0x12)?,
                    md5: Some(md5),
                    name_offset: reader.u32(at + 0x3a)?,
                    directory: reader.u16(at + 0x3e)?,
                    link_previous: reader.u32(at + 0x4c)?,
                    link_flags: reader.u8(at + 0x54)?,
                    volume: reader.u16(at + 0x55)?,
                }
            };
            descriptors.push(d);
        }

        // The path of an entry, and its directory and name with `/`
        // between parts. When the path is accepted, so is the directory,
        // whose characters and parts are among the path's.
        let mut entry_path = |reader: &mut HeaderReader,
                              index: usize,
                              d: &Descriptor|
         -> Result<(String, String, String)> {
            let name = reader.string(table + d.name_offset as u64, "file name")?;
            let dir_index = d.directory as usize;
            if dir_index >= directories.len() {
                return invalid(format!(
                    "InstallShield file {} names directory {}, which doesn't exist.",
                    index, d.directory
                ));
            }
            if directories[dir_index].is_none() {
                let offset = reader.u32(table + 4 * dir_index as u64)? as u64;
                directories[dir_index] = Some(reader.string(table + offset, "directory name")?);
            }
            let directory = directories[dir_index].clone().unwrap_or_default();
            let combined = if directory.is_empty() {
                name.clone()
            } else {
                format!("{}\\{}", directory, name)
            };
            let path = relative_path(&combined).map_err(|e| {
                Error::Invalid(format!(
                    "InstallShield file {} has a path that isn't portable: {}.",
                    index, e
                ))
            })?;
            if portable[dir_index].is_none() {
                portable[dir_index] = Some(if directory.is_empty() {
                    String::new()
                } else {
                    directory.replace('\\', "/")
                });
            }
            Ok((
                path,
                portable[dir_index].clone().unwrap_or_default(),
                name.replace('\\', "/"),
            ))
        };

        let mut skipped = Vec::new();
        let mut files: Vec<File> = Vec::new();
        // The different files at each path (by its folded form), in the
        // order their first entries come in the table.
        let mut by_path: HashMap<String, Vec<usize>> = HashMap::new();
        let mut path_order: Vec<String> = Vec::new();
        let mut expanded_total: u64 = 0;
        let mut volumes: HashMap<u32, VolumeHeader> = HashMap::new();
        for index in 0..file_count {
            let d = descriptors[index];
            let reason = if d.flags & FLAG_INVALID != 0 {
                Some((
                    SkipKind::MarkedInvalid,
                    "The cabinet marks the file invalid.",
                ))
            } else if d.name_offset == 0 {
                Some((SkipKind::NoName, "The file has no name."))
            } else if d.data_offset == 0 {
                Some((SkipKind::NoDataOffset, "The file has no data offset."))
            } else {
                None
            };
            if let Some((kind, reason)) = reason {
                // As in Unshield, the name of an entry left out doesn't have
                // to read: its path is recorded when it reads.
                let path = if d.name_offset != 0 {
                    match entry_path(reader, index, &d) {
                        Ok((path, _, _)) => Some(path),
                        Err(Error::Invalid(_)) => None,
                        Err(e) => return Err(e),
                    }
                } else {
                    None
                };
                skipped.push(Skipped {
                    index,
                    path,
                    kind,
                    reason: reason.to_string(),
                });
                continue;
            }
            // Any other entry's path has to read and be relative.
            let (path, directory, name) = entry_path(reader, index, &d)?;

            let data_index = resolve_link(&descriptors, index)?;
            let data = descriptors[data_index];
            let link_reason = if data.flags & FLAG_INVALID != 0 {
                Some(format!(
                    "The file links to file {}, which the cabinet marks invalid.",
                    data_index
                ))
            } else if data.data_offset == 0 {
                Some(format!(
                    "The file links to file {}, which has no data offset.",
                    data_index
                ))
            } else {
                None
            };
            if let Some(reason) = link_reason {
                skipped.push(Skipped {
                    index,
                    path: Some(path),
                    kind: SkipKind::LinksToUnavailableEntry,
                    reason,
                });
                continue;
            }
            if data.expanded > i64::MAX as u64 || data.compressed > i64::MAX as u64 {
                return invalid(format!(
                    "InstallShield file {} declares a size beyond 2^63 bytes.",
                    index
                ));
            }
            let entry_name = EntryName {
                index,
                directory_index: d.directory as usize,
                directory,
                name,
            };
            let key = fold(&path);
            if !by_path.contains_key(&key) {
                path_order.push(key.clone());
            }
            let at_path = by_path.entry(key).or_default();

            if let Some(&linked) = at_path.iter().find(|&&f| files[f].data_index == data_index) {
                let file = &mut files[linked];
                file.shared_by.push(entry_name);
                let record = if file.segments.is_none() {
                    (
                        Skipped {
                            index,
                            path: Some(path),
                            kind: SkipKind::StoredOutside,
                            reason: format!(
                                "The file shares the data of file {} at '{}', which is stored outside the cabinet.",
                                file.index, file.path
                            ),
                        },
                        None,
                    )
                } else {
                    (
                        Skipped {
                            index,
                            path: Some(path),
                            kind: SkipKind::SharesListedData,
                            reason: format!(
                                "The file shares the data of file {} at '{}', which is listed.",
                                file.index, file.path
                            ),
                        },
                        Some(format!(
                            "The file shares the data of file {} at '{}'.",
                            file.index, file.path
                        )),
                    )
                };
                file.skipped.push(record);
                continue;
            }
            // Version 6 records each file's MD5, so two copies stored apart
            // can be told to be the same file. The copy kept is the one
            // checked when read; the other isn't read, nor are its volumes.
            let same = at_path.iter().copied().find(|&f| {
                files[f].md5.is_some()
                    && data.md5.is_some()
                    && files[f].size == data.expanded
                    && files[f].md5 == data.md5
            });
            if let Some(same) = same.filter(|&f| files[f].segments.is_some()) {
                let file = &mut files[same];
                let duplicate = format!(
                    "The file duplicates file {} at '{}': same expanded size and MD5.",
                    file.index, file.path
                );
                file.shared_by.push(entry_name);
                file.skipped.push((
                    Skipped {
                        index,
                        path: Some(path),
                        kind: SkipKind::DuplicatesListedMember,
                        reason: duplicate.clone(),
                    },
                    Some(duplicate),
                ));
                continue;
            }

            let (segments, outside_volume) =
                self.segments(&mut volumes, data_index, &data, file_count)?;
            let compressed = data.flags & FLAG_COMPRESSED != 0;
            let mut file = File {
                path: path.clone(),
                size: data.expanded,
                index,
                data_index,
                compressed,
                obfuscated: data.flags & FLAG_OBFUSCATED != 0,
                md5: data.md5,
                segments,
                listed: entry_name.clone(),
                shared_by: Vec::new(),
                skipped: Vec::new(),
                stored_size: if compressed {
                    data.compressed
                } else {
                    data.expanded
                },
                lookup_path: None,
                found_length: None,
                outside_status: None,
                member: None,
            };
            if file.segments.is_none() {
                // Unshield looks for a file stored outside by the directory
                // and name of the entry that holds its data: for a link, the
                // entry the link ends at.
                file.lookup_path = if data_index == index {
                    Some(path.clone())
                } else if data.name_offset == 0 {
                    None
                } else {
                    Some(entry_path(reader, data_index, &data)?.0)
                };
                let evidence = format!(
                    "its data offset, {}, is the length of volume {}, which would hold it",
                    data.data_offset, outside_volume
                );
                let reason = match same {
                    Some(same) => format!(
                        "The file duplicates file {} at '{}' (same expanded size and MD5), and is also stored outside the cabinet: {}.",
                        files[same].index, files[same].path, evidence
                    ),
                    None if data_index == index => {
                        format!("The file is stored outside the cabinet: {}.", evidence)
                    }
                    None => format!(
                        "The file links to file {}, which is stored outside the cabinet: {}.",
                        data_index, evidence
                    ),
                };
                let record = Skipped {
                    index,
                    path: Some(path),
                    kind: SkipKind::StoredOutside,
                    reason,
                };
                match same {
                    Some(same) => {
                        files[same].shared_by.push(entry_name);
                        files[same].skipped.push((record, None));
                    }
                    None => {
                        file.skipped.push((record, None));
                        files.push(file);
                        at_path.push(files.len() - 1);
                    }
                }
                continue;
            }
            match same {
                Some(same) => {
                    // A version 6 copy of a file stored outside is read when
                    // its own data is inside: it takes the place of the
                    // entries stored outside and shares them.
                    let old = &mut files[same];
                    let mut shared = vec![old.listed.clone()];
                    shared.append(&mut old.shared_by);
                    file.shared_by = shared;
                    file.skipped = std::mem::take(&mut old.skipped);
                    files[same] = file;
                }
                None => {
                    files.push(file);
                    at_path.push(files.len() - 1);
                }
            }
            if data.expanded > limits.max_expanded_bytes - expanded_total {
                return invalid(format!(
                    "InstallShield cabinet expands to more than the limit of {} bytes.",
                    limits.max_expanded_bytes
                ));
            }
            expanded_total += data.expanded;
        }

        let mut listed: Vec<usize> = Vec::new();
        let mut conflicts: Vec<(String, Vec<usize>)> = Vec::new();
        for key in &path_order {
            let at_path = &by_path[key];
            if at_path.len() == 1 {
                let only = &files[at_path[0]];
                skipped.extend(only.skipped.iter().map(|(s, _)| s.clone()));
                if only.segments.is_some() {
                    listed.push(at_path[0]);
                }
                continue;
            }
            // A copy inside the cabinet that took the place of entries
            // stored outside is read from its own entry, but the file's
            // first entry is the first of those.
            let firsts: Vec<usize> = at_path
                .iter()
                .map(|&f| {
                    files[f]
                        .shared_by
                        .iter()
                        .map(|e| e.index)
                        .chain([files[f].listed.index])
                        .min()
                        .unwrap_or(0)
                })
                .collect();
            let held = held_reason(&firsts);
            for &f in at_path {
                let file = &files[f];
                if file.segments.is_some() {
                    skipped.push(Skipped {
                        index: file.index,
                        path: Some(file.path.clone()),
                        kind: SkipKind::PathHeldByDifferentFiles,
                        reason: held.clone(),
                    });
                }
                // Entries stored outside keep that kind: it is why they
                // aren't read.
                for (record, contested) in &file.skipped {
                    let mut record = record.clone();
                    if let Some(contested) = contested {
                        record.kind = SkipKind::PathHeldByDifferentFiles;
                        record.reason = format!("{} {}", held, contested);
                    }
                    skipped.push(record);
                }
            }
            conflicts.push((files[at_path[0]].path.clone(), at_path.clone()));
        }
        skipped.sort_by_key(|s| s.index);
        listed.sort_by_key(|&f| fold(&files[f].path));
        conflicts.sort_by_key(|(path, _)| fold(path));

        // Each file stored outside is looked up beside the header, at the
        // path of the entry that holds its data. Only one found there with
        // exactly its stored length is read, and only when neither its own
        // path nor the path looked up holds other files.
        let mut outside: Vec<(usize, bool)> = by_path
            .values()
            .flat_map(|at| {
                at.iter()
                    .filter(|&&f| files[f].segments.is_none())
                    .map(move |&f| (f, at.len() > 1))
            })
            .collect();
        outside.sort_by_key(|&(f, _)| files[f].index);
        for &(f, contested) in &outside {
            let Some(lookup) = files[f].lookup_path.clone() else {
                files[f].outside_status = Some(OutsideStatus::NoUsablePath);
                continue;
            };
            let found = match self.folder.lookup(&format!("{}{}", self.dir, lookup)) {
                Ok(Lookup::Found(_, length)) => Some((1, length)),
                Ok(Lookup::Missing) => Some((0, 0)),
                Ok(Lookup::Several) => Some((2, 0)),
                Err(_) => None,
            };
            let Some((matches, length)) = found else {
                files[f].outside_status = Some(OutsideStatus::LookupFailed);
                continue;
            };
            let file = &mut files[f];
            file.found_length = (matches == 1).then_some(length);
            let held_by_others =
                contested || by_path.get(&fold(&lookup)).is_some_and(|at| at.len() > 1);
            let status = if held_by_others {
                OutsideStatus::PathHeldByDifferentFiles
            } else if matches == 0 {
                OutsideStatus::Missing
            } else if matches > 1 {
                OutsideStatus::SeveralMatches
            } else if length != file.stored_size {
                OutsideStatus::LengthDiffers
            } else {
                OutsideStatus::Available
            };
            file.outside_status = Some(status);
            if status != OutsideStatus::Available {
                continue;
            }
            if file.size > limits.max_expanded_bytes - expanded_total {
                return invalid(format!(
                    "InstallShield cabinet expands to more than the limit of {} bytes.",
                    limits.max_expanded_bytes
                ));
            }
            expanded_total += file.size;
        }

        // The file groups are read last, from the header already read, so
        // a set that opens without them opens the same with them. They are
        // read for every entry of a listed member, of a conflict and of a
        // file stored outside.
        let described: Vec<usize> = listed
            .iter()
            .copied()
            .chain(conflicts.iter().flat_map(|(_, at)| at.iter().copied()))
            .chain(
                outside
                    .iter()
                    .filter(|(_, contested)| !contested)
                    .map(|&(f, _)| f),
            )
            .collect();
        let mut entries: Vec<usize> = described
            .iter()
            .flat_map(|&f| {
                files[f]
                    .shared_by
                    .iter()
                    .map(|e| e.index)
                    .chain([files[f].listed.index])
            })
            .collect();
        entries.sort_unstable();
        entries.dedup();
        let table_groups = GroupTable::read(reader, descriptor, self.major, file_count, limits)?;
        let (memberships, problem) = table_groups.memberships(&entries);
        self.groups = table_groups.groups;
        self.group_problem = problem;
        let info = |e: &EntryName| EntryInfo {
            index: e.index,
            directory_index: e.directory_index,
            directory: e.directory.clone(),
            name: e.name.clone(),
            groups: memberships
                .get(&e.index)
                .cloned()
                .unwrap_or_else(Membership::default),
        };
        for &f in &described {
            let file = &files[f];
            let member = Member {
                path: file.path.clone(),
                size: file.size,
                entry: info(&file.listed),
                shared_by: file.shared_by.iter().map(info).collect(),
            };
            let readable =
                file.segments.is_some() || file.outside_status == Some(OutsideStatus::Available);
            if readable {
                let plan = self.plans.len();
                self.plans.push(Plan {
                    path: file.path.clone(),
                    size: file.size,
                    compressed: file.compressed,
                    obfuscated: file.obfuscated,
                    md5: file.md5,
                    source: match &file.segments {
                        Some(segments) => Source::Volumes(segments.clone()),
                        None => Source::Outside {
                            path: file.lookup_path.clone().unwrap_or_default(),
                            stored: file.stored_size,
                        },
                    },
                });
                for e in file.shared_by.iter().chain([&file.listed]) {
                    self.readable.insert(e.index, plan);
                }
            }
            files[f].member = Some(member);
        }
        for &f in &listed {
            let plan = self.readable[&files[f].listed.index];
            self.by_path.insert(fold(&files[f].path), plan);
            self.members.extend(files[f].member.clone());
        }
        // Outside files on contested paths have no metadata of their own
        // above; their entries are given in the conflict.
        self.outside = outside
            .iter()
            .map(|&(f, _)| {
                let file = &files[f];
                let member = file.member.clone().unwrap_or_else(|| Member {
                    path: file.path.clone(),
                    size: file.size,
                    entry: info(&file.listed),
                    shared_by: file.shared_by.iter().map(info).collect(),
                });
                OutsideFile {
                    member,
                    lookup_path: file.lookup_path.clone(),
                    stored_size: file.stored_size,
                    compressed: file.compressed,
                    status: file.outside_status.unwrap_or(OutsideStatus::LookupFailed),
                    found_length: file.found_length,
                }
            })
            .collect();
        self.conflicts = conflicts
            .iter()
            .map(|(path, at)| {
                let mut stored_outside: Vec<EntryInfo> = at
                    .iter()
                    .filter(|&&f| files[f].segments.is_none())
                    .flat_map(|&f| files[f].shared_by.iter().chain([&files[f].listed]))
                    .map(info)
                    .collect();
                stored_outside.sort_by_key(|e| e.index);
                PathConflict {
                    path: path.clone(),
                    files: at
                        .iter()
                        .filter(|&&f| files[f].segments.is_some())
                        .filter_map(|&f| files[f].member.clone())
                        .collect(),
                    stored_outside,
                }
            })
            .collect();
        for (path, _) in &conflicts {
            self.contested.insert(fold(path));
        }
        self.skipped = skipped;
        Ok(())
    }

    /// Where a file's stored bytes are, following the volume headers as
    /// Unshield does: None, with the volume, when the file is stored
    /// outside the cabinet, its data offset being the length of the volume
    /// where its data starts, split or not. A file with no stored bytes
    /// reads as empty wherever its offset points, so it is never outside.
    /// Any other extent past the end of a volume is damage.
    fn segments(
        &self,
        volumes: &mut HashMap<u32, VolumeHeader>,
        index: usize,
        d: &Descriptor,
        file_count: usize,
    ) -> Result<(Option<Vec<Segment>>, u32)> {
        let compressed = d.flags & FLAG_COMPRESSED != 0;
        let mut remaining = if compressed { d.compressed } else { d.expanded };
        let v5_layout = self.major != 6;
        let mut volume = if v5_layout { 1 } else { d.volume as u32 };
        let mut header = self.volume(volumes, volume, index)?;
        let mut split = d.flags & FLAG_SPLIT != 0;
        let index64 = index as u64;
        if v5_layout {
            // Version 0 and 5 descriptors name no volume: the file is in
            // the first whose last file index reaches it.
            while index64 > header.last_index {
                volume += 1;
                header = self.volume(volumes, volume, index)?;
            }
            // A version 5 file is also split when that volume's record of
            // it disagrees. Unshield infers this for version 5 only.
            if self.major == 5 {
                split |= (index + 1 < file_count
                    && index64 == header.last_index
                    && header.last_compressed != d.compressed)
                    || (index > 0
                        && index64 == header.first_index
                        && header.first_compressed != d.compressed);
            }
        }
        // Unshield compares the offset with the length of the volume it
        // opens for the file before it reads a split file's parts.
        if remaining > 0 && d.data_offset == header.length {
            return Ok((None, volume));
        }
        let mut segments = Vec::new();
        loop {
            let (offset, length) = if !split {
                (d.data_offset, remaining)
            } else if index64 == header.last_index && header.last_offset != 0x7fff_ffff {
                (
                    header.last_offset,
                    if compressed {
                        header.last_compressed
                    } else {
                        header.last_expanded
                    },
                )
            } else if index64 == header.first_index {
                (
                    header.first_offset,
                    if compressed {
                        header.first_compressed
                    } else {
                        header.first_expanded
                    },
                )
            } else {
                return invalid(format!(
                    "InstallShield file {} is split but volume {} doesn't record it.",
                    index, volume
                ));
            };
            let take = length.min(remaining);
            if take == 0 && remaining > 0 {
                return invalid(format!(
                    "InstallShield volume {} holds none of file {}'s data.",
                    volume, index
                ));
            }
            if take > header.length || offset > header.length - take {
                return invalid(format!(
                    "InstallShield file {} lies past the end of volume {}, which is shorter than the header says.",
                    index, volume
                ));
            }
            if take > 0 {
                segments.push(Segment {
                    volume,
                    offset,
                    length: take,
                });
            }
            remaining -= take;
            if remaining == 0 {
                return Ok((Some(segments), 0));
            }
            if !split {
                return invalid(format!(
                    "InstallShield file {} isn't split but its data goes on past volume {}.",
                    index, volume
                ));
            }
            volume += 1;
            header = self.volume(volumes, volume, index)?;
        }
    }

    fn volume(
        &self,
        volumes: &mut HashMap<u32, VolumeHeader>,
        volume: u32,
        index: usize,
    ) -> Result<VolumeHeader> {
        if !(1..=MAX_VOLUME).contains(&volume) {
            return invalid(format!(
                "InstallShield file {} names volume {}, which is out of range.",
                index, volume
            ));
        }
        if let Some(header) = volumes.get(&volume) {
            return Ok(*header);
        }
        let (mut stream, length) = self.open_volume(volume)?;
        let v5_layout = self.major != 6;
        let mut bytes = vec![0u8; COMMON_HEADER_SIZE + if v5_layout { 40 } else { 64 }];
        if let Err(e) = stream.read_exact(&mut bytes) {
            return if e.kind() == std::io::ErrorKind::UnexpectedEof {
                invalid(format!(
                    "InstallShield volume {} is too short for its header.",
                    volume
                ))
            } else {
                Err(e.into())
            };
        }
        if u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) != SIGNATURE {
            return invalid(format!(
                "InstallShield volume {} isn't an InstallShield cabinet.",
                volume
            ));
        }
        let u32_at = |at: usize| {
            let at = COMMON_HEADER_SIZE + at;
            u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as u64
        };
        let header = if v5_layout {
            VolumeHeader {
                length,
                first_index: u32_at(8),
                last_index: u32_at(12),
                first_offset: u32_at(16),
                first_expanded: u32_at(20),
                first_compressed: u32_at(24),
                last_offset: if u32_at(28) == 0 {
                    0x7fff_ffff
                } else {
                    u32_at(28)
                },
                last_expanded: u32_at(32),
                last_compressed: u32_at(36),
            }
        } else {
            let u64_at = |at: usize| -> Result<u64> {
                let at = COMMON_HEADER_SIZE + at;
                let mut b = [0u8; 8];
                b.copy_from_slice(&bytes[at..at + 8]);
                let value = u64::from_le_bytes(b);
                if value > i64::MAX as u64 {
                    return invalid(format!(
                        "InstallShield volume {} header holds a value beyond 2^63.",
                        volume
                    ));
                }
                Ok(value)
            };
            VolumeHeader {
                length,
                first_index: u32_at(8),
                last_index: u32_at(12),
                first_offset: u64_at(16)?,
                first_expanded: u64_at(24)?,
                first_compressed: u64_at(32)?,
                last_offset: u64_at(40)?,
                last_expanded: u64_at(48)?,
                last_compressed: u64_at(56)?,
            }
        };
        volumes.insert(volume, header);
        Ok(header)
    }
}

/// The major version a cabinet's version word gives, in the two encodings
/// Unshield reads: a top byte of 1 keeps it in bits 12 to 15, a top byte
/// of 2 or 4 keeps a hundredfold version in the low word.
pub(crate) fn major_version(word: u32) -> Option<u32> {
    match word >> 24 {
        1 => Some((word >> 12) & 0xf),
        2 | 4 => Some((word & 0xffff) / 100),
        _ => None,
    }
}

/// The entry a version 6 entry's links end at: the one whose data it
/// shares. A link outside the table, or a cycle, is damage.
fn resolve_link(descriptors: &[Descriptor], index: usize) -> Result<usize> {
    let mut current = index;
    for _ in 0..=descriptors.len() {
        let d = &descriptors[current];
        if d.link_flags & LINK_PREVIOUS == 0 {
            return Ok(current);
        }
        let previous = d.link_previous as usize;
        if previous >= descriptors.len() || previous == current {
            return invalid(format!(
                "InstallShield file {} links to file {}, which doesn't exist.",
                current, d.link_previous
            ));
        }
        current = previous;
    }
    invalid(format!(
        "InstallShield file {} is part of a link cycle.",
        index
    ))
}
