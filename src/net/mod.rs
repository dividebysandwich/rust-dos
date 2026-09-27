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
pub mod ne2000;
pub mod port;
pub mod switch;
pub mod tunnel;

use port::PortQueue;
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
        }
    }
}

impl NetSettings {
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

/// What the LAN command shows: where this instance is with the LAN.
#[derive(Clone, Debug, Default)]
pub struct LanStatus {
    #[cfg(not(target_arch = "wasm32"))]
    pub hub: Option<hub::HubStatus>,
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
    /// What the screen should show.
    notices: Vec<String>,
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
            notices: Vec::new(),
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
            let password = (!password.is_empty()).then(|| password.to_string());
            self.hub()?.send(hub::Command::Host { port, room: room.into(), password });
            Ok(())
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (port, room, password);
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
        assert!(n.set("modem", "on").is_err());
        assert_eq!(NetSettings::default().entries().iter().find(|e| e.0 == "password").unwrap().1, None);
    }
}
