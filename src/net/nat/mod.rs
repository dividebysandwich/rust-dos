//! The router that takes the network card's guest to the internet, as a
//! home router with NAT does (and QEMU's user networking, whose addresses
//! it has): the guest is 10.0.2.15, the router 10.0.2.2, its name server
//! 10.0.2.3. It answers ARP and pings for its own addresses, hands out the
//! guest's address by DHCP or BOOTP, answers name lookups through the
//! host's resolver, and makes the guest's TCP connections and UDP
//! datagrams the host's own. 10.0.2.2 as a destination is the host itself
//! (127.0.0.1).
//!
//! TCP ends at smoltcp's stack on the router's side of the link, which is
//! connected to a host socket the guest's connection is copied to. A new
//! connection is only answered once the host's has been made, so a
//! connection the host can't make is refused to the guest too.
//!
//! `Router` is the protocols alone: frames from the guest and events from
//! the host's sockets come in, and frames for the guest and actions for
//! the host come out (`nat::host` carries them out on the network thread).

pub mod dhcp;
pub mod dns;
pub mod host;
pub mod packet;

use super::frame::{self, ETHERTYPE_ARP, ETHERTYPE_IPV4, Mac};
use packet::{PROTO_ICMP, PROTO_TCP, PROTO_UDP};
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::tcp;
use smoltcp::time::{Duration, Instant};
use smoltcp::wire::{HardwareAddress, IpAddress, IpCidr, IpListenEndpoint};
use std::collections::{HashMap, VecDeque};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

/// The router's Ethernet address, and its addresses on the guest's
/// network.
pub const ROUTER_MAC: Mac = Mac([0x52, 0x54, 0x00, 0x12, 0x35, 0x02]);
pub const GATEWAY: Ipv4Addr = Ipv4Addr::new(10, 0, 2, 2);
pub const NAMESERVER: Ipv4Addr = Ipv4Addr::new(10, 0, 2, 3);
pub const GUEST: Ipv4Addr = Ipv4Addr::new(10, 0, 2, 15);
pub const NETMASK: Ipv4Addr = Ipv4Addr::new(255, 255, 255, 0);
/// How long the guest keeps its address before asking again: a day, as
/// home routers give, and more than the hour mTCP's programs want left.
const LEASE_SECONDS: u32 = 86_400;
/// A UDP flow nothing crossed for this long is forgotten.
const UDP_IDLE_MS: u64 = 60_000;
/// Each TCP connection's buffers on the router's side.
const TCP_BUFFER: usize = 64 * 1024;
/// How far the host's side of a connection may read ahead of the guest.
pub const TCP_CREDIT: usize = 64 * 1024;
/// A connection the guest stops answering is dropped after this long.
const TCP_TIMEOUT: Duration = Duration::from_secs(120);

/// What the host is to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// An Ethernet frame for the guest.
    ToGuest(Vec<u8>),
    /// Make TCP connection `flow` to `to`.
    Connect {
        flow: u64,
        to: SocketAddr,
    },
    /// Send what the guest sent on the connection.
    Send {
        flow: u64,
        data: Vec<u8>,
    },
    /// The guest has no more to send (it closed its side).
    Shutdown {
        flow: u64,
    },
    /// The connection is over.
    Close {
        flow: u64,
    },
    /// The host may read `bytes` more for the guest.
    Credit {
        flow: u64,
        bytes: usize,
    },
    /// Open a UDP socket for UDP flow `id`, send on it, and close it.
    UdpOpen {
        id: u64,
    },
    UdpSend {
        id: u64,
        to: SocketAddr,
        data: Vec<u8>,
    },
    UdpClose {
        id: u64,
    },
    /// Look up the IPv4 addresses of `name`.
    Resolve {
        token: u64,
        name: String,
    },
}

