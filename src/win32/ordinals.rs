//! The ordinals Windows 98 SE's DLLs export their functions under, which
//! the built-in DLLs take for theirs. A program may import a function by
//! its ordinal alone, and copy protections such as SafeDisc import
//! KERNEL32's functions that have no name (VxDCall0 at 1, LoadLibrary16
//! at 35) that way, so the numbers have to be Windows' own.
//!
//! `ordinals/<dll>.txt` lists the exports of the DLL as Windows 98 SE has
//! it, made by `tools/win98_ordinals.py` from the DLLs of an installation
//! with DirectX 7 (whose DDRAW, DSOUND, DINPUT and DirectMusic DLLs those
//! are): a line an export, its ordinal and its name, or "-" for one
//! exported by ordinal only.
//!
//! A built-in function gets its ordinal by these rules:
//!
//! - One Windows 98 exports by name has the ordinal Windows gives it.
//! - One whose own ordinal (`Api::ordinal`) is one Windows exports by
//!   ordinal only keeps it: COMCTL32's DPA functions, SHELL32's ID list
//!   ones and KERNEL32's VxDCall0 are numbered as Windows numbers them.
//!   It keeps its name too, so a program that asks for it by name still
//!   finds it.
//! - Any other, a function Windows 98 doesn't have, goes after the last
//!   of Windows' ordinals: that plus its own.
//!
//! The ordinals are the same under every `win32_version`. Programs import
//! by ordinal only what has no name, and that is the same on Windows 95.

use std::collections::HashMap;
use std::sync::OnceLock;

/// The DLLs' export lists, by the built-in DLL's name.
static LISTS: &[(&str, &str)] = &[
    ("ADVAPI32.DLL", include_str!("ordinals/advapi32_dll.txt")),
    ("ADVPACK.DLL", include_str!("ordinals/advpack_dll.txt")),
    ("COMCTL32.DLL", include_str!("ordinals/comctl32_dll.txt")),
    ("COMDLG32.DLL", include_str!("ordinals/comdlg32_dll.txt")),
    ("DDRAW.DLL", include_str!("ordinals/ddraw_dll.txt")),
    ("DINPUT.DLL", include_str!("ordinals/dinput_dll.txt")),
    ("DMBAND.DLL", include_str!("ordinals/dmband_dll.txt")),
    ("DMCOMPOS.DLL", include_str!("ordinals/dmcompos_dll.txt")),
    ("DMIME.DLL", include_str!("ordinals/dmime_dll.txt")),
    ("DMLOADER.DLL", include_str!("ordinals/dmloader_dll.txt")),
    ("DMSTYLE.DLL", include_str!("ordinals/dmstyle_dll.txt")),
    ("DMUSIC.DLL", include_str!("ordinals/dmusic_dll.txt")),
    ("DPLAYX.DLL", include_str!("ordinals/dplayx_dll.txt")),
    ("DSOUND.DLL", include_str!("ordinals/dsound_dll.txt")),
    ("GDI32.DLL", include_str!("ordinals/gdi32_dll.txt")),
    ("IMAGEHLP.DLL", include_str!("ordinals/imagehlp_dll.txt")),
    ("KERNEL32.DLL", include_str!("ordinals/kernel32_dll.txt")),
    ("LZ32.DLL", include_str!("ordinals/lz32_dll.txt")),
    ("MPR.DLL", include_str!("ordinals/mpr_dll.txt")),
    ("MSIMG32.DLL", include_str!("ordinals/msimg32_dll.txt")),
    ("MSVCRT.DLL", include_str!("ordinals/msvcrt_dll.txt")),
    ("NTDLL.DLL", include_str!("ordinals/ntdll_dll.txt")),
    ("OLE32.DLL", include_str!("ordinals/ole32_dll.txt")),
    ("OLEAUT32.DLL", include_str!("ordinals/oleaut32_dll.txt")),
    ("OPENGL32.DLL", include_str!("ordinals/opengl32_dll.txt")),
    ("SHELL32.DLL", include_str!("ordinals/shell32_dll.txt")),
    ("USER32.DLL", include_str!("ordinals/user32_dll.txt")),
    ("VERSION.DLL", include_str!("ordinals/version_dll.txt")),
    ("WINMM.DLL", include_str!("ordinals/winmm_dll.txt")),
    ("WINSPOOL.DRV", include_str!("ordinals/winspool_drv.txt")),
    ("WOW32.DLL", include_str!("ordinals/wow32_dll.txt")),
    ("WS2_32.DLL", include_str!("ordinals/ws2_32_dll.txt")),
    ("WSOCK32.DLL", include_str!("ordinals/wsock32_dll.txt")),
];

