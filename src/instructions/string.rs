use crate::cpu::{Cpu, CpuFlags};
use iced_x86::{Instruction, Mnemonic, Register};

pub fn handle(cpu: &mut Cpu, instr: &Instruction) {
    let has_rep = instr.has_rep_prefix();
    let has_repne = instr.has_repne_prefix();

    // Non-REP: Execute once and return
    if !has_rep && !has_repne {
        execute_once(cpu, instr);
        return;
    }

    while cpu.cx != 0 {
        // Execute the instruction (Updates DI/SI and Flags)
        execute_once(cpu, instr);

        // Decrement CX
        cpu.cx = cpu.cx.wrapping_sub(1);

        // Check termination based on Flags (ZF)
        let zf = cpu.get_cpu_flag(CpuFlags::ZF);
        match instr.mnemonic() {
            Mnemonic::Cmpsb | Mnemonic::Cmpsw | Mnemonic::Scasb | Mnemonic::Scasw => {
                // REPE/REPZ (F3): Loop while Equal (ZF=1). Stop if Not Equal (ZF=0).
                if has_rep && !zf {
                    break;
                }
                // REPNE/REPNZ (F2): Loop while Not Equal (ZF=0). Stop if Equal (ZF=1).
                if has_repne && zf {
                    break;
                }
            }
            _ => {} // MOVS, STOS, LODS do not check flags for termination
        }
    }
}

fn execute_once(cpu: &mut Cpu, instr: &Instruction) {
    match instr.mnemonic() {
        Mnemonic::Movsb => movs(cpu, instr, 1),
        Mnemonic::Movsw => movs(cpu, instr, 2),
        Mnemonic::Stosb => stos(cpu, instr, 1),
        Mnemonic::Stosw => stos(cpu, instr, 2),
        Mnemonic::Lodsb => lods(cpu, instr, 1),
        Mnemonic::Lodsw => lods(cpu, instr, 2),
        Mnemonic::Cmpsb => cmps(cpu, instr, 1),
        Mnemonic::Cmpsw => cmps(cpu, instr, 2),
        Mnemonic::Scasb => scas(cpu, instr, 1),
        Mnemonic::Scasw => scas(cpu, instr, 2),
        Mnemonic::Outsb => outs(cpu, instr, 1),
        Mnemonic::Outsw => outs(cpu, instr, 2),
        Mnemonic::Insb => ins(cpu, instr, 1),
        Mnemonic::Insw => ins(cpu, instr, 2),
        _ => {
            cpu.bus.log_string(&format!(
                "[STRING] Unsupported instruction: {:?}",
                instr.mnemonic()
            ));
        }
    }
}

fn get_string_src_segment(instr: &Instruction, cpu: &Cpu) -> u16 {
    match instr.segment_prefix() {
        Register::CS => cpu.cs,
        Register::ES => cpu.es,
        Register::SS => cpu.ss,
        Register::DS => cpu.ds,
        _ => cpu.ds,
    }
}

fn update_indices(cpu: &mut Cpu, size: u16, update_si: bool, update_di: bool) {
    let delta = if cpu.dflag() {
        (0u16).wrapping_sub(size)
    } else {
        size
    };

    if update_si {
        cpu.si = cpu.si.wrapping_add(delta);
    }
    if update_di {
        cpu.di = cpu.di.wrapping_add(delta);
    }
}

fn movs(cpu: &mut Cpu, instr: &Instruction, size: u16) {
    let src_seg = get_string_src_segment(instr, cpu);
    let src_addr = cpu.get_physical_addr(src_seg, cpu.si);
    let dst_addr = cpu.get_physical_addr(cpu.es, cpu.di);

    if size == 1 {
        let val = cpu.bus.read_8(src_addr);
        cpu.bus.write_8(dst_addr, val);
    } else {
        let val = cpu.bus.read_16(src_addr);
        cpu.bus.write_16(dst_addr, val);
    }

    update_indices(cpu, size, true, true);
}

fn stos(cpu: &mut Cpu, _instr: &Instruction, size: u16) {
    let dst_addr = cpu.get_physical_addr(cpu.es, cpu.di);

    if size == 1 {
        cpu.bus.write_8(dst_addr, cpu.get_al());
    } else {
        cpu.bus.write_16(dst_addr, cpu.ax);
    }

    update_indices(cpu, size, false, true);
}

fn lods(cpu: &mut Cpu, instr: &Instruction, size: u16) {
    let src_seg = get_string_src_segment(instr, cpu);
    let src_addr = cpu.get_physical_addr(src_seg, cpu.si);

    if size == 1 {
        let val = cpu.bus.read_8(src_addr);
        cpu.set_reg8(Register::AL, val);
    } else {
        let val = cpu.bus.read_16(src_addr);
        cpu.ax = val;
    }

    update_indices(cpu, size, true, false);
}

fn cmps(cpu: &mut Cpu, instr: &Instruction, size: u16) {
    let src_seg = get_string_src_segment(instr, cpu);
    let src_addr = cpu.get_physical_addr(src_seg, cpu.si);
    let dst_addr = cpu.get_physical_addr(cpu.es, cpu.di);

    if size == 1 {
        let a = cpu.bus.read_8(src_addr);
        let b = cpu.bus.read_8(dst_addr);
        cpu.alu_sub_8(a, b);
    } else {
        let a = cpu.bus.read_16(src_addr);
        let b = cpu.bus.read_16(dst_addr);
        cpu.alu_sub_16(a, b);
    }

    update_indices(cpu, size, true, true);
}

fn scas(cpu: &mut Cpu, _instr: &Instruction, size: u16) {
    let dst_addr = cpu.get_physical_addr(cpu.es, cpu.di);

    if size == 1 {
        let acc = cpu.get_al();
        let mem = cpu.bus.read_8(dst_addr);
        cpu.alu_sub_8(acc, mem);
    } else {
        let acc = cpu.ax;
        let mem = cpu.bus.read_16(dst_addr);
        cpu.alu_sub_16(acc, mem);
    }

    update_indices(cpu, size, false, true);
}

fn outs(cpu: &mut Cpu, instr: &Instruction, size: u16) {
    let src_seg = get_string_src_segment(instr, cpu);
    let src_addr = cpu.get_physical_addr(src_seg, cpu.si);
    let port = cpu.dx;

    if size == 1 {
        let val = cpu.bus.read_8(src_addr);
        cpu.bus.io_write(port, val);
    } else {
        let val = cpu.bus.read_16(src_addr);
        // 16-bit I/O: Low byte to Port, High byte to Port+1
        cpu.bus.io_write(port, (val & 0xFF) as u8);
        cpu.bus.io_write(port.wrapping_add(1), (val >> 8) as u8);
    }

    update_indices(cpu, size, true, false);
}

fn ins(cpu: &mut Cpu, _instr: &Instruction, size: u16) {
    let dst_addr = cpu.get_physical_addr(cpu.es, cpu.di);
    let port = cpu.dx;

    if size == 1 {
        let val = cpu.bus.io_read(port);
        cpu.bus.write_8(dst_addr, val);
    } else {
        let low = cpu.bus.io_read(port);
        let high = cpu.bus.io_read(port.wrapping_add(1));
        let val = (low as u16) | ((high as u16) << 8);
        cpu.bus.write_16(dst_addr, val);
    }

    update_indices(cpu, size, false, true);
}
