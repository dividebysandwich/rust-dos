//! The packets the router reads and writes itself: ARP, IPv4 with UDP and
//! ICMP, and enough of TCP to see a connection start. (TCP proper is
//! smoltcp's.)

use std::net::Ipv4Addr;

pub const PROTO_ICMP: u8 = 1;
pub const PROTO_TCP: u8 = 6;
pub const PROTO_UDP: u8 = 17;

/// The Internet checksum of `data`, folded, starting from `sum`.
pub fn checksum(data: &[u8], mut sum: u32) -> u16 {
    for pair in data.chunks(2) {
        sum += u16::from_be_bytes([pair[0], *pair.get(1).unwrap_or(&0)]) as u32;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

/// An IPv4 packet's header fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ipv4 {
    pub src: Ipv4Addr,
    pub dst: Ipv4Addr,
    pub protocol: u8,
    /// Where the payload starts, and where it ends.
    pub header_len: usize,
    pub total_len: usize,
}

/// The header of the IPv4 packet `ip`, if it is a whole one (not a
/// fragment) with a good checksum.
pub fn parse_ipv4(ip: &[u8]) -> Option<Ipv4> {
    if ip.len() < 20 || ip[0] >> 4 != 4 {
        return None;
    }
    let header_len = (ip[0] & 0x0F) as usize * 4;
    let total_len = u16::from_be_bytes([ip[2], ip[3]]) as usize;
    if header_len < 20 || total_len < header_len || total_len > ip.len() || checksum(&ip[..header_len], 0) != 0 {
        return None;
    }
    // More fragments, or an offset: fragments aren't put together.
    if u16::from_be_bytes([ip[6], ip[7]]) & 0x3FFF != 0 {
        return None;
    }
    Some(Ipv4 {
        src: Ipv4Addr::new(ip[12], ip[13], ip[14], ip[15]),
        dst: Ipv4Addr::new(ip[16], ip[17], ip[18], ip[19]),
        protocol: ip[9],
        header_len,
        total_len,
    })
}

/// An IPv4 packet from `src` to `dst` with `payload`.
pub fn build_ipv4(src: Ipv4Addr, dst: Ipv4Addr, protocol: u8, payload: &[u8]) -> Vec<u8> {
    let total = 20 + payload.len();
    let mut ip = Vec::with_capacity(total);
    ip.extend_from_slice(&[0x45, 0, (total >> 8) as u8, total as u8, 0, 0, 0x40, 0, 64, protocol, 0, 0]);
    ip.extend_from_slice(&src.octets());
    ip.extend_from_slice(&dst.octets());
    let sum = checksum(&ip, 0);
    ip[10..12].copy_from_slice(&sum.to_be_bytes());
    ip.extend_from_slice(payload);
    ip
}

/// The sum of the pseudo-header of a UDP or TCP segment.
fn pseudo_sum(src: Ipv4Addr, dst: Ipv4Addr, protocol: u8, len: usize) -> u32 {
    let (s, d) = (src.octets(), dst.octets());
    [
        u16::from_be_bytes([s[0], s[1]]),
        u16::from_be_bytes([s[2], s[3]]),
        u16::from_be_bytes([d[0], d[1]]),
        u16::from_be_bytes([d[2], d[3]]),
    ]
    .iter()
    .map(|&w| w as u32)
    .sum::<u32>()
        + protocol as u32
        + len as u32
}

/// A UDP datagram's ports and payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Udp<'a> {
    pub src_port: u16,
    pub dst_port: u16,
    pub payload: &'a [u8],
}

pub fn parse_udp(segment: &[u8]) -> Option<Udp<'_>> {
    if segment.len() < 8 {
        return None;
    }
    let len = u16::from_be_bytes([segment[4], segment[5]]) as usize;
    if len < 8 || len > segment.len() {
        return None;
    }
    Some(Udp {
        src_port: u16::from_be_bytes([segment[0], segment[1]]),
        dst_port: u16::from_be_bytes([segment[2], segment[3]]),
        payload: &segment[8..len],
    })
}

/// An IPv4 packet with a UDP datagram.
pub fn build_udp(src: Ipv4Addr, src_port: u16, dst: Ipv4Addr, dst_port: u16, payload: &[u8]) -> Vec<u8> {
    let len = 8 + payload.len();
    let mut udp = Vec::with_capacity(len);
    udp.extend_from_slice(&src_port.to_be_bytes());
    udp.extend_from_slice(&dst_port.to_be_bytes());
    udp.extend_from_slice(&(len as u16).to_be_bytes());
    udp.extend_from_slice(&[0, 0]);
    udp.extend_from_slice(payload);
    let sum = match checksum(&udp, pseudo_sum(src, dst, PROTO_UDP, len)) {
        0 => 0xFFFF,
        sum => sum,
    };
    udp[6..8].copy_from_slice(&sum.to_be_bytes());
    build_ipv4(src, dst, PROTO_UDP, &udp)
}

/// A TCP segment's ports and flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tcp {
    pub src_port: u16,
    pub dst_port: u16,
    pub syn: bool,
    pub ack: bool,
    pub rst: bool,
}

