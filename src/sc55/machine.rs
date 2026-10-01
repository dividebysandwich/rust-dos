//! The module's main processor, a Hitachi H8/532, with its memory map,
//! on-chip devices and the chips around it: the sub-MCU and the PCM chip.
//! One struct holds them all, as they reach into each other. The LCD's
//! controller isn't kept: nothing it does is heard.
//! A port of Nuked-SC55's `mcu.cpp` and `mcu_interrupt.cpp`.

use super::pcm::Pcm;
use super::rom::{Family, Loaded, Location};
use super::submcu::SubMcu;
use super::timer::Timer;

// On-chip registers, as offsets from FF80h.
pub(super) const DEV_P1DDR: usize = 0x00;
pub(super) const DEV_P1DR: usize = 0x02;
pub(super) const DEV_P2DDR: usize = 0x01;
pub(super) const DEV_P2DR: usize = 0x03;
pub(super) const DEV_P3DDR: usize = 0x04;
pub(super) const DEV_P4DDR: usize = 0x05;
pub(super) const DEV_P3DR: usize = 0x06;
pub(super) const DEV_P4DR: usize = 0x07;
pub(super) const DEV_P5DDR: usize = 0x08;
pub(super) const DEV_P6DDR: usize = 0x09;
pub(super) const DEV_P5DR: usize = 0x0a;
pub(super) const DEV_P6DR: usize = 0x0b;
pub(super) const DEV_P7DDR: usize = 0x0c;
pub(super) const DEV_P7DR: usize = 0x0e;
pub(super) const DEV_P8DR: usize = 0x0f;
pub(super) const DEV_TMR_TCR: usize = 0x50;
pub(super) const DEV_TMR_TCSR: usize = 0x51;
pub(super) const DEV_TMR_TCORA: usize = 0x52;
pub(super) const DEV_TMR_TCORB: usize = 0x53;
pub(super) const DEV_TMR_TCNT: usize = 0x54;
pub(super) const DEV_SMR: usize = 0x58;
pub(super) const DEV_BRR: usize = 0x59;
pub(super) const DEV_SCR: usize = 0x5a;
pub(super) const DEV_TDR: usize = 0x5b;
pub(super) const DEV_SSR: usize = 0x5c;
pub(super) const DEV_RDR: usize = 0x5d;
pub(super) const DEV_ADDRAH: usize = 0x60;
pub(super) const DEV_ADDRAL: usize = 0x61;
pub(super) const DEV_ADDRDL: usize = 0x67;
pub(super) const DEV_ADCSR: usize = 0x68;
pub(super) const DEV_IPRA: usize = 0x70;
pub(super) const DEV_IPRB: usize = 0x71;
pub(super) const DEV_IPRC: usize = 0x72;
pub(super) const DEV_IPRD: usize = 0x73;
pub(super) const DEV_DTED: usize = 0x77;
pub(super) const DEV_WCR: usize = 0x78;
pub(super) const DEV_RAMCR: usize = 0x79;
pub(super) const DEV_P1CR: usize = 0x7c;
pub(super) const DEV_P9DDR: usize = 0x7e;
pub(super) const DEV_P9DR: usize = 0x7f;

pub(super) const SR_MASK: u16 = 0x870f;
pub(super) const STATUS_T: u16 = 0x8000;
pub(super) const STATUS_N: u16 = 0x08;
pub(super) const STATUS_Z: u16 = 0x04;
pub(super) const STATUS_V: u16 = 0x02;
pub(super) const STATUS_C: u16 = 0x01;
pub(super) const STATUS_INT_MASK: u16 = 0x700;

// Exception vectors.
const VECTOR_RESET: u32 = 0;
const VECTOR_INVALID_INSTRUCTION: u32 = 2;
const VECTOR_ADDRESS_ERROR: u32 = 8;
const VECTOR_TRACE: u32 = 9;
const VECTOR_NMI: u32 = 11;
const VECTOR_TRAPA_0: u32 = 16;
const VECTOR_IRQ0: u32 = 32;
const VECTOR_IRQ1: u32 = 33;

