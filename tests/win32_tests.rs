//! The built-in Windows (win32): Win32 console programs started from the
//! DOS prompt, which call the built-in KERNEL32 and end back at the
//! prompt. The programs are PE files made here with rust-dos-pe's builder,
//! their code assembled with iced.

use iced_x86::code_asm::*;
use rust_dos::cpu::{CoreMode, Cpu, HostConsole};
use rust_dos::exec::{NoHook, StopReason, run_batch};
use rust_dos_pe::{self as pe, Builder, Imp};
use std::fs;
use std::path::PathBuf;

const BASE: u32 = 0x40_0000;
const STD_OUTPUT_HANDLE: i32 = -11;

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

/// A program at `base` importing `imports`, with `data` and the code `f`
/// makes. With `relocatable` it has relocations for the addresses in its
/// code and data that point into it, found by their values.
fn program(
    base: u32,
    relocatable: bool,
    imports: &[(&str, &[&str])],
    data: &[u8],
    f: impl FnOnce(&mut CodeAssembler, &Layout) -> Result<(), IcedError>,
) -> Vec<u8> {
    let mut b = Builder::exe(base);
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
        data: base + data_rva,
        iat,
    };
    let code_rva = b.next_rva();
    let mut a = CodeAssembler::new(32).unwrap();
    f(&mut a, &layout).unwrap();
    let code = a.assemble((base + code_rva) as u64).unwrap();
    assert_eq!(
        b.section(
            ".text",
            &code,
            pe::SCN_CODE | pe::SCN_EXECUTE | pe::SCN_READ
        ),
        code_rva
    );
    let end = b.next_rva();
    if relocatable {
        let mut words = |section: &[u8], rva: u32| {
            for i in 0..section.len().saturating_sub(3) {
                let value = u32::from_le_bytes(section[i..i + 4].try_into().unwrap());
                if (base..base + end).contains(&value) {
                    b.reloc(rva + i as u32);
                }
            }
        };
        words(data, data_rva);
        words(&code, code_rva);
    }
    b.entry(code_rva);
    b.build()
}

fn scratch(name: &str, files: &[(&str, Vec<u8>)]) -> PathBuf {
    let base = PathBuf::from("target/test_win32").join(name);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    for (file, bytes) in files {
        fs::write(base.join(file), bytes).unwrap();
    }
    base
}

/// A machine with the built-in Windows on, `program` loaded from the
/// shell as T.EXE, and what it prints kept.
fn machine(test: &str, program: Vec<u8>) -> Cpu {
    let mut cpu = Cpu::new(scratch(test, &[("T.EXE", program)]));
    cpu.bus.win32.enabled = true;
    cpu.host_console = Some(HostConsole::default());
    cpu.load_shell();
    assert!(cpu.load_executable("T.EXE", None));
    cpu
}

/// Run the machine until the program ends and the shell is back, for at
/// most `batches` batches.
fn run_to_exit(cpu: &mut Cpu, batches: usize) -> bool {
    for _ in 0..batches {
        cpu.bus.start_batch(cpu.bus.clock.icount + 100_000);
        if run_batch(cpu, &mut NoHook, false) == StopReason::ShellReloaded {
            return true;
        }
    }
    false
}

/// What the program printed, to standard output and error.
fn printed(cpu: &Cpu) -> String {
    let console = cpu.host_console.as_ref().unwrap();
    let mut text = console.stdout.clone();
    text.extend(&console.stderr);
    String::from_utf8_lossy(&text).into_owned()
}

/// Run `program` to its end; its exit code and what it printed.
fn run(test: &str, program: Vec<u8>) -> (u8, String) {
    let mut cpu = machine(test, program);
    assert!(
        run_to_exit(&mut cpu, 200),
        "the program didn't end: {:?}",
        printed(&cpu)
    );
    assert!(!cpu.bus.win32.active());
    (cpu.errorlevel, printed(&cpu))
}

const HELLO: &[u8] = b"Hello from Win32\r\n";

/// Writes HELLO to standard output with WriteFile, then ends with the
/// count WriteFile says it wrote.
fn hello(base: u32, relocatable: bool) -> Vec<u8> {
    let mut data = HELLO.to_vec();
    data.resize(0x40, 0);
    // A pointer to the text, which a relocation moves with the image.
    data[0x20..0x24].copy_from_slice(&(base + 0x2000).to_le_bytes());
    let imports: &[(&str, &[&str])] = &[(
        "KERNEL32.dll",
        &["GetStdHandle", "WriteFile", "ExitProcess"],
    )];
    program(base, relocatable, imports, &data, |a, l| {
        a.push(STD_OUTPUT_HANDLE)?;
        a.call(l.import(0))?;
        a.push(0)?;
        a.push(l.at(0x30) as i32)?;
        a.push(HELLO.len() as i32)?;
        a.push(dword_ptr(l.at(0x20)))?;
        a.push(eax)?;
        a.call(l.import(1))?;
        a.push(dword_ptr(l.at(0x30)))?;
        a.call(l.import(2))?;
        a.int3()
    })
}

