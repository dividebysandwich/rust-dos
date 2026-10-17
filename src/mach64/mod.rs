//! An ATI 3D RAGE (`machine=svga_ati_3drage`), the mach64 GT, or the 3D
//! RAGE II (`svga_ati_3drage2`), the GT-B: a VGA with ATI's accelerator
//! CRTC, hardware cursor, 2D draw engine and a 3D engine that draws
//! trapezoids, which ATI's Windows drivers (their DirectDraw, Direct3D
//! HAL and ATI3DCIF) and its BIOS's own functions drive.
//!
//! The card's memory is the Super VGA's: the little-endian half of its
//! 16 MB aperture (BAR0) is the VESA linear frame buffer, with the
//! registers in the top 2 KB of it; the other half is the same memory
//! again (big-endian on a real card), its last page the registers again.
//! The registers below 40h are also ports: 256 at BAR1 (block I/O), or
//! the "sparse" ones every 400h from 2ECh up.
//!
//! What the registers do is from ATI's register reference for the 264VT
//! and the 3D RAGE (RRG-G02700); where it says too little, from what
//! ATI's drivers were seen to need (LeonG-ZA's DOSBox-X work found most
//! of that).

pub mod bios;
pub mod engine;
pub mod overlay;
pub mod regs;
pub mod tri;

use crate::savestate::{Reader, Result, State, Writer};
use std::cell::RefCell;
use std::io::Write;

/// Where the BIOS put the card's ports (BAR1), and how many.
pub const IO_BASE: u16 = 0xD800;
pub const IO_SIZE: u16 = 0x100;
/// The aperture (BAR0): 16 MB, aligned to it.
pub const APERTURE: u32 = 16 << 20;
/// Where in each half of the aperture the registers are: block 1, then
/// block 0, 1 KB each.
pub const REGISTERS: u32 = 0x7F_F800;
/// The video memory.
pub const MEMORY: u32 = 4 << 20;

/// The sparse ports' registers, by bits 14-10 of the port.
const SPARSE: [u8; 32] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x10, 0x11, 0x12, 0x18, 0x19, 0x1A, 0x1B, 0x1C,
    0x20, 0x21, 0x24, 0x28, 0x2C, 0x2D, 0x2E, 0x30, 0x31, 0x34, 0x37, 0x38, 0x39, 0x3A, 0x1E, 0x00,
];

/// The PLL's registers after a reset ([RRG] appendix B).
const PLL_RESET: [u8; 16] = [
    0x00, 0xD4, 0x36, 0x4F, 0x97, 0x04, 0x6A, 0xBE, 0xD6, 0xEE, 0x88, 0x00, 0x41, 0x00, 0x00, 0x00,
];

/// The PLL's reference: a 14.31818 MHz crystal.
const XTAL: u64 = 14_318_180;

/// What the card is doing when a register is read or written: the CRTC's
/// scanline, whether it is in the vertical blank, and how many vertical
/// retraces there have been.
#[derive(Clone, Copy, Debug, Default)]
pub struct Live {
    pub line: u32,
    pub vblank: bool,
    pub retraces: u64,
}

/// What the card draws into and with: its memory, and the VGA DAC's
/// palette (3 bytes an entry, 6 or 8 bits), which 8-bit texels go
/// through.
pub struct Target<'a> {
    pub vram: &'a mut [u8],
    pub dac: &'a [u8],
    pub dac_8bit: bool,
}

/// What a register write did beyond the register.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Effect {
    /// The picture's format, size, timing or start may have changed.
    pub display: bool,
    /// The engine drew into the memory.
    pub drawn: bool,
    /// The hardware cursor moved or changed.
    pub cursor: bool,
}

#[derive(Clone, Debug)]
struct PciConfig {
    command: u16,
    bar0: u32,
    bar1: u32,
    rom: u32,
    subsystem: u32,
    irq_line: u8,
    /// Register 40h: bit 2 block I/O at BAR1, else the sparse ports at
    /// the base bits 1-0 pick.
    io_decode: u8,
}

impl Default for PciConfig {
    fn default() -> Self {
        Self {
            command: 0x0003,
            bar0: crate::video::vbe::LFB_BASE as u32,
            bar1: IO_BASE as u32,
            rom: 0,
            subsystem: 0,
            irq_line: 0,
            io_decode: 0x04,
        }
    }
}

crate::state_fields!(PciConfig {
    command,
    bar0,
    bar1,
    rom,
    subsystem,
    irq_line,
    io_decode
});

/// What the accelerator CRTC shows: `width` x `height` pixels of `bpp`
/// bits (8 through the palette, 15, 16, 24 or 32) from `base` in the
/// memory, `pitch` bytes a line, with `timing`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Screen {
    pub width: u16,
    pub height: u16,
    pub bpp: u8,
    pub base: u32,
    pub pitch: u32,
    pub timing: crate::video::crt::CrtTiming,
}

