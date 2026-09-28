//! A 3dfx Voodoo Graphics (SST-1): the PCI card DOS and Windows 95 games
//! program through Glide, which draws textured, shaded, depth-buffered
//! triangles into its own frame buffer and puts that on the monitor in
//! place of the VGA's picture, which it otherwise passes through.
//!
//! The card is a port of DOSBox-X's Voodoo emulation (voodoo_emu.cpp,
//! voodoo_interface.cpp, voodoo.cpp and the SST device of pci_bus.cpp),
//! itself MAME's SST-1 core by Aaron Giles, with what DOSBox-X leaves out
//! filled in from MAME: the gamma table (clutData), swaps that wait for the
//! vertical retrace as the swapbufferCMD asks, the pending swap count and
//! busy bits the status register reports meanwhile, and the vertical
//! retrace from the card's own video timing.
//!
//! The card sits on the PCI bus as device 0 (vendor 121Ah, device 0001h,
//! a multimedia video device), where DOSBox-X has it and so where a
//! Windows 95 installed there knows it. Its 16 MB BAR0 window holds the
//! registers (0-3FFFFFh), the linear frame buffer (400000h-7FFFFFh) and
//! the texture memory (800000h up).
//!
//! `voodoo_memory` gives it DOSBox-X's `voodoo_maxmem` board, a 4 MB frame
//! buffer and two texture units with 4 MB each, or a retail board's 2 MB
//! frame buffer and one texture unit with 2 MB.

pub mod backlog;
pub mod lfb;
pub mod mem;
pub mod mirror;
pub mod raster;
pub mod register;
pub mod regs;
pub mod setup;
pub mod tables;
pub mod texture;
pub mod workers;

use crate::savestate::{Reader, Result, State, StateError, Writer};
use crate::timer::PIT_HZ;
use crate::video::crt::CrtTiming;
use mem::Vram;
use raster::Stats;
use regs::*;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use texture::Tmu;
use backlog::{Access, Backlog, TileLayout};
use workers::{Job, Word};

/// The size of the card's memory window.
pub const WINDOW: u32 = 16 << 20;
/// Where the BIOS puts it (DOSBox-X's `VOODOO_INITIAL_LFB`).
pub const INITIAL_BASE: u32 = 0xD000_0000;

/// The two boards.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Board {
    /// 2 MB frame buffer, one texture unit with 2 MB (4 MB in all).
    Standard,
    /// 4 MB frame buffer, two texture units with 4 MB each (12 MB).
    #[default]
    Max,
}

crate::state_enum!(Board { Board::Standard, Board::Max });

impl Board {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "4" | "4mb" => Some(Board::Standard),
            "12" | "12mb" => Some(Board::Max),
            _ => None,
        }
    }

    /// The memory in MB, as `voodoo_memory` names it.
    pub fn megabytes(self) -> u32 {
        match self {
            Board::Standard => 4,
            Board::Max => 12,
        }
    }

    fn frame_buffer(self) -> usize {
        match self {
            Board::Standard => 2 << 20,
            Board::Max => 4 << 20,
        }
    }

    fn texture_units(self) -> usize {
        match self {
            Board::Standard => 1,
            Board::Max => 2,
        }
    }

    fn texture_memory(self) -> usize {
        match self {
            Board::Standard => 2 << 20,
            Board::Max => 4 << 20,
        }
    }
}

/// What draws the card's triangles: the emulator's own rasterizer, or
/// the host's OpenGL.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Renderer {
    #[default]
    Software,
    OpenGl,
}

impl Renderer {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "software" => Some(Renderer::Software),
            "opengl" => Some(Renderer::OpenGl),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Renderer::Software => "software",
            Renderer::OpenGl => "opengl",
        }
    }
}

/// The `voodoo` settings: whether there is a card, which board, and how
/// the host draws for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VoodooSettings {
    pub enabled: bool,
    pub board: Board,
    pub renderer: Renderer,
    /// How many times the native size OpenGL draws at.
    pub scale: u32,
}

impl Default for VoodooSettings {
    fn default() -> Self {
        Self { enabled: false, board: Board::Max, renderer: Renderer::Software, scale: 2 }
    }
}

impl VoodooSettings {
    /// The card the machine has, if any.
    pub fn board(&self) -> Option<Board> {
        self.enabled.then_some(self.board)
    }
}

/// The time of an access, in the two units the card works with.
#[derive(Clone, Copy, Debug, Default)]
pub struct Now {
    pub ns: u64,
    pub ticks: u64,
}

/// What a write asks of the machine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Effect {
    /// A swap waits for a retrace: the next event changed.
    pub reschedule: bool,
    /// The FIFO is full behind a pending swap: the CPU waits until PIT
    /// tick `stall_to`.
    pub stall_to: Option<u64>,
}

/// A swap waiting for the vertical retrace it is due at: the retrace's
/// number (`CrtTiming::retraces`) and when it begins, in PIT ticks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PendingSwap {
    pub retrace: u64,
    pub due: u64,
}

crate::state_fields!(PendingSwap { retrace, due });

/// The frame buffer interface chip (FBI): the frame buffer and what is
/// drawn into it.
#[derive(Clone, Debug)]
pub struct Fbi {
    pub ram: Vram,
    /// Byte mask of the frame buffer.
    pub mask: u32,
    /// Byte offsets of the three colour buffers and the auxiliary buffer,
    /// NONE where there is none (`recompute_video_memory`).
    pub rgboffs: [u32; 3],
    pub auxoffs: u32,
    pub frontbuf: u8,
    pub backbuf: u8,
    /// Y origin at the bottom: rows count up from this line.
    pub yorigin: u32,
    pub width: u32,
    pub height: u32,
    pub rowpixels: u32,
    /// The memory FIFO's size in entries, 0 when it is off.
    pub fifo_size: u32,

