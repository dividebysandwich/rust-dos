//! Paths and directories: the current directory, full and short names,
//! the Windows, system and temporary directories, searches with
//! FindFirstFileA, and making, removing, copying and moving files and
//! directories, all through DOS's drives (`DiskController`).
//!
//! The built-in DOS has no long file names (INT 21h AX=71xxh), but its
//! host drives keep the host's names, which are the long ones Windows 95
//! shows; its 8.3 names are what DOS programs see. Names on disk images
//! and CDs are 8.3 alone.

use super::{
    ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER,
    ERROR_PATH_NOT_FOUND, FALSE, INVALID_HANDLE_VALUE, TRUE, set_error, string_at,
};
use crate::cpu::Cpu;
use crate::win32::object::{self, FindEntry, Kind};

pub(super) const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_NOT_SAME_DEVICE: u32 = 17;
const ERROR_NO_MORE_FILES: u32 = 18;
pub(super) const ERROR_FILE_EXISTS: u32 = 80;
pub(super) const ERROR_INVALID_NAME: u32 = 123;
pub(super) const ERROR_FILENAME_EXCED_RANGE: u32 = 206;

const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const FILE_ATTRIBUTE_DEVICE: u32 = 0x40;
const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;
const INVALID_FILE_ATTRIBUTES: u32 = 0xFFFF_FFFF;

/// Where Windows 95 is, as far as programs ask.
const WINDOWS: &str = "C:\\WINDOWS";
const SYSTEM: &str = "C:\\WINDOWS\\SYSTEM";

/// WIN32_FIND_DATAA's size, and where its names are.
const FIND_DATA: usize = 320;
const FIND_NAME: u32 = 44;
const FIND_SHORT_NAME: u32 = 304;

/// The longest name a WIN32_FIND_DATAA holds.
const MAX_PATH: usize = 260;

/// A Win32 path as DOS takes it: '/' as '\', and the dots and spaces that
/// end a name gone, as Windows drops them ("Remove Me." is "Remove Me").
pub(super) fn dos_path(path: &str) -> String {
    path.replace('/', "\\")
        .split('\\')
        .map(|part| match part.trim_end_matches(['.', ' ']) {
            _ if part == "." || part == ".." => part,
            "" => part,
            trimmed => trimmed,
        })
        .collect::<Vec<_>>()
        .join("\\")
}

/// Whether a path has wildcards, which no file's name has.
pub(super) fn has_wildcards(path: &str) -> bool {
    path.contains(['*', '?'])
}

/// The letter of the drive `drive` (0 for A:).
fn letter(drive: u8) -> char {
    (b'A' + drive) as char
}

/// The drive a full path ("C:\...") is on.
pub(super) fn drive_of(full: &str) -> u8 {
    full.as_bytes()[0].to_ascii_uppercase().wrapping_sub(b'A')
}

/// The current directory of the drive `drive`, fully qualified ("C:\" or
/// "C:\GAMES"): as SetCurrentDirectoryA was given it, with long names,
/// while DOS's is still that directory.
pub(crate) fn current_directory(cpu: &Cpu, drive: u8) -> String {
    let dir = cpu
        .bus
        .disk
        .get_current_directory_of(drive)
        .unwrap_or_default();
    let short = format!("{}:\\{}", letter(drive), dir);
    cpu.bus
        .win32
        .long_dirs
        .iter()
        .find(|(s, _)| s.eq_ignore_ascii_case(&short))
        .map_or(short, |(_, long)| long.clone())
}

/// The full path of `path`, as GetFullPathNameA makes it: on the drive it
/// names or the current one, from its root or that drive's current
/// directory, with "." and ".." taken out. It ends with a backslash when
/// `path` does. Network paths ("\\server\share") stay as they are.
pub(crate) fn full_path(cpu: &Cpu, path: &str) -> String {
    let path = path.replace('/', "\\");
    if path.starts_with("\\\\") {
        return path;
    }
    let bytes = path.as_bytes();
    let (drive, rest) = match bytes {
        [d, b':', ..] if d.is_ascii_alphabetic() => (d.to_ascii_uppercase() - b'A', &path[2..]),
        _ => (cpu.bus.disk.get_current_drive(), path.as_str()),
    };
    let base = if rest.starts_with('\\') {
        String::new()
    } else {
        current_directory(cpu, drive)
    };
    let mut parts: Vec<&str> = base
        .get(3..)
        .unwrap_or("")
        .split('\\')
        .filter(|p| !p.is_empty())
        .collect();
    for part in rest.split('\\') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    let mut full = format!("{}:\\{}", letter(drive), parts.join("\\"));
    if rest.ends_with('\\') && !parts.is_empty() {
        full.push('\\');
    }
    full
}

