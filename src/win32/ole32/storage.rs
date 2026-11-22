//! Structured storage: compound files (docfiles), which StgCreateDocfile
//! makes and StgOpenStorage opens, with their IStorage, the IStream of
//! their streams and IEnumSTATSTG, as Windows 98's OLE32 has them.
//!
//! A docfile open in a process is a tree of elements here (`Doc`):
//! storages, and streams with their bytes. The file is read into the
//! tree when it is opened and written from it, in the compound file
//! format (cfb), when the root storage commits or, opened without
//! STGM_TRANSACTED, when its last reference goes. The objects the
//! program holds (`Object`) name an element of a docfile; an element
//! destroyed under one, or a docfile whose root storage was released,
//! leaves it answering STG_E_REVERTED, as Windows does.
//!
//! What the methods check and return follows Windows' docfile
//! implementation as Wine's ole32/storage32.c describes it (LGPL 2.1 or
//! later):
//!
//! Copyright 1999 Francis Beaudet
//! Copyright 1999 Sylvain St-Germain
//! Copyright 1999 Thuy Nguyen
//! Copyright 2005 Mike McCormack

use super::stream::{IID_ISEQUENTIALSTREAM, IID_ISTREAM, STATSTG_SIZE, STGTY_STREAM};
use super::{dll_index, vtable};
use crate::cpu::Cpu;
use crate::win32::com::{self, E_INVALIDARG, E_NOINTERFACE, E_OUTOFMEMORY, Guid, S_OK};
use crate::win32::kernel32::files;
use crate::win32::task::{Next, Task};
use crate::win32::user32::current_pid;
use std::io::{Read, Write};

pub const STG_E_FILENOTFOUND: u32 = 0x8003_0002;
pub const STG_E_ACCESSDENIED: u32 = 0x8003_0005;
pub const STG_E_INVALIDPOINTER: u32 = 0x8003_0009;
pub const STG_E_INVALIDFUNCTION: u32 = 0x8003_0001;
pub const STG_E_WRITEFAULT: u32 = 0x8003_001D;
pub const STG_E_READFAULT: u32 = 0x8003_001E;
pub const STG_E_SHAREVIOLATION: u32 = 0x8003_0020;
pub const STG_E_FILEALREADYEXISTS: u32 = 0x8003_0050;
pub const STG_E_INVALIDNAME: u32 = 0x8003_00FC;
pub const STG_E_INVALIDFLAG: u32 = 0x8003_00FF;
pub const STG_E_REVERTED: u32 = 0x8003_0102;
pub const STG_E_MEDIUMFULL: u32 = 0x8003_0070;
const S_FALSE: u32 = 1;

/// The most bytes a stream of a docfile holds here: a write or SetSize
/// past it fails, rather than the emulator taking that much memory.
const MAX_STREAM: u64 = 0x4000_0000;

