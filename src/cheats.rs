//! Finding a game's values in memory and changing them, as cheat finders
//! do: search memory for a value (the lives left, say), or for every
//! address when the value isn't known; play on and narrow the addresses
//! down by the value it has now, or by how it changed; then set it, or
//! freeze it at a value the machine puts back every frame.

use std::ops::Range;

/// How many bytes a value takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Width {
    Byte,
    Word,
    Dword,
}

impl Width {
    pub const ALL: [Width; 3] = [Width::Byte, Width::Word, Width::Dword];

    pub fn bytes(self) -> usize {
        match self {
            Width::Byte => 1,
            Width::Word => 2,
            Width::Dword => 4,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Width::Byte => "8-bit (0 to 255)",
            Width::Word => "16-bit (0 to 65535)",
            Width::Dword => "32-bit",
        }
    }

    fn max(self) -> u32 {
        match self {
            Width::Byte => 0xFF,
            Width::Word => 0xFFFF,
            Width::Dword => 0xFFFF_FFFF,
        }
    }

    /// The little-endian value at `addr`.
    pub fn read(self, mem: &[u8], addr: usize) -> u32 {
        (0..self.bytes()).fold(0, |value, i| value | (mem.get(addr + i).copied().unwrap_or(0) as u32) << (8 * i))
    }

    /// `value`'s bytes, little-endian.
    pub fn bytes_of(self, value: u32) -> Vec<u8> {
        value.to_le_bytes()[..self.bytes()].to_vec()
    }
}

/// Where to search.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Area {
    /// The 640 KB programs run in, above the BIOS's data.
    Conventional,
    /// Conventional, upper and extended memory, where DOS extenders keep
    /// their programs' data.
    All,
}

impl Area {
    pub const ALL: [Area; 2] = [Area::Conventional, Area::All];

    pub fn name(self) -> &'static str {
        match self {
            Area::Conventional => "conventional memory",
            Area::All => "all memory",
        }
    }

    /// The address ranges it has in `len` bytes of RAM.
    fn ranges(self, len: usize) -> Vec<Range<usize>> {
        let mut ranges = vec![0x500..0xA0000.min(len)];
        if self == Area::All {
            ranges.push(0xC8000.min(len)..0xF0000.min(len));
            ranges.push(0x10_0000.min(len)..len);
        }
        ranges.retain(|r| !r.is_empty());
        ranges
    }
}

/// How the value of an address has to compare to go on being a
/// candidate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Narrow {
    Equal(u32),
    Changed,
    Unchanged,
    Increased,
    Decreased,
}

impl Narrow {
    fn keeps(self, now: u32, before: u32) -> bool {
        match self {
            Narrow::Equal(value) => now == value,
            Narrow::Changed => now != before,
            Narrow::Unchanged => now == before,
            Narrow::Increased => now > before,
            Narrow::Decreased => now < before,
        }
    }
}

/// An address still in the running, with its value now and at the last
/// search.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub addr: usize,
    pub value: u32,
    pub previous: u32,
}

/// A search and the addresses still in it.
#[derive(Clone, Debug)]
pub struct Search {
    width: Width,
    /// A bit for each byte of memory: whether its address is a candidate.
    bits: Vec<u64>,
    count: usize,
    /// Memory as it was at the last search, which the comparisons are
    /// against.
    snapshot: Vec<u8>,
}

impl Search {
    /// A new search of `area` in `mem`: the addresses holding `value`, or
    /// with None every address, to narrow down by how the value changes.
    pub fn new(mem: &[u8], width: Width, area: Area, value: Option<u32>) -> Self {
        let mut search = Self { width, bits: vec![0; mem.len().div_ceil(64)], count: 0, snapshot: mem.to_vec() };
        for range in area.ranges(mem.len()) {
            let last = range.end.saturating_sub(width.bytes() - 1);
            for addr in range.start..last {
                if value.is_none_or(|v| width.read(mem, addr) == v) {
                    search.bits[addr / 64] |= 1 << (addr % 64);
                    search.count += 1;
                }
            }
        }
        search
    }

    pub fn width(&self) -> Width {
        self.width
    }

    pub fn count(&self) -> usize {
        self.count
    }

    /// The candidates, in order.
    fn addrs(&self) -> impl Iterator<Item = usize> + '_ {
        self.bits.iter().enumerate().filter(|(_, word)| **word != 0).flat_map(|(i, &word)| {
            let base = i * 64;
            (0..64).filter(move |bit| word & (1 << bit) != 0).map(move |bit| base + bit)
        })
    }

    /// Keep the candidates whose value in `mem` compares to theirs at the
    /// last search as `how` says; `mem` is what the next one compares to.
    pub fn narrow(&mut self, mem: &[u8], how: Narrow) {
        let width = self.width;
        let mut count = 0;
        for (i, word) in self.bits.iter_mut().enumerate() {
            let mut left = *word;
            let mut kept = 0u64;
            while left != 0 {
                let bit = left.trailing_zeros() as usize;
                left &= left - 1;
                let addr = i * 64 + bit;
                if how.keeps(width.read(mem, addr), width.read(&self.snapshot, addr)) {
                    kept |= 1 << bit;
                }
            }
            *word = kept;
            count += kept.count_ones() as usize;
        }
        self.count = count;
        self.snapshot.clear();
        self.snapshot.extend_from_slice(mem);
    }

    /// No longer a candidate.
    pub fn remove(&mut self, addr: usize) {
        if let Some(word) = self.bits.get_mut(addr / 64)
            && *word & (1 << (addr % 64)) != 0
        {
            *word &= !(1 << (addr % 64));
            self.count -= 1;
        }
    }

    /// `take` of the candidates from the `skip`th on, with their values in
    /// `mem` and at the last search.
    pub fn candidates(&self, mem: &[u8], skip: usize, take: usize) -> Vec<Candidate> {
        self.addrs()
            .skip(skip)
            .take(take)
            .map(|addr| Candidate { addr, value: self.width.read(mem, addr), previous: self.width.read(&self.snapshot, addr) })
            .collect()
    }
}

