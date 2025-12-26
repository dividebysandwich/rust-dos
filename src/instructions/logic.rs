use iced_x86::{Instruction, Mnemonic, OpKind, MemorySize};
use crate::cpu::{Cpu, FLAG_ZF, FLAG_SF, FLAG_OF, FLAG_CF};
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
        _ => {}
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
    cpu.set_flag(FLAG_ZF, if is_8bit { (res & 0xFF) == 0 } else { res == 0 });
    cpu.set_flag(FLAG_SF, if is_8bit { (res & 0x80) != 0 } else { (res & 0x8000) != 0 });
    cpu.set_flag(FLAG_OF, false);
    cpu.set_flag(FLAG_CF, false);
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
    cpu.set_flag(FLAG_ZF, if is_8bit { (res & 0xFF) == 0 } else { res == 0 });
    cpu.set_flag(FLAG_SF, if is_8bit { (res & 0x80) != 0 } else { (res & 0x8000) != 0 });
    cpu.set_flag(FLAG_OF, false);
    cpu.set_flag(FLAG_CF, false);
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
    if instr.op1_kind() == OpKind::Immediate8 {
        instr.immediate8() as u32
    } else if instr.op1_kind() == OpKind::Register {
        cpu.get_reg8(instr.op1_register()) as u32
    } else {
        1
    }
}

/// SHL, SHR, SAR
fn shift_op(cpu: &mut Cpu, instr: &Instruction, mnemonic: Mnemonic) {
    let is_8bit = match instr.op0_kind() {
        OpKind::Register => is_8bit_reg(instr.op0_register()),
        OpKind::Memory => instr.memory_size() == MemorySize::UInt8,
        _ => false,
    };

    // Read Dest
    let (val, addr_opt) = if instr.op0_kind() == OpKind::Register {
        let reg = instr.op0_register();
        let v = if is_8bit { cpu.get_reg8(reg) as u16 } else { cpu.get_reg16(reg) };
        (v, None)
    } else {
        let addr = calculate_addr(cpu, instr);
        let v = if is_8bit { cpu.bus.read_8(addr) as u16 } else { cpu.bus.read_16(addr) };
        (v, Some(addr))
    };

    let count = get_shift_count(cpu, instr);
    // Mask count (x86 masks shift count to 5 bits i.e. 0-31)
    let effective_count = count & 0x1F;

    if effective_count == 0 { return; }

    let (res, last_out) = match mnemonic {
        Mnemonic::Shl | Mnemonic::Sal => {
            let res = val.wrapping_shl(effective_count);
            // CF is the last bit shifted out
            let bit_pos = if is_8bit { 8 } else { 16 };
            let last_out = if effective_count <= bit_pos {
                (val >> (bit_pos - effective_count)) & 1
            } else { 0 };
            (res, last_out)
        },
        Mnemonic::Shr => {
            let res = val.wrapping_shr(effective_count);
            let last_out = (val >> (effective_count - 1)) & 1;
            (res, last_out)
        },
        Mnemonic::Sar => {
            let res = if is_8bit {
                (val as i8).wrapping_shr(effective_count) as u16
            } else {
                (val as i16).wrapping_shr(effective_count) as u16
            };
            // CF logic for SAR: Copy sign bit if shifting, or last bit out
            let last_out = if is_8bit {
                ((val as u8 >> (effective_count - 1)) & 1) as u16
            } else {
                (val >> (effective_count - 1)) & 1
            };
            (res, last_out as u16)
        },
        _ => (val, 0),
    };

    // Write Back
    if let Some(addr) = addr_opt {
        if is_8bit { cpu.bus.write_8(addr, res as u8); } else { cpu.bus.write_16(addr, res); }
    } else {
        let reg = instr.op0_register();
        if is_8bit { cpu.set_reg8(reg, res as u8); } else { cpu.set_reg16(reg, res); }
    }

    // Flags
    cpu.set_flag(FLAG_ZF, if is_8bit { (res & 0xFF) == 0 } else { res == 0 });
    cpu.set_flag(FLAG_SF, if is_8bit { (res & 0x80) != 0 } else { (res & 0x8000) != 0 });
    cpu.set_flag(FLAG_CF, last_out != 0);
    cpu.update_pf(res);
    
    // OF is only defined for count == 1
    // Simplification: Not fully implementing OF for shifts here as it varies per instruction
}

