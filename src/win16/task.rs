//! Tasks: the Win16 process, which the first Win16 program to start
//! makes, and the programs' instances in it, each a thread. A task starts
//! at its program's entry point with the registers Windows 3.1 gives it;
//! its startup code calls InitTask, which makes the local heap, runs the
//! entry points of the libraries the program came with, and returns the
//! command line. The task ends when its program calls DOS's exit (INT 21h
//! AH=4Ch), or faults.
//!
//! Tasks take turns as Windows 3.1's do (the Win16 lock): the task that
//! last ran goes on until it waits (for a message, mostly) or yields;
//! then another ready task runs. The scheduler asks `may_run`.

use super::thunk::{self, EAX, EBX, ECX, EDI, ESI, ESP};
use super::{Task16, global, ldt, module};
use crate::cpu::{Cpu, Seg};
use crate::dpmi::Context;
use crate::win32::memory::{GRANULARITY, PAGE, PRIVATE, SHARED, SYSTEM, USER, WRITABLE};
use crate::win32::object::Kind;
use crate::win32::thread::Thread;
use rust_dos_ne as ne;
use rust_dos_pe as pe;

/// The most tasks there are at once: the LDT's TIB selectors before the
/// first selector given out.
const MAX_TASKS: usize = (ldt::FIRST as usize) - ((crate::win32::system::TIB3 as usize) >> 3);

/// The Win16 process's file name and where it is said to be.
const PROCESS_NAME: &str = "KRNL386.EXE";

/// The places in the first megabyte KERNEL exports selectors of.
pub const REAL_SEGMENTS: [u16; 10] = [
    0x0000, 0x0040, 0xA000, 0xB000, 0xB800, 0xC000, 0xD000, 0xE000, 0xF000, 0xF000,
];

/// The built-in DLLs the system modules' functions go on to, mapped in
/// the Win16 process.
const DLLS: &[&str] = &[
    "USER32.DLL",
    "GDI32.DLL",
    "WINMM.DLL",
    "SHELL32.DLL",
    "COMDLG32.DLL",
    "LZ32.DLL",
    "VERSION.DLL",
    "ADVAPI32.DLL",
];

/// The image of the Win16 process's program: one byte of code, which
/// never runs, as its main thread is the first task's.
fn process_image() -> Vec<u8> {
    let mut b = pe::Builder::exe(0x0040_0000);
    b.subsystem(pe::SUBSYSTEM_GUI);
    let rva = b.section(
        ".text",
        &[0xCC],
        pe::SCN_CODE | pe::SCN_EXECUTE | pe::SCN_READ,
    );
    b.entry(rva);
    b.build()
}

/// Make the Win16 process, if there is none: its process with the
/// environment `environment`, its tables, the global heap, the system
/// modules. Its main thread, which doesn't run, is the first task's.
/// Whether it was made now.
pub(super) fn ensure_process(cpu: &mut Cpu, environment: Vec<String>) -> Result<bool, String> {
    if super::win16_ref(cpu).is_some() {
        return Ok(false);
    }
    make_process(cpu, environment).inspect_err(|_| discard_process(cpu))?;
    Ok(true)
}

/// Take away the Win16 process `ensure_process` made, when it or its
/// first task couldn't be made: no task of it ever ran.
fn discard_process(cpu: &mut Cpu) {
    let pid = cpu.bus.win32.win16.pid;
    if pid != 0 {
        crate::win32::discard_process(cpu, pid);
    }
    cpu.bus.win32.win16 = super::State::default();
}

/// Make the Win16 process (see `ensure_process`).
fn make_process(cpu: &mut Cpu, environment: Vec<String>) -> Result<(), String> {
    cpu.bus.win32.win16 = super::State::default();
    let path = format!("{}\\{}", crate::win32::loader::SYSTEM_DIR, PROCESS_NAME);
    let created = crate::win32::create_process_from(
        cpu,
        &path,
        &path,
        &process_image(),
        environment,
        Vec::new(),
        None,
        Vec::new(),
        true,
    )?;
    let pid = created.process.1;
    cpu.bus.win32.win16.pid = pid;
    cpu.bus.win32.user.input.win16 = pid;
    let made = super::with_win16(cpu, pid, |p, bus| {
        let mut memory = std::mem::take(&mut p.memory);
        let result = (|| {
            for dll in DLLS {
                let index = crate::win32::stubs::find_dll(dll)?;
                crate::win32::loader::map_builtin(bus, &mut memory, p, index)?;
            }
            // The first megabyte, for the selectors of its places.
            for page in 0..0x100 {
                memory.map(bus, page * PAGE, page * PAGE, USER | WRITABLE)?;
            }
            Some(())
        })();
        p.memory = memory;
        result
    })
    .flatten();
    if made.is_none() {
        return Err("There isn't enough memory for Windows' 16-bit system.".to_string());
    }
    let system = super::win16(cpu).system;
    crate::win32::system::open_soft_ints(cpu, system);
    super::dos::init(cpu)?;
    // The global heap.
    let heap = super::with_win16(cpu, pid, |p, bus| {
        crate::win32::heap::Heap::create(&mut p.memory, bus, 0, 0)
    })
    .flatten()
    .ok_or("There isn't enough memory for the global heap.")?;
    cpu.bus.win32.win16.heap = heap;
    thunk::init(cpu).ok_or("There isn't enough memory for the thunks.")?;
    // The selectors of the first megabyte's places.
    for seg in REAL_SEGMENTS {
        if cpu
            .bus
            .win32
            .win16
            .real_selectors
            .iter()
            .any(|&(s, _)| s == seg)
        {
            continue;
        }
        let sel = cpu.bus.win32.win16.ldt.alloc(1).ok_or("The LDT is full.")?;
        ldt::write(
            cpu,
            system,
            sel,
            ldt::segment(seg as u32 * 16, 0xFFFF, ldt::ACCESS_DATA),
        );
        cpu.bus.win32.win16.real_selectors.push((seg, sel));
    }
    // The system modules every program has.
    let mut inits = Vec::new();
    for name in module::SYSTEM_MODULES {
        module::load(cpu, name, "", &mut inits)?;
    }
    cpu.bus.log_string("[W16] Made the Win16 process");
    Ok(())
}

