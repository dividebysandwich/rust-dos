//! A PowerVR PCX2 (Matrox m3D, VideoLogic Apocalypse 3Dx): NEC's PCI 3D
//! accelerator with no display of its own. The driver writes the scene
//! as planes and an object list into host memory and texturing records
//! and textures into the card's 4 MB; the card renders it a 32-pixel
//! tile at a time and writes the finished pixels over the bus into the
//! VGA card's linear frame buffer, so the picture reaches the monitor
//! through the VGA.
//!
//! What the chip does comes from Imagination's driver source for the
//! PowerVR Series1 (github.com/powervr-graphics/PowerVR-Series1, MIT
//! licensed): the register map and the formats of the parameters it
//! reads, and its C simulator of the PCX2 (`Source/simulat3`).
//!
//! The card is PCI device 2 (vendor 1033h, NEC; device 0046h), BAR0 its
//! registers, BAR1 its texture memory.

pub mod regs;
pub mod render;
pub mod tsp;

use crate::savestate::{Reader, Result, State, StateError, Writer};
use std::cell::RefCell;
use std::io::Write;

/// The size of BAR0, the registers' window.
pub const REGISTER_WINDOW: u32 = 64 << 10;
/// The size of BAR1, the texture memory.
pub const TEXTURE_MEMORY: u32 = 4 << 20;
/// Where the BIOS puts the windows: above the 3dfx card's.
pub const INITIAL_REGISTERS: u32 = 0xD100_0000;
pub const INITIAL_TEXTURES: u32 = 0xD140_0000;
/// The interrupt line the BIOS routed the card's INTA# to.
pub const IRQ: u8 = 11;
/// The chip's clock, which its ISP and TSP run at.
const CLOCK_HZ: u64 = 66_000_000;

/// The chips.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Chip {
    #[default]
    Pcx2,
}

crate::state_enum!(Chip { Chip::Pcx2 });

impl Chip {
    /// A `powervr` setting: the chip, or None for no card.
    pub fn parse(s: &str) -> Option<Option<Self>> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "false" | "no" | "none" => Some(None),
            "pcx2" | "on" | "true" | "yes" => Some(Some(Chip::Pcx2)),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Chip::Pcx2 => "pcx2",
        }
    }

    fn device_id(self) -> u32 {
        match self {
            Chip::Pcx2 => 0x0046,
        }
    }
}

/// Which window an address is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Window {
    Registers,
    Textures,
}

#[derive(Clone, Debug)]
struct PciConfig {
    command: u16,
    bar0: u32,
    bar1: u32,
    irq_line: u8,
}

crate::state_fields!(PciConfig { command, bar0, bar1, irq_line });

impl Default for PciConfig {
    /// As the BIOS leaves it: memory decoding and bus mastering on.
    fn default() -> Self {
        Self { command: 0x0006, bar0: INITIAL_REGISTERS, bar1: INITIAL_TEXTURES, irq_line: IRQ }
    }
}

/// The frame buffer's pixel formats (PACKMODE).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    Rgb32,
    Rgb24,
    Rgb565,
    Rgb555,
}

impl PixelFormat {
    pub fn bytes(self) -> u32 {
        match self {
            PixelFormat::Rgb32 => 4,
            PixelFormat::Rgb24 => 3,
            PixelFormat::Rgb565 | PixelFormat::Rgb555 => 2,
        }
    }

    /// A pixel (0xRRGGBB) packed, with the 4x4 ordered dither's `bias`
    /// (0-7, added before dropping bits) if dithering.
    pub fn pack(self, rgb: u32, bias: u32) -> u32 {
        let channel = |shift: u32, bits: u32| {
            let c = (rgb >> shift & 0xFF) + (bias >> (bits - 5));
            c.min(255) >> (8 - bits)
        };
        match self {
            PixelFormat::Rgb32 | PixelFormat::Rgb24 => rgb,
            PixelFormat::Rgb565 => channel(16, 5) << 11 | channel(8, 6) << 5 | channel(0, 5),
            PixelFormat::Rgb555 => channel(16, 5) << 10 | channel(8, 5) << 5 | channel(0, 5),
        }
    }
}

/// Where and how a render's pixels go.
pub struct Output {
    pub address: u32,
    pub stride: u32,
    pub format: PixelFormat,
    pub dither: bool,
    pub columns: std::ops::Range<u32>,
}

