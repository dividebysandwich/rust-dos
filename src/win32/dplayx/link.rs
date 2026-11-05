//! Reliable, ordered messages between two endpoints over IPX, which
//! loses, repeats and reorders packets: DirectPlay's guaranteed messages
//! and everything that changes a session go this way.
//!
//! A message is cut into pieces that each fit a packet, each with a flag
//! saying whether more of the message follows. Each piece gets the next
//! sequence number of the link and is sent again until the other end
//! acknowledges it: every packet between the two carries the highest
//! sequence number its sender has received in order, and an endpoint
//! that has received something and has nothing to send soon sends an
//! acknowledgement alone. Pieces that come early wait for the ones before
//! them. Times are the emulated clock's microseconds.

use super::wire::Addr;
use std::collections::VecDeque;

/// How long a piece waits for its acknowledgement before it is sent
/// again.
pub const RETRY: u64 = 200_000;
/// How long the other end may stay silent, or leave a piece
/// unacknowledged, before it is taken for gone.
pub const LOST: u64 = 20_000_000;
/// How long a link with nothing to send waits before it says it is
/// still there.
pub const KEEPALIVE: u64 = 2_000_000;
/// How many pieces are on their way at once.
const WINDOW: usize = 64;
/// How many early pieces are kept.
const EARLY: usize = 256;
/// The longest message put together; a longer one is dropped.
const LONGEST: usize = 4 << 20;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Piece {
    pub seq: u32,
    pub body: Vec<u8>,
    /// When it was sent last (0: not yet), and first.
    pub sent: u64,
    pub first: u64,
}

crate::state_fields!(Piece {
    seq,
    body,
    sent,
    first
});

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Link {
    pub peer: Addr,
    /// The sequence number of the next piece queued, from 1.
    pub next_seq: u32,
    /// The pieces not acknowledged yet, in order.
    pub unacked: VecDeque<Piece>,
    /// The sequence number the next piece received in order has.
    pub expected: u32,
    /// Pieces that came before those ahead of them.
    pub early: Vec<Piece>,
    /// The message the pieces received so far are of.
    pub partial: Vec<u8>,
    /// When something came from the other end last, and when something
    /// went to it.
    pub heard: u64,
    pub spoke: u64,
    /// Whether an acknowledgement is owed.
    pub ack_owed: bool,
}

crate::state_fields!(Link {
    peer,
    next_seq,
    unacked,
    expected,
    early,
    partial,
    heard,
    spoke,
    ack_owed
});

impl Link {
    pub fn new(peer: Addr, now: u64) -> Link {
        Link {
            peer,
            next_seq: 1,
            expected: 1,
            heard: now,
            spoke: now,
            ..Link::default()
        }
    }

    /// Queue `msg`, in pieces of at most `room` bytes with their flag.
    pub fn queue(&mut self, msg: &[u8], room: usize) {
        let room = room.max(2) - 1;
        let mut chunks = msg.chunks(room).peekable();
        if chunks.peek().is_none() {
            self.push(vec![0]);
            return;
        }
        while let Some(chunk) = chunks.next() {
            let mut body = Vec::with_capacity(chunk.len() + 1);
            body.push(chunks.peek().is_some() as u8);
            body.extend_from_slice(chunk);
            self.push(body);
        }
    }

    fn push(&mut self, body: Vec<u8>) {
        self.unacked.push_back(Piece {
            seq: self.next_seq,
            body,
            sent: 0,
            first: 0,
        });
        self.next_seq = self.next_seq.wrapping_add(1);
    }

    /// The pieces to send at `now`: those in the window not sent yet, and
    /// those whose acknowledgement is late.
    pub fn due(&mut self, now: u64) -> Vec<(u32, Vec<u8>)> {
        let mut out = Vec::new();
        for piece in self.unacked.iter_mut().take(WINDOW) {
            if piece.sent == 0 || now.saturating_sub(piece.sent) >= RETRY {
                if piece.first == 0 {
                    piece.first = now.max(1);
                }
                piece.sent = now.max(1);
                out.push((piece.seq, piece.body.clone()));
            }
        }
        if !out.is_empty() {
            self.spoke = now;
            self.ack_owed = false;
        }
        out
    }

