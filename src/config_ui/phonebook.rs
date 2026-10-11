//! The Serial page's phonebook, opened with Enter on *Phonebook*: the
//! numbers the modem dials bulletin boards with (serial/phonebook.rs), the
//! user's, rust-dos.com's and the built-in ones. Enter copies the command
//! that dials one, and the list from rust-dos.com is fetched only when the
//! user asks (`ConfigUi::phone_catalog`).

use super::draw::{self, Grid};
use super::rooms::wrap;
use super::{ConfigUi, Hit, Host, Target, UiKey, fit};
use crate::serial::phonebook::{self, Entry, Source};
use std::sync::mpsc::{Receiver, TryRecvError};

/// rust-dos.com's list: not asked for yet, being asked for, or what came.
#[derive(Default)]
pub(super) enum Catalog {
    #[default]
    NotAsked,
    Asking(Receiver<Result<Vec<Entry>, String>>),
    /// The boards that came, or why none did.
    Got(Result<usize, String>),
}

/// A line of the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Line {
    Heading(&'static str),
    Board(usize),
    Ask,
    Note(String),
}

impl Line {
    fn selectable(&self) -> bool {
        matches!(self, Line::Board(_) | Line::Ask)
    }
}

pub struct PhoneList {
    boards: Vec<(Entry, Source)>,
    pub(super) selected: usize,
    scroll: usize,
}

/// Whether this rust-dos fetches the list.
const FETCHES: bool = cfg!(all(feature = "sdl", not(target_arch = "wasm32")));

impl ConfigUi {
    /// Enter on *Phonebook*: the list, on its first board.
    pub(super) fn open_phonebook(&mut self) {
        self.status = None;
        self.phonebook = Some(PhoneList { boards: phonebook::entries(), selected: 1, scroll: 0 });
    }

    pub(super) fn phone_lines(&self, list: &PhoneList) -> Vec<Line> {
        let mut lines = vec![Line::Heading("Numbers to dial")];
        lines.extend((0..list.boards.len()).map(Line::Board));
        if FETCHES {
            lines.push(Line::Heading("On rust-dos.com"));
            lines.push(Line::Ask);
            match &self.phone_catalog {
                Catalog::Got(Ok(0)) => lines.push(Line::Note("It lists no boards yet".into())),
                Catalog::Got(Err(e)) => lines.push(Line::Note(e.clone())),
                _ => {}
            }
        }
        lines
    }

    /// rust-dos.com's list, as it comes.
    pub(super) fn poll_phonebook(&mut self) {
        let Catalog::Asking(result) = &self.phone_catalog else { return };
        let got = match result.try_recv() {
            Ok(got) => got,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("the question stopped".to_string()),
        };
        self.phone_catalog = Catalog::Got(got.map(|boards| {
            let count = boards.len();
            phonebook::set_website(boards);
            count
        }));
        if let Some(list) = &mut self.phonebook {
            list.boards = phonebook::entries();
        }
    }

    /// Ask rust-dos.com for its boards, on a thread of its own.
    fn ask_for_boards(&mut self) {
        if matches!(self.phone_catalog, Catalog::Asking(_)) {
            return;
        }
        let (done, result) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new().name("phonebook".to_string()).spawn(move || {
            let _ = done.send(phonebook::fetch_catalog());
        });
        match spawned {
            Ok(_) => {
                self.phone_catalog = Catalog::Asking(result);
                self.status = None;
            }
            Err(e) => self.error(format!("Can't ask rust-dos.com: {}", e)),
        }
    }

    /// Whether a COM port has the modem.
    fn has_modem(&self) -> bool {
        self.settings.serial.ports.contains(&crate::serial::PortType::Modem)
    }

    pub(super) fn phonebook_key(&mut self, key: UiKey, host: &mut dyn Host) {
        let Some(list) = &self.phonebook else { return };
        let lines = self.phone_lines(list);
        let selected = list.selected.min(lines.len() - 1);
        match key {
            UiKey::Up | UiKey::Down | UiKey::PageUp | UiKey::PageDown | UiKey::Home | UiKey::End => {
                let page = self.visible.saturating_sub(8).max(1);
                let last = lines.len() - 1;
                let next = match key {
                    UiKey::Up => (0..selected).rev().find(|&i| lines[i].selectable()),
                    UiKey::Down => (selected + 1..=last).find(|&i| lines[i].selectable()),
                    UiKey::PageUp => (0..=selected.saturating_sub(page)).rev().find(|&i| lines[i].selectable()),
                    UiKey::PageDown => (selected..=(selected + page).min(last)).rev().find(|&i| lines[i].selectable()),
                    UiKey::Home => lines.iter().position(Line::selectable),
                    _ => lines.iter().rposition(Line::selectable),
                };
                if let (Some(next), Some(list)) = (next, &mut self.phonebook) {
                    list.selected = next;
                }
            }
            UiKey::Esc => {
                self.phonebook = None;
                self.status = None;
            }
            UiKey::Enter => match &lines[selected] {
                Line::Board(i) => {
                    let command = format!("ATDT {}", list.boards[*i].0.number);
                    match host.copy_text(&command) {
                        Ok(()) => self.info(format!("{} is copied: paste it into your terminal program", command)),
                        Err(_) => self.info(format!("Type {} in your terminal program", command)),
                    }
                }
                Line::Ask => self.ask_for_boards(),
                Line::Heading(_) | Line::Note(_) => {}
            },
            _ => {}
        }
    }

