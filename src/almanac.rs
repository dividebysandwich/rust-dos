//! The game settings almanac, almanac.rust-dos.com: players' reports of
//! the settings games run well with, and for each game the settings most
//! reports chose, or the ones a moderator blessed, which win.
//!
//! The settings window asks it for a program's settings and reports the
//! ones a game played well with (config_ui/almanac.rs). With
//! `almanac_auto`, a program started from the prompt waits while the
//! frontend asks it by the program file's name and checksums, and starts
//! with what it answers (`ask_first`, `ShellWait::Almanac`); the frontend
//! puts the user's settings back once it has ended.
//!
//! Only the settings of the emulated machine go back and forth (`KEYS`):
//! never drives, commands or paths. With them goes what the game's
//! profile says of the game (`About`): its year, publisher and the
//! player's notes, and its UUID, with which the almanac replaces the
//! report the profile sent before instead of adding another.

use crate::config::{self, Settings};
use crate::cpu::Cpu;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// The almanac's address, for the messages.
pub const SITE: &str = "almanac.rust-dos.com";
/// Its JSON API.
pub const API: &str = "https://almanac.rust-dos.com/api/v1";
/// The variable that names another almanac's API, one's own or a test's.
pub const API_VARIABLE: &str = "RUST_DOS_ALMANAC_API";

/// The settings the almanac knows, by section and key: those of the
/// machine a game runs on, and the frame rate cap, which some games need
/// (Test Drive 4 misbehaves above 60) and a benchmark's entry turns off.
pub const KEYS: [(&str, &str); 41] = [
    ("emulator", "machine"),
    ("emulator", "cpu"),
    ("emulator", "core"),
    ("emulator", "fpu"),
    ("emulator", "cycles"),
    ("emulator", "memsize"),
    ("emulator", "ems"),
    ("emulator", "umb"),
    ("emulator", "dpmi"),
    ("emulator", "dos_high"),
    ("emulator", "dos_version"),
    ("emulator", "win32"),
    ("emulator", "win32_version"),
    ("emulator", "win32_depth"),
    ("emulator", "win32_desktop"),
    ("emulator", "win32_locale"),
    ("emulator", "voodoo"),
    ("emulator", "voodoo_model"),
    ("emulator", "voodoo_memory"),
    ("emulator", "powervr"),
    ("emulator", "composite"),
    ("emulator", "monochrome"),
    ("emulator", "hard_disk_speed"),
    ("emulator", "floppy_disk_speed"),
    ("emulator", "fps_cap"),
    ("sound", "sbtype"),
    ("sound", "sbbase"),
    ("sound", "irq"),
    ("sound", "dma"),
    ("sound", "hdma"),
    ("sound", "awe32ram"),
    ("sound", "opl"),
    ("sound", "gus"),
    ("sound", "gusbase"),
    ("sound", "gusirq"),
    ("sound", "gusdma"),
    ("sound", "midisynth"),
    ("sound", "mt32model"),
    ("sound", "lpt_dac"),
    ("sound", "tandy"),
    ("joystick", "joysticktype"),
];

/// Whether the almanac's setting `key` of `section` is one of `KEYS`,
/// under its current name or an old one (`config::OLD_NAMES`), which a
/// recommendation may still carry and the configuration still reads.
fn known(section: &str, key: &str) -> bool {
    let key = config::renamed(section, key);
    KEYS.iter()
        .any(|&(s, k)| s.eq_ignore_ascii_case(section) && k.eq_ignore_ascii_case(key))
}

/// A file of a game's version, as the almanac knows it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileInfo {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crc32: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub md5: Option<String>,
}

/// The file `name` (a DOS path; the almanac takes its name) with the
/// contents `bytes`: its size and checksums.
pub fn file_info(name: &str, bytes: &[u8]) -> FileInfo {
    use md5::{Digest, Md5};
    let base = name.rsplit(['\\', '/', ':']).next().unwrap_or(name);
    let md5: String = Md5::digest(bytes)
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect();
    FileInfo {
        name: base.to_ascii_lowercase(),
        size: Some(bytes.len() as u64),
        crc32: Some(format!("{:08x}", crc32fast::hash(bytes))),
        md5: Some(md5),
    }
}

/// What the program `bytes` are, as the almanac files a version under:
/// `win32` for a Windows 95/98 program (PE), `win16` for a Windows 3.1
/// one (NE, not OS/2's), `dos` for anything else. The Windows ones run on
/// the built-in Windows (`win32`).
pub fn platform_of(bytes: &[u8]) -> &'static str {
    if rust_dos_pe::is_pe(bytes) {
        "win32"
    } else if crate::win32::is_win16(bytes) {
        "win16"
    } else {
        "dos"
    }
}

/// Whether the program file `name` (lower case, without a directory) is
/// a game's installer: `SETUP.EXE`, `INSTALL.EXE` and the like, or the
/// engine InstallShield's launcher unpacks (`_INS0432._MP`). What an
/// installer is says nothing of the game it installs: a Windows 95 game
/// often comes with a Windows 3.1 setup.
pub fn is_installer(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name);
    matches!(stem, "setup" | "_setup" | "install" | "instal") || stem.starts_with("_ins")
}

/// The program a game starts with, as the settings window asks about it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Program {
    /// Its file, a DOS path.
    pub path: String,
    pub files: Vec<FileInfo>,
    /// [`platform_of`] its file, when there is one and it isn't an
    /// installer's ([`is_installer`]).
    pub platform: Option<String>,
    /// The name to look the game up by: its profile's, or the program's.
    pub name: String,
    /// What the profile of the game playing says of it, if one plays.
    pub about: Option<About>,
}

/// What a game profile tells the almanac of its game (`[game]`'s `year`,
/// `publisher`, `notes` and `uuid`, games.rs).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct About {
    pub year: Option<i32>,
    pub publisher: Option<String>,
    pub notes: Option<String>,
    /// The profile's UUID, the report's key: a report sent with it
    /// replaces the one sent with it on the same game before.
    pub key: Option<String>,
}

