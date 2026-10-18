//! Drawing RC's screen: the DOS screen behind, the panels, the command
//! line and the key bar, and whatever is open over them.

use crate::host::Host;
use crate::panel::{FRAME, HEADING, Layout, View, thousands};
use crate::screen::{Screen, centre, centre_left, pad};
use crate::viewer::draw_quick;
use crate::{CTRL, Commander, Mode, cp437, path};

const KEY_DIGITS: u8 = 0x07;
const KEY_LABEL: u8 = 0x30;
const HELP_BOX: u8 = 0x30;

/// What F1 shows.
pub const HELP_TEXT: &[&str] = &[
    "Rat Commander is a file manager in the style of the",
    "Norton Commander 4, built into rust-dos.",
    "",
    "Panels",
    "  Tab            The other panel",
    "  Enter          Into a directory, or run a program",
    "  Ctrl+PgUp      Up a directory",
    "  Ins            Mark or unmark a file",
    "  Gray + - *     Select, deselect or invert a group",
    "  Alt+letter     Find a name in the panel",
    "  Alt+F1 Alt+F2  Drive of the left or right panel",
    "  Ctrl+F1 F2     Left or right panel on or off",
    "  Ctrl+O         Panels on or off",
    "  Ctrl+P         The other panel on or off",
    "  Ctrl+U         Swap the panels",
    "  Ctrl+R         Read the directory again",
    "  Ctrl+L         Info in the other panel",
    "  Ctrl+Q         Quick view in the other panel",
    "  Ctrl+F3..F7    Sort by name, extension, time, size,",
    "                 or unsorted",
    "",
    "Files",
    "  F3             View the file",
    "  F4             Edit the file",
    "  F5             Copy",
    "  F6             Rename or move",
    "  F7             Make a directory",
    "  F8, Del        Delete",
    "  F9             The menus",
    "  F10            Quit",
    "",
    "Commands",
    "  Alt+F7         Find a file",
    "  Alt+F8         The command lines typed before",
    "  Alt+F10        The directory tree",
    "",
    "Command line",
    "  Enter          Run the line typed",
    "  Ctrl+Enter     The file's name onto the line",
    "  Ctrl+E Ctrl+X  The lines typed before",
    "  Esc            Clear the line",
    "",
    "The viewer",
    "  F2             Wrap or unwrap the lines",
    "  F4             Hex or text",
    "  F7             Search, Shift+F7 again",
    "  Esc, F3, F10   Back to the panels",
];

impl Commander {
    pub(crate) fn draw(&mut self, host: &mut dyn Host) {
        let mut s = Screen::new(self.cols, self.rows);
        if let Mode::View(v) = &mut self.mode {
            v.draw(&mut s);
            let labels = v.key_labels();
            draw_key_bar(&mut s, &labels);
            self.screen = s;
            return;
        }
        // The DOS screen, where the panels don't cover it.
        for r in 0..self.rows {
            for c in 0..self.cols {
                let (ch, a) = self.user.get(r, c);
                s.set(r, c, ch, a);
            }
        }
        for p in 0..2 {
            if self.shows(p) {
                self.draw_panel(host, &mut s, p);
            }
        }
        self.draw_command_line(host, &mut s);
        if self.options.key_bar {
            let labels = self.key_labels();
            draw_key_bar(&mut s, &labels);
        }
        if self.options.clock
            && self.shows(1)
            && !matches!(self.mode, Mode::Menu(_) | Mode::Help(_))
        {
            let (h, m, sec) = host.clock();
            let mut text = crate::panel::clock_text(h, m);
            if sec % 2 == 1 {
                text = text.replacen(':', " ", 1);
            }
            s.str(0, self.cols - 6, &text, 0x30);
            self.shown_second = Some(sec);
        }
        let checks = self.checks();
        let search_layout = self.layout(self.active);
        match &mut self.mode {
            Mode::Menu(bar) => bar.draw(&mut s, &checks),
            Mode::Dialog(d, _) => d.draw(&mut s),
            Mode::Job(job) => {
                job.progress.draw(&mut s);
                if let Some(q) = job.question() {
                    // The question goes under the progress box, as NC has it.
                    if q.top.is_none() {
                        q.top = Some(10);
                    }
                    q.draw(&mut s);
                }
                if s.cursor.is_none() {
                    s.cursor = None;
                }
            }
            Mode::Search(text) => {
                let l = search_layout;
                let r = l.h.saturating_sub(1);
                let x = l.x + 8;
                s.fill(r, x, 24, 3, b' ', 0x70);
                s.frame(r, x, 24, 3, 0x70, true);
                s.str(r + 1, x + 2, "Search:", 0x70);
                s.text(r + 1, x + 10, &pad(&cp437::bytes(text), 12), 0x30);
                s.cursor = Some((x + 10 + text.len().min(11), r + 1));
            }
            Mode::Help(top) => draw_help(&mut s, *top),
            Mode::Find(find) => find.dialog.draw(&mut s),
            Mode::Panels | Mode::View(_) => {}
        }
        self.screen = s;
    }

