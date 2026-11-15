//! The frame rate cap (`fps_cap`): the most frames a second a program
//! may show, whichever card shows them. Each card's or API's present
//! asks `FrameCap::wait` when the frame it is about to show would show,
//! and waits in its own way until the time it answers: the 3dfx card
//! holds its swap and the program with it (`Voodoo::swap`), a thread of
//! the built-in Windows blocks as it does for a retrace and calls again
//! (`win32::pace`). Once the frame shows, the present tells the cap
//! (`shown`).
//!
//! Each frame has a turn, a frame time (1/30 s at 30) after the last
//! one's, in emulated time. A program that draws faster than the cap
//! waits for the turn, so its frames come a frame time apart; one that
//! draws slower shows each frame as it is done. A frame that comes after
//! its turn shows at once, and if it is no more than three frame times
//! late (`LATE_FRAMES`) the turns keep their schedule: the next frame's
//! turn is a frame time after the late one's turn, sooner than that
//! after the late frame itself, and the program gets the time back
//! (`shown`). A program that can draw faster than the cap therefore
//! shows the cap's rate whatever its single frames take, and one whose
//! logic steps once a frame loses no time to the cap: Myth II, whose
//! frames take 9 to 18 ms and one in a thousand 50, shows 75.0 frames a
//! second under a cap of 75. With turns that started again from every
//! late frame it showed 66.5, and 74.9 with one or two frame times
//! kept, the 50 ms frame costing the frames after it 37 ms.
//!
//! A frame later than that starts the schedule again, as from a frame
//! that came a frame time late: the next frame shows as soon as it is
//! drawn, the ones after it wait a frame time each, and no more than
//! three frames ever make up for a late one. Grom's menu, which steps
//! its simulation for 114 to 190 ms between frames drawn in under 3 ms,
//! shows 25 frames a second under a cap of 60; let through until the
//! number was made up, it showed 60, four in five under 3 ms after the
//! one before, of which a display of 60 Hz showed 20. The frame time
//! kept is for a program slower than the cap whose frames are uneven:
//! Myth II at 50 frames a second, its frames 13 to 26 ms, starts the
//! schedule again three times a second, and with the schedule started
//! from the late frame itself the short frame after each waited for
//! its turn, 46 holds in 20 s and 13 frames fewer. With a frame time
//! kept none waits. What the frames after such a late one wait for is
//! counted (`Counters`), so that a program running slow for it can be
//! told. The window, with `vrr`, refreshes at the cap's rate while a
//! program shows its frames through it (`refresh_timing`).
//!
//! A present that waits for the display's retrace already shows no more
//! frames than the display does: with the cap at or above the display's
//! rate, the cap holds none of its frames, and below it the frame shows
//! at the retrace nearest its turn (`wait`).
//!
//! The default, `auto` (`Cap::Auto`), holds a present that doesn't wait
//! for the retrace to 60 frames a second and none that does: a program
//! that waits for the retrace of a 72, 75 or 85 Hz mode shows a frame at
//! each as on the real machine, and one that counts its frames for its
//! timing keeps its speed. A rate set by number holds every present.
//!
//! The clock is the machine's emulated time, and only it: the turns, the
//! time a present asks about and the time a held present waits for are
//! all emulated nanoseconds, and a held present waits as a thread waits
//! for a timeout, while the machine's time runs on, its interrupts and
//! other threads run, and with nothing to run it idles to the turn. How
//! fast the host runs the machine changes how long the hold takes on the
//! wall clock, never which frames are held or until when. A run with
//! `--deterministic` therefore holds the same frames until the same
//! emulated times every time, and a fixed `cycles` below real time
//! holds no frame that is late in emulated time, however late it is on
//! the wall clock.

use std::cell::Cell;
use std::rc::Rc;

use crate::savestate::{Reader, Result, State, Writer};
use crate::video::crt::CrtTiming;

/// The rates `fps_cap` takes, in frames a second.
pub const RATES: std::ops::RangeInclusive<u32> = 10..=240;

/// A value of `fps_cap`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Cap {
    /// `off` (or 0): no cap. A machine made without settings (the
    /// tests, the benchmarks) has none.
    #[default]
    Off,
    /// `auto`, the default: `AUTO_FPS` for a present that doesn't wait
    /// for the display's retrace, and none for one that does, which
    /// shows at the display's rate as on the real machine. A program
    /// that waits for the retrace of a 72, 75 or 85 Hz mode runs at that
    /// rate, and one that counts frames for its timing at its speed.
    Auto,
    /// A rate of `RATES` for every present, also one that waits for the
    /// retrace of a faster display.
    Fps(u32),
}

/// `fps_cap` when the configuration doesn't set it.
pub const DEFAULT: Cap = Cap::Auto;

/// The rate `auto` holds presents that don't wait for the retrace to:
/// the rate of most displays. A program slower than it keeps its own.
pub const AUTO_FPS: u32 = 60;

impl Cap {
    /// The rate of a present that doesn't wait for the retrace, None
    /// for none.
    pub fn rate(self) -> Option<u32> {
        match self {
            Cap::Off => None,
            Cap::Auto => Some(AUTO_FPS),
            Cap::Fps(fps) => Some(fps),
        }
    }
}

/// A rate, None or 0 for no cap.
impl From<Option<u32>> for Cap {
    fn from(fps: Option<u32>) -> Cap {
        match fps {
            Some(fps) if fps > 0 => Cap::Fps(fps),
            _ => Cap::Off,
        }
    }
}

/// A value of `fps_cap`: `auto`, `off` (or 0) or a rate of `RATES`;
/// None for anything else.
pub fn parse(value: &str) -> Option<Cap> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("off") || value == "0" {
        return Some(Cap::Off);
    }
    if value.eq_ignore_ascii_case("auto") {
        return Some(Cap::Auto);
    }
    value
        .parse()
        .ok()
        .filter(|fps| RATES.contains(fps))
        .map(Cap::Fps)
}

