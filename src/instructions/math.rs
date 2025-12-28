use iced_x86::{Instruction, Mnemonic, OpKind, MemorySize, Register};
use crate::cpu::{Cpu, CpuFlags};
use crate::interrupts;
use super::utils::{calculate_addr, is_8bit_reg};

pub fn handle(cpu: &mut Cpu, instr: &Instruction) {
    match instr.mnemonic() {
        Mnemonic::Add => add(cpu, instr),
        Mnemonic::Sub => sub(cpu, instr),
        Mnemonic::Adc => adc(cpu, instr),
        Mnemonic::Sbb => sbb(cpu, instr),
        Mnemonic::Inc => inc(cpu, instr),
        Mnemonic::Dec => dec(cpu, instr),
        Mnemonic::Neg => neg(cpu, instr),
        Mnemonic::Cmp => cmp(cpu, instr),
        Mnemonic::Mul => mul(cpu, instr),
        Mnemonic::Imul => imul(cpu, instr),
        Mnemonic::Div => div(cpu, instr),
        Mnemonic::Idiv => idiv(cpu, instr),
        Mnemonic::Aaa => aaa(cpu),
        Mnemonic::Aas => aas(cpu),
        Mnemonic::Aam => aam(cpu, instr),
        Mnemonic::Das => das(cpu),
        Mnemonic::Daa => daa(cpu),
        _ => { cpu.bus.log_string(&format!("[MATH] Unsupported instruction: {:?}", instr.mnemonic())); }
    }
}

// ========================================================================
// Helpers
// ========================================================================

fn get_op0_val(cpu: &Cpu, instr: &Instruction, is_8bit: bool) -> (u16, Option<usize>) {
    if instr.op0_kind() == OpKind::Register {
        let val = if is_8bit { cpu.get_reg8(instr.op0_register()) as u16 } else { cpu.get_reg16(instr.op0_register()) };
        (val, None)
    } else {
        let addr = calculate_addr(cpu, instr);
        let val = if is_8bit { cpu.bus.read_8(addr) as u16 } else { cpu.bus.read_16(addr) };
        (val, Some(addr))
    }
}

fn get_op1_val(cpu: &Cpu, instr: &Instruction, is_8bit: bool) -> u16 {
    if instr.op1_kind() == OpKind::Register {
        if is_8bit { cpu.get_reg8(instr.op1_register()) as u16 } else { cpu.get_reg16(instr.op1_register()) }
    } else if instr.op1_kind() == OpKind::Immediate8 {
        // Sign-extend 8-bit immediate if operation is 16-bit
        if is_8bit {
            instr.immediate8() as u16
        } else {
            instr.immediate8() as i8 as i16 as u16
        }
    } else if instr.op1_kind() == OpKind::Immediate8to16 {
        instr.immediate8to16() as u16
    } else if instr.op1_kind() == OpKind::Immediate16 {
        instr.immediate16()
    } else { // Memory
        let addr = calculate_addr(cpu, instr);
        if is_8bit { cpu.bus.read_8(addr) as u16 } else { cpu.bus.read_16(addr) }
    }
}

fn write_back(cpu: &mut Cpu, instr: &Instruction, res: u16, addr: Option<usize>, is_8bit: bool) {
    if let Some(a) = addr {
        if is_8bit { cpu.bus.write_8(a, res as u8); } else { cpu.bus.write_16(a, res); }
    } else {
        let reg = instr.op0_register();
        if is_8bit { cpu.set_reg8(reg, res as u8); } else { cpu.set_reg16(reg, res); }
    }
}

// ========================================================================
// Implementations
// ========================================================================

fn add(cpu: &mut Cpu, instr: &Instruction) {
    let is_8bit = match instr.op0_kind() {
        OpKind::Register => is_8bit_reg(instr.op0_register()),
        OpKind::Memory => instr.memory_size() == MemorySize::UInt8,
        _ => false,
    };
    
    let (dest, addr) = get_op0_val(cpu, instr, is_8bit);
    let src = get_op1_val(cpu, instr, is_8bit);

    let res = if is_8bit {
        cpu.alu_add_8(dest as u8, src as u8) as u16
    } else {
        cpu.alu_add_16(dest, src)
    };
    
    write_back(cpu, instr, res, addr, is_8bit);
}

