//! Drawing the window: tabs, pages, popups, dialogs, the browser, help and
//! the key hints.

use super::browser::{Browser, Row};
use super::dialog::{self, Field, TextField};
use super::draw::{self, Grid, Layout, Rgb};
use super::help;
use super::item::{Applies, Input, Item};
use super::{ConfigUi, Hit, PAGES, Page, Target, UiKey, bootable, fit, fit_hints, hints_width};
use crate::disk::DriveKind;
use crate::mixer::Channel;
use crate::mount::contract_home;
use crate::video::Frame;

impl ConfigUi {
    /// Draw the window over `frame`, if it is open.
    pub fn draw(&mut self, frame: &mut Frame) {
        if !self.open {
            return;
        }
        if self.setup.is_some() {
            return self.draw_setup(frame);
        }
        if self.manual_page_shown() {
            return self.draw_manual_page(frame);
        }
        self.layer = None;
        let layout = Layout::for_frame(frame.width as usize, frame.height as usize);
        let (cols, rows) = (layout.cols, layout.rows);
        if cols < 20 || rows < 8 {
            return;
        }
        let mut g = Grid::new(cols, rows);
        self.hits.clear();

        // Frame, title, tabs and separators.
        g.line(0, 0xC9, 0xCD, 0xBB, draw::BORDER);
        let title = " Rust-DOS settings ";
        g.text((cols - title.len()) / 2, 0, title, draw::BRIGHT);
        for row in 1..rows - 1 {
            g.char(0, row, 0xBA, draw::BORDER);
            g.char(cols - 1, row, 0xBA, draw::BORDER);
        }
        g.line(2, 0xC7, 0xC4, 0xB6, draw::BORDER);
        g.line(rows - 4, 0xC7, 0xC4, 0xB6, draw::BORDER);
        g.line(rows - 1, 0xC8, 0xCD, 0xBC, draw::BORDER);
        self.draw_tabs(&mut g);

        // The page, or what is open over it.
        let content = 3..rows - 4;
        let chooser = self.chooser.is_some();
        self.visible = content.len();
        if chooser {
            self.draw_chooser(&mut g, content.clone());
        } else if self.browser.is_some() {
            self.draw_browser(&mut g, content.clone());
        } else if self.manual.is_some() {
            self.draw_manual_list(&mut g, content.clone());
        } else if self.dialog.is_some() {
            self.draw_dialog(&mut g, content.clone());
        } else if self.image_dialog.is_some() {
            self.draw_image_dialog(&mut g, content.clone());
        } else if self.game_dialog.is_some() {
            self.draw_game_dialog(&mut g, content.clone());
        } else if self.autoexec.is_some() {
            self.draw_autoexec(&mut g, content.clone());
        } else if self.rooms.is_some() {
            self.draw_rooms(&mut g, content.clone());
        } else if self.scenes.is_some() {
            self.draw_scenes(&mut g, content.clone());
        } else if self.phonebook.is_some() {
            self.draw_phonebook(&mut g, content.clone());
        } else if self.almanac.is_some() {
            self.draw_almanac(&mut g, content.clone());
        } else if self.confirm_sc55.is_some() {
            self.draw_sc55_question(&mut g, content.clone());
        } else if self.confirm_glide {
            self.draw_glide_question(&mut g, content.clone());
        } else if self.confirm_launch.is_some() {
            if let Some(pending) = &self.confirm_launch {
                let name = self.games.iter().find(|game| game.id == pending.id).map_or(pending.id.as_str(), |game| game.name.as_str());
                g.text_to(3, content.start + 2, &fit(&format!("Close the running program and start {}?", name), cols - 6), draw::BRIGHT, cols - 3);
                g.text_to(3, content.start + 4, "Enter or Y: close it and start    Esc or N: keep it running", draw::DIM, cols - 3);
            }
        } else if self.page == Page::Drives {
            self.draw_drives(&mut g, content.clone());
        } else if self.page == Page::Games {
            self.draw_games(&mut g, content.clone());
        } else if self.page == Page::States {
            self.draw_states(&mut g, content.clone());
        } else if self.page == Page::Cheats {
            self.draw_cheats(&mut g, content.clone());
        } else if self.page == Page::Achievements {
            self.draw_achievements(&mut g, content.clone());
        } else if self.page == Page::Stats {
            self.draw_stats(&mut g, content.clone());
        } else {
            self.draw_settings(&mut g, content.clone());
        }
        let help_area = self.draw_help(&mut g, content);

        self.draw_hints(&mut g, rows - 3);
        let status_row = rows - 2;
        match &self.status {
            Some(status) => {
                let color = if status.error { draw::ERROR } else { draw::GOOD };
                g.text_to(2, status_row, &fit(&status.text, cols - 4), color, cols - 2);
            }
            None => {
                let text = match &self.config_file {
                    Some(path) if self.active_game.is_some() => {
                        format!("Game profile: {}", contract_home(path, self.home.as_deref()))
                    }
                    Some(path) => format!("Configuration file: {}", contract_home(path, self.home.as_deref())),
                    None => "No configuration file (--no-config): the settings can't be saved".to_string(),
                };
                g.text_to(2, status_row, &fit(&text, cols - 4), draw::DIM, cols - 2);
            }
        }

        draw::render(&g, &layout, frame);
        for plot in std::mem::take(&mut self.plots) {
            draw::plot(frame, &layout, plot.cells, &plot.values, plot.max, plot.color, draw::OPAQUE);
        }
        self.draw_pictures(frame, &layout);
        if let Some(area) = help_area {
            draw::render_area(&g, &layout, frame, draw::PANEL_ALPHA, area);
        }
        self.layout = Some(layout);
    }

