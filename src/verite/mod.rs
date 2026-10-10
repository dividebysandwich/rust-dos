//! A Rendition Vérité V1000 (`machine=svga_verite`): a VGA with a RISC
//! processor that runs microcode the program loads into its memory, which
//! takes commands from a FIFO and draws 3D with them. Rendition's DOS
//! library for it (RRedline, "Speedy3D") loads its microcode, starts the
//! processor through the BIOS, and writes commands to the FIFO through the
//! card's ports or by DMA from lists in the PC's memory.
//!
//! The card's memory is the Super VGA's: its BAR0 window is the VESA
//! linear frame buffer.
//!
//! The registers are those of the XFree86 driver for the card
//! (xf86-video-rendition's `commonregs.h`, `v1kregs.h`), at the I/O ports
//! BAR1 puts at E400h.

pub mod cmd;
pub mod draw;
pub mod ramdac;
pub mod regs;
pub mod risc;
pub mod twod;

use crate::savestate::{Reader, Result, State, Writer};
use std::cell::RefCell;
use std::io::Write;

/// Where the BIOS put the card's ports (BAR1), and how many.
pub const IO_BASE: u16 = 0xE400;
pub const IO_SIZE: u16 = 0x100;
/// The card's memory window (BAR0): 16 MB, the VESA linear frame buffer.
pub const MEMORY_WINDOW: u32 = 16 << 20;
/// The FIFO's depth.
pub const FIFO_SIZE: u8 = 0x1F;
/// The version the microcode reports.
const VERSION: u32 = 0x0502_0014;
/// The interrupt line the BIOS routed INTA# to.
pub const IRQ: u8 = 10;
/// Where the BIOS keeps the board's description (`board_data`), as
/// segment and offset: the end of its ROM.
pub const BOARD_DATA: (u16, u16) = (0xC000, 0x7F00);

/// The board's description, which the BIOS's AX=158Dh points Rendition's
/// library to: PCI vital product data (a read-only resource, tag 90h),
/// whose "ZC" keyword holds the ports' and the memory's base (their top
/// bytes), the interrupt line, the memory in 64 KB units, and fields the
/// library keeps (0 here); then the end tag.
pub fn board_data(memory: u32) -> Vec<u8> {
    let mut zc = vec![(IO_BASE >> 8) as u8, (crate::video::vbe::LFB_BASE >> 24) as u8, IRQ];
    zc.extend_from_slice(&((memory >> 16) as u16).to_le_bytes());
    zc.extend_from_slice(&[0; 2 + 2 + 4 + 2 + 1]);
    let mut vpd = vec![b'Z', b'C', zc.len() as u8];
    vpd.extend_from_slice(&zc);
    let mut data = vec![0x90];
    data.extend_from_slice(&(vpd.len() as u16).to_le_bytes());
    data.extend_from_slice(&vpd);
    data.extend_from_slice(&[0x79, 0]);
    data
}

/// CRTCSTATUS at time `t_ns` of a display with timing `crt`: the vertical
/// state in bits 23-22 (active, front porch, sync, back porch) with the
/// scanlines left in it in bits 21-11, and the horizontal state in bits
/// 10-9 (active, front porch, back porch, sync), as xf86-video-rendition's
/// `commonregs.h` lays them out. Drivers poll it to wait for the
/// vertical retrace.
pub fn crtc_status(crt: &crate::video::crt::CrtTiming, t_ns: u64) -> u32 {
    let (line, column) = crt.position(t_ns);
    let (vertical, end) = if line < crt.display {
        (0, crt.display)
    } else if line < crt.retrace_start {
        (1, crt.retrace_start)
    } else if line < crt.retrace_end {
        (3, crt.retrace_end)
    } else {
        (2, crt.total)
    };
    let left = end.saturating_sub(line).min(0x7FF);
    let blank = (crt.line_ns - crt.hdisplay_ns).max(1) as u64;
    let horizontal = match column.checked_sub(crt.hdisplay_ns as u64) {
        None => 0,
        Some(c) if c < blank / 3 => 1,
        Some(c) if c < blank * 2 / 3 => 3,
        Some(_) => 2,
    };
    vertical << 22 | left << 11 | horizontal << 9
}

