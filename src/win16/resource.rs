//! Resources. KERNEL's resource functions read a module's resources
//! from its NE file as they are. The 32-bit functions a 16-bit program's
//! calls go on to (LoadIconA, DialogBoxParamA, LoadMenuA, LoadStringA and
//! their family) find them in a PE image of resources the module gets
//! when they first need one (`image32`): the same resources with the
//! templates of dialogs and menus, the string tables and the accelerator
//! tables in their 32-bit forms, their strings wide. An instance handle a
//! 16-bit program passes them stands for that image's base.

use super::{Module16, global};
use crate::cpu::Cpu;
use crate::win32::Module;
use crate::win32::memory::{PAGE, PRIVATE, SHARED, USER};
use rust_dos_ne as ne;
use rust_dos_pe as pe;

/// The resource types whose 32-bit forms differ.
const RT_MENU: u16 = 4;
const RT_DIALOG: u16 = 5;
const RT_STRING: u16 = 6;
const RT_ACCELERATOR: u16 = 9;

const DS_SETFONT: u32 = 0x40;

/// A resource type or name, in the order a PE directory has them: names
/// first, sorted, then numbers.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Name(String),
    Id(u16),
}

fn key(id: &ne::ResId) -> Key {
    match id {
        ne::ResId::Int(n) => Key::Id(*n),
        ne::ResId::Name(s) => Key::Name(s.to_ascii_uppercase()),
    }
}

fn wide(bytes: &[u8]) -> Vec<u8> {
    crate::win32::kernel32::ansi_to_wide(bytes)
        .iter()
        .flat_map(|c| c.to_le_bytes())
        .collect()
}

/// A reader of a 16-bit resource's bytes.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn u8(&mut self) -> u8 {
        let b = self.bytes.get(self.at).copied().unwrap_or(0);
        self.at += 1;
        b
    }

    fn u16(&mut self) -> u16 {
        self.u8() as u16 | (self.u8() as u16) << 8
    }

    fn u32(&mut self) -> u32 {
        self.u16() as u32 | (self.u16() as u32) << 16
    }

    fn sz(&mut self) -> &[u8] {
        let start = self.at.min(self.bytes.len());
        let len = self.bytes[start..]
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(self.bytes.len() - start);
        self.at = start + len + 1;
        &self.bytes[start..start + len]
    }

    fn done(&self) -> bool {
        self.at >= self.bytes.len()
    }
}

/// A 32-bit resource being written.
struct Writer(Vec<u8>);

impl Writer {
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }

    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }

    fn wsz(&mut self, s: &[u8]) {
        self.0.extend_from_slice(&wide(s));
        self.u16(0);
    }

    fn align4(&mut self) {
        while !self.0.len().is_multiple_of(4) {
            self.0.push(0);
        }
    }
}

/// A 16-bit dialog template in its 32-bit form.
pub fn dialog32(bytes: &[u8]) -> Vec<u8> {
    let mut r = Reader { bytes, at: 0 };
    let mut w = Writer(Vec::new());
    let style = r.u32();
    let count = r.u8();
    w.u32(style);
    w.u32(0);
    w.u16(count as u16);
    for _ in 0..4 {
        w.u16(r.u16());
    }
    // The menu.
    match r.bytes.get(r.at).copied().unwrap_or(0) {
        0 => {
            r.at += 1;
            w.u16(0);
        }
        0xFF => {
            r.at += 1;
            let id = r.u16();
            w.u16(0xFFFF);
            w.u16(id);
        }
        _ => {
            let s = r.sz().to_vec();
            w.wsz(&s);
        }
    }
    // The class, the caption and the font.
    let class = r.sz().to_vec();
    w.wsz(&class);
    let caption = r.sz().to_vec();
    w.wsz(&caption);
    if style & DS_SETFONT != 0 {
        w.u16(r.u16());
        let face = r.sz().to_vec();
        w.wsz(&face);
    }
    for _ in 0..count {
        w.align4();
        let (x, y, cx, cy, id) = (r.u16(), r.u16(), r.u16(), r.u16(), r.u16());
        let style = r.u32();
        w.u32(style);
        w.u32(0);
        for v in [x, y, cx, cy, id] {
            w.u16(v);
        }
        let class = r.bytes.get(r.at).copied().unwrap_or(0);
        if class & 0x80 != 0 {
            r.at += 1;
            w.u16(0xFFFF);
            w.u16(class as u16);
        } else {
            let s = r.sz().to_vec();
            w.wsz(&s);
        }
        if r.bytes.get(r.at) == Some(&0xFF) {
            r.at += 1;
            let id = r.u16();
            w.u16(0xFFFF);
            w.u16(id);
        } else {
            let s = r.sz().to_vec();
            w.wsz(&s);
        }
        let extra = r.u8() as usize;
        w.u16(extra as u16);
        let data = r.bytes.get(r.at..r.at + extra).unwrap_or(&[]).to_vec();
        r.at += extra;
        w.0.extend_from_slice(&data);
    }
    w.0
}

