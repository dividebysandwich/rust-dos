use crate::cpu::Cpu;

pub fn handle(cpu: &mut Cpu) {
    cpu.set_ax(640); // KB
}