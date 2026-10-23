//! SHFileOperationA: copying, moving, renaming and deleting files and
//! folders, with wildcards, many at once, as setup and uninstall
//! programs ask the shell to. Ported from Wine's shlfileop.c:
//!
//! Copyright 2000 Juergen Schmied
//! Copyright 2002 Andriy Palamarchuk
//! Copyright 2004 Dietrich Teickner (from Odin)
//! Copyright 2004 Rolf Kalbermatter
//!
//! This library is free software; you can redistribute it and/or modify
//! it under the terms of the GNU Lesser General Public License as
//! published by the Free Software Foundation; either version 2.1 of the
//! License, or (at your option) any later version.
//!
//! The shell asks before it overwrites or deletes unless FOF_NOCONFIRMATION
//! says not to, and shows its progress unless FOF_SILENT; here the answer
//! is always yes and there is no progress window. Deleted files don't go
//! to the Recycle Bin, as there is none.

use crate::cpu::Cpu;
use crate::win32::kernel32::files;
use crate::win32::user32::{bytes_at, string_at};

const FO_MOVE: u32 = 1;
const FO_COPY: u32 = 2;
const FO_DELETE: u32 = 3;
const FO_RENAME: u32 = 4;

const FOF_MULTIDESTFILES: u32 = 0x0001;
const FOF_FILESONLY: u32 = 0x0080;

const ERROR_SUCCESS: u32 = 0;
const ERROR_PATH_NOT_FOUND: u32 = 3;
const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_NO_MORE_SEARCH_HANDLES: u32 = 113;
const ERROR_ALREADY_EXISTS: u32 = 183;
const ERROR_GEN_FAILURE: u32 = 31;
const ERROR_CANCELLED: u32 = 1223;
/// What the shell returns for a file to work on that isn't there.
const ERROR_SHELL_INTERNAL_FILE_NOT_FOUND: u32 = 1026;

const DIRECTORY: u32 = 0x10;
const INVALID: u32 = 0xFFFF_FFFF;

fn is_dir(attributes: u32) -> bool {
    attributes != INVALID && attributes & DIRECTORY != 0
}

/// Wine's IsAttribFile: attributes that don't say "directory", which
/// those of something that isn't there do.
fn is_file(attributes: u32) -> bool {
    attributes & DIRECTORY == 0
}

fn exists(cpu: &mut Cpu, path: &str) -> bool {
    files::attributes(cpu, path).is_ok()
}

fn attributes(cpu: &mut Cpu, path: &str) -> u32 {
    files::attributes(cpu, path).unwrap_or(INVALID)
}

/// `dir` and `name` joined by one backslash.
fn combine(dir: &str, name: &str) -> String {
    if dir.ends_with('\\') {
        format!("{}{}", dir, name)
    } else {
        format!("{}\\{}", dir, name)
    }
}

/// A file or folder of a list.
#[derive(Clone, Debug, Default)]
struct Entry {
    attributes: u32,
    directory: String,
    name: String,
    full: String,
    from_wildcard: bool,
    from_relative: bool,
    exists: bool,
}

impl Entry {
    fn new(full: &str) -> Entry {
        let (directory, name) = match full.rfind('\\') {
            Some(i) => (full[..i].to_string(), full[i + 1..].to_string()),
            None => (String::new(), String::new()),
        };
        Entry {
            directory,
            name,
            full: full.to_string(),
            ..Entry::default()
        }
    }
}

#[derive(Debug, Default)]
struct List {
    entries: Vec<Entry>,
    any_wildcard: bool,
    any_directory: bool,
    any_missing: bool,
}

/// The strings of a double-0-ended list at `at`.
fn strings(cpu: &mut Cpu, at: u32) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut p = at;
    loop {
        // By the string's bytes: a character above 7Fh is two bytes of
        // the String.
        let s = bytes_at(cpu, p)?;
        if s.is_empty() {
            return Some(out);
        }
        p += s.len() as u32 + 1;
        out.push(crate::dosstr::from_bytes(&s));
        if out.len() > 0x1000 {
            return Some(out);
        }
    }
}

fn is_relative(path: &str) -> bool {
    let b = path.as_bytes();
    !(b.first() == Some(&b'\\') || (b.len() >= 2 && b[1] == b':'))
}

