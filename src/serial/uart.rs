//! The National 16550A UART of a PC's serial port (or the 8250 before it,
//! which has no FIFOs): eight ports from its base (3F8h for COM1, 2F8h for
//! COM2, 3E8h for COM3, 2E8h for COM4).
//!
//! 00h the receive buffer / transmit holding register (the divisor's low
//! byte with DLAB set), 01h the interrupt enable register (the divisor's
//! high byte with DLAB set), 02h the interrupt identification register /
//! FIFO control register, 03h the line control register, 04h the modem
//! control register, 05h the line status register, 06h the modem status
//! register, 07h the scratch register.
//!
//! Characters take the time the baud rate and the line format give them,
//! both ways: what the program writes leaves the shift register a
//! character time after it went in, and what comes from the other end
//! (`receive`) waits outside the chip and enters its receive FIFO one
//! character time after another. What waits outside is held while the FIFO
//! is full, as a cable with flow control would, rather than lost to an
//! overrun. The interrupt line goes to the PIC through OUT2, as on a PC.

use std::collections::VecDeque;

/// The UART's ports, from its base.
pub const PORTS: u16 = 8;

/// The chips there are.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Chip {
    /// The 8250 (16450): one character each way.
    Ns8250,
    /// The 16550A: 16-character FIFOs each way.
    #[default]
    Ns16550,
}

crate::state_enum!(Chip { Chip::Ns8250, Chip::Ns16550 });

/// Interrupt enable bits.
const IER_RDA: u8 = 0x01;
const IER_THRE: u8 = 0x02;
const IER_RLS: u8 = 0x04;
const IER_MS: u8 = 0x08;
/// Line control: divisor latch access.
const LCR_DLAB: u8 = 0x80;
/// Modem control lines.
pub const MCR_DTR: u8 = 0x01;
pub const MCR_RTS: u8 = 0x02;
pub const MCR_OUT1: u8 = 0x04;
pub const MCR_OUT2: u8 = 0x08;
pub const MCR_LOOP: u8 = 0x10;
/// Line status bits.
const LSR_DR: u8 = 0x01;
const LSR_OE: u8 = 0x02;
const LSR_BI: u8 = 0x10;
const LSR_THRE: u8 = 0x20;
const LSR_TEMT: u8 = 0x40;
/// Modem status: the lines from the other end, and their changes.
pub const MSR_CTS: u8 = 0x10;
pub const MSR_DSR: u8 = 0x20;
pub const MSR_RI: u8 = 0x40;
pub const MSR_DCD: u8 = 0x80;
const MSR_TERI: u8 = 0x04;
/// Characters a FIFO holds.
const FIFO: usize = 16;
/// Characters waiting outside the chip for its receive FIFO, at most.
const WAITING: usize = 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Uart {
    pub base: u16,
    pub irq: u8,
    pub chip: Chip,
    divisor: u16,
    ier: u8,
    lcr: u8,
    mcr: u8,
    scr: u8,
    /// The FIFO control register's FIFO enable and receive trigger bits.
    fifo_on: bool,
    trigger: u8,
    /// Overrun and break, until LSR is read.
    lsr_errors: u8,
    /// The lines from the other end (CTS, DSR, RI, DCD) and the changes
    /// to them since MSR was read.
    lines: u8,
    msr_delta: u8,
    /// The receive FIFO (one character without FIFOs).
    rx: VecDeque<u8>,
    /// Characters from the other end, waiting to enter it.
    waiting: VecDeque<u8>,
    /// When the next one enters, while any wait.
    rx_due: Option<u64>,
    /// When the receive FIFO's timeout comes, while it holds characters
    /// below its trigger level, and whether it came.
    timeout_due: Option<u64>,
    timeout: bool,
    /// The transmit FIFO (the holding register without FIFOs).
    tx: VecDeque<u8>,
    /// The character in the shift register and when it is out.
    shifting: Option<(u8, u64)>,
    /// The holding register emptied and IIR hasn't reported it since.
    thre_pending: bool,
    /// The characters sent, for the other end.
    pub outgoing: Vec<u8>,
    /// Whether the port holds its IRQ line up, as the bus last saw it.
    pub pic_line: bool,
}

