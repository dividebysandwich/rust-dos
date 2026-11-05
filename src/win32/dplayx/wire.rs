//! What DirectPlay's objects send each other: packets on one IPX socket of
//! the built-in IPX driver (`SOCKET`), so they travel between instances
//! the way a DOS game's IPX packets do, through the LAN room's relay.
//!
//! The protocol is rust-dos's own, not Windows' DirectPlay wire format.
//! Every packet starts with a header: a magic number with the protocol's
//! version, the packet's kind, the endpoints it is from and to (each
//! DirectPlay object is one, by a random number, on its instance's IPX
//! node), and for the packets between two endpoints (link.rs) a sequence
//! number, an acknowledgement, the sender's number for its link and the
//! oldest piece it still waits to have acknowledged. Its body is one of:
//!
//! - an enumeration request, broadcast to every endpoint, and the replies
//!   of the sessions' hosts;
//! - an acknowledgement alone, which also keeps a quiet link alive;
//! - a piece of a reliable message (`Msg`), in order on its link;
//! - a datagram: a whole `Msg` that may be lost, for a player's message
//!   sent without DPSEND_GUARANTEED;
//! - a request for the players and groups of a session the sender isn't
//!   in (EnumPlayers with DPENUMPLAYERS_SESSION), and the host's answer.
//!
//! Numbers are little-endian, strings UTF-16 with their length first.

use super::session::{Entity, Info, Member};
use crate::net::frame::Mac;

/// The IPX socket DirectPlay's packets go to and come from, on every
/// instance. It is in the dynamic range, high above the numbers the
/// driver gives programs first (4002h up), and the driver keeps it from
/// them while DirectPlay has it.
pub const SOCKET: u16 = 0x7D50;

const MAGIC: [u8; 3] = *b"rDP";
const VERSION: u8 = 2;
pub const HEADER: usize = 30;

/// The kinds of packets.
pub const ENUM_REQUEST: u8 = 1;
pub const ENUM_REPLY: u8 = 2;
pub const ACK: u8 = 3;
pub const RELIABLE: u8 = 4;
pub const DATAGRAM: u8 = 5;
pub const ROSTER_REQUEST: u8 = 6;
pub const ROSTER_REPLY: u8 = 7;

/// A DirectPlay object's address: its instance's IPX node and its own
/// number there.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Addr {
    pub node: Mac,
    pub endpoint: u32,
}

crate::state_fields!(Addr { node, endpoint });

/// A packet's header.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Header {
    pub kind: u8,
    pub from: u32,
    /// 0 for every endpoint.
    pub to: u32,
    pub seq: u32,
    pub ack: u32,
    /// The sender's link (0 for a packet of none) and its base
    /// (`Link::base`).
    pub link: u32,
    pub base: u32,
}

impl Header {
    pub fn write(&self, body: &[u8]) -> Vec<u8> {
        let mut p = Vec::with_capacity(HEADER + body.len());
        p.extend_from_slice(&MAGIC);
        p.push(VERSION);
        p.push(self.kind);
        p.push(0);
        p.extend_from_slice(&self.from.to_le_bytes());
        p.extend_from_slice(&self.to.to_le_bytes());
        p.extend_from_slice(&self.seq.to_le_bytes());
        p.extend_from_slice(&self.ack.to_le_bytes());
        p.extend_from_slice(&self.link.to_le_bytes());
        p.extend_from_slice(&self.base.to_le_bytes());
        p.extend_from_slice(body);
        p
    }

    /// The header of `packet` and its body, or None if it isn't one of
    /// this version's packets.
    pub fn read(packet: &[u8]) -> Option<(Header, &[u8])> {
        if packet.len() < HEADER || packet[..3] != MAGIC || packet[3] != VERSION {
            return None;
        }
        let word = |at: usize| u32::from_le_bytes(packet[at..at + 4].try_into().unwrap());
        let header = Header {
            kind: packet[4],
            from: word(6),
            to: word(10),
            seq: word(14),
            ack: word(18),
            link: word(22),
            base: word(26),
        };
        Some((header, &packet[HEADER..]))
    }
}

