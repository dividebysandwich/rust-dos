//! IDirectPlay2, 3 and 4's methods, A and W: the DirectPlay that games
//! of DirectX 5 and later use, on the session of `session.rs`.
//!
//! The errors follow DirectPlay's order, as Wine's tests have it: a
//! method that needs a connection says DPERR_UNINITIALIZED before
//! InitializeConnection, and one that needs a session says
//! DPERR_INVALIDPARAMS before Open.

use super::calls::{self, List};
use super::guest::{self, Blob};
use super::net;
use super::session::*;
use super::{
    DPERR_ALREADYINITIALIZED, DPERR_BUFFERTOOSMALL, DPERR_INVALIDFLAGS, DPERR_UNINITIALIZED,
    MAX_DATA, object, object_of, play, provider_of,
};
use crate::cpu::Cpu;
use crate::win32::com;
use crate::win32::object::{self as kernel, Kind};
use crate::win32::task::{Next, Task};

const DPERR_NOMESSAGES: u32 = 0x8877_00BE;
const DPERR_CONNECTING: u32 = 0x8877_015E;
const DPERR_UNKNOWNMESSAGE: u32 = 0x8877_0172;
pub(super) const DPERR_UNSUPPORTED: u32 = com::E_NOTIMPL;
const DPERR_PENDING: u32 = 0x8000_000A;

const DPOPEN_JOIN: u32 = 1;
const DPOPEN_CREATE: u32 = 2;
const DPOPEN_RETURNSTATUS: u32 = 0x80;
const DPENUMSESSIONS_AVAILABLE: u32 = 1;
const DPENUMSESSIONS_ALL: u32 = 2;
const DPENUMSESSIONS_ASYNC: u32 = 0x10;
const DPENUMSESSIONS_STOPASYNC: u32 = 0x20;
const DPENUMSESSIONS_PASSWORDREQUIRED: u32 = 0x40;
const DPENUMSESSIONS_RETURNSTATUS: u32 = 0x80;
const DPRECEIVE_ALL: u32 = 1;
const DPRECEIVE_TOPLAYER: u32 = 2;
const DPRECEIVE_FROMPLAYER: u32 = 4;
const DPRECEIVE_PEEK: u32 = 8;
const DPSEND_GUARANTEED: u32 = 1;
const DPSEND_ASYNC: u32 = 0x200;
const DPSEND_NOSENDCOMPLETEMSG: u32 = 0x400;
const DPSET_LOCAL: u32 = 1;
const DPGET_LOCAL: u32 = 1;
const DPENUMPLAYERS_LOCAL: u32 = 0x8;
const DPENUMPLAYERS_REMOTE: u32 = 0x10;
const DPENUMPLAYERS_GROUP: u32 = 0x20;
const DPENUMPLAYERS_SESSION: u32 = 0x80;
const DPENUMPLAYERS_SERVERPLAYER: u32 = 0x100;
const DPENUMPLAYERS_SPECTATOR: u32 = 0x200;
const DPENUMGROUPS_SHORTCUT: u32 = 0x400;
const DPENUMGROUPS_STAGINGAREA: u32 = 0x800;
const DPENUMGROUPS_HIDDEN: u32 = 0x1000;
const DPMESSAGEQUEUE_SEND: u32 = 1;
const DPCAPS_ISHOST: u32 = 0x2;
const DPCAPS_GUARANTEEDOPTIMIZED: u32 = 0x20;
const DPCAPS_GUARANTEEDSUPPORTED: u32 = 0x40;
const DPPLAYERCAPS_LOCAL: u32 = 0x800;
const DPCAPS_SIZE: u32 = 40;
/// The session flags SetSessionDesc may change.
const MUTABLE_FLAGS: u32 = DPSESSION_NEWPLAYERSDISABLED
    | DPSESSION_JOINDISABLED
    | DPSESSION_PRIVATE
    | DPSESSION_PASSWORDREQUIRED;

/// What a method that has an object returns.
type Answer = Result<u32, u32>;

pub(super) fn answer(r: Answer) -> Option<u32> {
    Some(r.unwrap_or_else(|e| e))
}

fn now(cpu: &Cpu) -> u64 {
    cpu.bus.clock.now_micros()
}

/// The object's network side once it has a connection.
fn connected(cpu: &mut Cpu, id: usize) -> Result<&mut Play, u32> {
    let p = play(cpu, id);
    if p.provider == 0 {
        return Err(DPERR_UNINITIALIZED);
    }
    Ok(p)
}

/// The object's session, once it has one.
fn in_session(cpu: &mut Cpu, id: usize) -> Result<&mut Play, u32> {
    let p = connected(cpu, id)?;
    if p.session.is_none() {
        return Err(DPERR_INVALIDPARAMS);
    }
    Ok(p)
}

/// Do `f` to the object's session and send what it has to send: for the
/// methods that change it.
fn change(cpu: &mut Cpu, id: usize, f: impl FnOnce(&mut Play, u64) -> Result<(), u32>) -> Answer {
    let t = now(cpu);
    net::prepare(cpu, id);
    let r = f(in_session(cpu, id)?, t);
    net::after(cpu, id);
    r.map(|_| DP_OK)
}

/// Hand `blob` out at `at`, whose room is at `size_at`, as DirectPlay
/// does: the size it needs there always, and DPERR_BUFFERTOOSMALL when
/// it doesn't fit or there is no buffer.
pub fn give(cpu: &mut Cpu, blob: &Blob, at: u32, size_at: u32) -> Answer {
    if size_at == 0 || !cpu.bus.guest_probe(size_at, 4, true) {
        return Err(DPERR_INVALIDPARAMS);
    }
    let room = cpu.bus.guest_read_32(size_at);
    cpu.bus.guest_write_32(size_at, blob.len());
    if at == 0 || room < blob.len() {
        return Err(DPERR_BUFFERTOOSMALL);
    }
    if !blob.write(cpu, at) {
        return Err(DPERR_INVALIDPARAMS);
    }
    Ok(DP_OK)
}

