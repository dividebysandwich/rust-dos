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
//!
//! `[game]`'s `achievements=` says which version of the game it is for
//! RetroAchievements: the hash of its archive, or the archive (a zip or a
//! .dosz, relative to the games folder).
//!
//! `[game]`'s `overlay=true`, which new profiles have, leaves the game's
//! own drives (its `[drives]`' host directories and disk images) as they
//! are: what the game changes on them goes to `saves/<id>/<drive letter>`
//! (a disk image's to a delta file there, `diskdelta`) beside the
//! games folder (`saves_dir`, `overlay_drives`), and deleting that
//! (`reset`) takes the game back to how it was installed.

use crate::config::{self, DriveChange, Settings};
use crate::cpu::Cpu;
use crate::hostfs;
use crate::mount::MountSpec;
use std::path::{Path, PathBuf};

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
    let mut text = format!("[game]\nname={}\noverlay=true\n", new.name.trim());
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
    /// What RetroAchievements knows the game by (`achievements=`).
    pub achievements: Option<String>,
    /// Its drives' changes go to its saves folder (`overlay=`,
    /// `overlay_drives`).
    pub overlay: bool,
    /// The keys pressed as it starts (`input=`, autoinput.rs).
    pub input: Option<String>,
    /// Its gamepad mapping's lines (`[gamepad]`, padmap.rs).
    pub pad: Vec<(String, String)>,
    /// Whether its package has launch configurations to choose from
    /// (`variants=`).
    pub variants: bool,
    /// Problems in the profile.
    pub warnings: Vec<String>,
    /// Its saves folder, held while it plays (`overlay_drives`).
    pub saves_lock: Option<SavesLock>,
}

/// A game's saves folder held by the one rust-dos that plays it: two
/// playing it at once would each write its changes over the other's.
/// The system lets go of it when the game ends, or the program does.
#[derive(Clone, Debug)]
pub struct SavesLock(#[allow(dead_code)] std::sync::Arc<std::fs::File>);

/// Hold the saves folder `saves` (`SavesLock`), or say why not: another
/// rust-dos plays the game. Where locks can't be had (a frontend's
/// storage, a file system without them), the folder isn't held.
fn lock_saves(saves: &Path) -> Result<Option<SavesLock>, String> {
    if hostfs::is_foreign(saves) || std::fs::create_dir_all(saves).is_err() {
        return Ok(None);
    }
    let Ok(file) = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(saves.join(".lock")) else {
        return Ok(None);
    };
    match file.try_lock() {
        Ok(()) => Ok(Some(SavesLock(std::sync::Arc::new(file)))),
        Err(std::fs::TryLockError::WouldBlock) => {
            Err(format!("the game is being played already, by another Rust-DOS, which writes its changes in {}", saves.display()))
        }
        Err(std::fs::TryLockError::Error(_)) => Ok(None),
    }
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
        achievements: own.game_achievements,
        overlay: own.game_overlay,
        input: own.game_input,
        pad: own.game_pad,
        variants: own.game_variants,
        warnings: own.warnings,
        saves_lock: None,
    })
}

/// The folder of the games' saves, `saves` beside the games folder
/// `games`: a folder for each game, with one for each drive in it.
pub fn saves_dir(games: &Path) -> PathBuf {
    games.parent().unwrap_or(games).join("saves")
}

/// The folder for its changes in the game's saves (`saves`/`id`) for each
/// of the profile's own drives that is an archive or in one, and with `overlay=`
/// each that is a host directory or disk image, unless it has one or is
/// read-only. The saves are held while the game plays (`SavesLock`): an
/// error if another rust-dos plays it.
pub fn overlay_drives(prepared: &mut Prepared, id: &str, saves: &Path) -> Result<(), String> {
    let saves = saves.join(id);
    let mut overlaid = false;
    for spec in &mut prepared.drives {
        let opts = &mut spec.opts;
        // An archive, or a folder or image in one (`game.dosz/automount/c.vhd`).
        let archive = (crate::archive::is_archive_name(&spec.path) && hostfs::is_file(&spec.path))
            || (!hostfs::exists(&spec.path) && crate::archive::split(&spec.path).is_some());
        let folder = prepared.overlay && hostfs::is_dir(&spec.path);
        // A disk image's changes go to a delta file there (`diskdelta`).
        let image = prepared.overlay && !archive && hostfs::is_file(&spec.path);
        if opts.overlay.is_some() || opts.read_only || opts.kind == crate::disk::DriveKind::CdRom || !(archive || folder || image) {
            continue;
        }
        opts.overlay = Some(saves.join(crate::disk::drive_key(spec.drive)));
        overlaid = true;
    }
    if overlaid {
        prepared.saves_lock = lock_saves(&saves)?;
    }
    Ok(())
}

/// Take the game `id` back to how it was installed: its folder in the
/// saves folder `saves` goes.
pub fn reset(saves: &Path, id: &str) -> Result<(), String> {
    let saves = saves.join(id);
    match hostfs::remove_dir_all(&saves) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("{}: {}", saves.display(), e)),
        _ => Ok(()),
    }
}

/// The profile `text` with `achievements=value` in its `[game]` section,
/// in place of one there was.
pub fn set_achievements(text: &str, value: &str) -> String {
    set_game_key(text, "achievements", value, false)
}

/// The profile `text` with another `manual=value` line in its `[game]`
/// section.
pub fn add_manual(text: &str, value: &str) -> String {
    set_game_key(text, "manual", value, true)
}

