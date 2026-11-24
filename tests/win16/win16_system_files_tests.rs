//! The built-in 16-bit system modules as files in C:\WINDOWS\SYSTEM, as
//! Windows 95 and 98 keep them: a program finds KRNL386.EXE, USER.EXE,
//! GDI.EXE and MMSYSTEM.DLL there with OpenFile, reads their version,
//! and a copy of one loads as the built-in module. The programs are made
//! byte by byte (tests/common/win16).

use crate::win16;

use rust_dos_ne as ne;
use std::fs;
use win16::{CODE, Code, program, run, run_with};

/// KERNEL's ordinals, and VER's.
const GET_PROC_ADDRESS: u16 = 50;
const OPEN_FILE: u16 = 74;
const LSTRLEN: u16 = 90;
const LOAD_LIBRARY: u16 = 95;
const WIN_EXEC: u16 = 166;
const GET_FILE_VERSION_INFO_SIZE: u16 = 6;

/// OpenFile's flag that only asks whether the file is there.
const OF_EXIST: u16 = 0x4000;

/// Put `text`, ended by a zero, after the code, and its offset at the
/// places `at`.
fn strings(code: &mut Code, texts: &[(u16, &str)]) {
    for &(at, text) in texts {
        let here = code.here();
        code.patch(at, here);
        code.emit(text.as_bytes()).emit(&[0]);
    }
}

/// A program that looks for `name` with OpenFile(OF_EXIST), as
/// AAPLAY.DLL looks for MMSYSTEM.DLL, and ends with the length of the
/// path OpenFile found it at, or 1 if it found it nowhere.
fn open_file(name: &str) -> Vec<u8> {
    let mut code = Code::new();
    let at = code.here() + 2;
    code.push_far(0);
    // The OFSTRUCT at DS:20h, its path at 28h.
    code.emit(&[0x1E]).push(0x20).push(OF_EXIST);
    code.call("KERNEL", OPEN_FILE);
    code.emit(&[
        0x3D, 0xFF, 0xFF, // cmp ax,-1
        0x75, 0x06, // jne found
        0xB0, 0x01, 0xB4, 0x4C, 0xCD, 0x21, // mov al,1; exit
        0x1E, // found: push ds
    ]);
    code.push(0x28).call("KERNEL", LSTRLEN).exit_al();
    strings(&mut code, &[(at, name)]);
    program(&code, CODE)
}

/// OpenFile finds the 16-bit system modules in the system directory,
/// where Windows 95 and 98 have them.
#[test]
fn the_system_modules_are_files_in_the_system_directory() {
    for (name, path) in [
        ("MMSYSTEM.DLL", "C:\\WINDOWS\\SYSTEM\\MMSYSTEM.DLL"),
        ("KRNL386.EXE", "C:\\WINDOWS\\SYSTEM\\KRNL386.EXE"),
        ("USER.EXE", "C:\\WINDOWS\\SYSTEM\\USER.EXE"),
        ("GDI.EXE", "C:\\WINDOWS\\SYSTEM\\GDI.EXE"),
        ("COMMDLG.DLL", "C:\\WINDOWS\\SYSTEM\\COMMDLG.DLL"),
    ] {
        let test = format!("system_file_{}", name.replace('.', "_"));
        let (exit, log) = run(&test, open_file(name));
        assert_eq!(exit as usize, path.len(), "{name}: {log}");
    }
}

/// VER's GetFileVersionInfoSize finds a version resource in MMSYSTEM.DLL.
/// The program ends with 2 if it does, 1 if not.
#[test]
fn a_system_module_has_a_version() {
    let mut code = Code::new();
    let at = code.here() + 2;
    code.push_far(0);
    code.emit(&[0x1E]).push(0x10);
    code.call("VER", GET_FILE_VERSION_INFO_SIZE);
    code.emit(&[
        0x0B, 0xC0, // or ax,ax
        0xB0, 0x01, // mov al,1
        0x74, 0x02, // jz end
        0xB0, 0x02, // mov al,2
    ]);
    code.exit_al();
    strings(&mut code, &[(at, "MMSYSTEM.DLL")]);
    let (exit, log) = run("system_file_version", program(&code, CODE));
    assert_eq!(exit, 2, "{log}");
}

