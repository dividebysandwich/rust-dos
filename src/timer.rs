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

use crate::autospeed::AutoSpeed;
use crate::bus::Bus;
use crate::video::crt::CrtTiming;
use std::time::Duration;
use web_time::Instant;

pub const PIT_HZ: u64 = 1_193_182;

/// Emulated milliseconds per host video frame.
const FRAME: Duration = Duration::from_micros(16_667);
/// How far emulated time may fall behind the wall clock before the backlog
/// is dropped rather than caught up.
const MAX_LAG: Duration = Duration::from_millis(50);

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

/// Emulated time an I/O port access takes on the ISA bus, as in DOSBox.
/// Delay loops made of port reads (AdLib detection polls the status port
/// 200 times to wait out an 80 us timer) rely on it.
pub const IO_READ_NS: u64 = 1000;
pub const IO_WRITE_NS: u64 = 750;

fn duration_to_ticks(d: Duration) -> u64 {
    (d.as_nanos() * PIT_HZ as u128 / 1_000_000_000) as u64
}

/// `a * b / c`, as with 128-bit numbers (the quotient cut to 64 bits), but
/// in 64 where the product fits: a 128-bit division is a call of a
/// library function on 64-bit hosts, and the clock works these out at
/// every port access.
#[inline]
pub fn mul_div(a: u64, b: u64, c: u64) -> u64 {
    match a.checked_mul(b) {
        Some(p) => p / c,
        None => (a as u128 * b as u128 / c as u128) as u64,
    }
}

/// `a * b / c` rounded up, as `mul_div`, saturating at `u64::MAX`.
#[inline]
pub fn mul_div_ceil(a: u64, b: u64, c: u64) -> u64 {
    match a.checked_mul(b) {
        Some(p) => p.div_ceil(c),
        None => (a as u128 * b as u128).div_ceil(c as u128).min(u64::MAX as u128) as u64,
    }
}

fn ticks_to_duration(ticks: u64) -> Duration {
    Duration::from_nanos((ticks as u128 * 1_000_000_000 / PIT_HZ as u128) as u64)
}

