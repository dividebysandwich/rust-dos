//! The rust-dos configuration file: a DOSBox-style INI file with
//! `[emulator]`, `[sound]`, `[drives]` and `[autoexec]` sections. See
//! `rust-dos.conf.example` for the format.
//!
//! Lookup order, first match wins: `--config FILE`, `./rust-dos.conf`, then
//! `rust-dos.conf` in the per-user configuration directory, where a
//! commented template is written on first start.
//!
//! The settings window saves back into the file in use (`save`), changing
//! only the lines of the settings and drives and keeping everything else.

use crate::cpu::CpuModel;
use crate::disk::DRIVE_Z;
use crate::diskio::{DiskSettings, DiskSpeed, NoiseMode};
use crate::mount::{MountSpec, contract_home, mount_spec_value, parse_drive_letter, parse_mount_spec, tokenize};
use crate::timer::CpuSpeed;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "rust-dos.conf";
/// Written to the default location on first start.
pub const TEMPLATE: &str = include_str!("../rust-dos.conf.example");

/// The per-user directory for rust-dos's files, e.g. `~/.config/rust-dos`
/// on Linux.
pub fn user_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("rust-dos"))
}

/// Per-user default: `<config dir>/rust-dos/rust-dos.conf`, e.g.
/// `~/.config/rust-dos/rust-dos.conf` on Linux.
pub fn default_path() -> Option<PathBuf> {
    user_dir().map(|d| d.join(FILE_NAME))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigSource {
    CommandLine,
    WorkingDir,
    UserDefault,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Located {
    Found(PathBuf, ConfigSource),
    /// Nothing found; `default` is where a template should be written.
    NotFound {
        default: Option<PathBuf>,
    },
}

/// Find the config file to use. An explicitly requested file must exist.
pub fn locate(cli: Option<&Path>, cwd: &Path, default: Option<PathBuf>) -> Result<Located, String> {
    if let Some(path) = cli {
        let path = cwd.join(path);
        if path.is_file() {
            return Ok(Located::Found(path, ConfigSource::CommandLine));
        }
        return Err(format!("Config file {} not found", path.display()));
    }
    let local = cwd.join(FILE_NAME);
    if local.is_file() {
        return Ok(Located::Found(local, ConfigSource::WorkingDir));
    }
    match default {
        Some(path) if path.is_file() => Ok(Located::Found(path, ConfigSource::UserDefault)),
        default => Ok(Located::NotFound { default }),
    }
}

#[derive(Debug, Default)]
pub struct Config {
    /// The file the settings came from, if any.
    pub source: Option<PathBuf>,
    /// True if `source` was just created from the template.
    pub created: bool,
    pub scale: Option<u32>,
    /// Desktop fullscreen (`fullscreen`).
    pub fullscreen: Option<bool>,
    /// Stretch the picture to 4:3 (`aspect`).
    pub aspect: Option<bool>,
    /// How the picture is scaled to the window (`filter`).
    pub filter: Option<Filter>,
    /// Emulated CPU speed (`cycles`).
    pub cycles: Option<CpuSpeed>,
    /// Emulated processor (`cpu`).
    pub cpu: Option<CpuModel>,
    /// RAM in MB (`memsize`).
    pub memsize: Option<usize>,
    /// `[sound]`: the Sound Blaster (None: `sbtype=none`), the FM chip,
    /// the Gravis Ultrasound, and the MPU-401's synthesizer.
    pub sound: SoundConfig,
    /// How fast the disks are (`[emulator]`) and the noises they make
    /// (`[sound]`).
    pub disk: DiskSettings,
    /// `[drives]` entries in file order, at most one per drive.
    pub drives: Vec<MountSpec>,
    /// `[autoexec]` command lines in file order.
    pub autoexec: Vec<String>,
    /// Problems worth telling the user about; none of them are fatal.
    pub warnings: Vec<String>,
}

impl Config {
    pub fn drive(&self, drive: u8) -> Option<&MountSpec> {
        self.drives.iter().find(|spec| spec.drive == drive)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    None,
    Emulator,
    Sound,
    Drives,
    Autoexec,
    Unknown,
}

impl Section {
    fn parse(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "emulator" => Section::Emulator,
            "sound" => Section::Sound,
            "drives" => Section::Drives,
            "autoexec" => Section::Autoexec,
            _ => Section::Unknown,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Section::Emulator => "emulator",
            Section::Sound => "sound",
            Section::Drives => "drives",
            Section::Autoexec => "autoexec",
            Section::None | Section::Unknown => "",
        }
    }
}

/// How the picture is scaled up to the window (`filter`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Filter {
    /// Sharp pixels.
    #[default]
    Nearest,
    /// Smooth, interpolated pixels.
    Linear,
}

impl Filter {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "nearest" => Some(Filter::Nearest),
            "linear" => Some(Filter::Linear),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Filter::Nearest => "nearest",
            Filter::Linear => "linear",
        }
    }
}

/// A yes/no setting: true, on, yes or 1, or false, off, no or 0.
fn parse_bool(value: &str) -> Option<bool> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "on" | "yes" | "1" => Some(true),
        "false" | "off" | "no" | "0" => Some(false),
        _ => None,
    }
}

/// The synthesizer that plays the MPU-401's General MIDI (`midisynth`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MidiSynth {
    /// The SoundFont if one is set, else the Ultrasound patches.
    Auto,
    SoundFont,
    Gus,
    None,
}

/// The `[sound]` section.
#[derive(Debug, Clone, PartialEq)]
pub struct SoundConfig {
    /// The Sound Blaster's resources, and whether there is one.
    pub sb: crate::sb::SbConfig,
    pub sb_installed: bool,
    /// OPL3 (as on an SB Pro 2 or SB16) rather than OPL2.
    pub opl3: bool,
    pub soundfont: Option<PathBuf>,
    /// The Gravis Ultrasound; `enabled` says whether there is one.
    pub gus: crate::gus::GusConfig,
    pub midisynth: MidiSynth,
}

impl MidiSynth {
    pub fn name(self) -> &'static str {
        match self {
            MidiSynth::Auto => "auto",
            MidiSynth::SoundFont => "soundfont",
            MidiSynth::Gus => "gus",
            MidiSynth::None => "none",
        }
    }
}

impl Default for SoundConfig {
    fn default() -> Self {
        Self {
            sb: crate::sb::SbConfig::default(),
            sb_installed: true,
            opl3: true,
            soundfont: None,
            gus: crate::gus::GusConfig::default(),
            midisynth: MidiSynth::Auto,
        }
    }
}

