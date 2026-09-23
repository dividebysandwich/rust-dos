//! The execution loop: timer service, hardware interrupts, the shell's
//! command hand-off, emulator service traps (BOPs), and instruction decode
//! and execution.
//!
//! The front end runs it in batches of emulated time between video frames
//! (`run_batch`); tests step it one instruction at a time (`Cpu::step`).
//! Both go through the same code.

use iced_x86::{Decoder, DecoderOptions};

use crate::command::CommandDispatcher;
use crate::cpu::{CR0_PE, Cpu, CpuFlags, CpuState, Fault, IntSource, Seg};
use crate::instr_cache::InstrCache;

/// Why `run_batch` returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// Emulated time reached the end of the batch.
    BatchEnd,
    /// The hook asked to stop before an instruction (breakpoint, step).
    Paused,
    /// A program ended and the shell was reloaded.
    ShellReloaded,
}

/// Observer of the execution loop, such as the debugger.
pub trait ExecHook {
    /// Called before each instruction while `run_batch`'s `hot` is set,
    /// with the instruction's physical address and all of RAM. Returns true
    /// to stop before the instruction executes.
    fn before_exec(&mut self, cpu: &Cpu, phys_ip: usize, ram: &[u8]) -> bool;
}

/// A hook that observes nothing.
pub struct NoHook;

impl ExecHook for NoHook {
    fn before_exec(&mut self, _cpu: &Cpu, _phys_ip: usize, _ram: &[u8]) -> bool {
        false
    }
}

/// Instruction fetch: a decoder reading straight out of RAM, and the
/// decoded-instruction cache, taken out of the CPU while it runs so a
/// cached instruction can be used in place while the CPU executes it.
struct Fetch {
    decoder: Decoder<'static>,
    ram: &'static [u8],
    cache: InstrCache,
}

impl Fetch {
    fn new(cpu: &mut Cpu) -> Self {
        // SAFETY: we build a read-only view of the bus RAM that outlives the
        // mutable borrows of the CPU while instructions execute (which may
        // write RAM through `bus.write_8`). This is sound because:
        //   1. The RAM buffer is allocated once in `Bus::new` and never
        //      resized, so the pointer stays valid.
        //   2. Emulation is single-threaded, so no concurrent access occurs.
        //   3. Reads go through this slice and writes through the bus, one
        //      after the other, never overlapping, and `u8` has no alignment
        //      or validity requirements that aliasing could violate.
        //   4. Self-modifying code works because the decoder reads the bytes
        //      at decode time, and every RAM write bumps the page generation
        //      that invalidates cached decodes.
        let ram = cpu.bus.ram();
        let ram: &'static [u8] = unsafe { std::slice::from_raw_parts(ram.as_ptr(), ram.len()) };
        Self {
            decoder: Decoder::with_ip(16, ram, 0, DecoderOptions::NONE),
            ram,
            cache: std::mem::take(&mut cpu.decode_cache),
        }
    }

    /// Give the decoded-instruction cache back to the CPU.
    fn finish(self, cpu: &mut Cpu) {
        cpu.decode_cache = self.cache;
    }
}

/// Run until emulated time reaches the batch end set with
/// `Bus::start_batch`, the hook stops execution, or the shell is reloaded.
/// `hot` enables the per-instruction `ExecHook::before_exec` call.
pub fn run_batch(cpu: &mut Cpu, hook: &mut dyn ExecHook, hot: bool) -> StopReason {
    let mut fetch = Fetch::new(cpu);
    let reason = run(cpu, &mut fetch, hook, hot);
    fetch.finish(cpu);
    reason
}

fn run(cpu: &mut Cpu, fetch: &mut Fetch, hook: &mut dyn ExecHook, hot: bool) -> StopReason {
    loop {
        if cpu.bus.clock.icount >= cpu.bus.clock.deadline {
            if cpu.bus.clock.icount >= cpu.bus.clock.batch_end() {
                return StopReason::BatchEnd;
            }
            cpu.bus.service_timers();
        }

        if deliver_interrupts(cpu) {
            continue;
        }

        // Fast check first: the shell rarely has anything to do.
        if cpu.pending_command.is_some()
            || !cpu.batch_queue.is_empty()
            || cpu.state == CpuState::RebootShell
        {
            match shell_services(cpu) {
                Shell::Idle => {}
                Shell::Handled => continue,
                Shell::Reloaded => return StopReason::ShellReloaded,
            }
        }

        if let Some(reason) = instruction(cpu, fetch, hook, hot) {
            return reason;
        }
    }
}

