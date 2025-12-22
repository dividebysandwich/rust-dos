use iced_x86::{Instruction, MemorySize, Mnemonic, OpKind, Register};

use crate::cpu::{Cpu, FLAG_AF, FLAG_CF, FLAG_OF, FLAG_SF, FLAG_ZF};
use crate::interrupt::handle_interrupt;

// ========================================================================
// Helper Functions
// ========================================================================

fn is_8bit_reg(reg: Register) -> bool {
    matches!(
        reg,
        Register::AL
            | Register::CL
            | Register::DL
            | Register::BL
            | Register::AH
            | Register::CH
            | Register::DH
            | Register::BH
    )
}

// Helper to calculate effective address for Memory operands
fn calculate_addr(cpu: &Cpu, instr: &Instruction) -> usize {
    // Get the Segment Base
    // Default to DS, unless there is a segment override prefix.
    let segment = match instr.segment_prefix() {
        Register::ES => cpu.es,
        Register::CS => cpu.cs,
        Register::SS => cpu.ss,
        Register::DS => cpu.ds,
        Register::FS => 0,
        Register::GS => 0,
        _ => cpu.ds, // Default to DS for data operations
    };

    // Get Base Register Value
    let base = if instr.memory_base() != Register::None {
        cpu.get_reg16(instr.memory_base()) as u32
    } else {
        0
    };

    // Get Index Register Value * Scale
    let index = if instr.memory_index() != Register::None {
        let val = cpu.get_reg16(instr.memory_index()) as u32;
        let scale = instr.memory_index_scale() as u32;
        val * scale
    } else {
        0
    };

    // Get Displacement (The critical fix for [0x0006])
    // We treat everything as u32 to handle the wrap-around math cleanly
    let displacement = instr.memory_displacement32();

    // Calculate Offset with 16-bit wrap-around
    // (Base + Index + Disp) & 0xFFFF
    let offset = (base.wrapping_add(index).wrapping_add(displacement)) & 0xFFFF;

    // Convert Segment:Offset to Physical Address (usize)
    cpu.get_physical_addr(segment, offset as u16)
}

fn get_string_src_segment(instr: &Instruction, cpu: &Cpu) -> u16 {
    match instr.segment_prefix() {
        Register::CS => cpu.cs,
        Register::ES => cpu.es,
        Register::SS => cpu.ss,
        Register::DS => cpu.ds,
        _ => cpu.ds, // Default behavior
    }
}

fn get_shift_count(instr: &Instruction, cpu: &Cpu) -> u32 {
    if instr.op1_kind() == OpKind::Immediate8 {
        instr.immediate8() as u32
    } else if instr.op1_kind() == OpKind::Register {
        cpu.get_reg8(instr.op1_register()) as u32
    } else {
        1
    }
}

// ========================================================================
// Execution Logic
// ========================================================================

