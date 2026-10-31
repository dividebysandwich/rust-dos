//! Loading NE modules into the Win16 process, as Windows 3.1's KERNEL
//! does: each segment a block of the global heap with its selector, the
//! modules it imports from loaded first, its relocations written, and the
//! prologues of its exported functions patched to load their data
//! segment. The module database (its handle's block) is a copy of the
//! file's header and resident tables.
//!
//! The relocations and the prologues are ported from Wine's krnl386
//! ne_segment.c (LGPL 2.1 or later, copyright 1993 Robert J. Amstadt,
//! 1995 Alexandre Julliard).

use super::{Module16, Segment16, builtin, global, ldt};
use crate::cpu::Cpu;
use crate::win32::memory::{PAGE, SHARED, SYSTEM, USER};
use rust_dos_ne as ne;

/// A module's segment's selector, and the entry points' addresses: a
/// constant's selector.
const CONSTANT_SEL: u16 = 0xFFFF;

/// The loader's errors LoadLibrary returns, below 32: the file (or one
/// it imports from) wasn't found, or isn't a Windows module it can load.
pub const ERROR_FILE_NOT_FOUND: u32 = 2;
pub const ERROR_BAD_FORMAT: u32 = 11;

/// Why a module didn't load: the error LoadLibrary returns for it, and
/// the message to log or show.
#[derive(Clone, Debug)]
pub struct LoadError {
    pub code: u32,
    pub message: String,
}

impl LoadError {
    fn new(code: u32, message: String) -> LoadError {
        LoadError { code, message }
    }
}

impl From<LoadError> for String {
    fn from(e: LoadError) -> String {
        e.message
    }
}

/// Load the module `name` (a module name, or a file's name or path) for
/// the module in the directory `dir`: one loaded already, its count
/// raised, a system module, or the file found where Windows 3.1 looks.
/// Its place in `State::modules`, and the libraries loaded with it whose
/// entry points are yet to run, in the order they are to run in.
pub fn load(
    cpu: &mut Cpu,
    name: &str,
    dir: &str,
    inits: &mut Vec<usize>,
) -> Result<usize, LoadError> {
    let file_name = name.rsplit(['\\', '/', ':']).next().unwrap_or(name);
    let module_name = file_name
        .split('.')
        .next()
        .unwrap_or(file_name)
        .to_ascii_uppercase();
    let has_path = name.contains(['\\', '/', ':']);
    // A system module is found by its file's name too, which may differ
    // from its module's (KRNL386.EXE is KERNEL), as Windows finds it.
    let builtin = builtin::find(file_name).filter(|_| !has_path);
    let module_name = builtin.map_or(module_name, |i| {
        builtin::MODULES[i].name.to_ascii_uppercase()
    });
    let by_file = |m: &Module16| {
        file_name.contains('.')
            && m.path
                .rsplit(['\\', '/', ':'])
                .next()
                .is_some_and(|f| f.eq_ignore_ascii_case(file_name))
    };
    if !has_path
        && let Some(i) = cpu
            .bus
            .win32
            .win16
            .modules
            .iter()
            .position(|m| m.name == module_name || by_file(m))
    {
        cpu.bus.win32.win16.modules[i].refs += 1;
        return Ok(i);
    }
    if let Some(index) = builtin {
        let image = builtin::image(cpu, index);
        let path = format!(
            "{}\\{}",
            crate::win32::loader::SYSTEM_DIR,
            builtin::MODULES[index].file
        );
        return load_file(cpu, image, &path, Some(index as u16), inits);
    }
    let Some((path, file)) = find_file(cpu, name, dir) else {
        return Err(LoadError::new(
            ERROR_FILE_NOT_FOUND,
            format!("{} can't be found.", file_name),
        ));
    };
    // A module of that name may be loaded already from another path.
    if let Ok(parsed) = ne::Module::parse(&file) {
        let loaded_name = parsed.name().to_ascii_uppercase();
        if let Some(i) = cpu
            .bus
            .win32
            .win16
            .modules
            .iter()
            .position(|m| m.name == loaded_name)
        {
            cpu.bus.win32.win16.modules[i].refs += 1;
            return Ok(i);
        }
    }
    load_file(cpu, file, &path, None, inits)
}

