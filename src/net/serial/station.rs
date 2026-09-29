//! The serial ports' side of the network thread: the link to the other
//! player in the LAN room (the first two members pair up; `link`), a
//! modem's calls to that player through it and to hosts over TCP (`tcp`),
//! and the calls a modem takes on a TCP port. What the ports send comes as
//! `LinkCmd`s; what happens goes back to them as `LinkEvent`s through the
//! `SerialQueue`.

use super::link::{self, Link, Record};
use super::tcp::{self, TcpEvent, Telnet};
use super::SerialQueue;
use crate::net::hub::Command;
use crate::net::tunnel::wire::Roster;
use crate::serial::{LinkCmd, LinkEvent};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc::{UnboundedSender, WeakUnboundedSender};
use tokio::task::JoinHandle;

/// Where a port's call is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Call {
    #[default]
    None,
    /// To or from the other player in the room.
    RoomDialing,
    RoomRinging,
    Room,
    /// Over TCP: dialed, coming in, through.
    TcpDialing(u64),
    TcpRinging(u64),
    Tcp(u64),
}

impl Call {
    fn tcp(self) -> Option<u64> {
        match self {
            Call::TcpDialing(id) | Call::TcpRinging(id) | Call::Tcp(id) => Some(id),
            _ => None,
        }
    }

    fn room(self) -> bool {
        matches!(self, Call::RoomDialing | Call::RoomRinging | Call::Room)
    }
}

struct Connection {
    tx: UnboundedSender<Vec<u8>>,
    telnet: Option<Telnet>,
    /// What came before the call was answered.
    early: Vec<u8>,
}

/// The ports the settings give the network.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Setup {
    /// The port that goes to the other player in the room.
    pub linked: Option<usize>,
    /// The modem that takes calls on TCP port `listen`.
    pub modem: Option<usize>,
    pub listen: Option<u16>,
    pub telnet: bool,
}

/// The link's state, for the status line and the debugger.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SerialStatus {
    /// The port, the member at the other end, whether the link is up.
    pub port: Option<usize>,
    pub peer: Option<u8>,
    pub up: bool,
    pub rtt_ms: Option<u32>,
    pub retransmits: u64,
    /// Whether the relay carries serial links (None: not in a room).
    pub relay_serial: Option<bool>,
    /// Calls on TCP: taken on this port.
    pub listening: Option<u16>,
}

pub struct Station {
    queue: Option<Arc<SerialQueue>>,
    setup: Setup,
    link: Option<Link>,
    calls: [Call; 4],
    tcp: HashMap<u64, Connection>,
    next_id: u64,
    listener: Option<(u16, JoinHandle<()>)>,
    events: WeakUnboundedSender<Command>,
    /// Datagrams for the other player: (member, payload).
    pub out: Vec<(u8, Vec<u8>)>,
    pub notices: Vec<String>,
    /// Told the user their place in the room keeps them off the link, or
    /// the relay can't carry it.
    told_third: bool,
    told_relay: bool,
    relay_serial: Option<bool>,
}

/// Listened calls' numbers start here, dialed ones at 1.
const INCOMING_IDS: u64 = 1 << 62;

impl Station {
    pub fn new(events: WeakUnboundedSender<Command>) -> Self {
        Self {
            queue: None,
            setup: Setup::default(),
            link: None,
            calls: [Call::None; 4],
            tcp: HashMap::new(),
            next_id: 0,
            listener: None,
            events,
            out: Vec::new(),
            notices: Vec::new(),
            told_third: false,
            told_relay: false,
            relay_serial: None,
        }
    }

    fn tell(&self, port: usize, event: LinkEvent) {
        if let Some(queue) = &self.queue {
            queue.push(port, event);
        }
    }

