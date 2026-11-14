//! ToadForce, a PCI display adapter that never existed: rust-dos defines
//! its registers, its memory, its command ring and its capabilities, and
//! this module is the card. `docs/toadforce-spec.md` is its interface as
//! a guest sees it, which the code and its tests follow chapter by
//! chapter, and `docs/toadforce.md` says why it is shaped this way.
//!
//! The card is three windows and an interrupt line:
//!
//! - BAR 0, the registers (`regs`): identity, the feature level, the
//!   error latch, the display's mode (`display`) and the capability
//!   table (`caps`).
//! - BAR 1, card memory, which holds every surface. It is the Super
//!   VGA's video memory (`Bus::vbe`), which the VESA BIOS's modes use
//!   the start of, so the card here is handed it with each access and
//!   never keeps it.
//! - BAR 2, the command ring. A write of `RING_PUT` runs the commands
//!   the guest put there (`command`), all of them, before it returns.
//!
//! Nothing here knows the bus. `Bus`'s side (`bus/toadforce.rs`) decodes
//! the windows, tells the card the time with every access (`Live`),
//! and carries out what an access changed outside the card (`Effects`):
//! the picture on the screen and the interrupt line.
//!
//! Feature level 0 is the card as a display adapter: modes, `FILL`,
//! `BLT` and `FLIP`. The 3D levels add commands and capability entries
//! and leave this level's as they are.

pub mod blit;
pub mod caps;
pub mod capture;
pub mod command;
pub mod display;
pub mod format;
#[cfg(feature = "voodoo-gl")]
pub mod gl;
pub mod gpu;
pub mod level1;
pub mod pci;
pub mod raster;
pub mod regs;

use command::Command;
pub use display::{Change, Live};
use pci::Bar;
use regs::Error;

/// What draws the card's triangles (`toadforce_renderer`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Renderer {
    /// The host's OpenGL where the window has it, and the card's own
    /// rasteriser elsewhere.
    #[default]
    Auto,
    /// The host's OpenGL; where there is none, the log says so and the
    /// card's own rasteriser draws.
    OpenGl,
    /// The card's own rasteriser, on the processor.
    Software,
}

impl Renderer {
    pub const ALL: [Renderer; 3] = [Renderer::Auto, Renderer::OpenGl, Renderer::Software];

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Renderer::Auto),
            "opengl" => Some(Renderer::OpenGl),
            "software" => Some(Renderer::Software),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Renderer::Auto => "auto",
            Renderer::OpenGl => "opengl",
            Renderer::Software => "software",
        }
    }
}

/// What an access changed outside the card, for the bus to carry out.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Effects {
    /// What the screen has to follow.
    pub display: Change,
    /// The bytes of card memory commands wrote: each run from its first
    /// byte to after its last.
    pub drawn: Vec<(u32, u32)>,
}

/// Where the commands being run came from, for `ERROR_AT`.
#[derive(Clone, Copy)]
enum Origin {
    /// The ring, from this byte offset on.
    Ring(u32),
    /// `ToadForce::submit`.
    Direct,
}

