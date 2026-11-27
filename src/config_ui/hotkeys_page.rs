//! The settings window's Hotkeys page: the hotkey profile, each action's
//! keys, rebinding one by pressing its new keys, and custom profiles saved
//! under a name of the user's (hotkeys.rs).
//!
//! A rebind takes the next key pressed with its modifiers, which the front
//! end hands over (`hotkey_pressed`) before its hotkeys see it, so Ctrl+F12
//! can be bound too. Keys another action has are given over only once the
//! user says so, and no change leaves the window without a way to open it.

use super::dialog::TextField;
use super::draw::{self, Grid};
use super::item::Item;
use super::{ConfigUi, Hit, Host, Popup, Target, UiKey, fit};
use crate::hotkeys::{self, Action, Combo, CustomProfile, HotkeySettings, Profile};
use std::path::PathBuf;

/// A row of the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HotkeyRow {
    Profile,
    /// Save the keys as a custom profile.
    SaveAs,
    /// Give every action the profile's keys again.
    ResetAll,
    Action(Action),
}

/// An action waiting for its new keys: in place of the ones it has, or
/// (`add`) beside them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Capture {
    pub action: Action,
    pub add: bool,
}

/// Keys pressed for an action that another action has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Clash {
    pub capture: Capture,
    pub combo: Combo,
    pub owner: Action,
}

/// The page's state besides the settings.
#[derive(Debug, Default)]
pub(super) struct HotkeysPage {
    /// Where custom profiles are kept, if anywhere, and those kept there.
    pub dir: Option<PathBuf>,
    pub custom: Vec<CustomProfile>,
    pub capture: Option<Capture>,
    pub clash: Option<Clash>,
    /// A custom profile's name being typed.
    pub naming: Option<TextField>,
    /// The custom profile asked to be deleted.
    pub confirm_delete: Option<String>,
    /// The key a rebind just took without Ctrl or Alt. The front end
    /// sends the text it types after the key itself, and that text isn't
    /// the window's: an R would reset the action just bound, a Y or N
    /// answer the question about a key another action has.
    pub typed: Option<hotkeys::Key>,
    /// The profile last picked (of `hotkey_choices`) and the keys it gave,
    /// which the profile row shows while the keys are those: a built-in
    /// profile may give the keys of a custom one.
    pub picked: Option<(usize, HotkeySettings)>,
}

impl HotkeysPage {
    /// Read the custom profiles again.
    pub fn refresh(&mut self) {
        let custom = self
            .dir
            .as_deref()
            .map(hotkeys::custom_profiles)
            .unwrap_or_default();
        // Another list has other places in it.
        if custom != self.custom {
            self.picked = None;
        }
        self.custom = custom;
    }

    /// Whether something waits for an answer: keys, a name or a yes.
    pub fn busy(&self) -> bool {
        self.capture.is_some()
            || self.clash.is_some()
            || self.naming.is_some()
            || self.confirm_delete.is_some()
    }

    pub fn cancel(&mut self) {
        self.capture = None;
        self.clash = None;
        self.naming = None;
        self.confirm_delete = None;
        self.typed = None;
    }

    /// Whether `text` is what the key a rebind just took typed, which the
    /// window drops. Only the text right after the key can be: a key that
    /// typed nothing (a dead key, a keypad key without Num Lock) lets the
    /// next text through.
    pub fn typed_by_rebind(&mut self, text: &str) -> bool {
        let Some(key) = self.typed.take() else {
            return false;
        };
        let name = key.name();
        name.chars().count() == 1 && text.eq_ignore_ascii_case(name)
    }
}

/// The key that ends a rebind without one.
fn is_escape(combo: Combo) -> bool {
    !combo.ctrl && !combo.alt && !combo.shift && combo.key.name() == "Escape"
}

impl ConfigUi {
    pub(super) fn hotkey_rows(&self) -> Vec<HotkeyRow> {
        let mut rows = vec![HotkeyRow::Profile];
        if self.hotkeys_page.dir.is_some() {
            rows.push(HotkeyRow::SaveAs);
        }
        rows.push(HotkeyRow::ResetAll);
        rows.extend(Action::ALL.map(HotkeyRow::Action));
        rows
    }

