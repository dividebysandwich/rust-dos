//! ShellExecuteA, ShellExecuteExA and FindExecutableA: a program run, or
//! a document opened with the program its type's verb names in the
//! registry (HKEY_CLASSES_ROOT\.ext names the type, the type's
//! shell\<verb>\command the command line).
//!
//! Ported from Wine's dlls/shell32/shlexec.c and classes.c:
//!
//! Copyright 1998 Marcus Meissner
//! Copyright 2002 Eric Pouech
//!
//! This library is free software; you can redistribute it and/or
//! modify it under the terms of the GNU Lesser General Public
//! License as published by the Free Software Foundation; either
//! version 2.1 of the License, or (at your option) any later version.
//!
//! This library is distributed in the hope that it will be useful,
//! but WITHOUT ANY WARRANTY; without even the implied warranty of
//! MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU
//! Lesser General Public License for more details.
//!
//! Differences from Wine: environment strings in the file name are
//! expanded only with SEE_MASK_DOENVSUBST, as Windows does; the whole
//! file name is looked for before a name with spaces is split into a
//! program and its arguments; a shell link runs what it links to.
//! There are no DDE conversations with the program a type names
//! (ddeexec), no "Open With" dialog and no error message boxes: the
//! program is started with its command line alone.

use super::link;
use crate::cpu::Cpu;
use crate::win32::kernel32::files;
use crate::win32::registry::{self, HKEY_CLASSES_ROOT, Open};
use crate::win32::user32::string_at;

const SE_ERR_FNF: u32 = 2;
const SE_ERR_PNF: u32 = 3;
const SE_ERR_NOASSOC: u32 = 31;
const ERROR_BAD_FORMAT: u32 = 11;
/// What ShellExecuteA returns once the program started. Windows 95
/// returns the program's instance handle; tests written on NT want 33,
/// and programs only compare it with 32.
const STARTED: u32 = 33;

const SEE_MASK_CLASSNAME: u32 = 0x1;
const SEE_MASK_CLASSALL: u32 = 0x3;
const SEE_MASK_IDLIST: u32 = 0x4;
const SEE_MASK_NOCLOSEPROCESS: u32 = 0x40;
const SEE_MASK_DOENVSUBST: u32 = 0x200;

/// The extensions of files that are programs, as win.ini's
/// [windows] programs= lists them.
const PROGRAMS: [&str; 4] = ["exe", "com", "bat", "pif"];

/// What a ShellExecuteExA call asks for.
#[derive(Default)]
struct Request {
    mask: u32,
    verb: Option<String>,
    file: String,
    parameters: String,
    directory: String,
    show: u32,
    class: Option<String>,
    id_list: u32,
}

/// The default value of the key `path` under HKEY_CLASSES_ROOT.
fn class_value(cpu: &mut Cpu, path: &str) -> Option<String> {
    registry::string_value(cpu, HKEY_CLASSES_ROOT, path, "")
}

fn class_key_exists(cpu: &mut Cpu, path: &str) -> bool {
    registry::key_exists(cpu, HKEY_CLASSES_ROOT, path)
}

/// The first subkey of the key `path` under HKEY_CLASSES_ROOT.
fn first_subkey(cpu: &mut Cpu, path: &str) -> Option<String> {
    let registry = registry::get(cpu);
    let Open::Key(root) = registry.open(HKEY_CLASSES_ROOT, 0) else {
        return None;
    };
    let key = registry.find(root, path)?;
    let child = *registry.key(key).children.first()?;
    Some(registry.key(child).name.clone())
}

/// The verb to use for the type `class`: `verb` if given, else the
/// default its shell key names, else "open", else its first verb
/// (Wine's HCR_GetDefaultVerbW).
fn default_verb(cpu: &mut Cpu, class: &str, verb: Option<&str>) -> Option<String> {
    if let Some(verb) = verb {
        return Some(verb.to_string());
    }
    let shell = format!("{}\\shell", class);
    if let Some(named) = class_value(cpu, &shell).filter(|v| !v.is_empty())
        && class_key_exists(cpu, &format!("{}\\{}\\command", shell, named))
    {
        return Some(named);
    }
    if class_key_exists(cpu, &format!("{}\\open\\command", shell)) {
        return Some("open".to_string());
    }
    first_subkey(cpu, &shell)
}

