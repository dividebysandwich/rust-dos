//! The built-in DLLs as files in the built-in Windows's system directory,
//! C:\WINDOWS\SYSTEM: programs open, read, search and look for them as
//! files, read their PE headers and export directories, and can't write
//! them. The programs are PE files made with rust-dos-pe's builder, their
//! code assembled with iced; each sets a bit of its exit code for each
//! check that passes.

use iced_x86::code_asm::*;
use rust_dos::cpu::{Cpu, HostConsole};
use rust_dos::exec::{NoHook, StopReason, run_batch};
use rust_dos::voodoo::Board;
use rust_dos_pe::{self as pe, Builder, Imp};
use std::fs;
use std::path::PathBuf;

const BASE: u32 = 0x40_0000;

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const FILE_SHARE_READ: u32 = 1;
const CREATE_NEW: u32 = 1;
const CREATE_ALWAYS: u32 = 2;
const OPEN_EXISTING: u32 = 3;
const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;
const OF_EXIST: u32 = 0x4000;
const ERROR_ACCESS_DENIED: u32 = 5;

/// Where a test program's data and imports are, for its code.
struct Layout {
    data: u32,
    names: Vec<&'static str>,
    iat: Vec<u32>,
}

impl Layout {
    /// The address of byte `offset` of the data.
    fn at(&self, offset: u32) -> u32 {
        self.data + offset
    }

    /// The import address table slot of the function `name`.
    fn import(&self, name: &str) -> AsmMemoryOperand {
        let i = self.names.iter().position(|n| *n == name).unwrap();
        dword_ptr(self.iat[i] as u64)
    }
}

