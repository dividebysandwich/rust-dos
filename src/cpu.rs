use bitflags::bitflags;
use iced_x86::{Decoder, DecoderOptions, Instruction, MemorySize, Mnemonic, OpKind, Register};
use std::collections::VecDeque;

use crate::bus::Bus;
use crate::f80::F80;
use crate::instructions::utils::calculate_addr;
use crate::shell::get_shell_code;

// FPU Tag Word Values
pub const FPU_TAG_EMPTY: u8 = 1;
pub const FPU_TAG_VALID: u8 = 0;

// Constants for Flag Bits
bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct CpuFlags: u16 {
        const CF = 0x0001;
        const PF = 0x0004;
        const AF = 0x0010;
        const ZF = 0x0040;
        const SF = 0x0080;
        const DF = 0x0400; // Bit 10
        const IF = 0x0200;
        const TF = 0x0100;
        const OF = 0x0800;
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
    // General Purpose
    pub ax: u16,
    pub bx: u16,
    pub cx: u16,
    pub dx: u16,
    pub di: u16,
    pub si: u16,

    // For future 32-bit instructions
    pub eax: u32,
    #[allow(dead_code)]
    pub ebx: u32,
    #[allow(dead_code)]
    pub ecx: u32,
    #[allow(dead_code)]
    pub edx: u32,
    #[allow(dead_code)]
    pub edi: u32,
    #[allow(dead_code)]
    pub esi: u32,
    #[allow(dead_code)]
    pub ebp: u32,
    #[allow(dead_code)]
    pub esp: u32,
    #[allow(dead_code)]
    pub eip: u32,
    #[allow(dead_code)]
    pub eflags: u32,

    // Pointers & Segments
    pub bp: u16,
    pub sp: u16,
    pub cs: u16,
    pub ds: u16,
    pub es: u16,
    pub ss: u16,
    pub ip: u16,

    pub bus: Bus,
    flags: CpuFlags,
    pub state: CpuState,
    pub pending_command: Option<String>,
    pub current_psp: u16,
    pub heap_pointer: u16,

    // FPU State
    pub fpu_stack: [F80; 8],
    pub fpu_top: usize,
    fpu_flags: FpuFlags,
    pub fpu_control: u16,
    pub fpu_tags: [u8; 8],

    // REMOVEME: FLOAT DEBUGGING
    pub debug_qb_print: bool,
    pub last_fstp_addr: usize,

    // Execution Trace
    pub trace_log: VecDeque<String>,
    pub process_stack: Vec<ProcessContext>,
    pub last_timer_tick: u128,
}

#[derive(PartialEq, Debug)]
#[allow(dead_code)]
pub enum CpuState {
    Running,
    Halted,
    RebootShell,
}

#[derive(Debug, Clone)]
pub struct ProcessContext {
    pub ax: u16,
    pub bx: u16,
    pub cx: u16,
    pub dx: u16,
    pub si: u16,
    pub di: u16,
    pub bp: u16,
    pub sp: u16,
    pub cs: u16,
    pub ds: u16,
    pub es: u16,
    pub ss: u16,
    pub ip: u16,
    pub flags: CpuFlags,
    pub psp: u16,
    pub heap_pointer: u16,
}

use std::path::PathBuf;

impl Cpu {
    pub fn new(root_path: PathBuf) -> Self {
        Self {
            ax: 0,
            bx: 0,
            cx: 0,
            dx: 0,
            di: 0,
            si: 0,
            bp: 0,
            sp: 0,
            cs: 0,
            ds: 0,
            es: 0,
            ss: 0,
            eip: 0,
            eax: 0,
            ebx: 0,
            ecx: 0,
            edx: 0,
            edi: 0,
            esi: 0,
            ebp: 0,
            esp: 0,
            eflags: 0,
            ip: 0x100,
            bus: Bus::new(root_path),
            flags: CpuFlags::from_bits_truncate(0x0002), // Default Flag State, Bit 1 is always set
            state: CpuState::Running,
            pending_command: None,
            fpu_stack: [F80::new(); 8],
            fpu_top: 0,
            fpu_flags: FpuFlags::from_bits_truncate(0x0000),
            fpu_control: 0x037F, // Default Control Word
            fpu_tags: [FPU_TAG_EMPTY; 8],
            debug_qb_print: false,
            last_fstp_addr: 0,
            trace_log: VecDeque::new(),
            current_psp: 0, // Will be set by loader
            heap_pointer: 0x2000,
            process_stack: Vec::new(),
            last_timer_tick: 0,
        }
    }

