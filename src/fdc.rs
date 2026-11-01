//! The floppy disk controller of a booted system: an Intel 82077AA (NEC
//! uPD765 compatible) on the primary ports 3F0h-3F5h and 3F7h, with IRQ 6
//! and DMA channel 2, for drives 0 and 1, the floppy units 00h and 01h
//! whose disks INT 13h reads and writes too. A system's own floppy driver
//! (Windows 9x's HSFLOP.PDR, Linux, OS/2) finds it there.
//!
//! DOSBox-X's `FloppyController` (floppy.cpp) is the model: the digital
//! output register's reset, drive select and motors, the main status
//! register, the command and result phases through the data port, the
//! sectors moved by DMA until terminal count or the end of the track, and
//! the disk change line in the digital input register. Beyond DOSBox-X,
//! from the 82077AA data sheet: the four polling interrupts after a reset,
//! SENSE INTERRUPT STATUS's single invalid byte with none pending, the
//! drive busy bits of a seek, relative seeks, the data rate a disk reads
//! at, FORMAT TRACK, VERSION, PERPENDICULAR MODE and LOCK, the command's
//! EOT as the last sector of a track, the end of the track (with MT, of
//! the cylinder) ending a transfer with EN, READ DELETED DATA finding
//! normal data marks (CM, with SK skipping the sector), and the write
//! protect, missing address mark and wrong cylinder errors.
//!
//! What isn't there: transfers without DMA (SPECIFY's ND), READ TRACK,
//! VERIFY and the SCAN commands, deleted data marks (images have none, so
//! WRITE DELETED DATA writes normal sectors) and sectors of other sizes
//! than 512 bytes.

use crate::diskimage::{DiskImage, SECTOR_SIZE, STATUS_CRC_ERROR};
use crate::dma::Dma;
use crate::timer::PIT_HZ;
use std::ops::Range;
use std::rc::Rc;

/// The ports: the base, the digital output register, the tape drive
/// register, the main status (read) and data rate select (write)
/// registers, the data port (FIFO), and the digital input (read) and
/// configuration control (write) registers.
pub const BASE: u16 = 0x3F0;
const DOR: u16 = 0x3F2;
const TDR: u16 = 0x3F3;
const MSR: u16 = 0x3F4;
pub const DATA: u16 = 0x3F5;
pub const DIR: u16 = 0x3F7;

pub const IRQ: u8 = 6;
pub const DMA_CHANNEL: usize = 2;

/// The handle of its Plug and Play BIOS node: after the serial ports
/// COM3 and COM4 (`pnpbios`).
pub const PNP_HANDLE: u8 = 0x13;

/// The drives the controller has (units 00h and 01h).
pub const DRIVES: usize = 2;

/// The digital output register's bits.
const DOR_RESET: u8 = 0x04;
const DOR_DMA: u8 = 0x08;

/// The main status register's bits.
const RQM: u8 = 0x80;
const DIO: u8 = 0x40;
const CB: u8 = 0x10;

/// ST0: abnormal termination, invalid command, seek end, equipment check.
const ST0_ABNORMAL: u8 = 0x40;
const ST0_INVALID: u8 = 0x80;
const ST0_SEEK_END: u8 = 0x20;
const ST0_EQUIPMENT_CHECK: u8 = 0x10;
/// The polling interrupt of a drive after a reset (ready changed).
const ST0_POLL: u8 = 0xC0;
/// ST1: end of cylinder, data error, overrun, no data, not writable,
/// missing address mark.
const ST1_EN: u8 = 0x80;
const ST1_DE: u8 = 0x20;
const ST1_OR: u8 = 0x10;
const ST1_ND: u8 = 0x04;
const ST1_NW: u8 = 0x02;
const ST1_MA: u8 = 0x01;
/// ST2: control mark, data error in the data field, wrong cylinder, bad
/// cylinder.
const ST2_CM: u8 = 0x40;
const ST2_DD: u8 = 0x20;
const ST2_WC: u8 = 0x10;
const ST2_BC: u8 = 0x02;

/// What VERSION answers: an 82077AA's enhanced controller.
const VERSION: u8 = 0x90;
/// CONFIGURE's byte as it is after a reset: the FIFO off, drive polling
/// on.
const CONFIG_DEFAULT: u8 = 0x20;
const CONFIG_NO_POLL: u8 = 0x10;

/// The sector size code of 512 bytes.
const N_512: u8 = 2;

/// PIT ticks in `ms` milliseconds.
fn ticks(ms: f64) -> u64 {
    (ms * PIT_HZ as f64 / 1000.0).ceil() as u64
}

/// What a command's execution phase works with.
pub struct Env<'a> {
    /// The disks in drives 0 and 1.
    pub disks: [Option<Rc<DiskImage>>; DRIVES],
    pub dma: &'a mut Dma,
    pub ram: &'a mut [u8],
    /// PIT ticks.
    pub now: u64,
    /// How long a sector takes at `floppy_disk_speed`, in nanoseconds; 0
    /// at the maximum.
    pub ns_per_sector: u64,
    /// The memory DMA wrote, which the caller must treat as modified.
    pub written: Option<Range<usize>>,
    /// The BIOS's show of a transfer for a V86 monitor (`Bus::faked_io`):
    /// READ and WRITE DATA end at once, moving nothing.
    pub faked: bool,
}

impl Env<'_> {
    fn disk(&self, drive: usize) -> Option<Rc<DiskImage>> {
        self.disks.get(drive).cloned().flatten()
    }

    fn note_written(&mut self, span: Option<Range<usize>>) {
        if let Some(span) = span {
            self.written = Some(match self.written.take() {
                Some(w) => w.start.min(span.start)..w.end.max(span.end),
                None => span,
            });
        }
    }
}

/// Where the controller is in a command.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Phase {
    /// Taking a command's bytes (none yet: idle).
    #[default]
    Command,
    /// Carrying it out, until `exec_due`.
    Execution,
    /// Handing out the result bytes.
    Result,
}

crate::state_enum!(Phase { Phase::Command, Phase::Execution, Phase::Result });