// Interrupt sources, in priority order for equal levels.
pub(super) const INT_NMI: u32 = 0;
pub(super) const INT_IRQ0: u32 = 1;
pub(super) const INT_IRQ1: u32 = 2;
pub(super) const INT_FRT0_OCIA: u32 = 4;
pub(super) const INT_FRT0_FOVI: u32 = 6;
pub(super) const INT_TIMER_CMIA: u32 = 15;
pub(super) const INT_TIMER_CMIB: u32 = 16;
pub(super) const INT_TIMER_OVI: u32 = 17;
pub(super) const INT_ANALOG: u32 = 18;
pub(super) const INT_UART_RX: u32 = 19;
pub(super) const INT_UART_TX: u32 = 20;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Exception {
    AddressError,
    InvalidInstruction,
    Trace,
}

/// Set or clear an interrupt request in a set of them.
#[inline]
pub(super) fn set_request(pending: &mut u32, source: u32, value: bool) {
    if value {
        *pending |= 1 << source;
    } else {
        *pending &= !(1 << source);
    }
}

const UART_BUFFER_SIZE: usize = 8192;

pub(super) struct Machine {
    pub r: [u16; 8],
    pub pc: u16,
    pub sr: u16,
    pub cp: u8,
    pub dp: u8,
    pub ep: u8,
    pub tp: u8,
    pub br: u8,
    pub sleep: bool,
    pub ex_ignore: bool,
    pub exception_pending: Option<Exception>,
    pub interrupt_pending: u32,
    pub trapa_pending: u16,
    pub cycles: u64,

    pub rom1: Box<[u8]>,
    pub rom2: Box<[u8]>,
    pub ram: Box<[u8]>,
    pub sram: Box<[u8]>,
    pub rom2_mask: u32,

    pub dev_register: [u8; 0x80],
    pub sw_pos: u8,
    pub io_sd: u8,

    pub uart_write_ptr: usize,
    pub uart_read_ptr: usize,
    pub uart_buffer: Box<[u8]>,
    pub uart_rx_byte: u8,
    pub uart_rx_delay: u64,
    pub uart_tx_delay: u64,

    pub is_mk1: bool,
    pub is_cm300: bool,
    pub is_scb55: bool,
    pub is_sc155: bool,

    pub ga_int: [bool; 8],
    pub ga_int_enable: u8,
    pub ga_int_trigger: u8,
    pub ga_lcd_counter: i32,

    pub p0_data: u8,
    pub p1_data: u8,
    pub adf_rd: bool,
    pub analog_end_time: u64,
    pub ssr_rd: u8,

    // The operand of the instruction being carried out.
    pub operand_type: u8,
    pub operand_ea: u16,
    pub operand_ep: u8,
    pub operand_word: bool,
    pub operand_reg: u8,
    pub operand_data: u16,
    pub opcode_extended: bool,

    pub timer: Timer,
    pub sm: SubMcu,
    pub pcm: Box<Pcm>,

    /// Frames the PCM chip made since they were last taken.
    pub samples: Vec<[i32; 2]>,
}

