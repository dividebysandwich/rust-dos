//! Modules: loading and freeing DLLs, their entry points, the functions
//! they export, their resources, and the system's version.
//!
//! A DLL's entry point (DllMain) learns that the process or a thread
//! starts or ends. Those calls into the program go one after the other
//! through `call::call`, a chain whose state is the call's data: what
//! happens when it is done (`then`), its argument, the reason and
//! reserved argument DllMain gets, the next module, and the modules'
//! bases. `run_chain` makes the next call, or when none is left, does
//! what the chain was for (`done`).

use super::{
    ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER, ERROR_PROC_NOT_FOUND, FALSE, TRUE, set_error,
    string_at,
};
use crate::cpu::Cpu;
use crate::win32::call::{self, Call, DLL_NOTIFIED, RESOURCE_ENUMERATED};
use crate::win32::loader::{
    self, ATTACHED, ERROR_DLL_INIT_FAILED, LOAD_WITH_ALTERED_SEARCH_PATH, Loader, NO_THREAD_CALLS,
    Wanted,
};
use crate::win32::{process, running, thread, with_process};

/// DllMain's reasons.
const DLL_PROCESS_DETACH: u32 = 0;
const DLL_PROCESS_ATTACH: u32 = 1;
const DLL_THREAD_DETACH: u32 = 3;

/// What a chain of DllMain calls does when it is done, with its
/// argument: go to the program's entry point (its address); return from
/// LoadLibrary (the module); unmap the modules freed and return (TRUE, or
/// 0 for a load that failed); end the thread or the process (the exit
/// code); unmap the modules and end the thread (the exit code).
const THEN_START: u32 = 0;
const THEN_LOAD: u32 = 1;
const THEN_FREE: u32 = 2;
const THEN_EXIT_THREAD: u32 = 3;
const THEN_EXIT_PROCESS: u32 = 4;
const THEN_FREE_EXIT: u32 = 5;

/// The places in a chain's data.
const THEN: usize = 0;
const ARG: usize = 1;
const REASON: usize = 2;
const RESERVED: usize = 3;
const NEXT: usize = 4;
const LIST: usize = 5;

/// A chain's data: `then` with `arg`, DllMain's `reason` and `reserved`
/// argument, for the modules at `bases`.
fn chain(then: u32, arg: u32, reason: u32, reserved: u32, bases: &[u32]) -> Vec<u32> {
    let mut data = vec![then, arg, reason, reserved, 0];
    data.extend_from_slice(bases);
    data
}

/// Call the next module's entry point the chain `data` has left, or when
/// none is left, do what it was for. `resume` is where the function that
/// started it returns to.
fn run_chain(cpu: &mut Cpu, resume: (u32, u32), mut data: Vec<u32>) {
    let reason = data[REASON];
    while data[NEXT] as usize + LIST < data.len() {
        let base = data[LIST + data[NEXT] as usize];
        data[NEXT] += 1;
        let Some(module) = process(cpu).modules.iter().find(|m| m.base == base) else {
            continue;
        };
        let attached = module.flags & ATTACHED != 0;
        let wanted = match reason {
            DLL_PROCESS_ATTACH => !attached,
            DLL_PROCESS_DETACH => attached,
            _ => attached && module.flags & NO_THREAD_CALLS == 0,
        };
        if module.entry == 0 || !wanted {
            continue;
        }
        let (entry, reserved) = (module.entry, data[RESERVED]);
        if !call::call(
            cpu,
            entry,
            &[base, reason, reserved],
            DLL_NOTIFIED,
            resume,
            data,
        ) {
            crate::win32::fatal(cpu, "a thread's stack isn't there", None);
        }
        return;
    }
    done(cpu, resume, data);
}

