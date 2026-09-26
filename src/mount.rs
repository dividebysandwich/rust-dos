//! Parsing of the `MOUNT` command, and of the drive mount specifications of
//! the `[drives]` section of the config file.
//!
//! `MOUNT` takes DOSBox Staging's syntax, in which MOUNT took in IMGMOUNT:
//! `MOUNT drive path [path ...] [options]`, with the options anywhere on
//! the line. The path is a host directory, or a disk or CD image (.img,
//! .ima, .vfd, .flp, .dsk, .360, .720, .1200, .1440, .iso, .cue, .bin,
//! .gog, .ins) on the host or on a mounted drive (C:\GAME\CD.CUE), whose
//! type is found from the image unless `-t` gives it. Several images, or a
//! wildcard that matches several (disk*.img), make a list that Ctrl+F4
//! steps through. A: and B: are floppies whatever the type says. `IMGMOUNT`
//! is the same command, for the batch files made for DOSBox.
//!
//! A mount spec, as `[drives]` takes it, is the same without the drive:
//! `<host path> [more images] [type] [-t type] [-label NAME] [-ro]
//! [-chs C,H,S]` where type is `floppy` (alias `fdd`), `hdd` (alias `dir`)
//! or `cdrom` (alias `iso`).

use crate::disk::{DRIVE_Z, DriveKind, LASTDRIVE, MountOptions};
use crate::diskimage::Chs;
use std::cmp::Ordering;
use std::path::{Component, Path, PathBuf};

pub const MOUNT_USAGE: &str = "\
Usage: MOUNT drive directory [-t floppy|hdd|cdrom] [-label NAME] [-ro]\r
       MOUNT drive image [image ...] [-t floppy|hdd|cdrom] [-label NAME] [-ro]\r
                         [-chs C,H,S] [-size 512,S,H,C]\r
       MOUNT -u drive\r
       MOUNT             lists the drives\r
An image can be on a mounted drive (C:\\GAME\\CD.CUE), and a wildcard\r
(disk*.img) mounts the images that match. Ctrl+F4 puts the next image of a\r
list in. -pr takes relative paths from the configuration file's folder.\r
IMGMOUNT is the same command.\r
";

/// The extensions of disk and CD images.
const IMAGE_EXTENSIONS: &[&str] =
    &["img", "ima", "vfd", "flp", "dsk", "360", "720", "1200", "1440", "iso", "cue", "bin", "gog", "ins"];