/// The controller.
#[derive(Clone, Debug)]
pub struct Fdc {
    dor: u8,
    tdr: u8,
    /// The data rate (CCR and DSR bits 0-1): 0 500 kb/s, 1 300 kb/s, 2
    /// 250 kb/s, 3 1 Mb/s.
    rate: u8,
    phase: Phase,
    command: Vec<u8>,
    result: Vec<u8>,
    result_pos: usize,
    /// When the execution phase of the command ends (PIT ticks).
    exec_due: Option<u64>,
    /// The interrupt of a command's result phase, or of READ ID's.
    int: bool,
    /// The polling interrupts of the drives after a reset, for SENSE
    /// INTERRUPT STATUS to report (drives from 4 - `reset_polls` on).
    reset_polls: u8,
    /// A drive's seek: when it ends, and then its ST0 for SENSE INTERRUPT
    /// STATUS.
    seek_due: [Option<u64>; 4],
    seek_end: [u8; 4],
    seek_status: [Option<u8>; 4],
    /// The present cylinder number the controller has for each drive, and
    /// where the drive's heads are.
    pcn: [u8; 4],
    track: [u8; 4],
    /// The drives' disk change lines: set when another disk goes in,
    /// cleared by a step with a disk in the drive.
    changed: [bool; 4],
    /// SPECIFY's two bytes, CONFIGURE's two, PERPENDICULAR MODE's, LOCK
    /// and the last transfer's EOT, for DUMPREG.
    specify: [u8; 2],
    config: u8,
    pretrk: u8,
    perpendicular: u8,
    lock: bool,
    eot: u8,
    /// The sector READ ID finds next under the heads.
    rotation: u8,
    /// The level of IRQ 6 the bus last passed to the interrupt controller.
    pub pic_line: bool,
    /// Messages for the log.
    pub log: Vec<String>,
    /// What was read and written since last asked: drive, first sector,
    /// sectors.
    pub activity: Vec<(usize, u64, u32)>,
}

crate::state_fields!(Fdc {
    dor, tdr, rate, phase, command, result, result_pos, exec_due, int, reset_polls,
    seek_due, seek_end, seek_status, pcn, track, changed, specify, config, pretrk, perpendicular,
    lock, eot, rotation, pic_line,
} skip { log, activity });

impl Default for Fdc {
    fn default() -> Self {
        Self::new()
    }
}

/// How many bytes a command has, from its first byte; None for one the
/// controller doesn't know.
fn command_len(op: u8) -> Option<usize> {
    Some(match op {
        0x03 => 3,                                        // SPECIFY
        0x04 => 2,                                        // SENSE DRIVE STATUS
        0x07 => 2,                                        // RECALIBRATE
        0x08 => 1,                                        // SENSE INTERRUPT STATUS
        0x0A | 0x4A => 2,                                 // READ ID
        0x0D | 0x4D => 6,                                 // FORMAT TRACK
        0x0E => 1,                                        // DUMPREG
        0x0F | 0x8F | 0xCF => 3,                          // SEEK, RELATIVE SEEK
        0x10 => 1,                                        // VERSION
        0x12 => 2,                                        // PERPENDICULAR MODE
        0x13 => 4,                                        // CONFIGURE
        0x14 | 0x94 => 1,                                 // LOCK
        _ if op & 0x1F == 0x06 || op & 0x1F == 0x0C => 9, // READ (DELETED) DATA
        _ if op & 0x3F == 0x05 || op & 0x3F == 0x09 => 9, // WRITE (DELETED) DATA
        _ => return None,
    })
}

/// The data rates (CCR codes) a disk of `sectors` per track reads at: a
/// 2.88 MB disk's 1 Mb/s, a high density disk's 500 kb/s, and a double
/// density disk's 250 kb/s, or 300 kb/s in a high density 5.25" drive.
fn reads_at(sectors: u32, rate: u8) -> bool {
    match sectors {
        s if s >= 36 => rate == 3,
        s if s >= 15 => rate == 0,
        _ => rate == 1 || rate == 2,
    }
}

impl Fdc {
    /// A controller as a BIOS leaves it: out of reset with DMA and the
    /// interrupt on, the motors off, the disks' change lines set.
    pub fn new() -> Self {
        Self {
            dor: DOR_RESET | DOR_DMA,
            tdr: 0,
            rate: 0,
            phase: Phase::Command,
            command: Vec::new(),
            result: Vec::new(),
            result_pos: 0,
            exec_due: None,
            int: false,
            reset_polls: 0,
            seek_due: [None; 4],
            seek_end: [0; 4],
            seek_status: [None; 4],
            pcn: [0; 4],
            track: [0; 4],
            changed: [true; 4],
            specify: [0xAF, 0x02],
            config: CONFIG_DEFAULT,
            pretrk: 0,
            perpendicular: 0,
            lock: false,
            eot: 18,
            rotation: 0,
            pic_line: false,
            log: Vec::new(),
            activity: Vec::new(),
        }
    }

    /// Whether `port` is the controller's: 3F6h is the IDE channel's.
    pub fn claims(port: u16) -> bool {
        (BASE..=DIR).contains(&port) && port != 0x3F6
    }

    fn selected(&self) -> usize {
        (self.dor & 3) as usize
    }

    fn in_reset(&self) -> bool {
        self.dor & DOR_RESET == 0
    }

    /// The interrupt line: a result phase's or READ ID's interrupt, or a
    /// status for SENSE INTERRUPT STATUS, while the digital output
    /// register lets it out.
    pub fn irq(&self) -> bool {
        let pending =
            self.int || self.reset_polls > 0 || self.seek_status.iter().any(Option::is_some);
        pending && self.dor & (DOR_RESET | DOR_DMA) == DOR_RESET | DOR_DMA
    }

    /// When the controller next needs attention (PIT ticks).
    pub fn next_event(&self) -> Option<u64> {
        self.seek_due
            .iter()
            .flatten()
            .copied()
            .chain(self.exec_due)
            .min()
    }

    /// Another disk went in drive `drive`, or the disk came out.
    pub fn media_changed(&mut self, drive: usize) {
        if let Some(line) = self.changed.get_mut(drive) {
            *line = true;
        }
    }

    // --- Ports ---

    /// A byte read at `port`, one of the controller's.
    pub fn read(&mut self, port: u16) -> u8 {
        match port {
            DOR => self.dor,
            TDR => self.tdr & 3,
            MSR => self.msr(),
            DATA => self.read_data(),
            // The disk change line of the selected drive; the other bits
            // as DOSBox-X has them: a high density data rate.
            DIR => 0x78 | (self.changed[self.selected()] as u8) << 7,
            // The status registers A and B are a PS/2's.
            _ => 0xFF,
        }
    }

