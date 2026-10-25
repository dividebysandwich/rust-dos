//! GDI: drawing, on GDI32's. The handles are the same in both; what
//! differs are the structures (LOGFONT, LOGPEN, TEXTMETRIC and BITMAP have
//! 16-bit integers in Windows 3.1), the arrays of points, and the
//! functions that return a point in DX:AX, which GDI32 has as the *Ex
//! functions that write it.

use super::builtin::{Api16, Def, rust, win32};
use super::thunk::{self, KIND_PLAIN};
use super::user::{P_GDI, P_POINTS, P_WORDS};
use crate::cpu::Cpu;
use crate::dpmi::Context;

pub static MODULE: Def = Def {
    name: "GDI",
    file: "GDI.EXE",
    apis: APIS,
    library: false,
};

const G: &str = "GDI32.DLL";

/// The functions. Append only: a function's place is its id's.
static APIS: &[Api16] = &[
    win32("SetBkColor", 1, "wl", G, "SetBkColor", b'w'),
    win32("SetBkMode", 2, "ww", G, "SetBkMode", b'w'),
    win32("SetMapMode", 3, "ww", G, "SetMapMode", b'w'),
    win32("SetROP2", 4, "ww", G, "SetROP2", b'w'),
    rust("SetRelAbs", 5, "ww", one),
    win32("SetPolyFillMode", 6, "ww", G, "SetPolyFillMode", b'w'),
    win32("SetStretchBltMode", 7, "ww", G, "SetStretchBltMode", b'w'),
    rust("SetTextCharacterExtra", 8, "ws", zero),
    win32("SetTextColor", 9, "wl", G, "SetTextColor", b'w'),
    win32(
        "SetTextJustification",
        10,
        "wss",
        G,
        "SetTextJustification",
        b'w',
    ),
    rust("SetWindowOrg", 11, "wss", set_window_org),
    rust("SetWindowExt", 12, "wss", set_window_ext),
    rust("SetViewportOrg", 13, "wss", set_viewport_org),
    rust("SetViewportExt", 14, "wss", set_viewport_ext),
    rust("OffsetWindowOrg", 15, "wss", offset_window_org),
    rust("ScaleWindowExt", 16, "wssss", scale_window_ext),
    rust("OffsetViewportOrg", 17, "wss", offset_viewport_org),
    rust("ScaleViewportExt", 18, "wssss", scale_viewport_ext),
    win32("LineTo", 19, "wss", G, "LineTo", b'b'),
    rust("MoveTo", 20, "wss", move_to),
    win32("ExcludeClipRect", 21, "wssss", G, "ExcludeClipRect", b'w'),
    win32(
        "IntersectClipRect",
        22,
        "wssss",
        G,
        "IntersectClipRect",
        b'w',
    ),
    win32("Arc", 23, "wssssssss", G, "Arc", b'b'),
    win32("Ellipse", 24, "wssss", G, "Ellipse", b'b'),
    win32("FloodFill", 25, "wssl", G, "FloodFill", b'b'),
    win32("Pie", 26, "wssssssss", G, "Pie", b'b'),
    win32("Rectangle", 27, "wssss", G, "Rectangle", b'b'),
    win32("RoundRect", 28, "wssssss", G, "RoundRect", b'b'),
    win32("PatBlt", 29, "wssssl", G, "PatBlt", b'b'),
    win32("SaveDC", 30, "w", G, "SaveDC", b'w'),
    win32("SetPixel", 31, "wssl", G, "SetPixel", b'w'),
    win32("OffsetClipRgn", 32, "wss", G, "OffsetClipRgn", b'w'),
    win32("TextOut", 33, "wsspw", G, "TextOutA", b'b'),
    win32("BitBlt", 34, "wsssswssl", G, "BitBlt", b'b'),
    win32("StretchBlt", 35, "wsssswssssl", G, "StretchBlt", b'b'),
    rust("Polygon", 36, "wpw", polygon),
    rust("Polyline", 37, "wpw", polyline),
    rust("Escape", 38, "wwwpp", escape),
    win32("RestoreDC", 39, "ws", G, "RestoreDC", b'b'),
    win32("FillRgn", 40, "www", G, "FillRgn", b'b'),
    win32("FrameRgn", 41, "wwwss", G, "FrameRgn", b'b'),
    win32("InvertRgn", 42, "ww", G, "InvertRgn", b'b'),
    win32("PaintRgn", 43, "ww", G, "PaintRgn", b'b'),
    win32("SelectClipRgn", 44, "ww", G, "SelectClipRgn", b'w'),
    win32("SelectObject", 45, "ww", G, "SelectObject", b'w'),
    win32("CombineRgn", 47, "wwws", G, "CombineRgn", b'w'),
    win32("CreateBitmap", 48, "wwwwp", G, "CreateBitmap", b'w'),
    rust("CreateBitmapIndirect", 49, "p", create_bitmap_indirect),
    rust("CreateBrushIndirect", 50, "p", create_brush_indirect),
    win32(
        "CreateCompatibleBitmap",
        51,
        "www",
        G,
        "CreateCompatibleBitmap",
        b'w',
    ),
    win32("CreateCompatibleDC", 52, "w", G, "CreateCompatibleDC", b'w'),
    rust("CreateDC", 53, "pppp", create_dc),
    win32(
        "CreateEllipticRgn",
        54,
        "ssss",
        G,
        "CreateEllipticRgn",
        b'w',
    ),
    win32(
        "CreateEllipticRgnIndirect",
        55,
        "R",
        G,
        "CreateEllipticRgnIndirect",
        b'w',
    ),
    win32("CreateFont", 56, "ssssswwwwwwwwp", G, "CreateFontA", b'w'),
    rust("CreateFontIndirect", 57, "p", create_font_indirect),
    win32("CreateHatchBrush", 58, "wl", G, "CreateHatchBrush", b'w'),
    win32("CreatePatternBrush", 60, "w", G, "CreatePatternBrush", b'w'),
    win32("CreatePen", 61, "ssl", G, "CreatePen", b'w'),
    rust("CreatePenIndirect", 62, "p", create_pen_indirect),
    rust("CreatePolygonRgn", 63, "pws", create_polygon_rgn),
    win32("CreateRectRgn", 64, "ssss", G, "CreateRectRgn", b'w'),
    win32(
        "CreateRectRgnIndirect",
        65,
        "R",
        G,
        "CreateRectRgnIndirect",
        b'w',
    ),
    win32("CreateSolidBrush", 66, "l", G, "CreateSolidBrush", b'w'),
    rust("DPtoLP", 67, "wps", dp_to_lp),
    win32("DeleteDC", 68, "w", G, "DeleteDC", b'b'),
    win32("DeleteObject", 69, "w", G, "DeleteObject", b'b'),
    rust("EnumFonts", 70, "wpSl", enum_fonts),
    rust("EnumObjects", 71, "wwSl", enum_objects),
    win32("EqualRgn", 72, "ww", G, "EqualRgn", b'b'),
    win32("GetBitmapBits", 74, "wlp", G, "GetBitmapBits", b'w'),
    win32("GetBkColor", 75, "w", G, "GetBkColor", b'w'),
    win32("GetBkMode", 76, "w", G, "GetBkMode", b'w'),
    win32("GetClipBox", 77, "wR", G, "GetClipBox", b'w'),
    rust("GetCurrentPosition", 78, "w", get_current_position),
    rust("GetDCOrg", 79, "w", get_dc_org),
    win32("GetDeviceCaps", 80, "ws", G, "GetDeviceCaps", b'w'),
    win32("GetMapMode", 81, "w", G, "GetMapMode", b'w'),
    rust("GetObject", 82, "wsp", get_object),
    win32("GetPixel", 83, "wss", G, "GetPixel", b'w'),
    win32("GetPolyFillMode", 84, "w", G, "GetPolyFillMode", b'w'),
    win32("GetROP2", 85, "w", G, "GetROP2", b'w'),
    rust("GetRelAbs", 86, "w", one),
    win32("GetStockObject", 87, "w", G, "GetStockObject", b'w'),
    win32("GetStretchBltMode", 88, "w", G, "GetStretchBltMode", b'w'),
    rust("GetTextCharacterExtra", 89, "w", zero),
    win32("GetTextColor", 90, "w", G, "GetTextColor", b'w'),
    rust("GetTextExtent", 91, "wps", get_text_extent),
    win32("GetTextFace", 92, "wsp", G, "GetTextFaceA", b'w'),
    rust("GetTextMetrics", 93, "wp", get_text_metrics),
    rust("GetViewportExt", 94, "w", get_viewport_ext),
    rust("GetViewportOrg", 95, "w", get_viewport_org),
    rust("GetWindowExt", 96, "w", get_window_ext),
    rust("GetWindowOrg", 97, "w", get_window_org),
    rust("LPtoDP", 99, "wps", lp_to_dp),
    rust("LineDDA", 100, "ssssSl", line_dda),
    win32("OffsetRgn", 101, "wss", G, "OffsetRgn", b'w'),
    win32("PtVisible", 103, "wss", G, "PtVisible", b'b'),
    win32("RectVisibleOld", 104, "wR", G, "RectVisible", b'b'),
    win32("SetBitmapBits", 106, "wlp", G, "SetBitmapBits", b'w'),
    win32("AddFontResource", 119, "p", G, "AddFontResourceA", b'w'),
    rust("Death", 121, "w", zero),
    rust("Resurrection", 122, "wwwwwww", zero),
    win32("PlayMetaFile", 123, "ww", G, "PlayMetaFile", b'b'),
    rust("GetMetaFile", 124, "p", zero),
    rust("CreateMetaFile", 125, "p", zero),
    rust("CloseMetaFile", 126, "w", zero),
    win32("DeleteMetaFile", 127, "w", G, "DeleteMetaFile", b'b'),
    rust("MulDiv", 128, "sss", mul_div),
    rust("SetEnvironment", 132, "ppw", zero),
    rust("GetEnvironment", 133, "ppw", zero),
    win32("GetRgnBox", 134, "wR", G, "GetRgnBox", b'w'),
    win32(
        "RemoveFontResource",
        136,
        "p",
        G,
        "RemoveFontResourceA",
        b'b',
    ),
    rust("SetBrushOrg", 148, "wss", set_brush_org),
    rust("GetBrushOrg", 149, "w", get_brush_org),
    win32("UnrealizeObject", 150, "w", G, "UnrealizeObject", b'b'),
    rust("CreateIC", 153, "pppp", create_ic),
    win32("GetNearestColor", 154, "wl", G, "GetNearestColor", b'w'),
    rust("QueryAbort", 155, "ww", one),
    win32(
        "CreateDiscardableBitmap",
        156,
        "www",
        G,
        "CreateDiscardableBitmap",
        b'w',
    ),
    win32("PtInRegion", 161, "wss", G, "PtInRegion", b'b'),
    rust("GetBitmapDimension", 162, "w", zero),
    rust("SetBitmapDimension", 163, "wss", zero),
    win32("SetRectRgn", 172, "wssss", G, "SetRectRgn", b'b'),
    win32("RectInRegionOld", 181, "wR", G, "RectInRegion", b'b'),
    win32("SetBoundsRect", 193, "wRw", G, "SetBoundsRect", b'w'),
    win32("GetBoundsRect", 194, "wRw", G, "GetBoundsRect", b'w'),
    win32("GetFontData", 311, "wllpl", G, "GetFontData", b'w'),
    win32("GetRasterizerCaps", 313, "pw", G, "GetRasterizerCaps", b'b'),
    rust("EnumFontFamilies", 330, "wpSl", enum_fonts),
    win32("GetTextAlign", 345, "w", G, "GetTextAlign", b'w'),
    win32("SetTextAlign", 346, "ww", G, "SetTextAlign", b'w'),
    win32("Chord", 348, "wssssssss", G, "Chord", b'b'),
    win32("SetMapperFlags", 349, "wl", G, "SetMapperFlags", b'w'),
    rust("GetCharWidth", 350, "wwwp", get_char_width),
    rust("ExtTextOut", 351, "wsswpSwp", ext_text_out),
    win32("CreatePalette", 360, "p", G, "CreatePalette", b'w'),
    win32("GDISelectPalette", 361, "www", G, "SelectPalette", b'w'),
    win32("GDIRealizePalette", 362, "w", G, "RealizePalette", b'w'),
    win32(
        "GetPaletteEntries",
        363,
        "wwwp",
        G,
        "GetPaletteEntries",
        b'w',
    ),
    win32(
        "SetPaletteEntries",
        364,
        "wwwp",
        G,
        "SetPaletteEntries",
        b'w',
    ),
    rust("RealizeDefaultPalette", 365, "w", zero),
    win32("UpdateColors", 366, "w", G, "UpdateColors", b'b'),
    win32("AnimatePalette", 367, "wwwp", G, "AnimatePalette", b'b'),
    win32("ResizePalette", 368, "ww", G, "ResizePalette", b'b'),
    win32(
        "GetNearestPaletteIndex",
        370,
        "wl",
        G,
        "GetNearestPaletteIndex",
        b'w',
    ),
    win32("ExtFloodFill", 372, "wsslw", G, "ExtFloodFill", b'b'),
    win32(
        "SetSystemPaletteUse",
        373,
        "ww",
        G,
        "SetSystemPaletteUse",
        b'w',
    ),
    win32(
        "GetSystemPaletteUse",
        374,
        "w",
        G,
        "GetSystemPaletteUse",
        b'w',
    ),
    win32(
        "GetSystemPaletteEntries",
        375,
        "wwwp",
        G,
        "GetSystemPaletteEntries",
        b'w',
    ),
    rust("ResetDC", 376, "wp", reset_dc),
    rust("StartDoc", 377, "wp", minus_one),
    win32("EndDoc", 378, "w", G, "EndDoc", b'w'),
    win32("StartPage", 379, "w", G, "StartPage", b'w'),
    win32("EndPage", 380, "w", G, "EndPage", b'w'),
    rust("SetAbortProc", 381, "wS", one),
    win32("AbortDoc", 382, "w", G, "AbortDoc", b'w'),
    rust("GetCurLogFont", 411, "w", get_cur_log_font),
    win32(
        "StretchDIBits",
        439,
        "wssssssssppwl",
        G,
        "StretchDIBits",
        b'w',
    ),
    win32("SetDIBits", 440, "wwwwppw", G, "SetDIBits", b'w'),
    win32("GetDIBits", 441, "wwwwppw", G, "GetDIBits", b'w'),
    win32("CreateDIBitmap", 442, "wplppw", G, "CreateDIBitmap", b'w'),
    win32(
        "SetDIBitsToDevice",
        443,
        "wsssssswwppw",
        G,
        "SetDIBitsToDevice",
        b'w',
    ),
    win32(
        "CreateRoundRectRgn",
        444,
        "ssssss",
        G,
        "CreateRoundRectRgn",
        b'w',
    ),
    rust("CreateDIBPatternBrush", 445, "ww", create_dib_pattern_brush),
    rust("PolyPolygon", 450, "wppw", poly_polygon),
    rust("CreatePolyPolygonRgn", 451, "ppws", create_poly_polygon_rgn),
    rust("SetObjectOwner", 461, "ww", zero),
    win32("IsGDIObject", 462, "w", G, "GetObjectType", b'w'),
    rust("MakeObjectPrivate", 463, "ww", zero),
    win32("RectVisible", 465, "wR", G, "RectVisible", b'b'),
    win32("RectInRegion", 466, "wR", G, "RectInRegion", b'b'),
    rust("GetBitmapDimensionEx", 468, "wP", zero),
    win32("GetBrushOrgEx", 469, "wP", G, "GetBrushOrgEx", b'b'),
    win32(
        "GetCurrentPositionEx",
        470,
        "wP",
        G,
        "GetCurrentPositionEx",
        b'b',
    ),
    win32(
        "GetTextExtentPoint",
        471,
        "wpsP",
        G,
        "GetTextExtentPointA",
        b'b',
    ),
    win32("GetViewportExtEx", 472, "wP", G, "GetViewportExtEx", b'b'),
    win32("GetViewportOrgEx", 473, "wP", G, "GetViewportOrgEx", b'b'),
    win32("GetWindowExtEx", 474, "wP", G, "GetWindowExtEx", b'b'),
    win32("GetWindowOrgEx", 475, "wP", G, "GetWindowOrgEx", b'b'),
    win32(
        "OffsetViewportOrgEx",
        476,
        "wssP",
        G,
        "OffsetViewportOrgEx",
        b'b',
    ),
    win32(
        "OffsetWindowOrgEx",
        477,
        "wssP",
        G,
        "OffsetWindowOrgEx",
        b'b',
    ),
    rust("SetBitmapDimensionEx", 478, "wssP", zero),
    win32("SetViewportExtEx", 479, "wssP", G, "SetViewportExtEx", b'b'),
    win32("SetViewportOrgEx", 480, "wssP", G, "SetViewportOrgEx", b'b'),
    win32("SetWindowExtEx", 481, "wssP", G, "SetWindowExtEx", b'b'),
    win32("SetWindowOrgEx", 482, "wssP", G, "SetWindowOrgEx", b'b'),
    win32("MoveToEx", 483, "wssP", G, "MoveToEx", b'b'),
    win32(
        "ScaleViewportExtEx",
        484,
        "wssssP",
        G,
        "ScaleViewportExtEx",
        b'b',
    ),
    win32(
        "ScaleWindowExtEx",
        485,
        "wssssP",
        G,
        "ScaleWindowExtEx",
        b'b',
    ),
    rust("PolyBezier", 502, "wpw", poly_bezier),
    rust("PolyBezierTo", 503, "wpw", poly_bezier_to),
    win32("ExtSelectClipRgn", 508, "www", G, "ExtSelectClipRgn", b'w'),
    win32("AbortPath", 511, "w", G, "AbortPath", b'b'),
    win32("BeginPath", 512, "w", G, "BeginPath", b'b'),
    win32("CloseFigure", 513, "w", G, "CloseFigure", b'b'),
    win32("EndPath", 514, "w", G, "EndPath", b'b'),
    win32("FillPath", 515, "w", G, "FillPath", b'b'),
    win32("FlattenPath", 516, "w", G, "FlattenPath", b'b'),
    win32("PathToRegion", 518, "w", G, "PathToRegion", b'w'),
    win32("SelectClipPath", 519, "ww", G, "SelectClipPath", b'b'),
    win32("StrokeAndFillPath", 520, "w", G, "StrokeAndFillPath", b'b'),
    win32("StrokePath", 521, "w", G, "StrokePath", b'b'),
    win32("WidenPath", 522, "w", G, "WidenPath", b'b'),
    win32("GetArcDirection", 524, "w", G, "GetArcDirection", b'w'),
    win32("SetArcDirection", 525, "ww", G, "SetArcDirection", b'w'),
];

