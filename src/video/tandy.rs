//! The IBM PCjr's video (`machine=pcjr`) and the Tandy 1000's copy of it
//! (`machine=tandy`): a Motorola 6845 at 3D4h as on the CGA, and the video
//! gate array in front of it. The gate array has no memory of its own: it
//! shows system RAM, the first 128 KB on the PCjr and the top 128 KB of
//! the 640 on the Tandy, in pages of 16 KB. The page register (3DFh) picks
//! the page (or pair of pages) the picture comes from and the one the
//! processor sees through the CGA's window at B8000h. Its registers, one
//! index port and one data port on the Tandy (3DAh, 3DEh) and both at 3DAh
//! on the PCjr, add the palette of 16 colours, the border, and the 16-colour
//! modes: 160x200 and 320x200 at two pixels a byte, and on the Tandy 4
//! colours at 640x200 in pairs of bytes. The Tandy keeps the CGA's Mode
//! Control and Color Select registers at 3D8h and 3D9h; the PCjr has its
//! own mode control in the gate array. As in DOSBox (vga_other.cpp,
//! vga_memory.cpp and vga_draw.cpp), which this follows.

use super::VideoMode;
use super::adapter::Adapter;
use super::palette::rgbi;
use super::vga::VgaCard;
use crate::bus::Bus;

/// The ports of the Tandy's video: the 6845 and its mirrors, Mode Control,
/// Color Select, the status and the gate array's index, its data, and the
/// page register.
pub const TANDY_PORTS: &[u16] =
    &[0x3D0, 0x3D1, 0x3D2, 0x3D3, 0x3D4, 0x3D5, 0x3D6, 0x3D7, 0x3D8, 0x3D9, 0x3DA, 0x3DE, 0x3DF];
/// The PCjr's: the 6845, the gate array at 3DAh and the page register.
pub const PCJR_PORTS: &[u16] = &[0x3D0, 0x3D1, 0x3D2, 0x3D3, 0x3D4, 0x3D5, 0x3D6, 0x3D7, 0x3DA, 0x3DF];

/// Where the Tandy's 128 KB of video memory starts: the top of 640 KB.
pub const TANDY_BANK: usize = 0x80000;
/// The size of a page of it.
pub const PAGE: usize = 0x4000;

/// The 6845 registers of the 32 KB modes 09h and 0Ah: 80 characters of 8
/// dots a row, 50 rows of 4 scanlines, 262 lines in all.
#[rustfmt::skip]
const CRTC_32K: [u8; 16] = [0x71, 0x50, 0x5A, 0x0A, 0x3F, 0x06, 0x32, 0x38, 0x02, 0x03, 0x06, 0x07, 0, 0, 0, 0];

/// Mode Control (3D8h on the Tandy, gate array register 0 on the PCjr) as
/// the BIOS sets it for modes 0-0Ah.
const TANDY_MODE_CONTROL: [u8; 11] = [0x2C, 0x28, 0x2D, 0x29, 0x2A, 0x2E, 0x1E, 0x29, 0x2A, 0x2B, 0x3B];
const PCJR_MODE_CONTROL: [u8; 11] = [0x0C, 0x08, 0x0D, 0x09, 0x0A, 0x0E, 0x0E, 0x09, 0x1A, 0x1B, 0x0B];

/// The gate array's registers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GateArray {
    /// The register the next data write goes to.
    pub index: u8,
    /// PCjr: whether the next write to 3DAh is data (reading 3DAh makes it
    /// an index again).
    pub flip_flop: bool,
    /// The bits of a colour that pick its palette register (1).
    pub palette_mask: u8,
    pub border: u8,
    /// Mode Control 2 (3): 16 colours (Tandy bit 4), 4 colours at 640x200
    /// (Tandy bit 3), 2 colours at 640x200 (PCjr bit 3), blinking (PCjr
    /// bit 1).
    pub mode2: u8,
    /// Tandy: the extended RAM register (5).
    pub ext: u8,
    /// The palette registers (10h-1Fh): an RGBI colour each.
    pub palette: [u8; 16],
    /// The CRT/processor page register (3DFh): bits 0-2 the page the
    /// picture comes from, 3-5 the one the processor sees at B8000h, 6-7
    /// how the scanlines are spread over 8 KB banks.
    pub page: u8,
}

impl Default for GateArray {
    fn default() -> Self {
        Self {
            index: 0,
            flip_flop: false,
            palette_mask: 0x0F,
            border: 0,
            mode2: 0,
            ext: 0,
            palette: std::array::from_fn(|i| i as u8),
            page: 0x3F,
        }
    }
}

