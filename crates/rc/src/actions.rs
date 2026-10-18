//! What keys, clicks and menu items do.

use std::collections::VecDeque;

use crate::dialog::{Button, Dialog, Event, Item};
use crate::host::{Host, Mouse, Outcome, attr};
use crate::keys::Key;
use crate::menu::{self, Action, Bar, Checks, Hit};
use crate::ops::{self, Job, Kind, Source};
use crate::panel::{Filter, Layout, Sort, View};
use crate::tree::Tree;
use crate::viewer::{Done, Viewer};
use crate::{Commander, Find, Mode, Purpose, cp437, path};

/// How long between two clicks makes them a double click.
const DOUBLE_CLICK_MS: u64 = 400;

impl Commander {
    // ----- Geometry -----

    /// The rows the panels take.
    pub(crate) fn panel_height(&self) -> usize {
        let h = self.command_row();
        if self.options.full_screen {
            h
        } else {
            (h / 2 + 1).min(h)
        }
    }

    pub(crate) fn layout(&self, p: usize) -> Layout {
        let half = self.cols / 2;
        let (x, w) = if p == 0 {
            (0, half)
        } else {
            (half, self.cols - half)
        };
        Layout::new(x, w, self.panel_height(), self.options.mini_status)
    }

    pub(crate) fn file_view(&self, p: usize) -> bool {
        matches!(self.panels[p].view, View::Brief | View::Full)
    }

    pub(crate) fn shows(&self, p: usize) -> bool {
        !self.panels_off && self.panels[p].visible
    }

    pub(crate) fn checks(&self) -> Checks {
        Checks {
            views: [self.panels[0].view, self.panels[1].view],
            sorts: [self.panels[0].sort, self.panels[1].sort],
            auto_menus: self.options.auto_menus,
            path_prompt: self.options.path_prompt,
            key_bar: self.options.key_bar,
            full_screen: self.options.full_screen,
            mini_status: self.options.mini_status,
            clock: self.options.clock,
        }
    }

    /// The DOS directory follows the active panel, so that command lines
    /// run there.
    pub(crate) fn sync(&mut self, host: &mut dyn Host) {
        for p in 0..2 {
            if self.panels[p].stale {
                let _ = self.panels[p].refresh(host);
                let l = self.layout(p);
                self.panels[p].scroll(&l);
            }
        }
        let dir = self.panels[self.active].dir.clone();
        if !dir.eq_ignore_ascii_case(&self.synced) {
            host.set_current(&dir);
            self.synced = dir;
        }
    }

    // ----- Keys -----

    pub(crate) fn key(&mut self, host: &mut dyn Host, key: Key) -> Option<Outcome> {
        match std::mem::take(&mut self.mode) {
            Mode::Panels => self.panel_key(host, key),
            Mode::Menu(bar) => self.menu_key(host, bar, key),
            Mode::Dialog(mut d, purpose) => {
                let event = d.key(key);
                self.dialog_event(host, d, *purpose, event)
            }
            Mode::Search(text) => self.search_key(host, text, key),
            Mode::View(mut v) => {
                match v.key(key, self.cols, self.rows) {
                    Done::Stay => self.mode = Mode::View(v),
                    Done::Quit => {}
                    Done::Help => self.mode = Mode::Help(0),
                }
                None
            }
            Mode::Job(mut job) => {
                if let Some(q) = job.question() {
                    let event = q.key(key);
                    job.answer(event);
                } else if key == Key::Esc {
                    job.cancel();
                }
                self.mode = Mode::Job(job);
                None
            }
            Mode::Help(top) => {
                let top = match key {
                    Key::Up => top.saturating_sub(1),
                    Key::Down => top + 1,
                    Key::PgUp => top.saturating_sub(10),
                    Key::PgDn => top + 10,
                    Key::Home => 0,
                    Key::Esc | Key::F(10) | Key::Enter => return None,
                    _ => top,
                };
                self.mode = Mode::Help(top.min(crate::draw::HELP_TEXT.len().saturating_sub(1)));
                None
            }
            Mode::Find(mut find) => {
                match find.dialog.key(key) {
                    Event::Cancel | Event::Button(Button::Cancel) => {}
                    Event::Button(_) => {
                        let at = find.dialog.selected();
                        if let Some(file) = find.found.get(at).cloned() {
                            self.go_to_file(&file);
                        }
                    }
                    Event::None => self.mode = Mode::Find(find),
                }
                None
            }
        }
    }

