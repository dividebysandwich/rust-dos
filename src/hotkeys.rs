//! The emulator's hotkeys: the keys that save and load states, open the
//! settings window, pause, run fast and the rest, which the rust-dos
//! program takes before the machine sees them.
//!
//! Each hotkey is an `Action`, which the `[hotkeys]` section of the
//! configuration binds to keys. Its `profile` setting picks a set of
//! bindings (`Profile`): rust-dos's own, the default, or the keys of
//! another emulator. A line named after an action binds that action alone,
//! over the profile. `HotkeySettings::resolve` puts the two together into
//! the table the program looks keys up in (`Hotkeys`), and says which
//! bindings clash.
//!
//! The keys are named here, without SDL, so the configuration can be read
//! and checked by the whole crate; the program maps SDL's key names to
//! them (`Key::named`).

use std::fmt;

/// What a hotkey does. The names are the `[hotkeys]` settings'.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    SettingsWindow,
    SaveStatesPage,
    PerformanceOverlay,
    SoundVisualizer,
    Manuals,
    SaveState,
    LoadState,
    NextSlot,
    PreviousSlot,
    Pause,
    FastForward,
    Rewind,
    Slower,
    Faster,
    Fullscreen,
    MouseCapture,
    Mute,
    Screenshot,
    RecordSound,
    RecordVideo,
    RecordAnimation,
    SwapDisk,
    CopyText,
    PasteText,
    PrinterEject,
    Degauss,
    RecenterView,
}

impl Action {
    /// Every action, in the order the configuration and the documentation
    /// list them.
    pub const ALL: [Action; 27] = [
        Action::SettingsWindow,
        Action::SaveStatesPage,
        Action::PerformanceOverlay,
        Action::SoundVisualizer,
        Action::Manuals,
        Action::SaveState,
        Action::LoadState,
        Action::NextSlot,
        Action::PreviousSlot,
        Action::Pause,
        Action::FastForward,
        Action::Rewind,
        Action::Slower,
        Action::Faster,
        Action::Fullscreen,
        Action::MouseCapture,
        Action::Mute,
        Action::Screenshot,
        Action::RecordSound,
        Action::RecordVideo,
        Action::RecordAnimation,
        Action::SwapDisk,
        Action::CopyText,
        Action::PasteText,
        Action::PrinterEject,
        Action::Degauss,
        Action::RecenterView,
    ];

    /// The action's setting in `[hotkeys]`.
    pub fn name(self) -> &'static str {
        match self {
            Action::SettingsWindow => "settings_window",
            Action::SaveStatesPage => "save_states_page",
            Action::PerformanceOverlay => "performance_overlay",
            Action::SoundVisualizer => "sound_visualizer",
            Action::Manuals => "manuals",
            Action::SaveState => "save_state",
            Action::LoadState => "load_state",
            Action::NextSlot => "next_slot",
            Action::PreviousSlot => "previous_slot",
            Action::Pause => "pause",
            Action::FastForward => "fast_forward",
            Action::Rewind => "rewind",
            Action::Slower => "slower",
            Action::Faster => "faster",
            Action::Fullscreen => "fullscreen",
            Action::MouseCapture => "mouse_capture",
            Action::Mute => "mute",
            Action::Screenshot => "screenshot",
            Action::RecordSound => "record_sound",
            Action::RecordVideo => "record_video",
            Action::RecordAnimation => "record_animation",
            Action::SwapDisk => "swap_disk",
            Action::CopyText => "copy_text",
            Action::PasteText => "paste_text",
            Action::PrinterEject => "printer_eject",
            Action::Degauss => "degauss",
            Action::RecenterView => "recenter_view",
        }
    }

    pub fn parse(name: &str) -> Option<Action> {
        let name = name.trim();
        Action::ALL
            .into_iter()
            .find(|a| a.name().eq_ignore_ascii_case(name))
    }

    fn index(self) -> usize {
        Action::ALL.iter().position(|&a| a == self).unwrap_or(0)
    }

    /// Whether the action lasts while its key is held, and ends as the key
    /// comes up.
    pub fn held(self) -> bool {
        matches!(self, Action::FastForward | Action::Rewind)
    }

    /// Whether the action does something while the settings window is
    /// open. A key without Ctrl or Alt bound to one that doesn't goes to
    /// the window instead, which has its own F1, F2 and so on.
    pub fn in_settings_window(self) -> bool {
        !matches!(
            self,
            Action::SaveState
                | Action::LoadState
                | Action::NextSlot
                | Action::PreviousSlot
                | Action::Slower
                | Action::Faster
                | Action::Rewind
                | Action::Fullscreen
                | Action::MouseCapture
                | Action::CopyText
                | Action::RecordAnimation
                | Action::RecenterView
        )
    }
}

