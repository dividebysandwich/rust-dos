//! Making a process of a program: its memory and tables, the program's
//! image at its image base (or relocated), the DLLs it imports, its
//! imports bound, its static TLS, its stack and its thread's TIB.
//!
//! DLLs are loaded here too, for the program and for LoadLibrary: the
//! built-in ones mapped where Windows 95 has them, and DLL files found on
//! the search path, mapped, relocated when their base is taken, and their
//! own imports loaded and bound in turn. Calling their entry points is
//! kernel32's (module.rs), once the loader is done.

use super::heap::Heap;
use super::memory::{
    GRANULARITY, Memory, PAGE, PAGE_READWRITE, PRIVATE, SHARED, SYSTEM, USER, WRITABLE,
};
use super::stubs::{self, STUB};
use super::system::{self, SYSTEM_BLOCK_PAGES};
use super::{FIRST_MISSING, MAX_MISSING, Module, Process};
use crate::bus::Bus;
use crate::cpu::Cpu;
use rust_dos_pe as pe;

/// A process needs at least this much extended memory to start.
pub const MIN_MEMORY: u32 = 0x40_0000;
/// The largest stack committed when the process starts; a larger reserve
/// stays reserved, with no pages behind it.
const MAX_STACK_COMMIT: u32 = 0x10_0000;
/// Where the TLS slot array is in the TIB's page (`fs:[2Ch]` points at
/// it), and how many slots it has, as on Windows 95.
pub(crate) const TLS_ARRAY: u32 = 0x100;
pub(super) const TLS_SLOTS: u32 = 80;
/// The address space the process heap can grow to.
const HEAP_RESERVE: u32 = 0x100_0000;

/// The TIB's fields.
const TIB_EXCEPTION_LIST: u32 = 0x00;
const TIB_STACK_TOP: u32 = 0x04;
const TIB_STACK_LIMIT: u32 = 0x08;
const TIB_SELF: u32 = 0x18;
const TIB_TLS_ARRAY: u32 = 0x2C;
/// The process's database, as Windows 95 has it (`process_block`).
pub(crate) const TIB_PROCESS: u32 = 0x30;

/// Where the process's database is: in the block of the command line,
/// after the ANSI one. Of its fields only the process heap at 18h is
/// filled, which Windows 95 has there as Windows NT has it in the PEB
/// that fs:[30h] points to there: Wine's DLLs read it so for
/// GetProcessHeap.
pub(crate) fn process_block(command_line: u32) -> u32 {
    command_line + PAGE / 2
}
const PROCESS_HEAP: u32 = 0x18;

/// Windows 95's directories, where the built-in DLLs are said to be and
/// which the search for a DLL goes through.
pub(crate) const WINDOWS_DIR: &str = "C:\\WINDOWS";
pub(crate) const SYSTEM_DIR: &str = "C:\\WINDOWS\\SYSTEM";

/// What a module is (`Module::flags`): a built-in DLL, a DLL that came
/// with the program (neither is ever unloaded), one mapped as data, one
/// whose imports weren't loaded, one whose entry point returned TRUE to
/// DLL_PROCESS_ATTACH, and one that DisableThreadLibraryCalls was called
/// for.
pub(crate) const BUILTIN: u32 = 0x01;
pub(super) const STATIC: u32 = 0x02;
pub(crate) const DATAFILE: u32 = 0x04;
pub(super) const UNRESOLVED: u32 = 0x08;
pub(super) const ATTACHED: u32 = 0x10;
pub(super) const NO_THREAD_CALLS: u32 = 0x20;
/// A DLL of the folder the `win32_dlls` setting names, guest code
/// (`guest_file`): the functions it imports that the built-in DLLs lack
/// fail where it calls them, rather than ending the program.
pub(super) const GUEST: u32 = 0x40;

/// LoadLibraryExA's flags.
pub(super) const DONT_RESOLVE_DLL_REFERENCES: u32 = 0x01;
pub(super) const LOAD_LIBRARY_AS_DATAFILE: u32 = 0x02;
pub(super) const LOAD_WITH_ALTERED_SEARCH_PATH: u32 = 0x08;

/// The errors a load fails with (GetLastError).
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
const ERROR_BAD_FORMAT: u32 = 11;
const ERROR_GEN_FAILURE: u32 = 31;
const ERROR_PROC_NOT_FOUND: u32 = 127;
pub(super) const ERROR_DLL_INIT_FAILED: u32 = 1114;
const ERROR_DLL_NOT_FOUND: u32 = 1157;

/// What the loader made: the process, and where the program starts.
pub struct Loaded {
    pub process: Process,
    pub entry: u32,
    pub stack_top: u32,
    /// The main thread's stack: the lowest address of its reserve, and
    /// its top.
    pub stack: (u32, u32),
}

/// Make the process of the program `path`, whose file is `file`, with
/// `environment`, whose PATH the search for its DLLs goes through. Fails
/// with a message for the user; then nothing is left taken.
pub fn load(
    cpu: &mut Cpu,
    path: &str,
    line: &str,
    file: &[u8],
    environment: Vec<String>,
) -> Result<Loaded, String> {
    let name = path
        .rsplit(['\\', '/', ':'])
        .next()
        .unwrap_or(path)
        .to_string();
    let mut image =
        pe::Image::load(file).map_err(|e| format!("{} can't be loaded: {}.", name, e))?;
    if image.headers.is_dll() {
        return Err(format!("{} is a DLL, not a program.", name));
    }
    let imports = image
        .imports()
        .map_err(|e| format!("{}'s imports can't be read: {}.", name, e))?;
    let mut memory = Memory::new(&mut cpu.bus).ok_or_else(|| {
        format!(
            "There isn't enough extended memory to run {} (it needs 4 MB).",
            name
        )
    })?;
    let mut process = Process {
        name: name.clone(),
        path: path.to_string(),
        environment,
        ..Process::default()
    };
    let loaded = build(cpu, &mut memory, &mut process, line, &mut image, &imports);
    match loaded {
        Some(Ok(mut loaded)) => {
            process.memory = memory;
            loaded.process = process;
            Ok(loaded)
        }
        result => {
            memory.release(&mut cpu.bus);
            Err(match result {
                Some(Err(message)) => message,
                _ => format!("There isn't enough extended memory to run {}.", name),
            })
        }
    }
}

