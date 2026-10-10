use bitflags::bitflags;
use iced_x86::MemorySize;
use std::collections::VecDeque;

use crate::bus::Bus;
use crate::f80::F80;
use crate::instr_cache::InstrCache;
use crate::shell::get_shell_code;

pub mod alu;
pub mod fault;
mod farxfer;
pub mod layout;
pub mod mem;
pub mod paging;
mod regs;
pub mod seg;
mod state;
pub mod task;
pub use fault::{CpuResult, Fault, IntSource};
pub use mem::{Access, MemRef};
pub use regs::{ATTR_DB, ATTR_G, Seg, SegCache};
pub use seg::Descriptor;

/// Where the shell runs: its code at SHELL_SEGMENT:0100, its line buffers
/// at 0200 and 0300, and its stack below SHELL_STACK, in the memory between
/// the BIOS data area and DOS's data (`dos_data::SEGMENT`), clear of the
/// interrupt vector table, whose vectors the BIOS and programs write.
pub const SHELL_SEGMENT: u16 = 0x0070;
pub const SHELL_STACK: u16 = 0x0F00;

/// The room an environment keeps for the program's path after it: a
/// DOS path, its count word and its terminator.
const MAX_PATH: usize = 128 + 3;

/// Where a program goes (see `load_executable`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Placement {
    /// Started from the shell: above the resident programs, with all of
    /// conventional memory.
    Shell,
    /// Started by EXEC, into the block allocated for it at this PSP segment.
    Child(u16),
    /// Started from the shell with LOADHIGH, into the upper memory block
    /// allocated for it at this PSP segment.
    High(u16),
}

/// The paragraphs a program needs to start: its PSP, its code, and for a
/// COM file a stack, for an EXE file the memory its header asks for.
fn program_paras(bytes: &[u8]) -> u16 {
    if bytes.len() >= 0x20 && &bytes[0..2] == b"MZ" {
        let word = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]) as usize;
        let (last_page, pages, header, min_alloc) = (word(2), word(4), word(8), word(10));
        let module = match (pages, last_page) {
            (0, _) => bytes.len(),
            (p, 0) => p * 512,
            (p, l) => (p - 1) * 512 + l,
        };
        (0x10 + module.saturating_sub(header * 16).div_ceil(16) + min_alloc).min(0xFFFF) as u16
    } else {
        (0x10 + bytes.len().div_ceil(16) + 0x20).min(0x1000) as u16
    }
}

/// FLAGS bits POPF and IRET load: CF, PF, AF, ZF, SF, TF, IF, DF, OF, IOPL
/// and NT.
const FLAGS16_WRITABLE: u32 = 0x7FD5;

/// The processor being emulated, in the order they came out: a later one
/// has what an earlier one has (`model >= CpuModel::I486`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CpuModel {
    I386,
    /// A 486DX: on-chip FPU, EFLAGS.AC, BSWAP/XADD/CMPXCHG/INVD/WBINVD/
    /// INVLPG, and no CPUID.
    I486,
    /// A Pentium as DOSBox-X has it: the 486's, CPUID (EFLAGS.ID) saying
    /// family 5, model 1, stepping 7 with an FPU, 4 MB pages, the time
    /// stamp counter, MSRs and CMPXCHG8B, and CR4 with its PSE and TSD
    /// bits. It has no virtual-8086 mode extensions.
    Pentium,
    /// A Pentium MMX (P55C): the Pentium's, MMX, and CPUID saying family
    /// 5, model 4, stepping 3.
    PentiumMmx,
}

impl CpuModel {
    /// The EFLAGS bits a program may change beyond the 386's: AC on a 486,
    /// and ID too on a Pentium, whose toggling tells that CPUID is there.
    pub fn eflags_extra(self) -> u32 {
        match self {
            CpuModel::I386 => 0,
            CpuModel::I486 => CpuFlags::AC.bits(),
            CpuModel::Pentium | CpuModel::PentiumMmx => CpuFlags::AC.bits() | CpuFlags::ID.bits(),
        }
    }

    /// The family, model and stepping CPUID reports, which DX also holds
    /// after a reset.
    pub fn signature(self) -> u32 {
        match self {
            CpuModel::I386 => 0x0303,
            CpuModel::I486 => 0x0402,
            CpuModel::Pentium => 0x0517,
            CpuModel::PentiumMmx => 0x0543,
        }
    }

    /// The CPU's name as people know it.
    pub fn describe(self) -> &'static str {
        match self {
            CpuModel::I386 => "386",
            CpuModel::I486 => "486",
            CpuModel::Pentium => "Pentium",
            CpuModel::PentiumMmx => "Pentium MMX",
        }
    }

    /// The most RAM in MB (`memsize`) a machine with this CPU takes, as
    /// the motherboards of its day did: 64 MB on a 386, 128 MB on a 486,
    /// 256 MB on a Pentium (430FX/VX) and 512 MB on a Pentium MMX (430HX).
    pub fn max_memsize(self) -> usize {
        match self {
            CpuModel::I386 => 64,
            CpuModel::I486 => 128,
            CpuModel::Pentium => 256,
            CpuModel::PentiumMmx => 512,
        }
    }

    /// The family: 3, 4 or 5.
    pub fn family(self) -> u8 {
        (self.signature() >> 8) as u8
    }
}

/// Which core runs the instructions (the `core` setting).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CoreMode {
    /// The interpreter until a program switches to protected mode, then
    /// the dynamic recompiler until that program ends, as DOSBox's
    /// `core=auto` does.
    #[default]
    Auto,
    /// The dynamic recompiler throughout.
    Dynamic,
    /// The interpreter throughout.
    Normal,
}

impl CoreMode {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(CoreMode::Auto),
            "dynamic" => Ok(CoreMode::Dynamic),
            "normal" => Ok(CoreMode::Normal),
            _ => Err(format!("invalid core '{}': expected auto, dynamic or normal", value.trim())),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            CoreMode::Auto => "auto",
            CoreMode::Dynamic => "dynamic",
            CoreMode::Normal => "normal",
        }
    }

    /// The core a new CPU starts with: `RUST_DOS_CORE` from the
    /// environment if it names one (the tests run on either core that
    /// way), otherwise the interpreter. The front ends set the configured
    /// one.
    fn initial() -> Self {
        std::env::var("RUST_DOS_CORE").ok().and_then(|v| CoreMode::parse(&v).ok()).unwrap_or(CoreMode::Normal)
    }
}

/// A descriptor table register (GDTR, IDTR): base address and limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DescTable {
    pub base: u32,
    pub limit: u16,
}

/// CR0 bits.
pub const CR0_PE: u32 = 0x0000_0001;
pub const CR0_MP: u32 = 0x0000_0002;
pub const CR0_EM: u32 = 0x0000_0004;
pub const CR0_TS: u32 = 0x0000_0008;
pub const CR0_ET: u32 = 0x0000_0010;
/// 486: numeric errors through #MF rather than IRQ 13.
pub const CR0_NE: u32 = 0x0000_0020;
/// 486: write protection of read-only pages against supervisor code.
pub const CR0_WP: u32 = 0x0001_0000;
/// 486: alignment checks.
pub const CR0_AM: u32 = 0x0004_0000;
/// 486: cache control.
pub const CR0_NW: u32 = 0x2000_0000;
pub const CR0_CD: u32 = 0x4000_0000;
pub const CR0_PG: u32 = 0x8000_0000;

/// CR4 bits (Pentium): RDTSC only at level 0 (TSD), and 4 MB pages (PSE).
pub const CR4_TSD: u32 = 0x0000_0004;
pub const CR4_PSE: u32 = 0x0000_0010;

// FPU Tag Word Values
pub const FPU_TAG_EMPTY: u8 = 1;
pub const FPU_TAG_VALID: u8 = 0;
/// The real indefinite as a double (`F80::get_f64` of it): what an empty
/// register reads as.
pub const FPU_INDEFINITE_F64: f64 = f64::from_bits(0xFFF8_0000_0000_0000);