fn zero(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

fn one(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(1)
}

fn minus_one(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0xFFFF)
}

/// The address of GDI32's function `name`.
fn api(name: &str) -> u32 {
    super::builtin::win32_address(G, name).unwrap_or_else(|| panic!("GDI32 has no {}", name))
}

/// Go on to GDI32's `name` with `args`, the 32-bit stack below `top`
/// (scratch space above it from `thunk::scratch32`, or the stack's top),
/// `post` seeing to what it returns with `data` when there is any.
fn go_at(cpu: &mut Cpu, top: u32, name: &str, args: &[u32], data: Vec<u32>) -> Option<u32> {
    let ctx = Context::of(cpu);
    let post = if data.is_empty() {
        thunk::POST_PLAIN
    } else {
        thunk::POST_USER
    };
    thunk::enter32_at(cpu, ctx, top, api(name), args, post, data);
    None
}

fn go(cpu: &mut Cpu, name: &str, args: &[u32]) -> Option<u32> {
    let top = thunk::scratch32(cpu, 0);
    go_at(cpu, top, name, args, Vec::new())
}

/// What `post` does: `data[0]`.
const P_POINT_DWORD: u32 = P_GDI;
const P_OBJECT: u32 = P_GDI + 1;
const P_TEXTMETRIC: u32 = P_GDI + 2;