    fn hotkey_row(&self) -> Option<HotkeyRow> {
        self.hotkey_rows().get(self.row).copied()
    }

    /// Whether the Hotkeys page waits for the keys of a rebind, which the
    /// front end then hands to `hotkey_pressed` instead of acting on them.
    pub fn capturing_hotkey(&self) -> bool {
        self.open && self.hotkeys_page.capture.is_some()
    }

    /// The keys pressed for a rebind: a key with the modifiers held. Esc
    /// alone ends the rebind without one.
    pub fn hotkey_pressed(&mut self, combo: Combo, host: &mut dyn Host) {
        let Some(capture) = self.hotkeys_page.capture else {
            return;
        };
        self.hotkeys_page.typed = (!combo.has_ctrl_or_alt()).then_some(combo.key);
        if is_escape(combo) {
            self.hotkeys_page.capture = None;
            self.status = None;
            return;
        }
        if capture.action == Action::SettingsWindow && combo.window_uses() {
            return self.error(format!(
                "{} moves or types in this window: the window's key needs Ctrl or Alt",
                combo
            ));
        }
        self.hotkeys_page.capture = None;
        let owner = self.settings.hotkeys.table().owner(combo);
        match owner {
            Some(owner) if owner != capture.action => {
                self.status = None;
                self.hotkeys_page.clash = Some(Clash {
                    capture,
                    combo,
                    owner,
                });
            }
            Some(_) if capture.add => {
                self.info(format!("{} is {}'s already", combo, capture.action.name()))
            }
            _ => self.bind_hotkey(capture, combo, host),
        }
    }

    fn bind_hotkey(&mut self, capture: Capture, combo: Combo, host: &mut dyn Host) {
        let mut new = self.settings.hotkeys.clone();
        new.bind(capture.action, combo, capture.add);
        let done = format!("{} is {} now", capture.action.name(), combo);
        self.apply_hotkeys(new, done, host);
    }

    /// Put `new` in place of the keys, and in the running emulator, unless
    /// they leave no way to open this window, or give it a key it moves or
    /// types with (a custom profile's file edited by hand can). Says
    /// `done`, or what the change makes the keys clash on.
    pub(super) fn apply_hotkeys(&mut self, new: HotkeySettings, done: String, host: &mut dyn Host) {
        if !new.window_reachable(self.settings.mouse_menu_hold) {
            return self
                .error("Nothing would open this window: give settings_window another key first");
        }
        // A key the window had before stays, so that a configuration
        // with one still lets the other keys change.
        let window = Action::SettingsWindow;
        let had: Vec<Combo> = self.settings.hotkeys.table().keys(window).collect();
        if let Some(combo) = new
            .table()
            .keys(window)
            .find(|c| c.window_uses() && !had.contains(c))
        {
            return self.error(format!(
                "{} moves or types in this window: the window's key needs Ctrl or Alt",
                combo
            ));
        }
        if new == self.settings.hotkeys {
            return self.info(done);
        }
        // The warnings the keys had before aren't this change's.
        let before = self.settings.hotkeys.resolve().1;
        self.settings.hotkeys = new;
        self.changed(Item::HotkeyProfile, host);
        if self.status.is_some() {
            return;
        }
        let warnings = self.settings.hotkeys.resolve().1;
        match warnings.into_iter().find(|w| !before.contains(w)) {
            Some(warning) => self.error(warning.trim_start_matches("[hotkeys] ").to_string()),
            None => self.info(done),
        }
    }

    /// The profiles to pick from: the built-in ones, under which the
    /// actions' own keys stay, as `[hotkeys]` has them, then the custom
    /// ones, which replace them. Each with its name and the keys picking
    /// it gives.
    fn hotkey_choices(&self) -> Vec<(String, HotkeySettings)> {
        let mut choices: Vec<(String, HotkeySettings)> = Profile::ALL
            .iter()
            .map(|&profile| {
                let keys = HotkeySettings {
                    profile,
                    ..self.settings.hotkeys.clone()
                };
                (profile.describe().to_string(), keys)
            })
            .collect();
        for custom in &self.hotkeys_page.custom {
            let mut keys = self.settings.hotkeys.clone();
            custom.load_into(&mut keys);
            choices.push((format!("{} (custom)", custom.name), keys));
        }
        choices
    }

