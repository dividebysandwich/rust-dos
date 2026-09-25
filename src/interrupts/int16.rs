use crate::cpu::{Cpu, CpuFlags};

// BDA Address for Keyboard Shift Flags
const BDA_SHIFT_FLAGS: usize = 0x0417;

pub fn handle(cpu: &mut Cpu) {
    let ah = cpu.get_ah();
    match ah {
        // AH = 00h: Read Key (Blocking)
        // AH = 10h: Read Extended Key (Blocking)
        0x00 | 0x10 => {
            if ah == 0x00 {
                drop_enhanced_keys(cpu);
            }
            if let Some(key_code) = cpu.bus.keyboard_buffer.pop_front() {
                // Key found: Return in AX
                cpu.set_ax(if ah == 0x00 { plain_keystroke(key_code) } else { key_code });
            } else {
                // Buffer empty: wait in the BIOS for a keystroke.
                cpu.hle_wait();
            }
        }

        // AH = 01h: Check Key Status (Non-Blocking)
        // Returns: ZF=1 if no key, ZF=0 if key waiting (and AX=Key)
        0x01 => {
            drop_enhanced_keys(cpu);
            if let Some(&key_code) = cpu.bus.keyboard_buffer.front() {
                cpu.set_cpu_flag(CpuFlags::ZF, false);
                cpu.set_ax(plain_keystroke(key_code));
            } else {
                cpu.set_cpu_flag(CpuFlags::ZF, true);
            }
        }
        0x11 => {
            if let Some(&key_code) = cpu.bus.keyboard_buffer.front() {
                cpu.set_cpu_flag(CpuFlags::ZF, false); // Key available
                cpu.set_ax(key_code); // Preview key (do not remove)
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
            let key = cpu.cx();
            // Cap buffer at 16 keys to emulate BIOS buffer size limit
            if cpu.bus.keyboard_buffer.len() < crate::keyboard::BIOS_BUFFER_KEYS {
                cpu.bus.keyboard_buffer.push_back(key);
                cpu.set_reg8(iced_x86::Register::AL, 0); // Success
            } else {
                cpu.set_reg8(iced_x86::Register::AL, 1); // Full
            }
        }

        _ => {
            cpu.bus
                .log_string(&format!("[BIOS] Unhandled INT 16h AH={:02X}", ah));
        }
    }
}

/// Whether only the enhanced keyboard's functions (AH=10h and 11h) return
/// a keystroke: those of F11 and F12 and of the combinations the older
/// keyboards had no code for (Ctrl+Up, Alt+Tab and the like), whose codes
/// are above 84h. The older functions skip them, as an AT BIOS does.
fn is_enhanced(key: u16) -> bool {
    key >> 8 > 0x84 || (key & 0xFF == 0xF0 && key >> 8 != 0)
}

/// A keystroke as the older functions return it: the grey keys' E0h
/// becomes 00h, as the keys of the keypad they copy have.
fn plain_keystroke(key: u16) -> u16 {
    if key >> 8 != 0 && key & 0xFF == 0xE0 { key & 0xFF00 } else { key }
}

/// Throw away the enhanced keystrokes at the front of the buffer, which
/// AH=00h and 01h skip.
fn drop_enhanced_keys(cpu: &mut Cpu) {
    while cpu.bus.keyboard_buffer.front().is_some_and(|&key| is_enhanced(key)) {
        cpu.bus.keyboard_buffer.pop_front();
    }
}
