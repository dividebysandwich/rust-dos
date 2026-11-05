//! IDirectPlay's own methods, DirectX 3's DirectPlay: ANSI strings as
//! separate arguments instead of DPNAMEs, session descriptions with
//! fixed-size strings and a session number, and players whose event
//! DirectPlay makes. Its methods with the same arguments as
//! IDirectPlay2's (Send, Receive, Close, DestroyPlayer...) are those of
//! play.rs. Its programs get its own system messages (guest::system1).

use super::calls::{self, List};
use super::guest;
use super::play::{self, DPERR_UNSUPPORTED, answer};
use super::session::*;
use super::{
    DPERR_ALREADYINITIALIZED, DPERR_BUFFERTOOSMALL, PLAY, object, object_of, play as play_of,
};
use crate::cpu::Cpu;
use crate::win32::com::Guid;
use crate::win32::object::{self as kernel, Kind};

/// IDirectPlay's EnumPlayers flag: list the players of the session
/// enumerated before, instead of the open one, which isn't there.
const DPENUMPLAYERS_PREVIOUS: u32 = 0x4;

/// CreatePlayer(this, id, short name, long name, event): with an event
/// pointer, DirectPlay makes the player's event and writes its handle.
pub fn create_player(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, out, short, long, event_at] = args[..] else {
        return Some(DPERR_INVALIDPARAMS);
    };
    let (id, _) = object!(cpu, this);
    let r = (|| {
        if play_of(cpu, id).provider == 0 {
            return Err(super::DPERR_UNINITIALIZED);
        }
        let short = guest::read_str(cpu, short, false).ok_or(DPERR_INVALIDPARAMS)?;
        let long = guest::read_str(cpu, long, false).ok_or(DPERR_INVALIDPARAMS)?;
        if event_at != 0 && !cpu.bus.guest_probe(event_at, 4, true) {
            return Err(DPERR_INVALIDPARAMS);
        }
        if play_of(cpu, id).session.is_none() {
            return Err(DPERR_INVALIDPARAMS);
        }
        let (event, handle) = if event_at != 0 {
            let kind = Kind::Event {
                manual: false,
                signaled: false,
            };
            let handle = kernel::create(cpu, kind, None);
            let event = kernel::resolve(cpu, handle).unwrap_or(0);
            kernel::objects(cpu).hold(event);
            (event, handle)
        } else {
            (0, 0)
        };
        let r = play::make(
            cpu,
            id,
            false,
            out,
            (short, long),
            Vec::new(),
            0,
            0,
            event,
            handle,
        );
        match r {
            Ok(_) if event_at != 0 => cpu.bus.guest_write_32(event_at, handle),
            Err(_) if event != 0 => {
                kernel::close(cpu, handle);
                kernel::objects(cpu).release(event);
            }
            _ => {}
        }
        r
    })();
    answer(r)
}

/// CreateGroup(this, id, short name, long name).
pub fn create_group(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, out, short, long] = args[..] else {
        return Some(DPERR_INVALIDPARAMS);
    };
    let (id, _) = object!(cpu, this);
    let r = (|| {
        if play_of(cpu, id).provider == 0 {
            return Err(super::DPERR_UNINITIALIZED);
        }
        let short = guest::read_str(cpu, short, false).ok_or(DPERR_INVALIDPARAMS)?;
        let long = guest::read_str(cpu, long, false).ok_or(DPERR_INVALIDPARAMS)?;
        play::make(cpu, id, true, out, (short, long), Vec::new(), 0, 0, 0, 0)
    })();
    answer(r)
}