#[derive(Clone, Debug)]
struct PciConfig {
    command: u16,
    bar1: u32,
    irq_line: u8,
}

impl Default for PciConfig {
    fn default() -> Self {
        Self { command: 0x0007, bar1: IO_BASE as u32, irq_line: IRQ }
    }
}

crate::state_fields!(PciConfig { command, bar1, irq_line });

/// The microcode the RISC runs, whose commands the FIFO carries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Microcode {
    /// Rendition's 3D library's, started through the BIOS.
    #[default]
    Speedy3d,
    /// The Windows driver's 2D microcode, started through the debug
    /// registers.
    TwoD,
}

impl State for Microcode {
    fn save(&self, w: &mut Writer) {
        (*self as u8).save(w);
    }

    fn load(&mut self, r: &mut Reader) -> Result<()> {
        let mut v = 0u8;
        v.load(r)?;
        *self = if v == 1 { Microcode::TwoD } else { Microcode::Speedy3d };
        Ok(())
    }
}

/// What the card's own CRTC shows: `width` x `height` pixels of `bpp`
/// bits (8 through the palette, 15, 16 or 32) from `base` in its memory,
/// `pitch` bytes a line, with `timing`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Screen {
    pub width: u16,
    pub height: u16,
    pub bpp: u8,
    pub base: u32,
    pub pitch: u32,
    pub timing: crate::video::crt::CrtTiming,
}

/// What commands did beyond the card.
#[derive(Default)]
pub struct Effects {
    /// The buffer shown from the next retrace on.
    pub display: Option<u32>,
    /// Whether the memory changed.
    pub drawn: bool,
}

pub struct Verite {
    pci: PciConfig,
    /// The I/O registers, a byte each.
    regs: Vec<u8>,
    /// The bytes of a FIFO word written a byte or a word at a time.
    partial: u32,
    partial_mask: u32,
    /// The FIFO's words, as the microcode reads them.
    pub fifo: Vec<u32>,
    /// What the microcode answers through the output FIFO.
    pub output: std::collections::VecDeque<u32>,
    /// Whether the processor runs the microcode (the BIOS started it).
    pub running: bool,
    /// The processor as the debug registers show it.
    pub risc: risc::Risc,
    /// Which microcode's commands the FIFO carries.
    pub microcode: Microcode,
    /// The RAMDAC.
    pub dac: ramdac::Bt485,
    /// The 2D microcode's state.
    pub twod: twod::Engine,
    /// Whether the card's own CRTC shows the screen (the Windows driver
    /// sets its modes through it), rather than the VGA.
    pub native: bool,
    /// The RAMDAC's palette as last loaded (256 0x00RRGGBB).
    pub palette: Vec<u32>,
    /// What the drawing commands draw with.
    pub draw: draw::DrawState,
    /// The commands seen, by opcode and vertex type, for the trace.
    seen: std::collections::BTreeMap<(u16, u16), u64>,
    /// Commands carried out, for the trace's tallies.
    executed: u64,
    /// Lines for the emulator's log.
    pub log: Vec<String>,
    /// `RUST_DOS_VERITE_TRACE`: every access and FIFO word, in
    /// `trace.txt` in that folder.
    trace: RefCell<Option<std::io::BufWriter<std::fs::File>>>,
}

impl Default for Verite {
    fn default() -> Self {
        Self::new()
    }
}

