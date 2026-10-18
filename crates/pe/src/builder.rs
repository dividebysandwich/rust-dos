//! Writing images. The file's layout is the mapped image's: sections are
//! aligned to 4 KB both in the file and in memory, so a section's file
//! offset is its RVA and the file is the image as the loader maps it. The
//! emulator's own DLLs are built so, and so are the test programs.

use crate::*;
use std::collections::BTreeMap;

/// Both alignments of a built image.
const ALIGN: u32 = 0x1000;
/// Where the PE header goes, after the MZ header and the DOS stub.
const PE_OFFSET: usize = 0x80;

/// The DOS program at the start of the file: it says it can't run, and
/// ends with exit code 1, as linkers' default stubs do.
const DOS_STUB: [u8; 14] = [
    0x0E, // push cs
    0x1F, // pop ds
    0xBA, 0x0E, 0x00, // mov dx,000Eh (the message)
    0xB4, 0x09, // mov ah,09h
    0xCD, 0x21, // int 21h
    0xB8, 0x01, 0x4C, // mov ax,4C01h
    0xCD, 0x21, // int 21h
];
const DOS_MESSAGE: &[u8] = b"This program cannot be run in DOS mode.\r\r\n$";

/// A function to import: by name or by ordinal.
#[derive(Clone, Copy, Debug)]
pub enum Imp<'a> {
    Name(&'a str),
    Ordinal(u16),
}

struct Built {
    name: String,
    rva: u32,
    data: Vec<u8>,
    virtual_size: u32,
    characteristics: u32,
}

/// A PE image being written.
pub struct Builder {
    image_base: u32,
    /// A DLL's name, which its export directory has.
    dll: Option<String>,
    subsystem: u16,
    entry: u32,
    stack: (u32, u32),
    sections: Vec<Built>,
    dirs: [DataDir; 16],
    exports: Vec<(Option<String>, u16, u32)>,
    relocs: Vec<u32>,
    resources: BTreeMap<u16, BTreeMap<u16, BTreeMap<u16, Vec<u8>>>>,
}

fn align(n: u32) -> u32 {
    n.div_ceil(ALIGN) * ALIGN
}

fn put16(out: &mut [u8], at: usize, value: u16) {
    out[at..at + 2].copy_from_slice(&value.to_le_bytes());
}

