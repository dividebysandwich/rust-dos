//! Networking: the LAN tunnel that joins rust-dos instances through a relay
//! over UDP (`tunnel`), the switch in each instance that the emulated
//! network devices hang off (`switch`), and the IPX driver of the built-in
//! DOS (`ipx`).
//!
//! Everything that crosses the tunnel is an Ethernet frame, so the IPX
//! driver of the built-in DOS and a network card of a booted system talk
//! to each other the way machines on one Ethernet segment would.
//!
//! `Net` is the part the emulator keeps on its bus. The sockets live on a
//! thread of their own (`hub`), which the browser build has none of.

pub mod frame;
#[cfg(not(target_arch = "wasm32"))]
pub mod hub;
pub mod ipx;
#[cfg(not(target_arch = "wasm32"))]
pub mod nat;
pub mod ne2000;
pub mod port;
pub mod serial;
pub mod switch;
pub mod tunnel;

use port::PortQueue;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Mutex;
use std::net::SocketAddr;
use std::sync::Arc;
use switch::Port;

/// Fill `buf` with random bytes: from the operating system, or in the
/// browser, where there is none to ask, from the clock.
pub fn fill_random(buf: &mut [u8]) {
    #[cfg(not(target_arch = "wasm32"))]
    if getrandom::fill(buf).is_ok() {
        return;
    }
    let nanos =
        web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
    // SplitMix64 over the time and the buffer's address.
    let mut state = nanos ^ (buf.as_ptr() as u64).rotate_left(32);
    for chunk in buf.chunks_mut(8) {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        chunk.copy_from_slice(&z.to_le_bytes()[..chunk.len()]);
    }
}

/// A random 64-bit number.
pub fn random_u64() -> u64 {
    let mut bytes = [0; 8];
    fill_random(&mut bytes);
    u64::from_le_bytes(bytes)
}

/// Whether the built-in DOS has the IPX driver (`ipx`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum IpxMode {
    /// From the first LAN HOST or LAN JOIN on.
    #[default]
    Auto,
    On,
    Off,
}

impl IpxMode {
    pub const ALL: [IpxMode; 3] = [IpxMode::Auto, IpxMode::On, IpxMode::Off];

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(IpxMode::Auto),
            "true" | "on" | "yes" | "1" => Some(IpxMode::On),
            "false" | "off" | "no" | "0" => Some(IpxMode::Off),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            IpxMode::Auto => "auto",
            IpxMode::On => "true",
            IpxMode::Off => "false",
        }
    }
}

/// The `[network]` settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetSettings {
    pub ipx: IpxMode,
    /// The IPX driver's IRQ, or None to take the first one free.
    pub ipx_irq: Option<u8>,
    /// The NE2000 network card, its ports and IRQ, and its address (None
    /// for a random one each start).
    pub ne2000: bool,
    pub nic_base: u16,
    pub nic_irq: u8,
    pub mac: Option<frame::Mac>,
    pub ipx_frame: ipx::FrameType,
    /// A LAN room to join at startup: None, or a relay (`host[:port]`),
    /// or `Some("")` for the first relay that answers on the LAN.
    pub lan: Option<String>,
    /// A port to host LAN rooms on from startup.
    pub lan_host: Option<u16>,
    pub room: String,
    pub password: String,
    /// Whether LAN JOIN, LAN LIST and the room browser go to the relay
    /// online (`relay`, `host[:port]`) unless told another, or look on
    /// this network, where making a room in the browser hosts it here.
    pub online: bool,
    pub relay: String,
    /// The name this instance's player goes by in LAN rooms, empty for
    /// none (the others see "Player" and the member's number).
    pub player: String,
}

impl Default for NetSettings {
    fn default() -> Self {
        Self {
            ipx: IpxMode::Auto,
            ipx_irq: None,
            ne2000: false,
            nic_base: 0x300,
            nic_irq: 10,
            mac: None,
            ipx_frame: ipx::FrameType::EthernetII,
            lan: None,
            lan_host: None,
            room: DEFAULT_ROOM.into(),
            password: String::new(),
            online: false,
            relay: DEFAULT_RELAY.into(),
            player: String::new(),
        }
    }
}

