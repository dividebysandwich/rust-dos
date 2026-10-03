//! The ways a game can start, chosen as it is launched: its package's
//! launch configurations (games::LaunchChoices). The first row starts it
//! with the options of its categories, chosen on the rows under it with
//! Left and Right; the rows after it start the other launch
//! configurations, its tools among them.

use super::draw::{self, Grid};
use super::{ConfigUi, Hit, Host, Target, UiKey, fit};
use crate::games::LaunchChoices;

/// A row of the chooser.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
    Start,
    Category(usize),
    Entry(usize),
}

pub(super) struct Chooser {
    id: String,
    choices: LaunchChoices,
    /// The option chosen in each category.
    picks: Vec<usize>,
    rows: Vec<Row>,
    row: usize,
}

impl Chooser {
    fn new(id: &str, choices: LaunchChoices) -> Self {
        let mut rows = vec![Row::Start];
        rows.extend((0..choices.variants.categories.len()).map(Row::Category));
        rows.extend((0..choices.variants.entries.len()).map(Row::Entry));
        let picks = vec![0; choices.variants.categories.len()];
        Self { id: id.to_string(), choices, picks, rows, row: 0 }
    }

    /// Whether it has options to choose with Left and Right.
    pub(super) fn has_categories(&self) -> bool {
        !self.choices.variants.categories.is_empty()
    }

    /// Select row `i`: whether it was selected already.
    pub(super) fn select(&mut self, i: usize) -> bool {
        let was = self.row == i;
        self.row = i.min(self.rows.len() - 1);
        was
    }

    /// The launch configuration the row starts (None: the default), and
    /// whether it is a tool.
    fn chosen(&self) -> (Option<String>, bool) {
        let variant = match self.rows[self.row] {
            Row::Start | Row::Category(_) => self.choices.variants.combination(&self.picks),
            Row::Entry(i) => Some(self.choices.variants.entries[i].dir.clone()),
        };
        let tool = variant.as_ref().is_some_and(|v| self.choices.tools.contains(v));
        (variant, tool)
    }
}

impl ConfigUi {
    /// Offer the ways to start the game `id`, if it has any: whether it
    /// does. The window is open.
    pub fn show_launch(&mut self, id: &str, host: &dyn Host) -> bool {
        let Some(choices) = host.launch_choices(id) else { return false };
        self.chooser = Some(Chooser::new(id, choices));
        self.status = None;
        true
    }

    pub(super) fn chooser_key(&mut self, key: UiKey, host: &mut dyn Host) {
        let Some(chooser) = &mut self.chooser else { return };
        match key {
            UiKey::Esc => {
                self.chooser = None;
                self.close();
            }
            UiKey::Left | UiKey::Right => {
                if let Row::Category(k) = chooser.rows[chooser.row] {
                    let count = chooser.choices.variants.categories[k].len();
                    let step = if key == UiKey::Right { 1 } else { count - 1 };
                    chooser.picks[k] = (chooser.picks[k] + step) % count;
                }
            }
            UiKey::Enter => {
                let (variant, tool) = chooser.chosen();
                let id = chooser.id.clone();
                match host.launch_variant(&id, variant.as_deref(), tool) {
                    Ok(message) => {
                        self.chooser = None;
                        self.close();
                        self.notice = Some(message);
                    }
                    Err(e) => self.error(e),
                }
            }
            _ => {
                if let Some(row) = Self::navigate(key, chooser.row, chooser.rows.len(), 10) {
                    chooser.row = row;
                }
            }
        }
    }

    pub(super) fn draw_chooser(&mut self, g: &mut Grid, content: std::ops::Range<usize>) {
        let Some(chooser) = &self.chooser else { return };
        let cols = g.cols;
        let top = content.start;
        g.text_to(3, top, &fit(&format!("Start {}", chooser.choices.name), cols - 6), draw::BRIGHT, cols - 3);
        let variants = &chooser.choices.variants;
        let mut hits = Vec::new();
        let mut row = top + 2;
        for (i, kind) in chooser.rows.iter().enumerate() {
            if row >= content.end {
                break;
            }
            // A gap before the other ways to start it.
            if matches!(kind, Row::Entry(0)) {
                row += 1;
                g.text(3, row, "Or start:", draw::DIM);
                row += 2;
            }
            if i == chooser.row {
                g.background(1, row, cols - 2, draw::SELECT);
            }
            hits.push(Hit { row, col: 1, width: cols - 2, target: Target::Row(i) });
            match *kind {
                Row::Start => {
                    let label = if variants.categories.is_empty() { "The game" } else { "The game, with:" };
                    g.text(5, row, label, draw::BRIGHT);
                }
                Row::Category(k) => {
                    let option = &variants.categories[k][chooser.picks[k]];
                    g.char(7, row, 0x11, draw::KEY);
                    hits.push(Hit { row, col: 7, width: 1, target: Target::Step(i, -1) });
                    let after = g.text_to(9, row, &fit(option, cols - 14), draw::BRIGHT, cols - 4);
                    g.char(after + 1, row, 0x10, draw::KEY);
                    hits.push(Hit { row, col: after + 1, width: 1, target: Target::Step(i, 1) });
                }
                Row::Entry(e) => {
                    let entry = &variants.entries[e];
                    let end = g.text_to(5, row, &fit(&entry.name, cols - 18), draw::BRIGHT, cols - 12);
                    if chooser.choices.tools.contains(&entry.dir) {
                        g.text(end + 2, row, "tool", draw::DIM);
                    }
                }
            }
            row += 1;
        }
        self.hits.extend(hits);
    }

    /// A click on the chooser's row `i`, or on one of its arrows.
    pub(super) fn chooser_clicked(&mut self, target: Target, host: &mut dyn Host) -> bool {
        let Some(chooser) = &mut self.chooser else { return false };
        match target {
            Target::Row(i) => {
                if chooser.select(i) {
                    self.chooser_key(UiKey::Enter, host);
                }
            }
            Target::Step(i, dir) => {
                chooser.select(i);
                self.chooser_key(if dir < 0 { UiKey::Left } else { UiKey::Right }, host);
            }
            Target::Key(key) => self.key(key, host),
            _ => {}
        }
        true
    }
}
