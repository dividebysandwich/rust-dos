//! Win16: Windows 3.1 programs (NE files) on the built-in Windows, as
//! Windows 95 runs them. Their tasks are threads of one Win32 process,
//! the Win16 process (KRNL386.EXE, as Windows 95 has it), which they
//! share: its address space, its LDT, where their segments' selectors
//! are, and the global heap. A task's 16-bit code runs at level 3 in
//! that LDT's segments; the system's 16-bit modules (KERNEL, USER, GDI
//! and the others) are NE images the emulator builds, each export a stub
//! that traps into Rust (builtin.rs), as the built-in DLLs' do. Most of
//! them go on to the 32-bit function that does the work, on the task's
//! thread: its 16-bit context is kept while the 32-bit code runs on a
//! stack of its own (thunk.rs), and window procedures and the other
//! callbacks of 16-bit code are called the other way.
//!
//! Tasks take turns as Windows 3.1's do: one runs until it waits for a
//! message or yields (the Win16 lock, `State::lock`), while the Win32
//! processes' threads go on around them.

mod builtin;
pub mod dos;
mod dpmi;
mod gdi;
mod global;
mod handles;
mod kernel;
mod ldt;
mod local;
mod message;
mod misc;
mod module;
mod resource;
pub mod task;
pub(crate) mod thunk;
mod user;

pub use builtin::function_name;

use crate::bus::Bus;
use crate::cpu::{ATTR_DB, Cpu, Seg};
use crate::dpmi::Context;
use crate::win32::Process;
use crate::win32::heap::Heap;
use crate::win32::memory::Memory;
use std::collections::BTreeMap;

pub use global::Arena;
pub use ldt::Ldt;

/// The Win16 tasks' state: the system's.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// The Win16 process's id, 0 while there is none.
    pub pid: u32,
    /// The thread id of the task that holds the Win16 lock, or 0: while
    /// it can run, no other task does.
    pub lock: u32,
    /// The LDT's entries given out, and the global heap's blocks by
    /// their first selector's index.
    pub ldt: Ldt,
    pub arena: BTreeMap<u16, Arena>,
    /// Where the global heap's memory comes from.
    pub heap: Heap,
    /// The modules loaded, the system's first.
    pub modules: Vec<Module16>,
    pub tasks: Vec<Task16>,
    /// The selectors of the first megabyte's places that KERNEL exports
    /// (__0000H and the others), by segment.
    pub real_selectors: Vec<(u16, u16)>,
    /// The procedure thunks (thunk.rs): the 32-bit slots that call 16-bit
    /// procedures, and the 16-bit slots that call 32-bit ones.
    pub thunks32: Vec<Thunk>,
    pub thunks16: Vec<Thunk>,
    /// Where the 32-bit slots are, and the 16-bit slots' selector and
    /// address.
    pub thunk32_base: u32,
    pub thunk16_sel: u16,
    pub thunk16_base: u32,
    /// MakeProcInstance's thunks: their segment's selector and address,
    /// and each one's function and data segment.
    pub inst_sel: u16,
    pub inst_base: u32,
    pub inst_thunks: Vec<(u32, u16)>,
    /// The local heaps (local.rs).
    pub local_heaps: Vec<local::LocalHeap>,
    /// The atom table's names and how many uses each has (kernel.rs).
    pub atoms: Vec<(String, u16)>,
    /// The transfer buffer's segment, in conventional memory (dos.rs).
    pub tb: u16,
    /// WinG's bitmaps (misc.rs): each one's handle and the 16:16 pointer
    /// to its bits.
    pub wing_bitmaps: Vec<(u32, u32)>,
    /// The wave headers given to waveOut (misc.rs): each one's linear
    /// address, its own 16:16 address, and its data's 16:16 and linear
    /// addresses.
    pub wave_headers: Vec<[u32; 4]>,
    /// The blocks of linear memory DPMI function 0501h gave, by their
    /// addresses (their handles).
    pub dpmi_blocks: Vec<u32>,
    /// The imports that no module has (`builtin::missing_address`), by
    /// the place of the stub that stands for each.
    pub missing: Vec<String>,
    /// The menu names (16:16 pointers) classes were registered with, by
    /// the class's name in capitals or "#atom", for GetClassInfo.
    pub class_menus: Vec<(String, u32)>,
    /// The multimedia files' buffers (misc.rs): each file's handle, its
    /// buffer's 16:16 and flat addresses, and the handle of the global
    /// block it is, or 0 if it is the program's.
    pub mmio_buffers: Vec<[u32; 4]>,
}

