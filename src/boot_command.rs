//! BOOT: start an operating system from a disk image, with DOSBox
//! Staging's syntax. The images given on the command line go on A: (or the
//! drive -l names) as MOUNT puts them there; a hard disk image mounted
//! with MOUNT or IMGMOUNT boots with -l and its drive, or its number.

use crate::command::ShellCommand;
use crate::cpu::Cpu;
use crate::disk::{DriveKind, FLOPPY_DRIVES, drive_key, drive_number, numbered_drive};
use crate::mount::{MountCmd, PathContext, parse_drive_name, parse_mount_tokens, tokenize};
use crate::video::print_string;

pub const BOOT_USAGE: &str = concat!(
    "Boots an operating system from a disk image.\r\n",
    "\r\n",
    "BOOT [image [image ...]] [-l drive]\r\n",
    "\r\n",
    "  image     Floppy disk images to put in A: and boot from; Ctrl+F4 changes\r\n",
    "            to the next one\r\n",
    "  -l drive  The drive to boot from: A: or B:, or a hard disk image mounted\r\n",
    "            with MOUNT or IMGMOUNT, by its letter or number (0 to 3)\r\n",
    "\r\n",
    "The system has the machine until it turns it off. Examples:\r\n",
    "  IMGMOUNT C win95.img\r\n",
    "  BOOT -l C\r\n",
);

/// What BOOT was asked to do.
#[derive(Debug, PartialEq, Eq)]
enum Request {
    Help,
    /// Boot from `drive` (A: unless -l says otherwise), after putting the
    /// images there.
    Boot { drive: u8, images: Vec<String> },
}

fn parse(tokens: &[String]) -> Result<Request, String> {
    let mut drive = None;
    let mut images = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let token = &tokens[i];
        match token.to_ascii_lowercase().as_str() {
            "/?" | "-?" | "-h" | "--help" => return Ok(Request::Help),
            "-l" => {
                i += 1;
                let letter = tokens.get(i).ok_or("-l needs a drive letter")?;
                drive = Some(parse_letter(letter)?);
            }
            t if t.starts_with("-l") && t.len() > 2 => drive = Some(parse_letter(&token[2..])?),
            t if t.starts_with('-') => return Err(format!("Unknown option '{}'", token)),
            _ => images.push(token.clone()),
        }
        i += 1;
    }
    if drive.is_none() && images.is_empty() {
        return Ok(Request::Help);
    }
    Ok(Request::Boot { drive: drive.unwrap_or(0), images })
}

fn parse_letter(text: &str) -> Result<u8, String> {
    parse_drive_name(text).ok_or_else(|| format!("'{}' isn't a drive letter or number", text))
}

/// BOOT [image ...] [-l drive]
pub struct BootCommand;

impl ShellCommand for BootCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        let request = tokenize(args).and_then(|tokens| parse(&tokens));
        let (drive, images) = match request {
            Ok(Request::Help) => {
                print_string(cpu, BOOT_USAGE);
                return;
            }
            Ok(Request::Boot { drive, images }) => (drive, images),
            Err(e) => {
                print_string(cpu, &format!("{}\r\n", e));
                return;
            }
        };
        if !cpu.process_stack.is_empty() || cpu.secondary.is_some() {
            print_string(cpu, "BOOT can't start a system while a program runs\r\n");
            return;
        }
        if !images.is_empty()
            && let Err(e) = mount_images(cpu, drive, images)
        {
            print_string(cpu, &format!("{}\r\n", e));
            return;
        }
        if let Err(e) = crate::boot::boot_drive(cpu, drive) {
            print_string(cpu, &format!("{}\r\n", e));
        }
    }
}

/// Put `images` in `drive` as MOUNT does: floppy images in A: and B:, hard
/// disk images elsewhere. Floppies without a DOS file system, as booter
/// games have, go in the floppy unit by number, as do those for a unit
/// that has a disk mounted by number.
fn mount_images(cpu: &mut Cpu, drive: u8, images: Vec<String>) -> Result<(), String> {
    let numbered = (drive < FLOPPY_DRIVES).then(|| numbered_drive(drive));
    match numbered {
        Some(numbered) if cpu.bus.disk.is_mounted(numbered) => mount_as(cpu, numbered, &images),
        Some(numbered) => mount_as(cpu, drive, &images).or_else(|e| mount_as(cpu, numbered, &images).map_err(|_| e)),
        None => mount_as(cpu, drive, &images),
    }
}

/// Mount `images` as `drive`, as MOUNT does.
fn mount_as(cpu: &mut Cpu, drive: u8, images: &[String]) -> Result<(), String> {
    let floppy = drive < FLOPPY_DRIVES || drive_number(drive).is_some_and(|n| n < FLOPPY_DRIVES);
    let kind = if floppy { "floppy" } else { "hdd" };
    let mut tokens = vec![drive_key(drive)];
    tokens.extend(images.iter().cloned());
    tokens.extend(["-t".to_string(), kind.to_string()]);
    let cwd = std::env::current_dir().unwrap_or_default();
    let home = crate::hostdirs::home_dir();
    let disk = &cpu.bus.disk;
    let locate = |path: &str| disk.resolve_path(path);
    let paths =
        PathContext { base: &cwd, config_dir: cpu.bus.config_dir.as_deref(), home: home.as_deref(), locate: &locate };
    let spec = match parse_mount_tokens(&tokens, &paths)? {
        MountCmd::Mount(spec) => spec,
        _ => return Err("BOOT needs disk images".to_string()),
    };
    let replace = spec.path.is_file() && cpu.bus.disk.drive_kind(drive) != Some(DriveKind::Virtual);
    cpu.bus.mount_drive(spec.drive, &spec.path, spec.opts, replace).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(line: &str) -> Vec<String> {
        tokenize(line).unwrap()
    }

    #[test]
    fn boots_a_drive_or_images() {
        assert_eq!(parse(&tokens("-l C")), Ok(Request::Boot { drive: 2, images: vec![] }));
        assert_eq!(parse(&tokens("-lc:")), Ok(Request::Boot { drive: 2, images: vec![] }));
        assert_eq!(
            parse(&tokens("disk1.img disk2.img")),
            Ok(Request::Boot { drive: 0, images: vec!["disk1.img".into(), "disk2.img".into()] })
        );
        assert_eq!(parse(&tokens("")), Ok(Request::Help));
        assert_eq!(parse(&tokens("/?")), Ok(Request::Help));
        assert!(parse(&tokens("-l")).is_err());
        assert!(parse(&tokens("-x")).is_err());
    }
}