impl Verite {
    pub fn new() -> Self {
        let mut card = Self {
            pci: PciConfig::default(),
            regs: vec![0; IO_SIZE as usize],
            partial: 0,
            partial_mask: 0,
            fifo: Vec::new(),
            output: Default::default(),
            running: false,
            risc: Default::default(),
            microcode: Microcode::Speedy3d,
            dac: Default::default(),
            twod: Default::default(),
            native: false,
            palette: Vec::new(),
            draw: Default::default(),
            seen: Default::default(),
            executed: 0,
            log: Vec::new(),
            trace: RefCell::new(None),
        };
        if let Ok(dir) = std::env::var("RUST_DOS_VERITE_TRACE") {
            let dir = std::path::PathBuf::from(dir);
            match std::fs::create_dir_all(&dir).and_then(|_| std::fs::File::create(dir.join("trace.txt"))) {
                Ok(file) => *card.trace.get_mut() = Some(std::io::BufWriter::new(file)),
                Err(e) => card.log.push(format!("[VERITE] Can't trace into {}: {}", dir.display(), e)),
            }
        }
        card.reset();
        card
    }

    /// As after a reset.
    pub fn reset(&mut self) {
        self.regs.fill(0);
        self.partial_mask = 0;
        self.fifo.clear();
        self.output.clear();
        self.running = false;
        self.risc = Default::default();
        self.microcode = Microcode::Speedy3d;
        self.dac = Default::default();
        self.twod = Default::default();
        self.native = false;
    }

    /// The processor held, or started at `pc`: the microcode starts by
    /// reporting its version, which the library checks.
    pub fn run(&mut self, pc: Option<u32>) {
        self.running = pc.is_some();
        self.microcode = Microcode::Speedy3d;
        if let Some(pc) = pc {
            self.trace(|| format!("RISC start at {:08X}", pc));
            self.output.push_back(VERSION);
        }
    }

    /// Carry out the commands in the FIFO, as Rendition's microcode would,
    /// on the card's memory `vram`: each whole command, leaving a partial
    /// one for the words to come.
    pub fn execute(&mut self, vram: &mut [u8]) -> Effects {
        let mut fx = Effects::default();
        if !self.running {
            return fx;
        }
        if self.microcode == Microcode::TwoD {
            self.execute_2d(vram, &mut fx);
            return fx;
        }
        let mut at = 0;
        while at < self.fifo.len() {
            let word = self.fifo[at];
            let len = match cmd::length(word) {
                cmd::Length::Words(n) => n,
                cmd::Length::Counted { header, item } => match self.fifo.get(at + header - 1) {
                    Some(&count) => header + item * count as usize,
                    None => break,
                },
                cmd::Length::Rect => match (self.fifo.get(at + 3), self.fifo.get(at + 4)) {
                    (Some(&bytes), Some(&lines)) => cmd::rect_words(bytes, lines),
                    _ => break,
                },
                cmd::Length::Spans => {
                    let mut end = at + 5;
                    while self.fifo.get(end).is_some_and(|&w| w & 0x8000_0000 == 0) {
                        end += 6;
                    }
                    if end >= self.fifo.len() {
                        break;
                    }
                    end + 1 - at
                }
                cmd::Length::Write => match self.fifo.get(at + 2) {
                    Some(&bytes) => 3 + bytes.div_ceil(4) as usize,
                    None => break,
                },
                cmd::Length::Bytes => match self.fifo.get(at + 2) {
                    Some(&size) => cmd::bytes_words(size),
                    None => break,
                },
                cmd::Length::Unknown => {
                    // Lost: skip the word, as the stream can't be followed.
                    self.trace(|| format!("command {:08X} not known", word));
                    at += 1;
                    continue;
                }
            };
            if at + len > self.fifo.len() {
                break;
            }
            let command: Vec<u32> = self.fifo[at..at + len].to_vec();
            at += len;
            self.command(&command, vram, &mut fx);
        }
        self.fifo.drain(..at);
        fx
    }

