//! What the settings window's Stats page shows: the frames per second the
//! program draws (`Bus::frames_drawn`), the display's refresh rate, the
//! emulated CPU's speed, and how much of the host's time the emulator
//! takes, now and over the last half minute. The frontends record each of
//! their frames here, with the page open or not.

use crate::bus::Bus;
use std::collections::VecDeque;
use std::time::Duration;

/// How long a sample averages over.
const WINDOW: Duration = Duration::from_millis(250);
/// The samples the graphs keep: half a minute of them.
pub const HISTORY: usize = 120;

/// One of the frontend's frames: how long it was, how long the emulator
/// worked in it (running the machine and drawing the picture, not waiting
/// for the next frame), how long of that drawing the picture took, and
/// the instructions it ran, and whether the dynamic recompiler ran them.
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameTimes {
    pub wall: Duration,
    pub busy: Duration,
    pub render: Duration,
    pub executed: u64,
    pub recompiler: bool,
}

/// The numbers and graphs of the Stats page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatsView {
    /// The frames the program draws a second, and the display's refresh
    /// rate.
    pub fps: f32,
    pub refresh_hz: f32,
    pub cycles_per_ms: u32,
    /// Emulated instructions a second, in millions, and whether the
    /// dynamic recompiler ran them (else the interpreter).
    pub mips: f32,
    pub recompiler: bool,
    /// The share of the host's time the emulator takes, in percent.
    pub cpu_use: f32,
    /// Drawing the picture, in ms a frame.
    pub render_ms: f32,
    /// The last half minute of `fps` and `cpu_use`, oldest first.
    pub fps_history: Vec<f32>,
    pub cpu_history: Vec<f32>,
}

/// The samples on their way, and the histories.
#[derive(Clone, Debug, Default)]
pub struct Stats {
    wall: Duration,
    busy: Duration,
    render: Duration,
    executed: u64,
    frames: u32,
    drawn: u64,
    last_drawn: Option<u64>,
    view: StatsView,
    fps_history: VecDeque<f32>,
    cpu_history: VecDeque<f32>,
}

impl Stats {
    pub fn new() -> Self {
        Self::default()
    }

    /// Take in a frontend's frame, the machine as it is after it.
    pub fn record(&mut self, bus: &Bus, times: FrameTimes) {
        let drawn = bus.frames_drawn;
        self.drawn += drawn - self.last_drawn.unwrap_or(drawn);
        self.last_drawn = Some(drawn);
        self.wall += times.wall;
        self.busy += times.busy;
        self.render += times.render;
        self.executed += times.executed;
        self.frames += 1;
        self.view.refresh_hz = bus.vga.peek_timing().hz() as f32;
        self.view.cycles_per_ms = bus.clock.cycles_per_ms();
        self.view.recompiler = times.recompiler;
        if self.wall < WINDOW {
            return;
        }
        let seconds = self.wall.as_secs_f32();
        self.view.fps = self.drawn as f32 / seconds;
        self.view.cpu_use = (self.busy.as_secs_f32() / seconds * 100.0).min(100.0);
        self.view.mips = self.executed as f32 / seconds / 1e6;
        self.view.render_ms = self.render.as_secs_f32() * 1000.0 / self.frames as f32;
        for (history, value) in [(&mut self.fps_history, self.view.fps), (&mut self.cpu_history, self.view.cpu_use)] {
            if history.len() == HISTORY {
                history.pop_front();
            }
            history.push_back(value);
        }
        self.wall = Duration::ZERO;
        self.busy = Duration::ZERO;
        self.render = Duration::ZERO;
        self.executed = 0;
        self.frames = 0;
        self.drawn = 0;
    }

    pub fn view(&self) -> StatsView {
        StatsView {
            fps_history: self.fps_history.iter().copied().collect(),
            cpu_history: self.cpu_history.iter().copied().collect(),
            ..self.view.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn frame(ms: u64, busy_ms: u64) -> FrameTimes {
        FrameTimes {
            wall: Duration::from_millis(ms),
            busy: Duration::from_millis(busy_ms),
            render: Duration::from_micros(500),
            executed: 100_000,
            recompiler: false,
        }
    }

    #[test]
    fn rates_come_from_a_quarter_second_of_frames() {
        let mut bus = Bus::new(PathBuf::from("."));
        let mut stats = Stats::new();
        stats.record(&bus, FrameTimes::default());
        // 25 frames of 10 ms, 5 of them busy, a program frame every other.
        for i in 0..25 {
            if i % 2 == 0 {
                bus.frames_drawn += 1;
            }
            stats.record(&bus, frame(10, 5));
        }
        let view = stats.view();
        assert!((view.fps - 13.0 / 0.25).abs() < 1.0, "{:?}", view);
        assert!((view.cpu_use - 50.0).abs() < 0.1);
        assert!((view.mips - 10.0).abs() < 0.5);
        assert!((view.render_ms - 0.5).abs() < 0.05);
        assert!(view.refresh_hz > 69.0 && view.refresh_hz < 71.0, "text mode at 70 Hz");
        assert_eq!((view.fps_history.len(), view.cpu_history.len()), (1, 1));
    }

    #[test]
    fn the_histories_keep_half_a_minute() {
        let bus = Bus::new(PathBuf::from("."));
        let mut stats = Stats::new();
        for _ in 0..HISTORY * 2 {
            stats.record(&bus, frame(250, 100));
        }
        assert_eq!(stats.view().cpu_history.len(), HISTORY);
        assert!(stats.view().cpu_history.iter().all(|&c| (c - 40.0).abs() < 0.1));
    }
}
