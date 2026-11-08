//! The ordinals Windows 98 SE's DLLs export their functions under, which
//! the tables of the built-in DLLs (`Api::ordinal`, `Var::ordinal`) give
//! theirs, and the tests that hold the tables to them. A program may
//! import a function by its ordinal alone, and copy protections such as
//! SafeDisc import KERNEL32's functions that have no name (VxDCall0 at 1,
//! LoadLibrary16 at 35) that way, so the numbers have to be Windows' own.
//!
//! `ordinals/<dll>.txt` lists the exports of the DLL as Windows 98 SE has
//! it, made by `tools/win98_ordinals.py` from the DLLs on its CD (DirectX
//! 6.1's DDRAW, DSOUND, DINPUT and DirectMusic, the DirectX the built-in
//! Windows reports): a line an export, its ordinal and its name, or "-"
//! for one exported by ordinal only.
//!
//! A built-in function's ordinal follows these rules, which the tests
//! check:
//!
//! - One Windows 98 exports by name has the ordinal Windows gives it.
//! - One Windows exports by ordinal only has that ordinal, and its name
//!   too, so a program that asks for it by name still finds it:
//!   COMCTL32's DPA functions, SHELL32's ID list ones and KERNEL32's
//!   VxDCall0 are numbered as Windows numbers them. The lists don't say
//!   which function such an ordinal is, so `BY_ORDINAL_ONLY` names each
//!   one the built-in DLLs have.
//! - Any other, a function Windows 98 doesn't have, goes after the last
//!   of Windows' ordinals, where no program expects one of Windows'.
//!
//! The ordinals are the same under every `win32_version`. Programs import
//! by ordinal only what has no name, and that is the same on Windows 95.
//! The built-in DLLs Windows 98 SE doesn't have (GLIDE2X, GLIDE3X, D3D8,
//! DINPUT8) number their functions as the DLLs they stand for do.

use super::stubs::{DLLS, Dll};
use std::collections::HashMap;