/// Whether a path is a disk or CD image's, by its extension.
fn is_image_name(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| IMAGE_EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// Parse numbers separated by commas, as -chs and -size take them.
fn numbers(value: &str, count: usize, option: &str) -> Result<Vec<u32>, String> {
    let numbers: Vec<u32> = value.split(',').map(|n| n.trim().parse::<u32>()).collect::<Result<_, _>>().map_err(|_| {
        format!("{} needs {} numbers separated by commas", option, count)
    })?;
    if numbers.len() != count || numbers.contains(&0) {
        return Err(format!("{} needs {} numbers separated by commas", option, count));
    }
    Ok(numbers)
}

/// A hard disk geometry from `-chs cylinders,heads,sectors`.
fn parse_chs(value: &str) -> Result<Chs, String> {
    let n = numbers(value, 3, "-chs")?;
    if n[1] > 255 || n[2] > 63 {
        return Err("-chs takes at most 255 heads and 63 sectors".to_string());
    }
    Ok(Chs { cylinders: n[0], heads: n[1], sectors: n[2] })
}

/// A hard disk geometry from DOSBox's `-size bytes,sectors,heads,cylinders`.
fn parse_size(value: &str) -> Result<Chs, String> {
    let n = numbers(value, 4, "-size")?;
    if n[0] != 512 {
        return Err("Only disks with 512-byte sectors can be mounted".to_string());
    }
    parse_chs(&format!("{},{},{}", n[3], n[2], n[1]))
}

/// The options that take a value, and those that don't. A value can start
/// with a dash (`-label -DISK-`), but can't be one of these.
const VALUE_OPTIONS: &[&str] = &["-t", "-fs", "-label", "-chs", "-size", "-freesize", "-usecd"];
const FLAGS: &[&str] =
    &["-u", "-ro", "-pr", "-ide", "-ioctl", "-noioctl", "-ioctl_dio", "-ioctl_dx", "-ioctl_mci", "-aspi"];

fn is_option(token: &str) -> bool {
    VALUE_OPTIONS.iter().chain(FLAGS).any(|option| token.eq_ignore_ascii_case(option))
}

/// A mount command line taken apart: the words that aren't options (the
/// drive, the paths and the types) and what the options say.
#[derive(Default)]
struct Arguments {
    words: Vec<String>,
    opts: MountOptions,
    unmount: bool,
    /// -pr: relative paths are from the configuration file's folder.
    config_relative: bool,
}

fn parse_arguments(tokens: &[String]) -> Result<Arguments, String> {
    let mut args = Arguments::default();
    let mut iter = tokens.iter().peekable();
    while let Some(token) = iter.next() {
        let option = token.to_ascii_lowercase();
        if VALUE_OPTIONS.contains(&option.as_str()) {
            let value = iter.next_if(|value| !is_option(value)).ok_or_else(|| missing_value(&option))?;
            option_value(&option, value, &mut args.opts)?;
            continue;
        }
        match option.as_str() {
            "-u" => args.unmount = true,
            "-ro" => args.opts.read_only = true,
            "-pr" => args.config_relative = true,
            // DOSBox's IDE controller and its own CD-ROM access, which
            // batch files made for it ask for: taken and ignored.
            "-ide" | "-ioctl" | "-noioctl" | "-ioctl_dio" | "-ioctl_dx" | "-ioctl_mci" | "-aspi" => {}
            _ if token.starts_with('-') => return Err(format!("Unknown option '{}'", token)),
            _ => args.words.push(token.clone()),
        }
    }
    Ok(args)
}

fn missing_value(option: &str) -> String {
    let value = match option {
        "-t" => "a drive type",
        "-fs" => "a file system",
        "-label" => "a name",
        "-chs" | "-size" => "a geometry",
        _ => "a number",
    };
    format!("{} needs {}", option, value)
}

fn option_value(option: &str, value: &str, opts: &mut MountOptions) -> Result<(), String> {
    match option {
        "-t" if value.eq_ignore_ascii_case("overlay") => {
            return Err("Overlay mounts aren't supported".to_string());
        }
        "-t" => opts.kind = parse_kind(value).ok_or_else(|| format!("Unknown drive type '{}'", value))?,
        "-fs" => match value.to_ascii_lowercase().as_str() {
            "fat" => {}
            "iso" => opts.kind = DriveKind::CdRom,
            "none" => return Err("Disk images without a DOS file system can't be mounted".to_string()),
            other => return Err(format!("Unknown file system '{}'", other)),
        },
        "-label" => opts.label = Some(value.to_string()),
        "-chs" => opts.geometry = Some(parse_chs(value)?),
        "-size" => opts.geometry = Some(parse_size(value)?),
        // DOSBox's free space reports and CD-ROM access: taken and ignored.
        _ => {}
    }
    Ok(())
}

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
    Help,
    Mount(MountSpec),
    Unmount(u8),
}

/// Where the paths a mount names are found.
pub struct PathContext<'a> {
    /// The folder relative host paths are taken from.
    pub base: &'a Path,
    /// The configuration file's folder, which -pr takes them from instead.
    pub config_dir: Option<&'a Path>,
    pub home: Option<&'a Path>,
    /// The host path that a DOS path ("C:\GAME\CD.CUE", or "CD.CUE" in the
    /// current directory) names on a mounted drive, whether or not there is
    /// anything there.
    pub locate: &'a dyn Fn(&str) -> Option<PathBuf>,
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
        "floppy" | "fdd" => Some(DriveKind::Floppy),
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