/// The file `name` and its path: as it is if it has a directory, else in
/// the current directory, `dir`, Windows' directories and the PATH, with
/// ".DLL" added to a name without an extension.
fn find_file(cpu: &mut Cpu, name: &str, dir: &str) -> Option<(String, Vec<u8>)> {
    let name = if name.contains('.') {
        name.to_string()
    } else {
        format!("{}.DLL", name)
    };
    let mut candidates = Vec::new();
    if name.contains(['\\', '/', ':']) {
        candidates.push(name.clone());
    } else {
        let cwd = crate::win32::kernel32::current_directory(cpu, cpu.bus.disk.get_current_drive());
        for d in [
            cwd.as_str(),
            dir,
            crate::win32::loader::WINDOWS_DIR,
            crate::win32::loader::SYSTEM_DIR,
        ] {
            if !d.is_empty() {
                candidates.push(join(d, &name));
            }
        }
        if let Some(path) = crate::win32::running(cpu).and_then(|p| {
            p.environment
                .iter()
                .find_map(|v| v.strip_prefix("PATH=").map(str::to_string))
        }) {
            for d in path.split(';').filter(|d| !d.is_empty()) {
                candidates.push(join(d, &name));
            }
        }
    }
    for path in candidates {
        if let Some(file) = cpu.read_program_file(&path)
            && ne::is_ne(&file)
        {
            return Some((path, file.to_vec()));
        }
    }
    None
}

fn join(dir: &str, file: &str) -> String {
    if dir.ends_with('\\') {
        format!("{}{}", dir, file)
    } else {
        format!("{}\\{}", dir, file)
    }
}

