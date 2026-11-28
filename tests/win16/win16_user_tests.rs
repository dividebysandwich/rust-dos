//! Win16's USER: what Windows 3.1 programs get from its functions on the
//! built-in Windows. The programs are made byte by byte
//! (tests/common/win16).

use crate::win16;

use win16::{CODE, Code, program, program_named, run, run_with, run_with_prompts};

/// ExitWindows's code for restarting Windows.
const EW_RESTARTWINDOWS: u16 = 0x42;

/// USER's ordinals.
const EXIT_WINDOWS: u16 = 7;
const EXIT_WINDOWS_EXEC: u16 = 246;
const WINDOW_FROM_POINT: u16 = 30;
const GET_WINDOW_TEXT: u16 = 36;
const CREATE_WINDOW: u16 = 41;
const FIND_WINDOW: u16 = 50;
const GET_ACTIVE_WINDOW: u16 = 60;
const SET_SCROLL_RANGE: u16 = 64;
const GET_SCROLL_RANGE: u16 = 65;
const SEND_MESSAGE: u16 = 111;
const CALL_WINDOW_PROC: u16 = 122;
const GET_CLASS_LONG: u16 = 131;
const SET_WINDOW_LONG: u16 = 136;
const REGISTER_CLASS: u16 = 57;
const GET_CLASS_INFO: u16 = 404;
const DEF_WINDOW_PROC: u16 = 107;
const SET_WINDOWS_HOOK_EX: u16 = 291;
const POST_QUIT_MESSAGE: u16 = 6;
const GET_DLG_ITEM_INT: u16 = 95;
const GET_MESSAGE: u16 = 108;
const PEEK_MESSAGE: u16 = 109;
const POST_MESSAGE: u16 = 110;
const WAIT_MESSAGE: u16 = 112;
const DISPATCH_MESSAGE: u16 = 114;

/// KERNEL's ordinals.
const GLOBAL_ALLOC: u16 = 15;
const GLOBAL_LOCK: u16 = 18;
const YIELD: u16 = 29;
const GET_MODULE_USAGE: u16 = 48;
const WIN_EXEC: u16 = 166;

/// Window styles.
const POPUP: u32 = 0x9000_0000; // WS_POPUP | WS_VISIBLE
const CHILD: u32 = 0x5000_0000; // WS_CHILD | WS_VISIBLE
const WS_TABSTOP: u32 = 0x0001_0000;
const BS_AUTOCHECKBOX: u32 = 0x0003;
const BS_GROUPBOX: u32 = 0x0007;

/// SBM_GETRANGE as Windows 3.1 numbers it.
const SBM_GETRANGE16: u16 = 0x0403;

/// A program's code, and the strings that go after it.
#[derive(Default)]
struct Program {
    code: Code,
    /// Where the code pushes a string's offset, and the string.
    strings: Vec<(u16, &'static [u8])>,
}

impl Program {
    /// Push the far pointer to `text` (NUL-terminated).
    fn push_string(&mut self, text: &'static [u8]) {
        self.strings.push((self.code.here() + 2, text));
        self.code.push_far(0);
    }

