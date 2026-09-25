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
pub mod task;
pub use fault::{CpuResult, Fault, IntSource};
pub use mem::{Access, MemRef};
pub use regs::{ATTR_DB, ATTR_G, Seg, SegCache};
pub use seg::Descriptor;

/// Where the environment of programs started from the shell lives. The
/// area below the first MCB belongs to the shell.
pub const ENV_SEGMENT: u16 = 0x0C00;

/// Where the shell runs: its code at SHELL_SEGMENT:0100, its line buffers
/// at 0200 and 0300, and its stack below SHELL_STACK, in the memory between
/// the BIOS data area and the environment (ENV_SEGMENT), clear of the
/// interrupt vector table, whose vectors the BIOS and programs write.
pub const SHELL_SEGMENT: u16 = 0x0070;
pub const SHELL_STACK: u16 = 0x0F00;

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

/// The processor being emulated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CpuModel {
    I386,
    /// A 486DX: on-chip FPU, EFLAGS.AC, BSWAP/XADD/CMPXCHG/INVD/WBINVD/
    /// INVLPG, and no CPUID.
    I486,
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

// FPU Tag Word Values
pub const FPU_TAG_EMPTY: u8 = 1;
pub const FPU_TAG_VALID: u8 = 0;

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
        /// EFLAGS bits: resume, virtual-8086 mode, alignment check.
        const IOPL = 0x3000;
        const NT = 0x4000;
        const RF = 0x0001_0000;
        const VM = 0x0002_0000;
        const AC = 0x0004_0000;
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

pub struct Cpu {
    /// EAX, ECX, EDX, EBX, ESP, EBP, ESI, EDI (see `regs.rs` accessors).
    gpr: [u32; 8],
    eip: u32,
    /// ES, CS, SS, DS, FS, GS.
    seg: [SegCache; 6],
    pub model: CpuModel,
    pub cr0: u32,
    pub cr2: u32,
    pub cr3: u32,
    /// Debug registers; stored, but breakpoints are not implemented.
    pub dr: [u32; 8],
    pub gdtr: DescTable,
    pub idtr: DescTable,
    /// The local descriptor table and task register: selector and
    /// descriptor cache.
    pub ldtr: SegCache,
    pub tr: SegCache,
    /// Current privilege level: 0 in real mode, 3 in virtual-8086 mode,
    /// in protected mode that of the code segment.
    pub cpl: u8,
    /// Page translations, see `paging.rs`.
    pub tlb: paging::Tlb,

    pub bus: Bus,
    flags: CpuFlags,
    pub state: CpuState,
    pub pending_command: Option<String>,
    /// The lines typed at the prompt, for Up and Down. They stay while
    /// programs run and the shell is loaded again.
    pub shell_history: crate::shell::ShellHistory,
    /// Where Tab left the line at the prompt, for Tab again.
    pub shell_completion: Option<crate::shell::Completion>,
    /// What PAUSE or CHOICE waits for; batch lines wait with it.
    pub shell_wait: Option<crate::shell::ShellWait>,
    /// Where the prompt was printed, (column, row), while a line is typed
    /// after it.
    pub shell_prompt_at: Option<(u8, u8)>,
    /// The batch files running, whose lines are dispatched as if typed
    /// at the prompt while the shell is idle (no child program on the
    /// process_stack and CS still in shell-land), and ECHO.
    pub batch: crate::batch::Batch,
    /// The master environment (SET, PATH), in order. Programs started from
    /// the shell get a copy.
    pub environment: Vec<(String, String)>,
    pub current_psp: u16,
    pub heap_pointer: u16,
    /// MCB segment where memory above the TSRs kept resident from the shell
    /// begins; `FIRST_MCB_SEG` when there are none. Programs started from
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
    /// Error code of the last failed DOS call, for INT 21h AH=59h.
    pub last_dos_error: u16,
    /// Scan code of an extended key whose 00h the console functions of
    /// INT 21h have returned, for the next read.
    pub con_pending_scan: Option<u8>,
    /// Memory allocation strategy (INT 21h AH=58h).
    pub alloc_strategy: u16,
    /// End of an INT 15h AH=86h wait in progress, in PIT ticks.
    pub bios_wait_until: Option<u64>,

