//! F7: the history in a window over the screen, as clink's and COMMAND's
//! F7, the newest line at the bottom. Typing narrows it to the lines
//! with the text in them; Enter runs the line chosen, Tab puts it on the
//! prompt to edit, Del forgets it and Esc closes the window.

use super::{Done, LineEditor};
use crate::cpu::Cpu;
use crate::dosstr;
use crate::edit::keys::{self, Key};
use crate::edit::screen::Screen;
use crate::shell::video_call;

/// The window, and the selected line.
const WINDOW: u8 = 0x1F;
const SELECTED: u8 = 0x70;

#[derive(Clone, Debug)]
pub struct Popup {
    /// The screen under the window, as it was (cells of character and
    /// attribute).
    saved: Vec<u8>,
    cols: usize,
    rows: usize,
    /// The text the lines are narrowed to.
    filter: Vec<u8>,
    /// The history's entries shown, oldest first, by index.
    shown: Vec<usize>,
    /// The one selected, and the first in the window, in `shown`.
    selected: usize,
    top: usize,
    /// The cursor's shape, while it is hidden.
    cursor_shape: u16,
}

/// The text screen's memory of the page shown.
fn text_base(cpu: &Cpu) -> usize {
    cpu.bus.vga.text_window().0 + cpu.bus.read_16(0x044E) as usize
}

impl Popup {
    /// The rows of lines the window shows.
    fn height(&self) -> usize {
        self.rows.saturating_sub(6).max(1)
    }

    /// Narrow the lines to those with the filter's text in them, the
    /// newest selected.
    fn narrow(&mut self, cpu: &Cpu) {
        let filter = dosstr::from_bytes(&self.filter).to_ascii_lowercase();
        self.shown = cpu
            .shell_history
            .entries()
            .iter()
            .enumerate()
            .filter(|(_, e)| e.to_ascii_lowercase().contains(&filter))
            .map(|(i, _)| i)
            .collect();
        self.selected = self.shown.len().saturating_sub(1);
        self.top = self.shown.len().saturating_sub(self.height());
    }

    /// Move the selection by `by` lines, keeping it in the window.
    fn select(&mut self, by: isize) {
        let last = self.shown.len().saturating_sub(1) as isize;
        self.selected = (self.selected as isize + by).clamp(0, last.max(0)) as usize;
        let height = self.height();
        if self.selected < self.top {
            self.top = self.selected;
        } else if self.selected >= self.top + height {
            self.top = self.selected + 1 - height;
        }
    }

    /// The entry selected, if any line shows.
    fn entry(&self, cpu: &Cpu) -> Option<Vec<u8>> {
        let index = *self.shown.get(self.selected)?;
        cpu.shell_history.entries().get(index).map(|e| dosstr::to_bytes(e))
    }

    /// The window over the screen as it was.
    fn draw(&self, cpu: &mut Cpu) {
        let (cols, rows) = (self.cols, self.rows);
        let mut s = Screen::new(cols, rows);
        for row in 0..rows {
            for col in 0..cols {
                let at = (row * cols + col) * 2;
                s.set(row, col, self.saved[at], self.saved[at + 1]);
            }
        }
        let entries = cpu.shell_history.entries();
        let longest = self.shown.iter().map(|&i| entries[i].chars().count()).max().unwrap_or(0);
        let width = (longest + 4).clamp(38, cols.saturating_sub(2));
        let height = self.height().min(self.shown.len().max(1)) + 2;
        let (left, top) = ((cols - width) / 2, (rows.saturating_sub(height)) / 2);
        s.fill(top, left, width, height, b' ', WINDOW);
        s.frame(top, left, width, height, WINDOW);
        let title = if self.filter.is_empty() {
            " History ".to_string()
        } else {
            format!(" History: {} ", dosstr::from_bytes(&self.filter))
        };
        let title: String = title.chars().take(width - 4).collect();
        s.text(top, left + (width - title.chars().count()) / 2, &dosstr::to_bytes(&title), WINDOW);
        let help = " Enter run  Tab edit  Del forget ";
        if help.len() + 4 <= width {
            s.str(top + height - 1, left + (width - help.len()) / 2, help, WINDOW);
        }
        if self.shown.is_empty() {
            s.str(top + 1, left + 2, "(nothing)", WINDOW);
        }
        for (row, &index) in self.shown.iter().enumerate().skip(self.top).take(height - 2) {
            let attr = if row == self.selected { SELECTED } else { WINDOW };
            let y = top + 1 + row - self.top;
            s.fill(y, left + 1, width - 2, 1, b' ', attr);
            let text: Vec<u8> = dosstr::to_bytes(&entries[index]).into_iter().take(width - 4).collect();
            s.text(y, left + 2, &text, attr);
        }
        s.shadow(top, left, width, height);
        let base = text_base(cpu);
        for (i, b) in s.bytes().into_iter().enumerate() {
            cpu.bus.write_8(base + i, b);
        }
    }