    fn key_labels(&self) -> [&'static str; 10] {
        if self.shift & crate::ALT != 0 {
            [
                "Left", "Right", "View..", "Edit..", "", "", "Find", "Histry", "", "Tree",
            ]
        } else if self.shift & CTRL != 0 {
            [
                "Left", "Right", "Name", "Exten", "Time", "Size", "Unsort", "", "", "",
            ]
        } else {
            [
                "Help", "Menu", "View", "Edit", "Copy", "RenMov", "Mkdir", "Delete", "PullDn",
                "Quit",
            ]
        }
    }

    fn draw_command_line(&self, host: &dyn Host, s: &mut Screen) {
        let row = self.command_row();
        s.fill(row, 0, self.cols, 1, b' ', 0x07);
        let prompt = if self.options.path_prompt {
            // The last line of a prompt of several ($_).
            let p = host.prompt();
            let last = p.split(|&b| b == b'\n').next_back().unwrap_or(&[]).to_vec();
            last.into_iter()
                .filter(|&b| b != b'\r')
                .collect::<Vec<u8>>()
        } else {
            let dir = &self.panels[self.active].dir;
            format!("{}>", &dir[..2]).into_bytes()
        };
        let room = self.cols.saturating_sub(prompt.len() + 1).max(1);
        let skip = (self.cmd_cursor + 1).saturating_sub(room);
        s.text(row, 0, &prompt, 0x07);
        let shown: Vec<u8> = self.cmdline.iter().skip(skip).take(room).copied().collect();
        s.text(row, prompt.len(), &shown, 0x07);
        s.cursor = Some((prompt.len() + self.cmd_cursor - skip, row));
    }

    fn draw_panel(&mut self, host: &mut dyn Host, s: &mut Screen, p: usize) {
        let l = self.layout(p);
        let active = p == self.active && !matches!(self.mode, Mode::Help(_));
        let panel = &self.panels[p];
        match panel.view {
            View::Brief | View::Full => {
                panel.draw_frame(s, &l, active, None);
                panel.draw_files(s, &l, active);
            }
            View::Tree => {
                let drive = panel.drive();
                panel.draw_frame(s, &l, active, Some("Tree"));
                let (cursor, top) = self.tree_pos[p];
                if let Some(t) = self.tree_of(drive) {
                    t.draw(s, &l, cursor, top, active);
                }
            }
            View::Info => {
                panel.draw_frame(s, &l, false, Some("Info"));
                self.draw_info(host, s, &l, 1 - p);
            }
            View::Quick => {
                panel.draw_frame(s, &l, false, Some("Text View"));
                let other = &self.panels[1 - p];
                let current = other.current().filter(|i| !i.is_up()).cloned();
                match current {
                    Some(item) if !item.is_dir() => {
                        let file = path::join(&other.dir, &item.entry.name);
                        if self.quick.as_ref().is_none_or(|(f, _)| *f != file) {
                            let data = host.read(&file).ok();
                            self.quick = Some((file.clone(), data));
                        }
                        let data = self.quick.as_ref().and_then(|(_, d)| d.as_deref());
                        draw_quick(s, &l, &item.entry.name, data, false);
                    }
                    Some(item) => {
                        draw_quick(s, &l, &item.entry.name.to_ascii_uppercase(), None, true)
                    }
                    None => {}
                }
            }
        }
    }

