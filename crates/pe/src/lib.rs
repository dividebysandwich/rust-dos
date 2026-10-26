//! Portable Executable files, the EXEs and DLLs of 32-bit Windows: their
//! headers and sections, and what a loader needs from the image they make
//! in memory (imports, exports, base relocations, the TLS directory and
//! resources). Only 32-bit images for the 386 (PE32) are read.
//!
//! `Image::load` maps a file's sections at their relative virtual
//! addresses (RVAs), as the loader puts them in memory; everything after
//! that is read from the mapped image. `Builder` writes images, for the
//! emulator's own DLLs and for tests.

mod builder;

pub use builder::{Builder, Imp};

/// Why a file can't be loaded as an image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// No MZ header, or no PE header where it points.
    NotPe,
    /// A PE file of another processor (the machine field), or a PE32+
    /// (64-bit) one.
    Unsupported(String),
    /// A header, table or string runs past the end of the file or image.
    Truncated(&'static str),
    /// A value no valid image has.
    Invalid(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotPe => write!(f, "not a Portable Executable file"),
            Error::Unsupported(what) => write!(f, "unsupported image: {}", what),
            Error::Truncated(what) => write!(f, "the {} runs past the end of the image", what),
            Error::Invalid(what) => write!(f, "invalid image: {}", what),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// The machine field of a 386 image.
pub const MACHINE_I386: u16 = 0x014C;
/// File characteristics.
pub const FILE_RELOCS_STRIPPED: u16 = 0x0001;
pub const FILE_EXECUTABLE: u16 = 0x0002;
pub const FILE_32BIT: u16 = 0x0100;
pub const FILE_DLL: u16 = 0x2000;
/// Subsystems.
pub const SUBSYSTEM_GUI: u16 = 2;
pub const SUBSYSTEM_CONSOLE: u16 = 3;
/// Section characteristics.
pub const SCN_CODE: u32 = 0x0000_0020;
pub const SCN_INITIALIZED_DATA: u32 = 0x0000_0040;
pub const SCN_UNINITIALIZED_DATA: u32 = 0x0000_0080;
pub const SCN_EXECUTE: u32 = 0x2000_0000;
pub const SCN_READ: u32 = 0x4000_0000;
pub const SCN_WRITE: u32 = 0x8000_0000;
/// The data directories' indexes.
pub const DIR_EXPORT: usize = 0;
pub const DIR_IMPORT: usize = 1;
pub const DIR_RESOURCE: usize = 2;
pub const DIR_BASERELOC: usize = 5;
pub const DIR_TLS: usize = 9;
pub const DIR_IAT: usize = 12;

/// The magic of a PE32 optional header.
const PE32_MAGIC: u16 = 0x010B;
const PE32_PLUS_MAGIC: u16 = 0x020B;

/// Whether `file` starts with an MZ header whose `e_lfanew` points at a
/// PE signature: a Win32 program or DLL, rather than a DOS one.
pub fn is_pe(file: &[u8]) -> bool {
    pe_offset(file).is_some()
}

/// Where the PE signature is, if `file` has one.
fn pe_offset(file: &[u8]) -> Option<usize> {
    if !file.starts_with(b"MZ") {
        return None;
    }
    let at = u32_at(file, 0x3C)? as usize;
    (file.get(at..at.checked_add(4)?)? == b"PE\0\0").then_some(at)
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

/// A data directory: where a table is in the image, and how long it is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DataDir {
    pub rva: u32,
    pub size: u32,
}

/// A section's header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub name: String,
    pub rva: u32,
    pub virtual_size: u32,
    pub raw_offset: u32,
    pub raw_size: u32,
    pub characteristics: u32,
}

impl Section {
    /// How much of the image it takes: its virtual size, or for a linker
    /// that left that 0, its raw size.
    pub fn mapped_size(&self) -> u32 {
        if self.virtual_size != 0 {
            self.virtual_size
        } else {
            self.raw_size
        }
    }

    pub fn writable(&self) -> bool {
        self.characteristics & SCN_WRITE != 0
    }
}

/// The headers: the COFF file header's and the optional header's fields a
/// loader uses, the data directories and the section table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Headers {
    pub machine: u16,
    pub characteristics: u16,
    pub image_base: u32,
    /// The entry point's RVA (0: none, as in many DLLs).
    pub entry: u32,
    pub section_alignment: u32,
    pub file_alignment: u32,
    pub size_of_image: u32,
    pub size_of_headers: u32,
    pub subsystem: u16,
    pub major_subsystem: u16,
    pub minor_subsystem: u16,
    pub dll_characteristics: u16,
    pub stack_reserve: u32,
    pub stack_commit: u32,
    pub heap_reserve: u32,
    pub heap_commit: u32,
    pub dirs: [DataDir; 16],
    pub sections: Vec<Section>,
}

