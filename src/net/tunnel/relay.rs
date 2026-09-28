//! The relay that joins rust-dos instances into LANs. Clients join a room,
//! and each room is an Ethernet switch of its own: the relay learns which
//! member each source address belongs to, sends a frame for a known
//! address to that member only, and floods broadcasts, multicasts and
//! frames for unknown addresses to the whole room.
//!
//! A room is made by the first member to join it and goes when the last
//! one leaves. On a relay without a password of its own, the member who
//! makes a room decides whether it wants a password (`auth`). Anyone may
//! list the rooms (LIST), which is how a room browser finds them.
//!
//! `Relay` is the protocol alone, fed datagrams and the time and handing
//! back datagrams to send, so it can be tested without sockets.
//! `RelayServer` runs one on a UDP socket in a thread of its own.

use super::auth;
use super::frag::{self, Reassembler};
use super::wire::{self, DecodeError, Message, RejectReason, RoomInfo};
use crate::net::frame::{self, Mac};
use std::collections::HashMap;
use std::net::SocketAddr;

/// How often members send KEEPALIVE, in seconds.
pub const KEEPALIVE_S: u8 = 5;
/// A member not heard from for this long is dropped.
pub const MEMBER_TIMEOUT_MS: u64 = 30_000;
/// How long a dropped member's index is kept for it.
pub const RESERVE_MS: u64 = 60_000;
/// Members in one room: their indexes are 1 to this.
pub const MAX_ROOM: usize = 200;
/// Members in all rooms together, and from one IP address other than the
/// relay's own machine: a public relay can't be filled by one stranger,
/// and a LAN party behind one NAT still fits.
pub const MAX_MEMBERS: usize = 1000;
pub const MAX_PER_ADDRESS: usize = 32;
/// Source addresses one member may send from: its network card and its
/// IPX driver, and a little more.
pub const MAX_MACS: usize = 4;
/// Frames a member may send per second, and in a burst.
const RATE_PER_S: f64 = 5000.0;
const BURST: f64 = 10_000.0;

/// What a relay is: its name in OFFER and ROOMS, and the password all its
/// rooms want, if it has one.
#[derive(Clone, Debug, Default)]
pub struct RelayConfig {
    pub name: String,
    pub password: Option<String>,
    /// The port OFFER tells clients to join on.
    pub port: u16,
}

/// A datagram to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outgoing {
    pub to: SocketAddr,
    pub bytes: Vec<u8>,
}

struct Member {
    client_id: u64,
    room: String,
    index: u8,
    addr: SocketAddr,
    last_seen: u64,
    macs: Vec<Mac>,
    /// The frames it may still send now.
    allowance: f64,
    refilled: u64,
    /// Whether it was told off for too many addresses already.
    warned: bool,
}

/// The key of a room without a password.
const NO_KEY: [u8; 32] = [0; 32];

#[derive(Default)]
struct Room {
    /// Tokens of the members, in order of joining.
    members: Vec<u64>,
    /// Which member each learnt address belongs to.
    macs: HashMap<Mac, u64>,
    /// What joining takes (`auth::room_key`), `NO_KEY` for nothing.
    key: [u8; 32],
}

/// A left member's index, kept for it a while.
struct Reservation {
    room: String,
    client_id: u64,
    index: u8,
    until: u64,
}

pub struct Relay {
    config: RelayConfig,
    secret: [u8; 32],
    members: HashMap<u64, Member>,
    rooms: HashMap<String, Room>,
    reserved: Vec<Reservation>,
    reassembler: Reassembler<(u64, u16)>,
    events: Vec<String>,
}

impl Relay {
    pub fn new(config: RelayConfig) -> Self {
        let mut secret = [0; 32];
        crate::net::fill_random(&mut secret);
        Self {
            config,
            secret,
            members: HashMap::new(),
            rooms: HashMap::new(),
            reserved: Vec::new(),
            reassembler: Reassembler::new(),
            events: Vec::new(),
        }
    }

    /// Joins, leaves and oddities since the last call, to log.
    pub fn take_events(&mut self) -> Vec<String> {
        std::mem::take(&mut self.events)
    }

    /// The rooms and how many are in each, the fullest first.
    pub fn rooms(&self) -> Vec<RoomInfo> {
        let mut rooms: Vec<RoomInfo> = self
            .rooms
            .iter()
            .map(|(name, room)| RoomInfo {
                name: name.clone(),
                members: room.members.len() as u16,
                password: room.key != NO_KEY,
            })
            .collect();
        rooms.sort_by(|a, b| b.members.cmp(&a.members).then_with(|| a.name.cmp(&b.name)));
        rooms
    }

    /// How many rooms have `filter` in their names, ignoring case, and
    /// those from the `start`th on.
    pub fn list(&self, filter: &str, start: usize) -> (usize, Vec<RoomInfo>) {
        let filter = filter.to_lowercase();
        let mut rooms = self.rooms();
        rooms.retain(|r| r.name.to_lowercase().contains(&filter));
        let total = rooms.len();
        (total, rooms.into_iter().skip(start).take(u8::MAX as usize).collect())
    }

    pub fn member_count(&self) -> usize {
        self.members.len()
    }

