use crate::cpu::{Cpu, CpuState, CpuFlags};

pub mod int00;
pub mod int10;
pub mod int11;
pub mod int12;
pub mod int15;
pub mod int16;
pub mod int1a;
pub mod int20;
pub mod int21;
pub mod int2f;
pub mod int33;
pub mod utils;


/// Called when the CPU encounters "INT XX" instruction.
/// This simulates the REAL hardware sequence: Push Flags/CS/IP -> Jump to IVT.
pub fn handle_interrupt(cpu: &mut Cpu, vector: u8) {
    // Read IVT
    let ivt_addr = (vector as usize) * 4;
    let new_ip = cpu.bus.read_16(ivt_addr);
    let new_cs = cpu.bus.read_16(ivt_addr + 2);

    if new_cs == 0 && new_ip == 0 {
        cpu.bus.log_string(&format!("[CPU] Null Interrupt {:02X}", vector));
        return;
    }

    // Push State (Simulate Hardware)
    cpu.push(cpu.flags.bits());
    cpu.push(cpu.cs);
    cpu.push(cpu.ip);

    // Jump
    cpu.cs = new_cs;
    cpu.ip = new_ip;
    
    // Disable Interrupts
    cpu.set_cpu_flag(CpuFlags::IF, false);
    cpu.set_cpu_flag(CpuFlags::TF, false);
}

pub fn handle_hle(cpu: &mut Cpu, vector: u8) {
    match vector {
        0x00 => int00::handle(cpu),
        0x10 => int10::handle(cpu),
        0x11 => int11::handle(cpu),
        0x12 => int12::handle(cpu),
        0x15 => int15::handle(cpu),
        0x16 => int16::handle(cpu),
        0x1A => int1a::handle(cpu),
        0x20 => int20::handle(cpu),
        0x21 => int21::handle(cpu),
        0x28 => { /* Idle Interrupt - Do nothing */ },
        0x2A => { /* DOS Timer Tick - Do nothing for now */ },
        0x2F => int2f::handle(cpu),
        0x33 => int33::handle(cpu),
        0x34 | 0x35 | 0x36 | 0x37 | 0x38 | 0x39 | 0x3A | 0x3B | 0x3C | 0x3D | 0x3E | 0x3F => {
             /* FPU Vector - IRET */ 
             // TODO: Implement FPU
        }
        0x4C => {
            cpu.bus.log_string("[DOS] Program Exited. Rebooting Shell...");
            cpu.state = CpuState::RebootShell;
        }
        _ => {
            cpu.bus.log_string(&format!("[CPU] Unhandled HLE Interrupt Vector {:02X}", vector));
        }
    }
}
