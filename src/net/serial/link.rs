//! A serial link between two members of a LAN room, over the relay's
//! SERIAL datagrams, which may be lost or come out of order: a reliable,
//! ordered stream of records (characters, the lines, a modem's call) each
//! way, with numbered segments, acknowledgements and retransmission. Like
//! the tunnel's client, it is the protocol alone: datagrams and the time
//! (in ms) in, datagrams for the other member out.
//!
//! Each end picks a random session number when it starts. Every datagram
//! carries the sender's and, once known, the receiver's: an end that
//! starts again (another program, a new instance) has a new one, which
//! starts the stream over.
//!
//! ```text
//! HELLO  kind 1, sender's session u32, receiver's session u32 (0 unknown)
//! DATA   kind 2, sessions, seq u16, ack u16, stream bytes
//! ACK    kind 3, sessions, ack u16
//! ```
//!
//! `ack` is the next segment the sender of it expects.

use std::collections::{BTreeMap, VecDeque};

const HELLO: u8 = 1;
const DATA: u8 = 2;
const ACK: u8 = 3;
const HEADER: usize = 9;

/// Stream bytes in a segment, below the tunnel's `MAX_FRAGMENT` with room
/// for the headers.
const SEGMENT: usize = 1100;
/// Segments in flight, and how far ahead of the next expected one a
/// segment is kept.
const WINDOW: usize = 32;
/// HELLO while the other end doesn't know us; an ACK when nothing else
/// went for this long; the other end gone after this long without a word.
const HELLO_MS: u64 = 500;
const KEEPALIVE_MS: u64 = 1000;
const DEAD_MS: u64 = 10_000;
/// Retransmission timeouts.
const MIN_RTO_MS: u64 = 60;
const MAX_RTO_MS: u64 = 3000;
/// Records waiting to go, at most (while the other end reads nothing).
const PENDING: usize = 1024 * 1024;

/// What goes through the stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Record {
    Bytes(Vec<u8>),
    Lines { dtr: bool, rts: bool },
    /// A modem calls, and the called one answers, or either hangs up.
    Call,
    Answer,
    Hangup,
}

impl Record {
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Record::Bytes(bytes) => {
                for chunk in bytes.chunks(u16::MAX as usize) {
                    out.push(1);
                    out.extend_from_slice(&(chunk.len() as u16).to_be_bytes());
                    out.extend_from_slice(chunk);
                }
            }
            Record::Lines { dtr, rts } => out.extend([2, *dtr as u8 | (*rts as u8) << 1]),
            Record::Call => out.push(3),
            Record::Answer => out.push(4),
            Record::Hangup => out.push(5),
        }
    }

    /// The first record in `stream` and its length, if all of it is there.
    fn decode(stream: &[u8]) -> Option<(Option<Record>, usize)> {
        let record = match *stream.first()? {
            1 => {
                let len = u16::from_be_bytes([*stream.get(1)?, *stream.get(2)?]) as usize;
                let bytes = stream.get(3..3 + len)?;
                return Some((Some(Record::Bytes(bytes.to_vec())), 3 + len));
            }
            2 => {
                let bits = *stream.get(1)?;
                return Some((Some(Record::Lines { dtr: bits & 1 != 0, rts: bits & 2 != 0 }), 2));
            }
            3 => Record::Call,
            4 => Record::Answer,
            5 => Record::Hangup,
            // Something newer: skipped, a byte at a time.
            _ => return Some((None, 1)),
        };
        Some((Some(record), 1))
    }
}

/// What happened on the link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkEvent {
    /// Both ends know each other: the stream runs.
    Up,
    /// The other end went, or started over.
    Down,
    Record(Record),
}

struct Segment {
    seq: u16,
    data: Vec<u8>,
    sent: u64,
    tries: u32,
}

pub struct Link {
    /// The other end: its member index.
    pub peer: u8,
    mine: u32,
    theirs: u32,
    up: bool,
    next_seq: u16,
    in_flight: VecDeque<Segment>,
    pending: Vec<u8>,
    expected: u16,
    early: BTreeMap<u16, Vec<u8>>,
    stream: Vec<u8>,
    ack_due: bool,
    srtt: Option<f64>,
    rto: u64,
    hello_due: u64,
    last_heard: u64,
    last_sent: u64,
    /// Segments sent again, for the debugger.
    pub retransmits: u64,
}

