//! The prompt's line editor, after clink's: the line is edited anywhere in
//! it, by character and by word, with undo, the command history and Tab
//! completion. The shell's code hands it every key typed at the prompt
//! (SERVICE_SHELL_KEY), and it keeps the line on the screen and in the
//! shell's buffer at DS:0200h.

pub mod complete;
pub mod editor;
pub mod history;
pub mod render;
pub mod settings;

use std::path::PathBuf;

use crate::cpu::Cpu;
use crate::dosstr;
use crate::edit::keys::{self, Key};
use crate::shell::{ShellWait, labels};
use editor::Line;

/// The line being typed at the prompt, and where it is on the screen.
#[derive(Clone, Debug, Default)]
pub struct LineEditor {
    pub line: Line,
    /// The cell the line begins on: where the cursor was after the prompt.
    anchor: usize,
    /// The attribute of the screen where the line is typed.
    attr: u8,
    /// The cells of the line on the screen, as last written.
    shown: Vec<(u8, u8)>,
    /// A line DATE or TIME asks for: no history and no completion.
    plain: bool,
    /// The line being typed before Up went to the history, for Down to
    /// come back to.
    draft: Option<Vec<u8>>,
    /// The cursor's shape before a block one showed overwriting.
    cursor_shape: Option<u16>,
}

/// In the shell's segment after the line's buffer: how many characters of
/// the line are after the cursor, and how many cells of it are on the
/// screen, from which `recover` finds the line again after a state is
/// loaded.
const AFTER_CURSOR: u16 = 0x0280;
const SHOWN: u16 = 0x0281;

impl LineEditor {
    /// The cells of the line: its characters in the screen's attribute.
    fn cells(&self) -> Vec<(u8, u8)> {
        self.line.text.iter().map(|&b| (b, self.attr)).collect()
    }

    /// The editor for the line in the shell's buffer, as a state saved
    /// while it was typed left it.
    fn recover(cpu: &mut Cpu) -> Self {
        let buffer = cpu.get_physical_addr(cpu.ds(), 0x0200);
        let len = (cpu.si() as usize).saturating_sub(0x0200).min(crate::shell::MAX_LINE);
        let text: Vec<u8> = (0..len).map(|i| cpu.bus.read_8(buffer + i)).collect();
        let after = (cpu.bus.read_8(cpu.get_physical_addr(cpu.ds(), AFTER_CURSOR)) as usize).min(len);
        let shown = (cpu.bus.read_8(cpu.get_physical_addr(cpu.ds(), SHOWN)) as usize).max(len);
        let cursor = len - after;
        LineEditor {
            line: Line::new(text, cursor),
            anchor: render::cursor_cell(cpu).saturating_sub(cursor),
            attr: 0x07,
            // Written again whole.
            shown: vec![(0, 0); shown],
            plain: matches!(cpu.shell_wait, Some(ShellWait::Line(_))),
            ..Default::default()
        }
    }

    /// Put the line in the shell's buffer, SI after it.
    fn store(&self, cpu: &mut Cpu) {
        let buffer = cpu.get_physical_addr(cpu.ds(), 0x0200);
        for (i, &b) in self.line.text.iter().enumerate() {
            cpu.bus.write_8(buffer + i, b);
        }
        cpu.set_si(0x0200 + self.line.text.len() as u16);
        let after = self.line.text.len() - self.line.cursor;
        cpu.bus.write_8(cpu.get_physical_addr(cpu.ds(), AFTER_CURSOR), after as u8);
        cpu.bus.write_8(cpu.get_physical_addr(cpu.ds(), SHOWN), self.shown.len().min(255) as u8);
    }
}

/// Where the history is kept on a desktop: `shell_history.txt` in the
/// per-user directory.
pub fn default_history_file() -> Option<PathBuf> {
    crate::config::user_dir().map(|d| d.join("shell_history.txt"))
}

/// Take the `[shell]` settings: the history is kept in the file the host
/// gave (`ShellHistory::set_home`) when they say to keep it.
pub fn configure(cpu: &mut Cpu, settings: &settings::ShellSettings) {
    cpu.shell_settings = *settings;
    cpu.shell_history.configure(settings.save_history, settings.history_size);
}

/// The prompt is on the screen (or isn't, with ECHO off): a new line
/// begins at the cursor.
pub fn start(cpu: &mut Cpu) {
    let saved = (cpu.ax(), cpu.bx(), cpu.cx(), cpu.dx());
    let ed = LineEditor {
        anchor: render::cursor_cell(cpu),
        attr: render::attribute_at_cursor(cpu),
        plain: matches!(cpu.shell_wait, Some(ShellWait::Line(_))),
        ..Default::default()
    };
    ed.store(cpu);
    cpu.line_editor = Some(ed);
    cpu.shell_history.reset();
    restore(cpu, saved);
}

/// The line is given up (batch lines came to run): the cursor goes after
/// all of it on the screen, and its shape is as it was.
pub fn finish(cpu: &mut Cpu) {
    let Some(mut ed) = cpu.line_editor.take() else { return };
    let saved = (cpu.ax(), cpu.bx(), cpu.cx(), cpu.dx());
    render::restore_cursor_shape(cpu, &mut ed);
    render::set_cursor(cpu, ed.anchor + ed.shown.len());
    restore(cpu, saved);
}

fn restore(cpu: &mut Cpu, (ax, bx, cx, dx): (u16, u16, u16, u16)) {
    cpu.set_ax(ax);
    cpu.set_reg16(iced_x86::Register::BX, bx);
    cpu.set_cx(cx);
    cpu.set_dx(dx);
}

