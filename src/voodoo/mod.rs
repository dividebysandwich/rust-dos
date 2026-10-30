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
//!
//! `voodoo_model=voodoo2` makes it a Voodoo2 (device 0002h, 8 or 12 MB),
//! which DOSBox-X doesn't have. Its parts come from the Voodoo2 Graphics
//! specification (revision 1.16), 3dfx's Glide 3 sources for it (cvg),
//! 86Box's Voodoo2 and MAME's voodoo_2 device: the command FIFO
//! (`cmdfifo`), the triangle setup engine (`tsu`), the 2D blitter
//! (`blit`), and the constants where the two chips differ (`chip`).

pub mod backlog;
pub mod banshee;
pub mod blit;
pub mod chip;
pub mod cmdfifo;
#[cfg(feature = "voodoo-gl")]
pub mod gl;
pub mod gpu;
pub mod lfb;
pub mod mem;
pub mod mirror;
pub mod overlay;
pub mod raster;
pub mod register;
pub mod regs;
pub mod setup;
pub mod tables;
pub mod tracker;
pub mod texture;
pub mod tsu;
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
use backlog::{Access, Backlog, Footprint, TileLayout};
use workers::{Job, Word};

/// The size of the card's memory window.
pub const WINDOW: u32 = 16 << 20;
/// Where the BIOS puts it (DOSBox-X's `VOODOO_INITIAL_LFB`).
pub const INITIAL_BASE: u32 = 0xD000_0000;

/// The chip a board carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Model {
    /// The Voodoo Graphics (SST-1).
    #[default]
    Voodoo1,
    /// The Voodoo2 (CVG): its command FIFO, triangle setup engine, 2D
    /// blitter and the rest it added.
    Voodoo2,
    /// The Banshee and Voodoo3 (`banshee`): the Voodoo2's 3D engine on a
    /// display adapter.
    Banshee,
}

impl Model {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "voodoo1" | "voodoo" | "1" => Some(Model::Voodoo1),
            "voodoo2" | "2" => Some(Model::Voodoo2),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Model::Voodoo1 => "voodoo1",
            Model::Voodoo2 => "voodoo2",
            Model::Banshee => "banshee",
        }
    }
}

/// The boards: a Voodoo Graphics with 4 or 12 MB, a Voodoo2 with 8 or 12.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Board {
    /// 2 MB frame buffer, one texture unit with 2 MB (4 MB in all).
    Standard,
    /// 4 MB frame buffer, two texture units with 4 MB each (12 MB).
    #[default]
    Max,
    /// A Voodoo2: 4 MB frame buffer, two texture units with 2 MB each (8 MB).
    Voodoo2,
    /// A Voodoo2: 4 MB frame buffer, two texture units with 4 MB each (12 MB).
    Voodoo2Max,
    /// A Voodoo Banshee with 16 MB, the display adapter
    /// (`machine=svga_banshee`), not an add-on card.
    Banshee,
    /// A Voodoo3 3000 with 16 MB, the display adapter
    /// (`machine=svga_voodoo3`).
    Voodoo3,
}

crate::state_enum!(Board {
    Board::Standard,
    Board::Max,
    Board::Voodoo2,
    Board::Voodoo2Max,
    Board::Banshee,
    Board::Voodoo3,
});

impl Board {
    /// The boards in the order saves and recordings number them.
    pub const ALL: [Board; 4] = [Board::Standard, Board::Max, Board::Voodoo2, Board::Voodoo2Max];

    /// The board `voodoo_memory` names ("4", "8" or "12"), before
    /// `voodoo_model` picks the chip (`of`).
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "4" | "4mb" => Some(Board::Standard),
            "8" | "8mb" => Some(Board::Voodoo2),
            "12" | "12mb" => Some(Board::Max),
            _ => None,
        }
    }

    /// How the settings menu names it.
    pub fn describe(self) -> &'static str {
        match self {
            Board::Standard => "Voodoo Graphics, 4 MB (one texture unit)",
            Board::Max => "Voodoo Graphics, 12 MB (two texture units)",
            Board::Voodoo2 => "Voodoo2, 8 MB",
            Board::Voodoo2Max => "Voodoo2, 12 MB",
            Board::Banshee => "Voodoo Banshee, 16 MB",
            Board::Voodoo3 => "Voodoo3 3000, 16 MB",
        }
    }

    /// The board of `model` with `megabytes` of memory: the nearest it
    /// comes in (a Voodoo Graphics 4 or 12 MB, a Voodoo2 8 or 12).
    pub fn of(model: Model, megabytes: u32) -> Self {
        match (model, megabytes) {
            (Model::Voodoo1, ..=4) => Board::Standard,
            (Model::Voodoo1, _) => Board::Max,
            (Model::Voodoo2, ..=8) => Board::Voodoo2,
            (Model::Voodoo2, _) => Board::Voodoo2Max,
            (Model::Banshee, _) => Board::Voodoo3,
        }
    }

    pub fn model(self) -> Model {
        match self {
            Board::Standard | Board::Max => Model::Voodoo1,
            Board::Voodoo2 | Board::Voodoo2Max => Model::Voodoo2,
            Board::Banshee | Board::Voodoo3 => Model::Banshee,
        }
    }

    /// Which display adapter it is, if it is one.
    pub fn banshee(self) -> Option<banshee::Kind> {
        match self {
            Board::Banshee => Some(banshee::Kind::Banshee),
            Board::Voodoo3 => Some(banshee::Kind::Voodoo3),
            _ => None,
        }
    }

    /// Its number in `ALL`.
    pub fn index(self) -> u32 {
        Self::ALL.iter().position(|&b| b == self).unwrap_or(0) as u32
    }

    /// The memory in MB, as `voodoo_memory` names it.
    pub fn megabytes(self) -> u32 {
        match self {
            Board::Standard => 4,
            Board::Max | Board::Voodoo2Max => 12,
            Board::Voodoo2 => 8,
            Board::Banshee | Board::Voodoo3 => 16,
        }
    }

    fn frame_buffer(self) -> usize {
        match self {
            Board::Standard => 2 << 20,
            Board::Banshee | Board::Voodoo3 => banshee::MEMORY,
            _ => 4 << 20,
        }
    }

    fn texture_units(self) -> usize {
        match self {
            Board::Standard | Board::Banshee => 1,
            _ => 2,
        }
    }

    fn texture_memory(self) -> usize {
        match self {
            Board::Standard | Board::Voodoo2 => 2 << 20,
            Board::Max | Board::Voodoo2Max => 4 << 20,
            Board::Banshee | Board::Voodoo3 => banshee::MEMORY,
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

/// Whether textures use the game's filters or always fetch one texel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TextureSampling {
    #[default]
    Default,
    Unfiltered,
}

impl TextureSampling {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "default" => Some(Self::Default),
            "unfiltered" => Some(Self::Unfiltered),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Unfiltered => "unfiltered",
        }
    }
}