    /// The triangle's vertices (12.4) and start values and gradients.
    pub ax: i16,
    pub ay: i16,
    pub bx: i16,
    pub by: i16,
    pub cx: i16,
    pub cy: i16,
    pub startr: i32,
    pub startg: i32,
    pub startb: i32,
    pub starta: i32,
    pub startz: i32,
    pub startw: i64,
    pub drdx: i32,
    pub dgdx: i32,
    pub dbdx: i32,
    pub dadx: i32,
    pub dzdx: i32,
    pub dwdx: i64,
    pub drdy: i32,
    pub dgdy: i32,
    pub dbdy: i32,
    pub dady: i32,
    pub dzdy: i32,
    pub dwdy: i64,

    /// The fog table.
    pub fogblend: [u8; 64],
    pub fogdelta: [u8; 64],
}

pub const NONE: u32 = u32::MAX;

crate::state_fields!(Fbi {
    ram, frontbuf, backbuf, width, height,
    ax, ay, bx, by, cx, cy,
    startr, startg, startb, starta, startz, startw,
    drdx, dgdx, dbdx, dadx, dzdx, dwdx, drdy, dgdy, dbdy, dady, dzdy, dwdy,
    fogblend, fogdelta,
} skip { mask, rgboffs, auxoffs, yorigin, rowpixels, fifo_size });

impl State for Vram {
    fn save(&self, w: &mut Writer) {
        (self.bytes() as u64).save(w);
        w.bytes(&self.to_bytes());
    }
    fn load(&mut self, r: &mut Reader) -> Result<()> {
        let len = r.count()?;
        if len != self.bytes() {
            return Err(StateError::Mismatch(format!(
                "its 3dfx card has {} KB where this one has {} KB",
                len / 1024,
                self.bytes() / 1024
            )));
        }
        self.load_bytes(r.take(len)?);
        Ok(())
    }
}

impl Fbi {
    fn new(bytes: usize) -> Self {
        Self {
            ram: Vram::new(bytes),
            mask: bytes as u32 - 1,
            rgboffs: [0; 3],
            auxoffs: NONE,
            frontbuf: 0,
            backbuf: 1,
            yorigin: 0,
            width: 640,
            height: 480,
            rowpixels: 640,
            fifo_size: 0,
            ax: 0,
            ay: 0,
            bx: 0,
            by: 0,
            cx: 0,
            cy: 0,
            startr: 0,
            startg: 0,
            startb: 0,
            starta: 0,
            startz: 0,
            startw: 0,
            drdx: 0,
            dgdx: 0,
            dbdx: 0,
            dadx: 0,
            dzdx: 0,
            dwdx: 0,
            drdy: 0,
            dgdy: 0,
            dbdy: 0,
            dady: 0,
            dzdy: 0,
            dwdy: 0,
            fogblend: [0; 64],
            fogdelta: [0; 64],
        }
    }
}

/// The card's PCI configuration: the command register, BAR0, initEnable
/// and the video clock (config C0h turns it on, E0h off).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PciConfig {
    pub command: u16,
    pub bar: u32,
    pub init_enable: u32,
    pub clock_enabled: bool,
}

crate::state_fields!(PciConfig { command, bar, init_enable, clock_enabled });

impl Default for PciConfig {
    fn default() -> Self {
        Self { command: 0x0002, bar: INITIAL_BASE, init_enable: 0, clock_enabled: false }
    }
}

/// The card.
pub struct Voodoo {
    pub board: Board,
    chipmask: u32,
    tmu_config: u32,
    /// The registers: the FBI's at 0, TMU 0's at 100h, TMU 1's at 200h.
    pub reg: Box<[u32; 0x400]>,
    pub pci: PciConfig,
    dac: [u8; 8],
    dac_read: u8,
    pub fbi: Fbi,
    pub tmu: Vec<Tmu>,
    send_config: bool,
    /// The pixel counters not yet added to the registers.
    stats: Stats,
    /// The gamma table: 33 entries of RGB (clutData).
    clut: [u32; 33],
    /// Swaps waiting for their retrace, oldest first; the retrace the last
    /// swap happened at; and the FIFO writes since the oldest pending one.
    pub swaps: VecDeque<PendingSwap>,
    last_swap: u64,
    fifo_writes: u32,
    /// The picture changed since the display last looked, or the card
    /// took or gave back the monitor.
    display_dirty: bool,
    /// Whether the display last saw the card's picture on the screen.
    showing: bool,
    /// Buffer swaps since the bus last counted them as frames drawn.
    pub swapped: u64,
    /// The video timing the card's registers describe.
    timing: CrtTiming,
    /// RGB of each 5-6-5 pixel through the gamma table, rebuilt when it
    /// changes.
    palette: Vec<[u8; 3]>,
    palette_dirty: bool,
    /// The workers that draw, and the colour buffers drawn into since
    /// they last finished.
    pool: workers::Pool,
    drawn_to: Vec<usize>,
    /// Jobs were given to the workers since they last finished, and the
    /// texture units queued triangles read.
    outstanding: Cell<bool>,
    tmu_in_use: Cell<u8>,
    /// Frame buffer writes behind queued jobs, not yet a job themselves.
    words: RefCell<Vec<Word>>,
    /// While the OpenGL renderer draws the picture: the jobs held back
    /// until something reads the memory (`backlog`), whether they may be
    /// (`RUST_DOS_VOODOO_PRUNE=0` says no), whether a program read the
    /// pixel counters, which dropped jobs would miss, and how often
    /// something made them draw.
    backlog: RefCell<Backlog>,
    prune: bool,
    counters_read: Cell<bool>,
    catch_ups: Cell<u64>,
    /// Whether the display wants the card's picture from its memory,
    /// which it doesn't while the OpenGL renderer draws it.
    software_picture: bool,
    /// The drawing recorded for the OpenGL renderer, while it draws.
    mirror: Option<Box<mirror::Mirror>>,
    /// Messages already logged once.
    logged: u32,
    /// Lines for the log, which the bus writes out.
    pub log: Vec<String>,
}

