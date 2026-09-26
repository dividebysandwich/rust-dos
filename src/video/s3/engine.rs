//! The S3 Trio64's graphics engine, the 8514/A-style accelerator behind
//! ports 82E8h-BEE8h and E2E8h (and their memory-mapped copies): lines,
//! rectangle fills, image transfers from the processor with colour
//! expansion, screen-to-screen copies and pattern fills, into video memory
//! at the screen width and depth CR50 gives it. It runs each command at
//! once, so it is never busy.
//!
//! Ported from DOSBox-X's XGA emulation (vga_xga.cpp), which Windows 95's
//! S3 driver was installed and run against.

/// Video memory as the engine draws in it: the screen's width in pixels
/// and the bytes each takes.
pub struct Surface<'a> {
    pub vram: &'a mut [u8],
    pub width: u32,
    pub bytes: u32,
}

impl Surface<'_> {
    fn mask(&self) -> u32 {
        match self.bytes {
            1 => 0xFF,
            2 => 0xFFFF,
            _ => 0xFFFF_FFFF,
        }
    }

    fn at(&self, x: u32, y: u32) -> Option<usize> {
        let at = (y as usize * self.width as usize + x as usize) * self.bytes as usize;
        (at + self.bytes as usize <= self.vram.len()).then_some(at)
    }

    fn get(&self, x: u32, y: u32) -> u32 {
        let Some(at) = self.at(x, y) else { return 0 };
        let mut value = 0u32;
        for i in 0..self.bytes as usize {
            value |= (self.vram[at + i] as u32) << (8 * i);
        }
        value
    }

    fn put(&mut self, x: u32, y: u32, value: u32) {
        let Some(at) = self.at(x, y) else { return };
        for i in 0..self.bytes as usize {
            self.vram[at + i] = (value >> (8 * i)) as u8;
        }
    }
}

/// An image transfer from the processor in progress (a rectangle command
/// with bit 8 set): where the next pixel goes, and how the data comes.
#[derive(Clone, Copy, Debug, Default)]
struct Transfer {
    active: bool,
    newline: bool,
    x: u16,
    y: u16,
    dx: u16,
    dy: u16,
    x1: u16,
    x2: u16,
    y2: u16,
    /// Bus size: 0 for 8 bits, 0x20 for 16, 0x40 for 32, 0x60 for 32 in
    /// bytes.
    bus: u16,
    /// The first half of a 32-bit pixel sent as two 16-bit words.
    data: u32,
    half: bool,
    /// Swap the bytes of each 16-bit word (CMD bit 12 clear with a 16-bit
    /// bus).
    swap: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Engine {
    /// Scissors: left, top, right, bottom.
    clip: [u16; 4],
    read_mask: u32,
    write_mask: u32,
    fore: u32,
    back: u32,
    color_compare: u32,
    command: u32,
    fore_mix: u16,
    back_mix: u16,
    cur_x: u16,
    cur_y: u16,
    cur_x2: u16,
    cur_y2: u16,
    dest_x: u16,
    dest_y: u16,
    dest_x2: u16,
    dest_y2: u16,
    err_term: u16,
    /// Minor and major axis pixel counts (the rectangle's height and width
    /// less one).
    minor: u16,
    major: u16,
    pix_cntl: u16,
    /// Multifunction control miscellaneous (MULT_MISC) and its second
    /// register, and the next register a multifunction read returns (a
    /// cell, as reads through memory step it too).
    misc: u16,
    misc2: u16,
    read_sel: std::cell::Cell<u16>,
    transfer: Transfer,
}

/// The result of a mix (a raster operation) of source `src` and
/// destination `dst`.
fn mix(mode: u16, src: u32, dst: u32) -> u32 {
    match mode & 0x0F {
        0x00 => !dst,
        0x01 => 0,
        0x02 => 0xFFFF_FFFF,
        0x03 => dst,
        0x04 => !src,
        0x05 => src ^ dst,
        0x06 => !(src ^ dst),
        0x07 => src,
        0x08 => !(src & dst),
        0x09 => !src | dst,
        0x0A => src | !dst,
        0x0B => src | dst,
        0x0C => src & dst,
        0x0D => src & !dst,
        0x0E => !src & dst,
        _ => !(src | dst),
    }
}

impl Engine {
    pub fn new() -> Self {
        Self { clip: [0, 0, 0xFFF, 0xFFF], ..Default::default() }
    }