/// The `voodoo` settings: whether there is a card, which board, and how
/// the host draws for it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoodooSettings {
    pub enabled: bool,
    pub board: Board,
    pub renderer: Renderer,
    pub texture_sampling: TextureSampling,
    /// How many times the native size OpenGL draws at.
    pub scale: u32,
    /// Whether the CRT look's lines follow `scale`, so they stay as thin
    /// as the card's own lines.
    pub scale_shader: bool,
    /// The samples a pixel OpenGL's multisampling takes, 1 for none.
    pub msaa: u32,
    /// The texture anisotropy OpenGL uses, 1 for none.
    pub anisotropy: u32,
    /// The most frames a second programs may show on the card, if capped.
    pub fps_cap: Option<u32>,
    /// Glide's gamma factor for the guest's SST_*GAMMA variables, if set.
    pub gamma: Option<f32>,
    /// Whether OpenGL dithers to 5-6-5 as the card does where a game
    /// turns dithering on (`voodoo_dither`), or keeps 8 bits a colour.
    pub dither: bool,
    /// Whether reads of the buffers get OpenGL's pictures of them
    /// (`voodoo_readback=gpu`), or what the software renderer draws from
    /// the same commands (`exact`).
    pub gpu_reads: bool,
}

impl Default for VoodooSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            board: Board::Max,
            renderer: Renderer::Software,
            texture_sampling: TextureSampling::Default,
            scale: 2,
            scale_shader: true,
            msaa: 1,
            anisotropy: 1,
            fps_cap: None,
            gamma: None,
            dither: true,
            gpu_reads: true,
        }
    }
}

impl VoodooSettings {
    /// The card the machine has, if any.
    pub fn board(&self) -> Option<Board> {
        self.enabled.then_some(self.board)
    }

    /// The guest variables Glide reads its gamma from, empty without the card.
    pub fn environment(&self) -> Vec<crate::env_inject::Rule> {
        let gamma = self.gamma.filter(|_| self.enabled).map(|g| g.to_string());
        ["SST_RGAMMA", "SST_GGAMMA", "SST_BGAMMA"].into_iter().map(|name| (name, gamma.clone())).collect()
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
    /// A swap waits for its turn under the frame rate cap: the program
    /// waits until PIT tick `hold_to`, interrupts going on (`Bus::hold`).
    pub hold_to: Option<u64>,
    /// A Banshee's video registers changed: the display's mode and timing
    /// need working out again.
    pub settle: bool,
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
    /// How the rows of the colour buffers and of the auxiliary buffer lie
    /// in memory (a Banshee's own, or `rowpixels` apart), and the bits of
    /// a row counted from the Y origin.
    pub stride: raster::Stride,
    pub aux_stride: raster::Stride,
    pub ymask: i32,
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
} skip { mask, rgboffs, auxoffs, yorigin, rowpixels, stride, aux_stride, ymask, fifo_size });

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
            stride: raster::Stride::linear(640),
            aux_stride: raster::Stride::linear(640),
            ymask: 0x3FF,
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
    /// What the board's chip is: its register map, field widths, IDs
    /// and power-on values.
    pub(crate) chip: &'static chip::Chip,
    /// A host rendering preference, not a guest register or saved state.
    texture_sampling: TextureSampling,
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
    /// The most frames a second the program may show (`voodoo_fps_cap`),
    /// the time of the last swap it let through, in ns, and a swap
    /// waiting for its turn: its swapbufferCMD and when it is due, in PIT
    /// ticks.
    fps_cap: Option<u32>,
    capped_at: u64,
    deferred: Option<(u32, u64)>,
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
    /// The memory the jobs given out since then, and the frame buffer
    /// writes waiting to be, may read or write.
    footprint: RefCell<Footprint>,
    /// Frame buffer writes behind queued jobs, not yet a job themselves.
    words: RefCell<Vec<Word>>,
    /// Some of `words` came while jobs were held back: they aren't in the
    /// footprint, and the backlog hasn't seen them (`spare`).
    words_unseen: Cell<bool>,
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
    mirror: RefCell<Option<Box<mirror::Mirror>>>,
    /// With `voodoo_readback=gpu`: whether the renderer's pictures of the
    /// buffers are what reads of them get (`gpu_reads`), whether the
    /// memory is behind them (jobs since were left to the renderer
    /// alone; which bytes are, the tracker in `gpu` keeps), whether the
    /// program used alpha planes, which the renderer keeps where they
    /// can't be read back, and the read backs and the jobs left to the
    /// renderer, for the debugger.
    gpu_reads: bool,
    gpu_behind: Cell<bool>,
    gpu_alpha: Cell<bool>,
    gpu_read_backs: Cell<u64>,
    gpu_only: Cell<u64>,
    /// What the renderer has of the card's memory (`tracker`): which
    /// pages of a Banshee's memory it lacks (`banshee_scanout`), and which
    /// bytes only it has, in its copy of the memory or in its 3D buffers.
    gpu: RefCell<gpu::Gpu>,
    /// A Banshee's picture changed, which the software picture took
    /// first (`prepare_display`), so the renderer's scanout still has to
    /// hear of it.
    scanout_dirty: bool,
    /// Messages already logged once.
    logged: u32,
    /// Where the card's accesses are recorded (`RUST_DOS_VOODOO_RECORD`),
    /// for `tests/voodoo_tests.rs` to replay and compare.
    recorder: std::cell::RefCell<Option<std::io::BufWriter<std::fs::File>>>,
    /// Lines for the log, which the bus writes out.
    pub log: Vec<String>,
    /// A Voodoo2's command FIFO, the setup engine's vertices (the last
    /// three, and how many since the strip or fan began) and its
    /// blitter's CPU-to-screen copy.
    pub(crate) cmdfifo: cmdfifo::CmdFifo,
    pub(crate) svert: [tsu::Vertex; 3],
    pub(crate) sverts: u32,
    pub(crate) blt: blit::Blit,
    /// `blt_queue_dest`'s answer for the copy under way, and what it was
    /// worked out from (`blt_queue_key`): a copy sends its data a word at
    /// a time, tens of thousands a frame.
    blt_queue: Cell<Option<([u32; 15], Option<usize>)>>,
    /// The Banshee's or Voodoo3's own state when the card is one: the
    /// display adapter around the 3D engine, whose memory it shares.
    pub banshee: Option<Box<banshee::Banshee>>,
}

/// Frame buffer writes that go out to the workers together.
const WORDS_A_JOB: usize = 4096;

/// One-time log messages.
const LOG_BYTE: u32 = 1;
const LOG_RESERVED: u32 = 2;
const LOG_DIRECT: u32 = 4;
pub(crate) const LOG_IGNORED: u32 = 8;

impl Voodoo {
    pub fn new(board: Board) -> Self {
        Self::with_workers(board, workers::Pool::default_workers())
    }

