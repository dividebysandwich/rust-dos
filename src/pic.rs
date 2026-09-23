//! The two cascaded 8259A programmable interrupt controllers of an AT.
//!
//! The master handles IRQ 0-7 at ports 20h/21h, the slave IRQ 8-15 at ports
//! A0h/A1h, and the slave's output drives the master's IRQ 2. Programs
//! initialize them with ICW1-ICW4 (the vector base comes from ICW2: 08h and
//! 70h after the BIOS), mask lines with OCW1, acknowledge interrupts with an
//! EOI (OCW2), and read the request or in-service register (OCW3).
//!
//! Devices raise edge-triggered requests with `Pic::raise`; the Sound
//! Blaster's line is level-triggered and passed in by the bus when it asks
//! for the next interrupt. Protected-mode DOS extenders that don't remap the
//! PIC read the in-service register to tell IRQ 0-7 from CPU exceptions at
//! the same vectors, so the ISR bits are set at acknowledge time as on
//! hardware.

/// The IRQ of the master that the slave is wired to.
const CASCADE: u8 = 2;

/// One 8259A.
#[derive(Clone, Debug)]
pub struct Pic8259 {
    /// Interrupt request register: lines waiting to be delivered.
    pub irr: u8,
    /// Interrupt mask register (OCW1): 1 = line disabled.
    pub imr: u8,
    /// In-service register: delivered, not yet acknowledged with an EOI.
    pub isr: u8,
    /// Vector of line 0 (ICW2, low 3 bits clear).
    pub base: u8,
    /// Which initialization word the next data port write is: 0 when the
    /// chip is initialized, otherwise 2, 3 or 4.
    init_step: u8,
    /// ICW1 said an ICW4 follows.
    icw4_needed: bool,
    /// ICW1 said this is the only 8259 (no ICW3).
    single: bool,
    /// ICW4: automatic end of interrupt.
    pub aeoi: bool,
    /// OCW3: port 20h/A0h reads return the ISR instead of the IRR.
    read_isr: bool,
}

impl Pic8259 {
    fn new(base: u8, imr: u8) -> Self {
        Self {
            irr: 0,
            imr,
            isr: 0,
            base,
            init_step: 0,
            icw4_needed: false,
            single: false,
            aeoi: false,
            read_isr: false,
        }
    }

    /// The line this chip would signal now, given the lines requesting
    /// (`requests`): unmasked, and of higher priority (lower number) than
    /// every line in service.
    fn highest(&self, requests: u8) -> Option<u8> {
        let requests = requests & !self.imr;
        if requests == 0 {
            return None;
        }
        let line = requests.trailing_zeros() as u8;
        if self.isr != 0 && self.isr.trailing_zeros() as u8 <= line {
            return None;
        }
        Some(line)
    }

    fn acknowledge(&mut self, line: u8) {
        self.irr &= !(1 << line);
        if !self.aeoi {
            self.isr |= 1 << line;
        }
    }

    /// Write to the command port (20h/A0h): ICW1, OCW2 or OCW3.
    fn write_command(&mut self, value: u8) {
        if value & 0x10 != 0 {
            // ICW1 starts initialization: the mask and in-service state are
            // cleared and the data port expects ICW2.
            self.icw4_needed = value & 0x01 != 0;
            self.single = value & 0x02 != 0;
            self.imr = 0;
            self.isr = 0;
            self.irr = 0;
            self.aeoi = false;
            self.read_isr = false;
            self.init_step = 2;
        } else if value & 0x08 != 0 {
            // OCW3: select the register command port reads return.
            if value & 0x02 != 0 {
                self.read_isr = value & 0x01 != 0;
            }
        } else {
            // OCW2: end of interrupt, optionally with rotation (which we
            // treat as a plain EOI).
            match value >> 5 {
                // Non-specific: the highest priority line in service.
                0b001 | 0b101 => self.isr &= self.isr.wrapping_sub(1),
                // Specific: the line in bits 0-2.
                0b011 | 0b111 => self.isr &= !(1 << (value & 0x07)),
                _ => {}
            }
        }
    }

