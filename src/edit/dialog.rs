//! EDIT's dialog boxes: text fields, check boxes, lists and buttons in a
//! shadowed box, Tab moving between them, as EDIT.COM's dialogs work.

use super::keys::Key;
use super::screen::Screen;

/// The buttons of the dialogs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Ok,
    Cancel,
    Yes,
    No,
    FindVerify,
    ReplaceAll,
    Replace,
    Skip,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Control {
    /// A line of text, `width` wide at (`row`, `col`).
    Field { row: usize, col: usize, width: usize, text: Vec<u8>, cursor: usize },
    Check { row: usize, col: usize, label: String, on: bool },
    Button { row: usize, col: usize, label: String, id: Button },
    /// A framed list: its items show inside the frame at (`row`, `col`).
    List { row: usize, col: usize, width: usize, height: usize, items: Vec<String>, sel: usize, top: usize },
}

impl Control {
    pub fn field(row: usize, col: usize, width: usize, text: &[u8]) -> Self {
        Control::Field { row, col, width, text: text.to_vec(), cursor: text.len() }
    }

    pub fn check(row: usize, col: usize, label: &str, on: bool) -> Self {
        Control::Check { row, col, label: label.to_string(), on }
    }

    pub fn button(row: usize, col: usize, label: &str, id: Button) -> Self {
        Control::Button { row, col, label: label.to_string(), id }
    }

    pub fn list(row: usize, col: usize, width: usize, height: usize) -> Self {
        Control::List { row, col, width, height, items: Vec::new(), sel: 0, top: 0 }
    }

    /// The columns a button takes: "< label >".
    fn button_width(label: &str) -> usize {
        label.len() + 4
    }
}