/// GetPlayerName(this, id, short, short size, long, long size): each
/// name to its buffer, or its size when the buffer is missing or small.
pub fn get_player_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, who, short_at, short_size, long_at, long_size] = args[..] else {
        return Some(DPERR_INVALIDPARAMS);
    };
    let (id, _) = object!(cpu, this);
    let p = play_of(cpu, id);
    if p.provider == 0 {
        return Some(super::DPERR_UNINITIALIZED);
    }
    let Some(e) = p.session.as_ref().and_then(|s| s.player(who)).cloned() else {
        return Some(DPERR_INVALIDPLAYER);
    };
    let mut result = DP_OK;
    for (text, at, size_at) in [
        (&e.short, short_at, short_size),
        (&e.long, long_at, long_size),
    ] {
        if size_at == 0 || !cpu.bus.guest_probe(size_at, 4, true) {
            return Some(DPERR_INVALIDPARAMS);
        }
        let bytes = guest::name1(text);
        let room = cpu.bus.guest_read_32(size_at);
        cpu.bus.guest_write_32(size_at, bytes.len() as u32);
        if at == 0 || room < bytes.len() as u32 {
            result = DPERR_BUFFERTOOSMALL;
            continue;
        }
        if !cpu.bus.guest_probe(at, bytes.len(), true) {
            return Some(DPERR_INVALIDPARAMS);
        }
        cpu.bus.guest_write_bytes(at, &bytes);
    }
    Some(result)
}

/// SetPlayerName(this, id, short name, long name).
pub fn set_player_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, who, short, long] = args[..] else {
        return Some(DPERR_INVALIDPARAMS);
    };
    let (id, _) = object!(cpu, this);
    let (Some(short), Some(long)) = (
        guest::read_str(cpu, short, false),
        guest::read_str(cpu, long, false),
    ) else {
        return Some(DPERR_INVALIDPARAMS);
    };
    if play_of(cpu, id).provider == 0 {
        return Some(super::DPERR_UNINITIALIZED);
    }
    let t = cpu.bus.clock.now_micros();
    super::net::prepare(cpu, id);
    let r = match play_of(cpu, id).session.is_some() {
        true => play_of(cpu, id).set_name(who, false, short, long, false, t),
        false => Err(DPERR_INVALIDPARAMS),
    };
    super::net::after(cpu, id);
    answer(r.map(|_| DP_OK))
}

/// EnableNewPlayers(this, enable): the host lets players join, or not.
pub fn enable_new_players(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, _) = object!(cpu, args[0]);
    let p = play_of(cpu, id);
    if p.provider == 0 {
        return Some(super::DPERR_UNINITIALIZED);
    }
    let Some(s) = p.session.as_ref() else {
        return Some(DPERR_INVALIDPARAMS);
    };
    let mut info = s.info.clone();
    let password = s.password.clone();
    if args[1] != 0 {
        info.flags &= !DPSESSION_NEWPLAYERSDISABLED;
    } else {
        info.flags |= DPSESSION_NEWPLAYERSDISABLED;
    }
    let t = cpu.bus.clock.now_micros();
    super::net::prepare(cpu, id);
    let r = play_of(cpu, id).set_desc(info, password, t);
    super::net::after(cpu, id);
    answer(r.map(|_| DP_OK))
}

/// EnumSessions(this, desc, timeout, callback, context, flags).
pub fn enum_sessions(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, desc, timeout, callback, context, flags] = args[..] else {
        return Some(DPERR_INVALIDPARAMS);
    };
    let (id, _) = object!(cpu, this);
    let Some((info, _, password)) = guest::read_desc1(cpu, desc) else {
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
        wide: false,
        object: id,
        version: 1,
    };
    // IDirectPlay has no async enumeration: its flags pick which
    // sessions are listed.
    play::enumerate(cpu, id, list, search, timeout, flags & 0x3)
}

/// Open(this, desc): make a session (DPOPEN_CREATESESSION), or join the
/// one found with the description's session number.
pub fn open(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, _) = object!(cpu, args[0]);
    let Some((mut info, flags, password)) = guest::read_desc1(cpu, args[1]) else {
        return Some(DPERR_INVALIDPARAMS);
    };
    if flags & 1 != 0 {
        let number = guest::session_number(&info.instance);
        let p = play_of(cpu, id);
        let found = p
            .found
            .iter()
            .find(|f| guest::session_number(&f.info.instance) == number);
        match found.map(|f| f.info.instance) {
            Some(instance) => info.instance = instance,
            None if play_of(cpu, id).joining.is_none() => return Some(DPERR_NOSESSIONS),
            None => {}
        }
    }
    play::open_session(cpu, id, info, password, flags & 3)
}

