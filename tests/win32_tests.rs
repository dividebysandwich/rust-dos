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

#[test]
fn a_program_starts_another_and_waits_for_its_exit_code() {
    // C.EXE says hello and ends with 7.
    let child = {
        let imports: &[(&str, &[&str])] = &[("KERNEL32.DLL", &["ExitProcess"])];
        program(BASE, false, imports, &[0; 4], |a, l| {
            a.push(7)?;
            a.call(l.import(0))?;
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
    let mut data = b"C.EXE an argument\0".to_vec();
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
    let mut cpu = Cpu::new(scratch(
        "child_process",
        &[("T.EXE", parent), ("C.EXE", child)],
    ));
    cpu.bus.win32.enabled = true;
    cpu.host_console = Some(HostConsole::default());
    cpu.load_shell();
    assert!(cpu.load_executable("T.EXE", None));
    assert!(run_to_exit(&mut cpu, 200), "{}", printed(&cpu));
    assert_eq!(cpu.errorlevel, 0x71, "{}", printed(&cpu));
    assert!(!cpu.bus.win32.active());
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