/// A DLL's entry point returned `value`: the chain goes on. A DLL that
/// fails DLL_PROCESS_ATTACH fails LoadLibrary, or the process if it came
/// with the program.
pub(in crate::win32) fn notified(cpu: &mut Cpu, value: u32, call: Call) {
    let data = call.data;
    let base = data[LIST + data[NEXT] as usize - 1];
    let reason = data[REASON];
    if reason == DLL_PROCESS_ATTACH && value == 0 {
        attach_failed(cpu, call.resume, &data, base);
        return;
    }
    if let Some(m) = process(cpu).modules.iter_mut().find(|m| m.base == base) {
        match reason {
            DLL_PROCESS_ATTACH => m.flags |= ATTACHED,
            DLL_PROCESS_DETACH => m.flags &= !ATTACHED,
            _ => {}
        }
    }
    run_chain(cpu, call.resume, data);
}

/// The DLL at `base` returned FALSE to DLL_PROCESS_ATTACH. One loaded
/// with the program ends it, as on Windows 95; for LoadLibrary, the
/// modules it loaded are freed again (those whose entry points ran learn
/// that they are detached), and it fails.
fn attach_failed(cpu: &mut Cpu, resume: (u32, u32), data: &[u32], base: u32) {
    if data[THEN] == THEN_START {
        let path = process(cpu)
            .modules
            .iter()
            .find(|m| m.base == base)
            .map_or(String::new(), |m| m.path.clone());
        let message = format!(
            "Initialization of the dynamic link library {} failed. The process is terminating abnormally.",
            path
        );
        cpu.bus.log_string(&format!("[W32] {}", message));
        crate::win32::say(cpu, &message);
        crate::win32::exit(cpu, crate::win32::FAILED);
        return;
    }
    let mut freed = Vec::new();
    loader::release(process(cpu), data[ARG], &mut freed);
    let data = chain(THEN_FREE, 0, DLL_PROCESS_DETACH, 0, &freed);
    run_chain(cpu, resume, data);
}

/// What a chain was for, now that the entry points have run.
fn done(cpu: &mut Cpu, resume: (u32, u32), data: Vec<u32>) {
    let arg = data[ARG];
    match data[THEN] {
        THEN_START => cpu.set_eip(arg),
        THEN_LOAD => call::finish(cpu, resume, arg),
        THEN_FREE | THEN_FREE_EXIT => {
            let bases = data[LIST..].to_vec();
            with_process(cpu, |p, bus| {
                let mut memory = std::mem::take(&mut p.memory);
                for base in bases {
                    loader::unmap(bus, &mut memory, p, base);
                }
                p.memory = memory;
            });
            cpu.tlb.flush();
            if data[THEN] == THEN_FREE_EXIT {
                exit_thread(cpu, arg);
                return;
            }
            if arg == 0 {
                set_error(cpu, ERROR_DLL_INIT_FAILED);
            }
            call::finish(cpu, resume, arg);
        }
        THEN_EXIT_THREAD => thread::exit_current(cpu, arg),
        _ => crate::win32::exit(cpu, arg),
    }
}

/// The main thread's start when the DLLs that came with the program have
/// entry points: each learns that the process starts, those a DLL
/// imports from before it, then the program's entry point (in EAX) runs.
pub(super) fn process_start(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let entry = cpu.eax();
    let bases: Vec<u32> = process(cpu)
        .modules
        .iter()
        .filter(|m| m.entry != 0 && m.flags & ATTACHED == 0)
        .map(|m| m.base)
        .collect();
    // DllMain's reserved argument is not NULL for a DLL that came with
    // the program.
    let data = chain(THEN_START, entry, DLL_PROCESS_ATTACH, 1, &bases);
    run_chain(cpu, call::here(cpu), data);
    None
}

/// The running thread ends with `code` (ExitThread, or its function
/// returned): the DLLs learn of it, last loaded first, then it ends. The
/// last thread to end ends the process, as ExitProcess.
pub(super) fn exit_thread(cpu: &mut Cpu, code: u32) {
    let process = process(cpu);
    let current = process.current;
    let others = process
        .threads
        .iter()
        .enumerate()
        .any(|(i, t)| Some(i) != current && t.exit_code.is_none());
    if !others {
        exit_process(cpu, code);
        return;
    }
    if process.exiting {
        thread::exit_current(cpu, code);
        return;
    }
    let bases: Vec<u32> = process.modules.iter().rev().map(|m| m.base).collect();
    let data = chain(THEN_EXIT_THREAD, code, DLL_THREAD_DETACH, 0, &bases);
    run_chain(cpu, call::here(cpu), data);
}

