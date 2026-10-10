//! The V1000's RAMDAC, a Brooktree Bt485 at ports B0h-BFh of the card's:
//! the palette, the command registers and the hardware cursor, as
//! xf86-video-rendition's `vramdac.c` programs them. The VGA's own DAC
//! ports reach the same palette.

/// The registers, as offsets from B0h.
mod reg {
    pub const WRITE_ADDR: u8 = 0x0;
    pub const DATA: u8 = 0x1;
    pub const PIXEL_MASK: u8 = 0x2;
    pub const READ_ADDR: u8 = 0x3;
    pub const CURSOR_WRITE_ADDR: u8 = 0x4;
    pub const CURSOR_DATA: u8 = 0x5;
    pub const COMMAND_0: u8 = 0x6;
    pub const CURSOR_READ_ADDR: u8 = 0x7;
    pub const COMMAND_1: u8 = 0x8;
    pub const COMMAND_2: u8 = 0x9;
    /// The status register, or command register 3 when command register
    /// 0's bit 7 is set and the address register holds 1.
    pub const STATUS: u8 = 0xA;
    pub const CURSOR_RAM: u8 = 0xB;
    pub const CURSOR_X_LOW: u8 = 0xC;
    pub const CURSOR_X_HIGH: u8 = 0xD;
    pub const CURSOR_Y_LOW: u8 = 0xE;
    pub const CURSOR_Y_HIGH: u8 = 0xF;
}

#[derive(Clone, Debug)]
pub struct Bt485 {
    /// The address register: a palette entry, or the cursor RAM's address
    /// (its low 8 bits; command register 3 has bits 8-9).
    pub address: u8,
    /// Which of red, green and blue the palette port reads or writes next.
    pub step: u8,
    /// The palette entry a read gets.
    pub read_address: u8,
    pub command: [u8; 4],
    pub pixel_mask: u8,
    /// The cursor's colour registers' address and step, and their colours
    /// (0 is the overscan colour), RGB.
    pub cursor_address: u8,
    pub cursor_step: u8,
    pub cursor_colours: [u8; 12],
    /// 64x64 pixels of two bits: the first plane, then the second, 512
    /// bytes each, the leftmost pixel in a byte's top bit; a 32x32 cursor
    /// uses 128 bytes of each.
    pub cursor_ram: Vec<u8>,
    /// The cursor's lower right corner, plus one, on the screen.
    pub cursor_x: u16,
    pub cursor_y: u16,
}

impl Default for Bt485 {
    fn default() -> Self {
        Self {
            address: 0,
            step: 0,
            read_address: 0,
            command: [0; 4],
            pixel_mask: 0xFF,
            cursor_address: 0,
            cursor_step: 0,
            cursor_colours: [0; 12],
            cursor_ram: vec![0; 1024],
            cursor_x: 0,
            cursor_y: 0,
        }
    }
}

crate::state_fields!(Bt485 {
    address,
    step,
    read_address,
    command,
    pixel_mask,
    cursor_address,
    cursor_step,
    cursor_colours,
    cursor_ram,
    cursor_x,
    cursor_y,
});

/// What the cursor does to a pixel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorPixel {
    Transparent,
    /// One of the cursor colour registers, 1 to 3.
    Colour(usize),
    Invert,
}

impl Bt485 {
    /// Whether command register 3 is what the status port reaches.
    fn command_3_selected(&self) -> bool {
        self.command[0] & 0x80 != 0 && self.address == 1
    }

    /// The cursor RAM's address: bits 8-9 from command register 3.
    fn cursor_ram_address(&self) -> usize {
        ((self.command[3] as usize & 3) << 8) | self.address as usize
    }

    fn advance_cursor_ram(&mut self) {
        let next = (self.cursor_ram_address() + 1) & 0x3FF;
        self.address = next as u8;
        self.command[3] = (self.command[3] & !3) | (next >> 8) as u8;
    }

