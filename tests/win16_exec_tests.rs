//! Win16 KERNEL's WinExec: the programs a Windows 3.1 program starts on
//! the built-in Windows, and where their DLLs are found. The programs
//! are made byte by byte (tests/win16).

mod win16;

use rust_dos_ne as ne;
use std::fs;
use win16::{CODE, Code, program, run_with};

/// KERNEL's ordinals.
const WIN_EXEC: u16 = 166;

/// A program named `name` that imports the DLL `dll` (calling nothing in
/// it) and ends at once with exit code 1.
fn importer(name: &str, dll: &str) -> Vec<u8> {
    let mut b = ne::Builder::new(name, ne::MULTIPLEDATA);
    b.import_module(dll);
    let text = b.segment(ne::SegmentSpec {
        data: vec![0xB8, 0x01, 0x4C, 0xCD, 0x21], // mov ax,4C01h; int 21h
        flags: CODE,
        alloc: 0,
        relocations: Vec::new(),
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
    // WinExec("C:/SUB/CHILD.EXE"), whose HELPER.DLL is in C:\SUB only:
    // exit code 1 if it starts, else WinExec's error (0Bh when the DLL
    // isn't found).
    let mut code = Code::new();
    let path_at = code.here() + 2;
    code.push_far(0)
        .push(1) // SW_SHOWNORMAL
        .call("KERNEL", WIN_EXEC)
        .emit(&[
            0x3D, 0x20, 0x00, // cmp ax,20h
            0x72, 0x02, // jb over the next
            0xB0, 0x01, // mov al,1
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