    fn panel_key(&mut self, host: &mut dyn Host, key: Key) -> Option<Outcome> {
        let a = self.active;
        let l = self.layout(a);
        let typing = !self.cmdline.is_empty();
        if self.panels[a].view == View::Tree && !self.panels_off && self.tree_key(host, key) {
            return None;
        }
        match key {
            Key::F(1) => self.mode = Mode::Help(0),
            Key::F(3) | Key::AltF(3) => self.view_file(host),
            Key::F(4) | Key::AltF(4) => return self.edit_file(host),
            Key::F(5) => self.copy_dialog(Kind::Copy),
            Key::F(6) => self.copy_dialog(Kind::Move),
            Key::F(7) => self.mkdir_dialog(),
            Key::F(8) => self.delete_dialog(),
            Key::Del if !typing => self.delete_dialog(),
            Key::F(9) => self.open_bar(),
            Key::F(10) => self.quit_dialog(),
            Key::AltF(1) => self.drive_dialog(host, 0),
            Key::AltF(2) => self.drive_dialog(host, 1),
            Key::AltF(7) => self.find_dialog(),
            Key::AltF(8) => self.history_dialog(host),
            Key::AltF(10) => self.ncd(host, None),
            Key::CtrlF(1) => self.panels[0].visible = !self.panels[0].visible,
            Key::CtrlF(2) => self.panels[1].visible = !self.panels[1].visible,
            Key::CtrlF(3) => self.panels[a].set_sort(Sort::Name),
            Key::CtrlF(4) => self.panels[a].set_sort(Sort::Extension),
            Key::CtrlF(5) => self.panels[a].set_sort(Sort::Time),
            Key::CtrlF(6) => self.panels[a].set_sort(Sort::Size),
            Key::CtrlF(7) => self.panels[a].set_sort(Sort::Unsorted),
            Key::Ctrl(b'O') => self.panels_off = !self.panels_off,
            Key::Ctrl(b'P') => {
                let other = 1 - a;
                self.panels[other].visible = !self.panels[other].visible;
            }
            Key::Ctrl(b'U') => self.swap(),
            Key::Ctrl(b'R') => self.reread(a),
            Key::Ctrl(b'L') => self.toggle_other(host, View::Info),
            Key::Ctrl(b'Q') => self.toggle_other(host, View::Quick),
            Key::Ctrl(b'B') => self.options.key_bar = !self.options.key_bar,
            Key::Ctrl(b'E') => self.history_step(host, true),
            Key::Ctrl(b'X') => self.history_step(host, false),
            Key::Ctrl(b'Y') => {
                self.cmdline.clear();
                self.cmd_cursor = 0;
            }
            Key::CtrlEnter => {
                if let Some(item) = self.panels[a].current().filter(|i| !i.is_up()) {
                    let name = format!("{} ", item.entry.name.to_ascii_lowercase());
                    self.insert(&cp437::bytes(&name));
                }
            }
            Key::CtrlLeftBracket | Key::CtrlRightBracket => {
                let p = if key == Key::CtrlLeftBracket { 0 } else { 1 };
                let dir = format!("{} ", self.panels[p].dir);
                self.insert(&cp437::bytes(&dir));
            }
            Key::Tab | Key::ShiftTab => self.switch_panel(),
            Key::Ins => {
                self.panels[a].toggle_mark();
                self.panels[a].step(1, &l);
            }
            Key::GrayPlus => self.select_dialog(true),
            Key::GrayMinus => self.select_dialog(false),
            Key::GrayStar => self.panels[a].invert_marks(),
            Key::Alt(c) if c.is_ascii_alphanumeric() => {
                let text = (c as char).to_string();
                self.panels[a].find_prefix(&text, &l);
                self.mode = Mode::Search(text);
            }
            Key::Enter => {
                if typing {
                    let line = cp437::string(&self.cmdline);
                    return Some(Outcome::Run { line, echo: true });
                }
                return self.open_current(host);
            }
            Key::Esc => {
                self.cmdline.clear();
                self.cmd_cursor = 0;
                self.history_at = None;
            }
            Key::Backspace => {
                if self.cmd_cursor > 0 {
                    self.cmd_cursor -= 1;
                    self.cmdline.remove(self.cmd_cursor);
                }
            }
            Key::Char(c) => self.insert(&[c]),
            Key::Del => {
                if self.cmd_cursor < self.cmdline.len() {
                    self.cmdline.remove(self.cmd_cursor);
                }
            }
            Key::Left if typing => self.cmd_cursor = self.cmd_cursor.saturating_sub(1),
            Key::Right if typing => self.cmd_cursor = (self.cmd_cursor + 1).min(self.cmdline.len()),
            Key::Home if typing => self.cmd_cursor = 0,
            Key::End if typing => self.cmd_cursor = self.cmdline.len(),
            Key::Up => self.panels[a].step(-1, &l),
            Key::Down => self.panels[a].step(1, &l),
            Key::PgUp => self.panels[a].step(-(self.panels[a].page(&l) as isize) + 1, &l),
            Key::PgDn => self.panels[a].step(self.panels[a].page(&l) as isize - 1, &l),
            Key::Home => self.panels[a].home(&l),
            Key::End => self.panels[a].end(&l),
            Key::Left | Key::Right => {
                // A column in Brief, a page in Full.
                let delta = l.list as isize;
                self.panels[a].step(if key == Key::Left { -delta } else { delta }, &l);
            }
            Key::CtrlPgUp => self.go_up(a),
            Key::CtrlPgDn
                if self.panels[a]
                    .current()
                    .is_some_and(|i| i.is_dir() && !i.is_up()) =>
            {
                return self.open_current(host);
            }
            _ => {}
        }
        None
    }

    fn insert(&mut self, text: &[u8]) {
        for &b in text {
            self.cmdline.insert(self.cmd_cursor, b);
            self.cmd_cursor += 1;
        }
        self.history_at = None;
    }

    /// Ctrl+E and Ctrl+X: the command lines typed before.
    fn history_step(&mut self, host: &dyn Host, back: bool) {
        let lines = host.history();
        if lines.is_empty() {
            return;
        }
        let at = match (self.history_at, back) {
            (None, true) => lines.len() - 1,
            (None, false) => return,
            (Some(i), true) => i.saturating_sub(1),
            (Some(i), false) if i + 1 >= lines.len() => {
                self.history_at = None;
                self.cmdline.clear();
                self.cmd_cursor = 0;
                return;
            }
            (Some(i), false) => i + 1,
        };
        self.history_at = Some(at);
        self.cmdline = cp437::bytes(&lines[at]);
        self.cmd_cursor = self.cmdline.len();
    }

    fn switch_panel(&mut self) {
        let other = 1 - self.active;
        if self.shows(other)
            && matches!(
                self.panels[other].view,
                View::Brief | View::Full | View::Tree
            )
        {
            self.active = other;
        }
    }

    fn swap(&mut self) {
        self.panels.swap(0, 1);
        self.tree_pos.swap(0, 1);
        self.active = 1 - self.active;
    }

    fn reread(&mut self, p: usize) {
        self.panels[p].stale = true;
        self.forget_zips = true;
        let drive = self.panels[p].drive();
        self.trees.retain(|t| t.drive != drive);
        self.quick = None;
    }

    /// Ctrl+L and Ctrl+Q: the other panel shows Info or Quick view, or
    /// goes back to its files.
    fn toggle_other(&mut self, host: &mut dyn Host, view: View) {
        let other = 1 - self.active;
        let to = if self.panels[other].view == view {
            View::Brief
        } else {
            view
        };
        self.set_view(host, other, to);
        self.panels[other].visible = true;
    }

