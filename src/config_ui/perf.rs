//! The Stats page: the frames a second the program draws and the host's
//! CPU use in big digits, each in a panel with a graph of its last half
//! minute, and the emulated CPU's numbers below them. And the performance
//! overlay: the two numbers and small graphs of them at the bottom right
//! of the picture while the window is closed (Ctrl+Shift+F12).

use super::draw::{self, BigFont, Grid, Layout, Rgb};
use super::{ConfigUi, Plot};
use crate::stats::StatsView;
use crate::video::Frame;

/// The colour the host's CPU use shows in: green, yellow from 75%, red
/// from 90%, where the emulator is close to not keeping up.
fn load_color(percent: f32) -> Rgb {
    if percent >= 90.0 {
        draw::ERROR
    } else if percent >= 75.0 {
        draw::KEY
    } else {
        draw::GOOD
    }
}

/// The top of the frames graph's scale: the display's refresh rate or the
/// most frames a second seen, whichever is more, up to the next ten.
fn fps_scale(view: &StatsView) -> f32 {
    let max = view.fps_history.iter().copied().fold(view.refresh_hz, f32::max).max(1.0);
    (max / 10.0).ceil() * 10.0
}

/// Bytes as KB or MB.
fn size(bytes: u64) -> String {
    if bytes < 1 << 20 { format!("{} KB", bytes.div_ceil(1024)) } else { format!("{:.1} MB", bytes as f32 / (1 << 20) as f32) }
}

/// The numbers under the panels: label, short label and value.
fn details(view: &StatsView) -> Vec<(&'static str, &'static str, String)> {
    let mut details = vec![
        ("Cycles", "Cycles", format!("{}/ms", view.cycles_per_ms)),
        ("MIPS", "MIPS", format!("{:.1}", view.mips)),
    ];
    if crate::dynrec::AVAILABLE {
        details.push(("Recompiled", "Recomp.", format!("{:.0}%", view.recompiled)));
    }
    details.push(("Halted", "Halted", format!("{:.0}%", view.halted)));
    details.push(("Render", "Render", format!("{:.1} ms", view.render_ms)));
    if crate::dynrec::AVAILABLE {
        details.push(("Code cache", "Code", size(view.code_bytes)));
    }
    details
}

/// One of the page's two panels.
struct Panel {
    title: &'static str,
    /// What the top border says on the right.
    note: String,
    value: String,
    color: Rgb,
    history: Vec<f32>,
    max: f32,
    graph: Rgb,
    /// How a value of the history shows beside the digits.
    unit: &'static str,
}

/// Draw the performance overlay at the bottom right of `frame`: the
/// frames a second and the host's CPU use, each over a small graph of its
/// last half minute, side by side on a panel like the window's, but half
/// as opaque, so the picture shows through.
fn draw_overlay(frame: &mut Frame, view: &StatsView) {
    let (width, height) = (frame.width as usize, frame.height as usize);
    let cell_h = if height >= 300 { 16 } else { 8 };
    let graph = (width / 64).clamp(8, 12);
    let (cols, rows) = (2 * graph + 3, 3);
    if cols * 8 + 16 > width || (rows + 1) * cell_h > height {
        return;
    }
    let layout = Layout { x: width - cols * 8 - 8, y: height - rows * cell_h - cell_h / 2, cell_h, cols, rows };
    let panels = [
        ("FPS", format!("{:.0}", view.fps), draw::GOOD, &view.fps_history, fps_scale(view), draw::GOOD),
        ("CPU", format!("{:.0}%", view.cpu_use), load_color(view.cpu_use), &view.cpu_history, 100.0, draw::KEY),
    ];
    let mut grid = Grid::new(cols, rows);
    for (i, (label, value, color, ..)) in panels.iter().enumerate() {
        let col = 1 + i * (graph + 1);
        grid.text(col, 0, label, draw::TEXT);
        grid.text(col + graph - value.len(), 0, value, *color);
    }
    draw::render_blended(&grid, &layout, frame, draw::PANEL_ALPHA / 2);
    for (i, (.., history, max, color)) in panels.into_iter().enumerate() {
        draw::plot(frame, &layout, (1 + i * (graph + 1), 1, graph, rows - 1), history, max, color, draw::OPAQUE / 2);
    }
}