/// Images larger than this aren't loaded (Windows 95's private arena is
/// 2 GB, but no program of its time comes near 256 MB).
const MAX_IMAGE: u32 = 0x1000_0000;

impl Headers {
    /// Read the headers of the PE file `file`.
    pub fn parse(file: &[u8]) -> Result<Headers> {
        let pe = pe_offset(file).ok_or(Error::NotPe)?;
        let coff = pe + 4;
        let r16 = |at: usize, what| u16_at(file, at).ok_or(Error::Truncated(what));
        let r32 = |at: usize, what| u32_at(file, at).ok_or(Error::Truncated(what));
        let machine = r16(coff, "file header")?;
        if machine != MACHINE_I386 {
            return Err(Error::Unsupported(format!(
                "machine {:04X}h, not a 386",
                machine
            )));
        }
        let sections = r16(coff + 2, "file header")? as usize;
        let optional_size = r16(coff + 16, "file header")? as usize;
        let characteristics = r16(coff + 18, "file header")?;
        let opt = coff + 20;
        match r16(opt, "optional header")? {
            PE32_MAGIC => {}
            PE32_PLUS_MAGIC => return Err(Error::Unsupported("a 64-bit (PE32+) image".into())),
            magic => {
                return Err(Error::Invalid(format!(
                    "optional header magic {:04X}h",
                    magic
                )));
            }
        }
        if optional_size < 96 {
            return Err(Error::Truncated("optional header"));
        }
        let field = |offset: usize| r32(opt + offset, "optional header");
        let field16 = |offset: usize| r16(opt + offset, "optional header");
        let count = (field(92)? as usize).min(16).min((optional_size - 96) / 8);
        let mut dirs = [DataDir::default(); 16];
        for (i, dir) in dirs.iter_mut().enumerate().take(count) {
            *dir = DataDir {
                rva: field(96 + 8 * i)?,
                size: field(100 + 8 * i)?,
            };
        }
        let table = opt + optional_size;
        let sections = (0..sections)
            .map(|i| {
                let at = table + 40 * i;
                let header = file
                    .get(at..at + 40)
                    .ok_or(Error::Truncated("section table"))?;
                let name = &header[..8];
                let name = &name[..name.iter().position(|&b| b == 0).unwrap_or(8)];
                let word = |offset| u32_at(header, offset).unwrap();
                Ok(Section {
                    name: String::from_utf8_lossy(name).into_owned(),
                    virtual_size: word(8),
                    rva: word(12),
                    raw_size: word(16),
                    raw_offset: word(20),
                    characteristics: word(36),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let headers = Headers {
            machine,
            characteristics,
            entry: field(16)?,
            image_base: field(28)?,
            section_alignment: field(32)?,
            file_alignment: field(36)?,
            major_subsystem: field16(48)?,
            minor_subsystem: field16(50)?,
            size_of_image: field(56)?,
            size_of_headers: field(60)?,
            subsystem: field16(68)?,
            dll_characteristics: field16(70)?,
            stack_reserve: field(72)?,
            stack_commit: field(76)?,
            heap_reserve: field(80)?,
            heap_commit: field(84)?,
            dirs,
            sections,
        };
        headers.check()?;
        Ok(headers)
    }

    fn check(&self) -> Result<()> {
        if self.size_of_image == 0 || self.size_of_image > MAX_IMAGE {
            return Err(Error::Invalid(format!(
                "image size {:X}h",
                self.size_of_image
            )));
        }
        if self.image_base & 0xFFFF != 0 {
            return Err(Error::Invalid(format!(
                "image base {:08X}h not 64 KB aligned",
                self.image_base
            )));
        }
        for section in &self.sections {
            let end = section.rva as u64 + section.mapped_size() as u64;
            if end > self.size_of_image as u64 {
                return Err(Error::Invalid(format!(
                    "section {} ends past the image",
                    section.name
                )));
            }
        }
        Ok(())
    }

    pub fn is_dll(&self) -> bool {
        self.characteristics & FILE_DLL != 0
    }

    /// Whether the image has base relocations, so it can load somewhere
    /// other than its image base.
    pub fn relocatable(&self) -> bool {
        self.characteristics & FILE_RELOCS_STRIPPED == 0 && self.dirs[DIR_BASERELOC].size != 0
    }

    /// The section the RVA `rva` is in.
    pub fn section_at(&self, rva: u32) -> Option<&Section> {
        self.sections
            .iter()
            .find(|s| rva >= s.rva && rva - s.rva < s.mapped_size().max(1))
    }
}

/// An imported function: by name (with the hint, an index into the DLL's
/// export names to try first) or by ordinal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Import {
    Name { hint: u16, name: String },
    Ordinal(u16),
}

/// The functions imported from one DLL, each with the RVA of the import
/// address table's slot the loader writes its address to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportedDll {
    pub name: String,
    pub functions: Vec<(u32, Import)>,
}

/// An exported function or variable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportTarget {
    /// In the image, at this RVA.
    Rva(u32),
    /// In another DLL: "DLL.Function" or "DLL.#ordinal".
    Forwarder(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Export {
    pub ordinal: u16,
    pub name: Option<String>,
    pub target: ExportTarget,
}

/// The export directory: the DLL's own name for itself, and what it
/// exports.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Exports {
    pub name: String,
    pub ordinal_base: u32,
    pub exports: Vec<Export>,
}

impl Exports {
    /// The export named `name`.
    pub fn by_name(&self, name: &str) -> Option<&Export> {
        self.exports
            .iter()
            .find(|e| e.name.as_deref() == Some(name))
    }

    pub fn by_ordinal(&self, ordinal: u16) -> Option<&Export> {
        self.exports.iter().find(|e| e.ordinal == ordinal)
    }
}

/// A base relocation: the RVA it changes, and its type (`REL_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Relocation {
    pub rva: u32,
    pub kind: u8,
}

/// Base relocation types: padding, the high or low 16 bits of an address,
/// and a whole 32-bit address (the one 32-bit linkers write).
pub const REL_ABSOLUTE: u8 = 0;
pub const REL_HIGH: u8 = 1;
pub const REL_LOW: u8 = 2;
pub const REL_HIGHLOW: u8 = 3;

/// The TLS directory: the template of the thread-local data (virtual
/// addresses, as the image has them at its image base), the variable the
/// loader writes the TLS index to, and the list of callbacks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tls {
    pub start: u32,
    pub end: u32,
    pub index_address: u32,
    pub callbacks_address: u32,
    pub zero_fill: u32,
    /// The callbacks' addresses, read from the list.
    pub callbacks: Vec<u32>,
}

