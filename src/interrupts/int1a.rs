use iced_x86::Register;
use crate::cpu::{Cpu, CpuFlags};

pub fn handle(cpu: &mut Cpu) {
    let ah = cpu.get_ah();
    match ah {
        0x00 => {
            // The tick count the timer interrupt maintains, as a real BIOS
            // does, so it agrees with programs reading 0040:006C directly.
            cpu.set_cx(cpu.bus.read_16(0x046E));
            cpu.set_dx(cpu.bus.read_16(0x046C));
            // AL = midnight flag, cleared by the read
            let midnight = cpu.bus.read_8(0x0470);
            cpu.bus.write_8(0x0470, 0);
            cpu.set_reg8(Register::AL, midnight);
        }
        0x02 => { // Get Real-Time
            cpu.set_cx(0); cpu.set_dx(0);
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }
        0x04 => { // Get Date
            cpu.set_cx(0x2000); cpu.set_dx(0x0101);
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }
        _ => cpu.bus.log_string(&format!("[BIOS] Unhandled INT 1A AH={:02X}", ah)),
    }
}