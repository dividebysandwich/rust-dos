//! The card's commands: what the words of one mean (chapter 9 of
//! `docs/toadforce-spec.md`).
//!
//! `decode` turns the words of one command into a `Command`, checking
//! everything that can be checked without the card's state: the opcode
//! against the level, the length against the opcode, and each argument
//! against card memory's size. What it returns is safe to carry out, but
//! for a `Flip`, whose surface depends on the mode shown. Decoding is
//! kept apart from carrying out so that the ring and `ToadForce::submit`
//! run one decoder, and so that a renderer on another thread can later
//! be handed commands already checked.

use super::caps;
use super::format::{Rect, SURFACE_WORDS, Surface};
use super::level1;
use super::regs::Error;

pub const NOP: u16 = 0x0000;
pub const FILL: u16 = 0x0001;
pub const BLT: u16 = 0x0002;
pub const FLIP: u16 = 0x0003;

/// `BLT`'s flags: bit 0, leave the destination where the source pixel
/// equals the colour key.
pub const BLT_COLOUR_KEY: u32 = 1 << 0;
/// `FLIP`'s flags: bit 0, at the next vertical blank.
pub const FLIP_WAIT: u32 = 1 << 0;

/// A command's first word: the opcode in bits 0 to 15 and the length in
/// words, this one included, in bits 16 to 31.
pub const fn header(opcode: u16, words: usize) -> u32 {
    opcode as u32 | (words as u32) << 16
}

/// The length the header word `header` gives its command, in words.
pub fn length(header: u32) -> usize {
    (header >> 16) as usize
}

/// The lengths an opcode's commands can have.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Length {
    Exactly(usize),
    AtLeast(usize),
    /// The first, and after it any whole number of the second: the
    /// fixed arguments and a list of items.
    List(usize, usize),
}

impl Length {
    fn allows(self, words: usize) -> bool {
        match self {
            Length::Exactly(n) => words == n,
            Length::AtLeast(n) => words >= n,
            Length::List(least, item) => words >= least && (words - least).is_multiple_of(item),
        }
    }
}

/// The lengths `opcode`'s commands can have at `level`, if the level has
/// the command. The levels' command lists are frozen with their
/// capability tables (`caps`), and the tests below hold a copy.
pub fn lengths(level: u32, opcode: u16) -> Option<Length> {
    Some(match opcode {
        NOP => Length::AtLeast(1),
        FILL => Length::Exactly(11),
        BLT => Length::Exactly(21),
        FLIP => Length::Exactly(3),
        _ if level >= 1 => match level1::lengths(opcode)? {
            (least, 0) => Length::Exactly(least),
            (least, item) => Length::List(least, item),
        },
        _ => return None,
    })
}

/// A command, decoded. One of level 1 borrows its lists from the words
/// it was decoded from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command<'a> {
    Nop,
    /// Write `value` to each pixel of `rect` of `dst`.
    Fill {
        dst: Surface,
        rect: Rect,
        value: u32,
    },
    /// Copy `src_rect` of `src` to `dst_rect` of `dst`, stretched to it
    /// where their sizes differ, leaving the pixels whose source equals
    /// `key`.
    Blt {
        dst: Surface,
        dst_rect: Rect,
        src: Surface,
        src_rect: Rect,
        key: Option<u32>,
    },
    /// Show the surface at `offset`, at the next vertical blank with
    /// `wait`.
    Flip {
        offset: u32,
        wait: bool,
    },
    Level1(level1::Command<'a>),
}