    /// The Info panel: memory, the other panel's drive and directory.
    fn draw_info(&self, host: &mut dyn Host, s: &mut Screen, l: &Layout, other: usize) {
        let w = l.w - 2;
        let x = l.x + 1;
        let panel = &self.panels[other];
        let drive = panel.drive();
        let letter = format!("{}:", (b'A' + drive) as char);
        let line = |s: &mut Screen, row: usize, text: String| {
            // Numbers and the drive in yellow, as NC has them.
            let bytes = centre_left(&cp437::bytes(&text), w);
            for (i, &b) in bytes.iter().enumerate() {
                s.set(row, x + i, b, FRAME);
            }
            let mut i = 0;
            while i < bytes.len() {
                let start = i;
                while i < bytes.len()
                    && (bytes[i].is_ascii_digit() || (bytes[i] == b',' && i > start))
                {
                    i += 1;
                }
                if i > start {
                    for c in start..i {
                        s.paint(row, x + c, HEADING);
                    }
                } else {
                    i += 1;
                }
            }
        };
        let version = format!("The Rat Commander, Version {}", env!("CARGO_PKG_VERSION"));
        s.text(1, x, &centre_left(version.as_bytes(), w), FRAME);
        s.text(2, x, &centre_left(b"Built into rust-dos", w), FRAME);
        s.divider(3, l.x, l.w, FRAME);
        let (total, free) = host.memory();
        line(s, 4, format!("{} Bytes Memory", thousands(total)));
        line(s, 5, format!("{} Bytes Free", thousands(free)));
        if let Some((size, avail)) = host.space(drive) {
            line(
                s,
                6,
                format!("{} total bytes on drive {}", thousands(size), letter),
            );
            line(
                s,
                7,
                format!("{} bytes free on drive {}", thousands(avail), letter),
            );
        }
        let files = panel.items.iter().filter(|i| !i.is_dir()).count();
        // NC counts ".." with the directories.
        let dirs = panel.items.iter().filter(|i| i.is_dir()).count();
        let bytes: u64 = panel
            .items
            .iter()
            .filter(|i| !i.is_dir())
            .map(|i| i.entry.size)
            .sum();
        line(
            s,
            8,
            format!(
                "{} file{} and {} director{}",
                files,
                if files == 1 { "" } else { "s" },
                dirs,
                if dirs == 1 { "y" } else { "ies" }
            ),
        );
        line(s, 9, format!("use {} bytes in", thousands(bytes)));
        s.text(10, x, &centre_left(&cp437::bytes(&panel.dir), w), FRAME);
        s.divider(11, l.x, l.w, FRAME);
        let label = host.label(drive).unwrap_or_default();
        s.str(12, x, " Volume Label : ", FRAME);
        if !label.is_empty() {
            s.str(12, x + 16, &format!(" {} ", label), HEADING);
        }
        s.divider(13, l.x, l.w, FRAME);
        s.str(15, x, "  No `dirinfo' file in this directory", FRAME);
    }
}

/// The ten function keys' labels on the bottom row.
pub fn draw_key_bar(s: &mut Screen, labels: &[&str; 10]) {
    let row = s.rows - 1;
    s.fill(row, 0, s.cols, 1, b' ', KEY_DIGITS);
    let width = (s.cols.saturating_sub(80)) / 10 + 6;
    let mut at = 0;
    for (i, label) in labels.iter().enumerate() {
        if i > 0 {
            at += 1;
        }
        let digits = (i + 1).to_string();
        s.str(row, at, &digits, KEY_DIGITS);
        at += digits.len();
        let w = if i == 9 {
            s.cols.saturating_sub(at)
        } else {
            width
        };
        s.text(row, at, &pad(label.as_bytes(), w), KEY_LABEL);
        at += w;
    }
}

/// F1's help, a box over the panels.
fn draw_help(s: &mut Screen, top: usize) {
    let w = 66.min(s.cols);
    let x = (s.cols - w) / 2;
    let y = 2;
    let h = s.rows.saturating_sub(4).max(8);
    s.fill(y, x, w, h, b' ', HELP_BOX);
    s.frame(y, x, w, h, HELP_BOX, true);
    s.title(y, x, w, "Help", HELP_BOX);
    let version = format!("The Rat Commander, Version {}", env!("CARGO_PKG_VERSION"));
    s.text(y + 1, x + 1, &centre(version.as_bytes(), w - 2), HELP_BOX);
    s.divider(y + 2, x, w, HELP_BOX);
    let rows = h - 4;
    for r in 0..rows {
        if let Some(line) = HELP_TEXT.get(top + r) {
            s.str(y + 3 + r, x + 2, line, HELP_BOX);
        }
    }
    s.str(y + h - 1, x + 2, " Esc ", HELP_BOX);
    s.cursor = None;
}