/// The profile `text` with `key=value` in its `[game]` section: in place
/// of one there was, or (`add`) after the others.
fn set_game_key(text: &str, key: &str, value: &str, add: bool) -> String {
    let line = format!("{}={}", key, value);
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let header = |l: &str| l.trim().starts_with('[');
    let Some(start) = lines.iter().position(|l| l.trim().eq_ignore_ascii_case("[game]")) else {
        return format!("[game]\n{}\n\n{}", line, text);
    };
    let end = lines[start + 1..].iter().position(|l| header(l)).map_or(lines.len(), |p| start + 1 + p);
    let key_of = |l: &str| l.split_once('=').map(|(k, _)| k.trim().to_ascii_lowercase());
    match (start + 1..end).find(|&i| key_of(&lines[i]).as_deref() == Some(key)) {
        Some(i) if !add => lines[i] = line,
        _ => {
            // After the section's last setting.
            let at = (start + 1..end).rev().find(|&i| key_of(&lines[i]).is_some()).map_or(start + 1, |i| i + 1);
            lines.insert(at, line);
        }
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// Make `path` one of the manuals of the game `id`, whose profile is in
/// the games folder `dir`: a `manual=` line in it, relative to the folder
/// where it is in it.
pub fn add_manual_file(dir: &Path, id: &str, path: &Path, home: Option<&Path>) -> Result<(), String> {
    let file = dir.join(format!("{}.conf", id));
    let text = hostfs::read_to_string(&file).map_err(|e| format!("{}: {}", file.display(), e))?;
    let value = match path.strip_prefix(dir) {
        Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
        Err(_) => crate::mount::contract_home(path, home),
    };
    hostfs::write(&file, add_manual(&text, &value)).map_err(|e| format!("{}: {}", file.display(), e))
}

/// The folder of the game `id`'s extras, beside its profile in the games
/// folder `dir`: the documents and pictures in it are its manuals.
pub fn extras_dir(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{}.extras", id))
}

/// The manuals and extras of the game `id`, whose profile `text` is in
/// the games folder `dir`: its `manual=` lines, then the files of its
/// extras folder, by name, then those in the `EXTRAS` folders of its
/// drives' packages (`package_extras`).
pub fn manuals(dir: &Path, id: &str, text: &str, home: Option<&Path>) -> Vec<crate::manuals::Manual> {
    use crate::manuals::{Manual, is_manual_name, title_of};
    let config = config::parse(text, dir, home);
    let mut manuals: Vec<Manual> = config.game_manuals.iter().map(|v| Manual::parse(v, dir, home)).collect();
    let mut extras: Vec<PathBuf> = hostfs::read_dir(extras_dir(dir, id))
        .into_iter()
        .flatten()
        .filter(|e| !e.is_dir && is_manual_name(&e.path))
        .map(|e| e.path)
        .collect();
    extras.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    for path in extras {
        if !manuals.iter().any(|m| m.path == path) {
            manuals.push(Manual { title: title_of(&path), path });
        }
    }
    // The EXTRAS folders of its drives: of the folders and archives, and
    // of the archives the drives are in.
    for spec in &config.drives {
        let root = crate::archive::split(&spec.path).map_or_else(|| spec.path.clone(), |(archive, _)| archive);
        for manual in package_extras(&root) {
            if !manuals.iter().any(|m| m.path == manual.path) {
                manuals.push(manual);
            }
        }
    }
    manuals
}

/// The hash RetroAchievements knows a game by, from its profile's
/// `achievements=`: the hash itself, or the archive's, found from `dir`
/// (the games folder).
pub fn achievements_hash(value: &str, dir: &Path, home: Option<&Path>) -> Result<String, String> {
    let value = value.trim();
    if crate::achievements::hash::is_hash(value) {
        return Ok(value.to_ascii_lowercase());
    }
    crate::achievements::hash::hash_archive(&crate::mount::expand_host_path(value, dir, home))
}

/// The profiles in the directory `dir`: each one's entry and text, by
/// name.
pub fn list(dir: &Path) -> Vec<(GameEntry, String)> {
    let Ok(read) = hostfs::read_dir(dir) else { return Vec::new() };
    let mut games: Vec<(GameEntry, String)> = read
        .into_iter()
        .filter_map(|e| {
            let path = e.path;
            let is_conf = path.extension().is_some_and(|x| x.eq_ignore_ascii_case("conf"));
            let id = path.file_stem()?.to_str()?.to_string();
            let text = if is_conf { hostfs::read_to_string(&path).ok()? } else { return None };
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
/// directory), or such a file; or a game's package (`add_package`). Returns the profile's id and the game's name
/// and what of the configuration didn't come across.
pub fn import(dir: &Path, source: &Path, home: Option<&Path>) -> Result<(String, String, Vec<String>), String> {
    // A .dosc: the game it goes with.
    if source.extension().is_some_and(|e| e.eq_ignore_ascii_case("dosc")) {
        let package = crate::archive::dosz_of(source)
            .ok_or_else(|| format!("{}: the game it goes with isn't beside it", source.display()))?;
        return add_package(dir, &package);
    }
    // A game's package: its profile, as dropping it makes it.
    if is_package(source) || (crate::archive::is_archive_name(source) && hostfs::is_file(source)) {
        return add_package(dir, source);
    }
    // The profile's paths are absolute: it lives in another folder.
    let source = hostfs::canonicalize(source).map_err(|e| format!("{}: {}", source.display(), e))?;
    let source = source.as_path();
    let imported = if hostfs::is_dir(source) {
        crate::import::gog::import(source, home)?
    } else {
        crate::import::gog::import_conf(source, home)?
    };
    let taken: Vec<String> = list(dir).into_iter().map(|(e, _)| e.id).collect();
    let id = slug(&imported.name, &taken);
    let path = dir.join(format!("{}.conf", id));
    hostfs::create_dir_all(dir)
        .and_then(|()| hostfs::write(&path, imported.profile_text(home)))
        .map_err(|e| format!("cannot write {}: {}", path.display(), e))?;
    Ok((id, imported.name, imported.warnings))
}

/// The name of a package's own configuration, at its root.
pub const PACKAGE_CONF: &str = "rust-dos.conf";

/// The folder of a package's manuals and extras, at its root.
pub const PACKAGE_EXTRAS: &str = "EXTRAS";

/// The name of the file or folder at the root of the package `package` (a
/// folder, or a zip or 7z archive) that is `name` in any case.
fn package_entry(package: &Path, name: &str) -> Option<String> {
    let names: Vec<String> = if hostfs::is_dir(package) {
        hostfs::read_dir(package).ok()?.into_iter().map(|e| e.name.to_string_lossy().into_owned()).collect()
    } else {
        let mut names: Vec<String> = crate::archive::open(package).ok()?.files();
        // A folder of it, by the files in it.
        for file in names.clone() {
            if let Some((top, _)) = file.split_once('/') {
                names.push(top.to_string());
            }
        }
        names.into_iter().filter(|n| !n.contains('/')).collect()
    };
    names.into_iter().find(|n| n.eq_ignore_ascii_case(name))
}

/// A file of the package `package` (a folder, or a path into an archive).
fn read_package_file(path: &Path) -> Result<String, String> {
    let data = match hostfs::read(path) {
        Ok(data) => data,
        Err(e) => crate::archive::read_member(path).unwrap_or_else(|| Err(format!("{}: {}", path.display(), e)))?,
    };
    Ok(String::from_utf8_lossy(&data).into_owned())
}

/// The DOSBox configuration that goes with the package `package`, which
/// has no `rust-dos.conf`: its `dosbox.conf` at its root, or a file named
/// as it is beside it (`GAME.conf` for `GAME.zip`), as .dosz packages load
/// them.
fn package_dosbox_conf(package: &Path) -> Option<String> {
    if let Some(name) = package_entry(package, "dosbox.conf") {
        return read_package_file(&package.join(name)).ok();
    }
    let stem = package.file_stem()?.to_string_lossy().to_ascii_lowercase();
    let dir = package.parent()?;
    let beside = hostfs::read_dir(dir).ok()?.into_iter().find(|e| {
        let name = e.name.to_string_lossy().to_ascii_lowercase();
        !e.is_dir && name.strip_suffix(".conf") == Some(stem.as_str())
    })?;
    // Not a profile of rust-dos's that happens to be there. Those beside
    // .dosz packages often have only an [autoexec], which runs on the
    // package as C:.
    let text = read_package_file(&beside.path).ok()?;
    (crate::import::dosbox::is_dosbox_conf(&text) || !config::has_own_sections(&text)).then_some(text)
}

/// The manuals and extras in the package `root` (a folder, or a zip or
/// 7z archive): the documents and pictures in its `EXTRAS` folder, in
/// any case, by name.
pub fn package_extras(root: &Path) -> Vec<crate::manuals::Manual> {
    use crate::manuals::{Manual, is_manual_name, title_of};
    let Some(extras) = package_entry(root, PACKAGE_EXTRAS) else { return Vec::new() };
    let mut files: Vec<PathBuf> = if hostfs::is_dir(root) {
        hostfs::read_dir(root.join(&extras)).into_iter().flatten().filter(|e| !e.is_dir).map(|e| e.path).collect()
    } else {
        let prefix = format!("{}/", extras);
        crate::archive::open(root)
            .map(|a| a.files())
            .unwrap_or_default()
            .into_iter()
            .filter(|f| f.starts_with(&prefix) && !f[prefix.len()..].contains('/'))
            .map(|f| root.join(f))
            .collect()
    };
    files.retain(|p| is_manual_name(p));
    files.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    files.into_iter().map(|path| Manual { title: title_of(&path), path }).collect()
}

/// `text` without the sections `names` (in lower case).
fn without_sections(text: &str, names: &[&str]) -> String {
    let mut out = String::new();
    let mut skip = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            skip = names.contains(&trimmed[1..trimmed.len() - 1].trim().to_ascii_lowercase().as_str());
        }
        if !skip {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// The value of `key` in `[game]`, if `text` has it.
fn game_value(text: &str, key: &str) -> Option<String> {
    let mut in_game = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_game = line.eq_ignore_ascii_case("[game]");
        } else if in_game
            && let Some((k, v)) = line.split_once('=')
            && k.trim().eq_ignore_ascii_case(key)
        {
            return Some(v.trim().to_string());
        }
    }
    None
}

/// A game's package as a profile in the games folder `dir`: a zip or 7z
/// archive, or a folder, with the game's files and maybe its own
/// `rust-dos.conf` and `EXTRAS` folder at its root (GAME-PACKAGES.md). The
/// package is C:, its changes kept apart (`overlay=true`). Its
/// configuration's settings, drives, manuals and commands go in the
/// profile, its paths taken from the package's root; without commands, the
/// one program there is to start the game (`archive::start_program`)
/// starts, or the prompt is left on C:. Without a `rust-dos.conf`, a
/// DOSBox configuration of the package's (`package_dosbox_conf`) is
/// imported into the profile instead. A profile made for it before is
/// the one. Returns the profile's id and name, and what of a DOSBox
/// configuration didn't come across.
pub fn add_package(dir: &Path, package: &Path) -> Result<(String, String, Vec<String>), String> {
    add_package_in(dir, package, None)
}

/// `add_package`, the game running in the operating system `os` (as
/// `[rust-dos] os=` has it) when its package doesn't name one.
pub fn add_package_in(dir: &Path, package: &Path, os: Option<&str>) -> Result<(String, String, Vec<String>), String> {
    let package = hostfs::canonicalize(package).map_err(|e| format!("{}: {}", package.display(), e))?;
    // Without Windows' `\\?\`, as the profile has it, or it isn't found again.
    let package = PathBuf::from(crate::mount::display_host_path(&package));
    let profiles = list(dir);
    let is_archive = !hostfs::is_dir(&package);
    let source = package_source(&package, is_archive, os);
    let made_before = profiles.iter().find(|(_, text)| {
        // On C:, or where its configuration's REMOUNT moved it.
        config::parse(text, dir, None).drives.iter().any(|d| d.path.starts_with(&package))
    });
    // Made again when what it was made from has changed: a configuration
    // edited, or a .dosc put beside it, since.
    let remade = match made_before {
        Some((entry, text)) if config::parse(text, dir, None).game_source.as_deref() == Some(source.as_str()) => {
            return Ok((entry.id.clone(), entry.name.clone(), Vec::new()));
        }
        Some((entry, _)) => Some(entry.id.clone()),
        None => None,
    };
    let (text, name, warnings) = package_profile(&package, &source, None, os)?;
    // Its profile made before, or neither a profile nor a folder there
    // already.
    let id = remade.unwrap_or_else(|| {
        let taken: Vec<String> = profiles
            .into_iter()
            .map(|(e, _)| e.id)
            .chain(hostfs::read_dir(dir).into_iter().flatten().map(|e| e.name.to_string_lossy().to_lowercase()))
            .collect();
        slug(&name, &taken)
    });
    let path = dir.join(format!("{}.conf", id));
    hostfs::create_dir_all(dir)
        .and_then(|()| hostfs::write(&path, text))
        .map_err(|e| format!("cannot write {}: {}", path.display(), e))?;
    Ok((id, name, warnings))
}

/// The profile of the package at `package` (its path as the profiles
/// have it), made from `source` (`package_source`), with its .dosc's
/// launch configuration `variant` over it, or the default, in the
/// operating system `os` if it names none: its text, the game's name, and
/// what of its configuration didn't come across. The drives of its
/// `automount` folder are the profile's too (`import::dosbox::arrange`).
fn package_profile(package: &Path, source: &str, variant: Option<&str>, os: Option<&str>) -> Result<(String, String, Vec<String>), String> {
    let package = package.to_path_buf();
    let is_archive = !hostfs::is_dir(&package);
    let variants = if is_archive { crate::archive::variants(&package) } else { Vec::new() };
    let files: Vec<String> = if is_archive {
        crate::archive::open_variant(&package, variant)?.files()
    } else {
        hostfs::read_dir(&package)
            .map_err(|e| format!("{}: {}", package.display(), e))?
            .into_iter()
            .filter(|e| !e.is_dir)
            .map(|e| e.name.to_string_lossy().into_owned())
            .collect()
    };
    let own = match package_entry(&package, PACKAGE_CONF) {
        Some(name) => read_package_file(&package.join(name))?,
        None => String::new(),
    };
    // Its paths are from the package's root.
    let mut conf = config::parse(&own, &package, None);
    let stem = package.file_stem().map_or("Game".to_string(), |n| n.to_string_lossy().into_owned());
    let name = conf.game_name.clone().unwrap_or(stem);
    let name = match variant {
        Some(v) => format!("{} ({})", name, crate::archive::Variants::shown(v)),
        None => name,
    };
    // Its paths are from the package, or from the folder beside it, where
    // a .conf of its name is.
    let bases: Vec<PathBuf> = std::iter::once(package.clone()).chain(package.parent().map(Path::to_path_buf)).collect();
    // A package made for DOSBox, or a .dosz: its DOS.YML (in it
    // or its .dosc), then its DOSBox configuration's settings, drives and
    // commands over those.
    let yml = if own.is_empty() && is_archive { crate::archive::dos_yml(&package, variant) } else { Vec::new() };
    let yml = (!yml.is_empty()).then(|| crate::import::dos_yml::import(&yml, &package, &name));
    let os = conf.game_os.clone().or_else(|| os.map(str::to_string));
    let dosbox_conf = if own.is_empty() { package_dosbox_conf(&package) } else { None }.map(|mut text| {
        if let Some(os) = os.as_ref().filter(|_| crate::import::dosbox::conf_os(&text).is_none()) {
            text.push_str(&format!("\n[rust-dos]\nos={}\n", os));
        }
        crate::import::dosbox::import_in(&[&text], &bases, &name, None, Some(&package))
    });
    // A DOSBox configuration's drives come with the automount folder's.
    let arranged = dosbox_conf.is_some();
    let dosbox = match (yml, dosbox_conf) {
        (Some(mut yml), Some(over)) => {
            yml.merge(over);
            Some(yml)
        }
        (yml, over) => over.or(yml),
    };
    if let Some(imported) = &dosbox {
        conf.drives = imported.drives.clone();
        // Only C: from a configuration that runs nothing itself.
        if imported.autoexec.iter().any(|l| !l.eq_ignore_ascii_case("C:")) {
            conf.autoexec = imported.autoexec.clone();
        }
    }
    let mut warnings = dosbox.as_ref().map(|d| d.warnings.clone()).unwrap_or_default();
    if !arranged {
        // The package on C:, and the automount folder's drives and the
        // system around it; the one program there is runs on the
        // package's drive.
        let mut setup = crate::import::Imported { drives: conf.drives.clone(), autoexec: conf.autoexec.clone(), ..Default::default() };
        if !setup.drives.iter().any(|d| d.drive == crate::disk::DRIVE_C || d.path == package) {
            setup.drives.insert(0, MountSpec { drive: crate::disk::DRIVE_C, path: package.clone(), opts: Default::default() });
        }
        let scan = crate::automount::scan(&package);
        crate::import::dosbox::arrange(&mut setup, Some(&package), &scan, os.as_deref());
        if conf.autoexec.is_empty() && !setup.autoexec.iter().any(|l| l.starts_with("BOOT")) {
            let lettered = |d: &&MountSpec| d.drive < crate::disk::LASTDRIVE;
            setup.autoexec = match setup.drives.iter().filter(lettered).find(|d| d.path == package) {
                Some(d) => std::iter::once(format!("{}:", crate::disk::drive_letter(d.drive))).chain(crate::archive::start_program(&files)).collect(),
                None if setup.drives.iter().filter(lettered).any(|d| d.drive == crate::disk::DRIVE_C) => vec!["C:".to_string()],
                None => Vec::new(),
            };
        }
        conf.drives = setup.drives;
        conf.autoexec = setup.autoexec;
        warnings.extend(setup.warnings);
    }
    let mut text = format!("[game]\nname={}\nsource={}\n", name, source);
    text.push_str(&format!("overlay={}\n", game_value(&own, "overlay").unwrap_or_else(|| "true".to_string())));
    // RetroAchievements knows the game by its archive's hash.
    match game_value(&own, "achievements") {
        Some(value) => text.push_str(&format!("achievements={}\n", value)),
        None if is_archive => {
            if let Ok(hash) = crate::achievements::hash::hash_archive(&package) {
                text.push_str(&format!("achievements={}\n", hash));
            }
        }
        None => {}
    }
    if !variants.is_empty() && variant.is_none() {
        text.push_str("variants=true\n");
    }
    if let Some(os) = &os {
        text.push_str(&format!("os={}\n", os));
    }
    if let Some(input) = dosbox.as_ref().and_then(|d| d.input.as_ref()).or(conf.game_input.as_ref()) {
        text.push_str(&format!("input={}\n", input));
    }
    for value in &conf.game_manuals {
        let manual = crate::manuals::Manual::parse(value, &package, None);
        text.push_str(&format!("manual={}|{}\n", manual.path.display(), manual.title));
    }
    let settings = match &dosbox {
        Some(imported) => imported.settings_text(),
        None => without_sections(&own, &["game", "gamepad", "drives", "autoexec"]),
    };
    if !settings.trim().is_empty() {
        text.push('\n');
        text.push_str(settings.trim_end());
        text.push('\n');
    }
    let mut drives = conf.drives.clone();
    // The package's drive with the launch configuration's files over it.
    for spec in drives.iter_mut().filter(|d| d.path == package) {
        spec.opts.variant = variant.map(str::to_string);
    }
    let pad = dosbox.as_ref().map(|d| d.pad.clone()).filter(|p| !p.is_empty()).unwrap_or_else(|| conf.game_pad.clone());
    if !pad.is_empty() {
        text.push_str("\n[gamepad]\n");
        for (key, value) in &pad {
            text.push_str(&format!("{}={}\n", key, value));
        }
    }
    text.push_str("\n[drives]\n");
    for spec in &drives {
        text.push_str(&format!("{}={}\n", crate::disk::drive_key(spec.drive), crate::mount::mount_spec_value(spec, None)));
    }
    text.push_str("\n[autoexec]\n");
    let autoexec = if conf.autoexec.is_empty() {
        // The program runs on the package's C:, not on the automount
        // folder's.
        let on_c = drives.iter().any(|d| d.drive == crate::disk::DRIVE_C && d.path == package);
        let program = crate::archive::start_program(&files).filter(|_| on_c);
        std::iter::once("C:".to_string()).chain(program).collect()
    } else {
        conf.autoexec.clone()
    };
    for line in autoexec {
        text.push_str(&line);
        text.push('\n');
    }
    Ok((text, name, warnings))
}

/// The ways a game can start, from its package's launch configurations.
#[derive(Clone, Debug)]
pub struct LaunchChoices {
    /// The game's name.
    pub name: String,
    pub variants: crate::archive::Variants,
    /// The launch configurations that are tools (`run_utility`), after
    /// which the choice is offered again.
    pub tools: Vec<String>,
}

/// The package of a game's profile (in the games folder `dir`) that has
/// launch configurations.
fn variant_package(own: &config::Config) -> Option<&Path> {
    own.drives
        .iter()
        .map(|d| d.path.as_path())
        .find(|p| crate::archive::is_archive_name(p) && hostfs::is_file(p) && !crate::archive::variants(p).is_empty())
}

/// The ways the game of the profile `text` can start, if its package has
/// launch configurations (`variants=`).
pub fn launch_choices(dir: &Path, text: &str) -> Option<LaunchChoices> {
    let own = config::parse(text, dir, None);
    if !own.game_variants {
        return None;
    }
    let package = variant_package(&own)?;
    let dirs = crate::archive::variants(package);
    let tools = dirs
        .iter()
        .filter(|v| {
            let yml = crate::archive::dos_yml(package, Some(v));
            let utility = |text: &String| {
                text.lines().any(|l| {
                    let (k, v) = l.split_once(':').unwrap_or((l, ""));
                    k.trim().eq_ignore_ascii_case("run_utility") && v.trim().eq_ignore_ascii_case("true")
                })
            };
            yml.last().is_some_and(utility)
        })
        .cloned()
        .collect();
    let name = own.game_name.clone().unwrap_or_default();
    Some(LaunchChoices { name, variants: crate::archive::Variants::new(&dirs), tools })
}

/// The profile `text` of a package's game (made by `add_package`, in the
/// games folder `dir`) with its .dosc's launch configuration `variant`
/// over it, or None when its package has no such thing.
pub fn variant_profile(dir: &Path, text: &str, variant: &str) -> Result<String, String> {
    let own = config::parse(text, dir, None);
    let package = variant_package(&own).ok_or("the game's package has no launch configurations")?;
    let source = own.game_source.clone().unwrap_or_default();
    Ok(package_profile(package, &source, Some(variant), own.game_os.as_deref())?.0)
}

/// How packages are imported: a profile made by an older import is made
/// again, with what the newer one reads (DOS.YML's keys, say).
const IMPORT_VERSION: u32 = 4;

/// What a package's profile is made from, fingerprinted: its own
/// rust-dos.conf, a DOSBox configuration in it or beside it, its DOS.YML
/// files (its .dosc's too), what its automount folder holds, and the
/// operating system `os` it is to run in if it names none. A profile with
/// another `source` is made again.
fn package_source(package: &Path, is_archive: bool, os: Option<&str>) -> String {
    use sha2::{Digest, Sha256};
    let own = package_entry(package, PACKAGE_CONF).and_then(|name| read_package_file(&package.join(name)).ok());
    let mut hasher = Sha256::new();
    hasher.update(IMPORT_VERSION.to_le_bytes());
    for part in [own.clone(), package_dosbox_conf(package), Some(crate::automount::listing(package)), os.map(str::to_string)] {
        hasher.update(part.unwrap_or_default().as_bytes());
        hasher.update([0]);
    }
    if own.is_none() && is_archive {
        // A .dosc keeps the archive's root, DOS.YML or not.
        hasher.update([crate::archive::dosc_of(package).is_some() as u8]);
        // Its launch configurations, with theirs.
        let variants = crate::archive::variants(package);
        for variant in std::iter::once(None).chain(variants.iter().map(|v| Some(v.as_str()))) {
            hasher.update(variant.unwrap_or("").as_bytes());
            for yml in crate::archive::dos_yml(package, variant) {
                hasher.update(yml.as_bytes());
                hasher.update([0]);
            }
        }
    }
    hasher.finalize()[..12].iter().map(|b| format!("{:02x}", b)).collect()
}

/// Whether `path` is a game's package with its own configuration: a
/// folder or an archive with a `rust-dos.conf` at its root.
pub fn is_package(path: &Path) -> bool {
    package_entry(path, PACKAGE_CONF).is_some()
}

/// Whether the package says how it runs: its rust-dos.conf, or a DOSBox
/// configuration in it or beside it.
pub fn package_has_conf(package: &Path) -> bool {
    is_package(package) || package_dosbox_conf(package).is_some()
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
    /// What each drive had before it (`drives_before`), to put back what
    /// it, its commands or its player changed: SUBST and REMOUNT too.
    pub replaced: Vec<(u8, Option<MountSpec>)>,
    /// `Cpu::programs_loaded` as it was launched.
    pub programs_before: u64,
    /// The keys it presses once its program has started (`input=`), until
    /// the front end takes them (`take_input`).
    pub input: Option<Vec<crate::autoinput::Step>>,
    /// Its gamepad mapping (`[gamepad]`), which the first pad plays with.
    pub pad: Option<crate::padmap::PadMapping>,
    /// It is a tool of the game's (its setup program, say): when it ends,
    /// the ways to start the game are offered again.
    pub choose_after: bool,
    /// Its saves folder, held while it plays.
    pub saves_lock: Option<SavesLock>,
}

/// What each drive has mounted (None: nothing, or a drive held in
/// memory), as a game starts.
pub fn drives_before(cpu: &Cpu) -> Vec<(u8, Option<MountSpec>)> {
    (0..crate::disk::DRIVE_SLOTS).map(|d| (d, cpu.bus.disk.drive_info(d).and_then(|i| i.mount))).collect()
}

/// The drives as they were (`drives_before`) again, where they changed.
/// Returns what couldn't be put back.
pub fn restore_drives(cpu: &mut Cpu, before: Vec<(u8, Option<MountSpec>)>) -> Vec<String> {
    let mut errors = Vec::new();
    for (drive, spec) in before.into_iter().rev() {
        let info = cpu.bus.disk.drive_info(drive);
        let now = info.as_ref().and_then(|i| i.mount.clone());
        if now == spec {
            continue;
        }
        let result = match spec {
            Some(spec) => cpu.bus.mount_drive(drive, &spec.path, spec.opts, true).map(|_| ()),
            // Drives held in memory (the Ultrasound's patches) are the
            // settings', which are back already.
            None if info.is_some_and(|i| i.kind == crate::disk::DriveKind::Virtual) => continue,
            None => cpu.bus.unmount_drive(drive),
        };
        if let Err(e) = result {
            errors.push(format!("drive {}: {}", crate::disk::drive_key(drive), e));
        }
    }
    errors
}

impl ActiveGame {
    /// The keys to press, once the game's program has started.
    pub fn take_input(&mut self, cpu: &Cpu) -> Option<crate::autoinput::AutoInput> {
        if cpu.programs_loaded > self.programs_before { self.input.take().map(crate::autoinput::AutoInput::new) } else { None }
    }

    /// The keys a profile presses (`Prepared::input`), and what in them
    /// couldn't be read.
    pub fn input_steps(input: Option<&str>) -> (Option<Vec<crate::autoinput::Step>>, Vec<String>) {
        match input {
            Some(text) => {
                let (steps, warnings) = crate::autoinput::parse(text);
                ((!steps.is_empty()).then_some(steps), warnings)
            }
            None => (None, Vec::new()),
        }
    }

    /// Whether the game's commands have run, a program among them, and the
    /// prompt is back. A game whose commands only leave it at the prompt
    /// (no program to start) stays with its drives until another is
    /// launched.
    pub fn done(&self, cpu: &Cpu) -> bool {
        cpu.programs_loaded > self.programs_before
            && !cpu.batch.is_active()
            && cpu.pending_command.is_none()
            && cpu.shell_wait.is_none()
            && cpu.shell_idle()
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
        assert_eq!(text, "[game]\nname=Commander Keen 4\noverlay=true\n\n[emulator]\ncycles=10000\nems=false\n\n[autoexec]\nC:\nCD \\KEEN4\nKEEN4E\n");
        let config = config::parse(&text, Path::new("/"), None);
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
        assert_eq!(entry("keen4", &text), GameEntry { id: "keen4".into(), name: "Commander Keen 4".into(), command: "KEEN4E".into() });

        // In the root of D:, the same settings.
        let root = NewGame { directory: "d:\\".into(), ..keen() };
        let text = profile_text(&root, &base, &base, &[], None).unwrap();
        assert_eq!(text, "[game]\nname=Commander Keen 4\noverlay=true\n\n[autoexec]\nD:\nKEEN4E\n");
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
    fn a_profile_gets_the_hash_retroachievements_knows_it_by() {
        let text = "[game]\nname=Keen\n\n[autoexec]\nKEEN4E\n";
        let with = set_achievements(text, "abc");
        assert_eq!(with, "[game]\nname=Keen\nachievements=abc\n\n[autoexec]\nKEEN4E\n");
        assert_eq!(set_achievements(&with, "def"), "[game]\nname=Keen\nachievements=def\n\n[autoexec]\nKEEN4E\n");
        assert_eq!(set_achievements("[autoexec]\nX\n", "abc"), "[game]\nachievements=abc\n\n[autoexec]\nX\n");
        let two = add_manual(&add_manual(text, "a.pdf"), "b.png|Map");
        assert_eq!(two, "[game]\nname=Keen\nmanual=a.pdf\nmanual=b.png|Map\n\n[autoexec]\nKEEN4E\n");
        let prepared = prepare("keen", &Settings::default(), &with, Path::new("/"), None).unwrap();
        assert_eq!(prepared.achievements.as_deref(), Some("abc"));
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

    /// A package without a configuration: its automount folder's drives,
    /// and the system it is to run in, as the Boot OS core option asks.
    #[test]
    fn a_package_s_automount_folder_and_system_go_in_its_profile() {
        let dir = std::path::PathBuf::from("target/test_games_automount");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("os")).unwrap();
        let archive = dir.join("Quest.dosz");
        let data = crate::archive::zip::tests::zip(&[("QUEST.EXE", b"MZ", false), ("automount/d.iso", b"", false), ("automount/d1.iso", b"", false)]);
        std::fs::write(&archive, data).unwrap();
        let games = dir.join("games");
        let (id, _, warnings) = add_package(&games, &archive).unwrap();
        assert!(warnings.is_empty(), "{:?}", warnings);
        let text = std::fs::read_to_string(games.join(format!("{}.conf", id))).unwrap();
        let prepared = prepare(&id, &Settings::default(), &text, &games, None).unwrap();
        let archive = std::fs::canonicalize(&archive).unwrap();
        let d = prepared.drives.iter().find(|d| d.drive == 3).unwrap();
        assert_eq!(d.path, archive.join("automount/d.iso"));
        assert_eq!(d.opts.more_images, [archive.join("automount/d1.iso")]);
        assert_eq!(prepared.autoexec, ["C:", "QUEST.EXE"]);
        // In a system of files, which is C:; the package is the letter
        // after the disc's.
        std::fs::create_dir_all(dir.join("os/AutoDos")).unwrap();
        crate::os_images::add_search_dir(std::fs::canonicalize(dir.join("os")).unwrap());
        let (again, _, _) = add_package_in(&games, &archive, Some("autodos")).unwrap();
        assert_eq!(again, id, "made again under its id");
        let text = std::fs::read_to_string(games.join(format!("{}.conf", id))).unwrap();
        assert!(text.contains("\nos=autodos\n"), "{}", text);
        let prepared = prepare(&id, &Settings::default(), &text, &games, None).unwrap();
        assert_eq!(prepared.drives.iter().find(|d| d.drive == 4).unwrap().path, archive);
        assert_eq!(prepared.autoexec, ["E:", "QUEST.EXE"]);
        // One rust-dos at a time plays it.
        let mut first = prepare(&id, &Settings::default(), &text, &games, None).unwrap();
        overlay_drives(&mut first, &id, &saves_dir(&games)).unwrap();
        assert!(first.saves_lock.is_some());
        let mut second = prepare(&id, &Settings::default(), &text, &games, None).unwrap();
        assert!(overlay_drives(&mut second, &id, &saves_dir(&games)).unwrap_err().contains("played already"));
        drop(first);
        overlay_drives(&mut second, &id, &saves_dir(&games)).unwrap();
    }

    #[test]
    fn a_zipped_game_gets_a_profile_with_the_archive_as_c() {
        let dir = std::path::PathBuf::from("target/test_games_archive");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let archive = dir.join("Commander Keen.zip");
        let data = crate::archive::zip::tests::zip(&[("KEEN/KEEN4E.EXE", b"MZ", true), ("KEEN/SETUP.EXE", b"MZ", false)]);
        std::fs::write(&archive, data).unwrap();
        let games = dir.join("games");
        let (id, name, _) = add_package(&games, &archive).unwrap();
        assert_eq!((id.as_str(), name.as_str()), ("commander-keen", "Commander Keen"));
        let text = std::fs::read_to_string(games.join("commander-keen.conf")).unwrap();
        let prepared = prepare(&id, &Settings::default(), &text, &games, None).unwrap();
        // RetroAchievements knows it by the archive's hash.
        let hash = crate::achievements::hash::hash_archive(&archive).unwrap();
        assert_eq!(prepared.achievements.as_deref(), Some(hash.as_str()));
        assert_eq!(achievements_hash(&hash.to_uppercase(), &games, None).unwrap(), hash);
        assert_eq!(achievements_hash("../Commander Keen.zip", &games, None).unwrap(), hash);
        assert!(prepared.warnings.is_empty(), "{:?}", prepared.warnings);
        let archive = std::fs::canonicalize(&archive).unwrap();
        assert_eq!(prepared.drives[0].path, archive, "C: is the archive");
        assert_eq!(prepared.autoexec, ["C:", "KEEN4E.EXE"]);
        // Its changes go to its saves.
        let mut prepared = prepared;
        assert!(prepared.overlay);
        overlay_drives(&mut prepared, &id, &saves_dir(&games)).unwrap();
        let saves = dir.join("saves/commander-keen");
        assert_eq!(prepared.drives[0].opts.overlay.as_deref(), Some(saves.join("C").as_path()));
        std::fs::create_dir_all(saves.join("C")).unwrap();
        reset(&saves_dir(&games), &id).unwrap();
        assert!(!saves.exists());
        reset(&saves_dir(&games), &id).unwrap();
        // Again: the same profile.
        assert_eq!(add_package(&games, &dir.join("Commander Keen.zip")).unwrap().0, "commander-keen");
        assert_eq!(list(&games).len(), 1);
    }

    #[test]
    fn a_package_made_for_dosbox_gets_its_settings_and_commands() {
        let dir = std::path::PathBuf::from("target/test_games_dosbox_package");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let conf: &[u8] = b"[sdl]\nfullscreen=true\n[cpu]\ncycles=fixed 8000\n[sblaster]\nsbtype=sbpro1\nirq=4\n[autoexec]\nmount c game\nc:\nloadfix game.exe\nexit\n";
        let data = crate::archive::zip::tests::zip(&[("DOSBOX.CONF", conf, false), ("GAME/GAME.EXE", b"MZ", false), ("GAME/SETUP.EXE", b"MZ", false)]);
        std::fs::write(dir.join("Game.zip"), data).unwrap();
        let games = dir.join("games");
        let (id, name, warnings) = add_package(&games, &dir.join("Game.zip")).unwrap();
        assert_eq!(name, "Game");
        assert!(warnings.iter().any(|w| w.starts_with("irq=4")), "{:?}", warnings);
        let text = std::fs::read_to_string(games.join(format!("{}.conf", id))).unwrap();
        let prepared = prepare(&id, &Settings::default(), &text, &games, None).unwrap();
        assert!(prepared.warnings.is_empty(), "{:?}\n{}", prepared.warnings, text);
        assert_eq!(prepared.settings.cycles, crate::timer::CpuSpeed::Fixed(8000));
        assert_eq!(prepared.settings.sound.sb.model, crate::sb::SbModel::SbPro2);
        let archive = std::fs::canonicalize(dir.join("Game.zip")).unwrap();
        assert_eq!(prepared.drives.len(), 1);
        assert_eq!(prepared.drives[0].path, archive.join("game"));
        assert_eq!(prepared.autoexec, ["c:", "game.exe"]);
        assert!(prepared.overlay);

        // GAME.conf beside GAME.zip, with C: the zip.
        let data = crate::archive::zip::tests::zip(&[("RUN.BAT", b"", false), ("PLAY.EXE", b"MZ", false)]);
        std::fs::write(dir.join("Other.zip"), data).unwrap();
        std::fs::write(dir.join("other.conf"), "[dosbox]\nmachine=ega\n[autoexec]\nrun.bat\n").unwrap();
        let (id, _, _) = add_package(&games, &dir.join("Other.zip")).unwrap();
        let text = std::fs::read_to_string(games.join(format!("{}.conf", id))).unwrap();
        let prepared = prepare(&id, &Settings::default(), &text, &games, None).unwrap();
        assert_eq!(prepared.drives[0].path, std::fs::canonicalize(dir.join("Other.zip")).unwrap());
        assert_eq!(prepared.autoexec, ["C:", "run.bat"]);
        assert!(text.contains("machine=ega"), "{}", text);

        // One with only an [autoexec].
        std::fs::write(dir.join("Plain.zip"), crate::archive::zip::tests::zip(&[("A.EXE", b"MZ", false), ("B.EXE", b"MZ", false)])).unwrap();
        std::fs::write(dir.join("Plain.conf"), "[autoexec]\n@echo off\nb.exe\n").unwrap();
        let (id, _, _) = add_package(&games, &dir.join("Plain.zip")).unwrap();
        let text = std::fs::read_to_string(games.join(format!("{}.conf", id))).unwrap();
        let prepared = prepare(&id, &Settings::default(), &text, &games, None).unwrap();
        assert_eq!(prepared.autoexec, ["C:", "@echo off", "b.exe"], "{}", text);
        // Not a rust-dos profile that happens to have the name.
        std::fs::write(dir.join("Mine.zip"), crate::archive::zip::tests::zip(&[("A.EXE", b"MZ", false), ("B.EXE", b"MZ", false)])).unwrap();
        std::fs::write(dir.join("Mine.conf"), "[game]\nname=Mine\n[autoexec]\nb.exe\n").unwrap();
        assert!(!package_has_conf(&dir.join("Mine.zip")));

        // Edited, it is made again under its id; left alone, it stays as
        // the player changed it.
        let plain = games.join(format!("{}.conf", id));
        std::fs::write(dir.join("Plain.conf"), "[autoexec]\na.exe\n").unwrap();
        assert_eq!(add_package(&games, &dir.join("Plain.zip")).unwrap().0, id);
        let text = std::fs::read_to_string(&plain).unwrap();
        assert!(text.contains("\na.exe\n") && !text.contains("b.exe"), "{}", text);
        std::fs::write(&plain, text.replace("[game]\n", "[game]\n\n[emulator]\ncycles=1234\n\n[game]\n")).unwrap();
        assert_eq!(add_package(&games, &dir.join("Plain.zip")).unwrap().0, id);
        assert!(std::fs::read_to_string(&plain).unwrap().contains("cycles=1234"));
        // One made before profiles said what they were made from is made again.
        let text = std::fs::read_to_string(&plain).unwrap();
        let old: String = text.lines().filter(|l| !l.starts_with("source=")).map(|l| format!("{}\n", l)).collect();
        std::fs::write(&plain, old).unwrap();
        add_package(&games, &dir.join("Plain.zip")).unwrap();
        assert!(!std::fs::read_to_string(&plain).unwrap().contains("cycles=1234"));
        assert_eq!(list(&games).iter().filter(|(e, _)| e.name == "Plain").count(), 1);

        // One with launch configurations: the default's profile says so, and
        // each is made from it on its own.
        std::fs::write(dir.join("Tyr.dosz"), crate::archive::zip::tests::zip(&[("TYR.EXE", b"MZ", false)])).unwrap();
        let changes = crate::archive::zip::tests::zip(&[
            ("DOS.YML", b"run_path: C:\\TYR.EXE\r\n", false),
            ("[Setup Program]/DOS.YML", b"run_path: C:\\SETUP.EXE\r\nrun_utility: true\r\n", false),
            ("[Setup Program]/SETUP.EXE", b"MZ", false),
        ]);
        std::fs::write(dir.join("Tyr.dosc"), changes).unwrap();
        let (id, _, _) = add_package(&games, &dir.join("Tyr.dosz")).unwrap();
        let text = std::fs::read_to_string(games.join(format!("{}.conf", id))).unwrap();
        assert!(prepare(&id, &Settings::default(), &text, &games, None).unwrap().variants, "{}", text);
        let setup = variant_profile(&games, &text, "Setup Program").unwrap();
        let prepared = prepare(&id, &Settings::default(), &setup, &games, None).unwrap();
        assert_eq!(prepared.name, "Tyr (Setup Program)");
        assert_eq!(prepared.autoexec, ["C:", "CD \\", "SETUP.EXE"]);
        assert_eq!(prepared.drives[0].opts.variant.as_deref(), Some("Setup Program"));
        assert!(!prepared.variants);
        let choices = launch_choices(&games, &text).unwrap();
        assert_eq!(choices.tools, ["Setup Program"]);
        assert_eq!(choices.variants.entries.len(), 1);

        // One that moves the zip to D: and boots Windows from an image beside it.
        std::fs::write(dir.join("Win.dosz"), crate::archive::zip::tests::zip(&[("GAME.EXE", b"MZ", false)])).unwrap();
        std::fs::write(dir.join("win98.img"), b"").unwrap();
        std::fs::write(dir.join("Win.conf"), "[dosbox]\nmachine=svga_s3\n[autoexec]\nremount c d\nimgmount c win98.img\nboot c:\n").unwrap();
        let (id, _, warnings) = add_package(&games, &dir.join("Win.dosz")).unwrap();
        assert!(warnings.is_empty(), "{:?}", warnings);
        let text = std::fs::read_to_string(games.join(format!("{}.conf", id))).unwrap();
        let prepared = prepare(&id, &Settings::default(), &text, &games, None).unwrap();
        let drive = |letter| prepared.drives.iter().find(|d| d.drive == letter).map(|d| d.path.clone());
        assert_eq!(drive(3), Some(std::fs::canonicalize(dir.join("Win.dosz")).unwrap()), "{}", text);
        assert_eq!(drive(crate::disk::DRIVE_C), Some(std::fs::canonicalize(dir.join("win98.img")).unwrap()), "{}", text);
        assert_eq!(prepared.autoexec, ["boot -l C"]);
        // Found again on D:.
        assert_eq!(add_package(&games, &dir.join("Win.dosz")).unwrap().0, id);
    }

    #[test]
    fn a_games_manuals_are_its_lines_and_its_extras() {
        let games = std::path::PathBuf::from("target/test_games_manuals");
        let _ = std::fs::remove_dir_all(&games);
        std::fs::create_dir_all(games.join("keen.extras")).unwrap();
        for name in ["Map.png", "code wheel.JPG", "notes.txt"] {
            std::fs::write(games.join("keen.extras").join(name), b"").unwrap();
        }
        let text = "[game]\nname=Keen\nmanual=keen/Manual.pdf|The manual\n[autoexec]\nKEEN4E\n";
        let manuals = manuals(&games, "keen", text, None);
        let shown: Vec<(&str, &Path)> = manuals.iter().map(|m| (m.title.as_str(), m.path.as_path())).collect();
        assert_eq!(
            shown,
            [
                ("The manual", games.join("keen/Manual.pdf").as_path()),
                ("code wheel", games.join("keen.extras/code wheel.JPG").as_path()),
                ("Map", games.join("keen.extras/Map.png").as_path()),
            ]
        );
    }

    #[test]
    fn a_package_brings_its_settings_drives_and_manuals() {
        let dir = std::path::PathBuf::from("target/test_games_package");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let conf = "[game]\nname=Pool of Radiance\nmanual=DOCS/Rules.pdf|Rule book\n\n[emulator]\ncycles=8000\n\n\
                    [drives]\nD=CD cdrom\n\n[autoexec]\nC:\nSTART\n";
        let data = crate::archive::zip::tests::zip(&[
            ("POOL/rust-dos.conf", conf.as_bytes(), false),
            ("POOL/START.EXE", b"MZ", false),
            ("POOL/CD/DATA.DAT", b"cd", false),
            ("POOL/DOCS/Rules.pdf", b"%PDF", false),
            ("POOL/Extras/Code Wheel.png", b"", false),
            ("POOL/Extras/notes.txt", b"", false),
        ]);
        let package = dir.join("pool.zip");
        std::fs::write(&package, data).unwrap();
        let games = dir.join("games");
        let (id, name, _) = add_package(&games, &package).unwrap();
        assert_eq!((id.as_str(), name.as_str()), ("pool-of-radiance", "Pool of Radiance"));
        let text = std::fs::read_to_string(games.join("pool-of-radiance.conf")).unwrap();
        let prepared = prepare(&id, &Settings::default(), &text, &games, None).unwrap();
        assert!(prepared.warnings.is_empty(), "{:?}\n{}", prepared.warnings, text);
        let package = std::fs::canonicalize(&package).unwrap();
        let drives: Vec<(u8, PathBuf)> = prepared.drives.iter().map(|d| (d.drive, d.path.clone())).collect();
        assert_eq!(drives, [(2, package.clone()), (3, package.join("CD"))]);
        assert_eq!(prepared.drives[1].opts.kind, crate::disk::DriveKind::CdRom);
        assert_eq!(prepared.settings.cycles, crate::config::Settings::from_config(&config::parse("[emulator]\ncycles=8000\n", &dir, None)).cycles);
        assert_eq!(prepared.autoexec, ["C:", "START"]);
        assert!(prepared.overlay);
        let shown: Vec<(String, PathBuf)> = manuals(&games, &id, &text, None).into_iter().map(|m| (m.title, m.path)).collect();
        assert_eq!(
            shown,
            [("Rule book".to_string(), package.join("DOCS/Rules.pdf")), ("Code Wheel".to_string(), package.join("Extras/Code Wheel.png"))]
        );
        assert_eq!(crate::archive::read_member(&shown[0].1), Some(Ok(b"%PDF".to_vec())));
        // Again: the same profile.
        assert_eq!(add_package(&games, &package).unwrap().0, id);

        // A folder: the same, without its conf's own overlay=false.
        let folder = dir.join("Keen");
        std::fs::create_dir_all(folder.join("extras")).unwrap();
        std::fs::write(folder.join("RUST-DOS.CONF"), "[game]\noverlay=false\n").unwrap();
        std::fs::write(folder.join("KEEN4E.EXE"), b"MZ").unwrap();
        std::fs::write(folder.join("extras/Hint Book.pdf"), b"%PDF").unwrap();
        assert!(is_package(&folder) && !is_package(&dir.join("games")));
        let (id, _, _) = add_package(&games, &folder).unwrap();
        let text = std::fs::read_to_string(games.join(format!("{}.conf", id))).unwrap();
        let prepared = prepare(&id, &Settings::default(), &text, &games, None).unwrap();
        assert_eq!((prepared.overlay, prepared.autoexec.clone()), (false, vec!["C:".to_string(), "KEEN4E.EXE".to_string()]));
        let titles: Vec<String> = manuals(&games, &id, &text, None).into_iter().map(|m| m.title).collect();
        assert_eq!(titles, ["Hint Book"]);
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
            programs_before: cpu.programs_loaded,
            input: None,
            pad: None,
            choose_after: false,
            saves_lock: None,
        };
        cpu.queue_batch_lines(["X"]);
        assert!(!game.done(&cpu));
        cpu.batch.clear();
        assert!(!game.done(&cpu), "it ran no program: it stays at the prompt");
        cpu.programs_loaded += 1;
        assert!(game.done(&cpu));
    }
}
