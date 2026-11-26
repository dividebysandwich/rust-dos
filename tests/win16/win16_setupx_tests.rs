//! SETUPX's INF engine on the built-in Windows 98, as W95INF16.DLL, the
//! 16-bit half of the ADVPACK.DLL of Internet Explorer's time, drives it
//! to install an IExpress package's INF. The program is made byte by byte
//! (tests/common/win16) and ends with the number of the first check that
//! failed, 0 when all held.

use crate::win16;

use std::fs;
use std::path::Path;
use win16::{CODE, Code, program, run_with};

/// SETUPX's ordinals.
const IP_OPEN: u16 = 2;
const IP_CLOSE: u16 = 4;
const CTL_GET_LDD_PATH: u16 = 38;
const GEN_INSTALL: u16 = 101;
const GEN_FORM_STR: u16 = 103;
const VCP_OPEN: u16 = 200;
const VCP_CLOSE: u16 = 201;
const IP_GET_PROFILE_STRING: u16 = 210;
const CTL_SET_LDD_PATH: u16 = 508;

/// GenInstall's parts: the files (and GENINSTALL_DO_REGSRCPATH), then
/// the INI files and the registry, as W95INF16 asks for them; and
/// VcpClose's VCPFL_ALL.
const FILES: u16 = 0x41;
const THE_REST: u16 = 0x3E;
const VCPFL_ALL: u16 = 0x1A;

/// Where the program keeps things in its data segment.
const HINF: u16 = 0x10;
const BUFFER: u16 = 0x200;

const INF: &str = "[Version]\r\n\
Signature=\"$Chicago$\"\r\n\
\r\n\
[DefaultInstall]\r\n\
CopyFiles   =Test.System, Test.App\r\n\
DelFiles=Test.Gone\r\n\
AddReg      =Test.Reg ; the values\r\n\
UpdateInis=Test.Inis\r\n\
RegisterOCXs=Test.Register\r\n\
\r\n\
[DestinationDirs]\r\n\
Test.System =11\r\n\
Test.App=10,RDAPP\r\n\
Test.Gone=10\r\n\
\r\n\
[Test.System]\r\n\
rdtest.dll,,,97\r\n\
msvcrt.dll,rdcrt.dll,,97\r\n\
\r\n\
[Test.App]\r\n\
readme.txt\r\n\
\r\n\
[Test.Gone]\r\n\
gone.txt\r\n\
\r\n\
[Test.Reg]\r\n\
HKLM,\"Software\\RdTest\",\"Path\",,\"%11%\\rdtest.dll\"\r\n\
HKLM,\"Software\\RdTest\",\"Name\",,\"%Product%\"\r\n\
HKLM,\"Software\\RdTest\",\"Bytes\",1,01,2a\r\n\
\r\n\
[Test.Inis]\r\n\
system.ini, drivers32,, \"rdtest.codec=rdtest.dll\"\r\n\
\r\n\
[Test.Register]\r\n\
%11%\\rdtest.dll\r\n\
\r\n\
[Strings]\r\n\
Product=\"RD Test\"\r\n";

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

/// push ds; push `offset`: a far pointer into the data segment.
fn push_data(code: &mut Code, offset: u16) {
    code.emit(&[0x1E]).push(offset);
}

/// End with `fail` unless AX is 0 (OK).
fn expect_ok(code: &mut Code, fail: u8) {
    code.emit(&[0x0B, 0xC0]) // or ax,ax
        .emit(&[0x74, 0x06]) // jz over the exit
        .emit(&[0xB0, fail, 0xB4, 0x4C, 0xCD, 0x21]);
}

/// End with `fail` unless the byte at `at` is `value`.
fn expect_byte(code: &mut Code, at: u16, value: u8, fail: u8) {
    code.emit(&[0x80, 0x3E, lo(at), hi(at), value])
        .emit(&[0x74, 0x06])
        .emit(&[0xB0, fail, 0xB4, 0x4C, 0xCD, 0x21]);
}

