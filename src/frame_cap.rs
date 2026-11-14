//! The frame rate cap (`fps_cap`): the most frames a second a program
//! may show, whichever card shows them. Each card's or API's present
//! asks `FrameCap::wait` when the frame it is about to show would show,
//! and waits in its own way until the time it answers: the 3dfx card
//! holds its swap and the program with it (`Voodoo::swap`), a thread of
//! the built-in Windows blocks as it does for a retrace and calls again
//! (`win32::pace`). Once the frame shows, the present tells the cap
//! (`shown`).
//!
//! The cap's frames are the multiples of its frame time (1/30 s at 30)
//! in emulated time. A frame may show in a frame time after the one the
//! last frame counts in, so a program that draws faster than the cap
//! waits for the start of the next one, and its frames come at the
//! cap's frame times; one that draws slower shows each frame as it is
//! done. A frame that comes late counts in the first frame time it
//! missed, or a quarter of a second before it showed, and the frames
//! after it show at once until they are back in step (`shown`). The
//! window, with `vrr`, refreshes at the cap's frame times while a
//! program shows its frames through it (`refresh_timing`).
//!
//! A present that waits for the display's retrace already shows no more
//! frames than the display does: with the cap at or above the display's
//! rate, the cap holds none of its frames, and below it the frame shows
//! at the retrace nearest its turn (`wait`).
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

/// `fps_cap` when the configuration doesn't set it: the rate of most
/// displays. A program slower than it keeps its own rate.
pub const DEFAULT_FPS: Option<u32> = Some(60);

/// A value of `fps_cap`: Some(None) for `off` (or 0), Some(Some(fps))
/// for a rate of `RATES`, None for anything else.
pub fn parse(value: &str) -> Option<Option<u32>> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("off") || value == "0" {
        return Some(None);
    }
    value
        .parse()
        .ok()
        .filter(|fps| RATES.contains(fps))
        .map(Some)
}

/// `fps_cap`'s value as the configuration file writes it.
pub fn name(fps: Option<u32>) -> String {
    fps.map_or("off".to_string(), |fps| fps.to_string())
}

/// How long after its last frame a program counts as still showing
/// frames through the cap, in ns: the window refreshes at the cap's
/// frame times until then.
const ACTIVE_NS: u64 = 1_000_000_000;

/// How far a late frame lets the frames after it catch up, in ns: they
/// show at once, in the frame times it missed, up to this long before
/// it. Without it, a program whose frames are uneven loses a turn to
/// each long frame and stays below the cap though it could keep it:
/// Grom's menu at 150,000 cycles draws 71 frames a second without the
/// cap, most in 2.6 ms and about 23 a second with a 35 ms step of its
/// simulation, up to five steps (190 ms) in one. Under a cap of 60 it
/// showed 55 a second with a tenth of a second of catch-up and 60 with
/// a quarter, which covers its longest frames.
const CATCH_UP_NS: u64 = 250_000_000;

/// The cap and the frames it let through.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameCap {
    /// The most frames a second, None for no cap: the setting's, which a
    /// save state doesn't keep.
    fps: Option<u32>,
    /// When the last frame showed, in emulated ns; None before the
    /// first.
    last: Option<u64>,
    /// The frame time the last frame counts in (`shown`), in emulated
    /// ns: the next frame may show in the one after it.
    slot: Option<u64>,
}

impl FrameCap {
    /// `fps_cap`: None (or 0) for no cap.
    pub fn set_fps(&mut self, fps: Option<u32>) {
        self.fps = fps.filter(|&fps| fps > 0);
    }

    pub fn fps(&self) -> Option<u32> {
        self.fps
    }

    /// When the last frame showed, in emulated ns.
    pub fn last(&self) -> Option<u64> {
        self.last
    }

    /// The cap's frame time in ns, if there is a cap.
    pub fn frame_ns(&self) -> Option<u64> {
        self.fps.map(|fps| 1_000_000_000 / u64::from(fps))
    }