/// A resource's type, name or language: a number, or a name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResId {
    Id(u16),
    Name(String),
}

/// A resource: where its data is (an RVA) and how long it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resource {
    pub kind: ResId,
    pub name: ResId,
    pub language: ResId,
    pub rva: u32,
    pub size: u32,
    pub code_page: u32,
}

/// An image as the loader maps it: `data` is `size_of_image` bytes, the
/// headers and each section at its RVA.
#[derive(Clone, Debug)]
pub struct Image {
    pub headers: Headers,
    pub data: Vec<u8>,
}

impl Image {
    /// Map the PE file `file`.
    pub fn load(file: &[u8]) -> Result<Image> {
        let headers = Headers::parse(file)?;
        let mut data = vec![0u8; headers.size_of_image as usize];
        let head = (headers.size_of_headers as usize)
            .min(file.len())
            .min(data.len());
        data[..head].copy_from_slice(&file[..head]);
        for section in &headers.sections {
            // Raw data past the end of the file is cut short, as Windows'
            // loader does with a last section's padding. A section whose
            // data is at offset 0 has none in the file (Watcom's .bss): it
            // stays zeros. Raw data past the virtual size is mapped up to
            // the next multiple of the section alignment, as in Windows:
            // patches put code there (Zanzarah's Russian release). That
            // padding stops at the next section, so a header with a wrong
            // alignment doesn't overwrite the start of that section.
            let next = headers
                .sections
                .iter()
                .map(|s| s.rva)
                .filter(|&rva| rva > section.rva)
                .min()
                .unwrap_or(u32::MAX);
            let pages = section
                .mapped_size()
                .checked_next_multiple_of(headers.section_alignment.max(1))
                .unwrap_or(u32::MAX)
                .min(next - section.rva)
                .max(section.mapped_size());
            let len = section.raw_size.min(pages) as usize;
            let from = section.raw_offset as usize;
            let to = section.rva as usize;
            if len == 0 || from == 0 || from >= file.len() || to >= data.len() {
                continue;
            }
            let len = len.min(file.len() - from).min(data.len() - to);
            data[to..to + len].copy_from_slice(&file[from..from + len]);
        }
        Ok(Image { headers, data })
    }

