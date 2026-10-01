//! What the core does with the content it is given: which folder or image
//! each drive has, which settings go over rust-dos.conf's, and what runs
//! at the prompt. And the settings themselves, in layers: the built-in
//! defaults, rust-dos.conf, the core options, then the content's own.

use rust_dos::hostfs as fs;
use std::path::{Path, PathBuf};

use rust_dos::config::{self, Settings};
use rust_dos::disk::{DRIVE_C, DriveKind, MountOptions, numbered_drive};
use rust_dos::diskimage::{self, ImageKind};
use rust_dos::games;
use rust_dos::import::drop::{DropAction, drop_action};
use rust_dos::mount::MountSpec;

use crate::options::Values;

/// The folders the core keeps its files in.
#[derive(Clone, Debug)]
pub struct Dirs {
    /// The frontend's system directory, and the core's folder in it:
    /// rust-dos.conf, SoundFonts and MT-32 ROMs.
    pub system: PathBuf,
    pub sys: PathBuf,
    /// The core's folder in the frontend's save directory: the unpacked and
    /// imported games, their saves, and the empty C: of the machine without
    /// content.
    pub data: PathBuf,
}

impl Dirs {
    pub fn new(system: PathBuf, save: PathBuf) -> Self {
        Self { sys: system.join("rust-dos"), system, data: save.join("rust-dos") }
    }

    /// The game profiles, and the folders of unpacked archives.
    pub fn games(&self) -> PathBuf {
        self.data.join("games")
    }

    /// Profiles of games set up for DOSBox, each in a folder named after
    /// the content, so loading it again finds it.
    fn imports(&self) -> PathBuf {
        self.data.join("imported")
    }

    /// The games' changes, a folder for each (`games::overlay_drives`).
    pub fn saves(&self) -> PathBuf {
        self.data.join("saves")
    }

    /// The machine's C: when the content has none.
    pub fn drive_c(&self) -> PathBuf {
        self.data.join("drive_c")
    }
}

/// rust-dos.conf: in the core's folder in the system directory, or in the
/// system directory itself. The settings window saves into it.
#[derive(Clone, Debug)]
pub struct Base {
    pub file: PathBuf,
    pub text: String,
}

impl Base {
    pub fn load(dirs: &Dirs) -> Self {
        let candidates = [dirs.sys.join("rust-dos.conf"), dirs.system.join("rust-dos.conf")];
        for file in &candidates {
            if let Ok(text) = fs::read_to_string(file) {
                return Self { file: file.clone(), text };
            }
        }
        Self { file: candidates[0].clone(), text: String::new() }
    }

    /// The folder relative paths in the file are relative to.
    pub fn dir(&self) -> &Path {
        self.file.parent().unwrap_or(Path::new("."))
    }

    pub fn config(&self) -> config::Config {
        config::parse(&self.text, self.dir(), home().as_deref())
    }
}

/// A rust-dos.conf beside the content, without commands of its own: its
/// settings and drives go over the others'.
#[derive(Clone, Debug)]
pub struct Overlay {
    pub text: String,
    pub dir: PathBuf,
}

/// A game profile (games.rs) the content is, or comes with: launched at
/// the prompt, as the Games page does.
#[derive(Clone, Debug)]
pub struct Profile {
    pub id: String,
    pub text: String,
    /// The folder its relative paths are relative to.
    pub dir: PathBuf,
    /// Its file, which the settings window saves into.
    pub file: PathBuf,
}

/// What the machine starts with.
#[derive(Clone, Debug, Default)]
pub struct Plan {
    /// C:'s folder, unless a mount replaces it.
    pub c_root: PathBuf,
    /// Drives to mount over the configuration's.
    pub mounts: Vec<MountSpec>,
    pub overlay: Option<Overlay>,
    pub profile: Option<Profile>,
    /// Commands for the prompt after the startup commands.
    pub commands: Vec<String>,
    /// The drive whose disks the frontend's disk control changes.
    pub disk_drive: Option<u8>,
    /// What happened, for the log.
    pub notes: Vec<String>,
}

pub fn home() -> Option<PathBuf> {
    rust_dos::hostdirs::home_dir()
}

