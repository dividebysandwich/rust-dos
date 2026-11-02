//! The multimedia timers: timeGetTime and the others that read the
//! clock, and timeSetEvent's timers, which call a function of the
//! program's or set an event once or every so many milliseconds.
//!
//! A timer's times are by the emulated clock, in microseconds. A
//! periodic timer's next time is its last one and a period, whenever
//! the call was made, so it keeps its period without drift; one that
//! fell behind catches up a call at a time. Timers of a process are
//! looked at on its multimedia thread (`poll`), which makes the calls
//! one after another as Windows 95's timer thread did: a callback slower
//! than the period delays the next one, and doesn't run twice at once.

use super::callback::{self, Callback};
use super::{MMSYSERR_INVALPARAM, MMSYSERR_NOERROR};
use crate::cpu::Cpu;
use crate::win32::object::{self, Kind};

pub const TIMERR_NOERROR: u32 = 0;
pub const TIMERR_NOCANDO: u32 = 97;

/// timeSetEvent's flags.
const TIME_PERIODIC: u32 = 0x0001;
const TIME_CALLBACK_EVENT_SET: u32 = 0x0010;
const TIME_CALLBACK_EVENT_PULSE: u32 = 0x0020;

/// The shortest and longest period, in milliseconds, as Windows 95 has
/// them in timeGetDevCaps.
const PERIOD_MIN: u32 = 1;
const PERIOD_MAX: u32 = 1_000_000;
/// The longest delay timeSetEvent takes.
const DELAY_MAX: u32 = 65_535;

/// MMTIME's wType for milliseconds.
const TIME_MS: u32 = 0x0001;

/// What a timer does when its time comes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Action {
    /// Calls the program's function `address` as
    /// `f(id, 0, user, 0, 0)`.
    #[default]
    None,
    Call {
        address: u32,
        user: u32,
    },
    /// Sets the event (by its object, which the timer holds), or pulses
    /// it.
    Set {
        event: u32,
    },
    Pulse {
        event: u32,
    },
}

/// A timer timeSetEvent made.
#[derive(Clone, Debug, Default)]
pub struct Timer {
    pub id: u32,
    /// The process it is in.
    pub pid: u32,
    /// Its period (or delay), in microseconds.
    pub period: u64,
    /// When it is next due, by the clock's microseconds.
    pub next: u64,
    pub periodic: bool,
    pub action: Action,
}

/// The timers, and what timeBeginPeriod asked for.
#[derive(Clone, Debug, Default)]
pub struct Timers {
    pub timers: Vec<Timer>,
    /// The id the last timer was given.
    pub last_id: u32,
    /// The periods timeBeginPeriod asked for and timeEndPeriod hasn't
    /// ended, which change nothing: the timers keep their times anyway.
    pub periods: Vec<u32>,
}

fn timers(cpu: &mut Cpu) -> &mut Timers {
    &mut cpu.bus.win32.winmm.timers
}

/// timeGetTime: the milliseconds since the machine started, by the
/// emulated clock. It reads the clock as GetTickCount does, so an idle
/// proof follows it (`idle::tick_count`).
pub fn time_get_time(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(crate::idle::tick_count(cpu))
}

pub fn now_ms(cpu: &Cpu) -> u32 {
    (cpu.bus.clock.now_micros() / 1000) as u32
}

/// timeGetSystemTime(mmtime, size): the same in an MMTIME, as
/// milliseconds.
pub fn time_get_system_time(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[at, size] = args else { unreachable!() };
    if size >= 12 && at != 0 {
        if !cpu.bus.guest_probe(at, 8, true) {
            return None;
        }
        let ms = now_ms(cpu);
        cpu.bus.guest_write_32(at, TIME_MS);
        cpu.bus.guest_write_32(at + 4, ms);
    }
    Some(MMSYSERR_NOERROR)
}

/// timeGetDevCaps(caps, size): the shortest and longest period. Windows
/// 95 can't do it with a TIMECAPS that is too small or missing.
pub fn time_get_dev_caps(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[at, size] = args else { unreachable!() };
    if at == 0 || size < 8 {
        return Some(TIMERR_NOCANDO);
    }
    if !cpu.bus.guest_probe(at, 8, true) {
        return None;
    }
    cpu.bus.guest_write_32(at, PERIOD_MIN);
    cpu.bus.guest_write_32(at + 4, PERIOD_MAX);
    Some(TIMERR_NOERROR)
}

/// timeBeginPeriod(period).
pub fn time_begin_period(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let period = args[0];
    if !(PERIOD_MIN..=PERIOD_MAX).contains(&period) {
        return Some(TIMERR_NOCANDO);
    }
    timers(cpu).periods.push(period);
    Some(TIMERR_NOERROR)
}

