//! Win16 MMSYSTEM: what a Windows 3.1 program gets back from the
//! multimedia functions on the built-in Windows. The programs are made
//! byte by byte (tests/win16).

mod win16;

use win16::{CODE, Code, program, run};

/// MMSYSTEM's ordinals, and KERNEL's and USER's the programs call.
const WAVE_OUT_OPEN: u16 = 404;
const WAVE_OUT_CLOSE: u16 = 405;
const WAVE_OUT_PREPARE_HEADER: u16 = 406;
const WAVE_OUT_UNPREPARE_HEADER: u16 = 407;
const WAVE_OUT_WRITE: u16 = 408;
const WAVE_OUT_RESET: u16 = 411;
const WAVE_OUT_GET_POSITION: u16 = 412;
const TIME_GET_SYSTEM_TIME: u16 = 601;
const TIME_GET_DEV_CAPS: u16 = 604;
const GET_CURRENT_TASK: u16 = 36;
const MESSAGE_BOX: u16 = 1;
const CREATE_WINDOW: u16 = 41;
const REGISTER_CLASS: u16 = 57;
const DEF_WINDOW_PROC: u16 = 107;
const PEEK_MESSAGE: u16 = 109;

/// MM_WOM_DONE, and waveOutOpen's flags for a window or a task to be
/// told, their high words.
const MM_WOM_DONE: u16 = 0x3BD;
const CALLBACK_WINDOW: u16 = 1;
const CALLBACK_TASK: u16 = 2;

/// Where the programs keep things in their data segment: the instance,
/// an MMTIME at 40h and the words after it, a wave device's handle, a
/// window, a WAVEFORMAT, a WNDCLASS, a MSG, a WAVEHDR and its samples.
const INSTANCE: u16 = 0x10;
const TIME: u16 = 0x40;
const HANDLE: u16 = 0x50;
const HWND: u16 = 0x52;
const FORMAT: u16 = 0x60;
const CLASS: u16 = 0x80;
const MSG: u16 = 0xA0;
const HEADER: u16 = 0xC0;
const SAMPLES: u16 = 0xE0;

fn lo(v: u16) -> u8 {
    v as u8
}

fn hi(v: u16) -> u8 {
    (v >> 8) as u8
}

/// mov word [at],value.
fn set(code: &mut Code, at: u16, value: u16) {
    code.emit(&[0xC7, 0x06, lo(at), hi(at), lo(value), hi(value)]);
}

/// End with `fail` unless the word at `at` is `value` (`equal`) or isn't.
fn expect(code: &mut Code, at: u16, value: u16, equal: bool, fail: u8) {
    code.emit(&[0x81, 0x3E, lo(at), hi(at), lo(value), hi(value)])
        .emit(&[if equal { 0x74 } else { 0x75 }, 0x06]) // je/jne over the next
        .emit(&[0xB0, fail, 0xB4, 0x4C, 0xCD, 0x21]); // mov al,fail; exit
}

/// mov [at],cs or ds.
fn set_seg(code: &mut Code, at: u16, ds: bool) {
    code.emit(&[0x8C, if ds { 0x1E } else { 0x0E }, lo(at), hi(at)]);
}

/// push word [at].
fn push_word(code: &mut Code, at: u16) {
    code.emit(&[0xFF, 0x36, lo(at), hi(at)]);
}

/// Open the wave mapper for 8-bit mono PCM at 11025 Hz into HANDLE, with
/// `callback` (what pushes its low word) told as `flags` (their high
/// word) say; end with `fail` if it can't.
fn open_wave(code: &mut Code, callback: impl FnOnce(&mut Code), flags: u16, fail: u8) {
    // PCMWAVEFORMAT: PCM, mono, 11025 Hz, 11025 bytes a second, 1-byte
    // blocks of 8 bits.
    for (i, word) in [1u16, 1, 11025, 0, 11025, 0, 1, 8].into_iter().enumerate() {
        set(code, FORMAT + 2 * i as u16, word);
    }
    code.emit(&[0x1E]) // push ds
        .push(HANDLE)
        .push(0xFFFF) // WAVE_MAPPER
        .emit(&[0x1E])
        .push(FORMAT)
        .push(0);
    callback(code);
    code.push(0)
        .push(0) // instance
        .push(flags)
        .push(0)
        .call("MMSYSTEM", WAVE_OUT_OPEN);
    expect_ax(code, 0, fail);
}