fn bytes_blob(data: &[u8]) -> Blob {
    let mut b = Blob::default();
    b.raw(data);
    b
}

/// Whether `callback` is code the program may call.
fn callable(cpu: &mut Cpu, callback: u32) -> bool {
    callback != 0 && crate::win32::kernel32::virtual_memory::allows(cpu, callback, 1, false)
}

// --- Connections ---

/// InitializeConnection(this, connection, flags): the connection's
/// service provider, if it is one there is.
pub fn initialize_connection(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, connection, flags] = args[..] else {
        return Some(DPERR_INVALIDPARAMS);
    };
    let (id, _) = object!(cpu, this);
    if flags != 0 {
        return Some(DPERR_INVALIDFLAGS);
    }
    if play(cpu, id).provider != 0 {
        return Some(DPERR_ALREADYINITIALIZED);
    }
    if connection == 0
        || !cpu
            .bus
            .guest_probe(connection, super::CHUNK_HEADER as usize, false)
    {
        return Some(DPERR_INVALIDPARAMS);
    }
    // An address that names no provider names none there is.
    let provider = super::address::provider_of_address(cpu, connection).unwrap_or(com::GUID_NULL);
    Some(super::initialize(cpu, id, &provider))
}

/// EnumConnections(this, application, callback, context, flags): the
/// service providers, DirectPlay's connections; the lobby's are none.
pub fn enum_connections(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, _, callback, context, flags] = args[..] else {
        return Some(DPERR_INVALIDPARAMS);
    };
    let (id, vtable) = object!(cpu, this);
    if !callable(cpu, callback) {
        return Some(DPERR_INVALIDPARAMS);
    }
    let all = super::DPCONNECTION_DIRECTPLAY | super::DPCONNECTION_DIRECTPLAYLOBBY;
    if flags & !all != 0 {
        return Some(DPERR_INVALIDFLAGS);
    }
    if flags != 0 && flags & super::DPCONNECTION_DIRECTPLAY == 0 {
        return Some(DP_OK);
    }
    let list = List {
        kind: calls::CONNECTION_LIST,
        callback,
        context,
        wide: super::wide(vtable),
        object: id,
        version: 2,
    };
    calls::providers(cpu, list)
}

/// GetCaps(this, caps, flags), and GetPlayerCaps(this, player, caps,
/// flags) with `player`: the provider's, which is the LAN's.
fn caps(cpu: &mut Cpu, id: usize, player: Option<u32>, at: u32) -> Answer {
    let p = connected(cpu, id)?;
    let room = p.room.saturating_sub(super::wire::HEADER as u32);
    let mut flags = DPCAPS_GUARANTEEDSUPPORTED | DPCAPS_GUARANTEEDOPTIMIZED;
    if let Some(player) = player {
        let s = p.session.as_ref().ok_or(DPERR_INVALIDPARAMS)?;
        let e = s.player(player).ok_or(DPERR_INVALIDPLAYER)?;
        if e.local {
            flags |= DPPLAYERCAPS_LOCAL;
        }
        let host = s.members.iter().find(|m| m.index == s.host).map(|m| m.addr);
        if host == Some(e.owner) {
            flags |= DPCAPS_ISHOST;
        }
    } else if p.session.as_ref().is_some_and(Session::is_host) {
        flags |= DPCAPS_ISHOST;
    }
    if at == 0
        || !cpu.bus.guest_probe(at, DPCAPS_SIZE as usize, true)
        || cpu.bus.guest_read_32(at) != DPCAPS_SIZE
    {
        return Err(DPERR_INVALIDPARAMS);
    }
    // dwSize, dwFlags, dwMaxBufferSize, dwMaxQueueSize, dwMaxPlayers,
    // dwHundredBaud (10 Mbit/s), dwLatency, dwMaxLocalPlayers,
    // dwHeaderLength, dwTimeout.
    let values = [
        DPCAPS_SIZE,
        flags,
        room,
        0,
        65_536,
        100_000,
        0,
        65_536,
        super::wire::HEADER as u32,
        (ENUM_TIMEOUT / 1000) as u32,
    ];
    for (i, v) in values.into_iter().enumerate() {
        cpu.bus.guest_write_32(at + 4 * i as u32, v);
    }
    Ok(DP_OK)
}

pub fn get_caps(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, _) = object!(cpu, args[0]);
    answer(caps(cpu, id, None, args[1]))
}

pub fn get_player_caps(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, _) = object!(cpu, args[0]);
    answer(caps(cpu, id, Some(args[1]), args[2]))
}

// --- Sessions ---

/// EnumSessions(this, desc, timeout, callback, context, flags).
pub fn enum_sessions(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, desc, timeout, callback, context, flags] = args[..] else {
        return Some(DPERR_INVALIDPARAMS);
    };
    let (id, vtable) = object!(cpu, this);
    let wide = super::wide(vtable);
    let Some((info, password)) = guest::read_desc2(cpu, desc, wide) else {
        return Some(DPERR_INVALIDPARAMS);
    };
    let search = Search {
        application: info.application,
        password,
        background: false,
        next: 0,
    };
    let list = List {
        kind: calls::SESSION_LIST,
        callback,
        context,
        wide,
        object: id,
        version: 2,
    };
    enumerate(cpu, id, list, search, timeout, flags)
}

