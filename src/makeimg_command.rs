//! The MAKEIMG command, as DOSBox Staging has it: it makes a new disk
//! image (makeimg.rs) on the host's files, or with -d at a DOS path on a
//! drive mounted from a host directory, once the user says yes:
//!
//! ```text
//! MAKEIMG floppy.img -t fd_1440kb -label MYDISK
//! MAKEIMG hdd.img -t hd -size 500
//! MAKEIMG C:\IMAGES\HDD120.IMG -t hd_120mb -d
//! ```

use crate::command::ShellCommand;
use crate::cpu::Cpu;
use crate::diskimage::Chs;
use crate::makeimg::{self, ImageSpec, Plan};
use crate::mount::{display_host_path, expand_host_path, tokenize};
use crate::shell::ShellWait;
use crate::video::print_string;
use std::path::PathBuf;

/// MAKEIMG's help, DOSBox Staging's made to fit a screen.
const HELP: &str = "Create a new empty disk image.\r\n\
\r\n\
MAKEIMG [FILE] -t TYPE [-size MB | -chs C,H,S] [-fat 12|16|32] [-spc N]\r\n\
\x20       [-label NAME] [-noformat] [-force] [-writetodos | -d]\r\n\
\r\n\
\x20 FILE          The image on the host (MAKEIMG.IMG if left out), or with -d\r\n\
\x20               at a DOS path on a drive mounted from a host directory.\r\n\
\x20 -t TYPE       fd_160kb, fd_180kb, fd_320kb, fd_360kb, fd_720kb, fd_1200kb,\r\n\
\x20               fd_1440kb, fd_2880kb, hd_20mb, hd_40mb, hd_80mb, hd_120mb,\r\n\
\x20               hd_250mb, hd_520mb, hd_1gb, hd_2gb, or hd with -size or -chs.\r\n\
\x20 -size MB      The size of an hd.\r\n\
\x20 -chs C,H,S    The cylinders, heads and sectors of an hd.\r\n\
\x20 -fat 12|16|32 The file system, else the one the size calls for.\r\n\
\x20 -spc N        Sectors per cluster.\r\n\
\x20 -label NAME   The volume label.\r\n\
\x20 -noformat     All zeros: no partition table or file system.\r\n\
\x20 -force        Overwrite an existing file.\r\n\
\r\n\
Examples: MAKEIMG floppy.img -t fd_1440kb -label MYDISK\r\n\
\x20         MAKEIMG hdd.img -t hd -size 500\r\n\
\x20         MAKEIMG C:\\IMAGES\\HDD120.IMG -t hd_120mb -d\r\n\
MOUNT mounts FAT12 and FAT16 images; FAT32 ones are for other systems.\r\n";

/// What the command line asks for.
#[derive(Debug, PartialEq)]
struct Request {
    file: String,
    spec: ImageSpec,
    force: bool,
    /// The file is a DOS path (-d).
    dos: bool,
}