    /// The page tabs on row 1: with a space around each title where they
    /// fit, without where that fits, and else as many as fit around the
    /// selected one, with ◄ and ► for those left out.
    fn draw_tabs(&mut self, g: &mut Grid) {
        let end = g.cols - 1;
        let room = end - 2;
        let titles: Vec<&str> = PAGES.iter().map(|p| p.title()).collect();
        let width = |pad: usize, pages: &[&str]| pages.iter().map(|t| t.len() + 2 * pad + 1).sum::<usize>().saturating_sub(1);
        let pad = if width(1, &titles) <= room { 1 } else { 0 };
        let selected = PAGES.iter().position(|&p| p == self.page).unwrap_or(0);
        // The first tab shown: the selected one and the one after it (so
        // it shows there are more) fit after it, with room for the arrows.
        let last = (selected + 1).min(titles.len() - 1);
        let mut first = 0;
        if width(pad, &titles) > room {
            while first < selected && width(pad, &titles[first..=last]) + 4 > room {
                first += 1;
            }
        }
        let mut x = 2;
        if first > 0 {
            g.char(x, 1, 0x11, draw::KEY);
            self.hits.push(Hit { row: 1, col: x, width: 1, target: Target::Key(UiKey::BackTab) });
            x += 2;
        }
        for (i, &page) in PAGES.iter().enumerate().skip(first) {
            let label = format!("{:pad$}{}{:pad$}", "", page.title(), "", pad = pad);
            let more = i + 1 < PAGES.len();
            // Room for this one, and the ► if more follow.
            if x + label.len() + if more { 2 } else { 0 } > end && i > selected {
                g.char(end - 1, 1, 0x10, draw::KEY);
                self.hits.push(Hit { row: 1, col: end - 1, width: 1, target: Target::Key(UiKey::Tab) });
                break;
            }
            let is_selected = page == self.page;
            if is_selected {
                g.background(x, 1, label.len().min(end - x), draw::SELECT);
            }
            let after = g.text_to(x, 1, &label, if is_selected { draw::BRIGHT } else { draw::TEXT }, end);
            self.hits.push(Hit { row: 1, col: x, width: after - x, target: Target::Tab(page) });
            x = after + 1;
        }
    }

    /// Scroll `scroll` so that `selected` is among the `visible` rows.
    pub(super) fn keep_visible(scroll: &mut usize, selected: usize, visible: usize) {
        if selected < *scroll {
            *scroll = selected;
        } else if visible > 0 && selected >= *scroll + visible {
            *scroll = selected + 1 - visible;
        }
    }

    pub(super) fn select_row(&self, g: &mut Grid, row: usize) {
        g.background(1, row, g.cols - 2, draw::SELECT);
    }

    /// A scroll bar left of the right border, beside the `list` rows that
    /// show `total` rows from `scroll` on, if they don't all fit. A click
    /// above or below its thumb pages up or down.
    pub(super) fn draw_scrollbar(&mut self, g: &mut Grid, list: std::ops::Range<usize>, scroll: usize, total: usize) {
        let height = list.len();
        if height == 0 || total <= height {
            return;
        }
        let col = g.cols - 2;
        let thumb = (height * height).div_ceil(total).clamp(1, height);
        let hidden = total - height;
        let top = (scroll.min(hidden) * (height - thumb) + hidden / 2) / hidden;
        for (i, row) in list.enumerate() {
            let key = match i {
                _ if i < top => UiKey::PageUp,
                _ if i >= top + thumb => UiKey::PageDown,
                _ => {
                    g.char(col, row, 0xDB, draw::BORDER);
                    continue;
                }
            };
            g.char(col, row, 0xB0, draw::DIM);
            self.hits.push(Hit { row, col, width: 1, target: Target::Key(key) });
        }
    }