/// Load the NE file `file` from `path` (a system module's image, for
/// `builtin`).
pub fn load_file(
    cpu: &mut Cpu,
    file: Vec<u8>,
    path: &str,
    builtin: Option<u16>,
    inits: &mut Vec<usize>,
) -> Result<usize, LoadError> {
    let file_name = path
        .rsplit(['\\', '/', ':'])
        .next()
        .unwrap_or(path)
        .to_string();
    let bad = |message: String| LoadError::new(ERROR_BAD_FORMAT, message);
    let parsed = ne::Module::parse(&file)
        .map_err(|e| bad(format!("{} can't be loaded: {}.", file_name, e)))?;
    if parsed.flags & ne::LINKERROR != 0 {
        return Err(bad(format!("{} has errors the linker found.", file_name)));
    }
    if parsed.target_os == ne::OS_OS2 {
        return Err(bad(format!("{} is an OS/2 program.", file_name)));
    }
    if parsed.flags & ne::SELFLOAD != 0 {
        return Err(bad(format!(
            "{} loads its own segments, which the built-in Windows doesn't support yet.",
            file_name
        )));
    }
    // A segment flagged 32-bit (`ne::SEG_32BIT`) loads as a 16-bit one,
    // as Windows 3.1 and 95 load it. The programs that have one, such as
    // Metal Marines, set the D bit of its selector themselves with DPMI
    // functions 000Bh and 000Ch.
    let name = parsed.name().to_ascii_uppercase();

    // The module database.
    let db = global::alloc(
        cpu,
        global::GMEM_ZEROINIT,
        parsed.tables.len() as u32 + 0x40,
        0,
        false,
    );
    if db == 0 {
        return Err(out_of_memory(&file_name));
    }
    if let Some(a) = global::get(cpu, db) {
        super::poke(cpu, a.base, &parsed.tables);
    }
    set_owner(cpu, db, db);

    // The segments' blocks.
    let mut segments = Vec::new();
    for (i, seg) in parsed.segments.iter().enumerate() {
        let mut size = seg.size();
        if parsed.auto_data as usize == i + 1 {
            let mut extra = parsed.heap as u32;
            if parsed.ss_sp.0 == parsed.auto_data || parsed.ss_sp.0 == 0 {
                extra += parsed.stack as u32;
            }
            size = (size + extra).min(0x1_0000);
        }
        let handle = if builtin.is_some() && !seg.is_data() {
            builtin_segment(cpu, size, db)
        } else {
            let mut flags = global::GMEM_ZEROINIT;
            if seg.flags & ne::SEG_MOVEABLE != 0 {
                flags |= global::GMEM_MOVEABLE;
            }
            if seg.flags & ne::SEG_DISCARDABLE != 0 {
                flags |= global::GMEM_DISCARDABLE;
            }
            Some(global::alloc(cpu, flags, size, db, !seg.is_data())).filter(|&h| h != 0)
        };
        let Some(handle) = handle else {
            free_segments(cpu, &segments);
            global::free(cpu, db);
            return Err(out_of_memory(&file_name));
        };
        segments.push(Segment16 {
            handle,
            selector: global::handle_to_sel(handle),
            size,
            flags: seg.flags,
        });
    }
    let dgroup = match parsed.auto_data {
        0 => 0,
        n => segments.get(n as usize - 1).map_or(0, |s| s.handle),
    };
    let index = cpu.bus.win32.win16.modules.len();
    cpu.bus.win32.win16.modules.push(Module16 {
        name: name.clone(),
        path: path.to_string(),
        handle: db,
        file: if builtin.is_some() {
            Vec::new()
        } else {
            file.clone()
        },
        segments: segments.clone(),
        dgroup,
        refs: 1,
        builtin,
        resources32: 0,
        deps: Vec::new(),
        flags: parsed.flags,
        expected_version: parsed.expected_version,
        loaded: Vec::new(),
    });

    // The modules it imports from.
    let dir = crate::win32::loader::directory_of(path);
    let mut deps = Vec::new();
    for dep in &parsed.module_refs {
        match load(cpu, dep, &dir, inits) {
            Ok(i) => deps.push(cpu.bus.win32.win16.modules[i].handle),
            Err(e) => {
                for h in deps {
                    release(cpu, h);
                }
                unload(cpu, index);
                let message = format!("{} needs {}: {}", file_name, dep, e.message);
                return Err(LoadError::new(e.code, message));
            }
        }
    }
    cpu.bus.win32.win16.modules[index].deps = deps.clone();

    // The segments' contents, relocated.
    for (i, seg) in parsed.segments.iter().enumerate() {
        let Ok(data) = seg.data(&file) else {
            continue;
        };
        let mut bytes = data.to_vec();
        bytes.resize((segments[i].size as usize).max(bytes.len()), 0);
        let relocations = seg.relocations(&file).unwrap_or_default();
        relocate(
            cpu,
            &parsed,
            &segments,
            &deps,
            &mut bytes,
            &relocations,
            index,
        );
        if !seg.is_data() {
            patch_prologues(&parsed, i as u8 + 1, &mut bytes, &segments, dgroup);
        }
        if let Some(a) = global::get(cpu, segments[i].handle) {
            bytes.truncate(a.size as usize);
            super::poke(cpu, a.base, &bytes);
        }
    }
    if parsed.is_dll() && builtin.is_none() {
        inits.push(index);
    }
    cpu.bus.log_string(&format!(
        "[W16] Loaded {} ({}), {} segments, module {:04X}",
        name,
        path,
        segments.len(),
        db
    ));
    Ok(index)
}

fn out_of_memory(name: &str) -> LoadError {
    let message = format!("There isn't enough memory to load {}.", name);
    LoadError::new(ERROR_BAD_FORMAT, message)
}

/// The handle 16-bit code gets for the module at `index`, which
/// LoadLibrary returns: a library's instance, or the module's handle.
pub fn instance_of(cpu: &Cpu, index: usize) -> u16 {
    let m = &cpu.bus.win32.win16.modules[index];
    if m.dgroup != 0 { m.dgroup } else { m.handle }
}

