//! The network card's router to the internet (net/nat): ARP, DHCP, ping,
//! DNS, UDP and TCP as a guest uses them, with the test standing in for
//! the host's sockets and resolver. The guest's TCP is a second smoltcp
//! stack, on 10.0.2.15, talking to the router in Ethernet frames.

use rust_dos::net::frame::{self, ETHERTYPE_ARP, ETHERTYPE_IPV4, Mac};
use rust_dos::net::nat::packet::{self, PROTO_ICMP, PROTO_UDP};
use rust_dos::net::nat::{Action, Event, GATEWAY, GUEST, NAMESERVER, ROUTER_MAC, Router};
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::tcp;
use smoltcp::time::Instant;
use smoltcp::wire::{HardwareAddress, IpAddress, IpCidr};
use std::collections::VecDeque;
use std::net::{Ipv4Addr, SocketAddr};

const GUEST_MAC: Mac = Mac([0x02, 0x00, 0x00, 0x00, 0x00, 0x15]);

/// Frames the router sent the guest, and the actions for the host.
fn split(actions: Vec<Action>) -> (Vec<Vec<u8>>, Vec<Action>) {
    let mut frames = Vec::new();
    let mut rest = Vec::new();
    for action in actions {
        match action {
            Action::ToGuest(frame) => frames.push(frame),
            action => rest.push(action),
        }
    }
    (frames, rest)
}

/// The IPv4 packets in `frames`.
fn ip_packets(frames: &[Vec<u8>]) -> Vec<Vec<u8>> {
    frames
        .iter()
        .filter(|f| frame::ethertype(f) == Some(ETHERTYPE_IPV4))
        .map(|f| {
            let ip = &f[frame::HEADER..];
            let len = packet::parse_ipv4(ip).unwrap().total_len;
            ip[..len].to_vec()
        })
        .collect()
}

fn from_guest(router: &mut Router, kind: u16, payload: &[u8], destination: Mac) -> (Vec<Vec<u8>>, Vec<Action>) {
    router.from_guest(&frame::build(destination, GUEST_MAC, kind, payload), 0);
    split(router.take_actions())
}

#[test]
fn answers_arp_and_pings_for_its_addresses() {
    let mut router = Router::new(0);
    let mut arp = vec![0, 1, 8, 0, 6, 4, 0, 1];
    arp.extend_from_slice(&GUEST_MAC.0);
    arp.extend_from_slice(&GUEST.octets());
    arp.extend_from_slice(&[0; 6]);
    arp.extend_from_slice(&GATEWAY.octets());
    let (frames, _) = from_guest(&mut router, ETHERTYPE_ARP, &arp, Mac::BROADCAST);
    assert_eq!(frames.len(), 1);
    assert_eq!(frame::destination(&frames[0]), Some(GUEST_MAC));
    assert_eq!(&frames[0][frame::HEADER + 8..frame::HEADER + 14], &ROUTER_MAC.0);
    // Not for another address on the network.
    arp[24..28].copy_from_slice(&[10, 0, 2, 99]);
    assert!(from_guest(&mut router, ETHERTYPE_ARP, &arp, Mac::BROADCAST).0.is_empty());

    let mut echo = vec![8, 0, 0, 0, 0, 1, 0, 1, 1, 2, 3, 4];
    let sum = packet::checksum(&echo, 0);
    echo[2..4].copy_from_slice(&sum.to_be_bytes());
    let ping = packet::build_ipv4(GUEST, NAMESERVER, PROTO_ICMP, &echo);
    let (frames, _) = from_guest(&mut router, ETHERTYPE_IPV4, &ping, ROUTER_MAC);
    let reply = &ip_packets(&frames)[0];
    let header = packet::parse_ipv4(reply).unwrap();
    assert_eq!((header.src, header.dst), (NAMESERVER, GUEST));
    assert_eq!(reply[20], 0, "an echo reply");
}

#[test]
fn hands_out_the_guest_address_by_dhcp() {
    let mut router = Router::new(0);
    let mut discover = vec![0u8; 240];
    discover[0..3].copy_from_slice(&[1, 1, 6]);
    discover[4..8].copy_from_slice(&[1, 2, 3, 4]);
    discover[28..34].copy_from_slice(&GUEST_MAC.0);
    discover[236..240].copy_from_slice(&[99, 130, 83, 99]);
    discover.extend_from_slice(&[53, 1, 1, 255]);
    let ip = packet::build_udp(Ipv4Addr::UNSPECIFIED, 68, Ipv4Addr::BROADCAST, 67, &discover);
    let (frames, _) = from_guest(&mut router, ETHERTYPE_IPV4, &ip, Mac::BROADCAST);
    let offer = &ip_packets(&frames)[0];
    let udp = packet::parse_udp(&offer[20..]).unwrap();
    assert_eq!((udp.src_port, udp.dst_port), (67, 68));
    assert_eq!(&udp.payload[16..20], &GUEST.octets(), "yiaddr");
    assert_eq!(&udp.payload[4..8], &[1, 2, 3, 4], "xid");
}

