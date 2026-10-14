//! The configuration file: finding and reading it, the drives it holds,
//! saving to it, and the games folder beside it.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::args::Args;
use crate::config::{self, Settings};
use crate::cpu::Cpu;
use crate::disk::{DRIVE_SLOTS, DriveKind};
use crate::games;
use crate::mount::{self, MountCmd, MountSpec};
use rust_dos::games::GameEntry;

/// What saving the settings compares against: the settings and drives as
/// the configuration file has them, from startup or the last save. Saving
/// writes what changed since and adds the settings the file has no line
/// for, so the command-line options that weren't changed, drives that
/// failed to mount and the file's own spelling of paths stay as they are.
pub(crate) struct Saved {
    pub(crate) file: Option<PathBuf>,
    /// The `[autoexec]` lines, whose MOUNTs aren't copied to `[drives]`.
    pub(crate) autoexec: Vec<String>,
    pub(crate) settings: Settings,
    pub(crate) drives: BTreeMap<u8, MountSpec>,
}

/// The drives the configuration file can hold, by letter: mounts of host
/// directories and CD images, not the drives held in memory.
pub(crate) fn mounted_drives(cpu: &Cpu) -> BTreeMap<u8, MountSpec> {
    cpu.bus
        .disk
        .all_drives()
        .into_iter()
        .filter(|info| info.kind != DriveKind::Virtual)
        .filter_map(|info| info.mount.map(|spec| (info.drive, spec)))
        .collect()
}

/// The drives the startup commands (`[autoexec]` and C:\AUTOEXEC.BAT)
/// mount with MOUNT or IMGMOUNT, in the configuration file in `config_dir`.
fn startup_mounts(cpu: &Cpu, autoexec: &[String], config_dir: Option<&std::path::Path>) -> Vec<MountSpec> {
    let cwd = std::env::current_dir().unwrap_or_default();
    let home = rust_dos::hostdirs::home_dir();
    let mut lines = autoexec.to_vec();
    if let Some(file) = cpu.bus.disk.file_data("C:\\AUTOEXEC.BAT")
        && let Ok(bytes) = file.read()
    {
        lines.extend(String::from_utf8_lossy(&bytes).lines().map(str::to_string));
    }
    let disk = &cpu.bus.disk;
    let locate = |path: &str| disk.resolve_path(path);
    let paths = mount::PathContext { base: &cwd, config_dir, home: home.as_deref(), locate: &locate };
    lines
        .iter()
        .filter_map(|line| {
            let (command, args) = line.trim().trim_start_matches('@').split_once(char::is_whitespace)?;
            if !command.eq_ignore_ascii_case("MOUNT") && !command.eq_ignore_ascii_case("IMGMOUNT") {
                return None;
            }
            match mount::parse_mount_command(args, &paths) {
                Ok(MountCmd::Mount(spec)) => Some(spec),
                _ => None,
            }
        })
        .collect()
}

/// The drives that changed since the configuration file was read or last
/// saved, but for those the startup commands mount.
pub(crate) fn drive_changes(cpu: &Cpu, saved: &Saved) -> Vec<config::DriveChange> {
    let current = mounted_drives(cpu);
    let startup = startup_mounts(cpu, &saved.autoexec, config_dir(saved.file.as_deref()).as_deref());
    let same_place = |a: &MountSpec, b: &MountSpec| {
        a.drive == b.drive && std::fs::canonicalize(&a.path).ok() == std::fs::canonicalize(&b.path).ok()
    };
    let mut changes = Vec::new();
    for drive in 0..DRIVE_SLOTS {
        let (before, now) = (saved.drives.get(&drive), current.get(&drive));
        match now {
            _ if before == now => {}
            None => changes.push((drive, None)),
            // A startup command mounts it again anyway.
            Some(spec) if before.is_none() && startup.iter().any(|s| same_place(s, spec)) => {}
            Some(spec) => changes.push((drive, Some(spec.clone()))),
        }
    }
    changes
}

/// Save every setting, and the drives that changed since the file was read
/// or last saved.
pub(crate) fn save_config(cpu: &mut Cpu, saved: &mut Saved, settings: &Settings) -> Result<(), String> {
    let Some(path) = saved.file.clone() else {
        return Err("No configuration file".to_string());
    };
    let changes = drive_changes(cpu, saved);
    let home = rust_dos::hostdirs::home_dir();
    config::save(&path, &saved.settings, settings, &changes, home.as_deref(), config::Saving::All)?;
    cpu.bus.log_string(&format!("[CONFIG] Saved the settings to {}", path.display()));
    saved.settings = settings.clone();
    saved.drives = mounted_drives(cpu);
    Ok(())
}

/// The folder of the game profiles: `games` beside the configuration
/// file `config`.
pub(crate) fn games_dir(config: Option<&std::path::Path>) -> Option<PathBuf> {
    config?.parent().map(|dir| dir.join("games"))
}

/// The folder of a configuration file, which MOUNT -pr takes relative paths
/// from.
pub(crate) fn config_dir(config: Option<&std::path::Path>) -> Option<PathBuf> {
    std::path::absolute(config?).ok()?.parent().map(std::path::Path::to_path_buf)
}

/// The game profile `query` names: its entry, its text and its folder.
pub(crate) fn find_game(config: Option<&std::path::Path>, query: &str) -> Result<(GameEntry, String, PathBuf), String> {
    let dir = games_dir(config).ok_or("--game needs a configuration file, beside which the games folder is")?;
    let games = games::list(&dir);
    let entries: Vec<GameEntry> = games.iter().map(|(e, _)| e.clone()).collect();
    let entry = games::find(&entries, query)
        .ok_or_else(|| format!("No game called {} in {}", query, dir.display()))?
        .clone();
    let text = games.into_iter().find(|(e, _)| e.id == entry.id).map(|(_, t)| t).unwrap_or_default();
    Ok((entry, text, dir))
}

/// A configuration problem: shown on the terminal and written to the log.
pub(crate) fn config_warning(cpu: &mut Cpu, msg: &str) {
    eprintln!("[CONFIG] Warning: {}", msg);
    cpu.bus.log_string(&format!("[CONFIG] Warning: {}", msg));
}

/// The note in the box above the first DOS prompt: the configuration
/// file in use.
pub(crate) fn banner_notes(config: &config::Config, no_config: bool) -> Vec<(String, String)> {
    let file = match &config.source {
        Some(path) => {
            let path = mount::display_host_path(&std::path::absolute(path).unwrap_or_else(|_| path.clone()));
            if config.created { format!("{} (new)", path) } else { path }
        }
        None if no_config => "none (--no-config)".to_string(),
        None => "none".to_string(),
    };
    vec![("Config file: ".to_string(), file)]
}

/// Find and parse the configuration file (see config.rs for the lookup
/// order). Problems in the file are reported but never stop the emulator;
/// only a missing `--config` file does. The file used goes to the log once
/// there is one (see `create_cpu`).
pub(crate) fn load_config(args: &Args) -> Result<config::Config, String> {
    if args.no_config {
        return Ok(config::Config::default());
    }
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let config = config::load(
        args.config.as_deref(),
        &cwd,
        config::exe_dir().as_deref(),
        config::default_path(),
        rust_dos::hostdirs::home_dir().as_deref(),
    )?;
    if config.created
        && let Some(path) = &config.source
    {
        eprintln!("[CONFIG] Created default configuration file {}", path.display());
    }
    for warning in &config.warnings {
        eprintln!("[CONFIG] Warning: {}", warning);
    }
    Ok(config)
}