    pub(crate) fn set_view(&mut self, host: &mut dyn Host, p: usize, view: View) {
        self.panels[p].view = view;
        self.panels[p].stale = true;
        if view == View::Tree {
            let dir = self.panels[p].dir.clone();
            let at = self.tree(host, self.panels[p].drive()).find(&dir);
            self.tree_pos[p] = (at, 0);
            self.scroll_tree(p);
        }
        // Info and Quick view take no keys: the other panel does.
        if matches!(view, View::Info | View::Quick) && self.active == p && self.file_view(1 - p) {
            self.active = 1 - p;
        }
    }

    /// A drive's tree, read the first time it is needed.
    fn tree(&mut self, host: &mut dyn Host, drive: u8) -> &Tree {
        if !self.trees.iter().any(|t| t.drive == drive) {
            self.trees.push(Tree::read(host, drive));
        }
        self.trees
            .iter()
            .find(|t| t.drive == drive)
            .expect("read above")
    }

    pub(crate) fn tree_of(&self, drive: u8) -> Option<&Tree> {
        self.trees.iter().find(|t| t.drive == drive)
    }

    fn scroll_tree(&mut self, p: usize) {
        let rows = self.layout(p).list + 1;
        let (cursor, top) = &mut self.tree_pos[p];
        if *cursor < *top {
            *top = *cursor;
        } else if *cursor >= *top + rows {
            *top = *cursor + 1 - rows;
        }
    }

    /// The keys of a Tree panel; whether it took the key.
    fn tree_key(&mut self, host: &mut dyn Host, key: Key) -> bool {
        let a = self.active;
        let drive = self.panels[a].drive();
        let len = self.tree(host, drive).nodes.len();
        let rows = self.layout(a).list + 1;
        let (cursor, _) = self.tree_pos[a];
        let to = match key {
            Key::Up => cursor.saturating_sub(1),
            Key::Down => (cursor + 1).min(len.saturating_sub(1)),
            Key::PgUp => cursor.saturating_sub(rows - 1),
            Key::PgDn => (cursor + rows - 1).min(len.saturating_sub(1)),
            Key::Home => 0,
            Key::End => len.saturating_sub(1),
            Key::Enter if self.cmdline.is_empty() => {
                self.switch_panel();
                return true;
            }
            _ => return false,
        };
        self.tree_pos[a].0 = to;
        self.scroll_tree(a);
        self.follow_tree(a);
        true
    }

    /// The Tree panel's directory, and the other panel showing it.
    fn follow_tree(&mut self, p: usize) {
        let drive = self.panels[p].drive();
        let Some(dir) = self
            .tree_of(drive)
            .and_then(|t| t.nodes.get(self.tree_pos[p].0))
            .map(|n| n.path.clone())
        else {
            return;
        };
        self.panels[p].dir = dir.clone();
        let other = 1 - p;
        if self.file_view(other) {
            self.panels[other].go(&dir, None);
        }
    }

    /// Enter: into a directory, or run a program.
    fn open_current(&mut self, host: &mut dyn Host) -> Option<Outcome> {
        let a = self.active;
        let item = self.panels[a].current()?.clone();
        if item.is_up() {
            self.go_up(a);
            return None;
        }
        if item.is_dir() {
            let dir = path::join(&self.panels[a].dir, &item.entry.name);
            if host.list(&dir).is_ok() {
                self.panels[a].go(&dir, None);
            }
            return None;
        }
        // A zip archive opens as a directory.
        if crate::archive::is_zip_name(&item.entry.name) {
            let dir = path::join(&self.panels[a].dir, &item.entry.name);
            match host.list(&dir) {
                Ok(_) => self.panels[a].go(&dir, None),
                Err(e) => self.error(&format!("Cannot open {}", path::shown(&dir)), e),
            }
            return None;
        }
        let (_, ext) = path::split_name(&item.entry.name);
        if ["EXE", "COM", "BAT"]
            .iter()
            .any(|e| ext.eq_ignore_ascii_case(e))
        {
            if self.in_archive(host) {
                self.message("Archive", "Copy a program out of the archive to run it");
                return None;
            }
            return Some(Outcome::Run {
                line: item.entry.name.to_ascii_lowercase(),
                echo: true,
            });
        }
        None
    }

    /// Up a directory, the cursor on the one it came from.
    fn go_up(&mut self, p: usize) {
        let dir = self.panels[p].dir.clone();
        if let Some(up) = path::parent(&dir) {
            self.panels[p].go(&up, Some(path::name(&dir).to_string()));
        }
    }

    /// A directory and a file in it: Find file's choice.
    fn go_to_file(&mut self, file: &str) {
        let a = self.active;
        if let Some(dir) = path::parent(file) {
            if self.panels[a].view == View::Tree {
                self.panels[a].view = View::Brief;
            }
            self.panels[a].go(&dir, Some(path::name(file).to_string()));
        }
    }

    fn search_key(&mut self, host: &mut dyn Host, mut text: String, key: Key) -> Option<Outcome> {
        let a = self.active;
        let l = self.layout(a);
        match key {
            Key::Char(c) | Key::Alt(c) if c > b' ' => {
                text.push(c as char);
                if !self.panels[a].find_prefix(&text, &l) {
                    text.pop();
                }
            }
            Key::Backspace => {
                text.pop();
                if text.is_empty() {
                    return None;
                }
            }
            Key::Esc => return None,
            _ => return self.panel_key(host, key),
        }
        self.mode = Mode::Search(text);
        None
    }

    // ----- The menus -----

    fn open_bar(&mut self) {
        let mut bar = Bar {
            menu: if self.active == 0 { 0 } else { 4 },
            item: None,
        };
        if self.options.auto_menus {
            bar.open(&self.checks());
        }
        self.mode = Mode::Menu(bar);
    }

