use bitflags::bitflags;
use iced_x86::MemorySize;
use std::collections::VecDeque;

use crate::bus::Bus;
use crate::f80::F80;
use crate::instr_cache::InstrCache;
use crate::shell::get_shell_code;

pub mod alu;
pub mod fault;
pub mod mem;
mod regs;
pub use fault::{CpuResult, Fault, IntSource};
pub use mem::{Access, MemRef};
pub use regs::{ATTR_DB, ATTR_G, Seg, SegCache};

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
pub const CR0_PG: u32 = 0x8000_0000;

// FPU Tag Word Values
pub const FPU_TAG_EMPTY: u8 = 1;
pub const FPU_TAG_VALID: u8 = 0;

// Constants for Flag Bits
bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

    pub bus: Bus,
    flags: CpuFlags,
    pub state: CpuState,
    pub pending_command: Option<String>,
    /// Pending batch-file command lines waiting to be dispatched as if the user
    /// had typed them at the prompt. Drained by main loop while the shell is
    /// idle (no child program on the process_stack and CS still in shell-land).
    pub batch_queue: VecDeque<String>,
    /// COMMAND.COM "ECHO" state. When false, batch lines run silently (no
    /// prompt+line echo before dispatch). Toggled by the ECHO ON / ECHO OFF
    /// built-in. Defaults to true; persists across batches like real DOS.
    pub batch_echo: bool,
    pub current_psp: u16,
    pub heap_pointer: u16,
    /// MCB segment where memory above the TSRs kept resident from the shell
    /// begins; `FIRST_MCB_SEG` when there are none. Programs started from
    /// the shell load right above it.
    pub resident_end: u16,
    /// Exit code (AL) and termination type (AH) of the most recently terminated
    /// child process. Read-and-clear by INT 21h AH=4Dh. Termination type:
    /// 0 = normal (INT 21 AH=4C), 1 = Ctrl-C, 2 = critical error, 3 = TSR.
    pub last_child_exit: u16,

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
    /// Set by BIOS services that wait for input (INT 16h with an empty
    /// keyboard buffer). The main loop then skips ahead to the next timer
    /// event instead of spinning through the retry loop, like it does for HLT.
    pub idle: bool,
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