/// `load`'s work once the memory is taken. None when it runs out. The
/// `Loaded` it returns has an empty process: the process is `process`.
fn build(
    cpu: &mut Cpu,
    memory: &mut Memory,
    process: &mut Process,
    line: &str,
    image: &mut pe::Image,
    imports: &[pe::ImportedDll],
) -> Option<Result<Loaded, String>> {
    let name = process.name.clone();
    let bus = &mut cpu.bus;

    // The thread's TIB, and the system's tables.
    let tib = memory.reserve(None, GRANULARITY, SHARED, SYSTEM)?;
    memory.commit(bus, tib, PAGE, USER | WRITABLE)?;
    process.tib = tib;
    process.system = memory.pages(bus, SYSTEM_BLOCK_PAGES)?;
    system::build_tables(cpu, memory, process.system, tib)?;
    let bus = &mut cpu.bus;

    // KERNEL32, which has the system's own stubs, whatever the program
    // imports.
    map_builtin(bus, memory, process, 0)?;

    // The program, where it wants to be, or where there is room when its
    // relocations let it move.
    let (preferred, size) = (image.headers.image_base, image.headers.size_of_image);
    let base = match memory.reserve_image(Some(preferred), size, PRIVATE, SHARED) {
        Some(base) => base,
        None if image.headers.relocatable() => {
            let base = memory.reserve_image(None, size, PRIVATE, SHARED)?;
            if let Err(e) = image.relocate(base) {
                return Some(Err(format!("{} can't be relocated: {}.", name, e)));
            }
            base
        }
        None => {
            return Some(Err(format!(
                "{} must be loaded at {:08X}h, where Windows 95 has no room for it.",
                name, preferred
            )));
        }
    };
    process.image_base = base;
    process.modules.insert(
        0,
        Module {
            name: name.to_string(),
            path: process.path.clone(),
            base,
            size,
            entry: 0,
            refs: 0,
            flags: STATIC,
            deps: Vec::new(),
        },
    );

    // Static TLS: the template's copy for the thread, in slot 0, which the
    // program finds in its index variable.
    let mut tls_block = None;
    match image.tls() {
        Ok(Some(tls)) => {
            let template = match image.tls_template(&tls) {
                Ok(template) => template,
                Err(e) => return Some(Err(format!("{}'s TLS directory is damaged: {}.", name, e))),
            };
            let len = (template.len() as u32).max(1).next_multiple_of(PAGE);
            let block = memory.reserve(None, len, PRIVATE, SHARED)?;
            memory.commit(bus, block, len, USER | WRITABLE)?;
            memory.write(bus, block, &template)?;
            // (Relocated with the image, as the directory's addresses are.)
            let index = tls.index_address.wrapping_sub(base);
            if image.set_u32(index, 0).is_err() {
                return Some(Err(format!("{}'s TLS index is outside it.", name)));
            }
            if !tls.callbacks.is_empty() {
                bus.log_string("[W32] The program's TLS callbacks aren't called yet");
            }
            process.tls_used = 1;
            process.tls_template = template;
            tls_block = Some(block);
        }
        Ok(None) => {}
        Err(e) => return Some(Err(format!("{}'s TLS directory is damaged: {}.", name, e))),
    }

    // The image's pages, writable where a section is.
    map_image(bus, memory, image, base, false)?;

    // The DLLs it imports, and its imports bound: the built-in functions'
    // stubs, and for the functions the built-in DLLs don't have yet, stubs
    // that say so.
    let mut loader = Loader {
        bus,
        memory,
        process,
        static_load: true,
        first_dir: String::new(),
        mapped: Vec::new(),
        raised: Vec::new(),
    };
    loader.first_dir = directory_of(&loader.process.path);
    match bind_imports(&mut loader, base, imports, &name) {
        Ok(deps) => loader.process.modules[0].deps = deps,
        Err(e) if e.code == ERROR_NOT_ENOUGH_MEMORY => return None,
        Err(e) => return Some(Err(e.message)),
    }
    let bus = &mut cpu.bus;

    // The stack: the reserve, its top committed.
    let reserve = image.headers.stack_reserve.clamp(GRANULARITY, 0x100_0000);
    process.stack_reserve = reserve;
    let headers = &image.headers;
    process.subsystem_version =
        (headers.major_subsystem as u32) << 16 | headers.minor_subsystem as u32;
    let stack = memory.reserve(None, reserve, PRIVATE, SHARED)?;
    let reserve = reserve.next_multiple_of(GRANULARITY);
    let commit = reserve.min(MAX_STACK_COMMIT);
    let stack_top = stack + reserve;
    memory.commit(bus, stack_top - commit, commit, USER | WRITABLE)?;

    // The idle loop's stack, which only interrupts use.
    let idle = memory.reserve(None, GRANULARITY, SHARED, SYSTEM)?;
    memory.commit(bus, idle, PAGE, USER | WRITABLE)?;
    process.idle_stack = idle + PAGE;

    // The process heap.
    let heap = memory.reserve(None, HEAP_RESERVE, PRIVATE, SHARED)?;
    process.heap = Heap::new(heap, HEAP_RESERVE, true);

    // The command line, in a block of the process's own: in ANSI, and
    // in the page after it in Unicode (GetCommandLineW).
    let data = memory.reserve(None, GRANULARITY, PRIVATE, SHARED)?;
    memory.commit(bus, data, 2 * PAGE, USER | WRITABLE)?;
    let mut line = crate::dosstr::to_bytes(line);
    let bus = &mut cpu.bus;
    line.truncate(PAGE as usize / 2 - 1);
    line.push(0);
    memory.write(bus, data, &line)?;
    let wide = super::kernel32::ansi_to_wide(&line);
    let wide: Vec<u8> = wide.iter().flat_map(|c| c.to_le_bytes()).collect();
    memory.write(bus, data + PAGE, &wide)?;
    process.command_line = data;
    memory.write_32(bus, process_block(data) + PROCESS_HEAP, process.heap.base)?;

    // The TIB.
    for (field, value) in [
        (TIB_EXCEPTION_LIST, 0xFFFF_FFFF),
        (TIB_STACK_TOP, stack_top),
        (TIB_STACK_LIMIT, stack_top - commit),
        (TIB_SELF, tib),
        (TIB_TLS_ARRAY, tib + TLS_ARRAY),
        (TIB_PROCESS, process_block(data)),
    ] {
        memory.write_32(bus, tib + field, value)?;
    }
    if let Some(block) = tls_block {
        memory.write_32(bus, tib + TLS_ARRAY, block)?;
    }

    if image.headers.entry == 0 {
        return Some(Err(format!("{} has no entry point.", name)));
    }
    Some(Ok(Loaded {
        process: Process::default(),
        entry: base + image.headers.entry,
        stack_top,
        stack: (stack, stack_top),
    }))
}

