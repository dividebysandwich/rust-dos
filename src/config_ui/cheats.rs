//! The settings window's Cheats page: searching memory for a game's
//! values, narrowing the addresses down as they change, and setting or
//! freezing them (see cheats.rs).

use super::dialog::TextField;
use super::draw::{self, Grid};
use super::{ConfigUi, Hit, Host, Target, UiKey};
use crate::cheats::{Area, Candidate, Freeze, Narrow, Search, Width, format_address, parse_value};

/// The most candidates the page lists.
const SHOWN: usize = 200;

/// How the narrowing compares, as the page offers it.
const NARROWS: [(&str, Option<Narrow>); 5] = [
    ("equal to", None),
    ("changed", Some(Narrow::Changed)),
    ("unchanged", Some(Narrow::Unchanged)),
    ("increased", Some(Narrow::Increased)),
    ("decreased", Some(Narrow::Decreased)),
];

/// A row of the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheatRow {
    Width,
    Area,
    Search,
    Narrow,
    Candidate(usize),
    Frozen(usize),
}

/// The page's state: the search, what it lists, and a value being typed.
pub struct Cheats {
    width: Width,
    area: Area,
    /// Which of `NARROWS`.
    narrow: usize,
    search: Option<Search>,
    shown: Vec<Candidate>,
    freezes: Vec<Freeze>,
    pub edit: Option<(CheatRow, TextField)>,
}

impl Default for Cheats {
    fn default() -> Self {
        Self { width: Width::Byte, area: Area::Conventional, narrow: 0, search: None, shown: Vec::new(), freezes: Vec::new(), edit: None }
    }
}

impl Cheats {
    pub fn rows(&self) -> Vec<CheatRow> {
        let mut rows = vec![CheatRow::Width, CheatRow::Area, CheatRow::Search, CheatRow::Narrow];
        rows.extend((0..self.shown.len()).map(CheatRow::Candidate));
        rows.extend((0..self.freezes.len()).map(CheatRow::Frozen));
        rows
    }

    /// What the page lists, as memory has it now.
    pub fn refresh(&mut self, host: &dyn Host) {
        let mem = host.memory();
        self.shown = self.search.as_ref().map_or_else(Vec::new, |s| s.candidates(mem, 0, SHOWN));
        self.freezes = host.freezes();
    }
}

impl ConfigUi {
    fn cheat_row(&self) -> Option<CheatRow> {
        self.cheats.rows().get(self.row).copied()
    }

    pub(super) fn cheats_key(&mut self, key: UiKey, host: &mut dyn Host) {
        let Some(row) = self.cheat_row() else { return };
        let cheats = &mut self.cheats;
        let dir = match key {
            UiKey::Left => -1,
            UiKey::Right => 1,
            _ => 0,
        };
        match (row, key) {
            (CheatRow::Width, UiKey::Left | UiKey::Right | UiKey::Enter) => {
                cheats.width = super::cycle(&Width::ALL, cheats.width, if dir == 0 { 1 } else { dir });
                // Another size is another search.
                cheats.search = None;
                cheats.refresh(host);
                self.status = None;
            }
            (CheatRow::Area, UiKey::Left | UiKey::Right | UiKey::Enter) => {
                cheats.area = super::cycle(&Area::ALL, cheats.area, if dir == 0 { 1 } else { dir });
            }
            (CheatRow::Search, UiKey::Enter) => cheats.edit = Some((row, TextField::default())),
            (CheatRow::Narrow, UiKey::Left | UiKey::Right) => {
                cheats.narrow = (cheats.narrow as isize + dir).rem_euclid(NARROWS.len() as isize) as usize;
            }
            (CheatRow::Narrow, UiKey::Enter) => match NARROWS[cheats.narrow].1 {
                None => cheats.edit = Some((row, TextField::default())),
                Some(how) => self.narrow(how, host),
            },
            (CheatRow::Candidate(i), UiKey::Enter) => {
                let value = cheats.shown[i].value.to_string();
                cheats.edit = Some((row, TextField::new(&value)));
            }
            (CheatRow::Candidate(i), UiKey::Insert) => {
                let candidate = cheats.shown[i];
                let mut freezes = host.freezes();
                freezes.retain(|f| f.addr != candidate.addr);
                freezes.push(Freeze { addr: candidate.addr, width: cheats.width, value: candidate.value });
                host.set_freezes(freezes);
                cheats.refresh(host);
                self.info(format!("{} stays at {} while this program runs", format_address(candidate.addr), candidate.value));
            }
            (CheatRow::Candidate(i), UiKey::Delete) => {
                let addr = cheats.shown[i].addr;
                if let Some(search) = &mut cheats.search {
                    search.remove(addr);
                }
                cheats.refresh(host);
                self.row = self.row.min(self.row_count().saturating_sub(1));
            }
            (CheatRow::Frozen(i), UiKey::Enter) => {
                let value = cheats.freezes[i].value.to_string();
                cheats.edit = Some((row, TextField::new(&value)));
            }
            (CheatRow::Frozen(i), UiKey::Delete) => {
                let mut freezes = host.freezes();
                if i < freezes.len() {
                    let freeze = freezes.remove(i);
                    self.info(format!("{} is free again", format_address(freeze.addr)));
                }
                host.set_freezes(freezes);
                self.cheats.refresh(host);
                self.row = self.row.min(self.row_count().saturating_sub(1));
            }
            _ => {}
        }
    }

