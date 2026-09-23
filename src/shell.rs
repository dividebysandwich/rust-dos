use crate::cpu::Cpu;
use crate::disk::{DiskController, drive_letter};
use crate::interrupts::utils::read_asciiz_string;
use crate::video;

/// Private BOP vector the shell uses to hand a typed line to the emulator.
/// It is only ever executed inline from the shell code (never via the IVT),
/// so it can't collide with a real INT FFh or with the INT 2Fh multiplexer.
pub const SHELL_COMMAND_BOP: u8 = 0xFF;

/// A Tiny "OS" written in Machine Code. Reads keys into a buffer at offset 0x0200
/// On Enter, hands the line to the Rust shell via the SHELL_COMMAND_BOP trap.
/// Handles backspace visually and in buffer
#[rustfmt::skip] // keep one instruction per line
pub fn get_shell_code() -> Vec<u8> {
    vec![
        // ----------------------------------------------------
        // BOOTLOADER: Initialize Segments
        // ----------------------------------------------------
        // We are loaded at CS=0x0000, IP=0x0100 (see Cpu::load_shell).
        // Set DS=ES=SS=CS so buffers and stack live in the shell segment.
        0x8C, 0xC8, // MOV AX, CS (Copy CS to AX)
        0x8E, 0xD8, // MOV DS, AX
        0x8E, 0xC0, // MOV ES, AX
        0x8E, 0xD0, // MOV SS, AX
        0xBC, 0x00, 0xFF, // MOV SP, 0xFF00
        // ----------------------------------------------------
        // SHELL LOOP START
        // ----------------------------------------------------
        // Label: PROMPT_START (0x010B)
        // 1. Print the current drive and ":\"
        0xB4, 0x19, 0xCD, 0x21, // MOV AH, 19h, INT 21h (AL = drive, 0=A)
        0x04, 0x41, // ADD AL, 'A'
        0xB4, 0x0E, 0xCD, 0x10, // MOV AH, 0Eh, INT 10h
        0xB0, 0x3A, 0xCD, 0x10, // MOV AL, ':', INT 10h
        0xB0, 0x5C, 0xCD, 0x10, // MOV AL, '\', INT 10h
        // 2. Get Current Directory (INT 21h, AH=47h)
        // Returns null-terminated string at DS:SI (we use 0x0300 as buffer)
        // DL = Drive (0=Default/C)
        0xB4, 0x47, // MOV AH, 47h
        0xB2, 0x00, // MOV DL, 0
        0xBE, 0x00, 0x03, // MOV SI, 0x0300
        0xCD, 0x21, // INT 21h
        // 3. Print CWD Loop
        0xBE, 0x00, 0x03, // MOV SI, 0x0300 (Reset SI to start of buffer)
        // Label: PRINT_LOOP
        0xAC, // LODSB (AL = [SI], SI++)
        0x08, 0xC0, // OR AL, AL
        0x74, 0x06, // JZ PRINT_DONE (+6 bytes to '>')
        0xB4, 0x0E, // MOV AH, 0Eh
        0xCD, 0x10, // INT 10h
        0xEB, 0xF5, // JMP PRINT_LOOP (-11 bytes)
        // Label: PRINT_DONE
        // 4. Print ">"
        0xB4, 0x0E, // MOV AH, 0Eh (SafeGuard: ensure AH is 0E)
        0xB0, 0x3E, 0xCD, 0x10, // MOV AL, '>', INT 10h
        // Reset Buffer Pointer
        0xBE, 0x00, 0x02, // MOV SI, 0x0200 (Buffer Start)
        // ----------------------------------------------------
        // INPUT LOOP (Wait for keys)
        // ----------------------------------------------------
        // Label: WAIT_KEY
        0xB4, 0x00, // MOV AH, 00h
        0xCD, 0x16, // INT 16h (Wait for Key)
        // Check ENTER (0x0D)
        0x3C, 0x0D, // CMP AL, 0x0D
        // Jump +36 bytes (0x24) to skip normal char handling AND backspace handling
        0x74, 0x24, // JE EXECUTE
        // Check BACKSPACE (0x08)
        0x3C, 0x08, // CMP AL, 0x08
        0x74, 0x09, // JE HANDLE_BACKSPACE (+9 bytes)
        // Normal Character
        0xB4, 0x0E, // MOV AH, 0Eh (Teletype Output)
        0xCD, 0x10, // INT 10h (Print Char)
        0x88, 0x04, // MOV [SI], AL (Store in Buffer)
        0x46, // INC SI (Advance Pointer)
        0xEB, 0xEB, // JMP WAIT_KEY (-21 bytes)
        // ----------------------------------------------------
        // BACKSPACE HANDLER
        // ----------------------------------------------------
        // Check Boundary (Start of Buffer)
        0x81, 0xFE, 0x00, 0x02, // CMP SI, 0x0200
        0x74, 0xE5, // JE WAIT_KEY (-27 bytes) (If empty, just wait)
        // Perform Visual Backspace
        0x4E, // DEC SI (Move Pointer Back)
        0xB4, 0x0E, // MOV AH, 0Eh
        0xB0, 0x08, 0xCD, 0x10, // Print Backspace
        0xB0, 0x20, 0xCD, 0x10, // Print Space
        0xB0, 0x08, 0xCD, 0x10, // Print Backspace
        0xEB, 0xD4, // JMP WAIT_KEY (-44 bytes)
        // ----------------------------------------------------
        // EXECUTE COMMAND
        // ----------------------------------------------------
        0xC6, 0x04, 0x00, // MOV BYTE PTR [SI], 0 (Null Terminate)
        // Print Newline
        0xB4, 0x0E, 0xB0, 0x0D, 0xCD, 0x10, // CR
        0xB0, 0x0A, 0xCD, 0x10, // LF
        0xBA, 0x00, 0x02, // MOV DX, 0x0200
        // The trap returns like an IRET, so build the frame an INT would:
        // FLAGS, CS, then the address of the JMP below.
        0x9C, // PUSHF
        0x0E, // PUSH CS
        0xB8, 0x82, 0x01, // MOV AX, 0x0182
        0x50, // PUSH AX
        0xFE, 0x38, SHELL_COMMAND_BOP, // Hand the line to the Rust shell
        // ----------------------------------------------------
        // RESET LOOP
        // ----------------------------------------------------
        0xEB, 0x87, // 0x0182: JMP PROMPT_START (-121 bytes)
    ]
}

/// SHELL_COMMAND_BOP handler: queue the ASCIIZ line at DS:DX for the main
/// loop to dispatch.
pub fn handle_command_bop(cpu: &mut Cpu) {
    // Safety: Clear buffer so we don't repeat commands
    cpu.bus.keyboard_buffer.clear();
    cpu.con_pending_scan = None;

    // Read Command from DS:DX (set by the shell code)
    let phys_addr = cpu.get_physical_addr(cpu.ds(), cpu.dx());
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
        cpu.pending_command = Some(clean_cmd);
    }
}

/// The DOS prompt for the current drive and directory, e.g. "D:\GAMES>".
pub fn prompt_string(disk: &DiskController) -> String {
    format!(
        "{}:\\{}>",
        drive_letter(disk.get_current_drive()),
        disk.get_current_directory()
    )
}

pub fn show_prompt(cpu: &mut Cpu) {
    // let col = cpu.bus.read_8(0x0450);
    // if col != 0 {
    //     video::print_string(cpu, "\r\n");
    // }

    let prompt = prompt_string(&cpu.bus.disk);
    video::print_string(cpu, &prompt);
}
