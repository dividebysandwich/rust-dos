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

/// Per-user default: `<config dir>/rust-dos/rust-dos.conf`, e.g.
/// `~/.config/rust-dos/rust-dos.conf` on Linux.
pub fn default_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("rust-dos").join(FILE_NAME))
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
    Drives,
    Autoexec,
    Unknown,
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
            Section::Emulator | Section::Drives => {
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
            E=/x iso\n\
            nonsense\n\
            [sound]\n\
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
            "line 10: drive E: Unknown option 'iso'",
            "line 11: expected key=value",
            "line 12: unknown section [sound]",
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