/// The selector KERNEL exports for the place `seg` of the first
/// megabyte.
pub fn real_selector(cpu: &Cpu, seg: u16) -> u16 {
    cpu.bus
        .win32
        .win16
        .real_selectors
        .iter()
        .find(|&&(s, _)| s == seg)
        .map_or(0, |&(_, sel)| sel)
}

/// The program at `path` DOS loaded starts, with the command line `line`
/// (its path, then its arguments): the Win16 process is made, and its
/// first task. Fails with a message for the user.
pub fn start(
    cpu: &mut Cpu,
    path: &str,
    line: &str,
    environment: Vec<String>,
) -> Result<(), String> {
    let arguments = line.get(path.len()..).unwrap_or("").to_string();
    let made = ensure_process(cpu, environment)?;
    if let Err(e) = create_task(cpu, path, &arguments, 1, 0) {
        if made {
            discard_process(cpu);
        }
        return Err(e);
    }
    cpu.bus.win32.root = cpu.bus.win32.win16.pid;
    Ok(())
}

/// What `spawn` made: the task's handle, its process object and id, and
/// its thread's object and id.
pub struct Spawned {
    pub task: u16,
    pub process: (u32, u32),
    pub thread: (u32, u32),
}

/// Start the program at `path` with `arguments` as a new task, shown as
/// `show` says, from the task `parent` (0 for a Win32 program): the
/// Win16 process is made if there is none. Fails with a message.
pub fn spawn(
    cpu: &mut Cpu,
    path: &str,
    arguments: &str,
    show: u16,
    parent: u16,
    environment: Vec<String>,
) -> Result<Spawned, String> {
    let made = ensure_process(cpu, environment)?;
    let i = match create_task(cpu, path, arguments, show, parent) {
        Ok(i) => i,
        Err(e) => {
            if made {
                discard_process(cpu);
            }
            return Err(e);
        }
    };
    let task = &cpu.bus.win32.win16.tasks[i];
    let tid = task.tid;
    let thread_object = super::win16_ref(cpu)
        .and_then(|p| p.threads.iter().find(|t| t.id == tid))
        .map_or(0, |t| t.object);
    Ok(Spawned {
        task: task.handle,
        process: (task.process_object, task.process_id),
        thread: (thread_object, tid),
    })
}