/// A program that loads `first`, copies the file `source` to `copy` with
/// DOS's calls, loads `copy` and ends with 2 if it got the same module
/// as for `first`, 3 if another one, or 1 if the load failed.
fn load_copy(first: &str, source: &str, copy: &str) -> Vec<u8> {
    let mut code = Code::new();
    let mut texts = Vec::new();
    let at = code.here() + 2;
    code.push_far(0).call("KERNEL", LOAD_LIBRARY);
    texts.push((at, first));
    code.emit(&[0x50]); // push ax
    // push ds; push cs; pop ds; mov dx,source; mov ax,3D00h; int 21h;
    // pop ds; mov si,ax.
    code.emit(&[0x1E, 0x0E, 0x1F, 0xBA]);
    texts.push((code.here(), source));
    code.emit(&[0, 0, 0xB8, 0x00, 0x3D, 0xCD, 0x21, 0x1F, 0x8B, 0xF0]);
    // The same with mov dx,copy; xor cx,cx; mov ah,3Ch: di the copy.
    code.emit(&[0x1E, 0x0E, 0x1F, 0xBA]);
    texts.push((code.here(), copy));
    code.emit(&[0, 0, 0x33, 0xC9, 0xB4, 0x3C, 0xCD, 0x21, 0x1F, 0x8B, 0xF8]);
    let next = code.here();
    code.emit(&[
        0xB4, 0x3F, 0x8B, 0xDE, // mov ah,3Fh; mov bx,si
        0xB9, 0x80, 0x00, 0xBA, 0x20, 0x00, // mov cx,80h; mov dx,20h
        0xCD, 0x21, // int 21h
        0x72, 0x11, // jc done
        0x0B, 0xC0, 0x74, 0x0D, // or ax,ax; jz done
        0x8B, 0xC8, 0xB4, 0x40, 0x8B, 0xDF, // mov cx,ax; mov ah,40h; mov bx,di
        0xBA, 0x20, 0x00, 0xCD, 0x21, // mov dx,20h; int 21h
        0xEB, // jmp next
    ]);
    let back = next as i32 - (code.here() as i32 + 1);
    code.emit(&[back as i8 as u8]);
    code.emit(&[
        0xB4, 0x3E, 0x8B, 0xDE, 0xCD, 0x21, // done: close si
        0xB4, 0x3E, 0x8B, 0xDF, 0xCD, 0x21, // close di
    ]);
    let at = code.here() + 2;
    code.push_far(0).call("KERNEL", LOAD_LIBRARY);
    texts.push((at, copy));
    code.emit(&[
        0x5B, // pop bx
        0x3D, 0x20, 0x00, // cmp ax,20h
        0x72, 0x0A, // jb failed
        0x3B, 0xC3, // cmp ax,bx
        0x75, 0x0C, // jne another
        0xB0, 0x02, 0xB4, 0x4C, 0xCD, 0x21, // mov al,2; exit
        0xB0, 0x01, 0xB4, 0x4C, 0xCD, 0x21, // failed: mov al,1; exit
        0xB0, 0x03, 0xB4, 0x4C, 0xCD, 0x21, // another: mov al,3; exit
    ]);
    strings(&mut code, &texts);
    program(&code, CODE)
}

/// A system module's file holds stubs of the built-in functions, which
/// only the built-in module runs: a copy of COMMDLG.DLL under another
/// name, loaded by its path, is the built-in COMMDLG already loaded.
#[test]
fn a_copy_of_a_system_module_loads_the_built_in_module() {
    let exe = load_copy(
        "COMMDLG.DLL",
        "C:\\WINDOWS\\SYSTEM\\COMMDLG.DLL",
        "C:\\MYDLG.DLL",
    );
    let (exit, log) = run("system_file_copy", exe);
    assert_eq!(exit, 2, "{log}");
}

/// LoadLibrary of a system module's file by its path in the system
/// directory gives the built-in module, as its name alone does, even
/// where a program put a file of its own of that name there: here an
/// LZEXPAND.DLL that exports nothing. The program ends with 2 if the
/// module it got has LZOpenFile, 3 if not, or 1 if the load failed.
#[test]
fn a_system_module_loads_by_its_path() {
    let mut code = Code::new();
    let mut texts = Vec::new();
    let at = code.here() + 2;
    code.push_far(0).call("KERNEL", LOAD_LIBRARY);
    texts.push((at, "C:\\WINDOWS\\SYSTEM\\LZEXPAND.DLL"));
    code.emit(&[
        0x3D, 0x20, 0x00, // cmp ax,20h
        0x73, 0x06, // jae loaded
        0xB0, 0x01, 0xB4, 0x4C, 0xCD, 0x21, // mov al,1; exit
        0x50, // loaded: push ax
    ]);
    let at = code.here() + 2;
    code.push_far(0).call("KERNEL", GET_PROC_ADDRESS);
    texts.push((at, "LZOPENFILE"));
    code.emit(&[
        0x0B, 0xC2, // or ax,dx
        0xB0, 0x03, // mov al,3
        0x74, 0x02, // jz end
        0xB0, 0x02, // mov al,2
    ]);
    code.exit_al();
    strings(&mut code, &texts);
    let (exit, log) = run_with("system_file_path", program(&code, CODE), |_, base| {
        let mut b = ne::Builder::new("LZEXPAND", ne::LIBMODULE);
        b.segment(ne::SegmentSpec {
            data: vec![0xCB], // retf
            flags: CODE,
            alloc: 0,
            relocations: Vec::new(),
        });
        let dir = base.join("WINDOWS").join("SYSTEM");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("LZEXPAND.DLL"), b.build()).unwrap();
    });
    assert_eq!(exit, 2, "{log}");
}

/// KERNEL, USER and GDI are programs by their files' flags, but WinExec
/// of their files starts nothing: the built-in modules have no program
/// code. The program ends with 2 if WinExec failed, 3 if it started a
/// task.
#[test]
fn a_system_module_file_does_not_start() {
    let mut code = Code::new();
    let at = code.here() + 2;
    code.push_far(0).push(1).call("KERNEL", WIN_EXEC);
    code.emit(&[
        0x3D, 0x20, 0x00, // cmp ax,20h
        0xB0, 0x02, // mov al,2
        0x72, 0x02, // jb end
        0xB0, 0x03, // mov al,3
    ]);
    code.exit_al();
    strings(&mut code, &[(at, "C:\\WINDOWS\\SYSTEM\\USER.EXE")]);
    let (exit, log) = run("system_file_winexec", program(&code, CODE));
    assert_eq!(exit, 2, "{log}");
}
