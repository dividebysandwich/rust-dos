//! DirectPlay's calls into the program, as a task (task.rs): the
//! service providers of DirectPlayEnumerate and EnumConnections, the
//! sessions EnumSessions found, and the players and groups of
//! EnumPlayers and its kind, one callback after another until the list
//! ends or the callback returns FALSE. What each callback gets is written
//! to the task's scratch space.
//!
//! The same task makes a method that waits (a sync EnumSessions, an Open
//! that joins) wait: its thread sleeps a while and then calls the method
//! again, which finds where it was in its object (`Play::waiting`,
//! `Play::joining`).

use super::guest::{self, Blob};
use super::session::DP_OK;
use super::{DPCONNECTION_DIRECTPLAY, PROVIDERS, objects};
use crate::cpu::Cpu;
use crate::win32::task::{self, Next, Task};
use crate::win32::thread::{self, Wait};

/// The kinds of calls (the task's variable 0).
pub const PROVIDER_LIST: u32 = 0;
pub const CONNECTION_LIST: u32 = 1;
pub const SESSION_LIST: u32 = 2;
pub const PLAYER_LIST: u32 = 3;
pub const WAIT: u32 = 4;

/// The variables: the kind, the callback, its context, where the list
/// is, whether strings are Unicode, the object, the interface's version
/// (1 for IDirectPlay), then the kind's own and the list's items.
const KIND: usize = 0;
const CALLBACK: usize = 1;
const CONTEXT: usize = 2;
const NEXT: usize = 3;
const WIDE: usize = 4;
const OBJECT: usize = 5;
const VERSION: usize = 6;
/// A session list's: the timeout's milliseconds and whether it is an
/// async enumeration; then the items, the found sessions' places.
const TIMEOUT: usize = 7;
const ASYNC: usize = 8;
const ITEMS: usize = 9;

/// EnumSessions' callback's flag for its last call.
const DPESC_TIMEDOUT: u32 = 1;

/// A call's start: its fixed variables.
pub struct List {
    pub kind: u32,
    pub callback: u32,
    pub context: u32,
    pub wide: bool,
    pub object: usize,
    pub version: u32,
}

impl List {
    fn vars(&self) -> Vec<u32> {
        vec![
            self.kind,
            self.callback,
            self.context,
            0,
            self.wide as u32,
            self.object as u32,
            self.version,
        ]
    }
}

/// List the service providers (DirectPlayEnumerate, EnumConnections).
pub fn providers(cpu: &mut Cpu, list: List) -> Option<u32> {
    task::start(cpu, task::DPLAY_CALL, list.vars(), 512)
}

/// List the found sessions at `items` (their places in `Play::found`),
/// then call once more with DPESC_TIMEDOUT.
pub fn sessions(
    cpu: &mut Cpu,
    list: List,
    timeout: u32,
    background: bool,
    items: &[usize],
) -> Option<u32> {
    let mut vars = list.vars();
    vars.push(timeout);
    vars.push(background as u32);
    vars.extend(items.iter().map(|&i| i as u32));
    task::start(cpu, task::DPLAY_CALL, vars, 4096)
}

/// List the players or groups `items`, each with the flags its callback
/// gets.
pub fn players(cpu: &mut Cpu, list: List, items: &[(u32, u32)]) -> Option<u32> {
    let mut vars = list.vars();
    vars.extend([0, 0]);
    for &(id, flags) in items {
        vars.extend([id, flags]);
    }
    task::start(cpu, task::DPLAY_CALL, vars, 4096)
}

/// Make the thread wait until `until` (the clock's microseconds) and
/// then call the method again.
pub fn wait(cpu: &mut Cpu, until: u64) -> Option<u32> {
    let vars = vec![WAIT, until as u32, (until >> 32) as u32];
    task::start(cpu, task::DPLAY_CALL, vars, 0)
}

pub fn sleep_and_retry(cpu: &mut Cpu, t: &Task, until: u64) -> Next {
    t.retry(cpu);
    thread::block(
        cpu,
        Wait {
            deadline: Some(until),
            ..Wait::default()
        },
    );
    Next::Stop
}

/// The task's step: the next item to the callback, or the wait.
pub fn step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    match t.var(KIND) {
        WAIT => {
            let until = t.var(1) as u64 | (t.var(2) as u64) << 32;
            sleep_and_retry(cpu, t, until)
        }
        PROVIDER_LIST | CONNECTION_LIST => provider_step(cpu, t, value),
        SESSION_LIST => session_step(cpu, t, value),
        PLAYER_LIST => player_step(cpu, t, value),
        _ => Next::Done(DP_OK),
    }
}