/// The keys a hotkey can be on, by their names in the configuration:
/// the first name of each is the one written back. The others are
/// SDL's names of the keys and common short forms.
const KEYS: &[&[&str]] = &[
    &["A"],
    &["B"],
    &["C"],
    &["D"],
    &["E"],
    &["F"],
    &["G"],
    &["H"],
    &["I"],
    &["J"],
    &["K"],
    &["L"],
    &["M"],
    &["N"],
    &["O"],
    &["P"],
    &["Q"],
    &["R"],
    &["S"],
    &["T"],
    &["U"],
    &["V"],
    &["W"],
    &["X"],
    &["Y"],
    &["Z"],
    &["0"],
    &["1"],
    &["2"],
    &["3"],
    &["4"],
    &["5"],
    &["6"],
    &["7"],
    &["8"],
    &["9"],
    &["F1"],
    &["F2"],
    &["F3"],
    &["F4"],
    &["F5"],
    &["F6"],
    &["F7"],
    &["F8"],
    &["F9"],
    &["F10"],
    &["F11"],
    &["F12"],
    &["F13"],
    &["F14"],
    &["F15"],
    &["F16"],
    &["F17"],
    &["F18"],
    &["F19"],
    &["F20"],
    &["F21"],
    &["F22"],
    &["F23"],
    &["F24"],
    &["Escape", "Esc"],
    &["Enter", "Return"],
    &["Tab"],
    &["Space"],
    &["Backspace"],
    &["Insert", "Ins"],
    &["Delete", "Del"],
    &["Home"],
    &["End"],
    &["PageUp", "PgUp"],
    &["PageDown", "PgDn"],
    &["Up"],
    &["Down"],
    &["Left"],
    &["Right"],
    &["Pause", "Break"],
    &["PrintScreen", "PrtSc", "SysRq"],
    &["ScrollLock"],
    &["Minus", "-"],
    &["Equals", "="],
    &["LeftBracket", "["],
    &["RightBracket", "]"],
    &["Semicolon", ";"],
    &["Apostrophe", "'"],
    &["Grave", "Backquote", "`"],
    &["Comma", ","],
    &["Period", "."],
    &["Slash", "/"],
    &["Backslash", "\\"],
    &["Keypad0", "Keypad 0"],
    &["Keypad1", "Keypad 1"],
    &["Keypad2", "Keypad 2"],
    &["Keypad3", "Keypad 3"],
    &["Keypad4", "Keypad 4"],
    &["Keypad5", "Keypad 5"],
    &["Keypad6", "Keypad 6"],
    &["Keypad7", "Keypad 7"],
    &["Keypad8", "Keypad 8"],
    &["Keypad9", "Keypad 9"],
    &["KeypadPlus", "Keypad +"],
    &["KeypadMinus", "Keypad -"],
    &["KeypadMultiply", "Keypad *"],
    &["KeypadDivide", "Keypad /"],
    &["KeypadEnter", "Keypad Enter"],
    &["KeypadPeriod", "Keypad ."],
];

/// A key, without its modifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key(u8);

impl Key {
    /// The key called `name`, in any case: a name of the configuration's
    /// or SDL's.
    pub fn named(name: &str) -> Option<Key> {
        let name = name.trim();
        KEYS.iter()
            .position(|names| names.iter().any(|n| n.eq_ignore_ascii_case(name)))
            .map(|i| Key(i as u8))
    }

    pub fn name(self) -> &'static str {
        KEYS[self.0 as usize][0]
    }

    /// Whether the key types a character or edits a line: bound without
    /// Ctrl or Alt, the program can't have it for typing.
    fn types(self) -> bool {
        let name = self.name();
        name.len() == 1
            || name.starts_with("Keypad")
            || matches!(name, "Space" | "Enter" | "Tab" | "Backspace")
            || KEYS[self.0 as usize].iter().any(|n| n.len() == 1)
    }
}

/// A key with the modifiers held with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Combo {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: Key,
}

