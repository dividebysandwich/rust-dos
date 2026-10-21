//! DDRAW.DLL: DirectDraw: the display's surfaces, palettes and clippers,
//! in full screen on the VBE modes and in a window on the desktop.
//!
//! A DirectDraw object, a surface, a palette and a clipper are each an
//! `Object` in DirectDraw's table, with an interface block (com.rs) for
//! each interface the program asked for: IDirectDraw, 2, 4 and 7 of one
//! DirectDraw object, IDirectDrawSurface to 7 of a surface. As on
//! Windows each interface has its own reference count, and an object
//! goes when all its counts are 0. A surface made through IDirectDraw4
//! or 7 holds a reference on that interface; one made through
//! IDirectDraw or IDirectDraw2 holds none, and when its DirectDraw
//! object goes it stays as a surface that is lost until the program
//! releases it.
//!
//! The pixels of surfaces in video memory are in the card's 4 MB of VBE
//! memory, which each process that uses DirectDraw has mapped whole
//! (`display.rs`); those of surfaces in system memory are in pages of the
//! process. In a window the primary surface is the desktop at the start
//! of video memory; in full screen, with a mode DirectDraw set, it is
//! the mode's picture, and GDI's drawing on the screen goes nowhere until
//! the mode is given back.
//!
//! (Phase 3 of docs/native-windows.md.)

/// The value of a `Result` whose error is an HRESULT, or the method
/// returns the error.
macro_rules! hr {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(e) => return Some(e),
        }
    };
}

mod blt;
mod clipper;
mod directdraw;
mod display;
pub mod enumerate;
mod palette;
mod surface;

use super::com::{self, E_INVALIDARG, E_NOINTERFACE, Guid, IID_IUNKNOWN, S_OK};
use super::stubs::{Api, Conv, Dll, Vtable};
use crate::cpu::Cpu;

pub use display::owns_display;

