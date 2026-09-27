//! The Ethernet switch inside each instance, with a port for each of:
//! the network card's guest (`Nic`), the built-in DOS's IPX driver
//! (`Ipx`), the router that takes the card's guest to the internet
//! (`Router`), and the LAN tunnel (`Uplink`).
//!
//! The router serves only this instance's card: the frames of other
//! instances never reach it, so every instance can have a router at the
//! same addresses without them meeting on the LAN.

use super::frame::{self, Mac};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Port {
    Nic,
    Ipx,
    Router,
    Uplink,
}

/// The ports that are there, with the local ones' addresses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Switch {
    pub nic: Option<Mac>,
    pub ipx: Option<Mac>,
    pub router: Option<Mac>,
    pub uplink: bool,
}

impl Switch {
    /// The ports a frame that came in at `from` goes out at.
    pub fn route(&self, from: Port, frame: &[u8]) -> Vec<Port> {
        let Some(destination) = frame::destination(frame) else { return Vec::new() };
        let group = destination.is_group();
        let mut to = Vec::new();
        let mut add = |port: Port, present: bool| {
            if present && port != from {
                to.push(port);
            }
        };
        match from {
            Port::Nic | Port::Ipx => {
                let local = [(Port::Nic, self.nic), (Port::Ipx, self.ipx)];
                if group {
                    add(Port::Router, from == Port::Nic && self.router.is_some());
                    for (port, mac) in local {
                        add(port, mac.is_some());
                    }
                    add(Port::Uplink, self.uplink);
                } else if from == Port::Nic && self.router == Some(destination) {
                    add(Port::Router, true);
                } else if let Some((port, _)) = local.iter().find(|(_, mac)| *mac == Some(destination)) {
                    add(*port, true);
                } else {
                    add(Port::Uplink, self.uplink);
                }
            }
            // The card decides what to take by its own address filter
            // (a card in promiscuous mode takes everything).
            Port::Uplink => {
                add(Port::Nic, self.nic.is_some());
                add(Port::Ipx, self.ipx.is_some_and(|mac| group || mac == destination));
            }
            Port::Router => add(Port::Nic, self.nic.is_some()),
        }
        to
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::frame::{ETHERTYPE_IPV4, build};

    const NIC: Mac = Mac([2, 0, 0, 0, 0, 1]);
    const IPX: Mac = Mac([2, 0, 0, 0, 0, 2]);
    const ROUTER: Mac = Mac([0x52, 0x54, 0, 0x12, 0x35, 2]);
    const OTHER: Mac = Mac([2, 0, 0, 0, 0, 9]);

    fn to(destination: Mac, source: Mac) -> Vec<u8> {
        build(destination, source, ETHERTYPE_IPV4, &[])
    }

    #[test]
    fn keeps_the_router_to_its_own_card() {
        let s = Switch { nic: Some(NIC), ipx: Some(IPX), router: Some(ROUTER), uplink: true };
        use Port::*;
        assert_eq!(s.route(Nic, &to(Mac::BROADCAST, NIC)), vec![Router, Ipx, Uplink]);
        assert_eq!(s.route(Ipx, &to(Mac::BROADCAST, IPX)), vec![Nic, Uplink]);
        assert_eq!(s.route(Nic, &to(ROUTER, NIC)), vec![Router]);
        assert_eq!(s.route(Ipx, &to(ROUTER, IPX)), vec![Uplink], "not the IPX driver's router");
        assert_eq!(s.route(Nic, &to(IPX, NIC)), vec![Ipx]);
        assert_eq!(s.route(Ipx, &to(NIC, IPX)), vec![Nic]);
        assert_eq!(s.route(Nic, &to(OTHER, NIC)), vec![Uplink]);
        assert_eq!(s.route(Uplink, &to(Mac::BROADCAST, OTHER)), vec![Nic, Ipx]);
        assert_eq!(s.route(Uplink, &to(IPX, OTHER)), vec![Nic, Ipx]);
        assert_eq!(s.route(Uplink, &to(NIC, OTHER)), vec![Nic]);
        assert_eq!(s.route(Uplink, &to(ROUTER, OTHER)), vec![Nic], "never the router");
        assert_eq!(s.route(Router, &to(NIC, ROUTER)), vec![Nic]);
        assert_eq!(s.route(Nic, &to(NIC, NIC)), Vec::<Port>::new());
    }

    #[test]
    fn leaves_out_missing_ports() {
        let s = Switch { nic: None, ipx: Some(IPX), router: None, uplink: false };
        assert!(s.route(Port::Ipx, &to(Mac::BROADCAST, IPX)).is_empty());
        assert!(s.route(Port::Ipx, &to(OTHER, IPX)).is_empty());
        assert!(s.route(Port::Ipx, &[0; 4]).is_empty());
    }
}
