//! Win16 KERNEL's WinExec: the programs a Windows 3.1 program starts on
//! the built-in Windows, and where their DLLs are found. The programs
//! are made byte by byte (tests/win16).

mod win16;

use rust_dos_ne as ne;
use rust_dos_pe as pe;
use std::fs;
use win16::{CODE, Code, program, run_with};

/// KERNEL's ordinals.
const GET_MODULE_FILE_NAME: u16 = 49;
const WIN_EXEC: u16 = 166;

/// A program named `name` that imports the DLL `dll` (calling nothing in
/// it) and ends at once: with exit code 1 if GetModuleFileName gives its
/// path with backslashes only, as Windows keeps it, or 2 if it has a `/`.
fn importer(name: &str, dll: &str) -> Vec<u8> {
    let mut b = ne::Builder::new(name, ne::MULTIPLEDATA);
    let kernel = b.import_module("KERNEL");
    b.import_module(dll);
    let code = vec![
        0x57, // push di: the instance
        0x1E, // push ds
        0x68, 0x20, 0x00, // push 20h: the buffer
        0x68, 0x80, 0x00, // push 80h: its size
        0x9A, 0xFF, 0xFF, 0x00, 0x00, // call far GetModuleFileName
        0xBE, 0x20, 0x00, // mov si,20h
        0xAC, // next: lodsb
        0x3C, b'/', // cmp al,'/'
        0x74, 0x08, // je slash
        0x0A, 0xC0, // or al,al
        0x75, 0xF7, // jnz next
        0xB0, 0x01, // mov al,1
        0xEB, 0x02, // jmp end
        0xB0, 0x02, // slash: mov al,2
        0xB4, 0x4C, 0xCD, 0x21, // end: mov ah,4Ch; int 21h
    ];
    let text = b.segment(ne::SegmentSpec {
        data: code,
        flags: CODE,
        alloc: 0,
        relocations: vec![ne::Relocation {
            source: ne::ADDR_FARPTR,
            additive: false,
            offset: 9,
            target: ne::Target::Ordinal {
                module: kernel,
                ordinal: GET_MODULE_FILE_NAME,
            },
        }],
    });
    let data = b.segment(ne::SegmentSpec {
        data: Vec::new(),
        flags: ne::SEG_DATA | 0x0C00,
        alloc: 0x100,
        relocations: Vec::new(),
    });
    b.auto_data(data, 0x400, 0x800);
    b.entry_point(text, 0);
    b.initial_stack(data, 0);
    b.build()
}

/// A DLL named `name` whose LibMain returns 1.
fn library(name: &str) -> Vec<u8> {
    let mut b = ne::Builder::new(name, ne::LIBMODULE);
    let text = b.segment(ne::SegmentSpec {
        data: vec![0xB8, 0x01, 0x00, 0xCB], // mov ax,1; retf
        flags: CODE,
        alloc: 0,
        relocations: Vec::new(),
    });
    b.entry_point(text, 0);
    b.build()
}

#[test]
fn a_program_started_with_a_slash_path_finds_its_dlls_beside_it() {
    // WinExec("C:/SUB/CHILD.EXE"), whose HELPER.DLL is in C:\SUB only.
    // The child ends after the program, so the exit code is its own: 1
    // when it started and has its path as Windows keeps it. The program
    // ends with WinExec's error if it fails (0Bh when the DLL isn't
    // found), or 10h.
    let mut code = Code::new();
    let path_at = code.here() + 2;
    code.push_far(0)
        .push(1) // SW_SHOWNORMAL
        .call("KERNEL", WIN_EXEC)
        .emit(&[
            0x3D, 0x20, 0x00, // cmp ax,20h
            0x72, 0x02, // jb over the next
            0xB0, 0x10, // mov al,10h
        ])
        .exit_al();
    let path = code.here();
    code.emit(b"C:/SUB/CHILD.EXE\0");
    code.patch(path_at, path);
    let (exit, log) = run_with("slash_path", program(&code, CODE), |_, base| {
        let sub = base.join("SUB");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("CHILD.EXE"), importer("CHILD", "HELPER")).unwrap();
        fs::write(sub.join("HELPER.DLL"), library("HELPER")).unwrap();
    });
    assert_eq!(exit, 1, "{log}");
}

/// A Win32 program whose entry point returns `code`, its exit code.
fn win32_program(code: u8) -> Vec<u8> {
    let mut b = pe::Builder::exe(0x40_0000);
    b.subsystem(pe::SUBSYSTEM_GUI);
    let text = [0xB8, code, 0x00, 0x00, 0x00, 0xC3]; // mov eax,code; ret
    let rva = b.section(".text", &text, pe::SCN_CODE | pe::SCN_EXECUTE | pe::SCN_READ);
    b.entry(rva);
    b.build()
}

#[test]
fn a_program_starts_a_win32_program() {
    // WinExec("C:\WIN32.EXE"): on Windows 98 a Windows 3.1 program
    // starts a Win32 one as a process of its own, as InstallShield's
    // setup launcher starts its engine, and WinExec gives 33. The program
    // ends at once with 10h if WinExec gave anything else, else with 1,
    // which DOS gets: it is the process DOS started. The Win32 program
    // runs on and ends with 9.
    let mut code = Code::new();
    let path_at = code.here() + 2;
    code.push_far(0)
        .push(1) // SW_SHOWNORMAL
        .call("KERNEL", WIN_EXEC)
        .emit(&[
            0x3D, 0x21, 0x00, // cmp ax,33
            0xB0, 0x01, // mov al,1
            0x74, 0x02, // je over the next
            0xB0, 0x10, // mov al,10h
        ])
        .exit_al();
    let path = code.here();
    code.emit(b"C:\\WIN32.EXE\0");
    code.patch(path_at, path);
    let (exit, log) = run_with("win32_child", program(&code, CODE), |_, base| {
        fs::write(base.join("WIN32.EXE"), win32_program(9)).unwrap();
    });
    assert_eq!(exit, 1, "{log}");
    assert!(log.contains("WIN32.EXE ended with exit code 9"), "{log}");
}