/// Frame buffer writes that go out to the workers together.
const WORDS_A_JOB: usize = 4096;

/// One-time log messages.
const LOG_BYTE: u32 = 1;
const LOG_RESERVED: u32 = 2;
const LOG_DIRECT: u32 = 4;

impl Voodoo {
    pub fn new(board: Board) -> Self {
        Self::with_workers(board, workers::Pool::default_workers())
    }

    /// A card that draws with `workers` threads (0: none).
    pub fn with_workers(board: Board, workers: usize) -> Self {
        let units = board.texture_units();
        let mut v = Self {
            board,
            chipmask: if units == 2 { 0x07 } else { 0x03 },
            tmu_config: if units == 2 { 0xD1 } else { 0x11 },
            reg: Box::new([0; 0x400]),
            pci: PciConfig::default(),
            dac: [0; 8],
            dac_read: 0,
            fbi: Fbi::new(board.frame_buffer()),
            tmu: (0..units).map(|i| Tmu::new(board.texture_memory(), 0x100 * (i + 1))).collect(),
            send_config: false,
            stats: Stats::default(),
            clut: default_clut(),
            swaps: VecDeque::new(),
            last_swap: 0,
            fifo_writes: 0,
            display_dirty: true,
            showing: false,
            swapped: 0,
            timing: CrtTiming::VESA_480,
            palette: vec![[0; 3]; 65536],
            palette_dirty: true,
            pool: workers::Pool::new(workers),
            drawn_to: Vec::new(),
            outstanding: Cell::new(false),
            tmu_in_use: Cell::new(0),
            words: RefCell::new(Vec::new()),
            backlog: RefCell::new(Backlog::default()),
            prune: std::env::var("RUST_DOS_VOODOO_PRUNE").map_or(true, |v| v != "0"),
            counters_read: Cell::new(false),
            catch_ups: Cell::new(0),
            software_picture: true,
            mirror: None,
            logged: 0,
            log: Vec::new(),
        };
        v.reset();
        v
    }

    /// A PCI reset: the power-on registers, the monitor given back to the
    /// VGA, nothing pending. The memory keeps what it had.
    pub fn reset(&mut self) {
        self.flush();
        self.pool.reset_stats();
        self.counters_read.set(false);
        self.reg.fill(0);
        self.pci = PciConfig::default();
        self.dac = [0; 8];
        self.dac_read = 0;
        self.send_config = false;
        self.stats = Stats::default();
        self.clut = default_clut();
        self.swaps.clear();
        self.last_swap = 0;
        self.fifo_writes = 0;
        self.fbi.frontbuf = 0;
        self.fbi.backbuf = 1;
        self.fbi.width = 640;
        self.fbi.height = 480;
        self.fbi.fogblend = [0; 64];
        self.fbi.fogdelta = [0; 64];
        for tmu in &mut self.tmu {
            tmu.reset();
        }
        self.reg[FBI_INIT0] = (1 << 4) | (0x10 << 6);
        self.reg[FBI_INIT1] = (1 << 1) | (1 << 8) | (1 << 12) | (2 << 20);
        self.reg[FBI_INIT2] = (1 << 6) | (0x100 << 23);
        self.reg[FBI_INIT3] = (2 << 13) | (0xF << 17);
        self.reg[FBI_INIT4] = 1;
        self.fbi.yorigin = 0;
        self.recompute_video_memory();
        self.timing = CrtTiming::VESA_480;
        self.palette_dirty = true;
        self.display_dirty = true;
        if let Some(mirror) = &mut self.mirror {
            mirror.forget_textures();
        }
    }

    // --- PCI ---

    /// A byte of the configuration space.
    pub fn config_read(&self, reg: u8) -> u8 {
        let dword = match reg & 0xFC {
            0x00 => 0x0001_121A,
            0x04 => 0x0080_0000 | self.pci.command as u32,
            // Revision 2, a multimedia video device.
            0x08 => 0x0400_0002,
            0x10 => self.pci.bar | 0x08,
            0x3C => 0x0000_00FF,
            0x40 => self.pci.init_enable,
            _ => 0,
        };
        (dword >> (8 * (reg & 3))) as u8
    }

    /// A byte of the configuration space written. True if the card's
    /// window or its video clock changed.
    pub fn config_write(&mut self, reg: u8, value: u8) -> bool {
        match reg {
            0x04 => self.pci.command = (self.pci.command & 0xFF00) | (value & 0x23) as u16,
            0x05 => self.pci.command = (self.pci.command & 0x00FF) | ((value & 0x01) as u16) << 8,
            // BAR0: 16 MB aligned, so only its top byte.
            0x13 => {
                self.pci.bar = (value as u32) << 24;
                return true;
            }
            0x40..=0x43 => {
                let shift = 8 * (reg - 0x40) as u32;
                self.pci.init_enable = (self.pci.init_enable & !(0xFF << shift)) | (value as u32) << shift;
            }
            0xC0 | 0xE0 => {
                let on = reg == 0xC0;
                if on != self.pci.clock_enabled {
                    self.pci.clock_enabled = on;
                    self.display_dirty = true;
                }
                return true;
            }
            _ => {}
        }
        false
    }