impl About {
    /// What the profile `text` says.
    pub fn of_profile(text: &str) -> About {
        let config = config::parse(text, Path::new("/"), None);
        About {
            year: config.game_year,
            publisher: config.game_publisher,
            notes: config.game_notes,
            key: config.game_uuid,
        }
    }

    /// The profile `text` saying `self`'s year, publisher and notes (the
    /// ones that are None taken out), and with a UUID of its own.
    pub fn into_profile(&self, text: &str) -> String {
        let year = self.year.map(|y| y.to_string());
        let mut text = text.to_string();
        for (key, value) in [
            ("year", year.as_deref()),
            ("publisher", self.publisher.as_deref()),
            ("notes", self.notes.as_deref()),
        ] {
            text = crate::games::set_game_value(&text, key, value);
        }
        crate::games::with_uuid(&text)
    }
}

/// A report the almanac took: its number, whether it replaced the one
/// the profile sent before, and whether the game it was to add as a new
/// one was in the almanac already, so it went to that game.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub struct Reported {
    pub id: i64,
    #[serde(default)]
    pub replaced: bool,
    #[serde(skip)]
    pub existing: bool,
}

/// The program at the DOS path `path`, read from the machine's drives.
pub fn program(cpu: &mut Cpu, path: &str) -> Option<Program> {
    let bytes = cpu.bus.disk.file_data(path)?.read().ok()?;
    let file = file_info(path, &bytes);
    let name = file
        .name
        .rsplit_once('.')
        .map_or(file.name.as_str(), |(stem, _)| stem)
        .to_ascii_uppercase();
    let platform = (!is_installer(&file.name)).then(|| platform_of(&bytes).to_string());
    Some(Program {
        path: path.to_string(),
        files: vec![file],
        platform,
        name,
        about: None,
    })
}

/// A report's colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Rating {
    #[default]
    Green,
    Yellow,
    Red,
}

impl Rating {
    pub const ALL: [Rating; 3] = [Rating::Green, Rating::Yellow, Rating::Red];

    pub fn label(self) -> &'static str {
        match self {
            Rating::Green => "Runs great",
            Rating::Yellow => "Playable, some problems",
            Rating::Red => "Not playable",
        }
    }
}

/// How many reports gave each colour.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Ratings {
    #[serde(default)]
    pub green: u32,
    #[serde(default)]
    pub yellow: u32,
    #[serde(default)]
    pub red: u32,
}

impl Ratings {
    /// "3 green, 1 yellow", or "no reports".
    pub fn describe(&self) -> String {
        let parts: Vec<String> = [
            (self.green, "green"),
            (self.yellow, "yellow"),
            (self.red, "red"),
        ]
        .into_iter()
        .filter(|&(n, _)| n > 0)
        .map(|(n, colour)| format!("{} {}", n, colour))
        .collect();
        if parts.is_empty() {
            "no reports".to_string()
        } else {
            parts.join(", ")
        }
    }
}

/// A version of a game a lookup found (`POST /games/lookup`).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Match {
    /// The game's slug.
    pub game: String,
    pub name: String,
    pub version_id: i64,
    pub version_label: String,
    /// Files that matched by name and checksum, and by name alone.
    #[serde(default)]
    pub hash_matches: u32,
    #[serde(default)]
    pub name_matches: u32,
}

/// A game a search found (`GET /games?q=`).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct GameSummary {
    pub slug: String,
    pub name: String,
    #[serde(default)]
    pub year: Option<i32>,
    #[serde(default)]
    pub rating: Ratings,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Setting {
    pub section: String,
    pub key: String,
    pub value: String,
}

/// Where the settings recommended come from.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Source {
    /// A moderator picked one report's settings.
    Blessed {
        #[serde(default)]
        blessed_by: Option<String>,
    },
    /// The value most reports chose, setting by setting.
    Consensus { reports: u32 },
    /// No one reported settings yet.
    None,
}

/// What a report the recommendation comes from says: its player's notes,
/// and a moderator's description of how the game plays.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Note {
    pub rating: Rating,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub rust_dos_version: Option<String>,
}

impl Note {
    /// `"Crashes in the menu" (red, 1.8.3)`, a moderator's words first;
    /// None if it says nothing.
    pub fn describe(&self) -> Option<String> {
        let words: Vec<&str> = [self.description.as_deref(), self.notes.as_deref()]
            .into_iter()
            .flatten()
            .map(str::trim)
            .filter(|w| !w.is_empty())
            .collect();
        if words.is_empty() {
            return None;
        }
        let colour = match self.rating {
            Rating::Green => "green",
            Rating::Yellow => "yellow",
            Rating::Red => "red",
        };
        let from = match &self.rust_dos_version {
            Some(v) => format!("{}, {}", colour, v),
            None => colour.to_string(),
        };
        Some(format!("\u{201C}{}\u{201D} ({})", words.join(" "), from))
    }
}

/// A game's recommended settings (`GET /games/{slug}/config`).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Recommendation {
    pub source: Source,
    #[serde(default)]
    pub rating: Ratings,
    #[serde(default)]
    pub settings: Vec<Setting>,
    /// The notes of the reports it comes from, newest first: the blessed
    /// one's, or those of the reports the consensus counts, red ones too.
    #[serde(default)]
    pub notes: Vec<Note>,
}

impl Recommendation {
    /// The settings Rust-DOS takes from it.
    pub fn usable(&self) -> Vec<&Setting> {
        self.settings
            .iter()
            .filter(|s| known(&s.section, &s.key))
            .collect()
    }