impl Machine {
    /// A module with `roms`, before reset.
    pub fn new(roms: &Loaded, oversampling: bool) -> Machine {
        let family = roms.romset.family;
        let mut rom1 = vec![0u8; 0x8000];
        let src = roms.get(Location::Rom1);
        rom1[..src.len()].copy_from_slice(src);
        let mut rom2 = vec![0u8; 0x80000];
        let src = roms.get(Location::Rom2);
        rom2[..src.len()].copy_from_slice(src);
        let rom2_mask = (src.len().max(1) - 1) as u32;
        let mut machine = Machine {
            r: [0; 8],
            pc: 0,
            sr: 0,
            cp: 0,
            dp: 0,
            ep: 0,
            tp: 0,
            br: 0,
            sleep: false,
            ex_ignore: false,
            exception_pending: None,
            interrupt_pending: 0,
            trapa_pending: 0,
            cycles: 0,
            rom1: rom1.into(),
            rom2: rom2.into(),
            ram: vec![0; 0x400].into(),
            sram: vec![0; 0x8000].into(),
            rom2_mask,
            dev_register: [0; 0x80],
            sw_pos: 3,
            io_sd: 0,
            uart_write_ptr: 0,
            uart_read_ptr: 0,
            uart_buffer: vec![0; UART_BUFFER_SIZE].into(),
            uart_rx_byte: 0,
            uart_rx_delay: 0,
            uart_tx_delay: 0,
            is_mk1: family.is_mk1(),
            is_cm300: family == Family::Cm300,
            is_scb55: matches!(family, Family::Scb55 | Family::Rlp3237),
            is_sc155: matches!(family, Family::Sc155 | Family::Sc155Mk2),
            ga_int: [false; 8],
            ga_int_enable: 0,
            ga_int_trigger: 0,
            ga_lcd_counter: 0,
            p0_data: 0,
            p1_data: 0,
            adf_rd: false,
            analog_end_time: 0,
            ssr_rd: 0,
            operand_type: 0,
            operand_ea: 0,
            operand_ep: 0,
            operand_word: false,
            operand_reg: 0,
            operand_data: 0,
            opcode_extended: false,
            timer: Timer::new(family.is_mk1()),
            sm: SubMcu::new(roms.get(Location::SmRom)),
            pcm: Pcm::new(roms, oversampling),
            samples: Vec::with_capacity(64),
        };
        machine.reset();
        machine.sm_reset();
        machine
    }

    pub fn reset(&mut self) {
        self.r = [0; 8];
        self.pc = 0;
        self.sr = 0x700;
        self.cp = 0;
        self.dp = 0;
        self.ep = 0;
        self.tp = 0;
        self.br = 0;
        let reset_address = self.vector_address(VECTOR_RESET);
        self.cp = (reset_address >> 16) as u8;
        self.pc = reset_address as u16;
        self.exception_pending = None;
        self.device_reset();
        if self.is_mk1 {
            self.ga_int_enable = 255;
        }
    }

    /// A MIDI byte into the serial port's buffer.
    pub fn post_uart(&mut self, data: u8) {
        self.uart_buffer[self.uart_write_ptr] = data;
        self.uart_write_ptr = (self.uart_write_ptr + 1) % UART_BUFFER_SIZE;
    }

    /// Bytes waiting in the serial port's buffer.
    pub fn uart_pending(&self) -> usize {
        (self.uart_write_ptr + UART_BUFFER_SIZE - self.uart_read_ptr) % UART_BUFFER_SIZE
    }

    pub(super) fn uart_take(&mut self) -> u8 {
        let byte = self.uart_buffer[self.uart_read_ptr];
        self.uart_read_ptr = (self.uart_read_ptr + 1) % UART_BUFFER_SIZE;
        byte
    }

    // -----------------------------------------------------------------------
    // Memory.

    #[inline]
    pub fn get_address(page: u8, address: u16) -> u32 {
        ((page as u32) << 16) + address as u32
    }

    pub fn vector_address(&mut self, vector: u32) -> u32 {
        self.read32(vector * 4)
    }

