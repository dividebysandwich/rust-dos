//! Rewind: the machine's states of the last minutes, to go back through
//! (held Alt+F11). A state is taken every half second of emulated time;
//! the newest is kept whole, and each one before it as what it takes to
//! go back to it from the one after: a delta.
//!
//! A delta goes section by section (see machine.rs) and block by block:
//! a block that didn't change is marked so, and one that did is kept XOR
//! the same bytes of the newer state, mostly zeros, deflated. A section
//! whose length changed (a queue grew) is compared both from its start and
//! from its end, so the blocks after the change still line up.
//!
//! The deltas take as much memory as `rewind_memory` allows; the oldest go
//! first.

use flate2::Compression;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use std::collections::VecDeque;
use std::io::{Read, Write};

/// The bytes compared at a time.
const BLOCK: usize = 4096;
/// How a block of the older state is kept: XOR the newer state's bytes
/// at the same place from the section's start or from its end, or as
/// those bytes, the same (a block that didn't change takes no more).
const FROM_START: u8 = 0;
const FROM_END: u8 = 1;
const SAME_FROM_START: u8 = 2;
const SAME_FROM_END: u8 = 3;

/// The sections of a state: where each starts and ends, its header
/// included.
fn sections(state: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 10 <= state.len() {
        let len = u32::from_le_bytes(state[at + 6..at + 10].try_into().unwrap()) as usize;
        let end = (at + 10).saturating_add(len).min(state.len());
        out.push((at, end));
        at = end;
    }
    if at < state.len() {
        out.push((at, state.len()));
    }
    out
}

/// The part of `other` that lies under `len` bytes placed at `at`: the
/// range of those bytes and the range of `other`'s.
fn overlap(len: usize, other: &[u8], at: isize) -> (std::ops::Range<usize>, std::ops::Range<usize>) {
    let lo = at.max(0);
    let hi = (at + len as isize).min(other.len() as isize);
    if lo >= hi {
        return (0..0, 0..0);
    }
    let (lo, hi) = (lo as usize, hi as usize);
    let first = (lo as isize - at) as usize;
    (first..first + (hi - lo), lo..hi)
}

/// XOR `block` into `out` with the bytes of `other` from `at` on (zeros
/// where `other` has none).
fn xor_into(out: &mut Vec<u8>, block: &[u8], other: &[u8], at: isize) {
    let start = out.len();
    out.extend_from_slice(block);
    let (mine, theirs) = overlap(block.len(), other, at);
    for (b, o) in out[start + mine.start..start + mine.end].iter_mut().zip(&other[theirs]) {
        *b ^= o;
    }
}

/// How many bytes of `block` differ from `other` from `at` on.
fn differences(block: &[u8], other: &[u8], at: isize) -> usize {
    let (mine, theirs) = overlap(block.len(), other, at);
    let outside = block[..mine.start].iter().chain(&block[mine.end..]).filter(|&&b| b != 0).count();
    outside + block[mine].iter().zip(&other[theirs]).filter(|(b, o)| b != o).count()
}

/// What `undo` needs to make `old` from `new`.
pub fn delta(old: &[u8], new: &[u8]) -> Vec<u8> {
    let (old_sections, new_sections) = (sections(old), sections(new));
    let mut out = Vec::with_capacity(old.len());
    out.extend_from_slice(&(old_sections.len() as u32).to_le_bytes());
    for (i, &(start, end)) in old_sections.iter().enumerate() {
        let o = &old[start..end];
        // The newer state's section of the same place and tag, if it has
        // one.
        let n = new_sections
            .get(i)
            .map(|&(s, e)| &new[s..e])
            .filter(|n| n.len() >= 4 && o.len() >= 4 && n[..4] == o[..4]);
        out.extend_from_slice(&(o.len() as u32).to_le_bytes());
        out.push(n.is_some() as u8);
        let n = n.unwrap_or(&[]);
        let shift = n.len() as isize - o.len() as isize;
        for at in (0..o.len()).step_by(BLOCK) {
            let block = &o[at..(at + BLOCK).min(o.len())];
            let from_start = at as isize;
            let from_end = at as isize + shift;
            let same = |from: isize| from >= 0 && n.get(from as usize..from as usize + block.len()) == Some(block);
            if same(from_start) {
                out.push(SAME_FROM_START);
                continue;
            }
            if shift != 0 && same(from_end) {
                out.push(SAME_FROM_END);
                continue;
            }
            let end_is_better = shift != 0 && differences(block, n, from_end) < differences(block, n, from_start);
            if end_is_better {
                out.push(FROM_END);
                xor_into(&mut out, block, n, from_end);
            } else {
                out.push(FROM_START);
                xor_into(&mut out, block, n, from_start);
            }
        }
    }
    let mut deflate = DeflateEncoder::new(Vec::new(), Compression::fast());
    deflate.write_all(&out).expect("writing to memory");
    deflate.finish().expect("writing to memory")
}