/// The settings: rust-dos.conf's, the core options' over them, and the
/// overlay's over those. A game profile goes over these as it is launched.
pub fn settings(base: &Base, options: &Values, overlay: Option<&Overlay>) -> (Settings, Vec<String>) {
    let home = home();
    let text = format!("{}\n{}", base.text, options.config_text());
    let parsed = config::parse(&text, base.dir(), home.as_deref());
    let mut warnings: Vec<String> = parsed.warnings.iter().map(|w| format!("rust-dos.conf: {}", w)).collect();
    let mut settings = Settings::from_config(&parsed);
    if let Some(overlay) = overlay {
        let layered = format!(
            "{}\n{}",
            config::update_text("", &Settings::default(), &settings, &[], home.as_deref()),
            overlay.text
        );
        let parsed = config::parse(&layered, &overlay.dir, home.as_deref());
        warnings.extend(parsed.warnings.iter().map(|w| format!("{}: {}", overlay.dir.join("rust-dos.conf").display(), w)));
        settings = Settings::from_config(&parsed);
    }
    (frontend_settings(settings), warnings)
}

/// What the frontend does in the core's place: it draws the picture and
/// rewinds, and RetroAchievements is its own.
pub fn frontend_settings(mut settings: Settings) -> Settings {
    settings.voodoo.renderer = rust_dos::voodoo::Renderer::Software;
    settings.rewind = false;
    settings.achievements.enabled = false;
    settings
}

/// The drives the overlay mounts, and the commands of its `[autoexec]`.
pub fn overlay_drives(overlay: &Overlay) -> Vec<MountSpec> {
    config::parse(&overlay.text, &overlay.dir, home().as_deref()).drives
}

/// Whether `text` is a DOSBox configuration rather than rust-dos's.
fn is_dosbox_conf(text: &str) -> bool {
    text.lines().map(str::trim).any(|line| {
        let line = line.to_ascii_lowercase();
        ["[dosbox]", "[cpu]", "[sdl]", "[render]", "[sblaster]", "[dos]", "[speaker]", "[midi]"].contains(&line.as_str())
    })
}

/// Whether configuration text has commands to start a game.
fn has_autoexec(text: &str) -> bool {
    !config::autoexec_lines(text).iter().all(|l| l.trim().is_empty() || l.trim().starts_with('#'))
}

/// The rust-dos.conf in `dir`, as a profile if it starts a game, else as
/// an overlay.
fn folder_conf(dir: &Path, plan: &mut Plan) -> Result<bool, String> {
    let file = dir.join("rust-dos.conf");
    let Ok(text) = fs::read_to_string(&file) else { return Ok(false) };
    let name = dir.file_name().map_or("game".to_string(), |n| n.to_string_lossy().into_owned());
    if has_autoexec(&text) {
        plan.profile = Some(Profile { id: games::slug(&name, &[]), text, dir: dir.to_path_buf(), file: file.clone() });
    } else {
        plan.overlay = Some(Overlay { text, dir: dir.to_path_buf() });
    }
    plan.notes.push(format!("Using {}", file.display()));
    Ok(true)
}

/// The game set up for DOSBox at `source`, as a profile: imported the
/// first time, and found again after.
fn imported(dirs: &Dirs, source: &Path) -> Result<Profile, String> {
    let stem = source.file_stem().or_else(|| source.file_name()).map_or("game".into(), |n| n.to_string_lossy().into_owned());
    let dir = dirs.imports().join(games::slug(&stem, &[]));
    let existing = || games::list(&dir).into_iter().next();
    let (entry, text) = match existing() {
        Some(found) => found,
        None => {
            let (_, name, warnings) = games::import(&dir, source, home().as_deref())?;
            let (entry, text) = existing().ok_or_else(|| format!("{} was imported, but its profile isn't there", name))?;
            for warning in warnings {
                eprintln!("[CONFIG] Import of {}: {}", name, warning);
            }
            (entry, text)
        }
    };
    let file = dir.join(format!("{}.conf", entry.id));
    Ok(Profile { id: entry.id, text, dir, file })
}

/// The lines of an m3u playlist: the images in it, relative to its folder.
fn playlist(path: &Path) -> Result<Vec<PathBuf>, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {}", path.display(), e))?;
    let dir = path.parent().unwrap_or(Path::new("."));
    let images: Vec<PathBuf> = text
        .lines()
        .map(|l| l.trim().trim_start_matches('\u{FEFF}'))
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| dir.join(l.split('|').next().unwrap_or(l)))
        .collect();
    if images.is_empty() {
        return Err(format!("{} lists no disk images", path.display()));
    }
    Ok(images)
}