fn adc(cpu: &mut Cpu, instr: &Instruction) {
    let is_8bit = match instr.op0_kind() {
        OpKind::Register => is_8bit_reg(instr.op0_register()),
        OpKind::Memory => instr.memory_size() == MemorySize::UInt8,
        _ => false,
    };

    let (dest, addr) = get_op0_val(cpu, instr, is_8bit);
    let src = get_op1_val(cpu, instr, is_8bit);

    let res = if is_8bit {
        cpu.alu_adc_8(dest as u8, src as u8) as u16
    } else {
        cpu.alu_adc_16(dest, src)
    };

    write_back(cpu, instr, res, addr, is_8bit);
}

fn sub(cpu: &mut Cpu, instr: &Instruction) {
    let is_8bit = match instr.op0_kind() {
        OpKind::Register => is_8bit_reg(instr.op0_register()),
        OpKind::Memory => instr.memory_size() == MemorySize::UInt8,
        _ => false,
    };

    let (dest, addr) = get_op0_val(cpu, instr, is_8bit);
    let src = get_op1_val(cpu, instr, is_8bit);

    let res = if is_8bit {
        cpu.alu_sub_8(dest as u8, src as u8) as u16
    } else {
        cpu.alu_sub_16(dest, src)
    };

    write_back(cpu, instr, res, addr, is_8bit);
}

fn sbb(cpu: &mut Cpu, instr: &Instruction) {
    let is_8bit = match instr.op0_kind() {
        OpKind::Register => is_8bit_reg(instr.op0_register()),
        OpKind::Memory => instr.memory_size() == MemorySize::UInt8,
        _ => false,
    };

    let (dest, addr) = get_op0_val(cpu, instr, is_8bit);
    let src = get_op1_val(cpu, instr, is_8bit);

    let res = if is_8bit {
        cpu.alu_sbb_8(dest as u8, src as u8) as u16
    } else {
        cpu.alu_sbb_16(dest, src)
    };

    write_back(cpu, instr, res, addr, is_8bit);
}

fn cmp(cpu: &mut Cpu, instr: &Instruction) {
    let is_8bit = match instr.op0_kind() {
        OpKind::Register => is_8bit_reg(instr.op0_register()),
        OpKind::Memory => instr.memory_size() == MemorySize::UInt8,
        _ => false,
    };
    let (dest, _) = get_op0_val(cpu, instr, is_8bit);
    let src = get_op1_val(cpu, instr, is_8bit);

    if is_8bit {
        cpu.alu_sub_8(dest as u8, src as u8);
    } else {
        cpu.alu_sub_16(dest, src);
    }
}

fn inc(cpu: &mut Cpu, instr: &Instruction) {
    let is_8bit = match instr.op0_kind() {
        OpKind::Register => is_8bit_reg(instr.op0_register()),
        OpKind::Memory => instr.memory_size() == MemorySize::UInt8,
        _ => false,
    };

    let (val, addr) = get_op0_val(cpu, instr, is_8bit);
    
    let res = if is_8bit {
        let r = (val as u8).wrapping_add(1);
        cpu.set_cpu_flag(CpuFlags::ZF, r == 0);
        cpu.set_cpu_flag(CpuFlags::SF, (r & 0x80) != 0);
        cpu.set_cpu_flag(CpuFlags::OF, val == 0x7F);
        cpu.update_pf(r as u16);
        r as u16
    } else {
        let r = val.wrapping_add(1);
        cpu.set_cpu_flag(CpuFlags::ZF, r == 0);
        cpu.set_cpu_flag(CpuFlags::SF, (r & 0x8000) != 0);
        cpu.set_cpu_flag(CpuFlags::OF, val == 0x7FFF);
        cpu.update_pf(r);
        r
    };
    
    write_back(cpu, instr, res, addr, is_8bit);
}

