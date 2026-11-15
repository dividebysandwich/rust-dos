//! Pixel formats and surfaces: rectangles of pixels in card memory, as
//! commands and the mode registers describe them (chapter 4 of
//! `docs/toadforce-spec.md`).

use super::caps;
use super::regs::Error;
use crate::savestate::{self, Reader, State, StateError, Writer};

/// A pixel format, by its code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum Format {
    /// An index into a palette.
    P8 = 1,
    /// Red in bits 11 to 15, green in 5 to 10, blue in 0 to 4.
    R5G6B5 = 2,
    /// Red in bits 16 to 23, green in 8 to 15, blue in 0 to 7.
    X8R8G8B8 = 3,
    /// The same with alpha in bits 24 to 31.
    A8R8G8B8 = 4,
    // Level 1's, for textures and depth buffers.
    /// Red in bits 10 to 14, green in 5 to 9, blue in 0 to 4.
    X1R5G5B5 = 5,
    /// The same with alpha in bit 15.
    A1R5G5B5 = 6,
    /// Alpha, red, green and blue in four bits each, from bit 12 down.
    A4R4G4B4 = 7,
    /// Alpha alone; the colour is white.
    A8 = 8,
    /// Luminance alone; the alpha is opaque.
    L8 = 9,
    /// Luminance in bits 0 to 7, alpha in 8 to 15.
    A8L8 = 10,
    /// Blocks of 4x4 texels in 8 bytes: two colours and 2 bits a texel,
    /// with one bit of alpha.
    DXT1 = 11,
    /// Blocks in 16 bytes: 4 bits of alpha a texel, then a `DXT1` colour
    /// block. The colours are multiplied by the alpha.
    DXT2 = 12,
    /// As `DXT2`, with colours that aren't.
    DXT3 = 13,
    /// Blocks in 16 bytes: two alphas and 3 bits a texel, then a `DXT1`
    /// colour block. The colours are multiplied by the alpha.
    DXT4 = 14,
    /// As `DXT4`, with colours that aren't.
    DXT5 = 15,
    /// Signed du in bits 0 to 7, dv in 8 to 15.
    V8U8 = 16,
    /// Signed du in bits 0 to 4, dv in 5 to 9, luminance in 10 to 15.
    L6V5U5 = 17,
    /// Signed du in bits 0 to 7, dv in 8 to 15, luminance in 16 to 23.
    X8L8V8U8 = 18,
    /// Depth in 16 bits.
    D16 = 19,
    /// Depth in bits 8 to 31.
    D24X8 = 20,
    /// Depth in bits 8 to 31, stencil in 0 to 7.
    D24S8 = 21,
}

/// Every format, by code.
pub const FORMATS: [Format; 21] = [
    Format::P8,
    Format::R5G6B5,
    Format::X8R8G8B8,
    Format::A8R8G8B8,
    Format::X1R5G5B5,
    Format::A1R5G5B5,
    Format::A4R4G4B4,
    Format::A8,
    Format::L8,
    Format::A8L8,
    Format::DXT1,
    Format::DXT2,
    Format::DXT3,
    Format::DXT4,
    Format::DXT5,
    Format::V8U8,
    Format::L6V5U5,
    Format::X8L8V8U8,
    Format::D16,
    Format::D24X8,
    Format::D24S8,
];

/// A saved state holds a format by its code, so a format added to
/// `FORMATS` is saved without a second list to keep in step.
impl State for Format {
    fn save(&self, w: &mut Writer) {
        (self.code() as u8).save(w);
    }

    fn load(&mut self, r: &mut Reader) -> savestate::Result<()> {
        let mut code = 0u8;
        code.load(r)?;
        *self = Format::from_code(code as u32)
            .ok_or_else(|| StateError::Invalid(format!("a pixel format of code {code}")))?;
        Ok(())
    }
}

impl Format {
    /// The format code `code` names, if it names one.
    pub fn from_code(code: u32) -> Option<Self> {
        FORMATS.into_iter().find(|format| format.code() == code)
    }

    pub fn code(self) -> u32 {
        self as u32
    }

    /// Whether the set of formats `mask` (bit n for code n, as the
    /// capability table gives one) has this format.
    pub fn in_mask(self, mask: u32) -> bool {
        mask >> self.code() & 1 != 0
    }