/// A DLL's exports on Windows 98 SE.
pub struct Exports {
    /// The ordinal of each function exported by name.
    named: HashMap<&'static str, u16>,
    /// Whether each ordinal up to the last is one exported by ordinal only.
    unnamed: Vec<bool>,
    last: u16,
}

impl Exports {
    fn parse(list: &'static str) -> Exports {
        let mut named = HashMap::new();
        let mut unnamed = Vec::new();
        let mut last = 0;
        for line in list.lines() {
            let (ordinal, name) = line.split_once(' ').expect("an ordinal and a name");
            let ordinal: u16 = ordinal.parse().expect("an ordinal");
            last = last.max(ordinal);
            if name == "-" {
                if unnamed.len() <= ordinal as usize {
                    unnamed.resize(ordinal as usize + 1, false);
                }
                unnamed[ordinal as usize] = true;
            } else {
                named.insert(name, ordinal);
            }
        }
        Exports {
            named,
            unnamed,
            last,
        }
    }

    /// The ordinal the built-in function `name`, numbered `own` in its
    /// DLL's table, is exported under (see the module's notes).
    pub fn ordinal(&self, name: &str, own: u16) -> u16 {
        if let Some(&ordinal) = self.named.get(name) {
            ordinal
        } else if self.unnamed.get(own as usize) == Some(&true) {
            own
        } else {
            self.last + own
        }
    }
}

/// Windows 98 SE's exports of the DLL `dll`, if it has the DLL.
pub fn win98(dll: &str) -> Option<&'static Exports> {
    static PARSED: OnceLock<Vec<Exports>> = OnceLock::new();
    let parsed = PARSED.get_or_init(|| LISTS.iter().map(|(_, l)| Exports::parse(l)).collect());
    let i = LISTS
        .iter()
        .position(|(name, _)| name.eq_ignore_ascii_case(dll))?;
    Some(&parsed[i])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_export_of_a_built_in_dll_has_an_ordinal_of_its_own() {
        for dll in super::super::stubs::DLLS {
            let Some(win98) = win98(dll.name) else {
                continue;
            };
            let apis = dll.apis.iter().map(|a| (a.name, a.ordinal));
            let vars = dll.vars.iter().map(|v| (v.name, v.ordinal));
            let mut taken = HashMap::new();
            for (name, own) in apis.chain(vars).filter(|e| e.1 != 0) {
                let ordinal = win98.ordinal(name, own);
                if let Some(other) = taken.insert(ordinal, name) {
                    panic!("{}: {} and {} are both #{}", dll.name, other, name, ordinal);
                }
            }
        }
    }

    #[test]
    fn every_built_in_dll_windows_98_has_has_its_list() {
        // GLIDE2X is 3dfx's, and D3D8 and DINPUT8 DirectX 8's, none of
        // them Windows 98 SE's. A DLL added to the built-in ones needs a
        // list made by tools/win98_ordinals.py, or it keeps numbers of
        // its own.
        for dll in super::super::stubs::DLLS {
            let other = ["GLIDE2X.DLL", "D3D8.DLL", "DINPUT8.DLL"].contains(&dll.name);
            assert_eq!(win98(dll.name).is_some(), !other, "{}", dll.name);
        }
    }

    #[test]
    fn kernel32s_unnamed_functions_have_windows_98s_ordinals() {
        let kernel32 = win98("KERNEL32.DLL").unwrap();
        assert_eq!(kernel32.ordinal("VxDCall0", 1), 1);
        assert_eq!(kernel32.ordinal("VxDCall8", 9), 9);
        assert_eq!(kernel32.ordinal("LoadLibrary16", 35), 35);
        assert_eq!(kernel32.ordinal("GetProcAddress16", 37), 37);
        // Named ones take Windows' ordinal whatever their own is.
        assert_eq!(kernel32.ordinal("AddAtomA", 1), 50);
        // One Windows 98 doesn't have goes after its last.
        assert_eq!(kernel32.ordinal("Win16Return", 950), 865 + 950);
    }
}
