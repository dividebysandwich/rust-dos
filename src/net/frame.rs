//! Ethernet frames as the tunnel carries them: destination and source
//! addresses, then the EtherType (or an 802.3 length), then the payload,
//! without the preamble and without the frame check sequence.

use std::fmt;

/// The header: destination, source, EtherType or length.
pub const HEADER: usize = 14;
/// The shortest frame on the wire, padded up to it, and the longest.
pub const MIN_FRAME: usize = 60;
pub const MAX_FRAME: usize = 1514;

/// EtherTypes.
pub const ETHERTYPE_IPV4: u16 = 0x0800;
pub const ETHERTYPE_ARP: u16 = 0x0806;
pub const ETHERTYPE_IPX: u16 = 0x8137;

/// A 48-bit Ethernet address.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Mac(pub [u8; 6]);

impl Mac {
    pub const BROADCAST: Mac = Mac([0xFF; 6]);

    /// Broadcast or multicast: the group bit of the first byte.
    pub fn is_group(self) -> bool {
        self.0[0] & 1 != 0
    }

    pub fn is_broadcast(self) -> bool {
        self == Self::BROADCAST
    }

    /// A random unicast address with the locally administered bit set, so
    /// it can't be a real card's. In deterministic mode it comes from the
    /// mode's generator, the same in every run with the same start time.
    pub fn random_local() -> Mac {
        let mut bytes = [0; 6];
        if !crate::deterministic::random_bytes(&mut bytes) {
            super::fill_random(&mut bytes);
        }
        bytes[0] = (bytes[0] & 0xFC) | 0x02;
        Mac(bytes)
    }

    /// `02:00:5E:12:34:56`, with colons or dashes.
    pub fn parse(s: &str) -> Option<Mac> {
        let parts: Vec<&str> = s.trim().split([':', '-']).collect();
        if parts.len() != 6 {
            return None;
        }
        let mut bytes = [0; 6];
        for (b, p) in bytes.iter_mut().zip(parts) {
            if p.is_empty() || p.len() > 2 {
                return None;
            }
            *b = u8::from_str_radix(p, 16).ok()?;
        }
        Some(Mac(bytes))
    }
}

impl fmt::Display for Mac {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let b = self.0;
        write!(f, "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}", b[0], b[1], b[2], b[3], b[4], b[5])
    }
}

impl fmt::Debug for Mac {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// The destination address of `frame`, if it has a header.
pub fn destination(frame: &[u8]) -> Option<Mac> {
    Some(Mac(frame.get(0..6)?.try_into().ok()?))
}

/// The source address of `frame`, if it has a header.
pub fn source(frame: &[u8]) -> Option<Mac> {
    Some(Mac(frame.get(6..12)?.try_into().ok()?))
}

/// The EtherType or 802.3 length field of `frame`.
pub fn ethertype(frame: &[u8]) -> Option<u16> {
    Some(u16::from_be_bytes(frame.get(12..14)?.try_into().ok()?))
}

/// A frame with this header and payload, padded to the minimum length.
pub fn build(destination: Mac, source: Mac, ethertype: u16, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity((HEADER + payload.len()).max(MIN_FRAME));
    frame.extend_from_slice(&destination.0);
    frame.extend_from_slice(&source.0);
    frame.extend_from_slice(&ethertype.to_be_bytes());
    frame.extend_from_slice(payload);
    if frame.len() < MIN_FRAME {
        frame.resize(MIN_FRAME, 0);
    }
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_prints_addresses() {
        let mac = Mac::parse("02:00:5e:12:34:AB").unwrap();
        assert_eq!(mac, Mac([0x02, 0x00, 0x5E, 0x12, 0x34, 0xAB]));
        assert_eq!(mac.to_string(), "02:00:5E:12:34:AB");
        assert_eq!(Mac::parse("02-00-5E-12-34-AB"), Some(mac));
        assert_eq!(Mac::parse("02:00:5E:12:34"), None);
        assert_eq!(Mac::parse("02:00:5E:12:34:ABC"), None);
        assert_eq!(Mac::parse("02:00:5E:12:34:XY"), None);
    }

    #[test]
    fn random_addresses_are_local_unicast() {
        for _ in 0..32 {
            let mac = Mac::random_local();
            assert!(!mac.is_group());
            assert_eq!(mac.0[0] & 2, 2);
        }
        assert_ne!(Mac::random_local(), Mac::random_local());
    }

    #[test]
    fn builds_padded_frames() {
        let a = Mac([2, 0, 0, 0, 0, 1]);
        let frame = build(Mac::BROADCAST, a, ETHERTYPE_IPX, &[1, 2, 3]);
        assert_eq!(frame.len(), MIN_FRAME);
        assert_eq!(destination(&frame), Some(Mac::BROADCAST));
        assert_eq!(source(&frame), Some(a));
        assert_eq!(ethertype(&frame), Some(ETHERTYPE_IPX));
        assert_eq!(&frame[14..17], &[1, 2, 3]);
        assert!(Mac::BROADCAST.is_group() && Mac::BROADCAST.is_broadcast());
        assert_eq!(destination(&[0; 5]), None);
    }
}