    /// "blessed by a moderator", "the choice of 4 reports".
    pub fn describe(&self) -> String {
        match &self.source {
            Source::Blessed {
                blessed_by: Some(by),
            } => format!("blessed by {}", by),
            Source::Blessed { .. } => "blessed by a moderator".to_string(),
            Source::Consensus { reports: 1 } => "from 1 report".to_string(),
            Source::Consensus { reports } => format!("the choice of {} reports", reports),
            Source::None => "no settings reported yet".to_string(),
        }
    }

    /// Its notes, `Note::describe`d, one after the other.
    pub fn notes_text(&self) -> String {
        let notes: Vec<String> = self.notes.iter().filter_map(Note::describe).collect();
        notes.join(" ")
    }
}

/// A game (and version) the almanac has, as the window lists it: found by
/// the program's file or by name, with its settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub slug: String,
    pub name: String,
    /// The version the program's file matched, if one did: its id and label.
    pub version: Option<(i64, String)>,
    /// It matched by checksum, not only by name.
    pub hash_match: bool,
    pub ratings: Ratings,
    /// Its settings, None if they couldn't be had.
    pub recommendation: Option<Recommendation>,
}

impl Candidate {
    /// Whether it has settings to apply.
    pub fn has_settings(&self) -> bool {
        self.recommendation
            .as_ref()
            .is_some_and(|r| !r.usable().is_empty())
    }
}

/// The candidate whose settings a program starts with (`almanac_auto`):
/// one its file matched by checksum, blessed settings before a consensus.
pub fn best(candidates: &[Candidate]) -> Option<&Candidate> {
    let usable = || {
        candidates
            .iter()
            .filter(|c| c.hash_match && c.has_settings())
    };
    let blessed = |c: &&Candidate| {
        c.recommendation
            .as_ref()
            .is_some_and(|r| matches!(r.source, Source::Blessed { .. }))
    };
    usable().find(blessed).or_else(|| usable().next())
}

/// The settings a report sends: `settings`' values of `KEYS`, by section.
/// The memory is `memory_mb`, the size the machine runs with, also where
/// `memsize` isn't set: the default for its CPU and display adapter is
/// what the game ran with, and whoever takes the report's settings may
/// have another. (A CPU chosen in the settings window changes the
/// settings' default at once, the machine's memory at the next start.)
/// The CPU is the model the machine has, also where `cpu` isn't set and
/// the model follows the display adapter (`Settings::cpu_model`).
pub fn report_settings(
    settings: &Settings,
    memory_mb: usize,
    home: Option<&Path>,
) -> BTreeMap<String, BTreeMap<String, String>> {
    let mut map: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let memory =
        Settings { memsize: Some(memory_mb), cpu: Some(settings.cpu_model()), ..settings.clone() };
    for (section, key, value) in config::values(&memory, home) {
        if let Some(value) = value
            && known(section, key)
        {
            map.entry(section.to_string())
                .or_default()
                .insert(key.to_string(), value);
        }
    }
    map
}

/// `base` with `settings` over it, those of `KEYS` only, as a profile's
/// are layered over the configuration's (games.rs's `prepare`). Each
/// value goes through the configuration's parser, which ignores one it
/// doesn't take (`fps_cap=500`), leaving `base`'s.
///
/// Reports send the memory the game ran with, set or not, so a
/// recommendation always has a `memsize`. Where `base` doesn't set it and
/// the recommended size is the default for the recommended CPU and display
/// adapter, it stays unset: the machine gets the same size, and saving
/// the settings writes no `memsize` line that would stop it following a
/// later change of the CPU or the adapter. Any other recommended size is
/// set, below the default too, as the game is known to run with it. The
/// CPU, which reports always send too, stays unset the same way where
/// `base` doesn't set it and the recommended model is the default for the
/// recommended display adapter (`default_cpu`).
pub fn layer(base: &Settings, settings: &[&Setting], home: Option<&Path>) -> Settings {
    let mut sections: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for s in settings {
        let clean = |t: &str| !t.is_empty() && !t.contains(['\n', '\r', '[', ']', '=']);
        if known(&s.section, &s.key) && clean(&s.value) {
            sections
                .entry(s.section.as_str())
                .or_default()
                .push(format!("{}={}", s.key, s.value.trim()));
        }
    }
    let mut text = config::update_text("", &Settings::default(), base, &[], home);
    for (section, lines) in sections {
        text.push_str(&format!("\n[{}]\n{}\n", section, lines.join("\n")));
    }
    let mut layered = Settings::from_config(&config::parse(&text, Path::new("/"), home));
    if base.cpu.is_none() && layered.cpu == Some(config::default_cpu(layered.machine)) {
        layered.cpu = None;
    }
    if base.memsize.is_none()
        && layered.memsize == Some(config::default_memsize(layered.cpu_model(), layered.machine))
    {
        layered.memsize = None;
    }
    layered
}

/// How long a program waits for the almanac at most, in the host's time:
/// the frontend gives up then (`WAIT_TICKS` is only for a frontend that
/// doesn't answer at all).
pub const WAIT: std::time::Duration = std::time::Duration::from_secs(10);
/// How long it waits in emulated time, in PIT ticks: longer, as the
/// machine can run ahead of the host's clock while it waits.
const WAIT_TICKS: u64 = 60 * crate::timer::PIT_HZ;

/// The prompt's side of `almanac_auto`.
#[derive(Clone, Debug, Default)]
pub struct Shell {
    /// Programs started from the prompt wait for the almanac: the frontend
    /// sets it, with `almanac_auto` on and no game profile playing.
    pub auto: bool,
    /// The program that waited, which runs without asking again.
    pub cleared: Option<String>,
    /// What the frontend made of the answer, which ends the wait.
    pub answer: Option<String>,
    /// The last program the prompt started, a DOS path, for the settings
    /// window to ask about.
    pub last_program: Option<String>,
}