/// The block of a system module's code segment: whole pages of their
/// own, whose physical pages are the process's stubs'.
fn builtin_segment(cpu: &mut Cpu, size: u32, owner: u16) -> Option<u16> {
    let len = size.max(1).next_multiple_of(PAGE);
    let pid = cpu.bus.win32.win16.pid;
    let base = super::with_win16(cpu, pid, |p, bus| {
        let base = p.memory.reserve(None, len, SHARED, SYSTEM)?;
        p.memory.commit(bus, base, len, USER)?;
        for page in (base..base + len).step_by(PAGE as usize) {
            let phys = p.memory.translate(bus, page)?;
            p.stub_pages.push(phys >> 12);
        }
        Some(base)
    })??;
    global::create_block(cpu, global::GMEM_FIXED, base, size, owner, true, true)
}

fn free_segments(cpu: &mut Cpu, segments: &[Segment16]) {
    for s in segments {
        global::free(cpu, s.handle);
    }
}

/// Set the owner of the block `handle`.
fn set_owner(cpu: &mut Cpu, handle: u16, owner: u16) {
    let sel = global::handle_to_sel(handle);
    if let Some(a) = cpu.bus.win32.win16.arena.get_mut(&ldt::index(sel)) {
        a.owner = owner;
    }
}

/// The address of the entry point `ordinal` of the module whose handle is
/// `module`: a 16:16 address, a constant's value as offset with selector
/// FFFFh; 0 if it has none.
pub fn entry_point(cpu: &Cpu, module: u16, ordinal: u16) -> u32 {
    let Some(index) = super::module_index(cpu, module) else {
        return 0;
    };
    let m = &cpu.bus.win32.win16.modules[index];
    if let Some(b) = m.builtin {
        let apis = builtin::MODULES[b as usize].apis;
        let Some(i) = apis
            .iter()
            .position(|a| a.ordinal == ordinal && ordinal != 0)
        else {
            return 0;
        };
        return match apis[i].run {
            builtin::Run::Value(f) => (CONSTANT_SEL as u32) << 16 | f(cpu) as u32,
            _ => {
                let sel = m.segments.first().map_or(0, |s| s.selector);
                ((sel as u32) << 16) | (i as u32 * builtin::STUB)
            }
        };
    }
    let parsed = match parsed_of(cpu, m) {
        Some(p) => p,
        None => return 0,
    };
    match parsed.entry(ordinal) {
        Some(ne::Entry::Code {
            segment, offset, ..
        }) => match (segment as usize)
            .checked_sub(1)
            .and_then(|i| m.segments.get(i))
        {
            Some(s) => (s.selector as u32) << 16 | offset as u32,
            None => 0,
        },
        Some(ne::Entry::Constant { value, .. }) => (CONSTANT_SEL as u32) << 16 | value as u32,
        _ => 0,
    }
}

/// The ordinal of the entry point named `name` of the module `module`.
pub fn ordinal_of(cpu: &Cpu, module: u16, name: &str) -> Option<u16> {
    let index = super::module_index(cpu, module)?;
    let m = &cpu.bus.win32.win16.modules[index];
    if let Some(b) = m.builtin {
        return builtin::MODULES[b as usize]
            .apis
            .iter()
            .find(|a| a.ordinal != 0 && a.name.eq_ignore_ascii_case(name))
            .map(|a| a.ordinal);
    }
    let parsed = parsed_of(cpu, m)?;
    parsed
        .resident_names
        .iter()
        .chain(&parsed.nonresident_names)
        .skip(1)
        .find(|(n, o)| *o != 0 && n.eq_ignore_ascii_case(name))
        .map(|(_, o)| *o)
}

/// The module's header and tables: its file's, or a system module's
/// image's, built again.
pub fn parsed_of(cpu: &Cpu, m: &Module16) -> Option<ne::Module> {
    match m.builtin {
        Some(b) => ne::Module::parse(&builtin::image(cpu, b as usize)).ok(),
        None => ne::Module::parse(&m.file).ok(),
    }
}

