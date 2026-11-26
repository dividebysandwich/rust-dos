//! Kernel32's LockFile and UnlockFile, and ReadFile and WriteFile of
//! locked bytes. Windows 95 and 98 keep one set of locks for each file in
//! VSHARE, whoever opened it: a DOS program's INT 21h AH=5Ch, a Win16
//! task's and a Win32 process's LockFile all lock the same bytes, and the
//! others then neither lock, read nor write them (ERROR_LOCK_VIOLATION,
//! 33). The process's own other opens of the file are kept from the
//! bytes too, as LockFile's documentation says, and the locks go when the
//! file object closes and when the process ends.
//!
//! T.EXE and U.EXE (`write_probe` writes them) were run from a DOS box of
//! Windows 98 SE, booted in rust-dos: both left FFh in RESULT.BIN, every
//! check passed. UnlockFile of bytes the process didn't lock fails there
//! with ERROR_LOCK_VIOLATION, and a DOS program's reads and writes of
//! locked bytes with error 05h.

use iced_x86::code_asm::*;
use rust_dos::cpu::{Cpu, HostConsole};
use rust_dos::exec::{NoHook, StopReason, run_batch};
use rust_dos_pe::{self as pe, Builder, Imp};
use std::fs;
use std::path::PathBuf;

const BASE: u32 = 0x40_0000;
const GENERIC_READ_WRITE: u32 = 0xC000_0000;
const FILE_SHARE_READ_WRITE: i32 = 3;
const CREATE_ALWAYS: i32 = 2;
const OPEN_EXISTING: i32 = 3;
const FILE_ATTRIBUTE_NORMAL: i32 = 0x80;
const ERROR_LOCK_VIOLATION: i32 = 33;

/// The file the programs lock, and its size.
const DATA: &str = "DATA.BIN";
const DATA_SIZE: usize = 100;

const KERNEL32: &[&str] = &[
    "CreateFileA",
    "WriteFile",
    "ReadFile",
    "LockFile",
    "UnlockFile",
    "SetFilePointer",
    "CreateProcessA",
    "WaitForSingleObject",
    "GetExitCodeProcess",
    "CloseHandle",
    "ExitProcess",
    "GetLastError",
    "GetStdHandle",
    "SetEndOfFile",
];

/// Where the program's data is, and its imports' slots, for its code.
struct Layout {
    data: u32,
    iat: Vec<u32>,
}

impl Layout {
    fn at(&self, offset: u32) -> i32 {
        (self.data + offset) as i32
    }

    fn f(&self, name: &str) -> AsmMemoryOperand {
        let i = KERNEL32.iter().position(|n| *n == name).unwrap();
        dword_ptr(self.iat[i] as u64)
    }
}

