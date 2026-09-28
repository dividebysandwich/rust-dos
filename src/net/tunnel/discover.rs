//! Finding relays and their rooms. DISCOVER is broadcast to the relay port
//! (and sent to this machine, whose own broadcasts some systems don't loop
//! back), and each relay that hears it answers with an OFFER. LIST asks one
//! relay for its rooms, a page at a time.

use super::wire::{self, DEFAULT_PORT, DecodeError, Message, Packet, RoomInfo};
use crate::net::RoomList;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

/// How long looking for relays on the LAN listens for them.
pub const DISCOVER_WAIT: Duration = Duration::from_millis(1500);
/// How long a page of rooms is waited for, asked for this many times.
const PAGE_WAIT: Duration = Duration::from_millis(1500);
const PAGE_TRIES: u32 = 3;
/// The most pages of rooms asked for, some 250 rooms or more: a search
/// finds the others.
const MAX_PAGES: usize = 8;

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

/// `host`, `host:port` or an address, with the relay port unless given.
pub fn split_relay(relay: &str) -> (String, u16) {
    let relay = relay.trim();
    if let Ok(addr) = relay.parse::<SocketAddr>() {
        return (addr.ip().to_string(), addr.port());
    }
    if let Some((host, port)) = relay.rsplit_once(':')
        && !host.contains(':')
        && let Ok(port) = port.parse()
    {
        return (host.trim_matches(['[', ']']).to_string(), port);
    }
    (relay.trim_matches(['[', ']']).to_string(), DEFAULT_PORT)
}

/// Where the relay `relay` (`host[:port]`) is, or with None, the first
/// relay that answers on the LAN. An IPv4 address first: a relay's IPv6
/// address may not be reachable.
pub fn find(relay: Option<&str>) -> Result<SocketAddr, String> {
    let Some(relay) = relay else {
        let found =
            discover(DEFAULT_PORT, DISCOVER_WAIT).map_err(|e| format!("can't look for relays on the LAN: {}", e))?;
        return found.first().map(|f| f.relay).ok_or_else(|| "no relay answered on the LAN".to_string());
    };
    let (host, port) = split_relay(relay);
    let mut addrs: Vec<SocketAddr> =
        (host.as_str(), port).to_socket_addrs().map_err(|e| format!("can't find {}: {}", host, e))?.collect();
    addrs.sort_by_key(|a| !a.is_ipv4());
    addrs.first().copied().ok_or_else(|| format!("{} has no address", host))
}

/// The rooms whose names contain `filter` at the relay `relay`, as `find`
/// has it.
pub fn rooms(relay: Option<&str>, filter: &str) -> Result<RoomList, String> {
    list(find(relay)?, filter)
}