/// `full` without the backslash at its end, but a root's.
pub(super) fn trim_slash(full: &str) -> &str {
    match full.strip_suffix('\\') {
        Some(trimmed) if trimmed.len() > 2 => trimmed,
        _ => full,
    }
}

/// The error for a file that isn't there: its directory isn't either
/// (ERROR_PATH_NOT_FOUND), or it is (ERROR_FILE_NOT_FOUND).
pub(super) fn not_found(cpu: &Cpu, full: &str) -> u32 {
    let full = trim_slash(full);
    let parent = match full.rfind('\\') {
        Some(i) if i <= 2 => &full[..3],
        Some(i) => &full[..i],
        None => full,
    };
    if cpu.bus.disk.is_directory(parent) {
        ERROR_FILE_NOT_FOUND
    } else {
        ERROR_PATH_NOT_FOUND
    }
}

/// Copy `s` and its NUL to the buffer at `buffer` of `size` bytes, as the
/// functions that give a path do: its length; or when it doesn't fit, the
/// size it needs, and the buffer as it was.
pub(super) fn put_string(cpu: &mut Cpu, buffer: u32, size: u32, s: &str) -> Option<u32> {
    let mut bytes = crate::dosstr::to_bytes(s);
    let len = bytes.len() as u32;
    if size <= len {
        return Some(len + 1);
    }
    bytes.push(0);
    cpu.bus.guest_write_bytes(buffer, &bytes);
    (!cpu.bus.guest_faulted()).then_some(len)
}

/// `put_string` for the functions that also give where the file's name
/// starts in it (`*filepart`): NULL for a path ending in a backslash.
fn put_path(cpu: &mut Cpu, buffer: u32, size: u32, filepart: u32, s: &str) -> Option<u32> {
    let len = put_string(cpu, buffer, size, s)?;
    if len as usize == s.len() && filepart != 0 {
        let name = match s.rfind('\\') {
            Some(i) if i + 1 < s.len() => buffer + i as u32 + 1,
            Some(_) => 0,
            None => buffer,
        };
        cpu.bus.guest_write_32(filepart, name);
    }
    Some(len)
}

/// GetWindowsDirectoryA(buffer, size).
pub(super) fn get_windows_directory(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    put_string(cpu, args[0], args[1], WINDOWS)
}

/// GetSystemDirectoryA(buffer, size).
pub(super) fn get_system_directory(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    put_string(cpu, args[0], args[1], SYSTEM)
}

/// GetCurrentDirectoryA(size, buffer).
pub(super) fn get_current_directory(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let dir = current_directory(cpu, cpu.bus.disk.get_current_drive());
    put_string(cpu, args[1], args[0], &dir)
}

/// SetCurrentDirectoryA(path): the directory, and its drive, become the
/// current ones. The directory is kept as it was named, long names and
/// all, for GetCurrentDirectoryA; DOS keeps it in short names.
pub(super) fn set_current_directory(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    let name = string_at(cpu, args[0])?;
    if name.is_empty() {
        set_error(cpu, ERROR_PATH_NOT_FOUND);
        return Some(FALSE);
    }
    if let Err(code) = change_directory(cpu, &name) {
        set_error(cpu, code);
        return Some(FALSE);
    }
    Some(TRUE)
}

/// Make the directory `name` and its drive the current ones, as
/// SetCurrentDirectoryA and MSVCRT's _chdir do: the error code when it
/// isn't there.
pub(crate) fn change_directory(cpu: &mut Cpu, name: &str) -> Result<(), u32> {
    let full = full_path(cpu, &dos_path(name));
    let full = trim_slash(&full).to_string();
    let disk = &mut cpu.bus.disk;
    if !disk.is_directory(&full) || !disk.set_current_directory(&full) {
        return Err(not_found(cpu, &full));
    }
    let drive = drive_of(&full);
    disk.set_current_drive(drive);
    crate::win32::dos(cpu, |cpu| crate::dos_data::write_cds(&mut cpu.bus, drive));
    let short = format!(
        "{}:\\{}",
        letter(drive),
        cpu.bus
            .disk
            .get_current_directory_of(drive)
            .unwrap_or_default()
    );
    let long_dirs = &mut cpu.bus.win32.long_dirs;
    long_dirs.retain(|(s, _)| drive_of(s) != drive);
    if !short.eq_ignore_ascii_case(&full) {
        long_dirs.push((short, full));
    }
    Ok(())
}