/// Whether sequence number `a` comes before `b`.
fn before(a: u16, b: u16) -> bool {
    (b.wrapping_sub(a) as i16) > 0
}

impl Link {
    pub fn new(peer: u8, now: u64) -> Self {
        let mut mine = 0;
        while mine == 0 {
            mine = crate::net::random_u64() as u32;
        }
        Self {
            peer,
            mine,
            theirs: 0,
            up: false,
            next_seq: 0,
            in_flight: VecDeque::new(),
            pending: Vec::new(),
            expected: 0,
            early: BTreeMap::new(),
            stream: Vec::new(),
            ack_due: false,
            srtt: None,
            rto: 250,
            hello_due: now,
            last_heard: now,
            last_sent: now,
            retransmits: 0,
        }
    }

    pub fn is_up(&self) -> bool {
        self.up
    }

    /// The smoothed round trip, once measured.
    pub fn rtt_ms(&self) -> Option<u32> {
        self.srtt.map(|s| s as u32)
    }

    /// Put `record` in the stream to the other end. Dropped while the
    /// link is down, as a cable that isn't plugged in carries nothing.
    pub fn send(&mut self, record: &Record) {
        if self.up && self.pending.len() < PENDING {
            record.encode(&mut self.pending);
        }
    }

    fn header(&self, kind: u8) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER + 4);
        out.push(kind);
        out.extend_from_slice(&self.mine.to_be_bytes());
        out.extend_from_slice(&self.theirs.to_be_bytes());
        out
    }

    /// Forget the stream both ways: the other end is new, or gone.
    fn restart(&mut self, events: &mut Vec<LinkEvent>) {
        if self.up {
            events.push(LinkEvent::Down);
        }
        self.up = false;
        self.next_seq = 0;
        self.in_flight.clear();
        self.pending.clear();
        self.expected = 0;
        self.early.clear();
        self.stream.clear();
        self.ack_due = false;
    }

    /// Take a datagram from the other end.
    pub fn handle(&mut self, now: u64, payload: &[u8], out: &mut Vec<Vec<u8>>) -> Vec<LinkEvent> {
        let mut events = Vec::new();
        if payload.len() < HEADER {
            return events;
        }
        let kind = payload[0];
        let sender = u32::from_be_bytes(payload[1..5].try_into().unwrap());
        let receiver = u32::from_be_bytes(payload[5..9].try_into().unwrap());
        if sender == 0 {
            return events;
        }
        if sender != self.theirs {
            // Another end, or the same one started over.
            if self.theirs != 0 || self.up {
                self.restart(&mut events);
            }
            self.theirs = sender;
        }
        self.last_heard = now;
        if receiver != self.mine {
            // It doesn't know us (yet): say who we are.
            self.hello_due = now;
            self.poll_into(now, out, &mut events);
            return events;
        }
        if !self.up {
            self.up = true;
            events.push(LinkEvent::Up);
        }
        let body = &payload[HEADER..];
        match kind {
            HELLO => {
                // It knows us, and wants to hear that we know it.
                self.ack_due = true;
            }
            DATA if body.len() >= 4 => {
                let seq = u16::from_be_bytes([body[0], body[1]]);
                let ack = u16::from_be_bytes([body[2], body[3]]);
                self.acked(now, ack);
                self.ack_due = true;
                if seq == self.expected {
                    self.stream.extend_from_slice(&body[4..]);
                    self.expected = self.expected.wrapping_add(1);
                    while let Some(data) = self.early.remove(&self.expected) {
                        self.stream.extend_from_slice(&data);
                        self.expected = self.expected.wrapping_add(1);
                    }
                    self.take_records(&mut events);
                } else if before(self.expected, seq) && seq.wrapping_sub(self.expected) < WINDOW as u16 * 2 {
                    self.early.insert(seq, body[4..].to_vec());
                }
            }
            ACK if body.len() >= 2 => self.acked(now, u16::from_be_bytes([body[0], body[1]])),
            _ => {}
        }
        self.poll_into(now, out, &mut events);
        events
    }

    fn take_records(&mut self, events: &mut Vec<LinkEvent>) {
        let mut at = 0;
        while let Some((record, len)) = Record::decode(&self.stream[at..]) {
            at += len;
            if let Some(record) = record {
                events.push(LinkEvent::Record(record));
            }
        }
        self.stream.drain(..at);
    }

    /// The other end expects segment `ack` next: those before it arrived.
    fn acked(&mut self, now: u64, ack: u16) {
        while let Some(first) = self.in_flight.front() {
            if !before(first.seq, ack) {
                break;
            }
            if first.tries == 1 {
                let sample = now.saturating_sub(first.sent) as f64;
                let srtt = self.srtt.map_or(sample, |s| s * 0.875 + sample * 0.125);
                self.srtt = Some(srtt);
                self.rto = ((srtt * 2.0) as u64 + 20).clamp(MIN_RTO_MS, MAX_RTO_MS);
            }
            self.in_flight.pop_front();
        }
    }

    /// What is to go now: HELLO, new segments, those to send again, an
    /// acknowledgement or a keepalive. Also tells when the other end is
    /// gone.
    pub fn poll(&mut self, now: u64, out: &mut Vec<Vec<u8>>) -> Vec<LinkEvent> {
        let mut events = Vec::new();
        self.poll_into(now, out, &mut events);
        events
    }

    fn poll_into(&mut self, now: u64, out: &mut Vec<Vec<u8>>, events: &mut Vec<LinkEvent>) {
        if now.saturating_sub(self.last_heard) >= DEAD_MS && (self.up || self.theirs != 0) {
            self.restart(events);
            self.theirs = 0;
            self.hello_due = now;
        }
        if !self.up {
            if now >= self.hello_due {
                self.hello_due = now + HELLO_MS;
                self.last_sent = now;
                out.push(self.header(HELLO));
            }
            return;
        }
        // New segments.
        while !self.pending.is_empty() && self.in_flight.len() < WINDOW {
            let take = self.pending.len().min(SEGMENT);
            let data: Vec<u8> = self.pending.drain(..take).collect();
            let seq = self.next_seq;
            self.next_seq = self.next_seq.wrapping_add(1);
            out.push(self.data(seq, &data, now));
            self.in_flight.push_back(Segment { seq, data, sent: now, tries: 1 });
        }
        // Those not acknowledged in time.
        let rto = self.rto;
        let mut again = Vec::new();
        for segment in &mut self.in_flight {
            if now.saturating_sub(segment.sent) >= rto {
                segment.sent = now;
                segment.tries += 1;
                again.push((segment.seq, segment.data.clone()));
            }
        }
        if !again.is_empty() {
            self.retransmits += again.len() as u64;
            self.rto = (self.rto * 2).min(MAX_RTO_MS);
            for (seq, data) in again {
                out.push(self.data(seq, &data, now));
            }
        }
        if self.ack_due || now.saturating_sub(self.last_sent) >= KEEPALIVE_MS {
            let mut ack = self.header(ACK);
            ack.extend_from_slice(&self.expected.to_be_bytes());
            out.push(ack);
            self.ack_due = false;
            self.last_sent = now;
        }
    }

    /// A DATA datagram, which acknowledges what came too.
    fn data(&mut self, seq: u16, data: &[u8], now: u64) -> Vec<u8> {
        let mut out = self.header(DATA);
        out.extend_from_slice(&seq.to_be_bytes());
        out.extend_from_slice(&self.expected.to_be_bytes());
        out.extend_from_slice(data);
        self.ack_due = false;
        self.last_sent = now;
        out
    }

    /// When `poll` has something to do next.
    pub fn next_due(&self) -> u64 {
        let mut due = self.last_heard + DEAD_MS;
        if !self.up {
            return due.min(self.hello_due);
        }
        due = due.min(self.last_sent + KEEPALIVE_MS);
        if let Some(first) = self.in_flight.iter().map(|s| s.sent).min() {
            due = due.min(first + self.rto);
        }
        if self.ack_due || (!self.pending.is_empty() && self.in_flight.len() < WINDOW) {
            due = 0;
        }
        due
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two ends, and the datagrams between them, lost or reordered as a
    /// test wants.
    struct Pair {
        a: Link,
        b: Link,
        now: u64,
        events_a: Vec<LinkEvent>,
        events_b: Vec<LinkEvent>,
    }

    impl Pair {
        fn new() -> Self {
            Pair { a: Link::new(2, 0), b: Link::new(1, 0), now: 0, events_a: Vec::new(), events_b: Vec::new() }
        }

        /// Run for `ms`, passing what goes each way through `filter`.
        fn run(&mut self, ms: u64, mut filter: impl FnMut(bool, usize, Vec<u8>) -> Option<Vec<u8>>) {
            let (mut to_a, mut to_b): (Vec<Vec<u8>>, Vec<Vec<u8>>) = (Vec::new(), Vec::new());
            let mut n = 0;
            for _ in 0..ms {
                self.now += 1;
                let mut out = Vec::new();
                self.events_a.extend(self.a.poll(self.now, &mut out));
                to_b.extend(out.drain(..).filter_map(|d| {
                    n += 1;
                    filter(true, n, d)
                }));
                self.events_b.extend(self.b.poll(self.now, &mut out));
                to_a.extend(out.drain(..).filter_map(|d| {
                    n += 1;
                    filter(false, n, d)
                }));
                for d in std::mem::take(&mut to_b) {
                    self.events_b.extend(self.b.handle(self.now, &d, &mut out));
                }
                to_a.append(&mut out);
                for d in std::mem::take(&mut to_a) {
                    self.events_a.extend(self.a.handle(self.now, &d, &mut out));
                }
                to_b.append(&mut out);
            }
        }

        fn received(events: &[LinkEvent]) -> Vec<u8> {
            events
                .iter()
                .filter_map(|e| match e {
                    LinkEvent::Record(Record::Bytes(b)) => Some(b.clone()),
                    _ => None,
                })
                .flatten()
                .collect()
        }
    }

    #[test]
    fn comes_up_and_carries_records() {
        let mut p = Pair::new();
        p.run(20, |_, _, d| Some(d));
        assert!(p.a.is_up() && p.b.is_up());
        assert_eq!(p.events_a, [LinkEvent::Up]);
        assert_eq!(p.events_b, [LinkEvent::Up]);
        p.a.send(&Record::Lines { dtr: true, rts: false });
        p.a.send(&Record::Bytes(b"hello".to_vec()));
        p.a.send(&Record::Call);
        p.run(10, |_, _, d| Some(d));
        assert_eq!(
            p.events_b[1..],
            [
                LinkEvent::Record(Record::Lines { dtr: true, rts: false }),
                LinkEvent::Record(Record::Bytes(b"hello".to_vec())),
                LinkEvent::Record(Record::Call),
            ]
        );
    }

    #[test]
    fn survives_loss_and_reordering() {
        let mut p = Pair::new();
        p.run(20, |_, _, d| Some(d));
        let sent: Vec<u8> = (0..50_000u32).map(|i| (i * 7 % 251) as u8).collect();
        for chunk in sent.chunks(300) {
            p.a.send(&Record::Bytes(chunk.to_vec()));
        }
        // Every third datagram lost, both ways.
        let mut held: Option<Vec<u8>> = None;
        p.run(20_000, |_, n, d| {
            if n % 3 == 0 {
                return None;
            }
            // And every fifth held back behind the next.
            if n % 5 == 0 {
                return held.replace(d);
            }
            Some(d)
        });
        assert_eq!(Pair::received(&p.events_b), sent);
        assert!(p.a.retransmits > 0);
        assert!(p.a.rtt_ms().is_some());
    }

    #[test]
    fn a_new_session_starts_over() {
        let mut p = Pair::new();
        p.run(20, |_, _, d| Some(d));
        // B starts again (another instance for the same member).
        p.b = Link::new(1, p.now);
        p.events_a.clear();
        p.run(20, |_, _, d| Some(d));
        assert_eq!(p.events_a, [LinkEvent::Down, LinkEvent::Up]);
        p.a.send(&Record::Bytes(b"x".to_vec()));
        p.run(10, |_, _, d| Some(d));
        assert_eq!(Pair::received(&p.events_b), b"x");
    }

    #[test]
    fn the_other_end_going_silent_takes_the_link_down() {
        let mut p = Pair::new();
        p.run(20, |_, _, d| Some(d));
        p.events_a.clear();
        p.run(DEAD_MS + 10, |to_b, _, d| (!to_b).then_some(d).filter(|_| false));
        assert_eq!(p.events_a, [LinkEvent::Down]);
        assert!(!p.a.is_up());
    }
}