    fn draw_drives(&mut self, g: &mut Grid, content: std::ops::Range<usize>) {
        let cols = g.cols;
        Self::keep_visible(&mut self.scroll, self.row, content.len());
        let label_col = cols - 21;
        let path_width = label_col.saturating_sub(15);
        for (i, row) in (self.scroll..self.row_count()).zip(content.clone()) {
            if i == self.row {
                self.select_row(g, row);
            }
            self.hits.push(Hit { row, col: 1, width: cols - 2, target: Target::Row(i) });
            let Some(info) = self.drives.get(i) else {
                let text = match i - self.drives.len() {
                    0 if self.frontend.host_files => "+ Mount a drive...",
                    0 => "+ Insert a disk or CD image...",
                    _ => "+ Create a disk image...",
                };
                g.text(2, row, text, draw::KEY);
                continue;
            };
            let builtin = info.kind == DriveKind::Virtual;
            let (fg, dim) = if builtin { (draw::DIM, draw::DIM) } else { (draw::TEXT, draw::BRIGHT) };
            g.text(2, row, &info.name(), dim);
            g.text(6, row, info.kind.name(), fg);
            let mut path = match info.image.as_ref().or(info.root.as_ref()) {
                Some(path) => contract_home(path, self.home.as_deref()),
                None => "(built into Rust-DOS)".to_string(),
            };
            if info.images.len() > 1 {
                path = format!("({}/{}) {}", info.image_index + 1, info.images.len(), path);
            }
            g.text_to(14, row, &fit(&path, path_width), fg, label_col - 1);
            g.text_to(label_col, row, &info.label, fg, cols - 9);
            let flags = self.drive_flags(info);
            g.text(cols - 2 - flags.len(), row, &flags, fg);
        }
        self.draw_scrollbar(g, content, self.scroll, self.row_count());
    }

    fn draw_settings(&mut self, g: &mut Grid, content: std::ops::Range<usize>) {
        let cols = g.cols;
        let items = self.items();
        Self::keep_visible(&mut self.scroll, self.row, content.len());
        let value_col = 27.min(cols / 2);
        let note_col = cols - 13;
        // Where the selected setting's value is, for its popup list.
        let mut anchor = None;
        for (i, row) in (self.scroll..items.len()).zip(content.clone()) {
            let item = items[i];
            let selected = i == self.row;
            if selected {
                self.select_row(g, row);
            }
            self.hits.push(Hit { row, col: 1, width: cols - 2, target: Target::Row(i) });
            if item.input() == Input::Link {
                g.text_to(2, row, item.label(), draw::KEY, note_col - 1);
            } else {
                g.text_to(2, row, item.label(), if selected { draw::BRIGHT } else { draw::TEXT }, value_col - 1);
            }
            let note = match item.applies() {
                Applies::Now => "",
                Applies::AtPrompt => "at prompt",
                Applies::NowAndAtPrompt => "now+prompt",
                Applies::NextStart => "next start",
            };
            match item {
                Item::Volume(Channel::Master) if self.muted => {
                    g.text(note_col + 1, row, "muted", draw::ERROR);
                }
                Item::Volume(channel) => self.draw_meter(g, note_col + 1, row, self.levels[channel as usize]),
                _ => {
                    g.text(note_col + 1, row, note, draw::NOTE);
                }
            }

            let end = note_col.saturating_sub(1);
            if item.input() == Input::Link {
                continue;
            }
            // Its list's values under its value.
            if selected {
                anchor = Some((value_col, row));
            }
            let fields = item.fields(&self.settings);
            if !fields.is_empty() {
                let focus = self.draw_fields(g, row, i, &fields, (value_col, end), selected);
                if selected && let Some(col) = focus {
                    anchor = Some((col - 1, row));
                }
                continue;
            }
            if selected && let Some(field) = &self.edit {
                let width = end.saturating_sub(value_col);
                let (text, cursor) = field.view(width);
                g.background(value_col, row, width, draw::FIELD);
                g.text_to(value_col, row, &text, draw::BRIGHT, end);
                g.background(value_col + cursor, row, 1, draw::SELECT);
                continue;
            }
            let value = fit(&item.value(&self.settings, self.home.as_deref()), end.saturating_sub(value_col + 4));
            // What Left and Right step has their arrows; a value picked
            // from a list or typed is a button too.
            if matches!(item.input(), Input::Choice | Input::Slider | Input::Presets) {
                g.char(value_col, row, 0x11, draw::KEY);
                self.hits.push(Hit { row, col: value_col, width: 1, target: Target::Step(i, -1) });
                let after = g.text_to(value_col + 2, row, &value, draw::BRIGHT, end);
                if item.input() != Input::Slider {
                    self.hits.push(Hit { row, col: value_col + 2, width: after - value_col - 2, target: Target::Button(i) });
                }
                g.char(after + 1, row, 0x10, draw::KEY);
                self.hits.push(Hit { row, col: after + 1, width: 1, target: Target::Step(i, 1) });
            } else {
                let after = Self::draw_button(g, value_col, row, &value, end, false);
                self.hits.push(Hit { row, col: value_col, width: after - value_col, target: Target::Button(i) });
            }
        }
        self.draw_scrollbar(g, content.clone(), self.scroll, items.len());
        if let Some(anchor) = anchor {
            self.draw_popup(g, content, anchor);
        }
    }

