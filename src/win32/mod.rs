//! The built-in Windows: Win32 programs (PE files) run without a copy of
//! Windows, on the built-in DOS, as they did on Windows 95's. See
//! docs/native-windows.md for the plan this is the start of.
//!
//! When DOS loads a program that is a PE file and `win32` is on, the DOS
//! stub doesn't run: the program starts at a trap in the ROM (`START`),
//! which makes its process (loader.rs). That takes a block of extended
//! memory for the process's pages, tables and page tables, maps the
//! built-in DLLs and the program where Windows 95 has them, and goes on in
//! protected mode at level 3 at the program's entry point (system.rs).
//! The DLLs' functions are stubs of a service trap (`FE 3E`, then the
//! function's 16-bit id) that land in Rust (stubs.rs, kernel32.rs).
//! ExitProcess goes back to real mode and ends the DOS process with
//! INT 21h AH=4Ch, so the shell or the program that started it goes on.

mod advapi32;
mod call;
mod com;
mod convert;
mod ddraw;
mod dinput;
mod dsound;
pub mod gdi32;
mod glide;
mod heap;
mod kernel32;
mod loader;
mod mapping;
mod memory;
mod msvcrt;
mod object;
mod ole32;
mod registry;
mod seh;
pub mod snapshot;
mod stubs;
mod system;
mod task;
mod thread;
pub mod user32;
mod winmm;

pub use snapshot::snapshot;
pub use stubs::function_name;

/// `State` for an enum whose variants have named fields (or `{}`), as the
/// variant's place in the list and then its fields.
macro_rules! state_variants {
    ($t:ident { $($v:ident { $($f:ident),* $(,)? }),* $(,)? }) => {
        impl crate::savestate::State for $t {
            #[allow(unused_assignments)]
            fn save(&self, w: &mut crate::savestate::Writer) {
                let mut index = 0u8;
                $(
                    if let $t::$v { $($f),* } = self {
                        crate::savestate::State::save(&index, w);
                        $( crate::savestate::State::save($f, w); )*
                        return;
                    }
                    index += 1;
                )*
            }
            #[allow(unused_assignments)]
            fn load(&mut self, r: &mut crate::savestate::Reader) -> crate::savestate::Result<()> {
                let mut tag = 0u8;
                crate::savestate::State::load(&mut tag, r)?;
                let mut index = 0u8;
                $(
                    if tag == index {
                        $(
                            let mut $f = Default::default();
                            crate::savestate::State::load(&mut $f, r)?;
                        )*
                        *self = $t::$v { $($f),* };
                        return Ok(());
                    }
                    index += 1;
                )*
                Err(crate::savestate::StateError::Invalid(format!(
                    "a {} it doesn't know",
                    stringify!($t)
                )))
            }
        }
    };
}
pub(crate) use state_variants;

use crate::cpu::{Cpu, CpuState, Seg};
// A processor context, as the DPMI host keeps its clients'.
use crate::dpmi::Context;
use memory::Memory;

/// The service trap's second byte: `FE 3E lo hi`.
pub const TRAP: u8 = 0x3E;

/// The ROM's part (offsets in segment F000): the start of a process, the
/// return of the real-mode handler of a hardware interrupt, the end of a
/// process (INT 21h AH=4Ch), and the IDT's 256 traps.
pub(crate) const ROM_SEG: u16 = 0xF000;
const ROM_BASE: usize = 0xF0000;
const START: u16 = 0x3800;
const IRQ_RETURN: u16 = 0x3804;
const EXIT: u16 = 0x3808;
const IDT_TRAPS: u16 = 0x3C00;

/// The traps' ids: the built-in DLLs' functions below `FIRST_MISSING`
/// (stubs.rs), then the stubs of functions they don't have yet, then the
/// system's, in the ROM.
const FIRST_MISSING: u16 = 0xE000;
const MAX_MISSING: u32 = 0x1000;
const FIRST_SYSTEM: u16 = 0xF000;
const ID_IDT: u16 = 0xF100;
const ID_IRQ_RETURN: u16 = 0xF200;
const ID_START: u16 = 0xF300;

/// The index of ESP in `Context::gpr`.
const ESP: usize = 4;

/// The exit code of a program that ended because of an error.
const FAILED: u32 = 0xFF;

