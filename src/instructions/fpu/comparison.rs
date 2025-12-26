use iced_x86::{Instruction, Mnemonic, OpKind, MemorySize, Register};

use crate::cpu::{Cpu, FLAG_CF, FLAG_PF, FLAG_ZF, FPU_C0, FPU_C1, FPU_C2, FPU_C3};
use crate::instructions::utils::calculate_addr;

// Performs the FPU comparison and sets Status Word flags
// Used by FCOM, FCOMP, FCOMPP
fn fpu_compare(cpu: &mut Cpu, val: f64) {
    let st0 = cpu.fpu_get(0);

    // Clear Condition Codes C0, C2, C3 (Bits 8, 10, 14)
    // Mask = 0x4500
    cpu.fpu_status &= !0x4500;

    if st0 > val {
        // ST(0) > Source: C3=0, C2=0, C0=0 (All cleared)
    } else if st0 < val {
        // ST(0) < Source: C0=1
        cpu.fpu_status |= FPU_C0;
    } else if st0 == val {
        // ST(0) == Source: C3=1
        cpu.fpu_status |= FPU_C3;
    } else {
        // Unordered (NaN): C3=1, C2=1, C0=1
        cpu.fpu_status |= FPU_C3 | FPU_C2 | FPU_C0; // 0x4500
    }
}

pub fn fcom_variants(cpu: &mut Cpu, instr: &Instruction) {
    // Determine the value to compare against
    let val = if instr.mnemonic() == Mnemonic::Fcompp {
        // FCOMPP always compares ST(0) with ST(1)
        cpu.fpu_get(1)
    } else {
        // FCOM / FCOMP can take Memory, Register, or Default to ST(1)
        if instr.op0_kind() == OpKind::Memory {
            let addr = calculate_addr(cpu, instr);
            match instr.memory_size() {
                MemorySize::Float32 => f32::from_bits(cpu.bus.read_32(addr)) as f64,
                MemorySize::Float64 => f64::from_bits(cpu.bus.read_64(addr)),
                _ => 0.0, // Should probably be NaN
            }
        } else if instr.op0_kind() == OpKind::Register {
            let idx = instr.op0_register().number() - Register::ST0.number();
            cpu.fpu_get(idx as usize)
        } else {
            cpu.fpu_get(1) // Default implicit ST(1)
        }
    };

    // Perform Comparison
    fpu_compare(cpu, val);

    // Handle Pops
    match instr.mnemonic() {
        Mnemonic::Fcomp => {
            cpu.fpu_pop(); // Pop 1
        }
        Mnemonic::Fcompp => {
            cpu.fpu_pop(); // Pop 1
            cpu.fpu_pop(); // Pop 2
        }
        _ => {} // FCOM pops nothing
    }
}

// FXAM: Examine ST(0)
pub fn fxam(cpu: &mut Cpu) {
    // Check Tag First
    if cpu.fpu_tags[cpu.fpu_top] == crate::cpu::FPU_TAG_EMPTY {
        // C3=1, C2=0, C0=1 (Empty)
        cpu.fpu_status &= !0x4700;
        cpu.fpu_status |= 0x4100;
        return;
    }
    
    let st0 = cpu.fpu_get(0);
    let mut c0 = 0;
    let mut c2 = 0;
    let mut c3 = 0;
    let mut c1 = 0; // Sign

    // Check Sign
    if st0.is_sign_negative() {
        c1 = 1;
    }

    // Categorize
    if st0.is_nan() {
        // NaN: C3=0, C2=0, C0=1
        c0 = 1;
    } else if st0 == 0.0 || st0 == -0.0 {
        // Zero: C3=1, C2=0, C0=0
        c3 = 1;
    } else if st0.is_infinite() {
        // Infinity: C3=0, C2=1, C0=1
        c2 = 1;
        c0 = 1;
    } else if st0.is_subnormal() { 
        // Denormal: C3=1, C2=1, C0=0
        c3 = 1;
        c2 = 1;
    } else {
        // Normal Finite: C3=0, C2=1, C0=0
        c2 = 1;
    }

    // Update Status Word
    // C0 is Bit 8, C1 is Bit 9, C2 is Bit 10, C3 is Bit 14
    let mask = 0x4700; // Clear C3, C2, C1, C0
    cpu.fpu_status &= !mask;

    if c0 == 1 { cpu.fpu_status |= FPU_C0; } // Bit 8
    if c1 == 1 { cpu.fpu_status |= FPU_C1; } // Bit 9
    if c2 == 1 { cpu.fpu_status |= FPU_C2; } // Bit 10
    if c3 == 1 { cpu.fpu_status |= FPU_C3; } // Bit 14
}

// FTST: Test ST(0) against 0.0
pub fn ftst(cpu: &mut Cpu) {
    fpu_compare(cpu, 0.0);
}

// FICOM/FICOMP: Integer Compare
pub fn ficom_variants(cpu: &mut Cpu, instr: &Instruction) {
    let addr = crate::instructions::utils::calculate_addr(cpu, instr);
    let val = match instr.memory_size() {
        iced_x86::MemorySize::Int16 => (cpu.bus.read_16(addr) as i16) as f64,
        iced_x86::MemorySize::Int32 => (cpu.bus.read_32(addr) as i32) as f64,
        _ => 0.0,
    };
    
    fpu_compare(cpu, val);
    
    if instr.mnemonic() == iced_x86::Mnemonic::Ficomp {
        cpu.fpu_pop();
    }
}

// FCOMI/FUCOMI... (Pentium Pro)
// These set CPU EFLAGS (ZF, PF, CF) directly, not the FPU status word.
pub fn fcomi_variants(cpu: &mut Cpu, instr: &Instruction) {
    let idx = instr.op0_register().number() - iced_x86::Register::ST0.number();
    let st0 = cpu.fpu_get(0);
    let sti = cpu.fpu_get(idx as usize);
    
    // Set ZF, PF, CF based on comparison
    // ZF=1 if Equal, CF=1 if Less, PF=1 if NaN
    #[allow(unused_assignments)]
    let mut zf = false;
    #[allow(unused_assignments)]
    let mut cf = false;
    #[allow(unused_assignments)]
    let mut pf = false;
    
    if st0.is_nan() || sti.is_nan() {
        zf = true; pf = true; cf = true; // "Unordered"
    } else if st0 > sti {
        zf = false; pf = false; cf = false;
    } else if st0 < sti {
        zf = false; pf = false; cf = true;
    } else {
        zf = true; pf = false; cf = false;
    }
    
    cpu.set_flag(FLAG_ZF, zf);
    cpu.set_flag(FLAG_PF, pf);
    cpu.set_flag(FLAG_CF, cf);
    
    // Pop if P-variant
    match instr.mnemonic() {
        iced_x86::Mnemonic::Fcomip | iced_x86::Mnemonic::Fucomip => { cpu.fpu_pop();},
        _ => {}
    }
}