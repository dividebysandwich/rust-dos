//! The shell's commands for files and directories: COPY, DEL (ERASE), REN
//! (RENAME), MD (MKDIR), RD (RMDIR) and VOL, on every kind of drive the
//! disk controller has (host directories, disk images, drives in memory
//! and CD-ROMs, which refuse to be written).

use crate::command::ShellCommand;
use crate::cpu::Cpu;
use crate::disk::{DosDirEntry, drive_letter, parse_drive_prefix};
use crate::video::{print_cp437, print_string};

/// DOS's message for an error code of the disk controller.
fn error_text(code: u8) -> &'static str {
    match code {
        0x02 => "File not found",
        0x03 => "Path not found",
        0x05 => "Access denied",
        0x0F => "Invalid drive specification",
        0x13 => "Write protect error",
        0x27 => "Insufficient disk space",
        0x50 => "File exists",
        _ => "General failure",
    }
}

fn has_wildcards(spec: &str) -> bool {
    spec.contains(['*', '?'])
}

/// A path as a search spec for the files in it: a directory (or a drive,
/// "D:") stands for everything in it.
fn files_spec(cpu: &Cpu, spec: &str) -> String {
    if spec.ends_with(['\\', ':']) {
        format!("{}*.*", spec)
    } else if !has_wildcards(spec) && cpu.bus.disk.is_directory(spec) {
        format!("{}\\*.*", spec)
    } else {
        spec.to_string()
    }
}

/// Whether a path names a directory to put files in: one that exists, a
/// path ending in a backslash, or a drive.
fn is_directory_target(cpu: &Cpu, path: &str) -> bool {
    path.ends_with(['\\', ':']) || (!has_wildcards(path) && cpu.bus.disk.is_directory(path))
}

/// The directory part of a path, with its separator: "C:\GAMES\" of
/// "C:\GAMES\*.TXT", "A:" of "A:*.*", nothing of "*.TXT".
fn directory_part(path: &str) -> &str {
    path.rfind(['\\', ':']).map_or("", |i| &path[..=i])
}

/// The name REN and COPY give `name` after `template`, which may have
/// wildcards: a '?' keeps the character at its place, a '*' the rest of
/// the name (or extension), anything else takes the place of the
/// character there. REN *.TXT *.BAK, COPY A?.DAT B?.DAT.
pub fn apply_template(name: &str, template: &str) -> String {
    fn part(source: &str, template: &str) -> String {
        let source: Vec<char> = source.chars().collect();
        let mut out = String::new();
        for (i, c) in template.chars().enumerate() {
            match c {
                '*' => {
                    out.extend(source.iter().skip(i));
                    break;
                }
                '?' => out.extend(source.get(i)),
                _ => out.push(c),
            }
        }
        out
    }
    let (base, ext) = name.rsplit_once('.').unwrap_or((name, ""));
    let (template_base, template_ext) = template.split_once('.').unwrap_or((template, ""));
    let (base, ext) = (part(base, template_base), part(ext, template_ext));
    if ext.is_empty() { base } else { format!("{}.{}", base, ext) }
}

/// Take the time a file's reading or writing takes on its drive's disk.
fn disk_activity(cpu: &mut Cpu, path: &str, bytes: usize, write: bool) {
    if let Some(drive) = cpu.bus.disk.drive_of(path) {
        let key = cpu.bus.disk.file_key(path);
        let access = crate::disknoise::Access::File { write, key };
        let bytes = crate::diskio::OPEN_BYTES + bytes as u32;
        cpu.bus.drive_activity(drive, bytes, access);
    }
}

/// The contents of a file, with the time reading it takes; with `ascii`,
/// up to its end of file mark (^Z).
fn read_file(cpu: &mut Cpu, path: &str, ascii: bool) -> Result<Vec<u8>, u8> {
    let data = cpu.bus.disk.file_data(path).ok_or(0x02u8)?.read().map_err(|_| 0x05u8)?;
    disk_activity(cpu, path, data.len(), false);
    let end = if ascii { data.iter().position(|&b| b == 0x1A).unwrap_or(data.len()) } else { data.len() };
    Ok(data[..end].to_vec())
}