pub const IID_ISTORAGE: Guid = Guid::new(0xB, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_IENUMSTATSTG: Guid = Guid::new(0xD, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);

/// STGM: the access, the sharing and how a docfile is made or opened.
const STGM_ACCESS: u32 = 0x3;
const STGM_SHARE: u32 = 0x70;
const STGM_SHARE_EXCLUSIVE: u32 = 0x10;
const STGM_CREATE: u32 = 0x1000;
const STGM_TRANSACTED: u32 = 0x1_0000;
const STGM_DELETEONRELEASE: u32 = 0x400_0000;

const STGTY_STORAGE: u32 = 1;
const STATFLAG_NONAME: u32 = 1;

/// The longest name an element has, in UTF-16 units, without its zero.
const MAX_NAME: usize = 31;

/// An element of a docfile: a storage, or a stream and its bytes.
#[derive(Clone, Debug, Default)]
pub struct Element {
    name: Vec<u16>,
    /// The storage it is in (the root is element 0, its own parent).
    parent: u32,
    storage: bool,
    clsid: Vec<u8>,
    bits: u32,
    data: Vec<u8>,
    /// FILETIMEs: storages have them, streams have 0.
    created: u64,
    modified: u64,
    /// DestroyElement took it, or the storage it was in.
    gone: bool,
}

/// A docfile open in a process.
#[derive(Clone, Debug, Default)]
pub struct Doc {
    pid: u32,
    /// The file's full path; empty for one made without a name, which
    /// lives in memory only.
    path: String,
    elements: Vec<Element>,
    /// The root storage's STGM.
    mode: u32,
    /// Elements changed since the file was last written.
    dirty: bool,
    /// The root storage was released: the docfile's other objects are
    /// reverted.
    closed: bool,
    /// The objects of it the program holds.
    objects: u32,
}

/// What an object of a docfile is.
const STORAGE: u32 = 0;
const STREAM: u32 = 1;
const ENUM: u32 = 2;

/// An object the program holds: an IStorage or IStream of an element of
/// a docfile, or an IEnumSTATSTG of a storage's elements.
#[derive(Clone, Debug, Default)]
pub struct Object {
    pid: u32,
    kind: u32,
    doc: u32,
    element: u32,
    refs: u32,
    /// The STGM it was opened with.
    mode: u32,
    /// A stream's position.
    position: u64,
    /// An enumerator's elements and the next one's place.
    list: Vec<u32>,
    at: u32,
}

/// The docfiles and their objects.
#[derive(Clone, Debug, Default)]
pub struct Storages {
    docs: Vec<Option<Doc>>,
    objects: Vec<Option<Object>>,
}

fn storages(cpu: &mut Cpu) -> &mut Storages {
    &mut super::state(cpu).storage.0
}

fn doc(cpu: &mut Cpu, id: u32) -> &mut Doc {
    storages(cpu).docs[id as usize]
        .as_mut()
        .expect("a docfile of the table")
}

fn obj(cpu: &mut Cpu, id: u32) -> &mut Object {
    storages(cpu).objects[id as usize]
        .as_mut()
        .expect("an object of the table")
}

fn element(cpu: &mut Cpu, doc_id: u32, index: u32) -> &mut Element {
    &mut doc(cpu, doc_id).elements[index as usize]
}

/// A process ended: its docfiles and objects went with it, unwritten.
pub fn process_ended(cpu: &mut Cpu, pid: u32) {
    let s = storages(cpu);
    for slot in s.objects.iter_mut() {
        if slot.as_ref().is_some_and(|o| o.pid == pid) {
            *slot = None;
        }
    }
    for slot in s.docs.iter_mut() {
        if slot.as_ref().is_some_and(|d| d.pid == pid) {
            *slot = None;
        }
    }
}

// Names and times.

/// The element name at `at`: STG_E_INVALIDPOINTER for none,
/// STG_E_INVALIDNAME for one too long or with a character a name can't
/// have.
fn name_at(cpu: &mut Cpu, at: u32) -> Result<Vec<u16>, u32> {
    if at == 0 {
        return Err(STG_E_INVALIDPOINTER);
    }
    let mut name = Vec::new();
    loop {
        if !cpu.bus.guest_probe(at + 2 * name.len() as u32, 2, false) {
            return Err(STG_E_INVALIDPOINTER);
        }
        let c = cpu.bus.guest_read_16(at + 2 * name.len() as u32);
        if c == 0 {
            break;
        }
        if name.len() == MAX_NAME || b"\\/:!".iter().any(|&b| c == b as u16) {
            return Err(STG_E_INVALIDNAME);
        }
        name.push(c);
    }
    Ok(name)
}

/// Whether two element names are the same, as the compound file format
/// compares them: without regard to case.
fn same_name(a: &[u16], b: &[u16]) -> bool {
    let upper = |c: u16| match char::from_u32(c as u32) {
        Some(ch) => {
            let mut u = ch.to_uppercase();
            match (u.next(), u.next()) {
                (Some(x), None) if (x as u32) < 0x1_0000 => x as u32 as u16,
                _ => c,
            }
        }
        None => c,
    };
    a.len() == b.len() && a.iter().zip(b).all(|(&x, &y)| upper(x) == upper(y))
}

/// FILETIME's count at the Unix epoch.
const UNIX_EPOCH_TICKS: u64 = 116_444_736_000_000_000;

// cfb's times are web_time's, which are std's except in the browser.
fn system_time(ticks: u64) -> web_time::SystemTime {
    let epoch = web_time::UNIX_EPOCH;
    if ticks >= UNIX_EPOCH_TICKS {
        epoch + std::time::Duration::from_nanos((ticks - UNIX_EPOCH_TICKS) * 100)
    } else {
        epoch - std::time::Duration::from_nanos((UNIX_EPOCH_TICKS - ticks) * 100)
    }
}

fn ticks_of(time: web_time::SystemTime) -> u64 {
    match time.duration_since(web_time::UNIX_EPOCH) {
        Ok(d) => UNIX_EPOCH_TICKS.saturating_add((d.as_nanos() / 100) as u64),
        Err(e) => UNIX_EPOCH_TICKS.saturating_sub((e.duration().as_nanos() / 100) as u64),
    }
}

// The file.

/// The element path cfb names `index` of `elements` by.
fn path_of(elements: &[Element], index: u32) -> String {
    let mut names = Vec::new();
    let mut i = index;
    while i != 0 {
        let e = &elements[i as usize];
        names.push(String::from_utf16_lossy(&e.name));
        i = e.parent;
    }
    names.reverse();
    format!("/{}", names.join("/"))
}

fn uuid_of(clsid: &[u8]) -> uuid::Uuid {
    uuid::Uuid::from_bytes_le(<[u8; 16]>::try_from(clsid).unwrap_or_default())
}

/// The compound file of `elements`, as Windows writes one.
fn encode(elements: &[Element]) -> std::io::Result<Vec<u8>> {
    let mut f = cfb::CompoundFile::create(std::io::Cursor::new(Vec::new()))?;
    // Parents before children: an element's parent has a lower index
    // when it was made, but one renamed or read in keeps its place, so
    // the tree is walked.
    let mut order = Vec::new();
    let mut stack = vec![0u32];
    while let Some(i) = stack.pop() {
        order.push(i);
        for (j, e) in elements.iter().enumerate().rev() {
            if j != 0 && !e.gone && e.parent == i {
                stack.push(j as u32);
            }
        }
    }
    for &i in &order {
        let e = &elements[i as usize];
        let path = path_of(elements, i);
        if i != 0 {
            if e.storage {
                f.create_storage(&path)?;
            } else {
                f.create_stream(&path)?.write_all(&e.data)?;
            }
        }
        if e.storage {
            f.set_storage_clsid(&path, uuid_of(&e.clsid))?;
            f.set_created_time(&path, system_time(e.created))?;
            f.set_modified_time(&path, system_time(e.modified))?;
        }
        f.set_state_bits(&path, e.bits)?;
    }
    f.flush()?;
    Ok(f.into_inner().into_inner())
}

/// The elements of the compound file `data`, or None when it isn't one.
fn decode(data: &[u8]) -> Option<Vec<Element>> {
    let mut f = cfb::CompoundFile::open(std::io::Cursor::new(data)).ok()?;
    let root = f.root_entry();
    let mut elements = vec![Element {
        parent: 0,
        storage: true,
        clsid: root.clsid().to_bytes_le().to_vec(),
        bits: root.state_bits(),
        created: ticks_of(root.created()),
        modified: ticks_of(root.modified()),
        ..Element::default()
    }];
    let mut paths = vec![std::path::PathBuf::from("/")];
    let entries: Vec<_> = f
        .walk()
        .filter(|e| !e.is_root())
        .map(|e| {
            (
                e.path().to_path_buf(),
                e.name().to_string(),
                e.is_storage(),
                *e.clsid(),
                e.state_bits(),
                e.created(),
                e.modified(),
            )
        })
        .collect();
    for (path, name, storage, clsid, bits, created, modified) in entries {
        let parent = path
            .parent()
            .and_then(|p| paths.iter().position(|q| q == p))
            .unwrap_or(0);
        let mut data = Vec::new();
        if !storage {
            f.open_stream(&path).ok()?.read_to_end(&mut data).ok()?;
        }
        elements.push(Element {
            name: name.encode_utf16().collect(),
            parent: parent as u32,
            storage,
            clsid: clsid.to_bytes_le().to_vec(),
            bits,
            data,
            created: if storage { ticks_of(created) } else { 0 },
            modified: if storage { ticks_of(modified) } else { 0 },
            gone: false,
        });
        paths.push(path);
    }
    Some(elements)
}

/// Write the docfile `id` to its file.
fn save(cpu: &mut Cpu, id: u32) -> Result<(), u32> {
    let d = doc(cpu, id);
    if d.path.is_empty() {
        d.dirty = false;
        return Ok(());
    }
    let path = d.path.clone();
    let bytes = encode(&d.elements).map_err(|_| STG_E_WRITEFAULT)?;
    files::write_whole(cpu, &path, &bytes).map_err(|_| STG_E_WRITEFAULT)?;
    doc(cpu, id).dirty = false;
    Ok(())
}

// Objects.

fn new_doc(cpu: &mut Cpu, path: String, mode: u32, elements: Vec<Element>) -> u32 {
    let d = Doc {
        pid: current_pid(cpu),
        path,
        elements,
        mode,
        dirty: false,
        closed: false,
        objects: 0,
    };
    let docs = &mut storages(cpu).docs;
    match docs.iter().position(Option::is_none) {
        Some(i) => {
            docs[i] = Some(d);
            i as u32
        }
        None => {
            docs.push(Some(d));
            docs.len() as u32 - 1
        }
    }
}

/// A new object of the element `index` of the docfile `doc_id`: its
/// interface, or None when memory is out.
fn new_object(cpu: &mut Cpu, kind: u32, doc_id: u32, index: u32, mode: u32) -> Option<u32> {
    let o = Object {
        pid: current_pid(cpu),
        kind,
        doc: doc_id,
        element: index,
        refs: 1,
        mode,
        ..Object::default()
    };
    let objects = &mut storages(cpu).objects;
    let id = match objects.iter().position(Option::is_none) {
        Some(i) => {
            objects[i] = Some(o);
            i
        }
        None => {
            objects.push(Some(o));
            objects.len() - 1
        }
    };
    let name = match kind {
        STORAGE => "IStorage",
        STREAM => "IStorageStream",
        _ => "IEnumSTATSTG",
    };
    let at = com::new_interface(cpu, dll_index(), vtable(name), id as u32, 0);
    match at {
        Some(_) => doc(cpu, doc_id).objects += 1,
        None => storages(cpu).objects[id] = None,
    }
    at
}

/// The object of the interface `this`, if it is one of these of `kind`.
fn object_of(cpu: &mut Cpu, this: u32, kind: u32) -> Option<u32> {
    let (v, id) = com::interface_of(cpu, dll_index(), this)?;
    let name = match kind {
        STORAGE => "IStorage",
        STREAM => "IStorageStream",
        _ => "IEnumSTATSTG",
    };
    if v != vtable(name) {
        return None;
    }
    storages(cpu)
        .objects
        .get(id as usize)?
        .as_ref()
        .filter(|o| o.kind == kind)?;
    Some(id)
}

/// The object of `this` of `kind` whose element is still there: its id,
/// or the HRESULT to fail with.
fn live(cpu: &mut Cpu, this: u32, kind: u32) -> Result<u32, u32> {
    let id = object_of(cpu, this, kind).ok_or(E_INVALIDARG)?;
    let (doc_id, index) = (obj(cpu, id).doc, obj(cpu, id).element);
    let d = doc(cpu, doc_id);
    if d.closed || d.elements[index as usize].gone {
        return Err(STG_E_REVERTED);
    }
    Ok(id)
}

fn writable(mode: u32) -> bool {
    mode & STGM_ACCESS != 0
}

/// The live child of the storage `parent` named `name`.
fn child(cpu: &mut Cpu, doc_id: u32, parent: u32, name: &[u16]) -> Option<u32> {
    doc(cpu, doc_id)
        .elements
        .iter()
        .enumerate()
        .position(|(i, e)| i != 0 && !e.gone && e.parent == parent && same_name(&e.name, name))
        .map(|i| i as u32)
}

/// Whether an object the program holds names the element `index`.
fn is_open(cpu: &mut Cpu, doc_id: u32, index: u32) -> bool {
    storages(cpu)
        .objects
        .iter()
        .flatten()
        .any(|o| o.doc == doc_id && o.element == index && o.kind != ENUM)
}

/// Whether the element `index` is `ancestor` or in it, at any depth.
fn within(cpu: &mut Cpu, doc_id: u32, index: u32, ancestor: u32) -> bool {
    let elements = &doc(cpu, doc_id).elements;
    let mut i = index;
    // No more steps than there are elements, whatever the parents say.
    for _ in 0..=elements.len() {
        if i == ancestor {
            return true;
        }
        if i == 0 {
            return false;
        }
        i = elements[i as usize].parent;
    }
    false
}

/// Destroy the element `index` and what is in it.
fn destroy(cpu: &mut Cpu, doc_id: u32, index: u32) {
    let d = doc(cpu, doc_id);
    d.elements[index as usize].gone = true;
    d.elements[index as usize].data = Vec::new();
    loop {
        let mut more = false;
        for i in 1..d.elements.len() {
            let p = d.elements[i].parent as usize;
            if !d.elements[i].gone && d.elements[p].gone {
                d.elements[i].gone = true;
                d.elements[i].data = Vec::new();
                more = true;
            }
        }
        if !more {
            break;
        }
    }
}

/// An element changed: its storage's modified time is now's.
fn touched(cpu: &mut Cpu, doc_id: u32, storage: u32) {
    let now = crate::win32::kernel32::time::file_time_now(cpu);
    let d = doc(cpu, doc_id);
    d.dirty = true;
    d.elements[storage as usize].modified = now;
}

/// Write STATSTG for the element `index` of `doc_id` at `out`, opened
/// with `mode`: the name, unless `flag` is STATFLAG_NONAME, in a block of
/// the task allocator.
fn write_stat(cpu: &mut Cpu, doc_id: u32, index: u32, mode: u32, out: u32, flag: u32) -> bool {
    let d = doc(cpu, doc_id);
    let e = d.elements[index as usize].clone();
    let name: Vec<u16> = if index == 0 {
        d.path.encode_utf16().collect()
    } else {
        e.name.clone()
    };
    let mut name_at = 0;
    if flag & STATFLAG_NONAME == 0 {
        name_at = super::task_alloc(cpu, 2 * name.len() as u32 + 2);
        if name_at == 0 {
            return false;
        }
        for (i, &c) in name.iter().chain(&[0]).enumerate() {
            cpu.bus.guest_write_16(name_at + 2 * i as u32, c);
        }
    }
    cpu.bus.guest_fill(out, STATSTG_SIZE, 0);
    cpu.bus.guest_write_32(out, name_at);
    cpu.bus.guest_write_32(
        out + 4,
        if e.storage {
            STGTY_STORAGE
        } else {
            STGTY_STREAM
        },
    );
    cpu.bus.guest_write_32(out + 8, e.data.len() as u32);
    for (at, t) in [(16, e.modified), (24, e.created), (32, 0)] {
        cpu.bus.guest_write_32(out + at, t as u32);
        cpu.bus.guest_write_32(out + at + 4, (t >> 32) as u32);
    }
    cpu.bus.guest_write_32(out + 40, mode);
    let clsid = <[u8; 16]>::try_from(e.clsid.as_slice()).unwrap_or_default();
    cpu.bus.guest_write_bytes(out + 48, &clsid);
    cpu.bus.guest_write_32(out + 64, e.bits);
    true
}

// The API.

/// The docfile path `name` names, or STG_E_INVALIDNAME.
fn file_name(cpu: &mut Cpu, name: u32) -> Result<String, u32> {
    let mut units = Vec::new();
    loop {
        let at = name + 2 * units.len() as u32;
        if units.len() > 260 || !cpu.bus.guest_probe(at, 2, false) {
            return Err(STG_E_INVALIDNAME);
        }
        let c = cpu.bus.guest_read_16(at);
        if c == 0 {
            break;
        }
        units.push(c);
    }
    let bytes = crate::win32::kernel32::wide_to_ansi(cpu, &units);
    let text = crate::dosstr::from_bytes(&bytes);
    Ok(files::full_path(cpu, &text))
}

/// StgCreateDocfile(name, mode, reserved, out): a new docfile in the file
/// `name` (made anew with STGM_CREATE, else only if it isn't there), or
/// in memory when there is no name.
pub fn stg_create_docfile(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[name, mode, _, out] = args else {
        unreachable!()
    };
    if out == 0 || !cpu.bus.guest_probe(out, 4, true) {
        return Some(STG_E_INVALIDPOINTER);
    }
    cpu.bus.guest_write_32(out, 0);
    if mode & STGM_ACCESS == 0 || mode & STGM_ACCESS == 3 {
        return Some(STG_E_INVALIDFLAG);
    }
    let path = if name == 0 {
        String::new()
    } else {
        match file_name(cpu, name) {
            Ok(p) => p,
            Err(e) => return Some(e),
        }
    };
    if !path.is_empty() {
        if open_doc(cpu, &path).is_some() {
            return Some(STG_E_SHAREVIOLATION);
        }
        if files::is_file(cpu, &path) && mode & STGM_CREATE == 0 {
            return Some(STG_E_FILEALREADYEXISTS);
        }
    }
    let now = crate::win32::kernel32::time::file_time_now(cpu);
    let root = Element {
        storage: true,
        clsid: vec![0; 16],
        created: now,
        modified: now,
        ..Element::default()
    };
    let deleted = name == 0 || mode & STGM_DELETEONRELEASE != 0;
    let doc_id = new_doc(cpu, path, mode, vec![root]);
    if !deleted && let Err(e) = save(cpu, doc_id) {
        storages(cpu).docs[doc_id as usize] = None;
        return Some(e);
    }
    if deleted {
        doc(cpu, doc_id).path.clear();
    }
    match new_object(cpu, STORAGE, doc_id, 0, mode) {
        Some(at) => {
            cpu.bus.guest_write_32(out, at);
            Some(S_OK)
        }
        None => {
            storages(cpu).docs[doc_id as usize] = None;
            Some(E_OUTOFMEMORY)
        }
    }
}

/// The docfile of `path` open in the running process, if there is one.
fn open_doc(cpu: &mut Cpu, path: &str) -> Option<u32> {
    let pid = current_pid(cpu);
    storages(cpu)
        .docs
        .iter()
        .position(|d| {
            d.as_ref()
                .is_some_and(|d| d.pid == pid && !d.closed && d.path.eq_ignore_ascii_case(path))
        })
        .map(|i| i as u32)
}

/// StgOpenStorage(name, priority, mode, exclude, reserved, out): the
/// docfile in the file `name`.
pub fn stg_open_storage(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[name, _, mode, _, _, out] = args else {
        unreachable!()
    };
    if out == 0 || !cpu.bus.guest_probe(out, 4, true) {
        return Some(STG_E_INVALIDPOINTER);
    }
    cpu.bus.guest_write_32(out, 0);
    if name == 0 {
        return Some(STG_E_INVALIDNAME);
    }
    let path = match file_name(cpu, name) {
        Ok(p) => p,
        Err(e) => return Some(e),
    };
    if open_doc(cpu, &path).is_some() {
        return Some(STG_E_SHAREVIOLATION);
    }
    let data = match files::read_whole(cpu, &path) {
        Ok(d) => d,
        Err(_) => return Some(STG_E_FILENOTFOUND),
    };
    let Some(elements) = decode(&data) else {
        return Some(STG_E_FILEALREADYEXISTS);
    };
    let doc_id = new_doc(cpu, path, mode, elements);
    if mode & STGM_DELETEONRELEASE != 0 {
        let path = doc(cpu, doc_id).path.clone();
        let _ = files::delete_file(cpu, &path);
        doc(cpu, doc_id).path.clear();
    }
    match new_object(cpu, STORAGE, doc_id, 0, mode) {
        Some(at) => {
            cpu.bus.guest_write_32(out, at);
            Some(S_OK)
        }
        None => {
            storages(cpu).docs[doc_id as usize] = None;
            Some(E_OUTOFMEMORY)
        }
    }
}

/// StgIsStorageFile(name): S_OK for a compound file, S_FALSE for another.
pub fn stg_is_storage_file(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        return Some(STG_E_INVALIDNAME);
    }
    let path = match file_name(cpu, args[0]) {
        Ok(p) => p,
        Err(e) => return Some(e),
    };
    match files::read_whole(cpu, &path) {
        Ok(data) if data.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]) => {
            Some(S_OK)
        }
        Ok(_) => Some(S_FALSE),
        Err(_) => Some(STG_E_FILENOTFOUND),
    }
}

