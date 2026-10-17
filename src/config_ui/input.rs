//! Keys, the mouse wheel and clicks: page and settings navigation, popups
//! and value entry.

use super::dialog::{Field, TextField};
use super::item::{Applies, Input, Item};
use super::{ConfigUi, Host, PAGES, Page, Pick, Popup, Target, UiKey};
use crate::mount::contract_home;

impl ConfigUi {
    /// Typed text, from the keyboard's text input.
    pub fn text(&mut self, text: &str, host: &mut dyn Host) {
        for c in text.chars().filter(|c| !c.is_control()) {
            self.key(UiKey::Char(c), host);
        }
    }

    pub fn key(&mut self, key: UiKey, host: &mut dyn Host) {
        if !self.open {
            return;
        }
        if self.setup.is_some() {
            return self.setup_key(key, host);
        }
        if key == UiKey::Overlay {
            self.overlay_key();
        } else if key == UiKey::Help {
            self.toggle_help();
        } else if self.help.is_some() {
            self.help_key(key);
        } else if self.confirm_launch.is_some() {
            self.confirm_launch_key(key, host);
        } else if self.chooser.is_some() {
            self.chooser_key(key, host);
        } else if self.browser.is_some() {
            self.browser_key(key, host);
        } else if self.manual.is_some() {
            self.manual_key(key, host);
        } else if self.dialog.is_some() {
            self.dialog_key(key, host);
        } else if self.image_dialog.is_some() {
            self.image_dialog_key(key, host);
        } else if self.game_dialog.is_some() {
            self.game_dialog_key(key, host);
        } else if self.autoexec.is_some() {
            self.autoexec_key(key, host);
        } else if self.rooms.is_some() {
            self.rooms_key(key, host);
        } else if self.scenes.is_some() {
            self.scenes_key(key, host);
        } else if self.phonebook.is_some() {
            self.phonebook_key(key, host);
        } else if self.cheats.edit.is_some() {
            self.cheats_edit_key(key, host);
        } else if self.achievements.edit.is_some() {
            self.achievements_edit_key(key, host);
        } else if self.popup.is_some() {
            self.popup_key(key, host);
        } else if self.edit.is_some() {
            self.edit_key(key, host);
        } else {
            self.page_key(key, host);
        }
    }

    /// Mouse wheel: `dy` > 0 is up.
    pub fn wheel(&mut self, dy: i32, host: &mut dyn Host) {
        if self.manual_page_shown() && self.manual_wheel(dy) {
            return;
        }
        let key = if dy > 0 { UiKey::Up } else { UiKey::Down };
        for _ in 0..dy.unsigned_abs().min(5) {
            self.key(key, host);
        }
    }