/// The command `words` hold, header first, at `level` on a card with
/// `memory` bytes. `words` is as long as the header says.
pub fn decode(words: &[u32], level: u32, memory: u32) -> Result<Command<'_>, Error> {
    let opcode = words[0] as u16;
    let lengths = lengths(level, opcode).ok_or(Error::BadOpcode)?;
    if !lengths.allows(words.len()) {
        return Err(Error::BadLength);
    }
    let rect_in = |surface: &Surface, words: &[u32]| {
        let rect = Rect::parse(words);
        surface.holds(&rect).then_some(rect).ok_or(Error::BadRange)
    };
    Ok(match opcode {
        NOP => Command::Nop,
        FILL => {
            let dst = Surface::parse(&words[1..], caps::BLIT_FORMATS, memory)?;
            let rect = rect_in(&dst, &words[6..10])?;
            Command::Fill {
                dst,
                rect,
                value: words[10],
            }
        }
        BLT => {
            // Every value before any range, so that what a command with
            // several faults is rejected for doesn't depend on which
            // surface has them.
            let dst = Surface::values(&words[1..], caps::BLIT_FORMATS)?;
            let src = Surface::values(&words[10..], caps::BLIT_FORMATS)?;
            let flags = words[19];
            if dst.format.bytes() != src.format.bytes() || flags & !BLT_COLOUR_KEY != 0 {
                return Err(Error::BadValue);
            }
            dst.in_memory(memory)?;
            src.in_memory(memory)?;
            let dst_rect = rect_in(&dst, &words[1 + SURFACE_WORDS..])?;
            let src_rect = rect_in(&src, &words[10 + SURFACE_WORDS..])?;
            Command::Blt {
                dst,
                dst_rect,
                src,
                src_rect,
                key: (flags & BLT_COLOUR_KEY != 0).then_some(words[20]),
            }
        }
        FLIP => {
            let flags = words[2];
            if flags & !FLIP_WAIT != 0 {
                return Err(Error::BadValue);
            }
            Command::Flip {
                offset: words[1],
                wait: flags & FLIP_WAIT != 0,
            }
        }
        _ => Command::Level1(level1::decode(opcode, words, memory)?),
    })
}

/// The words of commands, for the emulator's own drivers and the tests:
/// what a guest's driver writes into the ring.
pub mod encode {
    use super::{BLT, BLT_COLOUR_KEY, FILL, FLIP, FLIP_WAIT, NOP, header};
    use crate::toadforce::format::{Rect, Surface};

    fn surface(out: &mut Vec<u32>, s: &Surface) {
        out.extend([s.offset, s.pitch, s.width, s.height, s.format.code()]);
    }

    fn rect(out: &mut Vec<u32>, r: &Rect) {
        out.extend([r.left, r.top, r.width, r.height]);
    }

    /// A `NOP` of `words` words, 1 at least.
    pub fn nop(out: &mut Vec<u32>, words: usize) {
        out.push(header(NOP, words));
        out.resize(out.len() + words - 1, 0);
    }

    pub fn fill(out: &mut Vec<u32>, dst: &Surface, r: &Rect, value: u32) {
        out.push(header(FILL, 11));
        surface(out, dst);
        rect(out, r);
        out.push(value);
    }

    pub fn blt(
        out: &mut Vec<u32>,
        dst: &Surface,
        dst_rect: &Rect,
        src: &Surface,
        src_rect: &Rect,
        key: Option<u32>,
    ) {
        out.push(header(BLT, 21));
        surface(out, dst);
        rect(out, dst_rect);
        surface(out, src);
        rect(out, src_rect);
        out.extend([key.map_or(0, |_| BLT_COLOUR_KEY), key.unwrap_or(0)]);
    }