    /// Take datagram `bytes` from `from` at `now` (ms).
    pub fn handle(&mut self, now: u64, from: SocketAddr, bytes: &[u8], out: &mut Vec<Outgoing>) {
        let packet = match wire::decode(bytes) {
            Ok(packet) => packet,
            Err(DecodeError::Version(_)) => {
                // Answer only what is longer than the answer.
                let reply = wire::encode(0, &Message::Reject { reason: RejectReason::Version });
                if bytes.len() >= reply.len() {
                    out.push(Outgoing { to: from, bytes: reply });
                }
                return;
            }
            Err(DecodeError::Malformed) => return,
        };
        let token = packet.token;
        match packet.message {
            Message::Discover => {
                let offer = Message::Offer {
                    port: self.config.port,
                    password: self.password().is_some(),
                    name: self.config.name.clone(),
                    rooms: self.rooms(),
                };
                out.push(Outgoing { to: from, bytes: wire::encode(0, &offer) });
            }
            Message::List { start, filter } => {
                let (total, rooms) = self.list(&filter, start as usize);
                let page = Message::Rooms {
                    name: self.config.name.clone(),
                    password: self.password().is_some(),
                    total: total.min(u16::MAX as usize) as u16,
                    start,
                    rooms,
                };
                out.push(Outgoing { to: from, bytes: wire::encode(0, &page) });
            }
            Message::Hello { client_id, room } => {
                let cookie = auth::cookie(&self.secret, from, client_id, now / auth::COOKIE_SLOT_MS);
                let existing = self.rooms.get(&room);
                let challenge = Message::Challenge {
                    cookie,
                    password: self.password().is_some() || existing.is_some_and(|r| r.key != NO_KEY),
                    fresh: self.password().is_none() && existing.is_none(),
                };
                out.push(Outgoing { to: from, bytes: wire::encode(0, &challenge) });
            }
            Message::Join { client_id, cookie, room, proof, key } => {
                self.join(now, from, client_id, &cookie, room, &proof, &key, out);
            }
            Message::Data { seq, fragment, count, payload, .. } => {
                if !self.heard(now, token, from) || !self.allow(now, token) {
                    return;
                }
                if let Some(frame) = self.reassembler.add(now, (token, seq), fragment, count, &payload) {
                    self.forward(token, seq, &frame, out);
                }
            }
            Message::Keepalive { stamp } => {
                if self.heard(now, token, from) {
                    let members = self.room_of(token).map_or(0, |r| r.members.len()) as u16;
                    out.push(Outgoing { to: from, bytes: wire::encode(token, &Message::Ack { stamp, members }) });
                } else {
                    let reply = Message::Reject { reason: RejectReason::Unknown };
                    out.push(Outgoing { to: from, bytes: wire::encode(0, &reply) });
                }
            }
            Message::Leave => {
                if self.heard(now, token, from) {
                    self.remove(now, token, "left");
                }
            }
            // What only a relay sends.
            Message::Offer { .. }
            | Message::Rooms { .. }
            | Message::Challenge { .. }
            | Message::Welcome { .. }
            | Message::Reject { .. }
            | Message::Ack { .. } => {}
        }
    }

    /// Drop the members that went silent and forget old reservations.
    pub fn tick(&mut self, now: u64) {
        let silent: Vec<u64> = self
            .members
            .iter()
            .filter(|(_, m)| now.saturating_sub(m.last_seen) > MEMBER_TIMEOUT_MS)
            .map(|(t, _)| *t)
            .collect();
        for token in silent {
            self.remove(now, token, "timed out");
        }
        self.reserved.retain(|r| r.until > now);
        self.reassembler.expire(now);
    }

    fn password(&self) -> Option<&str> {
        self.config.password.as_deref().filter(|p| !p.is_empty())
    }

    fn room_of(&self, token: u64) -> Option<&Room> {
        self.rooms.get(&self.members.get(&token)?.room)
    }