    /// The 2D microcode's commands in the FIFO: the loader's four words
    /// first after the RISC was started, then whole commands, leaving a
    /// partial one for the words to come.
    fn execute_2d(&mut self, vram: &mut [u8], fx: &mut Effects) {
        let mut at = 0;
        while at < self.fifo.len() {
            if self.twod.loader {
                if self.fifo.len() - at < 4 {
                    break;
                }
                let words = format!("{:08X?}", &self.fifo[at..at + 4]);
                self.trace(|| format!("2D loader {}", words));
                self.twod.loader = false;
                at += 4;
                continue;
            }
            let len = match twod::length(&self.fifo[at..], self.twod.bpp()) {
                Some(Some(len)) => len,
                Some(None) => break,
                None => {
                    // Lost: the stream can't be followed past it.
                    let rest = format!("{:08X?}", &self.fifo[at..]);
                    self.trace(|| format!("2D command not known: {}", rest));
                    at = self.fifo.len();
                    break;
                }
            };
            let words: Vec<u32> = self.fifo[at..at + len].to_vec();
            at += len;
            *self.seen.entry((words[0] as u16, 0x2D)).or_default() += 1;
            self.trace(|| format!("2D {:08X?}", &words[..words.len().min(12)]));
            if self.twod.run(&words, vram, &mut self.output) {
                fx.drawn = true;
            } else {
                self.trace(|| format!("2D command {:08X} not carried out", words[0]));
            }
        }
        self.fifo.drain(..at);
    }

