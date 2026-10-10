//! The H8/532's instructions. A port of Nuked-SC55's `mcu_opcodes.cpp`,
//! its quirks kept: what the firmware does depends on them.

use super::machine::{Exception, Machine, SR_MASK, STATUS_C, STATUS_N, STATUS_V, STATUS_Z};

// Kinds of general operand.
const DIRECT: u8 = 0;
const INDIRECT: u8 = 1;
const ABSOLUTE: u8 = 2;
const IMMEDIATE: u8 = 3;

impl Machine {
    #[inline]
    fn set_status(&mut self, condition: bool, mask: u16) {
        if condition {
            self.sr |= mask;
        } else {
            self.sr &= !mask;
        }
    }

    /// N and Z from a value, V cleared.
    #[inline]
    fn set_status_common(&mut self, val: u32, word: bool) {
        let val = if word {
            let val = val & 0xffff;
            self.set_status(val & 0x8000 != 0, STATUS_N);
            val
        } else {
            let val = val & 0xff;
            self.set_status(val & 0x80 != 0, STATUS_N);
            val
        };
        self.set_status(val == 0, STATUS_Z);
        self.set_status(false, STATUS_V);
    }

    fn sub_common(&mut self, t1: i32, t2: i32, c_bit: i32, word: bool) -> i32 {
        let (result, n, z, c, v);
        if word {
            let st = (t1 as i16) as i32 - (t2 as i16) as i32 - c_bit;
            let t = (t1 as u16) as i32 - (t2 as u16) as i32 - c_bit;
            c = (t >> 16) & 1 != 0;
            result = t & 0xffff;
            n = result & 0x8000 != 0;
            z = result == 0;
            v = st < i16::MIN as i32 || st > i16::MAX as i32;
        } else {
            let st = (t1 as i8) as i32 - (t2 as i8) as i32 - c_bit;
            let t = (t1 as u8) as i32 - (t2 as u8) as i32 - c_bit;
            c = (t >> 8) & 1 != 0;
            result = t & 0xff;
            n = result & 0x80 != 0;
            z = result == 0;
            v = st < i8::MIN as i32 || st > i8::MAX as i32;
        }
        self.set_status(n, STATUS_N);
        self.set_status(z, STATUS_Z);
        self.set_status(c, STATUS_C);
        self.set_status(v, STATUS_V);
        result
    }

    fn add_common(&mut self, t1: i32, t2: i32, c_bit: i32, word: bool) -> i32 {
        let (result, n, z, c, v);
        if word {
            let st = (t1 as i16) as i32 + (t2 as i16) as i32 + c_bit;
            let t = (t1 as u16) as i32 + (t2 as u16) as i32 + c_bit;
            c = (t >> 16) & 1 != 0;
            result = t & 0xffff;
            n = result & 0x8000 != 0;
            z = result == 0;
            v = st < i16::MIN as i32 || st > i16::MAX as i32;
        } else {
            let st = (t1 as i8) as i32 + (t2 as i8) as i32 + c_bit;
            let t = (t1 as u8) as i32 + (t2 as u8) as i32 + c_bit;
            c = (t >> 8) & 1 != 0;
            result = t & 0xff;
            n = result & 0x80 != 0;
            z = result == 0;
            v = st < i8::MIN as i32 || st > i8::MAX as i32;
        }
        self.set_status(n, STATUS_N);
        self.set_status(z, STATUS_Z);
        self.set_status(c, STATUS_C);
        self.set_status(v, STATUS_V);
        result
    }

    fn page_for_register(&self, reg: u8) -> u8 {
        if reg >= 6 {
            self.tp
        } else if reg >= 4 {
            self.ep
        } else {
            self.dp
        }
    }

    fn control_register_write(&mut self, reg: u8, word: bool, data: u32) {
        if word {
            match reg {
                0 => self.sr = (data as u16) & SR_MASK,
                5 => self.dp = data as u8,
                4 => self.ep = data as u8,
                3 => self.br = data as u8,
                _ => {}
            }
        } else {
            match reg {
                1 => {
                    self.sr &= !0xff;
                    self.sr |= (data & 0xff) as u16;
                    self.sr &= SR_MASK;
                }
                3 => self.br = data as u8,
                4 => self.ep = data as u8,
                5 => self.dp = data as u8,
                7 => self.tp = data as u8,
                _ => {}
            }
        }
    }

