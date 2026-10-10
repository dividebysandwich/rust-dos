//! The mkII's sub-MCU, a Mitsubishi M37450 (a 6502 relative), which takes
//! the MIDI input and passes it to the main processor through shared
//! memory. A port of Nuked-SC55's `submcu.cpp`, its quirks kept (a 16-bit
//! read reads one byte twice).

use super::machine::Machine;

const SM_STATUS_C: u8 = 1;
const SM_STATUS_Z: u8 = 2;
const SM_STATUS_I: u8 = 4;
const SM_STATUS_D: u8 = 8;
const SM_STATUS_T: u8 = 32;
const SM_STATUS_N: u8 = 128;

const SM_VECTOR_UART3_TX: u16 = 0;
const SM_VECTOR_UART2_TX: u16 = 1;
const SM_VECTOR_UART1_TX: u16 = 2;
const SM_VECTOR_COLLISION: u16 = 3;
const SM_VECTOR_TIMER_X: u16 = 4;
const SM_VECTOR_IPCM0: u16 = 5;
const SM_VECTOR_UART3_RX: u16 = 6;
const SM_VECTOR_UART2_RX: u16 = 7;
const SM_VECTOR_UART1_RX: u16 = 8;
const SM_VECTOR_RESET: u16 = 9;

const SM_DEV_P1_DATA: usize = 0x00;
const SM_DEV_P1_DIR: usize = 0x01;
const SM_DEV_RAM_DIR: usize = 0x02;
const SM_DEV_UART1_MODE_STATUS: usize = 0x05;
const SM_DEV_UART1_CTRL: usize = 0x06;
const SM_DEV_UART2_DATA: usize = 0x08;
const SM_DEV_UART2_MODE_STATUS: usize = 0x09;
const SM_DEV_UART2_CTRL: usize = 0x0a;
const SM_DEV_UART3_MODE_STATUS: usize = 0x0d;
const SM_DEV_UART3_CTRL: usize = 0x0e;
const SM_DEV_IPCM0: usize = 0x10;
const SM_DEV_IPCE0: usize = 0x14;
const SM_DEV_SEMAPHORE: usize = 0x19;
const SM_DEV_COLLISION: usize = 0x1a;
const SM_DEV_INT_ENABLE: usize = 0x1b;
const SM_DEV_INT_REQUEST: usize = 0x1c;
const SM_DEV_PRESCALER: usize = 0x1d;
const SM_DEV_TIMER: usize = 0x1e;
const SM_DEV_TIMER_CTRL: usize = 0x1f;

pub(super) struct SubMcu {
    pc: u16,
    a: u8,
    x: u8,
    y: u8,
    s: u8,
    sr: u8,
    pub cycles: u64,
    sleep: bool,
    rom: Box<[u8]>,
    ram: [u8; 128],
    shared_ram: [u8; 192],
    access: [u8; 0x18],
    p0_dir: u8,
    p1_dir: u8,
    device_mode: [u8; 32],
    cts: u8,
    timer_cycles: u64,
    timer_prescaler: u8,
    timer_counter: u8,
    uart_rx_gotbyte: bool,
}

impl SubMcu {
    pub fn new(rom: &[u8]) -> SubMcu {
        let mut copy = vec![0u8; 0x1000];
        copy[..rom.len().min(0x1000)].copy_from_slice(&rom[..rom.len().min(0x1000)]);
        SubMcu {
            pc: 0,
            a: 0,
            x: 0,
            y: 0,
            s: 0,
            sr: 0,
            cycles: 0,
            sleep: false,
            rom: copy.into(),
            ram: [0; 128],
            shared_ram: [0; 192],
            access: [0; 0x18],
            p0_dir: 0,
            p1_dir: 0,
            device_mode: [0; 32],
            cts: 0,
            timer_cycles: 0,
            timer_prescaler: 0,
            timer_counter: 0,
            uart_rx_gotbyte: false,
        }
    }

    #[inline]
    fn set_status(&mut self, condition: bool, mask: u8) {
        if condition {
            self.sr |= mask;
        } else {
            self.sr &= !mask;
        }
    }