    /// Where the card's window is, if it has one.
    pub fn base(&self) -> Option<u32> {
        (self.pci.bar != 0).then_some(self.pci.bar)
    }

    /// Whether writes to fbiInit0-4 are allowed (initEnable bit 0).
    fn init_writes(&self) -> bool {
        self.pci.init_enable & 1 != 0
    }

    // --- The memory window ---

    /// A dword of the window at byte `offset`, 4-aligned (`voodoo_r`).
    pub fn read(&self, offset: u32, now: Now) -> u32 {
        let index = (offset >> 2) & 0x3F_FFFF;
        if index & (0xC0_0000 / 4) == 0 {
            self.register_read(index, now)
        } else if index & (0x80_0000 / 4) == 0 {
            self.lfb_read(index)
        } else {
            0xFFFF_FFFF
        }
    }

    /// A dword of the window written, or the half `mask` selects
    /// (`voodoo_w`).
    pub fn write(&mut self, offset: u32, data: u32, mask: u32, now: Now) -> Effect {
        let index = (offset >> 2) & 0x3F_FFFF;
        let mut effect = Effect::default();
        let fifo = if index & (0xC0_0000 / 4) == 0 {
            self.register_write(index, data, now, &mut effect)
        } else if index & (0x80_0000 / 4) == 0 {
            self.lfb_write(index, data, mask);
            true
        } else {
            self.texture_write(index, data);
            true
        };
        if fifo {
            self.fifo_write(now, &mut effect);
        }
        effect
    }

    /// Byte accesses, which the card doesn't take.
    pub fn byte_write(&mut self) {
        self.log_once(LOG_BYTE, "[3DFX] Byte writes to the card are ignored");
    }

    fn log_once(&mut self, flag: u32, message: &str) {
        if self.logged & flag == 0 {
            self.logged |= flag;
            self.log.push(message.to_string());
        }
    }

    /// The frame buffer's memory, with everything drawn into it.
    pub fn frame_buffer(&self) -> &Vram {
        self.catch_up();
        &self.fbi.ram
    }

    /// Wait until the workers drew everything given to them.
    pub fn flush(&mut self) {
        self.catch_up();
        self.drawn_to.clear();
    }

    /// Draw everything queued, and wait for it.
    pub(crate) fn catch_up(&self) {
        if !self.backlog.borrow().is_empty() {
            self.catch_ups.set(self.catch_ups.get() + 1);
        }
        self.submit_words();
        self.release_backlog();
        self.pool.flush();
        self.outstanding.set(false);
        self.tmu_in_use.set(0);
    }

    /// Whether jobs are held back (`backlog`): while the OpenGL renderer
    /// draws, unless a program reads the pixel counters or the buffers'
    /// layout gives the tiles no meaning.
    fn deferring(&self) -> bool {
        self.mirror.is_some() && self.prune && !self.counters_read.get() && self.backlog.borrow().layout.prunable
    }

    /// What a job does to the tiles, when jobs are held back.
    pub(crate) fn access(&self, f: impl FnOnce(&TileLayout) -> Access) -> Option<Access> {
        self.deferring().then(|| f(&self.backlog.borrow().layout))
    }

    /// Queue `job` after the frame buffer writes before it: held back
    /// with its `access`, or given to the workers.
    pub(crate) fn submit(&self, job: Job, access: Option<Access>) {
        self.submit_words();
        if let Job::Triangle { texcount, .. } = &job {
            self.tmu_in_use.set(self.tmu_in_use.get() | ((1u8 << texcount) - 1));
        }
        match access {
            Some(access) if self.deferring() => {
                let out = self.backlog.borrow_mut().push(job, access);
                self.hand_over(out);
            }
            _ => self.hand_over([job]),
        }
    }

    /// Give the workers `jobs`, after those held back.
    fn hand_over(&self, jobs: impl IntoIterator<Item = Job>) {
        if !self.backlog.borrow().is_empty() && !self.deferring() {
            self.release_backlog();
        }
        for job in jobs {
            self.outstanding.set(self.pool.workers() > 0);
            self.pool.submit(job);
        }
    }

    /// The jobs held back, pruned, to the workers without waiting.
    fn release_backlog(&self) {
        if self.backlog.borrow().is_empty() {
            return;
        }
        let held = self.backlog.borrow_mut().take();
        for job in held {
            self.outstanding.set(self.pool.workers() > 0);
            self.pool.submit(job);
        }
    }

    /// The frame buffer writes queued behind jobs, as a job of their own,
    /// after the jobs held back.
    fn submit_words(&self) {
        let words = std::mem::take(&mut *self.words.borrow_mut());
        if !words.is_empty() {
            self.release_backlog();
            self.outstanding.set(self.pool.workers() > 0);
            self.pool.submit(Job::Pixels { fb: self.fbi.ram.clone(), words });
        }
    }

    /// Whether frame buffer writes have to wait their turn behind jobs.
    pub(crate) fn writes_queue(&self) -> bool {
        self.outstanding.get() || !self.words.borrow().is_empty() || !self.backlog.borrow().is_empty()
    }

    /// Frame buffer writes for the queue (`writes_queue`): held back with
    /// the jobs, or every so many out as a job, or straight into memory
    /// when the workers are done by then.
    pub(crate) fn queue_words(&self, new: &[Word]) {
        if self.deferring() && self.words.borrow().is_empty() {
            let out = self.backlog.borrow_mut().push_words(&self.fbi.ram, new);
            self.hand_over(out);
            return;
        }
        self.release_backlog();
        let mut words = self.words.borrow_mut();
        words.extend_from_slice(new);
        if words.len() < WORDS_A_JOB {
            return;
        }
        if self.pool.busy() {
            drop(words);
            self.submit_words();
        } else {
            let ram = &self.fbi.ram;
            for w in words.drain(..) {
                if (w.at as usize) < ram.len() {
                    ram.set(w.at as usize, w.value);
                }
            }
            self.outstanding.set(false);
            self.tmu_in_use.set(0);
        }
    }

