use iced_x86::{Instruction, OpKind, Register};
use crate::cpu::{Cpu, FPU_TAG_EMPTY, FPU_TAG_VALID, FpuFlags};
use crate::f80::F80;
use crate::instructions::utils::calculate_addr;

pub fn fninit(cpu: &mut Cpu) {
    // Initialize FPU
    cpu.fpu_top = 0;
    // Clear stack for debug clarity
    cpu.fpu_stack = [F80::new(); 8];
    cpu.fpu_control = 0x037F;
    // Reset FPU status registers here.
    cpu.set_fpu_flags(FpuFlags::empty());
    // Clear stack
    for i in 0..8 {
        cpu.fpu_tags[i] = FPU_TAG_EMPTY;
    }
}

// FNCLEX: Clear FPU Exceptions
pub fn fnclex(cpu: &mut Cpu) {
    // This clears IE, DE, ZE, OE, UE, PE, SF, ES, and the Busy bit.
    // It leaves the TOP pointer and Condition Codes (C0-C3) untouched.
    cpu.set_fpu_flag(FpuFlags::EXCEPTIONS, false);
}

// FLDCW: Load Control Word from Memory
pub fn fldcw(cpu: &mut Cpu, instr: &Instruction) {
    let addr = calculate_addr(cpu, instr);
    let cw = cpu.lin_read_16(addr);
    cpu.fpu_control = cw;
}

// FNSTCW: Store Control Word
// Programs read this to modify rounding settings, then write it back with FLDCW.
pub fn fnstcw(cpu: &mut Cpu, instr: &Instruction) {
    let addr = calculate_addr(cpu, instr);
    cpu.lin_write_16(addr, cpu.fpu_control);
}

// FNSTSW: Store FPU Status Word (No Wait)
// Usually: FNSTSW AX  or  FNSTSW [mem]
pub fn fnstsw(cpu: &mut Cpu, instr: &Instruction) {
    let flags = cpu.get_fpu_flags();
    
    // FPU Top is usually stored in bits 11-13 of the Status Word.
    // But we store it separately in our CPU struct, so we need to combine them.
    let mut raw_bits = flags.bits();
    raw_bits = (raw_bits & !0x3800) | ((cpu.fpu_top as u16 & 0x07) << 11);

    if instr.op0_kind() == OpKind::Register {
        if instr.op0_register() == Register::AX {
            cpu.set_ax(raw_bits);
        }
    } else if instr.op0_kind() == OpKind::Memory {
        let addr = calculate_addr(cpu, instr);
        cpu.lin_write_16(addr, raw_bits);
    }
}

pub fn ffree(cpu: &mut Cpu, instr: &Instruction) {
    let reg_offset = instr.op0_register().number() - iced_x86::Register::ST0.number();
    let phys_idx = cpu.fpu_get_phys_index(reg_offset as usize);
    
    // Mark as EMPTY
    cpu.fpu_tags[phys_idx] = crate::cpu::FPU_TAG_EMPTY;
}

// FINCSTP: Increment Stack Top Pointer
// This simply rotates the stack pointer. It does NOT push/pop values or change tags.
pub fn fincstp(cpu: &mut Cpu) {
    cpu.fpu_top = (cpu.fpu_top.wrapping_add(1)) & 7;
}

// FDECSTP: Decrement Stack Top Pointer
pub fn fdecstp(cpu: &mut Cpu) {
    cpu.fpu_top = (cpu.fpu_top.wrapping_sub(1)) & 7;
}

/// The status word: the flags with the stack top in bits 11-13.
fn status_word(cpu: &Cpu) -> u16 {
    (cpu.get_fpu_flags().bits() & !0x3800) | ((cpu.fpu_top as u16 & 0x07) << 11)
}

/// The tag word: two bits per physical register, 00 valid, 01 zero,
/// 10 special (NaN, infinity), 11 empty.
fn tag_word(cpu: &Cpu) -> u16 {
    let mut tag_word: u16 = 0;
    for i in 0..8 {
        let tag = if cpu.fpu_tags[i] == FPU_TAG_EMPTY {
            0b11
        } else {
            let val = cpu.fpu_stack[i];
            if val.is_zero() {
                0b01
            } else if val.is_nan() || val.is_infinite() {
                0b10
            } else {
                0b00
            }
        };
        tag_word |= tag << (i * 2);
    }
    tag_word
}

