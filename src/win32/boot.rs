//! The boot's work: what a setup program leaves for the next start of
//! Windows, when it replaces files that are in use or has more to do
//! after a restart. Windows 9x does it in two steps:
//!
//! - WININIT.EXE runs during the boot, before Windows starts. It carries
//!   out the `[rename]` section of `WININIT.INI` in the Windows directory,
//!   line by line: `NUL=file` deletes the file, `new=old` puts the file
//!   `old` in the place of `new`, replacing the file there. Then it
//!   renames `WININIT.INI` to `WININIT.BAK`.
//! - At the start, Windows runs the commands of the RunOnce keys and
//!   deletes each value before its command starts: HKLM's
//!   RunServicesOnce, then HKLM's RunOnce, then HKCU's RunOnce, each key's
//!   values in their order.
//!
//! The built-in Windows has no boot. Each program DOS starts opens a
//! session of its own: the registry is read again, HKEY_DYN_DATA is made
//! anew, and the session ends with its last process. So the start of a
//! session is the built-in Windows' boot, and its work runs there, before
//! the program DOS started: WININIT.INI first, then the RunOnce commands,
//! one at a time. Each command's program, and what it starts, runs to its
//! end before the next one starts, and the program DOS started comes
//! last. Its exit code is the one DOS gets, and it starts in the current
//! directories DOS had before the commands, whatever they changed.
//!
//! A restart (ExitWindowsEx with EWX_REBOOT) boots at once: when the
//! session's last process has ended, WININIT.INI's work is done and the
//! RunOnce commands run in the same DOS process, as after a boot. Then
//! the DOS prompt comes back, with the exit code of the program that
//! asked for the restart.
//!
//! Both kinds of work stay on drive C:, in WININIT.INI and in the
//! registry's file. Work left when the emulator was closed before a
//! restart runs at the next session, as on Windows at the next boot.
//!
//! The commands are those the keys have when the boot starts. A value a
//! command's program sets in a RunOnce key waits for the next boot, as
//! on Windows, where a program that has more to do after the next
//! restart adds its value again. A value that is gone when its turn
//! comes doesn't run.

