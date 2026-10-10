//! Deterministic mode: the machine runs on emulated time alone, so the same
//! program with the same input at the same emulated times executes the same
//! instructions and ends in the same state on every run, whatever the
//! host's speed.
//!
//! It runs the machine as the game suite's harness does
//! (`tests/gamesuite/machine.rs`):
//!
//! - The speed is a fixed number of instructions per emulated millisecond,
//!   never `auto` or `max`, which follow the host's clock.
//! - The machine's clock (the real-time clock, DOS's date and time, file
//!   times, and the BIOS tick count it starts with) starts at a fixed date
//!   and time and runs with emulated time: `hosttime` is set to it at every
//!   emulated millisecond.
//! - Execution runs in batches that end at whole emulated milliseconds.
//!   What happens between batches happens at those boundaries: the
//!   display's retrace latch at every millisecond, and mixing the sound and
//!   delivering input every `TICK_MS` milliseconds (`Boundary::tick`).
//!
//! The front end still decides how many milliseconds a host frame runs
//! (`frame_end`), which only changes how fast the run goes.

use crate::cpu::Cpu;
use crate::exec::{self, ExecHook, StopReason};
use chrono::{NaiveDate, NaiveDateTime, TimeDelta};
use std::cell::Cell;

const MS: u64 = 1_000_000;

/// Emulated milliseconds between the points where input is delivered and
/// the sound is mixed.
pub const TICK_MS: u64 = 10;

/// The speed when the settings ask for `auto` or `max`: `auto`'s speed for
/// real-mode programs.
pub const DEFAULT_CYCLES: u32 = crate::timer::AUTO_REAL_MODE_CYCLES;

/// Half the period of the text cursor's and blinking characters' blink.
const BLINK_NS: u64 = 500 * MS;

/// The date and time the clock starts at unless another is given, as in
/// the game suite.
pub fn default_start() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(1995, 4, 11).unwrap().and_hms_opt(12, 34, 56).unwrap()
}

/// A start time written `YYYY-MM-DD HH:MM:SS` (or with a `T` between the
/// date and the time, or without the seconds).
pub fn parse_start(s: &str) -> Result<NaiveDateTime, String> {
    let s = s.trim().replacen('T', " ", 1);
    ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M"]
        .iter()
        .find_map(|format| NaiveDateTime::parse_from_str(&s, format).ok())
        .ok_or_else(|| format!("invalid start time '{}': expected YYYY-MM-DD HH:MM:SS", s))
}

thread_local! {
    /// The generator's state in deterministic mode, on the machine's
    /// thread; None without the mode.
    static RANDOM: Cell<Option<u64>> = const { Cell::new(None) };
}

/// In deterministic mode, fill `buf` from the mode's generator (SplitMix64,
/// seeded with the start time) and return true; without it, return false
/// and leave `buf` alone. For what the machine makes up that a program
/// can see: the NE2000's and the IPX driver's addresses.
pub fn random_bytes(buf: &mut [u8]) -> bool {
    RANDOM.with(|state| {
        let Some(mut s) = state.get() else { return false };
        for chunk in buf.chunks_mut(8) {
            s = s.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = s;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            chunk.copy_from_slice(&z.to_le_bytes()[..chunk.len()]);
        }
        state.set(Some(s));
        true
    })
}

/// What a step reached: a whole emulated millisecond, and whether it is
/// one where input is delivered (every `TICK_MS`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Boundary {
    pub ms: u64,
    pub tick: bool,
}

pub struct Deterministic {
    /// The date and time at emulated time 0.
    pub start: NaiveDateTime,
    /// The tick (emulated time / `TICK_MS`) whose work was done last.
    last_tick: u64,
}

impl Deterministic {
    /// The mode with the clock at `start`, and the addresses the machine
    /// makes up (`random_bytes`) from a generator seeded with it. Make it
    /// before the machine, whose BIOS reads the clock as it starts.
    pub fn new(start: NaiveDateTime) -> Self {
        let mode = Self { start, last_tick: 0 };
        mode.set_clock(0);
        let seed = start.and_utc().timestamp() as u64;
        RANDOM.with(|state| state.set(Some(seed ^ 0x9E37_79B9_7F4A_7C15)));
        mode
    }

    /// Set this thread's clock to `ms` emulated milliseconds after the
    /// start.
    fn set_clock(&self, ms: u64) {
        crate::hosttime::fix(Some(self.start + TimeDelta::milliseconds(ms as i64)));
    }

    /// The end of a host frame's batch: `target`, the instruction count
    /// the pacer would run to, moved back to the last tick before it, so
    /// that frames end where a tick's work was done. Never before now.
    pub fn frame_end(&self, cpu: &Cpu, target: u64) -> u64 {
        let clock = &cpu.bus.clock;
        let tick_ns = TICK_MS * MS;
        let ns = clock.ns_at(target) / tick_ns * tick_ns;
        clock.icount_at_ns(ns).max(clock.icount)
    }

    /// Run to the next whole emulated millisecond, or to `end` if that
    /// comes first. At a whole millisecond the display latches what a
    /// retrace since latches, the sound is mixed at every tick, and the
    /// clock moves on. Returns why the run stopped and the millisecond it
    /// reached, if it did; on a tick the caller delivers input.
    pub fn step(&mut self, cpu: &mut Cpu, hook: &mut dyn ExecHook, hot: bool, end: u64) -> (StopReason, Option<Boundary>) {
        let boundary = (cpu.bus.clock.now_ns() / MS + 1) * MS;
        let step_end = cpu.bus.clock.icount_at_ns(boundary).min(end);
        cpu.bus.start_batch(step_end);
        let reason = exec::run_batch(cpu, hook, hot);
        let now = cpu.bus.clock.now_ns();
        if now < boundary {
            return (reason, None);
        }
        let ms = now / MS;
        cpu.bus.sync_display();
        // A tick crossed: by an instruction that took more than a
        // millisecond (a long I/O stall), or a state loaded since, too.
        let tick = ms / TICK_MS != self.last_tick;
        if tick {
            self.last_tick = ms / TICK_MS;
            cpu.bus.audio_catch_up();
        }
        self.set_clock(ms);
        (reason, Some(Boundary { ms, tick }))
    }

    /// Whether the text cursor and blinking characters show now: they
    /// blink with emulated time.
    pub fn blink_visible(&self, cpu: &Cpu) -> bool {
        (cpu.bus.clock.now_ns() / BLINK_NS).is_multiple_of(2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_times_parse_with_or_without_seconds() {
        let at = NaiveDate::from_ymd_opt(1994, 1, 2).unwrap().and_hms_opt(3, 4, 5).unwrap();
        assert_eq!(parse_start("1994-01-02 03:04:05"), Ok(at));
        assert_eq!(parse_start("1994-01-02T03:04:05"), Ok(at));
        assert_eq!(parse_start("1994-01-02 03:04"), Ok(at - TimeDelta::seconds(5)));
        assert!(parse_start("yesterday").is_err());
    }
}