    #[allow(clippy::too_many_arguments)]
    fn join(
        &mut self,
        now: u64,
        from: SocketAddr,
        client_id: u64,
        cookie: &[u8; 16],
        room: String,
        proof: &[u8; 32],
        key: &[u8; 32],
        out: &mut Vec<Outgoing>,
    ) {
        let reject = |reason| Outgoing { to: from, bytes: wire::encode(0, &Message::Reject { reason }) };
        if !wire::valid_room(&room) {
            out.push(reject(RejectReason::Name));
            return;
        }
        if !auth::cookie_is_good(&self.secret, from, client_id, now, cookie) {
            out.push(reject(RejectReason::Cookie));
            return;
        }
        // What the room wants: the relay's password, else what the room
        // was made with, else, for a room this join makes, the key it
        // brings.
        let wanted = match (self.password(), self.rooms.get(&room)) {
            (Some(password), _) => auth::room_key(Some(password), &room),
            (None, Some(existing)) => existing.key,
            // Told the room was there, the client kept its key; the room
            // has gone since. It starts over, and is told it is fresh.
            (None, None) if *key == NO_KEY && *proof != NO_KEY => {
                out.push(reject(RejectReason::Cookie));
                return;
            }
            (None, None) => *key,
        };
        if wanted == NO_KEY && *proof != NO_KEY {
            out.push(reject(RejectReason::Open));
            return;
        }
        if !auth::equal(&auth::proof(&wanted, &room, client_id, cookie), proof) {
            self.events.push(format!("{} gave a wrong password for room \"{}\"", from, room));
            out.push(reject(RejectReason::Password));
            return;
        }
        // A client that joins again (its WELCOME was lost, or it moved to
        // another room) leaves its old place first.
        let again: Vec<u64> = self.members.iter().filter(|(_, m)| m.client_id == client_id).map(|(t, _)| *t).collect();
        for token in again {
            self.remove(now, token, "rejoined");
        }
        let members_in_room = self.rooms.get(&room).map_or(0, |r| r.members.len());
        let Some(index) = self.free_index(&room, client_id) else {
            out.push(reject(RejectReason::Full));
            return;
        };
        // Instances on the relay's own machine are no strangers.
        let from_there = self.members.values().filter(|m| m.addr.ip() == from.ip()).count();
        let crowded = from_there >= MAX_PER_ADDRESS && !from.ip().is_loopback();
        if members_in_room >= MAX_ROOM || self.members.len() >= MAX_MEMBERS || crowded {
            out.push(reject(RejectReason::Full));
            return;
        }
        let token = loop {
            let token = crate::net::random_u64();
            if token != 0 && !self.members.contains_key(&token) {
                break token;
            }
        };
        self.reserved.retain(|r| !(r.room == room && r.index == index));
        self.members.insert(
            token,
            Member {
                client_id,
                room: room.clone(),
                index,
                addr: from,
                last_seen: now,
                macs: Vec::new(),
                allowance: BURST,
                refilled: now,
                warned: false,
            },
        );
        let entry = self.rooms.entry(room.clone()).or_insert_with(|| Room { key: wanted, ..Default::default() });
        entry.members.push(token);
        let members = entry.members.len() as u16;
        self.events.push(format!("{} joined room \"{}\" as member {} ({} in the room)", from, room, index, members));
        let welcome = Message::Welcome { index, keepalive: KEEPALIVE_S, members };
        out.push(Outgoing { to: from, bytes: wire::encode(token, &welcome) });
    }

    /// The index for `client_id` in `room`: the one kept for it, else the
    /// lowest that is neither taken nor kept for someone else, else the
    /// lowest that is only kept.
    fn free_index(&self, room: &str, client_id: u64) -> Option<u8> {
        let taken: Vec<u8> = self.members.values().filter(|m| m.room == room).map(|m| m.index).collect();
        let kept = |index: u8| self.reserved.iter().any(|r| r.room == room && r.index == index);
        if let Some(r) = self.reserved.iter().find(|r| r.room == room && r.client_id == client_id)
            && !taken.contains(&r.index)
        {
            return Some(r.index);
        }
        let free = |index: &u8| !taken.contains(index);
        (1..=MAX_ROOM as u8).find(|i| free(i) && !kept(*i)).or_else(|| (1..=MAX_ROOM as u8).find(free))
    }

    /// Whether `token` is a member's, noting that it was heard from, at
    /// `from`: a member's NAT may give it a new port at any time.
    fn heard(&mut self, now: u64, token: u64, from: SocketAddr) -> bool {
        let Some(member) = self.members.get_mut(&token) else { return false };
        member.last_seen = now;
        if member.addr != from {
            self.events.push(format!(
                "member {} of room \"{}\" moved from {} to {}",
                member.index, member.room, member.addr, from
            ));
            member.addr = from;
        }
        true
    }

    /// Whether the member may send another frame piece now.
    fn allow(&mut self, now: u64, token: u64) -> bool {
        let Some(member) = self.members.get_mut(&token) else { return false };
        let elapsed = now.saturating_sub(member.refilled) as f64 / 1000.0;
        member.allowance = (member.allowance + elapsed * RATE_PER_S).min(BURST);
        member.refilled = now;
        if member.allowance < 1.0 {
            return false;
        }
        member.allowance -= 1.0;
        true
    }

    /// Learn the source of the frame `sender` sent and pass the frame on.
    fn forward(&mut self, sender: u64, seq: u16, frame: &[u8], out: &mut Vec<Outgoing>) {
        let (Some(source), Some(destination)) = (frame::source(frame), frame::destination(frame)) else { return };
        if frame.len() < frame::HEADER || source.is_group() {
            return;
        }
        let Some(member) = self.members.get(&sender) else { return };
        let (room_name, index) = (member.room.clone(), member.index);
        let Some(room) = self.rooms.get_mut(&room_name) else { return };
        match room.macs.get(&source).copied() {
            Some(owner) if owner == sender => {}
            owner => {
                let member = self.members.get_mut(&sender).unwrap();
                if member.macs.len() >= MAX_MACS {
                    if !member.warned {
                        member.warned = true;
                        self.events
                            .push(format!("member {} of room \"{}\" sends from too many addresses", index, room_name));
                    }
                    return;
                }
                member.macs.push(source);
                if let Some(owner) = owner
                    && let Some(old) = self.members.get_mut(&owner)
                {
                    old.macs.retain(|m| *m != source);
                    let old_index = old.index;
                    self.events.push(format!(
                        "{} moved from member {} to member {} of room \"{}\"",
                        source, old_index, index, room_name
                    ));
                }
                room.macs.insert(source, sender);
            }
        }
        let targets: Vec<u64> = match room.macs.get(&destination) {
            Some(&owner) if !destination.is_group() => vec![owner],
            _ => room.members.clone(),
        };
        let pieces = frag::split(frame);
        for target in targets {
            if target == sender {
                continue;
            }
            let Some(member) = self.members.get(&target) else { continue };
            for (i, piece) in pieces.iter().enumerate() {
                let data = Message::Data {
                    source: index,
                    seq,
                    fragment: i as u8,
                    count: pieces.len() as u8,
                    payload: piece.to_vec(),
                };
                out.push(Outgoing { to: member.addr, bytes: wire::encode(target, &data) });
            }
        }
    }