#[test]
fn a_program_writes_to_its_console_and_exits() {
    let (code, text) = run("hello", hello(BASE, false));
    assert_eq!(text, "Hello from Win32\r\n");
    assert_eq!(code as usize, HELLO.len());
}

#[test]
fn a_write_of_more_than_64_kb_is_counted_whole() {
    // 70000 bytes, past what DOS counts in 16 bits: the count's high word
    // is the exit code.
    const LEN: u32 = 70_000;
    let mut data = vec![0u8; 0x40];
    data.resize(0x40 + LEN as usize, b'x');
    let imports: &[(&str, &[&str])] = &[(
        "KERNEL32.dll",
        &["GetStdHandle", "WriteFile", "ExitProcess"],
    )];
    let exe = program(BASE, false, imports, &data, |a, l| {
        a.push(STD_OUTPUT_HANDLE)?;
        a.call(l.import(0))?;
        a.push(0)?;
        a.push(l.at(0x30) as i32)?;
        a.push(LEN as i32)?;
        a.push(l.at(0x40) as i32)?;
        a.push(eax)?;
        a.call(l.import(1))?;
        a.mov(eax, dword_ptr(l.at(0x30)))?;
        a.shr(eax, 16)?;
        a.push(eax)?;
        a.call(l.import(2))?;
        a.int3()
    });
    let mut cpu = machine("write_64k", exe);
    assert!(run_to_exit(&mut cpu, 2000), "{:?}", printed(&cpu));
    assert_eq!(cpu.errorlevel, (LEN >> 16) as u8);
    assert_eq!(printed(&cpu).len(), LEN as usize);
}

#[test]
fn a_program_runs_on_the_dynamic_recompiler() {
    let mut cpu = machine("dynamic", hello(BASE, false));
    cpu.core = CoreMode::Dynamic;
    assert!(run_to_exit(&mut cpu, 200));
    assert_eq!(printed(&cpu), "Hello from Win32\r\n");
    assert_eq!(cpu.errorlevel as usize, HELLO.len());
}

#[test]
fn a_program_is_relocated_when_its_base_is_taken() {
    // 90000000h is in the shared arena, where no program goes.
    let (code, text) = run("relocated", hello(0x9000_0000, true));
    assert_eq!(text, "Hello from Win32\r\n");
    assert_eq!(code as usize, HELLO.len());
}

#[test]
fn a_program_that_must_stay_at_its_base_is_refused() {
    let (code, text) = run("fixed", hello(0x9000_0000, false));
    assert_eq!(code, 1);
    assert!(text.contains("must be loaded at 90000000h"), "{}", text);
}

#[test]
fn without_the_built_in_windows_the_dos_stub_runs() {
    let mut cpu = machine("off", hello(BASE, false));
    cpu.bus.win32.enabled = false;
    // Loaded again, now with it off.
    cpu.load_shell();
    assert!(cpu.load_executable("T.EXE", None));
    assert!(run_to_exit(&mut cpu, 200));
    assert!(
        printed(&cpu).contains("This program cannot be run in DOS mode."),
        "{}",
        printed(&cpu)
    );
    assert_eq!(cpu.errorlevel, 1);
}

#[test]
fn returning_from_the_entry_point_ends_with_its_value() {
    let (code, _) = run(
        "return",
        program(BASE, false, &[], &[0], |a, _| {
            a.mov(eax, 0x2A)?;
            a.ret()
        }),
    );
    assert_eq!(code, 0x2A);
}

#[test]
fn a_function_the_built_in_windows_lacks_ends_the_program() {
    let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", &["GetVersion", "FlushViewOfFile"])];
    let (code, text) = run(
        "missing",
        program(BASE, false, imports, &[0], |a, l| {
            a.call(l.import(1))?;
            a.mov(eax, 0x2A)?;
            a.ret()
        }),
    );
    assert_eq!(code, 0xFF);
    assert!(
        text.contains("T.EXE needs KERNEL32.DLL's function FlushViewOfFile"),
        "{}",
        text
    );
}

#[test]
fn a_program_importing_an_unknown_dll_is_refused() {
    let imports: &[(&str, &[&str])] = &[
        ("KERNEL32.DLL", &["ExitProcess"]),
        ("USER32.DLL", &["MessageBoxA"]),
    ];
    let (code, text) = run(
        "unknown_dll",
        program(BASE, false, imports, &[0], |a, _| a.ret()),
    );
    assert_eq!(code, 1);
    assert!(text.contains("T.EXE needs USER32.DLL"), "{}", text);
}

#[test]
fn an_exception_ends_the_program_with_a_message() {
    let (code, text) = run("exception", program(BASE, false, &[], &[0], |a, _| a.ud2()));
    assert_eq!(code, 0xFF);
    assert!(
        text.contains("T.EXE ended: invalid opcode (exception 06h)"),
        "{}",
        text
    );
}

#[test]
fn a_program_cannot_reach_the_first_megabyte() {
    let (code, text) = run(
        "supervisor",
        program(BASE, false, &[], &[0], |a, _| {
            a.mov(eax, dword_ptr(0x46Cu64))?;
            a.ret()
        }),
    );
    assert_eq!(code, 0xFF);
    assert!(
        text.contains("page fault (exception 0Eh, error 5, address 0000046C)"),
        "{}",
        text
    );
}

