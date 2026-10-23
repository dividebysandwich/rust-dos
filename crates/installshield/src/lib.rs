//! InstallShield's installer archives, listed and read without running
//! the setup program that would unpack them:
//!
//! - Cabinet sets of InstallShield 5 and 6 (`Cabinet`): a header,
//!   `data1.hdr`, and volumes `data1.cab`, `data2.cab` and so on, or a set
//!   whose header is at the start of `data1.cab`. Major versions 0, 5 and
//!   6 are read, laid out as Unshield (MIT) reads them.
//! - InstallShield 3 archives (`Archive`): one file, `_SETUP.1` on a CD or
//!   a `.Z` file inside a self-extractor, whose members are compressed
//!   with PKWARE's Data Compression Library (`explode`).
//! - The archive a setup's 32-bit engine comes in (`EngineArchive`),
//!   `_INST32I.EX_` beside an InstallShield 3 to 5 `SETUP.EXE`.
//!
//! The readers follow the design of the refurbished-dinosaurs toolkit's
//! (`RefurbishedDinosaurs.LegacyFormats`, MIT), whose decision records
//! 0014 and 0018 to 0021 explain the choices: everything the header
//! declares is checked when the set is opened, before any member is read
//! (relative paths, data inside the volumes, limits on counts and sizes);
//! entries that can't be read are listed with the reason, and the rest of
//! the set still opens; a path that holds two different files lists
//! neither, and each is read by its entry's index. Nothing in a file makes
//! them panic.
//!
//! Reading a member checks it as far as the format allows: its size, and
//! for version 6 cabinets the MD5 the header records. Version 0 and 5
//! cabinets and InstallShield 3 archives record no checksum, so their
//! members are checked by size and framing only.

#![forbid(unsafe_code)]

use std::io::{self, Read, Seek};
use std::path::{Path, PathBuf};

mod archive;
mod cabinet;
mod engine;
mod explode;
mod groups;
mod member;
#[cfg(test)]
mod tests;
#[cfg(any(test, feature = "test-util"))]
pub mod write;

pub use archive::Archive;
pub use cabinet::Cabinet;
pub use engine::{EngineArchive, EngineFile, is_engine_archive};
pub use explode::explode;
pub use groups::{FileGroup, Membership, MembershipKind};

/// Why an archive can't be opened, or a member read.
#[derive(Debug)]
pub enum Error {
    /// The file couldn't be read.
    Io(io::Error),
    /// A file the set needs isn't there: a volume, or a member asked for.
    NotFound(String),
    /// What the file holds isn't what the format allows, or breaks a
    /// limit.
    Invalid(String),
    /// A version or form of the format the reader doesn't read.
    Unsupported(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{}", e),
            Error::NotFound(s) | Error::Invalid(s) | Error::Unsupported(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Error {
        Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

fn invalid<T>(message: impl Into<String>) -> Result<T> {
    Err(Error::Invalid(message.into()))
}

/// Bounds on what an archive may declare, checked when it is opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// The most entries its file table may hold.
    pub max_files: usize,
    /// The most bytes the files it lists may expand to, together.
    pub max_expanded_bytes: u64,
    /// The most header bytes read into memory: a cabinet's header, or an
    /// InstallShield 3 archive's two tables together. A `.cab` that holds
    /// its header is read from its start only as far as the header's
    /// structures reach, so a cabinet of any size opens.
    pub max_header_bytes: u64,
    /// The longest file, directory or group name in a cabinet's header,
    /// in bytes before its terminating NUL.
    pub max_name_bytes: usize,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            max_files: 100_000,
            max_expanded_bytes: 8 << 30,
            max_header_bytes: 64 << 20,
            max_name_bytes: 1024,
        }
    }
}

/// How the compressed members of a cabinet set hold their deflate data.
/// Nothing in the header says which one a set uses, so the caller names
/// it, and every compressed member is read that way; a member that doesn't
/// decode in that form fails its read, and is never tried in the other.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Compression {
    /// Chunks, each a 16-bit length and that many bytes of raw deflate
    /// data that expand to 64 KB at most: what Unshield reads by default.
    #[default]
    Chunks,
    /// One raw deflate stream without lengths, flushed after each chunk,
    /// so each ends with the empty stored block `00 00 FF FF` and none is
    /// final: what Unshield reads with `-O`, found in some version 0 sets.
    Markers,
}

impl Compression {
    /// The form by its name in a profile or on the command line:
    /// `chunks`, or `markers` (also `old`, after Unshield's `-O`).
    pub fn parse(text: &str) -> Option<Compression> {
        match text.trim().to_ascii_lowercase().as_str() {
            "chunks" => Some(Compression::Chunks),
            "markers" | "old" => Some(Compression::Markers),
            _ => None,
        }
    }
}

