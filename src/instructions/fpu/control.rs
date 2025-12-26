use iced_x86::{Instruction, OpKind, Register};
use crate::cpu::Cpu;
use crate::instructions::utils::calculate_addr;

pub fn fninit(cpu: &mut Cpu) {
    // Initialize FPU
    cpu.fpu_top = 0;
    // Clear stack for debug clarity
    cpu.fpu_stack = [0.0; 8];
    // TODO reset FPU status registers here.
    cpu.fpu_status = 0;
    cpu.fpu_control = 0x037F;
}

// FNCLEX: Clear FPU Exceptions
pub fn fnclex(cpu: &mut Cpu) {
    // Clear FPU Exceptions
    cpu.fpu_status &= !0x00FF; 
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
    if instr.op0_kind() == OpKind::Register {
        if instr.op0_register() == Register::AX {
            cpu.ax = cpu.fpu_status;
        }
    } else if instr.op0_kind() == OpKind::Memory {
        let addr = calculate_addr(cpu, instr);
        cpu.bus.write_16(addr, cpu.fpu_status);
    }
}

pub fn ffree(cpu: &mut Cpu, instr: &Instruction) {
    let reg_offset = instr.op0_register().number() - iced_x86::Register::ST0.number();
    let phys_idx = cpu.fpu_get_phys_index(reg_offset as usize);
    
    // Mark as EMPTY
    cpu.fpu_tags[phys_idx] = crate::cpu::FPU_TAG_EMPTY;
}