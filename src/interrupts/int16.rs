use crate::cpu::Cpu;

pub fn handle(cpu: &mut Cpu) {
    let ah = cpu.get_ah();
    if ah == 0x00 {
        if let Some(k) = cpu.bus.keyboard_buffer.pop_front() {
            cpu.ax = k;
        } else {
            // BLOCKING: Rewind IP to retry
            cpu.ip = cpu.ip.wrapping_sub(2);
        }
    } else {
        cpu.bus.log_string(&format!("[BIOS] Unhandled INT 16h AH={:02X}", ah));
    }
}