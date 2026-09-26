//! The execution loop: timer service, hardware interrupts, the shell's
//! command hand-off, emulator service traps (BOPs), and instruction decode
//! and execution.
//!
//! The front end runs it in batches of emulated time between video frames
//! (`run_batch`); tests step it one instruction at a time (`Cpu::step`).
//! Both go through the same code.

use iced_x86::{Decoder, DecoderOptions, Instruction};

use crate::bus::GEN_SHIFT;
use crate::command::{CommandDispatcher, split_command};
use crate::cpu::{ATTR_DB, CR0_PE, CR0_PG, Cpu, CpuFlags, CpuState, Fault, IntSource, SHELL_SEGMENT, Seg};
use crate::dynrec::{DynState, Run};
use crate::instr_cache::InstrCache;
use crate::instructions::Handler;

/// Why `run_batch` returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// Emulated time reached the end of the batch.
    BatchEnd,
    /// The hook asked to stop before an instruction (breakpoint, step).
    Paused,
    /// A program ended and the shell was reloaded.
    ShellReloaded,
    /// The EXIT command asked to turn the machine off (`Bus::exit_requested`).
    Exit,
}

/// Observer of the execution loop, such as the debugger.
pub trait ExecHook {
    /// Called before each instruction while `run_batch`'s `hot` is set,
    /// with the instruction's physical address and all of RAM. Returns true
    /// to stop before the instruction executes.
    fn before_exec(&mut self, cpu: &Cpu, phys_ip: usize, ram: &[u8]) -> bool;

    /// Whether `before_exec` has to see every instruction. When it doesn't,
    /// the dynamic recompiler runs blocks of instructions between the
    /// calls: `before_exec` then sees the first instruction of each block
    /// and the instructions the interpreter runs, which include every HLT.
    fn per_instruction(&self) -> bool {
        true
    }
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
    /// Decoders for 16-bit and 32-bit code segments.
    decoder16: Decoder<'static>,
    decoder32: Decoder<'static>,
    ram: &'static [u8],
    cache: InstrCache,
    window: CodeWindow,
    /// The dynamic recompiler, taken out of the CPU like the cache.
    dynrec: DynState,
}

/// The page instructions are being fetched from: where it is in physical
/// memory, and what that depends on. Inside it, an instruction needs none of
/// the checks and translation `locate` makes: it can't run past the page
/// (the window ends 15 bytes before it does), past RAM, or into the IVT,
/// and a CS limit it would run past sends it through `locate`. The
/// translation holds while the TLB isn't flushed, the privilege level (the
/// pages' user bits) and the A20 gate don't change; CS can change, as the
/// window is linear addresses.
struct CodeWindow {
    /// First linear address, and how many bytes the window has (0: none).
    lin: u32,
    len: u32,
    /// Physical address of `lin`.
    phys: usize,
    tlb_epoch: u32,
    cpl: u8,
    a20_mask: u32,
}

/// Bytes at the end of a page where an instruction can start and run on
/// into the next one (the longest is 15 bytes).
const PAGE_TAIL: u32 = 15;

impl CodeWindow {
    const NONE: CodeWindow = CodeWindow { lin: 0, len: 0, phys: 0, tlb_epoch: 0, cpl: 0, a20_mask: 0 };

    /// The physical address of the instruction at `eip` (linear address
    /// `lin_ip`), if it is inside the window and ends within the CS limit.
    #[inline(always)]
    fn phys(&self, cpu: &Cpu, eip: u32, lin_ip: u32, cs_limit: u32) -> Option<usize> {
        let offset = lin_ip.wrapping_sub(self.lin);
        let inside = offset < self.len
            && eip as u64 + PAGE_TAIL as u64 - 1 <= cs_limit as u64
            && self.tlb_epoch == cpu.tlb.epoch
            && self.cpl == cpu.cpl
            && self.a20_mask == cpu.bus.a20_mask();
        inside.then(|| self.phys + offset as usize)
    }

    /// Translated code ran on into `page` through links (see
    /// `dynrec::Page`): make it the window, as the interpreter would have
    /// when it fetched from it, with what the window depended on then
    /// (`at`: the TLB's epoch, CPL and the A20 gate before the code ran,
    /// which only its last instruction can have changed).
    fn moved(&mut self, page: crate::dynrec::Page, at: (u32, u8, u32)) {
        if self.len != 0 && self.lin == page.lin {
            return;
        }
        let (tlb_epoch, cpl, a20_mask) = at;
        *self = CodeWindow { lin: page.lin, len: 0x1000 - PAGE_TAIL, phys: page.phys, tlb_epoch, cpl, a20_mask };
    }

