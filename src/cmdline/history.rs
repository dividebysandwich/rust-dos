//! The lines typed at the prompt, for Up and Down.

/// The lines typed at the prompt, oldest first, for Up and Down.
#[derive(Clone, Debug, Default)]
pub struct ShellHistory {
    entries: Vec<String>,
    /// The entry Up and Down are at: `entries.len()` is the new line
    /// below the newest.
    pos: usize,
}

impl ShellHistory {
    /// The lines kept.
    const MAX: usize = 100;

    /// A line was entered: keep it, unless it is empty or the one before
    /// again, and start again from below the newest.
    pub fn push(&mut self, line: &str) {
        if !line.is_empty() && self.entries.last().map(String::as_str) != Some(line) {
            self.entries.push(line.to_string());
            if self.entries.len() > Self::MAX {
                self.entries.remove(0);
            }
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

    pub fn entries(&self) -> &[String] {
        &self.entries
    }
}
