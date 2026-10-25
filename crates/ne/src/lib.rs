//! New Executable files, the EXEs and DLLs of 16-bit Windows: the header,
//! the segment table and each segment's relocations, the entry table, the
//! resident and non-resident names, the module references and imported
//! names, and the resource table. Only Windows modules are of interest,
//! but OS/2 1.x files have the same layout and read the same way.
//!
//! `Module::parse` reads every table and checks it against the file, so
//! what a loader takes from a `Module` is in range. A segment's data and
//! relocations are read from the file on demand, as a loader loads
//! segments one at a time. `Builder` writes modules, for the emulator's
//! own 16-bit modules and for tests.

mod builder;

pub use builder::{Builder, SegmentSpec};

/// Why a file can't be read as a module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// No MZ header, or no NE header where it points.
    NotNe,
    /// A table or string runs past the end of the file.
    Truncated(&'static str),
    /// A value no valid module has.
    Invalid(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotNe => write!(f, "not a New Executable file"),
            Error::Truncated(what) => write!(f, "the {} runs past the end of the file", what),
            Error::Invalid(what) => write!(f, "invalid module: {}", what),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// The module flags (`Module::flags`).
pub const SINGLEDATA: u16 = 0x0001;
pub const MULTIPLEDATA: u16 = 0x0002;
/// Windows 3.x: the module needs protected mode.
pub const PROTMODE: u16 = 0x0008;
/// A program that loads its own segments (a self-loading application).
pub const SELFLOAD: u16 = 0x0800;
/// The linker found errors: the module mustn't be loaded.
pub const LINKERROR: u16 = 0x2000;
/// A DLL.
pub const LIBMODULE: u16 = 0x8000;

/// The segment flags (`Segment::flags`).
pub const SEG_DATA: u16 = 0x0001;
pub const SEG_MOVEABLE: u16 = 0x0010;
pub const SEG_SHAREABLE: u16 = 0x0020;
pub const SEG_PRELOAD: u16 = 0x0040;
/// Execute-only for code, read-only for data.
pub const SEG_READONLY: u16 = 0x0080;
pub const SEG_RELOCINFO: u16 = 0x0100;
pub const SEG_DISCARDABLE: u16 = 0x1000;
/// A 32-bit segment (OS/2 2.x linkers set it; Windows never has it).
pub const SEG_32BIT: u16 = 0x2000;

/// The target operating systems (`Module::target_os`).
pub const OS_UNKNOWN: u8 = 0;
pub const OS_OS2: u8 = 1;
pub const OS_WINDOWS: u8 = 2;

/// An entry point's flags: exported, and using the shared data segment.
pub const ENTRY_EXPORTED: u8 = 0x01;
pub const ENTRY_SHARED_DATA: u8 = 0x02;

/// The segment number of a constant entry in the entry table, and of a
/// moveable one.
const BUNDLE_CONSTANT: u8 = 0xFE;
const BUNDLE_MOVEABLE: u8 = 0xFF;

/// Relocations' address types: what they write at their offset.
pub const ADDR_LOBYTE: u8 = 0;
pub const ADDR_SELECTOR: u8 = 2;
pub const ADDR_FARPTR: u8 = 3;
pub const ADDR_OFFSET: u8 = 5;
pub const ADDR_FARPTR48: u8 = 11;
pub const ADDR_OFFSET32: u8 = 13;

/// Relocations' target types (3 is an OS fixup), and the flag of an
/// additive one.
const RELOC_INTERNAL: u8 = 0;
const RELOC_ORDINAL: u8 = 1;
const RELOC_NAME: u8 = 2;
const RELOC_ADDITIVE: u8 = 4;

/// Whether `file` starts with an MZ header whose `e_lfanew` points at an
/// NE signature: a 16-bit Windows (or OS/2) module.
pub fn is_ne(file: &[u8]) -> bool {
    ne_offset(file).is_some()
}

/// Where the NE header is, if `file` has one.
pub fn ne_offset(file: &[u8]) -> Option<usize> {
    if !file.starts_with(b"MZ") {
        return None;
    }
    let at = u32_at(file, 0x3C)? as usize;
    (file.get(at..at.checked_add(2)?)? == b"NE").then_some(at)
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(at..at.checked_add(2)?)?.try_into().ok()?,
    ))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

/// A string of the name tables: a length byte, then that many bytes.
fn pascal_at(bytes: &[u8], at: usize) -> Option<&[u8]> {
    let len = *bytes.get(at)? as usize;
    bytes.get(at + 1..at + 1 + len)
}

fn text(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

/// A segment of the segment table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    /// Where its data is in the file, in bytes; 0 for a segment with none
    /// (its memory is zeroed).
    pub offset: u32,
    /// How many bytes of data the file has.
    pub length: u32,
    pub flags: u16,
    /// How much memory it takes, at least its length.
    pub min_alloc: u32,
}

impl Segment {
    pub fn is_data(&self) -> bool {
        self.flags & SEG_DATA != 0
    }

    pub fn has_relocations(&self) -> bool {
        self.flags & SEG_RELOCINFO != 0 && self.offset != 0
    }

    /// The memory it takes: its minimum allocation or its data, whichever
    /// is larger, at most 64 KB.
    pub fn size(&self) -> u32 {
        self.min_alloc.max(self.length).min(0x1_0000)
    }

    /// Its data in `file`.
    pub fn data<'a>(&self, file: &'a [u8]) -> Result<&'a [u8]> {
        if self.offset == 0 {
            return Ok(&[]);
        }
        let start = self.offset as usize;
        file.get(start..start + self.length as usize)
            .ok_or(Error::Truncated("segment data"))
    }

    /// Its relocations in `file`, which follow its data.
    pub fn relocations(&self, file: &[u8]) -> Result<Vec<Relocation>> {
        if !self.has_relocations() {
            return Ok(Vec::new());
        }
        let at = self.offset as usize + self.length as usize;
        let count = u16_at(file, at).ok_or(Error::Truncated("relocation count"))? as usize;
        let records = file
            .get(at + 2..at + 2 + 8 * count)
            .ok_or(Error::Truncated("relocation records"))?;
        records
            .as_chunks::<8>()
            .0
            .iter()
            .map(|r| {
                let flags = r[1];
                let offset = u16::from_le_bytes([r[2], r[3]]);
                let a = u16::from_le_bytes([r[4], r[5]]);
                let b = u16::from_le_bytes([r[6], r[7]]);
                let target = match flags & 3 {
                    RELOC_INTERNAL => Target::Internal {
                        segment: r[4],
                        offset: b,
                    },
                    RELOC_ORDINAL => Target::Ordinal {
                        module: a,
                        ordinal: b,
                    },
                    RELOC_NAME => Target::Name {
                        module: a,
                        name_offset: b,
                    },
                    // RELOC_OSFIXUP.
                    _ => Target::OsFixup(a),
                };
                Ok(Relocation {
                    source: r[0],
                    additive: flags & RELOC_ADDITIVE != 0,
                    offset,
                    target,
                })
            })
            .collect()
    }
}