/// GetFullPathNameA(path, size, buffer, filepart).
pub(super) fn get_full_path_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[name, size, buffer, filepart] = args else {
        unreachable!()
    };
    let name = string_at(cpu, name)?;
    if name.is_empty() {
        set_error(cpu, ERROR_INVALID_NAME);
        return Some(0);
    }
    let full = full_path(cpu, &name);
    put_path(cpu, buffer, size, filepart, &full)
}

/// The 8.3 name of the entry `name` of the directory `dir` (a full
/// path): its own on drives that have no long names.
fn short_name(cpu: &Cpu, dir: &str, name: &str) -> String {
    let names = cpu.bus.disk.long_names(dir).unwrap_or_default();
    let upper = name.to_ascii_uppercase();
    names
        .iter()
        .find(|(long, _)| long.eq_ignore_ascii_case(name))
        .or_else(|| names.iter().find(|(_, short)| *short == upper))
        .map_or(upper, |(_, short)| short.clone())
}

/// The path `path` with each name its 8.3 one, as GetShortPathNameA makes
/// it: a relative path stays relative.
fn short_path(cpu: &Cpu, path: &str) -> String {
    let path = dos_path(path);
    let bytes = path.as_bytes();
    let prefix_len = match bytes {
        [d, b':', b'\\', ..] if d.is_ascii_alphabetic() => 3,
        [d, b':', ..] if d.is_ascii_alphabetic() => 2,
        [b'\\', ..] => 1,
        _ => 0,
    };
    let (prefix, rest) = path.split_at(prefix_len);
    let mut dir = trim_slash(&full_path(
        cpu,
        if prefix.is_empty() { "." } else { prefix },
    ))
    .to_string();
    let mut out = prefix.to_string();
    let parts: Vec<&str> = rest.split('\\').collect();
    for (i, part) in parts.iter().enumerate() {
        let name = match *part {
            "" => String::new(),
            "." | ".." => part.to_string(),
            _ => short_name(cpu, &dir, part),
        };
        out.push_str(&name);
        if i + 1 < parts.len() {
            out.push('\\');
        }
        if !part.is_empty() {
            dir = trim_slash(&full_path(cpu, &format!("{}\\{}", dir, part))).to_string();
        }
    }
    out
}

/// GetShortPathNameA(path, buffer, size): the path of a file or
/// directory that is there in 8.3 names.
pub(super) fn get_short_path_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[name, buffer, size] = args else {
        unreachable!()
    };
    if name == 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    let name = string_at(cpu, name)?;
    if name.is_empty() {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    if has_wildcards(&name) {
        set_error(cpu, ERROR_INVALID_NAME);
        return Some(0);
    }
    let full = full_path(cpu, &dos_path(&name));
    if !cpu.bus.disk.exists(trim_slash(&full)) {
        let code = not_found(cpu, &full);
        set_error(cpu, code);
        return Some(0);
    }
    let short = short_path(cpu, &name);
    put_string(cpu, buffer, size, &short)
}

/// The value of the environment variable `name`, if it is set.
fn environment(cpu: &mut Cpu, name: &str) -> Option<String> {
    let process = crate::win32::process(cpu);
    let i = super::variable(process, name)?;
    let value = process.environment[i].split_once('=')?.1;
    (!value.is_empty()).then(|| value.to_string())
}

/// GetTempPathA(size, buffer): TMP's directory, or TEMP's, or the
/// current one, as a full path ("C:" is that drive's current directory)
/// with a backslash at its end.
pub(super) fn get_temp_path(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let dir = environment(cpu, "TMP")
        .or_else(|| environment(cpu, "TEMP"))
        .unwrap_or_else(|| ".".to_string());
    let mut dir = full_path(cpu, &dir);
    if !dir.ends_with('\\') {
        dir.push('\\');
    }
    put_string(cpu, args[1], args[0], &dir)
}