/// A pixel of the hardware cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorPixel {
    Transparent,
    /// CUR_CLR0 or CUR_CLR1.
    Colour(u8),
    Invert,
}

/// The hardware cursor, where it is: `x`, `y` on the screen of its first
/// shown pixel; `skip` pixels of each row left out on the left, and
/// `rows` rows shown from its map at `base`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub x: i32,
    pub y: i32,
    pub skip: u32,
    pub rows: u32,
    pub base: u32,
}

impl Cursor {
    /// Pixel `cx` (0-63) of row `cy` of the map in `vram`: two bits each,
    /// four to a byte from the low bits up, 16 bytes a row.
    pub fn pixel(&self, vram: &[u8], cx: u32, cy: u32) -> CursorPixel {
        let at = (self.base + cy * 16 + cx / 4) as usize % vram.len();
        match vram[at] >> (2 * (cx & 3)) & 3 {
            0 => CursorPixel::Colour(0),
            1 => CursorPixel::Colour(1),
            2 => CursorPixel::Transparent,
            _ => CursorPixel::Invert,
        }
    }
}

pub struct Mach64 {
    /// A 3D RAGE II (GT-B) rather than a 3D RAGE (GT).
    pub rage2: bool,
    pci: PciConfig,
    /// The registers as they read back, block 0 then block 1; the parts
    /// of composite registers are kept, not the composites.
    regs: Vec<u32>,
    /// The PLL's registers, which CLOCK_CNTL reaches.
    pll: [u8; 16],
    /// The vertical retraces counted when the program last acknowledged
    /// one in CRTC_INT_CNTL.
    retraces_acked: u64,
    /// The draw engine's operation in progress, and the texture palette.
    pub engine: engine::Engine,
    /// Whether the accelerator CRTC shows the screen rather than the VGA
    /// (`Bus::mach64_settle`).
    pub native: bool,
    /// Lines for the emulator's log.
    pub log: Vec<String>,
    /// `RUST_DOS_MACH64_TRACE`: every register access, in `trace.txt` in
    /// that folder.
    trace: RefCell<Option<std::io::BufWriter<std::fs::File>>>,
}

impl Default for Mach64 {
    fn default() -> Self {
        Self::new(false)
    }
}

impl Mach64 {
    pub fn new(rage2: bool) -> Self {
        let mut card = Self {
            rage2,
            pci: PciConfig::default(),
            regs: vec![0; regs::COUNT],
            pll: PLL_RESET,
            retraces_acked: 0,
            engine: Default::default(),
            native: false,
            log: Vec::new(),
            trace: RefCell::new(None),
        };
        if let Ok(dir) = std::env::var("RUST_DOS_MACH64_TRACE") {
            let dir = std::path::PathBuf::from(dir);
            match std::fs::create_dir_all(&dir)
                .and_then(|_| std::fs::File::create(dir.join("trace.txt")))
            {
                Ok(file) => *card.trace.get_mut() = Some(std::io::BufWriter::new(file)),
                Err(e) => card.log.push(format!(
                    "[MACH64] Can't trace into {}: {}",
                    dir.display(),
                    e
                )),
            }
        }
        card.reset();
        card
    }

    /// As after a reset: the VGA shows the screen, the engine is idle with
    /// a screen-sized scissor, the memory size is in MEM_CNTL.
    pub fn reset(&mut self) {
        self.pci = PciConfig::default();
        self.regs.fill(0);
        self.pll = PLL_RESET;
        self.engine = Default::default();
        self.native = false;
        use regs::*;
        let size_code = match MEMORY {
            m if m <= 512 << 10 => 0,
            m if m <= 1 << 20 => 1,
            m if m <= 2 << 20 => 2,
            _ => 3,
        };
        self.regs[MEM_CNTL] = size_code | 3 << 11;
        self.regs[BUS_CNTL] = 0x0800_00FF;
        self.regs[GEN_TEST_CNTL] = 0x100;
        self.regs[CONFIG_STAT0] = 0x12;
        self.regs[DST_CNTL] = 0x3;
        self.regs[DP_WRITE_MSK] = 0xFFFF_FFFF;
        self.regs[SC_RIGHT] = 0x0FFF;
        self.regs[SC_BOTTOM] = 0x3FFF;
    }

    /// CONFIG_CHIP_ID: the ASIC revision over the type, "GT".
    pub fn chip_id(&self) -> u32 {
        if self.rage2 { 0x9A00_4754 } else { 0x4000_4754 }
    }

    // --- PCI ---

    /// Where the linear aperture is, if the card decodes memory.
    pub fn aperture(&self) -> Option<u32> {
        (self.pci.command & 2 != 0 && self.pci.bar0 != 0).then_some(self.pci.bar0)
    }

