use crate::cpu::Cpu;

pub fn handle(cpu: &mut Cpu) {
    // 80x25 Color, No FPU, Floppy
    // Bits 4-5: 10 (80x25 Color)
    // Bit 1: 0 (No FPU)
    // Bit 0: 1 (Floppy)
    cpu.ax = 0b0000_0000_0010_0001;
}
