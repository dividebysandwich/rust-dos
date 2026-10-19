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

mod convert;
mod heap;
mod kernel32;
mod loader;
mod memory;
mod stubs;
mod system;

pub use stubs::function_name;

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
}

/// A running Win32 program.
#[derive(Clone, Debug, Default)]
pub struct Process {
    /// Its file's name.
    name: String,
    memory: Memory,
    /// The system block's physical address (system.rs).
    system: u32,
    /// The DOS process it runs in, and the stack its DOS stub had, where
    /// real-mode handlers of hardware interrupts run.
    psp: u16,
    rm_stack: (u16, u16),
    /// The A20 gate and CR4 before it started.
    a20: bool,
    cr4: u32,
    /// The program's context while the real-mode handler of a hardware
    /// interrupt runs.
    interrupted: Option<Context>,
    modules: Vec<Module>,
    image_base: u32,
    /// The physical pages of the built-in DLLs' stubs: the traps there are
    /// theirs.
    stub_pages: Vec<u32>,
    /// The functions of the stubs that end the program, and where those
    /// are.
    missing: Vec<(String, String)>,
    missing_base: u32,
    /// The DOS handles of its handles (`dos_handle`).
    handles: Vec<u16>,
    last_error: u32,
    command_line: u32,
    tib: u32,
    /// TLS slots in use, a bit each.
    tls_used: u128,
    /// The environment, as "NAME=value" strings: DOS's when it started.
    environment: Vec<String>,
    /// The process heap (GetProcessHeap).
    heap: heap::Heap,
}

impl Process {
    /// The handle of DOS's handle `dos`: 4, 8, 12, ...
    fn dos_handle(&mut self, dos: u16) -> u32 {
        let i = self
            .handles
            .iter()
            .position(|&h| h == dos)
            .unwrap_or_else(|| {
                self.handles.push(dos);
                self.handles.len() - 1
            });
        (i as u32 + 1) * 4
    }

    /// The DOS handle of the handle `handle`.
    fn dos_of(&self, handle: u32) -> Option<u16> {
        if !handle.is_multiple_of(4) || handle == 0 {
            return None;
        }
        self.handles.get((handle / 4 - 1) as usize).copied()
    }

    /// The base of the module `name`, ".DLL" added to a name without an
    /// extension.
    fn module(&self, name: &str) -> Option<u32> {
        let file = name.rsplit(['\\', '/']).next().unwrap_or(name);
        let file = if file.contains('.') {
            file.to_string()
        } else {
            format!("{}.DLL", file)
        };
        self.modules
            .iter()
            .find(|m| m.name.eq_ignore_ascii_case(&file))
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
    process: Option<Process>,
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
        self.process.is_some()
    }

    /// Whether DOS loaded a Win32 program that hasn't started yet.
    pub fn pending(&self) -> bool {
        self.pending.is_some()
    }

    /// The extended memory the process holds, as (address, bytes): for the
    /// XMS pool after a state is loaded.
    pub fn reservations(&self) -> Vec<(u32, u32)> {
        self.process
            .iter()
            .map(|p| (p.memory.base, p.memory.len))
            .collect()
    }

    /// The running program's modules (name, base, size), for debuggers.
    pub fn modules(&self) -> Vec<(String, u32, u32)> {
        self.process
            .iter()
            .flat_map(|p| p.modules.iter().map(|m| (m.name.clone(), m.base, m.size)))
            .collect()
    }
}

fn process(cpu: &mut Cpu) -> &mut Process {
    cpu.bus.win32.process.as_mut().expect("a Win32 process")
}

