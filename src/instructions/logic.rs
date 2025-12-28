use iced_x86::{Instruction, Mnemonic, OpKind, MemorySize, Register};
use crate::cpu::{Cpu, CpuFlags};
use super::utils::{calculate_addr, is_8bit_reg};

pub fn handle(cpu: &mut Cpu, instr: &Instruction) {
    match instr.mnemonic() {
        Mnemonic::And => logic_op(cpu, instr, |a, b| a & b),
        Mnemonic::Or => logic_op(cpu, instr, |a, b| a | b),
        Mnemonic::Xor => logic_op(cpu, instr, |a, b| a ^ b),
        Mnemonic::Test => test(cpu, instr),
        Mnemonic::Not => not(cpu, instr),
        Mnemonic::Shl | Mnemonic::Sal => shift_op(cpu, instr, Mnemonic::Shl),
        Mnemonic::Shr => shift_op(cpu, instr, Mnemonic::Shr),
        Mnemonic::Sar => shift_op(cpu, instr, Mnemonic::Sar),
        Mnemonic::Rcl => rotate_op(cpu, instr, Mnemonic::Rcl),
        Mnemonic::Rcr => rotate_op(cpu, instr, Mnemonic::Rcr),
        Mnemonic::Rol => rotate_op(cpu, instr, Mnemonic::Rol),
        Mnemonic::Ror => rotate_op(cpu, instr, Mnemonic::Ror),
        Mnemonic::Aad => aad(cpu, instr),
        _ => { cpu.bus.log_string(&format!("[LOGIC] Unsupported instruction: {:?}", instr.mnemonic())); }
    }
}

/// Generic helper for AND, OR, XOR (Read -> Op -> Write -> Flags)
fn logic_op<F>(cpu: &mut Cpu, instr: &Instruction, op: F)
where F: Fn(u16, u16) -> u16 {
    // Determine operand size based on Destination (Op0)
    let is_8bit = match instr.op0_kind() {
        OpKind::Register => is_8bit_reg(instr.op0_register()),
        OpKind::Memory => instr.memory_size() == MemorySize::UInt8,
        _ => false,
    };

    // Read Destination (Op0)
    let (dest, addr_opt) = if instr.op0_kind() == OpKind::Register {
        let reg = instr.op0_register();
        let val = if is_8bit { cpu.get_reg8(reg) as u16 } else { cpu.get_reg16(reg) };
        (val, None)
    } else {
        let addr = calculate_addr(cpu, instr);
        let val = if is_8bit { cpu.bus.read_8(addr) as u16 } else { cpu.bus.read_16(addr) };
        (val, Some(addr))
    };

    // Read Source (Op1)
    let src = if instr.op1_kind() == OpKind::Register {
        if is_8bit { cpu.get_reg8(instr.op1_register()) as u16 } else { cpu.get_reg16(instr.op1_register()) }
    } else if instr.op1_kind() == OpKind::Immediate8 {
        instr.immediate8() as u16
    } else if instr.op1_kind() == OpKind::Immediate8to16 {
        instr.immediate8to16() as u16
    } else if instr.op1_kind() == OpKind::Immediate16 {
        instr.immediate16()
    } else {
        // Memory Source
        let addr = calculate_addr(cpu, instr);
        if is_8bit { cpu.bus.read_8(addr) as u16 } else { cpu.bus.read_16(addr) }
    };

    // Perform Operation
    let res = op(dest, src);

    // Write Back
    if let Some(addr) = addr_opt {
        if is_8bit {
            cpu.bus.write_8(addr, res as u8);
        } else {
            cpu.bus.write_16(addr, res);
        }
    } else {
        let reg = instr.op0_register();
        if is_8bit {
            cpu.set_reg8(reg, res as u8);
        } else {
            cpu.set_reg16(reg, res);
        }
    }

    // Update Flags
    cpu.set_cpu_flag(CpuFlags::ZF, if is_8bit { (res & 0xFF) == 0 } else { res == 0 });
    cpu.set_cpu_flag(CpuFlags::SF, if is_8bit { (res & 0x80) != 0 } else { (res & 0x8000) != 0 });
    cpu.set_cpu_flag(CpuFlags::OF, false);
    cpu.set_cpu_flag(CpuFlags::CF, false);
    cpu.update_pf(res);
}

