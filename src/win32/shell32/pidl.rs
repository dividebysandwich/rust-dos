//! Item ID lists (PIDLs): the shell's names of the things in its
//! namespace, as Windows 95's SHELL32 4.00 makes them. A list is items
//! one after another, each a 16-bit size (its own two bytes counted)
//! and then data, ended by a size of 0. The empty list is the desktop.
//!
//! Each item's first data byte is its type (Wine's pidl.h names them):
//! a CLSID under the desktop (My Computer, the Recycle Bin), a drive
//! under My Computer ("C:\" in 20 bytes and a word), and folders and
//! files under that: size, DOS date, time and attributes, then the long
//! name and the 8.3 name, each ended by a 0. Windows 95 has no Unicode
//! part after those, which later shells add.
//!
//! The lists live in the task allocator (SHAlloc), as Windows has them,
//! and programs free them with ILFree or the allocator's Free.

use crate::cpu::Cpu;
use crate::win32::com::Guid;
use crate::win32::kernel32::files::{self, FindEntry};

/// Item types.
pub const PT_GUID: u8 = 0x1F;
pub const PT_DRIVE: u8 = 0x23;
pub const PT_SHELLEXT: u8 = 0x2E;
pub const PT_FOLDER: u8 = 0x31;
pub const PT_VALUE: u8 = 0x32;
pub const PT_YAGUID: u8 = 0x70;

/// The shell's folders that aren't on a disk, by their CLSIDs.
pub const CLSID_MY_COMPUTER: Guid = Guid::new(
    0x20D0_4FE0,
    0x3AEA,
    0x1069,
    [0xA2, 0xD8, 0x08, 0x00, 0x2B, 0x30, 0x30, 0x9D],
);
pub const CLSID_RECYCLE_BIN: Guid = Guid::new(
    0x645F_F040,
    0x5081,
    0x101B,
    [0x9F, 0x08, 0x00, 0xAA, 0x00, 0x2F, 0x95, 0x4E],
);
pub const CLSID_INTERNET: Guid = Guid::new(
    0x871C_5380,
    0x42A0,
    0x1069,
    [0xA2, 0xEA, 0x08, 0x00, 0x2B, 0x30, 0x30, 0x9D],
);
pub const CLSID_NETWORK: Guid = Guid::new(
    0x208D_2C60,
    0x3AEA,
    0x1069,
    [0xA2, 0xD7, 0x08, 0x00, 0x2B, 0x30, 0x30, 0x9D],
);
pub const CLSID_CONTROL_PANEL: Guid = Guid::new(
    0x21EC_2020,
    0x3AEA,
    0x1069,
    [0xA2, 0xDD, 0x08, 0x00, 0x2B, 0x30, 0x30, 0x9D],
);
pub const CLSID_PRINTERS: Guid = Guid::new(
    0x2227_A280,
    0x3AEA,
    0x1069,
    [0xA2, 0xDE, 0x08, 0x00, 0x2B, 0x30, 0x30, 0x9D],
);

/// The longest list read from a program, in bytes.
const MAX_LIST: usize = 0x1_0000;

/// One item: its type and data, without the size.
pub type Item = Vec<u8>;

/// An item for a CLSID, of the type `kind`.
pub fn guid_item(kind: u8, guid: &Guid) -> Item {
    let mut item = vec![kind, 0];
    item.extend_from_slice(&guid.0);
    item
}

/// The item of a drive, by its letter.
pub fn drive_item(letter: char) -> Item {
    let mut item = vec![0u8; 23];
    item[0] = PT_DRIVE;
    item[1] = letter.to_ascii_uppercase() as u8;
    item[2..4].copy_from_slice(b":\\");
    item
}

/// The item of a file or folder found on disk.
pub fn file_item(entry: &FindEntry) -> Item {
    let directory = entry.attributes & 0x10 != 0;
    let mut item = vec![if directory { PT_FOLDER } else { PT_VALUE }, 0];
    item.extend_from_slice(&entry.size.to_le_bytes());
    item.extend_from_slice(&entry.date.to_le_bytes());
    item.extend_from_slice(&entry.time.to_le_bytes());
    item.extend_from_slice(&(entry.attributes as u16).to_le_bytes());
    item.extend_from_slice(&crate::dosstr::to_bytes(&entry.name));
    item.push(0);
    item.extend_from_slice(&crate::dosstr::to_bytes(&entry.short));
    item.push(0);
    // Windows keeps items an even size.
    if item.len() % 2 != 0 {
        item.push(0);
    }
    item
}

/// The list of `items` as bytes, its end included.
pub fn encode(items: &[Item]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for item in items {
        bytes.extend_from_slice(&((item.len() + 2) as u16).to_le_bytes());
        bytes.extend_from_slice(item);
    }
    bytes.extend_from_slice(&[0, 0]);
    bytes
}