    pub fn status(&self) -> SerialStatus {
        SerialStatus {
            port: self.setup.linked,
            peer: self.link.as_ref().map(|l| l.peer),
            up: self.link.as_ref().is_some_and(|l| l.is_up()),
            rtt_ms: self.link.as_ref().and_then(|l| l.rtt_ms()),
            retransmits: self.link.as_ref().map_or(0, |l| l.retransmits),
            relay_serial: self.relay_serial,
            listening: self.listener.as_ref().map(|(port, _)| *port),
        }
    }

    /// The emulator's ports: where the events go, which port links, which
    /// modem takes TCP calls on which port.
    pub fn setup(&mut self, queue: Arc<SerialQueue>, setup: Setup, now: u64) {
        self.queue = Some(queue);
        if setup.linked != self.setup.linked {
            // Another port links: the link starts over for it.
            if let (Some(old), Some(link)) = (self.setup.linked, &self.link)
                && link.is_up()
            {
                self.tell(old, LinkEvent::Peer(false));
            }
            let peer = self.link.take().map(|l| l.peer);
            self.setup.linked = setup.linked;
            if setup.linked.is_some()
                && let Some(peer) = peer
            {
                self.link = Some(Link::new(peer, now));
            }
        }
        if setup.listen != self.setup.listen || self.listener.is_none() && setup.listen.is_some() {
            if let Some((_, task)) = self.listener.take() {
                task.abort();
            }
            if let Some(port) = setup.listen {
                let events = self.events.clone();
                let notices = self.events.clone();
                let task = tokio::spawn(async move {
                    if let Err(e) = tcp::listen(port, INCOMING_IDS, events).await
                        && let Some(tx) = notices.upgrade()
                    {
                        let _ = tx.send(Command::Serial(super::SerialCommand::ListenFailed(format!(
                            "The modem can't take calls on TCP port {}: {}",
                            port, e
                        ))));
                    }
                });
                self.listener = Some((port, task));
            }
        }
        self.setup = setup;
    }

    pub fn listen_failed(&mut self, notice: String) {
        self.listener = None;
        self.notices.push(notice);
    }

    /// Who is in the room now (None: not in one), which member this is,
    /// and whether the relay carries serial links: the first two members
    /// pair up.
    pub fn roster(&mut self, me: Option<u8>, roster: Option<&Roster>, relay_serial: bool, now: u64) {
        let pair: Vec<u8> = roster.map(|r| r.members.iter().take(2).map(|m| m.index).collect()).unwrap_or_default();
        self.relay_serial = roster.map(|_| relay_serial);
        let peer = match me {
            Some(me) if pair.len() == 2 && pair.contains(&me) => pair.iter().copied().find(|&i| i != me),
            _ => None,
        };
        if let (Some(_), Some(roster), Some(me)) = (self.setup.linked, roster, me) {
            if roster.total > 2 && !pair.contains(&me) && !self.told_third {
                self.told_third = true;
                self.notices.push("Serial link: only the first two players in a room are connected".into());
            }
            if peer.is_some() && !relay_serial && !self.told_relay {
                self.told_relay = true;
                self.notices.push("This relay can't carry serial links; update it".into());
            }
        }
        if roster.is_none() {
            self.told_third = false;
            self.told_relay = false;
        }
        let peer = peer.filter(|_| relay_serial && self.setup.linked.is_some());
        if self.link.as_ref().map(|l| l.peer) == peer {
            return;
        }
        self.drop_link();
        if let Some(peer) = peer {
            self.link = Some(Link::new(peer, now));
            self.poll(now);
        }
    }

    /// The link goes: the port hears the other player went, and its call
    /// ends.
    fn drop_link(&mut self) {
        let Some(link) = self.link.take() else { return };
        if let Some(port) = self.setup.linked {
            if self.calls[port].room() {
                self.calls[port] = Call::None;
                self.tell(port, LinkEvent::NoCarrier);
            }
            if link.is_up() {
                self.tell(port, LinkEvent::Peer(false));
            }
        }
    }