pub struct PowerVr {
    pub chip: Chip,
    pci: PciConfig,
    regs: Vec<u32>,
    textures: Vec<u8>,
    /// Renders started.
    renders: u32,
    /// A render the bus has to carry out.
    started: bool,
    /// When the render under way ends, in PIT ticks.
    end_at: Option<u64>,
    /// How the host shades.
    pub shader: tsp::Shader,
    /// Lines for the emulator's log.
    pub log: Vec<String>,
    /// `RUST_DOS_POWERVR_TRACE`: every access, written to `trace.txt` in
    /// that folder, with snapshots of the first renders beside it.
    trace: RefCell<Option<Trace>>,
}

struct Trace {
    dir: std::path::PathBuf,
    out: std::io::BufWriter<std::fs::File>,
    /// Snapshots of every this many renders (`RUST_DOS_POWERVR_TRACE_EVERY`).
    every: u32,
}

impl PowerVr {
    pub fn new(chip: Chip) -> Self {
        let mut card = Self {
            chip,
            pci: PciConfig::default(),
            regs: vec![0; regs::COUNT],
            textures: vec![0; TEXTURE_MEMORY as usize],
            renders: 0,
            started: false,
            end_at: None,
            shader: tsp::Shader::default(),
            log: Vec::new(),
            trace: RefCell::new(None),
        };
        card.reset();
        if let Ok(dir) = std::env::var("RUST_DOS_POWERVR_TRACE") {
            let dir = std::path::PathBuf::from(dir);
            match std::fs::create_dir_all(&dir).and_then(|_| std::fs::File::create(dir.join("trace.txt"))) {
                Ok(file) => {
                    let every = std::env::var("RUST_DOS_POWERVR_TRACE_EVERY").ok().and_then(|n| n.parse().ok()).unwrap_or(1000);
                    *card.trace.get_mut() = Some(Trace { dir, out: std::io::BufWriter::new(file), every: u32::max(every, 1) });
                }
                Err(e) => card.log.push(format!("[PVR] Can't trace into {}: {}", dir.display(), e)),
            }
        }
        card
    }

    /// As after a PCI reset.
    pub fn reset(&mut self) {
        self.pci = PciConfig::default();
        self.regs.fill(0);
        self.regs[regs::ID] = 0x0000_0046;
        self.regs[regs::REVISION] = 0x0000_0002;
    }

    // --- PCI ---

    /// A byte of the configuration space.
    pub fn config_read(&self, reg: u8) -> u8 {
        let dword = match reg & 0xFC {
            0x00 => self.chip.device_id() << 16 | 0x1033,
            // Fast back-to-back capable, medium DEVSEL.
            0x04 => 0x0280_0000 | self.pci.command as u32,
            // Revision 2, a multimedia device of another kind.
            0x08 => 0x0480_0002,
            0x10 => self.pci.bar0,
            0x14 => self.pci.bar1 | 0x08,
            // INTA#, routed to `irq_line`.
            0x3C => 0x0000_0100 | self.pci.irq_line as u32,
            _ => 0,
        };
        let value = (dword >> (8 * (reg & 3))) as u8;
        self.trace_line(|| format!("cfg r {:02X} = {:02X}", reg, value));
        value
    }

    /// A byte of the configuration space written.
    pub fn config_write(&mut self, reg: u8, value: u8) {
        self.trace_line(|| format!("cfg w {:02X} = {:02X}", reg, value));
        let set = |bar: &mut u32, mask: u32| {
            let shift = 8 * (reg & 3) as u32;
            *bar = (*bar & !(0xFF << shift) | (value as u32) << shift) & mask;
        };
        match reg {
            0x04 => self.pci.command = (self.pci.command & 0xFF00) | (value & 0x07) as u16,
            0x10..=0x13 => set(&mut self.pci.bar0, !(REGISTER_WINDOW - 1)),
            0x14..=0x17 => set(&mut self.pci.bar1, !(TEXTURE_MEMORY - 1)),
            0x3C => self.pci.irq_line = value,
            _ => {}
        }
    }

