use rust_dos::cpu::{Cpu, CpuFlags};
use iced_x86::Register;
mod testrunners;
use testrunners::run_cpu_code;

#[test]
fn test_rep_movsb_forward() {
    let mut cpu = Cpu::new();
    
    // Setup Pointers
    cpu.ds = 0x1000;
    cpu.si = 0x0000; // Source
    cpu.es = 0x2000;
    cpu.di = 0x0010; // Dest
    cpu.cx = 5;      // Count
    cpu.set_dflag(false); // Increment mode

    // Write source data: 1, 2, 3, 4, 5
    let src_phys = cpu.get_physical_addr(0x1000, 0x0000);
    for i in 0..5 {
        cpu.bus.write_8(src_phys + i, (i + 1) as u8);
    }

    // F3 A4: REP MOVSB
    let code = [0xF3, 0xA4];
    run_cpu_code(&mut cpu, &code);

    // Verify Destination
    let dst_phys = cpu.get_physical_addr(0x2000, 0x0010);
    for i in 0..5 {
        assert_eq!(cpu.bus.read_8(dst_phys + i), (i + 1) as u8, "Byte {} mismatch", i);
    }

    // Verify Register Updates
    assert_eq!(cpu.cx, 0, "CX should be 0");
    assert_eq!(cpu.si, 5, "SI incremented by 5");
    assert_eq!(cpu.di, 0x0015, "DI incremented by 5");
}

#[test]
fn test_rep_stosw_backward() {
    let mut cpu = Cpu::new();
    
    cpu.es = 0x1000;
    cpu.di = 0x0008; // Start at offset 8
    cpu.cx = 3;      // Write 3 Words
    cpu.ax = 0xABCD; // Pattern
    cpu.set_dflag(true); // Decrement mode

    // FD: STD (Set Direction Flag)
    // F3 AB: REP STOSW
    let code = [0xFD, 0xF3, 0xAB];
    run_cpu_code(&mut cpu, &code);

    assert!(cpu.get_cpu_flag(CpuFlags::DF));
    assert_eq!(cpu.cx, 0);
    
    // Check pointers: Started at 8. 3 words = 6 bytes. 
    // Decrement: 8 -> 6 -> 4 -> 2. Final DI should be 2.
    assert_eq!(cpu.di, 2);

    // Check Memory (Backwards from 8)
    // Word 1 at [7,8] ?? No, x86 stores at [DI] then decrements.
    // So 1st Word at 8, 2nd at 6, 3rd at 4.
    // wait, little endian. Low byte at addr, High at addr+1.
    // Actually, std behavior: Store at ES:DI, then sub 2.
    let base = cpu.get_physical_addr(0x1000, 0);
    assert_eq!(cpu.bus.read_16(base + 8), 0xABCD);
    assert_eq!(cpu.bus.read_16(base + 6), 0xABCD);
    assert_eq!(cpu.bus.read_16(base + 4), 0xABCD);
}

#[test]
fn test_lodsb_no_rep() {
    let mut cpu = Cpu::new();
    
    cpu.ds = 0x1000;
    cpu.si = 0x0005;
    cpu.set_dflag(false);

    // Write 'X' (0x58) to source
    let addr = cpu.get_physical_addr(0x1000, 0x0005);
    cpu.bus.write_8(addr, 0x58);

    // AC: LODSB
    run_cpu_code(&mut cpu, &[0xAC]);

    assert_eq!(cpu.get_al(), 0x58);
    assert_eq!(cpu.si, 0x0006);
}