    /// A command of port `port`'s cable or modem.
    pub fn command(&mut self, port: usize, command: LinkCmd, now: u64) {
        if port >= 4 {
            return;
        }
        let linked = self.setup.linked == Some(port);
        let call = self.calls[port];
        match command {
            LinkCmd::Bytes(bytes) => {
                if let Call::Tcp(id) = call {
                    if let Some(c) = self.tcp.get(&id) {
                        let bytes = if c.telnet.is_some() { Telnet::outgoing(&bytes) } else { bytes };
                        let _ = c.tx.send(bytes);
                    }
                } else if linked && let Some(link) = &mut self.link {
                    link.send(&Record::Bytes(bytes));
                }
            }
            LinkCmd::Lines { dtr, rts } => {
                if linked && let Some(link) = &mut self.link {
                    link.send(&Record::Lines { dtr, rts });
                }
            }
            LinkCmd::Dial(None) => match &mut self.link {
                Some(link) if linked && link.is_up() && call == Call::None => {
                    link.send(&Record::Call);
                    self.calls[port] = Call::RoomDialing;
                }
                _ => self.tell(port, LinkEvent::NoCarrier),
            },
            LinkCmd::Dial(Some(address)) => {
                if call != Call::None {
                    return self.tell(port, LinkEvent::NoCarrier);
                }
                self.next_id += 1;
                let id = self.next_id;
                let tx = tcp::dial(id, address, self.events.clone());
                let telnet = self.setup.telnet.then(Telnet::default);
                self.tcp.insert(id, Connection { tx, telnet, early: Vec::new() });
                self.calls[port] = Call::TcpDialing(id);
            }
            LinkCmd::Answer => match call {
                Call::RoomRinging => {
                    if let Some(link) = &mut self.link {
                        link.send(&Record::Answer);
                    }
                    self.calls[port] = Call::Room;
                    self.tell(port, LinkEvent::Connected);
                }
                Call::TcpRinging(id) => {
                    self.calls[port] = Call::Tcp(id);
                    self.tell(port, LinkEvent::Connected);
                    let early = self.tcp.get_mut(&id).map(|c| std::mem::take(&mut c.early)).unwrap_or_default();
                    if !early.is_empty() {
                        self.tell(port, LinkEvent::Bytes(early));
                    }
                }
                _ => {}
            },
            LinkCmd::Hangup => {
                if call.room()
                    && let Some(link) = &mut self.link
                {
                    link.send(&Record::Hangup);
                }
                if let Some(id) = call.tcp() {
                    self.tcp.remove(&id);
                }
                self.calls[port] = Call::None;
            }
        }
        self.poll(now);
    }

    /// A datagram of the link from member `from`.
    pub fn datagram(&mut self, from: u8, payload: &[u8], now: u64) {
        let Some(link) = &mut self.link else { return };
        if link.peer != from {
            return;
        }
        let mut out = Vec::new();
        let events = link.handle(now, payload, &mut out);
        let peer = link.peer;
        self.out.extend(out.into_iter().map(|d| (peer, d)));
        self.link_events(events);
    }

    fn link_events(&mut self, events: Vec<link::LinkEvent>) {
        let Some(port) = self.setup.linked else { return };
        for event in events {
            match event {
                link::LinkEvent::Up => {
                    self.notices.push(format!("COM{} is linked to the other player", port + 1));
                    self.tell(port, LinkEvent::Peer(true));
                }
                link::LinkEvent::Down => {
                    if self.calls[port].room() {
                        self.calls[port] = Call::None;
                        self.tell(port, LinkEvent::NoCarrier);
                    }
                    self.tell(port, LinkEvent::Peer(false));
                }
                link::LinkEvent::Record(record) => self.record(port, record),
            }
        }
    }