    fn menu_key(&mut self, host: &mut dyn Host, mut bar: Bar, key: Key) -> Option<Outcome> {
        let c = self.checks();
        let n = menu::TITLES.len();
        match key {
            Key::Esc | Key::F(10) => return None,
            Key::Left => {
                bar.menu = (bar.menu + n - 1) % n;
                if bar.item.is_some() {
                    bar.open(&c);
                }
            }
            Key::Right => {
                bar.menu = (bar.menu + 1) % n;
                if bar.item.is_some() {
                    bar.open(&c);
                }
            }
            Key::Up | Key::Down if bar.item.is_some() => {
                bar.step(if key == Key::Up { -1 } else { 1 }, &c)
            }
            Key::Up | Key::Down | Key::Enter if bar.item.is_none() => bar.open(&c),
            Key::Home if bar.item.is_some() => {
                bar.open(&c);
            }
            Key::End if bar.item.is_some() => {
                bar.open(&c);
                bar.step(-1, &c);
            }
            Key::Enter => {
                let item = bar
                    .item
                    .and_then(|i| menu::items(bar.menu, &c).get(i).map(|i| i.action));
                return item.and_then(|a| self.act(host, a));
            }
            Key::Char(ch) | Key::Alt(ch) => {
                if bar.item.is_some() {
                    if let Some(i) = bar.hot_item(ch, &c) {
                        let action = menu::items(bar.menu, &c)[i].action;
                        return self.act(host, action);
                    }
                } else if let Some(m) = menu::title_for(ch) {
                    bar.menu = m;
                    bar.open(&c);
                }
            }
            _ => {}
        }
        self.mode = Mode::Menu(bar);
        None
    }

    /// What a menu item does.
    pub(crate) fn act(&mut self, host: &mut dyn Host, action: Action) -> Option<Outcome> {
        match action {
            Action::View(p, v) => {
                self.set_view(host, p, v);
                self.panels[p].visible = true;
            }
            Action::OnOff(p) => self.panels[p].visible = !self.panels[p].visible,
            Action::Sort(p, s) => self.panels[p].set_sort(s),
            Action::Reread(p) => self.reread(p),
            Action::Filter(p) => self.filter_dialog(p),
            Action::Drive(p) => self.drive_dialog(host, p),
            Action::Help => self.mode = crate::Mode::Help(0),
            Action::ViewFile => self.view_file(host),
            Action::Edit => return self.edit_file(host),
            Action::Copy => self.copy_dialog(Kind::Copy),
            Action::Move => self.copy_dialog(Kind::Move),
            Action::Mkdir => self.mkdir_dialog(),
            Action::Delete => self.delete_dialog(),
            Action::Attributes => self.attributes_dialog(host),
            Action::SelectGroup => self.select_dialog(true),
            Action::DeselectGroup => self.select_dialog(false),
            Action::Invert => self.panels[self.active].invert_marks(),
            Action::Restore => self.panels[self.active].restore_marks(),
            Action::Quit => self.quit_dialog(),
            Action::Ncd => self.ncd(host, None),
            Action::FindFile => self.find_dialog(),
            Action::History => self.history_dialog(host),
            Action::Swap => self.swap(),
            Action::PanelsOnOff => self.panels_off = !self.panels_off,
            Action::AutoMenus => self.options.auto_menus = !self.options.auto_menus,
            Action::PathPrompt => self.options.path_prompt = !self.options.path_prompt,
            Action::KeyBar => self.options.key_bar = !self.options.key_bar,
            Action::FullScreen => self.options.full_screen = !self.options.full_screen,
            Action::MiniStatus => self.options.mini_status = !self.options.mini_status,
            Action::Clock => self.options.clock = !self.options.clock,
            Action::Missing | Action::Separator => {}
        }
        None
    }

    // ----- F3 and F4 -----

    fn view_file(&mut self, host: &mut dyn Host) {
        let a = self.active;
        let Some(item) = self.panels[a].current().filter(|i| !i.is_dir()).cloned() else {
            return;
        };
        let file = path::join(&self.panels[a].dir, &item.entry.name);
        match host.read(&file) {
            Ok(data) => self.mode = Mode::View(Box::new(Viewer::new(&path::shown(&file), data))),
            Err(e) => self.error(&format!("Cannot read the file {}", file), e),
        }
    }

    /// Whether the active panel shows what is in a zip archive.
    fn in_archive(&self, host: &mut dyn Host) -> bool {
        crate::archive::split(host, &[], &self.panels[self.active].dir).is_some()
    }

    fn edit_file(&mut self, host: &mut dyn Host) -> Option<Outcome> {
        let a = self.active;
        self.panels[a].current().filter(|i| !i.is_dir())?;
        if self.in_archive(host) {
            self.message("Archive", "Copy a file out of the archive to edit it");
            return None;
        }
        let item = self.panels[a].current()?;
        let file = path::join(&self.panels[a].dir, &item.entry.name);
        Some(Outcome::Run {
            line: format!("EDIT {}", file),
            echo: false,
        })
    }

    // ----- Dialogs -----

    fn dialog(&mut self, d: Dialog, purpose: Purpose) {
        self.mode = Mode::Dialog(Box::new(d), Box::new(purpose));
    }

    pub(crate) fn error(&mut self, what: &str, code: u8) {
        let text = ops::error_text(code);
        let d = Dialog::message("Error", &[what, &text], &[("OK", Button::Ok)], true);
        self.dialog(d, Purpose::Message);
    }

    pub(crate) fn message(&mut self, title: &str, text: &str) {
        let d = Dialog::message(title, &[text], &[("OK", Button::Ok)], true);
        self.dialog(d, Purpose::Message);
    }

    fn sources(&self) -> Vec<Source> {
        let p = &self.panels[self.active];
        p.selection()
            .into_iter()
            .map(|i| Source {
                path: path::join(&p.dir, &i.entry.name),
                entry: i.entry,
            })
            .collect()
    }

