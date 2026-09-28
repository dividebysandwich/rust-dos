//! An ATA hard disk on an IDE channel, with a hard disk image of the
//! machine's: the image the BIOS's INT 13h has as the same disk, so what a
//! system's own IDE driver (Windows 9x's ESDI_506.PDR, Linux, a DOS ATA
//! driver) writes, INT 13h reads, and both go into the image's journal for
//! save states and rewind.
//!
//! A port of DOSBox-X's `IDEATADevice` (ide.cpp): the PIO commands with
//! 28-bit addresses by cylinder, head and sector or LBA, READ and WRITE
//! MULTIPLE, an interrupt for each data block and none after the last one
//! read, the address registers counting along, IDENTIFY DEVICE, and a
//! geometry of no more than 16 heads for the INT 13h one of more (as a
//! BIOS translates it). Beyond DOSBox-X: SEEK, EXECUTE DEVICE DIAGNOSTIC,
//! the power management commands and READ NATIVE MAX ADDRESS, the ID not
//! found error for a sector that isn't there, and reads and writes that
//! take as long as `hard_disk_speed` says.

use super::{BSY, DRDY, DRQ, DSC, ERR, Env, ticks};
use crate::diskimage::{Chs, DiskImage, SECTOR_SIZE};
use std::rc::Rc;

/// The error register's bits: the command was aborted, or the sector
/// isn't there.
const ABRT: u8 = 0x04;
const IDNF: u8 = 0x10;

/// The most sectors READ and WRITE MULTIPLE move a data block.
pub const MULTIPLE_MAX: u8 = 16;

/// Delays, in ms: a command that reads or writes, the next block of one,
/// and IDENTIFY DEVICE.
const COMMAND_DELAY: f64 = 0.1;
const BLOCK_DELAY: f64 = 0.00001;
const IDENTIFY_DELAY: f64 = 0.01;

/// Where the geometry of a disk above 8.4 GB stops: 16383 cylinders of 16
/// heads of 63 sectors, as IDENTIFY reports it; the rest is reached by
/// LBA.
const CHS_MAX_SECTORS: u64 = 16383 * 16 * 63;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Ready,
    Busy,
    DataRead,
    DataWrite,
}

crate::state_enum!(State { State::Ready, State::Busy, State::DataRead, State::DataWrite });

/// A disk's cylinders, heads and sectors, as ATA addresses them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Geometry {
    pub cylinders: u32,
    pub heads: u32,
    pub sectors: u32,
}

crate::state_fields!(Geometry { cylinders, heads, sectors });

impl Geometry {
    /// The geometry of a disk of `total` sectors whose INT 13h geometry is
    /// `bios`: that one where it has 16 heads or fewer, else 16 heads of
    /// its sectors per track, as a BIOS's translation has it (1024/64/63
    /// is 4096/16/63).
    pub fn for_disk(bios: Chs, total: u64) -> Self {
        let sectors = bios.sectors.clamp(1, 63);
        let heads = bios.heads.clamp(1, 16);
        let most = if total > CHS_MAX_SECTORS { 16383 } else { 65535 };
        let cylinders = (total / (heads as u64 * sectors as u64)).clamp(1, most) as u32;
        Geometry { cylinders, heads, sectors }
    }

    fn total(self) -> u64 {
        self.cylinders as u64 * self.heads as u64 * self.sectors as u64
    }
}

/// The disk.
#[derive(Clone, Debug)]
pub struct Ata {
    /// The drive slot whose disk image the disk has.
    pub drive: u8,
    // The disk's task file.
    feature: u8,
    count: u8,
    lba: [u8; 3],
    drivehead: u8,
    command: u8,
    status: u8,
    state: State,
    allow_writing: bool,
    irq_signal: bool,
    /// Its sectors, its geometry as IDENTIFY reports it, and the one the
    /// system set with INITIALIZE DEVICE PARAMETERS, which CHS addresses
    /// go by.
    total: u64,
    physical: Geometry,
    logical: Geometry,
    /// READ and WRITE MULTIPLE's sectors a block (SET MULTIPLE MODE).
    multiple: u8,
    /// The data block and how far the host has come in it.
    buf: Vec<u8>,
    buf_pos: usize,
    buf_len: usize,
    /// Blocks of the command done.
    progress: u32,
    /// The work of the command, due at `delayed_at` (PIT ticks).
    delayed: bool,
    delayed_at: u64,
    /// Messages for the log.
    pub log: Vec<String>,
    /// Sectors read or written since the bus last looked: (first sector,
    /// sectors), for the disk's noise.
    pub activity: Vec<(u64, u32)>,
}

