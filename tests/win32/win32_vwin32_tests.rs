//! VWIN32 through CreateFileA("\\.\VWIN32") and DeviceIoControl: Get
//! Drive Map Info (INT 21h AX=440Dh, CX=086Fh), which Prince of Persia 3D
//! asks for each drive, and the errors of the other control codes and
//! handles. The expected values are those a probe program measured on
//! Windows 98 SE booted from an IDE hard disk with a CD-ROM drive. The
//! programs are PE files made with rust-dos-pe's builder, their code
//! assembled with iced; each ends with exit code 100 when every check
//! passed, or with the number of the check that failed.

use crate::cdimage;
use iced_x86::code_asm::*;
use rust_dos::cpu::{Cpu, HostConsole};
use rust_dos::exec::{NoHook, StopReason, run_batch};
use rust_dos_pe::{self as pe, Builder, Imp};
use std::fs;
use std::path::PathBuf;

const BASE: u32 = 0x40_0000;
const PASSED: u8 = 100;

const INVALID_HANDLE_VALUE: u32 = 0xFFFF_FFFF;
const FILE_FLAG_DELETE_ON_CLOSE: u32 = 0x0400_0000;
const STD_OUTPUT_HANDLE: i32 = -11;

const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_INVALID_HANDLE: u32 = 6;
const ERROR_NOT_SUPPORTED: u32 = 50;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;

const VWIN32_DIOC_DOS_IOCTL: u32 = 1;

/// The functions the programs import, in this order.
const IMPORTS: &[&str] = &[
    "CreateFileA",
    "CloseHandle",
    "DeviceIoControl",
    "GetLastError",
    "SetLastError",
    "GetStdHandle",
    "ExitProcess",
];

/// The data's places: the names, the handle, a DIOC_REGISTERS (EBX,
/// EDX, ECX, EAX, EDI, ESI, flags), the count DeviceIoControl gives back
/// and a DRIVE_MAP_INFO.
const VWIN32: u32 = 0x000;
const NOSUCH: u32 = 0x010;
const DRIVE: u32 = 0x020;
const HANDLE: u32 = 0x030;
const RETURNED: u32 = 0x034;
const REGS: u32 = 0x040;
const REG_EBX: u32 = REGS;
const REG_EDX: u32 = REGS + 4;
const REG_ECX: u32 = REGS + 8;
const REG_EAX: u32 = REGS + 12;
const REG_FLAGS: u32 = REGS + 24;
const INFO: u32 = 0x080;
const DATA_SIZE: usize = 0x100;

struct Layout {
    data: u32,
    iat: Vec<u32>,
}

impl Layout {
    fn at(&self, offset: u32) -> u32 {
        self.data + offset
    }

    fn import(&self, name: &str) -> AsmMemoryOperand {
        let i = IMPORTS.iter().position(|n| *n == name).unwrap();
        dword_ptr(self.iat[i] as u64)
    }
}

/// A console program importing IMPORTS from KERNEL32, with `data` and the
/// code `f` makes.
fn program(
    data: &[u8],
    f: impl FnOnce(&mut CodeAssembler, &Layout) -> Result<(), IcedError>,
) -> Vec<u8> {
    let mut b = Builder::exe(BASE);
    b.subsystem(pe::SUBSYSTEM_CONSOLE);
    let list: Vec<Imp> = IMPORTS.iter().map(|n| Imp::Name(n)).collect();
    let iat = b.imports(&[("KERNEL32.dll", list.as_slice())]).concat();
    let data_rva = b.section(
        ".data",
        data,
        pe::SCN_INITIALIZED_DATA | pe::SCN_READ | pe::SCN_WRITE,
    );
    let layout = Layout {
        data: BASE + data_rva,
        iat,
    };
    let code_rva = b.next_rva();
    let mut a = CodeAssembler::new(32).unwrap();
    f(&mut a, &layout).unwrap();
    let code = a.assemble((BASE + code_rva) as u64).unwrap();
    b.section(
        ".text",
        &code,
        pe::SCN_CODE | pe::SCN_EXECUTE | pe::SCN_READ,
    );
    b.entry(code_rva);
    b.build()
}

/// Call `name` with `args`, pushed last first.
fn call(a: &mut CodeAssembler, l: &Layout, name: &str, args: &[u32]) -> Result<(), IcedError> {
    for &arg in args.iter().rev() {
        a.push(arg as i32)?;
    }
    a.call(l.import(name))
}

