//! Emulated time, the 8253 PIT channel 0 (IRQ 0) and CPU speed pacing.
//!
//! Emulated time is derived from the number of executed instructions at a
//! rate of `cycles_per_ms` instructions per emulated millisecond, the way
//! DOSBox counts cycles. Timer interrupts are therefore scheduled at exact
//! instruction counts: a program that reprograms the PIT to 150 Hz gets 150
//! evenly spaced IRQ 0s per emulated second, however the host happens to
//! batch the work between video frames. The `Pacer` keeps emulated time in
//! step with the wall clock.
//!
//! Time is measured in PIT input clock ticks (1.193182 MHz).

use crate::autospeed::{AutoSpeed, Backlog};
use crate::bus::Bus;
use crate::video::crt::CrtTiming;
use std::time::Duration;
use web_time::Instant;

pub use rust_dos_audio_core::{PIT_HZ, mul_div, mul_div_ceil};

/// Emulated milliseconds per host video frame.
const FRAME: Duration = Duration::from_micros(16_667);
/// How far emulated time may fall behind the wall clock before the backlog
/// is dropped rather than caught up.
pub(crate) const MAX_LAG: Duration = Duration::from_millis(50);
/// A backlog of this much is one gap: running frames, it is dropped at
/// `MAX_LAG`. The frontend stopped (a page in the background, a window
/// being dragged, the host asleep), which says nothing of whether the host
/// keeps up with the speed.
const GAP: Duration = Duration::from_secs(2);

/// How many frames of emulated time a frame runs while fast forwarding,
/// as far as the host keeps up.
const FAST_FORWARD_FRAMES: u32 = 8;

/// Speed range accepted for `cycles`, in instructions per emulated ms.
pub const MIN_CYCLES: u32 = 100;
pub const MAX_CYCLES: u32 = 2_000_000;
/// Starting point for `CpuSpeed::Max` before the first adjustment.
const MAX_INITIAL_CYCLES: u32 = 20_000;
/// `CpuSpeed::Auto`'s least speed unless it is given one, which real-mode
/// programs without frames to go by run at, DOSBox's: about a 286 at 12
/// MHz, which the speed-sensitive real-mode games of the 1980s run right
/// at.
pub const AUTO_REAL_MODE_CYCLES: u32 = 3000;
/// Share of each frame that `CpuSpeed::Max` gives to instruction execution.
/// The rest is left for rendering, audio and the host.
const MAX_BUSY_SHARE: f64 = 0.85;
/// Host time the instructions of a measurement of the host's speed take
/// at least. A browser's clock counts in steps of up to a millisecond, in
/// which a whole batch can fit: a batch measured to have taken no time
/// would make the host infinitely fast.
const HOST_SAMPLE: Duration = Duration::from_millis(10);
/// Instructions a frame has to run for a measurement of the host's speed
/// to take it: fewer, and the frame's time is mostly not theirs.
const SAMPLE_INSTRUCTIONS: u64 = 10_000;
/// How much a frame's counts weigh in the busy share of the recent frames
/// (`Pacer::end_cpu_frame`) against the next frame's: about half a second
/// of frames at 60 a second carries most of it.
const BUSY_DECAY: f64 = 0.97;
/// The share of the recent frames' counts the processor halted from which
/// GDI's blits no longer count as the host's work (`Pacer::end_cpu_frame`).
const BLITS_UNTIL_HALTED: f64 = 0.5;
/// How many times `SAMPLE_INSTRUCTIONS` a frame at the least speed `Max`
/// and `Auto` go down to has room for (`Pacer::least_timed`). A frame
/// doesn't run all of it: the guest halts or waits on a device for part
/// of it, and a retrace-paced batch can end early.
const TIMED_HEADROOM: f64 = 2.0;

/// Emulated time an I/O port access takes on the ISA bus, as in DOSBox.
/// Delay loops made of port reads (AdLib detection polls the status port
/// 200 times to wait out an 80 us timer) rely on it.
pub const IO_READ_NS: u64 = 1000;
pub const IO_WRITE_NS: u64 = 750;

fn duration_to_ticks(d: Duration) -> u64 {
    (d.as_nanos() * PIT_HZ as u128 / 1_000_000_000) as u64
}

fn ticks_to_duration(ticks: u64) -> Duration {
    Duration::from_nanos((ticks as u128 * 1_000_000_000 / PIT_HZ as u128) as u64)
}

/// How far past the start of a picture a batch that runs to it ends, so
/// that the display sees the retrace before it whatever the rounding
/// between instructions, ticks and nanoseconds.
const RETRACE_MARGIN_NS: u64 = 1000;

/// How long before a frame is due `sleep_until` stops sleeping and spins:
/// a sleep can overshoot by about that much, a millisecond with Windows'
/// timer, a tenth of that elsewhere. (Spinning takes the host's time.)
#[cfg(windows)]
const SPIN: Duration = Duration::from_millis(1);
#[cfg(not(windows))]
const SPIN: Duration = Duration::from_micros(200);

/// Wait until `at`, closer than a sleep alone would.
fn sleep_until(at: Instant) {
    let now = Instant::now();
    if at > now + SPIN {
        std::thread::sleep(at - now - SPIN);
    }
    while Instant::now() < at {
        std::thread::yield_now();
    }
}

/// A video frame (1/60 s) in PIT ticks.
pub fn frame_ticks() -> u64 {
    duration_to_ticks(FRAME)
}

/// Instruction counter and the mapping between instructions and emulated
/// time. The main loop advances `icount` and calls `Bus::service_timers`
/// whenever it reaches `deadline`.
pub struct Clock {
    /// Instructions executed since start (plus instructions skipped while
    /// the CPU waited for an interrupt).
    pub icount: u64,
    /// The next instruction count at which a timer event is due or the
    /// current execution batch ends, whichever comes first.
    pub deadline: u64,
    /// Instruction counts charged for I/O bus time rather than executed.
    pub stalled: u64,
    /// Instruction counts skipped while the CPU waited for an interrupt.
    pub idle: u64,
    /// Of `stalled`, the counts charged for work the emulator does in its
    /// own code for the emulated machine (the built-in GDI's blits): the
    /// host spends its time on it, as on instructions, so the host's speed
    /// counts them as run: all of them while the recent frames halted
    /// none of their counts, fewer the more they halted, and none from
    /// `BLITS_UNTIL_HALTED` of them (`Pacer::end_cpu_frame`).
    pub offloaded: u64,
    /// Of `idle`, the counts skipped while a disk service waited out the
    /// drive's time (`diskio::wait`): the program didn't ask for those.
    /// Only differences between two points count, so a save state leaves
    /// it out.
    pub disk_wait: u64,
    batch_end: u64,
    cycles_per_ms: u32,
    base_icount: u64,
    base_ticks: u64,
    /// `base_ticks` in nanoseconds, for events finer than a PIT tick
    /// (838 ns), such as the scanlines of the display.
    base_ns: u64,
    /// The time stamp counter's clock.
    pub tsc: Tsc,
}

/// The clock the time stamp counter counts: the instructions (the `cycles`
/// speed), or a fixed rate in emulated time, which a speed that changes as
/// it runs (`cycles=max` and `auto`) needs: a program that measures the
/// counter's rate once and times with it later would otherwise time wrong
/// (3DMark 2000 reported a quarter of its frame rates).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tsc {
    /// The count at instruction count `icount`.
    pub base: u64,
    pub icount: u64,
    /// Counts a second, or None for one an instruction.
    pub hz: Option<u64>,
}

impl Clock {
    pub fn new(cycles_per_ms: u32) -> Self {
        Self {
            icount: 0,
            deadline: u64::MAX,
            stalled: 0,
            idle: 0,
            offloaded: 0,
            disk_wait: 0,
            batch_end: u64::MAX,
            cycles_per_ms: cycles_per_ms.clamp(MIN_CYCLES, MAX_CYCLES),
            base_icount: 0,
            base_ticks: 0,
            base_ns: 0,
            tsc: Tsc::default(),
        }
    }

    pub fn cycles_per_ms(&self) -> u32 {
        self.cycles_per_ms
    }

    fn instructions_per_second(&self) -> u64 {
        self.cycles_per_ms as u64 * 1000
    }

    /// The instructions executed since start: `icount` less the counts
    /// skipped while waiting and charged for I/O.
    pub fn run_count(&self) -> u64 {
        self.icount.saturating_sub(self.idle + self.stalled)
    }

    /// Current emulated time in PIT ticks.
    pub fn now_ticks(&self) -> u64 {
        self.ticks_at(self.icount)
    }

    /// The emulated time in PIT ticks at instruction count `icount`.
    pub fn ticks_at(&self, icount: u64) -> u64 {
        let delta = icount.saturating_sub(self.base_icount);
        self.base_ticks + mul_div(delta, PIT_HZ, self.instructions_per_second())
    }

    /// Current emulated time in nanoseconds.
    pub fn now_ns(&self) -> u64 {
        let delta = self.icount.saturating_sub(self.base_icount);
        self.base_ns + mul_div(delta, 1_000_000, self.cycles_per_ms as u64)
    }

    /// The emulated time in nanoseconds at instruction count `icount`.
    pub fn ns_at(&self, icount: u64) -> u64 {
        let delta = icount.saturating_sub(self.base_icount);
        self.base_ns + mul_div(delta, 1_000_000, self.cycles_per_ms as u64)
    }

    /// The first instruction count at which `now_ns() >= ns`.
    pub fn icount_at_ns(&self, ns: u64) -> u64 {
        let delta = ns.saturating_sub(self.base_ns);
        self.base_icount.saturating_add(mul_div_ceil(delta, self.cycles_per_ms as u64, 1_000_000))
    }

    /// `end`, the instruction count a batch would end at, or the picture's
    /// first line if `end` lies in the blanking after a vertical retrace of
    /// the display (`refresh`'s). The frame is composed from the machine as
    /// the batch left it, all at once, and a program that moves the Start
    /// Address and redraws to match in the blanking has only done half of
    /// that there: Prehistorik 2's difficulty screen showed its text out of
    /// place in such frames, about one in twenty. A monitor shows no
    /// blanking. At most a quarter of a frame is added (`blanking_end`), and
    /// the next batch gives it back, as it runs to the same time it would
    /// have.
    pub fn past_blanking(&self, end: u64, refresh: &CrtTiming) -> u64 {
        refresh.blanking_end(self.ns_at(end)).map_or(end, |ns| self.icount_at_ns(ns))
    }

    /// Current emulated time in microseconds.
    pub fn now_micros(&self) -> u64 {
        mul_div(self.now_ticks(), 1_000_000, PIT_HZ)
    }

    /// The emulated time in microseconds at instruction count `icount`.
    pub fn micros_at(&self, icount: u64) -> u64 {
        mul_div(self.ticks_at(icount), 1_000_000, PIT_HZ)
    }

    /// The first instruction count at which `now_micros() >= micros`.
    pub fn icount_at_micros(&self, micros: u64) -> u64 {
        self.icount_at(mul_div_ceil(micros, PIT_HZ, 1_000_000))
    }

    /// The first instruction count at which `now_ticks() >= ticks`.
    pub fn icount_at(&self, ticks: u64) -> u64 {
        let delta = ticks.saturating_sub(self.base_ticks);
        self.base_icount.saturating_add(mul_div_ceil(delta, self.instructions_per_second(), PIT_HZ))
    }

    /// The first instruction count at which `now_micros() / 1000` is `ms`
    /// or more.
    pub fn icount_at_ms(&self, ms: u64) -> u64 {
        self.icount_at(mul_div_ceil(ms * 1000, PIT_HZ, 1_000_000))
    }

    /// Change the speed from the current instruction onwards. Callers on the
    /// bus must recompute the deadline afterwards (`Bus::set_cycles_per_ms`).
    pub fn set_cycles_per_ms(&mut self, cycles_per_ms: u32) {
        if self.tsc.hz.is_some() {
            self.rebase_tsc();
        }
        self.base_ticks = self.now_ticks();
        self.base_ns = self.now_ns();
        self.base_icount = self.icount;
        self.cycles_per_ms = cycles_per_ms.clamp(MIN_CYCLES, MAX_CYCLES);
    }

    /// The time stamp counter, before the processor's offset: it goes on
    /// while the processor waits for an interrupt.
    pub fn tsc(&self) -> u64 {
        let delta = self.icount.saturating_sub(self.tsc.icount);
        let counts = match self.tsc.hz {
            None => delta,
            Some(hz) => mul_div(delta, hz, self.instructions_per_second()),
        };
        self.tsc.base.wrapping_add(counts)
    }

    /// Make the time stamp counter count `hz` a second of emulated time
    /// from now on, or the instructions (None), going on from where it is.
    pub fn set_tsc_hz(&mut self, hz: Option<u64>) {
        if hz != self.tsc.hz {
            self.rebase_tsc();
            self.tsc.hz = hz;
        }
    }

    fn rebase_tsc(&mut self) {
        self.tsc.base = self.tsc();
        self.tsc.icount = self.icount;
    }

    pub fn batch_end(&self) -> u64 {
        self.batch_end
    }

    pub fn set_batch_end(&mut self, batch_end: u64) {
        self.batch_end = batch_end;
    }

    /// Recompute `deadline` from the next timer event (in PIT ticks).
    pub fn schedule(&mut self, next_event: Option<u64>) {
        let event = next_event.map_or(u64::MAX, |t| self.icount_at(t));
        self.deadline = event.min(self.batch_end);
    }

    /// Let `ns` nanoseconds of emulated time pass without executing
    /// anything, for an I/O port access.
    pub fn stall(&mut self, ns: u64) {
        let n = self.stall_count(ns);
        self.icount += n;
        self.stalled += n;
    }