/// GetTempFileNameA(directory, prefix, unique, buffer): the name
/// "<directory>\<3 of the prefix><number>.TMP" to `buffer`. With
/// `unique` 0 a number no file has yet, whose file it makes, empty;
/// otherwise `unique`, and no file. Windows 95 wants a prefix.
pub(super) fn get_temp_file_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[dir, prefix, unique, buffer] = args else {
        unreachable!()
    };
    let dir = string_at(cpu, dir)?;
    if prefix == 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    let prefix: String = string_at(cpu, prefix)?.chars().take(3).collect();
    let mut base = dir;
    if !base.is_empty() && !base.ends_with(['\\', '/']) {
        base.push('\\');
    }
    let name = |n: u32| format!("{}{}{:X}.TMP", base, prefix, n);
    if unique != 0 {
        let unique = unique & 0xFFFF;
        put_string(cpu, buffer, u32::MAX, &name(unique))?;
        return Some(unique);
    }
    let start = (cpu.bus.clock.now_micros() / 1000) as u32;
    for i in 0..0xFFFF {
        let n = (start.wrapping_add(i) & 0xFFFF).max(1);
        let file = name(n);
        match super::file::open_file(cpu, &file, super::file::GENERIC_WRITE, 0, 1) {
            Ok((sft, _)) => {
                cpu.bus.disk.close_file(sft);
                put_string(cpu, buffer, u32::MAX, &file)?;
                set_error(cpu, 0);
                return Some(n);
            }
            Err(ERROR_FILE_EXISTS | ERROR_ACCESS_DENIED)
                if cpu.bus.disk.exists(&dos_path(&file)) => {}
            Err(code) => {
                set_error(cpu, code);
                return Some(0);
            }
        }
    }
    set_error(cpu, ERROR_FILE_EXISTS);
    Some(0)
}

/// The directories SearchPathA looks in when it isn't given any: the
/// program's, the current one, the system and Windows directories, and
/// those of PATH.
fn search_directories(cpu: &mut Cpu) -> Vec<String> {
    let program = crate::win32::process(cpu).path.clone();
    let mut dirs = vec![
        program
            .rsplit_once('\\')
            .map_or(String::new(), |(d, _)| d.to_string()),
    ];
    dirs.push(current_directory(cpu, cpu.bus.disk.get_current_drive()));
    dirs.push(SYSTEM.to_string());
    dirs.push(WINDOWS.to_string());
    if let Some(path) = environment(cpu, "PATH") {
        dirs.extend(
            path.split(';')
                .filter(|d| !d.is_empty())
                .map(str::to_string),
        );
    }
    dirs
}

/// The full path of the file `name` in the first of `dirs` (separated by
/// semicolons) that has it, or of those `search_directories` gives. A
/// name with a directory in it is looked for there alone.
pub(super) fn search(cpu: &mut Cpu, dirs: Option<&str>, name: &str) -> Option<String> {
    if name.contains(['\\', '/', ':']) {
        let full = full_path(cpu, name);
        return cpu.bus.disk.is_file(&dos_path(&full)).then_some(full);
    }
    let dirs = match dirs {
        Some(dirs) => dirs.split(';').map(str::to_string).collect(),
        None => search_directories(cpu),
    };
    dirs.iter()
        .filter(|d| !d.is_empty())
        .map(|d| full_path(cpu, &format!("{}\\{}", d.trim_end_matches('\\'), name)))
        .find(|full| cpu.bus.disk.is_file(&dos_path(full)))
}

/// SearchPathA(path, name, extension, size, buffer, filepart): the full
/// path of the file `name`, with `extension` added if it has none, in
/// the directories of `path`, or where `search_directories` says.
pub(super) fn search_path(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[dirs, name, extension, size, buffer, filepart] = args else {
        unreachable!()
    };
    let dirs = if dirs == 0 {
        None
    } else {
        Some(string_at(cpu, dirs)?)
    };
    if name == 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    let mut name = string_at(cpu, name)?;
    if extension != 0 {
        let extension = string_at(cpu, extension)?;
        let file = name.rsplit(['\\', '/']).next().unwrap_or("");
        if !file.contains('.') {
            name.push_str(&extension);
        }
    }
    match search(cpu, dirs.as_deref(), &name) {
        Some(full) => put_path(cpu, buffer, size, filepart, &full),
        None => {
            set_error(cpu, ERROR_FILE_NOT_FOUND);
            Some(0)
        }
    }
}

/// The path at `at` as DOS takes it, or with the last error
/// ERROR_PATH_NOT_FOUND, None inside: None if a page isn't there.
pub(super) fn path_arg(cpu: &mut Cpu, at: u32) -> Option<Option<String>> {
    if at == 0 {
        set_error(cpu, ERROR_PATH_NOT_FOUND);
        return Some(None);
    }
    let name = string_at(cpu, at)?;
    if name.is_empty() {
        set_error(cpu, ERROR_PATH_NOT_FOUND);
        return Some(None);
    }
    Some(Some(dos_path(&name)))
}

