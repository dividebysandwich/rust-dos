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
//! A custom profile (`CustomProfile`) is a set of these lines saved under
//! a name of the user's, in a file of the per-user directory, which the
//! settings window's Hotkeys page saves and loads. The configuration never
//! names one: loading it writes its lines into `[hotkeys]`.
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

    /// What the action does, as the settings window lists it.
    pub fn describe(self) -> &'static str {
        match self {
            Action::SettingsWindow => "Settings window",
            Action::SaveStatesPage => "Save states page",
            Action::PerformanceOverlay => "Performance overlay",
            Action::SoundVisualizer => "Sound visualizer",
            Action::Manuals => "Manuals",
            Action::SaveState => "Save state",
            Action::LoadState => "Load state",
            Action::NextSlot => "Next slot",
            Action::PreviousSlot => "Previous slot",
            Action::Pause => "Pause",
            Action::FastForward => "Fast forward (held)",
            Action::Rewind => "Rewind (held)",
            Action::Slower => "CPU slower",
            Action::Faster => "CPU faster",
            Action::Fullscreen => "Fullscreen",
            Action::MouseCapture => "Capture the mouse",
            Action::Mute => "Mute",
            Action::Screenshot => "Screenshot",
            Action::RecordSound => "Record sound",
            Action::RecordVideo => "Record video",
            Action::RecordAnimation => "Record a GIF",
            Action::SwapDisk => "Next disk",
            Action::CopyText => "Copy text",
            Action::PasteText => "Paste text",
            Action::PrinterEject => "Eject the printer's page",
            Action::Degauss => "Degauss",
            Action::RecenterView => "Recenter the 3D view",
        }
    }

    /// Whether the action does something while the settings window is
    /// open. A key without Ctrl or Alt bound to one that doesn't goes to
    /// the window instead, as does one the window has a use for (its F1,
    /// F2, F10 and so on) unless it is the window's own key.
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
    // PrintScreen with any modifiers: Shift with Ctrl or Alt falls back
    // to the key without it (`Hotkeys::action`).
    (
        Action::RecordAnimation,
        "PrintScreen, Shift+PrintScreen, Ctrl+PrintScreen, Alt+PrintScreen, Ctrl+Alt+PrintScreen",
    ),
    (Action::SwapDisk, "Ctrl+F4"),
    (Action::CopyText, "Ctrl+Shift+C"),
    (Action::PasteText, "Ctrl+Shift+V"),
    (Action::PrinterEject, "Ctrl+Shift+F5"),
    (Action::Degauss, "Ctrl+Shift+D"),
    (Action::RecenterView, "Ctrl+Shift+Home, Ctrl+Alt+Shift+Home"),
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
                    // A key that types is said only once it is a hotkey:
                    // one another line kept isn't.
                    None => {
                        table.push((combo, action));
                        if !combo.has_ctrl_or_alt() && combo.key.types() {
                            warnings.push(format!(
                                "[hotkeys] {} is {}, which programs can't type while it is a hotkey",
                                action.name(),
                                combo
                            ));
                        }
                    }
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

    /// Whether `action` has a line of its own over the profile.
    pub fn is_own(&self, action: Action) -> bool {
        self.keys[action.index()].is_some()
    }

    /// The keys `action` is bound to: its own line's, else the profile's.
    /// Where two actions are bound to one key, the table gives it to one
    /// of them (`resolve`).
    pub fn keys_of(&self, action: Action) -> Vec<Combo> {
        match &self.keys[action.index()] {
            Some(keys) => keys.clone(),
            None => self.profile.keys(action),
        }
    }

    /// Bind `action` to `keys`. Keys the profile gives it leave it without
    /// a line of its own.
    pub fn set_keys(&mut self, action: Action, keys: Vec<Combo>) {
        self.keys[action.index()] = (keys != self.profile.keys(action)).then_some(keys);
    }

    /// Give `action` the profile's keys again.
    pub fn reset(&mut self, action: Action) {
        self.keys[action.index()] = None;
    }

    /// Give every action the profile's keys again.
    pub fn reset_all(&mut self) {
        self.keys = std::array::from_fn(|_| None);
    }

    /// How many actions have lines of their own.
    pub fn own_count(&self) -> usize {
        self.keys.iter().filter(|k| k.is_some()).count()
    }

    /// Bind `combo` to `action`, in place of its keys, or with `add` as
    /// one more. Any other action bound to `combo` loses it, so that the
    /// key does what it was bound for last.
    pub fn bind(&mut self, action: Action, combo: Combo, add: bool) {
        for other in Action::ALL.into_iter().filter(|&a| a != action) {
            let keys = self.keys_of(other);
            if keys.contains(&combo) {
                self.set_keys(other, keys.into_iter().filter(|&c| c != combo).collect());
            }
        }
        let mut keys = if add {
            self.keys_of(action)
        } else {
            Vec::new()
        };
        if !keys.contains(&combo) {
            keys.push(combo);
        }
        self.set_keys(action, keys);
    }

    /// Whether the settings window still opens with these keys: by a key
    /// of its own, or by the right button held still (`menu_hold`, the
    /// `mouse_menu_hold` setting).
    pub fn window_reachable(&self, menu_hold: bool) -> bool {
        menu_hold || self.table().keys(Action::SettingsWindow).next().is_some()
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

    /// The action `combo` is bound to, exactly: without the Shift that
    /// `action` leaves out.
    pub fn owner(&self, combo: Combo) -> Option<Action> {
        self.table
            .iter()
            .find(|(c, _)| *c == combo)
            .map(|&(_, a)| a)
    }
}