    /// A byte of the configuration space: ATI's mach64 GT (1002h:4754h),
    /// a VGA (or another display controller with CFG_VGA_DIS), BAR0 the
    /// aperture, BAR1 the block I/O ports. Its interrupt is disabled by
    /// a strap.
    pub fn config_read(&self, reg: u8) -> u8 {
        let subclass = if self.regs[regs::CONFIG_CNTL] & 1 << 19 != 0 {
            0x80
        } else {
            0
        };
        let dword = match reg & 0xFC {
            0x00 => 0x4754_1002,
            0x04 => 0x0280_0000 | self.pci.command as u32,
            0x08 => 0x0300_0000 | subclass << 16 | self.chip_id() >> 24,
            0x10 => self.pci.bar0,
            0x14 => self.pci.bar1 & 0xFFFF_FF00 | 1,
            0x2C => self.pci.subsystem,
            0x30 => self.pci.rom,
            0x3C => self.pci.irq_line as u32,
            0x40 => self.pci.io_decode as u32,
            _ => 0,
        };
        let value = (dword >> (8 * (reg & 3))) as u8;
        self.trace(|| format!("cfg r {:02X} = {:02X}", reg, value));
        value
    }

    /// A configuration byte written. True if the aperture or the ports
    /// moved.
    pub fn config_write(&mut self, reg: u8, value: u8) -> bool {
        self.trace(|| format!("cfg w {:02X} = {:02X}", reg, value));
        let set = |dword: &mut u32, value: u8| {
            let shift = 8 * (reg & 3);
            *dword = *dword & !(0xFF << shift) | (value as u32) << shift;
        };
        match reg {
            0x04 => {
                self.pci.command = (self.pci.command & 0xFF00) | (value & 0x03) as u16;
                return true;
            }
            // 16 MB aligned.
            0x13 => {
                set(&mut self.pci.bar0, value);
                return true;
            }
            // 256 ports.
            0x15..=0x17 => {
                set(&mut self.pci.bar1, value);
                return true;
            }
            0x2C..=0x2F => set(&mut self.pci.subsystem, value),
            0x30 => set(&mut self.pci.rom, value & 1),
            0x31 => set(&mut self.pci.rom, value & 0x80),
            0x32 | 0x33 => set(&mut self.pci.rom, value),
            0x3C => self.pci.irq_line = value,
            0x40 => {
                self.pci.io_decode = value & 7;
                return true;
            }
            _ => {}
        }
        false
    }

    /// Whether the ports are the block at BAR1, rather than the sparse ones.
    pub fn block_io(&self) -> bool {
        self.pci.io_decode & 4 != 0
    }

    /// The first port: BAR1's, or the sparse ports' base.
    pub fn io_base(&self) -> u16 {
        if self.block_io() {
            (self.pci.bar1 & 0xFF00) as u16
        } else {
            [0x2EC, 0x1CC, 0x1C8, 0x2EC][(self.pci.io_decode & 3) as usize]
        }
    }

    /// The register (and byte of it) port `port` reaches, if it is one of
    /// the card's: block I/O at BAR1, or the sparse ports.
    pub fn port(&self, port: u16) -> Option<(usize, u8)> {
        if self.pci.command & 1 == 0 {
            return None;
        }
        if self.pci.io_decode & 4 != 0 {
            let base = (self.pci.bar1 & 0xFF00) as u16;
            let offset = port.wrapping_sub(base);
            return (offset < IO_SIZE).then_some(((offset >> 2) as usize, (offset & 3) as u8));
        }
        let base = [0x2EC, 0x1CC, 0x1C8, 0x2EC][(self.pci.io_decode & 3) as usize];
        (port & 0x3FC == base && port < 0x8000)
            .then(|| (SPARSE[(port >> 10) as usize] as usize, (port & 3) as u8))
    }

    /// The register at offset `offset` of the 2 KB register area: block 1
    /// in the first KB (while BUS_CNTL bit 27 decodes it), block 0 in the
    /// second.
    pub fn register_at(&self, offset: u32) -> Option<usize> {
        match offset {
            0x400..0x800 => Some(((offset - 0x400) >> 2) as usize),
            0..0x400 if self.regs[regs::BUS_CNTL] & 1 << 27 != 0 => {
                Some(0x100 + (offset >> 2) as usize)
            }
            _ => None,
        }
    }

    /// Whether the registers are also at B000:F800h (block 1) and
    /// B000:FC00h (block 0) (CONFIG_CNTL bit 2).
    pub fn vga_window(&self) -> bool {
        self.regs[regs::CONFIG_CNTL] & 4 != 0
    }

    // --- Registers ---