/// A console program importing KERNEL32's `KERNEL32` functions, with
/// `data` and the code `f` makes.
fn program(
    data: &[u8],
    f: impl FnOnce(&mut CodeAssembler, &Layout) -> Result<(), IcedError>,
) -> Vec<u8> {
    let mut b = Builder::exe(BASE);
    b.subsystem(pe::SUBSYSTEM_CONSOLE);
    let names: Vec<Imp> = KERNEL32.iter().map(|n| Imp::Name(n)).collect();
    let iat = b.imports(&[("KERNEL32.DLL", &names)]).concat();
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

/// The programs' data: the file's name at 0, a command line at 10h,
/// RESULT.BIN at 30h, a STARTUPINFO at 40h, a PROCESS_INFORMATION at
/// 90h, an exit code at 0A0h, a count at 0A4h, two handles at 0A8h and
/// 0ACh, and a buffer at 100h.
fn data(line: &str) -> Vec<u8> {
    let mut data = vec![0u8; 0x180];
    data[..DATA.len()].copy_from_slice(DATA.as_bytes());
    assert!(line.len() < 0x20);
    data[0x10..0x10 + line.len()].copy_from_slice(line.as_bytes());
    data[0x30..0x3A].copy_from_slice(b"RESULT.BIN");
    data[0x40] = 68;
    data
}

/// CreateFileA of the file named at `name` to read and write, shared
/// with everyone, with `disposition`: the handle in EAX.
fn open(a: &mut CodeAssembler, l: &Layout, name: u32, disposition: i32) -> Result<(), IcedError> {
    a.push(0)?;
    a.push(FILE_ATTRIBUTE_NORMAL)?;
    a.push(disposition)?;
    a.push(0)?;
    a.push(FILE_SHARE_READ_WRITE)?;
    a.push(GENERIC_READ_WRITE as i32)?;
    a.push(l.at(name))?;
    a.call(l.f("CreateFileA"))
}

/// LockFile or UnlockFile (`name`) of the handle at `handle`: `len` bytes
/// from `offset`. Its result in EAX.
fn lock(
    a: &mut CodeAssembler,
    l: &Layout,
    name: &str,
    handle: u32,
    offset: i32,
    len: i32,
) -> Result<(), IcedError> {
    a.push(0)?;
    a.push(len)?;
    a.push(0)?;
    a.push(offset)?;
    a.push(dword_ptr(l.at(handle) as u64))?;
    a.call(l.f(name))
}

/// ReadFile or WriteFile (`name`) of 4 bytes at offset 12 of the handle
/// at `handle`. Its result in EAX.
fn io_at_12(a: &mut CodeAssembler, l: &Layout, name: &str, handle: u32) -> Result<(), IcedError> {
    a.push(0)?;
    a.push(0)?;
    a.push(12)?;
    a.push(dword_ptr(l.at(handle) as u64))?;
    a.call(l.f("SetFilePointer"))?;
    a.push(0)?;
    a.push(l.at(0xA4))?;
    a.push(4)?;
    a.push(l.at(0x100))?;
    a.push(dword_ptr(l.at(handle) as u64))?;
    a.call(l.f(name))
}

/// Set bit `bit` of EBX if the call before failed with `error`.
fn failed_with(a: &mut CodeAssembler, l: &Layout, error: i32, bit: u32) -> Result<(), IcedError> {
    let mut skip = a.create_label();
    a.test(eax, eax)?;
    a.jnz(skip)?;
    a.call(l.f("GetLastError"))?;
    a.cmp(eax, error)?;
    a.jne(skip)?;
    a.or(ebx, 1i32 << bit)?;
    a.set_label(&mut skip)?;
    a.nop()
}

/// Set bit `bit` of EBX if the call before succeeded.
fn succeeded(a: &mut CodeAssembler, bit: u32) -> Result<(), IcedError> {
    let mut skip = a.create_label();
    a.test(eax, eax)?;
    a.jz(skip)?;
    a.or(ebx, 1i32 << bit)?;
    a.set_label(&mut skip)?;
    a.nop()
}

/// A Win32 program that opens DATA.BIN, which bytes 10 to 29 of are
/// another holder's, and ends with a bit for each check that held:
/// LockFile of bytes 15 to 24 fails with ERROR_LOCK_VIOLATION (1), so do
/// ReadFile (2) and WriteFile (4) of bytes 12 to 15, and UnlockFile of
/// bytes 10 to 29 fails with ERROR_LOCK_VIOLATION and LockFile of bytes 40 to
/// 49, which it leaves locked, succeeds (8).
fn win32_other_holder() -> Vec<u8> {
    program(&data(""), |a, l| {
        a.xor(ebx, ebx)?;
        open(a, l, 0, OPEN_EXISTING)?;
        a.mov(dword_ptr(l.at(0xA8) as u64), eax)?;
        lock(a, l, "LockFile", 0xA8, 15, 10)?;
        failed_with(a, l, ERROR_LOCK_VIOLATION, 0)?;
        io_at_12(a, l, "ReadFile", 0xA8)?;
        failed_with(a, l, ERROR_LOCK_VIOLATION, 1)?;
        io_at_12(a, l, "WriteFile", 0xA8)?;
        failed_with(a, l, ERROR_LOCK_VIOLATION, 2)?;
        lock(a, l, "UnlockFile", 0xA8, 10, 20)?;
        let mut done = a.create_label();
        a.test(eax, eax)?;
        a.jnz(done)?;
        a.call(l.f("GetLastError"))?;
        a.cmp(eax, ERROR_LOCK_VIOLATION)?;
        a.jne(done)?;
        lock(a, l, "LockFile", 0xA8, 40, 10)?;
        succeeded(a, 3)?;
        a.set_label(&mut done)?;
        a.push(ebx)?;
        a.call(l.f("ExitProcess"))?;
        a.int3()
    })
}

/// A DOS program (a COM file) with the checks of `win32_other_holder`,
/// through INT 21h: AH=5Ch fails with error 21h on bytes 10 to 29, which
/// another holds (1), AH=3Fh and 40h fail on them (2, 4), and AH=5Ch
/// locks bytes 40 to 49, which it leaves locked (8). It ends with those
/// bits. The reads and writes are checked for a failure alone: in a DOS
/// box of Windows 98 SE they fail with error 05h, which the built-in DOS
/// doesn't give yet (#1998).
fn dos_other_holder() -> Vec<u8> {
    const NAME: u16 = 0x200;
    const BUFFER: u16 = 0x240;
    let mut a = CodeAssembler::new(16).unwrap();
    let asm = |a: &mut CodeAssembler| -> Result<(), IcedError> {
        a.xor(bp, bp)?;
        a.mov(ax, 0x3D42)?;
        a.mov(dx, NAME as u32)?;
        a.int(0x21)?;
        let mut end = a.create_label();
        a.jc(end)?;
        a.mov(bx, ax)?;
        // Each check: the call, then bit `bit` if it failed, with 21h
        // when `code`.
        let fails = |a: &mut CodeAssembler, bit: u32, code: bool| -> Result<(), IcedError> {
            let mut skip = a.create_label();
            a.int(0x21)?;
            a.jnc(skip)?;
            if code {
                a.cmp(ax, 0x21)?;
                a.jne(skip)?;
            }
            a.or(bp, 1i32 << bit)?;
            a.set_label(&mut skip)?;
            a.nop()
        };
        let seek_12 = |a: &mut CodeAssembler| -> Result<(), IcedError> {
            a.mov(ax, 0x4200)?;
            a.xor(cx, cx)?;
            a.mov(dx, 12u32)?;
            a.int(0x21)
        };
        a.mov(ax, 0x5C00)?;
        a.xor(cx, cx)?;
        a.mov(dx, 15u32)?;
        a.xor(si, si)?;
        a.mov(di, 10u32)?;
        fails(a, 0, true)?;
        seek_12(a)?;
        a.mov(ah, 0x3F)?;
        a.mov(cx, 4u32)?;
        a.mov(dx, BUFFER as u32)?;
        fails(a, 1, false)?;
        seek_12(a)?;
        a.mov(ah, 0x40)?;
        a.mov(cx, 4u32)?;
        a.mov(dx, BUFFER as u32)?;
        fails(a, 2, false)?;
        a.mov(ax, 0x5C00)?;
        a.xor(cx, cx)?;
        a.mov(dx, 40u32)?;
        a.xor(si, si)?;
        a.mov(di, 10u32)?;
        a.int(0x21)?;
        let mut locked = a.create_label();
        a.jc(locked)?;
        a.or(bp, 8)?;
        a.set_label(&mut locked)?;
        a.nop()?;
        a.set_label(&mut end)?;
        a.mov(ax, bp)?;
        a.mov(ah, 0x4C)?;
        a.int(0x21)
    };
    asm(&mut a).unwrap();
    let mut com = a.assemble(0x100).unwrap();
    assert!(com.len() <= (NAME - 0x100) as usize);
    com.resize((NAME - 0x100) as usize, 0);
    com.extend_from_slice(DATA.as_bytes());
    com.push(0);
    com
}

/// T.EXE: it makes DATA.BIN, writes 100 bytes and locks bytes 10 to 29,
/// opens the file a second time, and starts the program `line` names,
/// which another process holds the lock from. It ends with a bit for
/// each check that held, and the child's exit code above them: ReadFile
/// of locked bytes through the second handle fails with
/// ERROR_LOCK_VIOLATION (1), through the first it reads them (2); after
/// the child ended, the bytes 40 to 49 it locked are free again (4); and
/// after the first handle is closed, the second reads the bytes its lock
/// had (8). It writes the code to RESULT.BIN too, for a run on Windows
/// 98, where a program started from the desktop has no exit code to see.
fn parent(line: &str) -> Vec<u8> {
    program(&data(line), |a, l| {
        a.xor(ebx, ebx)?;
        open(a, l, 0, CREATE_ALWAYS)?;
        a.mov(dword_ptr(l.at(0xA8) as u64), eax)?;
        a.push(0)?;
        a.push(l.at(0xA4))?;
        a.push(DATA_SIZE as i32)?;
        a.push(l.at(0x100))?;
        a.push(dword_ptr(l.at(0xA8) as u64))?;
        a.call(l.f("WriteFile"))?;
        lock(a, l, "LockFile", 0xA8, 10, 20)?;
        open(a, l, 0, OPEN_EXISTING)?;
        a.mov(dword_ptr(l.at(0xAC) as u64), eax)?;
        io_at_12(a, l, "ReadFile", 0xAC)?;
        failed_with(a, l, ERROR_LOCK_VIOLATION, 0)?;
        io_at_12(a, l, "ReadFile", 0xA8)?;
        succeeded(a, 1)?;

        a.push(l.at(0x90))?;
        a.push(l.at(0x40))?;
        for _ in 0..6 {
            a.push(0)?;
        }
        a.push(l.at(0x10))?;
        a.push(0)?;
        a.call(l.f("CreateProcessA"))?;
        a.push(-1)?;
        a.push(dword_ptr(l.at(0x90) as u64))?;
        a.call(l.f("WaitForSingleObject"))?;
        a.push(l.at(0xA0))?;
        a.push(dword_ptr(l.at(0x90) as u64))?;
        a.call(l.f("GetExitCodeProcess"))?;
        a.mov(eax, dword_ptr(l.at(0xA0) as u64))?;
        a.and(eax, 0xF)?;
        a.shl(eax, 4)?;
        a.or(ebx, eax)?;

        lock(a, l, "LockFile", 0xAC, 40, 10)?;
        succeeded(a, 2)?;
        a.push(dword_ptr(l.at(0xA8) as u64))?;
        a.call(l.f("CloseHandle"))?;
        io_at_12(a, l, "ReadFile", 0xAC)?;
        succeeded(a, 3)?;
        a.mov(dword_ptr(l.at(0xA0) as u64), ebx)?;
        open(a, l, 0x30, CREATE_ALWAYS)?;
        a.mov(esi, eax)?;
        a.push(0)?;
        a.push(l.at(0xA4))?;
        a.push(4)?;
        a.push(l.at(0xA0))?;
        a.push(esi)?;
        a.call(l.f("WriteFile"))?;
        a.push(esi)?;
        a.call(l.f("CloseHandle"))?;
        a.push(ebx)?;
        a.call(l.f("ExitProcess"))?;
        a.int3()
    })
}

fn scratch(name: &str, files: &[(&str, Vec<u8>)]) -> PathBuf {
    let base = PathBuf::from("target/test_win32_locks").join(name);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    for (file, bytes) in files {
        fs::write(base.join(file), bytes).unwrap();
    }
    base
}

/// A machine with the built-in Windows on and `files` in C:'s folder,
/// the shell loaded.
fn machine(test: &str, files: &[(&str, Vec<u8>)]) -> Box<Cpu> {
    let mut cpu = Cpu::new(scratch(test, files));
    cpu.bus.win32.enabled = true;
    cpu.host_console = Some(HostConsole::default());
    cpu.load_shell();
    cpu
}

/// Run T.EXE to its end: its exit code.
fn run(cpu: &mut Cpu) -> u8 {
    assert!(cpu.load_executable("T.EXE", None));
    for _ in 0..300 {
        cpu.bus.start_batch(cpu.bus.clock.icount + 100_000);
        if run_batch(cpu, &mut NoHook, false) == StopReason::ShellReloaded {
            assert!(!cpu.bus.win32.active());
            return cpu.errorlevel;
        }
    }
    panic!("T.EXE didn't end");
}

#[test]
fn a_win32_processs_lock_keeps_another_process_from_the_bytes() {
    let mut cpu = machine(
        "win32_child",
        &[("T.EXE", parent("C.EXE")), ("C.EXE", win32_other_holder())],
    );
    let code = run(&mut cpu);
    assert_eq!(code, 0xFF, "the checks that failed: {:08b}", !code);
    assert!(!cpu.bus.disk.has_locks());
}

#[test]
fn a_win32_processs_lock_keeps_a_dos_program_from_the_bytes() {
    let mut cpu = machine(
        "dos_child",
        &[("T.EXE", parent("D.COM")), ("D.COM", dos_other_holder())],
    );
    let code = run(&mut cpu);
    assert_eq!(code, 0xFF, "the checks that failed: {:08b}", !code);
    assert!(!cpu.bus.disk.has_locks());
}

/// A DOS program, the shell, holds bytes 10 to 29 of DATA.BIN with
/// AH=5Ch, and a Win32 program it starts can't lock, read or write them.
#[test]
fn a_dos_programs_lock_keeps_a_win32_process_from_the_bytes() {
    let mut cpu = machine(
        "dos_holder",
        &[
            ("T.EXE", win32_other_holder()),
            (DATA, vec![0x55; DATA_SIZE]),
        ],
    );
    let shell = cpu.current_psp;
    let sft = cpu.bus.disk.open_file(DATA, 0x42, shell).unwrap();
    cpu.bus.disk.lock_region(sft, shell as u32, 10, 20).unwrap();
    let code = run(&mut cpu);
    assert_eq!(code, 0x0F, "the checks that failed: {:04b}", !code & 0x0F);
}

/// A Win32 child that locks bytes 40 to 49 through its standard input,
/// the parent's handle of DATA.BIN, and ends without unlocking them: its
/// exit code is LockFile's result.
fn win32_std_input_locker() -> Vec<u8> {
    program(&data(""), |a, l| {
        a.push(-10)?;
        a.call(l.f("GetStdHandle"))?;
        a.mov(dword_ptr(l.at(0xA8) as u64), eax)?;
        lock(a, l, "LockFile", 0xA8, 40, 10)?;
        a.push(eax)?;
        a.call(l.f("ExitProcess"))?;
        a.int3()
    })
}

/// T.EXE: it makes DATA.BIN and starts C.EXE with its handle of the file
/// as the child's standard input, so that the file object, and DOS's
/// entry, outlive the child. It ends with the child's exit code in bit 0
/// and, in bit 1, whether its own LockFile of the bytes the child locked
/// succeeds once the child ended.
fn parent_sharing_the_file() -> Vec<u8> {
    program(&data("C.EXE"), |a, l| {
        open(a, l, 0, CREATE_ALWAYS)?;
        a.mov(dword_ptr(l.at(0xA8) as u64), eax)?;
        // STARTF_USESTDHANDLES, and the file as hStdInput.
        a.mov(dword_ptr(l.at(0x6C) as u64), 0x100)?;
        a.mov(dword_ptr(l.at(0x78) as u64), eax)?;
        a.push(0)?;
        a.push(l.at(0xA4))?;
        a.push(DATA_SIZE as i32)?;
        a.push(l.at(0x100))?;
        a.push(dword_ptr(l.at(0xA8) as u64))?;
        a.call(l.f("WriteFile"))?;
        a.push(l.at(0x90))?;
        a.push(l.at(0x40))?;
        a.push(0)?;
        a.push(0)?;
        a.push(0)?;
        a.push(1)?;
        a.push(0)?;
        a.push(0)?;
        a.push(l.at(0x10))?;
        a.push(0)?;
        a.call(l.f("CreateProcessA"))?;
        a.push(-1)?;
        a.push(dword_ptr(l.at(0x90) as u64))?;
        a.call(l.f("WaitForSingleObject"))?;
        a.push(l.at(0xA0))?;
        a.push(dword_ptr(l.at(0x90) as u64))?;
        a.call(l.f("GetExitCodeProcess"))?;
        a.mov(ebx, dword_ptr(l.at(0xA0) as u64))?;
        a.and(ebx, 1)?;
        lock(a, l, "LockFile", 0xA8, 40, 10)?;
        succeeded(a, 1)?;
        a.push(ebx)?;
        a.call(l.f("ExitProcess"))?;
        a.int3()
    })
}

/// A process's locks go when it ends, also those made through a file
/// object another process still holds, whose entry in DOS's table stays
/// open (`remove_process`).
#[test]
fn a_processs_locks_go_when_it_ends_with_the_file_still_open() {
    let mut cpu = machine(
        "std_input_child",
        &[
            ("T.EXE", parent_sharing_the_file()),
            ("C.EXE", win32_std_input_locker()),
        ],
    );
    let code = run(&mut cpu);
    assert_eq!(code, 3, "the checks that failed: {:02b}", !code & 3);
}

/// A Win32 program that opens DATA.BIN, which bytes 10 to 29 of are
/// another holder's, and ends with a bit for each check that held:
/// SetEndOfFile at byte 5 fails with ERROR_LOCK_VIOLATION (1), as
/// WriteFile of no bytes there does, and at byte 50, past the lock, it
/// succeeds (2).
fn win32_cuts_the_file() -> Vec<u8> {
    program(&data(""), |a, l| {
        a.xor(ebx, ebx)?;
        open(a, l, 0, OPEN_EXISTING)?;
        a.mov(dword_ptr(l.at(0xA8) as u64), eax)?;
        for (at, check) in [(5, 0), (50, 1)] {
            a.push(0)?;
            a.push(0)?;
            a.push(at)?;
            a.push(dword_ptr(l.at(0xA8) as u64))?;
            a.call(l.f("SetFilePointer"))?;
            a.push(dword_ptr(l.at(0xA8) as u64))?;
            a.call(l.f("SetEndOfFile"))?;
            if check == 0 {
                failed_with(a, l, ERROR_LOCK_VIOLATION, check)?;
            } else {
                succeeded(a, check)?;
            }
        }
        a.push(ebx)?;
        a.call(l.f("ExitProcess"))?;
        a.int3()
    })
}

/// A DOS program, the shell, holds bytes 10 to 29 of DATA.BIN with
/// AH=5Ch, and a Win32 program it starts can't cut the file into them.
#[test]
fn a_lock_keeps_set_end_of_file_from_the_bytes() {
    let mut cpu = machine(
        "set_end_of_file",
        &[
            ("T.EXE", win32_cuts_the_file()),
            (DATA, vec![0x55; DATA_SIZE]),
        ],
    );
    let shell = cpu.current_psp;
    let sft = cpu.bus.disk.open_file(DATA, 0x42, shell).unwrap();
    cpu.bus.disk.lock_region(sft, shell as u32, 10, 20).unwrap();
    let code = run(&mut cpu);
    assert_eq!(code, 3, "the checks that failed: {:02b}", !code & 3);
    let file = PathBuf::from("target/test_win32_locks/set_end_of_file").join(DATA);
    assert_eq!(fs::metadata(file).unwrap().len(), 50);
}

/// Writes the programs to the folder RUST_DOS_LOCK_PROBE names, to be run
/// on a booted Windows 98: T.EXE with the Win32 child (C.EXE), U.EXE
/// with the DOS one (D.COM), each leaving its exit code in RESULT.BIN.
#[test]
#[ignore]
fn write_probe() {
    let dir = PathBuf::from(std::env::var("RUST_DOS_LOCK_PROBE").unwrap());
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("T.EXE"), parent("C.EXE")).unwrap();
    fs::write(dir.join("C.EXE"), win32_other_holder()).unwrap();
    fs::write(dir.join("U.EXE"), parent("D.COM")).unwrap();
    fs::write(dir.join("D.COM"), dos_other_holder()).unwrap();
}
