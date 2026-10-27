//! The API trace: the line the log gets for a call of a built-in
//! function, `[W32] caller: DLL.Function(arguments) = result` from
//! `stubs::call` and its `[W16]` twin from the Win16 modules.
//!
//! The `win32_trace` setting picks which calls get one. `all` writes every
//! call. `collapsed`, the default, keeps a game that polls in a loop from
//! filling the log: a call site, a return address and a function, gets
//! its first `FIRST_LINES` lines in a `WINDOW` of emulated time, and the
//! rest of its calls in the window are only counted. The count goes to
//! the log as one summary line when the window has ended, at the site's
//! next call or the next sweep of all the sites, whichever comes first,
//! so a loop that stops still gets its summary. A site whose window
//! ended with a summary and that is called again at once is still in its
//! loop, and its next window gets `BUSY_LINES` lines. A call that isn't
//! written isn't formatted either: it costs a hash lookup.
//!
//! The windows go by the emulated clock, so a run's log is the same
//! however fast the host is. The counts are the log's bookkeeping, not
//! the machine's: save states leave them out, and a loaded state or a
//! reset starts them again.

use crate::config::Win32Trace;
use crate::cpu::Cpu;
use std::collections::HashMap;
use std::hash::BuildHasherDefault;

/// How long a call site's window is, in emulated nanoseconds.
const WINDOW: u64 = 1_000_000_000;
/// The lines a call site gets in a window.
const FIRST_LINES: u32 = 3;
/// The lines it gets in a window that follows one with a summary while
/// its calls go on.
const BUSY_LINES: u32 = 1;
/// How long a site with nothing to report is kept without a call.
const IDLE: u64 = 10 * WINDOW;

/// Which trace a call is in: the 32-bit functions' or the 16-bit
/// modules'.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    W32,
    W16,
}

/// A call of a built-in function, as the trace knows it.
#[derive(Clone, Copy, Debug)]
pub struct Call {
    pub kind: Kind,
    /// The return address: flat for a 32-bit call, segment and offset
    /// for a 16-bit one.
    pub caller: u32,
    /// The function's id, its stub's trap number.
    pub id: u16,
    /// The module's name, as the line shows it, and the function's.
    pub module: &'static str,
    pub name: &'static str,
}

impl Call {
    fn key(&self) -> u64 {
        (self.caller as u64) << 32 | ((self.kind == Kind::W16) as u64) << 16 | self.id as u64
    }

    /// `[W32] 004D72B6: WINMM.timeGetTime`, the start of its lines.
    fn site(&self) -> String {
        match self.kind {
            Kind::W32 => format!("[W32] {:08X}: {}.{}", self.caller, self.module, self.name),
            Kind::W16 => format!(
                "[W16] {:04X}:{:04X}: {}.{}",
                self.caller >> 16,
                self.caller & 0xFFFF,
                self.module,
                self.name
            ),
        }
    }
}

/// A call site's window.
#[derive(Clone, Copy, Debug)]
struct Site {
    call: Call,
    /// When its window started, and when it was last called.
    start: u64,
    last: u64,
    /// The lines written in the window, and how many it may have.
    written: u32,
    lines: u32,
    /// The calls not written since the last summary.
    skipped: u32,
    /// Whether its last window ended with a summary.
    summarized: bool,
}

impl Site {
    fn new(call: Call, now: u64, lines: u32) -> Self {
        Site {
            call,
            start: now,
            last: now,
            written: 1,
            lines,
            skipped: 0,
            summarized: false,
        }
    }

    /// Whether `now` is past its window, or before it, as after the
    /// emulated clock was set back.
    fn ended(&self, now: u64) -> bool {
        now < self.start || now - self.start >= WINDOW
    }

    /// The summary of the calls not written, if there were any.
    fn summary(&mut self) -> Option<String> {
        if self.skipped == 0 {
            return None;
        }
        let line = format!(
            "{} x{} more in {:.1} s",
            self.call.site(),
            self.skipped,
            self.last.saturating_sub(self.start) as f64 / 1e9
        );
        self.skipped = 0;
        self.summarized = true;
        Some(line)
    }
}

/// A hasher for the sites' keys, which are one word each.
#[derive(Default)]
struct KeyHasher(u64);