/// A module in the process: the program or a DLL.
#[derive(Clone, Debug, Default)]
pub struct Module {
    pub name: String,
    pub base: u32,
    pub size: u32,
    /// Its entry point (DllMain), or 0 for the program's and the
    /// built-in DLLs', which have none to call.
    pub entry: u32,
    /// Its file's DOS path, as GetModuleFileNameA gives it.
    pub path: String,
    /// How many LoadLibrary calls hold it (not counted for the modules
    /// that stay while the program runs).
    pub refs: u32,
    /// What it is (loader.rs: `BUILTIN`, `STATIC` and the others).
    pub flags: u32,
    /// The modules it imports from, each holding a reference of its own.
    pub deps: Vec<u32>,
}

/// A running Win32 program.
#[derive(Clone, Debug, Default)]
pub struct Process {
    /// Its id, and its object, which it holds while it runs.
    id: u32,
    object: u32,
    /// Its file's name, and its path as DOS has it.
    name: String,
    path: String,
    memory: Memory,
    /// The system block's physical address (system.rs).
    system: u32,
    modules: Vec<Module>,
    image_base: u32,
    /// The physical pages of the built-in DLLs' stubs: the traps there are
    /// theirs.
    stub_pages: Vec<u32>,
    /// The functions of the stubs that end the program, and where those
    /// are.
    missing: Vec<(String, String)>,
    missing_base: u32,
    /// Its handles, and those of standard input, output and error.
    handles: object::Handles,
    std: [u32; 3],
    /// Its threads, and which of them runs, if the process does.
    threads: Vec<thread::Thread>,
    current: Option<usize>,
    /// The top of the idle loop's stack.
    idle_stack: u32,
    /// The running thread's last error and TIB, kept in its entry while
    /// another runs.
    last_error: u32,
    tib: u32,
    command_line: u32,
    /// The program's TLS template, which each thread gets a copy of.
    tls_template: Vec<u8>,
    /// The filter SetUnhandledExceptionFilter set, or 0.
    exception_filter: u32,
    /// The stack the program's header asks for, which threads get.
    stack_reserve: u32,
    /// TLS slots in use, a bit each.
    tls_used: u128,
    /// The environment, as "NAME=value" strings: DOS's when it started.
    environment: Vec<String>,
    /// The process heap (GetProcessHeap).
    heap: heap::Heap,
    /// The heaps HeapCreate made, and the handles of GlobalAlloc's and
    /// LocalAlloc's moveable blocks.
    heaps: Vec<heap::Heap>,
    moveables: Vec<heap::Moveable>,
    /// The STARTUPINFO its parent gave, which GetStartupInfoA returns:
    /// empty for the process DOS started.
    startup: Vec<u8>,
    /// Its priority class, which only GetPriorityClass reads, and its
    /// error mode, which only SetErrorMode does.
    priority_class: u32,
    error_mode: u32,
    /// The version of Windows its header says it was made for, as
    /// major << 16 | minor.
    subsystem_version: u32,

    /// The static TLS of the DLLs that came with the program: each one's
    /// slot and template, which each thread gets a copy of.
    dll_tls: Vec<(u32, Vec<u8>)>,
    /// Whether ExitProcess is ending it, telling the DLLs.
    exiting: bool,
}

impl Process {
    pub fn current_thread(&self) -> Option<&thread::Thread> {
        self.threads.get(self.current?)
    }

    pub fn current_thread_mut(&mut self) -> Option<&mut thread::Thread> {
        self.threads.get_mut(self.current?)
    }

    /// The DLLs whose entry points learn of threads that start and end,
    /// as (base, entry point), in the order they were loaded in.
    fn entry_points(&self) -> Vec<(u32, u32)> {
        let flags = loader::ATTACHED | loader::NO_THREAD_CALLS;
        self.modules
            .iter()
            .filter(|m| m.entry != 0 && m.flags & flags == loader::ATTACHED)
            .map(|m| (m.base, m.entry))
            .collect()
    }

    /// The base of the module `name`, ".DLL" added to a name without an
    /// extension, known by its file name as on Windows 95. Modules mapped
    /// as data aren't found.
    fn module(&self, name: &str) -> Option<u32> {
        let file = loader::module_file_name(name);
        self.modules
            .iter()
            .find(|m| m.name.eq_ignore_ascii_case(&file) && m.flags & loader::DATAFILE == 0)
            .map(|m| m.base)
    }
}

