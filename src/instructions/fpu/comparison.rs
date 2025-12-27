use iced_x86::{Instruction, Mnemonic, OpKind, MemorySize, Register};

use crate::cpu::{Cpu, FpuFlags, CpuFlags};
use crate::f80::F80;
use crate::instructions::utils::calculate_addr;

// Performs the FPU comparison and sets Status Word flags
// Used by FCOM, FCOMP, FCOMPP
fn fpu_compare(cpu: &mut Cpu, val: F80) {
    let st0 = cpu.fpu_get(0);

    // Clear Condition Codes C0, C2, C3 (Bits 8, 10, 14)
    cpu.set_fpu_flag(FpuFlags::C0 | FpuFlags::C2 | FpuFlags::C3, false);

    if st0.is_nan() || val.is_nan() {
        // Unordered (NaN): C3=1, C2=1, C0=1
        cpu.set_fpu_flag(FpuFlags::C3 | FpuFlags::C2 | FpuFlags::C0, true);
    } else if st0.get() == val.get() {
        // ST(0) == Source: C3=1
        cpu.set_fpu_flag(FpuFlags::C3, true);
    } else {
        // Since F80 is a custom struct, we use get_f64() for the magnitude check.
        // TODO: implement F80::less_than.
        let a = st0.get_f64();
        let b = val.get_f64();

        if a < b {
            // ST(0) < Source: C0=1
            cpu.set_fpu_flag(FpuFlags::C0, true);
        }
        // else: ST(0) > Source: C3=0, C2=0, C0=0 (Already cleared)
    }
}

pub fn fcom_variants(cpu: &mut Cpu, instr: &Instruction) {
    // Determine the value to compare against as an F80
    let val = if instr.mnemonic() == Mnemonic::Fcompp {
        cpu.fpu_get(1)
    } else {
        match instr.op0_kind() {
            OpKind::Memory => {
                let addr = calculate_addr(cpu, instr);
                let mut f = F80::new();
                match instr.memory_size() {
                    MemorySize::Float32 => {
                        f.set_f64(f32::from_bits(cpu.bus.read_32(addr)) as f64);
                    }
                    MemorySize::Float64 => {
                        f.set_f64(f64::from_bits(cpu.bus.read_64(addr)));
                    }
                    _ => f.set_real_indefinite(),
                }
                f
            }
            OpKind::Register => {
                let idx = instr.op0_register().number() - Register::ST0.number();
                cpu.fpu_get(idx as usize)
            }
            _ => cpu.fpu_get(1), // Default implicit ST(1)
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
        // C3=1, C2=0, C1=0/1(Sign), C0=1 (Empty)
        cpu.set_fpu_flag(FpuFlags::C3 | FpuFlags::C2 | FpuFlags::C1 | FpuFlags::C0, false);
        cpu.set_fpu_flag(FpuFlags::C3 | FpuFlags::C0, true);
        return;
    }
    
    let st0 = cpu.fpu_get(0);
    
    // Clear all condition codes first
    cpu.set_fpu_flag(FpuFlags::C3 | FpuFlags::C2 | FpuFlags::C1 | FpuFlags::C0, false);

    // Check Sign (C1)
    if st0.get_sign() {
        cpu.set_fpu_flag(FpuFlags::C1, true);
    }

    // Categorize
    if st0.is_nan() {
        cpu.set_fpu_flag(FpuFlags::C0, true);
    } else if st0.is_zero() {
        cpu.set_fpu_flag(FpuFlags::C3, true);
    } else if st0.is_infinite() {
        cpu.set_fpu_flag(FpuFlags::C2 | FpuFlags::C0, true);
    } else if st0.is_denormal() { 
        cpu.set_fpu_flag(FpuFlags::C3 | FpuFlags::C2, true);
    } else {
        // Normal Finite
        cpu.set_fpu_flag(FpuFlags::C2, true);
    }
}

// FTST: Test ST(0) against 0.0
pub fn ftst(cpu: &mut Cpu) {
    fpu_compare(cpu, F80::new());
}

// FICOM/FICOMP: Integer Compare
pub fn ficom_variants(cpu: &mut Cpu, instr: &Instruction) {
    let addr = calculate_addr(cpu, instr);
    let val = cpu.load_int_to_f80(addr, instr.memory_size());
    
    fpu_compare(cpu, val);
    
    if instr.mnemonic() == iced_x86::Mnemonic::Ficomp {
        cpu.fpu_pop();
    }
}

// FCOMI/FUCOMI... (Pentium Pro+)
// These set CPU EFLAGS (ZF, PF, CF) directly, not the FPU status word condition codes.
pub fn fcomi_variants(cpu: &mut Cpu, instr: &Instruction) {
    let idx = (instr.op1_register().number() - iced_x86::Register::ST0.number()) as usize;
    let st0 = cpu.fpu_get(0);
    let sti = cpu.fpu_get(idx);
    
    // Set ZF, PF, CF based on comparison
    // ZF=1 if Equal, CF=1 if Less, PF=1 if NaN
    #[allow(unused_assignments)]
    let mut zf = false;
    #[allow(unused_assignments)]
    let mut pf = false;
    #[allow(unused_assignments)]
    let mut cf = false;
    
    if st0.is_nan() || sti.is_nan() {
        zf = true; pf = true; cf = true; // "Unordered"
    } else if st0.get() == sti.get() {
        zf = true; pf = false; cf = false; // Equal
    } else {
        // For magnitude comparison, get_f64 is sufficient 
        let a = st0.get_f64();
        let b = sti.get_f64();
        if a < b {
            zf = false; pf = false; cf = true;
        } else {
            zf = false; pf = false; cf = false;
        }
    }
    
    cpu.set_cpu_flag(CpuFlags::ZF, zf);
    cpu.set_cpu_flag(CpuFlags::PF, pf);
    cpu.set_cpu_flag(CpuFlags::CF, cf);

    // Pop if P-variant (FCOMIP / FUCOMIP)
    let m = instr.mnemonic();
    if m == iced_x86::Mnemonic::Fcomip || m == iced_x86::Mnemonic::Fucomip {
        cpu.fpu_pop();
    }
}