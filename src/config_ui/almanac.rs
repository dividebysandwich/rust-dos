//! The Emulator page's almanac (almanac.rs), opened with Enter on *Fetch
//! settings from Almanac* or *Report settings to Almanac*: the games
//! almanac.rust-dos.com has for the program started last (by its file's
//! name and checksums) and by the name typed, each with the settings it
//! recommends. Fetching, Enter puts the settings of the game picked in
//! place; reporting, Enter tells the almanac how the game played with the
//! settings as they are, to the game picked or to a new one by the name
//! typed. A game takes any number of reports; one from a game profile
//! replaces the one the profile sent on that game before (its `uuid=`).
//! Reporting, the game's year, publisher and the player's notes go too,
//! from the profile of the game playing, and are kept in it.

use super::dialog::TextField;
use super::draw::{self, Grid};
use super::rooms::wrap;
use super::{ConfigUi, Hit, Host, Target, UiKey, fit};
use crate::almanac::{self, About, Candidate, Program, Rating, Reported, Subject};
use std::sync::mpsc::{Receiver, TryRecvError};

/// Whether this rust-dos asks the almanac.
pub(super) const ASKS: bool = cfg!(all(feature = "sdl", not(target_arch = "wasm32")));

/// What a question to the almanac answered.
enum Answer {
    Candidates(Result<Vec<Candidate>, String>),
    /// A report sent: what the almanac made of it, the game's name, and
    /// why the game's profile couldn't keep what the report said of it.
    Reported(Result<Reported, String>, String, Option<String>),
}

/// What the report says of the game, besides its settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AboutField {
    Year,
    Publisher,
    Notes,
}

impl AboutField {
    const ALL: [AboutField; 3] = [AboutField::Year, AboutField::Publisher, AboutField::Notes];

    fn label(self) -> &'static str {
        match self {
            AboutField::Year => "Year",
            AboutField::Publisher => "Publisher",
            AboutField::Notes => "Notes",
        }
    }
}

/// A line of the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Line {
    Search,
    Rating,
    About(AboutField),
    Heading(&'static str),
    Game(usize),
    /// Reporting: a new game, by the name typed.
    NewGame,
    Note(String),
}

impl Line {
    fn selectable(&self) -> bool {
        matches!(
            self,
            Line::Search | Line::Rating | Line::About(_) | Line::Game(_) | Line::NewGame
        )
    }
}

pub struct AlmanacView {
    /// Reporting, or fetching settings.
    report: bool,
    /// The program started last, if one did.
    program: Option<Program>,
    search: TextField,
    rating: Rating,
    year: TextField,
    publisher: TextField,
    notes: TextField,
    candidates: Vec<Candidate>,
    asking: Option<Receiver<Answer>>,
    /// What the last question or report came to, and whether it failed.
    note: Option<(String, bool)>,
    pub(super) selected: usize,
    scroll: usize,
}

impl ConfigUi {
    /// Enter on *Fetch settings* (`report` false) or *Report settings*:
    /// the almanac's games for the program started last, being asked for.
    pub(super) fn open_almanac(&mut self, report: bool, host: &mut dyn Host) {
        self.status = None;
        let program = host.almanac_program();
        let name = program.as_ref().map_or(String::new(), |p| p.name.clone());
        let about = program
            .as_ref()
            .and_then(|p| p.about.clone())
            .unwrap_or_default();
        let year = about.year.map_or(String::new(), |y| y.to_string());
        let mut view = AlmanacView {
            report,
            program,
            search: TextField::new(&name),
            rating: Rating::Green,
            year: TextField::new(&year),
            publisher: TextField::new(about.publisher.as_deref().unwrap_or("")),
            notes: TextField::new(about.notes.as_deref().unwrap_or("")),
            candidates: Vec::new(),
            asking: None,
            note: None,
            selected: 0,
            scroll: 0,
        };
        ask(&mut view);
        self.almanac = Some(view);
    }

