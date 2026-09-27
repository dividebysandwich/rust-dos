//! The LAN tunnel's datagrams. Each UDP payload starts with a 16-byte
//! header: the magic `RDLN`, the protocol version, the message type, two
//! reserved bytes and the sender's or receiver's session token (0 before a
//! member has joined). All numbers are big-endian.
//!
//! A client finds a relay with DISCOVER (broadcast on the LAN) and OFFER,
//! is handed a cookie with HELLO and CHALLENGE, and joins a room with JOIN,
//! answered by WELCOME or REJECT. Members then exchange DATA, which carries
//! an Ethernet frame in one or more fragments, and keep their place and
//! their NAT's mapping with KEEPALIVE and ACK, until LEAVE.
//!
//! Requests that a relay answers before knowing the sender are at least as
//! long as the answer, so a relay can't be used to amplify traffic towards
//! a forged address.

/// The start of every datagram.
pub const MAGIC: [u8; 4] = *b"RDLN";
pub const VERSION: u8 = 1;
pub const HEADER: usize = 16;
/// The relay's UDP port unless another is given.
pub const DEFAULT_PORT: u16 = 21213;
/// The largest piece of a frame in one datagram, below the path MTU of the
/// internet with room to spare for tunnels and PPPoE.
pub const MAX_FRAGMENT: usize = 1200;
/// The length DISCOVER and HELLO are padded to, the most OFFER and
/// CHALLENGE take.
pub const DISCOVER_SIZE: usize = 256;
pub const HELLO_SIZE: usize = 64;
/// The longest room or relay name.
pub const MAX_NAME: usize = 32;

const DISCOVER: u8 = 1;
const OFFER: u8 = 2;
const HELLO: u8 = 3;
const CHALLENGE: u8 = 4;
const JOIN: u8 = 5;
const WELCOME: u8 = 6;
const REJECT: u8 = 7;
const DATA: u8 = 8;
const KEEPALIVE: u8 = 9;
const ACK: u8 = 10;
const LEAVE: u8 = 11;

/// Why a relay turned a client away.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RejectReason {
    /// The password is wrong, or missing.
    Password,
    /// The room, or the relay, has no place left.
    Full,
    /// The cookie is too old, or not the relay's.
    Cookie,
    /// The client speaks another protocol version.
    Version,
    /// The relay doesn't know the token: it restarted, or dropped the
    /// member for being silent too long.
    Unknown,
}

impl RejectReason {
    fn code(self) -> u8 {
        match self {
            RejectReason::Password => 1,
            RejectReason::Full => 2,
            RejectReason::Cookie => 3,
            RejectReason::Version => 4,
            RejectReason::Unknown => 5,
        }
    }

    fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            1 => RejectReason::Password,
            2 => RejectReason::Full,
            3 => RejectReason::Cookie,
            4 => RejectReason::Version,
            5 => RejectReason::Unknown,
            _ => return None,
        })
    }

    pub fn describe(self) -> &'static str {
        match self {
            RejectReason::Password => "wrong password",
            RejectReason::Full => "the room is full",
            RejectReason::Cookie => "the join took too long",
            RejectReason::Version => "the relay runs another version of rust-dos",
            RejectReason::Unknown => "the relay no longer knows this member",
        }
    }
}

/// A room as an OFFER lists it: its name and how many are in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoomInfo {
    pub name: String,
    pub members: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    Discover,
    /// A relay's answer to DISCOVER: the port it listens on, whether it
    /// wants a password, its name and its rooms.
    Offer {
        port: u16,
        password: bool,
        name: String,
        rooms: Vec<RoomInfo>,
    },
    /// `client_id` is random and kept for the life of the process, so a
    /// relay can give a member that comes back its old place.
    Hello {
        client_id: u64,
    },
    Challenge {
        cookie: [u8; 16],
        password: bool,
    },
    /// `proof` is `auth::proof` of the password, zeros without one.
    Join {
        client_id: u64,
        cookie: [u8; 16],
        room: String,
        proof: [u8; 32],
    },
    /// The member's index in its room (1-200), how often it should send
    /// KEEPALIVE, in seconds, and how many are in the room. The header
    /// carries the new token.
    Welcome {
        index: u8,
        keepalive: u8,
        members: u16,
    },
    Reject {
        reason: RejectReason,
    },
    /// A fragment of a frame: from which member (filled in by the relay),
    /// the sender's frame number, which fragment and of how many.
    Data {
        source: u8,
        seq: u16,
        fragment: u8,
        count: u8,
        payload: Vec<u8>,
    },
    /// `stamp` is the sender's clock in ms, echoed by ACK to time the
    /// round trip.
    Keepalive {
        stamp: u32,
    },
    Ack {
        stamp: u32,
        members: u16,
    },
    Leave,
}

