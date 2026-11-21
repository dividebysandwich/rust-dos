//! Win16 KERNEL's WinExec: the programs a Windows 3.1 program starts on
//! the built-in Windows, and where their DLLs are found. The programs
//! are made byte by byte (tests/common/win16).

use crate::win16;

use rust_dos_ne as ne;
use rust_dos_pe as pe;
use std::fs;
use win16::{CODE, Code, dos_marker, program, run_command, run_with};

/// KERNEL's ordinals.
const GET_MODULE_FILE_NAME: u16 = 49;
const LOAD_LIBRARY: u16 = 95;
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
    let rva = b.section(
        ".text",
        &text,
        pe::SCN_CODE | pe::SCN_EXECUTE | pe::SCN_READ,
    );
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

#[test]
fn the_prompt_starts_a_screen_saver_by_its_scr_name() {
    // A screen saver is a Windows program named .SCR, which Windows
    // starts with /s to run it and /c for its settings. With the
    // built-in Windows on, the prompt starts SAVER.SCR as Windows would.
    // It ends with 7 when its command tail (ES is its PSP) is " /s", else
    // with 1.
    let mut code = Code::new();
    code.emit(&[
        0x26, 0x81, 0x3E, 0x81, 0x00, b' ', b'/', // cmp word es:[81h],'/ '
        0xB0, 0x01, // mov al,1
        0x75, 0x02, // jne over the next
        0xB0, 0x07, // mov al,7
    ])
    .exit_al();
    let saver = program(&code, CODE);
    let (exit, log) = run_command("scr_prompt", Vec::new(), "SAVER.SCR", "/s", |_, base| {
        fs::write(base.join("SAVER.SCR"), saver).unwrap();
    });
    assert_eq!(exit, 7, "{log}");
}

#[test]
fn the_prompt_runs_no_scr_without_the_builtin_windows() {
    // Without the built-in Windows, a .SCR is no program to COMMAND.COM.
    let base = std::path::PathBuf::from("target/test_win16/scr_no_windows");
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    fs::write(base.join("SAVER.SCR"), program(Code::new().exit_al(), CODE)).unwrap();
    let cpu = rust_dos::cpu::Cpu::new(base);
    assert_eq!(rust_dos::exec::find_program(&cpu, "SAVER.SCR"), None);
}

#[test]
fn a_command_with_non_ascii_bytes_is_looked_up() {
    // DOS bytes above 127 reach the shell as chars of two bytes, so a
    // name's last four bytes don't always start a char. Such a name has
    // no program, with the built-in Windows on or off.
    let base = std::path::PathBuf::from("target/test_win16/non_ascii_command");
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    let mut cpu = rust_dos::cpu::Cpu::new(base);
    for enabled in [false, true] {
        cpu.bus.win32.enabled = enabled;
        assert_eq!(rust_dos::exec::find_program(&cpu, "\u{e9}a12"), None);
    }
}

#[test]
fn a_program_with_a_loaded_module_name_runs_its_own_code() {
    // The program, module T, runs C:\SUB\CHILD.EXE, another program whose
    // module is named T too, as Sierra's setup runs its copy _SETUP.EXE.
    // That file loads as a module of its own and ends with 7. Taken for
    // a second instance of the program, it would run the program's code
    // again instead.
    let mut code = Code::new();
    let path_at = code.here() + 2;
    code.push_far(0)
        .push(1) // SW_SHOWNORMAL
        .call("KERNEL", WIN_EXEC)
        .emit(&[
            0x3D, 0x20, 0x00, // cmp ax,20h
            0xB0, 0x01, // mov al,1
            0x73, 0x02, // jae over the next
            0xB0, 0x10, // mov al,10h
        ])
        .exit_al();
    let path = code.here();
    code.emit(b"C:\\SUB\\CHILD.EXE\0");
    code.patch(path_at, path);
    let mut child = Code::new();
    child.emit(&[0xB0, 0x07]).exit_al(); // mov al,7
    let child = program(&child, CODE);
    let (exit, log) = run_with("same_module_name", program(&code, CODE), |_, base| {
        let sub = base.join("SUB");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("CHILD.EXE"), child).unwrap();
    });
    assert_eq!(exit, 7, "{log}");
}