    /// Make the page of `lin_ip`, which is at `phys_ip`, the window, unless
    /// it is the first page (the tripwire watches the IVT there) or not all
    /// in RAM.
    fn open(&mut self, cpu: &Cpu, lin_ip: u32, phys_ip: usize, ram_len: usize) {
        let offset = lin_ip & 0xFFF;
        let phys = phys_ip - offset as usize;
        if lin_ip < 0x1000 || phys + 0x1000 > ram_len {
            *self = Self::NONE;
            return;
        }
        *self = CodeWindow {
            lin: lin_ip - offset,
            len: 0x1000 - PAGE_TAIL,
            phys,
            tlb_epoch: cpu.tlb.epoch,
            cpl: cpu.cpl,
            a20_mask: cpu.bus.a20_mask(),
        };
    }
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
            decoder16: Decoder::with_ip(16, ram, 0, DecoderOptions::NONE),
            decoder32: Decoder::with_ip(32, ram, 0, DecoderOptions::NONE),
            ram,
            cache: std::mem::take(&mut cpu.decode_cache),
            window: CodeWindow::NONE,
            dynrec: std::mem::take(&mut cpu.dynrec),
        }
    }

    /// Give the decoded-instruction cache and the recompiler back to the
    /// CPU.
    fn finish(self, cpu: &mut Cpu) {
        cpu.decode_cache = self.cache;
        cpu.dynrec = self.dynrec;
    }
}

/// Run until emulated time reaches the batch end set with
/// `Bus::start_batch`, the hook stops execution, the shell is reloaded or
/// EXIT turns the machine off.
/// `hot` enables the per-instruction `ExecHook::before_exec` call.
pub fn run_batch(cpu: &mut Cpu, hook: &mut dyn ExecHook, hot: bool) -> StopReason {
    if crate::savestate::machine::chaos() {
        crate::savestate::machine::reload(cpu);
    }
    let mut fetch = Fetch::new(cpu);
    // Copies of the loop: the one without the per-instruction hook doesn't
    // test for it on every instruction, and the interpreter's doesn't look
    // for translated code. A program that switches to protected mode
    // during the batch (`core=auto`) goes on the recompiler in the next.
    let dynamic = cpu.dynamic_active() && !(hot && hook.per_instruction());
    let reason = match (hot, dynamic) {
        (false, false) => run::<false, false>(cpu, &mut fetch, hook),
        (false, true) => run::<false, true>(cpu, &mut fetch, hook),
        (true, false) => run::<true, false>(cpu, &mut fetch, hook),
        (true, true) => run::<true, true>(cpu, &mut fetch, hook),
    };
    fetch.finish(cpu);
    reason
}

fn run<const HOT: bool, const DYN: bool>(cpu: &mut Cpu, fetch: &mut Fetch, hook: &mut dyn ExecHook) -> StopReason {
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

        // Fast check first: the shell rarely has anything to do. It hands
        // over command lines from its own code (at SHELL_SEGMENT), and batch
        // lines wait while a program started from the batch file runs, so
        // they only count once the shell is back at its prompt.
        if cpu.state == CpuState::RebootShell
            || (cpu.in_shell_code()
                && (cpu.pending_command.is_some()
                    || (cpu.batch.is_active() && cpu.shell_wait.is_none() && cpu.process_stack.is_empty())))
        {
            match shell_services(cpu) {
                Shell::Idle => {}
                // Nothing after EXIT runs, not even the rest of its batch file.
                Shell::Handled if cpu.bus.exit_requested => return StopReason::Exit,
                Shell::Handled => continue,
                Shell::Reloaded => return StopReason::ShellReloaded,
            }
        }

        // An instruction in an interrupt shadow runs on its own, as the
        // loop checks for interrupts again after it.
        let stop = if DYN && !cpu.irq_shadow && cpu.dynamic_active() {
            dynamic::<HOT>(cpu, fetch, hook, false)
        } else {
            instruction::<HOT>(cpu, fetch, hook)
        };
        if let Some(reason) = stop {
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
        // Callers may have raised interrupts or changed the PICs directly.
        self.bus.refresh_irq();
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
        if self.dynamic_active() && !self.irq_shadow {
            dynamic::<false>(self, &mut fetch, &mut NoHook, true);
        } else {
            instruction::<false>(self, &mut fetch, &mut NoHook);
        }
        fetch.finish(self);
    }

    /// True while the shell runs with no program started from it: the
    /// point at which batch lines and typed commands are dispatched.
    pub fn at_shell_prompt(&self) -> bool {
        self.in_shell_code() && self.process_stack.is_empty()
    }

    /// Whether the shell's code runs: at SHELL_SEGMENT, in real mode. In
    /// protected mode the same number is a selector of a DOS extender's
    /// (DOS/4GW's code is 0070h), whose program is anything but idle.
    pub fn in_shell_code(&self) -> bool {
        !self.pe() && self.cs() == SHELL_SEGMENT
    }

    /// True while no program runs: the shell is at its prompt, or in the
    /// BIOS waiting for the keystroke it asked for.
    pub fn shell_idle(&self) -> bool {
        if !self.process_stack.is_empty() {
            return false;
        }
        let caller_cs = || {
            // A service waiting for slow disk access has its deadline and
            // vector (10 bytes) over the return frame.
            let waiting = self.ip() == crate::bios::IO_WAIT;
            let frame = self.get_physical_addr(self.ss(), self.sp().wrapping_add(if waiting { 12 } else { 2 }));
            self.bus.read_16(frame)
        };
        !self.pe() && (self.cs() == SHELL_SEGMENT || (self.cs() == 0xF000 && caller_cs() == SHELL_SEGMENT))
    }
}