    /// The bytes of a pixel. A block format has no pixel of its own in
    /// memory and gives 0: see `block_bytes`.
    pub fn bytes(self) -> u32 {
        use Format::*;
        match self {
            P8 | A8 | L8 => 1,
            R5G6B5 | X1R5G5B5 | A1R5G5B5 | A4R4G4B4 | A8L8 | V8U8 | L6V5U5 | D16 => 2,
            X8R8G8B8 | A8R8G8B8 | X8L8V8U8 | D24X8 | D24S8 => 4,
            DXT1 | DXT2 | DXT3 | DXT4 | DXT5 => 0,
        }
    }

    /// The bytes of a block of 4x4 texels, in a format stored as blocks.
    pub fn block_bytes(self) -> Option<u32> {
        match self {
            Format::DXT1 => Some(8),
            Format::DXT2 | Format::DXT3 | Format::DXT4 | Format::DXT5 => Some(16),
            _ => None,
        }
    }

    /// The bytes one line of an image `width` texels wide takes at
    /// least: its pixels, or its row of blocks.
    pub fn line_bytes(self, width: u32) -> u64 {
        match self.block_bytes() {
            Some(block) => width.div_ceil(4) as u64 * block as u64,
            None => width as u64 * self.bytes() as u64,
        }
    }

    /// The lines of memory an image `height` texels high has: its lines
    /// of pixels, or its rows of blocks.
    pub fn lines(self, height: u32) -> u32 {
        match self.block_bytes() {
            Some(_) => height.div_ceil(4),
            None => height,
        }
    }

    /// The bytes from the first of an image to after its last, with
    /// `pitch` bytes from one line of memory to the next.
    pub fn image_bytes(self, width: u32, height: u32, pitch: u32) -> u64 {
        (self.lines(height) as u64 - 1) * pitch as u64 + self.line_bytes(width)
    }

    /// A colour of 8 bits a channel (alpha in bits 24 to 31, red in 16
    /// to 23, green in 8 to 15, blue in 0 to 7) as a pixel of a render
    /// target's format: each channel's top bits.
    pub fn pack(self, argb: u32) -> u32 {
        let (r, g, b) = (argb >> 16 & 0xFF, argb >> 8 & 0xFF, argb & 0xFF);
        match self {
            Format::R5G6B5 => (r >> 3) << 11 | (g >> 2) << 5 | b >> 3,
            Format::X8R8G8B8 => argb & 0x00FF_FFFF,
            _ => argb,
        }
    }

    /// The bits of a pixel the format defines, which a colour key
    /// compares.
    pub fn defined_bits(self) -> u32 {
        use Format::*;
        match self {
            P8 | A8 | L8 => 0xFF,
            X1R5G5B5 => 0x7FFF,
            R5G6B5 | A1R5G5B5 | A4R4G4B4 | A8L8 | V8U8 | L6V5U5 | D16 => 0xFFFF,
            X8R8G8B8 | X8L8V8U8 => 0x00FF_FFFF,
            D24X8 => 0xFFFF_FF00,
            A8R8G8B8 | D24S8 => 0xFFFF_FFFF,
            DXT1 | DXT2 | DXT3 | DXT4 | DXT5 => 0,
        }
    }
}

/// A rectangle of a surface: left, top, width and height in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub left: u32,
    pub top: u32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    /// The rectangle four words of a command describe.
    pub fn parse(words: &[u32]) -> Self {
        Rect {
            left: words[0],
            top: words[1],
            width: words[2],
            height: words[3],
        }
    }
}

/// A surface in card memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Surface {
    /// Where its top left pixel is.
    pub offset: u32,
    /// The bytes from one line to the next.
    pub pitch: u32,
    pub width: u32,
    pub height: u32,
    pub format: Format,
}

impl Default for Surface {
    fn default() -> Self {
        Surface {
            offset: 0,
            pitch: 0,
            width: 0,
            height: 0,
            format: Format::P8,
        }
    }
}

crate::state_fields!(Surface {
    offset,
    pitch,
    width,
    height,
    format,
});

/// The words of a surface's description in a command.
pub const SURFACE_WORDS: usize = 5;

impl Surface {
    /// The surface the five words of a description make in a card memory
    /// of `memory` bytes, when its format is one of `formats`. A size, a
    /// format or a pitch the card doesn't take is `BadValue`, and a
    /// surface that takes them and reaches past card memory `BadRange`.
    pub fn parse(words: &[u32], formats: u32, memory: u32) -> Result<Self, Error> {
        let surface = Self::values(words, formats)?;
        surface.in_memory(memory)?;
        Ok(surface)
    }