    /// Whether the processor still owes an image transfer data.
    pub fn busy(&self) -> bool {
        self.transfer.active
    }

    /// The source a mix takes (bits 5-6): the background or foreground
    /// colour, or the pixel given (data from the processor or the
    /// screen).
    fn source(&self, mode: u16, pixel: u32) -> u32 {
        match mode >> 5 & 3 {
            0 => self.back,
            1 => self.fore,
            _ => pixel,
        }
    }

    /// Draw a pixel as the command allows: when it writes (bits 0 and 4),
    /// inside the scissors, through the write mask.
    fn draw(&self, s: &mut Surface, x: u32, y: u32, value: u32) {
        if self.command & 0x11 != 0x11 {
            return;
        }
        let [left, top, right, bottom] = self.clip.map(|c| c as u32);
        if x < left || x > right || y < top || y > bottom {
            return;
        }
        let mask = self.write_mask & s.mask();
        let old = s.get(x, y);
        s.put(x, y, (old & !mask) | (value & mask));
    }

    /// Mix `src` into the pixel at (x, y) with mode `mode`.
    fn mix_point(&self, s: &mut Surface, x: u32, y: u32, mode: u16, src: u32) {
        let dst = s.get(x, y);
        self.draw(s, x, y, mix(mode, src, dst));
    }

    /// A port or memory-mapped register written, `len` bytes of `value`.
    /// Returns whether video memory changed.
    pub fn write(&mut self, port: u16, value: u32, len: u8, s: &mut Surface) -> bool {
        let word = value as u16;
        match port {
            0x82E8 => self.cur_y = word & 0x0FFF,
            0x86E8 => self.cur_x = word & 0x0FFF,
            0x8AE8 => self.dest_y = word & 0x3FFF,
            0x8EE8 => self.dest_x = word & 0x3FFF,
            0x92E8 => self.err_term = word & 0x3FFF,
            0x96E8 => self.major = word & 0x0FFF,
            0x9AE8 => return self.command(value, s),
            0xA2E8 => self.back = self.set_dual(self.back, value, s),
            0xA6E8 => self.fore = self.set_dual(self.fore, value, s),
            0xAAE8 => self.write_mask = self.set_dual(self.write_mask, value, s),
            0xAEE8 => self.read_mask = self.set_dual(self.read_mask, value, s),
            0xB2E8 => self.color_compare = self.set_dual(self.color_compare, value, s),
            0xB6E8 => self.back_mix = word,
            0xBAE8 => self.fore_mix = word,
            0xBEE8 => {
                self.multifunction(word);
                if len == 4 {
                    self.multifunction((value >> 16) as u16);
                }
            }
            // Pixel data (PIX_TRANS), and the image transfer area of the
            // memory-mapped registers.
            0xE2E8 | 0xE2EA | 0x0000..=0x7FFF => {
                self.transfer.newline = false;
                return self.pixel_data(value, len, s);
            }
            // The packed memory-mapped registers.
            0x8100 => {
                self.cur_y = word & 0x0FFF;
                if len == 4 {
                    self.cur_x = (value >> 16) as u16 & 0x0FFF;
                }
            }
            0x8102 => self.cur_x = word & 0x0FFF,
            0x8104 => {
                self.cur_y2 = word & 0x0FFF;
                if len == 4 {
                    self.cur_x2 = (value >> 16) as u16 & 0x0FFF;
                }
            }
            0x8106 => self.cur_x2 = word & 0x0FFF,
            0x8108 => {
                self.dest_y = word & 0x3FFF;
                if len == 4 {
                    self.dest_x = (value >> 16) as u16 & 0x3FFF;
                }
            }
            0x810A => self.dest_x = word & 0x3FFF,
            0x810C => {
                self.dest_y2 = word & 0x3FFF;
                if len == 4 {
                    self.dest_x2 = (value >> 16) as u16 & 0x3FFF;
                }
            }
            0x810E => self.dest_x2 = word & 0x3FFF,
            0x8110 => self.err_term = word & 0x3FFF,
            0x8118 => return self.command(value, s),
            0x8120 => self.back = value,
            0x8124 => self.fore = value,
            0x8128 => self.write_mask = value,
            0x812C => self.read_mask = value,
            0x8130 => self.color_compare = value,
            0x8134 => {
                self.back_mix = word;
                if len == 4 {
                    self.fore_mix = (value >> 16) as u16;
                }
            }
            0x8136 => self.fore_mix = word,
            0x8138 => {
                self.clip[1] = word & 0x0FFF;
                if len == 4 {
                    self.clip[0] = (value >> 16) as u16 & 0x0FFF;
                }
            }
            0x813A => self.clip[0] = word & 0x0FFF,
            0x813C => {
                self.clip[3] = word & 0x0FFF;
                if len == 4 {
                    self.clip[2] = (value >> 16) as u16 & 0x0FFF;
                }
            }
            0x813E => self.clip[2] = word & 0x0FFF,
            0x8140 => {
                self.pix_cntl = word;
                if len == 4 {
                    self.misc2 = (value >> 16) as u16 & 0x0FFF;
                }
            }
            0x8144 => {
                self.misc = word;
                if len == 4 {
                    self.read_sel.set((value >> 16) as u16 & 0x07);
                }
            }
            0x8148 => {
                self.minor = word & 0x0FFF;
                if len == 4 {
                    self.major = (value >> 16) as u16 & 0x0FFF;
                }
            }
            0x814A => self.major = word & 0x0FFF,
            _ => {}
        }
        false
    }