/// The list of files and folders that `names` give, wildcards found.
fn parse(cpu: &mut Cpu, names: Option<Vec<String>>) -> Result<List, u32> {
    let names = names.ok_or(ERROR_INVALID_PARAMETER)?;
    if names.is_empty() {
        return Err(ERROR_ACCESS_DENIED);
    }
    let mut list = List::default();
    for name in names {
        let relative = is_relative(&name);
        let path = if relative {
            let current = files::full_path(cpu, ".");
            combine(&current, &name)
        } else {
            name
        };
        if path.contains(['*', '?']) {
            list.any_wildcard = true;
            let i = path.rfind('\\').unwrap_or(0);
            let dir_part = &path[..path.rfind('\\').map_or(0, |i| i + 1)];
            // A drive's root keeps its backslash.
            let dir = if i <= 2 { dir_part } else { &path[..i] };
            let pattern = &path[dir_part.len()..];
            for found in files::find(cpu, dir, pattern).unwrap_or_default() {
                if found.name == "." || found.name == ".." {
                    continue;
                }
                let mut entry = Entry::new(&format!("{}{}", dir_part, found.name));
                entry.from_wildcard = true;
                entry.from_relative = relative;
                entry.attributes = found.attributes;
                entry.exists = true;
                list.any_directory |= is_dir(entry.attributes);
                list.entries.push(entry);
            }
        } else {
            let mut entry = Entry::new(&path);
            entry.from_relative = relative;
            entry.attributes = attributes(cpu, &path);
            entry.exists = entry.attributes != INVALID;
            list.any_missing |= !entry.exists;
            list.any_directory |= is_dir(entry.attributes);
            list.entries.push(entry);
        }
    }
    Ok(list)
}

/// MoveFileEx with MOVEFILE_REPLACE_EXISTING, then MoveFile.
fn move_file(cpu: &mut Cpu, from: &str, to: &str) -> u32 {
    if files::move_file(cpu, from, to).is_ok() {
        return ERROR_SUCCESS;
    }
    let there = attributes(cpu, to);
    if there != INVALID && !is_dir(there) && !is_dir(attributes(cpu, from)) {
        let _ = files::set_attributes(cpu, to, 0);
        if files::delete_file(cpu, to).is_ok() && files::move_file(cpu, from, to).is_ok() {
            return ERROR_SUCCESS;
        }
    }
    match files::move_file(cpu, from, to) {
        Ok(()) => ERROR_SUCCESS,
        Err(code) => code,
    }
}

/// Copy a file over whatever is there, read-only or not.
fn copy_file(cpu: &mut Cpu, from: &str, to: &str) -> u32 {
    let there = attributes(cpu, to);
    if there != INVALID && there & 1 != 0 {
        let _ = files::set_attributes(cpu, to, there & !1 & 0x27);
    }
    match files::copy_file(cpu, from, to, false) {
        Ok(()) => ERROR_SUCCESS,
        Err(code) => code,
    }
}

/// Delete a folder and everything in it.
fn delete_directory(cpu: &mut Cpu, dir: &str) -> bool {
    let Ok(found) = files::find(cpu, dir, "*") else {
        return false;
    };
    for entry in found {
        if entry.name == "." || entry.name == ".." {
            continue;
        }
        let path = combine(dir, &entry.name);
        let done = if is_dir(entry.attributes) {
            delete_directory(cpu, &path)
        } else {
            files::delete_file(cpu, &path).is_ok()
        };
        if !done {
            return false;
        }
    }
    files::remove_directory(cpu, dir).is_ok()
}

/// Make `dir` and the folders it is in.
fn create_dest_dirs(cpu: &mut Cpu, dir: &str) {
    let mut at = dir.find('\\');
    while let Some(i) = at {
        match dir[i + 1..].find('\\') {
            Some(j) => {
                let end = i + 1 + j;
                if !exists(cpu, &dir[..end]) {
                    let _ = files::create_directory(cpu, &dir[..end]);
                }
                at = Some(end);
            }
            None => break,
        }
    }
    if !dir.is_empty() && !exists(cpu, dir) {
        let _ = files::create_directory(cpu, dir);
    }
}

/// What an operation was asked.
struct Op {
    func: u32,
    flags: u32,
    aborted: bool,
}

/// One operation from the lists of names `from` and `to`.
fn operate(cpu: &mut Cpu, op: &mut Op, from: Option<Vec<String>>, to: Option<Vec<String>>) -> u32 {
    let from = match parse(cpu, from) {
        Ok(list) => list,
        Err(code) => return code,
    };
    let mut to = if op.func == FO_DELETE {
        List::default()
    } else {
        parse(cpu, to).unwrap_or_default()
    };
    let result = match op.func {
        FO_COPY => copy_files(cpu, op, &from, &mut to),
        FO_DELETE => delete_files(cpu, op, &from),
        FO_MOVE => move_files(cpu, op, &from, &to),
        FO_RENAME => rename_files(cpu, &from, &to),
        _ => ERROR_INVALID_PARAMETER,
    };
    if result == ERROR_CANCELLED {
        op.aborted = true;
    }
    result
}