/// The older state `delta` was made for, from the newer `new`.
pub fn undo(new: &[u8], delta: &[u8]) -> Result<Vec<u8>, String> {
    let mut raw = Vec::with_capacity(new.len());
    DeflateDecoder::new(delta).read_to_end(&mut raw).map_err(|e| format!("a damaged rewind step: {}", e))?;
    let bad = || "a damaged rewind step".to_string();
    let mut at = 0;
    let mut take = |len: usize| -> Result<&[u8], String> {
        let bytes = raw.get(at..at + len).ok_or_else(bad)?;
        at += len;
        Ok(bytes)
    };
    let new_sections = sections(new);
    let count = u32::from_le_bytes(take(4)?.try_into().unwrap()) as usize;
    let mut old = Vec::with_capacity(new.len());
    for i in 0..count {
        let len = u32::from_le_bytes(take(4)?.try_into().unwrap()) as usize;
        let aligned = take(1)?[0] != 0;
        let n = match new_sections.get(i) {
            Some(&(s, e)) if aligned => &new[s..e],
            _ => &[][..],
        };
        let shift = n.len() as isize - len as isize;
        for block_at in (0..len).step_by(BLOCK) {
            let size = BLOCK.min(len - block_at);
            let how = take(1)?[0];
            let from = if matches!(how, FROM_END | SAME_FROM_END) { block_at as isize + shift } else { block_at as isize };
            if matches!(how, SAME_FROM_START | SAME_FROM_END) {
                let from = usize::try_from(from).map_err(|_| bad())?;
                old.extend_from_slice(n.get(from..from + size).ok_or_else(bad)?);
            } else {
                let bytes = take(size)?;
                xor_into(&mut old, bytes, n, from);
            }
        }
    }
    Ok(old)
}

/// The states kept: the newest whole, with its emulated time, and the
/// deltas back to each one before it, with theirs, oldest first.
pub struct History {
    latest: Option<(u64, Vec<u8>)>,
    deltas: VecDeque<(u64, Vec<u8>)>,
    /// The deltas' bytes, and the most the deltas and the newest state may
    /// take.
    bytes: usize,
    budget: usize,
}

impl History {
    pub fn new(budget: usize) -> Self {
        Self { latest: None, deltas: VecDeque::new(), bytes: 0, budget }
    }

    /// Keep `state`, of emulated time `at_ns`, as the newest.
    pub fn push(&mut self, at_ns: u64, state: Vec<u8>) {
        if let Some((before, latest)) = self.latest.take() {
            let back = delta(&latest, &state);
            self.bytes += back.len();
            self.deltas.push_back((before, back));
        }
        self.latest = Some((at_ns, state));
        self.trim();
    }

    /// Drop the oldest states until they all fit the budget.
    fn trim(&mut self) {
        let whole = self.latest.as_ref().map_or(0, |(_, s)| s.len());
        while self.bytes + whole > self.budget
            && let Some((_, oldest)) = self.deltas.pop_front()
        {
            self.bytes -= oldest.len();
        }
    }

    /// Go back a state: the one before the newest becomes the newest, and
    /// is returned with its emulated time. None when there is none before.
    pub fn step_back(&mut self) -> Option<(u64, &[u8])> {
        let (at_ns, back) = self.deltas.pop_back()?;
        self.bytes -= back.len();
        let (_, latest) = self.latest.take()?;
        match undo(&latest, &back) {
            Ok(older) => self.latest = Some((at_ns, older)),
            Err(_) => {
                // Nothing before a damaged step can be reached either.
                self.deltas.clear();
                self.bytes = 0;
                self.latest = Some((at_ns, latest));
                return None;
            }
        }
        self.latest.as_ref().map(|(at, state)| (*at, state.as_slice()))
    }

    pub fn clear(&mut self) {
        self.latest = None;
        self.deltas.clear();
        self.bytes = 0;
    }

    pub fn set_budget(&mut self, budget: usize) {
        self.budget = budget;
        self.trim();
    }

    /// The states kept.
    pub fn len(&self) -> usize {
        self.latest.is_some() as usize + self.deltas.len()
    }

    pub fn is_empty(&self) -> bool {
        self.latest.is_none()
    }

    /// The memory the states take.
    pub fn bytes(&self) -> usize {
        self.bytes + self.latest.as_ref().map_or(0, |(_, s)| s.len())
    }
}

/// A `History` on a thread of its own, which packs the states the machine
/// hands it while the machine runs on.
#[cfg(not(target_arch = "wasm32"))]
pub struct Rewinder {
    tx: std::sync::mpsc::SyncSender<Message>,
}

#[cfg(not(target_arch = "wasm32"))]
enum Message {
    Push(u64, Vec<u8>),
    StepBack(std::sync::mpsc::Sender<Option<(u64, Vec<u8>)>>),
    Clear,
    Budget(usize),
}