    /// `stall` for `ns` nanoseconds, `times` over: port accesses made at
    /// once.
    pub fn stall_times(&mut self, ns: u64, times: u64) {
        let n = self.stall_count(ns) * times;
        self.icount += n;
        self.stalled += n;
    }

    /// `stall` for `ns` nanoseconds of work done in the emulator's own code
    /// (`offloaded`).
    pub fn stall_offloaded(&mut self, ns: u64) {
        let n = self.stall_count(ns);
        self.icount += n;
        self.stalled += n;
        self.offloaded += n;
    }

    /// The instruction count `stall` adds for `ns` nanoseconds.
    pub fn stall_count(&self, ns: u64) -> u64 {
        self.cycles_per_ms as u64 * ns / 1_000_000
    }

    /// Let emulated time pass without executing anything until PIT tick
    /// `ticks`, for a device that holds the bus that long.
    pub fn stall_to(&mut self, ticks: u64) {
        let target = self.icount_at(ticks);
        if target > self.icount {
            self.stalled += target - self.icount;
            self.icount = target;
        }
    }

    /// Fast-forward to the deadline, for a CPU that is waiting for an
    /// interrupt. Returns the number of instructions skipped.
    pub fn skip_to_deadline(&mut self) -> u64 {
        let target = self.deadline.min(self.batch_end);
        let skipped = target.saturating_sub(self.icount);
        self.icount += skipped;
        self.idle += skipped;
        skipped
    }
}

/// An 8254 counter whose gate is always up: channel 0, which drives IRQ 0,
/// and channel 1, the DRAM refresh request. Only the timing is modelled;
/// port-level byte sequencing (LSB/MSB toggles, latches) lives on the bus.
#[derive(Clone, Debug)]
pub struct Pit0 {
    /// Counter mode, 0..=5 (6 and 7 alias 2 and 3).
    mode: u8,
    /// Count loaded at each reload, 1..=65536.
    reload: u32,
    /// Count written while a mode 2/3 period was running. It takes effect
    /// at `pending_at`, as on the 8254: at the end of the current
    /// half-cycle in mode 3, of the current period in mode 2.
    pending_reload: Option<u32>,
    /// False after a control word until the first count is written.
    counting: bool,
    /// Where a control word stopped the count: reads return it until a
    /// count is written.
    stopped: u16,
    /// Whether the counter will raise IRQ 0 at `next_tc`. One-shot modes
    /// disarm after firing until a new count is written.
    armed: bool,
    period_start: u64,
    next_tc: u64,
    /// How far into its period the count was at `period_start`: a mode 3
    /// count loaded at the end of a high half-cycle starts with the low
    /// half. Saved in the 8254 section, as are the next two.
    pub(crate) phase: u64,
    /// The tick `pending_reload` loads at.
    pub(crate) pending_at: u64,
    /// Bits 5-0 of the last control word (access, mode as written and
    /// BCD), which the read-back command's status byte gives back.
    pub(crate) control: u8,
}

impl Default for Pit0 {
    fn default() -> Self {
        Self::new()
    }
}

/// The value a mode 3 count reads `into` ticks into its period. It runs
/// down by two from the count, rounded down to an even number, in each
/// half: an odd count N loads N - 1 and is high for one tick longer than
/// it is low (the 8254 data sheet's mode 3).
fn square_wave_count(reload: u64, into: u64) -> u16 {
    let high = reload.div_ceil(2);
    let into_half = if into < high { into } else { into - high };
    ((reload & !1) - 2 * into_half) as u16
}

impl Pit0 {
    /// Power-on state as left by the BIOS: mode 3, count 65536 (18.2 Hz).
    pub fn new() -> Self {
        Self {
            mode: 3,
            reload: 0x10000,
            pending_reload: None,
            counting: true,
            stopped: 0,
            armed: true,
            period_start: 0,
            next_tc: 0x10000,
            phase: 0,
            pending_at: 0,
            control: 0x36,
        }
    }

    /// Channel 1 as the BIOS leaves it: mode 2, count 18, a refresh
    /// request every 15 microseconds.
    pub fn refresh() -> Self {
        let mut counter = Self::new();
        counter.set_mode(0x14, 0);
        counter.write_count(18, 0);
        counter
    }

    fn periodic(&self) -> bool {
        matches!(self.mode, 2 | 3)
    }

    /// Current reload value, 1..=65536.
    pub fn reload(&self) -> u32 {
        self.reload
    }

    /// Counter mode, 0..=5.
    pub fn mode(&self) -> u8 {
        self.mode
    }

    /// False between a control word and the first count written after it.
    pub fn is_counting(&self) -> bool {
        self.counting
    }

    /// Time of the next IRQ 0 edge, or of the end of the half-cycle a new
    /// mode 3 count loads at, if one is coming. The count programs write
    /// again on every tick, the one already loaded, gets no event of its
    /// own: loading it at the half-cycle changes no edge, and `advance`
    /// or the next count write makes the load.
    pub fn next_event(&self) -> Option<u64> {
        if !(self.counting && self.armed) {
            return None;
        }
        Some(match self.pending_reload {
            Some(reload) if self.pending_at < self.next_tc && reload != self.reload => {
                self.pending_at
            }
            _ => self.next_tc,
        })
    }

    /// Control word for the counter at `now`: set the mode and stop
    /// counting until a count is written. The count stays where it got to
    /// (Future Crew's demos time a frame by stopping the counter so and
    /// reading it). `control` is the control word; its bits 5-0 are kept.
    pub fn set_mode(&mut self, control: u8, now: u64) {
        self.stopped = self.count(now);
        self.control = control & 0x3F;
        let mode = (control >> 1) & 0x07;
        self.mode = if mode >= 6 { mode - 4 } else { mode };
        self.counting = false;
        self.armed = false;
        self.pending_reload = None;
    }

    /// A complete count was written (0 means 65536).
    pub fn write_count(&mut self, count: u16, now: u64) {
        let reload = if count == 0 { 0x10000 } else { count as u32 };
        self.load_at_half_cycle(now);
        if self.counting && self.periodic() {
            let current = self.reload as u64;
            let into = (now.saturating_sub(self.period_start) + self.phase) % current;
            let high = current.div_ceil(2);
            let left = if self.mode == 3 && into < high {
                high - into
            } else {
                current - into
            };
            self.pending_reload = Some(reload);
            self.pending_at = now + left;
            return;
        }
        self.reload = reload;
        self.pending_reload = None;
        self.counting = true;
        self.armed = true;
        self.period_start = now;
        self.phase = 0;
        self.next_tc = now + reload as u64;
    }

    /// Load a mode 3 count written in a high half-cycle if that half has
    /// ended by `now`: it loads where the output goes low, and runs the
    /// low half of its own period first. No event comes at the load, so
    /// whatever changes the counter makes it first.
    fn load_at_half_cycle(&mut self, now: u64) {
        if let Some(reload) = self.pending_reload
            && self.pending_at < self.next_tc
            && self.pending_at <= now
        {
            self.pending_reload = None;
            self.reload = reload;
            self.period_start = self.pending_at;
            self.phase = (reload as u64).div_ceil(2);
            self.next_tc = self.pending_at + reload as u64 - self.phase;
        }
    }

    /// Advance to `now`. Returns true if at least one IRQ 0 edge happened.
    pub fn advance(&mut self, now: u64) -> bool {
        if !(self.counting && self.armed) {
            return false;
        }
        let mut edge = false;
        loop {
            self.load_at_half_cycle(now);
            if self.next_tc > now {
                return edge;
            }
            edge = true;
            if !self.periodic() {
                // One-shot: the output stays high until the next count
                // write, while the count keeps running down through zero.
                self.armed = false;
                return true;
            }
            self.period_start = self.next_tc;
            self.phase = 0;
            if let Some(reload) = self.pending_reload
                && self.pending_at <= self.period_start
            {
                self.pending_reload = None;
                self.reload = reload;
            }
            let reload = self.reload as u64;
            if self.pending_reload.is_none() {
                // Skip whole periods arithmetically; several may have
                // passed.
                let behind = now - self.period_start;
                self.period_start += behind / reload * reload;
            }
            self.next_tc = self.period_start + reload;
        }
    }

    /// This counter with a pending count that is due by `now` loaded.
    fn settled(&self, now: u64) -> Option<Self> {
        let due = self.counting && self.pending_reload.is_some() && self.pending_at <= now;
        due.then(|| {
            let mut settled = self.clone();
            settled.advance(now);
            settled
        })
    }

    /// Ticks into the current period at `now`, for a periodic mode.
    fn period_position(&self, now: u64) -> u64 {
        (now.saturating_sub(self.period_start) + self.phase) % self.reload as u64
    }

    /// The value a counter read returns at `now`. The count runs down from
    /// the reload value; periodic modes restart at each terminal count (the
    /// square wave of mode 3 counts down by two, twice a period, once for
    /// each half of the wave), the one-shot modes wrap to FFFFh and go on.
    pub fn count(&self, now: u64) -> u16 {
        if !self.counting {
            return self.stopped;
        }
        if let Some(settled) = self.settled(now) {
            return settled.count(now);
        }
        if self.periodic() {
            let reload = self.reload as u64;
            let into = self.period_position(now);
            if self.mode == 3 {
                square_wave_count(reload, into)
            } else {
                (reload - into) as u16
            }
        } else {
            // One-shot counters keep counting down through zero.
            let elapsed = now.saturating_sub(self.period_start);
            (self.reload as u64).wrapping_sub(elapsed) as u16
        }
    }

    /// The counter's output at `now`.
    pub fn output(&self, now: u64) -> bool {
        if !self.counting {
            // Mode 0's output goes low at the control word, the others'
            // high.
            return self.mode != 0;
        }
        if let Some(settled) = self.settled(now) {
            return settled.output(now);
        }
        let reload = self.reload as u64;
        let elapsed = now.saturating_sub(self.period_start);
        match self.mode {
            2 => self.period_position(now) != reload - 1,
            3 => self.period_position(now) < reload.div_ceil(2),
            0 | 1 => elapsed >= reload,
            _ => elapsed != reload,
        }
    }

    /// The parts a state saved before the 8254 section lacks: a pending
    /// count loads at the end of the period, as it did then, and the
    /// control word's bits are those `access` and the mode give.
    pub(crate) fn without_8254_state(&mut self, access: u8) {
        self.phase = 0;
        self.pending_at = self.next_tc;
        self.control = access << 4 | self.mode << 1;
    }

    /// The status byte the read-back command latches at `now`: the output,
    /// the null count flag (a count or control word written and not yet
    /// loaded into the counting element) and the control word's bits 5-0.
    pub fn status(&self, now: u64) -> u8 {
        let null = !self.counting || self.pending_reload.is_some() && self.pending_at > now;
        (self.output(now) as u8) << 7 | (null as u8) << 6 | self.control
    }
}

/// How fast the emulated CPU runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CpuSpeed {
    /// As many instructions per millisecond as the host can sustain in real
    /// time, adjusted every frame.
    Max,
    /// A fixed number of instructions per emulated millisecond.
    Fixed(u32),
    /// The speed the running program needs, found from the frames it
    /// draws and its waiting (see autospeed.rs); at least this, the speed
    /// real-mode programs run at when they show nothing to go by.
    Auto(u32),
}

impl Default for CpuSpeed {
    fn default() -> Self {
        CpuSpeed::Auto(AUTO_REAL_MODE_CYCLES)
    }
}

impl std::fmt::Display for CpuSpeed {
    /// As `parse` reads it.
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match *self {
            CpuSpeed::Max => write!(f, "max"),
            CpuSpeed::Fixed(n) => write!(f, "{}", n),
            CpuSpeed::Auto(AUTO_REAL_MODE_CYCLES) => write!(f, "auto"),
            CpuSpeed::Auto(n) => write!(f, "auto {}", n),
        }
    }
}

impl CpuSpeed {
    /// Parse `max`, `auto`, `auto` with its least speed (`auto 5000`), or
    /// an instruction count per millisecond.
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        let count = |n: &str| match n.parse::<u32>() {
            Ok(n) if (MIN_CYCLES..=MAX_CYCLES).contains(&n) => Some(n),
            _ => None,
        };
        let mut words = s.split_whitespace();
        let speed = match (words.next(), words.next(), words.next()) {
            (Some(w), None, _) if w.eq_ignore_ascii_case("max") => Some(CpuSpeed::Max),
            (Some(w), None, _) if w.eq_ignore_ascii_case("auto") => Some(CpuSpeed::default()),
            (Some(w), Some(n), None) if w.eq_ignore_ascii_case("auto") => count(n).map(CpuSpeed::Auto),
            (Some(n), None, _) => count(n).map(CpuSpeed::Fixed),
            _ => None,
        };
        speed.ok_or_else(|| {
            format!(
                "invalid cycles '{}': expected auto, max or {}-{}",
                s, MIN_CYCLES, MAX_CYCLES
            )
        })
    }

    pub fn initial_cycles(self) -> u32 {
        match self {
            CpuSpeed::Max => MAX_INITIAL_CYCLES,
            CpuSpeed::Fixed(n) | CpuSpeed::Auto(n) => n,
        }
    }

    /// The speed a step slower or faster (the hotkeys): 10% of the speed
    /// the CPU runs at, `current`, rounded to hundreds. Slower from max or
    /// auto starts from the speed they reached, as does faster from auto;
    /// faster from max stays there.
    pub fn stepped(self, current: u32, faster: bool) -> CpuSpeed {
        if self == CpuSpeed::Max && faster {
            return CpuSpeed::Max;
        }
        let step = (current / 10).max(100);
        let next = if faster { current.saturating_add(step) } else { current.saturating_sub(step) };
        CpuSpeed::Fixed((next.div_ceil(100) * 100).clamp(MIN_CYCLES, MAX_CYCLES))
    }
}