/// Make a task of the program at `path`. Its place in `State::tasks`.
fn create_task(
    cpu: &mut Cpu,
    path: &str,
    arguments: &str,
    show: u16,
    parent: u16,
) -> Result<usize, String> {
    let live = cpu
        .bus
        .win32
        .win16
        .tasks
        .iter()
        .filter(|t| t.exit_code.is_none())
        .count();
    if live >= MAX_TASKS {
        return Err("There are too many Windows programs running.".to_string());
    }
    let mut inits = Vec::new();
    let index = module::load(cpu, path, "", &mut inits)?;
    let m = cpu.bus.win32.win16.modules[index].clone();
    if m.flags & ne::LIBMODULE != 0 {
        module::release(cpu, m.handle);
        return Err(format!("{} is a library, not a program.", path));
    }
    let parsed = module::parsed_of(cpu, &m).ok_or("The program can't be read.")?;
    let prev = cpu
        .bus
        .win32
        .win16
        .tasks
        .iter()
        .filter(|t| t.exit_code.is_none() && t.module == m.handle)
        .map(|t| t.instance)
        .next_back()
        .unwrap_or(0);
    let instance = module::new_instance(cpu, index);
    if parsed.auto_data != 0 && instance == 0 {
        module::release(cpu, m.handle);
        return Err(format!("There isn't enough memory to run {}.", m.name));
    }

    // The task database, with the PDB at 100h, which has a selector of
    // its own.
    let tdb = global::alloc(cpu, global::GMEM_ZEROINIT, 0x200, 0, false);
    let psp = cpu.bus.win32.win16.ldt.alloc(1).unwrap_or(0);
    if tdb == 0 || psp == 0 {
        module::release(cpu, m.handle);
        return Err(format!("There isn't enough memory to run {}.", m.name));
    }
    let tdb_base = global::get(cpu, tdb).map_or(0, |a| a.base);
    let system = super::win16(cpu).system;
    ldt::write(
        cpu,
        system,
        psp,
        ldt::segment(tdb_base + 0x100, 0xFF, ldt::ACCESS_DATA),
    );
    if let Some(a) = cpu.bus.win32.win16.arena.get_mut(&ldt::index(tdb)) {
        a.owner = tdb;
    }
    let environment = environment_block(cpu, &m.path, tdb);
    write_tdb(
        cpu,
        tdb_base,
        tdb,
        psp,
        &m,
        instance,
        prev,
        show,
        parent,
        environment,
        arguments,
    );

    // The 16-bit stack, in the automatic data segment unless the header
    // says otherwise.
    let (ss_seg, sp) = parsed.ss_sp;
    let ss = if ss_seg == 0 || ss_seg == parsed.auto_data {
        global::handle_to_sel(instance)
    } else {
        m.segments
            .get(ss_seg as usize - 1)
            .map_or(0, |s| s.selector)
    };
    let sp = if sp != 0 {
        sp
    } else {
        let seg = parsed.segments.get(parsed.auto_data.max(1) as usize - 1);
        (seg.map_or(0, |s| s.min_alloc) as u16).wrapping_add(parsed.stack)
    } & !1;
    let (cs_seg, ip) = parsed.cs_ip;
    let Some(cs) = m
        .segments
        .get(cs_seg.max(1) as usize - 1)
        .map(|s| s.selector)
    else {
        module::release(cpu, m.handle);
        return Err(format!("{} has no entry point.", m.name));
    };

    // Its registers as Windows 3.1 starts a program with them.
    let mut ctx = Context {
        eip: ip as u32,
        eflags: 0x0202,
        seg: [psp, cs, ss, global::handle_to_sel(instance), 0, 0],
        ..Context::default()
    };
    ctx.gpr[EBX] = parsed.stack as u32;
    ctx.gpr[ECX] = parsed.heap as u32;
    ctx.gpr[EDI] = instance as u32;
    ctx.gpr[ESI] = prev as u32;
    ctx.gpr[ESP] = sp as u32;

    // Its thread: the Win16 process's main thread for the first task,
    // else a new one.
    let first = cpu.bus.win32.win16.tasks.is_empty();
    let (tid, stack32) = if first {
        let p = super::win16(cpu);
        let thread = &mut p.threads[0];
        thread.ctx = ctx;
        thread.suspended = 0;
        (thread.id, thread.stack.1)
    } else {
        match new_thread(cpu, ctx) {
            Some(made) => made,
            None => {
                module::release(cpu, m.handle);
                return Err(format!("There isn't enough memory to run {}.", m.name));
            }
        }
    };
    // Its process object, which its starters wait on.
    let win32 = &mut cpu.bus.win32;
    let process_id = if first {
        win32.win16.pid
    } else {
        win32.new_id()
    };
    let process_object = win32.objects.add(
        Kind::Process {
            id: process_id,
            exit_code: None,
        },
        None,
    );
    win32.win16.tasks.push(Task16 {
        tid,
        handle: tdb,
        module: m.handle,
        instance,
        psp,
        stack16: (ss, sp),
        stack32,
        frames: Vec::new(),
        arguments: arguments.to_string(),
        show,
        parent,
        process_object,
        process_id,
        exit_code: None,
        dta: (psp as u32) << 16 | 0x80,
        error_mode: 0,
        yielding: false,
        vectors: Vec::new(),
        inits: inits
            .iter()
            .map(|&i| win32.win16.modules[i].handle)
            .collect(),
    });
    cpu.bus.log_string(&format!(
        "[W16] Task {:04X} of {} starts at {:04X}:{:04X}, stack {:04X}:{:04X}",
        tdb, m.name, cs, ip, ss, sp
    ));
    Ok(cpu.bus.win32.win16.tasks.len() - 1)
}

/// The bytes of a client task's 16-bit stack.
const CLIENT_STACK: u32 = 0x8000;