/// MAKEIMG's arguments, None for its help.
fn parse(args: &str) -> Result<Option<Request>, String> {
    let tokens = tokenize(args)?;
    if tokens.is_empty() || tokens.iter().any(|t| matches!(t.as_str(), "/?" | "-?" | "-h" | "--help")) {
        return Ok(None);
    }
    // The file comes first; without one, MAKEIMG.IMG.
    let (file, options) = match tokens[0].starts_with('-') {
        true => ("MAKEIMG.IMG".to_string(), &tokens[..]),
        false => (tokens[0].clone(), &tokens[1..]),
    };
    let mut request = Request { file, spec: ImageSpec::default(), force: false, dos: false };
    let mut kind = None;
    let mut options = options.iter();
    let number = |option: &str, text: &str| text.parse::<u64>().map_err(|_| format!("Invalid {} {}", option, text));
    while let Some(option) = options.next() {
        let lower = option.to_ascii_lowercase();
        let mut value = || options.next().ok_or_else(|| format!("{} needs a value", lower));
        let spec = &mut request.spec;
        match lower.as_str() {
            "-t" => kind = Some(value()?.clone()),
            "-size" => spec.size_mb = Some(number(&lower, value()?)?),
            "-chs" => {
                let text = value()?;
                let parts: Vec<u32> = text.split(',').map(|p| p.trim().parse().ok()).collect::<Option<_>>().unwrap_or_default();
                let [cylinders, heads, sectors] = parts[..] else {
                    return Err(format!("Invalid -chs {} (cylinders,heads,sectors)", text));
                };
                spec.chs = Some(Chs { cylinders, heads, sectors });
            }
            "-fat" => spec.fat = Some(number(&lower, value()?)?.min(255) as u8),
            "-spc" => spec.sectors_per_cluster = Some(number(&lower, value()?)?.min(u32::MAX as u64) as u32),
            "-label" => spec.label = Some(value()?.clone()),
            "-force" => request.force = true,
            "-noformat" => spec.unformatted = true,
            "-writetodos" | "-d" => request.dos = true,
            _ => return Err(format!("Unknown option {} (MAKEIMG /? lists them)", option)),
        }
    }
    let kind = kind.ok_or("Which kind of disk? -t TYPE (MAKEIMG /? lists them)")?;
    if !kind.eq_ignore_ascii_case("hd") {
        request.spec.preset = Some(makeimg::preset(&kind).ok_or_else(|| format!("Unknown disk type: {}", kind))?);
    }
    Ok(Some(request))
}

/// Where the image goes and what it will be.
struct Target {
    path: PathBuf,
    /// Its DOS path, with -d.
    dos: Option<String>,
    plan: Plan,
    force: bool,
}

impl Target {
    fn shown(&self) -> String {
        self.dos.clone().unwrap_or_else(|| display_host_path(&self.path))
    }
}

/// Everything MAKEIMG's arguments ask for, checked before it asks: the
/// disk, and a file that can be made where it goes. None for its help.
fn prepare(cpu: &Cpu, args: &str) -> Result<Option<Target>, String> {
    let Some(request) = parse(args)? else { return Ok(None) };
    let plan = makeimg::plan(&request.spec)?;
    let disk = &cpu.bus.disk;
    let (path, dos) = if request.dos {
        let (drive, _) = crate::disk::parse_drive_prefix(&request.file);
        let info = disk.drive_info(drive.unwrap_or(disk.get_current_drive())).ok_or("Target drive is invalid.")?;
        let path = disk.resolve_path(&request.file).filter(|_| info.image.is_none()).ok_or(
            "Cannot create image inside another disk image.\r\nTarget must be a mounted local directory.",
        )?;
        if info.read_only {
            return Err("Target drive is read-only.".to_string());
        }
        (path, Some(disk.qualify_path(&request.file).unwrap_or(request.file)))
    } else {
        let cwd = std::env::current_dir().unwrap_or_default();
        (expand_host_path(&request.file, &cwd, dirs::home_dir().as_deref()), None)
    };
    let target = Target { path, dos, plan, force: request.force };
    if target.path.is_dir() {
        return Err(format!("{} is a directory", target.shown()));
    }
    if target.path.exists() && !target.force {
        return Err(format!("File {} already exists. Use -force to overwrite.", target.shown()));
    }
    // A mounted image stays as it is.
    let same = |image: &PathBuf| image.canonicalize().ok().is_some_and(|p| Some(p) == target.path.canonicalize().ok());
    if let Some(info) = disk.mounted_drives().iter().find(|i| i.images.iter().chain(&i.image).any(same)) {
        return Err(format!("{} is mounted as drive {}: (MOUNT -u {} unmounts it)", target.shown(), info.letter(), info.letter()));
    }
    Ok(Some(target))
}