/// The frame rates `auto_fps` takes: the frame rate `CpuSpeed::Auto` aims
/// for in place of the display's refresh rate.
pub const AUTO_FPS_RANGE: std::ops::RangeInclusive<u32> = 10..=240;

/// Parse `auto_fps`: `auto` (None, the display's refresh rate) or a frame
/// rate.
pub fn parse_auto_fps(s: &str) -> Result<Option<u32>, String> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("auto") {
        return Ok(None);
    }
    match s.parse::<u32>() {
        Ok(n) if AUTO_FPS_RANGE.contains(&n) => Ok(Some(n)),
        _ => Err(format!(
            "invalid auto_fps '{}': expected auto or {}-{}",
            s,
            AUTO_FPS_RANGE.start(),
            AUTO_FPS_RANGE.end()
        )),
    }
}

/// `auto_fps` as `parse_auto_fps` reads it.
pub fn auto_fps_text(fps: Option<u32>) -> String {
    fps.map_or("auto".to_string(), |n| n.to_string())
}

/// Keeps emulated time in step with the wall clock, one video frame at a
/// time, and tunes the speed in `CpuSpeed::Max` and `Auto` modes.
pub struct Pacer {
    speed: CpuSpeed,
    /// Finds `Auto`'s speed, and whether the program it measures switched
    /// to protected mode.
    auto: AutoSpeed,
    protected: bool,
    /// As fast as the host keeps up with, estimated from its frames.
    host: HostEstimate,
    /// While the program gets all the host has, whether the last frame
    /// timed said the host can't run the speed it ran at (`cap_boost`).
    boost_short: bool,
    /// The frames measured for the host's speed so far: the instructions
    /// they ran, the time those took and the rest of their work.
    sample: HostSample,
    /// `Clock::offloaded`, `Clock::idle` and `Clock::icount` as the last
    /// frame left them.
    offloaded: u64,
    idle: u64,
    icount: u64,
    /// The counts of the recent frames and those of them the processor
    /// halted, each frame's weighing `BUSY_DECAY` times the next one's.
    recent_ran: f64,
    recent_idle: f64,
    /// The emulated time dropped behind the wall clock so far
    /// (`MAX_LAG`), stalls and lag alike (`backlog` sorts them for `Auto`).
    /// And the emulated time the last frame ended at, and whether it ran
    /// on from the one before: the backlog after frames of a paused
    /// machine, which the frontend goes on drawing, is the pause's.
    lost_ns: u64,
    last_ns: u64,
    ran: bool,
    /// Of `lost_ns`, what was lag rather than a stall, which `Auto` goes by.
    backlog: Backlog,
    /// Whether the frames run by the wall clock (`batch_end`). A frontend
    /// that runs them at its own pace (libretro) drops nothing, so that
    /// `lost_ns` says nothing and the host estimate is all there is.
    wall_paced: bool,
    /// Wall-clock instant that corresponds to emulated time `anchor_ticks`.
    anchor_wall: Instant,
    anchor_ticks: u64,
    next_frame: Instant,
    /// Fast forward (held Alt+F12): emulated time runs ahead of the wall
    /// clock.
    fast_forward: bool,
    /// When the frame the batch runs to is due on the wall clock, and the
    /// emulated time it ends at (`retrace_batch_end`).
    deadline: Option<(Instant, u64)>,
    /// Whether this frame was shown at its deadline (`wait_to_present`).
    presented: bool,
    /// The emulated time the batch covers, a frame's.
    period: Duration,
}

impl Pacer {
    pub fn new(speed: CpuSpeed, auto_fps: Option<u32>, now: Instant) -> Self {
        Self {
            speed,
            auto: AutoSpeed::new(speed.initial_cycles(), auto_fps),
            protected: false,
            host: HostEstimate::default(),
            boost_short: false,
            sample: HostSample::default(),
            offloaded: 0,
            idle: 0,
            icount: 0,
            recent_ran: 0.0,
            recent_idle: 0.0,
            lost_ns: 0,
            last_ns: 0,
            ran: false,
            backlog: Backlog::default(),
            wall_paced: false,
            anchor_wall: now,
            anchor_ticks: 0,
            next_frame: now,
            fast_forward: false,
            deadline: None,
            presented: false,
            period: FRAME,
        }
    }

    /// Start or stop fast forwarding. When it stops, emulated time goes on
    /// from where it got to, rather than waiting for the wall clock to catch
    /// up with it. `Auto` measures afresh: the program and the host are the
    /// same, so how the host kept up stays.
    pub fn set_fast_forward(&mut self, on: bool, clock: &Clock, now: Instant) {
        if self.fast_forward && !on {
            self.auto.start_over();
            self.backlog.start_over();
            self.anchor_to(clock, now);
        }
        self.fast_forward = on;
    }

    /// Go on from the clock's time as it is now, after it jumped (a save
    /// state loaded): without this, emulated time ahead of the wall clock
    /// would stand still until the wall clock caught up, and time behind it
    /// would race to catch up. `Auto` starts over, and the bus forgets the
    /// speed it had to go back to (`Bus::end_work_boost`).
    pub fn rebase(&mut self, bus: &mut Bus, now: Instant) {
        self.auto.reset();
        self.backlog.start_over();
        bus.activity.work_return = None;
        self.anchor_to(&bus.clock, now);
    }

    /// Make `now` the wall-clock time of the clock's emulated time.
    fn anchor_to(&mut self, clock: &Clock, now: Instant) {
        self.anchor_wall = now;
        self.anchor_ticks = clock.now_ticks();
        self.next_frame = now;
        self.deadline = None;
    }

    pub fn fast_forward(&self) -> bool {
        self.fast_forward
    }

    /// Change the speed, as from the settings window. The caller sets the
    /// clock's rate (`CpuSpeed::initial_cycles`).
    pub fn set_speed(&mut self, speed: CpuSpeed) {
        if let CpuSpeed::Auto(base) = speed {
            self.auto = self.auto.with(base, self.auto.target_fps());
        }
        self.speed = speed;
    }

    /// Change the frame rate `Auto` aims for (`auto_fps`), None for the
    /// display's refresh rate; the search starts over.
    pub fn set_auto_fps(&mut self, fps: Option<u32>) {
        self.auto = self.auto.with(self.auto.base(), fps);
    }

    /// A pacer for `settings`' speed, `auto_fps` and `auto_busy_os`.
    pub fn for_settings(settings: &crate::config::Settings, now: Instant) -> Self {
        let mut pacer = Self::new(settings.cycles, settings.auto_fps, now);
        pacer.set_busy_os(settings.auto_busy_os);
        pacer
    }

    /// Whether `Auto` gives a program that keeps an idling protected-mode
    /// system busy all the host has (`auto_busy_os`).
    pub fn set_busy_os(&mut self, on: bool) {
        self.auto.set_busy_os(on);
    }

    /// `end_frame` for `cpu`'s machine. `Auto` takes its CPU model, whose
    /// clock sets how slow a protected-mode program may go
    /// (`CpuModel::auto_least`) and which the settings window can change,
    /// and whether its system was seen idling (`Cpu::os_idles`, which a
    /// state loaded brings); the machine keeps what `Auto` saw. The
    /// built-in Windows is such a system from the start: its idle loop
    /// halts the processor whenever none of its threads can run, so a
    /// Win32 program that keeps it from halting is at work, even one that
    /// never waits long enough for `Auto` to see the halt. A client of the
    /// emulator's DPMI host has no system under it (`set_dpmi_client`).
    /// `Auto` also counts the DOS programs started and ended
    /// (`set_programs`).
    pub fn end_cpu_frame(
        &mut self,
        cpu: &mut crate::cpu::Cpu,
        executed: u64,
        exec: Duration,
        overhead: Duration,
    ) -> Option<u32> {
        self.auto.set_protected_least(cpu.model.auto_least());
        self.auto.set_idles(cpu.os_idles || cpu.bus.win32.active());
        self.auto.set_dpmi_client(cpu.bus.dpmi.active() && !cpu.bus.win32.active());
        self.auto.set_programs(cpu.programs.started + cpu.programs.ended);
        // What the idle skip's watching cost (`Observe::take_watched`) is
        // work of the frame's own, as the rest of `overhead` is: counted
        // per instruction, the proofs that come with each timer event would
        // make the host look slower at running code than it is. A frame
        // that ran little but the watched instructions counts as one that
        // hardly ran anything, as a frame of waiting does: it mostly
        // waited, in the passes that were skipped.
        let (watched, run) = cpu.bus.observe.take_watched();
        let watched = watched.min(exec);
        let (executed, exec, overhead) =
            (executed.saturating_sub(run), exec - watched, overhead + watched);
        // The emulated time the built-in GDI's blits took (`Clock::offloaded`)
        // is work the host did in `exec`, in its own code: it runs as many
        // of those counts in that time as of instructions. Left out, the
        // host would look as much slower as the program blits, and a speed
        // it has the time for would never be tried: a game that draws its
        // frames through GDI would leave the host idle the blits' share of
        // the time, at a speed whose blits take longer than the host's.
        //
        // That holds for a program that keeps the processor busy: at a
        // higher speed it draws more frames, and blits in the same share
        // of its work. One that waits between its frames (a movie at its
        // own pace) blits as often at any speed and has time to spare at
        // the speed it runs, so the estimate doesn't hold it back, and its
        // blits say nothing of what a higher speed would bring, which is
        // instructions. Counted in full, the blits of Chaos Overlords'
        // movie, halted 90 % of the time, took the estimate to 2.5 times
        // the speed the host runs the game's next work at, and `Auto` went
        // there when that work began. The blits count in full while the
        // recent frames (about half a second's, as a single frame's would
        // give a movie's frame that decodes and blits) halted none of
        // their counts, not at all once they halted `BLITS_UNTIL_HALTED`
        // of them, and in proportion between.
        let clock = &cpu.bus.clock;
        let offloaded = clock.offloaded.saturating_sub(self.offloaded);
        let ran = clock.icount.saturating_sub(self.icount);
        let idle = clock.idle.saturating_sub(self.idle).min(ran);
        (self.offloaded, self.idle, self.icount) = (clock.offloaded, clock.idle, clock.icount);
        self.recent_ran = self.recent_ran * BUSY_DECAY + ran as f64;
        self.recent_idle = self.recent_idle * BUSY_DECAY + idle as f64;
        let halted = if self.recent_ran > 0.0 {
            self.recent_idle / self.recent_ran
        } else {
            1.0
        };
        let share = (1.0 - halted / BLITS_UNTIL_HALTED).clamp(0.0, 1.0);
        let executed = executed + (offloaded as f64 * share) as u64;
        let next = self.end_frame(&cpu.bus, cpu.pm_latched, executed, exec, overhead);
        cpu.os_idles = self.auto.idles();
        // What a device may go back to in the middle of the next frame
        // (`Bus::end_work_boost`): only a speed `Auto` set.
        cpu.bus.activity.work_return = match self.speed {
            CpuSpeed::Auto(_) => self.auto.work_return(),
            _ => None,
        };
        next
    }

    /// The instruction count the next batch should run to, so emulated time
    /// catches up with the wall clock at `now`. If emulation fell too far
    /// behind (slow host, debugger pause), the backlog is dropped and only
    /// one frame's worth is scheduled.
    pub fn batch_end(&mut self, clock: &Clock, now: Instant) -> u64 {
        self.wall_paced = true;
        self.deadline = None;
        self.period = FRAME;
        let emulated = clock.now_ticks();
        if self.fast_forward {
            return clock.icount_at(emulated + duration_to_ticks(FRAME) * FAST_FORWARD_FRAMES as u64);
        }
        let wall =
            self.anchor_ticks + duration_to_ticks(now.saturating_duration_since(self.anchor_wall));
        let target = if wall > emulated + duration_to_ticks(MAX_LAG) {
            self.anchor_wall = now;
            self.anchor_ticks = emulated + duration_to_ticks(FRAME);
            self.lose(wall - self.anchor_ticks);
            self.anchor_ticks
        } else {
            wall.max(emulated)
        };
        clock.icount_at(target)
    }

    /// Like `batch_end`, but on to where the display (`refresh`'s) next
    /// starts a picture after the wall clock, past the vertical retrace that
    /// latched its Start Address, noting when that is due on the wall
    /// clock, which `wait_to_present` waits for: a frame shown for each
    /// retrace, a display with a variable refresh rate refreshes at the
    /// emulated one, 70 Hz or whatever the CRTC's registers make it, rather
    /// than the host's frames'. Ending at the picture's start rather than
    /// the retrace's lets the frame have what programs set during the
    /// retrace for it, such as Jazz Jackrabbit's pixel panning, which goes
    /// with the Start Address it set before. A host that falls behind skips
    /// retraces rather than slowing the machine down.
    pub fn retrace_batch_end(&mut self, clock: &Clock, refresh: &CrtTiming, now: Instant) -> u64 {
        self.wall_paced = true;
        if self.fast_forward {
            return self.batch_end(clock, now);
        }
        let emulated = clock.now_ticks();
        let mut wall =
            self.anchor_ticks + duration_to_ticks(now.saturating_duration_since(self.anchor_wall));
        if wall > emulated + duration_to_ticks(MAX_LAG) {
            self.lose(wall - emulated);
            self.anchor_wall = now;
            self.anchor_ticks = emulated;
            wall = emulated;
        }
        // The picture's start in the display's nanoseconds, which count
        // from where the clock's ticks do.
        let from_ns = clock.now_ns() + ticks_to_duration(wall.saturating_sub(emulated)).as_nanos() as u64;
        let display_ns = refresh.next_display(from_ns) + RETRACE_MARGIN_NS;
        let ahead = (((display_ns - clock.now_ns()) as u128 * PIT_HZ as u128).div_ceil(1_000_000_000)) as u64;
        let target = emulated + ahead;
        self.deadline = Some((self.anchor_wall + ticks_to_duration(target - self.anchor_ticks), target));
        self.period = Duration::from_nanos(refresh.frame_ns());
        clock.icount_at(target)
    }