fn dec(cpu: &mut Cpu, instr: &Instruction) {
    let is_8bit = match instr.op0_kind() {
        OpKind::Register => is_8bit_reg(instr.op0_register()),
        OpKind::Memory => instr.memory_size() == MemorySize::UInt8,
        _ => false,
    };

    let (val, addr) = get_op0_val(cpu, instr, is_8bit);

    let res = if is_8bit {
        let v = val as u8;
        let r = v.wrapping_sub(1);
        
        cpu.set_cpu_flag(CpuFlags::ZF, r == 0);
        cpu.set_cpu_flag(CpuFlags::SF, (r & 0x80) != 0);
        cpu.set_cpu_flag(CpuFlags::OF, v == 0x80);
        cpu.set_cpu_flag(CpuFlags::AF, (v & 0x0F) == 0);
        cpu.update_pf(r as u16);
        r as u16
    } else {
        let r = val.wrapping_sub(1);
        cpu.set_cpu_flag(CpuFlags::ZF, r == 0);
        cpu.set_cpu_flag(CpuFlags::SF, (r & 0x8000) != 0);
        cpu.set_cpu_flag(CpuFlags::OF, val == 0x8000);
        cpu.set_cpu_flag(CpuFlags::AF, (val & 0x0F) == 0);
        
        cpu.update_pf(r);
        r
    };
    
    write_back(cpu, instr, res, addr, is_8bit);
}

fn neg(cpu: &mut Cpu, instr: &Instruction) {
    let is_8bit = match instr.op0_kind() {
        OpKind::Register => is_8bit_reg(instr.op0_register()),
        OpKind::Memory => instr.memory_size() == MemorySize::UInt8,
        _ => false,
    };

    let (val, addr) = get_op0_val(cpu, instr, is_8bit);
    
    // Perform subtraction 0 - val
    let res = if is_8bit {
        cpu.alu_sub_8(0, val as u8) as u16
    } else {
        cpu.alu_sub_16(0, val)
    };
    
    // Fix up CF: NEG 0 clears CF, otherwise sets it.
    // alu_sub logic sets CF (Borrow) if 0 < val, which is true for all val != 0.
    // So alu_sub handles NEG CF correctly automagically
    
    write_back(cpu, instr, res, addr, is_8bit);
}

fn mul(cpu: &mut Cpu, instr: &Instruction) {
    let is_8bit = match instr.op0_kind() {
        OpKind::Register => is_8bit_reg(instr.op0_register()),
        OpKind::Memory => instr.memory_size() == MemorySize::UInt8,
        _ => false,
    };

    let src = get_op0_val(cpu, instr, is_8bit).0; 

    if is_8bit {
        let al = cpu.get_al() as u16;
        let res = al * src;
        cpu.ax = res;

        let overflow = (res & 0xFF00) != 0;
        cpu.set_cpu_flag(CpuFlags::CF, overflow);
        cpu.set_cpu_flag(CpuFlags::OF, overflow);
    } else {
        let ax = cpu.ax as u32;
        let res = ax * (src as u32);
        cpu.ax = (res & 0xFFFF) as u16;
        cpu.dx = (res >> 16) as u16;

        let overflow = (res & 0xFFFF0000) != 0;
        cpu.set_cpu_flag(CpuFlags::CF, overflow);
        cpu.set_cpu_flag(CpuFlags::OF, overflow);
    }
}

fn imul(cpu: &mut Cpu, instr: &Instruction) {
    // 1-Operand Form
    if instr.op_count() == 1 {
        let is_8bit = match instr.op0_kind() {
            OpKind::Register => is_8bit_reg(instr.op0_register()),
            OpKind::Memory => instr.memory_size() == MemorySize::UInt8,
            _ => false,
        };

        let src = get_op0_val(cpu, instr, is_8bit).0;

        if is_8bit {
            let al = cpu.get_al() as i8 as i16;
            let s = src as u8 as i8 as i16;
            let res = al * s;
            cpu.ax = res as u16;

            let fits = res == (res as i8 as i16);
            cpu.set_cpu_flag(CpuFlags::CF, !fits);
            cpu.set_cpu_flag(CpuFlags::OF, !fits);
        } else {
            let ax = cpu.ax as i16 as i32;
            let s = src as i16 as i32;
            let res = ax * s;
            cpu.ax = (res & 0xFFFF) as u16;
            cpu.dx = (res >> 16) as u16;

            let fits = res == (res as i16 as i32);
            cpu.set_cpu_flag(CpuFlags::CF, !fits);
            cpu.set_cpu_flag(CpuFlags::OF, !fits);
        }
    } 
    // Multi-Operand Forms
    else {
        let dest_reg = instr.op0_register();

        let val1 = if instr.op_count() == 2 {
            cpu.get_reg16(dest_reg) as i16 as i32
        } else {
            // Dest is op0, Src1 is op1
            get_op1_val(cpu, instr, false) as i16 as i32
        };

        let val2 = if instr.op_count() == 2 {
            get_op1_val(cpu, instr, false) as i16 as i32
        } else {
            // 3-Op: Src2 is immediate
            if instr.op2_kind() == OpKind::Immediate8 {
                instr.immediate8() as i8 as i16 as i32
            } else {
                instr.immediate16() as i16 as i32
            }
        };

        let res = val1 * val2;
        cpu.set_reg16(dest_reg, res as u16);

        let fits = res == (res as i16 as i32);
        cpu.set_cpu_flag(CpuFlags::CF, !fits);
        cpu.set_cpu_flag(CpuFlags::OF, !fits);
    }
}