    /// The window `addr` falls in, and its offset there, if memory
    /// decoding is on.
    pub fn window(&self, addr: u32) -> Option<(Window, u32)> {
        if self.pci.command & 0x02 == 0 {
            return None;
        }
        let inside = |base: u32, size: u32| base != 0 && addr.wrapping_sub(base) < size;
        if inside(self.pci.bar0, REGISTER_WINDOW) {
            Some((Window::Registers, addr - self.pci.bar0))
        } else if inside(self.pci.bar1, TEXTURE_MEMORY) {
            Some((Window::Textures, addr - self.pci.bar1))
        } else {
            None
        }
    }

    /// The interrupt line, if the card interrupts.
    pub fn irq(&self) -> Option<u8> {
        (self.pci.irq_line < 16).then_some(self.pci.irq_line)
    }

    /// Whether the card asserts INTA#.
    pub fn irq_asserted(&self) -> bool {
        self.regs[regs::INTSTATUS] & self.regs[regs::INTMASK] != 0
    }

    // --- Registers ---

    pub fn read_register(&self, offset: u32) -> u32 {
        let reg = (offset >> 2) as usize;
        let value = self.regs.get(reg).copied().unwrap_or(0);
        self.trace_line(|| format!("r {:<18} = {:08X}", regs::name(reg), value));
        value
    }

    /// A register written. True if the interrupt line may have changed.
    pub fn write_register(&mut self, offset: u32, value: u32, mask: u32) -> bool {
        let reg = (offset >> 2) as usize;
        if reg >= regs::COUNT {
            return false;
        }
        let value = self.regs[reg] & !mask | value & mask;
        self.trace_line(|| format!("w {:<18} = {:08X}", regs::name(reg), value));
        match reg {
            regs::ID | regs::REVISION => {}
            // A reset of the render pipeline, before each render.
            regs::SOFTRESET => {
                self.regs[reg] = value;
                if value & 1 != 0 {
                    self.regs[regs::INTSTATUS] &= !regs::END_OF_RENDER;
                }
                return true;
            }
            // Writing a status bit clears it.
            regs::INTSTATUS => {
                self.regs[reg] &= !value;
                return true;
            }
            regs::INTMASK => {
                self.regs[reg] = value;
                return true;
            }
            regs::STARTRENDER => {
                self.regs[reg] = value;
                self.start_render();
                return true;
            }
            _ => self.regs[reg] = value,
        }
        false
    }

    /// A render starts.
    fn start_render(&mut self) {
        if self.end_at.take().is_some() {
            self.regs[regs::INTSTATUS] |= regs::END_OF_RENDER;
        }
        self.renders += 1;
        self.started = true;
        if let Some(t) = self.trace.get_mut().as_mut() {
            let _ = t.out.flush();
        }
    }

    /// The render started, if one was: the bus carries it out, then calls
    /// `finish_render`.
    pub fn take_start(&mut self) -> bool {
        std::mem::take(&mut self.started)
    }

    /// Render the scene from `ram`, the machine's memory.
    pub fn render(&self, ram: &[u8]) -> render::Rendered {
        render::render(&self.regs, &self.textures, render::Memory { ram }, &self.shader)
    }

    /// The render took the chip `rendered`'s clocks from `now` (PIT
    /// ticks): the ISP's and the TSP's, which work at once.
    pub fn rendering(&mut self, rendered: &render::Rendered, now: u64) {
        let clocks = rendered.isp_clocks.max(rendered.tsp_clocks);
        self.end_at = Some(now + clocks * crate::timer::PIT_HZ / CLOCK_HZ);
    }

    /// When the render under way ends.
    pub fn next_event(&self) -> Option<u64> {
        self.end_at
    }

    /// End the render, if its time came. True if it did.
    pub fn service(&mut self, now: u64) -> bool {
        match self.end_at {
            Some(at) if at <= now => {
                self.end_at = None;
                self.regs[regs::INTSTATUS] |= regs::END_OF_RENDER;
                true
            }
            _ => false,
        }
    }