    /// A byte written at `port`, one of the controller's.
    pub fn write(&mut self, port: u16, value: u8, env: &mut Env) {
        match port {
            DOR => self.write_dor(value),
            TDR => self.tdr = value & 3,
            // The data rate select register: a software reset, which ends
            // by itself, and the data rate.
            MSR => {
                self.rate = value & 3;
                if value & 0x80 != 0 {
                    self.reset();
                    self.reset_done();
                }
            }
            DATA => self.write_data(value, env),
            DIR => self.rate = value & 3,
            _ => {}
        }
    }

    fn msr(&self) -> u8 {
        if self.in_reset() {
            return 0;
        }
        // The drives seeking, or with a seek's end not yet sensed.
        let busy = (0..4)
            .filter(|&d| self.seek_due[d].is_some() || self.seek_status[d].is_some())
            .fold(0, |m, d| m | 1 << d);
        busy | match self.phase {
            Phase::Command if self.command.is_empty() => RQM,
            Phase::Command => RQM | CB,
            Phase::Execution => CB,
            Phase::Result => RQM | DIO | CB,
        }
    }

    /// The digital output register: the reset line, drive select, DMA and
    /// interrupt enable, motors.
    fn write_dor(&mut self, value: u8) {
        let was_reset = self.in_reset();
        self.dor = value;
        if !was_reset && self.in_reset() {
            self.reset();
        } else if was_reset && !self.in_reset() {
            self.reset_done();
        }
    }

    /// The controller held in reset: no command, no interrupt, the
    /// cylinders 0, and CONFIGURE's settings back unless LOCKed.
    fn reset(&mut self) {
        self.phase = Phase::Command;
        self.command.clear();
        self.result.clear();
        self.result_pos = 0;
        self.exec_due = None;
        self.int = false;
        self.reset_polls = 0;
        self.seek_due = [None; 4];
        self.seek_status = [None; 4];
        self.pcn = [0; 4];
        if !self.lock {
            self.config = CONFIG_DEFAULT;
            self.pretrk = 0;
        }
        // PERPENDICULAR MODE's GAP and WGATE; the drives' bits stay.
        self.perpendicular &= 0x3C;
    }

    /// Out of reset: with drive polling on, an interrupt and a status for
    /// each of the four drives.
    fn reset_done(&mut self) {
        if self.config & CONFIG_NO_POLL == 0 {
            self.reset_polls = 4;
        }
    }

    fn read_data(&mut self) -> u8 {
        if self.in_reset() || self.phase != Phase::Result {
            return 0xFF;
        }
        let value = self.result.get(self.result_pos).copied().unwrap_or(0xFF);
        // Reading the result clears its interrupt.
        self.int = false;
        self.result_pos += 1;
        if self.result_pos >= self.result.len() {
            self.idle();
        }
        value
    }

    fn idle(&mut self) {
        self.phase = Phase::Command;
        self.command.clear();
        self.result.clear();
        self.result_pos = 0;
    }

    fn write_data(&mut self, value: u8, env: &mut Env) {
        if self.in_reset() || self.phase != Phase::Command {
            return;
        }
        if self.command.is_empty() {
            // A new command clears the last one's interrupt.
            self.int = false;
            if command_len(value).is_none() {
                self.log.push(format!(
                    "[FDC] Command {:02X}h isn't one the controller knows",
                    value
                ));
                self.finish(vec![ST0_INVALID], false);
                return;
            }
        }
        self.command.push(value);
        if Some(self.command.len()) == command_len(self.command[0]) {
            self.start(env);
        }
    }

    /// Hand out `result`, with or without an interrupt; with none, the
    /// controller is idle again.
    fn finish(&mut self, result: Vec<u8>, interrupt: bool) {
        self.exec_due = None;
        if result.is_empty() {
            self.idle();
            return;
        }
        self.phase = Phase::Result;
        self.result = result;
        self.result_pos = 0;
        self.command.clear();
        self.int = interrupt;
    }

    /// The command's bytes are in: carry it out, at once or after its
    /// execution phase.
    fn start(&mut self, env: &mut Env) {
        let c = self.command.clone();
        let drive = (c.get(1).copied().unwrap_or(0) & 3) as usize;
        match c[0] {
            // SPECIFY: step rate, head unload and load times, DMA.
            0x03 => {
                self.specify = [c[1], c[2]];
                if c[2] & 1 != 0 {
                    self.log
                        .push("[FDC] Transfers without DMA aren't supported".to_string());
                }
                self.finish(vec![], false);
            }
            // SENSE DRIVE STATUS: ST3.
            0x04 => {
                let disk = env.disk(drive);
                let st3 = 0x28
                    | (disk.as_ref().is_some_and(|d| !d.writable()) as u8) << 6
                    | ((self.track[drive] == 0) as u8) << 4
                    | (c[1] & 7);
                self.finish(vec![st3], false);
            }
            // SENSE INTERRUPT STATUS: the polling statuses after a reset,
            // then the seeks that ended, or invalid with none.
            0x08 => {
                let result = if self.reset_polls > 0 {
                    let d = 4 - self.reset_polls as usize;
                    self.reset_polls -= 1;
                    vec![ST0_POLL | d as u8, self.pcn[d]]
                } else if let Some(d) = (0..4).find(|&d| self.seek_status[d].is_some()) {
                    let st0 = self.seek_status[d].take().unwrap_or(0);
                    vec![st0, self.pcn[d]]
                } else {
                    vec![ST0_INVALID]
                };
                self.finish(result, false);
            }
            // RECALIBRATE: up to 79 steps out, to track 0.
            0x07 => {
                let steps = self.track[drive].min(79);
                let to_zero = steps == self.track[drive];
                self.step(drive, -(steps as i32), env);
                self.pcn[drive] = 0;
                let st0 = drive as u8
                    | if to_zero {
                        ST0_SEEK_END
                    } else {
                        ST0_SEEK_END | ST0_ABNORMAL | ST0_EQUIPMENT_CHECK
                    };
                self.begin_seek(drive, steps as u32, st0, env.now);
            }
            // SEEK, or RELATIVE SEEK by a number of cylinders in or out.
            0x0F | 0x8F | 0xCF => {
                let target = match c[0] {
                    0x0F => c[2] as i32,
                    0xCF => self.pcn[drive] as i32 + c[2] as i32,
                    _ => self.pcn[drive] as i32 - c[2] as i32,
                }
                .clamp(0, 255);
                let delta = target - self.pcn[drive] as i32;
                self.step(drive, delta, env);
                self.pcn[drive] = target as u8;
                let st0 = ST0_SEEK_END | (c[1] & 7);
                self.begin_seek(drive, delta.unsigned_abs(), st0, env.now);
            }
            // DUMPREG.
            0x0E => {
                let mut result = self.pcn.to_vec();
                result.extend([
                    self.specify[0],
                    self.specify[1],
                    self.eot,
                    (self.lock as u8) << 7 | (self.perpendicular & 0x3F),
                    self.config,
                    self.pretrk,
                ]);
                self.finish(result, false);
            }
            0x10 => self.finish(vec![VERSION], false),
            // PERPENDICULAR MODE: kept for DUMPREG; the drives are all
            // ordinary ones.
            0x12 => {
                if c[1] & 0x80 != 0 {
                    self.perpendicular = c[1] & 0x3F;
                } else {
                    self.perpendicular = (self.perpendicular & 0x3C) | (c[1] & 3);
                }
                self.finish(vec![], false);
            }
            // CONFIGURE: FIFO, drive polling, precompensation track.
            0x13 => {
                self.config = c[2] & 0x7F;
                self.pretrk = c[3];
                self.finish(vec![], false);
            }
            // LOCK: CONFIGURE's settings kept through resets.
            0x14 | 0x94 => {
                self.lock = c[0] & 0x80 != 0;
                self.finish(vec![(self.lock as u8) << 4], false);
            }
            // The BIOS's show of a sector transfer: done, with the next
            // sector's ID, no data moved and no interrupt.
            _ if env.faked && c.len() == 9 && c[0] & 0x1F != 0x0D => {
                self.finish(vec![c[1] & 7, 0, 0, c[2], c[3], c[4].wrapping_add(1), c[5]], false);
            }
            // READ ID, READ and WRITE DATA, FORMAT TRACK: after the time
            // the disk takes.
            _ => {
                let sectors = match c[0] & 0x1F {
                    0x0A => 1,
                    0x0D => c[3].max(1) as u64,
                    // What DMA has left to move, in sectors.
                    _ => (env.dma.channel(DMA_CHANNEL).cur_count as u64 + 1)
                        .div_ceil(SECTOR_SIZE as u64),
                };
                let ns = (env.ns_per_sector * sectors).max(100_000);
                self.phase = Phase::Execution;
                self.exec_due = Some(env.now + ticks(ns as f64 / 1_000_000.0));
            }
        }
    }