/// The DLLs' export lists, by the built-in DLL's name.
static LISTS: &[(&str, &str)] = &[
    ("ADVAPI32.DLL", include_str!("ordinals/advapi32_dll.txt")),
    ("ADVPACK.DLL", include_str!("ordinals/advpack_dll.txt")),
    ("AMSTREAM.DLL", include_str!("ordinals/amstream_dll.txt")),
    ("AVIFIL32.DLL", include_str!("ordinals/avifil32_dll.txt")),
    ("COMCTL32.DLL", include_str!("ordinals/comctl32_dll.txt")),
    ("COMDLG32.DLL", include_str!("ordinals/comdlg32_dll.txt")),
    ("DCIMAN32.DLL", include_str!("ordinals/dciman32_dll.txt")),
    ("DDRAW.DLL", include_str!("ordinals/ddraw_dll.txt")),
    ("DINPUT.DLL", include_str!("ordinals/dinput_dll.txt")),
    ("DMBAND.DLL", include_str!("ordinals/dmband_dll.txt")),
    ("DMCOMPOS.DLL", include_str!("ordinals/dmcompos_dll.txt")),
    ("DMIME.DLL", include_str!("ordinals/dmime_dll.txt")),
    ("DMLOADER.DLL", include_str!("ordinals/dmloader_dll.txt")),
    ("DMSTYLE.DLL", include_str!("ordinals/dmstyle_dll.txt")),
    ("DMUSIC.DLL", include_str!("ordinals/dmusic_dll.txt")),
    ("DPLAY.DLL", include_str!("ordinals/dplay_dll.txt")),
    ("DPLAYX.DLL", include_str!("ordinals/dplayx_dll.txt")),
    ("DSOUND.DLL", include_str!("ordinals/dsound_dll.txt")),
    ("GDI32.DLL", include_str!("ordinals/gdi32_dll.txt")),
    ("GLU32.DLL", include_str!("ordinals/glu32_dll.txt")),
    ("IMAGEHLP.DLL", include_str!("ordinals/imagehlp_dll.txt")),
    ("IMM32.DLL", include_str!("ordinals/imm32_dll.txt")),
    ("KERNEL32.DLL", include_str!("ordinals/kernel32_dll.txt")),
    ("LZ32.DLL", include_str!("ordinals/lz32_dll.txt")),
    ("MPR.DLL", include_str!("ordinals/mpr_dll.txt")),
    ("MSACM32.DLL", include_str!("ordinals/msacm32_dll.txt")),
    ("MSIMG32.DLL", include_str!("ordinals/msimg32_dll.txt")),
    ("MSVCRT.DLL", include_str!("ordinals/msvcrt_dll.txt")),
    ("MSVFW32.DLL", include_str!("ordinals/msvfw32_dll.txt")),
    ("NTDLL.DLL", include_str!("ordinals/ntdll_dll.txt")),
    ("OLE32.DLL", include_str!("ordinals/ole32_dll.txt")),
    ("OLEAUT32.DLL", include_str!("ordinals/oleaut32_dll.txt")),
    ("OLEDLG.DLL", include_str!("ordinals/oledlg_dll.txt")),
    ("OLEPRO32.DLL", include_str!("ordinals/olepro32_dll.txt")),
    ("OPENGL32.DLL", include_str!("ordinals/opengl32_dll.txt")),
    ("QUARTZ.DLL", include_str!("ordinals/quartz_dll.txt")),
    ("SHELL32.DLL", include_str!("ordinals/shell32_dll.txt")),
    ("TAPI32.DLL", include_str!("ordinals/tapi32_dll.txt")),
    ("USER32.DLL", include_str!("ordinals/user32_dll.txt")),
    ("VERSION.DLL", include_str!("ordinals/version_dll.txt")),
    ("WINMM.DLL", include_str!("ordinals/winmm_dll.txt")),
    ("WINSPOOL.DRV", include_str!("ordinals/winspool_drv.txt")),
    ("WNASPI32.DLL", include_str!("ordinals/wnaspi32_dll.txt")),
    ("WOW32.DLL", include_str!("ordinals/wow32_dll.txt")),
    ("WS2_32.DLL", include_str!("ordinals/ws2_32_dll.txt")),
    ("WSOCK32.DLL", include_str!("ordinals/wsock32_dll.txt")),
];

/// A DLL's exports on Windows 98 SE.
struct Exports {
    /// The ordinal of each function exported by name.
    named: HashMap<&'static str, u16>,
    /// The ordinals of the functions exported by ordinal only.
    unnamed: Vec<u16>,
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
                unnamed.push(ordinal);
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
}

/// Windows 98 SE's exports of the DLL `dll`, if it has the DLL.
fn win98(dll: &str) -> Option<Exports> {
    let (_, list) = LISTS
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(dll))?;
    Some(Exports::parse(list))
}