/// Deliver a pending hardware interrupt, or a mouse event handler call, if
/// interrupts are enabled. Returns true if one was delivered.
#[inline(always)]
fn deliver_interrupts(cpu: &mut Cpu) -> bool {
    // The instruction after STI, MOV SS or POP SS runs before any
    // interrupt, so a program can switch SS:SP without being interrupted
    // halfway.
    cpu.bus.irq_ready
        && cpu.get_cpu_flag(CpuFlags::IF)
        && !cpu.irq_shadow
        && deliver_pending(cpu)
}

fn deliver_pending(cpu: &mut Cpu) -> bool {
    // Deliver at the start of an instruction as a CPU would, when the PIC
    // lets the line through: not masked in the IMR and not blocked by an
    // interrupt still in service.
    if let Some(irq) = cpu.bus.pic_pending_irq() {
        let vector = cpu.bus.pic.vector(irq);
        if !cpu.pe() {
            let entry = cpu.idtr.base.wrapping_add(vector as u32 * 4);
            if cpu.read_linear_u16(entry) == 0 && cpu.read_linear_u16(entry.wrapping_add(2)) == 0 {
                // No handler installed — drop the IRQ rather than spinning on it.
                cpu.bus.pic_drop(irq);
                return true;
            }
        }
        cpu.bus.pic_acknowledge(irq);
        cpu.state = CpuState::Running;
        let (eip, esp) = (cpu.eip(), cpu.esp());
        if let Err(fault) = cpu.deliver_interrupt(vector, IntSource::External, None) {
            cpu.set_eip(eip);
            cpu.set_esp(esp);
            cpu.raise(fault);
        }
        return true;
    }

    // Mouse event handler installed with INT 33h AX=000C, called the
    // real-mode way.
    let called = !cpu.pe() && crate::mouse::deliver_callback(cpu);
    cpu.bus.refresh_irq();
    called
}

enum Shell {
    /// Nothing for the shell to do.
    Idle,
    /// A command line was dispatched.
    Handled,
    /// A program ended and the shell was reloaded.
    Reloaded,
}

/// The emulated time COMMAND.COM takes to read and run a batch line. It
/// also lets the timers run between the lines of a batch file that loops
/// with GOTO and starts no program, which would otherwise never give the
/// front end the machine back.
pub const BATCH_LINE_NS: u64 = 500_000;