/// timeEndPeriod(period): ends a timeBeginPeriod of the same period.
pub fn time_end_period(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let period = args[0];
    if !(PERIOD_MIN..=PERIOD_MAX).contains(&period) {
        return Some(TIMERR_NOCANDO);
    }
    let periods = &mut timers(cpu).periods;
    match periods.iter().position(|&p| p == period) {
        Some(i) => {
            periods.remove(i);
            Some(TIMERR_NOERROR)
        }
        None => Some(TIMERR_NOCANDO),
    }
}

/// timeSetEvent(delay, resolution, proc, user, flags): a timer due in
/// `delay` milliseconds, and every `delay` after that if it is
/// periodic: its id, or 0.
pub fn time_set_event(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[delay, _resolution, proc, user, flags] = args else {
        unreachable!()
    };
    if !(1..=DELAY_MAX).contains(&delay) {
        return Some(0);
    }
    let action = if flags & (TIME_CALLBACK_EVENT_SET | TIME_CALLBACK_EVENT_PULSE) != 0 {
        let Some(event) = object::resolve(cpu, proc).filter(|&id| {
            matches!(
                object::objects(cpu).get(id).map(|o| &o.kind),
                Some(Kind::Event { .. })
            )
        }) else {
            return Some(0);
        };
        object::objects(cpu).hold(event);
        if flags & TIME_CALLBACK_EVENT_SET != 0 {
            Action::Set { event }
        } else {
            Action::Pulse { event }
        }
    } else {
        Action::Call {
            address: proc,
            user,
        }
    };
    if callback::ensure_thread(cpu).is_none() {
        release(cpu, &action);
        return Some(0);
    }
    let pid = crate::win32::user32::current_pid(cpu);
    let now = cpu.bus.clock.now_micros();
    let t = timers(cpu);
    t.last_id = t.last_id.wrapping_add(1).max(1);
    let id = t.last_id;
    let period = delay as u64 * 1000;
    t.timers.push(Timer {
        id,
        pid,
        period,
        next: now + period,
        periodic: flags & TIME_PERIODIC != 0,
        action,
    });
    super::schedule(cpu);
    Some(id)
}

/// Let go of the event a timer's action holds.
fn release(cpu: &mut Cpu, action: &Action) {
    if let Action::Set { event } | Action::Pulse { event } = *action {
        object::objects(cpu).release(event);
    }
}

/// timeKillEvent(id).
pub fn time_kill_event(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = args[0];
    let t = timers(cpu);
    let Some(i) = t.timers.iter().position(|t| t.id == id) else {
        return Some(MMSYSERR_INVALPARAM);
    };
    let timer = t.timers.remove(i);
    release(cpu, &timer.action);
    super::schedule(cpu);
    Some(TIMERR_NOERROR)
}

/// The timers of the process `pid` that are due at `now` do what they
/// do, each once (the multimedia thread calls this): the calls are
/// queued for the thread. Whether one did.
pub fn poll(cpu: &mut Cpu, pid: u32, now: u64) -> bool {
    let mut fired = Vec::new();
    let t = timers(cpu);
    t.timers.retain_mut(|timer| {
        if timer.pid != pid || timer.next > now {
            return true;
        }
        fired.push((timer.id, timer.action.clone()));
        timer.next += timer.period;
        timer.periodic
    });
    let any = !fired.is_empty();
    for (id, action) in fired {
        let periodic = timers(cpu).timers.iter().any(|t| t.id == id);
        match action {
            Action::None => {}
            Action::Call { address, user } => {
                callback::notify(cpu, &Callback::Function { pid, address }, id, 0, user, 0, 0);
            }
            Action::Set { event } => callback::set_event(cpu, event),
            Action::Pulse { event } => callback::pulse_event(cpu, event),
        }
        if !periodic {
            // A one-shot timer is gone once it fired.
            release(cpu, &action);
        }
    }
    any
}

/// When the next timer is due, if there is one.
pub fn next_due(cpu: &Cpu, pid: Option<u32>) -> Option<u64> {
    let timers = cpu.bus.win32.winmm.timers.timers.iter();
    timers
        .filter(|t| pid.is_none_or(|p| t.pid == p))
        .map(|t| t.next)
        .min()
}

/// A process ended: its timers go.
pub fn process_ended(cpu: &mut Cpu, pid: u32) {
    let gone: Vec<Timer> = {
        let t = timers(cpu);
        let (gone, kept) = t.timers.drain(..).partition(|t| t.pid == pid);
        t.timers = kept;
        gone
    };
    for timer in gone {
        release(cpu, &timer.action);
    }
}

crate::win32::state_variants!(Action {
    None {},
    Call { address, user },
    Set { event },
    Pulse { event },
});
crate::state_fields!(Timer {
    id,
    pid,
    period,
    next,
    periodic,
    action
});
crate::state_fields!(Timers {
    timers,
    last_id,
    periods
});
