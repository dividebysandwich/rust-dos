//! Win16's GDI: what Windows 3.1 programs get from its functions on the
//! built-in Windows. The programs are made byte by byte
//! (tests/common/win16).

use crate::win16;

use win16::{CODE, Code, program, run};

/// USER's ordinals.
const CREATE_WINDOW: u16 = 41;
const GET_DC: u16 = 66;
const SELECT_PALETTE: u16 = 282;

/// GDI's ordinals.
const CREATE_PALETTE: u16 = 360;
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
    code.emit(&[
        0x57, // push di
        0xFF, 0x76, at, // push word [bp+at]
    ])
    .push(background)
    .call("USER", SELECT_PALETTE);
}

#[test]
fn is_dc_current_palette_tells_whether_the_dc_has_the_foreground_palette() {
    // GDI.412, IsDCCurrentPalette(hdc), which Video for Windows 1.1's
    // DrawDib calls: Windows 98's compares the DC's palette with the one
    // GDISelectPalette last selected in the foreground, which USER's
    // SelectPalette does for a DC of the active window without
    // bForceBackground. A window's DC, palettes A and B: exit code 1
    // when every answer is right, the failing step's code otherwise.
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
    // A at [bp+2], B at [bp], both from the same LOGPALETTE.
    let mut palette_at = Vec::new();
    for _ in 0..2 {
        palette_at.push(code.here() + 2);
        code.push_far(0).call("GDI", CREATE_PALETTE).emit(&[0x50]); // push ax
    }
    code.emit(&[0x8B, 0xEC]); // mov bp,sp
    // The DC has the default palette, and no palette is the foreground
    // one yet.
    expect_current(&mut code, 0, 2);
    // A in the foreground.
    select(&mut code, 2, 0);
    expect_current(&mut code, 1, 3);
    // B in the background: the DC no longer has the foreground palette.
    select(&mut code, 0, 1);
    expect_current(&mut code, 0, 4);
    // A again, in the background: still the foreground palette.
    select(&mut code, 2, 1);
    expect_current(&mut code, 1, 5);
    // A handle that isn't a DC.
    code.push(0);
    code.call("GDI", IS_DC_CURRENT_PALETTE);
    expect_ax(&mut code, 0, 6);
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
    let (exit, log) = run("is_dc_current_palette", program(&code, CODE));
    assert_eq!(exit, 1, "{log}");
}