/// Call the wave function `ordinal` with the device and the header at
/// HEADER (32 bytes); end with `fail` if it fails.
fn header_call(code: &mut Code, ordinal: u16, fail: u8) {
    push_word(code, HANDLE);
    code.emit(&[0x1E])
        .push(HEADER)
        .push(32)
        .call("MMSYSTEM", ordinal);
    expect_ax(code, 0, fail);
}

/// A WAVEHDR at HEADER of 16 bytes of silence, prepared, written and
/// returned by waveOutReset, which has MM_WOM_DONE posted for it; end
/// with 10 to 13 for a call that fails.
fn play_and_reset(code: &mut Code) {
    for i in 0..16 {
        set(code, HEADER + 2 * i, 0);
    }
    for i in 0..8 {
        set(code, SAMPLES + 2 * i, 0x8080);
    }
    set(code, HEADER, SAMPLES);
    set_seg(code, HEADER + 2, true);
    set(code, HEADER + 4, 16);
    header_call(code, WAVE_OUT_PREPARE_HEADER, 10);
    header_call(code, WAVE_OUT_WRITE, 11);
    push_word(code, HANDLE);
    code.call("MMSYSTEM", WAVE_OUT_RESET);
    expect_ax(code, 0, 12);
}

/// End with `fail` if AX is 0.
fn expect_nonzero(code: &mut Code, fail: u8) {
    code.emit(&[0x0B, 0xC0]) // or ax,ax
        .emit(&[0x75, 0x06]) // jnz over the next
        .emit(&[0xB0, fail, 0xB4, 0x4C, 0xCD, 0x21]); // mov al,fail; exit
}

/// End with `fail` unless AX is `value`.
fn expect_ax(code: &mut Code, value: u16, fail: u8) {
    code.emit(&[0x3D, lo(value), hi(value)]) // cmp ax,value
        .emit(&[0x74, 0x06]) // je over the next
        .emit(&[0xB0, fail, 0xB4, 0x4C, 0xCD, 0x21]); // mov al,fail; exit
}

/// Marks in the MMTIME's place: the union and the words after it.
fn mark(code: &mut Code) {
    set(code, TIME, 0xEEEE);
    set(code, TIME + 2, 0xFFFF);
    set(code, TIME + 4, 0xFFFF);
    set(code, TIME + 6, 0xCAFE);
    set(code, TIME + 8, 0xBEEF);
}

/// The MMTIME has `kind` and a DWORD time written, and the two bytes
/// after the DWORD and the word after the MMTIME left as they were.
fn expect_dword_time(code: &mut Code, kind: u16) {
    expect(code, TIME, kind, true, 2);
    expect(code, TIME + 2, 0xFFFF, false, 3);
    expect(code, TIME + 6, 0xCAFE, true, 4);
    expect(code, TIME + 8, 0xBEEF, true, 5);
}

#[test]
fn an_mmtime_gets_windows_3_1s_eight_bytes_and_no_more() {
    // timeGetSystemTime into an 8-byte MMTIME: TIME_MS and the time,
    // and nothing past the DWORD. Exit code 1, or which check failed.
    let mut code = Code::new();
    mark(&mut code);
    code.emit(&[0x1E]) // push ds
        .push(TIME)
        .push(8) // cbmmt: sizeof(MMTIME) in Windows 3.1
        .call("MMSYSTEM", TIME_GET_SYSTEM_TIME);
    expect_ax(&mut code, 0, 6);
    expect_dword_time(&mut code, 1); // TIME_MS
    code.emit(&[0xB0, 0x01]).exit_al();
    let (exit, log) = run("mmtime_size", program(&code, CODE));
    assert_eq!(exit, 1, "{log}");
}

