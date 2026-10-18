//! A panel: a directory's files, how they are shown and sorted, which are
//! marked and where the cursor is.

use crate::host::{Entry, Host, attr};
use crate::path;
use crate::screen::{Screen, centre, centre_left, line, pad, right};
use rust_dos_savestate::{Reader, Result as StateResult, State, Writer};

/// The panel's colours: cyan on blue, the cursor black on cyan, marked
/// files yellow.
pub const FRAME: u8 = 0x1B;
pub const HEADING: u8 = 0x1E;
pub const CURSOR: u8 = 0x30;
pub const MARKED: u8 = 0x1E;
pub const MARKED_CURSOR: u8 = 0x3E;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum View {
    #[default]
    Brief,
    Full,
    Info,
    Tree,
    Quick,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    #[default]
    Name,
    Extension,
    Time,
    Size,
    Unsorted,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    Executables,
    Custom,
}

rust_dos_savestate::state_enum!(View {
    View::Brief,
    View::Full,
    View::Info,
    View::Tree,
    View::Quick,
});
rust_dos_savestate::state_enum!(Sort {
    Sort::Name,
    Sort::Extension,
    Sort::Time,
    Sort::Size,
    Sort::Unsorted,
});
rust_dos_savestate::state_enum!(Filter { Filter::All, Filter::Executables, Filter::Custom });

/// An entry of the panel, marked or not.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Item {
    pub entry: Entry,
    pub marked: bool,
}

impl Item {
    pub fn is_dir(&self) -> bool {
        self.entry.is_dir()
    }

    pub fn is_up(&self) -> bool {
        self.entry.name == ".."
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Panel {
    pub dir: String,
    pub view: View,
    pub sort: Sort,
    pub filter: Filter,
    /// The masks of a custom filter, separated by spaces, commas or
    /// semicolons.
    pub mask: String,
    pub visible: bool,
    pub items: Vec<Item>,
    pub cursor: usize,
    pub top: usize,
    /// The marks before the last operation that cleared them, for
    /// Restore selection.
    pub last_marks: Vec<String>,
    /// To be read again before it is shown.
    pub stale: bool,
    /// The name to put the cursor on, and the names to mark, once read.
    want_cursor: Option<String>,
    want_marks: Vec<String>,
}

impl Default for Panel {
    fn default() -> Self {
        Panel {
            dir: "C:\\".to_string(),
            view: View::Brief,
            sort: Sort::Name,
            filter: Filter::All,
            mask: "*.*".to_string(),
            visible: true,
            items: Vec::new(),
            cursor: 0,
            top: 0,
            last_marks: Vec::new(),
            stale: true,
            want_cursor: None,
            want_marks: Vec::new(),
        }
    }
}

/// The panel's settings, its cursor's file and its marks; the files are
/// read again.
impl State for Panel {
    fn save(&self, w: &mut Writer) {
        self.dir.save(w);
        self.view.save(w);
        self.sort.save(w);
        self.filter.save(w);
        self.mask.save(w);
        self.visible.save(w);
        let cursor = self
            .current()
            .map(|i| i.entry.name.clone())
            .unwrap_or_default();
        cursor.save(w);
        self.top.save(w);
        self.marked_names().save(w);
        self.last_marks.save(w);
    }

    fn load(&mut self, r: &mut Reader) -> StateResult<()> {
        self.dir.load(r)?;
        self.view.load(r)?;
        self.sort.load(r)?;
        self.filter.load(r)?;
        self.mask.load(r)?;
        self.visible.load(r)?;
        let mut cursor = String::new();
        cursor.load(r)?;
        self.top.load(r)?;
        self.want_marks.load(r)?;
        self.last_marks.load(r)?;
        self.want_cursor = Some(cursor);
        self.items.clear();
        self.stale = true;
        Ok(())
    }
}

/// The parts of a panel's frame: its rows and where its columns are.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub x: usize,
    pub w: usize,
    pub h: usize,
    /// Rows of files, from row 2.
    pub list: usize,
    pub status: bool,
}

impl Layout {
    pub fn new(x: usize, w: usize, h: usize, status: bool) -> Layout {
        let list = h.saturating_sub(if status { 5 } else { 3 }).max(1);
        Layout {
            x,
            w,
            h,
            list,
            status,
        }
    }

