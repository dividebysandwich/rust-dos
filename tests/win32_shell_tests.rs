//! The built-in Windows' shell: special folders made on first use, a
//! shell link saved through IShellLinkA and IPersistFile, a Program
//! Manager group made through DDEML as InstallShield makes one, a file
//! copied by SHFileOperationA, and FindExecutableA following the
//! registry's associations. Each runs on the interpreter and on the
//! dynamic recompiler. The programs are PE files made with rust-dos-pe's
//! builder, their code assembled with iced.

use iced_x86::code_asm::*;
use rust_dos::cpu::{CoreMode, Cpu, HostConsole};
use rust_dos::exec::{NoHook, StopReason, run_batch};
use rust_dos_pe::{self as pe, Builder, Imp};
use std::fs;
use std::path::PathBuf;

const BASE: u32 = 0x40_0000;
const CORES: [CoreMode; 2] = [CoreMode::Normal, CoreMode::Dynamic];

/// Where a test program's data and imports are, for its code.
struct Layout {
    data: u32,
    iat: Vec<u32>,
}

impl Layout {
    /// The address of byte `offset` of the data.
    fn at(&self, offset: u32) -> i32 {
        (self.data + offset) as i32
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
    let iat = b.imports(&dlls).concat();
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
    let flags = pe::SCN_CODE | pe::SCN_EXECUTE | pe::SCN_READ;
    assert_eq!(b.section(".text", &code, flags), code_rva);
    b.entry(code_rva);
    b.build()
}

/// The folder of a test's drive C:.
fn scratch_path(name: &str) -> PathBuf {
    PathBuf::from("target/test_win32_shell").join(name)
}

/// A fresh folder for a test's drive C:, with `files` in it.
fn scratch(name: &str, files: &[(&str, Vec<u8>)]) -> PathBuf {
    let base = scratch_path(name);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    for (file, bytes) in files {
        fs::write(base.join(file), bytes).unwrap();
    }
    base
}

/// A machine on `core` with the built-in Windows on and `program`
/// loaded from the shell as T.EXE, with `files` on drive C:.
fn machine(test: &str, core: CoreMode, files: &[(&str, Vec<u8>)]) -> Cpu {
    let mut cpu = Cpu::new(scratch(test, files));
    cpu.core = core;
    cpu.bus.win32.enabled = true;
    cpu.host_console = Some(HostConsole::default());
    cpu.load_shell();
    assert!(cpu.load_executable("T.EXE", None));
    cpu
}

/// Run the machine until the program ends.
fn run_to_exit(cpu: &mut Cpu) {
    for _ in 0..20_000 {
        cpu.bus.start_batch(cpu.bus.clock.icount + 100_000);
        if run_batch(cpu, &mut NoHook, false) == StopReason::ShellReloaded {
            return;
        }
    }
    panic!("the program didn't end: {:?}", printed(cpu));
}

/// What the program printed, to standard output and error.
fn printed(cpu: &Cpu) -> String {
    let console = cpu.host_console.as_ref().unwrap();
    let mut text = console.stdout.clone();
    text.extend(&console.stderr);
    String::from_utf8_lossy(&text).into_owned()
}

/// Put `bytes` at `offset` of `data`.
fn put(data: &mut [u8], offset: usize, bytes: &[u8]) {
    data[offset..offset + bytes.len()].copy_from_slice(bytes);
}

/// `text` as a zero-ended UTF-16 string.
fn wide(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain([0])
        .flat_map(|c| c.to_le_bytes())
        .collect()
}

/// Call the import `slot` with `args`, pushed last first.
fn call(a: &mut CodeAssembler, slot: AsmMemoryOperand, args: &[i32]) -> Result<(), IcedError> {
    for &arg in args.iter().rev() {
        a.push(arg)?;
    }
    a.call(slot)
}

/// Call method `index` of the interface whose pointer is at `at`, with
/// `args` after the interface.
fn call_method(a: &mut CodeAssembler, at: i32, index: i32, args: &[i32]) -> Result<(), IcedError> {
    for &arg in args.iter().rev() {
        a.push(arg)?;
    }
    a.mov(eax, dword_ptr(at as u64))?;
    a.push(eax)?;
    a.mov(eax, dword_ptr(eax))?;
    a.call(dword_ptr(eax + 4 * index))
}

/// Set bit `bit` of EBX when EAX is `value`.
fn check_eax(a: &mut CodeAssembler, value: i32, bit: u32) -> Result<(), IcedError> {
    let mut skip = a.create_label();
    a.cmp(eax, value)?;
    a.jne(skip)?;
    a.or(ebx, 1 << bit)?;
    a.set_label(&mut skip)?;
    a.nop()
}

/// Set bit `bit` of EBX when EAX isn't 0.
fn check_nonzero(a: &mut CodeAssembler, bit: u32) -> Result<(), IcedError> {
    let mut skip = a.create_label();
    a.test(eax, eax)?;
    a.je(skip)?;
    a.or(ebx, 1 << bit)?;
    a.set_label(&mut skip)?;
    a.nop()
}

/// End the program with EBX as its exit code.
fn exit(a: &mut CodeAssembler, slot: AsmMemoryOperand) -> Result<(), IcedError> {
    a.push(ebx)?;
    a.call(slot)?;
    a.int3()
}

/// The host path of `path` (backslashes) on the test's drive C:,
/// whatever the case of its names on the host.
fn host_file(test: &str, path: &str) -> Option<PathBuf> {
    let mut at = scratch_path(test);
    for part in path.split('\\') {
        let entries = fs::read_dir(&at).ok()?;
        let found = entries
            .filter_map(Result::ok)
            .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(part))?;
        at = found.path();
    }
    Some(at)
}