/// Whether a full path is a drive's root ("C:\").
pub(super) fn is_root(full: &str) -> bool {
    full.len() <= 3
}

/// CreateDirectoryA(path, security).
pub(super) fn create_directory(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(path) = path_arg(cpu, args[0])? else {
        return Some(FALSE);
    };
    let done = make_directory(cpu, &path);
    Some(result(cpu, done))
}

/// TRUE for `Ok`, or FALSE with the error set.
fn result(cpu: &mut Cpu, done: Result<(), u32>) -> u32 {
    match done {
        Ok(()) => TRUE,
        Err(code) => {
            set_error(cpu, code);
            FALSE
        }
    }
}

/// Make the directory `path` (as `dos_path` gives it).
pub(super) fn make_directory(cpu: &mut Cpu, path: &str) -> Result<(), u32> {
    if has_wildcards(path) {
        return Err(ERROR_INVALID_NAME);
    }
    let full = full_path(cpu, path);
    let full = trim_slash(&full);
    let disk = &mut cpu.bus.disk;
    if is_root(full) {
        return Err(ERROR_ACCESS_DENIED);
    }
    if disk.exists(full) {
        return Err(ERROR_ALREADY_EXISTS);
    }
    disk.create_directory(full).map_err(u32::from)
}

/// RemoveDirectoryA(path): an empty directory, not the current one.
pub(super) fn remove_directory(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(path) = path_arg(cpu, args[0])? else {
        return Some(FALSE);
    };
    let done = unmake_directory(cpu, &path);
    Some(result(cpu, done))
}

/// Remove the empty directory `path` (as `dos_path` gives it).
pub(super) fn unmake_directory(cpu: &mut Cpu, path: &str) -> Result<(), u32> {
    if has_wildcards(path) {
        return Err(ERROR_INVALID_NAME);
    }
    let full = full_path(cpu, path);
    let full = trim_slash(&full);
    match cpu.bus.disk.remove_directory(full) {
        Ok(()) => Ok(()),
        // The current directory: in use.
        Err(0x10) => Err(ERROR_ACCESS_DENIED),
        Err(code) => Err(code as u32),
    }
}

/// DeleteFileA(path): a file; a directory is access denied.
pub(super) fn delete_file(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(path) = path_arg(cpu, args[0])? else {
        return Some(FALSE);
    };
    let done = delete(cpu, &path);
    Some(result(cpu, done))
}

/// Delete the file `path` (as `dos_path` gives it).
pub(super) fn delete(cpu: &mut Cpu, path: &str) -> Result<(), u32> {
    let disk = &mut cpu.bus.disk;
    if disk.is_directory(path) {
        return Err(ERROR_ACCESS_DENIED);
    }
    if has_wildcards(path) {
        return Err(ERROR_INVALID_NAME);
    }
    // A read-only file stays, which hosts don't all see to.
    if disk.get_file_attribute(path).is_ok_and(|a| a & 0x01 != 0) {
        return Err(ERROR_ACCESS_DENIED);
    }
    disk.delete_file(path).map_err(u32::from)
}

/// A file's bytes, its DOS time and date and its attributes.
type Whole = (Vec<u8>, (u16, u16), u16);

/// The file `path`, read through DOS.
pub(super) fn read_whole(cpu: &mut Cpu, path: &str) -> Result<Whole, u32> {
    let psp = cpu.bus.win32.psp;
    let disk = &mut cpu.bus.disk;
    if disk.is_directory(path) {
        return Err(ERROR_ACCESS_DENIED);
    }
    let sft = disk.open_file(path, 0x20, psp).map_err(|e| e as u32)?;
    let mut data = Vec::new();
    let result = loop {
        match disk.read_file(sft, 0x8000) {
            Ok(bytes) if bytes.is_empty() => break Ok(()),
            Ok(bytes) => data.extend(bytes),
            Err(code) => break Err(code as u32),
        }
    };
    let stamp = disk.file_time(sft).unwrap_or((0, 0x21));
    disk.close_file(sft);
    let attr = disk.get_file_attribute(path).unwrap_or(0x20);
    result.map(|()| (data, stamp, attr))
}