    fn control_register_read(&self, reg: u8, word: bool) -> u32 {
        let double = |b: u8| b as u32 | (b as u32) << 8;
        if word {
            match reg {
                0 => (self.sr & SR_MASK) as u32,
                5 => double(self.dp),
                4 => double(self.ep),
                3 => double(self.br),
                _ => 0,
            }
        } else {
            match reg {
                1 => (self.sr & SR_MASK & 0xff) as u32,
                3 => self.br as u32,
                4 => self.ep as u32,
                5 => self.dp as u32,
                7 => self.tp as u32,
                _ => 0,
            }
        }
    }

    fn operand_read(&mut self) -> u32 {
        match self.operand_type {
            DIRECT => {
                let r = self.r[self.operand_reg as usize] as u32;
                if self.operand_word { r } else { r & 0xff }
            }
            INDIRECT | ABSOLUTE => {
                let address = Self::get_address(self.operand_ep, self.operand_ea);
                if self.operand_word {
                    if self.operand_ea & 1 != 0 {
                        self.exception_pending = Some(Exception::AddressError);
                    }
                    self.read16(address) as u32
                } else {
                    self.read(address) as u32
                }
            }
            _ => self.operand_data as u32,
        }
    }

    fn operand_write(&mut self, data: u32) {
        match self.operand_type {
            DIRECT => {
                let r = &mut self.r[self.operand_reg as usize];
                if self.operand_word {
                    *r = data as u16;
                } else {
                    *r = (*r & !0xff) | (data & 0xff) as u16;
                }
            }
            INDIRECT | ABSOLUTE => {
                let address = Self::get_address(self.operand_ep, self.operand_ea);
                if self.operand_word {
                    if self.operand_ea & 1 != 0 {
                        self.exception_pending = Some(Exception::AddressError);
                    }
                    self.write16(address, data as u16);
                } else {
                    self.write(address, data as u8);
                }
            }
            _ => self.exception_pending = Some(Exception::InvalidInstruction),
        }
    }

    /// Store into a register's low byte or all of it.
    #[inline]
    fn set_register(&mut self, reg: u8, value: u32, word: bool) {
        let r = &mut self.r[reg as usize];
        if word {
            *r = value as u16;
        } else {
            *r = (*r & !0xff) | (value & 0xff) as u16;
        }
    }

    fn read_code16(&mut self) -> u16 {
        let high = self.read_code() as u16;
        (high << 8) | self.read_code() as u16
    }