    /// Emulated time fell `ticks` behind the wall clock, and the backlog is
    /// dropped: lost, unless the machine was paused until now or the
    /// frontend stopped (`GAP`).
    fn lose(&mut self, ticks: u64) {
        let backlog = ticks_to_duration(ticks);
        if self.ran && backlog < GAP {
            self.lost_ns += backlog.as_nanos() as u64;
        }
    }

    /// The emulated time dropped behind the wall clock so far, stalls and
    /// lag alike: `Auto` goes by the lag only (`Backlog::frame`).
    pub fn lost(&self) -> Duration {
        Duration::from_nanos(self.lost_ns)
    }

    /// Wait for the frame's deadline (`retrace_batch_end`), if the batch
    /// got to it, so it is shown then. Returns how long it waited, which
    /// isn't the emulator's work.
    pub fn wait_to_present(&mut self, clock: &Clock) -> Duration {
        let start = Instant::now();
        if let Some((at, ticks)) = self.deadline.take()
            && clock.now_ticks() >= ticks
        {
            sleep_until(at);
            self.next_frame = at;
            self.presented = true;
        }
        start.elapsed()
    }

    /// Frame bookkeeping after a batch, on `bus` as it is after it. At max,
    /// retune the speed so that executing one frame's worth of
    /// instructions plus the rest of the frame's work (`overhead`) fits
    /// into `MAX_BUSY_SHARE` of a frame. At auto, the speed the running
    /// program needs (`AutoSpeed`), which each measurement keeps to what
    /// the host's estimate and the emulated time it dropped allow;
    /// `protected` says whether it switched to protected mode
    /// (`Cpu::pm_latched`), and a program starting or ending starts the
    /// search over. `executed` counts only instructions that actually ran,
    /// not ones skipped while waiting for an interrupt.
    /// Returns the new speed if it changed.
    pub fn end_frame(
        &mut self,
        bus: &Bus,
        protected: bool,
        executed: u64,
        exec: Duration,
        overhead: Duration,
    ) -> Option<u32> {
        let current = bus.clock.cycles_per_ms();
        let ns = bus.clock.now_ns();
        self.ran = ns > self.last_ns;
        self.last_ns = ns;
        let frame_ns = self.period.as_nanos() as f64;
        let ideal_of = |ns_per_instr: f64, overhead_ns: f64| {
            let budget_ns = (frame_ns * MAX_BUSY_SHARE - overhead_ns).max(frame_ns * 0.1);
            budget_ns / ns_per_instr / (frame_ns / 1_000_000.0)
        };
        // At auto, a frame of a stall stays out of the samples
        // (`HostEstimate::outlier`).
        let frame_ideal = (executed >= SAMPLE_INSTRUCTIONS).then(|| {
            ideal_of(
                exec.as_nanos() as f64 / executed as f64,
                overhead.as_nanos() as f64,
            )
        });
        let outlier = matches!(self.speed, CpuSpeed::Auto(_))
            && frame_ideal.is_some_and(|ideal| self.host.outlier(ideal));
        let ideal = if outlier {
            None
        } else {
            self.sample
                .add(executed, exec, overhead)
                .map(|(ns_per_instr, overhead_ns)| ideal_of(ns_per_instr, overhead_ns))
        };
        match self.speed {
            CpuSpeed::Fixed(_) => None,
            CpuSpeed::Max => {
                // Move gradually so one unusual frame can't swing the speed.
                let next = match ideal {
                    Some(ideal) => ideal.clamp(current as f64 * 0.9, current as f64 * 1.1),
                    // A frame that ran something (not one paused in the
                    // settings window), too little for a measurement to
                    // take it, at a speed too low for a frame to have
                    // room for that: nothing would measure the host again,
                    // and a speed that a busy host pushed down this far
                    // would stay. Go up until frames can be timed. Frames
                    // a measurement took and is still adding up don't
                    // count: the measurement decides.
                    None if (1..SAMPLE_INSTRUCTIONS).contains(&executed)
                        && current < self.least_timed() =>
                    {
                        current as f64 * 1.1
                    }
                    None => return None,
                };
                let next = (next as u32).clamp(MIN_CYCLES, MAX_CYCLES);
                (next != current).then_some(next)
            }
            CpuSpeed::Auto(_) => {
                if let Some(ideal) = ideal {
                    self.host.add(ideal, current);
                }
                self.cap_boost(frame_ideal, current);
                // No lower than frames can be timed at, or a busy host could
                // cap the speed where nothing measures it again (with a base
                // speed below that, as `auto 300` has in real mode).
                let least = self.least_timed();
                let host_max = self
                    .host
                    .max
                    .map_or(MAX_CYCLES, |max| (max as u32).clamp(least, MAX_CYCLES));
                if protected != self.protected {
                    self.protected = protected;
                    // A program switched to protected mode: the search
                    // starts over. The protected-mode program ended (or
                    // the machine rebooted): whatever runs next starts
                    // from the real-mode speed, with no idle loop seen.
                    if protected {
                        self.auto.reset();
                    } else {
                        self.auto.left_protected_mode();
                        let base = self.auto.base();
                        return (base != current).then_some(base);
                    }
                }
                // Fast forwarding, the host's time says nothing of the
                // program's needs.
                if self.fast_forward {
                    return None;
                }
                if self.wall_paced {
                    // How far the emulated time is behind the wall clock
                    // now: what the next batch runs to catch up, as
                    // `batch_end` reckons it.
                    let since = Instant::now().saturating_duration_since(self.anchor_wall);
                    let wall = self.anchor_ticks + duration_to_ticks(since);
                    let behind = ticks_to_duration(wall.saturating_sub(bus.clock.now_ticks()));
                    let behind_ns = behind.as_nanos() as u64;
                    // A frame of a paused machine (the debugger, the
                    // settings window, a rewind) runs nothing and falls
                    // behind the wall clock by the pause: no batch_end
                    // moves the anchor until it resumes.
                    if self.ran {
                        let lag = self.backlog.frame(behind_ns, self.lost_ns, current);
                        if let Some(cut) = self.auto.falling_behind(bus, lag) {
                            return Some(cut);
                        }
                    }
                    let lag_ns = self.backlog.lag_ns();
                    return self.auto.update(bus, protected, host_max, lag_ns);
                }
                let next = self
                    .auto
                    .update(bus, protected, host_max, self.backlog.lag_ns());
                // Nothing tells that the host fell behind, but for its
                // estimate: a heavier scene than it keeps up with at this
                // speed.
                match next.unwrap_or(current) {
                    speed if speed > host_max && current > host_max => Some(host_max),
                    speed => (speed != current).then_some(speed),
                }
            }
        }
    }

    /// The least speed whose frames have room for `TIMED_HEADROOM` times
    /// the instructions a measurement of the host takes
    /// (`SAMPLE_INSTRUCTIONS`): about 1,200 instructions per ms at 60
    /// frames a second. Below it, frames would hardly ever be timed.
    fn least_timed(&self) -> u32 {
        let frame_ms = self.period.as_secs_f64() * 1000.0;
        let least = (SAMPLE_INSTRUCTIONS as f64 * TIMED_HEADROOM / frame_ms).ceil();
        (least as u32).clamp(MIN_CYCLES, MAX_CYCLES)
    }

    /// While the program gets all the host has (`AutoSpeed::flat_out`),
    /// its speed is the host's estimate and follows it down
    /// (`AutoSpeed::update`), with no room above what the host runs. The
    /// estimate leaves out up to `STALL_FRAMES` slow frames in a row as a
    /// stall's (`HostEstimate::outlier`), and each of those runs as many
    /// times slower than real time as the frame is slower than the
    /// estimate. Zanzarah's intro movie starts right after a load the
    /// estimate was taken on, at 1,074,449, with frames of 250 ms: eight
    /// of them were two seconds of a frozen movie (#1223).
    ///
    /// So from the `TRY_BEHIND_FRAMES`th such frame in a row, as many late
    /// frames as end a step up (`AutoSpeed::falling_behind`), a cap brings
    /// the speed half the way down to that frame's (`HOST_FALL`). From then
    /// on, two frames in a row short of the speed (`HOST_SHORT`) move it
    /// half the way down to the second, as two such samples move the
    /// estimate: one alone may be a present or a shader compiled, which
    /// says nothing of the program's work (`HostEstimate::add`). The
    /// frames are the emulator's, which run whether the program presents
    /// or not, so a load without frames of its own is watched the same way.
    ///
    /// The cap is the boost's (`AutoSpeed::boost_cap`): the next
    /// measurement decides from the estimate and drops it. The estimate
    /// stays as the stall rule keeps it: a load's stall of three frames or
    /// more (a disc read, a shader compiled) says nothing of the host's
    /// speed at what the program does after the load, and an estimate, or
    /// a measurement, taken down by it would start that from too low a
    /// speed.
    fn cap_boost(&mut self, frame_ideal: Option<f64>, current: u32) {
        if !self.auto.flat_out() {
            self.boost_short = false;
            return;
        }
        let Some(ideal) = frame_ideal else {
            return;
        };
        let short = ideal < current as f64 * HOST_SHORT;
        let was_short = std::mem::replace(&mut self.boost_short, short);
        let run = self.host.outliers;
        let from = match self.auto.boost_cap() {
            Some(cap) if short && was_short => cap,
            None if run >= crate::autospeed::TRY_BEHIND_FRAMES => current as f64,
            _ => return,
        };
        if ideal < from {
            // No lower than frames can be timed at (see `end_frame`).
            let cap = (from + (ideal - from) * HOST_FALL).max(self.least_timed() as f64);
            self.auto.cap_boost(cap);
            if crate::autospeed::trace() {
                eprintln!(
                    "[auto] boost capped at {cap:.0}: a frame at {ideal:.0}, {run} stalled \
                     in a row (estimate {:.0})",
                    self.host.max.unwrap_or(0.0)
                );
            }
        }
    }

    /// Sleep until the next video frame is due; fast forwarding, or once
    /// the frame was shown at its deadline (`wait_to_present`), not at all.
    pub fn wait_for_next_frame(&mut self) {
        if std::mem::take(&mut self.presented) {
            return;
        }
        if self.fast_forward {
            self.next_frame = Instant::now();
            return;
        }
        self.next_frame += FRAME;
        let now = Instant::now();
        if self.next_frame > now {
            std::thread::sleep(self.next_frame - now);
        } else if now - self.next_frame > FRAME {
            // Too far behind to catch up; start counting from now.
            self.next_frame = now;
        }
    }
}

/// Stalls of the host made on purpose, to test how `Auto` takes them:
/// `RUST_DOS_TEST_STALL` lists them as `AT_MS:LENGTH_MS`, separated by
/// commas, and the frame that first runs past each emulated time sleeps
/// for that long while its instructions run, as a read of a file the host
/// doesn't have in its cache does. The variable is read once; without it,
/// no frame sleeps. Returns the stall due by `now_ns`, if one is.
pub fn test_stall(now_ns: u64) -> Option<Duration> {
    use std::sync::{Mutex, OnceLock};
    /// The stalls not yet made, as (emulated ms, length in ms).
    type Stalls = Mutex<Vec<(u64, u64)>>;
    static STALLS: OnceLock<Option<Stalls>> = OnceLock::new();
    let stalls = STALLS.get_or_init(|| {
        let text = std::env::var("RUST_DOS_TEST_STALL").ok()?;
        let mut stalls: Vec<(u64, u64)> = text
            .split(',')
            .filter_map(|stall| {
                let (at, length) = stall.trim().split_once(':')?;
                Some((at.trim().parse().ok()?, length.trim().parse().ok()?))
            })
            .collect();
        // The latest first, so the next due is the last.
        stalls.sort_unstable_by(|a, b| b.cmp(a));
        Some(Mutex::new(stalls))
    });
    let mut stalls = stalls.as_ref()?.lock().ok()?;
    let &(at_ms, length_ms) = stalls.last()?;
    if now_ns < at_ms.saturating_mul(1_000_000) {
        return None;
    }
    stalls.pop();
    Some(Duration::from_millis(length_ms))
}

/// Frames summed up until their instructions took long enough to time
/// (`HOST_SAMPLE`).
#[derive(Default)]
struct HostSample {
    executed: u64,
    exec: Duration,
    overhead: Duration,
    frames: u32,
}

impl HostSample {
    /// Add a frame that ran `executed` instructions in `exec`, with
    /// `overhead` of other work. Once there are enough, the nanoseconds an
    /// instruction took and a frame's other work, and start over. Frames
    /// that hardly ran anything (waiting for an interrupt) don't count.
    fn add(&mut self, executed: u64, exec: Duration, overhead: Duration) -> Option<(f64, f64)> {
        if executed < SAMPLE_INSTRUCTIONS {
            return None;
        }
        self.executed += executed;
        self.exec += exec;
        self.overhead += overhead;
        self.frames += 1;
        if self.exec < HOST_SAMPLE {
            return None;
        }
        let sample = std::mem::take(self);
        Some((
            sample.exec.as_nanos() as f64 / sample.executed as f64,
            sample.overhead.as_nanos() as f64 / sample.frames as f64,
        ))
    }
}

/// `Auto`'s estimate of as fast as the host keeps up with, from the
/// samples of its speed (`HostSample`).
#[derive(Default)]
struct HostEstimate {
    max: Option<f64>,
    /// Whether the last sample said the host can't run the speed it was
    /// taken at.
    short: bool,
    /// The samples taken so far, up to `HOST_SEED`, and the frames in a
    /// row left out as a stall's (`outlier`).
    samples: u32,
    outliers: u32,
}