    pub fn read(&mut self, address: u32) -> u8 {
        let mut address_rom = address & 0x3ffff;
        if address & 0x80000 != 0 {
            address_rom |= 0x40000;
        }
        let page = (address >> 16) & 0xf;
        let address = address & 0xffff;
        match page {
            0 => {
                if address & 0x8000 == 0 {
                    self.rom1[(address & 0x7fff) as usize]
                } else if !self.is_mk1 {
                    let base = 0xe000u32;
                    if (base..base | 0x400).contains(&address) {
                        self.pcm_read(address & 0x3f)
                    } else if !self.is_scb55 && (0xec00..0xf000).contains(&address) {
                        self.sm_sys_read(address & 0xff)
                    } else if address >= 0xff80 {
                        self.device_read((address & 0x7f) as usize)
                    } else if (0xfb80..0xff80).contains(&address) && self.dev_register[DEV_RAMCR] & 0x80 != 0 {
                        self.ram[((address - 0xfb80) & 0x3ff) as usize]
                    } else if (0x8000..0xe000).contains(&address) {
                        self.sram[(address & 0x7fff) as usize]
                    } else if address == base | 0x402 {
                        let ret = self.ga_int_trigger;
                        self.ga_int_trigger = 0;
                        set_request(&mut self.interrupt_pending, INT_IRQ1, false);
                        ret
                    } else {
                        0xff
                    }
                } else if (0xe000..0xe040).contains(&address) {
                    self.pcm_read(address & 0x3f)
                } else if address >= 0xff80 {
                    self.device_read((address & 0x7f) as usize)
                } else if (0xfb80..0xff80).contains(&address) && self.dev_register[DEV_RAMCR] & 0x80 != 0 {
                    self.ram[((address - 0xfb80) & 0x3ff) as usize]
                } else if (0x8000..0xe000).contains(&address) {
                    self.sram[(address & 0x7fff) as usize]
                } else if (0xf000..0xf100).contains(&address) {
                    // The buttons' rows; none is ever pressed.
                    self.io_sd = address as u8;
                    0xff
                } else if address == 0xf106 {
                    let ret = self.ga_int_trigger;
                    self.ga_int_trigger = 0;
                    set_request(&mut self.interrupt_pending, INT_IRQ1, false);
                    ret
                } else {
                    0xff
                }
            }
            1..=4 | 8 | 9 | 14 | 15 => self.rom2[(address_rom & self.rom2_mask) as usize],
            10 | 11 if !self.is_mk1 => self.sram[(address & 0x7fff) as usize],
            5 if self.is_mk1 => self.sram[(address & 0x7fff) as usize],
            5 | 10..=13 => 0xff,
            _ => 0x00,
        }
    }

    pub fn read16(&mut self, address: u32) -> u16 {
        let address = address & !1;
        let b0 = self.read(address);
        let b1 = self.read(address + 1);
        ((b0 as u16) << 8) | b1 as u16
    }

    pub fn read32(&mut self, address: u32) -> u32 {
        let address = address & !3;
        let b0 = self.read(address) as u32;
        let b1 = self.read(address + 1) as u32;
        let b2 = self.read(address + 2) as u32;
        let b3 = self.read(address + 3) as u32;
        (b0 << 24) | (b1 << 16) | (b2 << 8) | b3
    }

    pub fn write(&mut self, address: u32, value: u8) {
        let page = (address >> 16) & 0xf;
        let address = address & 0xffff;
        if page == 0 {
            if address & 0x8000 == 0 {
                return;
            }
            if !self.is_mk1 {
                let base = 0xe000u32;
                if (base | 0x400..base | 0x800).contains(&address) {
                    // E404h and E405h are the LCD's.
                    if address == base | 0x401 {
                        self.io_sd = value;
                    } else if address == base | 0x402 {
                        self.ga_int_enable = value << 1;
                    }
                } else if (base..base | 0x400).contains(&address) {
                    self.pcm_write(address & 0x3f, value);
                } else if !self.is_scb55 && (0xec00..0xf000).contains(&address) {
                    self.sm_sys_write(address & 0xff, value);
                } else if address >= 0xff80 {
                    self.device_write((address & 0x7f) as usize, value);
                } else if (0xfb80..0xff80).contains(&address) && self.dev_register[DEV_RAMCR] & 0x80 != 0 {
                    self.ram[((address - 0xfb80) & 0x3ff) as usize] = value;
                } else if (0x8000..0xe000).contains(&address) {
                    self.sram[(address & 0x7fff) as usize] = value;
                }
            } else if (0xe000..0xe040).contains(&address) {
                self.pcm_write(address & 0x3f, value);
            } else if address >= 0xff80 {
                self.device_write((address & 0x7f) as usize, value);
            } else if (0xfb80..0xff80).contains(&address) && self.dev_register[DEV_RAMCR] & 0x80 != 0 {
                self.ram[((address - 0xfb80) & 0x3ff) as usize] = value;
            } else if (0x8000..0xe000).contains(&address) {
                self.sram[(address & 0x7fff) as usize] = value;
            } else if (0xf000..0xf100).contains(&address) {
                self.io_sd = address as u8;
            } else if address == 0xf104 || address == 0xf105 {
                // The LCD's; the gate array interrupts when it is done.
                self.ga_lcd_counter = 500;
            } else if address == 0xf107 {
                self.io_sd = value;
            }
        } else if (page == 5 && self.is_mk1) || (page == 10 && !self.is_mk1) {
            self.sram[(address & 0x7fff) as usize] = value;
        }
    }

