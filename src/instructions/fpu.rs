use iced_x86::{Instruction, Mnemonic};
use crate::cpu::Cpu;
use super::utils::calculate_addr;

pub fn handle(cpu: &mut Cpu, instr: &Instruction) {
    match instr.mnemonic() {
        Mnemonic::Fninit => {
            // Initialize FPU
            // Set Control Word to default (0x037F)
            // TODO reset FPU status registers here.
            // cpu.fpu_status = 0;
            // cpu.fpu_control = 0x037F;
        }
        Mnemonic::Fnclex => {
            // Clear FPU Exceptions
            // cpu.fpu_status &= !0x00FF; 
        }
        Mnemonic::Fldcw => {
            // Load Control Word from Memory
            let addr = calculate_addr(cpu, instr);
            let cw = cpu.bus.read_16(addr);
            // cpu.fpu_control = cw;
            // cpu.bus.log_string(&format!("[FPU] FLDCW loaded Control Word: {:04X}", cw));
        }
        _ => {}
    }
}