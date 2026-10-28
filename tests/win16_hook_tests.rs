//! Win16's hooks: a Windows 3.1 program's hook procedures, called by the
//! built-in Windows with Windows 3.1's arguments. The programs are made
//! byte by byte (tests/win16).

mod win16;

use win16::{CODE, Code, program, run};

/// USER's ordinals.
const CREATE_WINDOW: u16 = 41;
const SET_WINDOWS_HOOK_EX: u16 = 291;
const CALL_NEXT_HOOK_EX: u16 = 293;

const WH_CBT: u8 = 5;
const HCBT_CREATEWND: u8 = 3;

/// Where the program keeps things in its data segment: its instance,
/// its hook's handle, and what its hook saw.
const INSTANCE: u16 = 0x10;
const HOOK: u16 = 0x12;
const SEEN: u16 = 0x16;

fn lo(v: u16) -> u8 {
    v as u8
}

fn hi(v: u16) -> u8 {
    (v >> 8) as u8
}

#[test]
fn a_cbt_hook_sees_the_window_being_made_and_passes_it_on() {
    // Sets a WH_CBT hook, makes a STATIC window 123 pixels wide, and ends
    // with what the hook saw: 1 if it got HCBT_CREATEWND with a
    // CBT_CREATEWND16 whose CREATESTRUCT16 has that width, 0 if not; EEh
    // if no window was made.
    let mut code = Code::new();
    code.emit(&[0x89, 0x3E, lo(INSTANCE), hi(INSTANCE)]) // mov [INSTANCE],di
        .emit(&[0x6A, WH_CBT]); // push WH_CBT
    let hook_at = code.here() + 2;
    code.push_far(0)
        .emit(&[0x57, 0x6A, 0x00]) // push di (instance), push 0 (all tasks)
        .call("USER", SET_WINDOWS_HOOK_EX)
        .emit(&[0xA3, lo(HOOK), hi(HOOK)]) // mov [HOOK],ax
        .emit(&[0x89, 0x16, lo(HOOK + 2), hi(HOOK + 2)]); // mov [HOOK+2],dx
    let class_at = code.here() + 2;
    code.push_far(0);
    let title_at = code.here() + 2;
    code.push_far(0)
        .push(0x8000) // WS_POPUP
        .push(0)
        .push(0) // x
        .push(0) // y
        .push(123) // cx
        .push(45) // cy
        .push(0) // parent
        .push(0) // menu
        .emit(&[0xFF, 0x36, lo(INSTANCE), hi(INSTANCE)]) // push [INSTANCE]
        .push(0)
        .push(0) // lpParam
        .call("USER", CREATE_WINDOW)
        .emit(&[
            0x0B,
            0xC0, // or ax,ax
            0xA0,
            lo(SEEN),
            hi(SEEN), // mov al,[SEEN]
            0x75,
            0x02, // jnz over the next
            0xB0,
            0xEE, // mov al,0EEh
        ])
        .exit_al();

    // The hook (code, wParam, lParam), Pascal's: notes what it saw, then
    // returns what CallNextHookEx does.
    let hook = code.here();
    code.emit(&[
        0x55,
        0x8B,
        0xEC, // push bp; mov bp,sp
        0x1E, // push ds
        0x83,
        0x7E,
        0x0C,
        HCBT_CREATEWND, // cmp word [bp+12],HCBT_CREATEWND
        0x75,
        0x10, // jne next
        0xC4,
        0x5E,
        0x06, // les bx,[bp+6]: the CBT_CREATEWND16
        0x26,
        0xC4,
        0x1F, // les bx,es:[bx]: its CREATESTRUCT16
        0x26,
        0x83,
        0x7F,
        0x0C,
        123, // cmp word es:[bx+12],123: cx
        0x75,
        0x05, // jne next
        0xC6,
        0x06,
        lo(SEEN),
        hi(SEEN),
        0x01, // mov byte [SEEN],1
        // next:
        0xFF,
        0x36,
        lo(HOOK + 2),
        hi(HOOK + 2), // push [HOOK+2]
        0xFF,
        0x36,
        lo(HOOK),
        hi(HOOK), // push [HOOK]
        0xFF,
        0x76,
        0x0C, // push word [bp+12]: code
        0xFF,
        0x76,
        0x0A, // push word [bp+10]: wParam
        0xFF,
        0x76,
        0x08, // push word [bp+8]: lParam
        0xFF,
        0x76,
        0x06, // push word [bp+6]
    ])
    .call("USER", CALL_NEXT_HOOK_EX)
    .emit(&[
        0x1F, // pop ds
        0x5D, // pop bp
        0xCA, 0x08, 0x00, // retf 8
    ]);
    let class = code.here();
    code.emit(b"STATIC\0");
    let title = code.here();
    code.emit(b"T\0");
    code.patch(hook_at, hook);
    code.patch(class_at, class);
    code.patch(title_at, title);

    let (exit, log) = run("cbt_create", program(&code, CODE));
    assert_eq!(exit, 1, "{log}");
}
