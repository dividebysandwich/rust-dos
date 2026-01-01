use crate::cpu::{Cpu, CpuFlags};
use iced_x86::Register;

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
        0xC0 => {
            // Get System Configuration
            // Return ES:BX pointing to Config Table (8 bytes)
            // We'll construct a dummy table at F000:E800 (Phys FE800)
            let table_seg = 0xF000;
            let table_off = 0xE800;
            let phys_addr = 0xFE800;

            // Byte 0-1: Length (8)
            cpu.bus.write_16(phys_addr, 0x0008);
            // Byte 2: Model (FC = AT)
            cpu.bus.write_8(phys_addr + 2, 0xFC);
            // Byte 3: Submodel (01 = AT)
            cpu.bus.write_8(phys_addr + 3, 0x01);
            // Byte 4: BIOS Revision (0)
            cpu.bus.write_8(phys_addr + 4, 0x00);
            // Byte 5: Feature Info 1 (0)
            cpu.bus.write_8(phys_addr + 5, 0x00);
            // Byte 6-9: Reserved/Features
            cpu.bus.write_8(phys_addr + 6, 0x00);
            cpu.bus.write_8(phys_addr + 7, 0x00);

            cpu.es = table_seg;
            cpu.bx = table_off;
            cpu.set_reg8(Register::AH, 0);
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }
        _ => cpu
            .bus
            .log_string(&format!("[BIOS] Unhandled INT 15h AH={:02X}", ah)),
    }
}