// Constants for Flag Bits
bitflags! {
    /// EFLAGS. Transparent, so the dynamic recompiler's code reads and
    /// writes it as the u32 it is.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(transparent)]
    pub struct CpuFlags: u32 {
        const CF = 0x0001;
        /// Bit 1 reads as 1 on every x86.
        const R1 = 0x0002;
        const PF = 0x0004;
        const AF = 0x0010;
        const ZF = 0x0040;
        const SF = 0x0080;
        const DF = 0x0400; // Bit 10
        const IF = 0x0200;
        const TF = 0x0100;
        const OF = 0x0800;
        /// I/O privilege level (bits 12-13), nested task, and the 386
        /// EFLAGS bits: resume, virtual-8086 mode, alignment check (486)
        /// and the CPUID bit (Pentium).
        const IOPL = 0x3000;
        const NT = 0x4000;
        const RF = 0x0001_0000;
        const VM = 0x0002_0000;
        const AC = 0x0004_0000;
        const ID = 0x0020_0000;
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct FpuFlags: u16 {
        // Condition Codes
        const C0 = 0x0100;
        const C1 = 0x0200;
        const C2 = 0x0400; // Bit 10
        const C3 = 0x4000; // Bit 14

        // Exception Flags (Bits 0-5)
        const IE = 0x0001; // Invalid Operation
        const DE = 0x0002; // Denormalized Operand
        const ZE = 0x0004; // Zero Divide
        const OE = 0x0008; // Overflow
        const UE = 0x0010; // Underflow
        const PE = 0x0020; // Precision

        // Status Bits
        const SF = 0x0040; // Stack Fault
        const ES = 0x0080; // Error Summary Status
        const B  = 0x8000; // Busy bit

        // A helper group for FNCLEX
        const EXCEPTIONS = Self::IE.bits() | Self::DE.bits() | Self::ZE.bits() |
                           Self::OE.bits() | Self::UE.bits() | Self::PE.bits() |
                           Self::SF.bits() | Self::ES.bits() | Self::B.bits();
    }
}

/// In declared order (`repr(C)`), so that the fields translated code uses
/// most come first, together: the AArch64 recompiler reaches them with one
/// base register and a 12-bit offset (`dynrec::a64`), which the compiler's
/// own order of the fields didn't keep on every target.
#[repr(C)]
pub struct Cpu {
    /// EAX, ECX, EDX, EBX, ESP, EBP, ESI, EDI (see `regs.rs` accessors).
    gpr: [u32; 8],
    eip: u32,
    flags: CpuFlags,
    /// ES, CS, SS, DS, FS, GS.
    seg: [SegCache; 6],
    /// Current privilege level: 0 in real mode, 3 in virtual-8086 mode,
    /// in protected mode that of the code segment.
    pub cpl: u8,
    /// Instructions (including emulator service traps) run since start, not
    /// counting interrupt entries or time skipped while halted.
    pub executed: u64,
    // FPU State
    fpu_stack: crate::f80::FpuRegs,
    pub fpu_top: usize,
    fpu_flags: FpuFlags,
    pub fpu_control: u16,
    pub fpu_tags: [u8; 8],

    pub model: CpuModel,
    pub cr0: u32,
    pub cr2: u32,
    pub cr3: u32,
    /// CR4 (Pentium): `CR4_TSD` and `CR4_PSE`.
    pub cr4: u32,
    /// What the time stamp counter (Pentium) adds to the instruction
    /// clock, which it counts (see `tsc`).
    pub tsc_offset: u64,
    /// The Pentium's performance monitoring MSRs: the control and event
    /// select (11h) and the two counters (12h, 13h), which hold what was
    /// written to them.
    pub perf_msrs: [u64; 3],
    /// Debug registers; stored, but breakpoints are not implemented.
    pub dr: [u32; 8],
    pub gdtr: DescTable,
    pub idtr: DescTable,
    /// The local descriptor table and task register: selector and
    /// descriptor cache.
    pub ldtr: SegCache,
    pub tr: SegCache,
    /// Page translations, see `paging.rs`.
    pub tlb: paging::Tlb,

    pub bus: Bus,
    pub state: CpuState,
    pub pending_command: Option<String>,
    /// The lines typed at the prompt, for Up and Down. They stay while
    /// programs run and the shell is loaded again.
    pub shell_history: crate::cmdline::history::ShellHistory,
    /// Where Tab left the line at the prompt, for Tab again.
    pub shell_completion: Option<crate::cmdline::complete::Completion>,
    /// The line being typed at the prompt.
    pub line_editor: Option<crate::cmdline::LineEditor>,
    /// How the prompt suggests, colours and keeps its history.
    pub shell_settings: crate::cmdline::settings::ShellSettings,
    /// What PAUSE or CHOICE waits for; batch lines wait with it.
    pub shell_wait: Option<crate::shell::ShellWait>,
    /// Where the prompt was printed, (column, row), while a line is typed
    /// after it.
    pub shell_prompt_at: Option<(u8, u8)>,
    /// What EDIT cut or copied, for pasting in this EDIT or the next.
    pub edit_clipboard: Vec<u8>,
    /// How the session started, for a reboot to start it again.
    pub startup: crate::boot::Startup,
    /// Set by a reset of the built-in DOS (`bios::post`) with the state
    /// RebootShell: the machine reboots instead of only the shell
    /// starting again.
    pub reboot: bool,
    /// The secondary COMMAND.COMs running, the innermost last.
    pub secondary_shells: Vec<crate::command_com::SecondaryShell>,
    /// Set while a command line of a secondary COMMAND.COM runs: what it
    /// asks the shell's program to do.
    pub secondary: Option<crate::command_com::Dispatch>,
    /// What a built-in command prints while its output is redirected
    /// (`>file`), instead of the screen.
    pub stdout_capture: Option<Vec<u8>>,
    /// What a built-in command reads while its input is redirected
    /// (`<file`, or a pipe); None for the keyboard.
    pub stdin_redirect: Option<Vec<u8>>,
    /// The batch files running, whose lines are dispatched as if typed
    /// at the prompt while the shell is idle (no child program on the
    /// process_stack and CS still in shell-land), and ECHO.
    pub batch: crate::batch::Batch,
    /// The master environment (SET, PATH), in order. Programs started from
    /// the shell get a copy.
    pub environment: Vec<(String, String)>,
    /// The variables the settings keep in the environment.
    pub env_injector: crate::env_inject::EnvInjector,
    pub current_psp: u16,
    pub heap_pointer: u16,
    /// MCB segment where memory above the TSRs kept resident from the shell
    /// begins; the first MCB when there are none. Programs started from
    /// the shell load right above it.
    pub resident_end: u16,
    /// The PSPs of the TSRs kept resident in upper memory (LOADHIGH).
    pub resident_upper: Vec<u16>,
    /// Exit code (AL) and termination type (AH) of the most recently terminated
    /// child process. Read-and-clear by INT 21h AH=4Dh. Termination type:
    /// 0 = normal (INT 21 AH=4C), 1 = Ctrl-C, 2 = critical error, 3 = TSR.
    pub last_child_exit: u16,
    /// The exit code of the last program started from the shell, which
    /// IF ERRORLEVEL tests.
    pub errorlevel: u8,
    /// The program running, as DOS started it (`KEEN4E.EXE`), for what a
    /// save state says it was saved in; empty at the prompt.
    pub program: String,
    /// Error code of the last failed DOS call, for INT 21h AH=59h.
    pub last_dos_error: u16,
    /// Scan code of an extended key whose 00h the console functions of
    /// INT 21h have returned, for the next read.
    pub con_pending_scan: Option<u8>,
    /// The line being typed for INT 21h AH=0Ah or a read of CON.
    pub con_line: Option<crate::interrupts::int21::ConLine>,
    /// What is left of the last line read from CON, for the next reads.
    pub con_pending: VecDeque<u8>,
    /// Memory allocation strategy (INT 21h AH=58h).
    pub alloc_strategy: u16,
    /// End of an INT 15h AH=86h wait in progress, in PIT ticks.
    pub bios_wait_until: Option<u64>,

    pub process_stack: Vec<ProcessContext>,
    /// How many programs have been loaded, for telling whether a game's
    /// commands started one (`games::ActiveGame::done`).
    pub programs_loaded: u64,
    /// The programs started and ended, for the debugger.
    pub programs: ProgramEvents,
    /// Set by STI, MOV SS and POP SS: hardware interrupts wait until the
    /// next instruction has run.
    pub irq_shadow: bool,
    /// Decoded instructions, see `instr_cache.rs`.
    pub decode_cache: InstrCache,
    /// Vectors that software interrupts found at 0000:0000, logged once each.
    null_interrupts: [u64; 4],
    /// Switches between real and protected mode (CR0.PE changes).
    pub mode_switches: u64,
    /// Exceptions raised since start, and the most recent ones.
    pub exceptions: u64,
    pub exception_log: VecDeque<fault::ExceptionRecord>,
    /// Set by BIOS services that wait for input (INT 16h with an empty
    /// keyboard buffer). The main loop then skips ahead to the next timer
    /// event instead of spinning through the retry loop, like it does for HLT.
    pub idle: bool,
    /// Set by a BIOS or DOS service that has to wait (for a keystroke): the
    /// service trap runs again instead of returning to the caller.
    pub hle_retry: bool,
    /// The core that runs instructions, see `dynamic_active`.
    pub core: CoreMode,
    /// A program switched to protected mode since it started: `core=auto`
    /// runs it on the dynamic recompiler and `cycles=auto` at max speed
    /// until it ends.
    pub pm_latched: bool,
    /// REP MOVS and STOS do the iterations that stay in plain RAM at once
    /// (`instructions::string`). Off, they go one at a time, which tests
    /// compare them with.
    pub string_bulk: bool,
    /// The FPU adds and subtracts doubles as the host does, not 80 bits
    /// (`fpu=fast`, not exact); set with `set_fpu_fast`.
    pub fpu_fast: bool,
    /// Data segment loads in protected mode that went through, to load the
    /// same selectors again quickly (see `seg::SegLoad`).
    pub(crate) seg_loads: [seg::SegLoad; seg::SEG_LOADS],
    /// The dynamic recompiler's translated code, see `dynrec`.
    pub dynrec: crate::dynrec::DynState,
}

#[derive(PartialEq, Debug)]
#[allow(dead_code)]
pub enum CpuState {
    Running,
    Halted,
    RebootShell,
}

/// The architectural register state: general-purpose registers, EIP,
/// EFLAGS and the segment registers with their descriptor caches.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CpuSnapshot {
    pub gpr: [u32; 8],
    pub eip: u32,
    pub flags: CpuFlags,
    pub seg: [SegCache; 6],
}

/// The FPU's state as `Cpu::idle_key` compares it: the registers' bits.
#[derive(Clone, PartialEq)]
pub struct FpuKey {
    pub regs: crate::f80::FpuBits,
    pub top: usize,
    pub control: u16,
    pub status: u16,
    pub tags: [u8; 8],
}

/// The programs DOS started and ended since start, for the debugger to
/// stop at (`debug::DebugHub`).
#[derive(Debug, Clone, Default)]
pub struct ProgramEvents {
    /// Programs loaded and about to run, and where the last one starts
    /// (the physical address of its entry point), its name and its PSP.
    pub started: u64,
    pub entry: usize,
    pub entry_cs_ip: (u16, u16),
    pub name: String,
    pub psp: u16,
    /// Programs that ended, and how the last one did.
    pub ended: u64,
    pub last_exit: Option<ProgramExit>,
    /// Programs started and not yet ended: the ones a shell reload ends
    /// without an exit of their own (`Cpu::note_abort`).
    pub running: u32,
}

/// How a program ended.
#[derive(Debug, Clone, PartialEq)]
pub struct ProgramExit {
    pub name: String,
    /// The exit code (AL of INT 21h AH=4Ch or 31h; 0 for INT 20h), or
    /// None for a program the emulator ended (`Cpu::note_abort`).
    pub code: Option<u8>,
    /// It stayed resident (INT 21h AH=31h).
    pub resident: bool,
}

/// A parent process's state while its child runs (INT 21h AH=4Bh).
#[derive(Debug, Clone)]
pub struct ProcessContext {
    pub regs: CpuSnapshot,
    pub psp: u16,
    /// The PSP of the process EXEC started in its place, which returns
    /// here when it ends (0 before it is loaded), and where that PSP is in
    /// physical memory (`Cpu::psp_address`): the processes of Windows'
    /// virtual machines can have their PSPs at the same segment.
    pub child: u16,
    pub child_at: u32,
    pub heap_pointer: u16,
    /// The parent's DTA (segment, offset), which the child's replaces.
    pub dta: (u16, u16),
    /// The parent's name (`Cpu::program`).
    pub program: String,
}

use std::path::PathBuf;

/// The environment at startup. BLASTER, ULTRASND and ULTRADIR advertise
/// the sound cards' resources as SET lines in AUTOEXEC.BAT would (the
/// configuration can change the cards).
fn default_environment() -> Vec<(String, String)> {
    let gus = crate::gus::GusConfig::default();
    vec![
        // Z: holds COMMAND.COM, as in DOSBox.
        ("PATH".to_string(), "C:\\;Z:\\".to_string()),
        ("COMSPEC".to_string(), "Z:\\COMMAND.COM".to_string()),
        ("BLASTER".to_string(), crate::sb::SbConfig::default().blaster()),
        ("ULTRASND".to_string(), gus.ultrasnd()),
        ("ULTRADIR".to_string(), gus.ultradir()),
    ]
}

impl Cpu {
    /// A CPU on a machine with the default amount of RAM.
    pub fn new(root_path: PathBuf) -> Self {
        Self::with_bus(Bus::new(root_path))
    }

    /// A CPU on a machine with `memory_mb` MB of RAM.
    pub fn with_memory(root_path: PathBuf, memory_mb: usize) -> Self {
        Self::with_bus(Bus::with_memory(root_path, memory_mb))
    }