/// TEST: Same as AND, but discards result
fn test(cpu: &mut Cpu, instr: &Instruction) {
    let is_8bit = match instr.op0_kind() {
        OpKind::Register => is_8bit_reg(instr.op0_register()),
        OpKind::Memory => instr.memory_size() == MemorySize::UInt8,
        _ => false,
    };

    // Read Dest
    let dest = if instr.op0_kind() == OpKind::Register {
        if is_8bit { cpu.get_reg8(instr.op0_register()) as u16 } else { cpu.get_reg16(instr.op0_register()) }
    } else {
        let addr = calculate_addr(cpu, instr);
        if is_8bit { cpu.bus.read_8(addr) as u16 } else { cpu.bus.read_16(addr) }
    };

    // Read Source
    let src = if instr.op1_kind() == OpKind::Register {
        if is_8bit { cpu.get_reg8(instr.op1_register()) as u16 } else { cpu.get_reg16(instr.op1_register()) }
    } else if instr.op1_kind() == OpKind::Immediate8 {
        instr.immediate8() as u16
    } else if instr.op1_kind() == OpKind::Immediate8to16 {
        instr.immediate8to16() as u16
    } else if instr.op1_kind() == OpKind::Immediate16 {
        instr.immediate16()
    } else {
        let addr = calculate_addr(cpu, instr);
        if is_8bit { cpu.bus.read_8(addr) as u16 } else { cpu.bus.read_16(addr) }
    };

    let res = dest & src;

    // Flags Only
    cpu.set_cpu_flag(CpuFlags::ZF, if is_8bit { (res & 0xFF) == 0 } else { res == 0 });
    cpu.set_cpu_flag(CpuFlags::SF, if is_8bit { (res & 0x80) != 0 } else { (res & 0x8000) != 0 });
    cpu.set_cpu_flag(CpuFlags::OF, false);
    cpu.set_cpu_flag(CpuFlags::CF, false);
    cpu.update_pf(res);
}

/// NOT: Invert bits (One's Complement)
fn not(cpu: &mut Cpu, instr: &Instruction) {
    let is_8bit = match instr.op0_kind() {
        OpKind::Register => is_8bit_reg(instr.op0_register()),
        OpKind::Memory => instr.memory_size() == MemorySize::UInt8,
        _ => false,
    };

    // Read
    let (val, addr_opt) = if instr.op0_kind() == OpKind::Register {
        let reg = instr.op0_register();
        let v = if is_8bit { cpu.get_reg8(reg) as u16 } else { cpu.get_reg16(reg) };
        (v, None)
    } else {
        let addr = calculate_addr(cpu, instr);
        let v = if is_8bit { cpu.bus.read_8(addr) as u16 } else { cpu.bus.read_16(addr) };
        (v, Some(addr))
    };

    // Invert
    let res = !val;

    // Write
    if let Some(addr) = addr_opt {
        if is_8bit { cpu.bus.write_8(addr, res as u8); } else { cpu.bus.write_16(addr, res); }
    } else {
        let reg = instr.op0_register();
        if is_8bit { cpu.set_reg8(reg, res as u8); } else { cpu.set_reg16(reg, res); }
    }
    
    // NOT does not modify flags
}

/// Helper to get shift count from Op1 or CL
fn get_shift_count(cpu: &Cpu, instr: &Instruction) -> u32 {
    // x86 shifts/rotates usually have the count in Op1
    match instr.op1_kind() {
        OpKind::Immediate8 => instr.immediate8() as u32,
        OpKind::Register => cpu.get_reg8(instr.op1_register()) as u32,
        _ => 1, // Fallback for single-operand decodings
    }
}