/// A message between endpoints: reliable, in order, or a datagram.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Msg {
    /// A request to join the session `instance`.
    Join {
        instance: [u8; 16],
        password: Vec<u16>,
    },
    /// The host's answer: DP_OK and the session, with the member number
    /// the joiner gets and everyone in it, or an error.
    Joined {
        result: u32,
        info: Info,
        password: Vec<u16>,
        member: u16,
        key: u32,
        members: Vec<Member>,
        entities: Vec<Entity>,
    },
    /// The host took in a member, whom the others then hear from.
    MemberJoined {
        member: Member,
    },
    /// The sender left the session.
    Leave,
    /// A member became the host, as the one with the lowest number left.
    NewHost {
        member: u16,
    },
    /// A player or group, made or told again (to a member who joined).
    Create {
        entity: Entity,
    },
    Destroy {
        id: u32,
    },
    AddToGroup {
        group: u32,
        player: u32,
    },
    DeleteFromGroup {
        group: u32,
        player: u32,
    },
    AddGroupToGroup {
        parent: u32,
        group: u32,
    },
    DeleteGroupFromGroup {
        parent: u32,
        group: u32,
    },
    SetName {
        id: u32,
        short: Option<Vec<u16>>,
        long: Option<Vec<u16>>,
    },
    SetData {
        id: u32,
        data: Vec<u8>,
    },
    SetDesc {
        info: Info,
        password: Vec<u16>,
    },
    SetGroupOwner {
        group: u32,
        owner: u32,
    },
    Chat {
        from: u32,
        to: u32,
        flags: u32,
        text: Vec<u16>,
    },
    /// A player's message to `to` (a player, a group or all), for the
    /// receiving endpoint's players among `recipients`.
    Data {
        from: u32,
        to: u32,
        recipients: Vec<u32>,
        data: Vec<u8>,
    },
}

/// Bytes being written.
#[derive(Default)]
pub struct Writer(pub Vec<u8>);

impl Writer {
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.u32(v.len() as u32);
        self.0.extend_from_slice(v);
        self
    }
    pub fn raw(&mut self, v: &[u8]) -> &mut Self {
        self.0.extend_from_slice(v);
        self
    }
    pub fn text(&mut self, v: &[u16]) -> &mut Self {
        self.u32(v.len() as u32);
        for &c in v {
            self.u16(c);
        }
        self
    }
    pub fn opt_text(&mut self, v: &Option<Vec<u16>>) -> &mut Self {
        match v {
            Some(t) => self.u8(1).text(t),
            None => self.u8(0),
        }
    }
    pub fn ids(&mut self, v: &[u32]) -> &mut Self {
        self.u32(v.len() as u32);
        for &id in v {
            self.u32(id);
        }
        self
    }
}

/// Bytes being read: every read fails once one ran past the end.
pub struct Reader<'a>(pub &'a [u8]);

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        if self.0.len() < n {
            return None;
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Some(head)
    }
    pub fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    pub fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    pub fn guid(&mut self) -> Option<[u8; 16]> {
        self.take(16)?.try_into().ok()
    }
    pub fn mac(&mut self) -> Option<Mac> {
        Some(Mac(self.take(6)?.try_into().ok()?))
    }
    /// A count, which can't be more than the bytes left divided by the
    /// size of what it counts.
    fn count(&mut self, unit: usize) -> Option<usize> {
        let n = self.u32()? as usize;
        (n.checked_mul(unit)? <= self.0.len()).then_some(n)
    }
    pub fn bytes(&mut self) -> Option<Vec<u8>> {
        let n = self.count(1)?;
        Some(self.take(n)?.to_vec())
    }
    pub fn text(&mut self) -> Option<Vec<u16>> {
        let n = self.count(2)?;
        (0..n).map(|_| self.u16()).collect()
    }
    pub fn opt_text(&mut self) -> Option<Option<Vec<u16>>> {
        match self.u8()? {
            0 => Some(None),
            _ => Some(Some(self.text()?)),
        }
    }
    pub fn ids(&mut self) -> Option<Vec<u32>> {
        let n = self.count(4)?;
        (0..n).map(|_| self.u32()).collect()
    }
}

impl Info {
    pub fn write(&self, w: &mut Writer) {
        w.u32(self.flags)
            .raw(&self.instance)
            .raw(&self.application)
            .u32(self.max_players)
            .u32(self.current_players)
            .opt_text(&self.name)
            .u8(self.has_password as u8);
        for &u in &self.user {
            w.u32(u);
        }
    }