pub fn parse_tcp(segment: &[u8]) -> Option<Tcp> {
    if segment.len() < 20 {
        return None;
    }
    let flags = segment[13];
    Some(Tcp {
        src_port: u16::from_be_bytes([segment[0], segment[1]]),
        dst_port: u16::from_be_bytes([segment[2], segment[3]]),
        syn: flags & 0x02 != 0,
        ack: flags & 0x10 != 0,
        rst: flags & 0x04 != 0,
    })
}

/// The echo reply to the ICMP echo request `icmp`, if it is one.
pub fn echo_reply(icmp: &[u8]) -> Option<Vec<u8>> {
    if icmp.len() < 8 || icmp[0] != 8 || checksum(icmp, 0) != 0 {
        return None;
    }
    let mut reply = icmp.to_vec();
    reply[0] = 0;
    reply[2..4].copy_from_slice(&[0, 0]);
    let sum = checksum(&reply, 0);
    reply[2..4].copy_from_slice(&sum.to_be_bytes());
    Some(reply)
}

/// An ARP request for an IPv4 address: who asks (address and IP), and
/// for which IP.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArpRequest {
    pub sender_mac: [u8; 6],
    pub sender_ip: Ipv4Addr,
    pub target_ip: Ipv4Addr,
}

pub fn parse_arp_request(arp: &[u8]) -> Option<ArpRequest> {
    // Ethernet, IPv4, address lengths 6 and 4, a request.
    if arp.len() < 28 || arp[0..8] != [0, 1, 8, 0, 6, 4, 0, 1] {
        return None;
    }
    Some(ArpRequest {
        sender_mac: arp[8..14].try_into().unwrap(),
        sender_ip: Ipv4Addr::new(arp[14], arp[15], arp[16], arp[17]),
        target_ip: Ipv4Addr::new(arp[24], arp[25], arp[26], arp[27]),
    })
}

/// The reply to `request`: `ip` is at `mac`.
pub fn arp_reply(request: &ArpRequest, mac: [u8; 6], ip: Ipv4Addr) -> Vec<u8> {
    let mut arp = vec![0, 1, 8, 0, 6, 4, 0, 2];
    arp.extend_from_slice(&mac);
    arp.extend_from_slice(&ip.octets());
    arp.extend_from_slice(&request.sender_mac);
    arp.extend_from_slice(&request.sender_ip.octets());
    arp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packets_round_trip_with_good_checksums() {
        let (a, b) = (Ipv4Addr::new(10, 0, 2, 15), Ipv4Addr::new(10, 0, 2, 3));
        let ip = build_udp(a, 1024, b, 53, b"query");
        let header = parse_ipv4(&ip).unwrap();
        assert_eq!((header.src, header.dst, header.protocol, header.total_len), (a, b, PROTO_UDP, ip.len()));
        let udp = parse_udp(&ip[header.header_len..]).unwrap();
        assert_eq!((udp.src_port, udp.dst_port, udp.payload), (1024, 53, &b"query"[..]));
        // The UDP checksum over the pseudo-header comes out right.
        let segment = &ip[20..];
        assert_eq!(checksum(segment, pseudo_sum(a, b, PROTO_UDP, segment.len())), 0);
        // A damaged header is refused, and so are fragments.
        let mut bad = ip.clone();
        bad[8] ^= 1;
        assert_eq!(parse_ipv4(&bad), None);
        let mut fragment = build_ipv4(a, b, PROTO_UDP, &[0; 8]);
        fragment[6] = 0x20;
        let sum = checksum(
            &{
                let mut h = fragment[..20].to_vec();
                h[10..12].copy_from_slice(&[0, 0]);
                h
            },
            0,
        );
        fragment[10..12].copy_from_slice(&sum.to_be_bytes());
        assert_eq!(parse_ipv4(&fragment), None);
    }

    #[test]
    fn answers_echo_requests_and_arp() {
        let mut request = vec![8, 0, 0, 0, 0x12, 0x34, 0, 1, b'p', b'i', b'n', b'g'];
        let sum = checksum(&request, 0);
        request[2..4].copy_from_slice(&sum.to_be_bytes());
        let reply = echo_reply(&request).unwrap();
        assert_eq!(reply[0], 0);
        assert_eq!(checksum(&reply, 0), 0);
        assert_eq!(&reply[4..], &request[4..]);
        assert_eq!(echo_reply(&reply), None, "a reply isn't answered");

        let mut arp = vec![0, 1, 8, 0, 6, 4, 0, 1, 2, 0, 0, 0, 0, 1, 10, 0, 2, 15, 0, 0, 0, 0, 0, 0, 10, 0, 2, 2];
        let request = parse_arp_request(&arp).unwrap();
        assert_eq!(request.target_ip, Ipv4Addr::new(10, 0, 2, 2));
        let reply = arp_reply(&request, [0x52, 0x54, 0, 0x12, 0x35, 2], request.target_ip);
        assert_eq!(&reply[6..8], &[0, 2]);
        assert_eq!(&reply[18..24], &[2, 0, 0, 0, 0, 1]);
        arp[7] = 2;
        assert_eq!(parse_arp_request(&arp), None, "a reply isn't a request");
    }
}
