//! Win16's USER: what Windows 3.1 programs get from its functions on the
//! built-in Windows. The programs are made byte by byte (tests/win16).

mod win16;

use win16::{CODE, Code, program, run};

/// USER's ordinals.
const GET_ACTIVE_WINDOW: u16 = 60;
const GET_DESKTOP_WINDOW: u16 = 286;

#[test]
fn with_no_window_active_get_active_window_gives_the_desktop_window() {
    // Exit code 1 when GetActiveWindow gives the desktop window, 0 when
    // it gives another window or none.
    let mut code = Code::new();
    code.call("USER", GET_ACTIVE_WINDOW)
        .emit(&[0x50]) // push ax
        .call("USER", GET_DESKTOP_WINDOW)
        .emit(&[
            0x59, // pop cx
            0x3B, 0xC1, // cmp ax,cx
            0xB0, 0x00, // mov al,0
            0x75, 0x02, // jne over the next
            0xB0, 0x01, // mov al,1
        ])
        .exit_al();
    let (exit, log) = run("active_none", program(&code, CODE));
    assert_eq!(exit, 1, "{log}");
}
