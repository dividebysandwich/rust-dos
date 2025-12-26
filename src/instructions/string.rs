use iced_x86::{Instruction, Mnemonic, Register};
use crate::cpu::{Cpu, FLAG_ZF};

pub fn handle(cpu: &mut Cpu, instr: &Instruction) {
    match instr.mnemonic() {
        // Move String Data (DS:SI -> ES:DI)
        Mnemonic::Movsb => movs(cpu, instr, 1),
        Mnemonic::Movsw => movs(cpu, instr, 2),

        // Store String Data (AL/AX -> ES:DI)
        Mnemonic::Stosb => stos(cpu, instr, 1),
        Mnemonic::Stosw => stos(cpu, instr, 2),

        // Load String Data (DS:SI -> AL/AX)
        Mnemonic::Lodsb => lods(cpu, instr, 1),
        Mnemonic::Lodsw => lods(cpu, instr, 2),

        // Compare String Data (DS:SI - ES:DI)
        Mnemonic::Cmpsb => cmps(cpu, instr, 1),
        Mnemonic::Cmpsw => cmps(cpu, instr, 2),

        // Scan String Data (AL/AX - ES:DI)
        Mnemonic::Scasb => scas(cpu, instr, 1),
        Mnemonic::Scasw => scas(cpu, instr, 2),

        _ => {}
    }
}

// Helper: Get source segment for string instructions (DS is default, but can be overridden)
fn get_string_src_segment(instr: &Instruction, cpu: &Cpu) -> u16 {
    match instr.segment_prefix() {
        Register::CS => cpu.cs,
        Register::ES => cpu.es,
        Register::SS => cpu.ss,
        Register::DS => cpu.ds,
        _ => cpu.ds, // Default to DS if no prefix
    }
}

fn movs(cpu: &mut Cpu, instr: &Instruction, size: u16) {
    let has_rep = instr.has_rep_prefix();

    loop {
        // Check Loop Condition (REP checks CX=0 before execution)
        if has_rep && cpu.cx == 0 { break; }

        // Calculate Addresses
        // Source: DS:SI (overridable)
        let src_seg = get_string_src_segment(instr, cpu);
        let src_addr = cpu.get_physical_addr(src_seg, cpu.si);
        
        // Destination: ES:DI (Always ES, cannot be overridden)
        let dest_addr = cpu.get_physical_addr(cpu.es, cpu.di);

        // Perform Copy
        if size == 1 {
            let val = cpu.bus.read_8(src_addr);
            cpu.bus.write_8(dest_addr, val);
        } else {
            let val = cpu.bus.read_16(src_addr);
            cpu.bus.write_16(dest_addr, val);
        }

        // Update Indices (SI/DI)
        // If Direction Flag (DF) is set, decrement. Else increment.
        let delta = if cpu.dflag() { 
            // Two's complement negation for u16 wrapping
            (0u16).wrapping_sub(size) 
        } else { 
            size 
        };
        
        cpu.si = cpu.si.wrapping_add(delta);
        cpu.di = cpu.di.wrapping_add(delta);

        // Handle Repetition
        if has_rep {
            cpu.cx = cpu.cx.wrapping_sub(1);
        } else {
            break; // No REP prefix, run once
        }
    }
}

fn stos(cpu: &mut Cpu, instr: &Instruction, size: u16) {
    let has_rep = instr.has_rep_prefix();

    loop {
        if has_rep && cpu.cx == 0 { break; }

        // Destination: ES:DI
        let dest_addr = cpu.get_physical_addr(cpu.es, cpu.di);

        // Write Value (AL or AX)
        if size == 1 {
            cpu.bus.write_8(dest_addr, cpu.get_al());
        } else {
            cpu.bus.write_16(dest_addr, cpu.ax);
        }

        let delta = if cpu.dflag() { (0u16).wrapping_sub(size) } else { size };
        cpu.di = cpu.di.wrapping_add(delta);

        if has_rep {
            cpu.cx = cpu.cx.wrapping_sub(1);
        } else {
            break;
        }
    }
}

