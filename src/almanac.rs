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
//! never drives, commands or paths.

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
/// machine a game runs on.
pub const KEYS: [(&str, &str); 37] = [
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
    ("emulator", "voodoo"),
    ("emulator", "voodoo_model"),
    ("emulator", "voodoo_memory"),
    ("emulator", "powervr"),
    ("emulator", "composite"),
    ("emulator", "monochrome"),
    ("emulator", "hard_disk_speed"),
    ("emulator", "floppy_disk_speed"),
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

fn known(section: &str, key: &str) -> bool {
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

/// The program a game starts with, as the settings window asks about it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Program {
    /// Its file, a DOS path.
    pub path: String,
    pub files: Vec<FileInfo>,
    /// The name to look the game up by: its profile's, or the program's.
    pub name: String,
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
    Some(Program {
        path: path.to_string(),
        files: vec![file],
        name,
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

/// A game's recommended settings (`GET /games/{slug}/config`).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Recommendation {
    pub source: Source,
    #[serde(default)]
    pub rating: Ratings,
    #[serde(default)]
    pub settings: Vec<Setting>,
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
pub fn report_settings(
    settings: &Settings,
    home: Option<&Path>,
) -> BTreeMap<String, BTreeMap<String, String>> {
    let mut map: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for (section, key, value) in config::values(settings, home) {
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
/// are layered over the configuration's (games.rs's `prepare`).
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
    Settings::from_config(&config::parse(&text, Path::new("/"), home))
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
    rating: Rating,
    settings: &BTreeMap<String, BTreeMap<String, String>>,
) -> Result<i64, String> {
    #[cfg(all(feature = "sdl", not(target_arch = "wasm32")))]
    return client::report(subject, files, rating, settings);
    #[cfg(not(all(feature = "sdl", not(target_arch = "wasm32"))))]
    {
        let _ = (subject, files, rating, settings);
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

    /// The answer's JSON, or the almanac's error.
    fn answer<T: DeserializeOwned>(
        result: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    ) -> Result<T, String> {
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
            struct Problem {
                error: String,
            }
            let why = serde_json::from_str::<Problem>(&text)
                .map_or_else(|_| status.to_string(), |p| p.error);
            return Err(format!("{}: {}", SITE, why));
        }
        serde_json::from_str(&text).map_err(|e| format!("{}'s answer: {}", SITE, e))
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
        let body = serde_json::to_string(body).map_err(|e| e.to_string())?;
        answer(
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

    #[derive(Deserialize)]
    struct Report {
        id: i64,
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

    /// Report that the game played `rating` with `settings`: to its
    /// version of the program's `files`, which is added if the game
    /// doesn't have it yet, and the game too if it is new. Returns the
    /// report's number.
    pub fn report(
        subject: &Subject,
        files: &[FileInfo],
        rating: Rating,
        settings: &BTreeMap<String, BTreeMap<String, String>>,
    ) -> Result<i64, String> {
        let new_version = || {
            (!files.is_empty())
                .then(|| serde_json::json!({ "label": version_label(files), "files": files }))
        };
        let (slug, version) = match subject {
            Subject::Game {
                slug,
                version: Some(v),
            } => (slug.clone(), Some(*v)),
            Subject::Game {
                slug,
                version: None,
            } => {
                // The version with this file, if the game has one; else a new one.
                let game: Game = get(&format!("/games/{}", escape(slug)))?;
                let same = |f: &FileInfo| files.iter().any(|g| g.md5.is_some() && g.md5 == f.md5);
                let known = game
                    .versions
                    .iter()
                    .find(|v| v.files.iter().any(same))
                    .map(|v| v.id);
                let version = match (known, new_version()) {
                    (Some(id), _) => Some(id),
                    (None, Some(body)) => Some(
                        post::<Version>(&format!("/games/{}/versions", escape(slug)), &body)?.id,
                    ),
                    (None, None) => None,
                };
                (game.slug, version)
            }
            Subject::New(name) => {
                let body = serde_json::json!({ "name": name.trim(), "version": new_version() });
                let game: Game = post("/games", &body)?;
                let version = game.versions.first().map(|v| v.id);
                (game.slug, version)
            }
        };
        let body = serde_json::json!({
            "game": slug,
            "version_id": version,
            "rating": rating,
            "settings": settings,
            "rust_dos_version": env!("CARGO_PKG_VERSION"),
        });
        post::<Report>("/reports", &body).map(|r| r.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let settings = report_settings(&Settings::default(), None);
        let count: usize = settings.values().map(BTreeMap::len).sum();
        assert_eq!(count, KEYS.len(), "every key has a value: {:?}", settings);
        assert!(settings["emulator"].contains_key("memsize"));
        assert!(!settings["emulator"].contains_key("scale"));
        assert!(!settings.contains_key("drives"));
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
        assert_eq!(layered.memsize, 8);
        assert!(!layered.ems);
        assert_eq!(layered.scale, base.scale);
        assert_eq!(layered.cycles, base.cycles);
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