/// Write the relocations `relocations` of a segment, whose bytes are
/// `bytes`, of the module `parsed` (whose segments are `segments` and
/// whose imports `deps`).
fn relocate(
    cpu: &mut Cpu,
    parsed: &ne::Module,
    segments: &[Segment16],
    deps: &[u16],
    bytes: &mut [u8],
    relocations: &[ne::Relocation],
    index: usize,
) {
    let self_handle = cpu.bus.win32.win16.modules[index].handle;
    for r in relocations {
        let address = match &r.target {
            ne::Target::Ordinal { module, ordinal } => {
                let dep = (*module as usize)
                    .checked_sub(1)
                    .and_then(|i| deps.get(i))
                    .copied()
                    .unwrap_or(0);
                let a = entry_point(cpu, dep, *ordinal);
                if a == 0 {
                    unresolved(cpu, dep, &format!("#{}", ordinal))
                } else {
                    a
                }
            }
            ne::Target::Name {
                module,
                name_offset,
            } => {
                let dep = (*module as usize)
                    .checked_sub(1)
                    .and_then(|i| deps.get(i))
                    .copied()
                    .unwrap_or(0);
                let name = parsed.imported_name(*name_offset).unwrap_or_default();
                let a = ordinal_of(cpu, dep, &name).map_or(0, |o| entry_point(cpu, dep, o));
                if a == 0 {
                    unresolved(cpu, dep, &name)
                } else {
                    a
                }
            }
            ne::Target::Internal { segment, offset } => {
                if *segment == 0xFF {
                    entry_point(cpu, self_handle, *offset)
                } else {
                    match (*segment as usize)
                        .checked_sub(1)
                        .and_then(|i| segments.get(i))
                    {
                        Some(s) => (s.selector as u32) << 16 | *offset as u32,
                        None => 0,
                    }
                }
            }
            // The floating-point emulator's fixups: INT 34h-3Dh do their
            // instruction (dos.rs), as Wine has them.
            ne::Target::OsFixup(_) => continue,
        };
        let (sel, off) = ((address >> 16) as u16, address as u16);
        let word = |bytes: &[u8], at: usize| {
            bytes
                .get(at..at + 2)
                .map_or(0xFFFF, |b| u16::from_le_bytes([b[0], b[1]]))
        };
        let put = |bytes: &mut [u8], at: usize, v: u16| {
            if let Some(b) = bytes.get_mut(at..at + 2) {
                b.copy_from_slice(&v.to_le_bytes());
            }
        };
        // The 32-bit offsets of a segment flagged 32-bit: the offset
        // widened, which the program's 32-bit code reads whole.
        let dword = |bytes: &[u8], at: usize| {
            bytes
                .get(at..at + 4)
                .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        };
        let put32 = |bytes: &mut [u8], at: usize, v: u32| {
            if let Some(b) = bytes.get_mut(at..at + 4) {
                b.copy_from_slice(&v.to_le_bytes());
            }
        };
        let kind = r.source & 0x7F;
        let mut at = r.offset as usize;
        if r.additive {
            match kind {
                ne::ADDR_LOBYTE => {
                    if let Some(b) = bytes.get_mut(at) {
                        *b = b.wrapping_add(off as u8);
                    }
                }
                ne::ADDR_OFFSET => {
                    let v = word(bytes, at).wrapping_add(off);
                    put(bytes, at, v);
                }
                ne::ADDR_FARPTR => {
                    let v = word(bytes, at).wrapping_add(off);
                    put(bytes, at, v);
                    put(bytes, at + 2, sel);
                }
                ne::ADDR_SELECTOR => {
                    if word(bytes, at) == 0 {
                        put(bytes, at, sel);
                    }
                }
                ne::ADDR_OFFSET32 => {
                    let v = dword(bytes, at).wrapping_add(off as u32);
                    put32(bytes, at, v);
                }
                ne::ADDR_FARPTR48 => {
                    let v = dword(bytes, at).wrapping_add(off as u32);
                    put32(bytes, at, v);
                    put(bytes, at + 4, sel);
                }
                _ => unknown_fixup(cpu, kind),
            }
            continue;
        }
        // A chain: each place has the offset of the next, to FFFFh.
        for _ in 0..0x8000 {
            let next = word(bytes, at);
            match kind {
                ne::ADDR_LOBYTE => {
                    if let Some(b) = bytes.get_mut(at) {
                        *b = off as u8;
                    }
                }
                ne::ADDR_OFFSET => put(bytes, at, off),
                ne::ADDR_FARPTR => {
                    put(bytes, at, off);
                    put(bytes, at + 2, sel);
                }
                ne::ADDR_SELECTOR => put(bytes, at, sel),
                ne::ADDR_OFFSET32 => put32(bytes, at, off as u32),
                ne::ADDR_FARPTR48 => {
                    put32(bytes, at, off as u32);
                    put(bytes, at + 4, sel);
                }
                _ => {
                    unknown_fixup(cpu, kind);
                    break;
                }
            }
            if next as usize == at || next as usize >= bytes.len() {
                break;
            }
            at = next as usize;
        }
    }
}