impl Default for Uart {
    fn default() -> Self {
        Self::new(0x3F8, 4, Chip::Ns16550)
    }
}

crate::state_fields!(Uart {
    base, irq, chip, divisor, ier, lcr, mcr, scr, fifo_on, trigger, lsr_errors, lines, msr_delta, rx, waiting,
    rx_due, timeout_due, timeout, tx, shifting, thre_pending, pic_line
} skip { outgoing });

impl Uart {
    pub fn new(base: u16, irq: u8, chip: Chip) -> Self {
        let mut uart = Self {
            base,
            irq,
            chip,
            divisor: 12,
            ier: 0,
            lcr: 0x03,
            mcr: 0,
            scr: 0,
            fifo_on: false,
            trigger: 1,
            lsr_errors: 0,
            lines: 0,
            msr_delta: 0,
            rx: VecDeque::new(),
            waiting: VecDeque::new(),
            rx_due: None,
            timeout_due: None,
            timeout: false,
            tx: VecDeque::new(),
            shifting: None,
            thre_pending: false,
            outgoing: Vec::new(),
            pic_line: false,
        };
        uart.reset();
        uart
    }

    /// The chip's master reset: its registers cleared, what it held gone.
    /// The lines from the other end stay as they are.
    pub fn reset(&mut self) {
        self.ier = 0;
        self.mcr = 0;
        self.lcr = 0x03;
        self.fifo_on = false;
        self.trigger = 1;
        self.lsr_errors = 0;
        self.msr_delta = 0;
        self.rx.clear();
        self.rx_due = None;
        self.timeout_due = None;
        self.timeout = false;
        self.tx.clear();
        self.shifting = None;
        self.thre_pending = false;
        if !self.waiting.is_empty() {
            self.rx_due = Some(0);
        }
    }

    /// The bits a character takes on the wire: start, data, parity, stop.
    fn bits(&self) -> u64 {
        let data = 5 + (self.lcr & 3) as u64;
        let parity = (self.lcr >> 3 & 1) as u64;
        let stop = if self.lcr & 0x04 != 0 { 2 } else { 1 };
        1 + data + parity + stop
    }

    /// The baud rate the divisor gives (a divisor of 0 as 1).
    pub fn baud(&self) -> u32 {
        115_200 / self.divisor.max(1) as u32
    }

    /// The data bits of a character (5-8).
    pub fn data_bits(&self) -> u8 {
        5 + (self.lcr & 3)
    }

    /// The PIT ticks a character takes.
    pub fn char_ticks(&self) -> u64 {
        (crate::timer::PIT_HZ * self.bits() * self.divisor.max(1) as u64).div_ceil(115_200).max(1)
    }

    /// The modem control lines the program sets (DTR, RTS, OUT1, OUT2),
    /// as the other end sees them: none in loopback mode.
    pub fn mcr_out(&self) -> u8 {
        if self.mcr & MCR_LOOP != 0 { 0 } else { self.mcr & 0x0F }
    }

    fn fifo_size(&self) -> usize {
        if self.fifo_on { FIFO } else { 1 }
    }

    /// The modem status lines, as the program reads them: the other end's,
    /// or its own modem control lines in loopback mode.
    fn msr_lines(&self) -> u8 {
        if self.mcr & MCR_LOOP != 0 {
            let m = self.mcr;
            (m & MCR_RTS) << 3 | (m & MCR_DTR) << 5 | (m & MCR_OUT1) << 4 | (m & MCR_OUT2) << 4
        } else {
            self.lines
        }
    }

