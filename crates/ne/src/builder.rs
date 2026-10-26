//! Writing modules: the header and tables after a DOS stub, then each
//! segment's data and relocations, then the resources, all aligned to 16
//! bytes. The emulator's own 16-bit modules are built so, and so are the
//! test programs.

use crate::*;
use std::collections::BTreeMap;

/// Where the NE header goes, after the MZ header and the DOS stub.
const NE_OFFSET: usize = 0x80;
/// The alignment shift of segments and resources: 16 bytes.
const SHIFT: u16 = 4;
/// The NE header's own length.
const HEADER: usize = 0x40;

/// The DOS program at the start of the file: it says it needs Windows and
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
const DOS_MESSAGE: &[u8] = b"This program requires Microsoft Windows.\r\n$";

/// A segment to write: its data, flags (`SEG_` values), the memory it
/// takes (0 for its data's length) and its relocations.
#[derive(Clone, Debug, Default)]
pub struct SegmentSpec {
    pub data: Vec<u8>,
    pub flags: u16,
    pub alloc: u32,
    pub relocations: Vec<Relocation>,
}

/// A module being written.
pub struct Builder {
    name: String,
    description: String,
    flags: u16,
    auto_data: u16,
    heap: u16,
    stack: u16,
    cs_ip: (u16, u16),
    ss_sp: (u16, u16),
    expected_version: u16,
    segments: Vec<SegmentSpec>,
    module_refs: Vec<u16>,
    imported: Vec<u8>,
    resident: Vec<(String, u16)>,
    nonresident: Vec<(String, u16)>,
    entries: BTreeMap<u16, Entry>,
    resources: Vec<(ResId, ResId, u16, Vec<u8>)>,
}

impl Builder {
    /// A module named `name` with the module flags `flags` (`LIBMODULE`
    /// for a DLL, `SINGLEDATA` or `MULTIPLEDATA`), made for Windows 3.10.
    pub fn new(name: &str, flags: u16) -> Builder {
        Builder {
            name: name.to_ascii_uppercase(),
            description: name.to_string(),
            flags,
            auto_data: 0,
            heap: 0,
            stack: 0,
            cs_ip: (0, 0),
            ss_sp: (0, 0),
            expected_version: 0x030A,
            segments: Vec::new(),
            // The imported names table starts with an empty string.
            module_refs: Vec::new(),
            imported: vec![0],
            resident: Vec::new(),
            nonresident: Vec::new(),
            entries: BTreeMap::new(),
            resources: Vec::new(),
        }
    }

    /// Add a segment; its number, from 1.
    pub fn segment(&mut self, spec: SegmentSpec) -> u16 {
        self.segments.push(spec);
        self.segments.len() as u16
    }

    /// The segment numbered `number` (from 1), to add relocations to.
    pub fn segment_mut(&mut self, number: u16) -> &mut SegmentSpec {
        &mut self.segments[number as usize - 1]
    }

    /// The automatic data segment, and the local heap and stack added to
    /// it.
    pub fn auto_data(&mut self, segment: u16, heap: u16, stack: u16) {
        self.auto_data = segment;
        self.heap = heap;
        self.stack = stack;
    }

    /// The entry point, a program's or a DLL's LibMain.
    pub fn entry_point(&mut self, segment: u16, offset: u16) {
        self.cs_ip = (segment, offset);
    }

    /// The initial stack (offset 0: the end of the automatic data segment
    /// with the stack added).
    pub fn initial_stack(&mut self, segment: u16, offset: u16) {
        self.ss_sp = (segment, offset);
    }

    pub fn expected_version(&mut self, version: u16) {
        self.expected_version = version;
    }

    pub fn description(&mut self, text: &str) {
        self.description = text.to_string();
    }

    /// The module reference of `name`: its number, from 1, for
    /// relocations.
    pub fn import_module(&mut self, name: &str) -> u16 {
        let offset = self.imported_name(name);
        if let Some(i) = self.module_refs.iter().position(|&o| o == offset) {
            return i as u16 + 1;
        }
        self.module_refs.push(offset);
        self.module_refs.len() as u16
    }