/// `fps_cap`'s value as the configuration file writes it.
pub fn name(cap: Cap) -> String {
    match cap {
        Cap::Off => "off".to_string(),
        Cap::Auto => "auto".to_string(),
        Cap::Fps(fps) => fps.to_string(),
    }
}

/// How long after its last frame a program counts as still showing
/// frames through the cap, in ns: the window refreshes at the cap's
/// frame times until then.
const ACTIVE_NS: u64 = 1_000_000_000;

/// How late a frame may come, in frame times, with the turns keeping
/// their schedule (`FrameCap::shown`): 50 ms under a cap of 60. The
/// least that costs no game of those measured any time: Myth II draws
/// a frame in 50 ms when it takes an order, 33.5 ms after its turn
/// under a cap of 60 and 37 ms under 75, which one and two frame times
/// don't cover. More would let a longer run of frames through at once
/// after a stall.
const LATE_FRAMES: u64 = 3;

/// The most time a fresh start of the schedule counts as owed to a
/// program (`Counters::dropped_ns`), in ns: a longer stretch without
/// frames is a load or a pause, which no schedule would make up for.
const OWED_MOST_NS: u64 = 1_000_000_000;

/// What the cap did to the frames that came to it, counted up since the
/// machine started, for `/api/stats`. A present the cap doesn't hold
/// (one that waits for the retrace, under `auto` or a cap at or above
/// the display's rate) counts in none of them, and a save state keeps
/// none.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    /// The frames that showed through the cap.
    pub shown: u64,
    /// Those that waited for their turn.
    pub held: u64,
    /// Those that came after their turn, and showed at once.
    pub late: u64,
    /// Those of them more than `LATE_FRAMES` frame times late, from
    /// which the schedule started again.
    pub reanchors: u64,
    /// The time held frames waited that the schedule would not have made
    /// them wait had it not started again, in ns: a fresh start moves the
    /// turns back by how late its frame was less a frame time, and the
    /// frames held after it wait up to that much longer than under the
    /// turns before, until the next fresh start. Over the emulated time,
    /// the most a program that steps its logic once a frame ran slow for
    /// the cap's turns starting again.
    pub dropped_ns: u64,
}

/// The time the frames the cap held took to draw and the time it held
/// them, in ns, and how many there were, counted up since the machine
/// started (`FrameCap::held`). A present that comes sooner than
/// `LEAST_WORK_NS` after the frame before showed counts in none: it
/// shows a frame drawn already, and the time it was held kept the
/// program from no drawing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Held {
    pub frames: u64,
    pub work_ns: u64,
    pub hold_ns: u64,
}

impl Held {
    /// The frames the program would have drawn in the time the cap held
    /// it since `before`, an earlier reading: that time over the time its
    /// held frames took to draw on average. A program held to 60 that
    /// draws a frame in 4 ms would have drawn 3.2 more in each hold of
    /// 12.7 ms. One whose frames take 1 and 9 ms by turns is held 15.7
    /// and 7.7 ms, and would have drawn 4.7 frames of 5 ms on average in
    /// the two holds: 200 a second, as it draws without the cap.
    pub fn frames_since(&self, before: &Held) -> f64 {
        let work = self.work_ns.saturating_sub(before.work_ns);
        if work == 0 {
            return 0.0;
        }
        let frames = self.frames.saturating_sub(before.frames) as f64;
        self.hold_ns.saturating_sub(before.hold_ns) as f64 * frames / work as f64
    }
}

/// The least time after the frame before showed for a present to count
/// as a frame drawn (`Held`), in ns: Max Payne's menu in a window
/// presents every 0.74 ms uncapped.
const LEAST_WORK_NS: u64 = 100_000;

/// The cap and the frames it let through.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameCap {
    /// The setting's cap, which a save state doesn't keep.
    cap: Cap,
    /// The display's frame time, if the last present to ask (`wait`)
    /// waited for its retrace, which `auto` doesn't hold: the display's
    /// own refresh paces the window then (`refresh_timing`), and a frame
    /// shows at the retrace nearest its turn (`shown`). A save state
    /// doesn't keep it: the first present after a load sets it again.
    retrace: Option<u64>,
    /// When the last frame showed, in emulated ns; None before the
    /// first.
    last: Option<u64>,
    /// The last frame's turn (`shown`), in emulated ns: the next frame
    /// may show a frame time after it.
    slot: Option<u64>,
    /// When the frame now held came (`wait`), in emulated ns, and what
    /// the cap held so far (`held`). Only `cycles=auto` reads them, never
    /// the machine, and a save state doesn't keep them.
    held_from: Option<u64>,
    held: Held,
    /// What the cap did (`counters`), and how far the schedule's fresh
    /// starts moved the turns back that held frames haven't waited out
    /// yet, in ns (`Counters::dropped_ns`). The machine reads neither.
    counters: Counters,
    owed: u64,
}

impl FrameCap {
    /// `fps_cap`: a `Cap`, or a rate with None (or 0) for no cap.
    pub fn set(&mut self, cap: impl Into<Cap>) {
        self.cap = match cap.into() {
            Cap::Fps(0) => Cap::Off,
            cap => cap,
        };
    }

    pub fn cap(&self) -> Cap {
        self.cap
    }

    /// When the last frame showed, in emulated ns.
    pub fn last(&self) -> Option<u64> {
        self.last
    }

    /// The cap's frame time in ns for a present that doesn't wait for
    /// the retrace, if there is a cap.
    pub fn frame_ns(&self) -> Option<u64> {
        self.cap.rate().map(|fps| 1_000_000_000 / u64::from(fps))
    }