    /// The popup list of values from (`col`, `row`), where the value it
    /// picks is: below it, or above it where it doesn't fit below, else as
    /// low in the rows `content` as it fits, scrolled where it is longer
    /// than they are.
    fn draw_popup(&mut self, g: &mut Grid, content: std::ops::Range<usize>, (col, row): (usize, usize)) {
        let home = self.home.as_deref();
        let Some(popup) = &mut self.popup else { return };
        let labels: Vec<String> = popup.choices.iter().map(|s| popup.item.value(s, home)).collect();
        let height = (labels.len() + 2).min(content.len());
        let top = if row + 1 + height <= content.end {
            row + 1
        } else if row >= content.start + height {
            row - height
        } else {
            content.end - height
        };
        let list = height.saturating_sub(2);
        if list == 0 {
            return;
        }
        popup.visible = list;
        Self::keep_visible(&mut popup.scroll, popup.selected, list);

        // Framed, with a space either side of the values, left of the
        // window's right border.
        let longest = labels.iter().map(|l| l.chars().count()).max().unwrap_or(0);
        let width = (longest + 4).min(g.cols - 2);
        let left = col.min(g.cols - 1 - width);
        let right = left + width - 1;
        let bottom = top + height - 1;
        for r in top..=bottom {
            g.background(left, r, width, draw::FIELD);
            g.text_to(left, r, &" ".repeat(width), draw::TEXT, right + 1);
            g.char(left, r, 0xB3, draw::BORDER);
            g.char(right, r, 0xB3, draw::BORDER);
            self.hits.push(Hit { row: r, col: left, width, target: Target::Popup });
        }
        for x in left + 1..right {
            g.char(x, top, 0xC4, draw::BORDER);
            g.char(x, bottom, 0xC4, draw::BORDER);
        }
        g.char(left, top, 0xDA, draw::BORDER);
        g.char(right, top, 0xBF, draw::BORDER);
        g.char(left, bottom, 0xC0, draw::BORDER);
        g.char(right, bottom, 0xD9, draw::BORDER);
        // ▲ and ▼ where there are more above and below.
        if popup.scroll > 0 {
            g.char(right - 2, top, 0x1E, draw::KEY);
        }
        if popup.scroll + list < labels.len() {
            g.char(right - 2, bottom, 0x1F, draw::KEY);
        }

        for (i, r) in (popup.scroll..labels.len()).zip(top + 1..bottom) {
            let selected = i == popup.selected;
            if selected {
                g.background(left + 1, r, width - 2, draw::SELECT);
            }
            let text = fit(&labels[i], width.saturating_sub(4));
            g.text_to(left + 2, r, &text, if selected { draw::BRIGHT } else { draw::TEXT }, right - 1);
            self.hits.push(Hit { row: r, col: left + 1, width: width - 2, target: Target::PopupRow(i) });
        }
    }

    /// A button at (`col`, `row`), up to the column `end`: `value` in
    /// brackets, which Enter or a click changes. Returns the column after
    /// it.
    fn draw_button(g: &mut Grid, col: usize, row: usize, value: &str, end: usize, focused: bool) -> usize {
        if focused {
            g.background(col, row, (value.chars().count() + 2).min(end.saturating_sub(col)), draw::FIELD);
        }
        let after = g.text_to(col, row, "[", draw::KEY, end);
        let after = g.text_to(after, row, value, draw::BRIGHT, end);
        g.text_to(after, row, "]", draw::KEY, end)
    }

    /// The fields of row `i`, a row of several, side by side in the
    /// columns `cols` of grid row `row`: each by its name with its value
    /// on a button of its own, the one Left and Right go to marked while
    /// the row is selected. Returns the column of that one's button, if it
    /// fit.
    fn draw_fields(
        &mut self,
        g: &mut Grid,
        row: usize,
        i: usize,
        fields: &[Item],
        cols: (usize, usize),
        selected: bool,
    ) -> Option<usize> {
        let (mut x, end) = cols;
        let focus = self.field.min(fields.len() - 1);
        let mut focus_col = None;
        for (f, field) in fields.iter().enumerate() {
            if f > 0 {
                x += 2;
            }
            let name = field.label();
            if !name.is_empty() {
                x = g.text_to(x, row, name, draw::TEXT, end) + 1;
            }
            let value = field.value(&self.settings, self.home.as_deref());
            if x + value.chars().count() + 2 > end {
                break;
            }
            if f == focus {
                focus_col = Some(x);
            }
            let after = Self::draw_button(g, x, row, &value, end, selected && f == focus);
            self.hits.push(Hit { row, col: x, width: after - x, target: Target::RowField(i, f) });
            x = after;
        }
        focus_col
    }

    /// A level meter of `METER` cells at (`col`, `row`): 6 dB a cell
    /// from -60 dB, green, then yellow for the loudest, red where the
    /// sound clips.
    fn draw_meter(&self, g: &mut Grid, col: usize, row: usize, level: f32) {
        const METER: usize = 10;
        let lit = if level <= 0.0 {
            0
        } else {
            let db = 20.0 * level.log10();
            ((db + 60.0) / 6.0).ceil().clamp(0.0, METER as f32) as usize
        };
        for cell in 0..METER {
            let color = match cell {
                _ if cell >= lit => draw::DIM,
                _ if cell == METER - 1 && level >= 1.0 => draw::ERROR,
                _ if cell >= METER - 3 => draw::KEY,
                _ => draw::GOOD,
            };
            g.char(col + cell, row, if cell < lit { 0xFE } else { 0xFA }, color);
        }
    }