    /// A register as the program reads it, without what reading does.
    pub fn read(&self, reg: usize, live: Live) -> u32 {
        use regs::*;
        let r = |i: usize| self.regs[i];
        let value = match reg {
            _ if let Some(fields) = fields(reg, self.rage2) => fields
                .iter()
                .map(|&(part, shift, mask)| (r(part) & mask) << shift)
                .sum(),
            CRTC_VLINE_CRNT_VLINE => r(reg) & 0x7FF | (live.line & 0x7FF) << 16,
            CRTC_INT_CNTL => {
                let latched = live.retraces > self.retraces_acked;
                r(reg) & !0x25 | live.vblank as u32 | (latched as u32) << 2 | (live.line & 1) << 5
            }
            CLOCK_CNTL => r(reg) | (self.pll[(r(reg) >> 10 & 0xF) as usize] as u32) << 16,
            DAC_CNTL => r(reg) | 1 << 16,
            CONFIG_CNTL => r(reg) | 2 | (self.pci.bar0 >> 22 & 0x3FF) << 4,
            CONFIG_CHIP_ID => self.chip_id(),
            LEAD_BRES_LNTH => r(DST_BRES_LNTH),
            GUI_STAT => self.gui_stat(),
            HOST_DATA0..=HOST_DATA15 | FIFO_STAT => 0,
            _ => r(reg),
        };
        self.trace(|| format!("r {:<22} = {:08X}", regs::name(reg), value));
        value
    }

    /// GUI_STAT: the engine busy (waiting for host data), where DST_X and
    /// DST_Y are against the scissors, and the FIFO's free entries.
    fn gui_stat(&self) -> u32 {
        let free: u32 = if self.rage2 { 64 } else { 32 };
        let (x, y) = (self.signed(regs::DST_X, 13), self.signed(regs::DST_Y, 15));
        let outside = [
            x < self.signed(regs::SC_LEFT, 13),
            x > self.signed(regs::SC_RIGHT, 13),
            y < self.signed(regs::SC_TOP, 15),
            y > self.signed(regs::SC_BOTTOM, 15),
        ];
        let clip: u32 = outside
            .iter()
            .enumerate()
            .map(|(i, &o)| (o as u32) << (8 + i))
            .sum();
        free << 16 | clip | self.engine.busy() as u32
    }

    /// Register `reg`'s low `bits` bits, sign-extended.
    pub fn signed(&self, reg: usize, bits: u32) -> i32 {
        ((self.regs[reg] << (32 - bits)) as i32) >> (32 - bits)
    }

    /// A register's value as kept.
    pub fn reg(&self, reg: usize) -> u32 {
        self.regs[reg]
    }

    /// Set a register's value as kept (the engine's results, the BIOS).
    pub fn set_reg(&mut self, reg: usize, value: u32) {
        self.regs[reg] = value & regs::write_mask(reg);
    }