    /// Carry out the instruction starting with `operand`.
    pub(super) fn execute(&mut self, operand: u8) {
        match operand {
            0x00 => {}
            0x01 | 0x06 | 0x07 | 0x10 | 0x11 => self.jump_jmp(operand),
            0x02 => {
                // LDM
                let rlist = self.read_code();
                for i in 0..8 {
                    if rlist & (1 << i) != 0 {
                        let data = self.pop_stack();
                        if i != 7 {
                            self.r[i] = data;
                        }
                    }
                }
            }
            0x03 => {
                // PJSR
                let page = self.read_code();
                let address = self.read_code16();
                self.push_stack(self.pc);
                self.push_stack(self.cp as u16);
                self.cp = page;
                self.pc = address;
            }
            0x04 | 0x05 | 0x0c | 0x0d | 0x15 | 0x1d | 0xa0..=0xff => self.operand_general(operand),
            0x08 => {
                // TRAPA
                let opcode = self.read_code();
                if opcode & 0xf0 == 0x10 {
                    self.trapa_pending |= 1 << (opcode & 0x0f);
                }
            }
            0x0a => {
                // RTE
                self.sr = self.pop_stack();
                self.cp = self.pop_stack() as u8;
                self.pc = self.pop_stack();
                self.ex_ignore = true;
            }
            0x0e | 0x1e => {
                // BSR
                let disp = if operand == 0x0e { self.read_code() as i8 as u16 } else { self.read_code16() };
                self.push_stack(self.pc);
                self.pc = self.pc.wrapping_add(disp);
            }
            0x12 => {
                // STM
                let rlist = self.read_code();
                for i in (0..8).rev() {
                    if rlist & (1 << i) != 0 {
                        let mut data = self.r[i];
                        if i == 7 {
                            data = data.wrapping_sub(2);
                        }
                        self.push_stack(data);
                    }
                }
            }
            0x13 => {
                // PJMP
                let page = self.read_code();
                let address = self.read_code16();
                self.cp = page;
                self.pc = address;
            }
            0x14 | 0x1c => {
                // RTD
                let imm = self.read_code() as i8;
                self.pc = self.pop_stack();
                if operand == 0x14 {
                    self.r[7] = self.r[7].wrapping_add(imm as u16);
                }
            }
            0x18 => {
                // JSR
                let address = self.read_code16();
                self.push_stack(self.pc);
                self.pc = address;
            }
            0x19 => self.pc = self.pop_stack(),
            0x1a => self.sleep = true,
            0x20..=0x3f => self.jump_bcc(operand),
            0x40..=0x4f => {
                // CMP:I
                let reg = (operand & 7) as usize;
                let word = operand & 0x08 != 0;
                let t2 = if word { self.read_code16() as i32 } else { self.read_code() as i32 };
                let t1 = self.r[reg] as i32;
                self.sub_common(t1, t2, 0, word);
            }
            0x50..=0x57 => {
                // MOV:E
                let reg = (operand & 7) as usize;
                let data = self.read_code();
                self.r[reg] = (self.r[reg] & !0xff) | data as u16;
                self.set_status_common(data as u32, false);
            }
            0x58..=0x5f => {
                // MOV:I
                let reg = (operand & 7) as usize;
                let data = self.read_code16();
                self.r[reg] = data;
                self.set_status_common(data as u32, true);
            }
            0x60..=0x6f => {
                // MOV:L
                let reg = (operand & 7) as usize;
                let word = operand & 0x08 != 0;
                let addr = ((self.br as u16) << 8) | self.read_code() as u16;
                if word {
                    if addr & 1 != 0 {
                        self.exception_pending = Some(Exception::AddressError);
                    }
                    let data = self.read16(addr as u32);
                    self.r[reg] = data;
                    self.set_status_common(data as u32, true);
                } else {
                    let data = self.read(addr as u32);
                    self.r[reg] = (self.r[reg] & !0xff) | data as u16;
                    self.set_status_common(data as u32, false);
                }
            }
            0x70..=0x7f => {
                // MOV:S
                let reg = (operand & 7) as usize;
                let word = operand & 0x08 != 0;
                let addr = ((self.br as u16) << 8) | self.read_code() as u16;
                if word {
                    if addr & 1 != 0 {
                        self.exception_pending = Some(Exception::AddressError);
                    }
                    let data = self.r[reg];
                    self.write16(addr as u32, data);
                    self.set_status_common(data as u32, true);
                } else {
                    let data = self.r[reg] & 0xff;
                    self.write(addr as u32, data as u8);
                    self.set_status_common(data as u32, false);
                }
            }
            0x80..=0x9f => self.short_movf(operand),
            // Not implemented: 09, 0B, 0F, 16, 17, 1B, 1F.
            _ => {}
        }
    }

    fn short_movf(&mut self, opcode: u8) {
        let reg = (opcode & 7) as usize;
        let siz = opcode & 0x08 != 0;
        let disp = self.read_code() as i8;
        let addr = (self.r[6].wrapping_add(disp as u16) as u32) | (self.tp as u32) << 16;
        if opcode & 0x10 == 0 {
            if siz {
                // As Nuked-SC55 has it: a word read into the low byte.
                let data = self.read16(addr);
                self.r[reg] = (self.r[reg] & !0xff) | data;
                self.set_status_common(data as u32, false);
            } else {
                let data = self.read(addr) as u16;
                self.r[reg] = data;
                self.set_status_common(data as u32, true);
            }
        } else if siz {
            let data = self.r[reg] & 0xff;
            self.write(addr, data as u8);
            self.set_status_common(data as u32, false);
        } else {
            let data = self.r[reg];
            self.write16(addr, data);
            self.set_status_common(data as u32, true);
        }
    }

