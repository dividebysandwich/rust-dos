use iced_x86::Instruction;

use super::operand::{effective_offset, mem_seg};
use crate::cpu::Cpu;

/// Linear address of the FPU's memory operand, which the instruction
/// dispatcher has checked for the whole operand (see `check_span`). The
/// FPU reads and writes it with `lin_read_8` and friends.
pub fn calculate_addr(cpu: &Cpu, instr: &Instruction) -> usize {
    let base = cpu.seg_cache(mem_seg(instr)).base;
    base.wrapping_add(effective_offset(cpu, instr)) as usize
}