/// Feed batch lines to the shell, dispatch the command line it handed over,
/// and reload it after a program ends.
fn shell_services(cpu: &mut Cpu) -> Shell {
    // If the shell is idle at its prompt and batch lines are pending, take
    // the next and feed it through the same path as a typed command. The
    // line is echoed at a synthesized prompt, MS-DOS style, unless ECHO is
    // off or it begins with '@'. Ctrl+C ends the batch files.
    let mut from_batch = false;
    if cpu.pending_command.is_none() && cpu.batch.is_active() && cpu.shell_wait.is_none() && cpu.at_shell_prompt() {
        if take_ctrl_c(cpu) {
            cpu.batch.clear();
            crate::video::print_string(cpu, "^C\r\n");
        } else if let Some(line) = cpu.batch.next_line(&cpu.environment) {
            if line.echo {
                crate::shell::show_prompt(cpu);
                crate::video::print_string(cpu, &format!("{}\r\n", line.text));
            }
            cpu.pending_command = Some(line.text);
            from_batch = true;
            cpu.bus.clock.stall(BATCH_LINE_NS);
        }
    }

    // (Checked before taking it: `take` would store None back on every
    // instruction.)
    if cpu.pending_command.is_some() {
        let cmd = cpu.pending_command.take().unwrap();
        cpu.bus.disk_io.clear();
        // A batch file a batch line starts takes the place of the one
        // running.
        cpu.batch.dispatching = from_batch;
        run_command_line(cpu, &cmd);
        cpu.batch.dispatching = false;
        if from_batch {
            cpu.batch.settle();
        }
        // A program loaded from a slow disk starts once it's read.
        let disk_time = cpu.bus.disk_io.take_pending();
        if disk_time > 0 && cpu.state == CpuState::Running {
            crate::diskio::wait_before(cpu, disk_time);
        }
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

/// Take a Ctrl+C (or Ctrl+Break) keystroke out of the keyboard buffer, if
/// one was pressed.
fn take_ctrl_c(cpu: &mut Cpu) -> bool {
    let buffer = &mut cpu.bus.keyboard_buffer;
    match buffer.iter().position(|&key| key & 0xFF == 0x03 || key == 0x0000) {
        Some(i) => {
            buffer.remove(i);
            true
        }
        None => false,
    }
}

/// Run a command line handed over by the shell (or by IF): a built-in
/// command, or a program or batch file.
pub fn run_command_line(cpu: &mut Cpu, cmd: &str) {
    cpu.bus
        .log_string(&format!("[MAIN] Processing Command: {}", cmd));

    // A leading '@' (which hides a batch line's echo) means nothing here.
    let cmd = cmd.trim_start();
    let cmd = cmd.strip_prefix('@').unwrap_or(cmd);
    let (cmd, redirect) = take_redirections(cmd);
    let (command, args) = split_command(&cmd);
    if command.is_empty() {
        return;
    }

    if redirect.output.is_some() {
        cpu.stdout_capture = Some(Vec::new());
    }
    let builtin = CommandDispatcher::new().dispatch(cpu, command, args);
    if let Some(captured) = cpu.stdout_capture.take()
        && builtin
    {
        write_redirected(cpu, &redirect, &captured);
    }
    if builtin {
        // Built-in command executed. The shell continues.
        return;
    }

    match start(cpu, command, args.trim(), false, false) {
        None => crate::video::print_string(cpu, "Bad command or file name.\r\n"),
        Some(Started::Program) => redirect_program(cpu, &redirect),
        Some(Started::Batch) => {}
    }
}

/// Where a command line sends its output and takes its input.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Redirections {
    /// `>file`, or `>>file` to add to it (true).
    pub output: Option<(String, bool)>,
    /// `<file`.
    pub input: Option<String>,
}

/// A command line without its redirections (`>file`, `>>file`, `<file`),
/// and them. What comes after a '|' is left out: there are no pipes.
pub fn take_redirections(line: &str) -> (String, Redirections) {
    let mut redirect = Redirections::default();
    let mut rest = String::new();
    let mut chars = line.chars().peekable();
    let mut quoted = false;
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                rest.push(c);
            }
            '>' | '<' if !quoted => {
                let append = c == '>' && chars.next_if_eq(&'>').is_some();
                while chars.next_if(|c| *c == ' ' || *c == '\t').is_some() {}
                let mut name = String::new();
                while let Some(c) = chars.next_if(|c| !matches!(c, ' ' | '\t' | '<' | '>' | '|')) {
                    name.push(c);
                }
                if c == '>' {
                    redirect.output = Some((name, append));
                } else {
                    redirect.input = Some(name);
                }
            }
            '|' if !quoted => break,
            _ => rest.push(c),
        }
    }
    (rest.trim_end().to_string(), redirect)
}

/// Where a built-in command's redirected output goes: into the file
/// (after what it had with >>), or nowhere for NUL.
fn write_redirected(cpu: &mut Cpu, redirect: &Redirections, captured: &[u8]) {
    let Some((path, append)) = &redirect.output else { return };
    match crate::disk::char_device(path) {
        Some(crate::disk::CharDevice::Con) => return crate::video::print_cp437(cpu, captured, 0x07),
        Some(_) => return,
        None => {}
    }
    let mut data = Vec::new();
    if *append && let Some(file) = cpu.bus.disk.file_data(path) {
        data.extend(file.read().map(|b| b.to_vec()).unwrap_or_default());
        if data.last() == Some(&0x1A) {
            data.pop();
        }
    }
    data.extend_from_slice(captured);
    if cpu.bus.disk.write_whole_file(path, &data, None).is_err() {
        crate::video::print_string(cpu, "File creation error\r\n");
    }
}