    fn jump_bcc(&mut self, operand: u8) {
        let disp = if operand & 0x10 != 0 { self.read_code16() } else { self.read_code() as i8 as u16 };
        let n = self.sr & STATUS_N != 0;
        let c = self.sr & STATUS_C != 0;
        let z = self.sr & STATUS_Z != 0;
        let v = self.sr & STATUS_V != 0;
        let branch = match operand & 0x0f {
            0x0 => true,
            0x1 => false,
            0x2 => !(c || z),
            0x3 => c || z,
            0x4 => !c,
            0x5 => c,
            0x6 => !z,
            0x7 => z,
            0x8 => !v,
            0x9 => v,
            0xa => !n,
            0xb => n,
            0xc => n == v,
            0xd => n != v,
            0xe => !(z || n != v),
            _ => z || n != v,
        };
        if branch {
            self.pc = self.pc.wrapping_add(disp);
        }
    }

    fn jump_jmp(&mut self, operand: u8) {
        match operand {
            0x11 => {
                let opcode = self.read_code();
                let opcode_h = opcode >> 3;
                let opcode_l = (opcode & 7) as usize;
                if opcode == 0x19 {
                    self.cp = self.pop_stack() as u8;
                    self.pc = self.pop_stack();
                } else if opcode_h == 0x19 {
                    self.push_stack(self.pc);
                    self.push_stack(self.cp as u16);
                    let l = opcode_l & !1;
                    self.cp = self.r[l] as u8;
                    self.pc = self.r[l + 1];
                } else if opcode_h == 0x1a {
                    self.pc = self.r[opcode_l];
                } else if opcode_h == 0x1b {
                    self.push_stack(self.pc);
                    self.pc = self.r[opcode_l];
                }
            }
            0x01 | 0x06 | 0x07 => {
                // SCB/F, SCB/EQ, SCB/NE
                let opcode = self.read_code();
                let reg = (opcode & 7) as usize;
                if opcode >> 3 == 0x17 {
                    let disp = self.read_code() as i8 as u16;
                    let z = self.sr & STATUS_Z != 0;
                    let count = match operand {
                        0x01 => true,
                        0x06 => z,
                        _ => !z,
                    };
                    if count {
                        self.r[reg] = self.r[reg].wrapping_sub(1);
                        if self.r[reg] != 0xffff {
                            self.pc = self.pc.wrapping_add(disp);
                        }
                    }
                }
            }
            _ => {
                // 0x10: JMP @aa:16
                self.pc = self.read_code16();
            }
        }
    }

    fn operand_general(&mut self, operand: u8) {
        let word = operand & 0x08 != 0;
        let reg = operand & 0x07;
        let mut kind = DIRECT;
        let mut disp: u16 = 0;
        let mut increase = 0i8;
        let mut data: u16 = 0;
        let mut addr: u16 = 0;
        let mut addrpage = 0u8;
        match operand & 0xf0 {
            0xa0 => {}
            0xd0 => kind = INDIRECT,
            0xe0 => {
                kind = INDIRECT;
                disp = self.read_code() as i8 as u16;
            }
            0xf0 => {
                kind = INDIRECT;
                disp = self.read_code16();
            }
            0xb0 => {
                kind = INDIRECT;
                increase = -1;
            }
            0xc0 => {
                kind = INDIRECT;
                increase = 1;
            }
            0x00 => {
                if reg == 5 {
                    kind = ABSOLUTE;
                    addr = ((self.br as u16) << 8) | self.read_code() as u16;
                } else if reg == 4 {
                    kind = IMMEDIATE;
                    data = if word { self.read_code16() } else { self.read_code() as u16 };
                }
            }
            0x10 if reg == 5 => {
                kind = ABSOLUTE;
                addr = self.read_code16();
                addrpage = self.dp;
            }
            _ => {}
        }
        let mut ea = 0u16;
        let mut ep = 0u8;
        if kind == INDIRECT {
            let step = if word || reg == 7 { 2 } else { 1 };
            let r = reg as usize;
            if increase < 0 {
                self.r[r] = self.r[r].wrapping_sub(step);
            }
            ea = self.r[r].wrapping_add(disp);
            if increase > 0 {
                self.r[r] = self.r[r].wrapping_add(step);
            }
            ep = self.page_for_register(reg);
        } else if kind == ABSOLUTE {
            ea = addr;
            ep = addrpage;
        }

        let mut opcode = self.read_code();
        self.opcode_extended = opcode == 0x00;
        if self.opcode_extended {
            opcode = self.read_code();
        }
        let opcode_reg = opcode & 0x07;
        let opcode = opcode >> 3;

        self.operand_type = kind;
        self.operand_ea = ea;
        self.operand_ep = ep;
        self.operand_word = word;
        self.operand_reg = reg;
        self.operand_data = data;

        self.general(opcode, opcode_reg);
    }