/// What the gate array shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Graphics {
    Text,
    /// 2 colours at 640x200, a bit a pixel.
    Two,
    /// 4 colours at 320x200, two bits a pixel.
    Four,
    /// 4 colours at 640x200: a byte of each pixel's low bits and one of
    /// its high bits for every 8 pixels.
    FourHigh,
    /// 16 colours at 160x200 or 320x200, a nibble a pixel.
    Sixteen,
}

/// An RGBI colour as 8-bit RGB.
fn rgb(color: u8) -> (u8, u8, u8) {
    let [r, g, b] = rgbi(color & 0x0F);
    (r << 2 | r >> 4, g << 2 | g >> 4, b << 2 | b >> 4)
}

impl VgaCard {
    fn is_tandy(&self) -> bool {
        self.adapter == Adapter::Tandy
    }

    /// Set the registers as the BIOS does for `mode` (0-6, 8-0Ah).
    pub(super) fn tandy_set_mode(&mut self, mode: VideoMode) {
        let number = (mode as usize).min(0x0A);
        let crtc = match mode {
            VideoMode::Text40x25 | VideoMode::Text40x25Color => &super::cga::CRTC_40,
            VideoMode::Text80x25 | VideoMode::Text80x25Color => &super::cga::CRTC_80,
            VideoMode::Tandy320x200x16 | VideoMode::Tandy640x200x4 => &CRTC_32K,
            _ => &super::cga::CRTC_GRAPHICS,
        };
        self.crtc_regs = [0; 25];
        self.crtc_regs[..16].copy_from_slice(crtc);
        self.cga_color = if matches!(number, 0x06 | 0x0A) { 0x3F } else { 0x30 };
        let mut ga = GateArray::default();
        if self.is_tandy() {
            self.cga_mode = TANDY_MODE_CONTROL[number];
            ga.mode2 = match number {
                0x08 | 0x09 => 0x14,
                0x0A => 0x0C,
                _ => 0,
            };
            ga.page = if number >= 0x09 { 0xF6 } else { 0x3F };
        } else {
            self.cga_mode = PCJR_MODE_CONTROL[number];
            ga.mode2 = match number {
                0..=4 => 0x02,
                0x06 => 0x08,
                _ => 0,
            };
            ga.page = match number {
                0..=3 => 0x3F,
                0x09.. => 0xF6,
                _ => 0x7F,
            };
            // The PCjr's BIOS sets the palette for the CGA modes' colours:
            // mode 4's cyan, magenta and white, mode 6's white.
            match number {
                0x04 | 0x05 => ga.palette[1..4].copy_from_slice(&[0x03, 0x05, 0x0F]),
                0x06 => ga.palette[1] = 0x0F,
                _ => {}
            }
        }
        self.tandy = ga;
        self.cga_changed();
    }

    pub(super) fn tandy_io_read(&mut self, port: u16) -> u8 {
        match port {
            0x3D0..=0x3D7 => self.cga_io_read(port),
            _ => 0xFF,
        }
    }

    pub(super) fn tandy_io_write(&mut self, port: u16, value: u8) {
        match port {
            0x3D0..=0x3D7 => self.cga_io_write(port, value),
            0x3D8 if self.is_tandy() => {
                self.cga_mode = value & 0x3F;
                self.cga_changed();
            }
            0x3D9 if self.is_tandy() => {
                self.cga_color = value;
                self.mark_dirty_full();
            }
            0x3DA if self.is_tandy() => self.tandy.index = value,
            0x3DE if self.is_tandy() => self.gate_array_write(value),
            0x3DA => {
                if self.tandy.flip_flop {
                    self.gate_array_write(value);
                } else {
                    // An index with bit 4 set (a palette register) blanks
                    // the picture until another is written.
                    self.tandy.index = value;
                    self.mark_dirty_full();
                }
                self.tandy.flip_flop = !self.tandy.flip_flop;
            }
            0x3DF => {
                self.tandy.page = value;
                self.mark_dirty_full();
            }
            _ => {}
        }
    }

    fn gate_array_write(&mut self, value: u8) {
        match self.tandy.index & 0x1F {
            0x00 if !self.is_tandy() => {
                self.cga_mode = value;
                self.cga_changed();
            }
            0x01 => self.tandy.palette_mask = value,
            0x02 => self.tandy.border = value,
            0x03 => {
                self.tandy.mode2 = value;
                self.cga_changed();
            }
            0x05 => self.tandy.ext = value,
            index @ 0x10..=0x1F => self.tandy.palette[index as usize - 0x10] = value & 0x0F,
            _ => {}
        }
        self.mark_dirty_full();
    }