    /// Write to the data port (21h/A1h): the next ICW during
    /// initialization, OCW1 (the mask) otherwise.
    fn write_data(&mut self, value: u8) {
        match self.init_step {
            2 => {
                self.base = value & 0xF8;
                self.init_step = if !self.single {
                    3
                } else if self.icw4_needed {
                    4
                } else {
                    0
                };
            }
            3 => {
                // ICW3: the cascade wiring, which is fixed on an AT.
                self.init_step = if self.icw4_needed { 4 } else { 0 };
            }
            4 => {
                self.aeoi = value & 0x02 != 0;
                self.init_step = 0;
            }
            _ => self.imr = value,
        }
    }

    fn read_command(&self, requests: u8) -> u8 {
        if self.read_isr { self.isr } else { requests }
    }
}

/// The master and slave 8259A.
#[derive(Clone, Debug)]
pub struct Pic {
    pub master: Pic8259,
    pub slave: Pic8259,
}

impl Default for Pic {
    fn default() -> Self {
        Self::new()
    }
}

impl Pic {
    /// The state the BIOS leaves: vectors 08h and 70h, and every line of
    /// the master enabled (programs expect their IRQ to work without
    /// unmasking it), the slave's lines masked.
    pub fn new() -> Self {
        Self {
            master: Pic8259::new(0x08, 0x00),
            slave: Pic8259::new(0x70, 0xFF),
        }
    }

    /// Request `irq` (0-15), edge-triggered.
    pub fn raise(&mut self, irq: u8) {
        if irq < 8 {
            self.master.irr |= 1 << irq;
        } else {
            self.slave.irr |= 1 << (irq - 8);
        }
    }

    /// Withdraw a request for `irq` that hasn't been delivered.
    pub fn lower(&mut self, irq: u8) {
        if irq < 8 {
            self.master.irr &= !(1 << irq);
        } else {
            self.slave.irr &= !(1 << (irq - 8));
        }
    }

    /// Requests of the slave, including level-triggered lines 8-15 in
    /// `levels`.
    fn slave_requests(&self, levels: u16) -> u8 {
        self.slave.irr | (levels >> 8) as u8
    }

    /// Requests of the master, including level-triggered lines 0-7 in
    /// `levels` and the slave's output on the cascade line.
    fn master_requests(&self, levels: u16) -> u8 {
        let mut requests = self.master.irr | levels as u8;
        if self.slave.highest(self.slave_requests(levels)).is_some() {
            requests |= 1 << CASCADE;
        }
        requests
    }

    /// The IRQ (0-15) the CPU would receive now. `levels` are the
    /// level-triggered request lines (bit n = IRQ n).
    pub fn pending(&self, levels: u16) -> Option<u8> {
        let line = self.master.highest(self.master_requests(levels))?;
        if line != CASCADE {
            return Some(line);
        }
        self.slave
            .highest(self.slave_requests(levels))
            .map(|l| l + 8)
    }

    /// The CPU takes interrupt `irq`: it goes in service (on both chips for
    /// a slave line). Returns its vector.
    pub fn acknowledge(&mut self, irq: u8) -> u8 {
        if irq < 8 {
            self.master.acknowledge(irq);
            self.master.base | irq
        } else {
            self.slave.acknowledge(irq - 8);
            self.master.acknowledge(CASCADE);
            self.slave.base | (irq - 8)
        }
    }

    /// Vector of `irq` with the current programming.
    pub fn vector(&self, irq: u8) -> u8 {
        if irq < 8 { self.master.base | irq } else { self.slave.base | (irq - 8) }
    }

    pub fn write(&mut self, port: u16, value: u8) {
        match port {
            0x20 => self.master.write_command(value),
            0x21 => self.master.write_data(value),
            0xA0 => self.slave.write_command(value),
            _ => self.slave.write_data(value),
        }
    }

    pub fn read(&self, port: u16, levels: u16) -> u8 {
        match port {
            0x20 => self.master.read_command(self.master_requests(levels)),
            0x21 => self.master.imr,
            0xA0 => self.slave.read_command(self.slave_requests(levels)),
            _ => self.slave.imr,
        }
    }
}