const MF_POPUP: u16 = 0x0010;
const MF_END: u16 = 0x0080;

/// A 16-bit menu template in its 32-bit form.
fn menu32(bytes: &[u8]) -> Vec<u8> {
    let mut r = Reader { bytes, at: 0 };
    let mut w = Writer(Vec::new());
    let (version, offset) = (r.u16(), r.u16());
    w.u16(version);
    w.u16(offset);
    r.at += offset as usize;
    menu_items(&mut r, &mut w, 0);
    w.0
}

/// A menu's items, to the one with MF_END, each popup's own items after
/// it.
fn menu_items(r: &mut Reader, w: &mut Writer, depth: u32) {
    while !r.done() {
        let flags = r.u16();
        w.u16(flags);
        if flags & MF_POPUP == 0 {
            w.u16(r.u16());
        }
        let text = r.sz().to_vec();
        w.wsz(&text);
        if flags & MF_POPUP != 0 && depth < 16 {
            menu_items(r, w, depth + 1);
        }
        if flags & MF_END != 0 {
            return;
        }
    }
}

/// A 16-bit string table block in its 32-bit form.
fn strings32(bytes: &[u8]) -> Vec<u8> {
    let mut r = Reader { bytes, at: 0 };
    let mut w = Writer(Vec::new());
    for _ in 0..16 {
        let len = r.u8() as usize;
        let text = r.bytes.get(r.at..r.at + len).unwrap_or(&[]).to_vec();
        r.at += len;
        let units = crate::win32::kernel32::ansi_to_wide(&text);
        w.u16(units.len() as u16);
        for u in units {
            w.u16(u);
        }
    }
    w.0
}

/// A 16-bit accelerator table in its 32-bit form.
fn accelerators32(bytes: &[u8]) -> Vec<u8> {
    let mut w = Writer(Vec::new());
    for entry in bytes.as_chunks::<5>().0 {
        w.u16(entry[0] as u16);
        w.u16(u16::from_le_bytes([entry[1], entry[2]]));
        w.u16(u16::from_le_bytes([entry[3], entry[4]]));
        w.u16(0);
        if entry[0] & 0x80 != 0 {
            break;
        }
    }
    w.0
}

/// The 32-bit form of the resource `data` of the type `kind`.
fn convert(kind: &Key, data: &[u8]) -> Vec<u8> {
    match kind {
        Key::Id(RT_DIALOG) => dialog32(data),
        Key::Id(RT_MENU) => menu32(data),
        Key::Id(RT_STRING) => strings32(data),
        Key::Id(RT_ACCELERATOR) => accelerators32(data),
        _ => data.to_vec(),
    }
}

