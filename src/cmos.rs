//! The AT's CMOS RAM and real-time clock (MC146818) at ports 70h/71h.
//!
//! Port 70h selects a register (bit 7 masks NMI), port 71h reads or writes
//! it. The clock registers read the host's local time in BCD. The BIOS
//! configuration part reports the floppy drives mounted on A: and B: as
//! 1.44 MB drives, a coprocessor, and the base and extended memory sizes.
//! Register 0Fh is the shutdown status byte that tells the BIOS, after a CPU
//! reset, whether to resume a program through the pointer at 40:67 (used by
//! 286-era protected mode code).

use chrono::{Datelike, Local, Timelike};

pub const SHUTDOWN_STATUS: u8 = 0x0F;

pub struct Cmos {
    index: u8,
    ram: [u8; 128],
}

fn bcd(value: u32) -> u8 {
    (((value / 10) % 10) << 4 | (value % 10)) as u8
}

impl Cmos {
    /// CMOS contents for a machine with `extended_kb` KB above 1 MB.
    pub fn new(extended_kb: u32) -> Self {
        let mut ram = [0u8; 128];
        ram[0x0A] = 0x26; // 32.768 kHz time base, 1024 Hz periodic rate
        ram[0x0B] = 0x02; // 24-hour clock, BCD
        ram[0x0D] = 0x80; // RAM and time valid
        ram[0x14] = 0x02; // math coprocessor present; floppies from set_floppies
        ram[0x15] = 0x80; // 640 KB base memory (0280h)
        ram[0x16] = 0x02;
        let ext = extended_kb.min(0xFFFF);
        ram[0x17] = ext as u8;
        ram[0x18] = (ext >> 8) as u8;
        ram[0x30] = ext as u8;
        ram[0x31] = (ext >> 8) as u8;
        let mut cmos = Self { index: 0, ram };
        cmos.update_checksum();
        cmos
    }

    /// Report `count` 1.44 MB floppy drives, A: first, in the drive type
    /// byte (10h) and the equipment byte (14h).
    pub fn set_floppies(&mut self, count: u8) {
        self.ram[0x10] = match count {
            0 => 0x00,
            1 => 0x40,
            _ => 0x44,
        };
        self.ram[0x14] &= !0xC1;
        if count > 0 {
            self.ram[0x14] |= 0x01 | (count.min(4) - 1) << 6;
        }
        self.update_checksum();
    }

    /// Checksum of registers 10h-2Dh at 2Eh (high) and 2Fh (low).
    fn update_checksum(&mut self) {
        let sum: u16 = self.ram[0x10..=0x2D].iter().map(|&b| b as u16).sum();
        self.ram[0x2E] = (sum >> 8) as u8;
        self.ram[0x2F] = sum as u8;
    }

    pub fn write_index(&mut self, value: u8) {
        self.index = value & 0x7F;
    }

    pub fn read_data(&self) -> u8 {
        let now = Local::now();
        match self.index {
            0x00 => bcd(now.second()),
            0x02 => bcd(now.minute()),
            0x04 => bcd(now.hour()),
            0x06 => bcd(now.weekday().number_from_sunday()),
            0x07 => bcd(now.day()),
            0x08 => bcd(now.month()),
            0x09 => bcd(now.year() as u32 % 100),
            0x32 => bcd(now.year() as u32 / 100),
            // Status A: no update in progress.
            0x0A => self.ram[0x0A] & 0x7F,
            i => self.ram[i as usize],
        }
    }

    pub fn write_data(&mut self, value: u8) {
        match self.index {
            // The clock can't be set, and status C/D are read-only.
            0x00..=0x09 | 0x0C | 0x0D | 0x32 => {}
            i => self.ram[i as usize] = value,
        }
    }

    /// A register's stored value, for the BIOS.
    pub fn get(&self, index: u8) -> u8 {
        self.ram[(index & 0x7F) as usize]
    }

    pub fn set(&mut self, index: u8, value: u8) {
        self.ram[(index & 0x7F) as usize] = value;
    }
}