    fn update_nz(&mut self, val: u8) {
        self.set_status(val == 0, SM_STATUS_Z);
        self.set_status(val & 0x80 != 0, SM_STATUS_N);
    }
}

impl Machine {
    fn sm_read(&mut self, address: u16) -> u8 {
        let address = address & 0x1fff;
        let sm = &mut self.sm;
        if address & 0x1000 != 0 {
            sm.rom[(address & 0xfff) as usize]
        } else if address < 0x80 {
            sm.ram[address as usize]
        } else if (0xc0..0xd8).contains(&address) {
            sm.access[(address & 0x1f) as usize]
        } else if (0xe0..0x100).contains(&address) {
            let address = (address & 0x1f) as usize;
            match address {
                SM_DEV_UART2_DATA => {
                    sm.uart_rx_gotbyte = false;
                    self.uart_rx_byte
                }
                SM_DEV_UART1_MODE_STATUS | SM_DEV_UART3_MODE_STATUS => 5,
                SM_DEV_UART2_MODE_STATUS => ((sm.uart_rx_gotbyte as u8) << 1) | 5,
                SM_DEV_P1_DATA => self.read_p1(),
                SM_DEV_P1_DIR => sm.p1_dir,
                SM_DEV_PRESCALER => sm.timer_prescaler,
                SM_DEV_TIMER => sm.timer_counter,
                _ => sm.device_mode[address],
            }
        } else if (0x200..0x2c0).contains(&address) {
            let address = (address & 0xff) as usize;
            if sm.device_mode[SM_DEV_RAM_DIR] & (1 << (address >> 5)) != 0 {
                sm.access[address >> 3] &= !(1 << (address & 7));
            }
            sm.shared_ram[address]
        } else {
            0
        }
    }

    fn sm_write(&mut self, address: u16, data: u8) {
        let address = address & 0x1fff;
        if address < 0x80 {
            self.sm.ram[address as usize] = data;
        } else if (0xe0..0x100).contains(&address) {
            let address = (address & 0x1f) as usize;
            let mode = &mut self.sm.device_mode;
            match address {
                SM_DEV_P1_DATA => self.p1_data = data,
                SM_DEV_P1_DIR => self.sm.p1_dir = data,
                SM_DEV_INT_REQUEST => mode[SM_DEV_INT_REQUEST] &= data,
                SM_DEV_COLLISION => {
                    mode[SM_DEV_COLLISION] &= !0x7f;
                    mode[SM_DEV_COLLISION] |= data & 0x7f;
                    if data & 0x80 == 0 {
                        mode[SM_DEV_COLLISION] &= !0x80;
                    }
                }
                _ => mode[address] = data,
            }
            if address == SM_DEV_UART3_MODE_STATUS || address == SM_DEV_UART3_CTRL {
                let mode = &self.sm.device_mode;
                let value = mode[SM_DEV_UART3_MODE_STATUS] & 0x80 != 0 && mode[SM_DEV_UART3_CTRL] & 0x20 == 0;
                self.ga_set_int(5, value);
            }
        } else if (0x200..0x2c0).contains(&address) {
            let address = (address & 0xff) as usize;
            self.sm.access[address >> 3] |= 1 << (address & 7);
            self.sm.shared_ram[address] = data;
        }
    }

    /// The main processor writing the sub-MCU's side of things.
    pub(super) fn sm_sys_write(&mut self, address: u32, data: u8) {
        let address = (address & 0xff) as usize;
        let sm = &mut self.sm;
        if address < 0xc0 {
            sm.access[address >> 3] |= 1 << (address & 7);
            sm.shared_ram[address] = data;
        } else if (0xf8..0xfc).contains(&address) {
            sm.device_mode[SM_DEV_IPCM0 + (address & 3)] = data;
            if address & 3 == 0 {
                sm.device_mode[SM_DEV_INT_REQUEST] |= 0x10;
                sm.device_mode[SM_DEV_SEMAPHORE] &= !0x80;
            }
        } else if address == 0xff {
            sm.device_mode[SM_DEV_SEMAPHORE] &= !0x1f;
            sm.device_mode[SM_DEV_SEMAPHORE] |= data & 0x1f;
        } else if address == 0xf5 {
            self.p1_data = data;
        } else if address == 0xf6 {
            self.p0_data = data;
        } else if address == 0xf7 {
            sm.p0_dir = data;
        }
    }