#[test]
fn answers_names_through_the_host() {
    let mut router = Router::new(0);
    let mut query = vec![0xAB, 0xCD, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0];
    query.extend_from_slice(b"\x07example\x03com\x00\x00\x01\x00\x01");
    let ip = packet::build_udp(GUEST, 1053, NAMESERVER, 53, &query);
    let (frames, actions) = from_guest(&mut router, ETHERTYPE_IPV4, &ip, ROUTER_MAC);
    assert!(frames.is_empty());
    let [Action::Resolve { token, name }] = &actions[..] else { panic!("{:?}", actions) };
    assert_eq!(name, "example.com");
    router.event(Event::Resolved { token: *token, addresses: vec![Ipv4Addr::new(93, 184, 216, 34)], found: true }, 0);
    let (frames, _) = split(router.take_actions());
    let answer = &ip_packets(&frames)[0];
    let udp = packet::parse_udp(&answer[20..]).unwrap();
    assert_eq!((udp.src_port, udp.dst_port), (53, 1053));
    assert_eq!(&udp.payload[0..2], &[0xAB, 0xCD]);
    assert_eq!(&udp.payload[udp.payload.len() - 4..], &[93, 184, 216, 34]);
}

#[test]
fn takes_udp_to_the_host_and_back() {
    let mut router = Router::new(0);
    let ip = packet::build_udp(GUEST, 5000, Ipv4Addr::new(8, 8, 8, 8), 53, b"question");
    let (_, actions) = from_guest(&mut router, ETHERTYPE_IPV4, &ip, ROUTER_MAC);
    let [Action::UdpOpen { id }, Action::UdpSend { id: sent_on, to, data }] = &actions[..] else {
        panic!("{:?}", actions)
    };
    assert_eq!((id, to, &data[..]), (sent_on, &"8.8.8.8:53".parse::<SocketAddr>().unwrap(), &b"question"[..]));
    // The router's address is the host.
    let ip = packet::build_udp(GUEST, 5000, GATEWAY, 7, b"x");
    let (_, actions) = from_guest(&mut router, ETHERTYPE_IPV4, &ip, ROUTER_MAC);
    assert!(matches!(&actions[..], [Action::UdpSend { to, .. }] if to.to_string() == "127.0.0.1:7"), "{:?}", actions);
    router.event(Event::UdpData { id: *id, from: "8.8.8.8:53".parse().unwrap(), data: b"answer".to_vec() }, 0);
    let (frames, _) = split(router.take_actions());
    let reply = &ip_packets(&frames)[0];
    let header = packet::parse_ipv4(reply).unwrap();
    assert_eq!((header.src, header.dst, header.protocol), (Ipv4Addr::new(8, 8, 8, 8), GUEST, PROTO_UDP));
    assert_eq!(packet::parse_udp(&reply[20..]).unwrap().payload, b"answer");
    // Idle flows close.
    router.poll(61_000);
    assert!(router.take_actions().contains(&Action::UdpClose { id: *id }));
}

/// The guest's side: IP packets, which the test carries to and from the
/// router in Ethernet frames.
#[derive(Default)]
struct GuestLink {
    rx: VecDeque<Vec<u8>>,
    tx: Vec<Vec<u8>>,
}

struct Rx(Vec<u8>);
impl RxToken for Rx {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(&self.0)
    }
}
struct Tx<'a>(&'a mut Vec<Vec<u8>>);
impl TxToken for Tx<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut p = vec![0; len];
        let r = f(&mut p);
        self.0.push(p);
        r
    }
}
impl Device for GuestLink {
    type RxToken<'a> = Rx;
    type TxToken<'a> = Tx<'a>;
    fn receive(&mut self, _: Instant) -> Option<(Rx, Tx<'_>)> {
        Some((Rx(self.rx.pop_front()?), Tx(&mut self.tx)))
    }
    fn transmit(&mut self, _: Instant) -> Option<Tx<'_>> {
        Some(Tx(&mut self.tx))
    }
    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ip;
        caps.max_transmission_unit = 1500;
        caps
    }
}

struct Guest {
    iface: Interface,
    link: GuestLink,
    sockets: SocketSet<'static>,
}

impl Guest {
    fn new() -> Self {
        let mut link = GuestLink::default();
        let mut iface = Interface::new(Config::new(HardwareAddress::Ip), &mut link, Instant::from_millis(0));
        iface.update_ip_addrs(|a| {
            let _ = a.push(IpCidr::new(IpAddress::Ipv4(GUEST), 24));
        });
        iface.routes_mut().add_default_ipv4_route(GATEWAY).unwrap();
        Self { iface, link, sockets: SocketSet::new(Vec::new()) }
    }

    fn socket(&mut self) -> SocketHandle {
        let s = tcp::Socket::new(tcp::SocketBuffer::new(vec![0; 8192]), tcp::SocketBuffer::new(vec![0; 8192]));
        self.sockets.add(s)
    }