/// What a key or click did in a dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    None,
    Press(Button),
    Cancel,
    /// Enter, or a click on the item selected, in the list that is
    /// control `n`.
    ListEnter(usize),
    /// The selection moved in the list that is control `n`.
    ListMove(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dialog<K> {
    /// What the dialog is for: what its buttons do.
    pub kind: K,
    pub title: String,
    pub width: usize,
    pub height: usize,
    /// The row it goes at; None for the middle of the screen.
    pub at_row: Option<usize>,
    /// Text in it: (row, col, text).
    pub labels: Vec<(usize, usize, String)>,
    /// Rows with a divider across.
    pub dividers: Vec<usize>,
    pub controls: Vec<Control>,
    pub focus: usize,
    /// Pressed by Enter where the focus isn't on a button.
    pub default: Button,
}

/// Dialog colours: black on grey, the hotkeys and the focused button's
/// brackets bright white.
const ATTR: u8 = 0x70;
const BRIGHT: u8 = 0x7F;
const FIELD: u8 = 0x07;

impl<K> Dialog<K> {
    pub fn new(kind: K, title: &str, width: usize, height: usize) -> Self {
        Self {
            kind,
            title: title.to_string(),
            width,
            height,
            at_row: None,
            labels: Vec::new(),
            dividers: Vec::new(),
            controls: Vec::new(),
            focus: 0,
            default: Button::Ok,
        }
    }

    pub fn label(mut self, row: usize, col: usize, text: &str) -> Self {
        self.labels.push((row, col, text.to_string()));
        self
    }

    pub fn control(mut self, control: Control) -> Self {
        self.controls.push(control);
        self
    }

    /// A divider at `row`, and centred on the row under it the buttons.
    pub fn buttons(mut self, row: usize, buttons: &[(&str, Button)]) -> Self {
        self.dividers.push(row);
        let total: usize = buttons.iter().map(|(l, _)| Control::button_width(l)).sum::<usize>() + 3 * (buttons.len() - 1);
        let mut col = self.width.saturating_sub(total) / 2;
        for &(label, id) in buttons {
            self.controls.push(Control::button(row + 1, col, label, id));
            col += Control::button_width(label) + 3;
        }
        self
    }

    /// A message box: lines of text and buttons, the first the default.
    pub fn message(kind: K, title: &str, lines: &[&str], buttons: &[(&str, Button)]) -> Self {
        let buttons_width: usize = buttons.iter().map(|(l, _)| l.len() + 7).sum();
        let width = lines.iter().map(|l| l.len() + 6).max().unwrap_or(0).max(buttons_width + 4).max(title.len() + 6);
        let mut dialog = Self::new(kind, title, width, lines.len() + 5);
        for (i, line) in lines.iter().enumerate() {
            dialog = dialog.label(i + 1, (width - line.len()) / 2, line);
        }
        dialog.default = buttons[0].1;
        let row = lines.len() + 2;
        let mut dialog = dialog.buttons(row, buttons);
        dialog.focus = dialog.controls.len() - buttons.len();
        dialog
    }

    /// The text of the field that is control `n`.
    pub fn text(&self, n: usize) -> Vec<u8> {
        match self.controls.get(n) {
            Some(Control::Field { text, .. }) => text.clone(),
            _ => Vec::new(),
        }
    }

    pub fn set_text(&mut self, n: usize, new: &[u8]) {
        if let Some(Control::Field { text, cursor, .. }) = self.controls.get_mut(n) {
            *text = new.to_vec();
            *cursor = text.len();
        }
    }

    pub fn checked(&self, n: usize) -> bool {
        matches!(self.controls.get(n), Some(Control::Check { on: true, .. }))
    }

    /// The selected item of the list that is control `n`.
    pub fn selected(&self, n: usize) -> Option<&str> {
        match self.controls.get(n) {
            Some(Control::List { items, sel, .. }) => items.get(*sel).map(|s| s.as_str()),
            _ => None,
        }
    }

    pub fn set_items(&mut self, n: usize, new: Vec<String>) {
        if let Some(Control::List { items, sel, top, .. }) = self.controls.get_mut(n) {
            *items = new;
            *sel = 0;
            *top = 0;
        }
    }

    /// The top left corner on a screen `cols` by `rows`.
    pub fn origin(&self, cols: usize, rows: usize) -> (usize, usize) {
        let row = self.at_row.unwrap_or(rows.saturating_sub(self.height) / 2);
        (row, cols.saturating_sub(self.width) / 2)
    }

    fn move_focus(&mut self, forward: bool) {
        let n = self.controls.len();
        if n > 0 {
            self.focus = if forward { (self.focus + 1) % n } else { (self.focus + n - 1) % n };
        }
    }

    /// The button Enter presses: the focused one, else the default.
    fn enter_button(&self) -> Button {
        match self.controls.get(self.focus) {
            Some(Control::Button { id, .. }) => *id,
            _ => self.default,
        }
    }

    /// What `key` does.
    pub fn key(&mut self, key: Key) -> Event {
        match key {
            Key::Esc => return Event::Cancel,
            Key::Tab => {
                self.move_focus(true);
                return Event::None;
            }
            Key::BackTab => {
                self.move_focus(false);
                return Event::None;
            }
            _ => {}
        }
        let focus = self.focus;
        let enter = self.enter_button();
        // A button's first letter presses it, but not while typing in a
        // field.
        let typing = matches!(self.controls.get(focus), Some(Control::Field { .. }));
        match self.controls.get_mut(focus) {
            Some(Control::Field { text, cursor, .. }) => match key {
                Key::Enter => return Event::Press(enter),
                Key::Char(c) => {
                    text.insert(*cursor, c);
                    *cursor += 1;
                }
                Key::Left => *cursor = cursor.saturating_sub(1),
                Key::Right => *cursor = (*cursor + 1).min(text.len()),
                Key::Home => *cursor = 0,
                Key::End => *cursor = text.len(),
                Key::Backspace if *cursor > 0 => {
                    *cursor -= 1;
                    text.remove(*cursor);
                }
                Key::Del if *cursor < text.len() => {
                    text.remove(*cursor);
                }
                _ => {}
            },
            Some(Control::Check { on, .. }) => match key {
                Key::Char(b' ') => *on = !*on,
                Key::Up | Key::Left | Key::Down | Key::Right => self.move_focus(matches!(key, Key::Down | Key::Right)),
                Key::Enter => return Event::Press(enter),
                _ => {}
            },
            Some(Control::List { items, sel, top, height, .. }) => {
                let before = *sel;
                let last = items.len().saturating_sub(1);
                match key {
                    Key::Enter => return if items.is_empty() { Event::None } else { Event::ListEnter(focus) },
                    Key::Up => *sel = sel.saturating_sub(1),
                    Key::Down => *sel = (*sel + 1).min(last),
                    Key::PgUp => *sel = sel.saturating_sub(*height),
                    Key::PgDn => *sel = (*sel + *height).min(last),
                    Key::Home => *sel = 0,
                    Key::End => *sel = last,
                    // A letter goes to the next item starting with it.
                    Key::Char(c) if c.is_ascii_graphic() => {
                        let n = items.len();
                        if let Some(i) = (1..=n).map(|i| (*sel + i) % n).find(|&i| {
                            items[i].trim_start_matches(['[', '-']).as_bytes().first().is_some_and(|f| f.eq_ignore_ascii_case(&c))
                        }) {
                            *sel = i;
                        }
                    }
                    _ => {}
                }
                if *sel < *top {
                    *top = *sel;
                } else if *sel >= *top + *height {
                    *top = *sel + 1 - *height;
                }
                if *sel != before {
                    return Event::ListMove(focus);
                }
            }
            Some(Control::Button { .. }) => match key {
                Key::Enter | Key::Char(b' ') => return Event::Press(enter),
                Key::Left | Key::Up => self.move_focus(false),
                Key::Right | Key::Down => self.move_focus(true),
                _ => {}
            },
            None => {}
        }
        if !typing && let Key::Char(c) | Key::Alt(c) = key {
            if let Some(id) = self.controls.iter().find_map(|control| match control {
                Control::Button { label, id, .. } if label.as_bytes()[0].eq_ignore_ascii_case(&c) => Some(*id),
                _ => None,
            }) {
                return Event::Press(id);
            }
        }
        Event::None
    }

    /// What a click at (`row`, `col`) of a screen `cols` by `rows` does.
    pub fn click(&mut self, row: usize, col: usize, cols: usize, rows: usize) -> Event {
        let (top, left) = self.origin(cols, rows);
        if row < top || col < left || row >= top + self.height || col >= left + self.width {
            return Event::None;
        }
        let (r, c) = (row - top, col - left);
        for (i, control) in self.controls.iter_mut().enumerate() {
            match control {
                Control::Field { row, col, width, text, cursor } if r == *row && c >= *col && c < *col + *width => {
                    *cursor = (c - *col).min(text.len());
                    self.focus = i;
                    return Event::None;
                }
                Control::Check { row, col, label, on } if r == *row && c >= *col && c < *col + label.len() + 4 => {
                    *on = !*on;
                    self.focus = i;
                    return Event::None;
                }
                Control::Button { row, col, label, id } if r == *row && c >= *col && c < *col + label.len() + 4 => {
                    self.focus = i;
                    return Event::Press(*id);
                }
                Control::List { row, col, width, height, items, sel, top } => {
                    if r >= *row && r < *row + *height && c >= *col && c < *col + *width {
                        let was_focused = self.focus == i;
                        self.focus = i;
                        let at = *top + (r - *row);
                        if at < items.len() {
                            if at == *sel && was_focused {
                                return Event::ListEnter(i);
                            }
                            *sel = at;
                            return Event::ListMove(i);
                        }
                        return Event::None;
                    }
                }
                _ => {}
            }
        }
        Event::None
    }

    pub fn draw(&self, s: &mut Screen) {
        let (top, left) = self.origin(s.cols, s.rows);
        s.frame(top, left, self.width, self.height, ATTR);
        s.shadow(top, left, self.width, self.height);
        if !self.title.is_empty() {
            let title = format!(" {} ", self.title);
            s.str(top, left + (self.width.saturating_sub(title.len())) / 2, &title, ATTR);
        }
        for &row in &self.dividers {
            s.divider(top + row, left, self.width, ATTR);
        }
        for (row, col, text) in &self.labels {
            s.str(top + row, left + col, text, ATTR);
        }
        let enter = self.enter_button();
        for (i, control) in self.controls.iter().enumerate() {
            let focused = i == self.focus;
            match control {
                Control::Field { row, col, width, text, cursor } => {
                    let first = (cursor + 1).saturating_sub(*width);
                    s.fill(top + row, left + col, *width, 1, b' ', FIELD);
                    s.text(top + row, left + col, &text[first.min(text.len())..text.len().min(first + width)], FIELD);
                }
                Control::Check { row, col, label, on } => {
                    s.str(top + row, left + col, &format!("[{}] {}", if *on { 'X' } else { ' ' }, label), ATTR);
                }
                Control::Button { row, col, label, id } => {
                    let bracket = if *id == enter { BRIGHT } else { ATTR };
                    s.set(top + row, left + col, b'<', bracket);
                    s.str(top + row, left + col + 2, label, ATTR);
                    s.set(top + row, left + col + label.len() + 3, b'>', bracket);
                    s.set(top + row, left + col + 2, label.as_bytes()[0], BRIGHT);
                }
                Control::List { row, col, width, height, items, sel, top: first } => {
                    s.frame(top + row - 1, left + col - 1, width + 2, height + 2, ATTR);
                    for (n, item) in items.iter().skip(*first).take(*height).enumerate() {
                        let attr = if focused && first + n == *sel { FIELD } else { ATTR };
                        s.fill(top + row + n, left + col, *width, 1, b' ', attr);
                        s.str(top + row + n, left + col + 1, &item.chars().take(width - 1).collect::<String>(), attr);
                    }
                }
            }
        }
    }

    /// Where the cursor goes for the focused control, on a screen `cols`
    /// by `rows`.
    pub fn cursor(&self, cols: usize, rows: usize) -> Option<(usize, usize)> {
        let (top, left) = self.origin(cols, rows);
        match self.controls.get(self.focus)? {
            Control::Field { row, col, width, cursor, .. } => {
                let first = (cursor + 1).saturating_sub(*width);
                Some((top + row, left + col + cursor - first))
            }
            Control::Check { row, col, .. } => Some((top + row, left + col + 1)),
            Control::Button { row, col, .. } => Some((top + row, left + col + 2)),
            Control::List { row, col, sel, top: first, .. } => Some((top + row + sel.saturating_sub(*first), left + col + 1)),
        }
    }
}