/// Writes its command line and the results of GetVersion and
/// GetModuleHandleA(NULL) and ("kernel32") as hex, then returns.
#[test]
fn a_program_finds_its_command_line_version_and_modules() {
    let imports: &[(&str, &[&str])] = &[(
        "KERNEL32.DLL",
        &[
            "GetCommandLineA",
            "GetVersion",
            "GetModuleHandleA",
            "GetStdHandle",
            "WriteFile",
        ],
    )];
    let mut data = b"kernel32\0".to_vec();
    data.resize(0x40, 0);
    let (code, text) = run(
        "queries",
        program(BASE, false, imports, &data, |a, l| {
            let mut strlen = a.create_label();
            let mut done = a.create_label();
            a.call(l.import(0))?;
            a.mov(esi, eax)?;
            a.mov(ecx, esi)?;
            a.set_label(&mut strlen)?;
            a.cmp(byte_ptr(ecx), 0)?;
            a.je(done)?;
            a.inc(ecx)?;
            a.jmp(strlen)?;
            a.set_label(&mut done)?;
            a.sub(ecx, esi)?;
            a.mov(edi, ecx)?;
            a.push(STD_OUTPUT_HANDLE)?;
            a.call(l.import(3))?;
            a.push(0)?;
            a.push(l.at(0x30) as i32)?;
            a.push(edi)?;
            a.push(esi)?;
            a.push(eax)?;
            a.call(l.import(4))?;
            // The checks, a bit each in the exit code.
            a.xor(ebx, ebx)?;
            a.call(l.import(1))?;
            a.cmp(eax, 0xC000_0004u32 as i32)?;
            a.sete(bl)?;
            a.push(0)?;
            a.call(l.import(2))?;
            a.cmp(eax, BASE as i32)?;
            a.sete(al)?;
            a.shl(al, 1)?;
            a.or(bl, al)?;
            a.push(l.at(0) as i32)?;
            a.call(l.import(2))?;
            a.cmp(eax, 0xBFF7_0000u32 as i32)?;
            a.sete(al)?;
            a.shl(al, 2)?;
            a.or(bl, al)?;
            a.mov(eax, ebx)?;
            a.ret()
        }),
    );
    assert_eq!(code, 7);
    assert!(text.ends_with("T.EXE"), "{}", text);
}

/// Static TLS: the program's variable, as its template has it, through
/// fs:[2Ch] and the index the loader wrote.
#[test]
fn a_program_reads_its_static_tls() {
    // The directory at 0, the index at 0x20, the template at 0x30.
    let mut data = vec![0u8; 0x40];
    let at = |offset: u32| BASE + 0x1000 + offset;
    for (i, value) in [at(0x30), at(0x34), at(0x20), 0, 0, 0]
        .into_iter()
        .enumerate()
    {
        data[i * 4..i * 4 + 4].copy_from_slice(&value.to_le_bytes());
    }
    data[0x20..0x24].copy_from_slice(&0xFFFFu32.to_le_bytes());
    data[0x30..0x34].copy_from_slice(&0x2Au32.to_le_bytes());
    let mut b = Builder::exe(BASE);
    let data_rva = b.section(
        ".data",
        &data,
        pe::SCN_INITIALIZED_DATA | pe::SCN_READ | pe::SCN_WRITE,
    );
    assert_eq!(data_rva, 0x1000);
    let mut a = CodeAssembler::new(32).unwrap();
    a.mov(eax, dword_ptr(0x2Cu64).fs()).unwrap();
    a.mov(ecx, dword_ptr((BASE + 0x1020) as u64)).unwrap();
    a.mov(eax, dword_ptr(eax + ecx * 4)).unwrap();
    a.mov(eax, dword_ptr(eax)).unwrap();
    a.ret().unwrap();
    let code_rva = b.next_rva();
    let code = a.assemble((BASE + code_rva) as u64).unwrap();
    b.section(
        ".text",
        &code,
        pe::SCN_CODE | pe::SCN_EXECUTE | pe::SCN_READ,
    );
    b.directory(pe::DIR_TLS, data_rva, 24).entry(code_rva);
    let (code, _) = run("tls", b.build());
    assert_eq!(code, 0x2A);
}

