//! The 3dfx Voodoo Banshee and Voodoo3: a whole display adapter, a VGA
//! core with 2D and 3D engines and a video processor around one memory,
//! instead of the Voodoo Graphics' and Voodoo2's add-on card behind a VGA
//! pass-through. Windows uses it as its display, so 3dfx's driver can
//! run Direct3D on the primary display and register its OpenGL ICD.
//!
//! The card is a port of 86Box's vid_voodoo_banshee.c and its 2D engine
//! (Sarah Walker, PCem; GPLv2), checked against 3dfx's Voodoo3
//! specification (revision 1.0, November 1999) and its programming guide
//! (revision 1.4a). Its 3D engine is the Voodoo2's (`super`), drawing
//! into the card's memory where the 3D registers' buffer addresses say.
//!
//! On the PCI bus it is device 1 (vendor 121Ah, device 0005h for the
//! Voodoo3, 0003h for the Banshee, a VGA-compatible display controller)
//! with three BARs:
//!
//! * BAR0, 32 MB: the registers. The init and video registers (also at
//!   the I/O BAR) from 0, the command FIFOs' and AGP registers from
//!   80000h, the 2D engine's from 100000h, the 3D engine's from 200000h,
//!   texture downloads from 600000h and the 3D linear frame buffer (through
//!   lfbMode) from 1000000h.
//! * BAR1, 32 MB: the memory, linear, with addresses from the tile base
//!   in lfbMemoryConfig on mapped to tiles of 128 bytes by 32 rows.
//! * BAR2, 256 ports: the init and video registers, with the VGA's ports
//!   at B0h-DFh.
//!
//! The VGA core is rust-dos's VGA (`VgaCard`), which has its own memory:
//! the card's memory holds what the extended modes show, which the video
//! processor puts on the screen once vgaInit0 bit 12 (extended shift out)
//! and vidProcCfg bit 0 are set, in the size the CRTC registers give.

pub mod blit2d;
pub mod ddc;
pub mod display;
pub mod fifo;
pub mod render3d;

use super::{Effect, Now, Voodoo};
use crate::video::crt::Extension;
use std::collections::VecDeque;

/// The size of BAR0 and of BAR1.
pub const BAR_SIZE: u32 = 32 << 20;
/// The offsets in the card's window: BAR0's from 0, BAR1's from here.
pub const LINEAR: u32 = BAR_SIZE;
/// Where the BIOS puts the BARs.
pub const INITIAL_BAR0: u32 = 0xD000_0000;
pub const INITIAL_BAR1: u32 = 0xD200_0000;
pub const INITIAL_IO: u32 = 0xD000;
/// The interrupt line the BIOS routes INTA# to.
pub const IRQ: u8 = 11;
/// The memory: 16 MB of SGRAM.
pub const MEMORY: usize = 16 << 20;

/// Which chip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Banshee,
    Voodoo3,
}

impl Kind {
    pub fn device_id(self) -> u16 {
        match self {
            Kind::Banshee => 0x0003,
            Kind::Voodoo3 => 0x0005,
        }
    }

    fn revision(self) -> u8 {
        match self {
            Kind::Banshee => 3,
            Kind::Voodoo3 => 1,
        }
    }

    /// The subsystem: a Banshee with SGRAM, a Voodoo3 3000 PCI (which
    /// 3dfx's Voodoo3.inf names SUBSYS_003A121A).
    fn subsystem(self) -> u32 {
        match self {
            Kind::Banshee => 0x0004_121A,
            Kind::Voodoo3 => 0x003A_121A,
        }
    }

    /// The texture units: the Voodoo3 has two.
    pub fn texture_units(self) -> usize {
        match self {
            Kind::Banshee => 1,
            Kind::Voodoo3 => 2,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Banshee => "banshee",
            Kind::Voodoo3 => "voodoo3",
        }
    }
}

// The init and video registers (byte offsets in the I/O BAR and BAR0).
pub const STATUS: u32 = 0x00;
pub const PCI_INIT0: u32 = 0x04;
pub const LFB_MEMORY_CONFIG: u32 = 0x0C;
pub const MISC_INIT0: u32 = 0x10;
pub const MISC_INIT1: u32 = 0x14;
pub const DRAM_INIT0: u32 = 0x18;
pub const DRAM_INIT1: u32 = 0x1C;
pub const VGA_INIT0: u32 = 0x28;
pub const VGA_INIT1: u32 = 0x2C;
pub const STRAP_INFO: u32 = 0x38;
pub const PLL_CTRL0: u32 = 0x40;
pub const DAC_MODE: u32 = 0x4C;
pub const DAC_ADDR: u32 = 0x50;
pub const DAC_DATA: u32 = 0x54;
pub const VID_PROC_CFG: u32 = 0x5C;
pub const HW_CUR_PAT_ADDR: u32 = 0x60;
pub const HW_CUR_LOC: u32 = 0x64;
pub const HW_CUR_C0: u32 = 0x68;
pub const HW_CUR_C1: u32 = 0x6C;
pub const VID_SERIAL_PARALLEL_PORT: u32 = 0x78;
pub const VID_IN_STATUS_CURRENT_LINE: u32 = 0x94;
pub const VID_SCREEN_SIZE: u32 = 0x98;
pub const VID_OVERLAY_START_COORDS: u32 = 0x9C;
pub const VID_OVERLAY_END_SCREEN_COORDS: u32 = 0xA0;
pub const VID_OVERLAY_DUDX: u32 = 0xA4;
pub const VID_OVERLAY_DUDX_OFFSET_SRC_WIDTH: u32 = 0xA8;
pub const VID_OVERLAY_DVDY: u32 = 0xAC;
pub const VID_OVERLAY_DVDY_OFFSET: u32 = 0xE0;
pub const VID_DESKTOP_START_ADDR: u32 = 0xE4;
pub const VID_DESKTOP_OVERLAY_STRIDE: u32 = 0xE8;
pub const VID_CURR_OVERLAY_START_ADDR: u32 = 0xFC;