fn copy_dir_to_dir(cpu: &mut Cpu, op: &mut Op, from: &Entry, dest: &str) {
    if from.name == "." || from.name == ".." {
        return;
    }
    let to = if exists(cpu, dest) {
        combine(dest, &from.name)
    } else {
        dest.to_string()
    };
    let _ = files::create_directory(cpu, &to);
    let sources = vec![combine(&from.full, "*.*")];
    let mut inner = Op {
        func: FO_COPY,
        flags: op.flags & !FOF_MULTIDESTFILES,
        aborted: false,
    };
    operate(cpu, &mut inner, Some(sources), Some(vec![to]));
}

fn copy_to_dir(cpu: &mut Cpu, op: &mut Op, from: &Entry, to: &Entry) {
    if !exists(cpu, &to.full) {
        let _ = files::create_directory(cpu, &to.full);
    }
    if is_file(from.attributes) {
        let dest = combine(&to.full, &from.name);
        copy_file(cpu, &from.full, &dest);
    } else if !(op.flags & FOF_FILESONLY != 0 && from.from_wildcard) {
        copy_dir_to_dir(cpu, op, from, &to.full);
    }
}

/// FO_COPY.
fn copy_files(cpu: &mut Cpu, op: &mut Op, from: &List, to: &mut List) -> u32 {
    if from.any_missing {
        return ERROR_SHELL_INTERNAL_FILE_NOT_FOUND;
    }
    if to.entries.is_empty() {
        // No destination: the current directory.
        let current = files::full_path(cpu, ".");
        *to = parse(cpu, Some(vec![current])).unwrap_or_default();
    }
    let Some(first) = to.entries.first().cloned() else {
        return ERROR_CANCELLED;
    };
    if op.flags & FOF_MULTIDESTFILES != 0 {
        if from.any_wildcard {
            return ERROR_CANCELLED;
        }
        if from.entries.len() != to.entries.len() {
            if from.entries.len() != 1 && !is_dir(first.attributes) {
                return ERROR_CANCELLED;
            }
            to.entries.truncate(1);
        } else if is_dir(first.attributes) {
            for i in 1..to.entries.len() {
                if !is_dir(to.entries[i].attributes) || !is_dir(from.entries[i].attributes) {
                    return ERROR_CANCELLED;
                }
            }
        }
    } else if from.entries.len() != 1 {
        if to.entries.len() != 1 && !is_dir(first.attributes) {
            return ERROR_CANCELLED;
        }
        if exists(cpu, &first.full) && is_file(first.attributes) {
            return ERROR_CANCELLED;
        }
        if to.entries.len() == 1 && first.from_relative && !exists(cpu, &first.full) {
            return ERROR_CANCELLED;
        }
    }
    let mut dest = first;
    for (i, entry) in from.entries.iter().enumerate() {
        if op.flags & FOF_MULTIDESTFILES != 0 && to.entries.len() > 1 {
            dest = to.entries[i].clone();
        }
        if is_dir(entry.attributes) && entry.full.eq_ignore_ascii_case(&dest.directory) {
            return ERROR_SUCCESS;
        }
        create_dest_dirs(cpu, &dest.directory);
        if entry.full.eq_ignore_ascii_case(&dest.full) {
            return if is_file(entry.attributes) {
                ERROR_NO_MORE_SEARCH_HANDLES
            } else {
                ERROR_SUCCESS
            };
        }
        if (from.entries.len() > 1 && to.entries.len() == 1) || is_dir(dest.attributes) {
            copy_to_dir(cpu, op, entry, &dest);
        } else if is_dir(entry.attributes) {
            copy_dir_to_dir(cpu, op, entry, &dest.full);
        } else if copy_file(cpu, &entry.full, &dest.full) != ERROR_SUCCESS {
            op.aborted = true;
            return ERROR_CANCELLED;
        }
    }
    ERROR_SUCCESS
}

/// FO_DELETE.
fn delete_files(cpu: &mut Cpu, op: &mut Op, from: &List) -> u32 {
    if from.entries.is_empty() {
        return ERROR_SUCCESS;
    }
    if from.any_missing {
        return ERROR_SHELL_INTERNAL_FILE_NOT_FOUND;
    }
    for entry in &from.entries {
        if !is_file(entry.attributes) && op.flags & FOF_FILESONLY != 0 && entry.from_wildcard {
            continue;
        }
        let done = if is_file(entry.attributes) {
            files::delete_file(cpu, &entry.full).is_ok()
        } else {
            delete_directory(cpu, &entry.full)
        };
        if !done {
            return ERROR_PATH_NOT_FOUND;
        }
    }
    ERROR_SUCCESS
}

fn move_dir_to_dir(cpu: &mut Cpu, op: &mut Op, from: &Entry, dest: &str) {
    if from.name == "." || from.name == ".." {
        return;
    }
    let _ = files::create_directory(cpu, dest);
    let mut inner = Op {
        func: FO_MOVE,
        flags: op.flags,
        aborted: false,
    };
    operate(
        cpu,
        &mut inner,
        Some(vec![combine(&from.full, "*.*")]),
        Some(vec![dest.to_string()]),
    );
    // What was moved out leaves the folder empty.
    let _ = files::remove_directory(cpu, &from.full);
}