/// The drive a mount is for. DOSBox's drive numbers 0 to 3 are for disks
/// to boot from, which rust-dos doesn't do.
fn parse_drive(word: &str) -> Result<u8, String> {
    match parse_drive_letter(word) {
        Some(drive) => Ok(drive),
        None if matches!(word, "0" | "1" | "2" | "3") => {
            Err(format!("Drive number {} is for booting an image, which rust-dos doesn't do", word))
        }
        None => Err(format!("Invalid drive letter '{}'", word)),
    }
}

/// Turn a user-supplied host path into a usable one: `~` expands to the
/// home directory and relative paths are taken relative to `base`, with
/// `.` and `..` folded away ("../games" from /home/u/dos is /home/u/games).
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
        return path.to_path_buf();
    }
    let mut joined = base.to_path_buf();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir if matches!(joined.components().next_back(), Some(Component::Normal(_))) => {
                joined.pop();
            }
            part => joined.push(part),
        }
    }
    joined
}

/// Parse `<path> [options]` for `drive`, as `[drives]` takes it: host
/// paths, relative to `base`.
pub fn parse_mount_spec(
    drive: u8,
    tokens: &[String],
    base: &Path,
    home: Option<&Path>,
) -> Result<MountSpec, String> {
    let args = parse_arguments(tokens)?;
    if args.unmount {
        return Err("Unknown option '-u'".to_string());
    }
    mount_spec(drive, args, &PathContext { base, config_dir: None, home, locate: &|_| None })
}

/// Parse the arguments of a `MOUNT` (or `IMGMOUNT`) command.
pub fn parse_mount_command(args: &str, paths: &PathContext) -> Result<MountCmd, String> {
    parse_mount_tokens(&tokenize(args)?, paths)
}

/// Parse the arguments of a `MOUNT` command, split into tokens.
pub fn parse_mount_tokens(tokens: &[String], paths: &PathContext) -> Result<MountCmd, String> {
    if tokens.is_empty() {
        return Ok(MountCmd::List);
    }
    if tokens.iter().any(|t| matches!(t.to_ascii_lowercase().as_str(), "/?" | "-?" | "-h" | "--help")) {
        return Ok(MountCmd::Help);
    }
    let mut args = parse_arguments(tokens)?;
    // MOUNT -u d, or MOUNT d -u.
    if args.unmount {
        return match args.words.as_slice() {
            [drive] => parse_drive_letter(drive),
            _ => None,
        }
        .map(MountCmd::Unmount)
        .ok_or_else(|| "MOUNT -u needs a drive letter".to_string());
    }
    if args.words.is_empty() {
        return Err("Missing drive letter".to_string());
    }
    let drive = parse_drive(&args.words.remove(0))?;
    mount_spec(drive, args, paths).map(MountCmd::Mount)
}

/// The mount of `drive` that the words after the drive and the options ask
/// for.
fn mount_spec(drive: u8, args: Arguments, paths: &PathContext) -> Result<MountSpec, String> {
    if drive == DRIVE_Z {
        return Err("Drive Z: is reserved".to_string());
    }
    let Arguments { words, mut opts, config_relative, .. } = args;
    let base = paths.config_dir.filter(|_| config_relative).unwrap_or(paths.base);
    let (first, rest) = words.split_first().ok_or("Missing host directory or disk image")?;
    let mut images = find_paths(first, base, paths)?;
    // The words after an image are more images, or its type.
    let image = images[0].is_file() || is_image_name(&images[0]);
    for word in rest {
        match parse_kind(word) {
            Some(kind) => opts.kind = kind,
            None if image => images.extend(find_paths(word, base, paths)?),
            None => return Err(format!("Unknown option '{}'", word)),
        }
    }
    let path = images.remove(0);
    opts.more_images = images;
    Ok(MountSpec { drive, path, opts })
}