/// End with exit code `exit` unless the flags say equal.
fn unless_equal(a: &mut CodeAssembler, l: &Layout, exit: u8) -> Result<(), IcedError> {
    let mut ok = a.create_label();
    a.je(ok)?;
    call(a, l, "ExitProcess", &[exit as u32])?;
    a.set_label(&mut ok)
}

/// End with exit code `exit` unless EAX is `value`.
fn expect_eax(a: &mut CodeAssembler, l: &Layout, value: u32, exit: u8) -> Result<(), IcedError> {
    a.cmp(eax, value as i32)?;
    unless_equal(a, l, exit)
}

/// End with exit code `exit` unless the last error is `code`.
fn expect_error(a: &mut CodeAssembler, l: &Layout, code: u32, exit: u8) -> Result<(), IcedError> {
    call(a, l, "GetLastError", &[])?;
    expect_eax(a, l, code, exit)
}

/// End with exit code `exit` unless the dword at `offset` is `value`.
fn expect_dword(
    a: &mut CodeAssembler,
    l: &Layout,
    offset: u32,
    value: u32,
    exit: u8,
) -> Result<(), IcedError> {
    a.cmp(dword_ptr(l.at(offset) as u64), value as i32)?;
    unless_equal(a, l, exit)
}

/// End with exit code `exit` unless the byte at `offset` is `value`.
fn expect_byte(
    a: &mut CodeAssembler,
    l: &Layout,
    offset: u32,
    value: u8,
    exit: u8,
) -> Result<(), IcedError> {
    a.cmp(byte_ptr(l.at(offset) as u64), value as i32)?;
    unless_equal(a, l, exit)
}

/// DeviceIoControl on the handle at HANDLE with `code`, REGS in and out
/// with sizes `size`, and the count to RETURNED.
fn device_io_control(
    a: &mut CodeAssembler,
    l: &Layout,
    code: u32,
    size: u32,
) -> Result<(), IcedError> {
    for arg in [0, l.at(RETURNED), size, l.at(REGS), size, l.at(REGS), code] {
        a.push(arg as i32)?;
    }
    a.push(dword_ptr(l.at(HANDLE) as u64))?;
    a.call(l.import("DeviceIoControl"))
}

/// Ask for the drive map of `drive` (1 for A:) with INFO's first byte
/// `size`, INFO's other bytes CCh and RETURNED 0: VWIN32 says it did,
/// with 28 bytes of registers; check `exit` and the next ones.
fn drive_map(
    a: &mut CodeAssembler,
    l: &Layout,
    drive: u32,
    size: u8,
    exit: u8,
) -> Result<(), IcedError> {
    for i in (0..32).step_by(4) {
        a.mov(dword_ptr(l.at(INFO + i) as u64), 0xCCCC_CCCCu32 as i32)?;
    }
    a.mov(byte_ptr(l.at(INFO) as u64), size as i32)?;
    a.mov(dword_ptr(l.at(RETURNED) as u64), 0)?;
    a.mov(dword_ptr(l.at(REG_EAX) as u64), 0x440D)?;
    a.mov(dword_ptr(l.at(REG_EBX) as u64), drive as i32)?;
    a.mov(dword_ptr(l.at(REG_ECX) as u64), 0x086F)?;
    a.mov(dword_ptr(l.at(REG_EDX) as u64), l.at(INFO) as i32)?;
    a.mov(dword_ptr(l.at(REG_FLAGS) as u64), 1)?;
    device_io_control(a, l, VWIN32_DIOC_DOS_IOCTL, 28)?;
    expect_eax(a, l, 1, exit)?;
    expect_dword(a, l, RETURNED, 28, exit + 1)
}

/// End with `exit` unless INFO holds the DRIVE_MAP_INFO `expected` after
/// its first byte, and the registers say it succeeded.
fn expect_info(
    a: &mut CodeAssembler,
    l: &Layout,
    expected: [u8; 15],
    exit: u8,
) -> Result<(), IcedError> {
    expect_dword(a, l, REG_EAX, 0, exit)?;
    expect_dword(a, l, REG_FLAGS, 0x0246, exit)?;
    for (i, &b) in expected.iter().enumerate() {
        expect_byte(a, l, INFO + 1 + i as u32, b, exit)?;
    }
    // The buffer past the structure stays as it was.
    expect_byte(a, l, INFO + 16, 0xCC, exit)
}