/// The built-in Windows's state.
#[derive(Debug, Default)]
pub struct Win32 {
    /// Whether PE programs run (the `win32` setting); off, their DOS stub
    /// runs as on DOS.
    pub enabled: bool,
    /// The program DOS loaded, which starts at `START`.
    pending: Option<String>,
    /// The processes, the one DOS started first. They run in that DOS
    /// process, which waits for the last of them to end.
    processes: Vec<Process>,
    /// The process whose thread runs, or whose idle loop.
    running: usize,
    /// Where the processes' pages come from.
    pool: Option<memory::Pool>,
    /// The kernel objects, which the processes' handles are of.
    objects: object::Objects,
    /// The DOS process the programs run in, and the stack its DOS stub
    /// had, where real-mode handlers of hardware interrupts run.
    psp: u16,
    rm_stack: (u16, u16),
    /// The A20 gate and CR4 before the first program started.
    a20: bool,
    cr4: u32,
    /// The program's context while the real-mode handler of a hardware
    /// interrupt runs.
    interrupted: Option<Context>,
    /// The exit code of the program DOS started, which DOS gets when the
    /// last program ends.
    exit_code: u32,
    /// When the running thread's turn started, by the clock's
    /// microseconds.
    slice_start: u64,
    /// How many process and thread ids have been given out.
    ids: u32,
    /// The id of the process DOS started, whose exit code DOS gets.
    root: u32,
    /// The registry, which ADVAPI32's functions work on.
    registry: registry::Registry,

    /// The file mappings' memory, by the index their objects keep.
    sections: Vec<Option<mapping::Section>>,

    /// The current directories SetCurrentDirectoryA was given with long
    /// names, as (the directory in short names, as DOS has it; as given).
    long_dirs: Vec<(String, String)>,
    /// The console's input and output modes (SetConsoleMode), or None
    /// for those it starts with.
    console_mode: Option<(u32, u32)>,

    /// USER's and GDI's state: windows, messages, and what GDI draws
    /// with. They are the system's, shared by the processes.
    user: user32::State,
    gdi: gdi32::State,

    /// COM's and the DirectX and multimedia DLLs' state, the system's,
    /// and how many GUIDs CoCreateGuid made.
    ole: ole32::State,
    ddraw: ddraw::State,
    dsound: dsound::State,
    dinput: dinput::State,
    winmm: winmm::State,
    glide: glide::State,
    guids: u32,
}

impl Win32 {
    /// No process: as after a program ends. The setting stays.
    pub fn reset(&mut self) {
        *self = Win32 {
            enabled: self.enabled,
            ..Win32::default()
        };
    }

    /// Whether a Win32 program is running.
    pub fn active(&self) -> bool {
        !self.processes.is_empty()
    }

    /// Whether DOS loaded a Win32 program that hasn't started yet.
    pub fn pending(&self) -> bool {
        self.pending.is_some()
    }

    /// The extended memory the processes hold, as (address, bytes): for
    /// the XMS pool after a state is loaded.
    pub fn reservations(&self) -> Vec<(u32, u32)> {
        self.pool.iter().map(|p| (p.base, p.len)).collect()
    }

    /// The running program's modules (name, base, size), for debuggers.
    pub fn modules(&self) -> Vec<(String, u32, u32)> {
        self.processes
            .get(self.running)
            .iter()
            .flat_map(|p| p.modules.iter().map(|m| (m.name.clone(), m.base, m.size)))
            .collect()
    }

    /// When WinMM next has work to do (its timers, MIDI streams and MCI
    /// devices), in PIT ticks, for the bus's timer events.
    pub fn next_event(&self) -> Option<u64> {
        let micros = self.winmm.due?;
        Some(crate::timer::mul_div_ceil(micros, crate::timer::PIT_HZ, 1_000_000))
    }

    /// The scale of what the MIDI synthesizer plays (left, right): the
    /// MIDI device's volume and the mixer's.
    pub fn midi_gain(&self) -> (f32, f32) {
        winmm::midi::gain(&self.winmm)
    }

    /// The scale of what the CD drive plays: the mixer's CD line.
    pub fn cd_gain(&self) -> (f32, f32) {
        self.winmm.mixer.cd_gain()
    }

    /// Whether that time, or the time the sound device next needs its
    /// tick (`audio_next_event`), has come at PIT tick `now` (`timer`).
    #[inline]
    pub fn due(&self, now: u64) -> bool {
        [self.next_event(), self.dsound.due, self.winmm.wave.due]
            .into_iter()
            .flatten()
            .any(|t| t <= now)
    }