/// A relocation of a kind the loader doesn't know, left as the file has
/// it.
fn unknown_fixup(cpu: &mut Cpu, kind: u8) {
    cpu.bus
        .log_string(&format!("[W16] A relocation of kind {kind} isn't applied"));
}

/// An import of `what` from `module`, which it doesn't have: where a
/// call of it goes, a stub that ends the program saying so.
fn unresolved(cpu: &mut Cpu, module: u16, what: &str) -> u32 {
    let name = super::module_index(cpu, module).map_or("?".to_string(), |i| {
        cpu.bus.win32.win16.modules[i].name.clone()
    });
    cpu.bus.log_string(&format!(
        "[W16] An import of {}.{} the module doesn't have",
        name, what
    ));
    super::builtin::missing_address(cpu, format!("{}.{}", name, what))
}

/// Patch the prologues of the exported functions of the code segment
/// `segnum`, whose bytes are `bytes`, as Windows 3.1's loader does:
/// "push ds; pop ax" and "mov ax, ds" become "mov ax, DGROUP" in a
/// function using the shared data segment, and "nop; nop" in an exported
/// one of a module with an instance for each task, which gets its data
/// segment in AX from its caller.
fn patch_prologues(
    parsed: &ne::Module,
    segnum: u8,
    bytes: &mut [u8],
    segments: &[Segment16],
    dgroup: u16,
) {
    if parsed.auto_data == 0 || dgroup == 0 {
        return;
    }
    let dgroup_sel = segments
        .get(parsed.auto_data as usize - 1)
        .map_or(0, |s| s.selector);
    for entry in &parsed.entries {
        let ne::Entry::Code {
            segment,
            offset,
            flags,
            ..
        } = *entry
        else {
            continue;
        };
        if segment != segnum {
            continue;
        }
        let at = offset as usize;
        let Some(p) = bytes.get_mut(at..at + 3) else {
            continue;
        };
        if p[2] != 0x90 {
            continue;
        }
        if p[0] == 0x1E && p[1] == 0x58 {
            p[0] = 0x8C;
            p[1] = 0xD8;
        }
        if p[0] == 0x8C && p[1] == 0xD8 {
            if flags & ne::ENTRY_SHARED_DATA != 0 {
                p[0] = 0xB8;
                p[1..3].copy_from_slice(&dgroup_sel.to_le_bytes());
            } else if parsed.flags & ne::MULTIPLEDATA != 0 && flags & ne::ENTRY_EXPORTED != 0 {
                p[0] = 0x90;
                p[1] = 0x90;
            }
        }
    }
}

/// The system modules, which the Win16 process loads when it starts and
/// which stay. MMSYSTEM is one, as SYSTEM.INI's "drivers=mmsystem.dll"
/// has it on Windows 3.1 and 95: programs ask GetModuleHandle for it to
/// see whether there is multimedia sound.
pub const SYSTEM_MODULES: [&str; 8] = [
    "KERNEL", "USER", "GDI", "KEYBOARD", "SYSTEM", "DISPLAY", "SOUND", "MMSYSTEM",
];