    pub fn write16(&mut self, address: u32, value: u16) {
        let address = address & !1;
        self.write(address, (value >> 8) as u8);
        self.write(address + 1, value as u8);
    }

    #[inline]
    pub fn read_code(&mut self) -> u8 {
        let byte = self.read(Self::get_address(self.cp, self.pc));
        self.pc = self.pc.wrapping_add(1);
        byte
    }

    // -----------------------------------------------------------------------
    // On-chip devices.

    fn analog_read_pin(&self, pin: u32) -> u16 {
        const BATTERY: u16 = 0x2a0;
        const SW: [u16; 4] = [0, 0x155, 0x2aa, 0x3ff];
        // The remote control unit and the SC-155's sliders read 0.
        if self.is_cm300 {
            return 0;
        }
        if self.is_mk1 {
            if self.is_sc155 && self.dev_register[DEV_P9DR] & 1 != 0 {
                return 0;
            }
            if pin == 7 {
                if self.is_sc155 && self.dev_register[DEV_P9DR] & 2 != 0 { 0 } else { BATTERY }
            } else {
                0
            }
        } else {
            if self.is_sc155 && self.io_sd & 16 != 0 {
                return 0;
            }
            if pin == 7 {
                match (self.io_sd >> 2) & 3 {
                    0 => BATTERY,
                    2 => SW[self.sw_pos as usize & 3],
                    _ => 0,
                }
            } else {
                0
            }
        }
    }

    fn analog_sample(&mut self, channel: u8) {
        let value = self.analog_read_pin(channel as u32);
        let dest = ((channel as usize) << 1) & 6;
        self.dev_register[DEV_ADDRAH + dest] = (value >> 2) as u8;
        self.dev_register[DEV_ADDRAL + dest] = ((value << 6) & 0xc0) as u8;
    }

    fn device_write(&mut self, address: usize, data: u8) {
        if (0x10..0x40).contains(&address) {
            self.timer.write_frt(address, data, &mut self.interrupt_pending);
            return;
        }
        if (0x50..0x55).contains(&address) {
            self.timer.write_tmr(address, data, &mut self.interrupt_pending);
            return;
        }
        match address {
            DEV_ADCSR => {
                self.dev_register[address] &= !0x7f;
                self.dev_register[address] |= data & 0x7f;
                if data & 0x80 == 0 && self.adf_rd {
                    self.dev_register[address] &= !0x80;
                    set_request(&mut self.interrupt_pending, INT_ANALOG, false);
                }
                if data & 0x40 == 0 {
                    set_request(&mut self.interrupt_pending, INT_ANALOG, false);
                }
                return;
            }
            DEV_SSR => {
                if data & 0x80 == 0 && self.ssr_rd & 0x80 != 0 {
                    self.dev_register[address] &= !0x80;
                    self.uart_tx_delay = self.cycles + 3000;
                    set_request(&mut self.interrupt_pending, INT_UART_TX, false);
                }
                if data & 0x40 == 0 && self.ssr_rd & 0x40 != 0 {
                    self.uart_rx_delay = self.cycles + 3000;
                    self.dev_register[address] &= !0x40;
                    set_request(&mut self.interrupt_pending, INT_UART_RX, false);
                }
                if data & 0x20 == 0 && self.ssr_rd & 0x20 != 0 {
                    self.dev_register[address] &= !0x20;
                }
                if data & 0x10 == 0 && self.ssr_rd & 0x10 != 0 {
                    self.dev_register[address] &= !0x10;
                }
            }
            _ => {}
        }
        self.dev_register[address] = data;
    }