    /// Port B0h + `offset` written. The palette's entries go to and come
    /// from `palette` (256 RGB triples, in `width` bits).
    pub fn write(&mut self, offset: u8, value: u8, palette: &mut [u8]) {
        match offset {
            reg::WRITE_ADDR => {
                self.address = value;
                self.step = 0;
            }
            reg::DATA => {
                palette[self.address as usize * 3 + self.step as usize] = value;
                self.step += 1;
                if self.step == 3 {
                    self.step = 0;
                    self.address = self.address.wrapping_add(1);
                }
            }
            reg::PIXEL_MASK => self.pixel_mask = value,
            reg::READ_ADDR => {
                self.read_address = value;
                self.step = 0;
            }
            reg::CURSOR_WRITE_ADDR | reg::CURSOR_READ_ADDR => {
                self.cursor_address = value & 3;
                self.cursor_step = 0;
            }
            reg::CURSOR_DATA => {
                self.cursor_colours[self.cursor_address as usize * 3 + self.cursor_step as usize] = value;
                self.cursor_step += 1;
                if self.cursor_step == 3 {
                    self.cursor_step = 0;
                    self.cursor_address = (self.cursor_address + 1) & 3;
                }
            }
            reg::COMMAND_0 => self.command[0] = value,
            reg::COMMAND_1 => self.command[1] = value,
            reg::COMMAND_2 => self.command[2] = value,
            reg::STATUS if self.command_3_selected() => self.command[3] = value,
            reg::STATUS => {}
            reg::CURSOR_RAM => {
                let at = self.cursor_ram_address();
                self.cursor_ram[at] = value;
                self.advance_cursor_ram();
            }
            reg::CURSOR_X_LOW => self.cursor_x = (self.cursor_x & 0xF00) | value as u16,
            reg::CURSOR_X_HIGH => self.cursor_x = (self.cursor_x & 0xFF) | ((value as u16 & 0xF) << 8),
            reg::CURSOR_Y_LOW => self.cursor_y = (self.cursor_y & 0xF00) | value as u16,
            reg::CURSOR_Y_HIGH => self.cursor_y = (self.cursor_y & 0xFF) | ((value as u16 & 0xF) << 8),
            _ => {}
        }
    }

    /// Port B0h + `offset` read.
    pub fn read(&mut self, offset: u8, palette: &[u8]) -> u8 {
        match offset {
            reg::WRITE_ADDR => self.address,
            reg::DATA => {
                let value = palette[self.read_address as usize * 3 + self.step as usize];
                self.step += 1;
                if self.step == 3 {
                    self.step = 0;
                    self.read_address = self.read_address.wrapping_add(1);
                }
                value
            }
            reg::PIXEL_MASK => self.pixel_mask,
            reg::READ_ADDR => self.read_address,
            reg::CURSOR_WRITE_ADDR | reg::CURSOR_READ_ADDR => self.cursor_address,
            reg::CURSOR_DATA => {
                let value = self.cursor_colours[self.cursor_address as usize * 3 + self.cursor_step as usize];
                self.cursor_step += 1;
                if self.cursor_step == 3 {
                    self.cursor_step = 0;
                    self.cursor_address = (self.cursor_address + 1) & 3;
                }
                value
            }
            reg::COMMAND_0 => self.command[0],
            reg::COMMAND_1 => self.command[1],
            reg::COMMAND_2 => self.command[2],
            // The status: the ID of a Bt485 in bits 7-4.
            reg::STATUS if self.command_3_selected() => self.command[3],
            reg::STATUS => 0x80,
            reg::CURSOR_RAM => {
                let value = self.cursor_ram[self.cursor_ram_address()];
                self.advance_cursor_ram();
                value
            }
            reg::CURSOR_X_LOW => self.cursor_x as u8,
            reg::CURSOR_X_HIGH => (self.cursor_x >> 8) as u8,
            reg::CURSOR_Y_LOW => self.cursor_y as u8,
            reg::CURSOR_Y_HIGH => (self.cursor_y >> 8) as u8,
            _ => 0xFF,
        }
    }

    /// Whether the palette's entries are 8 bits (command register 0 bit 1)
    /// rather than 6.
    pub fn dac_8bit(&self) -> bool {
        self.command[0] & 0x02 != 0
    }

