use iced_x86::Register;
use crate::cpu::Cpu;

pub fn handle(cpu: &mut Cpu) {
    let ah = cpu.get_ah();
    match ah {
        0x00 => {
            let elapsed_ms = cpu.bus.start_time.elapsed().as_millis();
            let ticks = (elapsed_ms as u64 * 182) / 10000;
            cpu.cx = (ticks >> 16) as u16;
            cpu.dx = (ticks & 0xFFFF) as u16;
            cpu.set_reg8(Register::AL, 0);
        }
        0x02 => { // Get Real-Time
            cpu.cx = 0; cpu.dx = 0;
            cpu.set_flag(crate::cpu::FLAG_CF, false);
        }
        0x04 => { // Get Date
            cpu.cx = 0x2000; cpu.dx = 0x0101;
            cpu.set_flag(crate::cpu::FLAG_CF, false);
        }
        _ => cpu.bus.log_string(&format!("[BIOS] Unhandled INT 1A AH={:02X}", ah)),
    }
}