    /// The offset of `name` in the imported names table, for a relocation
    /// that imports by name.
    pub fn imported_name(&mut self, name: &str) -> u16 {
        let mut at = 1;
        while at < self.imported.len() {
            let len = self.imported[at] as usize;
            if self.imported[at + 1..at + 1 + len] == *name.as_bytes() {
                return at as u16;
            }
            at += 1 + len;
        }
        let offset = self.imported.len() as u16;
        self.imported.push(name.len() as u8);
        self.imported.extend_from_slice(name.as_bytes());
        offset
    }

    /// Put `entry` at `ordinal` of the entry table.
    pub fn entry(&mut self, ordinal: u16, entry: Entry) {
        self.entries.insert(ordinal, entry);
    }

    /// Export ordinal `ordinal` by `name`, in the resident names or the
    /// non-resident ones.
    pub fn export(&mut self, name: &str, ordinal: u16, resident: bool) {
        let names = if resident {
            &mut self.resident
        } else {
            &mut self.nonresident
        };
        names.push((name.to_string(), ordinal));
    }

    /// Add a resource of type `type_id` named `id`.
    pub fn resource(&mut self, type_id: ResId, id: ResId, flags: u16, data: &[u8]) {
        self.resources.push((type_id, id, flags, data.to_vec()));
    }

    /// The entry table's bytes.
    fn entry_table(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut next = 1u16;
        let list: Vec<(u16, Entry)> = self.entries.iter().map(|(&o, &e)| (o, e)).collect();
        let mut i = 0;
        while i < list.len() {
            let (ordinal, entry) = list[i];
            // Unused ordinals before it.
            let mut gap = ordinal - next;
            while gap > 0 {
                let n = gap.min(255);
                out.extend_from_slice(&[n as u8, 0]);
                gap -= n;
            }
            // A bundle of entries of the same kind at ordinals in a row.
            let kind = |e: &Entry| match *e {
                Entry::Unused => 0u8,
                Entry::Code { moveable: true, .. } => BUNDLE_MOVEABLE,
                Entry::Code { segment, .. } => segment,
                Entry::Constant { .. } => BUNDLE_CONSTANT,
            };
            let k = kind(&entry);
            let mut j = i + 1;
            while j < list.len()
                && j - i < 255
                && list[j].0 == list[j - 1].0 + 1
                && kind(&list[j].1) == k
            {
                j += 1;
            }
            out.extend_from_slice(&[(j - i) as u8, k]);
            for &(_, e) in &list[i..j] {
                match e {
                    Entry::Code {
                        segment,
                        offset,
                        flags,
                        moveable: true,
                    } => {
                        out.extend_from_slice(&[flags, 0xCD, 0x3F, segment]);
                        out.extend_from_slice(&offset.to_le_bytes());
                    }
                    Entry::Code { offset, flags, .. } => {
                        out.push(flags);
                        out.extend_from_slice(&offset.to_le_bytes());
                    }
                    Entry::Constant { value, flags } => {
                        out.push(flags);
                        out.extend_from_slice(&value.to_le_bytes());
                    }
                    Entry::Unused => out.extend_from_slice(&[0, 0, 0]),
                }
            }
            next = list[j - 1].0 + 1;
            i = j;
        }
        out.push(0);
        out
    }