impl Combo {
    /// A combination as written: modifiers and a key joined by '+', such
    /// as `Ctrl+Shift+F3`, in any case.
    pub fn parse(text: &str) -> Result<Combo, String> {
        let text = text.trim();
        let parts: Vec<&str> = text.split('+').map(str::trim).collect();
        let (key, modifiers) = parts
            .split_last()
            .ok_or_else(|| format!("'{}' names no key", text))?;
        let mut combo = Combo {
            ctrl: false,
            alt: false,
            shift: false,
            key: Key::named(key).ok_or_else(|| format!("unknown key '{}' in '{}'", key, text))?,
        };
        for modifier in modifiers {
            let flag = match modifier.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => &mut combo.ctrl,
                "alt" => &mut combo.alt,
                "shift" => &mut combo.shift,
                _ => {
                    return Err(format!(
                        "unknown modifier '{}' in '{}' (Ctrl, Alt or Shift)",
                        modifier, text
                    ));
                }
            };
            *flag = true;
        }
        Ok(combo)
    }

    /// Whether the combination has Ctrl or Alt, which keeps it from the
    /// keys a program is typed.
    pub fn has_ctrl_or_alt(self) -> bool {
        self.ctrl || self.alt
    }
}

impl fmt::Display for Combo {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.ctrl {
            f.write_str("Ctrl+")?;
        }
        if self.alt {
            f.write_str("Alt+")?;
        }
        if self.shift {
            f.write_str("Shift+")?;
        }
        f.write_str(self.key.name())
    }
}

/// An action's keys as written: combinations separated by commas, or
/// `none` for no key.
pub fn parse_keys(text: &str) -> Result<Vec<Combo>, String> {
    let text = text.trim();
    if text.eq_ignore_ascii_case("none") || text.is_empty() {
        return Ok(Vec::new());
    }
    text.split(',').map(Combo::parse).collect()
}

/// `keys` as `parse_keys` reads them.
pub fn keys_text(keys: &[Combo]) -> String {
    if keys.is_empty() {
        return "none".to_string();
    }
    keys.iter()
        .map(Combo::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// A set of bindings for every action, as another emulator has its keys.
///
/// A profile of another emulator takes that emulator's default key for
/// each action rust-dos has too, and rust-dos's own keys for the rest.
/// Where the two clash, the other emulator's key wins, and the rust-dos
/// action it took is moved or left without a key, as its table says. Keys
/// that type in DOS (Space, Tab, Backspace, a letter without Ctrl or Alt)
/// are left out even where the emulator has them, as a DOS program needs
/// them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Profile {
    /// rust-dos's own keys, which follow DOSBox's where it has the action.
    #[default]
    RustDos,
    Pcsx2,
    DuckStation,
    PcsxRedux,
    Epsxe,
    RetroArch,
    DosBox,
    Box86,
}

impl Profile {
    pub const ALL: [Profile; 8] = [
        Profile::RustDos,
        Profile::Pcsx2,
        Profile::DuckStation,
        Profile::PcsxRedux,
        Profile::Epsxe,
        Profile::RetroArch,
        Profile::DosBox,
        Profile::Box86,
    ];

    /// The profile's name, as `profile` takes it.
    pub fn name(self) -> &'static str {
        match self {
            Profile::RustDos => "rust-dos",
            Profile::Pcsx2 => "pcsx2",
            Profile::DuckStation => "duckstation",
            Profile::PcsxRedux => "pcsx-redux",
            Profile::Epsxe => "epsxe",
            Profile::RetroArch => "retroarch",
            Profile::DosBox => "dosbox",
            Profile::Box86 => "86box",
        }
    }

    /// The name the settings window shows.
    pub fn describe(self) -> &'static str {
        match self {
            Profile::RustDos => "Rust-DOS",
            Profile::Pcsx2 => "PCSX2",
            Profile::DuckStation => "DuckStation",
            Profile::PcsxRedux => "PCSX-Redux",
            Profile::Epsxe => "ePSXe",
            Profile::RetroArch => "RetroArch",
            Profile::DosBox => "DOSBox",
            Profile::Box86 => "86Box",
        }
    }

    pub fn parse(name: &str) -> Option<Profile> {
        let name = name.trim();
        Profile::ALL
            .into_iter()
            .find(|p| p.name().eq_ignore_ascii_case(name))
    }

    /// The keys that differ from rust-dos's, as text, which a test
    /// checks: `none` for an action without a key.
    fn changes(self) -> &'static [(Action, &'static str)] {
        match self {
            Profile::RustDos => &[],
            Profile::Pcsx2 => PCSX2,
            Profile::DuckStation => DUCKSTATION,
            Profile::PcsxRedux => PCSX_REDUX,
            Profile::Epsxe => EPSXE,
            Profile::RetroArch => RETROARCH,
            Profile::DosBox => DOSBOX,
            Profile::Box86 => BOX86,
        }
    }

    /// The keys of `action` in this profile, as text.
    fn text(self, action: Action) -> &'static str {
        let find = |table: &[(Action, &'static str)]| {
            table.iter().find(|(a, _)| *a == action).map(|&(_, t)| t)
        };
        find(self.changes())
            .or_else(|| find(RUST_DOS))
            .unwrap_or("none")
    }

    /// The keys of `action` in this profile.
    pub fn keys(self, action: Action) -> Vec<Combo> {
        parse_keys(self.text(action)).unwrap_or_default()
    }
}