    fn with_bus(bus: Bus) -> Self {
        let resident_end = crate::mcb::first_mcb(&bus);
        Self {
            gpr: [0; 8],
            eip: 0x100,
            seg: [SegCache::real(0); 6],
            model: CpuModel::I486,
            cr0: CR0_ET,
            cr2: 0,
            cr3: 0,
            cr4: 0,
            tsc_offset: 0,
            perf_msrs: [0; 3],
            dr: [0; 8],
            gdtr: DescTable { base: 0, limit: 0xFFFF },
            idtr: DescTable { base: 0, limit: 0x3FF },
            ldtr: SegCache::null(0),
            tr: SegCache::null(0),
            cpl: 0,
            tlb: paging::Tlb::default(),
            bus,
            flags: CpuFlags::from_bits_truncate(0x0202), // Default Flag State: bit 1 reserved, IF=1
            state: CpuState::Running,
            pending_command: None,
            shell_history: Default::default(),
            shell_completion: None,
            line_editor: None,
            shell_settings: Default::default(),
            shell_wait: None,
            shell_prompt_at: None,
            edit_clipboard: Vec::new(),
            startup: crate::boot::Startup::default(),
            reboot: false,
            secondary_shells: Vec::new(),
            secondary: None,
            stdout_capture: None,
            stdin_redirect: None,
            batch: crate::batch::Batch::default(),
            environment: default_environment(),
            env_injector: Default::default(),
            fpu_stack: crate::f80::FpuRegs::default(),
            fpu_top: 0,
            fpu_flags: FpuFlags::from_bits_truncate(0x0000),
            fpu_control: 0x037F, // Default Control Word
            fpu_tags: [FPU_TAG_EMPTY; 8],
            current_psp: 0, // Will be set by loader
            heap_pointer: 0x2000,
            resident_end,
            resident_upper: Vec::new(),
            last_child_exit: 0,
            errorlevel: 0,
            program: String::new(),
            last_dos_error: 0,
            con_pending_scan: None,
            con_line: None,
            con_pending: VecDeque::new(),
            alloc_strategy: 0,
            bios_wait_until: None,
            process_stack: Vec::new(),
            programs_loaded: 0,
            programs: ProgramEvents::default(),
            irq_shadow: false,
            // 64K direct-mapped slots (~3.5 MB): comfortably large for any
            // DOS program's hot working set.
            decode_cache: InstrCache::new(16),
            executed: 0,
            null_interrupts: [0; 4],
            mode_switches: 0,
            exceptions: 0,
            exception_log: VecDeque::with_capacity(fault::EXCEPTION_LOG_LEN),
            idle: false,
            hle_retry: false,
            core: CoreMode::initial(),
            pm_latched: false,
            string_bulk: true,
            // (`RUST_DOS_FPU=fast` from the environment, as `RUST_DOS_CORE`:
            // the front ends set the configured one.)
            fpu_fast: std::env::var("RUST_DOS_FPU").is_ok_and(|v| v.eq_ignore_ascii_case("fast")),
            seg_loads: [seg::SegLoad::NONE; seg::SEG_LOADS],
            dynrec: crate::dynrec::DynState::default(),
        }
    }

    /// Whether the dynamic recompiler runs the instructions now: always
    /// with `core=dynamic`, and with `core=auto` once the running program
    /// has switched to protected mode. Never on a host it has no code
    /// generator for.
    #[inline(always)]
    pub fn dynamic_active(&self) -> bool {
        crate::dynrec::AVAILABLE
            && match self.core {
                CoreMode::Dynamic => true,
                CoreMode::Auto => self.pm_latched,
                CoreMode::Normal => false,
            }
    }

    /// Switch the FPU between exact and fast arithmetic (`fpu_fast`). The
    /// recompiler's code was translated for one of them, so it goes.
    pub fn set_fpu_fast(&mut self, fast: bool) {
        if fast != self.fpu_fast {
            self.fpu_fast = fast;
            self.dynrec.flush();
        }
    }

    /// A software interrupt found its vector at 0000:0000 and was skipped.
    /// Logged the first time for each vector.
    pub fn note_null_interrupt(&mut self, vector: u8) {
        let (word, bit) = (vector as usize / 64, 1u64 << (vector % 64));
        if self.null_interrupts[word] & bit == 0 {
            self.null_interrupts[word] |= bit;
            self.bus.log_string(&format!(
                "[CPU] INT {:02X}h has no handler (vector 0000:0000), skipped",
                vector
            ));
        }
    }

    /// CR0.PE changed. The first switches are logged; DOS extenders then
    /// switch for every DOS call and interrupt.
    pub fn note_mode_switch(&mut self, protected: bool) {
        self.mode_switches += 1;
        if protected {
            self.pm_latched = true;
        }
        if self.mode_switches <= 4 {
            self.bus.log_string(&format!(
                "[CPU] {} mode at {:04X}:{:08X}",
                if protected { "Protected" } else { "Real" },
                self.cs(),
                self.eip()
            ));
        }
    }

    /// A BIOS or DOS service can't finish yet (no keystroke): run it again
    /// when the CPU gets back to it, after the interrupts that could bring
    /// what it waits for. The caller's return frame stays on the stack, so
    /// this works however the service was called (INT, or a far call with
    /// the flags pushed, as DOS extenders and TSRs chain interrupts).
    pub fn hle_wait(&mut self) {
        self.hle_retry = true;
        self.idle = true;
    }

    /// PSP segment for programs started from the shell: right above the
    /// resident TSRs.
    pub fn transient_segment(&self) -> u16 {
        self.resident_end + 1
    }

    /// INT 21h AH=31h from a program started by the shell: shrink its PSP
    /// block to `paras` and keep it, plus any other block it owns, resident
    /// under the programs started afterwards.
    pub fn keep_resident(&mut self, psp: u16, paras: u16) {
        // DOS keeps at least the 6 paragraphs of the PSP itself.
        let _ = crate::mcb::resize(&mut self.bus, psp, paras.max(6));
        // Loaded high: it stays in its upper memory block.
        let cover = crate::mcb::umb_cover_seg(&self.bus);
        if psp > cover {
            self.resident_upper.push(psp);
            self.bus.xms.keep_resident();
            self.bus.log_string(&format!("[DOS] TSR: resident in upper memory at {:04X}", psp));
            return;
        }
        // The last block of conventional memory in use.
        let chain = crate::mcb::walk(&mut self.bus);
        let end = chain
            .iter()
            .take_while(|(s, _)| *s < cover)
            .filter(|(_, m)| !m.is_free())
            .last()
            .map_or(self.resident_end, |&(s, m)| {
                s.saturating_add(1).saturating_add(m.size)
            });
        if end >= crate::mcb::low_end(&self.bus) {
            self.bus
                .log_string("[DOS] TSR: no memory left above it, not keeping it resident");
            return;
        }
        self.resident_end = end;
        self.bus.xms.keep_resident();
        self.bus.log_string(&format!(
            "[DOS] TSR: resident up to {:04X}, programs now load at {:04X}",
            end,
            self.transient_segment()
        ));
    }

    /// Save the architectural register state.
    pub fn snapshot(&self) -> CpuSnapshot {
        CpuSnapshot {
            gpr: self.gpr,
            eip: self.eip,
            flags: self.flags,
            seg: self.seg,
        }
    }

    /// Everything of the CPU's an instruction can see, which `idle`
    /// compares between passes of a loop.
    pub fn idle_key(&self) -> crate::idle::CpuKey {
        crate::idle::CpuKey {
            regs: self.snapshot(),
            cr: [self.cr0, self.cr2, self.cr3, self.cr4],
            dr: self.dr,
            cpl: self.cpl,
            a20: self.bus.a20_mask(),
            tables: [self.gdtr, self.idtr],
            system: [self.ldtr, self.tr],
            fpu: FpuKey {
                regs: self.fpu_stack.bits(),
                top: self.fpu_top,
                control: self.fpu_control,
                status: self.fpu_flags.bits(),
                tags: self.fpu_tags,
            },
            shadow: self.irq_shadow,
            tainted: false,
        }
    }

    /// Put back a register state saved by `snapshot`.
    pub fn restore(&mut self, regs: &CpuSnapshot) {
        self.gpr = regs.gpr;
        self.eip = regs.eip;
        self.flags = regs.flags;
        self.seg = regs.seg;
    }

    pub fn save_process_context(&mut self) {
        let context = ProcessContext {
            regs: self.snapshot(),
            psp: self.current_psp,
            child: 0,
            child_at: 0,
            heap_pointer: self.heap_pointer,
            dta: (self.bus.dta_segment, self.bus.dta_offset),
            program: self.program.clone(),
        };
        self.process_stack.push(context);
        self.bus.log_string(&format!(
            "[CPU] Context Saved. Stack Depth: {}",
            self.process_stack.len()
        ));
    }

    /// End the running program with the exit code `code` (INT 21h AH=4Ch,
    /// and 0 for INT 20h and AH=00h): its memory is freed and its files
    /// closed, and the code kept for its parent (AH=4Dh), or as the
    /// ERRORLEVEL when the shell started it, which is then loaded again.
    /// Returns whether it went back to a parent.
    pub fn terminate(&mut self, code: u8) -> bool {
        self.last_child_exit = code as u16;
        self.note_exit(code, false);
        let psp = self.current_psp;
        // Its DPMI clients end with it, and its IPX sockets.
        crate::dpmi::process_ended(self, psp);
        if let Some(ipx) = &mut self.bus.net.ipx {
            ipx.program_ended(psp);
        }
        if !self.started_by_exec(psp) {
            self.end_made_process(psp);
            return true;
        }
        crate::dos_files::close_all(&mut self.bus, psp);
        crate::mcb::free_owned_by(&mut self.bus, psp);
        if self.return_to_parent() {
            return true;
        }
        self.errorlevel = code;
        self.state = CpuState::RebootShell;
        false
    }

    /// Count the running program as ended with exit code `code`, staying
    /// `resident` or not, for the debugger.
    pub fn note_exit(&mut self, code: u8, resident: bool) {
        let name = self.program.clone();
        let events = &mut self.programs;
        events.ended += 1;
        events.running = events.running.saturating_sub(1);
        events.last_exit = Some(ProgramExit { name, code: Some(code), resident });
    }

    /// Count the programs still running as ended, if any are, when the
    /// shell is loaded again without them having exited: a divide
    /// overflow, the tripwire, a DPMI client that can't go on, the
    /// debugger's or a frontend's close, a reboot. One exit is recorded,
    /// for the program that ran last, with no exit code.
    fn note_abort(&mut self) {
        let name = self.program.clone();
        let events = &mut self.programs;
        if events.running == 0 {
            return;
        }
        events.running = 0;
        events.ended += 1;
        events.last_exit = Some(ProgramExit { name, code: None, resident: false });
    }

