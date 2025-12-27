use iced_x86::{Instruction, Mnemonic, OpKind, Register, MemorySize};
use crate::cpu::{Cpu, CpuFlags};
use super::utils::{calculate_addr, get_effective_addr, is_8bit_reg};

pub fn handle(cpu: &mut Cpu, instr: &Instruction) {
    match instr.mnemonic() {
        Mnemonic::Mov => mov(cpu, instr),
        Mnemonic::Xchg => xchg(cpu, instr),
        
        // Stack Operations
        Mnemonic::Push => push(cpu, instr),
        Mnemonic::Pop => pop(cpu, instr),
        Mnemonic::Pusha => pusha(cpu),
        Mnemonic::Popa => popa(cpu),
        Mnemonic::Pushf => pushf(cpu),
        Mnemonic::Popf => popf(cpu),

        // Address Loading
        Mnemonic::Lea => lea(cpu, instr),
        Mnemonic::Lds => lds(cpu, instr),
        Mnemonic::Les => les(cpu, instr),

        // I/O Ports
        Mnemonic::In => port_in(cpu, instr),
        Mnemonic::Out => port_out(cpu, instr),

        // Conversion
        Mnemonic::Cbw => cbw(cpu),
        Mnemonic::Cwd => cwd(cpu),

        Mnemonic::Xlatb => xlatb(cpu),
        Mnemonic::Lahf => lahf(cpu),
        Mnemonic::Sahf => sahf(cpu),
        
        _ => { cpu.bus.log_string(&format!("[TRANSFER] Unsupported instruction: {:?}", instr.mnemonic())); }
    }
}

fn mov(cpu: &mut Cpu, instr: &Instruction) {
    // MOV [Mem], ...
    if instr.op0_kind() == OpKind::Memory {
        let addr = calculate_addr(cpu, instr);
        
        // Determine Source Value
        let val = if instr.op1_kind() == OpKind::Register {
            let reg = instr.op1_register();
            if is_8bit_reg(reg) {
                cpu.get_reg8(reg) as u16
            } else {
                cpu.get_reg16(reg)
            }
        } else if instr.op1_kind() == OpKind::Immediate8 {
            instr.immediate8() as u16
        } else if instr.op1_kind() == OpKind::Immediate16 {
            instr.immediate16()
        } else {
            0
        };

        // Strict Size Determination
        let is_8bit_dest = if instr.op1_kind() == OpKind::Register {
            is_8bit_reg(instr.op1_register())
        } else {
            // If immediate, trust the memory size hint from the instruction
            instr.memory_size() == MemorySize::UInt8
        };

        if is_8bit_dest {
            cpu.bus.write_8(addr, val as u8);
        } else {
            cpu.bus.write_16(addr, val);
        }
    } 
    // MOV Reg, ...
    else if instr.op0_kind() == OpKind::Register {
        let dest_reg = instr.op0_register();

        let val = if instr.op1_kind() == OpKind::Register {
            if is_8bit_reg(dest_reg) {
                cpu.get_reg8(instr.op1_register()) as u16
            } else {
                cpu.get_reg16(instr.op1_register())
            }
        } else if instr.op1_kind() == OpKind::Memory {
            let addr = calculate_addr(cpu, instr);
            if is_8bit_reg(dest_reg) {
                cpu.bus.read_8(addr) as u16
            } else {
                cpu.bus.read_16(addr)
            }
        } else if instr.op1_kind() == OpKind::Immediate8 {
            instr.immediate8() as u16
        } else if instr.op1_kind() == OpKind::Immediate16 {
            instr.immediate16()
        } else if instr.op1_kind() == OpKind::Immediate8to16 {
            instr.immediate8to16() as u16
        } else {
            0
        };

        if is_8bit_reg(dest_reg) {
            cpu.set_reg8(dest_reg, val as u8);
        } else {
            cpu.set_reg16(dest_reg, val);
        }
    }
    // MOV Segment, ... (e.g., MOV DS, AX)
    else if instr.op0_register().is_segment_register() {
        let dest_reg = instr.op0_register();
        let val = if instr.op1_kind() == OpKind::Register {
            cpu.get_reg16(instr.op1_register())
        } else if instr.op1_kind() == OpKind::Memory {
            let addr = calculate_addr(cpu, instr);
            cpu.bus.read_16(addr)
        } else {
            0
        };
        cpu.set_reg16(dest_reg, val);
    }
}