/// EnumSessions of either interface: a sync one asks, waits for the
/// answers and lists them; an async one asks in the background and lists
/// what came so far.
pub fn enumerate(
    cpu: &mut Cpu,
    id: usize,
    list: List,
    search: Search,
    timeout: u32,
    flags: u32,
) -> Option<u32> {
    if play(cpu, id).provider == 0 {
        return Some(DPERR_UNINITIALIZED);
    }
    if flags & DPENUMSESSIONS_STOPASYNC != 0 {
        play(cpu, id).stop_search();
        return Some(DP_OK);
    }
    if !callable(cpu, list.callback) {
        return Some(DPERR_INVALIDPARAMS);
    }
    net::poll(cpu);
    net::prepare(cpu, id);
    let t = now(cpu);
    let background = flags & DPENUMSESSIONS_ASYNC != 0;
    let esp = cpu.esp();
    let timeout_ms = if timeout == 0 {
        (ENUM_TIMEOUT / 1000) as u32
    } else {
        timeout
    };
    let p = play(cpu, id);
    if background {
        let running = p
            .search
            .as_ref()
            .is_some_and(|s| s.background && s.application == search.application);
        if !running {
            p.search(search.application, search.password, true, t);
            net::after(cpu, id);
        }
    } else {
        match &p.waiting {
            Some(w) if w.esp == esp && t < w.until => {
                let until = w.until;
                return calls::wait(cpu, until);
            }
            Some(w) if w.esp == esp => p.waiting = None,
            _ => {
                let until = t + timeout_ms as u64 * 1000;
                p.search(search.application, search.password, false, t);
                p.waiting = Some(Waiting { esp, until });
                net::after(cpu, id);
                return calls::wait(cpu, until);
            }
        }
    }
    let items = listed(play(cpu, id), flags);
    calls::sessions(cpu, list, timeout_ms, background, &items)
}

/// The found sessions EnumSessions lists with `flags`.
fn listed(p: &Play, flags: u32) -> Vec<usize> {
    let all = flags & DPENUMSESSIONS_ALL != 0;
    let passwords = flags & DPENUMSESSIONS_PASSWORDREQUIRED != 0;
    let _ = (DPENUMSESSIONS_AVAILABLE, DPENUMSESSIONS_RETURNSTATUS);
    p.found
        .iter()
        .enumerate()
        .filter(|(_, f)| {
            let i = &f.info;
            let closed = i.flags & (DPSESSION_NEWPLAYERSDISABLED | DPSESSION_JOINDISABLED) != 0;
            let full = i.max_players != 0 && i.current_players >= i.max_players;
            (all || !(closed || full)) && (passwords || !i.has_password)
        })
        .map(|(i, _)| i)
        .collect()
}

/// The last callback of a sync EnumSessions asked it to go on: ask again
/// and wait `ms` more.
pub fn enumerate_again(cpu: &mut Cpu, t: &Task, id: usize, ms: u32) -> Next {
    let ms = if ms == 0 {
        (ENUM_TIMEOUT / 1000) as u32
    } else {
        ms
    };
    let start = now(cpu);
    let until = start + ms as u64 * 1000;
    net::prepare(cpu, id);
    let p = play(cpu, id);
    let Some(search) = p.search.clone() else {
        return Next::Done(DP_OK);
    };
    p.search(search.application, search.password, false, start);
    p.waiting = Some(Waiting {
        esp: t.resume.1,
        until,
    });
    net::after(cpu, id);
    calls::sleep_and_retry(cpu, t, until)
}

/// Open(this, desc, flags), and SecureOpen(this, desc, flags, security,
/// credentials) without security.
pub fn open(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, vtable) = object!(cpu, args[0]);
    let (desc, flags) = (args[1], args[2]);
    if args.len() > 3 && (args[3] != 0 || args[4] != 0) {
        return Some(DPERR_UNSUPPORTED);
    }
    let wide = super::wide(vtable);
    let Some((info, password)) = guest::read_desc2(cpu, desc, wide) else {
        return Some(DPERR_INVALIDPARAMS);
    };
    open_session(cpu, id, info, password, flags)
}

/// Open of either interface: make the session, or join the one found
/// with that instance GUID and wait for the host's answer.
pub fn open_session(
    cpu: &mut Cpu,
    id: usize,
    info: Info,
    password: Vec<u16>,
    flags: u32,
) -> Option<u32> {
    if flags & !(DPOPEN_JOIN | DPOPEN_CREATE | DPOPEN_RETURNSTATUS) != 0
        || (flags & DPOPEN_JOIN != 0) == (flags & DPOPEN_CREATE != 0)
    {
        return Some(DPERR_INVALIDFLAGS);
    }
    let Ok(p) = connected(cpu, id) else {
        return Some(DPERR_UNINITIALIZED);
    };
    // A join this call started (it runs again while it waits) is in the
    // session once the host answered.
    let joining = flags & DPOPEN_JOIN != 0
        && p.joining
            .as_ref()
            .is_some_and(|j| j.instance == info.instance);
    if p.session.is_some() && !joining {
        return Some(DPERR_ALREADYINITIALIZED);
    }
    net::poll(cpu);
    net::prepare(cpu, id);
    let t = now(cpu);
    if flags & DPOPEN_CREATE != 0 {
        let mut instance = [0u8; 16];
        crate::net::fill_machine_random(&mut instance);
        let key = super::random_u32();
        let info = Info {
            instance,
            current_players: 0,
            ..info
        };
        play(cpu, id).create(info, password, key);
        cpu.bus.log_string("[DPLAY] A session was made");
        return Some(DP_OK);
    }
    let p = play(cpu, id);
    let ongoing = p
        .joining
        .as_ref()
        .is_some_and(|j| j.instance == info.instance);
    if !ongoing {
        let Some(found) = p
            .found
            .iter()
            .find(|f| f.info.instance == info.instance)
            .cloned()
        else {
            return Some(DPERR_NOSESSIONS);
        };
        p.join(&found, password, t);
        net::after(cpu, id);
    }
    let p = play(cpu, id);
    let joining = p.joining.clone().unwrap_or_default();
    match joining.result {
        Some(result) => {
            p.joining = None;
            if result == DP_OK {
                cpu.bus.log_string("[DPLAY] Joined a session");
            }
            Some(result)
        }
        None if flags & DPOPEN_RETURNSTATUS != 0 => Some(DPERR_CONNECTING),
        None => calls::wait(cpu, t + 5_000),
    }
}