/// Waits for the BIOS's tick count, which the timer interrupt advances
/// while the program runs, to pass a value, then ends.
#[test]
fn hardware_interrupts_reach_dos_while_a_program_runs_and_a_state_resumes_it() {
    let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", &["GetTickCount"])];
    let image = program(BASE, false, imports, &[0], |a, l| {
        let mut wait = a.create_label();
        a.call(l.import(0))?;
        a.lea(ebx, dword_ptr(eax + 300))?;
        a.set_label(&mut wait)?;
        a.call(l.import(0))?;
        a.cmp(eax, ebx)?;
        a.jb(wait)?;
        a.mov(eax, 0x2A)?;
        a.ret()
    });
    let mut cpu = machine("irq", image);
    let ticks = |cpu: &Cpu| cpu.bus.read_32(0x46C);
    let start = ticks(&cpu);
    while !cpu.bus.win32.active() {
        cpu.bus.start_batch(cpu.bus.clock.icount + 10_000);
        run_batch(&mut cpu, &mut NoHook, false);
    }
    for _ in 0..20 {
        cpu.bus.start_batch(cpu.bus.clock.icount + 10_000);
        run_batch(&mut cpu, &mut NoHook, false);
    }
    assert!(cpu.bus.win32.active() && cpu.pe());
    let state = rust_dos::savestate::machine::save(&cpu);
    assert!(run_to_exit(&mut cpu, 2000));
    assert_eq!(cpu.errorlevel, 0x2A);
    // 300 ms is about 5 ticks of 55 ms.
    assert!(ticks(&cpu) >= start + 4, "{} ticks", ticks(&cpu) - start);

    rust_dos::savestate::machine::load(&mut cpu, &state).unwrap();
    assert!(cpu.bus.win32.active());
    cpu.errorlevel = 0;
    assert!(run_to_exit(&mut cpu, 2000));
    assert_eq!(cpu.errorlevel, 0x2A);
    assert!(!cpu.bus.win32.active());
}

/// The process heap, a TLS slot, an environment variable and
/// GetProcAddress, a bit each in the exit code.
#[test]
fn a_program_uses_the_heap_tls_environment_and_exports() {
    let names: &[&str] = &[
        "GetProcessHeap",
        "HeapAlloc",
        "HeapReAlloc",
        "HeapSize",
        "HeapFree",
        "TlsAlloc",
        "TlsSetValue",
        "TlsGetValue",
        "SetEnvironmentVariableA",
        "GetEnvironmentVariableA",
        "GetModuleHandleA",
        "GetProcAddress",
        "GetTickCount",
    ];
    let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", names)];
    // Strings: the variable's name and value, the module and the function;
    // a buffer at 0x40.
    let mut data = b"RD_TEST\0value\0KERNEL32\0GetTickCount\0".to_vec();
    data.resize(0x80, 0);
    let (code, _) = run(
        "services",
        program(BASE, false, imports, &data, |a, l| {
            let f = |name: &str| l.import(names.iter().position(|n| *n == name).unwrap());
            a.xor(ebx, ebx)?;
            // 100 zeroed bytes, grown to 5000.
            a.call(f("GetProcessHeap"))?;
            a.mov(esi, eax)?;
            a.push(100)?;
            a.push(8)?;
            a.push(esi)?;
            a.call(f("HeapAlloc"))?;
            a.mov(edi, eax)?;
            a.cmp(dword_ptr(edi + 96), 0)?;
            a.sete(bl)?;
            a.push(5000)?;
            a.push(edi)?;
            a.push(0)?;
            a.push(esi)?;
            a.call(f("HeapReAlloc"))?;
            a.mov(edi, eax)?;
            a.push(edi)?;
            a.push(0)?;
            a.push(esi)?;
            a.call(f("HeapSize"))?;
            a.cmp(eax, 5000)?;
            a.sete(al)?;
            a.shl(al, 1)?;
            a.or(bl, al)?;
            a.push(edi)?;
            a.push(0)?;
            a.push(esi)?;
            a.call(f("HeapFree"))?;
            a.shl(al, 2)?;
            a.or(bl, al)?;
            // A TLS slot keeps its value.
            a.call(f("TlsAlloc"))?;
            a.mov(esi, eax)?;
            a.push(0x1234)?;
            a.push(esi)?;
            a.call(f("TlsSetValue"))?;
            a.push(esi)?;
            a.call(f("TlsGetValue"))?;
            a.cmp(eax, 0x1234)?;
            a.sete(al)?;
            a.shl(al, 3)?;
            a.or(bl, al)?;
            // A variable set reads back, five characters long.
            a.push(l.at(8) as i32)?;
            a.push(l.at(0) as i32)?;
            a.call(f("SetEnvironmentVariableA"))?;
            a.push(0x40)?;
            a.push(l.at(0x40) as i32)?;
            a.push(l.at(0) as i32)?;
            a.call(f("GetEnvironmentVariableA"))?;
            a.cmp(eax, 5)?;
            a.sete(al)?;
            a.shl(al, 4)?;
            a.or(bl, al)?;
            // GetProcAddress finds what the import table has.
            a.push(l.at(14) as i32)?;
            a.call(f("GetModuleHandleA"))?;
            a.push(l.at(23) as i32)?;
            a.push(eax)?;
            a.call(f("GetProcAddress"))?;
            a.cmp(eax, f("GetTickCount"))?;
            a.sete(al)?;
            a.shl(al, 5)?;
            a.or(bl, al)?;
            a.mov(eax, ebx)?;
            a.ret()
        }),
    );
    assert_eq!(code, 0x3F, "the checks that failed: {:06b}", !code & 0x3F);
}

/// Code for a module's function, which may use the module's layout.
type Function<'a> = (
    &'a str,
    &'a dyn Fn(&mut CodeAssembler, &Layout) -> Result<(), IcedError>,
);

