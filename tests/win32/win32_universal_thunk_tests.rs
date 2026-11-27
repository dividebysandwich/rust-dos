//! Win32s's Universal Thunks on the built-in Windows 98: KERNEL32's
//! UTRegister and UTUnRegister, the thunk UTRegister makes, and
//! WIN32S16's UTLinearToSelectorOffset and UTSelectorOffsetToLinear,
//! which the 16-bit side calls. The Win32 program is a PE file made here
//! with rust-dos-pe's builder, its code assembled with iced, and the
//! 16-bit DLL an NE file rust-dos-ne's builder makes.

use iced_x86::code_asm::*;
use rust_dos::config::WindowsVersion;
use rust_dos::cpu::{Cpu, HostConsole};
use rust_dos::exec::{NoHook, StopReason, run_batch};
use rust_dos::game_fixes::{FIXES, Kind};
use rust_dos_ne as ne;
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
    fn at(&self, offset: u32) -> u32 {
        self.data + offset
    }

    /// The dword at byte `offset` of the data.
    fn dword(&self, offset: u32) -> AsmMemoryOperand {
        dword_ptr(self.at(offset) as u64)
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
    b.section(
        ".text",
        &code,
        pe::SCN_CODE | pe::SCN_EXECUTE | pe::SCN_READ,
    );
    b.entry(code_rva);
    b.build()
}

/// Run `program` as T.EXE, with `more` files beside it, on the built-in
/// Windows 98 SE to its end: its exit code and what it printed.
fn run(test: &str, program: Vec<u8>, more: &[(&str, Vec<u8>)]) -> (u8, String) {
    run_as(test, "T.EXE", program, more, false)
}

