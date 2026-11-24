//! The 16-bit registry functions of KERNEL and SHELL on the built-in
//! Windows 98: the one registry Win32 programs use, with a Windows 3.1
//! program's HKEY_CLASSES_ROOT, 1. The programs are made byte by byte
//! (tests/common/win16) and end with the number of the first check that
//! failed, 0 when all held.

use crate::win16;

use std::fs;
use win16::{CODE, Code, program, run, run_with};

/// SHELL's ordinals, and KERNEL's RegQueryValueEx.
const REG_OPEN_KEY: u16 = 1;
const REG_CREATE_KEY: u16 = 2;
const REG_CLOSE_KEY: u16 = 3;
const REG_SET_VALUE: u16 = 5;
const REG_QUERY_VALUE: u16 = 6;
const REG_ENUM_KEY: u16 = 7;
const KERNEL_REG_QUERY_VALUE_EX: u16 = 225;

const HKEY_CLASSES_ROOT: u16 = 1;
const REG_SZ: u16 = 1;

/// Where the program keeps things in its data segment: two keys, a
/// size, a value's type, and a buffer.
const KEY: u16 = 0x10;
const KEY2: u16 = 0x14;
const SIZE: u16 = 0x18;
const TYPE: u16 = 0x1C;
const BUFFER: u16 = 0x40;

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

/// End with `fail` unless DX:AX is 0 (ERROR_SUCCESS).
fn expect_success(code: &mut Code, fail: u8) {
    code.emit(&[0x0B, 0xC2]) // or ax,dx
        .emit(&[0x74, 0x06]) // jz over the exit
        .emit(&[0xB0, fail, 0xB4, 0x4C, 0xCD, 0x21]);
}

/// End with `fail` unless the byte at `at` is `value`.
fn expect_byte(code: &mut Code, at: u16, value: u8, fail: u8) {
    code.emit(&[0x80, 0x3E, lo(at), hi(at), value])
        .emit(&[0x74, 0x06])
        .emit(&[0xB0, fail, 0xB4, 0x4C, 0xCD, 0x21]);
}

/// End with `fail` unless the word at `at` is `value`.
fn expect(code: &mut Code, at: u16, value: u16, fail: u8) {
    code.emit(&[0x81, 0x3E, lo(at), hi(at), lo(value), hi(value)])
        .emit(&[0x74, 0x06])
        .emit(&[0xB0, fail, 0xB4, 0x4C, 0xCD, 0x21]);
}

/// push dword [at].
fn push_dword(code: &mut Code, at: u16) {
    code.emit(&[0xFF, 0x36, lo(at + 2), hi(at + 2)])
        .emit(&[0xFF, 0x36, lo(at), hi(at)]);
}

/// push the 32-bit value `value`.
fn push_long(code: &mut Code, value: u32) {
    code.push((value >> 16) as u16).push(value as u16);
}

/// push ds; push `offset`: a far pointer into the data segment.
fn push_data(code: &mut Code, offset: u16) {
    code.emit(&[0x1E]).push(offset);
}

/// Put the string `text` and its NUL at `at` in the data segment.
fn put_string(code: &mut Code, at: u16, text: &[u8]) {
    let mut bytes = text.to_vec();
    bytes.push(0);
    if bytes.len() % 2 == 1 {
        bytes.push(0);
    }
    for (i, pair) in bytes.chunks(2).enumerate() {
        set(
            code,
            at + 2 * i as u16,
            u16::from_le_bytes([pair[0], pair[1]]),
        );
    }
}