    /// The resource table's bytes, the offsets of its resources' places
    /// (where their sector offsets go), and the data each goes with.
    fn resource_table(&self) -> (Vec<u8>, Vec<usize>) {
        if self.resources.is_empty() {
            return (Vec::new(), Vec::new());
        }
        let mut types: Vec<(ResId, Vec<usize>)> = Vec::new();
        for (i, (t, ..)) in self.resources.iter().enumerate() {
            match types.iter_mut().find(|(id, _)| id == t) {
                Some((_, list)) => list.push(i),
                None => types.push((t.clone(), vec![i])),
            }
        }
        // The names come after the types' table.
        let table_len: usize = 2 + types.iter().map(|(_, l)| 8 + 12 * l.len()).sum::<usize>() + 2;
        let mut names = Vec::new();
        let name_offset = |id: &ResId, names: &mut Vec<u8>| -> u16 {
            match id {
                ResId::Int(n) => 0x8000 | n,
                ResId::Name(s) => {
                    let offset = (table_len + names.len()) as u16;
                    names.push(s.len() as u8);
                    names.extend_from_slice(s.as_bytes());
                    offset
                }
            }
        };
        let mut out = SHIFT.to_le_bytes().to_vec();
        let mut places = vec![0; self.resources.len()];
        for (type_id, list) in &types {
            let raw = name_offset(type_id, &mut names);
            out.extend_from_slice(&raw.to_le_bytes());
            out.extend_from_slice(&(list.len() as u16).to_le_bytes());
            out.extend_from_slice(&[0; 4]);
            for &i in list {
                let (_, id, flags, data) = &self.resources[i];
                places[i] = out.len();
                out.extend_from_slice(&[0, 0]);
                let len = (data.len() as u32).div_ceil(1 << SHIFT) as u16;
                out.extend_from_slice(&len.to_le_bytes());
                out.extend_from_slice(&flags.to_le_bytes());
                let raw = name_offset(id, &mut names);
                out.extend_from_slice(&raw.to_le_bytes());
                out.extend_from_slice(&[0; 4]);
            }
        }
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&names);
        out.push(0);
        (out, places)
    }

    /// Write the module.
    pub fn build(&self) -> Vec<u8> {
        let mut file = vec![0u8; NE_OFFSET];
        file[..2].copy_from_slice(b"MZ");
        // The DOS stub: one 512-byte page, a 4-paragraph header.
        let stub_len = 0x40 + DOS_STUB.len() + DOS_MESSAGE.len();
        file[2..4].copy_from_slice(&(stub_len as u16).to_le_bytes());
        file[4..6].copy_from_slice(&1u16.to_le_bytes());
        file[8..10].copy_from_slice(&4u16.to_le_bytes());
        file[0x0C..0x0E].copy_from_slice(&0xFFFFu16.to_le_bytes());
        file[0x10..0x12].copy_from_slice(&0xB8u16.to_le_bytes());
        file[0x18..0x1A].copy_from_slice(&0x40u16.to_le_bytes());
        file[0x3C..0x40].copy_from_slice(&(NE_OFFSET as u32).to_le_bytes());
        file[0x40..0x40 + DOS_STUB.len()].copy_from_slice(&DOS_STUB);
        let msg = 0x40 + DOS_STUB.len();
        file[msg..msg + DOS_MESSAGE.len()].copy_from_slice(DOS_MESSAGE);

        // The tables after the header, in the order linkers write them.
        let seg_off = HEADER;
        let res_off = seg_off + 8 * self.segments.len();
        let (res_table, res_places) = self.resource_table();
        let resname_off = res_off + res_table.len();
        let mut resident = Vec::new();
        for (name, ordinal) in std::iter::once(&(self.name.clone(), 0)).chain(&self.resident) {
            resident.push(name.len() as u8);
            resident.extend_from_slice(name.as_bytes());
            resident.extend_from_slice(&ordinal.to_le_bytes());
        }
        resident.push(0);
        let modref_off = resname_off + resident.len();
        let imp_off = modref_off + 2 * self.module_refs.len();
        let entry_off = imp_off + self.imported.len();
        let entry_table = self.entry_table();
        let tables_end = entry_off + entry_table.len();
        let mut nonres = Vec::new();
        for (name, ordinal) in
            std::iter::once(&(self.description.clone(), 0)).chain(&self.nonresident)
        {
            nonres.push(name.len() as u8);
            nonres.extend_from_slice(name.as_bytes());
            nonres.extend_from_slice(&ordinal.to_le_bytes());
        }
        nonres.push(0);
        let nonres_off = NE_OFFSET + tables_end;

        let mut ne = vec![0u8; tables_end];
        ne[..2].copy_from_slice(b"NE");
        ne[2] = 5;
        ne[3] = 10;
        let put16 = |ne: &mut Vec<u8>, at: usize, v: u16| {
            ne[at..at + 2].copy_from_slice(&v.to_le_bytes());
        };
        put16(&mut ne, 0x04, entry_off as u16);
        put16(&mut ne, 0x06, entry_table.len() as u16);
        put16(&mut ne, 0x0C, self.flags);
        put16(&mut ne, 0x0E, self.auto_data);
        put16(&mut ne, 0x10, self.heap);
        put16(&mut ne, 0x12, self.stack);
        put16(&mut ne, 0x14, self.cs_ip.1);
        put16(&mut ne, 0x16, self.cs_ip.0);
        put16(&mut ne, 0x18, self.ss_sp.1);
        put16(&mut ne, 0x1A, self.ss_sp.0);
        put16(&mut ne, 0x1C, self.segments.len() as u16);
        put16(&mut ne, 0x1E, self.module_refs.len() as u16);
        put16(&mut ne, 0x20, nonres.len() as u16);
        put16(&mut ne, 0x22, seg_off as u16);
        put16(&mut ne, 0x24, res_off as u16);
        put16(&mut ne, 0x26, resname_off as u16);
        put16(&mut ne, 0x28, modref_off as u16);
        put16(&mut ne, 0x2A, imp_off as u16);
        ne[0x2C..0x30].copy_from_slice(&(nonres_off as u32).to_le_bytes());
        put16(&mut ne, 0x30, 0);
        put16(&mut ne, 0x32, SHIFT);
        put16(&mut ne, 0x34, self.resources.len() as u16);
        ne[0x36] = OS_WINDOWS;
        put16(&mut ne, 0x3E, self.expected_version);
        ne[res_off..resname_off].copy_from_slice(&res_table);
        ne[resname_off..modref_off].copy_from_slice(&resident);
        for (i, &o) in self.module_refs.iter().enumerate() {
            put16(&mut ne, modref_off + 2 * i, o);
        }
        ne[imp_off..entry_off].copy_from_slice(&self.imported);
        ne[entry_off..tables_end].copy_from_slice(&entry_table);
        file.extend_from_slice(&ne);
        file.extend_from_slice(&nonres);

        let align = |file: &mut Vec<u8>| {
            let unit = 1usize << SHIFT;
            file.resize(file.len().next_multiple_of(unit), 0);
        };
        for (i, seg) in self.segments.iter().enumerate() {
            let entry = NE_OFFSET + seg_off + 8 * i;
            let mut flags = seg.flags & !SEG_RELOCINFO;
            let alloc = if seg.alloc == 0 {
                seg.data.len() as u32
            } else {
                seg.alloc
            };
            let sector = if seg.data.is_empty() {
                0
            } else {
                align(&mut file);
                let sector = (file.len() >> SHIFT) as u16;
                file.extend_from_slice(&seg.data);
                if !seg.relocations.is_empty() {
                    flags |= SEG_RELOCINFO;
                    file.extend_from_slice(&(seg.relocations.len() as u16).to_le_bytes());
                    for r in &seg.relocations {
                        file.extend_from_slice(&encode_relocation(r));
                    }
                }
                sector
            };
            file[entry..entry + 2].copy_from_slice(&sector.to_le_bytes());
            let len = (seg.data.len() as u32 & 0xFFFF) as u16;
            file[entry + 2..entry + 4].copy_from_slice(&len.to_le_bytes());
            file[entry + 4..entry + 6].copy_from_slice(&flags.to_le_bytes());
            let alloc = (alloc & 0xFFFF) as u16;
            file[entry + 6..entry + 8].copy_from_slice(&alloc.to_le_bytes());
        }
        for (i, (.., data)) in self.resources.iter().enumerate() {
            align(&mut file);
            let sector = (file.len() >> SHIFT) as u16;
            let place = NE_OFFSET + res_off + res_places[i];
            file[place..place + 2].copy_from_slice(&sector.to_le_bytes());
            file.extend_from_slice(data);
        }
        align(&mut file);
        file
    }
}

/// A relocation record's eight bytes.
fn encode_relocation(r: &Relocation) -> [u8; 8] {
    let additive = if r.additive { RELOC_ADDITIVE } else { 0 };
    let (kind, a, b) = match &r.target {
        Target::Internal { segment, offset } => (RELOC_INTERNAL, *segment as u16, *offset),
        Target::Ordinal { module, ordinal } => (RELOC_ORDINAL, *module, *ordinal),
        Target::Name {
            module,
            name_offset,
        } => (RELOC_NAME, *module, *name_offset),
        Target::OsFixup(kind) => (3, *kind, 0),
    };
    let mut out = [0u8; 8];
    out[0] = r.source;
    out[1] = kind | additive;
    out[2..4].copy_from_slice(&r.offset.to_le_bytes());
    out[4..6].copy_from_slice(&a.to_le_bytes());
    out[6..8].copy_from_slice(&b.to_le_bytes());
    out
}