    pub fn read(r: &mut Reader) -> Option<Info> {
        Some(Info {
            flags: r.u32()?,
            instance: r.guid()?,
            application: r.guid()?,
            max_players: r.u32()?,
            current_players: r.u32()?,
            name: r.opt_text()?,
            has_password: r.u8()? != 0,
            user: [r.u32()?, r.u32()?, r.u32()?, r.u32()?],
        })
    }
}

impl Member {
    fn write(&self, w: &mut Writer) {
        w.raw(&self.addr.node.0)
            .u32(self.addr.endpoint)
            .u16(self.index);
    }

    fn read(r: &mut Reader) -> Option<Member> {
        Some(Member {
            addr: Addr {
                node: r.mac()?,
                endpoint: r.u32()?,
            },
            index: r.u16()?,
        })
    }
}

impl Entity {
    fn write(&self, w: &mut Writer) {
        w.u32(self.id)
            .u8(self.group as u8)
            .u32(self.flags)
            .opt_text(&self.short)
            .opt_text(&self.long)
            .bytes(&self.data)
            .u32(self.parent)
            .raw(&self.owner.node.0)
            .u32(self.owner.endpoint)
            .ids(&self.players)
            .ids(&self.groups)
            .u32(self.group_owner);
    }

    fn read(r: &mut Reader) -> Option<Entity> {
        Some(Entity {
            id: r.u32()?,
            group: r.u8()? != 0,
            flags: r.u32()?,
            short: r.opt_text()?,
            long: r.opt_text()?,
            data: r.bytes()?,
            parent: r.u32()?,
            owner: Addr {
                node: r.mac()?,
                endpoint: r.u32()?,
            },
            players: r.ids()?,
            groups: r.ids()?,
            group_owner: r.u32()?,
            ..Entity::default()
        })
    }
}

/// The tags of `Msg`'s variants.
mod tag {
    pub const JOIN: u8 = 1;
    pub const JOINED: u8 = 2;
    pub const MEMBER_JOINED: u8 = 3;
    pub const LEAVE: u8 = 4;
    pub const NEW_HOST: u8 = 5;
    pub const CREATE: u8 = 6;
    pub const DESTROY: u8 = 7;
    pub const ADD_TO_GROUP: u8 = 8;
    pub const DELETE_FROM_GROUP: u8 = 9;
    pub const ADD_GROUP_TO_GROUP: u8 = 10;
    pub const DELETE_GROUP_FROM_GROUP: u8 = 11;
    pub const SET_NAME: u8 = 12;
    pub const SET_DATA: u8 = 13;
    pub const SET_DESC: u8 = 14;
    pub const SET_GROUP_OWNER: u8 = 15;
    pub const CHAT: u8 = 16;
    pub const DATA: u8 = 17;
}