impl Cpu {
    /// Execute one step: service a due timer, deliver a pending hardware
    /// interrupt, or run one instruction or emulator service trap. The
    /// shell's command hand-off is left to `run_batch`.
    pub fn step(&mut self) {
        if self.bus.clock.icount >= self.bus.clock.deadline {
            self.bus.service_timers();
        }
        match self.state {
            CpuState::Running => {}
            // HLT: only an interrupt resumes execution.
            CpuState::Halted => {
                if deliver_interrupts(self) {
                    self.state = CpuState::Running;
                }
                return;
            }
            CpuState::RebootShell => return,
        }
        if deliver_interrupts(self) {
            return;
        }
        let mut fetch = Fetch::new(self);
        instruction(self, &mut fetch, &mut NoHook, false);
        fetch.finish(self);
    }

    /// True while the shell runs with no program started from it: the
    /// point at which batch lines and typed commands are dispatched.
    pub fn at_shell_prompt(&self) -> bool {
        self.cs() == 0 && self.process_stack.is_empty()
    }
}

/// Deliver a pending hardware interrupt, or a mouse event handler call, if
/// interrupts are enabled. Returns true if one was delivered.
#[inline(always)]
fn deliver_interrupts(cpu: &mut Cpu) -> bool {
    // The instruction after STI, MOV SS or POP SS runs before any
    // interrupt, so a program can switch SS:SP without being interrupted
    // halfway.
    cpu.get_cpu_flag(CpuFlags::IF)
        && !cpu.irq_shadow
        && cpu.bus.interrupt_requested()
        && deliver_pending(cpu)
}

fn deliver_pending(cpu: &mut Cpu) -> bool {
    // Deliver at the start of an instruction as a CPU would, when the PIC
    // lets the line through: not masked in the IMR and not blocked by an
    // interrupt still in service.
    if let Some(irq) = cpu.bus.pic_pending_irq() {
        let vector = cpu.bus.pic.vector(irq);
        let entry = cpu.idtr.base.wrapping_add(vector as u32 * 4);
        if cpu.read_linear_u16(entry) == 0 && cpu.read_linear_u16(entry.wrapping_add(2)) == 0 {
            // No handler installed — drop the IRQ rather than spinning on it.
            cpu.bus.pic_drop(irq);
            return true;
        }
        cpu.bus.pic_acknowledge(irq);
        let (eip, esp) = (cpu.eip(), cpu.esp());
        if let Err(fault) = cpu.deliver_interrupt(vector, IntSource::External) {
            cpu.set_eip(eip);
            cpu.set_esp(esp);
            cpu.raise(fault);
        }
        return true;
    }

    // Mouse event handler installed with INT 33h AX=000C.
    crate::mouse::deliver_callback(cpu)
}

enum Shell {
    /// Nothing for the shell to do.
    Idle,
    /// A command line was dispatched.
    Handled,
    /// A program ended and the shell was reloaded.
    Reloaded,
}

/// Feed batch lines to the shell, dispatch the command line it handed over,
/// and reload it after a program ends.
fn shell_services(cpu: &mut Cpu) -> Shell {
    // If the shell is idle at its prompt and batch lines are pending, pop
    // one and feed it through the same path as a typed command. The line is
    // echoed at a synthesized prompt, MS-DOS style. A leading '@'
    // suppresses the echo.
    if cpu.pending_command.is_none() && !cpu.batch_queue.is_empty() && cpu.at_shell_prompt() {
        let raw = cpu.batch_queue.pop_front().unwrap();
        let (line, line_echo) = match raw.strip_prefix('@') {
            Some(stripped) => (stripped.trim().to_string(), false),
            None => (raw, true),
        };
        if !line.is_empty() {
            // Per-line @ suppresses the echo for that line only; ECHO OFF
            // (cpu.batch_echo) suppresses all subsequent lines.
            if line_echo && cpu.batch_echo {
                crate::shell::show_prompt(cpu);
                crate::video::print_string(cpu, &format!("{}\r\n", line));
            }
            cpu.pending_command = Some(line);
        }
    }

    // (Checked before taking it: `take` would store None back on every
    // instruction.)
    if cpu.pending_command.is_some() {
        let cmd = cpu.pending_command.take().unwrap();
        dispatch_command(cpu, &cmd);
        return Shell::Handled;
    }

    if cpu.state == CpuState::RebootShell {
        cpu.load_shell();
        cpu.state = CpuState::Running;

        // Start the prompt on a new line.
        if cpu.bus.read_8(0x0450) != 0 {
            crate::video::print_string(cpu, "\r\n");
        }
        return Shell::Reloaded;
    }

    Shell::Idle
}