impl NetSettings {
    /// The relay rooms are at without another given: the one online, or
    /// None for those on this network.
    pub fn rooms_relay(&self) -> Option<&str> {
        self.online.then_some(self.relay.as_str())
    }

    /// Take `key=value` of the `[network]` section.
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        let off = |v: &str| matches!(v.to_ascii_lowercase().as_str(), "off" | "none" | "false" | "no" | "");
        match key.to_ascii_lowercase().as_str() {
            "ipx" => {
                self.ipx =
                    IpxMode::parse(value).ok_or_else(|| format!("invalid ipx '{}' (auto, true or false)", value))?
            }
            "ipxirq" => {
                self.ipx_irq = match value.trim() {
                    v if v.eq_ignore_ascii_case("auto") => None,
                    v => match v.parse::<u8>() {
                        Ok(irq) if matches!(irq, 3..=5 | 7 | 9..=11 | 15) => Some(irq),
                        _ => return Err(format!("invalid ipxirq '{}' (auto, 3, 4, 5, 7, 9, 10, 11 or 15)", value)),
                    },
                }
            }
            "ne2000" => {
                self.ne2000 = match value.trim().to_ascii_lowercase().as_str() {
                    "true" | "on" | "yes" | "1" => true,
                    "false" | "off" | "no" | "0" => false,
                    _ => return Err(format!("invalid ne2000 '{}' (true or false)", value)),
                }
            }
            "nicbase" => {
                self.nic_base = u16::from_str_radix(value.trim().trim_start_matches("0x"), 16)
                    .ok()
                    .filter(|base| NIC_BASES.contains(base))
                    .ok_or_else(|| format!("invalid nicbase '{}' (240, 260, 280, 2A0, 2C0, 300, 320, 340 or 360)", value))?
            }
            "nicirq" => {
                self.nic_irq = value
                    .trim()
                    .parse()
                    .ok()
                    .filter(|irq| matches!(irq, 3..=5 | 7 | 9..=11 | 15))
                    .ok_or_else(|| format!("invalid nicirq '{}' (3, 4, 5, 7, 9, 10, 11 or 15)", value))?
            }
            "macaddr" => {
                self.mac = match value.trim() {
                    v if v.eq_ignore_ascii_case("auto") => None,
                    v => Some(
                        frame::Mac::parse(v)
                            .filter(|mac| !mac.is_group())
                            .ok_or_else(|| format!("invalid macaddr '{}' (auto, or like 02:00:5E:12:34:56)", value))?,
                    ),
                }
            }
            "ipxframe" => {
                self.ipx_frame = ipx::FrameType::parse(value)
                    .ok_or_else(|| format!("invalid ipxframe '{}' (ethernet_ii, 802.3, 802.2 or snap)", value))?
            }
            "lan" => {
                self.lan = match value.trim() {
                    v if off(v) => None,
                    v if v.eq_ignore_ascii_case("discover") => Some(String::new()),
                    v => Some(v.to_string()),
                }
            }
            "lanhost" => {
                self.lan_host = match value.trim() {
                    v if off(v) => None,
                    v if v.eq_ignore_ascii_case("on") || v.eq_ignore_ascii_case("true") => {
                        Some(tunnel::wire::DEFAULT_PORT)
                    }
                    v => Some(v.parse().map_err(|_| format!("invalid lanhost '{}' (off or a UDP port)", value))?),
                }
            }
            "room" => {
                let room = value.trim();
                if room.is_empty() || room.len() > tunnel::wire::MAX_NAME {
                    return Err(format!("invalid room '{}' (1 to {} characters)", value, tunnel::wire::MAX_NAME));
                }
                self.room = room.to_string();
            }
            "password" => self.password = value.trim().to_string(),
            "player" => {
                let player = value.trim();
                if !tunnel::wire::valid_player(player) {
                    return Err(format!("invalid player '{}' (up to {} characters)", value, tunnel::wire::MAX_NAME));
                }
                self.player = player.to_string();
            }
            "online" => {
                self.online = match value.trim().to_ascii_lowercase().as_str() {
                    "true" | "on" | "yes" | "1" => true,
                    "false" | "off" | "no" | "0" => false,
                    _ => return Err(format!("invalid online '{}' (true or false)", value)),
                }
            }
            "relay" => {
                self.relay = match value.trim() {
                    v if !v.is_empty() && !v.contains(char::is_whitespace) => v.to_string(),
                    _ => return Err(format!("invalid relay '{}' (a host[:port])", value)),
                }
            }
            _ => return Err(format!("unknown setting '{}'", key)),
        }
        Ok(())
    }

    /// The settings as the configuration file has them: a line for each
    /// key with a value.
    pub fn entries(&self) -> Vec<(&'static str, Option<String>)> {
        vec![
            ("ipx", Some(self.ipx.name().to_string())),
            ("ipxirq", Some(self.ipx_irq.map_or("auto".to_string(), |irq| irq.to_string()))),
            ("ipxframe", Some(self.ipx_frame.name().to_string())),
            ("ne2000", Some(self.ne2000.to_string())),
            ("nicbase", Some(format!("{:X}", self.nic_base))),
            ("nicirq", Some(self.nic_irq.to_string())),
            ("macaddr", Some(self.mac.map_or("auto".to_string(), |mac| mac.to_string()))),
            ("online", Some(self.online.to_string())),
            ("relay", Some(self.relay.clone())),
            ("player", Some(self.player.clone())),
            (
                "lan",
                Some(match &self.lan {
                    None => "off".to_string(),
                    Some(relay) if relay.is_empty() => "discover".to_string(),
                    Some(relay) => relay.clone(),
                }),
            ),
            ("lanhost", Some(self.lan_host.map_or("off".to_string(), |port| port.to_string()))),
            ("room", Some(self.room.clone())),
            ("password", (!self.password.is_empty()).then(|| self.password.clone())),
        ]
    }
}

