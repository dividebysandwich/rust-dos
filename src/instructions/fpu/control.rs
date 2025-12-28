use iced_x86::{Instruction, OpKind, Register};
use crate::cpu::{Cpu, FPU_TAG_EMPTY, FpuFlags};
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