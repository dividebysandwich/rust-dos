//! Game profiles: a game's own settings and the commands that start it, in
//! a configuration file of its own (`games/<id>.conf` beside the
//! configuration file in use; in the browser, the page keeps them). A
//! profile has `[game]` with its `name=`, the settings that differ from the
//! configuration's in their sections, its `[drives]`, and `[autoexec]` with
//! the commands that start it:
//!
//! ```ini
//! [game]
//! name=Commander Keen 4
//!
//! [emulator]
//! cycles=10000
//!
//! [autoexec]
//! C:
//! CD \KEEN4
//! KEEN4E
//! ```
//!
//! Launching a game layers its settings over the configuration's, mounts
//! its drives and runs its commands; once they are done and the prompt is
//! back, the configuration's settings and drives are.

use crate::config::{self, DriveChange, Settings};
use crate::cpu::Cpu;
use crate::mount::MountSpec;
use std::path::Path;

/// A game in the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameEntry {
    /// Its file's name without `.conf`.
    pub id: String,
    pub name: String,
    /// The command that starts it: the last of its `[autoexec]`.
    pub command: String,
}

/// A game to make a profile of (the settings window's dialog): its name,
/// the DOS directory it is in and the command that starts it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NewGame {
    pub name: String,
    pub directory: String,
    pub command: String,
}

/// A profile's name and command, from its text.
pub fn entry(id: &str, text: &str) -> GameEntry {
    let config = config::parse(text, Path::new("/"), None);
    GameEntry {
        id: id.to_string(),
        name: config.game_name.unwrap_or_else(|| id.to_string()),
        command: config.autoexec.last().cloned().unwrap_or_default(),
    }
}

/// A file name for a game called `name`: its letters and digits in lower
/// case, the rest dashes, and a number if `taken` has it.
pub fn slug(name: &str, taken: &[String]) -> String {
    let mut base = String::new();
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            base.push(c.to_ascii_lowercase());
        } else if !base.ends_with('-') && !base.is_empty() {
            base.push('-');
        }
    }
    let mut base: String = base.trim_end_matches('-').chars().take(32).collect();
    if base.is_empty() {
        base = "game".to_string();
    }
    let mut id = base.clone();
    let mut n = 2;
    while taken.iter().any(|t| t.eq_ignore_ascii_case(&id)) {
        id = format!("{}-{}", base, n);
        n += 1;
    }
    id
}

/// The commands that start `new`: to its drive and directory, then the
/// program.
fn commands(new: &NewGame) -> Vec<String> {
    let mut lines = Vec::new();
    let dir = new.directory.trim();
    let path = match dir.split_once(':') {
        Some((drive, path)) if drive.len() == 1 && drive.chars().all(|c| c.is_ascii_alphabetic()) => {
            lines.push(format!("{}:", drive.to_ascii_uppercase()));
            path
        }
        _ => dir,
    };
    let path = path.trim().trim_end_matches('\\');
    if !path.is_empty() {
        let path = if path.starts_with('\\') { path.to_string() } else { format!("\\{}", path) };
        lines.push(format!("CD {}", path));
    }
    if !new.command.trim().is_empty() {
        lines.push(new.command.trim().to_string());
    }
    lines
}

/// The profile of `new`: its name, the settings of `current` that differ
/// from `base` (the configuration's), the drives that differ, and the
/// commands that start it.
pub fn profile_text(
    new: &NewGame,
    base: &Settings,
    current: &Settings,
    drives: &[DriveChange],
    home: Option<&Path>,
) -> Result<String, String> {
    if new.name.trim().is_empty() {
        return Err("The game needs a name".to_string());
    }
    if new.command.trim().is_empty() {
        return Err("The game needs a command that starts it".to_string());
    }
    let mut text = format!("[game]\nname={}\n", new.name.trim());
    let settings = config::update_text("", base, current, drives, home);
    if !settings.is_empty() {
        text.push('\n');
        text.push_str(&settings);
    }
    text.push_str("\n[autoexec]\n");
    for line in commands(new) {
        text.push_str(&line);
        text.push('\n');
    }
    Ok(text)
}