/// Make a task with no program of its own, which runs the 16-bit code a
/// Win32 process calls (flat.rs), as Windows 95 gives each Win32 process
/// a task database: named `name`, its thread starting at the 32-bit
/// context `ctx`, which gets its stack and TIB selector, waiting. Its
/// thread's id and its handle.
pub(super) fn client_task(
    cpu: &mut Cpu,
    name: &str,
    mut ctx: Context,
) -> Result<(u32, u16), String> {
    let live = cpu
        .bus
        .win32
        .win16
        .tasks
        .iter()
        .filter(|t| t.exit_code.is_none())
        .count();
    let full = || "There are too many Windows programs running.".to_string();
    if live >= MAX_TASKS {
        return Err(full());
    }
    let memory = || format!("There isn't enough memory for {}'s 16-bit calls.", name);
    let tdb = global::alloc(cpu, global::GMEM_ZEROINIT, 0x200, 0, false);
    let psp = cpu.bus.win32.win16.ldt.alloc(1).unwrap_or(0);
    let stack = global::alloc(cpu, global::GMEM_ZEROINIT, CLIENT_STACK, tdb, false);
    if tdb == 0 || psp == 0 || stack == 0 {
        for h in [stack, tdb] {
            if h != 0 {
                global::free(cpu, h);
            }
        }
        if psp != 0 {
            cpu.bus.win32.win16.ldt.free(psp, 1);
        }
        return Err(memory());
    }
    let tdb_base = global::get(cpu, tdb).map_or(0, |a| a.base);
    let system = super::win16(cpu).system;
    ldt::write(
        cpu,
        system,
        psp,
        ldt::segment(tdb_base + 0x100, 0xFF, ldt::ACCESS_DATA),
    );
    if let Some(a) = cpu.bus.win32.win16.arena.get_mut(&ldt::index(tdb)) {
        a.owner = tdb;
    }
    let module = super::Module16 {
        name: name.to_ascii_uppercase(),
        expected_version: 0x0400,
        ..super::Module16::default()
    };
    let environment = environment_block(cpu, name, tdb);
    write_tdb(
        cpu,
        tdb_base,
        tdb,
        psp,
        &module,
        0,
        0,
        1,
        0,
        environment,
        "",
    );
    // Its thread: the Win16 process's main thread if no task has it,
    // else a new one.
    let first = cpu.bus.win32.win16.tasks.is_empty();
    let made = if first {
        let p = super::win16(cpu);
        let thread = &mut p.threads[0];
        thread.suspended = 0;
        Some((thread.id, thread.stack.1))
    } else {
        new_thread(cpu, ctx)
    };
    let Some((tid, stack32)) = made else {
        // Its stack and environment block are the TDB's.
        global::free_owned(cpu, tdb);
        cpu.bus.win32.win16.ldt.free(psp, 1);
        global::free(cpu, tdb);
        return Err(memory());
    };
    let p = super::win16(cpu);
    let thread = p
        .threads
        .iter_mut()
        .find(|t| t.id == tid)
        .expect("the task's thread");
    ctx.gpr[ESP] = stack32;
    ctx.seg[Seg::FS as usize] = thread.selector;
    thread.ctx = ctx;
    thread.wait = Some(crate::win32::thread::Wait::default());
    let process_id = p.id;
    cpu.bus.win32.win16.tasks.push(Task16 {
        tid,
        handle: tdb,
        psp,
        stack16: (global::handle_to_sel(stack), CLIENT_STACK as u16),
        stack32,
        show: 1,
        process_id,
        dta: (psp as u32) << 16 | 0x80,
        ..Task16::default()
    });
    cpu.bus.log_string(&format!(
        "[W16] Task {:04X} runs {}'s 16-bit calls",
        tdb, name
    ));
    Ok((tid, tdb))
}

/// The client task `tid` (`client_task`) ends with its Win32 process:
/// its thread, its blocks and its selectors go. The Win16 process stays,
/// even without tasks; whoever ends the Win32 process sees to that.
pub(super) fn end_client_task(cpu: &mut Cpu, tid: u32) {
    let Some(t) = cpu.bus.win32.win16.tasks.iter().position(|k| k.tid == tid) else {
        return;
    };
    let task = cpu.bus.win32.win16.tasks[t].clone();
    if task.exit_code.is_some() {
        return;
    }
    cpu.bus.win32.win16.tasks[t].exit_code = Some(0);
    cpu.bus.win32.win16.tasks[t].frames.clear();
    if cpu.bus.win32.win16.lock == tid {
        cpu.bus.win32.win16.lock = 0;
    }
    let pid = cpu.bus.win32.win16.pid;
    if let Some(p) = cpu.bus.win32.processes.iter().position(|k| k.id == pid)
        && let Some(i) = cpu.bus.win32.processes[p]
            .threads
            .iter()
            .position(|k| k.id == tid)
    {
        crate::win32::thread::end(&mut cpu.bus.win32, p, i, 0);
    }
    crate::win32::user32::destroy_thread_windows(cpu, tid);
    global::free_owned(cpu, task.handle);
    cpu.bus.win32.win16.ldt.free(task.psp, 1);
    global::free(cpu, task.handle);
}

/// The task's environment block: the Win16 process's environment, then
/// a word 1 and the program's path, as DOS has them. Its selector.
fn environment_block(cpu: &mut Cpu, path: &str, owner: u16) -> u16 {
    let mut bytes = Vec::new();
    if let Some(p) = super::win16_ref(cpu) {
        for v in &p.environment {
            bytes.extend_from_slice(&crate::dosstr::to_bytes(v));
            bytes.push(0);
        }
    }
    bytes.push(0);
    bytes.extend_from_slice(&[1, 0]);
    bytes.extend_from_slice(&crate::dosstr::to_bytes(path));
    bytes.push(0);
    let h = global::alloc(cpu, global::GMEM_ZEROINIT, bytes.len() as u32, owner, false);
    if let Some(a) = global::get(cpu, h) {
        super::poke(cpu, a.base, &bytes);
    }
    global::handle_to_sel(h)
}

