//! The settings window's Games page: the game profiles, launched with
//! Enter, made from the current settings with Ins, deleted with Del.

use super::dialog::TextField;
use super::draw::{self, Grid};
use super::{ConfigUi, Hit, Host, Target, UiKey, fit};
use crate::games::NewGame;

/// The new game dialog's controls, in focus order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameField {
    Name,
    Directory,
    Command,
    Create,
    Cancel,
}

const FIELDS: [GameField; 5] = [GameField::Name, GameField::Directory, GameField::Command, GameField::Create, GameField::Cancel];

/// Making a profile of the current settings: the game's name, its
/// directory and its command.
pub struct GameDialog {
    pub name: TextField,
    pub directory: TextField,
    pub command: TextField,
    pub focus: GameField,
}

impl GameDialog {
    pub fn new(directory: &str) -> Self {
        Self {
            name: TextField::default(),
            directory: TextField::new(directory),
            command: TextField::default(),
            focus: GameField::Name,
        }
    }

    fn field(&mut self) -> Option<&mut TextField> {
        match self.focus {
            GameField::Name => Some(&mut self.name),
            GameField::Directory => Some(&mut self.directory),
            GameField::Command => Some(&mut self.command),
            _ => None,
        }
    }

    fn step_focus(&mut self, dir: isize) {
        let at = FIELDS.iter().position(|&f| f == self.focus).unwrap_or(0) as isize;
        self.focus = FIELDS[(at + dir).rem_euclid(FIELDS.len() as isize) as usize];
    }

    pub fn game(&self) -> NewGame {
        NewGame { name: self.name.text(), directory: self.directory.text(), command: self.command.text() }
    }
}

impl ConfigUi {
    /// The profiles and the running game as the frontend has them now.
    pub(super) fn refresh_games(&mut self, host: &dyn Host) {
        self.games = host.games();
        self.active_game = host.active_game();
        self.row = self.row.min(self.row_count().saturating_sub(1));
    }

    pub(super) fn games_key(&mut self, key: UiKey, host: &mut dyn Host) {
        // Del asked first; Enter deletes, anything else keeps the game.
        if let Some(i) = self.confirm_delete.take() {
            if key == UiKey::Enter
                && let Some(game) = self.games.get(i).cloned()
            {
                match host.delete_game(&game.id) {
                    Ok(()) => {
                        self.refresh_games(host);
                        self.info(format!("{} has been deleted", game.name));
                    }
                    Err(e) => self.error(e),
                }
            } else {
                self.status = None;
            }
            return;
        }
        let selected = self.games.get(self.row).cloned();
        match (key, selected) {
            (UiKey::Insert, _) | (UiKey::Enter, None) => {
                self.status = None;
                self.game_dialog = Some(GameDialog::new(&host.current_directory()));
            }
            (UiKey::Enter, Some(game)) => match host.launch_game(&game.id) {
                Ok(message) => {
                    self.close();
                    self.notice = Some(message);
                }
                Err(e) => self.error(e),
            },
            (UiKey::Delete, Some(game)) => {
                self.confirm_delete = Some(self.row);
                self.error(format!("Delete {}? Enter deletes it, Esc keeps it", game.name));
            }
            _ => {}
        }
    }

    pub(super) fn game_dialog_key(&mut self, key: UiKey, host: &mut dyn Host) {
        let Some(dialog) = &mut self.game_dialog else { return };
        if let Some(field) = dialog.field()
            && field.key(key)
        {
            return;
        }
        match key {
            UiKey::Esc => {
                self.game_dialog = None;
                self.status = None;
            }
            UiKey::Tab | UiKey::Down => dialog.step_focus(1),
            UiKey::BackTab | UiKey::Up => dialog.step_focus(-1),
            UiKey::Enter if dialog.focus == GameField::Cancel => {
                self.game_dialog = None;
                self.status = None;
            }
            UiKey::Enter if dialog.focus == GameField::Create || dialog.focus == GameField::Command => {
                let game = dialog.game();
                match host.create_game(&game, &self.settings) {
                    Ok(id) => {
                        self.game_dialog = None;
                        self.refresh_games(host);
                        if let Some(i) = self.games.iter().position(|g| g.id == id) {
                            self.row = i;
                        }
                        self.info(format!("{} has its profile: Enter launches it", game.name.trim()));
                    }
                    Err(e) => self.error(e),
                }
            }
            UiKey::Enter => dialog.step_focus(1),
            _ => {}
        }
    }