    /// One command.
    fn command(&mut self, words: &[u32], vram: &mut [u8], fx: &mut Effects) {
        let (opcode, vtype) = (words[0] as u16, (words[0] >> 16) as u16);
        *self.seen.entry((opcode, vtype)).or_default() += 1;
        self.executed += 1;
        if self.executed.is_multiple_of(10_000) {
            let seen = format!("{:X?}", self.seen);
            self.trace(|| format!("commands seen: {}", seen));
        }
        let d = &mut self.draw;
        let arg = words.get(1).copied().unwrap_or(0);
        match opcode {
            cmd::op::SYNC_AND_RESPOND => self.output.push_back(0),
            0x1004 => d.dst_base = arg,
            0x143B => d.dst_stride = draw::stride(arg),
            0x100E => d.width = arg,
            0x100F => d.height = arg,
            0x4000 => {
                d.tex_base = words[1];
                d.tex_stride = draw::stride(words[2]);
                d.tex_last_u = words[3] & 0xFFFF;
                d.tex_last_v = words[3] >> 16;
                d.scale_u = words[4];
                d.scale_v = words[5];
            }
            0x1231 => d.src_mode = arg,
            0x5028 => d.scale_u = arg,
            0x5029 => d.scale_v = arg,
            0x17BE => d.clamp_u = arg != 0,
            0x183F => d.clamp_v = arg != 0,
            0x1038 => d.max_u = arg,
            0x1839 => d.max_v = arg,
            0x15B5 => d.chroma = arg != 0,
            0x1017 => d.chroma_colour = arg,
            0x101A => d.chroma_mask = arg,
            0x89C6 => d.blend = arg != 0,
            0x1241 => d.blend_src = arg,
            0x1442 => d.blend_dst = arg,
            0x2055 => d.alpha = arg >> 16 & 0xFF,
            // The default red, green and blue, 16.16 of 0-255.
            0x2054 => d.fg = d.fg & !0xFF_0000 | (arg >> 16 & 0xFF) << 16,
            0x2056 => d.fg = d.fg & !0xFF00 | (arg >> 16 & 0xFF) << 8,
            0x2058 => d.fg = d.fg & !0xFF | arg >> 16 & 0xFF,
            0x3013 => d.fg = arg,
            0x1006 => d.dst_format = arg,
            0x1030 => d.src_format = arg,
            0x16B7 => d.src_bgr = arg != 0,
            // The 4-bit formats' 16 entries, two a word, the first high.
            0x7020 => {
                d.palette = words[1..9].iter().flat_map(|&w| [w >> 16, w & 0xFFFF]).collect();
            }
            0x13B2 => d.filter = arg != 0,
            0x602A => d.s_offset = arg,
            0x602B => d.t_offset = arg,
            0x1CCC => d.dst_read = arg == 0,
            0x1015 => d.dst_colour = arg,
            0x1010 => d.z_base = arg,
            0x183C => d.z_stride = draw::stride(arg),
            0x1643 => d.z_mode = arg & 7,
            0x1844 => d.z_write = arg != 0,
            0x205A => d.z = arg,
            0xAA47 => d.fog = arg != 0,
            0x1016 => d.fog_colour = arg,
            0x205B => d.fog_default = arg,
            cmd::op::TRIANGLE | cmd::op::TRIFAN | cmd::op::TRISTRIP => {
                if let Some(fields) = cmd::vertex_fields(vtype) {
                    let size = fields.len();
                    let (count, data) =
                        if opcode == cmd::op::TRIANGLE { (3, &words[1..]) } else { (arg as usize, &words[2..]) };
                    let vs: Vec<draw::Vertex> = (0..count).map(|k| draw::vertex(fields, &data[k * size..])).collect();
                    for k in 2..vs.len() {
                        let (a, b, c) = match opcode {
                            cmd::op::TRIFAN => (&vs[0], &vs[k - 1], &vs[k]),
                            _ if k % 2 == 1 => (&vs[k - 1], &vs[k - 2], &vs[k]),
                            _ => (&vs[k - 2], &vs[k - 1], &vs[k]),
                        };
                        d.triangle(vram, a, b, c);
                    }
                    fx.drawn = true;
                }
            }
            cmd::op::RECTANGLE if vtype == 1 => {
                d.rectangle(vram, words[1], words[2], words[3], words[4]);
                fx.drawn = true;
            }
            cmd::op::INTLINE => {
                d.int_line(vram, words[1], words[2]);
                fx.drawn = true;
            }
            cmd::op::BITBLT => {
                d.bitblt(vram, words[1], words[2], words[3]);
                fx.drawn = true;
            }
            // Bytes into memory: address, how many, the bytes. The RISC
            // stores a word's high half first as the host sees 16-bit
            // memory (vQuake sends its 16-bit colour tables half-swapped
            // by DMA, mode 3, for them to land in order).
            cmd::op::MEM_WRITE => {
                let bytes = words[3..].iter().flat_map(|w| w.rotate_left(16).to_le_bytes()).take(words[2] as usize);
                for (k, byte) in bytes.enumerate() {
                    let at = (words[1] as usize + k) % vram.len();
                    vram[at] = byte;
                }
                fx.drawn = true;
            }
            // A block into memory: base, bytes a line, bytes and lines,
            // then each line's bytes, padded to words.
            cmd::op::MEM_WRITE_RECT => {
                let (base, stride, bytes, lines) = (words[1], words[2], words[3] as usize, words[4]);
                let per_line = bytes.div_ceil(4);
                for line in 0..lines as usize {
                    let data = words[5 + line * per_line..][..per_line].iter().flat_map(|w| w.to_le_bytes());
                    let start = base.wrapping_add(line as u32 * stride) as usize;
                    for (i, byte) in data.take(bytes).enumerate() {
                        let at = (start + i) % vram.len();
                        vram[at] = byte;
                    }
                }
                fx.drawn = true;
            }
            cmd::op::PARTICLES => {
                for particle in words[2..].as_chunks::<4>().0 {
                    d.particle(vram, particle);
                }
                fx.drawn = true;
            }
            cmd::op::QSPAN => {
                d.spans(vram, &words[1..5], &words[5..words.len() - 1]);
                fx.drawn = true;
            }
            cmd::op::LOOKUP => {
                let bytes: Vec<u8> = words[3..].iter().flat_map(|w| w.to_le_bytes()).collect();
                d.lookup(vram, words[1], words[2], &bytes);
                fx.drawn = true;
            }
            cmd::op::DISPLAY => fx.display = Some(words[1]),
            cmd::op::PALETTE => self.palette = words[2..].to_vec(),
            // Memory filled: base, stride and width in bytes, lines, value.
            cmd::op::MEM_CLEAR_RECT => {
                let (base, stride, width, lines, value) = (words[1], words[2], words[3], words[4], words[5]);
                let bytes = value.to_le_bytes();
                for line in 0..lines {
                    let start = base.wrapping_add(line * stride) as usize;
                    for (i, byte) in vram.iter_mut().skip(start).take(width as usize).enumerate() {
                        *byte = bytes[i % 4];
                    }
                }
                fx.drawn = true;
            }
            // What isn't carried out yet, once, for the trace.
            _ => {
                if self.seen[&(opcode, vtype)] == 1 {
                    self.trace(|| format!("command {:08X} not carried out: {:08X?}", words[0], &words[1..]));
                }
            }
        }
    }