/// A DLL named `dll` (or with None a program) at `base`, importing
/// `imports`, with `data`, the functions `functions`, each in a section
/// of its own, and the resources `resources` (type, name, language,
/// data). The first function is the entry point; a DLL exports the others
/// by name, from ordinal 1. It has relocations for the addresses in its
/// code and data that point into it, found by their values.
fn module_image(
    base: u32,
    dll: Option<&str>,
    imports: &[(&str, &[&str])],
    data: &[u8],
    functions: &[Function],
    resources: &[(u16, u16, u16, &[u8])],
) -> Vec<u8> {
    let mut b = match dll {
        Some(name) => Builder::dll(base, name),
        None => Builder::exe(base),
    };
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
        data: base + data_rva,
        iat,
    };
    let mut sections = vec![(data_rva, data.to_vec())];
    for (i, (name, f)) in functions.iter().enumerate() {
        let rva = b.next_rva();
        let mut a = CodeAssembler::new(32).unwrap();
        f(&mut a, &layout).unwrap();
        let code = a.assemble((base + rva) as u64).unwrap();
        b.section(
            &format!(".t{}", i),
            &code,
            pe::SCN_CODE | pe::SCN_EXECUTE | pe::SCN_READ,
        );
        if i == 0 {
            b.entry(rva);
        } else if dll.is_some() {
            b.export(Some(name), i as u16, rva);
        }
        sections.push((rva, code));
    }
    let end = b.next_rva();
    for (rva, bytes) in &sections {
        for i in 0..bytes.len().saturating_sub(3) {
            let value = u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
            if (base..base + end).contains(&value) {
                b.reloc(rva + i as u32);
            }
        }
    }
    for &(kind, name, language, data) in resources {
        b.resource(kind, name, language, data);
    }
    b.build()
}

/// Run T.EXE from a folder with `files` to its end; its exit code and
/// what it printed.
fn run_files(test: &str, files: &[(&str, Vec<u8>)]) -> (u8, String) {
    let mut cpu = Cpu::new(scratch(test, files));
    cpu.bus.win32.enabled = true;
    cpu.host_console = Some(HostConsole::default());
    cpu.load_shell();
    assert!(cpu.load_executable("T.EXE", None));
    assert!(
        run_to_exit(&mut cpu, 400),
        "the program didn't end: {:?}",
        printed(&cpu)
    );
    assert!(!cpu.bus.win32.active());
    (cpu.errorlevel, printed(&cpu))
}

/// The test programs' data, when they import functions: after the import
/// section. The DLLs count their entry points' calls there, by reason:
/// B.DLL's at 100h, A.DLL's at 110h, and at 120h what B.DLL's count of
/// DLL_PROCESS_ATTACH was when A.DLL's ran.
const PROGRAM_DATA: u32 = BASE + 0x2000;
const B_COUNTS: u32 = PROGRAM_DATA + 0x100;
const A_COUNTS: u32 = PROGRAM_DATA + 0x110;
const B_FIRST: u32 = PROGRAM_DATA + 0x120;
/// Both DLLs want to be at 5A000000h, so the second loaded is relocated.
/// (Bytes of the base's address in code are told from others by it.)
const DLL_BASE: u32 = 0x5A00_0000;

/// B.DLL: counts its entry point's calls in the program's data, and says
/// "B detached" when it is; exports Seven, which returns 7.
fn dll_b() -> Vec<u8> {
    let mut data = b"B detached\r\n".to_vec();
    data.resize(0x40, 0);
    let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", &["GetStdHandle", "WriteFile"])];
    let main = |a: &mut CodeAssembler, l: &Layout| {
        let mut done = a.create_label();
        a.mov(eax, dword_ptr(esp + 8))?;
        a.inc(dword_ptr(eax * 4 + B_COUNTS as i32))?;
        a.test(eax, eax)?;
        a.jne(done)?;
        a.push(STD_OUTPUT_HANDLE)?;
        a.call(l.import(0))?;
        a.push(0)?;
        a.push(l.at(0x20) as i32)?;
        a.push(12)?;
        a.push(l.at(0) as i32)?;
        a.push(eax)?;
        a.call(l.import(1))?;
        a.set_label(&mut done)?;
        a.mov(eax, 1)?;
        a.ret_1(12)
    };
    let seven = |a: &mut CodeAssembler, _: &Layout| {
        a.mov(eax, 7)?;
        a.ret()
    };
    module_image(
        DLL_BASE,
        Some("B.DLL"),
        imports,
        &data,
        &[("", &main), ("Seven", &seven)],
        &[],
    )
}