    /// A register read through memory, which leaves the registers that
    /// step on reads (the multifunction and 32-bit colour registers) as
    /// they are.
    pub fn peek(&self, port: u16, len: u8, bytes: u32) -> u32 {
        let value = match port {
            0x9AE8 | 0x8118 => 0x0400,
            0x9AE9 => {
                if self.transfer.active {
                    0x04
                } else {
                    0x00
                }
            }
            0xA2E8 => self.back,
            0xA6E8 => self.fore,
            0xAAE8 => self.write_mask,
            0xAEE8 => self.read_mask,
            0xB2E8 => self.color_compare,
            // Windows 95's S3 driver reads MULT_MISC back this way to set
            // its bit 9.
            0xBEE8 => self.read_multifunction() as u32,
            _ => 0xFFFF_FFFF,
        };
        let value = if bytes < 4 && (0xA2E8..=0xB2E8).contains(&port) { value & 0xFFFF } else { value };
        match len {
            1 => value & 0xFF,
            2 => value & 0xFFFF,
            _ => value,
        }
    }

    /// A register read, `len` bytes.
    pub fn read(&mut self, port: u16, len: u8, s: &Surface) -> u32 {
        let value = match port {
            // Graphics processor status: idle, FIFO empty.
            0x9AE8 | 0x8118 => 0x0400,
            0x9AE9 => {
                if self.transfer.active {
                    0x04
                } else {
                    0x00
                }
            }
            0xA2E8 => self.get_dual(self.back, s),
            0xA6E8 => self.get_dual(self.fore, s),
            0xAAE8 => self.get_dual(self.write_mask, s),
            0xAEE8 => self.get_dual(self.read_mask, s),
            0xB2E8 => self.get_dual(self.color_compare, s),
            0xBEE8 => self.read_multifunction() as u32,
            _ => 0xFFFF_FFFF,
        };
        match len {
            1 => value & 0xFF,
            2 => value & 0xFFFF,
            _ => value,
        }
    }

