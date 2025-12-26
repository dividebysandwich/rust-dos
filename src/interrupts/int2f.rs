use crate::cpu::Cpu;
use crate::interrupts::utils::read_asciiz_string;

pub fn handle(cpu: &mut Cpu) {
    // Safety: Clear buffer so we don't repeat commands
    cpu.bus.keyboard_buffer.clear();

    // Read Command from DS:DX (set by shell.rs)
    let ds = cpu.ds;
    let dx = cpu.dx;
    let phys_addr = cpu.get_physical_addr(ds, dx);
    let raw_cmd = read_asciiz_string(&cpu.bus, phys_addr);

    // Clean String
    let mut clean_chars = Vec::new();
    for c in raw_cmd.chars() {
        if c == '\x08' {
            clean_chars.pop();
        } else if c.is_ascii_graphic() || c == ' ' {
            clean_chars.push(c);
        }
    }
    let clean_cmd: String = clean_chars.into_iter().collect();

    // Queue for Main Loop
    if !clean_cmd.is_empty() {
        cpu.bus.log_string(&format!("[INT2F] Queuing Command: {}", clean_cmd));
        cpu.pending_command = Some(clean_cmd);
    }
}