    /// What a dialog says it works on: a name, or how many files.
    fn what(sources: &[Source]) -> String {
        match sources {
            [one] => {
                let name = &one.entry.name;
                if one.entry.is_dir() {
                    name.to_ascii_uppercase()
                } else {
                    name.to_ascii_lowercase()
                }
            }
            many => format!("{} files", many.len()),
        }
    }

    fn copy_dialog(&mut self, kind: Kind) {
        if !self.file_view(self.active) {
            return;
        }
        let sources = self.sources();
        if sources.is_empty() {
            return;
        }
        let other = 1 - self.active;
        let target = if self.shows(other) && self.panels[other].view != View::Info {
            self.panels[other].dir.clone()
        } else {
            self.panels[self.active].dir.clone()
        };
        let what = match sources.as_slice() {
            [_] => format!("\"{}\"", Self::what(&sources)),
            _ => Self::what(&sources),
        };
        let (title, verb, button) = match kind {
            Kind::Move => ("Rename", "Rename or move", ("Rename", Button::Rename)),
            _ => ("Copy", "Copy", ("Copy", Button::Copy)),
        };
        let d = Dialog::new(
            title,
            68,
            vec![
                Item::text(&format!("{} {} to", verb, what)),
                Item::field(&target),
                Item::Sep,
                Item::check("Include sub directories", false),
                Item::Sep,
                Item::buttons(
                    &[
                        button,
                        ("F10-Tree", Button::Tree),
                        ("Cancel", Button::Cancel),
                    ],
                    true,
                ),
            ],
        );
        self.dialog(d, Purpose::Copy { sources, kind });
    }

    fn mkdir_dialog(&mut self) {
        if !self.file_view(self.active) {
            return;
        }
        let d = Dialog::new(
            "Make directory",
            68,
            vec![Item::text("Create the directory"), Item::field("")],
        );
        self.dialog(d, Purpose::Mkdir);
    }

    fn delete_dialog(&mut self) {
        if !self.file_view(self.active) {
            return;
        }
        let sources = self.sources();
        if sources.is_empty() {
            return;
        }
        let marked = self.panels[self.active].marked_count() > 0;
        let mut items = match (marked, sources.as_slice()) {
            (false, [_]) => vec![
                Item::centred("Do you wish to delete"),
                Item::centred(&Self::what(&sources)),
            ],
            _ => {
                let n = sources.len();
                vec![Item::centred(&format!(
                    "You have selected {} file{}.",
                    n,
                    if n == 1 { "" } else { "s" }
                ))]
            }
        };
        items.extend([
            Item::Sep,
            Item::centred_check("Include sub directories", false),
            Item::Sep,
            Item::buttons(
                &[("Delete", Button::Delete), ("Cancel", Button::Cancel)],
                false,
            ),
        ]);
        let d = Dialog::new("Delete", 39, items);
        self.dialog(d, Purpose::Delete { sources, marked });
    }

    fn attributes_dialog(&mut self, host: &mut dyn Host) {
        if !self.file_view(self.active) {
            return;
        }
        let sources = self.sources();
        let Some(first) = sources.first() else { return };
        let a = host.attributes(&first.path).unwrap_or(first.entry.attr);
        let d = Dialog::new(
            "File attributes",
            40,
            vec![
                Item::centred("Change file attributes for"),
                Item::centred(&Self::what(&sources)),
                Item::Sep,
                Item::check("Read only", a & attr::READ_ONLY != 0),
                Item::check("Archive", a & attr::ARCHIVE != 0),
                Item::check("Hidden", a & attr::HIDDEN != 0),
                Item::check("System", a & attr::SYSTEM != 0),
                Item::Sep,
                Item::buttons(&[("Set", Button::Set), ("Cancel", Button::Cancel)], true),
            ],
        );
        let paths = sources.into_iter().map(|s| s.path).collect();
        self.dialog(d, Purpose::Attributes { paths });
    }

    fn select_dialog(&mut self, on: bool) {
        if !self.file_view(self.active) {
            return;
        }
        let title = if on { "Select" } else { "Deselect" };
        let d = Dialog::new(title, 28, vec![Item::field("*.*")]);
        self.dialog(d, Purpose::Select { on });
    }

    fn filter_dialog(&mut self, p: usize) {
        let panel = &self.panels[p];
        let f = panel.filter;
        let d = Dialog::new(
            "Filter",
            40,
            vec![
                Item::radio("All files", f == Filter::All),
                Item::radio("Executable files", f == Filter::Executables),
                Item::radio("Custom:", f == Filter::Custom),
                Item::field(&panel.mask),
                Item::Sep,
                Item::buttons(&[("OK", Button::Ok), ("Cancel", Button::Cancel)], true),
            ],
        );
        self.dialog(d, Purpose::Filter { panel: p });
    }

    fn drive_dialog(&mut self, host: &dyn Host, p: usize) {
        let drives: Vec<u8> = host.drives().iter().map(|d| d.drive).collect();
        let letters: Vec<String> = drives
            .iter()
            .map(|&d| ((b'A' + d) as char).to_string())
            .collect();
        let sel = drives
            .iter()
            .position(|&d| d == self.panels[p].drive())
            .unwrap_or(0);
        let width = (letters.len() * 4 + 3).max(22);
        let side = if p == 0 { "left" } else { "right" };
        let mut d = Dialog::new(
            "Drive Letter",
            width,
            vec![
                Item::text(&format!("Choose ~{}~ drive:", side)),
                Item::Choices { list: letters, sel },
            ],
        );
        d.region = Some((self.layout(p).x, self.layout(p).w));
        self.dialog(d, Purpose::Drive { panel: p, drives });
    }

    fn quit_dialog(&mut self) {
        let d = Dialog::message(
            "The Rat Commander",
            &["Do you want to quit the Rat Commander?"],
            &[("Yes", Button::Yes), ("No", Button::No)],
            false,
        );
        self.dialog(d, Purpose::Quit);
    }