/// A decoded datagram.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packet {
    pub token: u64,
    pub message: Message,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// Not a tunnel datagram, or a damaged one.
    Malformed,
    /// A tunnel datagram of another protocol version.
    Version(u8),
}

/// The datagram of `message` with `token` in its header.
pub fn encode(token: u64, message: &Message) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    out.extend_from_slice(&MAGIC);
    out.push(VERSION);
    out.push(kind(message));
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(&token.to_be_bytes());
    match message {
        Message::Discover => out.resize(DISCOVER_SIZE, 0),
        Message::Offer { port, password, name, rooms } => {
            out.extend_from_slice(&port.to_be_bytes());
            out.push(*password as u8);
            put_name(&mut out, name);
            // As many rooms as fit in the size of a DISCOVER.
            let count_at = out.len();
            out.push(0);
            let mut count = 0u8;
            for room in rooms {
                let name = truncate(&room.name);
                if out.len() + 1 + name.len() + 2 > DISCOVER_SIZE || count == u8::MAX {
                    break;
                }
                put_name(&mut out, name);
                out.extend_from_slice(&room.members.to_be_bytes());
                count += 1;
            }
            out[count_at] = count;
        }
        Message::Hello { client_id } => {
            out.extend_from_slice(&client_id.to_be_bytes());
            out.resize(HELLO_SIZE, 0);
        }
        Message::Challenge { cookie, password } => {
            out.extend_from_slice(cookie);
            out.push(*password as u8);
        }
        Message::Join { client_id, cookie, room, proof } => {
            out.extend_from_slice(&client_id.to_be_bytes());
            out.extend_from_slice(cookie);
            put_name(&mut out, room);
            out.extend_from_slice(proof);
        }
        Message::Welcome { index, keepalive, members } => {
            out.push(*index);
            out.push(*keepalive);
            out.extend_from_slice(&members.to_be_bytes());
        }
        Message::Reject { reason } => out.push(reason.code()),
        Message::Data { source, seq, fragment, count, payload } => {
            out.push(*source);
            out.extend_from_slice(&seq.to_be_bytes());
            out.push(*fragment);
            out.push(*count);
            out.extend_from_slice(payload);
        }
        Message::Keepalive { stamp } => out.extend_from_slice(&stamp.to_be_bytes()),
        Message::Ack { stamp, members } => {
            out.extend_from_slice(&stamp.to_be_bytes());
            out.extend_from_slice(&members.to_be_bytes());
        }
        Message::Leave => {}
    }
    out
}

fn kind(message: &Message) -> u8 {
    match message {
        Message::Discover => DISCOVER,
        Message::Offer { .. } => OFFER,
        Message::Hello { .. } => HELLO,
        Message::Challenge { .. } => CHALLENGE,
        Message::Join { .. } => JOIN,
        Message::Welcome { .. } => WELCOME,
        Message::Reject { .. } => REJECT,
        Message::Data { .. } => DATA,
        Message::Keepalive { .. } => KEEPALIVE,
        Message::Ack { .. } => ACK,
        Message::Leave => LEAVE,
    }
}

/// A name cut to `MAX_NAME` bytes at a character boundary.
fn truncate(name: &str) -> &str {
    let mut end = name.len().min(MAX_NAME);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    &name[..end]
}