    /// The other end's lines (CTS, DSR, RI, DCD) are now `lines`.
    pub fn set_lines(&mut self, lines: u8) {
        let old = self.lines;
        self.lines = lines & 0xF0;
        if self.mcr & MCR_LOOP == 0 {
            self.note_msr_change(old, self.lines);
        }
    }

    pub fn lines(&self) -> u8 {
        self.lines
    }

    fn note_msr_change(&mut self, old: u8, new: u8) {
        let changed = old ^ new;
        self.msr_delta |= (changed & (MSR_CTS | MSR_DSR | MSR_DCD)) >> 4;
        if old & MSR_RI != 0 && new & MSR_RI == 0 {
            self.msr_delta |= MSR_TERI;
        }
    }

    /// Characters from the other end.
    pub fn receive(&mut self, bytes: &[u8], now: u64) {
        if bytes.is_empty() || self.mcr & MCR_LOOP != 0 {
            return;
        }
        let room = WAITING.saturating_sub(self.waiting.len());
        self.waiting.extend(bytes.iter().take(room));
        if self.rx_due.is_none() {
            self.rx_due = Some(now + self.char_ticks());
        }
    }

    /// Characters waiting to enter the chip, or in its receive FIFO.
    pub fn receiving(&self) -> usize {
        self.waiting.len() + self.rx.len()
    }

    /// Whether the receiving side has nothing waiting or unread.
    pub fn rx_idle(&self) -> bool {
        self.waiting.is_empty() && self.rx.is_empty()
    }

    /// The other end sends a break.
    pub fn receive_break(&mut self) {
        self.lsr_errors |= LSR_BI;
    }

    pub fn read(&mut self, offset: u16, now: u64) -> u8 {
        self.advance(now);
        match offset {
            0 if self.lcr & LCR_DLAB != 0 => self.divisor as u8,
            0 => self.read_rbr(now),
            1 if self.lcr & LCR_DLAB != 0 => (self.divisor >> 8) as u8,
            1 => self.ier,
            2 => {
                let iir = self.iir();
                // Reading IIR ends the interrupt of an empty holding
                // register it reports.
                if iir & 0x0F == 0x02 {
                    self.thre_pending = false;
                }
                iir
            }
            3 => self.lcr,
            4 => self.mcr,
            5 => {
                let lsr = self.lsr();
                self.lsr_errors = 0;
                lsr
            }
            6 => {
                let msr = self.msr_lines() | self.msr_delta;
                self.msr_delta = 0;
                msr
            }
            _ => self.scr,
        }
    }

    pub fn write(&mut self, offset: u16, value: u8, now: u64) {
        self.advance(now);
        match offset {
            0 if self.lcr & LCR_DLAB != 0 => {
                self.divisor = self.divisor & 0xFF00 | value as u16;
            }
            0 => self.write_thr(value, now),
            1 if self.lcr & LCR_DLAB != 0 => {
                self.divisor = self.divisor & 0x00FF | (value as u16) << 8;
            }
            1 => {
                let enabled = value & !self.ier;
                self.ier = value & 0x0F;
                // Enabling the holding register's interrupt while it is
                // empty interrupts at once, as drivers expect.
                if enabled & IER_THRE != 0 && self.tx.is_empty() {
                    self.thre_pending = true;
                }
            }
            2 => self.write_fcr(value),
            3 => self.lcr = value,
            4 => {
                let (old, loop_old) = (self.msr_lines(), self.mcr & MCR_LOOP != 0);
                self.mcr = value & 0x1F;
                let loop_now = self.mcr & MCR_LOOP != 0;
                if loop_old || loop_now {
                    self.note_msr_change(old, self.msr_lines());
                }
            }
            5 | 6 => {}
            _ => self.scr = value,
        }
    }