/// Give a program started from the shell the files its command line
/// redirected: its handle 1 (standard output) and 0 (standard input).
fn redirect_program(cpu: &mut Cpu, redirect: &Redirections) {
    let psp = cpu.current_psp;
    let bus = &mut cpu.bus;
    if let Some((path, append)) = &redirect.output {
        let is_device = crate::disk::char_device(path).is_some();
        if !is_device && (!append || !bus.disk.exists(path)) {
            let _ = bus.disk.write_whole_file(path, &[], None);
        }
        if let Ok(sft) = bus.disk.open_file(path, 0x01, psp) {
            if *append {
                let _ = bus.disk.seek_file(sft, 0, 2);
            }
            crate::dos_files::replace(bus, psp, 1, sft);
        }
    }
    if let Some(path) = &redirect.input
        && let Ok(sft) = bus.disk.open_file(path, 0x00, psp)
    {
        crate::dos_files::replace(bus, psp, 0, sft);
    }
    crate::dos_files::flush(bus);
}

/// Start a program or batch file from the shell with its command line
/// arguments, in upper memory with `high` (LOADHIGH). With no extension we
/// probe .com, .exe, then .bat, matching COMMAND.COM's precedence. A loaded
/// program starts at the CS:IP the loader set. False if there is none.
pub fn run_program(cpu: &mut Cpu, command: &str, args: &str, high: bool) -> bool {
    start(cpu, command, args, high, false).is_some()
}

/// What `start` started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Started {
    Program,
    Batch,
}

/// CALL: run a command line as the shell does, but a batch file on top of
/// the one running, which goes on after it.
pub fn call(cpu: &mut Cpu, line: &str) {
    let (command, args) = split_command(line);
    if command.is_empty() || CommandDispatcher::new().dispatch(cpu, command, args) {
        return;
    }
    if start(cpu, command, args.trim(), false, true).is_none() {
        crate::video::print_string(cpu, "Bad command or file name.\r\n");
    }
}

/// `run_program`, and with `call` a batch file on top of the one running.
fn start(cpu: &mut Cpu, command: &str, args: &str, high: bool, call: bool) -> Option<Started> {
    let path = find_program(cpu, command)?;
    if path.to_ascii_uppercase().ends_with(".BAT") {
        return cpu.start_batch_file(&path, command, args, call).then_some(Started::Batch);
    }
    if let Some(dispatch) = cpu.secondary.as_mut() {
        // A secondary COMMAND.COM runs it with EXEC.
        dispatch.exec = Some((path, args.to_string()));
        return Some(Started::Program);
    }
    load_program(cpu, &path, args, high).then_some(Started::Program)
}

/// Where the program or batch file `command` is, as COMMAND.COM looks for
/// it: where it says when it names a directory or drive, else in the
/// current directory and then in those of PATH, in order. Without an
/// extension it is a .COM, .EXE or .BAT file, the first of them in the
/// first directory that has one; other extensions don't run.
pub fn find_program(cpu: &Cpu, command: &str) -> Option<String> {
    const RUNNABLE: [&str; 3] = [".COM", ".EXE", ".BAT"];
    let name = command.rsplit(['\\', '/', ':']).next().unwrap_or(command);
    let names: Vec<String> = match name.rfind('.') {
        Some(dot) if RUNNABLE.iter().any(|ext| name[dot..].eq_ignore_ascii_case(ext)) => vec![command.to_string()],
        Some(_) => return None,
        None => RUNNABLE.iter().map(|ext| format!("{}{}", command, ext)).collect(),
    };
    let find = |dir: &str| names.iter().map(|n| format!("{}{}", dir, n)).find(|p| cpu.bus.disk.is_file(p));
    if let Some(path) = find("") {
        return Some(path);
    }
    if name.len() != command.len() {
        return None;
    }
    let path = cpu.get_env("PATH").unwrap_or("");
    path.split(';').map(str::trim).filter(|dir| !dir.is_empty()).find_map(|dir| {
        if dir.ends_with(['\\', ':']) { find(dir) } else { find(&format!("{}\\", dir)) }
    })
}