/// Where the NE2000 may be: 20h ports that no sound card's standard
/// ports overlap.
pub const NIC_BASES: [u16; 9] = [0x240, 0x260, 0x280, 0x2A0, 0x2C0, 0x300, 0x320, 0x340, 0x360];

/// The room LAN HOST and LAN JOIN use unless told another.
pub const DEFAULT_ROOM: &str = "lobby";

/// The public relay, where anyone can find and make rooms.
pub const DEFAULT_RELAY: &str = "relay.rust-dos.com";

/// The rooms a relay listed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoomList {
    /// Where the relay is, its name, and whether all its rooms want its
    /// password.
    pub relay: SocketAddr,
    pub name: String,
    pub password: bool,
    /// The rooms, the fullest first, and how many the relay has: more when
    /// they didn't all fit in the pages asked for.
    pub rooms: Vec<tunnel::wire::RoomInfo>,
    pub total: usize,
}

/// The rooms asked of a relay, or of those on this network
/// (`Net::browse`), and what came back.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listing {
    /// A question is out.
    pub asking: bool,
    /// What the last answer found, a list for each relay, or why there
    /// was none.
    pub result: Option<Result<Vec<RoomList>, String>>,
}

/// What the room browser shows of the LAN.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LanView {
    /// Where this instance is with the LAN, in words, and the relay and
    /// room it is in, once it is.
    pub state: String,
    pub joined: Option<(SocketAddr, String)>,
    /// This instance's member index in the room, and who is in it, as the
    /// relay last said.
    pub index: Option<u8>,
    pub roster: Option<tunnel::wire::Roster>,
    pub listing: Listing,
    /// The member the serial link goes to, and what it says of it
    /// ("COM2 linked").
    pub serial: Option<(u8, String)>,
}