    /// The width of each of Brief's three columns.
    pub fn brief_cols(&self) -> [usize; 3] {
        let inner = self.w - 4;
        let each = inner / 3;
        [each, each, inner - 2 * each]
    }

    /// The width of Full's name column.
    pub fn name_width(&self) -> usize {
        (self.w - 2).saturating_sub(26).max(8)
    }
}

impl Panel {
    pub fn at(dir: &str) -> Panel {
        Panel {
            dir: dir.to_string(),
            ..Panel::default()
        }
    }

    pub fn drive(&self) -> u8 {
        path::drive(&self.dir)
    }

    pub fn current(&self) -> Option<&Item> {
        self.items.get(self.cursor)
    }

    pub fn marked_names(&self) -> Vec<String> {
        self.items
            .iter()
            .filter(|i| i.marked)
            .map(|i| i.entry.name.clone())
            .collect()
    }

    pub fn marked_count(&self) -> usize {
        self.items.iter().filter(|i| i.marked).count()
    }

    /// What an operation works on: the marked entries, else the one under
    /// the cursor ("..", never).
    pub fn selection(&self) -> Vec<Item> {
        let marked: Vec<Item> = self.items.iter().filter(|i| i.marked).cloned().collect();
        if !marked.is_empty() {
            return marked;
        }
        self.current()
            .filter(|i| !i.is_up())
            .cloned()
            .into_iter()
            .collect()
    }

    /// Go to directory `dir`, the cursor on `cursor`'s name if given.
    pub fn go(&mut self, dir: &str, cursor: Option<String>) {
        self.dir = dir.to_string();
        self.want_cursor = cursor;
        self.want_marks.clear();
        self.items.clear();
        self.cursor = 0;
        self.top = 0;
        self.stale = true;
    }

    /// Read the directory again, keeping the cursor's file and the marks.
    /// A directory that went away is left for the nearest one there is.
    /// An error when not even its drive's root can be read.
    pub fn refresh(&mut self, host: &mut dyn Host) -> Result<(), u8> {
        let cursor = self
            .want_cursor
            .take()
            .or_else(|| self.current().map(|i| i.entry.name.clone()));
        let mut marks = std::mem::take(&mut self.want_marks);
        if !self.items.is_empty() {
            marks = self.marked_names();
        }
        let mut dir = self.dir.clone();
        let list = loop {
            match host.list(&dir) {
                Ok(list) => break list,
                Err(e) => match path::parent(&dir) {
                    Some(up) => dir = up,
                    None => {
                        self.items.clear();
                        self.stale = false;
                        return Err(e);
                    }
                },
            }
        };
        self.dir = dir;
        let mut items: Vec<Item> = list
            .into_iter()
            .filter(|e| e.attr & 0x08 == 0 && e.name != "." && e.name != "..")
            .filter(|e| e.is_dir() || self.shows(&e.name))
            .map(|entry| Item {
                marked: marks.iter().any(|m| m.eq_ignore_ascii_case(&entry.name)),
                entry,
            })
            .collect();
        sort(&mut items, self.sort);
        if !path::is_root(&self.dir) {
            items.insert(
                0,
                Item {
                    entry: Entry {
                        name: "..".to_string(),
                        attr: attr::DIRECTORY,
                        ..Entry::default()
                    },
                    marked: false,
                },
            );
        }
        let old_cursor = self.cursor;
        self.items = items;
        self.cursor = cursor
            .and_then(|c| {
                self.items
                    .iter()
                    .position(|i| i.entry.name.eq_ignore_ascii_case(&c))
            })
            .unwrap_or(old_cursor.min(self.items.len().saturating_sub(1)));
        self.stale = false;
        Ok(())
    }

    /// Whether the filter shows a file.
    fn shows(&self, name: &str) -> bool {
        match self.filter {
            Filter::All => true,
            Filter::Executables => {
                let (_, ext) = path::split_name(name);
                ["EXE", "COM", "BAT"]
                    .iter()
                    .any(|e| ext.eq_ignore_ascii_case(e))
            }
            Filter::Custom => self
                .mask
                .split([' ', ',', ';'])
                .filter(|m| !m.is_empty())
                .any(|m| path::matches(m, name)),
        }
    }