    /// A colour register written: all of it, or at 32 bits per pixel one
    /// word at a time, low then high, unless MULT_MISC bit 9 has them
    /// written whole.
    fn set_dual(&mut self, old: u32, value: u32, s: &Surface) -> u32 {
        match s.bytes {
            1 => value & 0xFF,
            2 => value & 0xFFFF,
            _ if self.misc & 0x200 != 0 => value,
            _ => {
                let high = self.misc & 0x10 != 0;
                self.misc ^= 0x10;
                if high { (old & 0xFFFF) | (value << 16) } else { (old & 0xFFFF_0000) | (value & 0xFFFF) }
            }
        }
    }

    fn get_dual(&mut self, value: u32, s: &Surface) -> u32 {
        match s.bytes {
            1 => value & 0xFF,
            2 => value & 0xFFFF,
            _ if self.misc & 0x200 != 0 => value,
            _ => {
                self.misc ^= 0x10;
                if self.misc & 0x10 != 0 { value & 0xFFFF } else { value >> 16 }
            }
        }
    }

    /// BEE8h: the register bits 12-15 select, the rest its value.
    fn multifunction(&mut self, value: u16) {
        let data = value & 0x0FFF;
        match value >> 12 {
            0x0 => self.minor = data,
            0x1 => self.clip[1] = data,
            0x2 => self.clip[0] = data,
            0x3 => self.clip[3] = data,
            0x4 => self.clip[2] = data,
            0xA => self.pix_cntl = data,
            0xD => self.misc2 = data,
            0xE => self.misc = data,
            0xF => self.read_sel.set(data),
            _ => {}
        }
    }

    fn read_multifunction(&self) -> u16 {
        let value = match self.read_sel.get() {
            0 => self.minor,
            1 => self.clip[1],
            2 => self.clip[0],
            3 => self.clip[3],
            4 => self.clip[2],
            5 => self.pix_cntl,
            6 => self.misc,
            10 => self.misc2,
            _ => 0,
        };
        self.read_sel.set(self.read_sel.get().wrapping_add(1));
        value
    }

    /// The command register (9AE8h): run a command, or for an image
    /// transfer, get ready for its data.
    fn command(&mut self, value: u32, s: &mut Surface) -> bool {
        self.command = value;
        let mut cmd = (value >> 13) & 7;
        if value & 0x800 != 0 {
            cmd |= 8;
        }
        match cmd {
            1 if value & 0x100 == 0 => {
                if value & 0x08 == 0 {
                    self.line_bresenham(value, s);
                } else {
                    self.line_vector(value, s);
                }
                true
            }
            2 if value & 0x100 == 0 => {
                self.transfer.active = false;
                self.rectangle(value, s);
                true
            }
            2 => {
                let dx = if value & 0x20 != 0 { 1 } else { 0xFFFF };
                let dy = if value & 0x80 != 0 { 1 } else { 0xFFFF };
                self.transfer = Transfer {
                    active: true,
                    newline: true,
                    x: self.cur_x,
                    y: self.cur_y,
                    dx,
                    dy,
                    x1: self.cur_x,
                    x2: (self.cur_x + self.major) & 0x0FFF,
                    y2: (self.cur_y + self.minor + 1) & 0x0FFF,
                    bus: ((value & 0x600) >> 4) as u16,
                    data: 0,
                    half: false,
                    swap: value & 0x1200 == 0x0200,
                };
                false
            }
            // Polygon fill: follow the edges' ends, as DOSBox-X does.
            3 => {
                if self.cur_y < self.dest_y && self.cur_y2 < self.dest_y2 {
                    self.cur_x = self.dest_x;
                    self.cur_y = self.dest_y;
                    self.cur_x2 = self.dest_x2;
                    self.cur_y2 = self.dest_y2;
                } else {
                    if self.cur_y == self.dest_y {
                        self.cur_x = self.dest_x;
                    }
                    if self.cur_y2 == self.dest_y2 {
                        self.cur_x2 = self.dest_x2;
                    }
                }
                false
            }
            6 => {
                self.blit(value, s);
                true
            }
            7 => {
                self.pattern(value, s);
                true
            }
            _ => false,
        }
    }