    /// Write the bytes of `value` that `mask` selects into register `reg`,
    /// and carry out what that starts in `target`. DAC_REGS and DAC_CNTL
    /// are the bus's.
    pub fn write(
        &mut self,
        reg: usize,
        value: u32,
        mask: u32,
        live: Live,
        target: &mut Target,
    ) -> Effect {
        use regs::*;
        self.trace(|| format!("w {:<22} = {:08X} & {:08X}", regs::name(reg), value, mask));
        let mut fx = Effect::default();
        let old = self.regs[reg];
        // An operation starts when the write reaches its top byte.
        let top = mask & 0xFF00_0000 != 0;
        let value = value & mask;
        match reg {
            CRTC_INT_CNTL => {
                if value & 4 != 0 {
                    self.retraces_acked = live.retraces;
                }
                let kept = 0x0055_000A & mask;
                self.regs[reg] = self.regs[reg] & !kept | value & kept;
                self.regs[reg] &= !(value & 0x00AA_0010);
                return fx;
            }
            CLOCK_CNTL => {
                self.store(reg, value, mask);
                let new = self.regs[reg];
                if new & 1 << 9 != 0 && mask & 0x00FF_0000 != 0 {
                    let index = (new >> 10 & 0xF) as usize;
                    let mut data = (value >> 16) as u8;
                    // TEST_COUNT reads 0; TEST_CNTL only holds in test mode Bh.
                    if index == 15 || index == 14 && self.regs[GEN_TEST_CNTL] >> 16 & 0xF != 0xB {
                        data = 0;
                    }
                    fx.display = self.pll[index] != data;
                    self.pll[index] = data;
                }
                fx.display |= (old ^ new) & 3 != 0;
                return fx;
            }
            GEN_TEST_CNTL => {
                self.store(reg, value, mask);
                // GUI_EN going off resets the engine.
                if old & 0x100 != 0 && self.regs[reg] & 0x100 == 0 {
                    self.engine.reset();
                }
                fx.cursor = (old ^ self.regs[reg]) & 0x80 != 0;
                return fx;
            }
            FIFO_STAT | GUI_STAT | CONFIG_CHIP_ID | CRC_SIG => return fx,
            _ => {}
        }
        // Registers the 3D RAGE II added: on the GT nothing is there.
        if !self.rage2
            && matches!(
                reg,
                DST_X_Y | DST_WIDTH_HEIGHT | DP_SET_GUI_ENGINE | TEX_PALETTE
            )
        {
            return fx;
        }
        let op = match reg {
            HOST_DATA0..=HOST_DATA15 => {
                let (data, bits) = match mask {
                    0xFF => (value, 8),
                    0xFF00 => (value >> 8, 8),
                    0xFF_0000 => (value >> 16, 8),
                    0xFF00_0000 => (value >> 24, 8),
                    0xFFFF => (value, 16),
                    0xFFFF_0000 => (value >> 16, 16),
                    _ => (value, 32),
                };
                fx.drawn = self.host_data(data, bits, target);
                return fx;
            }
            _ if let Some(fields) = fields(reg, self.rage2) => {
                for &(part, shift, part_mask) in fields {
                    let m = (mask >> shift) & part_mask;
                    self.regs[part] = self.regs[part] & !m | (value >> shift) & m;
                }
                match reg {
                    // The width doubles as a line's length.
                    DST_HEIGHT_WIDTH | DST_X_WIDTH => {
                        self.store(DST_BRES_LNTH, value >> 16 & 0x1FFF, mask >> 16 & 0x1FFF);
                        top.then_some(engine::Op::Fill)
                    }
                    DST_WIDTH_HEIGHT => {
                        self.store(DST_BRES_LNTH, value & 0x1FFF, mask & 0x1FFF);
                        top.then_some(engine::Op::Fill)
                    }
                    _ => None,
                }
            }
            // Bit 15 is DRAW_TRAP: a line (or trapezoid) instead of a
            // rectangle; bit 31 holds the operation back.
            DST_WIDTH => {
                self.store(DST_WIDTH, value, mask & 0x7FFF);
                self.store(DST_BRES_LNTH, value, mask & 0xFFFF);
                match (top, value & 1 << 31 != 0, value & 0x8000 != 0) {
                    (true, false, true) => Some(engine::Op::Line),
                    (true, false, false) => Some(engine::Op::Fill),
                    _ => None,
                }
            }
            // Bits 31 and 15 (dis, trap): 00 a line, 01 a trapezoid with
            // its trailing X kept, 10 only loads, 11 a trapezoid.
            DST_BRES_LNTH | LEAD_BRES_LNTH => {
                let (dis, trap) = (value & 1 << 31 != 0, value & 0x8000 != 0);
                self.store(DST_BRES_LNTH, value, mask & 0xFFFF);
                self.store(DST_WIDTH, value, mask & 0x7FFF);
                if dis || !trap {
                    self.store(DST_HEIGHT, value >> 16 & 0x1FFF, mask >> 16 & 0x1FFF);
                }
                (top && (trap || !dis)).then_some(engine::Op::Line)
            }
            CONTEXT_LOAD_CNTL => {
                self.store(reg, value, mask & 0x8003_FFFF);
                top.then_some(engine::Op::Context)
            }
            TEX_PALETTE => {
                self.store(reg, value, mask);
                if top {
                    let v = self.regs[reg];
                    self.engine.palette[(v >> 24) as usize] = v & 0xFF_FFFF;
                }
                None
            }
            DP_SET_GUI_ENGINE => {
                self.store(reg, value, mask);
                top.then_some(engine::Op::SetGuiEngine)
            }
            _ => {
                let canonical = alias(reg);
                self.store(canonical, value, mask);
                fx.display = matches!(
                    canonical,
                    CRTC_H_TOTAL_DISP..=CRTC_V_SYNC_STRT_WID | CRTC_OFF_PITCH | CRTC_GEN_CNTL
                ) && self.regs[canonical] != old;
                fx.cursor = matches!(canonical, CUR_CLR0..=CUR_HORZ_VERT_OFF);
                fx.display |= canonical >= 0x100;
                None
            }
        };
        if let Some(op) = op {
            fx.drawn = self.run(op, target);
        }
        fx
    }

    /// A whole register written as a program writes it, by the engine
    /// (a context) or the BIOS: what it starts is carried out.
    pub fn load_reg(&mut self, reg: usize, value: u32, target: &mut Target) -> Effect {
        self.write(reg, value, u32::MAX, Live::default(), target)
    }

    /// Keep the bits of `value` that `mask` and the register's own mask
    /// select.
    fn store(&mut self, reg: usize, value: u32, mask: u32) {
        let m = mask & regs::write_mask(reg);
        self.regs[reg] = self.regs[reg] & !m | value & m;
    }

    /// Carry out an operation the registers started.
    fn run(&mut self, op: engine::Op, target: &mut Target) -> bool {
        let mut engine = std::mem::take(&mut self.engine);
        let drawn = engine.start(op, self, target);
        self.engine = engine;
        drawn
    }

    /// Data written to HOST_DATA: `bits` of it.
    fn host_data(&mut self, data: u32, bits: u32, target: &mut Target) -> bool {
        let mut engine = std::mem::take(&mut self.engine);
        let drawn = engine.host_data(data, bits, self, target);
        self.engine = engine;
        drawn
    }

    // --- Display ---