/// End with `exit` unless the registers say DOS's error `error`, and
/// INFO's bytes after the first stay as they were.
fn expect_dos_error(
    a: &mut CodeAssembler,
    l: &Layout,
    error: u32,
    exit: u8,
) -> Result<(), IcedError> {
    expect_dword(a, l, REG_EAX, error, exit)?;
    expect_dword(a, l, REG_FLAGS, 0x0247, exit)?;
    expect_byte(a, l, INFO + 1, 0xCC, exit)
}

fn data() -> Vec<u8> {
    let mut data = vec![0u8; DATA_SIZE];
    data[VWIN32 as usize..][..11].copy_from_slice(b"\\\\.\\VWIN32\0");
    data[NOSUCH as usize..][..11].copy_from_slice(b"\\\\.\\NOSUCH\0");
    data[DRIVE as usize..][..7].copy_from_slice(b"\\\\.\\C:\0");
    data
}

/// Run T.EXE in a folder of its own as C:, with a CD image on D:, to its
/// end: its exit code and what it printed.
fn run(test: &str, exe: Vec<u8>) -> (u8, String) {
    let base = PathBuf::from("target/test_win32").join(test);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    fs::write(base.join("T.EXE"), exe).unwrap();
    let dir = cdimage::scratch(&format!("{test}_cd"));
    let image = cdimage::iso("TESTDISC", &[cdimage::file("HELLO.TXT", b"hello")]);
    fs::write(dir.join("GAME.BIN"), cdimage::mode1_2352(&image)).unwrap();
    let cue = dir.join("GAME.CUE");
    let sheet = "FILE \"GAME.BIN\" BINARY\r\n  TRACK 01 MODE1/2352\r\n    INDEX 01 00:00:00\r\n";
    fs::write(&cue, sheet).unwrap();

    let mut cpu = Cpu::new(base);
    cpu.bus.win32.enabled = true;
    cpu.host_console = Some(HostConsole::default());
    cpu.bus
        .mount_drive(3, &cue, rust_dos::disk::MountOptions::default(), false)
        .unwrap();
    cpu.load_shell();
    assert!(cpu.load_executable("T.EXE", None));
    let mut ended = false;
    for _ in 0..2000 {
        cpu.bus.start_batch(cpu.bus.clock.icount + 100_000);
        if run_batch(&mut cpu, &mut NoHook, false) == StopReason::ShellReloaded {
            ended = true;
            break;
        }
    }
    let console = cpu.host_console.as_ref().unwrap();
    let mut text = console.stdout.clone();
    text.extend(&console.stderr);
    let text = String::from_utf8_lossy(&text).into_owned();
    assert!(ended, "the program didn't end: {text:?}");
    (cpu.errorlevel, text)
}

/// Open VWIN32 as Prince of Persia 3D does, with no access and a
/// disposition of 0; the handle to HANDLE.
fn open_vwin32(a: &mut CodeAssembler, l: &Layout, exit: u8) -> Result<(), IcedError> {
    let args = [l.at(VWIN32), 0, 0, 0, 0, FILE_FLAG_DELETE_ON_CLOSE, 0];
    call(a, l, "CreateFileA", &args)?;
    a.mov(dword_ptr(l.at(HANDLE) as u64), eax)?;
    a.cmp(eax, INVALID_HANDLE_VALUE as i32)?;
    let mut ok = a.create_label();
    a.jne(ok)?;
    call(a, l, "ExitProcess", &[exit as u32])?;
    a.set_label(&mut ok)
}