/// A GDI32 function returned `value`: what 16-bit code gets.
pub fn post(cpu: &mut Cpu, value: u32, data: &[u32]) -> Option<u32> {
    let d = |i: usize| data.get(i).copied().unwrap_or(0);
    match d(0) {
        // The point written at d(1) as DX:AX, or 0 when the function
        // failed.
        P_POINT_DWORD => {
            if value == 0 {
                return Some(0);
            }
            let x = super::read_32(cpu, d(1));
            let y = super::read_32(cpu, d(1) + 4);
            Some(x & 0xFFFF | y << 16)
        }
        P_OBJECT => Some(object16(cpu, value, d(1), d(2), d(3))),
        P_TEXTMETRIC => {
            if value != 0 {
                text_metric16(cpu, d(1), d(2));
            }
            Some((value != 0) as u32)
        }
        _ => Some(value),
    }
}

fn sx(v: u32) -> u32 {
    v as u16 as i16 as i32 as u32
}

fn r16(cpu: &Cpu, at: u32) -> u32 {
    super::read_16(cpu, at) as u32
}

fn r32(cpu: &Cpu, at: u32) -> u32 {
    super::read_32(cpu, at)
}

fn w32(cpu: &mut Cpu, at: u32, value: u32) {
    cpu.bus.guest_write_32(at, value);
}