#[test]
fn a_short_mmtime_or_timecaps_is_left_alone() {
    // timeGetSystemTime with 4 bytes gives 0 and writes nothing;
    // timeGetDevCaps with 2 gives TIMERR_NOCANDO and writes nothing.
    let mut code = Code::new();
    mark(&mut code);
    code.emit(&[0x1E])
        .push(TIME)
        .push(4)
        .call("MMSYSTEM", TIME_GET_SYSTEM_TIME);
    expect_ax(&mut code, 0, 2);
    expect(&mut code, TIME, 0xEEEE, true, 3);
    code.emit(&[0x1E])
        .push(TIME)
        .push(2)
        .call("MMSYSTEM", TIME_GET_DEV_CAPS);
    expect_ax(&mut code, 97, 4);
    expect(&mut code, TIME, 0xEEEE, true, 5);
    expect(&mut code, TIME + 2, 0xFFFF, true, 6);
    code.emit(&[0xB0, 0x01]).exit_al();
    let (exit, log) = run("mmtime_short", program(&code, CODE));
    assert_eq!(exit, 1, "{log}");
}

#[test]
fn a_wave_position_gets_windows_3_1s_eight_bytes_and_no_more() {
    // waveOutGetPosition of an open device into an 8-byte MMTIME asked
    // for in bytes, as WAVMIX16 does: TIME_BYTES and the position, and
    // nothing past the DWORD.
    let mut code = Code::new();
    open_wave(&mut code, |code| _ = code.push(0), 0, 7);
    mark(&mut code);
    set(&mut code, TIME, 4); // TIME_BYTES
    code.emit(&[0xFF, 0x36, lo(HANDLE), hi(HANDLE)]) // push [HANDLE]
        .emit(&[0x1E])
        .push(TIME)
        .push(8)
        .call("MMSYSTEM", WAVE_OUT_GET_POSITION);
    expect_ax(&mut code, 0, 6);
    expect(&mut code, TIME, 4, true, 2);
    expect(&mut code, TIME + 6, 0xCAFE, true, 4);
    expect(&mut code, TIME + 8, 0xBEEF, true, 5);
    code.emit(&[0xFF, 0x36, lo(HANDLE), hi(HANDLE)])
        .call("MMSYSTEM", WAVE_OUT_CLOSE)
        .emit(&[0xB0, 0x01])
        .exit_al();
    let (exit, log) = run("wave_position", program(&code, CODE));
    assert_eq!(exit, 1, "{log}");
}

/// A window procedure that ends the program on MM_WOM_DONE, with 1 if
/// its lParam is the header at DS:HEADER and 2 if not, and passes the
/// other messages to DefWindowProc.
fn wave_window_proc(code: &mut Code) -> u16 {
    let at = code.here();
    #[rustfmt::skip]
    let bytes = [
        0x55, 0x8B, 0xEC, // push bp; mov bp,sp
        0x81, 0x7E, 0x0C, lo(MM_WOM_DONE), hi(MM_WOM_DONE), // cmp word [bp+0Ch],MM_WOM_DONE
        0x75, 0x18, // jne other
        0x81, 0x7E, 0x06, lo(HEADER), hi(HEADER), // cmp word [bp+6],HEADER
        0x75, 0x0B, // jne bad
        0x8C, 0xD8, // mov ax,ds
        0x39, 0x46, 0x08, // cmp [bp+8],ax
        0x75, 0x04, // jne bad
        0xB0, 0x01, // mov al,1
        0xEB, 0x02, // jmp end
        0xB0, 0x02, // bad: mov al,2
        0xB4, 0x4C, 0xCD, 0x21, // end: exit
        // other:
        0xFF, 0x76, 0x0E, // push word [bp+0Eh]: hwnd
        0xFF, 0x76, 0x0C, // push word [bp+0Ch]: msg
        0xFF, 0x76, 0x0A, // push word [bp+0Ah]: wParam
        0xFF, 0x76, 0x08, // push word [bp+8]: lParam
        0xFF, 0x76, 0x06, // push word [bp+6]
    ];
    code.emit(&bytes)
        .call("USER", DEF_WINDOW_PROC)
        .emit(&[0x5D, 0xCA, 0x0A, 0x00]); // pop bp; retf 10
    at
}

