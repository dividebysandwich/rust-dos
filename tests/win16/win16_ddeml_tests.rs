//! Win16 DDEML: a Windows 3.1 program talks to Program Manager through
//! DDEML on the built-in Windows, as a 16-bit setup program makes its
//! group (Delphi's TDdeClientConv, This Means War!'s setup). The
//! programs are made byte by byte (tests/common/win16).

use crate::win16;

use std::fs;
use std::path::{Path, PathBuf};
use win16::{CODE, Code, program, run_with};

/// DDEML's ordinals, and USER's the programs call.
const DDE_INITIALIZE: u16 = 2;
const DDE_UNINITIALIZE: u16 = 3;
const DDE_CONNECT: u16 = 7;
const DDE_DISCONNECT: u16 = 8;
const DDE_QUERY_CONV_INFO: u16 = 9;
const DDE_SET_USER_HANDLE: u16 = 10;
const DDE_CLIENT_TRANSACTION: u16 = 11;
const DDE_CREATE_DATA_HANDLE: u16 = 14;
const DDE_GET_DATA: u16 = 16;
const DDE_ACCESS_DATA: u16 = 17;
const DDE_UNACCESS_DATA: u16 = 18;
const DDE_FREE_DATA_HANDLE: u16 = 19;
const DDE_CREATE_STRING_HANDLE: u16 = 21;
const GET_MESSAGE: u16 = 108;
const DISPATCH_MESSAGE: u16 = 114;

const APPCMD_CLIENTONLY: u16 = 0x10;
const CP_WINANSI: u16 = 1004;
const CF_TEXT: u16 = 1;
const XTYP_EXECUTE: u16 = 0x4050;
const XTYP_REQUEST: u16 = 0x20B0;
const XTYP_XACT_COMPLETE: u16 = 0x8080;
const DDE_FACK: u16 = 0x8000;
/// Windows 3.1's CONVINFO's size, hwnd and hwndPartner included.
const CONVINFO_SIZE: u16 = 62;

/// Where the program keeps things in its data segment: the instance,
/// string handle, conversation, data handle and transaction result (a
/// DWORD each); what the callback saw (a flag, then the transaction id,
/// data and status, and what DdeQueryConvInfo gave); a request's item; a
/// MSG; a CONVINFO; a DWORD of size and a buffer.
const INST: u16 = 0x10;
const HSZ: u16 = 0x14;
const CONV: u16 = 0x18;
const HDATA: u16 = 0x1C;
const RESULT: u16 = 0x20;
const DONE: u16 = 0x24;
const XID: u16 = 0x28;
const XDATA: u16 = 0x2C;
const XSTATUS: u16 = 0x30;
const QUERIED: u16 = 0x34;
const ITEM: u16 = 0x38;
const MSG: u16 = 0x40;
const INFO: u16 = 0x60;
const SIZE: u16 = 0xA0;
const BUF: u16 = 0xA4;

/// The commands, Program Manager's: an asynchronous execute of a data
/// handle, then a synchronous one of a pointer.
const MAKE_GROUP: &[u8] = b"[CreateGroup(Dde16)][AddItem(C:\\T.EXE,Tool)]\0";
const SHOW_GROUP: &[u8] = b"[ShowGroup(Dde16,1)]\0";

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

/// push dword [at], high word first.
fn push_long(code: &mut Code, at: u16) {
    code.emit(&[0xFF, 0x36, lo(at + 2), hi(at + 2)])
        .emit(&[0xFF, 0x36, lo(at), hi(at)]);
}

/// push imm32, high word first.
fn push_dword(code: &mut Code, value: u32) {
    code.push((value >> 16) as u16).push(value as u16);
}

/// mov [at],ax and mov [at+2],dx.
fn store_dx_ax(code: &mut Code, at: u16) {
    code.emit(&[0xA3, lo(at), hi(at)])
        .emit(&[0x89, 0x16, lo(at + 2), hi(at + 2)]);
}

/// End with `fail` unless the word at `at` is `value`.
fn expect(code: &mut Code, at: u16, value: u16, fail: u8) {
    code.emit(&[0x81, 0x3E, lo(at), hi(at), lo(value), hi(value)])
        .emit(&[0x74, 0x06]) // je over the next
        .emit(&[0xB0, fail, 0xB4, 0x4C, 0xCD, 0x21]); // mov al,fail; exit
}

