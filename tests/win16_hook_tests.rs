//! Win16's hooks: a Windows 3.1 program's hook procedures, called by the
//! built-in Windows with Windows 3.1's arguments. The programs are made
//! byte by byte (tests/win16).

mod win16;

use win16::{CODE, Code, program, run};

/// USER's ordinals.
const GET_WINDOW_RECT: u16 = 32;
const CREATE_WINDOW: u16 = 41;
const SET_WINDOWS_HOOK: u16 = 121;
const CALL_MSG_FILTER: u16 = 123;
const DEF_HOOK_PROC: u16 = 235;
const SET_WINDOWS_HOOK_EX: u16 = 291;
const UNHOOK_WINDOWS_HOOK_EX: u16 = 292;
const CALL_NEXT_HOOK_EX: u16 = 293;

const WH_MSGFILTER: u16 = 0xFFFF;
const WH_CBT: u16 = 5;
const HCBT_CREATEWND: u8 = 3;

/// Where the program keeps things in its data segment: its instance,
/// its hooks' handles, what its hook saw, a RECT16 and a MSG16.
const INSTANCE: u16 = 0x10;
const HOOK: u16 = 0x12;
const SEEN: u16 = 0x16;
const HOOK2: u16 = 0x18;
const RECT: u16 = 0x20;
const MSG: u16 = 0x30;

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
        .emit(&[0x6A, WH_CBT as u8]); // push WH_CBT
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

/// The places of far pointers to a program's hooks, and which hook each
/// is, patched when the code is done.
#[derive(Default)]
struct Hooks(Vec<(u16, usize)>);

/// `push far hook` for the hook `which`, its offset patched later.
fn push_hook(code: &mut Code, hooks: &mut Hooks, which: usize) {
    hooks.0.push((code.here() + 2, which));
    code.push_far(0);
}

/// mov [slot],ax; mov [slot+2],dx.
fn keep_dx_ax(code: &mut Code, slot: u16) {
    code.emit(&[0xA3, lo(slot), hi(slot)])
        .emit(&[0x89, 0x16, lo(slot + 2), hi(slot + 2)]);
}

/// SetWindowsHookEx(kind, hook `which`, the instance, all tasks), the
/// handle kept at `slot`.
fn set_hook_ex(code: &mut Code, hooks: &mut Hooks, kind: u16, which: usize, slot: u16) {
    code.push(kind);
    push_hook(code, hooks, which);
    code.emit(&[0xFF, 0x36, lo(INSTANCE), hi(INSTANCE)]) // push [INSTANCE]
        .push(0)
        .call("USER", SET_WINDOWS_HOOK_EX);
    keep_dx_ax(code, slot);
}

/// The start of a program: its instance kept.
fn start() -> Code {
    let mut code = Code::new();
    code.emit(&[0x89, 0x3E, lo(INSTANCE), hi(INSTANCE)]); // mov [INSTANCE],di
    code
}

/// Make a STATIC popup 123 by 45 pixels, and end with its width, or EEh
/// if it wasn't made: the places of the strings' offsets, which `finish`
/// patches.
fn make_window_and_exit_with_width(code: &mut Code) -> (u16, u16) {
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
            0x0B, 0xC0, // or ax,ax
            0x75, 0x04, // jnz made
            0xB0, 0xEE, // mov al,0EEh
            0xEB, 0x00, // jmp end, its distance set below
        ]);
    let jmp = code.here() as usize - 1;
    code.emit(&[0x50, 0x1E]) // made: push ax (the window), push ds
        .push(RECT)
        .call("USER", GET_WINDOW_RECT)
        .emit(&[
            0xA1,
            lo(RECT + 4),
            hi(RECT + 4), // mov ax,[RECT.right]
            0x2B,
            0x06,
            lo(RECT),
            hi(RECT), // sub ax,[RECT.left]
        ]);
    code.bytes[jmp] = (code.here() as usize - jmp - 1) as u8;
    code.exit_al();
    (class_at, title_at)
}