fn xchg(cpu: &mut Cpu, instr: &Instruction) {
    let op0 = instr.op0_kind();
    let op1 = instr.op1_kind();

    let is_8bit = if op0 == OpKind::Register {
        is_8bit_reg(instr.op0_register())
    } else if op1 == OpKind::Register {
        is_8bit_reg(instr.op1_register())
    } else {
        instr.memory_size() == MemorySize::UInt8
    };

    // Read Operand 0
    let (val0, addr0) = if op0 == OpKind::Register {
        let reg = instr.op0_register();
        if is_8bit { (cpu.get_reg8(reg) as u16, None) } else { (cpu.get_reg16(reg), None) }
    } else {
        let addr = calculate_addr(cpu, instr);
        if is_8bit { (cpu.bus.read_8(addr) as u16, Some(addr)) } else { (cpu.bus.read_16(addr), Some(addr)) }
    };

    // Read Operand 1
    let (val1, addr1) = if op1 == OpKind::Register {
        let reg = instr.op1_register();
        if is_8bit { (cpu.get_reg8(reg) as u16, None) } else { (cpu.get_reg16(reg), None) }
    } else {
        let addr = calculate_addr(cpu, instr);
        if is_8bit { (cpu.bus.read_8(addr) as u16, Some(addr)) } else { (cpu.bus.read_16(addr), Some(addr)) }
    };

    // Write Value 1 to Operand 0 location
    if let Some(addr) = addr0 {
        if is_8bit { cpu.bus.write_8(addr, val1 as u8); } else { cpu.bus.write_16(addr, val1); }
    } else {
        let reg = instr.op0_register();
        if is_8bit { cpu.set_reg8(reg, val1 as u8); } else { cpu.set_reg16(reg, val1); }
    }

    // Write Value 0 to Operand 1 location
    if let Some(addr) = addr1 {
        if is_8bit { cpu.bus.write_8(addr, val0 as u8); } else { cpu.bus.write_16(addr, val0); }
    } else {
        let reg = instr.op1_register();
        if is_8bit { cpu.set_reg8(reg, val0 as u8); } else { cpu.set_reg16(reg, val0); }
    }
}

fn push(cpu: &mut Cpu, instr: &Instruction) {
    let val = if instr.op0_kind() == OpKind::Register {
        cpu.get_reg16(instr.op0_register())
    } else if instr.op0_kind() == OpKind::Immediate8 {
        instr.immediate8() as i8 as i16 as u16
    } else if instr.op0_kind() == OpKind::Immediate16 {
        instr.immediate16()
    } else if instr.op0_kind() == OpKind::Memory {
        let addr = calculate_addr(cpu, instr);
        cpu.bus.read_16(addr)
    } else {
        0
    };
    cpu.push(val);
}

fn pop(cpu: &mut Cpu, instr: &Instruction) {
    let val = cpu.pop();
    if instr.op0_kind() == OpKind::Register {
        cpu.set_reg16(instr.op0_register(), val);
    } else if instr.op0_kind() == OpKind::Memory {
        let addr = calculate_addr(cpu, instr);
        cpu.bus.write_16(addr, val);
    }
}

fn pusha(cpu: &mut Cpu) {
    let sp = cpu.get_reg16(Register::SP);
    cpu.push(cpu.get_reg16(Register::AX));
    cpu.push(cpu.get_reg16(Register::CX));
    cpu.push(cpu.get_reg16(Register::DX));
    cpu.push(cpu.get_reg16(Register::BX));
    cpu.push(sp);
    cpu.push(cpu.get_reg16(Register::BP));
    cpu.push(cpu.get_reg16(Register::SI));
    cpu.push(cpu.get_reg16(Register::DI));
}

fn popa(cpu: &mut Cpu) {
    let di = cpu.pop();
    let si = cpu.pop();
    let bp = cpu.pop();
    let _sp = cpu.pop(); // Pop and discard SP
    let bx = cpu.pop();
    let dx = cpu.pop();
    let cx = cpu.pop();
    let ax = cpu.pop();

    cpu.di = di;
    cpu.si = si;
    cpu.set_reg16(Register::BP, bp);
    cpu.set_reg16(Register::BX, bx);
    cpu.dx = dx;
    cpu.cx = cx;
    cpu.ax = ax;
}

fn pushf(cpu: &mut Cpu) {
    cpu.push(cpu.flags.bits());
}