    pub(super) fn sm_sys_read(&mut self, address: u32) -> u8 {
        let address = (address & 0xff) as usize;
        let sm = &mut self.sm;
        if address < 0xc0 {
            if sm.device_mode[SM_DEV_RAM_DIR] & (1 << (address >> 5)) == 0 {
                sm.access[address >> 3] &= !(1 << (address & 7));
            }
            sm.shared_ram[address]
        } else if (0xf8..0xfc).contains(&address) {
            if address & 3 == 0 {
                sm.device_mode[SM_DEV_INT_REQUEST] |= 0x10;
            }
            let val = sm.device_mode[SM_DEV_IPCE0 + (address & 3)];
            sm.device_mode[SM_DEV_IPCE0 + (address & 3)] = 0;
            val
        } else if address == 0xff {
            sm.device_mode[SM_DEV_SEMAPHORE]
        } else if address == 0xf5 {
            self.read_p1()
        } else if address == 0xf6 {
            self.read_p0()
        } else if address == 0xf7 {
            sm.p0_dir
        } else {
            0
        }
    }

    fn sm_vector_address(&mut self, vector: u16) -> u16 {
        let low = self.sm_read(0x1fec + vector * 2) as u16;
        low | (self.sm_read(0x1fec + vector * 2 + 1) as u16) << 8
    }

    pub(super) fn sm_reset(&mut self) {
        self.sm.pc = self.sm_vector_address(SM_VECTOR_RESET);
        let sm = &mut self.sm;
        sm.a = 0;
        sm.x = 0;
        sm.y = 0;
        sm.s = 0;
        sm.sr = 0;
        sm.cycles = 0;
        sm.sleep = false;
    }

    fn sm_advance(&mut self) -> u8 {
        let byte = self.sm_read(self.sm.pc);
        self.sm.pc = self.sm.pc.wrapping_add(1);
        byte
    }

    fn sm_advance16(&mut self) -> u16 {
        let low = self.sm_advance() as u16;
        low | (self.sm_advance() as u16) << 8
    }

    /// As Nuked-SC55 has it: the byte at `address`, twice.
    fn sm_read16(&mut self, address: u16) -> u16 {
        let low = self.sm_read(address) as u16;
        low | (self.sm_read(address) as u16) << 8
    }

    fn sm_push(&mut self, data: u8) {
        self.sm_write(self.sm.s as u16, data);
        self.sm.s = self.sm.s.wrapping_sub(1);
    }

    fn sm_pop(&mut self) -> u8 {
        self.sm.s = self.sm.s.wrapping_add(1);
        self.sm_read(self.sm.s as u16)
    }

    /// The operand address of the zero page and absolute modes the
    /// arithmetic instructions share: (zp), zp, zp+X, abs, abs+X, abs+Y,
    /// (zp+X), (zp)+Y. `None` for an immediate.
    fn sm_operand_address(&mut self, mode: u8) -> Option<u16> {
        let (x, y) = (self.sm.x as u16, self.sm.y as u16);
        Some(match mode {
            0 => return None,
            1 => self.sm_advance() as u16,
            2 => (self.sm_advance() as u16 + x) & 0xff,
            3 => self.sm_advance16(),
            4 => self.sm_advance16().wrapping_add(x),
            5 => self.sm_advance16().wrapping_add(y),
            6 => {
                let zp = (self.sm_advance() as u16 + x) & 0xff;
                self.sm_read16(zp)
            }
            _ => {
                let zp = self.sm_advance() as u16;
                self.sm_read16(zp).wrapping_add(y)
            }
        })
    }

