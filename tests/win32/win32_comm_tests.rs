//! Win32's serial ports: CreateFileA's handles of COM1 to COM4 and the
//! comm functions on them, on the machine's UARTs. Serious Sam opens a
//! port for a second mouse and sets it up with SetupComm, PurgeComm,
//! GetCommState and SetCommState. The programs are PE files made with
//! rust-dos-pe's builder, their code assembled with iced; each ends with
//! exit code 100 when every check passed, or with the number of the
//! check that failed.

use iced_x86::code_asm::*;
use rust_dos::cpu::{Cpu, HostConsole};
use rust_dos::exec::{NoHook, StopReason, run_batch};
use rust_dos::serial::SerialSettings;
use rust_dos_pe::{self as pe, Builder, Imp};
use std::fs;
use std::path::PathBuf;

const BASE: u32 = 0x40_0000;
const PASSED: u8 = 100;

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const OPEN_EXISTING: u32 = 3;
const STD_OUTPUT_HANDLE: i32 = -11;
const INVALID_HANDLE_VALUE: u32 = 0xFFFF_FFFF;
const MAXDWORD: u32 = u32::MAX;

const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_INVALID_HANDLE: u32 = 6;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_OPERATION_ABORTED: u32 = 995;

const PURGE_RXABORT: u32 = 2;
const PURGE_ALL: u32 = 0xF;
const CLRDTR: u32 = 6;

/// The functions the programs import, in this order.
const IMPORTS: &[&str] = &[
    "CreateFileA",
    "CloseHandle",
    "SetupComm",
    "PurgeComm",
    "GetCommState",
    "SetCommState",
    "ClearCommError",
    "SetCommMask",
    "EscapeCommFunction",
    "SetCommTimeouts",
    "ReadFile",
    "WriteFile",
    "GetLastError",
    "GetStdHandle",
    "ExitProcess",
    "CreateProcessA",
    "WaitForSingleObject",
    "CreateThread",
    "Sleep",
];

/// The data's places.
const COM2: u32 = 0x000;
const COM3: u32 = 0x010;
const DCB: u32 = 0x020;
const COMSTAT: u32 = 0x040;
const ERRORS: u32 = 0x050;
const HANDLE: u32 = 0x054;
const COUNT: u32 = 0x058;
const TIMEOUTS: u32 = 0x060;
const AT: u32 = 0x080;
const BUFFER: u32 = 0x100;
const DATA_SIZE: usize = 0x200;

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

/// Open COM2 for reading and writing as Serious Sam opens its second
/// mouse's port; the handle to HANDLE.
fn open_com2(a: &mut CodeAssembler, l: &Layout, exit: u8) -> Result<(), IcedError> {
    let args = [
        l.at(COM2),
        GENERIC_READ | GENERIC_WRITE,
        0,
        0,
        OPEN_EXISTING,
        0x80,
        0,
    ];
    call(a, l, "CreateFileA", &args)?;
    a.mov(dword_ptr(l.at(HANDLE) as u64), eax)?;
    a.cmp(eax, INVALID_HANDLE_VALUE as i32)?;
    let mut ok = a.create_label();
    a.jne(ok)?;
    call(a, l, "ExitProcess", &[exit as u32])?;
    a.set_label(&mut ok)
}

/// Push the handle at HANDLE and the other arguments, and call `name`.
fn call_port(a: &mut CodeAssembler, l: &Layout, name: &str, args: &[u32]) -> Result<(), IcedError> {
    for &arg in args.iter().rev() {
        a.push(arg as i32)?;
    }
    a.push(dword_ptr(l.at(HANDLE) as u64))?;
    a.call(l.import(name))
}

fn data() -> Vec<u8> {
    let mut data = vec![0u8; DATA_SIZE];
    data[COM2 as usize..][..5].copy_from_slice(b"COM2\0");
    data[COM3 as usize..][..5].copy_from_slice(b"COM3\0");
    data[AT as usize..][..3].copy_from_slice(b"AT\r");
    data
}

