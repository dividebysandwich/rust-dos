use iced_x86::{Instruction, Register};
use crate::cpu::Cpu;

pub fn is_8bit_reg(reg: Register) -> bool {
    reg.is_gpr8()
}

// Helper: Determine which segment register to use
fn get_segment(cpu: &Cpu, instr: &Instruction) -> u16 {
    match instr.segment_prefix() {
        Register::ES => cpu.es,
        Register::CS => cpu.cs,
        Register::SS => cpu.ss,
        Register::DS => cpu.ds,
        Register::FS => 0,
        Register::GS => 0,
        _ => {
            // Default rules: BP/SP use SS, others use DS
            let base = instr.memory_base();
            if base == Register::BP || base == Register::SP || base == Register::EBP || base == Register::ESP {
                cpu.ss
            } else {
                cpu.ds
            }
        }
    }
}

// Helper: Calculate ONLY the Offset (Effective Address)
pub fn get_effective_addr(cpu: &Cpu, instr: &Instruction) -> u16 {
    // Get Base
    let base = if instr.memory_base() != Register::None {
        cpu.get_reg16(instr.memory_base()) as u32
    } else {
        0
    };

    // Get Index * Scale
    let index = if instr.memory_index() != Register::None {
        let val = cpu.get_reg16(instr.memory_index()) as u32;
        let scale = instr.memory_index_scale() as u32;
        val * scale
    } else {
        0
    };

    // Displacement
    let displacement = instr.memory_displacement32();

    // Wrap at 16-bit
    (base.wrapping_add(index).wrapping_add(displacement)) as u16
}

// Helper: Calculate Full Physical Address (Segment:Offset -> Linear)
pub fn calculate_addr(cpu: &Cpu, instr: &Instruction) -> usize {
    let segment = get_segment(cpu, instr);
    let offset = get_effective_addr(cpu, instr);
    cpu.get_physical_addr(segment, offset)
}