impl HostEstimate {
    /// Whether a frame whose instructions and other work say the host
    /// keeps up with `ideal` is a stall's, which the samples leave out: a
    /// read of a file the host doesn't have in its cache, done while the
    /// instructions run, or a shader compiled. Its frames are `STALL_COST`
    /// times as slow as the estimate or more, alone or a few in a row
    /// between frames at the host's speed. Grom's loads (#783) read their
    /// files a piece at a time, and with them not in the host's cache every
    /// third frame said 24,000 to 28,000 against an estimate of 230,000:
    /// two such samples in a row move it half the way down each
    /// (`HOST_FALL`), and a few of them took it to 4,500. More such frames
    /// in a row than a stall takes (`STALL_FRAMES`) are the host's speed at
    /// the program's work now, and count.
    ///
    /// A program whose own work makes every few frames that slow reads high
    /// the same way: 2.7 times what the host runs with every third frame
    /// six times as slow, 1.5 times with every tenth. The pace bounds its
    /// speed. A heavy frame drops time only where it ends `MAX_LAG` behind,
    /// and at six times the light frames' cost and 50 frames a second or
    /// more, that takes light frames filling 0.58 of a frame or more. At
    /// half the estimate they fill 0.43 (`MAX_BUSY_SHARE` at all of it), so
    /// the drops come above half the estimate, where a measurement counts
    /// all its lag (`STALLED` in `AutoSpeed::update`), and the limits from
    /// a host that fell behind hold the speed. Frames many times heavier
    /// than that drop time under half the estimate too, which the
    /// measurements leave out as stalls: those can't be told from a load
    /// read cold.
    fn outlier(&mut self, ideal: f64) -> bool {
        let far = self
            .max
            .is_some_and(|max| ideal * crate::autospeed::STALL_COST as f64 <= max);
        if !far {
            self.outliers = 0;
            return false;
        }
        self.outliers = self.outliers.saturating_add(1);
        self.outliers <= crate::autospeed::STALL_FRAMES
    }

    /// Add a sample that says `ideal`, taken at the speed `current`. The
    /// estimate moves a tenth of the way to each sample, so that a light
    /// stretch of the program moves it little. Two samples in a row that
    /// say the host can't run the speed it runs at move it half the way
    /// down: the program went from work the host runs fast (a loading
    /// loop) to work it runs slowly (drawing in a banked VESA window), and
    /// the speed, which follows the estimate down, would otherwise stay
    /// above what the host keeps up with for a dozen slow frames or more, a
    /// second at a third of real time (Shannara's scene changes).
    ///
    /// A sample says that only below `HOST_SHORT` of the speed. A program
    /// that runs flat out runs at the estimate, and samples scatter around
    /// it: half of them fall below it, and half the way down to those
    /// against a tenth up to the others would hold the estimate under what
    /// the host has, by two thirds of the scatter.
    ///
    /// One such sample alone moves it a tenth. A stall of the host, or a
    /// frame whose other work took long (a present, a shader compiled),
    /// shrinks the time a sample's instructions had (`MAX_BUSY_SHARE`, down
    /// to a tenth of a frame), and drops one sample below the speed of any
    /// program running at more than a tenth of what the host has. Half the
    /// way down from there would cut the speed of every program above half
    /// the estimate, for a moment that says nothing of its work.
    ///
    /// The first `HOST_SEED` samples set it to the most of them. Before
    /// them there is nothing to tell a stall by, and the first sample of a
    /// run that a stall fell in said 1,200 to 2,500 where the host ran
    /// 15,000 (Doom with its files not in the host's cache): taken as the
    /// estimate, the first load ran at that, and a tenth of the way up a
    /// sample took seconds to make up for it.
    fn add(&mut self, ideal: f64, current: u32) {
        let short = ideal < current as f64 * HOST_SHORT;
        let was_short = std::mem::replace(&mut self.short, short);
        let seeding = self.samples < HOST_SEED;
        self.samples = (self.samples + 1).min(HOST_SEED);
        self.max = Some(match self.max {
            None => ideal,
            Some(max) if seeding => max.max(ideal),
            Some(max) => {
                let share = if short && was_short && ideal < max {
                    HOST_FALL
                } else {
                    HOST_DRIFT
                };
                max + (ideal - max) * share
            }
        });
    }
}

/// The share of the way to a sample of the host's speed that `Auto`'s
/// estimate of it moves (`HostEstimate::add`).
const HOST_DRIFT: f64 = 0.1;
/// The share it moves down when two samples in a row say the host can't
/// run the speed it runs at.
const HOST_FALL: f64 = 0.5;
/// The first samples, the most of which is the estimate. Chosen: a sample
/// is a frame, and in Doom's run the first said 1,200 to 2,500 where the
/// host ran 15,000, so the most of three passes over one or two stalled
/// frames. A run whose first three all stall starts low and climbs by
/// `HOST_DRIFT`, as it did without the seed.
const HOST_SEED: u32 = 3;
/// The share of the speed below which a sample says the host can't run
/// it: too far under the speed to be the scatter of samples taken at the
/// estimate.
const HOST_SHORT: f64 = 0.8;

/// PIT channel 2's counter, as programs read it: the count on port 42h
/// and the output on port 61h bit 5. Its gate is port 61h bit 0. Programs
/// time delay loops with it by raising the gate, running the loop, lowering
/// the gate and reading the count that stopped (MS-Kermit), so the gate has
/// to stop and restart the count as on the 8253. Its output, ANDed with
/// port 61h bit 1, drives the PC speaker (`high_ticks`).
#[derive(Clone, Debug)]
pub struct Pit2 {
    /// Counter mode, 0..=5 (6 and 7 alias 2 and 3).
    mode: u8,
    /// Count loaded at each reload, 1..=65536.
    reload: u32,
    /// Count written while a mode 2/3 period was running, and the tick it
    /// takes effect at: the end of the current half-cycle in mode 3, of the
    /// current period in mode 2, as the 8254 data sheet has it.
    pending: Option<(u32, u64)>,
    /// Port 61h bit 0.
    gate: bool,
    /// Tick the count last started from `from`.
    start: u64,
    /// Count at `start`: the reload value, or where a gate in mode 0 or 4
    /// stopped it.
    from: u32,
    /// The count and output while the counter doesn't run: after a control
    /// word until a count is written, with the gate low in modes 0, 2, 3
    /// and 4, and before the gate triggers modes 1 and 5.
    held: Option<(u16, bool)>,
    /// Access mode from the last control word: 1 = LSB only, 2 = MSB only,
    /// 3 = LSB then MSB.
    pub access: u8,
    /// Count frozen by a latch command, read until both bytes are read.
    pub latched: Option<u16>,
    /// A control word came and no count since: the next complete count
    /// loads at once, even into a mode 2 or 3 period that ran on through
    /// the control word. Saved after the PIT2 section's first fields.
    pub count_due: bool,
    /// How far into its period the count was at `start`: a mode 3 count
    /// loaded at the end of a high half-cycle starts with the low half.
    /// Saved in the 8254 section, as are the next two.
    pub(crate) phase: u64,
    /// Bits 5-0 of the last control word (access, mode as written and
    /// BCD), which the read-back command's status byte gives back.
    pub(crate) control: u8,
    /// Status byte latched by a read-back command, which the next read of
    /// port 42h returns before any latched count.
    pub status: Option<u8>,
}

impl Default for Pit2 {
    fn default() -> Self {
        Self::new()
    }
}

impl Pit2 {
    /// Power-on state: mode 3 with the gate low.
    pub fn new() -> Self {
        Self {
            mode: 3,
            reload: 0xFFFF,
            pending: None,
            gate: false,
            start: 0,
            from: 0xFFFF,
            held: Some((0xFFFF, true)),
            access: 3,
            latched: None,
            count_due: false,
            phase: 0,
            control: 0x36,
            status: None,
        }
    }

    fn periodic(&self) -> bool {
        matches!(self.mode, 2 | 3)
    }

    fn gated(&self) -> bool {
        matches!(self.mode, 0 | 2 | 3 | 4)
    }

    pub fn gate(&self) -> bool {
        self.gate
    }

    /// Counter mode, 0..=5.
    pub fn mode(&self) -> u8 {
        self.mode
    }

    /// A count written in mode 2 or 3 while a period ran, taking effect
    /// at the end of the half-cycle or period it was written in. A count
    /// that loads where a mode 3 output goes low starts with the low half
    /// of its own period. With the gate low the counter stops, and the
    /// count loads when the gate triggers it (`set_gate`).
    fn apply_pending(&mut self, now: u64) {
        if let Some((reload, at)) = self.pending
            && at <= now
            && self.held.is_none()
        {
            let current = self.reload as u64;
            let into = (at - self.start + self.phase) % current;
            let low_next = self.mode == 3 && into == current.div_ceil(2);
            self.pending = None;
            self.reload = reload;
            self.from = reload;
            self.start = at;
            self.phase = if low_next {
                (reload as u64).div_ceil(2)
            } else {
                0
            };
        }
    }

    /// Control word for channel 2: set the mode and stop the counter until
    /// a count is written, as the 8254 data sheet has it. `control` is the
    /// control word; its bits 5-0 are kept.
    ///
    /// A mode 2 or 3 count that runs when a control word for the same mode
    /// comes goes on with the count it had. A switch between modes 2 and 3
    /// stops it as any other control word does. Bao Xiao San Guo Zhi
    /// (SANGO.EXE) makes its random numbers with `out 43h,86h; out 43h,B6h`
    /// and two reads of port 42h, never writing a count; a stopped count
    /// gives it the same number every time (DOSBox SVN's "every roll is
    /// 5", DOSBox-X issue 1274).
    pub fn set_mode(&mut self, control: u8, now: u64) {
        self.apply_pending(now);
        let (count, _) = self.read(now);
        let mode = (control >> 1) & 0x07;
        let mode = if mode >= 6 { mode - 4 } else { mode };
        let runs_on = self.held.is_none() && self.periodic() && mode == self.mode;
        self.mode = mode;
        self.access = (control >> 4) & 0x03;
        self.control = control & 0x3F;
        self.pending = None;
        self.latched = None;
        self.status = None;
        self.count_due = true;
        if !runs_on {
            // Mode 0's output goes low at the control word, the others' high.
            self.held = Some((count, self.mode != 0));
        }
    }

    /// A complete count was written (0 means 65536).
    pub fn write_count(&mut self, count: u16, now: u64) {
        let reload = if count == 0 { 0x10000 } else { count as u32 };
        self.apply_pending(now);
        if self.held.is_none() && self.periodic() && !self.count_due {
            let current = self.reload as u64;
            let into = (now - self.start + self.phase) % current;
            let high = current.div_ceil(2);
            let left = if self.mode == 3 && into < high {
                high - into
            } else {
                current - into
            };
            self.pending = Some((reload, now + left));
            return;
        }
        self.count_due = false;
        self.reload = reload;
        self.from = reload;
        self.pending = None;
        self.phase = 0;
        if matches!(self.mode, 1 | 5) {
            // Loaded, but counting waits for the gate to trigger it.
            self.held = Some((reload as u16, true));
        } else if self.gate {
            self.start = now;
            self.held = None;
        } else {
            self.held = Some((reload as u16, self.mode != 0));
        }
    }

    /// Port 61h bit 0 changed to `gate`.
    pub fn set_gate(&mut self, gate: bool, now: u64) {
        if gate == self.gate {
            return;
        }
        self.apply_pending(now);
        self.gate = gate;
        if !gate {
            if self.gated() && self.held.is_none() {
                let (count, out) = self.read(now);
                // Modes 2 and 3 drive their output high while stopped.
                self.held = Some((count, out || self.periodic()));
            }
            return;
        }
        match self.mode {
            // A rising gate reloads the count and starts a new period,
            // with a count written since if there is one.
            1 | 2 | 3 | 5 => {
                if let Some((reload, _)) = self.pending.take() {
                    self.reload = reload;
                }
                self.from = self.reload;
                self.start = now;
                self.phase = 0;
                self.held = None;
            }
            // Modes 0 and 4 go on from where the gate stopped them.
            _ => {
                if let Some((count, _)) = self.held {
                    self.from = if count == 0 { 0x10000 } else { count as u32 };
                    self.start = now;
                    self.held = None;
                }
            }
        }
    }

    /// The count and output at `now`. A count that a mode 2 or 3 period
    /// had pending takes effect here without being stored: the state
    /// changes only at the writes to the channel and its gate, which the
    /// speaker's model (`high_ticks`) follows.
    pub fn read(&self, now: u64) -> (u16, bool) {
        if let Some((_, at)) = self.pending
            && at <= now
        {
            let mut applied = self.clone();
            applied.apply_pending(now);
            return applied.read_applied(now);
        }
        self.read_applied(now)
    }

    fn read_applied(&self, now: u64) -> (u16, bool) {
        if let Some(held) = self.held {
            return held;
        }
        let elapsed = now.saturating_sub(self.start);
        let reload = self.reload as u64;
        match self.mode {
            2 => {
                let into = (elapsed + self.phase) % reload;
                ((reload - into) as u16, into != reload - 1)
            }
            3 => {
                // The count runs down by two, twice a period, high for the
                // first half.
                let into = (elapsed + self.phase) % reload;
                (square_wave_count(reload, into), into < reload.div_ceil(2))
            }
            // One-shot modes count down through zero and wrap: 0 and 1
            // raise their output at the terminal count, 4 and 5 pulse it low
            // for a tick there.
            _ => {
                let count = (self.from as u64).wrapping_sub(elapsed) as u16;
                (count, self.output_at(elapsed))
            }
        }
    }

    /// The parts a state saved before the 8254 section lacks: no status
    /// latched, counts that start their periods from the top, and the
    /// control word's bits those the access and the mode give.
    pub(crate) fn without_8254_state(&mut self) {
        self.phase = 0;
        self.control = self.access << 4 | self.mode << 1;
        self.status = None;
    }

