//! A member of a relay's room: joins it, keeps its place with keepalives,
//! joins again when the relay forgets it or stops answering, and sends and
//! receives the room's frames. Like `Relay`, it is the protocol alone:
//! datagrams and the time in, datagrams for the relay out.

use super::auth;
use super::frag::{self, Reassembler};
use super::wire::{self, Message, RejectReason};
use std::net::SocketAddr;

/// How often HELLO and JOIN are sent again while the relay doesn't answer,
/// at first and after `QUICK_TRIES`.
const RETRY_MS: u64 = 1000;
const SLOW_RETRY_MS: u64 = 5000;
const QUICK_TRIES: u32 = 5;
/// A joined member that hears nothing from the relay for this long joins
/// again.
const SILENCE_MS: u64 = 16_000;

#[derive(Clone, Debug)]
pub struct ClientConfig {
    pub relay: SocketAddr,
    pub room: String,
    pub password: Option<String>,
}

/// Where the client is with the relay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Sending HELLO, waiting for CHALLENGE.
    Hello,
    /// Sending JOIN, waiting for WELCOME.
    Joining {
        cookie: [u8; 16],
    },
    Joined {
        token: u64,
        index: u8,
    },
    /// Turned away for good: a wrong password, a full room, another
    /// version.
    Rejected(RejectReason),
}

/// What happened, for the owner to act on or tell the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientEvent {
    Joined {
        index: u8,
        members: u16,
    },
    Rejected(RejectReason),
    /// The relay stopped answering; the client is joining again.
    Lost,
    Frame(Vec<u8>),
}

pub struct Client {
    config: ClientConfig,
    /// The room's key with the password (`auth::room_key`).
    key: [u8; 32],
    client_id: u64,
    phase: Phase,
    last_sent: u64,
    tries: u32,
    last_heard: u64,
    last_keepalive: u64,
    keepalive_ms: u64,
    members: u16,
    rtt_ms: Option<u32>,
    seq: u16,
    reassembler: Reassembler<(u8, u16)>,
}

impl Client {
    /// Start joining, with `out` taking the first HELLO.
    pub fn new(config: ClientConfig, client_id: u64, now: u64, out: &mut Vec<Vec<u8>>) -> Self {
        let mut client = Self {
            key: auth::room_key(config.password.as_deref(), &config.room),
            config,
            client_id,
            phase: Phase::Hello,
            last_sent: now,
            tries: 0,
            last_heard: now,
            last_keepalive: now,
            keepalive_ms: 5000,
            members: 0,
            rtt_ms: None,
            seq: 0,
            reassembler: Reassembler::new(),
        };
        client.send_hello(now, out);
        client
    }

