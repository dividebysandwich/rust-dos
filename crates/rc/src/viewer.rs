//! F3's viewer: a file as text, wrapped or not, or in hex, with a search;
//! and the start of a file as the Quick view panel shows it.

use crate::cp437;
use crate::dialog::{Button, Dialog, Event, Item};
use crate::keys::Key;
use crate::panel::{FRAME, Layout, thousands};
use crate::screen::{Screen, pad, right};

const TEXT: u8 = 0x1B;
const TITLE: u8 = 0x30;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Viewer {
    pub path: String,
    data: Vec<u8>,
    hex: bool,
    wrap: bool,
    /// Where each line starts, for the width it was made for.
    lines: Vec<usize>,
    width: usize,
    /// The first line shown, and the first column when not wrapping.
    top: usize,
    col: usize,
    search: Option<Dialog>,
    message: Option<Dialog>,
    find: Vec<u8>,
}

/// What a key did to the viewer.
pub enum Done {
    Stay,
    Quit,
    Help,
}

impl Viewer {
    pub fn new(path: &str, data: Vec<u8>) -> Viewer {
        Viewer {
            path: path.to_string(),
            data,
            hex: false,
            wrap: true,
            lines: Vec::new(),
            width: 0,
            top: 0,
            col: 0,
            search: None,
            message: None,
            find: Vec::new(),
        }
    }

    /// The lines for a screen `width` wide, made again if it changed.
    fn layout(&mut self, width: usize) {
        if self.width == width && !self.lines.is_empty() {
            return;
        }
        self.width = width;
        self.lines = if self.hex {
            (0..self.data.len().div_ceil(16).max(1))
                .map(|i| i * 16)
                .collect()
        } else {
            text_lines(&self.data, if self.wrap { width } else { usize::MAX })
        };
    }

    fn relayout(&mut self, width: usize) {
        // The line the top byte is in stays on top.
        let at = self.lines.get(self.top).copied().unwrap_or(0);
        self.lines.clear();
        self.layout(width);
        self.top = self.lines.iter().rposition(|&l| l <= at).unwrap_or(0);
    }

    pub fn key(&mut self, key: Key, cols: usize, rows: usize) -> Done {
        self.layout(cols);
        if let Some(d) = &mut self.message {
            if !matches!(d.key(key), Event::None) {
                self.message = None;
            }
            return Done::Stay;
        }
        if let Some(d) = &mut self.search {
            match d.key(key) {
                Event::None => {}
                Event::Cancel => self.search = None,
                Event::Button(_) => {
                    self.find = cp437::bytes(&d.field(0));
                    self.search = None;
                    self.find_next(false);
                }
            }
            return Done::Stay;
        }
        let page = rows.saturating_sub(2).max(1);
        let last = self.lines.len().saturating_sub(page);
        match key {
            Key::Esc | Key::F(3) | Key::F(10) => return Done::Quit,
            Key::F(1) => return Done::Help,
            Key::Up => self.top = self.top.saturating_sub(1),
            Key::Down => self.top = (self.top + 1).min(last),
            Key::PgUp => self.top = self.top.saturating_sub(page),
            Key::PgDn | Key::Char(b' ') => self.top = (self.top + page).min(last),
            Key::Home => {
                self.top = 0;
                self.col = 0;
            }
            Key::End => self.top = last,
            Key::Left => self.col = self.col.saturating_sub(1),
            Key::Right if !self.wrap && !self.hex => self.col += 1,
            Key::CtrlLeft => self.col = self.col.saturating_sub(20),
            Key::CtrlRight if !self.wrap && !self.hex => self.col += 20,
            Key::F(2) => {
                self.wrap = !self.wrap;
                self.col = 0;
                self.relayout(cols);
            }
            Key::F(4) => {
                self.hex = !self.hex;
                self.relayout(cols);
            }
            Key::F(7) => {
                let mut d = Dialog::new(
                    "View",
                    68,
                    vec![
                        Item::text("Search for the string"),
                        Item::field(&cp437::string(&self.find)),
                    ],
                );
                d.top = Some(5);
                self.search = Some(d);
            }
            Key::ShiftF(7) => self.find_next(true),
            _ => {}
        }
        Done::Stay
    }

