//! The built-in DLLs as files in the system directory, C:\WINDOWS\SYSTEM,
//! as Windows has its DLLs there: programs and setups that look for a DLL
//! by its file (open it, GetFileAttributes, FindFirstFile) find the ones
//! LoadLibrary would load, GLIDE2X.DLL and GLIDE3X.DLL with a 3dfx
//! card. MPROFILE.EXE, Xenocracy's hardware profiler, opens
//! C:\WINDOWS\SYSTEM\GLIDE2X.DLL to tell whether the machine has Glide.
//!
//! Each file is a PE image of the DLL: its headers, sections and export
//! directory as the DLL is mapped in a process, with the functions the
//! version of Windows has, and the version resource GetFileVersionInfo
//! gives for it. DOS shows them in the directory (disk/system_files.rs);
//! a file of the same name on the drive wins over one, and writing to
//! one makes such a file.
//!
//! MSVCRT20.DLL and MSVCRT40.DLL are there too, as files whose exports
//! forward to MSVCRT.DLL (forwarders.rs), which the loader loads as
//! any DLL file.
//!
//! So are the 16-bit system modules (win16/builtin.rs), all of them in
//! the system directory as on Windows 95 and 98: KRNL386.EXE, USER.EXE,
//! GDI.EXE, MMSYSTEM.DLL and the others, each an NE file with the
//! module's name, its exports and a version resource. AAPLAY.DLL, the
//! Autodesk Animator player, looks for MMSYSTEM.DLL there with OpenFile.
//! Loading one, by its name or its path, binds the built-in module.
//!
//! Which DLLs are there is the loader's call (`loader::builtin_for`), so
//! LoadLibrary and the files agree. They follow the settings: `sync` runs
//! when the settings are applied, as a program starts and after a save
//! state loads.
//!
//! The Windows directory, C:\WINDOWS, has the programs of Windows that
//! are built in the same way: NOTEPAD.EXE (user32/notepad.rs), which
//! setups run to show a readme, and REGEDIT.EXE (regedit.rs), which they
//! run to import a registration file.

use super::forwarders;
use super::loader::{self, SYSTEM_DIR, WINDOWS_DIR};
use super::stubs::{self, DLLS};
use super::version::WindowsVersion;
use crate::bus::Bus;
use crate::win16::builtin as builtin16;

/// The built-in DLLs there are, by their places in `DLLS`.
fn present(bus: &Bus) -> Vec<usize> {
    if !bus.win32.enabled {
        return Vec::new();
    }
    (0..DLLS.len())
        .filter(|&i| loader::builtin_for(bus, DLLS[i].name) == Some(i))
        .collect()
}

/// Show the built-in DLLs there are in the system directory, of the
/// version of Windows the programs see, and the built-in programs in the
/// Windows directory; none while the built-in Windows is off.
pub fn sync(bus: &mut Bus) {
    let version = super::version::shown(&bus.win32);
    let wanted = (version, present(bus));
    if bus.win32.dll_files.as_ref() == Some(&wanted) {
        return;
    }
    let mut files: Vec<_> = wanted
        .1
        .iter()
        .map(|&i| (DLLS[i].name.to_string(), stubs::file(i, version).into()))
        .collect();
    if bus.win32.enabled {
        files.push((
            "REGSVR32.EXE".to_string(),
            super::regsvr32::file(bus).into(),
        ));
        files.extend(
            forwarders::FORWARDERS
                .iter()
                .map(|f| (f.name.to_string(), forwarders::file(f, version).into())),
        );
        files.extend(builtin16::MODULES.iter().enumerate().map(|(i, def)| {
            let resource = super::verdll::builtin16_version(def.file, version);
            (def.file.to_string(), builtin16::file(i, &resource).into())
        }));
    }
    bus.disk.set_system_files(SYSTEM_DIR, files);
    let programs = if bus.win32.enabled {
        vec![
            (
                "NOTEPAD.EXE".to_string(),
                super::user32::notepad::file(bus).into(),
            ),
            ("REGEDIT.EXE".to_string(), super::regedit::file(bus).into()),
        ]
    } else {
        Vec::new()
    };
    bus.disk.set_system_files(WINDOWS_DIR, programs);
    bus.win32.dll_files = Some(wanted);
}

/// The DLLs the system directory shows and the version of Windows they
/// are of (`sync`).
pub(super) type Shown = (WindowsVersion, Vec<usize>);

#[cfg(test)]
mod tests {
    use super::super::verdll::{self, Resource};
    use super::*;
    use rust_dos_ne as ne;

    /// Each 16-bit system module's file is an NE file of the module, with
    /// its exports by name and ordinal and a 16-bit version resource, and
    /// is known as a copy of the module's file.
    #[test]
    fn the_16_bit_files_are_their_modules() {
        for (i, def) in builtin16::MODULES.iter().enumerate() {
            let version = verdll::builtin16_version(def.file, WindowsVersion::Win98Se);
            let file = builtin16::file(i, &version);
            let parsed = ne::Module::parse(&file).unwrap();
            assert_eq!(parsed.name(), def.name);
            assert_eq!(parsed.is_dll(), def.library, "{}", def.file);
            for api in def.apis.iter().filter(|a| a.ordinal != 0) {
                let ordinal = parsed.ordinal_of(api.name);
                assert_eq!(ordinal, Some(api.ordinal), "{}", api.name);
                assert!(parsed.entry(api.ordinal).is_some(), "{}", api.name);
            }
            assert_eq!(builtin16::of_file(&file), Some(i), "{}", def.file);
            let Ok(Resource::Ansi(found)) = verdll::find_resource(&file) else {
                panic!("{} has no 16-bit version resource", def.file);
            };
            // The resource table rounds its size up to its alignment.
            assert_eq!(found[..version.len()], version[..], "{}", def.file);
            let root = verdll::parse(&Resource::Ansi(found)).unwrap();
            assert_eq!(root.key, b"VS_VERSION_INFO");
            // dwFileOS: VOS_DOS_WINDOWS16.
            assert_eq!(root.value[32..36], 0x0001_0001u32.to_le_bytes());
        }
    }

    /// A program's own NE file isn't taken for a system module's.
    #[test]
    fn another_file_is_no_system_module() {
        let mut b = ne::Builder::new("COMMDLG", ne::LIBMODULE);
        b.description("COMMDLG.DLL");
        assert_eq!(builtin16::of_file(&b.build()), None);
    }
}
