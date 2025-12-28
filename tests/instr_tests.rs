use rust_dos::cpu::{Cpu, CpuFlags};
use rust_dos::f80::F80;
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
        cpu.ip = instr.next_ip() as u16;

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

#[test]
fn test_repe_scasb_backwards_mismatch() {
    let mut cpu = Cpu::new();
    cpu.es = 0x1000;
    cpu.di = 0x0004; // Point to the end of a buffer
    cpu.cx = 0x0005; 
    cpu.set_reg8(iced_x86::Register::AL, 0x30); // Scanning for '0'
    cpu.set_dflag(true); // Backwards!

    // Buffer at 1000:0000 -> [ '8', '0', '0', '0', '0' ]
    let phys = cpu.get_physical_addr(0x1000, 0x0000);
    cpu.bus.write_8(phys, b'8'); 
    cpu.bus.write_8(phys + 1, b'0');
    cpu.bus.write_8(phys + 2, b'0');
    cpu.bus.write_8(phys + 3, b'0');
    cpu.bus.write_8(phys + 4, b'0');

    // F3 AE : REPE SCASB
    // Should skip the four '0's and stop at '8'
    let code = [0xF3, 0xAE];
    run_code(&mut cpu, &code);

    // On hardware, after mismatch:
    // 1. DI points to one byte BEFORE the '8' (because it decrements after the match)
    // 2. ZF is cleared (0) because '8' != '0'
    // 3. CX should be 0 because it processed all bytes or stopped at the first non-zero
    assert_eq!(cpu.get_cpu_flag(CpuFlags::ZF), false);
    let comparison: u16 = 0x0000;
    assert_eq!(cpu.di, comparison.wrapping_sub(1)); // Stopped at index 0, then decremented
}

#[test]
fn test_qb_trim_logic() {
    let mut cpu = Cpu::new();
    // 1. Set AL to '0' (0x30), DI points to '8' (0x38)
    // 2. SCASB (AL vs [DI]) -> Should set ZF=0
    // 3. JZ ... (Should NOT jump)
    
    cpu.es = 0x1000;
    cpu.di = 0x0000;
    cpu.set_reg8(iced_x86::Register::AL, 0x30);
    cpu.bus.write_8(cpu.get_physical_addr(0x1000, 0), 0x38); // The '8'

    // AE (SCASB), 74 02 (JZ +2), B0 FF (MOV AL, FF)
    let code = [0xAE, 0x74, 0x02, 0xB0, 0xFF];
    run_code(&mut cpu, &code);

    // If ZF was correctly 0, the jump was not taken. AL should be 0xFF.
    assert_eq!(cpu.get_al(), 0xFF, "The trim logic took a jump it shouldn't have!");
}

#[test]
fn test_sbb_immediate_check() {
    let mut cpu = Cpu::new();
    // 1C 05  -> SBB AL, 5
    // 90     -> NOP
    let code = [0x1C, 0x05, 0x90];
    cpu.set_cpu_flag(CpuFlags::CF, false);
    cpu.set_reg8(iced_x86::Register::AL, 10);
    
    run_code(&mut cpu, &code);

    // If IP logic is right, AL = 5.
    // If IP logic is wrong and it read the NOP (0x90), AL = 10 - 0x90 = 0x80.
    assert_eq!(cpu.get_al(), 5, "SBB AL, imm8 read the wrong immediate value!");
}

#[test]
fn test_alu_sbb_comprehensive() {
    let mut cpu = Cpu::new();

    // --- 1. Basic Borrow Ripple (The 0x0100 - 1 Case) ---
    // Low Byte: 0x00 - 0x01 = 0xFF (CF=1)
    let res_low = cpu.alu_sub_8(0x00, 0x01);
    assert_eq!(res_low, 0xFF);
    assert_eq!(cpu.get_cpu_flag(CpuFlags::CF), true);

    // High Byte: 0x01 - 0x00 - (CF=1) = 0x00 (CF=0, ZF=1)
    let res_high = cpu.alu_sbb_8(0x01, 0x00);
    assert_eq!(res_high, 0x00, "High byte of 0x0100 - 1 should be 0");
    assert_eq!(cpu.get_cpu_flag(CpuFlags::ZF), true, "ZF should be set for high byte");
    assert_eq!(cpu.get_cpu_flag(CpuFlags::CF), false, "Borrow should be consumed");

    // --- 2. The "Max Borrow" Case (0x0000 - 1) ---
    // 0x0000 - 0x0001 = 0xFFFF
    cpu.set_cpu_flag(CpuFlags::CF, false);
    let res16 = cpu.alu_sbb_16(0x0000, 0x0001);
    assert_eq!(res16, 0xFFFF);
    assert_eq!(cpu.get_cpu_flag(CpuFlags::CF), true);
    assert_eq!(cpu.get_cpu_flag(CpuFlags::AF), true, "Borrow from bit 3 to 4 should set AF");

    // --- 3. Zero Flag Stability ---
    // 0x80 - 0x7F - (CF=1) = 0
    cpu.set_cpu_flag(CpuFlags::CF, true);
    let res8 = cpu.alu_sbb_8(0x80, 0x7F);
    assert_eq!(res8, 0);
    assert_eq!(cpu.get_cpu_flag(CpuFlags::ZF), true);
    assert_eq!(cpu.get_cpu_flag(CpuFlags::CF), false);

    // --- 4. Subbing with Carry-In causing a wrap ---
    // 0x00 - 0x00 - (CF=1) = 0xFF (CF=1)
    cpu.set_cpu_flag(CpuFlags::CF, true);
    let res8_wrap = cpu.alu_sbb_8(0x00, 0x00);
    assert_eq!(res8_wrap, 0xFF);
    assert_eq!(cpu.get_cpu_flag(CpuFlags::CF), true);
}