    /// CreateWindow(class, "", style, rect, parent, id): the parent is
    /// the window in SI, or none. The window comes back in AX.
    fn create_window(
        &mut self,
        class: &'static [u8],
        style: u32,
        rect: [u16; 4],
        parent: bool,
        id: u16,
    ) {
        self.push_string(class);
        self.push_string(b"\0");
        let code = &mut self.code;
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

    /// The program's file: the code, then the strings.
    fn build(mut self) -> Vec<u8> {
        self.place_strings();
        program(&self.code, CODE)
    }

    /// `build`, with the module name `name`.
    fn build_named(mut self, name: &str) -> Vec<u8> {
        self.place_strings();
        program_named(name, &self.code, CODE)
    }

    /// Put the strings after the code.
    fn place_strings(&mut self) {
        for (at, text) in std::mem::take(&mut self.strings) {
            let offset = self.code.here();
            self.code.emit(text);
            self.code.patch(at, offset);
        }
    }
}

/// End with exit code 1 when WindowFromPoint(x, y) gives the window in
/// DI, 2 when it gives the one in SI, 0 otherwise.
fn exit_with_window_at(code: &mut Code, x: u16, y: u16) {
    code.push(y).push(x).call("USER", WINDOW_FROM_POINT).emit(&[
        0x3B, 0xC7, // cmp ax,di
        0x75, 0x04, // jne not_di
        0xB0, 0x01, // mov al,1
        0xEB, 0x0A, // jmp done
        0x3B, 0xC6, // not_di: cmp ax,si
        0x75, 0x04, // jne other
        0xB0, 0x02, // mov al,2
        0xEB, 0x02, // jmp done
        0xB0, 0x00, // other: mov al,0
    ]);
    code.exit_al();
}

/// End with exit code 1 when AX is 3 and DX is 90 (the range the tests
/// set), 0 otherwise.
fn exit_with_range(code: &mut Code) {
    code.emit(&[
        0x3D, 0x03, 0x00, // cmp ax,3
        0x75, 0x09, // jne bad
        0x83, 0xFA, 0x5A, // cmp dx,90
        0x75, 0x04, // jne bad
        0xB0, 0x01, // mov al,1
        0xEB, 0x02, // jmp done
        0xB0, 0x00, // bad: mov al,0
    ]);
    code.exit_al();
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
    let mut p = Program::default();
    p.create_window(b"BUTTON\0", POPUP, [0, 0, 200, 100], false, 0);
    p.code.emit(&[0x8B, 0xF0]); // mov si,ax
    p.create_window(b"BUTTON\0", CHILD | BS_GROUPBOX, [10, 10, 150, 60], true, 1);
    p.code.emit(&[0x50]); // push ax
    let check = CHILD | WS_TABSTOP | BS_AUTOCHECKBOX;
    p.create_window(b"BUTTON\0", check, [20, 30, 100, 12], true, 2);
    p.code.emit(&[
        0x8B, 0xF8, // mov di,ax
        0x5E, // pop si (the group box)
    ]);
    exit_with_window_at(&mut p.code, 30, 35);
    let (exit, log) = run("group_box_click", p.build());
    assert_eq!(exit, 1, "{log}");
}

#[test]
fn window_from_point_passes_over_a_static_label() {
    // A static control without SS_NOTIFY over a push button: its
    // WM_NCHITTEST answers HTTRANSPARENT too, and the button below gets
    // the point. Exit code 1 for the button, 2 for the static.
    let mut p = Program::default();
    p.create_window(b"BUTTON\0", POPUP, [0, 0, 200, 100], false, 0);
    p.code.emit(&[0x8B, 0xF0]); // mov si,ax
    p.create_window(b"STATIC\0", CHILD, [10, 10, 100, 20], true, 1);
    p.code.emit(&[0x50]); // push ax
    p.create_window(b"BUTTON\0", CHILD, [10, 10, 100, 20], true, 2);
    p.code.emit(&[
        0x8B, 0xF8, // mov di,ax
        0x5E, // pop si (the static)
    ]);
    exit_with_window_at(&mut p.code, 20, 15);
    let (exit, log) = run("static_click", p.build());
    assert_eq!(exit, 1, "{log}");
}

#[test]
fn sbm_getrange_returns_the_range_in_dx_ax() {
    // Windows 3.1's SBM_GETRANGE (WM_USER+3) sent to a scroll bar
    // control returns its range as MAKELONG(min, max). Exit code 1 for
    // 3 and 90.
    let mut p = Program::default();
    p.create_window(b"SCROLLBAR\0", POPUP, [0, 0, 100, 12], false, 0);
    p.code
        .emit(&[0x8B, 0xF0]) // mov si,ax
        .emit(&[0x56]) // push si
        .push(2) // SB_CTL
        .push(3)
        .push(90)
        .push(0) // no redraw
        .call("USER", SET_SCROLL_RANGE)
        .emit(&[0x56]) // push si
        .push(SBM_GETRANGE16)
        .push(0)
        .push(0)
        .push(0)
        .call("USER", SEND_MESSAGE);
    exit_with_range(&mut p.code);
    let (exit, log) = run("sbm_getrange16", p.build());
    assert_eq!(exit, 1, "{log}");
}

#[test]
fn get_scroll_range_reads_a_scroll_bar_with_a_16_bit_window_procedure() {
    // A scroll bar control subclassed with a 16-bit window procedure, as
    // MFC's DDX_Control does, which passes every message on to the
    // class's procedure. SetScrollRange and GetScrollRange go through it
    // as SBM_SETRANGE and SBM_GETRANGE, which Windows 3.1 packs in lParam
    // and the return value. Exit code 1 when GetScrollRange gives 3 and
    // 90.
    let mut p = Program::default();
    p.create_window(b"SCROLLBAR\0", POPUP, [0, 0, 100, 12], false, 0);
    p.code
        .emit(&[0x8B, 0xF0]) // mov si,ax
        .emit(&[0x56]) // push si
        .push(0xFFFC); // GWL_WNDPROC
    let proc_at = p.code.here() + 2;
    p.code
        .push_far(0)
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
            0x8B, 0x16, 0x22, 0x00, // mov dx,[22h]
        ]);
    exit_with_range(&mut p.code);
    // The window procedure: CallWindowProc(the class's procedure, hwnd,
    // msg, wParam, lParam).
    let proc = p.code.here();
    p.code
        .emit(&[
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
    p.code.patch(proc_at, proc);
    let (exit, log) = run("scroll_range_16", p.build());
    assert_eq!(exit, 1, "{log}");
}

#[test]
fn a_superclass_of_listbox_takes_windows_3_1s_list_box_messages() {
    // A class of the program's own made from LISTBOX, as Delphi makes
    // TListBox: GetClassInfo, its window procedure passing every message
    // on to LISTBOX's with CallWindowProc. LB_ADDSTRING, LB_SETITEMDATA
    // and LB_GETITEMDATA, numbered from WM_USER as Windows 3.1 numbers
    // them, reach the list box. Exit code 1, or which check failed.
    const LB_ADDSTRING16: u16 = 0x0401;
    const LB_GETITEMDATA16: u16 = 0x041A;
    const LB_SETITEMDATA16: u16 = 0x041B;
    const CS_GLOBALCLASS: u16 = 0x4000;
    // The WNDCLASS at 20h, LISTBOX's procedure kept at 40h.
    const WC: u8 = 0x20;
    const BASE: u8 = 0x40;
    let mut p = Program::default();
    p.code.push(0);
    p.push_string(b"LISTBOX\0");
    p.code
        .emit(&[0x1E]) // push ds
        .push(WC as u16)
        .call("USER", GET_CLASS_INFO)
        .emit(&[0x0B, 0xC0, 0x75, 0x06]) // or ax,ax; jnz over the next
        .emit(&[0xB0, 2, 0xB4, 0x4C, 0xCD, 0x21])
        .emit(&[0xA1, WC + 2, 0x00, 0xA3, BASE, 0x00]) // the procedure
        .emit(&[0xA1, WC + 4, 0x00, 0xA3, BASE + 2, 0x00])
        .emit(&[0xC7, 0x06, WC, 0x00])
        .emit(&CS_GLOBALCLASS.to_le_bytes())
        .emit(&[0x89, 0x3E, WC + 10, 0x00]); // mov [WC+10],di: the instance
    let proc_at = p.code.here() + 4;
    p.code
        .emit(&[0xC7, 0x06, WC + 2, 0x00, 0x00, 0x00]) // its procedure
        .emit(&[0x8C, 0x0E, WC + 4, 0x00]); // mov [WC+4],cs
    let name_at = p.code.here() + 4;
    p.code
        .emit(&[0xC7, 0x06, WC + 22, 0x00, 0x00, 0x00]) // its name
        .emit(&[0x8C, 0x0E, WC + 24, 0x00])
        .emit(&[0x1E])
        .push(WC as u16)
        .call("USER", REGISTER_CLASS)
        .emit(&[0x0B, 0xC0, 0x75, 0x06])
        .emit(&[0xB0, 3, 0xB4, 0x4C, 0xCD, 0x21]);
    p.create_window(b"TLIST\0", POPUP | 0x40, [0, 0, 100, 60], false, 0);
    p.code
        .emit(&[0x8B, 0xF0]) // mov si,ax
        .emit(&[0x0B, 0xC0, 0x75, 0x06])
        .emit(&[0xB0, 4, 0xB4, 0x4C, 0xCD, 0x21]);
    for (i, text) in [b"one\0", b"two\0"].into_iter().enumerate() {
        p.code.emit(&[0x56]).push(LB_ADDSTRING16).push(0);
        p.push_string(text);
        p.code
            .call("USER", SEND_MESSAGE)
            .emit(&[0x3D, i as u8, 0x00, 0x74, 0x06]) // cmp ax,i
            .emit(&[0xB0, 5 + i as u8, 0xB4, 0x4C, 0xCD, 0x21]);
    }
    p.code
        .emit(&[0x56])
        .push(LB_SETITEMDATA16)
        .push(1)
        .push(0x1234)
        .push(0x5678)
        .call("USER", SEND_MESSAGE)
        .emit(&[0x56])
        .push(LB_GETITEMDATA16)
        .push(1)
        .push(0)
        .push(0)
        .call("USER", SEND_MESSAGE)
        .emit(&[0x3D, 0x78, 0x56, 0x75, 0x09]) // cmp ax,5678h; jne bad
        .emit(&[0x81, 0xFA, 0x34, 0x12, 0x75, 0x03]) // cmp dx,1234h; jne bad
        .emit(&[0xB0, 0x01])
        .exit_al()
        .emit(&[0xB0, 7])
        .exit_al();
    // The window procedure: CallWindowProc(LISTBOX's procedure, hwnd,
    // msg, wParam, lParam).
    let proc = p.code.here();
    p.code
        .emit(&[0x55, 0x8B, 0xEC]) // push bp; mov bp,sp
        .emit(&[0xFF, 0x36, BASE + 2, 0x00]) // push word [BASE+2]
        .emit(&[0xFF, 0x36, BASE, 0x00])
        .emit(&[
            0xFF, 0x76, 0x0E, // push [bp+0Eh] (hwnd)
            0xFF, 0x76, 0x0C, // push [bp+0Ch] (msg)
            0xFF, 0x76, 0x0A, // push [bp+0Ah] (wParam)
            0xFF, 0x76, 0x08, // push [bp+8] (lParam)
            0xFF, 0x76, 0x06, // push [bp+6]
        ])
        .call("USER", CALL_WINDOW_PROC)
        .emit(&[0x5D, 0xCA, 0x0A, 0x00]); // pop bp; retf 0Ah
    p.code.patch(proc_at, proc);
    let name = p.code.here();
    p.code.emit(b"TLIST\0");
    p.code.patch(name_at, name);
    let (exit, log) = run("listbox_superclass", p.build());
    assert_eq!(exit, 1, "{log}");
}

/// Where the subclass tests keep a buffer in their data segment.
const BUFFER: u16 = 0x20;

/// Subclass the control in SI with a 16-bit window procedure that
/// passes every message on to its class's procedure, as CTL3D, BWCC and
/// Memphis Math's SDSC.DLL do: USER32 then sends the control's messages
/// to 16-bit code, and they come back to 32-bit code through
/// CallWindowProc. Returns where the procedure's offset is to be
/// patched in.
fn subclass(p: &mut Program) -> u16 {
    p.code
        .emit(&[0x56]) // push si
        .push(0xFFFC); // GWL_WNDPROC
    let proc_at = p.code.here() + 2;
    p.code.push_far(0).call("USER", SET_WINDOW_LONG);
    proc_at
}

/// The procedure `subclass` installs: CallWindowProc(the class's
/// procedure, hwnd, msg, wParam, lParam).
fn pass_on_procedure(p: &mut Program, proc_at: u16) {
    let proc = p.code.here();
    p.code
        .emit(&[
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
    p.code.patch(proc_at, proc);
}

/// SendMessage(SI, msg, wParam, the string `text`).
fn send_string(p: &mut Program, msg: u16, wp: u16, text: &'static [u8]) {
    p.code.emit(&[0x56]).push(msg).push(wp);
    p.push_string(text);
    p.code.call("USER", SEND_MESSAGE);
}

/// SendMessage(SI, msg, wParam, DS:BUFFER).
fn send_buffer(p: &mut Program, msg: u16, wp: u16) {
    p.code
        .emit(&[0x56])
        .push(msg)
        .push(wp)
        .emit(&[0x1E]) // push ds
        .push(BUFFER)
        .call("USER", SEND_MESSAGE);
}

/// End with `fail` unless AX is `value`.
fn expect_ax(code: &mut Code, value: u16, fail: u8) {
    let [lo, hi] = value.to_le_bytes();
    code.emit(&[0x3D, lo, hi]) // cmp ax,value
        .emit(&[0x74, 0x06]) // je over the next
        .emit(&[0xB0, fail, 0xB4, 0x4C, 0xCD, 0x21]);
}

/// End with `fail` unless the bytes at DS:BUFFER are `text`, compared a
/// word at a time (`text` has an even length).
fn expect_buffer(code: &mut Code, text: &[u8], fail: u8) {
    for (i, pair) in text.chunks(2).enumerate() {
        let [at_lo, at_hi] = (BUFFER + 2 * i as u16).to_le_bytes();
        code.emit(&[0x81, 0x3E, at_lo, at_hi, pair[0], pair[1]]) // cmp word
            .emit(&[0x74, 0x06])
            .emit(&[0xB0, fail, 0xB4, 0x4C, 0xCD, 0x21]);
    }
}

#[test]
fn a_subclassed_list_box_takes_and_gives_back_its_strings() {
    // LB_ADDSTRING, LB_FINDSTRING and LB_GETTEXT sent to a list box with
    // a 16-bit window procedure, which gets the caller's strings and
    // buffer and passes them on to the list box's 32-bit procedure with
    // CallWindowProc. Exit code 1, or which check failed.
    const LB_ADDSTRING16: u16 = 0x0401;
    const LB_GETTEXT16: u16 = 0x040A;
    const LB_FINDSTRING16: u16 = 0x0410;
    let mut p = Program::default();
    p.create_window(b"LISTBOX\0", POPUP, [0, 0, 100, 60], false, 0);
    p.code.emit(&[0x8B, 0xF0]); // mov si,ax
    let proc_at = subclass(&mut p);
    send_string(&mut p, LB_ADDSTRING16, 0, b"alpha\0");
    expect_ax(&mut p.code, 0, 2);
    send_string(&mut p, LB_ADDSTRING16, 0, b"beta\0");
    expect_ax(&mut p.code, 1, 3);
    send_string(&mut p, LB_FINDSTRING16, 0xFFFF, b"be\0");
    expect_ax(&mut p.code, 1, 4);
    send_buffer(&mut p, LB_GETTEXT16, 1);
    expect_ax(&mut p.code, 4, 5);
    expect_buffer(&mut p.code, b"beta\0\0", 6);
    // LB_ERR for an item the list doesn't have, the buffer as it was.
    send_buffer(&mut p, LB_GETTEXT16, 5);
    expect_ax(&mut p.code, 0xFFFF, 7);
    expect_buffer(&mut p.code, b"beta\0\0", 8);
    p.code.emit(&[0xB0, 0x01]).exit_al();
    pass_on_procedure(&mut p, proc_at);
    let (exit, log) = run("listbox_subclass_strings", p.build());
    assert_eq!(exit, 1, "{log}");
}

#[test]
fn a_subclassed_list_box_without_strings_gives_back_item_data() {
    // An owner-drawn list box without LBS_HASSTRINGS keeps a DWORD for
    // each item: LB_ADDSTRING's lParam is the data, which goes to the
    // 16-bit window procedure as it is, and LB_GETTEXT writes the DWORD
    // into the caller's buffer, or nothing for an item it doesn't have.
    // Exit code 1, or which check failed.
    const LB_ADDSTRING16: u16 = 0x0401;
    const LB_GETTEXT16: u16 = 0x040A;
    const LBS_OWNERDRAWFIXED: u32 = 0x0010;
    let mut p = Program::default();
    p.create_window(
        b"LISTBOX\0",
        POPUP | LBS_OWNERDRAWFIXED,
        [0, 0, 100, 60],
        false,
        0,
    );
    p.code.emit(&[0x8B, 0xF0]); // mov si,ax
    let proc_at = subclass(&mut p);
    p.code
        .emit(&[0x56])
        .push(LB_ADDSTRING16)
        .push(0)
        .push(0x1234)
        .push(0x5678)
        .call("USER", SEND_MESSAGE);
    expect_ax(&mut p.code, 0, 2);
    send_buffer(&mut p, LB_GETTEXT16, 0);
    expect_ax(&mut p.code, 4, 3);
    expect_buffer(&mut p.code, &[0x78, 0x56, 0x34, 0x12], 4);
    send_buffer(&mut p, LB_GETTEXT16, 3);
    expect_ax(&mut p.code, 0xFFFF, 5);
    expect_buffer(&mut p.code, &[0x78, 0x56, 0x34, 0x12], 6);
    p.code.emit(&[0xB0, 0x01]).exit_al();
    pass_on_procedure(&mut p, proc_at);
    let (exit, log) = run("listbox_subclass_item_data", p.build());
    assert_eq!(exit, 1, "{log}");
}

#[test]
fn a_subclassed_combo_box_gives_back_the_selected_items_text() {
    // Memphis Math's Objective Selection: SDSC.DLL subclasses the combo
    // box after the game added its objectives, and the game reads the
    // first one's text with CB_GETLBTEXT to find it in its database. The
    // text has to reach the game's buffer through the 16-bit procedure.
    // Exit code 1, or which check failed.
    const CB_ADDSTRING16: u16 = 0x0403;
    const CB_GETLBTEXT16: u16 = 0x0408;
    const CB_SETCURSEL16: u16 = 0x040E;
    const CBS_DROPDOWNLIST: u32 = 0x0003;
    let mut p = Program::default();
    p.create_window(
        b"COMBOBOX\0",
        POPUP | CBS_DROPDOWNLIST,
        [0, 0, 200, 100],
        false,
        0,
    );
    p.code.emit(&[0x8B, 0xF0]); // mov si,ax
    send_string(&mut p, CB_ADDSTRING16, 0, b"FEQ01 - fractions\0");
    expect_ax(&mut p.code, 0, 2);
    let proc_at = subclass(&mut p);
    send_string(&mut p, CB_ADDSTRING16, 0, b"FLT01\0");
    expect_ax(&mut p.code, 1, 3);
    p.code
        .emit(&[0x56])
        .push(CB_SETCURSEL16)
        .push(0)
        .push(0)
        .push(0)
        .call("USER", SEND_MESSAGE);
    send_buffer(&mut p, CB_GETLBTEXT16, 0);
    expect_ax(&mut p.code, 17, 4);
    expect_buffer(&mut p.code, b"FEQ01 - fractions\0", 5);
    send_buffer(&mut p, CB_GETLBTEXT16, 1);
    expect_ax(&mut p.code, 5, 6);
    expect_buffer(&mut p.code, b"FLT01\0", 7);
    p.code.emit(&[0xB0, 0x01]).exit_al();
    pass_on_procedure(&mut p, proc_at);
    let (exit, log) = run("combobox_subclass_strings", p.build());
    assert_eq!(exit, 1, "{log}");
}

#[test]
fn a_subclassed_edit_control_replaces_and_gives_back_a_line() {
    // EM_REPLACESEL's string and EM_GETLINE's buffer, whose first word
    // is its size, through a 16-bit window procedure: the buffer gets
    // what the edit control writes, the line and a NUL after it where
    // the buffer has room, and keeps its other bytes. Exit code 1, or
    // which check failed.
    const EM_REPLACESEL16: u16 = 0x0412;
    const EM_GETLINE16: u16 = 0x0414;
    let mut p = Program::default();
    p.create_window(b"EDIT\0", POPUP, [0, 0, 200, 20], false, 0);
    p.code.emit(&[0x8B, 0xF0]); // mov si,ax
    let proc_at = subclass(&mut p);
    send_string(&mut p, EM_REPLACESEL16, 0, b"hello!\0");
    // The buffer: 16 bytes, and 'xx' after where the line ends, of
    // which the NUL replaces the first.
    let [at_lo, at_hi] = BUFFER.to_le_bytes();
    p.code
        .emit(&[0xC7, 0x06, at_lo, at_hi, 16, 0]) // mov word [BUFFER],16
        .emit(&[0xC7, 0x06, at_lo + 6, at_hi, b'x', b'x']);
    send_buffer(&mut p, EM_GETLINE16, 0);
    expect_ax(&mut p.code, 6, 2);
    expect_buffer(&mut p.code, b"hello!\0x", 3);
    // A size of 0 copies nothing.
    p.code.emit(&[0xC7, 0x06, at_lo, at_hi, 0, 0]); // mov word [BUFFER],0
    send_buffer(&mut p, EM_GETLINE16, 0);
    expect_ax(&mut p.code, 0, 4);
    expect_buffer(&mut p.code, b"\0\0llo!\0x", 5);
    p.code.emit(&[0xB0, 0x01]).exit_al();
    pass_on_procedure(&mut p, proc_at);
    let (exit, log) = run("edit_subclass_line", p.build());
    assert_eq!(exit, 1, "{log}");
}

#[test]
fn a_window_procedure_gets_the_stack_in_ds_and_its_instance_in_ax() {
    // A window whose instance is a global block of its own, as Delphi
    // makes a TMemo's edit control. USER calls its procedure with DS the
    // task's stack and AX the instance's data segment, so a procedure
    // that frees the block while it runs restores a DS that is still
    // there. The procedure notes a call with DS other than SS at 60h and
    // the last AX at 62h. Exit code 1, or which check failed.
    const CS_GLOBALCLASS: u16 = 0x4000;
    // GMEM_MOVEABLE | GMEM_ZEROINIT | GMEM_DDESHARE, Delphi's flags.
    const BLOCK_FLAGS: u16 = 0x2042;
    const WC: u8 = 0x20;
    const BAD_DS: u8 = 0x60;
    const LAST_AX: u8 = 0x62;
    const BLOCK: u8 = 0x64;
    let mut p = Program::default();
    p.code
        .push(BLOCK_FLAGS)
        .push(0)
        .push(0x100)
        .call("KERNEL", GLOBAL_ALLOC)
        .emit(&[0xA3, BLOCK, 0x00]) // mov [BLOCK],ax
        .emit(&[0x0B, 0xC0, 0x75, 0x06]) // or ax,ax; jnz over the next
        .emit(&[0xB0, 2, 0xB4, 0x4C, 0xCD, 0x21])
        .emit(&[0xC7, 0x06, WC, 0x00])
        .emit(&CS_GLOBALCLASS.to_le_bytes())
        .emit(&[0x89, 0x3E, WC + 10, 0x00]); // mov [WC+10],di: the instance
    let proc_at = p.code.here() + 4;
    p.code
        .emit(&[0xC7, 0x06, WC + 2, 0x00, 0x00, 0x00]) // its procedure
        .emit(&[0x8C, 0x0E, WC + 4, 0x00]); // mov [WC+4],cs
    let name_at = p.code.here() + 4;
    p.code
        .emit(&[0xC7, 0x06, WC + 22, 0x00, 0x00, 0x00]) // its name
        .emit(&[0x8C, 0x0E, WC + 24, 0x00])
        .emit(&[0x1E])
        .push(WC as u16)
        .call("USER", REGISTER_CLASS)
        .emit(&[0x0B, 0xC0, 0x75, 0x06])
        .emit(&[0xB0, 3, 0xB4, 0x4C, 0xCD, 0x21]);
    // CreateWindow(class, "", WS_POPUP, rect, no parent, no menu, the
    // block, no parameter).
    p.push_string(b"DSCHK\0");
    p.push_string(b"\0");
    p.code.push((POPUP >> 16) as u16).push(POPUP as u16);
    for v in [0, 0, 100, 60] {
        p.code.push(v);
    }
    p.code
        .push(0)
        .push(0)
        .emit(&[0xFF, 0x36, BLOCK, 0x00]) // push word [BLOCK]
        .push(0)
        .push(0)
        .call("USER", CREATE_WINDOW)
        .emit(&[0x0B, 0xC0, 0x75, 0x06])
        .emit(&[0xB0, 4, 0xB4, 0x4C, 0xCD, 0x21])
        .emit(&[0x80, 0x3E, BAD_DS, 0x00, 0x00]) // cmp byte [BAD_DS],0
        .emit(&[0x74, 0x06]) // je over the next
        .emit(&[0xB0, 5, 0xB4, 0x4C, 0xCD, 0x21])
        .emit(&[0xFF, 0x36, BLOCK, 0x00])
        .call("KERNEL", GLOBAL_LOCK)
        .emit(&[0x3B, 0x16, LAST_AX, 0x00]) // cmp dx,[LAST_AX]
        .emit(&[0x74, 0x06])
        .emit(&[0xB0, 6, 0xB4, 0x4C, 0xCD, 0x21])
        .emit(&[0xB0, 0x01])
        .exit_al();
    // The window procedure: DS and SS compared, AX kept, both through
    // SS (the program's data segment), then DefWindowProc.
    let proc = p.code.here();
    p.code
        .emit(&[0x8C, 0xDB, 0x8C, 0xD1]) // mov bx,ds; mov cx,ss
        .emit(&[0x39, 0xCB, 0x74, 0x06]) // cmp bx,cx; je over the next
        .emit(&[0x36, 0xC6, 0x06, BAD_DS, 0x00, 0x01]) // mov ss:[BAD_DS],1
        .emit(&[0x36, 0xA3, LAST_AX, 0x00]) // mov ss:[LAST_AX],ax
        .emit(&[0x55, 0x8B, 0xEC]) // push bp; mov bp,sp
        .emit(&[
            0xFF, 0x76, 0x0E, // push [bp+0Eh] (hwnd)
            0xFF, 0x76, 0x0C, // push [bp+0Ch] (msg)
            0xFF, 0x76, 0x0A, // push [bp+0Ah] (wParam)
            0xFF, 0x76, 0x08, // push [bp+8] (lParam)
            0xFF, 0x76, 0x06, // push [bp+6]
        ])
        .call("USER", DEF_WINDOW_PROC)
        .emit(&[0x5D, 0xCA, 0x0A, 0x00]); // pop bp; retf 0Ah
    p.code.patch(proc_at, proc);
    let name = p.code.here();
    p.code.emit(b"DSCHK\0");
    p.code.patch(name_at, name);
    let (exit, log) = run("wndproc_ds", p.build());
    assert_eq!(exit, 1, "{log}");
}

/// A program with a window of its own class, whose 16-bit procedure
/// answers WM_GETTEXT as Memphis Math's answer field does: it writes
/// "42" into lParam's buffer, notes the lParam it got at `GOT`, and
/// returns 0. The program sends WM_GETTEXT with its buffer, then calls
/// GetWindowText. With `hook`, a WH_CALLWNDPROC hook counts the
/// WM_GETTEXTs it sees at `HOOKED` first. Exit code 1, or which check
/// failed.
fn own_buffer_program(hook: bool) -> Vec<u8> {
    const WM_GETTEXT: u16 = 0x000D;
    const WH_CALLWNDPROC: u16 = 4;
    // The WNDCLASS at 40h; the lParam the procedure got, and the hook's
    // count.
    const WC: u8 = 0x40;
    const GOT: u8 = 0x60;
    const HOOKED: u8 = 0x64;
    let [buf_lo, buf_hi] = BUFFER.to_le_bytes();
    let mut p = Program::default();
    let mut hook_at = None;
    if hook {
        p.code.push(WH_CALLWNDPROC);
        hook_at = Some(p.code.here() + 2);
        p.code
            .push_far(0)
            .emit(&[0x57]) // push di (the instance)
            .push(0) // every task
            .call("USER", SET_WINDOWS_HOOK_EX)
            .emit(&[0x0B, 0xC2]); // or ax,dx
        exit_if_ax_zero(&mut p.code, 2);
    }
    p.code.emit(&[0x89, 0x3E, WC + 10, 0x00]); // mov [WC+10],di: the instance
    let proc_at = p.code.here() + 4;
    p.code
        .emit(&[0xC7, 0x06, WC + 2, 0x00, 0x00, 0x00]) // its procedure
        .emit(&[0x8C, 0x0E, WC + 4, 0x00]); // mov [WC+4],cs
    let name_at = p.code.here() + 4;
    p.code
        .emit(&[0xC7, 0x06, WC + 22, 0x00, 0x00, 0x00]) // its name
        .emit(&[0x8C, 0x0E, WC + 24, 0x00])
        .emit(&[0x1E])
        .push(WC as u16)
        .call("USER", REGISTER_CLASS);
    exit_if_ax_zero(&mut p.code, 3);
    p.create_window(b"ANSWER\0", POPUP, [0, 0, 100, 60], false, 0);
    p.code.emit(&[0x8B, 0xF0]); // mov si,ax
    exit_if_ax_zero(&mut p.code, 4);
    send_buffer(&mut p, WM_GETTEXT, 0x14);
    expect_ax(&mut p.code, 0, 5);
    expect_buffer(&mut p.code, b"42", 6);
    // The procedure got the caller's own buffer: DS:BUFFER.
    p.code
        .emit(&[0x81, 0x3E, GOT, 0x00, buf_lo, buf_hi]) // cmp [GOT],BUFFER
        .emit(&[0x74, 0x06])
        .emit(&[0xB0, 7, 0xB4, 0x4C, 0xCD, 0x21])
        .emit(&[0x8C, 0xD8]) // mov ax,ds
        .emit(&[0x3B, 0x06, GOT + 2, 0x00]) // cmp ax,[GOT+2]
        .emit(&[0x74, 0x06])
        .emit(&[0xB0, 8, 0xB4, 0x4C, 0xCD, 0x21]);
    if hook {
        p.code
            .emit(&[0x80, 0x3E, HOOKED, 0x00, 0x01]) // cmp byte [HOOKED],1
            .emit(&[0x74, 0x06])
            .emit(&[0xB0, 9, 0xB4, 0x4C, 0xCD, 0x21]);
    }
    // GetWindowText(SI, DS:BUFFER, 14h) sends WM_GETTEXT the same way.
    p.code
        .emit(&[0xC7, 0x06, buf_lo, buf_hi, 0x00, 0x00]) // mov word [BUFFER],0
        .emit(&[0xC7, 0x06, GOT, 0x00, 0x00, 0x00]) // mov word [GOT],0
        .emit(&[0x56]) // push si
        .emit(&[0x1E]) // push ds
        .push(BUFFER)
        .push(0x14)
        .call("USER", GET_WINDOW_TEXT);
    expect_ax(&mut p.code, 0, 10);
    expect_buffer(&mut p.code, b"42", 11);
    p.code
        .emit(&[0x81, 0x3E, GOT, 0x00, buf_lo, buf_hi]) // cmp [GOT],BUFFER
        .emit(&[0x74, 0x06])
        .emit(&[0xB0, 12, 0xB4, 0x4C, 0xCD, 0x21])
        .emit(&[0xB0, 0x01])
        .exit_al();
    // The window procedure: WM_GETTEXT answered, the rest to
    // DefWindowProc. DS and SS are the program's data segment.
    let proc = p.code.here();
    p.code
        .emit(&[0x55, 0x8B, 0xEC]) // push bp; mov bp,sp
        .emit(&[0x83, 0x7E, 0x0C, WM_GETTEXT as u8]) // cmp [bp+0Ch] (msg)
        .emit(&[0x75, 0x1F]) // jne other
        .emit(&[0xC4, 0x5E, 0x06]) // les bx,[bp+6] (lParam)
        .emit(&[0x36, 0x89, 0x1E, GOT, 0x00]) // mov ss:[GOT],bx
        .emit(&[0x36, 0x8C, 0x06, GOT + 2, 0x00]) // mov ss:[GOT+2],es
        .emit(&[0x26, 0xC7, 0x07, b'4', b'2']) // mov word es:[bx],'42'
        .emit(&[0x26, 0xC6, 0x47, 0x02, 0x00]) // mov byte es:[bx+2],0
        .emit(&[0x31, 0xC0, 0x31, 0xD2]) // xor ax,ax; xor dx,dx
        .emit(&[0x5D, 0xCA, 0x0A, 0x00]) // pop bp; retf 0Ah
        .emit(&[
            0xFF, 0x76, 0x0E, // other: push [bp+0Eh] (hwnd)
            0xFF, 0x76, 0x0C, // push [bp+0Ch] (msg)
            0xFF, 0x76, 0x0A, // push [bp+0Ah] (wParam)
            0xFF, 0x76, 0x08, // push [bp+8] (lParam)
            0xFF, 0x76, 0x06, // push [bp+6]
        ])
        .call("USER", DEF_WINDOW_PROC)
        .emit(&[0x5D, 0xCA, 0x0A, 0x00]); // pop bp; retf 0Ah
    p.code.patch(proc_at, proc);
    if let Some(at) = hook_at {
        // The hook (code, wParam, lParam): counts the WM_GETTEXTs in
        // the CWPSTRUCT16 (lParam, wParam, message, hwnd) it gets.
        let hook = p.code.here();
        p.code
            .emit(&[0x55, 0x8B, 0xEC]) // push bp; mov bp,sp
            .emit(&[0xC4, 0x5E, 0x06]) // les bx,[bp+6]
            .emit(&[0x26, 0x83, 0x7F, 0x06, WM_GETTEXT as u8]) // cmp es:[bx+6]
            .emit(&[0x75, 0x05]) // jne done
            .emit(&[0x36, 0xFE, 0x06, HOOKED, 0x00]) // inc byte ss:[HOOKED]
            .emit(&[0x31, 0xC0, 0x31, 0xD2]) // done: xor ax,ax; xor dx,dx
            .emit(&[0x5D, 0xCA, 0x08, 0x00]); // pop bp; retf 8
        p.code.patch(at, hook);
    }
    let name = p.code.here();
    p.code.emit(b"ANSWER\0");
    p.code.patch(name_at, name);
    p.build()
}

/// End with `fail` if AX is 0.
fn exit_if_ax_zero(code: &mut Code, fail: u8) {
    code.emit(&[0x0B, 0xC0, 0x75, 0x06]) // or ax,ax; jnz over the next
        .emit(&[0xB0, fail, 0xB4, 0x4C, 0xCD, 0x21]);
}

#[test]
fn send_message_gives_a_16_bit_window_procedure_the_callers_buffer() {
    // Windows 3.1's USER calls a 16-bit window procedure with the
    // message 16-bit code sends as it is, and Windows 95 does the same
    // between 16-bit code and a 16-bit procedure: the procedure writes
    // into the caller's own buffer, whatever it returns.
    let (exit, log) = run("own_buffer", own_buffer_program(false));
    assert_eq!(exit, 1, "{log}");
}

#[test]
fn a_call_window_proc_hook_sees_a_message_sent_to_a_16_bit_procedure() {
    // The WH_CALLWNDPROC hook runs before the procedure, once, and the
    // procedure still gets the caller's buffer.
    let (exit, log) = run("own_buffer_hook", own_buffer_program(true));
    assert_eq!(exit, 1, "{log}");
}

/// Point the short jump whose displacement is at `at` to `target`.
fn jump_here(code: &mut Code, at: u16, target: u16) {
    code.bytes[at as usize] = target.wrapping_sub(at + 1) as u8;
}

/// Register the class DIGITS (at DS:40h), whose window procedure
/// `digits_procedure` writes, and make a popup of it, in SI, and a
/// child of the popup with the id 5. End with 2, 3 or 4 if one fails.
/// Returns where the procedure's offset is to be patched in.
fn digits_windows(p: &mut Program) -> u16 {
    const WC: u8 = 0x40;
    p.code.emit(&[0x89, 0x3E, WC + 10, 0x00]); // mov [WC+10],di: the instance
    let proc_at = p.code.here() + 4;
    p.code
        .emit(&[0xC7, 0x06, WC + 2, 0x00, 0x00, 0x00]) // its procedure
        .emit(&[0x8C, 0x0E, WC + 4, 0x00]); // mov [WC+4],cs
    p.strings.push((p.code.here() + 4, b"DIGITS\0"));
    p.code
        .emit(&[0xC7, 0x06, WC + 22, 0x00, 0x00, 0x00]) // its name
        .emit(&[0x8C, 0x0E, WC + 24, 0x00])
        .emit(&[0x1E])
        .push(WC as u16)
        .call("USER", REGISTER_CLASS);
    exit_if_ax_zero(&mut p.code, 2);
    p.create_window(b"DIGITS\0", POPUP, [0, 0, 100, 60], false, 0);
    p.code.emit(&[0x8B, 0xF0]); // mov si,ax
    exit_if_ax_zero(&mut p.code, 3);
    p.create_window(b"DIGITS\0", CHILD, [0, 0, 50, 20], true, 5);
    exit_if_ax_zero(&mut p.code, 4);
    proc_at
}

/// The window procedure of DIGITS: WM_GETTEXT writes "1234" into
/// lParam's buffer and returns the word at SS:DIGITS_RETURN (in the
/// task's data segment), or 2 with `long`. With `long`, a WM_GETTEXT
/// for more than 255 characters fills the buffer with wParam - 1 B's
/// instead and returns their count, and WM_SETTEXT sets the word at
/// SS:DIGITS_SET and returns its string's length. WM_DESTROY quits; the
/// rest goes to DefWindowProc.
fn digits_procedure(p: &mut Program, long: bool) -> u16 {
    const WM_SETTEXT: u8 = 0x0C;
    const WM_GETTEXT: u8 = 0x0D;
    const WM_DESTROY: u8 = 0x02;
    let proc = p.code.here();
    p.code
        .emit(&[0x55, 0x8B, 0xEC, 0x57]) // push bp; mov bp,sp; push di
        .emit(&[0x8B, 0x46, 0x0C]) // mov ax,[bp+0Ch] (msg)
        .emit(&[0x3D, WM_GETTEXT, 0x00, 0x74, 0x00]); // cmp ax; je gettext
    let to_get = p.code.here() - 1;
    p.code.emit(&[0x3D, WM_SETTEXT, 0x00, 0x74, 0x00]); // je settext
    let to_set = p.code.here() - 1;
    p.code.emit(&[0x3D, WM_DESTROY, 0x00, 0x74, 0x00]); // je destroy
    let to_destroy = p.code.here() - 1;
    // Other messages, and WM_SETTEXT without `long`.
    let other = p.code.here();
    p.code
        .emit(&[
            0xFF, 0x76, 0x0E, // push [bp+0Eh] (hwnd)
            0xFF, 0x76, 0x0C, // push [bp+0Ch] (msg)
            0xFF, 0x76, 0x0A, // push [bp+0Ah] (wParam)
            0xFF, 0x76, 0x08, // push [bp+8] (lParam)
            0xFF, 0x76, 0x06, // push [bp+6]
        ])
        .call("USER", DEF_WINDOW_PROC)
        .emit(&[0xEB, 0x00]); // jmp done
    let mut to_done = vec![p.code.here() - 1];
    let here = p.code.here();
    jump_here(&mut p.code, to_destroy, here);
    p.code
        .push(0)
        .call("USER", POST_QUIT_MESSAGE)
        .emit(&[0x31, 0xC0, 0x31, 0xD2]) // xor ax,ax; xor dx,dx
        .emit(&[0xEB, 0x00]);
    to_done.push(p.code.here() - 1);
    let here = p.code.here();
    jump_here(&mut p.code, to_get, here);
    p.code.emit(&[0xC4, 0x7E, 0x06]); // les di,[bp+6] (lParam)
    if long {
        p.code
            .emit(&[0x8B, 0x4E, 0x0A]) // mov cx,[bp+0Ah] (wParam)
            .emit(&[0x81, 0xF9, 0x00, 0x01, 0x72, 0x00]); // cmp cx,100h; jb short
        let to_short = p.code.here() - 1;
        p.code
            .emit(&[0x49, 0x51]) // dec cx; push cx
            .emit(&[0xB0, b'B', 0xFC, 0xF3, 0xAA]) // mov al,'B'; cld; rep stosb
            .emit(&[0x26, 0xC6, 0x05, 0x00]) // mov byte es:[di],0
            .emit(&[0x58, 0x31, 0xD2]) // pop ax; xor dx,dx
            .emit(&[0xEB, 0x00]);
        to_done.push(p.code.here() - 1);
        let here = p.code.here();
        jump_here(&mut p.code, to_short, here);
    }
    p.code
        .emit(&[0x26, 0xC7, 0x05, b'1', b'2']) // mov word es:[di],'12'
        .emit(&[0x26, 0xC7, 0x45, 0x02, b'3', b'4']) // mov word es:[di+2],'34'
        .emit(&[0x26, 0xC6, 0x45, 0x04, 0x00]); // mov byte es:[di+4],0
    if long {
        p.code.emit(&[0xB8, 0x02, 0x00]); // mov ax,2
    } else {
        p.code.emit(&[0x36, 0xA1, DIGITS_RETURN, 0x00]); // mov ax,ss:[DIGITS_RETURN]
    }
    p.code.emit(&[0x31, 0xD2, 0xEB, 0x00]); // xor dx,dx; jmp done
    to_done.push(p.code.here() - 1);
    let here = p.code.here();
    jump_here(&mut p.code, to_set, here);
    if long {
        p.code
            .emit(&[0x36, 0xC7, 0x06, DIGITS_SET, 0x00, 0x01, 0x00]) // mov word ss:[DIGITS_SET],1
            .emit(&[0xC4, 0x7E, 0x06]) // les di,[bp+6]
            .emit(&[0x31, 0xC0, 0xB9, 0xFF, 0xFF]) // xor ax,ax; mov cx,0FFFFh
            .emit(&[0xFC, 0xF2, 0xAE]) // cld; repne scasb
            .emit(&[0xF7, 0xD1, 0x49]) // not cx; dec cx: the length
            .emit(&[0x8B, 0xC1, 0x31, 0xD2]) // mov ax,cx; xor dx,dx
            .emit(&[0xEB, 0x00]);
        to_done.push(p.code.here() - 1);
    } else {
        let back = other.wrapping_sub(p.code.here() + 2) as u8;
        p.code.emit(&[0xEB, back]); // jmp other
    }
    let done = p.code.here();
    p.code.emit(&[0x5F, 0x5D, 0xCA, 0x0A, 0x00]); // pop di; pop bp; retf 0Ah
    for at in to_done {
        jump_here(&mut p.code, at, done);
    }
    proc
}

/// Where the DIGITS procedure of `digits_procedure` without `long`
/// finds what it returns for WM_GETTEXT.
const DIGITS_RETURN: u8 = 0x60;

/// The word the DIGITS procedure of `digits_procedure` with `long` sets
/// to 1 when it gets WM_SETTEXT.
const DIGITS_SET: u8 = 0x66;

#[test]
fn get_dlg_item_int_reads_what_a_16_bit_control_writes() {
    // Windows 98's 16-bit GetDlgItemInt sends WM_GETTEXT as SendMessage
    // does, and takes the number from its buffer when the procedure
    // returns a length other than 0: "1234" with a length of 2 is 1234,
    // with a length of 0 no number. Exit code 1, or which check failed.
    const TRANSLATED: u8 = 0x62;
    let mut p = Program::default();
    let proc_at = digits_windows(&mut p);
    for (ret, value, translated, fail) in [(2u16, 1234u16, 1u8, 5u8), (0, 0, 0, 7)] {
        let [ret_lo, ret_hi] = ret.to_le_bytes();
        p.code
            .emit(&[0xC7, 0x06, DIGITS_RETURN, 0x00, ret_lo, ret_hi])
            .emit(&[0xC7, 0x06, TRANSLATED, 0x00, 0xEF, 0xBE]) // 0BEEFh
            .emit(&[0x56]) // push si
            .push(5)
            .emit(&[0x1E]) // push ds
            .push(TRANSLATED as u16)
            .push(0) // unsigned
            .call("USER", GET_DLG_ITEM_INT);
        expect_ax(&mut p.code, value, fail);
        p.code
            .emit(&[0x83, 0x3E, TRANSLATED, 0x00, translated]) // cmp word [TRANSLATED]
            .emit(&[0x74, 0x06])
            .emit(&[0xB0, fail + 1, 0xB4, 0x4C, 0xCD, 0x21]);
    }
    p.code.emit(&[0xB0, 0x01]).exit_al();
    let proc = digits_procedure(&mut p, false);
    p.code.patch(proc_at, proc);
    let (exit, log) = run("get_dlg_item_int_16", p.build());
    assert_eq!(exit, 1, "{log}");
}

/// A program that starts SRV.EXE (`intertask_server`), finds its DIGITS
/// window and sends it messages: WM_GETTEXT with its buffer, which gets
/// "1234" and 2; GetWindowText; GetDlgItemInt of the window's child;
/// WM_GETTEXT for 3000 bytes in a global block, which gets 2999 B's;
/// and WM_SETTEXT with those, whose length comes back. Then it closes
/// the window and ends with 1, or with which check failed.
fn intertask_client() -> Vec<u8> {
    const WM_SETTEXT: u16 = 0x0C;
    const WM_GETTEXT: u16 = 0x0D;
    const WM_CLOSE: u16 = 0x10;
    const TRANSLATED: u8 = 0x40;
    const BIG: u8 = 0x44;
    let [buf_lo, buf_hi] = BUFFER.to_le_bytes();
    let [buf2_lo, buf2_hi] = (BUFFER + 2).to_le_bytes();
    let mut p = Program::default();
    let mut fails = Vec::new();
    // Go on if the short jump `jcc` is taken, else end with `fail`.
    let check = |p: &mut Program, fails: &mut Vec<u16>, jcc: u8, fail: u8| {
        p.code.emit(&[jcc, 0x05, 0xB3, fail, 0xE9]); // jcc; mov bl,fail; jmp
        fails.push(p.code.here());
        p.code.emit(&[0x00, 0x00]);
    };
    p.code
        .push(0x40) // GMEM_ZEROINIT
        .push(0)
        .push(0x1000)
        .call("KERNEL", GLOBAL_ALLOC);
    exit_if_ax_zero(&mut p.code, 2);
    p.code
        .emit(&[0x50]) // push ax
        .call("KERNEL", GLOBAL_LOCK)
        .emit(&[0xA3, BIG, 0x00]) // mov [BIG],ax
        .emit(&[0x89, 0x16, BIG + 2, 0x00]); // mov [BIG+2],dx
    p.push_string(b"C:\\SRV.EXE\0");
    p.code
        .push(1)
        .call("KERNEL", WIN_EXEC)
        .emit(&[0x3D, 0x20, 0x00, 0x73, 0x06]) // cmp ax,20h; jae over
        .emit(&[0xB0, 0x03, 0xB4, 0x4C, 0xCD, 0x21]);
    // Until SRV's window is there: FindWindow("DIGITS", NULL), Yield.
    let find = p.code.here();
    p.push_string(b"DIGITS\0");
    p.code
        .push(0)
        .push(0)
        .call("USER", FIND_WINDOW)
        .emit(&[0x0B, 0xC0, 0x75, 0x08]) // or ax,ax; jnz found
        .call("KERNEL", YIELD);
    let back = find.wrapping_sub(p.code.here() + 3);
    p.code.emit(&[0xE9]).emit(&back.to_le_bytes());
    p.code.emit(&[0x8B, 0xF0]); // found: mov si,ax

    // SendMessage(SI, WM_GETTEXT, 14h, DS:BUFFER).
    send_buffer(&mut p, WM_GETTEXT, 0x14);
    p.code.emit(&[0x3D, 0x02, 0x00]); // cmp ax,2
    check(&mut p, &mut fails, 0x74, 4);
    p.code.emit(&[0x81, 0x3E, buf_lo, buf_hi, b'1', b'2']);
    check(&mut p, &mut fails, 0x74, 5);
    p.code.emit(&[0x81, 0x3E, buf2_lo, buf2_hi, b'3', b'4']);
    check(&mut p, &mut fails, 0x74, 6);

    // GetWindowText(SI, DS:BUFFER, 14h).
    p.code
        .emit(&[0xC7, 0x06, buf_lo, buf_hi, 0x00, 0x00])
        .emit(&[0xC7, 0x06, buf2_lo, buf2_hi, 0x00, 0x00])
        .emit(&[0x56, 0x1E])
        .push(BUFFER)
        .push(0x14)
        .call("USER", GET_WINDOW_TEXT)
        .emit(&[0x3D, 0x02, 0x00]);
    check(&mut p, &mut fails, 0x74, 7);
    p.code.emit(&[0x81, 0x3E, buf_lo, buf_hi, b'1', b'2']);
    check(&mut p, &mut fails, 0x74, 8);
    p.code.emit(&[0x81, 0x3E, buf2_lo, buf2_hi, b'3', b'4']);
    check(&mut p, &mut fails, 0x74, 9);

    // GetDlgItemInt(SI, 5, DS:TRANSLATED, FALSE).
    p.code
        .emit(&[0x56])
        .push(5)
        .emit(&[0x1E])
        .push(TRANSLATED as u16)
        .push(0)
        .call("USER", GET_DLG_ITEM_INT)
        .emit(&[0x3D, 0xD2, 0x04]); // cmp ax,1234
    check(&mut p, &mut fails, 0x74, 10);
    p.code.emit(&[0x83, 0x3E, TRANSLATED, 0x00, 0x01]);
    check(&mut p, &mut fails, 0x74, 11);

    // SendMessage(SI, WM_GETTEXT, 3000, the block).
    p.code
        .emit(&[0x56])
        .push(WM_GETTEXT)
        .push(3000)
        .emit(&[0xFF, 0x36, BIG + 2, 0x00, 0xFF, 0x36, BIG, 0x00])
        .call("USER", SEND_MESSAGE)
        .emit(&[0x3D, 0xB7, 0x0B]); // cmp ax,2999
    check(&mut p, &mut fails, 0x74, 12);
    p.code
        .emit(&[0xC4, 0x1E, BIG, 0x00]) // les bx,[BIG]
        .emit(&[0x26, 0x80, 0xBF, 0xB6, 0x0B, b'B']); // cmp byte es:[bx+2998]
    check(&mut p, &mut fails, 0x74, 13);
    p.code.emit(&[0x26, 0x80, 0xBF, 0xB7, 0x0B, 0x00]); // es:[bx+2999]
    check(&mut p, &mut fails, 0x74, 14);

    // SendMessage(SI, WM_SETTEXT, 0, the block's 2999 B's).
    p.code
        .emit(&[0x56])
        .push(WM_SETTEXT)
        .push(0)
        .emit(&[0xFF, 0x36, BIG + 2, 0x00, 0xFF, 0x36, BIG, 0x00])
        .call("USER", SEND_MESSAGE)
        .emit(&[0x3D, 0xB7, 0x0B]);
    check(&mut p, &mut fails, 0x74, 15);
    p.code.emit(&[0xB3, 0x01]); // mov bl,1

    // The end: PostMessage(SI, WM_CLOSE, 0, 0), and BL the exit code.
    let end = p.code.here();
    for at in fails {
        p.code.patch(at, end.wrapping_sub(at + 2));
    }
    p.code
        .emit(&[0x53, 0x56]) // push bx; push si
        .push(WM_CLOSE)
        .push(0)
        .push(0)
        .push(0)
        .call("USER", POST_MESSAGE)
        .emit(&[0x5B, 0x8A, 0xC3]) // pop bx; mov al,bl
        .exit_al();
    p.build()
}

/// How SRV.EXE (`intertask_server`) waits for the client's messages.
#[derive(Clone, Copy, PartialEq)]
enum Waits {
    /// In GetMessage, dispatching what it gets.
    GetMessage,
    /// In one Yield, then it ends with 34h if its procedure got
    /// WM_SETTEXT by then, else with 35h.
    Yield,
    /// It takes and dispatches what its queue holds with PeekMessage, and
    /// then calls WaitMessage until its procedure got WM_SETTEXT, and ends
    /// with 34h.
    WaitMessage,
    /// In one PeekMessage without PM_NOYIELD for WM_CLOSE only, which
    /// finds nothing and yields. The client posts WM_CLOSE during that
    /// yield, after its sends. Then it ends with 34h if PeekMessage
    /// returned FALSE and its procedure got WM_SETTEXT by then, 35h if
    /// it hadn't, 36h if PeekMessage returned TRUE.
    Peek,
}

/// SRV.EXE: the DIGITS windows of `digits_windows`, whose procedure
/// answers long texts too (`digits_procedure`), waiting as `waits` says.
/// With a message loop it ends with 33h when its window is destroyed.
fn intertask_server(waits: Waits) -> Vec<u8> {
    const MSG: u16 = 0x70;
    let mut p = Program::default();
    let proc_at = digits_windows(&mut p);
    if waits == Waits::Yield {
        p.code
            .call("KERNEL", YIELD)
            .emit(&[0x83, 0x3E, DIGITS_SET, 0x00, 0x00]) // cmp word [DIGITS_SET],0
            .emit(&[0xB0, 0x34, 0x75, 0x02, 0xB0, 0x35]) // mov al,34h; jne; mov al,35h
            .exit_al();
    }
    if waits == Waits::Peek {
        const WM_CLOSE: u16 = 0x10;
        p.code
            .emit(&[0x1E])
            .push(MSG)
            .push(0)
            .push(WM_CLOSE)
            .push(WM_CLOSE)
            .push(1) // PM_REMOVE
            .call("USER", PEEK_MESSAGE)
            .emit(&[0x0B, 0xC0, 0xB0, 0x36, 0x75, 0x0B]) // or ax,ax; mov al,36h; jnz end
            .emit(&[0x83, 0x3E, DIGITS_SET, 0x00, 0x00]) // cmp word [DIGITS_SET],0
            .emit(&[0xB0, 0x34, 0x75, 0x02, 0xB0, 0x35]) // mov al,34h; jne end; mov al,35h
            .exit_al();
    }
    if waits == Waits::WaitMessage {
        // WaitMessage returns at once while messages wait, as the
        // window's first WM_PAINT does.
        let peek = p.code.here();
        p.code
            .emit(&[0x1E])
            .push(MSG)
            .push(0)
            .push(0)
            .push(0)
            .push(1) // PM_REMOVE
            .call("USER", PEEK_MESSAGE)
            .emit(&[0x0B, 0xC0, 0x74, 0x0B]) // or ax,ax; jz waits
            .emit(&[0x1E])
            .push(MSG)
            .call("USER", DISPATCH_MESSAGE);
        let back = peek.wrapping_sub(p.code.here() + 2) as u8;
        p.code.emit(&[0xEB, back]); // jmp peek
        let again = p.code.here();
        p.code
            .call("USER", WAIT_MESSAGE)
            .emit(&[0x83, 0x3E, DIGITS_SET, 0x00, 0x00]); // cmp word [DIGITS_SET],0
        let back = again.wrapping_sub(p.code.here() + 2) as u8;
        p.code.emit(&[0x74, back]); // je again
        p.code.emit(&[0xB0, 0x34]).exit_al();
    }
    let get = p.code.here();
    p.code
        .emit(&[0x1E])
        .push(MSG)
        .push(0)
        .push(0)
        .push(0)
        .call("USER", GET_MESSAGE)
        .emit(&[0x0B, 0xC0, 0x74, 0x0C]) // or ax,ax; jz quit
        .emit(&[0x1E])
        .push(MSG)
        .call("USER", DISPATCH_MESSAGE);
    let back = get.wrapping_sub(p.code.here() + 3);
    p.code.emit(&[0xE9]).emit(&back.to_le_bytes());
    p.code.emit(&[0xB0, 0x33]).exit_al(); // quit
    let proc = digits_procedure(&mut p, true);
    p.code.patch(proc_at, proc);
    p.build_named("SRV")
}

/// Whether a task ended with the exit code `code`, as the log says.
fn ended_with(log: &str, code: u8) -> bool {
    ended_at(log, code).is_some()
}

/// The line of the log that says a task ended with the exit code `code`.
fn ended_at(log: &str, code: u8) -> Option<usize> {
    let line = format!(" ended with exit code {:X}", code);
    log.lines()
        .position(|l| l.starts_with("[W16] Task ") && l.ends_with(&line))
}

#[test]
fn a_message_to_another_tasks_window_reaches_its_procedure_as_it_is() {
    // Windows 3.1 switches to the task whose window it is and calls its
    // procedure with the sender's wParam and lParam, and Windows 98's
    // 16-bit USER queues the message for that task untranslated: the
    // procedure writes into the sender's own buffer, all of it, whatever
    // length it returns, and gets the sender's string whole.
    let (_, log) = run_with("intertask", intertask_client(), |_, base| {
        std::fs::write(base.join("SRV.EXE"), intertask_server(Waits::GetMessage)).unwrap();
    });
    assert!(ended_with(&log, 1), "{log}");
    assert!(ended_with(&log, 0x33), "{log}");
}

#[test]
fn a_task_that_yields_receives_what_another_task_sends() {
    // Windows 3.1's Yield goes through USER's UserYield, which handles
    // the messages other tasks sent to the task (Wine's Yield16 calls
    // UserYield16). On Windows 95 a message sent while the task is in
    // Yield is handled there, and the reply lets the sender run on to its
    // end before Yield returns (#1913's measurements in 86Box): the
    // client's sends all reach the server's procedure during its one
    // Yield, and the client ends first.
    let (_, log) = run_with("intertask_yield", intertask_client(), |_, base| {
        std::fs::write(base.join("SRV.EXE"), intertask_server(Waits::Yield)).unwrap();
    });
    assert!(ended_with(&log, 1), "{log}");
    assert!(ended_with(&log, 0x34), "{log}");
    assert!(ended_at(&log, 1) < ended_at(&log, 0x34), "{log}");
}

#[test]
fn a_task_that_peeks_receives_what_another_task_sends_while_it_yields() {
    // A PeekMessage without PM_NOYIELD that finds no message yields
    // through UserYield, as Yield does. On Windows 95 a message another
    // task sends meanwhile is handled, the reply lets the sender run on
    // to its end, and PeekMessage then returns FALSE without looking at
    // the queue again (QPEEK's measurements in 86Box, #1914): the
    // client's sends all reach the server's procedure during its one
    // PeekMessage, the client ends first, and the WM_CLOSE it posted
    // meanwhile isn't returned.
    let (_, log) = run_with("intertask_peek", intertask_client(), |_, base| {
        std::fs::write(base.join("SRV.EXE"), intertask_server(Waits::Peek)).unwrap();
    });
    assert!(ended_with(&log, 1), "{log}");
    assert!(ended_with(&log, 0x34), "{log}");
    assert!(ended_at(&log, 1) < ended_at(&log, 0x34), "{log}");
}

#[test]
fn a_task_that_waits_for_a_message_receives_what_another_task_sends() {
    // Windows' WaitMessage handles the messages other tasks sent to the
    // task while it waits, as GetMessage does, and waits on: a server
    // that only calls WaitMessage answers the client's sends.
    let (_, log) = run_with("intertask_wait", intertask_client(), |_, base| {
        let server = intertask_server(Waits::WaitMessage);
        std::fs::write(base.join("SRV.EXE"), server).unwrap();
    });
    assert!(ended_with(&log, 1), "{log}");
    assert!(ended_with(&log, 0x34), "{log}");
}

#[test]
fn exit_windows_exec_runs_the_dos_program_after_windows() {
    // ExitWindowsExec("C:\D.COM", "/X") ends Windows, and D.COM then
    // runs at the prompt with "/X" as its command line: it ends with 5Ah
    // when it got it, 5Bh when not. Should the call come back, T.EXE
    // ends with 9.
    let mut code = Code::new();
    code.push_far(0);
    let exe_at = code.here() - 2;
    code.push_far(0);
    let params_at = code.here() - 2;
    code.call("USER", EXIT_WINDOWS_EXEC)
        .emit(&[0xB0, 0x09]) // mov al,9
        .exit_al();
    let exe = code.here();
    code.emit(b"C:\\D.COM\0");
    code.patch(exe_at, exe);
    let params = code.here();
    code.emit(b"/X\0");
    code.patch(params_at, params);
    let (exit, log) = run_with_prompts("exit_windows_exec", program(&code, CODE), 2, |_, dir| {
        std::fs::write(dir.join("D.COM"), D_COM).unwrap();
    });
    assert_eq!(exit, 0x5A, "{log}");
}

/// The DOS program ExitWindowsExec runs in these tests, C:\D.COM: it
/// ends with 5Ah when its command line is "/X", else with 5Bh.
const D_COM: [u8; 15] = [
    0x81, 0x3E, 0x82, 0x00, 0x2F, 0x58, // cmp word [82h],"/X"
    0xB8, 0x5A, 0x4C, // mov ax,4C5Ah
    0x74, 0x02, // je +2
    0xB0, 0x5B, // mov al,5Bh
    0xCD, 0x21, // int 21h
];

/// A program whose top-level window, of the class NOEND, refuses
/// WM_QUERYENDSESSION when its lParam is 0, as Windows 3.1's and
/// Windows 98's are for a 16-bit ExitWindows. It ends with 2 or 3 when
/// the class or the window can't be made. The code goes on after the
/// window is made.
fn refusing_window() -> Program {
    const WM_QUERYENDSESSION: u8 = 0x11;
    const WC: u8 = 0x20;
    let mut p = Program::default();
    p.code.emit(&[0x89, 0x3E, WC + 10, 0x00]); // mov [WC+10],di: the instance
    let proc_at = p.code.here() + 4;
    p.code
        .emit(&[0xC7, 0x06, WC + 2, 0x00, 0x00, 0x00]) // its procedure
        .emit(&[0x8C, 0x0E, WC + 4, 0x00]); // mov [WC+4],cs
    let name_at = p.code.here() + 4;
    p.code
        .emit(&[0xC7, 0x06, WC + 22, 0x00, 0x00, 0x00]) // its name
        .emit(&[0x8C, 0x0E, WC + 24, 0x00])
        .emit(&[0x1E])
        .push(WC as u16)
        .call("USER", REGISTER_CLASS)
        .emit(&[0x0B, 0xC0, 0x75, 0x06])
        .emit(&[0xB0, 2, 0xB4, 0x4C, 0xCD, 0x21]);
    p.push_string(b"NOEND\0");
    p.push_string(b"\0");
    p.code.push((POPUP >> 16) as u16).push(POPUP as u16);
    for v in [0, 0, 100, 60] {
        p.code.push(v);
    }
    p.code
        .push(0)
        .push(0)
        .emit(&[0x57]) // push di: the instance
        .push(0)
        .push(0)
        .call("USER", CREATE_WINDOW)
        .emit(&[0x0B, 0xC0, 0x75, 0x06])
        .emit(&[0xB0, 3, 0xB4, 0x4C, 0xCD, 0x21]);
    let go_on = p.code.here() + 1;
    p.code.emit(&[0xE9, 0x00, 0x00]); // jmp over the procedure
    // The window procedure: 0 for WM_QUERYENDSESSION with lParam 0,
    // DefWindowProc's answer for the rest.
    let proc = p.code.here();
    p.code
        .emit(&[0x55, 0x8B, 0xEC]) // push bp; mov bp,sp
        .emit(&[0x83, 0x7E, 0x0C, WM_QUERYENDSESSION]) // cmp word [bp+0Ch],11h
        .emit(&[0x75, 0x10]) // jne default
        .emit(&[0x8B, 0x46, 0x06]) // mov ax,[bp+6]
        .emit(&[0x0B, 0x46, 0x08]) // or ax,[bp+8]
        .emit(&[0x75, 0x08]) // jnz default
        .emit(&[0x31, 0xC0, 0x31, 0xD2]) // xor ax,ax; xor dx,dx
        .emit(&[0x5D, 0xCA, 0x0A, 0x00]) // pop bp; retf 0Ah
        .emit(&[
            0xFF, 0x76, 0x0E, // default: push [bp+0Eh] (hwnd)
            0xFF, 0x76, 0x0C, // push [bp+0Ch] (msg)
            0xFF, 0x76, 0x0A, // push [bp+0Ah] (wParam)
            0xFF, 0x76, 0x08, // push [bp+8] (lParam)
            0xFF, 0x76, 0x06, // push [bp+6]
        ])
        .call("USER", DEF_WINDOW_PROC)
        .emit(&[0x5D, 0xCA, 0x0A, 0x00]); // pop bp; retf 0Ah
    p.code.patch(proc_at, proc);
    let name = p.code.here();
    p.code.emit(b"NOEND\0");
    p.code.patch(name_at, name);
    let here = p.code.here();
    p.code.patch(go_on, here.wrapping_sub(go_on + 2));
    p
}

#[test]
fn exit_windows_exec_does_not_ask_the_calling_tasks_windows() {
    // Windows 98 leaves the calling task's windows out of a 16-bit
    // ExitWindows: a probe whose own window refuses WM_QUERYENDSESSION
    // calls ExitWindows(0, 0) on Windows 98 SE, and its window gets no
    // message while Windows shuts down (#1542). Microsoft Test's frame
    // refuses while its script runs, and Microsoft Setup's scripts call
    // ExitWindowsExec from that script to restart Windows.
    //
    // T.EXE's own window refuses, and ExitWindowsExec("C:\D.COM", "/X")
    // ends Windows all the same: D.COM runs and ends with 5Ah. 7 is a
    // call that returned FALSE, 9 one that returned TRUE.
    let mut p = refusing_window();
    p.push_string(b"C:\\D.COM\0");
    p.push_string(b"/X\0");
    p.code
        .call("USER", EXIT_WINDOWS_EXEC)
        .emit(&[0x0B, 0xC0, 0x74, 0x04]) // or ax,ax; jz refused
        .emit(&[0xB0, 0x09, 0xEB, 0x02]) // mov al,9; jmp done
        .emit(&[0xB0, 0x07]) // refused: mov al,7
        .exit_al();
    let (exit, log) = run_with_prompts("exit_windows_exec_own", p.build(), 2, |_, dir| {
        std::fs::write(dir.join("D.COM"), D_COM).unwrap();
    });
    assert_eq!(exit, 0x5A, "{log}");
}

/// T.EXE: the window of `refusing_window`, then it starts C:\X.EXE and
/// yields until X has ended, and ends with 20h (4 when X didn't start).
fn refusing_task() -> Vec<u8> {
    const X: u8 = 0x50;
    let mut p = refusing_window();
    p.push_string(b"C:\\X.EXE\0");
    p.code
        .push(1)
        .call("KERNEL", WIN_EXEC)
        .emit(&[0x3D, 0x20, 0x00, 0x73, 0x06]) // cmp ax,20h; jae started
        .emit(&[0xB0, 0x04, 0xB4, 0x4C, 0xCD, 0x21])
        .emit(&[0xA3, X, 0x00]); // started: mov [X],ax
    let wait = p.code.here();
    p.code
        .call("KERNEL", YIELD)
        .emit(&[0xFF, 0x36, X, 0x00]) // push word [X]
        .call("KERNEL", GET_MODULE_USAGE)
        .emit(&[0x0B, 0xC0]); // or ax,ax
    let back = wait.wrapping_sub(p.code.here() + 2) as u8;
    p.code.emit(&[0x75, back]); // jnz wait
    p.code.emit(&[0xB0, 0x20]).exit_al();
    p.build()
}

/// X.EXE: `call` pushes its arguments and calls ExitWindows or
/// ExitWindowsExec; X ends with 7 when the call returned FALSE, 9 when
/// it returned TRUE.
fn ending_task(call: impl FnOnce(&mut Program)) -> Vec<u8> {
    let mut p = Program::default();
    call(&mut p);
    p.code
        .emit(&[0x0B, 0xC0, 0x74, 0x04]) // or ax,ax; jz refused
        .emit(&[0xB0, 0x09, 0xEB, 0x02]) // mov al,9; jmp done
        .emit(&[0xB0, 0x07]) // refused: mov al,7
        .exit_al();
    p.build_named("X")
}

#[test]
fn exit_windows_exec_runs_nothing_when_another_tasks_window_refuses() {
    // T.EXE's window refuses WM_QUERYENDSESSION, so X.EXE's
    // ExitWindowsExec("C:\D.COM", "/X") returns FALSE and X ends with 7.
    // The command line must not be left to run at the prompt.
    let x = ending_task(|p| {
        p.push_string(b"C:\\D.COM\0");
        p.push_string(b"/X\0");
        p.code.call("USER", EXIT_WINDOWS_EXEC);
    });
    let (exit, log) = run_with("exit_windows_exec_refused", refusing_task(), |_, dir| {
        std::fs::write(dir.join("X.EXE"), x).unwrap();
        std::fs::write(dir.join("D.COM"), D_COM).unwrap();
    });
    assert_eq!(exit, 0x20, "{log}");
    assert!(ended_with(&log, 7), "{log}");
    assert!(!log.contains("ExitWindowsExec: DOS runs"), "{log}");
}

#[test]
fn exit_windows_asks_another_tasks_window_with_lparam_0() {
    // ExitWindows(0, 0) from a 16-bit task asks the other tasks' windows
    // with lParam 0, as Windows 98 SE does (#1542's probe): T.EXE's
    // window refuses only then, and X.EXE's call returns FALSE.
    let x = ending_task(|p| {
        p.code.push(0).push(0).push(0).call("USER", EXIT_WINDOWS);
    });
    let (exit, log) = run_with("exit_windows_refused", refusing_task(), |_, dir| {
        std::fs::write(dir.join("X.EXE"), x).unwrap();
    });
    assert_eq!(exit, 0x20, "{log}");
    assert!(ended_with(&log, 7), "{log}");
}

#[test]
fn exit_windows_ends_the_program() {
    // ExitWindows(EW_RESTARTWINDOWS, 0) comes back only when a window
    // refuses, and then the exit code is 9.
    let mut code = Code::new();
    code.push(0)
        .push(EW_RESTARTWINDOWS)
        .push(0)
        .call("USER", EXIT_WINDOWS)
        .emit(&[0xB0, 0x09]) // mov al,9
        .exit_al();
    let (exit, log) = run("exit_windows", program(&code, CODE));
    assert_eq!(exit, 0, "{log}");
}

#[test]
fn a_multi_line_edit_keeps_what_was_drawn_on_it_below_its_lines() {
    // A multi-line edit control erases its background in WM_ERASEBKGND,
    // and its WM_PAINT draws only its lines: each one's band, out to the
    // right edge, in the background color, an empty first line too.
    // Windows 98's USER.EXE does that, and a program that draws on the
    // control between the erase and the WM_PAINT keeps what it drew below
    // the lines, as Memphis Math does with an objective's sample. Here
    // RedrawWindow erases the empty control and leaves WM_PAINT to come,
    // the program blackens it through GetDC, and UpdateWindow paints it.
    // Exit code 1, or which check failed.
    const GET_DC: u16 = 66;
    const RELEASE_DC: u16 = 68;
    const UPDATE_WINDOW: u16 = 124;
    const REDRAW_WINDOW: u16 = 290;
    const PAT_BLT: u16 = 29;
    const GET_PIXEL: u16 = 83;
    const ES_MULTILINE: u32 = 0x0004;
    const RDW_INVALIDATE_ERASE_ERASENOW: u16 = 0x0205;
    const BLACKNESS: u16 = 0x0042;
    let mut p = Program::default();
    p.create_window(b"EDIT\0", POPUP | ES_MULTILINE, [0, 0, 200, 100], false, 0);
    p.code.emit(&[0x8B, 0xF0]); // mov si,ax
    p.code.emit(&[0x56]).call("USER", UPDATE_WINDOW); // push si
    p.code
        .emit(&[0x56])
        .push(0) // no rectangle
        .push(0)
        .push(0) // no region
        .push(RDW_INVALIDATE_ERASE_ERASENOW)
        .call("USER", REDRAW_WINDOW);
    p.code.emit(&[0x56]).call("USER", GET_DC);
    p.code
        .emit(&[0x8B, 0xF8]) // mov di,ax
        .emit(&[0x57]) // push di
        .push(0)
        .push(0)
        .push(200)
        .push(100)
        .push(0) // the raster operation's high word
        .push(BLACKNESS)
        .call("GDI", PAT_BLT);
    p.code
        .emit(&[0x56, 0x57]) // push si, push di
        .call("USER", RELEASE_DC);
    p.code.emit(&[0x56]).call("USER", UPDATE_WINDOW);
    p.code.emit(&[0x56]).call("USER", GET_DC);
    p.code.emit(&[0x8B, 0xF8]); // mov di,ax
    // Below the first line: still black.
    p.code
        .emit(&[0x57])
        .push(100)
        .push(60)
        .call("GDI", GET_PIXEL);
    expect_ax(&mut p.code, 0, 2);
    p.code.emit(&[0x8B, 0xC2]); // mov ax,dx
    expect_ax(&mut p.code, 0, 3);
    // The empty first line: the window's color, white.
    p.code
        .emit(&[0x57])
        .push(100)
        .push(3)
        .call("GDI", GET_PIXEL);
    expect_ax(&mut p.code, 0xFFFF, 4);
    p.code.emit(&[0x8B, 0xC2]); // mov ax,dx
    expect_ax(&mut p.code, 0x00FF, 5);
    p.code.emit(&[0xB0, 0x01]).exit_al();
    let (exit, log) = run("edit_keeps_drawing", p.build());
    assert_eq!(exit, 1, "{log}");
}

/// The exit code `edit_colors_program` gives: the WM_CTLCOLOR types its
/// parent was asked for, bit 1 for CTLCOLOR_EDIT and bit 6 for
/// CTLCOLOR_STATIC; bit 0 if some of the text is black, bit 7 if some
/// is gray (COLOR_GRAYTEXT).
const ASKED_EDIT: u8 = 1 << 1;
const ASKED_STATIC: u8 = 1 << 6;
const BLACK_TEXT: u8 = 1 << 0;
const GRAY_TEXT: u8 = 1 << 7;

/// A program for the Windows `version` (0x030A, 0x0400) with an edit
/// control of the style `style` showing "WWWW", in a popup of its own
/// class whose procedure notes the type (lParam's high word) of each
/// WM_CTLCOLOR it gets and passes it on to DefWindowProc. The program
/// paints the edit with UpdateWindow, looks for black and gray pixels
/// through GetDC and GetPixel, and ends with the code ASKED_* and *_TEXT
/// say.
fn edit_colors_program(style: u32, version: u16) -> Vec<u8> {
    const SET_WINDOW_TEXT: u16 = 37;
    const GET_DC: u16 = 66;
    const UPDATE_WINDOW: u16 = 124;
    const GET_PIXEL: u16 = 83;
    const WM_CTLCOLOR: u8 = 0x19;
    // The WNDCLASS at 40h; the types asked for, the gray and black
    // pixels' counts, and where GetPixel looks.
    const WC: u8 = 0x40;
    const ASKED: u8 = 0x60;
    const GRAY: u8 = 0x62;
    const X: u8 = 0x64;
    const Y: u8 = 0x66;
    const BLACK: u8 = 0x68;
    let mut p = Program::default();
    p.code.emit(&[0x89, 0x3E, WC + 10, 0x00]); // mov [WC+10],di: the instance
    let proc_at = p.code.here() + 4;
    p.code
        .emit(&[0xC7, 0x06, WC + 2, 0x00, 0x00, 0x00]) // its procedure
        .emit(&[0x8C, 0x0E, WC + 4, 0x00]); // mov [WC+4],cs
    p.strings.push((p.code.here() + 4, b"COLORS\0"));
    p.code
        .emit(&[0xC7, 0x06, WC + 22, 0x00, 0x00, 0x00]) // its name
        .emit(&[0x8C, 0x0E, WC + 24, 0x00])
        .emit(&[0x1E])
        .push(WC as u16)
        .call("USER", REGISTER_CLASS);
    exit_if_ax_zero(&mut p.code, 2);
    p.create_window(b"COLORS\0", POPUP, [0, 0, 200, 100], false, 0);
    p.code.emit(&[0x8B, 0xF0]); // mov si,ax
    exit_if_ax_zero(&mut p.code, 3);
    p.create_window(b"EDIT\0", CHILD | style, [0, 0, 100, 40], true, 5);
    exit_if_ax_zero(&mut p.code, 4);
    p.code.emit(&[0x8B, 0xF0]); // mov si,ax
    p.code.emit(&[0x56]); // push si
    p.push_string(b"WWWW\0");
    p.code.call("USER", SET_WINDOW_TEXT);
    p.code.emit(&[0x56]).call("USER", UPDATE_WINDOW);
    p.code.emit(&[0x56]).call("USER", GET_DC);
    p.code.emit(&[0x8B, 0xF8]); // mov di,ax
    // Every pixel of the first 20 rows and 60 columns.
    p.code.emit(&[0xC7, 0x06, Y, 0x00, 0x00, 0x00]); // mov word [Y],0
    let rows = p.code.here();
    p.code.emit(&[0xC7, 0x06, X, 0x00, 0x00, 0x00]); // mov word [X],0
    let columns = p.code.here();
    p.code
        .emit(&[0x57]) // push di
        .emit(&[0xFF, 0x36, X, 0x00]) // push word [X]
        .emit(&[0xFF, 0x36, Y, 0x00]) // push word [Y]
        .call("GDI", GET_PIXEL)
        .emit(&[0x8B, 0xC8, 0x0B, 0xCA]) // mov cx,ax; or cx,dx
        .emit(&[0x75, 0x06]) // jnz not black
        .emit(&[0xFF, 0x06, BLACK, 0x00]) // inc word [BLACK]
        .emit(&[0xEB, 0x0F]) // jmp next
        .emit(&[0x3D, 0x80, 0x80]) // not black: cmp ax,8080h
        .emit(&[0x75, 0x0A]) // jne next
        .emit(&[0x81, 0xFA, 0x80, 0x00]) // cmp dx,80h
        .emit(&[0x75, 0x04]) // jne next
        .emit(&[0xFF, 0x06, GRAY, 0x00]) // inc word [GRAY]
        .emit(&[0xFF, 0x06, X, 0x00]) // next: inc word [X]
        .emit(&[0x83, 0x3E, X, 0x00, 60]) // cmp word [X],60
        .emit(&[0x72, 0x00]); // jb columns
    let here = p.code.here();
    jump_here(&mut p.code, here - 1, columns);
    p.code
        .emit(&[0xFF, 0x06, Y, 0x00]) // inc word [Y]
        .emit(&[0x83, 0x3E, Y, 0x00, 20]) // cmp word [Y],20
        .emit(&[0x72, 0x00]); // jb rows
    let here = p.code.here();
    jump_here(&mut p.code, here - 1, rows);
    // AL: the types asked for, and the text's colors.
    p.code
        .emit(&[0xA0, ASKED, 0x00]) // mov al,[ASKED]
        .emit(&[0x83, 0x3E, BLACK, 0x00, 0x00]) // cmp word [BLACK],0
        .emit(&[0x74, 0x02, 0x0C, BLACK_TEXT]) // je over; or al,BLACK_TEXT
        .emit(&[0x83, 0x3E, GRAY, 0x00, 0x00]) // cmp word [GRAY],0
        .emit(&[0x74, 0x02, 0x0C, GRAY_TEXT]) // je over; or al,GRAY_TEXT
        .exit_al();
    // The window procedure: WM_CTLCOLOR's type noted as a bit at
    // SS:ASKED (the task's data segment), then DefWindowProc for every
    // message.
    let proc = p.code.here();
    p.code
        .emit(&[0x55, 0x8B, 0xEC]) // push bp; mov bp,sp
        .emit(&[0x83, 0x7E, 0x0C, WM_CTLCOLOR]) // cmp word [bp+0Ch] (msg)
        .emit(&[0x75, 0x0D]) // jne other
        .emit(&[0x8A, 0x4E, 0x08]) // mov cl,[bp+8] (lParam's high word)
        .emit(&[0xB8, 0x01, 0x00, 0xD3, 0xE0]) // mov ax,1; shl ax,cl
        .emit(&[0x36, 0x09, 0x06, ASKED, 0x00]) // or ss:[ASKED],ax
        .emit(&[
            0xFF, 0x76, 0x0E, // other: push [bp+0Eh] (hwnd)
            0xFF, 0x76, 0x0C, // push [bp+0Ch] (msg)
            0xFF, 0x76, 0x0A, // push [bp+0Ah] (wParam)
            0xFF, 0x76, 0x08, // push [bp+8] (lParam)
            0xFF, 0x76, 0x06, // push [bp+6]
        ])
        .call("USER", DEF_WINDOW_PROC)
        .emit(&[0x5D, 0xCA, 0x0A, 0x00]); // pop bp; retf 0Ah
    p.code.patch(proc_at, proc);
    let mut file = p.build();
    let ne = u32::from_le_bytes(file[0x3C..0x40].try_into().unwrap()) as usize;
    file[ne + 0x3E..ne + 0x40].copy_from_slice(&version.to_le_bytes());
    file
}

const ES_MULTILINE: u32 = 0x0004;
const ES_READONLY: u32 = 0x0800;
const WS_DISABLED: u32 = 0x0800_0000;

#[test]
fn a_windows_3_programs_read_only_edit_asks_for_edit_colors() {
    // Windows 98's USER.EXE sends a program made for a Windows before
    // 4.0 WM_CTLCOLOR with CTLCOLOR_EDIT for a read-only edit, in the
    // multi-line edit's WM_ERASEBKGND and in WM_PAINT, as Windows 3.1
    // did. Memphis Math's CTL3D answers its read-only boxes that way.
    let program = edit_colors_program(ES_MULTILINE | ES_READONLY, 0x030A);
    let (exit, log) = run("edit_colors_read_only_3", program);
    assert_eq!(exit, ASKED_EDIT | BLACK_TEXT, "{log}");
}

#[test]
fn a_windows_3_programs_disabled_multi_line_edit_keeps_black_text() {
    // A disabled multi-line edit of a Windows 3.x program asks for
    // CTLCOLOR_EDIT too, and Windows 98 draws its text in the color
    // WM_CTLCOLOR set, black here.
    let program = edit_colors_program(ES_MULTILINE | WS_DISABLED, 0x030A);
    let (exit, log) = run("edit_colors_disabled_3", program);
    assert_eq!(exit, ASKED_EDIT | BLACK_TEXT, "{log}");
}

#[test]
fn a_windows_3_programs_disabled_single_line_edit_has_gray_text() {
    // A disabled single-line edit asks for CTLCOLOR_EDIT, and its text
    // is gray whatever version the program was made for.
    let program = edit_colors_program(WS_DISABLED, 0x030A);
    let (exit, log) = run("edit_colors_disabled_single_3", program);
    assert_eq!(exit, ASKED_EDIT | GRAY_TEXT, "{log}");
}

#[test]
fn a_windows_4_programs_read_only_or_disabled_edit_asks_for_static_colors() {
    // A program made for Windows 4.0 gets CTLCOLOR_STATIC for a
    // read-only or disabled edit, and a disabled one's text is gray.
    let program = edit_colors_program(ES_MULTILINE | WS_DISABLED, 0x0400);
    let (exit, log) = run("edit_colors_disabled_4", program);
    assert_eq!(exit, ASKED_STATIC | GRAY_TEXT, "{log}");
    let program = edit_colors_program(ES_MULTILINE | ES_READONLY, 0x0400);
    let (exit, log) = run("edit_colors_read_only_4", program);
    assert_eq!(exit, ASKED_STATIC | BLACK_TEXT, "{log}");
}

#[test]
fn the_desktops_instance_is_users_dgroup() {
    // GetWindowWord(GetDesktopWindow(), GWW_HINSTANCE) gives USER's
    // instance, its DGROUP, as LoadLibrary("USER.EXE") does, on Windows
    // 95 too (16CEh both). Civilization II's CIVJUMP.DLL reads and writes
    // the word at hwnd+6 in it, as a Windows 3.1 WND's hwndOwner: it must
    // be there for any HWND. GetWindowLong(GWL_HINSTANCE) gives the same
    // instance in AX. Exit code 1 when all holds.
    const GET_DESKTOP_WINDOW: u16 = 286;
    const GET_WINDOW_WORD: u16 = 133;
    const GET_WINDOW_LONG: u16 = 135;
    const LOAD_LIBRARY: u16 = 95;
    const GWW_HINSTANCE: u16 = 0xFFFA;
    let mut code = Code::new();
    code.call("USER", GET_DESKTOP_WINDOW)
        .emit(&[0x8B, 0xD8, 0x53, 0x50]) // mov bx,ax; push bx; push ax
        .push(GWW_HINSTANCE)
        .call("USER", GET_WINDOW_WORD)
        .emit(&[0x8B, 0xF0]) // mov si,ax
        .emit(&[0x0B, 0xC0, 0x75, 0x06, 0xB0, 0x02, 0xB4, 0x4C, 0xCD, 0x21]);
    let name_at = code.here() + 2;
    code.push_far(0)
        .call("KERNEL", LOAD_LIBRARY)
        .emit(&[0x3B, 0xC6, 0x74, 0x06, 0xB0, 0x03, 0xB4, 0x4C, 0xCD, 0x21])
        .emit(&[0x5B, 0x53, 0x53]) // pop bx; push bx; push bx
        .push(GWW_HINSTANCE)
        .call("USER", GET_WINDOW_LONG)
        .emit(&[0x3B, 0xC6, 0x74, 0x06, 0xB0, 0x05, 0xB4, 0x4C, 0xCD, 0x21])
        .emit(&[
            0x5B, // pop bx
            0x8E, 0xC6, // mov es,si
            0x26, 0x8B, 0x47, 0x06, // mov ax,es:[bx+6]
            0x26, 0x89, 0x5F, 0x06, // mov es:[bx+6],bx
            0x26, 0x3B, 0x5F, 0x06, // cmp bx,es:[bx+6]
            0x74, 0x06, 0xB0, 0x04, 0xB4, 0x4C, 0xCD, 0x21, // je; or exit 4
            0xB0, 0x01, // mov al,1
        ])
        .exit_al();
    let name = code.here();
    code.patch(name_at, name);
    code.emit(b"USER.EXE\0");
    let (exit, log) = run("desktop_instance", program(&code, CODE));
    assert_eq!(exit, 1, "{log}");
}