    /// The frequency of the pixel clock CLOCK_CNTL selects, in Hz: one of
    /// the PLL's four VCLKs, `2 * XTAL * N / (M * 2^post)`, or whatever
    /// VCLK_CNTL picks instead of the PLL.
    pub fn pixel_clock(&self) -> u64 {
        let select = (self.regs[regs::CLOCK_CNTL] & 3) as usize;
        let clock = match self.pll[5] & 3 {
            0 => 33_333_333,
            1 => 25_175_000,
            2 => XTAL,
            _ => {
                let (m, n) = (self.pll[2] as u64, self.pll[7 + select] as u64);
                let post = self.pll[6] >> (2 * select) & 3;
                if m < 2 || n < 2 {
                    25_175_000
                } else {
                    2 * XTAL * n / (m << post)
                }
            }
        };
        if clock < 1_000_000 { 25_175_000 } else { clock }
    }

    /// Program VCLK 3, the clock the BIOS uses, for `hz`: the reference
    /// divided by 36h, the post divider that puts the feedback divider in
    /// its range (80h-FFh), and the PLL as the clock source.
    pub fn program_clock(&mut self, hz: u64) {
        const M: u64 = 0x36;
        // The feedback divider without a post divider, doubled.
        let q2 = hz * M / XTAL;
        let post = match q2 {
            255.. => 0,
            127..=254 => 1,
            63..=126 => 2,
            _ => 3,
        };
        let n = (((hz * M) << post) / XTAL).div_ceil(2).clamp(0x80, 0xFF);
        self.pll[2] = M as u8;
        self.pll[6] = self.pll[6] & 0x3F | (post as u8) << 6;
        self.pll[10] = n as u8;
        self.pll[5] = self.pll[5] & !3 | 3;
    }

    /// The screen the accelerator CRTC shows, if CRTC_GEN_CNTL has it on
    /// (EXT_DISP_EN and CRTC_ENABLE) in a format this shows. The
    /// horizontal counts are in characters of 8 pixels less one, the
    /// vertical ones in scanlines less one; double scan shows each row
    /// twice.
    pub fn screen(&self) -> Option<Screen> {
        use regs::*;
        let gen_cntl = self.regs[CRTC_GEN_CNTL];
        if gen_cntl & 0x0300_0000 != 0x0300_0000 {
            return None;
        }
        let bpp = match gen_cntl >> 8 & 7 {
            2 => 8,
            3 => 15,
            4 => 16,
            5 => 24,
            6 => 32,
            _ => return None,
        };
        let h = self.regs[CRTC_H_TOTAL_DISP];
        let (v, v_sync) = (
            self.regs[CRTC_V_TOTAL_DISP],
            self.regs[CRTC_V_SYNC_STRT_WID],
        );
        let width = ((h >> 16 & 0xFF) + 1) * 8;
        let h_total = (((h & 0x1FF) + 1) * 8).max(width + 8);
        let lines = (v >> 16 & 0x7FF) + 1;
        let total = ((v & 0x7FF) + 1).max(lines + 1);
        let sync_start = ((v_sync & 0x7FF) + 1).clamp(lines, total - 1);
        let sync_end = (sync_start + (v_sync >> 16 & 0x1F).max(1)).min(total);
        let clock = self.pixel_clock();
        let ns = |pixels: u32| (pixels as u64 * 1_000_000_000 / clock) as u32;
        let timing = crate::video::crt::CrtTiming {
            line_ns: ns(h_total),
            hdisplay_ns: ns(width),
            total,
            display: lines,
            retrace_start: sync_start,
            retrace_end: sync_end,
        };
        let bytes = (bpp as u32).div_ceil(8);
        let pitch = (self.regs[CRTC_OFF_PITCH] >> 22 & 0x3FF) * 8 * bytes;
        let height = if gen_cntl & 1 != 0 { lines / 2 } else { lines };
        Some(Screen {
            width: width as u16,
            height: height.max(1) as u16,
            bpp,
            base: (self.regs[CRTC_OFF_PITCH] & 0xF_FFFF) * 8,
            pitch: pitch.max(width * bytes),
            timing,
        })
    }

    /// The hardware cursor, if GEN_TEST_CNTL bit 7 shows it. Its offsets
    /// leave out the first pixels of each row and shorten it from below
    /// (ATI's Windows driver keeps a 32x32 pointer at the right of rows
    /// 0-31 of the map, with both offsets 32).
    pub fn cursor(&self) -> Option<Cursor> {
        use regs::*;
        if self.regs[GEN_TEST_CNTL] & 0x80 == 0 {
            return None;
        }
        let (posn, off) = (self.regs[CUR_HORZ_VERT_POSN], self.regs[CUR_HORZ_VERT_OFF]);
        Some(Cursor {
            x: (posn & 0x7FF) as i32,
            y: (posn >> 16 & 0x7FF) as i32,
            skip: off & 0x3F,
            rows: 64 - (off >> 16 & 0x3F),
            base: (self.regs[CUR_OFFSET] & 0xF_FFFF) * 8,
        })
    }