// IUnknown of the three.

fn query(cpu: &mut Cpu, args: &[u32], kind: u32) -> Option<u32> {
    let &[this, iid, out] = args else {
        unreachable!()
    };
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    let Some(id) = object_of(cpu, this, kind) else {
        return Some(E_INVALIDARG);
    };
    let ok = match Guid::read(cpu, iid) {
        Some(g) if g == com::IID_IUNKNOWN => true,
        Some(g) => match kind {
            STORAGE => g == IID_ISTORAGE,
            STREAM => g == IID_ISTREAM || g == IID_ISEQUENTIALSTREAM,
            _ => g == IID_IENUMSTATSTG,
        },
        None => false,
    };
    if !ok {
        com::write_out(cpu, out, 0);
        return Some(E_NOINTERFACE);
    }
    obj(cpu, id).refs += 1;
    com::write_out(cpu, out, this);
    Some(S_OK)
}

fn add_ref(cpu: &mut Cpu, args: &[u32], kind: u32) -> Option<u32> {
    let Some(id) = object_of(cpu, args[0], kind) else {
        return Some(0);
    };
    let o = obj(cpu, id);
    o.refs += 1;
    Some(o.refs)
}

/// Release: with the last reference the object goes; the root storage's
/// writes the file (unless the docfile is transacted) and reverts the
/// docfile's other objects.
fn release(cpu: &mut Cpu, args: &[u32], kind: u32) -> Option<u32> {
    let this = args[0];
    let Some(id) = object_of(cpu, this, kind) else {
        return Some(0);
    };
    let o = obj(cpu, id);
    o.refs = o.refs.saturating_sub(1);
    let refs = o.refs;
    if refs > 0 {
        return Some(0);
    }
    let o = storages(cpu).objects[id as usize]
        .take()
        .expect("an object of the table");
    com::free_interface(cpu, this);
    let d = doc(cpu, o.doc);
    d.objects = d.objects.saturating_sub(1);
    if o.kind == STORAGE && o.element == 0 && !d.closed {
        let write = d.dirty && writable(d.mode) && d.mode & STGM_TRANSACTED == 0;
        d.closed = true;
        if write {
            let _ = save(cpu, o.doc);
        }
    }
    let d = doc(cpu, o.doc);
    if d.closed && d.objects == 0 {
        storages(cpu).docs[o.doc as usize] = None;
    }
    Some(0)
}

