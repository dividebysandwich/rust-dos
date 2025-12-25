use crate::cpu::{Cpu, CpuState};
use crate::video::print_string;

pub mod int00;
pub mod int10;
pub mod int11;
pub mod int12;
pub mod int15;
pub mod int16;
pub mod int1a;
pub mod int20;
pub mod int21;
pub mod int33;
pub mod utils;

pub fn handle_interrupt(cpu: &mut Cpu, vector: u8) {
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
        0x33 => int33::handle(cpu),
        0x4C => {
            cpu.bus.log_string("[DOS] Program Exited. Rebooting Shell...");
            cpu.state = CpuState::RebootShell;
        }
        _ => {
            cpu.bus.log_string(&format!("[CPU] Unhandled Interrupt Vector {:02X}", vector));
        }
    }
}