    fn sm_operand(&mut self, mode: u8) -> u8 {
        match self.sm_operand_address(mode) {
            Some(address) => self.sm_read(address),
            None => self.sm_advance(),
        }
    }

    fn sm_branch(&mut self, condition: bool) {
        let diff = self.sm_advance() as i8;
        if condition {
            self.sm.pc = self.sm.pc.wrapping_add(diff as u16);
        }
    }

    fn sm_compare(&mut self, register: u8, operand: u8) {
        let diff = register as i32 - operand as i32;
        self.sm.set_status(diff & 0x100 == 0, SM_STATUS_C);
        self.sm.update_nz(diff as u8);
    }

    /// ORA and AND: on the accumulator, or with the T flag set on the
    /// zero page byte X points at.
    fn sm_logic(&mut self, mode: u8, and: bool) {
        let t = self.sm.sr & SM_STATUS_T != 0;
        let val = if t { self.sm_read(self.sm.x as u16) } else { self.sm.a };
        let val2 = self.sm_operand(mode);
        let val = if and { val & val2 } else { val | val2 };
        if t {
            self.sm_write(self.sm.x as u16, val);
        } else {
            self.sm.a = val;
            self.sm.update_nz(val);
        }
    }

    fn sm_step_opcode(&mut self, opcode: u8) {
        // The addressing modes of the ORA/AND/LDA/CMP groups by the low
        // bits of the opcode.
        let group_mode = |op: u8| -> Option<u8> {
            Some(match op & 0x1f {
                0x09 => 0,
                0x05 => 1,
                0x15 => 2,
                0x0d => 3,
                0x1d => 4,
                0x19 => 5,
                0x01 => 6,
                0x11 => 7,
                _ => return None,
            })
        };
        match opcode {
            // ORA
            0x01 | 0x05 | 0x09 | 0x0d | 0x11 | 0x15 | 0x19 | 0x1d => {
                self.sm_logic(group_mode(opcode).unwrap(), false)
            }
            0x21 | 0x25 | 0x29 | 0x2d | 0x31 | 0x35 | 0x39 | 0x3d => {
                self.sm_logic(group_mode(opcode).unwrap(), true)
            }
            0x02 | 0x20 | 0x22 => {
                // JSR
                let newpc = match opcode {
                    0x20 => self.sm_advance16(),
                    0x02 => {
                        let zp = self.sm_advance() as u16;
                        self.sm_read16(zp)
                    }
                    _ => 0xff00 | self.sm_advance() as u16,
                };
                self.sm_push((self.sm.pc >> 8) as u8);
                self.sm_push(self.sm.pc as u8);
                self.sm.pc = newpc;
            }
            0x10 => self.sm_branch(self.sm.sr & SM_STATUS_N == 0),
            0x12 => self.sm.set_status(false, SM_STATUS_T),
            0x18 => self.sm.set_status(false, SM_STATUS_C),
            0x1a => {
                self.sm.a = self.sm.a.wrapping_sub(1);
                self.sm.update_nz(self.sm.a);
            }
            0x3a => {
                self.sm.a = self.sm.a.wrapping_add(1);
                self.sm.update_nz(self.sm.a);
            }
            0xc6 | 0xd6 | 0xce | 0xde | 0xe6 | 0xf6 | 0xee | 0xfe => {
                // DEC, INC
                let dest = match opcode & 0x18 {
                    0x00 => self.sm_advance() as u16,
                    0x10 => (self.sm_advance() as u16 + self.sm.x as u16) & 0xff,
                    0x08 => self.sm_advance16(),
                    _ => self.sm_advance16().wrapping_add(self.sm.x as u16),
                };
                let val = self.sm_read(dest);
                let val = if opcode < 0xe0 { val.wrapping_sub(1) } else { val.wrapping_add(1) };
                self.sm_write(dest, val);
                self.sm.update_nz(val);
            }
            0x38 => self.sm.set_status(true, SM_STATUS_C),
            0x3c => {
                // LDM
                let val = self.sm_advance();
                let dest = self.sm_advance() as u16;
                self.sm_write(dest, val);
            }
            0x40 => {
                // RTI
                self.sm.sr = self.sm_pop();
                let low = self.sm_pop() as u16;
                self.sm.pc = low | (self.sm_pop() as u16) << 8;
            }
            0x42 => self.sm.sleep = true,
            0x48 => self.sm_push(self.sm.a),
            0x4c => self.sm.pc = self.sm_advance16(),
            0x6c => {
                let address = self.sm_advance16();
                self.sm.pc = self.sm_read16(address);
            }
            0xb2 => {
                let zp = self.sm_advance() as u16;
                self.sm.pc = self.sm_read16(zp);
            }
            0x58 => self.sm.set_status(false, SM_STATUS_I),
            0x60 => {
                // RTS
                let low = self.sm_pop() as u16;
                self.sm.pc = low | (self.sm_pop() as u16) << 8;
            }
            0x68 => {
                self.sm.a = self.sm_pop();
                self.sm.update_nz(self.sm.a);
            }
            0x78 => self.sm.set_status(true, SM_STATUS_I),
            0x80 => self.sm_branch(true),
            0x81 | 0x85 | 0x8d | 0x91 | 0x95 | 0x99 | 0x9d => {
                // STA
                let (x, y) = (self.sm.x as u16, self.sm.y as u16);
                let dest = match opcode {
                    0x85 => self.sm_advance() as u16,
                    0x95 => self.sm_advance() as u16 + x,
                    0x8d => self.sm_advance16(),
                    0x9d => self.sm_advance16().wrapping_add(x),
                    0x99 => self.sm_advance16().wrapping_add(y),
                    0x81 => {
                        let zp = (self.sm_advance() as u16 + x) & 0xff;
                        self.sm_read16(zp)
                    }
                    _ => {
                        let zp = self.sm_advance() as u16;
                        self.sm_read16(zp).wrapping_add(y)
                    }
                };
                self.sm_write(dest, self.sm.a);
            }
            0x84 | 0x8c | 0x94 => {
                // STY
                let dest = match opcode {
                    0x84 => self.sm_advance() as u16,
                    0x94 => (self.sm_advance() as u16 + self.sm.x as u16) & 0xff,
                    _ => self.sm_advance16(),
                };
                self.sm_write(dest, self.sm.y);
            }
            0x86 | 0x8e | 0x96 => {
                // STX
                let dest = match opcode {
                    0x86 => self.sm_advance() as u16,
                    0x96 => self.sm_advance() as u16 + self.sm.x as u16,
                    _ => self.sm_advance16(),
                };
                self.sm_write(dest, self.sm.x);
            }
            0x8a => {
                self.sm.a = self.sm.x;
                self.sm.update_nz(self.sm.a);
            }
            0x90 => self.sm_branch(self.sm.sr & SM_STATUS_C == 0),
            0x9a => self.sm.s = self.sm.x,
            0xa0 | 0xa4 | 0xac | 0xb4 | 0xbc => {
                // LDY
                let x = self.sm.x as u16;
                let val = match opcode {
                    0xa0 => self.sm_advance(),
                    0xa4 => {
                        let a = self.sm_advance() as u16;
                        self.sm_read(a)
                    }
                    0xac => {
                        let a = self.sm_advance16();
                        self.sm_read(a)
                    }
                    0xb4 => {
                        let a = (self.sm_advance() as u16 + x) & 0xff;
                        self.sm_read(a)
                    }
                    _ => {
                        let a = self.sm_advance16().wrapping_add(x);
                        self.sm_read(a)
                    }
                };
                self.sm.y = val;
                self.sm.update_nz(val);
            }
            0xa2 | 0xa6 | 0xae | 0xb6 | 0xbe => {
                // LDX
                let y = self.sm.y as u16;
                let val = match opcode {
                    0xa2 => self.sm_advance(),
                    0xa6 => {
                        let a = self.sm_advance() as u16;
                        self.sm_read(a)
                    }
                    0xb6 => {
                        let a = (self.sm_advance() as u16 + y) & 0xff;
                        self.sm_read(a)
                    }
                    0xae => {
                        let a = self.sm_advance16();
                        self.sm_read(a)
                    }
                    _ => {
                        let a = self.sm_advance16().wrapping_add(y);
                        self.sm_read(a)
                    }
                };
                self.sm.x = val;
                self.sm.update_nz(val);
            }
            0xa1 | 0xa5 | 0xa9 | 0xad | 0xb1 | 0xb5 | 0xb9 | 0xbd => {
                // LDA
                let val = self.sm_operand(group_mode(opcode).unwrap());
                if self.sm.sr & SM_STATUS_T == 0 {
                    self.sm.a = val;
                    self.sm.update_nz(val);
                } else {
                    self.sm_write(self.sm.x as u16, val);
                }
            }
            0xaa => {
                self.sm.x = self.sm.a;
                self.sm.update_nz(self.sm.x);
            }
            0xb0 => self.sm_branch(self.sm.sr & SM_STATUS_C != 0),
            0xc0 | 0xc4 | 0xcc | 0xe0 | 0xe4 | 0xec => {
                // CPY, CPX: immediate, zero page, absolute
                let mode = match opcode & 0x0f {
                    0x0 => 0,
                    0x4 => 1,
                    _ => 3,
                };
                let operand = self.sm_operand(mode);
                let register = if opcode < 0xe0 { self.sm.y } else { self.sm.x };
                self.sm_compare(register, operand);
            }
            0xc1 | 0xc5 | 0xc9 | 0xcd | 0xd1 | 0xd5 | 0xd9 | 0xdd => {
                let operand = self.sm_operand(group_mode(opcode).unwrap());
                self.sm_compare(self.sm.a, operand);
            }
            0xc8 => {
                self.sm.y = self.sm.y.wrapping_add(1);
                self.sm.update_nz(self.sm.y);
            }
            0xd0 => self.sm_branch(self.sm.sr & SM_STATUS_Z == 0),
            0xd8 => self.sm.set_status(false, SM_STATUS_D),
            0xe8 => {
                self.sm.x = self.sm.x.wrapping_add(1);
                self.sm.update_nz(self.sm.x);
            }
            0xea => {}
            0xf0 => self.sm_branch(self.sm.sr & SM_STATUS_Z != 0),
            _ if opcode & 0x0f == 0x03 || opcode & 0x0f == 0x07 => {
                // BBC, BBS
                let zp = opcode & 4 != 0;
                let bit = (opcode >> 5) & 7;
                let kind = (opcode >> 4) & 1;
                let val = if zp {
                    let a = self.sm_advance() as u16;
                    self.sm_read(a)
                } else {
                    self.sm.a
                };
                let diff = self.sm_advance() as i8;
                if (val >> bit) & 1 != kind {
                    self.sm.pc = self.sm.pc.wrapping_add(diff as u16);
                }
            }
            _ if opcode & 0x0f == 0x0b || opcode & 0x0f == 0x0f => {
                // SEB, CLB
                let zp = opcode & 4 != 0;
                let bit = (opcode >> 5) & 7;
                let clear = (opcode >> 4) & 1 != 0;
                let dest = if zp { self.sm_advance() } else { 0 };
                let mut val = if zp { self.sm_read(dest as u16) } else { self.sm.a };
                if clear {
                    val &= !(1 << bit);
                } else {
                    val |= 1 << bit;
                }
                if zp {
                    self.sm_write(dest as u16, val);
                } else {
                    self.sm.a = val;
                }
            }
            _ => {}
        }
    }