    /// The heads of `drive` step `delta` tracks (in when positive): a step
    /// with a disk in the drive clears its change line.
    fn step(&mut self, drive: usize, delta: i32, env: &Env) {
        if delta == 0 {
            return;
        }
        self.track[drive] = (self.track[drive] as i32 + delta).clamp(0, 83) as u8;
        if env.disk(drive).is_some() {
            self.changed[drive] = false;
        }
    }

    /// A seek of `steps` steps on `drive`, whose end will be `st0`.
    fn begin_seek(&mut self, drive: usize, steps: u32, st0: u8, now: u64) {
        // SPECIFY's step rate, at 500 kb/s 16 - SRT ms a step, slower at
        // lower data rates.
        let step_ms = (16 - (self.specify[0] >> 4) as u32) as f64
            * match self.rate {
                1 => 1.67,
                2 => 2.0,
                3 => 0.5,
                _ => 1.0,
            };
        let ms = (step_ms * steps as f64).max(0.1);
        self.seek_due[drive] = Some(now + ticks(ms));
        self.seek_status[drive] = None;
        self.seek_end[drive] = st0;
        self.idle();
    }

    /// Carry out what came due.
    pub fn service(&mut self, env: &mut Env) {
        for d in 0..4 {
            if self.seek_due[d].is_some_and(|t| t <= env.now) {
                self.seek_due[d] = None;
                self.seek_status[d] = Some(self.seek_end[d]);
            }
        }
        if self.exec_due.is_some_and(|t| t <= env.now) && self.phase == Phase::Execution {
            self.execute(env);
        }
    }

    /// The end of a command's execution phase: READ ID, or the sectors
    /// read, written or formatted, and the result with its interrupt.
    fn execute(&mut self, env: &mut Env) {
        let c = std::mem::take(&mut self.command);
        let drive = (c[1] & 3) as usize;
        let hds = (c[1] >> 2) & 1;
        // Without a disk turning in the drive (none in it, or its motor
        // off) there are no index pulses, and the command waits for them
        // until a reset, as the 82077AA's does: the driver gives up after
        // its own timeout and finds the drive not ready. Windows 98's
        // HSFLOP.PDR takes an error for a disk it can't read instead
        // ("General failure"), and its Setup stops on that.
        if env.disk(drive).is_none() || self.dor & (0x10 << drive) == 0 {
            self.exec_due = None;
            return;
        }
        // A disk under the heads that the controller can read: in the
        // drive, its motor on, MFM at the data rate it was written at, on
        // a side and track it has.
        let disk = env.disk(drive).filter(|d| {
            let g = d.geometry();
            self.dor & (0x10 << drive) != 0
                && c[0] & 0x40 != 0
                && reads_at(g.sectors, self.rate)
                && (hds as u32) < g.heads
                && (self.track[drive] as u32) < g.cylinders
        });
        let result = match disk {
            // No address marks to be found.
            None => {
                // READ and WRITE DATA's own C, H, R and N; FORMAT TRACK's
                // bytes from the third on are N, SC, GPL and D.
                let id = if c.len() == 9 {
                    [c[2], c[3], c[4], c[5]]
                } else {
                    [self.track[drive], hds, 1, N_512]
                };
                [vec![ST0_ABNORMAL | (c[1] & 7), ST1_MA, 0], id.to_vec()].concat()
            }
            Some(disk) => match c[0] & 0x1F {
                0x0A => {
                    self.rotation = self.rotation % disk.geometry().sectors as u8 + 1;
                    vec![c[1] & 7, 0, 0, self.track[drive], hds, self.rotation, N_512]
                }
                0x0D => self.format(&c, &disk, env),
                _ => self.transfer(&c, &disk, env),
            },
        };
        self.finish(result, true);
    }

