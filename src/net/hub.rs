//! The network thread: it owns the switch between the emulated network
//! devices and the LAN tunnel, the tunnel's socket, and the relay when this
//! instance hosts one. The emulator hands it frames and commands through a
//! channel, and takes the frames for its devices from their `PortQueue`s;
//! what the user should know comes back as notices, and the state of the
//! LAN as a `HubStatus`.

use super::frame::Mac;
use super::nat::{self, Router};
use super::port::PortQueue;
use super::switch::{Port, Switch};
use super::tunnel::client::{Client, ClientConfig, ClientEvent};
use super::tunnel::discover;
use super::tunnel::relay::{RelayConfig, RelayServer};
use super::serial::SerialCommand;
use super::serial::station::Station;
use super::tunnel::wire::{RejectReason, RoomInfo, Roster};
use std::collections::HashMap;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, WeakUnboundedSender, unbounded_channel};

/// How often the tunnel's retries and keepalives are looked at.
const TICK: Duration = Duration::from_millis(200);

/// A room to join: at `relay` (`host[:port]`), or at the first relay
/// that answers on the LAN.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JoinRequest {
    pub relay: Option<String>,
    pub room: String,
    pub password: Option<String>,
    /// The name the player goes by in the room, empty for none.
    pub player: String,
}

pub enum Command {
    /// A device with address `mac` takes its frames from `queue`.
    Attach {
        port: Port,
        mac: Mac,
        queue: Arc<PortQueue>,
    },
    Detach(Port),
    /// A frame a device sent.
    Frame(Port, Vec<u8>),
    Join(JoinRequest),
    Leave,
    /// End the room for everyone in it, as its host, and leave it.
    Disband,
    /// Relay rooms on `port` (0 for any), and join the room of `join`
    /// (whose relay is this one) there; with `share`, at the relay of
    /// another instance on this machine if that has the port.
    Host {
        port: u16,
        join: JoinRequest,
        share: bool,
    },
    StopHost,
    /// Where the relay of join number `generation` is.
    Resolved {
        generation: u64,
        relay: Result<SocketAddr, String>,
        request: JoinRequest,
    },
    /// What happened to the router's connections on the host.
    Nat(nat::Event),
    /// The serial ports' links.
    Serial(super::serial::SerialCommand),
}

/// Where this instance is with the LAN.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum LanState {
    #[default]
    Off,
    /// Looking for the relay: on the LAN, or its address.
    Looking,
    Joining {
        relay: SocketAddr,
    },
    Joined {
        relay: SocketAddr,
        index: u8,
        members: u16,
        rtt_ms: Option<u32>,
    },
    /// The relay stopped answering; joining it again.
    Rejoining {
        relay: SocketAddr,
    },
    /// No relay, or it turned us away.
    Failed(String),
}

#[derive(Clone, Debug, Default)]
pub struct HubStatus {
    pub lan: LanState,
    pub room: String,
    /// Who is in the room, once joined, as the relay last said.
    pub roster: Option<Roster>,
    /// The relay this instance hosts, and its rooms.
    pub hosting: Option<SocketAddr>,
    pub hosted_rooms: Vec<RoomInfo>,
    /// Frames to and from the LAN.
    pub frames_out: u64,
    pub frames_in: u64,
    /// The network card's router: its TCP connections and UDP flows.
    pub nat: Option<(usize, usize)>,
    /// The serial link to the other player.
    pub serial: super::serial::station::SerialStatus,
}

#[derive(Default)]
struct Shared {
    status: Mutex<HubStatus>,
    notices: Mutex<Vec<String>>,
}

impl Shared {
    fn notice(&self, text: String) {
        if let Ok(mut notices) = self.notices.lock() {
            notices.push(text);
        }
    }

    fn update(&self, f: impl FnOnce(&mut HubStatus)) {
        if let Ok(mut status) = self.status.lock() {
            f(&mut status);
        }
    }
}