fn move_to_dir(cpu: &mut Cpu, op: &mut Op, from: &Entry, to: &Entry) {
    let dest = combine(&to.full, &from.name);
    if is_file(from.attributes) {
        move_file(cpu, &from.full, &dest);
    } else if !(op.flags & FOF_FILESONLY != 0 && from.from_wildcard) {
        move_dir_to_dir(cpu, op, from, &dest);
    }
}

/// FO_MOVE.
fn move_files(cpu: &mut Cpu, op: &mut Op, from: &List, to: &List) -> u32 {
    let multi = op.flags & FOF_MULTIDESTFILES != 0;
    if from.entries.is_empty() || to.entries.is_empty() {
        return ERROR_CANCELLED;
    }
    if !multi && to.entries.len() > 1 && from.entries.len() > 1 {
        return ERROR_CANCELLED;
    }
    if !multi && !from.any_directory && from.entries.len() > to.entries.len() {
        return ERROR_CANCELLED;
    }
    if !exists(cpu, &to.entries[0].directory) {
        return ERROR_CANCELLED;
    }
    if multi && from.entries.len() != to.entries.len() {
        return ERROR_CANCELLED;
    }
    for (i, entry) in from.entries.iter().enumerate() {
        let dest = if multi {
            &to.entries[i]
        } else {
            &to.entries[0]
        };
        if !exists(cpu, &dest.directory) {
            return ERROR_CANCELLED;
        }
        if dest.exists && is_dir(dest.attributes) {
            move_to_dir(cpu, op, entry, dest);
        } else {
            move_file(cpu, &entry.full, &dest.full);
        }
    }
    ERROR_SUCCESS
}

/// FO_RENAME.
fn rename_files(cpu: &mut Cpu, from: &List, to: &List) -> u32 {
    if from.entries.len() != 1 {
        return ERROR_GEN_FAILURE;
    }
    if to.entries.len() != 1 {
        return ERROR_CANCELLED;
    }
    let (a, b) = (&from.entries[0], &to.entries[0]);
    if !a.exists {
        return ERROR_SHELL_INTERNAL_FILE_NOT_FOUND;
    }
    if b.exists {
        return ERROR_ALREADY_EXISTS;
    }
    move_file(cpu, &a.full, &b.full)
}

/// SHFileOperationA(op): the SHFILEOPSTRUCTA (packed to bytes) at
/// `op`; 0 when done, or an error.
pub fn sh_file_operation(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[0];
    if at == 0 || !cpu.bus.guest_probe(at, 30, true) {
        return Some(ERROR_INVALID_PARAMETER);
    }
    let func = cpu.bus.guest_read_32(at + 4);
    let from_at = cpu.bus.guest_read_32(at + 8);
    let to_at = cpu.bus.guest_read_32(at + 12);
    let flags = cpu.bus.guest_read_16(at + 16) as u32;
    let from = if from_at == 0 {
        None
    } else {
        strings(cpu, from_at)
    };
    let to = if to_at == 0 || func == FO_DELETE {
        None
    } else {
        strings(cpu, to_at)
    };
    let mut op = Op {
        func,
        flags,
        aborted: false,
    };
    let result = operate(cpu, &mut op, from, to);
    cpu.bus.guest_write_32(at + 18, op.aborted as u32);
    cpu.bus.guest_write_32(at + 22, 0);
    cpu.bus.log_string(&format!(
        "[W32] SHFileOperation {} flags {:04X}: {}",
        func, flags, result
    ));
    Some(result)
}

/// SHCreateDirectory(hwnd, path) and SHCreateDirectoryExA(hwnd, path,
/// security): the folder and those it is in; ANSI on Windows 9x.
pub fn sh_create_directory(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    const ERROR_BAD_PATHNAME: u32 = 161;
    const ERROR_FILE_EXISTS: u32 = 80;
    let Some(path) = (args[1] != 0).then(|| string_at(cpu, args[1])).flatten() else {
        return Some(ERROR_BAD_PATHNAME);
    };
    if is_relative(&path) || path.starts_with('\\') {
        return Some(ERROR_BAD_PATHNAME);
    }
    let there = attributes(cpu, &path);
    if there != INVALID {
        return Some(if is_dir(there) {
            ERROR_ALREADY_EXISTS
        } else {
            ERROR_FILE_EXISTS
        });
    }
    create_dest_dirs(cpu, &files::full_path(cpu, &path));
    Some(if exists(cpu, &path) {
        ERROR_SUCCESS
    } else {
        ERROR_PATH_NOT_FOUND
    })
}