    fn sm_start_vector(&mut self, vector: u16) {
        self.sm_push((self.sm.pc >> 8) as u8);
        self.sm_push(self.sm.pc as u8);
        self.sm_push(self.sm.sr);
        self.sm.sr |= SM_STATUS_I;
        self.sm.sleep = false;
        self.sm.pc = self.sm_vector_address(vector);
    }

    fn sm_handle_interrupt(&mut self) {
        let sm = &mut self.sm;
        if sm.sr & SM_STATUS_I != 0 {
            return;
        }
        let mode = &mut sm.device_mode;
        let enabled = |mode: &[u8; 32], bit: u8| {
            mode[SM_DEV_INT_ENABLE] & bit != 0 && mode[SM_DEV_INT_REQUEST] & bit != 0
        };
        let checks: [(bool, u8, u16); 5] = [
            (mode[SM_DEV_UART1_CTRL] & 0x8 != 0, 0x80, SM_VECTOR_UART1_RX),
            (mode[SM_DEV_UART2_CTRL] & 0x8 != 0, 0x40, SM_VECTOR_UART2_RX),
            (mode[SM_DEV_UART3_CTRL] & 0x8 != 0, 0x20, SM_VECTOR_UART3_RX),
            (mode[SM_DEV_TIMER_CTRL] & 0x80 != 0, 0x10, SM_VECTOR_IPCM0),
            (mode[SM_DEV_TIMER_CTRL] & 0x40 != 0, 0x8, SM_VECTOR_TIMER_X),
        ];
        for (on, bit, vector) in checks {
            if on && enabled(mode, bit) {
                mode[SM_DEV_INT_REQUEST] &= !bit;
                self.sm_start_vector(vector);
                return;
            }
        }
        if mode[SM_DEV_COLLISION] & 0xc0 == 0xc0 {
            mode[SM_DEV_COLLISION] &= !0x80;
            self.sm_start_vector(SM_VECTOR_COLLISION);
            return;
        }
        let cts = sm.cts;
        let checks: [(usize, u8, u8, u16); 3] = [
            (SM_DEV_UART1_CTRL, 1, 0x4, SM_VECTOR_UART1_TX),
            (SM_DEV_UART2_CTRL, 2, 0x2, SM_VECTOR_UART2_TX),
            (SM_DEV_UART3_CTRL, 4, 0x1, SM_VECTOR_UART3_TX),
        ];
        for (ctrl, cts_bit, bit, vector) in checks {
            let mode = &mut self.sm.device_mode;
            if (mode[ctrl] & 0x10 == 0 || cts & cts_bit != 0) && enabled(mode, bit) {
                mode[SM_DEV_INT_REQUEST] &= !bit;
                self.sm_start_vector(vector);
                return;
            }
        }
    }