    // FPU State
    pub fpu_stack: [F80; 8],
    pub fpu_top: usize,
    fpu_flags: FpuFlags,
    pub fpu_control: u16,
    pub fpu_tags: [u8; 8],

    pub process_stack: Vec<ProcessContext>,
    /// Set by STI, MOV SS and POP SS: hardware interrupts wait until the
    /// next instruction has run.
    pub irq_shadow: bool,
    /// Decoded instructions, see `instr_cache.rs`.
    pub decode_cache: InstrCache,
    /// Instructions (including emulator service traps) run since start, not
    /// counting interrupt entries or time skipped while halted.
    pub executed: u64,
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
    /// runs it on the dynamic recompiler until it ends.
    pub dyn_latched: bool,
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

/// A parent process's state while its child runs (INT 21h AH=4Bh).
#[derive(Debug, Clone)]
pub struct ProcessContext {
    pub regs: CpuSnapshot,
    pub psp: u16,
    pub heap_pointer: u16,
}

use std::path::PathBuf;

/// The environment at startup. BLASTER, ULTRASND and ULTRADIR advertise
/// the sound cards' resources as SET lines in AUTOEXEC.BAT would (the
/// configuration can change the cards).
fn default_environment() -> Vec<(String, String)> {
    let gus = crate::gus::GusConfig::default();
    vec![
        ("PATH".to_string(), "C:\\".to_string()),
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
        Self {
            gpr: [0; 8],
            eip: 0x100,
            seg: [SegCache::real(0); 6],
            model: CpuModel::I486,
            cr0: CR0_ET,
            cr2: 0,
            cr3: 0,
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
            shell_history: crate::shell::ShellHistory::default(),
            shell_completion: None,
            shell_wait: None,
            shell_prompt_at: None,
            batch: crate::batch::Batch::default(),
            environment: default_environment(),
            fpu_stack: [F80::new(); 8],
            fpu_top: 0,
            fpu_flags: FpuFlags::from_bits_truncate(0x0000),
            fpu_control: 0x037F, // Default Control Word
            fpu_tags: [FPU_TAG_EMPTY; 8],
            current_psp: 0, // Will be set by loader
            heap_pointer: 0x2000,
            resident_end: crate::mcb::FIRST_MCB_SEG,
            resident_upper: Vec::new(),
            last_child_exit: 0,
            errorlevel: 0,
            last_dos_error: 0,
            con_pending_scan: None,
            alloc_strategy: 0,
            bios_wait_until: None,
            process_stack: Vec::new(),
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
            dyn_latched: false,
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
                CoreMode::Auto => self.dyn_latched,
                CoreMode::Normal => false,
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
            self.dyn_latched = true;
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
            self.bus.log_string(&format!("[DOS] TSR: resident in upper memory at {:04X}", psp));
            return;
        }
        // The last block of conventional memory in use.
        let chain = crate::mcb::walk(&self.bus);
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
            heap_pointer: self.heap_pointer,
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
        crate::mcb::free_owned_by(&mut self.bus, self.current_psp);
        self.bus.disk.close_process_files(self.current_psp);
        if self.return_to_parent() {
            return true;
        }
        self.errorlevel = code;
        self.state = CpuState::RebootShell;
        false
    }

    /// End the current process: back to the parent's context, returning
    /// through the terminate address in the process's PSP (0Ah), which
    /// EXEC set to the parent's return address and debuggers change. The
    /// parent's stack holds the interrupt frame the return pops.
    pub fn return_to_parent(&mut self) -> bool {
        let psp = self.current_psp as usize * 16;
        let terminate = (self.bus.read_16(psp + 0x0A), self.bus.read_16(psp + 0x0C));
        if !self.restore_process_context() {
            return false;
        }
        if terminate != (0, 0) {
            let frame = self.get_physical_addr(self.ss(), self.sp());
            self.bus.write_16(frame, terminate.0);
            self.bus.write_16(frame + 2, terminate.1);
        }
        true
    }

    pub fn restore_process_context(&mut self) -> bool {
        if let Some(context) = self.process_stack.pop() {
            // The program ended: `core=auto` goes back to the interpreter.
            self.dyn_latched = false;
            self.restore(&context.regs);
            self.current_psp = context.psp;
            self.heap_pointer = context.heap_pointer; // Restore heap specifically for that process? Maybe not... but safer.
            self.bus.log_string(&format!(
                "[CPU] Context Restored. Stack Depth: {}",
                self.process_stack.len()
            ));
            true
        } else {
            self.bus.log_string("[CPU] Restore Failed: Stack Empty");
            false
        }
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
    /// set this way; AC only exists on a 486.
    pub fn load_eflags(&mut self, value: u32) {
        let mut writable = FLAGS16_WRITABLE;
        if self.model == CpuModel::I486 {
            writable |= CpuFlags::AC.bits();
        }
        let keep = self.flags.bits() & CpuFlags::VM.bits();
        self.flags = CpuFlags::from_bits_retain(keep | (value & writable) | 0x0002);
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
        self.fpu_stack[self.fpu_top as usize] = val;
        // Mark as VALID
        self.fpu_tags[self.fpu_top as usize] = FPU_TAG_VALID;
    }

    // Pop value from FPU Stack
    pub fn fpu_pop(&mut self) -> F80 {
        let val = self.fpu_stack[self.fpu_top as usize];
        // Mark current top as EMPTY before moving on
        self.fpu_tags[self.fpu_top as usize] = FPU_TAG_EMPTY;
        // Increment top pointer (wrapping)
        self.fpu_top = (self.fpu_top + 1) & 7;
        val
    }

    // Access ST(i) relative to Top
    pub fn fpu_get(&self, index: usize) -> F80 {
        let actual_idx = (self.fpu_top.wrapping_add(index)) & 7;
        if self.fpu_tags[actual_idx as usize] == crate::cpu::FPU_TAG_EMPTY {
            let mut ind = F80::new();
            ind.set_real_indefinite();
            return ind;
        }
        self.fpu_stack[actual_idx as usize]
    }

    // Set ST(i) relative to Top
    pub fn fpu_set(&mut self, index: usize, val: F80) {
        let actual_idx = (self.fpu_top + index) & 7;
        self.fpu_stack[actual_idx] = val;
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

    /// Put the BIOS's interrupt vectors back, except those hooked by a
    /// resident TSR, in conventional or upper memory.
    fn install_bios_traps(&mut self) {
        let mut resident = vec![(crate::mcb::FIRST_MCB_SEG as usize + 1) * 16..self.resident_end as usize * 16];
        for &psp in &self.resident_upper {
            let block = crate::mcb::read_mcb(&self.bus, psp - 1);
            resident.push(psp as usize * 16..(psp as usize + block.size as usize) * 16);
        }
        crate::bios::restore_ivt(&mut self.bus, &resident);
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

    pub fn load_shell(&mut self) {
        // No program runs: `core=auto` is back on the interpreter, and the
        // dynamic recompiler's code for the last program goes.
        self.dyn_latched = false;
        self.dynrec.flush();

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
            .fill_ram(0x0500..crate::mcb::FIRST_MCB_SEG as usize * 16, 0);

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
        let resident: Vec<u16> = self.resident_upper.clone();
        if !crate::mcb::release_upper(&mut self.bus, &resident) {
            self.bus.log_string("[DOS] Upper memory chain corrupt, dropping the programs loaded high");
            self.resident_upper.clear();
        }

        // Re-install the HLE Interrupt Vectors
        self.install_bios_traps();
        self.alloc_strategy = 0;
        self.con_pending_scan = None;
        self.bios_wait_until = None;

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
        // Clear text VRAM so we don't show leftover text from the last program.
        self.bus.vga.vram_text.fill(0);
        self.bus.text_mem_mut().fill(0);
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
        self.bus.reset_timers();
        self.bus.reset_sound();
        // No program runs any more: its extended memory and A20 go too,
        // and a reset from now on is a cold boot.
        self.bus.xms = crate::xms::Xms::new();
        // The addresses of its values mean nothing to the next program.
        self.bus.freezes.clear();
        if self.bus.ems.is_some() {
            self.bus.ems = Some(crate::ems::Ems::new());
        }
        self.bus.set_a20(false);
        self.bus.kbc.output_port &= !crate::kbc::OUT_A20;
        self.bus.cmos.set(crate::cmos::SHUTDOWN_STATUS, 0);

        self.bus.disk.close_all_files();

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
        let block = crate::mcb::largest_free_upper(&self.bus).filter(|&(_, size)| size >= program_paras(&bytes));
        let Some((mcb_seg, size)) = block else {
            self.bus.log_string(&format!("[DOS] LOADHIGH: {} doesn't fit in upper memory, loading it low", filename));
            return self.load_program_bytes(filename, &bytes, Placement::Shell);
        };
        // The whole block, as EXEC gives a child all of one; the program
        // gives back what it doesn't need.
        let block = crate::mcb::read_mcb(&self.bus, mcb_seg);
        crate::mcb::write_mcb(&mut self.bus, mcb_seg, &crate::mcb::Mcb { owner: 0xFFFF, ..block });
        let psp = mcb_seg + 1;
        if !self.load_program_bytes(filename, &bytes, Placement::High(psp)) {
            crate::mcb::write_mcb(&mut self.bus, mcb_seg, &block);
            return false;
        }
        crate::mcb::write_mcb(&mut self.bus, mcb_seg, &crate::mcb::Mcb { owner: psp, size, ..block });
        true
    }

    fn load_program_bytes(&mut self, filename: &str, bytes: &[u8], placement: Placement) -> bool {
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
        if loaded && !matches!(placement, Placement::Child(_)) {
            // A program started from the shell gets the master environment
            // in the shell's environment area. (EXEC gives a child its own
            // copy of the parent's environment.)
            let path = self.program_path(filename);
            let block = self.environment_block(&path);
            let env_phys = self.get_physical_addr(ENV_SEGMENT, 0);
            self.bus.load_bytes(env_phys, &block);
            let psp_phys = self.get_physical_addr(self.current_psp, 0);
            self.bus.write_16(psp_phys + 0x2C, ENV_SEGMENT);
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
    /// is empty. Names are upper case, as COMMAND.COM stores them.
    pub fn set_env(&mut self, name: &str, value: &str) {
        let name = name.to_ascii_uppercase();
        let existing = self.environment.iter().position(|(n, _)| *n == name);
        match (existing, value.is_empty()) {
            (Some(i), true) => {
                self.environment.remove(i);
            }
            (Some(i), false) => self.environment[i].1 = value.to_string(),
            (None, false) => self.environment.push((name, value.to_string())),
            (None, true) => {}
        }
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
        let psp_phys = self.get_physical_addr(psp, 0);
        self.bus.write_8(psp_phys + 0x80, tail.len() as u8);
        self.bus.load_bytes(psp_phys + 0x81, &tail);
        self.bus.write_8(psp_phys + 0x81 + tail.len(), 0x0D);
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
            let phys = self.get_physical_addr(load_segment, 0);
            self.bus.load_bytes(phys, &bytes);
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
        let image_phys = self.get_physical_addr(load_segment, 0);
        self.bus.load_bytes(image_phys, &bytes[header_size..]);

        // Apply relocations: each entry is (offset, segment); the 16-bit word
        // at (load_segment + segment):offset gets `reloc_factor` added to it.
        if reloc_count > 0 && reloc_offset + reloc_count * 4 <= bytes.len() {
            for i in 0..reloc_count {
                let e = reloc_offset + i * 4;
                let rel_offset = u16::from_le_bytes([bytes[e], bytes[e + 1]]);
                let rel_seg = u16::from_le_bytes([bytes[e + 2], bytes[e + 3]]);

                let target_seg = load_segment.wrapping_add(rel_seg);
                let phys = self.get_physical_addr(target_seg, rel_offset);
                if phys + 2 <= self.bus.ram().len() {
                    let cur = self.bus.read_16(phys);
                    self.bus.write_16(phys, cur.wrapping_add(reloc_factor));
                }
            }
        }

        true
    }

    /// The top of a program's memory, for its PSP: the end of conventional
    /// memory, or of its upper memory block.
    fn memory_top(&self, placement: Placement, load_segment: u16) -> u16 {
        match placement {
            Placement::High(psp) => psp + crate::mcb::read_mcb(&self.bus, psp - 1).size,
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
        let phys_start_seg = self.get_physical_addr(load_segment, 0);
        self.bus.fill_ram(phys_start_seg..phys_start_seg + segment_bytes, 0);

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
        let phys_code_start = self.get_physical_addr(load_segment, start_offset);
        self.bus.load_bytes(phys_code_start, bytes);

        // COM State
        self.set_cs(load_segment);
        self.set_ds(load_segment);
        self.set_es(load_segment);
        self.set_ss(load_segment); // Stack is in the same segment
        self.set_ip(0x100); // Entry Point
        self.set_sp((segment_bytes - 2) as u16); // End of segment (64KB - 2)

        // Setup PSP (Program Segment Prefix) at CS:0000
        let psp_phys = self.get_physical_addr(load_segment, 0);

        // Offset 0x00: INT 20h (Exit Program)
        self.bus.write_8(psp_phys, 0xCD);
        self.bus.write_8(psp_phys + 1, 0x20);

        // Offset 0x02: Top of Memory (Segment): the end of conventional
        // memory (640 KB), or of the upper memory block.
        self.bus.write_16(psp_phys + 2, top);

        // [0x06] Bytes in Segment (CP/M compatibility)
        self.bus.write_8(psp_phys + 6, 0x03);
        self.bus.write_8(psp_phys + 7, 0x00);

        // Offset 0x2C: environment segment, set by whoever started the
        // program (load_executable or EXEC).
        self.bus.write_16(psp_phys + 0x2C, 0);
        // Offset 0x80: empty command tail, filled in by the caller.
        self.set_command_tail(load_segment, "");
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
            let first_mcb = crate::mcb::FIRST_MCB_SEG as usize * 16;
            self.bus.fill_ram(0x500..first_mcb, 0);
            let end = crate::mcb::low_end(&self.bus) as usize * 16;
            self.bus.fill_ram(self.resident_end as usize * 16..end, 0);
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
        let relocation_base_segment = load_segment + 0x10;

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

        // Standard loader
        // DOS behavior: Skip the header, load the rest to CS:0000 (after PSP)
        let image_start_phys = self.get_physical_addr(relocation_base_segment, 0);
        self.bus.load_bytes(image_start_phys, image_data);

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
                let phys_addr = self.get_physical_addr(target_seg, rel_offset);

                if phys_addr + 2 <= self.bus.ram().len() {
                    // Read the existing 16-bit value
                    let val_low = self.bus.ram()[phys_addr] as u16;
                    let val_high = self.bus.ram()[phys_addr + 1] as u16;
                    let mut val = (val_high << 8) | val_low;

                    // PATCH: Add the actual start segment to the value
                    val = val.wrapping_add(relocation_base_segment);

                    // Write it back
                    self.bus.load_bytes(phys_addr, &val.to_le_bytes());
                }
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

        let psp_phys = self.get_physical_addr(load_segment, 0);

        // Offset 0x00: INT 20h (Exit Program Instruction)
        self.bus.write_8(psp_phys, 0xCD);
        self.bus.write_8(psp_phys + 1, 0x20);

        // Offset 0x02: Top of Memory (Segment)
        // Programs read this to know how much RAM they have: the end of
        // conventional memory (640KB), or of their upper memory block.
        let top = self.memory_top(placement, load_segment);
        self.bus.write_16(psp_phys + 2, top);

        // Offset 0x80: empty command tail, filled in by the caller.
        self.set_command_tail(load_segment, "");
        // Offset 0x2C: environment segment, set by whoever started the
        // program (load_executable or EXEC).
        self.bus.write_16(psp_phys + 0x2C, 0);
        self.current_psp = load_segment;

        self.bus.log_string(&format!(
            "[DOS] Loaded. Entry CS:IP = {:04X}:{:04X}",
            self.cs(), self.ip()
        ));

        // Determine the child's memory block size. Two paths:
        //
        //  * Fresh boot (segment == None): rebuild the MCB chain from scratch
        //    giving the program min(max_alloc, available) paragraphs per the
        //    MZ header, then init a trailing free block.
        //
        //  * Nested EXEC (segment == Some): the caller has already allocated
        //    an MCB for us via mcb::alloc. We simply read its size and leave
        //    the chain alone so the parent's allocations stay intact.
        let image_paras = image_data.len().div_ceil(16) as u16;
        let min_program_paras = 0x10 + image_paras + min_alloc;

        let program_paras = if placement == Placement::Shell {
            let available = crate::mcb::low_end(&self.bus).saturating_sub(load_segment);
            let desired = if max_alloc == 0 {
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
            let mcb = crate::mcb::read_mcb(&self.bus, load_segment.wrapping_sub(1));
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
        self.heap_pointer = load_segment + program_paras + 1;

        self.bus
            .log_string(&format!("[DEBUG] Heap starts at {:04X}", self.heap_pointer));


        true
    }
}