    /// A left click at frame pixel (`x`, `y`).
    pub fn click(&mut self, x: i32, y: i32, host: &mut dyn Host) {
        let hit = self.layout.and_then(|l| l.cell_at(x, y)).and_then(|(col, row)| {
            self.hits
                .iter()
                .rev()
                .find(|h| h.row == row && (h.col..h.col + h.width).contains(&col))
                .map(|h| (h.target, col - h.col))
        });
        // A click off the help closes it, as one off the popup list does,
        // and does nothing else; the key hints still work.
        if self.help.is_some() && !matches!(hit, Some((Target::Help | Target::Key(_), _))) {
            self.help = None;
            return;
        }
        if self.popup.is_some() && !matches!(hit, Some((Target::PopupRow(_) | Target::Popup, _))) {
            self.popup = None;
            return;
        }
        let Some((target, into)) = hit else { return };
        if self.chooser.is_some() {
            self.chooser_clicked(target, host);
            return;
        }
        // The list of manuals' rows.
        if let (Some(view), Target::Row(i)) = (&mut self.manual, target)
            && self.browser.is_none()
        {
            if view.select(i) {
                self.key(UiKey::Enter, host);
            }
            return;
        }
        match target {
            Target::Tab(page) => {
                if self.manual.is_none()
                    && self.dialog.is_none()
                    && self.image_dialog.is_none()
                    && self.browser.is_none()
                    && self.game_dialog.is_none()
                    && self.autoexec.is_none()
                    && self.rooms.is_none()
                    && self.scenes.is_none()
                    && self.phonebook.is_none()
                {
                    self.edit = None;
                    self.cheats.edit = None;
                    self.achievements.edit = None;
                    self.show_page(page);
                }
            }
            Target::Row(i) if i == self.row => self.key(UiKey::Enter, host),
            Target::Row(i) => {
                self.edit = None;
                self.cheats.edit = None;
                self.achievements.edit = None;
                self.select(i);
            }
            Target::Button(i) => {
                self.edit = None;
                self.cheats.edit = None;
                self.achievements.edit = None;
                self.select(i);
                self.key(UiKey::Enter, host);
            }
            Target::RowField(i, field) => {
                self.select(i);
                self.field = field;
                self.key(UiKey::Enter, host);
            }
            Target::Step(i, dir) => {
                self.edit = None;
                self.cheats.edit = None;
                self.achievements.edit = None;
                self.row = i;
                self.key(if dir < 0 { UiKey::Left } else { UiKey::Right }, host);
            }
            Target::Key(key) => self.key(key, host),
            Target::Field(field) => {
                if let Some(dialog) = &mut self.dialog {
                    let again = dialog.focus == field;
                    dialog.focus = field;
                    match field {
                        Field::Browse | Field::Mount | Field::Boot | Field::Unmount | Field::Cancel => self.key(UiKey::Enter, host),
                        Field::Drive | Field::Kind | Field::ReadOnly | Field::Ide | Field::BootFlag if again => dialog.step(field, 1),
                        _ => {}
                    }
                }
            }
            Target::GameField(field) => self.game_field_clicked(field, host),
            Target::ImageField(field) => self.image_field_clicked(field, host),
            Target::BrowserRow(i) => {
                if let Some((browser, _)) = &mut self.browser {
                    if browser.selected == i {
                        self.key(UiKey::Enter, host);
                    } else {
                        browser.select(i);
                    }
                }
            }
            Target::BrowserDrive(i) => {
                if let Some((browser, _)) = &mut self.browser {
                    match browser.open_drive(i) {
                        Ok(()) => self.status = None,
                        Err(e) => self.error(e),
                    }
                }
            }
            Target::EditorLine(i) => self.autoexec_clicked(i, into),
            Target::RoomRow(_) | Target::RoomField(_) | Target::RoomButton(_) | Target::RoomsOnline(_) => {
                self.room_clicked(target, host)
            }
            Target::SceneRow(_) => self.scene_clicked(target, host),
            Target::PhoneRow(_) => self.phone_clicked(target, host),
            Target::PopupRow(i) => {
                if let Some(popup) = &mut self.popup {
                    popup.selected = i;
                }
                self.key(UiKey::Enter, host);
            }
            Target::Popup | Target::Help => {}
        }
    }

    pub(super) fn show_page(&mut self, page: Page) {
        if page != self.page {
            self.page = page;
            self.select(0);
            self.scroll = 0;
            self.confirm_delete = None;
            self.confirm_sc55 = None;
            self.confirm_glide = false;
        }
    }

    /// Select row `row`, at its first field.
    pub(super) fn select(&mut self, row: usize) {
        if row != self.row {
            self.row = row;
            self.field = 0;
        }
    }

    /// Up, Down, Page Up and Down, Home and End on a list of `rows` rows.
    pub(super) fn navigate(key: UiKey, selected: usize, rows: usize, page: usize) -> Option<usize> {
        let last = rows.saturating_sub(1);
        Some(match key {
            UiKey::Up => selected.saturating_sub(1),
            UiKey::Down => (selected + 1).min(last),
            UiKey::PageUp => selected.saturating_sub(page.max(1)),
            UiKey::PageDown => (selected + page.max(1)).min(last),
            UiKey::Home => 0,
            UiKey::End => last,
            _ => return None,
        })
    }