    pub(super) fn almanac_lines(view: &AlmanacView) -> Vec<Line> {
        let mut lines = vec![Line::Search];
        if view.report {
            lines.push(Line::Rating);
            lines.extend(AboutField::ALL.map(Line::About));
        }
        if view.asking.is_some() && view.candidates.is_empty() {
            lines.push(Line::Note(format!("Asking {}...", almanac::SITE)));
        } else if !view.candidates.is_empty() {
            lines.push(Line::Heading(if view.report {
                "Report to"
            } else {
                "Games"
            }));
            lines.extend((0..view.candidates.len()).map(Line::Game));
        }
        if view.report && !view.search.text().trim().is_empty() {
            if view.candidates.is_empty() {
                lines.push(Line::Heading("Report to"));
            }
            lines.push(Line::NewGame);
        }
        if let Some((text, _)) = &view.note {
            lines.push(Line::Note(text.clone()));
        }
        lines
    }

    /// The almanac's answers, as they come.
    pub(super) fn poll_almanac(&mut self) {
        let Some(view) = &mut self.almanac else {
            return;
        };
        let Some(asking) = &view.asking else { return };
        let answer = match asking.try_recv() {
            Ok(answer) => answer,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                Answer::Candidates(Err("the question stopped".to_string()))
            }
        };
        view.asking = None;
        match answer {
            Answer::Candidates(Ok(candidates)) => {
                view.note = candidates.is_empty().then(|| {
                    let text = if view.report {
                        "The almanac has no such game: report it as a new one"
                    } else {
                        "The almanac has no such game: type its name and press Enter"
                    };
                    (text.to_string(), false)
                });
                view.candidates = candidates;
                view.selected = 0;
                view.scroll = 0;
            }
            Answer::Candidates(Err(e)) => view.note = Some((e, true)),
            Answer::Reported(result, name, unkept) => {
                let (text, failed) = match result {
                    Ok(Reported { id, replaced: true }) => (
                        format!("Thank you! Your report #{} on {} is updated", id, name),
                        false,
                    ),
                    Ok(Reported { id, .. }) => (
                        format!("Thank you! Report #{} on {} is in the almanac", id, name),
                        false,
                    ),
                    Err(e) => (e, true),
                };
                view.note = Some(match unkept {
                    Some(why) => (format!("{}. {}", text, why), true),
                    None => (text, failed),
                });
            }
        }
    }

    pub(super) fn almanac_key(&mut self, key: UiKey, host: &mut dyn Host) {
        let Some(view) = &mut self.almanac else {
            return;
        };
        let lines = Self::almanac_lines(view);
        let selected = view.selected.min(lines.len() - 1);
        let line = lines[selected].clone();
        let field_edits = matches!(
            key,
            UiKey::Char(_)
                | UiKey::Backspace
                | UiKey::Delete
                | UiKey::Left
                | UiKey::Right
                | UiKey::Home
                | UiKey::End
        );
        if let Line::About(field) = line {
            if field_edits && view.about_field(field).key(key) {
                return;
            }
        } else if line == Line::Search || matches!(key, UiKey::Char(_)) {
            // Typing goes to the name, wherever else the cursor is.
            let edits = matches!(
                key,
                UiKey::Char(_) | UiKey::Backspace | UiKey::Delete | UiKey::Left | UiKey::Right
            ) || (line == Line::Search && matches!(key, UiKey::Home | UiKey::End));
            if edits && view.search.key(key) {
                view.selected = 0;
                return;
            }
        }
        match key {
            UiKey::Up
            | UiKey::Down
            | UiKey::PageUp
            | UiKey::PageDown
            | UiKey::Home
            | UiKey::End => {
                let page = self.visible.saturating_sub(8).max(1);
                let last = lines.len() - 1;
                let next = match key {
                    UiKey::Up => (0..selected).rev().find(|&i| lines[i].selectable()),
                    UiKey::Down => (selected + 1..=last).find(|&i| lines[i].selectable()),
                    UiKey::PageUp => (0..=selected.saturating_sub(page))
                        .rev()
                        .find(|&i| lines[i].selectable()),
                    UiKey::PageDown => (selected..=(selected + page).min(last))
                        .rev()
                        .find(|&i| lines[i].selectable()),
                    UiKey::Home => lines.iter().position(Line::selectable),
                    _ => lines.iter().rposition(Line::selectable),
                };
                if let Some(next) = next {
                    view.selected = next;
                }
            }
            UiKey::Esc => {
                self.almanac = None;
                self.status = None;
            }
            UiKey::Left | UiKey::Right | UiKey::Enter if line == Line::Rating => {
                let at = Rating::ALL
                    .iter()
                    .position(|&r| r == view.rating)
                    .unwrap_or(0) as isize;
                let step = if key == UiKey::Left { -1 } else { 1 };
                view.rating =
                    Rating::ALL[(at + step).rem_euclid(Rating::ALL.len() as isize) as usize];
            }
            UiKey::Enter if matches!(line, Line::About(_)) => {
                if let Some(next) = (selected + 1..lines.len()).find(|&i| lines[i].selectable()) {
                    view.selected = next;
                }
            }
            UiKey::Enter if view.asking.is_some() => {}
            UiKey::Enter => match line {
                Line::Search => ask(view),
                Line::Game(i) if view.report => {
                    let c = &view.candidates[i];
                    let subject = almanac_subject(c);
                    let name = c.name.clone();
                    self.send_report(subject, name, host);
                }
                Line::Game(i) => {
                    let c = view.candidates[i].clone();
                    self.apply_almanac(&c, host);
                }
                Line::NewGame => {
                    let name = view.search.text().trim().to_string();
                    self.send_report(Subject::New(name.clone()), name, host);
                }
                _ => {}
            },
            _ => {}
        }
    }

    /// Fetching: the settings `c` recommends over these.
    fn apply_almanac(&mut self, c: &Candidate, host: &mut dyn Host) {
        let Some(recommendation) = c.recommendation.as_ref().filter(|_| c.has_settings()) else {
            if let Some(view) = &mut self.almanac {
                view.note = Some((
                    format!("{} has no settings in the almanac yet", c.name),
                    true,
                ));
            }
            return;
        };
        let usable = recommendation.usable();
        self.settings = almanac::layer(&self.settings, &usable, self.home.as_deref());
        self.almanac = None;
        let about = format!(
            "{} settings for {} ({})",
            usable.len(),
            c.name,
            recommendation.describe()
        );
        match host.apply(&self.settings) {
            Err(problem) => self.error(problem),
            Ok(Some(note)) => self.info(format!("Applied {}. {}", about, note)),
            Ok(None) => self.info(format!("Applied {} (F2 saves them)", about)),
        }
    }

    /// Reporting: the settings as they are, the rating picked, what the
    /// report says of the game (kept in its profile, which gets a UUID if
    /// it has none), the program's file, to `subject`, on a thread of its
    /// own.
    fn send_report(&mut self, subject: Subject, name: String, host: &mut dyn Host) {
        let settings = almanac::report_settings(&self.settings, self.home.as_deref());
        let Some(view) = &mut self.almanac else {
            return;
        };
        let about = match view.about() {
            Ok(about) => about,
            Err(e) => {
                view.note = Some((e, true));
                return;
            }
        };
        // The report doesn't wait on the profile: one that can't be
        // written (a read-only games folder, say) lets it go all the same,
        // with the key the profile had, and the note says why it didn't
        // keep what the report says.
        let (about, unkept) = match host.almanac_save_about(&about) {
            Ok(Some(saved)) => (saved, None),
            Ok(None) => (about, None),
            Err(e) => {
                let why = format!(
                    "The game's profile can't keep the year, publisher and notes: {}",
                    e
                );
                (about, Some(why))
            }
        };
        if let Some(program) = &mut view.program {
            program.about = Some(about.clone());
        }
        let files = view
            .program
            .as_ref()
            .map_or_else(Vec::new, |p| p.files.clone());
        let platform = view.program.as_ref().and_then(|p| p.platform.clone());
        let rating = view.rating;
        view.note = Some((format!("Reporting to {}...", almanac::SITE), false));
        let (done, answer) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("almanac".to_string())
            .spawn(move || {
                let result = almanac::report(
                    &subject,
                    &files,
                    platform.as_deref(),
                    rating,
                    &settings,
                    &about,
                );
                let _ = done.send(Answer::Reported(result, name, unkept));
            });
        match spawned {
            Ok(_) => view.asking = Some(answer),
            Err(e) => view.note = Some((format!("Can't ask {}: {}", almanac::SITE, e), true)),
        }
    }

    pub(super) fn almanac_clicked(&mut self, target: Target, host: &mut dyn Host) {
        let Target::AlmanacRow(i) = target else {
            return;
        };
        let Some(view) = &mut self.almanac else {
            return;
        };
        if view.selected == i {
            self.key(UiKey::Enter, host);
        } else {
            view.selected = i;
        }
    }

    pub(super) fn almanac_hints(&self) -> Vec<(&'static str, &'static str, UiKey)> {
        let Some(view) = &self.almanac else {
            return Vec::new();
        };
        let lines = Self::almanac_lines(view);
        let mut hints = match lines.get(view.selected) {
            Some(Line::Search) => vec![("Enter", "Search", UiKey::Enter)],
            Some(Line::Rating) => vec![("\u{2190}\u{2192}", "Change", UiKey::Right)],
            Some(Line::About(_)) => vec![("Enter", "Next", UiKey::Enter)],
            Some(Line::Game(_)) if view.report => vec![("Enter", "Report", UiKey::Enter)],
            Some(Line::Game(_)) => vec![("Enter", "Apply", UiKey::Enter)],
            Some(Line::NewGame) => vec![("Enter", "Report", UiKey::Enter)],
            _ => Vec::new(),
        };
        hints.push(("Esc", "Back", UiKey::Esc));
        hints
    }

    pub(super) fn draw_almanac(&mut self, g: &mut Grid, content: std::ops::Range<usize>) {
        let Some(view) = &self.almanac else { return };
        let lines = Self::almanac_lines(view);
        let cols = g.cols;
        let Some(view) = &mut self.almanac else {
            return;
        };
        view.selected = view.selected.min(lines.len() - 1);
        let top = content.start;
        let title = if view.report {
            "Report settings to the Almanac"
        } else {
            "Fetch settings from the Almanac"
        };
        g.text_to(2, top, title, draw::BRIGHT, cols - 2);

        // What the line selected is, at the bottom.
        let about_rows = 4;
        let about_top = content.end.saturating_sub(about_rows);
        let about = about(view, &lines[view.selected]);
        for (line, row) in wrap(about.trim(), cols - 4, about_rows)
            .into_iter()
            .zip(about_top..content.end)
        {
            g.text_to(2, row, &line, draw::DIM, cols - 2);
        }

        let rows = top + 2..about_top.saturating_sub(1);
        Self::keep_visible(&mut view.scroll, view.selected, rows.len());
        let value_col = 12.min(cols / 3);
        let right_col = cols.saturating_sub(24);
        let mut hits = Vec::new();
        for (i, row) in (view.scroll..lines.len()).zip(rows.clone()) {
            let selected = i == view.selected;
            if selected && lines[i] != Line::Search {
                g.background(1, row, cols - 2, draw::SELECT);
            }
            if lines[i].selectable() {
                hits.push(Hit {
                    row,
                    col: 1,
                    width: cols - 2,
                    target: Target::AlmanacRow(i),
                });
            }
            let bright = |color| if selected { draw::BRIGHT } else { color };
            match &lines[i] {
                Line::Search => {
                    g.text(2, row, "Game", bright(draw::TEXT));
                    let width = (cols - 3).saturating_sub(value_col);
                    g.background(value_col, row, width, draw::FIELD);
                    let (shown, cursor) = view.search.view(width);
                    g.text_to(value_col, row, &shown, draw::BRIGHT, value_col + width);
                    if selected {
                        g.background(value_col + cursor, row, 1, draw::SELECT);
                    }
                }
                Line::Rating => {
                    g.text(2, row, "Rating", bright(draw::TEXT));
                    let color = match view.rating {
                        Rating::Green => draw::GOOD,
                        Rating::Yellow => draw::KEY,
                        Rating::Red => draw::ERROR,
                    };
                    g.text_to(
                        value_col,
                        row,
                        &format!("\u{25C4} {} \u{25BA}", view.rating.label()),
                        color,
                        cols - 2,
                    );
                }
                Line::About(field) => {
                    g.text(2, row, field.label(), bright(draw::TEXT));
                    let width = (cols - 3).saturating_sub(value_col);
                    g.background(value_col, row, width, draw::FIELD);
                    let text = match field {
                        AboutField::Year => &view.year,
                        AboutField::Publisher => &view.publisher,
                        AboutField::Notes => &view.notes,
                    };
                    let (shown, cursor) = text.view(width);
                    g.text_to(value_col, row, &shown, draw::BRIGHT, value_col + width);
                    if selected {
                        g.background(value_col + cursor, row, 1, draw::SELECT);
                    }
                }
                Line::Heading(title) => {
                    g.text_to(2, row, title, draw::NOTE, cols - 2);
                }
                Line::Game(c) => {
                    let c = &view.candidates[*c];
                    let mut name = format!("  {}", c.name);
                    if let Some((_, label)) = &c.version {
                        name.push_str(&format!(" - {}", label));
                    }
                    if c.hash_match {
                        name.push_str(" \u{221A}");
                    }
                    g.text_to(
                        2,
                        row,
                        &fit(&name, right_col.saturating_sub(3)),
                        bright(draw::TEXT),
                        right_col.saturating_sub(1),
                    );
                    let side = match &c.recommendation {
                        Some(r) if matches!(r.source, almanac::Source::Blessed { .. }) => {
                            "blessed".to_string()
                        }
                        _ => c.ratings.describe(),
                    };
                    g.text_to(right_col, row, &fit(&side, 22), bright(draw::DIM), cols - 2);
                }
                Line::NewGame => {
                    let text = format!("  + Add \"{}\" as a new game", view.search.text().trim());
                    g.text_to(2, row, &fit(&text, cols - 5), bright(draw::KEY), cols - 2);
                }
                Line::Note(text) => {
                    let error = view.note.as_ref().is_some_and(|(t, e)| t == text && *e);
                    g.text_to(
                        2,
                        row,
                        &fit(text, cols - 5),
                        if error { draw::ERROR } else { draw::NOTE },
                        cols - 2,
                    );
                }
            }
        }
        let (scroll, total) = (view.scroll, lines.len());
        self.hits.extend(hits);
        self.draw_scrollbar(g, rows, scroll, total);
    }
}