/// SHL, SHR, SAR
fn shift_op(cpu: &mut Cpu, instr: &Instruction, mnemonic: Mnemonic) {
    let is_8bit = match instr.op0_kind() {
        OpKind::Register => is_8bit_reg(instr.op0_register()),
        OpKind::Memory => instr.memory_size() == MemorySize::UInt8,
        _ => false,
    };

    let (val, addr_opt) = if instr.op0_kind() == OpKind::Register {
        let reg = instr.op0_register();
        (if is_8bit { cpu.get_reg8(reg) as u16 } else { cpu.get_reg16(reg) }, None)
    } else {
        let addr = calculate_addr(cpu, instr);
        (if is_8bit { cpu.bus.read_8(addr) as u16 } else { cpu.bus.read_16(addr) }, Some(addr))
    };

    let count = get_shift_count(cpu, instr) & 0x1F;
    if count == 0 { return; }

    let bit_width = if is_8bit { 8 } else { 16 };
    let mut res = val;
    let mut last_out = false;

    for _ in 0..count {
        match mnemonic {
            Mnemonic::Shl | Mnemonic::Sal => {
                last_out = (res & (1 << (bit_width - 1))) != 0;
                res <<= 1;
            },
            Mnemonic::Shr => {
                last_out = (res & 1) != 0;
                res >>= 1;
            },
            Mnemonic::Sar => {
                last_out = (res & 1) != 0;
                let msb_mask = 1 << (bit_width - 1);
                let msb = res & msb_mask;
                res = (res >> 1) | msb; // Sign extension
            },
            _ => {
                // If we hit this, the dispatcher sent a mnemonic we aren't handling
                println!("[DEBUG] shift_op got unexpected mnemonic: {:?}", mnemonic);
                return;
            }
        }
    }

    if is_8bit { res &= 0xFF; }

    // Write Back
    if let Some(addr) = addr_opt {
        if is_8bit { cpu.bus.write_8(addr, res as u8); } else { cpu.bus.write_16(addr, res); }
    } else {
        let reg = instr.op0_register();
        if is_8bit { cpu.set_reg8(reg, res as u8); } else { cpu.set_reg16(reg, res); }
    }

    // Flags
    cpu.set_cpu_flag(CpuFlags::ZF, if is_8bit { (res as u8) == 0 } else { res == 0 });
    cpu.set_cpu_flag(CpuFlags::SF, if is_8bit { (res & 0x80) != 0 } else { (res & 0x8000) != 0 });
    cpu.set_cpu_flag(CpuFlags::CF, last_out);
    cpu.update_pf(res);
}

fn rotate_op(cpu: &mut Cpu, instr: &Instruction, mnemonic: Mnemonic) {
    let is_8bit = match instr.op0_kind() {
        OpKind::Register => is_8bit_reg(instr.op0_register()),
        OpKind::Memory => instr.memory_size() == MemorySize::UInt8,
        _ => false,
    };

    let (mut val, addr_opt) = if instr.op0_kind() == OpKind::Register {
        let reg = instr.op0_register();
        (if is_8bit { cpu.get_reg8(reg) as u16 } else { cpu.get_reg16(reg) }, None)
    } else {
        let addr = calculate_addr(cpu, instr);
        (if is_8bit { cpu.bus.read_8(addr) as u16 } else { cpu.bus.read_16(addr) }, Some(addr))
    };

    let count = get_shift_count(cpu, instr) & 0x1F;
    if count == 0 { return; }

    let width = if is_8bit { 8 } else { 16 };

    for _ in 0..count {
        let old_cf = cpu.get_cpu_flag(CpuFlags::CF);
        let msb_mask = 1 << (width - 1);
        
        match mnemonic {
            Mnemonic::Rol => {
                let msb = (val & msb_mask) != 0;
                val = (val << 1) | (if msb { 1 } else { 0 });
                cpu.set_cpu_flag(CpuFlags::CF, msb);
            },
            Mnemonic::Ror => {
                let lsb = (val & 1) != 0;
                val = (val >> 1) | (if lsb { msb_mask } else { 0 });
                cpu.set_cpu_flag(CpuFlags::CF, lsb);
            },
            Mnemonic::Rcl => {
                let msb = (val & msb_mask) != 0;
                val = (val << 1) | (if old_cf { 1 } else { 0 });
                cpu.set_cpu_flag(CpuFlags::CF, msb);
            },
            Mnemonic::Rcr => {
                let lsb = (val & 1) != 0;
                val = (val >> 1) | (if old_cf { msb_mask } else { 0 });
                cpu.set_cpu_flag(CpuFlags::CF, lsb);
            },
            _ => unreachable!(),
        }
        if is_8bit { val &= 0xFF; }
    }

    if let Some(addr) = addr_opt {
        if is_8bit { cpu.bus.write_8(addr, val as u8); } else { cpu.bus.write_16(addr, val); }
    } else {
        let reg = instr.op0_register();
        if is_8bit { cpu.set_reg8(reg, val as u8); } else { cpu.set_reg16(reg, val); }
    }
}

fn aad(cpu: &mut Cpu, instr: &Instruction) {
    // Determine base (usually 10)
    let base = if instr.op_count() > 0 && instr.op0_kind() == OpKind::Immediate8 {
        instr.immediate8()
    } else {
        10
    };
    
    let al = cpu.get_al();
    let ah = cpu.get_ah();
    
    let res = (al as u16).wrapping_add((ah as u16).wrapping_mul(base as u16));
    
    cpu.set_reg8(Register::AL, (res & 0xFF) as u8);
    cpu.set_reg8(Register::AH, 0);
    
    cpu.set_cpu_flag(CpuFlags::SF, (res & 0x80) != 0);
    cpu.set_cpu_flag(CpuFlags::ZF, (res & 0xFF) == 0);
    cpu.update_pf(res);
}