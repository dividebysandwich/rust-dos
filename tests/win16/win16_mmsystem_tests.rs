//! Win16 MMSYSTEM: what a Windows 3.1 program gets back from the
//! multimedia functions on the built-in Windows. The programs are made
//! byte by byte (tests/common/win16).

use crate::win16;

use win16::{CODE, Code, program, run, run_with};

/// MMSYSTEM's ordinals, and KERNEL's and USER's the programs call.
const WAVE_OUT_OPEN: u16 = 404;
const WAVE_OUT_CLOSE: u16 = 405;
const WAVE_OUT_PREPARE_HEADER: u16 = 406;
const WAVE_OUT_UNPREPARE_HEADER: u16 = 407;
const WAVE_OUT_WRITE: u16 = 408;
const WAVE_OUT_RESET: u16 = 411;
const WAVE_OUT_GET_POSITION: u16 = 412;
const WAVE_OUT_GET_DEV_CAPS: u16 = 402;
const WAVE_OUT_GET_ID: u16 = 420;
const WAVE_OUT_GET_PITCH: u16 = 413;
const WAVE_OUT_SET_PITCH: u16 = 414;
const WAVE_OUT_GET_PLAYBACK_RATE: u16 = 417;
const WAVE_OUT_SET_PLAYBACK_RATE: u16 = 418;
const MIDI_OUT_GET_ID: u16 = 215;
const AUX_GET_NUM_DEVS: u16 = 350;
const AUX_GET_DEV_CAPS: u16 = 351;
const AUX_GET_VOLUME: u16 = 352;
const AUX_SET_VOLUME: u16 = 353;
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

#[test]
fn wave_out_get_id_writes_a_16_bit_uint() {
    // waveOutGetID of an open device, as Battle Beast calls it: 0 in the
    // word, the word after it left alone. A handle that isn't one gets
    // MMSYSERR_INVALHANDLE and nothing written, and so does midiOutGetID
    // with a handle that isn't a MIDI device's.
    let mut code = Code::new();
    open_wave(&mut code, |code| _ = code.push(0), 0, 7);
    mark(&mut code);
    push_word(&mut code, HANDLE);
    code.emit(&[0x1E])
        .push(TIME)
        .call("MMSYSTEM", WAVE_OUT_GET_ID);
    expect_ax(&mut code, 0, 2);
    expect(&mut code, TIME, 0, true, 3);
    expect(&mut code, TIME + 2, 0xFFFF, true, 4);
    set(&mut code, TIME, 0xEEEE);
    code.push(0x1234)
        .emit(&[0x1E])
        .push(TIME)
        .call("MMSYSTEM", WAVE_OUT_GET_ID);
    expect_ax(&mut code, 5, 5);
    expect(&mut code, TIME, 0xEEEE, true, 6);
    code.push(0x1234)
        .emit(&[0x1E])
        .push(TIME)
        .call("MMSYSTEM", MIDI_OUT_GET_ID);
    expect_ax(&mut code, 5, 8);
    expect(&mut code, TIME, 0xEEEE, true, 9);
    push_word(&mut code, HANDLE);
    code.call("MMSYSTEM", WAVE_OUT_CLOSE)
        .emit(&[0xB0, 0x01])
        .exit_al();
    let (exit, log) = run("wave_get_id", program(&code, CODE));
    assert_eq!(exit, 1, "{log}");
}

/// MMSYSTEM has waveOutGetPitch, waveOutSetPitch, waveOutGetPlaybackRate
/// and waveOutSetPlaybackRate, as Windows 3.1's does. Authorware's
/// runtime (Civilization II's Civilopedia) looks all four up and stops
/// if one is missing. The device has neither control, so each answers
/// MMSYSERR_NOTSUPPORTED and a Get leaves its DWORD alone. A handle that
/// isn't one gets MMSYSERR_INVALHANDLE.
#[test]
fn wave_out_pitch_and_rate_are_not_supported() {
    let mut code = Code::new();
    open_wave(&mut code, |code| _ = code.push(0), 0, 30);
    for (i, (ordinal, get)) in [
        (WAVE_OUT_GET_PITCH, true),
        (WAVE_OUT_SET_PITCH, false),
        (WAVE_OUT_GET_PLAYBACK_RATE, true),
        (WAVE_OUT_SET_PLAYBACK_RATE, false),
    ]
    .into_iter()
    .enumerate()
    {
        // Exit codes 2 to 25: three for the device, three for a handle
        // that isn't one.
        for (j, (handle, error)) in [(None, 8), (Some(0x1234), 5)].into_iter().enumerate() {
            let fail = 2 + 6 * i as u8 + 3 * j as u8;
            mark(&mut code);
            match handle {
                None => push_word(&mut code, HANDLE),
                Some(h) => _ = code.push(h),
            }
            if get {
                code.emit(&[0x1E]).push(TIME);
            } else {
                code.push(1).push(0); // 1.0 in 16.16
            }
            code.call("MMSYSTEM", ordinal);
            expect_ax(&mut code, error, fail);
            expect(&mut code, TIME, 0xEEEE, true, fail + 1);
            expect(&mut code, TIME + 2, 0xFFFF, true, fail + 2);
        }
    }
    push_word(&mut code, HANDLE);
    code.call("MMSYSTEM", WAVE_OUT_CLOSE)
        .emit(&[0xB0, 0x01])
        .exit_al();
    let (exit, log) = run("wave_pitch_rate", program(&code, CODE));
    assert_eq!(exit, 1, "{log}");
}

/// Where the aux test puts its AUXCAPS16 (44 bytes) and the word after.
const CAPS: u16 = 0x80;

#[test]
fn aux_devices_are_winmms_with_16_bit_caps() {
    // auxGetNumDevs counts WINMM's devices; auxGetDevCaps fills the 44
    // bytes of an AUXCAPS16 (a WORD version at 4, the name at 6, the
    // technology at 38, dwSupport at 40 with AUXCAPS_VOLUME) and not the
    // word after; AUX_MAPPER is a bad id; auxSetVolume and auxGetVolume
    // set and read a device's volume as a DWORD.
    let mut code = Code::new();
    code.call("MMSYSTEM", AUX_GET_NUM_DEVS);
    expect_nonzero(&mut code, 2);
    set(&mut code, CAPS + 40, 0);
    set(&mut code, CAPS + 44, 0xBEEF);
    code.push(0)
        .emit(&[0x1E])
        .push(CAPS)
        .push(44)
        .call("MMSYSTEM", AUX_GET_DEV_CAPS);
    expect_ax(&mut code, 0, 3);
    // Device 0 is "Sound Card Aux: Wave", AUXCAPS_WAVE (32), version 4.0.
    expect(&mut code, CAPS + 4, 0x0400, true, 12);
    expect(&mut code, CAPS + 6, u16::from_le_bytes(*b"So"), true, 13);
    expect(&mut code, CAPS + 38, 32, true, 14);
    code.emit(&[0xF6, 0x06, lo(CAPS + 40), hi(CAPS + 40), 0x01]) // test byte [..],1
        .emit(&[0x75, 0x06]) // jnz over the next
        .emit(&[0xB0, 4, 0xB4, 0x4C, 0xCD, 0x21]);
    expect(&mut code, CAPS + 44, 0xBEEF, true, 5);
    code.push(0xFFFF)
        .emit(&[0x1E])
        .push(CAPS)
        .push(44)
        .call("MMSYSTEM", AUX_GET_DEV_CAPS);
    expect_ax(&mut code, 2, 6); // MMSYSERR_BADDEVICEID
    code.push(0xFFFF)
        .push(0x1234)
        .push(0x5678)
        .call("MMSYSTEM", AUX_SET_VOLUME);
    expect_ax(&mut code, 2, 15);
    code.push(0)
        .push(0x1234)
        .push(0x5678)
        .call("MMSYSTEM", AUX_SET_VOLUME);
    expect_ax(&mut code, 0, 7);
    mark(&mut code);
    code.push(0)
        .emit(&[0x1E])
        .push(TIME)
        .call("MMSYSTEM", AUX_GET_VOLUME);
    expect_ax(&mut code, 0, 8);
    expect(&mut code, TIME, 0x5678, true, 9);
    expect(&mut code, TIME + 2, 0x1234, true, 10);
    expect(&mut code, TIME + 4, 0xFFFF, true, 11);
    code.emit(&[0xB0, 0x01]).exit_al();
    let (exit, log) = run("aux_devices", program(&code, CODE));
    assert_eq!(exit, 1, "{log}");
}

/// Call the MMSYSTEM function `ordinal` with `first` (a handle or a
/// device) and a null pointer, and `size` after it when there is one;
/// end with `fail` unless AX is `error`.
fn null_call(code: &mut Code, ordinal: u16, first: u16, size: Option<u16>, error: u16, fail: u8) {
    code.push(first).push(0).push(0);
    if let Some(size) = size {
        code.push(size);
    }
    code.call("MMSYSTEM", ordinal);
    expect_ax(code, error, fail);
}

