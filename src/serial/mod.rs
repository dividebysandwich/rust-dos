//! The serial ports COM1-COM4: a UART each (`uart`), and what is plugged
//! into it: a serial mouse (`mouse`), a null modem cable to the other
//! player in a LAN room (`nullmodem`), or a Hayes modem that dials the
//! other player or a host on the internet (`modem`).
//!
//! The mouse is moved by the host's mouse. The cable and the modem reach
//! the network thread through `LinkCmd`s and hear back through
//! `LinkEvent`s, which the bus passes between them and the network.

pub mod modem;
pub mod mouse;
pub mod nullmodem;
pub mod uart;

use crate::savestate::{Reader, Result, State, Writer};
use uart::{Chip, Uart};

/// The ports' standard addresses and IRQs.
pub const BASES: [u16; 4] = [0x3F8, 0x2F8, 0x3E8, 0x2E8];
pub const IRQS: [u8; 4] = [4, 3, 4, 3];

/// What a port has plugged in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PortType {
    /// No port.
    #[default]
    Off,
    /// The port, with nothing plugged in.
    Empty,
    Mouse,
    Modem,
    NullModem,
}

crate::state_enum!(PortType { PortType::Off, PortType::Empty, PortType::Mouse, PortType::Modem, PortType::NullModem });

impl PortType {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "false" | "disabled" => Some(PortType::Off),
            "empty" | "dummy" | "on" | "true" => Some(PortType::Empty),
            "mouse" | "serialmouse" => Some(PortType::Mouse),
            "modem" => Some(PortType::Modem),
            "nullmodem" | "null" => Some(PortType::NullModem),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            PortType::Off => "off",
            PortType::Empty => "empty",
            PortType::Mouse => "mouse",
            PortType::Modem => "modem",
            PortType::NullModem => "nullmodem",
        }
    }

    /// Whether it goes to another machine.
    pub fn links(self) -> bool {
        matches!(self, PortType::Modem | PortType::NullModem)
    }
}

/// The `[serial]` settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SerialSettings {
    pub ports: [PortType; 4],
    pub irqs: [u8; 4],
    pub chip: Chip,
    pub mouse: mouse::MouseType,
    /// A TCP port the modem takes calls on.
    pub modem_listen: Option<u16>,
    /// Whether the modem's TCP calls speak telnet.
    pub modem_telnet: bool,
}

impl Default for SerialSettings {
    fn default() -> Self {
        Self {
            ports: [PortType::Mouse, PortType::Modem, PortType::Off, PortType::Off],
            irqs: IRQS,
            chip: Chip::Ns16550,
            mouse: mouse::MouseType::Microsoft,
            modem_listen: None,
            modem_telnet: false,
        }
    }
}

impl SerialSettings {
    /// Take `key=value` of the `[serial]` section.
    pub fn set(&mut self, key: &str, value: &str) -> std::result::Result<(), String> {
        let key = key.to_ascii_lowercase();
        let on = |v: &str| match v.trim().to_ascii_lowercase().as_str() {
            "true" | "on" | "yes" | "1" => Some(true),
            "false" | "off" | "no" | "0" => Some(false),
            _ => None,
        };
        if let Some(n) = key.strip_suffix("irq").and_then(|k| port_number(k, "serial")) {
            self.irqs[n] = value
                .trim()
                .parse()
                .ok()
                .filter(|irq| matches!(irq, 3..=5 | 7 | 9..=12 | 15))
                .ok_or_else(|| format!("invalid {} '{}' (3, 4, 5, 7, 9, 10, 11, 12 or 15)", key, value))?;
            return Ok(());
        }
        if let Some(n) = port_number(&key, "serial") {
            self.ports[n] = PortType::parse(value)
                .ok_or_else(|| format!("invalid {} '{}' (off, mouse, modem, nullmodem or empty)", key, value))?;
            return Ok(());
        }
        match key.as_str() {
            "uart" => {
                self.chip = match value.trim() {
                    "16550" | "16550a" | "16550A" => Chip::Ns16550,
                    "8250" | "16450" => Chip::Ns8250,
                    _ => return Err(format!("invalid uart '{}' (16550 or 8250)", value)),
                }
            }
            "mousetype" => {
                self.mouse = mouse::MouseType::parse(value)
                    .ok_or_else(|| format!("invalid mousetype '{}' (microsoft or logitech)", value))?
            }
            "modemlisten" => {
                self.modem_listen = match value.trim() {
                    v if on(v) == Some(false) || v.is_empty() => None,
                    v => Some(
                        v.parse()
                            .ok()
                            .filter(|&p| p != 0)
                            .ok_or_else(|| format!("invalid modemlisten '{}' (off or a TCP port)", value))?,
                    ),
                }
            }
            "modemtelnet" => {
                self.modem_telnet = on(value).ok_or_else(|| format!("invalid modemtelnet '{}' (on or off)", value))?
            }
            _ => return Err(format!("unknown setting '{}'", key)),
        }
        Ok(())
    }

