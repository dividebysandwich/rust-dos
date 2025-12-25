use crate::cpu::Cpu;

pub fn handle(cpu: &mut Cpu) {
    if cpu.ax == 0x0000 {
        cpu.ax = 0x0000; // No Mouse
        cpu.bx = 0;
    } else {
        cpu.bus.log_string(&format!("[MOUSE] Unhandled Call Int 0x33 AX={:04X}", cpu.ax));
    }
}