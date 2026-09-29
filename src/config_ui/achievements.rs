//! The settings window's Achievements page: RetroAchievements on or off,
//! hardcore mode, logging in, which version the game playing is, and
//! its achievements and leaderboards (see achievements/).

use super::dialog::TextField;
use super::draw::{self, Grid};
use super::{ConfigUi, Hit, Host, Pick, Target, UiKey};
use crate::achievements::AchievementsView;

/// A row of the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AchRow {
    Enabled,
    Hardcore,
    User,
    Password,
    LogIn,
    /// Logged in: Enter logs out.
    Account,
    Status,
    /// The game's zip or .dosz, to know it by.
    Archive,
    Presence,
    Achievement(usize),
    Leaderboard(usize),
}

/// The page's state: what the session shows, and the account's fields.
#[derive(Default)]
pub struct Achievements {
    pub view: Option<AchievementsView>,
    user: TextField,
    password: TextField,
    /// The field being typed in.
    pub edit: Option<AchRow>,
}

impl Achievements {
    pub fn rows(&self, active_game: bool) -> Vec<AchRow> {
        // Where there is no RetroAchievements (the browser), a note.
        let Some(view) = &self.view else {
            return vec![AchRow::Status];
        };
        let mut rows = vec![AchRow::Enabled, AchRow::Hardcore];
        match &view.user {
            Some(_) => rows.push(AchRow::Account),
            None => rows.extend([AchRow::User, AchRow::Password, AchRow::LogIn]),
        }
        rows.push(AchRow::Status);
        if active_game {
            rows.push(AchRow::Archive);
        }
        if let Some(game) = &view.game {
            if !game.rich_presence.is_empty() {
                rows.push(AchRow::Presence);
            }
            rows.extend((0..game.achievements.len()).map(AchRow::Achievement));
            rows.extend((0..game.leaderboards.len()).map(AchRow::Leaderboard));
        }
        rows
    }

    /// What the page shows, as the session has it now.
    pub fn refresh(&mut self, host: &dyn Host) {
        self.view = host.achievements();
        if self.user.text().is_empty()
            && let Some((name, ..)) = self.view.as_ref().and_then(|v| v.user.clone())
        {
            self.user = TextField::new(&name);
        }
    }
}

impl ConfigUi {
    fn achievement_row(&self) -> Option<AchRow> {
        self.achievements
            .rows(self.active_game.is_some())
            .get(self.row)
            .copied()
    }

    /// Every frame while the page shows: the session's state.
    pub(super) fn poll_achievements(&mut self, host: &mut dyn Host) {
        if self.page == super::Page::Achievements {
            self.achievements.refresh(host);
            self.row = self.row.min(self.row_count().saturating_sub(1));
        }
    }

    fn apply_achievements(&mut self, host: &mut dyn Host, note: &str) {
        match host.apply(&self.settings) {
            Ok(_) => self.info(note),
            Err(e) => self.error(e),
        }
        self.achievements.refresh(host);
    }

    fn log_in(&mut self, host: &mut dyn Host) {
        let (user, password) = (
            self.achievements.user.text(),
            self.achievements.password.text(),
        );
        if user.trim().is_empty() || password.is_empty() {
            return self.error("Type your RetroAchievements user name and password");
        }
        if !self.settings.achievements.enabled {
            self.settings.achievements.enabled = true;
            let _ = host.apply(&self.settings);
        }
        match host.achievements_login(user.trim(), &password) {
            Ok(()) => self.info("Logging in..."),
            Err(e) => self.error(e),
        }
        self.achievements.password = TextField::default();
        self.achievements.refresh(host);
    }

    pub(super) fn achievements_key(&mut self, key: UiKey, host: &mut dyn Host) {
        let Some(row) = self.achievement_row() else {
            return;
        };
        let toggle = matches!(key, UiKey::Left | UiKey::Right | UiKey::Enter);
        match row {
            AchRow::Enabled if toggle => {
                self.settings.achievements.enabled = !self.settings.achievements.enabled;
                let note = if self.settings.achievements.enabled {
                    "RetroAchievements is on"
                } else {
                    "RetroAchievements is off"
                };
                self.apply_achievements(host, note);
            }
            AchRow::Hardcore if toggle => {
                self.settings.achievements.hardcore = !self.settings.achievements.hardcore;
                self.apply_achievements(host, "From the next game on (F2 saves it)");
            }
            AchRow::User | AchRow::Password if key == UiKey::Enter => {
                self.achievements.edit = Some(row)
            }
            AchRow::LogIn if key == UiKey::Enter => self.log_in(host),
            AchRow::Account if key == UiKey::Enter => {
                host.achievements_logout();
                self.info("Logged out of RetroAchievements");
                self.achievements.refresh(host);
            }
            AchRow::Archive if key == UiKey::Enter => self.open_browser(Pick::AchievementsArchive),
            _ => {}
        }
    }

