use rust_dos::cpu::{Cpu, CpuFlags};
use rust_dos::instructions::execute_instruction;
use iced_x86::{Decoder, DecoderOptions, Instruction};

fn run_code(cpu: &mut Cpu, code: &[u8]) {
    // Ensure IP starts at 0x100 (COM file start)
    cpu.ip = 0x100;

    // Safety limit to prevent infinite loops in tests (e.g., JMP $)
    let mut max_instructions = 100; 

    loop {
        if max_instructions == 0 {
            break;
        }
        max_instructions -= 1;

        // Calculate where we are in the byte array
        // We assume the code is loaded at 0x100.
        let offset = (cpu.ip as usize).wrapping_sub(0x100);

        // Check if we've run off the end of the code
        if offset >= code.len() {
            break;
        }

        // Decode ONE instruction at the current IP
        let mut decoder = Decoder::new(16, &code[offset..], DecoderOptions::NONE);
        decoder.set_ip(cpu.ip as u64);
        let mut instr = Instruction::default();
        
        if !decoder.can_decode() {
            break;
        }
        decoder.decode_out(&mut instr);

        // Advance IP (Fetch Step)
        // The CPU advances IP *before* executing. 
        // If the execution is a JUMP, it will overwrite this value.
        cpu.ip = cpu.ip.wrapping_add(instr.len() as u16);

        // Execute
        execute_instruction(cpu, &instr);
    }
}

#[test]
fn test_mov_and_registers() {
    let mut cpu = Cpu::new();
    
    // B8 34 12    MOV AX, 0x1234
    // 88 C4       MOV AH, AL (AH becomes 0x34)
    let code: [u8; 5] = [0xB8, 0x34, 0x12, 0x88, 0xC4];
    
    run_code(&mut cpu, &code);

    assert_eq!(cpu.ax, 0x3434); 
}

#[test]
fn test_stack_push_pop() {
    let mut cpu = Cpu::new();
    cpu.sp = 0xFFFE; // Initialize stack pointer

    // B8 55 AA    MOV AX, 0xAA55
    // 50          PUSH AX
    // B8 00 00    MOV AX, 0x0000
    // 5B          POP BX
    let code = [0xB8, 0x55, 0xAA, 0x50, 0xB8, 0x00, 0x00, 0x5B];

    run_code(&mut cpu, &code);

    assert_eq!(cpu.bx, 0xAA55);
    assert_eq!(cpu.sp, 0xFFFE); // SP should return to start
}

#[test]
fn test_inc_dec_flags() {
    let mut cpu = Cpu::new();
    
    // B0 FF       MOV AL, 0xFF
    // FE C0       INC AL  (Wraps to 0, ZF=1)
    // FE C8       DEC AL  (Wraps to FF, SF=1)
    let code = [0xB0, 0xFF, 0xFE, 0xC0, 0xFE, 0xC8];

    run_code(&mut cpu, &code);

    assert_eq!(cpu.get_al(), 0xFF);
    assert_eq!(cpu.get_cpu_flag(CpuFlags::ZF), false); 
    assert_eq!(cpu.get_cpu_flag(CpuFlags::SF), true); // 0xFF is -1 signed
}

#[test]
fn test_imul_16bit() {
    let mut cpu = Cpu::new();
    
    // B8 00 80    MOV AX, 0x8000 (-32768)
    // BB FF FF    MOV BX, 0xFFFF (-1)
    // F7 EB       IMUL BX  -> DX:AX = 32768 (0x00008000)
    let code = [0xB8, 0x00, 0x80, 0xBB, 0xFF, 0xFF, 0xF7, 0xEB];

    run_code(&mut cpu, &code);

    // Result should be positive 32768
    assert_eq!(cpu.dx, 0x0000);
    assert_eq!(cpu.ax, 0x8000);
    // Since 0x8000 requires 16 bits (unsigned representation), 
    // but as a signed 16-bit number it is negative, overflow flags checks 
    // depend on if the result fits in the lower half strictly.
    // 32768 does NOT fit in a signed i16 (max 32767). 
    assert_eq!(cpu.get_cpu_flag(CpuFlags::CF), true); 
}

#[test]
fn test_jumps_jz() {
    let mut cpu = Cpu::new();
    
    // B8 05 00    MOV AX, 5
    // 83 E8 05    SUB AX, 5  (ZF=1)
    // 74 02       JZ +2
    // B8 FF FF    MOV AX, 0xFFFF (Skipped)
    // 90          NOP
    let code = [0xB8, 0x05, 0x00, 0x83, 0xE8, 0x05, 0x74, 0x03, 0xB8, 0xFF, 0xFF, 0x90];

    run_code(&mut cpu, &code);

    assert_eq!(cpu.ax, 0); // Should remain 0, MOV AX, FFFF skipped
}

#[test]
fn test_string_rep_movsb() {
    let mut cpu = Cpu::new();
    cpu.si = 0x0000;
    cpu.di = 0x0010;
    cpu.cx = 0x0003; // Copy 3 bytes
    cpu.ds = 0x1000;
    cpu.es = 0x1000;
    cpu.set_dflag(false); // Increment

    // Manually populate source memory in RAM
    let src_phys = cpu.get_physical_addr(0x1000, 0x0000);
    cpu.bus.write_8(src_phys, 0xAA);
    cpu.bus.write_8(src_phys+1, 0xBB);
    cpu.bus.write_8(src_phys+2, 0xCC);

    // F3 A4       REP MOVSB
    let code = [0xF3, 0xA4];

    run_code(&mut cpu, &code);

    // Check Destination
    let dest_phys = cpu.get_physical_addr(0x1000, 0x0010);
    assert_eq!(cpu.bus.read_8(dest_phys), 0xAA);
    assert_eq!(cpu.bus.read_8(dest_phys+1), 0xBB);
    assert_eq!(cpu.bus.read_8(dest_phys+2), 0xCC);

    // Indices should update
    assert_eq!(cpu.si, 3);
    assert_eq!(cpu.di, 0x0013);
    assert_eq!(cpu.cx, 0);
}

#[test]
fn test_call_ret() {
    let mut cpu = Cpu::new();
    cpu.sp = 0xFFFE;

    // Layout:
    // 0x100: CALL +4     (E8 04 00) -> Jumps to 0x107 (Target)
    // 0x103: JMP +20     (EB 14)    -> Jumps to 0x119 (Exit test)
    // 0x105: NOP         (90)       -> Padding
    // 0x106: NOP         (90)       -> Padding
    // 0x107: RET         (C3)       -> Subroutine (Pops 0x103)

    let code = [
        0xE8, 0x04, 0x00, // 0x100: CALL +4 (Target 0x107). Push 0x103.
        0xEB, 0x14,       // 0x103: JMP +0x14 (Jump WAY out of bounds to stop runner)
        0x90, 0x90,       // 0x105: NOPs padding
        0xC3              // 0x107: RET
    ];

    run_code(&mut cpu, &code);

    // 1. CALL pushes 0x103, Jumps to 0x107.
    // 2. RET pops 0x103, Jumps to 0x103. Stack is now 0xFFFE.
    // 3. JMP jumps to 0x119. 
    // 4. run_code sees IP 0x119 > code.len(), and exits loop.
    
    assert_eq!(cpu.sp, 0xFFFE); // Stack should be balanced
}