impl Origin {
    /// What `ERROR_AT` holds for the command `word` words in.
    fn at(self, word: usize) -> u32 {
        match self {
            Origin::Ring(base) => base + 4 * word as u32,
            Origin::Direct => regs::AT_DIRECT,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ToadForce {
    /// The ring's contents, as words.
    ring: Vec<u32>,
    pub pci: pci::Config,
    /// The bytes of card memory.
    memory: u32,
    level_max: u32,
    level: u32,
    irq_status: u32,
    irq_enable: u32,
    ring_put: u32,
    ring_get: u32,
    error: u32,
    error_at: u32,
    pub display: display::Display,
    /// The state the commands of level 1 set.
    pub level1: level1::Level1,
    /// What draws level 1's primitives. It holds nothing between two
    /// runs of commands, and no part of the card's state.
    raster: raster::Renderer,
    /// The host renderer, if one is attached, and what it has that
    /// card memory hasn't. No part of the card's state: a saved state
    /// has card memory with all of it read back.
    gpu: gpu::Gpu,
    /// The interrupt request the card has up, which the bus keeps.
    pub irq_up: Option<u8>,
    /// Commands the emulator's own drivers handed over that haven't run
    /// yet (`queue`), in their order.
    queue: Vec<u32>,
    /// When the oldest of them was queued, in nanoseconds of the
    /// emulated clock.
    queue_since: u64,
    /// The commands since the card was made, for a look at what drives
    /// it. No part of the card's state.
    counts: Counts,
    /// The recording of what the card is given to draw, while one is
    /// made (`capture`). No part of the card's state.
    capture: capture::Slot,
    /// Where the registers' and the ring's windows are and how long,
    /// those that decode: worked out when a BAR, the command register
    /// or the machine's RAM changes (`Bus::toadforce_moved`), so that
    /// an access compares its address with them and nothing else.
    pub decoded: [Option<(u32, u32)>; 2],
}

/// The version of the card's saved state, which a saved state's `TOAD`
/// section and a recording (`capture`) carry. A change to what
/// `state_fields!` below saves takes a new one.
pub const STATE_VERSION: u16 = 3;

/// How long a queued command waits at most, in nanoseconds of the
/// emulated clock: well under a frame, so that a picture drawn straight
/// into what the display shows is there by the next one.
pub const QUEUE_NS: u64 = 5_000_000;

/// The words the queue holds at most.
pub const QUEUE_WORDS: usize = 1 << 18;

/// How many commands the card carried out, by kind, and how many it
/// rejected (register writes among those).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub nop: u64,
    pub fill: u64,
    pub blt: u64,
    pub flip: u64,
    /// The commands level 1 adds.
    pub level1: u64,
    /// The draw commands among them, and the triangles, lines and
    /// points they drew.
    pub draws: u64,
    pub primitives: u64,
    /// The draws whose vertices the card transformed and lit.
    pub transformed: u64,
    pub rejected: u64,
}

// The ring first: rewind compares states block by block from a section's
// start, so what is large and seldom changes goes before what moves.
crate::state_fields!(ToadForce {
    ring,
    pci,
    memory,
    level_max,
    level,
    irq_status,
    irq_enable,
    ring_put,
    ring_get,
    error,
    error_at,
    display,
    irq_up,
    level1,
    queue,
    queue_since,
} skip { counts, raster, gpu, capture, decoded });

impl ToadForce {
    /// A card with `memory` bytes of card memory (one of
    /// `regs::MEMORY_SIZES`) and the feature levels up to `level_max`, as
    /// at power-on.
    // While level 0 is the newest, the `min` below has nothing to do.
    #[allow(clippy::unnecessary_min_or_max)]
    pub fn new(memory: u32, level_max: u32) -> Self {
        debug_assert!(regs::MEMORY_SIZES.contains(&memory));
        ToadForce {
            ring: vec![0; (regs::RING_SIZE / 4) as usize],
            pci: pci::Config::new(memory),
            memory,
            level_max: level_max.min(caps::NEWEST_LEVEL),
            level: 0,
            irq_status: 0,
            irq_enable: 0,
            ring_put: 0,
            ring_get: 0,
            error: 0,
            error_at: 0,
            display: display::Display::default(),
            level1: level1::Level1::default(),
            raster: raster::Renderer::default(),
            gpu: gpu::Gpu::default(),
            irq_up: None,
            queue: Vec::new(),
            queue_since: 0,
            counts: Counts::default(),
            capture: capture::Slot::default(),
            decoded: [None; 2],
        }
    }

    /// Record what the card is given to draw from its next run of
    /// commands on, until `flips` page flips or with 0 until
    /// `record_end`, into the new file `path` (`capture`).
    pub fn record(&mut self, path: &std::path::Path, flips: u32) -> std::io::Result<()> {
        self.capture.0 = Some(Box::new(capture::Recorder::new(path, flips)?));
        Ok(())
    }

    /// End the recording, if one is being made.
    pub fn record_end(&mut self) {
        self.capture.0 = None;
    }

    /// Whether a recording is being made.
    pub fn recording(&self) -> bool {
        self.capture.0.is_some()
    }

    /// The bytes of card memory.
    pub fn memory(&self) -> u32 {
        self.memory
    }

    /// How many `FILL` and `BLT` commands the card carried out.
    pub fn drawn(&self) -> u64 {
        self.counts.fill + self.counts.blt
    }

    /// The commands the card carried out and rejected.
    pub fn counts(&self) -> Counts {
        self.counts
    }

    /// Have `host` draw the triangles it takes from now on, or with
    /// None the software renderer all of them again. What the host
    /// before it drew is read back into `memory`, card memory, first.
    pub fn set_host(&mut self, host: Option<Box<dyn gpu::Host>>, memory: &mut [u8]) {
        self.gpu.settle(memory, None, gpu::Cause::Other);
        self.gpu.set_host(host, self.memory);
    }

    /// Whether the host renderer has pixels card memory hasn't, which
    /// `settle` reads back. False without a host.
    #[inline]
    pub fn ahead(&self) -> bool {
        self.gpu.ahead()
    }

    /// Before the processor reads or writes the bytes `range` of
    /// `memory`, card memory (all of it for None): read back the
    /// surfaces the host renderer drew into that share a byte with
    /// them.
    #[inline]
    pub fn settle(&mut self, memory: &mut [u8], range: Option<(u32, u32)>) {
        self.settle_for(memory, range, gpu::Cause::Program);
    }

    /// `settle`, with what the bytes are wanted for, for the status's
    /// counts.
    pub fn settle_for(&mut self, memory: &mut [u8], range: Option<(u32, u32)>, why: gpu::Cause) {
        self.gpu.settle(memory, range, why);
    }

    /// The processor wrote `len` bytes of card memory at `offset`: the
    /// host renderer's copies of them are stale.
    #[inline]
    pub fn wrote(&mut self, offset: u32, len: u32) {
        self.gpu.pages.wrote(offset, len);
    }

    /// `memory`, card memory, with what the host renderer drew read
    /// back into a copy, when there is any: what a saved state keeps.
    /// The card and its memory stay as they are.
    pub fn settled(&self, memory: &[u8]) -> Option<Vec<u8>> {
        self.gpu.settled(memory)
    }

    /// Card memory was replaced, by a loaded state: the host renderer
    /// drops what it has. A recording ends, as the state the card
    /// jumped to isn't in it and a replay would go on from the old one.
    pub fn forget(&mut self) {
        self.gpu.forget();
        self.record_end();
    }

    /// The processor is about to write `len` bytes of `memory`, card
    /// memory, at `offset`, where `ahead`: the card notes them, and
    /// nothing is read back (`gpu::Gpu::writing`).
    pub fn writing(&mut self, offset: u32, len: u32) {
        self.gpu.writing(offset, len);
    }

    /// The program reads `len` bytes of `memory`, card memory, at
    /// `offset` by itself, where `ahead`: their value if the host has
    /// newer pixels there (`gpu::Gpu::read`).
    pub fn read_ahead(&self, offset: u32, len: u32, memory: &[u8]) -> Option<u32> {
        self.gpu.read(offset, len, memory)
    }

    /// Whether the host renderer shows `surface`, the picture on the
    /// screen, from its own copy this frame (`Gpu::shows`).
    pub fn host_shows(&mut self, surface: &format::Surface, memory: &mut [u8]) -> bool {
        self.gpu.shows(surface, memory)
    }

    /// Whether the host shows this frame's picture: asked once, by the
    /// read-back of the picture, which it replaces.
    pub fn host_presents(&mut self) -> bool {
        self.gpu.take_presents()
    }

    /// Whether a host renderer is attached (`set_host`).
    pub fn hosted(&self) -> bool {
        self.gpu.attached()
    }

    /// For the debugger's status: what the card shows and what was asked
    /// of it, at `live`. Reads change nothing.
    pub fn describe(&self, live: Live) -> serde_json::Value {
        let read = |offset| self.read(offset, live);
        let counts = self.counts;
        let bars = [Bar::Registers, Bar::Memory, Bar::Ring]
            .map(|bar| self.pci.window(bar).map(|(base, _)| format!("{base:X}")));
        serde_json::json!({
            "memory_mb": self.memory >> 20,
            "level": self.level,
            "level_max": self.level_max,
            "mode": self.display.shown().map(|shown| serde_json::json!({
                "width": shown.width,
                "height": shown.height,
                "format": format!("{:?}", shown.format),
                "pitch": shown.pitch,
                "refresh": shown.refresh,
                "start": format!("{:X}", self.display.start()),
            })),
            "frame": read(regs::FRAME),
            "scanline": read(regs::SCANLINE),
            "status": format!("{:X}", read(regs::STATUS)),
            "irq_status": format!("{:X}", read(regs::IRQ_STATUS)),
            "irq_enable": format!("{:X}", self.irq_enable),
            "irq": self.irq_up,
            "ring_put": format!("{:X}", self.ring_put),
            "error": self.error,
            "error_at": format!("{:X}", self.error_at),
            "bars": bars,
            "commands": {
                "nop": counts.nop,
                "fill": counts.fill,
                "blt": counts.blt,
                "flip": counts.flip,
                "level1": counts.level1,
                "draws": counts.draws,
                "primitives": counts.primitives,
                "transformed": counts.transformed,
                "rejected": counts.rejected,
            },
            "renderer": self.gpu.name(),
            "recording": self.recording(),
            "host": {
                "read_backs": self.gpu.read_backs(),
                "read_backs_for": self.gpu.causes(),
                "declined_draws": self.gpu.declined(),
                "declined_for": self.gpu.declined_for(),
                "writes": self.gpu.writes(),
                "counts": self.gpu.status(),
            },
        })
    }

    /// The highest feature level the card has.
    pub fn level_max(&self) -> u32 {
        self.level_max
    }

    /// The feature level in use.
    pub fn level(&self) -> u32 {
        self.level
    }

    /// The interrupt line the card asserts, if it asserts INTA# and the
    /// system gave it a line.
    pub fn irq(&self) -> Option<u8> {
        let line = self.pci.interrupt_line;
        (self.irq_status & self.irq_enable != 0 && line < 16).then_some(line)
    }

    /// When the card next changes by itself, in nanoseconds of the
    /// emulated clock: the vertical blank that raises an interrupt or
    /// makes a waiting flip, which `service` then brings the card to, or
    /// the queue's deadline (`queue_due`), when its owner runs it.
    pub fn next_event(&self) -> Option<u64> {
        let blank_irq =
            self.irq_enable & regs::IRQ_VBLANK != 0 && self.irq_status & regs::IRQ_VBLANK == 0;
        let blank = (blank_irq || self.display.flip_pending())
            .then(|| self.display.next_blank())
            .flatten();
        let queue = self.queued().then_some(self.queue_since + QUEUE_NS);
        match (blank, queue) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Queue the commands in `words` behind those queued before: they
    /// run, in order, with `run_queue`. True when the queue then holds
    /// as much as it may, or has held its oldest command as long as it
    /// may, and is to be run now.
    ///
    /// The emulator's own drivers hand their commands over this way. A
    /// draw by itself is a few rows on one thread; the draws of a batch
    /// go to the renderer's threads together. Nothing but the queue's
    /// owner sees the difference as long as the queue is run before
    /// anything else reads or writes card memory or the registers,
    /// which is the owner's to see to (`Bus::toadforce_flush`).
    pub fn queue(&mut self, words: &[u32], live: Live) -> bool {
        if self.queue.is_empty() {
            self.queue_since = live.ns;
        }
        self.queue.extend_from_slice(words);
        self.queue.len() >= QUEUE_WORDS || self.queue_due(live)
    }

    /// The queue, taken out of the card for a driver to write whole
    /// commands at its end, where they stay: `queue_put` gives it
    /// back. Until then the card has nothing queued, so the driver
    /// does nothing in between that runs the queue or adds to it.
    pub fn queue_take(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.queue)
    }

    /// The queue back from `queue_take`, which held `before` words
    /// then. True when it is to be run now, as `queue` says.
    pub fn queue_put(&mut self, words: Vec<u32>, before: usize, live: Live) -> bool {
        debug_assert!(self.queue.is_empty(), "commands were queued past a taken queue");
        debug_assert!(words.len() >= before && command::framed(&words[before..]));
        if before == 0 && !words.is_empty() {
            self.queue_since = live.ns;
        }
        self.queue = words;
        self.queue.len() >= QUEUE_WORDS || self.queue_due(live)
    }

    /// Whether commands are queued.
    #[inline]
    pub fn queued(&self) -> bool {
        !self.queue.is_empty()
    }

    /// Whether the oldest queued command has waited as long as it may.
    pub fn queue_due(&self, live: Live) -> bool {
        self.queued() && live.ns >= self.queue_since + QUEUE_NS
    }

    /// Run the queued commands, as `submit` runs them.
    pub fn run_queue(&mut self, memory: &mut [u8], live: Live) -> Effects {
        let mut words = std::mem::take(&mut self.queue);
        let fx = self.submit(&words, memory, live);
        // The buffer is kept for the next batch.
        words.clear();
        self.queue = words;
        fx
    }

    /// Bring the card up to `live`: the vertical blanks that began, and
    /// what they do.
    pub fn service(&mut self, live: Live) -> Effects {
        let mut fx = Effects::default();
        self.sync(live, &mut fx);
        fx
    }

    fn sync(&mut self, live: Live, fx: &mut Effects) {
        let synced = self.display.sync(live);
        self.irq_status |= irq_bits(synced);
        if synced.flipped {
            fx.display = fx.display.max(Change::Picture);
        }
    }

    /// The register at `offset` of BAR 0, a multiple of 4, as it reads at
    /// `live`. Reading changes nothing.
    pub fn read(&self, offset: u32, live: Live) -> u32 {
        match offset {
            regs::ID => regs::ID_VALUE,
            regs::LEVEL_MAX => self.level_max,
            regs::LEVEL => self.level,
            regs::STATUS => {
                let error = if self.error != 0 {
                    regs::STATUS_ERROR
                } else {
                    0
                };
                self.display.status(live) | error
            }
            regs::IRQ_STATUS => self.irq_status | irq_bits(self.display.due(live)),
            regs::IRQ_ENABLE => self.irq_enable,
            regs::RING_PUT => self.ring_put,
            regs::RING_GET => self.ring_get,
            regs::ERROR => self.error,
            regs::ERROR_AT => self.error_at,
            regs::MEMORY => self.memory,
            regs::MODE_ENABLE..=regs::SCANLINE | regs::PALETTE..regs::PALETTE_END => {
                self.display.read(offset, live)
            }
            regs::CAPS..regs::CAPS_END => {
                caps::read(self.level, ((offset - regs::CAPS) / 4) as usize)
            }
            _ => 0,
        }
    }

    /// A word written to the register at `offset` of BAR 0, a multiple of
    /// 4, at `live`. `memory` is card memory, which a write of
    /// `RING_PUT` draws into.
    pub fn write(&mut self, offset: u32, value: u32, memory: &mut [u8], live: Live) -> Effects {
        let mut fx = Effects::default();
        self.sync(live, &mut fx);
        match offset {
            // A level the card doesn't have is ignored: a driver reads
            // the register back.
            // A write of the level in use counts too: a driver that
            // takes the card over starts from the defaults.
            regs::LEVEL if value <= self.level_max => {
                self.level = value;
                self.level1 = level1::Level1::default();
            }
            regs::IRQ_STATUS => self.irq_status &= !value,
            regs::IRQ_ENABLE => self.irq_enable = value & regs::IRQ_ALL,
            regs::RING_PUT => {
                self.put(value, memory, &mut fx);
                // A guest's own driver reads and writes card memory
                // through the bus with nothing that says so first, and
                // a read there can't read back (`Bus::read_8`). What
                // its commands drew is in card memory when the write
                // returns, as the specification has it.
                self.gpu.settle(memory, None, gpu::Cause::Ring);
            }
            regs::ERROR => {
                self.error = 0;
                self.error_at = 0;
            }
            regs::RESET if value & 1 != 0 => self.reset(live, &mut fx),
            regs::MODE_ENABLE..=regs::MODE_REFRESH | regs::PALETTE..regs::PALETTE_END => {
                match self.display.write(offset, value, self.memory, live) {
                    Ok(change) => fx.display = fx.display.max(change),
                    Err(error) => self.latch(error, regs::AT_REGISTER),
                }
            }
            _ => {}
        }
        fx
    }

    /// Every register at its reset value, no mode shown, no flip pending
    /// and every command state at its default. Card memory, the ring's
    /// contents and PCI configuration space keep what they hold.
    pub fn reset(&mut self, live: Live, fx: &mut Effects) {
        self.level = 0;
        self.level1 = level1::Level1::default();
        self.irq_status = 0;
        self.irq_enable = 0;
        self.ring_put = 0;
        self.ring_get = 0;
        self.error = 0;
        self.error_at = 0;
        self.display.reset(live);
        fx.display = Change::Mode;
    }

    /// Latch the first rejection since `ERROR` was last written.
    fn latch(&mut self, error: Error, at: u32) {
        self.counts.rejected += 1;
        if self.error == 0 {
            self.error = error as u32;
            self.error_at = at;
            self.irq_status |= regs::IRQ_ERROR;
        }
    }

    /// `RING_PUT` written: run the commands from `RING_GET` up to
    /// `value`, round the ring's end if `value` is below it.
    fn put(&mut self, value: u32, memory: &mut [u8], fx: &mut Effects) {
        if !value.is_multiple_of(4) || value > regs::RING_PUT_MAX {
            self.latch(Error::BadPut, regs::AT_REGISTER);
            return;
        }
        // The commands don't change the ring, which is taken out of the
        // card while they change the rest of it.
        let ring = std::mem::take(&mut self.ring);
        let (get, put) = (self.ring_get as usize / 4, value as usize / 4);
        if put >= get {
            self.run(&ring[get..put], Origin::Ring(self.ring_get), memory, fx);
        } else if self.run(&ring[get..], Origin::Ring(self.ring_get), memory, fx) {
            self.run(&ring[..put], Origin::Ring(0), memory, fx);
        }
        self.ring = ring;
        self.ring_put = value;
        self.ring_get = value;
    }

    /// Run the commands `words` hold, as a write of `RING_PUT` runs the
    /// ring's: the way in for the emulator's own drivers (the built-in
    /// Windows'), which hand the card the words a guest's driver would
    /// have written into the ring, without the bus in between.
    pub fn submit(&mut self, words: &[u32], memory: &mut [u8], live: Live) -> Effects {
        let mut fx = Effects::default();
        self.sync(live, &mut fx);
        self.run(words, Origin::Direct, memory, &mut fx);
        fx
    }

    /// Run the commands in `words`, one after the other. False if a
    /// header's length left no next command to find, which drops the
    /// rest of what was submitted.
    fn run(&mut self, words: &[u32], origin: Origin, memory: &mut [u8], fx: &mut Effects) -> bool {
        let recorder = self.capture.0.take();
        let flips = self.counts.flip;
        let recorder = recorder.and_then(|mut recorder| {
            // The recording has card memory with everything drawn.
            self.gpu.settle(memory, None, gpu::Cause::Other);
            let mut state = crate::savestate::Writer::new();
            crate::savestate::State::save(self, &mut state);
            match recorder.before(&state.buf, memory, words) {
                Ok(()) => Some(recorder),
                Err(e) => {
                    eprintln!("[TOADFORCE] the recording ends: {e}");
                    None
                }
            }
        });
        let whole = self.run_each(words, origin, memory, fx);
        // The draws of a run are drawn by its end.
        self.raster.flush(memory, &mut self.gpu);
        if let Some(mut recorder) = recorder {
            self.gpu.settle(memory, None, gpu::Cause::Other);
            let flips = (self.counts.flip - flips) as u32;
            match recorder.after(memory, flips) {
                Ok(false) => self.capture.0 = Some(recorder),
                Ok(true) => {}
                Err(e) => eprintln!("[TOADFORCE] the recording ends: {e}"),
            }
        }
        whole
    }

    fn run_each(
        &mut self,
        words: &[u32],
        origin: Origin,
        memory: &mut [u8],
        fx: &mut Effects,
    ) -> bool {
        debug_assert_eq!(memory.len(), self.memory as usize);
        let mut at = 0;
        while at < words.len() {
            let len = command::length(words[at]);
            if len == 0 || len > words.len() - at {
                self.latch(Error::BadLength, origin.at(at));
                return false;
            }
            let done = command::decode(&words[at..at + len], self.level, self.memory)
                .and_then(|c| self.execute(c, memory, fx));
            if let Err(error) = done {
                self.latch(error, origin.at(at));
            }
            at += len;
        }
        true
    }

    /// Draw with `workers` threads beside the emulator's, whatever the
    /// host has.
    #[cfg(test)]
    pub(crate) fn fix_workers(&mut self, workers: usize) {
        self.raster.fix_workers(workers);
    }

    /// Carry out a decoded command.
    fn execute(
        &mut self,
        command: Command,
        memory: &mut [u8],
        fx: &mut Effects,
    ) -> Result<(), Error> {
        // What the queued draws were to draw is there before anything
        // else reads or writes the memory.
        if !matches!(
            command,
            Command::Nop | Command::Level1(level1::Command::Draw(_))
        ) && !matches!(command, Command::Level1(c) if c.sets_state())
        {
            self.raster.flush(memory, &mut self.gpu);
        }
        // The commands below that run on the processor read back what
        // the host renderer drew where they read or write, and count
        // what they wrote (`gpu`).
        match command {
            Command::Nop => self.counts.nop += 1,
            Command::Fill { dst, rect, value } => {
                let span = dst.span(&rect);
                // The host's, where it has the destination ahead.
                if !self.gpu.fill(&dst, &rect, value, memory) {
                    self.gpu.settle(memory, Some(span), gpu::Cause::Blit);
                    blit::fill(memory, &dst, &rect, value);
                    self.gpu.pages.wrote(span.0, span.1 - span.0);
                }
                self.counts.fill += 1;
                fx.drawn.push(span);
            }
            Command::Blt {
                dst,
                dst_rect,
                src,
                src_rect,
                key,
            } => {
                let span = dst.span(&dst_rect);
                let read = src.span(&src_rect);
                // The host's, where it has either surface ahead.
                let hosted = self.gpu.blt(&dst, &dst_rect, &src, &src_rect, key, memory);
                if !hosted {
                    self.gpu.settle(memory, Some(read), gpu::Cause::Blit);
                    self.gpu.settle(memory, Some(span), gpu::Cause::Blit);
                    blit::blt(memory, &dst, &dst_rect, &src, &src_rect, key);
                    self.gpu.pages.wrote(span.0, span.1 - span.0);
                }
                self.counts.blt += 1;
                fx.drawn.push(span);
            }
            Command::Flip { offset, wait } => {
                if self.display.flip(offset, wait, self.memory)? {
                    self.irq_status |= regs::IRQ_FLIP;
                    fx.display = fx.display.max(Change::Picture);
                }
                self.counts.flip += 1;
            }
            Command::Level1(level1::Command::Draw(draw)) => {
                let drawn = self
                    .raster
                    .draw(&mut self.level1, &draw, memory, &mut self.gpu)?;
                self.counts.level1 += 1;
                self.counts.draws += 1;
                self.counts.transformed += !draw.fvf.transformed as u64;
                if let Some(drawn) = drawn {
                    self.counts.primitives += drawn.primitives;
                    fx.drawn.push(drawn.span);
                }
            }
            Command::Level1(level1::Command::Clear {
                flags,
                colour,
                z,
                stencil,
                rects,
            }) => {
                // `CLEAR` is the one command of level 1 that touches
                // card memory. A buffer the host has a copy of is
                // cleared there. The others are written here, after
                // what the host drew in them is read back: the depth
                // buffer is read where a part of its bits is written.
                let (rects, buffers) = self.level1.clear(flags, colour, z, stencil, rects)?;
                for (surface, value, mask) in buffers {
                    let clear = gpu::Clear {
                        surface,
                        rects: &rects,
                        value,
                        mask,
                    };
                    let spans = rects.iter().map(|rect| surface.span(rect));
                    if !self.gpu.clear(&clear, memory) {
                        let bytes = gpu::range(&surface);
                        self.gpu.settle(memory, Some(bytes), gpu::Cause::Clear);
                        clear.write(memory);
                        for (first, end) in spans.clone() {
                            self.gpu.pages.wrote(first, end - first);
                        }
                    }
                    fx.drawn.extend(spans);
                }
                self.counts.level1 += 1;
            }
            Command::Level1(command) => {
                self.level1.apply(command)?;
                self.counts.level1 += 1;
            }
        }
        Ok(())
    }

    /// `len` bytes (1 to 4) of the ring at byte `offset`, as the guest
    /// reads back what it wrote. Bytes past the ring's end read as 0.
    pub fn ring_read(&self, offset: u32, len: u32) -> u32 {
        (0..len)
            .map(|i| {
                let at = offset + i;
                let word = self.ring.get((at / 4) as usize).copied().unwrap_or(0);
                (word >> (8 * (at & 3)) & 0xFF) << (8 * i)
            })
            .sum()
    }

    /// The low `len` bytes (1 to 4) of `value` written to the ring at
    /// byte `offset`.
    pub fn ring_write(&mut self, offset: u32, value: u32, len: u32) {
        for i in 0..len {
            let at = offset + i;
            if let Some(word) = self.ring.get_mut((at / 4) as usize) {
                let shift = 8 * (at & 3);
                *word = *word & !(0xFF << shift) | (value >> (8 * i) & 0xFF) << shift;
            }
        }
    }
}

/// The bits of `IRQ_STATUS` what a `sync` found sets.
fn irq_bits(synced: display::Synced) -> u32 {
    let blank = if synced.blank { regs::IRQ_VBLANK } else { 0 };
    let flip = if synced.flipped { regs::IRQ_FLIP } else { 0 };
    blank | flip
}

#[cfg(test)]
mod tests;