/// A Windows 3.1 program creates HKEY_CLASSES_ROOT\RDTEST\Sub with
/// SHELL's RegCreateKey and gives it a value with RegSetValue, then
/// reads it back by its path from HKEY_CLASSES_ROOT with RegQueryValue,
/// whose size it gives in the low word only, as such programs do, and
/// with KERNEL's RegQueryValueEx from the key. RegOpenKey of the root
/// with no subkey succeeds, and RegEnumKey of RDTEST gives "Sub". On
/// Windows 3.1 and 95 these work on the registry; they failed for every
/// key, and Visual Basic 4's runtime gives up at start when RegOpenKey
/// of HKEY_CLASSES_ROOT fails.
#[test]
fn a_program_writes_and_reads_the_registry() {
    let mut code = Code::new();
    let path = 0x80;
    put_string(&mut code, path, b"RDTEST\\Sub");
    put_string(&mut code, path + 0x20, b"RDTEST");
    put_string(&mut code, path + 0x30, b"value");

    // RegCreateKey(HKEY_CLASSES_ROOT, "RDTEST\Sub", &KEY).
    push_long(&mut code, HKEY_CLASSES_ROOT as u32);
    push_data(&mut code, path);
    push_data(&mut code, KEY);
    code.call("SHELL", REG_CREATE_KEY);
    expect_success(&mut code, 1);
    // RegSetValue(KEY, NULL, REG_SZ, "value", 5).
    push_dword(&mut code, KEY);
    push_long(&mut code, 0);
    push_long(&mut code, REG_SZ as u32);
    push_data(&mut code, path + 0x30);
    push_long(&mut code, 5);
    code.call("SHELL", REG_SET_VALUE);
    expect_success(&mut code, 2);

    // RegQueryValue(HKEY_CLASSES_ROOT, "RDTEST\Sub", BUFFER, &SIZE),
    // with junk in the size's high word.
    set(&mut code, SIZE, 0x20);
    set(&mut code, SIZE + 2, 0x1234);
    push_long(&mut code, HKEY_CLASSES_ROOT as u32);
    push_data(&mut code, path);
    push_data(&mut code, BUFFER);
    push_data(&mut code, SIZE);
    code.call("SHELL", REG_QUERY_VALUE);
    expect_success(&mut code, 3);
    expect_byte(&mut code, BUFFER, b'v', 4);
    expect_byte(&mut code, BUFFER + 4, b'e', 5);
    expect(&mut code, SIZE, 6, 6);

    // KERNEL's RegQueryValueEx(KEY, "", NULL, &TYPE, BUFFER, &SIZE).
    set(&mut code, BUFFER, 0);
    set(&mut code, SIZE, 0x20);
    set(&mut code, SIZE + 2, 0);
    set(&mut code, path + 0x40, 0);
    push_dword(&mut code, KEY);
    push_data(&mut code, path + 0x40);
    push_long(&mut code, 0);
    push_data(&mut code, TYPE);
    push_data(&mut code, BUFFER);
    push_data(&mut code, SIZE);
    code.call("KERNEL", KERNEL_REG_QUERY_VALUE_EX);
    expect_success(&mut code, 7);
    expect(&mut code, TYPE, REG_SZ, 8);
    expect_byte(&mut code, BUFFER, b'v', 9);

    // RegOpenKey(HKEY_CLASSES_ROOT, NULL, &KEY2).
    push_long(&mut code, HKEY_CLASSES_ROOT as u32);
    push_long(&mut code, 0);
    push_data(&mut code, KEY2);
    code.call("SHELL", REG_OPEN_KEY);
    expect_success(&mut code, 10);

    // RegOpenKey(HKEY_CLASSES_ROOT, "RDTEST", &KEY2) and RegEnumKey(KEY2,
    // 0, BUFFER, 20h).
    push_long(&mut code, HKEY_CLASSES_ROOT as u32);
    push_data(&mut code, path + 0x20);
    push_data(&mut code, KEY2);
    code.call("SHELL", REG_OPEN_KEY);
    expect_success(&mut code, 11);
    push_dword(&mut code, KEY2);
    push_long(&mut code, 0);
    push_data(&mut code, BUFFER);
    push_long(&mut code, 0x20);
    code.call("SHELL", REG_ENUM_KEY);
    expect_success(&mut code, 12);
    expect_byte(&mut code, BUFFER, b'S', 13);
    expect_byte(&mut code, BUFFER + 3, 0, 14);
    push_dword(&mut code, KEY2);
    code.call("SHELL", REG_CLOSE_KEY);
    expect_success(&mut code, 15);
    push_dword(&mut code, KEY);
    code.call("SHELL", REG_CLOSE_KEY);
    expect_success(&mut code, 16);
    code.emit(&[0xB0, 0x00]).exit_al();

    let (status, log) = run("registry16", program(&code, CODE));
    assert_eq!(status, 0, "check {status} failed:\n{log}");
}

/// KERNEL's WinExec and Yield.
const WIN_EXEC: u16 = 166;
const YIELD: u16 = 29;

/// A registration file of Windows 3.1's REGEDIT, as Video for Windows
/// 1.1's MPLAYER.REG is: a key's default value on each line, a comment,
/// an empty value, and Ctrl+Z at the end.
const MPLAYER_REG: &[u8] = b"REGEDIT\r\n\
HKEY_CLASSES_ROOT\\mplayer = Media Clip\r\n\
HKEY_CLASSES_ROOT\\mplayer\\protocol\\StdFileEditing\\PackageObjects = \r\n\
HKEY_CLASSES_ROOT\\mplayer\\shell\\open\\command = mplayer.exe /play /close %1\r\n\
\r\n\
;OLE2 Compatibility entries.\r\n\
HKEY_CLASSES_ROOT\\MPlayer\\CLSID = {0003000E-0000-0000-C000-000000000046}\r\n\
HKEY_CLASSES_ROOT\\.avi = mplayer\r\n\
\x1A";