// vgaInit0 and vidProcCfg bits.
pub const VGAINIT0_DAC_8BIT: u32 = 1 << 2;
pub const VGAINIT0_EXTENDED_SHIFT_OUT: u32 = 1 << 12;
pub const VIDPROC_ENABLE: u32 = 1 << 0;
pub const VIDPROC_CURSOR_X11: u32 = 1 << 1;
pub const VIDPROC_HALF_MODE: u32 = 1 << 4;
pub const VIDPROC_OVERLAY_ENABLE: u32 = 1 << 8;
pub const VIDPROC_DESKTOP_CLUT_BYPASS: u32 = 1 << 10;
pub const VIDPROC_OVERLAY_CLUT_BYPASS: u32 = 1 << 11;
pub const VIDPROC_DESKTOP_CLUT_SEL: u32 = 1 << 12;
pub const VIDPROC_OVERLAY_CLUT_SEL: u32 = 1 << 13;
pub const VIDPROC_H_SCALE: u32 = 1 << 14;
pub const VIDPROC_V_SCALE: u32 = 1 << 15;
pub const VIDPROC_DESKTOP_TILED: u32 = 1 << 24;
pub const VIDPROC_OVERLAY_TILED: u32 = 1 << 25;
pub const VIDPROC_2X: u32 = 1 << 26;
pub const VIDPROC_CURSOR: u32 = 1 << 27;

// vidSerialParallelPort: the DDC and I2C lines, written and read back.
const DDC_ENABLE: u32 = 1 << 18;
const DDC_CLOCK_W: u32 = 1 << 19;
const DDC_DATA_W: u32 = 1 << 20;
const DDC_CLOCK_R: u32 = 1 << 21;
const DDC_DATA_R: u32 = 1 << 22;
const I2C_ENABLE: u32 = 1 << 23;
const I2C_CLOCK_W: u32 = 1 << 24;
const I2C_DATA_W: u32 = 1 << 25;
const I2C_CLOCK_R: u32 = 1 << 26;
const I2C_DATA_R: u32 = 1 << 27;

/// The card's PCI configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pci {
    pub command: u16,
    pub bar0: u32,
    pub bar1: u32,
    pub io: u32,
    pub interrupt_line: u8,
}

crate::state_fields!(Pci {
    command,
    bar0,
    bar1,
    io,
    interrupt_line
});

impl Default for Pci {
    /// As the BIOS leaves it: I/O and memory decoding on, the BARs where
    /// it put them.
    fn default() -> Self {
        Self {
            command: 0x0003,
            bar0: INITIAL_BAR0,
            bar1: INITIAL_BAR1,
            io: INITIAL_IO,
            interrupt_line: IRQ,
        }
    }
}

/// The registers of the AGP mover (agpReqSize to agpMoveCMD).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Agp {
    pub req_size: u32,
    pub host_low: u32,
    pub host_high: u32,
    pub graphics_address: u32,
    pub graphics_stride: u32,
    pub move_cmd: u32,
}

crate::state_fields!(Agp {
    req_size,
    host_low,
    host_high,
    graphics_address,
    graphics_stride,
    move_cmd
});

/// The registers of the card's that a mode set of the BIOS changes:
/// vgaInit0, vidProcCfg and the CRTC's.
#[derive(Clone)]
pub struct ModeRegs {
    vga_init0: u32,
    vid_proc_cfg: u32,
    crtc: [u8; 0x40],
}