impl Msg {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::default();
        match self {
            Msg::Join { instance, password } => {
                w.u8(tag::JOIN).raw(instance).text(password);
            }
            Msg::Joined {
                result,
                info,
                password,
                member,
                key,
                members,
                entities,
            } => {
                w.u8(tag::JOINED).u32(*result);
                info.write(&mut w);
                w.text(password).u16(*member).u32(*key);
                w.u32(members.len() as u32);
                for m in members {
                    m.write(&mut w);
                }
                w.u32(entities.len() as u32);
                for e in entities {
                    e.write(&mut w);
                }
            }
            Msg::MemberJoined { member } => {
                w.u8(tag::MEMBER_JOINED);
                member.write(&mut w);
            }
            Msg::Leave => {
                w.u8(tag::LEAVE);
            }
            Msg::NewHost { member } => {
                w.u8(tag::NEW_HOST).u16(*member);
            }
            Msg::Create { entity } => {
                w.u8(tag::CREATE);
                entity.write(&mut w);
            }
            Msg::Destroy { id } => {
                w.u8(tag::DESTROY).u32(*id);
            }
            Msg::AddToGroup { group, player } => {
                w.u8(tag::ADD_TO_GROUP).u32(*group).u32(*player);
            }
            Msg::DeleteFromGroup { group, player } => {
                w.u8(tag::DELETE_FROM_GROUP).u32(*group).u32(*player);
            }
            Msg::AddGroupToGroup { parent, group } => {
                w.u8(tag::ADD_GROUP_TO_GROUP).u32(*parent).u32(*group);
            }
            Msg::DeleteGroupFromGroup { parent, group } => {
                w.u8(tag::DELETE_GROUP_FROM_GROUP).u32(*parent).u32(*group);
            }
            Msg::SetName { id, short, long } => {
                w.u8(tag::SET_NAME).u32(*id).opt_text(short).opt_text(long);
            }
            Msg::SetData { id, data } => {
                w.u8(tag::SET_DATA).u32(*id).bytes(data);
            }
            Msg::SetDesc { info, password } => {
                w.u8(tag::SET_DESC);
                info.write(&mut w);
                w.text(password);
            }
            Msg::SetGroupOwner { group, owner } => {
                w.u8(tag::SET_GROUP_OWNER).u32(*group).u32(*owner);
            }
            Msg::Chat {
                from,
                to,
                flags,
                text,
            } => {
                w.u8(tag::CHAT).u32(*from).u32(*to).u32(*flags).text(text);
            }
            Msg::Data {
                from,
                to,
                recipients,
                data,
            } => {
                w.u8(tag::DATA)
                    .u32(*from)
                    .u32(*to)
                    .ids(recipients)
                    .bytes(data);
            }
        }
        w.0
    }

    /// The message in `bytes`, or None if it isn't a whole one.
    pub fn decode(bytes: &[u8]) -> Option<Msg> {
        let mut r = Reader(bytes);
        let msg = match r.u8()? {
            tag::JOIN => Msg::Join {
                instance: r.guid()?,
                password: r.text()?,
            },
            tag::JOINED => {
                let result = r.u32()?;
                let info = Info::read(&mut r)?;
                let password = r.text()?;
                let member = r.u16()?;
                let key = r.u32()?;
                let n = r.count(12)?;
                let members = (0..n)
                    .map(|_| Member::read(&mut r))
                    .collect::<Option<_>>()?;
                let n = r.u32()?;
                let entities = (0..n)
                    .map(|_| Entity::read(&mut r))
                    .collect::<Option<_>>()?;
                Msg::Joined {
                    result,
                    info,
                    password,
                    member,
                    key,
                    members,
                    entities,
                }
            }
            tag::MEMBER_JOINED => Msg::MemberJoined {
                member: Member::read(&mut r)?,
            },
            tag::LEAVE => Msg::Leave,
            tag::NEW_HOST => Msg::NewHost { member: r.u16()? },
            tag::CREATE => Msg::Create {
                entity: Entity::read(&mut r)?,
            },
            tag::DESTROY => Msg::Destroy { id: r.u32()? },
            tag::ADD_TO_GROUP => Msg::AddToGroup {
                group: r.u32()?,
                player: r.u32()?,
            },
            tag::DELETE_FROM_GROUP => Msg::DeleteFromGroup {
                group: r.u32()?,
                player: r.u32()?,
            },
            tag::ADD_GROUP_TO_GROUP => Msg::AddGroupToGroup {
                parent: r.u32()?,
                group: r.u32()?,
            },
            tag::DELETE_GROUP_FROM_GROUP => Msg::DeleteGroupFromGroup {
                parent: r.u32()?,
                group: r.u32()?,
            },
            tag::SET_NAME => Msg::SetName {
                id: r.u32()?,
                short: r.opt_text()?,
                long: r.opt_text()?,
            },
            tag::SET_DATA => Msg::SetData {
                id: r.u32()?,
                data: r.bytes()?,
            },
            tag::SET_DESC => Msg::SetDesc {
                info: Info::read(&mut r)?,
                password: r.text()?,
            },
            tag::SET_GROUP_OWNER => Msg::SetGroupOwner {
                group: r.u32()?,
                owner: r.u32()?,
            },
            tag::CHAT => Msg::Chat {
                from: r.u32()?,
                to: r.u32()?,
                flags: r.u32()?,
                text: r.text()?,
            },
            tag::DATA => Msg::Data {
                from: r.u32()?,
                to: r.u32()?,
                recipients: r.ids()?,
                data: r.bytes()?,
            },
            _ => return None,
        };
        r.0.is_empty().then_some(msg)
    }
}