/// Why an entry of the file table isn't listed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipKind {
    /// The cabinet or archive marks it invalid.
    MarkedInvalid,
    /// It has no name.
    NoName,
    /// It has no data offset.
    NoDataOffset,
    /// A version 6 entry whose links end at an entry that is marked
    /// invalid or has no data offset.
    LinksToUnavailableEntry,
    /// It shares the data of the listed member at its path through a
    /// link.
    SharesListedData,
    /// A version 6 entry stored apart from the listed member at its path,
    /// with the same size and MD5.
    DuplicatesListedMember,
    /// Its data isn't in the cabinet's volumes: its data offset is the
    /// length of the volume where its data would start, which is how
    /// Unshield tells such a file. `Cabinet::outside_files` says whether
    /// the file was found beside the header.
    StoredOutside,
    /// Its path, in any case, holds two or more files the reader can't
    /// show to be the same, so the path lists none of them
    /// (`Cabinet::conflicts`).
    PathHeldByDifferentFiles,
}

impl SkipKind {
    /// A short name for listings.
    pub fn name(self) -> &'static str {
        match self {
            SkipKind::MarkedInvalid => "invalid",
            SkipKind::NoName => "no name",
            SkipKind::NoDataOffset => "no data",
            SkipKind::LinksToUnavailableEntry => "bad link",
            SkipKind::SharesListedData => "shared",
            SkipKind::DuplicatesListedMember => "duplicate",
            SkipKind::StoredOutside => "outside",
            SkipKind::PathHeldByDifferentFiles => "conflict",
        }
    }
}

/// An entry of the file table that isn't listed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skipped {
    /// Its index in the file table.
    pub index: usize,
    /// Its path, when its name reads and is a relative path.
    pub path: Option<String>,
    pub kind: SkipKind,
    /// Why, naming any other entry the reason refers to.
    pub reason: String,
}

/// What the header records of one entry of the file table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryInfo {
    /// Its index in the file table, which `read_entry` takes.
    pub index: usize,
    /// The index of its directory.
    pub directory_index: usize,
    /// Its directory with `/` between the parts, or empty for the root.
    pub directory: String,
    /// Its name, with any `\` written as `/`.
    pub name: String,
    /// The file groups whose ranges hold it (cabinets only).
    pub groups: Membership,
}

/// A file of an archive, from the entry it is read from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    /// Its directory and name joined with `/`.
    pub path: String,
    /// Its size, expanded.
    pub size: u64,
    pub entry: EntryInfo,
    /// The other entries at its path that hold the same data: those that
    /// link to it, and version 6 copies with its size and MD5. Reading the
    /// member gives their bytes too.
    pub shared_by: Vec<EntryInfo>,
}

/// A path, in any case, that holds two or more files the reader can't
/// show to be the same. None of them is listed at the path; each is read
/// by its entry's index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathConflict {
    /// The path as its first entry spells it.
    pub path: String,
    /// The files at the path whose data the reader reads, in the order of
    /// their first entries.
    pub files: Vec<Member>,
    /// The entries at the path whose data is stored outside the cabinet.
    pub stored_outside: Vec<EntryInfo>,
}

/// What was found where a file stored outside a cabinet was looked for:
/// beside the header, at its directory and name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutsideStatus {
    /// One file is there, as long as the entry's stored bytes, and the
    /// path holds no other file of the set: `read_entry` reads it.
    Available,
    /// No file is there.
    Missing,
    /// One file is there, of another length. It isn't read.
    LengthDiffers,
    /// Several files match the path in different cases.
    SeveralMatches,
    /// The path holds other files of the set too, so a file found there
    /// can't be told to be this one.
    PathHeldByDifferentFiles,
    /// The entry that holds the data is a version 6 link target without a
    /// name: there is no path to look up.
    NoUsablePath,
    /// The lookup failed (a folder couldn't be read).
    LookupFailed,
}

/// A file a cabinet keeps outside its volumes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutsideFile {
    pub member: Member,
    /// Where it was looked for, beside the header.
    pub lookup_path: Option<String>,
    /// The bytes the header says the cabinet stores for it.
    pub stored_size: u64,
    pub compressed: bool,
    pub status: OutsideStatus,
    /// The length of the file found, when exactly one was.
    pub found_length: Option<u64>,
}

/// A stream the readers read and seek in.
pub trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}

/// What a folder holds at a path.
pub enum Lookup {
    Missing,
    /// The one file there, opened, and its length.
    Found(Box<dyn ReadSeek>, u64),
    /// More than one file matches in different cases.
    Several,
}

/// The folder an archive is in, where a cabinet's volumes and the files
/// it keeps outside them are looked up: a host folder (`HostFolder`), or
/// a folder on an emulated drive.
pub trait Folder {
    /// The file at `path`, relative to the folder with `/` between its
    /// parts, each part matched in any case.
    fn lookup(&self, path: &str) -> io::Result<Lookup>;
}

/// A folder of the host's file system.
#[derive(Clone, Debug)]
pub struct HostFolder(pub PathBuf);

