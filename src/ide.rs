//! An ATAPI CD-ROM drive on the secondary IDE channel (ports 170h-177h
//! and 376h/377h, IRQ 15), for systems booted from a disk image: Windows
//! 95's protected-mode IDE driver (ESDI_506.PDR) and CDFS, or a DOS ATAPI
//! driver, find the CD image of the machine's CD-ROM drive there. Booted
//! hard disks stay on INT 13h, so the channel has the CD drive alone, as
//! master.
//!
//! A port of the ATAPI half of DOSBox-X's ide.cpp: the task file and its
//! status bits, the packet protocol with its data blocks and interrupts,
//! the timing of commands (0.25 ms to take a packet, 1 ms for most
//! commands, 3 ms a read), the disc's spin-up and a new disc's insertion,
//! and the packet commands Windows 95 uses. Beyond DOSBox-X: the ATAPI
//! signature after a soft reset, READ CAPACITY's last sector (not the
//! lead-out), sense data for unknown commands, and the relative address of
//! READ SUB-CHANNEL.

use crate::cdrom::audio::{CdPlayer, PlayState};
use crate::cdrom::image::CdImage;
use crate::cdrom::{DATA_SECTOR, LBA_OFFSET, RAW_SECTOR, lba_to_msf};
use crate::timer::PIT_HZ;
use std::rc::Rc;

/// The channel's ports, interrupt and Plug and Play node handle (where
/// DOSBox-X has the secondary channel, so an installed Windows 95 knows
/// it).
pub const BASE: u16 = 0x170;
pub const ALT: u16 = 0x376;
pub const IRQ: u8 = 15;
pub const PNP_HANDLE: u8 = 0x10;

const BSY: u8 = 0x80;
const DRDY: u8 = 0x40;
const DSC: u8 = 0x10;
const DRQ: u8 = 0x08;
const ERR: u8 = 0x01;

/// The largest data block, as DOSBox-X's buffer.
const BUFFER: usize = 512 * 128;
/// Sectors a read transfers at most per data block.
const SECTORS_PER_BLOCK: u32 = 16;

/// Delays, in ms.
const PACKET_DELAY: f64 = 0.25;
const IDENTIFY_DELAY: f64 = 0.01;
const COMMAND_DELAY: f64 = 1.0;
const READ_DELAY: f64 = 3.0;
const SPINUP_RETRY: f64 = 100.0;
const INSERTION: f64 = 4000.0;
const SPINUP: f64 = 1000.0;
const SPINDOWN: f64 = 10000.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Ready,
    Busy,
    DataRead,
    DataWrite,
    Packet,
    AtapiBusy,
}

crate::state_enum!(State { State::Ready, State::Busy, State::DataRead, State::DataWrite, State::Packet, State::AtapiBusy });

/// What a delay leads to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Delayed {
    None,
    /// PACKET: ready for the command packet.
    Packet,
    /// IDENTIFY PACKET DEVICE's data.
    Identify,
    /// A packet command's work.
    Busy,
}

crate::state_enum!(Delayed { Delayed::None, Delayed::Packet, Delayed::Identify, Delayed::Busy });

/// Where the disc is: none, going in, still, spinning up, just ready,
/// ready.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Loading {
    NoDisc,
    InsertCd,
    Idle,
    SpinningUp,
    Readied,
    Ready,
}

crate::state_enum!(Loading {
    Loading::NoDisc,
    Loading::InsertCd,
    Loading::Idle,
    Loading::SpinningUp,
    Loading::Readied,
    Loading::Ready,
});

/// What a command works with: the disc, the CD audio player, the time.
pub struct Env<'a> {
    pub image: Option<Rc<CdImage>>,
    pub player: &'a mut CdPlayer,
    /// PIT ticks.
    pub now: u64,
}

/// The channel and its drive.
#[derive(Clone, Debug)]
pub struct Ide {
    /// The DOS drive whose CD image the drive has.
    pub drive: u8,
    // The controller: the selected device, interrupts disabled (nIEN),
    // the soft reset and the interrupt line.
    select: u8,
    nien: bool,
    host_reset: bool,
    line: bool,
    // The drive's task file.
    feature: u8,
    count: u8,
    lba: [u8; 3],
    drivehead: u8,
    command: u8,
    status: u8,
    state: State,
    allow_writing: bool,
    irq_signal: bool,
    // The packet and its data.
    cdb: [u8; 12],
    cdb_len: usize,
    max_bytes: u32,
    buf: Vec<u8>,
    buf_pos: usize,
    buf_len: usize,
    // A read in data blocks: the next sector, the sectors left, the
    // sectors of this block, and READ CD's sector size.
    lba_next: u32,
    remaining: u32,
    xfer: u32,
    sector_size: u32,
    sense: [u8; 18],
    loading: Loading,
    has_changed: bool,
    /// The delayed command, and when it is due (PIT ticks).
    delayed: Delayed,
    delayed_at: u64,
    /// The disc's next change of state (insertion done, spun up, spun
    /// down).
    loading_at: Option<u64>,
    /// The level of IRQ 15 the bus last passed to the interrupt
    /// controller.
    pub pic_line: bool,
    /// Messages for the log.
    pub log: Vec<String>,
}