    /// Reading the status register (3DAh) makes the PCjr's next write to
    /// it an index.
    pub fn gate_array_status_read(&mut self) {
        if self.adapter == Adapter::Pcjr {
            self.tandy.flip_flop = false;
        }
    }

    /// What the registers show: text, or which of the graphics.
    pub fn gate_array_graphics(&self) -> Graphics {
        let (mode, mode2) = (self.cga_mode, self.tandy.mode2);
        if mode & 0x02 == 0 {
            return Graphics::Text;
        }
        if self.is_tandy() {
            if mode2 & 0x10 != 0 {
                Graphics::Sixteen
            } else if mode2 & 0x08 != 0 {
                Graphics::FourHigh
            } else if mode & 0x10 != 0 {
                Graphics::Two
            } else {
                Graphics::Four
            }
        } else if mode & 0x10 != 0 {
            Graphics::Sixteen
        } else if mode2 & 0x08 != 0 {
            Graphics::Two
        } else if mode & 0x01 != 0 {
            Graphics::FourHigh
        } else {
            Graphics::Four
        }
    }

    /// The mode the registers set, for programs that program the gate
    /// array without the BIOS.
    pub fn tandy_video_mode(&self) -> VideoMode {
        match self.gate_array_graphics() {
            Graphics::Text => self.cga_video_mode(),
            Graphics::Sixteen if self.cga_mode & 0x01 != 0 => VideoMode::Tandy320x200x16,
            Graphics::Sixteen => VideoMode::Tandy160x200x16,
            Graphics::FourHigh => VideoMode::Tandy640x200x4,
            Graphics::Two => VideoMode::Cga640x200,
            Graphics::Four if self.cga_mode & 0x04 != 0 => VideoMode::Cga320x200,
            Graphics::Four => VideoMode::Cga320x200Color,
        }
    }

    /// Whether the picture is on: Mode Control bit 3, and on the PCjr no
    /// palette register being written.
    pub fn gate_array_enabled(&self) -> bool {
        self.cga_mode & 0x08 != 0 && !(self.adapter == Adapter::Pcjr && self.tandy.index & 0x10 != 0)
    }

    /// Whether attribute bit 7 blinks characters: the Tandy's Mode Control
    /// bit 5, the PCjr's Mode Control 2 bit 1.
    pub(super) fn gate_array_blinks(&self) -> bool {
        if self.cga_mode & 0x02 != 0 {
            return false;
        }
        if self.is_tandy() { self.cga_mode & 0x20 != 0 } else { self.tandy.mode2 & 0x02 != 0 }
    }

    /// Where the video memory starts in system memory.
    pub fn gate_array_base(&self) -> usize {
        if self.is_tandy() { TANDY_BANK } else { 0 }
    }

    /// How the scanlines of a character row are spread over 8 KB banks: a
    /// mask of the scanline's low bits, 0 for one bank. The graphics modes
    /// have at least two.
    pub fn gate_array_line_mask(&self) -> usize {
        if self.is_tandy() && self.tandy.ext & 0x01 != 0 {
            return 0;
        }
        let mask = (self.tandy.page >> 6) as usize;
        if self.cga_mode & 0x02 != 0 { mask | 1 } else { mask }
    }

    /// The memory the picture comes from: where it starts in system memory,
    /// and its size, a page or (with four banks) a pair.
    pub fn crt_range(&self) -> (usize, usize) {
        let pair = (self.tandy.page >> 6) & 0x02 != 0;
        let page = (self.tandy.page & if pair { 0x06 } else { 0x07 }) as usize;
        (self.gate_array_base() + page * PAGE, if pair { 2 * PAGE } else { PAGE })
    }

    /// The memory the processor sees at B8000h: where it starts and how
    /// much of it there is before it repeats. The Tandy maps an even page
    /// with the one after it, 32 KB; an odd page, and any on the PCjr, is
    /// 16 KB twice.
    pub fn cpu_range(&self) -> (usize, usize) {
        let page = ((self.tandy.page >> 3) & 0x07) as usize;
        let size = if self.is_tandy() && page & 1 == 0 { 2 * PAGE } else { PAGE };
        (self.gate_array_base() + page * PAGE, size)
    }

    /// Where the processor's access to `addr` goes in system memory, when
    /// it is in the window at B8000h of a gate array.
    #[inline]
    pub fn cpu_window(&self, addr: usize) -> Option<usize> {
        if !self.adapter.gate_array() || !(0xB8000..0xC0000).contains(&addr) {
            return None;
        }
        let (base, size) = self.cpu_range();
        Some(base + ((addr - 0xB8000) & (size - 1)))
    }