impl HostFolder {
    /// The host paths at `path` under the folder, each part matched in any
    /// case.
    fn matches(&self, path: &str) -> io::Result<Vec<PathBuf>> {
        let parts: Vec<&str> = path.split('/').collect();
        let mut current = vec![self.0.clone()];
        for (i, part) in parts.iter().enumerate() {
            let last = i + 1 == parts.len();
            let wanted = fold(part);
            let mut next = Vec::new();
            for dir in &current {
                let entries = match std::fs::read_dir(dir) {
                    Ok(entries) => entries,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                    Err(e) => return Err(e),
                };
                for entry in entries {
                    let entry = entry?;
                    if fold(&entry.file_name().to_string_lossy()) != wanted {
                        continue;
                    }
                    let kind = entry.file_type()?;
                    // A link is followed to what it names.
                    let (file, dir) = if kind.is_symlink() {
                        let meta = std::fs::metadata(entry.path())?;
                        (meta.is_file(), meta.is_dir())
                    } else {
                        (kind.is_file(), kind.is_dir())
                    };
                    if (last && file) || (!last && dir) {
                        next.push(entry.path());
                    }
                }
            }
            current = next;
            if current.is_empty() {
                break;
            }
        }
        Ok(current)
    }
}

impl Folder for HostFolder {
    fn lookup(&self, path: &str) -> io::Result<Lookup> {
        let mut matches = self.matches(path)?;
        match matches.len() {
            0 => Ok(Lookup::Missing),
            1 => {
                let file = std::fs::File::open(matches.remove(0))?;
                let length = file.metadata()?.len();
                Ok(Lookup::Found(Box::new(io::BufReader::new(file)), length))
            }
            _ => Ok(Lookup::Several),
        }
    }
}

/// Which kind of InstallShield file `path` is, by its first bytes: a
/// cabinet's header (`.hdr`, or a `.cab` that holds one), or an
/// InstallShield 3 archive. None for anything else.
pub fn sniff(path: &Path) -> io::Result<Option<Kind>> {
    let mut start = [0u8; 4];
    let mut file = std::fs::File::open(path)?;
    let mut read = 0;
    while read < 4 {
        match file.read(&mut start[read..])? {
            0 => return Ok(None),
            n => read += n,
        }
    }
    Ok(kind_of(&start))
}

/// The kinds of InstallShield files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Cabinet,
    Archive,
}

/// The kind of InstallShield file that starts with `start`.
pub fn kind_of(start: &[u8]) -> Option<Kind> {
    let word = u32::from_le_bytes(start.get(..4)?.try_into().ok()?);
    match word {
        cabinet::SIGNATURE => Some(Kind::Cabinet),
        archive::SIGNATURE => Some(Kind::Archive),
        _ => None,
    }
}

/// `text` folded for comparing in any case.
fn fold(text: &str) -> String {
    text.chars().flat_map(char::to_lowercase).collect()
}

/// Bytes of a header's name, as ISO 8859-1, which InstallShield's names
/// are in.
fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

/// The names Windows keeps for devices, which no file can have.
const DEVICES: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// `path` with `/` between its parts, if it is a relative path that every
/// system can hold: no root, drive or `..`, no empty part, no control
/// character, none of the characters Windows refuses in names, no part
/// ending in a dot or space and no device name. Otherwise why not.
pub fn relative_path(path: &str) -> std::result::Result<String, String> {
    let normalized = path.replace('\\', "/");
    if normalized.trim().is_empty() {
        return Err("it is empty".to_string());
    }
    if normalized.starts_with('/') {
        return Err("it is rooted".to_string());
    }
    if let Some(c) = normalized.chars().find(|&c| c < ' ' || c == '\u{7f}') {
        return Err(format!("it holds the control character U+{:04X}", c as u32));
    }
    if let Some(c) = normalized.chars().find(|c| "<>:\"|?*".contains(*c)) {
        return Err(format!(
            "it holds '{}', which Windows doesn't allow in names",
            c
        ));
    }
    for part in normalized.split('/') {
        if part.is_empty() {
            return Err("it has an empty part".to_string());
        }
        if part == "." || part == ".." {
            return Err(format!("it has a '{}' part", part));
        }
        if part.ends_with('.') || part.ends_with(' ') {
            return Err(format!("its part '{}' ends with a dot or a space", part));
        }
        let stem = part.split('.').next().unwrap_or(part).trim_end_matches(' ');
        if DEVICES.contains(&fold(stem).as_str()) {
            return Err(format!("its part '{}' is the device name {}", part, stem));
        }
    }
    Ok(normalized)
}

/// The reason each entry at a path that holds different files is skipped
/// with, naming the first entry of each file: "files 3 and 14".
fn held_reason(firsts: &[usize]) -> String {
    let all: Vec<String> = firsts.iter().map(usize::to_string).collect();
    let (last, rest) = all
        .split_last()
        .map_or(("", &[][..]), |(l, r)| (l.as_str(), r));
    format!(
        "The path holds different files: files {} and {}, so none is listed at it.",
        rest.join(", "),
        last
    )
}
