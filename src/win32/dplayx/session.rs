//! A DirectPlay object's network side (`Play`): the sessions it found,
//! the session it is in, with its members, players and groups, the
//! messages waiting for its players, and its links to the other members.
//! It works on its own: the glue (`net.rs`) hands it the packets that
//! came and the time, and sends the packets it leaves in its outbox.
//!
//! A session is hosted by the member that made it, which answers the
//! enumeration requests other endpoints broadcast and takes in members:
//! it gives each a number and the session as it is, and tells the others
//! about the newcomer. From then on every member talks to every other
//! directly. A member that makes, changes or destroys a player or group
//! tells all the others itself, so what one member does reaches each
//! other member in the order it did it, before the messages its players
//! send after. A member also tells a newcomer about its own players and
//! groups, which the host's picture may have missed if it made them while
//! the newcomer joined. Player and group ids are unique without asking
//! anyone: each has the member's number in it, mixed with a number the
//! session got at random.
//!
//! What the players get follows DirectPlay on Windows 98, as Wine's
//! tests recorded it: a system message goes to each local player of the
//! object, the newest first, and to none if it has none; a message to a
//! group goes to the group's players but not to those of the groups added
//! to it; the sender doesn't get its own message.
//!
//! When the host leaves a session made with DPSESSION_MIGRATEHOST, the
//! member with the lowest number becomes the host, and its players get
//! DPSYS_HOST. Without it, the others play on, and no one else can join.
//! A member who loses every other member while it isn't the host has
//! lost the session (DPSYS_SESSIONLOST).

use super::link::Link;
use super::wire::{self, Addr, Header, Msg};
use crate::net::frame::Mac;
use std::collections::VecDeque;

pub const DPID_SYSMSG: u32 = 0;
pub const DPID_ALLPLAYERS: u32 = 0;
pub const DPID_SERVERPLAYER: u32 = 1;
pub const DPID_UNKNOWN: u32 = 0xFFFF_FFFF;

pub const DP_OK: u32 = 0;
const fn dperr(code: u32) -> u32 {
    0x8877_0000 | code
}
pub const DPERR_ACCESSDENIED: u32 = dperr(10);
pub const DPERR_CANTADDPLAYER: u32 = dperr(40);
pub const DPERR_CANTCREATEPLAYER: u32 = dperr(60);
pub const DPERR_INVALIDPLAYER: u32 = dperr(150);
pub const DPERR_INVALIDGROUP: u32 = dperr(155);
pub const DPERR_INVALIDPARAMS: u32 = 0x8007_0057;
pub const DPERR_NOSESSIONS: u32 = dperr(220);
pub const DPERR_TIMEOUT: u32 = dperr(240);
pub const DPERR_NONEWPLAYERS: u32 = dperr(330);
pub const DPERR_INVALIDPASSWORD: u32 = dperr(340);

/// Session flags.
pub const DPSESSION_NEWPLAYERSDISABLED: u32 = 0x1;
pub const DPSESSION_MIGRATEHOST: u32 = 0x4;
pub const DPSESSION_JOINDISABLED: u32 = 0x20;
pub const DPSESSION_NODATAMESSAGES: u32 = 0x80;
pub const DPSESSION_PRIVATE: u32 = 0x200;
pub const DPSESSION_PASSWORDREQUIRED: u32 = 0x400;

/// Player and group flags.
pub const DPPLAYER_LOCAL: u32 = 0x8;
pub const DPPLAYER_SERVERPLAYER: u32 = 0x100;
pub const DPPLAYER_SPECTATOR: u32 = 0x200;
pub const DPGROUP_STAGINGAREA: u32 = 0x800;
pub const DPGROUP_HIDDEN: u32 = 0x1000;

/// How long a sync EnumSessions or a join waits by default, and how
/// often an async EnumSessions asks again, and how long a session it
/// found stays in its list without answering.
pub const ENUM_TIMEOUT: u64 = 1_500_000;
pub const JOIN_TIMEOUT: u64 = 10_000_000;
const ASYNC_EVERY: u64 = 1_500_000;
const FOUND_STALE: u64 = 5_000_000;
/// The most messages waiting for an object's players; more drop the
/// oldest, as a queue nobody reads would otherwise grow for ever.
const QUEUE_LIMIT: usize = 16_384;

/// What a session's description says, as it goes between members.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Info {
    pub flags: u32,
    pub instance: [u8; 16],
    pub application: [u8; 16],
    pub max_players: u32,
    pub current_players: u32,
    pub name: Option<Vec<u16>>,
    pub has_password: bool,
    pub user: [u32; 4],
}

crate::state_fields!(Info {
    flags,
    instance,
    application,
    max_players,
    current_players,
    name,
    has_password,
    user
});

/// A member of a session: an endpoint, by its number in the session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Member {
    pub addr: Addr,
    pub index: u16,
}

crate::state_fields!(Member { addr, index });

/// A player or a group.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entity {
    pub id: u32,
    pub group: bool,
    /// DPPLAYER_SERVERPLAYER and SPECTATOR, or the group's flags.
    pub flags: u32,
    pub short: Option<Vec<u16>>,
    pub long: Option<Vec<u16>>,
    /// The data every member sees (DPSET_REMOTE).
    pub data: Vec<u8>,
    /// The group it was made in (CreateGroupInGroup), or 0.
    pub parent: u32,
    /// The endpoint that made it.
    pub owner: Addr,
    /// A group's players, and the groups added to it (AddGroupToGroup),
    /// in the order they came in, and its owner (SetGroupOwner).
    pub players: Vec<u32>,
    pub groups: Vec<u32>,
    pub group_owner: u32,
    /// What only this object has of it: whether it is this object's own,
    /// the data only it sees (DPSET_LOCAL), and the event set when one of
    /// its messages comes, by its kernel object, which it holds; the
    /// event's handle too when DirectPlay made the event (IDirectPlay's
    /// CreatePlayer).
    pub local: bool,
    pub local_data: Vec<u8>,
    pub event: u32,
    pub own_event: u32,
}

crate::state_fields!(Entity {
    id,
    group,
    flags,
    short,
    long,
    data,
    parent,
    owner,
    players,
    groups,
    group_owner,
    local,
    local_data,
    event,
    own_event
});

impl Entity {
    /// As it goes to other members: without what is this object's own.
    fn public(&self) -> Entity {
        Entity {
            local: false,
            local_data: Vec::new(),
            event: 0,
            own_event: 0,
            ..self.clone()
        }
    }

    /// The flags its system messages and enumerations report.
    pub fn reported_flags(&self) -> u32 {
        self.flags | if self.local { DPPLAYER_LOCAL } else { 0 }
    }
}

/// A system message, as it is kept until a player receives it: the
/// object's interface writes it out in its own form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sys {
    Create {
        entity: Entity,
        current: u32,
    },
    Destroy {
        entity: Entity,
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
    SetData {
        entity: Entity,
    },
    SetName {
        entity: Entity,
    },
    SetDesc {
        info: Info,
        password: Vec<u16>,
    },
    Host {},
    SessionLost {},
    Chat {
        from: u32,
        to_player: u32,
        to_group: u32,
        flags: u32,
        text: Vec<u16>,
    },
    SetGroupOwner {
        group: u32,
        new: u32,
        old: u32,
    },
    SendComplete {
        from: u32,
        to: u32,
        flags: u32,
        priority: u32,
        timeout: u32,
        context: u32,
        id: u32,
        time: u32,
    },
}

impl Default for Sys {
    fn default() -> Self {
        Sys::Host {}
    }
}

