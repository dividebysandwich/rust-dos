//! The built-in Windows as Windows 98 (the `win32_version` setting): the
//! version programs read, and the functions only Windows 98 has. The
//! values are those a Windows 98 SE install gives. The programs are PE
//! files made here with rust-dos-pe's builder, their code assembled with
//! iced.

use iced_x86::code_asm::*;
use rust_dos::config::WindowsVersion;
use rust_dos::cpu::{Cpu, HostConsole};
use rust_dos::exec::{NoHook, StopReason, run_batch};
use rust_dos_pe::{self as pe, Builder, Imp};
use std::fs;
use std::path::PathBuf;

const BASE: u32 = 0x40_0000;

/// Where a test program's data and imports are, for its code.
struct Layout {
    /// The data's address.
    data: u32,
    /// The import address table slot of each function, in the order the
    /// program imports them.
    iat: Vec<u32>,
}

impl Layout {
    /// The address of byte `offset` of the data.
    fn at(&self, offset: u32) -> u64 {
        (self.data + offset) as u64
    }

    /// The import address table slot of the function `index`.
    fn import(&self, index: usize) -> AsmMemoryOperand {
        dword_ptr(self.iat[index] as u64)
    }
}

/// A program importing `imports`, with `data` and the code `f` makes.
fn program(
    imports: &[(&str, &[&str])],
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
    let iat = if dlls.is_empty() {
        Vec::new()
    } else {
        b.imports(&dlls).concat()
    };
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

fn scratch(name: &str, files: &[(&str, Vec<u8>)]) -> PathBuf {
    let base = PathBuf::from("target/test_win32_win98").join(name);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    for (file, bytes) in files {
        fs::write(base.join(file), bytes).unwrap();
    }
    base
}

/// Run `program` as T.EXE under `version` to its end; its exit code and
/// what it printed.
fn run(version: WindowsVersion, test: &str, program: Vec<u8>) -> (u8, String) {
    run_with(version, test, program, &[])
}

/// `run`, with more files next to T.EXE.
fn run_with(
    version: WindowsVersion,
    test: &str,
    program: Vec<u8>,
    more: &[(&str, Vec<u8>)],
) -> (u8, String) {
    let name = format!("{}_{}", test, version.name());
    let mut files = vec![("T.EXE", program)];
    files.extend(more.iter().cloned());
    let mut cpu = Cpu::new(scratch(&name, &files));
    cpu.bus.win32.enabled = true;
    cpu.bus.win32.version = version;
    cpu.host_console = Some(HostConsole::default());
    cpu.load_shell();
    assert!(cpu.load_executable("T.EXE", None));
    let mut ended = false;
    for _ in 0..400 {
        cpu.bus.start_batch(cpu.bus.clock.icount + 100_000);
        if run_batch(&mut cpu, &mut NoHook, false) == StopReason::ShellReloaded {
            ended = true;
            break;
        }
    }
    let text = printed(&cpu);
    assert!(ended, "the program didn't end: {:?}", text);
    (cpu.errorlevel, text)
}

/// What the program printed, to standard output and error.
fn printed(cpu: &Cpu) -> String {
    let console = cpu.host_console.as_ref().unwrap();
    let mut text = console.stdout.clone();
    text.extend(&console.stderr);
    String::from_utf8_lossy(&text).into_owned()
}

use WindowsVersion::{Win95, Win98, Win98Se};

/// A program that ends with GetVersion's value shifted right by `shift`.
fn get_version(shift: u32) -> Vec<u8> {
    let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", &["GetVersion"])];
    program(imports, &[0], |a, l| {
        a.call(l.import(0))?;
        a.shr(eax, shift)?;
        a.ret()
    })
}

#[test]
fn get_version_follows_the_setting() {
    // The minor version is in the high byte of the low word.
    assert_eq!(run(Win95, "version_minor", get_version(8)).0, 0);
    assert_eq!(run(Win98, "version_minor", get_version(8)).0, 10);
    assert_eq!(run(Win98Se, "version_minor", get_version(8)).0, 10);
    assert_eq!(run(Win98Se, "version_major", get_version(0)).0, 4);
    // Bit 31 for Windows 9x, and 30 as well.
    assert_eq!(run(Win98Se, "version_high", get_version(24)).0, 0xC0);
}

/// A program that fills a version structure of `size` bytes with CCh,
/// calls GetVersionExA on it, and ends with its result in bit 8 and the
/// byte `byte` of the structure in the low byte (DOS keeps the low byte,
/// so a test of the result asks for `byte` past the end, which reads
/// 0).
fn get_version_ex(size: u32, byte: u32) -> Vec<u8> {
    let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", &["GetVersionExA"])];
    let mut data = vec![0xCCu8; 0x100];
    data[..4].copy_from_slice(&size.to_le_bytes());
    data.resize(0x200, 0);
    program(imports, &data, |a, l| {
        a.push(l.at(0) as i32)?;
        a.call(l.import(0))?;
        a.mov(ecx, eax)?;
        a.shl(ecx, 8)?;
        a.movzx(eax, byte_ptr(l.at(byte)))?;
        a.or(eax, ecx)?;
        a.ret()
    })
}

#[test]
fn get_version_ex_gives_the_build_and_service_pack() {
    // dwMinorVersion, the low byte of dwBuildNumber (2222 is 8AEh), and
    // szCSDVersion's " A ".
    assert_eq!(run(Win98Se, "ex_minor", get_version_ex(148, 8)).0, 10);
    assert_eq!(run(Win98Se, "ex_build", get_version_ex(148, 12)).0, 0xAE);
    assert_eq!(run(Win98, "ex_build", get_version_ex(148, 12)).0, 0xCE);
    assert_eq!(run(Win95, "ex_build", get_version_ex(148, 12)).0, 0xB6);
    assert_eq!(run(Win98Se, "ex_csd", get_version_ex(148, 21)).0, b'A');
    assert_eq!(run(Win95, "ex_csd", get_version_ex(148, 20)).0, 0);
}

#[test]
fn windows_98_writes_the_service_pack_up_to_its_end() {
    // After " A " and its 0, Windows 98 SE leaves the rest as it was;
    // the built-in Windows 95 clears it.
    assert_eq!(run(Win98Se, "ex_rest", get_version_ex(148, 24)).0, 0xCC);
    assert_eq!(run(Win95, "ex_rest", get_version_ex(148, 24)).0, 0);
}

#[test]
fn windows_98_takes_the_extended_structure() {
    // OSVERSIONINFOEXA's 156 bytes: TRUE in bit 8 on Windows 98, which
    // DOS's 8-bit exit code drops, so the test reads the low byte of
    // dwMajorVersion instead: 4 once filled, CCh when the call failed.
    assert_eq!(run(Win98Se, "ex_size", get_version_ex(156, 4)).0, 4);
    // wServicePackMajor: Windows 98 SE clears the extended fields.
    assert_eq!(run(Win98Se, "ex_fields", get_version_ex(156, 148)).0, 0);
    assert_eq!(run(Win95, "ex_size", get_version_ex(156, 4)).0, 0xCC);
    assert_eq!(run(Win98Se, "ex_bad_size", get_version_ex(100, 4)).0, 0xCC);
}

/// A program that ends with the byte `byte` of MSVCRT's variable
/// `name`.
fn msvcrt_variable(name: &str, byte: u32) -> Vec<u8> {
    let names = [name];
    let imports: &[(&str, &[&str])] = &[("MSVCRT.DLL", &names)];
    program(imports, &[0], |a, l| {
        a.mov(eax, l.import(0))?;
        a.movzx(eax, byte_ptr(eax + byte))?;
        a.ret()
    })
}

#[test]
fn msvcrt_has_the_version_in_its_variables() {
    assert_eq!(
        run(Win98Se, "winminor", msvcrt_variable("_winminor", 0)).0,
        10
    );
    assert_eq!(run(Win98Se, "winver", msvcrt_variable("_winver", 0)).0, 10);
    assert_eq!(
        run(Win98Se, "winver_major", msvcrt_variable("_winver", 1)).0,
        4
    );
    // Windows 98 SE's MSVCRT keeps GetVersion's top bit in _osver.
    assert_eq!(run(Win98Se, "osver", msvcrt_variable("_osver", 1)).0, 0xC0);
    assert_eq!(run(Win95, "osver", msvcrt_variable("_osver", 1)).0, 0x40);
    assert_eq!(run(Win95, "winminor", msvcrt_variable("_winminor", 0)).0, 0);
}

/// A program that ends with 1 when GetProcAddress finds USER32's
/// `function`, 0 when it doesn't.
fn finds(function: &str) -> Vec<u8> {
    let imports: &[(&str, &[&str])] = &[(
        "KERNEL32.DLL",
        &["GetModuleHandleA", "LoadLibraryA", "GetProcAddress"],
    )];
    let mut data = b"USER32.DLL\0".to_vec();
    data.resize(0x20, 0);
    data.extend_from_slice(function.as_bytes());
    data.push(0);
    program(imports, &data, |a, l| {
        a.push(l.at(0) as i32)?;
        a.call(l.import(1))?;
        a.push(l.at(0x20) as i32)?;
        a.push(eax)?;
        a.call(l.import(2))?;
        a.test(eax, eax)?;
        a.setne(al)?;
        a.movzx(eax, al)?;
        a.ret()
    })
}

#[test]
fn functions_of_windows_98_are_found_only_under_98() {
    assert_eq!(run(Win95, "find_track", finds("TrackMouseEvent")).0, 0);
    assert_eq!(run(Win98, "find_track", finds("TrackMouseEvent")).0, 1);
    assert_eq!(run(Win98Se, "find_track", finds("TrackMouseEvent")).0, 1);
    // A function both have.
    assert_eq!(run(Win95, "find_message_box", finds("MessageBoxA")).0, 1);
}

#[test]
fn a_program_importing_a_function_of_windows_98_needs_98() {
    let imports: &[(&str, &[&str])] = &[("USER32.DLL", &["GetAncestor"])];
    let exe = || {
        program(imports, &[0], |a, _| {
            a.mov(eax, 0x2A)?;
            a.ret()
        })
    };
    let (code, text) = run(Win95, "import_98", exe());
    assert_ne!(code, 0x2A);
    assert!(text.contains("missing export"), "{}", text);
    assert!(text.contains("GetAncestor"), "{}", text);
    assert!(text.contains("win32_version=98"), "{}", text);
    assert_eq!(run(Win98Se, "import_98", exe()).0, 0x2A);
}

/// A program that ends with the low byte of GetSystemMetrics(index).
fn system_metric(index: i32) -> Vec<u8> {
    let imports: &[(&str, &[&str])] = &[("USER32.DLL", &["GetSystemMetrics"])];
    program(imports, &[0], |a, l| {
        a.push(index)?;
        a.call(l.import(0))?;
        a.ret()
    })
}

#[test]
fn windows_98_has_the_virtual_screen_metrics() {
    // SM_CMONITORS and SM_CXVIRTUALSCREEN (640, 80h in its low byte).
    assert_eq!(run(Win98Se, "cmonitors", system_metric(80)).0, 1);
    assert_eq!(run(Win98Se, "cxvirtual", system_metric(78)).0, 0x80);
    assert_eq!(run(Win95, "cmonitors", system_metric(80)).0, 0);
}

const MONITOR: u32 = 0x11A2;

#[test]
fn the_monitor_functions_find_the_screen() {
    let imports: &[(&str, &[&str])] = &[(
        "USER32.DLL",
        &["MonitorFromWindow", "MonitorFromPoint", "GetDesktopWindow"],
    )];
    let exe = program(imports, &[0], |a, l| {
        // A bit for each answer as Windows 98 SE gives it.
        a.xor(ebx, ebx)?;
        let check = |a: &mut CodeAssembler, bit: u32, want: u32| {
            a.cmp(eax, want as i32)?;
            a.sete(cl)?;
            a.movzx(ecx, cl)?;
            a.shl(ecx, bit)?;
            a.or(ebx, ecx)
        };
        // No window and MONITOR_DEFAULTTONULL: none; with
        // MONITOR_DEFAULTTOPRIMARY the screen's.
        a.push(0)?;
        a.push(0)?;
        a.call(l.import(0))?;
        check(a, 0, 0)?;
        a.push(1)?;
        a.push(0)?;
        a.call(l.import(0))?;
        check(a, 1, MONITOR)?;
        a.call(l.import(2))?;
        a.push(0)?;
        a.push(eax)?;
        a.call(l.import(0))?;
        check(a, 2, MONITOR)?;
        // A point far off the screen, then one on it.
        a.push(0)?;
        a.push(0)?;
        a.push(-5000)?;
        a.call(l.import(1))?;
        check(a, 3, 0)?;
        a.push(0)?;
        a.push(5)?;
        a.push(5)?;
        a.call(l.import(1))?;
        check(a, 4, MONITOR)?;
        a.mov(eax, ebx)?;
        a.ret()
    });
    assert_eq!(run(Win98Se, "monitor_from", exe).0, 0x1F);
}

/// A program that fills a MONITORINFO of `size` with CCh, and ends with
/// GetMonitorInfoA's result in bit 7 and the byte `byte` of it in the
/// low bits.
fn monitor_info(size: u32, byte: u32) -> Vec<u8> {
    let imports: &[(&str, &[&str])] = &[("USER32.DLL", &["GetMonitorInfoA"])];
    let mut data = vec![0xCCu8; 0x80];
    data[..4].copy_from_slice(&size.to_le_bytes());
    program(imports, &data, |a, l| {
        a.push(l.at(0) as i32)?;
        a.push(MONITOR as i32)?;
        a.call(l.import(0))?;
        a.shl(eax, 7)?;
        a.movzx(ecx, byte_ptr(l.at(byte)))?;
        a.and(ecx, 0x7F)?;
        a.or(eax, ecx)?;
        a.ret()
    })
}

#[test]
fn get_monitor_info_gives_the_screen() {
    // rcMonitor's bottom, 480 (E0h, 60h in the low 7 bits), dwFlags
    // MONITORINFOF_PRIMARY, and MONITORINFOEXA's device name.
    assert_eq!(
        run(Win98Se, "info_bottom", monitor_info(40, 16)).0,
        0x80 | 0x60
    );
    assert_eq!(run(Win98Se, "info_flags", monitor_info(40, 36)).0, 0x81);
    assert_eq!(
        run(Win98Se, "info_name", monitor_info(72, 44)).0,
        0x80 | b'D'
    );
    // A wrong size fails and leaves the structure.
    assert_eq!(run(Win98Se, "info_size", monitor_info(12, 4)).0, 0x4C);
}

#[test]
fn enum_display_monitors_calls_back_for_the_screen() {
    let imports: &[(&str, &[&str])] = &[("USER32.DLL", &["EnumDisplayMonitors"])];
    let exe = program(imports, &[0; 0x40], |a, l| {
        // The callback's code, which the call skips, its address pushed.
        let mut over = a.create_label();
        a.call(over)?;
        // callback(monitor, hdc, rect, data)
        a.mov(eax, dword_ptr(esp + 4))?;
        a.mov(dword_ptr(l.at(0)), eax)?;
        a.mov(eax, dword_ptr(esp + 12))?;
        a.mov(eax, dword_ptr(eax + 12))?;
        a.mov(dword_ptr(l.at(4)), eax)?;
        a.mov(eax, dword_ptr(esp + 16))?;
        a.mov(dword_ptr(l.at(8)), eax)?;
        a.mov(eax, 1)?;
        a.ret_1(16)?;
        a.set_label(&mut over)?;
        a.pop(eax)?;
        a.push(0x55)?;
        a.push(eax)?;
        a.push(0)?;
        a.push(0)?;
        a.call(l.import(0))?;
        // The result, the monitor, the rectangle's bottom and the data.
        a.xor(ebx, ebx)?;
        a.cmp(eax, 1)?;
        a.sete(bl)?;
        a.cmp(dword_ptr(l.at(0)), MONITOR as i32)?;
        a.sete(cl)?;
        a.shl(cl, 1)?;
        a.or(bl, cl)?;
        a.cmp(dword_ptr(l.at(4)), 480)?;
        a.sete(cl)?;
        a.shl(cl, 2)?;
        a.or(bl, cl)?;
        a.cmp(dword_ptr(l.at(8)), 0x55)?;
        a.sete(cl)?;
        a.shl(cl, 3)?;
        a.or(bl, cl)?;
        a.mov(eax, ebx)?;
        a.ret()
    });
    assert_eq!(run(Win98Se, "enum_monitors", exe).0, 0xF);
}

#[test]
fn the_interlocked_functions_of_windows_98_work() {
    let imports: &[(&str, &[&str])] = &[(
        "KERNEL32.DLL",
        &["InterlockedCompareExchange", "InterlockedExchangeAdd"],
    )];
    let mut data = 5u32.to_le_bytes().to_vec();
    data.resize(0x10, 0);
    let exe = program(imports, &data, |a, l| {
        // 5 is there: 9 goes in. 7 isn't: nothing changes.
        a.push(5)?;
        a.push(9)?;
        a.push(l.at(0) as i32)?;
        a.call(l.import(0))?;
        a.mov(ebx, eax)?;
        a.push(7)?;
        a.push(1)?;
        a.push(l.at(0) as i32)?;
        a.call(l.import(0))?;
        a.shl(eax, 4)?;
        a.or(ebx, eax)?;
        a.push(3)?;
        a.push(l.at(0) as i32)?;
        a.call(l.import(1))?;
        // The old values 5 and 9 and 9, and the variable at 12.
        a.cmp(eax, 9)?;
        a.jne(0)?;
        a.cmp(ebx, 0x95)?;
        a.jne(0)?;
        a.mov(eax, dword_ptr(l.at(0)))?;
        a.ret()
    });
    assert_eq!(run(Win98Se, "interlocked", exe).0, 12);
}

#[test]
fn get_disk_free_space_ex_gives_bytes() {
    let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", &["GetDiskFreeSpaceExA"])];
    let exe = || {
        program(imports, &[0; 0x40], |a, l| {
            a.push(l.at(0x20) as i32)?;
            a.push(l.at(0x10) as i32)?;
            a.push(l.at(0) as i32)?;
            a.push(0)?;
            a.call(l.import(0))?;
            // TRUE, a total of some bytes, and the free bytes the same
            // for the caller as for all.
            a.mov(ecx, dword_ptr(l.at(0x10)))?;
            a.or(ecx, dword_ptr(l.at(0x14)))?;
            a.setne(cl)?;
            a.and(al, cl)?;
            a.mov(ecx, dword_ptr(l.at(0)))?;
            a.cmp(ecx, dword_ptr(l.at(0x20)))?;
            a.sete(cl)?;
            a.and(al, cl)?;
            a.movzx(eax, al)?;
            a.ret()
        })
    };
    // Windows 95 OSR2 has it as well.
    assert_eq!(run(Win98Se, "disk_free_ex", exe()).0, 1);
    assert_eq!(run(Win95, "disk_free_ex", exe()).0, 1);
}

/// A setup checks a whole install against the free space, so a folder
/// hard disk has more room under the built-in Windows than DOS's fake
/// 80 MB: about 3.75 GB when empty. GetDiskFreeSpaceExA gives it all.
/// GetDiskFreeSpaceA gives it as Windows 95 and 98 do, through INT 21h
/// AH=36h's 16-bit numbers: 65535 clusters of 32 KB, just under 2 GB.
#[test]
fn a_folder_hard_disk_has_room_for_an_install() {
    let imports: &[(&str, &[&str])] = &[(
        "KERNEL32.DLL",
        &["GetDiskFreeSpaceA", "GetDiskFreeSpaceExA"],
    )];
    let mut data = vec![0u8; 0x40];
    data.extend_from_slice(b"C:\\\0");
    let exe = program(imports, &data, |a, l| {
        // Sectors per cluster, bytes per sector, free and total clusters.
        a.push(l.at(0x0C) as i32)?;
        a.push(l.at(0x08) as i32)?;
        a.push(l.at(0x04) as i32)?;
        a.push(l.at(0) as i32)?;
        a.push(l.at(0x40) as i32)?;
        a.call(l.import(0))?;
        a.cmp(eax, 0)?;
        a.je(0)?;
        a.cmp(dword_ptr(l.at(0)), 64)?;
        a.jne(0)?;
        a.cmp(dword_ptr(l.at(0x04)), 512)?;
        a.jne(0)?;
        a.cmp(dword_ptr(l.at(0x0C)), 0xFFFF)?;
        a.jne(0)?;
        a.cmp(dword_ptr(l.at(0x08)), 0xFFFF)?;
        a.jne(0)?;
        // GetDiskFreeSpaceExA's free bytes: under 3.75 GB by what T.EXE
        // takes, with nothing above 4 GB.
        a.push(0)?;
        a.push(0)?;
        a.push(l.at(0x10) as i32)?;
        a.push(l.at(0x40) as i32)?;
        a.call(l.import(1))?;
        a.cmp(eax, 0)?;
        a.je(0)?;
        a.cmp(dword_ptr(l.at(0x14)), 0)?;
        a.jne(0)?;
        a.mov(eax, dword_ptr(l.at(0x10)))?;
        a.cmp(eax, 0xEFF0_0000u32 as i32)?;
        a.jb(0)?;
        a.cmp(eax, 0xF000_0000u32 as i32)?;
        a.ja(0)?;
        a.mov(eax, 1)?;
        a.ret()
    });
    assert_eq!(run(Win98Se, "disk_room", exe).0, 1);
}

#[test]
fn get_file_attributes_ex_gives_the_size() {
    let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", &["GetFileAttributesExA"])];
    let mut data = b"Long File Name.dat\0".to_vec();
    data.resize(0x20, 0);
    data.resize(0x60, 0xCC);
    let exe = program(imports, &data, |a, l| {
        a.push(l.at(0x20) as i32)?;
        a.push(0)?;
        a.push(l.at(0) as i32)?;
        a.call(l.import(0))?;
        a.test(eax, eax)?;
        a.je(0)?;
        // nFileSizeLow, after the attributes and three times.
        a.mov(eax, dword_ptr(l.at(0x20 + 32)))?;
        a.ret()
    });
    let file = ("Long File Name.dat", vec![7u8; 77]);
    assert_eq!(run_with(Win98Se, "attributes_ex", exe, &[file]).0, 77);
}

#[test]
fn get_long_path_name_gives_the_long_names() {
    let imports: &[(&str, &[&str])] =
        &[("KERNEL32.DLL", &["GetShortPathNameA", "GetLongPathNameA"])];
    let mut data = b"Long File Name.dat\0".to_vec();
    data.resize(0x100, 0);
    let exe = program(imports, &data, |a, l| {
        a.push(0x40)?;
        a.push(l.at(0x40) as i32)?;
        a.push(l.at(0) as i32)?;
        a.call(l.import(0))?;
        a.push(0x40)?;
        a.push(l.at(0x80) as i32)?;
        a.push(l.at(0x40) as i32)?;
        a.call(l.import(1))?;
        // The length, 18, and its first and last letters.
        a.mov(ebx, eax)?;
        a.cmp(byte_ptr(l.at(0x80)), b'L' as i32)?;
        a.jne(0)?;
        a.cmp(byte_ptr(l.at(0x80 + 17)), b't' as i32)?;
        a.jne(0)?;
        a.mov(eax, ebx)?;
        a.ret()
    });
    let file = ("Long File Name.dat", vec![1u8; 3]);
    assert_eq!(run_with(Win98Se, "long_path", exe, &[file]).0, 18);
}

/// A program that asks to restart Windows, and ends with 9 if the call
/// comes back.
fn restarts_windows() -> Vec<u8> {
    const EWX_REBOOT: i32 = 2;
    let imports: &[(&str, &[&str])] = &[("USER32.DLL", &["ExitWindowsEx"])];
    program(imports, &[0u8; 0x10], |a, l| {
        a.push(0)?;
        a.push(EWX_REBOOT)?;
        a.call(l.import(0))?;
        a.mov(eax, 9)?;
        a.ret()
    })
}

#[test]
fn exit_windows_ex_ends_the_program() {
    assert_eq!(run(Win98Se, "restart", restarts_windows()).0, 0);
}

/// A program that adds its command line and a line break to C:\RAN.TXT,
/// and ends with 5.
fn notes_its_command_line() -> Vec<u8> {
    const GENERIC_WRITE: i32 = 0x4000_0000;
    const OPEN_ALWAYS: i32 = 4;
    const FILE_END: i32 = 2;
    let kernel32 = [
        "GetCommandLineA",
        "lstrlenA",
        "CreateFileA",
        "SetFilePointer",
        "WriteFile",
        "CloseHandle",
    ];
    let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", &kernel32)];
    let mut data = b"C:\\RAN.TXT\0".to_vec();
    data.resize(0x20, 0);
    data.extend_from_slice(b"\r\n");
    data.resize(0x40, 0);
    program(imports, &data, |a, l| {
        a.call(l.import(0))?;
        a.mov(esi, eax)?;
        a.push(esi)?;
        a.call(l.import(1))?;
        a.mov(edi, eax)?;
        a.push(0)?;
        a.push(0x80)?;
        a.push(OPEN_ALWAYS)?;
        a.push(0)?;
        a.push(0)?;
        a.push(GENERIC_WRITE)?;
        a.push(l.at(0) as i32)?;
        a.call(l.import(2))?;
        a.mov(ebx, eax)?;
        a.push(FILE_END)?;
        a.push(0)?;
        a.push(0)?;
        a.push(ebx)?;
        a.call(l.import(3))?;
        for (at, len) in [(None, None), (Some(0x20), Some(2))] {
            a.push(0)?;
            a.push(l.at(0x30) as i32)?;
            match len {
                Some(len) => a.push(len)?,
                None => a.push(edi)?,
            }
            match at {
                Some(at) => a.push(l.at(at) as i32)?,
                None => a.push(esi)?,
            }
            a.push(ebx)?;
            a.call(l.import(4))?;
        }
        a.push(ebx)?;
        a.call(l.import(5))?;
        a.mov(eax, 5)?;
        a.ret()
    })
}

/// What a setup program leaves for the next boot: WININIT.INI's renames
/// and deletes, and RunOnce commands in the three keys a boot runs, each
/// running R.EXE, with one command whose program isn't there.
fn boot_work() -> Vec<(&'static str, Vec<u8>)> {
    let wininit = b"[Other]\r\nNUL=C:\\KEEP.TXT\r\n\r\n[rename]\r\nNUL=C:\\OLD.TMP\r\n\
        C:\\WINDOWS\\SYSTEM\\Q.DLL=C:\\WINDOWS\\SYSTEM\\Q.NEW\r\nC:\\NEW.DLL=C:\\NEW.TMP\r\n";
    let registry = "REGEDIT4\r\n\r\n\
        [HKEY_LOCAL_MACHINE\\Software\\Microsoft\\Windows\\CurrentVersion\\RunOnce]\r\n\
        \"first\"=\"C:\\\\R.EXE lm\"\r\n\"missing\"=\"C:\\\\NOWHERE.EXE x\"\r\n\r\n\
        [HKEY_USERS\\.Default\\Software\\Microsoft\\Windows\\CurrentVersion\\RunOnce]\r\n\
        \"user\"=\"R.EXE cu\"\r\n\r\n\
        [HKEY_LOCAL_MACHINE\\Software\\Microsoft\\Windows\\CurrentVersion\\RunServicesOnce]\r\n\
        \"service\"=\"C:\\\\R.EXE services\"\r\n";
    vec![
        ("R.EXE", notes_its_command_line()),
        ("REGISTRY.REG", registry.as_bytes().to_vec()),
        ("WINDOWS/WININIT.INI", wininit.to_vec()),
        ("WINDOWS/SYSTEM/Q.DLL", b"old".to_vec()),
        ("WINDOWS/SYSTEM/Q.NEW", b"new".to_vec()),
        ("OLD.TMP", b"x".to_vec()),
        ("KEEP.TXT", b"x".to_vec()),
        ("NEW.TMP", b"new".to_vec()),
    ]
}

/// Run `exe` from the folder `base`, as drive C:, to its end: its exit
/// code.
fn run_in(base: &std::path::Path, exe: &str) -> u8 {
    let mut cpu = Cpu::new(base.to_path_buf());
    cpu.bus.win32.enabled = true;
    cpu.bus.win32.version = Win98Se;
    cpu.host_console = Some(HostConsole::default());
    cpu.load_shell();
    assert!(cpu.load_executable(exe, None));
    for _ in 0..1000 {
        cpu.bus.start_batch(cpu.bus.clock.icount + 100_000);
        if run_batch(&mut cpu, &mut NoHook, false) == StopReason::ShellReloaded {
            return cpu.errorlevel;
        }
    }
    panic!("{} didn't end: {:?}", exe, printed(&cpu));
}

/// A folder for the test `name` with `files`, which may be in folders.
fn boot_scratch(name: &str, files: &[(&str, Vec<u8>)]) -> PathBuf {
    let base = scratch(name, &[]);
    for (file, bytes) in files {
        let path = base.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    base
}

/// Check that the boot's work in `boot_work` is done: the renames and
/// deletes, WININIT.INI renamed, the commands run in the boot's order
/// before `last`, and their values gone from the registry.
fn check_boot_done(base: &std::path::Path, last: Option<&str>) {
    let windows = base.join("WINDOWS");
    assert!(!base.join("OLD.TMP").exists());
    assert!(base.join("KEEP.TXT").exists(), "only [rename] counts");
    let system = windows.join("SYSTEM");
    assert_eq!(fs::read(system.join("Q.DLL")).unwrap(), b"new");
    assert!(!system.join("Q.NEW").exists());
    assert_eq!(fs::read(base.join("NEW.DLL")).unwrap(), b"new");
    assert!(!base.join("NEW.TMP").exists());
    assert!(!windows.join("WININIT.INI").exists());
    assert!(windows.join("WININIT.BAK").exists());
    let ran = fs::read_to_string(base.join("RAN.TXT")).unwrap();
    let mut lines: Vec<&str> = ran.lines().collect();
    if let Some(last) = last {
        assert!(lines.pop().unwrap().ends_with(last), "{:?}", ran);
    }
    assert_eq!(lines, ["C:\\R.EXE services", "C:\\R.EXE lm", "R.EXE cu"]);
    let registry = fs::read_to_string(base.join("REGISTRY.REG")).unwrap();
    for name in ["first", "missing", "user", "service"] {
        assert!(!registry.contains(&format!("\"{}\"", name)), "{}", registry);
    }
    assert!(registry.contains("CurrentVersion\\RunOnce]"));
}

#[test]
fn a_restart_does_the_work_left_for_the_boot() {
    let mut files = boot_work();
    files.push(("T.EXE", restarts_windows()));
    let base = boot_scratch("boot_restart", &files);
    // DOS gets the exit code of the program that asked for the restart,
    // not R.EXE's 5.
    assert_eq!(run_in(&base, "T.EXE"), 0);
    check_boot_done(&base, None);
}

#[test]
fn a_session_starts_with_the_work_left_for_the_boot() {
    let base = boot_scratch("boot_session", &boot_work());
    // R.EXE itself is the program DOS starts, and it runs last.
    assert_eq!(run_in(&base, "R.EXE"), 5);
    check_boot_done(&base, Some("R.EXE"));
    // The work is done once: the next session has none.
    fs::remove_file(base.join("RAN.TXT")).unwrap();
    assert_eq!(run_in(&base, "R.EXE"), 5);
    let ran = fs::read_to_string(base.join("RAN.TXT")).unwrap();
    assert_eq!(ran.lines().count(), 1, "{:?}", ran);
}

/// A program that makes C:\WINDOWS the current directory.
fn changes_directory() -> Vec<u8> {
    let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", &["SetCurrentDirectoryA"])];
    let mut data = b"C:\\WINDOWS\0".to_vec();
    data.resize(0x10, 0);
    program(imports, &data, |a, l| {
        a.push(l.at(0) as i32)?;
        a.call(l.import(0))?;
        a.mov(eax, 0)?;
        a.ret()
    })
}

/// A program that writes its current directory to C:\DIR.TXT.
fn notes_its_directory() -> Vec<u8> {
    const GENERIC_WRITE: i32 = 0x4000_0000;
    const CREATE_ALWAYS: i32 = 2;
    let kernel32 = [
        "GetCurrentDirectoryA",
        "CreateFileA",
        "WriteFile",
        "CloseHandle",
    ];
    let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", &kernel32)];
    let mut data = b"C:\\DIR.TXT\0".to_vec();
    data.resize(0x80, 0);
    program(imports, &data, |a, l| {
        a.push(l.at(0x20) as i32)?;
        a.push(0x40)?;
        a.call(l.import(0))?;
        a.mov(edi, eax)?;
        a.push(0)?;
        a.push(0x80)?;
        a.push(CREATE_ALWAYS)?;
        a.push(0)?;
        a.push(0)?;
        a.push(GENERIC_WRITE)?;
        a.push(l.at(0) as i32)?;
        a.call(l.import(1))?;
        a.mov(ebx, eax)?;
        a.push(0)?;
        a.push(l.at(0x70) as i32)?;
        a.push(edi)?;
        a.push(l.at(0x20) as i32)?;
        a.push(ebx)?;
        a.call(l.import(2))?;
        a.push(ebx)?;
        a.call(l.import(3))?;
        a.mov(eax, 0)?;
        a.ret()
    })
}

#[test]
fn the_program_dos_started_keeps_its_directory_after_the_commands() {
    let registry = "REGEDIT4\r\n\r\n\
        [HKEY_LOCAL_MACHINE\\Software\\Microsoft\\Windows\\CurrentVersion\\RunOnce]\r\n\
        \"cd\"=\"C:\\\\CD.EXE\"\r\n";
    let files = [
        ("CD.EXE", changes_directory()),
        ("D.EXE", notes_its_directory()),
        ("REGISTRY.REG", registry.as_bytes().to_vec()),
    ];
    let base = boot_scratch("boot_directory", &files);
    assert_eq!(run_in(&base, "D.EXE"), 0);
    let registry = fs::read_to_string(base.join("REGISTRY.REG")).unwrap();
    assert!(!registry.contains("\"cd\""), "the command ran: {}", registry);
    // CD.EXE's directory was its own: D.EXE starts in DOS's, the root.
    assert_eq!(fs::read_to_string(base.join("DIR.TXT")).unwrap(), "C:\\");
}

/// A setup program that leaves work for the next boot, a file to delete
/// in WININIT.INI and a RunOnce command, then ends the session with
/// ExitWindowsEx(`flags`). It ends with 9 if the call comes back.
fn leaves_work_and_exits_windows(flags: i32) -> Vec<u8> {
    const HKEY_LOCAL_MACHINE: i32 = 0x8000_0002_u32 as i32;
    const REG_SZ: i32 = 1;
    let imports: &[(&str, &[&str])] = &[
        ("KERNEL32.DLL", &["WritePrivateProfileStringA"]),
        (
            "ADVAPI32.DLL",
            &["RegCreateKeyA", "RegSetValueExA", "RegCloseKey"],
        ),
        ("USER32.DLL", &["ExitWindowsEx"]),
    ];
    let mut data = Vec::new();
    for (at, text) in [
        (0x00, &b"rename"[..]),
        (0x10, b"NUL"),
        (0x20, b"C:\\LATER.TMP"),
        (0x40, b"WININIT.INI"),
        (0x60, b"Software\\Microsoft\\Windows\\CurrentVersion\\RunOnce"),
        (0xA0, b"later"),
        (0xB0, b"C:\\R.EXE later"),
    ] {
        data.resize(at, 0);
        data.extend_from_slice(text);
        data.push(0);
    }
    data.resize(0x100, 0);
    program(imports, &data, |a, l| {
        for at in [0x40, 0x20, 0x10, 0x00] {
            a.push(l.at(at) as i32)?;
        }
        a.call(l.import(0))?;
        a.push(l.at(0xD0) as i32)?;
        a.push(l.at(0x60) as i32)?;
        a.push(HKEY_LOCAL_MACHINE)?;
        a.call(l.import(1))?;
        a.push(15)?;
        a.push(l.at(0xB0) as i32)?;
        a.push(REG_SZ)?;
        a.push(0)?;
        a.push(l.at(0xA0) as i32)?;
        a.push(dword_ptr(l.at(0xD0)))?;
        a.call(l.import(2))?;
        a.push(dword_ptr(l.at(0xD0)))?;
        a.call(l.import(3))?;
        a.push(0)?;
        a.push(flags)?;
        a.call(l.import(4))?;
        a.mov(eax, 9)?;
        a.ret()
    })
}

#[test]
fn a_restart_runs_what_the_session_left_for_it() {
    const EWX_REBOOT: i32 = 2;
    let files = [
        ("T.EXE", leaves_work_and_exits_windows(EWX_REBOOT)),
        ("R.EXE", notes_its_command_line()),
        ("LATER.TMP", b"x".to_vec()),
    ];
    let base = boot_scratch("boot_left_restart", &files);
    assert_eq!(run_in(&base, "T.EXE"), 0);
    assert!(!base.join("LATER.TMP").exists());
    assert!(base.join("WINDOWS").join("WININIT.BAK").exists());
    let ran = fs::read_to_string(base.join("RAN.TXT")).unwrap();
    assert_eq!(ran, "C:\\R.EXE later\r\n");
    let registry = fs::read_to_string(base.join("REGISTRY.REG")).unwrap();
    assert!(!registry.contains("\"later\""), "{}", registry);
}

#[test]
fn a_shutdown_leaves_the_work_for_the_next_session() {
    const EWX_SHUTDOWN: i32 = 1;
    let files = [
        ("T.EXE", leaves_work_and_exits_windows(EWX_SHUTDOWN)),
        ("R.EXE", notes_its_command_line()),
        ("LATER.TMP", b"x".to_vec()),
    ];
    let base = boot_scratch("boot_left_shutdown", &files);
    assert_eq!(run_in(&base, "T.EXE"), 0);
    let wininit = base.join("WINDOWS").join("WININIT.INI");
    assert!(wininit.exists(), "a shutdown doesn't boot");
    assert!(base.join("LATER.TMP").exists());
    assert!(!base.join("RAN.TXT").exists());
    // The next session, as in a later run of the emulator, boots first.
    assert_eq!(run_in(&base, "R.EXE"), 5);
    assert!(!wininit.exists());
    assert!(!base.join("LATER.TMP").exists());
    let ran = fs::read_to_string(base.join("RAN.TXT")).unwrap();
    let lines: Vec<&str> = ran.lines().collect();
    assert_eq!(lines.len(), 2, "{:?}", ran);
    assert_eq!(lines[0], "C:\\R.EXE later");
}

/// A program that ends with 7 when the DLL `dll` is a file of the system
/// directory (1), loads (2) and has `function` (4).
fn finds_system_dll(dll: &str, function: &str) -> Vec<u8> {
    let imports: &[(&str, &[&str])] = &[(
        "KERNEL32.DLL",
        &["GetFileAttributesA", "LoadLibraryA", "GetProcAddress"],
    )];
    let mut data = format!("C:\\WINDOWS\\SYSTEM\\{}\0", dll).into_bytes();
    data.resize(0x40, 0);
    data.extend_from_slice(format!("{}\0", dll).as_bytes());
    data.resize(0x60, 0);
    data.extend_from_slice(format!("{}\0", function).as_bytes());
    program(imports, &data, |a, l| {
        a.xor(ebx, ebx)?;
        a.push(l.at(0) as i32)?;
        a.call(l.import(0))?;
        a.cmp(eax, -1)?;
        a.setne(bl)?;
        a.push(l.at(0x40) as i32)?;
        a.call(l.import(1))?;
        a.test(eax, eax)?;
        let mut done = a.create_label();
        a.jz(done)?;
        a.or(ebx, 2)?;
        a.push(l.at(0x60) as i32)?;
        a.push(eax)?;
        a.call(l.import(2))?;
        a.test(eax, eax)?;
        a.jz(done)?;
        a.or(ebx, 4)?;
        a.set_label(&mut done)?;
        a.mov(eax, ebx)?;
        a.ret()
    })
}

/// One call of `asks_msacm32`: the function, its arguments, what it
/// returns, and what the program's output variable holds after it.
struct AcmCall(&'static str, &'static [i32], i32, Option<i32>);

/// Stand-ins in a call's arguments for the addresses of `asks_msacm32`'s
/// data, a handle no driver or stream has, and an address that isn't
/// memory the program has.
const OUT: i32 = -1;
const FORMAT: i32 = -2;
const HANDLE: i32 = 0x1234;
const UNMAPPED: i32 = 0x10;

/// acmStreamOpen's arguments: the first to convert, the second to ask if
/// a conversion can be done.
const STREAM: &[i32] = &[OUT, 0, FORMAT, FORMAT, 0, 0, 0, 0];
const STREAM_QUERY: &[i32] = &[0, 0, FORMAT, FORMAT, 0, 0, 0, 1];
const STREAM_OF_DRIVER: &[i32] = &[OUT, HANDLE, FORMAT, FORMAT, 0, 0, 0, 0];
const STREAM_NO_OUT: &[i32] = &[0, 0, FORMAT, FORMAT, 0, 0, 0, 0];
const STREAM_BAD_OUT: &[i32] = &[UNMAPPED, 0, FORMAT, FORMAT, 0, 0, 0, 0];
const STREAM_NO_FORMATS: &[i32] = &[OUT, 0, 0, 0, 0, 0, 0, 0];

/// What MSACM32's code answers without drivers, read from the code of
/// Windows 98 SE's 4.03.1998; MAX_SIZE_FORMAT is the size on a Windows
/// with its codecs, so programs that size a buffer by it run.
const ACM_CALLS: &[AcmCall] = &[
    AcmCall("acmMetrics", &[0, 50, OUT], 0, Some(50)),
    AcmCall("acmMetrics", &[0, 51, OUT], 512, Some(0)),
    AcmCall("acmMetrics", &[0, 1, OUT], 0, Some(0)),
    AcmCall("acmMetrics", &[0, 24, OUT], 0, Some(0)),
    AcmCall("acmMetrics", &[0, 30, OUT], 5, Some(-1)),
    AcmCall("acmMetrics", &[0, 100, OUT], 5, Some(0)),
    // The object goes first, then the metric, then the output.
    AcmCall("acmMetrics", &[HANDLE, 7, 0], 5, None),
    AcmCall("acmMetrics", &[0, 7, 0], 8, None),
    AcmCall("acmMetrics", &[0, 50, 0], 11, None),
    AcmCall("acmMetrics", &[0, 50, UNMAPPED], 11, None),
    AcmCall("acmStreamOpen", STREAM, 512, Some(0)),
    AcmCall("acmStreamOpen", STREAM_QUERY, 512, None),
    AcmCall("acmStreamOpen", STREAM_OF_DRIVER, 5, Some(0)),
    AcmCall("acmStreamOpen", STREAM_NO_OUT, 11, None),
    AcmCall("acmStreamOpen", STREAM_BAD_OUT, 11, None),
    AcmCall("acmStreamOpen", STREAM_NO_FORMATS, 11, Some(0)),
    AcmCall("acmStreamSize", &[HANDLE, 0, OUT, 0], 5, Some(0)),
    AcmCall("acmDriverOpen", &[OUT, HANDLE, 0], 5, Some(0)),
    AcmCall("acmDriverEnum", &[0, 0, 0], 11, None),
    AcmCall("acmDriverPriority", &[0, 0, 0x1_0000], 0, None),
    AcmCall("acmDriverPriority", &[0, 0, 0x2_0000], 0, None),
    AcmCall("acmDriverPriority", &[0, 0, 0], 10, None),
    AcmCall("acmDriverPriority", &[HANDLE, 0, 0], 5, None),
    AcmCall("acmFormatEnumA", &[0, 0, 0, 0, 0], 512, None),
    AcmCall("acmFormatTagEnumA", &[0, 0, 0, 0, 0], 0, None),
    AcmCall("acmFilterEnumA", &[0, 0, 0, 0, 0], 0, None),
    AcmCall("acmFormatDetailsA", &[0, 0, 0], 6, None),
    AcmCall("acmFilterDetailsA", &[0, 0, 0], 6, None),
    AcmCall("acmFormatTagDetailsA", &[0, 0, 1], 512, None),
];

/// A program that asks acmMetrics for the largest wave format as Test
/// Drive 4 does, into a variable on its stack, and then makes the calls
/// of `ACM_CALLS`. It ends with 0, with 255 if the question failed, or
/// with the number from 1 of the call that didn't answer as listed.
fn asks_msacm32() -> Vec<u8> {
    let mut names: Vec<&str> = ACM_CALLS.iter().map(|c| c.0).collect();
    names.sort();
    names.dedup();
    let imports: &[(&str, &[&str])] = &[("MSACM32.DLL", &names)];
    // The output variable, then a 22 kHz 8-bit mono PCM format.
    let mut data = vec![0xFFu8; 4];
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&22050u32.to_le_bytes());
    data.extend_from_slice(&22050u32.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&8u16.to_le_bytes());
    program(imports, &data, |a, l| {
        let mut failed = a.create_label();
        a.mov(ebx, 255)?;
        a.push(0)?;
        a.push(esp)?;
        a.push(50)?;
        a.push(0)?;
        a.call(l.import(names.iter().position(|n| *n == "acmMetrics").unwrap()))?;
        a.pop(ecx)?;
        a.test(eax, eax)?;
        a.jnz(failed)?;
        a.cmp(ecx, 50)?;
        a.jne(failed)?;
        for (number, call) in ACM_CALLS.iter().enumerate() {
            a.mov(ebx, number as i32 + 1)?;
            a.mov(dword_ptr(l.at(0)), -1)?;
            for &arg in call.1.iter().rev() {
                a.push(match arg {
                    OUT => l.at(0) as i32,
                    FORMAT => l.at(4) as i32,
                    other => other,
                })?;
            }
            a.call(l.import(names.iter().position(|n| *n == call.0).unwrap()))?;
            a.cmp(eax, call.2)?;
            a.jne(failed)?;
            if let Some(value) = call.3 {
                a.cmp(dword_ptr(l.at(0)), value)?;
                a.jne(failed)?;
            }
        }
        a.xor(ebx, ebx)?;
        a.set_label(&mut failed)?;
        a.mov(eax, ebx)?;
        a.ret()
    })
}

#[test]
fn msacm32_answers_without_codecs() {
    assert_eq!(run(Win98Se, "msacm32", asks_msacm32()).0, 0);
    assert_eq!(run(Win95, "msacm32", asks_msacm32()).0, 0);
}

/// A program that ends with acmGetVersion's minor version.
fn asks_acm_version() -> Vec<u8> {
    let imports: &[(&str, &[&str])] = &[("MSACM32.DLL", &["acmGetVersion"])];
    program(imports, &[0u8; 4], |a, l| {
        a.call(l.import(0))?;
        a.shr(eax, 16)?;
        a.ret()
    })
}

#[test]
fn msacm32_has_the_version_of_its_windows() {
    assert_eq!(run(Win95, "acm_version", asks_acm_version()).0, 0);
    assert_eq!(run(Win98, "acm_version", asks_acm_version()).0, 3);
    assert_eq!(run(Win98Se, "acm_version", asks_acm_version()).0, 3);
}

/// A program that imports DCIMAN32.DLL as ActiveMovie's QUARTZ.DLL does,
/// and ends with a bit for each answer that is Windows' on a display
/// driver without DCI: 1 if DCIOpenProvider gives no DC, 2 if
/// DCICreatePrimary fails with DCI_FAIL_UNSUPPORTED and 4 if it clears
/// the surface pointer, 8 if GetDCRegionData sizes the desktop DC's
/// visible region as one rectangle (an RGNDATA of 48 bytes) and 16 if
/// it writes that one, and 32 if GetWindowRegionData sizes the desktop
/// window's the same.
fn asks_dciman32() -> Vec<u8> {
    const DCI_FAIL_UNSUPPORTED: i32 = -4;
    let imports: &[(&str, &[&str])] = &[
        (
            "DCIMAN32.DLL",
            &[
                "DCIOpenProvider",
                "DCICreatePrimary",
                "GetDCRegionData",
                "GetWindowRegionData",
            ],
        ),
        ("USER32.DLL", &["GetDC", "GetDesktopWindow"]),
    ];
    let mut data = vec![0xFFu8; 4];
    data.resize(0x100, 0);
    program(imports, &data, |a, l| {
        a.xor(ebx, ebx)?;
        a.call(l.import(0))?;
        a.test(eax, eax)?;
        let mut provider = a.create_label();
        a.jnz(provider)?;
        a.or(ebx, 1)?;
        a.set_label(&mut provider)?;

        a.push(l.at(0) as i32)?;
        a.push(0)?;
        a.call(l.import(1))?;
        a.cmp(eax, DCI_FAIL_UNSUPPORTED)?;
        let mut primary = a.create_label();
        a.jne(primary)?;
        a.or(ebx, 2)?;
        a.set_label(&mut primary)?;
        a.cmp(dword_ptr(l.at(0)), 0)?;
        let mut cleared = a.create_label();
        a.jne(cleared)?;
        a.or(ebx, 4)?;
        a.set_label(&mut cleared)?;

        a.push(0)?;
        a.call(l.import(4))?;
        a.mov(esi, eax)?;
        a.push(0)?;
        a.push(0)?;
        a.push(esi)?;
        a.call(l.import(2))?;
        a.cmp(eax, 48)?;
        let mut sized = a.create_label();
        a.jne(sized)?;
        a.or(ebx, 8)?;
        a.set_label(&mut sized)?;

        a.push(l.at(0x10) as i32)?;
        a.push(0xF0)?;
        a.push(esi)?;
        a.call(l.import(2))?;
        a.cmp(eax, 48)?;
        let mut written = a.create_label();
        a.jne(written)?;
        // One rectangle, of RDH_RECTANGLES, starting at the screen's
        // corner.
        a.cmp(dword_ptr(l.at(0x18)), 1)?;
        a.jne(written)?;
        a.cmp(dword_ptr(l.at(0x14)), 1)?;
        a.jne(written)?;
        a.cmp(dword_ptr(l.at(0x30)), 0)?;
        a.jne(written)?;
        a.or(ebx, 16)?;
        a.set_label(&mut written)?;

        a.call(l.import(5))?;
        a.push(0)?;
        a.push(0)?;
        a.push(eax)?;
        a.call(l.import(3))?;
        a.cmp(eax, 48)?;
        let mut window = a.create_label();
        a.jne(window)?;
        a.or(ebx, 32)?;
        a.set_label(&mut window)?;
        a.mov(eax, ebx)?;
        a.ret()
    })
}

#[test]
fn dciman32_has_no_provider_and_gives_visible_regions() {
    assert_eq!(run(Win98Se, "dciman32", asks_dciman32()).0, 63);
    assert_eq!(run(Win95, "dciman32", asks_dciman32()).0, 63);
}

#[test]
fn msimg32_dll_comes_with_windows_98() {
    let finds = || finds_system_dll("MSIMG32.DLL", "GradientFill");
    assert_eq!(run(Win95, "msimg32", finds()).0, 0);
    assert_eq!(run(Win98, "msimg32", finds()).0, 7);
    assert_eq!(run(Win98Se, "msimg32", finds()).0, 7);
}

/// Windows 95 and 98 both have an NTDLL.DLL with RtlUnwind, which
/// Microsoft's D3DRM.DLL imports (Submarine Titans ships it beside the
/// game), as does DirectX 6.1a's ADVPACK.DLL.
#[test]
fn ntdll_dll_is_there_under_windows_95_and_98() {
    let finds = || finds_system_dll("NTDLL.DLL", "RtlUnwind");
    assert_eq!(run(Win95, "ntdll", finds()).0, 7);
    assert_eq!(run(Win98, "ntdll", finds()).0, 7);
    assert_eq!(run(Win98Se, "ntdll", finds()).0, 7);
}