/// A profile ready to launch: the settings it plays with, its drives and
/// the commands that start it.
#[derive(Debug)]
pub struct Prepared {
    pub name: String,
    pub settings: Settings,
    pub drives: Vec<MountSpec>,
    pub autoexec: Vec<String>,
    /// Problems in the profile.
    pub warnings: Vec<String>,
}

/// The profile `text` (of the game `id`) over the settings `base`: what it
/// sets, and the rest as `base` has it. Paths in it are relative to `dir`.
pub fn prepare(id: &str, base: &Settings, text: &str, dir: &Path, home: Option<&Path>) -> Result<Prepared, String> {
    let own = config::parse(text, dir, home);
    if own.autoexec.is_empty() {
        return Err(format!("The profile {} has no commands to start the game ([autoexec])", id));
    }
    // The configuration's settings as a file would have them, then the
    // profile's, which win.
    let layered = format!("{}\n{}", config::update_text("", &Settings::default(), base, &[], home), text);
    let settings = Settings::from_config(&config::parse(&layered, dir, home));
    Ok(Prepared {
        name: own.game_name.clone().unwrap_or_else(|| id.to_string()),
        settings,
        drives: own.drives,
        autoexec: own.autoexec,
        warnings: own.warnings,
    })
}

/// The profiles in the directory `dir`: each one's entry and text, by
/// name.
pub fn list(dir: &Path) -> Vec<(GameEntry, String)> {
    let Ok(read) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut games: Vec<(GameEntry, String)> = read
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let is_conf = path.extension().is_some_and(|x| x.eq_ignore_ascii_case("conf"));
            let id = path.file_stem()?.to_str()?.to_string();
            let text = if is_conf { std::fs::read_to_string(&path).ok()? } else { return None };
            Some((entry(&id, &text), text))
        })
        .collect();
    games.sort_by_key(|(e, _)| e.name.to_lowercase());
    games
}

/// The game `query` names: its id, or its name, in any case.
pub fn find<'a>(games: &'a [GameEntry], query: &str) -> Option<&'a GameEntry> {
    let query = query.trim();
    games
        .iter()
        .find(|g| g.id.eq_ignore_ascii_case(query))
        .or_else(|| games.iter().find(|g| g.name.eq_ignore_ascii_case(query)))
}

/// The DOS directory the prompt is in, where a new game likely is.
pub fn prompt_directory(cpu: &Cpu) -> String {
    let drive = cpu.bus.disk.get_current_drive();
    let dir = cpu.bus.disk.mounted_drives().into_iter().find(|d| d.drive == drive).map(|d| d.current_dir).unwrap_or_default();
    format!("{}:\\{}", crate::disk::drive_letter(drive), dir.trim_start_matches('\\'))
}

/// A game set up for DOSBox, made into a profile in the games folder `dir`:
/// a GOG install or a folder with DOSBox configuration files (`source` a
/// directory), or such a file. Returns the profile's id and the game's name
/// and what of the configuration didn't come across.
pub fn import(dir: &Path, source: &Path, home: Option<&Path>) -> Result<(String, String, Vec<String>), String> {
    // The profile's paths are absolute: it lives in another folder.
    let source = std::fs::canonicalize(source).map_err(|e| format!("{}: {}", source.display(), e))?;
    let source = source.as_path();
    let imported = if source.is_dir() {
        crate::import::gog::import(source, home)?
    } else {
        crate::import::gog::import_conf(source, home)?
    };
    let taken: Vec<String> = list(dir).into_iter().map(|(e, _)| e.id).collect();
    let id = slug(&imported.name, &taken);
    let path = dir.join(format!("{}.conf", id));
    std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::write(&path, imported.profile_text(home)))
        .map_err(|e| format!("cannot write {}: {}", path.display(), e))?;
    Ok((id, imported.name, imported.warnings))
}

/// A game that was launched and hasn't ended.
#[derive(Clone, Debug)]
pub struct ActiveGame {
    pub id: String,
    pub name: String,
    /// The settings before it, which come back when it ends.
    pub base: Settings,
    /// Its settings as its profile has them, which saving compares against.
    pub saved: Settings,
    /// The drives it mounted over, and what they had before (None: no
    /// drive).
    pub replaced: Vec<(u8, Option<MountSpec>)>,
}