    /// The pixel counters, the workers' included.
    fn counted(&self) -> Stats {
        self.catch_up();
        let mut stats = self.stats;
        stats.add(&self.pool.stats());
        stats
    }

    fn texture_write(&mut self, index: u32, data: u32) {
        let unit = ((index >> 19) & 3) as usize;
        if self.chipmask & (2 << unit) == 0 || unit >= self.tmu.len() {
            return;
        }
        // Queued triangles read the texture memory as they draw.
        if self.tmu_in_use.get() & (1 << unit) != 0 {
            self.flush();
        }
        if self.reg[self.tmu[unit].base + T_LOD] & (1 << 27) != 0 {
            self.log_once(LOG_DIRECT, "[3DFX] Direct texture writes (a Voodoo 2's) are ignored");
            return;
        }
        let seq8 = self.reg[0x100 + TEXTURE_MODE] & 0x8000_0000 != 0;
        let (reg, tmu) = (&self.reg[..], &mut self.tmu[unit]);
        tmu.write(reg, index, data, seq8);
    }

    // --- Video memory layout and output ---

    /// Where the buffers are, from fbiInit1, 2 and 4 (`recompute_video_memory`).
    pub(crate) fn recompute_video_memory(&mut self) {
        let buffer_pages = (self.reg[FBI_INIT2] >> 11) & 0x1FF;
        let fifo_start = (self.reg[FBI_INIT4] >> 8) & 0x3FF;
        let mut fifo_last = (self.reg[FBI_INIT4] >> 18) & 0x3FF;
        let fbi = &mut self.fbi;
        fbi.rowpixels = 64 * ((self.reg[FBI_INIT1] >> 4) & 0xF);
        fbi.rgboffs[0] = 0;
        fbi.rgboffs[1] = buffer_pages * 0x1000;
        if (self.reg[FBI_INIT2] >> 4) & 1 == 0 {
            // Two colour buffers and an auxiliary buffer.
            fbi.rgboffs[2] = NONE;
            fbi.auxoffs = 2 * buffer_pages * 0x1000;
        } else {
            // Three colour buffers and none.
            fbi.rgboffs[2] = 2 * buffer_pages * 0x1000;
            fbi.auxoffs = NONE;
        }
        for offs in &mut fbi.rgboffs {
            if *offs != NONE && *offs > fbi.mask {
                *offs = fbi.mask;
            }
        }
        if fbi.auxoffs != NONE && fbi.auxoffs > fbi.mask {
            fbi.auxoffs = fbi.mask;
        }
        fifo_last = fifo_last.min(fbi.mask / 0x1000);
        fbi.fifo_size = if fifo_start <= fifo_last && (self.reg[FBI_INIT0] >> 13) & 1 != 0 {
            ((fifo_last + 1 - fifo_start) * 0x1000 / 4).min(65536 * 2)
        } else {
            0
        };
        if fbi.rgboffs[2] == NONE {
            if fbi.frontbuf == 2 {
                fbi.frontbuf = 0;
            }
            if fbi.backbuf == 2 {
                fbi.backbuf = 0;
            }
        }
        self.update_tiles();
    }

    /// The tiles held-back jobs are worked out on, for the buffers where
    /// they are now. Jobs held back for the old ones go to the workers.
    pub(crate) fn update_tiles(&mut self) {
        let fbi = &self.fbi;
        let word = |offs: u32| (offs != NONE).then_some(offs as usize / 2);
        let bases = [word(fbi.rgboffs[0]), word(fbi.rgboffs[1]), word(fbi.rgboffs[2]), word(fbi.auxoffs)];
        let layout = TileLayout::new(bases, fbi.width, fbi.height, fbi.rowpixels, fbi.ram.len());
        if layout != self.backlog.borrow().layout {
            self.release_backlog();
            self.backlog.borrow_mut().layout = layout;
        }
    }

    /// Whether the card drives the monitor: its video clock runs and
    /// fbiInit0 turns off the VGA pass-through.
    pub fn output(&self) -> bool {
        self.pci.clock_enabled && self.reg[FBI_INIT0] & 1 != 0
    }

    /// The size of the picture it shows.
    pub fn size(&self) -> (u32, u32) {
        (self.fbi.width.max(1), self.fbi.height.max(1))
    }

    /// The refresh rate of its picture.
    pub fn refresh_hz(&self) -> f64 {
        self.timing.hz()
    }

    /// Get the picture ready to be drawn (the gamma table's colours), and
    /// say whether the screen changed: the picture shown did, or the card
    /// took or gave back the monitor.
    pub fn prepare_display(&mut self) -> bool {
        let output = self.output();
        let switched = std::mem::replace(&mut self.showing, output) != output;
        let changed = std::mem::take(&mut self.display_dirty);
        if !output {
            return switched;
        }
        if !self.picture_wanted() {
            // The OpenGL renderer draws it: nothing needs the memory.
            self.display_dirty |= changed;
            return switched;
        }
        // Jobs may still be drawing into the buffer shown.
        let front = self.fbi.rgboffs[self.fbi.frontbuf as usize];
        if front != NONE && self.drawn_to.contains(&(front as usize / 2)) {
            self.flush();
        }
        if self.palette_dirty {
            self.build_palette();
        }
        changed || switched
    }