    /// A click on a line: the selected one is as Enter.
    pub(super) fn phone_clicked(&mut self, target: Target, host: &mut dyn Host) {
        let Target::PhoneRow(i) = target else { return };
        let Some(list) = &mut self.phonebook else { return };
        if list.selected == i {
            self.key(UiKey::Enter, host);
        } else {
            list.selected = i;
        }
    }

    pub(super) fn phone_hints(&self) -> Vec<(&'static str, &'static str, UiKey)> {
        let Some(list) = &self.phonebook else { return Vec::new() };
        let lines = self.phone_lines(list);
        let mut hints = match lines.get(list.selected) {
            Some(Line::Board(_)) => vec![("Enter", "Copy ATDT", UiKey::Enter)],
            Some(Line::Ask) => vec![("Enter", "Ask", UiKey::Enter)],
            _ => Vec::new(),
        };
        hints.push(("Esc", "Back", UiKey::Esc));
        hints
    }

    pub(super) fn draw_phonebook(&mut self, g: &mut Grid, content: std::ops::Range<usize>) {
        let Some(list) = &self.phonebook else { return };
        let lines = self.phone_lines(list);
        let cols = g.cols;
        let asking = matches!(self.phone_catalog, Catalog::Asking(_));
        let asked = matches!(self.phone_catalog, Catalog::Got(Ok(_)));
        let modem = self.has_modem();
        let Some(list) = &mut self.phonebook else { return };
        list.selected = list.selected.min(lines.len() - 1);
        let top = content.start;
        g.text_to(2, top, "Modem Phonebook", draw::BRIGHT, cols - 2);

        // What the line selected is, at the bottom.
        let about_rows = 4;
        let about_top = content.end.saturating_sub(about_rows);
        let about = match &lines[list.selected] {
            _ if !modem => "No COM port has the modem: set one to modem to dial these.".to_string(),
            Line::Board(i) => {
                let (entry, source) = &list.boards[*i];
                let from = match source {
                    Source::Yours => "From [phonebook] in your configuration file.",
                    Source::Website => "From rust-dos.com's list.",
                    Source::BuiltIn => "Built into Rust-DOS.",
                };
                let how = if entry.telnet { "by telnet" } else { "as plain TCP" };
                format!("ATDT {} calls {} {}. {}", entry.number, entry.address, how, from)
            }
            Line::Ask => format!("Fetch a list of boards from {}. No other information is shared.", phonebook::CATALOG_URL),
            _ => String::new(),
        };
        let about_color = if modem { draw::DIM } else { draw::ERROR };
        for (line, row) in wrap(about.trim(), cols - 4, about_rows).into_iter().zip(about_top..content.end) {
            g.text_to(2, row, &line, about_color, cols - 2);
        }

        let rows = top + 2..about_top.saturating_sub(1);
        Self::keep_visible(&mut list.scroll, list.selected, rows.len());
        let number_col = cols.saturating_sub(16);
        let mut hits = Vec::new();
        for (i, row) in (list.scroll..lines.len()).zip(rows.clone()) {
            let selected = i == list.selected;
            if selected {
                g.background(1, row, cols - 2, draw::SELECT);
            }
            if lines[i].selectable() {
                hits.push(Hit { row, col: 1, width: cols - 2, target: Target::PhoneRow(i) });
            }
            let (text, color, number) = match &lines[i] {
                Line::Heading(title) => (title.to_string(), draw::NOTE, String::new()),
                Line::Board(b) => {
                    let (entry, _) = &list.boards[*b];
                    (format!("  {}", entry.name), draw::BRIGHT, entry.number.clone())
                }
                Line::Ask if asking => ("  Fetching the list...".to_string(), draw::DIM, String::new()),
                Line::Ask if asked => ("  Refresh".to_string(), draw::KEY, String::new()),
                Line::Ask => ("  Fetch more boards...".to_string(), draw::KEY, String::new()),
                Line::Note(text) => (format!("  {}", text), draw::ERROR, String::new()),
            };
            let color = if selected && lines[i].selectable() { draw::BRIGHT } else { color };
            g.text_to(2, row, &fit(&text, number_col.saturating_sub(3)), color, number_col.saturating_sub(1));
            g.text_to(number_col, row, &number, draw::TEXT, cols - 2);
        }
        let (scroll, total) = (list.scroll, lines.len());
        self.hits.extend(hits);
        self.draw_scrollbar(g, rows, scroll, total);
    }
}