    /// A new process or thread id: as on Windows 95, a large number,
    /// which programs only compare.
    fn new_id(&mut self) -> u32 {
        self.ids += 1;
        0xFFF0_0000 + self.ids * 0x24
    }
}

/// WinMM's or the sound device's time for work came (`Win32::due`),
/// between two instructions: the work is done, and if the program's code
/// was running, a thread of a higher priority that it readied (the
/// multimedia thread, whose callbacks ran at interrupt time on Windows
/// 95) runs at once.
pub fn timer(cpu: &mut Cpu) {
    cpu.bus.win32.winmm.due = None;
    if !cpu.bus.win32.active() {
        cpu.bus.win32.dsound.due = None;
        cpu.bus.win32.winmm.wave.due = None;
        cpu.bus.reschedule_timers();
        return;
    }
    let in_program = cpu.pe()
        && !cpu.v86()
        && cpu.cpl == 3
        && cpu.bus.win32.interrupted.is_none()
        && running(cpu).is_some_and(|p| p.current.is_some());
    if in_program {
        thread::preempt(cpu);
    } else {
        winmm::tick(cpu);
        dsound::tick(cpu);
    }
    winmm::schedule(cpu);
}

/// The running process.
fn process(cpu: &mut Cpu) -> &mut Process {
    let win32 = &mut cpu.bus.win32;
    win32
        .processes
        .get_mut(win32.running)
        .expect("a Win32 process")
}

/// The running process, if there is one, for what only reads it.
fn running(cpu: &Cpu) -> Option<&Process> {
    cpu.bus.win32.processes.get(cpu.bus.win32.running)
}

/// Run `f` with the running process and the bus, for work on the
/// process's memory.
fn with_process<T>(cpu: &mut Cpu, f: impl FnOnce(&mut Process, &mut crate::bus::Bus) -> T) -> T {
    let index = cpu.bus.win32.running;
    let mut process = std::mem::take(&mut cpu.bus.win32.processes[index]);
    let result = f(&mut process, &mut cpu.bus);
    cpu.bus.win32.processes[index] = process;
    result
}

/// Write the ROM's part: the traps, and INT 21h for the end of a process.
pub fn install_rom(bus: &mut crate::bus::Bus) {
    let trap = |id: u16| {
        let [lo, hi] = id.to_le_bytes();
        [0xFE, TRAP, lo, hi]
    };
    let at = |offset: u16| ROM_BASE + offset as usize;
    bus.write_rom(at(START), &trap(ID_START));
    bus.write_rom(at(IRQ_RETURN), &trap(ID_IRQ_RETURN));
    // INT 21h; and should DOS come back, again.
    bus.write_rom(at(EXIT), &[0xCD, 0x21, 0xEB, 0xFC]);
    let idt: Vec<u8> = (0..=255).flat_map(|v| trap(system::idt_id(v))).collect();
    bus.write_rom(at(IDT_TRAPS), &idt);
}

/// DOS loaded the program `filename` (its file is `bytes`) as an EXE: if
/// it is a PE file and the built-in Windows is on, make it start as a
/// Win32 program. Returns whether it does.
pub fn prepare(cpu: &mut Cpu, filename: &str, bytes: &[u8]) -> bool {
    if !cpu.bus.win32.enabled || !rust_dos_pe::is_pe(bytes) {
        return false;
    }
    // Under a virtual-8086 monitor (EMM386, Windows' 386 enhanced mode)
    // the process can't take the processor (`accepts`): the stub runs.
    if cpu.pe() {
        cpu.bus.log_string(&format!(
            "[W32] {} is a Win32 program, but DOS runs in virtual-8086 mode: its DOS stub runs",
            filename
        ));
        return false;
    }
    install_rom(&mut cpu.bus);
    // Keys typed before it started aren't its to read (`take_pressed`).
    cpu.bus.kbd.take_pressed();
    let path = cpu.program_path(filename);
    cpu.bus
        .log_string(&format!("[W32] {} is a Win32 program", path));
    cpu.bus.win32.pending = Some(path);
    cpu.set_cs(ROM_SEG);
    cpu.set_ip(START);
    true
}