/// Run `f` with the process and the bus, for work on the process's memory.
fn with_process<T>(cpu: &mut Cpu, f: impl FnOnce(&mut Process, &mut crate::bus::Bus) -> T) -> T {
    let mut process = cpu.bus.win32.process.take().expect("a Win32 process");
    let result = f(&mut process, &mut cpu.bus);
    cpu.bus.win32.process = Some(process);
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
            ID_START => !cpu.pe() && win32.pending.is_some() && win32.process.is_none(),
            ID_IRQ_RETURN => {
                !cpu.pe()
                    && win32
                        .process
                        .as_ref()
                        .is_some_and(|p| p.interrupted.is_some())
            }
            _ => id & 0xFF00 == ID_IDT && cpu.pm() && cpu.cpl == 0 && win32.process.is_some(),
        };
    }
    let Some(process) = &win32.process else {
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
    let loaded = match cpu.read_program_file(&path) {
        Some(file) => loader::load(cpu, &path, &file),
        None => Err(format!("{} can't be read.", path)),
    };
    let loaded = match loaded {
        Ok(loaded) => loaded,
        Err(message) => {
            cpu.bus.log_string(&format!("[W32] {}", message));
            say(cpu, &message);
            cpu.set_ax(0x4C01);
            cpu.set_ip(EXIT);
            return;
        }
    };
    let mut process = loaded.process;
    process.psp = cpu.current_psp;
    process.environment = dos_environment(cpu, process.psp);
    process.rm_stack = (cpu.ss(), cpu.sp());
    process.a20 = cpu.bus.a20();
    process.cr4 = cpu.cr4;
    let return_address = stubs::stub_address(0, kernel32::PROCESS_RETURN);
    let esp = loaded.stack_top - 4;
    process.memory.write_32(&mut cpu.bus, esp, return_address);
    cpu.bus.log_string(&format!(
        "[W32] Started {} at {:08X}, image at {:08X}, {} KB of memory",
        process.name,
        loaded.entry,
        process.image_base,
        process.memory.len / 1024
    ));
    cpu.bus.win32.process = Some(process);
    cpu.bus.set_a20(true);
    // Win32 programs start with the FPU's precision at 53 bits.
    cpu.fpu_control = 0x027F;
    let mut ctx = Context {
        eip: loaded.entry,
        eflags: 0x0202,
        ..Context::default()
    };
    ctx.gpr[ESP] = esp;
    ctx.seg = [
        system::DATA3,
        system::CODE3,
        system::DATA3,
        system::DATA3,
        system::TIB3,
        0,
    ];
    system::enter_user(cpu, &ctx);
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

/// End the process with exit code `code`: back in real mode, the DOS
/// process ends with the low byte as its exit code.
fn exit(cpu: &mut Cpu, code: u32) {
    let Some(process) = cpu.bus.win32.process.as_ref() else {
        return;
    };
    let rm_stack = process.rm_stack;
    cpu.bus.log_string(&format!(
        "[W32] {} ended with exit code {:X}",
        process.name, code
    ));
    system::enter_real(cpu, (ROM_SEG, EXIT), rm_stack, 0x0202);
    let process = cpu.bus.win32.process.take().expect("the process");
    process.memory.release(&mut cpu.bus);
    cpu.bus.set_a20(process.a20);
    cpu.set_ax(0x4C00 | (code & 0xFF) as u16);
    if cpu.state == CpuState::Halted {
        cpu.state = CpuState::Running;
    }
}

/// End the program because of `what` (where `ctx` says, if it was in the
/// program), saying so on standard error.
fn fatal(cpu: &mut Cpu, what: &str, ctx: Option<&Context>) {
    let name = cpu
        .bus
        .win32
        .process
        .as_ref()
        .map_or(String::new(), |p| p.name.clone());
    let place = match ctx {
        Some(ctx) => format!(" at {:04X}:{:08X}", ctx.seg[Seg::CS as usize], ctx.eip),
        None => format!(" at {:04X}:{:08X}", cpu.cs(), cpu.eip()),
    };
    let message = format!("{} ended: {}{}.", name, what, place);
    cpu.bus.log_string(&format!("[W32] {}", message));
    say(cpu, &message);
    exit(cpu, FAILED);
}

/// The 32-bit value at `lin` in the process's memory, read through its
/// page tables without a page fault: None where no page is there. For
/// what a message or a log line says, which mustn't fault a function that
/// has already ended the process.
fn peek_32(cpu: &Cpu, lin: u32) -> Option<u32> {
    let process = cpu.bus.win32.process.as_ref()?;
    let phys = process.memory.translate(&cpu.bus, lin)?;
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

/// DOS ended the process `psp`: a Win32 process in it, if any, goes too.
pub fn process_ended(cpu: &mut Cpu, psp: u16) {
    if cpu.bus.win32.process.as_ref().is_some_and(|p| p.psp == psp) {
        let process = cpu.bus.win32.process.take().expect("the process");
        process.memory.release(&mut cpu.bus);
        cpu.bus.set_a20(process.a20);
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

crate::state_fields!(Module { name, base, size });
crate::state_fields!(Process {
    name,
    memory,
    system,
    psp,
    rm_stack,
    a20,
    cr4,
    interrupted,
    modules,
    image_base,
    stub_pages,
    missing,
    missing_base,
    handles,
    last_error,
    command_line,
    tib,
    tls_used,
    environment,
    heap,
});
crate::state_fields!(Win32 { pending, process } skip { enabled });
