use iced_x86::Instruction;

use super::operand::{effective_offset, mem_seg};
use crate::cpu::Cpu;

/// Physical address of the memory operand, for the FPU, which accesses
/// memory directly through the bus. There is no limit check.
pub fn calculate_addr(cpu: &Cpu, instr: &Instruction) -> usize {
    let base = cpu.seg_cache(mem_seg(instr)).base;
    cpu.translate(base.wrapping_add(effective_offset(cpu, instr))) as usize
}