/// Close(this): leave the session.
pub fn close(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, _) = object!(cpu, args[0]);
    net::prepare(cpu, id);
    let t = now(cpu);
    play(cpu, id).close(t);
    net::after(cpu, id);
    Some(DP_OK)
}

/// GetSessionDesc(this, data, size).
pub fn get_session_desc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, vtable) = object!(cpu, args[0]);
    let r = (|| {
        let p = connected(cpu, id)?;
        let s = p.session.as_ref().ok_or(DPERR_NOSESSIONS)?;
        let mut blob = Blob::default();
        guest::desc2(
            &mut blob,
            &s.current_info(),
            &s.password,
            super::wide(vtable),
        );
        give(cpu, &blob, args[1], args[2])
    })();
    answer(r)
}

/// SetSessionDesc(this, desc, flags): the host's.
pub fn set_session_desc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, vtable) = object!(cpu, args[0]);
    let r = (|| {
        let p = connected(cpu, id)?;
        let old = p.session.as_ref().ok_or(DPERR_NOSESSIONS)?.info.clone();
        if !p.session.as_ref().unwrap().is_host() {
            return Err(DPERR_ACCESSDENIED);
        }
        let (info, password) =
            guest::read_desc2(cpu, args[1], super::wide(vtable)).ok_or(DPERR_INVALIDPARAMS)?;
        if (info.flags ^ old.flags) & !MUTABLE_FLAGS != 0 {
            return Err(DPERR_INVALIDPARAMS);
        }
        change(cpu, id, |p, t| p.set_desc(info, password, t))
    })();
    answer(r)
}

// --- Players and groups ---

/// The event at `handle` for a player's messages, held, or 0.
fn event_of(cpu: &mut Cpu, handle: u32) -> Result<u32, u32> {
    if handle == 0 {
        return Ok(0);
    }
    let id = kernel::resolve(cpu, handle).ok_or(DPERR_INVALIDPARAMS)?;
    let ok = matches!(
        kernel::objects(cpu).get(id).map(|o| &o.kind),
        Some(Kind::Event { .. })
    );
    if !ok {
        return Err(DPERR_INVALIDPARAMS);
    }
    kernel::objects(cpu).hold(id);
    Ok(id)
}

/// Make a player (with `event`) or a group of the object `id`, and write
/// its id at `out`.
#[allow(clippy::too_many_arguments)]
pub fn make(
    cpu: &mut Cpu,
    id: usize,
    group: bool,
    out: u32,
    name: guest::Names,
    data: Vec<u8>,
    flags: u32,
    parent: u32,
    event: u32,
    own_event: u32,
) -> Answer {
    if out == 0 || !cpu.bus.guest_probe(out, 4, true) {
        return Err(DPERR_INVALIDPARAMS);
    }
    let t = now(cpu);
    net::prepare(cpu, id);
    let p = in_session(cpu, id)?;
    let made = p.create_entity(group, flags, name.0, name.1, data, parent, event, t);
    if let (Ok(made), true) = (made, own_event != 0)
        && let Some(e) = p.session.as_mut().and_then(|s| s.entity_mut(made))
    {
        e.own_event = own_event;
    }
    net::after(cpu, id);
    let made = made?;
    cpu.bus.guest_write_32(out, made);
    Ok(DP_OK)
}

/// CreatePlayer(this, id, name, event, data, size, flags).
pub fn create_player(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, vtable) = object!(cpu, args[0]);
    let r = (|| {
        connected(cpu, id)?;
        let flags = args[6];
        if flags & !(DPPLAYER_SERVERPLAYER | DPPLAYER_SPECTATOR) != 0 {
            return Err(DPERR_INVALIDFLAGS);
        }
        let name =
            guest::read_name(cpu, args[2], super::wide(vtable)).ok_or(DPERR_INVALIDPARAMS)?;
        let data = guest::read_data(cpu, args[4], args[5]).ok_or(DPERR_INVALIDPARAMS)?;
        in_session(cpu, id)?;
        let event = event_of(cpu, args[3])?;
        let r = make(cpu, id, false, args[1], name, data, flags, 0, event, 0);
        if r.is_err() && event != 0 {
            kernel::objects(cpu).release(event);
        }
        r
    })();
    answer(r)
}

/// CreateGroup(this, id, name, data, size, flags), and
/// CreateGroupInGroup(this, parent, id, name, data, size, flags).
fn create_group_in(cpu: &mut Cpu, args: &[u32], parent: u32) -> Option<u32> {
    let (id, vtable) = object!(cpu, args[0]);
    let r = (|| {
        connected(cpu, id)?;
        let flags = args[5];
        if flags & !(DPGROUP_STAGINGAREA | DPGROUP_HIDDEN) != 0 {
            return Err(DPERR_INVALIDFLAGS);
        }
        let name =
            guest::read_name(cpu, args[2], super::wide(vtable)).ok_or(DPERR_INVALIDPARAMS)?;
        let data = guest::read_data(cpu, args[3], args[4]).ok_or(DPERR_INVALIDPARAMS)?;
        make(cpu, id, true, args[1], name, data, flags, parent, 0, 0)
    })();
    answer(r)
}