/// Map the pages of `image` at `base`, which is reserved for it, and
/// write the image there: writable where a section is, or with
/// `read_only` none of them.
fn map_image(
    bus: &mut Bus,
    memory: &mut Memory,
    image: &pe::Image,
    base: u32,
    read_only: bool,
) -> Option<()> {
    let size = image.headers.size_of_image;
    for page in (0..size).step_by(PAGE as usize) {
        let writable = !read_only
            && image
                .headers
                .sections
                .iter()
                .any(|s| s.writable() && page < s.rva + s.mapped_size() && s.rva < page + PAGE);
        let phys = memory.page(bus)?;
        memory.map(
            bus,
            base + page,
            phys,
            USER | if writable { WRITABLE } else { 0 },
        )?;
    }
    memory.write(bus, base, &image.data)
}

/// The command line of a program DOS started: its path and the PSP's
/// command tail.
pub fn dos_command_line(cpu: &mut Cpu, path: &str) -> String {
    let psp = cpu.current_psp as usize * 16;
    let len = (cpu.bus.read_8(psp + 0x80) as usize).min(0x7E);
    let tail: Vec<u8> = (0..len).map(|i| cpu.bus.read_8(psp + 0x81 + i)).collect();
    format!("{}{}", path, crate::dosstr::from_bytes(&tail))
}

/// A stub for `function` of the built-in DLL `dll`, which it doesn't have
/// yet: calling it ends the program, saying what it needed, or fails the
/// call when guest code calls it (`super::missing`), the stub popping the
/// `arg_bytes` of a stdcall function. Returns the stub's address.
fn missing_stub(
    bus: &mut Bus,
    memory: &mut Memory,
    process: &mut Process,
    dll: &str,
    function: &str,
    arg_bytes: u16,
) -> Option<u32> {
    let index = process.missing.len() as u32;
    if index >= MAX_MISSING {
        return None;
    }
    let at = index * STUB;
    if at.is_multiple_of(PAGE) {
        if index == 0 {
            process.missing_base = memory.reserve(None, MAX_MISSING * STUB, SHARED, SYSTEM)?;
        }
        memory.commit(bus, process.missing_base + at, PAGE, USER)?;
        let page = memory.translate(bus, process.missing_base + at)?;
        process.stub_pages.push(page >> 12);
    }
    let address = process.missing_base + at;
    let id = FIRST_MISSING + index as u16;
    let stub = match u8::try_from(arg_bytes / 4) {
        Ok(args) if args > 0 => stubs::stub(id, args, stubs::Conv::Stdcall),
        _ => stubs::stub(id, 0, stubs::Conv::Cdecl),
    };
    memory.write(bus, address, &stub)?;
    process
        .missing
        .push((dll.to_string(), function.to_string()));
    Some(address)
}

/// Why a load failed: the error LoadLibrary gives, and the message the
/// program that can't start ends with.
pub(super) struct LoadError {
    pub code: u32,
    pub message: String,
}

fn out_of_memory(name: &str) -> LoadError {
    LoadError {
        code: ERROR_NOT_ENOUGH_MEMORY,
        message: format!("There isn't enough extended memory to run {}.", name),
    }
}

/// A load of DLLs into a process: what it works on, and what it did.
pub(super) struct Loader<'a> {
    pub bus: &'a mut Bus,
    pub memory: &'a mut Memory,
    pub process: &'a mut Process,
    /// Whether the DLLs come with the program, which keeps them while it
    /// runs, rather than with LoadLibrary.
    pub static_load: bool,
    /// The directory searched first: the program's, or with
    /// LOAD_WITH_ALTERED_SEARCH_PATH the DLL's.
    pub first_dir: String,
    /// The DLLs it mapped, each after the DLLs it imports from: the order
    /// their entry points learn they are loaded in.
    pub mapped: Vec<u32>,
    /// The modules loaded before whose count it raised.
    pub raised: Vec<u32>,
}

/// A function a module exports, as it is asked for.
#[derive(Clone, Debug)]
pub(super) enum Wanted {
    Name(String),
    Ordinal(u16),
}

impl std::fmt::Display for Wanted {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Wanted::Name(name) => write!(f, "{}", name),
            Wanted::Ordinal(ordinal) => write!(f, "#{}", ordinal),
        }
    }
}

/// Where an export leads: an address, or another DLL's export, as
/// "DLL.Function" or "DLL.#ordinal".
enum Target {
    Address(u32),
    Forward(String),
}