    fn sm_update_timer(&mut self) {
        let sm = &mut self.sm;
        while sm.timer_cycles < sm.cycles {
            if sm.device_mode[SM_DEV_TIMER_CTRL] & 0x20 == 0 && !sm.sleep {
                if sm.timer_prescaler == 0 {
                    sm.timer_prescaler = sm.device_mode[SM_DEV_PRESCALER];
                    if sm.timer_counter == 0 {
                        sm.timer_counter = sm.device_mode[SM_DEV_TIMER];
                        sm.device_mode[SM_DEV_INT_REQUEST] |= 0x8;
                    } else {
                        sm.timer_counter -= 1;
                    }
                } else {
                    sm.timer_prescaler -= 1;
                }
            }
            sm.timer_cycles += 16;
        }
    }

    fn sm_update_uart(&mut self) {
        if self.sm.device_mode[SM_DEV_UART1_CTRL] & 4 == 0 || self.uart_write_ptr == self.uart_read_ptr {
            return;
        }
        if self.sm.uart_rx_gotbyte || self.sm.cycles < self.uart_rx_delay {
            return;
        }
        self.uart_rx_byte = self.uart_take();
        self.sm.uart_rx_gotbyte = true;
        self.sm.device_mode[SM_DEV_INT_REQUEST] |= 0x40;
        self.uart_rx_delay = self.sm.cycles + 3000 * 4;
    }

    /// Run the sub-MCU up to the main processor's `cycles`.
    pub(super) fn sm_update(&mut self) {
        let target = self.cycles * 5;
        while self.sm.cycles < target {
            self.sm_handle_interrupt();
            if !self.sm.sleep {
                let opcode = self.sm_advance();
                self.sm_step_opcode(opcode);
            }
            self.sm.cycles += 12 * 4;
            self.sm_update_timer();
            self.sm_update_uart();
        }
    }
}