fn put_name(out: &mut Vec<u8>, name: &str) {
    let name = truncate(name);
    out.push(name.len() as u8);
    out.extend_from_slice(name.as_bytes());
}

/// Reads the body of a datagram.
struct Body<'a> {
    bytes: &'a [u8],
}

impl<'a> Body<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        if self.bytes.len() < n {
            return Err(DecodeError::Malformed);
        }
        let (head, rest) = self.bytes.split_at(n);
        self.bytes = rest;
        Ok(head)
    }

    fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64, DecodeError> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        Ok(self.take(N)?.try_into().unwrap())
    }

    fn bool(&mut self) -> Result<bool, DecodeError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(DecodeError::Malformed),
        }
    }

    fn name(&mut self) -> Result<String, DecodeError> {
        let len = self.u8()? as usize;
        if len > MAX_NAME {
            return Err(DecodeError::Malformed);
        }
        String::from_utf8(self.take(len)?.to_vec()).map_err(|_| DecodeError::Malformed)
    }

    fn rest(&mut self) -> &'a [u8] {
        std::mem::take(&mut self.bytes)
    }

    /// Nothing may follow the fields, except padding where it is asked for.
    fn end(&self) -> Result<(), DecodeError> {
        if self.bytes.is_empty() { Ok(()) } else { Err(DecodeError::Malformed) }
    }
}