crate::win32::state_variants!(Sys {
    Create { entity, current },
    Destroy { entity },
    AddToGroup { group, player },
    DeleteFromGroup { group, player },
    AddGroupToGroup { parent, group },
    DeleteGroupFromGroup { parent, group },
    SetData { entity },
    SetName { entity },
    SetDesc { info, password },
    Host {},
    SessionLost {},
    Chat {
        from,
        to_player,
        to_group,
        flags,
        text
    },
    SetGroupOwner { group, new, old },
    SendComplete {
        from,
        to,
        flags,
        priority,
        timeout,
        context,
        id,
        time
    },
});

/// What a message carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Body {
    Data { data: Vec<u8> },
    System { sys: Sys },
}

impl Default for Body {
    fn default() -> Self {
        Body::Data { data: Vec::new() }
    }
}

crate::win32::state_variants!(Body {
    Data { data },
    System { sys },
});

/// A message waiting for a local player: from a player, or from the
/// system (DPID_SYSMSG).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Queued {
    pub from: u32,
    pub to: u32,
    pub body: Body,
}

crate::state_fields!(Queued { from, to, body });

/// The session an object is in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Session {
    pub info: Info,
    pub password: Vec<u16>,
    /// The host's member number, 0 when there is none (it left a session
    /// that doesn't migrate), and this object's.
    pub host: u16,
    pub me: u16,
    pub members: Vec<Member>,
    /// The players and groups, the oldest first.
    pub entities: Vec<Entity>,
    /// What ids are mixed with, how many this member made, and the host's
    /// next member number.
    pub key: u32,
    pub made: u16,
    pub next_member: u16,
    /// Whether every other member is gone (DPSYS_SESSIONLOST).
    pub lost: bool,
}

crate::state_fields!(Session {
    info,
    password,
    host,
    me,
    members,
    entities,
    key,
    made,
    next_member,
    lost
});

impl Session {
    pub fn is_host(&self) -> bool {
        self.host == self.me
    }

    pub fn entity(&self, id: u32) -> Option<&Entity> {
        self.entities.iter().find(|e| e.id == id)
    }

    pub fn entity_mut(&mut self, id: u32) -> Option<&mut Entity> {
        self.entities.iter_mut().find(|e| e.id == id)
    }

    pub fn player(&self, id: u32) -> Option<&Entity> {
        self.entity(id).filter(|e| !e.group)
    }

    pub fn group(&self, id: u32) -> Option<&Entity> {
        self.entity(id).filter(|e| e.group)
    }

    pub fn players(&self) -> u32 {
        self.entities.iter().filter(|e| !e.group).count() as u32
    }

    /// The local players, the newest first: the order their messages
    /// come in.
    pub fn local_players(&self) -> Vec<u32> {
        let local = self.entities.iter().rev().filter(|e| e.local && !e.group);
        local.map(|e| e.id).collect()
    }

    fn member_addr(&self, index: u16) -> Option<Addr> {
        let member = self.members.iter().find(|m| m.index == index)?;
        Some(member.addr)
    }

    /// The description as it is now, with its player count.
    pub fn current_info(&self) -> Info {
        Info {
            current_players: self.players(),
            has_password: !self.password.is_empty(),
            ..self.info.clone()
        }
    }

    /// A new id for a player or group of this member's.
    fn new_id(&mut self) -> u32 {
        loop {
            self.made = self.made.wrapping_add(1);
            let id = self.key ^ ((self.me as u32) << 16 | self.made as u32);
            let reserved = [DPID_SYSMSG, DPID_SERVERPLAYER, DPID_UNKNOWN].contains(&id);
            if !reserved && self.entity(id).is_none() {
                return id;
            }
        }
    }

    /// The players a message to `to` from `from` goes to: everyone but
    /// the sender for DPID_ALLPLAYERS, a group's players, or one player.
    pub fn recipients(&self, from: u32, to: u32) -> Result<Vec<u32>, u32> {
        if to == DPID_ALLPLAYERS {
            let all = self.entities.iter().filter(|e| !e.group && e.id != from);
            return Ok(all.map(|e| e.id).collect());
        }
        let target = self.entity(to).ok_or(DPERR_INVALIDPLAYER)?;
        if target.group {
            let players = target.players.iter().copied();
            return Ok(players.filter(|&p| p != from).collect());
        }
        Ok(vec![to])
    }
}

/// A session EnumSessions found: its description and its host, and its
/// players and groups once the host answered for them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Found {
    pub info: Info,
    pub host: Addr,
    pub heard: u64,
    pub roster: Option<Vec<Entity>>,
}

crate::state_fields!(Found {
    info,
    host,
    heard,
    roster
});

/// What EnumSessions looks for, and while it runs in the background
/// (DPENUMSESSIONS_ASYNC), when it asks again.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Search {
    pub application: [u8; 16],
    pub password: Vec<u16>,
    pub background: bool,
    pub next: u64,
}

crate::state_fields!(Search {
    application,
    password,
    background,
    next
});

/// A method that waits (a sync EnumSessions, an Open that joins): the
/// stack it was called with, which tells its call apart from a new one
/// when it runs again, and until when it waits.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Waiting {
    pub esp: u32,
    pub until: u64,
}

crate::state_fields!(Waiting { esp, until });

/// A join on its way: the host asked, the session, and the answer once
/// it came.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Joining {
    pub host: Addr,
    pub instance: [u8; 16],
    pub until: u64,
    pub result: Option<u32>,
}

crate::state_fields!(Joining {
    host,
    instance,
    until,
    result
});

/// A DirectPlay object's network side.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Play {
    /// Its number among the endpoints of its node, from its first
    /// connection on.
    pub endpoint: u32,
    /// Its service provider: 0 none yet, else `super::IPX` or `TCPIP`.
    pub provider: u8,
    /// Its node, and the most a packet's body takes (net.rs keeps both
    /// as the IPX driver has them).
    pub node: Mac,
    pub room: u32,
    pub session: Option<Session>,
    pub found: Vec<Found>,
    pub search: Option<Search>,
    pub waiting: Option<Waiting>,
    pub joining: Option<Joining>,
    pub links: Vec<Link>,
    pub queue: VecDeque<Queued>,
    /// SendEx's message ids.
    pub sends: u32,
    /// The packets to send, to a node (Mac::BROADCAST for all), and the
    /// events to set.
    pub outbox: Vec<(Mac, Vec<u8>)>,
    pub signal: Vec<u32>,
    /// The events of players that went, whose holds the glue lets go,
    /// and the handles DirectPlay made for them, which it closes.
    pub released: Vec<(u32, u32)>,
}

crate::state_fields!(Play {
    endpoint,
    provider,
    node,
    room,
    session,
    found,
    search,
    waiting,
    joining,
    links,
    queue,
    sends,
} skip {
    outbox,
    signal,
    released
});

impl Play {
    pub fn me(&self) -> Addr {
        Addr {
            node: self.node,
            endpoint: self.endpoint,
        }
    }

    /// The player or group `id`: of the open session, or of a found
    /// session whose host told its players (EnumPlayers'
    /// DPENUMPLAYERS_SESSION).
    pub fn known(&self, id: u32) -> Option<&Entity> {
        let open = self.session.as_ref().and_then(|s| s.entity(id));
        let found = || {
            self.found
                .iter()
                .flat_map(|f| f.roster.iter().flatten())
                .find(|e| e.id == id)
        };
        open.or_else(found)
    }

    /// The room in a packet for a piece of a reliable message.
    fn piece_room(&self) -> usize {
        (self.room as usize).saturating_sub(wire::HEADER).max(64)
    }