    /// Draw the front buffer into `rgb` (RGB24, `width` pixels a row, the
    /// card's size), through the gamma table (see `prepare_display`).
    pub fn render(&self, rgb: &mut [u8], width: usize) {
        let (w, h) = (self.fbi.width as usize, self.fbi.height as usize);
        let base = self.fbi.rgboffs[self.fbi.frontbuf as usize];
        if base == NONE {
            return;
        }
        let base = base as usize / 2;
        let ram = &self.fbi.ram;
        for y in 0..h {
            let Some(row) = rgb.get_mut(y * width * 3..(y * width + w.min(width)) * 3) else { break };
            let src = base + y * self.fbi.rowpixels as usize;
            for (x, out) in row.chunks_exact_mut(3).enumerate() {
                let at = src + x;
                let pixel = if at < ram.len() { ram.get(at) } else { 0 };
                out.copy_from_slice(&self.palette[pixel as usize]);
            }
        }
    }

    /// The 5-6-5 pixels' colours through the gamma table: each component
    /// scaled to 8 bits and interpolated between the table's 33 entries
    /// (MAME's `screen_update`).
    fn build_palette(&mut self) {
        let table = gamma_table(&self.clut);
        let (mut rt, mut gt, mut bt) = ([0u8; 32], [0u8; 64], [0u8; 32]);
        for x in 0..32usize {
            let y = (x << 3) | (x >> 2);
            rt[x] = table[y][0];
            bt[x] = table[y][2];
        }
        for (v, g) in gt.iter_mut().enumerate() {
            *g = table[(v << 2) | (v >> 4)][1];
        }
        for (pixel, out) in self.palette.iter_mut().enumerate() {
            *out = [rt[pixel >> 11], gt[(pixel >> 5) & 0x3F], bt[pixel & 0x1F]];
        }
        self.palette_dirty = false;
    }

    // --- Timing, swaps and the FIFO ---

    /// The video timing: the card's registers' frame at 60 Hz, which is
    /// what DOSBox-X always runs the card at.
    fn update_timing(&mut self) {
        let (h, v, dims) = (self.reg[H_SYNC], self.reg[V_SYNC], self.reg[VIDEO_DIMENSIONS]);
        let vtotal = ((v >> 16) & 0xFFF) + (v & 0xFFF);
        let vvis = (dims >> 16) & 0x3FF;
        if h == 0 || v == 0 || dims == 0 || vvis == 0 || vtotal <= vvis {
            return;
        }
        let line_ns = (1_000_000_000 / 60 / vtotal as u64) as u32;
        let htotal = ((h >> 16) & 0x3FF) + 1 + (h & 0xFF) + 1;
        let hvis = (dims & 0x3FF).min(htotal);
        let sync = (v & 0xFFF).clamp(1, vtotal - vvis);
        self.timing = CrtTiming {
            line_ns,
            hdisplay_ns: (line_ns as u64 * hvis as u64 / htotal.max(1) as u64) as u32,
            total: vtotal,
            display: vvis,
            retrace_start: vvis,
            retrace_end: vvis + sync,
        };
    }

    /// The number of retraces that began up to `ns`.
    fn retraces(&self, ns: u64) -> u64 {
        self.timing.retraces(ns)
    }

    /// When retrace number `k` begins, in PIT ticks, rounded up.
    fn retrace_ticks(&self, k: u64) -> u64 {
        let frame = self.timing.frame_ns();
        let ns = k.saturating_sub(1) * frame + self.timing.retrace_start as u64 * self.timing.line_ns as u64;
        (ns as u128 * PIT_HZ as u128).div_ceil(1_000_000_000) as u64
    }

    /// swapbufferCMD: the buffers swap now; with bit 0, the swap also
    /// stays pending until the retrace it waits for, `bits 8:1` retraces
    /// after the last swap and at least the next.
    pub(crate) fn swap(&mut self, data: u32, now: Now, effect: &mut Effect) {
        self.rotate_buffers();
        let current = self.retraces(now.ns);
        if data & 1 == 0 {
            self.last_swap = current;
            return;
        }
        let interval = ((data >> 1) & 0xFF) as u64;
        let after = self.swaps.back().map_or(self.last_swap, |s| s.retrace);
        let retrace = (current + 1).max(after + interval.max(1));
        let due = self.retrace_ticks(retrace);
        self.swaps.push_back(PendingSwap { retrace, due });
        effect.reschedule = true;
    }

    fn rotate_buffers(&mut self) {
        let fbi = &mut self.fbi;
        if fbi.rgboffs[2] == NONE {
            fbi.frontbuf = 1 - fbi.frontbuf.min(1);
            fbi.backbuf = 1 - fbi.frontbuf;
        } else {
            fbi.frontbuf = (fbi.frontbuf + 1) % 3;
            fbi.backbuf = (fbi.frontbuf + 1) % 3;
        }
        self.display_dirty = true;
        self.swapped += 1;
    }

    /// Swaps still pending at `ticks`.
    pub fn pending_swaps(&self, ticks: u64) -> usize {
        self.swaps.iter().filter(|s| s.due > ticks).count()
    }

    /// When the card next needs attention: its oldest pending swap's
    /// retrace.
    pub fn next_event(&self) -> Option<u64> {
        self.swaps.front().map(|s| s.due)
    }

    /// Retire the swaps whose retraces came.
    pub fn service(&mut self, ticks: u64) {
        while let Some(front) = self.swaps.front().copied() {
            if front.due > ticks {
                break;
            }
            self.swaps.pop_front();
            self.last_swap = front.retrace;
            self.fifo_writes = 0;
        }
    }