    /// Put the screen back as it was, and the cursor's shape.
    pub(super) fn close(&self, cpu: &mut Cpu) {
        let base = text_base(cpu);
        for (i, &b) in self.saved.iter().enumerate() {
            cpu.bus.write_8(base + i, b);
        }
        video_call(cpu, 0x0100, 0, self.cursor_shape, 0);
    }
}

/// F7: open the window, in a text mode, when there is a history.
pub fn open(cpu: &mut Cpu, ed: &mut LineEditor) {
    if crate::video::pixels::graphics_mode(&cpu.bus) || cpu.shell_history.entries().is_empty() {
        return;
    }
    let (cols, rows) = (cpu.bus.text_cols().max(40), cpu.bus.text_rows().max(10));
    let base = text_base(cpu);
    let saved = (0..cols * rows * 2).map(|i| cpu.bus.read_8(base + i)).collect();
    let cursor_shape = cpu.bus.read_16(0x0460);
    let mut popup = Popup { saved, cols, rows, filter: Vec::new(), shown: Vec::new(), selected: 0, top: 0, cursor_shape };
    popup.narrow(cpu);
    popup.draw(cpu);
    // No cursor while it shows.
    video_call(cpu, 0x0100, 0, 0x2000, 0);
    ed.popup = Some(popup);
}

/// A key while the window shows: what it did to the line, the window
/// closed unless it is still open (`ed.popup`).
pub(super) fn key(cpu: &mut Cpu, ed: &mut LineEditor, key: u16) -> Option<Done> {
    let mut popup = ed.popup.take()?;
    let page = popup.height() as isize;
    let done = match keys::decode(key) {
        Key::Up => {
            popup.select(-1);
            None
        }
        Key::Down => {
            popup.select(1);
            None
        }
        Key::PgUp => {
            popup.select(-page);
            None
        }
        Key::PgDn => {
            popup.select(page);
            None
        }
        Key::Home | Key::CtrlHome => {
            popup.select(isize::MIN / 2);
            None
        }
        Key::End | Key::CtrlEnd => {
            popup.select(isize::MAX / 2);
            None
        }
        Key::Char(c) => {
            popup.filter.push(c);
            popup.narrow(cpu);
            None
        }
        Key::Backspace => {
            popup.filter.pop();
            popup.narrow(cpu);
            None
        }
        Key::Del => {
            if let Some(&index) = popup.shown.get(popup.selected) {
                let selected = popup.selected;
                cpu.shell_history.delete(index);
                popup.narrow(cpu);
                popup.selected = selected.min(popup.shown.len().saturating_sub(1));
                popup.select(0);
            }
            None
        }
        Key::Enter => match popup.entry(cpu) {
            Some(entry) => {
                ed.line.replace(&entry);
                Some(Done::Enter)
            }
            None => Some(Done::No),
        },
        Key::Tab | Key::Right | Key::F(7) => {
            if let Some(entry) = popup.entry(cpu) {
                ed.line.replace(&entry);
            }
            Some(Done::No)
        }
        Key::Esc | Key::Left => Some(Done::No),
        Key::Ctrl(b'C') => Some(Done::Break),
        _ => None,
    };
    match done {
        None => {
            popup.draw(cpu);
            ed.popup = Some(popup);
            Some(Done::No)
        }
        Some(done) => {
            popup.close(cpu);
            Some(done)
        }
    }
}