    fn link(&mut self, peer: Addr, now: u64) -> &mut Link {
        let i = match self.links.iter().position(|l| l.peer == peer) {
            Some(i) => i,
            None => {
                self.links.push(Link::new(peer, now));
                self.links.len() - 1
            }
        };
        &mut self.links[i]
    }

    fn packet(&mut self, to: Addr, kind: u8, seq: u32, body: &[u8]) {
        let ack = self
            .links
            .iter()
            .find(|l| l.peer == to)
            .map_or(0, Link::ack);
        let header = Header {
            kind,
            from: self.endpoint,
            to: to.endpoint,
            seq,
            ack,
        };
        self.outbox.push((to.node, header.write(body)));
    }

    /// Send `msg` to `to` reliably.
    fn send(&mut self, to: Addr, msg: &Msg, now: u64) {
        let room = self.piece_room();
        self.link(to, now).queue(&msg.encode(), room);
    }

    /// Send `msg` to `to` as a datagram if it fits one, else reliably.
    fn send_datagram(&mut self, to: Addr, msg: &Msg, now: u64) {
        let bytes = msg.encode();
        if bytes.len() > self.piece_room() {
            self.send(to, msg, now);
            return;
        }
        self.packet(to, wire::DATAGRAM, 0, &bytes);
    }

    /// Send `msg` to every other member.
    fn tell_members(&mut self, msg: &Msg, now: u64) {
        let me = self.me();
        let members = self.session.iter().flat_map(|s| &s.members);
        let others: Vec<Addr> = members.map(|m| m.addr).filter(|&a| a != me).collect();
        for to in others {
            self.send(to, msg, now);
        }
    }

    /// Queue a message for the local player `to`, and set its event.
    fn deliver(&mut self, from: u32, to: u32, body: Body) {
        if self.queue.len() >= QUEUE_LIMIT {
            self.queue.pop_front();
        }
        self.queue.push_back(Queued { from, to, body });
        let event = self
            .session
            .as_ref()
            .and_then(|s| s.player(to))
            .map_or(0, |e| e.event);
        if event != 0 {
            self.signal.push(event);
        }
    }

    /// Tell each local player but `except` of `sys`, the newest first.
    fn tell(&mut self, sys: Sys, except: u32) {
        let players = self
            .session
            .as_ref()
            .map(Session::local_players)
            .unwrap_or_default();
        for to in players.into_iter().filter(|&p| p != except) {
            let body = Body::System { sys: sys.clone() };
            self.deliver(DPID_SYSMSG, to, body);
        }
    }

