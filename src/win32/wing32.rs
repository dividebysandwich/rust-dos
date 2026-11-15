//! WING32.DLL: WinG, Microsoft's library for fast drawing from 8-bit
//! DIBs on Windows 3.1, Windows 95 and NT 3.5, which games of 1994 and
//! 1995 brought with them and installed in the system directory. Its
//! DCs and bitmaps are GDI's: a WinG DC is a memory DC, a WinG bitmap a
//! DIB section whose bits the program writes, and WinG's blits are GDI's
//! with SRCCOPY. The built-in 16-bit WING.DLL (win16/misc.rs) does the
//! same for 16-bit programs.
//!
//! On Windows 95 WinG's own WING32.DLL thunks to its 16-bit WING.DLL and
//! the display driver. The built-in one is there under every
//! `win32_version`, as on a machine where a game's setup installed WinG,
//! and it takes the place of a WING32.DLL beside the program, as the
//! other built-in DLLs do.

use super::gdi32::{self, bitmap, dc, draw, palette};
use super::stubs::{Api, Conv, Dll, Handler};
use crate::cpu::Cpu;

pub static DLL: Dll = Dll {
    name: "WING32.DLL",
    base: 0xBF58_0000,
    apis: APIS,
    vars: &[],
    init: None,
    term: None,
    vtables: &[],
};

const fn stdcall(name: &'static str, ordinal: u16, args: u8, handler: Handler) -> Api {
    Api {
        name,
        ordinal,
        args,
        conv: Conv::Stdcall,
        handler,
    }
}

/// The functions, with the names WinG's header gives them. Their
/// ordinals are those of WinG's own WING32.DLL, which numbers its ten
/// exports 1 to 10 in the alphabetical order of their names; they differ
/// from the 16-bit WING.DLL's 1001 to 1010. Append only (see `stubs`).
static APIS: &[Api] = &[
    stdcall("WinGCreateDC", 3, 0, create_dc),
    stdcall("WinGRecommendDIBFormat", 8, 1, recommend_dib_format),
    stdcall("WinGCreateBitmap", 2, 3, create_bitmap),
    stdcall("WinGGetDIBPointer", 7, 2, get_dib_pointer),
    stdcall("WinGGetDIBColorTable", 6, 4, bitmap::get_dib_color_table),
    stdcall("WinGSetDIBColorTable", 9, 4, bitmap::set_dib_color_table),
    stdcall("WinGCreateHalftonePalette", 5, 0, create_halftone_palette),
    stdcall("WinGCreateHalftoneBrush", 4, 3, create_halftone_brush),
    stdcall("WinGStretchBlt", 10, 10, stretch_blt),
    stdcall("WinGBitBlt", 1, 8, bit_blt),
];

const SRCCOPY: u32 = 0x00CC_0020;
const COLORONCOLOR: u32 = 3;
const BITMAPINFOHEADER_SIZE: u32 = 40;

/// WinGCreateDC(): a memory DC compatible with the screen.
fn create_dc(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    dc::create_compatible_dc(cpu, &[0])
}

/// WinGRecommendDIBFormat(info): the format WinG draws fastest from,
/// written as a BITMAPINFOHEADER: 8 bits a pixel, top-down (a height of
/// -1). FALSE for a null pointer.
fn recommend_dib_format(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[0];
    if at == 0 {
        return Some(0);
    }
    // Size, width, height, planes and bits a pixel, BI_RGB, then the
    // image size, resolution and colour counts, all 0.
    let header: [u32; 10] = [
        BITMAPINFOHEADER_SIZE,
        1,
        -1i32 as u32,
        0x0008_0001,
        0,
        0,
        0,
        0,
        0,
        0,
    ];
    for (i, v) in header.into_iter().enumerate() {
        cpu.bus.guest_write_32(at + 4 * i as u32, v);
    }
    if cpu.bus.guest_faulted() {
        return None;
    }
    Some(1)
}

/// WinGCreateBitmap(hdc, info, bits): a DIB section of the format
/// `info` gives, with RGB colours, its bits' address written at `bits`
/// when that isn't null.
fn create_bitmap(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hdc, info, bits) = (args[0], args[1], args[2]);
    bitmap::create_dib_section(cpu, &[hdc, info, 0, bits, 0, 0])
}

/// WinGGetDIBPointer(bitmap, info): the address of a WinG bitmap's bits,
/// with its BITMAPINFOHEADER copied to `info` when that isn't null; 0
/// for a handle that isn't a DIB section.
fn get_dib_pointer(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (handle, info) = (args[0], args[1]);
    gdi32::init(cpu);
    let Some(dib) = gdi32::gdi(cpu).bitmap(handle).and_then(|b| b.dib.clone()) else {
        return Some(0);
    };
    if info != 0 {
        let header = &dib.header[..dib.header.len().min(BITMAPINFOHEADER_SIZE as usize)];
        for (i, &byte) in header.iter().enumerate() {
            cpu.bus.guest_write_8(info + i as u32, byte);
        }
        if cpu.bus.guest_faulted() {
            return None;
        }
    }
    Some(dib.bits)
}

/// WinGCreateHalftonePalette(): a palette of the halftone colours,
/// GDI's.
fn create_halftone_palette(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    palette::create_halftone_palette(cpu, &[0])
}

/// WinGCreateHalftoneBrush(hdc, color, dither): a brush of `color`. WinG
/// dithers it in the halftone palette's colours with the dither type
/// asked for; the solid brush of the colour stands in for the pattern,
/// which GDI then draws in the nearest colour.
fn create_halftone_brush(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    gdi32::create_solid_brush(cpu, &[args[1]])
}

/// WinGStretchBlt(dest, x, y, width, height, source, x, y, width,
/// height): StretchBlt with SRCCOPY, dropping rows and columns
/// (COLORONCOLOR) as WinG does, the destination's stretch mode put back
/// after.
fn stretch_blt(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let dest = args[0];
    let old = dc::set_stretch_blt_mode(cpu, &[dest, COLORONCOLOR])?;
    let mut all = args[..10].to_vec();
    all.push(SRCCOPY);
    let result = draw::stretch_blt(cpu, &all);
    if old != 0 {
        dc::set_stretch_blt_mode(cpu, &[dest, old])?;
    }
    result
}

/// WinGBitBlt(dest, x, y, width, height, source, x, y): BitBlt with
/// SRCCOPY.
fn bit_blt(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mut all = args[..8].to_vec();
    all.push(SRCCOPY);
    draw::bit_blt(cpu, &all)
}

#[cfg(test)]
mod tests {
    use super::APIS;

    /// WinG's WING32.DLL numbers its exports 1 to 10 by the alphabetical
    /// order of their names, and a program that imports one by ordinal
    /// gets that function.
    #[test]
    fn ordinals_follow_the_names_alphabetical_order() {
        let mut names: Vec<&str> = APIS.iter().map(|a| a.name).collect();
        names.sort_unstable();
        for api in APIS {
            let place = names.iter().position(|&n| n == api.name).unwrap();
            assert_eq!(api.ordinal as usize, place + 1, "{}", api.name);
        }
    }
}