/// End with `fail` unless the words at `a` and `b` are equal.
fn expect_same(code: &mut Code, a: u16, b: u16, fail: u8) {
    code.emit(&[0xA1, lo(a), hi(a)]) // mov ax,[a]
        .emit(&[0x3B, 0x06, lo(b), hi(b)]) // cmp ax,[b]
        .emit(&[0x74, 0x06])
        .emit(&[0xB0, fail, 0xB4, 0x4C, 0xCD, 0x21]);
}

/// End with `fail` if AX is 0.
fn expect_ax_nonzero(code: &mut Code, fail: u8) {
    code.emit(&[0x0B, 0xC0]) // or ax,ax
        .emit(&[0x75, 0x06])
        .emit(&[0xB0, fail, 0xB4, 0x4C, 0xCD, 0x21]);
}

/// End with `fail` if DX:AX is 0.
fn expect_long_nonzero(code: &mut Code, fail: u8) {
    code.emit(&[0x8B, 0xC8]) // mov cx,ax
        .emit(&[0x0B, 0xCA]) // or cx,dx
        .emit(&[0x75, 0x06])
        .emit(&[0xB0, fail, 0xB4, 0x4C, 0xCD, 0x21]);
}

/// The DDE callback, Pascal's: (type, format, conversation, hsz1, hsz2,
/// data, data1, data2), 28 bytes of arguments. For XTYP_XACT_COMPLETE
/// it keeps the transaction's id, data and status, asks
/// DdeQueryConvInfo of the conversation into INFO and sets DONE. It
/// returns 0.
fn callback(code: &mut Code) -> u16 {
    let at = code.here();
    code.emit(&[0x55, 0x8B, 0xEC]) // push bp; mov bp,sp
        // cmp word [bp+20h],XTYP_XACT_COMPLETE; jne end
        .emit(&[
            0x81,
            0x7E,
            0x20,
            lo(XTYP_XACT_COMPLETE),
            hi(XTYP_XACT_COMPLETE),
        ]);
    code.emit(&[0x0F, 0x85]);
    let jump = code.here();
    code.emit(&[0, 0]);
    // dwData1, hData and dwData2's low word.
    for (from, to) in [
        (10u8, XID),
        (12, XID + 2),
        (14, XDATA),
        (16, XDATA + 2),
        (6, XSTATUS),
    ] {
        code.emit(&[0x8B, 0x46, from]) // mov ax,[bp+from]
            .emit(&[0xA3, lo(to), hi(to)]); // mov [to],ax
    }
    set(code, INFO, CONVINFO_SIZE);
    set(code, INFO + 2, 0);
    code.emit(&[0xFF, 0x76, 0x1C]) // push word [bp+1Ch]: hConv's high word
        .emit(&[0xFF, 0x76, 0x1A]); // push word [bp+1Ah]
    push_dword(code, 0xFFFF_FFFF); // QID_SYNC
    code.emit(&[0x1E]) // push ds
        .push(INFO)
        .call("DDEML", DDE_QUERY_CONV_INFO)
        .emit(&[0xA3, lo(QUERIED), hi(QUERIED)]);
    set(code, DONE, 1);
    let end = code.here();
    code.patch(jump, end - (jump + 2));
    code.emit(&[0x33, 0xC0, 0x33, 0xD2]) // xor ax,ax; xor dx,dx
        .emit(&[0x5D, 0xCA, 28, 0]); // pop bp; retf 28
    at
}

/// The host path of `path` (backslashes) on the drive C: at `base`,
/// whatever the case of its names on the host.
fn host_file(base: &Path, path: &str) -> Option<PathBuf> {
    let mut at = base.to_path_buf();
    for part in path.split('\\') {
        let found = fs::read_dir(&at)
            .ok()?
            .filter_map(Result::ok)
            .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(part))?;
        at = found.path();
    }
    Some(at)
}