#[test]
fn a_null_pointer_is_refused_after_the_handle_or_device() {
    // A null pointer is MMSYSERR_INVALPARAM (11) for a good handle or
    // device, and the handle's MMSYSERR_INVALHANDLE (5) or the device's
    // MMSYSERR_BADDEVICEID (2) for a bad one; nothing is written at
    // linear 0.
    let mut code = Code::new();
    open_wave(&mut code, |code| _ = code.push(0), 0, 2);
    push_word(&mut code, HANDLE);
    code.push(0).push(0).call("MMSYSTEM", WAVE_OUT_GET_ID);
    expect_ax(&mut code, 11, 3);
    null_call(&mut code, WAVE_OUT_GET_ID, 0x1234, None, 5, 4);
    null_call(&mut code, WAVE_OUT_GET_DEV_CAPS, 0, Some(52), 11, 5);
    null_call(&mut code, WAVE_OUT_GET_DEV_CAPS, 0x1234, Some(52), 2, 6);
    null_call(&mut code, AUX_GET_DEV_CAPS, 0, Some(44), 11, 7);
    null_call(&mut code, AUX_GET_DEV_CAPS, 0xFFFF, Some(44), 2, 8);
    null_call(&mut code, AUX_GET_VOLUME, 0, None, 11, 9);
    null_call(&mut code, AUX_GET_VOLUME, 0xFFFF, None, 2, 10);
    push_word(&mut code, HANDLE);
    code.call("MMSYSTEM", WAVE_OUT_CLOSE)
        .emit(&[0xB0, 0x01])
        .exit_al();
    let (exit, log) = run("null_pointers", program(&code, CODE));
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

/// End with `fail` unless the word at `at` is DS.
fn expect_ds(code: &mut Code, at: u16, fail: u8) {
    code.emit(&[0x8C, 0xD8]) // mov ax,ds
        .emit(&[0x3B, 0x06, lo(at), hi(at)]) // cmp ax,[at]
        .emit(&[0x74, 0x06]) // je over the next
        .emit(&[0xB0, fail, 0xB4, 0x4C, 0xCD, 0x21]); // mov al,fail; exit
}

/// End with `fail` unless the header at HEADER still has the program's
/// 16:16 pointer to SAMPLES in lpData, and with `fail` + 1 unless its
/// dwFlags are `flags`.
fn expect_header(code: &mut Code, flags: u16, fail: u8) {
    expect(code, HEADER, SAMPLES, true, fail);
    expect_ds(code, HEADER + 2, fail);
    expect(code, HEADER + 16, flags, true, fail + 1);
}

#[test]
fn a_wave_header_keeps_the_16_16_data_pointer_the_program_wrote() {
    // Windows 98's MMSYSTEM leaves a 16-bit program's WAVEHDR as the
    // program wrote it, lpData too, and changes only dwFlags:
    // WHDR_PREPARED after waveOutPrepareHeader, WHDR_PREPARED and
    // WHDR_INQUEUE after waveOutWrite, WHDR_PREPARED and WHDR_DONE once
    // waveOutReset returns it, and WHDR_DONE after
    // waveOutUnprepareHeader. MM_WOM_DONE's header is then read as
    // Miles' WAIL16.DLL reads it: lpData loaded with LES and the first
    // samples read through it. Exit code 1; 20 to 27 for a header that
    // changed, 28 for other samples; 3, 4 and 10 to 14 are calls that
    // failed.
    let mut code = Code::new();
    code.call("KERNEL", GET_CURRENT_TASK)
        .emit(&[0xA3, lo(HWND), hi(HWND)]); // mov [HWND],ax: the task
    open_wave(&mut code, |code| push_word(code, HWND), CALLBACK_TASK, 4);
    for i in 0..16 {
        set(&mut code, HEADER + 2 * i, 0);
    }
    for i in 0..8 {
        set(&mut code, SAMPLES + 2 * i, 0x8080);
    }
    set(&mut code, SAMPLES, 0x5A81);
    set(&mut code, HEADER, SAMPLES);
    set_seg(&mut code, HEADER + 2, true);
    set(&mut code, HEADER + 4, 16);
    header_call(&mut code, WAVE_OUT_PREPARE_HEADER, 10);
    expect_header(&mut code, 0x02, 20);
    header_call(&mut code, WAVE_OUT_WRITE, 11);
    expect_header(&mut code, 0x12, 22);
    push_word(&mut code, HANDLE);
    code.call("MMSYSTEM", WAVE_OUT_RESET);
    expect_ax(&mut code, 0, 12);
    expect_header(&mut code, 0x03, 24);
    code.emit(&[0x1E]) // push ds
        .push(MSG)
        .push(0) // any window
        .push(MM_WOM_DONE)
        .push(MM_WOM_DONE)
        .push(1) // PM_REMOVE
        .call("USER", PEEK_MESSAGE);
    expect_nonzero(&mut code, 3);
    code.emit(&[0xC4, 0x3E, lo(MSG + 6), hi(MSG + 6)]) // les di,[MSG+6]
        .emit(&[0x26, 0xC4, 0x3D]) // les di,es:[di]: lpData
        .emit(&[0x26, 0x8B, 0x05]); // mov ax,es:[di]
    expect_ax(&mut code, 0x5A81, 28);
    header_call(&mut code, WAVE_OUT_UNPREPARE_HEADER, 14);
    expect_header(&mut code, 0x01, 26);
    code.emit(&[0xB0, 0x01]).exit_al();
    let (exit, log) = run("wave_header_kept", program(&code, CODE));
    assert_eq!(exit, 1, "{log}");
}

/// waveOutOpen's flags for a function to be called, their high word.
const CALLBACK_FUNCTION: u16 = 3;
/// MM_WOM_OPEN.
const MM_WOM_OPEN: u16 = 0x3BB;
/// Where the callback function keeps the calls' count, the messages of
/// the first two and the last MM_WOM_DONE's dwParam1.
const CALLS: u16 = 0x20;
const FIRST: u16 = 0x22;
const SECOND: u16 = 0x24;
const DONE_P1: u16 = 0x26;

/// A wave or MIDI callback function, `void CALLBACK f(HANDLE, UINT,
/// DWORD, DWORD, DWORD)`, which counts its calls in SS:CALLS, keeps the
/// messages of the first two in SS:FIRST and SS:SECOND, and the dwParam1
/// of `done` (MM_WOM_DONE or MM_MOM_DONE) in SS:DONE_P1.
fn callback_function(code: &mut Code, done: u16) -> u16 {
    let at = code.here();
    #[rustfmt::skip]
    let bytes = [
        0x55, 0x8B, 0xEC, // push bp; mov bp,sp
        0x8B, 0x46, 0x12, // mov ax,[bp+12h]: the message
        0x36, 0x83, 0x3E, lo(CALLS), hi(CALLS), 0x00, // cmp word ss:[CALLS],0
        0x75, 0x04, // jne not_first
        0x36, 0xA3, lo(FIRST), hi(FIRST), // mov ss:[FIRST],ax
        // not_first:
        0x36, 0x83, 0x3E, lo(CALLS), hi(CALLS), 0x01, // cmp word ss:[CALLS],1
        0x75, 0x04, // jne not_second
        0x36, 0xA3, lo(SECOND), hi(SECOND), // mov ss:[SECOND],ax
        // not_second:
        0x36, 0xFF, 0x06, lo(CALLS), hi(CALLS), // inc word ss:[CALLS]
        0x3D, lo(done), hi(done), // cmp ax,done
        0x75, 0x0E, // jne end
        0x8B, 0x46, 0x0A, // mov ax,[bp+0Ah]: dwParam1's low word
        0x36, 0xA3, lo(DONE_P1), hi(DONE_P1), // mov ss:[DONE_P1],ax
        0x8B, 0x46, 0x0C, // mov ax,[bp+0Ch]
        0x36, 0xA3, lo(DONE_P1 + 2), hi(DONE_P1 + 2), // mov ss:[DONE_P1+2],ax
        // end:
        0x5D, 0xCA, 0x10, 0x00, // pop bp; retf 10h
    ];
    code.emit(&bytes);
    at
}

#[test]
fn a_wave_callback_function_is_called_with_the_programs_header() {
    // A device opened with CALLBACK_FUNCTION calls the function with
    // MM_WOM_OPEN and then MM_WOM_DONE, whose dwParam1 is the program's
    // 16:16 pointer to the header waveOutReset returned, as Windows 3.1
    // calls Miles' WAIL16.DLL. Windows makes those calls, and
    // MM_WOM_CLOSE's, before waveOutOpen, waveOutReset and waveOutClose
    // return, since the program may free the header and the function
    // then. A block that plays to its end is done later: the task makes
    // that call when it asks for a message, here PeekMessage in a loop.
    // Exit code 1; 2 for no calls or another number, 3 and 4 for other
    // messages, 5 and 6 for another header, 8 for no call while the task
    // asks for messages and 9 for none in waveOutClose; 7, 14 and 10 to
    // 13 are calls that failed.
    let mut code = Code::new();
    for at in [CALLS, FIRST, SECOND, DONE_P1, DONE_P1 + 2] {
        set(&mut code, at, 0);
    }
    for (i, word) in [1u16, 1, 11025, 0, 11025, 0, 1, 8].into_iter().enumerate() {
        set(&mut code, FORMAT + 2 * i as u16, word);
    }
    code.emit(&[0x1E]) // push ds
        .push(HANDLE)
        .push(0xFFFF) // WAVE_MAPPER
        .emit(&[0x1E])
        .push(FORMAT);
    let callback_at = code.here() + 2;
    code.push_far(0)
        .push(0)
        .push(0) // instance
        .push(CALLBACK_FUNCTION)
        .push(0)
        .call("MMSYSTEM", WAVE_OUT_OPEN);
    expect_ax(&mut code, 0, 13);
    play_and_reset(&mut code);
    expect(&mut code, CALLS, 2, true, 2);
    expect(&mut code, FIRST, MM_WOM_OPEN, true, 3);
    expect(&mut code, SECOND, MM_WOM_DONE, true, 4);
    expect(&mut code, DONE_P1, HEADER, true, 5);
    expect_ds(&mut code, DONE_P1 + 2, 6);
    // The block again, played to its end this time.
    header_call(&mut code, WAVE_OUT_WRITE, 7);
    code.emit(&[0xBE, 0xFF, 0xFF]); // mov si,0FFFFh
    let ask = code.here();
    code.emit(&[0x1E]) // push ds
        .push(MSG)
        .push(0) // any window
        .push(0)
        .push(0)
        .push(1) // PM_REMOVE
        .call("USER", PEEK_MESSAGE)
        .emit(&[0x83, 0x3E, lo(CALLS), hi(CALLS), 0x03]) // cmp word [CALLS],3
        .emit(&[0x74, 0x09]) // je over the rest
        .emit(&[0x4E]); // dec si
    let back = ask as i32 - (code.here() as i32 + 2);
    code.emit(&[0x75, back as i8 as u8]) // jnz ask
        .emit(&[0xB0, 0x08, 0xB4, 0x4C, 0xCD, 0x21]); // mov al,8; exit
    push_word(&mut code, HANDLE);
    code.call("MMSYSTEM", WAVE_OUT_CLOSE);
    expect_ax(&mut code, 0, 14);
    expect(&mut code, CALLS, 4, true, 9);
    code.emit(&[0xB0, 0x01]).exit_al();
    let callback = callback_function(&mut code, MM_WOM_DONE);
    code.patch(callback_at, callback);
    let (exit, log) = run("wave_callback", program(&code, CODE));
    assert_eq!(exit, 1, "{log}");
}

const MIDI_OUT_GET_NUM_DEVS: u16 = 201;
const MIDI_OUT_GET_DEV_CAPS: u16 = 202;
const MIDI_OUT_GET_ERROR_TEXT: u16 = 203;
const MIDI_OUT_OPEN: u16 = 204;
const MIDI_OUT_CLOSE: u16 = 205;
const MIDI_OUT_PREPARE_HEADER: u16 = 206;
const MIDI_OUT_UNPREPARE_HEADER: u16 = 207;
const MIDI_OUT_SHORT_MSG: u16 = 208;
const MIDI_OUT_LONG_MSG: u16 = 209;
const MIDI_OUT_RESET: u16 = 210;
const MIDI_OUT_GET_VOLUME: u16 = 211;
const MIDI_OUT_SET_VOLUME: u16 = 212;
const WAVE_IN_GET_NUM_DEVS: u16 = 501;
const WAVE_IN_GET_DEV_CAPS: u16 = 502;
const WAVE_IN_OPEN: u16 = 504;
const WAVE_IN_CLOSE: u16 = 505;
const WAVE_IN_MESSAGE: u16 = 514;
const MIDI_IN_GET_NUM_DEVS: u16 = 301;
const MIDI_IN_GET_DEV_CAPS: u16 = 302;
const MIDI_IN_OPEN: u16 = 304;
const MIDI_IN_CLOSE: u16 = 305;

/// The MIDI callback messages.
const MM_MOM_OPEN: u16 = 0x3C7;
const MM_MOM_CLOSE: u16 = 0x3C8;
const MM_MOM_DONE: u16 = 0x3C9;

#[test]
fn midi_out_devices_are_winmms_with_16_bit_caps() {
    // midiOutGetNumDevs counts WINMM's devices. midiOutGetDevCaps fills
    // the 50 bytes of a MIDIOUTCAPS16 and not the word after: a WORD
    // version at 4, the name at 6, wTechnology at 38 (MOD_SYNTH, 2),
    // wVoices at 40, wChannelMask at 44 and dwSupport at 46
    // (MIDICAPS_VOLUME, LRVOLUME and STREAM). MIDI_MAPPER (FFFFh) is
    // MOD_MAPPER (5), and device 7 a bad id. The volume set is read back
    // as a DWORD, and midiOutGetErrorText has a text. With no input
    // device, waveInGetNumDevs and midiInGetNumDevs are 0, the
    // GetDevCaps and Open functions get MMSYSERR_BADDEVICEID, and
    // waveInClose, waveInMessage and midiInClose MMSYSERR_INVALHANDLE.
    // Exit code 1, or the check that failed.
    let mut code = Code::new();
    code.call("MMSYSTEM", MIDI_OUT_GET_NUM_DEVS);
    expect_nonzero(&mut code, 2);
    set(&mut code, CAPS + 38, 0xEEEE);
    set(&mut code, CAPS + 50, 0xBEEF);
    code.push(0)
        .emit(&[0x1E])
        .push(CAPS)
        .push(50)
        .call("MMSYSTEM", MIDI_OUT_GET_DEV_CAPS);
    expect_ax(&mut code, 0, 3);
    expect(&mut code, CAPS + 4, 0x0400, true, 12);
    expect(&mut code, CAPS + 6, u16::from_le_bytes(*b"MP"), true, 13);
    expect(&mut code, CAPS + 38, 2, true, 14);
    expect(&mut code, CAPS + 40, 32, true, 15);
    expect(&mut code, CAPS + 44, 0xFFFF, true, 16);
    expect(&mut code, CAPS + 46, 0x000B, true, 17);
    expect(&mut code, CAPS + 48, 0, true, 18);
    expect(&mut code, CAPS + 50, 0xBEEF, true, 4);
    code.push(0xFFFF)
        .emit(&[0x1E])
        .push(CAPS)
        .push(50)
        .call("MMSYSTEM", MIDI_OUT_GET_DEV_CAPS);
    expect_ax(&mut code, 0, 5);
    expect(&mut code, CAPS + 38, 5, true, 6);
    code.push(7)
        .emit(&[0x1E])
        .push(CAPS)
        .push(50)
        .call("MMSYSTEM", MIDI_OUT_GET_DEV_CAPS);
    expect_ax(&mut code, 2, 7); // MMSYSERR_BADDEVICEID
    code.push(0)
        .push(0x1234)
        .push(0x5678)
        .call("MMSYSTEM", MIDI_OUT_SET_VOLUME);
    expect_ax(&mut code, 0, 8);
    mark(&mut code);
    code.push(0)
        .emit(&[0x1E])
        .push(TIME)
        .call("MMSYSTEM", MIDI_OUT_GET_VOLUME);
    expect_ax(&mut code, 0, 9);
    expect(&mut code, TIME, 0x5678, true, 10);
    expect(&mut code, TIME + 2, 0x1234, true, 10);
    expect(&mut code, TIME + 4, 0xFFFF, true, 10);
    set(&mut code, CAPS, 0);
    code.push(6) // MMSYSERR_NODRIVER
        .emit(&[0x1E])
        .push(CAPS)
        .push(50)
        .call("MMSYSTEM", MIDI_OUT_GET_ERROR_TEXT);
    expect_ax(&mut code, 0, 11);
    expect(&mut code, CAPS, 0, false, 11);
    code.call("MMSYSTEM", WAVE_IN_GET_NUM_DEVS);
    expect_ax(&mut code, 0, 20);
    code.push(0)
        .emit(&[0x1E])
        .push(CAPS)
        .push(48)
        .call("MMSYSTEM", WAVE_IN_GET_DEV_CAPS);
    expect_ax(&mut code, 2, 21);
    code.emit(&[0x1E]) // push ds
        .push(HANDLE)
        .push(0xFFFF) // WAVE_MAPPER
        .emit(&[0x1E])
        .push(FORMAT)
        .push(0)
        .push(0)
        .push(0)
        .push(0)
        .push(0)
        .push(0)
        .call("MMSYSTEM", WAVE_IN_OPEN);
    expect_ax(&mut code, 2, 22);
    code.push(0x1234).call("MMSYSTEM", WAVE_IN_CLOSE);
    expect_ax(&mut code, 5, 23);
    code.push(0x1234)
        .push(0x0400) // MM_USER
        .push(0)
        .push(0)
        .push(0)
        .push(0)
        .call("MMSYSTEM", WAVE_IN_MESSAGE);
    expect_ax(&mut code, 5, 24);
    code.call("MMSYSTEM", MIDI_IN_GET_NUM_DEVS);
    expect_ax(&mut code, 0, 25);
    code.push(0)
        .emit(&[0x1E])
        .push(CAPS)
        .push(44)
        .call("MMSYSTEM", MIDI_IN_GET_DEV_CAPS);
    expect_ax(&mut code, 2, 26);
    code.emit(&[0x1E]) // push ds
        .push(HANDLE)
        .push(0)
        .push(0)
        .push(0)
        .push(0)
        .push(0)
        .push(0)
        .push(0)
        .call("MMSYSTEM", MIDI_IN_OPEN);
    expect_ax(&mut code, 2, 27);
    code.push(0x1234).call("MMSYSTEM", MIDI_IN_CLOSE);
    expect_ax(&mut code, 5, 28);
    code.emit(&[0xB0, 0x01]).exit_al();
    let (exit, log) = run("midi_caps", program(&code, CODE));
    assert_eq!(exit, 1, "{log}");
}

/// Open MIDI device `device` into HANDLE, with `callback` (what pushes
/// it, a DWORD) told as `flags` (their high word) say; end with `fail`
/// if it can't.
fn open_midi(code: &mut Code, device: u16, callback: impl FnOnce(&mut Code), flags: u16, fail: u8) {
    code.emit(&[0x1E]) // push ds
        .push(HANDLE)
        .push(device);
    callback(code);
    code.push(0)
        .push(0) // instance
        .push(flags)
        .push(0)
        .call("MMSYSTEM", MIDI_OUT_OPEN);
    expect_ax(code, 0, fail);
}

/// A MIDIHDR at HEADER of the 6 bytes of a General MIDI System On
/// message at SAMPLES, its lpData the program's 16:16 pointer.
fn midi_header(code: &mut Code) {
    for i in 0..14 {
        set(code, HEADER + 2 * i, 0);
    }
    set(code, SAMPLES, 0x7EF0);
    set(code, SAMPLES + 2, 0x097F);
    set(code, SAMPLES + 4, 0xF701);
    set(code, HEADER, SAMPLES);
    set_seg(code, HEADER + 2, true);
    set(code, HEADER + 4, 6);
}

/// Call the MIDI function `ordinal` with the device and the header at
/// HEADER, `size` bytes; end with `fail` unless it returns `result`.
fn midi_header_call(code: &mut Code, ordinal: u16, size: u16, result: u16, fail: u8) {
    push_word(code, HANDLE);
    code.emit(&[0x1E])
        .push(HEADER)
        .push(size)
        .call("MMSYSTEM", ordinal);
    expect_ax(code, result, fail);
}

#[test]
fn a_midi_callback_function_is_called_with_the_programs_header() {
    // MIDI output through the MIDI mapper with CALLBACK_FUNCTION, as
    // Battle Beast plays its music: the function gets MM_MOM_OPEN before
    // midiOutOpen returns, and midiOutGetID gives MIDI_MAPPER as a word.
    // A note goes out with midiOutShortMsg. A System Exclusive message
    // goes out with midiOutLongMsg from a 28-byte MIDIHDR whose lpData
    // is the program's 16:16 pointer, which stays as it was: only
    // dwFlags change (MHDR_PREPARED, then MHDR_DONE as well, then
    // MHDR_DONE alone once unprepared). MM_MOM_DONE, with the program's
    // pointer to the header, comes before midiOutLongMsg returns, and
    // MM_MOM_CLOSE before midiOutClose does. A header shorter than
    // Windows 3.1's is MMSYSERR_INVALPARAM. Exit code 1, or the check
    // that failed.
    let mut code = Code::new();
    for at in [CALLS, FIRST, SECOND, DONE_P1, DONE_P1 + 2] {
        set(&mut code, at, 0);
    }
    let mut callback_at = 0;
    open_midi(
        &mut code,
        0xFFFF, // MIDI_MAPPER
        |code| {
            callback_at = code.here() + 2;
            code.push_far(0);
        },
        CALLBACK_FUNCTION,
        2,
    );
    expect(&mut code, CALLS, 1, true, 3);
    expect(&mut code, FIRST, MM_MOM_OPEN, true, 3);
    set(&mut code, TIME, 0xEEEE);
    set(&mut code, TIME + 2, 0x1111);
    push_word(&mut code, HANDLE);
    code.emit(&[0x1E])
        .push(TIME)
        .call("MMSYSTEM", MIDI_OUT_GET_ID);
    expect_ax(&mut code, 0, 4);
    expect(&mut code, TIME, 0xFFFF, true, 5);
    expect(&mut code, TIME + 2, 0x1111, true, 5);
    // Note on: middle C, velocity 127, on channel 1.
    push_word(&mut code, HANDLE);
    code.push(0x007F)
        .push(0x3C90)
        .call("MMSYSTEM", MIDI_OUT_SHORT_MSG);
    expect_ax(&mut code, 0, 6);
    midi_header(&mut code);
    midi_header_call(&mut code, MIDI_OUT_PREPARE_HEADER, 20, 11, 7);
    midi_header_call(&mut code, MIDI_OUT_PREPARE_HEADER, 28, 0, 8);
    expect_header(&mut code, 0x02, 20);
    midi_header_call(&mut code, MIDI_OUT_LONG_MSG, 28, 0, 9);
    expect_header(&mut code, 0x03, 22);
    expect(&mut code, CALLS, 2, true, 10);
    expect(&mut code, SECOND, MM_MOM_DONE, true, 10);
    expect(&mut code, DONE_P1, HEADER, true, 11);
    expect_ds(&mut code, DONE_P1 + 2, 11);
    midi_header_call(&mut code, MIDI_OUT_UNPREPARE_HEADER, 28, 0, 12);
    expect_header(&mut code, 0x01, 24);
    push_word(&mut code, HANDLE);
    code.call("MMSYSTEM", MIDI_OUT_RESET);
    expect_ax(&mut code, 0, 13);
    push_word(&mut code, HANDLE);
    code.call("MMSYSTEM", MIDI_OUT_CLOSE);
    expect_ax(&mut code, 0, 14);
    expect(&mut code, CALLS, 3, true, 15);
    // The handle is gone.
    push_word(&mut code, HANDLE);
    code.call("MMSYSTEM", MIDI_OUT_CLOSE);
    expect_ax(&mut code, 5, 16);
    code.emit(&[0xB0, 0x01]).exit_al();
    let callback = callback_function(&mut code, MM_MOM_DONE);
    code.patch(callback_at, callback);
    let (exit, log) = run("midi_callback", program(&code, CODE));
    assert_eq!(exit, 1, "{log}");
}

/// Where `nested_long_msg_callback` keeps how deep its calls are, the
/// deepest they went, and the MM_MOM_DONEs it got.
const DEPTH: u16 = 0x2A;
const MAX_DEPTH: u16 = 0x2C;
const DONES: u16 = 0x2E;

/// A MIDI callback function that counts how deep its calls nest in
/// SS:DEPTH and SS:MAX_DEPTH and its MM_MOM_DONEs in SS:DONES, and sends
/// the header of the first MM_MOM_DONE again with midiOutLongMsg, as a
/// program that sends a SysEx from the callback does.
fn nested_long_msg_callback(code: &mut Code) -> u16 {
    let at = code.here();
    #[rustfmt::skip]
    let head = [
        0x55, 0x8B, 0xEC, // push bp; mov bp,sp
        0x36, 0xFF, 0x06, lo(DEPTH), hi(DEPTH), // inc word ss:[DEPTH]
        0x36, 0xA1, lo(DEPTH), hi(DEPTH), // mov ax,ss:[DEPTH]
        0x36, 0x3B, 0x06, lo(MAX_DEPTH), hi(MAX_DEPTH), // cmp ax,ss:[MAX_DEPTH]
        0x76, 0x04, // jbe not_deeper
        0x36, 0xA3, lo(MAX_DEPTH), hi(MAX_DEPTH), // mov ss:[MAX_DEPTH],ax
        // not_deeper:
        0x8B, 0x46, 0x12, // mov ax,[bp+12h]: the message
        0x3D, lo(MM_MOM_DONE), hi(MM_MOM_DONE), // cmp ax,MM_MOM_DONE
        0x75, 0x1E, // jne end
        0x36, 0xFF, 0x06, lo(DONES), hi(DONES), // inc word ss:[DONES]
        0x36, 0x83, 0x3E, lo(DONES), hi(DONES), 0x01, // cmp word ss:[DONES],1
        0x75, 0x11, // jne end
        0xFF, 0x76, 0x14, // push word [bp+14h]: the handle
        0xFF, 0x76, 0x0C, // push word [bp+0Ch]: dwParam1, the header
        0xFF, 0x76, 0x0A, // push word [bp+0Ah]
    ];
    code.emit(&head)
        .push(28)
        .call("MMSYSTEM", MIDI_OUT_LONG_MSG);
    #[rustfmt::skip]
    let end = [
        // end:
        0x36, 0xFF, 0x0E, lo(DEPTH), hi(DEPTH), // dec word ss:[DEPTH]
        0x5D, 0xCA, 0x10, 0x00, // pop bp; retf 10h
    ];
    code.emit(&end);
    at
}

#[test]
fn callback_functions_are_not_called_inside_each_other() {
    // A callback function that calls midiOutLongMsg, here for the
    // MM_MOM_DONE of an earlier one, isn't called again before it
    // returns: the MM_MOM_DONE of its own message comes after it
    // returns, and still before the program's midiOutLongMsg returns.
    // Exit code 1, 5 for MM_MOM_DONEs that didn't come, 6 for calls
    // inside each other, or the call that failed.
    let mut code = Code::new();
    for at in [DEPTH, MAX_DEPTH, DONES] {
        set(&mut code, at, 0);
    }
    let mut callback_at = 0;
    open_midi(
        &mut code,
        0xFFFF, // MIDI_MAPPER
        |code| {
            callback_at = code.here() + 2;
            code.push_far(0);
        },
        CALLBACK_FUNCTION,
        2,
    );
    midi_header(&mut code);
    midi_header_call(&mut code, MIDI_OUT_PREPARE_HEADER, 28, 0, 3);
    midi_header_call(&mut code, MIDI_OUT_LONG_MSG, 28, 0, 4);
    expect(&mut code, DONES, 2, true, 5);
    expect(&mut code, MAX_DEPTH, 1, true, 6);
    expect_header(&mut code, 0x03, 7);
    midi_header_call(&mut code, MIDI_OUT_UNPREPARE_HEADER, 28, 0, 9);
    push_word(&mut code, HANDLE);
    code.call("MMSYSTEM", MIDI_OUT_CLOSE);
    expect_ax(&mut code, 0, 10);
    expect(&mut code, MAX_DEPTH, 1, true, 11);
    code.emit(&[0xB0, 0x01]).exit_al();
    let callback = nested_long_msg_callback(&mut code);
    code.patch(callback_at, callback);
    let (exit, log) = run("midi_nested", program(&code, CODE));
    assert_eq!(exit, 1, "{log}");
}

/// PeekMessage(DS:MSG, any window, `msg`, `msg`, PM_REMOVE); end with
/// `fail` if there is none.
fn peek(code: &mut Code, msg: u16, fail: u8) {
    code.emit(&[0x1E]) // push ds
        .push(MSG)
        .push(0) // any window
        .push(msg)
        .push(msg)
        .push(1) // PM_REMOVE
        .call("USER", PEEK_MESSAGE);
    expect_nonzero(code, fail);
}

#[test]
fn a_task_is_posted_mm_mom_done_with_the_programs_header() {
    // MIDI device 0 opened with CALLBACK_TASK: the task gets MM_MOM_OPEN,
    // MM_MOM_DONE for the header midiOutLongMsg sent, its lParam the
    // program's 16:16 pointer to the header, and MM_MOM_CLOSE. Exit code
    // 1, or the check that failed.
    let mut code = Code::new();
    code.call("KERNEL", GET_CURRENT_TASK)
        .emit(&[0xA3, lo(HWND), hi(HWND)]); // mov [HWND],ax: the task
    open_midi(
        &mut code,
        0,
        |code| {
            code.push(0);
            push_word(code, HWND);
        },
        CALLBACK_TASK,
        2,
    );
    peek(&mut code, MM_MOM_OPEN, 3);
    midi_header(&mut code);
    midi_header_call(&mut code, MIDI_OUT_PREPARE_HEADER, 28, 0, 4);
    midi_header_call(&mut code, MIDI_OUT_LONG_MSG, 28, 0, 5);
    peek(&mut code, MM_MOM_DONE, 6);
    expect(&mut code, MSG + 6, HEADER, true, 7);
    expect_ds(&mut code, MSG + 8, 7);
    midi_header_call(&mut code, MIDI_OUT_UNPREPARE_HEADER, 28, 0, 8);
    push_word(&mut code, HANDLE);
    code.call("MMSYSTEM", MIDI_OUT_CLOSE);
    expect_ax(&mut code, 0, 9);
    peek(&mut code, MM_MOM_CLOSE, 10);
    code.emit(&[0xB0, 0x01]).exit_al();
    let (exit, log) = run("midi_task", program(&code, CODE));
    assert_eq!(exit, 1, "{log}");
}

const TIME_SET_EVENT: u16 = 602;
const TIME_KILL_EVENT: u16 = 603;

/// A timer's function, `void CALLBACK f(UINT id, UINT msg, DWORD user,
/// DWORD, DWORD)`, which counts its calls in SS:CALLS and keeps the last
/// call's id in SS:FIRST and the low word of its user's DWORD in
/// SS:SECOND.
fn timer_callback(code: &mut Code) -> u16 {
    let at = code.here();
    #[rustfmt::skip]
    let bytes = [
        0x55, 0x8B, 0xEC, // push bp; mov bp,sp
        0x36, 0xFF, 0x06, lo(CALLS), hi(CALLS), // inc word ss:[CALLS]
        0x8B, 0x46, 0x14, // mov ax,[bp+14h]: the id
        0x36, 0xA3, lo(FIRST), hi(FIRST), // mov ss:[FIRST],ax
        0x8B, 0x46, 0x0E, // mov ax,[bp+0Eh]: the user's low word
        0x36, 0xA3, lo(SECOND), hi(SECOND), // mov ss:[SECOND],ax
        0x5D, 0xCA, 0x10, 0x00, // pop bp; retf 10h
    ];
    code.emit(&bytes);
    at
}

/// Ask for messages with PeekMessage until SS:CALLS is `calls`; end with
/// `fail` if it isn't after 65535 asks.
fn peek_until_calls(code: &mut Code, calls: u8, fail: u8) {
    code.emit(&[0xBE, 0xFF, 0xFF]); // mov si,0FFFFh
    let ask = code.here();
    code.emit(&[0x1E]) // push ds
        .push(MSG)
        .push(0) // any window
        .push(0)
        .push(0)
        .push(1) // PM_REMOVE
        .call("USER", PEEK_MESSAGE)
        .emit(&[0x83, 0x3E, lo(CALLS), hi(CALLS), calls]) // cmp word [CALLS],calls
        .emit(&[0x74, 0x09]) // je over the rest
        .emit(&[0x4E]); // dec si
    let back = ask as i32 - (code.here() as i32 + 2);
    code.emit(&[0x75, back as i8 as u8]) // jnz ask
        .emit(&[0xB0, fail, 0xB4, 0x4C, 0xCD, 0x21]); // mov al,fail; exit
}

/// timeSetEvent(`delay` ms, 1 ms, the function at `callback`'s place,
/// 1234_5678h, `flags`); its id goes to HANDLE, and the program ends
/// with `fail` if there is none. Returns where the function's offset
/// goes.
fn set_timer(code: &mut Code, delay: u16, flags: u16, fail: u8) -> u16 {
    code.push(delay).push(1);
    let callback_at = code.here() + 2;
    code.push_far(0)
        .push(0x1234)
        .push(0x5678)
        .push(flags)
        .call("MMSYSTEM", TIME_SET_EVENT)
        .emit(&[0xA3, lo(HANDLE), hi(HANDLE)]); // mov [HANDLE],ax
    expect_nonzero(code, fail);
    callback_at
}

#[test]
fn a_timers_function_is_called_by_its_task() {
    // timeSetEvent's one-shot timer calls its function once, with the
    // timer's id and the user's DWORD, as Battle Beast's MIDI sequencer
    // sets one for each event of its songs; the task makes the call as
    // it asks for messages. A periodic timer calls it until
    // timeKillEvent, which succeeds once. Exit code 1, or the check
    // that failed.
    let mut code = Code::new();
    for at in [CALLS, FIRST, SECOND] {
        set(&mut code, at, 0);
    }
    let one_shot = set_timer(&mut code, 2, 0, 2);
    peek_until_calls(&mut code, 1, 3);
    code.emit(&[0xA1, lo(HANDLE), hi(HANDLE)]) // mov ax,[HANDLE]
        .emit(&[0x3B, 0x06, lo(FIRST), hi(FIRST)]) // cmp ax,[FIRST]
        .emit(&[0x74, 0x06]) // je over the next
        .emit(&[0xB0, 0x04, 0xB4, 0x4C, 0xCD, 0x21]); // mov al,4; exit
    expect(&mut code, SECOND, 0x5678, true, 5);
    let periodic = set_timer(&mut code, 1, 1, 6); // TIME_PERIODIC
    peek_until_calls(&mut code, 4, 7);
    push_word(&mut code, HANDLE);
    code.call("MMSYSTEM", TIME_KILL_EVENT);
    expect_ax(&mut code, 0, 8);
    push_word(&mut code, HANDLE);
    code.call("MMSYSTEM", TIME_KILL_EVENT);
    expect_nonzero(&mut code, 9);
    code.emit(&[0xB0, 0x01]).exit_al();
    let callback = timer_callback(&mut code);
    code.patch(one_shot, callback);
    code.patch(periodic, callback);
    let (exit, log) = run("time_set_event", program(&code, CODE));
    assert_eq!(exit, 1, "{log}");
}

const MCI_SEND_COMMAND: u16 = 701;

/// The commands and flags the MCI tests send, and what they get.
const MCI_OPEN: u16 = 0x803;
const MCI_CLOSE: u16 = 0x804;
const MCI_PLAY: u16 = 0x806;
const MCI_INFO: u16 = 0x80A;
const MCI_PUT: u16 = 0x842;
const MCI_WHERE: u16 = 0x843;
const MCI_STATUS: u16 = 0x814;
const MCI_OPEN_ELEMENT: u32 = 0x200;
const MCI_OPEN_TYPE: u32 = 0x2000;
const MCI_STATUS_ITEM: u32 = 0x100;
const MCI_STATUS_MODE: u16 = 4;
const MCI_INFO_PRODUCT: u32 = 0x100;
const MCI_DGV_RECT: u32 = 0x1_0000;
const MCI_DGV_PUT_SOURCE: u32 = 0x2_0000;
const MCI_DGV_PUT_DESTINATION: u32 = 0x4_0000;
const MCI_DGV_WHERE_MAX: u32 = 0x40_0000;
const MCI_MODE_STOP: u16 = 525;
const MCI_MODE_PLAY: u16 = 526;
const MCIERR_INVALID_DEVICE_ID: u16 = 257;

/// Where the MCI programs keep their blocks: MCI_OPEN_PARMS, with a
/// word after it; MCI_STATUS_PARMS; MCI_INFO_PARMS and its buffer;
/// MCI_DGV_RECT_PARMS for MCI_PUT and for MCI_WHERE, each with a word
/// after it.
const OPEN: u16 = 0x20;
const DEVICE: u16 = OPEN + 4;
const STATUS: u16 = 0x40;
const INFO: u16 = 0x50;
const BUFFER: u16 = 0x60;
const PUT: u16 = 0x80;
const WHERE: u16 = 0x90;

/// mciSendCommand(the device, `msg`, `flags`, DS:`parms`, or NULL for
/// 0); end with `fail` unless it returns `result` in DX:AX.
fn mci(code: &mut Code, msg: u16, flags: u32, parms: u16, result: u16, fail: u8) {
    push_word(code, DEVICE);
    code.push(msg).push((flags >> 16) as u16).push(flags as u16);
    if parms == 0 {
        code.push(0).push(0);
    } else {
        code.emit(&[0x1E]).push(parms); // push ds
    }
    code.call("MMSYSTEM", MCI_SEND_COMMAND);
    expect_ax(code, result, fail);
    code.emit(&[0x0B, 0xD2]) // or dx,dx
        .emit(&[0x74, 0x06]) // jz over the next
        .emit(&[0xB0, fail, 0xB4, 0x4C, 0xCD, 0x21]); // mov al,fail; exit
}

/// mov word [at],offset and mov [at+2],cs: a far pointer to an offset
/// of the code, which is patched in at the place this returns.
fn set_far(code: &mut Code, at: u16) -> u16 {
    let place = code.here() + 4;
    set(code, at, 0);
    set_seg(code, at + 2, false);
    place
}

/// MCI_OPEN_PARMS at OPEN: no callback, marks in wDeviceID, the word
/// after it and the word after the block, and pointers to the type and
/// the element, whose places in the code this returns.
fn open_parms(code: &mut Code) -> (u16, u16) {
    for i in 0..14 {
        set(code, OPEN + 2 * i, 0);
    }
    set(code, DEVICE, 0xEEEE);
    set(code, DEVICE + 2, 0xCAFE);
    set(code, OPEN + 20, 0xBEEF);
    (set_far(code, OPEN + 8), set_far(code, OPEN + 12))
}

/// MCI_STATUS of the mode at STATUS; end with `fail` unless it is
/// `mode`, a DWORD.
fn expect_mode(code: &mut Code, mode: u16, fail: u8) {
    set(code, STATUS, 0);
    set(code, STATUS + 2, 0);
    set(code, STATUS + 4, 0xEEEE);
    set(code, STATUS + 6, 0xEEEE);
    set(code, STATUS + 8, MCI_STATUS_MODE);
    set(code, STATUS + 10, 0);
    mci(code, MCI_STATUS, MCI_STATUS_ITEM, STATUS, 0, fail);
    expect(code, STATUS + 4, mode, true, fail);
    expect(code, STATUS + 6, 0, true, fail);
}

/// A Standard MIDI File of one empty track.
fn empty_midi() -> Vec<u8> {
    let mut file = b"MThd".to_vec();
    file.extend(6u32.to_be_bytes());
    file.extend([0, 0, 0, 1, 0, 96]);
    file.extend(b"MTrk");
    file.extend(4u32.to_be_bytes());
    file.extend([0x00, 0xFF, 0x2F, 0x00]);
    file
}

#[test]
fn mci_send_command_opens_a_sequencer_with_its_16_bit_parameter_block() {
    // MCI_OPEN with MCI_OPEN_TYPE | MCI_OPEN_ELEMENT and 16:16 strings,
    // as Metal Marines opens its theme music: the device id is the word
    // wDeviceID, and the reserved word after it is left alone. Then
    // MCI_STATUS's DWORD, MCI_INFO into a 16:16 buffer, MCI_CLOSE with
    // no block, and a status of the closed device. Exit code 1, or the
    // check that failed.
    let mut code = Code::new();
    let (ty, element) = open_parms(&mut code);
    mci(
        &mut code,
        MCI_OPEN,
        MCI_OPEN_TYPE | MCI_OPEN_ELEMENT,
        OPEN,
        0,
        2,
    );
    expect(&mut code, DEVICE, 0xEEEE, false, 3);
    expect(&mut code, DEVICE, 0, false, 3);
    expect(&mut code, DEVICE + 2, 0xCAFE, true, 4);
    expect(&mut code, OPEN + 20, 0xBEEF, true, 4);
    expect_mode(&mut code, MCI_MODE_STOP, 5);
    // MCI_INFO_PARMS: no callback, DS:BUFFER, 32 bytes.
    set(&mut code, INFO, 0);
    set(&mut code, INFO + 2, 0);
    set(&mut code, INFO + 4, BUFFER);
    set_seg(&mut code, INFO + 6, true);
    set(&mut code, INFO + 8, 32);
    set(&mut code, INFO + 10, 0);
    mci(&mut code, MCI_INFO, MCI_INFO_PRODUCT, INFO, 0, 6);
    expect(&mut code, BUFFER, u16::from_le_bytes(*b"MI"), true, 7);
    mci(&mut code, MCI_CLOSE, 0, 0, 0, 8);
    set(&mut code, STATUS + 8, MCI_STATUS_MODE);
    let invalid = MCIERR_INVALID_DEVICE_ID;
    mci(&mut code, MCI_STATUS, MCI_STATUS_ITEM, STATUS, invalid, 9);
    code.emit(&[0xB0, 0x01]).exit_al();
    let type_at = code.here();
    code.emit(b"sequencer\0");
    let element_at = code.here();
    code.emit(b"M.MID\0");
    code.patch(ty, type_at);
    code.patch(element, element_at);
    let (exit, log) = run_with("mci_sequencer", program(&code, CODE), |_, dir| {
        std::fs::write(dir.join("M.MID"), empty_midi()).unwrap();
    });
    assert_eq!(exit, 1, "{log}");
}

const MCI_SYSINFO: u16 = 0x810;
const MCI_SYSINFO_NAME: u32 = 0x400;
const MCI_DEVTYPE_SEQUENCER: u16 = 523;
/// Where the MCI_SYSINFO test keeps its MCI_SYSINFO_PARMS.
const SYSINFO: u16 = 0xA0;

#[test]
fn mci_send_command_gives_sysinfos_name_in_a_16_bit_buffer() {
    // MCI_SYSINFO_NAME of the first installed sequencer: the name comes
    // back in the 16:16 buffer lpstrReturn points to. wDeviceType is a
    // word with a reserved word after it, marked here, which stays as
    // it was. Exit code 1, or the check that failed.
    let mut code = Code::new();
    set(&mut code, DEVICE, 0);
    set(&mut code, SYSINFO, 0);
    set(&mut code, SYSINFO + 2, 0);
    set(&mut code, SYSINFO + 4, BUFFER);
    set_seg(&mut code, SYSINFO + 6, true);
    set(&mut code, SYSINFO + 8, 32);
    set(&mut code, SYSINFO + 10, 0);
    set(&mut code, SYSINFO + 12, 1);
    set(&mut code, SYSINFO + 14, 0);
    set(&mut code, SYSINFO + 16, MCI_DEVTYPE_SEQUENCER);
    set(&mut code, SYSINFO + 18, 0xCAFE);
    mci(&mut code, MCI_SYSINFO, MCI_SYSINFO_NAME, SYSINFO, 0, 2);
    expect(&mut code, BUFFER, u16::from_le_bytes(*b"se"), true, 3);
    expect(&mut code, BUFFER + 8, u16::from_le_bytes(*b"r\0"), true, 4);
    expect(&mut code, SYSINFO + 18, 0xCAFE, true, 5);
    code.emit(&[0xB0, 0x01]).exit_al();
    let (exit, log) = run("mci_sysinfo", program(&code, CODE));
    assert_eq!(exit, 1, "{log}");
}

/// The frames of an AVI file: twelve uncompressed 24-bit 32x32
/// pictures, with an index.
pub(crate) fn avi_clip() -> Vec<u8> {
    fn chunk(out: &mut Vec<u8>, id: &[u8], data: &[u8]) {
        out.extend_from_slice(id);
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
    }
    let le = |v: u32| v.to_le_bytes();
    let mut strh = vec![0; 56];
    strh[..4].copy_from_slice(b"vids");
    strh[20..24].copy_from_slice(&le(1));
    strh[24..28].copy_from_slice(&le(25));
    strh[32..36].copy_from_slice(&le(12));
    let mut bih = vec![0; 40];
    bih[..4].copy_from_slice(&le(40));
    bih[4..8].copy_from_slice(&le(32));
    bih[8..12].copy_from_slice(&le(32));
    bih[12..16].copy_from_slice(&[1, 0, 24, 0]);
    let mut strl = b"strl".to_vec();
    chunk(&mut strl, b"strh", &strh);
    chunk(&mut strl, b"strf", &bih);
    let mut hdrl = b"hdrl".to_vec();
    chunk(&mut hdrl, b"avih", &[0; 56]);
    chunk(&mut hdrl, b"LIST", &strl);
    let mut movi = b"movi".to_vec();
    let mut idx1 = Vec::new();
    for _ in 0..12 {
        idx1.extend_from_slice(b"00db");
        idx1.extend_from_slice(&le(0x10));
        idx1.extend_from_slice(&le(movi.len() as u32));
        idx1.extend_from_slice(&le(32 * 32 * 3));
        chunk(&mut movi, b"00db", &[0, 0, 255].repeat(32 * 32));
    }
    let mut body = b"AVI ".to_vec();
    chunk(&mut body, b"LIST", &hdrl);
    chunk(&mut body, b"LIST", &movi);
    chunk(&mut body, b"idx1", &idx1);
    let mut out = Vec::new();
    chunk(&mut out, b"RIFF", &body);
    out
}

/// An AVI file of sound only: one stream of 8-bit mono PCM at 11025 Hz,
/// with 100 bytes of silence and an index.
fn avi_sound() -> Vec<u8> {
    fn chunk(out: &mut Vec<u8>, id: &[u8], data: &[u8]) {
        out.extend_from_slice(id);
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
    }
    let le = |v: u32| v.to_le_bytes();
    let mut strh = vec![0; 56];
    strh[..4].copy_from_slice(b"auds");
    strh[20..24].copy_from_slice(&le(1));
    strh[24..28].copy_from_slice(&le(11025));
    strh[32..36].copy_from_slice(&le(100));
    strh[44..48].copy_from_slice(&le(1));
    // PCMWAVEFORMAT: PCM, mono, 11025 Hz, 11025 bytes a second, 1-byte
    // blocks of 8 bits.
    let mut wf = vec![1, 0, 1, 0];
    wf.extend_from_slice(&le(11025));
    wf.extend_from_slice(&le(11025));
    wf.extend_from_slice(&[1, 0, 8, 0]);
    let mut strl = b"strl".to_vec();
    chunk(&mut strl, b"strh", &strh);
    chunk(&mut strl, b"strf", &wf);
    let mut hdrl = b"hdrl".to_vec();
    chunk(&mut hdrl, b"avih", &[0; 56]);
    chunk(&mut hdrl, b"LIST", &strl);
    let mut movi = b"movi".to_vec();
    let mut idx1 = b"00wb".to_vec();
    idx1.extend_from_slice(&le(0x10));
    idx1.extend_from_slice(&le(movi.len() as u32));
    idx1.extend_from_slice(&le(100));
    chunk(&mut movi, b"00wb", &[0x80; 100]);
    let mut body = b"AVI ".to_vec();
    chunk(&mut body, b"LIST", &hdrl);
    chunk(&mut body, b"LIST", &movi);
    chunk(&mut body, b"idx1", &idx1);
    let mut out = Vec::new();
    chunk(&mut out, b"RIFF", &body);
    out
}

/// A RECT of words at `at` and a mark after it.
fn set_rect(code: &mut Code, at: u16, rect: [u16; 4]) {
    for (i, v) in rect.into_iter().enumerate() {
        set(code, at + 2 * i as u16, v);
    }
    set(code, at + 8, 0xBEEF);
}

/// End with `fail` unless the RECT of words at `at` is `rect` and the
/// mark after it is there.
fn expect_rect(code: &mut Code, at: u16, rect: [u16; 4], fail: u8) {
    for (i, v) in rect.into_iter().enumerate() {
        expect(code, at + 2 * i as u16, v, true, fail);
    }
    expect(code, at + 8, 0xBEEF, true, fail);
}

#[test]
fn mci_send_command_plays_an_avi_file_with_rects_of_words() {
    // MCI_OPEN with only MCI_OPEN_ELEMENT, as Metal Marines's readouts
    // open their movies: the .AVI file opens on AVIVideo. MCI_PUT's
    // RECT of words, with a negative left, comes back from MCI_WHERE
    // as words, with nothing written past them; the source's largest
    // rectangle is the picture's size. Then a play, and the close.
    // Exit code 1, or the check that failed.
    let mut code = Code::new();
    let (_, element) = open_parms(&mut code);
    mci(&mut code, MCI_OPEN, MCI_OPEN_ELEMENT, OPEN, 0, 2);
    expect(&mut code, DEVICE, 0xEEEE, false, 3);
    expect(&mut code, DEVICE + 2, 0xCAFE, true, 4);
    let dest = [(-8i16) as u16, 6, 20, 16];
    set(&mut code, PUT, 0);
    set(&mut code, PUT + 2, 0);
    set_rect(&mut code, PUT + 4, dest);
    let put = MCI_DGV_PUT_DESTINATION | MCI_DGV_RECT;
    mci(&mut code, MCI_PUT, put, PUT, 0, 5);
    set(&mut code, WHERE, 0);
    set(&mut code, WHERE + 2, 0);
    set_rect(&mut code, WHERE + 4, [0xEEEE; 4]);
    mci(&mut code, MCI_WHERE, MCI_DGV_PUT_DESTINATION, WHERE, 0, 6);
    expect_rect(&mut code, WHERE + 4, dest, 7);
    let max = MCI_DGV_PUT_SOURCE | MCI_DGV_WHERE_MAX;
    mci(&mut code, MCI_WHERE, max, WHERE, 0, 8);
    expect_rect(&mut code, WHERE + 4, [0, 0, 32, 32], 9);
    set(&mut code, STATUS, 0);
    set(&mut code, STATUS + 2, 0);
    mci(&mut code, MCI_PLAY, 0, STATUS, 0, 10);
    expect_mode(&mut code, MCI_MODE_PLAY, 11);
    mci(&mut code, MCI_CLOSE, 0, 0, 0, 12);
    code.emit(&[0xB0, 0x01]).exit_al();
    let element_at = code.here();
    code.emit(b"CLIP.AVI\0");
    code.patch(element, element_at);
    let (exit, log) = run_with("mci_avi", program(&code, CODE), |_, dir| {
        std::fs::write(dir.join("CLIP.AVI"), avi_clip()).unwrap();
    });
    assert_eq!(exit, 1, "{log}");
}

const MCI_SEND_STRING: u16 = 702;
const MCI_GET_DEVICE_ID: u16 = 703;
const GET_PALETTE_ENTRIES: u16 = 363;
const MCI_DGV_STATUS_HPAL: u16 = 0x4004;

/// mciSendString(the string at the code's `text`, DS:BUFFER, 32, 0);
/// end with `fail` unless it returns 0 in DX:AX. Returns the place of
/// the string's offset, which is patched in.
fn mci_string(code: &mut Code, fail: u8) -> u16 {
    code.emit(&[0x0E]); // push cs
    let place = code.here() + 1;
    code.push(0)
        .emit(&[0x1E]) // push ds
        .push(BUFFER)
        .push(32)
        .push(0)
        .call("MMSYSTEM", MCI_SEND_STRING);
    expect_ax(code, 0, fail);
    code.emit(&[0x0B, 0xD2]) // or dx,dx
        .emit(&[0x74, 0x06]) // jz over the next
        .emit(&[0xB0, fail, 0xB4, 0x4C, 0xCD, 0x21]); // mov al,fail; exit
    place
}

#[test]
fn mci_status_palette_handle_is_the_avi_devices_palette() {
    // `status c palette handle`, as Memphis Math's animations send it
    // through mciSendString, and MCI_STATUS of MCI_DGV_STATUS_HPAL
    // through mciSendCommand, before a frame is drawn. On a desktop of
    // 256 colours both give the palette MCIAVI draws the movie with:
    // the string's decimal number is the DWORD's, and GetPaletteEntries
    // counts the halftone palette's 256 colours of a 24-bit movie. On a
    // desktop of more colours there is no palette, and both give 0, as
    // they do for a file of sound and no pictures, which is never drawn.
    // Exit code 1, or the check that failed.
    let cases = [
        ("mci_hpal_8", true, avi_clip(), true),
        ("mci_hpal_24", false, avi_clip(), false),
        ("mci_hpal_sound", true, avi_sound(), false),
    ];
    for (test, desktop_palette, file, palette) in cases {
        let mut code = Code::new();
        let open = mci_string(&mut code, 2);
        code.emit(&[0x0E]); // push cs
        let alias = code.here() + 1;
        code.push(0).call("MMSYSTEM", MCI_GET_DEVICE_ID);
        expect_nonzero(&mut code, 3);
        code.emit(&[0xA3, lo(DEVICE), hi(DEVICE)]); // mov [DEVICE],ax
        let status = mci_string(&mut code, 4);
        // The reply's decimal number into AX.
        code.emit(&[0x33, 0xC0]) // xor ax,ax
            .emit(&[0xBE, lo(BUFFER), hi(BUFFER)]) // mov si,BUFFER
            .emit(&[0xB9, 10, 0]); // mov cx,10
        let top = code.here();
        code.emit(&[0x8A, 0x1C]) // mov bl,[si]
            .emit(&[0x0A, 0xDB]) // or bl,bl
            .emit(&[0x74, 0x0C]) // jz done
            .emit(&[0x80, 0xEB, b'0']) // sub bl,'0'
            .emit(&[0x32, 0xFF]) // xor bh,bh
            .emit(&[0xF7, 0xE1]) // mul cx
            .emit(&[0x03, 0xC3]) // add ax,bx
            .emit(&[0x46]); // inc si
        let back = top.wrapping_sub(code.here() + 2) as u8;
        code.emit(&[0xEB, back]); // jmp top
        code.emit(&[0xA3, lo(HWND), hi(HWND)]); // done: mov [HWND],ax
        // MCI_STATUS_PARMS of MCI_DGV_STATUS_HPAL.
        set(&mut code, STATUS, 0);
        set(&mut code, STATUS + 2, 0);
        set(&mut code, STATUS + 4, 0xEEEE);
        set(&mut code, STATUS + 6, 0xEEEE);
        set(&mut code, STATUS + 8, MCI_DGV_STATUS_HPAL);
        set(&mut code, STATUS + 10, 0);
        mci(&mut code, MCI_STATUS, MCI_STATUS_ITEM, STATUS, 0, 5);
        expect(&mut code, STATUS + 6, 0, true, 6);
        // mov ax,[HWND]; cmp ax,[STATUS+4]
        code.emit(&[0xA1, lo(HWND), hi(HWND)]);
        code.emit(&[0x3B, 0x06, lo(STATUS + 4), hi(STATUS + 4)]);
        code.emit(&[0x74, 0x06]) // je over the next
            .emit(&[0xB0, 7, 0xB4, 0x4C, 0xCD, 0x21]);
        if palette {
            // GetPaletteEntries(hpal, 0, 0, NULL): how many it has.
            push_word(&mut code, STATUS + 4);
            code.push(0).push(0).push(0).push(0);
            code.call("GDI", GET_PALETTE_ENTRIES);
            expect_ax(&mut code, 256, 8);
        } else {
            expect(&mut code, STATUS + 4, 0, true, 8);
            expect(&mut code, BUFFER, u16::from_le_bytes(*b"0\0"), true, 9);
        }
        mci(&mut code, MCI_CLOSE, 0, 0, 0, 10);
        code.emit(&[0xB0, 0x01]).exit_al();
        for (place, text) in [
            (open, &b"open CLIP.AVI alias c\0"[..]),
            (alias, b"c\0"),
            (status, b"status c palette handle\0"),
        ] {
            let at = code.here();
            code.emit(text);
            code.patch(place, at);
        }
        let (exit, log) = run_with(test, program(&code, CODE), |cpu, dir| {
            if desktop_palette {
                cpu.bus.win32.depth = rust_dos::config::DesktopDepth::Palette;
            }
            std::fs::write(dir.join("CLIP.AVI"), file).unwrap();
        });
        assert_eq!(exit, 1, "{test}: {log}");
    }
}

const CREATE_PALETTE: u16 = 360;
const WSPRINTF: u16 = 420;
const MCI_SETVIDEO: u16 = 0x876;
const MCI_DGV_SETVIDEO_ITEM: u32 = 0x10_0000;
const MCI_DGV_SETVIDEO_VALUE: u32 = 0x100_0000;
const MCI_DGV_SETVIDEO_PALHANDLE: u16 = 0x4007;

/// Where the MCI_SETVIDEO program keeps its MCI_DGV_SETVIDEO_PARMS, its
/// LOGPALETTE, the palette's handle and the command string it makes.
const SETVIDEO: u16 = 0x80;
const LOGPAL: u16 = 0xA0;
const HPAL: u16 = 0xB0;
const COMMAND: u16 = 0xC0;

#[test]
fn mci_setvideo_gives_an_avi_device_the_programs_palette() {
    // The Magic School Bus Explores the Solar System draws its scenes
    // and its movies through one identity palette: it sends
    // `setvideo <alias> palette handle to <hpal> wait`, made with
    // wsprintf, and `setaudio <alias> off wait`. MCIAVI then draws with
    // that palette, so `status palette handle` gives it, and closing the
    // device leaves it to the program. The built-in Windows didn't know
    // the commands (MCIERR_UNRECOGNIZED_COMMAND) and realized a palette
    // of its own in the program's window, which sent the program's to
    // the background. Exit code 1, or the check that failed.
    let mut code = Code::new();
    let open = mci_string(&mut code, 2);
    code.emit(&[0x0E]); // push cs
    let alias = code.here() + 1;
    code.push(0).call("MMSYSTEM", MCI_GET_DEVICE_ID);
    expect_nonzero(&mut code, 3);
    code.emit(&[0xA3, lo(DEVICE), hi(DEVICE)]); // mov [DEVICE],ax
    // CreatePalette of one colour.
    set(&mut code, LOGPAL, 0x300);
    set(&mut code, LOGPAL + 2, 1);
    set(&mut code, LOGPAL + 4, 0x2010);
    set(&mut code, LOGPAL + 6, 0x0030);
    code.emit(&[0x1E]).push(LOGPAL); // push ds
    code.call("GDI", CREATE_PALETTE);
    expect_nonzero(&mut code, 4);
    code.emit(&[0xA3, lo(HPAL), hi(HPAL)]); // mov [HPAL],ax
    // wsprintf(DS:COMMAND, the format, [HPAL]), cdecl.
    push_word(&mut code, HPAL);
    code.emit(&[0x0E]); // push cs
    let format = code.here() + 1;
    code.push(0).emit(&[0x1E]).push(COMMAND); // push ds
    code.call("USER", WSPRINTF);
    code.emit(&[0x83, 0xC4, 0x0A]); // add sp,10
    // mciSendString(DS:COMMAND, DS:BUFFER, 32, 0).
    code.emit(&[0x1E]).push(COMMAND); // push ds
    code.emit(&[0x1E]).push(BUFFER).push(32).push(0); // push ds
    code.call("MMSYSTEM", MCI_SEND_STRING);
    expect_ax(&mut code, 0, 5);
    // MCI_STATUS of MCI_DGV_STATUS_HPAL gives the program's palette.
    set(&mut code, STATUS, 0);
    set(&mut code, STATUS + 2, 0);
    set(&mut code, STATUS + 8, MCI_DGV_STATUS_HPAL);
    set(&mut code, STATUS + 10, 0);
    mci(&mut code, MCI_STATUS, MCI_STATUS_ITEM, STATUS, 0, 6);
    // mov ax,[HPAL]; cmp ax,[STATUS+4]
    code.emit(&[0xA1, lo(HPAL), hi(HPAL)]);
    code.emit(&[0x3B, 0x06, lo(STATUS + 4), hi(STATUS + 4)]);
    code.emit(&[0x74, 0x06]) // je over the next
        .emit(&[0xB0, 7, 0xB4, 0x4C, 0xCD, 0x21]);
    // MCI_DGV_SETVIDEO_PARMS through mciSendCommand: a handle of 0 gives
    // the device a palette of its own again, the halftone palette's 256
    // colours of a 24-bit movie, and the program's palette goes back in.
    for (value, fail) in [(None, 11), (Some(HPAL), 15)] {
        for k in 0..8 {
            set(&mut code, SETVIDEO + 2 * k, 0);
        }
        set(&mut code, SETVIDEO + 4, MCI_DGV_SETVIDEO_PALHANDLE);
        if let Some(at) = value {
            // mov ax,[at]; mov [SETVIDEO+8],ax
            code.emit(&[0xA1, lo(at), hi(at)]);
            code.emit(&[0xA3, lo(SETVIDEO + 8), hi(SETVIDEO + 8)]);
        }
        let flags = MCI_DGV_SETVIDEO_ITEM | MCI_DGV_SETVIDEO_VALUE;
        mci(&mut code, MCI_SETVIDEO, flags, SETVIDEO, 0, fail);
        mci(&mut code, MCI_STATUS, MCI_STATUS_ITEM, STATUS, 0, fail + 1);
        // mov ax,[HPAL]; cmp ax,[STATUS+4]
        code.emit(&[0xA1, lo(HPAL), hi(HPAL)]);
        code.emit(&[0x3B, 0x06, lo(STATUS + 4), hi(STATUS + 4)]);
        let same = if value.is_some() { 0x74 } else { 0x75 }; // je or jne
        code.emit(&[same, 0x06]) // over the next
            .emit(&[0xB0, fail + 2, 0xB4, 0x4C, 0xCD, 0x21]);
        if value.is_none() {
            // GetPaletteEntries(the device's palette, 0, 0, NULL).
            push_word(&mut code, STATUS + 4);
            code.push(0).push(0).push(0).push(0);
            code.call("GDI", GET_PALETTE_ENTRIES);
            expect_ax(&mut code, 256, fail + 3);
        }
    }
    let mute = mci_string(&mut code, 8);
    mci(&mut code, MCI_CLOSE, 0, 0, 0, 9);
    // The palette is still there: GetPaletteEntries(hpal, 0, 0, NULL).
    push_word(&mut code, HPAL);
    code.push(0).push(0).push(0).push(0);
    code.call("GDI", GET_PALETTE_ENTRIES);
    expect_ax(&mut code, 1, 10);
    code.emit(&[0xB0, 0x01]).exit_al();
    for (place, text) in [
        (open, &b"open CLIP.AVI alias c\0"[..]),
        (alias, b"c\0"),
        (format, b"setvideo c palette handle to %u wait\0"),
        (mute, b"setaudio c off wait\0"),
    ] {
        let at = code.here();
        code.emit(text);
        code.patch(place, at);
    }
    let (exit, log) = run_with("mci_setvideo", program(&code, CODE), |cpu, dir| {
        cpu.bus.win32.depth = rust_dos::config::DesktopDepth::Palette;
        std::fs::write(dir.join("CLIP.AVI"), avi_clip()).unwrap();
    });
    assert_eq!(exit, 1, "{log}");
}

const MMIO_OPEN: u16 = 1210;
const MMIO_CLOSE: u16 = 1211;
const MMIO_READ: u16 = 1212;
const MMIO_INSTALL_IO_PROC: u16 = 1221;
const GET_SELECTOR_LIMIT: u16 = 188;

/// Where the I/O procedure test keeps things: the file's name, its
/// handle, what was read, the first byte of the name the procedure got,
/// how many times it was closed, and the selector of the buffer its last
/// MMIOM_READ got.
const IO_NAME: u16 = 0x40;
const IO_HANDLE: u16 = 0x50;
const IO_BUFFER: u16 = 0x60;
const IO_NAME0: u16 = 0x70;
const IO_CLOSES: u16 = 0x72;
const IO_SEL: u16 = 0x74;

/// A 16-bit I/O procedure, IOProc(lpmmioinfo, wMsg, lParam1, lParam2),
/// Pascal's: MMIOM_OPEN keeps the name's first byte at IO_NAME0 and
/// starts at 0 (adwInfo[0] is the place), MMIOM_READ gives "HELLO" at
/// the place 0 and nothing after, moving lDiskOffset, keeping the
/// buffer's selector at IO_SEL, and MMIOM_CLOSE counts at IO_CLOSES. DS
/// is the program's, the stack's.
fn io_proc() -> Vec<u8> {
    let mut c: Vec<u8> = Vec::new();
    let mut to_ret0 = Vec::new();
    c.extend([0x55, 0x8B, 0xEC]); // push bp; mov bp,sp
    c.extend([0x8B, 0x46, 0x0E]); // mov ax,[bp+0Eh]: wMsg
    c.extend([0x3D, 0x03, 0x00, 0x75, 0x00]); // cmp ax,3 (MMIOM_OPEN); jne
    let not_open = c.len() - 1;
    c.extend([0xC4, 0x5E, 0x10]); // les bx,[bp+10h]: the MMIOINFO
    c.extend([0x26, 0xC7, 0x47, 0x2C, 0x00, 0x00]); // mov word es:[bx+2Ch],0
    c.extend([0xC4, 0x5E, 0x0A]); // les bx,[bp+0Ah]: the name
    c.extend([0x26, 0x8A, 0x07]); // mov al,es:[bx]
    c.extend([0xA2, lo(IO_NAME0), hi(IO_NAME0)]); // mov [IO_NAME0],al
    c.extend([0xEB, 0x00]); // jmp ret0
    to_ret0.push(c.len() - 1);
    c[not_open] = (c.len() - not_open - 1) as u8;
    c.extend([0x3D, 0x04, 0x00, 0x75, 0x06]); // cmp ax,4 (MMIOM_CLOSE); jne
    c.extend([0xFF, 0x06, lo(IO_CLOSES), hi(IO_CLOSES)]); // inc word [IO_CLOSES]
    c.extend([0xEB, 0x00]); // jmp ret0
    to_ret0.push(c.len() - 1);
    c.extend([0x85, 0xC0, 0x75, 0x00]); // test ax,ax (MMIOM_READ); jnz ret0
    to_ret0.push(c.len() - 1);
    c.extend([0x8B, 0x46, 0x0C]); // mov ax,[bp+0Ch]: the buffer's selector
    c.extend([0xA3, lo(IO_SEL), hi(IO_SEL)]); // mov [IO_SEL],ax
    c.extend([0xC4, 0x5E, 0x10]); // les bx,[bp+10h]
    c.extend([0x26, 0x83, 0x7F, 0x2C, 0x00, 0x75, 0x00]); // cmp word es:[bx+2Ch],0; jne ret0
    to_ret0.push(c.len() - 1);
    c.extend([0xC4, 0x7E, 0x0A]); // les di,[bp+0Ah]: the buffer
    c.extend([0x26, 0xC7, 0x05, b'H', b'E']); // mov word es:[di],"HE"
    c.extend([0x26, 0xC7, 0x45, 0x02, b'L', b'L']); // mov word es:[di+2],"LL"
    c.extend([0x26, 0xC6, 0x45, 0x04, b'O']); // mov byte es:[di+4],'O'
    c.extend([0xC4, 0x5E, 0x10]); // les bx,[bp+10h]
    c.extend([0x26, 0xC7, 0x47, 0x2C, 0x05, 0x00]); // mov word es:[bx+2Ch],5
    c.extend([0x26, 0x83, 0x47, 0x28, 0x05]); // add word es:[bx+28h],5: lDiskOffset
    c.extend([0xB8, 0x05, 0x00, 0x31, 0xD2, 0xEB, 0x04]); // mov ax,5; xor dx,dx; jmp done
    let ret0 = c.len();
    for at in to_ret0 {
        c[at] = (ret0 - at - 1) as u8;
    }
    c.extend([0x31, 0xC0, 0x31, 0xD2]); // xor ax,ax; xor dx,dx
    c.extend([0x5D, 0xCA, 0x0E, 0x00]); // done: pop bp; retf 14
    c
}

/// A Windows 3.1 program installs its own I/O procedure for 'TST ' and
/// opens "X.TST+A": mmio calls the 16-bit procedure with the name and a
/// 16-bit MMIOINFO, reads through it, and closes through it, and
/// MMIO_FINDPROC gives the 16-bit procedure back. The selector a read's
/// buffer is passed with is free once the read returns. MMSYSTEM had no
/// mmioInstallIOProc, and a program that imports it, as The Magic School
/// Bus Explores the Solar System's MSPAM.DLL does to read its sounds from
/// its compound file, ended at its first call.
#[test]
fn a_programs_own_io_procedure_reads_its_files() {
    let fcc = u32::from_le_bytes(*b"TST ");
    let mut code = Code::new();
    let mut proc_at = Vec::new();
    set(&mut code, IO_CLOSES, 0);
    set(&mut code, IO_SEL, 0);
    // mmioInstallIOProc('TST ', proc, MMIO_INSTALLPROC).
    code.push((fcc >> 16) as u16).push(fcc as u16);
    code.emit(&[0x0E]);
    proc_at.push(code.here() + 1);
    code.push(0).push(1).push(0);
    code.call("MMSYSTEM", MMIO_INSTALL_IO_PROC);
    expect_nonzero(&mut code, 1);
    // mmioOpen("X.TST+A", NULL, MMIO_READ).
    for (i, pair) in b"X.TST+A\0".chunks(2).enumerate() {
        let word = u16::from_le_bytes([pair[0], pair[1]]);
        set(&mut code, IO_NAME + 2 * i as u16, word);
    }
    code.emit(&[0x1E])
        .push(IO_NAME)
        .push(0)
        .push(0)
        .push(0)
        .push(0);
    code.call("MMSYSTEM", MMIO_OPEN);
    expect_nonzero(&mut code, 2);
    code.emit(&[0xA3, lo(IO_HANDLE), hi(IO_HANDLE)]); // mov [IO_HANDLE],ax
    expect(&mut code, IO_NAME0, u16::from(b'X'), true, 3);
    // GetSelectorLimit(the read's selector): 0, as it is free.
    expect(&mut code, IO_SEL, 0, false, 11);
    push_word(&mut code, IO_SEL);
    code.call("KERNEL", GET_SELECTOR_LIMIT);
    expect_ax(&mut code, 0, 10);
    // mmioRead(hmmio, IO_BUFFER, 5).
    push_word(&mut code, IO_HANDLE);
    code.emit(&[0x1E]).push(IO_BUFFER).push(0).push(5);
    code.call("MMSYSTEM", MMIO_READ);
    expect_ax(&mut code, 5, 4);
    expect(&mut code, IO_BUFFER, u16::from_le_bytes(*b"HE"), true, 5);
    expect(&mut code, IO_BUFFER + 4, u16::from(b'O'), true, 6);
    // mmioClose(hmmio, 0).
    push_word(&mut code, IO_HANDLE);
    code.push(0);
    code.call("MMSYSTEM", MMIO_CLOSE);
    expect_ax(&mut code, 0, 7);
    expect(&mut code, IO_CLOSES, 1, true, 8);
    // mmioInstallIOProc('TST ', NULL, MMIO_FINDPROC): the procedure.
    code.push((fcc >> 16) as u16).push(fcc as u16);
    code.push(0).push(0).push(4).push(0);
    code.call("MMSYSTEM", MMIO_INSTALL_IO_PROC);
    proc_at.push(code.here() + 1);
    code.emit(&[0x3D, 0, 0]); // cmp ax,proc
    code.emit(&[0x74, 0x06, 0xB0, 9, 0xB4, 0x4C, 0xCD, 0x21]);
    code.emit(&[0xB0, 0x00]).exit_al();
    let proc = code.here();
    code.emit(&io_proc());
    for at in proc_at {
        code.patch(at, proc);
    }
    let (exit, log) = run("mmio_io_proc16", program(&code, CODE));
    assert_eq!(exit, 0, "check {exit} failed\n{log}");
}