    /// A frame that would show at `at` (emulated ns): None if it may,
    /// or the time its present waits for, after which the frame shows
    /// (at the first retrace after it, for a present that waits for
    /// one). `refresh_ns` is the display's frame time for a present that
    /// waits for its retrace, which the cap doesn't hold when its own
    /// frame time is no longer.
    ///
    /// Such a present shows at the retrace nearest its turn, which may
    /// come up to half a retrace before it: the display's rate is never
    /// exactly a multiple of the cap's, and a retrace a little before the
    /// turn would otherwise wait for the next one, a frame shown every
    /// third retrace in place of every second (a cap of 30 on a display
    /// of 60.00006 Hz showed 25 frames a second).
    pub fn wait(&self, at: u64, refresh_ns: Option<u64>) -> Option<u64> {
        let frame = self.frame_ns()?;
        if refresh_ns.is_some_and(|refresh| frame <= refresh) {
            return None;
        }
        let slack = refresh_ns.map_or(0, |refresh| refresh / 2);
        let next = ((self.slot? + slack) / frame + 1) * frame - slack;
        (at < next).then_some(next)
    }

    /// A frame showed at `at`. A frame later than a frame time after the
    /// last counts in the frame time after the last one's, or no more
    /// than `CATCH_UP_NS` before it showed: the frames after it may show
    /// at once until they are back in step.
    pub fn shown(&mut self, at: u64) {
        self.last = Some(at);
        self.slot = Some(match (self.slot, self.frame_ns()) {
            (Some(slot), Some(frame)) if at > slot + frame => {
                (slot + frame).max(at.saturating_sub(CATCH_UP_NS))
            }
            _ => at,
        });
    }