/// A.DLL: imports B.DLL, counts its entry point's calls, and exports
/// Value, which returns B's Seven plus 35, read through a pointer that a
/// relocation moves with it.
fn dll_a() -> Vec<u8> {
    let mut data = vec![0u8; 0x10];
    data[0..4].copy_from_slice(&(DLL_BASE + 0x2004).to_le_bytes());
    data[4..8].copy_from_slice(&35u32.to_le_bytes());
    let imports: &[(&str, &[&str])] = &[("B.DLL", &["Seven"])];
    let main = |a: &mut CodeAssembler, l: &Layout| {
        assert_eq!(l.data, DLL_BASE + 0x2000);
        let mut done = a.create_label();
        a.mov(eax, dword_ptr(esp + 8))?;
        a.inc(dword_ptr(eax * 4 + A_COUNTS as i32))?;
        a.cmp(eax, 1)?;
        a.jne(done)?;
        a.mov(eax, dword_ptr((B_COUNTS + 4) as u64))?;
        a.mov(dword_ptr(B_FIRST as u64), eax)?;
        a.set_label(&mut done)?;
        a.mov(eax, 1)?;
        a.ret_1(12)
    };
    let value = |a: &mut CodeAssembler, l: &Layout| {
        a.call(l.import(0))?;
        a.mov(ecx, dword_ptr(l.at(0)))?;
        a.add(eax, dword_ptr(ecx))?;
        a.ret()
    };
    module_image(
        DLL_BASE,
        Some("A.DLL"),
        imports,
        &data,
        &[("", &main), ("Value", &value)],
        &[],
    )
}

/// Set bit `bit` of EBX when ZF is set.
fn check(a: &mut CodeAssembler, bit: u32) -> Result<(), IcedError> {
    a.sete(al)?;
    a.movzx(eax, al)?;
    a.shl(eax, bit)?;
    a.or(ebx, eax)
}

/// A DLL the program imports, which imports another, both with entry
/// points, one relocated: the entry points run before the program's,
/// those a DLL imports from first, and learn of the process's end.
#[test]
fn a_program_imports_dlls_from_disk() {
    let imports: &[(&str, &[&str])] = &[("A.DLL", &["Value"])];
    let main = |a: &mut CodeAssembler, l: &Layout| {
        assert_eq!(l.data, PROGRAM_DATA);
        a.xor(ebx, ebx)?;
        a.call(l.import(0))?;
        a.cmp(eax, 42)?;
        check(a, 0)?;
        a.cmp(dword_ptr((A_COUNTS + 4) as u64), 1)?;
        check(a, 1)?;
        a.cmp(dword_ptr((B_COUNTS + 4) as u64), 1)?;
        check(a, 2)?;
        a.cmp(dword_ptr(B_FIRST as u64), 1)?;
        check(a, 3)?;
        a.mov(eax, ebx)?;
        a.ret()
    };
    let exe = module_image(BASE, None, imports, &[0; 0x200], &[("", &main)], &[]);
    let (code, text) = run_files(
        "dll_static",
        &[("T.EXE", exe), ("A.DLL", dll_a()), ("B.DLL", dll_b())],
    );
    assert_eq!(
        code,
        0x0F,
        "the checks that failed: {:04b} {}",
        !code & 0x0F,
        text
    );
    assert_eq!(text, "B detached\r\n");
}

/// LoadLibraryA of a DLL that imports another, GetProcAddress, threads
/// that the DLLs' entry points learn of (but not B.DLL's, after
/// DisableThreadLibraryCalls), and FreeLibrary, which frees both.
#[test]
fn a_program_loads_and_frees_dlls() {
    let names: &[&str] = &[
        "LoadLibraryA",
        "GetProcAddress",
        "FreeLibrary",
        "GetModuleHandleA",
        "GetLastError",
        "CreateThread",
        "WaitForSingleObject",
        "DisableThreadLibraryCalls",
    ];
    let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", names)];
    let mut data = b"NOPE.DLL\0A.DLL\0Value\0B\0".to_vec();
    data.resize(0x200, 0);
    let main = |a: &mut CodeAssembler, l: &Layout| {
        assert_eq!(l.data, PROGRAM_DATA);
        let f = |name: &str| l.import(names.iter().position(|n| *n == name).unwrap());
        let mut after = a.create_label();
        a.xor(ebx, ebx)?;
        // A DLL that isn't there: Windows 95's ERROR_DLL_NOT_FOUND.
        a.push(l.at(0) as i32)?;
        a.call(f("LoadLibraryA"))?;
        a.mov(esi, eax)?;
        a.call(f("GetLastError"))?;
        // (The machine is a 486, without CMOV.)
        let mut wrong = a.create_label();
        a.cmp(eax, 1157)?;
        a.jne(wrong)?;
        a.test(esi, esi)?;
        a.set_label(&mut wrong)?;
        check(a, 0)?;
        // A.DLL, and B.DLL with it.
        a.push(l.at(9) as i32)?;
        a.call(f("LoadLibraryA"))?;
        a.mov(esi, eax)?;
        a.push(l.at(15) as i32)?;
        a.push(esi)?;
        a.call(f("GetProcAddress"))?;
        a.call(eax)?;
        a.cmp(eax, 42)?;
        check(a, 1)?;
        a.mov(eax, dword_ptr((A_COUNTS + 4) as u64))?;
        a.add(eax, dword_ptr((B_COUNTS + 4) as u64))?;
        a.add(eax, dword_ptr(B_FIRST as u64))?;
        a.cmp(eax, 3)?;
        check(a, 2)?;
        // A thread.
        a.push(l.at(21) as i32)?;
        a.call(f("GetModuleHandleA"))?;
        a.push(eax)?;
        a.call(f("DisableThreadLibraryCalls"))?;
        // The thread's function, whose address the call leaves.
        a.call(after)?;
        a.xor(eax, eax)?;
        a.ret_1(4)?;
        a.set_label(&mut after)?;
        a.pop(eax)?;
        a.push(0)?;
        a.push(0)?;
        a.push(0)?;
        a.push(eax)?;
        a.push(0)?;
        a.push(0)?;
        a.call(f("CreateThread"))?;
        a.push(-1)?;
        a.push(eax)?;
        a.call(f("WaitForSingleObject"))?;
        a.mov(eax, dword_ptr((A_COUNTS + 8) as u64))?;
        a.add(eax, dword_ptr((A_COUNTS + 12) as u64))?;
        a.add(eax, dword_ptr((B_COUNTS + 8) as u64))?;
        a.add(eax, dword_ptr((B_COUNTS + 12) as u64))?;
        a.cmp(eax, 2)?;
        check(a, 3)?;
        // Freed, both.
        a.push(esi)?;
        a.call(f("FreeLibrary"))?;
        a.add(eax, dword_ptr(A_COUNTS as u64))?;
        a.add(eax, dword_ptr(B_COUNTS as u64))?;
        a.cmp(eax, 3)?;
        check(a, 4)?;
        a.push(l.at(21) as i32)?;
        a.call(f("GetModuleHandleA"))?;
        a.test(eax, eax)?;
        check(a, 5)?;
        a.mov(eax, ebx)?;
        a.ret()
    };
    let exe = module_image(BASE, None, imports, &data, &[("", &main)], &[]);
    let (code, text) = run_files(
        "dll_dynamic",
        &[("T.EXE", exe), ("A.DLL", dll_a()), ("B.DLL", dll_b())],
    );
    assert_eq!(code, 0x3F, "the checks that failed: {:06b}", !code & 0x3F);
    assert_eq!(text, "B detached\r\n");
}