use super::registry::{self, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
use super::{kernel32, launcher};
use crate::cpu::Cpu;

/// WININIT.INI, in the Windows directory, and the name it gets once its
/// work is done.
pub const WININIT: &str = "C:\\WINDOWS\\WININIT.INI";
pub const WININIT_BAK: &str = "C:\\WINDOWS\\WININIT.BAK";

/// The keys whose commands a boot runs, in the order it runs them, with
/// the names the log gives them.
const RUN_ONCE: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\RunOnce";
const RUN_SERVICES_ONCE: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\RunServicesOnce";
const KEYS: [(u32, &str, &str); 3] = [
    (HKEY_LOCAL_MACHINE, RUN_SERVICES_ONCE, "HKLM"),
    (HKEY_LOCAL_MACHINE, RUN_ONCE, "HKLM"),
    (HKEY_CURRENT_USER, RUN_ONCE, "HKCU"),
];

/// The boot's state, for the DOS process the programs run in.
#[derive(Clone, Debug, Default)]
pub struct Boot {
    /// Whether the session had its boot.
    booted: bool,
    /// Whether the session ends with a restart: the boot runs when its
    /// last process has ended.
    restart: bool,
    /// The commands still to run, in order.
    commands: Vec<Command>,
    /// The program DOS started, which waits for the commands, and DOS's
    /// current directories when it was to start.
    program: Option<Program>,
    directories: Option<Directories>,
    /// Whether the program that is to start is one of the commands'.
    command: bool,
}

/// DOS's current drive and each drive's current directory, and the long
/// names SetCurrentDirectoryA was given for them (`Win32::long_dirs`).
/// The program DOS started gets them back after the commands, which run
/// in the same DOS process: on Windows a command changes Explorer's
/// directory for no other program.
#[derive(Clone, Debug, Default)]
pub struct Directories {
    current: (u8, Vec<(u8, String)>),
    long: Vec<(String, String)>,
}

/// A command of a RunOnce key: the key, as `KEYS` has it, the value's
/// name and its command line.
#[derive(Clone, Debug, Default)]
pub struct Command {
    key: u32,
    name: String,
    line: String,
}

/// A program that starts at `START` (mod.rs): its DOS path, its command
/// line if it isn't the PSP's, and whether it is a Windows 3.1 program.
#[derive(Clone, Debug, Default)]
pub struct Program {
    pub path: String,
    pub line: Option<String>,
    pub win16: bool,
}

/// A session starts the program pending: if it is the session's first,
/// this is the boot. WININIT.INI's work is done, and if there are RunOnce
/// commands, the first of them is the program that starts, with the
/// program DOS started waiting for them.
pub fn session_starts(cpu: &mut Cpu) {
    if std::mem::replace(&mut cpu.bus.win32.boot.booted, true) {
        return;
    }
    boot(cpu);
    if cpu.bus.win32.boot.commands.is_empty() {
        return;
    }
    cpu.bus.win32.boot.program = cpu.bus.win32.pending.take();
    cpu.bus.win32.boot.directories = Some(Directories {
        current: cpu.bus.disk.current_directories(),
        long: cpu.bus.win32.long_dirs.clone(),
    });
    next(cpu);
}

/// ExitWindowsEx ends the session with a restart.
pub fn restart(cpu: &mut Cpu) {
    cpu.bus.win32.boot.restart = true;
}

/// The session's last process ended, or the program that was to start
/// couldn't: make the next program the boot has pending, if there is
/// one. After a restart, that is the boot's. Whether one is.
pub fn next(cpu: &mut Cpu) -> bool {
    if std::mem::take(&mut cpu.bus.win32.boot.restart) {
        cpu.bus.log_string("[W32] Boot: Windows restarts");
        boot(cpu);
    }
    while !cpu.bus.win32.boot.commands.is_empty() {
        let command = cpu.bus.win32.boot.commands.remove(0);
        let Some(&(hkey, path, root)) = KEYS.get(command.key as usize) else {
            continue;
        };
        let place = format!("{}\\{}", root, path);
        // The value goes before the command starts, also when it can't.
        if !registry::delete_value_now(cpu, hkey, path, &command.name) {
            let line = format!("[W32] Boot: {} \"{}\" is gone", place, command.name);
            cpu.bus.log_string(&line);
            continue;
        }
        let line = format!(
            "[W32] Boot: {} \"{}\" runs {}",
            place, command.name, command.line
        );
        cpu.bus.log_string(&line);
        match program_of(cpu, &command.line) {
            Ok(program) => {
                cpu.bus.win32.pending = Some(program);
                cpu.bus.win32.boot.command = true;
                return true;
            }
            Err(message) => {
                let line = format!("[W32] Boot: {} can't run: {}", command.line, message);
                cpu.bus.log_string(&line);
            }
        }
    }
    let Some(program) = cpu.bus.win32.boot.program.take() else {
        return false;
    };
    if let Some(directories) = cpu.bus.win32.boot.directories.take() {
        put_back(cpu, directories);
    }
    cpu.bus.win32.pending = Some(program);
    cpu.bus.win32.boot.command = false;
    true
}

/// Make `directories` DOS's current ones again, in its tables too.
fn put_back(cpu: &mut Cpu, directories: Directories) {
    cpu.bus.disk.set_current_directories(&directories.current);
    cpu.bus.win32.long_dirs = directories.long;
    super::dos(cpu, |cpu| {
        for &(drive, _) in &directories.current.1 {
            crate::dos_data::write_cds(&mut cpu.bus, drive);
        }
    });
}

/// Whether the program that starts is one of the boot's commands' rather
/// than the one DOS started.
pub fn starts_command(cpu: &Cpu) -> bool {
    cpu.bus.win32.boot.command
}

/// The DOS process the programs ran in ended: the next one boots again.
pub fn dos_process_ended(cpu: &mut Cpu) {
    cpu.bus.win32.boot = Boot::default();
}

/// The boot: WININIT.INI's work, and the RunOnce keys' commands as they
/// are now, which run after it.
fn boot(cpu: &mut Cpu) {
    wininit(cpu);
    let keys = KEYS.map(|(hkey, path, _)| (hkey, path));
    let mut commands = Vec::new();
    for (key, values) in registry::string_values(cpu, &keys).into_iter().enumerate() {
        for (name, line) in values {
            if !line.trim().is_empty() {
                let key = key as u32;
                commands.push(Command { key, name, line });
            }
        }
    }
    cpu.bus.win32.boot.commands = commands;
}

/// The program the command line `line` names first, as it starts: found
/// as CreateProcessA finds it, from Explorer, which runs the commands on
/// Windows. A DOS program or a batch file can't start here.
fn program_of(cpu: &mut Cpu, line: &str) -> Result<Program, String> {
    let found = kernel32::program_in_line(cpu, line).ok_or("no such program")?;
    let (path, args, command_line) = (found.path.clone(), found.args, found.command_line(line));
    let file = cpu.read_program_file(&path).ok_or("it can't be read")?;
    if rust_dos_pe::is_pe(&file) {
        return Ok(Program {
            path,
            line: Some(command_line),
            win16: false,
        });
    }
    if let Some((engine, arguments)) = launcher::engine(cpu, &path, &file) {
        return Ok(Program {
            line: Some(format!("{} {}", engine, arguments)),
            path: engine,
            win16: false,
        });
    }
    if launcher::is_win16(&file) {
        // The Win16 layer finds the arguments after the path.
        let line = Some(format!("{}{}", path, args));
        return Ok(Program {
            path,
            line,
            win16: true,
        });
    }
    Err("it isn't a Windows program".to_string())
}

/// WININIT.INI's work, if there is the file: its `[rename]` section, then
/// the file is renamed to WININIT.BAK, replacing an older one.
fn wininit(cpu: &mut Cpu) {
    let Some(text) = cpu.bus.disk.file_data(WININIT).and_then(|f| f.read().ok()) else {
        return;
    };
    cpu.bus.log_string(&format!("[W32] Boot: {}", WININIT));
    for (new, old) in renames(&text) {
        let done = if new.eq_ignore_ascii_case("NUL") {
            cpu.bus.disk.delete_file(&old)
        } else {
            replace(cpu, &new, &old)
        };
        let line = match done {
            Ok(()) if new.eq_ignore_ascii_case("NUL") => format!("[W32] Boot: deleted {}", old),
            Ok(()) => format!("[W32] Boot: {} replaced {}", old, new),
            Err(code) => format!(
                "[W32] Boot: {}={} not done, DOS error {:02X}h",
                new, old, code
            ),
        };
        cpu.bus.log_string(&line);
    }
    let disk = &mut cpu.bus.disk;
    if disk.is_file(WININIT_BAK) {
        let _ = disk.delete_file(WININIT_BAK);
    }
    if let Err(code) = disk.rename_file(WININIT, WININIT_BAK) {
        let line = format!(
            "[W32] Boot: {} can't be renamed, DOS error {:02X}h",
            WININIT, code
        );
        cpu.bus.log_string(&line);
    }
}

/// Put the file `old` in the place of `new`: the file at `new` goes, and
/// `old` is moved there, or copied and deleted when it is on another
/// drive. Nothing changes when there is no file `old`.
fn replace(cpu: &mut Cpu, new: &str, old: &str) -> Result<(), u8> {
    const NOT_SAME_DEVICE: u8 = 0x11;
    let disk = &mut cpu.bus.disk;
    if !disk.is_file(old) {
        return Err(0x02);
    }
    if disk.is_file(new) {
        disk.delete_file(new)?;
    }
    match disk.rename_file(old, new) {
        Err(NOT_SAME_DEVICE) => {
            let data = disk
                .file_data(old)
                .and_then(|f| f.read().ok())
                .ok_or(0x05)?;
            disk.write_whole_file(new, &data, None)?;
            disk.delete_file(old)
        }
        done => done,
    }
}

/// The `[rename]` section of WININIT.INI's text: (new, old) for each
/// "new=old" line, in order. A name may come more than once, as NUL
/// does. Lines starting with ';' are comments.
pub fn renames(text: &[u8]) -> Vec<(String, String)> {
    let text = crate::dosstr::from_bytes(text);
    let mut section = false;
    let mut renames = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[') {
            let name = name.split(']').next().unwrap_or("").trim();
            section = name.eq_ignore_ascii_case("rename");
            continue;
        }
        if !section {
            continue;
        }
        if let Some((new, old)) = line.split_once('=') {
            let (new, old) = (new.trim(), old.trim());
            if !new.is_empty() && !old.is_empty() {
                renames.push((new.to_string(), old.to_string()));
            }
        }
    }
    renames
}