#[test]
fn mm_wom_done_read_by_a_32_bit_loop_carries_the_programs_header() {
    // The program's window is told about its wave header, which
    // waveOutReset returns, and a message box's loop (USER32's own, as a
    // popup menu's is) reads and dispatches the message. The window
    // procedure ends the program: 1 when lParam is the program's 16:16
    // pointer to the header, 2 when it is anything else. 3 to 5 and 10
    // to 13 are calls that failed before, 6 a message box that ended,
    // and the harness's "didn't end" a message that never came.
    let mut code = Code::new();
    code.emit(&[0x89, 0x3E, lo(INSTANCE), hi(INSTANCE)]); // mov [INSTANCE],di
    // WNDCLASS: the window procedure, the instance and the class name.
    for i in 0..13 {
        set(&mut code, CLASS + 2 * i, 0);
    }
    let proc_at = code.here() + 4;
    set(&mut code, CLASS + 2, 0);
    set_seg(&mut code, CLASS + 4, false);
    code.emit(&[0xA1, lo(INSTANCE), hi(INSTANCE)]) // mov ax,[INSTANCE]
        .emit(&[0xA3, lo(CLASS + 10), hi(CLASS + 10)]); // mov [CLASS+10],ax
    let name_at = code.here() + 4;
    set(&mut code, CLASS + 22, 0);
    set_seg(&mut code, CLASS + 24, false);
    code.emit(&[0x1E]).push(CLASS).call("USER", REGISTER_CLASS);
    expect_nonzero(&mut code, 3);
    let class_at = code.here() + 2;
    code.push_far(0);
    let title_at = code.here() + 2;
    code.push_far(0)
        .push(0x8000) // WS_POPUP
        .push(0)
        .push(0)
        .push(0)
        .push(10)
        .push(10)
        .push(0) // parent
        .push(0); // menu
    push_word(&mut code, INSTANCE);
    code.push(0)
        .push(0)
        .call("USER", CREATE_WINDOW)
        .emit(&[0xA3, lo(HWND), hi(HWND)]); // mov [HWND],ax
    expect_nonzero(&mut code, 4);
    open_wave(&mut code, |code| push_word(code, HWND), CALLBACK_WINDOW, 5);
    play_and_reset(&mut code);
    // The message box: its loop dispatches MM_WOM_DONE.
    code.push(0);
    let text_at = code.here() + 2;
    code.push_far(0);
    let caption_at = code.here() + 2;
    code.push_far(0)
        .push(0) // MB_OK
        .call("USER", MESSAGE_BOX)
        .emit(&[0xB0, 0x06])
        .exit_al();
    let proc = wave_window_proc(&mut code);
    code.patch(proc_at, proc);
    let name = code.here();
    code.emit(b"WAVE\0");
    for at in [name_at, class_at, title_at, text_at, caption_at] {
        code.patch(at, name);
    }
    let (exit, log) = run("wom_done_32", program(&code, CODE));
    assert_eq!(exit, 1, "{log}");
}

#[test]
fn mm_wom_done_keeps_the_programs_header_after_it_is_unprepared() {
    // A task is told about its wave header, which waveOutReset returns,
    // and the program unprepares the header before it reads the message,
    // as WAVMIX16 may when it closes the device. MM_WOM_DONE's lParam is
    // still the program's 16:16 pointer to the header: exit code 1, or 2
    // for another value, 3 for no message; 4 and 10 to 14 are calls that
    // failed before.
    let mut code = Code::new();
    code.call("KERNEL", GET_CURRENT_TASK)
        .emit(&[0xA3, lo(HWND), hi(HWND)]); // mov [HWND],ax: the task
    open_wave(&mut code, |code| push_word(code, HWND), CALLBACK_TASK, 4);
    play_and_reset(&mut code);
    header_call(&mut code, WAVE_OUT_UNPREPARE_HEADER, 14);
    code.emit(&[0x1E]) // push ds
        .push(MSG)
        .push(0) // any window
        .push(MM_WOM_DONE)
        .push(MM_WOM_DONE)
        .push(1) // PM_REMOVE
        .call("USER", PEEK_MESSAGE);
    expect_nonzero(&mut code, 3);
    expect(&mut code, MSG + 6, HEADER, true, 2);
    code.emit(&[0x8C, 0xD8]) // mov ax,ds
        .emit(&[0x3B, 0x06, lo(MSG + 8), hi(MSG + 8)]) // cmp ax,[MSG+8]
        .emit(&[0x74, 0x06]) // je over the next
        .emit(&[0xB0, 0x02, 0xB4, 0x4C, 0xCD, 0x21]) // mov al,2; exit
        .emit(&[0xB0, 0x01])
        .exit_al();
    let (exit, log) = run("wom_done_unprepared", program(&code, CODE));
    assert_eq!(exit, 1, "{log}");
}