    /// READ or WRITE DATA: sectors from the command's on, moved by DMA
    /// until terminal count, or the end of the track at the command's EOT
    /// (with MT, of the cylinder). The result: ST0-ST2 and the ID of the next sector.
    fn transfer(&mut self, c: &[u8], disk: &DiskImage, env: &mut Env) -> Vec<u8> {
        let write = matches!(c[0] & 0x1F, 0x05 | 0x09);
        let multi_track = c[0] & 0x80 != 0;
        let drive = (c[1] & 3) as usize;
        let mut hds = (c[1] >> 2) & 1;
        let (mut cylinder, mut head, mut sector, size) = (c[2], c[3], c[4], c[5]);
        self.eot = c[6];
        let geometry = disk.geometry();
        let per_track = geometry.sectors;
        let track = self.track[drive];
        // READ DELETED DATA finds only normal data marks, since images have
        // no deleted sectors: each sector sets CM, and with SK is skipped
        // unread, without SK is read and ends the command.
        let deleted = c[0] & 0x1F == 0x0C;
        let skip = c[0] & 0x20 != 0;
        let (mut st1, mut st2) = (0u8, 0u8);
        if write && !disk.writable() {
            st1 |= ST1_NW;
        } else if self.dor & DOR_DMA == 0 {
            // No DMA to move the data.
            st1 |= ST1_OR;
        }
        while st1 == 0 {
            // The sector's ID has the track's cylinder and the side's head.
            if cylinder != track {
                st1 |= ST1_ND;
                st2 |= if cylinder == 0xFF { ST2_BC } else { ST2_WC };
                break;
            }
            // MT carries on to side 1, which a single-sided disk hasn't.
            if hds as u32 >= geometry.heads {
                st1 |= ST1_MA;
                break;
            }
            if head != hds || size != N_512 || sector == 0 || sector as u32 > per_track {
                st1 |= ST1_ND;
                break;
            }
            let lba = (track as u64 * geometry.heads as u64 + hds as u64) * per_track as u64
                + sector as u64
                - 1;
            let mut data = [0u8; SECTOR_SIZE];
            if deleted {
                st2 |= ST2_CM;
            }
            let (moved, tc) = if deleted && skip {
                (SECTOR_SIZE, false)
            } else if write {
                // What terminal count leaves of the sector is zeros.
                let (moved, tc) = env.dma.transfer_read(DMA_CHANNEL, env.ram, &mut data);
                if moved == SECTOR_SIZE || tc {
                    if disk.write(lba, &data).is_err() {
                        st1 |= ST1_ND;
                        break;
                    }
                    self.note_activity(drive, lba);
                }
                (moved, tc)
            } else {
                match disk.read(lba, &mut data) {
                    Ok(()) => {}
                    Err(STATUS_CRC_ERROR) => {
                        st1 |= ST1_DE;
                        st2 |= ST2_DD;
                    }
                    Err(_) => {
                        st1 |= ST1_ND;
                        break;
                    }
                }
                self.note_activity(drive, lba);
                // The DMA channel set to verify moves nothing.
                if env.dma.channel(DMA_CHANNEL).mode & 0x0C == 0 {
                    let units =
                        SECTOR_SIZE.min(env.dma.channel(DMA_CHANNEL).cur_count as usize + 1);
                    let ready = env.dma.ready(DMA_CHANNEL);
                    let tc = env.dma.transfer_skip(DMA_CHANNEL, units);
                    (if ready { units } else { 0 }, tc)
                } else {
                    let (moved, tc, span) = env.dma.transfer_write(DMA_CHANNEL, env.ram, &data);
                    env.note_written(span);
                    (moved, tc)
                }
            };
            if moved < SECTOR_SIZE && !tc {
                st1 |= ST1_OR;
                break;
            }
            if st1 != 0 {
                break;
            }
            // The next sector's ID. The command's EOT ends the track early.
            let end_of_track = sector as u32 == per_track || sector == self.eot;
            let next_side = end_of_track && multi_track && hds == 0;
            if !end_of_track {
                sector += 1;
            } else {
                sector = 1;
                if next_side {
                    hds = 1;
                    head ^= 1;
                } else {
                    cylinder = cylinder.wrapping_add(1);
                    if multi_track {
                        head ^= 1;
                    }
                }
            }
            // A normal sector read without SK ends READ DELETED DATA.
            if tc || (deleted && !skip) {
                break;
            }
            if end_of_track && !next_side {
                st1 |= ST1_EN;
            }
        }
        // CM alone is a normal termination.
        let st0 = (c[1] & 3) | hds << 2 | if st1 | (st2 & !ST2_CM) != 0 { ST0_ABNORMAL } else { 0 };
        vec![st0, st1, st2, cylinder, head, sector, size]
    }

    /// FORMAT TRACK: the sectors whose IDs DMA brings, filled with the
    /// command's filler byte.
    fn format(&mut self, c: &[u8], disk: &DiskImage, env: &mut Env) -> Vec<u8> {
        let drive = (c[1] & 3) as usize;
        let hds = (c[1] >> 2) & 1;
        let (size, count, filler) = (c[2], c[3], c[5]);
        let geometry = disk.geometry();
        let track = self.track[drive];
        let mut st1 = 0u8;
        let mut last = [track, hds, 1, size];
        if !disk.writable() {
            st1 |= ST1_NW;
        } else if self.dor & DOR_DMA == 0 {
            st1 |= ST1_OR;
        }
        for _ in 0..count {
            if st1 != 0 {
                break;
            }
            let mut id = [0u8; 4];
            let (moved, tc) = env.dma.transfer_read(DMA_CHANNEL, env.ram, &mut id);
            if moved < id.len() && !tc {
                st1 |= ST1_OR;
                break;
            }
            last = id;
            // Only sectors the image has: 512 bytes, numbered from 1.
            let sector = id[2] as u32;
            if size == N_512 && id[3] == N_512 && (1..=geometry.sectors).contains(&sector) {
                let lba = (track as u64 * geometry.heads as u64 + hds as u64)
                    * geometry.sectors as u64
                    + sector as u64
                    - 1;
                if disk.write(lba, &[filler; SECTOR_SIZE]).is_err() {
                    st1 |= ST1_ND;
                    break;
                }
                self.note_activity(drive, lba);
            }
            if tc {
                break;
            }
        }
        let st0 = (c[1] & 7) | if st1 != 0 { ST0_ABNORMAL } else { 0 };
        [vec![st0, st1, 0], last.to_vec()].concat()
    }