pub fn storage_query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    query(cpu, args, STORAGE)
}

pub fn storage_add_ref(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    add_ref(cpu, args, STORAGE)
}

pub fn storage_release(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    release(cpu, args, STORAGE)
}

pub fn stream_query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    query(cpu, args, STREAM)
}

pub fn stream_add_ref(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    add_ref(cpu, args, STREAM)
}

pub fn stream_release(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    release(cpu, args, STREAM)
}

pub fn enum_query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    query(cpu, args, ENUM)
}

pub fn enum_add_ref(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    add_ref(cpu, args, ENUM)
}

pub fn enum_release(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    release(cpu, args, ENUM)
}

// IStorage.

/// CreateStream (`storage` false) and CreateStorage: an element named
/// `name` in the storage, replacing one of that name with STGM_CREATE.
fn create_element(cpu: &mut Cpu, args: &[u32], storage: bool) -> Option<u32> {
    let &[this, name, mode, _, _, out] = args else {
        unreachable!()
    };
    if out == 0 || !cpu.bus.guest_probe(out, 4, true) {
        return Some(STG_E_INVALIDPOINTER);
    }
    cpu.bus.guest_write_32(out, 0);
    let id = match live(cpu, this, STORAGE) {
        Ok(id) => id,
        Err(e) => return Some(e),
    };
    let name = match name_at(cpu, name) {
        Ok(n) => n,
        Err(e) => return Some(e),
    };
    if mode & STGM_SHARE != STGM_SHARE_EXCLUSIVE || mode & STGM_ACCESS == 3 {
        return Some(STG_E_INVALIDFLAG);
    }
    let (doc_id, parent, own) = (obj(cpu, id).doc, obj(cpu, id).element, obj(cpu, id).mode);
    if !writable(own) {
        return Some(STG_E_ACCESSDENIED);
    }
    if let Some(old) = child(cpu, doc_id, parent, &name) {
        if mode & STGM_CREATE == 0 {
            return Some(STG_E_FILEALREADYEXISTS);
        }
        if is_open(cpu, doc_id, old) {
            return Some(STG_E_ACCESSDENIED);
        }
        destroy(cpu, doc_id, old);
    }
    let now = crate::win32::kernel32::time::file_time_now(cpu);
    let times = if storage { now } else { 0 };
    let d = doc(cpu, doc_id);
    d.elements.push(Element {
        name,
        parent,
        storage,
        clsid: vec![0; 16],
        created: times,
        modified: times,
        ..Element::default()
    });
    let index = d.elements.len() as u32 - 1;
    touched(cpu, doc_id, parent);
    let kind = if storage { STORAGE } else { STREAM };
    match new_object(cpu, kind, doc_id, index, mode) {
        Some(at) => {
            cpu.bus.guest_write_32(out, at);
            Some(S_OK)
        }
        None => Some(E_OUTOFMEMORY),
    }
}

/// IStorage::CreateStream(this, name, mode, reserved, reserved, out).
pub fn create_stream(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    create_element(cpu, args, false)
}

/// IStorage::CreateStorage(this, name, mode, reserved, reserved, out).
pub fn create_storage(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    create_element(cpu, args, true)
}

