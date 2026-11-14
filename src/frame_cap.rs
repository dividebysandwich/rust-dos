//! The frame rate cap (`fps_cap`): the most frames a second a program
//! may show, whichever card shows them. Each card's or API's present
//! asks `FrameCap::wait` when the frame it is about to show would show,
//! and waits in its own way until the time it answers: the 3dfx card
//! holds its swap and the program with it (`Voodoo::swap`), a thread of
//! the built-in Windows blocks as it does for a retrace and calls again
//! (`win32::pace`). Once the frame shows, the present tells the cap
//! (`shown`).
//!
//! Under a cap, no two frames show closer together than its frame time
//! (1/30 s at 30), in emulated time. Each frame has a turn, a frame time
//! after the last one's, and a program that draws faster than the cap
//! waits for it, so its frames come a frame time apart; one that draws
//! slower shows each frame as it is done. A late frame shows when it
//! comes, and its turn is then: the frames after it wait a frame time
//! each, as after any other, and don't make up for it (`shown`). A
//! program that is late now and then therefore shows fewer frames than
//! the cap, evenly, where frames let through at once to make up the
//! number would come in bursts: with a quarter of a second of them,
//! Grom's menu showed 60 a second, four in five under 3 ms after the
//! one before, and a display of 60 Hz showed 20 of them; Myth II under
//! a cap of 75 showed frames from 9.5 ms apart. The window, with `vrr`,
//! refreshes at the cap's rate while a program shows its frames through
//! it (`refresh_timing`).
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
        if refresh_ns.is_some() && self.cap == Cap::Auto {
            return None;
        }
        if refresh_ns.is_some_and(|refresh| frame <= refresh) {
            return None;
        }
        let slack = refresh_ns.map_or(0, |refresh| refresh / 2);
        let next = (self.slot? + frame).saturating_sub(slack);
        (at < next).then_some(next)
    }

    /// A frame showed at `at`. Its turn was a frame time after the last
    /// one's, and it counts there if it showed by then; a later one
    /// counts when it showed, so the frames after a late one don't make
    /// up for it. A present that waits for the retrace shows at the
    /// retrace nearest its turn (`wait`), up to half a retrace before
    /// or after it, and counts at the turn: the retraces it shows at
    /// then keep the cap's rate (60 a second on a display of 70 Hz, a
    /// frame every retrace with one in seven left out).
    pub fn shown(&mut self, at: u64) {
        self.last = Some(at);
        let slack = self.retrace.map_or(0, |refresh| refresh / 2);
        self.slot = Some(match (self.slot, self.frame_ns()) {
            (Some(slot), Some(frame)) if at <= slot + frame + slack => slot + frame,
            _ => at,
        });
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
        self.update(|cap| (cap.last, cap.slot) = (None, None));
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
        self.update(|cap| (cap.last, cap.slot) = (last, slot));
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
    fn a_late_frame_shows_when_it_comes() {
        let mut cap = cap(20);
        cap.shown(50 * MS);
        assert_eq!(cap.wait(120 * MS, None), None);
        cap.shown(120 * MS);
        // The next a frame time after it, not after the turn it missed.
        assert_eq!(cap.wait(130 * MS, None), Some(170 * MS));
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
        let mut cap = cap(fps);
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
        shown
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
    fn the_frames_after_a_late_one_dont_make_up_for_it() {
        // Nine frames of a fifth of a frame time, then one of four: each
        // fast one waits a frame time, the ones after the late one too,
        // so ten frames take thirteen frame times, 46 a second. Shown at
        // once until the number was made up, they gave 60, three of them
        // a fifth of a frame time apart after every late one.
        let frame = 1_000_000_000 / 60;
        let mut work = vec![frame / 5; 9];
        work.push(frame * 4);
        let shown = presented(60, &work, 601);
        assert!(shown.windows(2).all(|w| w[1] - w[0] >= frame));
        assert!(
            (rate(&shown) - 600.0 / 13.0).abs() < 0.1,
            "{}",
            rate(&shown)
        );
    }

    #[test]
    fn a_program_with_long_frames_now_and_then_shows_no_bursts() {
        // Like Grom's menu at 150,000 cycles: frames of 2.6 ms, a 35 ms
        // step of its simulation in one frame in three, and once a second
        // five steps in one frame. Uncapped that is 61 frames a second,
        // two in three of them 2.6 ms after the one before. Under a cap
        // of 60 none comes sooner than a frame time after the last, and
        // no second has more than 60.
        let ms = 1_000_000;
        let frame = 1_000_000_000 / 60;
        let mut work = Vec::new();
        for i in 0..23 {
            work.extend([2_600_000, 2_600_000]);
            work.push(if i == 11 { 190 * ms } else { 37 * ms });
        }
        let shown = presented(60, &work, 1200);
        assert!(shown.windows(2).all(|w| w[1] - w[0] >= frame));
        let late = shown.windows(2).filter(|w| w[1] - w[0] > 100 * ms).count();
        assert!(late > 0);
        for (i, &t) in shown.iter().enumerate() {
            let in_second = shown[i..]
                .iter()
                .take_while(|&&u| u < t + 1_000_000_000)
                .count();
            assert!(
                in_second <= 60,
                "{} frames in the second from {} ns",
                in_second,
                t
            );
        }
    }

    #[test]
    fn a_frame_a_second_late_lets_none_show_at_once() {
        let frame = 1_000_000_000 / 60;
        let mut cap = cap(60);
        cap.shown(frame);
        let late = 1_000_000_000 + frame;
        assert_eq!(cap.wait(late, None), None);
        cap.shown(late);
        assert_eq!(cap.wait(late + 1000, None), Some(late + frame));
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
    fn of_fast_and_slow_frames_in_turn_only_the_fast_ones_wait() {
        // Half a frame time, then one and a half: the fast one waits for
        // its turn, a frame time after the slow one showed, and the slow
        // one shows when it is done, so two frames take two and a half
        // frame times, 24 a second under a cap of 30.
        let frame = 1_000_000_000 / 30;
        let shown = presented(30, &[frame / 2, frame * 3 / 2], 61);
        assert!(shown.windows(2).all(|w| w[1] - w[0] >= frame));
        assert!((rate(&shown) - 24.0).abs() < 0.1, "{}", rate(&shown));
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
    fn a_frame_at_the_retrace_after_its_turn_counts_at_the_turn() {
        // 60 a second on a display of 70 Hz: a frame whose turn is 7 ms
        // before a retrace shows at it, and the next one's turn is a
        // frame time after that turn. Without the retrace, as a held
        // swap finds it after a state is loaded unless the swap says so
        // (`shown_later`), it would count as late, 7 ms later.
        let (frame, refresh) = (1_000_000_000 / 60, 1_000_000_000 / 70);
        let shared = Shared::default();
        shared.set(Some(60));
        shared.shown(10 * MS);
        let at = 10 * MS + frame + 7 * MS;
        shared.shown_later(at, Some(refresh));
        assert_eq!(shared.get().slot, Some(10 * MS + frame));
        assert_eq!(shared.get().last(), Some(at));
        shared.shown_later(at + frame + 7 * MS, None);
        assert_eq!(shared.get().slot, Some(at + frame + 7 * MS));
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