/// The command line the type `class` has for `verb`.
fn class_command(cpu: &mut Cpu, class: &str, verb: Option<&str>) -> Option<String> {
    let verb = default_verb(cpu, class, verb)?;
    class_value(cpu, &format!("{}\\shell\\{}\\command", class, verb))
}

/// The command line `format` with its escapes replaced, as Wine's
/// SHELL_ArgifyW does: %1 and %L the file, %2 to %9 the arguments one by
/// one, %* all of them, %I the item id list, %NAME% an environment
/// variable. Also whether the file went in.
fn argify(
    cpu: &mut Cpu,
    format: &str,
    file: &str,
    id_list: u32,
    args: Option<&str>,
) -> (String, bool) {
    let mut out = String::new();
    let mut found_file = false;
    let mut done = false;
    let mut args = args;
    let chars: Vec<char> = format.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '%' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        i += 1;
        let c = chars.get(i).copied().unwrap_or('\0');
        match c {
            '\0' | '%' => out.push('%'),
            '2'..='9' | '0' | '*' if args.is_some() => {
                let rest = args.unwrap_or("");
                if c == '*' {
                    out.push('"');
                    out.push_str(rest);
                    out.push('"');
                    args = Some("");
                } else {
                    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
                    out.push_str(&rest[..end]);
                    args = Some(rest[end..].trim_start());
                }
            }
            '1'..='9' | '0' | '*' => {
                if !done || c == '1' {
                    let path = files::search(cpu, None, file).unwrap_or(file.to_string());
                    out.push_str(&path);
                }
                found_file = true;
            }
            'l' | 'L' => {
                out.push_str(file);
                found_file = true;
            }
            'i' | 'I' => {
                if id_list != 0 {
                    out.push_str(&format!(":{}", id_list));
                }
                found_file = true;
            }
            _ => {
                let rest: String = chars[i..].iter().collect();
                if let Some(end) = rest.find('%') {
                    let name = &rest[..end];
                    let value = files::expand(cpu, &format!("%{}%", name));
                    if value == format!("%{}%", name) {
                        out.push_str(name);
                    } else {
                        out.push_str(&value);
                    }
                    i += end;
                }
                done = true;
            }
        }
        if i < chars.len() {
            i += 1;
        }
    }
    (out, found_file)
}

/// The file `name` as SearchPathA finds it in `directory` and then the
/// usual places, ".exe" added if it has no extension.
fn search(cpu: &mut Cpu, directory: &str, name: &str) -> Option<String> {
    let file = name.rsplit(['\\', '/']).next().unwrap_or(name);
    let candidates = if file.contains('.') {
        vec![name.to_string()]
    } else {
        vec![name.to_string(), format!("{}.exe", name)]
    };
    for candidate in candidates {
        let dirs = (!directory.is_empty()).then_some(directory);
        if let Some(found) = files::search(cpu, dirs, &candidate) {
            return Some(found);
        }
        if dirs.is_some()
            && let Some(found) = files::search(cpu, None, &candidate)
        {
            return Some(found);
        }
    }
    None
}

fn extension(path: &str) -> Option<&str> {
    let file = path.rsplit(['\\', '/']).next().unwrap_or(path);
    file.rsplit_once('.')
        .map(|(_, e)| e)
        .filter(|e| !e.is_empty())
}

pub fn is_program(path: &str) -> bool {
    extension(path).is_some_and(|e| PROGRAMS.iter().any(|p| e.eq_ignore_ascii_case(p)))
}

fn is_directory(cpu: &mut Cpu, path: &str) -> bool {
    files::attributes(cpu, path).is_ok_and(|a| a & 0x10 != 0)
}

