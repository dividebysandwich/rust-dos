//! The job of InstallShield's 16-bit setup launcher, for the built-in
//! Windows, which runs no Windows 3.1 programs.
//!
//! The `SETUP.EXE` of an InstallShield 3 to 5 setup is a Windows 3.1
//! program ("Setup Launcher") whose job, on Windows 95 and 98, is to start
//! the setup's 32-bit engine. It unpacks the engine from `_INST32I.EX_`
//! beside it into the temporary folder and runs it with a command line
//! naming the compiled script and that folder. On Windows 98 SE, Enemy
//! Infestation's launcher left these in `C:\WINDOWS\TEMP` (TEMP's value),
//! and started the first with the command line after it:
//!
//! ```text
//! _INS0432._MP  ZDATA.DLL  _WUTL95.DLL
//! C:\WINDOWS\TEMP\_INS0432._MP -fE:\SETUP.INS  -z1 -cx -xC:\WINDOWS\TEMP\
//! ```
//!
//! Later launchers (5.00.212 and 5.00.221, whose engines come with
//! `ZDATAI5.DLL` and `_WUTL50.DLL`) add the setup's folder and its
//! language from `SETUP.LID`. Xenocracy's, with its CD in D:, gave this
//! on Windows 98 SE (`-q1` and the language, English's 0009):
//!
//! ```text
//! -fD:\SETUP.INS  -z1 -cx -xC:\WINDOWS\TEMP\ -x1D:\ -q10009
//! ```
//!
//! Without `-x1` such an engine looks for the setup's `layout.bin` and
//! cabinets in the temporary folder, and ends at once when they aren't
//! there. A launcher's arguments are in a table of strings in it, each
//! ending in a zero: those with `-x1` and `-q1%04x` give these too.
//!
//! The archive's 16-bit helper (`BOOT16.EXE`) stays in it. When the
//! engine has ended, the launcher starts `_ISDEL.EXE` from the setup's
//! folder to delete the files it unpacked: here they stay in the
//! temporary folder, as they do when a setup is cut short.
//!
//! 64-bit Windows, which runs no Windows 3.1 programs either, recognises
//! these launchers in the same way and runs a 32-bit one in their place.

use crate::cpu::Cpu;
use rust_dos_installshield::{EngineArchive, is_engine_archive};

/// The archive the engine comes in, beside the launcher.
const ARCHIVE: &str = "_INST32I.EX_";
/// The setup's compiled script, beside the launcher.
const SCRIPT: &str = "SETUP.INS";
/// The setup's languages, beside the launcher: `Default=` in its
/// `[Languages]` is the language's number in hex.
const LANGUAGES: &str = "SETUP.LID";
/// The entry of a later launcher's table of arguments that gives the
/// engine the setup's folder, with the zeros on either side.
const SOURCE_ARGUMENT: &[u8] = b"\0 -x1\0";
/// The entry of a later launcher's table of arguments that gives the
/// engine the setup's language, with the zeros on either side.
const LANGUAGE_ARGUMENT: &[u8] = b"\0 -q1%04x\0";
/// The engine's name in the archive.
const ENGINE: &str = "INSTALL.EXE";
/// The archive's Windows 3.1 helper, which Windows 95 and 98 don't
/// unpack.
const BOOT16: &str = "BOOT16.EXE";

/// Whether `bytes` are a Windows 3.1 program: an MZ header whose new
/// header (at the offset in 3Ch) starts "NE".
pub fn is_ne(bytes: &[u8]) -> bool {
    let Some(at) = bytes.get(0x3C..0x40) else {
        return false;
    };
    let at = u32::from_le_bytes([at[0], at[1], at[2], at[3]]) as usize;
    bytes.starts_with(b"MZ") && bytes.get(at..at.saturating_add(2)) == Some(b"NE")
}

/// What a later launcher (5.00.212 on) adds to the engine's arguments,
/// each when its table of arguments has the entry. An earlier launcher's
/// is the default, which adds nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LaterArguments {
    /// `-x1`: the setup's folder.
    pub source: bool,
    /// `-q1`: the setup's language, when the setup names one.
    pub language: Option<u16>,
}