pub fn execute_instruction(mut cpu: &mut Cpu, instr: &Instruction) {
    match instr.mnemonic() {
        Mnemonic::Int => {
            handle_interrupt(&mut cpu, instr.immediate8());
        }

        Mnemonic::Mov => {
            // Get Destination Kind
            // MOV [Mem], ...
            if instr.op0_kind() == OpKind::Memory {
                let addr = calculate_addr(&cpu, &instr);

                // Determine Source
                if instr.op1_kind() == OpKind::Register {
                    let reg = instr.op1_register();
                    if is_8bit_reg(reg) {
                        cpu.bus.write_8(addr, cpu.get_reg8(reg));
                    } else {
                        let val = cpu.get_reg16(reg);
                        cpu.bus.write_8(addr, (val & 0xFF) as u8);
                        cpu.bus.write_8(addr + 1, (val >> 8) as u8);
                    }
                } else if instr.op1_kind() == OpKind::Immediate8 {
                    cpu.bus.write_8(addr, instr.immediate8());
                } else if instr.op1_kind() == OpKind::Immediate16 {
                    let val = instr.immediate16();
                    cpu.bus.write_8(addr, (val & 0xFF) as u8);
                    cpu.bus.write_8(addr + 1, (val >> 8) as u8);
                }
            }
            // MOV Reg, ...
            else if instr.op0_kind() == OpKind::Register {
                let reg = instr.op0_register();

                // Source: Register
                if instr.op1_kind() == OpKind::Register {
                    let src_reg = instr.op1_register();
                    if is_8bit_reg(reg) {
                        cpu.set_reg8(reg, cpu.get_reg8(src_reg));
                    } else {
                        cpu.set_reg16(reg, cpu.get_reg16(src_reg));
                    }
                }
                // Source: Memory
                else if instr.op1_kind() == OpKind::Memory {
                    let addr = calculate_addr(&cpu, &instr);
                    if is_8bit_reg(reg) {
                        let val = cpu.bus.read_8(addr);
                        cpu.set_reg8(reg, val);
                    } else {
                        let low = cpu.bus.read_8(addr) as u16;
                        let high = cpu.bus.read_8(addr + 1) as u16;
                        cpu.set_reg16(reg, (high << 8) | low);
                    }
                }
                // Source: Immediate
                else if instr.op1_kind() == OpKind::Immediate8 {
                    cpu.set_reg8(reg, instr.immediate8());
                } else if instr.op1_kind() == OpKind::Immediate16 {
                    cpu.set_reg16(reg, instr.immediate16());
                } else if instr.op1_kind() == OpKind::Immediate8to16 {
                    // Sign-extend 8-bit immediate to 16-bit reg (rare for MOV but safe)
                    cpu.set_reg16(reg, instr.immediate8to16() as u16);
                }
                // Source: Segment Register (MOV AX, DS)
                else if instr.op1_kind() == OpKind::Register {
                    // Handled by Register case above (Iced treats SegRegs as Registers)
                    // But just in case OpKind::SegmentRegister appears in older versions:
                    cpu.set_reg16(reg, cpu.get_reg16(instr.op1_register()));
                }
            }
            // MOV Segment, Reg (MOV DS, AX)
            else if instr.op0_register().is_segment_register() {
                let reg = instr.op0_register();
                let val = if instr.op1_kind() == OpKind::Register {
                    cpu.get_reg16(instr.op1_register())
                } else if instr.op1_kind() == OpKind::Memory {
                    let addr = calculate_addr(&cpu, &instr);
                    let low = cpu.bus.read_8(addr) as u16;
                    let high = cpu.bus.read_8(addr + 1) as u16;
                    (high << 8) | low
                } else {
                    0
                };

                cpu.set_reg16(reg, val);
            }
        }

        // SHL: Shift Left (Multiply by 2)
        // SHL: Shift Logical Left
        Mnemonic::Shl | Mnemonic::Sal => {
            let count = get_shift_count(&instr, &cpu);
            let reg = instr.op0_register();

            // 16-bit
            if !is_8bit_reg(reg) {
                let val = cpu.get_reg16(reg);
                let result = val.wrapping_shl(count);
                cpu.set_reg16(reg, result);

                // Flags
                if count > 0 {
                    cpu.set_flag(FLAG_ZF, result == 0);
                    cpu.set_flag(FLAG_SF, (result & 0x8000) != 0);
                    // CF is the last bit shifted out
                    let last_out = (val >> (16 - count)) & 1;
                    cpu.set_flag(FLAG_CF, last_out != 0);
                }
            }
            // 8-bit
            else {
                let val = cpu.get_reg8(reg);
                let result = val.wrapping_shl(count);
                cpu.set_reg8(reg, result);

                if count > 0 {
                    cpu.set_flag(FLAG_ZF, result == 0);
                    cpu.set_flag(FLAG_SF, (result & 0x80) != 0);
                    let last_out = (val >> (8 - count)) & 1;
                    cpu.set_flag(FLAG_CF, last_out != 0);
                }
            }
        }

        // SHR: Shift Logical Right (Zero Fill)
        Mnemonic::Shr => {
            let count = get_shift_count(&instr, &cpu);
            let reg = instr.op0_register();

            if !is_8bit_reg(reg) {
                let val = cpu.get_reg16(reg);
                let result = val.wrapping_shr(count);
                cpu.set_reg16(reg, result);

                if count > 0 {
                    cpu.set_flag(FLAG_ZF, result == 0);
                    cpu.set_flag(FLAG_SF, (result & 0x8000) != 0); // Always 0 for SHR
                                                                   // CF is the last bit shifted out
                    let last_out = (val >> (count - 1)) & 1;
                    cpu.set_flag(FLAG_CF, last_out != 0);
                }
            } else {
                let val = cpu.get_reg8(reg);
                let result = val.wrapping_shr(count);
                cpu.set_reg8(reg, result);

                if count > 0 {
                    cpu.set_flag(FLAG_ZF, result == 0);
                    cpu.set_flag(FLAG_SF, (result & 0x80) != 0);
                    let last_out = (val >> (count - 1)) & 1;
                    cpu.set_flag(FLAG_CF, last_out != 0);
                }
            }
        }

        // SAR: Shift Arithmetic Right (Sign Extend)
        Mnemonic::Sar => {
            let count = get_shift_count(&instr, &cpu);
            let reg = instr.op0_register();

            if !is_8bit_reg(reg) {
                let val = cpu.get_reg16(reg) as i16; // Cast to signed for arithmetic shift
                let result = val.wrapping_shr(count);
                cpu.set_reg16(reg, result as u16);

                if count > 0 {
                    cpu.set_flag(FLAG_ZF, result == 0);
                    // Cast to u16 before masking to avoid overflow error
                    cpu.set_flag(FLAG_SF, (result as u16 & 0x8000) != 0);

                    // CF logic for SAR: Copy the sign bit if shifting, or the last bit out
                    let val_u = val as u16;
                    // Check the bit that was shifted out
                    let last_out = (val_u >> (count - 1)) & 1;
                    cpu.set_flag(FLAG_CF, last_out != 0);
                }
            } else {
                let val = cpu.get_reg8(reg) as i8; // Cast to signed
                let result = val.wrapping_shr(count);
                cpu.set_reg8(reg, result as u8);

                if count > 0 {
                    cpu.set_flag(FLAG_ZF, result == 0);
                    // Cast to u8 before masking
                    cpu.set_flag(FLAG_SF, (result as u8 & 0x80) != 0);

                    let val_u = val as u8;
                    let last_out = (val_u >> (count - 1)) & 1;
                    cpu.set_flag(FLAG_CF, last_out != 0);
                }
            }
        }

        // AND Dest, Src
        Mnemonic::And => {
            // Check size
            let is_8bit = match instr.op0_kind() {
                OpKind::Register => is_8bit_reg(instr.op0_register()),
                OpKind::Memory => instr.memory_size() == MemorySize::UInt8,
                _ => false,
            };

            if is_8bit {
                // --- 8-BIT AND ---
                let dest = if instr.op0_kind() == OpKind::Register {
                    cpu.get_reg8(instr.op0_register())
                } else {
                    let addr = calculate_addr(&cpu, &instr);
                    cpu.bus.read_8(addr)
                };

                let src = if instr.op1_kind() == OpKind::Register {
                    cpu.get_reg8(instr.op1_register())
                } else {
                    instr.immediate8()
                };

                let res = dest & src;

                if instr.op0_kind() == OpKind::Register {
                    cpu.set_reg8(instr.op0_register(), res);
                } else {
                    let addr = calculate_addr(&cpu, &instr);
                    cpu.bus.write_8(addr, res);
                }

                // Flags (8-bit)
                cpu.set_flag(FLAG_ZF, res == 0);
                cpu.set_flag(FLAG_SF, (res & 0x80) != 0);
                cpu.set_flag(FLAG_OF, false);
                cpu.set_flag(FLAG_CF, false);
            } else {
                // --- 16-BIT AND ---
                let dest = if instr.op0_kind() == OpKind::Register {
                    cpu.get_reg16(instr.op0_register())
                } else {
                    let addr = calculate_addr(&cpu, &instr);
                    let low = cpu.bus.read_8(addr) as u16;
                    let high = cpu.bus.read_8(addr + 1) as u16;
                    (high << 8) | low
                };

                let src = if instr.op1_kind() == OpKind::Register {
                    cpu.get_reg16(instr.op1_register())
                } else if instr.op1_kind() == OpKind::Immediate16 {
                    instr.immediate16()
                } else {
                    instr.immediate8to16() as u16
                };

                let res = dest & src;

                if instr.op0_kind() == OpKind::Register {
                    cpu.set_reg16(instr.op0_register(), res);
                } else {
                    let addr = calculate_addr(&cpu, &instr);
                    cpu.bus.write_8(addr, (res & 0xFF) as u8);
                    cpu.bus.write_8(addr + 1, (res >> 8) as u8);
                }

                // Flags (16-bit)
                cpu.set_flag(FLAG_ZF, res == 0);
                cpu.set_flag(FLAG_SF, (res & 0x8000) != 0);
                cpu.set_flag(FLAG_OF, false);
                cpu.set_flag(FLAG_CF, false);
            }
        }

        // OR Dest, Src
        Mnemonic::Or => {
            let is_8bit = match instr.op0_kind() {
                OpKind::Register => is_8bit_reg(instr.op0_register()),
                OpKind::Memory => instr.memory_size() == MemorySize::UInt8,
                _ => false,
            };

            if is_8bit {
                let dest = if instr.op0_kind() == OpKind::Register {
                    cpu.get_reg8(instr.op0_register())
                } else {
                    let addr = calculate_addr(&cpu, &instr);
                    cpu.bus.read_8(addr)
                };

                let src = if instr.op1_kind() == OpKind::Register {
                    cpu.get_reg8(instr.op1_register())
                } else {
                    instr.immediate8()
                };

                let res = dest | src;

                if instr.op0_kind() == OpKind::Register {
                    cpu.set_reg8(instr.op0_register(), res);
                } else {
                    let addr = calculate_addr(&cpu, &instr);
                    cpu.bus.write_8(addr, res);
                }
                cpu.set_flag(FLAG_ZF, res == 0);
                cpu.set_flag(FLAG_SF, (res & 0x80) != 0);
                cpu.set_flag(FLAG_OF, false);
                cpu.set_flag(FLAG_CF, false);
            } else {
                // 16-BIT
                let dest = if instr.op0_kind() == OpKind::Register {
                    cpu.get_reg16(instr.op0_register())
                } else {
                    let addr = calculate_addr(&cpu, &instr);
                    let low = cpu.bus.read_8(addr) as u16;
                    let high = cpu.bus.read_8(addr + 1) as u16;
                    (high << 8) | low
                };

                let src = if instr.op1_kind() == OpKind::Register {
                    cpu.get_reg16(instr.op1_register())
                } else if instr.op1_kind() == OpKind::Immediate16 {
                    instr.immediate16()
                } else {
                    instr.immediate8to16() as u16
                };

                let res = dest | src;

                if instr.op0_kind() == OpKind::Register {
                    cpu.set_reg16(instr.op0_register(), res);
                } else {
                    let addr = calculate_addr(&cpu, &instr);
                    cpu.bus.write_8(addr, (res & 0xFF) as u8);
                    cpu.bus.write_8(addr + 1, (res >> 8) as u8);
                }
                cpu.set_flag(FLAG_ZF, res == 0);
                cpu.set_flag(FLAG_SF, (res & 0x8000) != 0);
                cpu.set_flag(FLAG_OF, false);
                cpu.set_flag(FLAG_CF, false);
            }
        }

        // XOR Dest, Src
        Mnemonic::Xor => {
            let is_8bit = match instr.op0_kind() {
                OpKind::Register => is_8bit_reg(instr.op0_register()),
                OpKind::Memory => instr.memory_size() == MemorySize::UInt8,
                _ => false,
            };

            if is_8bit {
                let dest = if instr.op0_kind() == OpKind::Register {
                    cpu.get_reg8(instr.op0_register())
                } else {
                    let addr = calculate_addr(&cpu, &instr);
                    cpu.bus.read_8(addr)
                };

                let src = if instr.op1_kind() == OpKind::Register {
                    cpu.get_reg8(instr.op1_register())
                } else {
                    instr.immediate8()
                };

                let res = dest ^ src;

                if instr.op0_kind() == OpKind::Register {
                    cpu.set_reg8(instr.op0_register(), res);
                } else {
                    let addr = calculate_addr(&cpu, &instr);
                    cpu.bus.write_8(addr, res);
                }
                cpu.set_flag(FLAG_ZF, res == 0);
                cpu.set_flag(FLAG_SF, (res & 0x80) != 0);
                cpu.set_flag(FLAG_OF, false);
                cpu.set_flag(FLAG_CF, false);
            } else {
                // 16-BIT
                let dest = if instr.op0_kind() == OpKind::Register {
                    cpu.get_reg16(instr.op0_register())
                } else {
                    let addr = calculate_addr(&cpu, &instr);
                    let low = cpu.bus.read_8(addr) as u16;
                    let high = cpu.bus.read_8(addr + 1) as u16;
                    (high << 8) | low
                };

                let src = if instr.op1_kind() == OpKind::Register {
                    cpu.get_reg16(instr.op1_register())
                } else if instr.op1_kind() == OpKind::Immediate16 {
                    instr.immediate16()
                } else {
                    instr.immediate8to16() as u16
                };

                let res = dest ^ src;

                if instr.op0_kind() == OpKind::Register {
                    cpu.set_reg16(instr.op0_register(), res);
                } else {
                    let addr = calculate_addr(&cpu, &instr);
                    cpu.bus.write_8(addr, (res & 0xFF) as u8);
                    cpu.bus.write_8(addr + 1, (res >> 8) as u8);
                }
                cpu.set_flag(FLAG_ZF, res == 0);
                cpu.set_flag(FLAG_SF, (res & 0x8000) != 0);
                cpu.set_flag(FLAG_OF, false);
                cpu.set_flag(FLAG_CF, false);
            }
        }

        // Compare (CMP AL, Imm8)
        Mnemonic::Cmp => {
            // We need to determine if this is an 8-bit or 16-bit operation.
            let is_8bit = if instr.op0_kind() == OpKind::Register {
                is_8bit_reg(instr.op0_register())
            } else if instr.op1_kind() == OpKind::Register {
                is_8bit_reg(instr.op1_register())
            } else {
                // Fallback: Check Memory Size
                match instr.memory_size() {
                    MemorySize::UInt8 => true,
                    _ => false, // Default to 16-bit
                }
            };

            if is_8bit {
                // --- 8-BIT CMP ---
                let dest_val = if instr.op0_kind() == OpKind::Register {
                    cpu.get_reg8(instr.op0_register())
                } else if instr.op0_kind() == OpKind::Memory {
                    let addr = calculate_addr(cpu, instr);
                    cpu.bus.read_8(addr)
                } else {
                    0
                };

                let src_val = if instr.op1_kind() == OpKind::Register {
                    cpu.get_reg8(instr.op1_register())
                } else if instr.op1_kind() == OpKind::Immediate8 {
                    instr.immediate8()
                } else if instr.op1_kind() == OpKind::Memory {
                    let addr = calculate_addr(cpu, instr);
                    cpu.bus.read_8(addr)
                } else {
                    0
                };

                cpu.alu_sub_8(dest_val, src_val);
            } else {
                // --- 16-BIT CMP ---
                let dest_val = if instr.op0_kind() == OpKind::Register {
                    cpu.get_reg16(instr.op0_register())
                } else if instr.op0_kind() == OpKind::Memory {
                    let addr = calculate_addr(cpu, instr);
                    let low = cpu.bus.read_8(addr) as u16;
                    let high = cpu.bus.read_8(addr + 1) as u16;
                    (high << 8) | low
                } else {
                    0
                };

                // Memory handling for Source Operand
                let src_val = if instr.op1_kind() == OpKind::Register {
                    cpu.get_reg16(instr.op1_register())
                } else if instr.op1_kind() == OpKind::Immediate16 {
                    instr.immediate16()
                } else if instr.op1_kind() == OpKind::Immediate8to16 {
                    instr.immediate8to16() as u16
                } else if instr.op1_kind() == OpKind::Immediate8 {
                    instr.immediate8() as u16
                } else if instr.op1_kind() == OpKind::Memory {
                    let addr = calculate_addr(cpu, instr);
                    let low = cpu.bus.read_8(addr) as u16;
                    let high = cpu.bus.read_8(addr + 1) as u16;
                    (high << 8) | low
                } else {
                    0
                };

                cpu.alu_sub_16(dest_val, src_val);
            }
        }

        // Jump if Equal (JE)
        Mnemonic::Je => {
            if cpu.zflag() {
                cpu.ip = instr.near_branch16() as u16;
            }
        }

        // JCXZ: Jump short if CX register is 0
        Mnemonic::Jcxz => {
            if cpu.cx == 0 {
                cpu.ip = instr.near_branch16() as u16;
            }
        }

        // JNE / JNZ: Jump if Z-Flag is FALSE
        Mnemonic::Jne => {
            if !cpu.zflag() {
                cpu.ip = instr.near_branch16() as u16;
            }
        }

        // Unconditional Jump
        Mnemonic::Jmp => {
            match instr.op0_kind() {
                // JMP Reg (e.g., JMP BX)
                OpKind::Register => {
                    cpu.ip = cpu.get_reg16(instr.op0_register());
                }
                // JMP [Mem] (e.g., JMP [BX]) - Near Indirect
                OpKind::Memory => {
                    let addr = calculate_addr(cpu, instr);
                    let low = cpu.bus.read_8(addr) as u16;
                    let high = cpu.bus.read_8(addr + 1) as u16;
                    cpu.ip = (high << 8) | low;
                }
                // JMP Imm (e.g., JMP 0x1234) - Direct / Relative
                OpKind::NearBranch16 | OpKind::NearBranch32 | OpKind::NearBranch64 => {
                    cpu.ip = instr.near_branch16() as u16;
                }
                // JMP Far (e.g. JMP 1234:5678)
                OpKind::FarBranch16 => {
                    cpu.cs = instr.far_branch16();
                    cpu.ip = instr.near_branch16() as u16;
                }
                // Fallback for short jumps which are simple offsets
                _ => {
                    cpu.ip = instr.near_branch16() as u16;
                }
            }
        }

        // JBE (Jump Below or Equal) / JNA (Jump Not Above)
        // Jump if CF=1 OR ZF=1
        Mnemonic::Jbe => {
            if cpu.get_flag(FLAG_CF) || cpu.get_flag(FLAG_ZF) {
                cpu.ip = instr.near_branch16() as u16;
            }
        }

        // JO (Jump if Overflow)
        Mnemonic::Jo => {
            if cpu.get_flag(FLAG_OF) {
                cpu.ip = instr.near_branch16() as u16;
            }
        }

        // SBB: Subtract with Borrow (Dest = Dest - Src - CF)
        Mnemonic::Sbb => {
            let is_8bit = match instr.op0_kind() {
                OpKind::Register => is_8bit_reg(instr.op0_register()),
                OpKind::Memory => instr.memory_size() == MemorySize::UInt8,
                _ => false,
            };

            if is_8bit {
                // --- 8-BIT SBB ---
                let dest = if instr.op0_kind() == OpKind::Register {
                    cpu.get_reg8(instr.op0_register())
                } else {
                    let addr = calculate_addr(&cpu, &instr);
                    cpu.bus.read_8(addr)
                };

                let src = if instr.op1_kind() == OpKind::Register {
                    cpu.get_reg8(instr.op1_register())
                } else {
                    instr.immediate8()
                };

                let res = cpu.alu_sbb_8(dest, src); // Use helper

                if instr.op0_kind() == OpKind::Register {
                    cpu.set_reg8(instr.op0_register(), res);
                } else {
                    let addr = calculate_addr(&cpu, &instr);
                    cpu.bus.write_8(addr, res);
                }
            } else {
                // --- 16-BIT SBB ---
                let dest = if instr.op0_kind() == OpKind::Register {
                    cpu.get_reg16(instr.op0_register())
                } else {
                    let addr = calculate_addr(&cpu, &instr);
                    let low = cpu.bus.read_8(addr) as u16;
                    let high = cpu.bus.read_8(addr + 1) as u16;
                    (high << 8) | low
                };

                let src = if instr.op1_kind() == OpKind::Register {
                    cpu.get_reg16(instr.op1_register())
                } else if instr.op1_kind() == OpKind::Immediate16 {
                    instr.immediate16()
                } else {
                    instr.immediate8to16() as u16
                };

                let res = cpu.alu_sbb_16(dest, src); // Use helper

                if instr.op0_kind() == OpKind::Register {
                    cpu.set_reg16(instr.op0_register(), res);
                } else {
                    let addr = calculate_addr(&cpu, &instr);
                    cpu.bus.write_8(addr, (res & 0xFF) as u8);
                    cpu.bus.write_8(addr + 1, (res >> 8) as u8);
                }
            }
        }

        // AAA: ASCII Adjust After Addition
        // Used in BCD math, but often used by obfuscators to mess with flags
        Mnemonic::Aaa => {
            let al = cpu.get_al();
            let af = cpu.get_flag(FLAG_AF);

            if (al & 0x0F) > 9 || af {
                let new_al = al.wrapping_add(6);
                cpu.set_reg8(Register::AL, new_al & 0x0F);

                let ah = cpu.get_ah();
                cpu.set_reg8(Register::AH, ah.wrapping_add(1));

                cpu.set_flag(FLAG_AF, true);
                cpu.set_flag(FLAG_CF, true);
            } else {
                cpu.set_flag(FLAG_AF, false);
                cpu.set_flag(FLAG_CF, false);
                cpu.set_reg8(Register::AL, al & 0x0F);
            }
            // AAA does not affect OF, SF, ZF, PF according to Intel manual (undefined behavior)
            // We leave them as is.
        }
        // Increment (INC r/m)
        Mnemonic::Inc => {
            // Determine Operand Size
            let is_8bit = match instr.op0_kind() {
                OpKind::Register => is_8bit_reg(instr.op0_register()),
                OpKind::Memory => instr.memory_size() == MemorySize::UInt8,
                _ => false,
            };

            // Read Source
            let (val, addr_opt) = if instr.op0_kind() == OpKind::Register {
                let reg = instr.op0_register();
                let v = if is_8bit {
                    cpu.get_reg8(reg) as u16
                } else {
                    cpu.get_reg16(reg)
                };
                (v, None)
            } else {
                // Memory
                let addr = calculate_addr(&cpu, &instr);
                let v = if is_8bit {
                    cpu.bus.read_8(addr) as u16
                } else {
                    let low = cpu.bus.read_8(addr) as u16;
                    let high = cpu.bus.read_8(addr + 1) as u16;
                    (high << 8) | low
                };
                (v, Some(addr))
            };

            // Perform Increment
            let result = if is_8bit {
                (val as u8).wrapping_add(1) as u16
            } else {
                val.wrapping_add(1)
            };

            // Write Back
            if let Some(addr) = addr_opt {
                if is_8bit {
                    cpu.bus.write_8(addr, result as u8);
                } else {
                    cpu.bus.write_8(addr, (result & 0xFF) as u8);
                    cpu.bus.write_8(addr + 1, (result >> 8) as u8);
                }
            } else {
                let reg = instr.op0_register();
                if is_8bit {
                    cpu.set_reg8(reg, result as u8);
                } else {
                    cpu.set_reg16(reg, result);
                }
            }

            // Update Flags (Preserve CF)
            // OF: Set if sign bit changes from 0 to 1 (e.g. 0x7F -> 0x80)
            cpu.set_flag(
                FLAG_ZF,
                if is_8bit {
                    (result & 0xFF) == 0
                } else {
                    result == 0
                },
            );

            if is_8bit {
                let r8 = result as u8;
                cpu.set_flag(FLAG_SF, (r8 & 0x80) != 0);
                cpu.set_flag(FLAG_OF, r8 == 0x80); // Overflow 127 -> -128
                                                   // AF: Set if carry from bit 3 to 4. (val & 0xF) + 1 > 0xF
                cpu.set_flag(FLAG_AF, (val & 0x0F) + 1 > 0x0F);
            } else {
                cpu.set_flag(FLAG_SF, (result & 0x8000) != 0);
                cpu.set_flag(FLAG_OF, result == 0x8000); // Overflow 32767 -> -32768
                cpu.set_flag(FLAG_AF, (val & 0x0F) + 1 > 0x0F);
            }
            // PF (Parity) omitted for brevity, but rarely used by LZEXE
        }

        // Decrement (DEC r/m)
        Mnemonic::Dec => {
            // Determine Operand Size
            let is_8bit = match instr.op0_kind() {
                OpKind::Register => is_8bit_reg(instr.op0_register()),
                OpKind::Memory => instr.memory_size() == MemorySize::UInt8,
                _ => false,
            };

            // Read Source
            let (val, addr_opt) = if instr.op0_kind() == OpKind::Register {
                let reg = instr.op0_register();
                let v = if is_8bit {
                    cpu.get_reg8(reg) as u16
                } else {
                    cpu.get_reg16(reg)
                };
                (v, None)
            } else {
                let addr = calculate_addr(&cpu, &instr);
                let v = if is_8bit {
                    cpu.bus.read_8(addr) as u16
                } else {
                    let low = cpu.bus.read_8(addr) as u16;
                    let high = cpu.bus.read_8(addr + 1) as u16;
                    (high << 8) | low
                };
                (v, Some(addr))
            };

            // Perform Decrement
            let result = if is_8bit {
                (val as u8).wrapping_sub(1) as u16
            } else {
                val.wrapping_sub(1)
            };

            // Write Back
            if let Some(addr) = addr_opt {
                if is_8bit {
                    cpu.bus.write_8(addr, result as u8);
                } else {
                    cpu.bus.write_8(addr, (result & 0xFF) as u8);
                    cpu.bus.write_8(addr + 1, (result >> 8) as u8);
                }
            } else {
                let reg = instr.op0_register();
                if is_8bit {
                    cpu.set_reg8(reg, result as u8);
                } else {
                    cpu.set_reg16(reg, result);
                }
            }

            // Update Flags (Preserve CF)
            // OF: Set if sign bit changes from 1 to 0 (e.g. 0x80 -> 0x7F)
            cpu.set_flag(
                FLAG_ZF,
                if is_8bit {
                    (result & 0xFF) == 0
                } else {
                    result == 0
                },
            );

            if is_8bit {
                let r8 = result as u8;
                cpu.set_flag(FLAG_SF, (r8 & 0x80) != 0);
                cpu.set_flag(FLAG_OF, r8 == 0x7F); // Overflow -128 -> 127
                cpu.set_flag(FLAG_AF, (val & 0x0F) == 0); // Borrow from bit 4
            } else {
                cpu.set_flag(FLAG_SF, (result & 0x8000) != 0);
                cpu.set_flag(FLAG_OF, result == 0x7FFF); // Overflow -32768 -> 32767
                cpu.set_flag(FLAG_AF, (val & 0x0F) == 0);
            }
        }

        // Stack Operations
        Mnemonic::Push => {
            let val = if instr.op0_kind() == OpKind::Register {
                // This handles AX, BX... AND CS, DS, ES, SS automatically
                cpu.get_reg16(instr.op0_register())
            } else if instr.op0_kind() == OpKind::Immediate8 {
                instr.immediate8() as i8 as i16 as u16 // Sign extend
            } else if instr.op0_kind() == OpKind::Immediate16 {
                instr.immediate16()
            } else if instr.op0_kind() == OpKind::Memory {
                let addr = calculate_addr(cpu, instr);
                let low = cpu.bus.read_8(addr) as u16;
                let high = cpu.bus.read_8(addr + 1) as u16;
                (high << 8) | low
            } else {
                println!("[CPU] Warning: Pushing 0 for unknown OpKind");
                0
            };
            cpu.push(val);
        }

        Mnemonic::Pop => {
            let val = cpu.pop();
            
            if instr.op0_kind() == OpKind::Register {
                // This handles AX, BX... AND CS, DS, ES, SS automatically
                cpu.set_reg16(instr.op0_register(), val);
            } else if instr.op0_kind() == OpKind::Memory {
                let addr = calculate_addr(cpu, instr);
                cpu.bus.write_8(addr, (val & 0xFF) as u8);
                cpu.bus.write_8(addr + 1, (val >> 8) as u8);
            }
        }

        // Subroutines
        Mnemonic::Call => {
            // Check if it is a FAR CALL (Inter-segment)
            if instr.op0_kind() == OpKind::FarBranch16 {
                cpu.push(cpu.cs);
                cpu.push(cpu.ip);

                // Jump to Target
                cpu.cs = instr.far_branch16();
                cpu.ip = instr.near_branch16() as u16; // Iced uses near_branch16 for the offset part
            }
            // Standard NEAR CALL (Intra-segment)
            else {
                cpu.push(cpu.ip);

                // Handle Register/Memory calls (CALL AX, CALL [BX])
                if instr.op0_kind() == OpKind::Register {
                    cpu.ip = cpu.get_reg16(instr.op0_register());
                } else if instr.op0_kind() == OpKind::Memory {
                    let addr = calculate_addr(&cpu, &instr);
                    let low = cpu.bus.read_8(addr) as u16;
                    let high = cpu.bus.read_8(addr + 1) as u16;
                    cpu.ip = (high << 8) | low;
                } else {
                    // Relative/Immediate Call
                    cpu.ip = instr.near_branch16() as u16;
                }
            }
        }
        Mnemonic::Ret => {
            // NEAR RET: Pop IP
            cpu.ip = cpu.pop();
        }
        // LEA Reg, Mem: Load Effective Address
        Mnemonic::Lea => {
            let reg = instr.op0_register();
            let addr = calculate_addr(&cpu, &instr); // Use your existing helper
            cpu.set_reg16(reg, addr as u16);
        }
        // TEST: Logical Compare (AND but discard result)
        Mnemonic::Test => {
            // Determine size
            let is_8bit = match instr.op0_kind() {
                OpKind::Register => is_8bit_reg(instr.op0_register()),
                OpKind::Memory => instr.memory_size() == MemorySize::UInt8,
                _ => false,
            };

            if is_8bit {
                let val1 = if instr.op0_kind() == OpKind::Register {
                    cpu.get_reg8(instr.op0_register())
                } else {
                    // Memory
                    let addr = calculate_addr(&cpu, &instr);
                    cpu.bus.read_8(addr)
                };

                let val2 = if instr.op1_kind() == OpKind::Register {
                    cpu.get_reg8(instr.op1_register())
                } else {
                    instr.immediate8()
                };

                let result = val1 & val2;
                cpu.set_flag(FLAG_ZF, result == 0);
                cpu.set_flag(FLAG_SF, (result & 0x80) != 0);
                cpu.set_flag(FLAG_OF, false); // AND/TEST clears OF/CF
                cpu.set_flag(FLAG_CF, false);
            } else {
                let val1 = if instr.op0_kind() == OpKind::Register {
                    cpu.get_reg16(instr.op0_register())
                } else {
                    let addr = calculate_addr(&cpu, &instr);
                    let low = cpu.bus.read_8(addr) as u16;
                    let high = cpu.bus.read_8(addr + 1) as u16;
                    (high << 8) | low
                };

                let val2 = if instr.op1_kind() == OpKind::Register {
                    cpu.get_reg16(instr.op1_register())
                } else if instr.op1_kind() == OpKind::Immediate16 {
                    instr.immediate16()
                } else {
                    instr.immediate8to16() as u16
                };

                let result = val1 & val2;
                cpu.set_flag(FLAG_ZF, result == 0);
                cpu.set_flag(FLAG_SF, (result & 0x8000) != 0);
                cpu.set_flag(FLAG_OF, false);
                cpu.set_flag(FLAG_CF, false);
            }
        }

        // LODSB: Load Byte from [DS:SI] into AL, update SI
        Mnemonic::Lodsb => {
            let seg = get_string_src_segment(&instr, &cpu);
            let addr = cpu.get_physical_addr(seg, cpu.si);
            let val = cpu.bus.read_8(addr);
            cpu.set_reg8(Register::AL, val);
            if cpu.dflag() {
                cpu.si = cpu.si.wrapping_sub(1);
            } else {
                cpu.si = cpu.si.wrapping_add(1);
            }
        }
        // LODSW: Load Word from [DS:SI] into AX, update SI
        Mnemonic::Lodsw => {
            let seg = get_string_src_segment(&instr, &cpu);
            let addr = cpu.get_physical_addr(seg, cpu.si);
            let low = cpu.bus.read_8(addr) as u16;
            let high = cpu.bus.read_8(addr + 1) as u16;
            cpu.ax = (high << 8) | low;
            if cpu.dflag() {
                cpu.si = cpu.si.wrapping_sub(2);
            } else {
                cpu.si = cpu.si.wrapping_add(2);
            }
        }

        // STOSB: Store AL to [ES:DI], update DI
        Mnemonic::Stosb => {
            let has_rep = instr.has_rep_prefix();
            loop {
                if has_rep && cpu.cx == 0 {
                    break;
                }

                let addr = cpu.get_physical_addr(cpu.es, cpu.di);
                cpu.bus.write_8(addr, cpu.get_al());

                if cpu.dflag() {
                    cpu.di = cpu.di.wrapping_sub(1);
                } else {
                    cpu.di = cpu.di.wrapping_add(1);
                }

                if has_rep {
                    cpu.cx = cpu.cx.wrapping_sub(1);
                } else {
                    break;
                }
            }
        }
        // STOSW: Store AX to [ES:DI], update DI
        Mnemonic::Stosw => {
            let has_rep = instr.has_rep_prefix();
            loop {
                if has_rep && cpu.cx == 0 {
                    break;
                }

                let addr = cpu.get_physical_addr(cpu.es, cpu.di);
                cpu.bus.write_8(addr, cpu.get_al()); // Low
                cpu.bus.write_8(addr + 1, cpu.get_ah()); // High

                if cpu.dflag() {
                    cpu.di = cpu.di.wrapping_sub(2);
                } else {
                    cpu.di = cpu.di.wrapping_add(2);
                }

                if has_rep {
                    cpu.cx = cpu.cx.wrapping_sub(1);
                } else {
                    break;
                }
            }
        }

        // MOVSB: Move Byte [DS:SI] -> [ES:DI], update SI, DI
        Mnemonic::Movsb => {
            let has_rep = instr.has_rep_prefix();

            loop {
                // Check Exit Condition
                if has_rep && cpu.cx == 0 {
                    break;
                }

                // Perform Move
                let src_seg = get_string_src_segment(&instr, &cpu);
                let src_addr = cpu.get_physical_addr(src_seg, cpu.si);
                let dest_addr = cpu.get_physical_addr(cpu.es, cpu.di);

                let val = cpu.bus.read_8(src_addr);
                cpu.bus.write_8(dest_addr, val);

                // Update Indices
                if cpu.dflag() {
                    cpu.si = cpu.si.wrapping_sub(1);
                    cpu.di = cpu.di.wrapping_sub(1);
                } else {
                    cpu.si = cpu.si.wrapping_add(1);
                    cpu.di = cpu.di.wrapping_add(1);
                }

                // Handle Repetition
                if has_rep {
                    cpu.cx = cpu.cx.wrapping_sub(1);
                } else {
                    break; // Single execution
                }
            }
        }
        // MOVSW: Move Word [DS:SI] -> [ES:DI], update SI, DI
        Mnemonic::Movsw => {
            let has_rep = instr.has_rep_prefix();

            loop {
                if has_rep && cpu.cx == 0 {
                    break;
                }

                let src_seg = get_string_src_segment(&instr, &cpu);
                let src_addr = cpu.get_physical_addr(src_seg, cpu.si);
                let dest_addr = cpu.get_physical_addr(cpu.es, cpu.di);

                let low = cpu.bus.read_8(src_addr);
                let high = cpu.bus.read_8(src_addr + 1);

                cpu.bus.write_8(dest_addr, low);
                cpu.bus.write_8(dest_addr + 1, high);

                if cpu.dflag() {
                    cpu.si = cpu.si.wrapping_sub(2);
                    cpu.di = cpu.di.wrapping_sub(2);
                } else {
                    cpu.si = cpu.si.wrapping_add(2);
                    cpu.di = cpu.di.wrapping_add(2);
                }

                if has_rep {
                    cpu.cx = cpu.cx.wrapping_sub(1);
                } else {
                    break;
                }
            }
        }

        Mnemonic::Lds => {
            if instr.op0_kind() == OpKind::Register && instr.op1_kind() == OpKind::Memory {
                let reg = instr.op0_register();

                // Calculate memory address of the 32-bit pointer
                let segment = cpu.get_reg16(instr.memory_segment());
                let offset = cpu.get_reg16(instr.memory_base()); // Simplify: assumes no displacement/index for now
                let addr = cpu.get_physical_addr(segment, offset);

                // Read Offset (Low 16 bits)
                let val_low = cpu.bus.read_8(addr) as u16;
                let val_high = cpu.bus.read_8(addr + 1) as u16;
                let new_offset = (val_high << 8) | val_low;

                // Read Segment (High 16 bits)
                let seg_low = cpu.bus.read_8(addr + 2) as u16;
                let seg_high = cpu.bus.read_8(addr + 3) as u16;
                let new_seg = (seg_high << 8) | seg_low;

                // Update Registers
                cpu.set_reg16(reg, new_offset);
                cpu.ds = new_seg;

                // println!("[CPU] LDS loaded DS={:04X} {:?}={:04X}", new_seg, reg, new_offset);
            }
        }

        // LES Reg, Mem: Load ES:Reg from Memory
        Mnemonic::Les => {
            if instr.op0_kind() == OpKind::Register && instr.op1_kind() == OpKind::Memory {
                let reg = instr.op0_register();

                let segment = cpu.get_reg16(instr.memory_segment());
                let offset = cpu.get_reg16(instr.memory_base());
                let addr = cpu.get_physical_addr(segment, offset);

                let val_low = cpu.bus.read_8(addr) as u16;
                let val_high = cpu.bus.read_8(addr + 1) as u16;
                let new_offset = (val_high << 8) | val_low;

                let seg_low = cpu.bus.read_8(addr + 2) as u16;
                let seg_high = cpu.bus.read_8(addr + 3) as u16;
                let new_seg = (seg_high << 8) | seg_low;

                cpu.set_reg16(reg, new_offset);
                cpu.es = new_seg;
            }
        }

        Mnemonic::Add => {
            // Determine size: 8-bit or 16-bit?
            // We check op0 (dest). If it's an 8-bit reg or memory is byte ptr.
            let is_8bit = match instr.op0_kind() {
                OpKind::Register => is_8bit_reg(instr.op0_register()),
                OpKind::Memory => instr.memory_size() == MemorySize::UInt8,
                _ => false,
            };

            // Case A: Memory Destination
            if instr.op0_kind() == OpKind::Memory {
                let addr = calculate_addr(&cpu, &instr); // Use the new helper!

                if is_8bit {
                    let dest = cpu.bus.read_8(addr);
                    let src = if instr.op1_kind() == OpKind::Register {
                        cpu.get_reg8(instr.op1_register())
                    } else {
                        instr.immediate8()
                    }; // Handle Imm8

                    let res = cpu.alu_add_8(dest, src); // <--- You need to add this helper to CPU
                    cpu.bus.write_8(addr, res);
                } else {
                    let low = cpu.bus.read_8(addr) as u16;
                    let high = cpu.bus.read_8(addr + 1) as u16;
                    let dest = (high << 8) | low;

                    // Handle Immediates for Memory Ops
                    let src = if instr.op1_kind() == OpKind::Register {
                        cpu.get_reg16(instr.op1_register())
                    } else if instr.op1_kind() == OpKind::Immediate8to16 {
                        instr.immediate8to16() as u16
                    } else {
                        instr.immediate16()
                    };

                    let res = cpu.alu_add_16(dest, src);
                    cpu.bus.write_8(addr, (res & 0xFF) as u8);
                    cpu.bus.write_8(addr + 1, (res >> 8) as u8);
                }
            }
            // Case B: Register Destination
            else if instr.op0_kind() == OpKind::Register {
                let reg = instr.op0_register();

                if is_8bit {
                    let dest = cpu.get_reg8(reg);
                    let src = if instr.op1_kind() == OpKind::Register {
                        cpu.get_reg8(instr.op1_register())
                    } else {
                        instr.immediate8()
                    };

                    let res = cpu.alu_add_8(dest, src);
                    cpu.set_reg8(reg, res);
                } else {
                    let dest = cpu.get_reg16(reg);
                    let src = if instr.op1_kind() == OpKind::Register {
                        cpu.get_reg16(instr.op1_register())
                    } else if instr.op1_kind() == OpKind::Immediate8to16 {
                        instr.immediate8to16() as u16
                    } else {
                        instr.immediate16()
                    };

                    let res = cpu.alu_add_16(dest, src);
                    cpu.set_reg16(reg, res);
                }
            }
        }

        // SUB Dest, Src
        Mnemonic::Sub => {
            // Determine size (8-bit vs 16-bit)
            // Checks op0 (destination) to determine operation width
            let is_8bit = match instr.op0_kind() {
                OpKind::Register => is_8bit_reg(instr.op0_register()),
                OpKind::Memory => instr.memory_size() == MemorySize::UInt8,
                _ => false,
            };

            // Case A: Memory Destination
            if instr.op0_kind() == OpKind::Memory {
                let addr = calculate_addr(&cpu, &instr);

                if is_8bit {
                    let dest = cpu.bus.read_8(addr);
                    let src = if instr.op1_kind() == OpKind::Register {
                        cpu.get_reg8(instr.op1_register())
                    } else {
                        instr.immediate8()
                    };

                    let res = cpu.alu_sub_8(dest, src);
                    cpu.bus.write_8(addr, res);
                } else {
                    let low = cpu.bus.read_8(addr) as u16;
                    let high = cpu.bus.read_8(addr + 1) as u16;
                    let dest = (high << 8) | low;

                    let src = if instr.op1_kind() == OpKind::Register {
                        cpu.get_reg16(instr.op1_register())
                    } else if instr.op1_kind() == OpKind::Immediate8to16 {
                        instr.immediate8to16() as u16
                    } else {
                        instr.immediate16()
                    };

                    let res = cpu.alu_sub_16(dest, src);
                    cpu.bus.write_8(addr, (res & 0xFF) as u8);
                    cpu.bus.write_8(addr + 1, (res >> 8) as u8);
                }
            }
            // Case B: Register Destination
            else if instr.op0_kind() == OpKind::Register {
                let reg = instr.op0_register();

                if is_8bit {
                    let dest = cpu.get_reg8(reg);
                    let src = if instr.op1_kind() == OpKind::Register {
                        cpu.get_reg8(instr.op1_register())
                    } else {
                        instr.immediate8()
                    };

                    let res = cpu.alu_sub_8(dest, src);
                    cpu.set_reg8(reg, res);
                } else {
                    let dest = cpu.get_reg16(reg);
                    let src = if instr.op1_kind() == OpKind::Register {
                        cpu.get_reg16(instr.op1_register())
                    } else if instr.op1_kind() == OpKind::Immediate8to16 {
                        instr.immediate8to16() as u16
                    } else {
                        instr.immediate16()
                    };

                    let res = cpu.alu_sub_16(dest, src);
                    cpu.set_reg16(reg, res);
                }
            }
        }

        // NOT Dest (Invert bits)
        Mnemonic::Not => {
            // Determine size
            let is_8bit = match instr.op0_kind() {
                OpKind::Register => is_8bit_reg(instr.op0_register()),
                OpKind::Memory => instr.memory_size() == MemorySize::UInt8,
                _ => false,
            };

            if instr.op0_kind() == OpKind::Register {
                let reg = instr.op0_register();
                if is_8bit {
                    let val = cpu.get_reg8(reg);
                    cpu.set_reg8(reg, !val);
                } else {
                    let val = cpu.get_reg16(reg);
                    cpu.set_reg16(reg, !val);
                }
            } else if instr.op0_kind() == OpKind::Memory {
                let addr = calculate_addr(&cpu, &instr);
                if is_8bit {
                    let val = cpu.bus.read_8(addr);
                    cpu.bus.write_8(addr, !val);
                } else {
                    let low = cpu.bus.read_8(addr) as u16;
                    let high = cpu.bus.read_8(addr + 1) as u16;
                    let val = (high << 8) | low;
                    let res = !val;
                    cpu.bus.write_8(addr, (res & 0xFF) as u8);
                    cpu.bus.write_8(addr + 1, (res >> 8) as u8);
                }
            }
            // NOT does not affect flags on x86
        }

        // RETF: Return Far (Pop IP, then Pop CS)
        Mnemonic::Retf => {
            cpu.ip = cpu.pop();
            cpu.cs = cpu.pop();

            // RETF can optionally pop bytes from stack (RETF Imm16)
            if instr.op0_kind() == OpKind::Immediate16 {
                let imm = instr.immediate16();
                cpu.sp = cpu.sp.wrapping_add(imm);
            }
        }

        // JAE / JNB / JNC: Jump if Carry Flag is 0
        Mnemonic::Jae => {
            if !cpu.get_flag(FLAG_CF) {
                cpu.ip = instr.near_branch16() as u16;
            }
        }

        // SCASB: Compare AL with Byte at ES:DI
        Mnemonic::Scasb => {
            // Check for REP/REPE/REPNE Prefixes
            let has_rep = instr.has_rep_prefix() || instr.has_repe_prefix();
            let has_repne = instr.has_repne_prefix();

            // If repeated, this instruction runs inside a loop based on CX
            // If NOT repeated, it runs exactly once.
            loop {
                // Check loop counter first if strictly repeated
                if (has_rep || has_repne) && cpu.cx == 0 {
                    break;
                }

                // SCASB logic
                let addr = cpu.get_physical_addr(cpu.es, cpu.di);
                let mem_val = cpu.bus.read_8(addr);
                let al = cpu.get_al();

                // Compare (AL - Mem) -> Set Flags
                cpu.alu_sub_8(al, mem_val); // This updates ZF, CF, SF, OF

                // Update DI based on Direction Flag
                if cpu.dflag() {
                    cpu.di = cpu.di.wrapping_sub(1);
                } else {
                    cpu.di = cpu.di.wrapping_add(1);
                }

                // Handle Repetition Logic
                if has_rep || has_repne {
                    cpu.cx = cpu.cx.wrapping_sub(1);

                    // REPE (Repeat While Equal): Stop if ZF=0 (Not Equal)
                    if has_rep && !cpu.get_flag(FLAG_ZF) {
                        break;
                    }
                    // REPNE (Repeat While Not Equal): Stop if ZF=1 (Equal)
                    if has_repne && cpu.get_flag(FLAG_ZF) {
                        break;
                    }
                } else {
                    // No prefix? Run once and exit
                    break;
                }
            }
        }

        // IMUL: Signed Multiply
        Mnemonic::Imul => {
            // Check if it's the Single Operand form (IMUL r/m)
            // This form implicitly uses AL (8-bit) or AX (16-bit) as the other operand.
            if instr.op_count() == 1 {
                // Determine Size (8-bit or 16-bit)
                let is_8bit = match instr.op0_kind() {
                    OpKind::Register => is_8bit_reg(instr.op0_register()),
                    OpKind::Memory => instr.memory_size() == MemorySize::UInt8,
                    _ => false,
                };

                // Get Source Value (from Reg or Memory)
                let src_val = if instr.op0_kind() == OpKind::Register {
                    if is_8bit {
                        cpu.get_reg8(instr.op0_register()) as u16
                    } else {
                        cpu.get_reg16(instr.op0_register())
                    }
                } else if instr.op0_kind() == OpKind::Memory {
                    let addr = calculate_addr(&cpu, &instr);
                    if is_8bit {
                        cpu.bus.read_8(addr) as u16
                    } else {
                        let low = cpu.bus.read_8(addr) as u16;
                        let high = cpu.bus.read_8(addr + 1) as u16;
                        (high << 8) | low
                    }
                } else {
                    0
                };

                if is_8bit {
                    // --- 8-BIT IMUL ---
                    // AX = AL * src (Result is 16-bit signed)
                    let al = cpu.get_al() as i8 as i16;
                    let src = src_val as u8 as i8 as i16;
                    let res = al * src;

                    cpu.ax = res as u16;

                    // Flags: CF/OF set if result requires more than 8 bits
                    // i.e., top half is not just a sign extension of low half
                    let fits = res == (res as i8 as i16);
                    cpu.set_flag(FLAG_CF, !fits);
                    cpu.set_flag(FLAG_OF, !fits);
                } else {
                    // --- 16-BIT IMUL --- (This fixes your error)
                    // DX:AX = AX * src (Result is 32-bit signed)
                    let ax = cpu.ax as i16 as i32;
                    let src = src_val as i16 as i32;
                    let res = ax * src;

                    cpu.ax = (res & 0xFFFF) as u16; // Low Word
                    cpu.dx = ((res >> 16) & 0xFFFF) as u16; // High Word

                    // Flags: CF/OF set if result requires more than 16 bits
                    let fits = res == (res as i16 as i32);
                    cpu.set_flag(FLAG_CF, !fits);
                    cpu.set_flag(FLAG_OF, !fits);
                }
            }
            // Handle 2-Operand IMUL (IMUL Dest, Src) - 186/386+ feature
            else if instr.op_count() == 2 {
                // Similar logic to ADD/SUB but for multiplication
                // Note: 2-operand IMUL truncates the result to fit the destination size
                // and does NOT affect DX.
                let dest_reg = instr.op0_register();
                let dest_val = cpu.get_reg16(dest_reg) as i16 as i32;

                let src_val = if instr.op1_kind() == OpKind::Register {
                    cpu.get_reg16(instr.op1_register()) as i16 as i32
                } else if instr.op1_kind() == OpKind::Immediate16 {
                    instr.immediate16() as i16 as i32
                } else if instr.op1_kind() == OpKind::Memory {
                    let addr = calculate_addr(&cpu, &instr);
                    let low = cpu.bus.read_8(addr) as u16;
                    let high = cpu.bus.read_8(addr + 1) as u16;
                    ((high << 8) | low) as i16 as i32
                } else {
                    0
                };

                let res = dest_val * src_val;
                cpu.set_reg16(dest_reg, res as u16);

                let fits = res == (res as i16 as i32);
                cpu.set_flag(FLAG_CF, !fits);
                cpu.set_flag(FLAG_OF, !fits);
            }
        }

        // MUL: Unsigned Multiply (Always Single Operand)
        Mnemonic::Mul => {
            // Determine Size & Source Val (Same as IMUL logic)
            let is_8bit = match instr.op0_kind() {
                OpKind::Register => is_8bit_reg(instr.op0_register()),
                OpKind::Memory => instr.memory_size() == MemorySize::UInt8,
                _ => false,
            };

            // Get Source Value (from Reg or Memory)
            let src_val = if instr.op0_kind() == OpKind::Register {
                if is_8bit {
                    cpu.get_reg8(instr.op0_register()) as u16
                } else {
                    cpu.get_reg16(instr.op0_register())
                }
            } else if instr.op0_kind() == OpKind::Memory {
                let addr = calculate_addr(&cpu, &instr);
                if is_8bit {
                    cpu.bus.read_8(addr) as u16
                } else {
                    let low = cpu.bus.read_8(addr) as u16;
                    let high = cpu.bus.read_8(addr + 1) as u16;
                    (high << 8) | low
                }
            } else {
                0
            };

            if is_8bit {
                let al = cpu.get_al() as u16;
                let src = src_val as u8 as u16; // cast to u8 first to truncate, then u16
                let res = al * src;
                cpu.ax = res;

                // CF/OF set if upper half is nonzero
                let overflow = (res & 0xFF00) != 0;
                cpu.set_flag(FLAG_CF, overflow);
                cpu.set_flag(FLAG_OF, overflow);
            } else {
                let ax = cpu.ax as u32;
                let src = src_val as u32;
                let res = ax * src;
                cpu.ax = (res & 0xFFFF) as u16;
                cpu.dx = (res >> 16) as u16;

                let overflow = (res & 0xFFFF0000) != 0;
                cpu.set_flag(FLAG_CF, overflow);
                cpu.set_flag(FLAG_OF, overflow);
            }
        }

        // ADC: Add with Carry (Dest = Dest + Src + CF)
        Mnemonic::Adc => {
            let is_8bit = match instr.op0_kind() {
                OpKind::Register => is_8bit_reg(instr.op0_register()),
                OpKind::Memory => instr.memory_size() == MemorySize::UInt8,
                _ => false,
            };

            let cf = if cpu.get_flag(FLAG_CF) { 1 } else { 0 };

            if instr.op0_kind() == OpKind::Register {
                let reg = instr.op0_register();
                if is_8bit {
                    let dest = cpu.get_reg8(reg);
                    let src = if instr.op1_kind() == OpKind::Register {
                        cpu.get_reg8(instr.op1_register())
                    } else {
                        instr.immediate8()
                    };

                    // We need a helper for ADC that includes CF
                    // Logic: res = dest + src + cf
                    let (res_temp, carry1) = dest.overflowing_add(src);
                    let (res, carry2) = res_temp.overflowing_add(cf);

                    cpu.set_reg8(reg, res);

                    // Flags
                    cpu.set_flag(FLAG_CF, carry1 || carry2);
                    cpu.set_flag(FLAG_ZF, res == 0);
                    cpu.set_flag(FLAG_SF, (res & 0x80) != 0);
                    // OF: (dest & src & !res) | (!dest & !src & res) check sign bits
                    let sign_dest = (dest & 0x80) != 0;
                    let sign_src = (src & 0x80) != 0;
                    let sign_res = (res & 0x80) != 0;
                    cpu.set_flag(FLAG_OF, (sign_dest == sign_src) && (sign_dest != sign_res));
                    // AF logic omitted for brevity (rarely used outside BCD)
                } else {
                    // 16-bit ADC
                    let dest = cpu.get_reg16(reg);
                    let src = if instr.op1_kind() == OpKind::Register {
                        cpu.get_reg16(instr.op1_register())
                    } else if instr.op1_kind() == OpKind::Immediate8to16 {
                        instr.immediate8to16() as u16
                    } else {
                        instr.immediate16()
                    };

                    let cf_u16 = cf as u16;
                    let (res_temp, carry1) = dest.overflowing_add(src);
                    let (res, carry2) = res_temp.overflowing_add(cf_u16);

                    cpu.set_reg16(reg, res);

                    cpu.set_flag(FLAG_CF, carry1 || carry2);
                    cpu.set_flag(FLAG_ZF, res == 0);
                    cpu.set_flag(FLAG_SF, (res & 0x8000) != 0);

                    let sign_dest = (dest & 0x8000) != 0;
                    let sign_src = (src & 0x8000) != 0;
                    let sign_res = (res & 0x8000) != 0;
                    cpu.set_flag(FLAG_OF, (sign_dest == sign_src) && (sign_dest != sign_res));
                }
            }
            // Handle Memory Destination case if needed...
        }

        // DAS: Decimal Adjust AL after Subtraction
        Mnemonic::Das => {
            let mut al = cpu.get_al();
            let old_cf = cpu.get_flag(FLAG_CF);
            let old_af = cpu.get_flag(FLAG_AF);
            let mut new_cf = false;

            if (al & 0x0F) > 9 || old_af {
                al = al.wrapping_sub(6);
                cpu.set_flag(FLAG_AF, true);
                new_cf = old_cf || (al > 0x99); // borrow might happen
            } else {
                cpu.set_flag(FLAG_AF, false);
            }

            if al > 0x9F || old_cf {
                al = al.wrapping_sub(0x60);
                new_cf = true;
            }

            cpu.set_reg8(Register::AL, al);
            cpu.set_flag(FLAG_CF, new_cf);
            // DAS updates SF, ZF, PF, but leaves OF undefined.
            cpu.set_flag(FLAG_ZF, al == 0);
            cpu.set_flag(FLAG_SF, (al & 0x80) != 0);
        }

        // XCHG: Exchange two operands
        Mnemonic::Xchg => {
            let op0 = instr.op0_kind();
            let op1 = instr.op1_kind();

            // Read Value 1 (Operand 0)
            let (val1, addr1) = if op0 == OpKind::Register {
                if is_8bit_reg(instr.op0_register()) {
                    (cpu.get_reg8(instr.op0_register()) as u16, None::<usize>)
                } else {
                    (cpu.get_reg16(instr.op0_register()), None::<usize>)
                }
            } else {
                let addr = calculate_addr(&cpu, &instr);
                if instr.memory_size() == MemorySize::UInt8 {
                    (cpu.bus.read_8(addr) as u16, Some(addr))
                } else {
                    let low = cpu.bus.read_8(addr) as u16;
                    let high = cpu.bus.read_8(addr + 1) as u16;
                    ((high << 8) | low, Some(addr))
                }
            };

            // Read Value 2 (Operand 1)
            let (val2, addr2) = if op1 == OpKind::Register {
                if is_8bit_reg(instr.op1_register()) {
                    (cpu.get_reg8(instr.op1_register()) as u16, None::<usize>)
                } else {
                    (cpu.get_reg16(instr.op1_register()), None::<usize>)
                }
            } else {
                // Operand 1 is Memory (e.g. XCHG AX, [BX])
                let addr = calculate_addr(&cpu, &instr);
                if instr.memory_size() == MemorySize::UInt8 {
                    (cpu.bus.read_8(addr) as u16, Some(addr))
                } else {
                    let low = cpu.bus.read_8(addr) as u16;
                    let high = cpu.bus.read_8(addr + 1) as u16;
                    ((high << 8) | low, Some(addr))
                }
            };

            // Write Value 2 into Dest 1 (Operand 0)
            if let Some(addr) = addr1 {
                if instr.memory_size() == MemorySize::UInt8 {
                    cpu.bus.write_8(addr, val2 as u8);
                } else {
                    cpu.bus.write_8(addr, (val2 & 0xFF) as u8);
                    cpu.bus.write_8(addr + 1, (val2 >> 8) as u8);
                }
            } else {
                let reg = instr.op0_register();
                if is_8bit_reg(reg) {
                    cpu.set_reg8(reg, val2 as u8);
                } else {
                    cpu.set_reg16(reg, val2);
                }
            }

            // Write Value 1 into Dest 2 (Operand 1)
            if let Some(addr) = addr2 {
                if instr.memory_size() == MemorySize::UInt8 {
                    cpu.bus.write_8(addr, val1 as u8);
                } else {
                    cpu.bus.write_8(addr, (val1 & 0xFF) as u8);
                    cpu.bus.write_8(addr + 1, (val1 >> 8) as u8);
                }
            } else {
                let reg = instr.op1_register();
                if is_8bit_reg(reg) {
                    cpu.set_reg8(reg, val1 as u8);
                } else {
                    cpu.set_reg16(reg, val1);
                }
            }
        }

        // POPA: Pop All General Registers (186+)
        Mnemonic::Popa => {
            let di = cpu.pop();
            let si = cpu.pop();
            let bp = cpu.pop();
            let _sp_ignore = cpu.pop(); // SP is popped but discarded
            let bx = cpu.pop();
            let dx = cpu.pop();
            let cx = cpu.pop();
            let ax = cpu.pop();

            cpu.di = di;
            cpu.si = si;
            cpu.set_reg16(Register::BP, bp);
            // SP is already updated by the pops
            cpu.set_reg16(Register::BX, bx);
            cpu.dx = dx;
            cpu.cx = cx;
            cpu.ax = ax;
        }

        // WAIT: Wait for FPU (Treat as NOP for now)
        Mnemonic::Wait => {
            // Do nothing.
        }

        // Flag Manipulation
        Mnemonic::Stc => cpu.set_flag(FLAG_CF, true),
        Mnemonic::Sti => { /* Enable Interrupts - Often ignored in simple emus */ }
        Mnemonic::Clc => cpu.set_flag(FLAG_CF, false),
        Mnemonic::Cli => { /* Disable Interrupts */ }

        // JL / JNGE: Jump Less (Signed)
        // Jump if SF != OF
        Mnemonic::Jl => {
            if cpu.get_flag(FLAG_SF) != cpu.get_flag(FLAG_OF) {
                cpu.ip = instr.near_branch16() as u16;
            }
        }

        // JLE / JNG: Jump Less or Equal (Signed)
        // Jump if ZF=1 or SF != OF
        Mnemonic::Jle => {
            if cpu.get_flag(FLAG_ZF) || (cpu.get_flag(FLAG_SF) != cpu.get_flag(FLAG_OF)) {
                cpu.ip = instr.near_branch16() as u16;
            }
        }

        // JB / JC / JNAE: Jump if Below (Unsigned)
        // Jump if Carry Flag is 1
        Mnemonic::Jb => {
            if cpu.get_flag(FLAG_CF) {
                cpu.ip = instr.near_branch16() as u16;
            }
        }

        // FLAGS
        Mnemonic::Cld => cpu.set_dflag(false), // Clear Direction Flag (Increment)
        Mnemonic::Std => cpu.set_dflag(true),  // Set Direction Flag (Decrement)

        // PUSHF: Push Flags
        Mnemonic::Pushf => {
            let val = cpu.flags;
            cpu.push(val);
        }

        // POPF: Pop Flags
        Mnemonic::Popf => {
            let val = cpu.pop();
            // Always keep bit 1 set, bits 3,5,15 are usually 0 on 8086
            cpu.flags = (val & 0x0FD5) | 0x0002;
        }

        // LOOP: DEC CX, Jump if CX != 0
        Mnemonic::Loop => {
            cpu.cx = cpu.cx.wrapping_sub(1);
            if cpu.cx != 0 {
                cpu.ip = instr.near_branch16() as u16;
            }
        }

        // OUT Port, Reg (Write to Port)
        Mnemonic::Out => {
            // Determine Value (Source)
            let val = if is_8bit_reg(instr.op1_register()) {
                cpu.get_reg8(instr.op1_register())
            } else {
                // Simple PC Speaker only uses 8-bit output, but safe to grab AL
                cpu.get_al()
            };

            // Determine Port (Destination)
            let port = if instr.op0_kind() == OpKind::Register {
                cpu.dx // OUT DX, AL
            } else {
                instr.immediate8() as u16 // OUT Imm8, AL
            };

            cpu.bus.io_write(port, val);
        }

        // IN Reg, Port (Read from Port)
        Mnemonic::In => {
            // Determine Port (Source)
            let port = if instr.op1_kind() == OpKind::Register {
                cpu.dx // IN AL, DX
            } else {
                instr.immediate8() as u16 // IN AL, Imm8
            };

            let val = cpu.bus.io_read(port);

            // Write Value (Destination is always AL/AX)
            if is_8bit_reg(instr.op0_register()) {
                cpu.set_reg8(instr.op0_register(), val);
            } else {
                // Rarely used for speaker, but clears AH usually
                cpu.set_reg16(instr.op0_register(), val as u16);
            }
        }

        _ => {
            println!("[CPU] Unhandled Instruction: {}", instr);
        }
    }
}
