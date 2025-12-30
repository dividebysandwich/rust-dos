use crate::cpu::Cpu;
use crate::video;

/// A Tiny "OS" written in Machine Code. Reads keys into a buffer at offset 0x0200
/// On Enter, calls INT 20h (Our Rust Shell). Handles backspace visually and in buffer
pub fn get_shell_code() -> Vec<u8> {
    vec![
        // ----------------------------------------------------
        // BOOTLOADER: Initialize Segments
        // ----------------------------------------------------
        0x31, 0xC0, // XOR AX, AX
        0x8E, 0xD8, // MOV DS, AX
        0x8E, 0xC0, // MOV ES, AX
        0x8E, 0xD0, // MOV SS, AX
        0xBC, 0x00, 0xFF, // MOV SP, 0xFF00
        // ----------------------------------------------------
        // SHELL LOOP START
        // ----------------------------------------------------
        // Label: PROMPT_START
        // 1. Print "C:\"
        0xB4, 0x0E, // MOV AH, 0Eh
        0xB0, 0x43, 0xCD, 0x10, // MOV AL, 'C', INT 10h
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
        0xFE, 0x38, 0x2F, // INT 2Fh (Execute)
        // ----------------------------------------------------
        // RESET LOOP
        // ----------------------------------------------------
        0xEB, 0x94, // JMP PROMPT_START (-108 bytes)
    ]
}

pub fn show_prompt(cpu: &mut Cpu) {
    // let col = cpu.bus.read_8(0x0450);
    // if col != 0 {
    //     video::print_string(cpu, "\r\n");
    // }

    let cwd = cpu.bus.disk.get_current_directory();
    if cwd.is_empty() {
        video::print_string(cpu, "C:\\>");
    } else {
        video::print_string(cpu, &format!("C:\\{}>", cwd));
    }
}