    /// The screen the CRTC shows, if its video is on (CRTCCTL bit 12)
    /// in a format this shows. The CRTC's counts are xf86-video-rendition's
    /// (`vmodes.c`): units of 8 pixels across, less one, and lines less
    /// one. Its line offset is what it adds after it fetched a line, which
    /// it fetches in units of its video FIFO's size (128 bytes with
    /// CRTCCTL bit 4, else 64) less one unit when the line fills them
    /// exactly and the base is a multiple of 8. The refresh rate comes
    /// from a clock this doesn't model: 60 Hz.
    pub fn screen(&self) -> Option<Screen> {
        let ctl = self.read_quiet(regs::CRTCCTL);
        if ctl & 0x1000 == 0 {
            return None;
        }
        let bpp = match ctl & 0xF {
            1 | 2 => 8,
            4 => 16,
            6 => 15,
            12 => 32,
            _ => return None,
        };
        let (horz, vert) = (self.read_quiet(regs::CRTCHORZ), self.read_quiet(regs::CRTCVERT));
        let width = ((horz & 0xFF) + 1) * 8;
        let h_back = ((horz >> 9 & 0x3F) + 1) * 8;
        let h_sync = ((horz >> 16 & 0x1F) + 1) * 8;
        let h_front = ((horz >> 21 & 0x7) + 1) * 8;
        let lines = (vert & 0x7FF) + 1;
        let v_back = (vert >> 11 & 0x3F) + 1;
        let v_sync = (vert >> 17 & 0x7) + 1;
        let v_front = (vert >> 20 & 0x3F) + 1;
        let total = lines + v_front + v_sync + v_back;
        let line_ns = 1_000_000_000 / (60 * total);
        let h_total = width + h_front + h_sync + h_back;
        let timing = crate::video::crt::CrtTiming {
            line_ns,
            hdisplay_ns: (line_ns as u64 * width as u64 / h_total as u64) as u32,
            total,
            display: lines,
            retrace_start: lines + v_front,
            retrace_end: lines + v_front + v_sync,
        };
        let base = self.read_quiet(regs::FRAMEBASEA) & 0x00FF_FFFF;
        let bytes = width * (bpp as u32).div_ceil(8);
        let fifo = if ctl & 0x10 != 0 { 128 } else { 64 };
        let fetched = if base & 7 == 0 { (bytes - 1) / fifo * fifo } else { bytes / fifo * fifo };
        let pitch = fetched + (self.read_quiet(regs::CRTCOFFSET) & 0xFFFF);
        let height = if ctl & 0x2_0000 != 0 { lines / 2 } else { lines };
        Some(Screen {
            width: width as u16,
            height: height as u16,
            bpp,
            base,
            pitch: if pitch < bytes { bytes } else { pitch },
            timing,
        })
    }

    pub fn trace(&self, line: impl FnOnce() -> String) {
        if let Some(out) = self.trace.borrow_mut().as_mut() {
            let _ = writeln!(out, "{}", line());
            let _ = out.flush();
        }
    }

    pub fn flush_trace(&self) {
        if let Some(out) = self.trace.borrow_mut().as_mut() {
            let _ = out.flush();
        }
    }

    /// For the debugger's status: whether the microcode runs, the commands
    /// seen (opcode/vertex type: count) and the drawing state.
    pub fn describe(&self) -> serde_json::Value {
        let seen: serde_json::Map<String, serde_json::Value> =
            self.seen.iter().map(|((op, vt), n)| (format!("{:04X}/{}", op, vt), (*n).into())).collect();
        serde_json::json!({
            "running": self.running,
            "fifo": self.fifo.len(),
            "output": self.output.len(),
            "commands": seen,
            "draw": format!("{:?}", self.draw),
        })
    }

