//! Win16's USER: what Windows 3.1 programs get from its functions on the
//! built-in Windows. The programs are made byte by byte (tests/win16).

mod win16;

use win16::{CODE, Code, program, run};

/// USER's ordinals.
const WINDOW_FROM_POINT: u16 = 30;
const CREATE_WINDOW: u16 = 41;
const FIND_WINDOW: u16 = 50;
const GET_ACTIVE_WINDOW: u16 = 60;
const SET_SCROLL_RANGE: u16 = 64;
const GET_SCROLL_RANGE: u16 = 65;
const CALL_WINDOW_PROC: u16 = 122;
const GET_CLASS_LONG: u16 = 131;
const SET_WINDOW_LONG: u16 = 136;

/// Strings at the end of a program's code, and the places that push
/// their far pointers.
#[derive(Default)]
struct Strings {
    uses: Vec<(u16, &'static [u8])>,
}

impl Strings {
    /// Push the far pointer to `text` (NUL-terminated).
    fn push(&mut self, code: &mut Code, text: &'static [u8]) {
        self.uses.push((code.here() + 2, text));
        code.push_far(0);
    }

    /// Put the strings after the code and point the pushes at them.
    fn place(self, code: &mut Code) {
        for (at, text) in self.uses {
            let offset = code.here();
            code.emit(text);
            code.patch(at, offset);
        }
    }
}

/// CreateWindow(class, text, style, x, y, cx, cy, parent from SI or
/// none, id), its window in AX.
fn create_window(
    code: &mut Code,
    strings: &mut Strings,
    class: &'static [u8],
    style: u32,
    rect: [u16; 4],
    parent: bool,
    id: u16,
) {
    strings.push(code, class);
    strings.push(code, b"\0");
    code.push((style >> 16) as u16).push(style as u16);
    for v in rect {
        code.push(v);
    }
    if parent {
        code.emit(&[0x56]); // push si
    } else {
        code.push(0);
    }
    code.push(id)
        .push(0) // instance
        .push(0)
        .push(0) // no parameter
        .call("USER", CREATE_WINDOW);
}

#[test]
fn with_no_window_active_get_active_window_gives_program_managers() {
    // Exit code 1 when GetActiveWindow gives the window FindWindow finds
    // for Program Manager's class, 0 when it gives another or none.
    let mut code = Code::new();
    code.call("USER", GET_ACTIVE_WINDOW).emit(&[0x50]); // push ax
    let class_at = code.here() + 2;
    code.push_far(0)
        .push(0)
        .push(0) // no title
        .call("USER", FIND_WINDOW)
        .emit(&[
            0x59, // pop cx
            0x3B, 0xC1, // cmp ax,cx
            0x75, 0x08, // jne bad
            0x0B, 0xC0, // or ax,ax
            0x74, 0x04, // jz bad
            0xB0, 0x01, // mov al,1
            0xEB, 0x02, // jmp over the next
            0xB0, 0x00, // bad: mov al,0
        ])
        .exit_al();
    let class = code.here();
    code.emit(b"Progman\0");
    code.patch(class_at, class);
    let (exit, log) = run("active_none", program(&code, CODE));
    assert_eq!(exit, 1, "{log}");
}

#[test]
fn window_from_point_passes_over_a_group_box_to_the_check_box_in_it() {
    // A window with a group box and an auto check box inside it, made in
    // that order as a dialog makes them, so the group box is above the
    // check box. Windows 3.1 and 98 give the point on the check box to
    // the check box: the group box's WM_NCHITTEST answers HTTRANSPARENT.
    // Exit code 1 for the check box, 2 for the group box.
    let mut code = Code::new();
    let mut strings = Strings::default();
    let popup = 0x9000_0000; // WS_POPUP | WS_VISIBLE
    create_window(&mut code, &mut strings, b"BUTTON\0", popup, [0, 0, 200, 100], false, 0);
    code.emit(&[0x8B, 0xF0]); // mov si,ax
    let group = 0x5000_0007; // WS_CHILD | WS_VISIBLE | BS_GROUPBOX
    create_window(&mut code, &mut strings, b"BUTTON\0", group, [10, 10, 150, 60], true, 1);
    code.emit(&[0x50]); // push ax
    let check = 0x5001_0003; // WS_TABSTOP | BS_AUTOCHECKBOX
    create_window(&mut code, &mut strings, b"BUTTON\0", check, [20, 30, 100, 12], true, 2);
    code.emit(&[0x8B, 0xF8]) // mov di,ax
        .push(35) // y
        .push(30) // x
        .call("USER", WINDOW_FROM_POINT)
        .emit(&[
            0x59, // pop cx
            0x3B, 0xC7, // cmp ax,di
            0x75, 0x04, // jne not_check
            0xB0, 0x01, // mov al,1
            0xEB, 0x0A, // jmp done
            0x3B, 0xC1, // not_check: cmp ax,cx
            0x75, 0x04, // jne other
            0xB0, 0x02, // mov al,2
            0xEB, 0x02, // jmp done
            0xB0, 0x00, // other: mov al,0
        ])
        .exit_al();
    strings.place(&mut code);
    let (exit, log) = run("group_box_click", program(&code, CODE));
    assert_eq!(exit, 1, "{log}");
}

#[test]
fn get_scroll_range_reads_a_scroll_bar_with_a_16_bit_window_procedure() {
    // A scroll bar control subclassed with a 16-bit window procedure, as
    // MFC's DDX_Control does, which passes every message on to the class's
    // procedure. SetScrollRange and GetScrollRange go through it as
    // SBM_SETRANGE and SBM_GETRANGE, which Windows 3.1 packs in lParam and
    // the return value. Exit code 1 when GetScrollRange gives 3 and 90.
    let mut code = Code::new();
    let mut strings = Strings::default();
    let popup = 0x9000_0000; // WS_POPUP | WS_VISIBLE, SBS_HORZ
    create_window(&mut code, &mut strings, b"SCROLLBAR\0", popup, [0, 0, 100, 12], false, 0);
    code.emit(&[0x8B, 0xF0]) // mov si,ax
        .emit(&[0x56]) // push si
        .push(0xFFFC); // GWL_WNDPROC
    let proc_at = code.here() + 2;
    code.push_far(0)
        .call("USER", SET_WINDOW_LONG)
        .emit(&[0x56]) // push si
        .push(2) // SB_CTL
        .push(3)
        .push(90)
        .push(0) // no redraw
        .call("USER", SET_SCROLL_RANGE)
        .emit(&[0x56]) // push si
        .push(2) // SB_CTL
        .emit(&[0x1E]) // push ds
        .push(0x20)
        .emit(&[0x1E]) // push ds
        .push(0x22)
        .call("USER", GET_SCROLL_RANGE)
        .emit(&[
            0xA1, 0x20, 0x00, // mov ax,[20h]
            0x3D, 0x03, 0x00, // cmp ax,3
            0x75, 0x0C, // jne bad
            0xA1, 0x22, 0x00, // mov ax,[22h]
            0x3D, 0x5A, 0x00, // cmp ax,90
            0x75, 0x04, // jne bad
            0xB0, 0x01, // mov al,1
            0xEB, 0x02, // jmp done
            0xB0, 0x00, // bad: mov al,0
        ])
        .exit_al();
    // The window procedure: CallWindowProc(the class's procedure, hwnd,
    // msg, wParam, lParam).
    let proc = code.here();
    code.emit(&[
        0x55, // push bp
        0x8B, 0xEC, // mov bp,sp
        0xFF, 0x76, 0x0E, // push [bp+0Eh] (hwnd)
    ])
    .push(0xFFE8) // GCL_WNDPROC
    .call("USER", GET_CLASS_LONG)
    .emit(&[
        0x52, // push dx
        0x50, // push ax
        0xFF, 0x76, 0x0E, // push [bp+0Eh] (hwnd)
        0xFF, 0x76, 0x0C, // push [bp+0Ch] (msg)
        0xFF, 0x76, 0x0A, // push [bp+0Ah] (wParam)
        0xFF, 0x76, 0x08, // push [bp+8] (lParam)
        0xFF, 0x76, 0x06, // push [bp+6]
    ])
    .call("USER", CALL_WINDOW_PROC)
    .emit(&[
        0x5D, // pop bp
        0xCA, 0x0A, 0x00, // retf 0Ah
    ]);
    code.patch(proc_at, proc);
    strings.place(&mut code);
    let (exit, log) = run("scroll_range_16", program(&code, CODE));
    assert_eq!(exit, 1, "{log}");
}