/// A hook that, on HCBT_CREATEWND, makes the window 77 pixels wide, and
/// returns 0 in AX with DX not 0, as a hook declared BOOL may.
fn widen_hook(code: &mut Code) -> u16 {
    let at = code.here();
    code.emit(&[
        0x55,
        0x8B,
        0xEC, // push bp; mov bp,sp
        0x83,
        0x7E,
        0x0C,
        HCBT_CREATEWND, // cmp word [bp+12],HCBT_CREATEWND
        0x75,
        0x0C, // jne done
        0xC4,
        0x5E,
        0x06, // les bx,[bp+6]: the CBT_CREATEWND16
        0x26,
        0xC4,
        0x1F, // les bx,es:[bx]: its CREATESTRUCT16
        0x26,
        0xC7,
        0x47,
        0x0C,
        77,
        0, // mov word es:[bx+12],77: cx
        // done:
        0x33,
        0xC0, // xor ax,ax
        0xBA,
        0xFF,
        0xFF, // mov dx,0FFFFh
        0x5D, // pop bp
        0xCA,
        0x08,
        0x00, // retf 8
    ]);
    at
}

/// A hook that passes every call on with CallNextHookEx and its handle
/// at `slot`, and returns what that does.
fn pass_on_hook(code: &mut Code, slot: u16) -> u16 {
    let at = code.here();
    code.emit(&[
        0x55,
        0x8B,
        0xEC, // push bp; mov bp,sp
        0xFF,
        0x36,
        lo(slot + 2),
        hi(slot + 2), // push [slot+2]
        0xFF,
        0x36,
        lo(slot),
        hi(slot), // push [slot]
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
    .emit(&[0x5D, 0xCA, 0x08, 0x00]); // pop bp; retf 8
    at
}

/// A hook that passes every call on with DefHookProc and the hook
/// SetWindowsHook gave, kept at `slot`.
fn def_hook(code: &mut Code, slot: u16) -> u16 {
    let at = code.here();
    code.emit(&[
        0x55, 0x8B, 0xEC, // push bp; mov bp,sp
        0xFF, 0x76, 0x0C, // push word [bp+12]: code
        0xFF, 0x76, 0x0A, // push word [bp+10]: wParam
        0xFF, 0x76, 0x08, // push word [bp+8]: lParam
        0xFF, 0x76, 0x06, // push word [bp+6]
        0x1E, // push ds
    ])
    .push(slot)
    .call("USER", DEF_HOOK_PROC)
    .emit(&[0x5D, 0xCA, 0x08, 0x00]); // pop bp; retf 8
    at
}

/// Patch the hooks' offsets, `at` by which hook each is, and add the
/// window's strings.
fn finish(code: &mut Code, hooks: &Hooks, at: &[u16], strings: (u16, u16)) {
    for &(place, which) in &hooks.0 {
        code.patch(place, at[which]);
    }
    let class = code.here();
    code.emit(b"STATIC\0");
    let title = code.here();
    code.emit(b"T\0");
    code.patch(strings.0, class);
    code.patch(strings.1, title);
}

#[test]
fn a_cbt_hook_can_change_the_window_being_made() {
    // The hook makes the window 77 pixels wide instead of 123. It leaves
    // DX not 0, which USER mustn't take for a refusal.
    let mut code = start();
    let mut hooks = Hooks::default();
    set_hook_ex(&mut code, &mut hooks, WH_CBT, 0, HOOK);
    let strings = make_window_and_exit_with_width(&mut code);
    let widen = widen_hook(&mut code);
    finish(&mut code, &hooks, &[widen], strings);
    let (exit, log) = run("cbt_widen", program(&code, CODE));
    assert_eq!(exit, 77, "{log}");
}

#[test]
fn a_change_further_down_the_chain_comes_back_through_call_next_hook_ex() {
    // Two hooks: the later one, called first, passes the call on to the
    // one that widens the window. The window is 77 wide.
    let mut code = start();
    let mut hooks = Hooks::default();
    set_hook_ex(&mut code, &mut hooks, WH_CBT, 0, HOOK);
    set_hook_ex(&mut code, &mut hooks, WH_CBT, 1, HOOK2);
    let strings = make_window_and_exit_with_width(&mut code);
    let widen = widen_hook(&mut code);
    let pass = pass_on_hook(&mut code, HOOK2);
    finish(&mut code, &hooks, &[widen, pass], strings);
    let (exit, log) = run("cbt_chain", program(&code, CODE));
    assert_eq!(exit, 77, "{log}");
}

#[test]
fn def_hook_proc_passes_a_set_windows_hook_call_on() {
    // As above, with the later hook set by SetWindowsHook and passing
    // the call on with DefHookProc and the hook SetWindowsHook gave.
    let mut code = start();
    let mut hooks = Hooks::default();
    set_hook_ex(&mut code, &mut hooks, WH_CBT, 0, HOOK);
    code.push(WH_CBT);
    push_hook(&mut code, &mut hooks, 1);
    code.call("USER", SET_WINDOWS_HOOK);
    keep_dx_ax(&mut code, HOOK2);
    let strings = make_window_and_exit_with_width(&mut code);
    let widen = widen_hook(&mut code);
    let def = def_hook(&mut code, HOOK2);
    finish(&mut code, &hooks, &[widen, def], strings);
    let (exit, log) = run("cbt_def_hook", program(&code, CODE));
    assert_eq!(exit, 77, "{log}");
}

#[test]
fn an_unhooked_hook_is_not_called() {
    let mut code = start();
    let mut hooks = Hooks::default();
    set_hook_ex(&mut code, &mut hooks, WH_CBT, 0, HOOK);
    code.emit(&[0xFF, 0x36, lo(HOOK + 2), hi(HOOK + 2)]) // push [HOOK+2]
        .emit(&[0xFF, 0x36, lo(HOOK), hi(HOOK)]) // push [HOOK]
        .call("USER", UNHOOK_WINDOWS_HOOK_EX);
    let strings = make_window_and_exit_with_width(&mut code);
    let widen = widen_hook(&mut code);
    finish(&mut code, &hooks, &[widen], strings);
    let (exit, log) = run("cbt_unhooked", program(&code, CODE));
    assert_eq!(exit, 123, "{log}");
}

#[test]
fn what_a_filter_hook_changes_reaches_call_msg_filters_caller() {
    // A WH_MSGFILTER hook turns the message into WM_USER+5. The program
    // calls CallMsgFilter with a WM_LBUTTONDOWN and ends with the low
    // byte of the message it has after: 5.
    let mut code = start();
    let mut hooks = Hooks::default();
    set_hook_ex(&mut code, &mut hooks, WH_MSGFILTER, 0, HOOK);
    code.emit(&[0xC7, 0x06, lo(MSG + 2), hi(MSG + 2), 0x01, 0x02]) // mov word [MSG+2],201h
        .emit(&[0x1E]) // push ds
        .push(MSG)
        .push(0) // MSGF_DIALOGBOX
        .call("USER", CALL_MSG_FILTER)
        .emit(&[0xA0, lo(MSG + 2), hi(MSG + 2)]) // mov al,[MSG+2]
        .exit_al();
    let filter = code.here();
    code.emit(&[
        0x55, 0x8B, 0xEC, // push bp; mov bp,sp
        0xC4, 0x5E, 0x06, // les bx,[bp+6]: the MSG16
        0x26, 0xC7, 0x47, 0x02, 0x05, 0x04, // mov word es:[bx+2],405h
        0xB8, 0x01, 0x00, // mov ax,1: taken
        0x5D, // pop bp
        0xCA, 0x08, 0x00, // retf 8
    ]);
    for &(place, _) in &hooks.0 {
        code.patch(place, filter);
    }
    let (exit, log) = run("msg_filter", program(&code, CODE));
    assert_eq!(exit, 5, "{log}");
}