    /// The RGB colours of the colour numbers the current mode's pixels
    /// have, through the palette registers.
    pub fn gate_array_colors(&self) -> [(u8, u8, u8); 16] {
        let palette = &self.tandy.palette;
        let p = |i: u8| rgb(palette[(i & 0x0F) as usize]);
        match self.gate_array_graphics() {
            Graphics::Text | Graphics::Sixteen => std::array::from_fn(|i| p(i as u8)),
            Graphics::Two if self.is_tandy() => {
                let fg = p(self.cga_color & 0x0F);
                std::array::from_fn(|i| if i == 0 { p(0) } else { fg })
            }
            // The Tandy maps the four colours of 320x200 to palette registers
            // as the CGA's Color Select register picks its colours: the
            // background, and (by bits 4 and 5, and Mode Control's bit 2)
            // the bright or dark green, red and brown, cyan, magenta and
            // white, or cyan, red and white.
            Graphics::Four if self.is_tandy() => {
                let select = self.cga_color;
                let mut set = 0u8;
                let mut r_mask = 0x0Fu8;
                if select & 0x10 != 0 {
                    set |= 0x08;
                }
                if select & 0x20 != 0 {
                    set |= 0x01;
                }
                if self.cga_mode & 0x04 != 0 {
                    set |= 0x01;
                    r_mask &= !0x01;
                }
                let mask = self.tandy.palette_mask;
                let four = [select & 0x0F, (2 | set) & mask, (4 | (set & r_mask)) & mask, (6 | set) & mask];
                std::array::from_fn(|i| p(four[i & 3]))
            }
            _ => std::array::from_fn(|i| p(i as u8 & 0x03)),
        }
    }
}

/// Draw the graphics of a gate array into the 640x400 `canvas`: each of
/// the CRTC's scanlines (up to 200) twice, its pixels stretched to 640.
pub fn render_graphics(canvas: &mut [u8], bus: &Bus) {
    let vga = &bus.vga;
    if !vga.gate_array_enabled() {
        return;
    }
    let kind = vga.gate_array_graphics();
    let colors = vga.gate_array_colors();
    let (base, size) = vga.crt_range();
    let Some(mem) = bus.ram().get(base..base + size) else { return };
    let regs = &vga.crtc_regs;
    let row_bytes = (regs[1] as usize * 2).clamp(2, 256);
    let scan = (regs[9] & 0x1F) as usize + 1;
    let lines = ((regs[6] & 0x7F) as usize * scan).min(200);
    let line_mask = vga.gate_array_line_mask();
    let (shift, addr_mask) = if line_mask != 0 { (13, 0x1FFF) } else { (0, size - 1) };
    let start = vga.latched_start_addr * 2;
    let width = super::SCREEN_WIDTH as usize;

    let mut pixels: Vec<u8> = Vec::with_capacity(row_bytes * 8);
    for y in 0..lines {
        let (row, ra) = (y / scan, y % scan);
        let bank = (ra & line_mask) << shift;
        let at = |i: usize| mem[(bank + ((start + row * row_bytes + i) & addr_mask)) & (size - 1)];
        pixels.clear();
        match kind {
            Graphics::Text => return,
            Graphics::Sixteen => {
                for i in 0..row_bytes {
                    let byte = at(i);
                    pixels.extend([byte >> 4, byte & 0x0F]);
                }
            }
            Graphics::Four => {
                for i in 0..row_bytes {
                    let byte = at(i);
                    pixels.extend((0..4).map(|p| (byte >> (6 - p * 2)) & 3));
                }
            }
            Graphics::FourHigh => {
                for i in (0..row_bytes).step_by(2) {
                    let (low, high) = (at(i), at(i + 1));
                    pixels.extend((0..8).map(|p| (low >> (7 - p)) & 1 | ((high >> (7 - p)) & 1) << 1));
                }
            }
            Graphics::Two => {
                for i in 0..row_bytes {
                    let byte = at(i);
                    pixels.extend((0..8).map(|p| (byte >> (7 - p)) & 1));
                }
            }
        }
        let scale = (width / pixels.len().max(1)).max(1);
        for dy in 0..2 {
            let row_start = (y * 2 + dy) * width * 3;
            for (x, &color) in pixels.iter().take(width / scale).enumerate() {
                let (r, g, b) = colors[color as usize];
                for dx in 0..scale {
                    let i = row_start + (x * scale + dx) * 3;
                    if i + 2 < canvas.len() {
                        canvas[i] = r;
                        canvas[i + 1] = g;
                        canvas[i + 2] = b;
                    }
                }
            }
        }
    }
}