/// What a key did to the line.
enum Done {
    /// The line is still being typed.
    No,
    /// Enter: the line is to run.
    Enter,
    /// Ctrl+C: it is given up.
    Break,
}

/// The shell's code at KEY_READ (SERVICE_SHELL_KEY), with a key typed at
/// the prompt in AX (INT 16h AH=10h's): the line edited, on the screen and
/// in the buffer at DS:0200h with SI after it. AL is 0Dh when Enter hands
/// the line over, 0 otherwise; BX, CX and DX stay as they were.
pub fn key(cpu: &mut Cpu) {
    let key = cpu.ax();
    let saved = (cpu.ax(), cpu.bx(), cpu.cx(), cpu.dx());
    let mut ed = match cpu.line_editor.take() {
        Some(ed) => ed,
        None => LineEditor::recover(cpu),
    };
    let done = edit(cpu, &mut ed, key);
    if !matches!(done, Done::No) {
        // The cursor after the line, which goes on in the rows below.
        ed.line.end();
    }
    render::draw(cpu, &mut ed);
    ed.store(cpu);
    restore(cpu, saved);
    match done {
        Done::No => {
            cpu.line_editor = Some(ed);
            cpu.set_ax(0);
        }
        Done::Enter => {
            render::restore_cursor_shape(cpu, &mut ed);
            cpu.set_ax(0x000D);
        }
        // Ctrl+C gives up the line (and DATE's or TIME's question, and the
        // batch files waiting) and starts again at a new prompt.
        Done::Break => {
            render::restore_cursor_shape(cpu, &mut ed);
            crate::video::print_string(cpu, "^C\r\n");
            cpu.shell_wait = None;
            cpu.batch.clear();
            cpu.set_ax(0);
            cpu.set_ip(labels().prompt_start);
        }
    }
}

/// Edit the line for `key`.
fn edit(cpu: &mut Cpu, ed: &mut LineEditor, key: u16) -> Done {
    let line = &mut ed.line;
    match keys::decode(key) {
        Key::Enter => return Done::Enter,
        Key::Ctrl(b'C') => return Done::Break,
        Key::Char(c) => {
            line.insert(c);
        }
        Key::Esc => line.replace(b""),
        Key::Backspace => line.backspace(),
        Key::CtrlBackspace => line.delete_word_back(),
        Key::Ctrl(b'W') => line.delete_blank_word_back(),
        Key::Del => line.delete(),
        Key::CtrlDel => line.delete_word(),
        Key::Left => line.left(),
        Key::Right => line.right(),
        Key::Home => line.home(),
        Key::End => line.end(),
        Key::CtrlLeft => line.word_left(),
        Key::CtrlRight => line.word_right(),
        Key::CtrlHome | Key::Ctrl(b'U') => line.delete_to_start(),
        Key::CtrlEnd | Key::Ctrl(b'K') => line.delete_to_end(),
        Key::Ins => line.toggle_overwrite(),
        Key::Ctrl(b'Z') => {
            line.undo();
        }
        _ if ed.plain => {}
        Key::Up | Key::F(5) => {
            if cpu.shell_history.at_newest() {
                ed.draft = Some(ed.line.text.clone());
            }
            if let Some(entry) = cpu.shell_history.older() {
                ed.line.replace(&dosstr::to_bytes(entry));
            }
        }
        // As clink's history-search-backward and -forward: the entries
        // beginning with the line before the cursor, which stays where it
        // is.
        Key::PgUp | Key::PgDn | Key::F(8) => {
            if cpu.shell_history.at_newest() {
                ed.draft = Some(ed.line.text.clone());
            }
            let prefix = dosstr::from_bytes(&ed.line.text[..ed.line.cursor]);
            let current = dosstr::from_bytes(&ed.line.text);
            let older = keys::decode(key) != Key::PgDn;
            if let Some(entry) = cpu.shell_history.search_prefix(&prefix, &current, older).map(dosstr::to_bytes) {
                let cursor = ed.line.cursor;
                ed.line.replace(&entry);
                ed.line.cursor = cursor;
            }
        }
        Key::Down => {
            if let Some(entry) = cpu.shell_history.newer().map(dosstr::to_bytes) {
                let text = match cpu.shell_history.at_newest() {
                    true => ed.draft.take().unwrap_or_default(),
                    false => entry,
                };
                ed.line.replace(&text);
            }
        }
        // F1 and F3, as in COMMAND.COM: the previous command's character
        // at the cursor, or all of it from the cursor on.
        Key::F(n @ (1 | 3)) => {
            if let Some(previous) = cpu.shell_history.entries().last().map(|e| dosstr::to_bytes(e)) {
                let at = line.cursor;
                if at < previous.len() {
                    let end = if n == 1 { at + 1 } else { previous.len() };
                    line.splice(at, end.min(line.text.len()).max(at), &previous[at..end]);
                }
            }
        }
        Key::Tab | Key::BackTab => {
            let forward = keys::decode(key) == Key::Tab;
            let (before, after) = line.text.split_at(line.cursor);
            let after = after.to_vec();
            if let Some(mut completed) = complete::complete(cpu, before, forward) {
                completed.truncate(crate::shell::MAX_LINE);
                let cursor = completed.len();
                completed.extend(after);
                ed.line.replace(&completed);
                ed.line.cursor = cursor.min(ed.line.text.len());
            }
        }
        _ => {}
    }
    Done::No
}