fn div(cpu: &mut Cpu, instr: &Instruction) {
    let is_8bit = match instr.op0_kind() {
        OpKind::Register => is_8bit_reg(instr.op0_register()),
        OpKind::Memory => instr.memory_size() == MemorySize::UInt8,
        _ => false,
    };

    let src = get_op0_val(cpu, instr, is_8bit).0;

    if src == 0 {
        interrupts::handle_interrupt(cpu, 0x00);
        return;
    }

    if is_8bit {
        let dividend = cpu.ax;
        let divisor = src; 
        
        let quotient = dividend / divisor;
        let remainder = dividend % divisor;

        if quotient > 0xFF {
            interrupts::handle_interrupt(cpu, 0x00);
        } else {
            cpu.set_reg8(Register::AL, quotient as u8);
            cpu.set_reg8(Register::AH, remainder as u8);
        }
    } else {
        let dx = cpu.dx as u32;
        let ax = cpu.ax as u32;
        let dividend = (dx << 16) | ax;
        let divisor = src as u32;

        let quotient = dividend / divisor;
        let remainder = dividend % divisor;

        if quotient > 0xFFFF {
            interrupts::handle_interrupt(cpu, 0x00);
        } else {
            cpu.ax = quotient as u16;
            cpu.dx = remainder as u16;
        }
    }
}

fn idiv(cpu: &mut Cpu, instr: &Instruction) {
    let is_8bit = match instr.op0_kind() {
        OpKind::Register => is_8bit_reg(instr.op0_register()),
        OpKind::Memory => instr.memory_size() == MemorySize::UInt8,
        _ => false,
    };

    let src = get_op0_val(cpu, instr, is_8bit).0;

    if src == 0 {
        interrupts::handle_interrupt(cpu, 0x00);
        return;
    }

    if is_8bit {
        let dividend = cpu.ax as i16;
        let divisor = src as u8 as i8 as i16;

        if dividend == i16::MIN && divisor == -1 {
            interrupts::handle_interrupt(cpu, 0x00);
            return;
        }

        let quotient = dividend / divisor;
        let remainder = dividend % divisor;

        if quotient > 127 || quotient < -128 {
            interrupts::handle_interrupt(cpu, 0x00);
        } else {
            cpu.set_reg8(Register::AL, quotient as u8);
            cpu.set_reg8(Register::AH, remainder as u8);
        }
    } else {
        let dividend = ((cpu.dx as u32) << 16 | (cpu.ax as u32)) as i32;
        let divisor = src as i16 as i32;

        if dividend == i32::MIN && divisor == -1 {
            interrupts::handle_interrupt(cpu, 0x00);
            return;
        }

        let quotient = dividend / divisor;
        let remainder = dividend % divisor;

        if quotient > 32767 || quotient < -32768 {
            interrupts::handle_interrupt(cpu, 0x00);
        } else {
            cpu.ax = quotient as u16;
            cpu.dx = remainder as u16;
        }
    }
}

fn aaa(cpu: &mut Cpu) {
    let al = cpu.get_al();
    let af = cpu.get_cpu_flag(CpuFlags::AF);

    if (al & 0x0F) > 9 || af {
        let new_al = al.wrapping_add(6);
        cpu.set_reg8(Register::AL, new_al & 0x0F);

        let ah = cpu.get_ah();
        cpu.set_reg8(Register::AH, ah.wrapping_add(1));

        cpu.set_cpu_flag(CpuFlags::AF, true);
        cpu.set_cpu_flag(CpuFlags::CF, true);
    } else {
        cpu.set_cpu_flag(CpuFlags::AF, false);
        cpu.set_cpu_flag(CpuFlags::CF, false);
        cpu.set_reg8(Register::AL, al & 0x0F);
    }
}