/// What happened on the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Connected {
        flow: u64,
    },
    ConnectFailed {
        flow: u64,
    },
    /// What came on the connection, and its end.
    Data {
        flow: u64,
        data: Vec<u8>,
    },
    Eof {
        flow: u64,
    },
    /// The connection broke.
    Failed {
        flow: u64,
    },
    UdpData {
        id: u64,
        from: SocketAddr,
        data: Vec<u8>,
    },
    /// The addresses of a name looked up; `found` false if it has none.
    Resolved {
        token: u64,
        addresses: Vec<Ipv4Addr>,
        found: bool,
    },
}

/// The link to the guest as smoltcp sees it: IP packets in, IP packets out.
#[derive(Default)]
struct IpLink {
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
        let mut packet = vec![0; len];
        let result = f(&mut packet);
        self.0.push(packet);
        result
    }
}

impl Device for IpLink {
    type RxToken<'a> = Rx;
    type TxToken<'a> = Tx<'a>;

    fn receive(&mut self, _: Instant) -> Option<(Rx, Tx<'_>)> {
        let packet = self.rx.pop_front()?;
        Some((Rx(packet), Tx(&mut self.tx)))
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

/// A guest's TCP connection: its end on the router's side, and what the
/// host sent for it that the guest hasn't taken yet.
struct Flow {
    key: (u16, SocketAddrV4),
    /// The guest's SYN, kept while the host connects.
    syn: Option<Vec<u8>>,
    handle: Option<SocketHandle>,
    to_guest: VecDeque<u8>,
    host_eof: bool,
    /// The router closed its side (after the host's end), or passed the
    /// guest's end on.
    closed: bool,
    shut: bool,
}

struct UdpFlow {
    id: u64,
    guest: SocketAddrV4,
    last: u64,
}

pub struct Router {
    guest_mac: Option<Mac>,
    /// The address the router hands the guest.
    pub guest_ip: Ipv4Addr,
    iface: Interface,
    link: IpLink,
    sockets: SocketSet<'static>,
    flows: HashMap<u64, Flow>,
    flow_ids: HashMap<(u16, SocketAddrV4), u64>,
    /// UDP flows by the guest's port, and the ports by flow.
    udp: HashMap<u16, UdpFlow>,
    udp_ports: HashMap<u64, u16>,
    lookups: HashMap<u64, (dns::Query, SocketAddrV4)>,
    next_id: u64,
    actions: Vec<Action>,
}

impl Router {
    pub fn new(now: u64) -> Self {
        let mut link = IpLink::default();
        let mut config = Config::new(HardwareAddress::Ip);
        config.random_seed = super::random_u64();
        let mut iface = Interface::new(config, &mut link, Instant::from_millis(now as i64));
        iface.update_ip_addrs(|addrs| {
            let _ = addrs.push(IpCidr::new(IpAddress::Ipv4(GATEWAY), 24));
        });
        // The router's end of every connection has the address the guest
        // connected to.
        iface.set_any_ip(true);
        Self {
            guest_mac: None,
            guest_ip: GUEST,
            iface,
            link,
            sockets: SocketSet::new(Vec::new()),
            flows: HashMap::new(),
            flow_ids: HashMap::new(),
            udp: HashMap::new(),
            udp_ports: HashMap::new(),
            lookups: HashMap::new(),
            next_id: 1,
            actions: Vec::new(),
        }
    }

    /// What the host is to do now.
    pub fn take_actions(&mut self) -> Vec<Action> {
        std::mem::take(&mut self.actions)
    }

    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// When `poll` is next due, in ms from `now`.
    pub fn poll_delay(&mut self, now: u64) -> Option<u64> {
        self.iface.poll_delay(Instant::from_millis(now as i64), &self.sockets).map(|d| d.total_millis())
    }

    /// An Ethernet frame from the guest.
    pub fn from_guest(&mut self, frame: &[u8], now: u64) {
        let (Some(destination), Some(source), Some(kind)) =
            (frame::destination(frame), frame::source(frame), frame::ethertype(frame))
        else {
            return;
        };
        if destination != ROUTER_MAC && !destination.is_broadcast() || source.is_group() {
            return;
        }
        self.guest_mac = Some(source);
        let payload = &frame[frame::HEADER..];
        match kind {
            ETHERTYPE_ARP => self.arp(payload),
            ETHERTYPE_IPV4 => self.ipv4(payload, now),
            _ => {}
        }
        self.poll(now);
    }

    fn send_guest(&mut self, kind: u16, payload: &[u8]) {
        let destination = self.guest_mac.unwrap_or(Mac::BROADCAST);
        self.actions.push(Action::ToGuest(frame::build(destination, ROUTER_MAC, kind, payload)));
    }

    fn arp(&mut self, payload: &[u8]) {
        let Some(request) = packet::parse_arp_request(payload) else { return };
        if matches!(request.target_ip, GATEWAY | NAMESERVER) {
            let reply = packet::arp_reply(&request, ROUTER_MAC.0, request.target_ip);
            self.send_guest(ETHERTYPE_ARP, &reply);
        }
    }

    fn ipv4(&mut self, ip: &[u8], now: u64) {
        let Some(header) = packet::parse_ipv4(ip) else { return };
        let ip = &ip[..header.total_len];
        let body = &ip[header.header_len..];
        match header.protocol {
            PROTO_ICMP => {
                if matches!(header.dst, GATEWAY | NAMESERVER)
                    && let Some(reply) = packet::echo_reply(body)
                {
                    let packet = packet::build_ipv4(header.dst, header.src, PROTO_ICMP, &reply);
                    self.send_guest(ETHERTYPE_IPV4, &packet);
                }
            }
            PROTO_UDP => {
                if let Some(udp) = packet::parse_udp(body) {
                    self.udp(header.src, header.dst, udp, now);
                }
            }
            PROTO_TCP => self.tcp(&header, ip),
            _ => {}
        }
    }

    fn udp(&mut self, src: Ipv4Addr, dst: Ipv4Addr, udp: packet::Udp, now: u64) {
        let guest = SocketAddrV4::new(src, udp.src_port);
        if udp.dst_port == 67 {
            let lease = dhcp::Lease {
                address: self.guest_ip,
                router: GATEWAY,
                dns: NAMESERVER,
                netmask: NETMASK,
                seconds: LEASE_SECONDS,
            };
            if let Some(reply) = dhcp::reply(udp.payload, &lease) {
                let packet = packet::build_udp(GATEWAY, 67, Ipv4Addr::BROADCAST, 68, &reply);
                self.send_guest(ETHERTYPE_IPV4, &packet);
            }
            return;
        }
        if dst == NAMESERVER && udp.dst_port == 53 {
            let Some(query) = dns::parse(udp.payload) else { return };
            if query.wants_address() {
                let token = self.id();
                self.actions.push(Action::Resolve { token, name: query.name.clone() });
                self.lookups.insert(token, (query, guest));
            } else {
                let answer = dns::answer(&query, &[], true);
                let packet = packet::build_udp(NAMESERVER, 53, src, udp.src_port, &answer);
                self.send_guest(ETHERTYPE_IPV4, &packet);
            }
            return;
        }
        // Broadcasts and multicasts stay on the guest's network.
        if dst.is_broadcast() || dst.is_multicast() || dst == Ipv4Addr::new(10, 0, 2, 255) || src.is_unspecified() {
            return;
        }
        let Some(to) = host_address(dst, udp.dst_port) else { return };
        let id = match self.udp.get_mut(&udp.src_port) {
            Some(flow) => {
                flow.last = now;
                flow.guest = guest;
                flow.id
            }
            None => {
                let id = self.id();
                self.actions.push(Action::UdpOpen { id });
                self.udp.insert(udp.src_port, UdpFlow { id, guest, last: now });
                self.udp_ports.insert(id, udp.src_port);
                id
            }
        };
        self.actions.push(Action::UdpSend { id, to, data: udp.payload.to_vec() });
    }

    fn tcp(&mut self, header: &packet::Ipv4, ip: &[u8]) {
        let Some(tcp) = packet::parse_tcp(&ip[header.header_len..]) else { return };
        let key = (tcp.src_port, SocketAddrV4::new(header.dst, tcp.dst_port));
        match self.flow_ids.get(&key).and_then(|id| self.flows.get(id)) {
            // Connecting: the guest's SYN again, which waits with the first.
            Some(flow) if flow.handle.is_none() => {}
            Some(_) => self.link.rx.push_back(ip.to_vec()),
            None if tcp.syn && !tcp.ack => {
                // For an address it can't reach: smoltcp refuses it.
                let Some(to) = host_address(header.dst, tcp.dst_port) else {
                    self.link.rx.push_back(ip.to_vec());
                    return;
                };
                let id = self.id();
                self.flows.insert(
                    id,
                    Flow {
                        key,
                        syn: Some(ip.to_vec()),
                        handle: None,
                        to_guest: VecDeque::new(),
                        host_eof: false,
                        closed: false,
                        shut: false,
                    },
                );
                self.flow_ids.insert(key, id);
                self.actions.push(Action::Connect { flow: id, to });
            }
            // Anything else for no connection: smoltcp resets it.
            None => {
                if !tcp.rst {
                    self.link.rx.push_back(ip.to_vec());
                }
            }
        }
    }

    /// Something happened on the host, at `now`.
    pub fn event(&mut self, event: Event, now: u64) {
        match event {
            Event::Connected { flow } => {
                let Some(f) = self.flows.get_mut(&flow) else {
                    self.actions.push(Action::Close { flow });
                    return;
                };
                let mut socket = tcp::Socket::new(
                    tcp::SocketBuffer::new(vec![0; TCP_BUFFER]),
                    tcp::SocketBuffer::new(vec![0; TCP_BUFFER]),
                );
                socket.set_nagle_enabled(false);
                socket.set_timeout(Some(TCP_TIMEOUT));
                let (_, local) = f.key;
                let _ =
                    socket.listen(IpListenEndpoint { addr: Some(IpAddress::Ipv4(*local.ip())), port: local.port() });
                f.handle = Some(self.sockets.add(socket));
                if let Some(syn) = f.syn.take() {
                    self.link.rx.push_back(syn);
                }
            }
            Event::ConnectFailed { flow } => {
                // With no connection to take it, smoltcp refuses the SYN.
                if let Some(f) = self.flows.remove(&flow) {
                    self.flow_ids.remove(&f.key);
                    if let Some(syn) = f.syn {
                        self.link.rx.push_back(syn);
                    }
                }
            }
            Event::Data { flow, data } => {
                if let Some(f) = self.flows.get_mut(&flow) {
                    f.to_guest.extend(data);
                }
            }
            Event::Eof { flow } => {
                if let Some(f) = self.flows.get_mut(&flow) {
                    f.host_eof = true;
                }
            }
            Event::Failed { flow } => {
                if let Some(handle) = self.flows.get(&flow).and_then(|f| f.handle) {
                    self.sockets.get_mut::<tcp::Socket>(handle).abort();
                }
            }
            Event::UdpData { id, from, data } => {
                let Some(flow) = self.udp_ports.get(&id).and_then(|port| self.udp.get_mut(port)) else { return };
                flow.last = now;
                let guest = flow.guest;
                let SocketAddr::V4(from) = from else { return };
                // The host itself is the router's address.
                let src = if from.ip().is_loopback() { GATEWAY } else { *from.ip() };
                let packet = packet::build_udp(src, from.port(), *guest.ip(), guest.port(), &data);
                self.send_guest(ETHERTYPE_IPV4, &packet);
            }
            Event::Resolved { token, addresses, found } => {
                let Some((query, guest)) = self.lookups.remove(&token) else { return };
                let answer = dns::answer(&query, &addresses, found);
                let packet = packet::build_udp(NAMESERVER, 53, *guest.ip(), guest.port(), &answer);
                self.send_guest(ETHERTYPE_IPV4, &packet);
            }
        }
        self.poll(now);
    }

    /// Run the TCP stack: take what the guest sent, move data between the
    /// connections and the host, and send the guest what is due.
    pub fn poll(&mut self, now: u64) {
        for _ in 0..16 {
            self.iface.poll(Instant::from_millis(now as i64), &mut self.link, &mut self.sockets);
            let moved = self.pump();
            for ip in std::mem::take(&mut self.link.tx) {
                self.send_guest(ETHERTYPE_IPV4, &ip);
            }
            if !moved && self.link.rx.is_empty() {
                break;
            }
        }
        self.expire_udp(now);
    }

    /// Move data between the connections and the host; whether any moved.
    fn pump(&mut self) -> bool {
        let mut moved = false;
        let mut ended = Vec::new();
        for (&id, flow) in self.flows.iter_mut() {
            let Some(handle) = flow.handle else { continue };
            let socket = self.sockets.get_mut::<tcp::Socket>(handle);
            while socket.can_recv() {
                let mut data = vec![0; 16 * 1024];
                match socket.recv_slice(&mut data) {
                    Ok(n) if n > 0 => {
                        data.truncate(n);
                        self.actions.push(Action::Send { flow: id, data });
                        moved = true;
                    }
                    _ => break,
                }
            }
            if !flow.to_guest.is_empty() && socket.can_send() {
                let (front, back) = flow.to_guest.as_slices();
                let mut sent = socket.send_slice(front).unwrap_or(0);
                if sent == front.len() && !back.is_empty() {
                    sent += socket.send_slice(back).unwrap_or(0);
                }
                if sent > 0 {
                    flow.to_guest.drain(..sent);
                    self.actions.push(Action::Credit { flow: id, bytes: sent });
                    moved = true;
                }
            }
            if flow.host_eof && flow.to_guest.is_empty() && !flow.closed {
                socket.close();
                flow.closed = true;
                moved = true;
            }
            let state = socket.state();
            if !flow.shut && !socket.may_recv() && !matches!(state, tcp::State::Listen | tcp::State::SynReceived) {
                self.actions.push(Action::Shutdown { flow: id });
                flow.shut = true;
            }
            if matches!(state, tcp::State::Closed | tcp::State::TimeWait) {
                ended.push(id);
            }
        }
        for id in ended {
            if let Some(flow) = self.flows.remove(&id) {
                self.flow_ids.remove(&flow.key);
                if let Some(handle) = flow.handle {
                    self.sockets.remove(handle);
                }
                self.actions.push(Action::Close { flow: id });
                moved = true;
            }
        }
        moved
    }

    fn expire_udp(&mut self, now: u64) {
        let idle: Vec<u16> =
            self.udp.iter().filter(|(_, f)| now.saturating_sub(f.last) > UDP_IDLE_MS).map(|(port, _)| *port).collect();
        for port in idle {
            if let Some(flow) = self.udp.remove(&port) {
                self.udp_ports.remove(&flow.id);
                self.actions.push(Action::UdpClose { id: flow.id });
            }
        }
    }

    /// Connections and UDP flows open, for the debugger.
    pub fn counts(&self) -> (usize, usize) {
        (self.flows.len(), self.udp.len())
    }
}

/// Where on the host a guest's packet for `ip`:`port` goes: the router's
/// address is the host itself; the guest's own network isn't reached.
fn host_address(ip: Ipv4Addr, port: u16) -> Option<SocketAddr> {
    if ip == GATEWAY {
        return Some(SocketAddr::from((Ipv4Addr::LOCALHOST, port)));
    }
    let local = u32::from(ip) & u32::from(NETMASK) == u32::from(GATEWAY) & u32::from(NETMASK);
    (!local && !ip.is_unspecified() && !ip.is_loopback()).then(|| SocketAddr::from((ip, port)))
}
