//! Functions KERNEL32 has from Windows 98 on (version.rs lists them, so
//! that Windows 95 doesn't), and GetDiskFreeSpaceExA, which came with
//! Windows 95 OSR2.

use super::path::{
    ERROR_FILE_NOT_FOUND, ERROR_INVALID_NAME, dos_path, drive_of, find_entries, full_path,
    has_wildcards, is_root, not_found, path_arg, put_string, trim_slash,
};
use super::{
    ERROR_INVALID_DRIVE, ERROR_INVALID_PARAMETER, ERROR_PATH_NOT_FOUND, FALSE, TRUE, set_error,
    string_at,
};
use crate::cpu::Cpu;

/// GetFileExInfoStandard, GetFileAttributesExA's only level.
const GET_FILE_EX_INFO_STANDARD: u32 = 0;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const ERROR_BAD_NET_NAME: u32 = 67;

/// IsDebuggerPresent: no debugger runs the programs.
pub(super) fn is_debugger_present(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(FALSE)
}

/// InterlockedCompareExchange(dest, exchange, comperand): `exchange` in
/// the variable at `dest` if it holds `comperand`; its old value.
pub(super) fn interlocked_compare_exchange(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let old = cpu.bus.guest_read_32(args[0]);
    if old == args[2] {
        cpu.bus.guest_write_32(args[0], args[1]);
    }
    Some(old)
}

/// InterlockedExchangeAdd(dest, value): `value` added to the variable at
/// `dest`; its old value.
pub(super) fn interlocked_exchange_add(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let old = cpu.bus.guest_read_32(args[0]);
    cpu.bus.guest_write_32(args[0], old.wrapping_add(args[1]));
    Some(old)
}

/// GetDiskFreeSpaceExA(dir, free_to_caller, total, free): in bytes, of
/// the drive of a directory (the current one with NULL), as 64-bit
/// numbers; each pointer may be NULL.
pub(super) fn get_disk_free_space_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let drive = if args[0] == 0 {
        cpu.bus.disk.get_current_drive()
    } else {
        let dir = string_at(cpu, args[0])?;
        let full = full_path(cpu, &dos_path(&dir));
        if !cpu.bus.disk.is_directory(trim_slash(&full)) {
            set_error(cpu, ERROR_PATH_NOT_FOUND);
            return Some(FALSE);
        }
        full.as_bytes()[0].to_ascii_uppercase().wrapping_sub(b'A')
    };
    let Ok((sectors, free, bytes, total)) = cpu.bus.disk.windows_free_space(drive + 1) else {
        set_error(cpu, ERROR_INVALID_DRIVE);
        return Some(FALSE);
    };
    let cluster = sectors as u64 * bytes as u64;
    let free = free as u64 * cluster;
    let total = total as u64 * cluster;
    for (at, value) in args[1..].iter().zip([free, total, free]) {
        if *at != 0 {
            cpu.bus.guest_write_32(*at, value as u32);
            cpu.bus.guest_write_32(at + 4, (value >> 32) as u32);
        }
    }
    Some(TRUE)
}

/// GetFileAttributesExA(name, level, data): WIN32_FILE_ATTRIBUTE_DATA,
/// the attributes, the three times and the size, which are where
/// FindFirstFileA's WIN32_FIND_DATAA has them.
pub(super) fn get_file_attributes_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[name, level, data] = args else {
        unreachable!()
    };
    if level != GET_FILE_EX_INFO_STANDARD || data == 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    let Some(path) = path_arg(cpu, name)? else {
        return Some(FALSE);
    };
    if has_wildcards(&path) {
        set_error(cpu, ERROR_INVALID_NAME);
        return Some(FALSE);
    }
    let full = full_path(cpu, &path);
    let mut words = [0u32; 9];
    if is_root(&full) {
        if !cpu.bus.disk.is_mounted(super::path::drive_of(&full)) {
            set_error(cpu, ERROR_PATH_NOT_FOUND);
            return Some(FALSE);
        }
        words[0] = FILE_ATTRIBUTE_DIRECTORY;
    } else {
        let full = trim_slash(&full);
        let (dir, file) = match full.rfind('\\') {
            Some(i) if i <= 2 => (&full[..3], &full[i + 1..]),
            Some(i) => (&full[..i], &full[i + 1..]),
            None => (full, ""),
        };
        let entry = match find_entries(cpu, dir, file) {
            Ok(entries) => entries.into_iter().next(),
            Err(_) => None,
        };
        let Some(entry) = entry else {
            let code = not_found(cpu, full);
            set_error(cpu, code);
            return Some(FALSE);
        };
        let ticks = crate::win32::convert::dos_to_ticks(entry.date, entry.time).unwrap_or(0);
        words[0] = entry.attributes;
        for time in 0..3 {
            words[1 + 2 * time] = ticks as u32;
            words[2 + 2 * time] = (ticks >> 32) as u32;
        }
        words[8] = entry.size;
    }
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    cpu.bus.guest_write_bytes(data, &bytes);
    Some(TRUE)
}