/// A relocation of a segment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Relocation {
    /// What it writes (`ADDR_` values).
    pub source: u8,
    /// Whether the target is added to what is at `offset`; otherwise what
    /// is there is the offset of the next place to write the target at,
    /// the chain ending with FFFFh.
    pub additive: bool,
    pub offset: u16,
    pub target: Target,
}

/// What a relocation points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// The module's own segment `segment` at `offset`; segment FFh is a
    /// moveable one, and `offset` the ordinal of its entry point.
    Internal { segment: u8, offset: u16 },
    /// Entry point `ordinal` of the module the module reference `module`
    /// (from 1) names.
    Ordinal { module: u16, ordinal: u16 },
    /// The entry point of that module named by the imported name at
    /// `name_offset` (`Module::imported_name`).
    Name { module: u16, name_offset: u16 },
    /// A fixup for the floating-point emulator, by its type.
    OsFixup(u16),
}

/// An entry point of the entry table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    /// An ordinal with nothing at it.
    Unused,
    /// Segment `segment` (from 1) at `offset`; `moveable` when the entry
    /// was in a moveable bundle.
    Code {
        segment: u8,
        offset: u16,
        flags: u8,
        moveable: bool,
    },
    /// A constant (an absolute value, such as __AHINCR).
    Constant { value: u16, flags: u8 },
}

/// A resource name or type: a number or a string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResId {
    Int(u16),
    Name(String),
}

impl ResId {
    /// Whether it is `other` as Windows compares them: numbers by value,
    /// names without regard to case.
    pub fn matches(&self, other: &ResId) -> bool {
        match (self, other) {
            (ResId::Int(a), ResId::Int(b)) => a == b,
            (ResId::Name(a), ResId::Name(b)) => a.eq_ignore_ascii_case(b),
            _ => false,
        }
    }
}