impl Combo {
    /// Whether the settings window has a use for the combination itself:
    /// a key without Ctrl or Alt that moves in its lists or types in its
    /// fields. The settings window's own key closes the window whatever
    /// the key, so it can't be one of these.
    pub fn window_uses(self) -> bool {
        !self.has_ctrl_or_alt()
            && (self.key.types()
                || matches!(
                    self.key.name(),
                    "Escape"
                        | "Up"
                        | "Down"
                        | "Left"
                        | "Right"
                        | "PageUp"
                        | "PageDown"
                        | "Home"
                        | "End"
                        | "Insert"
                        | "Delete"
                ))
    }
}

/// The folder of the custom profiles, in rust-dos's per-user directory.
pub fn custom_dir() -> Option<std::path::PathBuf> {
    crate::config::user_dir().map(|dir| dir.join("hotkeys"))
}

/// The file name a custom profile is saved under, after its name.
const CUSTOM_EXTENSION: &str = "conf";

/// A set of keys saved under a name of the user's: the `[hotkeys]` lines
/// of every action, and the profile they were made over. Loading one picks
/// that profile, and the keys that differ from it become the actions' own
/// lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustomProfile {
    pub name: String,
    pub base: Profile,
    /// Each action's keys, or None for the base profile's: a file edited
    /// by hand may leave an action out.
    keys: [Option<Vec<Combo>>; Action::ALL.len()],
}

impl CustomProfile {
    /// The keys `settings` binds, under `name`.
    pub fn of(name: &str, settings: &HotkeySettings) -> CustomProfile {
        CustomProfile {
            name: name.to_string(),
            base: settings.profile,
            keys: std::array::from_fn(|i| Some(settings.keys_of(Action::ALL[i]))),
        }
    }

    pub fn keys_of(&self, action: Action) -> Vec<Combo> {
        match &self.keys[action.index()] {
            Some(keys) => keys.clone(),
            None => self.base.keys(action),
        }
    }

    /// Put the profile's keys in `settings`, in place of all they had.
    pub fn load_into(&self, settings: &mut HotkeySettings) {
        settings.profile = self.base;
        for action in Action::ALL {
            settings.set_keys(action, self.keys_of(action));
        }
    }

