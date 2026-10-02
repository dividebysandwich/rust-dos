//! The line being typed at the prompt and the edits the keys make to it,
//! apart from the screen: the bytes (code page 437), the cursor in them,
//! insert or overwrite, and the edits to undo.

use crate::shell::MAX_LINE;

/// What an edit was, for undo: a run of typed characters is one step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Last {
    Other,
    Typing,
}

#[derive(Clone, Debug)]
pub struct Line {
    pub text: Vec<u8>,
    /// Where the cursor is: before `text[cursor]`, at most `text.len()`.
    pub cursor: usize,
    pub overwrite: bool,
    undo: Vec<(Vec<u8>, usize)>,
    last: Last,
}

impl Default for Line {
    fn default() -> Self {
        Line { text: Vec::new(), cursor: 0, overwrite: false, undo: Vec::new(), last: Last::Other }
    }
}

/// The steps Ctrl+Z goes back through.
const UNDO_STEPS: usize = 100;

/// Whether `b` ends a word Ctrl+Backspace deletes: a blank, or a path's
/// separators.
fn path_separator(b: u8) -> bool {
    matches!(b, b' ' | b'\\' | b'/' | b':' | b';' | b',' | b'=')
}

impl Line {
    pub fn new(text: Vec<u8>, cursor: usize) -> Self {
        let cursor = cursor.min(text.len());
        Line { text, cursor, ..Default::default() }
    }

    /// Keep the line as it is for Ctrl+Z, before an edit of kind `last`.
    fn remember(&mut self, last: Last) {
        if last == Last::Typing && self.last == Last::Typing {
            return;
        }
        self.last = last;
        if self.undo.last().is_some_and(|(text, _)| *text == self.text) {
            return;
        }
        self.undo.push((self.text.clone(), self.cursor));
        if self.undo.len() > UNDO_STEPS {
            self.undo.remove(0);
        }
    }

    /// A move of the cursor: the typing after it is another step.
    fn moved(&mut self) {
        self.last = Last::Other;
    }

    /// Type `b` at the cursor, over the character there when overwriting.
    /// False when the line is full.
    pub fn insert(&mut self, b: u8) -> bool {
        let over = self.overwrite && self.cursor < self.text.len();
        if !over && self.text.len() >= MAX_LINE {
            return false;
        }
        // A blank ends a run of typing: undo takes words back.
        self.remember(if b == b' ' { Last::Other } else { Last::Typing });
        if b == b' ' {
            self.last = Last::Typing;
        }
        if over {
            self.text[self.cursor] = b;
        } else {
            self.text.insert(self.cursor, b);
        }
        self.cursor += 1;
        true
    }

    /// Type `bytes` at the cursor, as many as fit.
    pub fn insert_all(&mut self, bytes: &[u8]) {
        self.remember(Last::Other);
        for &b in bytes {
            let over = self.overwrite && self.cursor < self.text.len();
            if !over && self.text.len() >= MAX_LINE {
                break;
            }
            if over {
                self.text[self.cursor] = b;
            } else {
                self.text.insert(self.cursor, b);
            }
            self.cursor += 1;
        }
        self.last = Last::Other;
    }

    /// Put `text` in place of the line, the cursor at its end.
    pub fn replace(&mut self, text: &[u8]) {
        let text = &text[..text.len().min(MAX_LINE)];
        if text != self.text {
            self.remember(Last::Other);
        }
        self.text = text.to_vec();
        self.cursor = self.text.len();
        self.last = Last::Other;
    }

