//! EDIT: a full-screen text editor in the style of MS-DOS EDIT.COM, built
//! into the shell. A menu bar (File, Edit, Search, Help), dialogs to open
//! and save files, cut, copy and paste, Find and Replace.
//!
//! It runs as one of the shell's waits (`ShellWait::Edit`): the shell's
//! code reads the keys and hands each one over (`key`), and on every tick
//! between them EDIT looks at the mouse and at Alt pressed on its own
//! (`tick`). It draws straight into the text screen's memory, and puts back
//! the screen it found when it ends.

mod buffer;
mod dialog;
mod keys;
mod menu;
mod screen;

pub use buffer::{Buffer, Pos, Search};

use crate::command::ShellCommand;
use crate::cpu::Cpu;
use crate::shell::{ShellWait, enter_wait, video_call as video};
use dialog::{Button, Control, Dialog, Event};
use keys::Key;
use menu::{Action, MENUS};
use screen::Screen;

/// The first row of text, under the menu bar and the window's top edge.
const TEXT_TOP: usize = 2;
/// The edit window: grey on blue, the selection inverted.
const TEXT: u8 = 0x17;
const SELECTED: u8 = 0x71;
const TITLE: u8 = 0x71;
const STATUS: u8 = 0x30;
const SCROLL: u8 = 0x70;

const SHIFT: u8 = 0x03;
const ALT: u8 = 0x08;

/// EDIT [file]: edit the file, or a new one.
pub struct EditCommand;

impl ShellCommand for EditCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        if args.contains("/?") {
            crate::video::print_string(
                cpu,
                "Edits text files.\r\n\r\nEDIT [[drive:][path]filename]\r\n\r\n\
                 Alt or F10 opens the menus; F1 lists the keys.\r\n",
            );
            return;
        }
        if cpu.stdin_redirect.is_some() {
            crate::video::print_string(cpu, "EDIT needs the keyboard\r\n");
            return;
        }
        // EDIT.COM's switches (/B /G /H /NOHI) change nothing here.
        let file = args.split_whitespace().find(|a| !a.starts_with('/'));
        match Editor::open(cpu, file) {
            Ok(editor) => enter_wait(cpu, ShellWait::Edit(editor)),
            Err(e) => crate::video::print_string(cpu, &format!("{}\r\n", e)),
        }
    }
}

/// What comes after a file is saved, or not, when the text has changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Then {
    Nothing,
    New,
    Open,
    Exit,
}

/// What a dialog is for.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Kind {
    /// The file dialogs: the directory they show, and the files they list.
    Open { dir: String, filter: String },
    SaveAs { dir: String, filter: String, then: Then },
    Find,
    Replace,
    /// Find and Verify, at a match: where it started and whether it went
    /// round to the top.
    Verify { origin: Pos, wrapped: bool },
    SaveChanges(Then),
    Overwrite { path: String, then: Then },
    Info,
}