/// The resource section of the resources `resources` (type, name, data)
/// at the RVA `rva`.
fn section(resources: &[(Key, Key, Vec<u8>)], rva: u32) -> Vec<u8> {
    use std::collections::BTreeMap;
    let mut tree: BTreeMap<&Key, BTreeMap<&Key, &[u8]>> = BTreeMap::new();
    for (kind, name, data) in resources {
        tree.entry(kind).or_default().entry(name).or_insert(data);
    }
    let dir_size = |n: usize| 16 + 8 * n;
    let mut at = dir_size(tree.len());
    let mut type_dirs = Vec::new();
    for names in tree.values() {
        type_dirs.push(at);
        at += dir_size(names.len());
    }
    // Each name's directory has one language.
    let mut name_dirs = Vec::new();
    for names in tree.values() {
        for _ in names.values() {
            name_dirs.push(at);
            at += dir_size(1);
        }
    }
    let leaves = name_dirs.len();
    let data_entries = at;
    at += 16 * leaves;
    // The names' strings.
    let mut strings: Vec<(&Key, usize)> = Vec::new();
    let mut string_bytes = Vec::new();
    for (kind, names) in &tree {
        for key in std::iter::once(*kind).chain(names.keys().copied()) {
            if let Key::Name(s) = key
                && !strings.iter().any(|(k, _)| *k == key)
            {
                strings.push((key, at + string_bytes.len()));
                let units = crate::win32::kernel32::ansi_to_wide(s.as_bytes());
                string_bytes.extend_from_slice(&(units.len() as u16).to_le_bytes());
                for u in units {
                    string_bytes.extend_from_slice(&u.to_le_bytes());
                }
            }
        }
    }
    let mut out = vec![0u8; at];
    out.extend_from_slice(&string_bytes);
    let put32 = |out: &mut Vec<u8>, at: usize, v: u32| {
        out[at..at + 4].copy_from_slice(&v.to_le_bytes());
    };
    let name_field = |key: &Key| match key {
        Key::Id(n) => *n as u32,
        Key::Name(_) => {
            0x8000_0000
                | strings
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map_or(0, |(_, at)| *at as u32)
        }
    };
    let directory = |out: &mut Vec<u8>, at: usize, keys: &[&Key], targets: &[u32]| {
        let named = keys.iter().filter(|k| matches!(k, Key::Name(_))).count();
        out[at + 12..at + 14].copy_from_slice(&(named as u16).to_le_bytes());
        out[at + 14..at + 16].copy_from_slice(&((keys.len() - named) as u16).to_le_bytes());
        for (i, (key, target)) in keys.iter().zip(targets).enumerate() {
            put32(out, at + 16 + 8 * i, name_field(key));
            put32(out, at + 20 + 8 * i, *target);
        }
    };
    let kinds: Vec<&Key> = tree.keys().copied().collect();
    let targets: Vec<u32> = type_dirs.iter().map(|&d| 0x8000_0000 | d as u32).collect();
    directory(&mut out, 0, &kinds, &targets);
    let mut leaf = 0;
    for (names, &type_dir) in tree.values().zip(&type_dirs) {
        let keys: Vec<&Key> = names.keys().copied().collect();
        let dirs: Vec<u32> = (0..keys.len())
            .map(|i| 0x8000_0000 | name_dirs[leaf + i] as u32)
            .collect();
        directory(&mut out, type_dir, &keys, &dirs);
        for data in names.values() {
            let entry = data_entries + 16 * leaf;
            while !out.len().is_multiple_of(4) {
                out.push(0);
            }
            let at = rva + out.len() as u32;
            put32(&mut out, entry, at);
            put32(&mut out, entry + 4, data.len() as u32);
            out.extend_from_slice(data);
            let neutral = Key::Id(0);
            directory(&mut out, name_dirs[leaf], &[&neutral], &[entry as u32]);
            leaf += 1;
        }
    }
    out
}

