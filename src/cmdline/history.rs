//! The lines typed at the prompt, for Up and Down, the searches and the
//! suggestions, kept in a file of the host's between sessions as clink
//! keeps its history: one line each, the newest last, appended to as
//! lines are entered.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::command::ShellCommand;
use crate::cpu::Cpu;
use crate::video::print_string;

/// The lines kept unless the configuration says otherwise.
pub const DEFAULT_SIZE: usize = 1000;

/// The lines typed at the prompt, oldest first.
#[derive(Clone, Debug)]
pub struct ShellHistory {
    entries: Vec<String>,
    /// The entry Up and Down are at: `entries.len()` is the new line
    /// below the newest.
    pos: usize,
    /// The lines kept: the oldest go past it.
    max: usize,
    /// The file the lines are kept in, if they are.
    file: Option<PathBuf>,
    /// The file they are kept in when they are to be (the host's).
    home: Option<PathBuf>,
}

impl Default for ShellHistory {
    fn default() -> Self {
        ShellHistory { entries: Vec::new(), pos: 0, max: DEFAULT_SIZE, file: None, home: None }
    }
}

/// Whether `entry` begins with `prefix`, in any case.
fn starts_with(entry: &str, prefix: &str) -> bool {
    entry.len() >= prefix.len() && entry.is_char_boundary(prefix.len()) && entry[..prefix.len()].eq_ignore_ascii_case(prefix)
}

impl ShellHistory {
    /// Where the lines are kept when they are to be (`configure`).
    pub fn set_home(&mut self, file: Option<PathBuf>) {
        self.home = file;
    }

    /// Keep `max` lines, in the file of `set_home` when `save` (otherwise
    /// only for this session). A file they weren't kept in before is
    /// read, with the lines typed so far after its own.
    pub fn configure(&mut self, save: bool, max: usize) {
        let file = self.home.clone().filter(|_| save);
        self.max = max.max(1);
        if file != self.file {
            self.file = file;
            if let Some(path) = self.file.clone() {
                let typed = std::mem::take(&mut self.entries);
                self.entries = read(&path);
                for line in typed {
                    self.keep(line);
                }
                self.trim();
                // The file again as kept: without the older copies of a
                // line, the oldest past the size, and with the lines typed
                // before it was read.
                if read_lines(&path) != self.entries.len() {
                    self.rewrite();
                }
            }
        } else if self.trim() {
            self.rewrite();
        }
        self.pos = self.entries.len();
    }

    /// Put `line` last, without its older copies.
    fn keep(&mut self, line: String) {
        self.entries.retain(|e| *e != line);
        self.entries.push(line);
        self.trim();
    }

    /// Drop the oldest lines past the most kept; whether there were any.
    fn trim(&mut self) -> bool {
        let over = self.entries.len().saturating_sub(self.max);
        self.entries.drain(..over);
        over > 0
    }

    /// A line was entered: keep it, last, unless it is empty or begins with
    /// a blank (as clink's history.ignore_space), and start again from
    /// below the newest.
    pub fn push(&mut self, line: &str) {
        if !line.is_empty() && !line.starts_with(' ') {
            self.keep(line.to_string());
            // The older copies, and the oldest lines past the size, go from
            // the file the next time it is read.
            self.append(line);
        }
        self.pos = self.entries.len();
    }

    /// Up: the entry before, if there is one.
    pub fn older(&mut self) -> Option<&str> {
        self.pos = self.pos.checked_sub(1)?;
        self.entries.get(self.pos).map(String::as_str)
    }

    /// Down: the entry after, or the empty new line after the newest; None
    /// when already there.
    pub fn newer(&mut self) -> Option<&str> {
        if self.pos >= self.entries.len() {
            return None;
        }
        self.pos += 1;
        Some(self.entries.get(self.pos).map_or("", String::as_str))
    }

    /// Whether Up and Down are at the new line below the newest entry.
    pub fn at_newest(&self) -> bool {
        self.pos >= self.entries.len()
    }

    /// Up and Down start again from below the newest entry.
    pub fn reset(&mut self) {
        self.pos = self.entries.len();
    }