    /// The next line with the text searched for, without case.
    fn find_next(&mut self, again: bool) {
        if self.find.is_empty() {
            return;
        }
        let needle: Vec<u8> = self.find.iter().map(u8::to_ascii_uppercase).collect();
        let from = self
            .lines
            .get(self.top + usize::from(again))
            .copied()
            .unwrap_or(self.data.len());
        let from = if again {
            from
        } else {
            self.lines.get(self.top).copied().unwrap_or(0)
        };
        let hay = &self.data[from.min(self.data.len())..];
        let found = hay.windows(needle.len()).position(|w| {
            w.iter()
                .map(u8::to_ascii_uppercase)
                .eq(needle.iter().copied())
        });
        match found {
            Some(i) => {
                let at = from + i;
                self.top = self.lines.iter().rposition(|&l| l <= at).unwrap_or(0);
            }
            None => {
                self.message = Some(Dialog::message(
                    "Search",
                    &["Search string not found"],
                    &[("OK", Button::Ok)],
                    false,
                ))
            }
        }
    }

    /// The viewer's screen; the key bar is drawn by the caller.
    pub fn draw(&mut self, s: &mut Screen) {
        self.layout(s.cols);
        let rows = s.rows;
        s.fill(0, 0, s.cols, rows - 1, b' ', TEXT);
        // The title: the file, the column, its size, how far in.
        s.fill(0, 0, s.cols, 1, b' ', TITLE);
        s.str(0, 0, &format!("Text View: {}", self.path), TITLE);
        if s.cols >= 80 {
            s.str(0, 40, &format!("Col {}", self.col), TITLE);
            let size = format!("{} Bytes", thousands(self.data.len() as u64));
            s.text(0, 67 - size.len(), size.as_bytes(), TITLE);
        }
        let page = rows.saturating_sub(2);
        let pct = if self.top + page >= self.lines.len() {
            100
        } else {
            self.lines[self.top] * 100 / self.data.len().max(1)
        };
        s.text(0, s.cols - 4, &right(&format!("{}%", pct), 4), TITLE);
        for r in 0..page {
            let Some(&start) = self.lines.get(self.top + r) else {
                break;
            };
            let end = self
                .lines
                .get(self.top + r + 1)
                .copied()
                .unwrap_or(self.data.len());
            let text = if self.hex {
                hex_line(&self.data[start..end], start)
            } else {
                let cells = expand(&self.data[start..end]);
                cells.into_iter().skip(self.col).take(s.cols).collect()
            };
            s.text(1 + r, 0, &text, TEXT);
        }
        s.cursor = None;
        if let Some(d) = &mut self.search {
            d.draw(s);
        }
        if let Some(d) = &mut self.message {
            d.draw(s);
        }
    }

    /// The key bar's labels while viewing.
    pub fn key_labels(&self) -> [&'static str; 10] {
        [
            "Help",
            if self.wrap { "Unwrap" } else { "Wrap" },
            "",
            if self.hex { "ASCII" } else { "Hex" },
            "",
            "",
            "Search",
            "",
            "",
            "Quit",
        ]
    }
}

/// Where each line of a text starts, lines broken at LF and at `width`.
fn text_lines(data: &[u8], width: usize) -> Vec<usize> {
    let mut out = vec![0];
    let mut col = 0;
    for (i, &b) in data.iter().enumerate() {
        match b {
            b'\n' => {
                if i + 1 < data.len() {
                    out.push(i + 1);
                }
                col = 0;
            }
            b'\r' => {}
            b'\t' => col = (col / 8 + 1) * 8,
            _ => col += 1,
        }
        if col >= width && i + 1 < data.len() && data[i + 1] != b'\n' && data[i + 1] != b'\r' {
            out.push(i + 1);
            col = 0;
        }
    }
    out
}

