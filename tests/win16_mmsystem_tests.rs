//! Win16 MMSYSTEM: what a Windows 3.1 program gets back from the
//! multimedia functions on the built-in Windows. The programs are made
//! byte by byte (tests/win16).

mod win16;

use win16::{CODE, Code, program, run};

/// MMSYSTEM's ordinals.
const WAVE_OUT_OPEN: u16 = 404;
const WAVE_OUT_CLOSE: u16 = 405;
const WAVE_OUT_GET_POSITION: u16 = 412;
const TIME_GET_SYSTEM_TIME: u16 = 601;
const TIME_GET_DEV_CAPS: u16 = 604;

/// Where the programs keep things in their data segment: an MMTIME at
/// 40h and the words after it, a wave device's handle, a WAVEFORMAT.
const TIME: u16 = 0x40;
const HANDLE: u16 = 0x50;
const FORMAT: u16 = 0x60;

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
    // PCMWAVEFORMAT: PCM, mono, 11025 Hz, 11025 bytes a second, 1-byte
    // blocks of 8 bits.
    for (i, word) in [1u16, 1, 11025, 0, 11025, 0, 1, 8].into_iter().enumerate() {
        set(&mut code, FORMAT + 2 * i as u16, word);
    }
    code.emit(&[0x1E]) // push ds
        .push(HANDLE)
        .push(0xFFFF) // WAVE_MAPPER
        .emit(&[0x1E])
        .push(FORMAT)
        .push(0)
        .push(0) // no callback
        .push(0)
        .push(0) // instance
        .push(0)
        .push(0) // flags
        .call("MMSYSTEM", WAVE_OUT_OPEN);
    expect_ax(&mut code, 0, 7);
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
