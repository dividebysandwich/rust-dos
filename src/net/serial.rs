//! The serial ports' side of the network: the cable or modem of the port
//! that goes to another machine, its commands to the network thread, and
//! what comes back for it.

use super::Net;
use crate::serial::{LinkCmd, LinkEvent};
use std::collections::VecDeque;

/// What the network keeps for the serial ports.
#[derive(Default)]
pub struct SerialNet {
    /// The port that goes to the other player in a LAN room.
    pub linked: Option<usize>,
    /// The TCP port the modem takes calls on, and whether its calls speak
    /// telnet.
    pub listen: Option<u16>,
    pub telnet: bool,
    /// Events for the ports, with the port's index.
    events: VecDeque<(usize, LinkEvent)>,
}

impl Net {
    /// The serial settings the network needs.
    pub fn set_serial(&mut self, linked: Option<usize>, listen: Option<u16>, telnet: bool) {
        self.serial.linked = linked;
        self.serial.listen = listen;
        self.serial.telnet = telnet;
    }

    /// A command of port `n`'s cable or modem.
    pub fn serial_command(&mut self, n: usize, command: LinkCmd) {
        if let LinkCmd::Dial(_) = command {
            self.serial.events.push_back((n, LinkEvent::NoCarrier));
        }
    }

    /// The link's state, for the debugger.
    pub fn serial_status(&self) -> String {
        match self.serial.linked {
            Some(n) => format!("COM{} links to the room", n + 1),
            None => "no port links".to_string(),
        }
    }

    /// What came for the ports since the last time.
    pub fn serial_events(&mut self) -> Vec<(usize, LinkEvent)> {
        self.serial.events.drain(..).collect()
    }
}