#[test]
fn test_repne_scasb_match_found() {
    let mut cpu = Cpu::new();
    
    cpu.es = 0x1000;
    cpu.di = 0x0000;
    cpu.cx = 10;
    cpu.set_reg8(Register::AL, 0x42); // Search for 0x42
    cpu.set_dflag(false);

    // Memory: [00, 00, 00, 42, 00 ...]
    let base = cpu.get_physical_addr(0x1000, 0);
    cpu.bus.write_8(base + 0, 0x00);
    cpu.bus.write_8(base + 1, 0x00);
    cpu.bus.write_8(base + 2, 0x00);
    cpu.bus.write_8(base + 3, 0x42); // Match here (4th byte)
    cpu.bus.write_8(base + 4, 0x00);

    // F2 AE: REPNE SCASB
    run_cpu_code(&mut cpu, &[0xF2, 0xAE]);

    // 1. Loop 1 (Idx 0): 0x42 - 0x00 != 0. ZF=0. Continue.
    // 2. Loop 2 (Idx 1): ... ZF=0.
    // 3. Loop 3 (Idx 2): ... ZF=0.
    // 4. Loop 4 (Idx 3): 0x42 - 0x42 == 0. ZF=1. STOP.
    
    // CX started at 10. Decremented 4 times. Remaining: 6.
    assert_eq!(cpu.cx, 6);
    
    // DI incremented 4 times. Current: 4.
    assert_eq!(cpu.di, 4);

    // Flag should be Equal (ZF=1) indicating match found
    assert!(cpu.get_cpu_flag(CpuFlags::ZF));
}

#[test]
fn test_repne_scasb_no_match() {
    let mut cpu = Cpu::new();
    cpu.es = 0x1000;
    cpu.di = 0x0000;
    cpu.cx = 5;
    cpu.set_reg8(Register::AL, 0xFF);
    
    // F2 AE: REPNE SCASB
    run_cpu_code(&mut cpu, &[0xF2, 0xAE]);

    // Should run until CX=0 because 0xFF is not in empty memory (0x00)
    assert_eq!(cpu.cx, 0);
    assert_eq!(cpu.di, 5);
    assert!(!cpu.get_cpu_flag(CpuFlags::ZF)); // ZF=0 (Not Found)
}

#[test]
fn test_repe_cmpsb_mismatch() {
    let mut cpu = Cpu::new();
    
    cpu.ds = 0x1000; cpu.si = 0;
    cpu.es = 0x1000; cpu.di = 10;
    cpu.cx = 5;
    cpu.set_dflag(false);

    // Source: "HELLO"
    // Dest:   "HELXO"
    let src = cpu.get_physical_addr(0x1000, 0);
    let dst = cpu.get_physical_addr(0x1000, 10);
    
    let s_bytes = b"HELLO";
    let d_bytes = b"HELXO";

    for i in 0..5 {
        cpu.bus.write_8(src + i, s_bytes[i]);
        cpu.bus.write_8(dst + i, d_bytes[i]);
    }

    // F3 A6: REPE CMPSB
    run_cpu_code(&mut cpu, &[0xF3, 0xA6]);

    // Should stop at index 3 ('L' vs 'X')
    // CX decrements: 5->4 (H), 4->3 (E), 3->2 (L), 2->1 (Mismatch L!=X)
    // Wait, let's trace:
    // 1. 'H'=='H', ZF=1, CX=4, SI=1, DI=11
    // 2. 'E'=='E', ZF=1, CX=3, SI=2, DI=12
    // 3. 'L'=='L', ZF=1, CX=2, SI=3, DI=13
    // 4. 'L'!='X', ZF=0, CX=1, SI=4, DI=14 -> STOP

    assert_eq!(cpu.cx, 1);
    assert_eq!(cpu.si, 4);
    assert_eq!(cpu.di, 14);
    assert!(!cpu.get_cpu_flag(CpuFlags::ZF)); // Mismatch
}


#[test]
fn test_string_std_stosb_decrement() {
    let mut cpu = Cpu::new();

    // Scenario 2: STOSB with Direction Flag SET (Decrement)
    // Writing backwards from 0x2000.
    
    cpu.set_cpu_flag(CpuFlags::DF, true); // Decrement
    cpu.set_reg8(iced_x86::Register::AL, 0xFF);
    cpu.set_reg16(iced_x86::Register::DI, 0x2000);
    
    // AA -> STOSB (Once)
    testrunners::run_cpu_code(&mut cpu, &[0xAA]);
    
    assert_eq!(cpu.bus.read_8(0x2000), 0xFF);
    
    // 0x2000 - 1 = 0x1FFF
    assert_eq!(cpu.get_reg16(iced_x86::Register::DI), 0x1FFF, "DI should decrement by 1");
}