    /// The custom profile whose keys these are, if one is.
    fn loaded_custom(&self) -> Option<&CustomProfile> {
        let keys = &self.settings.hotkeys;
        self.hotkeys_page
            .custom
            .iter()
            .find(|c| c.is_loaded_in(keys))
    }

    /// Which of `hotkey_choices` the keys are: a custom profile they match,
    /// else the profile under them.
    fn hotkey_choice(&self) -> usize {
        let count = Profile::ALL.len() + self.hotkeys_page.custom.len();
        if let Some((at, keys)) = &self.hotkeys_page.picked
            && *keys == self.settings.hotkeys
            && *at < count
        {
            return *at;
        }
        let custom = self.loaded_custom().and_then(|loaded| {
            let at = self.hotkeys_page.custom.iter().position(|c| c == loaded)?;
            Some(Profile::ALL.len() + at)
        });
        let profile = self.settings.hotkeys.profile;
        custom.unwrap_or_else(|| Profile::ALL.iter().position(|&p| p == profile).unwrap_or(0))
    }

    /// The custom profile the profile row shows, if it shows one.
    fn shown_custom(&self) -> Option<&CustomProfile> {
        let at = self.hotkey_choice().checked_sub(Profile::ALL.len())?;
        self.hotkeys_page.custom.get(at)
    }

    /// What the profile row shows.
    pub(super) fn hotkey_profile_text(&self) -> String {
        if let Some(custom) = self.shown_custom() {
            return format!("{} (custom)", custom.name);
        }
        let keys = &self.settings.hotkeys;
        match keys.own_count() {
            0 => keys.profile.describe().to_string(),
            1 => format!("{}, 1 key changed", keys.profile.describe()),
            n => format!("{}, {} keys changed", keys.profile.describe(), n),
        }
    }

    /// Pick profile `choice` of `hotkey_choices`.
    pub(super) fn pick_hotkeys(&mut self, choice: usize, host: &mut dyn Host) {
        let Some((name, keys)) = self.hotkey_choices().into_iter().nth(choice) else {
            return;
        };
        let custom = choice >= Profile::ALL.len();
        let done = match keys.own_count() {
            _ if custom => format!("Loaded the keys of {}", name),
            0 => format!("The keys are {}'s", name),
            1 => format!("The keys are {}'s, but for 1 changed one", name),
            n => format!("The keys are {}'s, but for {} changed ones", name, n),
        };
        self.apply_hotkeys(keys.clone(), done, host);
        if self.settings.hotkeys == keys {
            self.hotkeys_page.picked = Some((choice, keys));
        }
    }

    fn open_hotkeys_popup(&mut self) {
        self.hotkeys_page.refresh();
        let choices = self.hotkey_choices();
        let labels = choices.iter().map(|(name, _)| name.clone()).collect();
        let settings = choices
            .into_iter()
            .map(|(_, keys)| crate::config::Settings {
                hotkeys: keys,
                ..self.settings.clone()
            })
            .collect();
        self.popup = Some(Popup {
            item: Item::HotkeyProfile,
            choices: settings,
            labels: Some(labels),
            selected: self.hotkey_choice(),
            scroll: 0,
            visible: self.visible,
        });
    }

