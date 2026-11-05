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
//!
//! Each end numbers its links as it makes them (`Link::id`), and every
//! packet of a link carries that number and the sequence number of the
//! oldest piece its sender still waits to have acknowledged (`base`).
//! An end that drops its link (the peer left, or went silent) and later
//! makes a new one to the same peer starts again from 1, while the peer
//! may still have the old link: a packet with a newer link number than
//! the one heard before tells the peer to receive from `base` on, and one
//! with an older number is a straggler of the old link, which is
//! ignored. Without that, the two ends would wait for pieces that never
//! come, or take the new ones for repeats.

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
    /// This end's number for the link, and the peer's for its own, 0
    /// until a packet of it came.
    pub id: u32,
    pub peer_link: u32,
    /// The sequence number of the next piece queued, from 1.
    pub next_seq: u32,
    /// The pieces not acknowledged yet, in order.
    pub unacked: VecDeque<Piece>,
    /// The sequence number the next piece received in order has.
    pub expected: u32,
    /// Pieces that came before those ahead of them.
    pub early: Vec<Piece>,
    /// The message the pieces received so far are of, and whether the
    /// rest of a message too long to keep is being skipped.
    pub partial: Vec<u8>,
    pub discarding: bool,
    /// When something came from the other end last, and when something
    /// went to it.
    pub heard: u64,
    pub spoke: u64,
    /// Whether an acknowledgement is owed.
    pub ack_owed: bool,
}

crate::state_fields!(Link {
    peer,
    id,
    peer_link,
    next_seq,
    unacked,
    expected,
    early,
    partial,
    discarding,
    heard,
    spoke,
    ack_owed
});

impl Link {
    /// A new link to `peer`, this end's `id`th.
    pub fn new(peer: Addr, id: u32, now: u64) -> Link {
        Link {
            peer,
            id,
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

    /// The sequence number of the oldest piece not acknowledged yet, or
    /// of the next one when every piece is: where the peer receives from
    /// when it hears this link first.
    pub fn base(&self) -> u32 {
        self.unacked.front().map_or(self.next_seq, |p| p.seq)
    }

    /// A packet of the peer's link `peer_link` came, whose oldest piece
    /// not acknowledged is `base`: false if it is of a link older than
    /// the one heard before, which the packet is to be ignored for. A
    /// newer one starts receiving again from `base`.
    pub fn heard_link(&mut self, peer_link: u32, base: u32) -> bool {
        if peer_link == self.peer_link {
            return true;
        }
        let newer = self.peer_link == 0 || (peer_link.wrapping_sub(self.peer_link) as i32) > 0;
        if !newer {
            return false;
        }
        self.peer_link = peer_link;
        self.expected = base;
        self.early.clear();
        self.partial.clear();
        self.discarding = false;
        true
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
        // A message too long to keep goes whole, its last piece too.
        if self.discarding || self.partial.len() + data.len() > LONGEST {
            self.partial.clear();
            self.discarding = more != 0;
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
        (
            Link::new(Addr::default(), 1, 0),
            Link::new(Addr::default(), 1, 0),
        )
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

    #[test]
    fn a_new_link_of_the_peer_is_received_from_its_base() {
        let (mut a, mut b) = pair();
        assert!(b.heard_link(a.id, a.base()));
        a.queue(b"one", 100);
        a.queue(b"two", 100);
        for (seq, body) in a.due(1) {
            b.received(seq, &body);
        }
        assert_eq!(b.ack(), 2);
        // a dropped its link and made a new one, which starts from 1: b
        // takes it from there, and not for repeats of the old one's.
        let mut a = Link::new(Addr::default(), 2, 5);
        a.queue(b"three", 100);
        assert!(b.heard_link(a.id, a.base()));
        let got: Vec<Vec<u8>> = a
            .due(5)
            .iter()
            .flat_map(|(s, p)| b.received(*s, p))
            .collect();
        assert_eq!(got, vec![b"three".to_vec()]);
        // A straggler of the old link changes nothing.
        assert!(!b.heard_link(1, 9));
        assert_eq!(b.ack(), 1);
        // b dropped its own: a hears of the new one, from its base.
        let mut b = Link::new(Addr::default(), 2, 5);
        b.queue(b"x", 100);
        b.due(5);
        b.acked(1);
        assert!(a.heard_link(b.id, b.base()));
        assert_eq!(a.ack(), 1, "b's new link has nothing for a yet");
    }

    #[test]
    fn a_message_too_long_goes_whole() {
        let (mut a, mut b) = pair();
        let long = vec![7u8; LONGEST + 100];
        a.queue(&long, 1 << 20);
        a.queue(b"after", 1 << 20);
        let got: Vec<Vec<u8>> = a
            .due(1)
            .iter()
            .flat_map(|(s, p)| b.received(*s, p))
            .collect();
        assert_eq!(got, vec![b"after".to_vec()]);
    }
}