/// A loaded NE module: a program or a DLL, or one of the system's.
#[derive(Clone, Debug, Default)]
pub struct Module16 {
    /// Its module name ("KERNEL", "SKIFREE"), in capitals.
    pub name: String,
    /// Its file's DOS path.
    pub path: String,
    /// Its handle: the selector of its module database, the copy of its
    /// header and tables in memory.
    pub handle: u16,
    /// Its file (for the resources, which are read from it), empty for
    /// the system's modules, which are rebuilt.
    pub file: Vec<u8>,
    /// Its segments: each one's handle, selector and size.
    pub segments: Vec<Segment16>,
    /// Its automatic data segment's handle (a DLL's instance), or 0.
    pub dgroup: u16,
    /// How many tasks and modules hold it.
    pub refs: u16,
    /// For a system module, its place in `builtin::MODULES`.
    pub builtin: Option<u16>,
    /// The base of the image of its resources the 32-bit functions read
    /// (resource.rs), or 0 before one is made.
    pub resources32: u32,
    /// The module handles of the modules it imports from, which it holds.
    pub deps: Vec<u16>,
    /// The header's flags, and the version of Windows it expects.
    pub flags: u16,
    pub expected_version: u16,
    /// The resources LoadResource loaded: (resource handle, block).
    pub loaded: Vec<(u16, u16)>,
}

#[derive(Clone, Debug, Default)]
pub struct Segment16 {
    pub handle: u16,
    pub selector: u16,
    pub size: u32,
    pub flags: u16,
}

/// A task: a program's instance, which a thread of the Win16 process
/// runs.
#[derive(Clone, Debug, Default)]
pub struct Task16 {
    /// Its thread's id.
    pub tid: u32,
    /// Its handle: its task database's selector.
    pub handle: u16,
    /// Its module, its instance (the automatic data segment's handle)
    /// and its PDB (PSP) selector.
    pub module: u16,
    pub instance: u16,
    pub psp: u16,
    /// Its 16-bit stack's top, as it started (SS:SP).
    pub stack16: (u16, u16),
    /// The top of the 32-bit stack its calls of 32-bit code run on.
    pub stack32: u32,
    /// The switches between its 16-bit and 32-bit code it is in, the
    /// innermost last (thunk.rs).
    pub frames: Vec<Frame>,
    /// Its command line's arguments, and how its window is to be shown.
    pub arguments: String,
    pub show: u16,
    /// The task that started it, or 0.
    pub parent: u16,
    /// The process object its starters wait on, which is signaled when it
    /// ends, and its id, which they get.
    pub process_object: u32,
    pub process_id: u32,
    /// Its exit code, once it ended.
    pub exit_code: Option<u16>,
    /// Its DTA, as a segment:offset pointer, for INT 21h's searches.
    pub dta: u32,
    /// What SetErrorMode last set.
    pub error_mode: u16,
    /// Whether it gave up its turn (Yield): the other tasks go first.
    pub yielding: bool,
    /// Its protected-mode interrupt vectors, as INT 21h AH=25h and INT 31h
    /// set them: (vector, selector:offset).
    pub vectors: Vec<(u8, u32)>,
    /// The libraries it came with whose entry points InitTask is yet to
    /// call, by module handle.
    pub inits: Vec<u16>,
}