/// A resource of the resource table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resource {
    /// Where its data is in the file, and how many bytes it has.
    pub offset: u32,
    pub length: u32,
    pub flags: u16,
    pub id: ResId,
}

/// The resources of one type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceType {
    pub type_id: ResId,
    pub resources: Vec<Resource>,
}

/// A module's header and tables.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Module {
    /// Where the NE header is in the file.
    pub ne: usize,
    pub linker: (u8, u8),
    pub flags: u16,
    /// The automatic data segment's number (from 1), or 0.
    pub auto_data: u16,
    /// The local heap's and the stack's sizes, added to the automatic data
    /// segment.
    pub heap: u16,
    pub stack: u16,
    /// The entry point (segment number, offset) and the initial stack
    /// (segment number, offset; offset 0 for the end of the automatic data
    /// segment with the stack and heap added).
    pub cs_ip: (u16, u16),
    pub ss_sp: (u16, u16),
    pub target_os: u8,
    pub os_flags: u8,
    /// The version of Windows it was made for: major in the high byte.
    pub expected_version: u16,
    pub align_shift: u16,
    pub segments: Vec<Segment>,
    /// The module references: the names of the modules it imports from,
    /// numbered from 1 in relocations.
    pub module_refs: Vec<String>,
    /// The resident names, the module's name first (its ordinal 0), and
    /// the non-resident ones, its description first.
    pub resident_names: Vec<(String, u16)>,
    pub nonresident_names: Vec<(String, u16)>,
    /// The entry points, ordinal 1 first.
    pub entries: Vec<Entry>,
    pub resources: Vec<ResourceType>,
    /// The resource table's alignment shift, by which its offsets and
    /// lengths are scaled.
    pub resource_shift: u16,
    /// The imported names table's bytes.
    imported_names: Vec<u8>,
    /// The bytes of the header and the resident tables (from the NE header
    /// to the end of the entry table), for an in-memory copy.
    pub tables: Vec<u8>,
    /// Offsets in `tables` (from the NE header) of the segment table, the
    /// resource table, the resident names, the module references, the
    /// imported names and the entry table.
    pub table_offsets: [u16; 6],
}

/// The most segments, module references and entry points a module may
/// have: more than any table of a 64 KB header could hold.
const MAX_SEGMENTS: usize = 0x1000;