crate::state_fields!(Ata {
    drive, feature, count, lba, drivehead, command, status, state, allow_writing, irq_signal,
    total, physical, logical, multiple, buf, buf_pos, buf_len, progress, delayed, delayed_at,
} skip { log, activity });

impl Ata {
    /// The disk of drive slot `drive`, whose image has `total` sectors and
    /// the INT 13h geometry `bios`, as it is after power on.
    pub fn new(drive: u8, bios: Chs, total: u64) -> Self {
        let geometry = Geometry::for_disk(bios, total);
        let mut ata = Self {
            drive,
            feature: 0,
            count: 0,
            lba: [0; 3],
            drivehead: 0,
            command: 0,
            status: DRDY | DSC,
            state: State::Ready,
            allow_writing: true,
            irq_signal: false,
            total,
            physical: geometry,
            logical: geometry,
            multiple: MULTIPLE_MAX,
            buf: vec![0; MULTIPLE_MAX as usize * SECTOR_SIZE],
            buf_pos: 0,
            buf_len: 0,
            progress: 0,
            delayed: false,
            delayed_at: 0,
            log: Vec::new(),
            activity: Vec::new(),
        };
        ata.signature();
        ata
    }

    /// Its geometry as IDENTIFY reports it.
    pub fn geometry(&self) -> Geometry {
        self.physical
    }

    /// The geometry CHS addresses go by now (INITIALIZE DEVICE
    /// PARAMETERS').
    pub fn logical_geometry(&self) -> Geometry {
        self.logical
    }

    /// The task file as a BIOS leaves it after reading the sector the
    /// address registers `lba` and drive/head `drivehead` point at, at
    /// once (DOSBox-X's `int13fakeio`): Windows for Workgroups' WDCTRL
    /// reads it back after INT 13h to see the BIOS drives the disk.
    pub fn bios_read(&mut self, lba: [u8; 3], drivehead: u8) {
        self.feature = 0;
        self.count = 0;
        self.lba = lba;
        self.drivehead = drivehead;
        self.state = State::Ready;
        self.status = DRDY | DSC;
        self.allow_writing = true;
    }

    /// Withdraw the interrupt request, as a BIOS's handler does.
    pub fn clear_irq(&mut self) {
        self.lower_irq();
    }

    pub fn irq_signal(&self) -> bool {
        self.irq_signal
    }

    pub fn status(&self) -> u8 {
        self.status
    }

    pub fn drivehead(&self) -> u8 {
        self.drivehead
    }

    /// When the disk next needs attention (PIT ticks).
    pub fn next_event(&self) -> Option<u64> {
        self.delayed.then_some(self.delayed_at)
    }

    /// Carry out what came due.
    pub fn service(&mut self, env: &mut Env) {
        if self.delayed && self.delayed_at <= env.now {
            self.delayed = false;
            self.run_delayed(env);
        }
    }

    fn schedule(&mut self, ms: f64, env: &Env) {
        self.delayed = true;
        self.delayed_at = env.now + if env.faked { 1 } else { ticks(ms) };
    }

    fn raise_irq(&mut self) {
        self.irq_signal = true;
    }

    fn lower_irq(&mut self) {
        self.irq_signal = false;
    }

    fn signature(&mut self) {
        self.count = 0x01;
        self.lba = [0x01, 0x00, 0x00];
    }

    // --- Registers ---