    /// Typing the user name or password: Enter goes on to the password,
    /// and from it logs in.
    pub(super) fn achievements_edit_key(&mut self, key: UiKey, host: &mut dyn Host) {
        let Some(row) = self.achievements.edit else {
            return;
        };
        let field = if row == AchRow::User {
            &mut self.achievements.user
        } else {
            &mut self.achievements.password
        };
        if field.key(key) {
            return;
        }
        match key {
            UiKey::Esc => self.achievements.edit = None,
            UiKey::Enter | UiKey::Tab if row == AchRow::User => {
                self.achievements.edit = Some(AchRow::Password);
                self.row += 1;
            }
            UiKey::Enter => {
                self.achievements.edit = None;
                self.log_in(host);
            }
            UiKey::BackTab if row == AchRow::Password => {
                self.achievements.edit = Some(AchRow::User);
                self.row -= 1;
            }
            UiKey::Tab | UiKey::BackTab => self.achievements.edit = None,
            _ => {}
        }
    }

    pub(super) fn draw_achievements(&mut self, g: &mut Grid, content: std::ops::Range<usize>) {
        let cols = g.cols;
        let rows = self.achievements.rows(self.active_game.is_some());
        Self::keep_visible(&mut self.scroll, self.row, content.len());
        let value_col = 22.min(cols / 2);
        let end = cols - 2;
        let on_off = |on: bool| if on { "on" } else { "off" };
        let view = self.achievements.view.clone();
        let view = view.as_ref();
        let mut hits = Vec::new();
        for (i, row) in (self.scroll..rows.len()).zip(content.clone()) {
            let selected = i == self.row;
            if selected {
                g.background(1, row, cols - 2, draw::SELECT);
            }
            hits.push(Hit {
                row,
                col: 1,
                width: cols - 2,
                target: Target::Row(i),
            });
            let label = if selected { draw::BRIGHT } else { draw::TEXT };
            let mut choice = |g: &mut Grid, text: &str| {
                g.char(value_col, row, 0x11, draw::KEY);
                hits.push(Hit {
                    row,
                    col: value_col,
                    width: 1,
                    target: Target::Step(i, -1),
                });
                let after = g.text_to(value_col + 2, row, text, draw::BRIGHT, end);
                g.char(after + 1, row, 0x10, draw::KEY);
                hits.push(Hit {
                    row,
                    col: after + 1,
                    width: 1,
                    target: Target::Step(i, 1),
                });
                after + 3
            };
            match rows[i] {
                AchRow::Enabled => {
                    g.text(2, row, "RetroAchievements", label);
                    choice(g, on_off(self.settings.achievements.enabled));
                }
                AchRow::Hardcore => {
                    g.text(2, row, "Hardcore mode", label);
                    let after = choice(g, on_off(self.settings.achievements.hardcore));
                    g.text_to(after, row, "no states, rewind or cheats", draw::DIM, end);
                }
                AchRow::User | AchRow::Password => {
                    let is_user = rows[i] == AchRow::User;
                    g.text(
                        2,
                        row,
                        if is_user { "User name" } else { "Password" },
                        label,
                    );
                    let field = if is_user {
                        &self.achievements.user
                    } else {
                        &self.achievements.password
                    };
                    let width = end.saturating_sub(value_col).min(24);
                    let (text, cursor) = field.view(width);
                    let text = if is_user {
                        text
                    } else {
                        "*".repeat(text.chars().count())
                    };
                    g.background(value_col, row, width, draw::FIELD);
                    g.text_to(value_col, row, &text, draw::BRIGHT, value_col + width);
                    if self.achievements.edit == Some(rows[i]) {
                        g.background(value_col + cursor, row, 1, draw::SELECT);
                    }
                }
                AchRow::LogIn => {
                    let busy = view.is_some_and(|v| v.logging_in);
                    g.text(
                        value_col,
                        row,
                        if busy { "Logging in..." } else { "[ Log in ]" },
                        if busy { draw::DIM } else { draw::KEY },
                    );
                }
                AchRow::Account => {
                    g.text(2, row, "Account", label);
                    if let Some((name, score, softcore)) = view.and_then(|v| v.user.clone()) {
                        let text = format!("{}, {} points ({} softcore)", name, score, softcore);
                        let after = g.text_to(value_col, row, &text, draw::BRIGHT, end);
                        g.text_to(after + 2, row, "Enter logs out", draw::DIM, end);
                    }
                }
                AchRow::Status => {
                    g.text(2, row, "Game", label);
                    let status = view.map_or(
                        "RetroAchievements needs the Rust-DOS program".to_string(),
                        |v| v.status.clone(),
                    );
                    let color = if view.is_some_and(|v| v.game.is_some()) {
                        draw::GOOD
                    } else {
                        draw::DIM
                    };
                    g.text_to(value_col, row, &status, color, end);
                }
                AchRow::Archive => {
                    g.text(2, row, "Game's archive", label);
                    g.text_to(
                        value_col,
                        row,
                        "Enter picks the zip or .dosz it came in",
                        draw::DIM,
                        end,
                    );
                }
                AchRow::Presence => {
                    g.text(2, row, "Now", label);
                    let text = view
                        .and_then(|v| v.game.as_ref())
                        .map_or(String::new(), |g| g.rich_presence.clone());
                    g.text_to(value_col, row, &text, draw::TEXT, end);
                }
                AchRow::Achievement(a) => {
                    let Some(a) = view
                        .and_then(|v| v.game.as_ref())
                        .and_then(|g| g.achievements.get(a))
                    else {
                        continue;
                    };
                    let (mark, color) = if a.unlocked {
                        (0xFB, draw::GOOD)
                    } else if a.primed {
                        (b'!', draw::KEY)
                    } else {
                        (0xFA, draw::DIM)
                    };
                    g.char(2, row, mark, color);
                    let title = if a.official {
                        a.title.clone()
                    } else {
                        format!("{} (unofficial)", a.title)
                    };
                    let after = g.text_to(
                        4,
                        row,
                        &title,
                        if a.unlocked { draw::BRIGHT } else { label },
                        end,
                    );
                    let mut note = format!("{} pts", a.points);
                    if let Some((value, target)) = a.progress {
                        note = format!("{}/{}, {}", value, target, note);
                    }
                    let after = g.text_to(after + 2, row, &note, draw::NOTE, end);
                    g.text_to(after + 2, row, &a.description, draw::DIM, end);
                }
                AchRow::Leaderboard(l) => {
                    let Some((title, description)) = view
                        .and_then(|v| v.game.as_ref())
                        .and_then(|g| g.leaderboards.get(l))
                    else {
                        continue;
                    };
                    g.char(2, row, 0xF0, draw::BORDER);
                    let after = g.text_to(4, row, title, label, end);
                    g.text_to(after + 2, row, description, draw::DIM, end);
                }
            }
        }
        self.hits.extend(hits);
        self.draw_scrollbar(g, content, self.scroll, rows.len());
    }

    /// The key hints of the page's selected row.
    pub(super) fn achievements_hints(&self) -> Vec<(&'static str, &'static str, UiKey)> {
        use UiKey::*;
        let mut hints = match self.achievement_row() {
            Some(AchRow::Enabled | AchRow::Hardcore) => vec![("\u{2190}\u{2192}", "Change", Right)],
            Some(AchRow::User | AchRow::Password) => vec![("Enter", "Type", Enter)],
            Some(AchRow::LogIn) => vec![("Enter", "Log in", Enter)],
            Some(AchRow::Account) => vec![("Enter", "Log out", Enter)],
            Some(AchRow::Archive) => vec![("Enter", "Pick", Enter)],
            _ => Vec::new(),
        };
        hints.extend([
            ("Tab", "Page", Tab),
            ("F2", "Save", Save),
            ("Esc", "Close", Esc),
        ]);
        hints
    }
}