/// Run `program` as `name`, as `run` does, with the game fixes on if
/// `fixes` is.
fn run_as(
    test: &str,
    name: &str,
    program: Vec<u8>,
    more: &[(&str, Vec<u8>)],
    fixes: bool,
) -> (u8, String) {
    let base = PathBuf::from("target/test_win32_universal").join(test);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    fs::write(base.join(name), program).unwrap();
    for (file, bytes) in more {
        fs::write(base.join(file), bytes).unwrap();
    }
    let mut cpu = Cpu::new(base);
    cpu.bus.win32.enabled = true;
    cpu.bus.win32.version = WindowsVersion::Win98Se;
    cpu.bus.game_fixes = fixes;
    cpu.host_console = Some(HostConsole::default());
    cpu.load_shell();
    assert!(cpu.load_executable(name, None));
    let mut ended = false;
    for _ in 0..400 {
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
    assert!(ended, "the program didn't end: {:?}", text);
    // The Win16 process the thunk's calls ran in ended with the program.
    assert!(!cpu.bus.win32.active());
    (cpu.errorlevel, text)
}

/// UT16.DLL, the 16-bit DLL of a Universal Thunk. UTINIT(callback,
/// buffer) fails for a callback, and otherwise writes 1616h at the
/// buffer's byte 8. UTPROC(buffer, value) adds the value to the word the
/// 16:16 pointer at the buffer's byte 0 points to, writes 5678h at the
/// flat address at its byte 4 through the pointer
/// UTLinearToSelectorOffset makes of it, and returns what
/// UTSelectorOffsetToLinear makes of the buffer's pointer. It imports
/// the first by name, as Lunicus's LUNIFX.DLL does, and the second by
/// ordinal.
fn ut_dll16() -> Vec<u8> {
    let mut b = ne::Builder::new("UT16", ne::LIBMODULE | ne::SINGLEDATA | ne::PROTMODE);
    b.expected_version(0x0400);
    let win32s16 = b.import_module("WIN32S16");
    let by_name = b.imported_name("UTLINEARTOSELECTOROFFSET");
    let mut code = vec![
        0x55, // push bp
        0x8B, 0xEC, // mov bp, sp
        0x31, 0xC0, // xor ax, ax
        0x31, 0xD2, // xor dx, dx
        0x8B, 0x4E, 0x0A, // mov cx, [bp+0Ah] (the callback)
        0x0B, 0x4E, 0x0C, // or cx, [bp+0Ch]
        0x75, 0x0A, // jnz done
        0xC4, 0x5E, 0x06, // les bx, [bp+6] (the buffer)
        0x26, 0xC7, 0x47, 0x08, 0x16, 0x16, // mov word es:[bx+8], 1616h
        0x40, // inc ax
        0x5D, // done: pop bp
        0xCA, 0x08, 0x00, // retf 8
    ];
    code.resize(0x40, 0x90);
    code.extend_from_slice(&[
        0x55, // push bp
        0x8B, 0xEC, // mov bp, sp
        0xC4, 0x5E, 0x0A, // les bx, [bp+0Ah] (the buffer)
        0x26, 0xC4, 0x1F, // les bx, es:[bx]
        0x8B, 0x46, 0x06, // mov ax, [bp+6] (the value)
        0x26, 0x01, 0x07, // add es:[bx], ax
        0xC4, 0x5E, 0x0A, // les bx, [bp+0Ah]
        0x66, 0x26, 0xFF, 0x77, 0x04, // push dword es:[bx+4]
    ]);
    let linear_to_segptr = code.len() as u16 + 1;
    code.extend_from_slice(&[
        0x9A, 0xFF, 0xFF, 0x00, 0x00, // call far UTLinearToSelectorOffset
        0x8E, 0xC2, // mov es, dx
        0x8B, 0xD8, // mov bx, ax
        0x26, 0xC7, 0x07, 0x78, 0x56, // mov word es:[bx], 5678h
        0x66, 0xFF, 0x76, 0x0A, // push dword [bp+0Ah]
    ]);
    let segptr_to_linear = code.len() as u16 + 1;
    code.extend_from_slice(&[
        0x9A, 0xFF, 0xFF, 0x00, 0x00, // call far UTSelectorOffsetToLinear
        0x5D, // pop bp
        0xCA, 0x08, 0x00, // retf 8
    ]);
    let farptr = |offset: u16, target: ne::Target| ne::Relocation {
        source: ne::ADDR_FARPTR,
        additive: false,
        offset,
        target,
    };
    let text = b.segment(ne::SegmentSpec {
        data: code,
        flags: 0,
        alloc: 0,
        relocations: vec![
            farptr(
                linear_to_segptr,
                ne::Target::Name {
                    module: win32s16,
                    name_offset: by_name,
                },
            ),
            farptr(
                segptr_to_linear,
                ne::Target::Ordinal {
                    module: win32s16,
                    ordinal: 45,
                },
            ),
        ],
    });
    let data = b.segment(ne::SegmentSpec {
        data: vec![0; 0x10],
        flags: ne::SEG_DATA,
        alloc: 0x100,
        relocations: Vec::new(),
    });
    b.auto_data(data, 0, 0);
    for (ordinal, name, offset) in [(1, "UTINIT", 0), (2, "UTPROC", 0x40)] {
        b.entry(
            ordinal,
            ne::Entry::Code {
                segment: text as u8,
                offset,
                flags: ne::ENTRY_EXPORTED,
                moveable: false,
            },
        );
        b.export(name, ordinal, true);
    }
    b.build()
}

/// The program's data: the names UTRegister takes, where the thunks'
/// addresses go (30h, 34h), the second block's address (38h), the third
/// block's (3Ch), the buffer (40h), the value UTPROC adds to (80h) and
/// the translation list (90h).
fn ut_data() -> Vec<u8> {
    let mut data = vec![0u8; 0x100];
    data[..9].copy_from_slice(b"UT16.DLL\0");
    data[0x10..0x17].copy_from_slice(b"UTInit\0");
    data[0x20..0x27].copy_from_slice(b"UTProc\0");
    data[0x80..0x84].copy_from_slice(&0x1000u32.to_le_bytes());
    data
}

const MEM_COMMIT: i32 = 0x1000;
const MEM_RESERVE: i32 = 0x2000;
const MEM_DECOMMIT: i32 = 0x4000;
const PAGE_READWRITE: i32 = 4;

#[test]
fn a_universal_thunk_calls_its_16_bit_dll_with_the_pointers_translated() {
    let imports: &[(&str, &[&str])] = &[(
        "KERNEL32.DLL",
        &[
            "UTRegister",
            "UTUnRegister",
            "VirtualAlloc",
            "ExitProcess",
            "VirtualFree",
        ],
    )];
    let exe = program(imports, &ut_data(), |a, l| {
        let mut fail = a.create_label();
        // UTRegister(BASE, "UT16.DLL", "UTInit", "UTProc", where,
        // callback, buffer).
        let register = |a: &mut CodeAssembler, module: u32, callback: u32, at: u32| {
            a.push(l.at(0x40))?;
            a.push(callback)?;
            a.push(l.at(at))?;
            for name in [0x20, 0x10, 0] {
                a.push(l.at(name))?;
            }
            a.push(module)?;
            a.call(l.import(0))
        };
        // A call of the thunk whose address is at `thunk`, with the value
        // `value`: its pointer to the value is to be turned through the
        // list, the flat address of the second block's byte `at` is for
        // UTLinearToSelectorOffset.
        let call = |a: &mut CodeAssembler, thunk: u32, value: i32, at: i32| {
            a.mov(l.dword(0x40), l.at(0x80))?;
            a.mov(eax, l.dword(0x38))?;
            a.add(eax, at)?;
            a.mov(l.dword(0x44), eax)?;
            a.mov(l.dword(0x90), l.at(0x40))?;
            a.mov(l.dword(0x94), 0)?;
            a.push(l.at(0x90))?;
            a.push(value)?;
            a.push(l.at(0x40))?;
            a.call(l.dword(thunk))
        };
        // A second block of 64 KB, its first page committed.
        a.push(PAGE_READWRITE)?;
        a.push(MEM_RESERVE)?;
        a.push(0x1_0000)?;
        a.push(0)?;
        a.call(l.import(2))?;
        a.mov(l.dword(0x38), eax)?;
        a.mov(ebx, 1)?;
        a.test(eax, eax)?;
        a.jz(fail)?;
        a.push(PAGE_READWRITE)?;
        a.push(MEM_COMMIT)?;
        a.push(0x1000)?;
        a.push(eax)?;
        a.call(l.import(2))?;
        a.mov(ebx, 2)?;
        a.test(eax, eax)?;
        a.jz(fail)?;
        // Registered, its UTInit run with the buffer and no callback.
        register(a, BASE, 0, 0x30)?;
        a.mov(ebx, 3)?;
        a.cmp(eax, 1)?;
        a.jne(fail)?;
        a.mov(ebx, 4)?;
        a.cmp(word_ptr(l.at(0x48) as u64), 0x1616)?;
        a.jne(fail)?;
        // UTProc gets the value, reaches the value through the list's
        // pointer and the second block through its own selector, and
        // returns the buffer's flat address.
        call(a, 0x30, 5, 0x10)?;
        a.mov(ebx, 5)?;
        a.cmp(eax, l.at(0x40))?;
        a.jne(fail)?;
        a.mov(ebx, 6)?;
        a.cmp(l.dword(0x80), 0x1005)?;
        a.jne(fail)?;
        a.mov(eax, l.dword(0x38))?;
        a.mov(ebx, 7)?;
        a.cmp(word_ptr(eax + 0x10), 0x5678)?;
        a.jne(fail)?;
        // The list's pointer stays a 16:16 one, its offset the value's
        // in its 32 KB block.
        a.mov(eax, l.at(0x80) & 0x7FFF)?;
        a.mov(ebx, 8)?;
        a.cmp(word_ptr(l.at(0x40) as u64), ax)?;
        a.jne(fail)?;
        // The second block's next page, committed after its selector was
        // made, is under that selector too.
        a.mov(eax, l.dword(0x38))?;
        a.add(eax, 0x1000)?;
        a.push(PAGE_READWRITE)?;
        a.push(MEM_COMMIT)?;
        a.push(0x1000)?;
        a.push(eax)?;
        a.call(l.import(2))?;
        a.mov(ebx, 9)?;
        a.test(eax, eax)?;
        a.jz(fail)?;
        call(a, 0x30, 7, 0x1010)?;
        a.mov(ebx, 10)?;
        a.cmp(l.dword(0x80), 0x100C)?;
        a.jne(fail)?;
        a.mov(eax, l.dword(0x38))?;
        a.mov(ebx, 11)?;
        a.cmp(word_ptr(eax + 0x1010), 0x5678)?;
        a.jne(fail)?;
        // That page decommitted, its physical page taken by a new block,
        // and committed again: the selector has the page that is there
        // now, and the new block keeps its own.
        a.mov(eax, l.dword(0x38))?;
        a.add(eax, 0x1000)?;
        a.push(MEM_DECOMMIT)?;
        a.push(0x1000)?;
        a.push(eax)?;
        a.call(l.import(4))?;
        a.mov(ebx, 12)?;
        a.test(eax, eax)?;
        a.jz(fail)?;
        a.push(PAGE_READWRITE)?;
        a.push(MEM_COMMIT | MEM_RESERVE)?;
        a.push(0x1000)?;
        a.push(0)?;
        a.call(l.import(2))?;
        a.mov(l.dword(0x3C), eax)?;
        a.mov(ebx, 13)?;
        a.test(eax, eax)?;
        a.jz(fail)?;
        a.mov(dword_ptr(eax + 0x10), 0x1111)?;
        a.mov(eax, l.dword(0x38))?;
        a.add(eax, 0x1000)?;
        a.push(PAGE_READWRITE)?;
        a.push(MEM_COMMIT)?;
        a.push(0x1000)?;
        a.push(eax)?;
        a.call(l.import(2))?;
        a.mov(ebx, 14)?;
        a.test(eax, eax)?;
        a.jz(fail)?;
        a.mov(dword_ptr(eax + 0x10), 0)?;
        call(a, 0x30, 9, 0x1010)?;
        a.mov(ebx, 15)?;
        a.cmp(l.dword(0x80), 0x1015)?;
        a.jne(fail)?;
        a.mov(eax, l.dword(0x38))?;
        a.mov(ebx, 16)?;
        a.cmp(word_ptr(eax + 0x1010), 0x5678)?;
        a.jne(fail)?;
        a.mov(eax, l.dword(0x3C))?;
        a.mov(ebx, 17)?;
        a.cmp(dword_ptr(eax + 0x10), 0x1111)?;
        a.jne(fail)?;
        // A DLL registers once. As on Windows 98, a second UTRegister runs
        // UTInit and writes the address of a thunk before it fails, and
        // that thunk works while the DLL stays loaded.
        a.mov(word_ptr(l.at(0x48) as u64), 0)?;
        a.mov(l.dword(0x34), 0)?;
        register(a, BASE, 0, 0x34)?;
        a.mov(ebx, 18)?;
        a.test(eax, eax)?;
        a.jnz(fail)?;
        a.mov(ebx, 19)?;
        a.cmp(word_ptr(l.at(0x48) as u64), 0x1616)?;
        a.jne(fail)?;
        a.mov(ebx, 20)?;
        a.cmp(l.dword(0x34), 0)?;
        a.je(fail)?;
        call(a, 0x34, 1, 0x10)?;
        a.mov(ebx, 21)?;
        a.cmp(l.dword(0x80), 0x1016)?;
        a.jne(fail)?;
        // A callback, which Windows 98 doesn't have, fails before the DLL
        // is loaded.
        a.mov(word_ptr(l.at(0x48) as u64), 0)?;
        a.mov(l.dword(0x34), 0)?;
        register(a, BASE + 1, BASE, 0x34)?;
        a.mov(ebx, 22)?;
        a.test(eax, eax)?;
        a.jnz(fail)?;
        a.cmp(word_ptr(l.at(0x48) as u64), 0)?;
        a.jne(fail)?;
        a.cmp(l.dword(0x34), 0)?;
        a.jne(fail)?;
        // Once unregistered, it registers again, UTInit run again.
        a.push(BASE)?;
        a.call(l.import(1))?;
        a.mov(word_ptr(l.at(0x48) as u64), 0)?;
        register(a, BASE, 0, 0x30)?;
        a.mov(ebx, 23)?;
        a.cmp(eax, 1)?;
        a.jne(fail)?;
        a.mov(ebx, 24)?;
        a.cmp(word_ptr(l.at(0x48) as u64), 0x1616)?;
        a.jne(fail)?;
        a.xor(ebx, ebx)?;
        a.set_label(&mut fail)?;
        a.push(ebx)?;
        a.call(l.import(3))?;
        a.int3()
    });
    let (code, text) = run("thunk", exe, &[("UT16.DLL", ut_dll16())]);
    assert_eq!(code, 0, "{}", text);
}

/// UT16.DLL for the selectors' test: UTINIT returns 1, and UTPROC(buffer,
/// value) writes the 16:16 pointer UTLinearToSelectorOffset makes of the
/// flat address at the buffer's byte 4 at its byte 8, and 5678h through
/// that pointer.
fn ut_dll16_pointer() -> Vec<u8> {
    let mut b = ne::Builder::new("UT16", ne::LIBMODULE | ne::SINGLEDATA | ne::PROTMODE);
    b.expected_version(0x0400);
    let win32s16 = b.import_module("WIN32S16");
    let by_name = b.imported_name("UTLINEARTOSELECTOROFFSET");
    let mut code = vec![
        0xB8, 0x01, 0x00, // mov ax, 1
        0x31, 0xD2, // xor dx, dx
        0xCA, 0x08, 0x00, // retf 8
    ];
    code.resize(0x40, 0x90);
    code.extend_from_slice(&[
        0x55, // push bp
        0x8B, 0xEC, // mov bp, sp
        0xC4, 0x5E, 0x0A, // les bx, [bp+0Ah] (the buffer)
        0x66, 0x26, 0xFF, 0x77, 0x04, // push dword es:[bx+4]
    ]);
    let linear_to_segptr = code.len() as u16 + 1;
    code.extend_from_slice(&[
        0x9A, 0xFF, 0xFF, 0x00, 0x00, // call far UTLinearToSelectorOffset
        0xC4, 0x5E, 0x0A, // les bx, [bp+0Ah]
        0x26, 0x89, 0x47, 0x08, // mov es:[bx+8], ax
        0x26, 0x89, 0x57, 0x0A, // mov es:[bx+0Ah], dx
        0x8E, 0xC2, // mov es, dx
        0x8B, 0xD8, // mov bx, ax
        0x26, 0xC7, 0x07, 0x78, 0x56, // mov word es:[bx], 5678h
        0x5D, // pop bp
        0xCA, 0x08, 0x00, // retf 8
    ]);
    let text = b.segment(ne::SegmentSpec {
        data: code,
        flags: 0,
        alloc: 0,
        relocations: vec![ne::Relocation {
            source: ne::ADDR_FARPTR,
            additive: false,
            offset: linear_to_segptr,
            target: ne::Target::Name {
                module: win32s16,
                name_offset: by_name,
            },
        }],
    });
    let data = b.segment(ne::SegmentSpec {
        data: vec![0; 0x10],
        flags: ne::SEG_DATA,
        alloc: 0x100,
        relocations: Vec::new(),
    });
    b.auto_data(data, 0, 0);
    for (ordinal, name, offset) in [(1, "UTINIT", 0), (2, "UTPROC", 0x40)] {
        b.entry(
            ordinal,
            ne::Entry::Code {
                segment: text as u8,
                offset,
                flags: ne::ENTRY_EXPORTED,
                moveable: false,
            },
        );
        b.export(name, ordinal, true);
    }
    b.build()
}

/// A program of `size` bytes that reserves and commits 192 KB, has
/// UTPROC translate the addresses of its bytes 18010h and 8010h, and
/// checks that 5678h got there. It ends with 10h when the two pointers
/// have the offset 8010h from the region's start and selectors one tile
/// (8) apart, as Win32s gives them; 20h when they have the offset 10h in
/// their 32 KB blocks, as Windows 98 does; and otherwise with the step
/// that failed.
fn region_selectors_program(size: usize) -> Vec<u8> {
    let imports: &[(&str, &[&str])] = &[(
        "KERNEL32.DLL",
        &["UTRegister", "VirtualAlloc", "ExitProcess"],
    )];
    let mut exe = program(imports, &ut_data(), |a, l| {
        let mut fail = a.create_label();
        let mut blocks = a.create_label();
        // The pointer UTPROC makes of the region's byte `at`, in ESI.
        let call = |a: &mut CodeAssembler, at: i32| {
            a.mov(eax, l.dword(0x38))?;
            a.add(eax, at)?;
            a.mov(l.dword(0x44), eax)?;
            a.push(0)?;
            a.push(0)?;
            a.push(l.at(0x40))?;
            a.call(l.dword(0x30))?;
            a.mov(esi, l.dword(0x48))
        };
        a.push(PAGE_READWRITE)?;
        a.push(MEM_COMMIT | MEM_RESERVE)?;
        a.push(0x3_0000)?;
        a.push(0)?;
        a.call(l.import(1))?;
        a.mov(l.dword(0x38), eax)?;
        a.mov(ebx, 1)?;
        a.test(eax, eax)?;
        a.jz(fail)?;
        // UTRegister(BASE, "UT16.DLL", "UTInit", "UTProc", where, no
        // callback, buffer).
        a.push(l.at(0x40))?;
        a.push(0)?;
        a.push(l.at(0x30))?;
        for name in [0x20, 0x10, 0] {
            a.push(l.at(name))?;
        }
        a.push(BASE)?;
        a.call(l.import(0))?;
        a.mov(ebx, 2)?;
        a.cmp(eax, 1)?;
        a.jne(fail)?;
        call(a, 0x1_8010)?;
        a.mov(edi, esi)?;
        a.mov(eax, l.dword(0x38))?;
        a.mov(ebx, 3)?;
        a.cmp(word_ptr(eax + 0x1_8010), 0x5678)?;
        a.jne(fail)?;
        call(a, 0x8010)?;
        a.mov(eax, l.dword(0x38))?;
        a.mov(ebx, 4)?;
        a.cmp(word_ptr(eax + 0x8010), 0x5678)?;
        a.jne(fail)?;
        a.mov(ebx, 5)?;
        a.cmp(di, si)?;
        a.jne(fail)?;
        a.cmp(si, 0x10)?;
        a.je(blocks)?;
        a.cmp(si, 0x8010)?;
        a.jne(fail)?;
        // Win32s's: the tile of the region's 10000h to 1FFFFh is the
        // next one.
        a.shr(esi, 16)?;
        a.shr(edi, 16)?;
        a.sub(edi, esi)?;
        a.mov(ebx, 6)?;
        a.cmp(edi, 8)?;
        a.jne(fail)?;
        a.mov(ebx, 0x10)?;
        a.jmp(fail)?;
        a.set_label(&mut blocks)?;
        a.mov(ebx, 0x20)?;
        a.set_label(&mut fail)?;
        a.push(ebx)?;
        a.call(l.import(2))?;
        a.int3()
    });
    assert!(exe.len() <= size);
    exe.resize(size, 0);
    exe
}

/// The program file the game fix `lunicus-win32s` gives Win32s's shims,
/// and its size.
fn win32s_program() -> (&'static str, usize) {
    FIXES
        .iter()
        .find_map(|fix| match fix.kind {
            Kind::Win32Shims { program, size, .. } if fix.name == "lunicus-win32s" => {
                Some((program, size))
            }
            _ => None,
        })
        .unwrap()
}

#[test]
fn a_win32s_program_gets_its_region_s_tiled_selectors() {
    let (name, size) = win32s_program();
    let more = [("UT16.DLL", ut_dll16_pointer())];
    let exe = region_selectors_program(size);
    let (code, text) = run_as("region_tiles", name, exe.clone(), &more, true);
    assert_eq!(code, 0x10, "{}", text);
    // Windows 98's 32 KB blocks for the program without the shims.
    let (code, text) = run_as("region_blocks", name, exe, &more, false);
    assert_eq!(code, 0x20, "{}", text);
}

/// A program of `size` bytes that 48 times reserves a region, from 13 MB
/// to 64 KB less than 16 MB, 64 KB more each time, commits its first
/// 128 KB, has UTPROC translate the address of its byte 18010h, checks
/// that 5678h got there, and releases it. It ends with 0 when every
/// pointer has the offset 8010h of the region's second tile, and
/// otherwise with the step that failed: 6 when a pointer has another
/// offset, as a 32 KB block's selector has when the LDT has no run of
/// the tiles left.
fn released_regions_program(size: usize) -> Vec<u8> {
    const MEM_RELEASE: i32 = 0x8000;
    let imports: &[(&str, &[&str])] = &[(
        "KERNEL32.DLL",
        &["UTRegister", "VirtualAlloc", "VirtualFree", "ExitProcess"],
    )];
    let mut exe = program(imports, &ut_data(), |a, l| {
        let mut fail = a.create_label();
        let mut again = a.create_label();
        // UTRegister(BASE, "UT16.DLL", "UTInit", "UTProc", where, no
        // callback, buffer).
        a.push(l.at(0x40))?;
        a.push(0)?;
        a.push(l.at(0x30))?;
        for name in [0x20, 0x10, 0] {
            a.push(l.at(name))?;
        }
        a.push(BASE)?;
        a.call(l.import(0))?;
        a.mov(ebx, 2)?;
        a.cmp(eax, 1)?;
        a.jne(fail)?;
        a.mov(ebp, 48)?;
        a.set_label(&mut again)?;
        // 16 MB less EBP times 64 KB: from 208 tiles to 255.
        a.mov(eax, ebp)?;
        a.shl(eax, 16)?;
        a.neg(eax)?;
        a.add(eax, 0x100_0000)?;
        a.push(PAGE_READWRITE)?;
        a.push(MEM_RESERVE)?;
        a.push(eax)?;
        a.push(0)?;
        a.call(l.import(1))?;
        a.mov(l.dword(0x38), eax)?;
        a.mov(ebx, 3)?;
        a.test(eax, eax)?;
        a.jz(fail)?;
        a.push(PAGE_READWRITE)?;
        a.push(MEM_COMMIT)?;
        a.push(0x2_0000)?;
        a.push(eax)?;
        a.call(l.import(1))?;
        a.mov(ebx, 4)?;
        a.test(eax, eax)?;
        a.jz(fail)?;
        a.add(eax, 0x1_8010)?;
        a.mov(l.dword(0x44), eax)?;
        a.push(0)?;
        a.push(0)?;
        a.push(l.at(0x40))?;
        a.call(l.dword(0x30))?;
        a.mov(eax, l.dword(0x38))?;
        a.mov(ebx, 5)?;
        a.cmp(word_ptr(eax + 0x1_8010), 0x5678)?;
        a.jne(fail)?;
        a.mov(ebx, 6)?;
        a.cmp(word_ptr(l.at(0x48) as u64), 0x8010)?;
        a.jne(fail)?;
        a.push(MEM_RELEASE)?;
        a.push(0)?;
        a.push(l.dword(0x38))?;
        a.call(l.import(2))?;
        a.mov(ebx, 7)?;
        a.test(eax, eax)?;
        a.jz(fail)?;
        a.dec(ebp)?;
        a.jnz(again)?;
        a.xor(ebx, ebx)?;
        a.set_label(&mut fail)?;
        a.push(ebx)?;
        a.call(l.import(3))?;
        a.int3()
    });
    assert!(exe.len() <= size);
    exe.resize(size, 0);
    exe
}

/// The tiled selectors of a region VirtualFree released are free again
/// once the program asks for another region's: 48 regions of 208 to 255
/// tiles need more of them than the LDT's 8192 entries.
#[test]
fn a_released_region_s_tiled_selectors_are_freed() {
    let (name, size) = win32s_program();
    let more = [("UT16.DLL", ut_dll16_pointer())];
    let exe = released_regions_program(size);
    let (code, text) = run_as("region_released", name, exe, &more, true);
    assert_eq!(code, 0, "{}", text);
}