    fn record(&mut self, port: usize, record: Record) {
        let call = self.calls[port];
        match record {
            Record::Bytes(bytes) => {
                if call.tcp().is_none() {
                    self.tell(port, LinkEvent::Bytes(bytes));
                }
            }
            Record::Lines { dtr, rts } => self.tell(port, LinkEvent::Lines { dtr, rts }),
            Record::Call => {
                if call == Call::None {
                    self.calls[port] = Call::RoomRinging;
                    self.tell(port, LinkEvent::Ring);
                } else if let Some(link) = &mut self.link {
                    // Busy.
                    link.send(&Record::Hangup);
                }
            }
            Record::Answer => {
                if call == Call::RoomDialing {
                    self.calls[port] = Call::Room;
                    self.tell(port, LinkEvent::Connected);
                }
            }
            Record::Hangup => {
                if call.room() {
                    self.calls[port] = Call::None;
                    self.tell(port, LinkEvent::NoCarrier);
                }
            }
        }
    }

    /// What happened to a TCP call.
    pub fn tcp_event(&mut self, event: TcpEvent) {
        let port_of = |calls: &[Call; 4], id: u64| calls.iter().position(|c| c.tcp() == Some(id));
        match event {
            TcpEvent::Connected { id } => {
                if let Some(port) = port_of(&self.calls, id) {
                    self.calls[port] = Call::Tcp(id);
                    self.tell(port, LinkEvent::Connected);
                } else {
                    self.tcp.remove(&id);
                }
            }
            TcpEvent::Failed { id, reason } => {
                self.tcp.remove(&id);
                if let Some(port) = port_of(&self.calls, id) {
                    self.calls[port] = Call::None;
                    self.notices.push(format!("COM{}: the call failed: {}", port + 1, reason));
                    self.tell(port, LinkEvent::NoCarrier);
                }
            }
            TcpEvent::Closed { id } => {
                self.tcp.remove(&id);
                if let Some(port) = port_of(&self.calls, id) {
                    self.calls[port] = Call::None;
                    self.tell(port, LinkEvent::NoCarrier);
                }
            }
            TcpEvent::Data { id, data } => {
                let Some(connection) = self.tcp.get_mut(&id) else { return };
                let data = match &mut connection.telnet {
                    Some(telnet) => {
                        let (data, replies) = telnet.incoming(&data);
                        if !replies.is_empty() {
                            let _ = connection.tx.send(replies);
                        }
                        data
                    }
                    None => data,
                };
                match port_of(&self.calls, id).map(|p| (p, self.calls[p])) {
                    Some((port, Call::Tcp(_))) if !data.is_empty() => self.tell(port, LinkEvent::Bytes(data)),
                    Some((_, Call::TcpRinging(_))) if connection.early.len() < 64 * 1024 => {
                        connection.early.extend_from_slice(&data);
                    }
                    _ => {}
                }
            }
            TcpEvent::Incoming { id, from, tx } => {
                let free = self.setup.modem.filter(|&port| self.calls[port] == Call::None);
                let Some(port) = free else {
                    // Busy, or no modem: the caller is hung up on.
                    return;
                };
                let telnet = self.setup.telnet.then(Telnet::default);
                self.tcp.insert(id, Connection { tx, telnet, early: Vec::new() });
                self.calls[port] = Call::TcpRinging(id);
                self.notices.push(format!("COM{}: a call from {}", port + 1, from));
                self.tell(port, LinkEvent::Ring);
            }
        }
    }

    /// Let the link send what it has to, and see whether the other end is
    /// still there.
    pub fn poll(&mut self, now: u64) {
        let Some(link) = &mut self.link else { return };
        let mut out = Vec::new();
        let events = link.poll(now, &mut out);
        let peer = link.peer;
        self.out.extend(out.into_iter().map(|d| (peer, d)));
        self.link_events(events);
    }

    /// When `poll` has something to do next (ms).
    pub fn next_due(&self) -> Option<u64> {
        self.link.as_ref().map(|l| l.next_due())
    }
}