/// COPY's command line: the files to copy, joined by '+' to concatenate
/// them, and where to.
#[derive(Debug, Default, PartialEq, Eq)]
struct CopyArgs {
    sources: Vec<String>,
    destination: Option<String>,
    /// /A before the destination: the sources are text, up to their end
    /// of file mark; /B: they are binary. None: text when concatenated.
    ascii_sources: Option<bool>,
    /// /A after the destination: it gets an end of file mark.
    ascii_destination: bool,
}

fn parse_copy(args: &str) -> Result<CopyArgs, &'static str> {
    enum Item<'a> {
        Path(&'a str),
        Plus,
        Switch(&'a str),
    }
    let mut items = Vec::new();
    for word in args.split_whitespace() {
        let mut rest = word;
        while !rest.is_empty() {
            if let Some(after) = rest.strip_prefix('+') {
                items.push(Item::Plus);
                rest = after;
            } else if let Some(after) = rest.strip_prefix('/') {
                let end = after.find(['/', '+']).unwrap_or(after.len());
                items.push(Item::Switch(&after[..end]));
                rest = &after[end..];
            } else {
                let end = rest.find(['/', '+']).unwrap_or(rest.len());
                items.push(Item::Path(&rest[..end]));
                rest = &rest[end..];
            }
        }
    }
    let mut copy = CopyArgs::default();
    let mut joined = true;
    for item in items {
        match item {
            Item::Plus => joined = true,
            Item::Path(path) if joined && copy.destination.is_none() => {
                copy.sources.push(path.to_string());
                joined = false;
            }
            Item::Path(path) if copy.destination.is_none() => copy.destination = Some(path.to_string()),
            Item::Path(_) => return Err("Too many parameters"),
            Item::Switch(switch) => match switch.to_ascii_uppercase().as_str() {
                "A" | "B" if copy.destination.is_some() => copy.ascii_destination = switch.eq_ignore_ascii_case("A"),
                "A" | "B" => copy.ascii_sources = Some(switch.eq_ignore_ascii_case("A")),
                "V" | "Y" | "-Y" => {}
                _ => return Err("Invalid switch"),
            },
        }
    }
    if copy.sources.is_empty() {
        return Err("Required parameter missing");
    }
    Ok(copy)
}

/// COPY source[+source...] [destination] [/A|/B] [/V] [/Y|/-Y]: copy files
/// (or directories' files) to a directory or under another name, the
/// wildcards of the destination taking the source's characters, or join
/// them into one. Plain copies keep their date and time. A destination of
/// CON prints the files.
pub struct CopyCommand;
impl ShellCommand for CopyCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        let copy = match parse_copy(args) {
            Ok(copy) => copy,
            Err(e) => {
                print_string(cpu, &format!("{}\r\n", e));
                return;
            }
        };
        if copy.sources.iter().any(|s| crate::disk::char_device(s).is_some()) {
            print_string(cpu, "Copying from a device isn't supported\r\n");
            return;
        }
        // Everything each source stands for.
        let mut found: Vec<(String, DosDirEntry)> = Vec::new();
        for source in &copy.sources {
            let spec = files_spec(cpu, source);
            match cpu.bus.disk.matching_files(&spec) {
                Ok(files) if !files.is_empty() => found.extend(files),
                _ => {
                    print_string(cpu, &format!("File not found - {}\r\n", source));
                    print_string(cpu, "        0 file(s) copied\r\n");
                    return;
                }
            }
        }
        let wildcards = copy.sources.iter().any(|s| has_wildcards(s));
        let destination = copy.destination.clone().unwrap_or_else(|| format!("{}:", drive_letter(cpu.bus.disk.get_current_drive())));
        let into_directory = is_directory_target(cpu, &destination);
        let joining = copy.sources.len() > 1
            || (wildcards && copy.destination.is_some() && !into_directory && !has_wildcards(&destination));
        let copied = if joining {
            join_files(cpu, &copy, &found, &destination)
        } else {
            copy_files(cpu, &copy, &found, &destination, into_directory, wildcards)
        };
        print_string(cpu, &format!("{:>9} file(s) copied\r\n", copied));
    }
}

/// Where a copy of `path`, named `name`, goes: into the destination
/// directory, under the destination's name with its wildcards filled in,
/// or to the destination.
fn target_of(destination: &str, into_directory: bool, name: &str) -> String {
    if into_directory {
        let dir = if destination.ends_with(['\\', ':']) { destination.to_string() } else { format!("{}\\", destination) };
        format!("{}{}", dir, name)
    } else if has_wildcards(destination) {
        let leaf = &destination[directory_part(destination).len()..];
        format!("{}{}", directory_part(destination), apply_template(name, leaf))
    } else {
        destination.to_string()
    }
}