    fn history_dialog(&mut self, host: &dyn Host) {
        let lines = host.history();
        if lines.is_empty() {
            return;
        }
        let height = lines.len().min(self.rows.saturating_sub(12)).max(1);
        let sel = lines.len() - 1;
        let d = Dialog::new(
            "History",
            60,
            vec![Item::List {
                list: lines.clone(),
                sel,
                top: sel + 1 - height,
                height,
            }],
        );
        self.dialog(d, Purpose::History { lines });
    }

    fn find_dialog(&mut self) {
        let d = Dialog::new(
            "Find File",
            50,
            vec![
                Item::text("File to find:"),
                Item::field("*.*"),
                Item::Sep,
                Item::buttons(&[("Find", Button::Ok), ("Cancel", Button::Cancel)], true),
            ],
        );
        self.dialog(d, Purpose::FindAsk);
    }

    /// The NCD tree of the active panel's drive: Enter goes to the
    /// directory, or puts it into the copy or move dialog `back`.
    fn ncd(&mut self, host: &mut dyn Host, back: Option<Box<(Dialog, Purpose)>>) {
        let a = self.active;
        let drive = self.panels[a].drive();
        let dir = self.panels[a].dir.clone();
        let tree = self.tree(host, drive);
        let lines = tree.lines();
        let paths: Vec<String> = tree.nodes.iter().map(|n| n.path.clone()).collect();
        let sel = tree.find(&dir);
        let height = lines.len().min(self.rows.saturating_sub(12)).max(1);
        let top = (sel + 1).saturating_sub(height);
        let d = Dialog::new(
            "Tree",
            50,
            vec![Item::List {
                list: lines,
                sel,
                top,
                height,
            }],
        );
        self.dialog(d, Purpose::Ncd { tree: paths, back });
    }

    fn dialog_event(
        &mut self,
        host: &mut dyn Host,
        mut d: Box<Dialog>,
        purpose: Purpose,
        event: Event,
    ) -> Option<Outcome> {
        let pressed = match event {
            Event::None => {
                self.mode = Mode::Dialog(d, Box::new(purpose));
                return None;
            }
            Event::Cancel => None,
            Event::Button(b) => Some(b),
        };
        let cancelled = matches!(pressed, None | Some(Button::Cancel | Button::No));
        match purpose {
            Purpose::Copy { sources, kind } => {
                if pressed == Some(Button::Tree) {
                    self.ncd(host, Some(Box::new((*d, Purpose::Copy { sources, kind }))));
                    return None;
                }
                if cancelled {
                    return None;
                }
                let typed = d.field(0);
                let dir = self.panels[self.active].dir.clone();
                if !self.writable(host, &path::qualify(&dir, &typed, path::root)) {
                    self.message("Error", "The disk is write-protected");
                    return None;
                }
                match ops::plan_copy(host, &sources, &dir, &typed, kind) {
                    Ok(job) => self.mode = Mode::Job(Box::new(job)),
                    Err(e) => self.message("Error", &e),
                }
            }
            Purpose::Mkdir => {
                let name = d.field(0);
                if cancelled || name.trim().is_empty() {
                    return None;
                }
                let a = self.active;
                let dir = path::qualify(&self.panels[a].dir, &name, path::root);
                if let Err(e) = make_dirs(host, &dir) {
                    self.error(&format!("Cannot create the directory {}", dir), e);
                    return None;
                }
                // The cursor on the new directory, or the first one made
                // in this one.
                let first = path::qualify(
                    &self.panels[a].dir,
                    name.trim().split('\\').next().unwrap_or(""),
                    path::root,
                );
                let cursor = if path::parent(&first)
                    .is_some_and(|p| p.eq_ignore_ascii_case(&self.panels[a].dir))
                {
                    Some(path::name(&first).to_string())
                } else {
                    None
                };
                self.panels[a].stale = true;
                if let Some(c) = cursor {
                    self.panels[a].go(&self.panels[a].dir.clone(), Some(c));
                }
                self.panels[1 - a].stale = true;
                self.trees.clear();
            }
            Purpose::Delete { sources, marked } => {
                if cancelled {
                    return None;
                }
                let job = ops::plan_delete(&sources, d.checked(0), marked);
                self.mode = Mode::Job(Box::new(job));
            }
            Purpose::Attributes { paths } => {
                if cancelled {
                    return None;
                }
                let bits = [attr::READ_ONLY, attr::ARCHIVE, attr::HIDDEN, attr::SYSTEM];
                for p in &paths {
                    let old = host.attributes(p).unwrap_or(0);
                    let mut new =
                        old & !(attr::READ_ONLY | attr::ARCHIVE | attr::HIDDEN | attr::SYSTEM);
                    for (i, b) in bits.iter().enumerate() {
                        if d.checked(i) {
                            new |= b;
                        }
                    }
                    if new != old
                        && let Err(e) = host.set_attributes(p, new & !attr::DIRECTORY)
                    {
                        self.error(&format!("Cannot change the attributes of {}", p), e);
                        return None;
                    }
                }
                self.panels[self.active].clear_marks();
                self.panels[self.active].stale = true;
            }
            Purpose::Select { on } => {
                if !cancelled {
                    let mask = d.field(0);
                    self.panels[self.active].mark_matching(&mask, on, false);
                }
            }
            Purpose::Filter { panel } => {
                if !cancelled {
                    let p = &mut self.panels[panel];
                    p.filter = [Filter::All, Filter::Executables, Filter::Custom][d.radio().min(2)];
                    let mask = d.field(0);
                    p.mask = if mask.trim().is_empty() {
                        "*.*".to_string()
                    } else {
                        mask
                    };
                    p.stale = true;
                }
            }
            Purpose::Drive { panel, drives } => {
                if cancelled {
                    return None;
                }
                let drive = drives.get(d.selected()).copied().unwrap_or(2);
                let dir = host.current_dir(drive).unwrap_or_else(|| path::root(drive));
                if let Err(e) = host.list(&dir) {
                    let _ = e;
                    let text = format!("Drive {}: is not ready.", (b'A' + drive) as char);
                    self.message("Error", &text);
                    return None;
                }
                if !self.file_view(panel) && self.panels[panel].view != View::Tree {
                    self.panels[panel].view = View::Brief;
                }
                self.panels[panel].go(&dir, None);
                if self.panels[panel].view == View::Tree {
                    self.set_view(host, panel, View::Tree);
                }
                self.panels[panel].visible = true;
                if self.file_view(panel) {
                    self.active = panel;
                }
            }
            Purpose::Quit => {
                if pressed == Some(Button::Yes) {
                    return Some(Outcome::Quit);
                }
            }
            Purpose::History { lines } => {
                if !cancelled && let Some(line) = lines.get(d.selected()) {
                    return Some(Outcome::Run {
                        line: line.clone(),
                        echo: true,
                    });
                }
            }
            Purpose::FindAsk => {
                if cancelled {
                    return None;
                }
                let mask = d.field(0);
                let root = path::root(self.panels[self.active].drive());
                let height = self.rows.saturating_sub(14).max(3);
                let mut dialog = Dialog::new(
                    "Find File",
                    60,
                    vec![
                        Item::text(&format!("Searching {}", root)),
                        Item::List {
                            list: Vec::new(),
                            sel: 0,
                            top: 0,
                            height,
                        },
                        Item::Sep,
                        Item::buttons(&[("Go to", Button::Ok), ("Cancel", Button::Cancel)], true),
                    ],
                );
                dialog.top = Some(2);
                self.mode = Mode::Find(Box::new(Find {
                    mask: if mask.trim().is_empty() {
                        "*.*".to_string()
                    } else {
                        mask
                    },
                    queue: VecDeque::from([root]),
                    found: Vec::new(),
                    dialog,
                }));
            }
            Purpose::Ncd { tree, back } => {
                let chosen = (!cancelled)
                    .then(|| tree.get(d.selected()).cloned())
                    .flatten();
                match back {
                    Some(back) => {
                        let (mut dialog, purpose) = *back;
                        if let Some(dir) = chosen {
                            dialog.set_field(0, &dir);
                        }
                        self.dialog(dialog, purpose);
                    }
                    None => {
                        if let Some(dir) = chosen {
                            let a = self.active;
                            if self.panels[a].view == View::Tree {
                                self.panels[a].dir = dir.clone();
                                self.set_view(host, a, View::Tree);
                                self.follow_tree(a);
                            } else {
                                self.panels[a].go(&dir, None);
                            }
                        }
                    }
                }
            }
            Purpose::Message => {}
        }
        // The dialog's own changes are dropped with it.
        d.items.clear();
        None
    }