    pub(super) fn hotkeys_key(&mut self, key: UiKey, host: &mut dyn Host) {
        let Some(row) = self.hotkey_row() else { return };
        match (row, key) {
            (HotkeyRow::Profile, UiKey::Left | UiKey::Right) => {
                let choices = self.hotkey_choices();
                let count = choices.len();
                let at = self.hotkey_choice();
                let step = |i: usize| {
                    if key == UiKey::Left {
                        (i + count - 1) % count
                    } else {
                        (i + 1) % count
                    }
                };
                // Past the profiles that would leave nothing to open this
                // window with, which `apply_hotkeys` refuses: else the row
                // stops at the first of them. Where every other one
                // would, the next is picked to say why.
                let menu_hold = self.settings.mouse_menu_hold;
                let mut next = step(at);
                while next != at && !choices[next].1.window_reachable(menu_hold) {
                    next = step(next);
                }
                if next == at {
                    next = step(at);
                }
                self.pick_hotkeys(next, host);
            }
            (HotkeyRow::Profile, UiKey::Enter) => self.open_hotkeys_popup(),
            (HotkeyRow::Profile, UiKey::Delete) => match self.shown_custom() {
                Some(custom) => self.hotkeys_page.confirm_delete = Some(custom.name.clone()),
                None => self.error("Only a custom profile can be deleted"),
            },
            (HotkeyRow::SaveAs, UiKey::Enter) => {
                let name = self
                    .shown_custom()
                    .map(|c| c.name.clone())
                    .unwrap_or_default();
                self.hotkeys_page.naming = Some(TextField::new(&name));
                self.status = None;
            }
            (HotkeyRow::ResetAll, UiKey::Enter) => {
                let mut new = self.settings.hotkeys.clone();
                new.reset_all();
                let done = format!("Every key is {}'s", new.profile.describe());
                self.apply_hotkeys(new, done, host);
            }
            (HotkeyRow::Action(action), UiKey::Enter | UiKey::Insert) => {
                let add = key == UiKey::Insert;
                self.hotkeys_page.capture = Some(Capture { action, add });
                self.status = None;
            }
            (HotkeyRow::Action(action), UiKey::Delete | UiKey::Backspace) => {
                let mut new = self.settings.hotkeys.clone();
                new.set_keys(action, Vec::new());
                self.apply_hotkeys(new, format!("{} has no key now", action.name()), host);
            }
            (HotkeyRow::Action(action), UiKey::Char('r' | 'R')) => {
                let mut new = self.settings.hotkeys.clone();
                new.reset(action);
                let done = format!("{} has {}'s keys", action.name(), new.profile.describe());
                self.apply_hotkeys(new, done, host);
            }
            _ => {}
        }
    }

    /// A key while the page waits for an answer. The keys of a rebind
    /// come through `hotkey_pressed`; here only Esc, from a click on its
    /// hint, ends it.
    pub(super) fn hotkeys_busy_key(&mut self, key: UiKey, host: &mut dyn Host) {
        let page = &mut self.hotkeys_page;
        if page.capture.is_some() {
            if key == UiKey::Esc {
                page.capture = None;
                self.status = None;
            }
        } else if let Some(clash) = page.clash {
            match key {
                UiKey::Enter | UiKey::Char('y' | 'Y') => {
                    page.clash = None;
                    self.bind_hotkey(clash.capture, clash.combo, host);
                }
                UiKey::Esc | UiKey::Char('n' | 'N') => {
                    page.clash = None;
                    page.capture = Some(clash.capture);
                }
                _ => {}
            }
        } else if let Some(field) = &mut page.naming {
            if field.key(key) {
                return;
            }
            match key {
                UiKey::Esc => page.naming = None,
                UiKey::Enter => {
                    let name = field.text();
                    self.save_custom_hotkeys(&name);
                }
                _ => {}
            }
        } else if let Some(name) = page.confirm_delete.clone() {
            match key {
                UiKey::Enter | UiKey::Char('y' | 'Y') => {
                    page.confirm_delete = None;
                    let Some(dir) = page.dir.clone() else { return };
                    match hotkeys::delete_custom(&dir, &name) {
                        Ok(()) => {
                            self.hotkeys_page.refresh();
                            self.info(format!(
                                "Deleted the custom profile {}; the keys stay",
                                name
                            ));
                        }
                        Err(e) => self.error(e),
                    }
                }
                UiKey::Esc | UiKey::Char('n' | 'N') => page.confirm_delete = None,
                _ => {}
            }
        }
    }