    /// The instructions with a general operand, by their opcode's top
    /// five bits.
    fn general(&mut self, opcode: u8, opcode_reg: u8) {
        let word = self.operand_word;
        let memory = self.operand_type == INDIRECT || self.operand_type == ABSOLUTE;
        let not_immediate = self.operand_type != IMMEDIATE;
        let rn = opcode_reg as usize;
        match opcode {
            0x00 => {
                // MOV:G with an immediate, and CMP:G
                if opcode_reg == 6 && memory {
                    let data = self.read_code() as i8 as i32 as u32;
                    self.operand_write(data);
                    self.set_status_common(data, word);
                } else if opcode_reg == 7 && memory {
                    let data = self.read_code16() as u32;
                    self.operand_write(data);
                    self.set_status_common(data, word);
                } else if opcode_reg == 4 && memory && !word {
                    let t1 = self.operand_read();
                    let t2 = self.read_code() as u32;
                    self.sub_common(t1 as i32, t2 as i32, 0, false);
                } else if opcode_reg == 4 && memory && word {
                    let t1 = self.operand_read();
                    let t2 = self.read_code() as i8 as u16 as u32;
                    self.sub_common(t1 as i32, t2 as i32, 0, true);
                } else if opcode_reg == 5 && memory {
                    let t1 = self.operand_read();
                    let t2 = self.read_code16() as u32;
                    self.sub_common(t1 as i32, t2 as i32, 0, word);
                }
            }
            0x01 => {
                // ADD:Q
                let t1 = self.operand_read() as i32;
                let t2 = match opcode_reg {
                    0 => 1,
                    1 => 2,
                    4 => -1,
                    5 => -2,
                    _ => 0,
                };
                let result = self.add_common(t1, t2, 0, word);
                self.operand_write(result as u32);
            }
            0x02 => self.clr_group(opcode_reg),
            0x03 => self.shift_group(opcode_reg),
            0x04 => {
                // ADD
                let t1 = self.r[rn] as i32;
                let t2 = self.operand_read() as i32;
                let result = self.add_common(t1, t2, 0, word);
                self.set_register(opcode_reg, result as u32, word);
            }
            0x05 => {
                // ADDS
                let mut data = self.operand_read();
                if !word {
                    data = data as i8 as i32 as u32;
                }
                self.r[rn] = (self.r[rn] as u32).wrapping_add(data) as u16;
            }
            0x06 => {
                // SUB
                let t1 = self.r[rn] as i32;
                let t2 = self.operand_read() as i32;
                let result = self.sub_common(t1, t2, 0, word);
                self.set_register(opcode_reg, result as u32, word);
            }
            0x07 => {
                // SUBS
                let t1 = self.r[rn] as i32;
                let t2 = self.operand_read() as i32;
                self.r[rn] = if word { t1.wrapping_sub(t2) as u16 } else { t1.wrapping_sub(t2 as i8 as i32) as u16 };
            }
            0x08 => {
                // OR
                let data = self.operand_read();
                self.r[rn] |= data as u16;
                self.set_status_common(self.r[rn] as u32, word);
            }
            0x09 => {
                if self.operand_type == IMMEDIATE {
                    // ORC
                    let data = self.operand_read();
                    let val = self.control_register_read(opcode_reg, word) | data;
                    self.control_register_write(opcode_reg, word, val);
                    if opcode_reg >= 2 {
                        self.set_status_common(val, word);
                    }
                    self.ex_ignore = true;
                } else {
                    // BSET with the bit in a register
                    let data = self.operand_read();
                    let bit = (self.r[rn] & 0x0f) as u32;
                    self.set_status(data & (1 << bit) == 0, STATUS_Z);
                    self.operand_write(data | (1 << bit));
                }
            }
            0x0a => {
                // AND
                let data = self.r[rn] as u32 & self.operand_read();
                self.set_register(opcode_reg, data, word);
                self.set_status_common(self.r[rn] as u32, word);
            }
            0x0b => {
                if self.operand_type == IMMEDIATE {
                    // ANDC
                    let data = self.operand_read();
                    let val = self.control_register_read(opcode_reg, word) & data;
                    self.control_register_write(opcode_reg, word, val);
                    if opcode_reg >= 2 {
                        self.set_status_common(val, word);
                    }
                    self.ex_ignore = true;
                } else {
                    // BCLR with the bit in a register
                    let data = self.operand_read();
                    let bit = (self.r[rn] & 0x0f) as u32;
                    self.set_status(data & (1 << bit) == 0, STATUS_Z);
                    self.operand_write(data & !(1 << bit));
                }
            }
            0x0c => {
                // XOR
                let data = self.operand_read();
                self.r[rn] ^= data as u16;
                self.set_status_common(self.r[rn] as u32, word);
            }
            0x0d => {}
            0x0e => {
                // CMP
                let t1 = self.r[rn] as i32;
                let t2 = self.operand_read() as i32;
                self.sub_common(t1, t2, 0, word);
            }
            0x0f => {
                // BTST with the bit in a register
                if not_immediate {
                    let data = self.operand_read();
                    let bit = (self.r[rn] & 0x0f) as u32;
                    self.set_status(data & (1 << bit) == 0, STATUS_Z);
                }
            }
            0x10 | 0x12 => self.movg(opcode, opcode_reg),
            0x11 => {
                // LDC
                let data = self.operand_read();
                self.control_register_write(opcode_reg, word, data);
                self.ex_ignore = true;
            }
            0x13 => {
                // STC
                let data = self.control_register_read(opcode_reg, word);
                self.operand_write(data);
            }
            0x14 => {
                // ADDX
                let t1 = self.r[rn] as i32;
                let t2 = self.operand_read() as i32;
                let c = self.sr & STATUS_C != 0;
                let z = self.sr & STATUS_Z != 0;
                let result = self.add_common(t1, t2, c as i32, word);
                if !z {
                    self.set_status(false, STATUS_Z);
                }
                self.set_register(opcode_reg, result as u32, word);
            }
            0x15 => self.mulxu(opcode_reg),
            0x16 => {
                // SUBX
                let t1 = self.r[rn] as i32;
                let t2 = self.operand_read() as i32;
                let c = self.sr & STATUS_C != 0;
                let result = self.sub_common(t1, t2, c as i32, word);
                self.set_register(opcode_reg, result as u32, word);
            }
            0x17 => self.divxu(opcode_reg),
            0x18..=0x1f if not_immediate => {
                // BSET, BCLR, BNOT, BTST with the bit in the instruction
                let data = self.operand_read();
                let bit = opcode_reg as u32 | ((opcode as u32 & 1) << 3);
                self.set_status(data & (1 << bit) == 0, STATUS_Z);
                match opcode {
                    0x18 | 0x19 => self.operand_write(data | (1 << bit)),
                    0x1a | 0x1b => self.operand_write(data & !(1 << bit)),
                    0x1c | 0x1d => self.operand_write(data ^ (1 << bit)),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn movg(&mut self, opcode: u8, opcode_reg: u8) {
        if self.opcode_extended {
            return;
        }
        let word = self.operand_word;
        let rn = opcode_reg as usize;
        if opcode & 2 != 0 {
            if self.operand_type == DIRECT {
                // XCH
                if word {
                    self.r.swap(rn, self.operand_reg as usize);
                }
            } else {
                let data = self.r[rn] as u32;
                self.operand_write(data);
                self.set_status_common(data, word);
            }
        } else {
            let data = self.operand_read();
            self.set_register(opcode_reg, data, word);
            self.set_status_common(data, word);
        }
    }

    fn clr_group(&mut self, opcode_reg: u8) {
        let word = self.operand_word;
        let not_immediate = self.operand_type != IMMEDIATE;
        let direct_byte = self.operand_type == DIRECT && !word;
        let or = self.operand_reg as usize;
        match opcode_reg {
            3 if not_immediate => {
                // CLR
                self.operand_write(0);
                self.set_status(false, STATUS_N);
                self.set_status(true, STATUS_Z);
                self.set_status(false, STATUS_V);
                self.set_status(false, STATUS_C);
            }
            6 if not_immediate => {
                // TST
                let data = self.operand_read();
                self.set_status_common(data, word);
                self.set_status(false, STATUS_C);
            }
            2 if direct_byte => {
                // EXTU
                let data = self.r[or] & 0xff;
                self.r[or] = data;
                self.set_status(false, STATUS_N);
                self.set_status(data == 0, STATUS_Z);
                self.set_status(false, STATUS_V);
                self.set_status(false, STATUS_C);
            }
            0 if direct_byte => {
                // SWAP
                let data = self.r[or].rotate_left(8);
                self.r[or] = data;
                self.set_status_common(data as u32, true);
            }
            5 if not_immediate => {
                // NOT
                let data = !self.operand_read();
                self.operand_write(data);
                self.set_status_common(data, word);
            }
            4 if not_immediate => {
                // NEG
                let data = self.operand_read();
                let result = self.sub_common(0, data as i32, 0, word);
                self.operand_write(result as u32);
            }
            1 if direct_byte => {
                // EXTS
                let data = self.r[or] as u32;
                self.r[or] = data as u8 as i8 as u16;
                self.set_status_common(data, true);
            }
            _ => {}
        }
    }

    fn shift_group(&mut self, opcode_reg: u8) {
        if self.operand_type == IMMEDIATE || opcode_reg > 6 {
            return;
        }
        let word = self.operand_word;
        let msb: u32 = if word { 0x8000 } else { 0x80 };
        let mut data = self.operand_read();
        let c;
        match opcode_reg {
            3 => {
                // SHLR
                c = data & 1 != 0;
                data >>= 1;
            }
            0 | 2 => {
                // SHAL, SHLL
                c = data & msb != 0;
                data <<= 1;
            }
            6 => {
                // ROTXL
                let bit = (self.sr & STATUS_C != 0) as u32;
                c = data & msb != 0;
                data = (data << 1) | bit;
            }
            4 => {
                // ROTL
                c = data & msb != 0;
                data = (data << 1) | c as u32;
            }
            1 => {
                // SHAR
                c = data & 1 != 0;
                let top = data & msb;
                data = ((data & (msb | (msb - 1))) >> 1) | top;
            }
            _ => {
                // 5: ROTR
                c = data & 1 != 0;
                data = (data >> 1) | if c { msb } else { 0 };
            }
        }
        self.operand_write(data);
        self.set_status(c, STATUS_C);
        self.set_status_common(data, word);
    }

    fn mulxu(&mut self, opcode_reg: u8) {
        let word = self.operand_word;
        let t1 = self.operand_read();
        let mut t2 = self.r[opcode_reg as usize] as u32;
        if !word {
            t2 &= 0xff;
        }
        let mut product = t1.wrapping_mul(t2);
        let n = if word {
            let r = (opcode_reg & !1) as usize;
            self.r[r] = (product >> 16) as u16;
            self.r[r | 1] = product as u16;
            product & 0x8000_0000 != 0
        } else {
            product &= 0xffff;
            self.r[opcode_reg as usize] = product as u16;
            product & 0x8000 != 0
        };
        self.set_status(n, STATUS_N);
        self.set_status(product == 0, STATUS_Z);
        self.set_status(false, STATUS_V);
        self.set_status(false, STATUS_C);
    }

    fn divxu(&mut self, opcode_reg: u8) {
        let word = self.operand_word;
        let t1 = self.operand_read();
        if t1 == 0 {
            self.set_status(false, STATUS_N);
            self.set_status(true, STATUS_Z);
            self.set_status(false, STATUS_V);
            self.set_status(false, STATUS_C);
            return;
        }
        let overflow = |m: &mut Machine| {
            m.set_status(false, STATUS_N);
            m.set_status(false, STATUS_Z);
            m.set_status(true, STATUS_V);
            m.set_status(false, STATUS_C);
        };
        if word {
            let r = (opcode_reg & !1) as usize;
            let t2 = (self.r[r] as u32) << 16 | self.r[r | 1] as u32;
            let (rem, quot) = (t2 % t1, t2 / t1);
            if quot > u16::MAX as u32 {
                overflow(self);
            } else {
                self.r[r] = rem as u16;
                self.r[r | 1] = quot as u16;
                self.set_status_common(quot, true);
                self.set_status(false, STATUS_C);
            }
        } else {
            let t2 = self.r[opcode_reg as usize] as u32;
            let (rem, quot) = (t2 % t1, t2 / t1);
            if quot > u8::MAX as u32 {
                overflow(self);
            } else {
                self.r[opcode_reg as usize] = (((rem & 0xff) << 8) | (quot & 0xff)) as u16;
                self.set_status_common(quot & 0xff, false);
                self.set_status(false, STATUS_C);
            }
        }
    }
}
