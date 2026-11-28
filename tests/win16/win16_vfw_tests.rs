//! Win16 Video for Windows: the built-in AVIFILE.DLL and MSVIDEO.DLL of
//! Windows 98 SE, which go on to AVIFIL32 and MSVFW32. The programs are
//! made byte by byte (tests/common/win16).

use crate::win16;
use crate::win16_mmsystem_tests::avi_clip;

use win16::{CODE, Code, program, run_with};

const AVI_FILE_INIT: u16 = 100;
const AVI_FILE_EXIT: u16 = 101;
const AVI_FILE_OPEN: u16 = 102;
const AVI_FILE_RELEASE: u16 = 141;
const AVI_FILE_GET_STREAM: u16 = 143;
const AVI_STREAM_RELEASE: u16 = 161;
const AVI_STREAM_INFO: u16 = 162;
const VIDEO_FOR_WINDOWS_VERSION: u16 = 2;

/// Where the program keeps things in its data segment: the IAVIFile and
/// IAVIStream pointers, and the AVISTREAMINFO with a word after it.
const FILE: u16 = 0x10;
const STREAM: u16 = 0x14;
const INFO: u16 = 0x100;
/// The 16-bit AVISTREAMINFO's size: its rcFrame is a RECT of words.
const INFO_SIZE: u16 = 0x84;

fn lo(v: u16) -> u8 {
    v as u8
}

fn hi(v: u16) -> u8 {
    (v >> 8) as u8
}

/// End with `fail` unless the last comparison found them equal.
fn or_fail(code: &mut Code, fail: u8) {
    code.emit(&[0x74, 0x06]) // je over the exit
        .emit(&[0xB0, fail, 0xB4, 0x4C, 0xCD, 0x21]);
}

/// End with `fail` unless DX:AX is `value`.
fn expect_dx_ax(code: &mut Code, value: u32, fail: u8) {
    let (l, h) = (value as u16, (value >> 16) as u16);
    code.emit(&[0x3D, lo(l), hi(l)]);
    or_fail(code, fail);
    code.emit(&[0x81, 0xFA, lo(h), hi(h)]);
    or_fail(code, fail);
}

/// End with `fail` unless the word at `at` is `value`.
fn expect_word(code: &mut Code, at: u16, value: u16, fail: u8) {
    code.emit(&[0x81, 0x3E, lo(at), hi(at), lo(value), hi(value)]);
    or_fail(code, fail);
}

/// push dword [at].
fn push_dword(code: &mut Code, at: u16) {
    let high = at + 2;
    code.emit(&[0xFF, 0x36, lo(high), hi(high)])
        .emit(&[0xFF, 0x36, lo(at), hi(at)]);
}

/// A program opens an AVI file with the built-in AVIFILE, gets its video
/// stream and its AVISTREAMINFO in the 16-bit layout: 132 bytes, with
/// rcFrame as four words, and nothing written after them. MSVIDEO's
/// VideoForWindowsVersion is 98 SE's, 4.03.1998. The program ends with 1,
/// or the number of the check that failed.
#[test]
fn avifile_gives_a_streams_info_in_the_16_bit_layout() {
    let mut code = Code::new();
    code.call("AVIFILE", AVI_FILE_INIT);
    // AVIFileOpen(&file, "CLIP.AVI", OF_READ, NULL)
    code.emit(&[0x1E]).push(FILE);
    let name = code.here() + 2;
    code.push_far(0).push(0).push(0).push(0);
    code.call("AVIFILE", AVI_FILE_OPEN);
    expect_dx_ax(&mut code, 0, 2);
    // AVIFileGetStream(file, &stream, streamtypeVIDEO, 0)
    push_dword(&mut code, FILE);
    code.emit(&[0x1E]).push(STREAM);
    code.push(0x7364).push(0x6976).push(0).push(0);
    code.call("AVIFILE", AVI_FILE_GET_STREAM);
    expect_dx_ax(&mut code, 0, 3);
    // mov word [INFO+INFO_SIZE],0BEEFh
    let after = INFO + INFO_SIZE;
    code.emit(&[0xC7, 0x06, lo(after), hi(after), 0xEF, 0xBE]);
    // AVIStreamInfo(stream, &info, sizeof info)
    push_dword(&mut code, STREAM);
    code.emit(&[0x1E]).push(INFO).push(0).push(INFO_SIZE);
    code.call("AVIFILE", AVI_STREAM_INFO);
    expect_dx_ax(&mut code, 0, 4);
    expect_word(&mut code, INFO, 0x6976, 5); // fccType "vids"
    expect_word(&mut code, INFO + 2, 0x7364, 6);
    expect_word(&mut code, INFO + 32, 12, 7); // dwLength
    expect_word(&mut code, INFO + 56, 32, 8); // rcFrame.right
    expect_word(&mut code, INFO + 58, 32, 9); // rcFrame.bottom
    expect_word(&mut code, INFO + 60, 0, 10); // dwEditCount
    expect_word(&mut code, after, 0xBEEF, 11);
    push_dword(&mut code, STREAM);
    code.call("AVIFILE", AVI_STREAM_RELEASE);
    push_dword(&mut code, FILE);
    code.call("AVIFILE", AVI_FILE_RELEASE);
    code.call("AVIFILE", AVI_FILE_EXIT);
    code.call("MSVIDEO", VIDEO_FOR_WINDOWS_VERSION);
    expect_dx_ax(&mut code, 0x0403_07CE, 12);
    code.emit(&[0xB0, 0x01]).exit_al();
    let name_at = code.here();
    code.emit(b"CLIP.AVI\0");
    code.patch(name, name_at);
    let (exit, log) = run_with("vfw16_stream_info", program(&code, CODE), |_, dir| {
        std::fs::write(dir.join("CLIP.AVI"), avi_clip()).unwrap();
    });
    assert_eq!(exit, 1, "{log}");
}
