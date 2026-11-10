//! Pixel formats and surfaces: rectangles of pixels in card memory, as
//! commands and the mode registers describe them (chapter 4 of
//! `docs/toadforce-spec.md`).

use super::caps;
use super::regs::Error;

/// A pixel format, by its code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
}

impl Format {
    /// The format code `code` names, if it names one.
    pub fn from_code(code: u32) -> Option<Self> {
        Some(match code {
            1 => Format::P8,
            2 => Format::R5G6B5,
            3 => Format::X8R8G8B8,
            4 => Format::A8R8G8B8,
            _ => return None,
        })
    }

    pub fn code(self) -> u32 {
        self as u32
    }

    /// Whether the set of formats `mask` (bit n for code n, as the
    /// capability table gives one) has this format.
    pub fn in_mask(self, mask: u32) -> bool {
        mask >> self.code() & 1 != 0
    }

    /// The bytes of a pixel.
    pub fn bytes(self) -> u32 {
        match self {
            Format::P8 => 1,
            Format::R5G6B5 => 2,
            Format::X8R8G8B8 | Format::A8R8G8B8 => 4,
        }
    }

    /// The bits of a pixel the format defines, which a colour key
    /// compares.
    pub fn defined_bits(self) -> u32 {
        match self {
            Format::P8 => 0xFF,
            Format::R5G6B5 => 0xFFFF,
            Format::X8R8G8B8 => 0x00FF_FFFF,
            Format::A8R8G8B8 => 0xFFFF_FFFF,
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