    fn device_read(&mut self, address: usize) -> u8 {
        if (0x10..0x40).contains(&address) {
            return self.timer.read_frt(address);
        }
        if (0x50..0x55).contains(&address) {
            return self.timer.read_tmr(address);
        }
        match address {
            DEV_ADCSR => {
                self.adf_rd = self.dev_register[address] & 0x80 != 0;
                self.dev_register[address]
            }
            DEV_SSR => {
                self.ssr_rd = self.dev_register[address];
                self.dev_register[address]
            }
            DEV_RDR => self.uart_rx_byte,
            0x00 => 0xff,
            DEV_P7DR => 0xff,
            DEV_P9DR => {
                // Bit 1 tells the mkII from the SC-155mkII.
                let cfg = if self.is_mk1 || self.is_sc155 { 0 } else { 2 };
                let dir = self.dev_register[DEV_P9DDR];
                (cfg & !dir) | (self.dev_register[DEV_P9DR] & dir)
            }
            _ => self.dev_register[address],
        }
    }

    fn device_reset(&mut self) {
        let d = &mut self.dev_register;
        d[DEV_P1DDR] = 0x03;
        d[DEV_P1DR] = 0;
        d[DEV_P1CR] = 0x87;
        d[DEV_P2DDR] = 0xE0;
        d[DEV_P2DR] = 0xE0;
        d[DEV_P3DDR] = 0;
        d[DEV_P3DR] = 0;
        d[DEV_P4DDR] = 0;
        d[DEV_P4DR] = 0;
        d[DEV_P5DDR] = 0;
        d[DEV_P5DR] = 0;
        d[DEV_P6DDR] = 0xF0;
        d[DEV_P6DR] = 0xF0;
        d[DEV_P7DDR] = 0;
        d[DEV_P7DR] = 0;
        d[DEV_P8DR] = 0;
        d[DEV_P9DDR] = 0;
        d[DEV_P9DR] = 0;
        self.timer.reset();
        let d = &mut self.dev_register;
        d[DEV_RDR] = 0;
        d[DEV_TDR] = 0xFF;
        d[DEV_SMR] = 0x04;
        d[DEV_SCR] = 0x0C;
        d[DEV_SSR] = 0x87;
        d[DEV_BRR] = 0xFF;
        for r in &mut d[DEV_ADDRAH..=DEV_ADDRDL] {
            *r = 0;
        }
        d[DEV_ADCSR] = 0;
        for r in &mut d[DEV_IPRA..=DEV_DTED] {
            *r = 0;
        }
        d[DEV_WCR] = 0xF3;
        d[DEV_RAMCR] = 0x80;
    }

    fn update_analog(&mut self) {
        let cycles = self.cycles;
        let ctrl = self.dev_register[DEV_ADCSR];
        let isscan = ctrl & 16 != 0;
        if ctrl & 0x20 != 0 {
            if self.analog_end_time == 0 {
                self.analog_end_time = cycles + 200;
            } else if self.analog_end_time < cycles {
                if isscan {
                    let base = ctrl & 4;
                    for i in 0..=(ctrl & 3) {
                        self.analog_sample(base + i);
                    }
                    self.analog_end_time = cycles + 200;
                } else {
                    self.analog_sample(ctrl & 7);
                    self.dev_register[DEV_ADCSR] &= !0x20;
                    self.analog_end_time = 0;
                }
                self.dev_register[DEV_ADCSR] |= 0x80;
                if ctrl & 0x40 != 0 {
                    set_request(&mut self.interrupt_pending, INT_ANALOG, true);
                }
            }
        } else {
            self.analog_end_time = 0;
        }
    }