/// Disk images as content: the first in its drive, the rest to change to.
fn images(images: Vec<PathBuf>, boot: bool, dirs: &Dirs, plan: &mut Plan) -> Result<(), String> {
    let first = images[0].clone();
    let more_images = images[1..].to_vec();
    let kind = diskimage::detect(&first, DriveKind::HardDisk)?;
    plan.c_root = dirs.drive_c();
    let (drive, kind, command) = match kind {
        ImageKind::Floppy if boot => (0, DriveKind::Floppy, "BOOT -l A".to_string()),
        ImageKind::Floppy => (0, DriveKind::Floppy, "A:".to_string()),
        ImageKind::Cd => (3, DriveKind::CdRom, "D:".to_string()),
        // A booted hard disk is the BIOS's first, with no DOS drive of its
        // own, as DOSBox's IMGMOUNT 2.
        ImageKind::HardDisk if boot => (numbered_drive(2), DriveKind::HardDisk, "BOOT -l C".to_string()),
        ImageKind::HardDisk => (DRIVE_C, DriveKind::HardDisk, "C:".to_string()),
    };
    plan.mounts.push(MountSpec { drive, path: first, opts: MountOptions { kind, more_images, ..MountOptions::default() } });
    plan.commands.push(command);
    plan.disk_drive = Some(drive);
    Ok(())
}

/// What to do with `content`; None starts the machine at the prompt.
pub fn plan(content: Option<&Path>, dirs: &Dirs, boot: bool) -> Result<Plan, String> {
    let mut plan = Plan { c_root: dirs.drive_c(), ..Plan::default() };
    let Some(path) = content else { return Ok(plan) };
    let path = fs::absolute(path).map_err(|e| format!("{}: {}", path.display(), e))?;
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if ext == "m3u" || ext == "m3u8" {
        images(playlist(&path)?, boot, dirs, &mut plan)?;
        return Ok(plan);
    }
    if ext == "conf" {
        let text = fs::read_to_string(&path).map_err(|e| format!("{}: {}", path.display(), e))?;
        if is_dosbox_conf(&text) {
            plan.profile = Some(imported(dirs, &path)?);
            return Ok(plan);
        }
        let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        if has_autoexec(&text) {
            let stem = path.file_stem().map_or("game".into(), |n| n.to_string_lossy().into_owned());
            plan.profile = Some(Profile { id: games::slug(&stem, &[]), text, dir, file: path.clone() });
        } else {
            plan.overlay = Some(Overlay { text, dir });
        }
        return Ok(plan);
    }
    match drop_action(&path) {
        DropAction::ImportGame(source) if fs::is_dir(&path) => {
            plan.profile = Some(imported(dirs, &source)?);
        }
        DropAction::ImportGame(_) | DropAction::MountFolder(_) => {
            plan.c_root = path.clone();
            folder_conf(&path, &mut plan)?;
        }
        DropAction::Run(program) => {
            let dir = program.parent().unwrap_or(Path::new(".")).to_path_buf();
            plan.c_root = dir.clone();
            // The folder's settings, but the program asked for.
            if folder_conf(&dir, &mut plan)?
                && let Some(profile) = plan.profile.take()
            {
                plan.overlay = Some(Overlay { text: without_autoexec(&profile.text), dir: profile.dir });
            }
            let name = program.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned());
            plan.commands.extend(["C:".to_string(), "CD \\".to_string(), name]);
        }
        DropAction::Package(package) => packaged(dirs, &package, &mut plan)?,
        DropAction::Disc(image) | DropAction::Floppy(image) | DropAction::HardDisk(image) => {
            images(vec![image], boot, dirs, &mut plan)?;
        }
        DropAction::Nothing(e) => return Err(format!("{}: {}", path.display(), e)),
    }
    Ok(plan)
}