/// Copy the file `from` to `to` through DOS: its bytes, time and
/// read-only attribute.
fn copy(cpu: &mut Cpu, from: &str, to: &str) -> Result<(), u32> {
    use super::file::{ERROR_SHARING_VIOLATION, GENERIC_READ, GENERIC_WRITE, sharing_violation};
    // The source is read sharing reading and writing, the copy written
    // sharing nothing, as handles of them would be.
    if sharing_violation(cpu, from, GENERIC_READ, 3) {
        return Err(ERROR_SHARING_VIOLATION);
    }
    let (data, stamp, attr) = read_whole(cpu, from)?;
    if cpu.bus.disk.is_file(to) && sharing_violation(cpu, to, GENERIC_WRITE, 0) {
        return Err(ERROR_SHARING_VIOLATION);
    }
    let disk = &mut cpu.bus.disk;
    if disk.is_directory(to) {
        return Err(ERROR_ACCESS_DENIED);
    }
    disk.write_whole_file(to, &data, Some(stamp))
        .map_err(|e| e as u32)?;
    if attr & 0x01 != 0 {
        let _ = disk.set_file_attribute(to, 0x01);
    }
    Ok(())
}

/// CopyFileA(from, to, fail if there): the file's copy, made anew.
pub(super) fn copy_file(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(from) = path_arg(cpu, args[0])? else {
        return Some(FALSE);
    };
    let Some(to) = path_arg(cpu, args[1])? else {
        return Some(FALSE);
    };
    let done = copy_path(cpu, &from, &to, args[2] != 0);
    Some(result(cpu, done))
}

/// Copy the file `from` to `to` (as `dos_path` gives them) as CopyFileA
/// does, failing if `to` is there and `fail_if_there`.
pub(super) fn copy_path(
    cpu: &mut Cpu,
    from: &str,
    to: &str,
    fail_if_there: bool,
) -> Result<(), u32> {
    if !cpu.bus.disk.is_file(from) {
        let full = full_path(cpu, from);
        return Err(not_found(cpu, &full));
    }
    // A file copied to itself: there already.
    let same = full_path(cpu, from).eq_ignore_ascii_case(&full_path(cpu, to));
    if same || (fail_if_there && cpu.bus.disk.exists(to)) {
        return Err(ERROR_FILE_EXISTS);
    }
    copy(cpu, from, to)
}

/// The sharing that the handles CreateFileA opened of the file at `path`
/// all allow (FILE_SHARE_READ, FILE_SHARE_WRITE), or None if none has it
/// open.
pub(super) fn open_sharing(cpu: &Cpu, path: &str) -> Option<u32> {
    let disk = &cpu.bus.disk;
    let path = disk.qualify_path(path)?;
    let objects = cpu.bus.win32.objects.objects.iter().flatten();
    objects
        .filter_map(|o| match o.kind {
            Kind::File { sft, share, .. } => disk
                .open_path(sft)
                .is_some_and(|p| p.eq_ignore_ascii_case(&path))
                .then_some(share),
            _ => None,
        })
        .reduce(|a, b| a & b)
}

/// MoveFileA(from, to): a file or directory renamed, or a file moved to
/// another drive (copied, then deleted). The new name mustn't be there,
/// and Windows 9x refuses to move a file a program has open.
pub(super) fn move_file(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(from) = path_arg(cpu, args[0])? else {
        return Some(FALSE);
    };
    let Some(to) = path_arg(cpu, args[1])? else {
        return Some(FALSE);
    };
    let done = move_path(cpu, &from, &to);
    Some(result(cpu, done))
}

/// Move or rename `from` to `to` (as `dos_path` gives them) as MoveFileA
/// does.
pub(super) fn move_path(cpu: &mut Cpu, from: &str, to: &str) -> Result<(), u32> {
    let (from_full, to_full) = (full_path(cpu, from), full_path(cpu, to));
    let disk = &mut cpu.bus.disk;
    if !disk.exists(from) {
        return Err(not_found(cpu, &from_full));
    }
    if has_wildcards(to) {
        return Err(ERROR_INVALID_NAME);
    }
    if from_full.eq_ignore_ascii_case(&to_full) {
        return Ok(());
    }
    if open_sharing(cpu, from).is_some() {
        return Err(ERROR_ACCESS_DENIED);
    }
    let disk = &mut cpu.bus.disk;
    if disk.exists(to) {
        return Err(ERROR_ALREADY_EXISTS);
    }
    if drive_of(&from_full) == drive_of(&to_full) {
        disk.rename_file(&from_full, &to_full)
            .map_err(|e| e as u32)
    } else if disk.is_directory(from) {
        Err(ERROR_NOT_SAME_DEVICE)
    } else {
        copy(cpu, &from_full, &to_full)
            .and_then(|()| cpu.bus.disk.delete_file(&from_full).map_err(|e| e as u32))
    }
}

