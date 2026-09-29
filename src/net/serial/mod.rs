//! The serial ports' side of the network: the cable or modem of the port
//! that goes to the other player in a LAN room, and modems' calls over
//! TCP. The emulator's side (`Net`'s methods here) passes the ports'
//! `LinkCmd`s to the network thread, gathering the characters a port sends
//! in a millisecond into one command, and takes the `LinkEvent`s that come
//! back from a `SerialQueue`; the network thread's side is `station`.

#[cfg(not(target_arch = "wasm32"))]
pub mod link;
#[cfg(not(target_arch = "wasm32"))]
pub mod station;
#[cfg(not(target_arch = "wasm32"))]
pub mod tcp;

use super::Net;
use crate::serial::{LinkCmd, LinkEvent, SerialSettings};
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// Events for the ports, from the network thread.
#[derive(Default)]
pub struct SerialQueue {
    events: Mutex<VecDeque<(usize, LinkEvent)>>,
    pending: AtomicBool,
}

impl SerialQueue {
    pub fn push(&self, port: usize, event: LinkEvent) {
        if let Ok(mut events) = self.events.lock() {
            events.push_back((port, event));
            self.pending.store(true, Ordering::Release);
        }
    }

    pub fn take(&self) -> Vec<(usize, LinkEvent)> {
        if !self.pending.swap(false, Ordering::Acquire) {
            return Vec::new();
        }
        self.events.lock().map(|mut e| e.drain(..).collect()).unwrap_or_default()
    }
}

/// What the emulator tells the network thread about serial ports.
#[cfg(not(target_arch = "wasm32"))]
pub enum SerialCommand {
    /// The ports as the settings have them.
    Setup(station::Setup, std::sync::Arc<SerialQueue>),
    /// A command of a port's cable or modem.
    Port(usize, LinkCmd),
    /// What happened to a modem's TCP call.
    Tcp(tcp::TcpEvent),
    /// The modem couldn't take calls on its port.
    ListenFailed(String),
}

/// What the network keeps for the serial ports.
#[derive(Default)]
pub struct SerialNet {
    pub settings: SerialSettings,
    pub queue: std::sync::Arc<SerialQueue>,
    /// The characters each port sent since the last poll.
    bytes: [Vec<u8>; 4],
    /// Events that didn't go through the network thread.
    events: VecDeque<(usize, LinkEvent)>,
}

impl SerialNet {
    /// The ports the network thread needs to know of.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn setup(&self) -> station::Setup {
        let s = &self.settings;
        station::Setup {
            linked: s.linked_port(),
            modem: s.ports.iter().position(|p| *p == crate::serial::PortType::Modem),
            listen: s.modem_listen,
            telnet: s.modem_telnet,
        }
    }
}

impl Net {
    /// The serial settings the network needs: a modem that takes calls
    /// starts the network thread.
    pub fn set_serial(&mut self, settings: &SerialSettings) {
        self.serial.settings = settings.clone();
        #[cfg(not(target_arch = "wasm32"))]
        {
            let listens = settings.modem_listen.is_some() && settings.ports.contains(&crate::serial::PortType::Modem);
            if listens
                && self.hub.is_none()
                && let Err(e) = self.hub()
            {
                self.notices.push(e);
            }
            self.send_serial_setup();
        }
    }

    /// Tell the network thread the ports, if it runs.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn send_serial_setup(&self) {
        if let Some(hub) = &self.hub {
            let setup = self.serial.setup();
            hub.send(super::hub::Command::Serial(SerialCommand::Setup(setup, self.serial.queue.clone())));
        }
    }

    /// A command of port `n`'s cable or modem. Characters wait for
    /// `serial_flush`; everything else goes after the characters before it.
    pub fn serial_command(&mut self, n: usize, command: LinkCmd) {
        if n >= 4 {
            return;
        }
        if let LinkCmd::Bytes(bytes) = command {
            if self.serial.bytes[n].len() < 1024 * 1024 {
                self.serial.bytes[n].extend_from_slice(&bytes);
            }
            return;
        }
        self.flush_port(n);
        #[cfg(not(target_arch = "wasm32"))]
        {
            // A call over TCP starts the network thread.
            if matches!(command, LinkCmd::Dial(Some(_)))
                && self.hub.is_none()
                && let Err(e) = self.hub()
            {
                self.notices.push(e);
            }
            if let Some(hub) = &self.hub {
                hub.send(super::hub::Command::Serial(SerialCommand::Port(n, command)));
                return;
            }
        }
        // No network: nothing answers.
        if let LinkCmd::Dial(_) = command {
            self.serial.events.push_back((n, LinkEvent::NoCarrier));
        }
    }

    /// Send the characters the ports sent since the last time.
    pub fn serial_flush(&mut self) {
        for n in 0..4 {
            self.flush_port(n);
        }
    }

    fn flush_port(&mut self, n: usize) {
        if self.serial.bytes[n].is_empty() {
            return;
        }
        let bytes = std::mem::take(&mut self.serial.bytes[n]);
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(hub) = &self.hub {
            hub.send(super::hub::Command::Serial(SerialCommand::Port(n, LinkCmd::Bytes(bytes))));
        }
        #[cfg(target_arch = "wasm32")]
        drop(bytes);
    }

    /// What came for the ports since the last time.
    pub fn serial_events(&mut self) -> Vec<(usize, LinkEvent)> {
        let mut events: Vec<_> = self.serial.events.drain(..).collect();
        events.extend(self.serial.queue.take());
        events
    }

    /// The link's state, for the debugger.
    pub fn serial_status(&self) -> serde_json::Value {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(hub) = &self.hub {
            let s = hub.status().serial;
            return serde_json::json!({
                "port": s.port.map(|p| format!("COM{}", p + 1)),
                "peer": s.peer,
                "up": s.up,
                "rtt_ms": s.rtt_ms,
                "retransmits": s.retransmits,
                "relay_serial": s.relay_serial,
                "listening": s.listening,
            });
        }
        serde_json::json!({ "port": self.serial.settings.linked_port().map(|p| format!("COM{}", p + 1)), "network": false })
    }
}