// The functions that return a point.

/// Call GDI32's `name` with `args` and a POINT it writes, which 16-bit
/// code gets in DX:AX.
fn point_dword(cpu: &mut Cpu, name: &str, args: &[u32]) -> Option<u32> {
    let pt = thunk::scratch32(cpu, 8);
    w32(cpu, pt, 0);
    w32(cpu, pt + 4, 0);
    let mut all = args.to_vec();
    all.push(pt);
    go_at(cpu, pt, name, &all, vec![P_POINT_DWORD, pt])
}

fn move_to(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    point_dword(cpu, "MoveToEx", args)
}

fn set_window_org(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    point_dword(cpu, "SetWindowOrgEx", args)
}

fn set_window_ext(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    point_dword(cpu, "SetWindowExtEx", args)
}

fn set_viewport_org(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    point_dword(cpu, "SetViewportOrgEx", args)
}

fn set_viewport_ext(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    point_dword(cpu, "SetViewportExtEx", args)
}

fn offset_window_org(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    point_dword(cpu, "OffsetWindowOrgEx", args)
}

fn offset_viewport_org(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    point_dword(cpu, "OffsetViewportOrgEx", args)
}

fn scale_window_ext(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    point_dword(cpu, "ScaleWindowExtEx", args)
}

fn scale_viewport_ext(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    point_dword(cpu, "ScaleViewportExtEx", args)
}