/// The card's own state, beside the 3D engine's (`Voodoo`).
#[derive(Clone, Debug)]
pub struct Banshee {
    pub kind: Kind,
    pub pci: Pci,
    /// The init and video registers, by byte offset / 4, as written.
    pub ext: [u32; 64],
    /// The colour lookup table: 512 entries of 24-bit RGB (two tables).
    pub clut: [u32; 512],
    /// The CRTC registers, mirrored from the VGA's (00h-18h), and the
    /// card's own (19h-3Fh).
    pub crtc: [u8; 0x40],
    /// The VGA's Miscellaneous Output and Clocking Mode, mirrored.
    pub misc_output: u8,
    pub seq01: u8,
    /// intrCtrl.
    pub intr_ctrl: u32,
    /// The VGA's vertical retrace interrupt, which CRTC register 11h's
    /// bit 4 arms: armed at retrace `r`, it is pending from retrace
    /// `r + 1` until the bit is written 0. None while it is held clear.
    pub vint: Option<u64>,
    /// The interrupt request the card has up.
    pub irq_up: Option<u8>,
    pub agp: Agp,
    pub fifo: [fifo::Fifo; 2],
    pub engine: blit2d::Engine,
    /// The buffer the overlay shows, which a swap sets from
    /// leftOverlayBuf, and the buffers of the swaps waiting for their
    /// retraces.
    pub overlay_addr: u32,
    pub overlay_queue: VecDeque<u32>,
    /// Writes to swapPending not yet matched by a swap.
    pub swap_count: u32,
    /// The monitor on the DDC lines.
    pub ddc: ddc::Ddc,
    /// What the display needs to hear: the picture changed, the palette
    /// changed.
    pub dirty: bool,
}

crate::state_fields!(Banshee {
    pci, ext, clut, crtc, misc_output, seq01, intr_ctrl, vint, irq_up, agp,
    fifo, engine, overlay_addr, overlay_queue, swap_count,
} skip { kind, ddc, dirty });

impl Banshee {
    /// As at power-on and after the BIOS: 16 MB of SGRAM in dramInit0
    /// and 1 (eight 2 MB parts, as Linux's tdfxfb reads them), the DDC
    /// lines released, the command FIFOs idle.
    pub fn new(kind: Kind) -> Self {
        let mut b = Self {
            kind,
            pci: Pci::default(),
            ext: [0; 64],
            clut: [0; 512],
            crtc: [0; 0x40],
            misc_output: 0,
            seq01: 0,
            intr_ctrl: 0,
            vint: None,
            irq_up: None,
            agp: Agp::default(),
            fifo: Default::default(),
            engine: blit2d::Engine::default(),
            overlay_addr: 0,
            overlay_queue: VecDeque::new(),
            swap_count: 0,
            ddc: ddc::Ddc::default(),
            dirty: true,
        };
        b.ext[(DRAM_INIT0 / 4) as usize] = (1 << 27) | (1 << 26);
        b.ext[(VID_SERIAL_PARALLEL_PORT / 4) as usize] = DDC_CLOCK_W | DDC_DATA_W;
        for (i, c) in b.clut.iter_mut().enumerate() {
            let v = (i & 0xFF) as u32;
            *c = v << 16 | v << 8 | v;
        }
        for fifo in &mut b.fifo {
            fifo.status = 1 << 28;
        }
        b
    }

    /// A PCI reset: as `new` leaves the card.
    pub fn reset(&mut self) {
        *self = Self::new(self.kind);
    }

    /// The line the 3D engine's Y origin at the bottom is: miscInit0
    /// bits 29:18.
    pub fn y_origin(&self) -> u32 {
        (self.reg(MISC_INIT0) >> 18) & 0xFFF
    }

    pub fn reg(&self, offset: u32) -> u32 {
        self.ext[((offset & 0xFC) >> 2) as usize]
    }

    fn set_reg(&mut self, offset: u32, value: u32) {
        self.ext[((offset & 0xFC) >> 2) as usize] = value;
    }

    // --- PCI ---

    /// A byte of the configuration space: a VGA-compatible display
    /// controller (class 0300h), the BARs, the subsystem, and the power
    /// management capability at 60h (86Box's `banshee_pci_read`), and
    /// INTA# for the vertical retrace interrupt.
    pub fn config_read(&self, reg: u8) -> u8 {
        let dword = match reg & 0xFC {
            0x00 => (self.kind.device_id() as u32) << 16 | 0x121A,
            // Status: a capabilities list.
            0x04 => 0x0010_0000 | self.pci.command as u32,
            0x08 => 0x0300_0000 | self.kind.revision() as u32,
            0x10 => self.pci.bar0,
            0x14 => self.pci.bar1,
            0x18 => self.pci.io | 0x01,
            0x2C => self.kind.subsystem(),
            0x34 => 0x60,
            // INTA#, routed to `interrupt_line`.
            0x3C => 0x0100 | self.pci.interrupt_line as u32,
            0x40 => 0x01,
            // Power management, version 1.1, no next capability.
            0x60 => 0x0021_0001,
            _ => 0,
        };
        (dword >> (8 * (reg & 3))) as u8
    }

    /// A byte of the configuration space written. True if the BARs or
    /// the decoding moved.
    pub fn config_write(&mut self, reg: u8, value: u8) -> bool {
        match reg {
            0x04 => self.pci.command = (self.pci.command & 0xFF00) | (value & 0x27) as u16,
            // 32 MB aligned: bits 25-31.
            0x13 => self.pci.bar0 = ((value & 0xFE) as u32) << 24,
            0x17 => self.pci.bar1 = ((value & 0xFE) as u32) << 24,
            // 256 ports: bits 8-31.
            0x19..=0x1B => {
                let shift = 8 * (reg - 0x18) as u32;
                self.pci.io = (self.pci.io & !(0xFF << shift)) | (value as u32) << shift;
            }
            0x3C => self.pci.interrupt_line = value,
            _ => return false,
        }
        true
    }