/// The controls of the file dialogs and of Find and Replace, by position.
mod at {
    pub const NAME: usize = 0;
    pub const FILES: usize = 1;
    pub const DIRS: usize = 2;
    pub const FIND: usize = 0;
    pub const REPLACE_WITH: usize = 1;
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Mode {
    Edit,
    /// The menu bar active (Alt or F10), its titles' hotkeys showing.
    Bar(usize),
    /// A menu pulled down, and the item highlighted.
    Menu(usize, usize),
    Dialog(Box<Dialog<Kind>>),
}

/// What there was before EDIT: the screen, the cursor and the mouse.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Saved {
    /// The text screen's cells; empty if it was in a graphics mode.
    cells: Vec<u8>,
    /// Row in the high byte, column in the low, as the BIOS keeps it.
    cursor: u16,
    shape: u16,
    mouse_installed: bool,
    mouse_hidden: i32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Mouse {
    /// The left button's presses counted, once read.
    presses: Option<u16>,
    /// The left button went down in the text: moving selects.
    dragging: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Editor {
    buf: Buffer,
    /// The file, as a full DOS path; None for an untitled text.
    path: Option<String>,
    search: Search,
    replace_with: Vec<u8>,
    saved: Saved,
    mode: Mode,
    alt_held: bool,
    /// Alt went down with no key since: letting go of it opens the menu
    /// bar.
    alt_armed: bool,
    mouse: Mouse,
    quit: bool,
    /// The screen needs drawing: after a state is loaded.
    redraw: bool,
    cols: usize,
    rows: usize,
}

impl Default for Editor {
    fn default() -> Self {
        Self {
            buf: Buffer::default(),
            path: None,
            search: Search::default(),
            replace_with: Vec::new(),
            saved: Saved::default(),
            mode: Mode::Edit,
            alt_held: false,
            alt_armed: false,
            mouse: Mouse::default(),
            quit: false,
            redraw: true,
            cols: 80,
            rows: 25,
        }
    }
}

crate::state_fields!(Pos { line, col });
crate::state_fields!(Search { text, case, whole_word });
crate::state_fields!(Buffer { lines, cursor, anchor, top, left, dirty, overwrite });
crate::state_fields!(Saved { cells, cursor, shape, mouse_installed, mouse_hidden });
crate::state_fields!(Editor {
    buf, path, search, replace_with, saved,
} skip {
    // A dialog or menu open goes; the screen is drawn again.
    mode, alt_held, alt_armed, mouse, quit, redraw, cols, rows,
});

/// A key while EDIT waits for one: the editor to wait on with, or None
/// once it has ended.
pub fn key(cpu: &mut Cpu, mut editor: Box<Editor>, key: u16) -> Option<Box<Editor>> {
    editor.size(cpu);
    editor.alt_armed = false;
    let shift = cpu.bus.read_8(0x0417) & SHIFT != 0;
    editor.key(cpu, keys::decode(key), shift);
    finish(cpu, editor)
}

/// A tick while EDIT waits for a key: the mouse, and Alt on its own.
/// Whether EDIT goes on.
pub fn tick(cpu: &mut Cpu) -> bool {
    let Some(ShellWait::Edit(mut editor)) = cpu.shell_wait.take() else { return true };
    editor.size(cpu);
    let changed = editor.poll(cpu);
    if !editor.quit && !changed && !editor.redraw {
        cpu.shell_wait = Some(ShellWait::Edit(editor));
        return true;
    }
    match finish(cpu, editor) {
        Some(editor) => {
            cpu.shell_wait = Some(ShellWait::Edit(editor));
            true
        }
        None => false,
    }
}

/// After a key or a click: the screen drawn again, or put back as it was
/// if EDIT ended.
fn finish(cpu: &mut Cpu, mut editor: Box<Editor>) -> Option<Box<Editor>> {
    if editor.quit {
        editor.close(cpu);
        return None;
    }
    editor.draw(cpu);
    Some(editor)
}

/// What a DOS error code says.
fn error_text(code: u8) -> String {
    match code {
        0x02 => "File not found.".to_string(),
        0x03 => "Path not found.".to_string(),
        0x05 => "Path/File access error.".to_string(),
        0x13 => "Disk is write protected.".to_string(),
        code => format!("Disk error ({:02X}h).", code),
    }
}

/// `name` in directory `dir`, unless it has a drive or starts at a root.
fn join(dir: &str, name: &str) -> String {
    if name.contains(':') || name.starts_with('\\') || name.starts_with('/') {
        name.to_string()
    } else if dir.ends_with('\\') {
        format!("{}{}", dir, name)
    } else {
        format!("{}\\{}", dir, name)
    }
}

/// The directory and the name of a path.
fn split_path(path: &str) -> (String, String) {
    match path.rfind('\\') {
        Some(i) => {
            let dir = &path[..i];
            let dir = if dir.ends_with(':') { format!("{}\\", dir) } else { dir.to_string() };
            (dir, path[i + 1..].to_string())
        }
        None => (String::new(), path.to_string()),
    }
}

impl Editor {
    /// Start editing `file`, or an untitled text: the screen and the
    /// mouse kept to put back, and the editor drawn.
    fn open(cpu: &mut Cpu, file: Option<&str>) -> Result<Box<Editor>, String> {
        let mut editor = Box::<Editor>::default();
        if let Some(name) = file {
            let path = cpu.bus.disk.qualify_path(name).ok_or_else(|| "Path not found".to_string())?;
            if cpu.bus.disk.is_directory(&path) {
                return Err(format!("{} is a directory", path));
            }
            // A file that isn't there is a new one by that name.
            if cpu.bus.disk.is_file(&path) {
                editor.buf = Self::load(cpu, &path).map_err(|e| e.trim_end_matches('.').to_string())?;
            }
            editor.path = Some(path);
        }

        if crate::video::text::geometry(&cpu.bus).is_none() {
            crate::interrupts::int10::set_mode(cpu, 0x03);
        } else {
            editor.size(cpu);
            let base = Self::text_base(cpu);
            let mut cells = vec![0u8; editor.cols * editor.rows * 2];
            for (i, b) in cells.iter_mut().enumerate() {
                *b = cpu.bus.read_8(base + i);
            }
            editor.saved.cells = cells;
        }
        editor.size(cpu);
        let page = cpu.bus.read_8(0x0462) as usize;
        editor.saved.cursor = cpu.bus.read_16(0x0450 + page * 2);
        editor.saved.shape = cpu.bus.read_16(0x0460);
        let mouse = &mut cpu.bus.mouse;
        editor.saved.mouse_installed = mouse.installed;
        editor.saved.mouse_hidden = mouse.hide_counter;
        mouse.reset(editor.cols as i32 * 8, editor.rows as i32 * 8);
        mouse.hide_counter = 0;
        editor.mouse.presses = Some(mouse.press_count[0]);
        editor.draw(cpu);
        Ok(editor)
    }

    /// The text of a file.
    fn load(cpu: &Cpu, path: &str) -> Result<Buffer, String> {
        let data = cpu.bus.disk.file_data(path).ok_or_else(|| error_text(0x02))?;
        let bytes = data.read().map_err(|_| error_text(0x05))?;
        Ok(Buffer::from_text(&bytes))
    }

    /// Where the text screen's page shows in memory.
    fn text_base(cpu: &Cpu) -> usize {
        cpu.bus.vga.text_window().0 + cpu.bus.read_16(0x044E) as usize
    }

    fn size(&mut self, cpu: &Cpu) {
        self.cols = cpu.bus.text_cols().max(40);
        self.rows = cpu.bus.text_rows().max(10);
    }

    fn text_width(&self) -> usize {
        self.cols - 2
    }

    fn text_height(&self) -> usize {
        self.rows - 4
    }

    /// The screen, the cursor and the mouse as they were before.
    fn close(&mut self, cpu: &mut Cpu) {
        if self.saved.cells.is_empty() {
            crate::interrupts::int10::set_mode(cpu, 0x03);
        } else {
            let base = Self::text_base(cpu);
            for (i, &b) in self.saved.cells.iter().enumerate() {
                cpu.bus.write_8(base + i, b);
            }
            let page = cpu.bus.read_8(0x0462) as u16;
            video(cpu, 0x0200, page << 8, 0, self.saved.cursor);
        }
        video(cpu, 0x0100, 0, self.saved.shape, 0);
        let mouse = &mut cpu.bus.mouse;
        mouse.installed = self.saved.mouse_installed;
        mouse.hide_counter = self.saved.mouse_hidden;
    }

    // ----- Keys -----

    fn key(&mut self, cpu: &mut Cpu, key: Key, shift: bool) {
        match std::mem::replace(&mut self.mode, Mode::Edit) {
            Mode::Edit => self.edit_key(cpu, key, shift),
            Mode::Bar(n) => self.bar_key(cpu, n, key),
            Mode::Menu(n, item) => self.menu_key(cpu, n, item, key),
            Mode::Dialog(mut dialog) => {
                let event = dialog.key(key);
                self.dialog_event(cpu, dialog, event);
            }
        }
    }

    fn edit_key(&mut self, cpu: &mut Cpu, key: Key, shift: bool) {
        let height = self.text_height();
        let buf = &mut self.buf;
        match key {
            Key::Char(c) => buf.type_char(c),
            Key::Enter => buf.newline(),
            Key::Backspace => buf.backspace(),
            Key::Tab => buf.tab(),
            Key::Del if shift => self.run(cpu, Action::Cut),
            Key::Del => buf.delete(),
            Key::Ins if shift => self.run(cpu, Action::Paste),
            Key::Ins => buf.overwrite = !buf.overwrite,
            Key::CtrlIns => self.run(cpu, Action::Copy),
            Key::Ctrl(b'X') => self.run(cpu, Action::Cut),
            Key::Ctrl(b'C') => self.run(cpu, Action::Copy),
            Key::Ctrl(b'V') => self.run(cpu, Action::Paste),
            Key::Ctrl(b'Y') => cpu.edit_clipboard = buf.delete_line(),
            Key::Up => buf.move_to(buf.up(1), shift),
            Key::Down => buf.move_to(buf.down(1), shift),
            Key::Left => buf.move_to(buf.left(), shift),
            Key::Right => buf.move_to(buf.right(), shift),
            Key::Home => buf.move_to(buf.home(), shift),
            Key::End => buf.move_to(buf.line_end(), shift),
            Key::CtrlLeft => buf.move_to(buf.word_left(), shift),
            Key::CtrlRight => buf.move_to(buf.word_right(), shift),
            Key::CtrlHome => buf.move_to(Pos::default(), shift),
            Key::CtrlEnd => buf.move_to(buf.end(), shift),
            Key::PgUp => {
                buf.top = buf.top.saturating_sub(height);
                buf.move_to(buf.up(height), shift);
            }
            Key::PgDn => {
                buf.top = (buf.top + height).min(buf.lines.len().saturating_sub(1));
                buf.move_to(buf.down(height), shift);
            }
            Key::CtrlUp => self.scroll(-1),
            Key::CtrlDown => self.scroll(1),
            Key::F(1) => self.run(cpu, Action::Keyboard),
            Key::F(3) => self.run(cpu, Action::RepeatFind),
            Key::F(10) => self.mode = Mode::Bar(0),
            Key::Alt(c) => {
                if let Some(n) = MENUS.iter().position(|m| menu::hotkey(m.title) == c) {
                    self.mode = Mode::Menu(n, 0);
                }
            }
            _ => {}
        }
        if self.mode == Mode::Edit {
            let (width, height) = (self.text_width(), self.text_height());
            self.buf.scroll_to_cursor(width, height);
        }
    }

    /// The view and the cursor `lines` down (up, less than 0), as Ctrl+Up
    /// and Ctrl+Down and the scroll bar move them.
    fn scroll(&mut self, lines: isize) {
        let last = self.buf.lines.len() - 1;
        let height = self.text_height();
        let buf = &mut self.buf;
        buf.top = buf.top.saturating_add_signed(lines).min(last);
        buf.anchor = None;
        buf.cursor.line = buf.cursor.line.saturating_add_signed(lines).min(last);
        if buf.cursor.line < buf.top {
            buf.cursor.line = buf.top;
        } else if buf.cursor.line >= buf.top + height {
            buf.cursor.line = buf.top + height - 1;
        }
    }

    fn bar_key(&mut self, cpu: &mut Cpu, n: usize, key: Key) {
        let count = MENUS.len();
        self.mode = match key {
            Key::Left => Mode::Bar((n + count - 1) % count),
            Key::Right => Mode::Bar((n + 1) % count),
            Key::Enter | Key::Down | Key::Up => Mode::Menu(n, 0),
            Key::Esc | Key::F(10) => Mode::Edit,
            Key::F(1) => {
                self.run(cpu, Action::Keyboard);
                return;
            }
            Key::Char(c) | Key::Alt(c) => match MENUS.iter().position(|m| menu::hotkey(m.title) == c.to_ascii_uppercase()) {
                Some(n) => Mode::Menu(n, 0),
                None => Mode::Bar(n),
            },
            _ => Mode::Bar(n),
        };
    }

    fn menu_key(&mut self, cpu: &mut Cpu, n: usize, item: usize, key: Key) {
        let count = MENUS.len();
        let items = MENUS[n].items;
        self.mode = match key {
            Key::Left => Mode::Menu((n + count - 1) % count, 0),
            Key::Right => Mode::Menu((n + 1) % count, 0),
            Key::Up => Mode::Menu(n, menu::step(n, item, false)),
            Key::Down => Mode::Menu(n, menu::step(n, item, true)),
            Key::Home => Mode::Menu(n, 0),
            Key::Esc | Key::F(10) => Mode::Edit,
            Key::Enter => return self.choose(cpu, n, item),
            Key::Char(c) | Key::Alt(c) => {
                match items.iter().position(|i| i.label.is_some_and(|l| menu::hotkey(l) == c.to_ascii_uppercase())) {
                    Some(item) => return self.choose(cpu, n, item),
                    None => Mode::Menu(n, item),
                }
            }
            _ => Mode::Menu(n, item),
        };
    }

    /// Item `item` of menu `n` chosen: its action, if it applies.
    fn choose(&mut self, cpu: &mut Cpu, n: usize, item: usize) {
        let action = MENUS[n].items[item].action;
        if self.enabled(cpu, action) {
            self.mode = Mode::Edit;
            self.run(cpu, action);
            if self.mode == Mode::Edit {
                let (width, height) = (self.text_width(), self.text_height());
                self.buf.scroll_to_cursor(width, height);
            }
        } else {
            self.mode = Mode::Menu(n, item);
        }
    }

    fn enabled(&self, cpu: &Cpu, action: Action) -> bool {
        match action {
            Action::Cut | Action::Copy | Action::Clear => self.buf.selection().is_some(),
            Action::Paste => !cpu.edit_clipboard.is_empty(),
            _ => true,
        }
    }

    // ----- Actions -----

    fn run(&mut self, cpu: &mut Cpu, action: Action) {
        match action {
            Action::New => self.unless_unsaved(cpu, Then::New),
            Action::Open => self.unless_unsaved(cpu, Then::Open),
            Action::Exit => self.unless_unsaved(cpu, Then::Exit),
            Action::Save => self.save(cpu, Then::Nothing),
            Action::SaveAs => self.save_as_dialog(cpu, Then::Nothing),
            Action::Cut => {
                if let Some(text) = self.buf.selected_text() {
                    cpu.edit_clipboard = text;
                    self.buf.delete_selection();
                }
            }
            Action::Copy => {
                if let Some(text) = self.buf.selected_text() {
                    cpu.edit_clipboard = text;
                }
            }
            Action::Paste => {
                let text = cpu.edit_clipboard.clone();
                if !text.is_empty() {
                    self.buf.insert_text(&text);
                }
            }
            Action::Clear => {
                self.buf.delete_selection();
            }
            Action::Find => self.find_dialog(),
            Action::RepeatFind => {
                if self.search.text.is_empty() {
                    self.find_dialog();
                } else {
                    self.find_next();
                }
            }
            Action::Replace => self.replace_dialog(),
            Action::Keyboard => self.keyboard_help(),
            Action::About => {
                let version = format!("Version {}", env!("CARGO_PKG_VERSION"));
                self.info(&["Rust-DOS Editor", &version, "", "Built into Rust-DOS's shell."]);
            }
        }
    }

    /// Do `then`, after asking to save the text if it has changes.
    fn unless_unsaved(&mut self, cpu: &mut Cpu, then: Then) {
        if self.buf.dirty {
            let dialog = Dialog::message(
                Kind::SaveChanges(then),
                "",
                &["Loaded file is not saved. Save it now?"],
                &[("Yes", Button::Yes), ("No", Button::No), ("Cancel", Button::Cancel)],
            );
            self.mode = Mode::Dialog(Box::new(dialog));
        } else {
            self.proceed(cpu, then);
        }
    }

    fn proceed(&mut self, cpu: &mut Cpu, then: Then) {
        match then {
            Then::Nothing => {}
            Then::New => {
                self.buf = Buffer::default();
                self.path = None;
            }
            Then::Open => self.open_dialog(cpu),
            Then::Exit => self.quit = true,
        }
    }

    /// Save to the file, or ask for one; then `then`.
    fn save(&mut self, cpu: &mut Cpu, then: Then) {
        match self.path.clone() {
            Some(path) => {
                if self.write(cpu, &path) {
                    self.proceed(cpu, then);
                }
            }
            None => self.save_as_dialog(cpu, then),
        }
    }

    /// Write the text to `path`; false, with the error shown, if it can't
    /// be.
    fn write(&mut self, cpu: &mut Cpu, path: &str) -> bool {
        match cpu.bus.disk.write_whole_file(path, &self.buf.to_text(), None) {
            Ok(()) => {
                cpu.bus.log_string(&format!("[EDIT] Saved {}", path));
                self.buf.dirty = false;
                true
            }
            Err(code) => {
                self.info(&[&error_text(code)]);
                false
            }
        }
    }

    /// A message with an OK button.
    fn info(&mut self, lines: &[&str]) {
        let dialog = Dialog::message(Kind::Info, "", lines, &[("OK", Button::Ok)]);
        self.mode = Mode::Dialog(Box::new(dialog));
    }

    fn keyboard_help(&mut self) {
        const KEYS: [(&str, &str); 12] = [
            ("Arrows, PgUp, PgDn", "Move the cursor"),
            ("Home, End", "Start and end of the line"),
            ("Ctrl+Left, Ctrl+Right", "Word before and after"),
            ("Ctrl+Home, Ctrl+End", "Start and end of the file"),
            ("Shift+movement", "Select text"),
            ("Shift+Del, Ctrl+X", "Cut"),
            ("Ctrl+Ins, Ctrl+C", "Copy"),
            ("Shift+Ins, Ctrl+V", "Paste"),
            ("Ctrl+Y", "Delete the line"),
            ("Ins", "Insert or overwrite"),
            ("F3", "Repeat the last find"),
            ("Alt, F10", "The menus"),
        ];
        let mut dialog = Dialog::new(Kind::Info, "Keyboard", 58, KEYS.len() + 5);
        for (i, (keys, what)) in KEYS.iter().enumerate() {
            dialog = dialog.label(i + 1, 3, keys).label(i + 1, 27, what);
        }
        let mut dialog = dialog.buttons(KEYS.len() + 2, &[("OK", Button::Ok)]);
        dialog.focus = 0;
        self.mode = Mode::Dialog(Box::new(dialog));
    }

    // ----- Files -----

    /// The directory a file dialog starts in: the file's, or the current
    /// one.
    fn start_dir(&self, cpu: &Cpu) -> String {
        if let Some(path) = &self.path {
            return split_path(path).0;
        }
        let disk = &cpu.bus.disk;
        let drive = disk.get_current_drive();
        let dir = disk.get_current_directory();
        format!("{}:\\{}", crate::disk::drive_letter(drive), dir)
    }

    fn file_dialog(cpu: &Cpu, kind: Kind, title: &str, name: &[u8]) -> Dialog<Kind> {
        let dialog = Dialog::new(kind, title, 64, 20)
            .control(Control::field(2, 14, 46, name))
            .control(Control::list(8, 3, 34, 8))
            .control(Control::list(8, 42, 18, 8));
        let mut dialog = dialog.buttons(17, &[("OK", Button::Ok), ("Cancel", Button::Cancel)]);
        dialog.focus = at::NAME;
        Self::refresh(cpu, &mut dialog);
        dialog
    }

    fn open_dialog(&mut self, cpu: &mut Cpu) {
        let kind = Kind::Open { dir: self.start_dir(cpu), filter: "*.TXT".to_string() };
        self.mode = Mode::Dialog(Box::new(Self::file_dialog(cpu, kind, "Open", b"*.TXT")));
    }

    fn save_as_dialog(&mut self, cpu: &mut Cpu, then: Then) {
        let name = self.path.as_deref().map(|p| split_path(p).1).unwrap_or_default();
        let kind = Kind::SaveAs { dir: self.start_dir(cpu), filter: "*.*".to_string(), then };
        self.mode = Mode::Dialog(Box::new(Self::file_dialog(cpu, kind, "Save As", name.as_bytes())));
    }

    /// The file dialog's lists and directory for the directory and files
    /// it is at.
    fn refresh(cpu: &Cpu, dialog: &mut Dialog<Kind>) {
        let (dir, filter) = match &dialog.kind {
            Kind::Open { dir, filter } | Kind::SaveAs { dir, filter, .. } => (dir.clone(), filter.clone()),
            _ => return,
        };
        let disk = &cpu.bus.disk;
        let mut files: Vec<String> = disk
            .list_directory(&join(&dir, &filter), 0)
            .unwrap_or_default()
            .into_iter()
            .filter(|e| !e.is_dir)
            .map(|e| e.filename)
            .collect();
        files.sort();
        let mut dirs: Vec<String> = disk
            .list_directory(&join(&dir, "*.*"), 0x10)
            .unwrap_or_default()
            .into_iter()
            .filter(|e| e.is_dir && e.filename != "." && e.filename != "..")
            .map(|e| e.filename)
            .collect();
        dirs.sort();
        if !dir.ends_with('\\') {
            dirs.insert(0, "..".to_string());
        }
        dirs.extend(
            disk.mounted_drives().iter().map(|d| format!("[-{}-]", crate::disk::drive_letter(d.drive))),
        );
        dialog.set_items(at::FILES, files);
        dialog.set_items(at::DIRS, dirs);
        dialog.labels = vec![
            (2, 3, "File Name:".to_string()),
            (4, 3, dir.chars().take(dialog.width - 6).collect()),
            (6, 17, "Files".to_string()),
            (6, 45, "Dirs/Drives".to_string()),
        ];
    }

    /// Go into the directory item `item` of a file dialog's list names.
    fn enter_dir(cpu: &Cpu, dialog: &mut Dialog<Kind>, item: &str) {
        let (Kind::Open { dir, .. } | Kind::SaveAs { dir, .. }) = &mut dialog.kind else { return };
        let new = if item == ".." {
            let (parent, _) = split_path(dir);
            parent
        } else if let Some(letter) = item.strip_prefix("[-").and_then(|s| s.strip_suffix("-]")) {
            let drive = letter.as_bytes()[0] - b'A';
            let current = cpu.bus.disk.get_current_directory_of(drive).unwrap_or_default();
            format!("{}:\\{}", letter, current)
        } else {
            join(dir, item)
        };
        *dir = new;
        Self::refresh(cpu, dialog);
    }

    /// The file dialog's OK: a pattern lists those files, a directory goes
    /// there, a file is opened or saved to.
    fn file_ok(&mut self, cpu: &mut Cpu, mut dialog: Box<Dialog<Kind>>) {
        let name = crate::dosstr::from_bytes(&dialog.text(at::NAME)).trim().to_string();
        if name.is_empty() {
            self.mode = Mode::Dialog(dialog);
            return;
        }
        let (Kind::Open { dir, filter } | Kind::SaveAs { dir, filter, .. }) = &mut dialog.kind else { return };
        let full = join(dir, &name);
        if name.contains(['*', '?']) {
            let (new_dir, pattern) = split_path(&full);
            if let Some(new_dir) = cpu.bus.disk.qualify_path(&new_dir).filter(|d| cpu.bus.disk.is_directory(d)) {
                *dir = new_dir;
            }
            *filter = pattern.to_ascii_uppercase();
            Self::refresh(cpu, &mut dialog);
            self.mode = Mode::Dialog(dialog);
            return;
        }
        let Some(path) = cpu.bus.disk.qualify_path(&full) else {
            self.info(&[&error_text(0x03)]);
            return;
        };
        if cpu.bus.disk.is_directory(&path) {
            *dir = path;
            let filter = filter.clone();
            Self::refresh(cpu, &mut dialog);
            dialog.set_text(at::NAME, filter.as_bytes());
            self.mode = Mode::Dialog(dialog);
            return;
        }
        match dialog.kind {
            Kind::Open { .. } => {
                if !cpu.bus.disk.is_file(&path) {
                    self.info(&[&error_text(0x02)]);
                    return;
                }
                match Self::load(cpu, &path) {
                    Ok(buf) => {
                        self.buf = buf;
                        self.path = Some(path);
                    }
                    Err(e) => self.info(&[&e]),
                }
            }
            Kind::SaveAs { then, .. } => {
                if cpu.bus.disk.is_file(&path) && self.path.as_deref() != Some(&path) {
                    let dialog = Dialog::message(
                        Kind::Overwrite { path, then },
                        "",
                        &["File already exists. Overwrite?"],
                        &[("Yes", Button::Yes), ("No", Button::No)],
                    );
                    self.mode = Mode::Dialog(Box::new(dialog));
                } else {
                    self.save_to(cpu, path, then);
                }
            }
            _ => {}
        }
    }

    fn save_to(&mut self, cpu: &mut Cpu, path: String, then: Then) {
        if self.write(cpu, &path) {
            self.path = Some(path);
            self.proceed(cpu, then);
        }
    }

    // ----- Find and Replace -----

    /// The text Find starts with: the selection on one line, the word at
    /// the cursor, or the last search.
    fn find_text(&self) -> Vec<u8> {
        if let Some(text) = self.buf.selected_text().filter(|t| !t.contains(&b'\n')) {
            return text;
        }
        let word = self.buf.word_at_cursor();
        if word.is_empty() { self.search.text.clone() } else { word }
    }

    fn find_dialog(&mut self) {
        let dialog = Dialog::new(Kind::Find, "Find", 64, 10)
            .label(2, 3, "Find What:")
            .control(Control::field(2, 14, 46, &self.find_text()))
            .control(Control::check(4, 3, "Match Upper/Lowercase", self.search.case))
            .control(Control::check(4, 36, "Whole Word", self.search.whole_word))
            .buttons(7, &[("OK", Button::Ok), ("Cancel", Button::Cancel)]);
        self.mode = Mode::Dialog(Box::new(dialog));
    }

    fn replace_dialog(&mut self) {
        let mut dialog = Dialog::new(Kind::Replace, "Replace", 64, 12)
            .label(2, 3, "Find What:")
            .label(4, 3, "Replace With:")
            .control(Control::field(2, 17, 43, &self.find_text()))
            .control(Control::field(4, 17, 43, &self.replace_with))
            .control(Control::check(6, 3, "Match Upper/Lowercase", self.search.case))
            .control(Control::check(6, 36, "Whole Word", self.search.whole_word))
            .buttons(
                9,
                &[("Find and Verify", Button::FindVerify), ("Replace All", Button::ReplaceAll), ("Cancel", Button::Cancel)],
            );
        dialog.default = Button::FindVerify;
        self.mode = Mode::Dialog(Box::new(dialog));
    }

    /// The next match, selected, or a message that there is none.
    fn find_next(&mut self) {
        if !self.buf.find_next(&self.search) {
            self.info(&["Match not found."]);
        }
    }

    /// The match after `from` that Find and Verify goes to next, going round
    /// to the top once, and not past where it started.
    fn next_replace(&mut self, from: Pos, origin: Pos, wrapped: bool) {
        let mut wrapped = wrapped;
        let mut found = self.buf.find_forward(from, &self.search);
        if found.is_none() && !wrapped {
            wrapped = true;
            found = self.buf.find_forward(Pos::default(), &self.search);
        }
        match found.filter(|&at| !wrapped || at < origin) {
            Some(at) => {
                self.buf.select_match(at, self.search.text.len());
                let (width, height) = (self.text_width(), self.text_height());
                self.buf.scroll_to_cursor(width, height);
                let mut dialog = Dialog::message(
                    Kind::Verify { origin, wrapped },
                    "Replace",
                    &["Replace this occurrence?"],
                    &[("Replace", Button::Replace), ("Skip", Button::Skip), ("Cancel", Button::Cancel)],
                );
                // Out of the way of the match.
                let row = TEXT_TOP + at.line - self.buf.top;
                dialog.at_row = Some(if row > self.rows / 2 { TEXT_TOP + 1 } else { self.rows - dialog.height - 2 });
                self.mode = Mode::Dialog(Box::new(dialog));
            }
            None => {
                self.buf.anchor = None;
                self.info(&["Replace complete."]);
            }
        }
    }

    /// The search a Find or Replace dialog sets.
    fn take_search(&mut self, dialog: &Dialog<Kind>, case: usize) {
        self.search = Search { text: dialog.text(at::FIND), case: dialog.checked(case), whole_word: dialog.checked(case + 1) };
    }

    // ----- Dialogs -----

    fn dialog_event(&mut self, cpu: &mut Cpu, mut dialog: Box<Dialog<Kind>>, event: Event) {
        let file_dialog = matches!(dialog.kind, Kind::Open { .. } | Kind::SaveAs { .. });
        match event {
            Event::None => self.mode = Mode::Dialog(dialog),
            Event::Cancel | Event::Press(Button::Cancel) => {
                if let Kind::Verify { .. } = dialog.kind {
                    self.buf.anchor = None;
                }
            }
            Event::ListMove(n) => {
                if file_dialog && n == at::FILES {
                    let name = dialog.selected(n).unwrap_or_default().to_string();
                    dialog.set_text(at::NAME, name.as_bytes());
                }
                self.mode = Mode::Dialog(dialog);
            }
            Event::ListEnter(n) if file_dialog && n == at::DIRS => {
                let item = dialog.selected(n).unwrap_or_default().to_string();
                Self::enter_dir(cpu, &mut dialog, &item);
                self.mode = Mode::Dialog(dialog);
            }
            Event::ListEnter(n) => {
                if file_dialog {
                    let name = dialog.selected(n).unwrap_or_default().to_string();
                    dialog.set_text(at::NAME, name.as_bytes());
                    self.file_ok(cpu, dialog);
                } else {
                    self.mode = Mode::Dialog(dialog);
                }
            }
            Event::Press(button) => match (dialog.kind.clone(), button) {
                (Kind::Open { .. } | Kind::SaveAs { .. }, Button::Ok) => {
                    // OK in the directories goes into the one selected.
                    if dialog.focus == at::DIRS {
                        let item = dialog.selected(at::DIRS).unwrap_or_default().to_string();
                        Self::enter_dir(cpu, &mut dialog, &item);
                        self.mode = Mode::Dialog(dialog);
                    } else {
                        self.file_ok(cpu, dialog);
                    }
                }
                (Kind::Find, Button::Ok) => {
                    self.take_search(&dialog, 1);
                    if !self.search.text.is_empty() {
                        self.find_next();
                    }
                }
                (Kind::Replace, Button::FindVerify | Button::ReplaceAll) => {
                    self.take_search(&dialog, 2);
                    self.replace_with = dialog.text(at::REPLACE_WITH);
                    if self.search.text.is_empty() {
                        return;
                    }
                    if button == Button::ReplaceAll {
                        let count = self.buf.replace_all(&self.search, &self.replace_with.clone());
                        self.info(&[if count > 0 { "Replace complete." } else { "Match not found." }]);
                    } else {
                        let origin = self.buf.cursor;
                        match self.buf.find_forward(origin, &self.search).or_else(|| self.buf.find_forward(Pos::default(), &self.search)) {
                            Some(_) => self.next_replace(origin, origin, false),
                            None => self.info(&["Match not found."]),
                        }
                    }
                }
                (Kind::Verify { mut origin, wrapped }, Button::Replace | Button::Skip) => {
                    let Some((at, _)) = self.buf.selection() else { return };
                    let len = self.search.text.len();
                    let mut next = Pos::new(at.line, at.col + 1);
                    if button == Button::Replace {
                        let with = self.replace_with.clone();
                        self.buf.replace_at(at, len, &with);
                        // Where it started moves with the text before it.
                        if at.line == origin.line && at.col < origin.col {
                            origin.col = (origin.col + with.len()).saturating_sub(len);
                        }
                        next = Pos::new(at.line, at.col + with.len());
                    }
                    self.buf.anchor = None;
                    self.buf.cursor = next;
                    self.next_replace(next, origin, wrapped);
                }
                (Kind::SaveChanges(then), Button::Yes) => self.save(cpu, then),
                (Kind::SaveChanges(then), Button::No) => self.proceed(cpu, then),
                (Kind::Overwrite { path, then }, Button::Yes) => self.save_to(cpu, path, then),
                (Kind::Overwrite { .. }, Button::No) => {}
                _ => {}
            },
        }
    }

    // ----- Mouse and Alt -----

    /// Look at the mouse and the Alt key; whether the screen changed.
    fn poll(&mut self, cpu: &mut Cpu) -> bool {
        let mut changed = false;
        let alt = cpu.bus.read_8(0x0417) & ALT != 0;
        if alt != self.alt_held {
            self.alt_held = alt;
            changed = true;
            if alt {
                self.alt_armed = true;
            } else if std::mem::take(&mut self.alt_armed) {
                // Alt pressed and let go on its own: the menu bar, or
                // back from it.
                self.mode = match std::mem::replace(&mut self.mode, Mode::Edit) {
                    Mode::Edit => Mode::Bar(0),
                    Mode::Bar(_) | Mode::Menu(..) => Mode::Edit,
                    dialog => dialog,
                };
            }
        }

        let mouse = &cpu.bus.mouse;
        let (presses, x, y, down) = (mouse.press_count[0], mouse.press_x[0], mouse.press_y[0], mouse.buttons & 1 != 0);
        let (mx, my) = (mouse.x, mouse.y);
        let seen = *self.mouse.presses.get_or_insert(presses);
        if presses != seen {
            self.mouse.presses = Some(presses);
            let shift = cpu.bus.read_8(0x0417) & SHIFT != 0;
            self.click(cpu, (y / 8).max(0) as usize, (x / 8).max(0) as usize, shift);
            changed = true;
        }
        if down && self.mouse.dragging {
            let pos = self.text_pos((my / 8).max(0) as usize, (mx / 8).max(0) as usize);
            if pos != self.buf.cursor {
                self.buf.move_to(pos, true);
                let (width, height) = (self.text_width(), self.text_height());
                self.buf.scroll_to_cursor(width, height);
                changed = true;
            }
        }
        if !down {
            self.mouse.dragging = false;
        }
        changed
    }

    /// The place in the text at a cell of the screen, inside the window.
    fn text_pos(&self, row: usize, col: usize) -> Pos {
        let row = row.clamp(TEXT_TOP, TEXT_TOP + self.text_height() - 1) - TEXT_TOP;
        let col = col.clamp(1, self.text_width()) - 1;
        let line = (self.buf.top + row).min(self.buf.lines.len() - 1);
        Pos::new(line, self.buf.left + col)
    }

    fn click(&mut self, cpu: &mut Cpu, row: usize, col: usize, shift: bool) {
        let (cols, rows) = (self.cols, self.rows);
        match std::mem::replace(&mut self.mode, Mode::Edit) {
            Mode::Dialog(mut dialog) => {
                let event = dialog.click(row, col, cols, rows);
                self.dialog_event(cpu, dialog, event);
            }
            Mode::Menu(n, item) => {
                if row == 0 {
                    if let Some(t) = menu::title_at(col, cols) {
                        self.mode = if t == n { Mode::Edit } else { Mode::Menu(t, 0) };
                    }
                    return;
                }
                let (left, width, height) = menu::menu_box(n, cols);
                if row >= 2 && row < height && col > left && col < left + width - 1 {
                    let at = row - 2;
                    if MENUS[n].items[at].label.is_some() {
                        self.choose(cpu, n, at);
                    } else {
                        self.mode = Mode::Menu(n, item);
                    }
                }
            }
            Mode::Bar(_) | Mode::Edit => {
                let (top, bottom) = (TEXT_TOP, TEXT_TOP + self.text_height());
                if row == 0 {
                    if let Some(n) = menu::title_at(col, cols) {
                        self.mode = Mode::Menu(n, 0);
                    }
                } else if col == cols - 1 && row >= top && row < bottom {
                    // The vertical scroll bar: its arrows a line, the rest
                    // a page.
                    let page = self.text_height() as isize;
                    let lines = if row == top {
                        -1
                    } else if row == bottom - 1 {
                        1
                    } else if row < top + thumb(self.buf.cursor.line, self.buf.lines.len(), self.text_height() - 2) {
                        -page
                    } else {
                        page
                    };
                    self.scroll(lines);
                } else if row == bottom && col > 0 && col < cols - 1 {
                    let width = self.text_width();
                    let buf = &mut self.buf;
                    buf.left = if col == 1 {
                        buf.left.saturating_sub(1)
                    } else if col == cols - 2 {
                        buf.left + 1
                    } else if col < 1 + thumb(buf.left, 256, width - 2) {
                        buf.left.saturating_sub(width)
                    } else {
                        buf.left + width
                    };
                    buf.cursor.col = buf.cursor.col.clamp(buf.left, buf.left + width - 1);
                    buf.anchor = None;
                } else if row >= top && row < bottom && col > 0 && col < cols - 1 {
                    let pos = self.text_pos(row, col);
                    self.buf.move_to(pos, shift);
                    self.mouse.dragging = true;
                }
            }
        }
    }

    // ----- Drawing -----

    fn draw(&mut self, cpu: &mut Cpu) {
        self.redraw = false;
        let (cols, rows) = (self.cols, self.rows);
        let mut s = Screen::new(cols, rows);
        let (highlight, hotkeys) = match self.mode {
            Mode::Bar(n) => (Some(n), true),
            Mode::Menu(n, _) => (Some(n), false),
            Mode::Edit => (None, self.alt_held),
            Mode::Dialog(_) => (None, false),
        };
        menu::draw_bar(&mut s, highlight, hotkeys);
        self.draw_window(&mut s);
        self.draw_status(cpu, &mut s);
        match &self.mode {
            Mode::Menu(n, item) => menu::draw_menu(&mut s, *n, *item, |action| self.enabled(cpu, action)),
            Mode::Dialog(dialog) => dialog.draw(&mut s),
            _ => {}
        }

        let base = Self::text_base(cpu);
        for (i, b) in s.bytes().into_iter().enumerate() {
            cpu.bus.write_8(base + i, b);
        }

        let cursor = match &self.mode {
            Mode::Edit => {
                let buf = &self.buf;
                Some((TEXT_TOP + buf.cursor.line - buf.top, 1 + buf.cursor.col - buf.left))
            }
            Mode::Dialog(dialog) => dialog.cursor(cols, rows),
            _ => None,
        };
        let page = cpu.bus.read_8(0x0462) as u16;
        match cursor {
            Some((row, col)) => {
                // An underline inserting, a block overwriting.
                let overwrite = self.buf.overwrite && self.mode == Mode::Edit;
                video(cpu, 0x0100, 0, if overwrite { 0x0007 } else { 0x0607 }, 0);
                video(cpu, 0x0200, page << 8, 0, (row as u16) << 8 | col as u16);
            }
            None => video(cpu, 0x0100, 0, 0x2000, 0),
        }
    }

    /// The edit window: its frame with the file's name, the text, and the
    /// scroll bars.
    fn draw_window(&self, s: &mut Screen) {
        let (cols, rows) = (self.cols, self.rows);
        let (width, height) = (self.text_width(), self.text_height());
        let bottom = TEXT_TOP + height;
        s.frame(1, 0, cols, rows - 2, TEXT);
        let name = match &self.path {
            Some(path) => split_path(path).1,
            None => "Untitled".to_string(),
        };
        let title = format!(" {} ", name);
        s.str(1, (cols.saturating_sub(title.len())) / 2, &title, TITLE);

        let buf = &self.buf;
        for r in 0..height {
            let n = buf.top + r;
            let Some(line) = buf.lines.get(n) else { continue };
            for c in 0..width {
                let col = buf.left + c;
                let ch = line.get(col).copied().unwrap_or(b' ');
                let selected = col < line.len() && buf.is_selected(Pos::new(n, col));
                s.set(TEXT_TOP + r, 1 + c, ch, if selected { SELECTED } else { TEXT });
            }
        }

        // The vertical scroll bar, at the right edge.
        s.set(TEXT_TOP, cols - 1, 0x18, SCROLL);
        for r in TEXT_TOP + 1..bottom - 1 {
            s.set(r, cols - 1, 0xB0, SCROLL);
        }
        s.set(bottom - 1, cols - 1, 0x19, SCROLL);
        let at = thumb(buf.cursor.line, buf.lines.len(), height - 2);
        s.set(TEXT_TOP + at, cols - 1, b' ', 0x00);
        // The horizontal one, along the bottom.
        s.set(bottom, 1, 0x1B, SCROLL);
        for c in 2..cols - 2 {
            s.set(bottom, c, 0xB0, SCROLL);
        }
        s.set(bottom, cols - 2, 0x1A, SCROLL);
        let at = thumb(buf.left, 256, width - 2);
        s.set(bottom, 1 + at, b' ', 0x00);
    }

    /// The status line: what the keys do, or the menu item's help, and
    /// where the cursor is.
    fn draw_status(&self, cpu: &Cpu, s: &mut Screen) {
        let row = self.rows - 1;
        s.fill(row, 0, self.cols, 1, b' ', STATUS);
        let text = match &self.mode {
            Mode::Edit => "Rust-DOS Editor  <F1=Help> Press ALT to activate menus".to_string(),
            Mode::Bar(_) => "F1=Help  Enter=Display Menu  Esc=Cancel  Arrow=Next Item".to_string(),
            Mode::Menu(n, item) => {
                let item = &MENUS[*n].items[*item];
                if self.enabled(cpu, item.action) { item.help.to_string() } else { format!("{} (not available now)", item.help) }
            }
            Mode::Dialog(_) => "Enter=Execute  Esc=Cancel  Tab=Next Field  Arrow=Next Item".to_string(),
        };
        let place = format!("{:05}:{:03}", self.buf.cursor.line + 1, (self.buf.cursor.col + 1).min(999));
        let room = self.cols - place.len() - 4;
        s.str(row, 1, &text.chars().take(room).collect::<String>(), STATUS);
        s.set(row, self.cols - place.len() - 3, screen::line::V, 0x30);
        s.str(row, self.cols - place.len() - 1, &place, STATUS);
    }
}

/// Where a scroll bar's thumb is, for `at` of `total` on a track `length`
/// long: from 1, past the arrow.
fn thumb(at: usize, total: usize, length: usize) -> usize {
    let span = length.max(1) - 1;
    1 + if total <= 1 { 0 } else { (at.min(total - 1) * span) / (total - 1) }
}