/// The file `name` in the host folder `dir`, its case ignored, as DOS
/// may have written its name in capitals.
fn find(dir: &Path, name: &str) -> Option<std::path::PathBuf> {
    fs::read_dir(dir)
        .ok()?
        .flatten()
        .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name))
        .map(|e| e.path())
}

/// A program calls SETUPX as W95INF16 does for an IExpress package's
/// INF: IpOpen, IpGetProfileString of the signature, CtlSetLddPath of
/// the source (LDID 1), VcpOpen, GenInstall of the files, VcpClose, which
/// copies them, then GenInstall of the registry and the INI files,
/// GenFormStrWithoutPlaceHolders, CtlGetLddPath and IpClose. On Windows
/// 98 SE that installs the files into the directories [DestinationDirs]
/// gives (11, the system directory; 10 and a subdirectory), deletes
/// DelFiles', writes AddReg's values with the [Strings] and the LDIDs'
/// directories put in, and adds the line to SYSTEM.INI. A copy over a
/// file with a version resource from one without keeps the file in
/// place: the built-in MSVCRT.DLL the system folder shows stays. The
/// built-in Windows' SETUPX had no functions, and the program ended at
/// its first call, IpOpen.
#[test]
fn a_program_installs_an_inf_through_setupx() {
    let mut code = Code::new();
    let inf = 0x20;
    let version = 0x40;
    let signature = 0x50;
    let source = 0x60;
    let section = 0x70;
    let form = 0x90;
    put_string(&mut code, inf, b"C:\\SRC\\TEST.INF");
    put_string(&mut code, version, b"Version");
    put_string(&mut code, signature, b"Signature");
    put_string(&mut code, source, b"C:\\SRC");
    put_string(&mut code, section, b"DefaultInstall");
    put_string(&mut code, form, b"%10%\\%Product%");

    // IpOpen("C:\SRC\TEST.INF", &HINF).
    push_data(&mut code, inf);
    push_data(&mut code, HINF);
    code.call("SETUPX", IP_OPEN);
    expect_ok(&mut code, 1);
    // IpGetProfileString(HINF, "Version", "Signature", BUFFER, 100h):
    // "$Chicago$", its quotes gone.
    code.emit(&[0xFF, 0x36, lo(HINF), hi(HINF)]); // push word [HINF]
    push_data(&mut code, version);
    push_data(&mut code, signature);
    push_data(&mut code, BUFFER);
    code.push(0x100);
    code.call("SETUPX", IP_GET_PROFILE_STRING);
    expect_ok(&mut code, 2);
    expect_byte(&mut code, BUFFER, b'$', 3);
    expect_byte(&mut code, BUFFER + 8, b'$', 4);
    expect_byte(&mut code, BUFFER + 9, 0, 5);
    // CtlSetLddPath(1, "C:\SRC").
    code.push(1);
    push_data(&mut code, source);
    code.call("SETUPX", CTL_SET_LDD_PATH);
    expect_ok(&mut code, 6);
    // VcpOpen(NULL, NULL).
    code.push(0).push(0).push(0).push(0);
    code.call("SETUPX", VCP_OPEN);
    expect_ok(&mut code, 7);
    // GenInstall(HINF, "DefaultInstall", files).
    code.emit(&[0xFF, 0x36, lo(HINF), hi(HINF)]);
    push_data(&mut code, section);
    code.push(FILES);
    code.call("SETUPX", GEN_INSTALL);
    expect_ok(&mut code, 8);
    // VcpClose(VCPFL_ALL, NULL).
    code.push(VCPFL_ALL).push(0).push(0);
    code.call("SETUPX", VCP_CLOSE);
    expect_ok(&mut code, 9);
    // GenInstall(HINF, "DefaultInstall", the rest).
    code.emit(&[0xFF, 0x36, lo(HINF), hi(HINF)]);
    push_data(&mut code, section);
    code.push(THE_REST);
    code.call("SETUPX", GEN_INSTALL);
    expect_ok(&mut code, 10);
    // GenFormStrWithoutPlaceHolders(BUFFER, "%10%\%Product%", HINF):
    // "C:\WINDOWS\RD Test".
    push_data(&mut code, BUFFER);
    push_data(&mut code, form);
    code.emit(&[0xFF, 0x36, lo(HINF), hi(HINF)]);
    code.call("SETUPX", GEN_FORM_STR);
    expect_byte(&mut code, BUFFER, b'C', 11);
    expect_byte(&mut code, BUFFER + 3, b'W', 12);
    expect_byte(&mut code, BUFFER + 11, b'R', 13);
    expect_byte(&mut code, BUFFER + 18, 0, 14);
    // CtlGetLddPath(11, BUFFER): "C:\WINDOWS\SYSTEM".
    code.push(11);
    push_data(&mut code, BUFFER);
    code.call("SETUPX", CTL_GET_LDD_PATH);
    expect_ok(&mut code, 15);
    expect_byte(&mut code, BUFFER + 11, b'S', 16);
    expect_byte(&mut code, BUFFER + 17, 0, 17);
    // IpClose(HINF), and again: the handle is closed.
    code.emit(&[0xFF, 0x36, lo(HINF), hi(HINF)]);
    code.call("SETUPX", IP_CLOSE);
    expect_ok(&mut code, 18);
    code.emit(&[0xB0, 0x00]).exit_al();

    let (status, log) = run_with("setupx_inf", program(&code, CODE), |_, base| {
        let src = base.join("SRC");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("TEST.INF"), INF).unwrap();
        fs::write(src.join("RDTEST.DLL"), b"the library").unwrap();
        fs::write(src.join("RDCRT.DLL"), b"no version here").unwrap();
        fs::write(src.join("README.TXT"), b"read me").unwrap();
        let windows = base.join("WINDOWS");
        fs::create_dir_all(windows.join("SYSTEM")).unwrap();
        fs::write(windows.join("GONE.TXT"), b"to go").unwrap();
    });
    assert_eq!(status, 0, "check {status} failed:\n{log}");

    let base = Path::new("target/test_win16/setupx_inf");
    let windows = base.join("WINDOWS");
    let system = windows.join("SYSTEM");
    let copied = find(&system, "rdtest.dll").expect("RDTEST.DLL in the system folder");
    assert_eq!(fs::read(copied).unwrap(), b"the library");
    let app = find(&windows, "rdapp").expect("the RDAPP folder");
    let readme = find(&app, "readme.txt").expect("README.TXT in RDAPP");
    assert_eq!(fs::read(readme).unwrap(), b"read me");
    assert!(
        find(&windows, "gone.txt").is_none(),
        "GONE.TXT is still there"
    );
    assert!(
        find(&system, "msvcrt.dll").is_none(),
        "an MSVCRT.DLL without a version replaced the built-in one:\n{log}"
    );
    let ini = fs::read_to_string(find(&windows, "system.ini").expect("SYSTEM.INI")).unwrap();
    assert!(ini.contains("[drivers32]"), "{ini}");
    assert!(ini.contains("rdtest.codec=rdtest.dll"), "{ini}");
    let registry = fs::read_to_string(base.join("REGISTRY.REG")).unwrap();
    let key = registry
        .split("\r\n\r\n")
        .find(|k| k.contains("\\Software\\RdTest]"))
        .unwrap_or_else(|| panic!("no RdTest key:\n{registry}"));
    assert!(
        key.contains("\"Path\"=\"C:\\\\WINDOWS\\\\SYSTEM\\\\rdtest.dll\""),
        "{key}"
    );
    assert!(key.contains("\"Name\"=\"RD Test\""), "{key}");
    assert!(key.contains("\"Bytes\"=hex:01,2a"), "{key}");
}