    // --- PCI ---

    /// A byte of the configuration space: Rendition's V1000 (1163h:0001h),
    /// a VGA, BAR0 its memory at `lfb`, BAR1 its ports.
    pub fn config_read(&self, reg: u8, lfb: u32) -> u8 {
        let dword = match reg & 0xFC {
            0x00 => 0x0001_1163,
            0x04 => 0x0280_0000 | self.pci.command as u32,
            0x08 => 0x0300_0002,
            0x10 => lfb,
            0x14 => self.pci.bar1 | 1,
            0x3C => 0x0000_0100 | self.pci.irq_line as u32,
            _ => 0,
        };
        let value = (dword >> (8 * (reg & 3))) as u8;
        self.trace(|| format!("cfg r {:02X} = {:02X}", reg, value));
        value
    }

    pub fn config_write(&mut self, reg: u8, value: u8) {
        self.trace(|| format!("cfg w {:02X} = {:02X}", reg, value));
        match reg {
            0x04 => self.pci.command = (self.pci.command & 0xFF00) | (value & 0x07) as u16,
            0x3C => self.pci.irq_line = value,
            _ => {}
        }
    }

    /// The register at `port`, if it is one of the card's.
    pub fn port(&self, port: u16) -> Option<u8> {
        let base = (self.pci.bar1 & 0xFF00) as u16;
        (self.pci.command & 1 != 0 && port.wrapping_sub(base) < IO_SIZE).then(|| (port - base) as u8)
    }

    // --- Ports ---

    /// A register's dword, without the effects of a read.
    pub fn read_quiet(&self, reg: u8) -> u32 {
        (0..4).map(|i| (self.regs[(reg as usize + i) % 256] as u32) << (8 * i)).sum()
    }

    /// A register read, `len` bytes from `reg`.
    pub fn read(&mut self, reg: u8, len: u8) -> u32 {
        let state = (regs::STATEDATA..regs::STATEDATA + 4)
            .contains(&reg)
            .then(|| self.risc.state(self.regs[regs::STATEINDEX as usize]))
            .flatten();
        let value = match reg {
            // The RISC's state STATEINDEX selects.
            _ if let Some(state) = state => state >> (8 * (reg - regs::STATEDATA)),
            regs::FIFOINFREE => FIFO_SIZE as u32,
            regs::FIFOOUTVALID => self.output.len().min(FIFO_SIZE as usize) as u32,
            // The output FIFO, through the apertures.
            0x00..=0x0F => self.output.pop_front().unwrap_or(0),
            _ => (0..len).map(|i| (self.regs[(reg as usize + i as usize) % 256] as u32) << (8 * i)).sum(),
        };
        self.trace(|| format!("r {:<14} {} = {:0w$X}", regs::name(reg), len, value, w = 2 * len as usize));
        value
    }

    /// A register written, `len` bytes from `reg`.
    pub fn write(&mut self, reg: u8, value: u32, len: u8) {
        self.trace(|| format!("w {:<14} {} = {:0w$X}", regs::name(reg), len, value, w = 2 * len as usize));
        if reg < 0x10 {
            self.fifo_write(reg, value, len);
            return;
        }
        for i in 0..len {
            self.regs[(reg as usize + i as usize) % 256] = (value >> (8 * i)) as u8;
        }
        // STATEDATA written is an instruction for the RISC to be forced
        // through.
        if reg <= regs::STATEDATA + 3 && reg + len > regs::STATEDATA {
            self.risc.ir = self.read_quiet(regs::STATEDATA);
        }
    }