    /// The status byte the read-back command latches at `now`: the output,
    /// the null count flag (a count or control word written and not yet
    /// loaded into the counting element) and the control word's bits 5-0.
    pub fn status(&self, now: u64) -> u8 {
        let (_, out) = self.read(now);
        let pending = self
            .pending
            .is_some_and(|(_, at)| at > now || self.held.is_some());
        let untriggered = matches!(self.mode, 1 | 5) && self.held.is_some();
        let null = self.count_due || pending || untriggered;
        (out as u8) << 7 | (null as u8) << 6 | self.control
    }

    /// The output at `at` ticks into the count's run, counted from the
    /// start of its period in the periodic modes (`start` plus `phase`).
    fn output_at(&self, at: u64) -> bool {
        let reload = self.reload as u64;
        match self.mode {
            2 => at % reload != reload - 1,
            3 => at % reload < reload.div_ceil(2),
            0 | 1 => at >= self.from as u64,
            _ => at != self.from as u64,
        }
    }

    /// How many of the first `at` ticks of the count's run the output is
    /// high, counted as for `output_at`.
    fn high_count(&self, at: u64) -> u64 {
        let reload = self.reload as u64;
        let from = self.from as u64;
        match self.mode {
            // Low for the last tick of each period.
            2 => at - at / reload,
            // High for the first half of each period, the longer one of an
            // odd count.
            3 => {
                let high = reload.div_ceil(2);
                at / reload * high + (at % reload).min(high)
            }
            0 | 1 => at.saturating_sub(from),
            _ => at - (at > from) as u64,
        }
    }

    /// How long the output is high from tick `a` to tick `b`, which can
    /// fall between ticks: the output keeps each tick's level until the
    /// next. The speaker's level is this output (with port 61h bit 1), and
    /// its mean over a mix frame is what the speaker plays (see
    /// `Bus::audio_catch_up`). `a` is no earlier than the last change to
    /// the channel's mode, count or gate.
    pub fn high_ticks(&self, a: f64, b: f64) -> f64 {
        if b <= a {
            return 0.0;
        }
        if let Some((_, at)) = self.pending
            && self.held.is_none()
        {
            let at = at as f64;
            if a < at {
                let mut before = self.clone();
                before.pending = None;
                if b <= at {
                    return before.high_ticks(a, b);
                }
                return before.high_ticks(a, at) + self.applied_at(at).high_ticks(at, b);
            }
            return self.applied_at(a).high_ticks(a, b);
        }
        if let Some((_, out)) = self.held {
            return if out { b - a } else { 0.0 };
        }
        self.high_until(b) - self.high_until(a)
    }

    /// This counter with its pending count, which takes effect by `t`,
    /// loaded.
    fn applied_at(&self, t: f64) -> Self {
        let mut applied = self.clone();
        applied.apply_pending(t.ceil() as u64);
        applied
    }

    /// How long the output is high from `start` to tick `t`, while the
    /// count runs.
    fn high_until(&self, t: f64) -> f64 {
        let at = (t - self.start as f64).max(0.0) + self.phase as f64;
        let whole = at.floor();
        let ticks = whole as u64;
        let part = if self.output_at(ticks) {
            at - whole
        } else {
            0.0
        };
        (self.high_count(ticks) - self.high_count(self.phase)) as f64 + part
    }

    /// The output's level from `now` on while nothing is written to the
    /// channel or its gate, if it holds one: the counter is stopped, or a
    /// one-shot mode is past its terminal count.
    pub fn steady_output(&self, now: u64) -> Option<bool> {
        if let Some((_, out)) = self.held {
            return Some(out);
        }
        let elapsed = now.saturating_sub(self.start);
        let from = self.from as u64;
        match self.mode {
            0 | 1 if elapsed >= from => Some(true),
            4 | 5 if elapsed > from => Some(true),
            _ => None,
        }
    }
}

/// The byte interface of channel 1's port 41h: the access mode, the
/// LSB/MSB flip-flops and the latches. Channels 0 and 2 keep theirs on the
/// bus (`Bus::pit0_access`) and in `Pit2`.
#[derive(Clone, Debug)]
pub struct PitPort {
    /// Access mode from the last control word: 1 = LSB only, 2 = MSB only,
    /// 3 = LSB then MSB.
    pub access: u8,
    write_msb: bool,
    read_msb: bool,
    /// The count as assembled from the bytes written.
    divisor: u16,
    /// Count frozen by a latch command, read until both bytes are read.
    latched: Option<u16>,
    /// Status byte latched by a read-back command, read before the count.
    status: Option<u8>,
}

impl Default for PitPort {
    fn default() -> Self {
        Self::new(3)
    }
}

impl PitPort {
    pub fn new(access: u8) -> Self {
        Self {
            access,
            write_msb: false,
            read_msb: false,
            divisor: 0,
            latched: None,
            status: None,
        }
    }

    /// A control word with `access` (not a latch command): the byte
    /// sequencing starts over and the latches are dropped.
    pub fn control(&mut self, access: u8) {
        *self = Self::new(access);
    }

    /// A latch command, or a read-back command's count latch. A count
    /// latched and not yet read stays: the 8254 ignores the later ones.
    pub fn latch_count(&mut self, count: u16) {
        if self.latched.is_none() {
            self.latched = Some(count);
            self.read_msb = false;
        }
    }

    /// A read-back command's status latch, ignored while one is unread.
    pub fn latch_status(&mut self, status: u8) {
        self.status.get_or_insert(status);
    }

    /// A byte written to the port. Returns the count once it is complete.
    pub fn write(&mut self, value: u8) -> Option<u16> {
        match self.access {
            1 => self.divisor = value as u16,
            2 => self.divisor = (value as u16) << 8,
            _ if !self.write_msb => {
                self.divisor = (self.divisor & 0xFF00) | value as u16;
                self.write_msb = true;
                return None;
            }
            _ => {
                self.divisor = (self.divisor & 0x00FF) | (value as u16) << 8;
                self.write_msb = false;
            }
        }
        Some(self.divisor)
    }

    /// A byte read from the port, with `live` the count at the moment: a
    /// latched status first, then a latched count, else the live one.
    pub fn read(&mut self, live: u16) -> u8 {
        if let Some(status) = self.status.take() {
            return status;
        }
        let count = self.latched.unwrap_or(live);
        match self.access {
            1 => {
                self.latched = None;
                count as u8
            }
            2 => {
                self.latched = None;
                (count >> 8) as u8
            }
            _ if !self.read_msb => {
                self.read_msb = true;
                count as u8
            }
            _ => {
                self.read_msb = false;
                self.latched = None;
                (count >> 8) as u8
            }
        }
    }
}

crate::state_fields!(Pit0 { mode, reload, pending_reload, counting, stopped, armed, period_start, next_tc } skip { phase, pending_at, control });
crate::state_fields!(Pit2 { mode, reload, pending, gate, start, from, held, access, latched } skip { count_due, phase, control, status });
crate::state_fields!(PitPort {
    access,
    write_msb,
    read_msb,
    divisor,
    latched,
    status
});
crate::state_fields!(Clock { icount, deadline, stalled, idle, batch_end, cycles_per_ms, base_icount, base_ticks, base_ns } skip { tsc, offloaded, disk_wait });
crate::state_fields!(Tsc { base, icount, hz });

#[cfg(test)]
mod tests {
    use super::*;

    /// The host estimate `Auto` keeps to, after `frames` of `executed`
    /// instructions, `offloaded` counts of GDI's blits and `halted` counts
    /// the processor waited, each 8 ms of the host's time, run `times`.
    fn estimate_of(frames: &[(u64, u64, u64)], times: usize) -> f64 {
        let mut cpu = crate::cpu::Cpu::new(std::path::PathBuf::from("."));
        let mut pacer = Pacer::new(CpuSpeed::Auto(3000), None, Instant::now());
        for &(executed, offloaded, halted) in frames.iter().cycle().take(frames.len() * times) {
            let cycles = cpu.bus.clock.cycles_per_ms() as u64;
            cpu.bus
                .clock
                .stall_offloaded(offloaded * 1_000_000 / cycles);
            cpu.bus.clock.icount += executed + halted;
            cpu.bus.clock.idle += halted;
            let ms = Duration::from_millis;
            pacer.end_cpu_frame(&mut cpu, executed, ms(8), Duration::ZERO);
        }
        pacer.host.max.unwrap()
    }

    fn host_estimate(executed: u64, offloaded: u64, halted: u64) -> f64 {
        estimate_of(&[(executed, offloaded, halted)], 4)
    }

    /// A blit's emulated time is work the host did, in its own code, in
    /// the frame's time: a host that runs half the instructions and blits
    /// the rest in the same time keeps up with the same speed, where
    /// leaving the blits out would halve it.
    #[test]
    fn the_hosts_speed_counts_blits_as_work() {
        let all = host_estimate(200_000, 0, 0);
        let half = host_estimate(100_000, 0, 0);
        let blitting = host_estimate(100_000, 100_000, 0);
        assert!((half / all - 0.5).abs() < 0.01, "{half} against {all}");
        assert!(
            (blitting / all - 1.0).abs() < 0.01,
            "{blitting} against {all}"
        );
    }

    /// A program that waits between its frames blits as often at any
    /// speed: its blits count less the more it halts, and not at all from
    /// `BLITS_UNTIL_HALTED`, where it gets the estimate of its
    /// instructions.
    #[test]
    fn a_waiting_programs_blits_count_as_its_busy_share() {
        let all = host_estimate(200_000, 0, 0);
        let half = host_estimate(100_000, 0, 0);
        // A quarter of the counts halted: half the blits.
        let quarter_halted = host_estimate(100_000, 100_000, 200_000 / 3);
        let waiting = host_estimate(100_000, 100_000, 200_000);
        let expected = (100_000.0 + 100_000.0 * 0.5) / 200_000.0;
        assert!(
            (quarter_halted / all - expected).abs() < 0.02,
            "{quarter_halted} against {all}"
        );
        assert!(
            (waiting / half - 1.0).abs() < 0.01,
            "{waiting} against {half}"
        );
    }

    /// A movie's frame that decodes and blits, among frames that wait for
    /// the next: the share halted is the recent frames', so its blits
    /// don't count.
    #[test]
    fn a_movies_blitting_frame_counts_in_the_movies_busy_share() {
        let waiting = (20_000, 0, 1_000_000);
        let frames = |blits| {
            [
                waiting,
                waiting,
                waiting,
                waiting,
                waiting,
                (100_000, blits, 0),
            ]
        };
        let plain = estimate_of(&frames(0), 10);
        let movie = estimate_of(&frames(100_000), 10);
        // Counted in full for its own frame, the blits would add half.
        assert!(
            (movie / plain - 1.0).abs() < 0.01,
            "{movie} against {plain}"
        );
    }

    #[test]
    fn the_host_estimate_falls_fast_when_the_host_cannot_run_the_speed() {
        // Shannara: a loading loop takes the estimate and the speed to
        // 1,135,918, then the drawing that follows runs at 250,000.
        let mut host = HostEstimate {
            max: Some(1_135_918.0),
            samples: HOST_SEED,
            ..HostEstimate::default()
        };
        let mut frames = 0;
        while let Some(max) = host.max.filter(|&max| max > 250_000.0 * 1.25) {
            host.add(250_000.0, max as u32);
            frames += 1;
        }
        // A tenth of the way each frame took 26. The first sample below
        // the speed moves it a tenth, the ones after it half the way.
        assert_eq!(frames, 5);
    }

    #[test]
    fn the_host_estimate_drifts_where_the_host_keeps_up() {
        let estimate = |max: Option<f64>, short: bool, ideal: f64, current: u32| {
            let mut host = HostEstimate {
                max,
                short,
                samples: HOST_SEED,
                ..HostEstimate::default()
            };
            host.add(ideal, current);
            host.max.unwrap()
        };
        // A sample below the estimate at a speed the host still runs
        // moves it a tenth of the way, as one above it does.
        assert_eq!(
            estimate(Some(400_000.0), true, 300_000.0, 200_000),
            390_000.0
        );
        assert_eq!(
            estimate(Some(400_000.0), true, 500_000.0, 450_000),
            410_000.0
        );
        assert_eq!(estimate(None, false, 500_000.0, 3000), 500_000.0);
        // One sample below the speed it runs at, a stall of the host or a
        // long present: a tenth of the way.
        assert_eq!(
            estimate(Some(400_000.0), false, 200_000.0, 400_000),
            380_000.0
        );
        // The second in a row: half the way.
        assert_eq!(
            estimate(Some(400_000.0), true, 200_000.0, 400_000),
            300_000.0
        );
    }

    #[test]
    fn a_stall_moves_the_host_estimate_a_tenth() {
        // A program at 300,000 under an estimate of 400,000; one sample
        // of a frame that stalled says 50,000, and the next ones the
        // host's speed again.
        let mut host = HostEstimate::default();
        for _ in 0..HOST_SEED {
            host.add(400_000.0, 300_000);
        }
        host.add(50_000.0, 300_000);
        assert_eq!(host.max, Some(365_000.0));
        host.add(400_000.0, 300_000);
        assert!(!host.short);
        assert_eq!(host.max, Some(368_500.0));
    }

    /// The host estimate after frames of 200,000 instructions that take the
    /// host the milliseconds of `frames_ms` each, in turn.
    fn estimate_after(frames_ms: &[u64]) -> f64 {
        let mut cpu = crate::cpu::Cpu::new(std::path::PathBuf::from("."));
        let mut pacer = Pacer::new(CpuSpeed::Auto(3000), None, Instant::now());
        for &ms in frames_ms {
            cpu.bus.clock.icount += 200_000;
            let exec = Duration::from_millis(ms);
            pacer.end_cpu_frame(&mut cpu, 200_000, exec, Duration::ZERO);
        }
        pacer.host.max.unwrap()
    }