/// OpenStream and OpenStorage: the element named `name`, which has to be
/// opened exclusively and not be open already, as on Windows.
fn open_element(
    cpu: &mut Cpu,
    this: u32,
    name: u32,
    mode: u32,
    out: u32,
    storage: bool,
) -> Option<u32> {
    if out == 0 || !cpu.bus.guest_probe(out, 4, true) {
        return Some(STG_E_INVALIDPOINTER);
    }
    cpu.bus.guest_write_32(out, 0);
    let id = match live(cpu, this, STORAGE) {
        Ok(id) => id,
        Err(e) => return Some(e),
    };
    let name = match name_at(cpu, name) {
        Ok(n) => n,
        Err(e) => return Some(e),
    };
    if mode & STGM_SHARE != STGM_SHARE_EXCLUSIVE
        || mode & STGM_ACCESS == 3
        || mode & (STGM_CREATE | STGM_DELETEONRELEASE) != 0
    {
        return Some(STG_E_INVALIDFLAG);
    }
    let (doc_id, parent, own) = (obj(cpu, id).doc, obj(cpu, id).element, obj(cpu, id).mode);
    if writable(mode) && !writable(own) {
        return Some(STG_E_ACCESSDENIED);
    }
    let Some(index) = child(cpu, doc_id, parent, &name) else {
        return Some(STG_E_FILENOTFOUND);
    };
    if doc(cpu, doc_id).elements[index as usize].storage != storage {
        return Some(STG_E_FILENOTFOUND);
    }
    if is_open(cpu, doc_id, index) {
        return Some(STG_E_ACCESSDENIED);
    }
    let kind = if storage { STORAGE } else { STREAM };
    match new_object(cpu, kind, doc_id, index, mode) {
        Some(at) => {
            cpu.bus.guest_write_32(out, at);
            Some(S_OK)
        }
        None => Some(E_OUTOFMEMORY),
    }
}

/// IStorage::OpenStream(this, name, reserved, mode, reserved, out).
pub fn open_stream(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, name, _, mode, _, out] = args else {
        unreachable!()
    };
    open_element(cpu, this, name, mode, out, false)
}

/// IStorage::OpenStorage(this, name, priority, mode, exclude, reserved,
/// out).
pub fn open_storage(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, name, _, mode, _, _, out] = args else {
        unreachable!()
    };
    open_element(cpu, this, name, mode, out, true)
}

/// Copy the element `from` of `src` with what is in it into the storage
/// `parent` of `dst`, under `name` (replacing an element of that name).
fn copy_into(cpu: &mut Cpu, src: u32, from: u32, dst: u32, parent: u32, name: Vec<u16>) {
    if let Some(old) = child(cpu, dst, parent, &name) {
        destroy(cpu, dst, old);
    }
    let mut e = doc(cpu, src).elements[from as usize].clone();
    e.name = name;
    e.parent = parent;
    let d = doc(cpu, dst);
    d.elements.push(e);
    let new = d.elements.len() as u32 - 1;
    d.dirty = true;
    let children: Vec<(u32, Vec<u16>)> = doc(cpu, src)
        .elements
        .iter()
        .enumerate()
        .filter(|&(i, c)| i != 0 && !c.gone && c.parent == from)
        .map(|(i, c)| (i as u32, c.name.clone()))
        .collect();
    for (c, n) in children {
        copy_into(cpu, src, c, dst, new, n);
    }
}

/// IStorage::CopyTo(this, excluded IIDs count, IIDs, excluded names,
/// target): the storage's elements, class and state bits copied into
/// another storage of these docfiles; one the program implements isn't
/// taken.
pub fn copy_to(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, _, _, _, target] = args else {
        unreachable!()
    };
    let id = match live(cpu, this, STORAGE) {
        Ok(id) => id,
        Err(e) => return Some(e),
    };
    if target == 0 {
        return Some(STG_E_INVALIDPOINTER);
    }
    let to = match live(cpu, target, STORAGE) {
        Ok(t) => t,
        Err(E_INVALIDARG) => return Some(STG_E_INVALIDFUNCTION),
        Err(e) => return Some(e),
    };
    let (src, from) = (obj(cpu, id).doc, obj(cpu, id).element);
    let (dst, parent) = (obj(cpu, to).doc, obj(cpu, to).element);
    if !writable(obj(cpu, to).mode) {
        return Some(STG_E_ACCESSDENIED);
    }
    // A storage copied into itself or a storage in it would copy its
    // own copies without end.
    if src == dst && within(cpu, dst, parent, from) {
        return Some(STG_E_ACCESSDENIED);
    }
    let e = doc(cpu, src).elements[from as usize].clone();
    let t = &mut doc(cpu, dst).elements[parent as usize];
    t.clsid = e.clsid;
    t.bits = e.bits;
    let children: Vec<(u32, Vec<u16>)> = doc(cpu, src)
        .elements
        .iter()
        .enumerate()
        .filter(|&(i, c)| i != 0 && !c.gone && c.parent == from)
        .map(|(i, c)| (i as u32, c.name.clone()))
        .collect();
    for (c, n) in children {
        copy_into(cpu, src, c, dst, parent, n);
    }
    touched(cpu, dst, parent);
    Some(S_OK)
}

/// IStorage::MoveElementTo(this, name, target, new name, flags):
/// STGMOVE_MOVE (0) moves, STGMOVE_COPY (1) copies, within these
/// docfiles.
pub fn move_element_to(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, name, target, new_name, flags] = args else {
        unreachable!()
    };
    let id = match live(cpu, this, STORAGE) {
        Ok(id) => id,
        Err(e) => return Some(e),
    };
    let to = match live(cpu, target, STORAGE) {
        Ok(t) => t,
        Err(E_INVALIDARG) => return Some(STG_E_INVALIDFUNCTION),
        Err(e) => return Some(e),
    };
    let (name, new_name) = match (name_at(cpu, name), name_at(cpu, new_name)) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => return Some(e),
    };
    if flags > 1 {
        return Some(STG_E_INVALIDFLAG);
    }
    let (src, parent) = (obj(cpu, id).doc, obj(cpu, id).element);
    let (dst, into) = (obj(cpu, to).doc, obj(cpu, to).element);
    if !writable(obj(cpu, to).mode) || (flags == 0 && !writable(obj(cpu, id).mode)) {
        return Some(STG_E_ACCESSDENIED);
    }
    let Some(from) = child(cpu, src, parent, &name) else {
        return Some(STG_E_FILENOTFOUND);
    };
    if is_open(cpu, src, from) {
        return Some(STG_E_ACCESSDENIED);
    }
    // An element moved or copied into itself or a storage in it would
    // copy its own copies without end.
    if src == dst && within(cpu, dst, into, from) {
        return Some(STG_E_ACCESSDENIED);
    }
    copy_into(cpu, src, from, dst, into, new_name);
    touched(cpu, dst, into);
    if flags == 0 {
        destroy(cpu, src, from);
        touched(cpu, src, parent);
    }
    Some(S_OK)
}

/// IStorage::Commit(this, flags): the root storage writes the file.
pub fn commit(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = match live(cpu, args[0], STORAGE) {
        Ok(id) => id,
        Err(e) => return Some(e),
    };
    let (doc_id, index) = (obj(cpu, id).doc, obj(cpu, id).element);
    if index != 0 || !writable(obj(cpu, id).mode) || !doc(cpu, doc_id).dirty {
        return Some(S_OK);
    }
    match save(cpu, doc_id) {
        Ok(()) => Some(S_OK),
        Err(e) => Some(e),
    }
}

