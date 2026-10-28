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
    program_for(pe::SUBSYSTEM_CONSOLE, base, relocatable, imports, data, f)
}

/// `program`, for the subsystem `subsystem`: a console or a GUI one.
fn program_for(
    subsystem: u16,
    base: u32,
    relocatable: bool,
    imports: &[(&str, &[&str])],
    data: &[u8],
    f: impl FnOnce(&mut CodeAssembler, &Layout) -> Result<(), IcedError>,
) -> Vec<u8> {
    let mut b = Builder::exe(base);
    b.subsystem(subsystem);
    program_with(b, relocatable, imports, data, f)
}

/// `program`, made with `b`, a builder whose headers are set up already.
fn program_with(
    mut b: Builder,
    relocatable: bool,
    imports: &[(&str, &[&str])],
    data: &[u8],
    f: impl FnOnce(&mut CodeAssembler, &Layout) -> Result<(), IcedError>,
) -> Vec<u8> {
    let base = b.image_base();
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
    machine_with(test, &[("T.EXE", program)])
}

/// A machine with the built-in Windows on, T.EXE of `files` loaded from
/// the shell, and what it prints kept.
fn machine_with(test: &str, files: &[(&str, Vec<u8>)]) -> Cpu {
    let mut cpu = Cpu::new(scratch(test, files));
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
    let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", &["GetVersion", "VirtualAllocEx"])];
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
        text.contains("T.EXE needs KERNEL32.DLL's function VirtualAllocEx"),
        "{}",
        text
    );
}