    fn update_uart_rx(&mut self) {
        if self.dev_register[DEV_SCR] & 16 == 0 || self.uart_write_ptr == self.uart_read_ptr {
            return;
        }
        if self.dev_register[DEV_SSR] & 0x40 != 0 || self.cycles < self.uart_rx_delay {
            return;
        }
        self.uart_rx_byte = self.uart_take();
        self.dev_register[DEV_SSR] |= 0x40;
        let enabled = self.dev_register[DEV_SCR] & 0x40 != 0;
        set_request(&mut self.interrupt_pending, INT_UART_RX, enabled);
    }

    fn update_uart_tx(&mut self) {
        if self.dev_register[DEV_SCR] & 32 == 0 || self.dev_register[DEV_SSR] & 0x80 != 0 {
            return;
        }
        if self.cycles < self.uart_tx_delay {
            return;
        }
        self.dev_register[DEV_SSR] |= 0x80;
        let enabled = self.dev_register[DEV_SCR] & 0x80 != 0;
        set_request(&mut self.interrupt_pending, INT_UART_TX, enabled);
    }

    // -----------------------------------------------------------------------
    // The gate array's interrupt lines and the ports the sub-MCU shares.

    pub fn ga_set_int(&mut self, line: usize, value: bool) {
        if value && !self.ga_int[line] && self.ga_int_enable & (1 << line) != 0 {
            self.ga_int_trigger = line as u8;
        }
        self.ga_int[line] = value;
        set_request(&mut self.interrupt_pending, INT_IRQ1, self.ga_int_trigger != 0);
    }

    pub fn read_p0(&self) -> u8 {
        0xff
    }

    /// The buttons, of which none is pressed.
    pub fn read_p1(&self) -> u8 {
        0xff
    }

    // -----------------------------------------------------------------------
    // Interrupts.

    pub fn push_stack(&mut self, data: u16) {
        if self.r[7] & 1 != 0 {
            self.exception_pending = Some(Exception::AddressError);
        }
        self.r[7] = self.r[7].wrapping_sub(2);
        self.write16(self.r[7] as u32, data);
    }

    pub fn pop_stack(&mut self) -> u16 {
        if self.r[7] & 1 != 0 {
            self.exception_pending = Some(Exception::AddressError);
        }
        let ret = self.read16(self.r[7] as u32);
        self.r[7] = self.r[7].wrapping_add(2);
        ret
    }

    fn interrupt_start_vector(&mut self, vector: u32, mask: i32) {
        let address = self.vector_address(vector);
        self.push_stack(self.pc);
        self.push_stack(self.cp as u16);
        self.push_stack(self.sr);
        self.sr &= !STATUS_T;
        if mask >= 0 {
            self.sr &= !STATUS_INT_MASK;
            self.sr |= (mask as u16) << 8;
        }
        self.sleep = false;
        self.cp = (address >> 16) as u8;
        self.pc = address as u16;
    }

