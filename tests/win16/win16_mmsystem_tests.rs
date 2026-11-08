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
fn avi_clip() -> Vec<u8> {
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