impl AlmanacView {
    fn about_field(&mut self, field: AboutField) -> &mut TextField {
        match field {
            AboutField::Year => &mut self.year,
            AboutField::Publisher => &mut self.publisher,
            AboutField::Notes => &mut self.notes,
        }
    }

    /// What the report says of the game: the fields', and the profile's
    /// key. An error if the year isn't one.
    pub(super) fn about(&self) -> Result<About, String> {
        let year = crate::games::parse_year(&self.year.text())?;
        let key = self
            .program
            .as_ref()
            .and_then(|p| p.about.as_ref())
            .and_then(|a| a.key.clone());
        Ok(About {
            year,
            publisher: crate::games::one_line(&self.publisher.text()),
            notes: crate::games::one_line(&self.notes.text()),
            key,
        })
    }
}

/// Ask the almanac for the program's games and those named as typed, on
/// a thread of its own.
fn ask(view: &mut AlmanacView) {
    if view.asking.is_some() {
        return;
    }
    let files = view
        .program
        .as_ref()
        .map_or_else(Vec::new, |p| p.files.clone());
    let name = view.search.text();
    if files.is_empty() && name.trim().is_empty() {
        view.note = Some(("Type the game's name and press Enter".to_string(), false));
        return;
    }
    view.note = None;
    let (done, answer) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("almanac".to_string())
        .spawn(move || {
            let _ = done.send(Answer::Candidates(almanac::candidates(&files, &name, true)));
        });
    match spawned {
        Ok(_) => view.asking = Some(answer),
        Err(e) => view.note = Some((format!("Can't ask {}: {}", almanac::SITE, e), true)),
    }
}

