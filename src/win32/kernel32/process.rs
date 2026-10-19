//! Processes: starting a program as a process of its own, finding one by
//! its id, and what a process learns of how it was started.

use super::{ERROR_INVALID_PARAMETER, FALSE, TRUE, set_error, string_at};
use crate::cpu::Cpu;
use crate::win32::object;
use crate::win32::{create_process, process};

const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_BAD_EXE_FORMAT: u32 = 193;

const CREATE_SUSPENDED: u32 = 4;
const STARTF_USESTDHANDLES: u32 = 0x100;

/// STARTUPINFOA's size and fields.
const STARTUPINFO_SIZE: u32 = 68;
const SI_RESERVED: u32 = 4;
const SI_DESKTOP: u32 = 8;
const SI_TITLE: u32 = 12;
const SI_FLAGS: u32 = 44;
const SI_RESERVED2: u32 = 52;
const SI_STD_INPUT: u32 = 56;

/// The first word of a command line: quoted, or up to a space.
fn first_word(line: &str) -> &str {
    let line = line.trim_start();
    match line.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or(rest),
        None => line.split([' ', '\t']).next().unwrap_or(line),
    }
}

/// The DOS path of the program `name`: as named, with ".EXE" added if it
/// has no extension, looked for where it says, in the directory of the
/// running program, then in the directories of PATH.
fn find_program(cpu: &mut Cpu, name: &str) -> Option<String> {
    let file = name.rsplit(['\\', '/']).next().unwrap_or(name);
    let name = if file.contains('.') {
        name.to_string()
    } else {
        format!("{}.EXE", name)
    };
    let mut candidates = vec![name.clone()];
    if !name.contains(['\\', '/', ':']) {
        let own = process(cpu).path.clone();
        if let Some(i) = own.rfind('\\') {
            candidates.push(format!("{}\\{}", &own[..i], name));
        }
        let path = process(cpu)
            .environment
            .iter()
            .find_map(|v| {
                let (k, value) = v.split_once('=')?;
                k.eq_ignore_ascii_case("PATH").then(|| value.to_string())
            })
            .unwrap_or_default();
        for dir in path.split(';').filter(|d| !d.is_empty()) {
            candidates.push(format!("{}\\{}", dir.trim_end_matches('\\'), name));
        }
    }
    candidates.into_iter().find_map(|c| {
        let path = cpu.program_path(&c);
        cpu.bus.disk.exists(&path).then_some(path)
    })
}

/// The environment block at `at`: "NAME=value" strings, each ended by a
/// zero, the last by two.
fn environment_at(cpu: &mut Cpu, mut at: u32) -> Option<Vec<String>> {
    let mut strings = Vec::new();
    loop {
        let s = string_at(cpu, at)?;
        if s.is_empty() {
            return Some(strings);
        }
        at += crate::dosstr::to_bytes(&s).len() as u32 + 1;
        strings.push(s);
    }
}