    /// A write that goes through the FIFO: behind a pending swap it waits
    /// there, and a full FIFO stalls the CPU until the swap is done.
    fn fifo_write(&mut self, now: Now, effect: &mut Effect) {
        self.service(now.ticks);
        let Some(front) = self.swaps.front() else { return };
        self.fifo_writes += 1;
        let capacity = if self.fbi.fifo_size > 0 { self.fbi.fifo_size } else { 64 };
        if self.fifo_writes > capacity {
            effect.stall_to = Some(front.due);
        }
    }

    /// The status register (`register_r`'s status case).
    fn status(&self, now: Now) -> u32 {
        let pending = self.pending_swaps(now.ticks) as u32;
        let mut result = 0x3F;
        if self.timing.status(now.ns) & 0x08 != 0 {
            result |= 1 << 6;
        }
        if pending > 0 {
            result |= 7 << 7;
        }
        result |= (self.fbi.frontbuf as u32) << 10;
        let free = if pending > 0 && self.fbi.fifo_size > 0 {
            self.fbi.fifo_size.saturating_sub(self.fifo_writes).min(0xFFFF)
        } else {
            0xFFFF
        };
        result |= free << 12;
        result |= pending.min(7) << 28;
        result
    }

    /// The scanline the card's video is on, for vRetrace.
    fn scanline(&self, now: Now) -> u32 {
        let frame = self.timing.frame_ns().max(1);
        ((now.ns % frame) / self.timing.line_ns.max(1) as u64) as u32
    }

    /// After a load: what is worked out from the registers.
    pub fn after_load(&mut self) {
        self.recompute_video_memory();
        self.fbi.yorigin = (self.reg[FBI_INIT3] >> 22) & 0x3FF;
        let reg = self.reg.clone();
        for tmu in &mut self.tmu {
            tmu.rebuild(&reg[..]);
        }
        self.update_timing();
        self.palette_dirty = true;
        self.display_dirty = true;
        if let Some(mirror) = &mut self.mirror {
            mirror.invalidate();
            mirror.forget_textures();
        }
    }

    // --- The OpenGL renderer's recording ---

    /// Record the drawing for the OpenGL renderer, or stop.
    pub fn set_mirror(&mut self, on: bool) {
        if on != self.mirror.is_some() {
            self.mirror = on.then(|| Box::new(mirror::Mirror::new()));
            self.release_backlog();
        }
    }

    /// Whether the display wants the card's picture from its memory (the
    /// OpenGL renderer draws it otherwise): screenshots and recordings,
    /// the debugger, overlays that mix with it.
    pub fn set_software_picture(&mut self, on: bool) {
        if on && !self.software_picture {
            self.display_dirty = true;
        }
        self.software_picture = on;
    }

    /// Whether the display draws the card's picture from its memory.
    pub fn picture_wanted(&self) -> bool {
        self.mirror.is_none() || self.software_picture
    }

    pub fn mirror_attached(&self) -> bool {
        self.mirror.is_some()
    }

    /// Take the buffers' pixels for the recording if their layout changed
    /// since it last did, or it wants them.
    pub(crate) fn mirror_sync(&mut self) {
        let Some(mirror) = &self.mirror else { return };
        let fbi = &self.fbi;
        let key = mirror::LayoutKey { width: fbi.width, height: fbi.height, rowpixels: fbi.rowpixels, color: fbi.rgboffs, aux: fbi.auxoffs };
        if !mirror.needs_snapshot(&key) {
            return;
        }
        self.flush();
        let snapshot = self.snapshot(self.mirror_layout());
        if let Some(mirror) = &mut self.mirror {
            mirror.resync(snapshot, key);
        }
    }

    fn mirror_layout(&self) -> mirror::Layout {
        let fbi = &self.fbi;
        mirror::Layout {
            width: fbi.width,
            height: fbi.height,
            rowpixels: fbi.rowpixels,
            color: fbi.rgboffs.iter().filter(|&&offs| offs != NONE).map(|&offs| offs / 2).collect(),
            aux: (fbi.auxoffs != NONE).then_some(fbi.auxoffs / 2),
        }
    }

    fn snapshot(&self, layout: mirror::Layout) -> mirror::Snapshot {
        let ram = &self.fbi.ram;
        let (width, height, rowpixels) = (layout.width as usize, layout.height as usize, layout.rowpixels as usize);
        let pixels = |base: u32| -> Vec<u16> {
            let mut out = Vec::with_capacity(width * height);
            for y in 0..height {
                let row = base as usize + y * rowpixels;
                out.extend((row..row + width).map(|at| if at < ram.len() { ram.get(at) } else { 0 }));
            }
            out
        };
        mirror::Snapshot { color: layout.color.iter().map(|&offs| pixels(offs)).collect(), aux: layout.aux.map(pixels), layout }
    }

    /// The drawing recorded since the last frame, and what the card shows.
    pub fn take_mirror(&mut self) -> Option<mirror::Frame> {
        self.mirror_sync();
        let front = self.fbi.rgboffs[self.fbi.frontbuf as usize];
        let output = self.output();
        let commands = self.mirror.as_mut()?.take();
        Some(mirror::Frame {
            commands,
            front: (front != NONE).then_some(front / 2),
            output,
            width: self.fbi.width,
            height: self.fbi.height,
            clut: self.clut,
        })
    }

    /// Pixel `x` of buffer row `y` written through the frame buffer: tell
    /// the recording its colour in buffer `dest` (a word offset) and its
    /// auxiliary buffer's value, those written.
    pub(crate) fn mirror_pixel(&mut self, dest: usize, x: i32, y: i32, color: Option<u16>, aux: Option<u16>) {
        let Some(mirror) = &mut self.mirror else { return };
        let fbi = &self.fbi;
        if x < 0 || y < 0 || x as u32 >= fbi.width || y as u32 >= fbi.height {
            return;
        }
        if let Some(value) = color {
            mirror.pixel(Some(dest as u32), x as u32, y as u32, value, fbi.width);
        }
        if let Some(value) = aux {
            mirror.pixel(None, x as u32, y as u32, value, fbi.width);
        }
    }