fn scratch(name: &str, exe: Vec<u8>) -> PathBuf {
    let base = PathBuf::from("target/test_win32").join(name);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    fs::write(base.join("T.EXE"), exe).unwrap();
    base
}

/// Run `exe` with the default serial ports (a mouse on COM1, a modem on
/// COM2, no COM3 or COM4) to its end: the machine, its exit code and
/// what it printed.
fn run(test: &str, exe: Vec<u8>) -> (Cpu, u8, String) {
    run_in(scratch(test, exe))
}

/// Run T.EXE of the folder `base` as `run` does.
fn run_in(base: PathBuf) -> (Cpu, u8, String) {
    let mut cpu = Cpu::new(base);
    cpu.bus.configure_serial(&SerialSettings::default());
    cpu.bus.win32.enabled = true;
    cpu.host_console = Some(HostConsole::default());
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
    let code = cpu.errorlevel;
    (cpu, code, text)
}

#[test]
fn a_port_opens_once_and_takes_the_dcb_serious_sam_gives() {
    let exe = program(&data(), |a, l| {
        // A port the machine doesn't have.
        let args = [
            l.at(COM3),
            GENERIC_READ | GENERIC_WRITE,
            0,
            0,
            OPEN_EXISTING,
            0x80,
            0,
        ];
        call(a, l, "CreateFileA", &args)?;
        expect_eax(a, l, INVALID_HANDLE_VALUE, 1)?;
        expect_error(a, l, ERROR_FILE_NOT_FOUND, 2)?;
        // One it has, which then is open.
        open_com2(a, l, 3)?;
        let args = [
            l.at(COM2),
            GENERIC_READ | GENERIC_WRITE,
            0,
            0,
            OPEN_EXISTING,
            0x80,
            0,
        ];
        call(a, l, "CreateFileA", &args)?;
        expect_eax(a, l, INVALID_HANDLE_VALUE, 4)?;
        expect_error(a, l, ERROR_ACCESS_DENIED, 5)?;
        // Serious Sam's start of its second mouse.
        call_port(a, l, "SetupComm", &[256, 256])?;
        expect_eax(a, l, 1, 6)?;
        call_port(a, l, "PurgeComm", &[PURGE_ALL])?;
        expect_eax(a, l, 1, 7)?;
        call_port(a, l, "GetCommState", &[l.at(DCB)])?;
        expect_eax(a, l, 1, 8)?;
        // 9600 bauds, 8 data bits, no parity, one stop bit, XON and XOFF
        // apart.
        expect_dword(a, l, DCB, 28, 9)?;
        expect_dword(a, l, DCB + 4, 9600, 10)?;
        expect_byte(a, l, DCB + 18, 8, 11)?;
        expect_byte(a, l, DCB + 19, 0, 12)?;
        expect_byte(a, l, DCB + 20, 0, 13)?;
        expect_byte(a, l, DCB + 21, 0x11, 14)?;
        expect_byte(a, l, DCB + 22, 0x13, 15)?;
        // 1200 bauds, 7N1, DTR and RTS on, fBinary and fParity.
        a.mov(dword_ptr(l.at(DCB + 4) as u64), 1200)?;
        a.mov(byte_ptr(l.at(DCB + 18) as u64), 7)?;
        a.mov(eax, dword_ptr(l.at(DCB + 8) as u64))?;
        a.and(eax, !0x3030)?;
        a.or(eax, 0x1013)?;
        a.mov(dword_ptr(l.at(DCB + 8) as u64), eax)?;
        call_port(a, l, "SetCommState", &[l.at(DCB)])?;
        expect_eax(a, l, 1, 16)?;
        call_port(a, l, "GetCommState", &[l.at(DCB)])?;
        expect_dword(a, l, DCB + 4, 1200, 17)?;
        expect_byte(a, l, DCB + 18, 7, 18)?;
        // Its polling: nothing came.
        call_port(a, l, "ClearCommError", &[l.at(ERRORS), l.at(COMSTAT)])?;
        expect_eax(a, l, 1, 19)?;
        expect_dword(a, l, ERRORS, 0, 20)?;
        expect_dword(a, l, COMSTAT + 4, 0, 21)?;
        // 5 data bits and two stop bits aren't a line Windows takes.
        a.mov(byte_ptr(l.at(DCB + 18) as u64), 5)?;
        a.mov(byte_ptr(l.at(DCB + 20) as u64), 2)?;
        call_port(a, l, "SetCommState", &[l.at(DCB)])?;
        expect_eax(a, l, 0, 22)?;
        expect_error(a, l, ERROR_INVALID_PARAMETER, 23)?;
        // Its shutdown.
        call_port(a, l, "SetCommMask", &[0])?;
        expect_eax(a, l, 1, 24)?;
        call_port(a, l, "EscapeCommFunction", &[CLRDTR])?;
        expect_eax(a, l, 1, 25)?;
        call_port(a, l, "CloseHandle", &[])?;
        expect_eax(a, l, 1, 26)?;
        // A handle that isn't a port's.
        call(a, l, "GetStdHandle", &[STD_OUTPUT_HANDLE as u32])?;
        a.push(256)?;
        a.push(256)?;
        a.push(eax)?;
        a.call(l.import("SetupComm"))?;
        expect_eax(a, l, 0, 28)?;
        expect_error(a, l, ERROR_INVALID_HANDLE, 29)?;
        call(a, l, "ExitProcess", &[PASSED as u32])
    });
    let (mut cpu, code, text) = run("comm_open", exe);
    assert_eq!(code, PASSED, "{text}");
    // SetCommState set the UART to 7N1, which closing the port leaves:
    // the line control register.
    assert_eq!(cpu.bus.serial_get(1, 3), Some(0x02));
}