pub fn create_group(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    create_group_in(cpu, args, 0)
}

pub fn create_group_in_group(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mut rest = vec![args[0]];
    rest.extend_from_slice(&args[2..]);
    create_group_in(cpu, &rest, args[1])
}

pub fn destroy_player(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, _) = object!(cpu, args[0]);
    answer(change(cpu, id, |p, t| p.destroy(args[1], false, t)))
}

pub fn destroy_group(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, _) = object!(cpu, args[0]);
    answer(change(cpu, id, |p, t| p.destroy(args[1], true, t)))
}

/// AddPlayerToGroup(this, group, player).
pub fn add_player_to_group(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, _) = object!(cpu, args[0]);
    answer(change(cpu, id, |p, t| {
        p.add_to_group(args[1], args[2], false, t)
    }))
}

pub fn delete_player_from_group(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, _) = object!(cpu, args[0]);
    answer(change(cpu, id, |p, t| {
        p.delete_from_group(args[1], args[2], false, t)
    }))
}

/// AddGroupToGroup(this, parent, group).
pub fn add_group_to_group(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, _) = object!(cpu, args[0]);
    answer(change(cpu, id, |p, t| {
        p.add_to_group(args[1], args[2], true, t)
    }))
}

pub fn delete_group_from_group(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, _) = object!(cpu, args[0]);
    answer(change(cpu, id, |p, t| {
        p.delete_from_group(args[1], args[2], true, t)
    }))
}

/// The player (or group) `who` of the object's session.
fn entity(cpu: &mut Cpu, id: usize, who: u32, group: bool) -> Result<Entity, u32> {
    let p = in_session(cpu, id)?;
    let s = p.session.as_ref().unwrap();
    let e = s.entity(who).filter(|e| e.group == group);
    e.cloned().ok_or(if group {
        DPERR_INVALIDGROUP
    } else {
        DPERR_INVALIDPLAYER
    })
}

/// GetPlayerName and GetGroupName(this, id, data, size).
fn get_name(cpu: &mut Cpu, args: &[u32], group: bool) -> Option<u32> {
    let (id, vtable) = object!(cpu, args[0]);
    let r = (|| {
        let e = entity(cpu, id, args[1], group)?;
        let blob = guest::name_blob(&e.short, &e.long, super::wide(vtable));
        give(cpu, &blob, args[2], args[3])
    })();
    answer(r)
}

pub fn get_player_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_name(cpu, args, false)
}

pub fn get_group_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_name(cpu, args, true)
}

/// SetPlayerName and SetGroupName(this, id, name, flags).
fn set_name(cpu: &mut Cpu, args: &[u32], group: bool) -> Option<u32> {
    let (id, vtable) = object!(cpu, args[0]);
    let r = (|| {
        connected(cpu, id)?;
        let (short, long) =
            guest::read_name(cpu, args[2], super::wide(vtable)).ok_or(DPERR_INVALIDPARAMS)?;
        let local = args[3] & DPSET_LOCAL != 0;
        change(cpu, id, |p, t| {
            p.set_name(args[1], group, short, long, local, t)
        })
    })();
    answer(r)
}

pub fn set_player_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    set_name(cpu, args, false)
}

pub fn set_group_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    set_name(cpu, args, true)
}

/// GetPlayerData and GetGroupData(this, id, data, size, flags).
fn get_data(cpu: &mut Cpu, args: &[u32], group: bool) -> Option<u32> {
    let (id, _) = object!(cpu, args[0]);
    let r = (|| {
        let e = entity(cpu, id, args[1], group)?;
        let data = if args[4] & DPGET_LOCAL != 0 {
            &e.local_data
        } else {
            &e.data
        };
        give(cpu, &bytes_blob(data), args[2], args[3])
    })();
    answer(r)
}

pub fn get_player_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_data(cpu, args, false)
}

pub fn get_group_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_data(cpu, args, true)
}

/// SetPlayerData and SetGroupData(this, id, data, size, flags).
fn set_data(cpu: &mut Cpu, args: &[u32], group: bool) -> Option<u32> {
    let (id, _) = object!(cpu, args[0]);
    let r = (|| {
        connected(cpu, id)?;
        let data = guest::read_data(cpu, args[2], args[3]).ok_or(DPERR_INVALIDPARAMS)?;
        let local = args[4] & DPSET_LOCAL != 0;
        change(cpu, id, |p, t| p.set_data(args[1], group, data, local, t))
    })();
    answer(r)
}

pub fn set_player_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    set_data(cpu, args, false)
}

pub fn set_group_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    set_data(cpu, args, true)
}

/// GetPlayerAddress(this, id, data, size): its service provider's
/// address.
pub fn get_player_address(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, _) = object!(cpu, args[0]);
    let r = (|| {
        entity(cpu, id, args[1], false)?;
        let provider = provider_of(play(cpu, id).provider);
        let address = super::address::of_provider(&provider);
        give(cpu, &bytes_blob(&address), args[2], args[3])
    })();
    answer(r)
}

