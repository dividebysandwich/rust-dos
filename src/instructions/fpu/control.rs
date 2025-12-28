use iced_x86::{Instruction, OpKind, Register};
use crate::cpu::{Cpu, FPU_TAG_EMPTY, FPU_TAG_VALID, FpuFlags};
use crate::f80::F80;
use crate::instructions::utils::calculate_addr;

pub fn fninit(cpu: &mut Cpu) {
    // Initialize FPU
    cpu.fpu_top = 0;
    // Clear stack for debug clarity
    cpu.fpu_stack = [F80::new(); 8];
    cpu.fpu_control = 0x037F;
    // Reset FPU status registers here.
    cpu.set_fpu_flags(FpuFlags::empty());
    // Clear stack
    for i in 0..8 {
        cpu.fpu_tags[i] = FPU_TAG_EMPTY;
    }
}

// FNCLEX: Clear FPU Exceptions
pub fn fnclex(cpu: &mut Cpu) {
    // This clears IE, DE, ZE, OE, UE, PE, SF, ES, and the Busy bit.
    // It leaves the TOP pointer and Condition Codes (C0-C3) untouched.
    cpu.set_fpu_flag(FpuFlags::EXCEPTIONS, false);
}

// FLDCW: Load Control Word from Memory
pub fn fldcw(cpu: &mut Cpu, instr: &Instruction) {
    let addr = calculate_addr(cpu, instr);
    let cw = cpu.bus.read_16(addr);
    cpu.fpu_control = cw;
}

// FNSTCW: Store Control Word
// Programs read this to modify rounding settings, then write it back with FLDCW.
pub fn fnstcw(cpu: &mut Cpu, instr: &Instruction) {
    let addr = calculate_addr(cpu, instr);
    cpu.bus.write_16(addr, cpu.fpu_control);
}

// FNSTSW: Store FPU Status Word (No Wait)
// Usually: FNSTSW AX  or  FNSTSW [mem]
pub fn fnstsw(cpu: &mut Cpu, instr: &Instruction) {
    let flags = cpu.get_fpu_flags();
    
    // FPU Top is usually stored in bits 11-13 of the Status Word.
    // But we store it separately in our CPU struct, so we need to combine them.
    let mut raw_bits = flags.bits();
    raw_bits = (raw_bits & !0x3800) | ((cpu.fpu_top as u16 & 0x07) << 11);

    if instr.op0_kind() == OpKind::Register {
        if instr.op0_register() == Register::AX {
            cpu.ax = raw_bits;
        }
    } else if instr.op0_kind() == OpKind::Memory {
        let addr = calculate_addr(cpu, instr);
        cpu.bus.write_16(addr, raw_bits);
    }
}

pub fn ffree(cpu: &mut Cpu, instr: &Instruction) {
    let reg_offset = instr.op0_register().number() - iced_x86::Register::ST0.number();
    let phys_idx = cpu.fpu_get_phys_index(reg_offset as usize);
    
    // Mark as EMPTY
    cpu.fpu_tags[phys_idx] = crate::cpu::FPU_TAG_EMPTY;
}

// FINCSTP: Increment Stack Top Pointer
// This simply rotates the stack pointer. It does NOT push/pop values or change tags.
pub fn fincstp(cpu: &mut Cpu) {
    cpu.fpu_top = (cpu.fpu_top.wrapping_add(1)) & 7;
}

// FDECSTP: Decrement Stack Top Pointer
pub fn fdecstp(cpu: &mut Cpu) {
    cpu.fpu_top = (cpu.fpu_top.wrapping_sub(1)) & 7;
}