    /// DEBUGREG written: a soft reset, or the instruction register
    /// carried out once, which the step bit going back to 0 reports.
    pub fn debug(&mut self, vram: &mut [u8]) {
        let debug = self.regs[regs::DEBUGREG as usize];
        if debug & regs::SOFTRESET != 0 {
            self.risc.reset();
        }
        // Let go after it was held: the RISC runs what the driver loaded,
        // which takes the 2D microcode's commands.
        let held = debug & regs::HOLDRISC != 0;
        if self.risc.held && !held {
            self.trace(|| format!("RISC runs from {:08X}", self.risc.pc));
            self.running = true;
            self.microcode = Microcode::TwoD;
            self.twod.loader = true;
        }
        self.risc.held = held;
        if debug & regs::STEPRISC != 0 {
            let ir = self.risc.ir;
            if !self.risc.step(vram) {
                self.trace(|| format!("RISC forced {:08X} not known", ir));
            }
            self.trace(|| format!("RISC step {:08X}, PC {:08X}", ir, self.risc.pc));
            self.regs[regs::DEBUGREG as usize] &= !regs::STEPRISC;
        }
    }

    /// A FIFO aperture written: bits 3-2 of the port say how its bytes are
    /// swapped, bits 1-0 which of the word's bytes this is.
    fn fifo_write(&mut self, reg: u8, value: u32, len: u8) {
        let shift = 8 * (reg & 3) as u32;
        let mask = if len >= 4 { u32::MAX } else { ((1u32 << (8 * len)) - 1) << shift };
        self.partial = self.partial & !mask | (value << shift) & mask;
        self.partial_mask |= mask;
        if self.partial_mask != u32::MAX {
            return;
        }
        let word = swap(self.partial, reg >> 2 & 3);
        self.partial_mask = 0;
        self.trace(|| format!("FIFO {:08X}", word));
        self.fifo.push(word);
    }
}

/// A word with its bytes swapped as aperture or DMA mode `mode` does:
/// none, all four, within each half, or the halves.
pub fn swap(word: u32, mode: u8) -> u32 {
    match mode {
        0 => word,
        1 => word.swap_bytes(),
        2 => (word & 0xFF00_FF00) >> 8 | (word & 0x00FF_00FF) << 8,
        _ => word.rotate_left(16),
    }
}

impl State for Verite {
    fn save(&self, w: &mut Writer) {
        self.pci.save(w);
        self.regs.save(w);
        self.partial.save(w);
        self.partial_mask.save(w);
        self.fifo.save(w);
        let output: Vec<u32> = self.output.iter().copied().collect();
        output.save(w);
        self.running.save(w);
        self.draw.save(w);
        self.risc.save(w);
        self.microcode.save(w);
        self.dac.save(w);
        self.native.save(w);
        self.twod.save(w);
    }

    fn load(&mut self, r: &mut Reader) -> Result<()> {
        self.pci.load(r)?;
        self.regs.load(r)?;
        self.regs.resize(IO_SIZE as usize, 0);
        self.partial.load(r)?;
        self.partial_mask.load(r)?;
        self.fifo.load(r)?;
        let mut output: Vec<u32> = Vec::new();
        output.load(r)?;
        self.output = output.into();
        self.running.load(r)?;
        self.draw.load(r)?;
        // States from before the RISC's debug registers have none.
        self.risc = Default::default();
        self.microcode = Microcode::Speedy3d;
        self.dac = Default::default();
        self.native = false;
        self.twod = Default::default();
        if !r.is_empty() {
            self.risc.load(r)?;
            self.microcode.load(r)?;
            self.dac.load(r)?;
            self.native.load(r)?;
            self.twod.load(r)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apertures_swap_bytes() {
        assert_eq!(swap(0x1122_3344, 0), 0x1122_3344);
        assert_eq!(swap(0x1122_3344, 1), 0x4433_2211);
        assert_eq!(swap(0x1122_3344, 2), 0x2211_4433);
        assert_eq!(swap(0x1122_3344, 3), 0x3344_1122);
    }

    #[test]
    fn a_word_written_a_byte_at_a_time_enters_the_fifo_whole() {
        let mut v = Verite::new();
        for (i, b) in [0x44, 0x33, 0x22, 0x11].into_iter().enumerate() {
            v.write(i as u8, b, 1);
        }
        assert_eq!(v.fifo, [0x1122_3344]);
        v.write(0x04, 0x1122_3344, 4);
        assert_eq!(v.fifo[1], 0x4433_2211);
    }
}