impl std::hash::Hasher for KeyHasher {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(b as u64);
        }
    }

    fn write_u64(&mut self, v: u64) {
        self.0 = (self.0.rotate_left(5) ^ v).wrapping_mul(0x517C_C1B7_2722_0A95);
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

/// The trace's level and its call sites' windows.
#[derive(Debug, Default)]
pub struct Trace {
    level: Win32Trace,
    sites: HashMap<u64, Site, BuildHasherDefault<KeyHasher>>,
    /// When the next sweep of the sites is due.
    next_sweep: u64,
    /// The summaries waiting for the log.
    due: Vec<String>,
}

impl Trace {
    /// A trace at the level `level` with no sites, as after a reset.
    pub fn new(level: Win32Trace) -> Self {
        Trace {
            level,
            ..Trace::default()
        }
    }

    pub fn level(&self) -> Win32Trace {
        self.level
    }

    /// Whether the line for `call` at `now` is written. The summaries
    /// due go to `due` first.
    fn record(&mut self, call: Call, now: u64) -> bool {
        match self.level {
            Win32Trace::All => return true,
            Win32Trace::Off => return false,
            Win32Trace::Collapsed => {}
        }
        if now >= self.next_sweep || now + WINDOW < self.next_sweep {
            self.sweep(now);
        }
        let site = match self.sites.entry(call.key()) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(Site::new(call, now, FIRST_LINES));
                return true;
            }
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
        };
        if !site.ended(now) {
            site.last = now;
            if site.written < site.lines {
                site.written += 1;
                return true;
            }
            site.skipped += 1;
            return false;
        }
        self.due.extend(site.summary());
        // Still in its loop: the summary was just written and the calls
        // go on.
        let busy = site.summarized && now.saturating_sub(site.last) < WINDOW;
        *site = Site::new(call, now, if busy { BUSY_LINES } else { FIRST_LINES });
        true
    }

    /// Queue the summaries of the windows that have ended, and forget
    /// the sites idle for long.
    fn sweep(&mut self, now: u64) {
        self.next_sweep = now + WINDOW;
        let due = &mut self.due;
        self.sites.retain(|_, site| {
            if !site.ended(now) {
                return true;
            }
            due.extend(site.summary());
            now >= site.last && now - site.last < IDLE
        });
    }

    /// Queue every summary, and start the sites again.
    fn flush(&mut self) {
        let mut sites: Vec<Site> = self.sites.drain().map(|(_, site)| site).collect();
        // In the order the windows started, the same on every run.
        sites.sort_by_key(|site| (site.start, site.call.key()));
        self.due.extend(sites.iter_mut().filter_map(Site::summary));
        self.next_sweep = 0;
    }
}

/// The trace's level.
pub fn level(cpu: &Cpu) -> Win32Trace {
    cpu.bus.win32.trace.level
}

/// Whether calls may get lines: the log goes somewhere and the trace
/// isn't off. Callers find the caller's address only then.
pub fn on(cpu: &Cpu) -> bool {
    cpu.bus.logs() && cpu.bus.win32.trace.level != Win32Trace::Off
}

/// Whether the line for `call` goes to the log, which gets the
/// summaries due first. Only when `on`.
pub fn admit(cpu: &mut Cpu, call: Call) -> bool {
    let now = cpu.bus.clock.now_ns();
    let write = cpu.bus.win32.trace.record(call, now);
    write_due(cpu);
    write
}

/// Set the trace's level, writing the summaries of the calls so far.
pub fn set_level(cpu: &mut Cpu, level: Win32Trace) {
    flush(cpu);
    cpu.bus.win32.trace.level = level;
}

/// Write every call site's summary now, as the last program ends.
pub fn flush(cpu: &mut Cpu) {
    cpu.bus.win32.trace.flush();
    write_due(cpu);
}