    /// The offset in the card's window of physical address `addr`, if it
    /// is in BAR0 or BAR1 (BAR1's from `LINEAR`). The BARs count where
    /// they are above the RAM and below the BIOS ROM's mirror.
    #[inline]
    pub fn window_at(&self, addr: u32, ram: usize) -> Option<u32> {
        if self.pci.command & 0x02 == 0 || addr >= 0xFFFE_0000 || (addr as usize) < ram {
            return None;
        }
        let in_bar = |bar: u32| bar != 0 && addr.wrapping_sub(bar) < BAR_SIZE;
        if in_bar(self.pci.bar1) {
            Some(LINEAR + (addr - self.pci.bar1))
        } else if in_bar(self.pci.bar0) {
            Some(addr - self.pci.bar0)
        } else {
            None
        }
    }

    /// The register of the I/O BAR at `port`, if it is one.
    #[inline]
    pub fn io_offset(&self, port: u16) -> Option<u32> {
        let base = self.pci.io & 0xFFFF_FF00;
        let offset = (port as u32).wrapping_sub(base);
        (self.pci.command & 0x01 != 0 && base != 0 && offset < 0x100).then_some(offset)
    }

    // --- The linear frame buffer ---

    /// The memory address of BAR1 offset `addr`: from the tile base on,
    /// rows of the tile stride in tiles of 128 bytes by 32 rows (86Box's
    /// `banshee_write_linear`).
    pub fn linear_addr(&self, addr: u32) -> u32 {
        let config = self.reg(LFB_MEMORY_CONFIG);
        let tile_base = (config & 0x1FFF) << 12;
        let addr = addr & (BAR_SIZE - 1);
        if addr < tile_base {
            return addr;
        }
        let shift = 10 + ((config >> 13) & 7);
        let tile_x = ((config >> 16) & 0x7F) * 128 * 32;
        let offset = addr - tile_base;
        let (x, y) = (offset & ((1 << shift) - 1), offset >> shift);
        tile_base + (x & 127) + (x >> 7) * 128 * 32 + (y & 31) * 128 + (y >> 5) * tile_x
    }

    // --- Video ---

    /// Whether the video processor shows the desktop from the card's
    /// memory instead of the VGA's picture.
    pub fn extended(&self) -> bool {
        self.reg(VGA_INIT0) & VGAINIT0_EXTENDED_SHIFT_OUT != 0
            && self.reg(VID_PROC_CFG) & VIDPROC_ENABLE != 0
    }

    /// The pixel clock PLL 0 makes: (N + 2) / ((M + 2) * 2^K) times
    /// 14.318 MHz.
    pub fn pll_hz(&self, n: usize) -> u64 {
        let v = self.ext[(PLL_CTRL0 / 4) as usize + n];
        let (k, m, nn) = (v & 3, (v >> 2) & 0x3F, (v >> 8) & 0xFF);
        (nn as u64 + 2) * 14_318_180 / ((m as u64 + 2) << k)
    }

    /// What the card adds to the VGA's timing registers: PLL 0 when the
    /// clock select picks it, 16-dot characters in 2X mode, and the extra
    /// bits of CR1Ah and CR1Bh.
    pub fn timing_extension(&self) -> Extension {
        let (cr1a, cr1b) = (self.crtc[0x1A] as u32, self.crtc[0x1B] as u32);
        Extension {
            clock: ((self.misc_output >> 2) & 3 == 3)
                .then(|| self.pll_hz(0))
                .filter(|&hz| hz > 0),
            wide_chars: self.reg(VID_PROC_CFG) & VIDPROC_2X != 0 && self.extended(),
            htotal_high: (cr1a & 1) << 8,
            vertical_high: [
                (cr1b & 1) << 10,
                ((cr1b >> 2) & 1) << 10,
                ((cr1b >> 6) & 1) << 10,
                0,
            ],
        }
    }

    /// The desktop's size in pixels: the CRTC's display end, horizontal
    /// doubled in 2X mode, with the card's extra bits.
    pub fn desktop_size(&self) -> (u32, u32) {
        let c = &self.crtc;
        let mut width = (c[0x01] as u32 + 1 + ((c[0x1A] as u32 >> 2) & 1) * 0x100) * 8;
        if self.reg(VID_PROC_CFG) & VIDPROC_2X != 0 {
            width *= 2;
        }
        let overflow = c[0x07] as u32;
        let height = (c[0x12] as u32
            | ((overflow >> 1) & 1) << 8
            | ((overflow >> 6) & 1) << 9
            | ((c[0x1B] as u32 >> 2) & 1) << 10)
            + 1;
        (width.clamp(8, 2048), height.clamp(1, 2048))
    }