fn get_current_position(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    point_dword(cpu, "GetCurrentPositionEx", args)
}

fn get_dc_org(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    point_dword(cpu, "GetDCOrgEx", args)
}

fn get_viewport_ext(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    point_dword(cpu, "GetViewportExtEx", args)
}

fn get_viewport_org(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    point_dword(cpu, "GetViewportOrgEx", args)
}

fn get_window_ext(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    point_dword(cpu, "GetWindowExtEx", args)
}

fn get_window_org(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    point_dword(cpu, "GetWindowOrgEx", args)
}

fn set_brush_org(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    point_dword(cpu, "SetBrushOrgEx", args)
}

fn get_brush_org(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    point_dword(cpu, "GetBrushOrgEx", args)
}

fn get_text_extent(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    point_dword(cpu, "GetTextExtentPointA", args)
}

// Arrays of points.

/// The most points a call converts.
const MAX_POINTS: u32 = 0x4000;

/// Room for `n` POINTs and `extra` bytes after them on the 32-bit stack,
/// the `n` POINT16s at `at16` copied to it: its address.
fn points32(cpu: &mut Cpu, at16: u32, n: u32, extra: u32) -> u32 {
    let n = n.min(MAX_POINTS);
    let at = thunk::scratch32(cpu, 8 * n + extra);
    for i in 0..2 * n {
        let v = sx(r16(cpu, at16 + 2 * i));
        w32(cpu, at + 4 * i, v);
    }
    at
}

