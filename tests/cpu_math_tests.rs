use rust_dos::cpu::{Cpu, CpuFlags};
use iced_x86::Register;
mod testrunners;
use testrunners::run_cpu_code;

#[test]
fn test_math_add_sub_adc_sbb() {
    let mut cpu = Cpu::new();

    // 1. ADD: 10 + 20 = 30
    cpu.set_reg16(Register::AX, 10);
    // 05 14 00 -> ADD AX, 20
    run_cpu_code(&mut cpu, &[0x05, 0x14, 0x00]);
    assert_eq!(cpu.ax, 30);
    assert!(!cpu.get_cpu_flag(CpuFlags::CF));

    // 2. ADC: 30 + 5 + CF(0) = 35
    // 83 D0 05 -> ADC AX, 5
    run_cpu_code(&mut cpu, &[0x83, 0xD0, 0x05]);
    assert_eq!(cpu.ax, 35);

    // 3. SUB: 35 - 40 = -5 (0xFFFB, CF=1)
    // 2D 28 00 -> SUB AX, 40
    run_cpu_code(&mut cpu, &[0x2D, 0x28, 0x00]);
    assert_eq!(cpu.ax, 0xFFFB);
    assert!(cpu.get_cpu_flag(CpuFlags::CF));

    // 4. SBB: 0xFFFB - 1 - CF(1) = 0xFFF9
    // 83 D8 01 -> SBB AX, 1
    run_cpu_code(&mut cpu, &[0x83, 0xD8, 0x01]);
    assert_eq!(cpu.ax, 0xFFF9);
}

#[test]
fn test_math_mul_div() {
    let mut cpu = Cpu::new();

    // MUL (Unsigned): 200 * 10 = 2000 (0x07D0)
    cpu.set_reg8(Register::AL, 200);
    cpu.set_reg8(Register::CL, 10);
    // F6 E1 -> MUL CL
    run_cpu_code(&mut cpu, &[0xF6, 0xE1]);
    assert_eq!(cpu.ax, 2000);
    assert!(cpu.get_cpu_flag(CpuFlags::CF), "Overflow should be set (res > 8bit)");

    // DIV (Unsigned): 2000 / 10 = 200 (AL=200, AH=0)
    cpu.ax = 2000;
    cpu.set_reg8(Register::BL, 10);
    // F6 F3 -> DIV BL
    run_cpu_code(&mut cpu, &[0xF6, 0xF3]);
    assert_eq!(cpu.get_reg8(Register::AL), 200);
    assert_eq!(cpu.get_reg8(Register::AH), 0);
}

#[test]
fn test_math_imul_idiv() {
    let mut cpu = Cpu::new();

    // IMUL (Signed): -5 * 10 = -50 (0xFFCE)
    cpu.set_reg8(Register::AL, 0xFB); // -5
    cpu.set_reg8(Register::DL, 10);
    // F6 EA -> IMUL DL
    run_cpu_code(&mut cpu, &[0xF6, 0xEA]);
    assert_eq!(cpu.ax, 0xFFCE);

    // IDIV (Signed): -50 / 10 = -5 (AL=0xFB, AH=0)
    cpu.ax = 0xFFCE;
    cpu.set_reg8(Register::BL, 10);
    // F6 FB -> IDIV BL
    run_cpu_code(&mut cpu, &[0xF6, 0xFB]);
    assert_eq!(cpu.get_reg8(Register::AL), 0xFB);
    assert_eq!(cpu.get_reg8(Register::AH), 0);
}

#[test]
fn test_math_inc_dec_neg_cmp() {
    let mut cpu = Cpu::new();

    // INC: 0xFFFF -> 0 (ZF=1, CF should NOT be affected)
    cpu.ax = 0xFFFF;
    cpu.set_cpu_flag(CpuFlags::CF, false);
    // 40 -> INC AX
    run_cpu_code(&mut cpu, &[0x40]);
    assert_eq!(cpu.ax, 0);
    assert!(cpu.get_cpu_flag(CpuFlags::ZF));
    assert!(!cpu.get_cpu_flag(CpuFlags::CF));

    // NEG: 5 -> -5 (0xFB)
    cpu.set_reg8(Register::AL, 5);
    // F6 D8 -> NEG AL
    run_cpu_code(&mut cpu, &[0xF6, 0xD8]);
    assert_eq!(cpu.get_reg8(Register::AL), 0xFB);
    assert!(cpu.get_cpu_flag(CpuFlags::CF), "NEG nonzero sets CF");

    // CMP: 10 - 10 = 0 (ZF=1)
    cpu.set_reg16(Register::BX, 10);
    // 81 FB 0A 00 -> CMP BX, 10
    run_cpu_code(&mut cpu, &[0x81, 0xFB, 0x0A, 0x00]);
    assert_eq!(cpu.bx, 10, "CMP must not modify destination");
    assert!(cpu.get_cpu_flag(CpuFlags::ZF));
}

#[test]
fn test_math_bcd_adjustments() {
    let mut cpu = Cpu::new();

    // DAA: AL=0x35, ADD AL, 0x39 (res=0x6E). DAA should adjust to 0x74.
    cpu.set_reg8(Register::AL, 0x35);
    // 04 39 -> ADD AL, 0x39
    // 27    -> DAA
    run_cpu_code(&mut cpu, &[0x04, 0x39, 0x27]);
    assert_eq!(cpu.get_reg8(Register::AL), 0x74);

    // DAS: AL=0x35, SUB AL, 0x39 (res=0xFC). DAS should adjust to 0x96 (CF=1)
    cpu.set_reg8(Register::AL, 0x35);
    cpu.set_cpu_flag(CpuFlags::AF, false);
    cpu.set_cpu_flag(CpuFlags::CF, false);
    // 2C 39 -> SUB AL, 0x39
    // 2F    -> DAS
    run_cpu_code(&mut cpu, &[0x2C, 0x39, 0x2F]);
    assert_eq!(cpu.get_reg8(Register::AL), 0x96);
    assert!(cpu.get_cpu_flag(CpuFlags::CF));

    // AAM: AL=0x1E (30). AAM 10 -> AH=3, AL=0
    cpu.set_reg8(Register::AL, 0x1E);
    // D4 0A -> AAM 10
    run_cpu_code(&mut cpu, &[0xD4, 0x0A]);
    assert_eq!(cpu.get_reg8(Register::AH), 3);
    assert_eq!(cpu.get_reg8(Register::AL), 0);
    assert!(cpu.get_cpu_flag(CpuFlags::ZF));

    // AAS: AL=0x33, SUB AL, 4 -> 0x2F. AAS -> 0x09 (with borrow logic)
    // Case: 0x05 - 0x06 = 0xFF. AAS: AL=0x09, AH decremented.
    cpu.set_reg8(Register::AL, 0x05);
    cpu.set_reg8(Register::AH, 0x02);
    // 2C 06 -> SUB AL, 6
    // 3F    -> AAS
    run_cpu_code(&mut cpu, &[0x2C, 0x06, 0x3F]);
    assert_eq!(cpu.get_reg8(Register::AL), 0x09);
    assert_eq!(cpu.get_reg8(Register::AH), 0x01);
}