    /// The settings as the configuration file has them.
    pub fn entries(&self) -> Vec<(&'static str, Option<String>)> {
        const PORTS: [&str; 4] = ["serial1", "serial2", "serial3", "serial4"];
        const IRQ_KEYS: [&str; 4] = ["serial1irq", "serial2irq", "serial3irq", "serial4irq"];
        let mut entries = Vec::new();
        for (key, port) in PORTS.into_iter().zip(self.ports) {
            entries.push((key, Some(port.name().to_string())));
        }
        for (key, irq) in IRQ_KEYS.into_iter().zip(self.irqs) {
            entries.push((key, Some(irq.to_string())));
        }
        entries.push(("uart", Some(if self.chip == Chip::Ns8250 { "8250" } else { "16550" }.into())));
        entries.push(("mousetype", Some(self.mouse.name().into())));
        entries.push(("modemlisten", Some(self.modem_listen.map_or("off".into(), |p| p.to_string()))));
        entries.push(("modemtelnet", Some(if self.modem_telnet { "on" } else { "off" }.into())));
        entries
    }

    /// The port that goes to the other player in a LAN room: the first
    /// modem or null modem.
    pub fn linked_port(&self) -> Option<usize> {
        self.ports.iter().position(|p| p.links())
    }
}

/// `serialN` in `key` after `prefix`: the port's index.
fn port_number(key: &str, prefix: &str) -> Option<usize> {
    key.strip_prefix(prefix)
        .and_then(|n| n.parse::<usize>().ok())
        .filter(|n| (1..=4).contains(n))
        .map(|n| n - 1)
}

/// What a port's cable or modem asks of the network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkCmd {
    /// Characters for the other end.
    Bytes(Vec<u8>),
    /// Our DTR and RTS, for the other end's DSR/DCD and CTS.
    Lines { dtr: bool, rts: bool },
    /// Call: the other player in the room (`None`), or `host:port`.
    Dial(Option<String>),
    /// Take the call that rings.
    Answer,
    /// End the call.
    Hangup,
}

/// What the network tells a port's cable or modem.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkEvent {
    /// The other player in the room is there (a cable to them), or gone.
    Peer(bool),
    /// A call is through.
    Connected,
    /// A call failed, or ended.
    NoCarrier,
    /// A call comes in.
    Ring,
    /// Characters from the other end.
    Bytes(Vec<u8>),
    /// The other end's DTR and RTS.
    Lines { dtr: bool, rts: bool },
}

/// What a port has plugged in, as it runs.
#[derive(Clone, Debug)]
pub enum Backend {
    Empty,
    Mouse(mouse::SerialMouse),
    Modem(Box<modem::Modem>),
    NullModem(nullmodem::NullModem),
}

impl Backend {
    fn new(kind: PortType, settings: &SerialSettings) -> Self {
        match kind {
            PortType::Off | PortType::Empty => Backend::Empty,
            PortType::Mouse => Backend::Mouse(mouse::SerialMouse::new(settings.mouse)),
            PortType::Modem => Backend::Modem(Box::default()),
            PortType::NullModem => Backend::NullModem(nullmodem::NullModem::default()),
        }
    }

    pub fn kind(&self) -> PortType {
        match self {
            Backend::Empty => PortType::Empty,
            Backend::Mouse(_) => PortType::Mouse,
            Backend::Modem(_) => PortType::Modem,
            Backend::NullModem(_) => PortType::NullModem,
        }
    }
}

/// A serial port: its UART, and what is plugged into it.
#[derive(Clone, Debug)]
pub struct SerialPort {
    pub uart: Uart,
    pub backend: Backend,
    /// The modem control lines as the backend last saw them.
    mcr: u8,
    /// What the backend asks of the network, for the bus to pass on.
    pub link: Vec<LinkCmd>,
}

impl SerialPort {
    pub fn new(n: usize, kind: PortType, settings: &SerialSettings) -> Self {
        Self {
            uart: Uart::new(BASES[n], settings.irqs[n], settings.chip),
            backend: Backend::new(kind, settings),
            mcr: 0,
            link: Vec::new(),
        }
    }

    /// After the UART did something: hand the backend what it sent and
    /// the lines it changed.
    pub fn sync(&mut self, now: u64) {
        let mcr = self.uart.mcr_out();
        if mcr != self.mcr {
            self.mcr = mcr;
            match &mut self.backend {
                Backend::Empty => {}
                Backend::Mouse(m) => m.control(&mut self.uart, mcr, now),
                Backend::Modem(m) => m.control(&mut self.uart, mcr, now, &mut self.link),
                Backend::NullModem(m) => m.control(mcr, &mut self.link),
            }
        }
        if !self.uart.outgoing.is_empty() {
            let bytes = std::mem::take(&mut self.uart.outgoing);
            match &mut self.backend {
                Backend::Empty | Backend::Mouse(_) => {}
                Backend::Modem(m) => m.transmit(&mut self.uart, &bytes, now, &mut self.link),
                Backend::NullModem(m) => m.transmit(&bytes, &mut self.link),
            }
        }
    }