/// The file name a module is known by: `name` without its directory,
/// with ".DLL" added when it has no extension (a final dot says it has
/// none), in capitals.
pub(super) fn module_file_name(name: &str) -> String {
    let file = name.rsplit(['\\', '/', ':']).next().unwrap_or(name);
    let file = file.trim_end_matches(' ');
    let file = match file.strip_suffix('.') {
        Some(bare) => bare.to_string(),
        None if !file.contains('.') && !file.is_empty() => format!("{}.DLL", file),
        None => file.to_string(),
    };
    file.to_ascii_uppercase()
}

/// The directory of the DOS path `path`, without a final backslash.
pub(crate) fn directory_of(path: &str) -> String {
    match path.rfind('\\') {
        Some(i) => path[..i].to_string(),
        None => String::new(),
    }
}

/// `dir` and `file` joined with a backslash.
fn join(dir: &str, file: &str) -> String {
    if dir.ends_with('\\') {
        format!("{}{}", dir, file)
    } else {
        format!("{}\\{}", dir, file)
    }
}

/// The DLL file `name` (`file` without its directory) on the disks: the
/// path it names, or without one the first found in the program's
/// directory, the current directory, the system and Windows directories
/// and the directories in PATH, as Windows 95 searches. Its full DOS path.
fn find_file(l: &Loader, name: &str, file: &str) -> Option<String> {
    let disk = &l.bus.disk;
    if name.contains(['\\', '/', ':']) {
        let name = name.trim_end_matches(' ');
        let path = disk.qualify_path(name)?;
        let path = join(&directory_of(&path), file);
        return disk.is_file(&path).then_some(path);
    }
    let mut dirs = vec![l.first_dir.clone()];
    dirs.extend(disk.qualify_directory("X"));
    dirs.push(SYSTEM_DIR.to_string());
    dirs.push(WINDOWS_DIR.to_string());
    let path = l.process.environment.iter().find_map(|s| {
        s.split_once('=')
            .filter(|(n, _)| n.eq_ignore_ascii_case("PATH"))
            .map(|(_, v)| v.to_string())
    });
    if let Some(path) = path {
        dirs.extend(
            path.split(';')
                .map(str::trim)
                .filter(|d| !d.is_empty())
                .map(str::to_string),
        );
    }
    dirs.into_iter()
        .filter(|d| !d.is_empty())
        .map(|d| join(&d, file))
        .find(|p| disk.is_file(p))
        .and_then(|p| disk.qualify_path(&p))
}

/// The contents of the file at the DOS path `path`.
fn read_file(bus: &mut Bus, path: &str) -> Option<crate::memfs::Bytes> {
    let bytes = bus.disk.file_data(path)?.read().ok()?;
    if let Some(drive) = bus.disk.drive_of(path) {
        let key = bus.disk.file_key(path);
        let access = crate::disknoise::Access::File { write: false, key };
        bus.drive_activity(
            drive,
            crate::diskio::OPEN_BYTES + bytes.len() as u32,
            access,
        );
    }
    Some(bytes)
}