#[test]
fn a_library_with_a_loaded_module_name_loads_from_its_file() {
    // The program, module T, loads C:\RES.DLL, a library whose module is
    // named T too, as Sierra's setup loads its resources from SETUPL.DLL,
    // module SETUP. LoadLibrary gives the library's own instance, not the
    // program's (DI at the start): the program ends with 1, or 10h if
    // LoadLibrary gave its own instance or an error.
    let mut code = Code::new();
    code.emit(&[0x8B, 0xDF]); // mov bx,di
    let path_at = code.here() + 2;
    code.push_far(0).call("KERNEL", LOAD_LIBRARY).emit(&[
        0x3B, 0xC3, // cmp ax,bx
        0x74, 0x09, // je bad
        0x3D, 0x20, 0x00, // cmp ax,20h
        0x72, 0x04, // jb bad
        0xB0, 0x01, // mov al,1
        0xEB, 0x02, // jmp end
        0xB0, 0x10, // bad: mov al,10h
    ]);
    code.exit_al();
    let path = code.here();
    code.emit(b"C:\\RES.DLL\0");
    code.patch(path_at, path);
    let (exit, log) = run_with("same_library_name", program(&code, CODE), |_, base| {
        fs::write(base.join("RES.DLL"), library("T")).unwrap();
    });
    assert_eq!(exit, 1, "{log}");
}

#[test]
fn a_file_with_a_loaded_file_name_is_that_module() {
    // C:\RES.DLL (module R) loads, then C:\SUB\RES.DLL (module Q) is
    // asked for. KERNEL finds a loaded module by the base name of the
    // file asked for, wherever the file is, so the second LoadLibrary
    // gives the first library's instance: the program ends with 1, or
    // 10h if it got another instance or an error.
    let mut code = Code::new();
    let first_at = code.here() + 2;
    code.push_far(0)
        .call("KERNEL", LOAD_LIBRARY)
        .emit(&[0x8B, 0xD8]); // mov bx,ax
    let second_at = code.here() + 2;
    code.push_far(0).call("KERNEL", LOAD_LIBRARY).emit(&[
        0x3B, 0xC3, // cmp ax,bx
        0x75, 0x09, // jne bad
        0x3D, 0x20, 0x00, // cmp ax,20h
        0x72, 0x04, // jb bad
        0xB0, 0x01, // mov al,1
        0xEB, 0x02, // jmp end
        0xB0, 0x10, // bad: mov al,10h
    ]);
    code.exit_al();
    let first = code.here();
    code.emit(b"C:\\RES.DLL\0");
    code.patch(first_at, first);
    let second = code.here();
    code.emit(b"C:\\SUB\\RES.DLL\0");
    code.patch(second_at, second);
    let (exit, log) = run_with("same_file_name", program(&code, CODE), |_, base| {
        fs::write(base.join("RES.DLL"), library("R")).unwrap();
        let sub = base.join("SUB");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("RES.DLL"), library("Q")).unwrap();
    });
    assert_eq!(exit, 1, "{log}");
}

#[test]
fn a_program_starts_a_dos_program_and_sees_what_it_wrote() {
    // WinExec("C:\DOSPROG.COM /X"): Windows 98 runs a DOS program in a
    // DOS box, on the same drives, and WinExec gives 33 or more. The
    // built-in Windows runs it full screen while the Windows programs
    // wait, so when WinExec returns, the file it writes is there. The DOS
    // program changes to C:\SUB, which on Windows is its DOS machine's
    // own current directory: the program opens MARK.TXT in its own, C:\.
    // It ends with 1 when it can open that file, 2 when it can't, and 10h
    // when WinExec didn't give 33.
    let mut code = Code::new();
    let path_at = code.here() + 2;
    code.push_far(0).push(1).call("KERNEL", WIN_EXEC);
    let mark_at = code.here() + 10;
    code.emit(&[
        0x3D, 0x21, 0x00, // cmp ax,33
        0xB0, 0x10, // mov al,10h
        0x75, 0x10, // jne end
        0x0E, 0x1F, // push cs; pop ds
        0xBA, 0xFF, 0xFF, // mov dx,mark
        0xB8, 0x00, 0x3D, // mov ax,3D00h
        0xCD, 0x21, // int 21h
        0xB0, 0x01, // mov al,1
        0x73, 0x02, // jnc end
        0xB0, 0x02, // mov al,2
    ])
    .exit_al();
    let path = code.here();
    code.emit(b"C:\\DOSPROG.COM /X\0");
    code.patch(path_at, path);
    let mark = code.here();
    code.emit(b"MARK.TXT\0");
    code.patch(mark_at, mark);
    let mut folder = None;
    let (exit, log) = run_with("dos_child", program(&code, CODE), |_, base| {
        fs::write(base.join("DOSPROG.COM"), dos_marker()).unwrap();
        fs::create_dir_all(base.join("SUB")).unwrap();
        folder = Some(base.to_path_buf());
    });
    assert_eq!(exit, 1, "{log}");
    assert!(log.contains("DOSPROG.COM ended with exit code 5"), "{log}");
    let written = fs::read(folder.unwrap().join("MARK.TXT")).unwrap();
    // Windows 4.x, and the command tail with its blank.
    assert_eq!(written[0], 4, "{written:?}");
    assert_eq!(&written[2..], b" /X");
}