    /// The source of a line's or rectangle's pixels: the foreground mix's.
    fn solid_source(&self) -> (u16, u32) {
        let mode = self.fore_mix;
        (mode, self.source(mode, 0))
    }

    /// Short-stroke style lines in one of eight directions (bits 5-7), the
    /// major axis count long.
    fn line_vector(&mut self, value: u32, s: &mut Surface) {
        let (sx, sy): (i32, i32) = match (value >> 5) & 7 {
            0 => (1, 0),
            1 => (1, -1),
            2 => (0, -1),
            3 => (-1, -1),
            4 => (-1, 0),
            5 => (-1, 1),
            6 => (0, 1),
            _ => (1, 1),
        };
        let mut count = self.major as i32;
        if value & 0x04 != 0 {
            if count == 0 {
                return;
            }
            count -= 1;
        }
        let (mut x, mut y) = (self.cur_x as i32, self.cur_y as i32);
        if (self.pix_cntl >> 6) & 3 == 0 {
            let (mode, src) = self.solid_source();
            for _ in 0..=count {
                self.mix_point(s, x as u32, y as u32, mode, src);
                x += sx;
                y += sy;
            }
        }
        self.cur_x = (x - 1) as u16;
        self.cur_y = y as u16;
    }

    /// Lines with Bresenham's algorithm from the axial and diagonal step
    /// constants (DESTY, DESTX) and the error term.
    fn line_bresenham(&mut self, value: u32, s: &mut Surface) {
        let sign13 = |v: u16| if v & 0x2000 != 0 { v as i32 | !0x1FFF } else { v as i32 };
        let minor = sign13(self.dest_y) >> 1;
        let major = -(sign13(self.dest_x) - (minor << 1)) >> 1;
        let mut sx = if value & 0x20 != 0 { 1 } else { -1 };
        let mut sy = if value & 0x80 != 0 { 1 } else { -1 };
        let mut e = sign13(self.err_term);
        let (mut x, mut y) = (self.cur_x as i32, self.cur_y as i32);
        let steep = value & 0x40 == 0;
        if !steep {
            std::mem::swap(&mut x, &mut y);
            std::mem::swap(&mut sx, &mut sy);
        }
        let run = self.major as i32 - if value & 0x04 != 0 { 1 } else { 0 };
        let solid = (self.pix_cntl >> 6) & 3 == 0;
        let (mode, src) = self.solid_source();
        for _ in 0..=run {
            if solid {
                let (px, py) = if steep { (x, y) } else { (y, x) };
                self.mix_point(s, px as u32, py as u32, mode, src);
            }
            while e > 0 {
                y += sy;
                e -= major << 1;
            }
            x += sx;
            e += minor << 1;
        }
        let (px, py) = if steep { (x, y) } else { (y, x) };
        self.cur_x = px as u16;
        self.cur_y = py as u16;
    }

    /// Fill a rectangle from the current position, width major + 1 and
    /// height minor + 1, in the directions of bits 5 and 7.
    fn rectangle(&mut self, value: u32, s: &mut Surface) {
        let dx = if value & 0x20 != 0 { 1 } else { -1 };
        let dy = if value & 0x80 != 0 { 1 } else { -1 };
        let mut run = self.major as i32;
        if value & 0x04 != 0 {
            if run == 0 {
                return;
            }
            run -= 1;
        }
        let solid = (self.pix_cntl >> 6) & 3 == 0;
        let (mode, src) = self.solid_source();
        let mut y = self.cur_y as i32;
        let mut x = self.cur_x as i32;
        for _ in 0..=self.minor {
            x = self.cur_x as i32;
            for _ in 0..=run {
                if solid {
                    self.mix_point(s, x as u32, y as u32, mode, src);
                }
                x += dx;
            }
            y += dy;
        }
        self.cur_x = x as u16;
        self.cur_y = y as u16;
    }