    /// A frame that would show at `at` (emulated ns): None if it may,
    /// or the time its present waits for, after which the frame shows
    /// (at the first retrace after it, for a present that waits for
    /// one). `refresh_ns` is the display's frame time for a present that
    /// waits for its retrace, which the cap doesn't hold when its own
    /// frame time is no longer, nor under `auto`.
    ///
    /// Such a present shows at the retrace nearest its turn, which may
    /// come up to half a retrace before it: the display's rate is never
    /// exactly a multiple of the cap's, and a retrace a little before the
    /// turn would otherwise wait for the next one, a frame shown every
    /// third retrace in place of every second (a cap of 30 on a display
    /// of 60.00006 Hz showed 25 frames a second).
    pub fn wait(&mut self, at: u64, refresh_ns: Option<u64>) -> Option<u64> {
        self.retrace = refresh_ns;
        let frame = self.frame_ns()?;
        if self.leaves_to_retrace() {
            return None;
        }
        let slack = refresh_ns.map_or(0, |refresh| refresh / 2);
        let next = (self.slot? + frame).saturating_sub(slack);
        if at >= next {
            return None;
        }
        // The call comes again at the turn: the frame came the first time.
        self.held_from.get_or_insert(at);
        Some(next)
    }

    /// Whether the last present to ask (`wait`) waits for a retrace that
    /// the cap leaves it to: any under `auto`, and one of a display no
    /// faster than the cap.
    fn leaves_to_retrace(&self) -> bool {
        match (self.retrace, self.frame_ns()) {
            (Some(refresh), Some(frame)) => self.cap == Cap::Auto || frame <= refresh,
            _ => false,
        }
    }

    /// A frame showed at `at`. Its turn was a frame time after the last
    /// one's, and the next one's is a frame time after that turn if this
    /// frame showed no more than `LATE_FRAMES` frame times after it: a
    /// frame that waited for its turn, and a late one, which the next
    /// ones make up for. From a frame later than that the schedule
    /// starts again as from a frame a frame time late: the next turn is
    /// when it showed, and the one after that a frame time later. One
    /// frame makes up for it, so that a program slower than the cap
    /// whose short frames follow long ones isn't held after each fresh
    /// start, and no more than `LATE_FRAMES` ever do at once. A present that
    /// waits for the retrace shows at the retrace nearest its turn
    /// (`wait`), up to half a retrace before or after it, which isn't
    /// late: the retraces it shows at keep the cap's rate (60 a second on
    /// a display of 70 Hz, a frame every retrace with one in seven left
    /// out).
    pub fn shown(&mut self, at: u64) {
        let held = self.held_from.take();
        let last = self.last.replace(at);
        let (Some(frame), false) = (self.frame_ns(), self.leaves_to_retrace()) else {
            self.slot = Some(at);
            return;
        };
        self.counters.shown += 1;
        // No hold is longer than a frame time and half a retrace: a
        // longer one is of a present that never showed.
        let hold = held.map(|from| (from, at.saturating_sub(from)));
        if let Some((from, hold)) = hold.filter(|&(_, hold)| hold <= 2 * frame) {
            self.counters.held += 1;
            let waited_out = hold.min(self.owed);
            self.owed -= waited_out;
            self.counters.dropped_ns += waited_out;
            let work = last.map_or(0, |last| from.saturating_sub(last));
            if work >= LEAST_WORK_NS {
                self.held.frames += 1;
                self.held.work_ns += work;
                self.held.hold_ns += hold;
            }
        }
        let Some(turn) = self.slot.map(|slot| slot + frame) else {
            self.slot = Some(at);
            return;
        };
        // A held frame shows at its turn, or within the clock's step for
        // a thread's wait after it (`win32::pace::check`).
        let slack = self.retrace.map_or(0, |refresh| refresh / 2);
        if held.is_none() && at > turn + slack {
            self.counters.late += 1;
        }
        self.slot = Some(if at <= turn + LATE_FRAMES * frame {
            turn
        } else {
            // What the frames after the fresh start before didn't wait
            // out isn't owed any more: a program that came this late
            // again had no use for those turns, as one slower than the
            // cap that speeds up (a load, then a menu) had none.
            self.counters.reanchors += 1;
            self.owed = (at - turn - frame).min(OWED_MOST_NS);
            at - frame
        });
    }

    /// What the cap did to the frames so far.
    pub fn counters(&self) -> Counters {
        self.counters
    }

    /// What the cap held so far, from which `cycles=auto` reckons the
    /// frames the program would have drawn while it was held
    /// (`Held::frames_since`) and adds them to the frames it counts, so
    /// that it reads a held program's frame rate as it reads the same
    /// program's without the cap, and settles at the same speed: a
    /// program held to 60 that would draw 120 has speed to spare, and one
    /// that draws its frames in bursts between long steps (Grom's menu
    /// shows 25 a second under a cap of 60 and 71 without, at one speed)
    /// isn't short of the cap's rate for the frames the cap spread out.
    /// The frames are reckoned at the held frames' average time, so that
    /// the short frames of a program whose frames take long and short
    /// times by turns don't stand for many more than it draws. A present
    /// that waits for the retrace under a cap below the display's rate
    /// counts from the retrace it would have shown at.
    pub fn held(&self) -> Held {
        self.held
    }

    /// A steady timing at the cap's rate, a retrace at every multiple
    /// of its frame time, while a program shows its frames through it
    /// (one in the last second before `now`): the window, with `vrr`,
    /// refreshes at those times. None under `auto` while the program
    /// waits for the retrace, which `auto` doesn't hold.
    pub fn refresh_timing(&self, now: u64) -> Option<CrtTiming> {
        if self.retrace.is_some() && self.cap == Cap::Auto {
            return None;
        }
        let frame = self.frame_ns()? as u32;
        let last = self.last?;
        if now.saturating_sub(last) > ACTIVE_NS {
            return None;
        }
        Some(CrtTiming {
            line_ns: frame,
            hdisplay_ns: frame,
            total: 1,
            display: 1,
            retrace_start: 0,
            retrace_end: 1,
        })
    }
}

