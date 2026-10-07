//! The commands Rendition's Speedy3D microcode takes from the FIFO: how
//! long each is, so the stream can be cut into them.
//!
//! A command's first word has its opcode in the low 16 bits and, for the
//! drawing commands, the vertex type in the high 16. The opcodes and the
//! vertex types are RRedline's (`V_FIFO_*`); the commands that set the
//! drawing state have opcodes of their own, whose arguments DOSBox's
//! Rendition fork (dosbox-staging-rendition, GPL) counts in its
//! `ValidateFifo`.

/// RRedline's commands.
pub mod op {
    pub const VERSION: u16 = 0;
    pub const CONTEXT_INIT: u16 = 2;
    pub const DISPLAY: u16 = 4;
    pub const STEREO_DISPLAY: u16 = 5;
    pub const WAIT_DISPLAY_SWITCH: u16 = 6;
    pub const SYNC_AND_RESPOND: u16 = 8;
    pub const MEM_WRITE: u16 = 9;
    pub const MEM_WRITE_RECT: u16 = 10;
    pub const MEM_CLEAR_RECT: u16 = 13;
    pub const MEM_WRITE_SPRITE: u16 = 14;
    pub const DOT: u16 = 15;
    pub const LINE: u16 = 16;
    pub const POLYLINE: u16 = 17;
    pub const INTLINE: u16 = 18;
    pub const INTPOLYLINE: u16 = 19;
    pub const AALINE: u16 = 20;
    pub const SPAN: u16 = 21;
    pub const SQUARE: u16 = 22;
    pub const RECTANGLE: u16 = 23;
    pub const TRIANGLE: u16 = 24;
    pub const AAEDGE: u16 = 25;
    pub const TRISTRIP: u16 = 26;
    pub const TRIFAN: u16 = 27;
    pub const AFFINE: u16 = 28;
    pub const WARP: u16 = 29;
    pub const BITBLT: u16 = 30;
    pub const BITFILL: u16 = 31;
    pub const RLEHACK: u16 = 32;
    pub const PALETTE: u16 = 33;
    pub const TIMER: u16 = 34;
    pub const FRAME_MARKER: u16 = 35;
    pub const COMPOSITE_RECT: u16 = 36;
    pub const PARTICLES: u16 = 37;
    pub const TRI_FILL: u16 = 38;
    pub const QSPAN: u16 = 39;
    pub const QLIGHT: u16 = 40;
    pub const BITBLT_MEM: u16 = 41;
    pub const LOOKUP: u16 = 42;
    pub const QAAZEDGE: u16 = 43;
    pub const FTRI_KXYZ: u16 = 48;
    pub const FTRI_KXYZUVQ: u16 = 49;
    pub const FTRI_KFXYZUVQ: u16 = 50;
    pub const FTRI_KSXYZ: u16 = 51;
    pub const FTRI_KSXYZUVQ: u16 = 52;
    pub const D3DTRI: u16 = 54;
    pub const D3DTRI_ODD: u16 = 55;
    pub const D3DTRI_EVEN: u16 = 56;
}

/// A vertex type's attributes, in their order in the vertex, a dword
/// each: K packed colour, I intensity, R G B, A alpha, S specular, F fog,
/// X Y Z, U V, Q. RRedline's `V_FIFO_*` vertex types by number.
pub fn vertex_fields(vtype: u16) -> Option<&'static str> {
    Some(match vtype {
        1 => "xy",
        2 => "xyuv",
        3 => "xyuvq",
        4 => "ixyuvq",
        5 => "fxyuvq",
        6 => "xyzuvq",
        7 => "rgbfxy",
        8 => "rgbxyz",
        9 => "ifxyuvq",
        10 => "ixyzuvq",
        11 => "rgbafxyuvq",
        12 => "rgbaxyzuvq",
        17 => "kfxy",
        18 => "kxyz",
        19 => "kafxyuvq",
        20 => "kaxyzuvq",
        21 => "ixyzuv",
        22 => "rgbafxyzuvq",
        24 => "kafxyzuvq",
        25 => "kxyuvq",
        26 => "ifxyzuvq",
        27 => "axy",
        28 => "kxy",
        29 => "kfxyz",
        30 => "fxyz",
        31 => "xyz",
        _ => return None,
    })
}

/// A vertex's dwords.
pub fn vertex_words(vtype: u16) -> Option<usize> {
    vertex_fields(vtype).map(str::len)
}

/// The arguments of the state commands, by opcode: what Rendition's
/// library (VLIB, linked whole into Tomb Raider's Vérité executable) sends
/// for each, beside the fork's counts, some of which are wrong.
fn state_arguments(opcode: u16) -> Option<usize> {
    Some(match opcode {
        0x000C | 0xFFFF => 0,
        0x4000 => 5,
        0x7020 => 8,
        0x1534 | 0x2055 | 0x2056 | 0x2058 | 0x2059 | 0x205B | 0x1008 | 0x18D2 | 0x100D | 0x1050 | 0x1851 | 0x1016
        | 0x0808 | 0x080A | 0x080B | 0x1004 | 0x143B | 0x1006 | 0x5028 | 0x5029 | 0x1231 | 0x15B5 | 0x602A | 0x602B
        | 0x17BE | 0x183F | 0xAA47 | 0x1AC8 | 0x3013 | 0x89C6 | 0x1030 | 0x1CCC | 0x1000 | 0x100E | 0x100F | 0x1038
        | 0x103A | 0x1241 | 0x13B2 | 0x1442 | 0x14B3 | 0x101D | 0x1643 | 0x101A | 0x1017 | 0x1839 | 0x1844 | 0x16B7
        | 0x1BCA | 0x9999 | 0x1C4B | 0x9945 | 0x1636 | 0x1010 | 0x1015 | 0x183C | 0x1040 | 0x1007 | 0x1011 | 0x1014
        | 0x2054 | 0x2057 | 0x205A => 1,
        _ => return None,
    })
}