    /// A byte read from task file register `reg` (0-7) while the disk is
    /// selected.
    pub fn read_register(&mut self, reg: u16, env: &mut Env) -> u8 {
        let reg = if self.status & BSY != 0 { 7 } else { reg };
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

    /// The data port read a word at a time.
    pub fn read_data(&mut self, env: &mut Env) -> u32 {
        if self.status & BSY != 0 {
            return self.read_register(7, env) as u32;
        }
        self.data_read(2, env)
    }

    /// A byte written to task file register `reg` (0-5, 7) while the disk
    /// is selected and not busy.
    pub fn write_register(&mut self, reg: u16, value: u8, env: &mut Env) {
        match reg {
            0 => self.data_write(value as u32, 1, env),
            1..=5 if self.allow_writing => match reg {
                1 => self.feature = value,
                2 => self.count = value,
                _ => self.lba[(reg - 3) as usize] = value,
            },
            7 => self.command(value, env),
            _ => {}
        }
    }

    /// The drive/head register written, selecting this disk.
    pub fn select(&mut self, value: u8) {
        if self.allow_writing {
            self.drivehead = value;
        }
    }

    /// The data port written a word at a time.
    pub fn write_data(&mut self, value: u32, env: &mut Env) {
        if self.status & BSY == 0 {
            self.data_write(value, 2, env);
        }
    }

    /// The soft reset (SRST) begins.
    pub fn reset_begin(&mut self) {
        self.status = 0xFF;
        self.allow_writing = true;
        self.state = State::Busy;
        self.delayed = false;
    }

    /// The soft reset ends: diagnostics passed, and the signature of a
    /// disk.
    pub fn reset_complete(&mut self) {
        self.allow_writing = true;
        self.state = State::Ready;
        self.feature = 0x01;
        self.signature();
        self.status = DRDY | DSC;
    }

    // --- Commands ---

    fn abort_error(&mut self, error: u8) {
        self.state = State::Ready;
        self.allow_writing = true;
        self.command = 0;
        self.delayed = false;
        self.feature = error;
        self.status = ERR | DRDY | DSC;
    }

    /// Whether command `cmd` may begin (`command_interruption_ok`).
    fn may_begin(&mut self, cmd: u8) -> bool {
        if cmd == self.command {
            return true;
        }
        if self.state != State::Ready && self.state != State::Busy && cmd == 0x08 {
            self.abort_error(ABRT);
            return true;
        }
        if self.state != State::Ready {
            self.abort_error(ABRT);
            return false;
        }
        true
    }

    /// End a command that moves no data.
    fn done(&mut self) {
        self.state = State::Ready;
        self.status = DRDY | DSC;
        self.allow_writing = true;
        self.raise_irq();
    }

    fn command(&mut self, cmd: u8, env: &mut Env) {
        if !self.may_begin(cmd) {
            return;
        }
        self.allow_writing = false;
        self.command = cmd;
        match cmd {
            // NOP: aborted, as it always is.
            0x00 => {
                self.abort_error(ABRT);
                self.raise_irq();
            }
            // DEVICE RESET, which Windows 95 wants an interrupt for.
            0x08 => {
                self.drivehead &= 0x10;
                self.feature = 0;
                self.signature();
                self.done();
            }
            // RECALIBRATE.
            0x10..=0x1F => {
                self.lba = [if self.lba_mode() { 0 } else { 1 }, 0, 0];
                self.drivehead &= 0x10;
                self.feature = 0;
                self.done();
            }
            0x20 | 0x21 | 0x40 | 0x41 | 0xC4 => {
                if cmd == 0xC4 && self.multiple == 0 {
                    self.abort_error(ABRT);
                    self.raise_irq();
                    return;
                }
                self.progress = 0;
                self.state = State::Busy;
                self.status = BSY;
                self.schedule(COMMAND_DELAY, env);
            }
            // The data first, without an interrupt.
            0x30 | 0x31 | 0xC5 => {
                if cmd == 0xC5 && self.multiple == 0 {
                    self.abort_error(ABRT);
                    self.raise_irq();
                    return;
                }
                self.progress = 0;
                self.state = State::DataWrite;
                self.status = DRDY | DRQ | DSC;
                let block = self.block_sectors();
                self.prepare(block * SECTOR_SIZE);
            }
            // SEEK: to a sector that is there.
            0x70 => {
                if self.address().is_ok() {
                    self.done();
                } else {
                    self.abort_error(IDNF);
                    self.raise_irq();
                }
            }
            // EXECUTE DEVICE DIAGNOSTIC: passed.
            0x90 => {
                self.feature = 0x01;
                self.signature();
                self.drivehead &= 0x10;
                self.done();
            }
            0x91 => self.initialize_device_parameters(),
            0xC6 => {
                let count = self.count;
                if count <= MULTIPLE_MAX && (count == 0 || count.is_power_of_two()) {
                    self.multiple = count;
                    self.done();
                } else {
                    self.abort_error(ABRT);
                    self.raise_irq();
                }
            }
            // PACKET and IDENTIFY PACKET DEVICE: not a packet device.
            // Windows 95 and DOS drivers ask every device both ways.
            0xA0 | 0xA1 => {
                self.abort_error(ABRT);
                self.status = ERR | DRDY | DSC | 0x20;
                self.drivehead &= 0x30;
                self.signature();
                self.raise_irq();
            }
            // The power management commands: always active and awake.
            0x94..=0x99 | 0xE0..=0xE6 => {
                if matches!(cmd, 0x98 | 0xE5) {
                    self.count = 0xFF;
                }
                self.done();
            }
            // FLUSH CACHE: there is none.
            0xE7 | 0xEA => self.done(),
            0xEC => {
                self.state = State::Busy;
                self.status = BSY;
                self.schedule(IDENTIFY_DELAY, env);
            }
            // SET FEATURES: transfer modes, power-on defaults, the write
            // cache and read look-ahead.
            0xEF => {
                if matches!(self.feature, 0x02 | 0x03 | 0x55 | 0x66 | 0x82 | 0xAA | 0xCC) {
                    self.done();
                } else {
                    self.abort_error(ABRT);
                    self.raise_irq();
                }
            }
            // READ NATIVE MAX ADDRESS: the last sector, by LBA.
            0xF8 => {
                let last = (self.total.min(0x1000_0000) - 1) as u32;
                self.lba = [last as u8, (last >> 8) as u8, (last >> 16) as u8];
                self.drivehead = (self.drivehead & 0xF0) | ((last >> 24) & 0x0F) as u8;
                self.done();
            }
            _ => {
                self.log.push(format!("[IDE] Unknown ATA command {:02X}", cmd));
                self.abort_error(ABRT);
                self.raise_irq();
            }
        }
    }

    /// INITIALIZE DEVICE PARAMETERS: the heads (drive/head register) and
    /// sectors per track (count) CHS addresses go by from now on.
    fn initialize_device_parameters(&mut self) {
        let sectors = self.count as u32;
        let heads = (self.drivehead & 0x0F) as u32 + 1;
        if sectors == 0 {
            self.abort_error(ABRT);
            self.raise_irq();
            return;
        }
        let cylinders = self.physical.total().div_ceil(sectors as u64 * heads as u64).min(65535) as u32;
        self.logical = Geometry { cylinders, heads, sectors };
        self.done();
    }

    /// Whether the drive/head register asks for LBA addresses.
    fn lba_mode(&self) -> bool {
        self.drivehead & 0x40 != 0
    }

    /// The sector the task file addresses: by LBA, or by cylinder, head
    /// and sector of the logical geometry. IDNF if it isn't there.
    fn address(&self) -> Result<u64, u8> {
        let lba = if self.lba_mode() {
            ((self.drivehead & 0x0F) as u64) << 24
                | (self.lba[2] as u64) << 16
                | (self.lba[1] as u64) << 8
                | self.lba[0] as u64
        } else {
            let g = self.logical;
            let (cylinder, head, sector) =
                ((self.lba[2] as u32) << 8 | self.lba[1] as u32, (self.drivehead & 0x0F) as u32, self.lba[0] as u32);
            if sector == 0 || sector > g.sectors || head >= g.heads || cylinder >= g.cylinders {
                return Err(IDNF);
            }
            (cylinder as u64 * g.heads as u64 + head as u64) * g.sectors as u64 + sector as u64 - 1
        };
        if lba < self.total { Ok(lba) } else { Err(IDNF) }
    }

    /// Move the task file on to the next sector
    /// (`increment_current_address`).
    fn advance(&mut self) {
        if self.lba_mode() {
            let lba = self.address().map_or(0, |a| a + 1);
            self.lba = [lba as u8, (lba >> 8) as u8, (lba >> 16) as u8];
            self.drivehead = (self.drivehead & 0xF0) | ((lba >> 24) & 0x0F) as u8;
            return;
        }
        let g = self.logical;
        self.lba[0] = self.lba[0].wrapping_add(1);
        if self.lba[0] as u32 > g.sectors {
            self.lba[0] = 1;
            let head = (self.drivehead & 0x0F) as u32 + 1;
            if head >= g.heads {
                self.drivehead &= 0xF0;
                let cylinder = ((self.lba[2] as u32) << 8 | self.lba[1] as u32) + 1;
                self.lba[1] = cylinder as u8;
                self.lba[2] = (cylinder >> 8) as u8;
            } else {
                self.drivehead = (self.drivehead & 0xF0) | head as u8;
            }
        }
    }

    /// Sectors the command still has to move: the count, 0 for 256.
    fn sectors_left(&self) -> usize {
        if self.count == 0 { 256 } else { self.count as usize }
    }

    /// Sectors of the command's next data block.
    fn block_sectors(&self) -> usize {
        match self.command {
            0xC4 | 0xC5 => (self.multiple as usize).max(1).min(self.sectors_left()),
            _ => 1,
        }
    }

    fn prepare(&mut self, len: usize) {
        self.buf_pos = 0;
        self.buf_len = len.min(self.buf.len());
    }

    /// Count off `sectors` of the command, moving the task file on: false
    /// at the end of it.
    fn count_off(&mut self, sectors: usize) -> bool {
        for _ in 0..sectors {
            self.progress += 1;
            if self.count == 1 {
                self.count = 0;
                return false;
            }
            self.count = self.count.wrapping_sub(1);
            self.advance();
        }
        true
    }

    /// The time moving `sectors` takes at the disk's speed, in ms, at
    /// least `least`.
    fn transfer_ms(env: &Env, sectors: usize, least: f64) -> f64 {
        (env.hard_disk_ns_per_sector as f64 * sectors as f64 / 1_000_000.0).max(least)
    }

    /// The disk image, and where the task file points on it.
    fn target(&self, env: &Env) -> Result<(Rc<DiskImage>, u64), u8> {
        let disk = env.disks.bios_image(self.drive).ok_or(ABRT)?;
        Ok((disk, self.address()?))
    }

    /// A delayed command's work.
    fn run_delayed(&mut self, env: &mut Env) {
        match self.command {
            0xEC => {
                self.identify();
                self.prepare(SECTOR_SIZE);
                self.state = State::DataRead;
                self.status = DRQ | DRDY | DSC;
                self.raise_irq();
            }
            0x20 | 0x21 | 0xC4 => self.read_block(env),
            0x40 | 0x41 => self.verify(env),
            0x30 | 0x31 | 0xC5 => self.write_block(env),
            _ => {}
        }
    }

    /// Read the next data block for the host, and interrupt.
    fn read_block(&mut self, env: &mut Env) {
        let sectors = self.block_sectors();
        let read = self.target(env).and_then(|(disk, lba)| {
            if lba + sectors as u64 > self.total {
                return Err(IDNF);
            }
            disk.read(lba, &mut self.buf[..sectors * SECTOR_SIZE]).map_err(|_| ABRT)?;
            Ok(lba)
        });
        match read {
            Ok(lba) => {
                self.activity.push((lba, sectors as u32));
                self.prepare(sectors * SECTOR_SIZE);
                self.state = State::DataRead;
                self.status = DRQ | DRDY | DSC;
                self.raise_irq();
            }
            Err(error) => {
                self.abort_error(error);
                self.raise_irq();
            }
        }
    }

    /// READ VERIFY: the sectors are read, and nothing is moved.
    fn verify(&mut self, env: &mut Env) {
        loop {
            let read = self.target(env).and_then(|(disk, lba)| {
                let mut sector = [0u8; SECTOR_SIZE];
                disk.read(lba, &mut sector).map_err(|_| ABRT)
            });
            if let Err(error) = read {
                self.abort_error(error);
                self.raise_irq();
                return;
            }
            if !self.count_off(1) {
                self.done();
                return;
            }
        }
    }

    /// Write the data block the host sent, and ask for the next one.
    fn write_block(&mut self, env: &mut Env) {
        let sectors = self.buf_len / SECTOR_SIZE;
        let written = self.target(env).and_then(|(disk, lba)| {
            if lba + sectors as u64 > self.total {
                return Err(IDNF);
            }
            disk.write(lba, &self.buf[..sectors * SECTOR_SIZE]).map_err(|_| ABRT)?;
            Ok(lba)
        });
        let lba = match written {
            Ok(lba) => lba,
            Err(error) => {
                self.abort_error(error);
                self.raise_irq();
                return;
            }
        };
        self.activity.push((lba, sectors as u32));
        if !self.count_off(sectors) {
            self.done();
            return;
        }
        self.state = State::DataWrite;
        self.status = DRQ | DRDY | DSC;
        let block = self.block_sectors();
        self.prepare(block * SECTOR_SIZE);
        self.raise_irq();
    }

    /// Data from the disk.
    fn data_read(&mut self, len: u8, env: &mut Env) -> u32 {
        if self.state != State::DataRead || self.status & DRQ == 0 || self.buf_pos >= self.buf_len {
            return 0xFFFF;
        }
        let mut value = 0u32;
        for i in 0..len as usize {
            value |= (*self.buf.get(self.buf_pos + i).unwrap_or(&0) as u32) << (8 * i);
        }
        self.buf_pos += len as usize;
        if self.buf_pos >= self.buf_len {
            self.io_completion(env);
        }
        value
    }

    /// Data to the disk.
    fn data_write(&mut self, value: u32, len: u8, env: &mut Env) {
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

    /// A data block went through: the next one of a read after a pause,
    /// with no interrupt at the end; a written one goes to the disk.
    fn io_completion(&mut self, env: &mut Env) {
        self.status &= !DRQ;
        match self.command {
            0x20 | 0x21 | 0xC4 => {
                let sectors = self.buf_len / SECTOR_SIZE;
                if !self.count_off(sectors) {
                    self.state = State::Ready;
                    self.status = DRDY | DSC;
                    self.allow_writing = true;
                    return;
                }
                self.state = State::Busy;
                self.status = BSY;
                let next = self.block_sectors();
                self.schedule(Self::transfer_ms(env, next, BLOCK_DELAY), env);
            }
            0x30 | 0x31 | 0xC5 => {
                self.state = State::Busy;
                self.status = BSY;
                let least = if self.progress == 0 { COMMAND_DELAY } else { BLOCK_DELAY };
                self.schedule(Self::transfer_ms(env, self.buf_len / SECTOR_SIZE, least), env);
            }
            _ => {
                // IDENTIFY's data.
                self.state = State::Ready;
                self.status = DRDY | DSC;
                self.allow_writing = true;
            }
        }
    }

    /// IDENTIFY DEVICE's 512 bytes, as DOSBox-X's
    /// (`generate_identify_device`).
    fn identify(&mut self) {
        let (p, l) = (self.physical, self.logical);
        let b = &mut self.buf[..SECTOR_SIZE];
        b.fill(0);
        let word = |b: &mut [u8], w: usize, v: u16| b[w * 2..w * 2 + 2].copy_from_slice(&v.to_le_bytes());
        let text = |b: &mut [u8], w: usize, len: usize, s: &str| {
            for i in 0..len {
                b[w * 2 + (i ^ 1)] = *s.as_bytes().get(i).unwrap_or(&b' ');
            }
        };
        // A fixed disk.
        word(b, 0, 0x0040);
        word(b, 1, p.cylinders as u16);
        word(b, 3, p.heads as u16);
        word(b, 4, (p.sectors * 512) as u16);
        word(b, 5, 512);
        word(b, 6, p.sectors as u16);
        text(b, 10, 20, &format!("RDSHD{:04}", self.drive));
        word(b, 20, 1);
        word(b, 21, 4);
        text(b, 23, 8, "1.0");
        text(b, 27, 40, "Rust-DOS ATA hard disk");
        word(b, 47, 0x8000 | MULTIPLE_MAX as u16);
        // IORDY and LBA, no DMA.
        word(b, 49, 0x0A00);
        word(b, 50, 0x4000);
        word(b, 51, 0x00F0);
        word(b, 52, 0x00F0);
        word(b, 53, 0x0007);
        // The geometry in use and its sectors.
        word(b, 54, l.cylinders as u16);
        word(b, 55, l.heads as u16);
        word(b, 56, l.sectors as u16);
        let current = l.total().min(u32::MAX as u64) as u32;
        b[57 * 2..57 * 2 + 4].copy_from_slice(&current.to_le_bytes());
        if self.multiple != 0 {
            word(b, 59, 0x0100 | self.multiple as u16);
        }
        let lba28 = self.total.min(0x0FFF_FFFF) as u32;
        b[60 * 2..60 * 2 + 4].copy_from_slice(&lba28.to_le_bytes());
        // PIO modes 3 and 4, and their cycle times.
        word(b, 64, 0x0003);
        for w in 65..=68 {
            word(b, w, 0x0078);
        }
        // ATA-1 to ATA-6; NOP, DEVICE RESET, the power management and
        // FLUSH CACHE.
        word(b, 80, 0x007E);
        word(b, 81, 0x0022);
        word(b, 82, 0x4208);
        word(b, 83, 0x5000);
        word(b, 84, 0x4000);
        word(b, 85, 0x4208);
        word(b, 86, 0x1000);
        word(b, 87, 0x4000);
        b[510] = 0xA5;
        let sum = b[..511].iter().fold(0u8, |s, &x| s.wrapping_add(x));
        b[511] = sum.wrapping_neg();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometries_have_16_heads_at_most() {
        let chs = |cylinders, heads, sectors| Chs { cylinders, heads, sectors };
        let g = Geometry::for_disk(chs(1023, 64, 63), 1023 * 64 * 63);
        assert_eq!((g.cylinders, g.heads, g.sectors), (4092, 16, 63));
        let g = Geometry::for_disk(chs(615, 4, 17), 615 * 4 * 17);
        assert_eq!((g.cylinders, g.heads, g.sectors), (615, 4, 17));
        // Past 8.4 GB, 16383/16/63.
        let g = Geometry::for_disk(chs(1023, 255, 63), 40 << 21);
        assert_eq!((g.cylinders, g.heads, g.sectors), (16383, 16, 63));
    }

    #[test]
    fn identify_carries_its_checksum() {
        let mut ata = Ata::new(2, Chs { cylinders: 1023, heads: 64, sectors: 63 }, 1023 * 64 * 63);
        ata.identify();
        assert_eq!(ata.buf[..512].iter().fold(0u8, |s, &x| s.wrapping_add(x)), 0);
        let word = |w: usize| u16::from_le_bytes([ata.buf[w * 2], ata.buf[w * 2 + 1]]);
        assert_eq!((word(0), word(1), word(3), word(6)), (0x0040, 4092, 16, 63));
        assert_eq!(word(60) as u32 | (word(61) as u32) << 16, 1023 * 64 * 63);
        assert_eq!(&ata.buf[54..58], b"uRts");
    }
}