/// A program importing `imports`, with `data` and the code `f` makes.
fn program(
    imports: &[(&str, &[&'static str])],
    data: &[u8],
    f: impl FnOnce(&mut CodeAssembler, &Layout) -> Result<(), IcedError>,
) -> Vec<u8> {
    let mut b = Builder::exe(BASE);
    let lists: Vec<Vec<Imp>> = imports
        .iter()
        .map(|(_, names)| names.iter().map(|n| Imp::Name(n)).collect())
        .collect();
    let dlls: Vec<(&str, &[Imp])> = imports
        .iter()
        .zip(&lists)
        .map(|((dll, _), list)| (*dll, list.as_slice()))
        .collect();
    let iat = b.imports(&dlls).concat();
    let data_rva = b.section(
        ".data",
        data,
        pe::SCN_INITIALIZED_DATA | pe::SCN_READ | pe::SCN_WRITE,
    );
    let layout = Layout {
        data: BASE + data_rva,
        names: imports
            .iter()
            .flat_map(|(_, n)| n.iter().copied())
            .collect(),
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

/// A C: drive for the test `name` with `files` on it, by their paths
/// there.
fn scratch(name: &str, files: &[(&str, &[u8])]) -> PathBuf {
    let base = PathBuf::from("target/test_win32_dll_files").join(name);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    for (file, bytes) in files {
        let path = base.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    base
}

/// Run `program` as C:\T.EXE on a machine with the built-in Windows, the
/// 3dfx card `board` and `files` on C:, to its end: its exit code. The
/// programs print nothing: an exception, which ends a program with exit
/// code FFh, prints a message.
fn run(test: &str, board: Option<Board>, files: &[(&str, &[u8])], program: Vec<u8>) -> u8 {
    let mut all = files.to_vec();
    all.push(("T.EXE", &program));
    let mut cpu = Cpu::new(scratch(test, &all));
    cpu.bus.win32.enabled = true;
    cpu.bus.configure_voodoo(board);
    cpu.host_console = Some(HostConsole::default());
    cpu.load_shell();
    assert!(cpu.load_executable("T.EXE", None));
    for _ in 0..400 {
        cpu.bus.start_batch(cpu.bus.clock.icount + 100_000);
        if run_batch(&mut cpu, &mut NoHook, false) == StopReason::ShellReloaded {
            let console = cpu.host_console.as_ref().unwrap();
            let printed = [&console.stdout[..], &console.stderr[..]].concat();
            assert!(printed.is_empty(), "{}", String::from_utf8_lossy(&printed));
            return cpu.errorlevel;
        }
    }
    let console = cpu.host_console.as_ref().unwrap();
    panic!(
        "the program didn't end: {}",
        String::from_utf8_lossy(&console.stdout)
    );
}

/// Call `slot` with `args`, pushed last first.
fn call(a: &mut CodeAssembler, slot: AsmMemoryOperand, args: &[u32]) -> Result<(), IcedError> {
    for &arg in args.iter().rev() {
        a.push(arg as i32)?;
    }
    a.call(slot)
}

/// Set bit `bit` of EBX if the flags say not equal (`ne`) or equal.
fn check(a: &mut CodeAssembler, ne: bool, bit: u32) -> Result<(), IcedError> {
    if ne {
        a.setne(al)?;
    } else {
        a.sete(al)?;
    }
    a.movzx(eax, al)?;
    a.shl(eax, bit)?;
    a.or(ebx, eax)
}

/// Set bit `bit` of EBX if the call just made failed with `error`.
fn failed_with(
    a: &mut CodeAssembler,
    l: &Layout,
    fail: u32,
    error: u32,
    bit: u32,
) -> Result<(), IcedError> {
    let mut done = a.create_label();
    a.cmp(eax, fail as i32)?;
    a.jne(done)?;
    a.call(l.import("GetLastError"))?;
    a.cmp(eax, error as i32)?;
    check(a, false, bit)?;
    a.set_label(&mut done)?;
    Ok(())
}

/// Put `bytes` at `offset` of `data`.
fn put(data: &mut [u8], offset: usize, bytes: &[u8]) {
    data[offset..offset + bytes.len()].copy_from_slice(bytes);
}

const KERNEL32: &[&str] = &[
    "CreateFileA",
    "ReadFile",
    "GetFileSize",
    "CloseHandle",
    "GetFileAttributesA",
    "SetFileAttributesA",
    "FindFirstFileA",
    "FindNextFileA",
    "FindClose",
    "OpenFile",
    "DeleteFileA",
    "MoveFileA",
    "CopyFileA",
    "LoadLibraryA",
    "GetProcAddress",
    "GetLastError",
];

const DDRAW: &[u8] = b"C:\\WINDOWS\\SYSTEM\\DDRAW.DLL\0";

// The data of the program that reads DDRAW.DLL.
const NAME: u32 = 0x40;
const MODULE: u32 = 0x60;
const COUNT: u32 = 0x80;
const HANDLE: u32 = 0x84;
const SIZE: u32 = 0x88;
const SECTIONS: u32 = 0x90;
const SECTION_COUNT: u32 = 0x94;
const EXPORTS: u32 = 0x98;
const INDEX: u32 = 0x9C;
const FUNCTION: u32 = 0xA0;
const LOADED: u32 = 0xA4;
const BUFFER: u32 = 0x1000;
const BUFFER_SIZE: usize = 0x4_0000;

/// Reads C:\WINDOWS\SYSTEM\DDRAW.DLL whole, checks its MZ and PE headers,
/// and finds DirectDrawCreate by walking its export directory through
/// the section table: the RVA it has there is where GetProcAddress finds
/// it in the DLL LoadLibrary loaded, whose base is the file's image base.
fn reads_ddraw() -> Vec<u8> {
    let mut data = vec![0; BUFFER as usize + BUFFER_SIZE];
    put(&mut data, 0, DDRAW);
    put(&mut data, NAME as usize, b"DirectDrawCreate\0");
    put(&mut data, MODULE as usize, b"DDRAW.DLL\0");
    program(&[("KERNEL32.DLL", KERNEL32)], &data, |a, l| {
        let at = |offset: u32| l.at(offset);
        let var = |offset: u32| dword_ptr(l.at(offset) as u64);
        let buffer = at(BUFFER);
        let mut to_pointer = a.create_label();
        a.xor(ebx, ebx)?;
        let open = [
            at(0),
            GENERIC_READ,
            FILE_SHARE_READ,
            0,
            OPEN_EXISTING,
            0x80,
            0,
        ];
        call(a, l.import("CreateFileA"), &open)?;
        a.mov(var(HANDLE), eax)?;
        a.cmp(eax, -1)?;
        check(a, true, 0)?;
        a.push(0)?;
        a.push(var(HANDLE))?;
        a.call(l.import("GetFileSize"))?;
        a.mov(var(SIZE), eax)?;
        a.push(0)?;
        a.push(at(COUNT) as i32)?;
        a.push(var(SIZE))?;
        a.push(buffer as i32)?;
        a.push(var(HANDLE))?;
        a.call(l.import("ReadFile"))?;
        a.mov(eax, var(COUNT))?;
        a.cmp(eax, 0x1000)?;
        a.setg(al)?;
        a.mov(ecx, var(COUNT))?;
        a.cmp(ecx, var(SIZE))?;
        a.sete(cl)?;
        a.and(al, cl)?;
        a.movzx(eax, al)?;
        a.shl(eax, 1)?;
        a.or(ebx, eax)?;
        a.push(var(HANDLE))?;
        a.call(l.import("CloseHandle"))?;
        a.cmp(word_ptr(buffer as u64), 0x5A4D)?;
        check(a, false, 2)?;
        a.mov(edi, dword_ptr((buffer + 0x3C) as u64))?;
        a.add(edi, buffer as i32)?;
        a.cmp(dword_ptr(edi), 0x4550)?;
        check(a, false, 3)?;
        // The section table, after the optional header.
        a.movzx(ecx, word_ptr(edi + 6))?;
        a.mov(var(SECTION_COUNT), ecx)?;
        a.movzx(eax, word_ptr(edi + 20))?;
        a.lea(eax, ptr(edi + eax + 24))?;
        a.mov(var(SECTIONS), eax)?;
        // The export directory: the first data directory.
        a.mov(edx, dword_ptr(edi + 24 + 96))?;
        a.call(to_pointer)?;
        a.mov(var(EXPORTS), eax)?;
        a.test(eax, eax)?;
        check(a, true, 4)?;
        let mut next = a.create_label();
        let mut found = a.create_label();
        let mut missing = a.create_label();
        a.mov(var(INDEX), 0)?;
        a.set_label(&mut next)?;
        a.mov(eax, var(EXPORTS))?;
        a.test(eax, eax)?;
        a.jz(missing)?;
        a.mov(ecx, var(INDEX))?;
        a.cmp(ecx, dword_ptr(eax + 24))?;
        a.jae(missing)?;
        a.mov(edx, dword_ptr(eax + 32))?;
        a.call(to_pointer)?;
        a.mov(ecx, var(INDEX))?;
        a.mov(edx, dword_ptr(eax + ecx * 4))?;
        a.call(to_pointer)?;
        a.mov(esi, eax)?;
        a.mov(edi, at(NAME) as i32)?;
        a.mov(ecx, 17)?;
        a.cld()?;
        a.repe().cmpsb()?;
        a.je(found)?;
        a.inc(var(INDEX))?;
        a.jmp(next)?;
        a.set_label(&mut found)?;
        a.mov(eax, var(EXPORTS))?;
        a.mov(edx, dword_ptr(eax + 36))?;
        a.call(to_pointer)?;
        a.mov(ecx, var(INDEX))?;
        a.movzx(ecx, word_ptr(eax + ecx * 2))?;
        a.mov(var(INDEX), ecx)?;
        a.mov(eax, var(EXPORTS))?;
        a.mov(edx, dword_ptr(eax + 28))?;
        a.call(to_pointer)?;
        a.mov(ecx, var(INDEX))?;
        a.mov(eax, dword_ptr(eax + ecx * 4))?;
        a.mov(var(FUNCTION), eax)?;
        a.or(ebx, 1 << 5)?;
        a.set_label(&mut missing)?;
        // The DLL LoadLibrary gives, at the file's image base, has it
        // there.
        call(a, l.import("LoadLibraryA"), &[at(MODULE)])?;
        a.mov(var(LOADED), eax)?;
        a.push(at(NAME) as i32)?;
        a.push(eax)?;
        a.call(l.import("GetProcAddress"))?;
        a.sub(eax, var(LOADED))?;
        a.cmp(eax, var(FUNCTION))?;
        check(a, false, 6)?;
        a.mov(edi, dword_ptr((buffer + 0x3C) as u64))?;
        a.mov(eax, dword_ptr(edi + (buffer as i32 + 24 + 28)))?;
        a.cmp(eax, var(LOADED))?;
        check(a, false, 7)?;
        a.mov(eax, ebx)?;
        a.ret()?;

        // EAX = where the RVA EDX is in the buffer, by the section that
        // has it; 0 if none does.
        a.set_label(&mut to_pointer)?;
        let mut section = a.create_label();
        let mut skip = a.create_label();
        a.mov(esi, var(SECTIONS))?;
        a.mov(ecx, var(SECTION_COUNT))?;
        a.set_label(&mut section)?;
        a.mov(eax, edx)?;
        a.sub(eax, dword_ptr(esi + 12))?;
        a.jb(skip)?;
        a.cmp(eax, dword_ptr(esi + 16))?;
        a.jae(skip)?;
        a.add(eax, dword_ptr(esi + 20))?;
        a.add(eax, buffer as i32)?;
        a.ret()?;
        a.set_label(&mut skip)?;
        a.add(esi, 40)?;
        a.dec(ecx)?;
        a.jnz(section)?;
        a.xor(eax, eax)?;
        a.ret()
    })
}

#[test]
fn ddraw_dll_is_a_pe_file_whose_exports_are_the_loaded_dlls() {
    assert_eq!(run("ddraw", None, &[], reads_ddraw()), 0xFF);
}

/// Looks for DDRAW.DLL with GetFileAttributes, FindFirstFile and
/// OpenFile, lists the system directory, and looks for GLIDE2X.DLL and
/// NTDLL.DLL, which the machine doesn't have.
fn finds_ddraw() -> Vec<u8> {
    let mut data = vec![0; 0x800];
    put(&mut data, 0, DDRAW);
    put(&mut data, 0x40, b"C:\\WINDOWS\\SYSTEM\0");
    put(&mut data, 0x60, b"C:\\WINDOWS\\SYSTEM\\*.DLL\0");
    put(&mut data, 0x80, b"C:\\WINDOWS\\SYSTEM\\GLIDE2X.DLL\0");
    put(&mut data, 0xA0, b"C:\\WINDOWS\\SYSTEM\\NTDLL.DLL\0");
    put(&mut data, 0xC0, b"C:\\WINDOWS\\*.*\0");
    // WIN32_FIND_DATA at 200h, an OFSTRUCT at 400h, a count at 600h.
    program(&[("KERNEL32.DLL", KERNEL32)], &data, |a, l| {
        let at = |offset: u32| l.at(offset);
        a.xor(ebx, ebx)?;
        call(a, l.import("GetFileAttributesA"), &[at(0)])?;
        a.cmp(eax, 0x21)?;
        check(a, false, 0)?;
        call(a, l.import("GetFileAttributesA"), &[at(0x40)])?;
        a.cmp(eax, 0x10)?;
        check(a, false, 1)?;
        // Found by its name, read-only, with its size.
        call(a, l.import("FindFirstFileA"), &[at(0), at(0x200)])?;
        a.push(eax)?;
        a.call(l.import("FindClose"))?;
        a.cmp(
            dword_ptr(at(0x200 + 44) as u64),
            u32::from_le_bytes(*b"DDRA") as i32,
        )?;
        a.sete(al)?;
        a.cmp(dword_ptr(at(0x200 + 32) as u64), 0x1000)?;
        a.seta(cl)?;
        a.and(al, cl)?;
        a.cmp(dword_ptr(at(0x200) as u64), 0x21)?;
        a.sete(cl)?;
        a.and(al, cl)?;
        a.movzx(eax, al)?;
        a.shl(eax, 2)?;
        a.or(ebx, eax)?;
        // The listing has the built-in DLLs: KERNEL32 to MPR.
        let mut more = a.create_label();
        let mut done = a.create_label();
        call(a, l.import("FindFirstFileA"), &[at(0x60), at(0x200)])?;
        a.mov(esi, eax)?;
        a.cmp(esi, -1)?;
        a.je(done)?;
        a.set_label(&mut more)?;
        a.inc(dword_ptr(at(0x600) as u64))?;
        a.push(at(0x200) as i32)?;
        a.push(esi)?;
        a.call(l.import("FindNextFileA"))?;
        a.test(eax, eax)?;
        a.jnz(more)?;
        a.push(esi)?;
        a.call(l.import("FindClose"))?;
        a.set_label(&mut done)?;
        a.cmp(dword_ptr(at(0x600) as u64), 20)?;
        a.setae(al)?;
        a.movzx(eax, al)?;
        a.shl(eax, 3)?;
        a.or(ebx, eax)?;
        call(a, l.import("OpenFile"), &[at(0), at(0x400), OF_EXIST])?;
        a.cmp(eax, 1)?;
        check(a, false, 4)?;
        call(a, l.import("GetFileAttributesA"), &[at(0x80)])?;
        a.cmp(eax, -1)?;
        check(a, false, 5)?;
        call(a, l.import("GetFileAttributesA"), &[at(0xA0)])?;
        a.cmp(eax, -1)?;
        check(a, false, 6)?;
        // C:\WINDOWS lists SYSTEM.
        call(a, l.import("FindFirstFileA"), &[at(0xC0), at(0x200)])?;
        a.mov(esi, eax)?;
        let mut next = a.create_label();
        let mut end = a.create_label();
        a.set_label(&mut next)?;
        a.cmp(
            dword_ptr(at(0x200 + 44) as u64),
            u32::from_le_bytes(*b"SYST") as i32,
        )?;
        a.je(end)?;
        a.push(at(0x200) as i32)?;
        a.push(esi)?;
        a.call(l.import("FindNextFileA"))?;
        a.test(eax, eax)?;
        a.jnz(next)?;
        a.set_label(&mut end)?;
        a.cmp(dword_ptr(at(0x200) as u64), 0x10)?;
        check(a, false, 7)?;
        a.mov(eax, ebx)?;
        a.ret()
    })
}

#[test]
fn the_system_directory_shows_the_built_in_dlls_as_files() {
    assert_eq!(run("find", None, &[], finds_ddraw()), 0xFF);
}

/// Opens GLIDE2X.DLL as MPROFILE.EXE does, with CreateFileA and with
/// MSVCRT's fopen "rb", and loads it.
fn finds_glide() -> Vec<u8> {
    let mut data = vec![0; 0x100];
    put(&mut data, 0, b"C:\\WINDOWS\\SYSTEM\\GLIDE2X.DLL\0");
    put(&mut data, 0x40, b"rb\0");
    put(&mut data, 0x50, b"GLIDE2X.DLL\0");
    let imports: &[(&str, &[&str])] = &[
        ("KERNEL32.DLL", KERNEL32),
        ("MSVCRT.DLL", &["fopen", "fclose"]),
    ];
    program(imports, &data, |a, l| {
        let at = |offset: u32| l.at(offset);
        a.xor(ebx, ebx)?;
        let open = [
            at(0),
            GENERIC_READ,
            FILE_SHARE_READ,
            0,
            OPEN_EXISTING,
            0x80,
            0,
        ];
        call(a, l.import("CreateFileA"), &open)?;
        a.cmp(eax, -1)?;
        check(a, true, 0)?;
        call(a, l.import("fopen"), &[at(0), at(0x40)])?;
        a.add(esp, 8)?;
        a.test(eax, eax)?;
        check(a, true, 1)?;
        call(a, l.import("LoadLibraryA"), &[at(0x50)])?;
        a.test(eax, eax)?;
        check(a, true, 2)?;
        a.mov(eax, ebx)?;
        a.ret()
    })
}

#[test]
fn glide2x_dll_is_there_with_a_3dfx_card_alone() {
    assert_eq!(run("glide", Some(Board::Voodoo2), &[], finds_glide()), 7);
    assert_eq!(
        run("glide-voodoo3", Some(Board::Voodoo3), &[], finds_glide()),
        7
    );
    assert_eq!(run("no-glide", None, &[], finds_glide()), 0);
}

/// Tries to write, make, delete, rename, copy over and change the
/// attributes of DDRAW.DLL, and makes a file beside it.
fn writes_ddraw() -> Vec<u8> {
    let mut data = vec![0; 0x100];
    put(&mut data, 0, DDRAW);
    put(&mut data, 0x40, b"C:\\WINDOWS\\SYSTEM\\OTHER.DLL\0");
    put(&mut data, 0x60, b"C:\\T.EXE\0");
    put(&mut data, 0x80, b"C:\\WINDOWS\\SYSTEM\\NEW.DLL\0");
    program(&[("KERNEL32.DLL", KERNEL32)], &data, |a, l| {
        let at = |offset: u32| l.at(offset);
        let denied = ERROR_ACCESS_DENIED;
        a.xor(ebx, ebx)?;
        let write = [at(0), GENERIC_WRITE, 0, 0, OPEN_EXISTING, 0x80, 0];
        call(a, l.import("CreateFileA"), &write)?;
        failed_with(a, l, u32::MAX, denied, 0)?;
        let both = GENERIC_READ | GENERIC_WRITE;
        let create = [at(0), both, 0, 0, CREATE_ALWAYS, 0x80, 0];
        call(a, l.import("CreateFileA"), &create)?;
        failed_with(a, l, u32::MAX, denied, 1)?;
        call(a, l.import("DeleteFileA"), &[at(0)])?;
        failed_with(a, l, 0, denied, 2)?;
        call(a, l.import("MoveFileA"), &[at(0), at(0x40)])?;
        failed_with(a, l, 0, denied, 3)?;
        call(
            a,
            l.import("SetFileAttributesA"),
            &[at(0), FILE_ATTRIBUTE_NORMAL],
        )?;
        failed_with(a, l, 0, denied, 4)?;
        call(a, l.import("CopyFileA"), &[at(0x60), at(0), 0])?;
        failed_with(a, l, 0, denied, 5)?;
        call(a, l.import("GetFileAttributesA"), &[at(0)])?;
        a.cmp(eax, 0x21)?;
        check(a, false, 6)?;
        // A file of its own beside it.
        let new = [at(0x80), GENERIC_WRITE, 0, 0, CREATE_NEW, 0x80, 0];
        call(a, l.import("CreateFileA"), &new)?;
        a.cmp(eax, -1)?;
        check(a, true, 7)?;
        a.push(eax)?;
        a.call(l.import("CloseHandle"))?;
        a.mov(eax, ebx)?;
        a.ret()
    })
}

#[test]
fn a_built_in_dll_file_is_read_only() {
    assert_eq!(run("write", None, &[], writes_ddraw()), 0xFF);
    let system = PathBuf::from("target/test_win32_dll_files/write/WINDOWS/SYSTEM");
    // Made on the drive with the directories on its way; DDRAW.DLL stays
    // the built-in one's, OTHER.DLL isn't made.
    assert!(system.join("NEW.DLL").is_file());
    assert!(!system.join("DDRAW.DLL").exists());
    assert!(!system.join("OTHER.DLL").exists());
}

/// Reads the first 8 bytes of DDRAW.DLL, looks for it, and for DSOUND.DLL
/// beside it.
fn reads_host_ddraw() -> Vec<u8> {
    let mut data = vec![0; 0x600];
    put(&mut data, 0, DDRAW);
    put(&mut data, 0x40, b"C:\\WINDOWS\\SYSTEM\\DSOUND.DLL\0");
    // A count at 80h, the bytes at 90h, WIN32_FIND_DATA at 200h.
    program(&[("KERNEL32.DLL", KERNEL32)], &data, |a, l| {
        let at = |offset: u32| l.at(offset);
        a.xor(ebx, ebx)?;
        let open = [
            at(0),
            GENERIC_READ,
            FILE_SHARE_READ,
            0,
            OPEN_EXISTING,
            0x80,
            0,
        ];
        call(a, l.import("CreateFileA"), &open)?;
        a.mov(esi, eax)?;
        a.push(0)?;
        a.push(at(0x80) as i32)?;
        a.push(16)?;
        a.push(at(0x90) as i32)?;
        a.push(esi)?;
        a.call(l.import("ReadFile"))?;
        a.cmp(
            dword_ptr(at(0x90) as u64),
            u32::from_le_bytes(*b"HOST") as i32,
        )?;
        check(a, false, 0)?;
        a.cmp(dword_ptr(at(0x80) as u64), 8)?;
        check(a, false, 1)?;
        a.push(0)?;
        a.push(esi)?;
        a.call(l.import("GetFileSize"))?;
        a.cmp(eax, 8)?;
        check(a, false, 2)?;
        a.push(esi)?;
        a.call(l.import("CloseHandle"))?;
        call(a, l.import("FindFirstFileA"), &[at(0), at(0x200)])?;
        a.push(eax)?;
        a.call(l.import("FindClose"))?;
        a.cmp(dword_ptr(at(0x200 + 32) as u64), 8)?;
        check(a, false, 3)?;
        call(a, l.import("GetFileAttributesA"), &[at(0)])?;
        a.cmp(eax, 0x20)?;
        check(a, false, 4)?;
        call(a, l.import("GetFileAttributesA"), &[at(0x40)])?;
        a.cmp(eax, 0x21)?;
        check(a, false, 5)?;
        a.or(ebx, 0xC0)?;
        a.mov(eax, ebx)?;
        a.ret()
    })
}

#[test]
fn a_file_of_the_same_name_on_the_drive_wins() {
    let files: &[(&str, &[u8])] = &[("WINDOWS/SYSTEM/DDRAW.DLL", b"HOSTFILE")];
    assert_eq!(run("host", None, files, reads_host_ddraw()), 0xFF);
}
