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
        // SHELL: Setup
        // ----------------------------------------------------
        0xBE, 0x00, 0x02, // MOV SI, 0x0200 (Buffer Start)
        // ----------------------------------------------------
        // MAIN INPUT LOOP (Offset 0x10E in memory)
        // ----------------------------------------------------
        // Reset AH to 00 (Read Key Mode)
        0xB4, 0x00, // MOV AH, 00h
        0xCD, 0x16, // INT 16h (Wait for Key)
        // Check ENTER (0x0D)
        0x3C, 0x0D, // CMP AL, 0x0D
        0x74, 0x1A, // JE EXECUTE (+26 bytes)
        // Check BACKSPACE (0x08)
        0x3C, 0x08, // CMP AL, 0x08
        0x74, 0x09, // JE HANDLE_BACKSPACE (+9 bytes)
        // Normal Character
        0xB4, 0x0E, // MOV AH, 0Eh (Teletype Output)
        0xCD, 0x10, // INT 10h (Print Char)
        0x88, 0x04, // MOV [SI], AL (Store in Buffer)
        0x46, // INC SI (Advance Pointer)
        // JMP LOOP_START
        // -21 bytes (0xEB) ensures we hit MOV AH, 00
        0xEB, 0xEB,
        // ----------------------------------------------------
        // BACKSPACE HANDLER
        // ----------------------------------------------------
        // Check Boundary
        0x81, 0xFE, 0x00, 0x02, // CMP SI, 0x0200
        0x74, 0xE5, // JE LOOP_START (Empty buffer? Restart)
        // Perform Backspace
        0x4E, // DEC SI (Move Pointer Back)
        0xB4, 0x0E, // MOV AH, 0Eh
        0xCD, 0x10, // INT 10h (Print BS)
        // -34 bytes (0xDE) ensures we hit MOV AH, 00
        0xEB, 0xDE, // JMP LOOP_START
        // ----------------------------------------------------
        // EXECUTE COMMAND
        // ----------------------------------------------------
        0xC6, 0x04, 0x00, // MOV BYTE PTR [SI], 0
        0xBA, 0x00, 0x02, // MOV DX, 0x0200
        0xCD, 0x20, // INT 20h
        // ----------------------------------------------------
        // RESET
        // ----------------------------------------------------
        0xBE, 0x00, 0x02, // MOV SI, 0x0200
        0xEB, 0xD1, // JMP LOOP_START (-47 bytes)
    ]
}
