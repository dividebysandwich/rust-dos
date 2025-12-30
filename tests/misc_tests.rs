use rust_dos::cpu::{Cpu, CpuFlags};
use iced_x86::Register;
mod testrunners;
use testrunners::run_cpu_code;

#[test]
fn test_cmp_r16_imm8_sign_extension() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));

    // SCENARIO: Compare AX (0) with -5 (0xFB).
    // Opcode: 83 F8 FB -> CMP AX, -5
    
    // Correct Behavior (Sign Extended):
    // 0 - (-5) = 5. 
    // Result is Positive. SF=0. ZF=0.
    // 0 > -5, so JG (Jump Greater) should be taken.
    
    cpu.set_reg16(Register::AX, 0);
    
    // 83 F8 FB -> CMP AX, 0xFB
    run_cpu_code(&mut cpu, &[0x83, 0xF8, 0xFB]);

    // Check Flags
    assert!(!cpu.get_cpu_flag(CpuFlags::SF), "CMP AX, -5 failed sign extension! (Treated -5 as 251, resulted in negative SF)");
    assert!(cpu.get_cpu_flag(CpuFlags::CF), "CMP AX, -5 did not cause borrow! (0 < 251)");
}

#[test]
fn test_xchg_functionality() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));

    // SCENARIO: Swap AX and BX.
    // Graphics routines use this to sort coordinates.
    // If XCHG fails, lines don't draw or draw backwards.

    cpu.set_reg16(Register::AX, 0x1111);
    cpu.set_reg16(Register::BX, 0x2222);

    // 93 -> XCHG AX, BX (or XCHG BX, AX - order doesn't matter)
    // Note: 90+reg opcodes exchange with AX.
    run_cpu_code(&mut cpu, &[0x93]);

    assert_eq!(cpu.get_reg16(Register::AX), 0x2222, "XCHG failed to update AX");
    assert_eq!(cpu.get_reg16(Register::BX), 0x1111, "XCHG failed to update BX");
}

#[test]
fn test_xchg_memory() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    let addr = 0x1000;

    // SCENARIO: Swap Register and Memory.
    // 87 1E 00 10 -> XCHG BX, [0x1000]
    
    cpu.set_reg16(Register::BX, 0x5555);
    cpu.bus.write_16(addr, 0xAAAA);

    run_cpu_code(&mut cpu, &[0x87, 0x1E, 0x00, 0x10]);

    assert_eq!(cpu.get_reg16(Register::BX), 0xAAAA, "XCHG Reg-Mem failed to update Register");
    assert_eq!(cpu.bus.read_16(addr), 0x5555, "XCHG Reg-Mem failed to update Memory");
}

#[test]
fn test_bp_access_uses_ss_default() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));

    // Setup Segments pointing to different memory areas
    cpu.set_reg16(Register::DS, 0x1000); // Data Segment base: 0x10000
    cpu.set_reg16(Register::SS, 0x2000); // Stack Segment base: 0x20000
    cpu.set_reg16(Register::BP, 0x0010); // Offset 0x0010

    // Write distinct values to physical memory
    // DS:[BP] -> 0x10000 + 0x0010 = 0x10010
    // We write 0xDA7A ("Data") here
    cpu.bus.write_16(0x10010, 0xDA7A); 
    
    // SS:[BP] -> 0x20000 + 0x0010 = 0x20010
    // We write 0x5555 ("Stack") here
    cpu.bus.write_16(0x20010, 0x5555); 

    // Execute: MOV AX, [BP]
    // Opcode: 8B 46 00 (ModRM implies [BP+0])
    // CORRECT Behavior: Accesses SS:[BP] -> Reads 0x5555
    // BUGGY Behavior: Accesses DS:[BP] -> Reads 0xDA7A
    run_cpu_code(&mut cpu, &[0x8B, 0x46, 0x00]);

    assert_eq!(cpu.get_reg16(Register::AX), 0x5555, 
        "Memory access with BP base did NOT default to Stack Segment (SS)!");
}

#[test]
fn test_lea_loads_offset_only() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    
    cpu.set_reg16(Register::DS, 0x5000);
    cpu.set_reg16(Register::BX, 0x1000);
    cpu.set_reg16(Register::SI, 0x0005);
    
    // LEA AX, [BX+SI+5]
    // 8D 40 05 -> LEA AX, [BX+SI+0x05]
    // Expected: 0x1000 + 0x0005 + 0x05 = 0x100A
    // Buggy: 0x50000 + ... (Includes segment)
    run_cpu_code(&mut cpu, &[0x8D, 0x40, 0x05]);

    assert_eq!(cpu.get_reg16(Register::AX), 0x100A, "LEA instruction should not include segment base!");
}