impl SoundConfig {
    /// The Sound Blaster to install, if any.
    pub fn card(&self) -> Option<crate::sb::SbConfig> {
        self.sb_installed.then_some(self.sb)
    }

    /// The Gravis Ultrasound to install, if any.
    pub fn ultrasound(&self) -> Option<crate::gus::GusConfig> {
        self.gus.enabled.then(|| self.gus.clone())
    }

    /// Problems between settings, once the section is read: an Ultrasound
    /// on the Sound Blaster's ports is left out; shared IRQs and DMA
    /// channels only work while programs use one card at a time.
    pub fn check(&mut self) -> Vec<String> {
        let mut warnings = Vec::new();
        let Some(sb) = self.card() else { return warnings };
        if !self.gus.enabled {
            return warnings;
        }
        if self.gus.base == sb.base {
            warnings.push(format!(
                "[sound]: gusbase {:X} is the Sound Blaster's base port; there is no Ultrasound",
                self.gus.base
            ));
            self.gus.enabled = false;
            return warnings;
        }
        if self.gus.irq == sb.irq {
            warnings.push(format!("[sound]: the Ultrasound and the Sound Blaster share IRQ {}", sb.irq));
        }
        if self.gus.dma == sb.dma8 || (sb.model == crate::sb::SbModel::Sb16 && self.gus.dma == sb.dma16) {
            warnings.push(format!("[sound]: the Ultrasound and the Sound Blaster share DMA {}", self.gus.dma));
        }
        warnings
    }

    fn set(&mut self, key: &str, value: &str, base_dir: &Path, home: Option<&Path>) -> Result<(), String> {
        let sb = &mut self.sb;
        let number = |min: u32, max: u32| match value.parse::<u32>() {
            Ok(n) if (min..=max).contains(&n) => Ok(n),
            _ => Err(format!("invalid {} '{}' ({} to {})", key, value, min, max)),
        };
        match key.to_ascii_lowercase().as_str() {
            "sbtype" => {
                if value.eq_ignore_ascii_case("none") {
                    self.sb_installed = false;
                } else {
                    sb.model = crate::sb::SbModel::parse(value)
                        .ok_or_else(|| format!("invalid sbtype '{}' (sb16, sbpro2, sb2 or none)", value))?;
                    self.sb_installed = true;
                }
            }
            "sbbase" => {
                let base = u16::from_str_radix(value.trim_start_matches("0x"), 16)
                    .ok()
                    .filter(|b| (0x210..=0x280).contains(b) && b & 0xF == 0)
                    .ok_or_else(|| format!("invalid sbbase '{}' (210 to 280, hex)", value))?;
                sb.base = base;
            }
            "irq" => {
                let irq = number(2, 15)?;
                if !matches!(irq, 2 | 3 | 5 | 7 | 9 | 10 | 11 | 12 | 15) {
                    return Err(format!("invalid irq '{}'", value));
                }
                sb.irq = irq as u8;
            }
            "dma" => sb.dma8 = number(0, 3).and_then(|d| if d == 2 { Err("dma 2 belongs to the floppy".to_string()) } else { Ok(d) })? as u8,
            "hdma" => sb.dma16 = number(5, 7)? as u8,
            "opl" => {
                self.opl3 = match value.to_ascii_lowercase().as_str() {
                    "opl3" => true,
                    "opl2" => false,
                    _ => return Err(format!("invalid opl '{}' (opl3 or opl2)", value)),
                }
            }
            "soundfont" => {
                let path = match (value.strip_prefix("~/"), home) {
                    (Some(rest), Some(h)) => h.join(rest),
                    _ => base_dir.join(value),
                };
                self.soundfont = Some(path);
            }
            "gus" => {
                self.gus.enabled =
                    parse_bool(value).ok_or_else(|| format!("invalid gus '{}' (true or false)", value))?;
            }
            "gusbase" => {
                // 230h would put 3X0h-3X7h on the MPU-401 at 330h.
                self.gus.base = u16::from_str_radix(value.trim_start_matches("0x"), 16)
                    .ok()
                    .filter(|b| matches!(b, 0x210 | 0x220 | 0x240 | 0x250 | 0x260))
                    .ok_or_else(|| format!("invalid gusbase '{}' (210, 220, 240, 250 or 260, hex)", value))?;
            }
            "gusirq" => {
                self.gus.irq = value
                    .parse::<u8>()
                    .ok()
                    .filter(|i| matches!(i, 2 | 3 | 5 | 7 | 11 | 12 | 15))
                    .ok_or_else(|| format!("invalid gusirq '{}' (2, 3, 5, 7, 11, 12 or 15)", value))?;
            }
            "gusdma" => {
                self.gus.dma = value
                    .parse::<u8>()
                    .ok()
                    .filter(|d| matches!(d, 1 | 3 | 5 | 6 | 7))
                    .ok_or_else(|| format!("invalid gusdma '{}' (1, 3, 5, 6 or 7)", value))?;
            }
            "gusdrive" => {
                self.gus.drive = match value.to_ascii_lowercase().as_str() {
                    "none" | "off" | "false" | "no" => None,
                    _ => Some(
                        parse_drive_letter(value)
                            .filter(|&d| (3..DRIVE_Z).contains(&d))
                            .ok_or_else(|| format!("invalid gusdrive '{}' (a letter from D to Y, or none)", value))?,
                    ),
                }
            }
            "ultradir" => {
                if value.is_empty() {
                    return Err("ultradir is empty".to_string());
                }
                self.gus.ultradir = Some(value.to_string());
            }
            "midisynth" => {
                self.midisynth = match value.to_ascii_lowercase().as_str() {
                    "auto" => MidiSynth::Auto,
                    "soundfont" => MidiSynth::SoundFont,
                    "gus" => MidiSynth::Gus,
                    "none" => MidiSynth::None,
                    _ => return Err(format!("invalid midisynth '{}' (auto, soundfont, gus or none)", value)),
                }
            }
            _ => return Err(format!("unknown setting '{}'", key)),
        }
        Ok(())
    }
}

