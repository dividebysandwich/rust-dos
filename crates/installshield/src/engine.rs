//! The archive an InstallShield 3 to 5 setup carries its 32-bit engine in,
//! `_INST32I.EX_` beside the 16-bit `SETUP.EXE` that unpacks it into the
//! temporary folder and starts it. Its layout, as the files on Enemy
//! Infestation's (1998) and Wages of War's (1996) CDs have it, all numbers
//! little-endian:
//!
//! - 0h: the signature `2A AB 79 D8`, then the word 100h;
//! - 8h: Stirling Technologies' copyright, 70 bytes;
//! - 4Eh: the number of files, a dword;
//! - 52h: an entry for each file: the offset of its data in the archive,
//!   its size there and expanded (dwords), 8 bytes of zeros, then two
//!   names, each a 16-bit length and that many bytes: the name the file
//!   had when it was built (`INSTALL.EXE`) and the name the setup gives
//!   it in the temporary folder (`_INS0432._MP`);
//! - the files' data, each imploded with PKWARE's Data Compression
//!   Library (`explode`).

use crate::{Error, Result, invalid, latin1};

/// The first four bytes of an engine archive, as a little-endian dword.
pub const SIGNATURE: u32 = 0xD879_AB2A;

const COUNT: usize = 0x4E;
const ENTRIES: usize = 0x52;
const ENTRY: usize = 20;
/// More files than any engine archive has.
const MAX_FILES: usize = 64;
/// More than the files of any engine archive expand to, together.
const MAX_EXPANDED: u64 = 64 << 20;

/// One file of an engine archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineFile {
    /// The name the file had when the archive was built: `INSTALL.EXE`
    /// for the engine itself.
    pub name: String,
    /// The name the setup gives it in the temporary folder.
    pub extract_as: String,
    offset: usize,
    stored: usize,
    size: usize,
}

impl EngineFile {
    /// The file's size, expanded.
    pub fn size(&self) -> usize {
        self.size
    }
}

/// An engine archive's table, read from its bytes.
#[derive(Clone, Debug)]
pub struct EngineArchive {
    files: Vec<EngineFile>,
}

impl EngineArchive {
    /// Read the table of the engine archive `data`, checking that every
    /// name is a plain file name and every file's data is inside it.
    pub fn parse(data: &[u8]) -> Result<EngineArchive> {
        if !is_engine_archive(data) {
            return invalid("not an InstallShield engine archive (_INST32I.EX_)");
        }
        let count = dword(data, COUNT)? as usize;
        if count > MAX_FILES {
            return invalid(format!("the engine archive lists {} files", count));
        }
        let mut at = ENTRIES;
        let mut files = Vec::with_capacity(count);
        let mut expanded = 0u64;
        for _ in 0..count {
            let offset = dword(data, at)? as usize;
            let stored = dword(data, at + 4)? as usize;
            let size = dword(data, at + 8)? as usize;
            at += ENTRY;
            let name = name(data, &mut at)?;
            let extract_as = name_of(data, &mut at)?;
            if offset
                .checked_add(stored)
                .is_none_or(|end| end > data.len())
            {
                return invalid(format!("{}'s data is outside the engine archive", name));
            }
            expanded += size as u64;
            if expanded > MAX_EXPANDED {
                return invalid("the engine archive's files expand to more than 64 MB");
            }
            files.push(EngineFile {
                name,
                extract_as,
                offset,
                stored,
                size,
            });
        }
        Ok(EngineArchive { files })
    }

    /// The archive's files, in its order.
    pub fn files(&self) -> &[EngineFile] {
        &self.files
    }

    /// The expanded data of `file`, from the archive's bytes `data`.
    pub fn read(&self, data: &[u8], file: &EngineFile) -> Result<Vec<u8>> {
        let stored = data
            .get(file.offset..file.offset + file.stored)
            .ok_or_else(|| Error::Invalid(format!("{} is cut short", file.name)))?;
        let out = crate::explode(stored, file.size)?;
        if out.len() != file.size {
            return invalid(format!(
                "{} expands to {} bytes, not the {} its entry gives",
                file.name,
                out.len(),
                file.size
            ));
        }
        Ok(out)
    }
}

/// Whether `data` starts as an engine archive does.
pub fn is_engine_archive(data: &[u8]) -> bool {
    data.get(..4).is_some_and(|s| s == SIGNATURE.to_le_bytes())
}

fn dword(data: &[u8], at: usize) -> Result<u32> {
    match data.get(at..at + 4) {
        Some(b) => Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
        None => invalid("the engine archive's table is cut short"),
    }
}

/// A name of an entry: a 16-bit length and the bytes.
fn name(data: &[u8], at: &mut usize) -> Result<String> {
    let len = match data.get(*at..*at + 2) {
        Some(b) => u16::from_le_bytes([b[0], b[1]]) as usize,
        None => return invalid("the engine archive's table is cut short"),
    };
    let bytes = data
        .get(*at + 2..*at + 2 + len)
        .ok_or_else(|| Error::Invalid("the engine archive's table is cut short".into()))?;
    *at += 2 + len;
    Ok(latin1(bytes))
}

/// The name a file is extracted as, which must be a plain file name: no
/// folder, drive or device, and none of the characters Windows refuses.
fn name_of(data: &[u8], at: &mut usize) -> Result<String> {
    let name = name(data, at)?;
    match crate::relative_path(&name) {
        Ok(path) if !path.contains('/') => Ok(name),
        _ => invalid(format!("the engine archive names a file {:?}", name)),
    }
}
