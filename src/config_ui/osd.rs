//! The on-screen message: a line at the top of the picture for what the
//! hotkeys did ("Paused", "CPU speed 20000", "Sound off"), drawn like the
//! settings window over the picture. A message lasts a moment; one that
//! stays, such as "Paused", shows until it is cleared, under the passing
//! ones.
//!
//! Notifications, such as an achievement unlocked, are two lines at the
//! bottom, one after another; and the values of the leaderboards being
//! attempted show in the bottom right corner.

use super::draw::{self, Grid, Layout};
use crate::video::Frame;
use web_time::{Duration, Instant};

/// How long a passing message shows.
const SHOWN: Duration = Duration::from_millis(2000);
/// How long a notification shows, and a big one.
const NOTIFIED: Duration = Duration::from_millis(3500);
const NOTIFIED_BIG: Duration = Duration::from_millis(6000);

#[derive(Default)]
pub struct Osd {
    /// A message that shows until `clear_lasting`.
    lasting: Option<String>,
    /// A message that shows until the time.
    passing: Option<(String, Instant)>,
    /// Notifications waiting, and the one showing until its time.
    queued: std::collections::VecDeque<(String, String, bool)>,
    notice: Option<(String, String, Instant)>,
    /// What shows in the bottom right corner.
    corner: Vec<String>,
}

impl Osd {
    pub fn new() -> Self {
        Self::default()
    }

    /// Show `text` for a moment.
    pub fn show(&mut self, text: impl Into<String>) {
        self.show_at(text, Instant::now());
    }

    fn show_at(&mut self, text: impl Into<String>, now: Instant) {
        self.passing = Some((text.into(), now + SHOWN));
    }

    /// Show `text` until `clear_lasting`.
    pub fn show_lasting(&mut self, text: impl Into<String>) {
        self.lasting = Some(text.into());
    }

    pub fn clear_lasting(&mut self) {
        self.lasting = None;
    }

    /// A notification: `title` over `detail`, shown longer if `big`.
    pub fn notify(&mut self, title: impl Into<String>, detail: impl Into<String>, big: bool) {
        self.queued.push_back((title.into(), detail.into(), big));
    }

    /// The lines of the bottom right corner (leaderboard values).
    pub fn set_corner(&mut self, lines: Vec<String>) {
        self.corner = lines;
    }

    /// The notification to show at `now`, if any.
    fn notice_at(&mut self, now: Instant) -> Option<(&str, &str)> {
        if self.notice.as_ref().is_some_and(|(_, _, until)| now >= *until) {
            self.notice = None;
        }
        if self.notice.is_none()
            && let Some((title, detail, big)) = self.queued.pop_front()
        {
            self.notice = Some((title, detail, now + if big { NOTIFIED_BIG } else { NOTIFIED }));
        }
        self.notice.as_ref().map(|(t, d, _)| (t.as_str(), d.as_str()))
    }

    /// The message to show at `now`, if any.
    fn text_at(&mut self, now: Instant) -> Option<&str> {
        if self.passing.as_ref().is_some_and(|(_, until)| now >= *until) {
            self.passing = None;
        }
        self.passing.as_ref().map(|(text, _)| text.as_str()).or(self.lasting.as_deref())
    }

    /// Draw the message, if there is one, over `frame`.
    pub fn draw(&mut self, frame: &mut Frame) {
        self.draw_at(frame, Instant::now());
    }

    fn draw_at(&mut self, frame: &mut Frame, now: Instant) {
        self.draw_notice(frame, now);
        self.draw_corner(frame);
        let Some(text) = self.text_at(now) else { return };
        let (width, height) = (frame.width as usize, frame.height as usize);
        let cols = (text.chars().count() + 2).min(width / 8);
        if cols < 3 {
            return;
        }
        let cell_h = if height >= 300 { 16 } else { 8 };
        let layout = Layout { x: (width - cols * 8) / 2, y: cell_h / 2, cell_h, cols, rows: 1 };
        let mut grid = Grid::new(cols, 1);
        grid.text_to(1, 0, text, draw::BRIGHT, cols - 1);
        draw::render(&grid, &layout, frame);
    }