    /// The swap count, pending swaps and busy bits of the status register
    /// (86Box's `banshee_status`) but for the retrace (`in_retrace`).
    /// The PCI FIFO is always empty; bit 6 is set outside the vertical
    /// retrace, as the specification's section 16.1 says.
    pub fn status(&self, in_retrace: bool, busy: bool) -> u32 {
        let mut status = 0x1F;
        if !in_retrace {
            status |= 1 << 6;
        }
        let fifo_busy = [0, 1].map(|n| self.fifo[n].enabled && self.fifo[n].depth > 0);
        if busy || fifo_busy[0] || fifo_busy[1] {
            status |= 0x780;
        }
        if fifo_busy[0] {
            status |= 1 << 11;
        }
        if fifo_busy[1] {
            status |= 1 << 12;
        }
        status | self.swap_count.min(7) << 28
    }

    /// An init or video register written whole (86Box's
    /// `banshee_ext_outl`). True if the display's mode or timing may have
    /// changed.
    pub fn ext_write(&mut self, offset: u32, value: u32) -> bool {
        let offset = offset & 0xFC;
        match offset {
            STATUS | STRAP_INFO | VID_IN_STATUS_CURRENT_LINE | VID_CURR_OVERLAY_START_ADDR => {
                return false;
            }
            DAC_ADDR => self.set_reg(offset, value & 0x1FF),
            DAC_DATA => {
                let at = (self.reg(DAC_ADDR) & 0x1FF) as usize;
                self.clut[at] = value & 0xFF_FFFF;
                self.dirty = true;
                return false;
            }
            VID_DESKTOP_START_ADDR => self.set_reg(offset, value & 0xFF_FFFF),
            VID_SERIAL_PARALLEL_PORT => {
                self.set_reg(offset, value);
                self.ddc
                    .drive(value & DDC_CLOCK_W != 0, value & DDC_DATA_W != 0);
                return false;
            }
            _ => self.set_reg(offset, value),
        }
        self.dirty = true;
        matches!(
            offset,
            VGA_INIT0
                | PLL_CTRL0
                | DAC_MODE
                | VID_PROC_CFG
                | VID_DESKTOP_START_ADDR
                | VID_DESKTOP_OVERLAY_STRIDE
        )
    }

    /// An init or video register read but for the status and the current
    /// line, which the card answers (86Box's `banshee_ext_inl`).
    pub fn ext_read(&self, offset: u32) -> u32 {
        let offset = offset & 0xFC;
        match offset {
            // 8 MB of SGRAM is what 86Box says; the memory is in
            // dramInit0. A PCI card with its interrupt on, a 32 KB BIOS.
            STRAP_INFO => 0x0000_0040,
            DAC_DATA => self.clut[(self.reg(DAC_ADDR) & 0x1FF) as usize],
            VID_SERIAL_PARALLEL_PORT => {
                // The monitor on the DDC lines; nothing on the I2C bus,
                // whose lines read as they are driven.
                let v = self.reg(offset) & !(DDC_CLOCK_R | DDC_DATA_R | I2C_CLOCK_R | I2C_DATA_R);
                let mut lines = 0;
                if v & DDC_ENABLE != 0 {
                    let (scl, sda) = self.ddc.lines();
                    lines |= if scl { DDC_CLOCK_R } else { 0 };
                    lines |= if sda { DDC_DATA_R } else { 0 };
                }
                if v & I2C_ENABLE != 0 {
                    lines |= if v & I2C_CLOCK_W != 0 { I2C_CLOCK_R } else { 0 };
                    lines |= if v & I2C_DATA_W != 0 { I2C_DATA_R } else { 0 };
                }
                v | lines
            }
            VID_CURR_OVERLAY_START_ADDR => self.overlay_addr,
            _ => self.reg(offset),
        }
    }

    /// The VGA's DAC entry `index` was written with `rgb` (8 bits each):
    /// the CLUT's first table is the same RAM.
    pub fn vga_palette_written(&mut self, index: u8, rgb: (u8, u8, u8)) {
        self.clut[index as usize] = (rgb.0 as u32) << 16 | (rgb.1 as u32) << 8 | rgb.2 as u32;
        self.dirty = true;
    }

    /// The BIOS set a VGA or VESA mode: the video processor and the
    /// extended shift out off and the DAC back to 6 bits, as the card's
    /// BIOS leaves them.
    pub fn bios_mode_set(&mut self) {
        let init0 = self.reg(VGA_INIT0) & !(VGAINIT0_EXTENDED_SHIFT_OUT | VGAINIT0_DAC_8BIT);
        self.set_reg(VGA_INIT0, init0);
        let cfg =
            self.reg(VID_PROC_CFG) & !(VIDPROC_ENABLE | VIDPROC_CURSOR | VIDPROC_OVERLAY_ENABLE);
        self.set_reg(VID_PROC_CFG, cfg);
        self.crtc[0x1A] = 0;
        self.crtc[0x1B] = 0;
        self.dirty = true;
    }

    /// The registers a mode set of the BIOS changes (`bios_mode_set`), to
    /// put back after a video BIOS service for a DOS box whose display
    /// Windows keeps.
    pub fn mode_regs(&self) -> ModeRegs {
        ModeRegs {
            vga_init0: self.reg(VGA_INIT0),
            vid_proc_cfg: self.reg(VID_PROC_CFG),
            crtc: self.crtc,
        }
    }