/// The base of the 32-bit image of the resources of the module at
/// `index`, made now if it has none yet. 0 if there is no memory for it.
pub fn image32(cpu: &mut Cpu, index: usize) -> u32 {
    let module = &cpu.bus.win32.win16.modules[index];
    if module.resources32 != 0 {
        return module.resources32;
    }
    let resources = resources_of(module);
    let name = format!("{}.16", module.name);
    let rsrc = section(&resources, 0x1000);
    // The image's size, its base not known yet: build it once to learn it.
    let build = |base: u32| {
        let mut b = pe::Builder::dll(base, &name);
        let rva = b.section(".rsrc", &rsrc, pe::SCN_INITIALIZED_DATA | pe::SCN_READ);
        b.directory(pe::DIR_RESOURCE, rva, rsrc.len() as u32);
        b.build()
    };
    let size = (build(0).len() as u32).next_multiple_of(PAGE);
    let pid = cpu.bus.win32.win16.pid;
    let made = super::with_win16(cpu, pid, |p, bus| {
        let base = p.memory.reserve_image(None, size, PRIVATE, SHARED)?;
        let file = build(base);
        p.memory.commit(bus, base, size, USER)?;
        p.memory.write(bus, base, &file)?;
        p.modules.push(Module {
            name: name.clone(),
            path: String::new(),
            base,
            size,
            entry: 0,
            refs: 0,
            flags: crate::win32::loader::BUILTIN | crate::win32::loader::DATAFILE,
            deps: Vec::new(),
        });
        Some(base)
    })
    .flatten()
    .unwrap_or(0);
    cpu.bus.win32.win16.modules[index].resources32 = made;
    made
}

/// The module's resources, in their 32-bit forms.
fn resources_of(module: &Module16) -> Vec<(Key, Key, Vec<u8>)> {
    let Ok(parsed) = ne::Module::parse(&module.file) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for kind in &parsed.resources {
        let k = key(&kind.type_id);
        for res in &kind.resources {
            let Ok(data) = parsed.resource_data(&module.file, res) else {
                continue;
            };
            out.push((k.clone(), key(&res.id), convert(&k, data)));
        }
    }
    // The names the name table gives resources, as the 32-bit functions
    // find them.
    for (kind, name, t, id) in name_table(&module.file, &parsed) {
        let (number, alias) = (
            (key(&ne::ResId::Int(t)), key(&ne::ResId::Int(id))),
            (key(&kind), key(&name)),
        );
        if out.iter().any(|(k, n, _)| (k, n) == (&alias.0, &alias.1)) {
            continue;
        }
        if let Some(data) = out
            .iter()
            .find(|(k, n, _)| (k, n) == (&number.0, &number.1))
            .map(|r| r.2.clone())
        {
            out.push((alias.0, alias.1, data));
        }
    }
    out
}

/// The resource a 16-bit program names: a number (a pointer whose
/// selector is 0), "#number" or a name, given the pointer's flat address
/// or the number.
pub fn res_id(cpu: &Cpu, arg: u32) -> ne::ResId {
    if arg < 0x1_0000 {
        return ne::ResId::Int(arg as u16);
    }
    let text = super::string(cpu, arg);
    match text.strip_prefix('#').and_then(|n| n.parse().ok()) {
        Some(n) => ne::ResId::Int(n),
        None => ne::ResId::Name(text),
    }
}

/// A resource handle (HRSRC): its type's place and its own in the
/// module's resource table, from 1, as one word.
fn hrsrc(type_index: usize, res_index: usize) -> u16 {
    ((type_index + 1) << 10 | res_index) as u16
}

/// FindResource(module, name, type): the resource's handle, or 0.
pub fn find(cpu: &Cpu, module: u16, name: &ne::ResId, kind: &ne::ResId) -> u16 {
    let Some(index) = super::module_index(cpu, module) else {
        return 0;
    };
    let file = &cpu.bus.win32.win16.modules[index].file;
    let Ok(parsed) = ne::Module::parse(file) else {
        return 0;
    };
    let named = name_table(file, &parsed)
        .into_iter()
        .find(|(k, n, _, _)| k.matches(kind) && n.matches(name));
    let (name, kind) = match named {
        Some((_, _, t, id)) => (&ne::ResId::Int(id), &ne::ResId::Int(t)),
        None => (name, kind),
    };
    for (t, rt) in parsed.resources.iter().enumerate() {
        if !rt.type_id.matches(kind) {
            continue;
        }
        if let Some(r) = rt.resources.iter().position(|r| r.id.matches(name)) {
            return hrsrc(t, r);
        }
    }
    0
}

