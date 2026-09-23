//! The rust-dos configuration file: a DOSBox-style INI file with
//! `[emulator]`, `[drives]` and `[autoexec]` sections. See
//! `rust-dos.conf.example` for the format.
//!
//! Lookup order, first match wins: `--config FILE`, `./rust-dos.conf`, then
//! `rust-dos.conf` in the per-user configuration directory, where a
//! commented template is written on first start.

use crate::disk::DRIVE_Z;
use crate::mount::{MountSpec, parse_drive_letter, parse_mount_spec, tokenize};
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
    /// Emulated CPU speed (`cycles`).
    pub cycles: Option<CpuSpeed>,
    /// Emulated processor (`cpu`).
    pub cpu: Option<crate::cpu::CpuModel>,
    /// RAM in MB (`memsize`).
    pub memsize: Option<usize>,
    /// `[sound]`: the Sound Blaster (None: `sbtype=none`), the FM chip,
    /// the Gravis Ultrasound, and the MPU-401's synthesizer.
    pub sound: SoundConfig,
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    None,
    Emulator,
    Sound,
    Drives,
    Autoexec,
    Unknown,
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
    fn check(&mut self) -> Vec<String> {
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
                self.gus.enabled = match value.to_ascii_lowercase().as_str() {
                    "true" | "on" | "yes" | "1" => true,
                    "false" | "off" | "no" | "0" => false,
                    _ => return Err(format!("invalid gus '{}' (true or false)", value)),
                }
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
            section = match name.trim().to_ascii_lowercase().as_str() {
                "emulator" => Section::Emulator,
                "sound" => Section::Sound,
                "drives" => Section::Drives,
                "autoexec" => Section::Autoexec,
                other => {
                    warn(format!("unknown section [{}]", other));
                    Section::Unknown
                }
            };
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
                        "cycles" => match CpuSpeed::parse(value) {
                            Ok(speed) => config.cycles = Some(speed),
                            Err(e) => warn(e),
                        },
                        "memsize" => match value.parse::<usize>() {
                            Ok(mb) if (2..=64).contains(&mb) => config.memsize = Some(mb),
                            _ => warn(format!("invalid memsize '{}' (2 to 64 MB)", value)),
                        },
                        "cpu" => match value.to_ascii_lowercase().as_str() {
                            "386" => config.cpu = Some(crate::cpu::CpuModel::I386),
                            "486" => config.cpu = Some(crate::cpu::CpuModel::I486),
                            _ => warn(format!("invalid cpu '{}' (386 or 486)", value)),
                        },
                        _ => warn(format!("unknown setting '{}'", key)),
                    }
                    continue;
                }

                if section == Section::Sound {
                    if let Err(e) = config.sound.set(key, value, base_dir, home) {
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
        assert_eq!(config.sound, SoundConfig::default());
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