/// IStorage::Revert(this): a transacted root storage goes back to the
/// file as it was last written; others have nothing to undo. The
/// elements of the docfile's other objects are gone from then on, so
/// those objects answer STG_E_REVERTED, as on Windows: the file's
/// elements come after them, and the root takes the file's root's
/// class, bits and times.
pub fn revert(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = match live(cpu, args[0], STORAGE) {
        Ok(id) => id,
        Err(e) => return Some(e),
    };
    let (doc_id, index) = (obj(cpu, id).doc, obj(cpu, id).element);
    let d = doc(cpu, doc_id);
    if index != 0 || d.mode & STGM_TRANSACTED == 0 || d.path.is_empty() {
        return Some(S_OK);
    }
    let path = d.path.clone();
    if let Some(mut read) = files::read_whole(cpu, &path)
        .ok()
        .and_then(|data| decode(&data))
    {
        let d = doc(cpu, doc_id);
        for e in d.elements.iter_mut().skip(1) {
            e.gone = true;
            e.data = Vec::new();
        }
        // The file's elements after the old ones: their parents move
        // with them, but the root stays at 0.
        let offset = d.elements.len() as u32 - 1;
        for e in read.iter_mut().skip(1) {
            if e.parent != 0 {
                e.parent += offset;
            }
        }
        let mut read = read.into_iter();
        if let Some(root) = read.next() {
            let name = std::mem::take(&mut d.elements[0].name);
            d.elements[0] = Element { name, ..root };
        }
        d.elements.extend(read);
        d.dirty = false;
    }
    Some(S_OK)
}

/// IStorage::EnumElements(this, reserved, reserved, reserved, out): an
/// enumerator of the storage's elements as they are now.
pub fn enum_elements(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, _, _, _, out] = args else {
        unreachable!()
    };
    if out == 0 || !cpu.bus.guest_probe(out, 4, true) {
        return Some(STG_E_INVALIDPOINTER);
    }
    cpu.bus.guest_write_32(out, 0);
    let id = match live(cpu, this, STORAGE) {
        Ok(id) => id,
        Err(e) => return Some(e),
    };
    let (doc_id, parent) = (obj(cpu, id).doc, obj(cpu, id).element);
    let list = children_of(cpu, doc_id, parent);
    let Some(at) = new_object(cpu, ENUM, doc_id, parent, 0) else {
        return Some(E_OUTOFMEMORY);
    };
    if let Some(e) = object_of(cpu, at, ENUM) {
        obj(cpu, e).list = list;
    }
    cpu.bus.guest_write_32(out, at);
    Some(S_OK)
}

fn children_of(cpu: &mut Cpu, doc_id: u32, parent: u32) -> Vec<u32> {
    doc(cpu, doc_id)
        .elements
        .iter()
        .enumerate()
        .filter(|&(i, e)| i != 0 && !e.gone && e.parent == parent)
        .map(|(i, _)| i as u32)
        .collect()
}

/// IStorage::DestroyElement(this, name).
pub fn destroy_element(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = match live(cpu, args[0], STORAGE) {
        Ok(id) => id,
        Err(e) => return Some(e),
    };
    let name = match name_at(cpu, args[1]) {
        Ok(n) => n,
        Err(e) => return Some(e),
    };
    let (doc_id, parent) = (obj(cpu, id).doc, obj(cpu, id).element);
    if !writable(obj(cpu, id).mode) {
        return Some(STG_E_ACCESSDENIED);
    }
    let Some(index) = child(cpu, doc_id, parent, &name) else {
        return Some(STG_E_FILENOTFOUND);
    };
    destroy(cpu, doc_id, index);
    touched(cpu, doc_id, parent);
    Some(S_OK)
}

/// IStorage::RenameElement(this, old, new).
pub fn rename_element(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = match live(cpu, args[0], STORAGE) {
        Ok(id) => id,
        Err(e) => return Some(e),
    };
    let (old, new) = match (name_at(cpu, args[1]), name_at(cpu, args[2])) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => return Some(e),
    };
    let (doc_id, parent) = (obj(cpu, id).doc, obj(cpu, id).element);
    if !writable(obj(cpu, id).mode) {
        return Some(STG_E_ACCESSDENIED);
    }
    if child(cpu, doc_id, parent, &new).is_some() {
        return Some(STG_E_FILEALREADYEXISTS);
    }
    let Some(index) = child(cpu, doc_id, parent, &old) else {
        return Some(STG_E_FILENOTFOUND);
    };
    if is_open(cpu, doc_id, index) {
        return Some(STG_E_ACCESSDENIED);
    }
    element(cpu, doc_id, index).name = new;
    touched(cpu, doc_id, parent);
    Some(S_OK)
}

/// IStorage::SetElementTimes(this, name, created, accessed, modified):
/// a storage's times (a stream has none); a NULL name is the storage's
/// own.
pub fn set_element_times(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, name, created, _, modified] = args else {
        unreachable!()
    };
    let id = match live(cpu, this, STORAGE) {
        Ok(id) => id,
        Err(e) => return Some(e),
    };
    let (doc_id, parent) = (obj(cpu, id).doc, obj(cpu, id).element);
    let index = if name == 0 {
        parent
    } else {
        let name = match name_at(cpu, name) {
            Ok(n) => n,
            Err(e) => return Some(e),
        };
        match child(cpu, doc_id, parent, &name) {
            Some(i) => i,
            None => return Some(STG_E_FILENOTFOUND),
        }
    };
    let read = |cpu: &mut Cpu, at: u32| {
        (at != 0 && cpu.bus.guest_probe(at, 8, false)).then(|| {
            cpu.bus.guest_read_32(at) as u64 | (cpu.bus.guest_read_32(at + 4) as u64) << 32
        })
    };
    let (c, m) = (read(cpu, created), read(cpu, modified));
    let e = element(cpu, doc_id, index);
    if e.storage {
        if let Some(c) = c {
            e.created = c;
        }
        if let Some(m) = m {
            e.modified = m;
        }
    }
    doc(cpu, doc_id).dirty = true;
    Some(S_OK)
}

/// IStorage::SetClass(this, clsid).
pub fn set_class(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = match live(cpu, args[0], STORAGE) {
        Ok(id) => id,
        Err(e) => return Some(e),
    };
    let Some(clsid) = Guid::read(cpu, args[1]) else {
        return Some(STG_E_INVALIDPOINTER);
    };
    let (doc_id, index) = (obj(cpu, id).doc, obj(cpu, id).element);
    if !writable(obj(cpu, id).mode) {
        return Some(STG_E_ACCESSDENIED);
    }
    element(cpu, doc_id, index).clsid = clsid.0.to_vec();
    doc(cpu, doc_id).dirty = true;
    Some(S_OK)
}

/// IStorage::SetStateBits(this, bits, mask).
pub fn set_state_bits(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, bits, mask] = args else {
        unreachable!()
    };
    let id = match live(cpu, this, STORAGE) {
        Ok(id) => id,
        Err(e) => return Some(e),
    };
    let (doc_id, index) = (obj(cpu, id).doc, obj(cpu, id).element);
    if !writable(obj(cpu, id).mode) {
        return Some(STG_E_ACCESSDENIED);
    }
    let e = element(cpu, doc_id, index);
    e.bits = (e.bits & !mask) | (bits & mask);
    doc(cpu, doc_id).dirty = true;
    Some(S_OK)
}