/// What FindExecutable finds for the file `path`, found already: the
/// command line its type has for `verb`, escapes replaced, or the
/// SE_ERR_ code.
fn find_command(
    cpu: &mut Cpu,
    path: &str,
    verb: Option<&str>,
    id_list: u32,
    args: Option<&str>,
) -> Result<String, u32> {
    let class = if is_directory(cpu, path) {
        "Folder".to_string()
    } else {
        let ext = extension(path).ok_or(SE_ERR_NOASSOC)?;
        if PROGRAMS.iter().any(|p| ext.eq_ignore_ascii_case(p)) {
            return Ok(path.to_string());
        }
        class_value(cpu, &format!(".{}", ext))
            .filter(|c| !c.is_empty())
            .ok_or(SE_ERR_NOASSOC)?
    };
    let command = class_command(cpu, &class, verb).ok_or(SE_ERR_NOASSOC)?;
    Ok(argify(cpu, &command, path, id_list, args).0)
}

/// Run the command line `line` in `directory`, if that is one: the
/// process's handle, or the SE_ERR_ code.
fn run(cpu: &mut Cpu, line: &str, directory: &str, show: u32) -> Result<u32, u32> {
    let before = files::current_directory(cpu);
    let moved = !directory.is_empty()
        && is_directory(cpu, directory)
        && files::change_directory(cpu, directory).is_ok();
    cpu.bus
        .log_string(&format!("[W32] ShellExecute runs {}", line));
    let started = files::run(cpu, line, show);
    if moved {
        let _ = files::change_directory(cpu, &before);
    }
    started.map_err(|code| match code {
        2 => SE_ERR_FNF,
        3 => SE_ERR_PNF,
        _ => ERROR_BAD_FORMAT,
    })
}

/// The program and arguments a shell link at `path` runs: its target,
/// its arguments and then `parameters`, and its working directory.
fn link_target(cpu: &mut Cpu, path: &str) -> Option<link::LinkData> {
    let (data, _, _) = crate::win32::kernel32::read_path(cpu, path).ok()?;
    link::parse(&data)
}

