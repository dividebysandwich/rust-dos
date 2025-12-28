use rust_dos::cpu::{Cpu, CpuFlags};

mod testrunners;

#[test]
fn test_logic_and_or_xor() {
    let mut cpu = Cpu::new();

    // AND: 0x0F0F & 0xFF00 = 0x0F00 (ZF=0, SF=0)
    cpu.set_reg16(iced_x86::Register::AX, 0x0F0F);
    // B9 00 FF -> MOV CX, 0xFF00
    // 21 C8    -> AND AX, CX
    testrunners::run_cpu_code(&mut cpu, &[0xB9, 0x00, 0xFF, 0x21, 0xC8]);
    assert_eq!(cpu.get_reg16(iced_x86::Register::AX), 0x0F00);
    assert!(!cpu.get_cpu_flag(CpuFlags::ZF));
    assert!(!cpu.get_cpu_flag(CpuFlags::SF));

    // OR: 0x55 AA | 0xAA 55 = 0xFFFF (SF=1)
    cpu.set_reg16(iced_x86::Register::BX, 0x55AA);
    // 81 CB 55 AA -> OR BX, 0xAA55
    testrunners::run_cpu_code(&mut cpu, &[0x81, 0xCB, 0x55, 0xAA]);
    assert_eq!(cpu.get_reg16(iced_x86::Register::BX), 0xFFFF);
    assert!(cpu.get_cpu_flag(CpuFlags::SF));

    // XOR: 0x1234 ^ 0x1234 = 0 (ZF=1)
    cpu.set_reg16(iced_x86::Register::DX, 0x1234);
    // 31 D2 -> XOR DX, DX
    testrunners::run_cpu_code(&mut cpu, &[0x31, 0xD2]);
    assert_eq!(cpu.get_reg16(iced_x86::Register::DX), 0);
    assert!(cpu.get_cpu_flag(CpuFlags::ZF));
}

#[test]
fn test_logic_not_test() {
    let mut cpu = Cpu::new();

    // NOT: ~0x00FF = 0xFF00 (Flags should not change)
    cpu.set_reg16(iced_x86::Register::AX, 0x00FF);
    cpu.set_cpu_flag(CpuFlags::ZF, true);
    // F7 D0 -> NOT AX
    testrunners::run_cpu_code(&mut cpu, &[0xF7, 0xD0]);
    assert_eq!(cpu.get_reg16(iced_x86::Register::AX), 0xFF00);
    assert!(cpu.get_cpu_flag(CpuFlags::ZF)); // Should remain true

    // TEST: 0x80 & 0x80 = 0x80 (Flags change, result discarded)
    cpu.set_reg8(iced_x86::Register::AL, 0x80);
    // A8 80 -> TEST AL, 0x80
    testrunners::run_cpu_code(&mut cpu, &[0xA8, 0x80]);
    assert_eq!(cpu.get_reg8(iced_x86::Register::AL), 0x80); // Still 0x80
    assert!(cpu.get_cpu_flag(CpuFlags::SF));
    assert!(!cpu.get_cpu_flag(CpuFlags::ZF));
}

#[test]
fn test_shifts_shl_shr_sar() {
    let mut cpu = Cpu::new();

    // SAR Test: 0xFE (-2) >> 1 should be 0xFF (-1)
    cpu.set_reg8(iced_x86::Register::BL, 0xFE);
    // D0 F8 is SAR BL, 1
    testrunners::run_cpu_code(&mut cpu, &[0xD0, 0xF8]);
    assert_eq!(cpu.get_reg8(iced_x86::Register::BL), 0xFF, "SAR 0xFE by 1 failed");
}

#[test]
fn test_rotates_rol_ror_rcl_rcr() {
    let mut cpu = Cpu::new();

    // ROL Test: 0x8001 rotated left 1 = 0x0003 (CF=1)
    cpu.set_reg16(iced_x86::Register::AX, 0x8001);
    testrunners::run_cpu_code(&mut cpu, &[0xD1, 0xC0]); 
    assert_eq!(cpu.get_reg16(iced_x86::Register::AX), 0x0003);

    // RCR Test: 0x01 rotated right through Carry (CF=1) = 0x80 (CF=1)
    cpu.set_reg8(iced_x86::Register::CL, 0x01);
    cpu.set_cpu_flag(CpuFlags::CF, true);
    // D0 D1 is RCR CL, 1
    testrunners::run_cpu_code(&mut cpu, &[0xD0, 0xD1]);
    assert_eq!(cpu.get_reg8(iced_x86::Register::CL), 0x80, "RCR 0x01 with CF=1 failed");
}

#[test]
fn test_logic_memory_operands() {
    let mut cpu = Cpu::new();
    let addr = 0x1000;
    cpu.bus.write_16(addr, 0xAAAA);
    
    // XOR [0x1000], 0xAAAA -> Sets memory to 0
    // 81 36 00 10 AA AA -> XOR word ptr [0x1000], 0xAAAA
    testrunners::run_cpu_code(&mut cpu, &[0x81, 0x36, 0x00, 0x10, 0xAA, 0xAA]);
    assert_eq!(cpu.bus.read_16(addr), 0x0000);
    assert!(cpu.get_cpu_flag(CpuFlags::ZF));
}

#[test]
fn test_rotates_rcr_only() {
    let mut cpu = Cpu::new();
    // Clear all registers
    cpu.ax = 0; cpu.bx = 0; cpu.cx = 0; cpu.dx = 0;
    
    cpu.set_reg8(iced_x86::Register::CL, 0x01);
    cpu.set_cpu_flag(CpuFlags::CF, true);
    
    testrunners::run_cpu_code(&mut cpu, &[0xD0, 0xD1]); // RCR CL, 1
    
    let cl_val = cpu.get_reg8(iced_x86::Register::CL);
    let al_val = cpu.get_reg8(iced_x86::Register::AL);
    
    println!("[DEBUG] AL: {}, CL: {}", al_val, cl_val);
    assert_eq!(cl_val, 0x80);
}