    /// Cursor colour `n` (CUR_CLR0 or CUR_CLR1) as RGB, or as its index
    /// through `palette` in an 8-bit mode.
    pub fn cursor_colour(
        &self,
        n: u8,
        palette: Option<&dyn Fn(u8) -> (u8, u8, u8)>,
    ) -> (u8, u8, u8) {
        let c = self.regs[regs::CUR_CLR0 + n as usize];
        match palette {
            Some(palette) => palette(c as u8),
            None => ((c >> 24) as u8, (c >> 16) as u8, (c >> 8) as u8),
        }
    }

    /// The 32 KB pages of the memory the VGA window's halves (A0000h and
    /// A8000h) read and write: MEM_VGA_RP_SEL and MEM_VGA_WP_SEL.
    pub fn window_page(&self, offset: usize, write: bool) -> usize {
        let sel = self.regs[if write {
            regs::MEM_VGA_WP_SEL
        } else {
            regs::MEM_VGA_RP_SEL
        }];
        let page = if offset < 0x8000 {
            sel & 0xFF
        } else {
            sel >> 16 & 0xFF
        };
        (page as usize * 0x8000 + (offset & 0x7FFF)) % MEMORY as usize
    }

    // --- Trace and status ---

    pub fn trace(&self, line: impl FnOnce() -> String) {
        if let Some(out) = self.trace.borrow_mut().as_mut() {
            let _ = writeln!(out, "{}", line());
        }
    }

    pub fn flush_trace(&self) {
        if let Some(out) = self.trace.borrow_mut().as_mut() {
            let _ = out.flush();
        }
    }

    /// For the debugger's status: the chip, where it is, what the CRTC
    /// shows and what the engine does.
    pub fn describe(&self) -> serde_json::Value {
        let screen = self.screen().map(|s| {
            serde_json::json!({
                "width": s.width, "height": s.height, "bpp": s.bpp,
                "base": s.base, "pitch": s.pitch, "hz": s.timing.hz(),
            })
        });
        let ports = if self.block_io() { "block" } else { "sparse" };
        serde_json::json!({
            "chip": if self.rage2 { "3D RAGE II (GT-B)" } else { "3D RAGE (GT)" },
            "aperture": self.aperture().map(|a| format!("{:08X}", a)),
            "ports": format!("{} {:04X}", ports, self.io_base()),
            "native": self.native,
            "screen": screen,
            "pixel_clock": self.pixel_clock(),
            "cursor": self.cursor().map(|c| format!("{:?}", c)),
            "engine": self.engine.describe(),
        })
    }
}

/// A composite register's parts: (register, lowest bit, mask).
fn fields(reg: usize, rage2: bool) -> Option<&'static [(usize, u32, u32)]> {
    use regs::*;
    Some(match reg {
        DST_Y_X | DST_Y_X_ALIAS => &[(DST_Y, 0, 0x7FFF), (DST_X, 16, 0x1FFF)],
        DST_HEIGHT_WIDTH => &[(DST_HEIGHT, 0, 0x7FFF), (DST_WIDTH, 16, 0xFFFF)],
        DST_X_WIDTH => &[(DST_X, 0, 0x1FFF), (DST_WIDTH, 16, 0xFFFF)],
        SRC_Y_X => &[(SRC_Y, 0, 0x7FFF), (SRC_X, 16, 0x1FFF)],
        SRC_HEIGHT1_WIDTH1 => &[(SRC_HEIGHT1, 0, 0x7FFF), (SRC_WIDTH1, 16, 0x1FFF)],
        SRC_Y_X_START => &[(SRC_Y_START, 0, 0x7FFF), (SRC_X_START, 16, 0x1FFF)],
        SRC_HEIGHT2_WIDTH2 => &[(SRC_HEIGHT2, 0, 0x7FFF), (SRC_WIDTH2, 16, 0x1FFF)],
        SC_LEFT_RIGHT => &[(SC_LEFT, 0, 0x1FFF), (SC_RIGHT, 16, 0x1FFF)],
        SC_TOP_BOTTOM => &[(SC_TOP, 0, 0x7FFF), (SC_BOTTOM, 16, 0x7FFF)],
        GUI_TRAJ_CNTL => &[
            (DST_CNTL, 0, 0xFFFF),
            (SRC_CNTL, 16, 0x9F),
            (PAT_CNTL, 24, 7),
            (HOST_CNTL, 28, 3),
        ],
        // The 3D RAGE II's, X and width low.
        DST_X_Y if rage2 => &[(DST_X, 0, 0x1FFF), (DST_Y, 16, 0x7FFF)],
        DST_WIDTH_HEIGHT if rage2 => &[(DST_WIDTH, 0, 0xFFFF), (DST_HEIGHT, 16, 0x7FFF)],
        _ => return None,
    })
}

/// The register an address is another name of: the scaler's and the 3D
/// engine's share some.
fn alias(reg: usize) -> usize {
    use regs::*;
    match reg {
        SCALE_Y_PITCH => S_Y_INC,
        SCALE_X_INC => RED_X_INC,
        SCALE_Y_INC => GREEN_X_INC,
        _ => reg,
    }
}

