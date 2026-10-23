//! SHGetFileInfoA: what the shell shows of a file or folder (its name,
//! type, attributes and icon), and Shell_MergeMenus.
//!
//! Icons come from SHELL32's own icons as Windows 95 numbers them: 0 a
//! document, 2 a program, 3 a folder, 8 a drive. There is no system
//! image list: SHGFI_SYSICONINDEX fails.

use crate::cpu::Cpu;
use crate::win32::kernel32::files;
use crate::win32::user32::string_at;

const SHGFI_ICON: u32 = 0x100;
const SHGFI_DISPLAYNAME: u32 = 0x200;
const SHGFI_TYPENAME: u32 = 0x400;
const SHGFI_ATTRIBUTES: u32 = 0x800;
const SHGFI_ICONLOCATION: u32 = 0x1000;
const SHGFI_EXETYPE: u32 = 0x2000;
const SHGFI_SYSICONINDEX: u32 = 0x4000;
const SHGFI_USEFILEATTRIBUTES: u32 = 0x10;
const SHGFI_PIDL: u32 = 0x8;

const SFGAO_CANCOPY: u32 = 0x1;
const SFGAO_CANMOVE: u32 = 0x2;
const SFGAO_CANLINK: u32 = 0x4;
const SFGAO_FILESYSTEM: u32 = 0x4000_0000;
const SFGAO_FOLDER: u32 = 0x2000_0000;
const SFGAO_HASSUBFOLDER: u32 = 0x8000_0000;

/// SHFILEINFOA's size and fields.
const INFO_SIZE: usize = 352;
const DISPLAY_NAME: u32 = 12;
const TYPE_NAME: u32 = 272;

/// Where SHELL32's icons are, as SHGFI_ICONLOCATION names it.
const SHELL32_PATH: &str = "C:\\WINDOWS\\SYSTEM\\SHELL32.DLL";

fn put(cpu: &mut Cpu, at: u32, max: usize, text: &str) {
    let mut bytes = crate::dosstr::to_bytes(text);
    bytes.truncate(max - 1);
    bytes.push(0);
    cpu.bus.guest_write_bytes(at, &bytes);
}

/// The type name Explorer shows for a file named `name`.
fn type_name(name: &str, directory: bool) -> String {
    if directory {
        return "Folder".to_string();
    }
    match name.rsplit_once('.') {
        Some((_, ext)) if ext.eq_ignore_ascii_case("exe") || ext.eq_ignore_ascii_case("com") => {
            "Application".to_string()
        }
        Some((_, ext)) if !ext.is_empty() => format!("{} File", ext.to_ascii_uppercase()),
        _ => "File".to_string(),
    }
}

/// The icon of SHELL32's a file or folder has.
fn icon_index(name: &str, directory: bool, root: bool) -> u32 {
    let program = name.rsplit_once('.').is_some_and(|(_, e)| {
        ["exe", "com", "bat", "pif"]
            .iter()
            .any(|x| e.eq_ignore_ascii_case(x))
    });
    if root {
        8
    } else if directory {
        3
    } else if program {
        2
    } else {
        0
    }
}