#[test]
fn a_special_folder_is_made_when_asked_for() {
    const CSIDL_PROGRAMS: i32 = 2;
    let imports: &[(&str, &[&str])] = &[
        ("KERNEL32.dll", &["ExitProcess"]),
        ("SHELL32.dll", &["SHGetSpecialFolderPathA"]),
    ];
    let code = program(imports, &[0; 0x110], |a, l| {
        a.xor(ebx, ebx)?;
        call(a, l.import(1), &[0, l.at(0), CSIDL_PROGRAMS, 1])?;
        check_nonzero(a, 0)?;
        exit(a, l.import(0))
    });
    for core in CORES {
        let mut cpu = machine("folders", core, &[("T.EXE", code.clone())]);
        run_to_exit(&mut cpu);
        assert_eq!(cpu.errorlevel, 1, "{core:?}: {}", printed(&cpu));
        let dir = host_file("folders", "WINDOWS\\Start Menu\\Programs");
        assert!(dir.is_some_and(|d| d.is_dir()), "{core:?}");
    }
}

#[test]
fn a_shell_link_is_saved_through_ishelllink_and_ipersistfile() {
    // CLSID_ShellLink, IID_IShellLinkA and IID_IPersistFile.
    let guid = |d1: u32, d2: u16| -> Vec<u8> {
        let mut g = d1.to_le_bytes().to_vec();
        g.extend(d2.to_le_bytes());
        g.extend([0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
        g
    };
    let mut data = vec![0u8; 0x100];
    put(&mut data, 0x00, &guid(0x0002_1401, 0));
    put(&mut data, 0x10, &guid(0x0002_14EE, 0));
    put(&mut data, 0x20, &guid(0x0000_010B, 0));
    // 0x30: the link, 0x34: its IPersistFile.
    put(&mut data, 0x40, b"C:\\T.EXE\0");
    put(&mut data, 0x60, &wide("C:\\T.LNK"));
    let imports: &[(&str, &[&str])] = &[
        ("KERNEL32.dll", &["ExitProcess"]),
        ("OLE32.dll", &["CoInitialize", "CoCreateInstance"]),
    ];
    let code = program(imports, &data, |a, l| {
        const CLSCTX_INPROC_SERVER: i32 = 1;
        a.xor(ebx, ebx)?;
        call(a, l.import(1), &[0])?;
        let args = [l.at(0), 0, CLSCTX_INPROC_SERVER, l.at(0x10), l.at(0x30)];
        call(a, l.import(2), &args)?;
        check_eax(a, 0, 0)?;
        // IShellLinkA::SetPath, QueryInterface, IPersistFile::Save.
        call_method(a, l.at(0x30), 20, &[l.at(0x40)])?;
        check_eax(a, 0, 1)?;
        call_method(a, l.at(0x30), 0, &[l.at(0x20), l.at(0x34)])?;
        check_eax(a, 0, 2)?;
        call_method(a, l.at(0x34), 6, &[l.at(0x60), 1])?;
        check_eax(a, 0, 3)?;
        exit(a, l.import(0))
    });
    for core in CORES {
        let mut cpu = machine("link", core, &[("T.EXE", code.clone())]);
        run_to_exit(&mut cpu);
        assert_eq!(cpu.errorlevel, 0xF, "{core:?}: {}", printed(&cpu));
        let link = fs::read(host_file("link", "T.LNK").unwrap()).unwrap();
        assert_eq!(&link[..4], &[0x4C, 0, 0, 0], "{core:?}");
        let text = String::from_utf8_lossy(&link);
        assert!(text.contains("C:\\T.EXE"), "{core:?}: {text:?}");
    }
}

#[test]
fn program_manager_makes_a_group_through_ddeml() {
    const APPCMD_CLIENTONLY: i32 = 0x10;
    const CP_WINANSI: i32 = 1004;
    const XTYP_EXECUTE: i32 = 0x4050;
    let command = b"[CreateGroup(Games)][AddItem(C:\\T.EXE,Test)]\0";
    let mut data = vec![0u8; 0x100];
    // 0x00: the instance, 0x04: the result.
    put(&mut data, 0x10, b"PROGMAN\0");
    put(&mut data, 0x20, command);
    let imports: &[(&str, &[&str])] = &[
        ("KERNEL32.dll", &["ExitProcess"]),
        (
            "USER32.dll",
            &[
                "DdeInitializeA",
                "DdeCreateStringHandleA",
                "DdeConnect",
                "DdeClientTransaction",
                "DdeDisconnect",
                "DdeUninitialize",
            ],
        ),
    ];
    let code = program(imports, &data, |a, l| {
        // The DDE callback's address in EBP: it answers 0 to all.
        let mut start = a.create_label();
        a.call(start)?;
        a.xor(eax, eax)?;
        a.ret_1(32)?;
        a.set_label(&mut start)?;
        a.pop(ebp)?;
        a.xor(ebx, ebx)?;
        a.push(0)?;
        a.push(APPCMD_CLIENTONLY)?;
        a.push(ebp)?;
        a.push(l.at(0))?;
        a.call(l.import(1))?;
        check_eax(a, 0, 0)?;
        a.mov(esi, dword_ptr(l.at(0) as u64))?;
        a.push(CP_WINANSI)?;
        a.push(l.at(0x10))?;
        a.push(esi)?;
        a.call(l.import(2))?;
        a.mov(edi, eax)?;
        // DdeConnect(instance, service, topic, context).
        a.push(0)?;
        a.push(edi)?;
        a.push(edi)?;
        a.push(esi)?;
        a.call(l.import(3))?;
        check_nonzero(a, 1)?;
        a.mov(edi, eax)?;
        let len = command.len() as i32;
        a.push(l.at(4))?;
        a.push(1000)?;
        a.push(XTYP_EXECUTE)?;
        a.push(0)?;
        a.push(0)?;
        a.push(edi)?;
        a.push(len)?;
        a.push(l.at(0x20))?;
        a.call(l.import(4))?;
        check_nonzero(a, 2)?;
        a.push(edi)?;
        a.call(l.import(5))?;
        a.push(esi)?;
        a.call(l.import(6))?;
        exit(a, l.import(0))
    });
    for core in CORES {
        let mut cpu = machine("progman", core, &[("T.EXE", code.clone())]);
        run_to_exit(&mut cpu);
        assert_eq!(cpu.errorlevel, 7, "{core:?}: {}", printed(&cpu));
        let item = "WINDOWS\\Start Menu\\Programs\\Games\\Test.lnk";
        let link = fs::read(host_file("progman", item).expect(item)).unwrap();
        assert_eq!(&link[..4], &[0x4C, 0, 0, 0], "{core:?}");
    }
}

#[test]
fn shfileoperation_copies_a_file() {
    const FO_COPY: u8 = 2;
    const FOF_SILENT_NOCONFIRMATION: u16 = 0x14;
    let mut data = vec![0u8; 0x80];
    // SHFILEOPSTRUCTA (packed): hwnd, wFunc, pFrom, pTo, fFlags.
    data[4] = FO_COPY;
    put(&mut data, 16, &FOF_SILENT_NOCONFIRMATION.to_le_bytes());
    put(&mut data, 0x40, b"C:\\A.TXT\0\0");
    put(&mut data, 0x60, b"C:\\B.TXT\0\0");
    let imports: &[(&str, &[&str])] = &[
        ("KERNEL32.dll", &["ExitProcess"]),
        ("SHELL32.dll", &["SHFileOperationA"]),
    ];
    let code = program(imports, &data, |a, l| {
        a.mov(dword_ptr(l.at(8) as u64), l.at(0x40))?;
        a.mov(dword_ptr(l.at(12) as u64), l.at(0x60))?;
        a.xor(ebx, ebx)?;
        call(a, l.import(1), &[l.at(0)])?;
        check_eax(a, 0, 0)?;
        exit(a, l.import(0))
    });
    for core in CORES {
        let files = [("T.EXE", code.clone()), ("A.TXT", b"copied".to_vec())];
        let mut cpu = machine("fileop", core, &files);
        run_to_exit(&mut cpu);
        assert_eq!(cpu.errorlevel, 1, "{core:?}: {}", printed(&cpu));
        let copy = fs::read(host_file("fileop", "B.TXT").unwrap()).unwrap();
        assert_eq!(copy, b"copied", "{core:?}");
    }
}

#[test]
fn findexecutable_follows_the_registry_association() {
    const HKEY_CLASSES_ROOT: i32 = 0x8000_0000_u32 as i32;
    const REG_SZ: i32 = 1;
    let mut data = vec![0u8; 0x200];
    put(&mut data, 0x00, b".tst\0");
    put(&mut data, 0x10, b"tstfile\0");
    put(&mut data, 0x20, b"tstfile\\shell\\open\\command\0");
    put(&mut data, 0x40, b"C:\\T.EXE \"%1\"\0");
    put(&mut data, 0x60, b"C:\\A.TST\0");
    // 0x100: the program FindExecutableA finds; 0x1F0: bytes written.
    let imports: &[(&str, &[&str])] = &[
        (
            "KERNEL32.dll",
            &["ExitProcess", "GetStdHandle", "WriteFile", "lstrlenA"],
        ),
        ("ADVAPI32.dll", &["RegSetValueA"]),
        ("SHELL32.dll", &["FindExecutableA"]),
    ];
    let code = program(imports, &data, |a, l| {
        a.xor(ebx, ebx)?;
        call(
            a,
            l.import(4),
            &[HKEY_CLASSES_ROOT, l.at(0), REG_SZ, l.at(0x10), 7],
        )?;
        call(
            a,
            l.import(4),
            &[HKEY_CLASSES_ROOT, l.at(0x20), REG_SZ, l.at(0x40), 13],
        )?;
        call(a, l.import(5), &[l.at(0x60), 0, l.at(0x100)])?;
        check_eax(a, 33, 0)?;
        call(a, l.import(3), &[l.at(0x100)])?;
        a.mov(esi, eax)?;
        call(a, l.import(1), &[-11])?;
        a.push(0)?;
        a.push(l.at(0x1F0))?;
        a.push(esi)?;
        a.push(l.at(0x100))?;
        a.push(eax)?;
        a.call(l.import(2))?;
        exit(a, l.import(0))
    });
    for core in CORES {
        let files = [("T.EXE", code.clone()), ("A.TST", b"x".to_vec())];
        let mut cpu = machine("find", core, &files);
        run_to_exit(&mut cpu);
        assert_eq!(cpu.errorlevel, 1, "{core:?}: {}", printed(&cpu));
        assert_eq!(printed(&cpu), "C:\\T.EXE", "{core:?}");
    }
}