/// A report on the game `c`, to the version the program matched.
fn almanac_subject(c: &Candidate) -> Subject {
    let version = c
        .version
        .as_ref()
        .filter(|_| c.hash_match)
        .map(|(id, _)| *id);
    Subject::Game {
        slug: c.slug.clone(),
        version,
    }
}

/// What the line `line` of the list is, under it.
fn about(view: &AlmanacView, line: &Line) -> String {
    let program = view.program.as_ref().and_then(|p| p.files.first());
    match line {
        Line::Search => match program {
            Some(file) => format!(
                "Games whose files match {} (the program started last), and those with \
                 the name typed. Enter searches.",
                file.name.to_ascii_uppercase()
            ),
            None => "No program has run yet: type the game's name and press Enter.".to_string(),
        },
        Line::Rating => "How the game plays with the settings as they are now.".to_string(),
        Line::About(field) => {
            let what = match field {
                AboutField::Year => {
                    "The year the game came out, which the almanac takes if it hasn't one."
                }
                AboutField::Publisher => {
                    "Who published it, which the almanac takes if it hasn't one."
                }
                AboutField::Notes => "How it plays, what it needs: shown with your report.",
            };
            let keeps = match view.program.as_ref().and_then(|p| p.about.as_ref()) {
                Some(about) if about.key.is_some() => {
                    "Its profile keeps it, and a report from it replaces the one it sent on \
                     that game before."
                }
                Some(_) => {
                    "Its profile keeps it, and gets a UUID: a report from it replaces the one \
                     it sent on that game before."
                }
                None => "No game profile plays: it goes with this report only.",
            };
            format!("{} {}", what, keeps)
        }
        Line::Game(i) => {
            let c = &view.candidates[*i];
            let found = match (&c.version, c.hash_match) {
                (Some((_, label)), true) => format!("Its version {} has this very program.", label),
                (Some((_, label)), false) => {
                    format!("Its version {} has a file of the same name.", label)
                }
                _ => "Found by its name.".to_string(),
            };
            let what = match &c.recommendation {
                _ if view.report => {
                    "Enter sends your settings and the rating, with the program's checksums."
                        .to_string()
                }
                Some(r) if c.has_settings() => {
                    let shown: Vec<String> = r
                        .usable()
                        .iter()
                        .take(8)
                        .map(|s| format!("{}={}", s.key, s.value))
                        .collect();
                    format!("Settings {}: {}.", r.describe(), shown.join(" "))
                }
                Some(_) => "No settings reported yet.".to_string(),
                None => "Its settings couldn't be had.".to_string(),
            };
            format!("{} {}", found, what)
        }
        Line::NewGame => "Adds the game to the almanac by this name, with the program's name and \
                          checksums, and your report."
            .to_string(),
        _ => String::new(),
    }
}