    fn writable(&self, host: &dyn Host, target: &str) -> bool {
        let drive = path::drive(target);
        host.drives()
            .iter()
            .find(|d| d.drive == drive)
            .is_none_or(|d| d.writable)
    }

    // ----- Operations and Find file, between keys -----

    /// Run what goes on in the background; whether the screen changed.
    pub(crate) fn run_jobs(&mut self, host: &mut dyn Host) -> bool {
        match std::mem::take(&mut self.mode) {
            Mode::Job(mut job) => {
                if job.question().is_none() {
                    job.run(host, ops_budget());
                }
                if job.done() {
                    self.job_done(&job);
                } else {
                    self.mode = Mode::Job(job);
                }
                true
            }
            Mode::Find(mut find) => {
                let changed = find_step(host, &mut find);
                self.mode = Mode::Find(find);
                changed
            }
            other => {
                self.mode = other;
                false
            }
        }
    }

    fn job_done(&mut self, job: &Job) {
        let a = self.active;
        if !job.cancelled {
            self.panels[a].clear_marks();
        }
        for p in &mut self.panels {
            p.stale = true;
            // The cursor on what was renamed or copied, where it shows.
            if let Some(to) = &job.follow
                && path::parent(to).is_some_and(|d| d.eq_ignore_ascii_case(&p.dir))
            {
                let dir = p.dir.clone();
                p.go(&dir, Some(path::name(to).to_string()));
            }
        }
        if job.dirs_changed {
            self.trees.clear();
        }
        self.quick = None;
    }

    // ----- The mouse -----

    /// The mouse since the last tick: what a click did, and whether the
    /// screen changed.
    pub(crate) fn mouse(&mut self, host: &mut dyn Host, m: &Mouse) -> (Option<Outcome>, bool) {
        let Some(before) = self.presses.replace(m.presses) else {
            return (None, false);
        };
        let mut outcome = None;
        let mut changed = false;
        for (button, &seen) in before.iter().enumerate() {
            if m.presses[button] != seen {
                let (col, row) = m.pressed_at[button];
                outcome = outcome.or(self.click(host, button, col, row));
                changed = true;
            }
        }
        (outcome, changed)
    }