/// The host paths a path on the command line names. An image is looked for
/// on the mounted drives first, as DOSBox looks for it, and a directory on
/// the host, so that a host path means the host's where it could also be a
/// DOS path (C:\GAMES on Windows). A wildcard in the last part of the path
/// names the files that match.
fn find_paths(word: &str, base: &Path, paths: &PathContext) -> Result<Vec<PathBuf>, String> {
    let host = expand_host_path(word, base, paths.home);
    let dos = (paths.locate)(word).filter(|p| p.exists());
    let found = match dos {
        _ if host.is_dir() => host,
        Some(dos) if dos.is_file() || !host.exists() => dos,
        _ if host.exists() || !word.contains(['*', '?']) => host,
        _ => return matching_files(word, base, paths),
    };
    Ok(vec![found])
}

/// The files a wildcard in the last part of a path matches, in natural
/// order (DISK2 before DISK10): in the directory on a mounted drive, or
/// else on the host.
fn matching_files(word: &str, base: &Path, paths: &PathContext) -> Result<Vec<PathBuf>, String> {
    // The pattern is after the last separator, or after a drive (C:*.IMG).
    let split = match word.rfind(['\\', '/']) {
        Some(i) => i + 1,
        None if word.as_bytes().get(1) == Some(&b':') => 2,
        None => 0,
    };
    let (dir, pattern) = word.split_at(split);
    let name = |path: &PathBuf| path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    for dir in [(paths.locate)(dir), Some(expand_host_path(dir, base, paths.home))].into_iter().flatten() {
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| matches_wildcard(&entry.file_name().to_string_lossy(), pattern))
            .map(|entry| entry.path())
            .filter(|path| path.is_file())
            .collect();
        if !files.is_empty() {
            files.sort_by(|a, b| natural_cmp(&name(a), &name(b)));
            return Ok(files);
        }
    }
    Err(format!("No files match {}", word))
}

