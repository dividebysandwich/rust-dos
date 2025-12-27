use crate::cpu::{Cpu, CpuFlags};

pub fn handle(cpu: &mut Cpu) {
    let ah = cpu.get_ah();
    match ah {
        0x88 => {
            // Extended Memory (16MB total -> 15MB extended)
            cpu.ax = 15360; 
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }
        0x86 => {
            // Wait (Microseconds)
            let micros = ((cpu.cx as u64) << 16) | (cpu.dx as u64);
            std::thread::sleep(std::time::Duration::from_micros(micros));
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }
        _ => cpu.bus.log_string(&format!("[BIOS] Unhandled INT 15h AH={:02X}", ah)),
    }
}