/// GetPlayerFlags and GetGroupFlags(this, id, flags).
fn get_flags(cpu: &mut Cpu, args: &[u32], group: bool) -> Option<u32> {
    let (id, _) = object!(cpu, args[0]);
    let r = (|| {
        let e = entity(cpu, id, args[1], group)?;
        if !com::write_out(cpu, args[2], e.reported_flags()) {
            return Err(DPERR_INVALIDPARAMS);
        }
        Ok(DP_OK)
    })();
    answer(r)
}

pub fn get_player_flags(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_flags(cpu, args, false)
}

pub fn get_group_flags(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_flags(cpu, args, true)
}

/// GetGroupParent(this, group, parent).
pub fn get_group_parent(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, _) = object!(cpu, args[0]);
    let r = (|| {
        let e = entity(cpu, id, args[1], true)?;
        if !com::write_out(cpu, args[2], e.parent) {
            return Err(DPERR_INVALIDPARAMS);
        }
        Ok(DP_OK)
    })();
    answer(r)
}

/// GetGroupOwner(this, group, owner).
pub fn get_group_owner(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, _) = object!(cpu, args[0]);
    let r = (|| {
        let e = entity(cpu, id, args[1], true)?;
        if !com::write_out(cpu, args[2], e.group_owner) {
            return Err(DPERR_INVALIDPARAMS);
        }
        Ok(DP_OK)
    })();
    answer(r)
}

/// SetGroupOwner(this, group, owner).
pub fn set_group_owner(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, _) = object!(cpu, args[0]);
    answer(change(cpu, id, |p, t| {
        p.set_group_owner(args[1], args[2], t)
    }))
}

// --- Enumerating players and groups ---

/// How long an enumeration of a session the object isn't in waits for
/// its host's answer.
const ROSTER_TIMEOUT: u64 = 1_500_000;

/// The players and groups a list picks from, the oldest first: the open
/// session's, or with DPENUMPLAYERS_SESSION those of the found session
/// whose instance GUID is at `instance`, which its host is asked for.
/// Err is what the method returns: an error, or None while it waits for
/// the host's answer and runs again.
fn roster(
    cpu: &mut Cpu,
    id: usize,
    instance: u32,
    callback: u32,
    flags: u32,
) -> Result<Vec<Entity>, Option<u32>> {
    connected(cpu, id).map_err(Some)?;
    if !callable(cpu, callback) {
        return Err(Some(DPERR_INVALIDPARAMS));
    }
    net::poll(cpu);
    if flags & DPENUMPLAYERS_SESSION == 0 {
        let p = in_session(cpu, id).map_err(Some)?;
        return Ok(p.session.as_ref().unwrap().entities.clone());
    }
    let instance = com::Guid::read(cpu, instance)
        .ok_or(Some(DPERR_INVALIDPARAMS))?
        .0;
    let esp = cpu.esp();
    let t = now(cpu);
    net::prepare(cpu, id);
    let p = play(cpu, id);
    let roster = p
        .found
        .iter()
        .find(|f| f.info.instance == instance)
        .and_then(|f| f.roster.clone());
    match p.waiting.clone() {
        Some(w) if w.esp == esp => match roster {
            Some(entities) => {
                p.waiting = None;
                Ok(entities)
            }
            None if t >= w.until => {
                p.waiting = None;
                Ok(Vec::new())
            }
            None => Err(calls::wait(cpu, (t + 5_000).min(w.until))),
        },
        _ => {
            if !p.ask_roster(&instance) {
                return Err(Some(DPERR_NOSESSIONS));
            }
            p.waiting = Some(Waiting {
                esp,
                until: t + ROSTER_TIMEOUT,
            });
            net::after(cpu, id);
            Err(calls::wait(cpu, t + 5_000))
        }
    }
}

/// The players and groups of a list, with the flags its callback gets,
/// the newest first.
fn pick(entities: &[Entity], pick: impl Fn(&Entity) -> Option<u32>) -> Vec<(u32, u32)> {
    entities
        .iter()
        .rev()
        .filter_map(|e| Some((e.id, pick(e)?)))
        .collect()
}

/// Whether a player passes EnumPlayers' `flags`.
fn player_passes(e: &Entity, flags: u32) -> bool {
    !(flags & DPENUMPLAYERS_LOCAL != 0 && !e.local
        || flags & DPENUMPLAYERS_REMOTE != 0 && e.local
        || flags & DPENUMPLAYERS_SERVERPLAYER != 0 && e.flags & DPPLAYER_SERVERPLAYER == 0
        || flags & DPENUMPLAYERS_SPECTATOR != 0 && e.flags & DPPLAYER_SPECTATOR == 0)
}

/// The flags a player's callback gets: the list's kind as Windows echoes
/// it, and the player's own.
fn player_flags(e: &Entity, flags: u32) -> u32 {
    flags & (DPENUMPLAYERS_LOCAL | DPENUMPLAYERS_REMOTE | DPENUMPLAYERS_GROUP)
        | e.flags & (DPPLAYER_SERVERPLAYER | DPPLAYER_SPECTATOR)
}

/// Whether a group passes EnumGroups' `flags`: hidden ones only when
/// asked for, staging areas alone when asked for.
fn group_passes(e: &Entity, flags: u32) -> bool {
    !(flags & DPENUMPLAYERS_LOCAL != 0 && !e.local
        || flags & DPENUMPLAYERS_REMOTE != 0 && e.local
        || flags & DPENUMGROUPS_STAGINGAREA != 0 && e.flags & DPGROUP_STAGINGAREA == 0
        || flags & DPENUMGROUPS_HIDDEN == 0 && e.flags & DPGROUP_HIDDEN != 0)
}

fn group_flags(e: &Entity) -> u32 {
    e.flags & (DPGROUP_STAGINGAREA | DPGROUP_HIDDEN)
}