#[test]
fn test_string_rep_stosw_direction() {
    let mut cpu = Cpu::new();

    // Scenario 1: REP STOSW with Direction Flag CLEAR (Increment)
    // We want to write 0xABCD to memory locations 0x1000, 0x1002, 0x1004.
    
    cpu.set_cpu_flag(CpuFlags::DF, false); // Increment
    cpu.set_reg16(iced_x86::Register::CX, 3);    // Count = 3
    cpu.set_reg16(iced_x86::Register::AX, 0xABCD);
    cpu.set_reg16(iced_x86::Register::DI, 0x1000); // Dest Index
    
    // F3 AB -> REP STOSW
    testrunners::run_cpu_code(&mut cpu, &[0xF3, 0xAB]);

    // Check memory
    assert_eq!(cpu.bus.read_16(0x1000), 0xABCD);
    assert_eq!(cpu.bus.read_16(0x1002), 0xABCD);
    assert_eq!(cpu.bus.read_16(0x1004), 0xABCD);
    
    // Check Registers
    assert_eq!(cpu.get_reg16(iced_x86::Register::CX), 0, "CX should be 0 after REP");
    assert_eq!(cpu.get_reg16(iced_x86::Register::DI), 0x1006, "DI should increment by 2 * 3 = 6");
}

#[test]
fn test_lods_segment_override() {
    let mut cpu = Cpu::new();

    // SCENARIO: Load String Byte (LODSB) with Segment Override.
    // Instruction: 2E AC -> LODSB CS:[SI]
    // 
    // Default: DS:[SI] (0x1000:0x0010) -> Contains 0xDD (Data)
    // Override: CS:[SI] (0x2000:0x0010) -> Contains 0xCC (Code)

    // Setup Segments
    cpu.set_reg16(iced_x86::Register::DS, 0x1000);
    cpu.set_reg16(iced_x86::Register::CS, 0x2000);
    cpu.set_reg16(iced_x86::Register::SI, 0x0010);

    // Setup Memory
    cpu.bus.write_8(0x10010, 0xDD); // Data Segment Value
    cpu.bus.write_8(0x20010, 0xCC); // Code Segment Value

    // Execute: CS: LODSB
    // Opcode: 2E (CS Prefix), AC (LODSB)
    testrunners::run_cpu_code(&mut cpu, &[0x2E, 0xAC]);

    assert_eq!(cpu.get_reg8(iced_x86::Register::AL), 0xCC, 
        "LODSB ignored the CS: Segment Override! Read from DS (0xDD) instead of CS (0xCC).");
}

#[test]
fn test_loop_zf_interaction() {
    let mut cpu = Cpu::new();

    // SCENARIO: LOOPE (Loop while Equal).
    // Should loop if CX != 0 AND ZF == 1.
    // If ZF becomes 0, it should fall through immediately.
    
    cpu.set_reg16(iced_x86::Register::CX, 5);
    cpu.set_cpu_flag(CpuFlags::ZF, false); // ZF=0 (Not Equal)

    // E1 FE -> LOOPE -2 (Jump back to self)
    // Should NOT jump because ZF is 0.
    
    cpu.ip = 0x100;
    testrunners::run_cpu_code(&mut cpu, &[0xE1, 0xFE]);

    // Should have executed ONCE, seen ZF=0, and continued.
    // CX should decrement once (standard behavior for LOOPx instructions: dec then check).
    assert_eq!(cpu.get_reg16(iced_x86::Register::CX), 4, "LOOPE should decrement CX once");
    assert_eq!(cpu.ip, 0x102, "LOOPE should NOT take branch if ZF=0");
}