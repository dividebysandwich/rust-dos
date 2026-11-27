//! DirectX's files whose work the built-in DLLs do, shown in the system
//! directory (dll_files.rs) with the version of the DirectX the registry
//! gives: D3DIM.DLL, Direct3D's immediate mode, which the built-in
//! DDRAW.DLL has in it; DDRAW16.DLL, DirectDraw's 16-bit half; and
//! DDHELP.EXE, DirectDraw's helper process. A machine with that DirectX
//! has all three, and nothing but DirectDraw itself uses them.
//!
//! DirectX's setups look at these files. DirectX 3's DXSETUP.EXE, which
//! Amok's CD runs from its menu and Beasts and Bumpkins' setup runs when
//! the player asks for DirectX, loads D3DIM.DLL to show Direct3D's
//! version, and its DSETUP.DLL copies each of its files that is missing
//! or no newer than its own into the system directory through
//! WININIT.INI. Without these files it put DirectX 3's Direct3D and
//! DirectDraw helpers there at the next start; with them it leaves them
//! as a real machine with a newer DirectX does.
//!
//! Each file has a version resource and no exports: D3DIM.DLL a DLL
//! without an entry point, DDRAW16.DLL a 16-bit DLL whose LibMain
//! returns 1, and DDHELP.EXE a program that ends at once. D3DXOF.DLL,
//! DirectX's file format library, isn't among them: no built-in DLL
//! has its DirectXFileCreate, and a file without it would only turn a
//! missing DLL into a missing function. A setup puts its own there.

use super::verdll;
use super::version::WindowsVersion;
use rust_dos_ne as ne;
use rust_dos_pe::{self as pe, Builder};

/// Where each 32-bit file is loaded: where DirectX 6.1's are, as Windows
/// 98 SE has them.
const D3DIM_BASE: u32 = 0x7EAC_0000;
const DDHELP_BASE: u32 = 0x0100_0000;

/// RT_VERSION, the version resource's name, and the language it is in.
const RT_VERSION: u16 = 16;
const VS_VERSION_INFO: u16 = 1;
const LANG_ENGLISH_US: u16 = 0x0409;

/// The files, by name, as the system directory shows them under
/// `windows`.
pub fn files(windows: WindowsVersion) -> Vec<(String, Vec<u8>)> {
    vec![
        ("D3DIM.DLL".to_string(), d3dim(windows)),
        ("DDRAW16.DLL".to_string(), ddraw16(windows)),
        ("DDHELP.EXE".to_string(), ddhelp(windows)),
    ]
}

fn d3dim(windows: WindowsVersion) -> Vec<u8> {
    let mut b = Builder::dll(D3DIM_BASE, "D3DIM.dll");
    let version = verdll::directx_file_version("D3DIM.DLL", windows);
    b.resource(RT_VERSION, VS_VERSION_INFO, LANG_ENGLISH_US, &version);
    b.build()
}

fn ddhelp(windows: WindowsVersion) -> Vec<u8> {
    let mut b = Builder::exe(DDHELP_BASE);
    b.subsystem(pe::SUBSYSTEM_GUI);
    // xor eax, eax; ret: the program ends with exit code 0.
    let text = b.section(
        ".text",
        &[0x31, 0xC0, 0xC3],
        pe::SCN_CODE | pe::SCN_EXECUTE | pe::SCN_READ,
    );
    b.entry(text);
    let version = verdll::directx_file_version("DDHELP.EXE", windows);
    b.resource(RT_VERSION, VS_VERSION_INFO, LANG_ENGLISH_US, &version);
    b.build()
}

fn ddraw16(windows: WindowsVersion) -> Vec<u8> {
    let mut b = ne::Builder::new("DDRAW16", ne::LIBMODULE | ne::PROTMODE);
    // mov ax, 1; retf: LibMain succeeds.
    let code = b.segment(ne::SegmentSpec {
        data: vec![0xB8, 0x01, 0x00, 0xCB],
        flags: ne::SEG_READONLY,
        alloc: 0,
        relocations: Vec::new(),
    });
    b.entry_point(code, 0);
    b.description("DDRAW16.DLL");
    let version = verdll::builtin16_version("DDRAW16.DLL", windows);
    b.resource(
        ne::ResId::Int(RT_VERSION),
        ne::ResId::Int(VS_VERSION_INFO),
        0,
        &version,
    );
    b.build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use verdll::Resource;

    /// The file version (MS, LS) of a version resource's fixed part.
    fn version_of(resource: Resource) -> (u32, u32) {
        let node = verdll::parse(&resource).unwrap();
        let at = |i: usize| u32::from_le_bytes(node.value[i..i + 4].try_into().unwrap());
        (at(8), at(12))
    }

    /// Each file is an image of its kind with the version of the DirectX
    /// the registry gives, which no DirectX 3 file is newer than.
    #[test]
    fn the_files_have_the_directx_version() {
        for windows in [WindowsVersion::Win95, WindowsVersion::Win98Se] {
            let [a, b, c, d] = super::super::version::directx(windows).file;
            let wanted = ((a as u32) << 16 | b as u32, (c as u32) << 16 | d as u32);
            for (name, file) in files(windows) {
                let resource = verdll::find_resource(&file).unwrap();
                if name == "DDRAW16.DLL" {
                    assert!(matches!(resource, Resource::Ansi(_)));
                    let module = ne::Module::parse(&file).unwrap();
                    assert_eq!(module.name(), "DDRAW16");
                    assert!(module.is_dll());
                } else {
                    assert!(matches!(resource, Resource::Unicode(_)));
                    let image = pe::Image::load(&file).unwrap();
                    assert_eq!(image.headers.is_dll(), name.ends_with(".DLL"), "{name}");
                }
                assert_eq!(version_of(resource), wanted, "{name}");
            }
        }
    }
}