    /// Sector `lba` of `drive` was read or written.
    fn note_activity(&mut self, drive: usize, lba: u64) {
        match self.activity.last_mut() {
            Some((d, first, count)) if *d == drive && *first + *count as u64 == lba => *count += 1,
            _ => self.activity.push((drive, lba, 1)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diskimage::{Chs, MemoryImage};
    use crate::savestate::State;

    /// A 1.44 MB disk whose sectors start with their own number.
    fn disk(read_only: bool) -> Rc<DiskImage> {
        let mut bytes = vec![0u8; 1_474_560];
        for (lba, sector) in bytes.chunks_mut(SECTOR_SIZE).enumerate() {
            sector[..2].copy_from_slice(&(lba as u16).to_le_bytes());
        }
        let geometry = Chs {
            cylinders: 80,
            heads: 2,
            sectors: 18,
        };
        Rc::new(
            DiskImage::from_memory(
                "a.img",
                MemoryImage::from(bytes),
                true,
                Some(geometry),
                read_only,
            )
            .unwrap(),
        )
    }

    /// A controller with a disk in drive 0, and the machine around it.
    struct Rig {
        fdc: Fdc,
        dma: Dma,
        ram: Vec<u8>,
        disks: [Option<Rc<DiskImage>>; DRIVES],
        now: u64,
        faked: bool,
    }

    impl Rig {
        fn new() -> Self {
            Self {
                fdc: Fdc::new(),
                dma: Dma::new(),
                ram: vec![0; 0x20000],
                disks: [Some(disk(false)), None],
                now: 0,
                faked: false,
            }
        }

        fn with<T>(&mut self, f: impl FnOnce(&mut Fdc, &mut Env) -> T) -> T {
            let mut env = Env {
                disks: self.disks.clone(),
                dma: &mut self.dma,
                ram: &mut self.ram,
                now: self.now,
                ns_per_sector: 0,
                written: None,
                faked: self.faked,
            };
            f(&mut self.fdc, &mut env)
        }

        fn out(&mut self, port: u16, value: u8) {
            self.with(|fdc, env| fdc.write(port, value, env));
        }

        /// A command's bytes, then the time it takes, then its result.
        fn command(&mut self, bytes: &[u8]) -> Vec<u8> {
            for &b in bytes {
                assert_eq!(
                    self.fdc.read(MSR) & (RQM | DIO),
                    RQM,
                    "ready for byte {:02X}h",
                    b
                );
                self.out(DATA, b);
            }
            self.settle();
            let mut result = Vec::new();
            while self.fdc.read(MSR) & (RQM | DIO) == RQM | DIO {
                result.push(self.fdc.read(DATA));
            }
            result
        }

        /// Let what is due happen.
        fn settle(&mut self) {
            while let Some(at) = self.fdc.next_event() {
                self.now = self.now.max(at);
                self.with(|fdc, env| fdc.service(env));
            }
        }

        /// DMA channel 2 set for `bytes` bytes at 10000h, into memory
        /// (`write`) or out of it.
        fn dma(&mut self, bytes: usize, write: bool) {
            let count = (bytes - 1) as u16;
            for (port, value) in [
                (0x0A, 0x06),
                (0x0B, if write { 0x46 } else { 0x4A }),
                (0x0C, 0),
                (0x04, 0x00),
                (0x04, 0x00),
                (0x81, 0x01),
                (0x05, count as u8),
                (0x05, (count >> 8) as u8),
                (0x0A, 0x02),
            ] {
                self.dma.write(port, value);
            }
        }

        /// The sector number a sector moved to 10000h + `at` starts with.
        fn sector_at(&self, at: usize) -> u16 {
            u16::from_le_bytes([self.ram[0x10000 + at], self.ram[0x10001 + at]])
        }

        /// Motor and drive 0 on, out of reset, the polling statuses read.
        fn ready(&mut self) {
            self.out(DOR, 0x1C);
            self.seek(0, 0);
        }

        fn seek(&mut self, drive: u8, cylinder: u8) {
            assert_eq!(self.command(&[0x0F, drive, cylinder]), [0u8; 0]);
            assert!(self.fdc.irq());
            assert_eq!(self.command(&[0x08]), [0x20 | drive, cylinder]);
        }
    }

    #[test]
    fn a_reset_reports_each_drive_once() {
        let mut rig = Rig::new();
        rig.out(DOR, 0x08);
        assert_eq!(rig.fdc.read(MSR), 0, "held in reset");
        rig.out(DOR, 0x0C);
        assert!(rig.fdc.irq());
        for d in 0..4 {
            assert_eq!(rig.command(&[0x08]), [0xC0 | d, 0]);
        }
        assert!(!rig.fdc.irq());
        // Nothing left to report: invalid, a single byte.
        assert_eq!(rig.command(&[0x08]), [0x80]);
        // The data rate select register's reset does the same.
        rig.out(MSR, 0x80);
        assert!(rig.fdc.irq());
        // With the interrupt and DMA off, the line stays down.
        rig.out(DOR, 0x04);
        assert!(!rig.fdc.irq());
    }

    #[test]
    fn the_82077aa_commands_answer() {
        let mut rig = Rig::new();
        assert_eq!(rig.command(&[0x10]), [0x90]);
        assert_eq!(rig.command(&[0x03, 0xDF, 0x02]), [0u8; 0]);
        assert_eq!(rig.command(&[0x13, 0x00, 0x57, 0x05]), [0u8; 0]);
        assert_eq!(rig.command(&[0x94]), [0x10]);
        assert_eq!(rig.command(&[0x12, 0x81]), [0u8; 0]);
        assert_eq!(
            rig.command(&[0x0E]),
            [0, 0, 0, 0, 0xDF, 0x02, 18, 0x81, 0x57, 0x05]
        );
        // LOCK keeps CONFIGURE's settings through a reset.
        rig.out(DOR, 0x08);
        rig.out(DOR, 0x0C);
        assert!(!rig.fdc.irq(), "drive polling is off");
        assert_eq!(rig.command(&[0x0E])[8..], [0x57, 0x05]);
        assert_eq!(rig.command(&[0x14]), [0x00]);
        // A command it doesn't know.
        assert_eq!(rig.command(&[0x18]), [0x80]);
        // SENSE DRIVE STATUS: ready, two sides, track 0, the head.
        assert_eq!(rig.command(&[0x04, 0x04]), [0x3C]);
    }

    #[test]
    fn a_seek_steps_the_heads_and_clears_the_disk_change_line() {
        let mut rig = Rig::new();
        rig.out(DOR, 0x1C);
        assert_eq!(rig.fdc.read(DIR) & 0x80, 0x80, "a disk went in");
        assert_eq!(rig.command(&[0x0F, 0x00, 0x00]), [0u8; 0]);
        assert_eq!(rig.fdc.read(MSR), 0x81, "drive 0 busy until sensed");
        assert_eq!(rig.command(&[0x08]), [0x20, 0]);
        assert_eq!(rig.fdc.read(DIR) & 0x80, 0x80, "no step was made");
        // Seeking takes the steps' time.
        rig.out(DATA, 0x0F);
        rig.out(DATA, 0x00);
        rig.out(DATA, 10);
        assert!(!rig.fdc.irq());
        assert_eq!(
            rig.fdc.next_event(),
            Some(rig.now + ticks(60.0)),
            "ten steps of 6 ms"
        );
        rig.settle();
        assert_eq!(rig.command(&[0x08]), [0x20, 10]);
        assert_eq!(rig.fdc.read(DIR) & 0x80, 0, "a step with a disk in");
        // Relative seeks, out and in.
        assert_eq!(rig.command(&[0x8F, 0x00, 4]), [0u8; 0]);
        assert_eq!(rig.command(&[0x08]), [0x20, 6]);
        assert_eq!(rig.command(&[0xCF, 0x04, 2]), [0u8; 0]);
        assert_eq!(rig.command(&[0x08]), [0x24, 8]);
        // RECALIBRATE; another disk sets the line again.
        assert_eq!(rig.command(&[0x07, 0x00]), [0u8; 0]);
        assert_eq!(rig.command(&[0x08]), [0x20, 0]);
        rig.fdc.media_changed(0);
        assert_eq!(rig.fdc.read(DIR) & 0x80, 0x80);
        // An empty drive keeps its line set.
        rig.out(DOR, 0x2D);
        rig.seek(1, 3);
        assert_eq!(rig.fdc.read(DIR) & 0x80, 0x80);
    }

    #[test]
    fn the_bios_show_of_a_read_ends_at_once_moving_nothing() {
        let mut rig = Rig::new();
        rig.ready();
        rig.faked = true;
        rig.dma(SECTOR_SIZE, true);
        // Drive 1 has no disk: the show reads all the same.
        for drive in [0, 1] {
            assert_eq!(rig.command(&[0xE6, drive, 0, 0, 1, 2, 18, 0x1B, 0xFF]), [drive, 0, 0, 0, 0, 2, 2]);
        }
        // A write too, writing nothing.
        rig.dma(SECTOR_SIZE, false);
        assert_eq!(rig.command(&[0xC5, 0, 0, 0, 1, 2, 18, 0x1B, 0xFF]), [0, 0, 0, 0, 0, 2, 2]);
        assert_eq!(rig.sector_at(0), 0, "no data moved");
        assert!(rig.fdc.activity.is_empty() && !rig.fdc.irq());
    }

    #[test]
    fn read_data_moves_sectors_until_terminal_count() {
        let mut rig = Rig::new();
        rig.ready();
        rig.seek(0, 1);
        // MT: cylinder 1 head 0 sectors 17 and 18, then head 1 sector 1.
        rig.dma(3 * SECTOR_SIZE, true);
        assert_eq!(
            rig.command(&[0xE6, 0x00, 1, 0, 17, 2, 18, 0x1B, 0xFF]),
            [0x04, 0, 0, 1, 1, 2, 2]
        );
        assert_eq!(
            [rig.sector_at(0), rig.sector_at(512), rig.sector_at(1024)],
            [52, 53, 54]
        );
        assert_eq!(rig.fdc.activity, [(0, 52, 3)]);
        assert!(!rig.fdc.irq(), "reading the result took the interrupt");
        // Without MT the end of the track ends it: EN.
        rig.dma(2 * SECTOR_SIZE, true);
        assert_eq!(
            rig.command(&[0x46, 0x00, 1, 0, 18, 2, 18, 0x1B, 0xFF]),
            [0x40, 0x80, 0, 2, 0, 1, 2]
        );
        // The last sector of the cylinder with MT: the next cylinder's ID.
        rig.dma(SECTOR_SIZE, true);
        assert_eq!(
            rig.command(&[0xC6, 0x04, 1, 1, 18, 2, 18, 0x1B, 0xFF]),
            [0x04, 0, 0, 2, 0, 1, 2]
        );
        assert_eq!(rig.sector_at(0), 71);
    }

    #[test]
    fn reads_fail_as_the_controller_fails_them() {
        let mut rig = Rig::new();
        rig.ready();
        rig.dma(SECTOR_SIZE, true);
        // Another cylinder than the heads are on: no data, wrong cylinder.
        assert_eq!(
            rig.command(&[0x46, 0x00, 5, 0, 1, 2, 18, 0x1B, 0xFF]),
            [0x40, 0x04, 0x10, 5, 0, 1, 2]
        );
        // A sector the track hasn't.
        assert_eq!(
            rig.command(&[0x46, 0x00, 0, 0, 19, 2, 18, 0x1B, 0xFF])[..3],
            [0x40, 0x04, 0]
        );
        // At 250 kb/s a high density disk has no address marks.
        rig.out(DIR, 0x02);
        assert_eq!(rig.command(&[0x4A, 0x00])[..2], [0x40, 0x01]);
        rig.out(DIR, 0x00);
        // With its motor off, or without a disk, nothing turns: the command
        // waits, busy, until a reset.
        rig.out(DOR, 0x0C);
        assert!(rig.command(&[0x4A, 0x00]).is_empty());
        assert_eq!(rig.fdc.read(MSR) & (RQM | CB), CB);
        assert!(!rig.fdc.irq());
        // A reset ends it, with the four drives' polling statuses.
        let reset = |rig: &mut Rig| {
            rig.out(DOR, 0x08);
            rig.out(DOR, 0x1C);
            for d in 0..4 {
                assert_eq!(rig.command(&[0x08])[0], 0xC0 | d);
            }
        };
        reset(&mut rig);
        rig.out(DOR, 0x2D);
        assert!(rig.command(&[0x4A, 0x01]).is_empty());
        // With the channel masked, the data overruns.
        reset(&mut rig);
        rig.seek(0, 0);
        rig.dma.write(0x0A, 0x06);
        assert_eq!(
            rig.command(&[0x46, 0x00, 0, 0, 1, 2, 18, 0x1B, 0xFF])[..2],
            [0x40, 0x10]
        );
    }

    #[test]
    fn multi_track_stops_at_the_side_a_single_sided_disk_lacks() {
        let mut rig = Rig::new();
        // A 160 KB disk: 40 cylinders, one side, 8 sectors a track.
        let bytes: Vec<u8> = (0..320u16).flat_map(|lba| {
            let mut sector = [0u8; SECTOR_SIZE];
            sector[..2].copy_from_slice(&lba.to_le_bytes());
            sector
        }).collect();
        let geometry = Chs { cylinders: 40, heads: 1, sectors: 8 };
        rig.disks[0] = Some(Rc::new(
            DiskImage::from_memory("ss.img", MemoryImage::from(bytes), true, Some(geometry), false).unwrap(),
        ));
        rig.ready();
        rig.out(DIR, 0x02);
        rig.dma(2 * SECTOR_SIZE, true);
        // Sector 8 of side 0, then side 1, which has no address marks.
        assert_eq!(
            rig.command(&[0xE6, 0x00, 0, 0, 8, 2, 8, 0x2A, 0xFF]),
            [0x44, 0x01, 0, 0, 1, 1, 2]
        );
        assert_eq!(rig.sector_at(0), 7);
        assert_eq!(rig.sector_at(512), 0, "nothing more was moved");
    }

    #[test]
    fn eot_is_the_last_sector_of_the_track() {
        let mut rig = Rig::new();
        rig.ready();
        rig.dma(2 * SECTOR_SIZE, true);
        // EOT 1: sector 1 only, then EN with the next cylinder's ID.
        assert_eq!(
            rig.command(&[0x46, 0x00, 0, 0, 1, 2, 1, 0x1B, 0xFF]),
            [0x40, 0x80, 0, 1, 0, 1, 2]
        );
        assert_eq!(rig.sector_at(0), 0);
        assert_eq!(rig.sector_at(512), 0, "nothing more was moved");
        // With MT, EOT ends side 0 and the transfer goes on to side 1.
        rig.dma(2 * SECTOR_SIZE, true);
        assert_eq!(
            rig.command(&[0xC6, 0x00, 0, 0, 2, 2, 2, 0x1B, 0xFF]),
            [0x04, 0, 0, 0, 1, 2, 2]
        );
        assert_eq!([rig.sector_at(0), rig.sector_at(512)], [1, 18]);
    }

    #[test]
    fn read_deleted_data_finds_normal_data_marks() {
        let mut rig = Rig::new();
        rig.ready();
        // Without SK: the sector is read, CM is set, and the command ends
        // normally after it.
        rig.dma(2 * SECTOR_SIZE, true);
        assert_eq!(
            rig.command(&[0x4C, 0x00, 0, 0, 3, 2, 18, 0x1B, 0xFF]),
            [0x00, 0, 0x40, 0, 0, 4, 2]
        );
        assert_eq!(rig.sector_at(0), 2);
        assert_eq!(rig.sector_at(512), 0, "nothing more was moved");
        // With SK every sector is skipped to the end of the track.
        rig.dma(2 * SECTOR_SIZE, true);
        assert_eq!(
            rig.command(&[0x6C, 0x00, 0, 0, 17, 2, 18, 0x1B, 0xFF]),
            [0x40, 0x80, 0x40, 1, 0, 1, 2]
        );
        assert_eq!(rig.sector_at(0), 2, "nothing was moved");
    }

    #[test]
    fn read_id_finds_each_sector_in_turn() {
        let mut rig = Rig::new();
        rig.ready();
        rig.seek(0, 2);
        let sectors: Vec<u8> = (0..19).map(|_| rig.command(&[0x4A, 0x04])[5]).collect();
        assert_eq!(sectors[..18], (1..=18).collect::<Vec<u8>>()[..]);
        assert_eq!(sectors[18], 1);
        assert_eq!(rig.command(&[0x4A, 0x04])[..5], [0x04, 0, 0, 2, 1]);
    }

    #[test]
    fn write_data_and_format_track_write_the_image() {
        let mut rig = Rig::new();
        rig.ready();
        rig.ram[0x10000..0x10400].fill(0xA5);
        rig.dma(2 * SECTOR_SIZE, false);
        assert_eq!(
            rig.command(&[0x45, 0x00, 0, 0, 3, 2, 18, 0x1B, 0xFF]),
            [0x00, 0, 0, 0, 0, 5, 2]
        );
        let disk = rig.disks[0].clone().unwrap();
        let mut sector = [0u8; SECTOR_SIZE];
        disk.read(2, &mut sector).unwrap();
        assert_eq!(sector, [0xA5; SECTOR_SIZE]);
        disk.read(3, &mut sector).unwrap();
        assert_eq!(sector, [0xA5; SECTOR_SIZE]);
        disk.read(4, &mut sector).unwrap();
        assert_eq!(sector[0], 4);
        // FORMAT TRACK: head 1 of cylinder 0, the IDs from memory.
        for r in 1..=18u8 {
            rig.ram[0x10000 + 4 * (r as usize - 1)..][..4].copy_from_slice(&[0, 1, r, 2]);
        }
        rig.dma(18 * 4, false);
        assert_eq!(
            rig.command(&[0x4D, 0x04, 2, 18, 0x54, 0xF6])[..3],
            [0x04, 0, 0]
        );
        for lba in 18..36 {
            disk.read(lba, &mut sector).unwrap();
            assert_eq!(sector, [0xF6; SECTOR_SIZE]);
        }
        // A write-protected disk.
        rig.disks[0] = Some(disk_read_only());
        rig.dma(SECTOR_SIZE, false);
        assert_eq!(
            rig.command(&[0x45, 0x00, 0, 0, 1, 2, 18, 0x1B, 0xFF])[..2],
            [0x40, 0x02]
        );
        assert_eq!(rig.command(&[0x04, 0x00])[0] & 0x40, 0x40);
    }

    fn disk_read_only() -> Rc<DiskImage> {
        disk(true)
    }

    #[test]
    fn the_state_comes_back() {
        let mut rig = Rig::new();
        rig.ready();
        rig.seek(0, 7);
        rig.out(DATA, 0x0F);
        rig.out(DATA, 0x00);
        rig.out(DATA, 9);
        let mut w = crate::savestate::Writer::new();
        rig.fdc.save(&mut w);
        let mut back = Fdc::default();
        back.load(&mut crate::savestate::Reader::new(&w.buf))
            .unwrap();
        rig.fdc = back;
        assert_eq!(rig.fdc.read(MSR), 0x81);
        rig.settle();
        assert_eq!(rig.command(&[0x08]), [0x20, 9]);
        assert_eq!(rig.fdc.read(DIR) & 0x80, 0);
    }
}