/// Carry out `request`: SE_ERR_ code or STARTED, and the process's
/// handle when one was started.
fn execute(cpu: &mut Cpu, mut request: Request) -> (u32, Option<u32>) {
    let mut singular = false;
    if request.file.starts_with('"') {
        let inner = &request.file[1..];
        request.file = inner.strip_suffix('"').unwrap_or(inner).to_string();
        singular = true;
    }
    if request.mask & SEE_MASK_IDLIST != 0
        && request.id_list != 0
        && let Some(path) = super::il::path_of(cpu, request.id_list)
    {
        request.file = path;
        singular = true;
    }
    if request.mask & SEE_MASK_CLASSALL == SEE_MASK_CLASSNAME
        && let Some(class) = request.class.clone()
    {
        let Some(command) = class_command(cpu, &class, request.verb.as_deref()) else {
            return (SE_ERR_NOASSOC, None);
        };
        let file = request.file.clone();
        let (mut line, found) = argify(cpu, &command, &file, request.id_list, None);
        if !found && !file.is_empty() {
            line = format!("{} \"{}\"", line, file);
        }
        return finish(cpu, &line, &request);
    }
    if request.mask & SEE_MASK_DOENVSUBST != 0 {
        request.file = files::expand(cpu, &request.file);
        request.parameters = files::expand(cpu, &request.parameters);
        request.directory = files::expand(cpu, &request.directory);
    }
    let directory = request.directory.clone();
    let mut found = search(cpu, &directory, &request.file);
    if found.is_none() && request.parameters.is_empty() && !singular {
        // A name with spaces may be a program and its arguments.
        let file = request.file.clone();
        for (at, _) in file.match_indices(' ') {
            if let Some(path) = search(cpu, &directory, &file[..at]) {
                request.parameters = file[at + 1..].trim_start().to_string();
                request.file = file[..at].to_string();
                found = Some(path);
                break;
            }
        }
    }
    let Some(path) = found else {
        if is_directory(cpu, &request.file) {
            return open_folder(cpu, &request.file.clone(), &request);
        }
        if let Some((scheme, _)) = request.file.split_once(':')
            && scheme.len() > 1
        {
            return url(cpu, scheme, &request);
        }
        if request.file.len() > 4 && request.file[..4].eq_ignore_ascii_case("www.") {
            let scheme = "http";
            request.file = format!("http://{}", request.file);
            return url(cpu, scheme, &request);
        }
        return (SE_ERR_FNF, None);
    };
    if is_directory(cpu, &path) {
        return open_folder(cpu, &path, &request);
    }
    let verb = request.verb.clone();
    let opens = verb
        .as_deref()
        .is_none_or(|v| v.eq_ignore_ascii_case("open"));
    if is_program(&path) && opens {
        let line = match request.parameters.as_str() {
            "" => format!("\"{}\"", path),
            p => format!("\"{}\" {}", path, p),
        };
        return finish(cpu, &line, &request);
    }
    if extension(&path).is_some_and(|e| e.eq_ignore_ascii_case("lnk"))
        && opens
        && let Some(data) = link_target(cpu, &path)
    {
        let parameters = match (data.arguments.as_str(), request.parameters.as_str()) {
            (a, "") => a.to_string(),
            ("", p) => p.to_string(),
            (a, p) => format!("{} {}", a, p),
        };
        let inner = Request {
            mask: request.mask & SEE_MASK_NOCLOSEPROCESS,
            verb: None,
            file: format!("\"{}\"", data.path),
            parameters,
            directory: if data.directory.is_empty() {
                request.directory.clone()
            } else {
                data.directory.clone()
            },
            show: request.show,
            ..Request::default()
        };
        return execute(cpu, inner);
    }
    let parameters = request.parameters.clone();
    let line = match find_command(
        cpu,
        &path,
        verb.as_deref(),
        request.id_list,
        Some(&parameters),
    ) {
        Ok(line) => line,
        Err(code) => return (code, None),
    };
    finish(cpu, &line, &request)
}

/// A folder opened as Explorer opens it: by the Folder type's command,
/// or else with a window of its own (as Explorer is part of this
/// Windows, nothing is started).
fn open_folder(cpu: &mut Cpu, path: &str, request: &Request) -> (u32, Option<u32>) {
    match find_command(cpu, path, request.verb.as_deref(), request.id_list, None) {
        Ok(line) => finish(cpu, &line, request),
        Err(_) => (STARTED, None),
    }
}

/// A URL opened by the command of its scheme's key.
fn url(cpu: &mut Cpu, scheme: &str, request: &Request) -> (u32, Option<u32>) {
    let Some(command) = class_command(cpu, scheme, request.verb.as_deref()) else {
        return (SE_ERR_NOASSOC, None);
    };
    let file = request.file.clone();
    let parameters = request.parameters.clone();
    let (line, _) = argify(cpu, &command, &file, 0, Some(&parameters));
    finish(cpu, &line, request)
}

fn finish(cpu: &mut Cpu, line: &str, request: &Request) -> (u32, Option<u32>) {
    match run(cpu, line, &request.directory, request.show) {
        Ok(process) if request.mask & SEE_MASK_NOCLOSEPROCESS != 0 => (STARTED, Some(process)),
        Ok(process) => {
            files::close(cpu, process);
            (STARTED, None)
        }
        Err(code) => (code, None),
    }
}

fn optional_string(cpu: &mut Cpu, at: u32) -> Option<Option<String>> {
    match at {
        0 => Some(None),
        at => Some(Some(string_at(cpu, at)?)),
    }
}

/// ShellExecuteA(window, verb, file, parameters, directory, show).
pub fn shell_execute(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [_, verb, file, parameters, directory, show] = args[..] else {
        return Some(0);
    };
    let request = Request {
        verb: optional_string(cpu, verb)?.filter(|v| !v.is_empty()),
        file: optional_string(cpu, file)?.unwrap_or_default(),
        parameters: optional_string(cpu, parameters)?.unwrap_or_default(),
        directory: optional_string(cpu, directory)?.unwrap_or_default(),
        show,
        ..Request::default()
    };
    Some(execute(cpu, request).0)
}

