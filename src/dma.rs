//! The AT's two 8237 DMA controllers: channels 0-3 move bytes (ports
//! 00h-0Fh), channels 4-7 move words (ports C0h-DEh), with the page
//! registers at 80h-8Fh supplying the address bits above the controllers'
//! 16. Channel 4 cascades the first controller.
//!
//! Devices pull data through `transfer_read`: the channel's address and
//! count advance, and when the count runs out the channel reaches terminal
//! count, sets its status bit, and either reloads (auto-init) or masks
//! itself. Programs watch the live address and count to know how far a
//! sound card has played.

/// One channel's registers.
#[derive(Clone, Copy, Debug)]
pub struct DmaChannel {
    pub base_addr: u16,
    pub base_count: u16,
    pub cur_addr: u16,
    pub cur_count: u16,
    pub page: u8,
    /// Mode register: bits 2-3 transfer type, 4 auto-init, 5 address
    /// decrement, 6-7 mode.
    pub mode: u8,
    pub masked: bool,
}

impl Default for DmaChannel {
    fn default() -> Self {
        Self { base_addr: 0, base_count: 0, cur_addr: 0, cur_count: 0, page: 0, mode: 0, masked: true }
    }
}

impl DmaChannel {
    pub fn auto_init(&self) -> bool {
        self.mode & 0x10 != 0
    }

    fn decrement(&self) -> bool {
        self.mode & 0x20 != 0
    }
}

/// One 8237: four channels and the byte pointer flip-flop.
#[derive(Default)]
struct Controller {
    ch: [DmaChannel; 4],
    flipflop: bool,
    /// Terminal count reached (bits 0-3), cleared by reading the status.
    tc: u8,
    /// Software requests (bits 0-3).
    request: u8,
    command: u8,
    temp: u8,
}

impl Controller {
    fn write_reg(&mut self, reg: u8, value: u8) {
        match reg {
            0..=7 => {
                let ch = &mut self.ch[(reg >> 1) as usize];
                let (base, cur) = if reg & 1 == 0 {
                    (&mut ch.base_addr, &mut ch.cur_addr)
                } else {
                    (&mut ch.base_count, &mut ch.cur_count)
                };
                *base = if self.flipflop {
                    (*base & 0x00FF) | (value as u16) << 8
                } else {
                    (*base & 0xFF00) | value as u16
                };
                *cur = *base;
                self.flipflop = !self.flipflop;
            }
            8 => self.command = value,
            9 => {
                let bit = 1 << (value & 3);
                if value & 4 != 0 { self.request |= bit } else { self.request &= !bit }
            }
            10 => self.ch[(value & 3) as usize].masked = value & 4 != 0,
            11 => self.ch[(value & 3) as usize].mode = value,
            12 => self.flipflop = false,
            13 => {
                // Master clear: everything masked, flip-flop and status clear.
                self.flipflop = false;
                self.tc = 0;
                self.request = 0;
                self.command = 0;
                self.temp = 0;
                for ch in &mut self.ch {
                    ch.masked = true;
                }
            }
            14 => {
                for ch in &mut self.ch {
                    ch.masked = false;
                }
            }
            _ => {
                for (i, ch) in self.ch.iter_mut().enumerate() {
                    ch.masked = value & (1 << i) != 0;
                }
            }
        }
    }

    fn read_reg(&mut self, reg: u8) -> u8 {
        match reg {
            0..=7 => {
                let ch = &self.ch[(reg >> 1) as usize];
                let v = if reg & 1 == 0 { ch.cur_addr } else { ch.cur_count };
                let b = if self.flipflop { (v >> 8) as u8 } else { v as u8 };
                self.flipflop = !self.flipflop;
                b
            }
            8 => {
                // Status: terminal counts (cleared by the read) and requests.
                let v = self.tc | (self.request << 4);
                self.tc = 0;
                v
            }
            13 => self.temp,
            15 => self.ch.iter().enumerate().fold(0xF0, |m, (i, c)| m | ((c.masked as u8) << i)),
            _ => 0xFF,
        }
    }
}

/// Page register port of each channel.
const PAGE_PORTS: [u16; 8] = [0x87, 0x83, 0x81, 0x82, 0x8F, 0x8B, 0x89, 0x8A];

/// The two controllers.
#[derive(Default)]
pub struct Dma {
    ctrl: [Controller; 2],
    /// Page registers without a channel (80h, 84h-86h, 88h, 8Ch-8Eh):
    /// plain scratch bytes; BIOSes use 80h for POST codes.
    extra_pages: [u8; 16],
}