/// A value the machine puts back at its address before every frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Freeze {
    pub addr: usize,
    pub width: Width,
    pub value: u32,
}

/// A value as typed: decimal, hex with 0x, $ or h, or negative, which is
/// stored as its two's complement.
pub fn parse_value(text: &str, width: Width) -> Result<u32, String> {
    let text = text.trim();
    let bad = || format!("'{}' isn't a value of {}", text, width.name());
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest.trim()),
        None => (false, text),
    };
    let hex = digits
        .strip_prefix("0x")
        .or_else(|| digits.strip_prefix("0X"))
        .or_else(|| digits.strip_prefix('$'))
        .or_else(|| digits.strip_suffix(['h', 'H']));
    let value = match hex {
        Some(hex) => u64::from_str_radix(hex, 16),
        None => digits.parse::<u64>(),
    }
    .map_err(|_| bad())?;
    let max = width.max() as u64;
    if negative {
        let limit = max / 2 + 1;
        if value > limit {
            return Err(bad());
        }
        return Ok(((max + 1 - value) & max) as u32);
    }
    if value > max {
        return Err(bad());
    }
    Ok(value as u32)
}

/// An address as the list shows it: segment:offset in the first
/// megabyte, a physical address above.
pub fn format_address(addr: usize) -> String {
    if addr < 0x10_0000 {
        format!("{:04X}:{:04X}", addr >> 4, addr & 0xF)
    } else {
        format!("{:06X}h", addr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory() -> Vec<u8> {
        vec![0; 0x20_0000]
    }

    #[test]
    fn a_known_value_is_found_and_narrowed_down() {
        let mut mem = memory();
        mem[0x1234] = 5;
        mem[0x5678] = 5;
        mem[0x15_0000] = 5;
        let search = Search::new(&mem, Width::Byte, Area::Conventional, Some(5));
        assert_eq!(search.count(), 2);
        let mut search = Search::new(&mem, Width::Byte, Area::All, Some(5));
        assert_eq!(search.count(), 3);
        // The game took a life.
        mem[0x5678] = 4;
        search.narrow(&mem, Narrow::Equal(4));
        assert_eq!(search.count(), 1);
        assert_eq!(search.candidates(&mem, 0, 10), [Candidate { addr: 0x5678, value: 4, previous: 4 }]);
    }

    #[test]
    fn an_unknown_value_is_narrowed_down_by_how_it_changes() {
        let mut mem = vec![0u8; 0x1000];
        let area = Area::Conventional;
        let mut search = Search::new(&mem, Width::Byte, area, None);
        assert_eq!(search.count(), 0x1000 - 0x500);
        mem[0x800] = 10;
        mem[0x900] = 3;
        search.narrow(&mem, Narrow::Increased);
        assert_eq!(search.count(), 2);
        mem[0x800] = 9;
        search.narrow(&mem, Narrow::Decreased);
        assert_eq!(search.candidates(&mem, 0, 5), [Candidate { addr: 0x800, value: 9, previous: 9 }]);
        search.narrow(&mem, Narrow::Unchanged);
        assert_eq!(search.count(), 1);
        mem[0x800] = 8;
        search.narrow(&mem, Narrow::Changed);
        assert_eq!(search.count(), 1);
        search.remove(0x800);
        assert_eq!(search.count(), 0);
    }

    #[test]
    fn wider_values_at_any_address() {
        let mut mem = vec![0u8; 0x1000];
        mem[0x701..0x703].copy_from_slice(&1000u16.to_le_bytes());
        mem[0x801..0x805].copy_from_slice(&123456u32.to_le_bytes());
        let search = Search::new(&mem, Width::Word, Area::Conventional, Some(1000));
        assert_eq!(search.candidates(&mem, 0, 5)[0].addr, 0x701);
        let search = Search::new(&mem, Width::Dword, Area::Conventional, Some(123456));
        assert_eq!(search.count(), 1);
        // Not past the end of the memory.
        let search = Search::new(&mem, Width::Dword, Area::Conventional, Some(0));
        assert!(search.candidates(&mem, 0, usize::MAX).iter().all(|c| c.addr + 4 <= 0x1000));
    }

    #[test]
    fn values_are_typed_in_decimal_hex_or_negative() {
        assert_eq!(parse_value("42", Width::Byte), Ok(42));
        assert_eq!(parse_value("0x1F", Width::Byte), Ok(0x1F));
        assert_eq!(parse_value("$ff", Width::Byte), Ok(0xFF));
        assert_eq!(parse_value("100h", Width::Word), Ok(0x100));
        assert_eq!(parse_value("-1", Width::Word), Ok(0xFFFF));
        assert_eq!(parse_value("-128", Width::Byte), Ok(0x80));
        assert!(parse_value("256", Width::Byte).is_err());
        assert!(parse_value("-129", Width::Byte).is_err());
        assert!(parse_value("lots", Width::Dword).is_err());
        assert_eq!(parse_value("4294967295", Width::Dword), Ok(u32::MAX));
    }

    #[test]
    fn addresses_show_as_segment_and_offset() {
        assert_eq!(format_address(0x12345), "1234:0005");
        assert_eq!(format_address(0x123456), "123456h");
        assert_eq!(Width::Word.bytes_of(0x1234), [0x34, 0x12]);
    }
}