/// Whether the trap with id `id` at the physical address `phys_ip` is one
/// the built-in Windows runs, in the mode the processor is in; otherwise
/// it is an invalid opcode.
pub fn accepts(cpu: &Cpu, phys_ip: usize, id: u16) -> bool {
    let win32 = &cpu.bus.win32;
    if id >= FIRST_SYSTEM {
        if !crate::bus::Bus::is_rom(phys_ip) {
            return false;
        }
        return match id {
            ID_START => !cpu.pe() && win32.pending.is_some() && !win32.active(),
            ID_IRQ_RETURN => !cpu.pe() && win32.interrupted.is_some(),
            _ => id & 0xFF00 == ID_IDT && cpu.pm() && cpu.cpl == 0 && win32.active(),
        };
    }
    let Some(process) = running(cpu) else {
        return false;
    };
    cpu.pm() && cpu.cpl == 3 && process.stub_pages.contains(&((phys_ip >> 12) as u32))
}

/// Run the trap with id `id` (see `accepts`).
pub fn service(cpu: &mut Cpu, id: u16) {
    match id {
        ID_START => start(cpu),
        ID_IRQ_RETURN => system::irq_return(cpu),
        _ if id & 0xFF00 == ID_IDT => system::idt(cpu, id as u8),
        _ if id >= FIRST_MISSING => missing(cpu, id - FIRST_MISSING),
        _ => stubs::call(cpu, id),
    }
}

/// The program DOS loaded starts: make its process and go to its entry
/// point. A program that can't start ends with a message and exit code 1.
fn start(cpu: &mut Cpu) {
    let Some(path) = cpu.bus.win32.pending.take() else {
        return;
    };
    let (psp, rm_stack, a20, cr4) = (cpu.current_psp, (cpu.ss(), cpu.sp()), cpu.bus.a20(), cpu.cr4);
    let win32 = &mut cpu.bus.win32;
    win32.psp = psp;
    win32.rm_stack = rm_stack;
    win32.a20 = a20;
    win32.cr4 = cr4;
    let environment = dos_environment(cpu, cpu.current_psp);
    let line = loader::dos_command_line(cpu, &path);
    match create_process(cpu, &path, &line, environment, Vec::new(), None, Vec::new(), false) {
        Ok(created) => {
            cpu.bus.win32.root = created.process.1;
            cpu.bus.set_a20(true);
            thread::reschedule(cpu);
        }
        Err(message) => {
            cpu.bus.log_string(&format!("[W32] {}", message));
            say(cpu, &message);
            if let Some(pool) = cpu.bus.win32.pool.take() {
                pool.release(&mut cpu.bus);
            }
            cpu.set_ax(0x4C01);
            cpu.set_ip(EXIT);
        }
    }
}

/// What `create_process` made: the process's object and id, and its main
/// thread's.
pub struct Created {
    pub process: (u32, u32),
    pub thread: (u32, u32),
}