    /// The mix for a pixel whose mask bit comes from the screen (PIX_CNTL
    /// bits 6-7 = 11b): the foreground mix where all the read-enabled bits
    /// of `pixel` are set.
    fn screen_mix(&self, pixel: u32) -> u16 {
        if pixel & self.read_mask == self.read_mask { self.fore_mix } else { self.back_mix }
    }

    /// Copy a rectangle from the current position to the destination, in
    /// the directions of bits 5 and 7, skipping pixels by colour compare.
    fn blit(&mut self, value: u32, s: &mut Surface) {
        let dx: i32 = if value & 0x20 != 0 { 1 } else { -1 };
        let dy: i32 = if value & 0x80 != 0 { 1 } else { -1 };
        let compare = self.color_compare & s.mask();
        let select = (self.pix_cntl >> 6) & 3;
        let (mut sy, mut ty) = (self.cur_y as i32, self.dest_y as i32);
        for _ in 0..=self.minor {
            let (mut sx, mut tx) = (self.cur_x as i32, self.dest_x as i32);
            for _ in 0..=self.major {
                let pixel = s.get(sx as u32, sy as u32);
                let mode = match select {
                    0 => self.fore_mix,
                    3 => self.screen_mix(pixel),
                    _ => 0x67,
                };
                let src = self.source(mode, pixel);
                // MULT_MISC bit 8: compare colours, leaving pixels equal
                // to it (or with bit 7, unequal) alone.
                let write = self.misc & 0x100 == 0 || ((src != compare) as u16 ^ (self.misc >> 7 & 1)) != 0;
                if write {
                    self.mix_point(s, tx as u32, ty as u32, mode, src);
                }
                sx += dx;
                tx += dx;
            }
            sy += dy;
            ty += dy;
        }
    }

    /// Fill a rectangle at the destination with the 8x8 pattern at the
    /// current position.
    fn pattern(&mut self, value: u32, s: &mut Surface) {
        let dx: i32 = if value & 0x20 != 0 { 1 } else { -1 };
        let dy: i32 = if value & 0x80 != 0 { 1 } else { -1 };
        let select = (self.pix_cntl >> 6) & 3;
        let (px, py) = (self.cur_x as i32, self.cur_y as i32);
        let mut ty = self.dest_y as i32;
        for _ in 0..=self.minor {
            let mut tx = self.dest_x as i32;
            for _ in 0..=self.major {
                let pixel = s.get((px + (tx & 7)) as u32, (py + (ty & 7)) as u32);
                let mode = match select {
                    0 => self.fore_mix,
                    3 => self.screen_mix(pixel),
                    _ => 0x67,
                };
                let src = self.source(mode, pixel);
                self.mix_point(s, tx as u32, ty as u32, mode, src);
                tx += dx;
            }
            ty += dy;
        }
    }

    /// Move the image transfer on a pixel, to the next row after the last
    /// of one (DOSBox-X's `XGA_CheckX`).
    fn advance(&mut self) {
        let t = &mut self.transfer;
        if t.newline {
            t.newline = false;
            return;
        }
        let next_row = if t.x < 2048 {
            t.x > t.x2
        } else if t.x2 > 2047 {
            4096 - t.x == 4096 - t.x2
        } else {
            4096 - t.x == t.x2
        };
        if next_row {
            t.x = t.x1;
            t.y = t.y.wrapping_add(t.dy) & 0x0FFF;
            t.newline = true;
            if t.y < 2048 && t.y > t.y2 {
                t.active = false;
            }
        }
    }

