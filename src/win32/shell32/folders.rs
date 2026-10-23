//! The special folders, by their CSIDL numbers: where Windows 95 keeps
//! the desktop, the Start Menu and the rest on drive C:, and the shell's
//! folders that aren't on a disk (My Computer, the Control Panel).
//!
//! A folder on disk has its path in the registry, in the "Shell Folders"
//! key of the current user, as Windows 95's Explorer writes it there;
//! a value that isn't there is set to the folder Windows 95 Setup makes.
//! The folder is made when a program first asks for it, as Explorer has
//! made them all on Windows by the time a program runs.

use super::pidl::{self, Item, PT_GUID, PT_SHELLEXT, PT_YAGUID};
use crate::cpu::Cpu;
use crate::win32::com::Guid;
use crate::win32::registry::{self, HKEY_CURRENT_USER, Open, REG_SZ};

/// Where Shell Folders is, under HKEY_CURRENT_USER.
const SHELL_FOLDERS: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Shell Folders";

/// What a special folder is.
pub enum Folder {
    /// The desktop: the empty list, its files in Desktop.
    Desktop,
    /// A folder on disk: its value in Shell Folders (or None for one
    /// that isn't kept there) and its path when the value isn't there.
    Disk(Option<&'static str>, &'static str),
    /// A folder that isn't on a disk: CLSID items, from the desktop down.
    Virtual(&'static [(u8, Guid)]),
}

/// The special folder `csidl`. Windows 95's SHELL32 4.00 has 00h to 15h;
/// the shell of Internet Explorer 4 (4.71) adds Application Data and
/// the cache's folders, which programs of the time ask for too. The
/// folders shared by all users (16h to 19h) are the current user's, as
/// Windows 95 has a single set.
pub fn folder(csidl: u32) -> Option<Folder> {
    use Folder::{Desktop, Disk, Virtual};
    const MY_COMPUTER: (u8, Guid) = (PT_GUID, pidl::CLSID_MY_COMPUTER);
    Some(match csidl & 0xFF {
        0x00 => Desktop,
        0x01 => Virtual(&[(PT_GUID, pidl::CLSID_INTERNET)]),
        0x02 | 0x17 => Disk(Some("Programs"), "C:\\WINDOWS\\Start Menu\\Programs"),
        0x03 => Virtual(&[MY_COMPUTER, (PT_SHELLEXT, pidl::CLSID_CONTROL_PANEL)]),
        0x04 => Virtual(&[MY_COMPUTER, (PT_YAGUID, pidl::CLSID_PRINTERS)]),
        0x05 => Disk(Some("Personal"), "C:\\My Documents"),
        0x06 => Disk(Some("Favorites"), "C:\\WINDOWS\\Favorites"),
        0x07 | 0x18 => Disk(
            Some("Startup"),
            "C:\\WINDOWS\\Start Menu\\Programs\\StartUp",
        ),
        0x08 => Disk(Some("Recent"), "C:\\WINDOWS\\Recent"),
        0x09 => Disk(Some("SendTo"), "C:\\WINDOWS\\SendTo"),
        0x0A => Virtual(&[(PT_GUID, pidl::CLSID_RECYCLE_BIN)]),
        0x0B | 0x16 => Disk(Some("Start Menu"), "C:\\WINDOWS\\Start Menu"),
        0x10 | 0x19 => Disk(Some("Desktop"), "C:\\WINDOWS\\Desktop"),
        0x11 => Virtual(&[MY_COMPUTER]),
        0x12 => Virtual(&[(PT_GUID, pidl::CLSID_NETWORK)]),
        0x13 => Disk(Some("NetHood"), "C:\\WINDOWS\\NetHood"),
        0x14 => Disk(Some("Fonts"), "C:\\WINDOWS\\FONTS"),
        0x15 => Disk(Some("Templates"), "C:\\WINDOWS\\ShellNew"),
        0x1A => Disk(Some("AppData"), "C:\\WINDOWS\\Application Data"),
        0x1B => Disk(Some("PrintHood"), "C:\\WINDOWS\\PrintHood"),
        0x20 => Disk(Some("Cache"), "C:\\WINDOWS\\Temporary Internet Files"),
        0x21 => Disk(Some("Cookies"), "C:\\WINDOWS\\Cookies"),
        0x22 => Disk(Some("History"), "C:\\WINDOWS\\History"),
        // Shell 5.0's, which programs written for later Windows ask for.
        0x24 => Disk(None, "C:\\WINDOWS"),
        0x25 => Disk(None, "C:\\WINDOWS\\SYSTEM"),
        0x26 => Disk(None, "C:\\Program Files"),
        0x2B => Disk(None, "C:\\Program Files\\Common Files"),
        _ => return None,
    })
}

/// The path of a folder on disk: its value in Shell Folders, set there
/// if it wasn't.
fn disk_path(cpu: &mut Cpu, value: Option<&str>, default: &str) -> String {
    let Some(value) = value else {
        return default.to_string();
    };
    let registry = registry::get(cpu);
    let Open::Key(user) = registry.open(HKEY_CURRENT_USER, 0) else {
        return default.to_string();
    };
    let (key, _) = registry.create(user, SHELL_FOLDERS);
    if let Some(v) = registry.value(key, value)
        && v.kind == REG_SZ
    {
        let end = v.data.iter().position(|&b| b == 0).unwrap_or(v.data.len());
        let path = crate::dosstr::from_bytes(&v.data[..end]);
        if !path.is_empty() {
            return path;
        }
    }
    let mut data = crate::dosstr::to_bytes(default);
    data.push(0);
    registry.set_value(key, value, REG_SZ, data);
    default.to_string()
}

/// Make the directory `path` and those it is in, where they aren't.
pub fn make_directories(cpu: &mut Cpu, path: &str) {
    let mut at = 3;
    while at <= path.len() {
        let end = path[at..].find('\\').map_or(path.len(), |i| at + i);
        let part = &path[..end];
        if crate::win32::kernel32::files::attributes(cpu, part).is_err() {
            let _ = crate::win32::kernel32::files::create_directory(cpu, part);
        }
        at = end + 1;
    }
}

/// The path of the special folder `csidl` on disk, made if it isn't
/// there; None for folders that aren't on disk or aren't known.
pub fn path(cpu: &mut Cpu, csidl: u32) -> Option<String> {
    let path = match folder(csidl)? {
        Folder::Desktop => disk_path(cpu, Some("Desktop"), "C:\\WINDOWS\\Desktop"),
        Folder::Disk(value, default) => disk_path(cpu, value, default),
        Folder::Virtual(_) => return None,
    };
    make_directories(cpu, &path);
    Some(path)
}

/// The desktop's folder on disk.
pub fn desktop(cpu: &mut Cpu) -> String {
    path(cpu, 0x10).unwrap_or_else(|| "C:\\WINDOWS\\Desktop".to_string())
}

/// The item ID list of the special folder `csidl`.
pub fn items(cpu: &mut Cpu, csidl: u32) -> Option<Vec<Item>> {
    match folder(csidl)? {
        Folder::Desktop => Some(Vec::new()),
        Folder::Virtual(ids) => Some(ids.iter().map(|(k, g)| pidl::guid_item(*k, g)).collect()),
        Folder::Disk(..) => {
            let path = path(cpu, csidl)?;
            pidl::from_path(cpu, &path, true)
        }
    }
}