/// Run a command line handed over by the shell: a built-in command, or a
/// program or batch file.
fn dispatch_command(cpu: &mut Cpu, cmd: &str) {
    cpu.bus
        .log_string(&format!("[MAIN] Processing Command: {}", cmd));

    let (command, args) = match cmd.split_once(' ') {
        Some((c, a)) => (c, a.trim()),
        None => (cmd, ""),
    };

    if CommandDispatcher::new().dispatch(cpu, command, args) {
        // Built-in command executed. The shell continues.
        return;
    }

    // Load a program. With no extension we probe .com, .exe, then .bat,
    // matching COMMAND.COM's precedence. A loaded program starts at the
    // CS:IP the loader set.
    let lower = command.to_lowercase();
    let loaded = if lower.ends_with(".bat") {
        cpu.queue_batch_file(command)
    } else if !command.contains('.') {
        load_program(cpu, &format!("{}.com", command), args)
            || load_program(cpu, &format!("{}.exe", command), args)
            || cpu.queue_batch_file(&format!("{}.bat", command))
    } else {
        load_program(cpu, command, args)
    };

    if !loaded {
        crate::video::print_string(cpu, "Bad command or file name.\r\n");
    }
}

/// Load a program from the shell with its command line arguments.
fn load_program(cpu: &mut Cpu, filename: &str, args: &str) -> bool {
    if !cpu.load_executable(filename, None) {
        return false;
    }
    cpu.set_command_tail(cpu.current_psp, args);
    true
}

/// Run one instruction or emulator service trap at CS:IP.
#[inline(always)]
fn instruction(
    cpu: &mut Cpu,
    fetch: &mut Fetch,
    hook: &mut dyn ExecHook,
    hot: bool,
) -> Option<StopReason> {
    // This instruction ends the interrupt shadow of the previous one.
    cpu.irq_shadow = false;

    // Instruction fetch past the end of the code segment raises #GP(0).
    // (In 16-bit code, EIP runs on past FFFFh rather than wrapping.)
    let eip = cpu.eip();
    let cs = *cpu.seg_cache(Seg::CS);
    if eip > cs.limit {
        cpu.raise(Fault::gp(0));
        return None;
    }
    let phys_ip = cpu.translate(cs.base.wrapping_add(eip)) as usize;

    // Tripwire: arriving in the IVT / BIOS data area with an application
    // context (DS not 0, not the shell at CS=0) almost always means a
    // corrupted FAR pointer landed us here.
    if cpu.cs() == 0
        && cpu.ip() < 0x100
        && cpu.ds() != 0
        && cpu.ds() != cpu.transient_segment()
        && cpu.current_psp != 0
    {
        report_tripwire(cpu);
        cpu.state = CpuState::RebootShell;
        return None;
    }

    if hot && hook.before_exec(cpu, phys_ip, fetch.ram) {
        return Some(StopReason::Paused);
    }
    cpu.executed += 1;

    // Decode via the decoded-instruction cache. On a hit (the common case
    // inside hot loops) iced's decoder is skipped entirely.
    let slow;
    let instr = if phys_ip + 16 <= fetch.ram.len() {
        let page_gen = cpu.bus.page_gen[phys_ip >> 12];
        let decoder = &mut fetch.decoder;
        fetch.cache.get_or_decode(phys_ip, cs.selector, eip, page_gen, |slot| {
            decoder.set_position(phys_ip).unwrap();
            decoder.set_ip(eip as u64);
            decoder.decode_out(slot);
        })
    } else {
        // At the end of RAM or past it: fetch through the bus.
        let mut bytes = [0u8; 16];
        for (i, byte) in bytes.iter_mut().enumerate() {
            let lin = cs.base.wrapping_add(eip).wrapping_add(i as u32);
            *byte = cpu.bus.read_8(cpu.translate(lin) as usize);
        }
        slow = Decoder::with_ip(16, &bytes, eip as u64, DecoderOptions::NONE).decode();
        &slow
    };

    let next_eip = eip.wrapping_add(instr.len() as u32);
    if next_eip - 1 > cs.limit {
        // The instruction's last bytes lie past the segment limit.
        cpu.raise(Fault::gp(0));
        return None;
    }
    let start_esp = cpu.esp();
    cpu.set_eip(next_eip);
    if let Err(fault) = crate::instructions::execute_instruction(cpu, instr) {
        // A fault leaves the instruction undone: EIP back on it, and ESP as
        // it was (the handlers commit everything else last).
        cpu.set_eip(eip);
        cpu.set_esp(start_esp);
        if !(fault == Fault::UD && service_trap(cpu, fetch.ram, phys_ip)) {
            cpu.raise(fault);
        }
    }
    cpu.bus.clock.icount += 1;

    if cpu.bus.reset_requested {
        // The keyboard controller or port 92h pulsed the reset line.
        cpu.bus.reset_requested = false;
        cpu.bus.log_string("[CPU] Reset requested");
        cpu.reset();
    }

    if cpu.state == CpuState::Halted && cpu.bus.clock.deadline != u64::MAX {
        // HLT: nothing runs until the next interrupt, so skip ahead to the
        // next timer event. With nothing scheduled (a CPU stepped outside a
        // batch) it stays halted until an interrupt arrives.
        cpu.state = CpuState::Running;
        cpu.bus.clock.skip_to_deadline();
    }
    None
}