/// The network thread, which stops when this is dropped.
pub struct Hub {
    tx: UnboundedSender<Command>,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Hub {
    pub fn start() -> io::Result<Hub> {
        let (tx, rx) = unbounded_channel();
        let shared = Arc::new(Shared::default());
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
        let state = State::new(shared.clone(), tx.downgrade());
        let thread =
            std::thread::Builder::new().name("rust-dos-net".into()).spawn(move || runtime.block_on(state.run(rx)))?;
        Ok(Hub { tx, shared, thread: Some(thread) })
    }

    pub fn send(&self, command: Command) {
        // A join starts looking at once, so whoever waits for it doesn't
        // take the state before it for its outcome.
        if matches!(command, Command::Join(_) | Command::Host { .. }) {
            self.shared.update(|s| s.lan = LanState::Looking);
        }
        let _ = self.tx.send(command);
    }

    pub fn status(&self) -> HubStatus {
        self.shared.status.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// What happened since the last call, for the screen and the log.
    pub fn take_notices(&self) -> Vec<String> {
        self.shared.notices.lock().map(|mut n| std::mem::take(&mut *n)).unwrap_or_default()
    }
}

impl Drop for Hub {
    fn drop(&mut self) {
        // The thread ends when the channel closes.
        let (tx, _) = unbounded_channel();
        drop(std::mem::replace(&mut self.tx, tx));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// This machine's name, from the environment or, on Linux, its hostname
/// file.
fn machine_name() -> Option<String> {
    ["HOSTNAME", "COMPUTERNAME"]
        .iter()
        .find_map(|v| std::env::var(v).ok())
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
}

/// Where the relay of `request` is (`discover::find`).
async fn resolve(request: Option<String>) -> Result<SocketAddr, String> {
    tokio::task::spawn_blocking(move || discover::find(request.as_deref())).await.map_err(|e| e.to_string())?
}

struct Uplink {
    socket: Arc<UdpSocket>,
    client: Client,
}

struct State {
    shared: Arc<Shared>,
    /// For the answers of lookups; weak, so the channel closes, and the
    /// thread ends, when the `Hub` goes.
    tx: WeakUnboundedSender<Command>,
    switch: Switch,
    queues: HashMap<Port, Arc<PortQueue>>,
    uplink: Option<Uplink>,
    relay: Option<RelayServer>,
    client_id: u64,
    start: Instant,
    /// Counts joins, so the answer to an old one is ignored.
    generation: u64,
    /// The router to the internet of the network card's guest, while
    /// there is a card, and its sockets on the host.
    router: Option<Router>,
    nat: nat::host::NatHost,
    /// The serial ports' links and calls.
    serial: Station,
}

impl State {
    fn new(shared: Arc<Shared>, tx: WeakUnboundedSender<Command>) -> Self {
        Self {
            shared,
            switch: Switch::default(),
            queues: HashMap::new(),
            uplink: None,
            relay: None,
            client_id: super::random_u64(),
            start: Instant::now(),
            generation: 0,
            router: None,
            nat: nat::host::NatHost::new(tx.clone()),
            serial: Station::new(tx.clone()),
            tx,
        }
    }

    fn now(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    async fn run(mut self, mut rx: UnboundedReceiver<Command>) {
        let mut buf = vec![0u8; 2048];
        let mut ticker = tokio::time::interval(TICK);
        loop {
            let socket = self.uplink.as_ref().map(|u| u.socket.clone());
            // When the router's TCP has something to do next.
            let now = self.now();
            let router_due = self
                .router
                .as_mut()
                .and_then(|r| r.poll_delay(now))
                .map(|ms| Duration::from_millis(ms.min(TICK.as_millis() as u64)));
            // When the serial link has something to do next.
            let serial_due = self.serial.next_due().map(|due| Duration::from_millis(due.saturating_sub(now)));
            tokio::select! {
                _ = tokio::time::sleep(serial_due.unwrap_or(TICK)), if serial_due.is_some() => {
                    let now = self.now();
                    self.serial.poll(now);
                    self.serial_flush();
                }
                _ = tokio::time::sleep(router_due.unwrap_or(TICK)), if router_due.is_some() => {
                    let now = self.now();
                    if let Some(router) = &mut self.router {
                        router.poll(now);
                    }
                    self.router_actions();
                }
                command = rx.recv() => match command {
                    Some(command) => self.command(command),
                    None => break,
                },
                received = async {
                    match &socket {
                        Some(socket) => socket.recv_from(&mut buf).await,
                        None => std::future::pending().await,
                    }
                } => {
                    // An error is (on Windows) an earlier datagram's port
                    // being closed.
                    if let Ok((len, from)) = received {
                        self.datagram(from, &buf[..len]);
                    }
                }
                _ = ticker.tick() => self.tick(),
            }
        }
        self.leave(false);
    }

    fn command(&mut self, command: Command) {
        match command {
            Command::Attach { port, mac, queue } => {
                match port {
                    Port::Nic => {
                        self.switch.nic = Some(mac);
                        if self.router.is_none() {
                            self.router = Some(Router::new(self.now()));
                            self.switch.router = Some(nat::ROUTER_MAC);
                        }
                    }
                    Port::Ipx => self.switch.ipx = Some(mac),
                    Port::Router => self.switch.router = Some(mac),
                    Port::Uplink => return,
                }
                self.queues.insert(port, queue);
            }
            Command::Detach(port) => {
                match port {
                    Port::Nic => {
                        self.switch.nic = None;
                        self.switch.router = None;
                        self.router = None;
                        self.nat.clear();
                    }
                    Port::Ipx => self.switch.ipx = None,
                    Port::Router => self.switch.router = None,
                    Port::Uplink => {}
                }
                self.queues.remove(&port);
            }
            Command::Frame(from, frame) => self.route(from, frame),
            Command::Join(request) => self.join(request),
            Command::Leave => self.leave(true),
            Command::Disband => {
                let Some(uplink) = &mut self.uplink else { return };
                let room = uplink.client.config().room.clone();
                let mut out = Vec::new();
                uplink.client.disband(&mut out);
                self.flush(out);
                self.leave(false);
                self.shared.notice(format!("Ended LAN room \"{}\"", room));
            }
            Command::Host { port, join, share } => self.host(port, join, share),
            Command::StopHost => {
                if let Some(relay) = self.relay.take() {
                    let local = relay.local_addr();
                    drop(relay);
                    self.shared.notice(format!("Stopped relaying on UDP port {}", local.port()));
                    let joined_own = self.uplink.as_ref().is_some_and(|u| {
                        let relay = u.client.config().relay;
                        relay.ip().is_loopback() && relay.port() == local.port()
                    });
                    if joined_own {
                        self.leave(true);
                    }
                }
                self.shared.update(|s| {
                    s.hosting = None;
                    s.hosted_rooms.clear();
                });
            }
            Command::Resolved { generation, relay, request } => {
                if generation == self.generation {
                    self.connect(relay, request);
                }
            }
            Command::Nat(event) => {
                let now = self.now();
                if let Some(router) = &mut self.router {
                    router.event(event, now);
                }
                self.router_actions();
            }
            Command::Serial(command) => {
                let now = self.now();
                match command {
                    SerialCommand::Setup(setup, queue) => {
                        self.serial.setup(queue, setup, now);
                        self.serial_roster();
                    }
                    SerialCommand::Port(port, command) => self.serial.command(port, command, now),
                    SerialCommand::Tcp(event) => self.serial.tcp_event(event),
                    SerialCommand::ListenFailed(notice) => self.serial.listen_failed(notice),
                }
                self.serial_flush();
            }
        }
    }

    /// Pair the serial link up with the room as it is now.
    fn serial_roster(&mut self) {
        let now = self.now();
        match &self.uplink {
            Some(uplink) => {
                let client = &uplink.client;
                let roster = client.roster().cloned();
                self.serial.roster(client.index(), roster.as_ref(), client.relay_serial(), now);
            }
            None => self.serial.roster(None, None, false, now),
        }
        self.serial_flush();
    }

    /// Send the serial link's datagrams, and tell what it has to say.
    fn serial_flush(&mut self) {
        let datagrams = std::mem::take(&mut self.serial.out);
        if !datagrams.is_empty()
            && let Some(uplink) = &mut self.uplink
        {
            let mut out = Vec::new();
            for (peer, payload) in datagrams {
                uplink.client.send_serial(peer, &payload, &mut out);
            }
            self.flush(out);
        }
        for notice in std::mem::take(&mut self.serial.notices) {
            self.shared.notice(notice);
        }
        let status = self.serial.status();
        self.shared.update(|s| s.serial = status);
    }

    /// Pass on what the router has for the card and for the host.
    fn router_actions(&mut self) {
        let Some(router) = &mut self.router else { return };
        for action in router.take_actions() {
            match action {
                nat::Action::ToGuest(frame) => self.route(Port::Router, frame),
                action => self.nat.run(action),
            }
        }
    }

    fn join(&mut self, request: JoinRequest) {
        self.leave(false);
        self.generation += 1;
        let generation = self.generation;
        self.shared.update(|s| {
            s.lan = LanState::Looking;
            s.room = request.room.clone();
        });
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let relay = resolve(request.relay.clone()).await;
            if let Some(tx) = tx.upgrade() {
                let _ = tx.send(Command::Resolved { generation, relay, request });
            }
        });
    }

    fn connect(&mut self, relay: Result<SocketAddr, String>, request: JoinRequest) {
        let bound = relay.and_then(|relay| {
            let local: SocketAddr =
                if relay.is_ipv4() { (Ipv4Addr::UNSPECIFIED, 0).into() } else { (Ipv6Addr::UNSPECIFIED, 0).into() };
            let socket = std::net::UdpSocket::bind(local)
                .and_then(|s| s.set_nonblocking(true).map(|_| s))
                .and_then(UdpSocket::from_std)
                .map_err(|e| format!("can't open a UDP socket: {}", e))?;
            Ok((relay, socket))
        });
        let (relay, socket) = match bound {
            Ok(bound) => bound,
            Err(e) => {
                self.shared.notice(format!("Can't join the LAN: {}", e));
                self.shared.update(|s| s.lan = LanState::Failed(e));
                return;
            }
        };
        let mut out = Vec::new();
        let JoinRequest { room, password, player, .. } = request;
        let config = ClientConfig { relay, room, password, player };
        let client = Client::new(config, self.client_id, self.now(), &mut out);
        let socket = Arc::new(socket);
        for datagram in out {
            let _ = socket.try_send_to(&datagram, relay);
        }
        self.uplink = Some(Uplink { socket, client });
        self.switch.uplink = true;
        self.shared.update(|s| s.lan = LanState::Joining { relay });
    }

    fn leave(&mut self, tell: bool) {
        self.generation += 1;
        self.switch.uplink = false;
        if let Some(router) = &mut self.router {
            router.guest_ip = nat::GUEST;
        }
        if let Some(mut uplink) = self.uplink.take() {
            let mut out = Vec::new();
            uplink.client.leave(&mut out);
            let relay = uplink.client.config().relay;
            for datagram in out {
                let _ = uplink.socket.try_send_to(&datagram, relay);
            }
            if tell {
                self.shared.notice("Left the LAN".into());
            }
        }
        self.shared.update(|s| {
            s.lan = LanState::Off;
            s.roster = None;
        });
        self.serial_roster();
    }

    fn host(&mut self, port: u16, join: JoinRequest, share: bool) {
        let password = join.password.clone();
        self.relay = None;
        // What room browsers show as its host: the player, or this machine.
        let name = Some(join.player.clone()).filter(|p| !p.is_empty()).or_else(machine_name);
        let config = RelayConfig { name: name.unwrap_or_else(|| "rust-dos".into()), password: password.clone(), port };
        let shared = self.shared.clone();
        let log = Box::new(move |line: String| shared.notice(format!("Relay: {}", line)));
        match RelayServer::start((Ipv4Addr::UNSPECIFIED, port).into(), config, log) {
            Ok(relay) => {
                let local = relay.local_addr();
                self.relay = Some(relay);
                self.shared.notice(format!("Hosting LAN rooms on UDP port {}", local.port()));
                self.shared.update(|s| s.hosting = Some(local));
                let relay = Some(format!("127.0.0.1:{}", local.port()));
                self.join(JoinRequest { relay, ..join });
            }
            Err(e) if share && e.kind() == io::ErrorKind::AddrInUse => {
                let relay = Some(format!("127.0.0.1:{}", port));
                self.join(JoinRequest { relay, ..join });
            }
            Err(e) => {
                self.shared.notice(format!("Can't host on UDP port {}: {}", port, e));
                self.shared.update(|s| {
                    s.hosting = None;
                    s.lan = LanState::Failed(format!("can't host on UDP port {}: {}", port, e));
                });
            }
        }
    }

    fn datagram(&mut self, from: SocketAddr, bytes: &[u8]) {
        let now = self.now();
        let Some(uplink) = &mut self.uplink else { return };
        let mut out = Vec::new();
        let events = uplink.client.handle(now, from, bytes, &mut out);
        self.flush(out);
        self.events(events);
    }

    fn tick(&mut self) {
        let now = self.now();
        if let Some(uplink) = &mut self.uplink {
            let mut out = Vec::new();
            let events = uplink.client.tick(now, &mut out);
            let (members, rtt) = (uplink.client.members(), uplink.client.rtt_ms());
            self.flush(out);
            self.events(events);
            self.shared.update(|s| {
                if let LanState::Joined { members: m, rtt_ms, .. } = &mut s.lan {
                    *m = members;
                    *rtt_ms = rtt;
                }
            });
        }
        if let Some(relay) = &self.relay {
            let rooms = relay.status().rooms;
            self.shared.update(|s| s.hosted_rooms = rooms);
        }
        if let Some(router) = &mut self.router {
            router.poll(now);
            let counts = router.counts();
            self.shared.update(|s| s.nat = Some(counts));
        }
        self.router_actions();
        self.serial.poll(now);
        self.serial_flush();
    }

    fn flush(&mut self, out: Vec<Vec<u8>>) {
        let Some(uplink) = &self.uplink else { return };
        let relay = uplink.client.config().relay;
        for datagram in out {
            let _ = uplink.socket.try_send_to(&datagram, relay);
        }
    }

    fn events(&mut self, events: Vec<ClientEvent>) {
        let Some(uplink) = &self.uplink else { return };
        let relay = uplink.client.config().relay;
        let room = uplink.client.config().room.clone();
        for event in events {
            match event {
                ClientEvent::Joined { index, members } => {
                    let others = match members.saturating_sub(1) {
                        0 => "no one else there yet".to_string(),
                        1 => "1 other there".to_string(),
                        n => format!("{} others there", n),
                    };
                    self.shared
                        .notice(format!("Joined LAN room \"{}\" at {} as member {}, {}", room, relay, index, others));
                    self.shared.update(|s| s.lan = LanState::Joined { relay, index, members, rtt_ms: None });
                    // On a LAN shared with other instances' guests, the
                    // card's guest has an address of its own.
                    if let Some(router) = &mut self.router {
                        router.guest_ip = nat::lan_guest(index);
                    }
                }
                ClientEvent::Rejected(RejectReason::Closed) => {
                    self.shared.notice(format!("LAN room \"{}\" was ended by its host", room));
                    self.shared.update(|s| {
                        s.lan = LanState::Failed(RejectReason::Closed.describe().into());
                        s.roster = None;
                    });
                    self.serial_roster();
                }
                ClientEvent::Rejected(reason) => {
                    self.shared.notice(format!("The relay at {} turned us away: {}", relay, reason.describe()));
                    self.shared.update(|s| s.lan = LanState::Failed(reason.describe().into()));
                }
                ClientEvent::Roster(roster) => {
                    self.shared.update(|s| s.roster = Some(roster));
                    self.serial_roster();
                }
                ClientEvent::Serial { from, payload } => {
                    let now = self.now();
                    self.serial.datagram(from, &payload, now);
                    self.serial_flush();
                }
                ClientEvent::Lost => {
                    self.shared.notice(format!("Lost the relay at {}; joining it again", relay));
                    self.shared.update(|s| s.lan = LanState::Rejoining { relay });
                }
                ClientEvent::Frame(frame) => {
                    self.shared.update(|s| s.frames_in += 1);
                    self.route(Port::Uplink, frame);
                }
            }
        }
    }

    /// Pass `frame`, which came in at `from`, on.
    fn route(&mut self, from: Port, frame: Vec<u8>) {
        for port in self.switch.route(from, &frame) {
            match port {
                Port::Uplink => {
                    let Some(uplink) = &mut self.uplink else { continue };
                    let mut out = Vec::new();
                    uplink.client.send_frame(&frame, &mut out);
                    if !out.is_empty() {
                        self.shared.update(|s| s.frames_out += 1);
                    }
                    self.flush(out);
                }
                Port::Router => {
                    let now = self.now();
                    if let Some(router) = &mut self.router {
                        router.from_guest(&frame, now);
                    }
                    self.router_actions();
                }
                port => {
                    if let Some(queue) = self.queues.get(&port) {
                        queue.push(frame.clone());
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wait up to 5 s for `f` to hold.
    fn wait_for(mut f: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    #[test]
    fn two_hubs_meet_in_a_hosted_room() {
        let a = Hub::start().unwrap();
        let b = Hub::start().unwrap();
        let (qa, qb) = (Arc::new(PortQueue::default()), Arc::new(PortQueue::default()));
        let (ma, mb) = (Mac([2, 0, 0, 0, 0, 0xA]), Mac([2, 0, 0, 0, 0, 0xB]));
        a.send(Command::Attach { port: Port::Ipx, mac: ma, queue: qa.clone() });
        b.send(Command::Attach { port: Port::Ipx, mac: mb, queue: qb.clone() });
        let join = JoinRequest { relay: None, room: "doom".into(), password: Some("pw".into()), player: "A".into() };
        a.send(Command::Host { port: 0, join, share: false });
        assert!(wait_for(|| matches!(a.status().lan, LanState::Joined { .. })), "{:?}", a.status());
        let port = a.status().hosting.unwrap().port();
        b.send(Command::Join(JoinRequest {
            relay: Some(format!("localhost:{}", port)),
            room: "doom".into(),
            password: Some("pw".into()),
            player: String::new(),
        }));
        assert!(wait_for(|| matches!(b.status().lan, LanState::Joined { index: 2, .. })), "{:?}", b.status());
        let frame = crate::net::frame::build(Mac::BROADCAST, ma, crate::net::frame::ETHERTYPE_IPX, &[1; 100]);
        a.send(Command::Frame(Port::Ipx, frame.clone()));
        assert!(wait_for(|| qb.is_pending()));
        assert_eq!(qb.pop(), Some(frame));
        assert!(!qa.is_pending(), "the sender doesn't hear itself");
        let notices = a.take_notices();
        assert!(notices.iter().any(|n| n.starts_with("Hosting LAN rooms")), "{:?}", notices);
        assert!(notices.iter().any(|n| n.contains("as member 1")), "{:?}", notices);
        // A wrong password.
        let c = Hub::start().unwrap();
        c.send(Command::Join(JoinRequest {
            relay: Some(format!("127.0.0.1:{}", port)),
            room: "doom".into(),
            password: None,
            player: String::new(),
        }));
        assert!(wait_for(|| matches!(c.status().lan, LanState::Failed(_))), "{:?}", c.status());
        // Both know who is there: A made the room, and hosts it.
        assert!(wait_for(|| b.status().roster.is_some_and(|r| r.total == 2)), "{:?}", b.status());
        let roster = b.status().roster.unwrap();
        assert_eq!((roster.host, roster.members[0].name.as_str()), (1, "A"));
        // A ends it, for B too.
        a.send(Command::Disband);
        assert!(wait_for(|| matches!(b.status().lan, LanState::Failed(_))), "{:?}", b.status());
        assert!(b.take_notices().iter().any(|n| n.contains("ended by its host")));
        // A sends the disband before it leaves, so B can hear it first.
        assert!(wait_for(|| a.status().lan == LanState::Off), "{:?}", a.status());
        assert_eq!(a.status().roster, None);
        b.send(Command::Leave);
        assert!(wait_for(|| b.status().lan == LanState::Off));
    }

    #[test]
    fn a_room_made_where_another_instance_relays_goes_on_its_relay() {
        let join = |room: &str, player: &str| JoinRequest {
            relay: None,
            room: room.into(),
            password: None,
            player: player.into(),
        };
        let a = Hub::start().unwrap();
        a.send(Command::Host { port: 0, join: join("doom", "Ranger"), share: true });
        assert!(wait_for(|| matches!(a.status().lan, LanState::Joined { .. })), "{:?}", a.status());
        let port = a.status().hosting.unwrap().port();
        let b = Hub::start().unwrap();
        b.send(Command::Host { port, join: join("duke", "Kate"), share: true });
        assert!(wait_for(|| matches!(b.status().lan, LanState::Joined { .. })), "{:?}", b.status());
        assert_eq!(b.status().hosting, None);
        assert!(wait_for(|| a.status().hosted_rooms.len() == 2), "{:?}", a.status());
        // The relay goes by its player's name on the LAN.
        let found = discover::discover(port, Duration::from_millis(500)).unwrap();
        assert_eq!(found.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["Ranger"]);
    }

    /// The address the router of `hub` offers a card's guest.
    fn offered(hub: &Hub, queue: &PortQueue, mac: Mac) -> Option<std::net::Ipv4Addr> {
        let mut discover = vec![0u8; 240];
        discover[0..3].copy_from_slice(&[1, 1, 6]);
        discover[28..34].copy_from_slice(&mac.0);
        discover[236..240].copy_from_slice(&[99, 130, 83, 99]);
        discover.extend_from_slice(&[53, 1, 1, 255]);
        let ip = nat::packet::build_udp(Ipv4Addr::UNSPECIFIED, 68, Ipv4Addr::BROADCAST, 67, &discover);
        let frame = crate::net::frame::build(Mac::BROADCAST, mac, crate::net::frame::ETHERTYPE_IPV4, &ip);
        while queue.pop().is_some() {}
        hub.send(Command::Frame(Port::Nic, frame));
        let mut offer = None;
        wait_for(|| {
            while let Some(frame) = queue.pop() {
                let ip = &frame[crate::net::frame::HEADER..];
                if let Some(header) = nat::packet::parse_ipv4(ip)
                    && let Some(udp) = nat::packet::parse_udp(&ip[header.header_len..header.total_len])
                    && udp.src_port == 67
                {
                    let y = &udp.payload[16..20];
                    offer = Some(Ipv4Addr::new(y[0], y[1], y[2], y[3]));
                }
            }
            offer.is_some()
        });
        offer
    }

    #[test]
    fn guests_on_a_shared_lan_get_addresses_of_their_own() {
        let a = Hub::start().unwrap();
        let b = Hub::start().unwrap();
        let (qa, qb) = (Arc::new(PortQueue::default()), Arc::new(PortQueue::default()));
        let (ma, mb) = (Mac([2, 0, 0, 0, 1, 0xA]), Mac([2, 0, 0, 0, 1, 0xB]));
        a.send(Command::Attach { port: Port::Nic, mac: ma, queue: qa.clone() });
        b.send(Command::Attach { port: Port::Nic, mac: mb, queue: qb.clone() });
        assert_eq!(offered(&a, &qa, ma), Some(nat::GUEST));
        let join = JoinRequest { relay: None, room: "net".into(), password: None, player: String::new() };
        a.send(Command::Host { port: 0, join, share: false });
        assert!(wait_for(|| matches!(a.status().lan, LanState::Joined { .. })));
        let port = a.status().hosting.unwrap().port();
        b.send(Command::Join(JoinRequest {
            relay: Some(format!("127.0.0.1:{}", port)),
            room: "net".into(),
            password: None,
            player: String::new(),
        }));
        assert!(wait_for(|| matches!(b.status().lan, LanState::Joined { .. })));
        assert_eq!(offered(&a, &qa, ma), Some(nat::lan_guest(1)));
        assert_eq!(offered(&b, &qb, mb), Some(nat::lan_guest(2)));
        b.send(Command::Leave);
        assert!(wait_for(|| b.status().lan == LanState::Off));
        assert_eq!(offered(&b, &qb, mb), Some(nat::GUEST));
    }
}