impl State for Mach64 {
    fn save(&self, w: &mut Writer) {
        self.rage2.save(w);
        self.pci.save(w);
        self.regs.save(w);
        self.pll.to_vec().save(w);
        self.retraces_acked.save(w);
        self.native.save(w);
        self.engine.save(w);
    }

    fn load(&mut self, r: &mut Reader) -> Result<()> {
        self.rage2.load(r)?;
        self.pci.load(r)?;
        self.regs.load(r)?;
        self.regs.resize(regs::COUNT, 0);
        let mut pll: Vec<u8> = Vec::new();
        pll.load(r)?;
        pll.resize(16, 0);
        self.pll.copy_from_slice(&pll);
        self.retraces_acked.load(r)?;
        self.native.load(r)?;
        self.engine.load(r)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(card: &mut Mach64, reg: usize, value: u32) -> Effect {
        let mut vram = vec![0; MEMORY as usize];
        card.load_reg(
            reg,
            value,
            &mut Target {
                vram: &mut vram,
                dac: &[],
                dac_8bit: false,
            },
        )
    }

    #[test]
    fn composites_split_and_read_back() {
        let mut card = Mach64::new(false);
        write(&mut card, regs::DST_Y_X, 0x0123_0456);
        assert_eq!(card.reg(regs::DST_X), 0x123);
        assert_eq!(card.reg(regs::DST_Y), 0x456);
        assert_eq!(card.read(regs::DST_Y_X, Live::default()), 0x0123_0456);
        write(&mut card, regs::GUI_TRAJ_CNTL, 0x3700_00FF);
        assert_eq!(card.reg(regs::DST_CNTL), 0xFF);
        assert_eq!(card.reg(regs::PAT_CNTL), 7);
        assert_eq!(card.reg(regs::HOST_CNTL), 3);
    }

    #[test]
    fn the_pll_is_reached_through_clock_cntl() {
        let mut card = Mach64::new(false);
        // PLL register 7 (VCLK0_FB_DIV) written, then read back.
        write(&mut card, regs::CLOCK_CNTL, 0x0055_1E00);
        write(&mut card, regs::CLOCK_CNTL, 7 << 10);
        assert_eq!(
            card.read(regs::CLOCK_CNTL, Live::default()) >> 16 & 0xFF,
            0x55
        );
    }

    #[test]
    fn sparse_and_block_ports() {
        let mut card = Mach64::new(false);
        assert_eq!(card.port(IO_BASE + 0x1C), Some((regs::CRTC_GEN_CNTL, 0)));
        assert_eq!(card.port(0x1EEC), None);
        card.config_write(0x40, 0);
        assert_eq!(card.port(0x1EEC), Some((regs::CRTC_GEN_CNTL, 0)));
        assert_eq!(card.port(0x6EEF), Some((regs::CONFIG_CHIP_ID, 3)));
        assert_eq!(card.port(IO_BASE), None);
    }

    #[test]
    fn a_programmed_clock_comes_back() {
        let mut card = Mach64::new(false);
        card.program_clock(65_000_000);
        write(&mut card, regs::CLOCK_CNTL, 3);
        let off = card.pixel_clock().abs_diff(65_000_000);
        assert!(off < 300_000, "{}", card.pixel_clock());
    }

    #[test]
    fn the_crtc_describes_a_screen() {
        let mut card = Mach64::new(false);
        // 640x480 at 25.175 MHz, 16 bits per pixel, pitch 640.
        write(&mut card, regs::CRTC_H_TOTAL_DISP, 0x004F_0063);
        write(&mut card, regs::CRTC_V_TOTAL_DISP, 0x01DF_020C);
        write(&mut card, regs::CRTC_V_SYNC_STRT_WID, 0x0002_01E9);
        write(&mut card, regs::CRTC_OFF_PITCH, 80 << 22 | 0x100);
        assert_eq!(card.screen(), None);
        write(&mut card, regs::CRTC_GEN_CNTL, 0x0300_0400);
        let s = card.screen().unwrap();
        assert_eq!(
            (s.width, s.height, s.bpp, s.pitch, s.base),
            (640, 480, 16, 1280, 0x800)
        );
        assert_eq!(s.timing.total, 525);
    }

    #[test]
    fn the_cursor_reads_two_bits_a_pixel() {
        let cursor = Cursor {
            x: 0,
            y: 0,
            skip: 0,
            rows: 64,
            base: 0,
        };
        let mut vram = vec![0u8; 64];
        vram[0] = 0b1110_0100;
        assert_eq!(cursor.pixel(&vram, 0, 0), CursorPixel::Colour(0));
        assert_eq!(cursor.pixel(&vram, 1, 0), CursorPixel::Colour(1));
        assert_eq!(cursor.pixel(&vram, 2, 0), CursorPixel::Transparent);
        assert_eq!(cursor.pixel(&vram, 3, 0), CursorPixel::Invert);
    }
}