/// rust-dos's own keys: Ctrl with the function keys for most, as DOSBox
/// has them (Ctrl+F4, Ctrl+F5, Ctrl+F6, Ctrl+F7, Ctrl+F8, Ctrl+F10,
/// Ctrl+F11, Alt+F12, Alt+Enter, Alt+Pause), and Ctrl+Shift with a letter
/// for the rest.
const RUST_DOS: &[(Action, &str)] = &[
    (Action::SettingsWindow, "Ctrl+F12"),
    (Action::SaveStatesPage, "Ctrl+F9"),
    (Action::PerformanceOverlay, "Ctrl+Shift+F12"),
    (Action::SoundVisualizer, "Ctrl+Shift+S"),
    (Action::Manuals, "Ctrl+Shift+M"),
    (Action::SaveState, "Ctrl+F1"),
    (Action::LoadState, "Ctrl+F2"),
    (Action::NextSlot, "Ctrl+F3"),
    (Action::PreviousSlot, "Ctrl+Shift+F3"),
    (Action::Pause, "Alt+Pause"),
    (Action::FastForward, "Alt+F12"),
    (Action::Rewind, "Alt+F11"),
    (Action::Slower, "Ctrl+F11"),
    (Action::Faster, "Ctrl+Shift+F11"),
    (Action::Fullscreen, "Alt+Enter"),
    (Action::MouseCapture, "Ctrl+F10"),
    (Action::Mute, "Ctrl+F8"),
    (Action::Screenshot, "Ctrl+F5"),
    (Action::RecordSound, "Ctrl+F6"),
    (Action::RecordVideo, "Ctrl+F7"),
    (Action::RecordAnimation, "PrintScreen"),
    (Action::SwapDisk, "Ctrl+F4"),
    (Action::CopyText, "Ctrl+Shift+C"),
    (Action::PasteText, "Ctrl+Shift+V"),
    (Action::PrinterEject, "Ctrl+Shift+F5"),
    (Action::Degauss, "Ctrl+Shift+D"),
    (Action::RecenterView, "Ctrl+Shift+Home"),
];

/// PCSX2's defaults, from `Pad::SetDefaultHotkeyConfig` in
/// pcsx2/SIO/Pad/Pad.cpp (commit 4e56d430, lines 219-270). Left out:
/// Space (pause), Tab and Period (turbo), which type in DOS.
const PCSX2: &[(Action, &str)] = &[
    (Action::SaveState, "F1"),
    (Action::LoadState, "F3"),
    (Action::NextSlot, "F2"),
    (Action::PreviousSlot, "Shift+F2"),
    (Action::Screenshot, "F8"),
    (Action::Mute, "Ctrl+M"),
];

/// DuckStation's defaults, from `Settings::SetDefaultHotkeyConfig` in
/// src/core/hotkeys.cpp (commit 2c79bd36, lines 38-52). Left out: Space
/// (pause) and Tab (fast forward), which type in DOS.
const DUCKSTATION: &[(Action, &str)] = &[
    (Action::LoadState, "F1"),
    (Action::SaveState, "F2"),
    (Action::PreviousSlot, "F3"),
    (Action::NextSlot, "F4"),
    (Action::Screenshot, "F10"),
    (Action::Fullscreen, "F11"),
];