    pub fn set_sort(&mut self, sort: Sort) {
        self.sort = sort;
        let name = self.current().map(|i| i.entry.name.clone());
        let up = self.items.first().is_some_and(Item::is_up);
        let start = usize::from(up);
        let mut rest = self.items.split_off(start);
        if sort == Sort::Unsorted {
            // The order the directory has comes back on reading it.
            self.items.append(&mut rest);
            self.want_cursor = name;
            self.stale = true;
            return;
        }
        crate::panel::sort(&mut rest, sort);
        self.items.append(&mut rest);
        if let Some(name) = name {
            self.cursor = self
                .items
                .iter()
                .position(|i| i.entry.name == name)
                .unwrap_or(0);
        }
    }

    /// How many entries a page shows.
    pub fn page(&self, layout: &Layout) -> usize {
        match self.view {
            View::Brief => layout.list * 3,
            _ => layout.list,
        }
    }

    /// Move the cursor by `delta`, keeping it in view.
    pub fn step(&mut self, delta: isize, layout: &Layout) {
        if self.items.is_empty() {
            return;
        }
        let last = self.items.len() as isize - 1;
        self.cursor = (self.cursor as isize + delta).clamp(0, last) as usize;
        self.scroll(layout);
    }

    pub fn home(&mut self, layout: &Layout) {
        self.cursor = 0;
        self.scroll(layout);
    }

    pub fn end(&mut self, layout: &Layout) {
        self.cursor = self.items.len().saturating_sub(1);
        self.scroll(layout);
    }

    /// The top entry moved so that the cursor shows. Brief scrolls by a
    /// column, Full by a row.
    pub fn scroll(&mut self, layout: &Layout) {
        let page = self.page(layout);
        let column = layout.list;
        if self.cursor < self.top {
            self.top = match self.view {
                View::Brief => self.cursor / column * column,
                _ => self.cursor,
            };
        } else if self.cursor >= self.top + page {
            self.top = match self.view {
                View::Brief => (self.cursor / column + 1).saturating_sub(3) * column,
                _ => self.cursor + 1 - page,
            };
        }
        // No empty space left at the end when the entries could fill it.
        let len = self.items.len();
        if self.top > 0 && self.top + page > len {
            let top = match self.view {
                View::Brief => len.div_ceil(column).saturating_sub(3) * column,
                _ => len.saturating_sub(page),
            };
            self.top = self.top.min(top);
        }
    }

    pub fn toggle_mark(&mut self) {
        if let Some(item) = self.items.get_mut(self.cursor)
            && !item.is_up()
        {
            item.marked = !item.marked;
        }
    }

    /// Mark (or unmark) the files that match a mask; directories with
    /// `dirs`.
    pub fn mark_matching(&mut self, mask: &str, on: bool, dirs: bool) {
        for item in self.items.iter_mut().filter(|i| !i.is_up()) {
            if (dirs || !item.is_dir())
                && mask
                    .split([' ', ',', ';'])
                    .filter(|m| !m.is_empty())
                    .any(|m| path::matches(m, &item.entry.name))
            {
                item.marked = on;
            }
        }
    }

    pub fn invert_marks(&mut self) {
        for item in self.items.iter_mut().filter(|i| !i.is_up() && !i.is_dir()) {
            item.marked = !item.marked;
        }
    }

    /// The marks gone, kept for Restore selection.
    pub fn clear_marks(&mut self) {
        let marks = self.marked_names();
        if !marks.is_empty() {
            self.last_marks = marks;
        }
        for item in &mut self.items {
            item.marked = false;
        }
    }

    pub fn restore_marks(&mut self) {
        let marks = self.last_marks.clone();
        for item in &mut self.items {
            item.marked = marks
                .iter()
                .any(|m| m.eq_ignore_ascii_case(&item.entry.name));
        }
    }

    /// The first entry from the cursor on whose name starts with `prefix`.
    pub fn find_prefix(&mut self, prefix: &str, layout: &Layout) -> bool {
        let prefix = prefix.to_ascii_uppercase();
        let n = self.items.len();
        for i in 0..n {
            let at = (self.cursor + i) % n;
            if self.items[at]
                .entry
                .name
                .to_ascii_uppercase()
                .starts_with(&prefix)
            {
                self.cursor = at;
                self.scroll(layout);
                return true;
            }
        }
        false
    }