    pub fn u16_at(&self, rva: u32) -> Result<u16> {
        u16_at(&self.data, rva as usize).ok_or(Error::Truncated("table"))
    }

    pub fn u32_at(&self, rva: u32) -> Result<u32> {
        u32_at(&self.data, rva as usize).ok_or(Error::Truncated("table"))
    }

    pub fn set_u32(&mut self, rva: u32, value: u32) -> Result<()> {
        let at = rva as usize;
        let slot = self
            .data
            .get_mut(at..at + 4)
            .ok_or(Error::Truncated("table"))?;
        slot.copy_from_slice(&value.to_le_bytes());
        Ok(())
    }

    /// The NUL-terminated string at `rva`.
    pub fn cstr(&self, rva: u32) -> Result<String> {
        let tail = self
            .data
            .get(rva as usize..)
            .ok_or(Error::Truncated("string"))?;
        let len = tail
            .iter()
            .position(|&b| b == 0)
            .ok_or(Error::Truncated("string"))?;
        Ok(String::from_utf8_lossy(&tail[..len]).into_owned())
    }

    /// The RVA of the virtual address `va` at the image base.
    fn rva_of(&self, va: u32) -> Result<u32> {
        let rva = va.wrapping_sub(self.headers.image_base);
        if rva >= self.headers.size_of_image {
            return Err(Error::Invalid(format!(
                "address {:08X}h outside the image",
                va
            )));
        }
        Ok(rva)
    }

    /// Data directory `index`, if there is one. It must start in the image,
    /// so that the fields read from it can't run past 4 GB.
    fn dir(&self, index: usize) -> Result<Option<DataDir>> {
        let dir = self.headers.dirs[index];
        if dir.rva == 0 || dir.size == 0 {
            return Ok(None);
        }
        if dir.rva >= self.headers.size_of_image {
            return Err(Error::Truncated("data directory"));
        }
        Ok(Some(dir))
    }

    /// The DLLs and functions the image imports.
    pub fn imports(&self) -> Result<Vec<ImportedDll>> {
        let Some(dir) = self.dir(DIR_IMPORT)? else {
            return Ok(Vec::new());
        };
        let mut dlls = Vec::new();
        for i in 0.. {
            let at = dir.rva + 20 * i;
            let lookup = self.u32_at(at)?;
            let name = self.u32_at(at + 12)?;
            let iat = self.u32_at(at + 16)?;
            if name == 0 && iat == 0 {
                break;
            }
            // Old Borland linkers leave the lookup table out: the address
            // table has the names until it is bound.
            let lookup = if lookup != 0 { lookup } else { iat };
            let mut functions = Vec::new();
            for j in 0.. {
                let entry = self.u32_at(lookup + 4 * j)?;
                if entry == 0 {
                    break;
                }
                let import = if entry & 0x8000_0000 != 0 {
                    Import::Ordinal(entry as u16)
                } else {
                    Import::Name {
                        hint: self.u16_at(entry)?,
                        name: self.cstr(entry + 2)?,
                    }
                };
                functions.push((iat + 4 * j, import));
            }
            dlls.push(ImportedDll {
                name: self.cstr(name)?,
                functions,
            });
        }
        Ok(dlls)
    }