/// Make a process of the program at `path`, with the command line `line`
/// and the environment `environment`, and its main thread, ready to run
/// or suspended. It inherits the handles `inherited`, as (handle,
/// object). Its standard input, output and error are DOS's handles 0, 1
/// and 2, or the objects `std` names, by the handles it inherited of them
/// where it did. `startup` is what GetStartupInfoA gives it. Fails with a
/// message for the user.
#[allow(clippy::too_many_arguments)]
fn create_process(
    cpu: &mut Cpu,
    path: &str,
    line: &str,
    environment: Vec<String>,
    inherited: Vec<(u32, u32)>,
    std: Option<[u32; 3]>,
    startup: Vec<u8>,
    suspended: bool,
) -> Result<Created, String> {
    let file = cpu
        .read_program_file(path)
        .ok_or_else(|| format!("{} can't be read.", path))?;
    if cpu.bus.win32.pool.is_none() {
        cpu.bus.win32.pool = memory::Pool::take(&mut cpu.bus, loader::MIN_MEMORY);
    }
    let loaded = loader::load(cpu, path, line, &file, environment)?;
    let mut process = loaded.process;
    process.priority_class = 0x20;
    process.startup = startup;
    let return_address = stubs::stub_address(0, kernel32::PROCESS_RETURN);
    let esp = loaded.stack_top - 4;
    process.memory.write_32(&mut cpu.bus, esp, return_address);
    cpu.bus.log_string(&format!(
        "[W32] Started {} at {:08X}, image at {:08X}, {} KB of memory left",
        process.name,
        loaded.entry,
        process.image_base,
        cpu.bus.win32.pool.as_ref().map_or(0, |p| p.available()) / 1024
    ));
    let win32 = &mut cpu.bus.win32;
    process.id = win32.new_id();
    let tid = win32.new_id();
    let objects = &mut win32.objects;
    process.object = objects.add(
        object::Kind::Process {
            id: process.id,
            exit_code: None,
        },
        None,
    );
    for (handle, id) in inherited {
        if process.handles.add_at(handle, id) {
            objects.hold(id);
        }
    }
    process.std = match std {
        Some(ids) => ids.map(|id| {
            let found = process.handles.iter().find(|&(_, o)| o == id);
            if let Some((handle, _)) = found {
                return handle;
            }
            objects.hold(id);
            process.handles.add(id)
        }),
        None => [0, 1, 2].map(|handle| {
            let id = objects.add(object::Kind::Dos { handle }, None);
            process.handles.add(id)
        }),
    };
    // The built-in DLLs' variables, which the standard handles go in.
    if stubs::init_process(&mut cpu.bus, &mut process).is_none() {
        let objects = &mut cpu.bus.win32.objects;
        for (_, id) in process.handles.iter() {
            objects.release(id);
        }
        objects.release(process.object);
        process.memory.release(&mut cpu.bus);
        return Err(format!(
            "There isn't enough extended memory to run {}.",
            process.name
        ));
    }
    let win32 = &mut cpu.bus.win32;
    let objects = &mut win32.objects;
    // The main thread, which runs the program's entry point, after the
    // entry points of the DLLs it came with when there are any: from
    // KERNEL32's process start, with the entry point in EAX.
    let attach = process.modules.iter().any(|m| m.entry != 0);
    let mut ctx = Context {
        eip: if attach {
            kernel32::process_start_address()
        } else {
            loaded.entry
        },
        eflags: 0x0202,
        ..Context::default()
    };
    ctx.gpr[0] = loaded.entry;
    ctx.gpr[ESP] = esp;
    ctx.seg = [
        system::DATA3,
        system::CODE3,
        system::DATA3,
        system::DATA3,
        system::TIB3,
        0,
    ];
    let thread_object = objects.add(
        object::Kind::Thread {
            id: tid,
            exit_code: None,
        },
        None,
    );
    process.threads.push(thread::Thread {
        id: tid,
        object: thread_object,
        tib: process.tib,
        selector: system::TIB3,
        stack: loaded.stack,
        ctx,
        suspended: suspended as u32,
        ..thread::Thread::default()
    });
    let created = Created {
        process: (process.object, process.id),
        thread: (thread_object, tid),
    };
    win32.processes.push(process);
    Ok(created)
}

/// The environment of the DOS process `psp`, as "NAME=value" strings.
fn dos_environment(cpu: &mut Cpu, psp: u16) -> Vec<String> {
    let segment = cpu.bus.read_16(psp as usize * 16 + 0x2C) as usize;
    let mut strings = Vec::new();
    if segment == 0 {
        return strings;
    }
    let mut at = segment * 16;
    // An environment is at most 32 KB.
    let end = at + 0x8000;
    while at < end {
        let bytes: Vec<u8> = (at..end)
            .map(|a| cpu.bus.read_8(a))
            .take_while(|&b| b != 0)
            .collect();
        if bytes.is_empty() {
            break;
        }
        at += bytes.len() + 1;
        strings.push(crate::dosstr::from_bytes(&bytes));
    }
    strings
}

/// End the running process with exit code `code`: its threads end, its
/// handles close and its memory goes back to the pool. Another process's
/// thread runs on; when there is none, back in real mode, the DOS process
/// ends with the exit code of the program it started.
fn exit(cpu: &mut Cpu, code: u32) {
    let index = cpu.bus.win32.running;
    end_process(cpu, index, code);
}

/// End the process at `index` with exit code `code`, as `exit`. Whether
/// the running thread goes on: false when it was one of the process's.
fn terminate(cpu: &mut Cpu, index: usize, code: u32) -> bool {
    let running = cpu.bus.win32.running == index;
    end_process(cpu, index, code);
    !running
}