#[cfg(not(target_arch = "wasm32"))]
impl Rewinder {
    pub fn start(budget: usize) -> Self {
        // One state waiting at most: a machine that takes them faster than
        // they are packed skips some.
        let (tx, rx) = std::sync::mpsc::sync_channel::<Message>(1);
        std::thread::spawn(move || {
            let mut history = History::new(budget);
            for message in rx {
                match message {
                    Message::Push(at_ns, state) => history.push(at_ns, state),
                    Message::StepBack(reply) => {
                        let _ = reply.send(history.step_back().map(|(at, state)| (at, state.to_vec())));
                    }
                    Message::Clear => history.clear(),
                    Message::Budget(bytes) => history.set_budget(bytes),
                }
            }
        });
        Self { tx }
    }

    /// Hand over a state, unless the last one is still being packed.
    /// Returns whether it was taken.
    pub fn offer(&self, at_ns: u64, state: Vec<u8>) -> bool {
        self.tx.try_send(Message::Push(at_ns, state)).is_ok()
    }

    /// Hand over a state, waiting for the thread to take it.
    pub fn push(&self, at_ns: u64, state: Vec<u8>) {
        let _ = self.tx.send(Message::Push(at_ns, state));
    }

    /// The state before the newest, which becomes the newest, with its
    /// emulated time.
    pub fn step_back(&self) -> Option<(u64, Vec<u8>)> {
        let (reply, answer) = std::sync::mpsc::channel();
        self.tx.send(Message::StepBack(reply)).ok()?;
        answer.recv().ok().flatten()
    }

    pub fn clear(&self) {
        let _ = self.tx.send(Message::Clear);
    }

    pub fn set_budget(&self, bytes: usize) {
        let _ = self.tx.send(Message::Budget(bytes));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A state of sections: a tag and its bytes each.
    fn state(parts: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut out = Vec::new();
        for (tag, bytes) in parts {
            out.extend_from_slice(*tag);
            out.extend_from_slice(&1u16.to_le_bytes());
            out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            out.extend_from_slice(bytes);
        }
        out
    }

    fn noise(seed: u32, len: usize) -> Vec<u8> {
        let mut x = seed.wrapping_mul(2_654_435_761).max(1);
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as u8
            })
            .collect()
    }

    #[test]
    fn a_delta_goes_back_exactly_and_is_small_where_little_changed() {
        let ram = noise(1, 1 << 20);
        let sound = noise(2, 200_000);
        let old = state(&[(b"RAM ", ram.clone()), (b"SOUN", sound.clone())]);
        // A few bytes of RAM changed, and a queue at the start of the sound
        // grew, moving what comes after it.
        let mut ram2 = ram.clone();
        ram2[1000] ^= 0xFF;
        ram2[500_000] = 7;
        let mut sound2 = vec![1, 2, 3];
        sound2.extend_from_slice(&sound);
        let new = state(&[(b"RAM ", ram2), (b"SOUN", sound2)]);
        let back = delta(&old, &new);
        assert_eq!(undo(&new, &back).unwrap(), old);
        assert!(back.len() < 20_000, "{} bytes for a few changes", back.len());

        // Sections that come and go, and states of other lengths.
        let fewer = state(&[(b"RAM ", noise(3, 5000))]);
        assert_eq!(undo(&fewer, &delta(&old, &fewer)).unwrap(), old);
        assert_eq!(undo(&old, &delta(&fewer, &old)).unwrap(), fewer);
        assert!(undo(&old, b"not a delta").is_err());
    }

    #[test]
    fn stepping_back_goes_through_the_states_newest_first_within_the_budget() {
        let states: Vec<Vec<u8>> = (0..20u32)
            .map(|n| {
                let mut ram = noise(9, 100_000);
                ram[n as usize * 1000..n as usize * 1000 + 100].fill(n as u8);
                state(&[(b"RAM ", ram), (b"CPU ", noise(n, 300 + n as usize))])
            })
            .collect();
        let mut history = History::new(usize::MAX);
        for (n, s) in states.iter().enumerate() {
            history.push(n as u64, s.clone());
        }
        assert_eq!(history.len(), 20);
        for k in 1..20 {
            let (at, s) = history.step_back().unwrap();
            assert_eq!((at, s), ((19 - k) as u64, states[19 - k].as_slice()), "{} steps back", k);
        }
        assert!(history.step_back().is_none(), "none before the first");

        // Without room for deltas, only the newest is kept; with room for
        // about half of them, the newer half.
        let mut history = History::new(0);
        for (n, s) in states.iter().enumerate() {
            history.push(n as u64, s.clone());
        }
        assert_eq!(history.len(), 1);
        let mut all = History::new(usize::MAX);
        for (n, s) in states.iter().enumerate() {
            all.push(n as u64, s.clone());
        }
        let whole = states[19].len();
        let budget = whole + (all.bytes() - whole) / 2;
        let mut history = History::new(budget);
        for (n, s) in states.iter().enumerate() {
            history.push(n as u64, s.clone());
        }
        assert!((5..16).contains(&history.len()), "{} kept", history.len());
        assert!(history.bytes() <= budget);
        let kept = history.len();
        let mut last = 19;
        while let Some((at, s)) = history.step_back() {
            last = at as usize;
            assert_eq!(s, states[last].as_slice());
        }
        assert_eq!(last, 20 - kept, "the oldest went");
    }
}