/// Fill the task database at `base` (and its PDB at 100h).
#[allow(clippy::too_many_arguments)]
fn write_tdb(
    cpu: &mut Cpu,
    base: u32,
    tdb: u16,
    psp: u16,
    m: &super::Module16,
    instance: u16,
    prev: u16,
    show: u16,
    parent: u16,
    environment: u16,
    arguments: &str,
) {
    let w16 = |cpu: &mut Cpu, at: u32, v: u16| super::write_16(cpu, base + at, v);
    w16(cpu, 0x0C, tdb);
    w16(cpu, 0x0E, prev);
    w16(cpu, 0x14, 0x037F);
    w16(cpu, 0x1A, m.expected_version);
    w16(cpu, 0x1C, instance);
    w16(cpu, 0x1E, m.handle);
    w16(cpu, 0x22, parent);
    w16(cpu, 0x60, psp);
    super::write_32(cpu, base + 0x62, (psp as u32) << 16 | 0x80);
    w16(cpu, 0xA8, show);
    let mut name = m.name.as_bytes().to_vec();
    name.resize(8, 0);
    super::poke(cpu, base + 0xF2, &name);
    w16(cpu, 0xFA, 0x4454);
    // The PDB.
    let pdb = base + 0x100;
    super::poke(cpu, pdb, &[0xCD, 0x20]);
    super::write_16(cpu, pdb + 0x02, 0xA000);
    super::poke(cpu, pdb + 0x18, &[0, 1, 2, 3, 4]);
    super::poke(cpu, pdb + 0x1D, &[0xFF; 15]);
    super::write_16(cpu, pdb + 0x2C, environment);
    super::write_16(cpu, pdb + 0x32, 20);
    super::write_32(cpu, pdb + 0x34, (psp as u32) << 16 | 0x18);
    super::poke(cpu, pdb + 0x50, &[0xCD, 0x21, 0xCB]);
    let mut tail = crate::dosstr::to_bytes(arguments);
    if !tail.is_empty() && tail[0] != b' ' {
        tail.insert(0, b' ');
    }
    tail.truncate(0x7E);
    // Windows 95 ends a command tail with a NUL, as WinMain's command
    // line needs, and only an empty one with DOS's carriage return.
    let mut line = vec![tail.len() as u8];
    line.extend_from_slice(&tail);
    line.push(if tail.is_empty() { 0x0D } else { 0 });
    super::poke(cpu, pdb + 0x80, &line);
}

/// A thread of the Win16 process for a new task, which starts at `ctx`:
/// its TIB, its TIB's selector and its 32-bit stack. Its id and its
/// 32-bit stack's top.
fn new_thread(cpu: &mut Cpu, ctx: Context) -> Option<(u32, u32)> {
    let pid = cpu.bus.win32.win16.pid;
    let index = super::win16(cpu).threads.len();
    if index >= MAX_TASKS {
        return None;
    }
    let selector = crate::win32::thread::selector_of(index);
    let made = super::with_win16(cpu, pid, |p, bus| {
        let memory = &mut p.memory;
        let stack = memory.reserve(None, thunk::STACK32, PRIVATE, SHARED)?;
        let top = stack + thunk::STACK32;
        memory.commit(bus, stack, thunk::STACK32, USER | WRITABLE)?;
        let tib = memory.reserve(None, GRANULARITY, SHARED, SYSTEM)?;
        memory.commit(bus, tib, PAGE, USER | WRITABLE)?;
        let block = crate::win32::loader::process_block(p.command_line);
        for (field, value) in [
            (0x00, 0xFFFF_FFFF),
            (0x04, top),
            (0x08, stack),
            (0x18, tib),
            (0x2C, tib + crate::win32::loader::TLS_ARRAY),
            (crate::win32::loader::TIB_PROCESS, block),
        ] {
            memory.write_32(bus, tib + field, value)?;
        }
        Some((stack, top, tib))
    })??;
    let (stack, top, tib) = made;
    let system = super::win16(cpu).system;
    crate::win32::system::set_tib(cpu, system, selector, tib);
    let win32 = &mut cpu.bus.win32;
    let id = win32.new_id();
    let object = win32.objects.add(
        Kind::Thread {
            id,
            exit_code: None,
        },
        None,
    );
    super::win16(cpu).threads.push(Thread {
        id,
        object,
        tib,
        selector,
        stack: (stack, top),
        ctx,
        ..Thread::default()
    });
    Some((id, top))
}

/// Whether the thread `t` of the process `p` may run now as the Win16
/// lock goes: one that isn't a task's may; a task may when no other task
/// holds the lock and can run.
pub fn may_run(win32: &crate::win32::Win32, p: usize, t: usize) -> bool {
    let win16 = &win32.win16;
    if win16.pid == 0 || win16.lock == 0 {
        return true;
    }
    let Some(process) = win32.processes.get(p) else {
        return true;
    };
    if process.id != win16.pid {
        return true;
    }
    let thread = &process.threads[t];
    if thread.id == win16.lock || !win16.tasks.iter().any(|k| k.tid == thread.id) {
        return true;
    }
    // The holder can't run (it waits, or ended): the lock is free.
    !process
        .threads
        .iter()
        .any(|h| h.id == win16.lock && h.ready())
}

/// The thread `tid` runs: if it is a task's, it takes the Win16 lock.
pub fn entered(win32: &mut crate::win32::Win32, tid: u32) {
    if win32.win16.pid != 0 && win32.win16.tasks.iter().any(|t| t.tid == tid) {
        win32.win16.lock = tid;
    }
}

/// The running task gives up its turn: another task that is ready runs,
/// and this one when its turn comes again. It goes on at `ctx`.
pub fn yield_now(cpu: &mut Cpu, ctx: Context) {
    crate::win32::system::load_context(cpu, &ctx);
    cpu.bus.win32.win16.lock = 0;
    crate::win32::thread::reschedule(cpu);
}