#[test]
fn a_program_importing_an_unknown_dll_is_refused() {
    let imports: &[(&str, &[&str])] = &[
        ("KERNEL32.DLL", &["ExitProcess"]),
        ("NOSUCH.DLL", &["Function"]),
    ];
    let (code, text) = run(
        "unknown_dll",
        program(BASE, false, imports, &[0], |a, _| a.ret()),
    );
    assert_eq!(code, 1);
    assert!(text.contains("T.EXE needs NOSUCH.DLL"), "{}", text);
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
    let elapsed = ticks(&cpu).wrapping_sub(start);
    assert!(elapsed >= 4, "{} ticks", elapsed);

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

/// Run T.EXE, which starts the program the command line `line` names
/// with CreateProcessA, waits for it and ends with what it learned, from
/// a folder with the folders `dirs` and C.EXE at `child`. C.EXE ends with
/// 7, or 8 when its command line starts with a quote, so 71h is the
/// program started, waited for and its exit code read.
fn start_child(test: &str, line: &str, child: &str, dirs: &[&str]) -> u8 {
    let child_program = {
        let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", &["GetCommandLineA", "ExitProcess"])];
        program(BASE, false, imports, &[0; 4], |a, l| {
            a.call(l.import(0))?;
            a.cmp(byte_ptr(eax), b'"' as i32)?;
            a.sete(al)?;
            a.movzx(eax, al)?;
            a.add(eax, 7)?;
            a.push(eax)?;
            a.call(l.import(1))?;
            a.int3()
        })
    };
    let names: &[&str] = &[
        "CreateProcessA",
        "WaitForSingleObject",
        "GetExitCodeProcess",
        "CloseHandle",
        "ExitProcess",
    ];
    let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", names)];
    // The command line, STARTUPINFO at 0x40, PROCESS_INFORMATION at 0x90,
    // the exit code at 0xA0.
    let mut data = line.as_bytes().to_vec();
    assert!(data.len() < 0x40);
    data.resize(0xB0, 0);
    data[0x40] = 68;
    let parent = program(BASE, false, imports, &data, |a, l| {
        let f = |name: &str| l.import(names.iter().position(|n| *n == name).unwrap());
        a.push(l.at(0x90) as i32)?;
        a.push(l.at(0x40) as i32)?;
        for _ in 0..6 {
            a.push(0)?;
        }
        a.push(l.at(0) as i32)?;
        a.push(0)?;
        a.call(f("CreateProcessA"))?;
        a.mov(ebx, eax)?;
        a.push(-1)?;
        a.push(dword_ptr(l.at(0x90)))?;
        a.call(f("WaitForSingleObject"))?;
        a.shl(eax, 1)?;
        a.or(ebx, eax)?;
        a.push(l.at(0xA0) as i32)?;
        a.push(dword_ptr(l.at(0x90)))?;
        a.call(f("GetExitCodeProcess"))?;
        a.push(dword_ptr(l.at(0x90)))?;
        a.call(f("CloseHandle"))?;
        a.push(dword_ptr(l.at(0x94)))?;
        a.call(f("CloseHandle"))?;
        // 1 for CreateProcessA, the wait's result above it, and the
        // child's exit code in the high nibble.
        a.mov(eax, dword_ptr(l.at(0xA0)))?;
        a.shl(eax, 4)?;
        a.or(eax, ebx)?;
        a.push(eax)?;
        a.call(f("ExitProcess"))?;
        a.int3()
    });
    let base = scratch(test, &[("T.EXE", parent)]);
    for dir in dirs {
        fs::create_dir_all(base.join(dir)).unwrap();
    }
    let child = base.join(child);
    fs::create_dir_all(child.parent().unwrap()).unwrap();
    fs::write(child, child_program).unwrap();
    let mut cpu = Cpu::new(base);
    cpu.bus.win32.enabled = true;
    cpu.host_console = Some(HostConsole::default());
    cpu.load_shell();
    assert!(cpu.load_executable("T.EXE", None));
    assert!(run_to_exit(&mut cpu, 200), "{}", printed(&cpu));
    assert!(!cpu.bus.win32.active());
    cpu.errorlevel
}

#[test]
fn a_program_starts_another_and_waits_for_its_exit_code() {
    let code = start_child("child_process", "C.EXE an argument", "C.EXE", &[]);
    assert_eq!(code, 0x71);
}

#[test]
fn win_exec_starts_a_program_or_says_why_not() {
    let child = {
        let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", &["ExitProcess"])];
        program(BASE, false, imports, &[0; 4], |a, l| {
            a.push(0)?;
            a.call(l.import(0))?;
            a.int3()
        })
    };
    let names: &[&str] = &["WinExec", "Sleep", "ExitProcess"];
    let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", names)];
    let mut data = b"C.EXE an argument ".to_vec();
    data.resize(0x20, 0);
    data.extend_from_slice(b"NONE.EXE ");
    data.resize(0x40, 0);
    let parent = program(BASE, false, imports, &data, |a, l| {
        let f = |name: &str| l.import(names.iter().position(|n| *n == name).unwrap());
        // 33 for the program started, 2 for the one not found.
        a.push(1)?;
        a.push(l.at(0) as i32)?;
        a.call(f("WinExec"))?;
        a.cmp(eax, 33)?;
        a.sete(bl)?;
        a.push(1)?;
        a.push(l.at(0x20) as i32)?;
        a.call(f("WinExec"))?;
        a.cmp(eax, 2)?;
        a.sete(al)?;
        a.shl(al, 1)?;
        a.or(bl, al)?;
        // The child ends before the parent.
        a.push(100)?;
        a.call(f("Sleep"))?;
        a.movzx(eax, bl)?;
        a.push(eax)?;
        a.call(f("ExitProcess"))?;
        a.int3()
    });
    let mut cpu = Cpu::new(scratch("win_exec", &[("T.EXE", parent), ("C.EXE", child)]));
    cpu.bus.win32.enabled = true;
    cpu.host_console = Some(HostConsole::default());
    cpu.load_shell();
    assert!(cpu.load_executable("T.EXE", None));
    assert!(run_to_exit(&mut cpu, 200), "{}", printed(&cpu));
    assert_eq!(cpu.errorlevel, 3, "{}", printed(&cpu));
}

/// A command line naming its program without quotes, in a folder with
/// spaces in its name: the program is found by the line up to each space
/// in turn ("V1.0", "V1.0 GAME\C.EXE"), as Windows finds it, and starts.
/// The folder V1.0 isn't a program, so the search goes on past it. The
/// program gets its name in quotes, as on Windows 98, so 81h, also when
/// blanks come before it.
#[test]
fn an_unquoted_program_path_with_spaces_is_found() {
    let line = "V1.0 GAME\\C.EXE an argument";
    let code = start_child("unquoted_path", line, "V1.0 GAME/C.EXE", &["V1.0"]);
    assert_eq!(code, 0x81);
    let line = " \tV1.0 GAME\\C.EXE an argument";
    let code = start_child("unquoted_path_blanks", line, "V1.0 GAME/C.EXE", &["V1.0"]);
    assert_eq!(code, 0x81);
}

#[test]
fn threads_take_turns_wait_for_each_other_and_share_a_critical_section() {
    let names: &[&str] = &[
        "CreateThread",
        "WaitForSingleObject",
        "GetExitCodeThread",
        "InitializeCriticalSection",
        "EnterCriticalSection",
        "LeaveCriticalSection",
        "Sleep",
        "ExitProcess",
    ];
    let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", names)];
    // The critical section at 0, a counter at 0x20, the thread's exit
    // code at 0x24, a flag the thread waits for at 0x28.
    let data = vec![0; 0x40];
    let (code, out) = run(
        "threads",
        program(BASE, false, imports, &data, |a, l| {
            let f = |name: &str| l.import(names.iter().position(|n| *n == name).unwrap());
            let mut over = a.create_label();
            let mut spin = a.create_label();
            let mut add = a.create_label();
            a.push(l.at(0) as i32)?;
            a.call(f("InitializeCriticalSection"))?;
            // The thread, with 1000 as its argument.
            a.push(0)?;
            a.push(0)?;
            a.push(1000)?;
            // Its address, which the CALL over it pushes.
            a.call(over)?;
            // The thread: count its argument's times, wait for the flag
            // with Sleep(0), return 5.
            a.push(ebx)?;
            a.mov(ebx, dword_ptr(esp + 8))?;
            let mut again = a.create_label();
            a.set_label(&mut again)?;
            a.push(l.at(0) as i32)?;
            a.call(f("EnterCriticalSection"))?;
            a.add(dword_ptr(l.at(0x20)), 1)?;
            a.push(l.at(0) as i32)?;
            a.call(f("LeaveCriticalSection"))?;
            a.dec(ebx)?;
            a.jnz(again)?;
            a.set_label(&mut spin)?;
            a.push(0)?;
            a.call(f("Sleep"))?;
            a.cmp(dword_ptr(l.at(0x28)), 0)?;
            a.je(spin)?;
            a.pop(ebx)?;
            a.mov(eax, 5)?;
            a.ret_1(4)?;
            a.set_label(&mut over)?;
            a.push(0)?;
            a.push(0)?;
            a.call(f("CreateThread"))?;
            a.mov(esi, eax)?;
            // The main thread adds 1000 too, in the section.
            a.mov(edi, 1000)?;
            a.set_label(&mut add)?;
            a.push(l.at(0) as i32)?;
            a.call(f("EnterCriticalSection"))?;
            a.add(dword_ptr(l.at(0x20)), 1)?;
            a.push(l.at(0) as i32)?;
            a.call(f("LeaveCriticalSection"))?;
            a.dec(edi)?;
            a.jnz(add)?;
            // Let the thread go on, which spins until then.
            a.mov(dword_ptr(l.at(0x28)), 1)?;
            a.push(-1)?;
            a.push(esi)?;
            a.call(f("WaitForSingleObject"))?;
            a.mov(ebx, eax)?;
            a.push(l.at(0x24) as i32)?;
            a.push(esi)?;
            a.call(f("GetExitCodeThread"))?;
            // 5: the count less 2000, plus the thread's exit code and
            // the wait's WAIT_OBJECT_0.
            a.mov(eax, dword_ptr(l.at(0x20)))?;
            a.sub(eax, 2000)?;
            a.add(eax, dword_ptr(l.at(0x24)))?;
            a.add(eax, ebx)?;
            a.push(eax)?;
            a.call(f("ExitProcess"))?;
            a.int3()
        }),
    );
    assert_eq!(code, 5, "{}", out);
}

/// Threads started and ended one after another, more than the LDT has
/// selectors for and more stacks than memory holds at once: each takes
/// the place of one that ended. The exit code is the last one's.
#[test]
fn ended_threads_make_room_for_new_ones() {
    let names: &[&str] = &[
        "CreateThread",
        "WaitForSingleObject",
        "GetExitCodeThread",
        "CloseHandle",
        "ExitProcess",
    ];
    let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", names)];
    let (code, out) = run(
        "thread_places",
        program(BASE, false, imports, &[0; 0x10], |a, l| {
            let f = |name: &str| l.import(names.iter().position(|n| *n == name).unwrap());
            let mut over = a.create_label();
            let mut again = a.create_label();
            let mut failed = a.create_label();
            // The thread's address in EBP, which the CALL over it pushes.
            a.call(over)?;
            // The thread returns its argument.
            a.mov(eax, dword_ptr(esp + 4))?;
            a.ret_1(4)?;
            a.set_label(&mut over)?;
            a.pop(ebp)?;
            a.mov(edi, 9000)?;
            a.set_label(&mut again)?;
            a.push(0)?;
            a.push(0)?;
            a.push(edi)?;
            a.push(ebp)?;
            a.push(0)?;
            a.push(0)?;
            a.call(f("CreateThread"))?;
            a.test(eax, eax)?;
            a.jz(failed)?;
            a.mov(esi, eax)?;
            a.push(-1)?;
            a.push(esi)?;
            a.call(f("WaitForSingleObject"))?;
            a.push(l.at(0) as i32)?;
            a.push(esi)?;
            a.call(f("GetExitCodeThread"))?;
            a.push(esi)?;
            a.call(f("CloseHandle"))?;
            a.dec(edi)?;
            a.jnz(again)?;
            // The last thread's argument and exit code was 1.
            a.push(dword_ptr(l.at(0)))?;
            a.call(f("ExitProcess"))?;
            a.set_label(&mut failed)?;
            a.push(99)?;
            a.call(f("ExitProcess"))?;
            a.int3()
        }),
    );
    assert_eq!(code, 1, "{}", out);
}

#[test]
fn a_handler_on_the_exception_chain_resumes_after_a_divide_error() {
    let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", &["ExitProcess"])];
    let (code, out) = run(
        "seh",
        program(BASE, false, imports, &[0; 4], |a, l| {
            let mut over = a.create_label();
            // The handler's address, which the CALL over it pushes.
            a.call(over)?;
            // The handler: the faulting DIV is two bytes long; go on
            // after it with EAX 42 (CONTEXT's Eip at B8h, Eax at B0h).
            a.mov(eax, dword_ptr(esp + 12))?;
            a.add(dword_ptr(eax + 0xB8), 2)?;
            a.mov(dword_ptr(eax + 0xB0), 42)?;
            a.xor(eax, eax)?;
            a.ret()?;
            a.set_label(&mut over)?;
            // A registration on the stack: the next one, then the
            // handler, which is on the stack already.
            a.push(dword_ptr(0).fs())?;
            a.mov(dword_ptr(0).fs(), esp)?;
            a.xor(edx, edx)?;
            a.xor(ecx, ecx)?;
            a.mov(eax, 1)?;
            a.div(ecx)?;
            a.pop(dword_ptr(0).fs())?;
            a.add(esp, 4)?;
            a.push(eax)?;
            a.call(l.import(0))?;
            a.int3()
        }),
    );
    assert_eq!(code, 42, "{}", out);
}

/// What a debugger sees of a program whose main thread spins while a
/// second thread waits for an event and a third was made suspended.
#[test]
fn a_snapshot_shows_the_processes_threads_and_handles() {
    use rust_dos::win32::snapshot::{Field, Registers, ThreadState};
    let names: &[&str] = &[
        "CreateEventA",
        "CreateThread",
        "WaitForSingleObject",
        "SetLastError",
    ];
    let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", names)];
    // A flag nothing sets at 0, the event's handle at 0x10.
    let data = [0u8; 0x20];
    let image = program(BASE, false, imports, &data, |a, l| {
        let mut main = a.create_label();
        let mut spin = a.create_label();
        // The worker, whose address the call leaves on the stack.
        a.call(main)?;
        a.push(-1)?;
        a.push(dword_ptr(l.at(0x10)))?;
        a.call(l.import(2))?;
        a.ret_1(4)?;
        a.set_label(&mut main)?;
        a.pop(esi)?;
        a.push(0)?;
        a.push(0)?;
        a.push(1)?;
        a.push(0)?;
        a.call(l.import(0))?;
        a.mov(dword_ptr(l.at(0x10)), eax)?;
        // One thread that runs, one suspended (CREATE_SUSPENDED).
        for flags in [0, 4] {
            a.push(0)?;
            a.push(flags)?;
            a.push(0)?;
            a.push(esi)?;
            a.push(0)?;
            a.push(0)?;
            a.call(l.import(1))?;
        }
        a.push(0x57)?;
        a.call(l.import(3))?;
        a.set_label(&mut spin)?;
        a.cmp(dword_ptr(l.at(0)), 0)?;
        a.je(spin)?;
        a.ret()
    });
    let mut cpu = machine("snapshot", image);
    let waiting = |cpu: &Cpu| {
        let s = rust_dos::win32::snapshot(cpu);
        s.processes.first().is_some_and(|p| {
            p.threads.len() == 3
                && p.threads[1].state == ThreadState::Waiting
                && p.threads[0].state == ThreadState::Running
        })
    };
    let mut batches = 0;
    while !waiting(&cpu) {
        batches += 1;
        assert!(batches < 2000, "the second thread didn't wait");
        cpu.bus.start_batch(cpu.bus.clock.icount + 10_000);
        run_batch(&mut cpu, &mut NoHook, false);
    }
    let s = rust_dos::win32::snapshot(&cpu);
    assert!(s.enabled && s.active && s.pending.is_none());
    assert_eq!(s.processes.len(), 1);
    let p = &s.processes[0];
    assert_eq!(p.name, "T.EXE");
    assert!(p.path.ends_with("T.EXE"), "{}", p.path);
    assert!(
        p.command_line
            .as_deref()
            .is_some_and(|c| c.contains("T.EXE")),
        "{:?}",
        p.command_line
    );
    assert_eq!(p.image_base, BASE);
    assert!(p.running && p.root && p.exit_code.is_none());

    let module = |name: &str| p.modules.iter().find(|m| m.name == name).unwrap();
    assert!(!module("T.EXE").builtin);
    assert_eq!(module("T.EXE").base, BASE);
    assert!(module("KERNEL32.DLL").builtin);

    let (main, worker, suspended) = (&p.threads[0], &p.threads[1], &p.threads[2]);
    assert!(matches!(
        main.registers,
        Registers::Cpu | Registers::Interrupted
    ));
    assert!(
        (BASE..BASE + 0x10000).contains(&main.eip),
        "{:08X}",
        main.eip
    );
    assert_eq!(main.last_error, 0x57);
    assert_ne!(main.tib, worker.tib);
    assert_eq!(worker.registers, Registers::Saved);
    let wait = worker.wait.as_ref().unwrap();
    assert_eq!(wait.timeout_ms, None);
    assert_eq!(wait.objects.len(), 1);
    let (handle, event) = &wait.objects[0];
    assert_eq!(event.kind, "event");
    let event_handle = handle.unwrap();
    assert_eq!(suspended.state, ThreadState::Suspended);
    assert_eq!(suspended.suspended, 1);
    assert!(suspended.wait.is_none());

    let kinds: Vec<(&str, u32)> = p
        .handles
        .iter()
        .map(|h| (h.object.kind, h.handle))
        .collect();
    assert_eq!(kinds.iter().filter(|(k, _)| *k == "dos").count(), 3);
    assert_eq!(kinds.iter().filter(|(k, _)| *k == "thread").count(), 2);
    let event = p.handles.iter().find(|h| h.handle == event_handle).unwrap();
    assert_eq!(event.object.kind, "event");
    let state = &event.object.state;
    assert!(state.contains(&("manual", Field::Bool(true))));
    assert!(state.contains(&("signaled", Field::Bool(false))));
    // The handle, and the wait's hold.
    assert_eq!(event.object.refs, 2);
}

/// The registry: a key made and a value set under HKEY_CURRENT_USER,
/// which the file on drive C: gets when the key closes; Windows 95's
/// version in the keys every registry starts with; and a key the file
/// had before the program started. A bit each in the exit code.
#[test]
fn a_program_reads_and_writes_the_registry() {
    const HKEY_CURRENT_USER: i32 = 0x8000_0001u32 as i32;
    const HKEY_LOCAL_MACHINE: i32 = 0x8000_0002u32 as i32;
    let names: &[&str] = &[
        "RegCreateKeyA",
        "RegSetValueExA",
        "RegCloseKey",
        "RegOpenKeyExA",
        "RegQueryValueExA",
        "RegOpenKeyA",
    ];
    let imports: &[(&str, &[&str])] = &[("ADVAPI32.DLL", names)];
    // Strings at 0, 0x10, 0x14, 0x40 and 0x80; handles at 0x50, 0x54
    // and 0x90; a DWORD to store at 0x58, a size at 0x5C and a buffer at
    // 0x60.
    let mut data = b"Software\\Test\0\0\0V\0\0\0".to_vec();
    data.extend(b"Software\\Microsoft\\Windows\\CurrentVersion\0");
    data.resize(0x40, 0);
    data.extend(b"VersionNumber\0");
    data.resize(0x58, 0);
    data.extend(0x1234_5678u32.to_le_bytes());
    data.resize(0x80, 0);
    data.extend(b"Software\\Pre\0");
    data.resize(0xA0, 0);
    let exe = program(BASE, false, imports, &data, |a, l| {
        let f = |name: &str| l.import(names.iter().position(|n| *n == name).unwrap());
        let bit = |a: &mut CodeAssembler, n: u32| -> Result<(), IcedError> {
            a.sete(al)?;
            a.movzx(eax, al)?;
            a.shl(eax, n)?;
            a.or(ebx, eax)
        };
        a.xor(ebx, ebx)?;
        a.push(l.at(0x50) as i32)?;
        a.push(l.at(0) as i32)?;
        a.push(HKEY_CURRENT_USER)?;
        a.call(f("RegCreateKeyA"))?;
        a.test(eax, eax)?;
        bit(a, 0)?;
        a.push(4)?;
        a.push(l.at(0x58) as i32)?;
        a.push(4)?;
        a.push(0)?;
        a.push(l.at(0x10) as i32)?;
        a.push(dword_ptr(l.at(0x50)))?;
        a.call(f("RegSetValueExA"))?;
        a.test(eax, eax)?;
        bit(a, 1)?;
        a.push(dword_ptr(l.at(0x50)))?;
        a.call(f("RegCloseKey"))?;
        a.test(eax, eax)?;
        bit(a, 2)?;
        a.push(l.at(0x54) as i32)?;
        a.push(0x2_0019)?;
        a.push(0)?;
        a.push(l.at(0x14) as i32)?;
        a.push(HKEY_LOCAL_MACHINE)?;
        a.call(f("RegOpenKeyExA"))?;
        a.test(eax, eax)?;
        bit(a, 3)?;
        a.mov(dword_ptr(l.at(0x5C)), 32)?;
        a.push(l.at(0x5C) as i32)?;
        a.push(l.at(0x60) as i32)?;
        a.push(0)?;
        a.push(0)?;
        a.push(l.at(0x40) as i32)?;
        a.push(dword_ptr(l.at(0x54)))?;
        a.call(f("RegQueryValueExA"))?;
        a.test(eax, eax)?;
        bit(a, 4)?;
        // "4.00.950" and its zero.
        a.cmp(dword_ptr(l.at(0x5C)), 9)?;
        bit(a, 5)?;
        // The value set is in another key.
        a.push(0)?;
        a.push(0)?;
        a.push(0)?;
        a.push(0)?;
        a.push(l.at(0x10) as i32)?;
        a.push(dword_ptr(l.at(0x54)))?;
        a.call(f("RegQueryValueExA"))?;
        a.cmp(eax, 2)?;
        bit(a, 6)?;
        a.push(l.at(0x90) as i32)?;
        a.push(l.at(0x80) as i32)?;
        a.push(HKEY_CURRENT_USER)?;
        a.call(f("RegOpenKeyA"))?;
        a.test(eax, eax)?;
        bit(a, 7)?;
        a.mov(eax, ebx)?;
        a.ret()
    });
    let before = b"REGEDIT4\r\n\r\n[HKEY_CURRENT_USER\\Software\\Pre]\r\n\"P\"=dword:7\r\n";
    let dir = scratch(
        "registry",
        &[("T.EXE", exe), ("REGISTRY.REG", before.to_vec())],
    );
    let mut cpu = Cpu::new(dir.clone());
    cpu.bus.win32.enabled = true;
    cpu.host_console = Some(HostConsole::default());
    cpu.load_shell();
    assert!(cpu.load_executable("T.EXE", None));
    assert!(run_to_exit(&mut cpu, 200), "{:?}", printed(&cpu));
    let code = cpu.errorlevel;
    assert_eq!(code, 0xFF, "the checks that failed: {:08b}", !code);
    let saved = fs::read_to_string(dir.join("REGISTRY.REG")).unwrap();
    assert!(saved.starts_with("REGEDIT4\r\n"));
    let test = "[HKEY_USERS\\.Default\\Software\\Test]\r\n\"V\"=dword:12345678\r\n";
    assert!(saved.contains(test), "{}", saved);
    let pre = "[HKEY_USERS\\.Default\\Software\\Pre]\r\n\"P\"=dword:00000007\r\n";
    assert!(saved.contains(pre), "{}", saved);
}

/// Push the arguments `args` of a stdcall function, the last first.
fn push_args(a: &mut CodeAssembler, args: &[i32]) -> Result<(), IcedError> {
    for &arg in args.iter().rev() {
        a.push(arg)?;
    }
    Ok(())
}

/// Set bit `bit` of EBX when the flags say equal.
fn bit_if_equal(a: &mut CodeAssembler, bit: u32) -> Result<(), IcedError> {
    let mut skip = a.create_label();
    a.jne(skip)?;
    a.or(ebx, 1 << bit)?;
    a.set_label(&mut skip)?;
    a.nop()
}

/// A region reserved, a page of it committed, queried, protected and
/// checked with IsBad*Ptr, then released; and GetSystemInfo. A bit each
/// in the exit code.
#[test]
fn a_program_reserves_commits_and_protects_memory() {
    const MEM_COMMIT: i32 = 0x1000;
    const MEM_RESERVE: i32 = 0x2000;
    const MEM_RELEASE: i32 = 0x8000;
    const MEM_FREE: i32 = 0x1_0000;
    const PAGE_NOACCESS: i32 = 1;
    const PAGE_READONLY: i32 = 2;
    const PAGE_READWRITE: i32 = 4;
    let names: &[&str] = &[
        "VirtualAlloc",
        "VirtualQuery",
        "VirtualProtect",
        "VirtualFree",
        "IsBadReadPtr",
        "IsBadWritePtr",
        "GetSystemInfo",
    ];
    let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", names)];
    let (code, text) = run(
        "virtual",
        program(BASE, false, imports, &[0; 0x100], |a, l| {
            let f = |name: &str| l.import(names.iter().position(|n| *n == name).unwrap());
            let info = l.at(0x40) as i32;
            let old = l.at(0x80) as i32;
            a.xor(ebx, ebx)?;
            // Three pages reserved, the second committed.
            push_args(a, &[0, 0x3000, MEM_RESERVE, PAGE_NOACCESS])?;
            a.call(f("VirtualAlloc"))?;
            a.mov(esi, eax)?;
            a.lea(edi, dword_ptr(esi + 0x1000))?;
            push_args(a, &[0x1000, MEM_COMMIT, PAGE_READWRITE])?;
            a.push(edi)?;
            a.call(f("VirtualAlloc"))?;
            a.cmp(eax, edi)?;
            bit_if_equal(a, 0)?;
            a.mov(dword_ptr(edi), 0x55)?;
            // The first page is reserved, the second committed.
            push_args(a, &[info, 28])?;
            a.push(esi)?;
            a.call(f("VirtualQuery"))?;
            a.mov(eax, dword_ptr(info as u64 + 16))?;
            a.cmp(eax, MEM_RESERVE)?;
            bit_if_equal(a, 1)?;
            push_args(a, &[info, 28])?;
            a.push(edi)?;
            a.call(f("VirtualQuery"))?;
            a.mov(eax, dword_ptr(info as u64 + 20))?;
            a.cmp(eax, PAGE_READWRITE)?;
            bit_if_equal(a, 2)?;
            // Read only now, after read and write.
            push_args(a, &[0x1000, PAGE_READONLY, old])?;
            a.push(edi)?;
            a.call(f("VirtualProtect"))?;
            a.mov(eax, dword_ptr(old as u64))?;
            a.cmp(eax, PAGE_READWRITE)?;
            bit_if_equal(a, 3)?;
            // Not writable, readable; the reserved page isn't readable.
            a.push(4)?;
            a.push(edi)?;
            a.call(f("IsBadWritePtr"))?;
            a.mov(ebp, eax)?;
            a.push(4)?;
            a.push(edi)?;
            a.call(f("IsBadReadPtr"))?;
            a.shl(eax, 1)?;
            a.or(ebp, eax)?;
            a.push(1)?;
            a.push(esi)?;
            a.call(f("IsBadReadPtr"))?;
            a.shl(eax, 2)?;
            a.or(ebp, eax)?;
            a.cmp(ebp, 5)?;
            bit_if_equal(a, 4)?;
            // Released, it is free.
            push_args(a, &[0, MEM_RELEASE])?;
            a.push(esi)?;
            a.call(f("VirtualFree"))?;
            push_args(a, &[info, 28])?;
            a.push(esi)?;
            a.call(f("VirtualQuery"))?;
            a.mov(eax, dword_ptr(info as u64 + 16))?;
            a.cmp(eax, MEM_FREE)?;
            bit_if_equal(a, 5)?;
            // 4 KB pages, 64 KB granularity, a 486.
            a.push(info)?;
            a.call(f("GetSystemInfo"))?;
            a.mov(eax, dword_ptr(info as u64 + 4))?;
            a.add(eax, dword_ptr(info as u64 + 28))?;
            a.add(eax, dword_ptr(info as u64 + 24))?;
            a.cmp(eax, 0x1000 + 0x1_0000 + 486)?;
            bit_if_equal(a, 6)?;
            a.mov(eax, ebx)?;
            a.ret()
        }),
    );
    assert_eq!(
        code,
        0x7F,
        "the checks that failed: {:07b} {}",
        !code & 0x7F,
        text
    );
}

/// A program builder at `BASE` whose stack reserve and commit sizes are
/// `reserve` and `commit`.
fn with_stack(reserve: u32, commit: u32) -> Builder {
    let mut b = Builder::exe(BASE);
    b.stack(reserve, commit);
    b
}

/// A stack's reservation goes 64 KB below its 64 KB multiple of the
/// reserve, as on Windows 98, whose main thread and CreateThread's
/// threads of a program with a 20 KB reserve have 128 KB between
/// VirtualQuery's AllocationBase and the TIB's stack top. Watcom's C
/// library takes the allocation base plus 76 KB as the lowest its
/// stack may go on Windows 95 and 98 (Cryo's Riverworld stops with
/// "Stack Overflow!" without it). A bit each in the exit code.
#[test]
fn a_stack_reservation_has_64_kb_below_the_stack() {
    let names: &[&str] = &["VirtualQuery", "CreateThread", "WaitForSingleObject"];
    let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", names)];
    let b = with_stack(0x5000, 0x5000);
    let program = program_with(b, false, imports, &[0; 0x100], |a, l| {
        let f = |name: &str| l.import(names.iter().position(|n| *n == name).unwrap());
        let info = l.at(0x40) as i32;
        let thread_info = l.at(0x80) as i32;
        let thread_span = l.at(0xC0);
        let mut over = a.create_label();
        a.xor(ebx, ebx)?;
        // The main thread's stack, from its allocation base to its top.
        push_args(a, &[info, 28])?;
        a.push(esp)?;
        a.call(f("VirtualQuery"))?;
        a.mov(eax, dword_ptr(4).fs())?;
        a.sub(eax, dword_ptr(info as u64 + 4))?;
        a.cmp(eax, 0x2_0000)?;
        bit_if_equal(a, 0)?;
        // Its committed pages end at the top.
        a.mov(eax, dword_ptr(info as u64))?;
        a.add(eax, dword_ptr(info as u64 + 12))?;
        a.cmp(eax, dword_ptr(4).fs())?;
        bit_if_equal(a, 1)?;
        // A thread with the program's stack size, which measures its own:
        // its parameter, flags and where its id goes (which Windows 98
        // needs).
        push_args(a, &[0, 0, l.at(0xC4) as i32])?;
        // Its address, which the CALL over it pushes.
        a.call(over)?;
        push_args(a, &[thread_info, 28])?;
        a.push(esp)?;
        a.call(f("VirtualQuery"))?;
        a.mov(eax, dword_ptr(4).fs())?;
        a.sub(eax, dword_ptr(thread_info as u64 + 4))?;
        a.mov(dword_ptr(thread_span), eax)?;
        a.ret_1(4)?;
        a.set_label(&mut over)?;
        push_args(a, &[0, 0])?;
        a.call(f("CreateThread"))?;
        a.push(-1)?;
        a.push(eax)?;
        a.call(f("WaitForSingleObject"))?;
        a.cmp(dword_ptr(thread_span), 0x2_0000)?;
        bit_if_equal(a, 2)?;
        a.mov(eax, ebx)?;
        a.ret()
    });
    let (code, text) = run("stack_floor", program);
    assert_eq!(
        code,
        7,
        "the checks that failed: {:03b} {}",
        !code & 7,
        text
    );
}

/// A thread's stack grows down to the floor below it, and a page fault in
/// the floor is a stack overflow: a thread with a 128 KB stack, 64 KB of
/// it committed, reads 96 KB below its top and then 32 KB into the floor,
/// and the unhandled exception filter ends the program with a bit each
/// for the exception's code, its address and the read that grew the
/// stack.
#[test]
fn a_stack_grows_to_its_floor_and_overflows_in_it() {
    let names: &[&str] = &[
        "SetUnhandledExceptionFilter",
        "CreateThread",
        "WaitForSingleObject",
        "ExitProcess",
    ];
    let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", names)];
    let b = with_stack(0x1_0000, 0x1000);
    let program = program_with(b, false, imports, &[0; 0x10], |a, l| {
        let f = |name: &str| l.import(names.iter().position(|n| *n == name).unwrap());
        let (grew, floor, id) = (l.at(0), l.at(4), l.at(8));
        let mut over_filter = a.create_label();
        let mut over_thread = a.create_label();
        // The filter, whose address the CALL over it pushes as
        // SetUnhandledExceptionFilter's argument: the record's code at
        // 0, the fault's address at 24.
        a.call(over_filter)?;
        a.mov(eax, dword_ptr(esp + 4))?;
        a.mov(ecx, dword_ptr(eax))?;
        a.xor(ebx, ebx)?;
        a.cmp(dword_ptr(ecx), 0xC000_00FDu32 as i32)?;
        bit_if_equal(a, 0)?;
        a.mov(eax, dword_ptr(ecx + 24))?;
        a.cmp(eax, dword_ptr(floor))?;
        bit_if_equal(a, 1)?;
        a.cmp(dword_ptr(grew), 1)?;
        bit_if_equal(a, 2)?;
        a.push(ebx)?;
        a.call(f("ExitProcess"))?;
        a.int3()?;
        a.set_label(&mut over_filter)?;
        a.call(f("SetUnhandledExceptionFilter"))?;
        // A thread with a 128 KB stack: its parameter, flags and where
        // its id goes, then its address, which the CALL over it pushes.
        push_args(a, &[0, 0, id as i32])?;
        a.call(over_thread)?;
        a.mov(eax, dword_ptr(4).fs())?;
        a.mov(ecx, dword_ptr(eax - 0x1_8000))?;
        a.mov(dword_ptr(grew), 1)?;
        a.sub(eax, 0x2_8000)?;
        a.mov(dword_ptr(floor), eax)?;
        a.mov(ecx, dword_ptr(eax))?;
        a.xor(eax, eax)?;
        a.ret_1(4)?;
        a.set_label(&mut over_thread)?;
        push_args(a, &[0, 0x2_0000])?;
        a.call(f("CreateThread"))?;
        a.push(-1)?;
        a.push(eax)?;
        a.call(f("WaitForSingleObject"))?;
        // The thread ended without a fault.
        a.xor(eax, eax)?;
        a.ret()
    });
    let (code, text) = run("stack_overflow", program);
    assert_eq!(
        code,
        7,
        "the checks that failed: {:03b} {}",
        !code & 7,
        text
    );
}

/// A heap of its own with a maximum size, a moveable global block, and a
/// mapping of the swap file viewed twice. A bit each in the exit code.
#[test]
fn a_program_uses_heaps_global_memory_and_mappings() {
    const GMEM_MOVEABLE: i32 = 2;
    const PAGE_READWRITE: i32 = 4;
    const FILE_MAP_WRITE: i32 = 2;
    const FILE_MAP_READ: i32 = 4;
    let names: &[&str] = &[
        "HeapCreate",
        "HeapAlloc",
        "HeapDestroy",
        "GlobalAlloc",
        "GlobalLock",
        "GlobalFlags",
        "CreateFileMappingA",
        "MapViewOfFile",
        "UnmapViewOfFile",
    ];
    let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", names)];
    let (code, text) = run(
        "heaps",
        program(BASE, false, imports, &[0; 0x40], |a, l| {
            let f = |name: &str| l.import(names.iter().position(|n| *n == name).unwrap());
            a.xor(ebx, ebx)?;
            // 64 KB at most: a larger block doesn't fit, a small one does.
            push_args(a, &[0, 0x1000, 0x1_0000])?;
            a.call(f("HeapCreate"))?;
            a.mov(esi, eax)?;
            push_args(a, &[0, 0x1_0001])?;
            a.push(esi)?;
            a.call(f("HeapAlloc"))?;
            a.mov(edi, eax)?;
            push_args(a, &[8, 0x100])?;
            a.push(esi)?;
            a.call(f("HeapAlloc"))?;
            a.test(eax, eax)?;
            a.setne(al)?;
            a.test(edi, edi)?;
            a.sete(ah)?;
            a.cmp(ax, 0x0101)?;
            bit_if_equal(a, 0)?;
            a.push(esi)?;
            a.call(f("HeapDestroy"))?;
            a.cmp(eax, 1)?;
            bit_if_equal(a, 1)?;
            // A moveable block's handle points at its address.
            push_args(a, &[GMEM_MOVEABLE, 16])?;
            a.call(f("GlobalAlloc"))?;
            a.mov(esi, eax)?;
            a.push(esi)?;
            a.call(f("GlobalLock"))?;
            a.cmp(eax, dword_ptr(esi))?;
            bit_if_equal(a, 2)?;
            a.push(esi)?;
            a.call(f("GlobalFlags"))?;
            a.cmp(eax, 1)?;
            bit_if_equal(a, 3)?;
            // Two views of a mapping, at one address in the shared arena.
            push_args(a, &[-1, 0, PAGE_READWRITE, 0, 0x2000, 0])?;
            a.call(f("CreateFileMappingA"))?;
            a.mov(ebp, eax)?;
            push_args(a, &[FILE_MAP_WRITE, 0, 0, 0])?;
            a.push(ebp)?;
            a.call(f("MapViewOfFile"))?;
            a.mov(esi, eax)?;
            push_args(a, &[FILE_MAP_READ, 0, 0, 0])?;
            a.push(ebp)?;
            a.call(f("MapViewOfFile"))?;
            a.mov(edi, eax)?;
            a.cmp(esi, edi)?;
            bit_if_equal(a, 4)?;
            // The data stays while one view is left.
            a.mov(dword_ptr(esi + 0x1000), 7)?;
            a.push(esi)?;
            a.call(f("UnmapViewOfFile"))?;
            a.cmp(dword_ptr(edi + 0x1000), 7)?;
            bit_if_equal(a, 5)?;
            a.mov(eax, ebx)?;
            a.ret()
        }),
    );
    assert_eq!(
        code,
        0x3F,
        "the checks that failed: {:06b} {}",
        !code & 0x3F,
        text
    );
}

/// HeapAlloc with HEAP_GENERATE_EXCEPTIONS raises STATUS_NO_MEMORY when
/// the heap is full; with no handler the program ends saying so.
#[test]
fn a_full_heap_raises_an_exception_with_heap_generate_exceptions() {
    const HEAP_GENERATE_EXCEPTIONS: i32 = 4;
    let names: &[&str] = &["HeapCreate", "HeapAlloc"];
    let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", names)];
    let (code, text) = run(
        "heapexception",
        program(BASE, false, imports, &[0], |a, l| {
            push_args(a, &[HEAP_GENERATE_EXCEPTIONS, 0x1000, 0x1_0000])?;
            a.call(l.import(0))?;
            push_args(a, &[0, 0x2_0000])?;
            a.push(eax)?;
            a.call(l.import(1))?;
            a.ret()
        }),
    );
    assert_eq!(code, 0xFF);
    assert!(text.contains("C0000017"), "{}", text);
}

/// An argument a test program passes: a value, or ESI.
#[derive(Clone, Copy)]
enum Arg {
    I(u32),
    Esi,
}

/// Call the import `slot` with `args`, pushed last first.
fn call_with(a: &mut CodeAssembler, slot: AsmMemoryOperand, args: &[Arg]) -> Result<(), IcedError> {
    for arg in args.iter().rev() {
        match *arg {
            Arg::I(value) => a.push(value as i32)?,
            Arg::Esi => a.push(esi)?,
        }
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

/// Put `bytes` at `offset` of `data`.
fn put(data: &mut [u8], offset: usize, bytes: &[u8]) {
    data[offset..offset + bytes.len()].copy_from_slice(bytes);
}

#[test]
fn a_program_makes_reads_finds_moves_and_deletes_files() {
    use Arg::{Esi, I};
    let names: &[&str] = &[
        "CreateDirectoryA",
        "CreateFileA",
        "WriteFile",
        "CloseHandle",
        "GetFileSize",
        "SetFilePointer",
        "ReadFile",
        "FindFirstFileA",
        "FindClose",
        "MoveFileA",
        "DeleteFileA",
        "RemoveDirectoryA",
    ];
    let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", names)];
    // The directory, the file, a pattern, the new name, the text; a count
    // at 50h, a buffer at 60h, WIN32_FIND_DATA at 100h.
    let mut data = vec![0; 0x300];
    put(&mut data, 0, b"D\0");
    put(&mut data, 0x10, b"D\\F.TXT\0");
    put(&mut data, 0x20, b"D\\*.TXT\0");
    put(&mut data, 0x30, b"G.TXT\0");
    put(&mut data, 0x40, b"hello");
    let mut cpu = machine(
        "files",
        program(BASE, false, imports, &data, |a, l| {
            let f = |name: &str| l.import(names.iter().position(|n| *n == name).unwrap());
            let at = |offset: u32| I(l.at(offset) as u32);
            let (file, count, buffer) = (at(0x10), at(0x50), at(0x60));
            a.xor(ebx, ebx)?;
            call_with(a, f("CreateDirectoryA"), &[at(0), I(0)])?;
            a.test(eax, eax)?;
            check(a, true, 0)?;
            // Five bytes written to a new file.
            let new = [file, I(0x4000_0000), I(0), I(0), I(1), I(0x80), I(0)];
            call_with(a, f("CreateFileA"), &new)?;
            a.mov(esi, eax)?;
            call_with(a, f("WriteFile"), &[Esi, at(0x40), I(5), count, I(0)])?;
            a.cmp(dword_ptr(l.at(0x50)), 5)?;
            check(a, false, 1)?;
            call_with(a, f("CloseHandle"), &[Esi])?;
            // Read back from the second byte on.
            let open = [file, I(0x8000_0000), I(1), I(0), I(3), I(0x80), I(0)];
            call_with(a, f("CreateFileA"), &open)?;
            a.mov(esi, eax)?;
            call_with(a, f("GetFileSize"), &[Esi, I(0)])?;
            a.cmp(eax, 5)?;
            check(a, false, 2)?;
            call_with(a, f("SetFilePointer"), &[Esi, I(1), I(0), I(0)])?;
            a.cmp(eax, 1)?;
            check(a, false, 3)?;
            call_with(a, f("ReadFile"), &[Esi, buffer, I(8), count, I(0)])?;
            a.cmp(dword_ptr(l.at(0x60)), u32::from_le_bytes(*b"ello") as i32)?;
            check(a, false, 4)?;
            call_with(a, f("CloseHandle"), &[Esi])?;
            // A search finds it, five bytes long.
            call_with(a, f("FindFirstFileA"), &[at(0x20), at(0x100)])?;
            a.mov(esi, eax)?;
            call_with(a, f("FindClose"), &[Esi])?;
            a.cmp(dword_ptr(l.at(0x100 + 32)), 5)?;
            check(a, false, 5)?;
            // Moved out of the directory, deleted, and the directory gone.
            call_with(a, f("MoveFileA"), &[file, at(0x30)])?;
            a.test(eax, eax)?;
            check(a, true, 6)?;
            call_with(a, f("DeleteFileA"), &[at(0x30)])?;
            call_with(a, f("RemoveDirectoryA"), &[at(0)])?;
            a.test(eax, eax)?;
            check(a, true, 7)?;
            a.mov(eax, ebx)?;
            a.ret()
        }),
    );
    assert!(
        run_to_exit(&mut cpu, 200),
        "the program didn't end: {:?}",
        printed(&cpu)
    );
    let code = cpu.errorlevel;
    assert_eq!(code, 0xFF, "the checks that failed: {:08b}", !code);
    let dir = PathBuf::from("target/test_win32/files");
    assert!(!dir.join("D").exists() && !dir.join("G.TXT").exists());
}

/// `cycles=auto` gives a Win32 program that computes without drawing,
/// reading or waiting (a game unpacking its level) all the host has: the
/// built-in Windows halts the processor when no thread can run, so one
/// that keeps it from halting is at work, as under Windows 98.
#[test]
fn auto_speed_gives_a_busy_win32_program_all_the_host_has() {
    use rust_dos::timer::{CpuSpeed, Pacer};
    use std::time::{Duration, Instant};
    // A loop that counts for ever.
    let mut cpu = machine(
        "busy",
        program(BASE, false, &[], &[0; 4], |a, _| {
            let mut top = a.create_label();
            a.set_label(&mut top)?;
            a.inc(eax)?;
            a.jmp(top)
        }),
    );
    let mut pacer = Pacer::new(CpuSpeed::default(), None, Instant::now());
    // Up to two seconds of emulated time, a 16 ms frame at a time, on a
    // host that runs 333,000 instructions in 2 ms.
    for _ in 0..125 {
        if cpu.bus.clock.cycles_per_ms() > 100_000 {
            break;
        }
        let frame = cpu.bus.clock.cycles_per_ms() as u64 * 16;
        cpu.bus.start_batch(cpu.bus.clock.icount + frame);
        run_batch(&mut cpu, &mut NoHook, false);
        let fast = Duration::from_millis(2);
        if let Some(speed) = pacer.end_cpu_frame(&mut cpu, 333_000, fast, Duration::ZERO) {
            cpu.bus.set_cycles_per_ms(speed);
        }
    }
    assert!(cpu.bus.win32.active());
    let speed = cpu.bus.clock.cycles_per_ms();
    assert!(speed > 100_000, "{}", speed);
}

/// A program that reads a file with ReadFile and the C runtime's _read
/// moves its bytes on the drive, as a DOS program's reads do: what
/// `cycles=auto` sees a program load by.
#[test]
fn a_programs_file_reads_count_as_disk_activity() {
    use Arg::I;
    const SIZE: usize = 0x10000;
    let names: &[&str] = &["VirtualAlloc", "CreateFileA", "ReadFile", "CloseHandle"];
    let crt: &[&str] = &["_open", "_read", "_close"];
    let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", names), ("MSVCRT.dll", crt)];
    // The file's name at 0, a count at 10h. The buffer is allocated, so
    // that the program, which DOS reads to start it, stays small.
    let mut data = vec![0; 0x20];
    put(&mut data, 0, b"R.DAT\0");
    let program = program(BASE, false, imports, &data, |a, l| {
        let f = |name: &str| l.import(names.iter().position(|n| *n == name).unwrap());
        let c = |name: &str| l.import(names.len() + crt.iter().position(|n| *n == name).unwrap());
        let (file, count) = (I(l.at(0) as u32), l.at(0x10) as i32);
        // VirtualAlloc(NULL, SIZE, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE)
        call_with(
            a,
            f("VirtualAlloc"),
            &[I(0), I(SIZE as u32), I(0x3000), I(4)],
        )?;
        a.mov(edi, eax)?;
        let open = [file, I(0x8000_0000), I(1), I(0), I(3), I(0x80), I(0)];
        call_with(a, f("CreateFileA"), &open)?;
        a.mov(esi, eax)?;
        a.push(0)?;
        a.push(count)?;
        a.push(SIZE as i32)?;
        a.push(edi)?;
        a.push(esi)?;
        a.call(f("ReadFile"))?;
        call_with(a, f("CloseHandle"), &[Arg::Esi])?;
        // _open(name, O_RDONLY | O_BINARY), cdecl.
        call_with(a, c("_open"), &[file, I(0x8000)])?;
        a.add(esp, 8)?;
        a.mov(esi, eax)?;
        a.push(SIZE as i32)?;
        a.push(edi)?;
        a.push(esi)?;
        a.call(c("_read"))?;
        a.add(esp, 12)?;
        call_with(a, c("_close"), &[Arg::Esi])?;
        a.add(esp, 4)?;
        a.xor(eax, eax)?;
        a.ret()
    });
    let files = [("T.EXE", program), ("R.DAT", vec![0x5A; SIZE])];
    let mut cpu = machine_with("file_reads", &files);
    let before = cpu.bus.activity.io_bytes;
    assert!(
        run_to_exit(&mut cpu, 200),
        "the program didn't end: {:?}",
        printed(&cpu)
    );
    assert_eq!(cpu.errorlevel, 0);
    let moved = cpu.bus.activity.io_bytes - before;
    assert!(moved >= 2 * SIZE as u64, "{} bytes moved", moved);
}

#[test]
fn a_program_finds_its_paths_and_console() {
    use Arg::{Esi, I};
    let names: &[&str] = &[
        "GetWindowsDirectoryA",
        "GetFullPathNameA",
        "GetCurrentDirectoryA",
        "GetStdHandle",
        "GetConsoleMode",
        "GetConsoleScreenBufferInfo",
        "GetTempFileNameA",
        "DeleteFileA",
    ];
    let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", names)];
    // A relative path, the temporary directory and prefix; a pointer at
    // 30h, a buffer at 40h, CONSOLE_SCREEN_BUFFER_INFO at 100h.
    let mut data = vec![0; 0x200];
    put(&mut data, 0, b"x\\..\\a.b\0");
    put(&mut data, 0x10, b".\0");
    put(&mut data, 0x18, b"pfx\0");
    let (code, _) = run(
        "paths",
        program(BASE, false, imports, &data, |a, l| {
            let f = |name: &str| l.import(names.iter().position(|n| *n == name).unwrap());
            let at = |offset: u32| I(l.at(offset) as u32);
            let buffer = at(0x40);
            a.xor(ebx, ebx)?;
            // C:\WINDOWS.
            call_with(a, f("GetWindowsDirectoryA"), &[buffer, I(0x40)])?;
            a.cmp(eax, 10)?;
            check(a, false, 0)?;
            // C:\a.b, its name from the fourth character.
            let full = [at(0), I(0x40), buffer, at(0x30)];
            call_with(a, f("GetFullPathNameA"), &full)?;
            a.cmp(eax, 6)?;
            check(a, false, 1)?;
            a.cmp(dword_ptr(l.at(0x30)), (l.at(0x40) + 3) as i32)?;
            check(a, false, 2)?;
            // C:\.
            call_with(a, f("GetCurrentDirectoryA"), &[I(0x40), buffer])?;
            a.cmp(eax, 3)?;
            check(a, false, 3)?;
            // The console's input has a mode, its output 80 columns.
            call_with(a, f("GetStdHandle"), &[I(-10i32 as u32)])?;
            a.mov(esi, eax)?;
            call_with(a, f("GetConsoleMode"), &[Esi, at(0x30)])?;
            a.test(eax, eax)?;
            check(a, true, 4)?;
            let output = I(STD_OUTPUT_HANDLE as u32);
            call_with(a, f("GetStdHandle"), &[output])?;
            a.mov(esi, eax)?;
            call_with(a, f("GetConsoleScreenBufferInfo"), &[Esi, at(0x100)])?;
            a.cmp(word_ptr(l.at(0x100)), 80)?;
            check(a, false, 5)?;
            // A temporary file made, and deleted.
            let temp = [at(0x10), at(0x18), I(0), buffer];
            call_with(a, f("GetTempFileNameA"), &temp)?;
            a.test(eax, eax)?;
            check(a, true, 6)?;
            call_with(a, f("DeleteFileA"), &[buffer])?;
            a.test(eax, eax)?;
            check(a, true, 7)?;
            a.mov(eax, ebx)?;
            a.ret()
        }),
    );
    assert_eq!(code, 0xFF, "the checks that failed: {:08b}", !code);
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
    let mut cpu = machine_with(test, files);
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
        check(a, false, 0)?;
        a.cmp(dword_ptr((A_COUNTS + 4) as u64), 1)?;
        check(a, false, 1)?;
        a.cmp(dword_ptr((B_COUNTS + 4) as u64), 1)?;
        check(a, false, 2)?;
        a.cmp(dword_ptr(B_FIRST as u64), 1)?;
        check(a, false, 3)?;
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
        check(a, false, 0)?;
        // A.DLL, and B.DLL with it.
        a.push(l.at(9) as i32)?;
        a.call(f("LoadLibraryA"))?;
        a.mov(esi, eax)?;
        a.push(l.at(15) as i32)?;
        a.push(esi)?;
        a.call(f("GetProcAddress"))?;
        a.call(eax)?;
        a.cmp(eax, 42)?;
        check(a, false, 1)?;
        a.mov(eax, dword_ptr((A_COUNTS + 4) as u64))?;
        a.add(eax, dword_ptr((B_COUNTS + 4) as u64))?;
        a.add(eax, dword_ptr(B_FIRST as u64))?;
        a.cmp(eax, 3)?;
        check(a, false, 2)?;
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
        check(a, false, 3)?;
        // Freed, both.
        a.push(esi)?;
        a.call(f("FreeLibrary"))?;
        a.add(eax, dword_ptr(A_COUNTS as u64))?;
        a.add(eax, dword_ptr(B_COUNTS as u64))?;
        a.cmp(eax, 3)?;
        check(a, false, 4)?;
        a.push(l.at(21) as i32)?;
        a.call(f("GetModuleHandleA"))?;
        a.test(eax, eax)?;
        check(a, false, 5)?;
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
        check(a, false, 0)?;
        a.call(l.import(1))?;
        a.cmp(eax, 1114)?;
        check(a, false, 1)?;
        a.push(l.at(0) as i32)?;
        a.call(l.import(2))?;
        a.test(eax, eax)?;
        check(a, false, 2)?;
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
        check(a, false, 0)?;
        a.push(esi)?;
        a.push(0)?;
        a.call(f("LoadResource"))?;
        a.push(eax)?;
        a.call(f("LockResource"))?;
        a.cmp(dword_ptr(eax), 0x6C6C6568)?;
        check(a, false, 1)?;
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
        check(a, false, 2)?;
        // "C:\T.EXE", and cut to fit 4 bytes with its NUL.
        a.push(0x100)?;
        a.push(l.at(0) as i32)?;
        a.push(0)?;
        a.call(f("GetModuleFileNameA"))?;
        a.cmp(eax, 8)?;
        check(a, false, 3)?;
        a.push(4)?;
        a.push(l.at(0x100) as i32)?;
        a.push(0)?;
        a.call(f("GetModuleFileNameA"))?;
        a.cmp(eax, 3)?;
        check(a, false, 4)?;
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
        check(a, false, 5)?;
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
            check(a, false, bit as u32)?;
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

/// A profile written through DOS's files reads back, and is on the disk
/// as Windows writes it; a bit each in the exit code.
#[test]
fn a_program_writes_and_reads_a_profile() {
    let names: &[&str] = &[
        "WritePrivateProfileStringA",
        "GetPrivateProfileIntA",
        "GetPrivateProfileStringA",
    ];
    let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", names)];
    // The section, key, value and file; a buffer at 0x20.
    let mut data = b"App\0key\x0042\0.\\T.INI\0".to_vec();
    data.resize(0x40, 0);
    let (code, _) = run(
        "profile",
        program(BASE, false, imports, &data, |a, l| {
            let f = |name: &str| l.import(names.iter().position(|n| *n == name).unwrap());
            a.xor(ebx, ebx)?;
            a.push(l.at(0x0B) as i32)?;
            a.push(l.at(0x08) as i32)?;
            a.push(l.at(0x04) as i32)?;
            a.push(l.at(0x00) as i32)?;
            a.call(f("WritePrivateProfileStringA"))?;
            a.cmp(eax, 1)?;
            a.sete(bl)?;
            // The value as a number.
            a.push(l.at(0x0B) as i32)?;
            a.push(7)?;
            a.push(l.at(0x04) as i32)?;
            a.push(l.at(0x00) as i32)?;
            a.call(f("GetPrivateProfileIntA"))?;
            a.cmp(eax, 42)?;
            a.sete(al)?;
            a.shl(al, 1)?;
            a.or(bl, al)?;
            // The section's keys: "key" and its zero, 4 bytes.
            a.push(l.at(0x0B) as i32)?;
            a.push(0x20)?;
            a.push(l.at(0x20) as i32)?;
            a.push(l.at(0x0A) as i32)?;
            a.push(0)?;
            a.push(l.at(0x00) as i32)?;
            a.call(f("GetPrivateProfileStringA"))?;
            a.cmp(eax, 4)?;
            a.sete(al)?;
            a.shl(al, 2)?;
            a.or(bl, al)?;
            // A key that isn't there gives the default.
            a.push(l.at(0x0B) as i32)?;
            a.push(7)?;
            a.push(l.at(0x00) as i32)?;
            a.push(l.at(0x00) as i32)?;
            a.call(f("GetPrivateProfileIntA"))?;
            a.cmp(eax, 7)?;
            a.sete(al)?;
            a.shl(al, 3)?;
            a.or(bl, al)?;
            a.mov(eax, ebx)?;
            a.ret()
        }),
    );
    assert_eq!(code, 0xF, "the checks that failed: {:04b}", !code & 0xF);
    let file = fs::read("target/test_win32/profile/T.INI").unwrap();
    assert_eq!(file, b"[App]\r\nkey=42\r\n");
}

/// FormatMessageA's inserts, EnumSystemLocalesA calling the program
/// back, MulDiv, GetDateFormatA, the performance counter's rate and
/// lstrcmpiA; a bit each in the exit code.
#[test]
fn a_program_formats_text_and_enumerates_locales() {
    let names: &[&str] = &[
        "FormatMessageA",
        "EnumSystemLocalesA",
        "MulDiv",
        "GetDateFormatA",
        "QueryPerformanceFrequency",
        "lstrcmpiA",
    ];
    let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", names)];
    // The message, its string insert, two strings to compare, a date
    // picture and the date (2 January 2000); the inserts' array at 0x40,
    // the callback's count at 0x48, the rate at 0x50 and buffers at 0x60
    // and 0xA0.
    let mut data = vec![0u8; 0xC0];
    data[0x00..0x09].copy_from_slice(b"%1!d!-%2\0");
    data[0x10..0x12].copy_from_slice(b"x\0");
    data[0x14..0x18].copy_from_slice(b"abc\0");
    data[0x18..0x1C].copy_from_slice(b"ABC\0");
    data[0x20..0x2F].copy_from_slice(b"yyyy-MM-dd ddd\0");
    data[0x30..0x38].copy_from_slice(&[0xD0, 0x07, 1, 0, 0, 0, 2, 0]);
    data[0x40] = 5;
    let (code, _) = run(
        "format",
        program(BASE, false, imports, &data, |a, l| {
            let f = |name: &str| l.import(names.iter().position(|n| *n == name).unwrap());
            a.xor(ebx, ebx)?;
            // "%1!d!-%2" with 5 and "x" from an array: "5-x".
            a.mov(dword_ptr(l.at(0x44)), l.at(0x10) as u32)?;
            a.push(l.at(0x40) as i32)?;
            a.push(0x40)?;
            a.push(l.at(0x60) as i32)?;
            a.push(0)?;
            a.push(0)?;
            a.push(l.at(0x00) as i32)?;
            a.push(0x2400)?;
            a.call(f("FormatMessageA"))?;
            a.cmp(eax, 3)?;
            a.sete(bl)?;
            a.cmp(dword_ptr(l.at(0x60)), 0x0078_2D35)?;
            a.sete(al)?;
            a.and(bl, al)?;
            // One locale, then TRUE. The call over the callback pushes its
            // address, the first argument.
            let mut enumerate = a.create_label();
            a.push(1)?;
            a.call(enumerate)?;
            // The callback: counts its calls and goes on.
            a.add(dword_ptr(l.at(0x48)), 1)?;
            a.mov(eax, 1)?;
            a.ret_1(4)?;
            a.set_label(&mut enumerate)?;
            a.call(f("EnumSystemLocalesA"))?;
            a.cmp(dword_ptr(l.at(0x48)), 1)?;
            a.sete(cl)?;
            a.cmp(eax, 1)?;
            a.sete(al)?;
            a.and(al, cl)?;
            a.shl(al, 1)?;
            a.or(bl, al)?;
            // 10 * 3 / 4 = 7.5, rounded up.
            a.push(4)?;
            a.push(3)?;
            a.push(10)?;
            a.call(f("MulDiv"))?;
            a.cmp(eax, 8)?;
            a.sete(al)?;
            a.shl(al, 2)?;
            a.or(bl, al)?;
            // "2000-01-02 Sun", the day of the week worked out.
            a.push(0x20)?;
            a.push(l.at(0xA0) as i32)?;
            a.push(l.at(0x20) as i32)?;
            a.push(l.at(0x30) as i32)?;
            a.push(0)?;
            a.push(0x409)?;
            a.call(f("GetDateFormatA"))?;
            a.cmp(eax, 15)?;
            a.sete(cl)?;
            a.cmp(dword_ptr(l.at(0xAB)), 0x006E_7553)?;
            a.sete(al)?;
            a.and(al, cl)?;
            a.shl(al, 3)?;
            a.or(bl, al)?;
            // The PIT's rate.
            a.push(l.at(0x50) as i32)?;
            a.call(f("QueryPerformanceFrequency"))?;
            a.cmp(dword_ptr(l.at(0x50)), 1_193_182)?;
            a.sete(al)?;
            a.shl(al, 4)?;
            a.or(bl, al)?;
            // The same but for case.
            a.push(l.at(0x18) as i32)?;
            a.push(l.at(0x14) as i32)?;
            a.call(f("lstrcmpiA"))?;
            a.test(eax, eax)?;
            a.sete(al)?;
            a.shl(al, 5)?;
            a.or(bl, al)?;
            a.mov(eax, ebx)?;
            a.ret()
        }),
    );
    assert_eq!(code, 0x3F, "the checks that failed: {:06b}", !code & 0x3F);
}

/// Data for a test program: strings and buffers, each at its offset.
#[derive(Default)]
struct Data(Vec<u8>);

impl Data {
    /// Add `bytes` on a four-byte boundary: their offset.
    fn add(&mut self, bytes: &[u8]) -> u32 {
        self.0.resize(self.0.len().next_multiple_of(4), 0);
        let at = self.0.len() as u32;
        self.0.extend_from_slice(bytes);
        at
    }

    /// Add `text` and a 0.
    fn text(&mut self, text: &str) -> u32 {
        let at = self.add(text.as_bytes());
        self.0.push(0);
        at
    }
}

#[test]
fn a_program_prints_with_msvcrt() {
    let names: &[&str] = &["printf", "exit"];
    let imports: &[(&str, &[&str])] = &[("MSVCRT.dll", names)];
    let format = "%d|%5i|%-5u|%x|%X|%o|%c|%s|%.2s|%08.3f|%e|%g|%p|%I64d|%hd|%+d|%#x|%%|%n\n";
    let mut d = Data::default();
    let format_at = d.text(format);
    let count_format = d.text("%d %.1f %s\n");
    let abc = d.text("abc");
    let xyz = d.text("xyz");
    let doubles = d.add(
        &[1.23456f64, 12345.678, 0.0001, -2.25]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<u8>>(),
    );
    let count = d.add(&[0; 4]);
    let exe = program(BASE, false, imports, &d.0, |a, l| {
        let f = |name: &str| l.import(names.iter().position(|n| *n == name).unwrap());
        let double = |a: &mut CodeAssembler, i: u32| -> Result<(), IcedError> {
            a.push(dword_ptr(l.at(doubles + 8 * i + 4)))?;
            a.push(dword_ptr(l.at(doubles + 8 * i)))
        };
        // Pushed last first.
        a.push(l.at(count) as i32)?;
        a.push(255)?;
        a.push(5)?;
        a.push(70000)?;
        a.push(1)?;
        a.push(0)?;
        a.push(0x1234)?;
        double(a, 2)?;
        double(a, 1)?;
        double(a, 0)?;
        a.push(l.at(xyz) as i32)?;
        a.push(l.at(abc) as i32)?;
        a.push(b'A' as i32)?;
        a.push(8)?;
        a.push(255)?;
        a.push(255)?;
        a.push(7)?;
        a.push(42)?;
        a.push(-42)?;
        a.push(l.at(format_at) as i32)?;
        a.call(f("printf"))?;
        a.add(esp, 4 * 22)?;
        a.push(l.at(abc) as i32)?;
        double(a, 3)?;
        a.push(dword_ptr(l.at(count)))?;
        a.push(l.at(count_format) as i32)?;
        a.call(f("printf"))?;
        a.add(esp, 20)?;
        a.push(3)?;
        a.call(f("exit"))?;
        a.int3()
    });
    let (code, text) = run("msvcrt_printf", exe);
    let line = "-42|   42|7    |ff|FF|10|A|abc|xy|0001.235|1.234568e+004|0.0001|\
                00001234|4294967296|4464|+5|0xff|%|";
    assert_eq!(text, format!("{}\r\n{} -2.3 abc\r\n", line, line.len()));
    assert_eq!(code, 3);
}

/// A width of 2^31 - 1 fills the buffer _snprintf has and no more, and
/// says it didn't fit.
#[test]
fn a_huge_printf_width_fills_only_the_buffer() {
    let names: &[&str] = &["_snprintf", "exit"];
    let imports: &[(&str, &[&str])] = &[("MSVCRT.dll", names)];
    let mut d = Data::default();
    let format = d.text("%*d");
    let buffer = d.add(&[b'x'; 12]);
    let exe = program(BASE, false, imports, &d.0, |a, l| {
        let f = |name: &str| l.import(names.iter().position(|n| *n == name).unwrap());
        a.push(1)?;
        a.push(0x7FFF_FFFF)?;
        a.push(l.at(format) as i32)?;
        a.push(8)?;
        a.push(l.at(buffer) as i32)?;
        a.call(f("_snprintf"))?;
        a.add(esp, 20)?;
        // -1, and the ninth byte untouched: 0xFF ^ 'x' ^ 'x'.
        a.movzx(ecx, byte_ptr(l.at(buffer + 8)))?;
        a.xor(eax, ecx)?;
        a.xor(eax, b'x' as i32)?;
        a.push(eax)?;
        a.call(f("exit"))?;
        a.int3()
    });
    let (code, text) = run("msvcrt_width", exe);
    assert_eq!(code, 0xFF, "{}", text);
}

#[test]
fn a_program_uses_msvcrt_files_heap_and_callbacks() {
    let names: &[&str] = &[
        "fopen",
        "fprintf",
        "fputs",
        "fclose",
        "fread",
        "fgets",
        "strcmp",
        "malloc",
        "realloc",
        "free",
        "_msize",
        "qsort",
        "memcmp",
        "atexit",
        "puts",
        "sscanf",
        "_setjmp3",
        "longjmp",
        "exit",
        "strtol",
        "_except_handler3",
        "_local_unwind2",
        "printf",
    ];
    let imports: &[(&str, &[&str])] =
        &[("MSVCRT.dll", names), ("KERNEL32.dll", &["RaiseException"])];
    let mut d = Data::default();
    let name = d.text("T.TXT");
    let w = d.text("w");
    let r = d.text("r");
    let rb = d.text("rb");
    let line_format = d.text("%s %d\n");
    let line = d.text("line");
    let second = d.text("second\n");
    let first_line = d.text("line 7\n");
    let bye = d.text("bye");
    let bits_format = d.text("%x\n");
    let scan_text = d.text("12 abc 0x1F");
    let scan_format = d.text("%d %s %x");
    let number = d.text("  -0x7fz");
    let numbers = d.add(
        &[5u32, 3, 9, 1, 7]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<u8>>(),
    );
    let sorted = d.add(
        &[1u32, 3, 5, 7, 9]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<u8>>(),
    );
    let buffer = d.add(&[0; 100]);
    let scanned = d.add(&[0; 16]);
    let jump = d.add(&[0; 64]);
    let scope = d.add(&[0; 24]);
    let bits = d.add(&[0; 4]);
    let end = d.add(&[0; 4]);
    let data_len = d.0.len() as u32;
    let exe = program(BASE, false, imports, &d.0, |a, l| {
        // The code is assembled once to find where the callbacks are.
        let code = |a: &mut CodeAssembler, at: [u32; 5]| -> Result<[CodeLabel; 5], IcedError> {
            let f = |name: &str| l.import(names.iter().position(|n| *n == name).unwrap());
            let mut compare = a.create_label();
            let mut at_exit = a.create_label();
            let mut filter = a.create_label();
            let mut handler = a.create_label();
            let mut finally = a.create_label();
            let mut after_jump = a.create_label();
            let mut after_try = a.create_label();
            let set = |a: &mut CodeAssembler, bit: u32| a.or(dword_ptr(l.at(bits)), bit as i32);
            // Write two lines, then read them back raw (CR LF on the disk)
            // and as text.
            a.push(l.at(w) as i32)?;
            a.push(l.at(name) as i32)?;
            a.call(f("fopen"))?;
            a.add(esp, 8)?;
            a.mov(esi, eax)?;
            a.push(7)?;
            a.push(l.at(line) as i32)?;
            a.push(l.at(line_format) as i32)?;
            a.push(esi)?;
            a.call(f("fprintf"))?;
            a.add(esp, 16)?;
            a.push(esi)?;
            a.push(l.at(second) as i32)?;
            a.call(f("fputs"))?;
            a.add(esp, 8)?;
            a.push(esi)?;
            a.call(f("fclose"))?;
            a.add(esp, 4)?;
            a.push(l.at(rb) as i32)?;
            a.push(l.at(name) as i32)?;
            a.call(f("fopen"))?;
            a.add(esp, 8)?;
            a.mov(esi, eax)?;
            a.push(esi)?;
            a.push(100)?;
            a.push(1)?;
            a.push(l.at(buffer) as i32)?;
            a.call(f("fread"))?;
            a.add(esp, 16)?;
            a.cmp(eax, 16)?;
            let mut skip = a.create_label();
            a.jne(skip)?;
            set(a, 1)?;
            a.set_label(&mut skip)?;
            a.push(esi)?;
            a.call(f("fclose"))?;
            a.add(esp, 4)?;
            a.push(l.at(r) as i32)?;
            a.push(l.at(name) as i32)?;
            a.call(f("fopen"))?;
            a.add(esp, 8)?;
            a.mov(esi, eax)?;
            a.push(esi)?;
            a.push(100)?;
            a.push(l.at(buffer) as i32)?;
            a.call(f("fgets"))?;
            a.add(esp, 12)?;
            a.push(l.at(first_line) as i32)?;
            a.push(l.at(buffer) as i32)?;
            a.call(f("strcmp"))?;
            a.add(esp, 8)?;
            let mut skip = a.create_label();
            a.test(eax, eax)?;
            a.jne(skip)?;
            set(a, 2)?;
            a.set_label(&mut skip)?;
            a.push(esi)?;
            a.call(f("fclose"))?;
            a.add(esp, 4)?;
            // The heap: a block grown keeps its size.
            a.push(16)?;
            a.call(f("malloc"))?;
            a.add(esp, 4)?;
            a.push(3000)?;
            a.push(eax)?;
            a.call(f("realloc"))?;
            a.add(esp, 8)?;
            a.mov(esi, eax)?;
            a.push(esi)?;
            a.call(f("_msize"))?;
            a.add(esp, 4)?;
            let mut skip = a.create_label();
            a.cmp(eax, 3000)?;
            a.jne(skip)?;
            set(a, 4)?;
            a.set_label(&mut skip)?;
            a.push(esi)?;
            a.call(f("free"))?;
            a.add(esp, 4)?;
            // qsort with the program's compare function.
            a.mov(eax, at[0])?;
            a.push(eax)?;
            a.push(4)?;
            a.push(5)?;
            a.push(l.at(numbers) as i32)?;
            a.call(f("qsort"))?;
            a.add(esp, 16)?;
            a.push(20)?;
            a.push(l.at(sorted) as i32)?;
            a.push(l.at(numbers) as i32)?;
            a.call(f("memcmp"))?;
            a.add(esp, 12)?;
            let mut skip = a.create_label();
            a.test(eax, eax)?;
            a.jne(skip)?;
            set(a, 8)?;
            a.set_label(&mut skip)?;
            // sscanf: three conversions.
            a.push(l.at(scanned + 4) as i32)?;
            a.push(l.at(scanned + 8) as i32)?;
            a.push(l.at(scanned) as i32)?;
            a.push(l.at(scan_format) as i32)?;
            a.push(l.at(scan_text) as i32)?;
            a.call(f("sscanf"))?;
            a.add(esp, 20)?;
            let mut skip = a.create_label();
            a.cmp(eax, 3)?;
            a.jne(skip)?;
            a.cmp(dword_ptr(l.at(scanned)), 12)?;
            a.jne(skip)?;
            a.cmp(dword_ptr(l.at(scanned + 4)), 0x1F)?;
            a.jne(skip)?;
            set(a, 0x10)?;
            a.set_label(&mut skip)?;
            // strtol: the end after the digits.
            a.push(16)?;
            a.push(l.at(end) as i32)?;
            a.push(l.at(number) as i32)?;
            a.call(f("strtol"))?;
            a.add(esp, 12)?;
            let mut skip = a.create_label();
            a.cmp(eax, -0x7F)?;
            a.jne(skip)?;
            a.cmp(dword_ptr(l.at(end)), l.at(number + 7) as i32)?;
            a.jne(skip)?;
            set(a, 0x20)?;
            a.set_label(&mut skip)?;
            // setjmp, then longjmp back to it with 5.
            a.push(0)?;
            a.push(l.at(jump) as i32)?;
            a.call(f("_setjmp3"))?;
            a.add(esp, 8)?;
            a.cmp(eax, 5)?;
            a.je(after_jump)?;
            a.push(5)?;
            a.push(l.at(jump) as i32)?;
            a.call(f("longjmp"))?;
            a.int3()?;
            a.set_label(&mut after_jump)?;
            set(a, 0x40)?;
            // __try with a filter and handler at level 0, a __finally at 1.
            a.mov(eax, at[2])?;
            a.mov(dword_ptr(l.at(scope + 4)), eax)?;
            a.mov(eax, at[3])?;
            a.mov(dword_ptr(l.at(scope + 8)), eax)?;
            a.mov(dword_ptr(l.at(scope)), -1)?;
            a.mov(dword_ptr(l.at(scope + 12)), 0)?;
            a.mov(dword_ptr(l.at(scope + 16)), 0)?;
            a.mov(eax, at[4])?;
            a.mov(dword_ptr(l.at(scope + 20)), eax)?;
            a.push(ebp)?;
            a.mov(ebp, esp)?;
            a.push(-1)?;
            a.push(l.at(scope) as i32)?;
            a.push(f("_except_handler3"))?;
            a.push(dword_ptr(0u64).fs())?;
            a.mov(dword_ptr(0u64).fs(), esp)?;
            a.sub(esp, 8)?;
            a.mov(dword_ptr(ebp - 0x18), esp)?;
            // __finally run by _local_unwind2, as for a return from it.
            a.mov(dword_ptr(ebp - 4), 1)?;
            a.lea(eax, dword_ptr(ebp - 0x10))?;
            a.push(0)?;
            a.push(eax)?;
            a.call(f("_local_unwind2"))?;
            a.add(esp, 8)?;
            a.cmp(dword_ptr(ebp - 4), 0)?;
            let mut skip = a.create_label();
            a.jne(skip)?;
            set(a, 0x80)?;
            a.set_label(&mut skip)?;
            a.push(0)?;
            a.push(0)?;
            a.push(0)?;
            a.push(0xE000_0001u32 as i32)?;
            a.call(dword_ptr(l.iat[names.len()] as u64))?;
            a.int3()?;
            a.set_label(&mut handler)?;
            a.mov(esp, dword_ptr(ebp - 0x18))?;
            a.cmp(dword_ptr(ebp - 4), -1)?;
            a.jne(after_try)?;
            set(a, 0x100)?;
            a.set_label(&mut after_try)?;
            a.mov(ecx, dword_ptr(ebp - 0x10))?;
            a.mov(dword_ptr(0u64).fs(), ecx)?;
            a.mov(esp, ebp)?;
            a.pop(ebp)?;
            // atexit's function runs at exit.
            a.mov(eax, at[1])?;
            a.push(eax)?;
            a.call(f("atexit"))?;
            a.add(esp, 4)?;
            a.push(dword_ptr(l.at(bits)))?;
            a.push(l.at(bits_format) as i32)?;
            a.call(f("printf"))?;
            a.add(esp, 8)?;
            a.push(0)?;
            a.call(f("exit"))?;
            a.int3()?;

            a.set_label(&mut compare)?;
            a.mov(eax, dword_ptr(esp + 4))?;
            a.mov(eax, dword_ptr(eax))?;
            a.mov(ecx, dword_ptr(esp + 8))?;
            a.sub(eax, dword_ptr(ecx))?;
            a.ret()?;

            a.set_label(&mut at_exit)?;
            a.push(l.at(bye) as i32)?;
            a.call(f("puts"))?;
            a.add(esp, 4)?;
            a.ret()?;

            // The filter: the handler for E0000001h, seen through the frame.
            a.set_label(&mut filter)?;
            a.mov(eax, dword_ptr(ebp - 0x14))?;
            a.mov(eax, dword_ptr(eax))?;
            a.cmp(dword_ptr(eax), 0xE000_0001u32 as i32)?;
            a.sete(al)?;
            a.movzx(eax, al)?;
            a.ret()?;

            a.set_label(&mut finally)?;
            a.cmp(dword_ptr(ebp - 4), 0)?;
            let mut skip = a.create_label();
            a.jne(skip)?;
            set(a, 0x200)?;
            a.set_label(&mut skip)?;
            a.ret()?;
            Ok([compare, at_exit, filter, handler, finally])
        };
        let labels = code(a, [0; 5])?;
        let ip = (l.data + data_len).next_multiple_of(0x1000) as u64;
        let options = iced_x86::BlockEncoderOptions::RETURN_NEW_INSTRUCTION_OFFSETS;
        let result = a.assemble_options(ip, options)?;
        let mut at = [0; 5];
        for (i, label) in labels.iter().enumerate() {
            at[i] = result.label_ip(label)? as u32;
        }
        a.reset();
        code(a, at)?;
        Ok(())
    });
    let (code, text) = run("msvcrt_files", exe);
    let file = PathBuf::from("target/test_win32/msvcrt_files/T.TXT");
    assert_eq!(fs::read(&file).unwrap(), b"line 7\r\nsecond\r\n");
    assert_eq!(text, "3ff\r\nbye\r\n");
    assert_eq!(code, 0);
}

#[test]
fn a_program_catches_its_cxx_exceptions() {
    let names: &[&str] = &["__CxxFrameHandler", "_CxxThrowException", "printf", "exit"];
    let imports: &[(&str, &[&str])] = &[("MSVCRT.dll", names)];
    let mut d = Data::default();
    let boom = d.text("boom");
    let bits_format = d.text("%x\n");
    // A char * thrown: its type, the type it can be caught as, the list
    // of those and the ThrowInfo.
    let char_pointer = d.add(&[0; 8]);
    d.text(".PAD");
    let catchable = d.add(&[0; 28]);
    let list = d.add(&[0; 8]);
    let throw_info = d.add(&[0; 16]);
    let thrown = d.add(&[0; 4]);
    // The outer function: a try (state 0) with a catch (char *) into
    // EBP - 14h. The inner one: an object (state 0), destroyed by
    // `destroy`, and a try (state 1) with a catch (...) that throws the
    // exception on with `throw;`.
    let outer = d.add(&[0; 28]);
    let outer_unwind = d.add(&[0; 16]);
    let outer_try = d.add(&[0; 20]);
    let outer_handler = d.add(&[0; 16]);
    let inner = d.add(&[0; 28]);
    let inner_unwind = d.add(&[0; 24]);
    let inner_try = d.add(&[0; 20]);
    let inner_handler = d.add(&[0; 16]);
    let destroyed = d.add(&[0; 4]);
    let bits = d.add(&[0; 4]);
    let outer_esp = d.add(&[0; 4]);
    let data_len = d.0.len() as u32;
    let exe = program(BASE, false, imports, &d.0, |a, l| {
        let code = |a: &mut CodeAssembler, at: [u32; 6]| -> Result<[CodeLabel; 6], IcedError> {
            let f = |name: &str| l.import(names.iter().position(|n| *n == name).unwrap());
            let mut outer_thunk = a.create_label();
            let mut inner_thunk = a.create_label();
            let mut outer_catch = a.create_label();
            let mut inner_catch = a.create_label();
            let mut destroy = a.create_label();
            let mut after = a.create_label();
            let mut inner_function = a.create_label();
            let mut thrower = a.create_label();
            let set = |a: &mut CodeAssembler, bit: u32| a.or(dword_ptr(l.at(bits)), bit as i32);
            let put = |a: &mut CodeAssembler, offset: u32, value: u32| {
                a.mov(dword_ptr(l.at(offset)), value as i32)
            };
            let data = |offset: u32| l.at(offset) as u32;
            // The tables.
            put(a, catchable, 1)?;
            put(a, catchable + 4, data(char_pointer))?;
            put(a, catchable + 12, u32::MAX)?;
            put(a, catchable + 20, 4)?;
            put(a, list, 1)?;
            put(a, list + 4, data(catchable))?;
            put(a, throw_info + 12, data(list))?;
            for (info, states, unwind, entry) in
                [(outer, 2, outer_unwind, outer_try), (inner, 3, inner_unwind, inner_try)]
            {
                put(a, info, 0x1993_0520)?;
                put(a, info + 4, states)?;
                put(a, info + 8, data(unwind))?;
                put(a, info + 12, 1)?;
                put(a, info + 16, data(entry))?;
            }
            put(a, outer_unwind, u32::MAX)?;
            put(a, outer_unwind + 8, u32::MAX)?;
            put(a, outer_try + 8, 1)?;
            put(a, outer_try + 12, 1)?;
            put(a, outer_try + 16, data(outer_handler))?;
            put(a, outer_handler + 4, data(char_pointer))?;
            put(a, outer_handler + 8, -0x14i32 as u32)?;
            put(a, outer_handler + 12, at[2])?;
            put(a, inner_unwind, u32::MAX)?;
            put(a, inner_unwind + 4, at[4])?;
            put(a, inner_try, 1)?;
            put(a, inner_try + 4, 1)?;
            put(a, inner_try + 8, 2)?;
            put(a, inner_try + 12, 1)?;
            put(a, inner_try + 16, data(inner_handler))?;
            put(a, inner_handler + 12, at[3])?;

            // The outer function's frame, as MSVC makes it.
            a.push(ebp)?;
            a.mov(ebp, esp)?;
            a.push(-1)?;
            a.mov(eax, at[0])?;
            a.push(eax)?;
            a.push(dword_ptr(0u64).fs())?;
            a.mov(dword_ptr(0u64).fs(), esp)?;
            a.sub(esp, 0x10)?;
            a.mov(dword_ptr(ebp - 0x10), esp)?;
            a.mov(dword_ptr(l.at(outer_esp)), esp)?;
            a.mov(dword_ptr(ebp - 4), 0)?;
            a.call(inner_function)?;
            a.int3()?;

            // catch (char *p): p is what was thrown. A try in the catch
            // block would save the block's ESP as the function's.
            a.set_label(&mut outer_catch)?;
            a.mov(dword_ptr(ebp - 0x10), esp)?;
            a.cmp(dword_ptr(ebp - 0x14), data(boom) as i32)?;
            let mut skip = a.create_label();
            a.jne(skip)?;
            set(a, 4)?;
            a.set_label(&mut skip)?;
            a.mov(eax, at[5])?;
            a.ret()?;

            // On after the try, with the function's stack from before
            // the catch, the inner object destroyed once.
            a.set_label(&mut after)?;
            let mut skip = a.create_label();
            a.cmp(esp, dword_ptr(l.at(outer_esp)))?;
            a.jne(skip)?;
            a.cmp(esp, dword_ptr(ebp - 0x10))?;
            a.jne(skip)?;
            set(a, 8)?;
            a.set_label(&mut skip)?;
            let mut skip = a.create_label();
            a.cmp(dword_ptr(l.at(destroyed)), 1)?;
            a.jne(skip)?;
            set(a, 2)?;
            a.set_label(&mut skip)?;
            a.mov(ecx, dword_ptr(ebp - 0x0C))?;
            a.mov(dword_ptr(0u64).fs(), ecx)?;
            a.mov(esp, ebp)?;
            a.pop(ebp)?;
            a.push(dword_ptr(l.at(bits)))?;
            a.push(l.at(bits_format) as i32)?;
            a.call(f("printf"))?;
            a.add(esp, 8)?;
            a.push(0)?;
            a.call(f("exit"))?;
            a.int3()?;

            a.set_label(&mut inner_function)?;
            a.push(ebp)?;
            a.mov(ebp, esp)?;
            a.push(-1)?;
            a.mov(eax, at[1])?;
            a.push(eax)?;
            a.push(dword_ptr(0u64).fs())?;
            a.mov(dword_ptr(0u64).fs(), esp)?;
            a.sub(esp, 0x10)?;
            a.mov(dword_ptr(ebp - 0x10), esp)?;
            a.mov(dword_ptr(ebp - 4), 0)?;
            a.mov(dword_ptr(ebp - 4), 1)?;
            a.call(thrower)?;
            a.int3()?;

            // catch (...): throw;
            a.set_label(&mut inner_catch)?;
            set(a, 1)?;
            a.push(0)?;
            a.push(0)?;
            a.call(f("_CxxThrowException"))?;
            a.int3()?;

            a.set_label(&mut destroy)?;
            a.inc(dword_ptr(l.at(destroyed)))?;
            a.ret()?;

            // throw "boom";
            a.set_label(&mut thrower)?;
            put(a, thrown, data(boom))?;
            a.push(data(throw_info) as i32)?;
            a.push(data(thrown) as i32)?;
            a.call(f("_CxxThrowException"))?;
            a.int3()?;

            a.set_label(&mut outer_thunk)?;
            a.mov(eax, data(outer))?;
            a.jmp(f("__CxxFrameHandler"))?;
            a.set_label(&mut inner_thunk)?;
            a.mov(eax, data(inner))?;
            a.jmp(f("__CxxFrameHandler"))?;
            Ok([outer_thunk, inner_thunk, outer_catch, inner_catch, destroy, after])
        };
        let labels = code(a, [0; 6])?;
        let ip = (l.data + data_len).next_multiple_of(0x1000) as u64;
        let options = iced_x86::BlockEncoderOptions::RETURN_NEW_INSTRUCTION_OFFSETS;
        let result = a.assemble_options(ip, options)?;
        let mut at = [0; 6];
        for (i, label) in labels.iter().enumerate() {
            at[i] = result.label_ip(label)? as u32;
        }
        a.reset();
        code(a, at)?;
        Ok(())
    });
    let (code, text) = run("msvcrt_cxx", exe);
    assert_eq!(text, "f\r\n");
    assert_eq!(code, 0);
}

/// A drive's current directory as MSVCRT's _chdir keeps it, in a variable
/// whose name starts with "=", set and read back; a name with "=" further
/// on is refused with ERROR_INVALID_PARAMETER. A bit each in the exit
/// code.
#[test]
fn a_variable_name_can_start_with_an_equals_sign() {
    let names: &[&str] = &[
        "SetEnvironmentVariableA",
        "GetEnvironmentVariableA",
        "GetLastError",
    ];
    let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", names)];
    let mut data = vec![0u8; 0x40];
    data[..4].copy_from_slice(b"=D:\0");
    data[4..8].copy_from_slice(b"D:\\\0");
    data[8..12].copy_from_slice(b"A=B\0");
    let exe = program(BASE, false, imports, &data, |a, l| {
        a.xor(ebx, ebx)?;
        // SetEnvironmentVariableA("=D:", "D:\") succeeds.
        a.push(l.at(4) as i32)?;
        a.push(l.at(0) as i32)?;
        a.call(l.import(0))?;
        a.test(eax, eax)?;
        a.setne(bl)?;
        // GetEnvironmentVariableA("=D:") gives "D:\", 3 long.
        a.push(0x20)?;
        a.push(l.at(0x20) as i32)?;
        a.push(l.at(0) as i32)?;
        a.call(l.import(1))?;
        a.cmp(eax, 3)?;
        a.sete(al)?;
        a.shl(al, 1)?;
        a.or(bl, al)?;
        a.cmp(dword_ptr(l.at(0x20)), 0x005C_3A44)?;
        a.sete(al)?;
        a.shl(al, 2)?;
        a.or(bl, al)?;
        // SetEnvironmentVariableA("A=B", "D:\") fails.
        a.push(l.at(4) as i32)?;
        a.push(l.at(8) as i32)?;
        a.call(l.import(0))?;
        a.test(eax, eax)?;
        a.sete(al)?;
        a.shl(al, 3)?;
        a.or(bl, al)?;
        a.call(l.import(2))?;
        a.cmp(eax, 87)?;
        a.sete(al)?;
        a.shl(al, 4)?;
        a.or(bl, al)?;
        a.movzx(eax, bl)?;
        a.ret()
    });
    assert_eq!(run("drive_variable", exe).0, 0b1_1111);
}

/// A child given the environment "=D:=D:\" and "A=1" finds only "A=1" in
/// the C runtime's: MSVCRT leaves out the strings whose name starts with
/// "=". Its exit code is the count, with 0x10 if the one it found is A's.
#[test]
fn the_c_runtime_leaves_drive_variables_out_of_its_environment() {
    let child = {
        let imports: &[(&str, &[&str])] = &[("MSVCRT.dll", &["__getmainargs", "exit"])];
        // argc, argv, env and the startup info.
        program(BASE, false, imports, &[0; 0x20], |a, l| {
            a.push(l.at(0x10) as i32)?;
            a.push(0)?;
            a.push(l.at(8) as i32)?;
            a.push(l.at(4) as i32)?;
            a.push(l.at(0) as i32)?;
            a.call(l.import(0))?;
            a.add(esp, 20)?;
            a.mov(esi, dword_ptr(l.at(8)))?;
            a.xor(ebx, ebx)?;
            let mut count = a.create_label();
            let mut counted = a.create_label();
            a.set_label(&mut count)?;
            a.cmp(dword_ptr(esi + ebx * 4), 0)?;
            a.je(counted)?;
            a.inc(ebx)?;
            a.jmp(count)?;
            a.set_label(&mut counted)?;
            a.mov(eax, dword_ptr(esi))?;
            a.cmp(byte_ptr(eax), b'A' as i32)?;
            a.sete(al)?;
            a.shl(al, 4)?;
            a.or(bl, al)?;
            a.push(ebx)?;
            a.call(l.import(1))?;
            a.int3()
        })
    };
    let names: &[&str] = &[
        "CreateProcessA",
        "WaitForSingleObject",
        "GetExitCodeProcess",
        "ExitProcess",
    ];
    let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", names)];
    // The command line, the environment block at 0x10, STARTUPINFO at
    // 0x40, PROCESS_INFORMATION at 0x90, the exit code at 0xA0.
    let mut data = b"C.EXE\0".to_vec();
    data.resize(0xB0, 0);
    data[0x10..0x1D].copy_from_slice(b"=D:=D:\\\0A=1\0\0");
    data[0x40] = 68;
    let parent = program(BASE, false, imports, &data, |a, l| {
        let f = |name: &str| l.import(names.iter().position(|n| *n == name).unwrap());
        a.push(l.at(0x90) as i32)?;
        a.push(l.at(0x40) as i32)?;
        a.push(0)?;
        a.push(l.at(0x10) as i32)?;
        for _ in 0..4 {
            a.push(0)?;
        }
        a.push(l.at(0) as i32)?;
        a.push(0)?;
        a.call(f("CreateProcessA"))?;
        a.push(-1)?;
        a.push(dword_ptr(l.at(0x90)))?;
        a.call(f("WaitForSingleObject"))?;
        a.push(l.at(0xA0) as i32)?;
        a.push(dword_ptr(l.at(0x90)))?;
        a.call(f("GetExitCodeProcess"))?;
        a.push(dword_ptr(l.at(0xA0)))?;
        a.call(f("ExitProcess"))?;
        a.int3()
    });
    let mut cpu = Cpu::new(scratch(
        "drive_variables_environ",
        &[("T.EXE", parent), ("C.EXE", child)],
    ));
    cpu.bus.win32.enabled = true;
    cpu.host_console = Some(HostConsole::default());
    cpu.load_shell();
    assert!(cpu.load_executable("T.EXE", None));
    assert!(run_to_exit(&mut cpu, 200), "{}", printed(&cpu));
    assert_eq!(cpu.errorlevel, 0x11, "{}", printed(&cpu));
}

const GENERIC_WRITE: i32 = 0x4000_0000;
const CREATE_ALWAYS: i32 = 2;
const INFINITE: i32 = -1;
const WAIT_TIMEOUT: i32 = 0x102;
const WAIT_FAILED: i32 = -1;
const WM_CLOSE: i32 = 0x0010;
const WS_POPUP: i32 = 0x8000_0000u32 as i32;

/// A program for `subsystem` importing `kernel32` from KERNEL32 and
/// `user32` from USER32, with `data` and the code `f` makes, which is
/// given the import address table slot of a function by its name.
fn win_program(
    subsystem: u16,
    kernel32: &[&str],
    user32: &[&str],
    data: &[u8],
    f: impl FnOnce(
        &mut CodeAssembler,
        &Layout,
        &dyn Fn(&str) -> AsmMemoryOperand,
    ) -> Result<(), IcedError>,
) -> Vec<u8> {
    let mut imports: Vec<(&str, &[&str])> = vec![("KERNEL32.DLL", kernel32)];
    if !user32.is_empty() {
        imports.push(("USER32.DLL", user32));
    }
    let names: Vec<&str> = kernel32.iter().chain(user32).copied().collect();
    program_for(subsystem, BASE, false, &imports, data, |a, l| {
        let slot = |name: &str| l.import(names.iter().position(|n| *n == name).unwrap());
        f(a, l, &slot)
    })
}

/// A GUI program that works for 300 ms, writes the file `name` and only
/// then gets messages, from a timer whose first one ends it: until then
/// it isn't idle.
fn works_then_waits_for_input(name: &str) -> Vec<u8> {
    let kernel32 = ["Sleep", "CreateFileA", "CloseHandle", "ExitProcess"];
    let user32 = ["SetTimer", "GetMessageA"];
    // The file's name, then a MSG at 0x20.
    let mut data = name.as_bytes().to_vec();
    data.resize(0x40, 0);
    win_program(pe::SUBSYSTEM_GUI, &kernel32, &user32, &data, |a, l, f| {
        a.push(300)?;
        a.call(f("Sleep"))?;
        push_args(
            a,
            &[l.at(0) as i32, GENERIC_WRITE, 0, 0, CREATE_ALWAYS, 0, 0],
        )?;
        a.call(f("CreateFileA"))?;
        a.push(eax)?;
        a.call(f("CloseHandle"))?;
        push_args(a, &[0, 0, 100, 0])?;
        a.call(f("SetTimer"))?;
        push_args(a, &[l.at(0x20) as i32, 0, 0, 0])?;
        a.call(f("GetMessageA"))?;
        a.push(0)?;
        a.call(f("ExitProcess"))?;
        a.int3()
    })
}

/// A program for `subsystem` that never gets messages: it sleeps until
/// it is ended.
fn never_waits_for_input(subsystem: u16) -> Vec<u8> {
    win_program(subsystem, &["Sleep"], &[], &[0; 4], |a, _, f| {
        a.push(INFINITE)?;
        a.call(f("Sleep"))?;
        a.int3()
    })
}

/// A console program that works for 300 ms, writes the file `name` and
/// ends.
fn console_writes(name: &str) -> Vec<u8> {
    let kernel32 = ["Sleep", "CreateFileA", "CloseHandle", "ExitProcess"];
    let mut data = name.as_bytes().to_vec();
    data.resize(0x20, 0);
    win_program(pe::SUBSYSTEM_CONSOLE, &kernel32, &[], &data, |a, l, f| {
        a.push(300)?;
        a.call(f("Sleep"))?;
        push_args(
            a,
            &[l.at(0) as i32, GENERIC_WRITE, 0, 0, CREATE_ALWAYS, 0, 0],
        )?;
        a.call(f("CreateFileA"))?;
        a.push(eax)?;
        a.call(f("CloseHandle"))?;
        a.push(0)?;
        a.call(f("ExitProcess"))?;
        a.int3()
    })
}

/// Run T.EXE, which starts the other programs of `files`, to its end:
/// its exit code.
fn run_family(test: &str, files: &[(&str, Vec<u8>)]) -> u8 {
    let mut cpu = Cpu::new(scratch(test, files));
    cpu.bus.win32.enabled = true;
    cpu.host_console = Some(HostConsole::default());
    cpu.load_shell();
    assert!(cpu.load_executable("T.EXE", None));
    assert!(run_to_exit(&mut cpu, 400), "{}", printed(&cpu));
    cpu.errorlevel
}

/// Set bit `bit` of EBX if the condition `cc` sets AL.
fn bit_if(
    a: &mut CodeAssembler,
    cc: fn(&mut CodeAssembler, AsmRegister8) -> Result<(), IcedError>,
    bit: u32,
) -> Result<(), IcedError> {
    cc(a, al)?;
    a.movzx(eax, al)?;
    a.shl(eax, bit)?;
    a.or(ebx, eax)
}

fn eq(a: &mut CodeAssembler, r: AsmRegister8) -> Result<(), IcedError> {
    a.sete(r)
}

fn ne(a: &mut CodeAssembler, r: AsmRegister8) -> Result<(), IcedError> {
    a.setne(r)
}

/// CreateProcessA of the command line at `line` of the data, with the
/// STARTUPINFO at 0x40 and the PROCESS_INFORMATION at 0x90.
fn create_child(
    a: &mut CodeAssembler,
    l: &Layout,
    f: &dyn Fn(&str) -> AsmMemoryOperand,
    line: u32,
) -> Result<(), IcedError> {
    a.push(l.at(0x90) as i32)?;
    a.push(l.at(0x40) as i32)?;
    for _ in 0..6 {
        a.push(0)?;
    }
    a.push(l.at(line) as i32)?;
    a.push(0)?;
    a.call(f("CreateProcessA"))
}

/// WaitForInputIdle of the process at 0x90 for `ms`.
fn idle(
    a: &mut CodeAssembler,
    l: &Layout,
    f: &dyn Fn(&str) -> AsmMemoryOperand,
    ms: i32,
) -> Result<(), IcedError> {
    a.push(ms)?;
    a.push(dword_ptr(l.at(0x90)))?;
    a.call(f("WaitForInputIdle"))
}

/// Data with `texts` at their offsets, a STARTUPINFO at 0x40 and room
/// for a PROCESS_INFORMATION at 0x90 and more up to 0x100.
fn child_data(texts: &[(usize, &str)]) -> Vec<u8> {
    let mut data = Vec::new();
    for &(at, text) in texts {
        data.resize(at, 0);
        data.extend_from_slice(text.as_bytes());
    }
    data.resize(0x100, 0);
    data[0x40] = 68;
    data
}

#[test]
fn win_exec_returns_once_the_program_waits_for_input() {
    // C.EXE writes C.DAT before it gets messages, so it is there when
    // WinExec returns. D.EXE is a console program, which WinExec doesn't
    // wait for: D.DAT isn't there yet.
    let kernel32 = ["WinExec", "GetFileAttributesA", "Sleep", "ExitProcess"];
    let user32 = ["WaitForInputIdle"];
    let data = child_data(&[
        (0, "C.EXE"),
        (0x10, "C.DAT"),
        (0x20, "D.EXE"),
        (0x30, "D.DAT"),
    ]);
    let parent = win_program(
        pe::SUBSYSTEM_CONSOLE,
        &kernel32,
        &user32,
        &data,
        |a, l, f| {
            a.xor(ebx, ebx)?;
            push_args(a, &[l.at(0) as i32, 1])?;
            a.call(f("WinExec"))?;
            a.cmp(eax, 33)?;
            bit_if(a, eq, 0)?;
            a.push(l.at(0x10) as i32)?;
            a.call(f("GetFileAttributesA"))?;
            a.cmp(eax, -1)?;
            bit_if(a, ne, 1)?;
            push_args(a, &[l.at(0x20) as i32, 1])?;
            a.call(f("WinExec"))?;
            a.cmp(eax, 33)?;
            bit_if(a, eq, 2)?;
            a.push(l.at(0x30) as i32)?;
            a.call(f("GetFileAttributesA"))?;
            a.cmp(eax, -1)?;
            bit_if(a, eq, 3)?;
            // The children end before the parent.
            a.push(1000)?;
            a.call(f("Sleep"))?;
            a.push(ebx)?;
            a.call(f("ExitProcess"))?;
            a.int3()
        },
    );
    let code = run_family(
        "win_exec_idle",
        &[
            ("T.EXE", parent),
            ("C.EXE", works_then_waits_for_input("C.DAT")),
            ("D.EXE", console_writes("D.DAT")),
        ],
    );
    assert_eq!(code, 0xF, "the checks that failed: {:04b}", !code & 0xF);
}

#[test]
fn wait_for_input_idle_waits_for_the_program_or_times_out() {
    let kernel32 = [
        "CreateProcessA",
        "TerminateProcess",
        "WaitForSingleObject",
        "GetFileAttributesA",
        "GetTickCount",
        "ExitProcess",
    ];
    let user32 = ["WaitForInputIdle"];
    // The command lines and A.DAT; the time a wait started at 0xA0.
    let texts = [
        (0, "A.EXE"),
        (0x10, "B.EXE"),
        (0x20, "E.EXE"),
        (0x30, "A.DAT"),
    ];
    let data = child_data(&texts);
    let parent = win_program(
        pe::SUBSYSTEM_CONSOLE,
        &kernel32,
        &user32,
        &data,
        |a, l, f| {
            let started = |a: &mut CodeAssembler| {
                a.call(f("GetTickCount"))?;
                a.mov(dword_ptr(l.at(0xA0)), eax)
            };
            let elapsed = |a: &mut CodeAssembler| {
                a.call(f("GetTickCount"))?;
                a.sub(eax, dword_ptr(l.at(0xA0)))
            };
            let terminate = |a: &mut CodeAssembler| {
                a.push(0)?;
                a.push(dword_ptr(l.at(0x90)))?;
                a.call(f("TerminateProcess"))
            };
            a.xor(ebx, ebx)?;
            // A.EXE: 0 once it waits for input, by when it wrote
            // A.DAT; once it ended, WAIT_FAILED.
            create_child(a, l, f, 0)?;
            idle(a, l, f, 5000)?;
            a.test(eax, eax)?;
            bit_if(a, eq, 0)?;
            a.push(l.at(0x30) as i32)?;
            a.call(f("GetFileAttributesA"))?;
            a.cmp(eax, -1)?;
            bit_if(a, ne, 1)?;
            a.push(INFINITE)?;
            a.push(dword_ptr(l.at(0x90)))?;
            a.call(f("WaitForSingleObject"))?;
            idle(a, l, f, 100)?;
            a.cmp(eax, WAIT_FAILED)?;
            bit_if(a, eq, 2)?;
            // B.EXE never gets messages: WAIT_TIMEOUT once the 200 ms
            // passed.
            create_child(a, l, f, 0x10)?;
            started(a)?;
            idle(a, l, f, 200)?;
            a.cmp(eax, WAIT_TIMEOUT)?;
            bit_if(a, eq, 3)?;
            elapsed(a)?;
            a.cmp(eax, 200)?;
            bit_if(a, |a, r| a.setae(r), 4)?;
            terminate(a)?;
            // E.EXE is a console program: WAIT_FAILED at once.
            create_child(a, l, f, 0x20)?;
            started(a)?;
            idle(a, l, f, 5000)?;
            a.cmp(eax, WAIT_FAILED)?;
            bit_if(a, eq, 5)?;
            elapsed(a)?;
            a.cmp(eax, 100)?;
            bit_if(a, |a, r| a.setb(r), 6)?;
            terminate(a)?;
            // A handle that isn't one: WAIT_FAILED.
            push_args(a, &[0x7FF0, 100])?;
            a.call(f("WaitForInputIdle"))?;
            a.cmp(eax, WAIT_FAILED)?;
            bit_if(a, eq, 7)?;
            a.push(ebx)?;
            a.call(f("ExitProcess"))?;
            a.int3()
        },
    );
    let code = run_family(
        "wait_for_input_idle",
        &[
            ("T.EXE", parent),
            ("A.EXE", works_then_waits_for_input("A.DAT")),
            ("B.EXE", never_waits_for_input(pe::SUBSYSTEM_GUI)),
            ("E.EXE", never_waits_for_input(pe::SUBSYSTEM_CONSOLE)),
        ],
    );
    assert_eq!(code, 0xFF, "the checks that failed: {:08b}", !code);
}

#[test]
fn wait_for_input_idle_handles_messages_sent_to_the_waiting_thread() {
    // S.EXE sends WM_CLOSE to the parent's window before it gets
    // messages of its own. The parent handles it while it waits, as on
    // Windows, and the window goes; else both would wait until the
    // time-out.
    let child = {
        let kernel32 = ["ExitProcess"];
        let user32 = ["FindWindowA", "SendMessageA", "SetTimer", "GetMessageA"];
        let data = child_data(&[(0, "Waiting")]);
        win_program(pe::SUBSYSTEM_GUI, &kernel32, &user32, &data, |a, l, f| {
            push_args(a, &[0, l.at(0) as i32])?;
            a.call(f("FindWindowA"))?;
            push_args(a, &[WM_CLOSE, 0, 0])?;
            a.push(eax)?;
            a.call(f("SendMessageA"))?;
            push_args(a, &[0, 0, 100, 0])?;
            a.call(f("SetTimer"))?;
            push_args(a, &[l.at(0x90) as i32, 0, 0, 0])?;
            a.call(f("GetMessageA"))?;
            a.push(0)?;
            a.call(f("ExitProcess"))?;
            a.int3()
        })
    };
    let kernel32 = [
        "CreateProcessA",
        "WaitForSingleObject",
        "GetTickCount",
        "ExitProcess",
    ];
    let user32 = ["WaitForInputIdle", "CreateWindowExA", "IsWindow"];
    // The time the wait started at 0xA0.
    let data = child_data(&[(0, "S.EXE"), (0x10, "Static"), (0x20, "Waiting")]);
    let parent = win_program(pe::SUBSYSTEM_GUI, &kernel32, &user32, &data, |a, l, f| {
        let (class, title) = (l.at(0x10) as i32, l.at(0x20) as i32);
        push_args(a, &[0, class, title, WS_POPUP, 0, 0, 10, 10, 0, 0, 0, 0])?;
        a.call(f("CreateWindowExA"))?;
        a.mov(esi, eax)?;
        a.xor(ebx, ebx)?;
        create_child(a, l, f, 0)?;
        a.call(f("GetTickCount"))?;
        a.mov(dword_ptr(l.at(0xA0)), eax)?;
        idle(a, l, f, 10000)?;
        a.test(eax, eax)?;
        bit_if(a, eq, 0)?;
        a.call(f("GetTickCount"))?;
        a.sub(eax, dword_ptr(l.at(0xA0)))?;
        a.cmp(eax, 5000)?;
        bit_if(a, |a, r| a.setb(r), 1)?;
        a.push(esi)?;
        a.call(f("IsWindow"))?;
        a.test(eax, eax)?;
        bit_if(a, eq, 2)?;
        a.push(INFINITE)?;
        a.push(dword_ptr(l.at(0x90)))?;
        a.call(f("WaitForSingleObject"))?;
        a.push(ebx)?;
        a.call(f("ExitProcess"))?;
        a.int3()
    });
    let code = run_family(
        "wait_for_input_idle_sent",
        &[("T.EXE", parent), ("S.EXE", child)],
    );
    assert_eq!(code, 0x7, "the checks that failed: {:03b}", !code & 0x7);
}

const WS_VISIBLE: i32 = 0x1000_0000;
const PM_REMOVE: i32 = 1;

/// A GUI program that shows a popup of class Static titled `title`, at
/// (`x`, 0) and 50 pixels square, and then gets and dispatches messages
/// until it is ended.
fn shows_a_window(title: &str, x: i32) -> Vec<u8> {
    let user32 = ["CreateWindowExA", "GetMessageA", "DispatchMessageA"];
    // The title, the class at 0x10 and a MSG at 0x20.
    let data = child_data(&[(0, title), (0x10, "Static")]);
    win_program(pe::SUBSYSTEM_GUI, &[], &user32, &data, |a, l, f| {
        let (class, title) = (l.at(0x10) as i32, l.at(0) as i32);
        let style = WS_POPUP | WS_VISIBLE;
        push_args(a, &[0, class, title, style, x, 0, 50, 50, 0, 0, 0, 0])?;
        a.call(f("CreateWindowExA"))?;
        let mut next = a.create_label();
        a.set_label(&mut next)?;
        push_args(a, &[l.at(0x20) as i32, 0, 0, 0])?;
        a.call(f("GetMessageA"))?;
        a.push(l.at(0x20) as i32)?;
        a.call(f("DispatchMessageA"))?;
        a.jmp(next)
    })
}

/// Take and dispatch the running thread's messages, with the MSG at
/// `msg` of the data, until there are none.
fn pump(
    a: &mut CodeAssembler,
    l: &Layout,
    f: &dyn Fn(&str) -> AsmMemoryOperand,
    msg: u32,
) -> Result<(), IcedError> {
    let mut next = a.create_label();
    let mut done = a.create_label();
    a.set_label(&mut next)?;
    push_args(a, &[l.at(msg) as i32, 0, 0, 0, PM_REMOVE])?;
    a.call(f("PeekMessageA"))?;
    a.test(eax, eax)?;
    a.je(done)?;
    a.push(l.at(msg) as i32)?;
    a.call(f("DispatchMessageA"))?;
    a.jmp(next)?;
    a.set_label(&mut done)?;
    a.nop()
}

#[test]
fn a_program_started_from_the_foreground_comes_to_it_and_stays() {
    // T.EXE shows a window and starts C.EXE, whose window comes to the
    // foreground: on Windows 98 a program the foreground program starts
    // may take it. Once T.EXE has looked at its messages, its queue has
    // no active window. Its SetActiveWindow then makes its window its
    // queue's active one behind C.EXE's, not raised over it, and its
    // SetForegroundWindow fails. (MechCommander's setup hid DirectPlay's
    // license dialog behind its full-screen window this way,
    // kibertoad/rust-dos#352.)
    let kernel32 = ["CreateProcessA", "TerminateProcess", "ExitProcess"];
    let user32 = [
        "WaitForInputIdle",
        "CreateWindowExA",
        "PeekMessageA",
        "DispatchMessageA",
        "GetForegroundWindow",
        "GetActiveWindow",
        "SetActiveWindow",
        "SetForegroundWindow",
        "GetTopWindow",
    ];
    // A MSG at 0xC0.
    let data = child_data(&[(0, "C.EXE"), (0x10, "Static"), (0x20, "Parent")]);
    let parent = win_program(pe::SUBSYSTEM_GUI, &kernel32, &user32, &data, |a, l, f| {
        let (class, title) = (l.at(0x10) as i32, l.at(0x20) as i32);
        let style = WS_POPUP | WS_VISIBLE;
        push_args(a, &[0, class, title, style, 100, 0, 50, 50, 0, 0, 0, 0])?;
        a.call(f("CreateWindowExA"))?;
        a.mov(esi, eax)?;
        a.xor(ebx, ebx)?;
        a.call(f("GetForegroundWindow"))?;
        a.cmp(eax, esi)?;
        bit_if(a, eq, 0)?;
        create_child(a, l, f, 0)?;
        idle(a, l, f, 5000)?;
        pump(a, l, f, 0xC0)?;
        a.call(f("GetForegroundWindow"))?;
        a.mov(edi, eax)?;
        a.cmp(edi, esi)?;
        bit_if(a, ne, 1)?;
        a.call(f("GetActiveWindow"))?;
        a.test(eax, eax)?;
        bit_if(a, eq, 2)?;
        a.push(esi)?;
        a.call(f("SetActiveWindow"))?;
        a.call(f("GetActiveWindow"))?;
        a.cmp(eax, esi)?;
        bit_if(a, eq, 3)?;
        a.call(f("GetForegroundWindow"))?;
        a.cmp(eax, edi)?;
        bit_if(a, eq, 4)?;
        a.push(0)?;
        a.call(f("GetTopWindow"))?;
        a.cmp(eax, edi)?;
        bit_if(a, eq, 5)?;
        a.push(esi)?;
        a.call(f("SetForegroundWindow"))?;
        a.test(eax, eax)?;
        bit_if(a, eq, 6)?;
        a.call(f("GetForegroundWindow"))?;
        a.cmp(eax, edi)?;
        bit_if(a, eq, 7)?;
        a.push(0)?;
        a.push(dword_ptr(l.at(0x90)))?;
        a.call(f("TerminateProcess"))?;
        a.push(ebx)?;
        a.call(f("ExitProcess"))?;
        a.int3()
    });
    let code = run_family(
        "foreground_from_parent",
        &[("T.EXE", parent), ("C.EXE", shows_a_window("Child", 0))],
    );
    assert_eq!(code, 0xFF, "the checks that failed: {:08b}", !code);
}

#[test]
fn a_click_on_a_window_behind_activates_it_and_is_delivered() {
    // T.EXE's window, with a button, is behind C.EXE's, which it started.
    // A click on the button asks the window with WM_MOUSEACTIVATE, whose
    // DefWindowProcA answer, MA_ACTIVATE, activates it (WA_CLICKACTIVE)
    // and brings it to the foreground, and the button gets the click:
    // the first click presses it, as on Windows (kibertoad/rust-dos#355).
    const WM_ACTIVATE: i32 = 0x0006;
    const WM_MOUSEACTIVATE: i32 = 0x0021;
    const WM_TIMER: i32 = 0x0113;
    const WM_COMMAND: i32 = 0x0111;
    const WS_CHILD: i32 = 0x4000_0000;
    const MOUSEEVENTF_LEFTDOWN: i32 = 0x0002;
    const MOUSEEVENTF_LEFTUP: i32 = 0x0004;
    let kernel32 = ["CreateProcessA", "TerminateProcess", "ExitProcess"];
    let user32 = [
        "WaitForInputIdle",
        "RegisterClassA",
        "CreateWindowExA",
        "DefWindowProcA",
        "GetMessageA",
        "PeekMessageA",
        "DispatchMessageA",
        "GetForegroundWindow",
        "GetActiveWindow",
        "GetFocus",
        "SetCursorPos",
        "mouse_event",
        "SetTimer",
    ];
    // What the window procedure saw: WM_COMMAND's count at 0xA0,
    // WM_MOUSEACTIVATE's lParam at 0xA4 and wParam at 0xA8, the last
    // WM_ACTIVATE's wParam at 0xAC. A MSG at 0xC0 and a WNDCLASSA at
    // 0xE0, its procedure put in by the code.
    let mut data = child_data(&[
        (0, "C.EXE"),
        (0x10, "Frame"),
        (0x20, "Button"),
        (0x30, "OK"),
    ]);
    data.resize(0x110, 0);
    data[0xF0..0xF4].copy_from_slice(&BASE.to_le_bytes());
    let parent = win_program(pe::SUBSYSTEM_GUI, &kernel32, &user32, &data, |a, l, f| {
        let mut start = a.create_label();
        a.call(start)?;
        // The window procedure.
        for (msg, at, arg) in [
            (WM_MOUSEACTIVATE, 0xA4, 16),
            (WM_MOUSEACTIVATE, 0xA8, 12),
            (WM_ACTIVATE, 0xAC, 12),
        ] {
            let mut other = a.create_label();
            a.cmp(dword_ptr(esp + 8), msg)?;
            a.jne(other)?;
            a.mov(eax, dword_ptr(esp + arg))?;
            a.mov(dword_ptr(l.at(at)), eax)?;
            a.set_label(&mut other)?;
        }
        let mut other = a.create_label();
        a.cmp(dword_ptr(esp + 8), WM_COMMAND)?;
        a.jne(other)?;
        a.inc(dword_ptr(l.at(0xA0)))?;
        a.set_label(&mut other)?;
        a.jmp(f("DefWindowProcA"))?;
        a.set_label(&mut start)?;
        a.pop(eax)?;
        a.mov(dword_ptr(l.at(0xE4)), eax)?;
        a.mov(dword_ptr(l.at(0xE0 + 36)), l.at(0x10) as i32)?;
        a.push(l.at(0xE0) as i32)?;
        a.call(f("RegisterClassA"))?;
        let (class, button) = (l.at(0x10) as i32, l.at(0x20) as i32);
        let style = WS_POPUP | WS_VISIBLE;
        let instance = BASE as i32;
        push_args(
            a,
            &[0, class, class, style, 0, 0, 200, 100, 0, 0, instance, 0],
        )?;
        a.call(f("CreateWindowExA"))?;
        a.mov(esi, eax)?;
        // The button, id 1, in the window: its last four arguments
        // first.
        push_args(a, &[1, 0, 0])?;
        a.push(esi)?;
        let ok = l.at(0x30) as i32;
        push_args(a, &[0, button, ok, WS_CHILD | WS_VISIBLE, 10, 10, 80, 30])?;
        a.call(f("CreateWindowExA"))?;
        a.mov(edi, eax)?;
        a.xor(ebx, ebx)?;
        create_child(a, l, f, 0)?;
        idle(a, l, f, 5000)?;
        pump(a, l, f, 0xC0)?;
        a.call(f("GetForegroundWindow"))?;
        a.cmp(eax, esi)?;
        bit_if(a, ne, 0)?;
        a.mov(dword_ptr(l.at(0xAC)), 0)?;
        push_args(a, &[50, 25])?;
        a.call(f("SetCursorPos"))?;
        for flags in [MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP] {
            push_args(a, &[flags, 0, 0, 0, 0])?;
            a.call(f("mouse_event"))?;
        }
        push_args(a, &[0, 0, 2000, 0])?;
        a.call(f("SetTimer"))?;
        let mut next = a.create_label();
        let mut done = a.create_label();
        a.set_label(&mut next)?;
        push_args(a, &[l.at(0xC0) as i32, 0, 0, 0])?;
        a.call(f("GetMessageA"))?;
        a.cmp(dword_ptr(l.at(0xC4)), WM_TIMER)?;
        a.je(done)?;
        a.push(l.at(0xC0) as i32)?;
        a.call(f("DispatchMessageA"))?;
        a.cmp(dword_ptr(l.at(0xA0)), 0)?;
        a.je(next)?;
        a.set_label(&mut done)?;
        a.cmp(dword_ptr(l.at(0xA0)), 1)?;
        bit_if(a, eq, 1)?;
        a.call(f("GetForegroundWindow"))?;
        a.cmp(eax, esi)?;
        bit_if(a, eq, 2)?;
        a.call(f("GetActiveWindow"))?;
        a.cmp(eax, esi)?;
        bit_if(a, eq, 3)?;
        // HTCLIENT and WM_LBUTTONDOWN; the window to activate.
        a.cmp(dword_ptr(l.at(0xA4)), 0x0201_0001)?;
        bit_if(a, eq, 4)?;
        a.cmp(dword_ptr(l.at(0xA8)), esi)?;
        bit_if(a, eq, 5)?;
        a.cmp(word_ptr(l.at(0xAC)), 2)?;
        bit_if(a, eq, 6)?;
        a.call(f("GetFocus"))?;
        a.cmp(eax, edi)?;
        bit_if(a, eq, 7)?;
        a.push(0)?;
        a.push(dword_ptr(l.at(0x90)))?;
        a.call(f("TerminateProcess"))?;
        a.push(ebx)?;
        a.call(f("ExitProcess"))?;
        a.int3()
    });
    let code = run_family(
        "click_activates_behind",
        &[("T.EXE", parent), ("C.EXE", shows_a_window("Child", 300))],
    );
    assert_eq!(code, 0xFF, "the checks that failed: {:08b}", !code);
}

#[test]
fn a_state_saved_while_win_exec_waits_resumes_the_wait() {
    // C.EXE writes C0.DAT, works for 300 ms, writes C.DAT and only then
    // gets messages. The state is saved once C0.DAT is there, while the
    // parent waits in WinExec; after a load, WinExec still returns only
    // once C.DAT is there.
    let kernel32 = ["Sleep", "CreateFileA", "CloseHandle", "ExitProcess"];
    let user32 = ["SetTimer", "GetMessageA"];
    // The files' names, then a MSG at 0x20.
    let mut data = b"C0.DAT".to_vec();
    data.resize(0x10, 0);
    data.extend_from_slice(b"C.DAT");
    data.resize(0x40, 0);
    let child = win_program(pe::SUBSYSTEM_GUI, &kernel32, &user32, &data, |a, l, f| {
        for (name, ms) in [(0, 300), (0x10, 0)] {
            push_args(
                a,
                &[l.at(name) as i32, GENERIC_WRITE, 0, 0, CREATE_ALWAYS, 0, 0],
            )?;
            a.call(f("CreateFileA"))?;
            a.push(eax)?;
            a.call(f("CloseHandle"))?;
            if ms > 0 {
                a.push(ms)?;
                a.call(f("Sleep"))?;
            }
        }
        push_args(a, &[0, 0, 100, 0])?;
        a.call(f("SetTimer"))?;
        push_args(a, &[l.at(0x20) as i32, 0, 0, 0])?;
        a.call(f("GetMessageA"))?;
        a.push(0)?;
        a.call(f("ExitProcess"))?;
        a.int3()
    });
    let kernel32 = ["WinExec", "GetFileAttributesA", "Sleep", "ExitProcess"];
    let user32 = ["WaitForInputIdle"];
    let data = child_data(&[(0, "C.EXE"), (0x10, "C.DAT")]);
    let parent = win_program(
        pe::SUBSYSTEM_CONSOLE,
        &kernel32,
        &user32,
        &data,
        |a, l, f| {
            a.xor(ebx, ebx)?;
            push_args(a, &[l.at(0) as i32, 1])?;
            a.call(f("WinExec"))?;
            a.cmp(eax, 33)?;
            bit_if(a, eq, 0)?;
            a.push(l.at(0x10) as i32)?;
            a.call(f("GetFileAttributesA"))?;
            a.cmp(eax, -1)?;
            bit_if(a, ne, 1)?;
            // The child ends before the parent.
            a.push(1000)?;
            a.call(f("Sleep"))?;
            a.push(ebx)?;
            a.call(f("ExitProcess"))?;
            a.int3()
        },
    );
    let dir = scratch(
        "win_exec_idle_state",
        &[("T.EXE", parent), ("C.EXE", child)],
    );
    let mut cpu = Cpu::new(dir.clone());
    cpu.bus.win32.enabled = true;
    cpu.host_console = Some(HostConsole::default());
    cpu.load_shell();
    assert!(cpu.load_executable("T.EXE", None));
    let mut batches = 0;
    while !dir.join("C0.DAT").exists() {
        batches += 1;
        assert!(batches < 400_000, "C.EXE didn't start");
        cpu.bus.start_batch(cpu.bus.clock.icount + 1_000);
        run_batch(&mut cpu, &mut NoHook, false);
    }
    assert!(!dir.join("C.DAT").exists(), "saved too late");
    let state = rust_dos::savestate::machine::save(&cpu);
    assert!(run_to_exit(&mut cpu, 400), "{}", printed(&cpu));
    assert_eq!(cpu.errorlevel, 0x3, "before the load");

    fs::remove_file(dir.join("C.DAT")).unwrap();
    rust_dos::savestate::machine::load(&mut cpu, &state).unwrap();
    cpu.errorlevel = 0;
    assert!(run_to_exit(&mut cpu, 400), "{}", printed(&cpu));
    assert_eq!(cpu.errorlevel, 0x3, "after the load");
}

/// GetLogicalDriveStringsA: asked with no buffer, and then with one a
/// byte too small, it writes nothing and gives the size it needs; asked
/// with exactly that size, it writes the list, which the program writes
/// out with its last NUL. Bit 0 of the exit code: the size was the list's
/// length and one; bit 1: the short buffer was left alone and the size
/// given again.
#[test]
fn get_logical_drive_strings_lists_the_drives_roots() {
    let names: &[&str] = &["GetLogicalDriveStringsA", "GetStdHandle", "WriteFile"];
    let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", names)];
    let mut data = vec![0u8; 0x100];
    data[0x10] = 0xAA;
    let exe = program(BASE, false, imports, &data, |a, l| {
        // The size needed, with no buffer.
        a.push(0)?;
        a.push(0)?;
        a.call(l.import(0))?;
        a.mov(ebx, eax)?;
        // A buffer one byte short of it.
        a.lea(ecx, dword_ptr(ebx - 1))?;
        a.push(l.at(0x10) as i32)?;
        a.push(ecx)?;
        a.call(l.import(0))?;
        a.cmp(eax, ebx)?;
        a.sete(dl)?;
        a.cmp(byte_ptr(l.at(0x10)), 0xAA)?;
        a.sete(al)?;
        a.and(dl, al)?;
        a.movzx(edi, dl)?;
        // The list, in a buffer of exactly that size.
        a.push(l.at(0x10) as i32)?;
        a.push(ebx)?;
        a.call(l.import(0))?;
        a.mov(esi, eax)?;
        a.lea(ecx, dword_ptr(eax + 1))?;
        a.push(0)?;
        a.push(l.at(0) as i32)?;
        a.push(ecx)?;
        a.push(l.at(0x10) as i32)?;
        a.push(-11)?;
        a.call(l.import(1))?;
        a.push(eax)?;
        a.call(l.import(2))?;
        a.lea(ecx, dword_ptr(esi + 1))?;
        a.cmp(ebx, ecx)?;
        a.sete(al)?;
        a.movzx(eax, al)?;
        a.shl(edi, 1)?;
        a.or(eax, edi)?;
        a.ret()
    });
    let (code, text) = run("logical_drive_strings", exe);
    assert_eq!(code, 3, "{:?}", text);
    let list = text
        .strip_suffix("\0\0")
        .expect("the list ends in two NULs");
    let roots: Vec<&str> = list.split('\0').collect();
    assert!(roots.contains(&"C:\\"), "{:?}", roots);
    assert!(
        roots.iter().all(|r| r.len() == 3 && r.ends_with(":\\")),
        "{:?}",
        roots
    );
}

/// GetVolumeInformationA on C:\, a host folder: it gives "FAT" as the
/// file system, which the program writes out, and 255 as the longest
/// name; with a file system name buffer too small for "FAT" and its NUL
/// it fails with ERROR_INSUFFICIENT_BUFFER. Bit 0 of the exit code: the
/// first call worked; bit 1: the longest name was 255; bit 2: the second
/// call failed with error 122.
#[test]
fn get_volume_information_names_the_file_system() {
    let names: &[&str] = &[
        "GetVolumeInformationA",
        "GetLastError",
        "GetStdHandle",
        "WriteFile",
    ];
    let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", names)];
    let mut data = vec![0u8; 0x100];
    data[0x80..0x83].copy_from_slice(b"C:\\");
    let exe = program(BASE, false, imports, &data, |a, l| {
        // Label (32 bytes), serial, longest name, flags, file system (16).
        let call = |a: &mut CodeAssembler, fs_size: i32| -> Result<(), IcedError> {
            a.push(fs_size)?;
            a.push(l.at(0x40) as i32)?;
            a.push(l.at(0x38) as i32)?;
            a.push(l.at(0x34) as i32)?;
            a.push(l.at(0x30) as i32)?;
            a.push(32)?;
            a.push(l.at(0x10) as i32)?;
            a.push(l.at(0x80) as i32)?;
            a.call(l.import(0))
        };
        call(a, 16)?;
        a.cmp(eax, 0)?;
        a.setne(bl)?;
        a.movzx(edi, bl)?;
        a.cmp(dword_ptr(l.at(0x34)), 255)?;
        a.sete(bl)?;
        a.movzx(ebx, bl)?;
        a.shl(ebx, 1)?;
        a.or(edi, ebx)?;
        // "FAT" to standard output.
        a.push(0)?;
        a.push(l.at(0x50) as i32)?;
        a.push(3)?;
        a.push(l.at(0x40) as i32)?;
        a.push(-11)?;
        a.call(l.import(2))?;
        a.push(eax)?;
        a.call(l.import(3))?;
        // Too small for "FAT" and its NUL.
        call(a, 3)?;
        a.mov(esi, eax)?;
        a.call(l.import(1))?;
        a.cmp(eax, 122)?;
        a.sete(bl)?;
        a.cmp(esi, 0)?;
        a.sete(cl)?;
        a.and(bl, cl)?;
        a.movzx(ebx, bl)?;
        a.shl(ebx, 2)?;
        a.or(edi, ebx)?;
        a.mov(eax, edi)?;
        a.ret()
    });
    let (code, text) = run("volume_information", exe);
    assert_eq!(code, 7, "{:?}", text);
    assert_eq!(text, "FAT");
}

/// The global atom table, as Delphi's VCL uses it when a program
/// starts. Bit 0 of the exit code: a name's atom is a string atom; bit
/// 1: the name in other case gives the same atom; bit 2:
/// GlobalFindAtomA finds it; bit 3: GlobalGetAtomNameA gives the name
/// as it was first added; bit 4: after two deletes it is gone; bit 5:
/// "#123" and MAKEINTATOM(77) are integer atoms.
#[test]
fn global_atoms_count_their_adds_and_deletes() {
    let names: &[&str] = &[
        "GlobalAddAtomA",
        "GlobalFindAtomA",
        "GlobalDeleteAtom",
        "GlobalGetAtomNameA",
    ];
    let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", names)];
    let mut data = vec![0u8; 0x100];
    put(&mut data, 0, b"Delphi1234\0");
    put(&mut data, 0x10, b"DELPHI1234\0");
    put(&mut data, 0x20, b"delphi1234\0");
    put(&mut data, 0x30, b"#123\0");
    let exe = program(BASE, false, imports, &data, |a, l| {
        let at = |offset: u32| Arg::I(l.at(offset) as u32);
        a.xor(ebx, ebx)?;
        call_with(a, l.import(0), &[at(0)])?;
        a.mov(esi, eax)?;
        a.cmp(eax, 0xC000)?;
        a.setae(al)?;
        a.movzx(eax, al)?;
        a.or(ebx, eax)?;
        call_with(a, l.import(0), &[at(0x10)])?;
        a.cmp(eax, esi)?;
        check(a, false, 1)?;
        call_with(a, l.import(1), &[at(0x20)])?;
        a.cmp(eax, esi)?;
        check(a, false, 2)?;
        call_with(a, l.import(3), &[Arg::Esi, at(0x80), Arg::I(64)])?;
        a.cmp(eax, 10)?;
        check(a, false, 3)?;
        a.cmp(byte_ptr(l.at(0x84)), b'h' as i32)?;
        a.jne(0)?;
        // Once deleted it is still there, twice it is gone.
        call_with(a, l.import(2), &[Arg::Esi])?;
        call_with(a, l.import(1), &[at(0)])?;
        a.cmp(eax, esi)?;
        a.jne(0)?;
        call_with(a, l.import(2), &[Arg::Esi])?;
        call_with(a, l.import(1), &[at(0)])?;
        a.cmp(eax, 0)?;
        check(a, false, 4)?;
        call_with(a, l.import(0), &[at(0x30)])?;
        a.mov(esi, eax)?;
        call_with(a, l.import(0), &[Arg::I(77)])?;
        a.cmp(eax, 77)?;
        a.sete(al)?;
        a.cmp(esi, 123)?;
        a.sete(cl)?;
        a.and(al, cl)?;
        a.movzx(eax, al)?;
        a.shl(eax, 5)?;
        a.or(ebx, eax)?;
        a.mov(eax, ebx)?;
        a.ret()
    });
    let (code, text) = run("global_atoms", exe);
    assert_eq!(code, 0x3F, "{:?}", text);
}

/// Calls GetTickCount `n` times in a loop, then ends.
fn poll_loop(n: i32) -> Vec<u8> {
    let imports: &[(&str, &[&str])] = &[("KERNEL32.dll", &["GetTickCount", "ExitProcess"])];
    program(BASE, false, imports, &[0; 16], |a, l| {
        let mut top = a.create_label();
        a.mov(esi, n)?;
        a.set_label(&mut top)?;
        a.call(l.import(0))?;
        a.dec(esi)?;
        a.jnz(top)?;
        a.push(0)?;
        a.call(l.import(1))?;
        a.int3()
    })
}

/// The log's lines for the program's calls of GetTickCount, with the
/// trace at `level`.
fn poll_trace(test: &str, level: rust_dos::config::Win32Trace) -> Vec<String> {
    let mut cpu = machine(test, poll_loop(10_000));
    let lines = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let log = lines.clone();
    cpu.bus.log_hook = Some(Box::new(move |line: &str| {
        if line.contains("KERNEL32.GetTickCount") {
            log.borrow_mut().push(line.to_string());
        }
    }));
    rust_dos::win32::trace::set_level(&mut cpu, level);
    assert!(run_to_exit(&mut cpu, 2000));
    lines.take()
}

#[test]
fn a_polling_loop_gets_its_first_calls_and_a_count_in_the_log() {
    let lines = poll_trace("trace_collapsed", rust_dos::config::Win32Trace::Collapsed);
    // A few lines in full, and the count of the rest, which the program's
    // end writes if its loop was shorter than a second.
    let (summaries, calls): (Vec<&String>, Vec<&String>) =
        lines.iter().partition(|l| l.contains(" more in "));
    assert!(calls.len() <= 12, "{lines:#?}");
    assert!(calls.iter().all(|l| l.contains("GetTickCount() = ")), "{calls:?}");
    let counted: usize = summaries
        .iter()
        .map(|l| {
            let count = l.split(" x").nth(1).unwrap();
            count.split(' ').next().unwrap().parse::<usize>().unwrap()
        })
        .sum();
    assert_eq!(calls.len() + counted, 10_000, "{lines:#?}");

    let all = poll_trace("trace_all", rust_dos::config::Win32Trace::All);
    assert_eq!(all.len(), 10_000);
    assert!(poll_trace("trace_off", rust_dos::config::Win32Trace::Off).is_empty());
}

/// The checksum both sides of the flat thunk test have.
const THUNK_CHECKSUM: u32 = 0x1234;

/// A 16-bit DLL, TEST16.DLL, as the thunk compiler's 16-bit side would
/// be: its 32-bit to 16-bit thunk data (exported as THK_THUNKDATA16), the
/// table of its two targets, and a DllEntryPoint that calls
/// ThunkConnect16 and keeps what it returned. Target 0 is READ(LPDWORD),
/// which returns the value there in DX:AX and adds 1 to it there; target
/// 1 is FLAG(), which returns what ThunkConnect16 returned.
fn thunk_dll16() -> Vec<u8> {
    use rust_dos_ne as ne;
    let mut b = ne::Builder::new("TEST16", ne::LIBMODULE | ne::SINGLEDATA | ne::PROTMODE);
    b.expected_version(0x0400);
    let kernel = b.import_module("KERNEL");
    let farptr = |offset: u16, target: ne::Target| ne::Relocation {
        source: ne::ADDR_FARPTR,
        additive: false,
        offset,
        target,
    };
    let mut code = vec![
        0x55, // push bp
        0x8B, 0xEC, // mov bp, sp
        0x1E, // push ds
        0x8B, 0x46, 0x0E, // mov ax, [bp+0Eh] (wDS)
        0x8E, 0xD8, // mov ds, ax
        0x1E, 0x6A, 0x30, // push ds:0030h, "TEST16.DLL"
        0x1E, 0x6A, 0x3C, // push ds:003Ch, "T.EXE"
        0xFF, 0x76, 0x10, // push word [bp+10h] (hInst)
        0x66, 0xFF, 0x76, 0x12, // push dword [bp+12h] (dwReason)
        0x1E, 0x6A, 0x00, // push ds:0000h, the thunk data
        0x1E, 0x6A, 0x44, // push ds:0044h, the 32-bit side's name
        0x0E, // push cs
        0x9A, 0xFF, 0xFF, 0x00, 0x00, // call far ThunkConnect16
        0xA3, 0x10, 0x00, // mov [0010h], ax
        0xB8, 0x01, 0x00, // mov ax, 1
        0x1F, // pop ds
        0x5D, // pop bp
        0xCA, 0x10, 0x00, // retf 10h
    ];
    code.resize(0x30, 0x90);
    // READ at 30h.
    code.extend_from_slice(&[
        0x55, 0x8B, 0xEC, // push bp; mov bp, sp
        0xC4, 0x5E, 0x06, // les bx, [bp+6]
        0x26, 0x8B, 0x07, // mov ax, es:[bx]
        0x26, 0x8B, 0x57, 0x02, // mov dx, es:[bx+2]
        0x26, 0xFF, 0x07, // inc word es:[bx]
        0x5D, // pop bp
        0xCA, 0x04, 0x00, // retf 4
    ]);
    code.resize(0x50, 0x90);
    // FLAG at 50h.
    code.extend_from_slice(&[
        0x1E, // push ds
        0xB8, 0xFF, 0xFF, // mov ax, DGROUP
        0x8E, 0xD8, // mov ds, ax
        0xA1, 0x10, 0x00, // mov ax, [0010h]
        0x31, 0xD2, // xor dx, dx
        0x1F, // pop ds
        0xCB, // retf
    ]);
    b.segment(ne::SegmentSpec {
        data: code,
        flags: 0,
        alloc: 0,
        relocations: vec![
            farptr(
                0x1E,
                ne::Target::Ordinal {
                    module: kernel,
                    ordinal: 651,
                },
            ),
            ne::Relocation {
                source: ne::ADDR_SELECTOR,
                additive: false,
                offset: 0x52,
                target: ne::Target::Internal {
                    segment: 2,
                    offset: 0,
                },
            },
        ],
    });
    let mut data = vec![0u8; 0x60];
    data[..4].copy_from_slice(b"LS01");
    data[4..8].copy_from_slice(&THUNK_CHECKSUM.to_le_bytes());
    // The table of targets' 16:16 address, and the table's two.
    for at in [0x08, 0x20, 0x24] {
        data[at..at + 2].copy_from_slice(&[0xFF, 0xFF]);
    }
    data[0x30..0x3B].copy_from_slice(b"TEST16.DLL\0");
    data[0x3C..0x42].copy_from_slice(b"T.EXE\0");
    data[0x44..0x54].copy_from_slice(b"THK_THUNKDATA32\0");
    let internal = |segment: u8, offset: u16| ne::Target::Internal { segment, offset };
    b.segment(ne::SegmentSpec {
        data,
        flags: ne::SEG_DATA,
        alloc: 0x100,
        relocations: vec![
            farptr(0x08, internal(2, 0x20)),
            farptr(0x20, internal(1, 0x30)),
            farptr(0x24, internal(1, 0x50)),
        ],
    });
    b.auto_data(2, 0, 0);
    for (ordinal, name, segment) in [(1, "THK_THUNKDATA16", 2), (2, "DLLENTRYPOINT", 1)] {
        b.entry(
            ordinal,
            ne::Entry::Code {
                segment,
                offset: 0,
                flags: ne::ENTRY_EXPORTED,
                moveable: false,
            },
        );
        b.export(name, ordinal, true);
    }
    b.build()
}

#[test]
fn a_flat_thunk_calls_a_16_bit_dll_with_a_mapped_pointer() {
    // The 32-bit thunk data, the names ThunkConnect32 takes, and a value
    // the 16-bit side reads and changes through a 16:16 pointer to it.
    let mut data = vec![0u8; 0x100];
    data[..4].copy_from_slice(b"LS01");
    data[4..8].copy_from_slice(&THUNK_CHECKSUM.to_le_bytes());
    data[0x0C..0x10].copy_from_slice(b"LB01");
    data[0x1C..0x20].copy_from_slice(&0x24u32.to_le_bytes());
    data[0x20..0x24].copy_from_slice(&0x44u32.to_le_bytes());
    data[0x24..0x64].fill(0xCC);
    data[0x80..0x90].copy_from_slice(b"THK_THUNKDATA16\0");
    data[0xA0..0xAB].copy_from_slice(b"TEST16.DLL\0");
    data[0xB0..0xB6].copy_from_slice(b"T.EXE\0");
    data[0xC0..0xC4].copy_from_slice(&0x1122_3344u32.to_le_bytes());
    let imports: &[(&str, &[&str])] = &[(
        "KERNEL32.DLL",
        &[
            "ThunkConnect32",
            "SMapLS_IP_EBP_8",
            "SUnMapLS_IP_EBP_8",
            "ExitProcess",
        ],
    )];
    let exe = program(BASE, false, imports, &data, |a, l| {
        let mut read = a.create_label();
        let mut flag = a.create_label();
        let mut fail = a.create_label();
        // The code ThunkConnect32 writes into the thunk data.
        let qt = l.at(0x24);
        // ThunkConnect32(data, "THK_THUNKDATA16", "TEST16.DLL", "T.EXE",
        // module, DLL_PROCESS_ATTACH).
        a.push(1)?;
        a.push(0)?;
        for at in [0xB0, 0xA0, 0x80, 0] {
            a.push(l.at(at) as i32)?;
        }
        a.call(l.import(0))?;
        a.mov(ebx, 1)?;
        a.cmp(eax, 1)?;
        a.jne(fail)?;
        a.push(l.at(0xC0) as i32)?;
        a.call(read)?;
        a.mov(ebx, 2)?;
        a.cmp(eax, 0x1122_3344)?;
        a.jne(fail)?;
        a.mov(ebx, 3)?;
        a.cmp(dword_ptr(l.at(0xC0)), 0x1122_3345)?;
        a.jne(fail)?;
        a.call(flag)?;
        a.mov(ebx, 4)?;
        a.cmp(eax, 1)?;
        a.jne(fail)?;
        a.xor(ebx, ebx)?;
        a.set_label(&mut fail)?;
        a.push(ebx)?;
        a.call(l.import(3))?;
        a.int3()?;
        // The thunk compiler's thunks: the target's number in CL kept at
        // EBP - 4, 40h bytes of frame, the arguments pushed below.
        a.set_label(&mut read)?;
        a.mov(cl, 0)?;
        a.push(ebp)?;
        a.mov(ebp, esp)?;
        a.push(ecx)?;
        a.sub(esp, 0x3C)?;
        a.call(l.import(1))?;
        a.push(eax)?;
        a.mov(eax, qt as u32)?;
        a.call(eax)?;
        a.shl(eax, 16)?;
        a.shrd(eax, edx, 16)?;
        a.call(l.import(2))?;
        a.leave()?;
        a.ret_1(4)?;
        a.set_label(&mut flag)?;
        a.mov(cl, 1)?;
        a.push(ebp)?;
        a.mov(ebp, esp)?;
        a.push(ecx)?;
        a.sub(esp, 0x3C)?;
        a.mov(eax, qt as u32)?;
        a.call(eax)?;
        a.shl(eax, 16)?;
        a.shrd(eax, edx, 16)?;
        a.leave()?;
        a.ret()
    });
    let dir = scratch(
        "flat_thunk",
        &[("T.EXE", exe), ("TEST16.DLL", thunk_dll16())],
    );
    let mut cpu = Cpu::new(dir);
    cpu.bus.win32.enabled = true;
    cpu.host_console = Some(HostConsole::default());
    cpu.load_shell();
    assert!(cpu.load_executable("T.EXE", None));
    assert!(
        run_to_exit(&mut cpu, 400),
        "the program didn't end: {:?}",
        printed(&cpu)
    );
    // The Win16 process the thunks made ended with the program.
    assert!(!cpu.bus.win32.active());
    assert_eq!(cpu.errorlevel, 0, "{}", printed(&cpu));
}