    /// Which entry a cell of the panel shows, if one.
    pub fn hit(&self, layout: &Layout, col: usize, row: usize) -> Option<usize> {
        if row < 2 || row >= 2 + layout.list || col <= layout.x || col >= layout.x + layout.w - 1 {
            return None;
        }
        let r = row - 2;
        let index = match self.view {
            View::Brief => {
                let [a, b, _] = layout.brief_cols();
                let c = col - layout.x - 1;
                let column = if c <= a {
                    0
                } else if c <= a + 1 + b {
                    1
                } else {
                    2
                };
                self.top + column * layout.list + r
            }
            View::Full => self.top + r,
            _ => return None,
        };
        (index < self.items.len()).then_some(index)
    }

    // ----- Drawing -----

    /// The frame, with the path (or `title`) in its top edge, and the
    /// columns' lines and headings for Brief and Full.
    pub fn draw_frame(&self, s: &mut Screen, l: &Layout, active: bool, title: Option<&str>) {
        s.fill(0, l.x, l.w, l.h, b' ', FRAME);
        s.frame(0, l.x, l.w, l.h, FRAME, true);
        let separators: Vec<usize> = match self.view {
            View::Brief => {
                let [a, b, _] = l.brief_cols();
                vec![l.x + 1 + a, l.x + 2 + a + b]
            }
            View::Full => {
                let n = l.name_width();
                vec![l.x + 1 + n, l.x + 11 + n, l.x + 20 + n]
            }
            _ => Vec::new(),
        };
        for &c in &separators {
            s.set(0, c, line::DDOWN, FRAME);
            for r in 1..1 + l.list + 1 {
                s.set(r, c, line::V, FRAME);
            }
        }
        if l.status {
            let r = l.h - 3;
            s.divider(r, l.x, l.w, FRAME);
            for &c in &separators {
                s.set(r, c, line::UP, FRAME);
            }
        } else {
            for &c in &separators {
                s.set(l.h - 1, c, line::DUP, FRAME);
            }
        }
        match self.view {
            View::Brief => {
                let [a, b, c] = l.brief_cols();
                let mut at = l.x + 1;
                for w in [a, b, c] {
                    s.text(1, at, &centre_left(b"Name", w), HEADING);
                    at += w + 1;
                }
            }
            View::Full => {
                let n = l.name_width();
                let mut at = l.x + 1;
                for (text, w) in [("Name", n), ("Size", 9), ("Date", 8), ("Time", 6)] {
                    s.text(1, at, &centre_left(text.as_bytes(), w), HEADING);
                    at += w + 1;
                }
            }
            _ => {}
        }
        // The headings are yellow; the spaces around them aren't.
        for c in l.x + 1..l.x + l.w - 1 {
            if s.get(1, c).0 == b' ' {
                s.paint(1, c, FRAME);
            }
        }
        let title = match title {
            Some(t) => t.to_string(),
            None => shorten(&self.dir, l.w - 6),
        };
        let text = format!(" {} ", title);
        let len = text.chars().count();
        let at = l.x + (l.w - len) / 2;
        s.str(0, at, &text, if active { CURSOR } else { FRAME });
    }

    /// Brief or Full: the files, the cursor if `active`, and the mini
    /// status line.
    pub fn draw_files(&self, s: &mut Screen, l: &Layout, active: bool) {
        match self.view {
            View::Brief => {
                let widths = l.brief_cols();
                let mut x = l.x + 1;
                for (c, &w) in widths.iter().enumerate() {
                    for r in 0..l.list {
                        let i = self.top + c * l.list + r;
                        if let Some(item) = self.items.get(i) {
                            let attr = self.item_attr(i, active);
                            s.text(2 + r, x, &brief_name(item, w), attr);
                        }
                    }
                    x += w + 1;
                }
            }
            View::Full => {
                let n = l.name_width();
                for r in 0..l.list {
                    let i = self.top + r;
                    let Some(item) = self.items.get(i) else { break };
                    let attr = self.item_attr(i, active);
                    let x = l.x + 1;
                    s.text(2 + r, x, &brief_name(item, n), attr);
                    s.set(2 + r, x + n, line::V, attr);
                    s.text(2 + r, x + n + 1, &size_text(item, 9), attr);
                    s.set(2 + r, x + n + 10, line::V, attr);
                    s.text(2 + r, x + n + 11, &date_text(item.entry.date), attr);
                    s.set(2 + r, x + n + 19, line::V, attr);
                    s.text(2 + r, x + n + 20, &time_text(item.entry.time), attr);
                }
            }
            _ => {}
        }
        if l.status {
            self.draw_status(s, l);
        }
    }