/// ExitProcess(code): the other threads end, the DLLs learn that the
/// process ends, last loaded first, then it ends.
pub(super) fn exit_process(cpu: &mut Cpu, code: u32) {
    if process(cpu).exiting {
        crate::win32::exit(cpu, code);
        return;
    }
    process(cpu).exiting = true;
    let win32 = &mut cpu.bus.win32;
    let p = win32.running;
    let current = win32.processes[p].current;
    for t in 0..win32.processes[p].threads.len() {
        if Some(t) != current {
            thread::end(win32, p, t, code);
        }
    }
    let bases: Vec<u32> = process(cpu).modules.iter().rev().map(|m| m.base).collect();
    // Not NULL: the process ends.
    let data = chain(THEN_EXIT_PROCESS, code, DLL_PROCESS_DETACH, 1, &bases);
    run_chain(cpu, call::here(cpu), data);
}

/// LoadLibraryA(name).
pub(super) fn load_library(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    load(cpu, args[0], 0)
}

/// LoadLibraryExA(name, file, flags): Windows 95 ignores the file
/// handle, which must be NULL.
pub(super) fn load_library_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    load(cpu, args[0], args[2])
}

/// Load the module whose name is at `name` (loader.rs) with `flags`: the
/// DLLs it mapped learn that they are loaded, those it imports from
/// first, and their entry points may fail it. Its base, or 0 with the
/// error set.
fn load(cpu: &mut Cpu, name: u32, flags: u32) -> Option<u32> {
    if name == 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    let name = string_at(cpu, name)?;
    let importer = process(cpu).name.clone();
    let altered = flags & LOAD_WITH_ALTERED_SEARCH_PATH != 0 && name.contains(['\\', '/', ':']);
    let first_dir = match cpu.bus.disk.qualify_path(&name) {
        Some(path) if altered => loader::directory_of(&path),
        _ => loader::directory_of(&process(cpu).path),
    };
    let result = with_process(cpu, |p, bus| {
        let mut memory = std::mem::take(&mut p.memory);
        let before: Vec<u32> = p.modules.iter().map(|m| m.base).collect();
        let mut l = Loader {
            bus: &mut *bus,
            memory: &mut memory,
            process: &mut *p,
            static_load: false,
            first_dir,
            mapped: Vec::new(),
            raised: Vec::new(),
        };
        let result = loader::load_module(&mut l, &name, flags, &importer);
        let (mapped, raised) = (l.mapped, l.raised);
        if result.is_err() {
            loader::roll_back(bus, &mut memory, p, &before, &raised);
        }
        p.memory = memory;
        result.map(|base| (base, mapped))
    });
    cpu.tlb.flush();
    let (base, mapped) = match result {
        Ok(loaded) => loaded,
        Err(e) => {
            cpu.bus.log_string(&format!(
                "[W32] LoadLibrary({}) failed: {}",
                name, e.message
            ));
            set_error(cpu, e.code);
            return Some(0);
        }
    };
    let attach = process(cpu)
        .modules
        .iter()
        .any(|m| mapped.contains(&m.base) && m.entry != 0);
    if !attach {
        return Some(base);
    }
    let data = chain(THEN_LOAD, base, DLL_PROCESS_ATTACH, 0, &mapped);
    run_chain(cpu, call::here(cpu), data);
    None
}

/// Whether `handle` is one of the process's modules.
fn is_module(cpu: &mut Cpu, handle: u32) -> bool {
    process(cpu).modules.iter().any(|m| m.base == handle)
}

/// FreeLibrary(module): a reference less; when the last goes, the DLL
/// learns that it is detached, and it goes, with the DLLs it imported
/// from that nothing else holds.
pub(super) fn free_library(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    free(cpu, args[0], THEN_FREE, TRUE)
}

/// FreeLibraryAndExitThread(module, code).
pub(super) fn free_library_and_exit_thread(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    free(cpu, args[0], THEN_FREE_EXIT, args[1])
}