/// A line's cells: tabs expanded, the line's end taken off.
fn expand(line: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for &b in line {
        match b {
            b'\n' | b'\r' => {}
            b'\t' => {
                let to = (out.len() / 8 + 1) * 8;
                out.resize(to, b' ');
            }
            0 => out.push(b' '),
            b => out.push(b),
        }
    }
    out
}

/// "00000  53 59 4D 41 │ 4E 54 45 43 │ ...   SYMANTEC LOCALIS"
fn hex_line(bytes: &[u8], offset: usize) -> Vec<u8> {
    let mut out = format!("{:05X}  ", offset).into_bytes();
    for i in 0..16 {
        match bytes.get(i) {
            Some(b) => out.extend(format!("{:02X} ", b).bytes()),
            None => out.extend(b"   "),
        }
        if i % 4 == 3 && i < 15 {
            out.extend([0xB3, b' ']);
        }
    }
    out.extend(b"  ");
    out.extend(bytes.iter().map(|&b| if b == 0 { b' ' } else { b }));
    out
}

/// The Quick view panel: the start of a file, or what a directory is.
pub fn draw_quick(s: &mut Screen, l: &Layout, name: &str, data: Option<&[u8]>, is_dir: bool) {
    let w = l.w - 2;
    let rows = l.list + 1;
    match data {
        Some(data) if !is_dir => {
            let lines = text_lines(data, w);
            for r in 0..rows {
                let Some(&start) = lines.get(r) else { break };
                let end = lines.get(r + 1).copied().unwrap_or(data.len());
                s.text(1 + r, l.x + 1, &pad(&expand(&data[start..end]), w), FRAME);
            }
            if l.status {
                let shown = lines.get(rows).copied().unwrap_or(data.len());
                let pct = shown * 100 / data.len().max(1);
                let mut line = pad(&cp437::bytes(&name.to_ascii_lowercase()), 12);
                line.extend(right(&data.len().to_string(), 12));
                line.extend(right(&format!("{}%", pct), 13));
                s.text(l.h - 2, l.x + 1, &pad(&line, w), FRAME);
            }
        }
        _ if is_dir => {
            s.str(3, l.x + 4, &format!("\"{}\" is a directory.", name), FRAME);
        }
        _ => {
            s.str(3, l.x + 4, "Cannot read the file.", FRAME);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_and_hex() {
        assert_eq!(text_lines(b"ab\r\ncd\r\n", 80), vec![0, 4]);
        assert_eq!(text_lines(b"abcdef", 4), vec![0, 4]);
        assert_eq!(expand(b"a\tb\r\n"), b"a       b");
        let line = hex_line(b"SYMANTEC LOCALIS", 0);
        assert_eq!(
            cp437::string(&line),
            "00000  53 59 4D 41 │ 4E 54 45 43 │ 20 4C 4F 43 │ 41 4C 49 53   SYMANTEC LOCALIS"
        );
    }

    #[test]
    fn wraps_and_searches() {
        let text: Vec<u8> = (0..100)
            .flat_map(|i| format!("line {}\r\n", i).into_bytes())
            .collect();
        let mut v = Viewer::new("C:\\X.TXT", text);
        let mut s = Screen::new(80, 25);
        v.draw(&mut s);
        assert_eq!(s.get(1, 0).0, b'l');
        v.key(Key::F(7), 80, 25);
        for c in b"line 50" {
            v.key(Key::Char(*c), 80, 25);
        }
        v.key(Key::Enter, 80, 25);
        assert_eq!(v.top, 50);
        v.key(Key::F(4), 80, 25);
        v.draw(&mut s);
        assert_eq!(s.get(1, 0).0, b'0');
    }
}