    /// Whether the session sends DPSYS_SETPLAYERORGROUPDATA and NAME.
    fn data_messages(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|s| s.info.flags & DPSESSION_NODATAMESSAGES == 0)
    }

    // --- Finding and joining sessions ---

    /// Ask every endpoint for its sessions of `application` (GUID_NULL
    /// for any). A sync search forgets what it found before.
    pub fn search(
        &mut self,
        application: [u8; 16],
        password: Vec<u16>,
        background: bool,
        now: u64,
    ) {
        if !background {
            self.found.clear();
        }
        let body = wire::enum_request(&application, &password);
        self.search = Some(Search {
            application,
            password,
            background,
            next: now + ASYNC_EVERY,
        });
        let header = Header {
            kind: wire::ENUM_REQUEST,
            from: self.endpoint,
            ..Header::default()
        };
        self.outbox.push((Mac::BROADCAST, header.write(&body)));
    }

    pub fn stop_search(&mut self) {
        self.search = None;
    }

    /// Join the session `found` names: the request goes; the answer comes
    /// later (`joining`).
    pub fn join(&mut self, found: &Found, password: Vec<u16>, now: u64) {
        let msg = Msg::Join {
            instance: found.info.instance,
            password,
        };
        self.joining = Some(Joining {
            host: found.host,
            instance: found.info.instance,
            until: now + JOIN_TIMEOUT,
            result: None,
        });
        self.send(found.host, &msg, now);
    }

    /// Make a session and host it.
    pub fn create(&mut self, info: Info, password: Vec<u16>, key: u32) {
        let me = Member {
            addr: self.me(),
            index: 1,
        };
        self.session = Some(Session {
            info: Info {
                flags: info.flags
                    | if password.is_empty() {
                        0
                    } else {
                        DPSESSION_PASSWORDREQUIRED
                    },
                ..info
            },
            password,
            host: 1,
            me: 1,
            members: vec![me],
            key,
            next_member: 2,
            ..Session::default()
        });
    }

    /// Leave the session: the others hear of it, the local players and
    /// groups go without messages.
    pub fn close(&mut self, now: u64) {
        if self.session.is_some() {
            self.tell_members(&Msg::Leave, now);
        }
        if let Some(session) = self.session.take() {
            for e in session.entities {
                if e.event != 0 || e.own_event != 0 {
                    self.released.push((e.event, e.own_event));
                }
            }
        }
        self.joining = None;
        self.waiting = None;
        self.queue.clear();
    }

    // --- What the local players do ---

    /// Make a player or group of this object's: its id.
    #[allow(clippy::too_many_arguments)]
    pub fn create_entity(
        &mut self,
        group: bool,
        flags: u32,
        short: Option<Vec<u16>>,
        long: Option<Vec<u16>>,
        data: Vec<u8>,
        parent: u32,
        event: u32,
        now: u64,
    ) -> Result<u32, u32> {
        let owner = self.me();
        let session = self.session.as_mut().ok_or(DPERR_INVALIDPARAMS)?;
        if parent != 0 && session.group(parent).is_none() {
            return Err(DPERR_INVALIDGROUP);
        }
        if !group {
            let disabled = session.info.flags & DPSESSION_NEWPLAYERSDISABLED != 0;
            let max = session.info.max_players;
            if disabled || (max != 0 && session.players() >= max) {
                return Err(DPERR_CANTADDPLAYER);
            }
        }
        let id = if !group && flags & DPPLAYER_SERVERPLAYER != 0 {
            if !session.is_host() || session.entity(DPID_SERVERPLAYER).is_some() {
                return Err(DPERR_CANTCREATEPLAYER);
            }
            DPID_SERVERPLAYER
        } else {
            session.new_id()
        };
        let current = if group { 0 } else { session.players() };
        let entity = Entity {
            id,
            group,
            flags,
            short,
            long,
            data,
            parent,
            owner,
            local: true,
            event,
            ..Entity::default()
        };
        session.entities.push(entity.clone());
        let public = entity.public();
        self.tell(Sys::Create { entity, current }, id);
        self.tell_members(&Msg::Create { entity: public }, now);
        Ok(id)
    }

    /// Destroy the player or group `id`.
    pub fn destroy(&mut self, id: u32, group: bool, now: u64) -> Result<(), u32> {
        let session = self.session.as_ref().ok_or(DPERR_INVALIDPARAMS)?;
        let e = session.entity(id).filter(|e| e.group == group);
        let e = e.ok_or(if group {
            DPERR_INVALIDGROUP
        } else {
            DPERR_INVALIDPLAYER
        })?;
        if !group && !e.local {
            return Err(DPERR_ACCESSDENIED);
        }
        self.remove(id);
        self.tell_members(&Msg::Destroy { id }, now);
        Ok(())
    }

    /// The player or group `id` goes: out of the groups it was in, and
    /// its groups' members with it; the local players are told.
    fn remove(&mut self, id: u32) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let Some(i) = session.entities.iter().position(|e| e.id == id) else {
            return;
        };
        let entity = session.entities.remove(i);
        let mut left = Vec::new();
        for g in session.entities.iter_mut().filter(|e| e.group) {
            if g.players.contains(&id) {
                g.players.retain(|&p| p != id);
                left.push(Sys::DeleteFromGroup {
                    group: g.id,
                    player: id,
                });
            }
            if g.groups.contains(&id) {
                g.groups.retain(|&p| p != id);
                left.push(Sys::DeleteGroupFromGroup {
                    parent: g.id,
                    group: id,
                });
            }
        }
        // Groups made in it go with it.
        let children: Vec<u32> = session
            .entities
            .iter()
            .filter(|e| e.group && e.parent == id)
            .map(|e| e.id)
            .collect();
        for sys in left {
            self.tell(sys, id);
        }
        if entity.event != 0 || entity.own_event != 0 {
            self.released.push((entity.event, entity.own_event));
        }
        self.tell(Sys::Destroy { entity }, id);
        for child in children {
            self.remove(child);
        }
    }

    /// Put the player (or group, `subgroup`) `member` into `group`.
    pub fn add_to_group(
        &mut self,
        group: u32,
        member: u32,
        subgroup: bool,
        now: u64,
    ) -> Result<(), u32> {
        let session = self.session.as_mut().ok_or(DPERR_INVALIDPARAMS)?;
        let ok = if subgroup {
            session.group(member).is_some() && member != group
        } else {
            session.player(member).is_some()
        };
        if !ok {
            return Err(if subgroup {
                DPERR_INVALIDGROUP
            } else {
                DPERR_INVALIDPLAYER
            });
        }
        let g = session
            .entity_mut(group)
            .filter(|e| e.group)
            .ok_or(DPERR_INVALIDGROUP)?;
        let list = if subgroup {
            &mut g.groups
        } else {
            &mut g.players
        };
        if list.contains(&member) {
            return Err(DPERR_INVALIDPARAMS);
        }
        list.push(member);
        let (sys, msg) = if subgroup {
            (
                Sys::AddGroupToGroup {
                    parent: group,
                    group: member,
                },
                Msg::AddGroupToGroup {
                    parent: group,
                    group: member,
                },
            )
        } else {
            (
                Sys::AddToGroup {
                    group,
                    player: member,
                },
                Msg::AddToGroup {
                    group,
                    player: member,
                },
            )
        };
        self.tell(sys, 0);
        self.tell_members(&msg, now);
        Ok(())
    }

    /// Take `member` out of `group`.
    pub fn delete_from_group(
        &mut self,
        group: u32,
        member: u32,
        subgroup: bool,
        now: u64,
    ) -> Result<(), u32> {
        let session = self.session.as_mut().ok_or(DPERR_INVALIDPARAMS)?;
        let g = session
            .entity_mut(group)
            .filter(|e| e.group)
            .ok_or(DPERR_INVALIDGROUP)?;
        let list = if subgroup {
            &mut g.groups
        } else {
            &mut g.players
        };
        let Some(i) = list.iter().position(|&m| m == member) else {
            return Err(if subgroup {
                DPERR_INVALIDGROUP
            } else {
                DPERR_INVALIDPLAYER
            });
        };
        list.remove(i);
        let (sys, msg) = if subgroup {
            (
                Sys::DeleteGroupFromGroup {
                    parent: group,
                    group: member,
                },
                Msg::DeleteGroupFromGroup {
                    parent: group,
                    group: member,
                },
            )
        } else {
            (
                Sys::DeleteFromGroup {
                    group,
                    player: member,
                },
                Msg::DeleteFromGroup {
                    group,
                    player: member,
                },
            )
        };
        self.tell(sys, 0);
        self.tell_members(&msg, now);
        Ok(())
    }

    /// Name the player or group `id`; with `local`, for this object
    /// alone.
    pub fn set_name(
        &mut self,
        id: u32,
        group: bool,
        short: Option<Vec<u16>>,
        long: Option<Vec<u16>>,
        local: bool,
        now: u64,
    ) -> Result<(), u32> {
        let session = self.session.as_mut().ok_or(DPERR_INVALIDPARAMS)?;
        let e = session.entity_mut(id).filter(|e| e.group == group);
        let e = e.ok_or(if group {
            DPERR_INVALIDGROUP
        } else {
            DPERR_INVALIDPLAYER
        })?;
        if !group && !e.local {
            return Err(DPERR_ACCESSDENIED);
        }
        e.short = short.clone();
        e.long = long.clone();
        if local {
            return Ok(());
        }
        let entity = e.clone();
        if self.data_messages() {
            self.tell(Sys::SetName { entity }, 0);
        }
        self.tell_members(&Msg::SetName { id, short, long }, now);
        Ok(())
    }

    /// Set the data of the player or group `id`: what all see, or with
    /// `local` what only this object sees.
    pub fn set_data(
        &mut self,
        id: u32,
        group: bool,
        data: Vec<u8>,
        local: bool,
        now: u64,
    ) -> Result<(), u32> {
        let session = self.session.as_mut().ok_or(DPERR_INVALIDPARAMS)?;
        let e = session.entity_mut(id).filter(|e| e.group == group);
        let e = e.ok_or(if group {
            DPERR_INVALIDGROUP
        } else {
            DPERR_INVALIDPLAYER
        })?;
        if !group && !e.local {
            return Err(DPERR_ACCESSDENIED);
        }
        if local {
            e.local_data = data;
            return Ok(());
        }
        e.data = data.clone();
        let entity = e.clone();
        if self.data_messages() {
            self.tell(Sys::SetData { entity }, 0);
        }
        self.tell_members(&Msg::SetData { id, data }, now);
        Ok(())
    }

    /// The host changes the session's description.
    pub fn set_desc(&mut self, info: Info, password: Vec<u16>, now: u64) -> Result<(), u32> {
        let session = self.session.as_mut().ok_or(DPERR_INVALIDPARAMS)?;
        if !session.is_host() {
            return Err(DPERR_ACCESSDENIED);
        }
        let old = &session.info;
        session.info = Info {
            instance: old.instance,
            application: old.application,
            flags: info.flags
                | if password.is_empty() {
                    0
                } else {
                    DPSESSION_PASSWORDREQUIRED
                },
            ..info
        };
        session.password = password.clone();
        let info = session.current_info();
        self.tell(
            Sys::SetDesc {
                info: info.clone(),
                password: password.clone(),
            },
            0,
        );
        self.tell_members(&Msg::SetDesc { info, password }, now);
        Ok(())
    }

    pub fn set_group_owner(&mut self, group: u32, owner: u32, now: u64) -> Result<(), u32> {
        let session = self.session.as_mut().ok_or(DPERR_INVALIDPARAMS)?;
        if owner != 0 && session.player(owner).is_none() {
            return Err(DPERR_INVALIDPLAYER);
        }
        let g = session
            .entity_mut(group)
            .filter(|e| e.group)
            .ok_or(DPERR_INVALIDGROUP)?;
        let old = std::mem::replace(&mut g.group_owner, owner);
        self.tell(
            Sys::SetGroupOwner {
                group,
                new: owner,
                old,
            },
            0,
        );
        self.tell_members(&Msg::SetGroupOwner { group, owner }, now);
        Ok(())
    }

    /// The local player `from` sends `data` to `to`.
    pub fn send_data(
        &mut self,
        from: u32,
        to: u32,
        data: Vec<u8>,
        guaranteed: bool,
        now: u64,
    ) -> Result<(), u32> {
        let session = self.session.as_ref().ok_or(DPERR_INVALIDPARAMS)?;
        if !session.player(from).is_some_and(|e| e.local) {
            return Err(DPERR_INVALIDPLAYER);
        }
        let recipients = session.recipients(from, to)?;
        let me = self.me();
        // To each endpoint, the players it has among the recipients.
        let mut remote: Vec<(Addr, Vec<u32>)> = Vec::new();
        for &id in &recipients {
            let Some(e) = session.player(id) else {
                continue;
            };
            if e.owner == me {
                continue;
            }
            match remote.iter_mut().find(|(a, _)| *a == e.owner) {
                Some((_, ids)) => ids.push(id),
                None => remote.push((e.owner, vec![id])),
            }
        }
        let local: Vec<u32> = session
            .local_players()
            .into_iter()
            .filter(|p| recipients.contains(p))
            .collect();
        for p in local {
            self.deliver(from, p, Body::Data { data: data.clone() });
        }
        for (addr, ids) in remote {
            let msg = Msg::Data {
                from,
                to,
                recipients: ids,
                data: data.clone(),
            };
            if guaranteed {
                self.send(addr, &msg, now);
            } else {
                self.send_datagram(addr, &msg, now);
            }
        }
        Ok(())
    }

    /// The local player `from` chats to `to`.
    pub fn chat(
        &mut self,
        from: u32,
        to: u32,
        flags: u32,
        text: Vec<u16>,
        now: u64,
    ) -> Result<(), u32> {
        let session = self.session.as_ref().ok_or(DPERR_INVALIDPARAMS)?;
        if !session.player(from).is_some_and(|e| e.local) {
            return Err(DPERR_INVALIDPLAYER);
        }
        session.recipients(from, to)?;
        let msg = Msg::Chat {
            from,
            to,
            flags,
            text,
        };
        self.chat_arrived(&msg);
        self.tell_members(&msg, now);
        Ok(())
    }

    fn chat_arrived(&mut self, msg: &Msg) {
        let (
            Msg::Chat {
                from,
                to,
                flags,
                text,
            },
            Some(session),
        ) = (msg, self.session.as_ref())
        else {
            return;
        };
        let Ok(recipients) = session.recipients(*from, *to) else {
            return;
        };
        let to_group = session.group(*to).map_or(0, |g| g.id);
        let local: Vec<u32> = session
            .local_players()
            .into_iter()
            .filter(|p| recipients.contains(p) && p != from)
            .collect();
        for p in local {
            let sys = Sys::Chat {
                from: *from,
                to_player: if to_group == 0 { *to } else { 0 },
                to_group,
                flags: *flags,
                text: text.clone(),
            };
            self.deliver(DPID_SYSMSG, p, Body::System { sys });
        }
    }

    // --- What comes from the network ---

    /// A packet from `from_node` came.
    pub fn handle(&mut self, from_node: Mac, packet: &[u8], now: u64) {
        let Some((header, body)) = Header::read(packet) else {
            return;
        };
        if header.to != 0 && header.to != self.endpoint {
            return;
        }
        let from = Addr {
            node: from_node,
            endpoint: header.from,
        };
        if from == self.me() {
            return;
        }
        match header.kind {
            wire::ENUM_REQUEST => self.enum_request(from, body),
            wire::ENUM_REPLY => self.enum_reply(from, body, now),
            wire::ROSTER_REQUEST => self.roster_request(from, body),
            wire::ROSTER_REPLY => {
                let Some((instance, entities)) = wire::read_roster_reply(body) else {
                    return;
                };
                let found = self
                    .found
                    .iter_mut()
                    .find(|f| f.info.instance == instance && f.host == from);
                if let Some(f) = found {
                    f.roster = Some(entities);
                }
            }
            wire::ACK | wire::RELIABLE | wire::DATAGRAM => {
                let known = self.links.iter().any(|l| l.peer == from);
                if !known && header.kind != wire::RELIABLE {
                    return;
                }
                let link = self.link(from, now);
                link.heard = now;
                link.acked(header.ack);
                let messages = match header.kind {
                    wire::RELIABLE => link.received(header.seq, body),
                    wire::DATAGRAM => vec![body.to_vec()],
                    _ => Vec::new(),
                };
                for bytes in messages {
                    if let Some(msg) = Msg::decode(&bytes) {
                        self.message(from, msg, header.kind == wire::RELIABLE, now);
                    }
                }
            }
            _ => {}
        }
    }

    fn enum_request(&mut self, from: Addr, body: &[u8]) {
        let Some((application, password)) = wire::read_enum_request(body) else {
            return;
        };
        let Some(session) = self.session.as_ref().filter(|s| s.is_host() && !s.lost) else {
            return;
        };
        let info = session.current_info();
        if application != [0; 16] && application != info.application {
            return;
        }
        if info.flags & DPSESSION_PRIVATE != 0 && password != session.password {
            return;
        }
        let reply = wire::enum_reply(&info);
        self.packet(from, wire::ENUM_REPLY, 0, &reply);
    }

    /// Ask the host of the found session `instance` for its players and
    /// groups: false if no such session was found.
    pub fn ask_roster(&mut self, instance: &[u8; 16]) -> bool {
        let Some(f) = self.found.iter_mut().find(|f| f.info.instance == *instance) else {
            return false;
        };
        f.roster = None;
        let host = f.host;
        self.packet(
            host,
            wire::ROSTER_REQUEST,
            0,
            &wire::roster_request(instance),
        );
        true
    }

    fn roster_request(&mut self, from: Addr, body: &[u8]) {
        let Some(instance) = wire::read_roster_request(body) else {
            return;
        };
        let Some(session) = self
            .session
            .as_ref()
            .filter(|s| s.is_host() && s.info.instance == instance)
        else {
            return;
        };
        let entities: Vec<Entity> = session.entities.iter().map(Entity::public).collect();
        let reply = wire::roster_reply(&instance, &entities, self.piece_room());
        self.packet(from, wire::ROSTER_REPLY, 0, &reply);
    }

    fn enum_reply(&mut self, from: Addr, body: &[u8], now: u64) {
        if self.search.is_none() {
            return;
        }
        let Some(info) = wire::read_enum_reply(body) else {
            return;
        };
        let found = Found {
            info,
            host: from,
            heard: now,
            roster: None,
        };
        match self
            .found
            .iter_mut()
            .find(|f| f.info.instance == found.info.instance)
        {
            Some(f) => {
                let roster = f.roster.take();
                *f = Found { roster, ..found };
            }
            None => self.found.push(found),
        }
    }

    fn is_member(&self, addr: Addr) -> bool {
        self.session
            .as_ref()
            .is_some_and(|s| s.members.iter().any(|m| m.addr == addr))
    }

    fn message(&mut self, from: Addr, msg: Msg, reliable: bool, now: u64) {
        match msg {
            Msg::Join { instance, password } if reliable => {
                self.join_request(from, instance, password, now)
            }
            Msg::Joined { .. } if reliable => self.joined(from, msg),
            _ if !self.is_member(from) => {}
            Msg::Data {
                from: player,
                recipients,
                data,
                ..
            } => {
                let Some(session) = self.session.as_ref() else {
                    return;
                };
                let local: Vec<u32> = session
                    .local_players()
                    .into_iter()
                    .filter(|p| recipients.contains(p))
                    .collect();
                for p in local {
                    self.deliver(player, p, Body::Data { data: data.clone() });
                }
            }
            _ if !reliable => {}
            Msg::MemberJoined { member } => self.member_joined(from, member, now),
            Msg::Leave => self.member_left(from, now),
            Msg::NewHost { member } => {
                if let Some(s) = self.session.as_mut() {
                    s.host = member;
                }
            }
            Msg::Create { entity } => self.created(from, entity),
            Msg::Destroy { id } => self.remove(id),
            Msg::AddToGroup { group, player } => {
                self.grouped(group, player, false, true);
            }
            Msg::DeleteFromGroup { group, player } => {
                self.grouped(group, player, false, false);
            }
            Msg::AddGroupToGroup { parent, group } => {
                self.grouped(parent, group, true, true);
            }
            Msg::DeleteGroupFromGroup { parent, group } => {
                self.grouped(parent, group, true, false);
            }
            Msg::SetName { id, short, long } => {
                let messages = self.data_messages();
                let Some(e) = self.session.as_mut().and_then(|s| s.entity_mut(id)) else {
                    return;
                };
                e.short = short;
                e.long = long;
                let entity = e.clone();
                if messages {
                    self.tell(Sys::SetName { entity }, 0);
                }
            }
            Msg::SetData { id, data } => {
                let messages = self.data_messages();
                let Some(e) = self.session.as_mut().and_then(|s| s.entity_mut(id)) else {
                    return;
                };
                e.data = data;
                let entity = e.clone();
                if messages {
                    self.tell(Sys::SetData { entity }, 0);
                }
            }
            Msg::SetDesc { info, password } => {
                let Some(s) = self.session.as_mut() else {
                    return;
                };
                if s.member_addr(s.host) != Some(from) {
                    return;
                }
                s.info = Info {
                    instance: s.info.instance,
                    ..info.clone()
                };
                s.password = password.clone();
                self.tell(Sys::SetDesc { info, password }, 0);
            }
            Msg::SetGroupOwner { group, owner } => {
                let Some(g) = self.session.as_mut().and_then(|s| s.entity_mut(group)) else {
                    return;
                };
                let old = std::mem::replace(&mut g.group_owner, owner);
                self.tell(
                    Sys::SetGroupOwner {
                        group,
                        new: owner,
                        old,
                    },
                    0,
                );
            }
            Msg::Chat { .. } => self.chat_arrived(&msg),
            Msg::Join { .. } | Msg::Joined { .. } => {}
        }
    }

    /// The host is asked to take in `from`.
    fn join_request(&mut self, from: Addr, instance: [u8; 16], password: Vec<u16>, now: u64) {
        let Some(session) = self.session.as_mut().filter(|s| s.is_host() && !s.lost) else {
            return;
        };
        if session.info.instance != instance {
            return;
        }
        let refused =
            if session.info.flags & (DPSESSION_JOINDISABLED | DPSESSION_NEWPLAYERSDISABLED) != 0 {
                Some(DPERR_NONEWPLAYERS)
            } else if !session.password.is_empty() && password != session.password {
                Some(DPERR_INVALIDPASSWORD)
            } else if session.info.max_players != 0 && session.players() >= session.info.max_players
            {
                Some(DPERR_NONEWPLAYERS)
            } else {
                None
            };
        if let Some(result) = refused {
            let msg = Msg::Joined {
                result,
                info: Info::default(),
                password: Vec::new(),
                member: 0,
                key: 0,
                members: Vec::new(),
                entities: Vec::new(),
            };
            self.send(from, &msg, now);
            return;
        }
        let known = session.members.iter().find(|m| m.addr == from).copied();
        let member = match known {
            Some(m) => m,
            None => {
                let m = Member {
                    addr: from,
                    index: session.next_member,
                };
                session.next_member = session.next_member.wrapping_add(1).max(2);
                session.members.push(m);
                m
            }
        };
        let msg = Msg::Joined {
            result: DP_OK,
            info: session.current_info(),
            password: session.password.clone(),
            member: member.index,
            key: session.key,
            members: session.members.clone(),
            entities: session.entities.iter().map(Entity::public).collect(),
        };
        let me = session
            .members
            .iter()
            .find(|m| m.index == session.me)
            .map(|m| m.addr);
        let others: Vec<Addr> = session
            .members
            .iter()
            .map(|m| m.addr)
            .filter(|&a| a != from && Some(a) != me)
            .collect();
        self.send(from, &msg, now);
        if known.is_none() {
            for to in others {
                self.send(to, &Msg::MemberJoined { member }, now);
            }
        }
    }

    /// The host's answer to a join came.
    fn joined(&mut self, from: Addr, msg: Msg) {
        let Msg::Joined {
            result,
            info,
            password,
            member,
            key,
            members,
            entities,
        } = msg
        else {
            return;
        };
        let Some(joining) = self
            .joining
            .as_mut()
            .filter(|j| j.host == from && j.result.is_none())
        else {
            return;
        };
        if result != DP_OK {
            joining.result = Some(result);
            return;
        }
        if info.instance != joining.instance {
            return;
        }
        joining.result = Some(DP_OK);
        let host = members
            .iter()
            .find(|m| m.addr == from)
            .map_or(0, |m| m.index);
        let next_member = members
            .iter()
            .map(|m| m.index)
            .max()
            .unwrap_or(1)
            .wrapping_add(1);
        let me = self.me();
        let members = members
            .into_iter()
            .map(|m| {
                if m.index == member {
                    Member {
                        addr: me,
                        index: member,
                    }
                } else {
                    m
                }
            })
            .collect();
        self.session = Some(Session {
            info,
            password,
            host,
            me: member,
            members,
            entities,
            key,
            next_member,
            ..Session::default()
        });
    }

    /// The host took in `member`: it hears from this one, and gets this
    /// one's players and groups in case the host's picture lacked them.
    fn member_joined(&mut self, from: Addr, member: Member, now: u64) {
        let me = self.me();
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if session.member_addr(session.host) != Some(from) || member.addr == me {
            return;
        }
        if !session.members.iter().any(|m| m.addr == member.addr) {
            session.members.push(member);
        }
        session.next_member = session.next_member.max(member.index.wrapping_add(1));
        let mine: Vec<Entity> = session
            .entities
            .iter()
            .filter(|e| e.owner == me)
            .map(Entity::public)
            .collect();
        for entity in mine {
            self.send(member.addr, &Msg::Create { entity }, now);
        }
    }

    /// A member made a player or group, or told this one about it again.
    fn created(&mut self, from: Addr, entity: Entity) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if let Some(e) = session.entity_mut(entity.id) {
            if e.owner == from {
                let (local, local_data, event, own_event) = (
                    e.local,
                    std::mem::take(&mut e.local_data),
                    e.event,
                    e.own_event,
                );
                *e = Entity {
                    local,
                    local_data,
                    event,
                    own_event,
                    ..entity
                };
            }
            return;
        }
        let current = if entity.group { 0 } else { session.players() };
        let entity = Entity {
            local: false,
            ..entity
        };
        session.entities.push(entity.clone());
        self.tell(Sys::Create { entity, current }, 0);
    }

    fn grouped(&mut self, group: u32, member: u32, subgroup: bool, add: bool) {
        let Some(g) = self
            .session
            .as_mut()
            .and_then(|s| s.entity_mut(group))
            .filter(|g| g.group)
        else {
            return;
        };
        let list = if subgroup {
            &mut g.groups
        } else {
            &mut g.players
        };
        let has = list.contains(&member);
        if add == has {
            return;
        }
        if add {
            list.push(member);
        } else {
            list.retain(|&m| m != member);
        }
        let sys = match (subgroup, add) {
            (false, true) => Sys::AddToGroup {
                group,
                player: member,
            },
            (false, false) => Sys::DeleteFromGroup {
                group,
                player: member,
            },
            (true, true) => Sys::AddGroupToGroup {
                parent: group,
                group: member,
            },
            (true, false) => Sys::DeleteGroupFromGroup {
                parent: group,
                group: member,
            },
        };
        self.tell(sys, 0);
    }

    /// The member at `addr` left, or was lost: its players go, its
    /// groups stay, and a new host comes if it was the host.
    fn member_left(&mut self, addr: Addr, _now: u64) {
        let me = self.me();
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let Some(i) = session.members.iter().position(|m| m.addr == addr) else {
            return;
        };
        let gone = session.members.remove(i);
        let players: Vec<u32> = session
            .entities
            .iter()
            .filter(|e| e.owner == addr && !e.group)
            .map(|e| e.id)
            .collect();
        for e in session.entities.iter_mut().filter(|e| e.owner == addr) {
            e.owner = me;
        }
        let was_host = gone.index == session.host;
        // Its link stays until what it sent is acknowledged (`tick`).
        for id in players {
            self.remove(id);
        }
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if was_host {
            if session.info.flags & DPSESSION_MIGRATEHOST != 0 {
                let next = session.members.iter().map(|m| m.index).min().unwrap_or(0);
                session.host = next;
                if next == session.me {
                    self.tell(Sys::Host {}, 0);
                }
            } else {
                session.host = 0;
            }
        }
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if !session.is_host() && session.members.len() <= 1 && !session.lost {
            session.lost = true;
            self.tell(Sys::SessionLost {}, 0);
        }
    }

    // --- Time passing ---

    /// Time passed to `now`: pieces are sent again, quiet links say they
    /// are there, lost members go, the background search asks again.
    pub fn tick(&mut self, now: u64) {
        let lost: Vec<Addr> = self
            .links
            .iter()
            .filter(|l| l.lost(now))
            .map(|l| l.peer)
            .collect();
        for addr in lost {
            if self
                .joining
                .as_ref()
                .is_some_and(|j| j.host == addr && j.result.is_none())
            {
                self.joining.as_mut().unwrap().result = Some(DPERR_TIMEOUT);
            }
            self.member_left(addr, now);
            self.links.retain(|l| l.peer != addr);
        }
        // Links of no use any more: their peer isn't a member, and they
        // have nothing left to send.
        let session = self.session.clone();
        let joining = self.joining.clone();
        self.links.retain(|l| {
            let member = session
                .as_ref()
                .is_some_and(|s| s.members.iter().any(|m| m.addr == l.peer));
            let host = joining.as_ref().is_some_and(|j| j.host == l.peer);
            member || host || !l.unacked.is_empty()
        });
        if let Some(j) = self.joining.as_mut()
            && j.result.is_none()
            && now >= j.until
        {
            j.result = Some(DPERR_TIMEOUT);
        }
        if let Some(search) = self.search.clone()
            && search.background
            && now >= search.next
        {
            self.found
                .retain(|f| now.saturating_sub(f.heard) < FOUND_STALE);
            self.search(search.application, search.password, true, now);
        }
    }

    /// Put what is due in the outbox: pieces, and acknowledgements alone.
    pub fn flush(&mut self, now: u64) {
        let mut packets = Vec::new();
        for link in &mut self.links {
            let pieces = link.due(now);
            if pieces.is_empty() && link.wants_ack(now) {
                link.ack_owed = false;
                link.spoke = now;
                packets.push((link.peer, wire::ACK, 0, Vec::new()));
            }
            for (seq, body) in pieces {
                packets.push((link.peer, wire::RELIABLE, seq, body));
            }
        }
        for (to, kind, seq, body) in packets {
            self.packet(to, kind, seq, &body);
        }
    }

    // --- What the players read ---

    /// The first message that `pick` takes, taken out of the queue
    /// unless `peek`.
    pub fn take(&mut self, pick: impl Fn(&Queued) -> bool, peek: bool) -> Option<Queued> {
        let i = self.queue.iter().position(pick)?;
        if peek {
            return self.queue.get(i).cloned();
        }
        self.queue.remove(i)
    }

    /// A SendEx with DPSEND_ASYNC is done: DPSYS_SENDCOMPLETE for its
    /// sender, and its message id.
    pub fn send_complete(&mut self, mut sys: Sys) -> u32 {
        self.sends = self.sends.wrapping_add(1).max(1);
        let id = self.sends;
        if let Sys::SendComplete { id: at, from, .. } = &mut sys {
            *at = id;
            let from = *from;
            self.deliver(DPID_SYSMSG, from, Body::System { sys });
        }
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP: [u8; 16] = [7; 16];

    fn play(node: u8, endpoint: u32) -> Play {
        Play {
            endpoint,
            provider: 1,
            node: Mac([2, 0, 0, 0, 0, node]),
            room: 1470,
            ..Play::default()
        }
    }

    /// Hand every packet in every outbox to the endpoints it is for,
    /// until none are left: a network that loses nothing.
    fn deliver(plays: &mut [&mut Play], now: u64) {
        for _ in 0..50 {
            let mut packets = Vec::new();
            for p in plays.iter_mut() {
                p.flush(now);
                let from = p.node;
                packets.extend(p.outbox.drain(..).map(|(to, bytes)| (from, to, bytes)));
            }
            if packets.is_empty() {
                return;
            }
            for (from, to, bytes) in packets {
                for p in plays.iter_mut() {
                    if to == Mac::BROADCAST || to == p.node {
                        p.handle(from, &bytes, now);
                    }
                }
            }
        }
        panic!("the endpoints never went quiet");
    }

    fn info(max: u32, flags: u32) -> Info {
        Info {
            flags,
            instance: [9; 16],
            application: APP,
            max_players: max,
            name: Some("Race".encode_utf16().collect()),
            ..Info::default()
        }
    }

    fn trace(p: &mut Play) -> Vec<(u32, u32, u32)> {
        let mut out = Vec::new();
        while let Some(m) = p.take(|_| true, false) {
            let kind = match m.body {
                Body::Data { .. } => 0,
                Body::System {
                    sys: Sys::Create { .. },
                } => 3,
                Body::System {
                    sys: Sys::Destroy { .. },
                } => 5,
                Body::System { sys: Sys::Host {} } => 0x101,
                Body::System {
                    sys: Sys::SessionLost {},
                } => 0x31,
                Body::System { .. } => 1,
            };
            out.push((m.from, m.to, kind));
        }
        out
    }

    fn join(host: &mut Play, guest: &mut Play, now: u64) {
        guest.search(APP, Vec::new(), false, now);
        deliver(&mut [host, guest], now);
        assert_eq!(guest.found.len(), 1);
        let found = guest.found[0].clone();
        guest.join(&found, Vec::new(), now);
        deliver(&mut [host, guest], now);
        assert_eq!(guest.joining.as_ref().unwrap().result, Some(DP_OK));
    }

    #[test]
    fn a_session_is_found_joined_and_played_in() {
        let (mut a, mut b) = (play(1, 10), play(2, 20));
        a.create(info(4, 0), Vec::new(), 0x5A5A_0000);
        let p0 = a
            .create_entity(false, 0, None, None, vec![], 0, 0, 0)
            .unwrap();
        join(&mut a, &mut b, 0);
        let session = b.session.as_ref().unwrap();
        assert_eq!((session.me, session.host), (2, 1));
        assert_eq!(
            session.players(),
            1,
            "the host's player came with the session"
        );
        let p1 = b
            .create_entity(false, 0, None, None, vec![], 0, 0, 0)
            .unwrap();
        deliver(&mut [&mut a, &mut b], 0);
        assert_ne!(p0, p1);
        assert_eq!(trace(&mut a), vec![(0, p0, 3)]);
        assert_eq!(
            trace(&mut b),
            vec![],
            "nothing about its own player, nor about players before it had one"
        );
        // Messages go both ways, to all players or to one.
        a.send_data(p0, DPID_ALLPLAYERS, vec![1], false, 0).unwrap();
        b.send_data(p1, p0, vec![2], true, 0).unwrap();
        deliver(&mut [&mut a, &mut b], 0);
        assert_eq!(trace(&mut b), vec![(p0, p1, 0)]);
        assert_eq!(trace(&mut a), vec![(p1, p0, 0)]);
        assert_eq!(
            a.send_data(p1, p0, vec![], false, 0),
            Err(DPERR_INVALIDPLAYER),
            "only from a local player"
        );
        // The guest leaves: its player goes for the host.
        b.close(0);
        deliver(&mut [&mut a, &mut b], 0);
        assert_eq!(trace(&mut a), vec![(0, p0, 5)]);
        assert_eq!(a.session.as_ref().unwrap().members.len(), 1);
    }

    #[test]
    fn system_messages_go_to_each_local_player_newest_first() {
        let mut a = play(1, 10);
        a.create(info(0, 0), Vec::new(), 1);
        let p0 = a
            .create_entity(false, 0, None, None, vec![], 0, 0, 0)
            .unwrap();
        let p1 = a
            .create_entity(false, 0, None, None, vec![], 0, 0, 0)
            .unwrap();
        let p2 = a
            .create_entity(false, 0, None, None, vec![], 0, 0, 0)
            .unwrap();
        assert_eq!(trace(&mut a), vec![(0, p0, 3), (0, p1, 3), (0, p0, 3)]);
        let g = a
            .create_entity(true, 0, None, None, vec![], 0, 0, 0)
            .unwrap();
        assert_eq!(trace(&mut a), vec![(0, p2, 3), (0, p1, 3), (0, p0, 3)]);
        a.add_to_group(g, p0, false, 0).unwrap();
        a.add_to_group(g, p1, false, 0).unwrap();
        trace(&mut a);
        a.send_data(p0, g, vec![], false, 0).unwrap();
        assert_eq!(trace(&mut a), vec![(p0, p1, 0)]);
        a.send_data(p2, DPID_ALLPLAYERS, vec![], false, 0).unwrap();
        assert_eq!(trace(&mut a), vec![(p2, p1, 0), (p2, p0, 0)]);
        // A player that goes leaves its group first.
        a.destroy(p1, false, 0).unwrap();
        assert_eq!(
            trace(&mut a),
            vec![(0, p2, 1), (0, p0, 1), (0, p2, 5), (0, p0, 5)]
        );
        assert_eq!(
            a.session.as_ref().unwrap().group(g).unwrap().players,
            vec![p0]
        );
    }

    #[test]
    fn the_host_migrates_or_the_session_is_lost() {
        for migrate in [true, false] {
            let flags = if migrate { DPSESSION_MIGRATEHOST } else { 0 };
            let (mut a, mut b, mut c) = (play(1, 10), play(2, 20), play(3, 30));
            a.create(info(0, flags), Vec::new(), 1);
            join(&mut a, &mut b, 0);
            join(&mut a, &mut c, 0);
            // b hears of c when the host sends it again.
            let retry = super::super::link::RETRY + 1;
            deliver(&mut [&mut a, &mut b, &mut c], retry);
            let pb = b
                .create_entity(false, 0, None, None, vec![], 0, 0, 0)
                .unwrap();
            let pc = c
                .create_entity(false, 0, None, None, vec![], 0, 0, 0)
                .unwrap();
            deliver(&mut [&mut a, &mut b, &mut c], retry);
            trace(&mut b);
            trace(&mut c);
            assert_eq!(b.session.as_ref().unwrap().members.len(), 3, "b heard of c");
            // The host is lost without a word, while b and c still hear
            // each other.
            let later = retry + super::super::link::LOST;
            deliver(&mut [&mut b, &mut c], later - 1);
            b.tick(later);
            c.tick(later);
            let host = |p: &Play| p.session.as_ref().unwrap().host;
            if migrate {
                assert_eq!((host(&b), host(&c)), (2, 2));
                assert_eq!(trace(&mut b), vec![(0, pb, 0x101)]);
                assert_eq!(trace(&mut c), vec![]);
            } else {
                assert_eq!((host(&b), host(&c)), (0, 0));
            }
            // Then c goes too.
            c.close(later);
            deliver(&mut [&mut b, &mut c], later);
            let t = trace(&mut b);
            assert_eq!(t[0], (0, pb, 5), "{:?}", pc);
            assert_eq!(t.len() == 2, !migrate, "lost only without a host: {:?}", t);
        }
    }

    #[test]
    fn a_full_or_closed_session_refuses_and_a_password_is_checked() {
        let (mut a, mut b) = (play(1, 10), play(2, 20));
        a.create(info(1, 0), vec![b'p' as u16], 1);
        a.create_entity(false, 0, None, None, vec![], 0, 0, 0)
            .unwrap();
        b.search(APP, Vec::new(), false, 0);
        deliver(&mut [&mut a, &mut b], 0);
        let found = b.found[0].clone();
        assert!(found.info.has_password);
        assert_eq!(found.info.current_players, 1);
        b.join(&found, vec![b'x' as u16], 0);
        deliver(&mut [&mut a, &mut b], 0);
        assert_eq!(
            b.joining.as_ref().unwrap().result,
            Some(DPERR_INVALIDPASSWORD)
        );
        b.join(&found, vec![b'p' as u16], 0);
        deliver(&mut [&mut a, &mut b], 0);
        assert_eq!(b.joining.as_ref().unwrap().result, Some(DPERR_NONEWPLAYERS));
        assert!(b.session.is_none());
        // The host tells who is in its session to one that isn't in it.
        assert!(b.ask_roster(&found.info.instance));
        deliver(&mut [&mut a, &mut b], 0);
        assert_eq!(b.found[0].roster.as_ref().map(Vec::len), Some(1));
        // Another application's search isn't answered.
        b.search([1; 16], Vec::new(), false, 0);
        deliver(&mut [&mut a, &mut b], 0);
        assert!(b.found.is_empty());
    }

    #[test]
    fn a_newcomer_learns_the_players_of_every_member() {
        let (mut a, mut b, mut c) = (play(1, 10), play(2, 20), play(3, 30));
        a.create(info(0, 0), Vec::new(), 1);
        join(&mut a, &mut b, 0);
        // b's player is made while the host takes c in: the host's
        // picture lacks it until b tells c itself.
        c.search(APP, Vec::new(), false, 0);
        deliver(&mut [&mut a, &mut c], 0);
        let found = c.found[0].clone();
        let pb = b
            .create_entity(false, 0, None, None, vec![], 0, 0, 0)
            .unwrap();
        c.join(&found, Vec::new(), 0);
        deliver(&mut [&mut a, &mut c], 0);
        assert!(c.session.as_ref().unwrap().player(pb).is_none());
        // b hears of c when the host sends it again.
        deliver(&mut [&mut a, &mut b, &mut c], super::super::link::RETRY + 1);
        assert!(c.session.as_ref().unwrap().player(pb).is_some());
        assert!(a.session.as_ref().unwrap().player(pb).is_some());
    }
}