    /// Stop the running program for the shell to be loaded again, with
    /// the commands queued for it dropped. `load_shell` frees its memory,
    /// files, vectors and extended memory, as for a program that crashed.
    pub fn close_program(&mut self) {
        self.batch.clear();
        self.pending_command = None;
        self.shell_wait = None;
        self.bus.disk.reset_current_directories();
        // A program closed in the middle may have left the Voodoo card
        // driving the monitor, or the PowerVR one rendering.
        self.bus.reset_voodoo();
        self.bus.reset_powervr();
        self.state = CpuState::RebootShell;
    }

    /// Where the PSP at segment `psp` is in physical memory, through the
    /// page tables of the machine that runs: in Windows' 386 enhanced mode
    /// the processes of different virtual machines can have their PSPs at
    /// the same segment, in memory of their own.
    pub fn psp_address(&self, psp: u16) -> u32 {
        let lin = psp as u32 * 16;
        self.peek_translate(lin).unwrap_or(lin)
    }

    /// The context EXEC kept for the parent of the process `psp`: its
    /// place in `process_stack`. The latest, as under Windows the
    /// processes of other virtual machines can start and end in between.
    pub fn exec_context(&self, psp: u16) -> Option<usize> {
        let at = self.psp_address(psp);
        self.process_stack.iter().rposition(|c| c.child == psp && c.child_at == at)
    }

    /// Whether the process `psp` is one EXEC or the shell started, whose
    /// parent's context the emulator keeps (or that goes back to the
    /// shell), rather than a PSP a program made itself (INT 21h AH=26h or
    /// 55h), as Windows does its tasks'.
    pub fn started_by_exec(&mut self, psp: u16) -> bool {
        if self.exec_context(psp).is_some() {
            return true;
        }
        let parent = self.bus.guest_read_16(psp as u32 * 16 + 0x16);
        psp == 0 || parent == 0 || parent == psp
    }

    /// End the process `psp`, a PSP a program made, as DOS ends any: its
    /// files closed and its memory freed, the INT 22h, 23h and 24h vectors
    /// it keeps put back, and its parent (PSP 16h) the current process
    /// again, back on the stack of the parent's last INT 21h call (PSP 2Eh)
    /// with the registers it saved there, returning to the ended process's
    /// terminate address (PSP 0Ah) with them. Windows' tasks end so, into
    /// its DOS extender.
    fn end_made_process(&mut self, psp: u16) {
        let base = psp as u32 * 16;
        crate::dos_files::close_all(&mut self.bus, psp);
        crate::mcb::free_owned_by(&mut self.bus, psp);
        for (i, vector) in [0x22u32, 0x23, 0x24].into_iter().enumerate() {
            let handler = self.bus.guest_read_32(base + 0x0A + 4 * i as u32);
            self.bus.guest_write_32(vector * 4, handler);
        }
        let (terminate_ip, terminate_cs) = (self.bus.guest_read_16(base + 0x0A), self.bus.guest_read_16(base + 0x0C));
        let parent = self.bus.guest_read_16(base + 0x16);
        self.current_psp = parent;
        let parent_base = parent as u32 * 16;
        let (sp, ss) = (self.bus.guest_read_16(parent_base + 0x2E), self.bus.guest_read_16(parent_base + 0x30));
        let at = |i: u16| ss as u32 * 16 + sp.wrapping_add(2 * i) as u32;
        let saved: Vec<u16> = (0..9).map(|i| self.bus.guest_read_16(at(i))).collect();
        self.set_ax(saved[0]);
        self.set_bx(saved[1]);
        self.set_cx(saved[2]);
        self.set_dx(saved[3]);
        self.set_si(saved[4]);
        self.set_di(saved[5]);
        self.set_bp(saved[6]);
        self.set_ds(saved[7]);
        self.set_es(saved[8]);
        self.set_ss(ss);
        self.set_sp(sp.wrapping_add(18));
        // The INT 21h returns to the terminate address.
        self.bus.guest_write_16(at(9), terminate_ip);
        self.bus.guest_write_16(at(10), terminate_cs);
        self.bus.log_string(&format!(
            "[DOS] Process {:04X} ended: back to {:04X} at {:04X}:{:04X}",
            psp, parent, terminate_cs, terminate_ip
        ));
    }

    /// End the current process: back to the parent's context, returning
    /// through the terminate address in the process's PSP (0Ah), which
    /// EXEC set to the parent's return address and debuggers change. The
    /// parent's stack holds the interrupt frame the return pops.
    pub fn return_to_parent(&mut self) -> bool {
        let psp = self.current_psp;
        let base = psp as u32 * 16;
        let terminate = (self.bus.guest_read_16(base + 0x0A), self.bus.guest_read_16(base + 0x0C));
        let Some(index) = self.exec_context(psp) else {
            self.bus.log_string(if self.process_stack.is_empty() {
                "[CPU] Restore Failed: Stack Empty"
            } else {
                "[CPU] Restore Failed: no parent's context for the process"
            });
            return false;
        };
        // Contexts kept after it are those of other virtual machines'
        // processes, which go on.
        let context = self.process_stack.remove(index);
        self.restore_context(context);
        if terminate != (0, 0) {
            let frame = self.get_physical_addr(self.ss(), self.sp()) as u32;
            self.bus.guest_write_16(frame, terminate.0);
            self.bus.guest_write_16(frame + 2, terminate.1);
        }
        true
    }

    pub fn restore_process_context(&mut self) -> bool {
        if let Some(context) = self.process_stack.pop() {
            self.restore_context(context);
            true
        } else {
            self.bus.log_string("[CPU] Restore Failed: Stack Empty");
            false
        }
    }

    /// Back to the parent's context `context`, taken off `process_stack`.
    fn restore_context(&mut self, context: ProcessContext) {
        // The program ended: `core=auto` goes back to the interpreter and
        // `cycles=auto` to the real-mode speed.
        self.pm_latched = false;
        self.restore(&context.regs);
        self.current_psp = context.psp;
        self.heap_pointer = context.heap_pointer; // Restore heap specifically for that process? Maybe not... but safer.
        (self.bus.dta_segment, self.bus.dta_offset) = context.dta;
        self.program = context.program;
        self.bus.log_string(&format!(
            "[CPU] Context Restored. Stack Depth: {}",
            self.process_stack.len()
        ));
    }

    // Helper to get a flag state
    pub fn get_cpu_flag(&self, mask: CpuFlags) -> bool {
        (self.flags & mask) != CpuFlags::empty()
    }

    // Helper to set/clear a flag
    pub fn set_cpu_flag(&mut self, mask: CpuFlags, value: bool) {
        if value {
            self.flags.insert(mask);
        } else {
            self.flags.remove(mask);
        }
    }

    /// Load the 16-bit FLAGS register, as POPF and IRET do in real mode:
    /// the status and control flags, IOPL and NT. Bit 15 stays 0 and bit 1
    /// stays 1, which is how programs tell a 386 from an 8086 or 286.
    pub fn set_cpu_flags(&mut self, new_flags: CpuFlags) {
        self.load_flags16(new_flags.bits() as u16);
    }

    /// See `set_cpu_flags`.
    pub fn load_flags16(&mut self, value: u16) {
        let upper = self.flags.bits() & 0xFFFF_0000;
        self.flags = CpuFlags::from_bits_retain(upper | (value as u32 & FLAGS16_WRITABLE) | 0x0002);
    }

    /// Load EFLAGS, as POPFD and IRETD do in real mode. VM and RF can't be
    /// set this way; AC only exists from the 486 on, ID on a Pentium.
    pub fn load_eflags(&mut self, value: u32) {
        let writable = FLAGS16_WRITABLE | self.model.eflags_extra();
        let keep = self.flags.bits() & CpuFlags::VM.bits();
        self.flags = CpuFlags::from_bits_retain(keep | (value & writable) | 0x0002);
    }

    /// The time stamp counter (Pentium): the instruction clock, which runs
    /// at the `cycles` speed, as DOSBox-X's counts its cycles, and goes on
    /// while the processor waits for an interrupt.
    pub fn tsc(&self) -> u64 {
        self.bus.clock.icount.wrapping_add(self.tsc_offset)
    }

    /// Set the time stamp counter (WRMSR 10h).
    pub fn set_tsc(&mut self, value: u64) {
        self.tsc_offset = value.wrapping_sub(self.bus.clock.icount);
    }

    /// EFLAGS as PUSHFD pushes it: VM and RF read as 0.
    pub fn eflags_image(&self) -> u32 {
        self.flags.bits() & !(CpuFlags::VM.bits() | CpuFlags::RF.bits())
    }

    pub fn get_cpu_flags(&self) -> CpuFlags {
        self.flags
    }

    /// The low 16 bits of the flags register, as PUSHF and an interrupt
    /// push them in 16-bit code.
    pub fn flags16(&self) -> u16 {
        self.flags.bits() as u16
    }

    pub fn set_fpu_flag(&mut self, flag: FpuFlags, value: bool) {
        if value {
            self.fpu_flags.insert(flag);
        } else {
            self.fpu_flags.remove(flag);
        }
    }

    #[allow(dead_code)]
    pub fn get_fpu_flag(&self, flag: FpuFlags) -> bool {
        self.fpu_flags.contains(flag)
    }

    pub fn set_fpu_flags(&mut self, new_flags: FpuFlags) {
        // Removed top pointer extraction, as we store it separately.
        //        let bits = new_flags.bits();
        //        // Bits 11, 12, 13 are the TOP pointer (0-7)
        //        self.fpu_top = ((bits >> 11) & 0x07) as usize;

        // Store the flags
        self.fpu_flags = new_flags;
    }

    pub fn get_fpu_flags(&self) -> FpuFlags {
        self.fpu_flags
    }

    #[allow(dead_code)]
    pub fn zflag(&self) -> bool {
        self.get_cpu_flag(CpuFlags::ZF)
    }

    #[allow(dead_code)]
    pub fn set_zflag(&mut self, val: bool) {
        self.set_cpu_flag(CpuFlags::ZF, val)
    }

    pub fn dflag(&self) -> bool {
        self.get_cpu_flag(CpuFlags::DF)
    }
    pub fn set_dflag(&mut self, val: bool) {
        self.set_cpu_flag(CpuFlags::DF, val)
    }

    /// The linear address of `segment:offset` in real or virtual-8086
    /// mode, where a BIOS service's caller has its data, for the
    /// `Bus::guest_*` accessors.
    pub fn real_linear(&self, segment: u16, offset: u16) -> u32 {
        ((segment as u32) << 4) + offset as u32
    }

    // Calculate Physical Address from Segment:Offset
    pub fn get_physical_addr(&self, segment: u16, offset: u16) -> usize {
        let addr = ((segment as usize) << 4) + offset as usize;
        // Without the A20 gate, FFFF:0010 and up wrap to the bottom of
        // memory as on an 8086.
        addr & self.bus.a20_mask() as usize
    }

    /// Extract Low byte of DX (DL)
    pub fn get_dl(&self) -> u8 {
        (self.dx() & 0xFF) as u8
    }

    /// Set Low byte of DX (DL)
    #[allow(dead_code)]
    pub fn set_dl(&mut self, value: u8) {
        self.set_dx((self.dx() & 0xFF00) | (value as u16));
    }

    // ============== FPU Operations =================