/// Start listing `items` to `callback`.
fn list(
    cpu: &mut Cpu,
    id: usize,
    vtable: usize,
    callback: u32,
    context: u32,
    items: Vec<(u32, u32)>,
) -> Option<u32> {
    let list = List {
        kind: calls::PLAYER_LIST,
        callback,
        context,
        wide: super::wide(vtable),
        object: id,
        version: if vtable == super::PLAY { 1 } else { 2 },
    };
    calls::players(cpu, list, &items)
}

/// EnumPlayers(this, instance, callback, context, flags).
pub fn enum_players(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, vtable) = object!(cpu, args[0]);
    let (callback, context, flags) = (args[2], args[3], args[4]);
    let all = match roster(cpu, id, args[1], callback, flags) {
        Ok(all) => all,
        Err(r) => return r,
    };
    let mut items = pick(&all, |e| {
        (!e.group && player_passes(e, flags)).then(|| player_flags(e, flags))
    });
    if flags & DPENUMPLAYERS_GROUP != 0 {
        items.extend(pick(&all, |e| e.group.then(|| group_flags(e))));
    }
    list(cpu, id, vtable, callback, context, items)
}

/// EnumGroups(this, instance, callback, context, flags): the groups at
/// the top.
pub fn enum_groups(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, vtable) = object!(cpu, args[0]);
    let (callback, context, flags) = (args[2], args[3], args[4]);
    let all = match roster(cpu, id, args[1], callback, flags) {
        Ok(all) => all,
        Err(r) => return r,
    };
    let items = pick(&all, |e| {
        (e.group && e.parent == 0 && group_passes(e, flags)).then(|| group_flags(e))
    });
    list(cpu, id, vtable, callback, context, items)
}

/// EnumGroupPlayers(this, group, instance, callback, context, flags).
pub fn enum_group_players(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, vtable) = object!(cpu, args[0]);
    let (group, callback, context, flags) = (args[1], args[3], args[4], args[5]);
    let all = match roster(cpu, id, args[2], callback, flags) {
        Ok(all) => all,
        Err(r) => return r,
    };
    let Some(g) = all.iter().find(|e| e.id == group && e.group) else {
        return Some(DPERR_INVALIDGROUP);
    };
    let members = g.players.clone();
    let items = pick(&all, |e| {
        (members.contains(&e.id) && player_passes(e, flags)).then(|| player_flags(e, flags))
    });
    list(cpu, id, vtable, callback, context, items)
}

/// EnumGroupsInGroup(this, group, instance, callback, context, flags):
/// the groups made in it, and those added to it as shortcuts.
pub fn enum_groups_in_group(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, vtable) = object!(cpu, args[0]);
    let (group, callback, context, flags) = (args[1], args[3], args[4], args[5]);
    let all = match roster(cpu, id, args[2], callback, flags) {
        Ok(all) => all,
        Err(r) => return r,
    };
    let Some(g) = all.iter().find(|e| e.id == group && e.group) else {
        return Some(DPERR_INVALIDGROUP);
    };
    let shortcuts = g.groups.clone();
    let items = pick(&all, |e| {
        if !e.group || !group_passes(e, flags) {
            return None;
        }
        if e.parent == group {
            Some(group_flags(e))
        } else {
            shortcuts
                .contains(&e.id)
                .then(|| group_flags(e) | DPENUMGROUPS_SHORTCUT)
        }
    });
    list(cpu, id, vtable, callback, context, items)
}

// --- Messages ---

/// Send(this, from, to, flags, data, size).
pub fn send(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, _) = object!(cpu, args[0]);
    answer(send_message(
        cpu, id, args[1], args[2], args[3], args[4], args[5],
    ))
}

fn send_message(
    cpu: &mut Cpu,
    id: usize,
    from: u32,
    to: u32,
    flags: u32,
    data: u32,
    size: u32,
) -> Answer {
    connected(cpu, id)?;
    if size > MAX_DATA {
        return Err(DPERR_INVALIDPARAMS);
    }
    let data = guest::read_data(cpu, data, size).ok_or(DPERR_INVALIDPARAMS)?;
    let guaranteed = flags & DPSEND_GUARANTEED != 0;
    change(cpu, id, |p, t| p.send_data(from, to, data, guaranteed, t))
}

/// SendEx(this, from, to, flags, data, size, priority, timeout, context,
/// message id): Send, which with DPSEND_ASYNC says it is pending and
/// tells the sender when it went.
pub fn send_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, _) = object!(cpu, args[0]);
    let [
        _,
        from,
        to,
        flags,
        data,
        size,
        priority,
        timeout,
        context,
        id_at,
    ] = args[..]
    else {
        return Some(DPERR_INVALIDPARAMS);
    };
    if let Err(e) = send_message(cpu, id, from, to, flags, data, size) {
        return Some(e);
    }
    if flags & DPSEND_ASYNC == 0 {
        return Some(DP_OK);
    }
    let time = (now(cpu) / 1000) as u32;
    let sys = Sys::SendComplete {
        from,
        to,
        flags,
        priority,
        timeout,
        context,
        id: 0,
        time,
    };
    let p = play(cpu, id);
    let message = if flags & DPSEND_NOSENDCOMPLETEMSG == 0 {
        p.send_complete(sys)
    } else {
        p.sends = p.sends.wrapping_add(1).max(1);
        p.sends
    };
    net::settle(cpu);
    if id_at != 0 {
        com::write_out(cpu, id_at, message);
    }
    Some(DPERR_PENDING)
}