/// MAKEIMG [FILE] -t TYPE [options]: ask, then make the image.
pub struct MakeImgCommand;
impl ShellCommand for MakeImgCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        match prepare(cpu, args) {
            Ok(None) => print_string(cpu, HELP),
            Ok(Some(target)) => {
                let question = match &target.dos {
                    Some(dos) => format!(
                        "Image will be created on the DOS filesystem at:\r\n  {}\r\n  Host path: {}\r\n\r\nProceed? (Y/N)\r\n",
                        dos,
                        display_host_path(&target.path)
                    ),
                    None => format!("Image will be created on the HOST filesystem at:\r\n  {}\r\n\r\nProceed? (Y/N)\r\n", target.shown()),
                };
                print_string(cpu, &question);
                crate::shell::enter_wait(cpu, ShellWait::MakeImg(args.to_string()));
            }
            Err(e) => print_string(cpu, &format!("{}\r\n", e)),
        }
    }
}

/// A key for MAKEIMG's question about the image `args` ask for: Y makes
/// it, N or Esc doesn't. Whether it was one of those.
pub fn answer(cpu: &mut Cpu, args: &str, key: u8) -> bool {
    match key {
        b'y' | b'Y' => {
            print_string(cpu, "Y\r\n");
            // Checked again: the drives may have changed since, in a
            // state loaded meanwhile.
            let made = prepare(cpu, args).and_then(|target| {
                let target = target.ok_or("")?;
                makeimg::write(&target.path, &target.plan, target.force)?;
                Ok(target)
            });
            match made {
                Ok(target) => {
                    let Chs { cylinders, heads, sectors } = target.plan.chs;
                    let mut text = format!("Created {} [CHS: {}, {}, {}]\r\n", target.shown(), cylinders, heads, sectors);
                    if let Some(volume) = &target.plan.volume {
                        text.push_str(&format!("Formatted as FAT{}\r\n", volume.bits));
                    }
                    print_string(cpu, &text);
                }
                Err(e) => print_string(cpu, &format!("{}\r\n", e)),
            }
            true
        }
        b'n' | b'N' | 0x1B => {
            print_string(cpu, "N\r\n\r\nOperation aborted.\r\n");
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_as_dosbox_staging_takes_them() {
        let r = parse("floppy.img -t FD_1440KB -label MyDisk").unwrap().unwrap();
        assert_eq!((r.file.as_str(), r.spec.preset.map(|p| p.name)), ("floppy.img", Some("fd_1440kb")));
        assert_eq!((r.spec.label.as_deref(), r.force, r.dos), (Some("MyDisk"), false, false));
        let r = parse("\"my disk.img\" -T hd -SIZE 500 -fat 16 -spc 8 -force -noformat -d").unwrap().unwrap();
        assert_eq!((r.file.as_str(), r.spec.preset, r.spec.size_mb), ("my disk.img", None, Some(500)));
        assert_eq!((r.spec.fat, r.spec.sectors_per_cluster, r.spec.unformatted, r.force, r.dos), (Some(16), Some(8), true, true, true));
        let r = parse("-t hd -chs 100,16,63").unwrap().unwrap();
        assert_eq!((r.file.as_str(), r.spec.chs), ("MAKEIMG.IMG", Some(Chs { cylinders: 100, heads: 16, sectors: 63 })));

        assert_eq!(parse(""), Ok(None));
        assert_eq!(parse("/?"), Ok(None));
        assert!(parse("x.img").unwrap_err().contains("-t TYPE"));
        assert!(parse("x.img -t fd_100kb").unwrap_err().contains("Unknown disk type: fd_100kb"));
        assert!(parse("x.img -t hd -size").unwrap_err().contains("-size needs a value"));
        assert!(parse("x.img -t hd -size big").unwrap_err().contains("Invalid -size big"));
        assert!(parse("x.img -t hd -chs 1,2").unwrap_err().contains("Invalid -chs"));
        assert!(parse("x.img -t hd -bogus").unwrap_err().contains("Unknown option -bogus"));
    }
}