/// IStorage::Stat(this, statstg, flag) and IStream::Stat.
fn stat(cpu: &mut Cpu, args: &[u32], kind: u32) -> Option<u32> {
    let &[this, out, flag] = args else {
        unreachable!()
    };
    let id = match live(cpu, this, kind) {
        Ok(id) => id,
        Err(e) => return Some(e),
    };
    if out == 0 || !cpu.bus.guest_probe(out, STATSTG_SIZE, true) {
        return Some(STG_E_INVALIDPOINTER);
    }
    let (doc_id, index, mode) = (obj(cpu, id).doc, obj(cpu, id).element, obj(cpu, id).mode);
    if !write_stat(cpu, doc_id, index, mode, out, flag) {
        return Some(E_OUTOFMEMORY);
    }
    Some(S_OK)
}

pub fn storage_stat(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    stat(cpu, args, STORAGE)
}

// IStream of a docfile's stream.

fn stream_parts(cpu: &mut Cpu, this: u32) -> Result<(u32, u32, u32), u32> {
    let id = live(cpu, this, STREAM)?;
    Ok((id, obj(cpu, id).doc, obj(cpu, id).element))
}

/// IStream::Read(this, buffer, count, read).
pub fn stream_read(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, buffer, count, read] = args else {
        unreachable!()
    };
    let (id, doc_id, index) = match stream_parts(cpu, this) {
        Ok(p) => p,
        Err(e) => return Some(e),
    };
    let position = obj(cpu, id).position;
    let data = &doc(cpu, doc_id).elements[index as usize].data;
    let start = (position as usize).min(data.len());
    let n = (data.len() - start).min(count as usize);
    let bytes = data[start..start + n].to_vec();
    if n > 0 && !cpu.bus.guest_probe(buffer, n, true) {
        return Some(STG_E_INVALIDPOINTER);
    }
    cpu.bus.guest_write_bytes(buffer, &bytes);
    obj(cpu, id).position = (start + n) as u64;
    if read != 0 {
        com::write_out(cpu, read, n as u32);
    }
    Some(S_OK)
}

/// IStream::Write(this, buffer, count, written).
pub fn stream_write(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, buffer, count, written] = args else {
        unreachable!()
    };
    let (id, doc_id, index) = match stream_parts(cpu, this) {
        Ok(p) => p,
        Err(e) => return Some(e),
    };
    if !writable(obj(cpu, id).mode) {
        return Some(STG_E_ACCESSDENIED);
    }
    if count > 0 && !cpu.bus.guest_probe(buffer, count as usize, false) {
        return Some(STG_E_INVALIDPOINTER);
    }
    // Seek takes any position; a write that would make the stream larger
    // than a docfile's stream is here fails.
    if obj(cpu, id).position.saturating_add(count as u64) > MAX_STREAM {
        return Some(STG_E_MEDIUMFULL);
    }
    let mut bytes = vec![0; count as usize];
    cpu.bus.guest_read_bytes(buffer, &mut bytes);
    let position = obj(cpu, id).position as usize;
    let data = &mut element(cpu, doc_id, index).data;
    if data.len() < position + bytes.len() {
        data.resize(position + bytes.len(), 0);
    }
    data[position..position + bytes.len()].copy_from_slice(&bytes);
    obj(cpu, id).position = (position + bytes.len()) as u64;
    doc(cpu, doc_id).dirty = true;
    if written != 0 {
        com::write_out(cpu, written, count);
    }
    Some(S_OK)
}

/// IStream::Seek(this, move (64 bits), origin, new position).
pub fn stream_seek(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, low, high, origin, out] = args else {
        unreachable!()
    };
    let (id, doc_id, index) = match stream_parts(cpu, this) {
        Ok(p) => p,
        Err(e) => return Some(e),
    };
    let by = (high as u64) << 32 | low as u64;
    let base = match origin {
        0 => 0,
        1 => obj(cpu, id).position,
        2 => doc(cpu, doc_id).elements[index as usize].data.len() as u64,
        _ => return Some(STG_E_INVALIDFUNCTION),
    };
    let new = base.wrapping_add(by);
    if (new as i64) < 0 {
        return Some(STG_E_INVALIDFUNCTION);
    }
    obj(cpu, id).position = new;
    if out != 0 && cpu.bus.guest_probe(out, 8, true) {
        cpu.bus.guest_write_32(out, new as u32);
        cpu.bus.guest_write_32(out + 4, (new >> 32) as u32);
    }
    Some(S_OK)
}

/// IStream::SetSize(this, size (64 bits)).
pub fn stream_set_size(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, low, high] = args else {
        unreachable!()
    };
    let (id, doc_id, index) = match stream_parts(cpu, this) {
        Ok(p) => p,
        Err(e) => return Some(e),
    };
    if !writable(obj(cpu, id).mode) {
        return Some(STG_E_ACCESSDENIED);
    }
    if high != 0 || low as u64 > MAX_STREAM {
        return Some(STG_E_INVALIDFUNCTION);
    }
    element(cpu, doc_id, index).data.resize(low as usize, 0);
    doc(cpu, doc_id).dirty = true;
    Some(S_OK)
}

/// The variables of a stream's CopyTo into a stream the program
/// implements: the block with the bytes and the out pointers.
const C_BLOCK: usize = 1;
const C_COUNT: usize = 2;
const C_READ: usize = 3;
const C_WRITTEN: usize = 4;

/// IStream::CopyTo(this, target, count (64 bits), read, written): from
/// the position on, into another stream; into one of these it is a copy
/// of bytes, into another the bytes go in one call of its Write.
pub fn stream_copy_to(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, target, low, high, read, written] = args else {
        unreachable!()
    };
    let (id, doc_id, index) = match stream_parts(cpu, this) {
        Ok(p) => p,
        Err(e) => return Some(e),
    };
    if target == 0 {
        return Some(STG_E_INVALIDPOINTER);
    }
    let want = (high as u64) << 32 | low as u64;
    let position = obj(cpu, id).position;
    let data = &doc(cpu, doc_id).elements[index as usize].data;
    let start = (position as usize).min(data.len());
    let n = ((data.len() - start) as u64).min(want) as usize;
    let bytes = data[start..start + n].to_vec();
    obj(cpu, id).position = (start + n) as u64;
    let report = |cpu: &mut Cpu, at: u32, value: u32| {
        if at != 0 && cpu.bus.guest_probe(at, 8, true) {
            cpu.bus.guest_write_32(at, value);
            cpu.bus.guest_write_32(at + 4, 0);
        }
    };
    if let Ok((to, to_doc, to_index)) = stream_parts(cpu, target) {
        if !writable(obj(cpu, to).mode) {
            return Some(STG_E_ACCESSDENIED);
        }
        if obj(cpu, to).position.saturating_add(n as u64) > MAX_STREAM {
            return Some(STG_E_MEDIUMFULL);
        }
        let at = obj(cpu, to).position as usize;
        let data = &mut element(cpu, to_doc, to_index).data;
        if data.len() < at + n {
            data.resize(at + n, 0);
        }
        data[at..at + n].copy_from_slice(&bytes);
        obj(cpu, to).position = (at + n) as u64;
        doc(cpu, to_doc).dirty = true;
        report(cpu, read, n as u32);
        report(cpu, written, n as u32);
        return Some(S_OK);
    }
    let Some(write) = super::method(cpu, target, 4) else {
        return Some(STG_E_INVALIDPOINTER);
    };
    let block = super::task_alloc(cpu, n as u32 + 4);
    if block == 0 {
        return Some(E_OUTOFMEMORY);
    }
    cpu.bus.guest_write_bytes(block, &bytes);
    cpu.bus.guest_write_32(block + n as u32, 0);
    let vars = vec![
        super::calls::STORAGE_COPY,
        block,
        n as u32,
        read,
        written,
        write,
        target,
    ];
    super::calls::start(cpu, vars, 0)
}

