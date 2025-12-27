use iced_x86::{Instruction, Mnemonic, Code, OpKind};
use crate::cpu::{Cpu, CpuFlags};
use super::utils::calculate_addr;

pub fn handle(cpu: &mut Cpu, instr: &Instruction) {
    match instr.mnemonic() {
        // Unconditional Transfers
        Mnemonic::Jmp => jmp(cpu, instr),
        Mnemonic::Call => call(cpu, instr),
        Mnemonic::Ret => ret(cpu, instr),
        Mnemonic::Retf => retf(cpu, instr),

        // Loops
        Mnemonic::Loop => loop_op(cpu, instr),
        Mnemonic::Loope => loope(cpu, instr),
        Mnemonic::Loopne => loopne(cpu, instr),
        Mnemonic::Jcxz | Mnemonic::Jecxz => jcxz(cpu, instr),

        // Conditional Jumps
        Mnemonic::Je => if cpu.get_cpu_flag(CpuFlags::ZF) { branch(cpu, instr) },
        Mnemonic::Jne => if !cpu.get_cpu_flag(CpuFlags::ZF) { branch(cpu, instr) },
        
        Mnemonic::Jb => if cpu.get_cpu_flag(CpuFlags::CF) { branch(cpu, instr) },
        Mnemonic::Jbe => if cpu.get_cpu_flag(CpuFlags::CF) || cpu.get_cpu_flag(CpuFlags::ZF) { branch(cpu, instr) },
        Mnemonic::Ja => if !cpu.get_cpu_flag(CpuFlags::CF) && !cpu.get_cpu_flag(CpuFlags::ZF) { branch(cpu, instr) },
        Mnemonic::Jae => if !cpu.get_cpu_flag(CpuFlags::CF) { branch(cpu, instr) },

        Mnemonic::Jl => if cpu.get_cpu_flag(CpuFlags::SF) != cpu.get_cpu_flag(CpuFlags::OF) { branch(cpu, instr) },
        Mnemonic::Jle => if cpu.get_cpu_flag(CpuFlags::ZF) || (cpu.get_cpu_flag(CpuFlags::SF) != cpu.get_cpu_flag(CpuFlags::OF)) { branch(cpu, instr) },
        Mnemonic::Jg => if !cpu.get_cpu_flag(CpuFlags::ZF) && (cpu.get_cpu_flag(CpuFlags::SF) == cpu.get_cpu_flag(CpuFlags::OF)) { branch(cpu, instr) },
        Mnemonic::Jge => if cpu.get_cpu_flag(CpuFlags::SF) == cpu.get_cpu_flag(CpuFlags::OF) { branch(cpu, instr) },

        Mnemonic::Js => if cpu.get_cpu_flag(CpuFlags::SF) { branch(cpu, instr) },
        Mnemonic::Jns => if !cpu.get_cpu_flag(CpuFlags::SF) { branch(cpu, instr) },
        Mnemonic::Jo => if cpu.get_cpu_flag(CpuFlags::OF) { branch(cpu, instr) },
        Mnemonic::Jno => if !cpu.get_cpu_flag(CpuFlags::OF) { branch(cpu, instr) },
        Mnemonic::Jp => if cpu.get_cpu_flag(CpuFlags::PF) { branch(cpu, instr) },
        Mnemonic::Jnp => if !cpu.get_cpu_flag(CpuFlags::PF) { branch(cpu, instr) },

        _ => { cpu.bus.log_string(&format!("[CONTROL] Unsupported instruction: {:?}", instr.mnemonic()));}
    }
}

fn branch(cpu: &mut Cpu, instr: &Instruction) {
    cpu.ip = instr.near_branch16() as u16;
}