    pub(super) fn draw_games(&mut self, g: &mut Grid, content: std::ops::Range<usize>) {
        let cols = g.cols;
        Self::keep_visible(&mut self.scroll, self.row, content.len());
        let command_col = 30.min(cols / 2);
        for (i, row) in (self.scroll..self.row_count()).zip(content.clone()) {
            if i == self.row {
                self.select_row(g, row);
            }
            self.hits.push(Hit { row, col: 1, width: cols - 2, target: Target::Row(i) });
            let Some(game) = self.games.get(i) else {
                g.text(2, row, "+ New game from the current settings...", draw::KEY);
                continue;
            };
            let running = self.active_game.as_deref() == Some(game.id.as_str());
            g.text_to(2, row, &fit(&game.name, command_col - 3), draw::BRIGHT, command_col - 1);
            let end = if running { cols - 11 } else { cols - 2 };
            g.text_to(command_col, row, &fit(&game.command, end.saturating_sub(command_col + 1)), draw::DIM, end);
            if running {
                g.text(cols - 10, row, "running", draw::GOOD);
            }
        }
        self.draw_scrollbar(g, content, self.scroll, self.row_count());
    }

    pub(super) fn draw_game_dialog(&mut self, g: &mut Grid, content: std::ops::Range<usize>) {
        let Some(dialog) = &self.game_dialog else { return };
        let cols = g.cols;
        let top = content.start;
        let value_col = 16.min(cols / 3);
        let width = (cols - 3).saturating_sub(value_col).min(40);
        g.text(2, top, "A new game, with the settings as they are now", draw::BRIGHT);
        let mut hits = Vec::new();
        let rows = [
            (GameField::Name, "Name", &dialog.name, "Commander Keen 4"),
            (GameField::Directory, "Directory", &dialog.directory, "C:\\KEEN4"),
            (GameField::Command, "Command", &dialog.command, "KEEN4E"),
        ];
        for (i, (field, label, text, example)) in rows.into_iter().enumerate() {
            let row = top + 2 + i;
            if row >= content.end {
                break;
            }
            let focused = dialog.focus == field;
            g.text(4, row, label, if focused { draw::BRIGHT } else { draw::TEXT });
            g.background(value_col, row, width, draw::FIELD);
            let (shown, cursor) = text.view(width);
            if shown.is_empty() && !focused {
                g.text_to(value_col, row, example, draw::DIM, value_col + width);
            }
            g.text_to(value_col, row, &shown, draw::BRIGHT, value_col + width);
            if focused {
                g.background(value_col + cursor, row, 1, draw::SELECT);
            }
            hits.push(Hit { row, col: value_col, width, target: Target::GameField(field) });
        }
        let row = top + 6;
        if row < content.end {
            let mut col = value_col;
            for (field, text) in [(GameField::Create, "[ Create ]"), (GameField::Cancel, "[ Cancel ]")] {
                let focused = dialog.focus == field;
                if focused {
                    g.background(col, row, text.len(), draw::SELECT);
                }
                g.text(col, row, text, if focused { draw::BRIGHT } else { draw::KEY });
                hits.push(Hit { row, col, width: text.len(), target: Target::GameField(field) });
                col += text.len() + 2;
            }
        }
        if top + 8 < content.end {
            g.text_to(4, top + 8, "It keeps the settings that differ from the configuration file's,", draw::DIM, cols - 2);
            g.text_to(4, top + 9, "and the drives mounted since. While the game plays, F2 saves to it.", draw::DIM, cols - 2);
        }
        self.hits.extend(hits);
    }

    /// A click on a control of the new game dialog.
    pub(super) fn game_field_clicked(&mut self, field: GameField, host: &mut dyn Host) {
        if let Some(dialog) = &mut self.game_dialog {
            dialog.focus = field;
            if matches!(field, GameField::Create | GameField::Cancel) {
                self.key(UiKey::Enter, host);
            }
        }
    }
}