    /// PgUp (`older`) or PgDn: the next entry from where Up and Down are
    /// that begins with `prefix` and isn't `current`, which they then are
    /// at.
    pub fn search_prefix(&mut self, prefix: &str, current: &str, older: bool) -> Option<&str> {
        let fits = |e: &String| starts_with(e, prefix) && !e.eq_ignore_ascii_case(current);
        let found = if older {
            self.entries[..self.pos.min(self.entries.len())].iter().rposition(fits)
        } else {
            let from = (self.pos + 1).min(self.entries.len());
            self.entries[from..].iter().position(fits).map(|i| from + i)
        }?;
        self.pos = found;
        Some(&self.entries[found])
    }

    /// Ctrl+R (`older`) or Ctrl+S: the entry from `from` on (backwards, or
    /// forwards) that has `text` in it, in any case, and its position.
    pub fn search_text(&self, text: &str, from: usize, older: bool) -> Option<(usize, &str)> {
        let text = text.to_ascii_lowercase();
        let fits = |e: &String| e.to_ascii_lowercase().contains(&text);
        let found = if older {
            self.entries[..(from + 1).min(self.entries.len())].iter().rposition(fits)
        } else {
            let from = from.min(self.entries.len());
            self.entries[from..].iter().position(fits).map(|i| from + i)
        }?;
        Some((found, &self.entries[found]))
    }

    /// The newest entry that begins with `prefix` and goes on after it.
    pub fn suggest(&self, prefix: &str) -> Option<&str> {
        self.entries.iter().rev().find(|e| e.len() > prefix.len() && starts_with(e, prefix)).map(String::as_str)
    }

    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    /// Forget entry `index`, and in the file.
    pub fn delete(&mut self, index: usize) -> bool {
        if index >= self.entries.len() {
            return false;
        }
        self.entries.remove(index);
        self.pos = self.entries.len();
        self.rewrite();
        true
    }

    /// Forget every entry, and in the file.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.pos = 0;
        self.rewrite();
    }

    /// Add `line` to the end of the file.
    fn append(&self, line: &str) {
        let Some(path) = &self.file else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let file = std::fs::OpenOptions::new().create(true).append(true).open(path);
        if let Ok(mut file) = file {
            let _ = writeln!(file, "{}", line);
        }
    }

    /// Write the file again with the entries as they are.
    fn rewrite(&self) {
        let Some(path) = &self.file else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let text: String = self.entries.iter().map(|e| format!("{}\n", e)).collect();
        let _ = std::fs::write(path, text);
    }
}

/// The lines of the file at `path`.
fn read_lines(path: &Path) -> usize {
    std::fs::read_to_string(path).map_or(0, |text| text.lines().count())
}

/// The entries kept in the file at `path`: its lines, the older copies of
/// a line left out.
fn read(path: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    let mut entries: Vec<String> = Vec::new();
    for line in text.lines().map(|l| l.trim_end_matches('\r')).filter(|l| !l.is_empty()) {
        entries.retain(|e| e != line);
        entries.push(line.to_string());
    }
    entries
}

/// HISTORY: the lines typed at the prompt, numbered; HISTORY n the last n
/// of them, HISTORY DELETE n forgets one and HISTORY CLEAR all.
pub struct HistoryCommand;

impl ShellCommand for HistoryCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        let words: Vec<&str> = args.split_whitespace().collect();
        let count = cpu.shell_history.entries().len();
        match words.as_slice() {
            [] => list(cpu, count),
            [help] if *help == "/?" => print_string(
                cpu,
                "Lists or changes the command history.\r\n\r\n\
                 HISTORY [n]\r\nHISTORY DELETE n\r\nHISTORY CLEAR\r\n\r\n\
                 \x20 n         Lists the last n lines (all of them without).\r\n\
                 \x20 DELETE n  Forgets line n.\r\n\
                 \x20 CLEAR     Forgets every line.\r\n\r\n\
                 Up and Down step through the lines, PgUp and PgDn through those\r\n\
                 beginning as the line does, Ctrl+R searches them, and F7 lists them.\r\n",
            ),
            [clear] if clear.eq_ignore_ascii_case("CLEAR") => cpu.shell_history.clear(),
            [delete, n] if delete.eq_ignore_ascii_case("DELETE") => match n.parse::<usize>() {
                Ok(n) if n >= 1 && cpu.shell_history.delete(n - 1) => {}
                _ => print_string(cpu, "Invalid history line number\r\n"),
            },
            [n] => match n.parse::<usize>() {
                Ok(n) => list(cpu, n),
                Err(_) => print_string(cpu, "Invalid parameter\r\n"),
            },
            _ => print_string(cpu, "Invalid parameter\r\n"),
        }
    }
}

