use iced_x86::Register;
use crate::cpu::{Cpu, CpuState};
use crate::command::{run_dir_command, run_type_command, run_ver_command};
use crate::video::{print_string};
use super::utils::read_asciiz_string;
use super::int10; // For CLS

pub fn handle(cpu: &mut Cpu) {
    // Check if called by User Program (CS != 0)
    if cpu.cs != 0 {
        cpu.state = CpuState::RebootShell;
        return;
    }

    // Emulator Internal Shell Logic
    let ds = cpu.ds;
    let dx = cpu.dx;
    let addr = cpu.get_physical_addr(ds, dx);
    let raw_cmd = read_asciiz_string(&cpu.bus, addr);

    // Process Editing (Backspaces)
    let mut clean_chars = Vec::new();
    for c in raw_cmd.chars() {
        if c == '\x08' {
            clean_chars.pop();
        } else if c.is_ascii_graphic() || c == ' ' {
            clean_chars.push(c);
        }
    }
    let clean_cmd: String = clean_chars.into_iter().collect();

    cpu.bus.log_string(&format!("[SHELL DEBUG] Cleaned: {:?}", clean_cmd));
    print_string(cpu, "\r\n");

    let (command, args) = match clean_cmd.split_once(' ') {
        Some((c, a)) => (c, a.trim()),
        None => (clean_cmd.as_str(), ""),
    };

    if command.eq_ignore_ascii_case("DIR") {
        run_dir_command(cpu);
    } else if command.eq_ignore_ascii_case("CLS") {
        // Invoke BIOS Scroll Up via int10 handler directly
        cpu.set_reg8(Register::AH, 0x06);
        cpu.set_reg8(Register::AL, 0x00);
        cpu.set_reg8(Register::BH, 0x07);
        cpu.set_reg8(Register::CH, 0x00);
        cpu.set_reg8(Register::CL, 0x00);
        cpu.set_reg8(Register::DH, 0x18);
        cpu.set_reg8(Register::DL, 0x4F);
        
        int10::handle(cpu);

        cpu.bus.write_8(0x450, 0x00); // Col
        cpu.bus.write_8(0x451, 0x00); // Row
    } else if command.eq_ignore_ascii_case("TYPE") {
        if args.is_empty() {
            print_string(cpu, "Required parameter missing\r\n");
        } else {
            run_type_command(cpu, args);
        }
    } else if command.eq_ignore_ascii_case("EXIT") {
        cpu.bus.log_string("[SHELL] Exiting Emulator...");
        std::process::exit(0);
    } else if command.eq_ignore_ascii_case("VER") || command.eq_ignore_ascii_case("VERSION") {
        run_ver_command(cpu);
    } else if command.is_empty() {
        // Do nothing
    } else {
        let filename = command.to_string();
        if !filename.contains('.') {
            if cpu.load_executable(&format!("{}.com", command)) { return; }
            if cpu.load_executable(&format!("{}.exe", command)) { return; }
        } else {
            if cpu.load_executable(&filename) { return; }
        }
        print_string(cpu, "Bad command or file name.\r\n");
    }

    print_string(cpu, "C:\\>");
}