/// The datagram `bytes`, if it is a well-formed one of this version.
pub fn decode(bytes: &[u8]) -> Result<Packet, DecodeError> {
    if bytes.len() < HEADER || bytes[0..4] != MAGIC {
        return Err(DecodeError::Malformed);
    }
    if bytes[4] != VERSION {
        return Err(DecodeError::Version(bytes[4]));
    }
    let token = u64::from_be_bytes(bytes[8..16].try_into().unwrap());
    let mut body = Body { bytes: &bytes[HEADER..] };
    let message = match bytes[5] {
        DISCOVER => {
            if bytes.len() < DISCOVER_SIZE {
                return Err(DecodeError::Malformed);
            }
            body.rest();
            Message::Discover
        }
        OFFER => {
            let port = body.u16()?;
            let password = body.bool()?;
            let name = body.name()?;
            let count = body.u8()?;
            let mut rooms = Vec::with_capacity(count as usize);
            for _ in 0..count {
                let name = body.name()?;
                let members = body.u16()?;
                rooms.push(RoomInfo { name, members });
            }
            Message::Offer { port, password, name, rooms }
        }
        HELLO => {
            if bytes.len() < HELLO_SIZE {
                return Err(DecodeError::Malformed);
            }
            let client_id = body.u64()?;
            body.rest();
            Message::Hello { client_id }
        }
        CHALLENGE => Message::Challenge { cookie: body.array()?, password: body.bool()? },
        JOIN => {
            Message::Join { client_id: body.u64()?, cookie: body.array()?, room: body.name()?, proof: body.array()? }
        }
        WELCOME => Message::Welcome { index: body.u8()?, keepalive: body.u8()?, members: body.u16()? },
        REJECT => Message::Reject { reason: RejectReason::from_code(body.u8()?).ok_or(DecodeError::Malformed)? },
        DATA => {
            let source = body.u8()?;
            let seq = body.u16()?;
            let fragment = body.u8()?;
            let count = body.u8()?;
            let payload = body.rest().to_vec();
            if count == 0 || fragment >= count || payload.is_empty() || payload.len() > MAX_FRAGMENT {
                return Err(DecodeError::Malformed);
            }
            Message::Data { source, seq, fragment, count, payload }
        }
        KEEPALIVE => Message::Keepalive { stamp: body.u32()? },
        ACK => Message::Ack { stamp: body.u32()?, members: body.u16()? },
        LEAVE => Message::Leave,
        _ => return Err(DecodeError::Malformed),
    };
    body.end()?;
    Ok(Packet { token, message })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(token: u64, message: Message) {
        let bytes = encode(token, &message);
        assert_eq!(decode(&bytes), Ok(Packet { token, message: message.clone() }), "{:?}", message);
        // Anything cut short is refused.
        for len in 0..bytes.len() {
            if let Ok(packet) = decode(&bytes[..len]) {
                // Only DATA may lose bytes and still be one.
                assert!(matches!(packet.message, Message::Data { .. }), "{:?} cut to {}", message, len);
            }
        }
    }

    #[test]
    fn every_message_round_trips() {
        let rooms = vec![RoomInfo { name: "doom".into(), members: 3 }, RoomInfo { name: "".into(), members: 0 }];
        round_trip(0, Message::Discover);
        round_trip(0, Message::Offer { port: 21213, password: true, name: "den".into(), rooms });
        round_trip(0, Message::Hello { client_id: 0x0123_4567_89AB_CDEF });
        round_trip(0, Message::Challenge { cookie: [7; 16], password: false });
        round_trip(0, Message::Join { client_id: 5, cookie: [9; 16], room: "duke".into(), proof: [3; 32] });
        round_trip(42, Message::Welcome { index: 7, keepalive: 5, members: 2 });
        for reason in [
            RejectReason::Password,
            RejectReason::Full,
            RejectReason::Cookie,
            RejectReason::Version,
            RejectReason::Unknown,
        ] {
            round_trip(0, Message::Reject { reason });
        }
        round_trip(42, Message::Data { source: 3, seq: 65535, fragment: 1, count: 2, payload: vec![1, 2, 3] });
        round_trip(42, Message::Keepalive { stamp: 123_456 });
        round_trip(42, Message::Ack { stamp: 123_456, members: 4 });
        round_trip(42, Message::Leave);
    }

    #[test]
    fn requests_are_as_long_as_their_answers() {
        let many = (0..100).map(|i| RoomInfo { name: format!("room number {:>20}", i), members: i }).collect();
        let offer = encode(0, &Message::Offer { port: 1, password: true, name: "x".repeat(40), rooms: many });
        assert!(offer.len() <= encode(0, &Message::Discover).len());
        let Ok(Packet { message: Message::Offer { name, rooms, .. }, .. }) = decode(&offer) else { panic!() };
        assert_eq!(name.len(), MAX_NAME);
        assert!(!rooms.is_empty());
        let challenge = encode(0, &Message::Challenge { cookie: [0; 16], password: true });
        assert!(challenge.len() <= encode(0, &Message::Hello { client_id: 0 }).len());
    }

    #[test]
    fn refuses_what_is_not_a_datagram_of_this_version() {
        assert_eq!(decode(b"hello world, this is not it"), Err(DecodeError::Malformed));
        let mut bytes = encode(1, &Message::Leave);
        bytes[4] = 9;
        assert_eq!(decode(&bytes), Err(DecodeError::Version(9)));
        let mut bytes = encode(1, &Message::Leave);
        bytes[5] = 99;
        assert_eq!(decode(&bytes), Err(DecodeError::Malformed));
        // Trailing bytes, a bad fragment number, an empty or oversized
        // fragment, a bad flag, a bad reason.
        let mut bytes = encode(1, &Message::Leave);
        bytes.push(0);
        assert_eq!(decode(&bytes), Err(DecodeError::Malformed));
        let data = |fragment, count, len| {
            encode(1, &Message::Data { source: 0, seq: 0, fragment, count, payload: vec![0; len] })
        };
        assert!(decode(&data(2, 2, 10)).is_err());
        assert!(decode(&data(0, 0, 10)).is_err());
        assert!(decode(&data(0, 1, 0)).is_err());
        assert!(decode(&data(0, 1, MAX_FRAGMENT + 1)).is_err());
        assert!(decode(&data(0, 1, MAX_FRAGMENT)).is_ok());
        let mut bytes = encode(0, &Message::Challenge { cookie: [0; 16], password: true });
        *bytes.last_mut().unwrap() = 2;
        assert!(decode(&bytes).is_err());
        let mut bytes = encode(0, &Message::Reject { reason: RejectReason::Full });
        *bytes.last_mut().unwrap() = 0;
        assert!(decode(&bytes).is_err());
    }
}