    /// Save the keys as the custom profile `name`.
    fn save_custom_hotkeys(&mut self, name: &str) {
        let Some(dir) = self.hotkeys_page.dir.clone() else {
            return;
        };
        let name = match hotkeys::custom_name(name) {
            Ok(name) => name,
            Err(e) => return self.error(e),
        };
        let profile = CustomProfile::of(&name, &self.settings.hotkeys);
        let replaced = self
            .hotkeys_page
            .custom
            .iter()
            .any(|c| c.name.eq_ignore_ascii_case(&name));
        match hotkeys::save_custom(&dir, &profile) {
            Ok(()) => {
                self.hotkeys_page.naming = None;
                self.hotkeys_page.refresh();
                self.info(if replaced {
                    format!("Saved the keys over the custom profile {}", name)
                } else {
                    format!("Saved the keys as the custom profile {}", name)
                });
            }
            Err(e) => self.error(e),
        }
    }

    pub(super) fn draw_hotkeys(&mut self, g: &mut Grid, content: std::ops::Range<usize>) {
        let cols = g.cols;
        // A question takes the page.
        if let Some(clash) = self.hotkeys_page.clash {
            let ask = format!(
                "{} is {}'s key. Give it to {}?",
                clash.combo,
                clash.owner.name(),
                clash.capture.action.name()
            );
            g.text_to(
                3,
                content.start + 1,
                &fit(&ask, cols - 6),
                draw::BRIGHT,
                cols - 3,
            );
            let then = format!("{} loses it.", clash.owner.name());
            g.text_to(
                3,
                content.start + 2,
                &fit(&then, cols - 6),
                draw::TEXT,
                cols - 3,
            );
            let keys = "Enter or Y: give it    Esc or N: press other keys";
            g.text_to(3, content.start + 4, keys, draw::DIM, cols - 3);
            return;
        }
        if let Some(name) = &self.hotkeys_page.confirm_delete {
            let ask = format!("Delete the custom profile {}?", name);
            g.text_to(
                3,
                content.start + 1,
                &fit(&ask, cols - 6),
                draw::BRIGHT,
                cols - 3,
            );
            let then = "The keys stay as they are.";
            g.text_to(3, content.start + 2, then, draw::TEXT, cols - 3);
            let keys = "Enter or Y: delete it    Esc or N: keep it";
            g.text_to(3, content.start + 4, keys, draw::DIM, cols - 3);
            return;
        }
        let rows = self.hotkey_rows();
        Self::keep_visible(&mut self.scroll, self.row, content.len());
        let value_col = 27.min(cols / 2);
        let note_col = cols - 13;
        let end = note_col.saturating_sub(1);
        let table = self.settings.hotkeys.table();
        let mut anchor = None;
        for (i, row) in (self.scroll..rows.len()).zip(content.clone()) {
            let selected = i == self.row;
            if selected {
                self.select_row(g, row);
            }
            self.hits.push(Hit {
                row,
                col: 1,
                width: cols - 2,
                target: Target::Row(i),
            });
            let label_color = if selected { draw::BRIGHT } else { draw::TEXT };
            match rows[i] {
                HotkeyRow::Profile => {
                    g.text_to(2, row, "Profile", label_color, value_col - 1);
                    if selected {
                        anchor = Some((value_col, row));
                    }
                    let value = fit(
                        &self.hotkey_profile_text(),
                        end.saturating_sub(value_col + 4),
                    );
                    g.char(value_col, row, 0x11, draw::KEY);
                    self.hits.push(Hit {
                        row,
                        col: value_col,
                        width: 1,
                        target: Target::Step(i, -1),
                    });
                    let after = g.text_to(value_col + 2, row, &value, draw::BRIGHT, end);
                    let width = after - value_col - 2;
                    self.hits.push(Hit {
                        row,
                        col: value_col + 2,
                        width,
                        target: Target::Button(i),
                    });
                    g.char(after + 1, row, 0x10, draw::KEY);
                    self.hits.push(Hit {
                        row,
                        col: after + 1,
                        width: 1,
                        target: Target::Step(i, 1),
                    });
                }
                HotkeyRow::SaveAs => {
                    // The name takes the value's column and the note's,
                    // which this row has no use for, and the label gives
                    // way to it: past the label, a 40-column window has
                    // no room left for a name.
                    let naming = self.hotkeys_page.naming.as_ref();
                    let label_end = if naming.is_some() {
                        value_col - 1
                    } else {
                        value_col + 6
                    };
                    let label = "Save as a custom profile...";
                    g.text_to(2, row, label, draw::KEY, label_end);
                    if let Some(field) = naming {
                        let col = value_col;
                        let field_end = cols - 3;
                        let width = field_end.saturating_sub(col);
                        let (text, cursor) = field.view(width);
                        g.background(col, row, width, draw::FIELD);
                        g.text_to(col, row, &text, draw::BRIGHT, field_end);
                        g.background(col + cursor, row, 1, draw::SELECT);
                    }
                }
                HotkeyRow::ResetAll => {
                    g.text_to(
                        2,
                        row,
                        "Reset every key to the profile's",
                        draw::KEY,
                        note_col - 1,
                    );
                }
                HotkeyRow::Action(action) => {
                    g.text_to(2, row, action.describe(), label_color, value_col - 1);
                    let capture = self.hotkeys_page.capture.filter(|c| c.action == action);
                    if let Some(capture) = capture {
                        let ask = if capture.add {
                            "Press the key to add (Esc: cancel)"
                        } else {
                            "Press the new key (Esc: cancel)"
                        };
                        let width = end.saturating_sub(value_col);
                        g.background(value_col, row, width, draw::FIELD);
                        g.text_to(value_col, row, &fit(ask, width), draw::KEY, end);
                        continue;
                    }
                    let keys: Vec<Combo> = table.keys(action).collect();
                    let (text, color) = if keys.is_empty() {
                        ("none".to_string(), draw::DIM)
                    } else {
                        (hotkeys::keys_text(&keys), draw::BRIGHT)
                    };
                    let text = fit(&text, end.saturating_sub(value_col));
                    let after = g.text_to(value_col, row, &text, color, end);
                    let width = after - value_col;
                    self.hits.push(Hit {
                        row,
                        col: value_col,
                        width,
                        target: Target::Button(i),
                    });
                    if self.settings.hotkeys.is_own(action) {
                        g.text(note_col + 1, row, "changed", draw::NOTE);
                    }
                }
            }
        }
        self.draw_scrollbar(g, content.clone(), self.scroll, rows.len());
        if let Some(anchor) = anchor {
            self.draw_popup(g, content, anchor);
        }
    }