/// The machine's cap, shared by the bus and the cards that present
/// through it (`Bus::frame_cap`).
#[derive(Clone, Debug, Default)]
pub struct Shared(Rc<Cell<FrameCap>>);

impl Shared {
    pub fn get(&self) -> FrameCap {
        self.0.get()
    }

    pub fn set(&self, cap: impl Into<Cap>) {
        let cap = cap.into();
        self.update(|c| c.set(cap));
    }

    pub fn shown(&self, at: u64) {
        self.update(|cap| cap.shown(at));
    }

    /// `shown` for a present that asked `wait` with `refresh_ns` some
    /// time before, with a save state loaded in between perhaps (the
    /// 3dfx card's held swap), which doesn't keep what `wait` noted.
    pub fn shown_later(&self, at: u64, refresh_ns: Option<u64>) {
        self.update(|cap| {
            cap.retrace = refresh_ns;
            cap.shown(at);
        });
    }

    /// `FrameCap::wait`.
    pub fn wait(&self, at: u64, refresh_ns: Option<u64>) -> Option<u64> {
        let mut cap = self.0.get();
        let until = cap.wait(at, refresh_ns);
        self.0.set(cap);
        until
    }

    /// Forget the frames shown, as a new machine or a loaded state that
    /// has none.
    pub fn forget(&self) {
        self.update(|cap| {
            (cap.last, cap.slot, cap.held_from, cap.owed) = (None, None, None, 0);
        });
    }

    fn update(&self, f: impl FnOnce(&mut FrameCap)) {
        let mut cap = self.0.get();
        f(&mut cap);
        self.0.set(cap);
    }
}

/// A save state keeps the frames shown, the cap being the setting's.
impl State for Shared {
    fn save(&self, w: &mut Writer) {
        let cap = self.get();
        cap.last.save(w);
        cap.slot.save(w);
    }