/// Size of the environment image: 28 bytes with a 32-bit operand size, 14
/// with a 16-bit one.
fn env_size(instr: &Instruction) -> usize {
    match instr.memory_size().size() {
        28 | 108 => 28,
        _ => 14,
    }
}

/// Write the environment (control, status and tag words, then the
/// instruction and operand pointers, which aren't tracked) at `addr`.
fn store_env(cpu: &mut Cpu, addr: usize, size: usize) {
    let (cw, sw, tw) = (cpu.fpu_control, status_word(cpu), tag_word(cpu));
    if size == 28 {
        // Each field takes a dword; the reserved upper words read as 1s.
        for (i, w) in [cw, sw, tw].into_iter().enumerate() {
            cpu.lin_write_32(addr + 4 * i, 0xFFFF_0000 | w as u32);
        }
        for i in 3..7 {
            cpu.lin_write_32(addr + 4 * i, 0);
        }
    } else {
        for (i, w) in [cw, sw, tw, 0, 0, 0, 0].into_iter().enumerate() {
            cpu.lin_write_16(addr + 2 * i, w);
        }
    }
}

/// Load the environment `store_env` writes.
fn load_env(cpu: &mut Cpu, addr: usize, size: usize) {
    let step = if size == 28 { 4 } else { 2 };
    cpu.fpu_control = cpu.lin_read_16(addr);
    let sw = cpu.lin_read_16(addr + step);
    let tag_word = cpu.lin_read_16(addr + 2 * step);
    cpu.fpu_top = ((sw >> 11) & 0x07) as usize;
    // Mask out the TOP bits before setting flags to avoid corruption
    cpu.set_fpu_flags(FpuFlags::from_bits_truncate(sw & !0x3800));
    for i in 0..8 {
        let tag = (tag_word >> (i * 2)) & 0x03;
        cpu.fpu_tags[i] = if tag == 0b11 { FPU_TAG_EMPTY } else { FPU_TAG_VALID };
    }
}

/// FSTENV/FNSTENV: store the environment, then mask all exceptions.
pub fn fnstenv(cpu: &mut Cpu, instr: &Instruction) {
    let addr = calculate_addr(cpu, instr);
    store_env(cpu, addr, env_size(instr));
    cpu.fpu_control |= 0x3F;
}

/// FLDENV: load the environment.
pub fn fldenv(cpu: &mut Cpu, instr: &Instruction) {
    let addr = calculate_addr(cpu, instr);
    load_env(cpu, addr, env_size(instr));
}

/// FSAVE/FNSAVE: store the environment and the registers ST(0) to ST(7),
/// 10 bytes each (94 or 108 bytes in all), then initialize the FPU.
pub fn fnsave(cpu: &mut Cpu, instr: &Instruction) {
    let addr = calculate_addr(cpu, instr);
    let size = env_size(instr);
    store_env(cpu, addr, size);
    for i in 0..8 {
        let bytes = cpu.fpu_stack[cpu.fpu_get_phys_index(i)].get_bytes();
        for (b, &byte) in bytes.iter().enumerate() {
            cpu.lin_write_8(addr + size + 10 * i + b, byte);
        }
    }
    fninit(cpu);
}

/// FRSTOR: load the image FSAVE stores.
pub fn frstor(cpu: &mut Cpu, instr: &Instruction) {
    let addr = calculate_addr(cpu, instr);
    let size = env_size(instr);
    load_env(cpu, addr, size);
    for i in 0..8 {
        let mut bytes = [0u8; 10];
        for (b, byte) in bytes.iter_mut().enumerate() {
            *byte = cpu.lin_read_8(addr + size + 10 * i + b);
        }
        let phys = cpu.fpu_get_phys_index(i);
        cpu.fpu_stack[phys].set_bytes(&bytes);
    }
}