    /// Keep the candidates that compare as `how` says.
    fn narrow(&mut self, how: Narrow, host: &mut dyn Host) {
        let Some(search) = &mut self.cheats.search else {
            return self.error("Start with a new search");
        };
        search.narrow(host.memory(), how);
        let count = search.count();
        self.cheats.refresh(host);
        self.info(match count {
            0 => "Nothing is left: start a new search".to_string(),
            1 => "One address is left: Enter sets it, Ins freezes it".to_string(),
            n => format!("{} addresses are left: play on and narrow them down", n),
        });
    }

    pub(super) fn cheats_edit_key(&mut self, key: UiKey, host: &mut dyn Host) {
        let Some((row, field)) = &mut self.cheats.edit else { return };
        if field.key(key) {
            return;
        }
        let row = *row;
        match key {
            UiKey::Esc => self.cheats.edit = None,
            UiKey::Enter => {
                let text = field.text();
                let width = self.cheats.width;
                let value = if text.trim().is_empty() && row == CheatRow::Search {
                    None
                } else {
                    match parse_value(&text, width) {
                        Ok(value) => Some(value),
                        Err(e) => return self.error(e),
                    }
                };
                self.cheats.edit = None;
                match (row, value) {
                    (CheatRow::Search, value) => {
                        let search = Search::new(host.memory(), width, self.cheats.area, value);
                        let count = search.count();
                        self.cheats.search = Some(search);
                        self.cheats.refresh(host);
                        self.info(match (value, count) {
                            (Some(v), 0) => format!("Nothing holds {}", v),
                            (Some(v), 1) => format!("One address holds {}: Enter sets it, Ins freezes it", v),
                            (Some(v), n) => format!("{} addresses hold {}: play on, then narrow them down", n, v),
                            (None, n) => format!("{} addresses: play on, then narrow them down by how the value changed", n),
                        });
                    }
                    (CheatRow::Narrow, Some(value)) => self.narrow(Narrow::Equal(value), host),
                    (CheatRow::Candidate(i), Some(value)) => {
                        let addr = self.cheats.shown[i].addr;
                        host.poke(addr, &width.bytes_of(value));
                        self.cheats.refresh(host);
                        self.info(format!("{} is {} now", format_address(addr), value));
                    }
                    (CheatRow::Frozen(i), Some(value)) => {
                        let mut freezes = host.freezes();
                        if let Some(freeze) = freezes.get_mut(i) {
                            freeze.value = value & width_mask(freeze.width);
                        }
                        host.set_freezes(freezes);
                        self.cheats.refresh(host);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    pub(super) fn draw_cheats(&mut self, g: &mut Grid, content: std::ops::Range<usize>) {
        let cols = g.cols;
        let rows = self.cheats.rows();
        Self::keep_visible(&mut self.scroll, self.row, content.len());
        let value_col = 27.min(cols / 2);
        let end = cols - 2;
        let cheats = &self.cheats;
        for (i, row) in (self.scroll..rows.len()).zip(content.clone()) {
            let selected = i == self.row;
            if selected {
                g.background(1, row, cols - 2, draw::SELECT);
            }
            self.hits.push(Hit { row, col: 1, width: cols - 2, target: Target::Row(i) });
            let label_color = if selected { draw::BRIGHT } else { draw::TEXT };
            let choice = |g: &mut Grid, text: &str, hits: &mut Vec<Hit>| {
                g.char(value_col, row, 0x11, draw::KEY);
                hits.push(Hit { row, col: value_col, width: 1, target: Target::Step(i, -1) });
                let after = g.text_to(value_col + 2, row, text, draw::BRIGHT, end);
                g.char(after + 1, row, 0x10, draw::KEY);
                hits.push(Hit { row, col: after + 1, width: 1, target: Target::Step(i, 1) });
                after + 3
            };
            let mut hits = Vec::new();
            match rows[i] {
                CheatRow::Width => {
                    g.text(2, row, "Value size", label_color);
                    choice(g, cheats.width.name(), &mut hits);
                }
                CheatRow::Area => {
                    g.text(2, row, "Search in", label_color);
                    choice(g, cheats.area.name(), &mut hits);
                }
                CheatRow::Search => {
                    g.text(2, row, "New search", label_color);
                    let note = match &cheats.search {
                        Some(search) => format!("{} found", search.count()),
                        None => "a value, or none for any".to_string(),
                    };
                    g.text_to(value_col + 2, row, &note, draw::DIM, end);
                }
                CheatRow::Narrow => {
                    g.text(2, row, "Narrow down", label_color);
                    choice(g, NARROWS[cheats.narrow].0, &mut hits);
                }
                CheatRow::Candidate(c) => {
                    let candidate = cheats.shown[c];
                    let changed = if candidate.value != candidate.previous {
                        format!("was {}", candidate.previous)
                    } else {
                        String::new()
                    };
                    g.text(4, row, &format_address(candidate.addr), label_color);
                    g.text(value_col, row, &format!("{:>10}  {:0w$X}h", candidate.value, candidate.value, w = cheats.width.bytes() * 2), draw::BRIGHT);
                    g.text_to(value_col + 26, row, &changed, draw::DIM, end);
                }
                CheatRow::Frozen(f) => {
                    let freeze = cheats.freezes[f];
                    g.text(2, row, "\u{0F}", draw::KEY);
                    g.text(4, row, &format_address(freeze.addr), label_color);
                    g.text(value_col, row, &format!("{:>10}  frozen", freeze.value), draw::GOOD);
                }
            }
            self.hits.extend(hits);
            // A value being typed, in place.
            if let Some((edited, field)) = &cheats.edit
                && *edited == rows[i]
            {
                let col = match rows[i] {
                    CheatRow::Narrow => value_col + NARROWS[cheats.narrow].0.len() + 6,
                    _ => value_col,
                };
                let width = end.saturating_sub(col).min(16);
                let (text, cursor) = field.view(width);
                g.background(col, row, width, draw::FIELD);
                g.text_to(col, row, &text, draw::BRIGHT, col + width);
                g.background(col + cursor, row, 1, draw::SELECT);
            }
        }
        self.draw_scrollbar(g, content, self.scroll, rows.len());
    }

    /// The key hints of the Cheats page's selected row.
    pub(super) fn cheats_hints(&self) -> Vec<(&'static str, &'static str, UiKey)> {
        use UiKey::*;
        let mut hints = match self.cheat_row() {
            Some(CheatRow::Width | CheatRow::Area) => vec![("\u{2190}\u{2192}", "Change", Right)],
            Some(CheatRow::Search) => vec![("Enter", "Search", Enter)],
            Some(CheatRow::Narrow) => vec![("\u{2190}\u{2192}", "Compare", Right), ("Enter", "Narrow", Enter)],
            Some(CheatRow::Candidate(_)) => vec![("Enter", "Set", Enter), ("Ins", "Freeze", Insert), ("Del", "Drop", Delete)],
            Some(CheatRow::Frozen(_)) => vec![("Enter", "Set", Enter), ("Del", "Unfreeze", Delete)],
            None => Vec::new(),
        };
        hints.extend([("Tab", "Page", Tab), ("Esc", "Close", Esc)]);
        hints
    }
}

fn width_mask(width: Width) -> u32 {
    match width {
        Width::Byte => 0xFF,
        Width::Word => 0xFFFF,
        Width::Dword => u32::MAX,
    }
}