    fn draw_dialog(&mut self, g: &mut Grid, content: std::ops::Range<usize>) {
        let Some(dialog) = &self.dialog else { return };
        let cols = g.cols;
        let top = content.start;
        let bottom = content.end;
        let label_col = 4;
        let value_col = 16.min(cols / 3);
        let end = cols - 3;
        g.text(2, top, &dialog.title(), draw::BRIGHT);
        let mut hits = Vec::new();
        let mut put = |g: &mut Grid, field: Field, row: usize, label: &str, draw: &dyn Fn(&mut Grid, bool) -> (usize, usize)| {
            if row >= bottom {
                return;
            }
            let focused = dialog.focus == field;
            if !label.is_empty() {
                g.text(label_col, row, label, if focused { draw::BRIGHT } else { draw::TEXT });
            }
            let (col, width) = draw(g, focused);
            hits.push(Hit { row, col, width, target: Target::Field(field) });
        };
        let choice = |text: String, row: usize, editable: bool| {
            move |g: &mut Grid, focused: bool| {
                let width = text.chars().count() + 4;
                if focused {
                    g.background(value_col, row, width, draw::SELECT);
                }
                if editable {
                    g.char(value_col, row, 0x11, draw::KEY);
                    g.char(value_col + width - 1, row, 0x10, draw::KEY);
                }
                g.text(value_col + 2, row, &text, draw::BRIGHT);
                (value_col, width)
            }
        };
        let text_field = |field: &TextField, row: usize, width: usize| {
            let field = field.clone();
            move |g: &mut Grid, focused: bool| {
                let width = width.min(end.saturating_sub(value_col));
                g.background(value_col, row, width, draw::FIELD);
                let (text, cursor) = field.view(width);
                g.text_to(value_col, row, &text, draw::BRIGHT, value_col + width);
                if focused {
                    g.background(value_col + cursor, row, 1, draw::SELECT);
                }
                (value_col, width)
            }
        };
        let letter = dialog::drive_choice(dialog.drive);
        let letter_width = letter.chars().count();
        put(g, Field::Drive, top + 1, "Drive", &choice(letter, top + 1, !dialog.existing));
        if dialog.numbered() && top + 1 < bottom {
            let col = value_col + letter_width + 5;
            g.text_to(col, top + 1, &fit("(no DOS drive letter)", end.saturating_sub(col)), draw::DIM, end);
        }
        let buttons = dialog.fields();
        put(g, Field::Path, top + 2, "Path", &text_field(&dialog.path, top + 2, cols));
        let button = |text: &'static str, row: usize, col: usize| {
            move |g: &mut Grid, focused: bool| {
                if focused {
                    g.background(col, row, text.len(), draw::SELECT);
                }
                g.text(col, row, text, if focused { draw::BRIGHT } else { draw::KEY });
                (col, text.len())
            }
        };
        put(g, Field::Browse, top + 3, "", &button("[ Browse... ]", top + 3, value_col));
        put(g, Field::Kind, top + 4, "Type", &choice(dialog.kind.name().to_string(), top + 4, !dialog.kind_fixed()));
        if buttons.contains(&Field::Label) {
            put(g, Field::Label, top + 5, "Label", &text_field(&dialog.label, top + 5, 12));
            if top + 5 < bottom {
                g.text(value_col + 14, top + 5, "(default empty)", draw::DIM);
            }
        }
        let ro = if dialog.read_only { "yes" } else { "no" }.to_string();
        put(g, Field::ReadOnly, top + 6, "Read-only", &choice(ro, top + 6, true));
        if buttons.contains(&Field::Ide) {
            put(g, Field::Ide, top + 7, "IDE slot", &choice(dialog.ide_name(), top + 7, true));
            if top + 7 < bottom {
                let col = value_col + dialog.ide_name().chars().count() + 5;
                g.text_to(col, top + 7, &fit("(for a booted system)", end.saturating_sub(col)), draw::DIM, end);
            }
        }
        if buttons.contains(&Field::Overlay) {
            put(g, Field::Overlay, top + 8, "Write to", &text_field(&dialog.overlay, top + 8, cols));
            put(g, Field::OverlayBrowse, top + 9, "", &button("[ Browse... ]", top + 9, value_col));
            if top + 9 < bottom {
                let hint = match dialog.delta_name() {
                    Some(name) if !dialog.overlay.text().trim().is_empty() => format!("(changes go to {} there)", name),
                    _ => "(empty = write to mounted image)".to_string(),
                };
                g.text_to(value_col + 15, top + 9, &fit(&hint, end.saturating_sub(value_col + 15)), draw::DIM, end);
            }
        }
        if buttons.contains(&Field::BootFlag) {
            let boot = if dialog.boot { "yes" } else { "no" }.to_string();
            put(g, Field::BootFlag, top + 10, "Auto-boot", &choice(boot, top + 10, true));
            if top + 10 < bottom {
                g.text(value_col + 10, top + 10, "(disk images only)", draw::DIM);
            }
        }
        let mut col = value_col;
        for (field, text) in [
            (Field::Mount, "[ Mount ]"),
            (Field::Boot, "[ Boot ]"),
            (Field::Unmount, "[ Unmount ]"),
            (Field::Cancel, "[ Cancel ]"),
        ] {
            if buttons.contains(&field) {
                put(g, field, top + 12, "", &button(text, top + 12, col));
                col += text.len() + 2;
            }
        }
        self.hits.extend(hits);
    }

    pub(super) fn draw_browser(&mut self, g: &mut Grid, content: std::ops::Range<usize>) {
        let Some((browser, _)) = &self.browser else { return };
        let cols = g.cols;
        g.text(2, content.start, browser.title, draw::BRIGHT);
        let mut top = content.start + 1;
        if !browser.drives.is_empty() {
            let (current, mut col) = (browser.drive(), 2);
            for (i, root) in browser.drives.iter().enumerate() {
                let text = format!("[{}]", Browser::drive_name(root));
                let width = text.chars().count();
                if col + width > cols - 2 {
                    break;
                }
                if current == Some(i) {
                    g.background(col, top, width, draw::SELECT);
                }
                g.text(col, top, &text, if current == Some(i) { draw::BRIGHT } else { draw::KEY });
                self.hits.push(Hit { row: top, col, width, target: Target::BrowserDrive(i) });
                col += width + 1;
            }
            top += 1;
        }
        let dir = contract_home(&browser.dir, self.home.as_deref());
        g.text_to(2, top, &fit(&dir, cols - 4), draw::DIM, cols - 2);
        let list = top + 1..content.end;
        Self::keep_visible(&mut self.browser_scroll, browser.selected, list.len());
        let (scroll, total) = (self.browser_scroll, browser.rows());
        let separator = std::path::MAIN_SEPARATOR;
        for (i, row) in (self.browser_scroll..browser.rows()).zip(list.clone()) {
            if i == browser.selected {
                g.background(1, row, cols - 2, draw::SELECT);
            }
            self.hits.push(Hit { row, col: 1, width: cols - 2, target: Target::BrowserRow(i) });
            let (text, color): (String, Rgb) = match browser.row(i) {
                Some(Row::UseThisDirectory) => ("[ Use this directory ]".to_string(), draw::KEY),
                Some(Row::Entry(e)) if e.is_dir && e.name != ".." => (format!("{}{}", e.name, separator), draw::TEXT),
                Some(Row::Entry(e)) if e.is_dir => (e.name.clone(), draw::TEXT),
                Some(Row::Entry(e)) => (e.name.clone(), draw::BRIGHT),
                None => continue,
            };
            g.text_to(4, row, &fit(&text, cols - 7), color, cols - 2);
        }
        self.draw_scrollbar(g, list, scroll, total);
    }

    /// The help topic on what is under the cursor: the setting selected,
    /// or the page, or the dialog open over it.
    fn help_topic(&self) -> Option<&'static str> {
        if let Some((_, pick)) = &self.browser {
            return Some(pick.help());
        }
        if self.manual.is_some() {
            Some("manuals")
        } else if self.dialog.is_some() {
            Some("mount")
        } else if self.image_dialog.is_some() {
            Some("new-image")
        } else if self.game_dialog.is_some() {
            Some("new-game")
        } else if self.autoexec.is_some() {
            Some("autoexec")
        } else if self.rooms.is_some() {
            Some("rooms")
        } else if self.scenes.is_some() {
            Some("vr-scenes")
        } else if self.phonebook.is_some() {
            Some("phonebook")
        } else if self.almanac.is_some() {
            Some("almanac")
        } else {
            self.page.help().or_else(|| self.item().map(Item::help))
        }
    }

    /// F1: show the help on what is under the cursor, or close it.
    pub(super) fn toggle_help(&mut self) {
        if self.help.take().is_none() {
            self.help = self.help_topic().and_then(help::topic).map(help::HelpView::new);
        }
    }

    /// The help's keys: Up, Down and the rest scroll it, and Esc, Enter
    /// and F1 close it.
    pub(super) fn help_key(&mut self, key: UiKey) {
        let Some(help) = &mut self.help else { return };
        let (last, page) = (help.max_scroll(), help.visible.saturating_sub(1).max(1));
        help.scroll = match key {
            UiKey::Up => help.scroll.saturating_sub(1),
            UiKey::Down => (help.scroll + 1).min(last),
            UiKey::PageUp => help.scroll.saturating_sub(page),
            UiKey::PageDown | UiKey::Char(' ') => (help.scroll + page).min(last),
            UiKey::Home => 0,
            UiKey::End => last,
            UiKey::Esc | UiKey::Enter | UiKey::Backspace => {
                self.help = None;
                return;
            }
            _ => return,
        };
    }

    /// The help over the rows `content`, from their top: its topic's title
    /// in the frame, its text below, scrolled, with ▲ and ▼ where there is
    /// more. Returns the columns and rows it covers, for the pictures and
    /// graphs drawn in pixels to leave alone.
    fn draw_help(&mut self, g: &mut Grid, content: std::ops::Range<usize>) -> Option<(std::ops::Range<usize>, std::ops::Range<usize>)> {
        let help = self.help.as_mut()?;
        let width = (g.cols - 2).min(76);
        if content.len() < 3 || width < 12 {
            return None;
        }
        let left = (g.cols - width) / 2;
        let right = left + width - 1;
        // As high as its text, up to the page's rows.
        let lines = help::layout(help.body(), width - 4);
        let top = content.start;
        let bottom = (top + lines.len() + 1).min(content.end - 1);
        for r in top..=bottom {
            g.background(left, r, width, draw::FIELD);
            g.text_to(left, r, &" ".repeat(width), draw::TEXT, right + 1);
            g.char(left, r, 0xB3, draw::BORDER);
            g.char(right, r, 0xB3, draw::BORDER);
            self.hits.push(Hit { row: r, col: left, width, target: Target::Help });
        }
        for x in left + 1..right {
            g.char(x, top, 0xC4, draw::BORDER);
            g.char(x, bottom, 0xC4, draw::BORDER);
        }
        g.char(left, top, 0xDA, draw::BORDER);
        g.char(right, top, 0xBF, draw::BORDER);
        g.char(left, bottom, 0xC0, draw::BORDER);
        g.char(right, bottom, 0xD9, draw::BORDER);
        let title = fit(&format!(" {} ", help.title()), width.saturating_sub(8));
        g.text(left + 2, top, &title, draw::BRIGHT);

        help.lines = lines.len();
        help.visible = bottom - top - 1;
        help.scroll = help.scroll.min(help.max_scroll());
        for (line, r) in lines.iter().skip(help.scroll).zip(top + 1..bottom) {
            let mut x = left + 2;
            for span in line {
                x = g.text_to(x, r, &span.text, span.color, right - 1);
            }
        }
        if help.scroll > 0 {
            g.char(right - 2, top, 0x1E, draw::KEY);
            self.hits.push(Hit { row: top, col: right - 2, width: 1, target: Target::Key(UiKey::PageUp) });
        }
        if help.scroll < help.max_scroll() {
            g.char(right - 2, bottom, 0x1F, draw::KEY);
            self.hits.push(Hit { row: bottom, col: right - 2, width: 1, target: Target::Key(UiKey::PageDown) });
        }
        Some((left..right + 1, top..bottom + 1))
    }

    /// The key hints for what is showing, each clickable.
    fn draw_hints(&mut self, g: &mut Grid, row: usize) {
        use UiKey::*;
        let mut hints: Vec<(&str, &str, UiKey)> = if self.help.is_some() {
            vec![("\u{2191}\u{2193}", "Scroll", Down), ("Esc", "Close", Esc)]
        } else if let Some(chooser) = &self.chooser {
            if chooser.has_categories() {
                vec![("Enter", "Start", Enter), ("\u{2190}\u{2192}", "Option", Right), ("Esc", "Cancel", Esc)]
            } else {
                vec![("Enter", "Start", Enter), ("Esc", "Cancel", Esc)]
            }
        } else if let Some((browser, _)) = &self.browser {
            let mut hints = vec![("Enter", "Open", Enter), ("Bksp", "Up", Backspace)];
            if !browser.drives.is_empty() {
                hints.push(("\u{2190}\u{2192}", "Drive", Right));
            }
            hints.push(("Esc", "Cancel", Esc));
            hints
        } else if self.manual.is_some() {
            vec![("Enter", "Open", Enter), ("Ins", "Add", Insert), ("F1", "Help", Help), ("Esc", "Back", Esc)]
        } else if self.dialog.is_some() {
            let boot = self.dialog.as_ref().is_some_and(|d| d.focus == Field::Boot);
            vec![("Tab", "Next", Tab), ("Enter", if boot { "Boot" } else { "Mount" }, Enter), ("Esc", "Cancel", Esc)]
        } else if self.game_dialog.is_some() || self.image_dialog.is_some() {
            vec![("Tab", "Next", Tab), ("Enter", "Create", Enter), ("Esc", "Cancel", Esc)]
        } else if self.autoexec.is_some() {
            vec![("F2", "Save", Save), ("Esc", "Cancel", Esc)]
        } else if self.rooms.is_some() {
            self.room_hints()
        } else if self.scenes.is_some() {
            self.scene_hints()
        } else if self.phonebook.is_some() {
            self.phone_hints()
        } else if self.almanac.is_some() {
            self.almanac_hints()
        } else if self.confirm_launch.is_some() {
            vec![("Enter", "Close and start", Enter), ("Esc", "Keep running", Esc)]
        } else if self.confirm_delete.is_some() {
            vec![("Enter", if self.confirm_reset { "Reset" } else { "Delete" }, Enter), ("Esc", "Keep", Esc)]
        } else if self.confirm_sc55.is_some() || self.confirm_glide {
            vec![("Enter", "Download", Enter), ("Esc", "Cancel", Esc)]
        } else if self.page == Page::States {
            vec![
                ("Enter", "Load", Enter),
                ("Ins", "Save", Insert),
                ("Del", "Delete", Delete),
                ("Tab", "Page", Tab),
                ("Esc", "Close", Esc),
            ]
        } else if self.popup.is_some() {
            let mut hints = vec![("\u{2191}\u{2193}", "Select", Down), ("Enter", "OK", Enter), ("Esc", "Cancel", Esc)];
            if self.item().is_some_and(|item| !item.fields(&self.settings).is_empty()) {
                hints.insert(1, ("\u{2190}\u{2192}", "Field", Right));
            }
            hints
        } else if self.edit.is_some() || self.cheats.edit.is_some() || self.achievements.edit.is_some() {
            vec![("Enter", "OK", Enter), ("Esc", "Cancel", Esc)]
        } else if self.page == Page::Cheats {
            self.cheats_hints()
        } else if self.page == Page::Achievements {
            self.achievements_hints()
        } else if self.page == Page::Stats {
            let overlay = if self.overlay { "Hide overlay" } else { "Show overlay" };
            vec![("Tab", "Page", Tab), ("Esc", "Close", Esc), ("Ctrl+Shift+F12", overlay, Overlay)]
        } else if self.page == Page::Games {
            vec![
                ("Enter", "Launch", Enter),
                ("Ins", "New", Insert),
                ("Del", "Delete", Delete),
                ("M", "Manuals", Char('m')),
                ("P", "Pad", Char('p')),
                ("R", "Reset", Char('r')),
                ("Tab", "Page", Tab),
                ("F2", "Save", Save),
                ("Esc", "Close", Esc),
            ]
        } else if self.page == Page::Drives {
            let (mount, unmount) = if self.frontend.host_files { ("Mount", "Unmount") } else { ("Insert", "Eject") };
            let mut hints = vec![
                ("Enter", "Change", Enter),
                ("Ins", mount, Insert),
                ("Del", unmount, Delete),
                ("Tab", "Page", Tab),
                ("F2", "Save", Save),
                ("Esc", "Close", Esc),
            ];
            if let Some(info) = self.drives.get(self.row) {
                if bootable(info) {
                    hints.insert(3, ("B", "Boot", Char('b')));
                } else if self.shared_unit(info.drive).is_some() {
                    hints.insert(3, ("S", "Sync", Char('s')));
                } else if self.folder_cd(info) {
                    hints.insert(3, ("R", "Reinsert", Char('r')));
                }
            }
            hints
        } else {
            let several = self.item().is_some_and(|item| !item.fields(&self.settings).is_empty());
            let mut hints = match self.item().map(Item::input) {
                _ if several => vec![("\u{2190}\u{2192}", "Field", Right), ("Enter", "List", Enter)],
                Some(Input::Choice) => vec![("\u{2190}\u{2192}", "Change", Right), ("Enter", "List", Enter)],
                Some(Input::Slider | Input::Presets) => {
                    vec![("\u{2190}\u{2192}", "Change", Right), ("Enter", "Type", Enter)]
                }
                Some(Input::Text) => vec![("Enter", "Type", Enter)],
                Some(Input::File) => vec![("Enter", "Pick", Enter), ("Del", "None", Delete)],
                Some(Input::Link)
                    if matches!(
                        self.item(),
                        Some(
                            Item::Rooms | Item::Phonebook | Item::AlmanacFetch | Item::AlmanacReport
                        )
                    ) =>
                {
                    vec![("Enter", "Open", Enter)]
                }
                Some(Input::Link) => vec![("Enter", "Edit", Enter)],
                None => Vec::new(),
            };
            hints.extend([("Tab", "Page", Tab), ("F2", "Save", Save), ("Esc", "Close", Esc)]);
            hints
        };
        if self.help.is_none() && self.help_topic().is_some() {
            hints.push(("F1", "Help", Help));
        }
        fit_hints(&mut hints, g.cols - 4);
        let gap = if hints_width(&hints, 2) <= g.cols - 4 { 2 } else { 1 };
        let end = g.cols - 2;
        let mut x = 2;
        for (key, action, ui_key) in hints {
            let width = key.chars().count() + 1 + action.len();
            if x + width > end {
                break;
            }
            let after = g.text(x, row, key, draw::KEY);
            g.text(after + 1, row, action, draw::TEXT);
            self.hits.push(Hit { row, col: x, width, target: Target::Key(ui_key) });
            x += width + gap;
        }
    }
}