/// Let go of a reference to the module `handle`: at the last, it goes,
/// and so do the references it held to the modules it imports from.
pub fn release(cpu: &mut Cpu, handle: u16) {
    let Some(index) = super::module_index(cpu, handle) else {
        return;
    };
    let m = &mut cpu.bus.win32.win16.modules[index];
    // The system's own modules stay; the other built-in libraries go when
    // the last user frees them, as DLLs do.
    if m.builtin.is_some() && SYSTEM_MODULES.contains(&m.name.as_str()) {
        m.refs = m.refs.saturating_sub(1).max(1);
        return;
    }
    m.refs = m.refs.saturating_sub(1);
    if m.refs > 0 {
        return;
    }
    let deps = m.deps.clone();
    unload(cpu, index);
    for d in deps {
        release(cpu, d);
    }
}

/// Take the module at `index` away: its blocks, and its entry.
fn unload(cpu: &mut Cpu, index: usize) {
    let m = cpu.bus.win32.win16.modules.remove(index);
    let code: Vec<u16> = m
        .segments
        .iter()
        .filter(|s| s.flags & ne::SEG_DATA == 0)
        .map(|s| s.selector | 3)
        .collect();
    super::hook::module_unloaded(cpu, &code);
    for s in &m.segments {
        global::free(cpu, s.handle);
    }
    global::free_owned(cpu, m.handle);
    global::free(cpu, m.handle);
    if m.resources32 != 0 {
        let pid = cpu.bus.win32.win16.pid;
        super::with_win16(cpu, pid, |p, bus| {
            p.memory.release_region(bus, m.resources32);
            p.modules.retain(|x| x.base != m.resources32);
        });
    }
}

/// A new instance of the program module at `index`: its automatic data
/// segment, the module's own for the first instance, a fresh copy from
/// the file, relocated, for the others. The module's own stays with an
/// instance that ended while others kept the module loaded, since its
/// data is that instance's: later instances get copies. Its handle, or
/// 0.
pub fn new_instance(cpu: &mut Cpu, index: usize) -> u16 {
    let m = cpu.bus.win32.win16.modules[index].clone();
    let used = cpu
        .bus
        .win32
        .win16
        .tasks
        .iter()
        .any(|t| t.instance == m.dgroup);
    if m.dgroup == 0 || !used {
        return m.dgroup;
    }
    let Ok(parsed) = ne::Module::parse(&m.file) else {
        return 0;
    };
    let n = parsed.auto_data as usize;
    let Some(seg) = parsed.segments.get(n - 1) else {
        return 0;
    };
    let size = m.segments[n - 1].size;
    let mut flags = global::GMEM_ZEROINIT;
    if seg.flags & ne::SEG_MOVEABLE != 0 {
        flags |= global::GMEM_MOVEABLE;
    }
    let handle = global::alloc(cpu, flags, size, m.handle, false);
    if handle == 0 {
        return 0;
    }
    let mut segments = m.segments.clone();
    segments[n - 1].handle = handle;
    segments[n - 1].selector = global::handle_to_sel(handle);
    let mut bytes = seg.data(&m.file).unwrap_or(&[]).to_vec();
    bytes.resize(size as usize, 0);
    let relocations = seg.relocations(&m.file).unwrap_or_default();
    relocate(
        cpu,
        &parsed,
        &segments,
        &m.deps,
        &mut bytes,
        &relocations,
        index,
    );
    if let Some(a) = global::get(cpu, handle) {
        super::poke(cpu, a.base, &bytes);
    }
    handle
}

/// The selector of the code segment of the loaded system module
/// `builtin`.
pub fn builtin_selector(cpu: &Cpu, builtin: usize) -> Option<u16> {
    cpu.bus
        .win32
        .win16
        .modules
        .iter()
        .find(|m| m.builtin == Some(builtin as u16))
        .and_then(|m| m.segments.first())
        .map(|s| s.selector)
}