fn points_call(cpu: &mut Cpu, name: &str, args: &[u32]) -> Option<u32> {
    let n = args[2].min(MAX_POINTS);
    let pts = points32(cpu, args[1], n, 0);
    go_at(cpu, pts, name, &[args[0], pts, n], Vec::new())
}

fn polygon(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    points_call(cpu, "Polygon", args)
}

fn polyline(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    points_call(cpu, "Polyline", args)
}

fn poly_bezier(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    points_call(cpu, "PolyBezier", args)
}

fn poly_bezier_to(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    points_call(cpu, "PolyBezierTo", args)
}

/// LPtoDP and DPtoLP: the points converted in place.
fn points_in_place(cpu: &mut Cpu, name: &str, args: &[u32]) -> Option<u32> {
    let n = (args[2] as i32).clamp(0, MAX_POINTS as i32) as u32;
    let pts = points32(cpu, args[1], n, 0);
    go_at(
        cpu,
        pts,
        name,
        &[args[0], pts, n],
        vec![P_POINTS, pts, args[1], n],
    )
}

fn lp_to_dp(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    points_in_place(cpu, "LPtoDP", args)
}

fn dp_to_lp(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    points_in_place(cpu, "DPtoLP", args)
}

fn create_polygon_rgn(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let n = args[1].min(MAX_POINTS);
    let pts = points32(cpu, args[0], n, 0);
    go_at(cpu, pts, "CreatePolygonRgn", &[pts, n, args[2]], Vec::new())
}

/// The counts of PolyPolygon's polygons, 16-bit ones at `at16`, `n` of
/// them: as 32-bit ones, and how many points they come to.
fn counts(cpu: &Cpu, at16: u32, n: u32) -> (Vec<u32>, u32) {
    let counts: Vec<u32> = (0..n.min(0x400)).map(|i| r16(cpu, at16 + 2 * i)).collect();
    let total = counts.iter().sum::<u32>().min(MAX_POINTS);
    (counts, total)
}

/// The points and the counts of a PolyPolygon on the 32-bit stack: the
/// points' address, and the counts'.
fn poly_polygon32(cpu: &mut Cpu, pts16: u32, counts16: u32, n: u32) -> (u32, u32, u32) {
    let (counts, total) = counts(cpu, counts16, n);
    let pts = points32(cpu, pts16, total, 4 * counts.len() as u32);
    let at = pts + 8 * total;
    for (i, c) in counts.iter().enumerate() {
        w32(cpu, at + 4 * i as u32, *c);
    }
    (pts, at, counts.len() as u32)
}

fn poly_polygon(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (pts, counts, n) = poly_polygon32(cpu, args[1], args[2], args[3]);
    go_at(
        cpu,
        pts,
        "PolyPolygon",
        &[args[0], pts, counts, n],
        Vec::new(),
    )
}

fn create_poly_polygon_rgn(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (pts, counts, n) = poly_polygon32(cpu, args[0], args[1], args[2]);
    go_at(
        cpu,
        pts,
        "CreatePolyPolygonRgn",
        &[pts, counts, n, args[3]],
        Vec::new(),
    )
}

// Structures.

fn create_bitmap_indirect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    // BITMAP16: type, width, height, width in bytes, planes (a byte),
    // bits per pixel (a byte), the bits (16:16).
    let b = args[0];
    let width = r16(cpu, b + 2);
    let height = r16(cpu, b + 4);
    let planes = super::peek(cpu, b + 8, 2).unwrap_or(vec![1, 1]);
    let bits = super::linear(cpu, r32(cpu, b + 10));
    go(
        cpu,
        "CreateBitmap",
        &[width, height, planes[0] as u32, planes[1] as u32, bits],
    )
}

fn create_brush_indirect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    // LOGBRUSH16: style, color (a long), hatch.
    let b = args[0];
    let lb = thunk::scratch32(cpu, 12);
    w32(cpu, lb, r16(cpu, b));
    w32(cpu, lb + 4, r32(cpu, b + 2));
    w32(cpu, lb + 8, sx(r16(cpu, b + 6)));
    go_at(cpu, lb, "CreateBrushIndirect", &[lb], Vec::new())
}