    /// Whether `settings` bind the keys of this profile, over its base.
    pub fn is_loaded_in(&self, settings: &HotkeySettings) -> bool {
        settings.profile == self.base
            && Action::ALL
                .into_iter()
                .all(|a| settings.keys_of(a) == self.keys_of(a))
    }

    /// The file's text: the lines `[hotkeys]` takes.
    pub fn text(&self) -> String {
        let mut text = format!(
            "# Rust-DOS's hotkeys, the custom profile \"{}\": the lines of [hotkeys].\n\
             [hotkeys]\nprofile={}\n",
            self.name,
            self.base.name()
        );
        for action in Action::ALL {
            let keys = keys_text(&self.keys_of(action));
            text.push_str(&format!("{}={}\n", action.name(), keys));
        }
        text
    }

    /// The profile `name` from its file's text. An action the file leaves
    /// out has the base profile's keys.
    pub fn parse(name: &str, text: &str) -> Result<CustomProfile, String> {
        let mut settings = HotkeySettings::default();
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty()
                || line.starts_with('#')
                || line.starts_with(';')
                || line.eq_ignore_ascii_case("[hotkeys]")
            {
                continue;
            }
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| format!("line {}: '{}' isn't a setting", n + 1, line))?;
            settings
                .set(key, value)
                .map_err(|e| format!("line {}: {}", n + 1, e))?;
        }
        Ok(CustomProfile {
            name: name.to_string(),
            base: settings.profile,
            keys: settings.keys,
        })
    }
}

/// `name` as a custom profile's, trimmed, or why it can't be one: it names
/// a file, so it is letters, digits, spaces, '-' and '_', and it isn't a
/// built-in profile's name.
pub fn custom_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Type a name for the profile".to_string());
    }
    if name.chars().count() > 32 {
        return Err("The name is longer than 32 characters".to_string());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '_'))
    {
        return Err("A name has letters, digits, spaces, '-' and '_' only".to_string());
    }
    if Profile::ALL
        .iter()
        .any(|p| p.name().eq_ignore_ascii_case(name) || p.describe().eq_ignore_ascii_case(name))
    {
        return Err(format!("{} is a built-in profile's name", name));
    }
    Ok(name.to_string())
}

/// The custom profiles in `dir`, by name. Files that don't read as one
/// are left out.
pub fn custom_profiles(dir: &std::path::Path) -> Vec<CustomProfile> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut profiles: Vec<CustomProfile> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension()?.to_str()? != CUSTOM_EXTENSION {
                return None;
            }
            let name = custom_name(path.file_stem()?.to_str()?).ok()?;
            let text = std::fs::read_to_string(&path).ok()?;
            CustomProfile::parse(&name, &text).ok()
        })
        .collect();
    profiles.sort_by_key(|p| p.name.to_lowercase());
    profiles
}

/// Write `profile` to its file in `dir`, in place of one of its name.
pub fn save_custom(dir: &std::path::Path, profile: &CustomProfile) -> Result<(), String> {
    let name = custom_name(&profile.name)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
    let path = dir.join(format!("{}.{}", name, CUSTOM_EXTENSION));
    std::fs::write(&path, profile.text()).map_err(|e| format!("{}: {}", path.display(), e))
}