    fn write_fcr(&mut self, value: u8) {
        if self.chip == Chip::Ns8250 {
            return;
        }
        let on = value & 0x01 != 0;
        if on != self.fifo_on {
            self.rx.clear();
            self.tx.clear();
        }
        self.fifo_on = on;
        if value & 0x02 != 0 {
            self.rx.clear();
            self.timeout = false;
            self.timeout_due = None;
        }
        if value & 0x04 != 0 {
            self.tx.clear();
        }
        self.trigger = [1, 4, 8, 14][(value >> 6) as usize];
        if !self.waiting.is_empty() && self.rx_due.is_none() {
            self.rx_due = Some(0);
        }
    }

    fn write_thr(&mut self, value: u8, now: u64) {
        if self.tx.len() >= self.fifo_size() {
            // The FIFO is full: the character overwrites the last.
            self.tx.pop_back();
        }
        self.tx.push_back(value);
        self.thre_pending = false;
        self.start_shift(now);
    }

    /// Send a character the way the BIOS or DOS does, waiting for room
    /// (rather than overwriting what the FIFO holds).
    pub fn send(&mut self, value: u8, now: u64) {
        self.advance(now);
        self.tx.push_back(value);
        self.thre_pending = false;
        self.start_shift(now);
    }

    /// Move the next character into an empty shift register.
    fn start_shift(&mut self, now: u64) {
        if self.shifting.is_some() {
            return;
        }
        if let Some(byte) = self.tx.pop_front() {
            self.shifting = Some((byte, now + self.char_ticks()));
            if self.tx.is_empty() {
                self.thre_pending = true;
            }
        }
    }

    fn read_rbr(&mut self, now: u64) -> u8 {
        let byte = self.rx.pop_front().unwrap_or(0);
        self.timeout = false;
        self.timeout_due = (!self.rx.is_empty() && self.fifo_on).then(|| now + 4 * self.char_ticks());
        if self.rx_due.is_none() && !self.waiting.is_empty() {
            // Held while the FIFO was full: the next comes in now.
            self.rx_due = Some(now);
            self.advance(now);
        }
        byte
    }

    /// Take a received character the way the BIOS does: the next one, if
    /// any came.
    pub fn take(&mut self, now: u64) -> Option<u8> {
        self.advance(now);
        (!self.rx.is_empty()).then(|| self.read_rbr(now))
    }

    fn lsr(&self) -> u8 {
        let mut lsr = self.lsr_errors;
        if !self.rx.is_empty() {
            lsr |= LSR_DR;
        }
        if self.tx.is_empty() {
            lsr |= LSR_THRE;
            if self.shifting.is_none() {
                lsr |= LSR_TEMT;
            }
        }
        lsr
    }

    /// Whether the receive FIFO holds as many characters as its trigger
    /// level wants (one without FIFOs).
    fn rx_ready(&self) -> bool {
        let level = if self.fifo_on { self.trigger as usize } else { 1 };
        self.rx.len() >= level
    }

    /// The interrupt identification: the highest priority interrupt that
    /// waits, and the FIFOs' bits.
    fn iir(&self) -> u8 {
        let fifo = if self.fifo_on { 0xC0 } else { 0 };
        fifo | self.interrupt().unwrap_or(0x01)
    }

    /// The interrupt that waits: its identification bits.
    fn interrupt(&self) -> Option<u8> {
        if self.ier & IER_RLS != 0 && self.lsr_errors & (LSR_OE | LSR_BI) != 0 {
            Some(0x06)
        } else if self.ier & IER_RDA != 0 && self.rx_ready() {
            Some(0x04)
        } else if self.ier & IER_RDA != 0 && self.timeout {
            Some(0x0C)
        } else if self.ier & IER_THRE != 0 && self.thre_pending {
            Some(0x02)
        } else if self.ier & IER_MS != 0 && self.msr_delta != 0 {
            Some(0x00)
        } else {
            None
        }
    }

    /// Whether the port holds its IRQ line up: an interrupt waits and OUT2
    /// lets it through.
    pub fn irq_line(&self) -> bool {
        self.mcr & MCR_OUT2 != 0 && self.interrupt().is_some()
    }

