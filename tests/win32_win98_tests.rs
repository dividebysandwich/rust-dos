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
/// 80 MB: about 1 GB when empty, which GetDiskFreeSpaceA gives in 32 KB
/// clusters, fewer than 65536 of them, as Windows 95 and 98 do.
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
        a.ja(0)?;
        // Free bytes: under 1 GB by what T.EXE takes, at most 1 GB.
        a.mov(eax, dword_ptr(l.at(0x08)))?;
        a.shl(eax, 15)?;
        a.cmp(eax, 0x3FF0_0000)?;
        a.jb(0)?;
        a.cmp(eax, 0x4000_0000)?;
        a.ja(0)?;
        a.mov(ebx, eax)?;
        // GetDiskFreeSpaceExA's free bytes, in 4 KB clusters: within 32 KB
        // of GetDiskFreeSpaceA's, with nothing above 4 GB.
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
        a.sub(eax, ebx)?;
        a.cmp(eax, 0x8000)?;
        a.jae(0)?;
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