/// Parse config text. Relative drive paths resolve against `base_dir`.
pub fn parse(text: &str, base_dir: &Path, home: Option<&Path>) -> Config {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let mut config = Config::default();
    let mut warnings = Vec::new();
    let mut section = Section::None;

    for (index, raw) in text.lines().enumerate() {
        let mut warn = |msg: String| warnings.push(format!("line {}: {}", index + 1, msg));
        let line = raw.trim();
        // Only whole-line comments: paths may contain '#' or ';'
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }

        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = Section::parse(name);
            if section == Section::Unknown {
                warn(format!("unknown section [{}]", name.trim().to_ascii_lowercase()));
            }
            continue;
        }

        match section {
            Section::Autoexec => config.autoexec.push(line.to_string()),
            Section::Unknown => {}
            Section::None => warn("setting outside of a section".to_string()),
            Section::Emulator | Section::Drives | Section::Sound => {
                let Some((key, value)) = line.split_once('=') else {
                    warn(format!("expected key=value, got '{}'", line));
                    continue;
                };
                let (key, value) = (key.trim(), value.trim());

                if section == Section::Emulator {
                    match key.to_ascii_lowercase().as_str() {
                        "scale" => match value.parse::<u32>() {
                            Ok(n) if (1..=16).contains(&n) => config.scale = Some(n),
                            _ => warn(format!("invalid scale '{}'", value)),
                        },
                        "fullscreen" | "aspect" => match parse_bool(value) {
                            Some(on) if key.eq_ignore_ascii_case("fullscreen") => config.fullscreen = Some(on),
                            Some(on) => config.aspect = Some(on),
                            None => warn(format!("invalid {} '{}' (true or false)", key, value)),
                        },
                        "filter" => match Filter::parse(value) {
                            Some(filter) => config.filter = Some(filter),
                            None => warn(format!("invalid filter '{}' (nearest or linear)", value)),
                        },
                        "cycles" => match CpuSpeed::parse(value) {
                            Ok(speed) => config.cycles = Some(speed),
                            Err(e) => warn(e),
                        },
                        "memsize" => match value.parse::<usize>() {
                            Ok(mb) if (2..=64).contains(&mb) => config.memsize = Some(mb),
                            _ => warn(format!("invalid memsize '{}' (2 to 64 MB)", value)),
                        },
                        "cpu" => match value.to_ascii_lowercase().as_str() {
                            "386" => config.cpu = Some(CpuModel::I386),
                            "486" => config.cpu = Some(CpuModel::I486),
                            _ => warn(format!("invalid cpu '{}' (386 or 486)", value)),
                        },
                        "hard_disk_speed" | "floppy_disk_speed" => match DiskSpeed::parse(value) {
                            Some(speed) if key.eq_ignore_ascii_case("hard_disk_speed") => {
                                config.disk.hard_disk_speed = speed
                            }
                            Some(speed) => config.disk.floppy_disk_speed = speed,
                            None => warn(format!("invalid {} '{}' (maximum, fast, medium or slow)", key, value)),
                        },
                        _ => warn(format!("unknown setting '{}'", key)),
                    }
                    continue;
                }

                if section == Section::Sound {
                    let lower = key.to_ascii_lowercase();
                    if matches!(lower.as_str(), "hard_disk_noise" | "floppy_disk_noise") {
                        match NoiseMode::parse(value) {
                            Some(mode) if lower == "hard_disk_noise" => config.disk.hard_disk_noise = mode,
                            Some(mode) => config.disk.floppy_disk_noise = mode,
                            None => warn(format!("invalid {} '{}' (off, seek-only or on)", key, value)),
                        }
                    } else if let Err(e) = config.sound.set(key, value, base_dir, home) {
                        warn(e);
                    }
                    continue;
                }

                let Some(drive) = parse_drive_letter(key) else {
                    warn(format!("invalid drive letter '{}'", key));
                    continue;
                };
                let letter = key.trim_end_matches(':').to_ascii_uppercase();
                if drive == DRIVE_Z {
                    warn("drive Z: is reserved".to_string());
                    continue;
                }
                match tokenize(value).and_then(|t| parse_mount_spec(drive, &t, base_dir, home)) {
                    Ok(spec) => {
                        if config.drive(drive).is_some() {
                            warn(format!(
                                "drive {}: defined twice, using the last one",
                                letter
                            ));
                            config.drives.retain(|s| s.drive != drive);
                        }
                        config.drives.push(spec);
                    }
                    Err(e) => warn(format!("drive {}: {}", letter, e)),
                }
            }
        }
    }
    warnings.extend(config.sound.check());
    // A drive of the user's own takes the letter of the built-in Ultrasound
    // software.
    if let Some(drive) = config.sound.gus.drive.filter(|_| config.sound.gus.enabled)
        && config.drive(drive).is_some()
    {
        let letter = crate::disk::drive_letter(drive);
        warnings.push(format!(
            "[sound]: gusdrive {}: is mounted in [drives]; the built-in Ultrasound software is left out",
            letter
        ));
        config.sound.gus.drive = None;
    }
    config.warnings = warnings;
    config
}

/// Write the commented template to `path`, creating parent directories.
/// Never overwrites an existing file.
pub fn write_template(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(TEMPLATE.as_bytes())
}

/// Locate, read and parse the configuration. Only a missing or unreadable
/// `--config` file is an error; everything else degrades to warnings.
pub fn load(
    cli: Option<&Path>,
    cwd: &Path,
    default: Option<PathBuf>,
    home: Option<&Path>,
) -> Result<Config, String> {
    let (path, source) = match locate(cli, cwd, default)? {
        Located::Found(path, source) => (path, source),
        Located::NotFound { default: None } => return Ok(Config::default()),
        Located::NotFound {
            default: Some(path),
        } => {
            return Ok(match write_template(&path) {
                Ok(()) => Config {
                    source: Some(path),
                    created: true,
                    ..parse(TEMPLATE, cwd, home)
                },
                Err(e) => Config {
                    warnings: vec![format!("could not create {}: {}", path.display(), e)],
                    ..Config::default()
                },
            });
        }
    };

    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) => {
            let msg = format!("cannot read {}: {}", path.display(), e);
            if source == ConfigSource::CommandLine {
                return Err(msg);
            }
            return Ok(Config {
                warnings: vec![msg],
                ..Config::default()
            });
        }
    };

    let absolute = std::path::absolute(&path).unwrap_or_else(|_| path.clone());
    let base_dir = absolute.parent().unwrap_or(cwd);
    let mut config = parse(&text, base_dir, home);
    config.warnings = config
        .warnings
        .into_iter()
        .map(|w| format!("{}: {}", path.display(), w))
        .collect();
    config.source = Some(path);
    Ok(config)
}