/// Print the last `n` entries, with their numbers.
fn list(cpu: &mut Cpu, n: usize) {
    let entries = cpu.shell_history.entries();
    let from = entries.len().saturating_sub(n);
    let width = entries.len().to_string().len();
    let text: String = entries[from..]
        .iter()
        .enumerate()
        .map(|(i, e)| format!("{:>width$}  {}\r\n", from + i + 1, e, width = width))
        .collect();
    print_string(cpu, &text);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history(lines: &[&str]) -> ShellHistory {
        let mut h = ShellHistory::default();
        for line in lines {
            h.push(line);
        }
        h
    }

    #[test]
    fn keeps_a_line_once_and_not_those_beginning_with_a_blank() {
        let h = history(&["dir", "ver", "dir", " secret", ""]);
        assert_eq!(h.entries(), ["ver", "dir"]);
    }

    #[test]
    fn keeps_at_most_its_size() {
        let mut h = history(&["a", "b", "c"]);
        h.configure(true, 2);
        assert_eq!(h.entries(), ["b", "c"]);
        h.push("d");
        assert_eq!(h.entries(), ["c", "d"]);
    }

    #[test]
    fn searches_by_prefix_from_where_it_is() {
        let mut h = history(&["dir a", "cd x", "DIR b", "ver"]);
        assert_eq!(h.search_prefix("dir", "dir", true), Some("DIR b"));
        assert_eq!(h.search_prefix("dir", "DIR b", true), Some("dir a"));
        assert_eq!(h.search_prefix("dir", "dir a", true), None);
        assert_eq!(h.search_prefix("dir", "dir a", false), Some("DIR b"));
        assert_eq!(h.search_prefix("dir", "DIR b", false), None);
    }

    #[test]
    fn searches_for_text_anywhere() {
        let h = history(&["copy a.txt b", "dir", "type A.TXT"]);
        assert_eq!(h.search_text("a.t", 2, true), Some((2, "type A.TXT")));
        assert_eq!(h.search_text("a.t", 1, true), Some((0, "copy a.txt b")));
        assert_eq!(h.search_text("a.t", 1, false), Some((2, "type A.TXT")));
        assert_eq!(h.search_text("zz", 2, true), None);
    }

    #[test]
    fn suggests_the_newest_line_going_on() {
        let h = history(&["dir /w", "dir /p", "dir"]);
        assert_eq!(h.suggest("dir"), Some("dir /p"));
        assert_eq!(h.suggest("DIR /W"), None);
        assert_eq!(h.suggest("x"), None);
    }

    #[test]
    fn is_kept_in_a_file() {
        let dir = std::env::temp_dir().join(format!("rust-dos-history-{}", std::process::id()));
        let path = dir.join("history.txt");
        let _ = std::fs::remove_dir_all(&dir);
        let mut h = history(&["typed before"]);
        h.set_home(Some(path.clone()));
        h.configure(true, 3);
        h.push("one");
        h.push("two");
        h.push("one");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "typed before\none\ntwo\none\n");

        // Read back without the older copies and past the size.
        let mut again = ShellHistory::default();
        again.set_home(Some(path.clone()));
        again.configure(false, 2);
        assert!(again.entries().is_empty());
        again.configure(true, 2);
        assert_eq!(again.entries(), ["two", "one"]);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "two\none\n");
        again.delete(0);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\n");
        again.clear();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