fn write_due(cpu: &mut Cpu) {
    if cpu.bus.win32.trace.due.is_empty() {
        return;
    }
    let mut due = std::mem::take(&mut cpu.bus.win32.trace.due);
    for line in due.drain(..) {
        cpu.bus.log_string(&line);
    }
    cpu.bus.win32.trace.due = due;
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    fn call(caller: u32, name: &'static str) -> Call {
        Call {
            kind: Kind::W32,
            caller,
            id: name.len() as u16,
            module: "WINMM",
            name,
        }
    }

    /// The calls at the times given, in ms: which were written, and the
    /// summaries due after each.
    fn run(trace: &mut Trace, calls: &[(Call, u64)]) -> (Vec<bool>, Vec<String>) {
        let mut written = Vec::new();
        let mut lines = Vec::new();
        for &(call, ms) in calls {
            written.push(trace.record(call, ms * MS));
            lines.append(&mut trace.due);
        }
        (written, lines)
    }

    #[test]
    fn a_loop_gets_its_first_lines_and_a_count() {
        let mut trace = Trace::new(Win32Trace::Collapsed);
        let poll = call(0x004D_72B6, "timeGetTime");
        let calls: Vec<_> = (0..10).map(|i| (poll, i)).collect();
        let (written, due) = run(&mut trace, &calls);
        assert_eq!(written.iter().filter(|&&w| w).count(), 3);
        assert_eq!(&written[..3], &[true; 3]);
        assert!(due.is_empty(), "{due:?}");
        // The next call, a window later, writes the summary first.
        let (written, due) = run(&mut trace, &[(poll, 1009)]);
        assert_eq!(written, [true]);
        assert_eq!(due, ["[W32] 004D72B6: WINMM.timeGetTime x7 more in 0.0 s"]);
    }

    #[test]
    fn a_busy_loop_gets_one_line_a_window() {
        let mut trace = Trace::new(Win32Trace::Collapsed);
        let poll = call(0x0040_1000, "timeGetTime");
        let calls: Vec<_> = (0..3000).map(|ms| (poll, ms)).collect();
        let (written, due) = run(&mut trace, &calls);
        // 3 lines in the first second, then 1 in each of the others.
        assert_eq!(written.iter().filter(|&&w| w).count(), 5);
        assert_eq!(
            due,
            [
                "[W32] 00401000: WINMM.timeGetTime x997 more in 1.0 s",
                "[W32] 00401000: WINMM.timeGetTime x999 more in 1.0 s",
            ]
        );
    }

    #[test]
    fn a_loop_that_stops_gets_its_summary_from_the_sweep() {
        let mut trace = Trace::new(Win32Trace::Collapsed);
        let poll = call(0x0040_1000, "PeekMessageA");
        let other = call(0x0040_2000, "Sleep");
        let mut calls: Vec<_> = (0..500).map(|ms| (poll, ms)).collect();
        calls.push((other, 1500));
        let (_, due) = run(&mut trace, &calls);
        assert_eq!(
            due,
            ["[W32] 00401000: WINMM.PeekMessageA x497 more in 0.5 s"]
        );
        // A while later it starts again with all its first lines.
        let calls: Vec<_> = (0..5).map(|ms| (poll, 5000 + ms)).collect();
        let (written, due) = run(&mut trace, &calls);
        assert_eq!(written, [true, true, true, false, false]);
        assert!(due.is_empty(), "{due:?}");
    }

    #[test]
    fn sites_are_told_apart_by_caller_and_function() {
        let mut trace = Trace::new(Win32Trace::Collapsed);
        let a = call(0x0040_1000, "timeGetTime");
        let b = call(0x0040_1000, "PeekMessageA");
        let c = call(0x0040_2000, "timeGetTime");
        let w16 = Call {
            kind: Kind::W16,
            ..a
        };
        let calls: Vec<_> = (0..4)
            .flat_map(|ms| [(a, ms), (b, ms), (c, ms), (w16, ms)])
            .collect();
        let (written, _) = run(&mut trace, &calls);
        assert_eq!(written.iter().filter(|&&w| w).count(), 12);
        trace.flush();
        assert_eq!(
            trace.due,
            [
                "[W32] 00401000: WINMM.timeGetTime x1 more in 0.0 s",
                "[W32] 00401000: WINMM.PeekMessageA x1 more in 0.0 s",
                "[W16] 0040:1000: WINMM.timeGetTime x1 more in 0.0 s",
                "[W32] 00402000: WINMM.timeGetTime x1 more in 0.0 s",
            ]
        );
    }

    #[test]
    fn all_and_off_keep_no_counts() {
        let poll = call(0x0040_1000, "timeGetTime");
        let calls: Vec<_> = (0..10).map(|ms| (poll, ms)).collect();
        let mut all = Trace::new(Win32Trace::All);
        assert_eq!(run(&mut all, &calls).0, [true; 10]);
        let mut off = Trace::new(Win32Trace::Off);
        assert_eq!(run(&mut off, &calls).0, [false; 10]);
        assert!(all.sites.is_empty() && off.sites.is_empty());
    }

    #[test]
    fn a_clock_set_back_starts_a_new_window() {
        let mut trace = Trace::new(Win32Trace::Collapsed);
        let poll = call(0x0040_1000, "timeGetTime");
        let calls: Vec<_> = (0..5).map(|ms| (poll, 9000 + ms)).collect();
        run(&mut trace, &calls);
        let (written, due) = run(&mut trace, &[(poll, 10)]);
        assert_eq!(written, [true]);
        assert_eq!(due, ["[W32] 00401000: WINMM.timeGetTime x2 more in 0.0 s"]);
    }

    /// The cost of a call that isn't written, for a look with
    /// `cargo test --release --lib win32::trace -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn skipped_call_cost() {
        let mut trace = Trace::new(Win32Trace::Collapsed);
        let sites: Vec<Call> = (0..16)
            .map(|i| call(0x0040_1000 + i * 16, "timeGetTime"))
            .collect();
        let n = 10_000_000u64;
        let start = std::time::Instant::now();
        let mut written = 0;
        for i in 0..n {
            // 10,000 calls an emulated millisecond.
            written += trace.record(sites[(i % 16) as usize], i * 100) as u32;
            trace.due.clear();
        }
        let ns = start.elapsed().as_nanos() as f64 / n as f64;
        println!("{ns:.1} ns a call, {written} written");
    }
}