    /// Replace `from..to` with `with`, the cursor after it.
    pub fn splice(&mut self, from: usize, to: usize, with: &[u8]) {
        if from == to && with.is_empty() {
            return;
        }
        self.remember(Last::Other);
        self.text.splice(from..to, with.iter().copied());
        self.text.truncate(MAX_LINE);
        self.cursor = (from + with.len()).min(self.text.len());
        self.last = Last::Other;
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.splice(self.cursor - 1, self.cursor, &[]);
        }
    }

    pub fn delete(&mut self) {
        if self.cursor < self.text.len() {
            let at = self.cursor;
            self.splice(at, at + 1, &[]);
            self.cursor = at;
        }
    }

    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
        self.moved();
    }

    pub fn right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.text.len());
        self.moved();
    }

    pub fn home(&mut self) {
        self.cursor = 0;
        self.moved();
    }

    pub fn end(&mut self) {
        self.cursor = self.text.len();
        self.moved();
    }

    /// Where the word before `at` begins, words ending at a character
    /// `ends` is true for.
    fn word_start(&self, at: usize, ends: fn(u8) -> bool) -> usize {
        let mut i = at;
        while i > 0 && ends(self.text[i - 1]) {
            i -= 1;
        }
        while i > 0 && !ends(self.text[i - 1]) {
            i -= 1;
        }
        i
    }

    /// Where the next word begins after `at` (or the end).
    fn next_word(&self, at: usize) -> usize {
        let mut i = at;
        while i < self.text.len() && self.text[i] != b' ' {
            i += 1;
        }
        while i < self.text.len() && self.text[i] == b' ' {
            i += 1;
        }
        i
    }

    /// Ctrl+Left: to the start of this word, or of the one before.
    pub fn word_left(&mut self) {
        self.cursor = self.word_start(self.cursor, |b| b == b' ');
        self.moved();
    }

    /// Ctrl+Right: to the start of the next word.
    pub fn word_right(&mut self) {
        self.cursor = self.next_word(self.cursor);
        self.moved();
    }

    /// Ctrl+Backspace: the word before the cursor, up to a blank or a path
    /// separator.
    pub fn delete_word_back(&mut self) {
        let from = self.word_start(self.cursor, path_separator);
        self.splice(from, self.cursor, &[]);
    }

    /// Ctrl+W: the word before the cursor, up to a blank.
    pub fn delete_blank_word_back(&mut self) {
        let from = self.word_start(self.cursor, |b| b == b' ');
        self.splice(from, self.cursor, &[]);
    }

    /// Ctrl+Del: from the cursor to the start of the next word.
    pub fn delete_word(&mut self) {
        let (at, to) = (self.cursor, self.next_word(self.cursor));
        self.splice(at, to, &[]);
        self.cursor = at;
    }

    /// Ctrl+Home: everything before the cursor.
    pub fn delete_to_start(&mut self) {
        self.splice(0, self.cursor, &[]);
    }

    /// Ctrl+End: everything from the cursor on.
    pub fn delete_to_end(&mut self) {
        let at = self.cursor;
        self.splice(at, self.text.len(), &[]);
        self.cursor = at;
    }

    pub fn toggle_overwrite(&mut self) {
        self.overwrite = !self.overwrite;
    }

    /// Ctrl+Z: the line as it was before the last edit. False when there
    /// is none.
    pub fn undo(&mut self) -> bool {
        let Some((text, cursor)) = self.undo.pop() else { return false };
        self.text = text;
        self.cursor = cursor;
        self.last = Last::Other;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, cursor: usize) -> Line {
        Line::new(text.as_bytes().to_vec(), cursor)
    }

    fn text(l: &Line) -> &str {
        std::str::from_utf8(&l.text).unwrap()
    }

    #[test]
    fn types_and_deletes_anywhere() {
        let mut l = line("dr", 1);
        l.insert(b'i');
        assert_eq!((text(&l), l.cursor), ("dir", 2));
        l.backspace();
        assert_eq!((text(&l), l.cursor), ("dr", 1));
        l.delete();
        assert_eq!((text(&l), l.cursor), ("d", 1));
        l.home();
        l.toggle_overwrite();
        l.insert(b'x');
        l.insert(b'y');
        assert_eq!((text(&l), l.cursor), ("xy", 2));
    }

    #[test]
    fn moves_and_deletes_by_word() {
        let mut l = line("copy a\\b.txt c:\\x", 17);
        l.word_left();
        assert_eq!(l.cursor, 13);
        l.word_left();
        assert_eq!(l.cursor, 5);
        l.word_left();
        l.word_left();
        assert_eq!(l.cursor, 0);
        l.word_right();
        assert_eq!(l.cursor, 5);
        l.end();
        l.delete_word_back();
        assert_eq!(text(&l), "copy a\\b.txt c:\\");
        l.delete_blank_word_back();
        assert_eq!(text(&l), "copy a\\b.txt ");
        l.home();
        l.delete_word();
        assert_eq!((text(&l), l.cursor), ("a\\b.txt ", 0));
        l.right();
        l.delete_to_end();
        assert_eq!(text(&l), "a");
        l.insert_all(b"bc");
        l.left();
        l.delete_to_start();
        assert_eq!((text(&l), l.cursor), ("c", 0));
    }

    #[test]
    fn undo_takes_back_a_word_of_typing_at_a_time() {
        let mut l = Line::default();
        for &b in b"echo hi" {
            l.insert(b);
        }
        l.backspace();
        assert!(l.undo());
        assert_eq!(text(&l), "echo hi");
        assert!(l.undo());
        assert_eq!(text(&l), "echo");
        assert!(l.undo());
        assert_eq!(text(&l), "");
        assert!(!l.undo());
    }

    #[test]
    fn a_full_line_takes_no_more() {
        let mut l = line(&"x".repeat(MAX_LINE), MAX_LINE);
        assert!(!l.insert(b'y'));
        l.home();
        l.toggle_overwrite();
        assert!(l.insert(b'y'));
        assert_eq!(l.text.len(), MAX_LINE);
    }
}
