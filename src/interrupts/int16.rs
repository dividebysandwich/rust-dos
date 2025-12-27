use crate::cpu::{Cpu, CpuFlags};

// BDA Address for Keyboard Shift Flags
const BDA_SHIFT_FLAGS: usize = 0x0417;

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
                cpu.set_cpu_flag(CpuFlags::ZF, false); // Key available
                cpu.ax = key_code; // Preview key (do not remove)
            } else {
                cpu.set_cpu_flag(CpuFlags::ZF, true); // No key
            }
        }

        // AH = 02h: Get Shift Status
        // Returns AL = Shift Flag Byte (from BDA 0x0417)
        // Bit 0: Right Shift
        // Bit 1: Left Shift
        // Bit 2: Ctrl
        // Bit 3: Alt
        // Bit 4: Scroll Lock
        // Bit 5: Num Lock
        // Bit 6: Caps Lock
        // Bit 7: Insert
        0x02 => {
            let status = cpu.bus.read_8(BDA_SHIFT_FLAGS);
            cpu.set_reg8(iced_x86::Register::AL, status);
        }

        // AH = 05h: Store Key (Push to Buffer)
        // CX = Key (CH=Scan, CL=Ascii)
        // Returns AL=0 (Success), AL=1 (Buffer Full)
        0x05 => {
            let key = cpu.cx;
            // Cap buffer at 16 keys to emulate BIOS buffer size limit
            if cpu.bus.keyboard_buffer.len() < 16 {
                cpu.bus.keyboard_buffer.push_back(key);
                cpu.set_reg8(iced_x86::Register::AL, 0); // Success
            } else {
                cpu.set_reg8(iced_x86::Register::AL, 1); // Full
            }
        }

        _ => {
            cpu.bus.log_string(&format!("[BIOS] Unhandled INT 16h AH={:02X}", ah));
        }
    }
}