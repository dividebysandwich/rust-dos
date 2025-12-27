use iced_x86::{Instruction, Mnemonic, OpKind, MemorySize, Register};

use crate::cpu::{Cpu, FpuFlags, CpuFlags};
use crate::instructions::utils::calculate_addr;

// Performs the FPU comparison and sets Status Word flags
// Used by FCOM, FCOMP, FCOMPP
fn fpu_compare(cpu: &mut Cpu, val: f64) {
    let st0 = cpu.fpu_get(0);

    // Clear Condition Codes C0, C2, C3 (Bits 8, 10, 14)
    cpu.set_fpu_flag(FpuFlags::C0 | FpuFlags::C2 | FpuFlags::C3, false);

    if st0 > val {
        // ST(0) > Source: C3=0, C2=0, C0=0 (All cleared)
    } else if st0 < val {
        // ST(0) < Source: C0=1
        cpu.set_fpu_flag(FpuFlags::C0, true);
    } else if st0 == val {
        // ST(0) == Source: C3=1
        cpu.set_fpu_flag(FpuFlags::C3, true);
    } else {
        // Unordered (NaN): C3=1, C2=1, C0=1
        cpu.set_fpu_flag(FpuFlags::C3 | FpuFlags::C2 | FpuFlags::C0, true);
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
        cpu.set_fpu_flag(FpuFlags::C3 | FpuFlags::C2 | FpuFlags::C1 | FpuFlags::C0, false);
        cpu.set_fpu_flag(FpuFlags::C3 | FpuFlags::C0, true);
        return;
    }
    
    let st0 = cpu.fpu_get(0);
    
    // Clear all condition codes first
    cpu.set_fpu_flag(FpuFlags::C3 | FpuFlags::C2 | FpuFlags::C1 | FpuFlags::C0, false);

    // Check Sign (C1)
    if st0.is_sign_negative() {
        cpu.set_fpu_flag(FpuFlags::C1, true);
    }

    // Categorize
    if st0.is_nan() {
        cpu.set_fpu_flag(FpuFlags::C0, true);
    } else if st0 == 0.0 || st0 == -0.0 {
        cpu.set_fpu_flag(FpuFlags::C3, true);
    } else if st0.is_infinite() {
        cpu.set_fpu_flag(FpuFlags::C2 | FpuFlags::C0, true);
    } else if st0.is_subnormal() { 
        cpu.set_fpu_flag(FpuFlags::C3 | FpuFlags::C2, true);
    } else {
        // Normal Finite
        cpu.set_fpu_flag(FpuFlags::C2, true);
    }
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
    
    cpu.set_cpu_flag(CpuFlags::ZF, zf);
    cpu.set_cpu_flag(CpuFlags::PF, pf);
    cpu.set_cpu_flag(CpuFlags::CF, cf);

    // Pop if P-variant
    match instr.mnemonic() {
        iced_x86::Mnemonic::Fcomip | iced_x86::Mnemonic::Fucomip => { cpu.fpu_pop();},
        _ => { cpu.bus.log_string(&format!("[FPU] Unsupported FCOMI/FUCOMI instruction: {:?}", instr.mnemonic())); }
    }
}