/// The long name of the entry `name` of the directory `dir` (a full
/// path): its own where the drive has no long names.
fn long_name(cpu: &Cpu, dir: &str, name: &str) -> String {
    let names = cpu.bus.disk.long_names(dir).unwrap_or_default();
    names
        .iter()
        .find(|(_, short)| short.eq_ignore_ascii_case(name))
        .or_else(|| {
            names
                .iter()
                .find(|(long, _)| long.eq_ignore_ascii_case(name))
        })
        .map_or(name.to_string(), |(long, _)| long.clone())
}

/// GetLongPathNameA(path, buffer, size): the path of a file or directory
/// that is there with each name its long one; a relative path stays
/// relative.
pub(super) fn get_long_path_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[name, buffer, size] = args else {
        unreachable!()
    };
    if name == 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    let name = string_at(cpu, name)?;
    if name.is_empty() {
        set_error(cpu, ERROR_FILE_NOT_FOUND);
        return Some(0);
    }
    // There is no network: Windows 98 takes no UNC name here, not even
    // one of \\?\ or of the computer's own shares.
    if name.starts_with("\\\\") || name.starts_with("//") {
        set_error(cpu, ERROR_BAD_NET_NAME);
        return Some(0);
    }
    if has_wildcards(&name) {
        set_error(cpu, ERROR_INVALID_NAME);
        return Some(0);
    }
    let path = dos_path(&name);
    let full = full_path(cpu, &path);
    if !cpu.bus.disk.exists(trim_slash(&full)) {
        let code = not_found(cpu, &full);
        set_error(cpu, code);
        return Some(0);
    }
    let bytes = path.as_bytes();
    let prefix_len = match bytes {
        [d, b':', b'\\', ..] if d.is_ascii_alphabetic() => 3,
        [d, b':', ..] if d.is_ascii_alphabetic() => 2,
        [b'\\', ..] => 1,
        _ => 0,
    };
    let (prefix, rest) = path.split_at(prefix_len);
    let start = if prefix.is_empty() { "." } else { prefix };
    let mut dir = trim_slash(&full_path(cpu, start)).to_string();
    let mut out = prefix.to_string();
    let parts: Vec<&str> = rest.split('\\').collect();
    for (i, part) in parts.iter().enumerate() {
        let long = match *part {
            "" | "." | ".." => part.to_string(),
            _ => long_name(cpu, &dir, part),
        };
        out.push_str(&long);
        if i + 1 < parts.len() {
            out.push('\\');
        }
        if !part.is_empty() {
            dir = trim_slash(&full_path(cpu, &format!("{}\\{}", dir, part))).to_string();
        }
    }
    put_string(cpu, buffer, size, &out)
}

/// Whether `path` names a directory, from the current one, for the
/// built-in DLLs (DirectMusic's loader).
pub fn is_directory(cpu: &Cpu, path: &str) -> bool {
    let full = full_path(cpu, &dos_path(path));
    if is_root(&full) {
        return cpu.bus.disk.is_mounted(drive_of(&full));
    }
    cpu.bus.disk.is_directory(trim_slash(&full))
}
