//! Frames longer than `MAX_FRAGMENT` cross the tunnel in pieces, which the
//! receiver puts back together. A frame whose pieces don't all arrive
//! within `TIMEOUT_MS` is dropped, as a frame lost on a real Ethernet is.

use super::wire::MAX_FRAGMENT;
use std::collections::HashMap;
use std::hash::Hash;

/// How long the pieces of one frame may take to arrive.
pub const TIMEOUT_MS: u64 = 1000;
/// Frames being put together at once, per reassembler.
const MAX_PENDING: usize = 64;
/// The most pieces a frame may have (4800 bytes, well above Ethernet's).
pub const MAX_COUNT: u8 = 4;

/// `frame` cut into pieces of at most `MAX_FRAGMENT` bytes.
pub fn split(frame: &[u8]) -> Vec<&[u8]> {
    frame.chunks(MAX_FRAGMENT).collect()
}

struct Pending {
    pieces: Vec<Option<Vec<u8>>>,
    missing: usize,
    started: u64,
}

/// Puts frames back together, each known by a key of the caller's (a
/// sender and its frame number).
pub struct Reassembler<K> {
    pending: HashMap<K, Pending>,
}

impl<K: Eq + Hash + Copy> Default for Reassembler<K> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Eq + Hash + Copy> Reassembler<K> {
    pub fn new() -> Self {
        Self { pending: HashMap::new() }
    }

    /// Take piece `index` of `count` of frame `key` at `now` (ms), and
    /// return the frame once all its pieces are in.
    pub fn add(&mut self, now: u64, key: K, index: u8, count: u8, piece: &[u8]) -> Option<Vec<u8>> {
        if count == 0 || index >= count || count > MAX_COUNT {
            return None;
        }
        if count == 1 {
            return Some(piece.to_vec());
        }
        if !self.pending.contains_key(&key) {
            self.expire(now);
            if self.pending.len() >= MAX_PENDING {
                // Make room by giving up on the oldest.
                let oldest = self.pending.iter().min_by_key(|(_, p)| p.started).map(|(k, _)| *k)?;
                self.pending.remove(&oldest);
            }
        }
        let pending = self.pending.entry(key).or_insert_with(|| Pending {
            pieces: vec![None; count as usize],
            missing: count as usize,
            started: now,
        });
        if pending.pieces.len() != count as usize {
            // The number was reused for a frame of another size.
            *pending = Pending { pieces: vec![None; count as usize], missing: count as usize, started: now };
        }
        let slot = &mut pending.pieces[index as usize];
        if slot.is_none() {
            *slot = Some(piece.to_vec());
            pending.missing -= 1;
        }
        if pending.missing > 0 {
            return None;
        }
        let pending = self.pending.remove(&key)?;
        Some(pending.pieces.into_iter().flatten().flatten().collect())
    }

    /// Drop the frames whose pieces took too long.
    pub fn expire(&mut self, now: u64) {
        self.pending.retain(|_, p| now.saturating_sub(p.started) < TIMEOUT_MS);
    }

    /// Frames still waiting for pieces.
    pub fn pending(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(len: usize) -> Vec<u8> {
        (0..len).map(|i| i as u8).collect()
    }

    #[test]
    fn splits_and_joins_in_any_order() {
        let long = frame(1514);
        let pieces = split(&long);
        assert_eq!(pieces.len(), 2);
        assert!(pieces.iter().all(|p| p.len() <= MAX_FRAGMENT));
        let mut r = Reassembler::new();
        assert_eq!(r.add(0, (1, 7), 1, 2, pieces[1]), None);
        assert_eq!(r.pending(), 1);
        assert_eq!(r.add(5, (1, 7), 1, 2, pieces[1]), None, "a repeated piece counts once");
        assert_eq!(r.add(10, (1, 7), 0, 2, pieces[0]), Some(long));
        assert_eq!(r.pending(), 0);
        assert_eq!(split(&frame(60)).len(), 1);
        assert_eq!(r.add(0, (1, 8), 0, 1, &frame(60)), Some(frame(60)));
    }

    #[test]
    fn keeps_frames_of_different_senders_apart() {
        let (a, b) = (frame(2000), vec![9u8; 2000]);
        let mut r = Reassembler::new();
        assert_eq!(r.add(0, (1, 1), 0, 2, split(&a)[0]), None);
        assert_eq!(r.add(0, (2, 1), 0, 2, split(&b)[0]), None);
        assert_eq!(r.add(0, (2, 1), 1, 2, split(&b)[1]), Some(b));
        assert_eq!(r.add(0, (1, 1), 1, 2, split(&a)[1]), Some(a));
    }

    #[test]
    fn drops_frames_with_lost_pieces() {
        let long = frame(1514);
        let pieces = split(&long);
        let mut r = Reassembler::new();
        assert_eq!(r.add(0, 1u16, 0, 2, pieces[0]), None);
        // The rest comes too late: the first piece is gone by then.
        r.expire(TIMEOUT_MS);
        assert_eq!(r.pending(), 0);
        assert_eq!(r.add(TIMEOUT_MS, 1, 1, 2, pieces[1]), None);
        // Too many at once: the oldest is given up.
        let mut r = Reassembler::new();
        for key in 0..MAX_PENDING as u16 + 1 {
            r.add(key as u64, key, 0, 2, pieces[0]);
        }
        assert_eq!(r.pending(), MAX_PENDING);
        assert_eq!(r.add(100, 0, 1, 2, pieces[1]), None, "frame 0 was given up");
        // Nonsense numbering.
        assert_eq!(r.add(0, 500, 2, 2, pieces[0]), None);
        assert_eq!(r.add(0, 500, 0, MAX_COUNT + 1, pieces[0]), None);
    }
}