/// The exported functions and variables of the built-in DLL `dll`, by
/// name and ordinal.
fn exports(dll: &Dll) -> impl Iterator<Item = (&'static str, u16)> + '_ {
    let apis = dll.apis.iter().map(|a| (a.name, a.ordinal));
    let vars = dll.vars.iter().map(|v| (v.name, v.ordinal));
    apis.chain(vars).filter(|e| e.1 != 0)
}

#[test]
fn every_export_has_windows_98s_ordinal_or_one_after_them() {
    let mut wrong = Vec::new();
    for dll in DLLS {
        let Some(win98) = win98(dll.name) else {
            continue;
        };
        for (name, ordinal) in exports(dll) {
            match win98.named.get(name) {
                Some(&real) if real != ordinal => wrong.push(format!(
                    "{}: {} is #{}, not #{}",
                    dll.name, name, real, ordinal
                )),
                Some(_) => {}
                None if ordinal > win98.last => {}
                None if BY_ORDINAL_ONLY.contains(&(dll.name, name, ordinal)) => {}
                None if win98.unnamed.contains(&ordinal) => wrong.push(format!(
                    "{}: {} is on #{}, which Windows exports without a name: \
                     list it in BY_ORDINAL_ONLY if it is that function",
                    dll.name, name, ordinal
                )),
                None => wrong.push(format!(
                    "{}: {} isn't Windows 98's, so goes after #{}, not at #{}",
                    dll.name, name, win98.last, ordinal
                )),
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn no_two_exports_of_a_dll_share_an_ordinal() {
    for dll in DLLS {
        let mut taken = HashMap::new();
        for (name, ordinal) in exports(dll) {
            if let Some(other) = taken.insert(ordinal, name) {
                panic!("{}: {} and {} are both #{}", dll.name, other, name, ordinal);
            }
        }
    }
}

#[test]
fn every_built_in_dll_windows_98_has_has_its_list() {
    // GLIDE2X and GLIDE3X are 3dfx's, and D3D8 and DINPUT8 DirectX 8's,
    // none of them Windows 98 SE's. A DLL added to the built-in ones
    // needs a list made by tools/win98_ordinals.py.
    for dll in DLLS {
        let other = ["GLIDE2X.DLL", "GLIDE3X.DLL", "D3D8.DLL", "DINPUT8.DLL"].contains(&dll.name);
        assert_eq!(win98(dll.name).is_some(), !other, "{}", dll.name);
    }
}

/// Every built-in export on an ordinal Windows 98 SE exports without a
/// name: the DLL, the function's name (Wine's, or Windows NT's) and the
/// ordinal. The lists can't tell which function an unnamed ordinal is, so
/// a function goes on one only by being listed here, and a program that
/// imports an ordinal the built-in DLL doesn't have fails to load rather
/// than binding to another function.
const BY_ORDINAL_ONLY: &[(&str, &str, u16)] = &[
    ("KERNEL32.DLL", "VxDCall0", 1),
    ("KERNEL32.DLL", "VxDCall1", 2),
    ("KERNEL32.DLL", "VxDCall2", 3),
    ("KERNEL32.DLL", "VxDCall3", 4),
    ("KERNEL32.DLL", "VxDCall4", 5),
    ("KERNEL32.DLL", "VxDCall5", 6),
    ("KERNEL32.DLL", "VxDCall6", 7),
    ("KERNEL32.DLL", "VxDCall7", 8),
    ("KERNEL32.DLL", "VxDCall8", 9),
    ("KERNEL32.DLL", "LoadLibrary16", 35),
    ("KERNEL32.DLL", "FreeLibrary16", 36),
    ("KERNEL32.DLL", "GetProcAddress16", 37),
    ("SHELL32.DLL", "ILFindLastID", 16),
    ("SHELL32.DLL", "ILRemoveLastID", 17),
    ("SHELL32.DLL", "ILClone", 18),
    ("SHELL32.DLL", "ILCloneFirst", 19),
    ("SHELL32.DLL", "ILIsEqual", 21),
    ("SHELL32.DLL", "ILIsParent", 23),
    ("SHELL32.DLL", "ILFindChild", 24),
    ("SHELL32.DLL", "ILCombine", 25),
    ("SHELL32.DLL", "SHILCreateFromPath", 28),
    ("SHELL32.DLL", "Shell_MergeMenus", 67),
    ("SHELL32.DLL", "StrRetToStrN", 96),
    ("SHELL32.DLL", "ILGetSize", 152),
    ("SHELL32.DLL", "ILGetNext", 153),
    ("SHELL32.DLL", "ILAppendID", 154),
    ("SHELL32.DLL", "ILFree", 155),
    ("SHELL32.DLL", "ILCreateFromPath", 157),
    ("SHELL32.DLL", "SHSimpleIDListFromPath", 162),
    ("SHELL32.DLL", "SHCreateDirectory", 165),
    ("SHELL32.DLL", "SHGetSpecialFolderPath", 175),
    ("SHELL32.DLL", "SHGetNewLinkInfoA", 179),
    ("SHELL32.DLL", "SHFree", 195),
    ("SHELL32.DLL", "SHAlloc", 196),
    ("COMCTL32.DLL", "Alloc", 71),
    ("COMCTL32.DLL", "ReAlloc", 72),
    ("COMCTL32.DLL", "Free", 73),
    ("COMCTL32.DLL", "GetSize", 74),
    ("COMCTL32.DLL", "CreateMRUListA", 151),
    ("COMCTL32.DLL", "FreeMRUList", 152),
    ("COMCTL32.DLL", "AddMRUStringA", 153),
    ("COMCTL32.DLL", "EnumMRUListA", 154),
    ("COMCTL32.DLL", "FindMRUStringA", 155),
    ("COMCTL32.DLL", "DelMRUString", 156),
    ("COMCTL32.DLL", "CreateMRUListLazyA", 157),
    ("COMCTL32.DLL", "AddMRUData", 167),
    ("COMCTL32.DLL", "FindMRUData", 169),
    ("COMCTL32.DLL", "Str_GetPtrA", 233),
    ("COMCTL32.DLL", "Str_SetPtrA", 234),
    ("COMCTL32.DLL", "DSA_Create", 320),
    ("COMCTL32.DLL", "DSA_Destroy", 321),
    ("COMCTL32.DLL", "DSA_GetItem", 322),
    ("COMCTL32.DLL", "DSA_GetItemPtr", 323),
    ("COMCTL32.DLL", "DSA_InsertItem", 324),
    ("COMCTL32.DLL", "DSA_SetItem", 325),
    ("COMCTL32.DLL", "DSA_DeleteItem", 326),
    ("COMCTL32.DLL", "DSA_DeleteAllItems", 327),
    ("COMCTL32.DLL", "DPA_Create", 328),
    ("COMCTL32.DLL", "DPA_Destroy", 329),
    ("COMCTL32.DLL", "DPA_Grow", 330),
    ("COMCTL32.DLL", "DPA_Clone", 331),
    ("COMCTL32.DLL", "DPA_GetPtr", 332),
    ("COMCTL32.DLL", "DPA_GetPtrIndex", 333),
    ("COMCTL32.DLL", "DPA_InsertPtr", 334),
    ("COMCTL32.DLL", "DPA_SetPtr", 335),
    ("COMCTL32.DLL", "DPA_DeletePtr", 336),
    ("COMCTL32.DLL", "DPA_DeleteAllPtrs", 337),
    ("COMCTL32.DLL", "DPA_Sort", 338),
    ("COMCTL32.DLL", "DPA_Search", 339),
    ("COMCTL32.DLL", "DPA_CreateEx", 340),
    ("COMCTL32.DLL", "DPA_EnumCallback", 385),
    ("COMCTL32.DLL", "DPA_DestroyCallback", 386),
    ("COMCTL32.DLL", "DSA_EnumCallback", 387),
    ("COMCTL32.DLL", "DSA_DestroyCallback", 388),
];

#[test]
fn functions_windows_exports_by_ordinal_only_have_its_ordinals() {
    for &(dll_name, name, ordinal) in BY_ORDINAL_ONLY {
        let dll = DLLS.iter().find(|d| d.name == dll_name).unwrap();
        let win98 = win98(dll_name).unwrap();
        assert!(
            exports(dll).any(|e| e == (name, ordinal)),
            "{dll_name}: {name} isn't exported as #{ordinal}"
        );
        assert!(
            win98.unnamed.contains(&ordinal) && !win98.named.contains_key(name),
            "{dll_name}: Windows doesn't export #{ordinal} by ordinal only"
        );
    }
}