/// C.DLL, whose entry point returns FALSE, and exports F.
fn dll_c() -> Vec<u8> {
    let main = |a: &mut CodeAssembler, _: &Layout| {
        a.xor(eax, eax)?;
        a.ret_1(12)
    };
    let f = |a: &mut CodeAssembler, _: &Layout| a.ret();
    module_image(
        DLL_BASE,
        Some("C.DLL"),
        &[],
        &[0; 4],
        &[("", &main), ("F", &f)],
        &[],
    )
}

#[test]
fn a_dll_whose_entry_point_fails_is_not_loaded() {
    let names: &[&str] = &["LoadLibraryA", "GetLastError", "GetModuleHandleA"];
    let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", names)];
    let main = |a: &mut CodeAssembler, l: &Layout| {
        a.xor(ebx, ebx)?;
        a.push(l.at(0) as i32)?;
        a.call(l.import(0))?;
        a.test(eax, eax)?;
        check(a, 0)?;
        a.call(l.import(1))?;
        a.cmp(eax, 1114)?;
        check(a, 1)?;
        a.push(l.at(0) as i32)?;
        a.call(l.import(2))?;
        a.test(eax, eax)?;
        check(a, 2)?;
        a.mov(eax, ebx)?;
        a.ret()
    };
    let exe = module_image(BASE, None, imports, b"C.DLL\0", &[("", &main)], &[]);
    let (code, _) = run_files("dll_fails", &[("T.EXE", exe), ("C.DLL", dll_c())]);
    assert_eq!(code, 7, "the checks that failed: {:03b}", !code & 7);

    // One the program imports ends it before it starts.
    let imports: &[(&str, &[&str])] = &[("C.DLL", &["F"])];
    let main = |a: &mut CodeAssembler, _: &Layout| {
        a.mov(eax, 0x2A)?;
        a.ret()
    };
    let exe = module_image(BASE, None, imports, &[0; 4], &[("", &main)], &[]);
    let (code, text) = run_files("dll_fails_static", &[("T.EXE", exe), ("C.DLL", dll_c())]);
    assert_eq!(code, 0xFF);
    assert!(
        text.contains("Initialization of the dynamic link library C:\\C.DLL failed"),
        "{}",
        text
    );
}

#[test]
fn a_program_missing_a_dll_function_is_refused() {
    let imports: &[(&str, &[&str])] = &[("C.DLL", &["G"])];
    let main = |a: &mut CodeAssembler, _: &Layout| a.ret();
    let exe = module_image(BASE, None, imports, &[0; 4], &[("", &main)], &[]);
    let (code, text) = run_files("dll_missing", &[("T.EXE", exe), ("C.DLL", dll_c())]);
    assert_eq!(code, 1);
    assert!(
        text.contains("T.EXE is linked to missing export C.DLL:G"),
        "{}",
        text
    );
}