    fn remove(&mut self, now: u64, token: u64, why: &str) {
        let Some(member) = self.members.remove(&token) else { return };
        if let Some(room) = self.rooms.get_mut(&member.room) {
            room.members.retain(|t| *t != token);
            room.macs.retain(|_, t| *t != token);
            if room.members.is_empty() {
                self.rooms.remove(&member.room);
            }
        }
        self.events.push(format!("member {} of room \"{}\" ({}) {}", member.index, member.room, member.addr, why));
        self.reserved.retain(|r| !(r.room == member.room && r.client_id == member.client_id));
        self.reserved.push(Reservation {
            room: member.room,
            client_id: member.client_id,
            index: member.index,
            until: now + RESERVE_MS,
        });
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use server::{RelayServer, RelayStatus, serve};

#[cfg(not(target_arch = "wasm32"))]
mod server {
    use super::{Outgoing, Relay, RelayConfig, RoomInfo};
    use std::io;
    use std::net::{SocketAddr, UdpSocket};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};

    /// What a running relay has.
    #[derive(Clone, Debug, Default)]
    pub struct RelayStatus {
        pub rooms: Vec<RoomInfo>,
        pub members: usize,
    }

    /// A relay on a UDP socket, in a thread of its own so it keeps
    /// relaying while its emulator is paused. Dropping it stops it.
    pub struct RelayServer {
        local: SocketAddr,
        stop: Arc<AtomicBool>,
        status: Arc<Mutex<RelayStatus>>,
        thread: Option<JoinHandle<()>>,
    }

    impl RelayServer {
        /// Relay on `bind`, handing `log` what happens.
        pub fn start(
            bind: SocketAddr,
            mut config: RelayConfig,
            log: Box<dyn FnMut(String) + Send>,
        ) -> io::Result<Self> {
            let socket = UdpSocket::bind(bind)?;
            socket.set_read_timeout(Some(Duration::from_millis(200)))?;
            let local = socket.local_addr()?;
            if config.port == 0 {
                config.port = local.port();
            }
            let stop = Arc::new(AtomicBool::new(false));
            let status = Arc::new(Mutex::new(RelayStatus::default()));
            let thread = std::thread::Builder::new().name("rust-dos-relay".into()).spawn({
                let (stop, status) = (stop.clone(), status.clone());
                move || run(socket, Relay::new(config), &stop, &status, log)
            })?;
            Ok(Self { local, stop, status, thread: Some(thread) })
        }

        pub fn local_addr(&self) -> SocketAddr {
            self.local
        }

        pub fn status(&self) -> RelayStatus {
            self.status.lock().map(|s| s.clone()).unwrap_or_default()
        }

        /// Keep relaying until the process ends (the relay program).
        pub fn wait(mut self) {
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    /// Relay on `bind` until the process ends, printing what happens: the
    /// relay program, and `rust-dos --relay`.
    pub fn serve(bind: SocketAddr, config: RelayConfig) -> Result<(), String> {
        let stamp = || chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let password = config.password.as_deref().is_some_and(|p| !p.is_empty());
        let server = RelayServer::start(bind, config, Box::new(move |line| println!("{} {}", stamp(), line)))
            .map_err(|e| format!("Can't relay on {}: {}", bind, e))?;
        println!(
            "{} Relaying LAN rooms on UDP {}{}; stop with Ctrl+C",
            stamp(),
            server.local_addr(),
            if password { ", with a password" } else { "" }
        );
        server.wait();
        Ok(())
    }

    impl Drop for RelayServer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn run(
        socket: UdpSocket,
        mut relay: Relay,
        stop: &AtomicBool,
        status: &Mutex<RelayStatus>,
        mut log: Box<dyn FnMut(String) + Send>,
    ) {
        let start = Instant::now();
        let mut buf = vec![0u8; 2048];
        let mut out: Vec<Outgoing> = Vec::new();
        let mut last_tick = 0;
        while !stop.load(Ordering::Relaxed) {
            let now = start.elapsed().as_millis() as u64;
            // An error is a timeout with nothing come, or (on Windows) an
            // earlier datagram's destination port was closed.
            if let Ok((len, from)) = socket.recv_from(&mut buf) {
                relay.handle(now, from, &buf[..len], &mut out);
            }
            for datagram in out.drain(..) {
                let _ = socket.send_to(&datagram.bytes, datagram.to);
            }
            if now >= last_tick + 1000 {
                last_tick = now;
                relay.tick(now);
            }
            let events = relay.take_events();
            // Members came or went: the rooms are told as it happens.
            if (!events.is_empty() || now == last_tick)
                && let Ok(mut status) = status.lock()
            {
                *status = RelayStatus { rooms: relay.rooms(), members: relay.member_count() };
            }
            for event in events {
                log(event);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::frame::ETHERTYPE_IPX;
    use crate::net::tunnel::wire::Packet;

    fn addr(n: u16) -> SocketAddr {
        SocketAddr::from(([192, 0, 2, 1], 4000 + n))
    }

    fn mac(n: u8) -> Mac {
        Mac([2, 0, 0, 0, 0, n])
    }

    fn one(out: &mut Vec<Outgoing>) -> Outgoing {
        assert_eq!(out.len(), 1, "{:?}", out);
        out.pop().unwrap()
    }

    fn message(bytes: &[u8]) -> Packet {
        wire::decode(bytes).unwrap()
    }

    /// Join `room` from `addr(n)` as client `n`; the token, or the reason.
    fn join(
        relay: &mut Relay,
        now: u64,
        n: u16,
        room: &str,
        password: Option<&str>,
    ) -> Result<(u64, u8), RejectReason> {
        join_from(relay, now, addr(n), n as u64, room, password)
    }

    /// Join `room` from `from` as client `id`, as `Client` does.
    fn join_from(
        relay: &mut Relay,
        now: u64,
        from: SocketAddr,
        id: u64,
        room: &str,
        password: Option<&str>,
    ) -> Result<(u64, u8), RejectReason> {
        let mut out = Vec::new();
        let hello = wire::encode(0, &Message::Hello { client_id: id, room: room.into() });
        relay.handle(now, from, &hello, &mut out);
        let Message::Challenge { cookie, fresh, .. } = message(&one(&mut out).bytes).message else { panic!() };
        let key = auth::room_key(password, room);
        let proof = auth::proof(&key, room, id, &cookie);
        let join =
            Message::Join { client_id: id, cookie, room: room.into(), proof, key: if fresh { key } else { NO_KEY } };
        relay.handle(now, from, &wire::encode(0, &join), &mut out);
        let reply = message(&one(&mut out).bytes);
        match reply.message {
            Message::Welcome { index, .. } => Ok((reply.token, index)),
            Message::Reject { reason } => Err(reason),
            other => panic!("{:?}", other),
        }
    }

    fn send(relay: &mut Relay, now: u64, from: SocketAddr, token: u64, seq: u16, frame: &[u8]) -> Vec<Outgoing> {
        let mut out = Vec::new();
        let pieces = frag::split(frame);
        for (i, piece) in pieces.iter().enumerate() {
            let data =
                Message::Data { source: 0, seq, fragment: i as u8, count: pieces.len() as u8, payload: piece.to_vec() };
            relay.handle(now, from, &wire::encode(token, &data), &mut out);
        }
        out
    }

    /// Who got the frame, as (address, token, source index) of each piece.
    fn receivers(out: &[Outgoing]) -> Vec<(SocketAddr, u64, u8)> {
        out.iter()
            .map(|o| {
                let packet = message(&o.bytes);
                let Message::Data { source, .. } = packet.message else { panic!() };
                (o.to, packet.token, source)
            })
            .collect()
    }

    fn ipx_frame(to: Mac, from: Mac) -> Vec<u8> {
        frame::build(to, from, ETHERTYPE_IPX, &[0xFF; 30])
    }

    #[test]
    fn switches_frames_within_a_room() {
        let mut relay = Relay::new(RelayConfig::default());
        let (a, ia) = join(&mut relay, 0, 1, "doom", None).unwrap();
        let (b, ib) = join(&mut relay, 0, 2, "doom", None).unwrap();
        let (c, _) = join(&mut relay, 0, 3, "doom", None).unwrap();
        let (d, _) = join(&mut relay, 0, 4, "duke", None).unwrap();
        assert_eq!((ia, ib), (1, 2));
        // A broadcast reaches everyone else in the room, not the sender and
        // not the other room.
        let out = send(&mut relay, 1, addr(1), a, 1, &ipx_frame(Mac::BROADCAST, mac(1)));
        assert_eq!(receivers(&out), vec![(addr(2), b, 1), (addr(3), c, 1)]);
        // Unknown unicast is flooded; once B has been heard, frames for it
        // go only to B.
        let out = send(&mut relay, 2, addr(1), a, 2, &ipx_frame(mac(2), mac(1)));
        assert_eq!(out.len(), 2);
        send(&mut relay, 3, addr(2), b, 1, &ipx_frame(Mac::BROADCAST, mac(2)));
        let out = send(&mut relay, 4, addr(1), a, 3, &ipx_frame(mac(2), mac(1)));
        assert_eq!(receivers(&out), vec![(addr(2), b, 1)]);
        // Long frames go in pieces and are passed on in pieces.
        let long = frame::build(mac(2), mac(1), 0x0800, &[7; 1500]);
        let out = send(&mut relay, 5, addr(1), a, 4, &long);
        assert_eq!(out.len(), 2);
        // Members of the other room hear nothing of it, and a stranger's
        // token is ignored.
        assert!(send(&mut relay, 6, addr(4), d, 1, &ipx_frame(mac(1), mac(4))).is_empty());
        assert!(send(&mut relay, 6, addr(9), 12345, 1, &ipx_frame(Mac::BROADCAST, mac(9))).is_empty());
        assert_eq!(
            relay.rooms(),
            vec![
                RoomInfo { name: "doom".into(), members: 3, password: false },
                RoomInfo { name: "duke".into(), members: 1, password: false },
            ]
        );
    }

    #[test]
    fn wants_the_password() {
        let config = RelayConfig { password: Some("swordfish".into()), ..Default::default() };
        let mut relay = Relay::new(config);
        assert_eq!(join(&mut relay, 0, 1, "doom", None), Err(RejectReason::Password));
        assert_eq!(join(&mut relay, 0, 1, "doom", Some("sword")), Err(RejectReason::Password));
        assert!(join(&mut relay, 0, 1, "doom", Some("swordfish")).is_ok());
        assert!(relay.take_events().iter().any(|e| e.contains("wrong password")));
        // Every room wants it, fresh or not.
        assert_eq!(join(&mut relay, 0, 2, "duke", None), Err(RejectReason::Password));
        assert!(join(&mut relay, 0, 2, "duke", Some("swordfish")).is_ok());
        assert!(relay.rooms().iter().all(|r| r.password));
    }

    #[test]
    fn rooms_want_the_password_they_were_made_with() {
        let mut relay = Relay::new(RelayConfig::default());
        let (a, _) = join(&mut relay, 0, 1, "doom", Some("pw")).unwrap();
        assert_eq!(join(&mut relay, 0, 2, "doom", None), Err(RejectReason::Password));
        assert_eq!(join(&mut relay, 0, 2, "doom", Some("nope")), Err(RejectReason::Password));
        assert!(join(&mut relay, 0, 2, "doom", Some("pw")).is_ok());
        // A room made without one takes no password, and says so.
        assert!(join(&mut relay, 0, 3, "duke", None).is_ok());
        assert_eq!(join(&mut relay, 0, 4, "duke", Some("pw")), Err(RejectReason::Open));
        assert_eq!(
            relay.rooms(),
            vec![
                RoomInfo { name: "doom".into(), members: 2, password: true },
                RoomInfo { name: "duke".into(), members: 1, password: false },
            ]
        );
        // The password goes with the room's last member.
        let mut out = Vec::new();
        relay.handle(1, addr(1), &wire::encode(a, &Message::Leave), &mut out);
        let b = relay.members.iter().find(|(_, m)| m.client_id == 2).map(|(t, _)| *t).unwrap();
        relay.handle(1, addr(2), &wire::encode(b, &Message::Leave), &mut out);
        assert!(join(&mut relay, 2, 5, "doom", None).is_ok());
    }

    #[test]
    fn a_room_gone_since_the_challenge_is_joined_afresh() {
        let mut relay = Relay::new(RelayConfig::default());
        let (a, _) = join(&mut relay, 0, 1, "doom", Some("pw")).unwrap();
        let mut out = Vec::new();
        relay.handle(0, addr(2), &wire::encode(0, &Message::Hello { client_id: 2, room: "doom".into() }), &mut out);
        let Message::Challenge { cookie, password, fresh } = message(&one(&mut out).bytes).message else { panic!() };
        assert!(password && !fresh);
        relay.handle(0, addr(1), &wire::encode(a, &Message::Leave), &mut out);
        // Its JOIN brings no key for the room, which would be made open.
        let key = auth::room_key(Some("pw"), "doom");
        let proof = auth::proof(&key, "doom", 2, &cookie);
        let join = Message::Join { client_id: 2, cookie, room: "doom".into(), proof, key: NO_KEY };
        relay.handle(0, addr(2), &wire::encode(0, &join), &mut out);
        assert_eq!(message(&one(&mut out).bytes).message, Message::Reject { reason: RejectReason::Cookie });
        assert!(join_from(&mut relay, 0, addr(2), 2, "doom", Some("pw")).is_ok());
        assert!(relay.rooms()[0].password);
    }

    #[test]
    fn refuses_bad_names_and_crowds() {
        let mut relay = Relay::new(RelayConfig::default());
        assert_eq!(join(&mut relay, 0, 1, "", None), Err(RejectReason::Name));
        assert_eq!(join(&mut relay, 0, 1, "  ", None), Err(RejectReason::Name));
        assert_eq!(join(&mut relay, 0, 1, "a\nb", None), Err(RejectReason::Name));
        // One address fills only so much of the relay.
        for n in 0..MAX_PER_ADDRESS as u16 {
            assert!(join(&mut relay, 0, 100 + n, &format!("room {}", n % 3), None).is_ok());
        }
        assert_eq!(join(&mut relay, 0, 999, "room 0", None), Err(RejectReason::Full));
        let elsewhere = SocketAddr::from(([198, 51, 100, 1], 4000));
        assert!(join_from(&mut relay, 0, elsewhere, 999, "room 0", None).is_ok());
    }

    #[test]
    fn lists_rooms_a_page_at_a_time() {
        let mut relay = Relay::new(RelayConfig { name: "den".into(), ..Default::default() });
        let far = |i: u64| SocketAddr::from(([198, 51, 100, i as u8], 5000));
        // Names as long as they go, so that the rooms take pages.
        let name = |i: u64| format!("Room {:02} {}", i, "-".repeat(wire::MAX_NAME - 8));
        for i in 0..50 {
            join_from(&mut relay, 0, far(i), 1000 + i, &name(i), None).unwrap();
        }
        join_from(&mut relay, 0, far(7), 2000, &name(7), Some("pw")).unwrap_err();
        join_from(&mut relay, 0, far(60), 2000, &name(7), None).unwrap();
        let page = |relay: &mut Relay, start: u16, filter: &str| {
            let mut out = Vec::new();
            relay.handle(0, addr(9), &wire::encode(0, &Message::List { start, filter: filter.into() }), &mut out);
            let reply = one(&mut out);
            assert!(reply.bytes.len() <= wire::LIST_SIZE);
            let Message::Rooms { name, password, total, start: from, rooms } = message(&reply.bytes).message else {
                panic!()
            };
            assert_eq!((name.as_str(), password, from), ("den", false, start));
            (total, rooms)
        };
        let (total, first) = page(&mut relay, 0, "");
        assert_eq!(total, 50);
        assert!(first.len() > 20 && first.len() < 50, "{}", first.len());
        assert_eq!(first[0], RoomInfo { name: name(7), members: 2, password: false });
        let mut all = first.clone();
        while all.len() < total as usize {
            let (_, more) = page(&mut relay, all.len() as u16, "");
            assert!(!more.is_empty());
            all.extend(more);
        }
        let mut names: Vec<&str> = all.iter().map(|r| r.name.as_str()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), 50);
        // A filter, in any case; past the end, nothing.
        let (total, rooms) = page(&mut relay, 0, "room 1");
        assert_eq!((total, rooms.len()), (10, 10));
        assert!(rooms.iter().all(|r| r.name.starts_with("Room 1")));
        assert_eq!(page(&mut relay, 60, ""), (50, vec![]));
        // A LIST shorter than its answer could be gets none.
        let mut out = Vec::new();
        let list = wire::encode(0, &Message::List { start: 0, filter: String::new() });
        relay.handle(0, addr(9), &list[..100], &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn wants_a_fresh_cookie_from_the_same_address() {
        let mut relay = Relay::new(RelayConfig::default());
        let mut out = Vec::new();
        relay.handle(0, addr(1), &wire::encode(0, &Message::Hello { client_id: 1, room: "x".into() }), &mut out);
        let Message::Challenge { cookie, password, fresh } = message(&one(&mut out).bytes).message else { panic!() };
        assert!(!password && fresh);
        let join = Message::Join { client_id: 1, cookie, room: "x".into(), proof: NO_KEY, key: NO_KEY };
        let join = wire::encode(0, &join);
        let rejected = |relay: &mut Relay, now, from| {
            let mut out = Vec::new();
            relay.handle(now, from, &join, &mut out);
            message(&one(&mut out).bytes).message == Message::Reject { reason: RejectReason::Cookie }
        };
        assert!(rejected(&mut relay, 0, addr(2)));
        assert!(rejected(&mut relay, 3 * auth::COOKIE_SLOT_MS, addr(1)));
        assert!(!rejected(&mut relay, auth::COOKIE_SLOT_MS, addr(1)));
    }

    #[test]
    fn follows_members_whose_port_changes() {
        let mut relay = Relay::new(RelayConfig::default());
        let (a, _) = join(&mut relay, 0, 1, "doom", None).unwrap();
        let (b, _) = join(&mut relay, 0, 2, "doom", None).unwrap();
        // B's NAT gives it a new port; its keepalive comes from there.
        let mut out = Vec::new();
        relay.handle(10, addr(20), &wire::encode(b, &Message::Keepalive { stamp: 99 }), &mut out);
        let ack = one(&mut out);
        assert_eq!(ack.to, addr(20));
        assert_eq!(message(&ack.bytes), Packet { token: b, message: Message::Ack { stamp: 99, members: 2 } });
        let out = send(&mut relay, 11, addr(1), a, 1, &ipx_frame(Mac::BROADCAST, mac(1)));
        assert_eq!(receivers(&out), vec![(addr(20), b, 1)]);
        // A keepalive with a token the relay doesn't know is told so.
        let mut out = Vec::new();
        relay.handle(12, addr(5), &wire::encode(777, &Message::Keepalive { stamp: 1 }), &mut out);
        assert_eq!(message(&one(&mut out).bytes).message, Message::Reject { reason: RejectReason::Unknown });
    }

    #[test]
    fn drops_silent_members_and_keeps_their_place() {
        let mut relay = Relay::new(RelayConfig::default());
        let (a, _) = join(&mut relay, 0, 1, "doom", None).unwrap();
        let (_, ib) = join(&mut relay, 0, 2, "doom", None).unwrap();
        assert_eq!(ib, 2);
        // A keeps talking, B goes quiet and is dropped.
        let mut out = Vec::new();
        relay.handle(MEMBER_TIMEOUT_MS, addr(1), &wire::encode(a, &Message::Keepalive { stamp: 0 }), &mut out);
        relay.tick(MEMBER_TIMEOUT_MS + 1);
        assert_eq!(relay.member_count(), 1);
        assert!(relay.take_events().iter().any(|e| e.contains("timed out")));
        // A newcomer doesn't get B's index while it is kept, and B gets it
        // back.
        let now = MEMBER_TIMEOUT_MS + 2;
        assert_eq!(join(&mut relay, now, 3, "doom", None).map(|j| j.1), Ok(3));
        assert_eq!(join(&mut relay, now, 2, "doom", None).map(|j| j.1), Ok(2));
        // Leaving frees the room.
        relay.handle(now, addr(1), &wire::encode(a, &Message::Leave), &mut out);
        assert_eq!(relay.member_count(), 2);
    }

    #[test]
    fn a_client_joining_again_keeps_one_place() {
        let mut relay = Relay::new(RelayConfig::default());
        let (first, i1) = join(&mut relay, 0, 1, "doom", None).unwrap();
        let (second, i2) = join(&mut relay, 1, 1, "doom", None).unwrap();
        assert_ne!(first, second);
        assert_eq!((i1, i2, relay.member_count()), (1, 1, 1));
        // Moving to another room.
        join(&mut relay, 2, 1, "duke", None).unwrap();
        assert_eq!(relay.rooms(), vec![RoomInfo { name: "duke".into(), members: 1, password: false }]);
    }

    #[test]
    fn limits_addresses_and_rate() {
        let mut relay = Relay::new(RelayConfig::default());
        let (a, _) = join(&mut relay, 0, 1, "doom", None).unwrap();
        join(&mut relay, 0, 2, "doom", None).unwrap();
        for n in 1..=MAX_MACS as u8 {
            assert_eq!(send(&mut relay, 0, addr(1), a, n as u16, &ipx_frame(Mac::BROADCAST, mac(n))).len(), 1);
        }
        assert!(send(&mut relay, 0, addr(1), a, 9, &ipx_frame(Mac::BROADCAST, mac(9))).is_empty());
        // Group source addresses are nonsense.
        assert!(send(&mut relay, 0, addr(1), a, 10, &ipx_frame(Mac::BROADCAST, Mac::BROADCAST)).is_empty());
        // A flood runs out of allowance, which comes back with time.
        let frame = ipx_frame(Mac::BROADCAST, mac(1));
        let passed =
            (0..BURST as u32 + 100).filter(|&i| !send(&mut relay, 1, addr(1), a, i as u16, &frame).is_empty()).count();
        assert!(passed <= BURST as usize, "{}", passed);
        assert_eq!(send(&mut relay, 1001, addr(1), a, 1, &frame).len(), 1);
    }

    #[test]
    fn answers_strangers_only_briefly() {
        let mut relay = Relay::new(RelayConfig { name: "den".into(), port: 21213, password: None });
        join(&mut relay, 0, 1, "doom", None).unwrap();
        let mut out = Vec::new();
        let discover = wire::encode(0, &Message::Discover);
        relay.handle(0, addr(7), &discover, &mut out);
        let offer = one(&mut out);
        assert!(offer.bytes.len() <= discover.len());
        let Message::Offer { port, password, name, rooms } = message(&offer.bytes).message else { panic!() };
        assert_eq!((port, password, name.as_str()), (21213, false, "den"));
        assert_eq!(rooms, vec![RoomInfo { name: "doom".into(), members: 1, password: false }]);
        // Garbage gets nothing; another version gets a short refusal.
        relay.handle(0, addr(7), b"GET / HTTP/1.0\r\n\r\n", &mut out);
        assert!(out.is_empty());
        let mut hello = wire::encode(0, &Message::Hello { client_id: 1, room: "doom".into() });
        hello[4] = 2;
        relay.handle(0, addr(7), &hello, &mut out);
        let reply = one(&mut out);
        assert!(reply.bytes.len() <= hello.len());
        assert_eq!(message(&reply.bytes).message, Message::Reject { reason: RejectReason::Version });
    }

    #[test]
    fn relays_over_a_real_socket() {
        use std::net::UdpSocket;
        use std::time::Duration;
        let server =
            RelayServer::start("127.0.0.1:0".parse().unwrap(), RelayConfig::default(), Box::new(|_| {})).unwrap();
        let relay = server.local_addr();
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        socket.send_to(&wire::encode(0, &Message::Hello { client_id: 1, room: "doom".into() }), relay).unwrap();
        let mut buf = [0; 2048];
        let (len, _) = socket.recv_from(&mut buf).unwrap();
        let Message::Challenge { cookie, .. } = message(&buf[..len]).message else { panic!() };
        let join = Message::Join { client_id: 1, cookie, room: "doom".into(), proof: NO_KEY, key: NO_KEY };
        socket.send_to(&wire::encode(0, &join), relay).unwrap();
        let (len, _) = socket.recv_from(&mut buf).unwrap();
        assert!(matches!(message(&buf[..len]).message, Message::Welcome { index: 1, .. }));
        drop(server);
    }
}