/// SendChatMessage(this, from, to, flags, chat).
pub fn send_chat_message(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, vtable) = object!(cpu, args[0]);
    let r = (|| {
        connected(cpu, id)?;
        let chat = args[4];
        if chat == 0 || !cpu.bus.guest_probe(chat, 12, false) {
            return Err(DPERR_INVALIDPARAMS);
        }
        let at = cpu.bus.guest_read_32(chat + 8);
        let text = guest::read_str(cpu, at, super::wide(vtable)).ok_or(DPERR_INVALIDPARAMS)?;
        let text = text.unwrap_or_default();
        change(cpu, id, |p, t| p.chat(args[1], args[2], args[3], text, t))
    })();
    answer(r)
}

/// Receive(this, from, to, flags, data, size).
pub fn receive(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, from_at, to_at, flags, data, size_at] = args[..] else {
        return Some(DPERR_INVALIDPARAMS);
    };
    let (id, vtable) = object!(cpu, this);
    let flags = if flags == 0 { DPRECEIVE_ALL } else { flags };
    if flags & !(DPRECEIVE_ALL | DPRECEIVE_TOPLAYER | DPRECEIVE_FROMPLAYER | DPRECEIVE_PEEK) != 0 {
        return Some(DPERR_INVALIDFLAGS);
    }
    if from_at == 0 || to_at == 0 || size_at == 0 {
        return Some(DPERR_INVALIDPARAMS);
    }
    if !cpu.bus.guest_probe(from_at, 4, true) || !cpu.bus.guest_probe(to_at, 4, true) {
        return Some(DPERR_INVALIDPARAMS);
    }
    if play(cpu, id).provider == 0 {
        return Some(DPERR_NOMESSAGES);
    }
    net::poll(cpu);
    let want_to = (flags & DPRECEIVE_TOPLAYER != 0).then(|| cpu.bus.guest_read_32(to_at));
    let want_from = (flags & DPRECEIVE_FROMPLAYER != 0).then(|| cpu.bus.guest_read_32(from_at));
    let v1 = vtable == super::PLAY;
    let wide = super::wide(vtable);
    let p = play(cpu, id);
    if v1 {
        // IDirectPlay's programs never see the messages it doesn't have.
        p.queue.retain(|m| match &m.body {
            Body::System { sys } => guest::system1(sys).is_some(),
            Body::Data { .. } => true,
        });
    }
    let pick =
        |m: &Queued| want_to.is_none_or(|t| m.to == t) && want_from.is_none_or(|f| m.from == f);
    let Some(message) = p.take(pick, true) else {
        return Some(DPERR_NOMESSAGES);
    };
    let blob = match &message.body {
        Body::Data { data } => bytes_blob(data),
        Body::System { sys } if v1 => bytes_blob(&guest::system1(sys).unwrap_or_default()),
        Body::System { sys } => guest::system2(sys, wide),
    };
    if let Err(e) = give(cpu, &blob, data, size_at) {
        return Some(e);
    }
    cpu.bus.guest_write_32(from_at, message.from);
    cpu.bus.guest_write_32(to_at, message.to);
    if flags & DPRECEIVE_PEEK == 0 {
        play(cpu, id).take(|m| *m == message, false);
    }
    Some(DP_OK)
}

/// GetMessageCount(this, player, count).
pub fn get_message_count(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, _) = object!(cpu, args[0]);
    let r = (|| {
        connected(cpu, id)?;
        net::poll(cpu);
        let p = in_session(cpu, id)?;
        let player = args[1];
        if !p
            .session
            .as_ref()
            .unwrap()
            .player(player)
            .is_some_and(|e| e.local)
        {
            return Err(DPERR_INVALIDPLAYER);
        }
        let count = p.queue.iter().filter(|m| m.to == player).count() as u32;
        if !com::write_out(cpu, args[2], count) {
            return Err(DPERR_INVALIDPARAMS);
        }
        Ok(DP_OK)
    })();
    answer(r)
}

/// GetMessageQueue(this, from, to, flags, messages, bytes): what waits
/// to be received; nothing waits to be sent, which goes at once.
pub fn get_message_queue(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, from, to, flags, count_at, bytes_at] = args[..] else {
        return Some(DPERR_INVALIDPARAMS);
    };
    let (id, _) = object!(cpu, this);
    if play(cpu, id).provider == 0 {
        return Some(DPERR_UNINITIALIZED);
    }
    net::poll(cpu);
    let (mut count, mut bytes) = (0u32, 0u32);
    if flags & DPMESSAGEQUEUE_SEND == 0 {
        for m in &play(cpu, id).queue {
            if (from == 0 || m.from == from) && (to == 0 || m.to == to) {
                count += 1;
                bytes += match &m.body {
                    Body::Data { data } => data.len() as u32,
                    Body::System { sys } => guest::system2(sys, false).len(),
                };
            }
        }
    }
    if count_at != 0 {
        com::write_out(cpu, count_at, count);
    }
    if bytes_at != 0 {
        com::write_out(cpu, bytes_at, bytes);
    }
    Some(DP_OK)
}

/// CancelMessage(this, id, flags): a message goes at once, so only "all
/// of them" (0) can be asked for, and there is nothing to cancel.
pub fn cancel_message(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    object!(cpu, args[0]);
    Some(if args[1] == 0 {
        DP_OK
    } else {
        DPERR_UNKNOWNMESSAGE
    })
}

pub fn cancel_priority(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    object!(cpu, args[0]);
    Some(DP_OK)
}

/// What lobbies and secure sessions do, which there are none of here.
pub fn unsupported(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, _) = object!(cpu, args[0]);
    if play(cpu, id).provider == 0 {
        return Some(DPERR_UNINITIALIZED);
    }
    Some(DPERR_UNSUPPORTED)
}