/// PCSX-Redux's keys, which src/gui/gui.cc checks (commit dfb13d82, lines
/// 1134-1194): it has one quick-save slot, so the slot keys stay
/// rust-dos's. Left out: Backspace (rewind) and Tab (turbo), which type
/// in DOS.
const PCSX_REDUX: &[(Action, &str)] = &[
    (Action::SaveState, "F1"),
    (Action::LoadState, "F2"),
    (Action::Pause, "Pause"),
    (Action::Screenshot, "F12"),
];

/// ePSXe's, from its FAQ (www.epsxe.com/files/ePSXe_FAQ.txt): F1 saves,
/// F2 picks the next slot and F3 loads.
const EPSXE: &[(Action, &str)] = &[
    (Action::SaveState, "F1"),
    (Action::NextSlot, "F2"),
    (Action::LoadState, "F3"),
];

/// RetroArch's keyboard defaults, from config.def.keybinds.h (commit
/// d01d896e, the desktop block from line 1039). F1, its menu, opens the
/// settings window, as does Ctrl+F12. Left out: Space and L (fast
/// forward), R (rewind), P (pause) and F (fullscreen), which type in DOS.
const RETROARCH: &[(Action, &str)] = &[
    (Action::SettingsWindow, "F1, Ctrl+F12"),
    (Action::SaveState, "F2"),
    (Action::LoadState, "F4"),
    (Action::PreviousSlot, "F6"),
    (Action::NextSlot, "F7"),
    (Action::Screenshot, "F8"),
    (Action::Mute, "F9"),
    (Action::MouseCapture, "F11"),
];

/// DOSBox's keys, from the 0.74 README's "Special Keys" and DOSBox
/// Staging's `MAPPER_AddHandler` calls (commit edde12b8; its shortcuts
/// page agrees). rust-dos has most already; DOSBox's Ctrl+F12 speeds the
/// CPU up, so Ctrl+F1, its keymapper, opens the settings window, and the
/// save state, which DOSBox has no key for, moves to Ctrl+Shift+F1. Ctrl+F7
/// records video, as in DOSBox Staging, and Ctrl+Alt+F5 too, as in 0.74.
const DOSBOX: &[(Action, &str)] = &[
    (Action::SettingsWindow, "Ctrl+F1"),
    (Action::SaveState, "Ctrl+Shift+F1"),
    (Action::Faster, "Ctrl+F12"),
    (Action::RecordVideo, "Ctrl+F7, Ctrl+Alt+F5"),
];

/// 86Box's, from `def_acc_keys` in src/86box.c of v6.0. Its fast forward
/// is a switch, which rust-dos's isn't, so it stays rust-dos's. Its
/// screenshot key is rust-dos's Ctrl+F11, which leaves the speed keys
/// without one.
const BOX86: &[(Action, &str)] = &[
    (Action::Fullscreen, "Ctrl+Alt+PageUp"),
    (Action::Screenshot, "Ctrl+F11"),
    (Action::MouseCapture, "Ctrl+End"),
    (Action::Pause, "Ctrl+Alt+F1"),
    (Action::Mute, "Ctrl+Alt+M"),
    (Action::Slower, "none"),
    (Action::Faster, "none"),
];

/// The `[hotkeys]` settings: the profile, and the actions bound over it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HotkeySettings {
    pub profile: Profile,
    /// Each action's own keys, in the order of `Action::ALL`, or None for
    /// the profile's.
    pub keys: [Option<Vec<Combo>>; Action::ALL.len()],
}

impl Default for HotkeySettings {
    fn default() -> Self {
        HotkeySettings {
            profile: Profile::default(),
            keys: std::array::from_fn(|_| None),
        }
    }
}