    /// What the image exports, if it has an export directory.
    pub fn exports(&self) -> Result<Option<Exports>> {
        let Some(dir) = self.dir(DIR_EXPORT)? else {
            return Ok(None);
        };
        let field = |offset| self.u32_at(dir.rva + offset);
        let name = field(12)?;
        let base = field(16)?;
        let functions = field(20)?;
        let names = field(24)?;
        let (function_table, name_table, ordinal_table) = (field(28)?, field(32)?, field(36)?);
        if functions > 0x10000 || names > 0x10000 {
            return Err(Error::Invalid(format!("{} exports", functions)));
        }
        let mut named = vec![None; functions as usize];
        for i in 0..names {
            let index = self.u16_at(ordinal_table + 2 * i)? as usize;
            let name = self.cstr(self.u32_at(name_table + 4 * i)?)?;
            if let Some(slot) = named.get_mut(index) {
                *slot = Some(name);
            }
        }
        let mut exports = Vec::new();
        for (i, name) in named.into_iter().enumerate() {
            let rva = self.u32_at(function_table + 4 * i as u32)?;
            if rva == 0 {
                continue;
            }
            let forwarded = rva >= dir.rva && rva - dir.rva < dir.size;
            let target = if forwarded {
                ExportTarget::Forwarder(self.cstr(rva)?)
            } else {
                ExportTarget::Rva(rva)
            };
            exports.push(Export {
                ordinal: base.wrapping_add(i as u32) as u16,
                name,
                target,
            });
        }
        let name = if name != 0 {
            self.cstr(name)?
        } else {
            String::new()
        };
        Ok(Some(Exports {
            name,
            ordinal_base: base,
            exports,
        }))
    }

    /// The base relocations, padding left out.
    pub fn relocations(&self) -> Result<Vec<Relocation>> {
        let Some(dir) = self.dir(DIR_BASERELOC)? else {
            return Ok(Vec::new());
        };
        let mut relocations = Vec::new();
        let mut at = dir.rva;
        let end = dir.rva.saturating_add(dir.size);
        while at.saturating_add(8) <= end {
            let page = self.u32_at(at)?;
            let size = self.u32_at(at + 4)?;
            if size < 8 {
                break;
            }
            for i in 0..(size - 8) / 2 {
                let entry = self.u16_at(at + 8 + 2 * i)?;
                let kind = (entry >> 12) as u8;
                if kind != REL_ABSOLUTE {
                    relocations.push(Relocation {
                        rva: page.wrapping_add((entry & 0xFFF) as u32),
                        kind,
                    });
                }
            }
            let Some(next) = at.checked_add(size) else {
                break;
            };
            at = next;
        }
        Ok(relocations)
    }

    /// Move the image to `base`: apply its relocations, and change the
    /// image base in the headers and here.
    pub fn relocate(&mut self, base: u32) -> Result<()> {
        let delta = base.wrapping_sub(self.headers.image_base);
        if delta == 0 {
            return Ok(());
        }
        for Relocation { rva, kind } in self.relocations()? {
            match kind {
                REL_HIGHLOW => {
                    let value = self.u32_at(rva)?;
                    self.set_u32(rva, value.wrapping_add(delta))?;
                }
                REL_HIGH | REL_LOW => {
                    let value = self.u16_at(rva)?;
                    let add = if kind == REL_HIGH {
                        (delta >> 16) as u16
                    } else {
                        delta as u16
                    };
                    let at = rva as usize;
                    self.data[at..at + 2].copy_from_slice(&value.wrapping_add(add).to_le_bytes());
                }
                _ => return Err(Error::Unsupported(format!("base relocation type {}", kind))),
            }
        }
        self.headers.image_base = base;
        if let Some(pe) = pe_offset(&self.data) {
            self.set_u32(pe as u32 + 24 + 28, base)?;
        }
        Ok(())
    }