fn free(cpu: &mut Cpu, module: u32, then: u32, arg: u32) -> Option<u32> {
    if !is_module(cpu, module) {
        set_error(cpu, ERROR_INVALID_HANDLE);
        if then == THEN_FREE_EXIT {
            exit_thread(cpu, arg);
            return None;
        }
        return Some(FALSE);
    }
    let mut freed = Vec::new();
    loader::release(process(cpu), module, &mut freed);
    let data = chain(then, arg, DLL_PROCESS_DETACH, 0, &freed);
    run_chain(cpu, call::here(cpu), data);
    None
}

/// GetModuleFileNameA(module, buffer, size): the module's DOS path, or
/// with NULL the program's, cut to fit with its NUL. Its length. As on
/// Windows 95, a module that isn't there gives 0 and an empty string.
pub(super) fn get_module_file_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[module, buffer, size] = args else {
        unreachable!()
    };
    let process = process(cpu);
    let base = if module == 0 {
        process.image_base
    } else {
        module
    };
    let path = process
        .modules
        .iter()
        .find(|m| m.base == base)
        .map(|m| m.path.clone());
    let Some(path) = path else {
        if size > 0 {
            cpu.bus.guest_write_8(buffer, 0);
            if cpu.bus.guest_faulted() {
                return None;
            }
        }
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0);
    };
    if size == 0 {
        return Some(0);
    }
    let mut bytes = crate::dosstr::to_bytes(&path);
    bytes.truncate(size as usize - 1);
    let len = bytes.len() as u32;
    bytes.push(0);
    cpu.bus.guest_write_bytes(buffer, &bytes);
    Some(len)
}

/// DisableThreadLibraryCalls(module): its entry point no longer learns
/// of threads that start and end.
pub(super) fn disable_thread_library_calls(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    match process(cpu).modules.iter_mut().find(|m| m.base == args[0]) {
        Some(m) => {
            m.flags |= NO_THREAD_CALLS;
            Some(TRUE)
        }
        None => {
            set_error(cpu, ERROR_INVALID_HANDLE);
            Some(FALSE)
        }
    }
}

/// The address of the function `args[1]` (a name, or an ordinal below
/// 10000h) that the module at `args[0]` exports, read from its export
/// directory in memory as Windows reads it. A forwarded export leads to
/// the other DLL's, if the process has that DLL.
pub(super) fn get_proc_address(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[module, name] = args else {
        unreachable!()
    };
    if !is_module(cpu, module) {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0);
    }
    let wanted = match name {
        0..0x1_0000 => Wanted::Ordinal(name as u16),
        _ => Wanted::Name(string_at(cpu, name)?),
    };
    let found = with_process(cpu, |p, bus| loader::find_export(bus, p, module, &wanted));
    if found.is_none() {
        set_error(cpu, ERROR_PROC_NOT_FOUND);
    }
    Some(found.unwrap_or(0))
}

/// The resource errors.
const ERROR_RESOURCE_DATA_NOT_FOUND: u32 = 1812;
const ERROR_RESOURCE_TYPE_NOT_FOUND: u32 = 1813;
const ERROR_RESOURCE_NAME_NOT_FOUND: u32 = 1814;
const ERROR_RESOURCE_LANG_NOT_FOUND: u32 = 1815;

/// The resource directory's data directory.
const DIR_RESOURCE: u32 = 2;

/// A resource's type, name or language: a number, or a name, which is
/// compared whatever its case.
#[derive(Clone, Debug, PartialEq)]
enum Res {
    Id(u16),
    Name(String),
}

impl Res {
    fn matches(&self, other: &Res) -> bool {
        match (self, other) {
            (Res::Id(a), Res::Id(b)) => a == b,
            (Res::Name(a), Res::Name(b)) => a.eq_ignore_ascii_case(b),
            _ => false,
        }
    }
}

/// The type or name a program passes: a number below 10000h, a string
/// "#number", or a name. None if a page isn't there.
fn res_arg(cpu: &mut Cpu, arg: u32) -> Option<Res> {
    if arg < 0x1_0000 {
        return Some(Res::Id(arg as u16));
    }
    let text = string_at(cpu, arg)?;
    Some(match text.strip_prefix('#').and_then(|n| n.parse().ok()) {
        Some(id) => Res::Id(id),
        None => Res::Name(text),
    })
}