impl HotkeySettings {
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        if key.trim().eq_ignore_ascii_case("profile") {
            let names: Vec<&str> = Profile::ALL.iter().map(|p| p.name()).collect();
            self.profile = Profile::parse(value).ok_or_else(|| {
                format!("invalid profile '{}' ({})", value.trim(), names.join(", "))
            })?;
            return Ok(());
        }
        let action =
            Action::parse(key).ok_or_else(|| format!("unknown setting '{}'", key.trim()))?;
        let keys = parse_keys(value).map_err(|e| format!("invalid {}: {}", action.name(), e))?;
        self.keys[action.index()] = Some(keys);
        Ok(())
    }

    /// The settings as lines of `[hotkeys]`: an action that follows the
    /// profile has none.
    pub fn entries(&self) -> Vec<(&'static str, Option<String>)> {
        let mut entries = vec![("profile", Some(self.profile.name().to_string()))];
        entries.extend(
            Action::ALL
                .iter()
                .map(|&a| (a.name(), self.keys[a.index()].as_deref().map(keys_text))),
        );
        entries
    }

    /// The keys of every action, the profile's under the actions' own,
    /// and what clashes. An action's own key that another action's own
    /// line has already is left out; a key of the profile's that an
    /// action's own line takes goes to that action. Each is said in a
    /// warning, as is a key without Ctrl or Alt that types.
    pub fn resolve(&self) -> (Hotkeys, Vec<String>) {
        let mut table: Vec<(Combo, Action)> = Vec::new();
        let mut warnings = Vec::new();
        let profile = self.profile.name();
        for action in Action::ALL {
            let Some(keys) = &self.keys[action.index()] else {
                continue;
            };
            for &combo in keys {
                match table.iter().find(|(c, _)| *c == combo) {
                    Some((_, other)) if *other != action => warnings.push(format!(
                        "[hotkeys] {} and {} are both {}: {} keeps it",
                        other.name(),
                        action.name(),
                        combo,
                        other.name()
                    )),
                    Some(_) => {}
                    None => table.push((combo, action)),
                }
                if !combo.has_ctrl_or_alt() && combo.key.types() {
                    warnings.push(format!(
                        "[hotkeys] {} is {}, which programs can't type while it is a hotkey",
                        action.name(),
                        combo
                    ));
                }
            }
        }
        for action in Action::ALL {
            if self.keys[action.index()].is_some() {
                continue;
            }
            for combo in self.profile.keys(action) {
                match table.iter().find(|(c, _)| *c == combo) {
                    Some((_, other)) => warnings.push(format!(
                        "[hotkeys] {} takes {} from {} of the {} profile",
                        other.name(),
                        combo,
                        action.name(),
                        profile
                    )),
                    None => table.push((combo, action)),
                }
            }
        }
        (Hotkeys { table }, warnings)
    }

    /// The table of the keys, without the warnings.
    pub fn table(&self) -> Hotkeys {
        self.resolve().0
    }
}

/// The keys of every action, for looking up the key pressed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hotkeys {
    table: Vec<(Combo, Action)>,
}

impl Hotkeys {
    /// The action of `key` pressed with these modifiers. Held with Shift
    /// too, a key that has no action of its own with Shift has the action
    /// it has without Shift, if that one has Ctrl or Alt: so Ctrl+Shift+F1
    /// saves the state as Ctrl+F1 does, while Shift+F1 isn't F1.
    pub fn action(&self, ctrl: bool, alt: bool, shift: bool, key: Key) -> Option<Action> {
        let find = |shift| {
            let combo = Combo {
                ctrl,
                alt,
                shift,
                key,
            };
            self.table
                .iter()
                .find(|(c, _)| *c == combo)
                .map(|&(_, a)| a)
        };
        find(shift).or_else(|| {
            if shift && (ctrl || alt) {
                find(false)
            } else {
                None
            }
        })
    }