fn put32(out: &mut [u8], at: usize, value: u32) {
    out[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

impl Builder {
    /// A program (a console one, unless `subsystem` says otherwise) at
    /// `image_base`.
    pub fn exe(image_base: u32) -> Builder {
        Builder {
            image_base,
            dll: None,
            subsystem: SUBSYSTEM_CONSOLE,
            entry: 0,
            stack: (0x10_0000, 0x1000),
            sections: Vec::new(),
            dirs: [DataDir::default(); 16],
            exports: Vec::new(),
            relocs: Vec::new(),
            resources: BTreeMap::new(),
        }
    }

    /// A DLL named `name` at `image_base`.
    pub fn dll(image_base: u32, name: &str) -> Builder {
        Builder {
            dll: Some(name.to_string()),
            subsystem: SUBSYSTEM_GUI,
            ..Builder::exe(image_base)
        }
    }

    pub fn subsystem(&mut self, subsystem: u16) -> &mut Self {
        self.subsystem = subsystem;
        self
    }

    /// The stack's reserve and commit sizes.
    pub fn stack(&mut self, reserve: u32, commit: u32) -> &mut Self {
        self.stack = (reserve, commit);
        self
    }

    pub fn image_base(&self) -> u32 {
        self.image_base
    }

    /// The RVA the next section gets.
    pub fn next_rva(&self) -> u32 {
        self.sections
            .last()
            .map_or(ALIGN, |s| s.rva + align(s.virtual_size.max(1)))
    }

    /// Add a section with `data`, as long in memory as in the file. Returns
    /// its RVA.
    pub fn section(&mut self, name: &str, data: &[u8], characteristics: u32) -> u32 {
        self.section_sized(name, data, data.len() as u32, characteristics)
    }

    /// Add a section `virtual_size` bytes long in memory, starting with
    /// `data` and zeros after it. Returns its RVA.
    pub fn section_sized(
        &mut self,
        name: &str,
        data: &[u8],
        virtual_size: u32,
        characteristics: u32,
    ) -> u32 {
        let rva = self.next_rva();
        self.sections.push(Built {
            name: name.to_string(),
            rva,
            data: data.to_vec(),
            virtual_size: virtual_size.max(data.len() as u32),
            characteristics,
        });
        rva
    }

    /// Import the functions of `dlls`, in an import section of their own.
    /// Returns the virtual addresses of their import address table slots,
    /// as the program's code calls through them.
    pub fn imports(&mut self, dlls: &[(&str, &[Imp])]) -> Vec<Vec<u32>> {
        let rva = self.next_rva();
        let slots: usize = dlls.iter().map(|(_, f)| f.len() + 1).sum();
        // The address tables first, all together; then the descriptors,
        // the lookup tables, and the names.
        let iat = 0usize;
        let descriptors = 4 * slots;
        let lookup = descriptors + 20 * (dlls.len() + 1);
        let mut out = vec![0u8; lookup + 4 * slots];
        let mut vas = Vec::new();
        let mut slot = 0usize;
        for (i, (dll, functions)) in dlls.iter().enumerate() {
            let first = slot;
            let mut dll_vas = Vec::new();
            for function in functions.iter() {
                let entry = match function {
                    Imp::Ordinal(ordinal) => 0x8000_0000 | *ordinal as u32,
                    Imp::Name(name) => {
                        // The hint (0: none), the name, and padding to
                        // keep the next entry aligned.
                        let at = out.len();
                        out.extend_from_slice(&[0, 0]);
                        out.extend_from_slice(name.as_bytes());
                        out.push(0);
                        if !out.len().is_multiple_of(2) {
                            out.push(0);
                        }
                        rva + at as u32
                    }
                };
                put32(&mut out, iat + 4 * slot, entry);
                put32(&mut out, lookup + 4 * slot, entry);
                dll_vas.push(self.image_base + rva + (iat + 4 * slot) as u32);
                slot += 1;
            }
            // The tables' terminating zero.
            slot += 1;
            let name = out.len();
            out.extend_from_slice(dll.as_bytes());
            out.push(0);
            let d = descriptors + 20 * i;
            put32(&mut out, d, rva + (lookup + 4 * first) as u32);
            put32(&mut out, d + 12, rva + name as u32);
            put32(&mut out, d + 16, rva + (iat + 4 * first) as u32);
            vas.push(dll_vas);
        }
        self.section(".idata", &out, SCN_INITIALIZED_DATA | SCN_READ | SCN_WRITE);
        self.dirs[DIR_IMPORT] = DataDir {
            rva: rva + descriptors as u32,
            size: 20 * (dlls.len() as u32 + 1),
        };
        self.dirs[DIR_IAT] = DataDir {
            rva,
            size: 4 * slots as u32,
        };
        vas
    }

    /// Export the function or variable at `rva` as `ordinal`, named
    /// `name` unless it is exported by ordinal only.
    pub fn export(&mut self, name: Option<&str>, ordinal: u16, rva: u32) -> &mut Self {
        self.exports.push((name.map(str::to_string), ordinal, rva));
        self
    }

    /// A base relocation for the 32-bit address at `rva`.
    pub fn reloc(&mut self, rva: u32) -> &mut Self {
        self.relocs.push(rva);
        self
    }

    /// A resource with numeric type, name and language.
    pub fn resource(&mut self, kind: u16, name: u16, language: u16, data: &[u8]) -> &mut Self {
        self.resources
            .entry(kind)
            .or_default()
            .entry(name)
            .or_default()
            .insert(language, data.to_vec());
        self
    }

    /// Set a data directory (for a table a section of the caller's has).
    pub fn directory(&mut self, index: usize, rva: u32, size: u32) -> &mut Self {
        self.dirs[index] = DataDir { rva, size };
        self
    }

    pub fn entry(&mut self, rva: u32) -> &mut Self {
        self.entry = rva;
        self
    }

    /// The image's file (and mapped image).
    pub fn build(mut self) -> Vec<u8> {
        self.export_section();
        self.resource_section();
        self.reloc_section();
        let size_of_image = self.next_rva();
        let mut out = vec![0u8; size_of_image as usize];
        self.write_headers(&mut out, size_of_image);
        for section in &self.sections {
            let at = section.rva as usize;
            out[at..at + section.data.len()].copy_from_slice(&section.data);
        }
        // The file ends with the last section's data.
        let end = self.sections.last().map_or(ALIGN as usize, |s| {
            (s.rva + align(s.data.len() as u32)) as usize
        });
        out.truncate(end.max(ALIGN as usize));
        out
    }

    fn export_section(&mut self) {
        let Some(dll) = self.dll.clone() else { return };
        if self.exports.is_empty() {
            return;
        }
        let rva = self.next_rva();
        let base = self.exports.iter().map(|e| e.1).min().unwrap() as u32;
        let last = self.exports.iter().map(|e| e.1).max().unwrap() as u32;
        let functions = (last - base + 1) as usize;
        let mut named: Vec<(&str, u16)> = self
            .exports
            .iter()
            .filter_map(|(n, o, _)| n.as_deref().map(|n| (n, *o)))
            .collect();
        named.sort();
        let table = 40usize;
        let name_table = table + 4 * functions;
        let ordinal_table = name_table + 4 * named.len();
        let mut out = vec![0u8; ordinal_table + 2 * named.len()];
        for &(_, ordinal, target) in &self.exports {
            put32(
                &mut out,
                table + 4 * (ordinal as u32 - base) as usize,
                target,
            );
        }
        let dll_name = out.len();
        out.extend_from_slice(dll.as_bytes());
        out.push(0);
        for (i, (name, ordinal)) in named.iter().enumerate() {
            let at = out.len();
            out.extend_from_slice(name.as_bytes());
            out.push(0);
            put32(&mut out, name_table + 4 * i, rva + at as u32);
            put16(
                &mut out,
                ordinal_table + 2 * i,
                (*ordinal as u32 - base) as u16,
            );
        }
        put32(&mut out, 12, rva + dll_name as u32);
        put32(&mut out, 16, base);
        put32(&mut out, 20, functions as u32);
        put32(&mut out, 24, named.len() as u32);
        put32(&mut out, 28, rva + table as u32);
        put32(&mut out, 32, rva + name_table as u32);
        put32(&mut out, 36, rva + ordinal_table as u32);
        let size = out.len() as u32;
        self.section(".edata", &out, SCN_INITIALIZED_DATA | SCN_READ);
        self.dirs[DIR_EXPORT] = DataDir { rva, size };
    }

    fn resource_section(&mut self) {
        if self.resources.is_empty() {
            return;
        }
        let rva = self.next_rva();
        let dir_size = |entries: usize| 16 + 8 * entries;
        // Where each directory goes: the root, the types' directories, the
        // names' directories; then the data entries and the data.
        let mut at = dir_size(self.resources.len());
        let mut type_dirs = Vec::new();
        for names in self.resources.values() {
            type_dirs.push(at);
            at += dir_size(names.len());
        }
        let mut name_dirs = Vec::new();
        for names in self.resources.values() {
            for languages in names.values() {
                name_dirs.push(at);
                at += dir_size(languages.len());
            }
        }
        let leaves: usize = self
            .resources
            .values()
            .flat_map(|n| n.values())
            .map(|l| l.len())
            .sum();
        let mut data_entry = at;
        let mut data_at = at + 16 * leaves;
        let mut out = vec![0u8; data_at];
        let directory = |out: &mut Vec<u8>, at: usize, ids: Vec<u16>, targets: Vec<u32>| {
            put16(out, at + 14, ids.len() as u16);
            for (i, (id, target)) in ids.iter().zip(targets).enumerate() {
                put32(out, at + 16 + 8 * i, *id as u32);
                put32(out, at + 20 + 8 * i, target);
            }
        };
        let kinds: Vec<u16> = self.resources.keys().copied().collect();
        directory(
            &mut out,
            0,
            kinds,
            type_dirs.iter().map(|&d| 0x8000_0000 | d as u32).collect(),
        );
        let mut name_dir = name_dirs.iter();
        for (names, &type_dir) in self.resources.values().zip(&type_dirs) {
            let dirs: Vec<usize> = (0..names.len())
                .map(|_| *name_dir.next().unwrap())
                .collect();
            let ids = names.keys().copied().collect();
            directory(
                &mut out,
                type_dir,
                ids,
                dirs.iter().map(|&d| 0x8000_0000 | d as u32).collect(),
            );
            for (languages, &dir) in names.values().zip(&dirs) {
                let mut entries = Vec::new();
                for data in languages.values() {
                    entries.push(data_entry as u32);
                    out.resize(data_at, 0);
                    out.extend_from_slice(data);
                    put32(&mut out, data_entry, rva + data_at as u32);
                    put32(&mut out, data_entry + 4, data.len() as u32);
                    data_at = out.len().div_ceil(4) * 4;
                    data_entry += 16;
                }
                directory(&mut out, dir, languages.keys().copied().collect(), entries);
            }
        }
        let size = out.len() as u32;
        self.section(".rsrc", &out, SCN_INITIALIZED_DATA | SCN_READ);
        self.dirs[DIR_RESOURCE] = DataDir { rva, size };
    }

    fn reloc_section(&mut self) {
        if self.relocs.is_empty() {
            return;
        }
        let rva = self.next_rva();
        let mut relocs = std::mem::take(&mut self.relocs);
        relocs.sort();
        relocs.dedup();
        let mut out = Vec::new();
        let mut i = 0;
        while i < relocs.len() {
            let page = relocs[i] & !0xFFF;
            let start = out.len();
            out.extend_from_slice(&page.to_le_bytes());
            out.extend_from_slice(&[0; 4]);
            while i < relocs.len() && relocs[i] & !0xFFF == page {
                let entry = (REL_HIGHLOW as u16) << 12 | (relocs[i] & 0xFFF) as u16;
                out.extend_from_slice(&entry.to_le_bytes());
                i += 1;
            }
            if !(out.len() - start).is_multiple_of(4) {
                out.extend_from_slice(&[0, 0]);
            }
            let size = (out.len() - start) as u32;
            put32(&mut out, start + 4, size);
        }
        let size = out.len() as u32;
        self.section(".reloc", &out, SCN_INITIALIZED_DATA | SCN_READ);
        self.dirs[DIR_BASERELOC] = DataDir { rva, size };
        self.relocs = relocs;
    }

    fn write_headers(&self, out: &mut [u8], size_of_image: u32) {
        // The MZ header: 4 paragraphs of header, the stub after them, the
        // load module ending where the PE header starts.
        out[..2].copy_from_slice(b"MZ");
        put16(out, 0x02, PE_OFFSET as u16 % 512);
        put16(out, 0x04, PE_OFFSET.div_ceil(512) as u16);
        put16(out, 0x08, 4);
        put16(out, 0x0A, 0x10);
        put16(out, 0x0C, 0xFFFF);
        put16(out, 0x10, 0xB8);
        put16(out, 0x18, 0x40);
        put32(out, 0x3C, PE_OFFSET as u32);
        out[0x40..0x40 + DOS_STUB.len()].copy_from_slice(&DOS_STUB);
        out[0x4E..0x4E + DOS_MESSAGE.len()].copy_from_slice(DOS_MESSAGE);

        let pe = PE_OFFSET;
        out[pe..pe + 4].copy_from_slice(b"PE\0\0");
        let coff = pe + 4;
        let mut characteristics = FILE_EXECUTABLE | FILE_32BIT;
        if self.relocs.is_empty() {
            characteristics |= FILE_RELOCS_STRIPPED;
        }
        if self.dll.is_some() {
            characteristics |= FILE_DLL;
        }
        put16(out, coff, MACHINE_I386);
        put16(out, coff + 2, self.sections.len() as u16);
        put16(out, coff + 16, 0xE0);
        put16(out, coff + 18, characteristics);
        let opt = coff + 20;
        let code = |s: &&Built| s.characteristics & SCN_CODE != 0;
        let size_of = |f: &dyn Fn(&&Built) -> bool| {
            self.sections
                .iter()
                .filter(f)
                .map(|s| align(s.data.len() as u32))
                .sum::<u32>()
        };
        put16(out, opt, 0x010B);
        out[opt + 2] = 6;
        put32(out, opt + 4, size_of(&code));
        put32(out, opt + 8, size_of(&|s| !code(s)));
        put32(out, opt + 16, self.entry);
        put32(
            out,
            opt + 20,
            self.sections.iter().find(code).map_or(0, |s| s.rva),
        );
        put32(
            out,
            opt + 24,
            self.sections.iter().find(|s| !code(s)).map_or(0, |s| s.rva),
        );
        put32(out, opt + 28, self.image_base);
        put32(out, opt + 32, ALIGN);
        put32(out, opt + 36, ALIGN);
        put16(out, opt + 40, 4);
        put16(out, opt + 48, 4);
        put32(out, opt + 56, size_of_image);
        put32(out, opt + 60, ALIGN);
        put16(out, opt + 68, self.subsystem);
        put32(out, opt + 72, self.stack.0);
        put32(out, opt + 76, self.stack.1);
        put32(out, opt + 80, 0x10_0000);
        put32(out, opt + 84, 0x1000);
        put32(out, opt + 92, 16);
        for (i, dir) in self.dirs.iter().enumerate() {
            put32(out, opt + 96 + 8 * i, dir.rva);
            put32(out, opt + 100 + 8 * i, dir.size);
        }
        let table = opt + 0xE0;
        for (i, section) in self.sections.iter().enumerate() {
            let at = table + 40 * i;
            let name = section.name.as_bytes();
            out[at..at + name.len().min(8)].copy_from_slice(&name[..name.len().min(8)]);
            put32(out, at + 8, section.virtual_size);
            put32(out, at + 12, section.rva);
            put32(out, at + 16, align(section.data.len() as u32));
            put32(
                out,
                at + 20,
                if section.data.is_empty() {
                    0
                } else {
                    section.rva
                },
            );
            put32(out, at + 36, section.characteristics);
        }
    }
}