    #[test]
    fn a_stall_in_the_first_samples_does_not_set_the_host_estimate() {
        // Doom's first frames with its files not in the host's cache: the
        // first sample says a tenth of what the host runs.
        let mut host = HostEstimate::default();
        host.add(1_500.0, 3000);
        host.add(15_000.0, 3000);
        assert_eq!(host.max, Some(15_000.0));
        // From then on the estimate moves by shares.
        host.add(15_000.0, 3000);
        host.add(25_000.0, 3000);
        assert_eq!(host.max, Some(16_000.0));
    }

    #[test]
    fn the_frames_of_a_stall_stay_out_of_the_host_estimate() {
        let steady = estimate_after(&[10; 30]);
        // Frames of 200 and 120 ms, alone and a few in a row among frames
        // of 10, leave the estimate where it was.
        let mut frames = vec![10; 20];
        for _ in 0..4 {
            frames.extend([200, 10, 10, 120, 120, 120, 10]);
        }
        let stalled = estimate_after(&frames);
        assert!(
            (stalled / steady - 1.0).abs() < 0.01,
            "{stalled} against {steady}"
        );
        // More of them in a row than a stall takes are the host's speed
        // now.
        let mut frames = vec![10; 20];
        frames.extend([200; 14]);
        let slow = estimate_after(&frames);
        assert!(slow < steady / 4.0, "{slow} against {steady}");
    }

    #[test]
    fn a_heavy_frame_every_few_frames_stays_out_of_the_host_estimate() {
        // A program whose every Nth frame costs the host a little over six
        // times what the others do: each such frame is alone between frames
        // at the host's speed, so all of them stay out, and the estimate
        // reads what the light frames ran at. Averaged over all frames the
        // host runs 2.7 times fewer instructions a ms than that with every
        // third frame heavy, and 1.51 times with every tenth
        // (`HostEstimate::outlier` says what bounds the speed then).
        let steady = estimate_after(&[10; 30]);
        for (every, high) in [(3, 2.7), (10, 1.51)] {
            let mut frames = vec![10; 20];
            for i in 1..=60 {
                frames.push(if i % every == 0 { 61 } else { 10 });
            }
            let mixed = estimate_after(&frames);
            assert!(
                (mixed / steady - 1.0).abs() < 0.01,
                "{mixed} against {steady}"
            );
            let mean_ms = (61.0 + 10.0 * (every - 1) as f64) / every as f64;
            assert!((mean_ms / 10.0 - high).abs() < 0.01, "{mean_ms}");
        }
    }

    #[test]
    fn the_scatter_of_a_flat_out_program_keeps_the_host_estimate() {
        // A program that runs at the estimate, with samples a tenth above
        // and below the host's 400,000 in turn and in pairs: the estimate
        // stays within a fiftieth of it.
        for pattern in [[1.1, 0.9, 1.1, 0.9], [1.1, 1.1, 0.9, 0.9]] {
            let mut host = HostEstimate::default();
            host.add(400_000.0, 400_000);
            for _ in 0..100 {
                for share in pattern {
                    let speed = host.max.unwrap() as u32;
                    host.add(400_000.0 * share, speed);
                }
            }
            let max = host.max.unwrap();
            assert!((max / 400_000.0 - 1.0).abs() < 0.02, "{max}");
        }
    }

    /// The host's nanoseconds an instruction of a load takes: the host
    /// keeps up with 1,075,000 instructions a ms of it (`MAX_BUSY_SHARE`).
    const LOAD_NS: f64 = 0.85e6 / 1_075_000.0;

    /// One frame of a protected-mode program at its speed, whose
    /// instructions take the host `ns` each, reading its data if `reads`.
    /// Returns the speed it ran at and the host's time it took.
    fn boosted_frame(
        cpu: &mut crate::cpu::Cpu,
        pacer: &mut Pacer,
        ns: f64,
        reads: bool,
    ) -> (u32, Duration) {
        boosted_frame_with(cpu, pacer, ns, reads, Duration::ZERO)
    }

    /// `boosted_frame`, with `overhead` of the frame's other work (a
    /// present).
    fn boosted_frame_with(
        cpu: &mut crate::cpu::Cpu,
        pacer: &mut Pacer,
        ns: f64,
        reads: bool,
        overhead: Duration,
    ) -> (u32, Duration) {
        let speed = cpu.bus.clock.cycles_per_ms();
        let executed = (speed as f64 * FRAME.as_secs_f64() * 1000.0) as u64;
        let exec = Duration::from_nanos((executed as f64 * ns) as u64);
        cpu.bus.clock.icount += executed;
        if reads {
            cpu.bus.activity.io_bytes += 250_000;
        }
        if let Some(next) = pacer.end_cpu_frame(cpu, executed, exec, overhead) {
            cpu.bus.clock.set_cycles_per_ms(next);
        }
        (speed, exec)
    }

    /// A protected-mode program at `Auto` that loads without frames at
    /// 433,332, and then gets all the host has for it.
    fn boosted() -> (Box<crate::cpu::Cpu>, Pacer) {
        let mut cpu = crate::cpu::Cpu::new(std::path::PathBuf::from("."));
        cpu.pm_latched = true;
        let mut pacer = Pacer::new(CpuSpeed::Auto(3000), None, Instant::now());
        // The load at its speed before the boost, which seeds the estimate,
        // then the measurement that finds it at work without frames.
        cpu.bus.clock.set_cycles_per_ms(433_332);
        for _ in 0..6 {
            boosted_frame(&mut cpu, &mut pacer, LOAD_NS, true);
        }
        let loading = crate::autospeed::Measurement {
            fps: None,
            polling: 0.0,
            halted: 0.0,
            output: 14_800_000.0,
            io: 14_800_000.0,
            refresh: 60.0,
            pace: 1.0,
            missed: 0,
            late: 0.0,
            paced: 0,
            ahead: Default::default(),
        };
        let host_max = pacer.host.max.unwrap() as u32;
        let boost = pacer.auto.decide(433_332, &loading, true, host_max);
        assert!(pacer.auto.flat_out());
        assert!(boost > 1_000_000, "{boost}");
        cpu.bus.clock.set_cycles_per_ms(boost);
        for _ in 0..6 {
            boosted_frame(&mut cpu, &mut pacer, LOAD_NS, true);
        }
        (cpu, pacer)
    }

    /// The boosted load of `boosted`, then `then`: the host's time each
    /// frame of that takes, in nanoseconds an instruction, frame by frame.
    /// Returns the speed and the host's time of each of those frames.
    fn boosted_then(then: &[f64]) -> Vec<(u32, Duration)> {
        let (mut cpu, mut pacer) = boosted();
        then.iter()
            .map(|&ns| boosted_frame(&mut cpu, &mut pacer, ns, true))
            .collect()
    }

    /// Zanzarah's intro movie starts right after a load that `Auto` gave
    /// all the host has, at 1,074,449, and its frames cost the host about
    /// fifteen times as much an instruction as the load's (#1223). The
    /// speed follows the estimate down, and the frames left out of it as a
    /// stall's each run at a fifteenth of real time: eight of them, as
    /// many as a stall may take, were two seconds of a frozen movie. Given
    /// all the host has, the boost comes down from the third frame that
    /// slow in a row.
    #[test]
    fn a_boost_into_work_the_host_runs_far_slower_comes_down_within_a_few_frames() {
        let movie = LOAD_NS * 15.0;
        // What the host keeps up with of the movie.
        let keeps_up = 0.85e6 / movie;
        let frames = boosted_then(&[movie; 40]);
        // The host's time until the speed is within twice that.
        let slow: Duration = frames
            .iter()
            .take_while(|&&(speed, _)| speed as f64 > keeps_up * 2.0)
            .map(|&(_, exec)| exec)
            .sum();
        assert!(slow < Duration::from_millis(1000), "{slow:?}: {frames:?}");
        // Three of them at the boost, then a frame of each speed down.
        let at_boost = frames.iter().filter(|f| f.0 == frames[0].0).count();
        assert_eq!(at_boost, crate::autospeed::TRY_BEHIND_FRAMES as usize);
        // It comes down to what the host keeps up with, no lower than half.
        let last = frames.last().unwrap().0 as f64;
        assert!(last > keeps_up * 0.5 && last < keeps_up * 1.5, "{last}");
    }

    /// A stall of one or two frames in a row during a boosted load, as
    /// Grom's cold reads are, leaves the speed: the frames after it run
    /// at the load's cost again.
    #[test]
    fn a_short_stall_during_a_boost_leaves_the_speed() {
        let stall = LOAD_NS * 15.0;
        let frames = boosted_then(&[stall, LOAD_NS, LOAD_NS, stall, stall, LOAD_NS, LOAD_NS]);
        let speeds: Vec<u32> = frames.iter().map(|f| f.0).collect();
        // The estimate moves no more than its drift with the light frames.
        assert!(
            speeds.iter().all(|&s| s as f64 > speeds[0] as f64 * 0.9),
            "{speeds:?}"
        );
    }

    /// A stall of three frames or more in a row during a boosted load (a
    /// disc read, a shader compiled) brings the boost down while it lasts,
    /// and leaves the host's estimate as the stall rule keeps it: once the
    /// load ends, the program goes back to the speed it had before the
    /// boost, under the estimate it had before the stall, as it does
    /// without a stall.
    #[test]
    fn a_long_stall_during_a_boost_leaves_the_estimate() {
        let (mut cpu, mut pacer) = boosted();
        let boost = cpu.bus.clock.cycles_per_ms();
        let before = pacer.host.max.unwrap();
        let stall = LOAD_NS * 15.0;
        for ns in [stall; 4].into_iter().chain([LOAD_NS; 4]) {
            boosted_frame(&mut cpu, &mut pacer, ns, true);
        }
        assert!(pacer.auto.flat_out());
        let during = cpu.bus.clock.cycles_per_ms();
        assert!(during < boost / 2, "{during} of {boost}");
        let same = |pacer: &Pacer| (pacer.host.max.unwrap() - before).abs() < before * 1e-6;
        assert!(same(&pacer), "{:?} against {before}", pacer.host.max);
        // The load ends: no reads for longer than `WORK_DONE_NS`.
        for _ in 0..10 {
            boosted_frame(&mut cpu, &mut pacer, LOAD_NS, false);
        }
        assert!(!pacer.auto.flat_out());
        assert_eq!(cpu.bus.clock.cycles_per_ms(), 433_332);
        assert!(same(&pacer), "{:?} against {before}", pacer.host.max);
        assert_eq!(pacer.auto.boost_cap(), None);
    }

    /// The same stall during a load that goes on past the next
    /// measurement: that measurement decides from the estimate, as the
    /// boost before the stall did, and boosts the load to it again rather
    /// than to the speed the stall's frames left.
    #[test]
    fn a_long_stall_during_a_load_that_goes_on_ends_with_the_boost() {
        let (mut cpu, mut pacer) = boosted();
        let boost = cpu.bus.clock.cycles_per_ms();
        let stall = LOAD_NS * 15.0;
        let speeds: Vec<u32> = [stall; 4]
            .into_iter()
            .chain([LOAD_NS; 40])
            .map(|ns| boosted_frame(&mut cpu, &mut pacer, ns, true).0)
            .collect();
        assert!(speeds.iter().any(|&s| s < boost / 2), "{speeds:?}");
        // The measurement comes within 30 frames (`WINDOW_NS`).
        let last = *speeds.last().unwrap();
        assert!(last as f64 > boost as f64 * 0.9, "{boost}: {speeds:?}");
        assert!(pacer.auto.flat_out());
    }

    /// Once slow frames cap a boost, a frame now and then whose other work
    /// takes long (a loading screen's present) leaves the cap: one such
    /// frame says nothing of the program's work, as it says nothing to the
    /// estimate (`HostEstimate::add`).
    #[test]
    fn a_present_now_and_then_leaves_a_capped_boost() {
        let (mut cpu, mut pacer) = boosted();
        let stall = LOAD_NS * 15.0;
        for _ in 0..4 {
            boosted_frame(&mut cpu, &mut pacer, stall, true);
        }
        let capped = cpu.bus.clock.cycles_per_ms();
        let present = Duration::from_millis(13);
        let mut speeds = Vec::new();
        // Short of the next measurement (`WINDOW_NS`).
        for frame in 0..9 {
            let overhead = if frame % 3 == 2 {
                present
            } else {
                Duration::ZERO
            };
            let (speed, _) = boosted_frame_with(&mut cpu, &mut pacer, LOAD_NS, true, overhead);
            speeds.push(speed);
        }
        assert!(pacer.auto.flat_out());
        assert!(speeds.iter().all(|&s| s == capped), "{capped}: {speeds:?}");
    }

    #[test]
    fn a_coarse_clock_never_makes_the_host_infinitely_fast() {
        // In a browser, a short batch can measure as having taken no time:
        // the host's speed comes from enough frames to time.
        let mut sample = HostSample::default();
        let ms = Duration::from_millis;
        assert_eq!(sample.add(50_000, Duration::ZERO, Duration::ZERO), None);
        assert_eq!(sample.add(50_000, ms(1), Duration::ZERO), None);
        assert_eq!(sample.add(100, ms(5), Duration::ZERO), None);
        for _ in 0..7 {
            assert_eq!(sample.add(50_000, Duration::ZERO, ms(1)), None);
        }
        let (ns_per_instr, overhead_ns) = sample.add(50_000, ms(9), ms(1)).unwrap();
        assert_eq!(ns_per_instr, 10_000_000.0 / 500_000.0);
        assert_eq!(overhead_ns, 800_000.0);
        // And starts over.
        assert_eq!(sample.add(50_000, ms(1), Duration::ZERO), None);
    }