/// The module a resource function names: NULL is the program. None if
/// the process has no such module.
fn resource_module(cpu: &mut Cpu, module: u32) -> Option<u32> {
    let process = process(cpu);
    let base = if module == 0 {
        process.image_base
    } else {
        module
    };
    process
        .modules
        .iter()
        .any(|m| m.base == base)
        .then_some(base)
}

/// The module's resource directory's address, if it has one.
fn resource_root(cpu: &Cpu, base: u32) -> Option<u32> {
    let process = running(cpu)?;
    let (rva, size) = loader::directory(&cpu.bus, &process.memory, base, DIR_RESOURCE)?;
    (rva != 0 && size != 0).then(|| base.wrapping_add(rva))
}

/// The entries of the resource directory at `dir` in the resource
/// section at `root`: each one's type, name or language, and what it
/// leads to (a directory with the top bit, else a data entry), as an
/// offset in the section.
fn resource_entries(cpu: &Cpu, root: u32, dir: u32) -> Option<Vec<(Res, u32)>> {
    let process = running(cpu)?;
    let (bus, memory) = (&cpu.bus, &process.memory);
    let counts = loader::peek(bus, memory, dir + 12, 4)?;
    let count = u16::from_le_bytes([counts[0], counts[1]]) as u32
        + u16::from_le_bytes([counts[2], counts[3]]) as u32;
    let mut entries = Vec::new();
    for i in 0..count {
        let name = loader::peek_32(bus, memory, dir + 16 + 8 * i)?;
        let target = loader::peek_32(bus, memory, dir + 20 + 8 * i)?;
        let id = if name & 0x8000_0000 != 0 {
            let at = root.wrapping_add(name & 0x7FFF_FFFF);
            let len = loader::peek(bus, memory, at, 2)?;
            let len = u16::from_le_bytes([len[0], len[1]]) as usize;
            let bytes = loader::peek(bus, memory, at + 2, 2 * len)?;
            let units: Vec<u16> = bytes
                .chunks(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            Res::Name(String::from_utf16_lossy(&units))
        } else {
            Res::Id(name as u16)
        };
        entries.push((id, target));
    }
    Some(entries)
}

/// The directory `want` leads to among the entries of the directory at
/// `dir`.
fn resource_subdir(cpu: &Cpu, root: u32, dir: u32, want: &Res) -> Option<u32> {
    resource_entries(cpu, root, dir)?
        .into_iter()
        .find(|(id, target)| id.matches(want) && target & 0x8000_0000 != 0)
        .map(|(_, target)| root.wrapping_add(target & 0x7FFF_FFFF))
}

/// The data entry of the resource `name` of type `kind` in the module at
/// `base`: in the language `language`, or one of the same primary
/// language, or a neutral one; with no language asked for, the neutral
/// one or else the first. Its address, the HRSRC.
fn find(cpu: &Cpu, base: u32, kind: &Res, name: &Res, language: Option<u16>) -> Result<u32, u32> {
    let root = resource_root(cpu, base).ok_or(ERROR_RESOURCE_DATA_NOT_FOUND)?;
    let names = resource_subdir(cpu, root, root, kind).ok_or(ERROR_RESOURCE_TYPE_NOT_FOUND)?;
    let languages = resource_subdir(cpu, root, names, name).ok_or(ERROR_RESOURCE_NAME_NOT_FOUND)?;
    let entries: Vec<(u16, u32)> = resource_entries(cpu, root, languages)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(id, target)| match id {
            Res::Id(id) if target & 0x8000_0000 == 0 => Some((id, target)),
            _ => None,
        })
        .collect();
    let found = match language {
        Some(language) => entries
            .iter()
            .find(|(id, _)| *id == language)
            .or_else(|| {
                entries
                    .iter()
                    .find(|(id, _)| id & 0x3FF == language & 0x3FF)
            })
            .or_else(|| entries.iter().find(|(id, _)| *id == 0)),
        None => entries
            .iter()
            .find(|(id, _)| *id == 0)
            .or_else(|| entries.first()),
    };
    found
        .map(|(_, target)| root.wrapping_add(*target))
        .ok_or(ERROR_RESOURCE_LANG_NOT_FOUND)
}