fn popf(cpu: &mut Cpu) {
    let val = cpu.pop();
    cpu.flags = CpuFlags::from_bits_truncate((val & 0x0FD5) | 0x0002);
}

fn lea(cpu: &mut Cpu, instr: &Instruction) {
    let reg = instr.op0_register();
    let offset = get_effective_addr(cpu, instr);
    cpu.set_reg16(reg, offset);
}

fn lds(cpu: &mut Cpu, instr: &Instruction) {
    let reg = instr.op0_register();
    let addr = calculate_addr(cpu, instr);
    let offset = cpu.bus.read_16(addr);
    let segment = cpu.bus.read_16(addr + 2);
    cpu.set_reg16(reg, offset);
    cpu.ds = segment;
}

fn les(cpu: &mut Cpu, instr: &Instruction) {
    let reg = instr.op0_register();
    let addr = calculate_addr(cpu, instr);
    let offset = cpu.bus.read_16(addr);
    let segment = cpu.bus.read_16(addr + 2);
    cpu.set_reg16(reg, offset);
    cpu.es = segment;
}

fn port_in(cpu: &mut Cpu, instr: &Instruction) {
    let port = if instr.op1_kind() == OpKind::Register {
        cpu.dx
    } else {
        instr.immediate8() as u16
    };
    let val = cpu.bus.io_read(port);
    if is_8bit_reg(instr.op0_register()) {
        cpu.set_reg8(instr.op0_register(), val);
    } else {
        cpu.set_reg16(instr.op0_register(), val as u16);
    }
}

fn port_out(cpu: &mut Cpu, instr: &Instruction) {
    let port = if instr.op0_kind() == OpKind::Register {
        cpu.dx
    } else {
        instr.immediate8() as u16
    };
    let val = if is_8bit_reg(instr.op1_register()) {
        cpu.get_reg8(instr.op1_register())
    } else {
        cpu.get_al() 
    };
    cpu.bus.io_write(port, val);
}

fn cbw(cpu: &mut Cpu) {
    let al = cpu.get_al() as i8;
    cpu.ax = al as i16 as u16;
}

fn cwd(cpu: &mut Cpu) {
    let ax = cpu.ax as i16;
    cpu.dx = if ax < 0 { 0xFFFF } else { 0x0000 };
}

fn xlatb(cpu: &mut Cpu) {
    // AL = Byte at [DS:BX + AL]
    let bx = cpu.bx;
    let al = cpu.get_al() as u16;
    let offset = bx.wrapping_add(al);
    let addr = cpu.get_physical_addr(cpu.ds, offset);
    
    let val = cpu.bus.read_8(addr);
    cpu.set_reg8(iced_x86::Register::AL, val);
}

// LAHF: Load Flags into AH
// Transfers SF, ZF, AF, PF, CF into bits 7, 6, 4, 2, 0 of AH.
// Bits 1, 3, 5 are reserved (usually 1, 0, 0 or similar, but 1,0,1 is common 8086).
pub fn lahf(cpu: &mut Cpu) {
    let mut ah: u8 = 0;
    if cpu.get_cpu_flag(CpuFlags::SF) { ah |= 0x80; }
    if cpu.get_cpu_flag(CpuFlags::ZF) { ah |= 0x40; }
    if cpu.get_cpu_flag(CpuFlags::AF) { ah |= 0x10; }
    if cpu.get_cpu_flag(CpuFlags::PF) { ah |= 0x04; }
    if cpu.get_cpu_flag(CpuFlags::CF) { ah |= 0x01; }
    
    // Set reserved bits (Bit 1=1 is standard for 8086)
    ah |= 0x02; 
    
    cpu.set_reg8(Register::AH, ah);
}

// SAHF: Store AH into Flags
// Transfers bits 7, 6, 4, 2, 0 of AH to SF, ZF, AF, PF, CF respectively.
pub fn sahf(cpu: &mut Cpu) {
    let ah = cpu.get_ah();
    cpu.set_cpu_flag(CpuFlags::SF, (ah & 0x80) != 0);
    cpu.set_cpu_flag(CpuFlags::ZF, (ah & 0x40) != 0);
    cpu.set_cpu_flag(CpuFlags::AF, (ah & 0x10) != 0);
    cpu.set_cpu_flag(CpuFlags::PF, (ah & 0x04) != 0);
    cpu.set_cpu_flag(CpuFlags::CF, (ah & 0x01) != 0);
}