fn end_process(cpu: &mut Cpu, index: usize, code: u32) {
    let win32 = &mut cpu.bus.win32;
    let Some(process) = win32.processes.get(index) else {
        return;
    };
    let (name, threads) = (process.name.clone(), process.threads.len());
    let tids: Vec<u32> = process.threads.iter().map(|t| t.id).collect();
    for i in 0..threads {
        thread::end(win32, index, i, code);
    }
    let process = &mut win32.processes[index];
    let handles: Vec<u32> = process.handles.iter().map(|(_, id)| id).collect();
    let own = process.object;
    for id in handles {
        win32.objects.release(id);
    }
    if let Some(object) = win32.objects.get_mut(own)
        && let object::Kind::Process { exit_code, .. } = &mut object.kind
    {
        *exit_code = Some(code);
    }
    win32.objects.release(own);
    if process.id == win32.root {
        win32.exit_code = code;
    }
    cpu.bus.log_string(&format!(
        "[W32] {} ended with exit code {:X}",
        name, code
    ));
    let process = cpu.bus.win32.processes.remove(index);
    process.memory.release(&mut cpu.bus);
    registry::process_ended(cpu, process.id);
    user32::process_ended(cpu, process.id, &tids);
    gdi32::process_ended(cpu, process.id);
    ole32::process_ended(cpu, process.id, &tids);
    ddraw::process_ended(cpu, process.id, &tids);
    dsound::process_ended(cpu, process.id, &tids);
    dinput::process_ended(cpu, process.id, &tids);
    winmm::process_ended(cpu, process.id, &tids);
    glide::process_ended(cpu, process.id, &tids);
    let win32 = &mut cpu.bus.win32;
    if win32.running != index {
        // Another process's thread runs on.
        if win32.running > index {
            win32.running -= 1;
        }
        thread::wake(cpu);
        return;
    }
    win32.running = 0;
    if !win32.processes.is_empty() {
        thread::wake(cpu);
        thread::run_next(cpu);
        return;
    }
    let (rm_stack, a20, code) = (win32.rm_stack, win32.a20, win32.exit_code);
    system::enter_real(cpu, (ROM_SEG, EXIT), rm_stack, 0x0202);
    if let Some(pool) = cpu.bus.win32.pool.take() {
        pool.release(&mut cpu.bus);
    }
    object::close_all_files(&mut cpu.bus);
    cpu.bus.win32.objects = object::Objects::default();
    cpu.bus.win32.sections.clear();
    cpu.bus.set_a20(a20);
    cpu.set_ax(0x4C00 | (code & 0xFF) as u16);
    if cpu.state == CpuState::Halted {
        cpu.state = CpuState::Running;
    }
}

/// End the program because of `what` (where `ctx` says, if it was in the
/// program), saying so on standard error.
fn fatal(cpu: &mut Cpu, what: &str, ctx: Option<&Context>) {
    let name = running(cpu).map_or(String::new(), |p| p.name.clone());
    let place = match ctx {
        Some(ctx) => format!(" at {:04X}:{:08X}", ctx.seg[Seg::CS as usize], ctx.eip),
        None => format!(" at {:04X}:{:08X}", cpu.cs(), cpu.eip()),
    };
    let message = format!("{} ended: {}{}.", name, what, place);
    cpu.bus.log_string(&format!("[W32] {}", message));
    say(cpu, &message);
    exit(cpu, FAILED);
}

/// The 32-bit value at `lin` in the running process's memory, read
/// through its page tables without a page fault: None where no page is
/// there. For what a message or a log line says, which mustn't fault a
/// function that has already ended the process.
fn peek_32(cpu: &Cpu, lin: u32) -> Option<u32> {
    let phys = running(cpu)?.memory.translate(&cpu.bus, lin)?;
    Some(cpu.bus.read_32(phys as usize))
}

/// The program called a function a built-in DLL doesn't have yet.
fn missing(cpu: &mut Cpu, index: u16) {
    let caller = peek_32(cpu, cpu.esp()).unwrap_or(0);
    let (dll, function) = process(cpu)
        .missing
        .get(index as usize)
        .cloned()
        .unwrap_or_default();
    let message = format!(
        "{} needs {}'s function {} (called from {:08X}), which the built-in Windows doesn't have yet.",
        process(cpu).name,
        dll,
        function,
        caller
    );
    cpu.bus.log_string(&format!("[W32] {}", message));
    say(cpu, &message);
    exit(cpu, FAILED);
}

/// Say `message` on standard error (DOS's handle 2).
fn say(cpu: &mut Cpu, message: &str) {
    let mut bytes = crate::dosstr::to_bytes(message);
    bytes.extend_from_slice(b"\r\n");
    let _ = dos(cpu, |cpu| {
        crate::interrupts::int21::write_handle(cpu, 2, &bytes)
    });
}