    /// Put back what `mode_regs` saved.
    pub fn restore_mode_regs(&mut self, regs: &ModeRegs) {
        self.set_reg(VGA_INIT0, regs.vga_init0);
        self.set_reg(VID_PROC_CFG, regs.vid_proc_cfg);
        self.crtc = regs.crtc;
        self.dirty = true;
    }

    /// For the debugger.
    pub fn describe(&self) -> serde_json::Value {
        let (w, h) = self.desktop_size();
        serde_json::json!({
            "kind": self.kind.name(),
            "bar0": format!("{:08X}", self.pci.bar0),
            "bar1": format!("{:08X}", self.pci.bar1),
            "io": format!("{:04X}", self.pci.io),
            "command": format!("{:04X}", self.pci.command),
            "extended": self.extended(),
            "desktop": format!("{}x{}", w, h),
            "vgaInit0": format!("{:08X}", self.reg(VGA_INIT0)),
            "vidProcCfg": format!("{:08X}", self.reg(VID_PROC_CFG)),
            "desktopStart": format!("{:06X}", self.reg(VID_DESKTOP_START_ADDR)),
            "stride": format!("{:08X}", self.reg(VID_DESKTOP_OVERLAY_STRIDE)),
            "pll0_hz": self.pll_hz(0),
            "cursor": format!("{:08X}", self.reg(HW_CUR_LOC)),
            "overlay": format!("{:06X}", self.overlay_addr),
            "fifo": self.fifo.iter().map(|f| f.describe()).collect::<Vec<_>>(),
        })
    }
}

/// Merge the bytes `mask` selects of `value` into `old`.
fn merge(old: u32, value: u32, mask: u32) -> u32 {
    (old & !mask) | (value & mask)
}

impl Voodoo {
    pub fn banshee(&self) -> Option<&Banshee> {
        self.banshee.as_deref()
    }

    /// The status register, in the init registers, the 2D and the 3D
    /// space alike.
    pub(crate) fn banshee_status(&self, now: Now) -> u32 {
        let Some(b) = &self.banshee else { return 0 };
        let in_retrace = self.timing.status(now.ns) & 0x08 != 0;
        b.status(in_retrace, self.drawing())
    }

    /// CRTC register 11h now holds `value`: bit 4 written 0 clears the
    /// vertical retrace interrupt and holds it clear, written 1 arms it
    /// for the next retrace (86Box's `banshee_out`).
    pub fn banshee_crtc11(&mut self, value: u8, now: Now) {
        let current = self.retraces(now.ns);
        let Some(b) = &mut self.banshee else { return };
        if value & 0x10 == 0 {
            b.vint = None;
        } else if b.vint.is_none() {
            b.vint = Some(current);
        }
    }

    /// Whether a vertical retrace interrupt is pending: Input Status 0's
    /// bit 7.
    pub fn banshee_vint_pending(&self, now: Now) -> bool {
        let current = self.retraces(now.ns);
        self.banshee
            .as_ref()
            .and_then(|b| b.vint)
            .is_some_and(|armed| current > armed)
    }

    /// Whether the card may interrupt: CRTC register 11h's bit 5 clear
    /// and pciInit0's bit 18 set.
    fn banshee_vint_enabled(&self) -> bool {
        self.banshee
            .as_ref()
            .is_some_and(|b| b.crtc[0x11] & 0x20 == 0 && b.reg(PCI_INIT0) & (1 << 18) != 0)
    }

    /// The interrupt line the card drives now, if it drives one.
    pub fn banshee_irq(&self, now: Now) -> Option<u8> {
        let b = self.banshee.as_ref()?;
        let line = b.pci.interrupt_line;
        (line < 16 && self.banshee_vint_enabled() && self.banshee_vint_pending(now)).then_some(line)
    }

    /// When, in PIT ticks, the armed interrupt becomes pending, while it
    /// isn't yet up.
    pub fn banshee_vint_due(&self) -> Option<u64> {
        let b = self.banshee.as_ref()?;
        let armed = b.vint?;
        // No line to raise (`banshee_irq`), nothing to wait for.
        (b.irq_up.is_none() && b.pci.interrupt_line < 16 && self.banshee_vint_enabled())
            .then(|| self.retrace_ticks(armed + 1))
    }

    /// The init register at byte `offset` (I/O BAR or BAR0), as the CPU
    /// reads it.
    pub fn banshee_ext_read(&self, offset: u32, now: Now) -> u32 {
        let Some(b) = &self.banshee else {
            return 0xFFFF_FFFF;
        };
        match offset & 0xFC {
            STATUS => self.banshee_status(now),
            VID_IN_STATUS_CURRENT_LINE => self.scanline(now) & 0x7FF,
            o => b.ext_read(o),
        }
    }

