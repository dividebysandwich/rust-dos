//! PKWARE's Data Compression Library ("implode"), which compresses the
//! members of InstallShield 3 archives, decoded as Ben Rudiak-Gould's
//! description of the format (comp.compression, 13 August 2001) and zlib's
//! `contrib/blast`, which reads it, have it:
//!
//! - Byte 0 is 0 for literals stored as plain bytes and 1 for literals
//!   coded with a Huffman code. Byte 1 is 4, 5 or 6: how many low bits of
//!   a distance are sent as plain bits, so the window is 1, 2 or 4 KB.
//! - Bits follow, least significant first. Each item starts with a bit: 0
//!   for a literal, 1 for a length and a distance. A length of 519 ends
//!   the data.
//! - The three Huffman codes are fixed. A code is sent most significant
//!   bit first with each bit inverted, so reading a bit and inverting it
//!   builds the canonical code.

use crate::member::Input;
use crate::{Error, Result, invalid};

const MAX_CODE_LENGTH: usize = 13;
const WINDOW: usize = 4096;
const END_LENGTH: usize = 519;

/// The code lengths, each byte a run: the high four bits are the run's
/// length less one, the low four the code length. They describe 256
/// literal, 16 length and 64 distance symbols.
const LITERAL_RUNS: [u8; 98] = [
    11, 124, 8, 7, 28, 7, 188, 13, 76, 4, 10, 8, 12, 10, 12, 10, 8, 23, 8, 9, 7, 6, 7, 8, 7, 6, 55,
    8, 23, 24, 12, 11, 7, 9, 11, 12, 6, 7, 22, 5, 7, 24, 6, 11, 9, 6, 7, 22, 7, 11, 38, 7, 9, 8,
    25, 11, 8, 11, 9, 12, 8, 12, 5, 38, 5, 38, 5, 11, 7, 5, 6, 21, 6, 10, 53, 8, 7, 24, 10, 27, 44,
    253, 253, 253, 252, 252, 252, 13, 12, 45, 12, 45, 12, 61, 12, 45, 44, 173,
];
const LENGTH_RUNS: [u8; 6] = [2, 35, 36, 53, 38, 23];
const DISTANCE_RUNS: [u8; 7] = [2, 20, 53, 230, 247, 151, 248];
const LENGTH_BASE: [usize; 16] = [3, 2, 4, 5, 6, 7, 8, 9, 10, 12, 16, 24, 40, 72, 136, 264];
const LENGTH_EXTRA: [u32; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8];

/// A canonical Huffman code: how many codes each length has, and the
/// symbols in canonical order (by length, then by symbol).
pub(crate) struct Huffman {
    pub(crate) counts: [u16; MAX_CODE_LENGTH + 1],
    symbols: Vec<u16>,
}

impl Huffman {
    pub(crate) fn new(runs: &[u8]) -> Huffman {
        let mut lengths = Vec::new();
        for &run in runs {
            for _ in 0..(run >> 4) + 1 {
                lengths.push(run & 15);
            }
        }
        let mut counts = [0u16; MAX_CODE_LENGTH + 1];
        for &length in &lengths {
            counts[length as usize] += 1;
        }
        let mut symbols: Vec<u16> = (0..lengths.len() as u16)
            .filter(|&s| lengths[s as usize] != 0)
            .collect();
        symbols.sort_by_key(|&s| lengths[s as usize]);
        Huffman { counts, symbols }
    }

    /// The lengths of each symbol, as the runs give them.
    #[cfg(any(test, feature = "test-util"))]
    pub(crate) fn lengths(runs: &[u8]) -> Vec<u8> {
        runs.iter()
            .flat_map(|&run| std::iter::repeat_n(run & 15, (run >> 4) as usize + 1))
            .collect()
    }

    /// Whether the lengths make a complete code.
    #[cfg(test)]
    pub(crate) fn complete(&self) -> bool {
        let mut left: i32 = 1;
        for &count in &self.counts[1..] {
            left = (left << 1) - count as i32;
        }
        left == 0
    }