/// CopyTo's call of the target's Write, and what follows it.
pub fn copy_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let (block, n) = (t.var(C_BLOCK), t.var(C_COUNT));
    if t.step == 0 {
        let (write, target) = (t.var(5), t.var(6));
        return Next::call(write, &[target, block, n, block + n], 1);
    }
    let written = cpu.bus.guest_read_32(block + n);
    super::task_free(cpu, block);
    for (at, v) in [(t.var(C_READ), n), (t.var(C_WRITTEN), written)] {
        if at != 0 && cpu.bus.guest_probe(at, 8, true) {
            cpu.bus.guest_write_32(at, v);
            cpu.bus.guest_write_32(at + 4, 0);
        }
    }
    Next::Done(value)
}

/// IStream::Commit and Revert: a docfile's stream in direct mode has
/// nothing to do.
pub fn stream_nothing(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    match stream_parts(cpu, args[0]) {
        Ok(_) => Some(S_OK),
        Err(e) => Some(e),
    }
}

/// IStream::LockRegion and UnlockRegion: a docfile's streams don't lock.
pub fn stream_lock_region(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(STG_E_INVALIDFUNCTION)
}

pub fn stream_stat(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    stat(cpu, args, STREAM)
}

/// IStream::Clone(this, out): another stream of the element, at the same
/// position.
pub fn stream_clone(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, out] = args else { unreachable!() };
    if out == 0 || !cpu.bus.guest_probe(out, 4, true) {
        return Some(STG_E_INVALIDPOINTER);
    }
    let (id, doc_id, index) = match stream_parts(cpu, this) {
        Ok(p) => p,
        Err(e) => return Some(e),
    };
    let (mode, position) = (obj(cpu, id).mode, obj(cpu, id).position);
    let Some(at) = new_object(cpu, STREAM, doc_id, index, mode) else {
        return Some(E_OUTOFMEMORY);
    };
    if let Some(new) = object_of(cpu, at, STREAM) {
        obj(cpu, new).position = position;
    }
    cpu.bus.guest_write_32(out, at);
    Some(S_OK)
}

// IEnumSTATSTG.

/// IEnumSTATSTG::Next(this, count, out, fetched): S_FALSE when fewer
/// than `count` are left.
pub fn enum_next(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, count, out, fetched] = args else {
        unreachable!()
    };
    let Some(id) = object_of(cpu, this, ENUM) else {
        return Some(E_INVALIDARG);
    };
    if out == 0 || (count != 1 && fetched == 0) {
        return Some(STG_E_INVALIDPOINTER);
    }
    let doc_id = obj(cpu, id).doc;
    if doc(cpu, doc_id).closed {
        return Some(STG_E_REVERTED);
    }
    let mut n = 0;
    while n < count {
        let o = obj(cpu, id);
        let Some(&index) = o.list.get(o.at as usize) else {
            break;
        };
        o.at += 1;
        if doc(cpu, doc_id).elements[index as usize].gone {
            continue;
        }
        let at = out + n * STATSTG_SIZE as u32;
        if !cpu.bus.guest_probe(at, STATSTG_SIZE, true) {
            return Some(STG_E_INVALIDPOINTER);
        }
        if !write_stat(cpu, doc_id, index, 0, at, 0) {
            return Some(E_OUTOFMEMORY);
        }
        n += 1;
    }
    if fetched != 0 {
        com::write_out(cpu, fetched, n);
    }
    Some(if n == count { S_OK } else { S_FALSE })
}

/// IEnumSTATSTG::Skip(this, count).
pub fn enum_skip(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = object_of(cpu, args[0], ENUM) else {
        return Some(E_INVALIDARG);
    };
    let o = obj(cpu, id);
    let left = o.list.len() as u32 - o.at.min(o.list.len() as u32);
    o.at += args[1].min(left);
    Some(if args[1] <= left { S_OK } else { S_FALSE })
}

/// IEnumSTATSTG::Reset(this).
pub fn enum_reset(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = object_of(cpu, args[0], ENUM) else {
        return Some(E_INVALIDARG);
    };
    obj(cpu, id).at = 0;
    Some(S_OK)
}

/// IEnumSTATSTG::Clone(this, out): another enumerator at the same place.
pub fn enum_clone(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, out] = args else { unreachable!() };
    let Some(id) = object_of(cpu, this, ENUM) else {
        return Some(E_INVALIDARG);
    };
    if out == 0 || !cpu.bus.guest_probe(out, 4, true) {
        return Some(STG_E_INVALIDPOINTER);
    }
    let o = obj(cpu, id).clone();
    let Some(at) = new_object(cpu, ENUM, o.doc, o.element, 0) else {
        return Some(E_OUTOFMEMORY);
    };
    if let Some(new) = object_of(cpu, at, ENUM) {
        obj(cpu, new).list = o.list;
        obj(cpu, new).at = o.at;
    }
    cpu.bus.guest_write_32(out, at);
    Some(S_OK)
}

crate::state_fields!(Element {
    name,
    parent,
    storage,
    clsid,
    bits,
    data,
    created,
    modified,
    gone
});
crate::state_fields!(Doc {
    pid,
    path,
    elements,
    mode,
    dirty,
    closed,
    objects
});
crate::state_fields!(Object {
    pid,
    kind,
    doc,
    element,
    refs,
    mode,
    position,
    list,
    at
});
crate::state_fields!(Storages { docs, objects });

#[cfg(test)]
mod tests {
    use super::*;

    fn el(name: &str, parent: u32, storage: bool, data: &[u8]) -> Element {
        Element {
            name: name.encode_utf16().collect(),
            parent,
            storage,
            clsid: vec![0; 16],
            data: data.to_vec(),
            created: if storage { 0x01C0_0000_0000_0000 } else { 0 },
            modified: if storage { 0x01C0_0000_0000_0000 } else { 0 },
            ..Element::default()
        }
    }

    #[test]
    fn a_docfile_reads_back_as_it_was_written() {
        let mut root = el("", 0, true, &[]);
        root.clsid = (1..=16).collect();
        root.bits = 7;
        let elements = vec![
            root,
            el("Contents", 0, true, &[]),
            el("\u{5}SummaryInformation", 0, false, b"summary"),
            el("Big", 1, false, &[0x5A; 5000]),
            el("Small", 1, false, b"x"),
        ];
        let bytes = encode(&elements).expect("written");
        assert_eq!(
            &bytes[..8],
            &[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]
        );
        let read = decode(&bytes).expect("a compound file");
        let names = |v: &[Element]| -> Vec<(String, u32, bool, Vec<u8>)> {
            let mut l: Vec<_> = v
                .iter()
                .skip(1)
                .map(|e| {
                    let parent = String::from_utf16_lossy(&v[e.parent as usize].name);
                    (
                        format!("{}/{}", parent, String::from_utf16_lossy(&e.name)),
                        e.created as u32,
                        e.storage,
                        e.data.clone(),
                    )
                })
                .collect();
            l.sort();
            l.into_iter().map(|(n, c, s, d)| (n, c, s, d)).collect()
        };
        assert_eq!(names(&read), names(&elements));
        assert_eq!(read[0].clsid, elements[0].clsid);
        assert_eq!(read[0].bits, 7);
        // The same tree writes the same bytes: no host time goes in.
        assert_eq!(encode(&elements).expect("written"), bytes);
    }

    #[test]
    fn names_compare_without_case() {
        let a: Vec<u16> = "Contents".encode_utf16().collect();
        let b: Vec<u16> = "CONTENTS".encode_utf16().collect();
        assert!(same_name(&a, &b));
        assert!(!same_name(&a, &b[1..]));
    }
}