#[test]
fn vwin32_gives_the_drive_map_windows_98_gives() {
    let exe = program(&data(), |a, l| {
        // The open leaves ERROR_INVALID_PARAMETER for the disposition of
        // 0, as Windows 98 SE's does.
        call(a, l, "SetLastError", &[0xAAAA])?;
        open_vwin32(a, l, 1)?;
        expect_error(a, l, ERROR_INVALID_PARAMETER, 2)?;

        // C:, a hard disk: the first BIOS disk, its partition at sector
        // 63, the drivers of the logical and the physical drive.
        drive_map(a, l, 3, 16, 3)?;
        expect_info(
            a,
            l,
            [16, 0x03, 0x80, 0, 0, 0, 0, 63, 0, 0, 0, 0, 0, 0, 0],
            5,
        )?;
        // 0 is the current drive, C:.
        drive_map(a, l, 0, 16, 6)?;
        expect_info(
            a,
            l,
            [16, 0x03, 0x80, 0, 0, 0, 0, 63, 0, 0, 0, 0, 0, 0, 0],
            8,
        )?;
        // D:, the CD-ROM drive: no BIOS unit, a drive DOS doesn't have,
        // which ejects.
        drive_map(a, l, 4, 16, 9)?;
        expect_info(
            a,
            l,
            [16, 0x0F, 0xFF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            11,
        )?;
        // A buffer of 32 bytes gets the 16 of the structure, its first
        // byte as the caller set it.
        drive_map(a, l, 3, 32, 12)?;
        expect_info(
            a,
            l,
            [16, 0x03, 0x80, 0, 0, 0, 0, 63, 0, 0, 0, 0, 0, 0, 0],
            14,
        )?;
        expect_byte(a, l, INFO, 32, 15)?;
        // A buffer under 16 bytes: Access Denied, nothing written.
        drive_map(a, l, 3, 12, 16)?;
        expect_dos_error(a, l, 5, 18)?;
        // A drive there isn't: Invalid Drive.
        drive_map(a, l, 20, 16, 19)?;
        expect_dos_error(a, l, 0x0F, 21)?;

        call_handle_close(a, l, 22)?;
        call(a, l, "ExitProcess", &[PASSED as u32])
    });
    let (code, text) = run("vwin32_drive_map", exe);
    assert_eq!(code, PASSED, "{text}");
}

fn call_handle_close(a: &mut CodeAssembler, l: &Layout, exit: u8) -> Result<(), IcedError> {
    a.push(dword_ptr(l.at(HANDLE) as u64))?;
    a.call(l.import("CloseHandle"))?;
    expect_eax(a, l, 1, exit)
}

#[test]
fn device_io_control_fails_as_windows_98s_does() {
    let exe = program(&data(), |a, l| {
        // A VxD there isn't, and a drive, which Windows 98 doesn't open
        // this way.
        for (name, exit) in [(NOSUCH, 1u8), (DRIVE, 3)] {
            let args = [l.at(name), 0, 0, 0, 0, FILE_FLAG_DELETE_ON_CLOSE, 0];
            call(a, l, "CreateFileA", &args)?;
            expect_eax(a, l, INVALID_HANDLE_VALUE, exit)?;
            expect_error(a, l, ERROR_FILE_NOT_FOUND, exit + 1)?;
        }

        open_vwin32(a, l, 5)?;
        // Codes 0 and 5 aren't VWIN32's, nor is 7.
        for (code, error, exit) in [
            (0, ERROR_INVALID_PARAMETER, 6u8),
            (5, ERROR_INVALID_PARAMETER, 8),
            (7, ERROR_NOT_SUPPORTED, 10),
        ] {
            device_io_control(a, l, code, 28)?;
            expect_eax(a, l, 0, exit)?;
            expect_error(a, l, error, exit + 1)?;
        }
        // Registers of 8 bytes.
        device_io_control(a, l, VWIN32_DIOC_DOS_IOCTL, 8)?;
        expect_eax(a, l, 0, 12)?;
        expect_error(a, l, ERROR_INSUFFICIENT_BUFFER, 13)?;
        call_handle_close(a, l, 14)?;

        // Something that isn't a VxD, and no handle at all.
        call(a, l, "GetStdHandle", &[STD_OUTPUT_HANDLE as u32])?;
        a.mov(dword_ptr(l.at(HANDLE) as u64), eax)?;
        device_io_control(a, l, VWIN32_DIOC_DOS_IOCTL, 28)?;
        expect_eax(a, l, 0, 15)?;
        expect_error(a, l, ERROR_INVALID_PARAMETER, 16)?;
        a.mov(dword_ptr(l.at(HANDLE) as u64), 0x1234)?;
        device_io_control(a, l, VWIN32_DIOC_DOS_IOCTL, 28)?;
        expect_eax(a, l, 0, 17)?;
        expect_error(a, l, ERROR_INVALID_HANDLE, 18)?;
        call(a, l, "ExitProcess", &[PASSED as u32])
    });
    let (code, text) = run("vwin32_errors", exe);
    assert_eq!(code, PASSED, "{text}");
}
