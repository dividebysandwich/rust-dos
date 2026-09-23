//! The 8042 keyboard controller at ports 60h and 64h.
//!
//! Scan code bytes from the keyboard queue up and enter the output buffer
//! one at a time: each byte raises IRQ 1 (when the command byte enables it)
//! and stays at port 60h until the CPU reads it, then the next one moves
//! in. Programs with their own INT 09h handler therefore see every byte of
//! a burst of key events, including the E0 prefixes of the extended keys.
//!
//! The controller also owns the A20 gate and the CPU reset line through its
//! output port (command D1h), which DOS extenders and HIMEM-style code use.

use std::collections::VecDeque;

/// Command byte bits.
const CMD_KBD_IRQ: u8 = 0x01;
const CMD_SYSTEM_FLAG: u8 = 0x04;
const CMD_KBD_DISABLED: u8 = 0x10;
const CMD_TRANSLATE: u8 = 0x40;

/// Output port bits.
pub const OUT_RESET: u8 = 0x01;
pub const OUT_A20: u8 = 0x02;

/// Status register bits.
const STATUS_OBF: u8 = 0x01;
const STATUS_SYSTEM: u8 = 0x04;
const STATUS_COMMAND: u8 = 0x08;
const STATUS_UNLOCKED: u8 = 0x10;

/// What the controller asks the rest of the machine to do after a port
/// write.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Effects {
    /// The A20 gate changed to this state.
    pub a20: Option<bool>,
    /// The CPU reset line was pulsed.
    pub reset: bool,
}

pub struct Kbc {
    /// Bytes waiting for the output buffer, oldest first.
    queue: VecDeque<u8>,
    /// The output buffer (port 60h) when full.
    output: Option<u8>,
    /// What port 60h reads when the output buffer is empty: the last byte.
    last: u8,
    pub command_byte: u8,
    pub output_port: u8,
    /// A controller command waiting for its parameter at port 60h.
    pending_command: Option<u8>,
    /// A keyboard command waiting for its parameter (ED, F3).
    pending_kbd: Option<u8>,
    /// The last write went to port 64h (status bit 3).
    last_was_command: bool,
    /// A byte just entered the output buffer and IRQ 1 should fire.
    irq: bool,
}

impl Default for Kbc {
    fn default() -> Self {
        Self::new()
    }
}

impl Kbc {
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            output: None,
            last: 0,
            command_byte: CMD_KBD_IRQ | CMD_SYSTEM_FLAG | CMD_TRANSLATE,
            output_port: OUT_RESET,
            pending_command: None,
            pending_kbd: None,
            last_was_command: false,
            irq: false,
        }
    }

    /// Queue scan code bytes from the keyboard.
    pub fn push_scancodes(&mut self, bytes: &[u8]) {
        self.queue.extend(bytes);
        self.refill();
    }

    /// Put a reply in front of the queued scan codes: controller command
    /// results go straight to the output buffer.
    fn reply(&mut self, byte: u8) {
        self.queue.push_front(byte);
        self.refill();
    }

    /// Move the next queued byte into an empty output buffer.
    fn refill(&mut self) {
        if self.output.is_none() && self.command_byte & CMD_KBD_DISABLED == 0 {
            if let Some(byte) = self.queue.pop_front() {
                self.output = Some(byte);
                self.last = byte;
                if self.command_byte & CMD_KBD_IRQ != 0 {
                    self.irq = true;
                }
            }
        }
    }

    /// True once per byte entering the output buffer: raise IRQ 1.
    pub fn take_irq(&mut self) -> bool {
        std::mem::take(&mut self.irq)
    }

    /// Bytes not yet read by the CPU, including the output buffer.
    pub fn pending(&self) -> usize {
        self.queue.len() + self.output.is_some() as usize
    }

    /// Port 60h read.
    pub fn read_data(&mut self) -> u8 {
        let byte = self.output.take().unwrap_or(self.last);
        self.refill();
        byte
    }

    /// Port 64h read: the status register.
    pub fn read_status(&self) -> u8 {
        let mut status = STATUS_UNLOCKED;
        if self.output.is_some() {
            status |= STATUS_OBF;
        }
        if self.command_byte & CMD_SYSTEM_FLAG != 0 {
            status |= STATUS_SYSTEM;
        }
        if self.last_was_command {
            status |= STATUS_COMMAND;
        }
        status
    }

    /// Port 64h write: a controller command.
    pub fn write_command(&mut self, command: u8) -> Effects {
        self.last_was_command = true;
        let mut effects = Effects::default();
        match command {
            0x20 => self.reply(self.command_byte),
            0x60 | 0xD1 | 0xD2 | 0xD3 | 0xD4 => self.pending_command = Some(command),
            0xA7 | 0xA8 => {} // disable / enable the aux (mouse) port
            0xA9 => self.reply(0x00), // aux interface test: OK
            0xAA => self.reply(0x55), // self test passed
            0xAB => self.reply(0x00), // keyboard interface test: OK
            0xAD => self.command_byte |= CMD_KBD_DISABLED,
            0xAE => {
                self.command_byte &= !CMD_KBD_DISABLED;
                self.refill();
            }
            0xC0 => self.reply(0xBF), // input port: keyboard unlocked, colour
            0xD0 => self.reply(self.output_port),
            // Undocumented A20 commands many chipsets support.
            0xDD => effects.a20 = Some(self.set_output_port(self.output_port & !OUT_A20)),
            0xDF => effects.a20 = Some(self.set_output_port(self.output_port | OUT_A20)),
            0xE0 => self.reply(0x00), // test inputs
            // Pulse output port bits low; bit 0 is the CPU reset line.
            0xF0..=0xFF => effects.reset = command & 0x01 == 0,
            _ => {}
        }
        effects
    }

    /// Set the output port and return the A20 state it selects.
    fn set_output_port(&mut self, value: u8) -> bool {
        self.output_port = value;
        value & OUT_A20 != 0
    }

    /// Port 60h write: a controller command's parameter, or a command to
    /// the keyboard.
    pub fn write_data(&mut self, value: u8) -> Effects {
        self.last_was_command = false;
        let mut effects = Effects::default();
        if let Some(command) = self.pending_command.take() {
            match command {
                0x60 => {
                    self.command_byte = value;
                    self.refill();
                }
                0xD1 => {
                    effects.a20 = Some(self.set_output_port(value));
                    effects.reset = value & OUT_RESET == 0;
                }
                // Write the keyboard (or aux) output buffer, as if the byte
                // had come from the device.
                0xD2 | 0xD3 => self.reply(value),
                _ => {} // D4: a byte for the mouse; there is none
            }
            return effects;
        }
        if let Some(command) = self.pending_kbd.take() {
            // The parameter of ED (LEDs) or F3 (typematic rate).
            let _ = command;
            self.reply(0xFA);
            return effects;
        }
        match value {
            0xED | 0xF3 => {
                self.pending_kbd = Some(value);
                self.reply(0xFA);
            }
            0xEE => self.reply(0xEE), // echo
            0xF2 => {
                // Identify: an MF2 keyboard.
                self.queue.push_front(0x83);
                self.queue.push_front(0xAB);
                self.reply(0xFA);
            }
            0xFF => {
                // Reset: ACK, then self test passed.
                self.queue.push_front(0xAA);
                self.reply(0xFA);
            }
            _ => self.reply(0xFA),
        }
        effects
    }
}
