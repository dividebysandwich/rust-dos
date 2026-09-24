//! Parsing of drive mount specifications, shared by the `MOUNT` shell
//! command and the `[drives]` section of the config file, and of the
//! DOSBox-style `IMGMOUNT` command.
//!
//! A mount spec is `<host path> [type] [-t type] [-label NAME] [-ro]` where
//! type is `floppy`, `hdd` (alias `dir`) or `cdrom` (alias `iso`). The path
//! is a directory, or a CD image (.cue, .iso, .bin, .img), which always
//! makes a CD-ROM drive.

use crate::disk::{DRIVE_Z, DriveKind, LASTDRIVE, MountOptions};
use std::path::{Path, PathBuf};

pub const MOUNT_USAGE: &str =
    "Usage: MOUNT [drive path [floppy|hdd|cdrom] [-label NAME] [-ro]]\r\n       MOUNT -u drive\r\n";

pub const IMGMOUNT_USAGE: &str =
    "Usage: IMGMOUNT drive image [-t cdrom|iso] [-label NAME]\r\n       IMGMOUNT -u drive\r\n";

/// A parsed request to mount `path` as `drive`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountSpec {
    pub drive: u8,
    pub path: PathBuf,
    pub opts: MountOptions,
}

/// What a `MOUNT` command line asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MountCmd {
    List,
    Mount(MountSpec),
    Unmount(u8),
}

/// Split a line into whitespace-separated tokens. Double quotes group text
/// containing spaces and are removed; backslashes are literal so Windows
/// paths need no escaping.
pub fn tokenize(line: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_token = false;
    let mut in_quotes = false;
    for c in line.chars() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                in_token = true;
            }
            c if c.is_whitespace() && !in_quotes => {
                if in_token {
                    tokens.push(std::mem::take(&mut current));
                    in_token = false;
                }
            }
            c => {
                current.push(c);
                in_token = true;
            }
        }
    }
    if in_quotes {
        return Err("Unterminated quote".to_string());
    }
    if in_token {
        tokens.push(current);
    }
    Ok(tokens)
}

pub fn parse_kind(s: &str) -> Option<DriveKind> {
    match s.to_ascii_lowercase().as_str() {
        "floppy" => Some(DriveKind::Floppy),
        "hdd" | "dir" => Some(DriveKind::HardDisk),
        "cdrom" | "iso" => Some(DriveKind::CdRom),
        _ => None,
    }
}

/// "d", "D" or "D:" -> 3. Z: is reserved and rejected by the mount itself.
pub fn parse_drive_letter(s: &str) -> Option<u8> {
    let s = s.strip_suffix(':').unwrap_or(s);
    let b = s.as_bytes();
    (b.len() == 1 && b[0].is_ascii_alphabetic())
        .then(|| b[0].to_ascii_uppercase() - b'A')
        .filter(|&d| d < LASTDRIVE)
}

/// Turn a user-supplied host path into a usable one: `~` expands to the
/// home directory and relative paths are taken relative to `base`.
pub fn expand_host_path(raw: &str, base: &Path, home: Option<&Path>) -> PathBuf {
    if let Some(home) = home {
        if raw == "~" {
            return home.to_path_buf();
        }
        if let Some(rest) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) {
            return home.join(rest);
        }
    }
    let path = Path::new(raw);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

/// Parse `<path> [options]` for `drive`.
pub fn parse_mount_spec(
    drive: u8,
    tokens: &[String],
    base: &Path,
    home: Option<&Path>,
) -> Result<MountSpec, String> {
    if drive == DRIVE_Z {
        return Err("Drive Z: is reserved".to_string());
    }
    let (raw_path, options) = tokens
        .split_first()
        .ok_or_else(|| "Missing host directory or CD image".to_string())?;

    let mut opts = MountOptions::default();
    let mut iter = options.iter();
    while let Some(token) = iter.next() {
        match token.to_ascii_lowercase().as_str() {
            "-t" => {
                let value = iter.next().ok_or("-t needs a drive type")?;
                opts.kind =
                    parse_kind(value).ok_or_else(|| format!("Unknown drive type '{}'", value))?;
            }
            "-label" => {
                let value = iter.next().ok_or("-label needs a name")?;
                opts.label = Some(value.clone());
            }
            "-ro" => opts.read_only = true,
            other => {
                opts.kind =
                    parse_kind(other).ok_or_else(|| format!("Unknown option '{}'", token))?;
            }
        }
    }

    Ok(MountSpec {
        drive,
        path: expand_host_path(raw_path, base, home),
        opts,
    })
}