/// InitTask, a register function: the local heap, the libraries' entry
/// points (each a call into 16-bit code, after which it runs again), and
/// the registers the startup code wants back.
pub fn init_task(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let t = super::current_task(cpu)?;
    // The libraries' entry points first, one at a time.
    if let Some(dll) = next_init(cpu, t) {
        let mut resume = Context::of(cpu);
        // Back to the trap, to go on.
        resume.eip = resume.eip.wrapping_sub(4);
        call_lib_init(cpu, dll, resume, thunk::THEN_TASK, Vec::new());
        return None;
    }
    let task = cpu.bus.win32.win16.tasks[t].clone();
    let ds = cpu.ds();
    // INSTANCEDATA at DS:0: the stack's top and bottom.
    let base = cpu.seg_cache(Seg::DS).base;
    let sp = cpu.sp();
    let stack = cpu.bx();
    let stackmin = sp;
    let stacktop = stackmin.saturating_sub(stack).saturating_add(150);
    cpu.bus.guest_write_16(base + 0x0A, stacktop);
    cpu.bus.guest_write_16(base + 0x0C, stackmin);
    cpu.bus.guest_write_16(base + 0x0E, stackmin);
    // The local heap.
    let heap = cpu.cx();
    if heap != 0 {
        super::local::init(cpu, ds, 0, heap);
    }
    // The command line, past its blanks. An empty one is the tail's
    // length byte, 0, as an empty string, where Windows 95 points.
    let pdb = super::lin(cpu, task.psp, 0);
    let len = super::peek(cpu, pdb + 0x80, 1).map_or(0, |b| b[0]);
    let tail = super::peek(cpu, pdb + 0x81, len as usize).unwrap_or_default();
    let blanks = tail
        .iter()
        .take_while(|&&b| b == b' ' || b == b'\t')
        .count();
    cpu.set_ax(1);
    cpu.set_bx(if len == 0 { 0x80 } else { 0x81 + blanks as u16 });
    cpu.set_cx(stacktop);
    cpu.set_dx(task.show);
    let prev = super::read_16(cpu, super::lin(cpu, task.handle, 0x0E));
    cpu.set_si(prev);
    cpu.set_di(task.instance);
    let psp = task.psp;
    load_es(cpu, psp);
    None
}

/// Load ES with `sel`, as the context switch would.
fn load_es(cpu: &mut Cpu, sel: u16) {
    let mut ctx = Context::of(cpu);
    ctx.seg[Seg::ES as usize] = sel;
    crate::win32::system::load_context(cpu, &ctx);
}

/// The next library of the task `t` whose entry point is yet to run.
fn next_init(cpu: &mut Cpu, t: usize) -> Option<u16> {
    let task = &mut cpu.bus.win32.win16.tasks[t];
    if task.inits.is_empty() {
        return None;
    }
    Some(task.inits.remove(0))
}

/// Initialize the library `module` as Windows 95 does when it loads one:
/// its entry point (`call_lib_main`), then, for a library made for
/// Windows 4.0, the DllEntryPoint it exports, with DLL_PROCESS_ATTACH.
/// `then` runs, with `data`, after both.
pub fn call_lib_init(cpu: &mut Cpu, module: u16, resume: Context, then: u16, data: Vec<u32>) {
    let mut chained = vec![then as u32, module as u32];
    chained.extend(data);
    call_lib_main(cpu, module, resume, thunk::THEN_DLL_ENTRY, chained);
}

/// A library's entry point returned (`call_lib_init`): its DllEntryPoint
/// runs next, if it has one, and then the `then` it was given.
pub fn then_dll_entry(cpu: &mut Cpu, _: u32, data: &[u32], ctx: &mut Context) -> Option<u32> {
    let then = data.first().copied().unwrap_or(0) as u16;
    let module = data.get(1).copied().unwrap_or(0) as u16;
    let rest = data.get(2..).map(<[u32]>::to_vec).unwrap_or_default();
    let Some((target, instance, ds, heap)) = dll_entry_point(cpu, module) else {
        return thunk::THENS[then as usize](cpu, 0, &rest, ctx);
    };
    cpu.bus.log_string(&format!(
        "[W16] Calling DllEntryPoint at {:04X}:{:04X}",
        target >> 16,
        target & 0xFFFF
    ));
    let mut regs = Context::default();
    regs.seg[Seg::DS as usize] = ds;
    regs.seg[Seg::ES as usize] = ds;
    regs.gpr[EAX] = ds as u32;
    // DllEntryPoint(dwReason, hInst, wDS, wHeapSize, dwReserved1,
    // wReserved2), Pascal's order.
    let args = [0, 1, instance, ds, heap, 0, 0, 0];
    let stack = thunk::stack16_for(cpu, ctx);
    thunk::call16_regs(cpu, target, &args, regs, *ctx, then, rest, stack);
    None
}

/// The DllEntryPoint of the library `module`, if it is one made for
/// Windows 4.0 that exports one: its 16:16 address, the library's
/// instance, its data segment's selector and its local heap's size.
fn dll_entry_point(cpu: &Cpu, module: u16) -> Option<(u32, u16, u16, u16)> {
    let m = &cpu.bus.win32.win16.modules[super::module_index(cpu, module)?];
    if m.builtin.is_some() || m.flags & ne::LIBMODULE == 0 || m.expected_version < 0x0400 {
        return None;
    }
    let ordinal = module::ordinal_of(cpu, m.handle, "DllEntryPoint")?;
    let target = module::entry_point(cpu, m.handle, ordinal);
    if target == 0 {
        return None;
    }
    let heap = module::parsed_of(cpu, m)?.heap;
    let instance = if m.dgroup != 0 { m.dgroup } else { m.handle };
    Some((target, instance, global::handle_to_sel(m.dgroup), heap))
}