/// A Windows 3.1 program runs `regedit /s C:\WINDOWS\MPLAYER.REG` with
/// WinExec, as Video for Windows 1.1's setup does, and reads the keys
/// back with RegQueryValue. Windows 95 and 98 have REGEDIT.EXE in the
/// Windows directory, and `/s` imports the file, in Windows 3.1's format
/// as in Windows 95's, without a message box; Windows 3.1's REGEDIT takes
/// the same command line. WinExec gives 33 for the Win32 program, which
/// runs beside the task: the program yields until the first key is
/// there (it ends with 20 if it never comes). Without REGEDIT.EXE,
/// WinExec gave 2, file not found.
#[test]
fn a_program_imports_a_windows_31_registration_file_with_regedit() {
    let mut code = Code::new();
    let line = 0x80;
    let first = 0xC0;
    let last = 0xD0;
    let command = 0xE0;
    put_string(&mut code, line, b"regedit /s C:\\WINDOWS\\MPLAYER.REG");
    put_string(&mut code, first, b"mplayer");
    put_string(&mut code, last, b".avi");
    put_string(&mut code, command, b"MPlayer\\shell\\open\\command");

    // WinExec(line, SW_SHOWNORMAL): 33.
    push_data(&mut code, line);
    code.push(1).call("KERNEL", WIN_EXEC);
    code.emit(&[0x3D, 0x21, 0x00, 0x74, 0x06]) // cmp ax,33; je over the exit
        .emit(&[0xB0, 0x01, 0xB4, 0x4C, 0xCD, 0x21]);

    // Until RegQueryValue(HKEY_CLASSES_ROOT, "mplayer") finds the key,
    // yield, 1000 times at most.
    code.emit(&[0xBE, 0xE8, 0x03]); // mov si,1000
    let again = code.here();
    set(&mut code, SIZE, 0x40);
    set(&mut code, SIZE + 2, 0);
    push_long(&mut code, HKEY_CLASSES_ROOT as u32);
    push_data(&mut code, first);
    push_data(&mut code, BUFFER);
    push_data(&mut code, SIZE);
    code.call("SHELL", REG_QUERY_VALUE);
    code.emit(&[0x0B, 0xC2]); // or ax,dx
    let found_at = code.here() + 1;
    code.emit(&[0x74, 0x00]); // jz found
    code.call("KERNEL", YIELD);
    code.emit(&[0x4E, 0x74, 0x03]); // dec si; jz gave up
    let back = again as i32 - (code.here() as i32 + 3);
    code.emit(&[0xE9]).emit(&(back as i16).to_le_bytes()); // jmp again
    code.emit(&[0xB0, 20]).exit_al(); // gave up
    let found = code.here();
    code.bytes[found_at as usize] = (found - found_at - 1) as u8;
    expect_byte(&mut code, BUFFER, b'M', 2);
    expect_byte(&mut code, BUFFER + 6, b'C', 3);
    expect(&mut code, SIZE, 11, 4);

    // The last line's key, and a key named in another case.
    set(&mut code, SIZE, 0x40);
    push_long(&mut code, HKEY_CLASSES_ROOT as u32);
    push_data(&mut code, last);
    push_data(&mut code, BUFFER);
    push_data(&mut code, SIZE);
    code.call("SHELL", REG_QUERY_VALUE);
    expect_success(&mut code, 5);
    expect_byte(&mut code, BUFFER, b'm', 6);
    expect(&mut code, SIZE, 8, 7);
    set(&mut code, SIZE, 0x40);
    push_long(&mut code, HKEY_CLASSES_ROOT as u32);
    push_data(&mut code, command);
    push_data(&mut code, BUFFER);
    push_data(&mut code, SIZE);
    code.call("SHELL", REG_QUERY_VALUE);
    expect_success(&mut code, 8);
    expect_byte(&mut code, BUFFER + 12, b'/', 9);

    // `regedit /e` writes a branch to a file; the program doesn't wait.
    let export = 0x100;
    let export_line = b"regedit /e C:\\OUT.REG HKEY_CLASSES_ROOT\\.AVI";
    put_string(&mut code, export, export_line);
    push_data(&mut code, export);
    code.push(1).call("KERNEL", WIN_EXEC);
    code.emit(&[0x3D, 0x21, 0x00, 0x74, 0x06]) // cmp ax,33; je over the exit
        .emit(&[0xB0, 10, 0xB4, 0x4C, 0xCD, 0x21]);
    code.emit(&[0xB0, 0x00]).exit_al();

    let mut folder = None;
    let (status, log) = run_with("regedit16", program(&code, CODE), |_, base| {
        fs::create_dir_all(base.join("WINDOWS")).unwrap();
        fs::write(base.join("WINDOWS").join("MPLAYER.REG"), MPLAYER_REG).unwrap();
        folder = Some(base.to_path_buf());
    });
    assert_eq!(status, 0, "check {status} failed:\n{log}");
    assert!(log.contains("REGEDIT.EXE ended with exit code 0"), "{log}");
    // Saved at once to the registry's file, the empty value too.
    let folder = folder.unwrap();
    let saved = fs::read_to_string(folder.join("REGISTRY.REG")).unwrap();
    let objects = "[HKEY_LOCAL_MACHINE\\Software\\Classes\\mplayer\\protocol\\\
                   StdFileEditing\\PackageObjects]\r\n@=\"\"\r\n";
    assert!(saved.contains(objects), "{saved}");
    // The branch, its key by the name it was made with, in Windows 95's
    // format.
    let exported = fs::read_to_string(folder.join("OUT.REG")).unwrap();
    assert_eq!(
        exported,
        "REGEDIT4\r\n\r\n[HKEY_CLASSES_ROOT\\.avi]\r\n@=\"mplayer\"\r\n"
    );
}