    /// The key hints of the Hotkeys page.
    pub(super) fn hotkeys_hints(&self) -> Vec<(&'static str, &'static str, UiKey)> {
        use UiKey::*;
        let page = &self.hotkeys_page;
        if page.capture.is_some() {
            return vec![("Esc", "Cancel", Esc)];
        }
        if page.clash.is_some() {
            return vec![("Enter", "Give it", Enter), ("Esc", "Other keys", Esc)];
        }
        if page.naming.is_some() {
            return vec![("Enter", "Save", Enter), ("Esc", "Cancel", Esc)];
        }
        if page.confirm_delete.is_some() {
            return vec![("Enter", "Delete", Enter), ("Esc", "Keep", Esc)];
        }
        let mut hints = match self.hotkey_row() {
            Some(HotkeyRow::Profile) => {
                let mut hints = vec![
                    ("\u{2190}\u{2192}", "Change", Right),
                    ("Enter", "List", Enter),
                ];
                if self.shown_custom().is_some() {
                    hints.push(("Del", "Delete", Delete));
                }
                hints
            }
            Some(HotkeyRow::SaveAs) => vec![("Enter", "Name it", Enter)],
            Some(HotkeyRow::ResetAll) => vec![("Enter", "Reset", Enter)],
            Some(HotkeyRow::Action(_)) => vec![
                ("Enter", "Rebind", Enter),
                ("Ins", "Add", Insert),
                ("Del", "Clear", Delete),
                ("R", "Reset", Char('r')),
            ],
            None => Vec::new(),
        };
        hints.extend([
            ("Tab", "Page", Tab),
            ("F2", "Save", Save),
            ("Esc", "Close", Esc),
        ]);
        hints
    }
}