/// Whether a file name matches a wildcard pattern, in any case: `*` stands
/// for any run of characters and `?` for one, and `*.*` for every name, as
/// in DOS.
fn matches_wildcard(name: &str, pattern: &str) -> bool {
    if pattern == "*.*" {
        return true;
    }
    let name: Vec<char> = name.to_lowercase().chars().collect();
    let pattern: Vec<char> = pattern.to_lowercase().chars().collect();
    let (mut n, mut p) = (0, 0);
    // The last star and where in the name it stands up to; a mismatch
    // after it gives it another character.
    let mut star = None;
    while n < name.len() {
        match pattern.get(p) {
            Some('*') => {
                star = Some((p, n));
                p += 1;
            }
            Some(&c) if c == '?' || c == name[n] => {
                n += 1;
                p += 1;
            }
            _ => match star {
                Some((star_p, star_n)) => {
                    star = Some((star_p, star_n + 1));
                    p = star_p + 1;
                    n = star_n + 1;
                }
                None => return false,
            },
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}

/// Names in the order people count: "disk2" before "disk10", in any case.
fn natural_cmp(a: &str, b: &str) -> Ordering {
    let digits = |s: &[u8]| s.iter().take_while(|c| c.is_ascii_digit()).count();
    let number = |s: &[u8]| -> Vec<u8> { s.iter().copied().skip_while(|&c| c == b'0').collect() };
    let (mut x, mut y) = (a.as_bytes(), b.as_bytes());
    while let (Some(&c), Some(&d)) = (x.first(), y.first()) {
        let order = if c.is_ascii_digit() && d.is_ascii_digit() {
            let (i, j) = (digits(x), digits(y));
            let (m, n) = (number(&x[..i]), number(&y[..j]));
            (x, y) = (&x[i..], &y[j..]);
            m.len().cmp(&n.len()).then(m.cmp(&n))
        } else {
            (x, y) = (&x[1..], &y[1..]);
            c.to_ascii_lowercase().cmp(&d.to_ascii_lowercase())
        };
        if order != Ordering::Equal {
            return order;
        }
    }
    x.len().cmp(&y.len()).then_with(|| a.cmp(b))
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

/// The `<host path> [more images] [type] [-label NAME] [-ro] [-chs C,H,S]`
/// text of a mount, as the `[drives]` section and `MOUNT` take it: the
/// inverse of `parse_mount_spec`.
pub fn mount_spec_value(spec: &MountSpec, home: Option<&Path>) -> String {
    let mut value = quote(&contract_home(&spec.path, home));
    for image in &spec.opts.more_images {
        value.push(' ');
        value.push_str(&quote(&contract_home(image, home)));
    }
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
    if let Some(chs) = spec.opts.geometry {
        value.push_str(&format!(" -chs {},{},{}", chs.cylinders, chs.heads, chs.sectors));
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
        assert_eq!(expand_host_path("./a/../../b", base, None), Path::new("/b"));
        assert_eq!(expand_host_path("../x", Path::new(""), None), Path::new("../x"));
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
        let spec = parse_mount_spec(0, &toks("a.img b.IMA floppy"), base, None).unwrap();
        assert_eq!(spec.opts.more_images, [base.join("b.IMA")]);
        // Only images make lists.
        assert!(parse_mount_spec(3, &toks("dir b.img"), base, None).is_err());
        assert!(parse_mount_spec(3, &toks("x -t"), base, None).is_err());
        assert!(parse_mount_spec(25, &toks("x"), base, None).is_err());
        assert!(parse_mount_spec(3, &toks("x -u"), base, None).is_err());
        let spec = parse_mount_spec(0, &toks("-ro -t fdd fl"), base, None).unwrap();
        assert_eq!((spec.path, spec.opts.kind, spec.opts.read_only), (base.join("fl"), DriveKind::Floppy, true));
    }

    /// Parse a MOUNT line, with relative paths from `cwd` and nothing on
    /// the DOS drives.
    fn mount(line: &str, cwd: &Path) -> Result<MountCmd, String> {
        parse_mount_command(line, &PathContext { base: cwd, config_dir: None, home: None, locate: &|_| None })
    }

    fn mounted(result: Result<MountCmd, String>) -> MountSpec {
        match result {
            Ok(MountCmd::Mount(spec)) => spec,
            other => panic!("{:?}", other),
        }
    }

    /// A scratch folder with these empty files in it.
    fn scratch(name: &str, files: &[&str]) -> PathBuf {
        let dir = PathBuf::from("target/test_mount").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for file in files {
            let path = dir.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        }
        std::fs::canonicalize(&dir).unwrap()
    }

    #[test]
    fn mount_commands() {
        let cwd = Path::new("/w");
        assert_eq!(mount("", cwd), Ok(MountCmd::List));
        assert_eq!(mount("/?", cwd), Ok(MountCmd::Help));
        assert_eq!(mount("-u d:", cwd), Ok(MountCmd::Unmount(3)));
        assert_eq!(mount("D -U", cwd), Ok(MountCmd::Unmount(3)));
        assert!(mount("-u", cwd).is_err());
        assert!(mount("-u d e", cwd).is_err());
        assert!(mount("dd x", cwd).is_err());
        assert!(mount("-ro", cwd).is_err());
        assert!(mount("d", cwd).is_err());
        let spec = mounted(mount(r#"e "my dir" floppy"#, cwd));
        assert_eq!((spec.drive, spec.path, spec.opts.kind), (4, cwd.join("my dir"), DriveKind::Floppy));
        let spec = mounted(mount("a d1.img d2.img d3.img -t floppy -ro", cwd));
        assert_eq!(spec.path, cwd.join("d1.img"));
        assert_eq!(spec.opts.more_images, [cwd.join("d2.img"), cwd.join("d3.img")]);
        assert_eq!((spec.opts.kind, spec.opts.read_only), (DriveKind::Floppy, true));
        let spec = mounted(mount("c hdd.img -size 512,63,16,142 -fs fat", cwd));
        assert_eq!(spec.opts.kind, DriveKind::HardDisk);
        assert_eq!(spec.opts.geometry, Some(Chs { cylinders: 142, heads: 16, sectors: 63 }));
        assert!(mount("c hdd.img -size 1024,63,16,142", cwd).is_err());
        assert!(mount("c hdd.img -chs 10,16", cwd).is_err());
        assert!(mount("a boot.img -fs none", cwd).is_err());
        assert!(mount("d x.img -t zip", cwd).is_err());
        assert!(is_image_name(Path::new("GAME.GOG")) && is_image_name(Path::new("disk.1440")));
    }

    #[test]
    fn mount_takes_dosbox_stagings_syntax() {
        let cwd = Path::new("/w");
        // The options go anywhere.
        let spec = mounted(mount("-t cdrom -label CD1 d ../cd -ro", cwd));
        assert_eq!((spec.drive, spec.path.as_path()), (3, Path::new("/cd")));
        assert_eq!(
            (spec.opts.kind, spec.opts.label.as_deref(), spec.opts.read_only),
            (DriveKind::CdRom, Some("CD1"), true)
        );
        // A value can start with a dash, but can't be another option.
        assert_eq!(mounted(mount("a disk.img -label -DISK-", cwd)).opts.label.as_deref(), Some("-DISK-"));
        assert_eq!(mount("a disk.img -label -ro", cwd), Err("-label needs a name".to_string()));
        assert_eq!(mounted(mount("a disks -t FDD", cwd)).opts.kind, DriveKind::Floppy);
        assert_eq!(mount("c x -t overlay", cwd), Err("Overlay mounts aren't supported".to_string()));
        assert!(mount("0 boot.img -t floppy", cwd).unwrap_err().contains("booting"));
        assert_eq!(mount("c x -foo", cwd), Err("Unknown option '-foo'".to_string()));
        // DOSBox's options that mean nothing here are taken.
        let spec = mounted(mount("d game.ins -t iso -ioctl -ide -freesize 100 -usecd 0", cwd));
        assert_eq!((spec.path, spec.opts.kind), (cwd.join("game.ins"), DriveKind::CdRom));
        assert!(mount("d x.iso -freesize", cwd).is_err());
        // -pr: from the configuration file's folder.
        let paths = PathContext { base: cwd, config_dir: Some(Path::new("/cfg")), home: None, locate: &|_| None };
        assert_eq!(mounted(parse_mount_command("c games -pr", &paths)).path, Path::new("/cfg/games"));
        assert_eq!(mounted(parse_mount_command("c games", &paths)).path, Path::new("/w/games"));
    }

    #[test]
    fn images_are_found_on_the_dos_drives_first_and_directories_on_the_host() {
        let dir = scratch(
            "dos_paths",
            &["c/DOTT/CD/Day Of The Tentacle.cue", "c/game.iso", "host/game.iso", "host/DOTT/readme.txt"],
        );
        let (c, host) = (dir.join("c"), dir.join("host"));
        // C: is the "c" folder, and C:\ the current directory.
        let locate = |p: &str| {
            let rest = p.strip_prefix("C:").unwrap_or(p).trim_start_matches('\\').replace('\\', "/");
            Some(if rest == "DOTT/CD/DAYOFT~2.CUE" { c.join("DOTT/CD/Day Of The Tentacle.cue") } else { c.join(rest) })
        };
        let paths = PathContext { base: &host, config_dir: None, home: None, locate: &locate };
        let parse = |line: &str| mounted(parse_mount_command(line, &paths));
        // As a DOSBox batch file names it, in 8.3 names.
        let spec = parse(r"d C:\DOTT\CD\DAYOFT~2.CUE -t cdrom");
        assert_eq!((spec.path, spec.opts.kind), (c.join("DOTT/CD/Day Of The Tentacle.cue"), DriveKind::CdRom));
        assert_eq!(parse("e game.iso").path, c.join("game.iso"));
        assert_eq!(parse("e other.iso").path, host.join("other.iso"));
        assert_eq!(parse("f DOTT").path, host.join("DOTT"));
        assert_eq!(parse(r"f DOTT\CD").path, c.join("DOTT/CD"));
        // IMGMOUNT's way, a list of images by their DOS paths.
        let spec = parse(r"d C:\game.iso C:\DOTT\CD\DAYOFT~2.CUE");
        assert_eq!(spec.opts.more_images, [c.join("DOTT/CD/Day Of The Tentacle.cue")]);
    }

    #[test]
    fn wildcards_mount_the_images_that_match_in_natural_order() {
        let dir = scratch(
            "wildcards",
            &["disk10.img", "Disk2.IMG", "disk1.img", "disk1.txt", "cd/b.cue", "cd/a.cue", "sub/x.img", "sub/y.img/z"],
        );
        let paths = PathContext { base: &dir, config_dir: None, home: None, locate: &|_| None };
        let spec = mounted(parse_mount_command("a disk*.img -t floppy", &paths));
        assert_eq!(spec.path, dir.join("disk1.img"));
        assert_eq!(spec.opts.more_images, [dir.join("Disk2.IMG"), dir.join("disk10.img")]);
        let spec = mounted(parse_mount_command("d cd/*.cue", &paths));
        assert_eq!((spec.path, spec.opts.more_images), (dir.join("cd/a.cue"), vec![dir.join("cd/b.cue")]));
        // With a list; directories don't match.
        let spec = mounted(parse_mount_command("a sub/*.img disk?.im?", &paths));
        assert_eq!(spec.path, dir.join("sub/x.img"));
        assert_eq!(spec.opts.more_images, [dir.join("disk1.img"), dir.join("Disk2.IMG")]);
        assert_eq!(parse_mount_command("a none*.img", &paths), Err("No files match none*.img".to_string()));
        // In a folder on a DOS drive.
        let cd = dir.join("cd");
        let locate = |p: &str| p.strip_prefix(r"C:\CD\").map(|rest| cd.join(rest));
        let paths = PathContext { base: Path::new("/nowhere"), config_dir: None, home: None, locate: &locate };
        assert_eq!(mounted(parse_mount_command(r"d C:\CD\*.CUE", &paths)).path, dir.join("cd/a.cue"));
    }

    #[test]
    fn natural_order_and_wildcard_matches() {
        let mut names = vec!["disk10.img", "Disk2.img", "disk1.img", "disk01.img", "a.img"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(names, ["a.img", "disk01.img", "disk1.img", "Disk2.img", "disk10.img"]);
        assert!(matches_wildcard("Game Disc 1.CUE", "*disc ?.cue"));
        assert!(matches_wildcard("DISK1", "*.*"));
        assert!(matches_wildcard("abcbc", "a*bc"));
        assert!(!matches_wildcard("disk10.img", "disk?.img"));
        assert!(!matches_wildcard("abc", "a*d"));
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
                opts: MountOptions { kind: DriveKind::Floppy, label: Some("DISK 1".into()), read_only: true, ..Default::default() },
            },
            MountSpec {
                drive: 3,
                path: "/games/cd.cue".into(),
                opts: MountOptions { kind: DriveKind::CdRom, label: None, read_only: false, ..Default::default() },
            },
            MountSpec {
                drive: 0,
                path: "/home/u/disks/disk 1.img".into(),
                opts: MountOptions {
                    kind: DriveKind::Floppy,
                    more_images: vec!["/home/u/disks/disk 2.img".into(), "/other/d3.ima".into()],
                    ..Default::default()
                },
            },
            MountSpec {
                drive: 2,
                path: "/hd/c.img".into(),
                opts: MountOptions {
                    geometry: Some(Chs { cylinders: 615, heads: 4, sectors: 17 }),
                    ..Default::default()
                },
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