/// Before the command line `line` starts the program `command` from the
/// top-level prompt: with `almanac_auto`, the prompt waits while the
/// frontend asks the almanac about it, and runs the line once the answer
/// is in. Whether it waits.
pub fn ask_first(cpu: &mut Cpu, line: &str, command: &str) -> bool {
    if !cpu.almanac.auto || cpu.secondary.is_some() || !cpu.process_stack.is_empty() {
        return false;
    }
    let Some(path) = crate::exec::find_program(cpu, command) else {
        return false;
    };
    if path.to_ascii_uppercase().ends_with(".BAT")
        || cpu.almanac.cleared.take().is_some_and(|p| p == path)
    {
        return false;
    }
    let name = path
        .rsplit(['\\', ':'])
        .next()
        .unwrap_or(&path)
        .to_ascii_uppercase();
    crate::video::print_string(
        cpu,
        &format!("Asking {} about {} (Esc skips)...\r\n", SITE, name),
    );
    let until = cpu.bus.clock.now_ticks() + WAIT_TICKS;
    cpu.almanac.answer = None;
    let wait = crate::shell::ShellWait::Almanac {
        line: line.to_string(),
        program: path,
        until,
    };
    crate::shell::enter_wait(cpu, wait);
    true
}

/// The program the prompt waits to start, if it waits for the almanac.
pub fn waiting(cpu: &Cpu) -> Option<&str> {
    match &cpu.shell_wait {
        Some(crate::shell::ShellWait::Almanac { program, .. }) => Some(program),
        _ => None,
    }
}

/// While the prompt waits for the almanac, every tick: once the frontend
/// answered or the time is up, the command line runs. Whether it waits on.
pub fn tick(cpu: &mut Cpu) -> bool {
    let Some(crate::shell::ShellWait::Almanac { until, .. }) = &cpu.shell_wait else {
        return true;
    };
    let text = match cpu.almanac.answer.take() {
        Some(text) => text,
        None if cpu.bus.clock.now_ticks() >= *until => {
            format!("No answer from {}: your own settings", SITE)
        }
        None => return true,
    };
    finish(cpu, &text);
    false
}

/// A key while the prompt waits for the almanac: Esc starts the program
/// with the settings as they are. Whether the wait is over.
pub fn key(cpu: &mut Cpu, key: u8) -> bool {
    if key == 0x1B {
        finish(cpu, "Skipped: your own settings");
        return true;
    }
    false
}

/// The wait is over: say how, and run the command line.
fn finish(cpu: &mut Cpu, text: &str) {
    let Some(crate::shell::ShellWait::Almanac { line, program, .. }) = cpu.shell_wait.take() else {
        return;
    };
    crate::video::print_string(cpu, &format!("{}\r\n", text));
    cpu.almanac.cleared = Some(program);
    cpu.pending_command = Some(line);
}

/// What a report is of: a game the almanac has (its slug, and the
/// version the program's file matched), or a new one by name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Subject {
    Game { slug: String, version: Option<i64> },
    New(String),
}

/// `client::candidates`, where this rust-dos asks the almanac.
pub fn candidates(
    files: &[FileInfo],
    name: &str,
    with_names: bool,
) -> Result<Vec<Candidate>, String> {
    #[cfg(all(feature = "sdl", not(target_arch = "wasm32")))]
    return client::candidates(files, name, with_names);
    #[cfg(not(all(feature = "sdl", not(target_arch = "wasm32"))))]
    {
        let _ = (files, name, with_names);
        Err(format!("This Rust-DOS doesn't ask {}", SITE))
    }
}

/// `client::report`, where this rust-dos asks the almanac.
pub fn report(
    subject: &Subject,
    files: &[FileInfo],
    platform: Option<&str>,
    rating: Rating,
    settings: &BTreeMap<String, BTreeMap<String, String>>,
    about: &About,
) -> Result<Reported, String> {
    #[cfg(all(feature = "sdl", not(target_arch = "wasm32")))]
    return client::report(subject, files, platform, rating, settings, about);
    #[cfg(not(all(feature = "sdl", not(target_arch = "wasm32"))))]
    {
        let _ = (subject, files, platform, rating, settings, about);
        Err(format!("This Rust-DOS doesn't ask {}", SITE))
    }
}

/// Asking the almanac, over HTTPS.
#[cfg(all(feature = "sdl", not(target_arch = "wasm32")))]
pub mod client {
    use super::*;
    use serde::de::DeserializeOwned;

