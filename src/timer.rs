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

use std::time::{Duration, Instant};

pub const PIT_HZ: u64 = 1_193_182;

/// Emulated milliseconds per host video frame.
const FRAME: Duration = Duration::from_micros(16_667);
/// How far emulated time may fall behind the wall clock before the backlog
/// is dropped rather than caught up.
const MAX_LAG: Duration = Duration::from_millis(50);

/// Speed range accepted for `cycles`, in instructions per emulated ms.
pub const MIN_CYCLES: u32 = 100;
pub const MAX_CYCLES: u32 = 2_000_000;
/// Starting point for `CpuSpeed::Max` before the first adjustment.
const MAX_INITIAL_CYCLES: u32 = 20_000;
/// Share of each frame that `CpuSpeed::Max` gives to instruction execution.
/// The rest is left for rendering, audio and the host.
const MAX_BUSY_SHARE: f64 = 0.85;

/// Emulated time an I/O port access takes on the ISA bus, as in DOSBox.
/// Delay loops made of port reads (AdLib detection polls the status port
/// 200 times to wait out an 80 us timer) rely on it.
pub const IO_READ_NS: u64 = 1000;
pub const IO_WRITE_NS: u64 = 750;

fn duration_to_ticks(d: Duration) -> u64 {
    (d.as_nanos() * PIT_HZ as u128 / 1_000_000_000) as u64
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
    batch_end: u64,
    cycles_per_ms: u32,
    base_icount: u64,
    base_ticks: u64,
    /// `base_ticks` in nanoseconds, for events finer than a PIT tick
    /// (838 ns), such as the scanlines of the display.
    base_ns: u64,
}

impl Clock {
    pub fn new(cycles_per_ms: u32) -> Self {
        Self {
            icount: 0,
            deadline: u64::MAX,
            stalled: 0,
            idle: 0,
            batch_end: u64::MAX,
            cycles_per_ms: cycles_per_ms.clamp(MIN_CYCLES, MAX_CYCLES),
            base_icount: 0,
            base_ticks: 0,
            base_ns: 0,
        }
    }

    pub fn cycles_per_ms(&self) -> u32 {
        self.cycles_per_ms
    }

    fn instructions_per_second(&self) -> u128 {
        self.cycles_per_ms as u128 * 1000
    }

    /// Current emulated time in PIT ticks.
    pub fn now_ticks(&self) -> u64 {
        let delta = self.icount.saturating_sub(self.base_icount) as u128;
        self.base_ticks + (delta * PIT_HZ as u128 / self.instructions_per_second()) as u64
    }

    /// Current emulated time in nanoseconds.
    pub fn now_ns(&self) -> u64 {
        let delta = self.icount.saturating_sub(self.base_icount) as u128;
        self.base_ns + (delta * 1_000_000 / self.cycles_per_ms as u128) as u64
    }

    /// Current emulated time in microseconds.
    pub fn now_micros(&self) -> u64 {
        (self.now_ticks() as u128 * 1_000_000 / PIT_HZ as u128) as u64
    }

    /// The first instruction count at which `now_ticks() >= ticks`.
    pub fn icount_at(&self, ticks: u64) -> u64 {
        let delta = ticks.saturating_sub(self.base_ticks) as u128;
        let n = (delta * self.instructions_per_second()).div_ceil(PIT_HZ as u128);
        self.base_icount
            .saturating_add(n.min(u64::MAX as u128) as u64)
    }