fn provider_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    if t.step == 1 && value == 0 {
        return Next::Done(DP_OK);
    }
    let next = t.var(NEXT) as usize;
    t.set(NEXT, next as u32 + 1);
    let Some(&(guid, name)) = PROVIDERS.get(next) else {
        return Next::Done(DP_OK);
    };
    let wide = t.var(WIDE) != 0;
    let (callback, context) = (t.var(CALLBACK), t.var(CONTEXT));
    let name: Vec<u16> = name.encode_utf16().collect();
    let at = t.scratch;
    guid.write(cpu, at);
    if t.var(KIND) == PROVIDER_LIST {
        // (GUID, name, major version, minor version, context).
        let text = guest::str_bytes(&name, wide);
        cpu.bus.guest_write_bytes(at + 16, &text);
        let args = [
            at,
            at + 16,
            super::PROVIDER_VERSION.0,
            super::PROVIDER_VERSION.1,
            context,
        ];
        return Next::call(callback, &args, 1);
    }
    // (GUID, connection, its size, name, flags, context).
    let address = super::address::of_provider(&guid);
    let name_at = at + 16 + address.len() as u32;
    cpu.bus.guest_write_bytes(at + 16, &address);
    let name = guest::name_blob(&Some(name), &None, wide);
    name.write(cpu, name_at);
    let args = [
        at,
        at + 16,
        address.len() as u32,
        name_at,
        DPCONNECTION_DIRECTPLAY,
        context,
    ];
    Next::call(callback, &args, 1)
}

fn session_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let (callback, context) = (t.var(CALLBACK), t.var(CONTEXT));
    let version = t.var(VERSION);
    let timeout_at = t.scratch;
    let desc_at = t.scratch + 8;
    let id = t.var(OBJECT) as usize;
    match t.step {
        // The callback said to stop.
        1 if value == 0 => return Next::Done(DP_OK),
        // The last call: TRUE asks a sync enumeration to go on, with the
        // timeout the callback may have changed.
        2 => {
            let background = t.var(ASYNC) != 0;
            if value == 0 || background {
                return Next::Done(DP_OK);
            }
            let ms = cpu.bus.guest_read_32(timeout_at);
            return super::play::enumerate_again(cpu, t, id, ms);
        }
        _ => {}
    }
    if t.step == 0 {
        cpu.bus.guest_write_32(timeout_at, t.var(TIMEOUT));
    }
    let next = t.var(NEXT) as usize;
    t.set(NEXT, next as u32 + 1);
    let found = t.vars.get(ITEMS + next).and_then(|&i| {
        objects(cpu)
            .get(id)?
            .as_ref()?
            .play
            .found
            .get(i as usize)
            .cloned()
    });
    let Some(found) = found else {
        // The last call, with no session.
        let args = if version == 1 {
            [0, context, timeout_at, DPESC_TIMEDOUT]
        } else {
            [0, timeout_at, DPESC_TIMEDOUT, context]
        };
        return Next::call(callback, &args, 2);
    };
    if version == 1 {
        let bytes = guest::desc1(&found.info);
        cpu.bus.guest_write_bytes(desc_at, &bytes);
        return Next::call(callback, &[desc_at, context, timeout_at, 0], 1);
    }
    let mut blob = Blob::default();
    guest::desc2(&mut blob, &found.info, &[], t.var(WIDE) != 0);
    if blob.len() > 4096 - 8 {
        blob = Blob::default();
        let info = super::session::Info {
            name: None,
            ..found.info.clone()
        };
        guest::desc2(&mut blob, &info, &[], false);
    }
    blob.write(cpu, desc_at);
    Next::call(callback, &[desc_at, timeout_at, 0, context], 1)
}

fn player_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    if t.step == 1 && value == 0 {
        return Next::Done(DP_OK);
    }
    let (callback, context) = (t.var(CALLBACK), t.var(CONTEXT));
    let id = t.var(OBJECT) as usize;
    loop {
        let next = t.var(NEXT) as usize;
        t.set(NEXT, next as u32 + 1);
        let (Some(&item), Some(&flags)) = (
            t.vars.get(ITEMS + 2 * next),
            t.vars.get(ITEMS + 2 * next + 1),
        ) else {
            return Next::Done(DP_OK);
        };
        // A player the callback destroyed meanwhile isn't listed.
        let entity = objects(cpu)
            .get(id)
            .and_then(Option::as_ref)
            .and_then(|o| o.play.known(item).cloned());
        let Some(e) = entity else {
            continue;
        };
        let at = t.scratch;
        if t.var(VERSION) == 1 {
            // (id, short name, long name, flags, context), ANSI.
            let short = guest::name1(&e.short);
            let long = guest::name1(&e.long);
            let (short, long) = (
                &short[..short.len().min(2047)],
                &long[..long.len().min(2047)],
            );
            cpu.bus.guest_write_bytes(at, short);
            cpu.bus.guest_write_bytes(at + 2048, long);
            cpu.bus.guest_write_8(at + 2047, 0);
            cpu.bus.guest_write_8(at + 4095, 0);
            return Next::call(callback, &[item, at, at + 2048, flags, context], 1);
        }
        // (id, player type, name, flags, context).
        let mut name = guest::name_blob(&e.short, &e.long, t.var(WIDE) != 0);
        if name.len() > 4096 {
            name = guest::name_blob(&None, &None, false);
        }
        name.write(cpu, at);
        let kind = (!e.group) as u32;
        return Next::call(callback, &[item, kind, at, flags, context], 1);
    }
}