impl LanView {
    /// Whether this instance hosts the room it is in.
    pub fn hosting(&self) -> bool {
        matches!((self.index, &self.roster), (Some(index), Some(roster)) if roster.host == index)
    }
}

/// What the LAN command shows: where this instance is with the LAN.
#[derive(Clone, Debug, Default)]
pub struct LanStatus {
    #[cfg(not(target_arch = "wasm32"))]
    pub hub: Option<hub::HubStatus>,
}

impl LanStatus {
    /// Where this instance is with the LAN, in words.
    pub fn describe(&self) -> String {
        #[cfg(not(target_arch = "wasm32"))]
        {
            use hub::LanState;
            let Some(status) = &self.hub else { return "not joined".to_string() };
            match &status.lan {
                LanState::Off => "not joined".to_string(),
                LanState::Looking => format!("looking for the relay of room \"{}\"", status.room),
                LanState::Joining { relay } => format!("joining room \"{}\" at {}", status.room, relay),
                LanState::Rejoining { relay } => format!("joining room \"{}\" at {} again", status.room, relay),
                LanState::Failed(e) => format!("not joined: {}", e),
                LanState::Joined { relay, index, members, rtt_ms } => format!(
                    "room \"{}\" at {}, member {} of {}{}",
                    status.room,
                    relay,
                    index,
                    members,
                    rtt_ms.map_or(String::new(), |ms| format!(", {} ms to the relay", ms))
                ),
            }
        }
        #[cfg(target_arch = "wasm32")]
        NO_NETWORK.to_string()
    }

    /// This instance's member index in the room it is in, and who is in
    /// it, as the relay last said.
    pub fn roster(&self) -> Option<(u8, tunnel::wire::Roster)> {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(status) = &self.hub
            && let hub::LanState::Joined { index, .. } = status.lan
            && let Some(roster) = &status.roster
        {
            return Some((index, roster.clone()));
        }
        None
    }

    /// The relay and the room this instance is in, once joined.
    pub fn joined(&self) -> Option<(SocketAddr, String)> {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(status) = &self.hub
            && let hub::LanState::Joined { relay, .. } = status.lan
        {
            return Some((relay, status.room.clone()));
        }
        None
    }
}

/// The network as the emulator keeps it on its bus.
pub struct Net {
    pub settings: NetSettings,
    /// The built-in DOS's IPX driver, once installed.
    pub ipx: Option<ipx::Ipx>,
    /// The frames the network thread has for the IPX driver.
    pub ipx_queue: Arc<PortQueue>,
    /// The NE2000, if the machine has one, and the frames for it.
    pub nic: Option<ne2000::Ne2000>,
    pub nic_queue: Arc<PortQueue>,
    #[cfg(not(target_arch = "wasm32"))]
    hub: Option<hub::Hub>,
    /// The rooms asked of a relay last, and how many were asked for, so
    /// the answer to one asked before the last is dropped.
    #[cfg(not(target_arch = "wasm32"))]
    listing: Arc<Mutex<(u64, Listing)>>,
    /// What the screen should show.
    notices: Vec<String>,
    /// The serial ports' links.
    pub serial: serial::SerialNet,
}

impl Default for Net {
    fn default() -> Self {
        Self::new()
    }
}

impl Net {
    pub fn new() -> Self {
        Self {
            settings: NetSettings::default(),
            ipx: None,
            ipx_queue: Arc::new(PortQueue::default()),
            nic: None,
            nic_queue: Arc::new(PortQueue::default()),
            #[cfg(not(target_arch = "wasm32"))]
            hub: None,
            #[cfg(not(target_arch = "wasm32"))]
            listing: Arc::default(),
            notices: Vec::new(),
            serial: serial::SerialNet::default(),
        }
    }

    /// Whether the network thread runs, so the frames it passes on have
    /// to be looked for often.
    pub fn active(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        if self.hub.is_some() {
            return true;
        }
        false
    }