/// Run DOS's code for the program. DOS's tables are in the first
/// megabyte, which the process maps one to one for the system only, so DOS
/// reaches them by their physical addresses, as without paging.
fn dos<T>(cpu: &mut Cpu, f: impl FnOnce(&mut Cpu) -> T) -> T {
    let paging = cpu.bus.guest_paging.take();
    let result = f(cpu);
    cpu.bus.guest_paging = paging;
    result
}

/// Whether the built-in Windows's sound device plays (DirectSound's
/// buffers, WinMM's wave audio), for the host's mixer.
pub fn audio_playing(win32: &Win32) -> bool {
    dsound::device::playing(win32)
}

/// The sound device's samples for the host's frames `first` to `first +
/// frames`, if it plays (dsound/device.rs).
pub fn audio_render(
    bus: &crate::bus::Bus,
    first: u64,
    frames: usize,
) -> Option<Vec<(f32, f32)>> {
    dsound::device::render(bus, first, frames)
}

/// When the sound device next needs its tick (a notification, a block
/// done), in PIT ticks after `now`.
pub fn audio_next_event(win32: &Win32, now: u64) -> Option<u64> {
    [
        dsound::next_event(&win32.dsound, now),
        winmm::wave::next_event(&win32.winmm.wave, now),
    ]
    .into_iter()
    .flatten()
    .min()
}

/// DOS ended the process `psp`: the Win32 processes in it, if any, go
/// too.
pub fn process_ended(cpu: &mut Cpu, psp: u16) {
    let win32 = &mut cpu.bus.win32;
    if win32.active() && win32.psp == psp {
        let a20 = win32.a20;
        for process in std::mem::take(&mut win32.processes) {
            process.memory.release(&mut cpu.bus);
            registry::process_ended(cpu, process.id);
        }
        if let Some(pool) = cpu.bus.win32.pool.take() {
            pool.release(&mut cpu.bus);
        }
        object::close_all_files(&mut cpu.bus);
        cpu.bus.win32.objects = object::Objects::default();
        cpu.bus.win32.sections.clear();
        if cpu.bus.win32.user.desktop {
            user32::paint::leave_desktop(cpu);
        }
        cpu.bus.win32.user = user32::State::default();
        cpu.bus.win32.gdi = gdi32::State::default();
        cpu.bus.win32.ole = ole32::State::default();
        cpu.bus.win32.ddraw = ddraw::State::default();
        cpu.bus.win32.dsound = dsound::State::default();
        cpu.bus.win32.dinput = dinput::State::default();
        cpu.bus.win32.winmm = winmm::State::default();
        cpu.bus.win32.glide = glide::State::default();
        cpu.bus.set_a20(a20);
    }
    cpu.bus.win32.pending = None;
}

/// What the trap with id `id` runs, for the disassembler.
pub fn trap_name(id: u16) -> String {
    match id {
        ID_START => "Win32 start".to_string(),
        ID_IRQ_RETURN => "Win32 IRQ return".to_string(),
        _ if id & 0xFF00 == ID_IDT => format!("Win32 IDT {:02X}h", id as u8),
        _ if id >= FIRST_MISSING => format!("Win32 missing function {}", id - FIRST_MISSING),
        _ => format!(
            "Win32 {}",
            function_name(id).unwrap_or_else(|| format!("{:04X}h", id))
        ),
    }
}

crate::state_fields!(Module {
    name,
    base,
    size,
    entry,
    path,
    refs,
    flags,
    deps
});
crate::state_fields!(Process {
    id,
    object,
    name,
    path,
    memory,
    system,
    modules,
    image_base,
    stub_pages,
    missing,
    missing_base,
    handles,
    std,
    threads,
    current,
    idle_stack,
    last_error,
    tib,
    command_line,
    tls_template,
    exception_filter,
    stack_reserve,
    tls_used,
    environment,
    heap,
    heaps,
    moveables,
    startup,
    priority_class,
    error_mode,
    subsystem_version,

    dll_tls,
    exiting,
});
crate::state_fields!(Win32 {
    pending,
    processes,
    running,
    pool,
    objects,
    psp,
    rm_stack,
    a20,
    cr4,
    interrupted,
    exit_code,
    slice_start,
    ids,
    root,
    registry,

    sections,

    long_dirs,
    console_mode,
    user,
    gdi,

    ole,
    ddraw,
    dsound,
    dinput,
    winmm,
    glide,
    guids,
} skip { enabled });