    /// The registers and what the chip holds, for the debugger.
    pub fn describe(&self) -> Vec<(&'static str, String)> {
        vec![
            ("base", format!("{:X}", self.base)),
            ("irq", self.irq.to_string()),
            ("chip", format!("{:?}", self.chip)),
            ("baud", self.baud().to_string()),
            ("lcr", format!("{:02X}", self.lcr)),
            ("ier", format!("{:02X}", self.ier)),
            ("iir", format!("{:02X}", self.iir())),
            ("mcr", format!("{:02X}", self.mcr)),
            ("lsr", format!("{:02X}", self.lsr())),
            ("msr", format!("{:02X}", self.msr_lines() | self.msr_delta)),
            ("fifo", format!("{} (trigger {})", self.fifo_on, self.trigger)),
            ("rx", format!("{} in the FIFO, {} waiting", self.rx.len(), self.waiting.len())),
            ("tx", format!("{} in the FIFO, shifting {}", self.tx.len(), self.shifting.is_some())),
            ("irq_line", self.irq_line().to_string()),
        ]
    }

    /// The next PIT tick something happens at.
    pub fn next_event(&self) -> Option<u64> {
        [self.shifting.map(|(_, due)| due), self.rx_due, self.timeout_due].into_iter().flatten().min()
    }