/// A switch between a task's 16-bit and 32-bit code (thunk.rs).
#[derive(Clone, Debug)]
pub enum Frame {
    /// 16-bit code called 32-bit code: its context, which goes on when
    /// that returns, and what is done with what it returns (`post`, a
    /// function of `thunk::POSTS`, with its data).
    To32 {
        ctx: Context,
        post: u16,
        data: Vec<u32>,
    },
    /// 32-bit code called 16-bit code: its context, which goes on when
    /// that returns, and what is done with what it returns.
    To16 {
        ctx: Context,
        then: u16,
        data: Vec<u32>,
    },
}

impl Default for Frame {
    fn default() -> Frame {
        Frame::To32 {
            ctx: Context::default(),
            post: 0,
            data: Vec::new(),
        }
    }
}

/// A procedure thunk: the procedure it calls (a 16:16 pointer or a 32-bit
/// address), what kind of procedure that is (`thunk::KIND_` values),
/// and for a window procedure's the task's instance.
#[derive(Clone, Debug, Default)]
pub struct Thunk {
    pub target: u32,
    pub kind: u8,
    pub extra: u32,
}

/// The Win16 process, if there is one.
pub fn win16_ref(cpu: &Cpu) -> Option<&Process> {
    let pid = cpu.bus.win32.win16.pid;
    if pid == 0 {
        return None;
    }
    cpu.bus.win32.processes.iter().find(|p| p.id == pid)
}

/// The Win16 process: there is one.
pub fn win16(cpu: &mut Cpu) -> &mut Process {
    let pid = cpu.bus.win32.win16.pid;
    cpu.bus
        .win32
        .processes
        .iter_mut()
        .find(|p| p.id == pid)
        .expect("the Win16 process")
}

/// Run `f` with the process `pid` and the bus, for work on its memory.
/// None if there is no such process.
pub fn with_win16<T>(
    cpu: &mut Cpu,
    pid: u32,
    f: impl FnOnce(&mut Process, &mut Bus) -> T,
) -> Option<T> {
    let index = cpu.bus.win32.processes.iter().position(|p| p.id == pid)?;
    let mut process = std::mem::take(&mut cpu.bus.win32.processes[index]);
    let result = f(&mut process, &mut cpu.bus);
    cpu.bus.win32.processes[index] = process;
    Some(result)
}

/// Run `f` with the global heap's allocator and the Win16 process's
/// memory.
pub fn with_win16_heap<T: Default>(
    cpu: &mut Cpu,
    f: impl FnOnce(&mut Heap, &mut Memory, &mut Bus) -> T,
) -> T {
    let pid = cpu.bus.win32.win16.pid;
    let mut heap = std::mem::take(&mut cpu.bus.win32.win16.heap);
    let result = with_win16(cpu, pid, |p, bus| {
        let mut memory = std::mem::take(&mut p.memory);
        let result = f(&mut heap, &mut memory, bus);
        p.memory = memory;
        result
    });
    cpu.bus.win32.win16.heap = heap;
    result.unwrap_or_default()
}

/// Whether the running process is the Win16 process.
pub fn in_win16(cpu: &Cpu) -> bool {
    let win16 = &cpu.bus.win32.win16;
    win16.pid != 0
        && cpu
            .bus
            .win32
            .processes
            .get(cpu.bus.win32.running)
            .is_some_and(|p| p.id == win16.pid)
}

/// The running thread's id, if one runs.
fn current_tid(cpu: &Cpu) -> Option<u32> {
    crate::win32::running(cpu)?.current_thread().map(|t| t.id)
}

/// The task the running thread is, if it is one.
pub fn current_task(cpu: &Cpu) -> Option<usize> {
    if !in_win16(cpu) {
        return None;
    }
    let tid = current_tid(cpu)?;
    cpu.bus.win32.win16.tasks.iter().position(|t| t.tid == tid)
}

/// The running task: there is one.
pub fn task(cpu: &mut Cpu) -> &mut Task16 {
    let i = current_task(cpu).expect("a Win16 task");
    &mut cpu.bus.win32.win16.tasks[i]
}