    fn agent() -> ureq::Agent {
        ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(8)))
            .user_agent(format!("rust-dos/{}", env!("CARGO_PKG_VERSION")))
            .http_status_as_error(false)
            .build()
            .into()
    }

    /// Why the almanac didn't do what it was asked: the message for the
    /// user, and the game whose name a new game took (`POST /games`
    /// answers 409 with its slug as `game`).
    struct Problem {
        message: String,
        game: Option<String>,
    }

    impl From<String> for Problem {
        fn from(message: String) -> Problem {
            Problem {
                message,
                game: None,
            }
        }
    }

    /// The answer's JSON, or why there is none.
    fn reply<T: DeserializeOwned>(
        result: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    ) -> Result<T, Problem> {
        let mut response = result.map_err(|e| format!("{} didn't answer: {}", SITE, e))?;
        let status = response.status();
        let text = response
            .body_mut()
            .with_config()
            .limit(4 << 20)
            .read_to_string()
            .map_err(|e| format!("{}: {}", SITE, e))?;
        if !status.is_success() {
            #[derive(Deserialize)]
            struct Error {
                error: String,
                #[serde(default)]
                game: Option<String>,
            }
            return Err(match serde_json::from_str::<Error>(&text) {
                Ok(e) => Problem {
                    message: format!("{}: {}", SITE, e.error),
                    game: e.game.filter(|_| status == 409),
                },
                Err(_) => format!("{}: {}", SITE, status).into(),
            });
        }
        Ok(serde_json::from_str(&text).map_err(|e| format!("{}'s answer: {}", SITE, e))?)
    }

    /// The answer's JSON, or the almanac's error.
    fn answer<T: DeserializeOwned>(
        result: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    ) -> Result<T, String> {
        reply(result).map_err(|p| p.message)
    }

    /// The API's address: `API`, or the one `API_VARIABLE` names.
    fn api() -> String {
        std::env::var(API_VARIABLE)
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| API.to_string())
    }

    fn get<T: DeserializeOwned>(path: &str) -> Result<T, String> {
        answer(agent().get(&format!("{}{}", api(), path)).call())
    }

    fn post<T: DeserializeOwned>(path: &str, body: &impl Serialize) -> Result<T, String> {
        post_reply(path, body).map_err(|p| p.message)
    }

    /// `post`, with why the almanac said no.
    fn post_reply<T: DeserializeOwned>(path: &str, body: &impl Serialize) -> Result<T, Problem> {
        let body = serde_json::to_string(body).map_err(|e| e.to_string())?;
        reply(
            agent()
                .post(&format!("{}{}", api(), path))
                .header("Content-Type", "application/json")
                .send(body),
        )
    }

    /// `text` for a URL's query.
    fn escape(text: &str) -> String {
        text.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    (b as char).to_string()
                }
                _ => format!("%{:02X}", b),
            })
            .collect()
    }

    /// The versions whose files `files` matches, best first.
    pub fn lookup(files: &[FileInfo]) -> Result<Vec<Match>, String> {
        if files.is_empty() {
            return Ok(Vec::new());
        }
        post("/games/lookup", &serde_json::json!({ "files": files }))
    }

    /// The games whose names have the words of `query`.
    pub fn search(query: &str) -> Result<Vec<GameSummary>, String> {
        get(&format!("/games?q={}", escape(query.trim())))
    }

    /// The settings recommended for the game `slug`, for its version
    /// `version` if there is one.
    pub fn recommendation(slug: &str, version: Option<i64>) -> Result<Recommendation, String> {
        let version = version.map_or(String::new(), |v| format!("?version={}", v));
        get(&format!("/games/{}/config{}", escape(slug), version))
    }

    /// The games the almanac has for the program's `files`, the ones they
    /// matched first, then those named like `name`, each with its
    /// settings. Without `with_names`, only the matches.
    pub fn candidates(
        files: &[FileInfo],
        name: &str,
        with_names: bool,
    ) -> Result<Vec<Candidate>, String> {
        let mut out: Vec<Candidate> = Vec::new();
        for m in lookup(files)? {
            // A version a name alone matched is only a guess.
            let hash_match = m.hash_matches > 0;
            if !hash_match && !with_names {
                continue;
            }
            if out.iter().any(|c| {
                c.slug == m.game
                    && c.version
                        .as_ref()
                        .is_some_and(|(id, _)| *id == m.version_id)
            }) {
                continue;
            }
            out.push(Candidate {
                slug: m.game,
                name: m.name,
                version: Some((m.version_id, m.version_label)),
                hash_match,
                ratings: Ratings::default(),
                recommendation: None,
            });
        }
        if with_names && !name.trim().is_empty() {
            for game in search(name)?.into_iter().take(20) {
                if out.iter().any(|c| c.slug == game.slug) {
                    continue;
                }
                out.push(Candidate {
                    slug: game.slug,
                    name: game.name,
                    version: None,
                    hash_match: false,
                    ratings: game.rating,
                    recommendation: None,
                });
            }
        }
        out.truncate(25);
        for c in &mut out {
            c.recommendation = recommendation(&c.slug, c.version.as_ref().map(|(id, _)| *id)).ok();
            if let Some(r) = &c.recommendation {
                c.ratings = r.rating.clone();
            }
        }
        Ok(out)
    }

    #[derive(Deserialize)]
    struct Version {
        id: i64,
        #[serde(default)]
        files: Vec<FileInfo>,
    }

    #[derive(Deserialize)]
    struct Game {
        slug: String,
        #[serde(default)]
        versions: Vec<Version>,
    }

    /// The label of the version a report adds for the program's file.
    fn version_label(files: &[FileInfo]) -> String {
        match files.first() {
            Some(f) => format!(
                "{} ({} bytes), reported from Rust-DOS",
                f.name.to_ascii_uppercase(),
                f.size.unwrap_or(0)
            ),
            None => "Reported from Rust-DOS".to_string(),
        }
    }

    /// The version of the game `slug` that has the program's `files`:
    /// one it has, or one added with the program's `platform` and the
    /// profile's `key`, which lets its holder correct it on the almanac's
    /// site. None without files. The game's slug, as the almanac gives it.
    fn version_of(
        slug: &str,
        files: &[FileInfo],
        platform: Option<&str>,
        key: Option<&str>,
    ) -> Result<(String, Option<i64>), String> {
        let game: Game = get(&format!("/games/{}", escape(slug)))?;
        let same = |f: &FileInfo| files.iter().any(|g| g.md5.is_some() && g.md5 == f.md5);
        let known = game
            .versions
            .iter()
            .find(|v| v.files.iter().any(same))
            .map(|v| v.id);
        let version = match (known, new_version(files, platform, key)) {
            (Some(id), _) => Some(id),
            (None, Some(body)) => {
                let path = format!("/games/{}/versions", escape(&game.slug));
                Some(post::<Version>(&path, &body)?.id)
            }
            (None, None) => None,
        };
        Ok((game.slug, version))
    }

    /// A new version for the program's `files`, None without any.
    fn new_version(
        files: &[FileInfo],
        platform: Option<&str>,
        key: Option<&str>,
    ) -> Option<serde_json::Value> {
        (!files.is_empty()).then(|| {
            serde_json::json!({
                "label": version_label(files),
                "platform": platform,
                "files": files,
                "key": key,
            })
        })
    }

    /// Report that the game played `rating` with `settings`, and what its
    /// profile says of it (`about`): to its version of the program's
    /// `files`, which is added, with the program's `platform`, if the game
    /// doesn't have it yet, and the game too if it is new. A new game
    /// whose name the almanac has already is that game. The almanac
    /// replaces the report sent with `about`'s key on the game before.
    pub fn report(
        subject: &Subject,
        files: &[FileInfo],
        platform: Option<&str>,
        rating: Rating,
        settings: &BTreeMap<String, BTreeMap<String, String>>,
        about: &About,
    ) -> Result<Reported, String> {
        let key = about.key.as_deref();
        let mut existing = false;
        let (slug, version) = match subject {
            Subject::Game {
                slug,
                version: Some(v),
            } => (slug.clone(), Some(*v)),
            Subject::Game {
                slug,
                version: None,
            } => version_of(slug, files, platform, key)?,
            Subject::New(name) => {
                let body = serde_json::json!({
                    "name": name.trim(),
                    "year": about.year,
                    "publisher": about.publisher,
                    "key": about.key,
                    "version": new_version(files, platform, key),
                });
                match post_reply::<Game>("/games", &body) {
                    Ok(game) => {
                        let version = game.versions.first().map(|v| v.id);
                        (game.slug, version)
                    }
                    Err(Problem {
                        game: Some(slug), ..
                    }) => {
                        existing = true;
                        version_of(&slug, files, platform, key)?
                    }
                    Err(problem) => return Err(problem.message),
                }
            }
        };
        let body = serde_json::json!({
            "game": slug,
            "version_id": version,
            "rating": rating,
            "settings": settings,
            "rust_dos_version": env!("CARGO_PKG_VERSION"),
            "notes": about.notes,
            "key": about.key,
            "year": about.year,
            "publisher": about.publisher,
        });
        let reported: Reported = post("/reports", &body)?;
        Ok(Reported {
            existing,
            ..reported
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame_cap::Cap;

    #[test]
    fn a_profile_keeps_what_the_report_says_of_its_game() {
        let old = "[game]\nname=Keen\nnotes=Old notes\noverlay=true\n\n[autoexec]\nKEEN4E\n";
        let about = About {
            year: Some(1991),
            publisher: Some("Apogee".into()),
            notes: None,
            key: None,
        };
        let text = about.into_profile(old);
        let kept = About::of_profile(&text);
        assert_eq!(
            (kept.year, kept.publisher.as_deref()),
            (Some(1991), Some("Apogee"))
        );
        // Notes taken out of the report are taken out of the profile.
        assert_eq!(kept.notes, None);
        assert!(!text.contains("notes="), "{}", text);
        // It gets a UUID, which stays as it is the next time.
        let key = kept.key.clone().expect("the profile gets a UUID");
        assert!(crate::games::valid_uuid(&key));
        let again = About {
            notes: Some("Needs EMS".into()),
            ..kept
        }
        .into_profile(&text);
        assert_eq!(About::of_profile(&again).key, Some(key));
        assert_eq!(About::of_profile(&again).notes.as_deref(), Some("Needs EMS"));
        assert!(again.ends_with("\n[autoexec]\nKEEN4E\n"), "{}", again);
    }

    #[test]
    fn programs_are_filed_by_platform() {
        let mut pe = vec![0u8; 0x48];
        pe[..2].copy_from_slice(b"MZ");
        pe[0x3C] = 0x40;
        pe[0x40..0x44].copy_from_slice(b"PE\0\0");
        assert_eq!(platform_of(&pe), "win32");
        // An NE header that doesn't parse runs as DOS, as its stub would.
        let mut ne = pe.clone();
        ne[0x40..0x44].copy_from_slice(b"NE\0\0");
        assert_eq!(platform_of(&ne), "dos");
        assert_eq!(platform_of(b"MZ\x90\0"), "dos");
        assert_eq!(platform_of(&[0xB4, 0x4C, 0xCD, 0x21]), "dos");
    }

    #[test]
    fn an_installer_says_nothing_of_the_games_platform() {
        for name in [
            "setup.exe",
            "install.exe",
            "_setup.exe",
            "instal.exe",
            "_ins0432._mp",
        ] {
            assert!(is_installer(name), "{}", name);
        }
        for name in ["andretti.exe", "setupx.exe", "keen4e.exe", "installer.exe"] {
            assert!(!is_installer(name), "{}", name);
        }
    }

    #[test]
    fn a_recommendation_brings_its_reports_notes() {
        let text = r#"{"source":{"kind":"consensus","reports":2},"settings":[],
            "notes":[{"report_id":2,"version_id":null,"rating":"red",
                      "notes":"Voodoo 3: crashes in the menu","rust_dos_version":"1.8.3",
                      "updated_at":2},
                     {"report_id":1,"version_id":null,"rating":"green",
                      "description":"Needs the CD in D:","notes":"Fine on Voodoo 2",
                      "rust_dos_version":null,"updated_at":1},
                     {"report_id":3,"version_id":null,"rating":"green","notes":"  ",
                      "rust_dos_version":null,"updated_at":0}]}"#;
        let r: Recommendation = serde_json::from_str(text).unwrap();
        assert_eq!(
            r.notes_text(),
            "\u{201C}Voodoo 3: crashes in the menu\u{201D} (red, 1.8.3) \
             \u{201C}Needs the CD in D: Fine on Voodoo 2\u{201D} (green)"
        );
    }

    #[test]
    fn a_file_is_known_by_its_name_size_and_checksums() {
        let f = file_info("C:\\KEEN4\\KEEN4E.EXE", b"hello");
        assert_eq!(f.name, "keen4e.exe");
        assert_eq!(f.size, Some(5));
        assert_eq!(f.crc32.as_deref(), Some("3610a686"));
        assert_eq!(f.md5.as_deref(), Some("5d41402abc4b2a76b9719d911017c592"));
    }

    #[test]
    fn reports_send_only_the_machines_settings() {
        let default = Settings::default();
        let settings = report_settings(&default, default.memory_mb(), None);
        let count: usize = settings.values().map(BTreeMap::len).sum();
        assert_eq!(count, KEYS.len(), "every key has a value: {:?}", settings);
        assert_eq!(settings["emulator"]["memsize"], "16", "the default, though memsize isn't set");
        assert_eq!(settings["emulator"]["fps_cap"], "auto");
        assert!(!settings["emulator"].contains_key("voodoo_fps_cap"));
        assert!(!settings["emulator"].contains_key("scale"));
        let off = Settings {
            fps_cap: Cap::Off,
            ..Settings::default()
        };
        assert_eq!(report_settings(&off, 16, None)["emulator"]["fps_cap"], "off");
        let thirty = Settings {
            fps_cap: Cap::Fps(30),
            ..Settings::default()
        };
        assert_eq!(report_settings(&thirty, 16, None)["emulator"]["fps_cap"], "30");
        assert!(!settings.contains_key("drives"));
        // The size the machine runs with, whatever the settings give now:
        // a Pentium II chosen after a start with the 486's 16 MB.
        let pentium_ii =
            Settings { cpu: Some(crate::cpu::CpuModel::PentiumII), ..Settings::default() };
        assert_eq!(pentium_ii.memory_mb(), 64);
        assert_eq!(report_settings(&pentium_ii, 16, None)["emulator"]["memsize"], "16");
        assert_eq!(report_settings(&pentium_ii, 64, None)["emulator"]["memsize"], "64");
    }

    /// A recommendation that sets the CPU or the display adapter and no
    /// memsize gets the memory that is their default, unless the
    /// configuration sets memsize.
    #[test]
    fn a_recommended_cpu_brings_its_default_memory() {
        use crate::cpu::CpuModel;
        use crate::video::adapter::Adapter;
        let setting = |key: &str, value: &str| Setting {
            section: "emulator".into(),
            key: key.into(),
            value: value.into(),
        };
        let cpu = setting("cpu", "pentium_ii");
        let layered = layer(&Settings::default(), &[&cpu], None);
        assert_eq!(layered.cpu, Some(CpuModel::PentiumII));
        assert_eq!((layered.memsize, layered.memory_mb()), (None, 64));
        let card = setting("machine", "svga_toadforce");
        assert_eq!(layer(&Settings::default(), &[&card], None).memory_mb(), 128);

        // A report of a ToadForce machine whose CPU follows the card sends
        // the Pentium III, which stays unset when it comes back.
        let toadforce = Settings { machine: Adapter::ToadForce, ..Settings::default() };
        let reported = report_settings(&toadforce, toadforce.memory_mb(), None);
        assert_eq!(reported["emulator"]["cpu"], "pentium_iii");
        let sent: Vec<Setting> = reported
            .iter()
            .flat_map(|(section, keys)| {
                keys.iter().map(|(key, value)| Setting {
                    section: section.clone(),
                    key: key.clone(),
                    value: value.clone(),
                })
            })
            .collect();
        let sent: Vec<&Setting> = sent.iter().collect();
        let layered = layer(&Settings::default(), &sent, None);
        assert_eq!((layered.machine, layered.cpu), (Adapter::ToadForce, None));
        assert_eq!(layered.cpu_model(), CpuModel::PentiumIII);
        // A configuration that sets the CPU keeps it set, and a Pentium II
        // recommended for the card is set.
        let base = Settings { cpu: Some(CpuModel::PentiumIII), ..Settings::default() };
        assert_eq!(layer(&base, &sent, None).cpu, Some(CpuModel::PentiumIII));
        let layered = layer(&Settings::default(), &[&card, &cpu], None);
        assert_eq!(layered.cpu, Some(CpuModel::PentiumII));

        // The configuration's memsize stays, and so does the
        // recommendation's own.
        let base = Settings { memsize: Some(16), ..Settings::default() };
        assert_eq!(layer(&base, &[&cpu], None).memory_mb(), 16);
        let memsize = setting("memsize", "32");
        let layered = layer(&Settings::default(), &[&cpu, &memsize], None);
        assert_eq!((layered.memsize, layered.memory_mb()), (Some(32), 32));
    }

    /// A recommended memsize that is the default of the recommended CPU
    /// and display adapter leaves memsize unset; any other is set, below
    /// the default too; and over a memsize the user set, the
    /// recommendation's is set as before.
    #[test]
    fn a_recommended_default_memory_stays_unset() {
        let setting = |key: &str, value: &str| Setting {
            section: "emulator".into(),
            key: key.into(),
            value: value.into(),
        };
        let cpu = setting("cpu", "pentium_ii");
        let unset = Settings::default();
        let layered = layer(&unset, &[&cpu, &setting("memsize", "64")], None);
        assert_eq!((layered.memsize, layered.memory_mb()), (None, 64));
        let card = setting("machine", "svga_toadforce");
        let layered = layer(&unset, &[&card, &setting("memsize", "128")], None);
        assert_eq!((layered.memsize, layered.memory_mb()), (None, 128));
        // The old reports' 16 MB beside a Pentium II: what the game ran with.
        let layered = layer(&unset, &[&cpu, &setting("memsize", "16")], None);
        assert_eq!((layered.memsize, layered.memory_mb()), (Some(16), 16));
        let layered = layer(&unset, &[&cpu, &setting("memsize", "128")], None);
        assert_eq!((layered.memsize, layered.memory_mb()), (Some(128), 128));

        // The user's own memsize is replaced by the recommendation's, which
        // is then set even where it is the default.
        let own = Settings { memsize: Some(32), ..Settings::default() };
        let layered = layer(&own, &[&cpu, &setting("memsize", "64")], None);
        assert_eq!((layered.memsize, layered.memory_mb()), (Some(64), 64));
        let layered = layer(&own, &[&cpu], None);
        assert_eq!((layered.memsize, layered.memory_mb()), (Some(32), 32));
    }

    #[test]
    fn layering_takes_the_machines_settings_and_nothing_else() {
        let setting = |section: &str, key: &str, value: &str| Setting {
            section: section.into(),
            key: key.into(),
            value: value.into(),
        };
        let given = [
            setting("emulator", "memsize", "8"),
            setting("emulator", "ems", "false"),
            setting("emulator", "scale", "4"),
            setting("drives", "c", "/etc"),
            setting("emulator", "cycles", "3000\n[autoexec]\nDEL *.*"),
        ];
        let base = Settings {
            cycles: crate::timer::CpuSpeed::Fixed(20000),
            ..Settings::default()
        };
        let layered = layer(&base, &given.iter().collect::<Vec<_>>(), None);
        assert_eq!(layered.memsize, Some(8));
        assert!(!layered.ems);
        assert_eq!(layered.scale, base.scale);
        assert_eq!(layered.cycles, base.cycles);
    }

    #[test]
    fn a_recommendation_sets_the_frame_rate_cap_under_either_name() {
        let setting = |key: &str, value: &str| Setting {
            section: "emulator".into(),
            key: key.into(),
            value: value.into(),
        };
        let base = Settings {
            fps_cap: Cap::Fps(45),
            ..Settings::default()
        };
        let layered = |given: &[Setting]| layer(&base, &given.iter().collect::<Vec<_>>(), None).fps_cap;
        assert_eq!(layered(&[setting("fps_cap", "60")]), Cap::Fps(60));
        assert_eq!(layered(&[setting("fps_cap", "auto")]), Cap::Auto);
        assert_eq!(layered(&[setting("fps_cap", "off")]), Cap::Off);
        assert_eq!(layered(&[setting("fps_cap", "0")]), Cap::Off);
        assert_eq!(layered(&[setting("voodoo_fps_cap", "30")]), Cap::Fps(30));
        assert_eq!(layered(&[setting("Voodoo_FPS_Cap", "off")]), Cap::Off);
        // What the parser doesn't take leaves the user's own value.
        for bad in ["5", "241", "fast", "-60", "60 fps"] {
            assert_eq!(layered(&[setting("fps_cap", bad)]), Cap::Fps(45), "{}", bad);
        }
        let r = Recommendation {
            source: Source::Consensus { reports: 2 },
            rating: Ratings::default(),
            settings: vec![setting("voodoo_fps_cap", "60")],
            notes: Vec::new(),
        };
        assert_eq!(r.usable().len(), 1, "the old name is the setting's");
    }

    #[test]
    fn layering_takes_the_system_locale_and_drops_an_unknown_one() {
        use crate::config::Win32Locale;
        let locale = |value: &str| Setting {
            section: "emulator".into(),
            key: "win32_locale".into(),
            value: value.into(),
        };
        let base = Settings {
            win32_locale: Win32Locale::PlPl,
            ..Settings::default()
        };
        let russian = locale("ru-RU");
        assert_eq!(layer(&base, &[&russian], None).win32_locale, Win32Locale::RuRu);
        // A locale the built-in Windows doesn't have leaves the base's, as
        // the configuration file's parser warns and keeps it.
        let japanese = locale("ja-JP");
        assert_eq!(layer(&base, &[&japanese], None).win32_locale, Win32Locale::PlPl);
        let reported = report_settings(&base, base.memory_mb(), None);
        assert_eq!(reported["emulator"]["win32_locale"], "pl-PL");
        // Every locale goes through by its name and is reported by it.
        for l in Win32Locale::ALL {
            let layered = layer(&base, &[&locale(l.name())], None);
            assert_eq!(layered.win32_locale, l);
            let reported = report_settings(&layered, layered.memory_mb(), None);
            assert_eq!(reported["emulator"]["win32_locale"], l.name());
        }
    }

    #[test]
    fn blessed_settings_win() {
        let candidate = |slug: &str, hash_match: bool, source: Source| Candidate {
            slug: slug.into(),
            name: slug.into(),
            version: None,
            hash_match,
            ratings: Ratings::default(),
            recommendation: Some(Recommendation {
                source,
                rating: Ratings::default(),
                settings: vec![Setting {
                    section: "emulator".into(),
                    key: "memsize".into(),
                    value: "8".into(),
                }],
                notes: Vec::new(),
            }),
        };
        let list = [
            candidate("named", false, Source::Blessed { blessed_by: None }),
            candidate("voted", true, Source::Consensus { reports: 3 }),
            candidate(
                "blessed",
                true,
                Source::Blessed {
                    blessed_by: Some("mod".into()),
                },
            ),
        ];
        assert_eq!(best(&list).map(|c| c.slug.as_str()), Some("blessed"));
        assert_eq!(best(&list[..2]).map(|c| c.slug.as_str()), Some("voted"));
        assert_eq!(best(&list[..1]), None);
    }

    #[test]
    fn recommendations_read_as_the_almanac_writes_them() {
        let text = r#"{"game":"descent","version_id":null,"scope":"game",
            "source":{"kind":"blessed","report_id":3,"blessed_by":"admin"},
            "rating":{"green":1,"yellow":0,"red":0,"overall":"green"},
            "settings":[{"section":"emulator","key":"cpu","value":"486"},
                        {"section":"sound","key":"sbtype","value":"sb16","agree":1,"voters":1}],
            "ini":"[emulator]\ncpu=486\n"}"#;
        let r: Recommendation = serde_json::from_str(text).unwrap();
        assert_eq!(
            r.source,
            Source::Blessed {
                blessed_by: Some("admin".into())
            }
        );
        assert_eq!(r.usable().len(), 2);
        assert_eq!(r.describe(), "blessed by admin");
        let none: Recommendation =
            serde_json::from_str(r#"{"source":{"kind":"none"},"settings":[]}"#).unwrap();
        assert_eq!(none.source, Source::None);
    }
}
