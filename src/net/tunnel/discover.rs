//! Finding relays on the LAN: DISCOVER is broadcast to the relay port (and
//! sent to this machine, whose own broadcasts some systems don't loop
//! back), and each relay that hears it answers with an OFFER.

use super::wire::{self, Message, RoomInfo};
use std::io;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

/// A relay that answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// Where to join it.
    pub relay: SocketAddr,
    pub name: String,
    pub password: bool,
    pub rooms: Vec<RoomInfo>,
}

/// The relays listening on `port` that answer within `wait`, in the order
/// they answered.
pub fn discover(port: u16, wait: Duration) -> io::Result<Vec<Found>> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    socket.set_broadcast(true)?;
    let discover = wire::encode(0, &Message::Discover);
    // Either may fail (no network, or no broadcast route); the other may
    // still find something.
    let sent = [Ipv4Addr::BROADCAST, Ipv4Addr::LOCALHOST]
        .into_iter()
        .filter(|ip| socket.send_to(&discover, (*ip, port)).is_ok())
        .count();
    if sent == 0 {
        return Err(io::Error::other("can't send on the LAN"));
    }
    let deadline = Instant::now() + wait;
    let mut found: Vec<Found> = Vec::new();
    let mut buf = [0; 2048];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        socket.set_read_timeout(Some(left))?;
        let Ok((len, from)) = socket.recv_from(&mut buf) else { continue };
        let Ok(packet) = wire::decode(&buf[..len]) else { continue };
        let Message::Offer { port, password, name, rooms } = packet.message else { continue };
        let relay = SocketAddr::new(from.ip(), if port == 0 { from.port() } else { port });
        // A relay on this machine answers both on the loopback and on the
        // LAN; one answer is enough.
        if !found.iter().any(|f| f.relay.port() == relay.port() && f.name == name && same_host(f.relay, relay)) {
            found.push(Found { relay, name, password, rooms });
        }
    }
    Ok(found)
}

/// Whether two answers came from one host: the same address, or this
/// machine by the loopback and by a LAN address.
fn same_host(a: SocketAddr, b: SocketAddr) -> bool {
    a.ip() == b.ip() || a.ip().is_loopback() || b.ip().is_loopback()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::tunnel::relay::{RelayConfig, RelayServer};

    #[test]
    fn finds_a_relay_on_this_machine() {
        let config = RelayConfig { name: "den".into(), password: Some("pw".into()), port: 0 };
        let server = RelayServer::start((Ipv4Addr::UNSPECIFIED, 0).into(), config, Box::new(|_| {})).unwrap();
        let port = server.local_addr().port();
        let found = discover(port, Duration::from_millis(500)).unwrap();
        assert_eq!(found.len(), 1, "{:?}", found);
        assert_eq!(found[0].relay.port(), port);
        assert_eq!((found[0].name.as_str(), found[0].password), ("den", true));
    }
}