    fn tcp(&mut self, h: SocketHandle) -> &mut tcp::Socket<'static> {
        self.sockets.get_mut::<tcp::Socket>(h)
    }

    /// Run the guest and the router until the frames stop, at `now`;
    /// the actions for the host.
    fn exchange(&mut self, router: &mut Router, now: u64) -> Vec<Action> {
        let mut host = Vec::new();
        for _ in 0..20 {
            self.iface.poll(Instant::from_millis(now as i64), &mut self.link, &mut self.sockets);
            let sent = std::mem::take(&mut self.link.tx);
            for ip in &sent {
                router.from_guest(&frame::build(ROUTER_MAC, GUEST_MAC, ETHERTYPE_IPV4, ip), now);
            }
            router.poll(now);
            let (frames, actions) = split(router.take_actions());
            host.extend(actions);
            let packets = ip_packets(&frames);
            if sent.is_empty() && packets.is_empty() {
                break;
            }
            self.link.rx.extend(packets);
        }
        host
    }
}

fn connect_flow(actions: &[Action]) -> (u64, SocketAddr) {
    actions
        .iter()
        .find_map(|a| match a {
            Action::Connect { flow, to } => Some((*flow, *to)),
            _ => None,
        })
        .expect("a connection to make")
}

#[test]
fn carries_a_tcp_connection_both_ways() {
    let mut router = Router::new(0);
    let mut guest = Guest::new();
    let h = guest.socket();
    let cx = guest.iface.context();
    guest.sockets.get_mut::<tcp::Socket>(h).connect(cx, (Ipv4Addr::new(93, 184, 216, 34), 80), 49152).unwrap();
    let actions = guest.exchange(&mut router, 0);
    let (flow, to) = connect_flow(&actions);
    assert_eq!(to.to_string(), "93.184.216.34:80");
    assert_eq!(guest.tcp(h).state(), tcp::State::SynSent, "not answered before the host's connection is made");

    router.event(Event::Connected { flow }, 10);
    guest.exchange(&mut router, 10);
    assert_eq!(guest.tcp(h).state(), tcp::State::Established);

    // Guest to host.
    guest.tcp(h).send_slice(b"GET / HTTP/1.0\r\n\r\n").unwrap();
    let actions = guest.exchange(&mut router, 20);
    let sent: Vec<u8> = actions
        .iter()
        .filter_map(|a| match a {
            Action::Send { flow: f, data } if *f == flow => Some(data.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(sent, b"GET / HTTP/1.0\r\n\r\n");

    // Host to guest, more than one window, then the host's end.
    let body: Vec<u8> = (0..20_000).map(|i| (i % 251) as u8).collect();
    router.event(Event::Data { flow, data: body.clone() }, 30);
    router.event(Event::Eof { flow }, 30);
    let mut got = Vec::new();
    let mut credit = 0;
    for t in 0..200 {
        let actions = guest.exchange(&mut router, 40 + t * 50);
        credit += actions
            .iter()
            .filter_map(|a| if let Action::Credit { bytes, .. } = a { Some(*bytes) } else { None })
            .sum::<usize>();
        let mut buf = vec![0; 4096];
        while let Ok(n) = guest.tcp(h).recv_slice(&mut buf) {
            if n == 0 {
                break;
            }
            got.extend_from_slice(&buf[..n]);
        }
        if got.len() == body.len() && !guest.tcp(h).may_recv() {
            break;
        }
    }
    assert_eq!(got.len(), body.len());
    assert_eq!(got, body);
    assert_eq!(credit, body.len(), "the host is told what the guest took");
    assert!(!guest.tcp(h).may_recv(), "the host's end reached the guest");

    // The guest closes too: the connection ends on both sides.
    guest.tcp(h).close();
    let mut actions = Vec::new();
    for t in 0..20 {
        actions.extend(guest.exchange(&mut router, 20_000 + t * 100));
    }
    assert!(actions.contains(&Action::Shutdown { flow }), "{:?}", actions);
    assert!(actions.contains(&Action::Close { flow }), "{:?}", actions);
}

#[test]
fn refuses_what_the_host_cant_connect_to() {
    let mut router = Router::new(0);
    let mut guest = Guest::new();
    let h = guest.socket();
    let cx = guest.iface.context();
    guest.sockets.get_mut::<tcp::Socket>(h).connect(cx, (GATEWAY, 1), 49153).unwrap();
    let actions = guest.exchange(&mut router, 0);
    let (flow, to) = connect_flow(&actions);
    assert_eq!(to.to_string(), "127.0.0.1:1", "the router's address is the host");
    router.event(Event::ConnectFailed { flow }, 10);
    guest.exchange(&mut router, 10);
    assert_eq!(guest.tcp(h).state(), tcp::State::Closed, "reset");
    // Another guest's address isn't the router's to reach.
    let h = guest.socket();
    let cx = guest.iface.context();
    guest.sockets.get_mut::<tcp::Socket>(h).connect(cx, (Ipv4Addr::new(10, 0, 2, 50), 80), 49154).unwrap();
    let actions = guest.exchange(&mut router, 20);
    assert!(!actions.iter().any(|a| matches!(a, Action::Connect { .. })));
}
