//! DLLs of Windows whose every export forwards to another DLL. Windows
//! 95 and 98 ship the C runtimes of older Visual C++ versions in the
//! system directory beside MSVCRT.DLL: MSVCRT20.DLL (Visual C++ 2), which
//! Windows 95's own MFC30.DLL uses, and MSVCRT40.DLL (Visual C++ 4),
//! which on Windows 98 forwards each of its functions to MSVCRT.DLL.
//! Programs and DLLs built with those compilers import them: Microsoft's
//! Reality Lab DLLs, which Warhammer: Shadow of the Horned Rat brings,
//! import MSVCRT20.DLL.
//!
//! Each is a file in the system directory (dll_files.rs) whose exports
//! are forwarders to the built-in MSVCRT.DLL's, as Windows 98's
//! MSVCRT40.DLL is. The loader loads it as any DLL file, follows its
//! forwarders and loads MSVCRT.DLL for them, so GetModuleHandle and
//! GetModuleFileName know the DLL by its own name. Its functions share
//! MSVCRT's state, its heap and its files; Visual C++ 2's own
//! MSVCRT20.DLL keeps its own, which a program only notices when it
//! hands a FILE or a block from one runtime to the other.

use super::stubs::{DLLS, find_dll};
use super::version::WindowsVersion;
use rust_dos_pe as pe;

/// A DLL that forwards every export to the built-in DLL `to`.
pub struct Forwarder {
    /// Its file name.
    pub name: &'static str,
    /// Where it asks to be loaded.
    pub base: u32,
    /// The built-in DLL its exports go to.
    pub to: &'static str,
}

pub static FORWARDERS: &[Forwarder] = &[
    Forwarder {
        name: "MSVCRT20.DLL",
        base: 0xBF54_0000,
        to: "MSVCRT.DLL",
    },
    Forwarder {
        name: "MSVCRT40.DLL",
        base: 0xBF56_0000,
        to: "MSVCRT.DLL",
    },
];

/// RT_VERSION, the version resource's name, and the language it is in.
const RT_VERSION: u16 = 16;
const VS_VERSION_INFO: u16 = 1;
const LANG_ENGLISH_US: u16 = 0x0409;

/// The file of the forwarder `f` as `version` of Windows has it: every
/// function and variable the built-in DLL it forwards to exports by name
/// under that version, numbered from 1 in the order of their names, and
/// its version resource.
pub fn file(f: &Forwarder, version: WindowsVersion) -> Vec<u8> {
    let target = DLLS[find_dll(f.to).expect("a forwarder's DLL is built in")];
    let module = f.to.strip_suffix(".DLL").unwrap_or(f.to);
    let apis = target
        .apis
        .iter()
        .filter(|a| a.ordinal != 0 && super::version::exports(version, target.name, a.name))
        .map(|a| a.name);
    let vars = target
        .vars
        .iter()
        .filter(|v| v.ordinal != 0)
        .map(|v| v.name);
    let mut names: Vec<&str> = apis.chain(vars).collect();
    names.sort_unstable();
    names.dedup();
    let mut b = pe::Builder::dll(f.base, f.name);
    for (i, name) in names.into_iter().enumerate() {
        b.export_forward(Some(name), i as u16 + 1, &format!("{module}.{name}"));
    }
    let resource = super::verdll::builtin_version(f.name, version);
    b.resource(RT_VERSION, VS_VERSION_INFO, LANG_ENGLISH_US, &resource);
    b.build()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every export of a forwarder is a forwarder to MSVCRT's function
    /// of the same name, and MSVCRT exports each.
    #[test]
    fn every_export_forwards_to_msvcrt() {
        let msvcrt = super::super::stubs::image(
            find_dll("MSVCRT.DLL").unwrap(),
            WindowsVersion::Win98Se,
        );
        let msvcrt = msvcrt.exports().unwrap().unwrap();
        for f in FORWARDERS {
            let image = pe::Image::load(&file(f, WindowsVersion::Win98Se)).unwrap();
            assert_eq!(image.headers.image_base, f.base);
            let exports = image.exports().unwrap().unwrap();
            assert_eq!(exports.name, f.name);
            assert!(exports.by_name("malloc").is_some(), "{}", f.name);
            for e in &exports.exports {
                let name = e.name.as_deref().unwrap();
                assert_eq!(
                    e.target,
                    pe::ExportTarget::Forwarder(format!("MSVCRT.{name}")),
                    "{}",
                    f.name
                );
                assert!(msvcrt.by_name(name).is_some(), "MSVCRT has no {name}");
            }
        }
    }

    /// The forwarders' images don't overlap each other or a built-in
    /// DLL's.
    #[test]
    fn forwarders_dont_overlap_the_built_in_dlls() {
        let version = WindowsVersion::Win98Se;
        let mut spans: Vec<(u32, u32, &str)> = (0..DLLS.len())
            .map(|i| {
                let size = super::super::stubs::image(i, version).headers.size_of_image;
                (DLLS[i].base, DLLS[i].base + size, DLLS[i].name)
            })
            .collect();
        for f in FORWARDERS {
            let size = pe::Image::load(&file(f, version)).unwrap().headers.size_of_image;
            spans.push((f.base, f.base + size, f.name));
        }
        spans.sort();
        for pair in spans.windows(2) {
            let ((_, end, a), (start, _, b)) = (pair[0], pair[1]);
            assert!(end <= start, "{a} ends at {end:08X}, past {b}'s base {start:08X}");
        }
    }
}