    /// A dword of the card's window at byte `offset`, 4-aligned.
    pub(crate) fn banshee_read(&self, offset: u32, now: Now) -> u32 {
        if offset >= LINEAR {
            let Some(b) = &self.banshee else {
                return 0xFFFF_FFFF;
            };
            let addr = b.linear_addr(offset - LINEAR);
            if self.writes_queue() {
                self.catch_up();
            }
            return blit2d::read(&self.fbi.ram, addr, 4, self.fbi.mask);
        }
        match offset & 0x1F0_0000 {
            0x000_0000 if offset & 0x8_0000 == 0 => self.banshee_ext_read(offset & 0xFF, now),
            0x000_0000 => self.banshee_cmd_read(offset),
            0x010_0000 => match offset & 0x1FC {
                0x00 => self.banshee_status(now),
                0x04 => self
                    .banshee
                    .as_ref()
                    .map_or(0, |b| b.intr_ctrl & 0x0030_003F),
                reg => self
                    .banshee
                    .as_ref()
                    .map_or(0xFFFF_FFFF, |b| b.engine.read(reg)),
            },
            0x020_0000..=0x050_0000 => self.banshee_3d_read(offset, now),
            0x100_0000..=0x1F0_0000 => self.lfb_read((offset & 0xFF_FFFC) >> 2),
            _ => 0xFFFF_FFFF,
        }
    }

    /// The bytes `mask` selects of a dword of the card's window written.
    pub(crate) fn banshee_write(&mut self, offset: u32, data: u32, mask: u32, now: Now) -> Effect {
        let mut effect = Effect::default();
        if offset >= LINEAR {
            self.banshee_linear_write(offset - LINEAR, data, mask, now, &mut effect);
            return effect;
        }
        match offset & 0x1F0_0000 {
            0x000_0000 if offset & 0x8_0000 == 0 => {
                let old = self.banshee_ext_read(offset & 0xFF, now);
                if let Some(b) = &mut self.banshee
                    && b.ext_write(offset & 0xFF, merge(old, data, mask))
                {
                    effect.settle = true;
                }
                if offset & 0xFC == MISC_INIT0
                    && let Some(b) = &self.banshee
                {
                    self.fbi.yorigin = b.y_origin();
                }
                // pciInit0 lets the interrupt out or holds it in.
                if offset & 0xFC == PCI_INIT0 {
                    effect.settle = true;
                }
                self.display_dirty = true;
            }
            0x000_0000 => self.banshee_cmd_write(offset, data, now, &mut effect),
            0x010_0000 => {
                if offset & 0x1FC == 0x04 {
                    if let Some(b) = &mut self.banshee {
                        b.intr_ctrl = data & 0x0030_003F;
                    }
                } else {
                    self.banshee_2d_write(offset & 0x1FC, data);
                }
            }
            0x020_0000..=0x050_0000 => self.banshee_3d_write(offset, data, now, &mut effect),
            0x060_0000 | 0x070_0000 => self.banshee_texture_write(offset, data),
            // The display changes if the buffer written is shown
            // (`mark_drawn`).
            0x100_0000..=0x1F0_0000 => self.lfb_write((offset & 0xFF_FFFC) >> 2, data, mask),
            _ => {}
        }
        effect
    }

    /// A 2D register written: the engine runs commands into the memory.
    pub(crate) fn banshee_2d_write(&mut self, reg: u32, data: u32) {
        if self.writes_queue() {
            self.catch_up();
        }
        let Some(b) = &mut self.banshee else { return };
        b.engine.write(&self.fbi.ram, reg, data);
        if std::mem::take(&mut b.engine.drawn) {
            self.display_dirty = true;
        }
        if b.engine.touched.is_empty() {
            return;
        }
        let mut runs = std::mem::take(&mut b.engine.touched);
        for &(start, end) in &runs {
            self.mirror_memory(start, end);
        }
        runs.clear();
        if let Some(b) = &mut self.banshee {
            b.engine.touched = runs;
        }
    }