/// KERNEL's FreeLibrary and GetModuleHandle.
const FREE_LIBRARY: u16 = 96;
const GET_MODULE_HANDLE: u16 = 47;

/// The places a test puts copies of a DLL in, by the length of the path
/// GetModuleFileName gives for each: the current directory (C:\, where
/// the shell starts the program), the Windows directory, the system
/// directory, and C:\APP, where the program is.
const IN_CURRENT: u8 = 11; // C:\PICK.DLL
const IN_WINDOWS: u8 = 19; // C:\WINDOWS\PICK.DLL
const IN_SYSTEM: u8 = 26; // C:\WINDOWS\SYSTEM\PICK.DLL
const IN_APP: u8 = 15; // C:\APP\PICK.DLL

/// Code that ends the program with the length of the path of the module
/// whose handle or instance is in AX, or with 1 if AX is below 32.
fn exit_with_path_length(code: &mut Code) {
    code.emit(&[
        0x3D, 0x20, 0x00, // cmp ax,20h
        0x73, 0x06, // jae over the exit
        0xB0, 0x01, 0xB4, 0x4C, 0xCD, 0x21, // mov al,1; exit
        0x50, // push ax
        0x1E, // push ds
        0x68, 0x20, 0x00, // push 20h: the buffer
        0x68, 0x80, 0x00, // push 80h: its size
    ]);
    code.call("KERNEL", GET_MODULE_FILE_NAME).exit_al();
}

/// A program, started as C:\APP\T.EXE with C:\ the current directory,
/// that first loads and frees each of `first` (paths), then loads `name`
/// and ends with the length of its path. `places` are the directories
/// (under C:\) that get a copy of `name`'s DLL.
fn search(test: &str, first: &[&str], name: &str, places: &[&str]) -> (u8, String) {
    search_after(test, first, &[], name, places)
}

/// `search`, with the files `gone` (paths) deleted after the loads of
/// `first`.
fn search_after(
    test: &str,
    first: &[&str],
    gone: &[&str],
    name: &str,
    places: &[&str],
) -> (u8, String) {
    let mut code = Code::new();
    let mut strings = Vec::new();
    for path in first {
        let at = code.here() + 2;
        code.push_far(0).call("KERNEL", LOAD_LIBRARY);
        code.emit(&[0x50]).call("KERNEL", FREE_LIBRARY); // push ax
        strings.push((at, *path));
    }
    for path in gone {
        // push ds; push cs; pop ds; mov dx,path; mov ah,41h; int 21h;
        // pop ds.
        code.emit(&[0x1E, 0x0E, 0x1F, 0xBA]);
        let at = code.here();
        code.emit(&[0, 0, 0xB4, 0x41, 0xCD, 0x21, 0x1F]);
        strings.push((at, *path));
    }
    let at = code.here() + 2;
    code.push_far(0).call("KERNEL", LOAD_LIBRARY);
    strings.push((at, name));
    exit_with_path_length(&mut code);
    for (at, text) in strings {
        let here = code.here();
        code.patch(at, here);
        code.emit(text.as_bytes()).emit(&[0]);
    }
    let exe = program(&code, CODE);
    let copy = exe.clone();
    let file = name.rsplit('\\').next().unwrap().to_string();
    run_command(test, exe, "APP\\T.EXE", "", move |_, base| {
        fs::create_dir_all(base.join("APP")).unwrap();
        fs::write(base.join("APP").join("T.EXE"), copy).unwrap();
        for place in places {
            let dir = base.join(place);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join(&file), library("PICK")).unwrap();
        }
    })
}

#[test]
fn the_current_directory_comes_before_the_windows_directory() {
    let places = ["", "WINDOWS", "WINDOWS/SYSTEM", "APP"];
    let (exit, log) = search("search_current", &[], "PICK.DLL", &places);
    assert_eq!(exit, IN_CURRENT, "{log}");
}

#[test]
fn the_windows_directory_comes_before_the_system_directory() {
    let places = ["WINDOWS", "WINDOWS/SYSTEM", "APP"];
    let (exit, log) = search("search_windows", &[], "PICK.DLL", &places);
    assert_eq!(exit, IN_WINDOWS, "{log}");
}