    /// The TLS directory, if there is one.
    pub fn tls(&self) -> Result<Option<Tls>> {
        let Some(dir) = self.dir(DIR_TLS)? else {
            return Ok(None);
        };
        let field = |offset| self.u32_at(dir.rva + offset);
        let callbacks_address = field(12)?;
        let mut callbacks = Vec::new();
        if callbacks_address != 0 {
            let list = self.rva_of(callbacks_address)?;
            for i in 0.. {
                let callback = self.u32_at(list + 4 * i)?;
                if callback == 0 {
                    break;
                }
                callbacks.push(callback);
            }
        }
        Ok(Some(Tls {
            start: field(0)?,
            end: field(4)?,
            index_address: field(8)?,
            callbacks_address,
            zero_fill: field(16)?,
            callbacks,
        }))
    }

    /// The template of the TLS data, as `tls` says where it is: the bytes
    /// between its start and end, and the zero fill after them.
    pub fn tls_template(&self, tls: &Tls) -> Result<Vec<u8>> {
        let len = tls
            .end
            .checked_sub(tls.start)
            .ok_or(Error::Invalid("TLS template".into()))?;
        // The zero fill is a number in the file: no more than an image.
        if tls.zero_fill > MAX_IMAGE {
            return Err(Error::Invalid(format!(
                "TLS zero fill {:X}h",
                tls.zero_fill
            )));
        }
        let start = self.rva_of(tls.start)? as usize;
        let mut data = self
            .data
            .get(start..start + len as usize)
            .ok_or(Error::Truncated("TLS template"))?
            .to_vec();
        data.resize(data.len() + tls.zero_fill as usize, 0);
        Ok(data)
    }

    /// Every resource, by type, name and language.
    pub fn resources(&self) -> Result<Vec<Resource>> {
        let Some(dir) = self.dir(DIR_RESOURCE)? else {
            return Ok(Vec::new());
        };
        let subdir = |node: Node, what: &str| match node {
            Node::Dir(offset) => Ok(offset),
            Node::Leaf(_) => Err(Error::Invalid(format!(
                "resource {} without a directory",
                what
            ))),
        };
        let mut resources = Vec::new();
        for (kind, node) in self.resource_dir(dir.rva, 0)? {
            for (name, node) in self.resource_dir(dir.rva, subdir(node, "type")?)? {
                for (language, node) in self.resource_dir(dir.rva, subdir(node, "name")?)? {
                    let Node::Leaf(entry) = node else {
                        return Err(Error::Invalid("resource tree deeper than 3 levels".into()));
                    };
                    let entry = dir.rva + entry;
                    resources.push(Resource {
                        kind: kind.clone(),
                        name: name.clone(),
                        language,
                        rva: self.u32_at(entry)?,
                        size: self.u32_at(entry + 4)?,
                        code_page: self.u32_at(entry + 8)?,
                    });
                }
            }
        }
        Ok(resources)
    }

    /// The entries of the resource directory at `offset` in the resource
    /// section, which is at `base`.
    fn resource_dir(&self, base: u32, offset: u32) -> Result<Vec<(ResId, Node)>> {
        let at = base + offset;
        let count = self.u16_at(at + 12)? as u32 + self.u16_at(at + 14)? as u32;
        let mut entries = Vec::new();
        for i in 0..count {
            let entry = at + 16 + 8 * i;
            let name = self.u32_at(entry)?;
            let target = self.u32_at(entry + 4)?;
            let id = if name & 0x8000_0000 != 0 {
                let string = base + (name & 0x7FFF_FFFF);
                let len = self.u16_at(string)? as u32;
                let units = (0..len)
                    .map(|k| self.u16_at(string + 2 + 2 * k))
                    .collect::<Result<Vec<u16>>>()?;
                ResId::Name(String::from_utf16_lossy(&units))
            } else {
                ResId::Id(name as u16)
            };
            let node = if target & 0x8000_0000 != 0 {
                Node::Dir(target & 0x7FFF_FFFF)
            } else {
                Node::Leaf(target)
            };
            entries.push((id, node));
        }
        Ok(entries)
    }
}

/// What a resource directory's entry leads to: another directory, or a
/// data entry. Both are offsets in the resource section.
#[derive(Clone, Copy)]
enum Node {
    Dir(u32),
    Leaf(u32),
}

#[cfg(test)]
mod tests;