/// SHELLEXECUTEINFOA's size and the fields read and written.
const INFO_SIZE: u32 = 60;

/// ShellExecuteExA(info): TRUE once started, with hInstApp 33 and, for
/// SEE_MASK_NOCLOSEPROCESS, hProcess the process's handle; else FALSE
/// with hInstApp the SE_ERR_ code.
pub fn shell_execute_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[0];
    if at == 0 || !cpu.bus.guest_probe(at, INFO_SIZE as usize, true) {
        return Some(0);
    }
    let field = |cpu: &mut Cpu, offset: u32| cpu.bus.guest_read_32(at + offset);
    if field(cpu, 0) != INFO_SIZE {
        crate::win32::user32::set_error(cpu, 87);
        return Some(0);
    }
    let mask = field(cpu, 4);
    let (verb, file, parameters, directory) = (
        field(cpu, 12),
        field(cpu, 16),
        field(cpu, 20),
        field(cpu, 24),
    );
    let (show, id_list, class) = (field(cpu, 28), field(cpu, 36), field(cpu, 40));
    let request = Request {
        mask,
        verb: optional_string(cpu, verb)?.filter(|v| !v.is_empty()),
        file: optional_string(cpu, file)?.unwrap_or_default(),
        parameters: optional_string(cpu, parameters)?.unwrap_or_default(),
        directory: optional_string(cpu, directory)?.unwrap_or_default(),
        show,
        class: if mask & SEE_MASK_CLASSALL == SEE_MASK_CLASSNAME {
            optional_string(cpu, class)?
        } else {
            None
        },
        id_list,
    };
    cpu.bus.guest_write_32(at + 56, 0);
    let (code, process) = execute(cpu, request);
    cpu.bus.guest_write_32(at + 32, code);
    if let Some(process) = process {
        cpu.bus.guest_write_32(at + 56, process);
    }
    if code <= 32 {
        crate::win32::user32::set_error(cpu, code);
    }
    Some((code > 32) as u32)
}

/// FindExecutableA(file, directory, result): the program that opens the
/// file into `result` (MAX_PATH bytes), and 33; or an SE_ERR_ code.
pub fn find_executable(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [file, directory, result] = args[..] else {
        return Some(0);
    };
    if result != 0 {
        if !cpu.bus.guest_probe(result, 1, true) {
            return None;
        }
        cpu.bus.guest_write_8(result, 0);
    }
    let Some(file) = optional_string(cpu, file)? else {
        return Some(SE_ERR_FNF);
    };
    let directory = optional_string(cpu, directory)?.unwrap_or_default();
    let Some(path) = search(cpu, &directory, &file) else {
        return Some(SE_ERR_FNF);
    };
    let line = match find_command(cpu, &path, None, 0, None) {
        Ok(line) => line,
        Err(code) => return Some(code),
    };
    // The program alone: what is in the quotes, or before the first
    // space.
    let program = match line.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or(rest).to_string(),
        None => line.split(' ').next().unwrap_or(&line).to_string(),
    };
    if result != 0 {
        let mut bytes = crate::dosstr::to_bytes(&program);
        bytes.truncate(259);
        bytes.push(0);
        if !cpu.bus.guest_probe(result, bytes.len(), true) {
            return None;
        }
        cpu.bus.guest_write_bytes(result, &bytes);
    }
    Some(STARTED)
}

#[cfg(test)]
mod tests {
    use super::extension;

    #[test]
    fn extensions_come_from_the_file_name() {
        assert_eq!(extension("C:\\A.B\\file"), None);
        assert_eq!(extension("C:\\A\\file.txt"), Some("txt"));
        assert_eq!(extension("C:\\A\\file."), None);
    }
}