/// The rooms whose names contain `filter` at the relay at `relay`, asked
/// for a page at a time until the relay has no more or `MAX_PAGES`.
pub fn list(relay: SocketAddr, filter: &str) -> Result<RoomList, String> {
    let local: SocketAddr =
        if relay.is_ipv4() { (Ipv4Addr::UNSPECIFIED, 0).into() } else { (Ipv6Addr::UNSPECIFIED, 0).into() };
    let socket = UdpSocket::bind(local).map_err(|e| format!("can't open a UDP socket: {}", e))?;
    let mut list = RoomList { relay, name: String::new(), password: false, rooms: Vec::new(), total: 0 };
    let mut buf = [0; 2048];
    for page in 0..MAX_PAGES {
        let start = list.rooms.len().min(u16::MAX as usize) as u16;
        let request = wire::encode(0, &Message::List { start, filter: filter.to_string() });
        let mut answer = None;
        'tries: for _ in 0..PAGE_TRIES {
            socket.send_to(&request, relay).map_err(|e| format!("can't reach the relay at {}: {}", relay, e))?;
            let deadline = Instant::now() + PAGE_WAIT / PAGE_TRIES;
            loop {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                socket.set_read_timeout(Some(left)).map_err(|e| e.to_string())?;
                // An error is a timeout, or (on Windows) the relay's port
                // being closed.
                let Ok((len, from)) = socket.recv_from(&mut buf) else { continue };
                if from != relay {
                    continue;
                }
                match wire::decode(&buf[..len]) {
                    Ok(Packet { message: Message::Rooms { name, password, total, start: at, rooms }, .. })
                        if at == start =>
                    {
                        answer = Some((name, password, total, rooms));
                        break 'tries;
                    }
                    Ok(Packet { message: Message::Reject { reason }, .. }) => return Err(reason.describe().into()),
                    Err(DecodeError::Version(_)) => return Err("the relay runs another version of rust-dos".into()),
                    _ => {}
                }
            }
        }
        let Some((name, password, total, rooms)) = answer else {
            if page == 0 {
                return Err(format!("the relay at {} doesn't answer", relay));
            }
            // The pages that came are something.
            break;
        };
        (list.name, list.password, list.total) = (name, password, total as usize);
        let before = list.rooms.len();
        for room in rooms {
            // Rooms may move up a page between two.
            if !list.rooms.iter().any(|r| r.name == room.name) {
                list.rooms.push(room);
            }
        }
        if list.rooms.len() >= list.total || list.rooms.len() == before {
            break;
        }
    }
    list.rooms.sort_by(|a, b| b.members.cmp(&a.members).then_with(|| a.name.cmp(&b.name)));
    Ok(list)
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

    #[test]
    fn splits_relay_addresses() {
        assert_eq!(split_relay("relay.example.com"), ("relay.example.com".into(), DEFAULT_PORT));
        assert_eq!(split_relay("relay.example.com:4000"), ("relay.example.com".into(), 4000));
        assert_eq!(split_relay(" 192.0.2.1:5 "), ("192.0.2.1".into(), 5));
        assert_eq!(split_relay("192.0.2.1"), ("192.0.2.1".into(), DEFAULT_PORT));
        assert_eq!(split_relay("[2001:db8::1]:7"), ("2001:db8::1".into(), 7));
        assert_eq!(split_relay("2001:db8::1"), ("2001:db8::1".into(), DEFAULT_PORT));
    }

    #[test]
    fn lists_the_rooms_of_a_relay() {
        let config = RelayConfig { name: "den".into(), password: None, port: 0 };
        let server = RelayServer::start("127.0.0.1:0".parse().unwrap(), config, Box::new(|_| {})).unwrap();
        let relay = server.local_addr();
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut buf = [0; 2048];
        // Enough rooms for several pages, one of them with a password.
        for i in 0..70u64 {
            let room = format!("a room with a long name {:02}", i);
            socket.send_to(&wire::encode(0, &Message::Hello { client_id: i, room: room.clone() }), relay).unwrap();
            let (len, _) = socket.recv_from(&mut buf).unwrap();
            let Message::Challenge { cookie, .. } = wire::decode(&buf[..len]).unwrap().message else { panic!() };
            let key = crate::net::tunnel::auth::room_key((i == 5).then_some("pw"), &room);
            let proof = crate::net::tunnel::auth::proof(&key, &room, i, &cookie);
            let join = Message::Join { client_id: i, cookie, room, proof, key };
            socket.send_to(&wire::encode(0, &join), relay).unwrap();
            let (len, _) = socket.recv_from(&mut buf).unwrap();
            assert!(matches!(wire::decode(&buf[..len]).unwrap().message, Message::Welcome { .. }));
        }
        let all = rooms(Some(&relay.to_string()), "").unwrap();
        assert_eq!((all.relay, all.name.as_str(), all.password, all.total), (relay, "den", false, 70));
        assert_eq!(all.rooms.len(), 70);
        assert!(all.rooms.iter().find(|r| r.name.ends_with(" 05")).unwrap().password);
        let some = list(relay, "NAME 1").unwrap();
        assert_eq!((some.total, some.rooms.len()), (10, 10));
        // Nothing listens there any more.
        drop(server);
        assert!(list(relay, "").unwrap_err().contains("doesn't answer"));
    }
}