crate::state_fields!(Ide {
    drive, select, nien, host_reset, line,
    feature, count, lba, drivehead, command, status, state, allow_writing, irq_signal,
    cdb, cdb_len, max_bytes, buf, buf_pos, buf_len,
    lba_next, remaining, xfer, sector_size, sense, loading, has_changed,
    delayed, delayed_at, loading_at, pic_line,
} skip { log });

fn ticks(ms: f64) -> u64 {
    (ms * PIT_HZ as f64 / 1000.0).ceil() as u64
}

fn be16(b: &[u8]) -> u32 {
    (b[0] as u32) << 8 | b[1] as u32
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

/// Bytes of READ CD's sectors: by expected sector type (byte 1 bits
/// 4:2, less 1, type 0 as 1) and the fields byte 9 asks for, bits 7:3
/// (DOSBox-X's `ReadCDTransferSectorSizeTable`).
const READ_CD_SIZE: [[u16; 32]; 5] = [
    [
        0, 0, 2352, 2352, 2352, 2352, 2352, 2352, 2352, 2352, 2352, 2352, 2352, 2352, 2352, 2352, 0, 0, 2352, 2352,
        2352, 2352, 2352, 2352, 2352, 2352, 2352, 2352, 2352, 2352, 2352, 2352,
    ],
    [
        0, 0, 2048, 2336, 4, 0, 2052, 2340, 0, 0, 2048, 2336, 4, 0, 2052, 2340, 0, 0, 0, 0, 16, 0, 2064, 2352, 0, 0, 0,
        0, 16, 0, 2064, 2352,
    ],
    [
        0, 0, 2336, 2336, 4, 0, 2340, 2340, 0, 0, 2336, 2336, 4, 4, 12, 12, 0, 0, 0, 0, 16, 0, 2352, 2352, 0, 0, 0, 0,
        16, 0, 2352, 2352,
    ],
    [
        0, 0, 2048, 2328, 4, 0, 0, 0, 8, 0, 2056, 2336, 12, 0, 2060, 2340, 0, 0, 0, 0, 16, 0, 0, 0, 0, 0, 0, 0, 24, 0,
        2072, 2352,
    ],
    [
        0, 0, 2328, 2328, 4, 0, 0, 0, 8, 0, 2336, 2336, 12, 0, 2340, 2340, 0, 0, 0, 0, 16, 0, 0, 0, 0, 0, 0, 0, 24, 0,
        2352, 2352,
    ],
];

impl Ide {
    /// The channel as a booted machine's BIOS leaves it, with the CD of
    /// `drive` in the drive (`has_disc`), standing still.
    pub fn new(drive: u8, has_disc: bool) -> Self {
        let mut ide = Self {
            drive,
            select: 0,
            nien: false,
            host_reset: false,
            line: false,
            feature: 0,
            count: 0,
            lba: [0; 3],
            drivehead: 0,
            command: 0,
            status: DRDY | DSC,
            state: State::Ready,
            allow_writing: true,
            irq_signal: false,
            cdb: [0; 12],
            cdb_len: 0,
            max_bytes: 0,
            buf: vec![0; BUFFER],
            buf_pos: 0,
            buf_len: 0,
            lba_next: 0,
            remaining: 0,
            xfer: 0,
            sector_size: 0,
            sense: [0; 18],
            loading: if has_disc { Loading::Idle } else { Loading::NoDisc },
            has_changed: false,
            delayed: Delayed::None,
            delayed_at: 0,
            loading_at: None,
            pic_line: false,
            log: Vec::new(),
        };
        ide.set_sense(0, 0, 0);
        ide
    }

    /// The interrupt line (IRQ 15).
    pub fn irq(&self) -> bool {
        self.line
    }

    /// When the channel next needs attention (PIT ticks).
    pub fn next_event(&self) -> Option<u64> {
        let delayed = (self.delayed != Delayed::None).then_some(self.delayed_at);
        [delayed, self.loading_at].into_iter().flatten().min()
    }

    /// Carry out what came due.
    pub fn service(&mut self, env: &mut Env) {
        if let Some(at) = self.loading_at
            && at <= env.now
        {
            self.loading_at = None;
            match self.loading {
                Loading::InsertCd => {
                    self.loading = Loading::SpinningUp;
                    self.loading_at = Some(env.now + ticks(SPINUP));
                }
                Loading::SpinningUp => {
                    self.loading = Loading::Readied;
                    self.loading_at = Some(env.now + ticks(SPINDOWN));
                }
                Loading::Readied | Loading::Ready => self.loading = Loading::Idle,
                _ => {}
            }
        }
        if self.delayed != Delayed::None && self.delayed_at <= env.now {
            let delayed = std::mem::replace(&mut self.delayed, Delayed::None);
            self.run_delayed(delayed, env);
        }
    }

    /// The disc changed (`has_disc`: a new one went in; otherwise it was
    /// taken out): the drive reports it as a real one does, not present
    /// while it goes in, then spinning up, then changed.
    pub fn media_changed(&mut self, has_disc: bool, now: u64) {
        self.has_changed = true;
        if has_disc {
            self.loading = Loading::InsertCd;
            self.loading_at = Some(now + ticks(INSERTION));
        } else {
            self.loading = Loading::NoDisc;
            self.loading_at = None;
        }
    }

    fn schedule(&mut self, delayed: Delayed, ms: f64, now: u64) {
        self.delayed = delayed;
        self.delayed_at = now + ticks(ms);
    }

    // --- The interrupt line ---

    fn raise_irq(&mut self) {
        self.irq_signal = true;
        self.check_irq();
    }

    fn lower_irq(&mut self) {
        self.irq_signal = false;
        self.check_irq();
    }

    fn check_irq(&mut self) {
        self.line = self.select == 0 && self.irq_signal && !self.nien;
    }

    // --- Ports ---

    /// A byte read at `port` (170h-177h, 376h-377h).
    pub fn read(&mut self, port: u16, env: &mut Env) -> u8 {
        if port == ALT || port == ALT + 1 {
            return self.read_alt(port);
        }
        if self.select != 0 {
            // No slave.
            return if port == BASE { 0xFF } else { 0 };
        }
        let reg = if self.status & BSY != 0 { 7 } else { port - BASE };
        match reg {
            0 => self.data_read(1, env) as u8,
            1 => self.feature,
            2 => self.count,
            3 => self.lba[0],
            4 => self.lba[1],
            5 => self.lba[2],
            6 => self.drivehead,
            _ => {
                // Reading the status clears the interrupt.
                if self.status & BSY == 0 {
                    self.lower_irq();
                }
                self.status
            }
        }
    }

    fn read_alt(&self, port: u16) -> u8 {
        let present = self.select == 0;
        if port == ALT {
            // The status, leaving the interrupt alone.
            if present { self.status } else { 0 }
        } else {
            // The drive address register.
            let heads = if present { ((self.drivehead & 0xF) ^ 0xF) << 2 } else { 0x3C };
            0x80 | (self.select != 0) as u8 | ((self.select != 1) as u8) << 1 | heads
        }
    }

    /// The data port read 2 or 4 bytes wide. A 32-bit read is split on
    /// the ISA bus, as DOSBox-X has it: the data port, then 172h.
    pub fn read_wide(&mut self, len: u8, env: &mut Env) -> u32 {
        if len == 4 {
            let low = self.read_wide(2, env);
            return low | (self.read(BASE + 2, env) as u32) << 16;
        }
        if self.select != 0 {
            return 0xFFFF;
        }
        if self.status & BSY != 0 {
            return self.read(BASE + 7, env) as u32;
        }
        self.data_read(2, env)
    }

    /// A byte written at `port`.
    pub fn write(&mut self, port: u16, value: u8, env: &mut Env) {
        if port == ALT {
            self.write_control(value);
            return;
        }
        if port == ALT + 1 {
            return;
        }
        let reg = port - BASE;
        if self.select == 0 && self.status & BSY != 0 {
            // Busy: drivers that select the same drive again are let be;
            // anything else is dropped.
            return;
        }
        match reg {
            0 => {
                if self.select == 0 {
                    self.data_write(value as u32, 1, env);
                }
            }
            1..=5 => {
                if self.select == 0 && self.allow_writing {
                    match reg {
                        1 => self.feature = value,
                        2 => self.count = value,
                        _ => self.lba[(reg - 3) as usize] = value,
                    }
                }
            }
            6 => {
                self.select = (value >> 4) & 1;
                if self.select == 0 && self.allow_writing {
                    self.drivehead = value;
                }
                self.check_irq();
            }
            _ => {
                if self.select == 0 {
                    self.command(value, env);
                }
            }
        }
    }

    /// The data port written 2 or 4 bytes wide (4: the data port, then
    /// 172h).
    pub fn write_wide(&mut self, value: u32, len: u8, env: &mut Env) {
        if len == 4 {
            self.write_wide(value & 0xFFFF, 2, env);
            self.write(BASE + 2, (value >> 16) as u8, env);
            return;
        }
        if self.select == 0 && self.status & BSY == 0 {
            self.data_write(value, 2, env);
        }
    }

    /// The device control register (376h): nIEN, and SRST, the soft reset
    /// that puts the ATAPI signature in the task file.
    fn write_control(&mut self, value: u8) {
        self.nien = value & 2 != 0;
        self.check_irq();
        let reset = value & 4 != 0;
        if reset && !self.host_reset {
            self.status = 0xFF;
            self.allow_writing = true;
            self.state = State::Busy;
            self.delayed = Delayed::None;
            self.host_reset = true;
        } else if !reset && self.host_reset {
            self.allow_writing = true;
            self.state = State::Ready;
            // Diagnostics passed, and the signature of a packet device.
            self.feature = 0x01;
            self.signature();
            self.status = DRDY | DSC;
            self.host_reset = false;
        }
    }

    fn signature(&mut self) {
        self.count = 0x01;
        self.lba = [0x01, 0x14, 0xEB];
    }

    // --- Commands ---

    fn abort_error(&mut self) {
        self.state = State::Ready;
        self.allow_writing = true;
        self.command = 0;
        self.status = ERR | DRDY | DSC;
    }

    /// Whether command `cmd` may begin (`command_interruption_ok`).
    fn may_begin(&mut self, cmd: u8) -> bool {
        if cmd == self.command {
            return true;
        }
        if self.state != State::Ready && self.state != State::Busy && cmd == 0x08 {
            self.state = State::Ready;
            self.allow_writing = true;
            self.command = 0;
            self.status = ERR | DRDY | DSC;
            self.delayed = Delayed::None;
            return true;
        }
        if self.state != State::Ready {
            self.abort_error();
            self.delayed = Delayed::None;
            return false;
        }
        true
    }

    fn command(&mut self, cmd: u8, env: &mut Env) {
        if !self.may_begin(cmd) {
            return;
        }
        self.allow_writing = false;
        self.command = cmd;
        match cmd {
            // DEVICE RESET: no interrupt.
            0x08 => {
                self.status = 0;
                self.drivehead &= 0x10;
                self.feature = 0x01;
                self.signature();
                self.allow_writing = true;
            }
            // READ SECTOR and IDENTIFY DEVICE: aborted, with the signature
            // of a packet device, which is how Windows 95 tells one.
            0x20 | 0xEC => {
                self.state = State::Ready;
                self.command = 0;
                self.status = ERR | DRDY;
                self.drivehead &= 0x30;
                self.feature = 0x04;
                self.signature();
                self.raise_irq();
                self.allow_writing = true;
            }
            0xA0 => {
                if self.feature & 1 != 0 {
                    // No DMA.
                    self.abort_error();
                    self.count = 0x03;
                    self.feature = 0xF4;
                    self.raise_irq();
                } else {
                    self.state = State::Busy;
                    self.status = BSY;
                    self.max_bytes = match (self.lba[2] as u32) << 8 | self.lba[1] as u32 {
                        0 => 0x10000,
                        n => n,
                    };
                    self.schedule(Delayed::Packet, PACKET_DELAY, env.now);
                }
            }
            0xA1 => {
                self.state = State::Busy;
                self.status = BSY;
                self.schedule(Delayed::Identify, IDENTIFY_DELAY, env.now);
            }
            // SET FEATURES: transfer modes and power-on defaults.
            0xEF => {
                if matches!(self.feature, 0x66 | 0xCC | 0x03) {
                    self.status = DRDY | DSC;
                    self.state = State::Ready;
                } else {
                    self.abort_error();
                }
                self.allow_writing = true;
                self.raise_irq();
            }
            _ => {
                self.abort_error();
                self.allow_writing = true;
                self.count = 0x03;
                self.feature = 0xF4;
                self.raise_irq();
            }
        }
    }

    fn run_delayed(&mut self, delayed: Delayed, env: &mut Env) {
        match delayed {
            Delayed::Packet => {
                self.state = State::Packet;
                self.status = DRDY | DSC | DRQ;
                // Command/data 1, input/output 0: the packet, please. No
                // interrupt.
                self.count = 0x01;
                self.cdb_len = 0;
            }
            Delayed::Identify => {
                self.identify();
                self.prepare_read(512);
                self.state = State::DataRead;
                self.status = DRQ | DRDY | DSC;
                self.raise_irq();
            }
            Delayed::Busy => self.busy_time(env),
            Delayed::None => {}
        }
    }

    fn prepare_read(&mut self, len: usize) {
        self.buf_pos = 0;
        self.buf_len = len.min(BUFFER);
    }

    /// Data from the drive (`data_read`).
    fn data_read(&mut self, len: u8, env: &mut Env) -> u32 {
        if self.state != State::DataRead || self.status & DRQ == 0 || self.buf_pos >= self.buf_len {
            return 0xFFFF;
        }
        let mut value = 0u32;
        for i in 0..len.min(4) as usize {
            value |= (*self.buf.get(self.buf_pos + i).unwrap_or(&0) as u32) << (8 * i);
        }
        self.buf_pos += len as usize;
        if self.buf_pos >= self.buf_len {
            self.io_completion(env);
        }
        value
    }

    /// Data to the drive: the command packet, or MODE SELECT's page data.
    fn data_write(&mut self, value: u32, len: u8, env: &mut Env) {
        if self.state == State::Packet {
            for i in 0..len as usize {
                if self.cdb_len < 12 {
                    self.cdb[self.cdb_len] = (value >> (8 * i)) as u8;
                    self.cdb_len += 1;
                }
            }
            if self.cdb_len >= 12 {
                self.packet(env);
            }
            return;
        }
        if self.state != State::DataWrite || self.status & DRQ == 0 || self.buf_pos + len as usize > self.buf_len {
            return;
        }
        for i in 0..len as usize {
            self.buf[self.buf_pos + i] = (value >> (8 * i)) as u8;
        }
        self.buf_pos += len as usize;
        if self.buf_pos >= self.buf_len {
            self.io_completion(env);
        }
    }

    /// A data block went through.
    fn io_completion(&mut self, env: &mut Env) {
        self.status &= !DRQ;
        if self.command != 0xA0 {
            self.status = DRDY | DSC;
            self.state = State::Ready;
            self.allow_writing = true;
            self.count = 0x03;
            return;
        }
        if self.count != 0x03 && matches!(self.cdb[0], 0x28 | 0xA8 | 0xBE) {
            // A read goes on with the next block, as large as the last.
            let size = if self.cdb[0] == 0xBE { self.sector_size.max(1) } else { DATA_SECTOR as u32 };
            let last = (self.lba[1] as u32 | (self.lba[2] as u32) << 8) / size;
            self.xfer = self.remaining.min(SECTORS_PER_BLOCK).min(BUFFER as u32 / size).min(last);
            self.remaining -= self.xfer;
            if self.xfer != 0 {
                self.count = 0x02;
                self.state = State::AtapiBusy;
                self.status = BSY;
                self.schedule(Delayed::Busy, READ_DELAY, env.now);
                return;
            }
        }
        self.count = 0x03;
        self.status = DRDY | DSC;
        self.state = State::Ready;
        self.allow_writing = true;
        // Drives interrupt again at the end of the transfer, and DOS
        // drivers wait for it.
        self.raise_irq();
    }

    fn set_sense(&mut self, key: u8, asc: u8, ascq: u8) {
        self.sense = [0; 18];
        self.sense[0] = 0x70;
        self.sense[2] = key & 0xF;
        self.sense[7] = 10;
        self.sense[12] = asc;
        self.sense[13] = ascq;
    }

    /// End a command with the error its sense data says.
    fn sense_error(&mut self) {
        let key = self.sense[2] & 0xF;
        self.count = 0x03;
        self.state = State::Ready;
        self.feature = key << 4 | if key != 0 { 0x04 } else { 0 };
        self.status = DRDY | if key != 0 { ERR } else { DSC };
        self.raise_irq();
        self.allow_writing = true;
    }

    /// Whether the disc can be used now: spinning it up if `trigger`,
    /// reporting a disc not there, one becoming ready (unless the command
    /// `wait`s for it) and one that changed (`common_spinup_response`).
    fn disc_ready(&mut self, trigger: bool, wait: bool, env: &Env) -> bool {
        match self.loading {
            Loading::Idle if trigger => {
                self.loading = Loading::SpinningUp;
                self.loading_at = Some(env.now + ticks(SPINUP));
            }
            Loading::Ready if trigger => self.loading_at = Some(env.now + ticks(SPINDOWN)),
            _ => {}
        }
        if env.image.is_none() {
            self.set_sense(0x02, 0x3A, 0);
            return false;
        }
        match self.loading {
            Loading::NoDisc | Loading::InsertCd => {
                self.set_sense(0x02, 0x3A, 0);
                false
            }
            Loading::SpinningUp if self.has_changed && !wait => {
                self.set_sense(0x02, 0x04, 0x01);
                false
            }
            Loading::Readied => {
                self.loading = Loading::Ready;
                if self.has_changed {
                    if trigger {
                        self.has_changed = false;
                    }
                    self.set_sense(0x02, 0x28, 0);
                    return false;
                }
                true
            }
            _ => true,
        }
    }

    /// The command packet arrived (`atapi_cmd_completion`).
    fn packet(&mut self, env: &mut Env) {
        let op = self.cdb[0];
        match op {
            0x00 => {
                if self.disc_ready(false, false, env) {
                    self.set_sense(0, 0, 0);
                }
                self.sense_error();
            }
            // Commands that need the disc, spun up.
            0x28 | 0xA8 | 0xBE | 0x2B | 0x42 | 0x43 | 0x45 | 0x47 | 0x4B | 0x4E | 0xA5 => {
                if !self.disc_ready(true, true, env) {
                    self.sense_error();
                    return;
                }
                self.set_sense(0, 0, 0);
                let delay = match op {
                    0x28 | 0xA8 | 0xBE => {
                        self.start_read();
                        READ_DELAY
                    }
                    _ => COMMAND_DELAY,
                };
                self.count = 0x02;
                self.state = State::AtapiBusy;
                self.status = BSY;
                self.schedule(Delayed::Busy, delay, env.now);
            }
            0x03 | 0x12 | 0x1B | 0x1E | 0x25 | 0x55 | 0x5A | 0xBD => {
                self.count = if op == 0x55 { 0x00 } else { 0x02 };
                self.state = State::AtapiBusy;
                self.status = BSY;
                self.schedule(Delayed::Busy, COMMAND_DELAY, env.now);
            }
            _ => {
                self.log.push(format!("[IDE] Unknown ATAPI command {:02X?}", self.cdb));
                self.set_sense(0x05, 0x20, 0);
                self.abort_error();
                self.count = 0x03;
                self.feature = 0xF4;
                self.raise_irq();
                self.allow_writing = true;
            }
        }
    }

    /// READ(10), READ(12) and READ CD: the sectors, and the first data
    /// block's, as large as the byte count the host allows.
    fn start_read(&mut self) {
        let cdb = self.cdb;
        let limit = self.lba[1] as u32 | (self.lba[2] as u32) << 8;
        self.lba_next = be32(&cdb[2..6]);
        let (count, size) = match cdb[0] {
            0x28 => (be16(&cdb[7..9]), DATA_SECTOR as u32),
            0xA8 => (be32(&cdb[6..10]), DATA_SECTOR as u32),
            _ => {
                let kind = ((cdb[1] >> 2) & 7) as usize;
                let mut size =
                    if kind <= 5 { READ_CD_SIZE[kind.max(1) - 1][(cdb[9] >> 3) as usize] as u32 } else { 0 };
                if size > 0 {
                    if cdb[9] & 4 != 0 {
                        size += 296;
                    } else if cdb[9] & 2 != 0 {
                        size += 294;
                    }
                }
                self.sector_size = size;
                ((cdb[6] as u32) << 16 | (cdb[7] as u32) << 8 | cdb[8] as u32, size)
            }
        };
        self.remaining = if size == 0 { 0 } else { count };
        self.xfer = self.remaining.min(SECTORS_PER_BLOCK).min(BUFFER as u32 / size.max(1)).min(limit / size.max(1));
        self.remaining -= self.xfer;
    }

    /// A packet command's work, after its delay (`on_atapi_busy_time`).
    fn busy_time(&mut self, env: &mut Env) {
        let op = self.cdb[0];
        // Commands wait while the disc spins up.
        let immediate = matches!(op, 0x00 | 0x03 | 0x12);
        if self.loading == Loading::SpinningUp && !immediate {
            self.schedule(Delayed::Busy, SPINUP_RETRY, env.now);
            return;
        }
        if self.loading == Loading::Readied && !immediate && !self.disc_ready(true, false, env) {
            self.sense_error();
            return;
        }
        let mut data = true;
        match op {
            0x03 => {
                let len = 18.min(self.max_bytes as usize);
                self.buf[..18].copy_from_slice(&self.sense);
                self.prepare_read(len);
            }
            0x12 => {
                self.inquiry();
                self.prepare_read(36.min(self.max_bytes as usize));
            }
            0x25 => {
                // The last sector's address, and the sector size.
                let last = env.image.as_ref().map_or(0, |i| i.leadout().saturating_sub(1));
                self.buf[..4].copy_from_slice(&last.to_be_bytes());
                self.buf[4..8].copy_from_slice(&(DATA_SECTOR as u32).to_be_bytes());
                self.prepare_read(8.min(self.max_bytes as usize));
            }
            0x28 | 0xA8 | 0xBE => {
                if !self.read_sectors(env) {
                    return;
                }
            }
            0x42 => self.read_subchannel(env),
            0x43 => self.read_toc(env),
            0x5A => self.mode_sense(),
            0xBD => {
                self.buf[..8].fill(0);
                self.prepare_read(8.min(self.max_bytes as usize));
            }
            0x55 => {
                // The mode pages are taken and ignored (Windows 95 asks
                // to send FFFFh bytes; 512 will do).
                let len = (self.lba[1] as u32 | (self.lba[2] as u32) << 8).min(512);
                self.lba[1] = len as u8;
                self.lba[2] = (len >> 8) as u8;
                self.buf_pos = 0;
                self.buf_len = ((len + 1) & !1) as usize;
                self.feature = 0;
                self.state = State::DataWrite;
                self.status = DRDY | DRQ | DSC;
                self.raise_irq();
                self.allow_writing = true;
                return;
            }
            _ => {
                self.no_data_command(env);
                data = false;
            }
        }
        if data {
            self.feature = 0;
            self.state = State::DataRead;
            self.status = DRDY | DRQ | DSC;
            self.lba[1] = self.buf_len as u8;
            self.lba[2] = (self.buf_len >> 8) as u8;
            self.raise_irq();
            self.allow_writing = true;
        }
    }

    /// The commands without data: start/stop, lock, seek, play, pause.
    fn no_data_command(&mut self, env: &mut Env) {
        let cdb = self.cdb;
        let image = env.image.clone();
        match cdb[0] {
            0x1B => {
                if cdb[4] & 3 == 2 {
                    self.log.push("[IDE] The CD-ROM drive is asked to eject; the disc stays in".to_string());
                }
            }
            // SEEK stops CD audio: Windows 95's CD Player stops playing
            // with it.
            0x2B => {
                if env.player.state() == PlayState::Playing {
                    env.player.reset();
                }
            }
            0x45 | 0xA5 | 0x47 => {
                let (start, end) = match cdb[0] {
                    0x45 => {
                        let start = be32(&cdb[2..6]);
                        (start, start.wrapping_add(be16(&cdb[7..9])))
                    }
                    0xA5 => {
                        let start = be32(&cdb[2..6]);
                        (start, start.wrapping_add(be32(&cdb[6..10])))
                    }
                    _ => {
                        let msf = |m: u8, s: u8, f: u8| {
                            if (m, s, f) == (0xFF, 0xFF, 0xFF) {
                                u32::MAX
                            } else {
                                ((m as u32 * 60 + s as u32) * 75 + f as u32).saturating_sub(LBA_OFFSET)
                            }
                        };
                        (msf(cdb[3], cdb[4], cdb[5]), msf(cdb[6], cdb[7], cdb[8]))
                    }
                };
                if start == u32::MAX {
                    env.player.resume();
                } else if let Some(image) = image
                    && end > start
                {
                    env.player.play(self.drive, image, start, end - start);
                }
            }
            0x4B => {
                if cdb[8] & 1 != 0 {
                    env.player.resume();
                } else if env.player.state() == PlayState::Playing {
                    env.player.stop();
                }
            }
            0x4E => env.player.reset(),
            _ => {}
        }
        self.count = 0x03;
        self.feature = 0;
        self.buf_len = 0;
        self.state = State::DataRead;
        self.status = DRDY | DSC;
        self.lba[1] = 0;
        self.lba[2] = 0;
        self.raise_irq();
        self.allow_writing = true;
    }

    /// A data block of sectors. False if the command ended with an error.
    fn read_sectors(&mut self, env: &mut Env) -> bool {
        if self.xfer == 0 {
            // Reading nothing is allowed; MSCDEX tests the drive that way.
            self.feature = 0;
            self.count = 0x03;
            self.buf_len = 0;
            self.state = State::Ready;
            self.status = DRDY;
            self.lba[1] = 0;
            self.lba[2] = 0;
            self.raise_irq();
            self.allow_writing = true;
            return false;
        }
        let raw = self.cdb[0] == 0xBE && self.sector_size == RAW_SECTOR as u32;
        let size = if raw { RAW_SECTOR } else { DATA_SECTOR };
        let supported = self.cdb[0] != 0xBE || matches!(self.sector_size as usize, DATA_SECTOR | RAW_SECTOR);
        let mut ok = supported && env.image.is_some();
        if ok {
            let image = env.image.clone().unwrap();
            for i in 0..self.xfer as usize {
                let lba = self.lba_next + i as u32;
                let at = i * size;
                let read = if raw {
                    let mut sector = [0u8; RAW_SECTOR];
                    image.read_raw(lba, &mut sector).map(|()| self.buf[at..at + RAW_SECTOR].copy_from_slice(&sector))
                } else {
                    let mut sector = [0u8; DATA_SECTOR];
                    image.read_data(lba, &mut sector).map(|()| self.buf[at..at + DATA_SECTOR].copy_from_slice(&sector))
                };
                if read.is_err() {
                    ok = false;
                    break;
                }
            }
        }
        if !ok {
            if env.image.is_none() {
                self.set_sense(0x02, 0x3A, 0);
            } else {
                self.set_sense(0x03, 0x11, 0);
            }
            self.feature = 0xF4;
            self.count = 0x03;
            self.buf_len = 0;
            self.remaining = 0;
            self.xfer = 0;
            self.state = State::Ready;
            self.status = DRDY | ERR;
            self.lba[1] = 0;
            self.lba[2] = 0;
            self.raise_irq();
            self.allow_writing = true;
            return false;
        }
        self.prepare_read((self.xfer as usize * size).min(self.max_bytes as usize));
        self.lba_next += self.xfer;
        self.count = 0x02;
        true
    }

    /// READ SUB-CHANNEL: the current position of CD audio.
    fn read_subchannel(&mut self, env: &mut Env) {
        let cdb = self.cdb;
        let list = cdb[3];
        self.buf[..24].fill(0);
        if list != 1 {
            self.prepare_read(8.min(self.max_bytes as usize));
            return;
        }
        let playing = env.player.drive() == Some(self.drive);
        let status = match env.player.state() {
            PlayState::Playing if playing => 0x11,
            PlayState::Paused if playing => 0x12,
            _ => 0x13,
        };
        let position = if playing { env.player.position() } else { 0 };
        let track = env.image.as_ref().and_then(|i| i.track_at(position).cloned());
        let (audio, number, index, relative) = match &track {
            Some(t) if position < t.start => (t.is_audio(), t.number, 0u8, 0),
            Some(t) => (t.is_audio(), t.number, 1u8, position - t.start),
            None => (false, 0, 0, 0),
        };
        let b = &mut self.buf;
        b[1] = status;
        let mut len = 4;
        if cdb[2] & 0x40 != 0 {
            b[4] = 0x01;
            b[5] = if audio { 0x10 } else { 0x14 };
            b[6] = number;
            b[7] = index;
            if cdb[1] & 2 != 0 {
                let (m, s, f) = lba_to_msf(position);
                b[8..12].copy_from_slice(&[0, m, s, f]);
                // The time into the track, a duration.
                let (m, s, f) = (relative / 75 / 60, relative / 75 % 60, relative % 75);
                b[12..16].copy_from_slice(&[0, m as u8, s as u8, f as u8]);
            } else {
                b[8..12].copy_from_slice(&position.to_be_bytes());
                b[12..16].copy_from_slice(&relative.to_be_bytes());
            }
            len = 16;
        }
        b[3] = (len - 4) as u8;
        self.prepare_read(len.min(self.max_bytes as usize));
    }

    /// READ TOC: the tracks (format 0), or the first session (format 1),
    /// no longer than the host allows (OAKCDROM.SYS rejects more).
    fn read_toc(&mut self, env: &mut Env) {
        let cdb = self.cdb;
        let allocation = be16(&cdb[7..9]) as usize;
        let format = cdb[2] & 0xF;
        let first_track = cdb[6];
        let msf = cdb[1] & 2 != 0;
        self.buf[..8].fill(0);
        let Some(image) = env.image.clone() else {
            self.prepare_read(8.min(self.max_bytes as usize));
            return;
        };
        let tracks = image.tracks();
        let address = |lba: u32| -> [u8; 4] {
            if msf {
                let (m, s, f) = lba_to_msf(lba);
                [0, m, s, f]
            } else {
                lba.to_be_bytes()
            }
        };
        let mut out = vec![0u8, 0];
        match format {
            0 => {
                out.push(tracks.first().map_or(1, |t| t.number));
                out.push(tracks.last().map_or(1, |t| t.number));
                for t in tracks.iter().filter(|t| t.number >= first_track) {
                    if out.len() + 8 > allocation {
                        break;
                    }
                    out.extend([0, if t.is_audio() { 0x10 } else { 0x14 }, t.number, 0]);
                    out.extend(address(t.start));
                }
                if out.len() + 8 <= allocation {
                    out.extend([0, 0x14, 0xAA, 0]);
                    out.extend(address(image.leadout()));
                }
            }
            1 => {
                out.extend([1, 1]);
                let t = tracks.first();
                out.extend([0, if t.is_some_and(|t| t.is_audio()) { 0x10 } else { 0x14 }, t.map_or(1, |t| t.number), 0]);
                out.extend(address(t.map_or(0, |t| t.start)));
            }
            _ => {
                self.log.push(format!("[IDE] READ TOC format {} isn't supported", format));
                self.prepare_read(8.min(self.max_bytes as usize));
                return;
            }
        }
        let len = out.len() - 2;
        out[0] = (len >> 8) as u8;
        out[1] = len as u8;
        self.buf[..out.len()].copy_from_slice(&out);
        self.prepare_read(out.len().min(self.max_bytes as usize).min(allocation));
    }

    /// MODE SENSE(10): the error recovery, audio control and capabilities
    /// pages.
    fn mode_sense(&mut self) {
        let page = self.cdb[2] & 0x3F;
        let mut out = vec![0u8; 8];
        out.extend([page, 0]);
        match page {
            0x01 => out.extend([0x00, 3, 0, 0, 0, 0, 0, 0, 0, 0]),
            0x0E => out.extend([0x04, 0, 0, 0, 0, 75, 0x01, 0xFF, 0x02, 0xFF, 0, 0, 0, 0]),
            0x2A => {
                out.extend([0x07, 0x00, 0x71, 0xFF, 0x2F, 0x03]);
                for v in [176u16 * 8, 256, 6 * 256, 176 * 8] {
                    out.extend(v.to_be_bytes());
                }
                out.extend([0, 0, 0, 0, 0, 0]);
            }
            _ => out.extend([0; 6]),
        }
        let len = out.len() - 2;
        out[0] = (len >> 8) as u8;
        out[1] = len as u8;
        out[9] = (out.len() - 10) as u8;
        self.buf[..out.len()].copy_from_slice(&out);
        self.prepare_read(out.len().min(self.max_bytes as usize));
    }

    fn inquiry(&mut self) {
        let b = &mut self.buf[..36];
        b.fill(0);
        b[0] = 0x05;
        b[1] = 0x80;
        b[3] = 0x21;
        b[4] = 36 - 5;
        let pad = |dst: &mut [u8], s: &str| {
            for (i, d) in dst.iter_mut().enumerate() {
                *d = *s.as_bytes().get(i).unwrap_or(&b' ');
            }
        };
        pad(&mut b[8..16], "RUST-DOS");
        pad(&mut b[16..32], "CD-ROM");
        pad(&mut b[32..36], "1.0");
    }

    /// IDENTIFY PACKET DEVICE's 512 bytes.
    fn identify(&mut self) {
        let b = &mut self.buf[..512];
        b.fill(0);
        let word = |b: &mut [u8], w: usize, v: u16| b[w * 2..w * 2 + 2].copy_from_slice(&v.to_le_bytes());
        // A removable packet device of type 5 (CD-ROM), 12-byte packets.
        word(b, 0, 0x85C0);
        let text = |b: &mut [u8], w: usize, len: usize, s: &str| {
            for i in 0..len {
                b[w * 2 + (i ^ 1)] = *s.as_bytes().get(i).unwrap_or(&b' ');
            }
        };
        text(b, 10, 20, "RDS0001");
        text(b, 23, 8, "1.0");
        text(b, 27, 40, "Rust-DOS ATAPI CD-ROM");
        word(b, 49, 0x0A00);
        word(b, 50, 0x4000);
        word(b, 51, 0x00F0);
        word(b, 52, 0x00F0);
        word(b, 53, 0x0006);
        word(b, 64, 0x0003);
        word(b, 67, 0x0078);
        word(b, 68, 0x0078);
        word(b, 80, 0x007E);
        word(b, 81, 0x0022);
        word(b, 82, 0x4008);
        word(b, 85, 0x4208);
        b[510] = 0xA5;
        let sum = b[..511].iter().fold(0u8, |s, &x| s.wrapping_add(x));
        b[511] = sum.wrapping_neg();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identify_carries_its_checksum_and_model() {
        let mut ide = Ide::new(3, true);
        ide.identify();
        assert_eq!(u16::from_le_bytes([ide.buf[0], ide.buf[1]]), 0x85C0);
        assert_eq!(ide.buf[..512].iter().fold(0u8, |s, &x| s.wrapping_add(x)), 0);
        // Words are big-endian pairs of characters.
        assert_eq!(&ide.buf[54..58], b"uRts");
    }

    #[test]
    fn read_cd_sector_sizes() {
        assert_eq!(READ_CD_SIZE[1][0x10 >> 3], 2048);
        assert_eq!(READ_CD_SIZE[0][0xF8 >> 3], 2352);
    }
}