    /// Bring the chip up to `now`: characters out of the shift register
    /// and into the receive FIFO, and the FIFO's timeout.
    pub fn advance(&mut self, now: u64) {
        while let Some((byte, due)) = self.shifting
            && due <= now
        {
            self.shifting = None;
            if self.mcr & MCR_LOOP != 0 {
                self.waiting.push_back(byte);
                self.rx_due.get_or_insert(due);
            } else {
                self.outgoing.push(byte);
            }
            if let Some(next) = self.tx.pop_front() {
                self.shifting = Some((next, due + self.char_ticks()));
                if self.tx.is_empty() {
                    self.thre_pending = true;
                }
            }
        }
        let size = self.fifo_size();
        while let Some(due) = self.rx_due
            && due <= now
        {
            if self.rx.len() >= size {
                // Full: the rest waits until the program reads.
                self.rx_due = None;
                break;
            }
            match self.waiting.pop_front() {
                Some(byte) => {
                    self.rx.push_back(byte);
                    self.timeout = false;
                    self.timeout_due = self.fifo_on.then(|| due + 4 * self.char_ticks());
                }
                None => {
                    self.rx_due = None;
                    break;
                }
            }
            self.rx_due = (!self.waiting.is_empty()).then(|| due + self.char_ticks());
        }
        if let Some(due) = self.timeout_due
            && due <= now
        {
            self.timeout_due = None;
            self.timeout = !self.rx.is_empty() && !self.rx_ready();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uart() -> Uart {
        let mut u = Uart::new(0x3F8, 4, Chip::Ns16550);
        // 115200 baud, 8N1.
        u.write(3, 0x80, 0);
        u.write(0, 1, 0);
        u.write(1, 0, 0);
        u.write(3, 0x03, 0);
        u
    }

    #[test]
    fn divisor_and_char_time() {
        let mut u = uart();
        assert_eq!(u.baud(), 115_200);
        // 10 bits at 115200 baud: 86.8 us, 104 PIT ticks.
        assert_eq!(u.char_ticks(), 104);
        u.write(3, 0x80, 0);
        u.write(0, 0x60, 0);
        assert_eq!(u.read(0, 0), 0x60);
        u.write(3, 0x02, 0);
        assert_eq!(u.baud(), 1200);
        assert_eq!(u.data_bits(), 7);
    }

    #[test]
    fn transmit_takes_a_character_time() {
        let mut u = uart();
        u.write(1, IER_THRE, 0);
        u.write(4, MCR_OUT2, 0);
        // Empty holding register: an interrupt at once.
        assert_eq!(u.read(2, 0) & 0x0F, 0x02);
        assert_eq!(u.read(2, 0) & 0x0F, 0x01);
        u.write(0, b'A', 0);
        // Into the shift register at once: the holding register is empty
        // again, the transmitter isn't.
        assert_eq!(u.read(5, 0) & (LSR_THRE | LSR_TEMT), LSR_THRE);
        assert!(u.irq_line());
        u.write(0, b'B', 1);
        assert_eq!(u.read(5, 1) & LSR_THRE, 0);
        u.advance(103);
        assert!(u.outgoing.is_empty());
        u.advance(104);
        assert_eq!(u.outgoing, b"A");
        u.advance(208);
        assert_eq!(u.outgoing, b"AB");
        assert_eq!(u.read(5, 208) & LSR_TEMT, LSR_TEMT);
    }

    #[test]
    fn receive_paced_and_held() {
        let mut u = uart();
        u.write(1, IER_RDA, 0);
        u.write(4, MCR_OUT2, 0);
        u.receive(b"hello", 0);
        assert_eq!(u.read(5, 0) & LSR_DR, 0);
        assert_eq!(u.next_event(), Some(104));
        u.advance(104);
        assert!(u.irq_line());
        assert_eq!(u.read(2, 104) & 0x0F, 0x04);
        // Without FIFOs the next waits until this one is read.
        u.advance(1000);
        assert_eq!(u.read(0, 1000), b'h');
        assert_eq!(u.read(0, 1000), b'e');
        assert_eq!(u.read(5, 1000) & LSR_DR, 0);
        u.advance(1104);
        assert_eq!(u.read(0, 1104), b'l');
    }

    #[test]
    fn fifo_trigger_and_timeout() {
        let mut u = uart();
        u.write(2, 0x81 | 0x06, 0); // FIFOs on, cleared, trigger 8.
        assert_eq!(u.read(2, 0) & 0xC0, 0xC0);
        u.write(1, IER_RDA, 0);
        u.write(4, MCR_OUT2, 0);
        u.receive(b"abc", 0);
        u.advance(3 * 104);
        assert_eq!(u.read(5, 3 * 104) & LSR_DR, LSR_DR);
        assert!(!u.irq_line());
        // Four character times after the last: the timeout.
        u.advance(3 * 104 + 4 * 104);
        assert!(u.irq_line());
        assert_eq!(u.read(2, 7 * 104) & 0x0F, 0x0C);
        assert_eq!(u.read(0, 7 * 104), b'a');
        assert!(!u.irq_line());
        u.receive(b"12345678", 800);
        u.advance(800 + 8 * 104);
        assert_eq!(u.read(2, 800 + 8 * 104) & 0x0F, 0x04);
    }

    #[test]
    fn loopback() {
        let mut u = uart();
        u.write(4, MCR_LOOP | MCR_RTS | MCR_OUT2, 0);
        assert_eq!(u.read(6, 0) & 0xF0, MSR_CTS | MSR_DCD);
        u.write(0, 0x55, 0);
        u.advance(300);
        assert!(u.outgoing.is_empty());
        assert_eq!(u.read(0, 300), 0x55);
        assert_eq!(u.mcr_out(), 0);
    }

    #[test]
    fn modem_status_changes() {
        let mut u = uart();
        u.write(1, IER_MS, 0);
        u.write(4, MCR_OUT2, 0);
        u.set_lines(MSR_DSR | MSR_DCD | MSR_RI);
        assert!(u.irq_line());
        assert_eq!(u.read(6, 0), MSR_DSR | MSR_DCD | MSR_RI | 0x0A);
        assert!(!u.irq_line());
        u.set_lines(MSR_DSR | MSR_DCD);
        assert_eq!(u.read(6, 0), MSR_DSR | MSR_DCD | MSR_TERI);
    }

    #[test]
    fn the_8250_has_no_fifos() {
        let mut u = Uart::new(0x2F8, 3, Chip::Ns8250);
        u.write(2, 0xC7, 0);
        assert_eq!(u.read(2, 0), 0x01);
    }
}
