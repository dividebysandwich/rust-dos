use crate::cpu::Cpu;

pub fn handle(cpu: &mut Cpu) {
    let ah = cpu.get_ah();
    match ah {
        // AH = 00h: Read Key (Blocking)
        // AH = 10h: Read Extended Key (Blocking)
        0x00 | 0x10 => {
            if let Some(key_code) = cpu.bus.keyboard_buffer.pop_front() {
                // Key found: Return in AX
                cpu.ax = key_code;
            } else {
                // Buffer empty: BLOCK.
                // We need to rewind the execution to retry 'INT 16h'.
                // Since we are in an HLE Trap, the specific 'INT 16h' caller address 
                // is sitting on the top of the Stack (pushed by the CPU before jumping to the trap).
                
                // Stack Layout: [IP, CS, Flags] (Top down)
                // We need to modify the IP at [SS:SP].
                
                let sp = cpu.sp;
                let ss = cpu.ss;
                let stack_addr = cpu.get_physical_addr(ss, sp);

                // Read the return IP from the stack
                let ret_ip = cpu.bus.read_16(stack_addr);

                // Subtract 2 bytes (Size of 'INT 16h' instruction: CD 16)
                // This ensures that when we 'IRET' later, we land back on the INT 16 instruction.
                let retry_ip = ret_ip.wrapping_sub(2);

                // Write it back to the stack
                cpu.bus.write_16(stack_addr, retry_ip);
            }
        }

        // AH = 01h: Check Key Status (Non-Blocking)
        // Returns: ZF=1 if no key, ZF=0 if key waiting (and AX=Key)
        0x01 | 0x11 => {
            if let Some(&key_code) = cpu.bus.keyboard_buffer.front() {
                cpu.set_flag(crate::cpu::FLAG_ZF, false); // Key available
                cpu.ax = key_code; // Preview key (do not remove)
            } else {
                cpu.set_flag(crate::cpu::FLAG_ZF, true); // No key
            }
        }

        _ => {
            cpu.bus.log_string(&format!("[BIOS] Unhandled INT 16h AH={:02X}", ah));
        }
    }
}