/// Whether the thread `tid` is a Win16 task's.
pub fn is_task(win32: &crate::win32::Win32, tid: u32) -> bool {
    win32.win16.pid != 0 && win32.win16.tasks.iter().any(|t| t.tid == tid)
}

/// Whether the context `ctx` the IDT's gate was entered from is a Win16
/// task's: its faults end the task (task.rs).
pub fn is_task_context(cpu: &Cpu, _ctx: &Context) -> bool {
    current_task(cpu).is_some()
}

/// Whether the processor is in 16-bit code of the Win16 process: a trap
/// there is a system module's stub (builtin.rs).
pub fn in_16bit_code(cpu: &Cpu) -> bool {
    in_win16(cpu) && is_16bit(cpu, &Context::of(cpu))
}

/// Run the system module's function `id`, whose stub 16-bit code called.
pub fn call(cpu: &mut Cpu, id: u16) {
    builtin::call(cpu, id);
}

/// Whether `ctx` is in 16-bit code of the Win16 process.
fn is_16bit(cpu: &Cpu, ctx: &Context) -> bool {
    let cs = ctx.seg[Seg::CS as usize];
    cs & 4 != 0
        && cs != crate::win32::system::CODE3
        && selector_base(cpu, cs).is_some_and(|(_, _, attr)| attr & ATTR_DB == 0)
}

/// Whether the IDT's gate for `vector` was entered by an INT n of a
/// Win16 task's 16-bit code: the instruction before where it goes on is
/// `CD vector`.
pub fn is_soft_int(cpu: &mut Cpu, vector: u8, ctx: &Context) -> bool {
    if current_task(cpu).is_none() || !is_16bit(cpu, ctx) {
        return false;
    }
    let cs = ctx.seg[Seg::CS as usize];
    let Some((base, _, _)) = selector_base(cpu, cs) else {
        return false;
    };
    let at = base.wrapping_add(ctx.eip.wrapping_sub(2) & 0xFFFF);
    peek(cpu, at, 2).is_some_and(|b| b == [0xCD, vector])
}

/// The base, limit and attributes (as `SegCache::attr` has them) of the
/// selector `sel` of the Win16 process's LDT, if it is present.
pub fn selector_base(cpu: &Cpu, sel: u16) -> Option<(u32, u32, u16)> {
    if sel & 4 == 0 {
        return None;
    }
    let system = win16_ref(cpu)?.system;
    let desc = ldt::read(cpu, system, sel);
    let desc = crate::cpu::Descriptor(desc);
    (desc.attr() & 0x80 != 0).then(|| (desc.base(), desc.limit(), desc.attr()))
}

/// The linear address of the 16:16 pointer `segptr`: 0 for a null one,
/// and for one whose selector isn't there.
pub fn linear(cpu: &Cpu, segptr: u32) -> u32 {
    if segptr == 0 {
        return 0;
    }
    match selector_base(cpu, (segptr >> 16) as u16) {
        Some((base, _, _)) => base.wrapping_add(segptr & 0xFFFF),
        None => 0,
    }
}

/// The linear address of `sel:off`.
pub fn lin(cpu: &Cpu, sel: u16, off: u16) -> u32 {
    linear(cpu, (sel as u32) << 16 | off as u32)
}

/// `len` bytes at the linear address `at` of the Win16 process, read
/// without a fault: None where a page isn't there.
pub fn peek(cpu: &Cpu, at: u32, len: usize) -> Option<Vec<u8>> {
    let process = win16_ref(cpu)?;
    crate::win32::loader::peek(&cpu.bus, &process.memory, at, len)
}

/// Up to `len` bytes at the linear address `at`: as many as there are
/// before a page that isn't there.
pub fn peek_upto(cpu: &Cpu, at: u32, len: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut from = at;
    let end = at as u64 + len as u64;
    while (from as u64) < end {
        let next = ((from as u64 | 0xFFF) + 1).min(end);
        match peek(cpu, from, (next - from as u64) as usize) {
            Some(bytes) => out.extend(bytes),
            None => break,
        }
        from = next as u32;
    }
    out
}