/// Load a program from the shell with its command line arguments.
fn load_program(cpu: &mut Cpu, filename: &str, args: &str, high: bool) -> bool {
    let loaded = if high { cpu.load_executable_high(filename) } else { cpu.load_executable(filename, None) };
    if !loaded {
        return false;
    }
    cpu.set_command_tail(cpu.current_psp, args);
    let ax = crate::interrupts::fcb::set_psp_fcbs(&mut cpu.bus, cpu.current_psp, &crate::dosstr::to_bytes(args));
    cpu.set_ax(ax);
    true
}

/// Where the instruction at CS:EIP is, as `fetch_location` found it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct At {
    pub eip: u32,
    /// Its linear address.
    pub lin_ip: u32,
    pub cs_limit: u32,
    /// A 32-bit code segment.
    pub code32: bool,
    pub phys_ip: usize,
    /// Its bytes are all in RAM at `phys_ip`, so it can be decoded in place
    /// and cached.
    pub cacheable: bool,
}

/// Run one instruction or emulator service trap at CS:IP.
#[inline(always)]
fn instruction<const HOT: bool>(cpu: &mut Cpu, fetch: &mut Fetch, hook: &mut dyn ExecHook) -> Option<StopReason> {
    // This instruction ends the interrupt shadow of the previous one.
    cpu.irq_shadow = false;
    let at = fetch_location(cpu, fetch)?;
    execute_at::<HOT>(cpu, fetch, hook, at)
}

/// Run translated code from CS:EIP, or the instruction there through the
/// interpreter where there is none: outside the code window, in the shell,
/// and in the page of the mouse driver's stub, which lets the next event
/// handler call in by clearing a byte of RAM (see `mouse::callback_busy`):
/// the interpreter looks for one after every instruction. With `single`,
/// translated blocks hold one instruction.
#[inline(always)]
fn dynamic<const HOT: bool>(
    cpu: &mut Cpu,
    fetch: &mut Fetch,
    hook: &mut dyn ExecHook,
    single: bool,
) -> Option<StopReason> {
    let at = fetch_location(cpu, fetch)?;
    if HOT && hook.before_exec(cpu, at.phys_ip, fetch.ram) {
        return Some(StopReason::Paused);
    }
    let translatable = fetch.window.phys(cpu, at.eip, at.lin_ip, at.cs_limit).is_some()
        && at.phys_ip >> 12 != crate::mouse::CALLBACK_STUB >> 12
        && !cpu.in_shell_code();
    if !translatable {
        return execute_at::<false>(cpu, fetch, hook, at);
    }
    // What the code window depends on, before the blocks run.
    let window = (cpu.tlb.epoch, cpu.cpl, cpu.bus.a20_mask());
    match fetch.dynrec.run(cpu, &at, single) {
        Run::Interpret => execute_at::<false>(cpu, fetch, hook, at),
        Run::Ran { page } => {
            fetch.window.moved(page, window);
            finish_instruction(cpu);
            None
        }
        Run::Fault { fault, phys_ip, page } => {
            fetch.window.moved(page, window);
            after_fault(cpu, fault, fetch.ram, phys_ip);
            cpu.bus.clock.icount += 1;
            finish_instruction(cpu);
            None
        }
        Run::Panic(payload) => std::panic::resume_unwind(payload),
    }
}

/// Find the instruction at CS:EIP: in the code window, or else through
/// `locate`, which raises the fault an instruction fetch takes (and returns
/// None, as it does when the tripwire goes off).
#[inline(always)]
fn fetch_location(cpu: &mut Cpu, fetch: &mut Fetch) -> Option<At> {
    let eip = cpu.eip();
    let cs = cpu.seg_cache(Seg::CS);
    let (cs_limit, code32, lin_ip) = (cs.limit, cs.attr & ATTR_DB != 0, cs.base.wrapping_add(eip));
    let (phys_ip, cacheable) = match fetch.window.phys(cpu, eip, lin_ip, cs_limit) {
        Some(phys_ip) => (phys_ip, true),
        None => locate(cpu, fetch, eip, lin_ip, cs_limit)?,
    };
    Some(At { eip, lin_ip, cs_limit, code32, phys_ip, cacheable })
}