impl LaterArguments {
    /// What the launcher `bytes` in `setup_dir` adds.
    fn of(cpu: &mut Cpu, bytes: &[u8], setup_dir: &str) -> Self {
        LaterArguments {
            source: has_argument(bytes, SOURCE_ARGUMENT),
            language: if has_argument(bytes, LANGUAGE_ARGUMENT) {
                language(cpu, setup_dir)
            } else {
                None
            },
        }
    }
}

/// The arguments the launcher gives the engine: the setup's script in
/// `setup_dir` and the temporary folder `temp`, both full DOS paths, and
/// what a later launcher adds (`later`).
pub fn engine_arguments(setup_dir: &str, temp: &str, later: LaterArguments) -> String {
    use std::fmt::Write;
    let setup_dir = setup_dir.trim_end_matches('\\');
    let temp = temp.trim_end_matches('\\');
    let mut arguments = format!("-f{}\\{}  -z1 -cx -x{}\\", setup_dir, SCRIPT, temp);
    if later.source {
        let _ = write!(arguments, " -x1{}\\", setup_dir);
    }
    if let Some(language) = later.language {
        let _ = write!(arguments, " -q1{:04x}", language);
    }
    arguments
}

/// Whether the table of arguments in the launcher `bytes` has `entry`.
fn has_argument(bytes: &[u8], entry: &[u8]) -> bool {
    bytes.windows(entry.len()).any(|w| w == entry)
}

/// The setup's language: `Default=` of the `[Languages]` of `SETUP.LID`
/// in `setup_dir`, a number in hex. None when the file isn't there or
/// doesn't name one.
fn language(cpu: &mut Cpu, setup_dir: &str) -> Option<u16> {
    let file = format!("{}\\{}", setup_dir, LANGUAGES);
    let value = super::kernel32::profile_value(cpu, file.as_bytes(), b"Languages", b"Default")?;
    u16::from_str_radix(std::str::from_utf8(&value).ok()?.trim(), 16).ok()
}

/// When the Windows 3.1 program `bytes`, at the DOS path `path`, is an
/// InstallShield setup launcher with its engine archive beside it: unpack
/// the engine into the temporary folder, and return the engine's DOS path
/// and its arguments, which the built-in Windows starts it with in the
/// launcher's place.
pub fn engine(cpu: &mut Cpu, path: &str, bytes: &[u8]) -> Option<(String, String)> {
    if !is_ne(bytes) {
        return None;
    }
    let setup_dir = path
        .rsplit_once('\\')
        .map_or("", |(dir, _)| dir)
        .to_string();
    let archive_path = format!("{}\\{}", setup_dir, ARCHIVE);
    if !cpu.bus.disk.is_file(&archive_path) {
        return None;
    }
    let data = cpu.bus.disk.file_data(&archive_path)?.read().ok()?;
    if !is_engine_archive(&data) {
        return None;
    }
    let say = |cpu: &mut Cpu, message: String| {
        cpu.bus.log_string(&format!("[W32] {}", message));
    };
    let archive = match EngineArchive::parse(&data) {
        Ok(archive) => archive,
        Err(e) => {
            say(cpu, format!("{}: {}", archive_path, e));
            return None;
        }
    };
    let engine = archive
        .files()
        .iter()
        .find(|f| f.name.eq_ignore_ascii_case(ENGINE))?;
    let temp = temp_directory(cpu)?;
    for file in archive.files() {
        if file.name.eq_ignore_ascii_case(BOOT16) {
            continue;
        }
        let target = format!("{}\\{}", temp, file.extract_as.to_ascii_uppercase());
        let written = archive
            .read(&data, file)
            .map_err(|e| e.to_string())
            .and_then(|bytes| {
                let disk = &mut cpu.bus.disk;
                disk.write_whole_file(&target, &bytes, None)
                    .map_err(|code| format!("DOS error {:02X}h", code))
            });
        if let Err(e) = written {
            say(
                cpu,
                format!("can't unpack {} to {}: {}", file.name, target, e),
            );
            return None;
        }
    }
    say(
        cpu,
        format!(
            "{} is InstallShield's setup launcher: its engine runs",
            path
        ),
    );
    let engine = format!("{}\\{}", temp, engine.extract_as.to_ascii_uppercase());
    let later = LaterArguments::of(cpu, bytes, &setup_dir);
    Some((engine, engine_arguments(&setup_dir, &temp, later)))
}