/// Find the resource for FindResourceA and FindResourceExA: its HRSRC, or
/// 0 with the error set.
fn find_for(
    cpu: &mut Cpu,
    module: u32,
    kind: u32,
    name: u32,
    language: Option<u16>,
) -> Option<u32> {
    let kind = res_arg(cpu, kind)?;
    let name = res_arg(cpu, name)?;
    let Some(base) = resource_module(cpu, module) else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0);
    };
    match find(cpu, base, &kind, &name, language) {
        Ok(found) => Some(found),
        Err(error) => {
            set_error(cpu, error);
            Some(0)
        }
    }
}

/// FindResourceA(module, name, type).
pub(super) fn find_resource(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    find_for(cpu, args[0], args[2], args[1], None)
}

/// FindResourceExA(module, type, name, language).
pub(super) fn find_resource_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    find_for(cpu, args[0], args[1], args[2], Some(args[3] as u16))
}

/// The data entry at `entry` (an HRSRC): its data's RVA and size.
fn data_entry(cpu: &Cpu, entry: u32) -> Option<(u32, u32)> {
    let process = running(cpu)?;
    let rva = loader::peek_32(&cpu.bus, &process.memory, entry)?;
    let size = loader::peek_32(&cpu.bus, &process.memory, entry + 4)?;
    Some((rva, size))
}

/// LoadResource(module, resource): the address of its data, which is in
/// the module's image.
pub(super) fn load_resource(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let base = resource_module(cpu, args[0]);
    let found = base.zip((args[1] != 0).then(|| data_entry(cpu, args[1])).flatten());
    match found {
        Some((base, (rva, _))) => Some(base.wrapping_add(rva)),
        None => {
            set_error(cpu, ERROR_RESOURCE_DATA_NOT_FOUND);
            Some(0)
        }
    }
}

/// LockResource(data): the data's address, which LoadResource gave.
pub(super) fn lock_resource(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(args[0])
}

/// SizeofResource(module, resource).
pub(super) fn sizeof_resource(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    match (args[1] != 0).then(|| data_entry(cpu, args[1])).flatten() {
        Some((_, size)) => Some(size),
        None => {
            set_error(cpu, ERROR_RESOURCE_DATA_NOT_FOUND);
            Some(0)
        }
    }
}

/// FreeResource(data): a resource stays with its module; 0 is success.
pub(super) fn free_resource(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(FALSE)
}

/// The places in a resource enumeration's data: what it lists, the
/// module, the callback and its argument, the type whose names it lists,
/// and the next entry.
const ENUM_KIND: usize = 0;
const ENUM_MODULE: usize = 1;
const ENUM_FUNCTION: usize = 2;
const ENUM_PARAM: usize = 3;
const ENUM_TYPE: usize = 4;
const ENUM_NEXT: usize = 5;
const ENUM_TYPES: u32 = 0;
const ENUM_NAMES: u32 = 1;

/// EnumResourceTypesA(module, function, param): the function is called
/// with each type the module's resources have, until it returns FALSE.
pub(super) fn enum_resource_types(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[module, function, param] = args else {
        unreachable!()
    };
    let found =
        resource_module(cpu, module).and_then(|base| Some((base, resource_root(cpu, base)?)));
    let Some((base, _)) = found else {
        set_error(cpu, ERROR_RESOURCE_DATA_NOT_FOUND);
        return Some(FALSE);
    };
    let data = vec![ENUM_TYPES, base, function, param, 0, 0];
    enum_next(cpu, call::here(cpu), data);
    None
}