    /// Change the speed from the current instruction onwards. Callers on the
    /// bus must recompute the deadline afterwards (`Bus::set_cycles_per_ms`).
    pub fn set_cycles_per_ms(&mut self, cycles_per_ms: u32) {
        self.base_ticks = self.now_ticks();
        self.base_ns = self.now_ns();
        self.base_icount = self.icount;
        self.cycles_per_ms = cycles_per_ms.clamp(MIN_CYCLES, MAX_CYCLES);
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
        let n = self.cycles_per_ms as u64 * ns / 1_000_000;
        self.icount += n;
        self.stalled += n;
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

/// 8253 PIT channel 0, which drives IRQ 0. Only the timing is modelled;
/// port-level byte sequencing (LSB/MSB toggles, latches) lives on the bus.
pub struct Pit0 {
    /// Counter mode, 0..=5 (6 and 7 alias 2 and 3).
    mode: u8,
    /// Count loaded at each reload, 1..=65536.
    reload: u32,
    /// Count written while a mode 2/3 period was running. It takes effect at
    /// the end of that period, as on real hardware.
    pending_reload: Option<u32>,
    /// False after a control word until the first count is written.
    counting: bool,
    /// Whether the counter will raise IRQ 0 at `next_tc`. One-shot modes
    /// disarm after firing until a new count is written.
    armed: bool,
    period_start: u64,
    next_tc: u64,
}

impl Default for Pit0 {
    fn default() -> Self {
        Self::new()
    }
}

impl Pit0 {
    /// Power-on state as left by the BIOS: mode 3, count 65536 (18.2 Hz).
    pub fn new() -> Self {
        Self {
            mode: 3,
            reload: 0x10000,
            pending_reload: None,
            counting: true,
            armed: true,
            period_start: 0,
            next_tc: 0x10000,
        }
    }

    fn periodic(&self) -> bool {
        matches!(self.mode, 2 | 3)
    }

    /// Current reload value, 1..=65536.
    pub fn reload(&self) -> u32 {
        self.reload
    }

    /// False between a control word and the first count written after it.
    pub fn is_counting(&self) -> bool {
        self.counting
    }

    /// Time of the next IRQ 0 edge, if one is coming.
    pub fn next_event(&self) -> Option<u64> {
        (self.counting && self.armed).then_some(self.next_tc)
    }

    /// Control word for channel 0: set the mode and stop counting until a
    /// count is written.
    pub fn set_mode(&mut self, mode: u8) {
        self.mode = if mode >= 6 { mode - 4 } else { mode };
        self.counting = false;
        self.armed = false;
        self.pending_reload = None;
    }

    /// A complete count was written (0 means 65536).
    pub fn write_count(&mut self, count: u16, now: u64) {
        let reload = if count == 0 { 0x10000 } else { count as u32 };
        if self.counting && self.periodic() {
            self.pending_reload = Some(reload);
            return;
        }
        self.reload = reload;
        self.pending_reload = None;
        self.counting = true;
        self.armed = true;
        self.period_start = now;
        self.next_tc = now + reload as u64;
    }

    /// Advance to `now`. Returns true if at least one IRQ 0 edge happened.
    pub fn advance(&mut self, now: u64) -> bool {
        if !(self.counting && self.armed) || self.next_tc > now {
            return false;
        }
        if !self.periodic() {
            // One-shot: the output stays high until the next count write,
            // while the count keeps running down through zero.
            self.armed = false;
            return true;
        }
        self.period_start = self.next_tc;
        if let Some(reload) = self.pending_reload.take() {
            self.reload = reload;
        }
        // Skip whole periods arithmetically; several may have passed.
        let reload = self.reload as u64;
        let behind = now - self.period_start;
        self.period_start += behind / reload * reload;
        self.next_tc = self.period_start + reload;
        true
    }

    /// The value a counter read returns at `now`. The count runs down from
    /// the reload value; periodic modes restart at each terminal count, the
    /// one-shot modes wrap to FFFFh and go on.
    pub fn count(&self, now: u64) -> u16 {
        if !self.counting {
            return self.reload as u16;
        }
        let elapsed = now.saturating_sub(self.period_start);
        if self.periodic() {
            let reload = self.reload as u64;
            (reload - elapsed % reload) as u16
        } else {
            // One-shot counters keep counting down through zero.
            (self.reload as u64).wrapping_sub(elapsed) as u16
        }
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
}

impl CpuSpeed {
    /// Parse `max` or an instruction count per millisecond.
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        if s.eq_ignore_ascii_case("max") {
            return Ok(CpuSpeed::Max);
        }
        match s.parse::<u32>() {
            Ok(n) if (MIN_CYCLES..=MAX_CYCLES).contains(&n) => Ok(CpuSpeed::Fixed(n)),
            _ => Err(format!(
                "invalid cycles '{}': expected max or {}-{}",
                s, MIN_CYCLES, MAX_CYCLES
            )),
        }
    }

    pub fn initial_cycles(self) -> u32 {
        match self {
            CpuSpeed::Max => MAX_INITIAL_CYCLES,
            CpuSpeed::Fixed(n) => n,
        }
    }
}

/// Keeps emulated time in step with the wall clock, one video frame at a
/// time, and tunes the speed in `CpuSpeed::Max` mode.
pub struct Pacer {
    speed: CpuSpeed,
    /// Wall-clock instant that corresponds to emulated time `anchor_ticks`.
    anchor_wall: Instant,
    anchor_ticks: u64,
    next_frame: Instant,
}

impl Pacer {
    pub fn new(speed: CpuSpeed, now: Instant) -> Self {
        Self {
            speed,
            anchor_wall: now,
            anchor_ticks: 0,
            next_frame: now,
        }
    }

    /// The instruction count the next batch should run to, so emulated time
    /// catches up with the wall clock at `now`. If emulation fell too far
    /// behind (slow host, debugger pause), the backlog is dropped and only
    /// one frame's worth is scheduled.
    pub fn batch_end(&mut self, clock: &Clock, now: Instant) -> u64 {
        let emulated = clock.now_ticks();
        let wall =
            self.anchor_ticks + duration_to_ticks(now.saturating_duration_since(self.anchor_wall));
        let target = if wall > emulated + duration_to_ticks(MAX_LAG) {
            self.anchor_wall = now;
            self.anchor_ticks = emulated + duration_to_ticks(FRAME);
            self.anchor_ticks
        } else {
            wall.max(emulated)
        };
        clock.icount_at(target)
    }

    /// Frame bookkeeping after a batch. In `Max` mode, retune the speed so
    /// that executing one frame's worth of instructions plus the rest of the
    /// frame's work (`overhead`) fits into `MAX_BUSY_SHARE` of a frame.
    /// `executed` counts only instructions that actually ran, not ones
    /// skipped while waiting for an interrupt.
    /// Returns the new speed if it changed.
    pub fn end_frame(
        &mut self,
        clock: &Clock,
        executed: u64,
        exec: Duration,
        overhead: Duration,
    ) -> Option<u32> {
        if self.speed != CpuSpeed::Max || executed < 10_000 {
            return None;
        }
        let ns_per_instr = exec.as_nanos() as f64 / executed as f64;
        let frame_ns = FRAME.as_nanos() as f64;
        let budget_ns =
            (frame_ns * MAX_BUSY_SHARE - overhead.as_nanos() as f64).max(frame_ns * 0.1);
        let frame_ms = frame_ns / 1_000_000.0;
        let ideal = budget_ns / ns_per_instr / frame_ms;
        // Move gradually so one unusual frame can't swing the speed.
        let current = clock.cycles_per_ms() as f64;
        let next = ideal.clamp(current * 0.9, current * 1.1);
        let next = (next as u32).clamp(MIN_CYCLES, MAX_CYCLES);
        (next != clock.cycles_per_ms()).then_some(next)
    }

    /// Sleep until the next video frame is due.
    pub fn wait_for_next_frame(&mut self) {
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