    /// The network thread, started if it isn't running.
    #[cfg(not(target_arch = "wasm32"))]
    fn hub(&mut self) -> Result<&hub::Hub, String> {
        if self.hub.is_none() {
            let hub = hub::Hub::start().map_err(|e| format!("can't start the network: {}", e))?;
            if let Some(ipx) = &self.ipx {
                hub.send(hub::Command::Attach { port: Port::Ipx, mac: ipx.node, queue: self.ipx_queue.clone() });
            }
            if let Some(nic) = &self.nic {
                hub.send(hub::Command::Attach { port: Port::Nic, mac: nic.mac, queue: self.nic_queue.clone() });
            }
            self.hub = Some(hub);
            self.send_serial_setup();
        }
        Ok(self.hub.as_ref().unwrap())
    }

    /// The IPX driver was installed: its frames come through the switch.
    pub fn ipx_installed(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        if let (Some(hub), Some(ipx)) = (&self.hub, &self.ipx) {
            hub.send(hub::Command::Attach { port: Port::Ipx, mac: ipx.node, queue: self.ipx_queue.clone() });
        }
    }

    /// The network card came, went or changed: the network thread, which
    /// it needs, runs, and passes it its frames.
    pub fn nic_changed(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        match &self.nic {
            Some(nic) => {
                let (mac, queue) = (nic.mac, self.nic_queue.clone());
                if let Ok(hub) = self.hub() {
                    hub.send(hub::Command::Attach { port: Port::Nic, mac, queue });
                }
            }
            None => {
                if let Some(hub) = &self.hub {
                    hub.send(hub::Command::Detach(Port::Nic));
                }
            }
        }
        self.nic_queue.clear();
    }

