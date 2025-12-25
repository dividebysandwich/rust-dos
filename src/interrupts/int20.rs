use crate::cpu::{Cpu, CpuState};
use crate::command::CommandDispatcher;
use crate::video::{print_string};
use super::utils::read_asciiz_string;

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

    let dispatcher = CommandDispatcher::new(); 
    
    if dispatcher.dispatch(cpu, command, args) {
        // Build-in command was found and executed
    } else if command.is_empty() {
        // Ignore empty
    } else {
        // Not a command? Try to load as Executable (.COM/.EXE)
        let filename = command.to_string();
        let loaded = if !filename.contains('.') {
             cpu.load_executable(&format!("{}.com", command)) 
             || cpu.load_executable(&format!("{}.exe", command))
        } else {
             cpu.load_executable(&filename)
        };

        if !loaded {
            print_string(cpu, "Bad command or file name.\r\n");
        }
    }

    print_string(cpu, "C:\\>");
}