/// Run the instruction `fetch_location` found. `HOOK` calls the
/// debugger's `before_exec` first.
#[inline(always)]
fn execute_at<const HOOK: bool>(
    cpu: &mut Cpu,
    fetch: &mut Fetch,
    hook: &mut dyn ExecHook,
    at: At,
) -> Option<StopReason> {
    let At { eip, lin_ip, cs_limit, code32, phys_ip, cacheable } = at;
    if HOOK && hook.before_exec(cpu, phys_ip, fetch.ram) {
        return Some(StopReason::Paused);
    }
    cpu.executed += 1;

    // Decode via the decoded-instruction cache. On a hit (the common case
    // inside hot loops) iced's decoder is skipped entirely. An instruction
    // that may continue on a page that isn't next to its own in physical
    // memory, or runs past the end of RAM, is fetched byte by byte and not
    // cached.
    let slow;
    let (instr, handler) = if cacheable {
        // The generations of the blocks holding the first and last byte
        // an instruction can have: a write to either changes the sum.
        let gens = &cpu.bus.page_gen;
        debug_assert!((phys_ip + 14) >> GEN_SHIFT < gens.len());
        // SAFETY: a cacheable instruction's 16 bytes are all in RAM, and
        // there is a generation for every block of RAM.
        let page_gen = unsafe {
            gens.get_unchecked(phys_ip >> GEN_SHIFT).wrapping_add(*gens.get_unchecked((phys_ip + 14) >> GEN_SHIFT))
        };
        let (decoder16, decoder32) = (&mut fetch.decoder16, &mut fetch.decoder32);
        fetch.cache.get_or_decode(phys_ip, eip, code32, page_gen, |slot| {
            let decoder = if code32 { decoder32 } else { decoder16 };
            decoder.set_position(phys_ip).unwrap();
            decoder.set_ip(eip as u64);
            decoder.decode_out(slot);
        })
    } else {
        match fetch_slow(cpu, lin_ip, eip, code32) {
            Ok(i) => {
                slow = i;
                (&slow, crate::instructions::execute_instruction as Handler)
            }
            Err(fault) => {
                cpu.raise(fault);
                return None;
            }
        }
    };

    let next_eip = eip.wrapping_add(instr.len() as u32);
    if next_eip - 1 > cs_limit {
        // The instruction's last bytes lie past the segment limit.
        cpu.raise(Fault::gp(0));
        return None;
    }
    let start_esp = cpu.esp();
    cpu.set_eip(next_eip);
    if let Err(fault) = handler(cpu, instr) {
        // A fault leaves the instruction undone: EIP back on it, and ESP as
        // it was (the handlers commit everything else last).
        cpu.set_eip(eip);
        cpu.set_esp(start_esp);
        after_fault(cpu, fault, fetch.ram, phys_ip);
    }
    cpu.bus.clock.icount += 1;
    finish_instruction(cpu);
    None
}

/// An instruction at `phys_ip` faulted and was undone: deliver the fault,
/// unless it is the #UD of an emulator service trap, which runs instead.
pub(crate) fn after_fault(cpu: &mut Cpu, fault: Fault, ram: &[u8], phys_ip: usize) {
    if !(fault == Fault::UD && service_trap(cpu, ram, phys_ip)) {
        cpu.raise(fault);
    }
}

/// After an instruction has run (or faulted): carry out a reset the
/// keyboard controller or port 92h asked for, and skip the time a HLT
/// waits.
#[inline(always)]
pub(crate) fn finish_instruction(cpu: &mut Cpu) {
    if cpu.bus.reset_requested {
        // The keyboard controller or port 92h pulsed the reset line.
        cpu.bus.reset_requested = false;
        cpu.bus.log_string("[CPU] Reset requested");
        cpu.reset();
        cpu.bus.refresh_irq();
    }

    if cpu.state != CpuState::Running {
        halt(cpu);
    }
}

/// After an instruction that left the CPU halted or wanting the shell back.
/// HLT: nothing runs until the next interrupt, so skip ahead to the next
/// timer event. With nothing scheduled (a CPU stepped outside a batch) it
/// stays halted until an interrupt arrives.
#[cold]
fn halt(cpu: &mut Cpu) {
    if cpu.state == CpuState::Halted && cpu.bus.clock.deadline != u64::MAX {
        cpu.state = CpuState::Running;
        cpu.bus.clock.skip_to_deadline();
    }
}