    pub fn config(&self) -> &ClientConfig {
        &self.config
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// Members in the room, as the relay last said.
    pub fn members(&self) -> u16 {
        self.members
    }

    /// The last round trip to the relay.
    pub fn rtt_ms(&self) -> Option<u32> {
        self.rtt_ms
    }

    fn send_hello(&mut self, now: u64, out: &mut Vec<Vec<u8>>) {
        self.phase = Phase::Hello;
        self.last_sent = now;
        let hello = Message::Hello { client_id: self.client_id, room: self.config.room.clone() };
        out.push(wire::encode(0, &hello));
    }

    /// JOIN, with the room's key if this makes the room (`fresh`).
    fn send_join(&mut self, now: u64, cookie: [u8; 16], fresh: bool, out: &mut Vec<Vec<u8>>) {
        self.phase = Phase::Joining { cookie };
        self.last_sent = now;
        let room = &self.config.room;
        let proof = auth::proof(&self.key, room, self.client_id, &cookie);
        let key = if fresh { self.key } else { [0; 32] };
        let join = Message::Join { client_id: self.client_id, cookie, room: room.clone(), proof, key };
        out.push(wire::encode(0, &join));
    }

    /// Take datagram `bytes` that came from `from`.
    pub fn handle(&mut self, now: u64, from: SocketAddr, bytes: &[u8], out: &mut Vec<Vec<u8>>) -> Vec<ClientEvent> {
        let Ok(packet) = wire::decode(bytes) else { return Vec::new() };
        let from_relay = from == self.config.relay;
        let mut events = Vec::new();
        match (self.phase, packet.message) {
            (Phase::Hello, Message::Challenge { cookie, fresh, .. }) if from_relay => {
                self.last_heard = now;
                self.send_join(now, cookie, fresh, out);
            }
            (Phase::Joining { .. }, Message::Welcome { index, keepalive, members }) if from_relay => {
                self.phase = Phase::Joined { token: packet.token, index };
                self.last_heard = now;
                self.last_keepalive = now;
                self.tries = 0;
                self.keepalive_ms = (keepalive.max(1) as u64) * 1000;
                self.members = members;
                events.push(ClientEvent::Joined { index, members });
            }
            (Phase::Hello | Phase::Joining { .. }, Message::Reject { reason }) if from_relay => match reason {
                // Too slow between HELLO and JOIN, or joining again after
                // the relay forgot us: start over.
                RejectReason::Cookie | RejectReason::Unknown => self.send_hello(now, out),
                reason => {
                    self.phase = Phase::Rejected(reason);
                    events.push(ClientEvent::Rejected(reason));
                }
            },
            (Phase::Joined { .. }, Message::Reject { reason: RejectReason::Unknown }) if from_relay => {
                self.send_hello(now, out);
            }
            (Phase::Joined { token, .. }, Message::Ack { stamp, members }) if packet.token == token => {
                self.last_heard = now;
                self.members = members;
                self.rtt_ms = Some((now as u32).wrapping_sub(stamp));
            }
            (Phase::Joined { token, .. }, Message::Data { source, seq, fragment, count, payload })
                if packet.token == token =>
            {
                self.last_heard = now;
                if let Some(frame) = self.reassembler.add(now, (source, seq), fragment, count, &payload) {
                    events.push(ClientEvent::Frame(frame));
                }
            }
            _ => {}
        }
        events
    }

    /// Send again what went unanswered, and keepalives.
    pub fn tick(&mut self, now: u64, out: &mut Vec<Vec<u8>>) -> Vec<ClientEvent> {
        let mut events = Vec::new();
        let retry = if self.tries < QUICK_TRIES { RETRY_MS } else { SLOW_RETRY_MS };
        match self.phase {
            Phase::Hello | Phase::Joining { .. } if now.saturating_sub(self.last_sent) >= retry => {
                self.tries += 1;
                // A cookie gets old: ask for a new one rather than resend.
                self.send_hello(now, out);
            }
            Phase::Joined { token, .. } => {
                if now.saturating_sub(self.last_heard) >= SILENCE_MS {
                    events.push(ClientEvent::Lost);
                    self.send_hello(now, out);
                } else if now.saturating_sub(self.last_keepalive) >= self.keepalive_ms {
                    self.last_keepalive = now;
                    out.push(wire::encode(token, &Message::Keepalive { stamp: now as u32 }));
                }
            }
            _ => {}
        }
        self.reassembler.expire(now);
        events
    }

    /// Send `frame` to the room, if joined.
    pub fn send_frame(&mut self, frame: &[u8], out: &mut Vec<Vec<u8>>) {
        let Phase::Joined { token, .. } = self.phase else { return };
        self.seq = self.seq.wrapping_add(1);
        let pieces = frag::split(frame);
        for (i, piece) in pieces.iter().enumerate() {
            let data = Message::Data {
                source: 0,
                seq: self.seq,
                fragment: i as u8,
                count: pieces.len() as u8,
                payload: piece.to_vec(),
            };
            out.push(wire::encode(token, &data));
        }
    }

    /// Say goodbye, if joined.
    pub fn leave(&mut self, out: &mut Vec<Vec<u8>>) {
        if let Phase::Joined { token, .. } = self.phase {
            out.push(wire::encode(token, &Message::Leave));
        }
        self.phase = Phase::Hello;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::frame::{self, ETHERTYPE_IPX, Mac};
    use crate::net::tunnel::relay::{Outgoing, Relay, RelayConfig};

    fn addr(n: u16) -> SocketAddr {
        SocketAddr::from(([192, 0, 2, 1], 4000 + n))
    }

    const RELAY: u16 = 0;

    /// A relay and its clients, with datagrams passed between them
    /// unless `lose` says to drop them.
    struct Lan {
        relay: Relay,
        clients: Vec<Client>,
        events: Vec<Vec<ClientEvent>>,
        lose: bool,
    }

    impl Lan {
        fn new(password: Option<&str>) -> Self {
            let config = RelayConfig { password: password.map(String::from), ..Default::default() };
            Self { relay: Relay::new(config), clients: Vec::new(), events: Vec::new(), lose: false }
        }

        fn add(&mut self, now: u64, room: &str, password: Option<&str>) -> usize {
            let config = ClientConfig { relay: addr(RELAY), room: room.into(), password: password.map(String::from) };
            let mut out = Vec::new();
            let n = self.clients.len();
            self.clients.push(Client::new(config, 100 + n as u64, now, &mut out));
            self.events.push(Vec::new());
            self.deliver(now, n, out);
            n
        }

        /// Pass the datagrams client `n` sent to the relay, and what that
        /// sets off.
        fn deliver(&mut self, now: u64, n: usize, datagrams: Vec<Vec<u8>>) {
            let mut to_clients: Vec<Outgoing> = Vec::new();
            for d in datagrams {
                if !self.lose {
                    self.relay.handle(now, addr(n as u16 + 1), &d, &mut to_clients);
                }
            }
            for o in to_clients {
                let m = (o.to.port() - 4001) as usize;
                let mut out = Vec::new();
                let events = self.clients[m].handle(now, addr(RELAY), &o.bytes, &mut out);
                self.events[m].extend(events);
                self.deliver(now, m, out);
            }
        }

        fn tick(&mut self, now: u64) {
            self.relay.tick(now);
            for n in 0..self.clients.len() {
                let mut out = Vec::new();
                let events = self.clients[n].tick(now, &mut out);
                self.events[n].extend(events);
                self.deliver(now, n, out);
            }
        }

        fn send(&mut self, now: u64, n: usize, frame: &[u8]) {
            let mut out = Vec::new();
            self.clients[n].send_frame(frame, &mut out);
            self.deliver(now, n, out);
        }

        fn take(&mut self, n: usize) -> Vec<ClientEvent> {
            std::mem::take(&mut self.events[n])
        }
    }

    #[test]
    fn joins_and_exchanges_frames() {
        let mut lan = Lan::new(Some("pw"));
        let a = lan.add(0, "doom", Some("pw"));
        let b = lan.add(0, "doom", Some("pw"));
        assert_eq!(lan.take(a), vec![ClientEvent::Joined { index: 1, members: 1 }]);
        assert_eq!(lan.take(b), vec![ClientEvent::Joined { index: 2, members: 2 }]);
        let long = frame::build(Mac::BROADCAST, Mac([2, 0, 0, 0, 0, 1]), ETHERTYPE_IPX, &[5; 1400]);
        lan.send(1, a, &long);
        assert_eq!(lan.take(b), vec![ClientEvent::Frame(long)]);
        assert!(lan.take(a).is_empty());
        // Keepalives keep both in, and time the round trip.
        for t in (1000..40_000).step_by(1000) {
            lan.tick(t);
        }
        assert!(lan.take(a).is_empty() && lan.take(b).is_empty());
        assert_eq!(lan.clients[a].members(), 2);
        assert_eq!(lan.clients[a].rtt_ms(), Some(0));
    }

    #[test]
    fn the_first_member_sets_the_room_password() {
        let mut lan = Lan::new(None);
        let a = lan.add(0, "doom", Some("pw"));
        let b = lan.add(0, "doom", Some("pw"));
        let c = lan.add(0, "doom", None);
        let d = lan.add(0, "duke", None);
        let e = lan.add(0, "duke", Some("pw"));
        assert_eq!(lan.take(a), vec![ClientEvent::Joined { index: 1, members: 1 }]);
        assert_eq!(lan.take(b), vec![ClientEvent::Joined { index: 2, members: 2 }]);
        assert_eq!(lan.take(c), vec![ClientEvent::Rejected(RejectReason::Password)]);
        assert_eq!(lan.take(d), vec![ClientEvent::Joined { index: 1, members: 1 }]);
        assert_eq!(lan.take(e), vec![ClientEvent::Rejected(RejectReason::Open)]);
        assert!(lan.relay.rooms().iter().any(|r| r.name == "doom" && r.password));
    }

    #[test]
    fn a_wrong_password_ends_the_attempt() {
        let mut lan = Lan::new(Some("pw"));
        let a = lan.add(0, "doom", Some("nope"));
        assert_eq!(lan.take(a), vec![ClientEvent::Rejected(RejectReason::Password)]);
        assert_eq!(lan.clients[a].phase(), Phase::Rejected(RejectReason::Password));
        let mut out = Vec::new();
        lan.clients[a].tick(10_000, &mut out);
        assert!(out.is_empty(), "no more tries");
    }

    #[test]
    fn keeps_trying_while_the_relay_is_away_and_rejoins_after() {
        let mut lan = Lan::new(None);
        lan.lose = true;
        let a = lan.add(0, "doom", None);
        let mut sent = 0;
        for t in (100..30_000).step_by(100) {
            let mut out = Vec::new();
            lan.clients[a].tick(t, &mut out);
            sent += out.len();
        }
        // Five quick tries a second apart, then one every five seconds.
        assert_eq!(sent, 5 + 4);
        lan.lose = false;
        lan.tick(31_000);
        assert_eq!(lan.take(a), vec![ClientEvent::Joined { index: 1, members: 1 }]);
        // The relay restarts and forgets everyone: the next keepalive is
        // refused, and the client joins again.
        lan.relay = Relay::new(RelayConfig::default());
        lan.tick(36_000);
        assert_eq!(lan.take(a), vec![ClientEvent::Joined { index: 1, members: 1 }]);
        // The relay goes silent: after a while the client says so and
        // starts over.
        lan.lose = true;
        for t in (37_000..60_000).step_by(1000) {
            lan.tick(t);
        }
        assert_eq!(lan.take(a), vec![ClientEvent::Lost]);
        assert_eq!(lan.clients[a].phase(), Phase::Hello);
    }

    #[test]
    fn ignores_strangers() {
        let mut lan = Lan::new(None);
        let a = lan.add(0, "doom", None);
        lan.take(a);
        let data =
            wire::encode(12345, &Message::Data { source: 1, seq: 1, fragment: 0, count: 1, payload: vec![0; 60] });
        let mut out = Vec::new();
        assert!(lan.clients[a].handle(1, addr(9), &data, &mut out).is_empty());
        let reject = wire::encode(0, &Message::Reject { reason: RejectReason::Unknown });
        lan.clients[a].handle(1, addr(9), &reject, &mut out);
        assert!(out.is_empty());
        assert!(matches!(lan.clients[a].phase(), Phase::Joined { .. }));
    }
}
