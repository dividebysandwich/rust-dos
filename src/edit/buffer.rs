//! EDIT's text: its lines, the cursor and the selection, and the edits
//! and searches on them. Columns may lie past a line's end, as in EDIT.COM:
//! typing there fills the gap with spaces.

/// A place in the text: a line and a column, from 0.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
}

impl Pos {
    pub fn new(line: usize, col: usize) -> Self {
        Self { line, col }
    }
}

/// How a search matches.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Search {
    pub text: Vec<u8>,
    /// Match Upper/Lowercase.
    pub case: bool,
    /// Whole Word.
    pub whole_word: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Buffer {
    /// Never empty: an empty text has one empty line.
    pub lines: Vec<Vec<u8>>,
    pub cursor: Pos,
    /// Where the selection started; it runs to the cursor.
    pub anchor: Option<Pos>,
    /// The first line and column shown.
    pub top: usize,
    pub left: usize,
    /// Changed since loaded or saved.
    pub dirty: bool,
    /// Typing replaces the character under the cursor (Ins).
    pub overwrite: bool,
}

impl Default for Buffer {
    fn default() -> Self {
        Self::from_text(b"")
    }
}

/// Spaces in place of tabs, to the next multiple of 8.
fn expand_tabs(line: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(line.len());
    for &b in line {
        if b == b'\t' {
            out.extend(std::iter::repeat_n(b' ', 8 - out.len() % 8));
        } else {
            out.push(b);
        }
    }
    out
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

impl Buffer {
    /// The text of a file: lines split at LF (with or without CR), up to
    /// an end of file mark (^Z), tabs expanded as EDIT.COM does.
    pub fn from_text(text: &[u8]) -> Self {
        let text = text.split(|&b| b == 0x1A).next().unwrap_or_default();
        let lines = text.split(|&b| b == b'\n').map(|line| expand_tabs(line.strip_suffix(b"\r").unwrap_or(line))).collect();
        Self { lines, cursor: Pos::default(), anchor: None, top: 0, left: 0, dirty: false, overwrite: false }
    }

    /// The text to save: the lines with CR LF between them.
    pub fn to_text(&self) -> Vec<u8> {
        self.lines.join(&b"\r\n"[..])
    }

    pub fn line(&self, n: usize) -> &[u8] {
        self.lines.get(n).map_or(&[], |l| l)
    }

    fn line_len(&self, n: usize) -> usize {
        self.line(n).len()
    }

    /// The last place in the text.
    pub fn end(&self) -> Pos {
        let last = self.lines.len() - 1;
        Pos::new(last, self.line_len(last))
    }

    // ----- Selection -----

    /// The selection, start before end; None when nothing is selected.
    pub fn selection(&self) -> Option<(Pos, Pos)> {
        let anchor = self.anchor?;
        match anchor.cmp(&self.cursor) {
            std::cmp::Ordering::Less => Some((anchor, self.cursor)),
            std::cmp::Ordering::Greater => Some((self.cursor, anchor)),
            std::cmp::Ordering::Equal => None,
        }
    }

    /// Whether the cell at `pos` is selected.
    pub fn is_selected(&self, pos: Pos) -> bool {
        self.selection().is_some_and(|(start, end)| start <= pos && pos < end)
    }

    /// The text from `start` to `end`, lines joined with CR LF; columns
    /// past a line's end are the spaces they'd be.
    pub fn text_between(&self, start: Pos, end: Pos) -> Vec<u8> {
        let mut out = Vec::new();
        for n in start.line..=end.line {
            let line = self.line(n);
            let from = if n == start.line { start.col } else { 0 };
            let to = if n == end.line { end.col } else { line.len() };
            for col in from..to {
                out.push(line.get(col).copied().unwrap_or(b' '));
            }
            if n != end.line {
                out.extend_from_slice(b"\r\n");
            }
        }
        out
    }

    pub fn selected_text(&self) -> Option<Vec<u8>> {
        self.selection().map(|(start, end)| self.text_between(start, end))
    }

    /// Take out the text from `start` to `end`; the cursor goes to
    /// `start`.
    fn remove(&mut self, start: Pos, end: Pos) {
        self.pad(start);
        let tail: Vec<u8> = self.line(end.line).get(end.col..).unwrap_or_default().to_vec();
        let line = &mut self.lines[start.line];
        line.truncate(start.col);
        line.extend_from_slice(&tail);
        self.lines.drain(start.line + 1..=end.line);
        self.cursor = start;
        self.anchor = None;
        self.dirty = true;
    }

    /// Delete the selection; false if there is none.
    pub fn delete_selection(&mut self) -> bool {
        match self.selection() {
            Some((start, end)) => {
                self.remove(start, end);
                true
            }
            None => {
                self.anchor = None;
                false
            }
        }
    }

    /// Spaces up to `pos.col` on its line, for typing past the end.
    fn pad(&mut self, pos: Pos) {
        let line = &mut self.lines[pos.line];
        if line.len() < pos.col {
            line.resize(pos.col, b' ');
        }
    }

    // ----- Editing -----

    /// Put `text` (lines separated by CR LF or LF) at the cursor, in place
    /// of the selection; the cursor ends after it.
    pub fn insert_text(&mut self, text: &[u8]) {
        self.delete_selection();
        let at = self.cursor;
        self.pad(at);
        let tail = self.lines[at.line].split_off(at.col);
        let mut pieces = text.split(|&b| b == b'\n').map(|p| p.strip_suffix(b"\r").unwrap_or(p).to_vec());
        let first = pieces.next().unwrap_or_default();
        self.lines[at.line].extend_from_slice(&first);
        let mut line = at.line;
        let mut col = at.col + first.len();
        for piece in pieces {
            line += 1;
            col = piece.len();
            self.lines.insert(line, piece);
        }
        self.lines[line].extend_from_slice(&tail);
        self.cursor = Pos::new(line, col);
        self.dirty = true;
    }

    /// A typed character: in place of the selection, inserted, or over
    /// the one under the cursor in overwrite mode.
    pub fn type_char(&mut self, c: u8) {
        let replaced = self.delete_selection();
        let at = self.cursor;
        self.pad(at);
        let line = &mut self.lines[at.line];
        if self.overwrite && !replaced && at.col < line.len() {
            line[at.col] = c;
        } else {
            line.insert(at.col, c);
        }
        self.cursor.col += 1;
        self.dirty = true;
    }

    /// Enter: the line splits at the cursor, and the new line starts
    /// under the first character of the one before, as EDIT.COM indents.
    pub fn newline(&mut self) {
        self.delete_selection();
        let at = self.cursor;
        let indent = self.line(at.line).iter().take_while(|&&b| b == b' ').count().min(at.col);
        self.pad(at);
        let mut tail = self.lines[at.line].split_off(at.col);
        // Blanks the cursor stood before don't move down.
        let skip = tail.iter().take_while(|&&b| b == b' ').count();
        tail.drain(..skip);
        let mut new = vec![b' '; indent];
        new.extend_from_slice(&tail);
        self.lines.insert(at.line + 1, new);
        self.cursor = Pos::new(at.line + 1, indent);
        self.dirty = true;
    }

    /// Backspace: the selection, or the character before the cursor, or
    /// the line break before it.
    pub fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        let at = self.cursor;
        if at.col > 0 {
            if at.col <= self.line_len(at.line) {
                self.lines[at.line].remove(at.col - 1);
                self.dirty = true;
            }
            self.cursor.col -= 1;
        } else if at.line > 0 {
            let end = Pos::new(at.line - 1, self.line_len(at.line - 1));
            self.remove(end, at);
        }
    }

    /// Del: the selection, or the character under the cursor, or the line
    /// break after it.
    pub fn delete(&mut self) {
        if self.delete_selection() {
            return;
        }
        let at = self.cursor;
        if at.col < self.line_len(at.line) {
            self.lines[at.line].remove(at.col);
            self.dirty = true;
        } else if at.line + 1 < self.lines.len() {
            self.remove(at, Pos::new(at.line + 1, 0));
        }
    }

    /// Ctrl+Y: the cursor's line, taken out whole; its text with the line
    /// break, for the clipboard.
    pub fn delete_line(&mut self) -> Vec<u8> {
        self.anchor = None;
        let n = self.cursor.line;
        let mut text = self.lines[n].clone();
        text.extend_from_slice(b"\r\n");
        if self.lines.len() > 1 {
            self.lines.remove(n);
            self.cursor.line = n.min(self.lines.len() - 1);
        } else {
            self.lines[0].clear();
        }
        self.cursor.col = 0;
        self.dirty = true;
        text
    }

    /// Tab: spaces to the next multiple of 8, typed or (overwriting)
    /// moved over.
    pub fn tab(&mut self) {
        let to = (self.cursor.col / 8 + 1) * 8;
        if self.overwrite && self.anchor.is_none() {
            self.cursor.col = to;
            return;
        }
        self.delete_selection();
        let n = to - self.cursor.col;
        self.insert_text(&vec![b' '; n]);
    }

    // ----- Moving -----

    /// Move the cursor to `pos`, selecting from where it was with `select`
    /// (Shift held).
    pub fn move_to(&mut self, pos: Pos, select: bool) {
        if select {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = pos;
    }

    pub fn left(&self) -> Pos {
        Pos::new(self.cursor.line, self.cursor.col.saturating_sub(1))
    }

    pub fn right(&self) -> Pos {
        Pos::new(self.cursor.line, self.cursor.col + 1)
    }

    pub fn up(&self, n: usize) -> Pos {
        Pos::new(self.cursor.line.saturating_sub(n), self.cursor.col)
    }

    pub fn down(&self, n: usize) -> Pos {
        Pos::new((self.cursor.line + n).min(self.lines.len() - 1), self.cursor.col)
    }

    /// Home: the line's first character that isn't a blank, or column 0
    /// from there.
    pub fn home(&self) -> Pos {
        let first = self.line(self.cursor.line).iter().take_while(|&&b| b == b' ').count();
        let first = if first == self.line_len(self.cursor.line) { 0 } else { first };
        Pos::new(self.cursor.line, if self.cursor.col == first { 0 } else { first })
    }

    pub fn line_end(&self) -> Pos {
        Pos::new(self.cursor.line, self.line_len(self.cursor.line))
    }

    /// Ctrl+Right: the start of the next word, over line ends.
    pub fn word_right(&self) -> Pos {
        let Pos { mut line, mut col } = self.cursor;
        let text = self.line(line);
        if col >= text.len() {
            if line + 1 >= self.lines.len() {
                return self.cursor;
            }
            line += 1;
            col = 0;
            let text = self.line(line);
            col += text.iter().take_while(|&&b| !is_word(b)).count();
            return Pos::new(line, col);
        }
        while col < text.len() && is_word(text[col]) {
            col += 1;
        }
        while col < text.len() && !is_word(text[col]) {
            col += 1;
        }
        Pos::new(line, col)
    }

    /// Ctrl+Left: the start of this word or the one before, over line
    /// starts.
    pub fn word_left(&self) -> Pos {
        let Pos { mut line, col } = self.cursor;
        let mut col = col.min(self.line_len(line));
        if col == 0 {
            if line == 0 {
                return Pos::new(0, 0);
            }
            line -= 1;
            col = self.line_len(line);
        }
        let text = self.line(line);
        while col > 0 && !is_word(text[col - 1]) {
            col -= 1;
        }
        while col > 0 && is_word(text[col - 1]) {
            col -= 1;
        }
        Pos::new(line, col)
    }

    /// The word at the cursor, for Find's text.
    pub fn word_at_cursor(&self) -> Vec<u8> {
        let text = self.line(self.cursor.line);
        let col = self.cursor.col.min(text.len());
        let start = col - text[..col].iter().rev().take_while(|&&b| is_word(b)).count();
        let end = col + text[col..].iter().take_while(|&&b| is_word(b)).count();
        text[start..end].to_vec()
    }

    /// Scroll so the cursor shows in a window `width` by `height`.
    pub fn scroll_to_cursor(&mut self, width: usize, height: usize) {
        let (width, height) = (width.max(1), height.max(1));
        if self.cursor.line < self.top {
            self.top = self.cursor.line;
        } else if self.cursor.line >= self.top + height {
            self.top = self.cursor.line + 1 - height;
        }
        if self.cursor.col < self.left {
            self.left = self.cursor.col;
        } else if self.cursor.col >= self.left + width {
            self.left = self.cursor.col + 1 - width;
        }
    }

    // ----- Searching -----

    /// Whether `search` matches at `col` of line `n`.
    fn matches_at(&self, n: usize, col: usize, search: &Search) -> bool {
        let line = self.line(n);
        let needle = &search.text;
        if needle.is_empty() || col + needle.len() > line.len() {
            return false;
        }
        let found = &line[col..col + needle.len()];
        let same = if search.case { found == &needle[..] } else { found.eq_ignore_ascii_case(needle) };
        if !same || !search.whole_word {
            return same;
        }
        let before = col == 0 || !is_word(line[col - 1]);
        let after = col + needle.len() >= line.len() || !is_word(line[col + needle.len()]);
        before && after
    }

    /// The first match at or after `from`, up to the end of the text.
    pub fn find_forward(&self, from: Pos, search: &Search) -> Option<Pos> {
        for n in from.line..self.lines.len() {
            let start = if n == from.line { from.col } else { 0 };
            for col in start..self.line_len(n) {
                if self.matches_at(n, col, search) {
                    return Some(Pos::new(n, col));
                }
            }
        }
        None
    }

    /// The next match after the cursor (after the selection's start, so
    /// a selected match isn't found again), going round to the top; it is
    /// selected.
    pub fn find_next(&mut self, search: &Search) -> bool {
        let from = match self.selection() {
            Some((start, _)) => Pos::new(start.line, start.col + 1),
            None => self.cursor,
        };
        let found = self.find_forward(from, search).or_else(|| self.find_forward(Pos::default(), search));
        match found {
            Some(at) => {
                self.select_match(at, search.text.len());
                true
            }
            None => false,
        }
    }

    /// Select `len` characters from `at`, the cursor after them.
    pub fn select_match(&mut self, at: Pos, len: usize) {
        self.anchor = Some(at);
        self.cursor = Pos::new(at.line, at.col + len);
    }

    /// Put `with` in place of the `len` characters at `at`.
    pub fn replace_at(&mut self, at: Pos, len: usize, with: &[u8]) {
        let line = &mut self.lines[at.line];
        line.splice(at.col..at.col + len, with.iter().copied());
        self.dirty = true;
    }

    /// Change All: every match, from the top; how many there were.
    pub fn replace_all(&mut self, search: &Search, with: &[u8]) -> usize {
        let mut count = 0;
        let mut at = Pos::default();
        while let Some(found) = self.find_forward(at, search) {
            self.replace_at(found, search.text.len(), with);
            at = Pos::new(found.line, found.col + with.len());
            count += 1;
        }
        if count > 0 {
            self.anchor = None;
            self.cursor = self.cursor.min(self.end());
        }
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(b: &Buffer) -> String {
        String::from_utf8(b.to_text()).unwrap()
    }

    #[test]
    fn loads_and_saves_lines() {
        let b = Buffer::from_text(b"one\r\ntwo\n\tx\r\n\x1Agarbage");
        assert_eq!(b.lines, vec![b"one".to_vec(), b"two".to_vec(), b"        x".to_vec(), vec![]]);
        assert_eq!(text(&b), "one\r\ntwo\r\n        x\r\n");
    }

    #[test]
    fn types_past_the_end_with_spaces() {
        let mut b = Buffer::from_text(b"ab");
        b.cursor = Pos::new(0, 4);
        b.type_char(b'x');
        assert_eq!(text(&b), "ab  x");
        assert!(b.dirty);
    }

    #[test]
    fn overwrite_replaces() {
        let mut b = Buffer::from_text(b"abc");
        b.overwrite = true;
        b.type_char(b'X');
        assert_eq!(text(&b), "Xbc");
    }

    #[test]
    fn enter_splits_and_indents() {
        let mut b = Buffer::from_text(b"  hello world");
        b.cursor = Pos::new(0, 7);
        b.newline();
        assert_eq!(text(&b), "  hello\r\n  world");
        assert_eq!(b.cursor, Pos::new(1, 2));
    }

    #[test]
    fn backspace_and_delete_join_lines() {
        let mut b = Buffer::from_text(b"ab\r\ncd");
        b.cursor = Pos::new(1, 0);
        b.backspace();
        assert_eq!(text(&b), "abcd");
        assert_eq!(b.cursor, Pos::new(0, 2));
        b.cursor = Pos::new(0, 4);
        b.insert_text(b"\r\nef");
        b.cursor = Pos::new(0, 4);
        b.delete();
        assert_eq!(text(&b), "abcdef");
    }

    #[test]
    fn selection_copy_cut_paste() {
        let mut b = Buffer::from_text(b"hello\r\nworld");
        b.cursor = Pos::new(0, 3);
        b.move_to(Pos::new(1, 2), true);
        assert_eq!(b.selected_text().unwrap(), b"lo\r\nwo");
        b.delete_selection();
        assert_eq!(text(&b), "helrld");
        b.insert_text(b"LO\r\nWO");
        assert_eq!(text(&b), "helLO\r\nWOrld");
        assert_eq!(b.cursor, Pos::new(1, 2));
    }

    #[test]
    fn words() {
        let b = Buffer { cursor: Pos::new(0, 0), ..Buffer::from_text(b"foo  bar.baz\r\nnext") };
        assert_eq!(b.word_right(), Pos::new(0, 5));
        let b = Buffer { cursor: Pos::new(0, 12), ..b };
        assert_eq!(b.word_right(), Pos::new(1, 0));
        assert_eq!(b.word_left(), Pos::new(0, 9));
        let b = Buffer { cursor: Pos::new(0, 6), ..b };
        assert_eq!(b.word_at_cursor(), b"bar");
    }

    #[test]
    fn find_goes_round_and_whole_words() {
        let mut b = Buffer::from_text(b"cat concat\r\nCat");
        let search = Search { text: b"cat".to_vec(), case: false, whole_word: false };
        b.cursor = Pos::new(0, 1);
        assert!(b.find_next(&search));
        assert_eq!(b.selection(), Some((Pos::new(0, 7), Pos::new(0, 10))));
        assert!(b.find_next(&search));
        assert_eq!(b.selection().unwrap().0, Pos::new(1, 0));
        assert!(b.find_next(&search));
        assert_eq!(b.selection().unwrap().0, Pos::new(0, 0));
        let whole = Search { whole_word: true, case: true, ..search };
        b.cursor = Pos::new(0, 1);
        b.anchor = None;
        assert!(b.find_next(&whole));
        assert_eq!(b.selection().unwrap().0, Pos::new(0, 0), "concat and Cat don't count");
    }

    #[test]
    fn replace_all_counts() {
        let mut b = Buffer::from_text(b"aXa\r\naa");
        let search = Search { text: b"a".to_vec(), case: true, whole_word: false };
        assert_eq!(b.replace_all(&search, b"aa"), 4);
        assert_eq!(text(&b), "aaXaa\r\naaaa");
    }

    #[test]
    fn delete_line() {
        let mut b = Buffer::from_text(b"one\r\ntwo");
        assert_eq!(b.delete_line(), b"one\r\n");
        assert_eq!(text(&b), "two");
    }
}