/// GetFileAttributesA(path): FILE_ATTRIBUTE_* as DOS's attributes have
/// them, or INVALID_FILE_ATTRIBUTES.
pub(super) fn get_file_attributes(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(path) = path_arg(cpu, args[0])? else {
        return Some(INVALID_FILE_ATTRIBUTES);
    };
    match attributes(cpu, &path) {
        Ok(attr) => Some(attr),
        Err(code) => {
            set_error(cpu, code);
            Some(INVALID_FILE_ATTRIBUTES)
        }
    }
}

/// The attributes of `path` (as `dos_path` gives it), as
/// GetFileAttributesA has them.
pub(super) fn attributes(cpu: &mut Cpu, path: &str) -> Result<u32, u32> {
    if has_wildcards(path) {
        return Err(ERROR_INVALID_NAME);
    }
    let full = full_path(cpu, path);
    let disk = &cpu.bus.disk;
    if is_root(&full) {
        return if disk.is_mounted(drive_of(&full)) {
            Ok(FILE_ATTRIBUTE_DIRECTORY)
        } else {
            Err(ERROR_PATH_NOT_FOUND)
        };
    }
    disk.get_file_attribute(trim_slash(&full))
        .map(u32::from)
        .map_err(u32::from)
}

/// SetFileAttributesA(path, attributes): DOS's (read-only, hidden,
/// system, archive), of which host drives keep read-only.
pub(super) fn set_file_attributes(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(path) = path_arg(cpu, args[0])? else {
        return Some(FALSE);
    };
    let attr = if args[1] == FILE_ATTRIBUTE_NORMAL {
        0
    } else {
        args[1] & 0x27
    };
    let done = set_attributes(cpu, &path, attr);
    Some(result(cpu, done))
}

/// Set the DOS attributes of `path` (as `dos_path` gives it).
pub(super) fn set_attributes(cpu: &mut Cpu, path: &str, attr: u32) -> Result<(), u32> {
    cpu.bus
        .disk
        .set_file_attribute(path, attr as u16)
        .map_err(u32::from)
}

/// Whether `name` matches the pattern `pattern` as Windows 95 matches
/// them: '*' any characters, '?' one or none at the end of a name or its
/// extension, "*.*" anything, and a '.' at the pattern's end a name with
/// no extension. Case doesn't count.
pub(crate) fn matches(name: &str, pattern: &str) -> bool {
    fn glob(n: &[u8], p: &[u8]) -> bool {
        match p.split_first() {
            None => n.is_empty(),
            Some((b'*', rest)) => (0..=n.len()).any(|i| glob(&n[i..], rest)),
            Some((b'?', rest)) => {
                (!n.is_empty() && glob(&n[1..], rest))
                    || ((n.is_empty() || n[0] == b'.') && glob(n, rest))
            }
            Some((b'.', rest)) if n.is_empty() => glob(n, rest),
            Some((c, rest)) => {
                n.first().is_some_and(|b| b.eq_ignore_ascii_case(c)) && glob(&n[1..], rest)
            }
        }
    }
    pattern == "*.*" || glob(name.as_bytes(), pattern.as_bytes())
}

/// The entries of the directory `dir` (a full path) whose long or short
/// names match `pattern`.
pub(crate) fn find_entries(cpu: &Cpu, dir: &str, pattern: &str) -> Result<Vec<FindEntry>, u32> {
    let disk = &cpu.bus.disk;
    let spec = format!("{}\\*.*", dir.trim_end_matches('\\'));
    let listed = disk.list_directory(&spec, 0x16).map_err(|e| e as u32)?;
    let long = disk.long_names(dir).unwrap_or_default();
    let mut entries = Vec::new();
    for entry in listed {
        let name = long
            .iter()
            .find(|(_, short)| *short == entry.filename)
            .map_or(entry.filename.clone(), |(long, _)| long.clone());
        if !matches(&name, pattern) && !matches(&entry.filename, pattern) {
            continue;
        }
        let short = if name == entry.filename {
            String::new()
        } else {
            entry.filename.clone()
        };
        entries.push(FindEntry {
            attributes: entry.attr as u32,
            date: entry.dos_date,
            time: entry.dos_time,
            size: entry.size,
            name,
            short,
        });
    }
    // A directory's own entries come first, "." before "..", as they do
    // on FAT; the DOS listing has them the other way round.
    let dots = |e: &FindEntry| match e.name.as_str() {
        "." => 0,
        ".." => 1,
        _ => 2,
    };
    entries.sort_by_key(dots);
    Ok(entries)
}