impl Module {
    /// Read the header and tables of the NE file `file`.
    pub fn parse(file: &[u8]) -> Result<Module> {
        let ne = ne_offset(file).ok_or(Error::NotNe)?;
        let r8 = |at: usize, what| file.get(ne + at).copied().ok_or(Error::Truncated(what));
        let r16 = |at: usize, what| u16_at(file, ne + at).ok_or(Error::Truncated(what));
        let r32 = |at: usize, what| u32_at(file, ne + at).ok_or(Error::Truncated(what));
        let linker = (r8(2, "header")?, r8(3, "header")?);
        let entry_off = r16(4, "header")? as usize;
        let entry_len = r16(6, "header")? as usize;
        let flags = r16(0x0C, "header")?;
        let auto_data = r16(0x0E, "header")?;
        let heap = r16(0x10, "header")?;
        let stack = r16(0x12, "header")?;
        let cs_ip = (r16(0x16, "header")?, r16(0x14, "header")?);
        let ss_sp = (r16(0x1A, "header")?, r16(0x18, "header")?);
        let seg_count = r16(0x1C, "header")? as usize;
        let mod_count = r16(0x1E, "header")? as usize;
        let nonres_len = r16(0x20, "header")? as usize;
        let seg_off = r16(0x22, "header")? as usize;
        let res_off = r16(0x24, "header")? as usize;
        let resname_off = r16(0x26, "header")? as usize;
        let modref_off = r16(0x28, "header")? as usize;
        let imp_off = r16(0x2A, "header")? as usize;
        let nonres_off = r32(0x2C, "header")? as usize;
        let align_shift = match r16(0x32, "header")? {
            0 => 9,
            s if s > 16 => return Err(Error::Invalid(format!("alignment shift {}", s))),
            s => s,
        };
        let target_os = r8(0x36, "header")?;
        let os_flags = r8(0x37, "header")?;
        let expected_version = r16(0x3E, "header")?;
        if seg_count > MAX_SEGMENTS || mod_count > MAX_SEGMENTS {
            return Err(Error::Invalid(
                "too many segments or module references".into(),
            ));
        }

        let mut segments = Vec::with_capacity(seg_count);
        for i in 0..seg_count {
            let at = seg_off + 8 * i;
            let sector = r16(at, "segment table")? as u32;
            let length = r16(at + 2, "segment table")? as u32;
            let flags = r16(at + 4, "segment table")?;
            let alloc = r16(at + 6, "segment table")? as u32;
            let offset = sector << align_shift;
            let length = if length == 0 && sector != 0 {
                0x1_0000
            } else {
                length
            };
            let min_alloc = if alloc == 0 { 0x1_0000 } else { alloc };
            if offset != 0 && file.len() < offset as usize + length as usize {
                return Err(Error::Truncated("segment data"));
            }
            segments.push(Segment {
                offset,
                length: if offset == 0 { 0 } else { length },
                flags,
                min_alloc,
            });
        }
        if auto_data as usize > segments.len() {
            return Err(Error::Invalid(format!(
                "automatic data segment {}",
                auto_data
            )));
        }

        // The imported names run from their table to the entry table, or
        // to the module references' table if that comes later.
        let imp_end = [entry_off, modref_off, resname_off, seg_off]
            .into_iter()
            .filter(|&o| o > imp_off)
            .min()
            .unwrap_or(entry_off.max(imp_off));
        let imported_names = file
            .get(ne + imp_off..ne + imp_end)
            .ok_or(Error::Truncated("imported names"))?
            .to_vec();
        let mut module_refs = Vec::with_capacity(mod_count);
        for i in 0..mod_count {
            let offset = r16(modref_off + 2 * i, "module references")? as usize;
            let name = pascal_at(&imported_names, offset)
                .ok_or(Error::Truncated("module reference name"))?;
            module_refs.push(text(name));
        }

        let resident_names = names(file, ne + resname_off, "resident names")?;
        let nonresident_names = if nonres_off == 0 || nonres_len == 0 {
            Vec::new()
        } else {
            let end = nonres_off + nonres_len;
            let table = file
                .get(nonres_off..end)
                .ok_or(Error::Truncated("non-resident names"))?;
            names(table, 0, "non-resident names")?
        };

        let entry_table = file
            .get(ne + entry_off..ne + entry_off + entry_len)
            .ok_or(Error::Truncated("entry table"))?;
        let entries = entries(entry_table)?;

        let (resources, resource_shift) = if res_off != 0 && res_off != resname_off {
            resources(file, ne + res_off, resname_off.saturating_sub(res_off))?
        } else {
            (Vec::new(), 0)
        };

        let table_end = [entry_off + entry_len, imp_end, resname_off, modref_off]
            .into_iter()
            .max()
            .unwrap_or(0);
        let tables = file
            .get(ne..ne + table_end)
            .ok_or(Error::Truncated("header"))?
            .to_vec();
        Ok(Module {
            ne,
            linker,
            flags,
            auto_data,
            heap,
            stack,
            cs_ip,
            ss_sp,
            target_os,
            os_flags,
            expected_version,
            align_shift,
            segments,
            module_refs,
            resident_names,
            nonresident_names,
            entries,
            resources,
            resource_shift,
            imported_names,
            tables,
            table_offsets: [
                seg_off as u16,
                res_off as u16,
                resname_off as u16,
                modref_off as u16,
                imp_off as u16,
                entry_off as u16,
            ],
        })
    }

    pub fn is_dll(&self) -> bool {
        self.flags & LIBMODULE != 0
    }

    /// The module's name: the first resident name.
    pub fn name(&self) -> &str {
        self.resident_names.first().map_or("", |(n, _)| n.as_str())
    }

    /// The entry point `ordinal`, if the module has it.
    pub fn entry(&self, ordinal: u16) -> Option<Entry> {
        let entry = *self.entries.get((ordinal as usize).checked_sub(1)?)?;
        (entry != Entry::Unused).then_some(entry)
    }

    /// The ordinal of the entry point named `name`, from the resident
    /// names or else the non-resident ones, compared without regard to
    /// case as Windows' GetProcAddress does.
    pub fn ordinal_of(&self, name: &str) -> Option<u16> {
        self.resident_names
            .iter()
            .skip(1)
            .chain(self.nonresident_names.iter().skip(1))
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|&(_, o)| o)
    }

    /// The imported name at `offset` of the imported names table.
    pub fn imported_name(&self, offset: u16) -> Option<String> {
        pascal_at(&self.imported_names, offset as usize).map(text)
    }

    /// The resource of type `type_id` named `id`.
    pub fn find_resource(&self, type_id: &ResId, id: &ResId) -> Option<&Resource> {
        self.resources
            .iter()
            .filter(|t| t.type_id.matches(type_id))
            .flat_map(|t| t.resources.iter())
            .find(|r| r.id.matches(id))
    }

    /// A resource's bytes in `file`.
    pub fn resource_data<'a>(&self, file: &'a [u8], res: &Resource) -> Result<&'a [u8]> {
        let start = res.offset as usize;
        file.get(start..start + res.length as usize)
            .ok_or(Error::Truncated("resource data"))
    }
}

