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
