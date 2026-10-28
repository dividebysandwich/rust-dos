//! Win16 MMSYSTEM: what a Windows 3.1 program gets back from the
//! multimedia functions on the built-in Windows. The programs are made
//! byte by byte (tests/win16).

mod win16;

use win16::{CODE, Code, program, run};

/// MMSYSTEM's ordinals.
const TIME_GET_SYSTEM_TIME: u16 = 601;

#[test]
fn an_mmtime_is_written_no_further_than_windows_3_1s_eight_bytes() {
    // timeGetSystemTime into an 8-byte MMTIME at DS:40h, a word at 48h
    // just after it: exit code 1 if that word is left as it was, 2 if
    // the time ran over it.
    let mut code = Code::new();
    code.emit(&[
        0xC7, 0x06, 0x40, 0x00, 0x01, 0x00, // mov word [40h],1: TIME_MS
        0xC7, 0x06, 0x48, 0x00, 0xEF, 0xBE, // mov word [48h],0BEEFh
        0x1E, // push ds
    ])
    .push(0x40)
    .push(8) // cbmmt: sizeof(MMTIME) in Windows 3.1
    .call("MMSYSTEM", TIME_GET_SYSTEM_TIME)
    .emit(&[
        0x81, 0x3E, 0x48, 0x00, 0xEF, 0xBE, // cmp word [48h],0BEEFh
        0x75, 0x04, // jne over
        0xB0, 0x01, // mov al,1
        0xEB, 0x02, // jmp end
        0xB0, 0x02, // over: mov al,2
    ])
    .exit_al();
    let (exit, log) = run("mmtime_size", program(&code, CODE));
    assert_eq!(exit, 1, "{log}");
}
