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
pub mod regs;

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
    }

    /// The processor held, or started at `pc`: the microcode starts by
    /// reporting its version, which the library checks.
    pub fn run(&mut self, pc: Option<u32>) {
        self.running = pc.is_some();
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
        let mut at = 0;
        while at < self.fifo.len() {
            let word = self.fifo[at];
            let len = match cmd::length(word) {
                cmd::Length::Words(n) => n,
                cmd::Length::Counted { header, item } => match self.fifo.get(at + header - 1) {
                    Some(&count) => header + item * count as usize,
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
                d.tex_mask_u = words[3] & 0xFFFF;
                d.tex_mask_v = words[3] >> 16;
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
            0x3013 => d.fg = arg,
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
            // Words into memory: address, count, the words.
            cmd::op::MEM_WRITE => {
                for (k, word) in words[3..].iter().enumerate() {
                    let at = (words[1] as usize + 4 * k) % vram.len();
                    if at + 4 <= vram.len() {
                        vram[at..at + 4].copy_from_slice(&word.to_le_bytes());
                    }
                }
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
            _ => {}
        }
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
        let value = match reg {
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