/// CreateProcessA(application, command line, process attributes, thread
/// attributes, inherit, flags, environment, directory, startup info,
/// process information): the program as a process of its own, which runs
/// beside its parent. The process information gets the handles of the
/// process and its main thread, and their ids.
pub(super) fn create_process_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[app_at, line_at, _, _, inherit, flags, env_at, _, startup_at, info_at] = args else {
        unreachable!()
    };
    let app = match app_at {
        0 => None,
        at => Some(string_at(cpu, at)?),
    };
    let line = match line_at {
        0 => None,
        at => Some(string_at(cpu, at)?),
    };
    let mut startup = vec![0u8; STARTUPINFO_SIZE as usize];
    if startup_at != 0 {
        cpu.bus.guest_read_bytes(startup_at, &mut startup);
    }
    let environment = match env_at {
        0 => None,
        at => Some(environment_at(cpu, at)?),
    };
    if !cpu.bus.guest_probe(info_at, 16, true) {
        return None;
    }
    let name = match (&app, &line) {
        (Some(app), _) => app.clone(),
        (None, Some(line)) => first_word(line).to_string(),
        (None, None) => {
            set_error(cpu, ERROR_INVALID_PARAMETER);
            return Some(FALSE);
        }
    };
    let Some(path) = find_program(cpu, &name) else {
        set_error(cpu, ERROR_FILE_NOT_FOUND);
        return Some(FALSE);
    };
    let line = line.unwrap_or_else(|| path.clone());
    let environment = environment.unwrap_or_else(|| process(cpu).environment.clone());
    let fields = startup.clone();
    let field = |i: u32| u32::from_le_bytes(fields[i as usize..][..4].try_into().unwrap());
    // The child's own copy: no strings or data of the parent's.
    for i in [SI_RESERVED, SI_DESKTOP, SI_TITLE, SI_RESERVED2] {
        startup[i as usize..i as usize + 4].fill(0);
    }
    startup[..4].copy_from_slice(&STARTUPINFO_SIZE.to_le_bytes());
    let inherited: Vec<(u32, u32)> = if inherit != 0 {
        process(cpu).handles.iter().collect()
    } else {
        Vec::new()
    };
    let std = if field(SI_FLAGS) & STARTF_USESTDHANDLES != 0 {
        let mut ids = [0; 3];
        for (i, id) in ids.iter_mut().enumerate() {
            let handle = field(SI_STD_INPUT + 4 * i as u32);
            *id = object::resolve(cpu, handle).unwrap_or(0);
        }
        // A handle that isn't one is DOS's.
        let own = process(cpu).std;
        for (i, id) in ids.iter_mut().enumerate() {
            if *id == 0 {
                *id = object::resolve(cpu, own[i]).unwrap_or(0);
            }
        }
        Some(ids)
    } else {
        let own = process(cpu).std;
        let ids = own.map(|h| object::resolve(cpu, h).unwrap_or(0));
        Some(ids)
    };
    let std = std.filter(|ids| ids.iter().all(|&id| object::objects(cpu).get(id).is_some()));
    let suspended = flags & CREATE_SUSPENDED != 0;
    let created = match create_process(cpu, &path, &line, environment, inherited, std, suspended)
    {
        Ok(created) => created,
        Err(message) => {
            cpu.bus.log_string(&format!("[W32] {}", message));
            set_error(cpu, ERROR_BAD_EXE_FORMAT);
            return Some(FALSE);
        }
    };
    if let Some(child) = cpu.bus.win32.processes.last_mut() {
        child.startup = startup;
    }
    let (process_object, pid) = created.process;
    let (thread_object, tid) = created.thread;
    let objects = object::objects(cpu);
    objects.hold(process_object);
    objects.hold(thread_object);
    let process_handle = object::new_handle(cpu, process_object);
    let thread_handle = object::new_handle(cpu, thread_object);
    for (i, value) in [process_handle, thread_handle, pid, tid].into_iter().enumerate() {
        cpu.bus.guest_write_32(info_at + 4 * i as u32, value);
    }
    set_error(cpu, 0);
    Some(TRUE)
}

/// OpenProcess(access, inherit, id): a handle of the process with the id
/// `id`, while it runs.
pub(super) fn open_process(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = args[2];
    let found = cpu
        .bus
        .win32
        .processes
        .iter()
        .find(|p| p.id == id)
        .map(|p| p.object);
    let Some(object) = found else {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    };
    object::objects(cpu).hold(object);
    Some(object::new_handle(cpu, object))
}

/// GetStartupInfoA(info): what the parent gave CreateProcessA, or for the
/// process DOS started, nothing but the standard handles.
pub(super) fn get_startup_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[0];
    let process = process(cpu);
    let mut info = process.startup.clone();
    if info.is_empty() {
        info = vec![0; STARTUPINFO_SIZE as usize];
        info[..4].copy_from_slice(&STARTUPINFO_SIZE.to_le_bytes());
    }
    if u32::from_le_bytes(info[SI_FLAGS as usize..][..4].try_into().unwrap()) & STARTF_USESTDHANDLES
        == 0
    {
        for (i, handle) in process.std.into_iter().enumerate() {
            let at = SI_STD_INPUT as usize + 4 * i;
            info[at..at + 4].copy_from_slice(&handle.to_le_bytes());
        }
    }
    cpu.bus.guest_write_bytes(at, &info);
    if cpu.bus.guest_faulted() {
        return None;
    }
    Some(0)
}