    /// The first half of `parse`: the description's values, wherever in
    /// card memory they put the surface. A command with two surfaces
    /// checks the values of both before the range of either.
    pub fn values(words: &[u32], formats: u32) -> Result<Self, Error> {
        let [offset, pitch, width, height, code] = words[..SURFACE_WORDS] else {
            return Err(Error::BadLength);
        };
        let format = Format::from_code(code)
            .filter(|f| f.in_mask(formats))
            .ok_or(Error::BadValue)?;
        let surface = Surface {
            offset,
            pitch,
            width,
            height,
            format,
        };
        let sizes = 1..=caps::SURFACE_MAX;
        if !sizes.contains(&width)
            || !sizes.contains(&height)
            || (pitch as u64) < width as u64 * format.bytes() as u64
        {
            return Err(Error::BadValue);
        }
        Ok(surface)
    }

    /// The second half of `parse`: the surface lies in a card memory of
    /// `memory` bytes.
    pub fn in_memory(&self, memory: u32) -> Result<(), Error> {
        if self.end() > memory as u64 {
            return Err(Error::BadRange);
        }
        Ok(())
    }

    /// The offset after the last byte of the last line.
    pub fn end(&self) -> u64 {
        self.offset as u64
            + (self.height as u64 - 1) * self.pitch as u64
            + self.width as u64 * self.format.bytes() as u64
    }

    /// Whether `rect` has a pixel and lies inside the surface.
    pub fn holds(&self, rect: &Rect) -> bool {
        rect.width >= 1
            && rect.height >= 1
            && rect.left as u64 + rect.width as u64 <= self.width as u64
            && rect.top as u64 + rect.height as u64 <= self.height as u64
    }

    /// Where the pixel at column `x` of line `y` is in card memory.
    pub fn at(&self, x: u32, y: u32) -> usize {
        self.offset as usize
            + y as usize * self.pitch as usize
            + x as usize * self.format.bytes() as usize
    }

    /// The bytes of card memory `rect` of the surface spans: from its
    /// first pixel to after its last.
    pub fn span(&self, rect: &Rect) -> (u32, u32) {
        let first = self.at(rect.left, rect.top);
        let last = self.at(rect.left + rect.width - 1, rect.top + rect.height - 1);
        (first as u32, (last + self.format.bytes() as usize) as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MEMORY: u32 = 32 << 20;
    const ANY: u32 = 0x1E;

    #[test]
    fn a_surface_ends_with_the_last_pixel_of_its_last_line() {
        // The pitch counts between lines only: the last line needs its
        // pixels and no more.
        let s = Surface::parse(&[MEMORY - 3 * 100 - 16, 100, 4, 4, 3], ANY, MEMORY).unwrap();
        assert_eq!(s.end(), MEMORY as u64);
        assert_eq!(
            Surface::parse(&[MEMORY - 3 * 100 - 15, 100, 4, 4, 3], ANY, MEMORY),
            Err(Error::BadRange)
        );
    }

    #[test]
    fn values_are_checked_before_the_range() {
        let parse = |words: [u32; 5]| Surface::parse(&words, ANY, MEMORY);
        assert_eq!(parse([0, 640, 640, 480, 0]), Err(Error::BadValue));
        assert_eq!(parse([0, 640, 640, 480, 5]), Err(Error::BadValue));
        assert_eq!(parse([0, 640, 0, 480, 1]), Err(Error::BadValue));
        assert_eq!(parse([0, 640, 640, 4097, 1]), Err(Error::BadValue));
        // A pitch less than a line of pixels.
        assert_eq!(parse([0, 1279, 640, 480, 2]), Err(Error::BadValue));
        assert_eq!(parse([u32::MAX, 0, 640, 480, 0]), Err(Error::BadValue));
        // Sizes that would overflow 32 bits are past the end, no more.
        assert_eq!(
            parse([u32::MAX, u32::MAX, 4096, 4096, 4]),
            Err(Error::BadRange)
        );
        // A format the command doesn't take.
        assert_eq!(
            Surface::parse(&[0, 640, 640, 480, 4], 0x0E, MEMORY),
            Err(Error::BadValue)
        );
    }

    #[test]
    fn a_rectangle_lies_inside_its_surface() {
        let s = Surface::parse(&[0, 640, 640, 480, 1], ANY, MEMORY).unwrap();
        let rect = |left, top, width, height| Rect {
            left,
            top,
            width,
            height,
        };
        assert!(s.holds(&rect(0, 0, 640, 480)));
        assert!(s.holds(&rect(639, 479, 1, 1)));
        assert!(!s.holds(&rect(0, 0, 0, 1)));
        assert!(!s.holds(&rect(1, 0, 640, 1)));
        assert!(!s.holds(&rect(u32::MAX, 0, 2, 1)));
        assert_eq!(s.span(&rect(2, 1, 3, 2)), (642, 1285));
    }
}