/// An enumeration request: the application whose sessions are wanted
/// (GUID_NULL for all) and the password given, which a private session
/// needs to answer.
pub fn enum_request(application: &[u8; 16], password: &[u16]) -> Vec<u8> {
    let mut w = Writer::default();
    w.raw(application).text(password);
    w.0
}

pub fn read_enum_request(body: &[u8]) -> Option<([u8; 16], Vec<u16>)> {
    let mut r = Reader(body);
    Some((r.guid()?, r.text()?))
}

pub fn enum_reply(info: &Info) -> Vec<u8> {
    let mut w = Writer::default();
    info.write(&mut w);
    w.0
}

pub fn read_enum_reply(body: &[u8]) -> Option<Info> {
    Info::read(&mut Reader(body))
}

pub fn roster_request(instance: &[u8; 16]) -> Vec<u8> {
    instance.to_vec()
}

pub fn read_roster_request(body: &[u8]) -> Option<[u8; 16]> {
    body.try_into().ok()
}

/// A host's players and groups, as many as fit in `room` bytes.
pub fn roster_reply(instance: &[u8; 16], entities: &[Entity], room: usize) -> Vec<u8> {
    let mut w = Writer::default();
    w.raw(instance);
    let mut items = Vec::new();
    for e in entities {
        let mut one = Writer::default();
        e.write(&mut one);
        if w.0.len() + 4 + items.iter().map(Vec::len).sum::<usize>() + one.0.len() > room {
            break;
        }
        items.push(one.0);
    }
    w.u32(items.len() as u32);
    for item in items {
        w.raw(&item);
    }
    w.0
}

pub fn read_roster_reply(body: &[u8]) -> Option<([u8; 16], Vec<Entity>)> {
    let mut r = Reader(body);
    let instance = r.guid()?;
    let n = r.u32()?;
    let entities = (0..n)
        .map(|_| Entity::read(&mut r))
        .collect::<Option<_>>()?;
    Some((instance, entities))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_and_messages_round_trip() {
        let header = Header {
            kind: RELIABLE,
            from: 7,
            to: 9,
            seq: 3,
            ack: 2,
            link: 4,
            base: 3,
        };
        let packet = header.write(b"body");
        assert_eq!(Header::read(&packet), Some((header, &b"body"[..])));
        assert_eq!(Header::read(&packet[..HEADER - 1]), None);
        let mut other = packet.clone();
        other[3] = 1;
        assert_eq!(Header::read(&other), None, "another version");

        let owner = Addr {
            node: Mac([2, 0, 0, 0, 0, 1]),
            endpoint: 5,
        };
        let entity = Entity {
            id: 0x1234,
            group: true,
            flags: 8,
            short: Some("Ann".encode_utf16().collect()),
            long: None,
            data: vec![1, 2, 3],
            parent: 4,
            owner,
            players: vec![1, 2],
            groups: vec![3],
            group_owner: 1,
            ..Entity::default()
        };
        let info = Info {
            flags: 4,
            instance: [1; 16],
            application: [2; 16],
            max_players: 8,
            current_players: 2,
            name: Some("Race".encode_utf16().collect()),
            has_password: true,
            user: [1, 2, 3, 4],
        };
        let messages = [
            Msg::Join {
                instance: [3; 16],
                password: vec![65],
            },
            Msg::Joined {
                result: 0,
                info: info.clone(),
                password: vec![],
                member: 2,
                key: 99,
                members: vec![Member {
                    addr: owner,
                    index: 1,
                }],
                entities: vec![entity.clone()],
            },
            Msg::Create { entity },
            Msg::SetName {
                id: 1,
                short: None,
                long: Some(vec![66]),
            },
            Msg::Data {
                from: 1,
                to: 0,
                recipients: vec![2, 3],
                data: vec![9; 300],
            },
            Msg::Leave,
            Msg::SetDesc {
                info,
                password: vec![],
            },
        ];
        for msg in messages {
            let bytes = msg.encode();
            assert_eq!(Msg::decode(&bytes), Some(msg.clone()));
            assert_eq!(Msg::decode(&bytes[..bytes.len() - 1]), None);
        }
        assert_eq!(Msg::decode(&[0xEE]), None);
        // A count larger than what follows is refused before anything is
        // made of it.
        assert_eq!(
            Msg::decode(&[tag::DATA, 0, 0, 0, 0, 0, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF]),
            None
        );
    }
}