#[test]
fn a_16_bit_program_makes_its_group_through_ddeml() {
    // DdeInitialize, a conversation with PROGMAN, an asynchronous execute
    // of a data handle whose XTYP_XACT_COMPLETE comes to the callback
    // from the message loop, a synchronous execute of a pointer, and a
    // request whose data DdeAccessData gives a 16:16 pointer to. Exit
    // code 1, or which check failed.
    let mut code = Code::new();
    // Jump over the callback and the strings.
    code.emit(&[0xE9]);
    let skip = code.here();
    code.emit(&[0, 0]);
    let proc = callback(&mut code);
    let name = code.here();
    code.emit(b"PROGMAN\0");
    let make = code.here();
    code.emit(MAKE_GROUP);
    let show = code.here();
    code.emit(SHOW_GROUP);
    let group = code.here();
    code.emit(b"Dde16\0");
    let start = code.here();
    code.patch(skip, start - (skip + 2));

    set(&mut code, INST, 0);
    set(&mut code, INST + 2, 0);
    set(&mut code, DONE, 0);
    // DdeInitialize(&INST, callback, APPCMD_CLIENTONLY, 0).
    code.emit(&[0x1E]).push(INST).push_far(proc);
    push_dword(&mut code, APPCMD_CLIENTONLY as u32);
    push_dword(&mut code, 0);
    code.call("DDEML", DDE_INITIALIZE);
    code.emit(&[0x3D, 0, 0, 0x74, 0x06]) // cmp ax,0; je over the next
        .emit(&[0xB0, 2, 0xB4, 0x4C, 0xCD, 0x21]);
    // DdeCreateStringHandle(INST, "PROGMAN", CP_WINANSI).
    push_long(&mut code, INST);
    code.push_far(name).push(CP_WINANSI);
    code.call("DDEML", DDE_CREATE_STRING_HANDLE);
    store_dx_ax(&mut code, HSZ);
    expect_long_nonzero(&mut code, 3);
    // DdeConnect(INST, HSZ, HSZ, NULL).
    push_long(&mut code, INST);
    push_long(&mut code, HSZ);
    push_long(&mut code, HSZ);
    push_dword(&mut code, 0);
    code.call("DDEML", DDE_CONNECT);
    store_dx_ax(&mut code, CONV);
    expect_long_nonzero(&mut code, 4);
    // DdeSetUserHandle(CONV, QID_SYNC, 12345678h).
    push_long(&mut code, CONV);
    push_dword(&mut code, 0xFFFF_FFFF);
    push_dword(&mut code, 0x1234_5678);
    code.call("DDEML", DDE_SET_USER_HANDLE);
    expect_ax_nonzero(&mut code, 5);
    // DdeCreateDataHandle(INST, MAKE_GROUP, its size, 0, 0, CF_TEXT, 0).
    push_long(&mut code, INST);
    code.push_far(make);
    push_dword(&mut code, MAKE_GROUP.len() as u32);
    push_dword(&mut code, 0);
    push_dword(&mut code, 0);
    code.push(CF_TEXT).push(0);
    code.call("DDEML", DDE_CREATE_DATA_HANDLE);
    store_dx_ax(&mut code, HDATA);
    expect_long_nonzero(&mut code, 6);
    // DdeClientTransaction(HDATA, -1, CONV, 0, CF_TEXT, XTYP_EXECUTE,
    // TIMEOUT_ASYNC, &RESULT): TRUE, and the id in RESULT.
    push_long(&mut code, HDATA);
    push_dword(&mut code, 0xFFFF_FFFF);
    push_long(&mut code, CONV);
    push_dword(&mut code, 0);
    code.push(CF_TEXT).push(XTYP_EXECUTE);
    push_dword(&mut code, 0xFFFF_FFFF);
    code.emit(&[0x1E]).push(RESULT);
    code.call("DDEML", DDE_CLIENT_TRANSACTION);
    expect_ax_nonzero(&mut code, 7);
    // The message loop, until the callback has run.
    let top = code.here();
    code.emit(&[0x83, 0x3E, lo(DONE), hi(DONE), 0x00]) // cmp word [DONE],0
        .emit(&[0x75, 0x00]); // jne out
    let out = code.here();
    code.emit(&[0x1E]).push(MSG).push(0).push(0).push(0);
    code.call("USER", GET_MESSAGE);
    code.emit(&[0x1E]).push(MSG).call("USER", DISPATCH_MESSAGE);
    let back = code.here() + 3;
    code.emit(&[0xE9])
        .emit(&(top.wrapping_sub(back)).to_le_bytes());
    let after = code.here();
    code.bytes[out as usize - 1] = (after - out) as u8;
    // XTYP_XACT_COMPLETE: the transaction's id, TRUE for its data, and
    // DDE_FACK.
    expect_same(&mut code, XID, RESULT, 8);
    expect_same(&mut code, XID + 2, RESULT + 2, 8);
    expect(&mut code, XDATA, 1, 9);
    expect(&mut code, XDATA + 2, 0, 9);
    expect(&mut code, XSTATUS, DDE_FACK, 10);
    // DdeQueryConvInfo in the callback: all of the CONVINFO, the user
    // handle, and the topic.
    expect(&mut code, QUERIED, CONVINFO_SIZE, 11);
    expect(&mut code, INFO + 4, 0x5678, 12);
    expect(&mut code, INFO + 6, 0x1234, 12);
    expect_same(&mut code, INFO + 20, HSZ, 13);
    expect_same(&mut code, INFO + 22, HSZ + 2, 13);
    // DdeClientTransaction(SHOW_GROUP, its size, CONV, 0, CF_TEXT,
    // XTYP_EXECUTE, 1000, &RESULT): TRUE, and DDE_FACK in RESULT.
    code.push_far(show);
    push_dword(&mut code, SHOW_GROUP.len() as u32);
    push_long(&mut code, CONV);
    push_dword(&mut code, 0);
    code.push(CF_TEXT).push(XTYP_EXECUTE);
    push_dword(&mut code, 1000);
    code.emit(&[0x1E]).push(RESULT);
    code.call("DDEML", DDE_CLIENT_TRANSACTION);
    expect_ax_nonzero(&mut code, 14);
    expect(&mut code, RESULT, DDE_FACK, 15);
    // A request for the group's items: "Tool\r\n", in the case the drive
    // keeps the link's name in.
    push_long(&mut code, INST);
    code.push_far(group).push(CP_WINANSI);
    code.call("DDEML", DDE_CREATE_STRING_HANDLE);
    store_dx_ax(&mut code, ITEM);
    push_dword(&mut code, 0); // no data
    push_dword(&mut code, 0);
    push_long(&mut code, CONV);
    push_long(&mut code, ITEM);
    code.push(CF_TEXT).push(XTYP_REQUEST);
    push_dword(&mut code, 1000);
    code.emit(&[0x1E]).push(RESULT);
    code.call("DDEML", DDE_CLIENT_TRANSACTION);
    store_dx_ax(&mut code, HDATA);
    expect_long_nonzero(&mut code, 16);
    // DdeAccessData(HDATA, &SIZE): a 16:16 pointer to "Tool", which the
    // program changes to "Xool".
    push_long(&mut code, HDATA);
    code.emit(&[0x1E]).push(SIZE);
    code.call("DDEML", DDE_ACCESS_DATA);
    expect_long_nonzero(&mut code, 17);
    expect(&mut code, SIZE, 7, 18);
    code.emit(&[0x8E, 0xC2]) // mov es,dx
        .emit(&[0x8B, 0xD8]) // mov bx,ax
        .emit(&[0x26, 0x8B, 0x07]) // mov ax,es:[bx]
        .emit(&[0x0D, 0x20, 0x20]) // or ax,2020h: lower case
        .emit(&[0x3D, b't', b'o']) // cmp ax,'to'
        .emit(&[0x74, 0x06])
        .emit(&[0xB0, 19, 0xB4, 0x4C, 0xCD, 0x21])
        .emit(&[0x26, 0xC6, 0x07, b'X']); // mov byte es:[bx],'X'
    // DdeUnaccessData puts the change back, which DdeGetData reads.
    push_long(&mut code, HDATA);
    code.call("DDEML", DDE_UNACCESS_DATA);
    expect_ax_nonzero(&mut code, 20);
    push_long(&mut code, HDATA);
    code.emit(&[0x1E]).push(BUF);
    push_dword(&mut code, 2);
    push_dword(&mut code, 0);
    code.call("DDEML", DDE_GET_DATA);
    code.emit(&[0x3D, 2, 0, 0x74, 0x06])
        .emit(&[0xB0, 21, 0xB4, 0x4C, 0xCD, 0x21]);
    code.emit(&[0xA1, lo(BUF), hi(BUF)]) // mov ax,[BUF]
        .emit(&[0x0D, 0x00, 0x20]); // or ax,2000h: the second letter lower case
    code.emit(&[0x3D, b'X', b'o', 0x74, 0x06])
        .emit(&[0xB0, 22, 0xB4, 0x4C, 0xCD, 0x21]);
    push_long(&mut code, HDATA);
    code.call("DDEML", DDE_FREE_DATA_HANDLE);
    expect_ax_nonzero(&mut code, 23);
    push_long(&mut code, CONV);
    code.call("DDEML", DDE_DISCONNECT);
    expect_ax_nonzero(&mut code, 24);
    push_long(&mut code, INST);
    code.call("DDEML", DDE_UNINITIALIZE);
    expect_ax_nonzero(&mut code, 25);
    code.emit(&[0xB0, 0x01]).exit_al();

    let mut base = PathBuf::new();
    let (exit, log) = run_with("ddeml_group", program(&code, CODE), |_, dir| {
        base = dir.to_path_buf();
    });
    assert_eq!(exit, 1, "{log}");
    let item = "WINDOWS\\Start Menu\\Programs\\Dde16\\Tool.lnk";
    let link = fs::read(host_file(&base, item).expect(item)).unwrap();
    assert_eq!(&link[..4], &[0x4C, 0, 0, 0]);
}