    /// The vector and priority of an interrupt source, if it can
    /// interrupt.
    fn vector_level(&self, source: u32) -> Option<(u32, i32)> {
        let d = &self.dev_register;
        let (vector, level) = match source {
            INT_IRQ0 => {
                if d[DEV_P1CR] & 0x20 == 0 {
                    return None;
                }
                (VECTOR_IRQ0, (d[DEV_IPRA] >> 4) & 7)
            }
            INT_IRQ1 => {
                if d[DEV_P1CR] & 0x40 == 0 {
                    return None;
                }
                (VECTOR_IRQ1, d[DEV_IPRA] & 7)
            }
            // The free-running timers' compare and overflow interrupts:
            // vectors 37-39, 41-43, 45-47.
            4..=6 => (37 + source - 4, (d[DEV_IPRB] >> 4) & 7),
            8..=10 => (41 + source - 8, d[DEV_IPRB] & 7),
            12..=14 => (45 + source - 12, (d[DEV_IPRC] >> 4) & 7),
            INT_TIMER_CMIA => (48, d[DEV_IPRC] & 7),
            INT_TIMER_CMIB => (49, d[DEV_IPRC] & 7),
            INT_TIMER_OVI => (50, d[DEV_IPRC] & 7),
            INT_ANALOG => (56, d[DEV_IPRD] & 7),
            INT_UART_RX => (53, (d[DEV_IPRD] >> 4) & 7),
            INT_UART_TX => (54, (d[DEV_IPRD] >> 4) & 7),
            _ => return None,
        };
        Some((vector, level as i32))
    }

    fn handle_interrupt(&mut self) {
        if self.trapa_pending != 0 {
            let trap = self.trapa_pending.trailing_zeros();
            self.trapa_pending &= !(1 << trap);
            self.interrupt_start_vector(VECTOR_TRAPA_0 + trap, -1);
            return;
        }
        if let Some(exception) = self.exception_pending {
            let vector = match exception {
                Exception::AddressError => VECTOR_ADDRESS_ERROR,
                Exception::InvalidInstruction => VECTOR_INVALID_INSTRUCTION,
                Exception::Trace => VECTOR_TRACE,
            };
            self.interrupt_start_vector(vector, -1);
            // Even one the pushes raised.
            self.exception_pending = None;
            return;
        }
        if self.interrupt_pending & (1 << INT_NMI) != 0 {
            self.interrupt_start_vector(VECTOR_NMI, 7);
            return;
        }
        let mask = ((self.sr >> 8) & 7) as i32;
        let mut pending = self.interrupt_pending;
        while pending != 0 {
            let source = pending.trailing_zeros();
            pending &= pending - 1;
            if let Some((vector, level)) = self.vector_level(source)
                && mask < level
            {
                self.interrupt_start_vector(vector, level);
                return;
            }
        }
    }

    // -----------------------------------------------------------------------
    // Running.

    /// One instruction, and the devices for its time.
    pub fn step(&mut self) {
        if !self.ex_ignore {
            if self.trapa_pending != 0 || self.exception_pending.is_some() || self.interrupt_pending != 0 {
                self.handle_interrupt();
            }
        } else {
            self.ex_ignore = false;
        }

        if !self.sleep {
            let operand = self.read_code();
            self.execute(operand);
            if self.sr & STATUS_T != 0 {
                self.exception_pending = Some(Exception::Trace);
            }
        }

        // Every instruction is taken to be 12 cycles.
        self.cycles += 12;

        if self.pcm.cycles < self.cycles {
            self.pcm_update();
        }
        self.timer.clock(self.cycles, &mut self.interrupt_pending);

        if !self.is_mk1 && !self.is_scb55 {
            self.sm_update();
        } else {
            self.update_uart_rx();
            self.update_uart_tx();
        }

        self.update_analog();

        if self.is_mk1 && self.ga_lcd_counter != 0 {
            self.ga_lcd_counter -= 1;
            if self.ga_lcd_counter == 0 {
                self.ga_set_int(1, false);
                self.ga_set_int(1, true);
            }
        }
    }

    /// The registers and the PCM chip's memory, in the order the
    /// comparison harness prints Nuked-SC55's.
    pub fn debug_state(&self) -> Vec<i64> {
        let mut v: Vec<i64> = self.r.iter().map(|&r| r as i64).collect();
        v.extend([self.pc as i64, self.cp as i64, self.sr as i64, self.cycles as i64]);
        v.extend(self.pcm.debug_state());
        v
    }
}
