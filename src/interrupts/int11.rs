use crate::cpu::Cpu;

pub fn handle(cpu: &mut Cpu) {
    // 80x25 Color, FPU, Floppy
    cpu.ax = 0b0000_0000_0010_0011; 
}