    /// Mix one pixel of an image transfer at its position and move on.
    fn transfer_pixel(&mut self, s: &mut Surface, mode: u16, src: u32) {
        let (x, y) = (self.transfer.x as u32, self.transfer.y as u32);
        self.mix_point(s, x, y, mode, src);
        self.transfer.x = self.transfer.x.wrapping_add(self.transfer.dx) & 0x0FFF;
        self.advance();
    }

    /// Data for an image transfer from the processor: pixels, or with
    /// PIX_CNTL bits 6-7 = 10b a bit for each pixel choosing the
    /// foreground or background mix.
    fn pixel_data(&mut self, value: u32, len: u8, s: &mut Surface) -> bool {
        if !self.transfer.active {
            return false;
        }
        let mut value = value;
        if self.transfer.swap && len >= 2 {
            value = ((value & 0xFF00_FF00) >> 8) | ((value & 0x00FF_00FF) << 8);
        }
        match (self.pix_cntl >> 6) & 3 {
            0 => {
                let mode = self.fore_mix;
                if mode >> 5 & 3 != 2 {
                    return false;
                }
                match (s.bytes, self.transfer.bus) {
                    (1, 0x00) => self.transfer_pixel(s, mode, value & 0xFF),
                    (1, 0x20) => {
                        for i in 0..len as u32 {
                            self.transfer_pixel(s, mode, value >> (8 * i) & 0xFF);
                            if self.transfer.newline {
                                break;
                            }
                        }
                    }
                    (1, _) => {
                        for i in 0..4 {
                            self.transfer_pixel(s, mode, value >> (8 * i) & 0xFF);
                        }
                    }
                    (4, 0x20) if len != 4 => {
                        if !self.transfer.half {
                            self.transfer.data = value & 0xFFFF;
                            self.transfer.half = true;
                            return false;
                        }
                        let pixel = (value << 16) | self.transfer.data;
                        self.transfer.half = false;
                        self.transfer_pixel(s, mode, pixel);
                    }
                    (4, _) => self.transfer_pixel(s, mode, value),
                    (2, 0x20) => self.transfer_pixel(s, mode, value & 0xFFFF),
                    (2, _) => {
                        self.transfer_pixel(s, mode, value & 0xFFFF);
                        if !self.transfer.newline {
                            self.transfer_pixel(s, mode, value >> 16);
                        }
                    }
                    _ => {}
                }
                true
            }
            2 => {
                let (chunk, chunks) = match self.transfer.bus {
                    0x00 => (8, 1),
                    0x20 => (16, if len == 4 { 2 } else { 1 }),
                    0x40 => (32, 1),
                    _ => (8, len as u32),
                };
                'chunks: for k in 0..chunks {
                    self.transfer.newline = false;
                    for n in 0..chunk {
                        // Each byte's pixels from its top bit down.
                        let bit = (n & 0xF8) + (7 - (n & 7)) + chunk * k;
                        let mode = if value >> bit & 1 != 0 { self.fore_mix } else { self.back_mix };
                        let src = match mode >> 5 & 3 {
                            0 => self.back,
                            1 => self.fore,
                            _ => 0,
                        };
                        self.transfer_pixel(s, mode, src);
                        if self.transfer.y < 2048 && self.transfer.y >= self.transfer.y2 {
                            self.transfer.active = false;
                            break 'chunks;
                        }
                        // The next chunk goes on the next row.
                        if self.transfer.newline {
                            break;
                        }
                    }
                }
                true
            }
            _ => false,
        }
    }
}

crate::state_fields!(Engine {
    clip, read_mask, write_mask, fore, back, color_compare, command, fore_mix, back_mix, cur_x, cur_y, cur_x2,
    cur_y2, dest_x, dest_y, dest_x2, dest_y2, err_term, minor, major, pix_cntl, misc, misc2, read_sel,
} skip {
    transfer,
});

#[cfg(test)]
mod tests {
    use super::*;