#[test]
fn reads_wait_for_characters_as_the_timeouts_say() {
    let exe = program(&data(), |a, l| {
        open_com2(a, l, 1)?;
        // The modem's answer to AT, read with a total timeout of 500 ms:
        // what came in that time, fewer than the 64 asked for.
        a.mov(dword_ptr(l.at(TIMEOUTS + 8) as u64), 500)?;
        call_port(a, l, "SetCommTimeouts", &[l.at(TIMEOUTS)])?;
        expect_eax(a, l, 1, 2)?;
        call_port(a, l, "WriteFile", &[l.at(AT), 3, l.at(COUNT), 0])?;
        expect_eax(a, l, 1, 3)?;
        expect_dword(a, l, COUNT, 3, 4)?;
        call_port(a, l, "ReadFile", &[l.at(BUFFER), 64, l.at(COUNT), 0])?;
        expect_eax(a, l, 1, 5)?;
        // Nothing comes after it: ReadIntervalTimeout MAXDWORD gives what
        // there is, none, at once.
        a.mov(dword_ptr(l.at(TIMEOUTS) as u64), MAXDWORD as i32)?;
        a.mov(dword_ptr(l.at(TIMEOUTS + 8) as u64), 0)?;
        call_port(a, l, "SetCommTimeouts", &[l.at(TIMEOUTS)])?;
        call_port(
            a,
            l,
            "ReadFile",
            &[l.at(BUFFER + 0x80), 64, l.at(ERRORS), 0],
        )?;
        expect_eax(a, l, 1, 6)?;
        expect_dword(a, l, ERRORS, 0, 7)?;
        // What came, printed.
        call(a, l, "GetStdHandle", &[STD_OUTPUT_HANDLE as u32])?;
        a.push(0)?;
        a.push(l.at(ERRORS) as i32)?;
        a.push(dword_ptr(l.at(COUNT) as u64))?;
        a.push(l.at(BUFFER) as i32)?;
        a.push(eax)?;
        a.call(l.import("WriteFile"))?;
        // Closed, the port opens again.
        call_port(a, l, "CloseHandle", &[])?;
        expect_eax(a, l, 1, 8)?;
        open_com2(a, l, 9)?;
        call(a, l, "ExitProcess", &[PASSED as u32])
    });
    let (_, code, text) = run("comm_read", exe);
    assert_eq!(code, PASSED, "{text}");
    assert!(text.starts_with("AT\r"), "{text:?}");
    assert!(text.contains("OK\r\n"), "{text:?}");
}