    // Push value to FPU Stack
    pub fn fpu_push(&mut self, val: F80) {
        // Decrement top pointer (wrapping)
        self.fpu_top = (self.fpu_top.wrapping_sub(1)) & 7;
        // Write Value
        self.fpu_stack.set(self.fpu_top, val);
        // Mark as VALID
        self.fpu_tags[self.fpu_top] = FPU_TAG_VALID;
    }

    /// Push a double: `fpu_push` of an `F80` set to it.
    pub fn fpu_push_f64(&mut self, val: f64) {
        self.fpu_top = (self.fpu_top.wrapping_sub(1)) & 7;
        self.fpu_stack.set_f64(self.fpu_top, val);
        self.fpu_tags[self.fpu_top] = FPU_TAG_VALID;
    }

    // Pop value from FPU Stack
    pub fn fpu_pop(&mut self) -> F80 {
        let val = self.fpu_stack.get(self.fpu_top);
        self.fpu_drop();
        val
    }

    /// Pop, for nothing.
    pub fn fpu_drop(&mut self) {
        // Mark current top as EMPTY before moving on
        self.fpu_tags[self.fpu_top] = FPU_TAG_EMPTY;
        // Increment top pointer (wrapping)
        self.fpu_top = (self.fpu_top + 1) & 7;
    }

    // Access ST(i) relative to Top
    pub fn fpu_get(&self, index: usize) -> F80 {
        let actual_idx = (self.fpu_top.wrapping_add(index)) & 7;
        if self.fpu_tags[actual_idx] == crate::cpu::FPU_TAG_EMPTY {
            let mut ind = F80::new();
            ind.set_real_indefinite();
            return ind;
        }
        self.fpu_stack.get(actual_idx)
    }

    /// ST(i) as a double: `fpu_get(i).get_f64()`.
    pub fn fpu_get_f64(&self, index: usize) -> f64 {
        let actual_idx = (self.fpu_top.wrapping_add(index)) & 7;
        if self.fpu_tags[actual_idx] == crate::cpu::FPU_TAG_EMPTY {
            return FPU_INDEFINITE_F64;
        }
        self.fpu_stack.get_f64(actual_idx)
    }

    // Set ST(i) relative to Top
    pub fn fpu_set(&mut self, index: usize, val: F80) {
        let actual_idx = (self.fpu_top + index) & 7;
        self.fpu_stack.set(actual_idx, val);
    }

    /// Set ST(i) to a double: `fpu_set` of an `F80` set to it.
    pub fn fpu_set_f64(&mut self, index: usize, val: f64) {
        let actual_idx = (self.fpu_top + index) & 7;
        self.fpu_stack.set_f64(actual_idx, val);
    }

    /// Physical register `i`'s 80 bits, whatever its tag, and setting them.
    pub fn fpu_reg(&self, i: usize) -> F80 {
        self.fpu_stack.get(i)
    }

    /// Physical register `i` as a double, whatever its tag.
    pub fn fpu_reg_f64(&self, i: usize) -> f64 {
        self.fpu_stack.get_f64(i)
    }

    pub fn fpu_set_reg(&mut self, i: usize, val: F80) {
        self.fpu_stack.set(i, val);
    }

    // Get physical index for ST(i)
    pub fn fpu_get_phys_index(&self, i: usize) -> usize {
        (self.fpu_top + i) & 7
    }

    pub fn load_int_to_f80(&mut self, addr: usize, size: MemorySize) -> F80 {
        let (val, neg) = match size {
            MemorySize::Int16 => {
                let v = self.lin_read_16(addr) as i16;
                (v.unsigned_abs() as u128, v < 0)
            }
            MemorySize::Int32 => {
                let v = self.lin_read_32(addr) as i32;
                (v.unsigned_abs() as u128, v < 0)
            }
            MemorySize::Int64 => {
                let v = self.lin_read_64(addr) as i64;
                (v.unsigned_abs() as u128, v < 0)
            }
            _ => (0, false),
        };

        let mut f = F80::new();
        f.st = F80::encode_from_u128(val, neg);
        f
    }

    /// The memory of the resident TSRs, in conventional and upper memory.
    fn resident_memory(&mut self) -> Vec<std::ops::Range<usize>> {
        let mut resident = vec![(crate::mcb::first_mcb(&self.bus) as usize + 1) * 16..self.resident_end as usize * 16];
        for &psp in &self.resident_upper {
            let block = crate::mcb::read_mcb(&mut self.bus, psp - 1);
            resident.push(psp as usize * 16..(psp as usize + block.size as usize) * 16);
        }
        resident
    }

    /// Put the BIOS's interrupt vectors back, except those hooked by a
    /// resident TSR, in conventional or upper memory.
    fn install_bios_traps(&mut self) {
        let resident = self.resident_memory();
        crate::bios::restore_ivt(&mut self.bus, &resident);
    }

    /// After the PICs were put back as the BIOS leaves them: the IRQs of
    /// the second PIC whose handlers are resident drivers' (a packet
    /// driver's network card) stay let through, as the drivers left them
    /// (`slave_imr`, the mask before).
    fn keep_resident_irqs(&mut self, slave_imr: u8) {
        let resident = self.resident_memory();
        for line in 0..8u8 {
            let vector = 0x70 + line as usize;
            let entry = (self.bus.read_16(vector * 4 + 2) as usize) << 4 | self.bus.read_16(vector * 4) as usize;
            if slave_imr & (1 << line) == 0 && resident.iter().any(|r| r.contains(&entry)) {
                self.bus.pic.slave.imr &= !(1 << line);
            }
        }
        self.bus.arm_ipx_irq();
    }

    /// Turn expanded memory and upper memory blocks on or off, with no
    /// program running. Upper memory holding resident programs stays as it
    /// is until rust-dos starts again.
    pub fn set_upper_memory(&mut self, ems: bool, umb: bool) -> Result<(), String> {
        crate::ems::set_enabled(&mut self.bus, ems);
        // With EMS, its page frame takes the upper half of upper memory.
        let size = if ems { 0x1000 } else { 0x2000 };
        let wanted = umb.then_some(size);
        if self.bus.umb.map(|u| u.size) == wanted {
            return Ok(());
        }
        if !self.resident_upper.is_empty() {
            return Err("Upper memory holds resident programs: it changes the next time rust-dos starts".to_string());
        }
        let _ = crate::mcb::link_upper(&mut self.bus, false);
        self.bus.umb = wanted.map(|size| crate::mcb::Umb { size, linked: false });
        crate::mcb::build_upper(&mut self.bus);
        match crate::mcb::release_from(&mut self.bus, self.resident_end) {
            Some(end) => self.resident_end = end,
            None => {
                crate::mcb::init_empty(&mut self.bus);
                self.resident_end = crate::mcb::first_free(&self.bus);
            }
        }
        self.bus.sync_drive_bda();
        Ok(())
    }

    /// Lay conventional memory out afresh for another machine, at the
    /// prompt: a Tandy's ends below its video memory and a PCjr's first
    /// block is above its. The resident programs go, and the programs
    /// loaded high.
    pub fn relayout_conventional(&mut self) {
        let _ = crate::mcb::link_upper(&mut self.bus, false);
        crate::mcb::init_empty(&mut self.bus);
        crate::mcb::build_upper(&mut self.bus);
        self.resident_end = crate::mcb::first_free(&self.bus);
        self.resident_upper.clear();
        self.bus.log_string(&format!(
            "[DOS] Conventional memory for this machine: {} KB from {:04X}, resident programs dropped",
            (crate::mcb::conventional_end(&self.bus) - self.resident_end - 1) as usize * 16 / 1024,
            self.resident_end + 1
        ));
    }

    /// Whether the screen is in the text mode the prompt runs in: mode 3
    /// (7 on a monochrome adapter), 80 columns by 25 rows, page 0.
    fn prompt_text_mode(&self) -> bool {
        let bus = &self.bus;
        let mode = if bus.vga.setup().mono() { 0x07 } else { 0x03 };
        let rows_ok = !bus.vga.adapter.ega_bios() || bus.read_8(0x0484) == 24;
        (bus.read_8(0x0449) & 0x7F) == mode
            && bus.video_mode as u8 == mode
            && bus.read_16(0x044A) == 80
            && rows_ok
            && bus.read_8(0x0462) == 0
    }

