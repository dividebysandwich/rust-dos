use iced_x86::{Instruction, Mnemonic, OpKind, MemorySize, Register};
use crate::cpu::Cpu;
use super::utils::calculate_addr;

pub fn handle(cpu: &mut Cpu, instr: &Instruction) {
    match instr.mnemonic() {
        Mnemonic::Fninit => {
            // Initialize FPU
            cpu.fpu_top = 0;
            // Clear stack for debug clarity
            cpu.fpu_stack = [0.0; 8];
            // TODO reset FPU status registers here.
            // cpu.fpu_status = 0;
            // cpu.fpu_control = 0x037F;
        }
        Mnemonic::Fnclex => {
            // Clear FPU Exceptions
            // cpu.fpu_status &= !0x00FF; 
        }
        Mnemonic::Fldcw => {
            // Load Control Word from Memory
            let addr = calculate_addr(cpu, instr);
            let cw = cpu.bus.read_16(addr);
            // cpu.fpu_control = cw;
            cpu.bus.log_string(&format!("[FPU] FLDCW loaded Control Word: {:04X}", cw));
        }

        // FLD: Load Floating Point Value
        Mnemonic::Fld => {
            if instr.op0_kind() == OpKind::Memory {
                let addr = calculate_addr(cpu, instr);
                let val = match instr.memory_size() {
                    MemorySize::Float32 => {
                        let bits = cpu.bus.read_32(addr);
                        f32::from_bits(bits) as f64
                    }
                    MemorySize::Float64 => {
                        let bits = cpu.bus.read_64(addr);
                        f64::from_bits(bits)
                    }
                    _ => {
                        cpu.bus.log_string("[FPU] FLD Unsupported memory size");
                        0.0
                    }
                };
                cpu.fpu_push(val);
            } else {
                // FLD ST(i) -> Push ST(i) onto stack
                // Register ST0 is 0, ST1 is 1, etc.
                let reg_offset = instr.op0_register().number() - Register::ST0.number();
                let val = cpu.fpu_get(reg_offset as usize);
                cpu.fpu_push(val);
            }
        }

        // FILD: Load Integer (Convert to Float and Push)
        Mnemonic::Fild => {
            let addr = calculate_addr(cpu, instr);
            let val = match instr.memory_size() {
                MemorySize::Int16 => (cpu.bus.read_16(addr) as i16) as f64,
                MemorySize::Int32 => (cpu.bus.read_32(addr) as i32) as f64,
                MemorySize::Int64 => (cpu.bus.read_64(addr) as i64) as f64,
                _ => 0.0,
            };
            cpu.fpu_push(val);
        }

        // FISTP: Store Integer and Pop
        Mnemonic::Fistp => {
            let val = cpu.fpu_pop();
            let addr = calculate_addr(cpu, instr);
            // Standard x87 FISTP rounds according to RC field. 
            // Check if Round to Nearest is similar enough to f64::round()
            let i_val = val.round(); 

            match instr.memory_size() {
                MemorySize::Int16 => {
                    // Saturate or wrap? x87 usually generates invalid op exception on overflow.
                    // TODO: Check if just casting is okay here.
                    cpu.bus.write_16(addr, (i_val as i16) as u16);
                }
                MemorySize::Int32 => {
                    cpu.bus.write_32(addr, (i_val as i32) as u32);
                }
                MemorySize::Int64 => {
                    // Write 64-bit int
                    let v = i_val as i64 as u64;
                    cpu.bus.write_32(addr, (v & 0xFFFFFFFF) as u32);
                    cpu.bus.write_32(addr + 4, (v >> 32) as u32);
                }
                _ => {}
            }
        }
        
        // FSTP: Store Float and Pop
        Mnemonic::Fstp => {
            let val = cpu.fpu_pop();
            if instr.op0_kind() == OpKind::Memory {
                let addr = calculate_addr(cpu, instr);
                match instr.memory_size() {
                    MemorySize::Float32 => {
                        let bits = (val as f32).to_bits();
                        cpu.bus.write_32(addr, bits);
                    }
                    MemorySize::Float64 => {
                        let bits = val.to_bits();
                        cpu.bus.write_32(addr, (bits & 0xFFFFFFFF) as u32);
                        cpu.bus.write_32(addr + 4, (bits >> 32) as u32);
                    }
                    _ => {}
                }
            }
        }

        // FDIV: Floating Point Divide
        Mnemonic::Fdiv => {
            // FDIV [mem] -> ST(0) = ST(0) / [mem]
            if instr.op0_kind() == OpKind::Memory {
                let addr = calculate_addr(cpu, instr);
                let divisor = match instr.memory_size() {
                    MemorySize::Float32 => f32::from_bits(cpu.bus.read_32(addr)) as f64,
                    MemorySize::Float64 => f64::from_bits(cpu.bus.read_64(addr)),
                    _ => 1.0,
                };
                let st0 = cpu.fpu_get(0);
                cpu.fpu_set(0, st0 / divisor);
            } 
            // FDIV ST(i), ST(0)  -> ST(i) = ST(i) / ST(0)
            else if instr.op0_register() != Register::ST0 && instr.op1_register() == Register::ST0 {
                let idx = instr.op0_register().number() - Register::ST0.number();
                let sti = cpu.fpu_get(idx as usize);
                let st0 = cpu.fpu_get(0);
                cpu.fpu_set(idx as usize, sti / st0);
            }
            // FDIV ST(0), ST(i) -> ST(0) = ST(0) / ST(i)
            else {
                let idx = instr.op1_register().number() - Register::ST0.number();
                let sti = cpu.fpu_get(idx as usize);
                let st0 = cpu.fpu_get(0);
                cpu.fpu_set(0, st0 / sti);
            }
        }

        // FSUBP: Subtract and Pop
        // FSUBP ST(1), ST(0) -> ST(1) = ST(1) - ST(0); Pop ST(0)
        Mnemonic::Fsubp => {
            let st0 = cpu.fpu_get(0); // Source
            let st1 = cpu.fpu_get(1); // Destination
            
            cpu.fpu_set(1, st1 - st0); // Math is hard
            cpu.fpu_pop(); // Pop ST0, so the result (in old ST1) becomes the new Top
        }

        _ => {
            cpu.bus.log_string(&format!("[FPU] Unhandled instruction: {:?}", instr.mnemonic()));
        }
    }
}