    /// The steady timing of the cap's frames, a retrace at every
    /// multiple of its frame time, while a program shows its frames
    /// through it (one in the last second before `now`): the window,
    /// with `vrr`, refreshes at those times.
    pub fn refresh_timing(&self, now: u64) -> Option<CrtTiming> {
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

    pub fn set_fps(&self, fps: Option<u32>) {
        self.update(|cap| cap.set_fps(fps));
    }

    pub fn shown(&self, at: u64) {
        self.update(|cap| cap.shown(at));
    }

    pub fn wait(&self, at: u64, refresh_ns: Option<u64>) -> Option<u64> {
        self.get().wait(at, refresh_ns)
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
        cap.set_fps(Some(fps));
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
        assert_eq!(cap.wait(12 * MS, None), Some(50 * MS));
        assert_eq!(cap.wait(49 * MS, None), Some(50 * MS));
        assert_eq!(cap.wait(50 * MS, None), None);
    }

    #[test]
    fn a_late_frame_shows_when_it_comes() {
        let mut cap = cap(20);
        cap.shown(50 * MS);
        assert_eq!(cap.wait(120 * MS, None), None);
        cap.shown(120 * MS);
        // In the frame time of 100 ms to 150 ms: the next at 150 ms.
        assert_eq!(cap.wait(130 * MS, None), Some(150 * MS));
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
        assert!(
            shown
                .windows(2)
                .skip(1)
                .all(|w| w[1] - w[0] == 1_000_000_000 / 30)
        );
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
        // The first frames come as soon as they are drawn, one in each
        // frame time; from the fourth on each waits for its turn.
        assert!(
            shown[3..].windows(2).all(|w| w[1] - w[0] == frame),
            "{:?}",
            &shown[..6]
        );
        assert!((rate(&shown) - 60.0).abs() < 0.5, "{}", rate(&shown));
    }

    #[test]
    fn a_late_frame_lets_the_next_ones_catch_up() {
        // Nine frames of a fifth of a frame time, then one of four: the
        // late one's missed frame times go to the fast ones after it, 60
        // a second; letting none catch up held every fast one and gave 46.
        let frame = 1_000_000_000 / 60;
        let mut work = vec![frame / 5; 9];
        work.push(frame * 4);
        let shown = presented(60, &work, 601);
        assert!((rate(&shown) - 60.0).abs() < 0.5, "{}", rate(&shown));
    }

    #[test]
    fn a_program_with_long_frames_now_and_then_gets_the_cap() {
        // Like Grom's menu at 150,000 cycles: frames of 2.6 ms, a 35 ms
        // step of its simulation in one frame in three, and once a second
        // five steps in one frame. Uncapped that is 61 frames a second,
        // and a cap of 60 shows 60.
        let ms = 1_000_000;
        let mut work = Vec::new();
        for i in 0..23 {
            work.extend([2_600_000, 2_600_000]);
            work.push(if i == 11 { 190 * ms } else { 37 * ms });
        }
        let shown = presented(60, &work, 1200);
        assert!(rate(&shown) > 59.5, "{}", rate(&shown));
        // No second shows more than the cap and the frames it caught up.
        let late = shown.windows(2).filter(|w| w[1] - w[0] > 100 * ms).count();
        assert!(late > 0);
        for (i, &t) in shown.iter().enumerate() {
            let in_second = shown[i..]
                .iter()
                .take_while(|&&u| u < t + 1_000_000_000)
                .count();
            assert!(
                in_second <= 60 + 15,
                "{} frames in the second from {} ns",
                in_second,
                t
            );
        }
    }

    #[test]
    fn a_frame_a_second_late_lets_a_quarter_of_a_second_catch_up() {
        let frame = 1_000_000_000 / 60;
        let mut cap = cap(60);
        cap.shown(frame);
        let late = 1_000_000_000 + frame;
        cap.shown(late);
        // It counts a quarter of a second before it showed, at 0.767 s:
        // the 15 frames after it show at once, in the frame times up to
        // the one it showed in, and the next waits.
        let mut now = late;
        let mut at_once = 0;
        while cap.wait(now + 1000, None).is_none() {
            now += 1000;
            cap.shown(now);
            at_once += 1;
        }
        assert_eq!(at_once, 15);
    }

    #[test]
    fn a_retrace_a_little_faster_than_twice_the_cap_shows_every_other_one() {
        // A display of 60.00006 Hz under a cap of 30: every frame waits for
        // the retrace, and shows at the one nearest its turn. The first
        // two come as they are drawn, in two frame times of the cap.
        let refresh = 16_666_650;
        let shown = at_retraces(30, refresh, 61);
        assert!(
            shown[1..].windows(2).all(|w| w[1] - w[0] == 2 * refresh),
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
    fn fast_and_slow_frames_in_turn_average_out() {
        // Half a frame time, then one and a half: on average the cap's,
        // and the cap holds only the first fast frame.
        let frame = 1_000_000_000 / 30;
        let shown = presented(30, &[frame / 2, frame * 3 / 2], 61);
        assert!((rate(&shown) - 30.0).abs() < 0.5, "{}", rate(&shown));
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

    /// The times frames show at under a cap of `fps` when each waits for
    /// a retrace of the display of frame time `refresh`, drawn at once.
    fn at_retraces(fps: u32, refresh: u64, frames: usize) -> Vec<u64> {
        let mut cap = cap(fps);
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
    fn no_cap_holds_nothing() {
        let mut cap = FrameCap::default();
        cap.shown(10 * MS);
        assert_eq!(cap.wait(10 * MS, None), None);
        cap.set_fps(Some(0));
        assert_eq!(cap.fps(), None);
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
        shared.set_fps(Some(30));
        shared.shown(123);
        // Late: it counts a quarter of a second before it showed.
        shared.shown(500 * MS);
        let mut w = Writer::new();
        shared.save(&mut w);
        let mut other = Shared::default();
        other.set_fps(Some(50));
        other.load(&mut Reader::new(&w.buf)).unwrap();
        assert_eq!(other.get().last(), Some(500 * MS));
        assert_eq!(other.get().fps(), Some(50));
        assert_eq!(other.get().slot, shared.get().slot);
        assert_ne!(other.get().slot, other.get().last());
    }
}