    /// The keys of `action`, in the order they were bound.
    pub fn keys(&self, action: Action) -> impl Iterator<Item = Combo> + '_ {
        self.table
            .iter()
            .filter(move |(_, a)| *a == action)
            .map(|&(c, _)| c)
    }

    /// The first key of `action` as text, for messages, or None if it has
    /// no key.
    pub fn key_text(&self, action: Action) -> Option<String> {
        self.keys(action).next().map(|c| c.to_string())
    }

    /// Whether `key` is one of `action`'s, whatever the modifiers: the key
    /// that ends a held action as it comes up.
    pub fn is_key_of(&self, action: Action, key: Key) -> bool {
        self.keys(action).any(|c| c.key == key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn combo(text: &str) -> Combo {
        Combo::parse(text).unwrap()
    }

    #[test]
    fn combinations_read_what_they_write() {
        let c = combo("shift+ctrl+f3");
        assert_eq!(
            c,
            Combo {
                ctrl: true,
                alt: false,
                shift: true,
                key: Key::named("F3").unwrap()
            }
        );
        assert_eq!(c.to_string(), "Ctrl+Shift+F3");
        assert_eq!(combo("Alt + Return").to_string(), "Alt+Enter");
        assert_eq!(combo("Ctrl+`").to_string(), "Ctrl+Grave");
        assert_eq!(Key::named("Keypad +").map(Key::name), Some("KeypadPlus"));
        assert!(Combo::parse("Ctrl+Hyper+F1").is_err());
        assert!(Combo::parse("Ctrl+F99").is_err());
        assert!(Combo::parse("Ctrl+").is_err());
        assert_eq!(parse_keys("none"), Ok(Vec::new()));
        let two = parse_keys("F1, Ctrl+F1").unwrap();
        assert_eq!(keys_text(&two), "F1, Ctrl+F1");
        assert_eq!(parse_keys(&keys_text(&two)), Ok(two));
    }

    /// The keys rust-dos had before they could be changed, each with the
    /// modifiers its check in the event loop asked for.
    #[test]
    fn the_default_profile_has_the_keys_rust_dos_always_had() {
        let table = HotkeySettings::default().table();
        let expected = [
            ("Ctrl+F12", Action::SettingsWindow),
            ("Ctrl+F9", Action::SaveStatesPage),
            ("Ctrl+Shift+F12", Action::PerformanceOverlay),
            ("Ctrl+Shift+S", Action::SoundVisualizer),
            ("Ctrl+Shift+M", Action::Manuals),
            ("Ctrl+F1", Action::SaveState),
            ("Ctrl+F2", Action::LoadState),
            ("Ctrl+F3", Action::NextSlot),
            ("Ctrl+Shift+F3", Action::PreviousSlot),
            ("Alt+Pause", Action::Pause),
            ("Alt+F12", Action::FastForward),
            ("Alt+F11", Action::Rewind),
            ("Ctrl+F11", Action::Slower),
            ("Ctrl+Shift+F11", Action::Faster),
            ("Alt+Enter", Action::Fullscreen),
            ("Ctrl+F10", Action::MouseCapture),
            ("Ctrl+F8", Action::Mute),
            ("Ctrl+F5", Action::Screenshot),
            ("Ctrl+F6", Action::RecordSound),
            ("Ctrl+F7", Action::RecordVideo),
            ("PrintScreen", Action::RecordAnimation),
            ("Ctrl+F4", Action::SwapDisk),
            ("Ctrl+Shift+C", Action::CopyText),
            ("Ctrl+Shift+V", Action::PasteText),
            ("Ctrl+Shift+F5", Action::PrinterEject),
            ("Ctrl+Shift+D", Action::Degauss),
            ("Ctrl+Shift+Home", Action::RecenterView),
        ];
        for (text, action) in expected {
            let c = combo(text);
            assert_eq!(
                table.action(c.ctrl, c.alt, c.shift, c.key),
                Some(action),
                "{}",
                text
            );
            assert_eq!(
                table.keys(action).collect::<Vec<_>>(),
                vec![c],
                "{}",
                action.name()
            );
        }
        // Shift held too, as the checks didn't look at it: Ctrl+Shift+F1
        // saved, and Ctrl+Alt+F1, which AltGr can be, didn't.
        let f1 = Key::named("F1").unwrap();
        assert_eq!(table.action(true, false, true, f1), Some(Action::SaveState));
        assert_eq!(table.action(true, true, false, f1), None);
        assert_eq!(table.action(false, false, false, f1), None);
        let enter = Key::named("Enter").unwrap();
        assert_eq!(
            table.action(false, true, true, enter),
            Some(Action::Fullscreen)
        );
    }

    #[test]
    fn every_profile_parses_and_has_no_clash() {
        for profile in Profile::ALL {
            for (action, text) in RUST_DOS.iter().chain(profile.changes()) {
                assert!(
                    parse_keys(text).is_ok(),
                    "{} {}: {}",
                    profile.name(),
                    action.name(),
                    text
                );
            }
            let settings = HotkeySettings {
                profile,
                ..Default::default()
            };
            let (table, warnings) = settings.resolve();
            assert_eq!(warnings, Vec::<String>::new(), "{}", profile.name());
            let mut seen = Vec::new();
            for (combo, _) in &table.table {
                assert!(!seen.contains(combo), "{}: {} twice", profile.name(), combo);
                seen.push(*combo);
            }
            // Every profile keeps a way to the settings window.
            assert!(
                table.key_text(Action::SettingsWindow).is_some(),
                "{}",
                profile.name()
            );
            assert_eq!(Profile::parse(profile.name()), Some(profile));
        }
    }

    /// REFERENCE.md's table of the profiles has the keys they have.
    #[test]
    fn the_reference_lists_every_profiles_keys() {
        let text = include_str!("../REFERENCE.md");
        let table = &text[text
            .find("| Action | `rust-dos` |")
            .expect("the profiles' table")..];
        let rows: Vec<Vec<&str>> = table
            .lines()
            .take_while(|line| line.starts_with('|'))
            .map(|line| line.trim_matches('|').split('|').map(str::trim).collect())
            .collect();
        let names: Vec<&str> = rows[0][1..].iter().map(|n| n.trim_matches('`')).collect();
        let profiles: Vec<Profile> = names.iter().map(|n| Profile::parse(n).expect(n)).collect();
        assert_eq!(profiles, Profile::ALL);
        assert_eq!(rows.len() - 2, Action::ALL.len());
        for (row, action) in rows[2..].iter().zip(Action::ALL) {
            assert_eq!(row[0].trim_matches('`'), action.name());
            for (cell, profile) in row[1..].iter().zip(Profile::ALL) {
                let keys = profile.keys(action);
                assert_eq!(
                    *cell,
                    if keys.is_empty() {
                        "-".to_string()
                    } else {
                        keys_text(&keys)
                    },
                    "{} {}",
                    profile.name(),
                    action.name()
                );
            }
        }
    }

    #[test]
    fn an_actions_own_line_wins_over_the_profile() {
        let mut s = HotkeySettings::default();
        s.set("Save_State", "F5").unwrap();
        s.set("screenshot", "Ctrl+F2").unwrap();
        s.set("mute", "none").unwrap();
        let (table, warnings) = s.resolve();
        let f5 = Key::named("F5").unwrap();
        let f1 = Key::named("F1").unwrap();
        assert_eq!(
            table.action(false, false, false, f5),
            Some(Action::SaveState)
        );
        // Ctrl+F1, save_state's in the profile, is no longer, and
        // load_state's Ctrl+F2 is taken by screenshot.
        assert_eq!(table.action(true, false, false, f1), None);
        assert_eq!(
            table.action(true, false, false, Key::named("F2").unwrap()),
            Some(Action::Screenshot)
        );
        assert_eq!(table.key_text(Action::LoadState), None);
        assert_eq!(table.key_text(Action::Mute), None);
        // The old key of a remapped action does nothing.
        assert_eq!(
            table.action(true, false, false, Key::named("F8").unwrap()),
            None
        );
        assert_eq!(
            warnings,
            ["[hotkeys] screenshot takes Ctrl+F2 from load_state of the rust-dos profile"]
        );
        assert!(s.set("save_state", "Ctrl+Nope").is_err());
        assert!(s.set("teleport", "F1").is_err());
        assert!(s.set("profile", "nes").is_err());
    }

    #[test]
    fn two_actions_on_one_key_are_refused() {
        let mut s = HotkeySettings::default();
        s.set("save_state", "Ctrl+S").unwrap();
        s.set("load_state", "Ctrl+S, Ctrl+L").unwrap();
        let (table, warnings) = s.resolve();
        let key = |name| Key::named(name).unwrap();
        assert_eq!(
            table.action(true, false, false, key("S")),
            Some(Action::SaveState)
        );
        assert_eq!(
            table.action(true, false, false, key("L")),
            Some(Action::LoadState)
        );
        assert_eq!(
            warnings,
            ["[hotkeys] save_state and load_state are both Ctrl+S: save_state keeps it"]
        );
    }

    #[test]
    fn a_typing_key_without_ctrl_or_alt_is_warned_of() {
        let mut s = HotkeySettings::default();
        s.set("pause", "P").unwrap();
        s.set("screenshot", "F12").unwrap();
        let warnings = s.resolve().1;
        assert_eq!(
            warnings,
            ["[hotkeys] pause is P, which programs can't type while it is a hotkey"]
        );
    }

    #[test]
    fn entries_read_back_the_same() {
        let mut s = HotkeySettings::default();
        s.set("save_state", "F1, Ctrl+F1").unwrap();
        s.set("mute", "none").unwrap();
        let mut again = HotkeySettings::default();
        for (key, value) in s.entries() {
            if let Some(value) = value {
                again.set(key, &value).unwrap();
            }
        }
        assert_eq!(again, s);
        assert_eq!(
            HotkeySettings::default()
                .entries()
                .iter()
                .filter(|(_, v)| v.is_some())
                .count(),
            1
        );
    }
}