fn copy_files(cpu: &mut Cpu, copy: &CopyArgs, found: &[(String, DosDirEntry)], destination: &str, into_directory: bool, show_names: bool) -> usize {
    let ascii = copy.ascii_sources.unwrap_or(false);
    let mut copied = 0;
    for (path, entry) in found {
        if show_names {
            print_string(cpu, &format!("{}\r\n", entry.filename));
        }
        let data = match read_file(cpu, path, ascii) {
            Ok(data) => data,
            Err(e) => {
                print_string(cpu, &format!("{} - {}\r\n", error_text(e), entry.filename));
                continue;
            }
        };
        let target = target_of(destination, into_directory, &entry.filename);
        if write_target(cpu, path, &target, data, copy.ascii_destination, Some((entry.dos_time, entry.dos_date))) {
            copied += 1;
        }
    }
    copied
}

/// COPY A+B C, or *.TXT into one file: the files one after the other, as
/// text (up to their end of file marks) unless /B says they are binary.
/// Without a destination they are added to the first.
fn join_files(cpu: &mut Cpu, copy: &CopyArgs, found: &[(String, DosDirEntry)], destination: &str) -> usize {
    let ascii = copy.ascii_sources.unwrap_or(true);
    let target = match &copy.destination {
        Some(_) => destination.to_string(),
        None => found[0].0.clone(),
    };
    let target_path = cpu.bus.disk.qualify_path(&target);
    let mut data = Vec::new();
    for (path, entry) in found {
        print_string(cpu, &format!("{}\r\n", entry.filename));
        // Joining onto the destination itself keeps what it had.
        if Some(path) == target_path.as_ref() && !data.is_empty() {
            continue;
        }
        match read_file(cpu, path, ascii) {
            Ok(bytes) => data.extend(bytes),
            Err(e) => print_string(cpu, &format!("{} - {}\r\n", error_text(e), entry.filename)),
        }
    }
    let source = found[0].0.clone();
    let from = if target_path.as_deref() == Some(source.as_str()) { "" } else { source.as_str() };
    usize::from(write_target(cpu, from, &target, data, copy.ascii_destination, None))
}

/// Write a copy to `target`, or print it for CON; a file isn't copied onto
/// itself. Whether it was written.
fn write_target(cpu: &mut Cpu, source: &str, target: &str, mut data: Vec<u8>, ascii: bool, stamp: Option<(u16, u16)>) -> bool {
    if ascii {
        data.push(0x1A);
    }
    match crate::disk::char_device(target) {
        Some(crate::disk::CharDevice::Con) => {
            print_cp437(cpu, &data, 0x07);
            return true;
        }
        Some(_) => return true,
        None => {}
    }
    if cpu.bus.disk.qualify_path(target).as_deref() == Some(source) {
        print_string(cpu, "File cannot be copied onto itself\r\n");
        return false;
    }
    match cpu.bus.disk.write_whole_file(target, &data, stamp) {
        Ok(()) => {
            disk_activity(cpu, target, data.len(), true);
            true
        }
        Err(e) => {
            print_string(cpu, &format!("{} - {}\r\n", error_text(e), target));
            false
        }
    }
}

/// DEL (ERASE) files: a name, a name with wildcards, or a directory for
/// all of its files. /P is taken and ignored.
pub struct DelCommand;
impl ShellCommand for DelCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        let Some(target) = args.split_whitespace().find(|a| !a.starts_with('/')) else {
            print_string(cpu, "Required parameter missing\r\n");
            return;
        };
        let spec = files_spec(cpu, target);
        let files = match cpu.bus.disk.matching_files(&spec) {
            Ok(files) if !files.is_empty() => files,
            Ok(_) | Err(0x02) => return print_string(cpu, "File not found\r\n"),
            Err(e) => return print_string(cpu, &format!("{}\r\n", error_text(e))),
        };
        for (path, entry) in files {
            match cpu.bus.disk.delete_file(&path) {
                Ok(()) => disk_activity(cpu, &path, 0, true),
                Err(e) => print_string(cpu, &format!("{} - {}\r\n", error_text(e), entry.filename)),
            }
        }
    }
}