    #[test]
    fn the_time_stamp_counter_counts_instructions_at_a_fixed_speed() {
        let mut clock = Clock::new(1000);
        clock.icount = 5000;
        assert_eq!(clock.tsc(), 5000);
        // A speed set by hand changes the counter's rate with it.
        clock.set_cycles_per_ms(3000);
        clock.icount = 8000;
        assert_eq!(clock.tsc(), 8000);
    }

    #[test]
    fn the_time_stamp_counter_keeps_its_rate_while_the_speed_changes() {
        // 3DMark 2000 measures the counter's rate once and times its
        // frames with it: at max the rate must not follow the speed.
        let mut clock = Clock::new(10_000);
        clock.icount = 1000;
        clock.set_tsc_hz(Some(200_000_000));
        assert_eq!(clock.tsc(), 1000, "it goes on from where it was");
        // 1 ms at 10000 instructions a ms.
        clock.icount += 10_000;
        assert_eq!(clock.tsc(), 1000 + 200_000);
        // 1 ms at 50000 counts as much, and at 3000.
        clock.set_cycles_per_ms(50_000);
        clock.icount += 50_000;
        assert_eq!(clock.tsc(), 1000 + 400_000);
        clock.set_cycles_per_ms(3000);
        clock.icount += 1500;
        assert_eq!(clock.tsc(), 1000 + 500_000);
        // It never goes back, however the rate rounds.
        clock.set_cycles_per_ms(7);
        let mut last = clock.tsc();
        for _ in 0..20 {
            clock.icount += 1;
            assert!(clock.tsc() > last);
            last = clock.tsc();
            clock.set_cycles_per_ms(if clock.cycles_per_ms() == 7 { 13 } else { 7 });
        }
        // Back to counting instructions, from there on.
        clock.set_tsc_hz(None);
        clock.icount += 10;
        assert_eq!(clock.tsc(), last + 10);
    }

    #[test]
    fn fast_forward_runs_ahead_and_goes_on_from_there() {
        let start = Instant::now();
        let mut clock = Clock::new(1000);
        let mut pacer = Pacer::new(CpuSpeed::Fixed(1000), None, start);
        // Normally a frame of wall time is a frame of emulated time.
        let frame_ticks = duration_to_ticks(FRAME);
        let end = pacer.batch_end(&clock, start + FRAME);
        assert!(end.abs_diff(clock.icount_at(frame_ticks)) <= 1);

        // Fast forward: eight frames each time, whatever the wall clock.
        pacer.set_fast_forward(true, &clock, start);
        for _ in 0..10 {
            let end = pacer.batch_end(&clock, start + FRAME);
            let target = clock.icount_at(clock.now_ticks() + frame_ticks * FAST_FORWARD_FRAMES as u64);
            assert_eq!(end, target);
            clock.icount = end;
        }
        let ahead = clock.now_ticks();
        assert!(ahead >= frame_ticks * 80);

        // Released, emulated time goes on from where it got to: the next
        // frame runs a frame, not nothing until the wall clock catches up.
        let now = start + FRAME * 2;
        pacer.set_fast_forward(false, &clock, now);
        let end = pacer.batch_end(&clock, now + FRAME);
        let ran = clock.icount_at(ahead + frame_ticks).abs_diff(end);
        assert!(ran <= 1, "{} instructions off a frame", ran);
    }

    #[test]
    fn retrace_pacing_runs_each_batch_past_a_retrace_due_a_frame_apart() {
        let start = Instant::now();
        let mut clock = Clock::new(1000);
        let mut pacer = Pacer::new(CpuSpeed::Fixed(1000), None, start);
        let refresh = CrtTiming::VGA_400;
        let frame = refresh.frame_ns();
        let mut now = start;
        let mut last: Option<(Instant, u64)> = None;
        for _ in 0..20 {
            let end = pacer.retrace_batch_end(&clock, &refresh, now);
            clock.icount = end;
            let retraces = refresh.retraces(clock.now_ns());
            let (at, _) = pacer.deadline.expect("a deadline");
            if let Some((last_at, last_retraces)) = last {
                // One retrace per batch, each due a 70 Hz frame after the last.
                assert_eq!(retraces, last_retraces + 1);
                let apart = (at - last_at).as_nanos() as i64;
                assert!((apart - frame as i64).abs() < 2000, "{} ns apart", apart);
            }
            // The batch ends just past the picture's start, after the
            // retrace.
            assert!(clock.now_ns() % frame < 3000);
            assert_eq!(pacer.period, Duration::from_nanos(frame));
            last = Some((at, retraces));
            // The frame is shown at its deadline; the next starts then.
            now = at + Duration::from_micros(300);
        }
    }

    #[test]
    fn a_batch_never_ends_in_the_blanking_after_a_retrace() {
        let start = Instant::now();
        let mut clock = Clock::new(1000);
        let mut pacer = Pacer::new(CpuSpeed::Fixed(1000), None, start);
        let refresh = CrtTiming::VGA_400;
        let frame = refresh.frame_ns();
        let mut moved = 0;
        // 60 Hz host frames against a 70 Hz display, through every phase.
        for n in 1..2000u64 {
            let now = start + Duration::from_nanos(n * 16_666_667);
            let plain = pacer.batch_end(&clock, now);
            let end = clock.past_blanking(plain, &refresh);
            assert!(end >= plain);
            if end > plain {
                // Held: to the picture's first line, and no further.
                moved += 1;
                assert_eq!(clock.ns_at(end) % frame / 1000, 0);
            }
            clock.icount = end;
            assert_eq!(refresh.blanking_end(clock.now_ns()), None);
            // Emulated time stays within the blanking of the wall clock.
            let behind = (now - start).as_nanos() as i64 - clock.now_ns() as i64;
            assert!(behind.abs() < (frame / 8) as i64, "{} ns", behind);
        }
        assert!(moved > 0);
    }

    #[test]
    fn retrace_pacing_skips_retraces_a_slow_host_missed() {
        let start = Instant::now();
        let mut clock = Clock::new(1000);
        let mut pacer = Pacer::new(CpuSpeed::Fixed(1000), None, start);
        let refresh = CrtTiming::VGA_400;
        clock.icount = pacer.retrace_batch_end(&clock, &refresh, start);
        let first = refresh.retraces(clock.now_ns());
        let (at, _) = pacer.deadline.unwrap();
        // The host took two and a half frames more: the batch runs to the
        // retrace after the wall clock, not the next one.
        let late = at + Duration::from_nanos(refresh.frame_ns() * 5 / 2);
        clock.icount = pacer.retrace_batch_end(&clock, &refresh, late);
        assert_eq!(refresh.retraces(clock.now_ns()), first + 3);
        let (next, _) = pacer.deadline.unwrap();
        assert!(next >= late);
    }

    #[test]
    fn the_wait_for_a_deadline_is_reported() {
        // The frontend leaves it out of the frame's work.
        let start = Instant::now();
        let mut clock = Clock::new(1000);
        let mut pacer = Pacer::new(CpuSpeed::Fixed(1000), None, start);
        clock.icount = pacer.retrace_batch_end(&clock, &CrtTiming::VGA_400, start);
        let (at, _) = pacer.deadline.unwrap();
        let waited = pacer.wait_to_present(&clock);
        assert!(pacer.presented && Instant::now() >= at);
        // About a 70 Hz frame.
        assert!(waited > Duration::from_millis(5), "{:?}", waited);
    }

    #[test]
    fn retrace_pacing_waits_only_for_a_batch_that_got_there() {
        let start = Instant::now();
        let clock = Clock::new(1000);
        let mut pacer = Pacer::new(CpuSpeed::Fixed(1000), None, start);
        let refresh = CrtTiming::VGA_400;
        // Stopped short (a breakpoint): no waiting, and the next frame
        // comes at the usual pace.
        pacer.retrace_batch_end(&clock, &refresh, start);
        assert!(pacer.wait_to_present(&clock) < Duration::from_millis(5));
        assert!(!pacer.presented);
        // Fast forwarding: no deadline at all.
        pacer.set_fast_forward(true, &clock, start);
        pacer.retrace_batch_end(&clock, &refresh, start);
        assert!(pacer.deadline.is_none());
        assert_eq!(pacer.period, FRAME);
    }

    #[test]
    fn the_speed_steps_by_a_tenth() {
        assert_eq!(CpuSpeed::Fixed(20_000).stepped(20_000, true), CpuSpeed::Fixed(22_000));
        assert_eq!(CpuSpeed::Fixed(20_000).stepped(20_000, false), CpuSpeed::Fixed(18_000));
        // Slower from max starts where max got to; faster stays max.
        assert_eq!(CpuSpeed::Max.stepped(123_456, false), CpuSpeed::Fixed(111_200));
        assert_eq!(CpuSpeed::Max.stepped(50_000, true), CpuSpeed::Max);
        // Auto goes either way from where it got to.
        assert_eq!(CpuSpeed::Auto(3000).stepped(40_000, true), CpuSpeed::Fixed(44_000));
        assert_eq!(CpuSpeed::Auto(3000).stepped(40_000, false), CpuSpeed::Fixed(36_000));
        // No slower than the slowest.
        assert_eq!(CpuSpeed::Fixed(150).stepped(150, false), CpuSpeed::Fixed(MIN_CYCLES));
        assert_eq!(CpuSpeed::Fixed(MIN_CYCLES).stepped(MIN_CYCLES, false), CpuSpeed::Fixed(MIN_CYCLES));
    }

    #[test]
    fn channel_2_counts_only_while_its_gate_is_up() {
        // MS-Kermit's calibration: mode 2, count 0, gate up for a delay
        // loop, gate down, then read the count without a latch.
        let mut pit = Pit2::new();
        pit.set_mode(0x34, 1000);
        pit.write_count(0, 1010);
        assert_eq!(pit.read(5000).0, 0);
        pit.set_gate(true, 20_000);
        pit.set_gate(false, 29_544);
        assert_eq!(pit.read(90_000).0.wrapping_neg(), 9544);
        // Raising the gate again starts a new period from the top.
        pit.set_gate(true, 100_000);
        assert_eq!(pit.read(100_010).0, 0xFFF6);
    }

    #[test]
    fn channel_2_mode_0_resumes_where_the_gate_stopped_it() {
        let mut pit = Pit2::new();
        pit.set_gate(true, 0);
        pit.set_mode(0x30, 0);
        pit.write_count(100, 10);
        assert_eq!(pit.read(50), (60, false));
        pit.set_gate(false, 50);
        assert_eq!(pit.read(500), (60, false));
        pit.set_gate(true, 1000);
        assert_eq!(pit.read(1059), (1, false));
        assert_eq!(pit.read(1060), (0, true));
    }

    /// How long the output is high over a span agrees with the output
    /// read tick by tick, in every mode, across a count that a mode 2 or
    /// 3 period had pending.
    #[test]
    fn the_time_high_matches_the_output() {
        for mode in 0..6u8 {
            for count in [2u16, 3, 7, 10] {
                let mut pit = Pit2::new();
                pit.set_mode(0x30 | mode << 1, 0);
                pit.set_gate(true, 0);
                pit.write_count(count, 5);
                if matches!(mode, 1 | 5) {
                    pit.set_gate(false, 6);
                    pit.set_gate(true, 7);
                }
                // A new count while the period runs.
                pit.write_count(count + 2, 13);
                let mut high = 0.0;
                for tick in 13..80u64 {
                    let at = pit.high_ticks(13.0, tick as f64);
                    assert_eq!(at, high, "mode {mode}, count {count}, to {tick}");
                    high += pit.read(tick).1 as u8 as f64;
                }
                // A span between ticks takes in the parts of the ticks at
                // its ends.
                let part = pit.high_ticks(20.25, 40.5);
                let whole = pit.high_ticks(21.0, 40.0);
                let ends = 0.75 * pit.read(20).1 as u8 as f64 + 0.5 * pit.read(40).1 as u8 as f64;
                assert!((part - whole - ends).abs() < 1e-9, "mode {mode}, count {count}");
            }
        }
    }

    /// A square wave's time high over whole periods: half of an even
    /// count, the longer half of an odd one.
    #[test]
    fn a_square_wave_is_high_for_its_longer_half() {
        let mut pit = Pit2::new();
        pit.set_mode(0x36, 0);
        pit.set_gate(true, 0);
        pit.write_count(2, 0);
        assert_eq!(pit.high_ticks(0.0, 1000.0), 500.0);
        pit.set_mode(0x36, 1000);
        pit.write_count(3, 1000);
        assert_eq!(pit.high_ticks(1000.0, 1300.0), 200.0);
    }
}

#[cfg(test)]
mod mul_div_tests {
    use super::{mul_div, mul_div_ceil};

    #[test]
    fn mul_div_is_the_128_bit_quotient() {
        let values = [
            0u64,
            1,
            999,
            1_000_000,
            1_193_182,
            (u64::MAX / 1_000_000) - 1,
            u64::MAX / 1_000_000,
            u64::MAX / 1_000_000 + 1,
            u64::MAX / 3,
            u64::MAX - 1,
            u64::MAX,
        ];
        for &a in &values {
            for &b in &[1u64, 1000, 1_000_000, 1_193_182, 2_000_000_000, u64::MAX] {
                for &c in &[1u64, 7, 1000, 1_193_182, 3_000_000, u64::MAX] {
                    let wide = a as u128 * b as u128;
                    assert_eq!(mul_div(a, b, c), (wide / c as u128) as u64, "{a} * {b} / {c}");
                    let up = wide.div_ceil(c as u128).min(u64::MAX as u128) as u64;
                    assert_eq!(mul_div_ceil(a, b, c), up, "{a} * {b} / {c} up");
                }
            }
        }
    }
}
