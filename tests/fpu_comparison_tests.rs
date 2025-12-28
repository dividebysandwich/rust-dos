use rust_dos::cpu::{Cpu, FpuFlags, CpuFlags};
use rust_dos::f80::F80;
use rust_dos::instructions::fpu::arithmetic::*;
use rust_dos::instructions::fpu::comparison::*;
use iced_x86::{Decoder, DecoderOptions, Instruction, Mnemonic};

// Helper to decode and run a single instruction
fn run_fpu_instr(cpu: &mut Cpu, code: &[u8]) {
    let mut decoder = Decoder::new(16, code, DecoderOptions::NONE);
    let instr = decoder.decode();
    
    cpu.ip = 0x100;
    
    match instr.mnemonic() {
        // Comparisons
        Mnemonic::Fcom | Mnemonic::Fcomp | Mnemonic::Fcompp => 
            rust_dos::instructions::fpu::comparison::fcom_variants(cpu, &instr),
            
        // Control Instructions (NEEDED for the Bridge Test)
        Mnemonic::Fnstsw | Mnemonic::Fstsw => 
            rust_dos::instructions::fpu::control::fnstsw(cpu, &instr),
            
        // SAHF (Store AH into Flags)
        Mnemonic::Sahf => {
            // Simple SAHF emulation for the test
            let ah = (cpu.ax >> 8) as u8;
            // SF: Bit 7, ZF: Bit 6, AF: Bit 4, PF: Bit 2, CF: Bit 0
            cpu.set_cpu_flag(rust_dos::cpu::CpuFlags::SF, (ah & 0x80) != 0);
            cpu.set_cpu_flag(rust_dos::cpu::CpuFlags::ZF, (ah & 0x40) != 0);
            cpu.set_cpu_flag(rust_dos::cpu::CpuFlags::AF, (ah & 0x10) != 0);
            cpu.set_cpu_flag(rust_dos::cpu::CpuFlags::PF, (ah & 0x04) != 0);
            cpu.set_cpu_flag(rust_dos::cpu::CpuFlags::CF, (ah & 0x01) != 0);
        }

        _ => panic!("Test runner missing handler for {:?}", instr.mnemonic()),
    }
}

#[test]
fn test_fcompp_stack_cleanup() {
    let mut cpu = Cpu::new();
    let initial_top = cpu.fpu_top;
    
    cpu.fpu_push(F80::new());
    cpu.fpu_push(F80::new());
    
    // DE D9: FCOMPP (Compare ST(0) with ST(1) and pop twice)
    run_fpu_instr(&mut cpu, &[0xDE, 0xD9]);

    assert_eq!(cpu.fpu_top, initial_top, "FCOMPP failed to restore FPU TOP");
    assert_eq!(cpu.fpu_tags[initial_top], rust_dos::cpu::FPU_TAG_EMPTY);
}

#[test]
fn test_fpu_to_cpu_flags_bridge() {
    let mut cpu = Cpu::new();
    
    // Compare 10.0 and 10.0 (Should set ZF)
    let mut f10 = F80::new(); f10.set_f64(10.0);
    cpu.fpu_push(f10);
    cpu.fpu_push(f10);
    
    // 1. FCOM ST(1) -> D8 D1
    run_fpu_instr(&mut cpu, &[0xD8, 0xD1]);
    assert!(cpu.get_fpu_flag(FpuFlags::C3), "FPU C3 should be set for equality");

    // 2. FSTSW AX -> DF E0
    run_fpu_instr(&mut cpu, &[0xDF, 0xE0]);
    let ah = (cpu.ax >> 8) as u8;
    assert!((ah & 0x40) != 0, "AH bit 6 (from C3) should be set. AH was {:02X}", ah);

    // 3. SAHF -> 9E
    run_fpu_instr(&mut cpu, &[0x9E]);
    assert!(cpu.get_cpu_flag(CpuFlags::ZF), "CPU ZF should be set after SAHF");
}

#[test]
fn test_fcom_reverse_opcode_dc() {
    let mut cpu = Cpu::new();
    
    // Scenario: ST(0)=8.0, ST(1)=10.0
    // We want to check if 10.0 < 8.0 (Which is FALSE)
    let mut f10 = F80::new(); f10.set_f64(10.0);
    let mut f8 = F80::new(); f8.set_f64(8.0);
    
    cpu.fpu_push(f10); // ST(1)
    cpu.fpu_push(f8);  // ST(0)

    // Opcode DC D1: FCOM ST(1), ST(0)
    // Semantics: Compare ST(1) [LHS] with ST(0) [RHS]
    // Is 10.0 < 8.0? -> NO. C0 should be 0.
    run_fpu_instr(&mut cpu, &[0xDC, 0xD1]);

    // YOUR CURRENT CODE FAIL:
    // It ignores DC, grabs ST(0)=8.0 as LHS, ST(1)=10.0 as RHS.
    // Checks: 8.0 < 10.0? -> YES. Sets C0=1.
    
    let flags = cpu.get_fpu_flags();
    if flags.contains(FpuFlags::C0) {
        panic!("FATAL: FCOM logic is swapped! 10.0 < 8.0 returned TRUE (C0 set). This causes the '08' bug.");
    }
}