/// Run the emulator service ("BOP") at `phys_ip`, if there is one. BOPs use
/// the invalid FE /7 encodings and only work in real mode:
///
/// * `FE 38 vv`: the HLE handler of interrupt vv, entered through the
///   interrupt vector table; returns with a simulated IRET.
/// * `FE 39 vv`: an inline service vv; execution continues after it.
fn service_trap(cpu: &mut Cpu, ram: &[u8], phys_ip: usize) -> bool {
    if cpu.cr0 & CR0_PE != 0 || phys_ip + 3 > ram.len() || ram[phys_ip] != 0xFE {
        return false;
    }
    let vector = ram[phys_ip + 2];
    match ram[phys_ip + 1] {
        0x38 => {
            crate::interrupts::handle_hle(cpu, vector);
            crate::interrupts::return_from_hle(cpu, vector);
        }
        0x39 => {
            cpu.set_ip(cpu.ip().wrapping_add(3));
            crate::interrupts::handle_inline_bop(cpu, vector);
        }
        _ => return false,
    }
    if cpu.idle {
        // A BIOS service is waiting for input: skip ahead to the next
        // timer event instead of spinning on the retry.
        cpu.idle = false;
        cpu.bus.clock.skip_to_deadline();
    }
    true
}

fn report_tripwire(cpu: &mut Cpu) {
    cpu.bus.log_string(&format!(
        "[TRIPWIRE] Entered IVT region CS:IP={:04X}:{:04X} DS={:04X} ES={:04X} SS:SP={:04X}:{:04X} AX={:04X} BX={:04X} CX={:04X} DX={:04X}",
        cpu.cs(), cpu.ip(), cpu.ds(), cpu.es(), cpu.ss(), cpu.sp(), cpu.ax(), cpu.bx(), cpu.cx(), cpu.dx()
    ));
    // Dump 64 bytes of stack so we can see remaining return addresses
    // (anything the RETF left unconsumed).
    let ss_base = (cpu.ss() as usize) * 16;
    let mut sbytes = String::new();
    for i in 0..64 {
        let a = ss_base + cpu.sp() as usize + i;
        if a < cpu.bus.ram().len() {
            sbytes.push_str(&format!("{:02X} ", cpu.bus.ram()[a]));
        }
    }
    cpu.bus.log_string(&format!(
        "[TRIPWIRE] stack@SS:SP ({:04X}:{:04X}): {}",
        cpu.ss(),
        cpu.sp(),
        sbytes.trim()
    ));
    cpu.bus.flush_log();
}