    fn page_key(&mut self, key: UiKey, host: &mut dyn Host) {
        if self.confirm_delete.is_some() && self.page == Page::States {
            return self.states_key(key, host);
        }
        if self.confirm_delete.is_some() {
            return self.games_key(key, host);
        }
        if self.confirm_sc55.is_some() {
            return self.sc55_confirm_key(key);
        }
        if self.confirm_glide {
            return self.glide_confirm_key(key);
        }
        if let Some(row) = Self::navigate(key, self.row, self.row_count(), self.visible) {
            self.select(row);
            return;
        }
        let at = PAGES.iter().position(|&p| p == self.page).unwrap_or(0);
        match key {
            UiKey::Esc => self.close(),
            UiKey::Tab => self.show_page(PAGES[(at + 1) % PAGES.len()]),
            UiKey::BackTab => self.show_page(PAGES[(at + PAGES.len() - 1) % PAGES.len()]),
            UiKey::Save => self.save(host),
            _ if self.page == Page::Drives => self.drives_key(key, host),
            _ if self.page == Page::Games => self.games_key(key, host),
            _ if self.page == Page::States => self.states_key(key, host),
            _ if self.page == Page::Cheats => self.cheats_key(key, host),
            _ if self.page == Page::Achievements => self.achievements_key(key, host),
            _ if self.page == Page::Stats => {}
            _ => self.setting_key(key, host),
        }
    }

    fn save(&mut self, host: &mut dyn Host) {
        let Some(path) = &self.config_file else {
            self.error("No configuration file to save to (Rust-DOS started with --no-config)");
            return;
        };
        let shown = contract_home(path, self.home.as_deref());
        match host.save(&self.settings) {
            Ok(()) => self.info(format!("Saved to {}", shown)),
            Err(e) => self.error(e),
        }
    }

    fn setting_key(&mut self, key: UiKey, host: &mut dyn Host) {
        let Some(item) = self.item() else { return };
        // A row of several: Left and Right go to the field beside, Enter
        // lists its values.
        let fields = item.fields(&self.settings);
        if !fields.is_empty() {
            let field = self.field.min(fields.len() - 1);
            match key {
                UiKey::Left | UiKey::Right => self.field = self.field_beside(fields.len(), key),
                UiKey::Enter => {
                    self.field = field;
                    self.open_popup(fields[field]);
                }
                _ => {}
            }
            return;
        }
        match (key, item.input()) {
            (UiKey::Left, Input::Choice | Input::Slider | Input::Presets) => self.step(item, -1, host),
            (UiKey::Right, Input::Choice | Input::Slider | Input::Presets) => self.step(item, 1, host),
            (UiKey::Enter, Input::Choice) => self.open_popup(item),
            (UiKey::Enter, Input::Slider | Input::Presets | Input::Text) => {
                self.edit = Some(TextField::new(&item.text(&self.settings)));
            }
            (UiKey::Enter, Input::File) if item == Item::Mt32Roms => self.open_browser(Pick::Mt32Roms),
            (UiKey::Enter, Input::File) if item == Item::Awe32Rom => self.open_browser(Pick::Awe32Rom),
            (UiKey::Enter, Input::File) if item == Item::VeriteRom => {
                self.open_browser(Pick::VeriteRom)
            }
            (UiKey::Enter, Input::File) if item == Item::Sc55Roms => self.open_browser(Pick::Sc55Roms),
            (UiKey::Enter, Input::File) if item == Item::VrScene => self.open_scenes(),
            (UiKey::Enter, Input::File) => self.open_browser(Pick::SoundFont),
            (UiKey::Enter, Input::Link) if item == Item::Rooms => self.open_rooms(),
            (UiKey::Enter, Input::Link) if item == Item::Phonebook => self.open_phonebook(),
            (UiKey::Enter, Input::Link) if item == Item::Awe32Download => self.download_awe32_rom(),
            (UiKey::Enter, Input::Link) if item == Item::Sc55Download => self.ask_sc55_download(),
            (UiKey::Enter, Input::Link) if item == Item::VoodooOverlay => self.ask_glide_download(),
            (UiKey::Enter, Input::Link) if item == Item::VrCenter => match host.center_vr() {
                Ok(message) => self.info(message),
                Err(e) => self.error(e),
            },
            (UiKey::Enter, Input::Link) if item == Item::VrMarkKeyboard => match host.mark_vr_keyboard() {
                Ok(message) => {
                    // What is marked is what is shown from now on.
                    if self.settings.vr.keyboard != crate::vr::VrKeyboard::Marked {
                        self.settings.vr.keyboard = crate::vr::VrKeyboard::Marked;
                        self.changed(Item::VrKeyboard, host);
                    }
                    self.info(message);
                }
                Err(e) => self.error(e),
            },
            (UiKey::Enter, Input::Link) => self.open_autoexec(host),
            (UiKey::Delete | UiKey::Backspace, _) if item.clear(&mut self.settings) => self.changed(item, host),
            _ => {}
        }
    }