fn create_pen_indirect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    // LOGPEN16: style, width (a POINT16), color.
    let p = args[0];
    let lp = thunk::scratch32(cpu, 16);
    w32(cpu, lp, r16(cpu, p));
    w32(cpu, lp + 4, sx(r16(cpu, p + 2)));
    w32(cpu, lp + 8, sx(r16(cpu, p + 4)));
    w32(cpu, lp + 12, r32(cpu, p + 6));
    go_at(cpu, lp, "CreatePenIndirect", &[lp], Vec::new())
}

/// A LOGFONT16 at `lf16` as a LOGFONTA at `lf`.
fn log_font32(cpu: &mut Cpu, lf16: u32, lf: u32) {
    for i in 0..5 {
        let v = sx(r16(cpu, lf16 + 2 * i));
        w32(cpu, lf + 4 * i, v);
    }
    let rest = super::peek(cpu, lf16 + 10, 40).unwrap_or(vec![0; 40]);
    super::poke(cpu, lf + 20, &rest);
}

/// A LOGFONTA at `lf` as a LOGFONT16 at `lf16`, `max` bytes of it.
fn log_font16(cpu: &mut Cpu, lf: u32, lf16: u32, max: u32) -> u32 {
    let mut b = Vec::new();
    for i in 0..5 {
        b.extend_from_slice(&(r32(cpu, lf + 4 * i) as u16).to_le_bytes());
    }
    b.extend(super::peek(cpu, lf + 20, 40).unwrap_or(vec![0; 40]));
    write_max(cpu, lf16, &b, max)
}

/// Write `bytes` at `at`, at most `max` of them: how many.
fn write_max(cpu: &mut Cpu, at: u32, bytes: &[u8], max: u32) -> u32 {
    let n = bytes.len().min(max as usize);
    super::poke(cpu, at, &bytes[..n]);
    n as u32
}

fn create_font_indirect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let lf = thunk::scratch32(cpu, 60);
    log_font32(cpu, args[0], lf);
    go_at(cpu, lf, "CreateFontIndirectA", &[lf], Vec::new())
}

/// GetObject: GDI32's into 128 bytes, then the structure it wrote (told
/// by its size) as the 16-bit one.
fn get_object(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let buf = thunk::scratch32(cpu, 128);
    go_at(
        cpu,
        buf,
        "GetObjectA",
        &[args[0], 128, buf],
        vec![P_OBJECT, buf, args[2], args[1].min(0x1000)],
    )
}

/// GDI32's GetObject wrote `size` bytes at `buf`: as the 16-bit structure
/// at `to` (null for its size), `max` bytes of it at most. How many.
fn object16(cpu: &mut Cpu, size: u32, buf: u32, to: u32, max: u32) -> u32 {
    let mut b = Vec::new();
    match size {
        // LOGPEN.
        16 => {
            b.extend_from_slice(&(r32(cpu, buf) as u16).to_le_bytes());
            b.extend_from_slice(&(r32(cpu, buf + 4) as u16).to_le_bytes());
            b.extend_from_slice(&(r32(cpu, buf + 8) as u16).to_le_bytes());
            b.extend_from_slice(&r32(cpu, buf + 12).to_le_bytes());
        }
        // LOGBRUSH.
        12 => {
            b.extend_from_slice(&(r32(cpu, buf) as u16).to_le_bytes());
            b.extend_from_slice(&r32(cpu, buf + 4).to_le_bytes());
            b.extend_from_slice(&(r32(cpu, buf + 8) as u16).to_le_bytes());
        }
        // BITMAP: the bits' pointer isn't one 16-bit code can use.
        24 => {
            for i in 0..4 {
                b.extend_from_slice(&(r32(cpu, buf + 4 * i) as u16).to_le_bytes());
            }
            let planes = super::peek(cpu, buf + 16, 4).unwrap_or(vec![0; 4]);
            b.extend_from_slice(&[planes[0], planes[2]]);
            b.extend_from_slice(&0u32.to_le_bytes());
        }
        // LOGFONTA.
        60 => {
            if to == 0 {
                return 50;
            }
            return log_font16(cpu, buf, to, max);
        }
        // A palette's count of entries, and the rest as they are.
        _ => {
            b = super::peek(cpu, buf, size.min(128) as usize).unwrap_or_default();
        }
    }
    if to == 0 {
        return b.len() as u32;
    }
    write_max(cpu, to, &b, max)
}

fn get_text_metrics(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let tm = thunk::scratch32(cpu, 56);
    go_at(
        cpu,
        tm,
        "GetTextMetricsA",
        &[args[0], tm],
        vec![P_TEXTMETRIC, tm, args[1]],
    )
}