fn jmp(cpu: &mut Cpu, instr: &Instruction) {
    match instr.code() {
        // JMP Rel (Short/Near)
        Code::Jmp_rel8_16 | Code::Jmp_rel16 | Code::Jmp_rel8_32 | Code::Jmp_rel32_32 => {
            cpu.ip = instr.near_branch16() as u16;
        }

        // JMP r/m16 (Near Indirect)
        Code::Jmp_rm16 => {
            if instr.op0_kind() == OpKind::Register {
                cpu.ip = cpu.get_reg16(instr.op0_register());
            } else {
                let addr = calculate_addr(cpu, instr);
                cpu.ip = cpu.bus.read_16(addr);
            }
        }

        // JMP ptr16:16 (Far Direct) -> JMP SEG:OFF
        // iced_x86: far_branch16 = Segment, near_branch16 = Offset
        Code::Jmp_ptr1616 => {
            cpu.ip = instr.far_branch16();
            cpu.cs = instr.near_branch16() as u16;
        }

        // JMP m16:16 (Far Indirect) -> JMP DWORD PTR [BX]
        Code::Jmp_m1616 => {
            let addr = calculate_addr(cpu, instr);
            let new_ip = cpu.bus.read_16(addr);
            let new_cs = cpu.bus.read_16(addr + 2);
            cpu.ip = new_ip;
            cpu.cs = new_cs;
        }
        _ => {cpu.bus.log_string(&format!("[CONTROL] Unsupported JMP instruction: {:?}", instr.code())); }
    }
}

fn call(cpu: &mut Cpu, instr: &Instruction) {
    match instr.code() {
        Code::Call_rel16 | Code::Call_rel32_32 => {
            cpu.push(cpu.ip);
            cpu.ip = instr.near_branch16() as u16;
        }
        Code::Call_rm16 => {
            cpu.push(cpu.ip);
            if instr.op0_kind() == OpKind::Register {
                cpu.ip = cpu.get_reg16(instr.op0_register());
            } else {
                let addr = calculate_addr(cpu, instr);
                cpu.ip = cpu.bus.read_16(addr);
            }
        }
        Code::Call_ptr1616 => {
            cpu.push(cpu.cs);
            cpu.push(cpu.ip);
            cpu.ip = instr.far_branch16();
            cpu.cs = instr.near_branch16() as u16;
        }
        Code::Call_m1616 => {
            let addr = calculate_addr(cpu, instr);
            let new_ip = cpu.bus.read_16(addr);
            let new_cs = cpu.bus.read_16(addr + 2);
            cpu.push(cpu.cs);
            cpu.push(cpu.ip);
            cpu.ip = new_ip;
            cpu.cs = new_cs;
        }
        _ => {cpu.bus.log_string(&format!("[CONTROL] Unsupported CALL instruction: {:?}", instr.code()));}
    }
}

fn ret(cpu: &mut Cpu, instr: &Instruction) {
    cpu.ip = cpu.pop();
    if instr.op0_kind() == OpKind::Immediate16 {
        cpu.sp = cpu.sp.wrapping_add(instr.immediate16());
    }
}

fn retf(cpu: &mut Cpu, instr: &Instruction) {
    cpu.ip = cpu.pop();
    cpu.cs = cpu.pop();
    if instr.op0_kind() == OpKind::Immediate16 {
        cpu.sp = cpu.sp.wrapping_add(instr.immediate16());
    }
}

fn loop_op(cpu: &mut Cpu, instr: &Instruction) {
    cpu.cx = cpu.cx.wrapping_sub(1);
    if cpu.cx != 0 {
        cpu.ip = instr.near_branch16() as u16;
    }
}

fn loope(cpu: &mut Cpu, instr: &Instruction) {
    cpu.cx = cpu.cx.wrapping_sub(1);
    if cpu.cx != 0 && cpu.get_cpu_flag(CpuFlags::ZF) {
        cpu.ip = instr.near_branch16() as u16;
    }
}

fn loopne(cpu: &mut Cpu, instr: &Instruction) {
    cpu.cx = cpu.cx.wrapping_sub(1);
    if cpu.cx != 0 && !cpu.get_cpu_flag(CpuFlags::ZF) {
        cpu.ip = instr.near_branch16() as u16;
    }
}

fn jcxz(cpu: &mut Cpu, instr: &Instruction) {
    if cpu.cx == 0 {
        cpu.ip = instr.near_branch16() as u16;
    }
}