#[test]
fn test_pusha_popa_order() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));

    // 1. Setup specific values in ALL general registers
    cpu.set_reg16(Register::AX, 0xAAAA);
    cpu.set_reg16(Register::CX, 0xCCCC);
    cpu.set_reg16(Register::DX, 0xDDDD);
    cpu.set_reg16(Register::BX, 0xBBBB);
    cpu.set_reg16(Register::SP, 0x1000); // Stack Pointer
    cpu.set_reg16(Register::BP, 0x5555);
    cpu.set_reg16(Register::SI, 0x1111);
    cpu.set_reg16(Register::DI, 0x2222);

    // 2. PUSHA (60)
    // Pushes in order: AX, CX, DX, BX, SP(original), BP, SI, DI
    run_cpu_code(&mut cpu, &[0x60]);

    // 3. Corrupt registers to ensure POPA actually restores them
    cpu.set_reg16(Register::AX, 0);
    cpu.set_reg16(Register::DI, 0);

    // 4. POPA (61)
    // Pops in reverse order: DI, SI, BP, SP(skip), BX, DX, CX, AX
    run_cpu_code(&mut cpu, &[0x61]);

    // 5. Verify ALL restored correctly
    assert_eq!(cpu.get_reg16(Register::AX), 0xAAAA, "POPA failed to restore AX");
    assert_eq!(cpu.get_reg16(Register::CX), 0xCCCC, "POPA failed to restore CX");
    assert_eq!(cpu.get_reg16(Register::DX), 0xDDDD, "POPA failed to restore DX");
    assert_eq!(cpu.get_reg16(Register::BX), 0xBBBB, "POPA failed to restore BX");
    assert_eq!(cpu.get_reg16(Register::BP), 0x5555, "POPA failed to restore BP");
    assert_eq!(cpu.get_reg16(Register::SI), 0x1111, "POPA failed to restore SI");
    assert_eq!(cpu.get_reg16(Register::DI), 0x2222, "POPA failed to restore DI");
    
    // SP should be back to 0x1000 (POPA does not pop SP value, but increments SP)
    assert_eq!(cpu.get_reg16(Register::SP), 0x1000, "POPA failed to restore SP");
}

#[test]
fn test_ret_imm16_cleans_stack() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));

    // SCENARIO: Pascal calling convention.
    // 1. Caller pushes args.
    // 2. Caller "CALLs" (pushes Return Address).
    // 3. Callee executes RET N to pop IP and clean args.

    cpu.set_reg16(Register::SP, 0x100);
    
    // 1. Simulate Args: Push 3 args (6 bytes)
    // SP starts at 0x100.
    cpu.push(0x0001); // SP -> 0xFE
    cpu.push(0x0002); // SP -> 0xFC
    cpu.push(0x0003); // SP -> 0xFA

    // 2. Simulate a CALL: Push Return Address (0x1234)
    cpu.push(0x1234); // SP -> 0xF8 (Top of Stack is now IP)
    
    // Execute RET 6 (C2 06 00)
    // Should: 
    //   a. Pop IP (0x1234) from 0xF8. SP becomes 0xFA.
    //   b. Add 6 to SP. SP becomes 0x100.
    run_cpu_code(&mut cpu, &[0xC2, 0x06, 0x00]);

    assert_eq!(cpu.ip, 0x1234, "RET failed to pop correct Return Address");
    assert_eq!(cpu.get_reg16(Register::SP), 0x100, "RET N failed to clean up stack arguments (SP incorrect)");
}

#[test]
fn test_xlat_translation() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));

    // SCENARIO: Text mode often uses lookup tables to map characters.
    // XLAT: AL = [DS:BX + AL]
    
    let table_addr = 0x2000;
    cpu.set_reg16(Register::BX, table_addr);
    
    // AL = 2 (Index)
    cpu.set_reg8(Register::AL, 0x02);
    
    // Write table at 0x2000: [00, 10, 20, 30...]
    cpu.bus.write_8(table_addr as usize + 2, 0x99); // The expected value

    // D7 -> XLAT
    run_cpu_code(&mut cpu, &[0xD7]);

    assert_eq!(cpu.get_reg8(Register::AL), 0x99, "XLAT failed to lookup byte");
}

#[test]
fn test_stos_uses_es_segment() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));

    // SCENARIO: STOSW writes AX to [ES:DI].
    // If it incorrectly uses DS, graphics will fail.
    
    cpu.set_reg16(Register::AX, 0xCAFE);
    cpu.set_reg16(Register::DI, 0x0010);
    
    // Setup Segments
    cpu.set_reg16(Register::DS, 0x1000); // Data Segment
    cpu.set_reg16(Register::ES, 0x2000); // Extra Segment (Video Memory usually)
    
    // Write "Canary" values to memory to verify where the write happens
    // DS:DI -> 0x10010
    cpu.bus.write_16(0x10010, 0xDEAD);
    // ES:DI -> 0x20010
    cpu.bus.write_16(0x20010, 0x0000);

    // AB -> STOSW
    run_cpu_code(&mut cpu, &[0xAB]);

    assert_eq!(cpu.bus.read_16(0x20010), 0xCAFE, "STOSW failed to write to ES segment!");
    assert_eq!(cpu.bus.read_16(0x10010), 0xDEAD, "STOSW incorrectly wrote to DS segment!");
}