    /// Send a frame from the device on `port`.
    pub fn send(&mut self, port: Port, frame: Vec<u8>) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(hub) = &self.hub {
            hub.send(hub::Command::Frame(port, frame));
        }
        #[cfg(target_arch = "wasm32")]
        let _ = (port, frame);
    }

    /// Host LAN rooms on UDP `port` (0 for any free one), and join `room`.
    pub fn host(&mut self, port: u16, room: &str, password: &str) -> Result<(), String> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let join = hub::JoinRequest {
                relay: None,
                room: room.into(),
                password: (!password.is_empty()).then(|| password.to_string()),
                player: self.settings.player.clone(),
            };
            self.hub()?.send(hub::Command::Host { port, join, share: false });
            Ok(())
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (port, room, password);
            Err(NO_NETWORK.into())
        }
    }

    /// Make `room` on this network and join it: on the relay this
    /// instance runs, which it starts on the relay port if it runs none,
    /// or with that port taken by another instance on this machine, on
    /// that one's.
    pub fn make_room(&mut self, room: &str, password: &str) -> Result<(), String> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if let Some(local) = self.status().hub.and_then(|h| h.hosting) {
                return self.join(Some(&format!("127.0.0.1:{}", local.port())), room, password);
            }
            let join = hub::JoinRequest {
                relay: None,
                room: room.into(),
                password: (!password.is_empty()).then(|| password.to_string()),
                player: self.settings.player.clone(),
            };
            self.hub()?.send(hub::Command::Host { port: tunnel::wire::DEFAULT_PORT, join, share: true });
            Ok(())
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (room, password);
            Err(NO_NETWORK.into())
        }
    }

    /// Join `room` at `relay` (`host[:port]`), or at the first relay that
    /// answers on the LAN.
    pub fn join(&mut self, relay: Option<&str>, room: &str, password: &str) -> Result<(), String> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let request = hub::JoinRequest {
                relay: relay.map(String::from),
                room: room.into(),
                password: (!password.is_empty()).then(|| password.to_string()),
                player: self.settings.player.clone(),
            };
            self.hub()?.send(hub::Command::Join(request));
            Ok(())
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (relay, room, password);
            Err(NO_NETWORK.into())
        }
    }

    /// End the room for everyone in it, as its host, and leave it.
    pub fn disband(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(hub) = &self.hub {
            hub.send(hub::Command::Disband);
        }
    }

    pub fn leave(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(hub) = &self.hub {
            hub.send(hub::Command::Leave);
        }
    }

    pub fn stop_hosting(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(hub) = &self.hub {
            hub.send(hub::Command::StopHost);
        }
    }

    pub fn status(&self) -> LanStatus {
        LanStatus {
            #[cfg(not(target_arch = "wasm32"))]
            hub: self.hub.as_ref().map(|h| h.status()),
        }
    }

    /// Ask `relay` (`host[:port]`, or None for each relay that answers on
    /// the LAN) for its rooms whose names contain `filter`, in a
    /// thread of its own: `listing` has the answer once it comes. The
    /// network thread needn't run for it.
    pub fn browse(&mut self, relay: Option<&str>, filter: &str) -> Result<(), String> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let shared = self.listing.clone();
            let Ok(mut asked) = shared.lock() else { return Err("can't ask the relay".into()) };
            asked.0 += 1;
            asked.1.asking = true;
            let generation = asked.0;
            drop(asked);
            let (relay, filter) = (relay.map(String::from), filter.to_string());
            std::thread::Builder::new()
                .name("rust-dos-rooms".into())
                .spawn(move || {
                    let result = tunnel::discover::rooms(relay.as_deref(), &filter);
                    if let Ok(mut asked) = shared.lock()
                        && asked.0 == generation
                    {
                        asked.1 = Listing { asking: false, result: Some(result) };
                    }
                })
                .map(|_| ())
                .map_err(|e| format!("can't ask the relay: {}", e))
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (relay, filter);
            Err(NO_NETWORK.into())
        }
    }

    /// The rooms `browse` asked for, as far as they came.
    pub fn listing(&self) -> Listing {
        #[cfg(not(target_arch = "wasm32"))]
        if let Ok(asked) = self.listing.lock() {
            return asked.1.clone();
        }
        Listing::default()
    }

    /// What the room browser shows.
    pub fn view(&self) -> LanView {
        let status = self.status();
        let (index, roster) = status.roster().unzip();
        #[cfg(not(target_arch = "wasm32"))]
        let serial = status.hub.as_ref().and_then(|hub| {
            let s = &hub.serial;
            let (port, peer) = (s.port?, s.peer?);
            Some((peer, format!("COM{} {}", port + 1, if s.up { "linked" } else { "linking" })))
        });
        #[cfg(target_arch = "wasm32")]
        let serial = None;
        LanView { state: status.describe(), joined: status.joined(), index, roster, listing: self.listing(), serial }
    }

    /// What happened on the LAN since the last call, for the screen.
    pub fn take_notices(&mut self) -> Vec<String> {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(hub) = &self.hub {
            self.notices.extend(hub.take_notices());
        }
        std::mem::take(&mut self.notices)
    }
}

