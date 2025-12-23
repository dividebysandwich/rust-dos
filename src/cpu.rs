use iced_x86::{Register, Instruction, OpKind, MemorySize};

use crate::bus::Bus;
use crate::cpu_instr::calculate_addr;
use crate::get_shell_code;

// Constants for Flag Bits
pub const FLAG_CF: u16 = 0x0001; // Carry
#[allow(dead_code)]
pub const FLAG_PF: u16 = 0x0004; // Parity (Rarely used but good to have)
pub const FLAG_AF: u16 = 0x0010; // Auxiliary (BCD math, rarely used)
pub const FLAG_ZF: u16 = 0x0040; // Zero
pub const FLAG_SF: u16 = 0x0080; // Sign
#[allow(dead_code)]
pub const FLAG_TF: u16 = 0x0100; // Trap (Debug)
#[allow(dead_code)]
pub const FLAG_IF: u16 = 0x0200; // Interrupt Enable
pub const FLAG_DF: u16 = 0x0400; // Direction
pub const FLAG_OF: u16 = 0x0800; // Overflow

pub struct Cpu {
    // General Purpose
    pub ax: u16,
    pub bx: u16,
    pub cx: u16,
    pub dx: u16,
    pub di: u16,
    pub si: u16,
    // Pointers & Segments
    pub bp: u16,
    pub sp: u16,
    pub cs: u16,
    pub ds: u16,
    pub es: u16,
    pub ss: u16,
    pub ip: u16,

    pub bus: Bus,
    pub flags: u16,
    pub state: CpuState,
}

#[derive(PartialEq)]
pub enum CpuState {
    Running,
    RebootShell,
}