    pub fn save_process_context(&mut self) {
        let context = ProcessContext {
            ax: self.ax,
            bx: self.bx,
            cx: self.cx,
            dx: self.dx,
            si: self.si,
            di: self.di,
            bp: self.bp,
            sp: self.sp,
            cs: self.cs,
            ds: self.ds,
            es: self.es,
            ss: self.ss,
            ip: self.ip,
            flags: self.flags,
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
            self.ax = context.ax;
            self.bx = context.bx;
            self.cx = context.cx;
            self.dx = context.dx;
            self.si = context.si;
            self.di = context.di;
            self.bp = context.bp;
            self.sp = context.sp;
            self.cs = context.cs;
            self.ds = context.ds;
            self.es = context.es;
            self.ss = context.ss;
            self.ip = context.ip;
            self.flags = context.flags;
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

    // ... step ...

    pub fn step(&mut self) {
        if self.state != CpuState::Running {
            return;
        }

        // Timer Check (Approx 18.2 Hz -> ~55ms)
        let now = self.bus.start_time.elapsed().as_millis();
        if now - self.last_timer_tick >= 55 {
            self.last_timer_tick = now;

            // println!("[DEBUG] Injecting INT 08h");

            // Inject INT 08h (Timer)
            let ivt_offset = 0x08 * 4;
            let handler_ip = self.bus.read_16(ivt_offset as usize);
            let handler_cs = self.bus.read_16((ivt_offset + 2) as usize);

            // Push Flags, CS, IP
            self.push(self.flags.bits());
            self.push(self.cs);
            self.push(self.ip);

            // Jump to Handler
            self.ip = handler_ip;
            self.cs = handler_cs;

            // Disable Interrupts (IF=0) and Trap Flag (TF=0)
            self.set_cpu_flag(CpuFlags::IF, false);
            self.set_cpu_flag(CpuFlags::TF, false);

            // We changed CS:IP, so we should return to fetch from new location
            return;
        }

        let phys_ip = self.get_physical_addr(self.cs, self.ip);
        // Ensure we can read at least a few bytes
        if phys_ip >= self.bus.ram.len() {
            return;
        }

        // Peek next bytes (simplified)
        let b0 = self.bus.read_8(phys_ip);
        let b1 = self
            .bus
            .read_8(self.get_physical_addr(self.cs, self.ip.wrapping_add(1)));

        // Check for "BOP" (BIOS Operation) -> FE 38 XX
        if b0 == 0xFE && b1 == 0x38 {
            let vector = self
                .bus
                .read_8(self.get_physical_addr(self.cs, self.ip.wrapping_add(2)));

            // Run the HLE handler
            crate::interrupts::handle_hle(self, vector);

            // Simulate IRET
            self.ip = self.pop();
            self.cs = self.pop();

            let hle_cf = self.get_cpu_flag(CpuFlags::CF);
            let hle_zf = self.get_cpu_flag(CpuFlags::ZF);
            let flags_to_restore = CpuFlags::from_bits_truncate(self.pop());

            self.set_cpu_flags(flags_to_restore);
            self.set_cpu_flag(CpuFlags::DF, false);
            self.set_cpu_flag(CpuFlags::CF, hle_cf);
            self.set_cpu_flag(CpuFlags::ZF, hle_zf);
            return;
        }

        // Decode
        // We slice safe
        let bytes = &self.bus.ram[phys_ip..];
        let mut decoder = Decoder::with_ip(16, bytes, self.ip as u64, DecoderOptions::NONE);
        let instr = decoder.decode();

        let disasm = format!("{:04X}:{:04X} {}", self.cs, self.ip, instr);
        self.bus.log_trace(&disasm);

        // Update IP
        self.ip = instr.next_ip() as u16;

        // Execute
        crate::instructions::execute_instruction(self, &instr);
    }

    // REMOVEME: Debugging QuickBASIC Float Conversion Issues
    pub fn trace_qb_conversion(&mut self, instr: &Instruction) {
        if !self.debug_qb_print {
            return;
        }

        // TRACK ZF CHANGES: If ZF changes without an obvious reason, we need to know
        let zf = self.get_cpu_flag(CpuFlags::ZF);

        match instr.mnemonic() {
            // Track the Decision Points
            Mnemonic::Je | Mnemonic::Jne => {
                // This is where the "08" vs "8" decision is actually made!
                self.bus.log_string(
                    format!(
                        "[QB-TRACE] {:?} taken? (ZF={}) at {:04X}:{:04X}",
                        instr.mnemonic(),
                        zf,
                        self.cs,
                        self.ip
                    )
                    .as_str(),
                );
            }

            // Monitor Sahf (The FPU->CPU Bridge)
            Mnemonic::Sahf => {
                let ah = (self.ax >> 8) as u8;
                self.bus.log_string(
                    format!("[QB-TRACE] SAHF: AH={:02X} (Bit6/ZF={})", ah, (ah >> 6) & 1).as_str(),
                );
            }

            // Enhanced Scasb (Watch the DI/CX result)
            Mnemonic::Scasb => {
                let val = self.get_al();
                let addr = self.get_physical_addr(self.es, self.di);
                let mem_val = self.bus.read_8(addr);
                self.bus.log_string(format!("[QB-TRACE] SCASB [{:05X}] AL={:02X} vs Mem={:02X} | CX={:04X} | DI={:04X} | ZF={}", 
                    addr, val, mem_val, self.cx, self.di, zf).as_str());
            }

            // Monitor Pointer Adjustment
            Mnemonic::Inc | Mnemonic::Dec => {
                let reg = instr.op0_register();
                if reg == Register::DI || reg == Register::SI || reg == Register::CX {
                    self.bus.log_string(
                        format!(
                            "[QB-TRACE] {:?} {:?} -> {:04X} (ZF={})",
                            instr.mnemonic(),
                            reg,
                            self.get_reg16(reg),
                            zf
                        )
                        .as_str(),
                    );
                }
            }

            // Keep existing trackers
            Mnemonic::Fstp if instr.memory_size() == MemorySize::Float80 => {
                let addr = calculate_addr(self, instr);
                self.last_fstp_addr = addr;
                let m = self.bus.read_64(addr);
                let se = self.bus.read_16(addr + 8);
                self.bus.log_string(
                    format!(
                        "\n[QB-TRACE] FSTP TBYTE at {:05X} Raw: {:04X} {:016X}",
                        addr, se, m
                    )
                    .as_str(),
                );
            }

            Mnemonic::Stosb => {
                let val = self.get_al();
                let addr = self.get_physical_addr(self.es, self.di);
                let ch = if val >= 32 && val <= 126 {
                    val as char
                } else {
                    '.'
                };
                self.bus.log_string(
                    format!(
                        "[QB-TRACE] STOSB [{:05X}] <- {:02X} ('{}') DI={:04X}",
                        addr, val, ch, self.di
                    )
                    .as_str(),
                );
            }

            Mnemonic::Loop | Mnemonic::Loope | Mnemonic::Loopne => {
                self.bus.log_string(
                    format!(
                        "[QB-TRACE] {:?} CX={:04X} ZF={} DI={:04X}",
                        instr.mnemonic(),
                        self.cx,
                        zf,
                        self.di
                    )
                    .as_str(),
                );
            }
            _ => {}
        }
    }

    // Update Parity Flag based on result
    pub fn update_pf(&mut self, result: u16) {
        let low_byte = (result & 0xFF) as u8;
        let ones = low_byte.count_ones();
        // Even parity means an even number of 1s (e.g., 0, 2, 4, 8)
        self.set_cpu_flag(CpuFlags::PF, (ones % 2) == 0);
    }

    // Helper to get a flag state
    pub fn get_cpu_flag(&self, mask: CpuFlags) -> bool {
        (self.flags & mask) != CpuFlags::empty()
    }

    // Helper to set/clear a flag
    pub fn set_cpu_flag(&mut self, mask: CpuFlags, value: bool) {
        // REMOVEME: ZF ALERT
        // if self.debug_qb_conversion && mask.contains(CpuFlags::ZF) {
        //     let old_zf = self.flags.contains(CpuFlags::ZF);

        //     // ALARM only when ZF changes from FALSE -> TRUE
        //     if !old_zf && value == true {
        //         let instr_str = self.get_instruction_at_ip();
        //         self.bus.log_string(&format!(
        //             "[ZF-ALARM] ZF flipped FALSE -> TRUE! CX:{:04X} | Instruction: {}",
        //             self.cx, instr_str
        //         ));
        //     }
        // }

        if value {
            self.flags.insert(mask);
        } else {
            self.flags.remove(mask);
        }
    }

    // Allows overwriting the flags register with a new bitflags struct
    pub fn set_cpu_flags(&mut self, new_flags: CpuFlags) {
        // REMOVEME: ZF ALERT
        // let old_zf = self.flags.contains(CpuFlags::ZF);
        // let new_zf = new_flags.contains(CpuFlags::ZF);
        // if self.debug_qb_conversion && !old_zf && new_zf {
        //     let instr_str = self.get_instruction_at_ip();
        //     self.bus.log_string(&format!(
        //         "[ZF-ALARM] ZF flipped FALSE -> TRUE! CX:{:04X} | Instruction: {}",
        //         self.cx, instr_str
        //     ));
        // }

        let raw_bits = new_flags.bits();

        // 0x0FD5 masks only the valid 8086 flags:
        // (CF, PF, AF, ZF, SF, TF, IF, DF, OF)
        // Then we OR with 0x0002 to ensure Bit 1 is always 1.
        let sanitized_bits = (raw_bits & 0x0FD5) | 0x0002;

        self.flags = CpuFlags::from_bits_truncate(sanitized_bits);
    }

    pub fn get_cpu_flags(&self) -> CpuFlags {
        self.flags
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

    /// Helper to read the first operand (Destination).
    /// Returns: (Value, Optional Memory Address, Is 8-bit?)
    /// If address is Some, you should write the result back to that address.
    /// If address is None, you should write the result back to the register.
    #[allow(dead_code)]
    pub fn read_op0(cpu: &mut Cpu, instr: &Instruction) -> (u16, Option<usize>, bool) {
        match instr.op0_kind() {
            // Handle Register Operand
            OpKind::Register => {
                let reg = instr.op0_register();
                let is_8bit = reg.is_gpr8();

                let val = if is_8bit {
                    cpu.get_reg8(reg) as u16
                } else {
                    cpu.get_reg16(reg)
                };

                (val, None, is_8bit)
            }

            // Handle Memory Operand
            OpKind::Memory => {
                let addr = calculate_addr(cpu, instr);
                let is_8bit = instr.memory_size() == MemorySize::UInt8;

                let val = if is_8bit {
                    cpu.bus.read_8(addr) as u16
                } else {
                    cpu.bus.read_16(addr) // Uses the new helper above
                };

                (val, Some(addr), is_8bit)
            }

            // Fallback (Should not happen for R/W ops like ADD/RCL)
            _ => (0, None, false),
        }
    }

    #[allow(dead_code)]
    pub fn get_segment_value(&self, seg: Register) -> u16 {
        match seg {
            Register::ES => self.es,
            Register::CS => self.cs,
            Register::SS => self.ss,
            Register::DS => self.ds,
            // FS and GS are rarely used in standard Real Mode DOS,
            // but returning 0 is safe for now.
            Register::FS => 0,
            Register::GS => 0,
            // Fallback: If for some reason a non-segment register is passed,
            // default to DS (Data Segment)
            _ => self.ds,
        }
    }

    // Extract High byte (AH)
    pub fn get_ah(&self) -> u8 {
        (self.ax >> 8) as u8
    }
    // Extract Low byte (AL)
    pub fn get_al(&self) -> u8 {
        (self.ax & 0xFF) as u8
    }

    // Set 8-bit Register
    pub fn set_reg8(&mut self, reg: Register, value: u8) {
        match reg {
            Register::AL => self.ax = (self.ax & 0xFF00) | (value as u16),
            Register::AH => self.ax = (self.ax & 0x00FF) | ((value as u16) << 8),
            Register::BL => self.bx = (self.bx & 0xFF00) | (value as u16),
            Register::BH => self.bx = (self.bx & 0x00FF) | ((value as u16) << 8),
            Register::CL => self.cx = (self.cx & 0xFF00) | (value as u16),
            Register::CH => self.cx = (self.cx & 0x00FF) | ((value as u16) << 8),
            Register::DL => self.dx = (self.dx & 0xFF00) | (value as u16),
            Register::DH => self.dx = (self.dx & 0x00FF) | ((value as u16) << 8),
            _ => {}
        }
    }

    // Get 8-bit Register
    pub fn get_reg8(&self, reg: Register) -> u8 {
        match reg {
            Register::AL => (self.ax & 0xFF) as u8,
            Register::AH => (self.ax >> 8) as u8,
            Register::BL => (self.bx & 0xFF) as u8,
            Register::BH => (self.bx >> 8) as u8,
            Register::CL => (self.cx & 0xFF) as u8,
            Register::CH => (self.cx >> 8) as u8,
            Register::DL => (self.dx & 0xFF) as u8,
            Register::DH => (self.dx >> 8) as u8,
            _ => 0, // Panic or return 0 for unhandled registers
        }
    }

    // Set 16-bit Register
    pub fn set_reg16(&mut self, reg: Register, value: u16) {
        match reg {
            Register::AX => self.ax = value,
            Register::BX => self.bx = value,
            Register::CX => self.cx = value,
            Register::DX => self.dx = value,
            Register::SI => self.si = value,
            Register::DI => self.di = value,
            Register::BP => self.bp = value,
            Register::SP => self.sp = value,

            Register::ES => self.es = value,
            Register::DS => self.ds = value,
            Register::SS => self.ss = value,
            Register::CS => self.cs = value,

            _ => panic!("Unimplemented register write: {:?}", reg),
        }
    }

    // Get 16-bit Register
    pub fn get_reg16(&self, reg: Register) -> u16 {
        match reg {
            Register::AX => self.ax,
            Register::BX => self.bx,
            Register::CX => self.cx,
            Register::DX => self.dx,
            Register::SI => self.si,
            Register::DI => self.di,
            Register::BP => self.bp,
            Register::SP => self.sp,

            Register::ES => self.es,
            Register::DS => self.ds,
            Register::CS => self.cs,
            Register::SS => self.ss,
            _ => 0, // Panic or return 0 for unhandled registers
        }
    }

    // ADD 16 bit
    pub fn alu_add_16(&mut self, dest: u16, src: u16) -> u16 {
        let (result, carry) = dest.overflowing_add(src);

        self.set_cpu_flag(CpuFlags::CF, carry);
        self.set_cpu_flag(CpuFlags::ZF, result == 0);
        self.set_cpu_flag(CpuFlags::SF, (result & 0x8000) != 0); // High bit set?

        self.update_pf(result);

        // Overflow (Signed): if operands have same sign, but result has diff sign
        let op1_sign = (dest & 0x8000) != 0;
        let op2_sign = (src & 0x8000) != 0;
        let res_sign = (result & 0x8000) != 0;
        let overflow = (op1_sign == op2_sign) && (res_sign != op1_sign);
        self.set_cpu_flag(CpuFlags::OF, overflow);
        self.set_cpu_flag(CpuFlags::AF, ((dest ^ src ^ result) & 0x10) != 0);

        result
    }

    // SUB (and CMP) 16 bit
    pub fn alu_sub_16(&mut self, dest: u16, src: u16) -> u16 {
        let (result, borrow) = dest.overflowing_sub(src);

        self.set_cpu_flag(CpuFlags::CF, borrow); // In SUB, CF acts as Borrow
        self.set_cpu_flag(CpuFlags::ZF, result == 0);
        self.set_cpu_flag(CpuFlags::SF, (result & 0x8000) != 0);

        self.update_pf(result);

        // Overflow (Signed): operands diff sign, result diff sign from dest
        let op1_sign = (dest & 0x8000) != 0;
        let op2_sign = (src & 0x8000) != 0;
        let res_sign = (result & 0x8000) != 0;
        let overflow = (op1_sign != op2_sign) && (res_sign != op1_sign);
        self.set_cpu_flag(CpuFlags::OF, overflow);
        self.set_cpu_flag(CpuFlags::AF, ((dest ^ src ^ result) & 0x10) != 0);

        result
    }

    // SUB/CMP 8-bit
    pub fn alu_sub_8(&mut self, dest: u8, src: u8) -> u8 {
        let (result, borrow) = dest.overflowing_sub(src);

        self.set_cpu_flag(CpuFlags::CF, borrow);
        self.set_cpu_flag(CpuFlags::ZF, result == 0);
        self.set_cpu_flag(CpuFlags::SF, (result & 0x80) != 0); // Check Bit 7

        self.update_pf(result as u16);

        // 8-bit overflow (signed)
        let op1_sign = (dest & 0x80) != 0;
        let op2_sign = (src & 0x80) != 0;
        let res_sign = (result & 0x80) != 0;
        let overflow = (op1_sign != op2_sign) && (res_sign != op1_sign);
        self.set_cpu_flag(CpuFlags::OF, overflow);
        self.set_cpu_flag(CpuFlags::AF, ((dest ^ src ^ result) & 0x10) != 0);

        result
    }

    // ADD 8-bit
    pub fn alu_add_8(&mut self, dest: u8, src: u8) -> u8 {
        let (result, carry) = dest.overflowing_add(src);

        self.set_cpu_flag(CpuFlags::CF, carry);
        self.set_cpu_flag(CpuFlags::ZF, result == 0);
        self.set_cpu_flag(CpuFlags::SF, (result & 0x80) != 0);

        self.update_pf(result as u16);

        // 8-bit overflow (signed)
        let op1_sign = (dest & 0x80) != 0;
        let op2_sign = (src & 0x80) != 0;
        let res_sign = (result & 0x80) != 0;
        let overflow = (op1_sign == op2_sign) && (res_sign != op1_sign);
        self.set_cpu_flag(CpuFlags::OF, overflow);
        self.set_cpu_flag(CpuFlags::AF, ((dest ^ src ^ result) & 0x10) != 0);

        result
    }

    // SBB 8-bit
    pub fn alu_sbb_8(&mut self, dest: u8, src: u8) -> u8 {
        let carry_in = if self.get_cpu_flag(CpuFlags::CF) {
            1
        } else {
            0
        };

        // We perform the math using u16 to easily detect borrows
        let result_wide = (dest as u16)
            .wrapping_sub(src as u16)
            .wrapping_sub(carry_in as u16);
        let result = result_wide as u8;

        // Flags
        self.set_cpu_flag(CpuFlags::ZF, result == 0);
        self.set_cpu_flag(CpuFlags::SF, (result & 0x80) != 0);

        self.update_pf(result as u16);

        // Carry (Borrow) happens if the result wrapped (result_wide > 0xFF)
        self.set_cpu_flag(CpuFlags::CF, result_wide > 0xFF);

        // Overflow for subtraction: (dest_sign != src_sign) && (dest_sign != result_sign)
        let res_sign = (result & 0x80) != 0;
        let src_sign = (src & 0x80) != 0;
        let dest_sign = (dest & 0x80) != 0;

        self.set_cpu_flag(
            CpuFlags::OF,
            (dest_sign != src_sign) && (dest_sign != res_sign),
        );
        self.set_cpu_flag(CpuFlags::AF, (dest & 0x0F) < ((src & 0x0F) + carry_in));

        result
    }

    // SBB 16-bit
    pub fn alu_sbb_16(&mut self, dest: u16, src: u16) -> u16 {
        let carry_in = if self.get_cpu_flag(CpuFlags::CF) {
            1
        } else {
            0
        };

        // Use u32 to capture borrows
        let result_wide = (dest as u32)
            .wrapping_sub(src as u32)
            .wrapping_sub(carry_in as u32);
        let result = result_wide as u16;

        self.set_cpu_flag(CpuFlags::ZF, result == 0);
        self.set_cpu_flag(CpuFlags::SF, (result & 0x8000) != 0);

        self.update_pf(result);

        // Carry flag if we wrapped past 0
        self.set_cpu_flag(CpuFlags::CF, result_wide > 0xFFFF);

        // Set OF if the sign of the destination was different from the source,
        // AND the sign of the result is different from the destination.
        let dest_s = (dest & 0x8000) != 0;
        let src_s = (src & 0x8000) != 0;
        let res_s = (result & 0x8000) != 0;
        self.set_cpu_flag(CpuFlags::OF, (dest_s != src_s) && (res_s != dest_s));
        self.set_cpu_flag(CpuFlags::AF, ((dest ^ src ^ result) & 0x10) != 0);

        result
    }

    // ADC 8-bit
    pub fn alu_adc_8(&mut self, dest: u8, src: u8) -> u8 {
        let cf_in = if self.get_cpu_flag(CpuFlags::CF) {
            1
        } else {
            0
        };

        // Use u16 to capture the carry out
        let res_wide = (dest as u16) + (src as u16) + (cf_in as u16);
        let result = res_wide as u8;

        self.set_cpu_flag(CpuFlags::CF, res_wide > 0xFF);
        self.set_cpu_flag(CpuFlags::ZF, result == 0);
        self.set_cpu_flag(CpuFlags::SF, (result & 0x80) != 0);
        self.update_pf(result as u16);

        // Overflow (Signed)
        let op1_sign = (dest & 0x80) != 0;
        let op2_sign = (src & 0x80) != 0;
        let res_sign = (result & 0x80) != 0;
        // Overflow happens if adding two numbers of same sign results in different sign
        self.set_cpu_flag(
            CpuFlags::OF,
            (op1_sign == op2_sign) && (res_sign != op1_sign),
        );

        // AF: (op1 ^ op2 ^ result) & 0x10
        // This detects if a carry occurred from bit 3 to bit 4
        self.set_cpu_flag(CpuFlags::AF, ((dest ^ src ^ result) & 0x10) != 0);
        result
    }

    // ADC 16-bit
    pub fn alu_adc_16(&mut self, dest: u16, src: u16) -> u16 {
        let cf_in = if self.get_cpu_flag(CpuFlags::CF) {
            1
        } else {
            0
        };

        // Use u32 to capture carry out
        let res_wide = (dest as u32) + (src as u32) + (cf_in as u32);
        let result = res_wide as u16;

        self.set_cpu_flag(CpuFlags::CF, res_wide > 0xFFFF);
        self.set_cpu_flag(CpuFlags::ZF, result == 0);
        self.set_cpu_flag(CpuFlags::SF, (result & 0x8000) != 0);
        self.update_pf(result);

        // Overflow (Signed)
        let op1_sign = (dest & 0x8000) != 0;
        let op2_sign = (src & 0x8000) != 0;
        let res_sign = (result & 0x8000) != 0;
        self.set_cpu_flag(
            CpuFlags::OF,
            (op1_sign == op2_sign) && (res_sign != op1_sign),
        );

        // AF: Carry from bit 3 to 4
        self.set_cpu_flag(CpuFlags::AF, ((dest ^ src ^ result) & 0x10) != 0);

        result
    }

    // Stack Operations
    pub fn push(&mut self, value: u16) {
        self.sp = self.sp.wrapping_sub(2);
        let addr = self.get_physical_addr(self.ss, self.sp);
        // Write Little Endian
        self.bus.write_8(addr, (value & 0xFF) as u8);
        self.bus.write_8(addr + 1, (value >> 8) as u8);
    }
    pub fn pop(&mut self) -> u16 {
        let addr = self.get_physical_addr(self.ss, self.sp);
        let low = self.bus.read_8(addr) as u16;
        let high = self.bus.read_8(addr + 1) as u16;
        self.sp = self.sp.wrapping_add(2);
        (high << 8) | low
    }

    /// Extract Low byte of DX (DL)
    pub fn get_dl(&self) -> u8 {
        (self.dx & 0xFF) as u8
    }

    /// Set Low byte of DX (DL)
    #[allow(dead_code)]
    pub fn set_dl(&mut self, value: u8) {
        self.dx = (self.dx & 0xFF00) | (value as u16);
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

    fn install_bios_traps(&mut self) {
        let mut phys_addr = 0xF1000;
        let hle_vectors = vec![
            0x08, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x1A, 0x20, 0x21, 0x2F, 0x33,
        ];

        for vec in hle_vectors {
            let ivt_offset = (vec as usize) * 4;
            let handler_offset = (phys_addr & 0xFFFF) as u16;

            // Point IVT to F000:Offset
            self.bus.write_16(ivt_offset, handler_offset); // IP
            self.bus.write_16(ivt_offset + 2, 0xF000); // CS

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
        // If we zero those, the system dies.
        for i in 0x0500..0xFFFF {
            self.bus.ram[i] = 0;
        }

        // Re-install the HLE Interrupt Vectors
        self.install_bios_traps();

        // DOS "Underscore" cursor
        // High Byte (0x06) = Start Scanline, Low Byte (0x07) = End Scanline
        self.bus.write_16(0x0460, 0x0D0E);

        // Copy bytes
        for (i, byte) in shell_code.iter().enumerate() {
            self.bus.ram[start_addr + i] = *byte;
        }

        // Reset CPU State to "Boot" values
        self.cs = 0;
        self.ds = 0;
        self.es = 0;
        self.ss = 0;
        self.ip = 0x100; // Entry Point
        self.sp = 0xFF00; // Stack Pointer (Safe distance away)
        self.bp = 0;

        self.ax = 0;
        self.bx = 0;
        self.cx = 0;
        self.dx = 0;
        self.si = 0;
        self.di = 0;

        self.flags = CpuFlags::from_bits_truncate(0x0002); // Reset Flags
        self.state = CpuState::Running;

        self.bus.log_string("[SYSTEM] Shell Loaded. Ready.");
    }

    // Helper to read a u16 from a byte slice (Little Endian)
    #[allow(dead_code)]
    fn read_u16_le(data: &[u8], offset: usize) -> u16 {
        let low = data[offset] as u16;
        let high = data[offset + 1] as u16;
        (high << 8) | low
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

    // COM loader
    fn load_com(&mut self, bytes: &[u8], segment: Option<u16>) -> bool {
        let load_segment = segment.unwrap_or(0x1000);
        let start_offset = 0x100; // COM files always start at 100h

        // Clear 64KB of RAM segment for safety (simulating clean load)
        let phys_start_seg = self.get_physical_addr(load_segment, 0);
        for i in 0..0x10000 {
            if phys_start_seg + i < self.bus.ram.len() {
                self.bus.ram[phys_start_seg + i] = 0;
            }
        }

        // Re-install the HLE Interrupt Vectors
        self.install_bios_traps();

        // Load the file data at offset 0x100
        let phys_code_start = self.get_physical_addr(load_segment, start_offset);
        for (i, b) in bytes.iter().enumerate() {
            if phys_code_start + i < self.bus.ram.len() {
                self.bus.ram[phys_code_start + i] = *b;
            }
        }

        // COM State
        self.cs = load_segment;
        self.ds = load_segment;
        self.es = load_segment;
        self.ss = load_segment; // Stack is in the same segment
        self.ip = 0x100; // Entry Point
        self.sp = 0xFFFE; // End of segment (64KB - 2)

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
        let default_env = b"PATH=C:\\\0COMSPEC=COMMAND.COM\0\0";
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
            self.cs, self.ip
        ));
        // Simple heuristic: COM files own the 64KB segment.
        // Heap starts after that.
        self.heap_pointer = load_segment + 0x1000;
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

        let init_ss = u16::from_le_bytes([bytes[14], bytes[15]]);
        let init_sp = u16::from_le_bytes([bytes[16], bytes[17]]);
        let init_ip = u16::from_le_bytes([bytes[20], bytes[21]]);
        let init_cs = u16::from_le_bytes([bytes[22], bytes[23]]);
        let reloc_table_offset = u16::from_le_bytes([bytes[24], bytes[25]]) as usize;
        let reloc_count = u16::from_le_bytes([bytes[6], bytes[7]]) as usize;

        // Clear RAM (Only if starting fresh at 0x1000, probably shouldn't blindly wipe if nested)
        if segment.is_none() {
            for i in 0x500..self.bus.ram.len() {
                self.bus.ram[i] = 0;
            }
        }

        // Re-install the HLE Interrupt Vectors
        self.install_bios_traps();

        let load_segment: u16 = segment.unwrap_or(0x1000);
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
        let image_data = &bytes[header_size..];

        for (i, &b) in image_data.iter().enumerate() {
            if image_start_phys + i < self.bus.ram.len() {
                self.bus.ram[image_start_phys + i] = b;
            }
        }

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

                if phys_addr + 2 <= self.bus.ram.len() {
                    // Read the existing 16-bit value
                    let val_low = self.bus.ram[phys_addr] as u16;
                    let val_high = self.bus.ram[phys_addr + 1] as u16;
                    let mut val = (val_high << 8) | val_low;

                    // PATCH: Add the actual start segment to the value
                    val = val.wrapping_add(relocation_base_segment);

                    // Write it back
                    self.bus.ram[phys_addr] = (val & 0xFF) as u8;
                    self.bus.ram[phys_addr + 1] = (val >> 8) as u8;
                }
            }
        }

        // Setup Registers
        self.ds = load_segment; // Point to PSP
        self.es = load_segment;

        // CS/SS are relative to the Image Start (relocation_base_segment)
        self.cs = relocation_base_segment.wrapping_add(init_cs);
        self.ss = relocation_base_segment.wrapping_add(init_ss);
        self.ip = init_ip;
        self.sp = init_sp;

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
        let default_env = b"PATH=C:\\\0COMSPEC=COMMAND.COM\0\0";
        for (i, &b) in default_env.iter().enumerate() {
            self.bus.write_8(env_phys + i, b);
        }

        self.bus.write_16(psp_phys + 0x2C, env_seg);
        self.current_psp = load_segment;

        self.bus.log_string(&format!(
            "[DOS] Loaded. Entry CS:IP = {:04X}:{:04X}",
            self.cs, self.ip
        ));

        // Calculate heap pointer (First free paragraph after image)
        // relocation_base_segment is where image starts.
        // Image length is bytes.len() - header_size
        let image_len = bytes.len() - header_size;
        let image_paras = (image_len + 15) / 16;
        self.heap_pointer = relocation_base_segment + image_paras as u16 + 1;

        self.bus
            .log_string(&format!("[DEBUG] Heap starts at {:04X}", self.heap_pointer));

        // Enable to do detailed debugging of exe programs
        //self.debug_qb_conversion = true;

        true
    }
}