    pub fn flip(out: &mut Vec<u32>, offset: u32, wait: bool) {
        out.extend([header(FLIP, 3), offset, if wait { FLIP_WAIT } else { 0 }]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::toadforce::format::Format;

    const MEMORY: u32 = 32 << 20;

    fn surface(offset: u32, format: Format) -> Surface {
        Surface {
            offset,
            pitch: 64 * format.bytes(),
            width: 64,
            height: 32,
            format,
        }
    }

    fn rect(left: u32, top: u32, width: u32, height: u32) -> Rect {
        Rect {
            left,
            top,
            width,
            height,
        }
    }

    /// The released levels' commands, copied by hand: opcode and length.
    /// A test that fails here means a released level changed.
    const FROZEN: [&[(u16, Length)]; 1] = [&[
        (0x0000, Length::AtLeast(1)),
        (0x0001, Length::Exactly(11)),
        (0x0002, Length::Exactly(21)),
        (0x0003, Length::Exactly(3)),
    ]];

    #[test]
    fn released_levels_keep_their_commands() {
        assert_eq!(FROZEN.len(), caps::RELEASED_LEVEL as usize + 1);
        for (level, frozen) in FROZEN.iter().enumerate() {
            for opcode in 0..=u16::MAX {
                let expected = frozen.iter().find(|c| c.0 == opcode).map(|c| c.1);
                assert_eq!(
                    lengths(level as u32, opcode),
                    expected,
                    "level {level}, opcode {opcode:04X}"
                );
            }
        }
    }

    #[test]
    fn what_is_encoded_decodes() {
        let (dst, src) = (
            surface(0, Format::X8R8G8B8),
            surface(0x10000, Format::A8R8G8B8),
        );
        let mut words = Vec::new();
        encode::fill(&mut words, &dst, &rect(1, 2, 3, 4), 0x1234_5678);
        assert_eq!(
            decode(&words, 0, MEMORY),
            Ok(Command::Fill {
                dst,
                rect: rect(1, 2, 3, 4),
                value: 0x1234_5678
            })
        );
        words.clear();
        encode::blt(
            &mut words,
            &dst,
            &rect(0, 0, 8, 8),
            &src,
            &rect(4, 4, 2, 2),
            Some(7),
        );
        assert_eq!(
            decode(&words, 0, MEMORY),
            Ok(Command::Blt {
                dst,
                dst_rect: rect(0, 0, 8, 8),
                src,
                src_rect: rect(4, 4, 2, 2),
                key: Some(7)
            })
        );
        words.clear();
        encode::flip(&mut words, 0x4000, true);
        assert_eq!(
            decode(&words, 0, MEMORY),
            Ok(Command::Flip {
                offset: 0x4000,
                wait: true
            })
        );
        words.clear();
        encode::nop(&mut words, 5);
        assert_eq!(words.len(), 5);
        assert_eq!(decode(&words, 0, MEMORY), Ok(Command::Nop));
    }

    #[test]
    fn faults_have_their_codes() {
        let dst = surface(0, Format::R5G6B5);
        assert_eq!(
            decode(&[header(0x0004, 1)], 0, MEMORY),
            Err(Error::BadOpcode)
        );
        assert_eq!(
            decode(&[header(FLIP, 2), 0], 0, MEMORY),
            Err(Error::BadLength)
        );
        assert_eq!(
            decode(&[header(FLIP, 3), 0, 2], 0, MEMORY),
            Err(Error::BadValue)
        );
        // A rectangle outside its surface, and one with no pixel.
        for r in [rect(60, 0, 5, 1), rect(0, 0, 0, 1), rect(0, 32, 1, 1)] {
            let mut words = Vec::new();
            encode::fill(&mut words, &dst, &r, 0);
            assert_eq!(decode(&words, 0, MEMORY), Err(Error::BadRange), "{r:?}");
        }
        // Pixels of different sizes, and a flag the level doesn't have.
        let mut words = Vec::new();
        let all = rect(0, 0, 64, 32);
        encode::blt(&mut words, &dst, &all, &surface(0, Format::P8), &all, None);
        assert_eq!(decode(&words, 0, MEMORY), Err(Error::BadValue));
        words.clear();
        encode::blt(&mut words, &dst, &all, &dst, &all, None);
        words[19] = 2;
        assert_eq!(decode(&words, 0, MEMORY), Err(Error::BadValue));
        // A surface past the end of card memory.
        words.clear();
        let past = surface(MEMORY - 64, Format::R5G6B5);
        encode::fill(&mut words, &past, &all, 0);
        assert_eq!(decode(&words, 0, MEMORY), Err(Error::BadRange));
        // A value comes before a range, whichever surface has which.
        for (to, from) in [(&past, &dst), (&dst, &past)] {
            words.clear();
            encode::blt(&mut words, to, &all, from, &all, None);
            assert_eq!(decode(&words, 0, MEMORY), Err(Error::BadRange));
            words[19] = 2;
            assert_eq!(decode(&words, 0, MEMORY), Err(Error::BadValue));
        }
        words.clear();
        encode::blt(&mut words, &past, &all, &dst, &all, None);
        words[14] = 9;
        assert_eq!(decode(&words, 0, MEMORY), Err(Error::BadValue));
    }
}