impl Dma {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the controllers own `port`.
    pub fn owns(port: u16) -> bool {
        matches!(port, 0x00..=0x0F | 0x81..=0x8F | 0xC0..=0xDF)
    }

    pub fn write(&mut self, port: u16, value: u8) {
        match port {
            0x00..=0x0F => self.ctrl[0].write_reg(port as u8, value),
            0xC0..=0xDF => {
                if port & 1 == 0 {
                    self.ctrl[1].write_reg(((port - 0xC0) >> 1) as u8, value);
                }
            }
            _ => match PAGE_PORTS.iter().position(|&p| p == port) {
                Some(ch) => self.channel_mut(ch).page = value,
                None => self.extra_pages[(port & 0xF) as usize] = value,
            },
        }
    }

    pub fn read(&mut self, port: u16) -> u8 {
        match port {
            0x00..=0x0F => self.ctrl[0].read_reg(port as u8),
            0xC0..=0xDF => {
                if port & 1 == 0 {
                    self.ctrl[1].read_reg(((port - 0xC0) >> 1) as u8)
                } else {
                    0xFF
                }
            }
            _ => match PAGE_PORTS.iter().position(|&p| p == port) {
                Some(ch) => self.channel(ch).page,
                None => self.extra_pages[(port & 0xF) as usize],
            },
        }
    }

    pub fn channel(&self, ch: usize) -> &DmaChannel {
        &self.ctrl[ch / 4].ch[ch % 4]
    }

    pub fn channel_mut(&mut self, ch: usize) -> &mut DmaChannel {
        &mut self.ctrl[ch / 4].ch[ch % 4]
    }

    /// Whether a device may transfer on `ch` now.
    pub fn ready(&self, ch: usize) -> bool {
        !self.channel(ch).masked
    }

    /// Read up to `buf.len()` bytes from memory on channel `ch` (4-7 move
    /// words, so `buf.len()` should be even), as a device reading a
    /// "memory to device" transfer does. Returns the number of bytes moved
    /// and whether the channel reached terminal count. A masked channel
    /// moves nothing.
    pub fn transfer_read(&mut self, ch: usize, ram: &[u8], buf: &mut [u8]) -> (usize, bool) {
        let words = ch >= 4;
        let unit = if words { 2 } else { 1 };
        let mut moved = 0;
        let mut tc = false;
        while moved + unit <= buf.len() {
            let c = self.channel_mut(ch);
            if c.masked {
                break;
            }
            let addr = if words {
                ((c.page as usize & 0xFE) << 16) | ((c.cur_addr as usize) << 1)
            } else {
                ((c.page as usize) << 16) | c.cur_addr as usize
            };
            for i in 0..unit {
                buf[moved + i] = ram.get(addr + i).copied().unwrap_or(0xFF);
            }
            moved += unit;
            c.cur_addr = if c.decrement() { c.cur_addr.wrapping_sub(1) } else { c.cur_addr.wrapping_add(1) };
            let (count, underflow) = c.cur_count.overflowing_sub(1);
            c.cur_count = count;
            if underflow {
                tc = true;
                if c.auto_init() {
                    c.cur_addr = c.base_addr;
                    c.cur_count = c.base_count;
                } else {
                    c.masked = true;
                }
                self.ctrl[ch / 4].tc |= 1 << (ch % 4);
                if !self.channel(ch).auto_init() {
                    break;
                }
            }
        }
        (moved, tc)
    }

    /// Advance channel `ch` by `units` transfers without moving data, as a
    /// device writing to memory we don't model (a sound card recording
    /// silence) would.
    pub fn transfer_skip(&mut self, ch: usize, units: usize) -> bool {
        let mut tc = false;
        for _ in 0..units {
            let c = self.channel_mut(ch);
            if c.masked {
                break;
            }
            c.cur_addr = if c.decrement() { c.cur_addr.wrapping_sub(1) } else { c.cur_addr.wrapping_add(1) };
            let (count, underflow) = c.cur_count.overflowing_sub(1);
            c.cur_count = count;
            if underflow {
                tc = true;
                if c.auto_init() {
                    c.cur_addr = c.base_addr;
                    c.cur_count = c.base_count;
                } else {
                    c.masked = true;
                }
                self.ctrl[ch / 4].tc |= 1 << (ch % 4);
            }
        }
        tc
    }
}