/// Write `data` at the linear address `at` of the Win16 process. False
/// where a page isn't there.
pub fn poke(cpu: &mut Cpu, at: u32, data: &[u8]) -> bool {
    let pid = cpu.bus.win32.win16.pid;
    with_win16(cpu, pid, |p, bus| p.memory.write(bus, at, data))
        .flatten()
        .is_some()
}

pub fn read_16(cpu: &Cpu, at: u32) -> u16 {
    peek(cpu, at, 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]))
}

pub fn read_32(cpu: &Cpu, at: u32) -> u32 {
    peek(cpu, at, 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

pub fn write_16(cpu: &mut Cpu, at: u32, value: u16) {
    poke(cpu, at, &value.to_le_bytes());
}

pub fn write_32(cpu: &mut Cpu, at: u32, value: u32) {
    poke(cpu, at, &value.to_le_bytes());
}

/// The NUL-terminated string at the linear address `at`, at most `max`
/// bytes of it.
pub fn string_bytes(cpu: &Cpu, at: u32, max: usize) -> Vec<u8> {
    let Some(process) = win16_ref(cpu) else {
        return Vec::new();
    };
    if at == 0 {
        return Vec::new();
    }
    crate::win32::loader::peek_string(&cpu.bus, &process.memory, at, max).unwrap_or_default()
}

/// The string at the linear address `at` as Rust text, a char a byte.
pub fn string(cpu: &Cpu, at: u32) -> String {
    crate::dosstr::from_bytes(&string_bytes(cpu, at, 0x1_0000))
}

/// The module whose handle is `handle`, or whose instance or one of
/// whose segments' selectors or handles it is, as KERNEL's functions take
/// any of them for a module.
pub fn module_index(cpu: &Cpu, handle: u16) -> Option<usize> {
    let win16 = &cpu.bus.win32.win16;
    if handle == 0 {
        return None;
    }
    let sel = handle | 1;
    win16
        .modules
        .iter()
        .position(|m| m.handle | 1 == sel)
        .or_else(|| {
            win16.modules.iter().position(|m| {
                m.dgroup | 1 == sel
                    || m.segments
                        .iter()
                        .any(|s| s.selector == sel || s.handle | 1 == sel)
            })
        })
        .or_else(|| {
            // A task's instance.
            let task = win16.tasks.iter().find(|t| t.instance | 1 == sel)?;
            win16.modules.iter().position(|m| m.handle == task.module)
        })
}

crate::state_fields!(State {
    pid,
    lock,
    ldt,
    arena,
    heap,
    modules,
    tasks,
    real_selectors,
    thunks32,
    thunks16,
    thunk32_base,
    thunk16_sel,
    thunk16_base,
    inst_sel,
    inst_base,
    inst_thunks,
    local_heaps,
    atoms,
    tb,
    wing_bitmaps,
    wave_headers,
    dpmi_blocks,
    missing,
    class_menus,
    mmio_buffers
});
crate::state_fields!(Module16 {
    name,
    path,
    handle,
    file,
    segments,
    dgroup,
    refs,
    builtin,
    resources32,
    deps,
    flags,
    expected_version,
    loaded
});
crate::state_fields!(Segment16 {
    handle,
    selector,
    size,
    flags
});
crate::state_fields!(Task16 {
    tid,
    handle,
    module,
    instance,
    psp,
    stack16,
    stack32,
    frames,
    arguments,
    show,
    parent,
    process_object,
    process_id,
    exit_code,
    dta,
    error_mode,
    yielding,
    vectors,
    inits
});
crate::win32::state_variants!(Frame {
    To32 { ctx, post, data },
    To16 { ctx, then, data },
});
crate::state_fields!(Thunk {
    target,
    kind,
    extra
});