    /// Where finished pixels go: the frame buffer's physical address, the
    /// bytes a line, the bytes a pixel, whether to dither, and the
    /// columns XCLIP lets through.
    pub fn output(&self) -> Output {
        let pack = self.regs[regs::PACKMODE];
        let xclip = self.regs[regs::XCLIP];
        let left = if xclip & 1 << 12 != 0 { xclip & 0x7FF } else { 0 };
        let right = if xclip & 1 << 28 != 0 { xclip >> 16 & 0x7FF } else { u32::MAX };
        Output {
            address: self.regs[regs::SOFADDR],
            stride: self.regs[regs::LSTRIDE],
            format: match pack & 3 {
                0 => PixelFormat::Rgb32,
                1 => PixelFormat::Rgb24,
                2 => PixelFormat::Rgb565,
                _ => PixelFormat::Rgb555,
            },
            dither: pack & 0x10 != 0,
            columns: left..right,
        }
    }

    /// The renders started.
    pub fn renders(&self) -> u32 {
        self.renders
    }

    /// The number of the render that just started, if the trace wants
    /// a snapshot of it: the first ones, and every 1000th (or as many as
    /// `RUST_DOS_POWERVR_TRACE_EVERY` says).
    pub fn snapshot_wanted(&self) -> Option<(std::path::PathBuf, u32)> {
        let trace = self.trace.borrow();
        let n = self.renders;
        trace.as_ref().filter(|t| n <= 4 || n.is_multiple_of(t.every)).map(|t| (t.dir.clone(), n))
    }

    // --- Texture memory ---

    pub fn read_texture(&self, offset: u32, len: u32) -> u32 {
        let at = offset as usize;
        (0..len as usize).map(|i| (self.textures[at + i] as u32) << (8 * i)).sum()
    }

    pub fn write_texture(&mut self, offset: u32, value: u32, len: u32) {
        let at = offset as usize;
        for i in 0..len as usize {
            self.textures[at + i] = (value >> (8 * i)) as u8;
        }
    }

    // --- Debugging ---

    pub fn registers(&self) -> &[u32] {
        &self.regs
    }

    pub fn textures(&self) -> &[u8] {
        &self.textures
    }

    fn trace_line(&self, line: impl FnOnce() -> String) {
        if let Some(t) = self.trace.borrow_mut().as_mut() {
            let _ = writeln!(t.out, "{}", line());
        }
    }

    /// A line in the trace from outside the card.
    pub fn trace_note(&self, line: &str) {
        self.trace_line(|| line.to_string());
        if let Some(t) = self.trace.borrow_mut().as_mut() {
            let _ = t.out.flush();
        }
    }

    /// For the debugger's status.
    pub fn describe(&self) -> serde_json::Value {
        let named = [
            regs::INTSTATUS,
            regs::INTMASK,
            regs::OBJECT_OFFSET,
            regs::PAGE_CTRL,
            regs::ISP_BASE,
            regs::PREC_BASE,
            regs::PACKMODE,
            regs::LSTRIDE,
            regs::SOFADDR,
            regs::XCLIP,
            regs::IEEEFP,
            regs::BILINEAR,
        ];
        let registers: serde_json::Map<String, serde_json::Value> =
            named.iter().map(|&r| (regs::name(r), format!("{:08X}", self.regs[r]).into())).collect();
        serde_json::json!({
            "chip": self.chip.name(),
            "bar0": format!("{:08X}", self.pci.bar0),
            "bar1": format!("{:08X}", self.pci.bar1),
            "irq": self.irq(),
            "renders": self.renders,
            "registers": registers,
        })
    }
}

impl State for PowerVr {
    fn save(&self, w: &mut Writer) {
        self.chip.save(w);
        // The memory first, for rewind's deltas.
        self.textures.save(w);
        self.regs.save(w);
        self.pci.save(w);
        self.renders.save(w);
        self.end_at.save(w);
    }

    fn load(&mut self, r: &mut Reader) -> Result<()> {
        let mut chip = Chip::default();
        chip.load(r)?;
        if chip != self.chip {
            return Err(StateError::Mismatch(format!(
                "its PowerVR card is a {}, this one a {}",
                chip.name().to_ascii_uppercase(),
                self.chip.name().to_ascii_uppercase()
            )));
        }
        self.textures.load(r)?;
        self.regs.load(r)?;
        if self.textures.len() != TEXTURE_MEMORY as usize || self.regs.len() != regs::COUNT {
            return Err(StateError::Mismatch("its PowerVR card's memory differs".into()));
        }
        self.pci.load(r)?;
        self.renders.load(r)?;
        self.end_at.load(r)?;
        Ok(())
    }
}