/// Delete the custom profile `name`'s file in `dir`.
pub fn delete_custom(dir: &std::path::Path, name: &str) -> Result<(), String> {
    let path = dir.join(format!("{}.{}", custom_name(name)?, CUSTOM_EXTENSION));
    std::fs::remove_file(&path).map_err(|e| format!("{}: {}", path.display(), e))
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
    /// What the event loop's checks did with a key before the keys could
    /// be changed, in their order: each asked for its modifiers, and most
    /// for Ctrl without Alt or Alt without Ctrl, and didn't look at Shift
    /// unless it told two keys apart. PrintScreen took any modifiers, and
    /// the 3D scene's Ctrl+Shift+Home any Alt.
    fn old_check(ctrl: bool, alt: bool, shift: bool, key: &str) -> Option<Action> {
        use Action::*;
        let (c, a) = (ctrl && !alt, alt && !ctrl);
        Some(match key {
            "Home" if ctrl && shift => RecenterView,
            "D" if c && shift => Degauss,
            "S" if c && shift => SoundVisualizer,
            "F12" if c && shift => PerformanceOverlay,
            "F12" if c => SettingsWindow,
            "M" if c && shift => Manuals,
            "C" if c && shift => CopyText,
            "V" if c && shift => PasteText,
            "F1" if c => SaveState,
            "F2" if c => LoadState,
            "F3" if c && shift => PreviousSlot,
            "F3" if c => NextSlot,
            "F9" if c => SaveStatesPage,
            "F4" if c => SwapDisk,
            "Pause" if a => Pause,
            "F11" if c && shift => Faster,
            "F11" if c => Slower,
            "F11" if a => Rewind,
            "F12" if a => FastForward,
            "F10" if c => MouseCapture,
            "Enter" if a => Fullscreen,
            "F5" if c && shift => PrinterEject,
            "F5" if c => Screenshot,
            "F6" if c => RecordSound,
            "F7" if c => RecordVideo,
            "F8" if c => Mute,
            "PrintScreen" => RecordAnimation,
            _ => return None,
        })
    }

    /// Every key with every modifier does in the default profile what the
    /// checks before it did.
    #[test]
    fn the_default_profile_has_the_keys_rust_dos_always_had() {
        let table = HotkeySettings::default().table();
        for i in 0..KEYS.len() {
            let key = Key(i as u8);
            for mods in 0..8 {
                let (ctrl, alt, shift) = (mods & 1 != 0, mods & 2 != 0, mods & 4 != 0);
                assert_eq!(
                    table.action(ctrl, alt, shift, key),
                    old_check(ctrl, alt, shift, key.name()),
                    "ctrl {} alt {} shift {} {}",
                    ctrl,
                    alt,
                    shift,
                    key.name()
                );
            }
        }
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
    fn a_typing_key_that_lost_a_clash_is_not_warned_of() {
        let mut s = HotkeySettings::default();
        s.set("save_state", "P").unwrap();
        s.set("pause", "P").unwrap();
        let warnings = s.resolve().1;
        assert_eq!(
            warnings,
            [
                "[hotkeys] save_state is P, which programs can't type while it is a hotkey",
                "[hotkeys] save_state and pause are both P: save_state keeps it",
            ]
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
    fn binding_a_key_takes_it_from_the_action_that_had_it() {
        let mut s = HotkeySettings::default();
        // Ctrl+F5 is screenshot's in the profile.
        s.bind(Action::SaveState, combo("Ctrl+F5"), false);
        assert_eq!(s.keys_of(Action::SaveState), [combo("Ctrl+F5")]);
        assert_eq!(s.keys_of(Action::Screenshot), Vec::new());
        let (table, warnings) = s.resolve();
        assert_eq!(warnings, Vec::<String>::new());
        assert_eq!(table.owner(combo("Ctrl+F5")), Some(Action::SaveState));
        assert_eq!(table.owner(combo("Ctrl+F1")), None);
        // Another key beside it, and the same key again, once.
        s.bind(Action::SaveState, combo("F11"), true);
        s.bind(Action::SaveState, combo("F11"), true);
        assert_eq!(
            s.keys_of(Action::SaveState),
            [combo("Ctrl+F5"), combo("F11")]
        );
        // The profile's keys again leave no line of its own.
        s.set_keys(Action::SaveState, vec![combo("Ctrl+F1")]);
        assert!(!s.is_own(Action::SaveState));
        assert_eq!(s.own_count(), 1);
        s.reset_all();
        assert_eq!(s, HotkeySettings::default());
    }

    #[test]
    fn the_settings_window_keeps_a_way_to_open_it() {
        let mut s = HotkeySettings::default();
        assert!(s.window_reachable(false));
        s.set_keys(Action::SettingsWindow, Vec::new());
        assert!(!s.window_reachable(false));
        assert!(s.window_reachable(true));
        // Its key taken by another action is as good as none.
        let mut s = HotkeySettings::default();
        s.bind(Action::Pause, combo("Ctrl+F12"), false);
        assert!(!s.window_reachable(false));
        assert!(combo("Up").window_uses());
        assert!(combo("Shift+A").window_uses());
        assert!(combo("Escape").window_uses());
        assert!(!combo("Ctrl+Up").window_uses());
        assert!(!combo("F1").window_uses());
        assert!(!combo("Alt+A").window_uses());
    }

    #[test]
    fn a_custom_profile_reads_what_it_writes_and_loads_its_keys() {
        let mut s = HotkeySettings {
            profile: Profile::DosBox,
            ..Default::default()
        };
        s.bind(Action::SaveState, combo("F5"), false);
        s.set_keys(Action::Mute, Vec::new());
        let custom = CustomProfile::of("My keys", &s);
        let text = custom.text();
        assert!(text.contains("\nprofile=dosbox\n"), "{}", text);
        assert!(text.contains("\nsave_state=F5\n"), "{}", text);
        assert!(text.contains("\nmute=none\n"), "{}", text);
        assert_eq!(CustomProfile::parse("My keys", &text), Ok(custom.clone()));
        assert!(custom.is_loaded_in(&s));

        // Loading it over other keys gives its keys, with lines of their
        // own only where they differ from its profile.
        let mut other = HotkeySettings::default();
        other.bind(Action::Pause, combo("Ctrl+P"), false);
        assert!(!custom.is_loaded_in(&other));
        custom.load_into(&mut other);
        assert_eq!(other, s);
        assert_eq!(other.own_count(), 2);

        // A file without a line for an action leaves it the profile's.
        let short =
            CustomProfile::parse("short", "[hotkeys]\nprofile=pcsx2\npause=Ctrl+P\n").unwrap();
        assert_eq!(short.keys_of(Action::SaveState), [combo("F1")]);
        assert_eq!(short.keys_of(Action::Pause), [combo("Ctrl+P")]);
        assert!(CustomProfile::parse("bad", "pause=Ctrl+Nope").is_err());
        assert!(CustomProfile::parse("bad", "teleport=F1").is_err());
        assert!(CustomProfile::parse("bad", "just words").is_err());
    }

    #[test]
    fn custom_profiles_are_files_named_after_them() {
        let dir = std::env::temp_dir().join(format!("rust-dos-hotkeys-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(custom_profiles(&dir), Vec::new());
        let mut s = HotkeySettings::default();
        s.bind(Action::SaveState, combo("F5"), false);
        save_custom(&dir, &CustomProfile::of("zed", &s)).unwrap();
        save_custom(
            &dir,
            &CustomProfile::of("Alpha 2", &HotkeySettings::default()),
        )
        .unwrap();
        std::fs::write(dir.join("broken.conf"), "pause=Ctrl+Nope").unwrap();
        std::fs::write(dir.join("notes.txt"), "save_state=F1").unwrap();
        let found = custom_profiles(&dir);
        let names: Vec<&str> = found.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Alpha 2", "zed"]);
        assert!(found[1].is_loaded_in(&s));
        // Saved again under its name, it is replaced.
        save_custom(&dir, &CustomProfile::of("zed", &HotkeySettings::default())).unwrap();
        assert!(custom_profiles(&dir)[1].is_loaded_in(&HotkeySettings::default()));
        delete_custom(&dir, "zed").unwrap();
        assert_eq!(custom_profiles(&dir).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(custom_name("  my keys "), Ok("my keys".to_string()));
        assert!(custom_name("").is_err());
        assert!(custom_name("../up").is_err());
        assert!(custom_name("DOSBox").is_err());
        assert!(custom_name("pcsx-redux").is_err());
        assert!(custom_name(&"x".repeat(33)).is_err());
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