impl ActiveGame {
    /// Whether the game's commands have run and the prompt is back.
    pub fn done(&self, cpu: &Cpu) -> bool {
        !cpu.batch.is_active() && cpu.pending_command.is_none() && cpu.shell_wait.is_none() && cpu.shell_idle()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timer::CpuSpeed;
    use std::path::PathBuf;

    fn keen() -> NewGame {
        NewGame { name: "Commander Keen 4".into(), directory: "C:\\KEEN4".into(), command: "KEEN4E".into() }
    }

    #[test]
    fn profile_text_holds_only_the_differences_and_the_commands() {
        let base = Settings::default();
        let current = Settings { cycles: CpuSpeed::Fixed(10000), ems: false, ..Settings::default() };
        let text = profile_text(&keen(), &base, &current, &[], None).unwrap();
        assert_eq!(text, "[game]\nname=Commander Keen 4\n\n[emulator]\ncycles=10000\nems=false\n\n[autoexec]\nC:\nCD \\KEEN4\nKEEN4E\n");
        let config = config::parse(&text, Path::new("/"), None);
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
        assert_eq!(entry("keen4", &text), GameEntry { id: "keen4".into(), name: "Commander Keen 4".into(), command: "KEEN4E".into() });

        // In the root of D:, the same settings.
        let root = NewGame { directory: "d:\\".into(), ..keen() };
        let text = profile_text(&root, &base, &base, &[], None).unwrap();
        assert_eq!(text, "[game]\nname=Commander Keen 4\n\n[autoexec]\nD:\nKEEN4E\n");
        assert!(profile_text(&NewGame { name: " ".into(), ..keen() }, &base, &base, &[], None).is_err());
        assert!(profile_text(&NewGame { command: "".into(), ..keen() }, &base, &base, &[], None).is_err());
    }

    #[test]
    fn a_profile_is_layered_over_the_base() {
        let base = Settings { scale: 3, cycles: CpuSpeed::Fixed(3000), ..Settings::default() };
        let text = "[game]\nname=Stunts\n[emulator]\ncycles=20000\n[drives]\nD=/games/stunts\n[autoexec]\nD:\nSTUNTS\n";
        let prepared = prepare("stunts", &base, text, Path::new("/cfg/games"), None).unwrap();
        assert_eq!(prepared.name, "Stunts");
        assert_eq!(prepared.settings.cycles, CpuSpeed::Fixed(20000));
        assert_eq!(prepared.settings.scale, 3, "the base's settings stay");
        assert_eq!(prepared.drives[0].path, PathBuf::from("/games/stunts"));
        assert_eq!(prepared.autoexec, ["D:", "STUNTS"]);
        assert!(prepare("empty", &base, "[game]\nname=Empty\n", Path::new("/"), None).is_err());
    }

    #[test]
    fn slugs_are_file_names_and_unique() {
        assert_eq!(slug("Commander Keen 4: Secret of the Oracle", &[]), "commander-keen-4-secret-of-the-o");
        assert_eq!(slug("  Stunts  ", &[]), "stunts");
        assert_eq!(slug("Stunts", &["stunts".into(), "stunts-2".into()]), "stunts-3");
        assert_eq!(slug("???", &[]), "game");
    }

    #[test]
    fn games_are_found_by_id_or_name() {
        let games = vec![entry("keen4", "[game]\nname=Commander Keen 4\n[autoexec]\nKEEN4E\n")];
        assert_eq!(find(&games, "KEEN4").map(|g| g.name.as_str()), Some("Commander Keen 4"));
        assert_eq!(find(&games, "commander keen 4").map(|g| g.id.as_str()), Some("keen4"));
        assert!(find(&games, "doom").is_none());
    }

    #[test]
    fn a_game_is_done_when_its_commands_ran_and_the_prompt_is_back() {
        let mut cpu = Cpu::new(PathBuf::from("."));
        cpu.load_shell();
        let game = ActiveGame {
            id: "x".into(),
            name: "X".into(),
            base: Settings::default(),
            saved: Settings::default(),
            replaced: Vec::new(),
        };
        cpu.queue_batch_lines(["X"]);
        assert!(!game.done(&cpu));
        cpu.batch.clear();
        assert!(game.done(&cpu));
    }
}