#[test]
fn a_port_closes_with_the_process_that_had_it_open() {
    // CHILD.EXE opens COM2 and ends without closing it.
    let child = program(&data(), |a, l| {
        open_com2(a, l, 1)?;
        call(a, l, "ExitProcess", &[0])
    });
    const CHILD: u32 = 0x0A0;
    const STARTUP: u32 = 0x100;
    const INFO: u32 = 0x150;
    let mut data = data();
    data[CHILD as usize..][..10].copy_from_slice(b"CHILD.EXE\0");
    // STARTUPINFOA's cb.
    data[STARTUP as usize] = 68;
    let exe = program(&data, |a, l| {
        let args = [0, l.at(CHILD), 0, 0, 0, 0, 0, 0, l.at(STARTUP), l.at(INFO)];
        call(a, l, "CreateProcessA", &args)?;
        expect_eax(a, l, 1, 1)?;
        // Its end, with this process still there, frees the port.
        a.push(-1)?;
        a.push(dword_ptr(l.at(INFO) as u64))?;
        a.call(l.import("WaitForSingleObject"))?;
        expect_eax(a, l, 0, 2)?;
        open_com2(a, l, 3)?;
        call(a, l, "ExitProcess", &[PASSED as u32])
    });
    let base = scratch("comm_child", exe);
    fs::write(base.join("CHILD.EXE"), child).unwrap();
    let (_, code, text) = run_in(base);
    assert_eq!(code, PASSED, "{text}");
}

#[test]
fn purge_comm_ends_the_read_another_thread_waits_in() {
    const RESULT: u32 = 0x0A0;
    const ERROR: u32 = 0x0A4;
    const THREAD: u32 = 0x0A8;
    let exe = program(&data(), |a, l| {
        open_com2(a, l, 1)?;
        // CreateThread's last three arguments, then its start address,
        // which the call pushes: the code after it, the thread's. With no
        // timeouts its ReadFile waits for 64 characters, which the modem
        // never sends.
        a.push(0)?;
        a.push(0)?;
        a.push(0)?;
        let mut create = a.create_label();
        a.call(create)?;
        call_port(a, l, "ReadFile", &[l.at(BUFFER), 64, l.at(COUNT), 0])?;
        a.mov(dword_ptr(l.at(RESULT) as u64), eax)?;
        call(a, l, "GetLastError", &[])?;
        a.mov(dword_ptr(l.at(ERROR) as u64), eax)?;
        a.ret_1(4)?;
        a.set_label(&mut create)?;
        a.push(0)?;
        a.push(0)?;
        a.call(l.import("CreateThread"))?;
        a.mov(dword_ptr(l.at(THREAD) as u64), eax)?;
        a.mov(dword_ptr(l.at(RESULT) as u64), 7)?;
        // Once the thread waits, PURGE_RXABORT ends its read.
        call(a, l, "Sleep", &[20])?;
        call_port(a, l, "PurgeComm", &[PURGE_RXABORT])?;
        expect_eax(a, l, 1, 2)?;
        a.push(-1)?;
        a.push(dword_ptr(l.at(THREAD) as u64))?;
        a.call(l.import("WaitForSingleObject"))?;
        expect_eax(a, l, 0, 3)?;
        expect_dword(a, l, RESULT, 0, 4)?;
        expect_dword(a, l, ERROR, ERROR_OPERATION_ABORTED, 5)?;
        expect_dword(a, l, COUNT, 0, 6)?;
        call(a, l, "ExitProcess", &[PASSED as u32])
    });
    let (_, code, text) = run("comm_purge", exe);
    assert_eq!(code, PASSED, "{text}");
}