// AAM: ASCII Adjust AX After Multiply
// Converts binary value in AL into two unpacked BCD digits.
pub fn aam(cpu: &mut Cpu, instr: &Instruction) {
    let mut al = cpu.get_al();
    
    // The base is usually 10 (0x0A), but encoded as an immediate.
    // If no immediate is present (rare/implicit), default to 10.
    let base = if instr.op_count() > 0 && instr.op0_kind() == OpKind::Immediate8 {
        instr.immediate8()
    } else {
        10
    };

    if base == 0 {
        // Division by zero exception (INT 0)
        crate::interrupts::handle_interrupt(cpu, 0x00);
        return;
    }

    let ah = al / base;
    al = al % base;

    cpu.set_reg8(Register::AH, ah);
    cpu.set_reg8(Register::AL, al);

    // Flags are set based on the result in AL
    cpu.set_cpu_flag(CpuFlags::SF, (al & 0x80) != 0);
    cpu.set_cpu_flag(CpuFlags::ZF, al == 0);
    cpu.update_pf(al as u16);
    // CF, OF, AF are undefined
}

fn das(cpu: &mut Cpu) {
    let mut al = cpu.get_al();
    let old_cf = cpu.get_cpu_flag(CpuFlags::CF);
    let old_af = cpu.get_cpu_flag(CpuFlags::AF);
    let mut new_cf = false;

    if (al & 0x0F) > 9 || old_af {
        al = al.wrapping_sub(6);
        cpu.set_cpu_flag(CpuFlags::AF, true);
        new_cf = old_cf || (al > 0x99); 
    } else {
        cpu.set_cpu_flag(CpuFlags::AF, false);
    }

    if al > 0x9F || old_cf {
        al = al.wrapping_sub(0x60);
        new_cf = true;
    }

    cpu.set_reg8(Register::AL, al);
    cpu.set_cpu_flag(CpuFlags::CF, new_cf);
    
    cpu.set_cpu_flag(CpuFlags::ZF, al == 0);
    cpu.set_cpu_flag(CpuFlags::SF, (al & 0x80) != 0);
    cpu.update_pf(al as u16);
}

fn daa(cpu: &mut Cpu) {
    let mut al = cpu.get_al();
    let mut cf = cpu.get_cpu_flag(CpuFlags::CF);
    let af = cpu.get_cpu_flag(CpuFlags::AF);

    // If lower nibble is invalid BCD (>9) or AF is set
    if (al & 0x0F) > 9 || af {
        al = al.wrapping_add(6);
        cpu.set_cpu_flag(CpuFlags::AF, true);
    } else {
        cpu.set_cpu_flag(CpuFlags::AF, false);
    }

    // If upper nibble is invalid BCD (>9) or CF was set
    // The check AL > 0x9F detects if the previous addition plus the correction caused an overflow
    if al > 0x9F || cf {
        al = al.wrapping_add(0x60);
        cf = true;
    }

    cpu.set_reg8(Register::AL, al);
    cpu.set_cpu_flag(CpuFlags::CF, cf);
    
    // Updates SF, ZF, PF based on result
    cpu.set_cpu_flag(CpuFlags::ZF, al == 0);
    cpu.set_cpu_flag(CpuFlags::SF, (al & 0x80) != 0);
    cpu.update_pf(al as u16);
    // OF is undefined
}

pub fn aas(cpu: &mut Cpu) {
    // If lower nibble > 9 or AF is set
    if (cpu.get_al() & 0x0F) > 9 || cpu.get_cpu_flag(CpuFlags::AF) {
        let al = cpu.get_al().wrapping_sub(6);
        cpu.set_reg8(Register::AL, al & 0x0F);
        let ah = (cpu.ax >> 8) as u8;
        cpu.ax = ((ah.wrapping_sub(1) as u16) << 8) | (cpu.get_al() as u16);
        cpu.set_cpu_flag(CpuFlags::CF, true);
        cpu.set_cpu_flag(CpuFlags::AF, true);
    } else {
        cpu.set_reg8(Register::AL, cpu.get_al() & 0x0F);
        cpu.set_cpu_flag(CpuFlags::CF, false);
        cpu.set_cpu_flag(CpuFlags::AF, false);
    }
}