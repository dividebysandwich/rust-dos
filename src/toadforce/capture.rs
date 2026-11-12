//! A recording of what the card was given to draw, for finding out why
//! two renderers draw a frame differently: the card's state and memory
//! at the start, each run of commands, and before each run the bytes
//! of card memory that something other than the card wrote since the
//! run before (the program's and the drivers' writes: textures, locked
//! surfaces). Played back into a card of the same state, the commands
//! draw the frames again, with any renderer and as often as wanted,
//! which a running program can't be made to do.
//!
//! The debugger starts one (`POST /api/toadforce/capture`), and
//! `gl::tests::replay_a_capture` plays one through the software
//! renderer and the OpenGL host draw by draw.
//!
//! The file: `TFCAP1\0\0`, the size of card memory, the size of the
//! card's saved state, the state, the memory, and then records until
//! the file's end. A record is a word that says what it is and one that
//! says how much: 1 and a count of words, a run's commands; 2 and a
//! count of bytes after the word of their offset, a write to card
//! memory. The file is whole after each run, so a recording that
//! wasn't ended is one up to its last run.

use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::path::Path;

const MAGIC: &[u8; 8] = b"TFCAP1\0\0";
const RUN: u32 = 1;
const PATCH: u32 = 2;

/// Bytes that are the same between two that differ and still go into
/// one record.
const GAP: usize = 32;

/// A recording being made.
pub struct Recorder {
    file: BufWriter<File>,
    /// The page flips still to record, or None to record until the
    /// recording is ended.
    flips: Option<u32>,
    /// Card memory as the last recorded run left it, empty before the
    /// first.
    after: Vec<u8>,
}

// A copy of a card records nothing.
impl Clone for Slot {
    fn clone(&self) -> Self {
        Slot(None)
    }
}

impl std::fmt::Debug for Slot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() {
            "recording"
        } else {
            "not recording"
        })
    }
}

/// The card's recorder, if it is recording. No part of its state.
#[derive(Default)]
pub struct Slot(pub Option<Box<Recorder>>);

impl Recorder {
    /// Record into the new file `path` until `flips` page flips were
    /// recorded, or with 0 until the recording is ended: a program
    /// that draws through a driver which copies its frames to the
    /// screen never flips.
    pub fn new(path: &Path, flips: u32) -> io::Result<Self> {
        Ok(Recorder {
            file: BufWriter::new(File::create(path)?),
            flips: (flips > 0).then_some(flips),
            after: Vec::new(),
        })
    }

    /// Before a run of `words`: the card's saved `state` and `memory`
    /// if this is the first, or else what was written to `memory`
    /// since the run before, and then the run.
    pub fn before(&mut self, state: &[u8], memory: &[u8], words: &[u32]) -> io::Result<()> {
        let f = &mut self.file;
        if self.after.is_empty() {
            f.write_all(MAGIC)?;
            f.write_all(&(memory.len() as u32).to_le_bytes())?;
            f.write_all(&(state.len() as u32).to_le_bytes())?;
            f.write_all(state)?;
            f.write_all(memory)?;
        } else {
            let mut at = 0;
            while let Some(first) = differ(&self.after, memory, at) {
                // To where `GAP` bytes are the same again.
                let mut end = first + 1;
                let mut same = 0;
                while end < memory.len() && same < GAP {
                    same = if self.after[end] == memory[end] {
                        same + 1
                    } else {
                        0
                    };
                    end += 1;
                }
                end -= same;
                f.write_all(&PATCH.to_le_bytes())?;
                f.write_all(&((end - first) as u32).to_le_bytes())?;
                f.write_all(&(first as u32).to_le_bytes())?;
                f.write_all(&memory[first..end])?;
                at = end;
            }
        }
        f.write_all(&RUN.to_le_bytes())?;
        f.write_all(&(words.len() as u32).to_le_bytes())?;
        for word in words {
            f.write_all(&word.to_le_bytes())?;
        }
        Ok(())
    }

    /// After the run, which flipped `flips` times and left `memory`.
    /// True when the recording is complete.
    pub fn after(&mut self, memory: &[u8], flips: u32) -> io::Result<bool> {
        self.after.clear();
        self.after.extend_from_slice(memory);
        self.file.flush()?;
        self.flips = self.flips.map(|left| left.saturating_sub(flips));
        Ok(self.flips == Some(0))
    }
}

/// The first byte from `at` on where `a` and `b` differ.
fn differ(a: &[u8], b: &[u8], at: usize) -> Option<usize> {
    // Eight bytes at a time over what is the same, which is most.
    let mut i = at;
    while i + 8 <= a.len() && a[i..i + 8] == b[i..i + 8] {
        i += 8;
    }
    (i..a.len()).find(|&i| a[i] != b[i])
}