    fn load(&mut self, r: &mut Reader) -> Result<()> {
        let (mut last, mut slot) = (None, None);
        last.load(r)?;
        slot.load(r)?;
        // The turns owed were of the run before.
        self.update(|cap| (cap.last, cap.slot, cap.held_from, cap.owed) = (last, slot, None, 0));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    fn cap(fps: u32) -> FrameCap {
        let mut cap = FrameCap::default();
        cap.set(Some(fps));
        cap
    }

    #[test]
    fn the_first_frame_shows_at_once() {
        assert_eq!(cap(20).wait(10 * MS, None), None);
    }

    #[test]
    fn a_frame_soon_after_waits_for_the_next_frame_time() {
        let mut cap = cap(20);
        cap.shown(10 * MS);
        assert_eq!(cap.wait(12 * MS, None), Some(60 * MS));
        assert_eq!(cap.wait(59 * MS, None), Some(60 * MS));
        assert_eq!(cap.wait(60 * MS, None), None);
    }

    #[test]
    fn a_late_frame_shows_when_it_comes_and_the_next_makes_up_for_it() {
        let mut cap = cap(20);
        cap.shown(50 * MS);
        assert_eq!(cap.wait(120 * MS, None), None);
        cap.shown(120 * MS);
        // 20 ms after its turn at 100: the next one's is at 150.
        assert_eq!(cap.wait(130 * MS, None), Some(150 * MS));
        cap.shown(150 * MS);
        assert_eq!(cap.wait(160 * MS, None), Some(200 * MS));
        let counted = cap.counters();
        assert_eq!((counted.shown, counted.held, counted.late), (3, 1, 1));
        assert_eq!((counted.reanchors, counted.dropped_ns), (0, 0));
    }

    #[test]
    fn a_frame_too_late_starts_the_turns_again() {
        let mut cap = cap(20);
        cap.shown(50 * MS);
        // Its turn was at 100. As late as a frame may be, the turns stay.
        let kept = 100 * MS + LATE_FRAMES * 50 * MS;
        let mut on_time = cap;
        on_time.shown(kept);
        assert_eq!(
            on_time.wait(kept, None),
            None,
            "the next one's turn is past"
        );
        assert_eq!(on_time.counters().reanchors, 0);
        // 10 ms later than that, they start again from it as from a
        // frame that came a frame time late: the next one shows as soon
        // as it is drawn, and the one after it a frame time after the
        // late one.
        let late = kept + 10 * MS;
        assert_eq!(cap.wait(late, None), None);
        cap.shown(late);
        assert_eq!(cap.wait(late + 5 * MS, None), None);
        cap.shown(late + 5 * MS);
        assert_eq!(cap.wait(late + 10 * MS, None), Some(late + 50 * MS));
        assert_eq!(cap.counters().reanchors, 1);
        assert_eq!(cap.counters().dropped_ns, 0, "nothing waited yet");
        // The frame held for 40 ms would have shown at once on the turns
        // before, and so the ones after it, until they have waited as
        // long as the turns moved back by.
        cap.shown(late + 50 * MS);
        assert_eq!(cap.counters().dropped_ns, 40 * MS);
        let mut now = late + 50 * MS;
        for _ in 0..9 {
            now = cap.wait(now + 5 * MS, None).unwrap();
            cap.shown(now);
        }
        let counted = cap.counters();
        assert_eq!((counted.shown, counted.held, counted.late), (13, 10, 2));
        assert_eq!(
            (counted.reanchors, counted.dropped_ns),
            (1, late - 150 * MS)
        );
    }

    #[test]
    fn a_program_slower_than_the_cap_is_owed_nothing() {
        // 20 frames a second under a cap of 60: every frame is late, the
        // turns start again whenever they are too far back, and none is
        // held, so the cap cost it no time.
        let mut cap = cap(60);
        for i in 1..=100 {
            assert_eq!(cap.wait(i * 50 * MS, None), None);
            cap.shown(i * 50 * MS);
        }
        let counted = cap.counters();
        assert_eq!((counted.shown, counted.held, counted.late), (100, 0, 99));
        assert!(counted.reanchors > 0);
        assert_eq!(counted.dropped_ns, 0);
    }

    #[test]
    fn a_program_slower_than_the_cap_that_speeds_up_is_owed_one_fresh_start() {
        // 20 frames a second for 10 s, its turns started again at every
        // frame, each moving them back by 33.3 ms, then frames of 5 ms.
        // Only the last fresh start counts: the ones before it added up
        // to a second, which the first second of fast frames counted as
        // dropped.
        let mut cap = cap(60);
        let mut now = 0;
        for _ in 0..200 {
            now += 50 * MS;
            assert_eq!(cap.wait(now, None), None);
            cap.shown(now);
        }
        assert_eq!(cap.counters().reanchors, 198);
        for _ in 0..600 {
            now = cap.wait(now + 5 * MS, None).unwrap_or(now + 5 * MS);
            cap.shown(now);
        }
        let dropped = cap.counters().dropped_ns;
        assert_eq!(dropped, 33_333_334);
    }

    #[test]
    fn a_load_counts_as_no_more_than_a_second_owed() {
        let frame = 1_000_000_000 / 60;
        let mut cap = cap(60);
        cap.shown(frame);
        // Ten seconds without a frame, then frames as fast as they go.
        let mut now = 10_000 * MS;
        cap.shown(now);
        for _ in 0..600 {
            now = cap.wait(now + MS, None).unwrap_or(now + MS);
            cap.shown(now);
        }
        assert_eq!(cap.counters().dropped_ns, OWED_MOST_NS);
    }

    #[test]
    fn frames_a_little_under_and_over_the_frame_time_by_turns_show_the_caps_rate() {
        // 0.9 and 1.1 frame times by turns, 60 a second without a cap:
        // the frame after a late one shows at its turn, 0.9 of a frame
        // time after the late one, and the rate stays 60. With turns
        // that started again from every late frame it was 57.1 (two
        // frames in 2.1 frame times), and 54.5 with 0.8 and 1.2.
        let frame = 1_000_000_000 / 60;
        for tenths in [1, 2, 5, 9] {
            let work = [frame * (10 - tenths) / 10, frame * (10 + tenths) / 10];
            let shown = presented(60, &work, 601);
            assert!((rate(&shown) - 60.0).abs() < 0.1, "{}", rate(&shown));
            let shortest = shown.windows(2).map(|w| w[1] - w[0]).min().unwrap();
            assert!(shortest >= work[0] - 10, "{shortest}");
        }
    }

    #[test]
    fn the_counters_leave_out_a_present_the_retrace_paces() {
        // 75 Hz under `auto`, and 60 Hz under a cap of 60.
        for (under, refresh) in [(Cap::Auto, 1_000_000_000 / 75), (Cap::Fps(60), 16_666_667)] {
            let mut cap = FrameCap::default();
            cap.set(under);
            for i in 1..=300 {
                assert_eq!(cap.wait(i * refresh, Some(refresh)), None);
                cap.shown(i * refresh);
            }
            assert_eq!(cap.counters(), Counters::default(), "{:?}", under);
            assert_eq!(cap.held(), Held::default());
        }
    }

    #[test]
    fn the_frames_a_held_program_would_have_drawn_make_up_its_rate() {
        // Frames of 4 ms under a cap of 50: each waits 16 ms, in which
        // the program would have drawn four more, 250 a second in all.
        let shown = presented_by(cap(50), &[4 * MS], 101).1;
        assert_eq!(shown.counters().held, 100);
        let would = shown.held().frames_since(&Held::default());
        assert!((would - 100.0 * 4.0).abs() < 1e-6, "{would}");
    }

    #[test]
    fn frames_of_uneven_length_are_reckoned_at_their_average() {
        // 1 and 9 ms by turns, 200 frames a second without a cap. Under
        // 60 each pair takes two frame times and is held for the rest,
        // in which the program would have drawn as many more pairs as
        // fit: 200 a second in all. Reckoned at each frame's own time,
        // the 1 ms frames' holds made it 556.
        let ms = 1_000_000;
        let (shown, cap) = presented_by(cap(60), &[ms, 9 * ms], 601);
        let seconds = (shown[600] - shown[0]) as f64 / 1e9;
        let would = cap.held().frames_since(&Held::default());
        let rate = (600.0 + would) / seconds;
        assert!((rate - 200.0).abs() < 2.0, "{rate}");
    }

    #[test]
    fn a_present_right_after_the_last_adds_no_frames_drawn() {
        // A frame of 33 ms shown twice, the second present at once: held
        // until its turn, in which the program drew nothing. Reckoned at
        // the least time a frame takes, a hold of a frame time stood for
        // 167 frames, and `cycles=auto` took the speed down to its least
        // at any speed.
        let (_, cap) = presented_by(cap(60), &[33 * MS, 0], 200);
        assert_eq!(cap.counters().held, 100);
        assert_eq!(cap.held(), Held::default());
    }

    #[test]
    fn a_program_drawing_as_fast_as_it_can_gets_the_caps_rate() {
        let mut cap = cap(30);
        let mut now = 3 * MS;
        let mut shown = Vec::new();
        for _ in 0..31 {
            if let Some(until) = cap.wait(now, None) {
                now = until;
            }
            cap.shown(now);
            shown.push(now);
            now += MS / 2;
        }
        let second = shown[30] - shown[0];
        assert!(
            (second as i64 - 1_000_000_000).abs() < 34 * MS as i64,
            "{} ns",
            second
        );
        assert!(shown.windows(2).all(|w| w[1] - w[0] == 1_000_000_000 / 30));
    }

    /// The times a program's frames show at under a cap of `fps`, when
    /// each frame takes the next of `work` (ns, in turn) to draw after
    /// the last one showed.
    fn presented(fps: u32, work: &[u64], frames: usize) -> Vec<u64> {
        presented_by(cap(fps), work, frames).0
    }

    fn presented_by(mut cap: FrameCap, work: &[u64], frames: usize) -> (Vec<u64>, FrameCap) {
        let mut now = 0;
        let mut shown = Vec::new();
        for i in 0..frames {
            now += work[i % work.len()];
            if let Some(until) = cap.wait(now, None) {
                now = until;
            }
            cap.shown(now);
            shown.push(now);
        }
        (shown, cap)
    }

    /// Frames a second over the frames `shown`.
    fn rate(shown: &[u64]) -> f64 {
        (shown.len() - 1) as f64 * 1e9 / (shown[shown.len() - 1] - shown[0]) as f64
    }

    #[test]
    fn a_program_faster_than_the_cap_gets_the_cap() {
        let frame = 1_000_000_000 / 60;
        let shown = presented(60, &[frame * 2 / 3], 121);
        // Every frame after the first waits for its turn.
        assert!(
            shown.windows(2).all(|w| w[1] - w[0] == frame),
            "{:?}",
            &shown[..6]
        );
        assert!((rate(&shown) - 60.0).abs() < 0.5, "{}", rate(&shown));
    }

    #[test]
    fn one_frame_after_a_frame_too_late_makes_up_for_it() {
        // Nine frames of a fifth of a frame time, then one that comes a
        // frame time later than a frame may: the first fast one after it
        // shows as it is drawn, and each of the others waits a frame
        // time after the late one's. Only that one and the next come
        // closer than a frame time after the frame before them, and no
        // three frames come within a frame time but for the late one's.
        let frame = 1_000_000_000 / 60;
        let mut work = vec![frame / 5; 9];
        work.push(frame * (LATE_FRAMES + 2));
        let shown = presented(60, &work, 601);
        let late = |i: usize| shown[i] - shown[i - 1] > LATE_FRAMES * frame;
        for i in 3..shown.len() {
            let apart = shown[i] - shown[i - 1];
            assert!(apart >= frame || late(i - 1) || late(i - 2), "{i}");
            assert!(shown[i] - shown[i - 2] >= frame || late(i - 2), "{i}");
        }
        let rate = rate(&shown);
        let expected = 600.0 / (10 + LATE_FRAMES) as f64;
        assert!((rate - expected).abs() < 0.1, "{rate}");
    }

    #[test]
    fn a_program_slower_than_the_cap_whose_frames_are_uneven_is_never_held() {
        // Frames of 10 and 30 ms by turns, 50 a second, and of 5 and 29
        // ms, 58.8, under a cap of 60. The frames fall behind their turns
        // by a little each, and every so often one starts the turns
        // again. Started from that frame itself, the short frame after
        // it came before its turn and waited 6.7 ms, as it would for a
        // 60 Hz retrace; a program that steps its logic once a frame ran
        // 1% slow for it. Started as from a frame a frame time late, no
        // frame is held.
        for work in [[10 * MS, 30 * MS], [5 * MS, 29 * MS]] {
            let (shown, cap) = presented_by(cap(60), &work, 601);
            let counted = cap.counters();
            assert!(counted.reanchors > 0);
            assert_eq!((counted.held, counted.dropped_ns), (0, 0), "{work:?}");
            assert_eq!(shown[600], 300 * (work[0] + work[1]) + work[0]);
        }
    }

    #[test]
    fn a_program_with_long_frames_now_and_then_shows_no_long_bursts() {
        // Like Grom's menu at 150,000 cycles: frames of 2.6 ms, a 35 ms
        // step of its simulation in one frame in three, and once a second
        // five steps in one frame. Uncapped that is 61 frames a second,
        // two in three of them 2.6 ms after the one before. Under a cap
        // of 60 the frames after a step make up for as many frame times
        // as a frame may be late, and no more: one after the five
        // steps, and no second has more than 60 and those.
        let ms = 1_000_000;
        let frame = 1_000_000_000 / 60;
        let mut work = Vec::new();
        for i in 0..23 {
            work.extend([2_600_000, 2_600_000]);
            work.push(if i == 11 { 190 * ms } else { 37 * ms });
        }
        let shown = presented(60, &work, 1200);
        let late = shown.windows(2).filter(|w| w[1] - w[0] > 100 * ms).count();
        assert!(late > 0);
        let most = LATE_FRAMES as usize;
        for (i, &t) in shown.iter().enumerate() {
            let within = |ns: u64| shown[i..].iter().take_while(|&&u| u < t + ns).count();
            assert!(within(frame) <= 1 + most, "{} at {} ns", within(frame), t);
            let in_second = within(1_000_000_000);
            assert!(
                in_second <= 60 + most,
                "{} in the second from {} ns",
                in_second,
                t
            );
            // After the five steps, one frame as it is drawn, and the
            // next a frame time after the steps' frame.
            if i > 0 && i + 2 < shown.len() && t - shown[i - 1] > 100 * ms {
                assert!(shown[i + 2] - t >= frame);
            }
        }
    }

    /// A program that steps its simulation on the clock, as Grom's menu
    /// does: a step of 150 ms of game time takes 120 ms to run, and it
    /// draws frames of 2.8 ms until the next step is due. `present`
    /// gives the time a frame drawn by then shows at. The frames' times,
    /// and the steps run a second.
    fn stepping_on_the_clock(mut present: impl FnMut(u64) -> u64) -> (Vec<u64>, f64) {
        let (step, step_takes, frame_takes) = (150 * MS, 120 * MS, 2_800_000);
        let (mut now, mut steps, mut shown) = (0, 0, Vec::new());
        while now < 60_000 * MS {
            while (steps + 1) * step <= now {
                now += step_takes;
                steps += 1;
            }
            now = present(now + frame_takes);
            shown.push(now);
        }
        (shown, steps as f64 / 60.0)
    }

    /// Of the frames `shown`, how many a second a display of 60 Hz shows:
    /// the last one done at each of its retraces.
    fn seen_at_60(shown: &[u64]) -> f64 {
        let refresh = 1_000_000_000 / 60;
        let last = *shown.last().unwrap();
        let mut seen: Vec<_> = (1..last / refresh)
            .filter_map(|k| shown.iter().rposition(|&t| t <= k * refresh))
            .collect();
        seen.dedup();
        seen.len() as f64 * 1e9 / last as f64
    }

    #[test]
    fn a_program_stepping_on_the_clock_loses_no_time_and_shows_what_a_retrace_wait_would() {
        // Uncapped, the program draws 72 frames a second, in bursts of
        // ten 2.8 ms apart after each step, of which a 60 Hz display
        // shows 20. Under a cap of 60 the first frame after a step's
        // shows as it is drawn and the others of a burst wait a frame
        // time each, and the time they wait the program spends on steps
        // that came due meanwhile: it shows 24 a second, a display 18 of
        // them, and its simulation keeps its speed. A present that waits
        // for a 60 Hz retrace, as on a machine with the display synced,
        // shows 13.5. So the cap shows no fewer frames than a display of
        // its rate would, nor much fewer than the display shows of the
        // uncapped frames, and costs the program no time. The frames it
        // would have drawn meanwhile make up its uncapped rate for
        // `cycles=auto`.
        let (free, free_steps) = stepping_on_the_clock(|at| at);
        let mut cap = cap(60);
        let (held, held_steps) = stepping_on_the_clock(|at| {
            let at = cap.wait(at, None).unwrap_or(at);
            cap.shown(at);
            at
        });
        let refresh = 1_000_000_000 / 60;
        let (synced, synced_steps) = stepping_on_the_clock(|at| at.div_ceil(refresh) * refresh);
        assert!((free_steps - held_steps).abs() < 0.05 && (free_steps - synced_steps).abs() < 0.05);
        let free_rate = rate(&free);
        let held_rate = rate(&held);
        assert!((free_rate - 72.0).abs() < 1.0, "{free_rate}");
        assert!((held_rate - 24.3).abs() < 1.0, "{held_rate}");
        assert!(held_rate > rate(&synced), "{}", rate(&synced));
        let (seen_held, seen_free) = (seen_at_60(&held), seen_at_60(&free));
        assert!(seen_held > 0.8 * seen_free, "{seen_held} of {seen_free}");
        let would = cap.held().frames_since(&Held::default());
        let read = (held.len() as f64 + would) / 60.0;
        assert!((read / free_rate - 1.0).abs() < 0.05, "{read}");
    }

    #[test]
    fn a_frame_a_second_late_lets_one_more_show_at_once() {
        let frame = 1_000_000_000 / 60;
        let mut cap = cap(60);
        cap.shown(frame);
        let late = 1_000_000_000 + frame;
        assert_eq!(cap.wait(late, None), None);
        cap.shown(late);
        assert_eq!(cap.wait(late + 1000, None), None);
        cap.shown(late + 1000);
        assert_eq!(cap.wait(late + 2000, None), Some(late + frame));
    }

    #[test]
    fn a_retrace_a_little_faster_than_twice_the_cap_shows_every_other_one() {
        // A display of 60.00006 Hz under a cap of 30: every frame waits for
        // the retrace, and shows at the one nearest its turn.
        let refresh = 16_666_650;
        let shown = at_retraces(30, refresh, 61);
        assert!(
            shown.windows(2).all(|w| w[1] - w[0] == 2 * refresh),
            "{:?}",
            &shown[..6]
        );
    }

    #[test]
    fn a_program_slower_than_the_cap_keeps_its_own_rate() {
        // 0.9 times the cap: never held, so 54 frames a second, not 30.
        let shown = presented(60, &[1_000_000_000 / 54], 109);
        assert!((rate(&shown) - 54.0).abs() < 0.1, "{}", rate(&shown));
        assert!(shown.windows(2).all(|w| w[1] - w[0] == 1_000_000_000 / 54));
    }

    #[test]
    fn of_fast_and_slow_frames_in_turn_the_fast_ones_make_up_for_the_slow() {
        // Half a frame time, then one and a half, under a cap of 30: the
        // slow one shows half a frame time after its turn, and the fast
        // one after it at its own turn, which is then. Two frames take
        // two frame times, 30 a second, by turns one and a half and half
        // a frame time apart.
        let frame = 1_000_000_000 / 30;
        let shown = presented(30, &[frame / 2, frame * 3 / 2], 61);
        assert!((rate(&shown) - 30.0).abs() < 0.1, "{}", rate(&shown));
        assert!(shown[1..].windows(2).all(|w| w[1] - w[0] >= frame / 2));
    }

    #[test]
    fn the_retrace_rules_with_the_cap_at_or_above_its_rate() {
        let mut cap = cap(60);
        cap.shown(10 * MS);
        let refresh = 1_000_000_000 / 59;
        assert_eq!(cap.wait(11 * MS, Some(refresh)), None);
        // 70 Hz: the cap holds the frames, each to the retrace nearest
        // its turn, one or two retraces apart, 60 a second.
        let shown = at_retraces(60, 1_000_000_000 / 70, 121);
        assert!((rate(&shown) - 60.0).abs() < 0.5, "{}", rate(&shown));
        let gaps = shown.windows(2).map(|w| w[1] - w[0]);
        assert!(gaps.clone().all(|gap| gap <= 2 * (1_000_000_000 / 70)));
    }

    #[test]
    fn a_frame_at_the_retrace_after_its_turn_isnt_late() {
        // 60 a second on a display of 70 Hz: a frame whose turn is 7 ms
        // before a retrace shows at it, and the next one's turn is a
        // frame time after that turn. Without the retrace, as a held
        // swap finds it after a state is loaded unless the swap says so
        // (`shown_later`), it would count as late.
        let (frame, refresh) = (1_000_000_000 / 60, 1_000_000_000 / 70);
        let shared = Shared::default();
        shared.set(Some(60));
        shared.shown(10 * MS);
        let at = 10 * MS + frame + 7 * MS;
        shared.shown_later(at, Some(refresh));
        assert_eq!(shared.get().slot, Some(10 * MS + frame));
        assert_eq!(shared.get().last(), Some(at));
        assert_eq!(shared.get().counters().late, 0);
        shared.shown_later(10 * MS + 2 * frame + 7 * MS, None);
        assert_eq!(shared.get().slot, Some(10 * MS + 2 * frame));
        assert_eq!(shared.get().counters().late, 1);
    }

    /// The times frames show at under a cap of `fps` when each waits for
    /// a retrace of the display of frame time `refresh`, drawn at once.
    fn at_retraces(fps: u32, refresh: u64, frames: usize) -> Vec<u64> {
        at_retraces_under(Cap::Fps(fps), refresh, frames)
    }

    fn at_retraces_under(under: Cap, refresh: u64, frames: usize) -> Vec<u64> {
        let mut cap = FrameCap::default();
        cap.set(under);
        let mut shown = Vec::new();
        let mut retrace = refresh;
        for _ in 0..frames {
            let at = match cap.wait(retrace, Some(refresh)) {
                Some(until) => until.div_ceil(refresh) * refresh,
                None => retrace,
            };
            cap.shown(at);
            shown.push(at);
            retrace = at + refresh;
        }
        shown
    }

    #[test]
    fn auto_holds_a_present_without_the_retrace_to_60() {
        let mut cap = FrameCap::default();
        cap.set(Cap::Auto);
        let mut now = MS;
        let mut shown = Vec::new();
        for _ in 0..121 {
            if let Some(until) = cap.wait(now, None) {
                now = until;
            }
            cap.shown(now);
            shown.push(now);
            now += MS;
        }
        assert!((rate(&shown) - 60.0).abs() < 0.5, "{}", rate(&shown));
    }

    #[test]
    fn auto_shows_a_75_hz_retrace_wait_at_75_and_a_cap_of_60_at_60() {
        let refresh = 1_000_000_000 / 75;
        let auto = at_retraces_under(Cap::Auto, refresh, 151);
        assert!((rate(&auto) - 75.0).abs() < 0.1, "{}", rate(&auto));
        let sixty = at_retraces_under(Cap::Fps(60), refresh, 121);
        assert!((rate(&sixty) - 60.0).abs() < 0.5, "{}", rate(&sixty));
    }

    #[test]
    fn under_auto_the_window_keeps_the_displays_refresh_while_frames_wait_for_it() {
        let shared = Shared::default();
        shared.set(Cap::Auto);
        shared.wait(10 * MS, None);
        shared.shown(10 * MS);
        let timing = shared.get().refresh_timing(20 * MS).unwrap();
        assert_eq!(timing.frame_ns(), 1_000_000_000 / 60);
        shared.wait(30 * MS, Some(1_000_000_000 / 75));
        shared.shown(30 * MS);
        assert_eq!(shared.get().refresh_timing(40 * MS), None);
    }

    #[test]
    fn the_setting_reads_auto_off_and_rates() {
        assert_eq!(parse("auto"), Some(Cap::Auto));
        assert_eq!(parse(" AUTO "), Some(Cap::Auto));
        assert_eq!(parse("off"), Some(Cap::Off));
        assert_eq!(parse("0"), Some(Cap::Off));
        assert_eq!(parse("75"), Some(Cap::Fps(75)));
        assert_eq!(parse("9"), None);
        assert_eq!(parse("241"), None);
        for cap in [Cap::Auto, Cap::Off, Cap::Fps(45)] {
            assert_eq!(parse(&name(cap)), Some(cap));
        }
        assert_eq!(DEFAULT, Cap::Auto);
    }

    #[test]
    fn no_cap_holds_nothing() {
        let mut cap = FrameCap::default();
        cap.shown(10 * MS);
        assert_eq!(cap.wait(10 * MS, None), None);
        cap.set(Some(0));
        assert_eq!(cap.cap(), Cap::Off);
        cap.set(Cap::Fps(0));
        assert_eq!(cap.cap(), Cap::Off);
        assert_eq!(cap.refresh_timing(10 * MS), None);
    }

    #[test]
    fn the_window_refreshes_at_the_caps_times_while_frames_come() {
        let mut cap = cap(40);
        assert_eq!(cap.refresh_timing(0), None, "no frame yet");
        cap.shown(100 * MS);
        let timing = cap.refresh_timing(500 * MS).unwrap();
        assert_eq!(timing.frame_ns(), 25 * MS);
        assert_eq!(cap.refresh_timing(1200 * MS), None, "none for a second");
    }

    #[test]
    fn a_state_keeps_the_last_frame_and_not_the_cap() {
        let shared = Shared::default();
        shared.set(Some(30));
        shared.shown(480 * MS);
        // At a retrace before its turn: it counts at the turn.
        shared.shown(500 * MS);
        let mut w = Writer::new();
        shared.save(&mut w);
        let mut other = Shared::default();
        other.set(Some(50));
        other.load(&mut Reader::new(&w.buf)).unwrap();
        assert_eq!(other.get().last(), Some(500 * MS));
        assert_eq!(other.get().cap(), Cap::Fps(50));
        assert_eq!(other.get().slot, shared.get().slot);
        assert_ne!(other.get().slot, other.get().last());
    }
}
