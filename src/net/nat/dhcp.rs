//! The router's DHCP server, which also answers BOOTP: it hands the card's
//! guest its address, the netmask, the router and the name server, as a
//! home router does. There is one guest, so one address to hand out.

use std::net::Ipv4Addr;

/// DHCP message types (option 53).
const DISCOVER: u8 = 1;
const OFFER: u8 = 2;
const REQUEST: u8 = 3;
const DECLINE: u8 = 4;
const ACK: u8 = 5;
const NAK: u8 = 6;
const RELEASE: u8 = 7;
const INFORM: u8 = 8;
const MAGIC_COOKIE: [u8; 4] = [99, 130, 83, 99];

/// What the server hands out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lease {
    pub address: Ipv4Addr,
    pub router: Ipv4Addr,
    pub dns: Ipv4Addr,
    pub netmask: Ipv4Addr,
    pub seconds: u32,
}

/// The reply to the DHCP or BOOTP request `request`, if it wants one: the
/// UDP payload, and whether it goes to the client's address (else to all).
pub fn reply(request: &[u8], lease: &Lease) -> Option<Vec<u8>> {
    // A BOOTREQUEST for Ethernet addresses.
    if request.len() < 240 || request[0] != 1 || request[1] != 1 || request[2] != 6 {
        return None;
    }
    let options = if request[236..240] == MAGIC_COOKIE { parse_options(&request[240..]) } else { Vec::new() };
    let kind = options.iter().find(|(code, _)| *code == 53).and_then(|(_, v)| v.first().copied());
    let requested = options
        .iter()
        .find(|(code, _)| *code == 50)
        .and_then(|(_, v)| (v.len() == 4).then(|| Ipv4Addr::new(v[0], v[1], v[2], v[3])));
    let ciaddr = Ipv4Addr::new(request[12], request[13], request[14], request[15]);
    let answer = match kind {
        // BOOTP: no message type, an ordinary reply.
        None => None,
        Some(DISCOVER) => Some(OFFER),
        // A request for another address (renewing an old lease) gets a
        // NAK, which sends the client back to DISCOVER.
        Some(REQUEST) => {
            let wanted = requested.unwrap_or(ciaddr);
            Some(if wanted == lease.address || wanted.is_unspecified() { ACK } else { NAK })
        }
        Some(INFORM) => Some(ACK),
        Some(DECLINE | RELEASE) | Some(_) => return None,
    };
    let mut out = vec![0u8; 240];
    out[0] = 2; // BOOTREPLY
    out[1] = 1;
    out[2] = 6;
    out[4..8].copy_from_slice(&request[4..8]); // xid
    out[10..12].copy_from_slice(&request[10..12]); // flags
    out[12..16].copy_from_slice(&request[12..16]); // ciaddr
    if answer != Some(NAK) && kind != Some(INFORM) {
        out[16..20].copy_from_slice(&lease.address.octets()); // yiaddr
    }
    out[20..24].copy_from_slice(&lease.router.octets()); // siaddr
    out[28..44].copy_from_slice(&request[28..44]); // chaddr
    out[236..240].copy_from_slice(&MAGIC_COOKIE);
    let mut add = |code: u8, value: &[u8]| {
        out.push(code);
        out.push(value.len() as u8);
        out.extend_from_slice(value);
    };
    if let Some(answer) = answer {
        add(53, &[answer]);
    }
    add(54, &lease.router.octets());
    if answer != Some(NAK) {
        add(1, &lease.netmask.octets());
        add(3, &lease.router.octets());
        add(6, &lease.dns.octets());
        let broadcast = u32::from(lease.address) | !u32::from(lease.netmask);
        add(28, &broadcast.to_be_bytes());
        if kind != Some(INFORM) {
            add(51, &lease.seconds.to_be_bytes());
            add(58, &(lease.seconds / 2).to_be_bytes());
            add(59, &(lease.seconds / 8 * 7).to_be_bytes());
        }
    }
    out.push(255);
    // BOOTP clients want the vendor area 64 bytes long at least.
    if out.len() < 300 {
        out.resize(300, 0);
    }
    Some(out)
}