    /// Something from the network for the backend.
    pub fn link_event(&mut self, event: LinkEvent, now: u64) {
        match &mut self.backend {
            Backend::Empty | Backend::Mouse(_) => {}
            Backend::Modem(m) => m.event(&mut self.uart, event, now, &mut self.link),
            Backend::NullModem(m) => m.event(&mut self.uart, event, now, &mut self.link),
        }
    }

    /// The next PIT tick the port or its backend needs attention at.
    pub fn next_event(&self) -> Option<u64> {
        let backend = match &self.backend {
            Backend::Modem(m) => m.next_event(),
            _ => None,
        };
        [self.uart.next_event(), backend].into_iter().flatten().min()
    }

    /// Bring the port and its backend up to `now`.
    pub fn advance(&mut self, now: u64) {
        self.uart.advance(now);
        if let Backend::Modem(m) = &mut self.backend {
            m.advance(&mut self.uart, now, &mut self.link);
        }
        self.sync(now);
    }

    /// Whether the backend is a mouse with power: a driver uses it.
    pub fn mouse_in_use(&self) -> bool {
        matches!(&self.backend, Backend::Mouse(m) if m.powered)
    }
}

impl State for SerialPort {
    fn save(&self, w: &mut Writer) {
        self.uart.save(w);
        self.backend.kind().save(w);
        match &self.backend {
            Backend::Empty => {}
            Backend::Mouse(m) => m.save(w),
            Backend::Modem(m) => m.save(w),
            Backend::NullModem(m) => m.save(w),
        }
        self.mcr.save(w);
    }

    fn load(&mut self, r: &mut Reader) -> Result<()> {
        self.uart.load(r)?;
        let mut kind = PortType::Empty;
        kind.load(r)?;
        if kind != self.backend.kind() {
            self.backend = Backend::new(kind, &SerialSettings::default());
        }
        match &mut self.backend {
            Backend::Empty => {}
            Backend::Mouse(m) => m.load(r)?,
            Backend::Modem(m) => m.load(r)?,
            Backend::NullModem(m) => m.load(r)?,
        }
        self.mcr.load(r)?;
        self.link.clear();
        Ok(())
    }
}

impl Default for SerialPort {
    fn default() -> Self {
        Self::new(0, PortType::Empty, &SerialSettings::default())
    }
}

/// The serial ports.
#[derive(Default)]
pub struct Serial {
    pub settings: SerialSettings,
    pub ports: [Option<SerialPort>; 4],
    /// The IRQ lines (bit n for IRQ n) the ports hold up, as the PICs last
    /// heard of them.
    pub pic_lines: u16,
}

impl Serial {
    /// Whether any port is there.
    pub fn any(&self) -> bool {
        self.ports.iter().any(|p| p.is_some())
    }

    /// The number of ports the BIOS lists (in its data area, without
    /// gaps): those from COM1 on up to the first missing one.
    pub fn bios_ports(&self) -> Vec<u16> {
        self.ports.iter().map_while(|p| p.as_ref().map(|p| p.uart.base)).collect()
    }

    /// Whether a serial mouse has power: a driver uses it.
    pub fn mouse_in_use(&self) -> bool {
        self.ports.iter().flatten().any(|p| p.mouse_in_use())
    }
}

/// The ports and their IRQ lines. A state's ports replace the machine's:
/// loading makes those it has and the machine hasn't.
impl State for Serial {
    fn save(&self, w: &mut Writer) {
        self.ports.save(w);
        self.pic_lines.save(w);
    }

    fn load(&mut self, r: &mut Reader) -> Result<()> {
        self.ports.load(r)?;
        self.pic_lines.load(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip() {
        let mut s = SerialSettings::default();
        s.set("serial3", "nullmodem").unwrap();
        s.set("Serial3IRQ", "5").unwrap();
        s.set("uart", "8250").unwrap();
        s.set("mousetype", "logitech").unwrap();
        s.set("modemlisten", "5000").unwrap();
        s.set("modemtelnet", "on").unwrap();
        assert!(s.set("serial5", "mouse").is_err());
        assert!(s.set("serial1", "printer").is_err());
        assert!(s.set("serial1irq", "2").is_err());
        let mut t = SerialSettings::default();
        for (key, value) in s.entries() {
            t.set(key, value.as_deref().unwrap()).unwrap();
        }
        assert_eq!(s, t);
        assert_eq!(t.linked_port(), Some(1));
    }
}