/// Why the browser can't join a LAN.
#[cfg(target_arch = "wasm32")]
const NO_NETWORK: &str = "the browser version of rust-dos has no network";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_settings_parse_and_print() {
        let mut n = NetSettings::default();
        for (key, value) in [
            ("ipx", "true"),
            ("ipxirq", "10"),
            ("ipxframe", "802.2"),
            ("ne2000", "true"),
            ("nicbase", "280"),
            ("nicirq", "5"),
            ("macaddr", "02:00:5e:12:34:56"),
            ("lan", "relay.example.com:4000"),
            ("lanhost", "21300"),
            ("room", "doom"),
            ("password", "swordfish"),
            ("online", "true"),
            ("relay", "relay.example.com:4000"),
            ("player", " Toumal "),
        ] {
            n.set(key, value).unwrap();
        }
        assert_eq!(
            n,
            NetSettings {
                ipx: IpxMode::On,
                ipx_irq: Some(10),
                ne2000: true,
                nic_base: 0x280,
                nic_irq: 5,
                mac: frame::Mac::parse("02:00:5E:12:34:56"),
                ipx_frame: ipx::FrameType::Llc8022,
                lan: Some("relay.example.com:4000".into()),
                lan_host: Some(21300),
                room: "doom".into(),
                password: "swordfish".into(),
                online: true,
                relay: "relay.example.com:4000".into(),
                player: "Toumal".into(),
            }
        );
        let mut again = NetSettings::default();
        for (key, value) in n.entries() {
            again.set(key, &value.unwrap()).unwrap();
        }
        assert_eq!(again, n);
        n.set("lan", "discover").unwrap();
        assert_eq!(n.lan, Some(String::new()));
        assert_eq!(n.entries().iter().find(|e| e.0 == "lan").unwrap().1.as_deref(), Some("discover"));
        n.set("lan", "off").unwrap();
        n.set("lanhost", "on").unwrap();
        assert_eq!((n.lan.clone(), n.lan_host), (None, Some(tunnel::wire::DEFAULT_PORT)));
        assert!(n.set("ipxirq", "12").is_err());
        assert!(n.set("nicbase", "330").is_err());
        assert!(n.set("nicirq", "12").is_err());
        assert!(n.set("macaddr", "01:00:5e:00:00:01").is_err(), "a group address");
        assert!(n.set("ipxframe", "token-ring").is_err());
        assert!(n.set("lanhost", "x").is_err());
        assert!(n.set("room", "").is_err());
        assert!(n.set("relay", "").is_err() && n.set("relay", "a b").is_err());
        assert!(n.set("player", &"x".repeat(33)).is_err() && n.set("player", "a\tb").is_err());
        assert!(n.set("online", "maybe").is_err());
        assert_eq!(n.rooms_relay(), Some("relay.example.com:4000"));
        // Rooms are on this network unless set otherwise.
        let default = NetSettings::default();
        assert_eq!((default.rooms_relay(), default.relay.as_str()), (None, DEFAULT_RELAY));
        assert!(n.set("modem", "on").is_err());
        assert_eq!(NetSettings::default().entries().iter().find(|e| e.0 == "password").unwrap().1, None);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn lists_the_rooms_of_a_relay_and_the_one_joined() {
        use tunnel::relay::{RelayConfig, RelayServer};
        use tunnel::wire::RoomInfo;
        let wait = |f: &mut dyn FnMut() -> bool| {
            let deadline = web_time::Instant::now() + std::time::Duration::from_secs(5);
            while !f() && web_time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };
        let config = RelayConfig { name: "den".into(), password: None, port: 0 };
        let server = RelayServer::start("127.0.0.1:0".parse().unwrap(), config, Box::new(|_| {})).unwrap();
        let relay = server.local_addr();
        let mut net = Net::new();
        net.settings.player = "Toumal".into();
        assert_eq!(net.view().state, "not joined");
        net.join(Some(&relay.to_string()), "doom", "pw").unwrap();
        wait(&mut || net.view().roster.is_some());
        // Who made the room hosts it.
        let view = net.view();
        assert!(view.hosting(), "{:?}", view);
        assert_eq!(view.roster.unwrap().members, [tunnel::wire::Member { index: 1, name: "Toumal".into() }]);
        net.browse(Some(&relay.to_string()), "").unwrap();
        assert!(net.listing().asking);
        wait(&mut || !net.listing().asking);
        let view = net.view();
        assert_eq!(view.joined, Some((relay, "doom".to_string())));
        assert!(view.state.contains("member 1 of 1"), "{}", view.state);
        let list = view.listing.result.unwrap().unwrap().remove(0);
        assert_eq!((list.relay, list.name.as_str(), list.total), (relay, "den", 1));
        assert_eq!(list.rooms, vec![RoomInfo { name: "doom".into(), members: 1, password: true }]);
        // A relay that doesn't answer.
        drop(server);
        net.browse(Some(&relay.to_string()), "").unwrap();
        wait(&mut || !net.listing().asking);
        assert!(net.listing().result.unwrap().is_err());
    }
}