    fn step(&mut self, item: Item, dir: isize, host: &mut dyn Host) {
        item.step(&mut self.settings, dir, &self.drives, self.frontend);
        self.changed(item, host);
    }

    /// The field beside the one chosen that Left or Right goes to, in a
    /// row of `fields`; none past the ends.
    fn field_beside(&self, fields: usize, key: UiKey) -> usize {
        let field = self.field.min(fields - 1);
        if key == UiKey::Left { field.saturating_sub(1) } else { (field + 1).min(fields - 1) }
    }

    /// List `item`'s values over the page, the one it has selected.
    fn open_popup(&mut self, item: Item) {
        let choices = item.choices(&self.settings, &self.drives, self.frontend);
        if choices.is_empty() {
            return;
        }
        let selected = choices.iter().position(|c| *c == self.settings).unwrap_or(0);
        self.popup = Some(Popup { item, choices, selected, scroll: 0, visible: self.visible });
    }

    /// A setting's values listed: Enter picks the one selected, a letter
    /// selects the next one starting with it, and in a row of several,
    /// Left and Right go on to the next field's.
    fn popup_key(&mut self, key: UiKey, host: &mut dyn Host) {
        let home = self.home.as_deref();
        let Some(popup) = &mut self.popup else { return };
        if let Some(i) = Self::navigate(key, popup.selected, popup.choices.len(), popup.visible) {
            popup.selected = i;
            return;
        }
        match key {
            UiKey::Esc => self.popup = None,
            UiKey::Enter => {
                let Some(Popup { item, mut choices, selected, .. }) = self.popup.take() else { return };
                let chosen = choices.swap_remove(selected);
                if chosen != self.settings {
                    self.settings = chosen;
                    self.changed(item, host);
                }
            }
            UiKey::Left | UiKey::Right => {
                let fields = self.item().map_or(Vec::new(), |item| item.fields(&self.settings));
                if !fields.is_empty() {
                    let field = self.field_beside(fields.len(), key);
                    if field != self.field {
                        self.field = field;
                        self.open_popup(fields[field]);
                    }
                }
            }
            UiKey::Char(c) => {
                let c = c.to_lowercase().next().unwrap_or(c);
                let n = popup.choices.len();
                let starts = |i: &usize| popup.item.value(&popup.choices[*i], home).to_lowercase().starts_with(c);
                if let Some(i) = (1..=n).map(|k| (popup.selected + k) % n).find(starts) {
                    popup.selected = i;
                }
            }
            _ => {}
        }
    }

    /// Hand changed settings to the emulator and say when they take effect.
    pub(super) fn changed(&mut self, item: Item, host: &mut dyn Host) {
        let result = host.apply(&self.settings);
        self.status = None;
        match result {
            Err(problem) => self.error(problem),
            Ok(Some(note)) => self.info(note),
            Ok(None) if item.applies() == Applies::NextStart => {
                self.info("Takes effect the next time Rust-DOS starts (F2 saves it)")
            }
            Ok(None) => {}
        }
        // Conflicts between the cards, as the configuration file reports them.
        if self.page == Page::Sound
            && let Some(problem) = self.settings.sound.clone().check().into_iter().next()
        {
            self.error(problem.trim_start_matches("[sound]: ").to_string());
        }
    }

    fn edit_key(&mut self, key: UiKey, host: &mut dyn Host) {
        let item = self.item();
        let (Some(field), Some(item)) = (&mut self.edit, item) else {
            self.edit = None;
            return;
        };
        if field.key(key) {
            return;
        }
        match key {
            UiKey::Esc => self.edit = None,
            UiKey::Enter => {
                let text = field.text();
                let mut settings = self.settings.clone();
                match item.set_text(&mut settings, &text) {
                    Ok(()) => {
                        self.edit = None;
                        if settings != self.settings {
                            self.settings = settings;
                            self.changed(item, host);
                        }
                    }
                    Err(e) => self.error(e),
                }
            }
            _ => {}
        }
    }
}