/// RCL, RCR, ROL, ROR
fn rotate_op(cpu: &mut Cpu, instr: &Instruction, mnemonic: Mnemonic) {
    let is_8bit = match instr.op0_kind() {
        OpKind::Register => is_8bit_reg(instr.op0_register()),
        OpKind::Memory => instr.memory_size() == MemorySize::UInt8,
        _ => false,
    };

    let (mut val, addr_opt) = if instr.op0_kind() == OpKind::Register {
        let reg = instr.op0_register();
        let v = if is_8bit { cpu.get_reg8(reg) as u16 } else { cpu.get_reg16(reg) };
        (v, None)
    } else {
        let addr = calculate_addr(cpu, instr);
        let v = if is_8bit { cpu.bus.read_8(addr) as u16 } else { cpu.bus.read_16(addr) };
        (v, Some(addr))
    };

    let count = get_shift_count(cpu, instr);
    let effective_count = count & 0x1F;

    if effective_count == 0 { return; }

    // Bits width
    let width = if is_8bit { 8 } else { 16 };
    let msb_mask = 1 << (width - 1);

    for _ in 0..effective_count {
        let old_cf = if cpu.get_flag(FLAG_CF) { 1 } else { 0 };
        let new_cf;

        match mnemonic {
            Mnemonic::Rcl => {
                // Rotate Left through Carry: CF <- MSB, LSB <- Old CF
                let msb = (val & msb_mask) != 0;
                new_cf = if msb { 1 } else { 0 };
                val = (val << 1) | old_cf;
            },
            Mnemonic::Rcr => {
                // Rotate Right through Carry: CF <- LSB, MSB <- Old CF
                let lsb = (val & 1) != 0;
                new_cf = if lsb { 1 } else { 0 };
                val = (val >> 1) | (old_cf << (width - 1));
            },
            Mnemonic::Rol => {
                // Rotate Left: CF <- MSB, LSB <- MSB
                let msb = (val & msb_mask) != 0;
                new_cf = if msb { 1 } else { 0 };
                val = (val << 1) | new_cf;
            },
            Mnemonic::Ror => {
                // Rotate Right: CF <- LSB, MSB <- LSB
                let lsb = (val & 1) != 0;
                new_cf = if lsb { 1 } else { 0 };
                val = (val >> 1) | (new_cf << (width - 1));
            },
            _ => { new_cf = 0; }
        }
        
        // Mask value to correct size
        if is_8bit { val &= 0xFF; }

        cpu.set_flag(FLAG_CF, new_cf != 0);
    }

    // Write Back
    if let Some(addr) = addr_opt {
        if is_8bit { cpu.bus.write_8(addr, val as u8); } else { cpu.bus.write_16(addr, val); }
    } else {
        let reg = instr.op0_register();
        if is_8bit { cpu.set_reg8(reg, val as u8); } else { cpu.set_reg16(reg, val); }
    }

    // OF Logic for Count == 1
    if effective_count == 1 {
        let msb = if is_8bit { (val >> 7) & 1 } else { (val >> 15) & 1 };
        let cf = if cpu.get_flag(FLAG_CF) { 1 } else { 0 };
        
        match mnemonic {
            Mnemonic::Rcl | Mnemonic::Rol => cpu.set_flag(FLAG_OF, (msb ^ cf) != 0),
            Mnemonic::Rcr | Mnemonic::Ror => {
                // OF = MSB ^ (Bit next to MSB) -> effectively (MSB ^ New MSB) logic varies slightly by manual
                // Simplified: OF set if sign bit changed
                // (This is a simplification, exact x86 XORs top two bits)
                let msb_prev = if is_8bit { (val >> 6) & 1 } else { (val >> 14) & 1 }; // Approximate
                cpu.set_flag(FLAG_OF, (msb ^ msb_prev) != 0); 
            }
            _ => {}
        }
    }
}