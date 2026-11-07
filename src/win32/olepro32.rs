//! OLEPRO32.DLL: OLE's standard font and picture objects, property
//! frames and colour translation, which Windows 95 and 98 both ship. A
//! program linked with MFC's OLE support imports OleCreateFontIndirect
//! from it by its ordinal, 253, for the fonts of OLE controls. Without
//! the DLL such a program, Discworld Noir's setup among them, doesn't
//! start.
//!
//! OleTranslateColor and DllCanUnloadNow are written. The font and
//! picture objects, the property frames, OleIconToCursor and the class
//! factory aren't yet: each is in the table with Windows 98 SE's
//! ordinal, so a program that imports it by name or by ordinal loads,
//! and calling it does what calling a missing function does
//! (`super::not_written`): the program ends with a message naming it.

use super::com::{E_INVALIDARG, S_OK};
use super::stubs::{Api, Conv, Dll};
use crate::cpu::Cpu;

pub static DLL: Dll = Dll {
    name: "OLEPRO32.DLL",
    base: 0xBF5C_0000,
    apis: APIS,
    vars: &[],
    init: None,
    term: None,
    vtables: &[],
};

const fn stdcall(
    name: &'static str,
    ordinal: u16,
    args: u8,
    handler: super::stubs::Handler,
) -> Api {
    Api {
        name,
        ordinal,
        args,
        conv: Conv::Stdcall,
        handler,
    }
}

/// Handlers for functions that aren't written yet, one a function, each
/// of which reports its own name.
macro_rules! not_written {
    ($($handler:ident: $name:literal),* $(,)?) => {
        $(
            fn $handler(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
                super::not_written(cpu, DLL.name, $name)
            }
        )*
    };
}

/// The functions, with Windows 98 SE's ordinals. Append only (see
/// `stubs`).
static APIS: &[Api] = &[
    stdcall("OleIconToCursor", 248, 2, icon_to_cursor),
    stdcall(
        "OleCreatePropertyFrameIndirect",
        249,
        1,
        property_frame_indirect,
    ),
    stdcall("OleCreatePropertyFrame", 250, 11, property_frame),
    stdcall("OleLoadPicture", 251, 5, load_picture),
    stdcall("OleCreatePictureIndirect", 252, 4, create_picture_indirect),
    stdcall("OleCreateFontIndirect", 253, 3, create_font_indirect),
    stdcall("OleTranslateColor", 254, 3, translate_color),
    stdcall("DllCanUnloadNow", 255, 0, dll_can_unload_now),
    stdcall("DllGetClassObject", 256, 3, dll_get_class_object),
    stdcall("DllRegisterServer", 257, 0, dll_register_server),
    stdcall("DllUnregisterServer", 258, 0, dll_unregister_server),
];

not_written! {
    icon_to_cursor: "OleIconToCursor",
    property_frame_indirect: "OleCreatePropertyFrameIndirect",
    property_frame: "OleCreatePropertyFrame",
    load_picture: "OleLoadPicture",
    create_picture_indirect: "OleCreatePictureIndirect",
    create_font_indirect: "OleCreateFontIndirect",
    dll_get_class_object: "DllGetClassObject",
    dll_register_server: "DllRegisterServer",
    dll_unregister_server: "DllUnregisterServer",
}

/// The last system colour index OleTranslateColor takes, as Wine's
/// takes: COLOR_MENUBAR. Windows 98's GetSysColor has the colours up to
/// COLOR_GRADIENTINACTIVECAPTION (28); one it doesn't have is black.
const LAST_SYSTEM_COLOR: u32 = 30;

/// A COLORREF's flag of a colour to match in the logical palette
/// (PALETTERGB).
const PALETTE_RGB: u32 = 0x0200_0000;

/// OleTranslateColor(clr, hpal, pcolorref): the COLORREF of the OLE_COLOR
/// `clr`, written to `pcolorref` unless that is NULL. The high byte says
/// what the colour is, and the translation follows Wine's:
///
/// - 00h, an RGB colour: itself, or with a palette its PALETTERGB.
/// - 01h, a PALETTEINDEX: itself, if a palette given has the entry.
/// - 02h, a PALETTERGB: itself.
/// - 80h, a system colour: GetSysColor of its low byte.
///
/// Any other high byte, a system colour past the last, or an index the
/// palette doesn't have, fails with E_INVALIDARG and writes nothing.
fn translate_color(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (color, palette, out) = (args[0], args[1], args[2]);
    let translated = match color >> 24 {
        0x00 if palette != 0 => color | PALETTE_RGB,
        0x00 | 0x02 => color,
        0x01 => {
            if palette != 0 && !palette_has(cpu, palette, color & 0xFFFF) {
                return Some(E_INVALIDARG);
            }
            color
        }
        0x80 => {
            let index = color & 0xFF;
            if index > LAST_SYSTEM_COLOR {
                return Some(E_INVALIDARG);
            }
            super::user32::sysparams::get_sys_color(cpu, &[index])?
        }
        _ => return Some(E_INVALIDARG),
    };
    if out != 0 {
        cpu.bus.guest_write_32(out, translated);
    }
    Some(S_OK)
}

/// Whether the palette `handle` is one and has an entry `index`.
fn palette_has(cpu: &mut Cpu, handle: u32, index: u32) -> bool {
    super::gdi32::init(cpu);
    super::gdi32::gdi(cpu)
        .palette(handle)
        .is_some_and(|p| (index as usize) < p.entries.len())
}

/// DllCanUnloadNow(): S_OK, as no object of the DLL's can be there for
/// a program to hold.
fn dll_can_unload_now(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(S_OK)
}