/// A game's package (a zip, .dosz or 7z archive, or a folder with a
/// rust-dos.conf): a game with the package as C:, its profile made the
/// first time and found after (`games::add_package`). An archive unpacked
/// into the games folder before is played from there.
fn packaged(dirs: &Dirs, package: &Path, plan: &mut Plan) -> Result<(), String> {
    let games_dir = dirs.games();
    let stem = package.file_stem().map_or("Game".into(), |n| n.to_string_lossy().into_owned());
    let unpacked = games_dir.join(games::slug(&stem, &[]));
    let id = if fs::is_file(package) && fs::is_dir(&unpacked) {
        plan.c_root = unpacked.clone();
        unpacked.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned())
    } else {
        games::add_package(&games_dir, package)?.0
    };
    let file = games_dir.join(format!("{}.conf", id));
    if let Ok(text) = fs::read_to_string(&file) {
        plan.profile = Some(Profile { id, text, dir: games_dir, file });
    }
    Ok(())
}

/// Configuration text without its `[autoexec]` section.
fn without_autoexec(text: &str) -> String {
    config::with_autoexec(text, &[])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(name: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/test-content").join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn dosbox_and_rust_dos_configurations_are_told_apart() {
        assert!(is_dosbox_conf("[sdl]\nfullscreen=false\n[cpu]\ncycles=max\n"));
        assert!(!is_dosbox_conf("[emulator]\ncycles=max\n[mixer]\nopl=80\n[autoexec]\nC:\n"));
        assert!(has_autoexec("[autoexec]\nC:\nGAME\n"));
        assert!(!has_autoexec("[emulator]\ncycles=max\n"));
    }

    #[test]
    fn programs_run_from_their_folder() {
        let dir = scratch("program");
        let dirs = Dirs::new(dir.join("system"), dir.join("saves"));
        fs::create_dir_all(dir.join("game")).unwrap();
        fs::write(dir.join("game/GAME.EXE"), "MZ").unwrap();
        fs::write(dir.join("game/rust-dos.conf"), "[emulator]\ncycles=5000\n[autoexec]\nGAME\n").unwrap();
        let plan = plan(Some(&dir.join("game/GAME.EXE")), &dirs, false).unwrap();
        assert_eq!(plan.c_root, dir.join("game"));
        assert_eq!(plan.commands, ["C:", "CD \\", "GAME.EXE"]);
        assert!(plan.profile.is_none());
        let overlay = plan.overlay.unwrap();
        assert!(!has_autoexec(&overlay.text));
        let (settings, warnings) = settings(&Base::load(&dirs), &Values::default(), Some(&overlay));
        assert!(warnings.is_empty(), "{:?}", warnings);
        assert_eq!(settings.cycles, rust_dos::timer::CpuSpeed::Fixed(5000));
    }

    #[test]
    fn playlists_put_their_images_in_one_drive() {
        let dir = scratch("m3u");
        let dirs = Dirs::new(dir.join("system"), dir.join("saves"));
        fs::write(dir.join("disk1.img"), vec![0u8; 1_474_560]).unwrap();
        fs::write(dir.join("disk2.img"), vec![0u8; 1_474_560]).unwrap();
        fs::write(dir.join("game.m3u"), "#EXTM3U\ndisk1.img\ndisk2.img\n").unwrap();
        let plan = plan(Some(&dir.join("game.m3u")), &dirs, false).unwrap();
        assert_eq!(plan.disk_drive, Some(0));
        assert_eq!(plan.mounts[0].path, dir.join("disk1.img"));
        assert_eq!(plan.mounts[0].opts.more_images, [dir.join("disk2.img")]);
        assert_eq!(plan.commands, ["A:"]);
        let booted = super::plan(Some(&dir.join("game.m3u")), &dirs, true).unwrap();
        assert_eq!(booted.commands, ["BOOT -l A"]);
    }

    #[test]
    fn the_core_options_go_over_rust_dos_conf() {
        let dir = scratch("layers");
        let dirs = Dirs::new(dir.join("system"), dir.join("saves"));
        fs::create_dir_all(&dirs.sys).unwrap();
        fs::write(dirs.sys.join("rust-dos.conf"), "[emulator]\ncycles=3000\nems=false\nrewind=true\n").unwrap();
        let mut options = Values::default();
        options.set("cycles", "20000");
        options.set("ems", "default");
        let (settings, _) = settings(&Base::load(&dirs), &options, None);
        assert_eq!(settings.cycles, rust_dos::timer::CpuSpeed::Fixed(20000));
        assert!(!settings.ems);
        assert!(!settings.rewind, "the frontend rewinds");
    }
}
