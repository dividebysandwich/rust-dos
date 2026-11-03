//! The COMMIT command: it writes the changes of a mounted disk image,
//! which go to a delta file (`diskdelta`), into the image, so that other
//! programs see them, and mounts the drive again over the image as it is
//! then:
//!
//! ```text
//! COMMIT C:
//! COMMIT 2
//! ```
//!
//! `rust-dos commit IMAGE` does the same from the host's shell.

use crate::command::ShellCommand;
use crate::cpu::Cpu;
use crate::disk::drive_name;
use crate::mount::{display_host_path, parse_drive_name};
use crate::video::print_string;

const HELP: &str = "Writes the changes of a disk image into the image.\r\n\
\r\n\
COMMIT drive\r\n\
\r\n\
\x20 drive  A drive mounted from a disk image (C:), or a disk number (2).\r\n\
\r\n\
What is written to a hard disk image goes to a delta file beside it\r\n\
(IMAGE.rdelta), or in the -overlay folder, and the image stays as it is.\r\n\
COMMIT writes those changes into the image, for other programs, and\r\n\
deletes the delta file. Save states of the disk from before no longer\r\n\
load. From the host: rust-dos commit IMAGE\r\n";

pub struct CommitCommand;

impl ShellCommand for CommitCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        let args = args.trim();
        let Some(drive) = parse_drive_name(args).filter(|_| !args.is_empty()) else {
            print_string(cpu, HELP);
            return;
        };
        let message = match commit(cpu, drive) {
            Ok(message) | Err(message) => message,
        };
        print_string(cpu, &format!("{}\r\n", message));
    }
}

/// Commit the changes of the disk in `drive`: unmount it, write the delta
/// file into the image, and mount it again as it was mounted. Returns what
/// to tell, or why it can't.
fn commit(cpu: &mut Cpu, drive: u8) -> Result<String, String> {
    let name = drive_name(drive);
    if cpu.bus.boot.is_some() {
        return Err("A booted system has the disk: shut it down first".to_string());
    }
    let disk = cpu
        .bus
        .disk
        .bios_image(drive)
        .ok_or_else(|| format!("Drive {} isn't a disk image", name))?;
    let delta = disk.delta_path().map(|p| p.to_path_buf()).ok_or_else(|| {
        format!(
            "Drive {} is written in place: there are no changes to commit",
            name
        )
    })?;
    let image = disk.path().to_path_buf();
    drop(disk);
    if crate::hostfs::is_layer(&image) {
        return Err(format!(
            "{} is in an archive, which can't be written",
            image.display()
        ));
    }
    let spec = cpu
        .bus
        .disk
        .drive_info(drive)
        .and_then(|info| info.mount)
        .ok_or_else(|| {
            format!(
                "Drive {} wasn't mounted by MOUNT or the configuration",
                name
            )
        })?;
    if !crate::hostfs::is_file(&delta) {
        return Ok(format!("Drive {} has no changes to commit", name));
    }
    // Closed, so that nothing holds the files: C:, which stays, is left
    // empty until the mount below, as REMOUNT leaves it.
    if drive == crate::disk::DRIVE_C {
        cpu.bus.disk.empty_drive_c();
    } else {
        cpu.bus.unmount_drive(drive)?;
    }
    let committed = crate::diskdelta::commit(&image, &delta);
    // Mounted again either way: over the image with the changes in it, or
    // as it was, with them still in the delta file.
    let mounted = cpu.bus.mount_drive(drive, &spec.path, spec.opts, true);
    let blocks = committed?;
    mounted?;
    Ok(format!(
        "{} blocks of changes written into {}; drive {} shows it as it is now",
        blocks,
        display_host_path(&image),
        name
    ))
}