/// Where the instruction at `eip` (linear address `lin_ip`) is, when it
/// isn't in the code window: its physical address, and whether its bytes
/// are all in RAM there so it can be decoded in place and cached. Raises
/// the fault an instruction fetch takes and returns None, as it does when
/// the tripwire goes off. Otherwise the page becomes the code window.
#[inline(never)]
fn locate(cpu: &mut Cpu, fetch: &mut Fetch, eip: u32, lin_ip: u32, cs_limit: u32) -> Option<(usize, bool)> {
    // Instruction fetch past the end of the code segment raises #GP(0).
    // (In 16-bit code, EIP runs on past FFFFh rather than wrapping.)
    if eip > cs_limit {
        cpu.raise(Fault::gp(0));
        return None;
    }
    let paging = cpu.cr0 & CR0_PG != 0;
    let phys_ip = if !paging {
        cpu.translate(lin_ip) as usize
    } else {
        let user = cpu.cpl == 3;
        match cpu.lin_to_phys(lin_ip, false, user) {
            Ok(p) => p as usize,
            Err(fault) => {
                cpu.raise(fault);
                return None;
            }
        }
    };

    // Tripwire: arriving in the IVT / BIOS data area with an application
    // context (DS not 0) almost always means a corrupted FAR pointer landed
    // us here.
    if cpu.cs() == 0
        && cpu.ip() < 0x100
        && !cpu.pe()
        && cpu.ds() != 0
        && cpu.ds() != cpu.transient_segment()
        && cpu.current_psp != 0
    {
        report_tripwire(cpu);
        cpu.state = CpuState::RebootShell;
        return None;
    }

    fetch.window.open(cpu, lin_ip, phys_ip, fetch.ram.len());
    // An instruction that starts near the end of a page may continue on the
    // next one. Where that page follows in physical memory too, as it
    // mostly does, its bytes lie together in RAM like any other's.
    let contiguous = !paging || lin_ip & 0xFFF <= 0xFF0 || next_page_follows(cpu, lin_ip, phys_ip);
    Some((phys_ip, contiguous && phys_ip + 16 <= fetch.ram.len()))
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
            cpu.bus.disk_io.clear();
            crate::interrupts::handle_hle(cpu, vector);
            let disk_time = cpu.bus.disk_io.take_pending();
            if cpu.hle_retry {
                cpu.hle_retry = false;
                if vector == 0x16 && crate::shell::abandon_input(cpu) {
                    // Batch lines came while the prompt waited for a key.
                    cpu.idle = false;
                } else {
                    // Stay on the trap, with interrupts on as the BIOS's own
                    // wait loops have them; the caller's flags come back with
                    // its return frame.
                    cpu.set_cpu_flag(CpuFlags::IF, true);
                }
            } else if disk_time > 0 && !(0x08..=0x0F).contains(&vector) && cpu.state == CpuState::Running {
                // Slow disk access: the service returns once it's done.
                crate::diskio::begin_wait(cpu, vector, disk_time);
            } else {
                crate::interrupts::return_from_hle(cpu, vector);
            }
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
    // Services change what may interrupt (INT 33h's mouse event mask).
    cpu.bus.refresh_irq();
    true
}

/// Whether the page after the one `lin_ip` is in maps to the physical page
/// after `phys_ip`'s, so an instruction can run on into it.
fn next_page_follows(cpu: &mut Cpu, lin_ip: u32, phys_ip: usize) -> bool {
    let next = (lin_ip | 0xFFF).wrapping_add(1);
    let user = cpu.cpl == 3;
    // A page that isn't there faults only if the instruction needs bytes
    // from it (`fetch_slow`): looking doesn't change CR2.
    let cr2 = cpu.cr2;
    match cpu.lin_to_phys(next, false, user) {
        Ok(p) => p as usize == (phys_ip | 0xFFF) + 1,
        Err(_) => {
            cpu.cr2 = cr2;
            false
        }
    }
}

/// Decode the instruction at `lin_ip` from bytes fetched one at a time:
/// at the end of RAM, or where the instruction may cross into another
/// page. A page that can't be fetched faults only if the instruction
/// needs bytes from it.
fn fetch_slow(cpu: &mut Cpu, lin_ip: u32, eip: u32, code32: bool) -> Result<Instruction, Fault> {
    let user = cpu.cpl == 3;
    let mut bytes = [0u8; 16];
    let mut len = 0;
    let mut missing = None;
    // Looking at a page the instruction turns out not to need doesn't
    // change CR2.
    let cr2 = cpu.cr2;
    for (i, byte) in bytes.iter_mut().enumerate() {
        let lin = lin_ip.wrapping_add(i as u32);
        match cpu.lin_to_phys(lin, false, user) {
            Ok(p) => *byte = cpu.bus.read_8(p as usize),
            Err(fault) => {
                missing = Some(fault);
                break;
            }
        }
        len += 1;
    }
    let mut decoder = Decoder::with_ip(if code32 { 32 } else { 16 }, &bytes[..len], eip as u64, DecoderOptions::NONE);
    let instr = decoder.decode();
    if decoder.last_error() == iced_x86::DecoderError::NoMoreBytes
        && let Some(fault) = missing
    {
        return Err(fault);
    }
    cpu.cr2 = cr2;
    Ok(instr)
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