// FSAVE / FNSAVE: Save FPU State
// Writes the 94-byte (108-byte in 32-bit mode) FPU Environment to memory.
// Initializes the FPU (Like FNINIT).
// This implements the 16-bit Protected/Real mode format (94 bytes).
//
// Layout (16-bit Real Mode):
// 00: Control Word (16)
// 02: Status Word (16)
// 04: Tag Word (16)
// 06: Instruction Pointer (Low)
// 08: Instruction Pointer (High) & Opcode
// 0A: Operand Pointer (Low)
// 0C: Operand Pointer (High)
// 0E: Register ST(0) ... ST(7) (10 bytes each * 8 = 80 bytes)
pub fn fnsave(cpu: &mut Cpu, instr: &Instruction) {
    let addr = calculate_addr(cpu, instr);
    
    // Construct Status Word (Flags + Top Ptr)
    let flags = cpu.get_fpu_flags();
    let sw = (flags.bits() & !0x3800) | ((cpu.fpu_top as u16 & 0x07) << 11);
    
    // Construct Tag Word
    // The x87 Tag Word uses 2 bits per register to indicate status:
    // 00 = Valid, 01 = Zero, 10 = Special (NaN/Inf), 11 = Empty
    // It is stored relative to physical registers 0..7
    let mut tag_word: u16 = 0;
    for i in 0..8 {
        let tag = if cpu.fpu_tags[i] == FPU_TAG_EMPTY {
            0b11
        } else {
            // Check value for 0.0 or Special if strictly required, 
            // but for simple emulation, 00 (Valid) is sufficient for non-empty.
            // (Real hardware checks the actual float value here)
            let val = cpu.fpu_stack[i];
            if val.is_zero() { 0b01 } 
            else if val.is_nan() || val.is_infinite() { 0b10 }
            else { 0b00 }
        };
        tag_word |= tag << (i * 2);
    }

    // Write Environment (14 bytes)
    cpu.bus.write_16(addr, cpu.fpu_control);      // 00: CW
    cpu.bus.write_16(addr + 2, sw);               // 02: SW
    cpu.bus.write_16(addr + 4, tag_word);         // 04: TW
    cpu.bus.write_16(addr + 6, 0); // IP Offset (Dummy)
    cpu.bus.write_16(addr + 8, 0); // CS Selector (Dummy)
    cpu.bus.write_16(addr + 10, 0); // Operand Offset (Dummy)
    cpu.bus.write_16(addr + 12, 0); // Operand Selector (Dummy)

    // Write Register Stack (80 bytes) starting at offset 14 (0x0E)
    // Written sequentially: ST(0), ST(1) ... NO! 
    // FNSAVE writes Physical Register 0 through Physical Register 7.
    let mut reg_addr = addr + 14;
    for i in 0..8 {
        let bytes = cpu.fpu_stack[i].get_bytes();
        for &b in bytes.iter() {
            cpu.bus.write_8(reg_addr, b);
            reg_addr += 1;
        }
    }

    // After saving, FSAVE initializes the FPU
    fninit(cpu);
}

// FRSTOR: Restore FPU State
// Reads the 94-byte buffer and restores Control, Status, Tags, and Registers.
pub fn frstor(cpu: &mut Cpu, instr: &Instruction) {
    let addr = calculate_addr(cpu, instr);

    // Load Environment
    cpu.fpu_control = cpu.bus.read_16(addr);
    let sw = cpu.bus.read_16(addr + 2);
    let tag_word = cpu.bus.read_16(addr + 4);

    // Decode Status Word
    cpu.fpu_top = ((sw >> 11) & 0x07) as usize;
    // Mask out the TOP bits before setting flags to avoid corruption
    let flags = FpuFlags::from_bits_truncate(sw & !0x3800);
    cpu.set_fpu_flags(flags);

    // Decode Tag Word
    for i in 0..8 {
        let tag = (tag_word >> (i * 2)) & 0x03;
        cpu.fpu_tags[i] = if tag == 0b11 { FPU_TAG_EMPTY } else { FPU_TAG_VALID };
    }

    // Load Registers (Physical 0..7)
    let mut reg_addr = addr + 14;
    for i in 0..8 {
        let mut bytes = [0u8; 10];
        for b in 0..10 {
            bytes[b] = cpu.bus.read_8(reg_addr);
            reg_addr += 1;
        }
        cpu.fpu_stack[i].set_bytes(&bytes);
    }
}