    /// The same for a write through the pixel pipeline, whose results are
    /// in memory: the colour, and with `aux` the auxiliary buffer's value.
    pub(crate) fn mirror_written(&mut self, dest: usize, x: i32, y: i32, aux: bool) {
        if self.mirror.is_none() || x < 0 || y < 0 {
            return;
        }
        let fbi = &self.fbi;
        let at = y as usize * fbi.rowpixels as usize + x as usize;
        let word = |offs: usize| (offs + at < fbi.ram.len()).then(|| fbi.ram.get(offs + at));
        let color = word(dest);
        let aux = if aux && fbi.auxoffs != NONE { word(fbi.auxoffs as usize / 2) } else { None };
        self.mirror_pixel(dest, x, y, color, aux);
    }

    /// For the debugger: what the card is doing.
    pub fn describe(&self, now: Now) -> serde_json::Value {
        let init: Vec<String> =
            [FBI_INIT0, FBI_INIT1, FBI_INIT2, FBI_INIT3, FBI_INIT4].iter().map(|&r| format!("{:08X}", self.reg[r])).collect();
        serde_json::json!({
            "board_mb": self.board.megabytes(),
            "base": format!("{:08X}", self.pci.bar),
            "clock": self.pci.clock_enabled,
            "output": self.output(),
            "width": self.fbi.width,
            "height": self.fbi.height,
            "hz": (self.refresh_hz() * 100.0).round() / 100.0,
            "front": self.fbi.frontbuf,
            "back": self.fbi.backbuf,
            "pending_swaps": self.pending_swaps(now.ticks),
            "fifo_writes": self.fifo_writes,
            "triangles": self.reg[FBI_TRIANGLES_OUT],
            "fbiInit": init,
            "init_enable": format!("{:08X}", self.pci.init_enable),
            "opengl": self.mirror.is_some(),
            "held_jobs": self.backlog.borrow().len(),
            "pruned_jobs": self.backlog.borrow().pruned,
            "catch_ups": self.catch_ups.get(),
            "counters_read": self.counters_read.get(),
            "software_picture": self.picture_wanted(),
        })
    }
}

/// What the gamma table (clutData, 33 entries of RGB) makes of each 8-bit
/// value of red, green and blue: between two of its entries, as the card
/// interpolates (MAME's `screen_update`).
pub fn gamma_table(clut: &[u32; 33]) -> [[u8; 3]; 256] {
    let mut clut = *clut;
    // Some programs write 0 to the last entry and mean white.
    if clut[32] & 0xFF_FFFF == 0 && clut[31] & 0xFF_FFFF != 0 {
        clut[32] = 0x20FF_FFFF;
    }
    let comp = |y: usize, shift: u32| -> u8 {
        let lo = (clut[y >> 3] >> shift) & 0xFF;
        let hi = (clut[(y >> 3) + 1] >> shift) & 0xFF;
        ((lo * (8 - (y as u32 & 7)) + hi * (y as u32 & 7)) >> 3) as u8
    };
    std::array::from_fn(|y| [comp(y, 16), comp(y, 8), comp(y, 0)])
}

/// The gamma table at power-on: 5-bit values scaled to 8 bits, white at
/// the end.
fn default_clut() -> [u32; 33] {
    let mut clut = [0u32; 33];
    for (i, c) in clut.iter_mut().enumerate().take(32) {
        let v = ((i << 3) | (i >> 2)) as u32;
        *c = (i as u32) << 24 | v << 16 | v << 8 | v;
    }
    clut[32] = 0x20FF_FFFF;
    clut
}

/// The section a save state keeps the card in: its memory first, where
/// it stays in place for rewind's deltas.
impl State for Voodoo {
    fn save(&self, w: &mut Writer) {
        // Everything drawn before its memory is saved.
        self.catch_up();
        self.board.save(w);
        self.fbi.ram.save(w);
        for tmu in &self.tmu {
            tmu.ram.save(w);
        }
        self.reg[..].iter().for_each(|r| r.save(w));
        self.pci.save(w);
        self.dac.save(w);
        self.dac_read.save(w);
        self.fbi.save(w);
        for tmu in &self.tmu {
            tmu.save(w);
        }
        self.send_config.save(w);
        self.counted().save(w);
        self.clut.save(w);
        self.swaps.save(w);
        self.last_swap.save(w);
        self.fifo_writes.save(w);
    }

    fn load(&mut self, r: &mut Reader) -> Result<()> {
        self.flush();
        let mut board = Board::default();
        board.load(r)?;
        if board != self.board {
            return Err(StateError::Mismatch(format!(
                "its 3dfx card has {} MB, this one {} MB",
                board.megabytes(),
                self.board.megabytes()
            )));
        }
        self.fbi.ram.load(r)?;
        for tmu in &mut self.tmu {
            tmu.ram.load(r)?;
        }
        for reg in self.reg.iter_mut() {
            reg.load(r)?;
        }
        self.pci.load(r)?;
        self.dac.load(r)?;
        self.dac_read.load(r)?;
        self.fbi.load(r)?;
        for tmu in &mut self.tmu {
            tmu.load(r)?;
        }
        self.send_config.load(r)?;
        self.stats.load(r)?;
        self.pool.reset_stats();
        self.clut.load(r)?;
        self.swaps.load(r)?;
        self.last_swap.load(r)?;
        self.fifo_writes.load(r)?;
        self.after_load();
        Ok(())
    }
}