    /// A card that draws with `workers` threads (0: none).
    pub fn with_workers(board: Board, workers: usize) -> Self {
        let units = board.texture_units();
        let chip = chip::Chip::of(board.model());
        let mut v = Self {
            board,
            chip,
            texture_sampling: TextureSampling::Default,
            chipmask: if units == 2 { 0x07 } else { 0x03 },
            tmu_config: chip.tmu_config[units - 1],
            reg: Box::new([0; 0x400]),
            pci: PciConfig::default(),
            dac: [0; 8],
            dac_read: 0,
            fbi: Fbi::new(board.frame_buffer()),
            tmu: Vec::new(),
            send_config: false,
            stats: Stats::default(),
            clut: default_clut(),
            swaps: VecDeque::new(),
            last_swap: 0,
            fifo_writes: 0,
            fps_cap: None,
            capped_at: 0,
            deferred: None,
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
            footprint: RefCell::new(Footprint::default()),
            words: RefCell::new(Vec::new()),
            words_unseen: Cell::new(false),
            backlog: RefCell::new(Backlog::default()),
            prune: std::env::var("RUST_DOS_VOODOO_PRUNE").map_or(true, |v| v != "0"),
            counters_read: Cell::new(false),
            catch_ups: Cell::new(0),
            software_picture: true,
            mirror: RefCell::new(None),
            gpu_reads: false,
            gpu_behind: Cell::new(false),
            gpu_alpha: Cell::new(false),
            gpu_read_backs: Cell::new(0),
            gpu_only: Cell::new(0),
            gpu: RefCell::new(gpu::Gpu::new(board.frame_buffer())),
            scanout_dirty: false,
            logged: 0,
            recorder: std::cell::RefCell::new(None),
            log: Vec::new(),
            cmdfifo: Default::default(),
            svert: Default::default(),
            sverts: 0,
            blt: Default::default(),
            blt_queue: Cell::new(None),
            banshee: board.banshee().map(|kind| Box::new(banshee::Banshee::new(kind))),
        };
        v.tmu = (0..units)
            .map(|i| {
                let base = 0x100 * (i + 1);
                if v.banshee.is_some() {
                    // One memory for everything, a Voodoo2's texture
                    // formats, and base addresses in bytes.
                    let mut tmu = Tmu::with_ram(v.fbi.ram.clone(), base);
                    tmu.v2 = true;
                    tmu.byte_base = true;
                    return tmu;
                }
                let mut tmu = Tmu::new(board.texture_memory(), base);
                tmu.v2 = chip.v2();
                tmu
            })
            .collect();
        v.reset();
        v
    }

    /// Change sampling for subsequent draws without altering guest registers.
    pub fn set_texture_sampling(&mut self, sampling: TextureSampling) {
        self.texture_sampling = sampling;
    }

    /// A PCI reset: the power-on registers, the monitor given back to the
    /// VGA, nothing pending. The memory keeps what it had.
    pub fn reset(&mut self) {
        self.flush();
        self.pool.reset_stats();
        self.counters_read.set(false);
        self.gpu_alpha.set(false);
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
        self.capped_at = 0;
        self.deferred = None;
        self.fbi.frontbuf = 0;
        self.fbi.backbuf = 1;
        self.fbi.width = 640;
        self.fbi.height = 480;
        self.fbi.fogblend = [0; 64];
        self.fbi.fogdelta = [0; 64];
        for tmu in &mut self.tmu {
            tmu.reset();
        }
        let init = [FBI_INIT0, FBI_INIT1, FBI_INIT2, FBI_INIT3, FBI_INIT4, FBI_INIT5, FBI_INIT6, FBI_INIT7];
        // A Banshee has none: its buffers are where its 3D registers say.
        let values = if self.banshee.is_some() { [0; 8] } else { self.chip.fbi_init };
        for (reg, value) in init.into_iter().zip(values) {
            // fbiInit5-7 are a Voodoo2's, 0 on a Voodoo Graphics.
            self.reg[reg] = value;
        }
        self.cmdfifo = Default::default();
        self.svert = Default::default();
        self.sverts = 0;
        self.blt = Default::default();
        self.fbi.yorigin = 0;
        self.recompute_video_memory();
        self.timing = CrtTiming::VESA_480;
        self.palette_dirty = true;
        self.display_dirty = true;
        if let Some(mirror) = self.mirror.get_mut() {
            mirror.forget_textures();
        }
        if let Some(b) = &mut self.banshee {
            b.reset();
        }
    }

    // --- PCI ---

    /// A byte of the configuration space.
    pub fn config_read(&self, reg: u8) -> u8 {
        if let Some(b) = &self.banshee {
            return b.config_read(reg);
        }
        let dword = match reg & 0xFC {
            0x00 => (self.chip.device_id as u32) << 16 | 0x121A,
            0x04 => 0x0080_0000 | self.pci.command as u32,
            // Revision 2, a multimedia video device.
            0x08 => 0x0400_0002,
            0x10 => self.pci.bar | 0x08,
            0x3C => 0x0000_00FF,
            // A Voodoo2 reads 5 in bits 15:12 (86Box).
            0x40 if self.chip.v2() => (self.pci.init_enable & 0xFFFF_0FFF) | 0x5000,
            0x40 => self.pci.init_enable,
            _ => 0,
        };
        (dword >> (8 * (reg & 3))) as u8
    }

