//! Ctrl+R and Ctrl+S: the history searched as text is typed, as readline's
//! reverse-i-search, shown in place of the line:
//! (reverse-i-search)`text': the line found

use super::editor::Line;
use super::{Done, LineEditor};
use crate::cpu::Cpu;
use crate::dosstr;
use crate::edit::keys::{self, Key};

#[derive(Clone, Debug)]
pub struct Search {
    /// What is searched for.
    text: Vec<u8>,
    /// The entry found, the line now.
    at: Option<usize>,
    /// Ctrl+R: towards the older entries.
    older: bool,
    /// Nothing has the text in it.
    failed: bool,
    /// The line before the search, for Esc to put back.
    saved: Line,
}

impl Search {
    pub fn new(line: &Line, older: bool) -> Self {
        Search { text: Vec::new(), at: None, older, failed: false, saved: line.clone() }
    }

    /// What shows before the line found.
    pub fn label(&self) -> Vec<u8> {
        let mut label = Vec::new();
        if self.failed {
            label.extend(b"(failed ");
        } else {
            label.push(b'(');
        }
        label.extend(if self.older { &b"reverse-i-search)`"[..] } else { &b"i-search)`"[..] });
        label.extend(&self.text);
        label.extend(b"': ");
        label
    }
}

/// Look for the search's text from entry `from` on, and put what has it
/// on the line, the cursor on the text in it.
fn find(cpu: &Cpu, ed: &mut LineEditor, from: usize) {
    let Some(search) = ed.search.as_mut() else { return };
    if search.text.is_empty() {
        return;
    }
    let text = dosstr::from_bytes(&search.text);
    match cpu.shell_history.search_text(&text, from, search.older) {
        Some((at, entry)) => {
            search.at = Some(at);
            search.failed = false;
            let entry = dosstr::to_bytes(entry);
            let lower = entry.to_ascii_lowercase();
            let wanted = search.text.to_ascii_lowercase();
            let cursor = lower.windows(wanted.len()).position(|w| w == wanted.as_slice()).unwrap_or(0);
            ed.line.replace(&entry);
            ed.line.cursor = cursor;
        }
        None => search.failed = true,
    }
}

/// A key while searching: Some when it is done with, None when the search
/// ends and the key edits the line found.
pub(super) fn key(cpu: &mut Cpu, ed: &mut LineEditor, key: u16) -> Option<Done> {
    let search = ed.search.as_mut()?;
    let newest = cpu.shell_history.entries().len().saturating_sub(1);
    let here = search.at.unwrap_or(if search.older { newest } else { 0 });
    match keys::decode(key) {
        Key::Char(c) => {
            search.text.push(c);
            find(cpu, ed, here);
        }
        Key::Backspace => {
            // Again from the newest.
            search.text.pop();
            search.at = None;
            search.failed = false;
            if search.text.is_empty() {
                ed.line = search.saved.clone();
            }
            let from = if search.older { newest } else { 0 };
            find(cpu, ed, from);
        }
        Key::Ctrl(c @ (b'R' | b'S')) => {
            let older = c == b'R';
            search.older = older;
            let next = match (search.at, older) {
                (Some(at), true) => at.checked_sub(1),
                (Some(at), false) => Some(at + 1),
                (None, _) => Some(here),
            };
            match next {
                Some(from) => find(cpu, ed, from),
                None => search.failed = true,
            }
        }
        Key::Esc | Key::Ctrl(b'G') => {
            ed.line = search.saved.clone();
            ed.search = None;
        }
        Key::Enter => {
            ed.search = None;
            return Some(Done::Enter);
        }
        Key::Ctrl(b'C') => {
            ed.search = None;
            return Some(Done::Break);
        }
        // Any other key takes the line found and edits it.
        _ => {
            ed.search = None;
            return None;
        }
    }
    Some(Done::No)
}