    fn item_attr(&self, i: usize, active: bool) -> u8 {
        let marked = self.items[i].marked;
        match (active && i == self.cursor, marked) {
            (true, true) => MARKED_CURSOR,
            (true, false) => CURSOR,
            (false, true) => MARKED,
            (false, false) => FRAME,
        }
    }

    /// The mini status line: the cursor's file, or what is marked.
    fn draw_status(&self, s: &mut Screen, l: &Layout) {
        let r = l.h - 2;
        let w = l.w - 2;
        let marked: Vec<&Item> = self.items.iter().filter(|i| i.marked).collect();
        if !marked.is_empty() {
            let bytes: u64 = marked
                .iter()
                .filter(|i| !i.is_dir())
                .map(|i| i.entry.size)
                .sum();
            let n = marked.len();
            let text = format!(
                "{} bytes in {} selected file{}",
                thousands(bytes),
                n,
                if n == 1 { "" } else { "s" }
            );
            s.text(r, l.x + 1, &centre(text.as_bytes(), w), MARKED);
            return;
        }
        let Some(item) = self.current() else { return };
        let n = l.name_width();
        let mut line = pad(&crate::cp437::bytes(&shown_name(item, true)), n);
        line.push(b' ');
        line.extend(size_text(item, 9));
        line.push(b' ');
        line.extend(date_text(item.entry.date));
        line.push(b' ');
        line.extend(time_text(item.entry.time));
        s.text(r, l.x + 1, &pad(&line, w), FRAME);
    }
}

/// Directories first, ".." at the top, then by the order asked for.
pub fn sort(items: &mut [Item], sort: Sort) {
    if sort == Sort::Unsorted {
        items.sort_by_key(|i| !i.is_dir());
        return;
    }
    items.sort_by(|a, b| {
        let (an, ae) = path::split_name(&a.entry.name);
        let (bn, be) = path::split_name(&b.entry.name);
        let name = || {
            an.to_ascii_uppercase()
                .cmp(&bn.to_ascii_uppercase())
                .then(ae.to_ascii_uppercase().cmp(&be.to_ascii_uppercase()))
        };
        b.is_dir().cmp(&a.is_dir()).then_with(|| match sort {
            Sort::Extension => ae
                .to_ascii_uppercase()
                .cmp(&be.to_ascii_uppercase())
                .then_with(name),
            Sort::Time => (b.entry.date, b.entry.time)
                .cmp(&(a.entry.date, a.entry.time))
                .then_with(name),
            Sort::Size => b.entry.size.cmp(&a.entry.size).then_with(name),
            _ => name(),
        })
    });
}

/// A name as the panels show it: directories in upper case, files in
/// lower case, with a dot (`dotted`) or with the extension in a column of
/// its own.
pub fn shown_name(item: &Item, dotted: bool) -> String {
    if item.is_up() {
        return "..".to_string();
    }
    let name = if item.is_dir() {
        item.entry.name.to_ascii_uppercase()
    } else {
        item.entry.name.to_ascii_lowercase()
    };
    if dotted {
        return name;
    }
    let (base, ext) = path::split_name(&name);
    if ext.is_empty() {
        base.to_string()
    } else {
        format!("{:<8} {}", base, ext)
    }
}

fn brief_name(item: &Item, w: usize) -> Vec<u8> {
    let name = shown_name(item, false);
    let bytes = crate::cp437::bytes(&name);
    if bytes.len() > w {
        // Too long for the column: cut, with a mark that it is.
        let mut cut = bytes[..w - 1].to_vec();
        cut.push(b'}');
        return cut;
    }
    pad(&bytes, w)
}

/// The size column: the size, or ►SUB-DIR◄ and ►UP--DIR◄.
pub fn size_text(item: &Item, w: usize) -> Vec<u8> {
    if item.is_up() {
        return right("►UP--DIR◄", w);
    }
    if item.is_dir() {
        return right("►SUB-DIR◄", w);
    }
    let size = item.entry.size;
    if size.to_string().len() > w {
        return right(&format!("{}K", size / 1024), w);
    }
    right(&size.to_string(), w)
}

/// "mm-dd-yy", the month space-padded.
pub fn date_text(date: u16) -> Vec<u8> {
    let day = date & 0x1F;
    let month = (date >> 5) & 0x0F;
    let year = (1980 + (date >> 9)) % 100;
    format!("{:>2}-{:02}-{:02}", month, day, year).into_bytes()
}

/// "hh:mma", 12-hour, as NC shows times.
pub fn time_text(time: u16) -> Vec<u8> {
    let hour = time >> 11;
    let minute = (time >> 5) & 0x3F;
    clock_text(hour as u8, minute as u8).into_bytes()
}

pub fn clock_text(hour: u8, minute: u8) -> String {
    let h12 = match hour % 12 {
        0 => 12,
        h => h,
    };
    let half = if hour < 12 { 'a' } else { 'p' };
    format!("{:>2}:{:02}{}", h12, minute, half)
}

/// 1234567 as "1,234,567".
pub fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A path cut to `w` characters, keeping the drive and the end.
pub fn shorten(path: &str, w: usize) -> String {
    if path.chars().count() <= w || w < 8 {
        return path.to_string();
    }
    let tail: String = path
        .chars()
        .rev()
        .take(w - 6)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!(
        "{}\\...{}",
        &path[..2],
        &tail[tail.find('\\').unwrap_or(0)..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, dir: bool, size: u64, date: u16) -> Item {
        Item {
            entry: Entry {
                name: name.to_string(),
                size,
                attr: if dir { attr::DIRECTORY } else { 0 },
                time: 0,
                date,
            },
            marked: false,
        }
    }

    #[test]
    fn sorts_directories_first() {
        let mut items = vec![
            item("B.TXT", false, 5, 3),
            item("A.DOC", false, 9, 1),
            item("ZDIR", true, 0, 0),
            item("C.AAA", false, 1, 2),
        ];
        sort(&mut items, Sort::Name);
        let names: Vec<_> = items.iter().map(|i| i.entry.name.as_str()).collect();
        assert_eq!(names, ["ZDIR", "A.DOC", "B.TXT", "C.AAA"]);
        sort(&mut items, Sort::Extension);
        let names: Vec<_> = items.iter().map(|i| i.entry.name.as_str()).collect();
        assert_eq!(names, ["ZDIR", "C.AAA", "A.DOC", "B.TXT"]);
        sort(&mut items, Sort::Size);
        let names: Vec<_> = items.iter().map(|i| i.entry.name.as_str()).collect();
        assert_eq!(names, ["ZDIR", "A.DOC", "B.TXT", "C.AAA"]);
        sort(&mut items, Sort::Time);
        let names: Vec<_> = items.iter().map(|i| i.entry.name.as_str()).collect();
        assert_eq!(names, ["ZDIR", "B.TXT", "C.AAA", "A.DOC"]);
    }

    #[test]
    fn formats_like_nc() {
        assert_eq!(
            shown_name(&item("README.TXT", false, 0, 0), false),
            "readme   txt"
        );
        assert_eq!(shown_name(&item("games", true, 0, 0), false), "GAMES");
        assert_eq!(date_text((46 << 9) | (10 << 5) | 18), b"10-18-26");
        assert_eq!(date_text(0), b" 0-00-80");
        assert_eq!(time_text((11 << 11) | (43 << 5)), b"11:43a");
        assert_eq!(time_text(0), b"12:00a");
        assert_eq!(time_text(13 << 11), b" 1:00p");
        assert_eq!(thousands(7765), "7,765");
        assert_eq!(thousands(81920000), "81,920,000");
        assert_eq!(
            size_text(&item("X", true, 0, 0), 9),
            crate::cp437::bytes("►SUB-DIR◄")
        );
    }

    #[test]
    fn brief_scrolls_by_columns() {
        let l = Layout::new(0, 40, 23, true);
        assert_eq!(l.list, 18);
        assert_eq!(l.brief_cols(), [12, 12, 12]);
        let mut p = Panel {
            items: (0..100)
                .map(|i| item(&format!("F{}", i), false, 0, 0))
                .collect(),
            ..Panel::default()
        };
        p.step(54, &l);
        assert_eq!(p.top, 18);
        assert_eq!(p.hit(&l, 1, 2), Some(18));
        assert_eq!(p.hit(&l, 14, 3), Some(37));
        p.home(&l);
        assert_eq!(p.top, 0);
    }
}