/// EnumPlayers(this, session, callback, context, flags), and
/// EnumGroups: the open session's.
fn enum_entities(cpu: &mut Cpu, args: &[u32], groups: bool) -> Option<u32> {
    let [this, _, callback, context, flags] = args[..] else {
        return Some(DPERR_INVALIDPARAMS);
    };
    let (id, vtable) = object!(cpu, this);
    if flags & DPENUMPLAYERS_PREVIOUS != 0 {
        return Some(DPERR_UNSUPPORTED);
    }
    let p = play_of(cpu, id);
    if p.provider == 0 {
        return Some(super::DPERR_UNINITIALIZED);
    }
    let Some(s) = p.session.as_ref() else {
        return Some(DPERR_INVALIDPARAMS);
    };
    let items: Vec<(u32, u32)> = s
        .entities
        .iter()
        .rev()
        .filter(|e| e.group == groups && (!groups || e.parent == 0))
        .map(|e| (e.id, e.flags & (DPPLAYER_SERVERPLAYER | DPPLAYER_SPECTATOR)))
        .collect();
    start_list(cpu, id, vtable, callback, context, items)
}

fn start_list(
    cpu: &mut Cpu,
    id: usize,
    vtable: usize,
    callback: u32,
    context: u32,
    items: Vec<(u32, u32)>,
) -> Option<u32> {
    if callback == 0 {
        return Some(DPERR_INVALIDPARAMS);
    }
    let list = List {
        kind: calls::PLAYER_LIST,
        callback,
        context,
        wide: false,
        object: id,
        version: if vtable == PLAY { 1 } else { 2 },
    };
    calls::players(cpu, list, &items)
}

pub fn enum_players(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    enum_entities(cpu, args, false)
}

pub fn enum_groups(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    enum_entities(cpu, args, true)
}

/// EnumGroupPlayers(this, group, callback, context, flags).
pub fn enum_group_players(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, group, callback, context, _] = args[..] else {
        return Some(DPERR_INVALIDPARAMS);
    };
    let (id, vtable) = object!(cpu, this);
    let p = play_of(cpu, id);
    if p.provider == 0 {
        return Some(super::DPERR_UNINITIALIZED);
    }
    let Some(g) = p.session.as_ref().and_then(|s| s.group(group)) else {
        return Some(DPERR_INVALIDGROUP);
    };
    let items = g.players.iter().rev().map(|&p| (p, 0)).collect();
    start_list(cpu, id, vtable, callback, context, items)
}

/// GetCaps(this, caps).
pub fn get_caps(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    play::get_caps(cpu, &[args[0], args[1], 0])
}

/// GetPlayerCaps(this, player, caps).
pub fn get_player_caps(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    play::get_player_caps(cpu, &[args[0], args[1], args[2], 0])
}

/// Initialize(this, provider): an object made without a provider
/// (CoCreateInstance, or DirectPlayCreate with GUID_NULL) takes one.
pub fn initialize(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, _) = object!(cpu, args[0]);
    if play_of(cpu, id).provider != 0 {
        return Some(DPERR_ALREADYINITIALIZED);
    }
    match Guid::read(cpu, args[1]) {
        Some(provider) if super::provider_code(&provider).is_some() => {
            Some(super::initialize(cpu, id, &provider))
        }
        _ => Some(DPERR_ALREADYINITIALIZED),
    }
}

/// SaveSession(this, reserved): DirectX 3 never had it working.
pub fn save_session(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    object!(cpu, args[0]);
    Some(DPERR_UNSUPPORTED)
}