impl ConfigUi {
    /// Show or hide the performance overlay. Returns whether it shows.
    pub fn toggle_overlay(&mut self) -> bool {
        self.overlay = !self.overlay;
        self.overlay
    }

    /// Whether the performance overlay is on: it shows while the window
    /// is closed, and needs the stats every frame (`set_stats`).
    pub fn overlay_shown(&self) -> bool {
        self.overlay
    }

    /// Ctrl+Shift+F12, or its hint on the Stats page: show or hide the
    /// performance overlay. Returns what to tell the user over the
    /// picture while the window is closed; open, its status line says it.
    pub fn overlay_key(&mut self) -> Option<&'static str> {
        let on = self.toggle_overlay();
        if self.open {
            self.info(if on {
                "The performance overlay shows while this window is closed"
            } else {
                "The performance overlay is off"
            });
            return None;
        }
        Some(if on { "Performance overlay on (Ctrl+Shift+F12 hides it)" } else { "Performance overlay off" })
    }

    /// Draw the performance overlay over `frame`, if it is on and the
    /// window closed.
    pub fn draw_overlay(&self, frame: &mut Frame) {
        if self.overlay
            && !self.open
            && let Some(view) = &self.stats
        {
            draw_overlay(frame, view);
        }
    }

    pub(super) fn draw_stats(&mut self, g: &mut Grid, content: std::ops::Range<usize>) {
        let cols = g.cols;
        let Some(view) = self.stats.clone() else {
            g.text(2, content.start, "Measuring...", draw::DIM);
            return;
        };
        let details = details(&view);
        let across = if cols >= 60 { 3 } else { 2 };
        let mut detail_rows = details.len().div_ceil(across);
        if content.len() < detail_rows + 5 {
            detail_rows = 0;
        }
        let gap = usize::from(detail_rows > 0 && content.len() >= detail_rows + 9);
        let panel_rows = content.len() - detail_rows - gap;

        // Side by side where they fit, else one above the other.
        let top = content.start;
        let side_by_side = cols >= 50;
        let (width, height, places) = if side_by_side {
            let width = (cols - 5) / 2;
            (width, panel_rows, [(2, top), (3 + width, top)])
        } else {
            let height = panel_rows / 2;
            (cols - 4, height, [(2, top), (2, top + height)])
        };
        let scale = fps_scale(&view);
        let panels = [
            Panel {
                title: "FPS",
                note: format!("{:.0} Hz display", view.refresh_hz),
                value: format!("{:.0}", view.fps),
                color: draw::GOOD,
                history: view.fps_history.clone(),
                max: scale,
                graph: draw::GOOD,
                unit: "",
            },
            Panel {
                title: "CPU",
                note: "of one host core".to_string(),
                value: format!("{:.0}%", view.cpu_use),
                color: load_color(view.cpu_use),
                history: view.cpu_history.clone(),
                max: 100.0,
                graph: draw::KEY,
                unit: "%",
            },
        ];
        let span = |max: &str| format!("30 s, 0-{}", max);
        let spans = [span(&format!("{:.0}", scale)), span("100%")];
        for ((panel, (col, row)), span) in panels.into_iter().zip(places).zip(spans) {
            self.draw_panel(g, panel, (col, row, width, height), &span);
        }

        // The details, `across` to a row: in columns of their own width
        // with the values lined up, or where that doesn't fit, with the
        // short labels and each column as wide as it needs.
        let first = content.end - detail_rows;
        let wide = (cols - 4) / across >= 22;
        let mut starts = vec![2; across];
        for column in 1..across {
            starts[column] = if wide {
                2 + column * (cols - 4) / across
            } else {
                let need = details.iter().skip(column - 1).step_by(across).map(|(_, short, value)| short.len() + 1 + value.len());
                starts[column - 1] + need.max().unwrap_or(0) + 2
            };
        }
        for (i, (label, short, value)) in details.iter().enumerate().take(detail_rows * across) {
            let (col, row) = (starts[i % across], first + i / across);
            let end = starts.get(i % across + 1).map_or(cols - 2, |&next| next - 1);
            let after = g.text_to(col, row, if wide { label } else { short }, draw::TEXT, end);
            let at = if wide { col + 11 } else { after + 1 };
            g.text_to(at, row, value, draw::BRIGHT, end);
        }
    }

    /// A panel in the cells `place` (column, row, width, height): a box
    /// with the title and the note in its top border and `span` in its
    /// bottom one, the value in big digits, the history's average, least
    /// and most beside them where there is room, and its graph below them,
    /// or beside them where there is no room below.
    fn draw_panel(&mut self, g: &mut Grid, panel: Panel, place: (usize, usize, usize, usize), span: &str) {
        let (col, row, width, height) = place;
        if width < 8 || height < 3 {
            return;
        }
        let (right, bottom) = (col + width - 1, row + height - 1);
        for x in col..=right {
            let (top_ch, bottom_ch) = match x {
                _ if x == col => (0xDA, 0xC0),
                _ if x == right => (0xBF, 0xD9),
                _ => (0xC4, 0xC4),
            };
            g.char(x, row, top_ch, draw::BORDER);
            g.char(x, bottom, bottom_ch, draw::BORDER);
        }
        for y in row + 1..bottom {
            g.char(col, y, 0xB3, draw::BORDER);
            g.char(right, y, 0xB3, draw::BORDER);
        }
        let title_end = g.text(col + 2, row, &format!(" {} ", panel.title), draw::BRIGHT);
        let in_border = |text: &str, after: usize| {
            let text = format!(" {} ", text);
            (after + 1 + text.len() < right).then(|| (right - 1 - text.len(), text))
        };
        if let Some((at, text)) = in_border(&panel.note, title_end) {
            g.text(at, row, &text, draw::DIM);
        }
        if let Some((at, text)) = in_border(span, col) {
            g.text(at, bottom, &text, draw::DIM);
        }

        // The biggest digits that leave room for the graph, below them or
        // beside them. Their room is kept for the widest value, "100%".
        let (inner, inner_rows) = (width - 4, height - 2);
        let reserve = |font: BigFont| font.width("100%");
        let fits = |font: BigFont| reserve(font) <= inner && font.height() <= inner_rows;
        let graph_room = |font: BigFont| inner_rows - font.height() >= 2 || inner - reserve(font) >= 10;
        let font = [BigFont::Large, BigFont::Small]
            .into_iter()
            .find(|&f| fits(f) && graph_room(f))
            .or_else(|| [BigFont::Large, BigFont::Small].into_iter().find(|&f| fits(f)));
        let (x, y) = (col + 2, row + 1);
        let Some(font) = font else {
            g.text_to(x, y, &panel.value, panel.color, right - 1);
            return;
        };
        let end = draw::big_text(g, x, y, &panel.value, font, panel.color);
        let below = inner_rows - font.height();
        if below >= 2 {
            self.plots.push(Plot {
                cells: (x, y + font.height(), inner, below),
                values: panel.history.clone(),
                max: panel.max,
                color: panel.graph,
            });
            // The last half minute's average, least and most, right of
            // the digits.
            if !panel.history.is_empty() {
                let average = panel.history.iter().sum::<f32>() / panel.history.len() as f32;
                let least = panel.history.iter().copied().fold(f32::MAX, f32::min);
                let most = panel.history.iter().copied().fold(0.0, f32::max);
                let lines = [("avg", average), ("min", least), ("max", most)];
                for (i, (label, value)) in lines.into_iter().enumerate().take(font.height()) {
                    let text = format!("{:.0}{}", value, panel.unit);
                    let at = (x + inner).saturating_sub(text.len() + label.len() + 1);
                    if at > end + 1 {
                        g.text(at, y + i, label, draw::DIM);
                        g.text(at + label.len() + 1, y + i, &text, draw::TEXT);
                    }
                }
            }
        } else if inner - reserve(font) >= 10 {
            let at = x + reserve(font) + 2;
            self.plots.push(Plot {
                cells: (at, y, x + inner - at, inner_rows),
                values: panel.history,
                max: panel.max,
                color: panel.graph,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_overlay_shows_at_the_bottom_right_while_the_window_is_closed() {
        let mut ui = ConfigUi::new();
        let view = StatsView { fps: 70.0, refresh_hz: 70.0, cpu_use: 95.0, fps_history: vec![70.0; 120], cpu_history: vec![95.0; 120], ..Default::default() };
        ui.set_stats(view);
        let lit = |frame: &Frame, x: usize, y: usize| frame.rgb[(y * frame.width as usize + x) * 3..][..3] != [0, 0, 0];
        let mut frame = Frame::new(640, 400);
        ui.draw_overlay(&mut frame);
        assert!(!lit(&frame, 600, 380), "off");

        assert_eq!(ui.overlay_key(), Some("Performance overlay on (Ctrl+Shift+F12 hides it)"));
        assert!(ui.overlay_shown());
        ui.draw_overlay(&mut frame);
        assert!(lit(&frame, 600, 380) && !lit(&frame, 400, 380) && !lit(&frame, 600, 300));
        let has = |frame: &Frame, c: Rgb| frame.rgb.chunks(3).any(|px| px == [c.0, c.1, c.2]);
        assert!(has(&frame, draw::GOOD) && has(&frame, draw::KEY) && has(&frame, draw::ERROR), "graphs, and the CPU use in red");

        // Half as opaque as the window: over white, its panel is lighter.
        let mut white = Frame::new(640, 400);
        white.rgb.fill(0xFF);
        ui.draw_overlay(&mut white);
        let mut window = Frame::new(640, 400);
        window.rgb.fill(0xFF);
        let layout = Layout { x: 0, y: 0, cell_h: 16, cols: 1, rows: 1 };
        draw::render(&Grid::new(1, 1), &layout, &mut window);
        let (overlay, panel) = (&white.rgb[(390 * 640 + 630) * 3..][..3], &window.rgb[..3]);
        assert!(overlay.iter().zip(panel).all(|(o, p)| o > p), "{:?} over {:?}", overlay, panel);

        // Not over the window, and not on a picture too small for it.
        ui.open = true;
        let mut frame = Frame::new(640, 400);
        ui.draw_overlay(&mut frame);
        assert!(!lit(&frame, 600, 380));
        assert_eq!(ui.overlay_key(), None, "the status line says it");
        assert!(!ui.overlay_shown());
        ui.open = false;
        ui.toggle_overlay();
        ui.draw_overlay(&mut Frame::new(100, 20));
        ui.draw_overlay(&mut Frame::new(320, 200));
    }

    #[test]
    fn the_cpu_use_shows_how_close_it_is_to_the_limit() {
        assert_eq!(load_color(40.0), draw::GOOD);
        assert_eq!(load_color(80.0), draw::KEY);
        assert_eq!(load_color(95.0), draw::ERROR);
    }

    #[test]
    fn the_frames_scale_takes_the_refresh_rate_or_more() {
        let mut view = StatsView { refresh_hz: 70.0, fps_history: vec![35.0], ..Default::default() };
        assert_eq!(fps_scale(&view), 70.0);
        view.fps_history.push(143.0);
        assert_eq!(fps_scale(&view), 150.0);
        assert_eq!(size(4096), "4 KB");
        assert_eq!(size(3 << 20), "3.0 MB");
    }
}