/// A resource of the program found, loaded and measured, one that isn't
/// there, the types enumerated, and the program's file name.
#[test]
fn a_program_reads_its_resources_and_file_name() {
    let names: &[&str] = &[
        "FindResourceA",
        "LoadResource",
        "LockResource",
        "SizeofResource",
        "GetLastError",
        "GetModuleFileNameA",
        "EnumResourceTypesA",
    ];
    let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", names)];
    const RT_RCDATA: u16 = 10;
    let main = |a: &mut CodeAssembler, l: &Layout| {
        let f = |name: &str| l.import(names.iter().position(|n| *n == name).unwrap());
        let mut after = a.create_label();
        a.xor(ebx, ebx)?;
        a.push(RT_RCDATA as i32)?;
        a.push(5)?;
        a.push(0)?;
        a.call(f("FindResourceA"))?;
        a.mov(esi, eax)?;
        a.push(esi)?;
        a.push(0)?;
        a.call(f("SizeofResource"))?;
        a.cmp(eax, 5)?;
        check(a, 0)?;
        a.push(esi)?;
        a.push(0)?;
        a.call(f("LoadResource"))?;
        a.push(eax)?;
        a.call(f("LockResource"))?;
        a.cmp(dword_ptr(eax), 0x6C6C6568)?;
        check(a, 1)?;
        a.push(RT_RCDATA as i32)?;
        a.push(6)?;
        a.push(0)?;
        a.call(f("FindResourceA"))?;
        a.mov(esi, eax)?;
        a.call(f("GetLastError"))?;
        // (The machine is a 486, without CMOV.)
        let mut wrong = a.create_label();
        a.cmp(eax, 1814)?;
        a.jne(wrong)?;
        a.test(esi, esi)?;
        a.set_label(&mut wrong)?;
        check(a, 2)?;
        // "C:\T.EXE", and cut to fit 4 bytes with its NUL.
        a.push(0x100)?;
        a.push(l.at(0) as i32)?;
        a.push(0)?;
        a.call(f("GetModuleFileNameA"))?;
        a.cmp(eax, 8)?;
        check(a, 3)?;
        a.push(4)?;
        a.push(l.at(0x100) as i32)?;
        a.push(0)?;
        a.call(f("GetModuleFileNameA"))?;
        a.cmp(eax, 3)?;
        check(a, 4)?;
        // The callback, whose address the call leaves: it keeps the type.
        a.call(after)?;
        a.mov(eax, dword_ptr(esp + 8))?;
        a.mov(dword_ptr(l.at(0x180)), eax)?;
        a.mov(eax, 1)?;
        a.ret_1(12)?;
        a.set_label(&mut after)?;
        a.pop(eax)?;
        a.push(0)?;
        a.push(eax)?;
        a.push(0)?;
        a.call(f("EnumResourceTypesA"))?;
        a.cmp(dword_ptr(l.at(0x180)), RT_RCDATA as i32)?;
        check(a, 5)?;
        a.mov(eax, ebx)?;
        a.ret()
    };
    let exe = module_image(
        BASE,
        None,
        imports,
        &[0; 0x200],
        &[("", &main)],
        &[(RT_RCDATA, 5, 0x409, b"hello")],
    );
    let (code, _) = run_files("resources", &[("T.EXE", exe)]);
    assert_eq!(code, 0x3F, "the checks that failed: {:06b}", !code & 0x3F);
}

/// LoadLibraryA of files Windows 95 can't load: an empty one is in a bad
/// format, and one whose PE header has no optional header gives
/// ERROR_GEN_FAILURE, as Wine's loader test found on Windows 9x.
#[test]
fn a_dll_that_is_not_an_image_is_refused() {
    let mut bad = vec![0u8; 0x40];
    bad[..2].copy_from_slice(b"MZ");
    bad[0x3C] = 0x40;
    bad.extend_from_slice(b"PE\0\0");
    let mut header = [0u8; 20];
    header[..2].copy_from_slice(&0x14Cu16.to_le_bytes());
    header[2] = 1;
    header[18..].copy_from_slice(&0x2002u16.to_le_bytes());
    bad.extend_from_slice(&header);
    let mut section = [0u8; 40];
    section[..7].copy_from_slice(b".rodata");
    bad.extend_from_slice(&section);
    bad.extend_from_slice(b"section data\0\0\0\0");
    let names: &[&str] = &["LoadLibraryA", "GetLastError"];
    let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", names)];
    let main = |a: &mut CodeAssembler, l: &Layout| {
        a.xor(ebx, ebx)?;
        for (bit, (at, error)) in [(0u32, 11u32), (10, 31)].into_iter().enumerate() {
            let mut wrong = a.create_label();
            a.push(l.at(at) as i32)?;
            a.call(l.import(0))?;
            a.mov(esi, eax)?;
            a.call(l.import(1))?;
            a.cmp(eax, error as i32)?;
            a.jne(wrong)?;
            a.test(esi, esi)?;
            a.set_label(&mut wrong)?;
            check(a, bit as u32)?;
        }
        a.mov(eax, ebx)?;
        a.ret()
    };
    let exe = module_image(
        BASE,
        None,
        imports,
        b"EMPTY.DLL\0BAD.DLL\0",
        &[("", &main)],
        &[],
    );
    let (code, _) = run_files(
        "dll_bad",
        &[("T.EXE", exe), ("EMPTY.DLL", Vec::new()), ("BAD.DLL", bad)],
    );
    assert_eq!(code, 3, "the checks that failed: {:02b}", !code & 3);
}