/// The settings in effect: the configuration with every value that is not
/// set filled in with its default.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub scale: u32,
    pub fullscreen: bool,
    pub aspect: bool,
    pub filter: Filter,
    pub cycles: CpuSpeed,
    pub cpu: CpuModel,
    /// RAM in MB.
    pub memsize: usize,
    pub sound: SoundConfig,
    pub disk: DiskSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            scale: 1,
            fullscreen: false,
            aspect: false,
            filter: Filter::Nearest,
            cycles: CpuSpeed::Max,
            cpu: CpuModel::I486,
            memsize: crate::bus::DEFAULT_MEMORY_MB,
            sound: SoundConfig::default(),
            disk: DiskSettings::default(),
        }
    }
}

impl Settings {
    pub fn from_config(config: &Config) -> Self {
        let default = Self::default();
        Self {
            scale: config.scale.unwrap_or(default.scale),
            fullscreen: config.fullscreen.unwrap_or(default.fullscreen),
            aspect: config.aspect.unwrap_or(default.aspect),
            filter: config.filter.unwrap_or(default.filter),
            cycles: config.cycles.unwrap_or(default.cycles),
            cpu: config.cpu.unwrap_or(default.cpu),
            memsize: config.memsize.unwrap_or(default.memsize),
            sound: config.sound.clone(),
            disk: config.disk,
        }
    }
}

/// Every setting as the file writes it: section, key and value, or None
/// for a setting that is not set.
fn entries(settings: &Settings, home: Option<&Path>) -> Vec<(Section, &'static str, Option<String>)> {
    use Section::{Emulator, Sound};
    let yes_no = |on: bool| Some(if on { "true" } else { "false" }.to_string());
    let sound = &settings.sound;
    let (sb, gus) = (&sound.sb, &sound.gus);
    vec![
        (Emulator, "scale", Some(settings.scale.to_string())),
        (Emulator, "fullscreen", yes_no(settings.fullscreen)),
        (Emulator, "aspect", yes_no(settings.aspect)),
        (Emulator, "filter", Some(settings.filter.name().to_string())),
        (
            Emulator,
            "cycles",
            Some(match settings.cycles {
                CpuSpeed::Max => "max".to_string(),
                CpuSpeed::Fixed(n) => n.to_string(),
            }),
        ),
        (
            Emulator,
            "cpu",
            Some(match settings.cpu {
                CpuModel::I386 => "386",
                CpuModel::I486 => "486",
            }
            .to_string()),
        ),
        (Emulator, "memsize", Some(settings.memsize.to_string())),
        (Emulator, "hard_disk_speed", Some(settings.disk.hard_disk_speed.name().to_string())),
        (Emulator, "floppy_disk_speed", Some(settings.disk.floppy_disk_speed.name().to_string())),
        (Sound, "sbtype", Some(if sound.sb_installed { sb.model.name() } else { "none" }.to_string())),
        (Sound, "sbbase", Some(format!("{:X}", sb.base))),
        (Sound, "irq", Some(sb.irq.to_string())),
        (Sound, "dma", Some(sb.dma8.to_string())),
        (Sound, "hdma", Some(sb.dma16.to_string())),
        (Sound, "opl", Some(if sound.opl3 { "opl3" } else { "opl2" }.to_string())),
        (Sound, "soundfont", sound.soundfont.as_deref().map(|p| contract_home(p, home))),
        (Sound, "gus", yes_no(gus.enabled)),
        (Sound, "gusbase", Some(format!("{:X}", gus.base))),
        (Sound, "gusirq", Some(gus.irq.to_string())),
        (Sound, "gusdma", Some(gus.dma.to_string())),
        (
            Sound,
            "gusdrive",
            Some(gus.drive.map_or("none".to_string(), |d| crate::disk::drive_letter(d).to_string())),
        ),
        (Sound, "ultradir", gus.ultradir.clone()),
        (Sound, "midisynth", Some(sound.midisynth.name().to_string())),
        (Sound, "hard_disk_noise", Some(settings.disk.hard_disk_noise.name().to_string())),
        (Sound, "floppy_disk_noise", Some(settings.disk.floppy_disk_noise.name().to_string())),
    ]
}

/// What a line of the file is, for `update_text`.
#[derive(Debug, PartialEq)]
enum Line {
    Header(Section),
    /// `key=value` (the key in lower case), or a drive line in `[drives]`.
    Setting(String),
    /// A commented-out `#key=value`, the template's examples.
    Example(String),
    /// Blank lines, other comments, `[autoexec]` commands.
    Other,
}

/// The section and kind of every line.
fn classify(lines: &[String]) -> Vec<(Section, Line)> {
    let mut section = Section::None;
    let key_of = |text: &str| {
        let key = text.split_once('=')?.0.trim();
        (!key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == ':' || c == '_'))
            .then(|| key.to_ascii_lowercase())
    };
    lines
        .iter()
        .map(|raw| {
            let line = raw.trim();
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                section = Section::parse(name);
                return (section, Line::Header(section));
            }
            let kind = match section {
                Section::Emulator | Section::Sound | Section::Drives => {
                    if let Some(comment) = line.strip_prefix(['#', ';']) {
                        key_of(comment.trim_start_matches(['#', ';']).trim_start())
                            .map_or(Line::Other, Line::Example)
                    } else {
                        key_of(line).map_or(Line::Other, Line::Setting)
                    }
                }
                _ => Line::Other,
            };
            (section, kind)
        })
        .collect()
}

/// Index after the last non-blank line of the last `[section]` block, or
/// None if the file has no such section.
fn section_end(lines: &[String], layout: &[(Section, Line)], section: Section) -> Option<usize> {
    let header = layout.iter().rposition(|(_, l)| *l == Line::Header(section))?;
    let mut last = header;
    for (i, (_, line)) in layout.iter().enumerate().skip(header + 1) {
        if matches!(line, Line::Header(_)) {
            break;
        }
        if !lines[i].trim().is_empty() {
            last = i;
        }
    }
    Some(last + 1)
}