/// Call the entry point (LibMain's startup) of the library `module`, the
/// context `resume` going on afterwards, as Windows 3.1 calls it: CX the
/// heap's size, DI the instance, DS and ES its data segment. `then`
/// runs, with `data`, when it returns; when there is no entry point,
/// at once.
pub fn call_lib_main(cpu: &mut Cpu, module: u16, mut resume: Context, then: u16, data: Vec<u32>) {
    let skip = |cpu: &mut Cpu, resume: &mut Context, data: &[u32]| {
        if let Some(value) = thunk::THENS[then as usize](cpu, 0, data, resume) {
            resume.gpr[EAX] = resume.gpr[EAX] & 0xFFFF_0000 | value & 0xFFFF;
            crate::win32::system::load_context(cpu, resume);
        }
    };
    let Some(index) = super::module_index(cpu, module) else {
        skip(cpu, &mut resume, &data);
        return;
    };
    let m = cpu.bus.win32.win16.modules[index].clone();
    let Some(parsed) = module::parsed_of(cpu, &m) else {
        skip(cpu, &mut resume, &data);
        return;
    };
    let (cs_seg, ip) = parsed.cs_ip;
    let Some(cs) = (cs_seg != 0)
        .then(|| m.segments.get(cs_seg as usize - 1))
        .flatten()
        .map(|s| s.selector)
    else {
        skip(cpu, &mut resume, &data);
        return;
    };
    let ds = global::handle_to_sel(m.dgroup);
    let mut regs = Context::default();
    regs.gpr[ECX] = parsed.heap as u32;
    regs.gpr[EDI] = if m.dgroup != 0 { m.dgroup } else { m.handle } as u32;
    regs.seg[Seg::DS as usize] = ds;
    regs.seg[Seg::ES as usize] = ds;
    regs.gpr[EAX] = ds as u32;
    cpu.bus.log_string(&format!(
        "[W16] Calling {}'s entry point at {:04X}:{:04X}",
        m.name, cs, ip
    ));
    let stack = thunk::stack16_for(cpu, &resume);
    thunk::call16_regs(
        cpu,
        (cs as u32) << 16 | ip as u32,
        &[],
        regs,
        resume,
        then,
        data,
        stack,
    );
}

/// A library's entry point returned: the context goes on as it was.
pub fn then(_: &mut Cpu, _: u32, _: &[u32], ctx: &mut Context) -> Option<u32> {
    Some(ctx.gpr[EAX])
}

/// WinExec and EXEC from the running task: the program at `path` as a
/// new task. Its instance, or the error below 32 Windows gives.
pub fn exec(cpu: &mut Cpu, path: &str, arguments: &str, show: u16) -> Result<u16, u16> {
    let parent = super::current_task(cpu).map_or(0, |t| cpu.bus.win32.win16.tasks[t].handle);
    let environment = super::win16_ref(cpu).map_or(Vec::new(), |p| p.environment.clone());
    let path = if path.contains('.') {
        path.to_string()
    } else {
        format!("{}.EXE", path)
    };
    let (path, file) = match cpu.read_program_file(&path) {
        Some(file) => (path, file),
        None => {
            let windows = format!("{}\\{}", crate::win32::loader::WINDOWS_DIR, path);
            if path.contains(['\\', '/', ':']) {
                return Err(2);
            }
            let file = cpu.read_program_file(&windows).ok_or(2u16)?;
            (windows, file)
        }
    };
    // The module's path as Windows keeps it, whatever form the caller
    // gave: full, with backslashes (D:/METAL.EXE is D:\METAL.EXE), which
    // GetModuleFileName gives and the module's DLLs are looked for by.
    let path = cpu.bus.disk.qualify_path(&path).unwrap_or(path);
    if pe::is_pe(&file) {
        return exec_win32(cpu, &path, arguments, show, environment);
    }
    match spawn(cpu, &path, arguments, show, parent, environment) {
        Ok(spawned) => Ok(cpu
            .bus
            .win32
            .win16
            .tasks
            .iter()
            .find(|t| t.handle == spawned.task)
            .map_or(0, |t| t.instance)),
        Err(message) => {
            cpu.bus
                .log_string(&format!("[W16] {} can't start: {}", path, message));
            Err(if message.contains("memory") { 8 } else { 11 })
        }
    }
}

/// `exec` of the Win32 program at `path`: as on Windows 98, a process of
/// its own, which runs beside the Win16 tasks, as the Win32 WinExec
/// starts it. 33 once it started, as Wine's WinExec16 gives for one, or
/// the error.
fn exec_win32(
    cpu: &mut Cpu,
    path: &str,
    arguments: &str,
    show: u16,
    environment: Vec<String>,
) -> Result<u16, u16> {
    let name = if path.contains([' ', '\t']) {
        format!("\"{}\"", path)
    } else {
        path.to_string()
    };
    let line = if arguments.is_empty() {
        name
    } else {
        format!("{} {}", name, arguments)
    };
    let startup = crate::win32::kernel32::startup_showing(show);
    let started = crate::win32::kernel32::spawn(
        cpu,
        Some(path),
        Some(&line),
        Some(environment),
        false,
        0,
        startup,
    );
    match started {
        Ok([process, thread, _, _]) => {
            crate::win32::object::close(cpu, process);
            crate::win32::object::close(cpu, thread);
            Ok(33)
        }
        Err(code) => {
            cpu.bus
                .log_string(&format!("[W16] {} can't start: error {}", path, code));
            Err(match code {
                2 | 8 => code as u16,
                _ => 11,
            })
        }
    }
}