    fn draw_notice(&mut self, frame: &mut Frame, now: Instant) {
        let (width, height) = (frame.width as usize, frame.height as usize);
        let Some((title, detail)) = self.notice_at(now) else { return };
        let len = title.chars().count().max(detail.chars().count());
        let cols = (len + 4).min(width / 8);
        let cell_h = if height >= 300 { 16 } else { 8 };
        if cols < 3 || height < cell_h * 4 {
            return;
        }
        let layout = Layout { x: (width - cols * 8) / 2, y: height - cell_h * 7 / 2, cell_h, cols, rows: 2 };
        let mut grid = Grid::new(cols, 2);
        grid.text_to(2, 0, title, draw::KEY, cols - 1);
        grid.text_to(2, 1, detail, draw::TEXT, cols - 1);
        draw::render(&grid, &layout, frame);
    }

    fn draw_corner(&mut self, frame: &mut Frame) {
        let (width, height) = (frame.width as usize, frame.height as usize);
        let cell_h = if height >= 300 { 16 } else { 8 };
        for (i, line) in self.corner.iter().rev().enumerate() {
            let cols = (line.chars().count() + 2).min(width / 8);
            let y = height.saturating_sub(cell_h * (i + 1) + cell_h / 2);
            if cols < 3 || y < cell_h {
                break;
            }
            let layout = Layout { x: width - cols * 8 - 8, y, cell_h, cols, rows: 1 };
            let mut grid = Grid::new(cols, 1);
            grid.text_to(1, 0, line, draw::BRIGHT, cols - 1);
            draw::render(&grid, &layout, frame);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(frame: &Frame) -> bool {
        frame.rgb.iter().any(|&b| b != 0)
    }

    #[test]
    fn a_message_shows_for_a_moment_over_the_lasting_one() {
        let mut osd = Osd::new();
        let start = Instant::now();
        let mut frame = Frame::new(640, 400);
        osd.draw_at(&mut frame, start);
        assert!(!lit(&frame), "nothing to show");

        osd.show_lasting("Paused");
        osd.show_at("CPU speed 20000", start);
        assert_eq!(osd.text_at(start), Some("CPU speed 20000"));
        assert_eq!(osd.text_at(start + SHOWN), Some("Paused"));
        osd.clear_lasting();
        assert_eq!(osd.text_at(start + SHOWN), None);

        osd.show_at("Sound off", start);
        osd.draw_at(&mut frame, start);
        assert!(lit(&frame));
        // At the top, centred.
        let row = |y: usize| frame.rgb[y * 640 * 3..(y + 1) * 640 * 3].iter().any(|&b| b != 0);
        assert!(row(10) && !row(100));
        // Even on a tiny picture.
        osd.draw_at(&mut Frame::new(16, 8), start);
    }

    #[test]
    fn notifications_show_one_after_another_at_the_bottom() {
        let mut osd = Osd::new();
        let start = Instant::now();
        osd.notify("Achievement unlocked: First", "Byte 0 is 1 (5 points)", true);
        osd.notify("Leaderboard started", "Fast", false);
        assert_eq!(osd.notice_at(start), Some(("Achievement unlocked: First", "Byte 0 is 1 (5 points)")));
        assert_eq!(osd.notice_at(start + NOTIFIED), Some(("Achievement unlocked: First", "Byte 0 is 1 (5 points)")));
        assert_eq!(osd.notice_at(start + NOTIFIED_BIG), Some(("Leaderboard started", "Fast")));
        assert_eq!(osd.notice_at(start + NOTIFIED_BIG + NOTIFIED), None);
        let mut frame = Frame::new(640, 400);
        osd.notify("A", "B", false);
        osd.set_corner(vec!["1:23.45".into()]);
        osd.draw_at(&mut frame, start);
        let row = |y: usize| frame.rgb[y * 640 * 3..(y + 1) * 640 * 3].iter().any(|&b| b != 0);
        assert!(row(350) && !row(100) && !row(10));
        osd.draw_at(&mut Frame::new(16, 8), start);
    }
}