/// REN (RENAME) old new: give files new names in their directory; with
/// wildcards, every matching file, after the new name's template.
pub struct RenCommand;
impl ShellCommand for RenCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        let words: Vec<&str> = args.split_whitespace().collect();
        let [from, to] = words[..] else {
            let message = if words.len() < 2 { "Required parameter missing" } else { "Too many parameters" };
            return print_string(cpu, &format!("{}\r\n", message));
        };
        if to.contains(['\\', ':']) {
            return print_string(cpu, "Invalid parameter\r\n");
        }
        let files = match cpu.bus.disk.matching_files(from) {
            Ok(files) if !files.is_empty() => files,
            _ => return print_string(cpu, "Duplicate file name or file not found\r\n"),
        };
        let dir = directory_part(from);
        for (path, entry) in files {
            let target = format!("{}{}", dir, apply_template(&entry.filename, to));
            if cpu.bus.disk.rename_file(&path, &target).is_err() {
                print_string(cpu, "Duplicate file name or file not found\r\n");
            }
        }
    }
}

/// MD (MKDIR) directory.
pub struct MdCommand;
impl ShellCommand for MdCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        match args.split_whitespace().next() {
            None => print_string(cpu, "Required parameter missing\r\n"),
            Some(dir) => {
                if cpu.bus.disk.create_directory(dir).is_err() {
                    print_string(cpu, "Unable to create directory\r\n");
                }
            }
        }
    }
}

/// RD (RMDIR) directory, which has to be empty.
pub struct RdCommand;
impl ShellCommand for RdCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        match args.split_whitespace().next() {
            None => print_string(cpu, "Required parameter missing\r\n"),
            Some(dir) => {
                if cpu.bus.disk.remove_directory(dir).is_err() {
                    print_string(cpu, "Invalid path, not directory,\r\nor directory not empty\r\n");
                }
            }
        }
    }
}

/// VOL [d:]: the label and serial number of a drive.
pub struct VolCommand;
impl ShellCommand for VolCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        let drive = match args.split_whitespace().next() {
            Some(arg) => match parse_drive_prefix(arg) {
                (Some(drive), "") => drive,
                _ => return print_string(cpu, "Invalid drive specification\r\n"),
            },
            None => cpu.bus.disk.get_current_drive(),
        };
        if !cpu.bus.disk.is_mounted(drive) {
            return print_string(cpu, "Invalid drive specification\r\n");
        }
        let letter = drive_letter(drive);
        let label = cpu.bus.disk.volume_label(drive).unwrap_or_default();
        let line = if label.is_empty() {
            format!(" Volume in drive {} has no label\r\n", letter)
        } else {
            format!(" Volume in drive {} is {}\r\n", letter, label)
        };
        print_string(cpu, &line);
        let serial = cpu.bus.disk.volume_serial(drive).unwrap_or(crate::interrupts::int21::VOLUME_SERIAL + drive as u32);
        print_string(cpu, &format!(" Volume Serial Number is {:04X}-{:04X}\r\n", serial >> 16, serial & 0xFFFF));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_fill_in_the_name() {
        assert_eq!(apply_template("REPORT.TXT", "*.BAK"), "REPORT.BAK");
        assert_eq!(apply_template("A1.DAT", "B?.DAT"), "B1.DAT");
        assert_eq!(apply_template("GAME.EXE", "*.*"), "GAME.EXE");
        assert_eq!(apply_template("GAME.EXE", "NEW"), "NEW");
        assert_eq!(apply_template("README", "*.TXT"), "README.TXT");
    }

    #[test]
    fn copy_lines_are_split_into_sources_and_a_destination() {
        let parsed = parse_copy("A.TXT+B.TXT /B C.TXT /A").unwrap();
        assert_eq!(parsed.sources, ["A.TXT", "B.TXT"]);
        assert_eq!(parsed.destination.as_deref(), Some("C.TXT"));
        assert_eq!((parsed.ascii_sources, parsed.ascii_destination), (Some(false), true));
        assert_eq!(parse_copy("A + B").unwrap().sources, ["A", "B"]);
        assert_eq!(parse_copy("*.* D:\\ /Y").unwrap().destination.as_deref(), Some("D:\\"));
        assert!(parse_copy("A B C").is_err());
        assert!(parse_copy("").is_err());
    }
}