fn lods(cpu: &mut Cpu, instr: &Instruction, size: u16) {
    let has_rep = instr.has_rep_prefix();

    loop {
        if has_rep && cpu.cx == 0 { break; }

        let src_seg = get_string_src_segment(instr, cpu);
        let src_addr = cpu.get_physical_addr(src_seg, cpu.si);

        if size == 1 {
            let val = cpu.bus.read_8(src_addr);
            cpu.set_reg8(Register::AL, val);
        } else {
            let val = cpu.bus.read_16(src_addr);
            cpu.ax = val;
        }

        let delta = if cpu.dflag() { (0u16).wrapping_sub(size) } else { size };
        cpu.si = cpu.si.wrapping_add(delta);

        if has_rep {
            cpu.cx = cpu.cx.wrapping_sub(1);
        } else {
            break;
        }
    }
}

fn cmps(cpu: &mut Cpu, instr: &Instruction, size: u16) {
    let has_repe = instr.has_rep_prefix() || instr.has_repe_prefix();
    let has_repne = instr.has_repne_prefix();
    let is_repeated = has_repe || has_repne;

    loop {
        if is_repeated && cpu.cx == 0 { break; }

        let src_seg = get_string_src_segment(instr, cpu);
        let src_addr = cpu.get_physical_addr(src_seg, cpu.si);
        let dest_addr = cpu.get_physical_addr(cpu.es, cpu.di);

        // Perform Subtraction (Src - Dest)
        // This updates flags (ZF, SF, OF, CF, etc.)
        if size == 1 {
            let val1 = cpu.bus.read_8(src_addr);
            let val2 = cpu.bus.read_8(dest_addr);
            cpu.alu_sub_8(val1, val2);
        } else {
            let val1 = cpu.bus.read_16(src_addr);
            let val2 = cpu.bus.read_16(dest_addr);
            cpu.alu_sub_16(val1, val2);
        }

        let delta = if cpu.dflag() { (0u16).wrapping_sub(size) } else { size };
        cpu.si = cpu.si.wrapping_add(delta);
        cpu.di = cpu.di.wrapping_add(delta);

        if is_repeated {
            cpu.cx = cpu.cx.wrapping_sub(1);

            // REPE (Loop while Equal/ZF=1): Break if ZF=0
            if has_repe && !cpu.get_flag(FLAG_ZF) { break; }
            
            // REPNE (Loop while NotEqual/ZF=0): Break if ZF=1
            if has_repne && cpu.get_flag(FLAG_ZF) { break; }
        } else {
            break;
        }
    }
}

fn scas(cpu: &mut Cpu, instr: &Instruction, size: u16) {
    let has_repe = instr.has_rep_prefix() || instr.has_repe_prefix();
    let has_repne = instr.has_repne_prefix();
    let is_repeated = has_repe || has_repne;

    loop {
        if is_repeated && cpu.cx == 0 { break; }

        let dest_addr = cpu.get_physical_addr(cpu.es, cpu.di);

        // Compare Accumulator - Memory
        if size == 1 {
            let val_acc = cpu.get_al();
            let val_mem = cpu.bus.read_8(dest_addr);
            cpu.alu_sub_8(val_acc, val_mem);
        } else {
            let val_acc = cpu.ax;
            let val_mem = cpu.bus.read_16(dest_addr);
            cpu.alu_sub_16(val_acc, val_mem);
        }

        let delta = if cpu.dflag() { (0u16).wrapping_sub(size) } else { size };
        cpu.di = cpu.di.wrapping_add(delta);

        if is_repeated {
            cpu.cx = cpu.cx.wrapping_sub(1);
            if has_repe && !cpu.get_flag(FLAG_ZF) { break; }
            if has_repne && cpu.get_flag(FLAG_ZF) { break; }
        } else {
            break;
        }
    }
}