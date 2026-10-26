//! The built-in DLLs as files in the system directory, C:\WINDOWS\SYSTEM,
//! as Windows has its DLLs there: programs and setups that look for a DLL
//! by its file (open it, GetFileAttributes, FindFirstFile) find the ones
//! LoadLibrary would load, GLIDE2X.DLL with a 3dfx card. MPROFILE.EXE,
//! Xenocracy's hardware profiler, opens C:\WINDOWS\SYSTEM\GLIDE2X.DLL to
//! tell whether the machine has Glide.
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

use super::loader::{self, SYSTEM_DIR};
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
/// version of Windows the programs see; none while the built-in Windows
/// is off.
pub fn sync(bus: &mut Bus) {
    let win32 = &bus.win32;
    let version = if win32.processes.is_empty() {
        win32.version
    } else {
        win32.os
    };
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
    bus.win32.dll_files = Some(wanted);
}

/// The DLLs the system directory shows and the version of Windows they
/// are of (`sync`).
pub(super) type Shown = (WindowsVersion, Vec<usize>);