    pub fn load_shell(&mut self) {
        self.note_abort();
        // A shell reload always starts outside any DOS process. Normally
        // termination restored this to the shell's PSP; frontends that
        // explicitly close a running process have no parent context to do so.
        self.current_psp = 0;
        // Windows' keyboard ends with it.
        crate::bios::windows_keyboard(&mut self.bus, false);
        // A system booted from a disk turned the machine off: DOS starts
        // over on it.
        if self.bus.boot.take().is_some() {
            // What it changed on the host folders it had as disks goes
            // into them.
            for line in self.bus.disk.finish_shared_disks(true) {
                self.bus.log_string(&format!("[BOOT] {}", line));
                self.bus.disk_notices.push(line);
            }
            self.bus.disk.keep_journals(false);
            self.bus.restore_dos_machine();
            self.resident_end = crate::mcb::first_free(&self.bus);
            self.resident_upper.clear();
            self.bus.log_string("[BOOT] The booted system is off; DOS starts again");
        }
        // No program runs: `core=auto` is back on the interpreter, and the
        // dynamic recompiler's code for the last program goes.
        self.pm_latched = false;
        self.dynrec.flush();
        self.program.clear();

        // Get the Code
        let shell_code = get_shell_code();

        // Load into RAM at CS:IP (SHELL_SEGMENT:0x0100)
        // We use 0x100 because .COM files (and our shell) expect to run there.
        let start_addr = SHELL_SEGMENT as usize * 16 + 0x100;

        // Clear RAM
        // 0x0000-0x03FF is the IVT.
        // 0x0400-0x04FF is the BIOS Data Area (BDA).
        // If we zero those, the system dies. The first MCB and resident TSRs
        // sit above.
        self.bus
            .fill_ram(0x0500..crate::mcb::first_mcb(&self.bus) as usize * 16, 0);

        // No program is running: every paragraph above the resident TSRs is
        // available for allocation, and upper memory but for the TSRs loaded
        // high, unlinked again.
        let _ = crate::mcb::link_upper(&mut self.bus, false);
        match crate::mcb::release_from(&mut self.bus, self.resident_end) {
            Some(end) => self.resident_end = end,
            None => {
                self.bus
                    .log_string("[DOS] MCB chain corrupt, dropping resident programs");
                crate::mcb::init_empty(&mut self.bus);
                self.resident_end = crate::mcb::first_free(&self.bus);
            }
        }
        // Upper memory blocks stay with the programs loaded high, and with
        // those resident in conventional memory that allocated them (HDPMI32
        // its buffer for DOS calls).
        let cover = crate::mcb::umb_cover_seg(&self.bus);
        let mut resident: Vec<u16> = self.resident_upper.clone();
        resident.extend(
            crate::mcb::walk(&mut self.bus)
                .iter()
                .take_while(|(s, _)| *s < cover)
                .map(|(_, m)| m.owner)
                .filter(|&owner| owner > crate::mcb::DOS_OWNER),
        );
        if !crate::mcb::release_upper(&mut self.bus, &resident) {
            self.bus.log_string("[DOS] Upper memory chain corrupt, dropping the programs loaded high");
            self.resident_upper.clear();
        }

        // Re-install the HLE Interrupt Vectors
        self.install_bios_traps();
        self.alloc_strategy = 0;
        self.con_pending_scan = None;
        self.con_line = None;
        self.con_pending.clear();
        self.secondary_shells.clear();
        self.secondary = None;
        // The programs that started the one killed are gone with it.
        self.process_stack.clear();
        self.bios_wait_until = None;

        // A program that ends in the prompt's own text mode leaves what it
        // printed on the screen, as in DOS, and the prompt goes on below.
        let kept_cursor = self.prompt_text_mode().then(|| (self.bus.read_8(0x0450), self.bus.read_8(0x0451)));

        // Reset text-mode BDA fields so state from a previous program (e.g.
        // Norton Commander's 80x50 configuration) doesn't leak into the shell
        // and cause the renderer to draw more rows than the shell expects.
        // Registers and palette as the mode set leaves them, so a program
        // that exits in mode X or with its own palette doesn't leave the
        // shell in a 60 Hz mode or odd colors.
        crate::video::bios::reset_for_shell(&mut self.bus);
        // A program that ends without taking its mouse event handler back
        // (or that the debugger stopped) mustn't leave the driver calling
        // into memory the shell reuses.
        self.bus.mouse.remove_callback();
        crate::mouse::clear_callback_busy(&mut self.bus);
        self.bus.mouse.ps2 = crate::mouse::Ps2Mouse::default();
        self.bus.port_accesses.clear();
        crate::keyboard::drop_unseen_keys(&mut self.bus);
        match kept_cursor {
            Some((col, row)) => {
                self.bus.write_8(0x0450, col);
                self.bus.write_8(0x0451, row);
                self.bus.cursor_x = col as usize;
                self.bus.cursor_y = row as usize;
            }
            None => {
                // Clear text VRAM so we don't show leftover text from the
                // last program's other mode.
                self.bus.vga.vram_text.fill(0);
                self.bus.text_mem_mut().fill(0);
            }
        }
        self.bus.vga.mark_dirty_full();

        // Copy bytes
        self.bus.load_bytes(start_addr, &shell_code);

        // Reset CPU State to "Boot" values
        self.reset_to_real_mode();
        self.set_cs(SHELL_SEGMENT);
        self.set_ds(SHELL_SEGMENT);
        self.set_es(SHELL_SEGMENT);
        self.set_ss(SHELL_SEGMENT);
        self.set_ip(0x100); // Entry Point
        self.set_sp(SHELL_STACK);
        self.set_bp(0);

        self.set_ax(0);
        self.set_bx(0);
        self.set_cx(0);
        self.set_dx(0);
        self.set_si(0);
        self.set_di(0);

        self.flags = CpuFlags::from_bits_truncate(0x0202); // Reset Flags (IF=1)
        self.state = CpuState::Running;
        self.idle = false;
        let slave_imr = self.bus.pic.slave.imr;
        self.bus.reset_timers();
        self.bus.reset_serial_irqs();
        self.keep_resident_irqs(slave_imr);
        self.bus.reset_sound();
        self.bus.reset_network();
        // No program runs any more: its extended memory and A20 go too,
        // but for what resident programs hold, and a reset from now on is
        // a cold boot.
        let a20 = self.bus.xms.program_ended();
        self.bus.dpmi.reset();
        // The addresses of its values mean nothing to the next program.
        self.bus.freezes.clear();
        if self.bus.ems.is_some() {
            self.bus.ems = Some(crate::ems::Ems::new());
        }
        self.bus.set_a20(a20);
        if a20 {
            self.bus.kbc.output_port |= crate::kbc::OUT_A20;
        } else {
            self.bus.kbc.output_port &= !crate::kbc::OUT_A20;
        }
        self.bus.cmos.set(crate::cmos::SHUTDOWN_STATUS, 0);

        self.bus.disk.close_all_files();
        crate::dos_files::write_table(&mut self.bus);
        crate::dos_data::write(&mut self.bus);
        crate::xms::install_entry(&mut self.bus);

        self.bus.log_string("[SYSTEM] Shell Loaded. Ready.");
    }

    // Helper to read a u16 from a byte slice (Little Endian)
    #[allow(dead_code)]
    fn read_u16_le(data: &[u8], offset: usize) -> u16 {
        let low = data[offset] as u16;
        let high = data[offset + 1] as u16;
        (high << 8) | low
    }

    /// The contents of a program or batch file, on any drive: a host
    /// directory, a disk or CD image or a drive held in memory. Reading it
    /// takes the time the drive's speed says.
    fn read_program_file(&mut self, filename: &str) -> Option<crate::memfs::Bytes> {
        let bytes = self.bus.disk.file_data(filename)?.read().ok()?;
        if let Some(drive) = self.bus.disk.drive_of(filename) {
            let key = self.bus.disk.file_key(filename);
            let access = crate::disknoise::Access::File { write: false, key };
            self.bus.drive_activity(drive, crate::diskio::OPEN_BYTES + bytes.len() as u32, access);
        }
        Some(bytes)
    }

    /// Start the built-in DOS as the session does (`startup`): the shell,
    /// the box above its first prompt, then the startup's command lines
    /// and batch files.
    pub fn start_dos(&mut self) {
        self.load_shell();
        crate::video::print_banner(self, &self.startup.notes.clone());
        for item in self.startup.commands.clone() {
            match item {
                crate::boot::StartupItem::Lines(lines) => self.queue_batch_lines(&lines),
                crate::boot::StartupItem::BatchFile(file) => {
                    self.queue_batch_file(&file);
                }
            }
        }
    }

    /// Run a .BAT file from the virtual disk once the batch lines queued
    /// before it have run, as AUTOEXEC.BAT after the config's [autoexec].
    /// Returns false if the file can't be located or read.
    pub fn queue_batch_file(&mut self, filename: &str) -> bool {
        let Some(bytes) = self.read_program_file(filename) else {
            return false;
        };
        self.bus.log_string(&format!("[BATCH] Queueing {} ({} bytes)", filename, bytes.len()));
        self.batch.append_file(filename, &bytes, "");
        true
    }

    /// Run shell command lines once the batch lines queued before them
    /// have run, as if they were a batch file: the config's [autoexec], a
    /// game's commands.
    pub fn queue_batch_lines<I, S>(&mut self, lines: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.batch.append_lines(lines);
    }

    /// Start the batch file `filename` with the parameters `args`, `name`
    /// being how it was called (its `%0`): in place of the batch file
    /// running when a batch line starts it, else (and with `call`) before
    /// the batch lines waiting. Returns false if it can't be read.
    pub fn start_batch_file(&mut self, filename: &str, name: &str, args: &str, call: bool) -> bool {
        let Some(bytes) = self.read_program_file(filename) else {
            return false;
        };
        self.bus.log_string(&format!("[BATCH] Starting {} ({} bytes)", filename, bytes.len()));
        self.batch.start_file(name, &bytes, args, call);
        true
    }

    /// Load a program: from the shell (`segment` None), above the resident
    /// programs with all of memory, or for EXEC, into the block allocated
    /// for it at PSP segment `segment`.
    pub fn load_executable(&mut self, filename: &str, segment: Option<u16>) -> bool {
        let Some(bytes) = self.read_program_file(filename) else {
            return false;
        };
        let placement = segment.map_or(Placement::Shell, Placement::Child);
        self.load_program_bytes(filename, &bytes, placement)
    }

    /// LOADHIGH: load a program from the shell into the largest free upper
    /// memory block, if it fits there, else as `load_executable` does.
    pub fn load_executable_high(&mut self, filename: &str) -> bool {
        let Some(bytes) = self.read_program_file(filename) else {
            return false;
        };
        let block = crate::mcb::largest_free_upper(&mut self.bus).filter(|&(_, size)| size >= program_paras(&bytes));
        let Some((mcb_seg, size)) = block else {
            self.bus.log_string(&format!("[DOS] LOADHIGH: {} doesn't fit in upper memory, loading it low", filename));
            return self.load_program_bytes(filename, &bytes, Placement::Shell);
        };
        // The whole block, as EXEC gives a child all of one; the program
        // gives back what it doesn't need.
        let block = crate::mcb::read_mcb(&mut self.bus, mcb_seg);
        crate::mcb::write_mcb(&mut self.bus, mcb_seg, &crate::mcb::Mcb { owner: 0xFFFF, ..block });
        let psp = mcb_seg + 1;
        if !self.load_program_bytes(filename, &bytes, Placement::High(psp)) {
            crate::mcb::write_mcb(&mut self.bus, mcb_seg, &block);
            return false;
        }
        crate::mcb::write_mcb(&mut self.bus, mcb_seg, &crate::mcb::Mcb { owner: psp, size, ..block });
        crate::mcb::name_program(&mut self.bus, psp, filename);
        true
    }

    fn load_program_bytes(&mut self, filename: &str, bytes: &[u8], placement: Placement) -> bool {
        self.programs_loaded += 1;
        self.bus.log_string(&format!(
            "[DOS] Loading {} ({} bytes)",
            filename,
            bytes.len()
        ));

        // Check for EXE Signature ("MZ")
        let loaded = if bytes.len() > 2 && bytes[0] == 0x4D && bytes[1] == 0x5A {
            self.load_exe(bytes, placement)
        } else {
            self.load_com(bytes, placement)
        };
        if loaded {
            // A program starts with its DTA at PSP:0080h, over its command
            // tail.
            self.bus.dta_segment = self.current_psp;
            self.bus.dta_offset = 0x80;
            self.program = filename.rsplit(['\\', '/', ':']).next().unwrap_or(filename).to_ascii_uppercase();
            let (cs, ip) = (self.cs(), self.ip());
            // Physical, as the debugger's breakpoints are: through the page
            // tables of a virtual machine (Windows' 386 enhanced mode).
            let lin = self.get_physical_addr(cs, ip);
            let entry = self.peek_translate(lin as u32).map_or(lin, |p| p as usize);
            let events = &mut self.programs;
            events.started += 1;
            events.running += 1;
            events.entry = entry;
            events.entry_cs_ip = (cs, ip);
            events.name = self.program.clone();
            events.psp = self.current_psp;
        }
        if loaded && !matches!(placement, Placement::Child(_)) {
            // A program started from the shell gets the master environment
            // in the shell's environment area. (EXEC gives a child its own
            // copy of the parent's environment.)
            let path = self.program_path(filename);
            self.apply_env_rules();
            let mut block = self.environment_block(&path);
            let layout = crate::dos_data::layout(&self.bus);
            if block.len() > layout.environment_bytes() {
                self.bus.log_string("[DOS] The environment doesn't fit in its area, cut short");
                block.truncate(layout.environment_bytes() - 2);
                block.extend_from_slice(&[0, 0]);
            }
            let env_phys = self.get_physical_addr(layout.environment, 0) as u32;
            self.bus.guest_write_bytes(env_phys, &block);
            let psp_phys = self.get_physical_addr(self.current_psp, 0) as u32;
            self.bus.guest_write_16(psp_phys + 0x2C, layout.environment);
        }
        if loaded && placement == Placement::Shell {
            crate::mcb::name_program(&mut self.bus, self.current_psp, filename);
        }
        // A program started by another keeps what that one has, unless it
        // brings its own host.
        let own = loaded && crate::dpmi::passes_over(bytes);
        if loaded && (own || !matches!(placement, Placement::Child(_))) {
            if own && self.bus.dpmi.enabled {
                self.bus.log_string("[DPMI] The program loads Glide's DOS overlay: DOS/4GW is its own DPMI host");
            }
            self.bus.dpmi.passed_over = own.then_some(self.current_psp);
        }
        loaded
    }

