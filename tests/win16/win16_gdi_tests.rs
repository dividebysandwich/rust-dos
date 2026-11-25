//! Win16's GDI: what Windows 3.1 programs get from its functions on the
//! built-in Windows. The programs are made byte by byte
//! (tests/common/win16).

use crate::win16;

use rust_dos::config::DesktopDepth;
use win16::{CODE, Code, program, run, run_with};

/// USER's ordinals.
const CREATE_WINDOW: u16 = 41;
const GET_DC: u16 = 66;
const SELECT_PALETTE: u16 = 282;

/// GDI's ordinals.
const CREATE_PALETTE: u16 = 360;
const GDI_SELECT_PALETTE: u16 = 361;
const IS_DC_CURRENT_PALETTE: u16 = 412;

/// WS_POPUP | WS_VISIBLE.
const POPUP: u32 = 0x9000_0000;

/// End the program with exit code `fail` unless AX is `value`.
fn expect_ax(code: &mut Code, value: u16, fail: u8) {
    let [lo, hi] = value.to_le_bytes();
    code.emit(&[
        0x3D, lo, hi, // cmp ax,value
        0x74, 0x06, // je over the exit
        0xB0, fail, // mov al,fail
    ])
    .exit_al();
}

/// IsDCCurrentPalette(DI): what AX must be.
fn expect_current(code: &mut Code, value: u16, fail: u8) {
    code.emit(&[0x57]) // push di
        .call("GDI", IS_DC_CURRENT_PALETTE);
    expect_ax(code, value, fail);
}

/// SelectPalette(DI, the palette at [BP+at], background).
fn select(code: &mut Code, at: u8, background: u16) {
    select_with(code, ("USER", SELECT_PALETTE), at, background);
}

/// The function `f` (SelectPalette or GDISelectPalette) of DI, the
/// palette at [BP+at] and `background`.
fn select_with(code: &mut Code, f: (&str, u16), at: u8, background: u16) {
    code.emit(&[
        0x57, // push di
        0xFF, 0x76, at, // push word [bp+at]
    ])
    .push(background)
    .call(f.0, f.1);
}

/// A program of the IsDCCurrentPalette tests: a window, its DC in DI,
/// and palettes A at [BP+2] and B at [BP], both from the same
/// LOGPALETTE, then the code `steps` adds, and exit code 1.
fn palette_program(steps: impl FnOnce(&mut Code)) -> Vec<u8> {
    let mut code = Code::new();
    let class_at = code.here() + 2;
    code.push_far(0);
    let title_at = code.here() + 2;
    code.push_far(0)
        .push((POPUP >> 16) as u16)
        .push(POPUP as u16)
        .push(0)
        .push(0)
        .push(100)
        .push(50)
        .push(0) // no parent
        .push(0) // no menu
        .push(0) // instance
        .push(0)
        .push(0) // no parameter
        .call("USER", CREATE_WINDOW)
        .emit(&[0x50]) // push ax
        .call("USER", GET_DC)
        .emit(&[0x8B, 0xF8]); // mov di,ax
    let mut palette_at = Vec::new();
    for _ in 0..2 {
        palette_at.push(code.here() + 2);
        code.push_far(0).call("GDI", CREATE_PALETTE).emit(&[0x50]); // push ax
    }
    code.emit(&[0x8B, 0xEC]); // mov bp,sp
    steps(&mut code);
    code.emit(&[0xB0, 0x01]).exit_al(); // mov al,1
    let class = code.here();
    code.emit(b"BUTTON\0");
    let title = code.here();
    code.emit(b"\0");
    // LOGPALETTE: version 300h, 2 entries.
    let palette = code.here();
    code.emit(&[0x00, 0x03, 0x02, 0x00]);
    code.emit(&[0x10, 0x20, 0x30, 0x00, 0x40, 0x50, 0x60, 0x00]);
    code.patch(class_at, class);
    code.patch(title_at, title);
    for at in palette_at {
        code.patch(at, palette);
    }
    program(&code, CODE)
}

/// The program of the IsDCCurrentPalette tests: IsDCCurrentPalette
/// after each SelectPalette of A and B, which must answer TRUE where the
/// palette is the foreground one when `palette`, on a palette device,
/// and never otherwise. Exit code 1 when every answer is right, the
/// failing step's code otherwise.
fn current_palette_program(palette: bool) -> Vec<u8> {
    let foreground = palette as u16;
    palette_program(|code| {
        // The DC has the default palette, and no palette is the
        // foreground one yet.
        expect_current(code, 0, 2);
        // A in the foreground.
        select(code, 2, 0);
        expect_current(code, foreground, 3);
        // B in the background: the DC no longer has the foreground
        // palette.
        select(code, 0, 1);
        expect_current(code, 0, 4);
        // A again, in the background: still the foreground palette.
        select(code, 2, 1);
        expect_current(code, foreground, 5);
        // A handle that isn't a DC.
        code.push(0);
        code.call("GDI", IS_DC_CURRENT_PALETTE);
        expect_ax(code, 0, 6);
    })
}

/// The program of the GDISelectPalette tests: GDISelectPalette of A
/// with wBkg FALSE makes it the foreground palette on any display, and
/// USER's SelectPalette of B then does the same only on a palette
/// device (`palette`). Exit codes as `current_palette_program`'s.
fn gdi_select_palette_program(palette: bool) -> Vec<u8> {
    let gdi = ("GDI", GDI_SELECT_PALETTE);
    palette_program(|code| {
        // With wBkg TRUE, A doesn't become the foreground palette.
        select_with(code, gdi, 2, 1);
        expect_current(code, 0, 2);
        // With wBkg FALSE it does.
        select_with(code, gdi, 2, 0);
        expect_current(code, 1, 3);
        // USER's SelectPalette of B in the foreground.
        select(code, 0, 0);
        expect_current(code, palette as u16, 4);
    })
}

#[test]
fn is_dc_current_palette_tells_whether_the_dc_has_the_foreground_palette() {
    // GDI.412, IsDCCurrentPalette(hdc), which Video for Windows 1.1's
    // DrawDib calls on a 256-colour display: Windows 98's compares the
    // DC's palette with the one GDISelectPalette last selected in the
    // foreground, which USER's SelectPalette does for a DC of the active
    // window without bForceBackground.
    let (exit, log) = run_with(
        "is_dc_current_palette",
        current_palette_program(true),
        |cpu, _| {
            cpu.bus.win32.depth = DesktopDepth::Palette;
        },
    );
    assert_eq!(exit, 1, "{log}");
}

#[test]
fn is_dc_current_palette_is_false_without_a_palette_device() {
    // USER's SelectPalette selects a palette in the foreground only on a
    // palette device, so on a desktop of more colours no DC has the
    // foreground palette.
    let (exit, log) = run("is_dc_current_palette_deep", current_palette_program(false));
    assert_eq!(exit, 1, "{log}");
}

#[test]
fn gdi_select_palette_selects_in_the_foreground_on_any_display() {
    // GDI.361, GDISelectPalette(hdc, hpal, wBkg): Windows 98 SE's keeps
    // the palette as its device's foreground one whenever wBkg is
    // FALSE. USER's SelectPalette passes FALSE only on a palette device.
    let (exit, log) = run_with(
        "gdi_select_palette",
        gdi_select_palette_program(true),
        |cpu, _| {
            cpu.bus.win32.depth = DesktopDepth::Palette;
        },
    );
    assert_eq!(exit, 1, "{log}");
    let (exit, log) = run("gdi_select_palette_deep", gdi_select_palette_program(false));
    assert_eq!(exit, 1, "{log}");
}