/// Where a new line of `section` goes: after the commented example of
/// `key` if there is one, else at the end of the section. Creates the
/// section, before `[autoexec]` or at the end, if the file has none.
fn insertion_point(lines: &mut Vec<String>, section: Section, key: Option<&str>) -> usize {
    let layout = classify(lines);
    if let Some(key) = key
        && let Some(i) = layout.iter().rposition(|(s, l)| *s == section && *l == Line::Example(key.to_string()))
    {
        return i + 1;
    }
    if let Some(end) = section_end(lines, &layout, section) {
        return end;
    }
    let header = format!("[{}]", section.name());
    match layout.iter().position(|(_, l)| *l == Line::Header(Section::Autoexec)) {
        Some(autoexec) => {
            lines.splice(autoexec..autoexec, [header, String::new()]);
            autoexec + 1
        }
        None => {
            if lines.last().is_some_and(|l| !l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(header);
            lines.len()
        }
    }
}

/// `line` (`key=value`) with a new value, keeping the key as written and
/// the spacing around the '='.
fn with_value(line: &str, value: &str) -> String {
    let eq = line.find('=').unwrap_or(line.len());
    let rest = line.get(eq + 1..).unwrap_or("");
    let gap = rest.len() - rest.trim_start().len();
    format!("{}={}{}", &line[..eq], &rest[..gap], value)
}

/// Set `key` in `section` to `value`: the last line that sets it gets the
/// new value, or a new line is added. With None, the lines that set it
/// are commented out.
fn set_key(lines: &mut Vec<String>, section: Section, key: &str, value: Option<&str>) {
    let layout = classify(lines);
    let setting = Line::Setting(key.to_string());
    let existing: Vec<usize> =
        (0..lines.len()).filter(|&i| layout[i].0 == section && layout[i].1 == setting).collect();
    match (existing.last(), value) {
        (Some(&i), Some(value)) => lines[i] = with_value(&lines[i], value),
        (Some(_), None) => {
            for i in existing {
                lines[i] = format!("#{}", lines[i]);
            }
        }
        (None, Some(value)) => {
            let at = insertion_point(lines, section, Some(key));
            lines.insert(at, format!("{}={}", key, value));
        }
        (None, None) => {}
    }
}

/// Set the `[drives]` line of `drive` to `spec`: the last line for the
/// drive gets the new value, or a line is added after the other drives.
/// With None, the drive's lines are removed.
fn set_drive(lines: &mut Vec<String>, drive: u8, spec: Option<&MountSpec>, home: Option<&Path>) {
    let layout = classify(lines);
    let is_drive = |(s, l): &(Section, Line)| {
        *s == Section::Drives && matches!(l, Line::Setting(k) if parse_drive_letter(k).is_some())
    };
    let existing: Vec<usize> = (0..lines.len())
        .filter(|&i| {
            is_drive(&layout[i])
                && matches!(&layout[i].1, Line::Setting(k) if parse_drive_letter(k) == Some(drive))
        })
        .collect();
    match (existing.last(), spec) {
        (Some(&i), Some(spec)) => lines[i] = with_value(&lines[i], &mount_spec_value(spec, home)),
        (Some(_), None) => {
            for &i in existing.iter().rev() {
                lines.remove(i);
            }
        }
        (None, Some(spec)) => {
            let line = format!("{}={}", crate::disk::drive_letter(drive), mount_spec_value(spec, home));
            let at = match layout.iter().rposition(is_drive) {
                Some(last) => last + 1,
                None => insertion_point(lines, Section::Drives, None),
            };
            lines.insert(at, line);
        }
        (None, None) => {}
    }
}

/// A change to `[drives]`: the drive's new mount, or None for no drive.
pub type DriveChange = (u8, Option<MountSpec>);

/// `original` with what changed from `baseline` to `settings`, and the
/// drive changes, written into it. Everything else in the file (comments,
/// blank lines, `[autoexec]`, the settings that didn't change) stays as it
/// is.
pub fn update_text(
    original: &str,
    baseline: &Settings,
    settings: &Settings,
    drives: &[DriveChange],
    home: Option<&Path>,
) -> String {
    let (bom, text) = match original.strip_prefix('\u{FEFF}') {
        Some(rest) => ("\u{FEFF}", rest),
        None => ("", original),
    };
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let final_newline = text.is_empty() || text.ends_with('\n');
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();

    let before = entries(baseline, home);
    for ((section, key, value), (_, _, old)) in entries(settings, home).into_iter().zip(before) {
        if value != old {
            set_key(&mut lines, section, key, value.as_deref());
        }
    }
    for (drive, spec) in drives {
        set_drive(&mut lines, *drive, spec.as_ref(), home);
    }

    let mut out = format!("{}{}", bom, lines.join(newline));
    if final_newline && !lines.is_empty() {
        out.push_str(newline);
    }
    out
}

/// Save what changed from `baseline` to `settings`, and the drive changes,
/// into the configuration file at `path` (see `update_text`). The file is
/// replaced in one step, so a failed write leaves the old one.
pub fn save(
    path: &Path,
    baseline: &Settings,
    settings: &Settings,
    drives: &[DriveChange],
    home: Option<&Path>,
) -> Result<(), String> {
    let original = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => TEMPLATE.to_string(),
        Err(e) => return Err(format!("cannot read {}: {}", path.display(), e)),
    };
    let text = update_text(&original, baseline, settings, drives, home);
    // Write through a symbolic link rather than replacing it, and keep the
    // file's permissions.
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut temp_name = target.file_name().unwrap_or_default().to_os_string();
    temp_name.push(".tmp");
    let temp = target.with_file_name(temp_name);
    let permissions = fs::metadata(&target).map(|m| m.permissions()).ok();
    fs::write(&temp, text)
        .and_then(|()| permissions.map_or(Ok(()), |p| fs::set_permissions(&temp, p)))
        .and_then(|()| fs::rename(&temp, &target))
        .map_err(|e| {
            let _ = fs::remove_file(&temp);
            format!("cannot write {}: {}", target.display(), e)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk::DriveKind;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::path::absolute(PathBuf::from("target/test_config_unit").join(name)).unwrap();
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn locate_priority() {
        let base = scratch("locate");
        let cwd = base.join("work");
        let default = base.join("home/rust-dos/rust-dos.conf");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(default.parent().unwrap()).unwrap();
        fs::write(base.join("custom.conf"), "").unwrap();

        // Nothing yet: the default path is where the template goes
        assert_eq!(
            locate(None, &cwd, Some(default.clone())),
            Ok(Located::NotFound {
                default: Some(default.clone())
            })
        );

        fs::write(&default, "").unwrap();
        assert_eq!(
            locate(None, &cwd, Some(default.clone())),
            Ok(Located::Found(default.clone(), ConfigSource::UserDefault))
        );

        fs::write(cwd.join(FILE_NAME), "").unwrap();
        assert_eq!(
            locate(None, &cwd, Some(default.clone())),
            Ok(Located::Found(
                cwd.join(FILE_NAME),
                ConfigSource::WorkingDir
            ))
        );

        // Relative --config paths are taken from the working directory
        assert_eq!(
            locate(
                Some(Path::new("../custom.conf")),
                &cwd,
                Some(default.clone())
            ),
            Ok(Located::Found(
                cwd.join("../custom.conf"),
                ConfigSource::CommandLine
            ))
        );
        assert!(locate(Some(Path::new("missing.conf")), &cwd, Some(default)).is_err());
    }

    #[test]
    fn parses_sections_drives_and_autoexec() {
        let text = "\u{FEFF}# comment\r\n\
            [Emulator]\r\n\
            scale = 3\r\n\
            cycles = 3000\r\n\
            \r\n\
            [DRIVES]\r\n\
            c = ~/dos\r\n\
            A: = floppy floppy -label disk1\r\n\
            D=\"My CD\" cdrom\r\n\
            E=C:\\Games\\Dos -ro\r\n\
            ; another comment\r\n\
            [autoexec]\r\n\
            @echo off\r\n\
            MOUNT F ~/x\r\n\
            # skipped\r\n\
            D:\r\n";
        let base = Path::new("/cfg");
        let home = Path::new("/home/u");
        let config = parse(text, base, Some(home));
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
        assert_eq!(config.scale, Some(3));
        assert_eq!(config.cycles, Some(CpuSpeed::Fixed(3000)));

        let c = config.drive(2).unwrap();
        assert_eq!(c.path, home.join("dos"));
        assert_eq!(c.opts.kind, DriveKind::HardDisk);
        let a = config.drive(0).unwrap();
        assert_eq!(a.path, base.join("floppy"));
        assert_eq!(a.opts.kind, DriveKind::Floppy);
        assert_eq!(a.opts.label.as_deref(), Some("disk1"));
        let d = config.drive(3).unwrap();
        assert_eq!(d.path, base.join("My CD"));
        assert_eq!(d.opts.kind, DriveKind::CdRom);
        let e = config.drive(4).unwrap();
        assert!(e.path.to_string_lossy().ends_with(r"C:\Games\Dos"));
        assert!(e.opts.read_only);

        assert_eq!(config.autoexec, ["@echo off", "MOUNT F ~/x", "D:"]);
    }

    #[test]
    fn problems_become_warnings() {
        let text = "orphan=1\n\
            [emulator]\n\
            scale=0\n\
            colour=blue\n\
            [drives]\n\
            Z=/tmp\n\
            7=/tmp\n\
            D=/one\n\
            D=/two\n\
            E=/x zip\n\
            nonsense\n\
            [joystick]\n\
            ignored=1\n";
        let config = parse(text, Path::new("/cfg"), None);
        let joined = config.warnings.join("\n");
        for expected in [
            "line 1: setting outside of a section",
            "line 3: invalid scale '0'",
            "line 4: unknown setting 'colour'",
            "line 6: drive Z: is reserved",
            "line 7: invalid drive letter '7'",
            "line 9: drive D: defined twice",
            "line 10: drive E: Unknown option 'zip'",
            "line 11: expected key=value",
            "line 12: unknown section [joystick]",
        ] {
            assert!(
                joined.contains(expected),
                "missing '{}' in:\n{}",
                expected,
                joined
            );
        }
        assert_eq!(config.warnings.len(), 9, "{}", joined);
        assert_eq!(config.drive(3).unwrap().path, Path::new("/two"));
        assert_eq!(config.scale, None);
    }

    #[test]
    fn sound_section() {
        let config = parse(
            "[sound]\nsbtype=sbpro2\nsbbase=240\nirq=5\ndma=3\nopl=opl2\nsoundfont=gm.sf2\ngus=false\n",
            Path::new("/cfg"),
            None,
        );
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
        let sb = config.sound.card().unwrap();
        assert_eq!((sb.model, sb.base, sb.irq, sb.dma8), (crate::sb::SbModel::SbPro2, 0x240, 5, 3));
        assert!(!config.sound.opl3);
        assert_eq!(config.sound.soundfont.as_deref(), Some(Path::new("/cfg/gm.sf2")));
        assert_eq!(sb.blaster(), "A240 I5 D3 T4");

        let config = parse("[sound]\nsbtype=none\nirq=4\n", Path::new("/cfg"), None);
        assert_eq!(config.sound.card(), None);
        assert!(config.warnings[0].contains("invalid irq '4'"));
    }

    #[test]
    fn ultrasound_settings() {
        let config = parse(
            "[sound]\ngusbase=260\ngusirq=11\ngusdma=6\nultradir=D:\\GUS\nmidisynth=gus\n",
            Path::new("/cfg"),
            None,
        );
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
        let gus = config.sound.ultrasound().unwrap();
        assert_eq!((gus.base, gus.irq, gus.dma, gus.ultradir().as_str()), (0x260, 11, 6, "D:\\GUS"));
        assert_eq!(gus.ultrasnd(), "260,6,6,11,11");
        assert!(!gus.builtin());
        assert_eq!(config.sound.midisynth, MidiSynth::Gus);

        let config = parse("[sound]\ngus=off\n", Path::new("/cfg"), None);
        assert_eq!(config.sound.ultrasound(), None);

        let config = parse(
            "[sound]\ngusbase=230\ngusirq=4\ngusdma=2\nmidisynth=mt32\ngusdrive=Z\n",
            Path::new("/cfg"),
            None,
        );
        assert_eq!(config.warnings.len(), 5, "{:?}", config.warnings);
        assert_eq!(config.sound.ultrasound(), Some(crate::gus::GusConfig::default()));
    }

    #[test]
    fn the_builtin_ultrasound_drive() {
        // X:, where ULTRADIR points unless it is set.
        let gus = parse("", Path::new("/cfg"), None).sound.gus;
        assert_eq!((gus.drive, gus.ultradir().as_str(), gus.builtin()), (Some(23), "X:\\ULTRASND", true));
        let gus = parse("[sound]\ngusdrive=u:\n", Path::new("/cfg"), None).sound.gus;
        assert_eq!((gus.drive, gus.ultradir().as_str()), (Some(20), "U:\\ULTRASND"));
        let gus = parse("[sound]\ngusdrive=none\n", Path::new("/cfg"), None).sound.gus;
        assert_eq!((gus.drive, gus.ultradir().as_str(), gus.builtin()), (None, "C:\\ULTRASND", false));
        let gus = parse("[sound]\nultradir=D:\\GUS\n", Path::new("/cfg"), None).sound.gus;
        assert_eq!((gus.drive, gus.ultradir().as_str(), gus.builtin()), (Some(23), "D:\\GUS", false));

        for bad in ["C", "Z", "A:", "XY", ""] {
            let config = parse(&format!("[sound]\ngusdrive={}\n", bad), Path::new("/cfg"), None);
            assert_eq!(config.warnings.len(), 1, "{}: {:?}", bad, config.warnings);
            assert_eq!(config.sound.gus.drive, Some(23));
        }

        // A drive of the user's own on X: wins.
        let config = parse("[sound]\n[drives]\nX=/games\n", Path::new("/cfg"), None);
        assert!(config.warnings[0].contains("gusdrive X:"), "{:?}", config.warnings);
        assert_eq!(config.sound.gus.ultradir(), "C:\\ULTRASND");
        let config = parse("[sound]\ngus=false\n[drives]\nX=/games\n", Path::new("/cfg"), None);
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
    }

    #[test]
    fn ultrasound_conflicts_with_the_sound_blaster() {
        let config = parse("[sound]\ngusbase=220\n", Path::new("/cfg"), None);
        assert_eq!(config.sound.ultrasound(), None);
        assert!(config.warnings[0].contains("gusbase 220"), "{:?}", config.warnings);

        let config = parse("[sound]\ngusirq=7\ngusdma=1\n", Path::new("/cfg"), None);
        assert!(config.sound.ultrasound().is_some());
        assert_eq!(config.warnings.len(), 2, "{:?}", config.warnings);

        // Without a Sound Blaster, 220h is free.
        let config = parse("[sound]\nsbtype=none\ngusbase=220\n", Path::new("/cfg"), None);
        assert_eq!(config.sound.ultrasound().map(|g| g.base), Some(0x220));
    }

    #[test]
    fn memsize_range() {
        let config = parse("[emulator]\nmemsize=32\n", Path::new("/cfg"), None);
        assert_eq!(config.memsize, Some(32));
        let config = parse("[emulator]\nmemsize=128\n", Path::new("/cfg"), None);
        assert_eq!(config.memsize, None);
        assert!(config.warnings[0].contains("invalid memsize '128'"));
    }

    #[test]
    fn cpu_model() {
        let config = parse("[emulator]\ncpu=386\n", Path::new("/cfg"), None);
        assert_eq!(config.cpu, Some(crate::cpu::CpuModel::I386));
        let config = parse("[emulator]\ncpu=8086\n", Path::new("/cfg"), None);
        assert_eq!(config.cpu, None);
        assert!(config.warnings[0].contains("invalid cpu '8086'"));
    }

    #[test]
    fn cycles_accepts_max_and_rejects_nonsense() {
        let config = parse("[emulator]\ncycles=Max\n", Path::new("/cfg"), None);
        assert_eq!(config.cycles, Some(CpuSpeed::Max));

        let config = parse("[emulator]\ncycles=fast\n", Path::new("/cfg"), None);
        assert_eq!(config.cycles, None);
        assert_eq!(config.warnings.len(), 1);
        assert!(config.warnings[0].starts_with("line 2: invalid cycles 'fast'"));
    }

    #[test]
    fn template_is_all_comments() {
        let config = parse(TEMPLATE, Path::new("/cfg"), None);
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
        assert!(config.drives.is_empty());
        assert!(config.autoexec.is_empty());
        assert_eq!(config.scale, None);
        assert_eq!(config.cycles, None);
        assert_eq!((config.fullscreen, config.aspect, config.filter), (None, None, None));
        assert_eq!(config.sound, SoundConfig::default());
    }

    #[test]
    fn display_settings() {
        let config = parse("[emulator]\nfullscreen=yes\naspect=off\nfilter=Linear\n", Path::new("/cfg"), None);
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
        assert_eq!((config.fullscreen, config.aspect, config.filter), (Some(true), Some(false), Some(Filter::Linear)));
        let config = parse("[emulator]\nfullscreen=maybe\nfilter=blur\n", Path::new("/cfg"), None);
        assert_eq!(config.warnings.len(), 2, "{:?}", config.warnings);
        assert_eq!(Settings::from_config(&config), Settings::default());
    }

    /// Settings with every value away from its default.
    fn changed_settings() -> Settings {
        let sound = SoundConfig {
            sb: crate::sb::SbConfig { model: crate::sb::SbModel::SbPro2, base: 0x240, irq: 5, dma8: 3, dma16: 6 },
            sb_installed: true,
            opl3: false,
            soundfont: Some(PathBuf::from("/home/u/sf/General User.sf2")),
            gus: crate::gus::GusConfig {
                enabled: true,
                base: 0x260,
                irq: 11,
                dma: 6,
                drive: Some(20),
                ultradir: Some("D:\\GUS".to_string()),
            },
            midisynth: MidiSynth::Gus,
        };
        Settings {
            scale: 3,
            fullscreen: true,
            aspect: true,
            filter: Filter::Linear,
            cycles: CpuSpeed::Fixed(3000),
            cpu: CpuModel::I386,
            memsize: 32,
            sound,
            disk: DiskSettings {
                hard_disk_speed: DiskSpeed::Medium,
                floppy_disk_speed: DiskSpeed::Slow,
                hard_disk_noise: NoiseMode::On,
                floppy_disk_noise: NoiseMode::SeekOnly,
            },
        }
    }

    fn drive_specs() -> Vec<MountSpec> {
        use crate::disk::MountOptions;
        vec![
            MountSpec { drive: 2, path: "/home/u/dos".into(), opts: MountOptions::default() },
            MountSpec {
                drive: 3,
                path: "/home/u/cd images/game.cue".into(),
                opts: MountOptions { kind: DriveKind::CdRom, label: Some("GAME".into()), read_only: false, ..Default::default() },
            },
            MountSpec {
                drive: 0,
                path: "/home/u/disks/disk 1.img".into(),
                opts: MountOptions {
                    kind: DriveKind::Floppy,
                    more_images: vec!["/home/u/disks/disk 2.img".into()],
                    ..Default::default()
                },
            },
        ]
    }

    #[test]
    fn saved_settings_parse_back() {
        let home = Path::new("/home/u");
        let settings = changed_settings();
        let changes: Vec<DriveChange> = drive_specs().into_iter().map(|s| (s.drive, Some(s))).collect();
        let text = update_text(TEMPLATE, &Settings::default(), &settings, &changes, Some(home));
        let config = parse(&text, Path::new("/cfg"), Some(home));
        assert!(config.warnings.is_empty(), "{:?}\n{}", config.warnings, text);
        assert_eq!(Settings::from_config(&config), settings);
        assert_eq!(config.drives, drive_specs());

        // The template's examples and comments stay, each new line right
        // after its example.
        for line in TEMPLATE.lines() {
            assert!(text.contains(line), "lost '{}'", line);
        }
        assert!(text.contains("#scale=2\nscale=3\n"), "{}", text);
        assert!(text.contains("#sbtype=sb16\nsbtype=sbpro2\n"), "{}", text);
        assert!(text.contains("#E=~/dos/images/game.cue\nC=~/dos\nD=\"~/cd images/game.cue\" cdrom -label GAME\n"), "{}", text);
        assert!(text.contains("soundfont=~/sf/General User.sf2\n"), "{}", text);

        // Saving the same again changes nothing.
        assert_eq!(update_text(&text, &settings, &settings, &changes, Some(home)), text);
    }

    #[test]
    fn unchanged_settings_leave_the_file_alone() {
        let settings = changed_settings();
        assert_eq!(update_text(TEMPLATE, &settings, &settings, &[], None), TEMPLATE);
        assert_eq!(update_text("", &settings, &settings, &[], None), "");
        assert_eq!(update_text("[emulator]\nscale=2", &settings, &settings, &[], None), "[emulator]\nscale=2");
    }

    #[test]
    fn existing_lines_change_in_place() {
        let text = "\u{FEFF}[Emulator]\r\nScale = 2\r\ncycles=max\r\n[sound]\r\nsoundfont=gm.sf2\r\n\r\n[drives]\r\nc: = /old\r\n# keep\r\nD=/cd cdrom\r\nd=/cd2\r\n[autoexec]\r\nC:\r\n";
        let sound = SoundConfig { soundfont: Some("gm.sf2".into()), ..SoundConfig::default() };
        let baseline = Settings { scale: 2, sound, ..Settings::default() };
        let settings = Settings { scale: 4, ..Settings::default() };
        let spec = |drive, path: &str| Some(MountSpec { drive, path: path.into(), opts: Default::default() });
        let changes = [(2, spec(2, "/new")), (3, None), (4, spec(4, "/e"))];
        assert_eq!(
            update_text(text, &baseline, &settings, &changes, None),
            // A cleared setting is commented out; a removed drive loses all
            // its lines; a new one goes after the other drives.
            "\u{FEFF}[Emulator]\r\nScale = 4\r\ncycles=max\r\n[sound]\r\n#soundfont=gm.sf2\r\n\r\n[drives]\r\nc: = /new\r\nE=/e\r\n# keep\r\n[autoexec]\r\nC:\r\n"
        );
    }

    #[test]
    fn missing_sections_go_before_autoexec() {
        let settings = Settings { memsize: 8, ..Settings::default() };
        let base = Settings::default();
        let changes = [(3, Some(MountSpec { drive: 3, path: "/d".into(), opts: Default::default() }))];
        assert_eq!(
            update_text("[autoexec]\nDIR\n", &base, &settings, &changes, None),
            "[emulator]\nmemsize=8\n\n[drives]\nD=/d\n\n[autoexec]\nDIR\n"
        );
        assert_eq!(update_text("# mine\n", &base, &settings, &[], None), "# mine\n\n[emulator]\nmemsize=8\n");
        // A second [emulator] block gets the new line.
        assert_eq!(
            update_text("[emulator]\nscale=2\n[sound]\n[emulator]\ncpu=386\n", &base, &settings, &[], None),
            "[emulator]\nscale=2\n[sound]\n[emulator]\ncpu=386\nmemsize=8\n"
        );
    }

    #[test]
    fn save_replaces_the_file() {
        let base = scratch("save");
        let path = base.join(FILE_NAME);
        fs::write(&path, "# my settings\n[emulator]\nscale=2\n").unwrap();
        let before = Settings { scale: 2, ..Settings::default() };
        let settings = Settings { scale: 3, ..Settings::default() };
        save(&path, &before, &settings, &[], None).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "# my settings\n[emulator]\nscale=3\n");
        assert_eq!(fs::read_dir(&base).unwrap().count(), 1);

        // A file that has gone missing starts over from the template.
        fs::remove_file(&path).unwrap();
        save(&path, &before, &settings, &[], None).unwrap();
        assert!(fs::read_to_string(&path).unwrap().contains("#scale=2\nscale=3\n"));
    }

    #[test]
    fn load_writes_template_once_and_reads_local_files() {
        let base = scratch("load");
        let cwd = base.join("work");
        fs::create_dir_all(&cwd).unwrap();
        let default = base.join("cfg/rust-dos/rust-dos.conf");

        let config = load(None, &cwd, Some(default.clone()), None).unwrap();
        assert!(config.created);
        assert_eq!(config.source.as_deref(), Some(default.as_path()));
        assert_eq!(fs::read_to_string(&default).unwrap(), TEMPLATE);

        // Second start uses the (edited) file and doesn't rewrite it
        fs::write(&default, "[emulator]\nscale=2\n").unwrap();
        let config = load(None, &cwd, Some(default.clone()), None).unwrap();
        assert!(!config.created);
        assert_eq!(config.scale, Some(2));

        // A file in the working directory wins, with paths relative to it
        fs::write(cwd.join(FILE_NAME), "[drives]\nA=disks/a floppy\nscale=9\n").unwrap();
        let config = load(None, &cwd, Some(default.clone()), None).unwrap();
        assert_eq!(config.scale, None);
        assert_eq!(config.drive(0).unwrap().path, cwd.join("disks/a"));
        assert_eq!(config.warnings.len(), 1);
        assert!(config.warnings[0].contains("rust-dos.conf: line 3"));

        assert!(load(Some(Path::new("nope.conf")), &cwd, Some(default), None).is_err());
    }

    #[test]
    fn unwritable_default_is_only_a_warning() {
        let base = scratch("unwritable");
        let blocker = base.join("file");
        fs::write(&blocker, "").unwrap();
        // The parent "directory" is a file, so creating it fails
        let config = load(None, &base, Some(blocker.join("rust-dos.conf")), None).unwrap();
        assert!(!config.created);
        assert_eq!(config.warnings.len(), 1);
        assert!(config.drives.is_empty());
    }
}