/// The temporary folder, as the launcher finds it: TEMP's value, or TMP's,
/// or the Windows folder's TEMP, made when it isn't there. None when it
/// can't be made.
fn temp_directory(cpu: &mut Cpu) -> Option<String> {
    let environment = super::dos_environment(cpu, cpu.current_psp);
    let variable =
        |name: &str| super::variable_value(&environment, name).map(|v| v.trim().to_string());
    let wanted = variable("TEMP")
        .or_else(|| variable("TMP"))
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| format!("{}\\TEMP", super::loader::WINDOWS_DIR));
    let disk = &mut cpu.bus.disk;
    let temp = disk.qualify_path(&wanted)?;
    let temp = temp.trim_end_matches('\\').to_string();
    if !disk.is_directory(&temp) {
        // Each folder on the way, from the root.
        let mut at = 0;
        while let Some(next) = temp[at..].find('\\').map(|i| at + i + 1) {
            let folder = &temp[..next - 1];
            if folder.len() > 2 && !disk.is_directory(folder) {
                disk.create_directory(folder).ok()?;
            }
            at = next;
        }
        disk.create_directory(&temp).ok()?;
    }
    Some(temp)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Windows 3.1 program's headers, as small as `is_ne` reads.
    fn ne_program() -> Vec<u8> {
        let mut bytes = vec![0u8; 0x80];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[0x3C] = 0x40;
        bytes[0x40..0x42].copy_from_slice(b"NE");
        bytes
    }

    #[test]
    fn recognises_windows_3_programs() {
        assert!(is_ne(&ne_program()));
        let mut pe = ne_program();
        pe[0x40..0x42].copy_from_slice(b"PE");
        assert!(!is_ne(&pe));
        let mut far = ne_program();
        far[0x3C..0x40].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        assert!(!is_ne(&far));
        assert!(!is_ne(b"MZ"));
    }

    #[test]
    fn unpacks_the_engine_beside_a_launcher() {
        let root = crate::test_dir::path("is-launcher");
        std::fs::create_dir_all(root.join("SETUP")).unwrap();
        let engine_bytes = b"MZ engine".repeat(50);
        let archive = rust_dos_installshield::write::engine_archive(&[
            ("INSTALL.EXE", "_INS0432._MP", &engine_bytes),
            ("zdata.dll", "zdata.dll", b"helper"),
            ("BOOT16.EXE", "_INJ0432._MP", b"MZ 16"),
        ]);
        std::fs::write(root.join("SETUP").join("_INST32I.EX_"), &archive).unwrap();
        let mut cpu = Cpu::new(root.clone());
        let (started, arguments) = engine(&mut cpu, "C:\\SETUP\\SETUP.EXE", &ne_program()).unwrap();
        assert_eq!(started, "C:\\WINDOWS\\TEMP\\_INS0432._MP");
        assert_eq!(
            arguments,
            "-fC:\\SETUP\\SETUP.INS  -z1 -cx -xC:\\WINDOWS\\TEMP\\"
        );
        let temp = root.join("WINDOWS").join("TEMP");
        assert_eq!(
            std::fs::read(temp.join("_INS0432._MP")).unwrap(),
            engine_bytes
        );
        assert_eq!(std::fs::read(temp.join("ZDATA.DLL")).unwrap(), b"helper");
        assert!(!temp.join("_INJ0432._MP").exists());
        // A PE program, or a launcher without its archive, isn't one.
        let mut pe = ne_program();
        pe[0x40..0x42].copy_from_slice(b"PE");
        assert_eq!(engine(&mut cpu, "C:\\SETUP\\SETUP.EXE", &pe), None);
        assert_eq!(engine(&mut cpu, "C:\\SETUP.EXE", &ne_program()), None);
    }

    #[test]
    fn gives_the_engine_the_launchers_command_line() {
        assert_eq!(
            engine_arguments("E:", "C:\\WINDOWS\\TEMP", LaterArguments::default()),
            "-fE:\\SETUP.INS  -z1 -cx -xC:\\WINDOWS\\TEMP\\"
        );
        assert_eq!(
            engine_arguments("D:\\SETUP\\", "C:\\TEMP\\", LaterArguments::default()),
            "-fD:\\SETUP\\SETUP.INS  -z1 -cx -xC:\\TEMP\\"
        );
        // A later launcher's, as Xenocracy's gave it on Windows 98 SE.
        let later = LaterArguments {
            source: true,
            language: Some(9),
        };
        assert_eq!(
            engine_arguments("D:", "C:\\WINDOWS\\TEMP", later),
            "-fD:\\SETUP.INS  -z1 -cx -xC:\\WINDOWS\\TEMP\\ -x1D:\\ -q10009"
        );
        let no_language = LaterArguments {
            language: None,
            ..later
        };
        assert_eq!(
            engine_arguments("D:\\SETUP", "C:\\TEMP", no_language),
            "-fD:\\SETUP\\SETUP.INS  -z1 -cx -xC:\\TEMP\\ -x1D:\\SETUP\\"
        );
        // Languages past 9 are hex, as the launcher's %04x writes them.
        let french = LaterArguments {
            language: Some(0x0C),
            ..later
        };
        assert_eq!(
            engine_arguments("D:", "C:\\TEMP", french),
            "-fD:\\SETUP.INS  -z1 -cx -xC:\\TEMP\\ -x1D:\\ -q1000c"
        );
    }

    /// A launcher whose table of arguments has `entries`, each ending in
    /// a zero, after the headers.
    fn launcher_with(entries: &[&str]) -> Vec<u8> {
        let mut bytes = ne_program();
        for entry in [" -f", " -z1", " -cx", " -x"].iter().chain(entries) {
            bytes.extend_from_slice(entry.as_bytes());
            bytes.push(0);
        }
        bytes
    }

    #[test]
    fn a_later_launcher_names_the_setups_folder_and_language() {
        let root = crate::test_dir::path("is-launcher-source");
        let setup = root.join("SETUP");
        std::fs::create_dir_all(&setup).unwrap();
        let archive = rust_dos_installshield::write::engine_archive(&[(
            "INSTALL.EXE",
            "_INS0566._MP",
            &b"MZ engine".repeat(50),
        )]);
        std::fs::write(setup.join("_INST32I.EX_"), &archive).unwrap();
        let mut cpu = Cpu::new(root.clone());
        let later = launcher_with(&[" -x1", " -q1%04x", " -m"]);
        let path = "C:\\SETUP\\SETUP.EXE";
        // Without SETUP.LID there is no language to give.
        let (_, arguments) = engine(&mut cpu, path, &later).unwrap();
        assert_eq!(
            arguments,
            "-fC:\\SETUP\\SETUP.INS  -z1 -cx -xC:\\WINDOWS\\TEMP\\ -x1C:\\SETUP\\"
        );
        std::fs::write(
            setup.join("SETUP.LID"),
            "[Languages]\r\nkey0=0007\r\nkey1=0009\r\nDefault=0009\r\ncount=2\r\n",
        )
        .unwrap();
        let (_, arguments) = engine(&mut cpu, path, &later).unwrap();
        assert_eq!(
            arguments,
            "-fC:\\SETUP\\SETUP.INS  -z1 -cx -xC:\\WINDOWS\\TEMP\\ -x1C:\\SETUP\\ -q10009"
        );
        // An earlier launcher gives neither, SETUP.LID or not, and only a
        // whole entry counts.
        for earlier in [&[" -m"][..], &[" -x10", " -q1%04xx"]] {
            let (_, arguments) = engine(&mut cpu, path, &launcher_with(earlier)).unwrap();
            assert_eq!(
                arguments,
                "-fC:\\SETUP\\SETUP.INS  -z1 -cx -xC:\\WINDOWS\\TEMP\\"
            );
        }
        // A Default= that isn't a number in hex names no language.
        std::fs::write(
            setup.join("SETUP.LID"),
            "[Languages]\r\nkey0=0009\r\nDefault=English\r\ncount=1\r\n",
        )
        .unwrap();
        let (_, arguments) = engine(&mut cpu, path, &later).unwrap();
        assert_eq!(
            arguments,
            "-fC:\\SETUP\\SETUP.INS  -z1 -cx -xC:\\WINDOWS\\TEMP\\ -x1C:\\SETUP\\"
        );
    }
}