    /// A variable of the master environment.
    pub fn get_env(&self, name: &str) -> Option<&str> {
        self.environment
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// Set a variable of the master environment, or remove it when `value`
    /// is empty. Names are upper case, as COMMAND.COM stores them. Returns
    /// false, changing nothing, when the environment would no longer fit
    /// in its area with a program's path after it: out of environment
    /// space.
    pub fn set_env(&mut self, name: &str, value: &str) -> bool {
        let name = name.to_ascii_uppercase();
        let existing = self.environment.iter().position(|(n, _)| *n == name);
        let old = existing.map_or(0, |i| name.len() + 2 + self.environment[i].1.len());
        let new = if value.is_empty() { 0 } else { name.len() + 2 + value.len() };
        if new > old && self.environment_block("").len() - old + new + MAX_PATH > crate::dos_data::layout(&self.bus).environment_bytes() {
            return false;
        }
        match (existing, value.is_empty()) {
            (Some(i), true) => {
                self.environment.remove(i);
            }
            (Some(i), false) => self.environment[i].1 = value.to_string(),
            (None, false) => self.environment.push((name, value.to_string())),
            (None, true) => {}
        }
        true
    }

    /// Replace the settings' variables and bring them into the environment.
    pub fn set_env_rules(&mut self, rules: Vec<crate::env_inject::Rule>) {
        self.env_injector.set_rules(rules);
        self.apply_env_rules();
    }

    /// Bring the settings' variables into the environment, unless the guest set them.
    pub fn apply_env_rules(&mut self) {
        let mut injector = std::mem::take(&mut self.env_injector);
        let fit = injector.apply(self);
        self.env_injector = injector;
        if !fit {
            self.bus.log_string("[ENV] The environment is out of space for the settings' variables");
        }
    }

    /// Lay DOS's tables out packed below the first MCB, as DOS=HIGH has
    /// them, or spread out up to 0FFFh, with no program running. Resident
    /// programs keep the layout until rust-dos starts again.
    pub fn set_dos_high(&mut self, high: bool) -> Result<(), String> {
        if self.bus.dos_high == high {
            return Ok(());
        }
        if self.resident_end != crate::mcb::first_free(&self.bus) {
            return Err("Resident programs are in conventional memory: dos_high changes the next time rust-dos starts".to_string());
        }
        let layout = if high { crate::dos_data::HIGH } else { crate::dos_data::LOW };
        if self.environment_block("").len() + MAX_PATH > layout.environment_bytes() {
            return Err("The environment is too large for dos_high: it stays off".to_string());
        }
        // The shell and DOS's data segment stay; what is after it moves.
        let from = crate::dos_data::HIGH.sft as usize * 16;
        let to = crate::dos_data::LOW.first_mcb as usize * 16 + 16;
        let _ = crate::mcb::link_upper(&mut self.bus, false);
        self.bus.dos_high = high;
        self.bus.fill_ram(from..to, 0);
        crate::mcb::init_empty(&mut self.bus);
        crate::mcb::build_upper(&mut self.bus);
        self.resident_end = crate::mcb::first_free(&self.bus);
        crate::dos_files::write_table(&mut self.bus);
        crate::dos_data::write(&mut self.bus);
        Ok(())
    }

    /// Fully qualified DOS path of a program file name.
    pub fn program_path(&self, filename: &str) -> String {
        self.bus
            .disk
            .qualify_path(filename)
            .unwrap_or_else(|| filename.to_ascii_uppercase())
    }

    /// An environment block with the master environment's variables,
    /// followed, as DOS 3+ does, by a word count of 1 and the program's
    /// fully qualified path. Programs find their own directory (and DOS
    /// extenders their own EXE file) through that path.
    pub fn environment_block(&self, program_path: &str) -> Vec<u8> {
        let mut block = Vec::new();
        for (name, value) in &self.environment {
            block.extend(crate::dosstr::to_bytes(name));
            block.push(b'=');
            block.extend(crate::dosstr::to_bytes(value));
            block.push(0);
        }
        if self.environment.is_empty() {
            block.push(0);
        }
        block.push(0);
        block.extend_from_slice(&[0x01, 0x00]);
        block.extend_from_slice(program_path.as_bytes());
        block.push(0);
        block
    }

    /// Write a program's command tail to its PSP (offset 80h: length, the
    /// text, CR). DOS passes the arguments with their leading space.
    pub fn set_command_tail(&mut self, psp: u16, args: &str) {
        let args = args.trim();
        let mut tail = Vec::new();
        if !args.is_empty() {
            tail.push(b' ');
            tail.extend(crate::dosstr::to_bytes(args).into_iter().take(125));
        }
        let psp_phys = self.get_physical_addr(psp, 0) as u32;
        self.bus.guest_write_8(psp_phys + 0x80, tail.len() as u8);
        self.bus.guest_write_bytes(psp_phys + 0x81, &tail);
        self.bus.guest_write_8(psp_phys + 0x81 + tail.len() as u32, 0x0D);
    }

    /// INT 21h AH=4Bh AL=03h — Load Overlay.
    ///
    /// Loads an EXE file's image (minus the MZ header) into a caller-specified
    /// memory block and applies relocations using a caller-supplied relocation
    /// factor. Does NOT create a PSP, does NOT change CS:IP or SS:SP, and does
    /// NOT start the overlay running — control returns to the caller, which
    /// will typically `CALL` or `JMP` into the overlay. Accepts .COM files too
    /// (no header, no relocations — just a raw copy into the overlay segment).
    ///
    /// `load_segment` = the segment at which the image bytes begin.
    /// `reloc_factor` = value added to every relocated 16-bit target.
    pub fn load_overlay(&mut self, filename: &str, load_segment: u16, reloc_factor: u16) -> bool {
        let bytes = match self.read_program_file(filename) {
            Some(b) => b,
            None => {
                self.bus
                    .log_string(&format!("[DOS] Overlay: cannot read '{}'", filename));
                return false;
            }
        };

        self.bus.log_string(&format!(
            "[DOS] Overlay load '{}' ({} bytes) -> seg {:04X} reloc_factor {:04X}",
            filename,
            bytes.len(),
            load_segment,
            reloc_factor
        ));

        // COM-style overlay (no MZ header, no relocations)
        if bytes.len() < 0x1C || &bytes[0..2] != b"MZ" {
            let phys = self.get_physical_addr(load_segment, 0) as u32;
            self.bus.guest_write_bytes(phys, &bytes);
            return true;
        }

        // EXE overlay
        let header_paras = u16::from_le_bytes([bytes[8], bytes[9]]) as usize;
        let header_size = header_paras * 16;
        if header_size > bytes.len() {
            self.bus
                .log_string("[DOS] Overlay: header larger than file");
            return false;
        }
        let reloc_count = u16::from_le_bytes([bytes[6], bytes[7]]) as usize;
        let reloc_offset = u16::from_le_bytes([bytes[24], bytes[25]]) as usize;

        // Copy image bytes directly at load_segment:0000 — no PSP, no offset.
        let image_phys = self.get_physical_addr(load_segment, 0) as u32;
        self.bus.guest_write_bytes(image_phys, &bytes[header_size..]);

        // Apply relocations: each entry is (offset, segment); the 16-bit word
        // at (load_segment + segment):offset gets `reloc_factor` added to it.
        if reloc_count > 0 && reloc_offset + reloc_count * 4 <= bytes.len() {
            for i in 0..reloc_count {
                let e = reloc_offset + i * 4;
                let rel_offset = u16::from_le_bytes([bytes[e], bytes[e + 1]]);
                let rel_seg = u16::from_le_bytes([bytes[e + 2], bytes[e + 3]]);

                let target_seg = load_segment.wrapping_add(rel_seg);
                let at = self.get_physical_addr(target_seg, rel_offset) as u32;
                let cur = self.bus.guest_read_16(at);
                self.bus.guest_write_16(at, cur.wrapping_add(reloc_factor));
            }
        }

        true
    }

    /// The top of a program's memory, for its PSP: the end of conventional
    /// memory, or of its upper memory block.
    fn memory_top(&mut self, placement: Placement, load_segment: u16) -> u16 {
        match placement {
            Placement::High(psp) => psp + crate::mcb::read_mcb(&mut self.bus, psp - 1).size,
            Placement::Shell => crate::mcb::low_end(&self.bus),
            Placement::Child(_) => crate::mcb::conventional_end(&self.bus),
        }
        .max(load_segment)
    }

    // COM loader
    fn load_com(&mut self, bytes: &[u8], placement: Placement) -> bool {
        let is_nested = matches!(placement, Placement::Child(_));
        let load_segment = match placement {
            Placement::Shell => self.transient_segment(),
            Placement::Child(segment) | Placement::High(segment) => segment,
        };
        let start_offset = 0x100; // COM files always start at 100h
        // A program loaded high has its block, up to 64 KB.
        let top = self.memory_top(placement, load_segment);
        let segment_bytes = match placement {
            Placement::High(_) => ((top - load_segment) as usize * 16).min(0x10000),
            _ => 0x10000,
        };

        // Clear 64KB of RAM segment for safety (simulating clean load)
        let phys_start_seg = self.get_physical_addr(load_segment, 0) as u32;
        self.bus.guest_fill(phys_start_seg, segment_bytes, 0);

        // Re-install the HLE Interrupt Vectors — but ONLY for the top-level
        // load. A nested EXEC (segment.is_some()) must preserve the parent's
        // IVT: any TSR / overlay that installed an INT 21h (or other) hook
        // expects its handler to remain live while the child runs. Clobbering
        // the IVT here was breaking F-117's VGAME.EXE, which calls
        // `INT 21h AX=BFBFh` expecting an MPS-specific handler MISC.EXE had
        // registered during the boot-time overlay load.
        if !is_nested {
            self.install_bios_traps();
        }

        // Load the file data at offset 0x100
        let phys_code_start = self.get_physical_addr(load_segment, start_offset) as u32;
        self.bus.guest_write_bytes(phys_code_start, bytes);

        // COM State
        self.set_cs(load_segment);
        self.set_ds(load_segment);
        self.set_es(load_segment);
        self.set_ss(load_segment); // Stack is in the same segment
        self.set_ip(0x100); // Entry Point
        self.set_sp((segment_bytes - 2) as u16); // End of segment (64KB - 2)

        // Setup PSP (Program Segment Prefix) at CS:0000
        let psp_phys = self.get_physical_addr(load_segment, 0) as u32;

        // Offset 0x00: INT 20h (Exit Program)
        self.bus.guest_write_bytes(psp_phys, &[0xCD, 0x20]);

        // Offset 0x02: Top of Memory (Segment): the end of conventional
        // memory (640 KB), or of the upper memory block.
        self.bus.guest_write_16(psp_phys + 2, top);

        // [0x06] Bytes in Segment (CP/M compatibility)
        self.bus.guest_write_bytes(psp_phys + 6, &[0x03, 0x00]);

        // Offset 0x2C: environment segment, set by whoever started the
        // program (load_executable or EXEC).
        self.bus.guest_write_16(psp_phys + 0x2C, 0);
        // Offset 0x80: empty command tail, filled in by the caller.
        self.set_command_tail(load_segment, "");
        // The handle table: the parent's handles for a program EXEC
        // starts, the standard ones for one the shell starts.
        let parent = matches!(placement, Placement::Child(_)).then_some(self.current_psp);
        crate::dos_files::init_psp(&mut self.bus, load_segment, parent);
        self.current_psp = load_segment;

        self.bus.log_string(&format!(
            "[DOS] Loaded COM file at {:04X}:{:04X}",
            self.cs(), self.ip()
        ));
        // COM files are allocated the full 64KB segment by DOS convention.
        self.heap_pointer = load_segment + 0x1000;
        // Loaded high, it has its upper memory block already.
        if !matches!(placement, Placement::High(_)) {
            crate::mcb::init_for_program(&mut self.bus, load_segment, 0x1000);
        }
        true
    }

    // EXE loader
    fn load_exe(&mut self, bytes: &[u8], placement: Placement) -> bool {
        if bytes.len() < 0x20 || &bytes[0..2] != b"MZ" {
            self.bus.log_string("[DOS] Invalid EXE: Missing MZ header");
            return false;
        }

        // Parse Header
        let header_paragraphs = u16::from_le_bytes([bytes[8], bytes[9]]) as usize;
        let header_size = header_paragraphs * 16;

        let min_alloc = u16::from_le_bytes([bytes[10], bytes[11]]);
        let max_alloc = u16::from_le_bytes([bytes[12], bytes[13]]);
        let init_ss = u16::from_le_bytes([bytes[14], bytes[15]]);
        let init_sp = u16::from_le_bytes([bytes[16], bytes[17]]);
        let init_ip = u16::from_le_bytes([bytes[20], bytes[21]]);
        let init_cs = u16::from_le_bytes([bytes[22], bytes[23]]);
        let reloc_table_offset = u16::from_le_bytes([bytes[24], bytes[25]]) as usize;
        let reloc_count = u16::from_le_bytes([bytes[6], bytes[7]]) as usize;

        // Clear Conventional Memory (Only if starting fresh from the shell, probably shouldn't blindly wipe if nested)
        // Stop at 0xA0000 to preserve VGA VRAM, BIOS ROM signature, font tables, and
        // Static Functionality Table set up in Bus::new(). Resident TSRs survive.
        if placement == Placement::Shell {
            let first_mcb = crate::mcb::first_mcb(&self.bus) as usize * 16;
            self.bus.fill_ram(0x500..first_mcb, 0);
            let end = crate::mcb::low_end(&self.bus) as usize * 16;
            self.bus.fill_ram(self.resident_end as usize * 16..end, 0);
            crate::dos_files::write_table(&mut self.bus);
            crate::dos_data::write(&mut self.bus);
            crate::xms::install_entry(&mut self.bus);
        }

        // Re-install the HLE Interrupt Vectors — only for the top-level load.
        // See the same guard in load_com for the reason: nested EXECs must
        // preserve the parent's IVT so TSR / overlay-installed hooks survive.
        if !matches!(placement, Placement::Child(_)) {
            self.install_bios_traps();
        }

        let load_segment: u16 = match placement {
            Placement::Shell => self.transient_segment(),
            Placement::Child(segment) | Placement::High(segment) => segment,
        };

        // Load Binary
        // Safety check: ensure header doesn't point past EOF
        if header_size > bytes.len() {
            self.bus
                .log_string("[DOS] Invalid EXE: Header larger than file");
            return false;
        }

        // The load module is the part of the file the MZ header counts:
        // pages of 512 bytes, the last one partly used. Bound programs such
        // as DOS extenders keep more data (their protected-mode image) after
        // it, which they read from the file themselves.
        let pages = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
        let last_page = u16::from_le_bytes([bytes[2], bytes[3]]) as usize;
        let module_len = match (pages, last_page) {
            (0, _) => bytes.len(),
            (p, 0) => p * 512,
            (p, l) => (p - 1) * 512 + l,
        };
        let image_end = module_len.clamp(header_size, bytes.len());
        let image_data = &bytes[header_size..image_end];

        // Determine the child's memory block size. Two paths:
        //
        //  * Fresh boot (segment == None): rebuild the MCB chain from scratch
        //    giving the program min(max_alloc, available) paragraphs per the
        //    MZ header, then init a trailing free block.
        //
        //  * Nested EXEC (segment == Some): the caller has already allocated
        //    an MCB for us via mcb::alloc. We simply read its size and leave
        //    the chain alone so the parent's allocations stay intact.
        //
        // A header asking for no memory at all, MINALLOC and MAXALLOC both
        // 0, means "load high": the program gets the whole block, its PSP at
        // the bottom as always, and its image at the top (KRNL386.EXE, which
        // builds Windows' first heap in the memory between the two).
        let load_high = min_alloc == 0 && max_alloc == 0;
        let image_paras = image_data.len().div_ceil(16) as u16;
        let min_program_paras = 0x10 + image_paras + min_alloc;

        let program_paras = if placement == Placement::Shell {
            let available = crate::mcb::low_end(&self.bus).saturating_sub(load_segment);
            let desired = if load_high {
                u16::MAX
            } else if max_alloc == 0 {
                min_program_paras
            } else {
                min_program_paras.saturating_add(max_alloc - min_alloc.min(max_alloc))
            };
            let paras = if desired >= available {
                available.saturating_sub(1).max(min_program_paras)
            } else {
                desired.max(min_program_paras)
            };
            crate::mcb::init_for_program(&mut self.bus, load_segment, paras);
            paras
        } else {
            // The caller allocated an MCB for us; trust its size.
            let mcb = crate::mcb::read_mcb(&mut self.bus, load_segment.wrapping_sub(1));
            if !mcb.is_valid() {
                self.bus
                    .log_string("[DOS] Nested load_exe: MCB at load_segment-1 is invalid");
                return false;
            }
            if mcb.size < min_program_paras {
                self.bus.log_string(&format!(
                    "[DOS] Nested load_exe: MCB size {:04X} < required {:04X}",
                    mcb.size, min_program_paras
                ));
                return false;
            }
            mcb.size
        };
        let relocation_base_segment = if load_high {
            load_segment + program_paras - image_paras
        } else {
            load_segment + 0x10
        };

        // Standard loader
        // DOS behavior: Skip the header, load the rest to CS:0000 (after PSP)
        let image_start_phys = self.get_physical_addr(relocation_base_segment, 0) as u32;
        self.bus.guest_write_bytes(image_start_phys, image_data);

        // Relocations
        // The file contains a table of pointers (Segment:Offset).
        // We must add 'relocation_base_segment' to the value found at those pointers.
        if reloc_count > 0 && reloc_table_offset + (reloc_count * 4) <= bytes.len() {
            for i in 0..reloc_count {
                let offset_idx = reloc_table_offset + (i * 4);

                // Read the relocation entry (Target Offset, Target Segment)
                let rel_offset = u16::from_le_bytes([bytes[offset_idx], bytes[offset_idx + 1]]);
                let rel_seg = u16::from_le_bytes([bytes[offset_idx + 2], bytes[offset_idx + 3]]);

                // Calculate physical address of the value we need to patch
                // The target segment in the table is relative to the Image Start
                let target_seg = relocation_base_segment.wrapping_add(rel_seg);
                let at = self.get_physical_addr(target_seg, rel_offset) as u32;

                // PATCH: Add the actual start segment to the value
                let val = self.bus.guest_read_16(at).wrapping_add(relocation_base_segment);
                self.bus.guest_write_16(at, val);
            }
        }

        // Setup Registers
        self.set_ds(load_segment); // Point to PSP
        self.set_es(load_segment);

        // CS/SS are relative to the Image Start (relocation_base_segment)
        self.set_cs(relocation_base_segment.wrapping_add(init_cs));
        self.set_ss(relocation_base_segment.wrapping_add(init_ss));
        self.set_ip(init_ip);
        self.set_sp(init_sp);

        let psp_phys = self.get_physical_addr(load_segment, 0) as u32;

        // Offset 0x00: INT 20h (Exit Program Instruction)
        self.bus.guest_write_bytes(psp_phys, &[0xCD, 0x20]);

        // Offset 0x02: Top of Memory (Segment)
        // Programs read this to know how much RAM they have: the end of
        // conventional memory (640KB), or of their upper memory block.
        let top = self.memory_top(placement, load_segment);
        self.bus.guest_write_16(psp_phys + 2, top);

        // Offset 0x80: empty command tail, filled in by the caller.
        self.set_command_tail(load_segment, "");
        // Offset 0x2C: environment segment, set by whoever started the
        // program (load_executable or EXEC).
        self.bus.guest_write_16(psp_phys + 0x2C, 0);
        // The handle table: the parent's handles for a program EXEC
        // starts, the standard ones for one the shell starts.
        let parent = matches!(placement, Placement::Child(_)).then_some(self.current_psp);
        crate::dos_files::init_psp(&mut self.bus, load_segment, parent);
        self.current_psp = load_segment;

        self.bus.log_string(&format!(
            "[DOS] Loaded. Entry CS:IP = {:04X}:{:04X}",
            self.cs(), self.ip()
        ));

        self.heap_pointer = load_segment + program_paras + 1;

        self.bus
            .log_string(&format!("[DEBUG] Heap starts at {:04X}", self.heap_pointer));


        true
    }
}

#[cfg(test)]
mod tests {
    use super::Cpu;
    use crate::voodoo::{regs::FBI_INIT0, Board};

    #[test]
    fn closing_program_resets_voodoo_output() {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        cpu.bus.configure_voodoo(Some(Board::Max));
        let voodoo = cpu.bus.voodoo.as_mut().unwrap();
        voodoo.pci.clock_enabled = true;
        voodoo.reg[FBI_INIT0] |= 1;
        assert!(voodoo.output());

        cpu.close_program();

        assert!(!cpu.bus.voodoo.as_ref().unwrap().output());
    }
}