/// One thing a recording has after its start.
pub enum Event {
    /// A run of commands.
    Run(Vec<u32>),
    /// Bytes written to card memory at an offset.
    Patch(u32, Vec<u8>),
}

/// A recording read back.
pub struct Recording {
    /// The card's saved state at the start.
    pub state: Vec<u8>,
    /// Card memory at the start.
    pub memory: Vec<u8>,
    pub events: Vec<Event>,
}

impl Recording {
    pub fn read(path: &Path) -> io::Result<Self> {
        let mut data = Vec::new();
        File::open(path)?.read_to_end(&mut data)?;
        let bad = || io::Error::new(io::ErrorKind::InvalidData, "not a ToadForce recording");
        let mut at = 0usize;
        let mut take = |n: usize| {
            let part = data.get(at..at.checked_add(n)?)?;
            at += n;
            Some(part)
        };
        let word = |part: Option<&[u8]>| Some(u32::from_le_bytes(part?.try_into().ok()?));
        if take(8) != Some(&MAGIC[..]) {
            return Err(bad());
        }
        let memory = word(take(4)).ok_or_else(bad)? as usize;
        let state = word(take(4)).ok_or_else(bad)? as usize;
        let state = take(state).ok_or_else(bad)?.to_vec();
        let memory = take(memory).ok_or_else(bad)?.to_vec();
        let mut events = Vec::new();
        // A record that is cut short, of a recording that wasn't ended,
        // ends it.
        while let Some(kind) = word(take(4)) {
            let Some(count) = word(take(4)) else { break };
            let count = count as usize;
            events.push(match kind {
                RUN => {
                    let Some(words) = take(4 * count) else { break };
                    let (words, _) = words.as_chunks::<4>();
                    Event::Run(words.iter().map(|w| u32::from_le_bytes(*w)).collect())
                }
                PATCH => {
                    let Some(offset) = word(take(4)) else { break };
                    let Some(bytes) = take(count) else { break };
                    Event::Patch(offset, bytes.to_vec())
                }
                _ => return Err(bad()),
            });
        }
        // Writes after the last run draw nothing.
        while let Some(Event::Patch(..)) = events.last() {
            events.pop();
        }
        Ok(Recording {
            state,
            memory,
            events,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recording_reads_back_as_it_was_made() {
        let path = std::env::temp_dir().join(format!("tfcap-{}.bin", std::process::id()));
        let mut memory = vec![0u8; 4096];
        let mut r = Recorder::new(&path, 2).unwrap();
        r.before(b"state", &memory, &[1, 2, 3]).unwrap();
        memory[10] = 7;
        assert!(!r.after(&memory, 1).unwrap());
        // Two writes close together are one record, a far one another.
        memory[100] = 1;
        memory[120] = 2;
        memory[4095] = 3;
        r.before(b"ignored", &memory, &[4]).unwrap();
        assert!(r.after(&memory, 1).unwrap());
        drop(r);

        let back = Recording::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(back.state, b"state");
        assert_eq!(back.memory, vec![0u8; 4096]);
        let mut seen = Vec::new();
        for event in &back.events {
            seen.push(match event {
                Event::Run(words) => format!("run {words:?}"),
                Event::Patch(at, bytes) => format!("patch {at} {}", bytes.len()),
            });
        }
        assert_eq!(
            seen,
            ["run [1, 2, 3]", "patch 100 21", "patch 4095 1", "run [4]"]
        );
    }

    /// A recording that counts no flips goes on until it is dropped,
    /// and one whose last record is cut short reads up to the run
    /// before it.
    #[test]
    fn a_recording_that_was_not_ended_reads_up_to_its_last_whole_run() {
        let path = std::env::temp_dir().join(format!("tfcap-cut-{}.bin", std::process::id()));
        let mut memory = vec![0u8; 256];
        let mut r = Recorder::new(&path, 0).unwrap();
        for run in 0..3u32 {
            memory[run as usize] = 1;
            r.before(b"state", &memory, &[run, run]).unwrap();
            assert!(!r.after(&memory, 5).unwrap());
        }
        drop(r);
        let whole = std::fs::read(&path).unwrap();
        std::fs::write(&path, &whole[..whole.len() - 2]).unwrap();
        let back = Recording::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        let runs: Vec<_> = back
            .events
            .iter()
            .filter_map(|event| match event {
                Event::Run(words) => Some(words[0]),
                Event::Patch(..) => None,
            })
            .collect();
        assert_eq!(runs, [0, 1]);
        assert!(matches!(back.events.last(), Some(Event::Run(_))));
    }
}