/// Write `entry` as the WIN32_FIND_DATAA at `at`.
fn write_find_data(cpu: &mut Cpu, at: u32, entry: &FindEntry) -> Option<()> {
    if !cpu.bus.guest_probe(at, FIND_DATA, true) {
        return None;
    }
    cpu.bus.guest_fill(at, FIND_DATA, 0);
    cpu.bus.guest_write_32(at, entry.attributes);
    let ticks = crate::win32::convert::dos_to_ticks(entry.date, entry.time).unwrap_or(0);
    for time in 0..3 {
        cpu.bus.guest_write_32(at + 4 + 8 * time, ticks as u32);
        cpu.bus
            .guest_write_32(at + 8 + 8 * time, (ticks >> 32) as u32);
    }
    cpu.bus.guest_write_32(at + 32, entry.size);
    let mut name = crate::dosstr::to_bytes(&entry.name);
    name.truncate(MAX_PATH - 1);
    cpu.bus.guest_write_bytes(at + FIND_NAME, &name);
    let mut short = crate::dosstr::to_bytes(&entry.short);
    short.truncate(13);
    cpu.bus.guest_write_bytes(at + FIND_SHORT_NAME, &short);
    Some(())
}

/// FindFirstFileA(pattern, data): the first file or directory whose name
/// matches the last part of `pattern` in the directory before it, to
/// `data`, and a handle for FindNextFileA; a device's name finds the
/// device. INVALID_HANDLE_VALUE with ERROR_FILE_NOT_FOUND when nothing
/// matches, ERROR_PATH_NOT_FOUND when there is no such directory.
pub(super) fn find_first_file(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(path) = path_arg(cpu, args[0])? else {
        return Some(INVALID_HANDLE_VALUE);
    };
    let full = full_path(cpu, &path);
    let (dir, pattern) = match full.rfind('\\') {
        Some(i) if i <= 2 => (&full[..3], &full[i + 1..]),
        Some(i) => (&full[..i], &full[i + 1..]),
        None => (full.as_str(), ""),
    };
    let disk = &cpu.bus.disk;
    let found = if !disk.is_directory(dir) {
        Err(ERROR_PATH_NOT_FOUND)
    } else if pattern.is_empty() {
        Err(ERROR_FILE_NOT_FOUND)
    } else if disk.device(pattern).is_some() && !has_wildcards(pattern) {
        Ok(vec![FindEntry {
            attributes: FILE_ATTRIBUTE_DEVICE,
            name: pattern.to_string(),
            date: 0x21,
            ..FindEntry::default()
        }])
    } else {
        find_entries(cpu, dir, pattern).and_then(|e| {
            if e.is_empty() {
                Err(ERROR_FILE_NOT_FOUND)
            } else {
                Ok(e)
            }
        })
    };
    let mut entries = match found {
        Ok(entries) => entries,
        Err(code) => {
            set_error(cpu, code);
            return Some(INVALID_HANDLE_VALUE);
        }
    };
    let first = entries.remove(0);
    write_find_data(cpu, args[1], &first)?;
    Some(object::create(cpu, Kind::Find { entries }, None))
}

/// FindNextFileA(handle, data): the search's next entry, or
/// ERROR_NO_MORE_FILES.
pub(super) fn find_next_file(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = object::resolve(cpu, args[0]);
    let entries = match id.and_then(|id| object::objects(cpu).get_mut(id)) {
        Some(object::Object {
            kind: Kind::Find { entries },
            ..
        }) => entries,
        _ => {
            set_error(cpu, ERROR_INVALID_HANDLE);
            return Some(FALSE);
        }
    };
    let Some(next) = entries.first().cloned() else {
        set_error(cpu, ERROR_NO_MORE_FILES);
        return Some(FALSE);
    };
    write_find_data(cpu, args[1], &next)?;
    if let Some(id) = id
        && let Some(object::Object {
            kind: Kind::Find { entries },
            ..
        }) = object::objects(cpu).get_mut(id)
    {
        entries.remove(0);
    }
    Some(TRUE)
}

/// FindClose(handle): the search's handle goes.
pub(super) fn find_close(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if !matches!(object::kind_of(cpu, args[0]), Some(Kind::Find { .. })) {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    }
    object::close(cpu, args[0]);
    Some(TRUE)
}