/// Parse the arguments of a `MOUNT` command. Relative paths resolve against
/// `cwd` (the emulator's working directory).
pub fn parse_mount_command(
    args: &str,
    cwd: &Path,
    home: Option<&Path>,
) -> Result<MountCmd, String> {
    let tokens = tokenize(args)?;
    let Some(first) = tokens.first() else {
        return Ok(MountCmd::List);
    };
    if first.eq_ignore_ascii_case("-u") {
        return match tokens.get(1).and_then(|t| parse_drive_letter(t)) {
            Some(drive) if tokens.len() == 2 => Ok(MountCmd::Unmount(drive)),
            _ => Err("MOUNT -u needs a drive letter".to_string()),
        };
    }
    let drive =
        parse_drive_letter(first).ok_or_else(|| format!("Invalid drive letter '{}'", first))?;
    parse_mount_spec(drive, &tokens[1..], cwd, home).map(MountCmd::Mount)
}

/// Parse the arguments of an `IMGMOUNT` command, DOSBox's command for disk
/// images: `IMGMOUNT d image [-t cdrom|iso] [-fs iso] [-label NAME]`. Only
/// CD images can be mounted. `locate` finds the image by its DOS path
/// (as DOSBox batch files name it, "C:\GAME\CD\GAME.CUE"); a host path
/// relative to `cwd` is the fallback.
pub fn parse_imgmount_command(
    args: &str,
    locate: &dyn Fn(&str) -> Option<PathBuf>,
    cwd: &Path,
    home: Option<&Path>,
) -> Result<MountCmd, String> {
    let tokens = tokenize(args)?;
    let Some(first) = tokens.first() else {
        return Err("IMGMOUNT needs a drive and an image".to_string());
    };
    if first.eq_ignore_ascii_case("-u") {
        return match tokens.get(1).and_then(|t| parse_drive_letter(t)) {
            Some(drive) if tokens.len() == 2 => Ok(MountCmd::Unmount(drive)),
            _ => Err("IMGMOUNT -u needs a drive letter".to_string()),
        };
    }
    let drive =
        parse_drive_letter(first).ok_or_else(|| format!("Invalid drive letter '{}'", first))?;
    if drive == DRIVE_Z {
        return Err("Drive Z: is reserved".to_string());
    }
    let mut opts = MountOptions { kind: DriveKind::CdRom, read_only: true, ..Default::default() };
    let mut image = None;
    let mut iter = tokens[1..].iter();
    while let Some(token) = iter.next() {
        match token.to_ascii_lowercase().as_str() {
            "-t" => {
                let value = iter.next().ok_or("-t needs a type")?;
                if !matches!(value.to_ascii_lowercase().as_str(), "cdrom" | "iso") {
                    return Err("Only CD-ROM images are supported".to_string());
                }
            }
            "-fs" => {
                let value = iter.next().ok_or("-fs needs a file system")?;
                if !value.eq_ignore_ascii_case("iso") {
                    return Err("Only CD-ROM images are supported".to_string());
                }
            }
            "-label" => opts.label = Some(iter.next().ok_or("-label needs a name")?.clone()),
            "-ro" => {}
            other if other.starts_with('-') => return Err(format!("Unknown option '{}'", token)),
            _ if image.is_some() => return Err("Only one image at a time can be mounted".to_string()),
            _ => image = Some(locate(token).unwrap_or_else(|| expand_host_path(token, cwd, home))),
        }
    }
    let path = image.ok_or_else(|| "Missing image".to_string())?;
    Ok(MountCmd::Mount(MountSpec { drive, path, opts }))
}