    fn click(
        &mut self,
        host: &mut dyn Host,
        button: usize,
        col: usize,
        row: usize,
    ) -> Option<Outcome> {
        let now = host.now_ms();
        let double = button == 0
            && self.last_click.is_some_and(|(t, c, r)| {
                c == col && r == row && now.saturating_sub(t) <= DOUBLE_CLICK_MS
            });
        self.last_click = if button == 0 && !double {
            Some((now, col, row))
        } else {
            None
        };
        let key_bar_row = self.rows - 1;
        match std::mem::take(&mut self.mode) {
            Mode::Panels | Mode::Search(_) => {
                if self.options.key_bar && row == key_bar_row {
                    return self.key_bar_click(host, col);
                }
                if row == 0 && button == 0 && !self.panels_off {
                    let mut bar = Bar {
                        menu: 0,
                        item: None,
                    };
                    if let Hit::Title(m) = bar.hit(col, 0, self.cols, &self.checks()) {
                        bar.menu = m;
                        bar.open(&self.checks());
                        self.mode = Mode::Menu(bar);
                        return None;
                    }
                }
                self.panel_click(host, button, col, row, double)
            }
            Mode::Menu(mut bar) => {
                let c = self.checks();
                match bar.hit(col, row, self.cols, &c) {
                    Hit::Title(m) => {
                        bar.menu = m;
                        bar.open(&c);
                        self.mode = Mode::Menu(bar);
                    }
                    Hit::Item(i) => {
                        let action = menu::items(bar.menu, &c)[i].action;
                        if action != Action::Separator {
                            return self.act(host, action);
                        }
                        self.mode = Mode::Menu(bar);
                    }
                    Hit::Outside => {}
                }
                None
            }
            Mode::Dialog(mut d, purpose) => {
                let event = d.click(col, row, self.cols, self.rows);
                let event = if double && matches!(event, Event::None) {
                    d.key(Key::Enter)
                } else {
                    event
                };
                self.dialog_event(host, d, *purpose, event)
            }
            Mode::Job(mut job) => {
                if let Some(q) = job.question() {
                    let event = q.click(col, row, self.cols, self.rows);
                    job.answer(event);
                }
                self.mode = Mode::Job(job);
                None
            }
            Mode::View(mut v) => {
                let key = if row == key_bar_row {
                    Some(Key::F((col * 10 / self.cols) as u8 + 1))
                } else if row < self.rows / 2 {
                    Some(Key::PgUp)
                } else {
                    Some(Key::PgDn)
                };
                self.mode = Mode::View(v.clone());
                if let Some(k) = key {
                    match v.key(k, self.cols, self.rows) {
                        Done::Stay => self.mode = Mode::View(v),
                        Done::Quit => self.mode = Mode::Panels,
                        Done::Help => self.mode = Mode::Help(0),
                    }
                }
                None
            }
            Mode::Help(_) => None,
            Mode::Find(mut find) => {
                match find.dialog.click(col, row, self.cols, self.rows) {
                    Event::Button(Button::Ok) => {
                        let at = find.dialog.selected();
                        if let Some(file) = find.found.get(at).cloned() {
                            self.go_to_file(&file);
                        }
                    }
                    Event::Button(_) | Event::Cancel => {}
                    Event::None => self.mode = Mode::Find(find),
                }
                None
            }
        }
    }

    /// A click on the key bar: its function key.
    fn key_bar_click(&mut self, host: &mut dyn Host, col: usize) -> Option<Outcome> {
        let n = (col / (self.cols / 10).max(1)).min(9) as u8 + 1;
        let key = if self.shift & crate::ALT != 0 {
            Key::AltF(n)
        } else if self.shift & crate::CTRL != 0 {
            Key::CtrlF(n)
        } else {
            Key::F(n)
        };
        self.panel_key(host, key)
    }

    fn panel_click(
        &mut self,
        host: &mut dyn Host,
        button: usize,
        col: usize,
        row: usize,
        double: bool,
    ) -> Option<Outcome> {
        if row >= self.panel_height() {
            return None;
        }
        let p = if col < self.cols / 2 { 0 } else { 1 };
        if !self.shows(p) {
            return None;
        }
        let l = self.layout(p);
        if self.panels[p].view == View::Tree {
            self.active = p;
            let (_, top) = self.tree_pos[p];
            if row >= 1 && row < 1 + l.list + 1 {
                let len = self
                    .tree_of(self.panels[p].drive())
                    .map_or(0, |t| t.nodes.len());
                let at = top + row - 1;
                if at < len {
                    self.tree_pos[p].0 = at;
                    self.follow_tree(p);
                }
            }
            return None;
        }
        if !self.file_view(p) {
            return None;
        }
        // The heading and the status line page up and down.
        if row == 1 || (l.status && row == l.h - 2) {
            self.active = p;
            let page = self.panels[p].page(&l) as isize;
            self.panels[p].step(if row == 1 { -page } else { page }, &l);
            return None;
        }
        let at = self.panels[p].hit(&l, col, row)?;
        self.active = p;
        self.panels[p].cursor = at;
        if button == 1 {
            self.panels[p].toggle_mark();
            return None;
        }
        if double {
            self.last_click = None;
            return self.open_current(host);
        }
        None
    }
}

fn ops_budget() -> usize {
    crate::JOB_BUDGET
}

/// A few directories more of Find file's search; whether it found more.
fn find_step(host: &mut dyn Host, find: &mut Find) -> bool {
    if find.queue.is_empty() {
        return false;
    }
    let before = find.found.len();
    for _ in 0..16 {
        let Some(dir) = find.queue.pop_front() else {
            break;
        };
        let Ok(list) = host.list(&dir) else { continue };
        for e in list {
            if e.name == "." || e.name == ".." {
                continue;
            }
            let p = path::join(&dir, &e.name);
            if e.is_dir() && find.queue.len() < 20_000 {
                find.queue.push_back(p.clone());
            }
            if find
                .mask
                .split([' ', ',', ';'])
                .filter(|m| !m.is_empty())
                .any(|m| path::matches(m, &e.name))
            {
                find.found.push(p);
            }
        }
    }
    let status = if find.queue.is_empty() {
        format!("{} found", find.found.len())
    } else {
        format!(
            "Searching {}",
            find.queue.front().cloned().unwrap_or_default()
        )
    };
    find.dialog.set_text(0, &status);
    let found = find.found.clone();
    for item in &mut find.dialog.items {
        if let Item::List { list, .. } = item {
            *list = found;
            break;
        }
    }
    let _ = before;
    true
}

/// Make a directory and those above it it needs.
fn make_dirs(host: &mut dyn Host, dir: &str) -> Result<(), u8> {
    let mut missing = Vec::new();
    let mut at = dir.to_string();
    while host.attributes(&at).is_err() {
        missing.push(at.clone());
        match path::parent(&at) {
            Some(up) => at = up,
            None => break,
        }
    }
    if missing.is_empty() {
        return Err(0x05);
    }
    for d in missing.into_iter().rev() {
        host.mkdir(&d)?;
    }
    Ok(())
}