/// How a command is cut from the stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Length {
    /// This many words in all.
    Words(usize),
    /// A header of this many words, then a count (the header's last word)
    /// of items of this many words each.
    Counted { header: usize, item: usize },
    /// A block of memory: a header of 5 words whose last two are the bytes
    /// a line and the lines, then each line's bytes, padded to words.
    Rect,
    /// Bytes into memory: a header of 3 words whose last is how many, then
    /// the bytes, padded to a word.
    Write,
    /// Bytes for a rectangle: a header of 3 words whose last is the width
    /// and height (16 bits each), then each line's bytes, padded to a word.
    Bytes,
    /// Spans: a header of 5 words, then spans of 6 words until a word
    /// with bit 31 set where a span would start (that word included).
    Spans,
    /// Not known: the stream can't be followed past it.
    Unknown,
}

/// The length of the command `word` starts.
pub fn length(word: u32) -> Length {
    let opcode = word as u16;
    let vtype = (word >> 16) as u16;
    let vertex = vertex_words(vtype);
    let words = |n: usize| Length::Words(1 + n);
    match opcode {
        op::VERSION | op::CONTEXT_INIT | op::SYNC_AND_RESPOND | op::WAIT_DISPLAY_SWITCH | op::RLEHACK => words(0),
        op::TIMER | op::COMPOSITE_RECT | op::BITBLT_MEM | 0x1D | 1 => words(0),
        op::DISPLAY => words(1),
        op::STEREO_DISPLAY | op::BITFILL | op::INTLINE | op::FRAME_MARKER | op::MEM_WRITE_SPRITE => words(2),
        op::MEM_CLEAR_RECT => words(5),
        op::QSPAN => Length::Spans,
        op::PARTICLES => Length::Counted { header: 2, item: 4 },
        op::TRI_FILL => words(6),
        op::AFFINE => words(10),
        op::BITBLT => words(3),
        op::FTRI_KXYZ => words(12),
        op::FTRI_KXYZUVQ => words(21),
        op::FTRI_KFXYZUVQ | op::FTRI_KSXYZUVQ => words(24),
        op::FTRI_KSXYZ => words(15),
        op::MEM_WRITE => Length::Write,
        op::MEM_WRITE_RECT => Length::Rect,
        op::LOOKUP => Length::Bytes,
        op::PALETTE => Length::Counted { header: 2, item: 1 },
        op::DOT => vertex.map_or(Length::Unknown, words),
        op::TRIANGLE => vertex.map_or(Length::Unknown, |v| words(3 * v)),
        op::LINE | op::AALINE | op::AAEDGE => vertex.map_or(Length::Unknown, |v| words(2 * v)),
        op::RECTANGLE => vertex.map_or(Length::Unknown, |v| words(2 + v)),
        op::SQUARE => vertex.map_or(Length::Unknown, |v| words(1 + v)),
        op::TRISTRIP | op::TRIFAN | op::POLYLINE => {
            vertex.map_or(Length::Unknown, |v| Length::Counted { header: 2, item: v })
        }
        _ => state_arguments(opcode).map_or(Length::Unknown, words),
    }
}

/// The words of a memory block of `lines` lines of `bytes` bytes.
pub fn rect_words(bytes: u32, lines: u32) -> usize {
    5 + bytes.div_ceil(4) as usize * lines as usize
}

/// The words of LOOKUP's `size` (width and height, 16 bits each) bytes.
pub fn bytes_words(size: u32) -> usize {
    3 + (size >> 16).div_ceil(4) as usize * (size & 0xFFFF) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_have_their_lengths() {
        assert_eq!(length(0x0000_000D), Length::Words(6), "a memory clear: base, stride, bytes, lines");
        assert_eq!(length(0x0000_1004), Length::Words(2));
        assert_eq!(length(0x0000_1010), Length::Words(2), "the Z buffer's base");
        assert_eq!(length(0x0000_9945), Length::Words(2));
        assert_eq!(length(0x0000_7020), Length::Words(9), "a palette of 16 16-bit entries");
        // A triangle of XYUVQ vertices: three of five words.
        assert_eq!(length(0x0003_0018), Length::Words(16));
        assert_eq!(length(0x0004_001A), Length::Counted { header: 2, item: 6 });
        assert_eq!(length(0x0000_0099), Length::Unknown);
        assert_eq!(rect_words(8, 8), 5 + 16, "8 lines of 8 bytes");
        assert_eq!(rect_words(6, 2), 5 + 4);
        assert_eq!(bytes_words(0x0040_0040), 3 + 1024, "a 64x64 tile");
        assert_eq!(bytes_words(0x004B_0030), 3 + 19 * 48, "lines of 75 bytes padded to 76");
        assert_eq!(length(0x0000_0009), Length::Write);
    }
}