    /// The other end acknowledged everything up to `ack`.
    pub fn acked(&mut self, ack: u32) {
        while self.unacked.front().is_some_and(|p| p.seq <= ack) {
            self.unacked.pop_front();
        }
    }

    /// The highest sequence number received in order.
    pub fn ack(&self) -> u32 {
        self.expected.wrapping_sub(1)
    }

    /// The piece `seq` came: the messages it completes, in order.
    pub fn received(&mut self, seq: u32, body: &[u8]) -> Vec<Vec<u8>> {
        self.ack_owed = true;
        let mut done = Vec::new();
        if seq < self.expected {
            return done;
        }
        if seq > self.expected {
            if self.early.len() < EARLY && !self.early.iter().any(|p| p.seq == seq) {
                self.early.push(Piece {
                    seq,
                    body: body.to_vec(),
                    ..Piece::default()
                });
            }
            return done;
        }
        self.take(body, &mut done);
        while let Some(i) = self.early.iter().position(|p| p.seq == self.expected) {
            let piece = self.early.swap_remove(i);
            self.take(&piece.body, &mut done);
        }
        done
    }

    fn take(&mut self, body: &[u8], done: &mut Vec<Vec<u8>>) {
        self.expected = self.expected.wrapping_add(1);
        let Some((&more, data)) = body.split_first() else {
            return;
        };
        if self.partial.len() + data.len() > LONGEST {
            self.partial.clear();
            return;
        }
        self.partial.extend_from_slice(data);
        if more == 0 {
            done.push(std::mem::take(&mut self.partial));
        }
    }

    /// Whether the other end is gone at `now`: silent, or not
    /// acknowledging, for too long.
    pub fn lost(&self, now: u64) -> bool {
        let stuck = self
            .unacked
            .front()
            .is_some_and(|p| p.first != 0 && now.saturating_sub(p.first) >= LOST);
        stuck || now.saturating_sub(self.heard) >= LOST
    }

    /// Whether an acknowledgement alone should go at `now`: one is owed,
    /// or the link has been quiet for a while.
    pub fn wants_ack(&self, now: u64) -> bool {
        self.ack_owed || now.saturating_sub(self.spoke) >= KEEPALIVE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair() -> (Link, Link) {
        (Link::new(Addr::default(), 0), Link::new(Addr::default(), 0))
    }

    #[test]
    fn messages_arrive_whole_and_in_order_through_loss_and_reordering() {
        let (mut a, mut b) = pair();
        let long: Vec<u8> = (0..1000u32).map(|i| i as u8).collect();
        a.queue(b"one", 100);
        a.queue(&long, 100);
        a.queue(b"", 100);
        let pieces = a.due(1);
        assert_eq!(pieces.len(), 1 + 11 + 1);
        // The second piece is lost, the rest come backwards.
        let mut got = Vec::new();
        for (seq, body) in pieces.iter().rev().filter(|(seq, _)| *seq != 2) {
            got.extend(b.received(*seq, body));
        }
        assert_eq!(got, vec![b"one".to_vec()]);
        assert_eq!(b.ack(), 1);
        a.acked(b.ack());
        assert!(a.due(2).is_empty(), "not time to send again yet");
        let again = a.due(2 + RETRY);
        assert_eq!(again.first().map(|p| p.0), Some(2));
        for (seq, body) in &again {
            got.extend(b.received(*seq, body));
        }
        assert_eq!(got, vec![b"one".to_vec(), long, Vec::new()]);
        // A piece received twice is delivered once.
        assert!(b.received(1, &pieces[0].1).is_empty());
        a.acked(b.ack());
        assert!(a.unacked.is_empty());
    }

    #[test]
    fn a_silent_or_deaf_peer_is_lost() {
        let (mut a, _) = pair();
        assert!(!a.lost(LOST - 1));
        assert!(a.lost(LOST));
        // It talks, but never acknowledges what was sent at 10.
        a.queue(b"x", 100);
        a.due(10);
        a.heard = LOST + 5;
        assert!(!a.lost(LOST + 9));
        assert!(a.lost(LOST + 10), "nothing acknowledged");
        assert!(!a.wants_ack(11));
        assert!(a.wants_ack(10 + KEEPALIVE));
    }
}