/// The error a file that isn't an image Windows 95 can load gives: one
/// with no PE header is in a bad format, and Windows 95 fails one whose
/// PE header it can't take with ERROR_GEN_FAILURE, as Wine's tests found.
fn bad_image_error(file: &[u8]) -> u32 {
    let at = |i: usize| {
        file.get(i..i + 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    };
    let pe = file
        .starts_with(b"MZ")
        .then(|| at(0x3C))
        .flatten()
        .and_then(|offset| file.get(offset as usize..offset as usize + 4));
    if pe == Some(b"PE\0\0") {
        ERROR_GEN_FAILURE
    } else {
        ERROR_BAD_FORMAT
    }
}

/// Map the built-in DLL `index` where Windows 95 has it, read only.
/// Its base.
pub(crate) fn map_builtin(
    bus: &mut Bus,
    memory: &mut Memory,
    process: &mut Process,
    index: usize,
) -> Option<u32> {
    let dll = stubs::DLLS[index];
    if let Some(m) = process.modules.iter().find(|m| m.base == dll.base) {
        return Some(m.base);
    }
    let built = stubs::image(index, bus.win32.os);
    let size = built.headers.size_of_image;
    memory.reserve_image(Some(dll.base), size, SHARED, SYSTEM)?;
    memory.commit(bus, dll.base, size.next_multiple_of(PAGE), USER)?;
    memory.write(bus, dll.base, &built.data)?;
    // Read only but for its variables.
    for section in built.headers.sections.iter().filter(|s| s.writable()) {
        let end = section.rva + section.mapped_size();
        for page in (section.rva..end).step_by(PAGE as usize) {
            if !memory.set_protection(bus, dll.base + page, PAGE_READWRITE) {
                return None;
            }
        }
    }
    for page in (dll.base..dll.base + size).step_by(PAGE as usize) {
        process.stub_pages.push(memory.translate(bus, page)? >> 12);
    }
    process.modules.push(Module {
        name: dll.name.to_string(),
        path: join(SYSTEM_DIR, dll.name),
        base: dll.base,
        size,
        entry: 0,
        refs: 0,
        flags: BUILTIN,
        deps: Vec::new(),
    });
    // Its variables: for a process that runs already (LoadLibrary);
    // `create_process` fills those of the DLLs a program starts with.
    if !process.threads.is_empty()
        && let Some(init) = dll.init
    {
        init(bus, process)?;
    }
    Some(dll.base)
}

/// Load the module `name` for `importer` (the program, or a DLL that
/// imports from it), as LoadLibraryExA with `flags` does: a built-in DLL,
/// one loaded already (its count raised), or the DLL file found on the
/// search path, mapped, and unless the flags say otherwise its imports
/// loaded and bound. Its base.
pub(super) fn load_module(
    l: &mut Loader,
    name: &str,
    flags: u32,
    importer: &str,
) -> Result<u32, LoadError> {
    let file = module_file_name(name);
    let datafile = flags & LOAD_LIBRARY_AS_DATAFILE != 0;
    let not_found = || LoadError {
        code: if datafile {
            ERROR_FILE_NOT_FOUND
        } else {
            ERROR_DLL_NOT_FOUND
        },
        message: format!(
            "{} needs {}, which the built-in Windows doesn't have yet.",
            importer, name
        ),
    };
    if file.is_empty() {
        return Err(not_found());
    }

    // A built-in DLL, by its name alone or in the system directory.
    let has_path = name.contains(['\\', '/', ':']);
    let in_system = || {
        l.bus
            .disk
            .qualify_path(name.trim_end_matches(' '))
            .is_some_and(|p| directory_of(&p).eq_ignore_ascii_case(SYSTEM_DIR))
    };
    if let Some(index) = builtin_for(l.bus, &file)
        && (!has_path || in_system())
    {
        return map_builtin(l.bus, l.memory, l.process, index)
            .ok_or_else(|| out_of_memory(importer));
    }

    // One loaded already, known by its file name as on Windows 95. A
    // path elsewhere doesn't name a built-in DLL, and one to map as data
    // names that file only.
    let qualified = if has_path {
        l.bus.disk.qualify_path(name.trim_end_matches(' '))
    } else {
        None
    };
    let loaded = l.process.modules.iter_mut().find(|m| {
        let same_file = qualified
            .as_deref()
            .is_some_and(|p| p.eq_ignore_ascii_case(&m.path));
        let by_name = m.name.eq_ignore_ascii_case(&file)
            && !(has_path && (datafile || m.flags & BUILTIN != 0));
        (same_file || by_name) && (datafile || m.flags & DATAFILE == 0)
    });
    if let Some(m) = loaded {
        if m.flags & (BUILTIN | STATIC) == 0 {
            m.refs += 1;
            if !l.mapped.contains(&m.base) {
                l.raised.push(m.base);
            }
        }
        return Ok(m.base);
    }

    // On the disks, or else in the `win32_dlls` folder, as if it were
    // in the system directory.
    let (path, bytes, guest) = match find_file(l, name, &file) {
        Some(path) => {
            let bytes = read_file(l.bus, &path).ok_or_else(not_found)?;
            (path, bytes, false)
        }
        None if !has_path || in_system() => {
            let host = guest_file(l.bus, &file).ok_or_else(not_found)?;
            let bytes = std::fs::read(&host).map_err(|_| not_found())?;
            l.bus.log_string(&format!(
                "[W32] {} loads {} from {}",
                importer,
                file,
                host.display()
            ));
            (join(SYSTEM_DIR, &file), bytes.into(), true)
        }
        None => return Err(not_found()),
    };
    let mut image = pe::Image::load(&bytes).map_err(|e| LoadError {
        code: bad_image_error(&bytes),
        message: format!("{} can't be loaded: {}.", file, e),
    })?;
    let (preferred, size) = (image.headers.image_base, image.headers.size_of_image);
    if size == 0 {
        return Err(LoadError {
            code: bad_image_error(&bytes),
            message: format!("{} can't be loaded: it is empty.", file),
        });
    }

    // As data: the image as it is, where there is room.
    if datafile {
        let base = l
            .memory
            .reserve_image(Some(preferred), size, PRIVATE, SHARED)
            .or_else(|| l.memory.reserve_image(None, size, PRIVATE, SHARED))
            .ok_or_else(|| out_of_memory(importer))?;
        map_image(l.bus, l.memory, &image, base, true).ok_or_else(|| out_of_memory(importer))?;
        l.process.modules.push(Module {
            name: file,
            path,
            base,
            size,
            entry: 0,
            refs: 1,
            flags: DATAFILE,
            deps: Vec::new(),
        });
        return Ok(base);
    }

    // Where it wants to be, in the private or the shared arena, or
    // relocated where there is room.
    let base = match l.memory.reserve_image(Some(preferred), size, PRIVATE, SYSTEM) {
        Some(base) => base,
        None if image.headers.relocatable() => {
            let base = l
                .memory
                .reserve_image(None, size, PRIVATE, SHARED)
                .ok_or_else(|| out_of_memory(importer))?;
            if let Err(e) = image.relocate(base) {
                l.memory.unreserve(base);
                return Err(LoadError {
                    code: ERROR_BAD_FORMAT,
                    message: format!("{} can't be relocated: {}.", file, e),
                });
            }
            base
        }
        None => {
            return Err(LoadError {
                code: ERROR_NOT_ENOUGH_MEMORY,
                message: format!(
                    "{} must be loaded at {:08X}h, where Windows 95 has no room for it.",
                    file, preferred
                ),
            });
        }
    };
    let resolve = flags & DONT_RESOLVE_DLL_REFERENCES == 0;
    let entry = if image.headers.is_dll() && image.headers.entry != 0 && resolve {
        base + image.headers.entry
    } else {
        0
    };
    let mut module_flags = if l.static_load { STATIC } else { 0 };
    if !resolve {
        module_flags |= UNRESOLVED;
    }
    if guest {
        module_flags |= GUEST;
    }
    l.process.modules.push(Module {
        name: file.clone(),
        path,
        base,
        size,
        entry,
        refs: 1,
        flags: module_flags,
        deps: Vec::new(),
    });
    map_image(l.bus, l.memory, &image, base, false).ok_or_else(|| out_of_memory(importer))?;

    if resolve {
        let imports = image.imports().map_err(|e| LoadError {
            code: ERROR_BAD_FORMAT,
            message: format!("{}'s imports can't be read: {}.", file, e),
        })?;
        let deps = bind_imports(l, base, &imports, &file)?;
        if let Some(m) = l.process.modules.iter_mut().find(|m| m.base == base) {
            m.deps = deps;
        }
        dll_tls(l, &image, &file)?;
    }

    // After the DLLs it imports from, which it needs first.
    if let Some(i) = l.process.modules.iter().position(|m| m.base == base) {
        let module = l.process.modules.remove(i);
        l.process.modules.push(module);
    }
    l.mapped.push(base);
    Ok(base)
}

/// The static TLS of the DLL `image`, which `load_module` mapped: with the
/// program, a slot of the process's, whose number goes to its index
/// variable, and the main thread's copy of its template there; threads
/// that start later get theirs (`tls_blocks`). Windows 95 can't give one
/// to a DLL LoadLibrary loads, and fails the load.
fn dll_tls(l: &mut Loader, image: &pe::Image, file: &str) -> Result<(), LoadError> {
    let damaged = |e: pe::Error| LoadError {
        code: ERROR_BAD_FORMAT,
        message: format!("{}'s TLS directory is damaged: {}.", file, e),
    };
    let Some(tls) = image.tls().map_err(damaged)? else {
        return Ok(());
    };
    if !l.static_load {
        return Err(LoadError {
            code: ERROR_DLL_INIT_FAILED,
            message: format!("{} has static TLS, which LoadLibrary can't give it.", file),
        });
    }
    let template = image.tls_template(&tls).map_err(damaged)?;
    let used = l.process.tls_used;
    let Some(slot) = (0..TLS_SLOTS).find(|&i| used & 1 << i == 0) else {
        return Err(LoadError {
            code: ERROR_DLL_INIT_FAILED,
            message: format!("{} needs a TLS slot, and there are none left.", file),
        });
    };
    let (bus, memory) = (&mut *l.bus, &mut *l.memory);
    memory
        .write_32(bus, tls.index_address, slot)
        .ok_or_else(|| LoadError {
            code: ERROR_BAD_FORMAT,
            message: format!("{}'s TLS index is outside it.", file),
        })?;
    let tib = l.process.tib;
    tls_block(bus, memory, tib, slot, &template).ok_or_else(|| out_of_memory(file))?;
    l.process.tls_used |= 1 << slot;
    l.process.dll_tls.push((slot, template));
    Ok(())
}

/// A copy of the TLS template `template` for the thread whose TIB is at
/// `tib`, in its slot `slot`.
fn tls_block(
    bus: &mut Bus,
    memory: &mut Memory,
    tib: u32,
    slot: u32,
    template: &[u8],
) -> Option<()> {
    let len = (template.len() as u32).max(1).next_multiple_of(PAGE);
    let block = memory.reserve(None, len, PRIVATE, SHARED)?;
    memory.commit(bus, block, len, USER | WRITABLE)?;
    memory.write(bus, block, template)?;
    memory.write_32(bus, tib + TLS_ARRAY + 4 * slot, block)
}

/// The copies of the static TLS of the process's DLLs for a new thread,
/// whose TIB is at `tib`.
pub(super) fn tls_blocks(bus: &mut Bus, process: &mut Process, tib: u32) -> Option<()> {
    for (slot, template) in process.dll_tls.clone() {
        tls_block(bus, &mut process.memory, tib, slot, &template)?;
    }
    Some(())
}

/// Load the DLLs `imports` names for the module at `base` (`importer`),
/// and write the addresses of the functions it imports to its import
/// address table. The functions of the built-in DLLs that they don't have
/// yet get stubs that say so; a DLL file without a function fails the
/// load, as does a built-in DLL's function of Windows 98 under 95. The
/// modules it took a reference on.
fn bind_imports(
    l: &mut Loader,
    base: u32,
    imports: &[pe::ImportedDll],
    importer: &str,
) -> Result<Vec<u32>, LoadError> {
    let mut deps = Vec::new();
    let guest = l
        .process
        .modules
        .iter()
        .any(|m| m.base == base && m.flags & GUEST != 0);
    for dll in imports {
        let dep = load_module(l, &dll.name, 0, importer)?;
        deps.push(dep);
        let builtin = l
            .process
            .modules
            .iter()
            .find(|m| m.base == dep && m.flags & BUILTIN != 0)
            .map(|m| m.name.clone());
        // The import library that says how many bytes of arguments the
        // stdcall functions guest code imports take.
        let mut library: Option<Vec<u8>> = None;
        for (slot, import) in &dll.functions {
            let wanted = match import {
                pe::Import::Name { name, .. } => Wanted::Name(name.clone()),
                pe::Import::Ordinal(ordinal) => Wanted::Ordinal(*ordinal),
            };
            let found = resolve_export(l, dep, &wanted, importer, &mut deps, 0)?;
            // A function the built-in DLL has only under Windows 98 is
            // missing under 95 as on Windows 95, and the program doesn't
            // load.
            let withheld = match (&wanted, &builtin) {
                (Wanted::Name(name), Some(dll)) => {
                    !super::version::exports(l.bus.win32.os, dll, name)
                }
                _ => false,
            };
            let address = match (found, &builtin) {
                (Some(address), _) => address,
                (None, Some(builtin)) if !withheld => {
                    let what = wanted.to_string();
                    let arg_bytes = match &wanted {
                        Wanted::Name(name) if guest => {
                            let library = library
                                .get_or_insert_with(|| import_library(l.bus, &dll.name));
                            stdcall_arg_bytes(library, name).unwrap_or(0)
                        }
                        _ => 0,
                    };
                    missing_stub(l.bus, l.memory, l.process, builtin, &what, arg_bytes)
                        .ok_or_else(|| out_of_memory(importer))?
                }
                (None, _) => {
                    // Windows 95 says no more, but the setting that has
                    // the function is worth naming.
                    let hint = if withheld {
                        " It is a Windows 98 function: win32_version=98 has it."
                    } else {
                        ""
                    };
                    return Err(LoadError {
                        code: ERROR_PROC_NOT_FOUND,
                        message: format!(
                            "{} is linked to missing export {}:{}.{}",
                            importer,
                            module_file_name(&dll.name),
                            wanted,
                            hint
                        ),
                    });
                }
            };
            l.memory
                .write_32(l.bus, base.wrapping_add(*slot), address)
                .ok_or_else(|| LoadError {
                    code: ERROR_BAD_FORMAT,
                    message: format!("{}'s import table is damaged.", importer),
                })?;
        }
    }
    Ok(deps)
}

/// The built-in DLL the module `file` (as `module_file_name` gives it)
/// names. With the `win32_dlls` setting, NTDLL.DLL is one, UCRT's
/// ucrtbase.dll and api-ms-win-crt-*.dll are MSVCRT.DLL and
/// KERNELBASE.DLL is KERNEL32.DLL, which Wine's DLLs import; a DLL the
/// setting replaces, which the folder has, isn't.
pub(super) fn builtin_for(bus: &Bus, file: &str) -> Option<usize> {
    let index = stubs::find_dll(file);
    let win32 = &bus.win32;
    if win32.guest_dlls.is_none() {
        return index.filter(|&i| stubs::DLLS[i].name != super::ntdll::DLL.name);
    }
    let stem = file.strip_suffix(".DLL").unwrap_or(file).to_ascii_lowercase();
    if win32.guest_replace.contains(&stem) && guest_file(bus, file).is_some() {
        return None;
    }
    match stem.as_str() {
        "ucrtbase" => stubs::find_dll("MSVCRT.DLL"),
        _ if stem.starts_with("api-ms-win-crt-") => stubs::find_dll("MSVCRT.DLL"),
        "kernelbase" => stubs::find_dll("KERNEL32.DLL"),
        _ => index,
    }
}

/// The DLL `file` in the host folder the `win32_dlls` setting names,
/// whatever the case of its name there.
fn guest_file(bus: &Bus, file: &str) -> Option<std::path::PathBuf> {
    let folder = bus.win32.guest_dlls.as_ref()?;
    std::fs::read_dir(folder)
        .ok()?
        .flatten()
        .find(|entry| entry.file_name().to_string_lossy().eq_ignore_ascii_case(file))
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
}

/// The import library of the DLL `dll` in the `win32_dlls` folder
/// ("lib<name>.a", as Wine's i386-windows folder has one for each of its
/// DLLs), or nothing.
fn import_library(bus: &Bus, dll: &str) -> Vec<u8> {
    let file = module_file_name(dll);
    let stem = file.strip_suffix(".DLL").unwrap_or(&file);
    guest_file(bus, &format!("LIB{}.A", stem))
        .and_then(|path| std::fs::read(path).ok())
        .unwrap_or_default()
}

/// The bytes of arguments the stdcall function `name` takes, by its
/// decorated name "_name@bytes" in the import library `library`. None
/// for a cdecl function, whose name has no decoration.
fn stdcall_arg_bytes(library: &[u8], name: &str) -> Option<u16> {
    let wanted = format!("_{}@", name);
    let wanted = wanted.as_bytes();
    let mut from = 0;
    while let Some(at) = library[from..]
        .windows(wanted.len())
        .position(|w| w == wanted)
        .map(|i| from + i)
    {
        from = at + 1;
        // The whole name, alone or after "__imp_".
        let alone = at == 0 || library[at - 1] == 0 || library[..at].ends_with(b"__imp_");
        let digits: Vec<u8> = library[at + wanted.len()..]
            .iter()
            .copied()
            .take_while(u8::is_ascii_digit)
            .collect();
        let end = at + wanted.len() + digits.len();
        if alone && !digits.is_empty() && library.get(end) == Some(&0) {
            return std::str::from_utf8(&digits).ok()?.parse().ok();
        }
    }
    None
}

/// The address of the export `wanted` of the module at `base`, following
/// forwarders to other DLLs, which are loaded for it (and kept in
/// `deps`). None if there is no such export.
fn resolve_export(
    l: &mut Loader,
    base: u32,
    wanted: &Wanted,
    importer: &str,
    deps: &mut Vec<u32>,
    depth: u32,
) -> Result<Option<u32>, LoadError> {
    match export_target(l.bus, l.memory, base, wanted) {
        None => Ok(None),
        Some(Target::Address(address)) => Ok(Some(address)),
        Some(Target::Forward(forward)) => {
            let Some((dll, function)) = forward.rsplit_once('.').filter(|_| depth < 8) else {
                return Ok(None);
            };
            let dep = load_module(l, dll, 0, importer)?;
            deps.push(dep);
            resolve_export(l, dep, &forwarded(function), importer, deps, depth + 1)
        }
    }
}

/// The function a forwarder's "Function" or "#ordinal" names.
fn forwarded(function: &str) -> Wanted {
    match function.strip_prefix('#').and_then(|n| n.parse().ok()) {
        Some(ordinal) => Wanted::Ordinal(ordinal),
        None => Wanted::Name(function.to_string()),
    }
}

/// The address of the export `wanted` of the module at `base`, for
/// GetProcAddress: forwarders are followed to DLLs the process has
/// already, and to built-in ones, which are mapped for it.
pub(super) fn find_export(
    bus: &mut Bus,
    process: &mut Process,
    base: u32,
    wanted: &Wanted,
) -> Option<u32> {
    let (mut base, mut wanted) = (base, wanted.clone());
    for _ in 0..8 {
        match export_target(bus, &process.memory, base, &wanted)? {
            Target::Address(address) => return Some(address),
            Target::Forward(forward) => {
                let (dll, function) = forward.rsplit_once('.')?;
                base = match process.module(dll) {
                    Some(base) => base,
                    None => {
                        let index = stubs::find_dll(&module_file_name(dll))?;
                        let mut memory = std::mem::take(&mut process.memory);
                        let mapped = map_builtin(bus, &mut memory, process, index);
                        process.memory = memory;
                        mapped?
                    }
                };
                wanted = forwarded(function);
            }
        }
    }
    None
}

/// Map the built-in DLL named `name` in the running process if it isn't
/// there: COM makes objects of DLLs that programs don't import, and the
/// objects' methods are in the DLL's image. Its base, or None.
pub(super) fn map_builtin_named(cpu: &mut crate::cpu::Cpu, name: &str) -> Option<u32> {
    let index = stubs::find_dll(name)?;
    super::with_process(cpu, |process, bus| {
        let mut memory = std::mem::take(&mut process.memory);
        let mapped = map_builtin(bus, &mut memory, process, index);
        process.memory = memory;
        mapped
    })
}

/// `len` bytes at `lin` in `memory`, read through its page tables
/// without a page fault: None where a page isn't there.
pub(crate) fn peek(bus: &Bus, memory: &Memory, lin: u32, len: usize) -> Option<Vec<u8>> {
    (0..len as u32)
        .map(|i| {
            let phys = memory.translate(bus, lin.wrapping_add(i))?;
            Some(bus.read_8(phys as usize))
        })
        .collect()
}

pub(super) fn peek_32(bus: &Bus, memory: &Memory, lin: u32) -> Option<u32> {
    let bytes = peek(bus, memory, lin, 4)?;
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

/// The NUL-terminated string at `lin`, at most `max` bytes of it.
pub(crate) fn peek_string(bus: &Bus, memory: &Memory, lin: u32, max: usize) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    while bytes.len() < max {
        let phys = memory.translate(bus, lin.wrapping_add(bytes.len() as u32))?;
        match bus.read_8(phys as usize) {
            0 => break,
            b => bytes.push(b),
        }
    }
    Some(bytes)
}

/// The address a data directory of the image at `base` gives, and its
/// size: 0 for one it doesn't have.
pub(super) fn directory(bus: &Bus, memory: &Memory, base: u32, index: u32) -> Option<(u32, u32)> {
    let pe = base.wrapping_add(peek_32(bus, memory, base + 0x3C)?);
    let count = peek_32(bus, memory, pe + 0x74)?;
    if index >= count {
        return Some((0, 0));
    }
    let rva = peek_32(bus, memory, pe + 0x78 + 8 * index)?;
    let size = peek_32(bus, memory, pe + 0x7C + 8 * index)?;
    Some((rva, size))
}

/// Where the export `wanted` of the module at `base` leads, read from its
/// export directory in memory as Windows reads it.
fn export_target(bus: &Bus, memory: &Memory, base: u32, wanted: &Wanted) -> Option<Target> {
    let (dir, dir_size) = directory(bus, memory, base, 0)?;
    if dir == 0 {
        return None;
    }
    let field = |offset: u32| peek_32(bus, memory, base.wrapping_add(dir).wrapping_add(offset));
    let ordinal_base = field(16)?;
    let functions = field(20)?;
    let names = field(24)?;
    let function_table = base.wrapping_add(field(28)?);
    let name_table = base.wrapping_add(field(32)?);
    let ordinal_table = base.wrapping_add(field(36)?);
    let index = match wanted {
        Wanted::Ordinal(ordinal) => (*ordinal as u32).checked_sub(ordinal_base)?,
        Wanted::Name(name) => {
            let name = crate::dosstr::to_bytes(name);
            (0..names).find_map(|i| {
                let at = base.wrapping_add(peek_32(bus, memory, name_table + 4 * i)?);
                if peek_string(bus, memory, at, 256)? != name {
                    return None;
                }
                let index = peek(bus, memory, ordinal_table + 2 * i, 2)?;
                Some(u16::from_le_bytes([index[0], index[1]]) as u32)
            })?
        }
    };
    if index >= functions {
        return None;
    }
    let rva = peek_32(bus, memory, function_table + 4 * index)?;
    if rva == 0 {
        return None;
    }
    if (dir..dir.wrapping_add(dir_size)).contains(&rva) {
        let text = peek_string(bus, memory, base.wrapping_add(rva), 256)?;
        return Some(Target::Forward(crate::dosstr::from_bytes(&text)));
    }
    Some(Target::Address(base.wrapping_add(rva)))
}

/// Take a reference on the module at `base` away (FreeLibrary): the
/// modules whose last reference went, it first and then those it imported
/// from, are added to `freed`. The built-in DLLs and those that came with
/// the program stay.
pub(super) fn release(process: &mut Process, base: u32, freed: &mut Vec<u32>) {
    let Some(module) = process.modules.iter_mut().find(|m| m.base == base) else {
        return;
    };
    if module.flags & (BUILTIN | STATIC) != 0 || module.refs == 0 {
        return;
    }
    module.refs -= 1;
    if module.refs > 0 {
        return;
    }
    let deps = module.deps.clone();
    freed.push(base);
    for dep in deps {
        release(process, dep, freed);
    }
}

/// Take the module at `base` out of the process: its pages and address
/// space are given back.
pub(super) fn unmap(bus: &mut Bus, memory: &mut Memory, process: &mut Process, base: u32) {
    let Some(i) = process.modules.iter().position(|m| m.base == base) else {
        return;
    };
    let module = process.modules.remove(i);
    for page in (base..base + module.size.next_multiple_of(PAGE)).step_by(PAGE as usize) {
        memory.unmap(bus, page);
    }
    memory.unreserve(base);
}

/// Undo a LoadLibrary that failed: the modules it mapped (those not in
/// `before`) go, and the counts it raised go down again.
pub(super) fn roll_back(
    bus: &mut Bus,
    memory: &mut Memory,
    process: &mut Process,
    before: &[u32],
    raised: &[u32],
) {
    let new: Vec<u32> = process
        .modules
        .iter()
        .map(|m| m.base)
        .filter(|b| !before.contains(b))
        .collect();
    for base in new {
        unmap(bus, memory, process, base);
    }
    for &base in raised {
        if let Some(m) = process.modules.iter_mut().find(|m| m.base == base) {
            m.refs = m.refs.saturating_sub(1);
        }
    }
}
