use iced_x86::{Instruction, Mnemonic, OpKind, MemorySize, Register};

use crate::cpu::{Cpu, FpuFlags, CpuFlags, FPU_TAG_EMPTY};
use crate::instructions::utils::calculate_addr;

// Performs the FPU comparison and sets Status Word flags
// Used by FCOM, FCOMP, FCOMPP
fn fpu_compare_values(cpu: &mut Cpu, lhs: f64, rhs: f64) {
    // Clear C0, C2, C3
    cpu.set_fpu_flag(FpuFlags::C0 | FpuFlags::C2 | FpuFlags::C3, false);

    if lhs.is_nan() || rhs.is_nan() {
        // Unordered: C3=1, C2=1, C0=1
        cpu.set_fpu_flag(FpuFlags::C0 | FpuFlags::C2 | FpuFlags::C3, true);
    } else if lhs == rhs {
        // Equal: C3=1
        cpu.set_fpu_flag(FpuFlags::C3, true);
    } else if lhs < rhs {
        // Less Than: C0=1
        cpu.set_fpu_flag(FpuFlags::C0, true);
    }
    // Greater Than: All flags 0
}

pub fn fcom_variants(cpu: &mut Cpu, instr: &Instruction) {
    let (lhs, rhs) = if instr.mnemonic() == Mnemonic::Fcompp {
        // FCOMPP is always ST(0) vs ST(1)
        (cpu.fpu_get(0).get_f64(), cpu.fpu_get(1).get_f64())
    } else {
        match instr.op0_kind() {
            OpKind::Memory => {
                // Memory Comparison is ALWAYS ST(0) vs Memory
                let val_0 = cpu.fpu_get(0).get_f64();
                let addr = calculate_addr(cpu, instr);
                let val_op = match instr.memory_size() {
                    MemorySize::Float32 => f32::from_bits(cpu.bus.read_32(addr)) as f64,
                    MemorySize::Float64 => f64::from_bits(cpu.bus.read_64(addr)),
                    _ => f64::NAN, 
                };
                (val_0, val_op)
            }
            OpKind::Register => {
                // Read raw opcode from memory
                // iced_x86 apparently has a bug with D8 vs DC ambiguity
                let cs_base = (cpu.cs as u32) << 4;
                let instr_addr = (cpu.ip as u32).wrapping_sub(instr.len() as u32);
                let phys_addr = cs_base.wrapping_add(instr_addr) & 0xFFFFF;
                let opcode_byte = cpu.bus.read_8(phys_addr as usize);

                // Identify the operand register index (i)
                // iced_x86 might say "FCOM ST0, ST1" or "FCOM ST1, ST0"
                // We need the register that is NOT ST0 to find 'i'.
                let reg_op0 = instr.op0_register();
                let reg_op1 = instr.op1_register();

                let idx = if reg_op0 != Register::ST0 && reg_op0 != Register::None {
                    reg_op0.number() - Register::ST0.number()
                } else if reg_op1 != Register::ST0 && reg_op1 != Register::None {
                    reg_op1.number() - Register::ST0.number()
                } else {
                    1 // Default to ST(1) if parsing fails or implicit
                };

                let val_i = cpu.fpu_get(idx as usize).get_f64();
                let val_0 = cpu.fpu_get(0).get_f64();

                // Determine direction
                // If memory has 0xDC, it's Reverse.
                // If memory is 0x00 (Unit Test environment), fallback to checking operands.
                let is_reverse = if opcode_byte == 0xDC {
                    true
                } else if opcode_byte == 0xD8 {
                    false
                } else {
                    // Fallback for Unit Tests that don't write to RAM:
                    // If op1 is ST0, iced_x86 usually implies Reverse syntax "FCOM STi, ST0"
                    instr.op1_register() == Register::ST0
                };

                if is_reverse {
                    (val_i, val_0) // ST(i) vs ST(0)
                } else {
                    (val_0, val_i) // ST(0) vs ST(i)
                }
            }
            _ => {
                (cpu.fpu_get(0).get_f64(), cpu.fpu_get(1).get_f64())
            }
        }
    };

    fpu_compare_values(cpu, lhs, rhs);

    match instr.mnemonic() {
        Mnemonic::Fcomp => { cpu.fpu_pop(); },
        Mnemonic::Fcompp => { cpu.fpu_pop(); cpu.fpu_pop(); },
        _ => {}
    }
}

pub fn ficom_variants(cpu: &mut Cpu, instr: &Instruction) {
    let st0 = cpu.fpu_get(0).get_f64();
    let addr = calculate_addr(cpu, instr);
    let val = match instr.memory_size() {
        MemorySize::Int16 => (cpu.bus.read_16(addr) as i16) as f64,
        MemorySize::Int32 => (cpu.bus.read_32(addr) as i32) as f64,
        _ => 0.0,
    };
    fpu_compare_values(cpu, st0, val);
    if instr.mnemonic() == Mnemonic::Ficomp {
        cpu.fpu_pop();
    }
}

// FXAM: Examine ST(0)
pub fn fxam(cpu: &mut Cpu) {
    // Clear C0, C1, C2, C3
    cpu.set_fpu_flag(FpuFlags::C3 | FpuFlags::C2 | FpuFlags::C1 | FpuFlags::C0, false);

    let tag = cpu.fpu_tags[cpu.fpu_top as usize];
    let st0 = cpu.fpu_stack[cpu.fpu_top as usize]; // Access raw stack to avoid fpu_get logic

    // Set C1 to the Sign Bit
    if st0.get_sign() {
        cpu.set_fpu_flag(FpuFlags::C1, true);
    }

    // Categorize based on Tag and Bits
    // Empty:    C3=1, C2=0, C0=1
    // Zero:     C3=1, C2=0, C0=0
    // Normal:   C3=0, C2=1, C0=0
    // Infinity: C3=0, C2=1, C0=1
    // NaN:      C3=0, C2=0, C0=1
    if tag == FPU_TAG_EMPTY {
        cpu.set_fpu_flag(FpuFlags::C3 | FpuFlags::C0, true);
    } else if st0.is_nan() {
        cpu.set_fpu_flag(FpuFlags::C0, true);
    } else if st0.is_zero() {
        cpu.set_fpu_flag(FpuFlags::C3, true);
    } else if st0.is_infinite() {
        cpu.set_fpu_flag(FpuFlags::C2 | FpuFlags::C0, true);
    } else if st0.is_denormal() {
        // Denormal: C3=1, C2=1, C0=0
        cpu.set_fpu_flag(FpuFlags::C3 | FpuFlags::C2, true);
    } else {
        // Normal Finite
        cpu.set_fpu_flag(FpuFlags::C2, true);
    }
}

// FTST: Test ST(0) against 0.0
pub fn ftst(cpu: &mut Cpu) {
    let st0 = cpu.fpu_get(0).get_f64();
    // Compare ST(0) vs 0.0
    fpu_compare_values(cpu, st0, 0.0);
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