    fn surface(vram: &mut [u8]) -> Surface<'_> {
        Surface { vram, width: 16, bytes: 1 }
    }

    fn engine() -> Engine {
        let mut e = Engine::new();
        e.write_mask = 0xFF;
        e.read_mask = 0xFF;
        e
    }

    /// Windows 95's S3 driver selects MULT_MISC for reading, reads it
    /// through memory and writes it back with bit 9 set.
    #[test]
    fn multifunction_registers_read_back_through_memory() {
        let mut vram = vec![0; 256];
        let mut e = engine();
        e.write(0xBEE8, 0xE010, 2, &mut surface(&mut vram));
        e.write(0xBEE8, 0xF006, 2, &mut surface(&mut vram));
        let misc = e.peek(0xBEE8, 2, 1);
        assert_eq!(misc, 0x0010);
        e.write(0x8144, (misc & 0x0FFF) | 0xE200, 2, &mut surface(&mut vram));
        assert_eq!(e.misc & 0x0FFF, 0x0210, "no colour compare");
        // The next read is the next register, as through the port.
        e.write(0xBEE8, 0xF005, 2, &mut surface(&mut vram));
        e.write(0xBEE8, 0xA0C0, 2, &mut surface(&mut vram));
        assert_eq!(e.peek(0xBEE8, 2, 1), 0x00C0);
        assert_eq!(e.peek(0xBEE8, 2, 1) & 0x0FFF, 0x0210);
    }

    #[test]
    fn a_rectangle_fills_with_the_foreground_colour() {
        let mut vram = vec![0u8; 256];
        let mut s = surface(&mut vram);
        let mut e = engine();
        e.fore = 0x55;
        e.fore_mix = 0x27; // foreground colour, SRC
        e.cur_x = 2;
        e.cur_y = 1;
        e.major = 3; // 4 wide
        e.minor = 1; // 2 high
        e.write(0x9AE8, 0x40B3, 2, &mut s); // rectangle, +X +Y, draw, write
        assert_eq!(&vram[16..24], &[0, 0, 0x55, 0x55, 0x55, 0x55, 0, 0]);
        assert_eq!(&vram[32..40], &[0, 0, 0x55, 0x55, 0x55, 0x55, 0, 0]);
        assert_eq!(vram[48 + 2], 0);
    }

    #[test]
    fn a_blit_copies_overlapping_rectangles_in_the_direction_given() {
        let mut vram: Vec<u8> = (0..=255u8).collect();
        let mut s = surface(&mut vram);
        let mut e = engine();
        e.fore_mix = 0x67; // bitmap data, SRC
        e.pix_cntl = 0;
        // Copy (0,0)-(3,0) one pixel right, right to left.
        e.cur_x = 3;
        e.cur_y = 0;
        e.dest_x = 4;
        e.dest_y = 0;
        e.major = 3;
        e.minor = 0;
        e.write(0x9AE8, 0xC093, 2, &mut s); // blit, -X +Y
        assert_eq!(&vram[0..6], &[0, 0, 1, 2, 3, 5]);
    }

    #[test]
    fn mono_image_data_expands_to_the_two_colours() {
        let mut vram = vec![0u8; 256];
        let mut s = surface(&mut vram);
        let mut e = engine();
        e.fore = 0x0F;
        e.back = 0x01;
        e.fore_mix = 0x27;
        e.back_mix = 0x07;
        e.pix_cntl = 0x80; // the mix from the processor's data
        e.cur_x = 0;
        e.cur_y = 0;
        e.major = 7;
        e.minor = 0;
        e.write(0x9AE8, 0x41B3, 2, &mut s); // rectangle with data, 8-bit bus
        assert!(e.busy());
        e.write(0xE2E8, 0b1010_0001, 1, &mut s);
        assert_eq!(&vram[0..8], &[0x0F, 0x01, 0x0F, 0x01, 0x01, 0x01, 0x01, 0x0F]);
    }
}
