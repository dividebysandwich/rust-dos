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
//! gives for it. DOS shows them in the directory (disk/system_files.rs),
//! read-only; a file of the same name on the drive wins over one.
//!
//! Which DLLs are there is the loader's call (`loader::builtin_for`), so
//! LoadLibrary and the files agree. They follow the settings: `sync` runs
//! when the settings are applied, as a program starts and after a save
//! state loads.
//!
//! The Windows directory, C:\WINDOWS, has the programs of Windows that
//! are built in the same way: NOTEPAD.EXE (user32/notepad.rs), which
//! setups run to show a readme.

use super::loader::{self, SYSTEM_DIR, WINDOWS_DIR};
use super::stubs::{self, DLLS};
use super::version::WindowsVersion;
use crate::bus::Bus;

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
    let files = wanted
        .1
        .iter()
        .map(|&i| (DLLS[i].name.to_string(), stubs::file(i, version).into()))
        .collect();
    bus.disk.set_system_files(SYSTEM_DIR, files);
    let programs = if bus.win32.enabled {
        vec![(
            "NOTEPAD.EXE".to_string(),
            super::user32::notepad::file().into(),
        )]
    } else {
        Vec::new()
    };
    bus.disk.set_system_files(WINDOWS_DIR, programs);
    bus.win32.dll_files = Some(wanted);
}

/// The DLLs the system directory shows and the version of Windows they
/// are of (`sync`).
pub(super) type Shown = (WindowsVersion, Vec<usize>);