/// How far past the start of a retrace a batch that runs to it ends, so
/// that the display sees it began whatever the rounding between
/// instructions, ticks and nanoseconds.
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

    fn instructions_per_second(&self) -> u64 {
        self.cycles_per_ms as u64 * 1000
    }

    /// Current emulated time in PIT ticks.
    pub fn now_ticks(&self) -> u64 {
        let delta = self.icount.saturating_sub(self.base_icount);
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

    /// Current emulated time in microseconds.
    pub fn now_micros(&self) -> u64 {
        mul_div(self.now_ticks(), 1_000_000, PIT_HZ)
    }

    /// The first instruction count at which `now_ticks() >= ticks`.
    pub fn icount_at(&self, ticks: u64) -> u64 {
        let delta = ticks.saturating_sub(self.base_ticks);
        self.base_icount.saturating_add(mul_div_ceil(delta, self.instructions_per_second(), PIT_HZ))
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
        let n = self.stall_count(ns);
        self.icount += n;
        self.stalled += n;
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
    /// Where a control word stopped the count: reads return it until a
    /// count is written.
    stopped: u16,
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
            stopped: 0,
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

    /// Control word for channel 0 at `now`: set the mode and stop counting
    /// until a count is written. The count stays where it got to (Future
    /// Crew's demos time a frame by stopping the counter so and reading it).
    pub fn set_mode(&mut self, mode: u8, now: u64) {
        self.stopped = self.count(now);
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
    /// the reload value; periodic modes restart at each terminal count (the
    /// square wave of mode 3 counts down by two, twice a period, once for
    /// each half of the wave), the one-shot modes wrap to FFFFh and go on.
    pub fn count(&self, now: u64) -> u16 {
        if !self.counting {
            return self.stopped;
        }
        let elapsed = now.saturating_sub(self.period_start);
        if self.periodic() {
            let reload = self.reload as u64;
            let into = elapsed % reload;
            let down = if self.mode == 3 { 2 * into % reload } else { into };
            (reload - down) as u16
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

/// Keeps emulated time in step with the wall clock, one video frame at a
/// time, and tunes the speed in `CpuSpeed::Max` and `Auto` modes.
pub struct Pacer {
    speed: CpuSpeed,
    /// Finds `Auto`'s speed, and whether the program it measures switched
    /// to protected mode.
    auto: AutoSpeed,
    protected: bool,
    /// As fast as the host keeps up with, averaged over frames.
    host_max: Option<f64>,
    /// The frames measured for the host's speed so far: the instructions
    /// they ran, the time those took and the rest of their work.
    sample: HostSample,
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
    pub fn new(speed: CpuSpeed, now: Instant) -> Self {
        Self {
            speed,
            auto: AutoSpeed::new(speed.initial_cycles()),
            protected: false,
            host_max: None,
            sample: HostSample::default(),
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
    /// up with it.
    pub fn set_fast_forward(&mut self, on: bool, clock: &Clock, now: Instant) {
        if self.fast_forward && !on {
            self.rebase(clock, now);
        }
        self.fast_forward = on;
    }

    /// Go on from the clock's time as it is now, after it jumped (a save
    /// state loaded, fast forward let go): without this, emulated time
    /// ahead of the wall clock would stand still until the wall clock
    /// caught up, and time behind it would race to catch up.
    pub fn rebase(&mut self, clock: &Clock, now: Instant) {
        self.auto.reset();
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
            self.auto = AutoSpeed::new(base);
        }
        self.speed = speed;
    }

    /// The instruction count the next batch should run to, so emulated time
    /// catches up with the wall clock at `now`. If emulation fell too far
    /// behind (slow host, debugger pause), the backlog is dropped and only
    /// one frame's worth is scheduled.
    pub fn batch_end(&mut self, clock: &Clock, now: Instant) -> u64 {
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
            self.anchor_ticks
        } else {
            wall.max(emulated)
        };
        clock.icount_at(target)
    }

    /// Like `batch_end`, but on to the start of the display's next vertical
    /// retrace (`refresh`'s) after the wall clock, noting when that is due
    /// on the wall clock, which `wait_to_present` waits for: a frame shown
    /// for each retrace when it begins, a display with a variable refresh
    /// rate refreshes at the emulated one, 70 Hz or whatever the CRTC's
    /// registers make it, rather than the host's frames'. A host that
    /// falls behind skips retraces rather than slowing the machine down.
    pub fn retrace_batch_end(&mut self, clock: &Clock, refresh: &CrtTiming, now: Instant) -> u64 {
        if self.fast_forward {
            return self.batch_end(clock, now);
        }
        let emulated = clock.now_ticks();
        let mut wall =
            self.anchor_ticks + duration_to_ticks(now.saturating_duration_since(self.anchor_wall));
        if wall > emulated + duration_to_ticks(MAX_LAG) {
            self.anchor_wall = now;
            self.anchor_ticks = emulated;
            wall = emulated;
        }
        // The retrace in the display's nanoseconds, which count from where
        // the clock's ticks do.
        let from_ns = clock.now_ns() + ticks_to_duration(wall.saturating_sub(emulated)).as_nanos() as u64;
        let retrace_ns = refresh.next_retrace(from_ns) + RETRACE_MARGIN_NS;
        let ahead = (((retrace_ns - clock.now_ns()) as u128 * PIT_HZ as u128).div_ceil(1_000_000_000)) as u64;
        let target = emulated + ahead;
        self.deadline = Some((self.anchor_wall + ticks_to_duration(target - self.anchor_ticks), target));
        self.period = Duration::from_nanos(refresh.frame_ns());
        clock.icount_at(target)
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
    /// program needs (`AutoSpeed`), no more than that; `protected` says
    /// whether it switched to protected mode (`Cpu::pm_latched`), and a
    /// program starting or ending starts the search over. `executed`
    /// counts only instructions that actually ran, not ones skipped while
    /// waiting for an interrupt.
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
        let ideal = self.sample.add(executed, exec, overhead).map(|(ns_per_instr, overhead_ns)| {
            let frame_ns = self.period.as_nanos() as f64;
            let budget_ns = (frame_ns * MAX_BUSY_SHARE - overhead_ns).max(frame_ns * 0.1);
            budget_ns / ns_per_instr / (frame_ns / 1_000_000.0)
        });
        match self.speed {
            CpuSpeed::Fixed(_) => None,
            CpuSpeed::Max => {
                // Move gradually so one unusual frame can't swing the speed.
                let next = ideal?.clamp(current as f64 * 0.9, current as f64 * 1.1);
                let next = (next as u32).clamp(MIN_CYCLES, MAX_CYCLES);
                (next != current).then_some(next)
            }
            CpuSpeed::Auto(_) => {
                if let Some(ideal) = ideal {
                    self.host_max = Some(self.host_max.map_or(ideal, |max| max * 0.9 + ideal * 0.1));
                }
                let host_max = self.host_max.map_or(MAX_CYCLES, |max| (max as u32).clamp(MIN_CYCLES, MAX_CYCLES));
                if protected != self.protected {
                    self.protected = protected;
                    self.auto.reset();
                    // The protected-mode program ended: whatever runs next
                    // starts from the real-mode speed.
                    if !protected {
                        let base = self.auto.base();
                        return (base != current).then_some(base);
                    }
                }
                // Fast forwarding, the host's time says nothing of the
                // program's needs.
                let next = if self.fast_forward { None } else { self.auto.update(bus, protected, host_max) };
                // A heavier scene than the host keeps up with at this speed.
                match next.unwrap_or(current) {
                    speed if speed > host_max && current > host_max => Some(host_max),
                    speed => (speed != current).then_some(speed),
                }
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
        if executed < 10_000 {
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

crate::state_fields!(Pit0 { mode, reload, pending_reload, counting, stopped, armed, period_start, next_tc });
crate::state_fields!(Clock { icount, deadline, stalled, idle, batch_end, cycles_per_ms, base_icount, base_ticks, base_ns });

#[cfg(test)]
mod tests {
    use super::*;

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
    fn fast_forward_runs_ahead_and_goes_on_from_there() {
        let start = Instant::now();
        let mut clock = Clock::new(1000);
        let mut pacer = Pacer::new(CpuSpeed::Fixed(1000), start);
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
    fn retrace_pacing_runs_each_batch_to_a_retrace_due_a_frame_apart() {
        let start = Instant::now();
        let mut clock = Clock::new(1000);
        let mut pacer = Pacer::new(CpuSpeed::Fixed(1000), start);
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
            // The batch ends just past the retrace's start.
            assert!(clock.now_ns() - (refresh.next_retrace(clock.now_ns()) - frame) < 3000);
            assert_eq!(pacer.period, Duration::from_nanos(frame));
            last = Some((at, retraces));
            // The frame is shown at its deadline; the next starts then.
            now = at + Duration::from_micros(300);
        }
    }

    #[test]
    fn retrace_pacing_skips_retraces_a_slow_host_missed() {
        let start = Instant::now();
        let mut clock = Clock::new(1000);
        let mut pacer = Pacer::new(CpuSpeed::Fixed(1000), start);
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
        let mut pacer = Pacer::new(CpuSpeed::Fixed(1000), start);
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
        let mut pacer = Pacer::new(CpuSpeed::Fixed(1000), start);
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