/// Host path for display, without Windows' `\\?\` verbatim prefix that
/// `canonicalize` adds.
pub fn display_host_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
}

/// Host path as the user would write it: under the home directory it
/// starts with `~/`, which `expand_host_path` turns back into the path.
pub fn contract_home(path: &Path, home: Option<&Path>) -> String {
    let display = display_host_path(path);
    let Some(home) = home else { return display };
    match Path::new(&display).strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => display,
    }
}

/// A token `tokenize` reads back as `s`: quoted if it contains whitespace.
fn quote(s: &str) -> String {
    if s.is_empty() || s.contains(char::is_whitespace) {
        format!("\"{}\"", s)
    } else {
        s.to_string()
    }
}

/// The `<host path> [type] [-label NAME] [-ro]` text of a mount, as the
/// `[drives]` section and `MOUNT` take it: the inverse of
/// `parse_mount_spec`.
pub fn mount_spec_value(spec: &MountSpec, home: Option<&Path>) -> String {
    let mut value = quote(&contract_home(&spec.path, home));
    if spec.opts.kind != DriveKind::HardDisk {
        value.push(' ');
        value.push_str(spec.opts.kind.name());
    }
    if let Some(label) = spec.opts.label.as_deref().filter(|l| !l.is_empty()) {
        value.push_str(" -label ");
        value.push_str(&quote(label));
    }
    if spec.opts.read_only {
        value.push_str(" -ro");
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<String> {
        tokenize(s).unwrap()
    }

    #[test]
    fn tokenizer_groups_quotes_and_keeps_backslashes() {
        assert_eq!(toks(r#"  a  "b c"  d"#), ["a", "b c", "d"]);
        assert_eq!(toks(r"C:\Games\Dos floppy"), [r"C:\Games\Dos", "floppy"]);
        assert_eq!(toks(r#""""#), [""]);
        assert!(tokenize(r#"a "b"#).is_err());
        assert!(toks("").is_empty());
    }

    #[test]
    fn drive_letters() {
        assert_eq!(parse_drive_letter("d"), Some(3));
        assert_eq!(parse_drive_letter("A:"), Some(0));
        assert_eq!(parse_drive_letter("dd"), None);
        assert_eq!(parse_drive_letter("1"), None);
    }

    #[test]
    fn paths_expand_home_and_base() {
        let home = Path::new("/home/u");
        let base = Path::new("/cfg");
        assert_eq!(expand_host_path("~", base, Some(home)), home);
        assert_eq!(
            expand_host_path("~/dos", base, Some(home)),
            home.join("dos")
        );
        assert_eq!(
            expand_host_path("games", base, Some(home)),
            base.join("games")
        );
        assert_eq!(expand_host_path("~/x", base, None), base.join("~/x"));
        #[cfg(unix)]
        assert_eq!(
            expand_host_path("/abs", base, Some(home)),
            Path::new("/abs")
        );
    }

    #[test]
    fn mount_options() {
        let base = Path::new("/b");
        let spec = parse_mount_spec(3, &toks("cd cdrom -label game1 -ro"), base, None).unwrap();
        assert_eq!(spec.path, base.join("cd"));
        assert_eq!(spec.opts.kind, DriveKind::CdRom);
        assert_eq!(spec.opts.label.as_deref(), Some("game1"));
        assert!(spec.opts.read_only);

        let spec = parse_mount_spec(0, &toks("fl -t floppy"), base, None).unwrap();
        assert_eq!(spec.opts.kind, DriveKind::Floppy);
        let spec = parse_mount_spec(4, &toks("x dir"), base, None).unwrap();
        assert_eq!(spec.opts.kind, DriveKind::HardDisk);

        assert!(parse_mount_spec(3, &[], base, None).is_err());
        assert!(parse_mount_spec(3, &toks("x zip"), base, None).is_err());
        let spec = parse_mount_spec(3, &toks("game.cue iso"), base, None).unwrap();
        assert_eq!(spec.opts.kind, DriveKind::CdRom);
        assert!(parse_mount_spec(3, &toks("x -t"), base, None).is_err());
        assert!(parse_mount_spec(25, &toks("x"), base, None).is_err());
    }

    #[test]
    fn mount_commands() {
        let cwd = Path::new("/w");
        assert_eq!(parse_mount_command("", cwd, None), Ok(MountCmd::List));
        assert_eq!(
            parse_mount_command("-u d:", cwd, None),
            Ok(MountCmd::Unmount(3))
        );
        assert!(parse_mount_command("-u", cwd, None).is_err());
        assert!(parse_mount_command("dd x", cwd, None).is_err());
        match parse_mount_command(r#"e "my dir" floppy"#, cwd, None).unwrap() {
            MountCmd::Mount(spec) => {
                assert_eq!(spec.drive, 4);
                assert_eq!(spec.path, cwd.join("my dir"));
                assert_eq!(spec.opts.kind, DriveKind::Floppy);
            }
            other => panic!("{:?}", other),
        }
    }

    #[test]
    fn imgmount_commands() {
        let cwd = Path::new("/w");
        let locate = |p: &str| (p == r"C:\DOTT\CD\DAYOFT~2.CUE").then(|| PathBuf::from("/games/dott/cd/x.cue"));
        match parse_imgmount_command(r"d C:\DOTT\CD\DAYOFT~2.CUE -t cdrom", &locate, cwd, None).unwrap() {
            MountCmd::Mount(spec) => {
                assert_eq!(spec.drive, 3);
                assert_eq!(spec.path, Path::new("/games/dott/cd/x.cue"));
                assert_eq!(spec.opts.kind, DriveKind::CdRom);
            }
            other => panic!("{:?}", other),
        }
        match parse_imgmount_command("e game.iso -fs iso -label GAME", &locate, cwd, None).unwrap() {
            MountCmd::Mount(spec) => {
                assert_eq!(spec.path, cwd.join("game.iso"));
                assert_eq!(spec.opts.label.as_deref(), Some("GAME"));
            }
            other => panic!("{:?}", other),
        }
        assert_eq!(parse_imgmount_command("-u d", &locate, cwd, None), Ok(MountCmd::Unmount(3)));
        assert!(parse_imgmount_command("d disk.img -t hdd", &locate, cwd, None).is_err());
        assert!(parse_imgmount_command("d a.cue b.cue", &locate, cwd, None).is_err());
        assert!(parse_imgmount_command("d", &locate, cwd, None).is_err());
        assert!(parse_imgmount_command("", &locate, cwd, None).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn mount_spec_values_parse_back() {
        let home = Path::new("/home/u");
        let base = Path::new("/cfg");
        let specs = [
            MountSpec { drive: 2, path: "/home/u/dos".into(), opts: MountOptions::default() },
            MountSpec {
                drive: 0,
                path: "/home/u/My Disks/a".into(),
                opts: MountOptions { kind: DriveKind::Floppy, label: Some("DISK 1".into()), read_only: true },
            },
            MountSpec {
                drive: 3,
                path: "/games/cd.cue".into(),
                opts: MountOptions { kind: DriveKind::CdRom, label: None, read_only: false },
            },
        ];
        for spec in specs {
            let value = mount_spec_value(&spec, Some(home));
            let back = parse_mount_spec(spec.drive, &toks(&value), base, Some(home)).unwrap();
            assert_eq!(back, spec, "{}", value);
        }
        assert_eq!(mount_spec_value(&MountSpec { drive: 2, path: "/home/u".into(), opts: MountOptions::default() }, Some(home)), "~");
        assert_eq!(
            mount_spec_value(&MountSpec { drive: 2, path: "/home/u/x y".into(), opts: MountOptions::default() }, None),
            "\"/home/u/x y\""
        );
    }

    #[test]
    fn display_strips_verbatim_prefix() {
        assert_eq!(display_host_path(Path::new(r"\\?\C:\dos")), r"C:\dos");
        assert_eq!(display_host_path(Path::new("/home/u")), "/home/u");
    }
}