pub static DLL: Dll = Dll {
    name: "DDRAW.DLL",
    base: 0xBFA0_0000,
    apis: APIS,
    vars: &[],
    init: None,
    vtables: VTABLES,
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

/// A method: stdcall, not exported.
const fn method(name: &'static str, args: u8, handler: super::stubs::Handler) -> Api {
    stdcall(name, 0, args, handler)
}

/// The functions. Append only (see `stubs`). The methods of the objects'
/// interfaces have ordinal 0: they aren't exported.
static APIS: &[Api] = &[
    stdcall("DllCanUnloadNow", 1, 0, dll_can_unload_now),
    stdcall("DllGetClassObject", 2, 3, dll_get_class_object),
    stdcall("DirectDrawCreate", 3, 3, directdraw::direct_draw_create),
    stdcall(
        "DirectDrawCreateEx",
        4,
        4,
        directdraw::direct_draw_create_ex,
    ),
    stdcall(
        "DirectDrawEnumerateA",
        5,
        2,
        enumerate::direct_draw_enumerate,
    ),
    stdcall("DirectDrawEnumerateW", 6, 2, enumerate::unsupported),
    stdcall(
        "DirectDrawEnumerateExA",
        7,
        3,
        enumerate::direct_draw_enumerate_ex,
    ),
    stdcall("DirectDrawEnumerateExW", 8, 3, enumerate::unsupported),
    stdcall(
        "DirectDrawCreateClipper",
        9,
        3,
        clipper::direct_draw_create_clipper,
    ),
    Api {
        name: "EnumerateHandler",
        ordinal: 0,
        args: 4,
        conv: Conv::Cdecl,
        handler: enumerate::handler,
    },
    method("DD_QueryInterface", 3, query_interface),
    method("DD_AddRef", 1, add_ref_method),
    method("DD_Release", 1, release_method),
    method("IDirectDraw_Compact", 1, directdraw::compact),
    method("IDirectDraw_CreateClipper", 4, clipper::create_clipper),
    method("IDirectDraw_CreatePalette", 5, palette::create_palette),
    method("IDirectDraw_CreateSurface", 4, surface::create_surface),
    method(
        "IDirectDraw_DuplicateSurface",
        3,
        surface::duplicate_surface,
    ),
    method(
        "IDirectDraw_EnumDisplayModes",
        5,
        enumerate::enum_display_modes,
    ),
    method("IDirectDraw_EnumSurfaces", 5, enumerate::enum_surfaces),
    method(
        "IDirectDraw_FlipToGDISurface",
        1,
        surface::flip_to_gdi_surface,
    ),
    method("IDirectDraw_GetCaps", 3, directdraw::get_caps),
    method(
        "IDirectDraw_GetDisplayMode",
        2,
        directdraw::get_display_mode,
    ),
    method(
        "IDirectDraw_GetFourCCCodes",
        3,
        directdraw::get_four_cc_codes,
    ),
    method("IDirectDraw_GetGDISurface", 2, surface::get_gdi_surface),
    method(
        "IDirectDraw_GetMonitorFrequency",
        2,
        directdraw::get_monitor_frequency,
    ),
    method("IDirectDraw_GetScanLine", 2, directdraw::get_scan_line),
    method(
        "IDirectDraw_GetVerticalBlankStatus",
        2,
        directdraw::get_vertical_blank_status,
    ),
    method("IDirectDraw_Initialize", 2, directdraw::initialize),
    method(
        "IDirectDraw_RestoreDisplayMode",
        1,
        directdraw::restore_display_mode,
    ),
    method(
        "IDirectDraw_SetCooperativeLevel",
        3,
        directdraw::set_cooperative_level,
    ),
    method(
        "IDirectDraw_SetDisplayMode",
        4,
        directdraw::set_display_mode,
    ),
    method(
        "IDirectDraw_WaitForVerticalBlank",
        3,
        directdraw::wait_for_vertical_blank,
    ),
    method(
        "IDirectDraw2_SetDisplayMode",
        6,
        directdraw::set_display_mode,
    ),
    method(
        "IDirectDraw2_GetAvailableVidMem",
        4,
        directdraw::get_available_vid_mem,
    ),
    method(
        "IDirectDraw4_GetSurfaceFromDC",
        3,
        surface::get_surface_from_dc,
    ),
    method(
        "IDirectDraw4_RestoreAllSurfaces",
        1,
        surface::restore_all_surfaces,
    ),
    method(
        "IDirectDraw4_TestCooperativeLevel",
        1,
        directdraw::test_cooperative_level,
    ),
    method(
        "IDirectDraw4_GetDeviceIdentifier",
        3,
        directdraw::get_device_identifier,
    ),
    method("IDirectDraw7_StartModeTest", 4, directdraw::unsupported),
    method("IDirectDraw7_EvaluateMode", 3, directdraw::unsupported),
    method(
        "Surface_AddAttachedSurface",
        2,
        surface::add_attached_surface,
    ),
    method("Surface_AddOverlayDirtyRect", 2, surface::not_overlay),
    method("Surface_Blt", 6, blt::blt),
    method("Surface_BltBatch", 4, directdraw::unsupported),
    method("Surface_BltFast", 6, blt::blt_fast),
    method(
        "Surface_DeleteAttachedSurface",
        3,
        surface::delete_attached_surface,
    ),
    method(
        "Surface_EnumAttachedSurfaces",
        3,
        enumerate::enum_attached_surfaces,
    ),
    method("Surface_EnumOverlayZOrders", 4, surface::not_overlay),
    method("Surface_Flip", 3, surface::flip),
    method(
        "Surface_GetAttachedSurface",
        3,
        surface::get_attached_surface,
    ),
    method("Surface_GetBltStatus", 2, surface::get_blt_status),
    method("Surface_GetCaps", 2, surface::get_caps),
    method("Surface_GetClipper", 2, surface::get_clipper),
    method("Surface_GetColorKey", 3, surface::get_color_key),
    method("Surface_GetDC", 2, surface::get_dc),
    method("Surface_GetFlipStatus", 2, surface::get_flip_status),
    method("Surface_GetOverlayPosition", 3, surface::not_overlay),
    method("Surface_GetPalette", 2, surface::get_palette),
    method("Surface_GetPixelFormat", 2, surface::get_pixel_format),
    method("Surface_GetSurfaceDesc", 2, surface::get_surface_desc),
    method("Surface_Initialize", 3, surface::initialize),
    method("Surface_IsLost", 1, surface::is_lost),
    method("Surface_Lock", 5, surface::lock),
    method("Surface_ReleaseDC", 2, surface::release_dc),
    method("Surface_Restore", 1, surface::restore),
    method("Surface_SetClipper", 2, surface::set_clipper),
    method("Surface_SetColorKey", 3, surface::set_color_key),
    method("Surface_SetOverlayPosition", 3, surface::not_overlay),
    method("Surface_SetPalette", 2, surface::set_palette),
    method("Surface_Unlock", 2, surface::unlock),
    method("Surface_UpdateOverlay", 6, surface::not_overlay),
    method("Surface_UpdateOverlayDisplay", 2, surface::not_overlay),
    method("Surface_UpdateOverlayZOrder", 3, surface::not_overlay),
    method("Surface2_GetDDInterface", 2, surface::get_dd_interface),
    method("Surface2_PageLock", 2, surface::page_lock),
    method("Surface2_PageUnlock", 2, surface::page_lock),
    method("Surface3_SetSurfaceDesc", 3, surface::set_surface_desc),
    method("Surface4_SetPrivateData", 5, surface::set_private_data),
    method("Surface4_GetPrivateData", 4, surface::get_private_data),
    method("Surface4_FreePrivateData", 2, surface::free_private_data),
    method(
        "Surface4_GetUniquenessValue",
        2,
        surface::get_uniqueness_value,
    ),
    method(
        "Surface4_ChangeUniquenessValue",
        1,
        surface::change_uniqueness_value,
    ),
    method("Surface7_SetPriority", 2, surface::set_priority),
    method("Surface7_GetPriority", 2, surface::get_priority),
    method("Surface7_SetLOD", 2, surface::set_lod),
    method("Surface7_GetLOD", 2, surface::get_lod),
    method("Palette_GetCaps", 2, palette::get_caps),
    method("Palette_GetEntries", 5, palette::get_entries),
    method("Palette_Initialize", 4, palette::initialize),
    method("Palette_SetEntries", 5, palette::set_entries),
    method("Clipper_GetClipList", 4, clipper::get_clip_list),
    method("Clipper_GetHWnd", 2, clipper::get_hwnd),
    method("Clipper_Initialize", 3, clipper::initialize),
    method(
        "Clipper_IsClipListChanged",
        2,
        clipper::is_clip_list_changed,
    ),
    method("Clipper_SetClipList", 3, clipper::set_clip_list),
    method("Clipper_SetHWnd", 3, clipper::set_hwnd),
];

const IUNKNOWN: [&str; 3] = ["DD_QueryInterface", "DD_AddRef", "DD_Release"];

/// IDirectDraw's methods after IUnknown's, with the SetDisplayMode of
/// the version, which takes more arguments from IDirectDraw2 on.
const fn ddraw_methods(set_display_mode: &'static str) -> [&'static str; 20] {
    [
        "IDirectDraw_Compact",
        "IDirectDraw_CreateClipper",
        "IDirectDraw_CreatePalette",
        "IDirectDraw_CreateSurface",
        "IDirectDraw_DuplicateSurface",
        "IDirectDraw_EnumDisplayModes",
        "IDirectDraw_EnumSurfaces",
        "IDirectDraw_FlipToGDISurface",
        "IDirectDraw_GetCaps",
        "IDirectDraw_GetDisplayMode",
        "IDirectDraw_GetFourCCCodes",
        "IDirectDraw_GetGDISurface",
        "IDirectDraw_GetMonitorFrequency",
        "IDirectDraw_GetScanLine",
        "IDirectDraw_GetVerticalBlankStatus",
        "IDirectDraw_Initialize",
        "IDirectDraw_RestoreDisplayMode",
        "IDirectDraw_SetCooperativeLevel",
        set_display_mode,
        "IDirectDraw_WaitForVerticalBlank",
    ]
}

const DD1: [&str; 23] = concat3(IUNKNOWN, ddraw_methods("IDirectDraw_SetDisplayMode"), []);
const DD2: [&str; 24] = concat3(
    IUNKNOWN,
    ddraw_methods("IDirectDraw2_SetDisplayMode"),
    ["IDirectDraw2_GetAvailableVidMem"],
);
const DD4: [&str; 28] = concat3(
    DD2,
    [
        "IDirectDraw4_GetSurfaceFromDC",
        "IDirectDraw4_RestoreAllSurfaces",
        "IDirectDraw4_TestCooperativeLevel",
        "IDirectDraw4_GetDeviceIdentifier",
    ],
    [],
);
const DD7: [&str; 30] = concat3(
    DD4,
    ["IDirectDraw7_StartModeTest", "IDirectDraw7_EvaluateMode"],
    [],
);

const SURFACE1: [&str; 36] = concat3(
    IUNKNOWN,
    [
        "Surface_AddAttachedSurface",
        "Surface_AddOverlayDirtyRect",
        "Surface_Blt",
        "Surface_BltBatch",
        "Surface_BltFast",
        "Surface_DeleteAttachedSurface",
        "Surface_EnumAttachedSurfaces",
        "Surface_EnumOverlayZOrders",
        "Surface_Flip",
        "Surface_GetAttachedSurface",
        "Surface_GetBltStatus",
        "Surface_GetCaps",
        "Surface_GetClipper",
        "Surface_GetColorKey",
        "Surface_GetDC",
        "Surface_GetFlipStatus",
        "Surface_GetOverlayPosition",
        "Surface_GetPalette",
        "Surface_GetPixelFormat",
        "Surface_GetSurfaceDesc",
        "Surface_Initialize",
        "Surface_IsLost",
        "Surface_Lock",
        "Surface_ReleaseDC",
        "Surface_Restore",
        "Surface_SetClipper",
        "Surface_SetColorKey",
        "Surface_SetOverlayPosition",
        "Surface_SetPalette",
        "Surface_Unlock",
        "Surface_UpdateOverlay",
        "Surface_UpdateOverlayDisplay",
        "Surface_UpdateOverlayZOrder",
    ],
    [],
);
const SURFACE2: [&str; 39] = concat3(
    SURFACE1,
    [
        "Surface2_GetDDInterface",
        "Surface2_PageLock",
        "Surface2_PageUnlock",
    ],
    [],
);
const SURFACE3: [&str; 40] = concat3(SURFACE2, ["Surface3_SetSurfaceDesc"], []);
const SURFACE4: [&str; 45] = concat3(
    SURFACE3,
    [
        "Surface4_SetPrivateData",
        "Surface4_GetPrivateData",
        "Surface4_FreePrivateData",
        "Surface4_GetUniquenessValue",
        "Surface4_ChangeUniquenessValue",
    ],
    [],
);
const SURFACE7: [&str; 49] = concat3(
    SURFACE4,
    [
        "Surface7_SetPriority",
        "Surface7_GetPriority",
        "Surface7_SetLOD",
        "Surface7_GetLOD",
    ],
    [],
);

/// Three lists of names one after the other, as one.
const fn concat3<const A: usize, const B: usize, const C: usize, const N: usize>(
    a: [&'static str; A],
    b: [&'static str; B],
    c: [&'static str; C],
) -> [&'static str; N] {
    let mut out = [""; N];
    let mut i = 0;
    while i < A {
        out[i] = a[i];
        i += 1;
    }
    while i < A + B {
        out[i] = b[i - A];
        i += 1;
    }
    while i < A + B + C {
        out[i] = c[i - A - B];
        i += 1;
    }
    out
}

/// The vtables, by their places in this list. Append only.
static VTABLES: &[Vtable] = &[
    Vtable {
        name: "IDirectDraw",
        methods: &DD1,
    },
    Vtable {
        name: "IDirectDraw2",
        methods: &DD2,
    },
    Vtable {
        name: "IDirectDraw4",
        methods: &DD4,
    },
    Vtable {
        name: "IDirectDraw7",
        methods: &DD7,
    },
    Vtable {
        name: "IDirectDrawSurface",
        methods: &SURFACE1,
    },
    Vtable {
        name: "IDirectDrawSurface2",
        methods: &SURFACE2,
    },
    Vtable {
        name: "IDirectDrawSurface3",
        methods: &SURFACE3,
    },
    Vtable {
        name: "IDirectDrawSurface4",
        methods: &SURFACE4,
    },
    Vtable {
        name: "IDirectDrawSurface7",
        methods: &SURFACE7,
    },
    Vtable {
        name: "IDirectDrawPalette",
        methods: &[
            "DD_QueryInterface",
            "DD_AddRef",
            "DD_Release",
            "Palette_GetCaps",
            "Palette_GetEntries",
            "Palette_Initialize",
            "Palette_SetEntries",
        ],
    },
    Vtable {
        name: "IDirectDrawClipper",
        methods: &[
            "DD_QueryInterface",
            "DD_AddRef",
            "DD_Release",
            "Clipper_GetClipList",
            "Clipper_GetHWnd",
            "Clipper_Initialize",
            "Clipper_IsClipListChanged",
            "Clipper_SetClipList",
            "Clipper_SetHWnd",
        ],
    },
];

/// The vtables' places.
pub const V_DD1: usize = 0;
pub const V_DD2: usize = 1;
pub const V_DD4: usize = 2;
pub const V_DD7: usize = 3;
pub const V_SURFACE1: usize = 4;
pub const V_SURFACE2: usize = 5;
pub const V_SURFACE3: usize = 6;
pub const V_SURFACE4: usize = 7;
pub const V_SURFACE7: usize = 8;
pub const V_PALETTE: usize = 9;
pub const V_CLIPPER: usize = 10;

/// The version of IDirectDraw or IDirectDrawSurface a vtable is.
pub fn version(vtable: usize) -> u32 {
    match vtable {
        V_DD1 | V_SURFACE1 => 1,
        V_DD2 | V_SURFACE2 => 2,
        V_SURFACE3 => 3,
        V_DD4 | V_SURFACE4 => 4,
        _ => 7,
    }
}

/// The IDirectDraw vtable of a version.
pub fn vtable_for_version(version: u32) -> usize {
    match version {
        1 => V_DD1,
        2 => V_DD2,
        4 => V_DD4,
        _ => V_DD7,
    }
}

// HRESULTs.

pub const DD_OK: u32 = S_OK;
const fn dderr(code: u32) -> u32 {
    0x8876_0000 | code
}
pub const DDERR_ALREADYINITIALIZED: u32 = dderr(5);
pub const DDERR_CANNOTATTACHSURFACE: u32 = dderr(10);
pub const DDERR_CANNOTDETACHSURFACE: u32 = dderr(20);
pub const DDERR_INVALIDCAPS: u32 = dderr(100);
pub const DDERR_INVALIDCLIPLIST: u32 = dderr(110);
pub const DDERR_INVALIDMODE: u32 = dderr(120);
pub const DDERR_INVALIDOBJECT: u32 = dderr(130);
pub const DDERR_INVALIDPARAMS: u32 = E_INVALIDARG;
pub const DDERR_INVALIDPIXELFORMAT: u32 = dderr(145);
pub const DDERR_INVALIDRECT: u32 = dderr(150);
pub const DDERR_NOCLIPLIST: u32 = dderr(205);
pub const DDERR_NOCOOPERATIVELEVELSET: u32 = dderr(212);
pub const DDERR_NOCOLORKEY: u32 = dderr(215);
pub const DDERR_NOEXCLUSIVEMODE: u32 = dderr(225);
pub const DDERR_NOTFOUND: u32 = dderr(255);
pub const DDERR_NOOVERLAYHW: u32 = dderr(260);
pub const DDERR_NORASTEROPHW: u32 = dderr(280);
pub const DDERR_OUTOFVIDEOMEMORY: u32 = dderr(380);
pub const DDERR_SURFACEALREADYATTACHED: u32 = dderr(410);
pub const DDERR_SURFACEBUSY: u32 = dderr(430);
pub const DDERR_SURFACELOST: u32 = dderr(450);
pub const DDERR_UNSUPPORTED: u32 = com::E_NOTIMPL;
pub const DDERR_VERTICALBLANKINPROGRESS: u32 = dderr(537);
pub const DDERR_INVALIDDIRECTDRAWGUID: u32 = dderr(561);
pub const DDERR_PRIMARYSURFACEALREADYEXISTS: u32 = dderr(564);
pub const DDERR_REGIONTOOSMALL: u32 = dderr(566);
pub const DDERR_CLIPPERISUSINGHWND: u32 = dderr(567);
pub const DDERR_NOCLIPPERATTACHED: u32 = dderr(568);
pub const DDERR_HWNDALREADYSET: u32 = dderr(571);
pub const DDERR_NOPALETTEATTACHED: u32 = dderr(572);
pub const DDERR_BLTFASTCANTCLIP: u32 = dderr(574);
pub const DDERR_NOTAOVERLAYSURFACE: u32 = dderr(580);
pub const DDERR_EXCLUSIVEMODEALREADYSET: u32 = dderr(581);
pub const DDERR_NOTFLIPPABLE: u32 = dderr(582);
pub const DDERR_CANTDUPLICATE: u32 = dderr(583);
pub const DDERR_NOTLOCKED: u32 = dderr(584);
pub const DDERR_CANTCREATEDC: u32 = dderr(585);
pub const DDERR_NODC: u32 = dderr(586);
pub const DDERR_WRONGMODE: u32 = dderr(587);
pub const DDERR_DCALREADYCREATED: u32 = dderr(620);
pub const DDERR_WASSTILLDRAWING: u32 = dderr(540);
pub const DDERR_EXPIRED: u32 = dderr(691);
pub const DDERR_NOALPHAHW: u32 = dderr(180);
pub const DDERR_NOROTATIONHW: u32 = dderr(290);
pub const DDERR_NOZBUFFERHW: u32 = dderr(340);
pub const DDERR_MOREDATA: u32 = dderr(690);
pub const DDERR_NOTINITIALIZED: u32 = com::CO_E_NOTINITIALIZED;

// Interfaces.

const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Guid {
    Guid::new(d1, d2, d3, d4)
}
const IID_DD1: Guid = guid(
    0x6C14_DB80,
    0xA733,
    0x11CE,
    [0xA5, 0x21, 0x00, 0x20, 0xAF, 0x0B, 0xE5, 0x60],
);
const IID_DD2: Guid = guid(
    0xB3A6_F3E0,
    0x2B43,
    0x11CF,
    [0xA2, 0xDE, 0x00, 0xAA, 0x00, 0xB9, 0x33, 0x56],
);
const IID_DD4: Guid = guid(
    0x9C59_509A,
    0x39BD,
    0x11D1,
    [0x8C, 0x4A, 0x00, 0xC0, 0x4F, 0xD9, 0x30, 0xC5],
);
pub const IID_DD7: Guid = guid(
    0x15E6_5EC0,
    0x3B9C,
    0x11D2,
    [0xB9, 0x2F, 0x00, 0x60, 0x97, 0x97, 0xEA, 0x5B],
);
const IID_SURFACE1: Guid = guid(
    0x6C14_DB81,
    0xA733,
    0x11CE,
    [0xA5, 0x21, 0x00, 0x20, 0xAF, 0x0B, 0xE5, 0x60],
);
const IID_SURFACE2: Guid = guid(
    0x5780_5885,
    0x6EEC,
    0x11CF,
    [0x94, 0x41, 0xA8, 0x23, 0x03, 0xC1, 0x0E, 0x27],
);
const IID_SURFACE3: Guid = guid(
    0xDA04_4E00,
    0x69B2,
    0x11D0,
    [0xA1, 0xD5, 0x00, 0xAA, 0x00, 0xB8, 0xDF, 0xBB],
);
const IID_SURFACE4: Guid = guid(
    0x0B2B_8630,
    0xAD35,
    0x11D0,
    [0x8E, 0xA6, 0x00, 0x60, 0x97, 0x97, 0xEA, 0x5B],
);
const IID_SURFACE7: Guid = guid(
    0x0667_5A80,
    0x3B9B,
    0x11D2,
    [0xB9, 0x2F, 0x00, 0x60, 0x97, 0x97, 0xEA, 0x5B],
);
const IID_PALETTE: Guid = guid(
    0x6C14_DB84,
    0xA733,
    0x11CE,
    [0xA5, 0x21, 0x00, 0x20, 0xAF, 0x0B, 0xE5, 0x60],
);
const IID_CLIPPER: Guid = guid(
    0x6C14_DB85,
    0xA733,
    0x11CE,
    [0xA5, 0x21, 0x00, 0x20, 0xAF, 0x0B, 0xE5, 0x60],
);

/// The classes CoCreateInstance makes of DDRAW's: objects that need
/// their Initialize called before they do anything.
pub static CLASSES: &[com::Class] = &[
    com::Class {
        clsid: guid(
            0xD7B7_0EE0,
            0x4340,
            0x11CF,
            [0xB0, 0x63, 0x00, 0x20, 0xAF, 0xC2, 0xCD, 0x35],
        ),
        dll: "DDRAW.DLL",
        name: "DirectDraw",
        create: directdraw::create_instance,
    },
    com::Class {
        clsid: guid(
            0x3C30_5196,
            0x50DB,
            0x11D3,
            [0x9C, 0xFE, 0x00, 0xC0, 0x4F, 0xD9, 0x30, 0xC5],
        ),
        dll: "DDRAW.DLL",
        name: "DirectDraw7",
        create: directdraw::create_instance,
    },
    com::Class {
        clsid: guid(
            0x5938_17A0,
            0x7DB3,
            0x11CF,
            [0xA2, 0xDE, 0x00, 0xAA, 0x00, 0xB9, 0x33, 0x56],
        ),
        dll: "DDRAW.DLL",
        name: "DirectDrawClipper",
        create: clipper::create_instance,
    },
];

// The state.

/// A pixel format, as DDPIXELFORMAT has it after its size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Format {
    pub flags: u32,
    pub fourcc: u32,
    pub bits: u32,
    pub r: u32,
    pub g: u32,
    pub b: u32,
    pub a: u32,
}

/// One of an object's interfaces: its vtable, its block, and its
/// reference count.
#[derive(Clone, Debug, Default)]
pub struct Iface {
    pub vtable: u32,
    pub address: u32,
    pub refs: u32,
}

/// A DirectDraw object, a surface, a palette or a clipper: what is the
/// same for all of them, and what its kind has.
#[derive(Clone, Debug, Default)]
pub struct Object {
    /// The process it is in.
    pub pid: u32,
    pub ifaces: Vec<Iface>,
    /// The DirectDraw object it was made by, or 0, and the interface of
    /// it the program made it through (GetDDInterface's).
    pub owner: u32,
    pub creator: u32,
    /// Whether it holds a reference on `creator`: what IDirectDraw4 and
    /// 7 make does.
    pub holds_creator: bool,
    pub ddraw: Option<directdraw::DirectDraw>,
    pub surface: Option<surface::Surface>,
    pub palette: Option<palette::Palette>,
    pub clipper: Option<clipper::Clipper>,
}

impl Object {
    /// The references on all its interfaces.
    fn refs(&self) -> u32 {
        self.ifaces.iter().map(|i| i.refs).sum()
    }
}

/// DirectDraw's state: the system's.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// The objects, by their ids less 1.
    pub objects: Vec<Option<Object>>,
    /// The display: the mode DirectDraw set and the object with exclusive
    /// mode, video memory and the processes' views of it.
    pub display: display::Display,
    /// The last value GetUniquenessValue gave out.
    pub uniqueness: u32,
}

pub fn state(cpu: &mut Cpu) -> &mut State {
    &mut cpu.bus.win32.ddraw
}

fn dll_index() -> usize {
    super::stubs::DLLS
        .iter()
        .position(|d| d.name == DLL.name)
        .expect("DDRAW is a built-in DLL")
}

pub fn current_pid(cpu: &Cpu) -> u32 {
    super::user32::current_pid(cpu)
}

impl State {
    pub fn object(&self, id: u32) -> Option<&Object> {
        self.objects.get(id.checked_sub(1)? as usize)?.as_ref()
    }

    pub fn object_mut(&mut self, id: u32) -> Option<&mut Object> {
        self.objects.get_mut(id.checked_sub(1)? as usize)?.as_mut()
    }

    pub fn surface(&self, id: u32) -> Option<&surface::Surface> {
        self.object(id)?.surface.as_ref()
    }

    pub fn surface_mut(&mut self, id: u32) -> Option<&mut surface::Surface> {
        self.object_mut(id)?.surface.as_mut()
    }

    /// The ids of the objects for which `f` holds.
    pub fn ids(&self, f: impl Fn(&Object) -> bool) -> Vec<u32> {
        self.objects
            .iter()
            .enumerate()
            .filter(|(_, o)| o.as_ref().is_some_and(&f))
            .map(|(i, _)| i as u32 + 1)
            .collect()
    }

    /// Put `object` in the table: its id.
    fn add(&mut self, object: Object) -> u32 {
        match self.objects.iter().position(Option::is_none) {
            Some(i) => {
                self.objects[i] = Some(object);
                i as u32 + 1
            }
            None => {
                self.objects.push(Some(object));
                self.objects.len() as u32
            }
        }
    }
}

/// Put `object` in the table, as the running process's: its id.
pub fn add_object(cpu: &mut Cpu, mut object: Object) -> u32 {
    object.pid = current_pid(cpu);
    state(cpu).add(object)
}

/// The interface `vtable` of the object `id`, made if it has none yet:
/// its address, or 0 when the heap is full.
pub fn iface(cpu: &mut Cpu, id: u32, vtable: usize) -> u32 {
    let Some(object) = state(cpu).object(id) else {
        return 0;
    };
    if let Some(i) = object.ifaces.iter().find(|i| i.vtable == vtable as u32) {
        return i.address;
    }
    let Some(address) = com::new_interface(cpu, dll_index(), vtable, id, 0) else {
        return 0;
    };
    if let Some(object) = state(cpu).object_mut(id) {
        object.ifaces.push(Iface {
            vtable: vtable as u32,
            address,
            refs: 0,
        });
    }
    address
}

/// Give the program the interface `vtable` of the object `id` at `out`,
/// with a reference: DD_OK, or the error.
pub fn give(cpu: &mut Cpu, id: u32, vtable: usize, out: u32) -> u32 {
    let address = iface(cpu, id, vtable);
    if address == 0 {
        return com::E_OUTOFMEMORY;
    }
    if !com::write_out(cpu, out, address) {
        return DDERR_INVALIDPARAMS;
    }
    add_ref(cpu, address);
    DD_OK
}

/// The object `this` is an interface of: its vtable's place and its id.
pub fn this(cpu: &mut Cpu, this: u32) -> Option<(usize, u32)> {
    let (vtable, id) = com::interface_of(cpu, dll_index(), this)?;
    state(cpu).object(id)?;
    Some((vtable, id))
}

/// Add a reference on the interface at `address`: its new count.
pub fn add_ref(cpu: &mut Cpu, address: u32) -> u32 {
    let Some((_, id)) = this(cpu, address) else {
        return 0;
    };
    let Some(object) = state(cpu).object_mut(id) else {
        return 0;
    };
    match object.ifaces.iter_mut().find(|i| i.address == address) {
        Some(i) => {
            i.refs += 1;
            i.refs
        }
        None => 0,
    }
}

/// Take a reference off the interface at `address`: its new count. The
/// object goes when none of its interfaces has one left.
pub fn release(cpu: &mut Cpu, address: u32) -> u32 {
    let Some((_, id)) = this(cpu, address) else {
        return 0;
    };
    let Some(object) = state(cpu).object_mut(id) else {
        return 0;
    };
    let Some(i) = object.ifaces.iter_mut().find(|i| i.address == address) else {
        return 0;
    };
    i.refs = i.refs.saturating_sub(1);
    let refs = i.refs;
    if object.refs() == 0 {
        unreferenced(cpu, id);
    }
    refs
}

/// The object `id` has no references left: it goes, unless something
/// else keeps it (a surface of a complex surface goes with its root).
fn unreferenced(cpu: &mut Cpu, id: u32) {
    let Some(object) = state(cpu).object(id) else {
        return;
    };
    if object.surface.as_ref().is_some_and(|s| s.root != 0) {
        return;
    }
    destroy(cpu, id);
}

/// The object `id` goes, and what it holds is let go.
pub fn destroy(cpu: &mut Cpu, id: u32) {
    let Some(object) = state(cpu).object(id).cloned() else {
        return;
    };
    if object.ddraw.is_some() {
        directdraw::destroyed(cpu, id);
    }
    if object.surface.is_some() {
        surface::destroyed(cpu, id);
    }
    if object.palette.is_some() {
        palette::destroyed(cpu, id);
    }
    for iface in &object.ifaces {
        com::free_interface(cpu, iface.address);
    }
    if let Some(slot) = state(cpu).objects.get_mut(id as usize - 1) {
        *slot = None;
    }
    if object.holds_creator && object.creator != 0 {
        release(cpu, object.creator);
    }
}

/// A process ended: its objects go, and the display it had.
pub fn process_ended(cpu: &mut Cpu, pid: u32, _tids: &[u32]) {
    display::process_ended(cpu, pid);
    let s = state(cpu);
    for slot in s.objects.iter_mut() {
        if slot.as_ref().is_some_and(|o| o.pid == pid) {
            *slot = None;
        }
    }
}

fn dll_can_unload_now(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(com::S_FALSE)
}

fn dll_get_class_object(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(super::ole32::dll_get_class_object(cpu, DLL.name, args))
}

// IUnknown, which all the interfaces share.

/// The vtable an interface ID asks for of an object of the kind of
/// `vtable`'s.
fn vtable_for(vtable: usize, iid: &Guid) -> Option<usize> {
    let ddraw = vtable <= V_DD7;
    let surface = (V_SURFACE1..=V_SURFACE7).contains(&vtable);
    let pairs: [(Guid, usize, bool); 11] = [
        (IID_DD1, V_DD1, ddraw),
        (IID_DD2, V_DD2, ddraw),
        (IID_DD4, V_DD4, ddraw),
        (IID_DD7, V_DD7, ddraw),
        (IID_SURFACE1, V_SURFACE1, surface),
        (IID_SURFACE2, V_SURFACE2, surface),
        (IID_SURFACE3, V_SURFACE3, surface),
        (IID_SURFACE4, V_SURFACE4, surface),
        (IID_SURFACE7, V_SURFACE7, surface),
        (IID_PALETTE, V_PALETTE, vtable == V_PALETTE),
        (IID_CLIPPER, V_CLIPPER, vtable == V_CLIPPER),
    ];
    if *iid == IID_IUNKNOWN {
        return Some(vtable);
    }
    pairs
        .iter()
        .find(|(g, _, kind)| *kind && g == iid)
        .map(|&(_, v, _)| v)
}

/// QueryInterface(this, iid, out).
fn query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this_, iid, out] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let Some((vtable, id)) = this(cpu, this_) else {
        return Some(DDERR_INVALIDOBJECT);
    };
    let Some(iid) = Guid::read(cpu, iid) else {
        return Some(DDERR_INVALIDPARAMS);
    };
    if out == 0 {
        return Some(DDERR_INVALIDPARAMS);
    }
    match vtable_for(vtable, &iid) {
        Some(v) if v == vtable => {
            add_ref(cpu, this_);
            com::write_out(cpu, out, this_);
            Some(S_OK)
        }
        Some(v) => Some(give(cpu, id, v, out)),
        None => {
            com::write_out(cpu, out, 0);
            Some(E_NOINTERFACE)
        }
    }
}

fn add_ref_method(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(add_ref(cpu, args[0]))
}

fn release_method(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(release(cpu, args[0]))
}

crate::state_fields!(Format {
    flags,
    fourcc,
    bits,
    r,
    g,
    b,
    a
});
crate::state_fields!(Iface {
    vtable,
    address,
    refs
});
crate::state_fields!(Object {
    pid,
    ifaces,
    owner,
    creator,
    holds_creator,
    ddraw,
    surface,
    palette,
    clipper
});
crate::state_fields!(State {
    objects,
    display,
    uniqueness
});