fn parse_options(mut bytes: &[u8]) -> Vec<(u8, Vec<u8>)> {
    let mut options = Vec::new();
    while let [code, rest @ ..] = bytes {
        match code {
            0 => bytes = rest,
            255 => break,
            _ => {
                let Some((&len, rest)) = rest.split_first() else { break };
                let Some(value) = rest.get(..len as usize) else { break };
                options.push((*code, value.to_vec()));
                bytes = &rest[len as usize..];
            }
        }
    }
    options
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEASE: Lease = Lease {
        address: Ipv4Addr::new(10, 0, 2, 15),
        router: Ipv4Addr::new(10, 0, 2, 2),
        dns: Ipv4Addr::new(10, 0, 2, 3),
        netmask: Ipv4Addr::new(255, 255, 255, 0),
        seconds: 3600,
    };

    fn request(kind: Option<u8>, requested: Option<Ipv4Addr>) -> Vec<u8> {
        let mut r = vec![0u8; 240];
        r[0..3].copy_from_slice(&[1, 1, 6]);
        r[4..8].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        r[28..34].copy_from_slice(&[2, 0, 0, 0, 0, 1]);
        if let Some(kind) = kind {
            r[236..240].copy_from_slice(&MAGIC_COOKIE);
            r.extend_from_slice(&[53, 1, kind]);
            if let Some(ip) = requested {
                r.extend_from_slice(&[50, 4]);
                r.extend_from_slice(&ip.octets());
            }
            r.push(255);
        }
        r
    }

    fn option(reply: &[u8], code: u8) -> Option<Vec<u8>> {
        parse_options(&reply[240..]).into_iter().find(|(c, _)| *c == code).map(|(_, v)| v)
    }

    #[test]
    fn offers_and_acknowledges_the_guest_address() {
        let offer = reply(&request(Some(DISCOVER), None), &LEASE).unwrap();
        assert_eq!(offer[0], 2);
        assert_eq!(&offer[4..8], &[0xDE, 0xAD, 0xBE, 0xEF]);
        assert_eq!(&offer[16..20], &[10, 0, 2, 15]);
        assert_eq!(&offer[28..34], &[2, 0, 0, 0, 0, 1]);
        assert_eq!(option(&offer, 53), Some(vec![OFFER]));
        assert_eq!(option(&offer, 3), Some(vec![10, 0, 2, 2]));
        assert_eq!(option(&offer, 6), Some(vec![10, 0, 2, 3]));
        assert_eq!(option(&offer, 1), Some(vec![255, 255, 255, 0]));
        let ack = reply(&request(Some(REQUEST), Some(LEASE.address)), &LEASE).unwrap();
        assert_eq!(option(&ack, 53), Some(vec![ACK]));
        assert_eq!(option(&ack, 51), Some(3600u32.to_be_bytes().to_vec()));
        // An old lease's address is refused.
        let nak = reply(&request(Some(REQUEST), Some(Ipv4Addr::new(10, 0, 2, 99))), &LEASE).unwrap();
        assert_eq!(option(&nak, 53), Some(vec![NAK]));
        assert_eq!(&nak[16..20], &[0; 4]);
        assert_eq!(reply(&request(Some(RELEASE), None), &LEASE), None);
    }

    #[test]
    fn answers_bootp() {
        let r = reply(&request(None, None), &LEASE).unwrap();
        assert_eq!(&r[16..20], &[10, 0, 2, 15]);
        assert_eq!(option(&r, 53), None);
        assert_eq!(option(&r, 3), Some(vec![10, 0, 2, 2]));
        assert!(r.len() >= 300);
        assert_eq!(reply(&[0u8; 100], &LEASE), None);
    }
}
