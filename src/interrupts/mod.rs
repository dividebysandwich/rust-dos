use crate::cpu::{Cpu, CpuFlags, CpuState};
pub mod int00;
pub mod int06;
pub mod int08;
pub mod int09;
pub mod int10;
pub mod int11;
pub mod int12;
pub mod int13;
pub mod int15;
pub mod int16;
pub mod int1a;
pub mod int20;
pub mod int21;
pub mod int25;
pub mod int2f;
pub mod int33;
pub mod mscdex;
pub mod vbe;
pub mod utils;

/// Return from an HLE handler as its IRET would. Service interrupts hand
/// their results back in CF and ZF (and clear DF), but the handlers of the
/// hardware interrupts (IRQ 0-7, vectors 08h-0Fh) must restore the
/// interrupted code's flags exactly: games chain their timer and keyboard
/// ISRs to ours, and those can land between any compare and its jump.
///
/// INT 25h/26h return with a RETF instead, leaving the caller's flags on
/// the stack for it to pop, with interrupts enabled as DOS leaves them.
pub fn return_from_hle(cpu: &mut Cpu, vector: u8) {
    let hle_cf = cpu.get_cpu_flag(CpuFlags::CF);
    let hle_zf = cpu.get_cpu_flag(CpuFlags::ZF);

    let ip = cpu.pop();

    cpu.set_ip(ip);
    let cs = cpu.pop();
    cpu.set_cs(cs);
    let stacked = cpu.pop();
    if matches!(vector, 0x25 | 0x26) {
        cpu.push(stacked);
    }
    let flags = CpuFlags::from_bits_truncate(stacked as u32);
    cpu.set_cpu_flags(flags);
    if matches!(vector, 0x25 | 0x26) {
        cpu.set_cpu_flag(CpuFlags::IF, true);
        cpu.set_cpu_flag(CpuFlags::TF, false);
    }

    if !(0x08..=0x0F).contains(&vector) {
        cpu.set_cpu_flag(CpuFlags::DF, false);
        cpu.set_cpu_flag(CpuFlags::CF, hle_cf);
        cpu.set_cpu_flag(CpuFlags::ZF, hle_zf);
    }
}

/// Inline emulator services (`FE 39 vv`), which ROM code calls in the middle
/// of a routine and which return by continuing with the next instruction.
pub fn handle_inline_bop(cpu: &mut Cpu, service: u8) {
    match service {
        crate::bios::SERVICE_TIMER_TICK => int08::tick(cpu),
        crate::bios::SERVICE_XMS => crate::xms::call(cpu),
        crate::bios::SERVICE_POST => crate::bios::post(cpu),
        crate::bios::SERVICE_CD_STRATEGY => mscdex::strategy(cpu),
        crate::bios::SERVICE_CD_INTERRUPT => mscdex::interrupt(cpu),
        crate::bios::SERVICE_VBE_WINDOW => vbe::window_call(cpu),
        crate::bios::SERVICE_IO_WAIT => crate::diskio::wait(cpu),
        _ => cpu.bus.log_string(&format!(
            "[CPU] Unknown inline emulator service {:02X}",
            service
        )),
    }
}

pub fn handle_hle(cpu: &mut Cpu, vector: u8) {
    match vector {
        0x00 => int00::handle(cpu),
        0x06 => int06::handle(cpu),
        0x08 => int08::handle(cpu),
        0x09 => int09::handle(cpu),
        0x10 => int10::handle(cpu),
        0x11 => int11::handle(cpu),
        0x12 => int12::handle(cpu),
        0x15 => int15::handle(cpu),
        0x16 => int16::handle(cpu),
        0x1A => int1a::handle(cpu),
        0x20 => int20::handle(cpu),
        0x21 => int21::handle(cpu),
        0x25 => int25::handle(cpu, false),
        0x26 => int25::handle(cpu, true),
        0x28 => { /* Idle Interrupt - Do nothing */ }
        0x2A => { /* DOS Timer Tick - Do nothing for now */ }
        0x13 => int13::handle(cpu),
        0x14 => {
            cpu.bus.log_string("[BIOS] Unhandled INT 14h (Serial)");
            cpu.set_reg8(iced_x86::Register::AH, 0x80);
        } // Time out
        0x17 => {
            cpu.bus.log_string("[BIOS] Unhandled INT 17h (Printer)");
            cpu.set_reg8(iced_x86::Register::AH, 0x29);
        } // IO Error, Selected, Out of Paper
        0x2F => int2f::handle(cpu),
        0x33 => int33::handle(cpu),
        0x34 | 0x35 | 0x36 | 0x37 | 0x38 | 0x39 | 0x3A | 0x3B | 0x3C | 0x3D | 0x3E | 0x3F => {
            /* FPU Vector - IRET */
            // TODO: Implement FPU
        }
        crate::shell::SHELL_COMMAND_BOP => crate::shell::handle_command_bop(cpu),
        0x4C => {
            cpu.bus
                .log_string("[DOS] Program Exited. Rebooting Shell...");
            cpu.state = CpuState::RebootShell;
        }
        _ => {
            cpu.bus.log_string(&format!(
                "[CPU] Unhandled HLE Interrupt Vector {:02X}",
                vector
            ));
        }
    }
}
