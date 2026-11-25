//! The generic thunks of Windows 95's KERNEL (513 to 518), with which
//! 16-bit code loads a Win32 DLL and calls its functions, as Wise's
//! 16-bit installers write the registry. The programs are made byte by
//! byte (tests/common/win16).

use crate::win16;

use win16::{CODE, Code, program, run};

/// KERNEL's ordinals.
const LOAD_LIBRARY_EX32W: u16 = 513;
const GET_PROC_ADDRESS32W: u16 = 515;
const GET_VDM_POINTER32W: u16 = 516;
const CALL_PROC32W: u16 = 517;
const CALL_PROC_EX32W: u16 = 518;

/// End the program with exit code `fail` unless DX:AX is `value`.
fn expect_dx_ax(code: &mut Code, value: u32, fail: u8) {
    let [a0, a1, d0, d1] = value.to_le_bytes();
    code.emit(&[
        0x3D, a0, a1, // cmp ax,low
        0x75, 0x06, // jne to the exit
        0x81, 0xFA, d0, d1, // cmp dx,high
        0x74, 0x06, // je over the exit
        0xB0, fail, // mov al,fail
    ])
    .exit_al();
}

/// Push the far pointers of the strings whose offsets are patched
/// later: where each offset goes.
fn push_strings(code: &mut Code, count: usize) -> Vec<u16> {
    (0..count)
        .map(|_| {
            let at = code.here() + 2;
            code.push_far(0);
            at
        })
        .collect()
}

#[test]
fn generic_thunks_call_a_win32_function_with_flat_pointers() {
    // KERNEL32's lstrcmpA, found with LoadLibraryEx32W and
    // GetProcAddress32W, called through CallProc32W and _CallProcEx32W
    // with "a" and "b": -1 when the two arrive in order and flat. Exit
    // code 1 when every step is right, the failing step's otherwise.
    let mut code = Code::new();
    let mut strings = Vec::new();
    // LoadLibraryEx32W("KERNEL32.DLL", 0, 0).
    strings.push(push_strings(&mut code, 1));
    code.push(0)
        .push(0)
        .push(0)
        .push(0)
        .call("KERNEL", LOAD_LIBRARY_EX32W)
        // GetProcAddress32W(the module, "lstrcmpA") into SI:DI.
        .emit(&[0x52, 0x50]); // push dx; push ax
    strings.push(push_strings(&mut code, 1));
    code.call("KERNEL", GET_PROC_ADDRESS32W).emit(&[
        0x8B, 0xF2, // mov si,dx
        0x8B, 0xF8, // mov di,ax
        0x0B, 0xC2, // or ax,dx
        0x75, 0x06, // jnz over the exit
        0xB0, 0x02, // mov al,2
    ]);
    code.exit_al();
    // CallProc32W("a", "b", lstrcmpA, both pointers, 2): it takes all
    // of it off the stack.
    code.emit(&[0x8B, 0xEC]); // mov bp,sp
    strings.push(push_strings(&mut code, 2));
    code.emit(&[0x56, 0x57]) // push si; push di
        .push(0)
        .push(3)
        .push(0)
        .push(2)
        .call("KERNEL", CALL_PROC32W);
    code.emit(&[
        0x3B, 0xE5, // cmp sp,bp
        0x74, 0x06, // je over the exit
        0xB0, 0x03, // mov al,3
    ])
    .exit_al();
    expect_dx_ax(&mut code, 0xFFFF_FFFF, 4);
    // CallProc32W with "b" made flat already, by GetVDMPointer32W in
    // protected mode, and the mask's bit 1 for "a": bit 0 is for the
    // last parameter.
    strings.push(push_strings(&mut code, 2));
    code.push(1).call("KERNEL", GET_VDM_POINTER32W);
    code.emit(&[0x52, 0x50, 0x56, 0x57]) // push dx; push ax; push si; push di
        .push(0)
        .push(2)
        .push(0)
        .push(2)
        .call("KERNEL", CALL_PROC32W);
    expect_dx_ax(&mut code, 0xFFFF_FFFF, 5);
    // _CallProcEx32W(2, both, lstrcmpA, "a", "b"), C's: the caller
    // takes the 20 bytes off.
    strings.push(push_strings(&mut code, 2));
    code.emit(&[0x56, 0x57]) // push si; push di
        .push(0)
        .push(3)
        .push(0)
        .push(2)
        .call("KERNEL", CALL_PROC_EX32W)
        .emit(&[0x83, 0xC4, 0x14]); // add sp,20
    code.emit(&[
        0x3B, 0xE5, // cmp sp,bp
        0x74, 0x06, // je over the exit
        0xB0, 0x06, // mov al,6
    ])
    .exit_al();
    expect_dx_ax(&mut code, 0xFFFF_FFFF, 7);
    // GetVDMPointer32W(1234h:0005h, real mode) is 12345h.
    code.push(0x1234)
        .push(0x0005)
        .push(0)
        .call("KERNEL", GET_VDM_POINTER32W);
    expect_dx_ax(&mut code, 0x12345, 8);
    code.emit(&[0xB0, 0x01]).exit_al(); // mov al,1
    fn text(code: &mut Code, s: &[u8]) -> u16 {
        let at = code.here();
        code.emit(s);
        at
    }
    let kernel32 = text(&mut code, b"KERNEL32.DLL\0");
    let name = text(&mut code, b"lstrcmpA\0");
    let a = text(&mut code, b"a\0");
    let b = text(&mut code, b"b\0");
    code.patch(strings[0][0], kernel32);
    code.patch(strings[1][0], name);
    // Pascal: "a" pushed first (the second CallProc32W: then the far
    // pointer to "b" for GetVDMPointer32W). C: "b" pushed first.
    code.patch(strings[2][0], a);
    code.patch(strings[2][1], b);
    code.patch(strings[3][0], a);
    code.patch(strings[3][1], b);
    code.patch(strings[4][0], b);
    code.patch(strings[4][1], a);
    let (exit, log) = run("generic_thunks", program(&code, CODE));
    assert_eq!(exit, 1, "{log}");
}