/// The items of the list at `at`: None if it isn't readable or runs
/// on past `MAX_LIST`.
pub fn read(cpu: &mut Cpu, at: u32) -> Option<Vec<Item>> {
    let mut items = Vec::new();
    let mut offset = 0u32;
    loop {
        if offset as usize > MAX_LIST || !cpu.bus.guest_probe(at + offset, 2, false) {
            return None;
        }
        let cb = cpu.bus.guest_read_16(at + offset) as u32;
        if cb == 0 {
            return Some(items);
        }
        if cb < 2 || !cpu.bus.guest_probe(at + offset, cb as usize, false) {
            return None;
        }
        let mut item = vec![0u8; cb as usize - 2];
        cpu.bus.guest_read_bytes(at + offset + 2, &mut item);
        items.push(item);
        offset += cb;
    }
}

/// A new list in the task allocator holding `bytes`: its address, or 0
/// when memory is out.
pub fn alloc(cpu: &mut Cpu, bytes: &[u8]) -> u32 {
    let at = crate::win32::ole32::task_alloc(cpu, bytes.len() as u32);
    if at != 0 {
        cpu.bus.guest_write_bytes(at, bytes);
    }
    at
}

/// A new list of `items`, as `alloc` makes it.
pub fn alloc_items(cpu: &mut Cpu, items: &[Item]) -> u32 {
    alloc(cpu, &encode(items))
}

/// The CLSID of a CLSID item.
pub fn item_guid(item: &[u8]) -> Option<Guid> {
    match item.first() {
        Some(&(PT_GUID | PT_SHELLEXT | PT_YAGUID)) if item.len() >= 18 => {
            let mut g = [0u8; 16];
            g.copy_from_slice(&item[2..18]);
            Some(Guid(g))
        }
        _ => None,
    }
}

/// The long name of a file or folder item.
fn item_name(item: &[u8]) -> Option<String> {
    match item.first() {
        Some(&(PT_FOLDER | PT_VALUE)) if item.len() > 12 => {
            let name = &item[12..];
            let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
            Some(crate::dosstr::from_bytes(&name[..end]))
        }
        _ => None,
    }
}

/// The drive letter of a drive item.
fn item_drive(item: &[u8]) -> Option<char> {
    match item {
        [PT_DRIVE, letter, b':', ..] if letter.is_ascii_alphabetic() => {
            Some((*letter as char).to_ascii_uppercase())
        }
        _ => None,
    }
}

/// The path on disk the list `items` names, if it names one: a drive
/// under My Computer and folders and files under it, or files and
/// folders on the desktop, whose folder is `desktop`.
pub fn path_of(items: &[Item], desktop: &str) -> Option<String> {
    let (mut path, rest) = match items.first() {
        None => return Some(desktop.to_string()),
        Some(first) if item_guid(first) == Some(CLSID_MY_COMPUTER) => {
            let drive = item_drive(items.get(1)?)?;
            (format!("{}:\\", drive), &items[2..])
        }
        Some(first) if item_name(first).is_some() => (desktop.to_string(), items),
        _ => return None,
    };
    for item in rest {
        let name = item_name(item)?;
        if !path.ends_with('\\') {
            path.push('\\');
        }
        path.push_str(&name);
    }
    Some(path)
}

/// The list naming the path `path` (a full one, "C:\..."): from what
/// is on disk, or for `simple` with names alone where nothing is there,
/// as SHSimpleIDListFromPath makes it. None if a part isn't there and
/// the list isn't simple, or the path isn't a drive's.
pub fn from_path(cpu: &mut Cpu, path: &str, simple: bool) -> Option<Vec<Item>> {
    let full = files::full_path(cpu, path);
    let bytes = full.as_bytes();
    if bytes.len() < 3 || bytes[1] != b':' || !bytes[0].is_ascii_alphabetic() {
        return None;
    }
    let mut items = vec![
        guid_item(PT_GUID, &CLSID_MY_COMPUTER),
        drive_item(bytes[0] as char),
    ];
    let mut dir = full[..3].to_string();
    let parts: Vec<&str> = full[3..].split('\\').filter(|p| !p.is_empty()).collect();
    for (i, part) in parts.iter().enumerate() {
        let found = files::find(cpu, &dir, part)
            .ok()
            .and_then(|e| e.into_iter().find(|e| e.name != "." && e.name != ".."));
        let entry = match found {
            Some(entry) => entry,
            None if simple => FindEntry {
                // A part before the last is a folder.
                attributes: if i + 1 < parts.len() { 0x10 } else { 0x20 },
                name: part.to_string(),
                ..FindEntry::default()
            },
            None => return None,
        };
        if !dir.ends_with('\\') {
            dir.push('\\');
        }
        dir.push_str(&entry.name);
        items.push(file_item(&entry));
    }
    Some(items)
}

/// Whether two items name the same thing: file names whatever their
/// case, the rest byte for byte.
pub fn same_item(a: &[u8], b: &[u8]) -> bool {
    match (item_name(a), item_name(b)) {
        (Some(x), Some(y)) => a[0] == b[0] && x.eq_ignore_ascii_case(&y),
        _ => a == b,
    }
}