/// The running task ends with exit code `code`: its windows, blocks and
/// modules go, its starters learn of its end, and its thread ends, the
/// Win16 process with the last task's.
pub fn exit(cpu: &mut Cpu, code: u16) {
    let Some(t) = super::current_task(cpu) else {
        crate::win32::exit(cpu, code as u32);
        return;
    };
    let task = cpu.bus.win32.win16.tasks[t].clone();
    cpu.bus.log_string(&format!(
        "[W16] Task {:04X} ended with exit code {:X}",
        task.handle, code
    ));
    cpu.bus.win32.win16.tasks[t].exit_code = Some(code);
    cpu.bus.win32.win16.tasks[t].frames.clear();
    if task.module != 0 {
        cpu.bus.win32.win16.exit_code = code;
    }
    if cpu.bus.win32.win16.lock == task.tid {
        cpu.bus.win32.win16.lock = 0;
    }
    // A Win32 process's client: the threads that wait for it go on.
    super::flat::task_ending(cpu, task.tid);
    // The process object its starters wait on.
    let objects = &mut cpu.bus.win32.objects;
    if let Some(object) = objects.get_mut(task.process_object)
        && let Kind::Process { exit_code, .. } = &mut object.kind
    {
        *exit_code = Some(code as u32);
    }
    objects.release(task.process_object);
    let others = cpu
        .bus
        .win32
        .win16
        .tasks
        .iter()
        .any(|k| k.exit_code.is_none());
    if !others {
        // The last task: the Win16 process ends.
        crate::win32::exit(cpu, code as u32);
        return;
    }
    crate::win32::user32::destroy_thread_windows(cpu, task.tid);
    global::free_owned(cpu, task.handle);
    let shared = cpu
        .bus
        .win32
        .win16
        .modules
        .iter()
        .any(|m| m.dgroup == task.instance);
    let freed = task.instance != 0 && !shared;
    let ds = if freed { global::handle_to_sel(task.instance) } else { 0 };
    super::hook::task_ended(cpu, task.tid, ds);
    if freed {
        global::free(cpu, task.instance);
    }
    cpu.bus.win32.win16.ldt.free(task.psp, 1);
    global::free(cpu, task.handle);
    module::release(cpu, task.module);
    crate::win32::thread::exit_current(cpu, code as u32);
}

/// A fault in a task's code: as Windows 3.1's "Application Error", the
/// task ends, the others go on. CLI and STI, which fault at level 3 with
/// IOPL 0, are the exception (`interrupt_flag`).
pub fn fault(cpu: &mut Cpu, vector: u8, error: Option<u32>, mut ctx: Context) {
    if vector == 13 && error == Some(0) && interrupt_flag(cpu, &mut ctx) {
        crate::win32::system::load_context(cpu, &ctx);
        return;
    }
    let name = super::current_task(cpu)
        .map(|t| cpu.bus.win32.win16.tasks[t].module)
        .and_then(|m| super::module_index(cpu, m))
        .map_or(String::new(), |i| {
            cpu.bus.win32.win16.modules[i].name.clone()
        });
    let what = match vector {
        0 => "a divide by zero".to_string(),
        6 => "an invalid instruction".to_string(),
        13 => "a general protection fault".to_string(),
        14 => "a page fault".to_string(),
        v => format!("exception {:02X}h", v),
    };
    let message = format!(
        "{} ended: {} at {:04X}:{:04X}{}.",
        name,
        what,
        ctx.seg[Seg::CS as usize],
        ctx.eip,
        error.map_or(String::new(), |e| format!(" (error code {:X})", e))
    );
    cpu.bus.log_string(&format!("[W16] {}", message));
    crate::win32::system::load_context(cpu, &ctx);
    exit(cpu, 0xFF);
}

/// Whether the instruction `ctx` faulted on is CLI or STI, which then
/// goes on after it. Windows' VMM keeps a virtual interrupt flag for
/// the protected-mode code of the system VM: CLI and STI change that
/// flag, and the code goes on, as Windows 3.1 programs expect around
/// their changes to the vector table or the BIOS data area. Both
/// instructions are skipped here, with no virtual flag: the hardware
/// interrupts go on to the real-mode handlers between a task's CLI and
/// STI, where Windows would hold them back.
fn interrupt_flag(cpu: &Cpu, ctx: &mut Context) -> bool {
    let Some((base, _, attr)) = super::task_code(cpu, ctx) else {
        return false;
    };
    let big = attr & crate::cpu::ATTR_DB != 0;
    let eip = if big { ctx.eip } else { ctx.eip & 0xFFFF };
    let is_cli_sti =
        super::peek(cpu, base.wrapping_add(eip), 1).is_some_and(|b| matches!(b[0], 0xFA | 0xFB));
    if is_cli_sti {
        let next = eip.wrapping_add(1);
        ctx.eip = if big { next } else { next & 0xFFFF };
    }
    is_cli_sti
}