impl Cpu {
    pub fn new() -> Self {
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
            ip: 0x100,
            bus: Bus::new(),
            flags: 0x0002, // Default Flag State, Bit 1 is always set
            state: CpuState::Running,
        }
    }

    // Helper to get a flag state
    pub fn get_flag(&self, mask: u16) -> bool {
        (self.flags & mask) != 0
    }

    // Helper to set/clear a flag
    pub fn set_flag(&mut self, mask: u16, value: bool) {
        if value {
            self.flags |= mask;
        } else {
            self.flags &= !mask;
        }
    }

    pub fn zflag(&self) -> bool {
        self.get_flag(FLAG_ZF)
    }

    #[allow(dead_code)]
    pub fn set_zflag(&mut self, val: bool) {
        self.set_flag(FLAG_ZF, val)
    }

    pub fn dflag(&self) -> bool {
        self.get_flag(FLAG_DF)
    }
    pub fn set_dflag(&mut self, val: bool) {
        self.set_flag(FLAG_DF, val)
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
pub fn read_op0(cpu: &mut Cpu, instr: &Instruction) -> (u16, Option<usize>, bool) {
    match instr.op0_kind() {
        // Handle Register Operand
        OpKind::Register => {
            let reg = instr.op0_register();
            // Use iced_x86 built-in check or your own helper
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

    // A helper to perform ADD and update all relevant flags
    pub fn alu_add_16(&mut self, dest: u16, src: u16) -> u16 {
        let (result, carry) = dest.overflowing_add(src);

        self.set_flag(FLAG_CF, carry);
        self.set_flag(FLAG_ZF, result == 0);
        self.set_flag(FLAG_SF, (result & 0x8000) != 0); // High bit set?

        // Overflow (Signed): if operands have same sign, but result has diff sign
        let op1_sign = (dest & 0x8000) != 0;
        let op2_sign = (src & 0x8000) != 0;
        let res_sign = (result & 0x8000) != 0;
        let overflow = (op1_sign == op2_sign) && (res_sign != op1_sign);
        self.set_flag(FLAG_OF, overflow);

        result
    }

    // A helper for SUB (and CMP)
    pub fn alu_sub_16(&mut self, dest: u16, src: u16) -> u16 {
        let (result, borrow) = dest.overflowing_sub(src);

        self.set_flag(FLAG_CF, borrow); // In SUB, CF acts as Borrow
        self.set_flag(FLAG_ZF, result == 0);
        self.set_flag(FLAG_SF, (result & 0x8000) != 0);

        // Overflow (Signed): operands diff sign, result diff sign from dest
        let op1_sign = (dest & 0x8000) != 0;
        let op2_sign = (src & 0x8000) != 0;
        let res_sign = (result & 0x8000) != 0;
        let overflow = (op1_sign != op2_sign) && (res_sign != op1_sign);
        self.set_flag(FLAG_OF, overflow);

        result
    }

    // Helper for 8-bit SUB/CMP
    pub fn alu_sub_8(&mut self, dest: u8, src: u8) -> u8 {
        let (result, borrow) = dest.overflowing_sub(src);

        self.set_flag(FLAG_CF, borrow);
        self.set_flag(FLAG_ZF, result == 0);
        self.set_flag(FLAG_SF, (result & 0x80) != 0); // Check Bit 7

        // 8-bit overflow (signed)
        let op1_sign = (dest & 0x80) != 0;
        let op2_sign = (src & 0x80) != 0;
        let res_sign = (result & 0x80) != 0;
        let overflow = (op1_sign != op2_sign) && (res_sign != op1_sign);
        self.set_flag(FLAG_OF, overflow);

        result
    }

    pub fn alu_add_8(&mut self, dest: u8, src: u8) -> u8 {
        let (result, carry) = dest.overflowing_add(src);

        self.set_flag(FLAG_CF, carry);
        self.set_flag(FLAG_ZF, result == 0);
        self.set_flag(FLAG_SF, (result & 0x80) != 0);

        // 8-bit overflow (signed)
        let op1_sign = (dest & 0x80) != 0;
        let op2_sign = (src & 0x80) != 0;
        let res_sign = (result & 0x80) != 0;
        let overflow = (op1_sign == op2_sign) && (res_sign != op1_sign);
        self.set_flag(FLAG_OF, overflow);

        result
    }

    // SBB 8-bit Helper
    pub fn alu_sbb_8(&mut self, dest: u8, src: u8) -> u8 {
        let carry_in = if self.get_flag(FLAG_CF) { 1 } else { 0 };

        // We perform the math using u16 to easily detect borrows
        let result_wide = (dest as u16)
            .wrapping_sub(src as u16)
            .wrapping_sub(carry_in as u16);
        let result = result_wide as u8;

        // Flags
        self.set_flag(FLAG_ZF, result == 0);
        self.set_flag(FLAG_SF, (result & 0x80) != 0);

        // Carry (Borrow) happens if the result wrapped (result_wide > 0xFF)
        self.set_flag(FLAG_CF, result_wide > 0xFF);

        // Overflow (Signed)
        // (Dest_Sign != Src_Sign) AND (Dest_Sign != Result_Sign)
        // Note: For SBB, this is an approximation that covers 99% of cases.
        let op1_sign = (dest & 0x80) != 0;
        let op2_sign = (src & 0x80) != 0;
        let res_sign = (result & 0x80) != 0;
        let overflow = (op1_sign != op2_sign) && (op1_sign != res_sign);
        self.set_flag(FLAG_OF, overflow);

        result
    }

    // SBB 16-bit Helper
    pub fn alu_sbb_16(&mut self, dest: u16, src: u16) -> u16 {
        let carry_in = if self.get_flag(FLAG_CF) { 1 } else { 0 };

        // Use u32 to capture borrows
        let result_wide = (dest as u32)
            .wrapping_sub(src as u32)
            .wrapping_sub(carry_in as u32);
        let result = result_wide as u16;

        self.set_flag(FLAG_ZF, result == 0);
        self.set_flag(FLAG_SF, (result & 0x8000) != 0);

        // Carry flag if we wrapped past 0
        self.set_flag(FLAG_CF, result_wide > 0xFFFF);

        let op1_sign = (dest & 0x8000) != 0;
        let op2_sign = (src & 0x8000) != 0;
        let res_sign = (result & 0x8000) != 0;
        let overflow = (op1_sign != op2_sign) && (op1_sign != res_sign);
        self.set_flag(FLAG_OF, overflow);

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

    pub fn load_shell(&mut self) {
        // Get the Code
        let shell_code = get_shell_code();

        // Load into RAM at CS:IP (0x0000:0x0100)
        // We use 0x100 because .COM files (and our shell) expect to run there.
        let start_addr = 0x100;

        // Optional: Zero out the first 64KB of RAM to prevent "Ghost" code
        // from previous programs interfering.
        for i in 0..0xFFFF {
            self.bus.ram[i] = 0;
        }

        // Reset Cursor Position (Col=0, Row=0) at Physical Address 0x0450
        self.bus.write_8(0x0450, 0x00); // Col
        self.bus.write_8(0x0451, 0x00); // Row
        
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

        self.flags = 0x0002; // Reset Flags
        self.state = CpuState::Running;

        self.bus.log_string("[SYSTEM] Shell Loaded. Ready.\n");
    }

    // Helper to read a u16 from a byte slice (Little Endian)
    #[allow(dead_code)]
    fn read_u16_le(data: &[u8], offset: usize) -> u16 {
        let low = data[offset] as u16;
        let high = data[offset + 1] as u16;
        (high << 8) | low
    }

    pub fn load_executable(&mut self, filename: &str) -> bool {
        // Find and Read the File
        let target_lower = filename.to_lowercase();
        let mut file_bytes = None;

        if let Ok(entries) = std::fs::read_dir(".") {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(name) = path.file_name() {
                    if name.to_string_lossy().to_lowercase() == target_lower {
                        if let Ok(bytes) = std::fs::read(path) {
                            file_bytes = Some(bytes);
                            break;
                        }
                    }
                }
            }
        }

        let bytes = match file_bytes {
            Some(b) => b,
            None => return false,
        };

        self.bus.log_string(&format!("[DOS] Loading {} ({} bytes)\n", filename, bytes.len()));

        // Check for EXE Signature ("MZ")
        if bytes.len() > 2 && bytes[0] == 0x4D && bytes[1] == 0x5A {
            return self.load_exe(&bytes);
        } else {
            return self.load_com(&bytes);
        }
    }

    // COM loader
    fn load_com(&mut self, bytes: &[u8]) -> bool {
        let load_segment = 0x1000;
        let start_offset = 0x100; // COM files always start at 100h

        // Clear 64KB of RAM segment for safety (simulating clean load)
        let phys_start_seg = self.get_physical_addr(load_segment, 0);
        for i in 0..0x10000 {
            if phys_start_seg + i < self.bus.ram.len() {
                self.bus.ram[phys_start_seg + i] = 0;
            }
        }
        
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
        self.ip = 0x100;        // Entry Point
        self.sp = 0xFFFE;       // End of segment (64KB - 2)

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

        self.bus.log_string(&format!("[DEBUG] Wrote PSP[06] = {:02X} at Phys {:05X}\n", 
            self.bus.read_8(psp_phys + 6), psp_phys + 6));

        self.bus.log_string(&format!("[DOS] Loaded COM file at {:04X}:{:04X}\n", self.cs, self.ip));
        true
    }

    // EXE loader
    pub fn load_exe(&mut self, bytes: &[u8]) -> bool {
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

        // Clear RAM
        for i in 0..self.bus.ram.len() {
            self.bus.ram[i] = 0;
        }

        let load_segment: u16 = 0x1000;
        let relocation_base_segment = load_segment + 0x10;

        // Load Binary
        // Safety check: ensure header doesn't point past EOF
        if header_size > bytes.len() {
            self.bus.log_string("[DOS] Invalid EXE: Header larger than file");
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
                let rel_offset = u16::from_le_bytes([bytes[offset_idx], bytes[offset_idx+1]]);
                let rel_seg = u16::from_le_bytes([bytes[offset_idx+2], bytes[offset_idx+3]]);
                
                // Calculate physical address of the value we need to patch
                // The target segment in the table is relative to the Image Start
                let target_seg = relocation_base_segment.wrapping_add(rel_seg);
                let phys_addr = self.get_physical_addr(target_seg, rel_offset);
                
                if phys_addr + 2 <= self.bus.ram.len() {
                    // Read the existing 16-bit value
                    let val_low = self.bus.ram[phys_addr] as u16;
                    let val_high = self.bus.ram[phys_addr+1] as u16;
                    let mut val = (val_high << 8) | val_low;
                    
                    // PATCH: Add the actual start segment to the value
                    val = val.wrapping_add(relocation_base_segment);
                    
                    // Write it back
                    self.bus.ram[phys_addr] = (val & 0xFF) as u8;
                    self.bus.ram[phys_addr+1] = (val >> 8) as u8;
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

        self.bus.log_string(&format!(
            "[DOS] Loaded. Entry CS:IP = {:04X}:{:04X}\n",
            self.cs, self.ip
        ));
        true
    }

}