    fn decode(&self, bits: &mut Bits, input: &mut Input) -> Result<usize> {
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for &count in &self.counts[1..] {
            code |= (bits.take(input, 1)? ^ 1) as i32;
            let count = count as i32;
            if code - first < count {
                return Ok(self.symbols[(index + code - first) as usize] as usize);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        invalid("the data holds a code that isn't in its code table")
    }
}

pub(crate) struct Codes {
    pub(crate) literal: Huffman,
    pub(crate) length: Huffman,
    pub(crate) distance: Huffman,
}

pub(crate) fn codes() -> &'static Codes {
    static CODES: std::sync::OnceLock<Codes> = std::sync::OnceLock::new();
    CODES.get_or_init(|| Codes {
        literal: Huffman::new(&LITERAL_RUNS),
        length: Huffman::new(&LENGTH_RUNS),
        distance: Huffman::new(&DISTANCE_RUNS),
    })
}

/// The runs of the three codes, for the tests' writer.
#[cfg(any(test, feature = "test-util"))]
pub(crate) fn runs() -> [&'static [u8]; 3] {
    [&LITERAL_RUNS, &LENGTH_RUNS, &DISTANCE_RUNS]
}

#[cfg(any(test, feature = "test-util"))]
pub(crate) const LENGTHS: ([usize; 16], [u32; 16]) = (LENGTH_BASE, LENGTH_EXTRA);

/// The bits not yet taken from the input.
#[derive(Default)]
struct Bits {
    buffer: u32,
    count: u32,
}

impl Bits {
    fn take(&mut self, input: &mut Input, count: u32) -> Result<u32> {
        while self.count < count {
            let Some(byte) = input.next_byte()? else {
                return invalid("the data ends before its end code");
            };
            self.buffer |= (byte as u32) << self.count;
            self.count += 8;
        }
        let value = self.buffer & ((1u32 << count) - 1);
        self.buffer >>= count;
        self.count -= count;
        Ok(value)
    }
}

/// A PKWARE DCL decoder, which keeps its place between reads.
pub(crate) struct Exploder {
    window: Box<[u8; WINDOW]>,
    coded_literals: bool,
    distance_bits: u32,
    started: bool,
    ended: bool,
    bits: Bits,
    produced: u64,
    copy_left: usize,
    copy_distance: usize,
}

impl Exploder {
    pub(crate) fn new() -> Exploder {
        Exploder {
            window: Box::new([0; WINDOW]),
            coded_literals: false,
            distance_bits: 0,
            started: false,
            ended: false,
            bits: Bits::default(),
            produced: 0,
            copy_left: 0,
            copy_distance: 0,
        }
    }

    /// The bytes expanded so far.
    pub(crate) fn produced(&self) -> u64 {
        self.produced
    }

    /// Fills `out` with the next expanded bytes; fewer only once the end
    /// code is read, and none from then on.
    pub(crate) fn read(&mut self, input: &mut Input, out: &mut [u8]) -> Result<usize> {
        if !self.started {
            self.start(input)?;
        }
        let codes = codes();
        let mut count = 0;
        while count < out.len() && !self.ended {
            if self.copy_left > 0 {
                let from = (self.produced as usize).wrapping_sub(self.copy_distance) & (WINDOW - 1);
                out[count] = self.put(self.window[from]);
                count += 1;
                self.copy_left -= 1;
                continue;
            }
            if self.bits.take(input, 1)? == 0 {
                let literal = if self.coded_literals {
                    codes.literal.decode(&mut self.bits, input)? as u8
                } else {
                    self.bits.take(input, 8)? as u8
                };
                out[count] = self.put(literal);
                count += 1;
                continue;
            }
            let symbol = codes.length.decode(&mut self.bits, input)?;
            let length =
                LENGTH_BASE[symbol] + self.bits.take(input, LENGTH_EXTRA[symbol])? as usize;
            if length == END_LENGTH {
                self.ended = true;
                break;
            }
            let low = if length == 2 { 2 } else { self.distance_bits };
            let high = codes.distance.decode(&mut self.bits, input)?;
            let distance = ((high << low) | self.bits.take(input, low)? as usize) + 1;
            if distance as u64 > self.produced {
                return invalid(format!(
                    "the data refers {} bytes back after only {} bytes",
                    distance, self.produced
                ));
            }
            self.copy_left = length;
            self.copy_distance = distance;
        }
        Ok(count)
    }

    fn start(&mut self, input: &mut Input) -> Result<()> {
        self.started = true;
        let literals = self.bits.take(input, 8)?;
        if literals > 1 {
            return invalid(format!(
                "the data starts with literal mode {}; only 0 and 1 exist",
                literals
            ));
        }
        self.coded_literals = literals == 1;
        self.distance_bits = self.bits.take(input, 8)?;
        if !(4..=6).contains(&self.distance_bits) {
            return invalid(format!(
                "the data declares a dictionary of {} bits; only 4, 5 and 6 exist",
                self.distance_bits
            ));
        }
        Ok(())
    }

    fn put(&mut self, value: u8) -> u8 {
        self.window[self.produced as usize & (WINDOW - 1)] = value;
        self.produced += 1;
        value
    }
}

/// Expand PKWARE DCL data whole: what it expands to, at most `max` bytes,
/// up to its end code. Bytes after the end code are left out.
pub fn explode(data: &[u8], max: usize) -> Result<Vec<u8>> {
    let mut cursor = std::io::Cursor::new(data);
    let mut input = Input::new(&mut cursor, data.len() as u64);
    let mut exploder = Exploder::new();
    let mut out = Vec::new();
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        let want = buf.len().min(max - out.len() + 1);
        let n = exploder.read(&mut input, &mut buf[..want])?;
        out.extend_from_slice(&buf[..n]);
        if out.len() > max {
            return Err(Error::Invalid(format!(
                "the data expands past {} bytes",
                max
            )));
        }
        if n < want {
            return Ok(out);
        }
    }
}