    /// A write through BAR1: into the memory, through the tiles, and into
    /// a command FIFO where its ring is.
    fn banshee_linear_write(
        &mut self,
        offset: u32,
        data: u32,
        mask: u32,
        now: Now,
        effect: &mut Effect,
    ) {
        let Some(b) = &self.banshee else { return };
        let addr = b.linear_addr(offset);
        // A command FIFO's ring is memory nothing draws into or shows: the
        // packets written there needn't wait for the triangles before
        // them, which would keep the workers from drawing while the
        // program builds the next ones.
        let in_ring = mask == 0xFFFF_FFFF && b.fifo.iter().any(|f| f.holds(addr));
        if self.writes_queue() && !in_ring {
            self.catch_up();
        }
        let ram = &self.fbi.ram;
        let fbmask = self.fbi.mask;
        if mask == 0xFFFF_FFFF {
            blit2d::write(ram, addr, 4, data, fbmask);
        } else {
            for i in 0..4 {
                if mask & (0xFF << (8 * i)) != 0 {
                    ram.set_byte(((addr + i) & fbmask) as usize, (data >> (8 * i)) as u8);
                }
            }
        }
        if in_ring {
            self.banshee_fifo_written(addr, now, effect);
        } else {
            self.display_dirty = true;
            // The bytes written, first to last.
            let addr = addr & fbmask;
            let (first, last) = (mask.trailing_zeros() / 8, 4 - mask.leading_zeros() / 8);
            self.mirror_memory(addr + first, addr + last);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pci_configuration_of_a_voodoo3_3000() {
        let b = Banshee::new(Kind::Voodoo3);
        let dword =
            |reg: u8| (0..4).fold(0u32, |v, i| v | (b.config_read(reg + i) as u32) << (8 * i));
        assert_eq!(dword(0x00), 0x0005_121A);
        assert_eq!(dword(0x08), 0x0300_0001);
        assert_eq!(dword(0x2C), 0x003A_121A);
        assert_eq!(dword(0x10), INITIAL_BAR0);
        assert_eq!(dword(0x14), INITIAL_BAR1);
        assert_eq!(dword(0x18), INITIAL_IO | 1);
        let b = Banshee::new(Kind::Banshee);
        assert_eq!(b.config_read(0x02), 0x03);
    }

    #[test]
    fn bars_size_as_32_mb() {
        let mut b = Banshee::new(Kind::Voodoo3);
        // A BIOS sizes BAR0 by writing all ones.
        for reg in 0x10..0x14 {
            b.config_write(reg, 0xFF);
        }
        assert_eq!(b.pci.bar0, 0xFE00_0000);
        b.config_write(0x13, 0xE4);
        assert_eq!(b.window_at(0xE400_0010, 1 << 20), Some(0x10));
        assert_eq!(
            b.window_at(INITIAL_BAR1 + 0x20, 1 << 20),
            Some(LINEAR + 0x20)
        );
        // Memory decoding off.
        b.config_write(0x04, 0x01);
        assert_eq!(b.window_at(INITIAL_BAR1, 1 << 20), None);
        assert_eq!(b.io_offset(INITIAL_IO as u16 + 0xB4), Some(0xB4));
    }

    #[test]
    fn linear_addresses_from_the_tile_base_are_tiled() {
        let mut b = Banshee::new(Kind::Voodoo3);
        // Tile base 1 MB, a stride of 2048 bytes (shift 1), 4 tiles a row.
        b.ext_write(LFB_MEMORY_CONFIG, 0x100 | 1 << 13 | 4 << 16);
        assert_eq!(b.linear_addr(0x1000), 0x1000);
        let base = 0x10_0000;
        // Byte 130 of row 33 of the tiled region.
        assert_eq!(
            b.linear_addr(base + 33 * 2048 + 130),
            base + 2 + 4096 + 128 + 4 * 4096
        );
    }

    #[test]
    fn pll_and_desktop_size() {
        let mut b = Banshee::new(Kind::Voodoo3);
        // N = 0x2B, M = 4, K = 1: 45 / 12 * 14.318 MHz.
        b.ext_write(PLL_CTRL0, 0x2B << 8 | 4 << 2 | 1);
        assert_eq!(b.pll_hz(0), 45 * 14_318_180 / 12);
        // 1024x768: CR01 = 127, CR12 = 0xFF with overflow bit 9 (767).
        b.crtc[0x01] = 127;
        b.crtc[0x12] = 0xFF;
        b.crtc[0x07] = 0x40;
        assert_eq!(b.desktop_size(), (1024, 768));
        // 2X mode doubles the CRTC's width: 800 characters of 16 pixels.
        b.crtc[0x01] = 99;
        b.ext_write(VID_PROC_CFG, VIDPROC_2X);
        assert_eq!(b.desktop_size().0, 1600);
        // CR1Ah bit 2 is the display end's bit 8: 272 characters, more
        // than the 2048 pixels the card shows.
        b.ext_write(VID_PROC_CFG, 0);
        b.crtc[0x01] = 0x0F;
        b.crtc[0x1A] = 0x04;
        assert_eq!(b.desktop_size().0, 2048);
    }

    #[test]
    fn dac_data_goes_to_the_clut() {
        let mut b = Banshee::new(Kind::Voodoo3);
        b.ext_write(DAC_ADDR, 0x105);
        b.ext_write(DAC_DATA, 0x12_3456);
        assert_eq!(b.clut[0x105], 0x12_3456);
        assert_eq!(b.ext_read(DAC_DATA), 0x12_3456);
        assert_eq!(b.ext_read(DAC_ADDR), 0x105);
    }

    #[test]
    fn ddc_lines_read_back_as_driven() {
        let mut b = Banshee::new(Kind::Voodoo3);
        b.ext_write(VID_SERIAL_PARALLEL_PORT, DDC_ENABLE | DDC_CLOCK_W);
        let v = b.ext_read(VID_SERIAL_PARALLEL_PORT);
        assert_ne!(v & DDC_CLOCK_R, 0);
        assert_eq!(v & DDC_DATA_R, 0);
        // Disabled, they read low.
        b.ext_write(VID_SERIAL_PARALLEL_PORT, DDC_CLOCK_W | DDC_DATA_W);
        let v = b.ext_read(VID_SERIAL_PARALLEL_PORT);
        assert_eq!(v & (DDC_CLOCK_R | DDC_DATA_R), 0);
    }
}