crate::state_fields!(Command { key, name, line });
crate::state_fields!(Program { path, line, win16 });
crate::state_fields!(Directories { current, long });
crate::state_fields!(Boot {
    booted,
    restart,
    commands,
    program,
    directories,
    command
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renames_reads_the_rename_section_in_order() {
        let text = b"[CombineVxDs]\r\nC:\\X=Y\r\n[rename]\r\n; a comment\r\n\
            NUL=C:\\WINDOWS\\TEMP\\A.TMP\r\n\
            C:\\WINDOWS\\SYSTEM\\Q.DLL=C:\\WINDOWS\\SYSTEM\\Q.NEW\r\n\
            NUL=C:\\B.TMP\r\n\r\n[Other]\r\nC:\\Z=W\r\n";
        assert_eq!(
            renames(text),
            [
                ("NUL", "C:\\WINDOWS\\TEMP\\A.TMP"),
                ("C:\\WINDOWS\\SYSTEM\\Q.DLL", "C:\\WINDOWS\\SYSTEM\\Q.NEW"),
                ("NUL", "C:\\B.TMP"),
            ]
            .map(|(a, b)| (a.to_string(), b.to_string()))
        );
    }

    #[test]
    fn renames_finds_the_section_whatever_its_case() {
        let text = b"[RENAME]\nNUL = C:\\A\n";
        assert_eq!(renames(text), [("NUL".to_string(), "C:\\A".to_string())]);
    }
}