/// Windows 3.1 looks in the directory of the current task's executable
/// only after the system directory, so a program's older copy of a
/// system DLL beside it isn't the one loaded.
#[test]
fn the_system_directory_comes_before_the_programs_directory() {
    let places = ["WINDOWS/SYSTEM", "APP"];
    let (exit, log) = search("search_system", &[], "PICK.DLL", &places);
    assert_eq!(exit, IN_SYSTEM, "{log}");
}

#[test]
fn the_programs_directory_is_searched() {
    let (exit, log) = search("search_app", &[], "PICK.DLL", &["APP"]);
    assert_eq!(exit, IN_APP, "{log}");
}

/// Windows 95 makes a DLL loaded from the system directory one of
/// Known16DLLs, and looks for those there first, before the current
/// directory: once C:\WINDOWS\SYSTEM\PICK.DLL was loaded, C:\PICK.DLL
/// isn't the one LoadLibrary("PICK.DLL") loads.
#[test]
fn a_known_dll_is_looked_for_in_the_system_directory_first() {
    let places = ["", "WINDOWS/SYSTEM"];
    let first = ["C:\\WINDOWS\\SYSTEM\\PICK.DLL"];
    let (exit, log) = search("search_known", &first, "PICK.DLL", &places);
    assert_eq!(exit, IN_SYSTEM, "{log}");
    // Without it, the current directory's comes first.
    let (exit, log) = search("search_unknown", &[], "PICK.DLL", &places);
    assert_eq!(exit, IN_CURRENT, "{log}");
}

/// A value of Known16DLLs stays when its file leaves the system
/// directory, and Windows 95 then looks in the Windows directory before
/// the current one (Microsoft's articles Q141969 and Q149430): the
/// system directory, the Windows directory, the current directory, the
/// task's directory, PATH.
#[test]
fn a_known_dll_moved_away_is_looked_for_in_the_windows_directory_next() {
    let places = ["", "WINDOWS", "WINDOWS/SYSTEM"];
    let system = "C:\\WINDOWS\\SYSTEM\\PICK.DLL";
    let (exit, log) = search_after("search_moved", &[system], &[system], "PICK.DLL", &places);
    assert_eq!(exit, IN_WINDOWS, "{log}");
}

/// A DLL that imports another is in C:\LIBS, with a copy of the other
/// beside it and one in C:\APP, the program's directory. Windows looks
/// for the import in the current task's directory, not in that of the
/// DLL that imports it, so C:\APP\INNER.DLL is loaded (16 characters,
/// where C:\LIBS\INNER.DLL has 17).
#[test]
fn a_dlls_import_is_looked_for_in_the_tasks_directory() {
    let mut code = Code::new();
    let outer_at = code.here() + 2;
    code.push_far(0).call("KERNEL", LOAD_LIBRARY);
    code.emit(&[
        0x3D, 0x20, 0x00, // cmp ax,20h
        0x73, 0x06, // jae over the exit
        0xB0, 0x02, 0xB4, 0x4C, 0xCD, 0x21, // mov al,2; exit
    ]);
    let inner_at = code.here() + 2;
    code.push_far(0).call("KERNEL", GET_MODULE_HANDLE);
    exit_with_path_length(&mut code);
    let here = code.here();
    code.patch(outer_at, here);
    code.emit(b"C:\\LIBS\\OUTER.DLL\0");
    let here = code.here();
    code.patch(inner_at, here);
    code.emit(b"INNER\0");
    let exe = program(&code, CODE);
    let copy = exe.clone();
    let (exit, log) = run_command("search_import", exe, "APP\\T.EXE", "", move |_, base| {
        let (app, libs) = (base.join("APP"), base.join("LIBS"));
        fs::create_dir_all(&app).unwrap();
        fs::create_dir_all(&libs).unwrap();
        fs::write(app.join("T.EXE"), copy).unwrap();
        fs::write(libs.join("OUTER.DLL"), library_importing("OUTER", "INNER")).unwrap();
        fs::write(libs.join("INNER.DLL"), library("INNER")).unwrap();
        fs::write(app.join("INNER.DLL"), library("INNER")).unwrap();
    });
    assert_eq!(exit, 16, "{log}");
}

/// A DLL named `name` that imports the DLL `dll`, whose LibMain returns
/// 1.
fn library_importing(name: &str, dll: &str) -> Vec<u8> {
    let mut b = ne::Builder::new(name, ne::LIBMODULE);
    b.import_module(dll);
    let text = b.segment(ne::SegmentSpec {
        data: vec![0xB8, 0x01, 0x00, 0xCB], // mov ax,1; retf
        flags: CODE,
        alloc: 0,
        relocations: Vec::new(),
    });
    b.entry_point(text, 0);
    b.build()
}
