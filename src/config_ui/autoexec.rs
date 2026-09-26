//! The settings window's editor of the `[autoexec]` section: the commands
//! typed at the DOS prompt on startup, of the configuration file, or of the
//! game's profile while one plays. The Emulator page's last row opens it;
//! F2 writes the lines into the file, Esc leaves it as it was.

use super::draw::{self, Grid};
use super::{ConfigUi, Hit, Host, Target, UiKey};

/// The lines being edited, and the cursor.
pub struct AutoexecEditor {
    lines: Vec<Vec<char>>,
    /// The cursor's line and column, in characters.
    line: usize,
    col: usize,
    /// The first line and column shown.
    scroll: usize,
    left: usize,
    changed: bool,
    /// Esc was pressed with changes: pressed again, it drops them.
    leaving: bool,
}

impl AutoexecEditor {
    pub fn new(lines: &[String]) -> Self {
        let mut lines: Vec<Vec<char>> = lines.iter().map(|l| l.chars().collect()).collect();
        if lines.is_empty() {
            lines.push(Vec::new());
        }
        Self { lines, line: 0, col: 0, scroll: 0, left: 0, changed: false, leaving: false }
    }

    pub fn lines(&self) -> Vec<String> {
        self.lines.iter().map(|l| l.iter().collect()).collect()
    }

    /// Edit or move with `key`, Page Up and Down going `page` lines.
    /// Returns false for keys it doesn't take.
    fn key(&mut self, key: UiKey, page: usize) -> bool {
        let (line, col, len) = (self.line, self.col, self.lines[self.line].len());
        let last = self.lines.len() - 1;
        let edited = match key {
            UiKey::Char(c) if !c.is_control() => {
                self.lines[line].insert(col, c);
                self.col += 1;
                true
            }
            UiKey::Enter => {
                let rest = self.lines[line].split_off(col);
                self.lines.insert(line + 1, rest);
                (self.line, self.col) = (line + 1, 0);
                true
            }
            UiKey::Backspace if col > 0 => {
                self.lines[line].remove(col - 1);
                self.col -= 1;
                true
            }
            UiKey::Backspace if line > 0 => {
                let rest = self.lines.remove(line);
                (self.line, self.col) = (line - 1, self.lines[line - 1].len());
                self.lines[line - 1].extend(rest);
                true
            }
            UiKey::Delete if col < len => {
                self.lines[line].remove(col);
                true
            }
            UiKey::Delete if line < last => {
                let next = self.lines.remove(line + 1);
                self.lines[line].extend(next);
                true
            }
            UiKey::Backspace | UiKey::Delete => false,
            UiKey::Left if col > 0 => {
                self.col -= 1;
                false
            }
            UiKey::Left if line > 0 => {
                (self.line, self.col) = (line - 1, self.lines[line - 1].len());
                false
            }
            UiKey::Right if col < len => {
                self.col += 1;
                false
            }
            UiKey::Right if line < last => {
                (self.line, self.col) = (line + 1, 0);
                false
            }
            UiKey::Left | UiKey::Right => false,
            UiKey::Home => {
                self.col = 0;
                false
            }
            UiKey::End => {
                self.col = len;
                false
            }
            UiKey::Up => {
                self.go_to(line.saturating_sub(1), col);
                false
            }
            UiKey::Down => {
                self.go_to(line + 1, col);
                false
            }
            UiKey::PageUp => {
                self.go_to(line.saturating_sub(page.max(1)), col);
                false
            }
            UiKey::PageDown => {
                self.go_to(line + page.max(1), col);
                false
            }
            _ => return false,
        };
        self.changed |= edited;
        true
    }

    /// Put the cursor on `line` at `col`, or as near as there is.
    fn go_to(&mut self, line: usize, col: usize) {
        self.line = line.min(self.lines.len() - 1);
        self.col = col.min(self.lines[self.line].len());
    }
}

impl ConfigUi {
    /// Open the editor on the `[autoexec]` lines of the file F2 saves to.
    pub(super) fn open_autoexec(&mut self, host: &dyn Host) {
        if self.config_file.is_none() {
            return self.error("No configuration file to edit (Rust-DOS started with --no-config)");
        }
        match host.autoexec() {
            Ok(lines) => {
                self.autoexec = Some(AutoexecEditor::new(&lines));
                self.status = None;
            }
            Err(e) => self.error(e),
        }
    }

    pub(super) fn autoexec_key(&mut self, key: UiKey, host: &mut dyn Host) {
        let Some(editor) = &mut self.autoexec else { return };
        match key {
            UiKey::Esc if editor.changed && !editor.leaving => {
                editor.leaving = true;
                self.error("Esc again drops the changes, F2 saves them");
            }
            UiKey::Esc => {
                self.autoexec = None;
                self.status = None;
            }
            UiKey::Save => match host.save_autoexec(&editor.lines()) {
                Ok(()) => {
                    self.autoexec = None;
                    let when = if self.active_game.is_some() {
                        "the game runs them the next time it is launched"
                    } else {
                        "they run the next time Rust-DOS starts"
                    };
                    self.info(format!("Saved the [autoexec] commands: {}", when));
                }
                Err(e) => self.error(e),
            },
            _ => {
                if editor.key(key, self.visible.saturating_sub(2)) && editor.leaving {
                    editor.leaving = false;
                    self.status = None;
                }
            }
        }
    }

    /// A click on line `line` of the editor, `col` columns into the text
    /// shown.
    pub(super) fn autoexec_clicked(&mut self, line: usize, col: usize) {
        if let Some(editor) = &mut self.autoexec {
            let col = editor.left + col;
            editor.go_to(line, col);
        }
    }

    pub(super) fn draw_autoexec(&mut self, g: &mut Grid, content: std::ops::Range<usize>) {
        let Some(editor) = &mut self.autoexec else { return };
        let cols = g.cols;
        let title = "[autoexec]: commands typed at the DOS prompt on startup";
        g.text_to(2, content.start, title, draw::BRIGHT, cols - 2);
        let list = content.start + 1..content.end;
        let (start, end) = (2, cols - 3);
        let width = end - start;
        Self::keep_visible(&mut editor.scroll, editor.line, list.len());
        if editor.col < editor.left {
            editor.left = editor.col;
        } else if editor.col >= editor.left + width {
            editor.left = editor.col + 1 - width;
        }
        for row in list.clone() {
            g.background(start, row, width, draw::FIELD);
        }
        for (i, row) in (editor.scroll..editor.lines.len()).zip(list.clone()) {
            let text: String = editor.lines[i].iter().skip(editor.left).take(width).collect();
            let comment = matches!(editor.lines[i].iter().find(|c| !c.is_whitespace()), Some('#' | ';'));
            g.text_to(start, row, &text, if comment { draw::DIM } else { draw::BRIGHT }, end);
            if i == editor.line {
                g.background(start + editor.col - editor.left, row, 1, draw::SELECT);
            }
            self.hits.push(Hit { row, col: start, width, target: Target::EditorLine(i) });
        }
        let (scroll, total) = (editor.scroll, editor.lines.len());
        self.draw_scrollbar(g, list, scroll, total);
    }
}