#[test]
fn test_rcl_preserves_zf() {
    let mut cpu = Cpu::new();
    
    // 1. CMP AL, AL (Sets ZF=1)
    // 2. MOV AL, 0xFE
    // 3. RCL AL, 1  (Result is non-zero, but ZF must remain 1)
    let code = [
        0x3C, 0x00,       // CMP AL, 0 (Sets ZF=1 if AL was 0)
        0xB0, 0xFE,       // MOV AL, 0xFE
        0xD0, 0xD0        // RCL AL, 1
    ];
    
    cpu.set_reg8(iced_x86::Register::AL, 0);
    run_code(&mut cpu, &code);

    assert_eq!(cpu.get_cpu_flag(CpuFlags::ZF), true, "RCL destroyed the Zero Flag!");
}

#[test]
fn test_adc_af_flag() {
    let mut cpu = Cpu::new();
    cpu.set_cpu_flag(CpuFlags::CF, true);

    // 0x07 + 0x08 + CF(1) = 0x10
    // Binary: 0111 + 1000 + 1 = 10000
    // There is a carry out of bit 3 into bit 4. AF must be 1.
    let code = [
        0xB0, 0x07,       // MOV AL, 7
        0x14, 0x08        // ADC AL, 8
    ];

    run_code(&mut cpu, &code);

    assert_eq!(cpu.get_al(), 0x10);
    assert_eq!(cpu.get_cpu_flag(CpuFlags::AF), true);
}

#[test]
fn test_das_instruction() {
    let mut cpu = Cpu::new();

    // Case 1: 0x9A -> 0x94 (Standard adjustment)
    // AL = 0x9A, DAS -> AL = 0x94
    cpu.set_reg8(iced_x86::Register::AL, 0x9A);
    cpu.set_cpu_flag(CpuFlags::AF, false);
    cpu.set_cpu_flag(CpuFlags::CF, false);
    
    let code = [0x2F]; // DAS
    run_code(&mut cpu, &code);
    assert_eq!(cpu.get_al(), 0x94, "DAS failed to adjust 0x9A to 0x94");

    // Case 2: Multi-digit borrow (Crucial for the '08' bug)
    // AL = 0x05, SUB AL, 0x06 (Result 0xFF, CF=1, AF=1)
    // DAS should turn 0xFF into 0x99 (representing -1 in BCD)
    cpu.set_reg8(iced_x86::Register::AL, 0x05);
    let code_sub_das = [0x2C, 0x06, 0x2F]; // SUB AL, 6; DAS
    run_code(&mut cpu, &code_sub_das);
    
    assert_eq!(cpu.get_al(), 0x99, "DAS failed to adjust subtraction result to BCD 99");
    assert_eq!(cpu.get_cpu_flag(CpuFlags::CF), true, "DAS should maintain/set CF for borrows");
}

#[test]
fn test_aas_instruction() {
    let mut cpu = Cpu::new();

    // 0x08 - 0x09 = 0xFF. AAS should adjust this.
    // AL = 0xFF -> AL = 0x09, AH = AH - 1, CF=1, AF=1
    cpu.ax = 0x0108; // AH=1, AL=8
    let code = [0x2C, 0x09, 0x3F]; // SUB AL, 9; AAS
    run_code(&mut cpu, &code);

    assert_eq!(cpu.get_al(), 0x09);
    assert_eq!(cpu.ax >> 8, 0x00, "AAS failed to decrement AH on borrow");
    assert_eq!(cpu.get_cpu_flag(CpuFlags::CF), true);
}

#[test]
fn test_fpu_comparison_flags() {
    let mut cpu = Cpu::new();
    
    // Compare 8.0 (ST0) with 10.0 (Mem)
    // Should result in ST0 < Source: C3=0, C2=0, C0=1
    // SAHF should then set: ZF=0, PF=0, CF=1
    
    let mut f8 = F80::new(); f8.set_f64(8.0);
    cpu.fpu_push(f8);
    
    let mut f10 = F80::new(); f10.set_f64(10.0);
    let addr = 0x2000;
    let bytes = f10.get_bytes(); // Assuming TBYTE
    for i in 0..10 { cpu.bus.write_8(addr + i, bytes[i]); }

    // D8 1E 00 20: FCOMP TBYTE PTR [2000]
    // 9E: SAHF
    let code = [0xD8, 0x1E, 0x00, 0x20, 0x9E];
    run_code(&mut cpu, &code);

    assert_eq!(cpu.get_cpu_flag(CpuFlags::CF), true, "Carry should be set (8 < 10)");
    assert_eq!(cpu.get_cpu_flag(CpuFlags::ZF), false, "Zero should be clear (8 != 10)");
    assert_eq!(cpu.get_cpu_flag(CpuFlags::PF), false, "Parity should be clear (Not Unordered)");
}