/// EnumResourceNamesA(module, type, function, param): the function is
/// called with the name of each resource of the type, until it returns
/// FALSE.
pub(super) fn enum_resource_names(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[module, kind, function, param] = args else {
        unreachable!()
    };
    let want = res_arg(cpu, kind)?;
    let Some(base) = resource_module(cpu, module) else {
        set_error(cpu, ERROR_RESOURCE_DATA_NOT_FOUND);
        return Some(FALSE);
    };
    let Some(root) = resource_root(cpu, base) else {
        set_error(cpu, ERROR_RESOURCE_DATA_NOT_FOUND);
        return Some(FALSE);
    };
    if resource_subdir(cpu, root, root, &want).is_none() {
        set_error(cpu, ERROR_RESOURCE_TYPE_NOT_FOUND);
        return Some(FALSE);
    }
    let data = vec![ENUM_NAMES, base, function, param, kind, 0];
    enum_next(cpu, call::here(cpu), data);
    None
}

/// The entries an enumeration lists.
fn enum_entries(cpu: &mut Cpu, data: &[u32]) -> Option<Vec<Res>> {
    let root = resource_root(cpu, data[ENUM_MODULE])?;
    let dir = if data[ENUM_KIND] == ENUM_NAMES {
        let want = res_arg(cpu, data[ENUM_TYPE])?;
        resource_subdir(cpu, root, root, &want)?
    } else {
        root
    };
    let entries = resource_entries(cpu, root, dir)?;
    Some(entries.into_iter().map(|(id, _)| id).collect())
}

/// Call the enumeration's function with its next entry, or return TRUE
/// when none is left. A name is passed as a string on the stack below the
/// call.
fn enum_next(cpu: &mut Cpu, resume: (u32, u32), mut data: Vec<u32>) {
    let entries = enum_entries(cpu, &data).unwrap_or_default();
    let Some(entry) = entries.get(data[ENUM_NEXT] as usize).cloned() else {
        call::finish(cpu, resume, TRUE);
        return;
    };
    data[ENUM_NEXT] += 1;
    let area = resume.1.wrapping_sub(0x200) & !3;
    let item = match entry {
        Res::Id(id) => id as u32,
        Res::Name(name) => {
            let mut bytes = crate::dosstr::to_bytes(&name);
            bytes.truncate(0xFF);
            bytes.push(0);
            if !cpu.bus.guest_probe(area, bytes.len(), true) {
                crate::win32::fatal(cpu, "a thread's stack isn't there", None);
                return;
            }
            cpu.bus.guest_write_bytes(area, &bytes);
            area
        }
    };
    cpu.set_esp(area);
    let (module, param) = (data[ENUM_MODULE], data[ENUM_PARAM]);
    let args = if data[ENUM_KIND] == ENUM_NAMES {
        vec![module, data[ENUM_TYPE], item, param]
    } else {
        vec![module, item, param]
    };
    let function = data[ENUM_FUNCTION];
    if !call::call(cpu, function, &args, RESOURCE_ENUMERATED, resume, data) {
        crate::win32::fatal(cpu, "a thread's stack isn't there", None);
    }
}

/// An enumeration's function returned `value`: FALSE stops it, and the
/// enumeration returns FALSE too.
pub(in crate::win32) fn enumerated(cpu: &mut Cpu, value: u32, call: Call) {
    if value == 0 {
        call::finish(cpu, call.resume, FALSE);
        return;
    }
    enum_next(cpu, call.resume, call.data);
}

/// OSVERSIONINFOA's size, which its first field must give. Windows 95
/// doesn't take the larger OSVERSIONINFOEXA, and leaves the last error as
/// it was when the size is wrong.
const OSVERSIONINFOA: u32 = 148;

/// Windows 95: 4.00.950, a 9x platform (VER_PLATFORM_WIN32_WINDOWS), the
/// build number's high word the version, and no service pack.
pub(super) fn get_version_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let info = args[0];
    if cpu.bus.guest_read_32(info) != OSVERSIONINFOA {
        return Some(FALSE);
    }
    let mut data = [0u8; OSVERSIONINFOA as usize];
    for (i, value) in [OSVERSIONINFOA, 4, 0, 0x0400_03B6, 1]
        .into_iter()
        .enumerate()
    {
        data[i * 4..i * 4 + 4].copy_from_slice(&value.to_le_bytes());
    }
    cpu.bus.guest_write_bytes(info, &data);
    Some(TRUE)
}