/// A table of names: strings each followed by its ordinal, ending with an
/// empty string.
fn names(file: &[u8], mut at: usize, what: &'static str) -> Result<Vec<(String, u16)>> {
    let mut names = Vec::new();
    loop {
        let len = *file.get(at).ok_or(Error::Truncated(what))? as usize;
        if len == 0 {
            return Ok(names);
        }
        let name = file
            .get(at + 1..at + 1 + len)
            .ok_or(Error::Truncated(what))?;
        let ordinal = u16_at(file, at + 1 + len).ok_or(Error::Truncated(what))?;
        names.push((text(name), ordinal));
        at += len + 3;
    }
}

/// The entry table's bundles, as a list of entry points by ordinal.
fn entries(table: &[u8]) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    let mut at = 0;
    while at < table.len() {
        let count = table[at] as usize;
        if count == 0 {
            break;
        }
        let kind = *table.get(at + 1).ok_or(Error::Truncated("entry table"))?;
        at += 2;
        if entries.len() + count > 0xFFFF {
            return Err(Error::Invalid("too many entry points".into()));
        }
        match kind {
            0 => entries.extend(std::iter::repeat_n(Entry::Unused, count)),
            BUNDLE_MOVEABLE => {
                for _ in 0..count {
                    let e = table
                        .get(at..at + 6)
                        .ok_or(Error::Truncated("entry table"))?;
                    entries.push(Entry::Code {
                        segment: e[3],
                        offset: u16::from_le_bytes([e[4], e[5]]),
                        flags: e[0],
                        moveable: true,
                    });
                    at += 6;
                }
            }
            segment => {
                for _ in 0..count {
                    let e = table
                        .get(at..at + 3)
                        .ok_or(Error::Truncated("entry table"))?;
                    let value = u16::from_le_bytes([e[1], e[2]]);
                    entries.push(if segment == BUNDLE_CONSTANT {
                        Entry::Constant { value, flags: e[0] }
                    } else {
                        Entry::Code {
                            segment,
                            offset: value,
                            flags: e[0],
                            moveable: false,
                        }
                    });
                    at += 3;
                }
            }
        }
    }
    Ok(entries)
}

/// The resource table at `at`, which is at most `len` bytes long (0 for
/// as long as it takes), and its alignment shift.
fn resources(file: &[u8], at: usize, len: usize) -> Result<(Vec<ResourceType>, u16)> {
    let what = "resource table";
    let base = at;
    let shift = u16_at(file, at).ok_or(Error::Truncated(what))?;
    if shift > 16 {
        return Err(Error::Invalid(format!(
            "resource alignment shift {}",
            shift
        )));
    }
    let id = |raw: u16| -> Result<ResId> {
        if raw & 0x8000 != 0 {
            Ok(ResId::Int(raw & 0x7FFF))
        } else {
            let name = pascal_at(file, base + raw as usize).ok_or(Error::Truncated(what))?;
            Ok(ResId::Name(text(name)))
        }
    };
    let mut types = Vec::new();
    let mut p = at + 2;
    loop {
        if len != 0 && p >= base + len {
            break;
        }
        let type_raw = u16_at(file, p).ok_or(Error::Truncated(what))?;
        if type_raw == 0 {
            break;
        }
        let count = u16_at(file, p + 2).ok_or(Error::Truncated(what))? as usize;
        p += 8;
        let mut resources = Vec::with_capacity(count.min(0x1000));
        for _ in 0..count {
            let r = file.get(p..p + 12).ok_or(Error::Truncated(what))?;
            let word = |i: usize| u16::from_le_bytes([r[i], r[i + 1]]);
            resources.push(Resource {
                offset: (word(0) as u32) << shift,
                length: (word(2) as u32) << shift,
                flags: word(4),
                id: id(word(6))?,
            });
            p += 12;
        }
        types.push(ResourceType {
            type_id: id(type_raw)?,
            resources,
        });
    }
    Ok((types, shift))
}

#[cfg(test)]
mod tests;
