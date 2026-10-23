//! Shell links: the ShellLink class (CLSID_ShellLink), whose objects
//! setup programs make with CoCreateInstance, fill through IShellLinkA
//! and save as .lnk files through IPersistFile, as Start Menu items and
//! desktop shortcuts. Windows 95 has no IShellLinkW.
//!
//! A .lnk file is in the format Microsoft documents as [MS-SHLLINK]: a
//! header, the target's item ID list, where it is on its volume (the
//! link information), then the strings (description, relative path,
//! working directory, arguments, icon), ANSI as Windows 95 writes them,
//! and the end of the extra data blocks. Reading takes Unicode strings
//! too, as later Windows writes them.

use super::pidl;
use crate::cpu::Cpu;
use crate::win32::com::{
    self, E_INVALIDARG, E_NOINTERFACE, E_OUTOFMEMORY, E_POINTER, Guid, IID_IUNKNOWN, S_FALSE, S_OK,
};
use crate::win32::kernel32::{self, files};
use crate::win32::user32::{current_pid, string_at};

pub const CLSID_SHELL_LINK: Guid = Guid::new(0x0002_1401, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_ISHELLLINKA: Guid = Guid::new(0x0002_14EE, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_IPERSISTFILE: Guid = Guid::new(0x0000_010B, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_IPERSIST: Guid = Guid::new(0x0000_010C, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);

const E_FAIL: u32 = 0x8000_4005;
const SW_SHOWNORMAL: u32 = 1;
const SLGP_UNCPRIORITY: u32 = 2;
const MAX_PATH: u32 = 260;

/// The vtables of a link's interfaces, by their places in SHELL32's.
pub const SHELL_LINK: usize = 0;
pub const PERSIST_FILE: usize = 1;

/// A shell link object.
#[derive(Clone, Debug, Default)]
pub struct Link {
    pub pid: u32,
    pub refs: u32,
    /// Its interfaces' blocks: IShellLinkA's and IPersistFile's, 0
    /// before the program asks for that one.
    pub interfaces: Vec<u32>,
    pub data: LinkData,
    /// The file it was last loaded from or saved to.
    pub file: Option<String>,
}

/// What a link holds, as its file has it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LinkData {
    /// The target's item ID list (pidl.rs), its end included; empty
    /// for none.
    pub list: Vec<u8>,
    pub path: String,
    pub description: String,
    pub relative: String,
    pub directory: String,
    pub arguments: String,
    pub icon: String,
    pub icon_index: u32,
    pub show: u32,
    pub hotkey: u16,
    /// The target's attributes and size, as the header has them.
    pub attributes: u32,
    pub size: u32,
}

crate::state_fields!(Link {
    pid,
    refs,
    interfaces,
    data,
    file
});
crate::state_fields!(LinkData {
    list,
    path,
    description,
    relative,
    directory,
    arguments,
    icon,
    icon_index,
    show,
    hotkey,
    attributes,
    size
});

fn links(cpu: &mut Cpu) -> &mut Vec<Option<Link>> {
    &mut cpu.bus.win32.shell.links
}

/// The class's create: a new link, as the interface `iid` at `out`.
pub fn create(cpu: &mut Cpu, iid: &Guid, out: u32) -> u32 {
    let vtable = match interface_for(iid) {
        Some(v) => v,
        None => return E_NOINTERFACE,
    };
    let link = Link {
        pid: current_pid(cpu),
        refs: 0,
        interfaces: vec![0, 0],
        data: LinkData {
            show: SW_SHOWNORMAL,
            ..LinkData::default()
        },
        file: None,
    };
    let list = links(cpu);
    let id = match list.iter().position(Option::is_none) {
        Some(i) => {
            list[i] = Some(link);
            i
        }
        None => {
            list.push(Some(link));
            list.len() - 1
        }
    };
    match interface(cpu, id, vtable) {
        Some(this) => {
            com::write_out(cpu, out, this);
            S_OK
        }
        None => {
            links(cpu)[id] = None;
            E_OUTOFMEMORY
        }
    }
}

/// The vtable an interface asked for is, if the object has it.
fn interface_for(iid: &Guid) -> Option<usize> {
    if *iid == IID_IUNKNOWN || *iid == IID_ISHELLLINKA {
        Some(SHELL_LINK)
    } else if *iid == IID_IPERSISTFILE || *iid == IID_IPERSIST {
        Some(PERSIST_FILE)
    } else {
        None
    }
}

/// The link `id`'s interface of the vtable `vtable`, made if it wasn't,
/// with a reference added.
fn interface(cpu: &mut Cpu, id: usize, vtable: usize) -> Option<u32> {
    let have = links(cpu)
        .get(id)?
        .as_ref()?
        .interfaces
        .get(vtable)
        .copied()?;
    let this = if have != 0 {
        have
    } else {
        let this = com::new_interface(cpu, super::dll_index(), vtable, id as u32, 0)?;
        links(cpu)[id].as_mut()?.interfaces[vtable] = this;
        this
    };
    links(cpu)[id].as_mut()?.refs += 1;
    Some(this)
}

/// The link the interface `this` is of.
fn link_id(cpu: &mut Cpu, this: u32) -> Option<usize> {
    let (vtable, id) = com::interface_of(cpu, super::dll_index(), this)?;
    if vtable != SHELL_LINK && vtable != PERSIST_FILE {
        return None;
    }
    let id = id as usize;
    links(cpu).get(id)?.as_ref()?;
    Some(id)
}

fn data(cpu: &mut Cpu, id: usize) -> &mut LinkData {
    &mut links(cpu)[id].as_mut().expect("a live link").data
}

/// IUnknown::QueryInterface of either interface.
pub fn query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, iid, out] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let Some(id) = link_id(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if out == 0 {
        return Some(E_POINTER);
    }
    let vtable = Guid::read(cpu, iid).and_then(|g| interface_for(&g));
    match vtable.and_then(|v| interface(cpu, id, v)) {
        Some(it) => {
            com::write_out(cpu, out, it);
            Some(S_OK)
        }
        None => {
            com::write_out(cpu, out, 0);
            Some(E_NOINTERFACE)
        }
    }
}

pub fn add_ref(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = link_id(cpu, args[0]) else {
        return Some(0);
    };
    let link = links(cpu)[id].as_mut()?;
    link.refs += 1;
    Some(link.refs)
}

pub fn release(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = link_id(cpu, args[0]) else {
        return Some(0);
    };
    let link = links(cpu)[id].as_mut()?;
    link.refs = link.refs.saturating_sub(1);
    let refs = link.refs;
    if refs == 0 {
        let gone = links(cpu)[id].take()?;
        for this in gone.interfaces.into_iter().filter(|&i| i != 0) {
            com::free_interface(cpu, this);
        }
    }
    Some(refs)
}

/// The process `pid` ended: its links go with its heap.
pub fn process_ended(cpu: &mut Cpu, pid: u32) {
    for slot in links(cpu).iter_mut() {
        if slot.as_ref().is_some_and(|l| l.pid == pid) {
            *slot = None;
        }
    }
}

/// Copy `text` to the buffer `at` of `size` bytes, cut to fit.
fn put(cpu: &mut Cpu, at: u32, size: u32, text: &str) -> bool {
    if at == 0 || size == 0 {
        return false;
    }
    let mut bytes = crate::dosstr::to_bytes(text);
    bytes.truncate(size as usize - 1);
    bytes.push(0);
    if !cpu.bus.guest_probe(at, bytes.len(), true) {
        return false;
    }
    cpu.bus.guest_write_bytes(at, &bytes);
    true
}

/// The string at `at`, empty for NULL.
fn text(cpu: &mut Cpu, at: u32) -> Option<String> {
    if at == 0 {
        return Some(String::new());
    }
    string_at(cpu, at)
}

/// A getter of a string: (this, buffer, size).
fn get_string(cpu: &mut Cpu, args: &[u32], field: fn(&LinkData) -> &String) -> Option<u32> {
    let Some(id) = link_id(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let value = field(data(cpu, id)).clone();
    put(cpu, args[1], args[2], &value);
    Some(S_OK)
}

/// A setter of a string: (this, string).
fn set_string(cpu: &mut Cpu, args: &[u32], field: fn(&mut LinkData) -> &mut String) -> Option<u32> {
    let Some(id) = link_id(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let Some(value) = text(cpu, args[1]) else {
        return Some(E_INVALIDARG);
    };
    *field(data(cpu, id)) = value;
    Some(S_OK)
}

pub fn get_description(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_string(cpu, args, |d| &d.description)
}

pub fn set_description(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    set_string(cpu, args, |d| &mut d.description)
}

pub fn get_working_directory(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_string(cpu, args, |d| &d.directory)
}

pub fn set_working_directory(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    set_string(cpu, args, |d| &mut d.directory)
}

pub fn get_arguments(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_string(cpu, args, |d| &d.arguments)
}

pub fn set_arguments(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    set_string(cpu, args, |d| &mut d.arguments)
}

/// SetRelativePath(this, path, reserved).
pub fn set_relative_path(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    set_string(cpu, args, |d| &mut d.relative)
}

/// GetHotkey(this, out).
pub fn get_hotkey(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = link_id(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let hotkey = data(cpu, id).hotkey;
    if args[1] == 0 || !cpu.bus.guest_probe(args[1], 2, true) {
        return Some(E_INVALIDARG);
    }
    cpu.bus.guest_write_16(args[1], hotkey);
    Some(S_OK)
}

pub fn set_hotkey(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = link_id(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    data(cpu, id).hotkey = args[1] as u16;
    Some(S_OK)
}

/// GetShowCmd(this, out).
pub fn get_show_cmd(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = link_id(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let show = data(cpu, id).show;
    if !com::write_out(cpu, args[1], show) {
        return Some(E_INVALIDARG);
    }
    Some(S_OK)
}

pub fn set_show_cmd(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = link_id(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    data(cpu, id).show = args[1];
    Some(S_OK)
}

/// GetIconLocation(this, buffer, size, index).
pub fn get_icon_location(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = link_id(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let (icon, index) = {
        let d = data(cpu, id);
        (d.icon.clone(), d.icon_index)
    };
    put(cpu, args[1], args[2], &icon);
    com::write_out(cpu, args[3], index);
    Some(S_OK)
}

/// SetIconLocation(this, path, index).
pub fn set_icon_location(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = link_id(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let Some(icon) = text(cpu, args[1]) else {
        return Some(E_INVALIDARG);
    };
    let d = data(cpu, id);
    d.icon = icon;
    d.icon_index = args[2];
    Some(S_OK)
}

/// GetIDList(this, out): a copy of the target's list, or NULL and
/// S_FALSE without one.
pub fn get_id_list(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = link_id(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    if args[1] == 0 {
        return Some(E_POINTER);
    }
    let list = data(cpu, id).list.clone();
    if list.is_empty() {
        com::write_out(cpu, args[1], 0);
        return Some(S_FALSE);
    }
    let at = pidl::alloc(cpu, &list);
    com::write_out(cpu, args[1], at);
    Some(if at == 0 { E_OUTOFMEMORY } else { S_OK })
}

/// SetIDList(this, list): the target, its path from the list.
pub fn set_id_list(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = link_id(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let Some(items) = (args[1] != 0).then(|| pidl::read(cpu, args[1])).flatten() else {
        return Some(E_INVALIDARG);
    };
    let path = super::il::path_of(cpu, args[1]).unwrap_or_default();
    let d = data(cpu, id);
    d.list = pidl::encode(&items);
    d.path = path;
    Some(S_OK)
}

/// SetPath(this, path): the target by its path. A name without a
/// directory is looked for where SearchPathA looks. S_FALSE when the
/// target isn't there, as Windows 9x returns.
pub fn set_path(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = link_id(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let Some(given) = text(cpu, args[1]) else {
        return Some(E_INVALIDARG);
    };
    let given = given.trim_matches('"').to_string();
    if given.is_empty() {
        let d = data(cpu, id);
        d.path.clear();
        d.list.clear();
        return Some(S_OK);
    }
    let found = if given.contains(['\\', ':']) {
        None
    } else {
        kernel32::search_file(cpu, &given)
    };
    let full = found.unwrap_or_else(|| files::full_path(cpu, &given));
    let there = files::attributes(cpu, &full);
    let list = pidl::from_path(cpu, &full, true)
        .map(|items| pidl::encode(&items))
        .unwrap_or_default();
    let d = data(cpu, id);
    d.path = full;
    d.list = list;
    match there {
        Ok(attributes) => {
            d.attributes = attributes;
            Some(S_OK)
        }
        Err(_) => Some(S_FALSE),
    }
}

/// GetPath(this, buffer, size, find data, flags): the target's path,
/// or an empty string and S_FALSE without one.
pub fn get_path(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, buffer, size, find, flags] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let Some(id) = link_id(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    let _ = flags & SLGP_UNCPRIORITY;
    let path = data(cpu, id).path.clone();
    put(cpu, buffer, size, &path);
    if find != 0 && cpu.bus.guest_probe(find, 320, true) {
        cpu.bus.guest_fill(find, 320, 0);
        if !path.is_empty() {
            let attributes = files::attributes(cpu, &path).unwrap_or(0);
            cpu.bus.guest_write_32(find, attributes);
            let name = path.rsplit('\\').next().unwrap_or("");
            put(cpu, find + 44, MAX_PATH, name);
        }
    }
    Some(if path.is_empty() { S_FALSE } else { S_OK })
}

/// Resolve(this, hwnd, flags): the target is taken as it is.
pub fn resolve(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(match link_id(cpu, args[0]) {
        Some(_) => S_OK,
        None => E_INVALIDARG,
    })
}

/// IPersist::GetClassID(this, out).
pub fn get_class_id(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if link_id(cpu, args[0]).is_none() {
        return Some(E_INVALIDARG);
    }
    if args[1] == 0 || !CLSID_SHELL_LINK.write(cpu, args[1]) {
        return Some(E_POINTER);
    }
    Some(S_OK)
}

/// IsDirty(this): links are saved whenever asked.
pub fn is_dirty(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(match link_id(cpu, args[0]) {
        Some(_) => S_OK,
        None => E_INVALIDARG,
    })
}

/// The Unicode string at `at` as ANSI, or None.
fn wide(cpu: &mut Cpu, at: u32) -> Option<String> {
    if at == 0 {
        return None;
    }
    let mut units = Vec::new();
    loop {
        let p = at + 2 * units.len() as u32;
        if units.len() > 0x8000 || !cpu.bus.guest_probe(p, 2, false) {
            return None;
        }
        match cpu.bus.guest_read_16(p) {
            0 => break,
            c => units.push(c),
        }
    }
    Some(crate::dosstr::from_bytes(&wide_to_ansi(&units)))
}

fn wide_to_ansi(units: &[u16]) -> Vec<u8> {
    units
        .iter()
        .map(|&c| if c < 0x100 { c as u8 } else { b'?' })
        .collect()
}

/// IPersistFile::Load(this, file, mode).
pub fn load(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = link_id(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let Some(file) = wide(cpu, args[1]) else {
        return Some(E_INVALIDARG);
    };
    let bytes = match kernel32::read_path(cpu, &file) {
        Ok((bytes, ..)) => bytes,
        Err(code) => return Some(hresult(code)),
    };
    let Some(loaded) = parse(&bytes) else {
        return Some(E_FAIL);
    };
    let link = links(cpu)[id].as_mut()?;
    link.data = loaded;
    link.file = Some(file);
    Some(S_OK)
}

/// HRESULT_FROM_WIN32.
fn hresult(code: u32) -> u32 {
    if code == 0 {
        E_FAIL
    } else {
        0x8007_0000 | (code & 0xFFFF)
    }
}

/// IPersistFile::Save(this, file, remember): to `file`, or to the file
/// it was loaded from for NULL.
pub fn save(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = link_id(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let file = match wide(cpu, args[1]) {
        Some(file) => file,
        None => match links(cpu)[id].as_ref()?.file.clone() {
            Some(file) => file,
            None => return Some(E_INVALIDARG),
        },
    };
    let mut d = data(cpu, id).clone();
    if !d.path.is_empty()
        && let Ok(attributes) = files::attributes(cpu, &d.path)
    {
        d.attributes = attributes;
        d.size = kernel32::read_path(cpu, &d.path).map_or(0, |(b, ..)| b.len() as u32);
    }
    let bytes = write(&d);
    if let Err(code) = files::write_whole(cpu, &file, &bytes) {
        return Some(hresult(code));
    }
    if args[2] != 0 {
        links(cpu)[id].as_mut()?.file = Some(file);
    }
    Some(S_OK)
}

/// IPersistFile::SaveCompleted(this, file).
pub fn save_completed(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(S_OK)
}

/// IPersistFile::GetCurFile(this, out): a copy of the file's name in
/// the task allocator, or NULL and S_FALSE before one.
pub fn get_cur_file(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = link_id(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    if args[1] == 0 {
        return Some(E_POINTER);
    }
    let Some(file) = links(cpu)[id].as_ref()?.file.clone() else {
        com::write_out(cpu, args[1], 0);
        return Some(S_FALSE);
    };
    let units: Vec<u16> = crate::dosstr::to_bytes(&file)
        .iter()
        .map(|&b| b as u16)
        .chain([0])
        .collect();
    let at = crate::win32::ole32::task_alloc(cpu, 2 * units.len() as u32);
    if at == 0 {
        com::write_out(cpu, args[1], 0);
        return Some(E_OUTOFMEMORY);
    }
    for (i, u) in units.iter().enumerate() {
        cpu.bus.guest_write_16(at + 2 * i as u32, *u);
    }
    com::write_out(cpu, args[1], at);
    Some(S_OK)
}

/// The header's flags.
const HAS_ID_LIST: u32 = 0x01;
const HAS_LINK_INFO: u32 = 0x02;
const HAS_NAME: u32 = 0x04;
const HAS_RELATIVE_PATH: u32 = 0x08;
const HAS_WORKING_DIR: u32 = 0x10;
const HAS_ARGUMENTS: u32 = 0x20;
const HAS_ICON_LOCATION: u32 = 0x40;
const IS_UNICODE: u32 = 0x80;
const HEADER_SIZE: usize = 0x4C;

/// The .lnk file of `d`.
pub fn write(d: &LinkData) -> Vec<u8> {
    let strings = [
        (HAS_NAME, &d.description),
        (HAS_RELATIVE_PATH, &d.relative),
        (HAS_WORKING_DIR, &d.directory),
        (HAS_ARGUMENTS, &d.arguments),
        (HAS_ICON_LOCATION, &d.icon),
    ];
    let mut flags = 0;
    if !d.list.is_empty() {
        flags |= HAS_ID_LIST;
    }
    let local = d.path.len() >= 3 && d.path.as_bytes()[1] == b':';
    if local {
        flags |= HAS_LINK_INFO;
    }
    for (flag, s) in strings {
        if !s.is_empty() {
            flags |= flag;
        }
    }
    let mut out = vec![0u8; HEADER_SIZE];
    out[0..4].copy_from_slice(&(HEADER_SIZE as u32).to_le_bytes());
    out[4..20].copy_from_slice(&CLSID_SHELL_LINK.0);
    out[20..24].copy_from_slice(&flags.to_le_bytes());
    out[24..28].copy_from_slice(&d.attributes.to_le_bytes());
    out[52..56].copy_from_slice(&d.size.to_le_bytes());
    out[56..60].copy_from_slice(&d.icon_index.to_le_bytes());
    out[60..64].copy_from_slice(&d.show.to_le_bytes());
    out[64..66].copy_from_slice(&d.hotkey.to_le_bytes());
    if flags & HAS_ID_LIST != 0 {
        out.extend_from_slice(&(d.list.len() as u16).to_le_bytes());
        out.extend_from_slice(&d.list);
    }
    if local {
        // The volume: a fixed disk with no label.
        let volume = [0x11, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0, 0x10, 0, 0, 0, 0];
        let mut base = crate::dosstr::to_bytes(&d.path);
        base.push(0);
        let base_at = 0x1C + volume.len();
        let suffix_at = base_at + base.len();
        let size = suffix_at + 1;
        let header = [
            size as u32,
            0x1C,
            1,
            0x1C,
            base_at as u32,
            0,
            suffix_at as u32,
        ];
        for word in header {
            out.extend_from_slice(&word.to_le_bytes());
        }
        out.extend_from_slice(&volume);
        out.extend_from_slice(&base);
        out.push(0);
    }
    for (_, s) in strings {
        if !s.is_empty() {
            let bytes = crate::dosstr::to_bytes(s);
            out.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
            out.extend_from_slice(&bytes);
        }
    }
    // No extra data blocks.
    out.extend_from_slice(&[0, 0, 0, 0]);
    out
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// The string at `at` in `b`, ended by a 0 or by `b`'s end.
fn c_string(b: &[u8], at: usize) -> String {
    let rest = b.get(at..).unwrap_or(&[]);
    let end = rest.iter().position(|&c| c == 0).unwrap_or(rest.len());
    crate::dosstr::from_bytes(&rest[..end])
}

/// What the .lnk file `b` holds, or None if it isn't one.
pub fn parse(b: &[u8]) -> Option<LinkData> {
    if u32_at(b, 0)? as usize != HEADER_SIZE || b.get(4..20)? != CLSID_SHELL_LINK.0 {
        return None;
    }
    let flags = u32_at(b, 20)?;
    let mut d = LinkData {
        attributes: u32_at(b, 24)?,
        size: u32_at(b, 52)?,
        icon_index: u32_at(b, 56)?,
        show: u32_at(b, 60)?,
        hotkey: u16_at(b, 64)?,
        ..LinkData::default()
    };
    let mut at = HEADER_SIZE;
    if flags & HAS_ID_LIST != 0 {
        let size = u16_at(b, at)? as usize;
        d.list = b.get(at + 2..at + 2 + size)?.to_vec();
        at += 2 + size;
    }
    if flags & HAS_LINK_INFO != 0 {
        let size = u32_at(b, at)? as usize;
        let info = b.get(at..at + size)?;
        let info_flags = u32_at(info, 8)?;
        if info_flags & 1 != 0 {
            let base = c_string(info, u32_at(info, 16)? as usize);
            let suffix = c_string(info, u32_at(info, 24)? as usize);
            d.path = base + &suffix;
        }
        at += size;
    }
    let unicode = flags & IS_UNICODE != 0;
    let fields: [(u32, &mut String); 5] = [
        (HAS_NAME, &mut d.description),
        (HAS_RELATIVE_PATH, &mut d.relative),
        (HAS_WORKING_DIR, &mut d.directory),
        (HAS_ARGUMENTS, &mut d.arguments),
        (HAS_ICON_LOCATION, &mut d.icon),
    ];
    for (flag, field) in fields {
        if flags & flag == 0 {
            continue;
        }
        let n = u16_at(b, at)? as usize;
        at += 2;
        let bytes = if unicode {
            let raw = b.get(at..at + 2 * n)?;
            at += 2 * n;
            let units: Vec<u16> = raw
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            wide_to_ansi(&units)
        } else {
            let raw = b.get(at..at + n)?.to_vec();
            at += n;
            raw
        };
        *field = crate::dosstr::from_bytes(&bytes);
    }
    if d.path.is_empty() && !d.list.is_empty() {
        let items = split(&d.list);
        d.path = pidl::path_of(&items, "C:\\WINDOWS\\Desktop").unwrap_or_default();
    }
    Some(d)
}

/// The items of a list's bytes.
fn split(list: &[u8]) -> Vec<pidl::Item> {
    let mut items = Vec::new();
    let mut at = 0;
    while let Some(cb) = u16_at(list, at) {
        let cb = cb as usize;
        if cb < 2 || at + cb > list.len() {
            break;
        }
        items.push(list[at + 2..at + cb].to_vec());
        at += cb;
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_reads_back_as_written() {
        let d = LinkData {
            list: pidl::encode(&[pidl::guid_item(pidl::PT_GUID, &pidl::CLSID_MY_COMPUTER)]),
            path: "C:\\GAME\\GAME.EXE".to_string(),
            description: "The game".to_string(),
            directory: "C:\\GAME".to_string(),
            arguments: "-nosound".to_string(),
            icon: "C:\\GAME\\GAME.ICO".to_string(),
            icon_index: 2,
            show: 3,
            hotkey: 0x0641,
            attributes: 0x20,
            size: 1234,
            ..LinkData::default()
        };
        assert_eq!(parse(&write(&d)), Some(d));
    }

    #[test]
    fn something_else_is_not_a_link() {
        assert_eq!(parse(b"MZ\x90\x00"), None);
        assert_eq!(parse(&[]), None);
    }
}