impl Cpu {
    pub fn new(root_path: PathBuf) -> Self {
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
            bus: Bus::new(root_path),
            flags: CpuFlags::from_bits_truncate(0x0202), // Default Flag State: bit 1 reserved, IF=1
            state: CpuState::Running,
            pending_command: None,
            batch_queue: VecDeque::new(),
            batch_echo: true,
            fpu_stack: [F80::new(); 8],
            fpu_top: 0,
            fpu_flags: FpuFlags::from_bits_truncate(0x0000),
            fpu_control: 0x037F, // Default Control Word
            fpu_tags: [FPU_TAG_EMPTY; 8],
            current_psp: 0, // Will be set by loader
            heap_pointer: 0x2000,
            resident_end: crate::mcb::FIRST_MCB_SEG,
            last_child_exit: 0,
            process_stack: Vec::new(),
            irq_shadow: false,
            // 64K direct-mapped slots (~3.5 MB): comfortably large for any
            // DOS program's hot working set.
            decode_cache: InstrCache::new(16),
            executed: 0,
            null_interrupts: [0; 4],
            idle: false,
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
        let chain = crate::mcb::walk(&self.bus);
        let end = chain
            .iter()
            .rev()
            .find(|(_, m)| !m.is_free())
            .map_or(self.resident_end, |&(s, m)| {
                s.saturating_add(1).saturating_add(m.size)
            });
        if end >= crate::mcb::END_OF_CONVENTIONAL {
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

    pub fn restore_process_context(&mut self) -> bool {
        if let Some(context) = self.process_stack.pop() {
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
        let phys_addr = (segment as usize * 16) + offset as usize;
        // MASK TO 20 BITS to emulate 8086 wrap-around
        phys_addr & 0xFFFFF
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

    pub fn load_int_to_f80(&self, addr: usize, size: MemorySize) -> F80 {
        let (val, neg) = match size {
            MemorySize::Int16 => {
                let v = self.bus.read_16(addr) as i16;
                (v.abs() as u128, v < 0)
            }
            MemorySize::Int32 => {
                let v = self.bus.read_32(addr) as i32;
                (v.abs() as u128, v < 0)
            }
            _ => (0, false),
        };

        let mut f = F80::new();
        f.st = F80::encode_from_u128(val, neg);
        f
    }

    /// Point the HLE vectors back at the emulator's handlers, except those
    /// hooked by a resident TSR.
    fn install_bios_traps(&mut self) {
        let mut phys_addr = 0xF1000;
        // Keep new vectors at the end: programs may have remembered the
        // addresses of the older traps.
        let hle_vectors = vec![
            0x08, 0x09, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x1A, 0x20, 0x21, 0x2F, 0x33,
            0x00, 0x06,
        ];
        let resident =
            (crate::mcb::FIRST_MCB_SEG as usize + 1) * 16..self.resident_end as usize * 16;

        for vec in hle_vectors {
            let ivt_offset = (vec as usize) * 4;
            let handler_offset = (phys_addr & 0xFFFF) as u16;

            // Point IVT to F000:Offset
            let target = self.get_physical_addr(
                self.bus.read_16(ivt_offset + 2),
                self.bus.read_16(ivt_offset),
            );
            if !resident.contains(&target) {
                self.bus.write_16(ivt_offset, handler_offset); // IP
                self.bus.write_16(ivt_offset + 2, 0xF000); // CS
            }

            // Ensure the Trap Instruction exists (FE 38 XX CF)
            self.bus.write_8(phys_addr, 0xFE);
            self.bus.write_8(phys_addr + 1, 0x38);
            self.bus.write_8(phys_addr + 2, vec);
            self.bus.write_8(phys_addr + 3, 0xCF);

            phys_addr += 4;
        }
    }

    pub fn load_shell(&mut self) {
        // Get the Code
        let shell_code = get_shell_code();

        // Load into RAM at CS:IP (0x0000:0x0100)
        // We use 0x100 because .COM files (and our shell) expect to run there.
        let start_addr = 0x100;

        // Clear RAM
        // 0x0000-0x03FF is the IVT.
        // 0x0400-0x04FF is the BIOS Data Area (BDA).
        // If we zero those, the system dies. The first MCB and resident TSRs
        // sit above.
        self.bus
            .fill_ram(0x0500..crate::mcb::FIRST_MCB_SEG as usize * 16, 0);

        // No program is running: every paragraph above the resident TSRs is
        // available for allocation.
        match crate::mcb::release_from(&mut self.bus, self.resident_end) {
            Some(end) => self.resident_end = end,
            None => {
                self.bus
                    .log_string("[DOS] MCB chain corrupt, dropping resident programs");
                crate::mcb::init_empty(&mut self.bus);
                self.resident_end = crate::mcb::FIRST_MCB_SEG;
            }
        }

        // Re-install the HLE Interrupt Vectors
        self.install_bios_traps();

        // Reset text-mode BDA fields so state from a previous program (e.g.
        // Norton Commander's 80x50 configuration) doesn't leak into the shell
        // and cause the renderer to draw more rows than the shell expects.
        self.bus.write_8(0x0449, 0x03); // Mode 3 (80x25 color text)
        self.bus.write_16(0x044A, 80); // 80 columns
        self.bus.write_8(0x0462, 0); // Active page 0
        self.bus.write_8(0x0450, 0); // Cursor col
        self.bus.write_8(0x0451, 0); // Cursor row
        self.bus.write_8(0x0484, 24); // 25 rows
        self.bus.write_16(0x0485, 16); // 8x16 font cell
        self.bus.video_mode = crate::video::VideoMode::Text80x25Color;
        // Clear text VRAM so we don't show leftover text from the last program.
        for byte in self.bus.vga.vram_text.iter_mut() {
            *byte = 0;
        }
        self.bus.vga.mark_dirty_full();

        // DOS "Underscore" cursor
        // High Byte (0x06) = Start Scanline, Low Byte (0x07) = End Scanline
        self.bus.write_16(0x0460, 0x0D0E);

        // Copy bytes
        self.bus.load_bytes(start_addr, &shell_code);

        // Reset CPU State to "Boot" values
        self.set_cs(0);
        self.set_ds(0);
        self.set_es(0);
        self.set_ss(0);
        self.set_ip(0x100); // Entry Point
        self.set_sp(0xFF00); // Stack Pointer (Safe distance away)
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

    /// Read a .BAT file from the virtual disk and append its commands to
    /// `batch_queue`. Blank lines and `REM` comments are stripped. Returns
    /// false if the file can't be located or read.
    pub fn queue_batch_file(&mut self, filename: &str) -> bool {
        let path = match self.bus.disk.resolve_path(filename) {
            Some(p) if p.is_file() => p,
            _ => return false,
        };
        let contents = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => return false,
        };
        self.bus.log_string(&format!(
            "[BATCH] Queueing {} ({} bytes)",
            filename,
            contents.len()
        ));
        self.queue_batch_lines(contents.lines());
        true
    }

    /// Append shell command lines to `batch_queue`, skipping blank lines and
    /// `REM` comments. Used for .BAT files and the config's [autoexec].
    pub fn queue_batch_lines<I, S>(&mut self, lines: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for raw_line in lines {
            let line = raw_line.as_ref().trim();
            if line.is_empty() {
                continue;
            }
            let upper = line.to_ascii_uppercase();
            if upper == "REM" || upper.starts_with("REM ") || upper.starts_with("REM\t") {
                continue;
            }
            self.batch_queue.push_back(line.to_string());
        }
    }

    pub fn load_executable(&mut self, filename: &str, segment: Option<u16>) -> bool {
        // Find and Read the File
        let resolved_path = self.bus.disk.resolve_path(filename);

        let bytes = match resolved_path {
            Some(path) => match std::fs::read(path) {
                Ok(b) => b,
                Err(_) => return false,
            },
            None => return false,
        };

        self.bus.log_string(&format!(
            "[DOS] Loading {} ({} bytes)",
            filename,
            bytes.len()
        ));

        // Check for EXE Signature ("MZ")
        if bytes.len() > 2 && bytes[0] == 0x4D && bytes[1] == 0x5A {
            return self.load_exe(&bytes, segment);
        } else {
            return self.load_com(&bytes, segment);
        }
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
        let resolved = self.bus.disk.resolve_path(filename);
        let bytes = match resolved.and_then(|p| std::fs::read(p).ok()) {
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

    // COM loader
    fn load_com(&mut self, bytes: &[u8], segment: Option<u16>) -> bool {
        let is_nested = segment.is_some();
        let load_segment = segment.unwrap_or(self.transient_segment());
        let start_offset = 0x100; // COM files always start at 100h

        // Clear 64KB of RAM segment for safety (simulating clean load)
        let phys_start_seg = self.get_physical_addr(load_segment, 0);
        self.bus.fill_ram(phys_start_seg..phys_start_seg + 0x10000, 0);

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
        self.set_sp(0xFFFE); // End of segment (64KB - 2)

        // Setup PSP (Program Segment Prefix) at CS:0000
        let psp_phys = self.get_physical_addr(load_segment, 0);

        // Offset 0x00: INT 20h (Exit Program)
        self.bus.write_8(psp_phys, 0xCD);
        self.bus.write_8(psp_phys + 1, 0x20);

        // Offset 0x02: Top of Memory (Segment)
        // 0xA000 corresponds to 640KB (standard DOS conventional memory limit)
        // We write it in Little Endian (00 A0)
        self.bus.write_8(psp_phys + 2, 0x00);
        self.bus.write_8(psp_phys + 3, 0xA0);

        // [0x06] Bytes in Segment (CP/M compatibility)
        self.bus.write_8(psp_phys + 6, 0x03);
        self.bus.write_8(psp_phys + 7, 0x00);

        // Offset 0x2C: Segment address of environment block
        // 0x0000 = No environment / Use parent. Prevents access violation if app checks.
        self.bus.write_8(psp_phys + 0x2C, 0x00);
        self.bus.write_8(psp_phys + 0x2D, 0x00);

        // TODO: Pass Command Line Arguments via PSP
        // Offset 0x80: Command Tail Length (Empty)
        self.bus.write_8(psp_phys + 0x80, 0x00);
        // Offset 0x81: Command Tail (CR only)
        self.bus.write_8(psp_phys + 0x81, 0x0D);

        // --- ENVIRONMENT SETUP ---
        // Create a default environment block if none exists (usually for first program)
        // Segment 0x0C00
        let env_seg = 0x0C00;
        let env_phys = self.get_physical_addr(env_seg, 0);

        // Simple Default Env: "PATH=C:\" \0 "COMSPEC=COMMAND.COM" \0 \0
        // BLASTER advertises the SB resource map to drivers at autodetect
        // time: A220 base I/O, I5 IRQ, D1 DMA, T3 = SB 2.0. Matches what
        // SET BLASTER in AUTOEXEC.BAT would publish on a real PC.
        let default_env = b"PATH=C:\\\0COMSPEC=COMMAND.COM\0BLASTER=A220 I5 D1 T3\0\0";
        for (i, &b) in default_env.iter().enumerate() {
            self.bus.write_8(env_phys + i, b);
        }

        // Point PSP to this environment
        self.bus.write_16(psp_phys + 0x2C, env_seg);
        self.current_psp = load_segment;

        self.bus.log_string(&format!(
            "[DEBUG] Wrote PSP[06] = {:02X} at Phys {:05X}. Env at {:04X}",
            self.bus.read_8(psp_phys + 6),
            psp_phys + 6,
            env_seg
        ));

        self.bus.log_string(&format!(
            "[DOS] Loaded COM file at {:04X}:{:04X}",
            self.cs(), self.ip()
        ));
        // COM files are allocated the full 64KB segment by DOS convention.
        self.heap_pointer = load_segment + 0x1000;
        crate::mcb::init_for_program(&mut self.bus, load_segment, 0x1000);
        true
    }

    // EXE loader
    pub fn load_exe(&mut self, bytes: &[u8], segment: Option<u16>) -> bool {
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
        if segment.is_none() {
            let first_mcb = crate::mcb::FIRST_MCB_SEG as usize * 16;
            self.bus.fill_ram(0x500..first_mcb, 0);
            self.bus
                .fill_ram(self.resident_end as usize * 16..0xA0000, 0);
        }

        // Re-install the HLE Interrupt Vectors — only for the top-level load.
        // See the same guard in load_com for the reason: nested EXECs must
        // preserve the parent's IVT so TSR / overlay-installed hooks survive.
        if segment.is_none() {
            self.install_bios_traps();
        }

        let load_segment: u16 = segment.unwrap_or(self.transient_segment());
        let relocation_base_segment = load_segment + 0x10;

        // Load Binary
        // Safety check: ensure header doesn't point past EOF
        if header_size > bytes.len() {
            self.bus
                .log_string("[DOS] Invalid EXE: Header larger than file");
            return false;
        }

        // Standard loader
        // DOS behavior: Skip the header, load the rest to CS:0000 (after PSP)
        let image_start_phys = self.get_physical_addr(relocation_base_segment, 0);
        self.bus.load_bytes(image_start_phys, &bytes[header_size..]);

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
        // Programs read this to know how much RAM they have.
        // We report 640KB (0xA000 paragraphs).
        // Little Endian: 00 A0
        self.bus.write_8(psp_phys + 2, 0x00);
        self.bus.write_8(psp_phys + 3, 0xA0);

        // TODO: Pass Command Line Arguments via PSP
        // Offset 0x80: Command Tail Length (0 bytes)
        self.bus.write_8(psp_phys + 0x80, 0x00);
        // Offset 0x81: Command Tail (CR character)
        self.bus.write_8(psp_phys + 0x81, 0x0D);

        // Create a default environment block
        let env_seg = 0x0C00;
        let env_phys = self.get_physical_addr(env_seg, 0);
        // BLASTER advertises the SB resource map to drivers at autodetect
        // time: A220 base I/O, I5 IRQ, D1 DMA, T3 = SB 2.0. Matches what
        // SET BLASTER in AUTOEXEC.BAT would publish on a real PC.
        let default_env = b"PATH=C:\\\0COMSPEC=COMMAND.COM\0BLASTER=A220 I5 D1 T3\0\0";
        for (i, &b) in default_env.iter().enumerate() {
            self.bus.write_8(env_phys + i, b);
        }

        self.bus.write_16(psp_phys + 0x2C, env_seg);
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
        let image_len = bytes.len() - header_size;
        let image_paras = ((image_len + 15) / 16) as u16;
        let min_program_paras = 0x10 + image_paras + min_alloc;

        let program_paras = if segment.is_none() {
            let available = 0xA000u16.saturating_sub(load_segment);
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