    /// A byte of the configuration space written. True if the card's
    /// window or its video clock changed.
    pub fn config_write(&mut self, reg: u8, value: u8) -> bool {
        self.record(b'P', [reg as u32, value as u32, 0]);
        if let Some(b) = &mut self.banshee {
            return b.config_write(reg, value);
        }
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

    /// Whether the card is a Voodoo2.
    pub(crate) fn v2(&self) -> bool {
        self.chip.v2()
    }

    /// Whether writes to the fbiInit registers are allowed (initEnable
    /// bit 0).
    fn init_writes(&self) -> bool {
        self.pci.init_enable & 1 != 0
    }

    // --- The memory window ---

    /// A dword of the window at byte `offset`, 4-aligned (`voodoo_r`).
    pub fn read(&self, offset: u32, now: Now) -> u32 {
        if self.banshee.is_some() {
            return self.banshee_read(offset & !3, now);
        }
        let index = (offset >> 2) & 0x3F_FFFF;
        if index & (0xC0_0000 / 4) == 0 {
            self.register_read(index, now)
        } else if index & (0x80_0000 / 4) == 0 {
            let value = self.lfb_read(index);
            self.record(b'R', [index, value, 0]);
            value
        } else {
            0xFFFF_FFFF
        }
    }

    /// Record an access as `tests/voodoo_tests.rs` replays it: a kind and
    /// three words, little-endian.
    fn record(&self, kind: u8, words: [u32; 3]) {
        use std::io::Write;
        let mut recorder = self.recorder.borrow_mut();
        let Some(out) = recorder.as_mut() else { return };
        let mut bytes = [kind; 13];
        for (i, w) in words.iter().enumerate() {
            bytes[1 + 4 * i..5 + 4 * i].copy_from_slice(&w.to_le_bytes());
        }
        let _ = out.write_all(&bytes);
    }

    /// Start recording the card's accesses into `path`, beginning with the
    /// board.
    pub fn record_into(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        *self.recorder.borrow_mut() = Some(std::io::BufWriter::new(std::fs::File::create(path)?));
        self.record(b'B', [self.board.index(), 0, 0]);
        Ok(())
    }

    /// A dword of the window written, or the half `mask` selects
    /// (`voodoo_w`).
    pub fn write(&mut self, offset: u32, data: u32, mask: u32, now: Now) -> Effect {
        if self.banshee.is_some() {
            return self.banshee_write(offset & !3, data, mask, now);
        }
        let index = (offset >> 2) & 0x3F_FFFF;
        self.record(b'W', [index, data, mask]);
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
        if index == regs::SWAPBUFFER_CMD as u32 && self.recorder.borrow().is_some() {
            // What the frame buffer holds at each swap, to compare.
            let bytes = self.frame_buffer().to_bytes();
            let hash = bytes.iter().fold(2166136261u32, |h, &b| (h ^ b as u32).wrapping_mul(16777619));
            self.record(b'S', [0, hash, 0]);
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

    /// The frame buffer's memory, with everything drawn into it, the
    /// GPU's 2D operations too.
    pub fn frame_buffer(&self) -> &Vram {
        self.catch_up();
        self.gpu_sync_all();
        &self.fbi.ram
    }

    /// Wait until the workers drew everything given to them.
    pub fn flush(&mut self) {
        self.catch_up();
        self.drawn_to.clear();
    }

    /// The same where what comes reads only `ranges` of the memory's
    /// bytes (`catch_up_in`).
    pub(crate) fn flush_in(&mut self, ranges: &[(u32, u32)]) {
        for &range in ranges {
            self.gpu_catch_up_in(Some(range));
        }
        self.catch_up_workers();
        self.drawn_to.clear();
    }

    /// Draw everything queued, and wait for it.
    pub(crate) fn catch_up(&self) {
        self.catch_up_in(None);
    }

    /// The same where what comes reads only bytes `start..end` of the
    /// memory (with None any of it): of the buffers behind the OpenGL
    /// renderer (`gpu_behind`), only the rows there are read back.
    pub(crate) fn catch_up_in(&self, range: Option<(u32, u32)>) {
        self.gpu_catch_up_in(range);
        self.catch_up_workers();
    }

    /// Draw the jobs held back and queued in software, and wait for them.
    fn catch_up_workers(&self) {
        if !self.backlog.borrow().is_empty() {
            self.catch_ups.set(self.catch_ups.get() + 1);
        }
        self.submit_words();
        self.release_backlog();
        self.pool.flush();
        self.outstanding.set(false);
        self.tmu_in_use.set(0);
        self.footprint.borrow_mut().clear();
    }

    /// Whether the workers are still drawing, with everything queued
    /// given to them first but without waiting for them: a Banshee's busy
    /// bits, which a program polls until they clear. Jobs held back
    /// (`backlog`) stay held: as far as the program can tell they are
    /// drawn, since whatever reads what they draw catches up first.
    ///
    /// In deterministic mode the workers' progress would show the
    /// program the host's speed, and a polling loop would run a different
    /// number of times on another host. The card reads idle there, as if
    /// it drew at once, the same way as held jobs: the workers go on, and
    /// what reads what they draw still waits for them.
    pub(crate) fn drawing(&self) -> bool {
        if !self.writes_queue() {
            return false;
        }
        if !self.deferring() {
            self.submit_words();
            self.release_backlog();
        }
        self.pool.give_out();
        if crate::deterministic::active() {
            return false;
        }
        let busy = self.pool.busy();
        if !busy && self.words.borrow().is_empty() {
            // The jobs given out are drawn; those held keep their own
            // footprint.
            self.footprint.borrow_mut().clear();
            if self.backlog.borrow().is_empty() {
                self.outstanding.set(false);
                self.tmu_in_use.set(0);
            }
        }
        busy
    }

    /// Whether jobs are held back (`backlog`): while the OpenGL renderer
    /// draws, unless a program reads the pixel counters or the buffers'
    /// layout gives the tiles no meaning.
    fn deferring(&self) -> bool {
        self.recording()
            && self.prune
            && !self.counters_read.get()
            && self.backlog.borrow().layout.prunable
    }

    /// What a job does to the tiles, when jobs are held back.
    pub(crate) fn access(&self, f: impl FnOnce(&TileLayout) -> Access) -> Option<Access> {
        self.deferring().then(|| f(&self.backlog.borrow().layout))
    }

    /// Queue `job` after the frame buffer writes before it: held back
    /// with its `access`, or given to the workers.
    pub(crate) fn submit(&self, job: Job, access: Option<Access>) {
        if access.as_ref().is_some_and(|a| self.gpu_takes(&job, a)) {
            self.gpu_left(&job);
            return;
        }
        // Drawn here, over the memory as the renderer has it.
        self.gpu_catch_up();
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
        // Without workers a job is drawn as it is given: it reaches
        // nothing after.
        let workers = self.pool.workers() > 0;
        for job in jobs {
            if workers {
                self.footprint.borrow_mut().add(&job);
            }
            self.outstanding.set(workers);
            self.pool.submit(job);
        }
    }

    /// The jobs held back, pruned, to the workers without waiting.
    fn release_backlog(&self) {
        if self.backlog.borrow().is_empty() {
            return;
        }
        let (held, reach) = self.backlog.borrow_mut().take();
        let workers = self.pool.workers() > 0;
        if workers {
            self.footprint.borrow_mut().merge(&reach);
        }
        for job in held {
            self.outstanding.set(workers);
            self.pool.submit(job);
        }
    }

    /// The frame buffer writes queued behind jobs, as a job of their own:
    /// held back after the jobs held, or after them to the workers. Those
    /// queued while jobs were held back aren't in the footprint yet
    /// (`queue_words`): handed over, they go into it.
    fn submit_words(&self) {
        let words = std::mem::take(&mut *self.words.borrow_mut());
        self.words_unseen.set(false);
        if words.is_empty() {
            return;
        }
        if self.deferring() {
            let out = self.backlog.borrow_mut().push_words(&self.fbi.ram, &words);
            self.hand_over(out);
        } else {
            self.hand_over([Job::Pixels {
                fb: self.fbi.ram.clone(),
                words,
            }]);
        }
    }

    /// Make bytes `start..end` of the frame buffer's memory free to be
    /// written before the jobs given out or held back are drawn, where
    /// that can be done: the jobs held back that take texels from there
    /// get a copy of them as they are (`Backlog::spare`). False if the
    /// jobs have to be drawn first.
    pub(crate) fn spare(&self, start: u32, end: u32) -> bool {
        if !self.writes_queue() {
            return true;
        }
        // Writes waiting from while jobs were held back may land in that
        // range: the backlog has to see them, as the write comes after
        // them. The others are in the footprint already, and stay in a
        // batch.
        if self.words_unseen.get() {
            self.submit_words();
        }
        let given_out = self.footprint.borrow();
        let range = (start as u64, end as u64);
        self.backlog
            .borrow_mut()
            .spare(range, &given_out, |a, b| self.gpu_sync_quiet(a as u32, b as u32))
    }

    /// Whether frame buffer writes have to wait their turn behind jobs.
    pub(crate) fn writes_queue(&self) -> bool {
        self.outstanding.get()
            || !self.words.borrow().is_empty()
            || !self.backlog.borrow().is_empty()
            || self.gpu_behind.get()
    }

    /// Frame buffer writes for the queue (`writes_queue`): held back with
    /// the jobs, or every so many out as a job, or straight into memory
    /// when the workers are done by then. Held back, they wait here until
    /// there are enough of them or something else comes: the backlog
    /// works out what a batch of them touches at once, which per write
    /// would cost more than drawing the jobs held (a Voodoo2 CPU-to-screen
    /// copy is tens of thousands of writes a frame).
    pub(crate) fn queue_words(&self, new: &[Word]) {
        // Behind the renderer, with nothing else waiting: the renderer
        // gets them from the recording, and the memory has them at once,
        // which a read back of the buffers writes over with the same.
        if self.gpu_behind.get()
            && !self.outstanding.get()
            && self.words.borrow().is_empty()
            && self.backlog.borrow().is_empty()
        {
            let ram = &self.fbi.ram;
            for w in new {
                if (w.at as usize) < ram.len() {
                    ram.set(w.at as usize, w.value);
                }
            }
            return;
        }
        if self.deferring() {
            let mut words = self.words.borrow_mut();
            words.extend_from_slice(new);
            self.words_unseen.set(true);
            if words.len() >= WORDS_A_JOB {
                drop(words);
                self.submit_words();
            }
            return;
        }
        self.footprint.borrow_mut().words(new);
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
            self.words_unseen.set(false);
            self.outstanding.set(false);
            self.tmu_in_use.set(0);
            self.footprint.borrow_mut().clear();
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
        // Queued triangles read the texture memory as they draw: they
        // keep it as it is, and the write goes into a copy, which no job
        // has yet; or they are drawn first. A Banshee's is the frame
        // buffer, which they draw in.
        if self.tmu_in_use.get() & (1 << unit) != 0 {
            let shared = self.tmu[unit].ram.same(&self.fbi.ram);
            // SAFETY: the texture memory is written only here, on the
            // emulator's thread; the workers only read it.
            if !shared && unsafe { self.tmu[unit].detach() } {
                self.tmu_in_use.set(self.tmu_in_use.get() & !(1 << unit));
            } else {
                self.flush();
            }
        }
        if self.reg[self.tmu[unit].base + T_LOD] & (1 << 27) != 0 {
            self.log_once(LOG_DIRECT, "[3DFX] Direct texture writes are ignored");
            return;
        }
        let (reg, tmu) = (&self.reg[..], &mut self.tmu[unit]);
        tmu.write(reg, index, data);
    }

    // --- Video memory layout and output ---

    /// A Voodoo Graphics' buffers: two colour buffers and an auxiliary
    /// one, or three colour buffers (fbiInit2 bit 4), rows of 64-pixel
    /// tiles (fbiInit1 bits 7:4).
    fn layout_v1(&self) -> (u32, u32) {
        ((self.reg[FBI_INIT2] >> 4) & 1, 64 * ((self.reg[FBI_INIT1] >> 4) & 0xF))
    }

    /// A Voodoo2's: fbiInit5 bits 10:9 can also ask for three colour
    /// buffers and an auxiliary one, and its tiles are 32 pixels wide, with
    /// two more bits of their count in fbiInit1 bit 24 and fbiInit6 bit 30
    /// (MAME's `voodoo_2_device::recompute_video_memory`).
    fn layout_v2(&self) -> (u32, u32) {
        let config = if self.reg[FBI_INIT2] & (1 << 4) != 0 { 1 } else { (self.reg[FBI_INIT5] >> 9) & 3 };
        let init1 = self.reg[FBI_INIT1];
        let xtiles = (self.reg[FBI_INIT6] >> 30) & 1 | ((init1 >> 4) & 0xF) << 1 | ((init1 >> 24) & 1) << 5;
        (config, 32 * xtiles)
    }

    /// Where the buffers are, from the fbiInit registers (`recompute_video_memory`).
    pub(crate) fn recompute_video_memory(&mut self) {
        if self.banshee.is_some() {
            self.banshee_buffers();
            return;
        }
        let (config, rowpixels) = if self.v2() { self.layout_v2() } else { self.layout_v1() };
        let buffer_pages = (self.reg[FBI_INIT2] >> 11) & 0x1FF;
        let fifo_start = (self.reg[FBI_INIT4] >> 8) & 0x3FF;
        let mut fifo_last = (self.reg[FBI_INIT4] >> 18) & 0x3FF;
        let fbi = &mut self.fbi;
        fbi.rowpixels = rowpixels;
        fbi.stride = raster::Stride::linear(rowpixels as usize);
        fbi.aux_stride = fbi.stride;
        fbi.rgboffs[0] = 0;
        fbi.rgboffs[1] = buffer_pages * 0x1000;
        match config {
            // Three colour buffers and none.
            1 => {
                fbi.rgboffs[2] = 2 * buffer_pages * 0x1000;
                fbi.auxoffs = NONE;
            }
            // Three colour buffers and an auxiliary buffer.
            2 => {
                fbi.rgboffs[2] = 2 * buffer_pages * 0x1000;
                fbi.auxoffs = 3 * buffer_pages * 0x1000;
            }
            // Two colour buffers and an auxiliary buffer (3 is reserved).
            _ => {
                fbi.rgboffs[2] = NONE;
                fbi.auxoffs = 2 * buffer_pages * 0x1000;
            }
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
    /// A Banshee's are the buffers the recording keeps
    /// (`banshee_mirror_key`): the one colBufferAddr points at and those
    /// drawn into lately, which a program swaps between, so that the
    /// frames drawn into one stay held while it draws into another.
    /// Without a recording, both of its colour buffers are colBufferAddr's,
    /// and nothing is held.
    pub(crate) fn update_tiles(&mut self) {
        let fbi = &self.fbi;
        let word = |offs: u32| (offs != NONE).then_some(offs as usize / 2);
        let recorded = self
            .banshee
            .is_some()
            .then(|| self.mirror_layout())
            .flatten();
        let words = fbi.ram.len();
        let layout = match recorded {
            Some(key) => {
                let [a, b, c] = key.color.map(word);
                let bases = [a, b, c, word(key.aux)];
                let size = (key.width, key.height);
                TileLayout::new(bases, size, key.stride, key.aux_stride, words)
            }
            None => {
                let [a, b, c] = fbi.rgboffs.map(word);
                let bases = [a, b, c, word(fbi.auxoffs)];
                let size = (fbi.width, fbi.height);
                TileLayout::new(bases, size, fbi.stride, fbi.aux_stride, words)
            }
        };
        if layout != self.backlog.borrow().layout {
            self.release_backlog();
            self.backlog.borrow_mut().layout = layout;
        }
    }

    /// Whether the card drives the monitor: fbiInit0 turns off the VGA
    /// pass-through, and on a Voodoo Graphics its video clock runs.
    pub fn output(&self) -> bool {
        if let Some(b) = &self.banshee {
            return b.extended();
        }
        (self.pci.clock_enabled || !self.chip.pci_video_clock) && self.reg[FBI_INIT0] & 1 != 0
    }

    /// The size of the picture it shows, in the mode's own pixels.
    pub fn size(&self) -> (u32, u32) {
        if let Some(b) = &self.banshee {
            return b.desktop_size();
        }
        (self.fbi.width.max(1), self.fbi.height.max(1))
    }

    /// The size of the picture as the monitor shows it, which `render`
    /// draws: `size`, but a Banshee's or Voodoo3's narrow modes are
    /// twice as wide (`Banshee::picture_size`).
    pub fn picture_size(&self) -> (u32, u32) {
        match &self.banshee {
            Some(b) => b.picture_size(),
            None => self.size(),
        }
    }

    /// The refresh rate of its picture.
    pub fn refresh_hz(&self) -> f64 {
        self.timing.hz()
    }

    /// The video timing of its picture.
    pub fn timing(&self) -> CrtTiming {
        self.timing
    }

    /// A Banshee's video timing, which its VGA core's registers give.
    /// Retraces are counted from time 0 at the timing's rate, so the
    /// swaps' retrace numbers are counted again at the new one: a mode
    /// with a lower refresh rate would otherwise put the next swap as far
    /// ahead as the old rate had counted past the new one, minutes after
    /// a long session, and hold the command FIFOs up until then.
    pub fn set_timing(&mut self, timing: CrtTiming, now: Now) {
        let changed = timing != self.timing;
        let vint_pending = changed && self.banshee_vint_pending(now);
        self.timing = timing;
        if !changed || timing.frame_ns() == 0 {
            return;
        }
        let current = timing.retraces(now.ns);
        self.last_swap = self.last_swap.min(current);
        for s in &mut self.swaps {
            let ns = (s.due as u128 * 1_000_000_000 / PIT_HZ as u128) as u64;
            s.retrace = timing.retraces(ns);
        }
        // The armed retrace interrupt counts retraces the same way: it
        // stays pending, or waits for the next retrace at the new rate.
        if let Some(armed) = self.banshee.as_mut().and_then(|b| b.vint.as_mut()) {
            *armed = if vint_pending { current.saturating_sub(1) } else { current };
        }
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
            // The OpenGL renderer draws it: nothing needs the memory. The
            // change stays marked, and a Banshee's video processor's, for
            // the renderer (`banshee_scanout`) or the next picture that
            // does.
            self.display_dirty |= changed;
            return switched;
        }
        if let Some(b) = &mut self.banshee {
            let dirty = std::mem::take(&mut b.dirty);
            let draw = changed || switched || dirty;
            // The OpenGL renderer may show the picture all the same (a
            // recording or a screenshot wants it from memory too): its
            // scanout needs the change as well.
            if self.recording() {
                self.scanout_dirty |= draw;
            }
            // Jobs may still be drawing into what it shows. Those that draw
            // elsewhere, into the next frame, go on beside the program.
            if draw && self.writes_queue() {
                self.flush();
            }
            return draw;
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

    /// Draw the front buffer into `rgba` (RGBA8, `width` pixels a row,
    /// `picture_size` big), through the gamma table (see
    /// `prepare_display`).
    pub fn render(&self, rgba: &mut [u8], width: usize) {
        if let Some(b) = &self.banshee {
            // The picture is drawn from the memory, which needs the
            // GPU's 2D operations.
            self.gpu_sync_all();
            b.render(&self.fbi.ram, rgba, width);
            return;
        }
        let (w, h) = (self.fbi.width as usize, self.fbi.height as usize);
        let base = self.fbi.rgboffs[self.fbi.frontbuf as usize];
        if base == NONE {
            return;
        }
        let base = base as usize / 2;
        let ram = &self.fbi.ram;
        for y in 0..h {
            let Some(row) = rgba.get_mut(y * width * 4..(y * width + w.min(width)) * 4) else {
                break;
            };
            let src = base + y * self.fbi.rowpixels as usize;
            for (x, out) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let at = src + x;
                let pixel = if at < ram.len() { ram.get(at) } else { 0 };
                let [r, g, b] = self.palette[pixel as usize];
                *out = [r, g, b, 255];
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
        let video = self.chip.video;
        let (vsync_on, vsync_off) = chip::fields(v, video.vsync);
        let (xwidth, vvis) = chip::fields(dims, video.dimensions);
        let vtotal = vsync_off + vsync_on;
        if h == 0 || v == 0 || dims == 0 || vvis == 0 || vtotal <= vvis {
            return;
        }
        let line_ns = (1_000_000_000 / 60 / vtotal as u64) as u32;
        let htotal = self.htotal();
        let hvis = xwidth.min(htotal);
        let sync = vsync_on.clamp(1, vtotal - vvis);
        self.timing = CrtTiming {
            line_ns,
            hdisplay_ns: (line_ns as u64 * hvis as u64 / htotal.max(1) as u64) as u32,
            total: vtotal,
            display: vvis,
            retrace_start: vvis,
            retrace_end: vvis + sync,
        };
    }

    /// The pixels of a line, sync included, from hSync.
    fn htotal(&self) -> u32 {
        let (on, off) = chip::fields(self.reg[H_SYNC], self.chip.video.hsync);
        off + 1 + on + 1
    }

    /// hvRetrace: the line in bits 12:0 and the pixel in it in bits 26:16.
    pub(crate) fn hv_retrace(&self, now: Now) -> u32 {
        let line = self.timing.line_ns.max(1) as u64;
        let hpos = (now.ns % self.timing.frame_ns().max(1)) % line * self.htotal() as u64 / line;
        (self.scanline(now) & 0x1FFF) | (hpos as u32 & 0x7FF) << 16
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

    /// The frame rate cap (`voodoo_fps_cap`): None for none.
    pub fn set_fps_cap(&mut self, fps: Option<u32>) {
        self.fps_cap = fps.filter(|&fps| fps > 0);
    }

    /// The steady timing the frame rate cap shows frames at, if there is
    /// a cap: a retrace at every multiple of its frame time, which capped
    /// swaps happen at.
    pub fn cap_timing(&self) -> Option<CrtTiming> {
        let fps = self.fps_cap?;
        let frame = (1_000_000_000 / fps as u64) as u32;
        Some(CrtTiming { line_ns: frame, hdisplay_ns: frame, total: 1, display: 1, retrace_start: 0, retrace_end: 1 })
    }

    /// swapbufferCMD: the buffers swap now; with bit 0, the swap also
    /// stays pending until the retrace it waits for, `bits 8:1` retraces
    /// after the last swap and at least the next. Under a frame rate cap,
    /// a swap waits for the first of the cap's frame times a frame after
    /// the last, and the program waits with it (`Effect::hold_to`), so
    /// frames come at an even pace when it could draw them faster.
    pub(crate) fn swap(&mut self, data: u32, now: Now, effect: &mut Effect) {
        self.backlog.borrow_mut().swapped();
        if let Some(cap) = self.cap_timing() {
            let frame = cap.frame_ns();
            let earliest = if self.capped_at == 0 { now.ns } else { now.ns.max(self.capped_at + frame) };
            let at = earliest.div_ceil(frame) * frame;
            self.capped_at = at;
            let due = (at as u128 * PIT_HZ as u128).div_ceil(1_000_000_000) as u64;
            if due > now.ticks {
                self.deferred = Some((data, due));
                effect.hold_to = Some(due);
                effect.reschedule = true;
                return;
            }
        }
        self.swap_now(data, now, effect);
    }

    fn swap_now(&mut self, data: u32, now: Now, effect: &mut Effect) {
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

    /// When the last of the swaps pending at `ticks` is done, in PIT
    /// ticks: None if none is pending.
    pub fn swaps_done_at(&self, ticks: u64) -> Option<u64> {
        self.swaps.iter().map(|s| s.due).filter(|&due| due > ticks).max()
    }

    /// When the card next needs attention: its oldest pending swap's
    /// retrace, or the turn of a swap the frame rate cap holds back.
    pub fn next_event(&self) -> Option<u64> {
        let pending = self.swaps.front().map(|s| s.due);
        let deferred = self.deferred.map(|(_, due)| due);
        pending.into_iter().chain(deferred).min()
    }

    /// When the swap the frame rate cap holds back is due, if one is.
    pub fn deferred_swap(&self) -> Option<u64> {
        self.deferred.map(|(_, due)| due)
    }

    /// Make the swap the frame rate cap held back when its turn came, and
    /// retire the swaps whose retraces came.
    pub fn service(&mut self, now: Now) {
        self.service_into(now, &mut Effect::default());
    }

    /// `service`, with what the command FIFOs a swap held up ask for once
    /// they go on (a Banshee's next swap among the timer events) put in
    /// `effect`.
    fn service_into(&mut self, now: Now, effect: &mut Effect) {
        if let Some((data, due)) = self.deferred
            && due <= now.ticks
        {
            self.deferred = None;
            self.swap_now(data, now, &mut Effect::default());
        }
        let ticks = now.ticks;
        let mut retired = 0;
        while let Some(front) = self.swaps.front().copied() {
            if front.due > ticks {
                break;
            }
            self.swaps.pop_front();
            self.last_swap = front.retrace;
            self.fifo_writes = 0;
            retired += 1;
        }
        if self.banshee.is_some() {
            self.banshee_swaps_done(retired, now, effect);
        }
    }

    /// A write that goes through the FIFO: behind a pending swap it waits
    /// there, and a full FIFO stalls the CPU until the swap is done.
    fn fifo_write(&mut self, now: Now, effect: &mut Effect) {
        self.service_into(now, effect);
        let Some(front) = self.swaps.front() else { return };
        self.fifo_writes += 1;
        let capacity = if self.cmdfifo_on() {
            (self.cmdfifo.end.saturating_sub(self.cmdfifo.base) / 4).max(1)
        } else if self.fbi.fifo_size > 0 {
            self.fbi.fifo_size
        } else {
            64
        };
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
        self.fbi.yorigin = match &self.banshee {
            Some(b) => b.y_origin(),
            None => (self.reg[FBI_INIT3] >> 22) & 0x3FF,
        };
        let reg = self.reg.clone();
        for tmu in &mut self.tmu {
            tmu.rebuild(&reg[..]);
        }
        self.update_timing();
        self.palette_dirty = true;
        self.display_dirty = true;
        // The memory loaded is newer than anything on the GPU.
        self.gpu.get_mut().pages.mark_all();
        if let Some(mirror) = self.mirror.get_mut() {
            mirror.invalidate();
            mirror.forget_textures();
        }
    }

    // --- The OpenGL renderer's recording ---

    /// Record the drawing for the OpenGL renderer, or stop.
    pub fn set_mirror(&mut self, on: bool) {
        if on != self.recording() {
            // What only the renderer has comes back before it goes, and
            // the card has no link to it from then on.
            if !on {
                self.set_gpu_link(None);
            }
            *self.mirror.get_mut() = on.then(|| Box::new(mirror::Mirror::new()));
            self.release_backlog();
            self.update_tiles();
            // A renderer starting over has none of the picture.
            self.gpu.get_mut().pages.mark_all();
            self.display_dirty = true;
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
        !self.recording() || self.software_picture
    }

    pub fn mirror_attached(&self) -> bool {
        self.recording()
    }

    /// Whether the card records its drawing for the OpenGL renderer.
    pub(crate) fn recording(&self) -> bool {
        self.mirror.borrow().is_some()
    }

    /// The buffers' layout the recording has.
    pub(crate) fn mirror_layout(&self) -> Option<mirror::LayoutKey> {
        self.mirror.borrow().as_ref()?.layout().copied()
    }

    /// Take the buffers' pixels for the recording if their layout changed
    /// since it last did, or it wants them.
    pub(crate) fn mirror_sync(&mut self) {
        let Some(old) = self.mirror.get_mut().as_ref().map(|m| m.layout().copied()) else {
            return;
        };
        let key = self.mirror_key(old.as_ref());
        if !self
            .mirror
            .get_mut()
            .as_ref()
            .is_some_and(|m| m.needs_snapshot(&key))
        {
            return;
        }
        self.flush();
        let size = self.fbi.ram.bytes() as u64;
        let clamp = |(start, end): (u64, u64)| (start.min(size) as u32, end.min(size) as u32);
        // The 3D engine drew into the buffers before without marking the
        // pages it changed, which the renderer's copy of the memory may
        // now be read or drawn into at: it gets them again.
        for (start, end) in old.iter().flat_map(|old| old.spans()).map(clamp) {
            self.gpu.get_mut().pages.mark(start, end);
        }
        // The software renderer draws the buffers into the memory from
        // now on, where the GPU may have done a 2D operation.
        for (start, end) in key.spans().map(clamp) {
            self.gpu_sync(start, end);
        }
        let snapshot = self.snapshot(key.layout());
        if let Some(mirror) = self.mirror.get_mut() {
            mirror.resync(snapshot, key);
        }
        // A Banshee's tiles follow the recording's buffers.
        self.update_tiles();
    }

    /// The layout the drawing is in now, `old` the one recorded last.
    fn mirror_key(&self, old: Option<&mirror::LayoutKey>) -> mirror::LayoutKey {
        if self.banshee.is_some() {
            return self.banshee_mirror_key(old);
        }
        let fbi = &self.fbi;
        mirror::LayoutKey {
            width: fbi.width,
            height: fbi.height,
            stride: fbi.stride,
            aux_stride: fbi.aux_stride,
            color: fbi.rgboffs,
            aux: fbi.auxoffs,
        }
    }

    fn snapshot(&self, layout: mirror::Layout) -> mirror::Snapshot {
        let ram = &self.fbi.ram;
        let (width, height) = (layout.width as usize, layout.height as usize);
        let pixels = |base: u32, stride: raster::Stride| -> Vec<u16> {
            let mut out = Vec::with_capacity(width * height);
            for y in 0..height {
                let row = base as usize + stride.row(y);
                out.extend((0..width).map(|x| {
                    let at = row + stride.col(x);
                    if at < ram.len() { ram.get(at) } else { 0 }
                }));
            }
            out
        };
        mirror::Snapshot {
            color: layout
                .color
                .iter()
                .map(|&offs| pixels(offs, layout.stride))
                .collect(),
            aux: layout.aux.map(|offs| pixels(offs, layout.aux_stride)),
            layout,
        }
    }

    /// The drawing recorded since the last frame, and what the card shows.
    pub fn take_mirror(&mut self) -> Option<mirror::Frame> {
        self.mirror_sync();
        let key = self.mirror_layout()?;
        let front = match &self.banshee {
            Some(_) => self.banshee_mirror_front(&key),
            None => {
                let front = self.fbi.rgboffs[self.fbi.frontbuf as usize];
                (front != NONE).then_some(front / 2)
            }
        };
        let output = self.output();
        let scanout = if output && self.banshee.is_some() {
            self.banshee_scanout(&key)
        } else {
            None
        };
        let commands = self.mirror.get_mut().as_mut()?.take();
        Some(mirror::Frame {
            commands,
            front,
            output,
            width: key.width,
            height: key.height,
            clut: self.clut,
            scanout,
        })
    }

    /// Bytes `start..end` of the frame buffer were written other than
    /// through the 3D engine (the CPU through a Banshee's linear frame
    /// buffer, its 2D engine, a command FIFO): tell the recording the
    /// pixels of its buffers there, from memory, which has what the 3D
    /// engine drew before (the writers catch up first).
    pub(crate) fn mirror_memory(&mut self, start: u32, end: u32) {
        let Some(mirror) = self.mirror.get_mut() else {
            return;
        };
        // The renderer's copy of the memory gets the pages again.
        self.gpu.get_mut().pages.mark(start, end);
        // The texture units read the same memory: what the recording
        // decoded from there is out of date.
        for tmu in &mut self.tmu {
            tmu.note_written(start, end);
        }
        // With no layout yet, the next snapshot reads the memory.
        let Some(key) = mirror.layout().copied() else {
            return;
        };
        let ram = &self.fbi.ram;
        let word = |at: usize| if at < ram.len() { ram.get(at) } else { 0 };
        let by_word = |mirror: &mut mirror::Mirror| {
            for at in start / 2..end.div_ceil(2) {
                if let Some((dest, x, y)) = key.locate(at) {
                    mirror.pixel(dest, x, y, word(at as usize), key.width);
                }
            }
        };
        if end.saturating_sub(start) <= 16 {
            by_word(mirror);
            return;
        }
        // The rows the bytes may touch, or the words themselves if they
        // are fewer: a tiled buffer's rows come in bands of 32, which a
        // burst into one tile would push whole.
        let rows = key.rows_in(start, end);
        let pixels: u64 = rows
            .iter()
            .map(|&(_, first, last)| (last - first) as u64 * key.width as u64)
            .sum();
        if (end - start).div_ceil(2) as u64 <= pixels {
            by_word(mirror);
            return;
        }
        for (dest, first, last) in rows {
            let (base, stride) = match dest {
                Some(offs) => (offs, key.stride),
                None => (key.aux / 2, key.aux_stride),
            };
            for y in first..last {
                let row = base as usize + stride.row(y as usize);
                for x in 0..key.width {
                    let value = word(row + stride.col(x as usize));
                    mirror.pixel(dest, x, y, value, key.width);
                }
            }
        }
    }

    /// Pixel `x` of buffer row `y` written through the frame buffer: tell
    /// the recording its colour in buffer `dest` (a word offset) and its
    /// auxiliary buffer's value, those written.
    pub(crate) fn mirror_pixel(&mut self, dest: usize, x: i32, y: i32, color: Option<u16>, aux: Option<u16>) {
        let Some(mirror) = self.mirror.get_mut() else { return };
        let fbi = &self.fbi;
        // The recorded buffers' size: on a Banshee the 3D engine reaches
        // further down than they go (`banshee_buffers`).
        let (width, height) =
            mirror.layout().map_or((fbi.width, fbi.height), |k| (k.width, k.height));
        if x < 0 || y < 0 || x as u32 >= width || y as u32 >= height {
            return;
        }
        if let Some(value) = color {
            mirror.pixel(Some(dest as u32), x as u32, y as u32, value, width);
        }
        if let Some(value) = aux {
            mirror.pixel(None, x as u32, y as u32, value, width);
        }
    }

    /// The same for a write through the pixel pipeline, whose results are
    /// in memory: the colour, and with `aux` the auxiliary buffer's value.
    pub(crate) fn mirror_written(&mut self, dest: usize, x: i32, y: i32, aux: bool) {
        if !self.recording() || x < 0 || y < 0 {
            return;
        }
        let fbi = &self.fbi;
        let word = |base: usize, stride: raster::Stride| {
            let at = base + stride.row(y as usize) + stride.col(x as usize);
            (at < fbi.ram.len()).then(|| fbi.ram.get(at))
        };
        let color = word(dest, fbi.stride);
        let aux = if aux && fbi.auxoffs != NONE {
            word(fbi.auxoffs as usize / 2, fbi.aux_stride)
        } else {
            None
        };
        self.mirror_pixel(dest, x, y, color, aux);
    }

    /// For the debugger: what the card is doing.
    pub fn describe(&self, now: Now) -> serde_json::Value {
        let init: Vec<String> =
            [FBI_INIT0, FBI_INIT1, FBI_INIT2, FBI_INIT3, FBI_INIT4].iter().map(|&r| format!("{:08X}", self.reg[r])).collect();
        serde_json::json!({
            "model": self.board.model().name(),
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
            "opengl": self.recording(),
            "held_jobs": self.backlog.borrow().len(),
            "pruned_jobs": self.backlog.borrow().pruned,
            "catch_ups": self.catch_ups.get(),
            "gpu_reads": self.gpu_reads,
            "gpu_read_backs": self.gpu_read_backs.get(),
            "gpu_only_jobs": self.gpu_only.get(),
            "released_jobs": self.backlog.borrow().released,
            "counters_read": self.counters_read.get(),
            "software_picture": self.picture_wanted(),
            "gpu_2d": self.banshee.is_some().then(|| self.gpu_describe()),
            "banshee": self.banshee.as_ref().map(|b| b.describe()),
            "banshee_3d": self.banshee.is_some().then(|| self.banshee_3d_describe()),
            "cmdfifo": self.cmdfifo_on().then(|| serde_json::json!({
                "base": format!("{:06X}", self.cmdfifo.base),
                "end": format!("{:06X}", self.cmdfifo.end),
                "read": format!("{:06X}", self.cmdfifo.read),
                "depth": self.cmdfifo.depth,
                "holes": self.cmdfifo.holes,
            })),
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
        // Everything drawn before its memory is saved, the GPU's 2D
        // operations too.
        self.catch_up();
        self.gpu_sync_all();
        self.board.save(w);
        self.fbi.ram.save(w);
        for tmu in self.tmu.iter().filter(|t| !t.ram.same(&self.fbi.ram)) {
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
        // Added later, at the end, where a state without them ends: the
        // swap the frame rate cap holds back.
        let (data, due) = self.deferred.unwrap_or_default();
        self.deferred.is_some().save(w);
        data.save(w);
        due.save(w);
        self.capped_at.save(w);
        // Then a Voodoo2's own.
        self.cmdfifo.save(w);
        self.svert.save(w);
        self.sverts.save(w);
        self.blt.save(w);
        if let Some(b) = &self.banshee {
            b.save(w);
        }
    }

    fn load(&mut self, r: &mut Reader) -> Result<()> {
        self.flush();
        let mut board = Board::default();
        board.load(r)?;
        if board != self.board {
            return Err(StateError::Mismatch(format!(
                "its 3dfx card is a {} with {} MB, this one a {} with {} MB",
                board.model().name(),
                board.megabytes(),
                self.board.model().name(),
                self.board.megabytes()
            )));
        }
        self.fbi.ram.load(r)?;
        let fb = self.fbi.ram.clone();
        for tmu in self.tmu.iter_mut().filter(|t| !t.ram.same(&fb)) {
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
        let (mut deferred, mut data, mut due) = (false, 0u32, 0u64);
        self.capped_at = 0;
        if !r.is_empty() {
            deferred.load(r)?;
            data.load(r)?;
            due.load(r)?;
            self.capped_at.load(r)?;
        }
        self.deferred = deferred.then_some((data, due));
        self.cmdfifo = Default::default();
        self.svert = Default::default();
        self.sverts = 0;
        self.blt = Default::default();
        if !r.is_empty() {
            self.cmdfifo.load(r)?;
            self.svert.load(r)?;
            self.sverts.load(r)?;
            self.blt.load(r)?;
        }
        if let Some(b) = &mut self.banshee {
            b.load(r)?;
            b.engine.after_load();
            b.dirty = true;
        }
        self.after_load();
        Ok(())
    }
}