    /// The cursor's size in pixels, if it is on (command register 2's
    /// bits 1-0): 64 or 32 (command register 3 bit 2).
    pub fn cursor_size(&self) -> Option<u32> {
        if self.command[2] & 3 == 0 {
            return None;
        }
        Some(if self.command[3] & 0x04 != 0 { 64 } else { 32 })
    }

    /// Where the cursor's top left pixel is on the screen.
    pub fn cursor_origin(&self) -> Option<(i32, i32)> {
        let size = self.cursor_size()? as i32;
        Some((self.cursor_x as i32 - size, self.cursor_y as i32 - size))
    }

    /// The cursor's pixel (`cx`, `cy`): its two bits, the second plane's
    /// high, by the cursor's mode.
    pub fn cursor_pixel(&self, cx: u32, cy: u32) -> CursorPixel {
        let Some(size) = self.cursor_size() else {
            return CursorPixel::Transparent;
        };
        let plane = (size * size / 8) as usize;
        let bit = (cy * size + cx) as usize;
        let mask = 0x80 >> (bit % 8);
        let p0 = self.cursor_ram[bit / 8] & mask != 0;
        let p1 = self.cursor_ram[plane + bit / 8] & mask != 0;
        match (self.command[2] & 3, p1, p0) {
            // Three colours.
            (1, false, false) => CursorPixel::Transparent,
            (1, false, true) => CursorPixel::Colour(1),
            (1, true, false) => CursorPixel::Colour(2),
            (1, true, true) => CursorPixel::Colour(3),
            // Two colours, as an XGA's: the second plane transparent.
            (2, false, false) => CursorPixel::Colour(1),
            (2, false, true) => CursorPixel::Colour(2),
            (2, true, false) => CursorPixel::Transparent,
            (2, true, true) => CursorPixel::Invert,
            // As X windows: the second plane shows.
            (_, false, _) => CursorPixel::Transparent,
            (_, true, false) => CursorPixel::Colour(2),
            (_, true, true) => CursorPixel::Colour(3),
        }
    }

    /// A cursor colour register's colour, in 8 bits a component.
    pub fn cursor_colour(&self, index: usize) -> (u8, u8, u8) {
        let c = &self.cursor_colours[index * 3..index * 3 + 3];
        if self.dac_8bit() {
            (c[0], c[1], c[2])
        } else {
            let six = |v: u8| (v << 2) | (v >> 4);
            (six(c[0]), six(c[1]), six(c[2]))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // vramdac.c's verite_loadcursor and verite_movecursor for a 64x64
    // cursor with its hot spot at (0, 0) at (100, 50) of the screen.
    #[test]
    fn a_cursor_loaded_and_moved_as_xfree86_does() {
        let (mut dac, mut palette) = (Bt485::default(), vec![0u8; 768]);
        dac.write(0x6, 0x80, &mut palette);
        dac.write(0x0, 0x01, &mut palette);
        dac.write(0xA, 0x04, &mut palette);
        dac.write(0x0, 0x00, &mut palette);
        // The first plane all 1s, the second all 0s.
        for _ in 0..512 {
            dac.write(0xB, 0xFF, &mut palette);
        }
        for _ in 0..512 {
            dac.write(0xB, 0x00, &mut palette);
        }
        dac.write(0x9, 0x01, &mut palette);
        for (offset, value) in [(0xC, 164), (0xD, 0), (0xE, 114), (0xF, 0)] {
            dac.write(offset, value, &mut palette);
        }
        assert_eq!(dac.cursor_size(), Some(64));
        assert_eq!(dac.cursor_origin(), Some((100, 50)));
        assert_eq!(dac.cursor_pixel(63, 63), CursorPixel::Colour(1));
    }

    #[test]
    fn palette_entries_go_red_green_blue_and_on() {
        let (mut dac, mut palette) = (Bt485::default(), vec![0u8; 768]);
        dac.write(0x0, 5, &mut palette);
        for v in [1, 2, 3, 4] {
            dac.write(0x1, v, &mut palette);
        }
        assert_eq!(&palette[15..19], &[1, 2, 3, 4]);
    }
}