/// SHGetFileInfoA(path, attributes, info, size, flags).
pub fn sh_get_file_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [path, attributes, info, size, flags] = args[..] else {
        return Some(0);
    };
    if path == 0 {
        return Some(0);
    }
    // Windows clears these three first, whatever the flags.
    if info != 0 && (size as usize) >= INFO_SIZE && cpu.bus.guest_probe(info, INFO_SIZE, true) {
        cpu.bus.guest_write_32(info, 0);
        cpu.bus.guest_write_8(info + DISPLAY_NAME, 0);
        cpu.bus.guest_write_8(info + TYPE_NAME, 0);
    }
    let (name, attr, root, desktop) = if flags & SHGFI_PIDL != 0 {
        let items = super::pidl::read(cpu, path)?;
        match super::il::path_of(cpu, path) {
            Some(p) if !items.is_empty() => {
                let attr = files::attributes(cpu, &p).unwrap_or(0x10);
                (p, attr, false, false)
            }
            _ => ("Desktop".to_string(), 0x10, false, items.is_empty()),
        }
    } else {
        let text = string_at(cpu, path)?;
        if text.is_empty() && flags == 0 {
            return Some(1);
        }
        let attr = if flags & SHGFI_USEFILEATTRIBUTES != 0 {
            attributes
        } else {
            match files::attributes(cpu, &text) {
                Ok(attr) => attr,
                Err(_) => return Some(0),
            }
        };
        let full = files::full_path(cpu, &text);
        let root = full.len() <= 3;
        (full, attr, root, false)
    };
    if info == 0 || (size as usize) < INFO_SIZE || !cpu.bus.guest_probe(info, INFO_SIZE, true) {
        return Some(if flags & (SHGFI_ATTRIBUTES | SHGFI_ICON) == 0 {
            1
        } else {
            0
        });
    }
    let directory = attr & 0x10 != 0;
    let short = if root {
        format!("({})", &name[..2])
    } else if desktop {
        name.clone()
    } else {
        name.rsplit('\\').next().unwrap_or("").to_string()
    };
    let icon = icon_index(&short, directory, root);
    if flags & SHGFI_EXETYPE != 0 {
        // A file that isn't there, named with SHGFI_USEFILEATTRIBUTES,
        // gives 1, as the tests saw Windows do.
        if flags & SHGFI_USEFILEATTRIBUTES != 0 {
            return Some(1);
        }
        return Some(exe_type(cpu, &name));
    }
    if flags & SHGFI_SYSICONINDEX != 0 {
        cpu.bus.guest_write_32(info + 4, icon);
        return Some(0);
    }
    if flags & SHGFI_ATTRIBUTES != 0 {
        let mut sfgao = SFGAO_CANCOPY | SFGAO_CANMOVE | SFGAO_CANLINK | SFGAO_FILESYSTEM;
        if directory {
            sfgao |= SFGAO_FOLDER | SFGAO_HASSUBFOLDER;
        }
        cpu.bus.guest_write_32(info + 8, sfgao);
    }
    if flags & SHGFI_DISPLAYNAME != 0 {
        put(cpu, info + DISPLAY_NAME, 260, &short);
    }
    if flags & SHGFI_TYPENAME != 0 {
        let kind = if root {
            "Local Disk".to_string()
        } else {
            type_name(&short, directory)
        };
        put(cpu, info + TYPE_NAME, 80, &kind);
    }
    if flags & SHGFI_ICONLOCATION != 0 {
        put(cpu, info + DISPLAY_NAME, 260, SHELL32_PATH);
        cpu.bus.guest_write_32(info + 4, icon);
    }
    if flags & SHGFI_ICON != 0 {
        // IDI_APPLICATION's icon, a copy the program destroys.
        let shared = crate::win32::user32::resource::load_icon(cpu, &[0, 32512])?;
        let copy = crate::win32::user32::resource::copy_icon(cpu, &[shared])?;
        cpu.bus.guest_write_32(info, copy);
        cpu.bus.guest_write_32(info + 4, icon);
    }
    Some(1)
}

/// SHGFI_EXETYPE's value for the file `path`: for a Windows program
/// "PE" and the Windows version it needs (4.0), for a DOS program "MZ",
/// and 0 for any other file.
fn exe_type(cpu: &mut Cpu, path: &str) -> u32 {
    if !super::exec::is_program(path) {
        return 0;
    }
    let Ok((data, _, _)) = crate::win32::kernel32::read_path(cpu, path) else {
        return 0;
    };
    if data.len() < 0x40 || &data[..2] != b"MZ" {
        return 0;
    }
    let at = u32::from_le_bytes([data[0x3C], data[0x3D], data[0x3E], data[0x3F]]) as usize;
    if data.get(at..at + 4) == Some(b"PE\0\0") {
        return 0x0400_4550;
    }
    0x5A4D
}

/// SHGetNewLinkInfoA(target, directory, name, must copy, flags): the
/// name a new shortcut to `target` gets in `directory`, as Windows 95's
/// Explorer names it ("Shortcut to X.lnk", then "Shortcut (2) to
/// X.lnk"), not one there already. FALSE if the target isn't there.
pub fn sh_get_new_link_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [target, directory, name_at, must_copy, flags] = args[..] else {
        return Some(0);
    };
    const SHGNLI_PIDL: u32 = 1;
    if must_copy != 0 {
        if !cpu.bus.guest_probe(must_copy, 4, true) {
            return None;
        }
        cpu.bus.guest_write_32(must_copy, 0);
    }
    if flags & SHGNLI_PIDL != 0 || target == 0 || directory == 0 || name_at == 0 {
        return Some(0);
    }
    let target = string_at(cpu, target)?;
    let directory = string_at(cpu, directory)?;
    if files::attributes(cpu, &target).is_err() {
        return Some(0);
    }
    let file = target.rsplit('\\').next().unwrap_or(&target);
    let stem = file.rsplit_once('.').map_or(file, |(s, _)| s);
    let dir = directory.trim_end_matches('\\');
    let mut name = format!("{}\\Shortcut to {}.lnk", dir, stem);
    let mut n = 2;
    while files::attributes(cpu, &name).is_ok() {
        name = format!("{}\\Shortcut ({}) to {}.lnk", dir, n, stem);
        n += 1;
    }
    put(cpu, name_at, 260, &name);
    Some(1)
}

/// Shell_MergeMenus(destination, source, at, first id, last id, flags):
/// with no menus to merge, the first id, as the next free one.
pub fn shell_merge_menus(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(args[3])
}