/// A module's name table (RT_NAMETABLE, Windows 3.0's), which gives
/// resources names: for each name, the type and the name (numbers or
/// names) and the numbers they stand for. Each entry is its length, the
/// type and the id (a name's number with bit 15 set), then the type's
/// name and the id's name.
fn name_table(file: &[u8], parsed: &ne::Module) -> Vec<(ne::ResId, ne::ResId, u16, u16)> {
    const RT_NAMETABLE: u16 = 15;
    let Some(table) = parsed
        .resources
        .iter()
        .find(|t| t.type_id == ne::ResId::Int(RT_NAMETABLE))
        .and_then(|t| t.resources.first())
    else {
        return Vec::new();
    };
    let Ok(data) = parsed.resource_data(file, table) else {
        return Vec::new();
    };
    let word = |at: usize| Some(u16::from_le_bytes([*data.get(at)?, *data.get(at + 1)?]));
    let mut entries = Vec::new();
    let mut at = 0;
    while let (Some(len), Some(t), Some(id)) = (word(at), word(at + 2), word(at + 4)) {
        if len < 6 {
            break;
        }
        let end = (at + len as usize).min(data.len());
        let mut strings = data[(at + 6).min(end)..end].split(|&b| b == 0);
        let mut text = || String::from_utf8_lossy(strings.next().unwrap_or_default()).into_owned();
        let (type_name, id_name) = (text(), text());
        let entry = |value: u16, text: String| {
            if value & 0x8000 != 0 {
                ne::ResId::Name(text)
            } else {
                ne::ResId::Int(value)
            }
        };
        entries.push((
            entry(t, type_name),
            entry(id, id_name),
            t & 0x7FFF,
            id & 0x7FFF,
        ));
        at += len as usize;
    }
    entries
}

/// The entry of the resource `hrsrc` in a module's resource table.
fn entry(parsed: &ne::Module, hrsrc: u16) -> Option<&ne::Resource> {
    let t = (hrsrc >> 10) as usize;
    let rt = parsed.resources.get(t.checked_sub(1)?)?;
    rt.resources.get((hrsrc & 0x3FF) as usize)
}

/// The data of the resource `hrsrc` of the module at `index`.
fn data(cpu: &Cpu, index: usize, hrsrc: u16) -> Option<Vec<u8>> {
    let file = &cpu.bus.win32.win16.modules[index].file;
    let parsed = ne::Module::parse(file).ok()?;
    let res = entry(&parsed, hrsrc)?;
    parsed.resource_data(file, res).ok().map(<[u8]>::to_vec)
}

/// Where the data of the resource `hrsrc` of `module` is in the
/// module's file.
pub fn offset(cpu: &Cpu, module: u16, hrsrc: u16) -> Option<u32> {
    let index = super::module_index(cpu, module)?;
    let parsed = ne::Module::parse(&cpu.bus.win32.win16.modules[index].file).ok()?;
    entry(&parsed, hrsrc).map(|r| r.offset)
}

/// SizeofResource.
pub fn size(cpu: &Cpu, module: u16, hrsrc: u16) -> u32 {
    super::module_index(cpu, module)
        .and_then(|i| data(cpu, i, hrsrc))
        .map_or(0, |d| d.len() as u32)
}

/// LoadResource: a global block with the resource's data, owned by the
/// module, the same one each time. Its handle, or 0.
pub fn load(cpu: &mut Cpu, module: u16, hrsrc: u16) -> u16 {
    let Some(index) = super::module_index(cpu, module) else {
        return 0;
    };
    if let Some(&(_, h)) = cpu.bus.win32.win16.modules[index]
        .loaded
        .iter()
        .find(|(r, _)| *r == hrsrc)
        && global::get(cpu, h).is_some()
    {
        return h;
    }
    let Some(bytes) = data(cpu, index, hrsrc) else {
        return 0;
    };
    let owner = cpu.bus.win32.win16.modules[index].handle;
    let handle = global::alloc(
        cpu,
        global::GMEM_MOVEABLE | global::GMEM_DISCARDABLE,
        bytes.len() as u32,
        owner,
        false,
    );
    if handle == 0 {
        return 0;
    }
    if let Some(a) = global::get(cpu, handle) {
        super::poke(cpu, a.base, &bytes);
    }
    let module = &mut cpu.bus.win32.win16.modules[index];
    module.loaded.retain(|(r, _)| *r != hrsrc);
    module.loaded.push((hrsrc, handle));
    handle
}
