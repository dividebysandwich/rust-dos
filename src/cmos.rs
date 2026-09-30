//! The AT's CMOS RAM and real-time clock (MC146818) at ports 70h/71h.
//!
//! Port 70h selects a register (bit 7 masks NMI), port 71h reads or writes
//! it. The clock registers read the host's local time in BCD, moved by
//! what DOS or the BIOS set the clock to. The BIOS
//! configuration part reports the floppy drives mounted on A: and B: as
//! 1.44 MB drives, a coprocessor, and the base and extended memory sizes.
//! Register 0Fh is the shutdown status byte that tells the BIOS, after a CPU
//! reset, whether to resume a program through the pointer at 40:67 (used by
//! 286-era protected mode code).
//!
//! The periodic interrupt (status B bit 6) raises IRQ 8 at the rate in
//! status A (1024 Hz as the BIOS sets it), which HX's Win32 emulation
//! times its Sleep and timers with: the bus fires it (`periodic_fired`) as
//! its `periodic_ticks` come due, and reading status C acknowledges it.

use chrono::{Datelike, NaiveDateTime, TimeDelta, Timelike};

pub const SHUTDOWN_STATUS: u8 = 0x0F;

/// Status B: the periodic interrupt enabled.
const PIE: u8 = 0x40;
/// Status C: an interrupt is asserted (IRQF), and a periodic one came (PF).
const IRQF: u8 = 0x80;
const PF: u8 = 0x40;

pub struct Cmos {
    index: u8,
    ram: [u8; 128],
    /// How far the machine's clock is from the host's, which DATE, TIME and
    /// the services that set the clock move it.
    offset: TimeDelta,
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
        let mut cmos = Self { index: 0, ram, offset: TimeDelta::zero() };
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

    /// Report the first two of `disks` (cylinders, heads, sectors) as hard
    /// disks of the user-defined type 47, as DOSBox-X's CMOS has them for
    /// a booted system's Windows 95: 12h's high nibble the first, low the
    /// second, the types at 19h and 1Ah, and their parameters at 1Bh and
    /// 24h (no write precompensation, landing zone the last cylinder).
    pub fn set_hard_disks(&mut self, disks: &[crate::diskimage::Chs]) {
        self.ram[0x12] = 0;
        self.ram[0x19..=0x2C].fill(0);
        for (i, chs) in disks.iter().take(2).enumerate() {
            self.ram[0x12] |= if i == 0 { 0xF0 } else { 0x0F };
            self.ram[0x19 + i] = 47;
            let cylinders = chs.cylinders.min(1024) as u16;
            let heads = chs.heads.min(255) as u8;
            let at = if i == 0 { 0x1B } else { 0x24 };
            let p = &mut self.ram[at..at + 9];
            p[0..2].copy_from_slice(&cylinders.to_le_bytes());
            p[2] = heads;
            p[3..5].copy_from_slice(&0xFFFFu16.to_le_bytes());
            p[5] = 0xC0 | ((heads > 8) as u8) << 3;
            p[6..8].copy_from_slice(&cylinders.to_le_bytes());
            p[8] = chs.sectors.min(63) as u8;
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

    /// The machine's local date and time: the host's, moved by what DOS
    /// or the BIOS set it to.
    pub fn now(&self) -> NaiveDateTime {
        crate::hosttime::now().naive_local() + self.offset
    }

    /// Set the machine's clock to `at`, a local date and time.
    pub fn set_now(&mut self, at: NaiveDateTime) {
        self.offset = at - crate::hosttime::now().naive_local();
    }

    /// The periodic interrupt's period in PIT ticks, when status B enables
    /// it and status A has a rate.
    pub fn periodic_ticks(&self) -> Option<u64> {
        let rate = self.ram[0x0A] & 0x0F;
        if self.ram[0x0B] & PIE == 0 || rate == 0 {
            return None;
        }
        // Rates 1 and 2 are 256 and 128 Hz; from 3 on, 32768 Hz halved
        // for each step past 1.
        let hz = if rate <= 2 { 32768 >> (rate + 6) } else { 32768 >> (rate - 1) };
        Some((crate::timer::PIT_HZ / hz).max(1))
    }

    /// A periodic interrupt came: true when it asserts IRQ 8, which it
    /// does again only once status C was read.
    pub fn periodic_fired(&mut self) -> bool {
        let asserted = self.ram[0x0C] & IRQF != 0;
        self.ram[0x0C] |= IRQF | PF;
        !asserted
    }

    /// Port 71h read. Status C reads clear it, acknowledging the interrupt.
    pub fn read_register(&mut self) -> u8 {
        let value = self.read_data();
        if self.index == 0x0C {
            self.ram[0x0C] = 0;
        }
        value
    }

    pub fn read_data(&self) -> u8 {
        let now = self.now();
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

impl crate::savestate::State for Cmos {
    fn save(&self, w: &mut crate::savestate::Writer) {
        self.index.save(w);
        self.ram.save(w);
        self.offset.num_milliseconds().save(w);
    }
    fn load(&mut self, r: &mut crate::savestate::Reader) -> crate::savestate::Result<()> {
        self.index.load(r)?;
        self.ram.load(r)?;
        let mut ms = 0i64;
        ms.load(r)?;
        self.offset = TimeDelta::milliseconds(ms);
        Ok(())
    }
}