/// A TEXTMETRICA at `tm` as a TEXTMETRIC16 at `tm16`: its eleven
/// integers as words, the bytes after them as they are.
fn text_metric16(cpu: &mut Cpu, tm: u32, tm16: u32) {
    let mut b = Vec::new();
    for i in 0..11 {
        b.extend_from_slice(&(r32(cpu, tm + 4 * i) as u16).to_le_bytes());
    }
    b.extend(super::peek(cpu, tm + 44, 9).unwrap_or(vec![0; 9]));
    super::poke(cpu, tm16, &b);
}

fn get_char_width(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let n = args[2].saturating_sub(args[1]).saturating_add(1).min(0x100);
    let buf = thunk::scratch32(cpu, 4 * n);
    go_at(
        cpu,
        buf,
        "GetCharWidthA",
        &[args[0], args[1], args[1] + n - 1, buf],
        vec![P_WORDS, buf, args[3], n],
    )
}

/// ExtTextOut(hdc, x, y, options, RECT16 *, string, count, INT16 *dx):
/// the rectangle and the widths widened.
fn ext_text_out(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let count = args[6].min(0x1000);
    let has_dx = args[7] != 0;
    let at = thunk::scratch32(cpu, 16 + if has_dx { 4 * count } else { 0 });
    let rect = if args[4] != 0 {
        for i in 0..4 {
            let v = sx(r16(cpu, args[4] + 2 * i));
            w32(cpu, at + 4 * i, v);
        }
        at
    } else {
        0
    };
    let dx = if has_dx {
        for i in 0..count {
            let v = sx(r16(cpu, args[7] + 2 * i));
            w32(cpu, at + 16 + 4 * i, v);
        }
        at + 16
    } else {
        0
    };
    let string = super::linear(cpu, args[5]);
    go_at(
        cpu,
        at,
        "ExtTextOutA",
        &[args[0], args[1], args[2], args[3], rect, string, count, dx],
        Vec::new(),
    )
}

/// CreateDC and CreateIC: the DEVMODE (whose 16-bit one is shorter) left
/// out.
fn create_dc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    go(cpu, "CreateDCA", &[args[0], args[1], args[2], 0])
}

fn create_ic(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    go(cpu, "CreateICA", &[args[0], args[1], args[2], 0])
}

fn reset_dc(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(args[0])
}

fn get_cur_log_font(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    // OBJ_FONT.
    go(cpu, "GetCurrentObject", &[args[0], 6])
}

/// CreateDIBPatternBrush(global block, usage): the packed DIB in the
/// block, which GDI32's function takes as it is.
fn create_dib_pattern_brush(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let sel = super::global::handle_to_sel(args[0] as u16);
    let at = super::lin(cpu, sel, 0);
    if at == 0 {
        return Some(0);
    }
    // The 32-bit function locks an HGLOBAL; a flat pointer is one that
    // locks to itself.
    go(cpu, "CreateDIBPatternBrush", &[at, args[1]])
}

/// Escape: the queries Windows 3.1 programs ask (QUERYESCSUPPORT and the
/// like) to GDI32's, which answers for the display.
fn escape(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    go(
        cpu,
        "Escape",
        &[args[0], args[1], args[2], args[3], args[4]],
    )
}

fn mul_div(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (a, b, c) = (
        args[0] as i32 as i64,
        args[1] as i32 as i64,
        args[2] as i32 as i64,
    );
    if c == 0 {
        return Some(0x8000);
    }
    // Rounded as Windows rounds, half away from zero.
    let p = a * b;
    let q = if (p < 0) != (c < 0) {
        (p - c / 2) / c
    } else {
        (p + c / 2) / c
    };
    if !(-32768..=32767).contains(&q) {
        return Some(0x8000);
    }
    Some(q as u16 as u32)
}

/// LineDDA's callback: (x, y, lParam).
const LINE_DDA_PROC: u32 = 3 | 0b100 << 8;

fn line_dda(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let proc = thunk::proc32(cpu, args[4], KIND_PLAIN, LINE_DDA_PROC);
    go(
        cpu,
        "LineDDA",
        &[args[0], args[1], args[2], args[3], proc, args[5]],
    )
}

/// EnumObjects' callback: (LOGPEN or LOGBRUSH, lParam). Windows 3.1
/// programs ask it for the pens and brushes there are to choose from;
/// none are listed.
fn enum_objects(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    cpu.bus.log_string("[W16] EnumObjects lists no objects");
    Some(0)
}

/// EnumFonts and EnumFontFamilies: the callback gets 16-bit LOGFONTs and
/// TEXTMETRICs, which there is no conversion of yet; no fonts are
/// listed.
fn enum_fonts(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    cpu.bus.log_string("[W16] EnumFonts lists no fonts");
    Some(1)
}
