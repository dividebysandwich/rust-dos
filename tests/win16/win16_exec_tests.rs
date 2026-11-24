//! Win16 KERNEL's WinExec: the programs a Windows 3.1 program starts on
//! the built-in Windows, and where their DLLs are found. The programs
//! are made byte by byte (tests/common/win16).

use crate::win16;

use rust_dos::disk::MountOptions;
use rust_dos_ne as ne;
use rust_dos_pe as pe;
use std::fs;
use win16::{CODE, Code, dos_marker, program, program_named, run_command, run_with};

/// KERNEL's ordinals.
const GET_MODULE_FILE_NAME: u16 = 49;
const LOAD_LIBRARY: u16 = 95;
const WIN_EXEC: u16 = 166;
const YIELD: u16 = 29;
const WAIT_EVENT: u16 = 30;

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
    // The child runs and ends before WinExec returns: with 1 when it
    // started and has its path as Windows keeps it. The program then
    // ends with WinExec's error if it failed (0Bh when the DLL isn't
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
    assert_eq!(exit, 0x10, "{log}");
    assert!(ended_with(&log, 1), "{log}");
}

/// Whether a task ended with the exit code `code`, as the log says.
fn ended_with(log: &str, code: u8) -> bool {
    let line = format!(" ended with exit code {:X}", code);
    log.lines()
        .any(|l| l.starts_with("[W16] Task ") && l.ends_with(&line))
}

#[test]
fn a_program_loaded_again_after_its_task_ended_runs_in_its_own_data_segment() {
    // The program runs C:\CHILD.EXE, waits (Yield) until its module is
    // gone, and runs it again. The second CHILD is a new module, its
    // first instance, which gets the first one's handles: it runs in the
    // module's own automatic data segment, as Windows runs a module's
    // first instance. It ends with 7 when DS is the selector its code
    // was relocated with for that segment, and with 9 when it was given
    // a copy of the segment, as the second instance of a module that is
    // still running is. The program waits for the second CHILD to end
    // too, and ends with 1 when the second WinExec gave an instance
    // handle (32 or more), 3 when it gave an error.
    const GET_MODULE_HANDLE: u16 = 47;
    const YIELD: u16 = 29;
    let mut code = Code::new();
    let mut names = Vec::new();
    // Until GetModuleHandle("CHILD") gives 0, Yield.
    let wait_for_child = |code: &mut Code, names: &mut Vec<u16>| {
        let wait = code.here();
        names.push(code.here() + 2);
        code.push_far(0).call("KERNEL", GET_MODULE_HANDLE).emit(&[
            0x0B, 0xC0, // or ax,ax
            0x74, 0x07, // jz over the next two
        ]);
        code.call("KERNEL", YIELD);
        let back = wait.wrapping_sub(code.here() + 2) as u8;
        code.emit(&[0xEB, back]); // jmp wait
    };
    let first_at = code.here() + 2;
    code.push_far(0).push(1).call("KERNEL", WIN_EXEC);
    wait_for_child(&mut code, &mut names);
    let second_at = code.here() + 2;
    code.push_far(0).push(1).call("KERNEL", WIN_EXEC);
    code.emit(&[0x8B, 0xF0]); // mov si,ax
    wait_for_child(&mut code, &mut names);
    code.emit(&[
        0x83, 0xFE, 0x20, // cmp si,32
        0xB0, 0x01, // mov al,1
        0x73, 0x02, // jae over the next
        0xB0, 0x03, // mov al,3
    ])
    .exit_al();
    let path = code.here();
    code.emit(b"C:\\CHILD.EXE\0");
    code.patch(first_at, path);
    code.patch(second_at, path);
    let name = code.here();
    code.emit(b"CHILD\0");
    for at in names {
        code.patch(at, name);
    }

    let mut child = Code::new();
    child
        .emit(&[
            0xB8, 0xFF, 0xFF, // mov ax,DGROUP
            0x8C, 0xDB, // mov bx,ds
            0x3B, 0xC3, // cmp ax,bx
            0xB0, 0x07, // mov al,7
            0x74, 0x02, // je over the next
            0xB0, 0x09, // mov al,9
        ])
        .exit_al();
    child.fixup(
        1,
        ne::ADDR_SELECTOR,
        ne::Target::Internal {
            segment: 2,
            offset: 0,
        },
    );
    let child = program_named("CHILD", &child, CODE);
    let (exit, log) = run_with("reloaded_module_data", program(&code, CODE), |_, base| {
        fs::write(base.join("CHILD.EXE"), child).unwrap();
    });
    // The second CHILD has the first one's handles, without which a copy
    // of its data segment can't be taken for the first's.
    let loads: Vec<&str> = log
        .lines()
        .filter(|l| l.contains("Loaded CHILD"))
        .map(|l| l.rsplit(' ').next().unwrap_or(""))
        .collect();
    assert_eq!(loads.len(), 2, "{log}");
    assert_eq!(loads[0], loads[1], "{log}");
    let ends: Vec<&str> = log
        .lines()
        .filter(|l| l.starts_with("[W16] Task") && l.contains("ended with exit code"))
        .map(|l| l.rsplit(' ').next().unwrap_or(""))
        .collect();
    // Both CHILDs, then the program.
    assert_eq!(ends.len(), 3, "{log}");
    assert_eq!(ends[1], "7", "{log}");
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
    // That file loads as a module of its own and ends with 7, before
    // WinExec returns. Taken for a second instance of the program, it
    // would run the program's code again instead. The program ends with
    // 1 when WinExec gave an instance.
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
    assert_eq!(exit, 1, "{log}");
    assert!(ended_with(&log, 7), "{log}");
}

#[test]
fn a_started_program_runs_until_it_yields_before_win_exec_returns() {
    // Windows 3.1's LoadModule gives the task it starts a DirectedYield,
    // and returns when the task yields back: the new program's startup
    // code runs first. Memphis Math's MEMPHIS starts its map, MMAP, and
    // at once posts it the show command through MCLIB.DLL, which finds
    // MMAP's task in a table MMAP fills as it starts; with the program
    // going on first, MMAP wasn't in it yet and never showed.
    //
    // CHILD.EXE calls WaitEvent(0), writes C:\MARK.TXT, yields and ends
    // with 7. The program ends with 1 if MARK.TXT is there when WinExec
    // returns, 2 if it isn't, and 10h if WinExec failed.
    let mut code = Code::new();
    let path_at = code.here() + 2;
    code.push_far(0).push(1).call("KERNEL", WIN_EXEC);
    let mark_at = code.here() + 10;
    code.emit(&[
        0x3D, 0x20, 0x00, // cmp ax,20h
        0xB0, 0x10, // mov al,10h
        0x72, 0x10, // jb end
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
    code.emit(b"C:\\CHILD.EXE\0");
    code.patch(path_at, path);
    let mark = code.here();
    code.emit(b"C:\\MARK.TXT\0");
    code.patch(mark_at, mark);

    let mut child = Code::new();
    // WaitEvent(0) first, as the C runtime's startup code calls it after
    // InitTask: it takes the event LoadModule gave the task.
    child.push(0).call("KERNEL", WAIT_EVENT);
    let name_at = child.here() + 3;
    child
        .emit(&[
            0x0E, 0x1F, // push cs; pop ds
            0xBA, 0xFF, 0xFF, // mov dx,name
            0x33, 0xC9, // xor cx,cx
            0xB4, 0x3C, // mov ah,3Ch
            0xCD, 0x21, // int 21h
            0x72, 0x06, // jc over the close
            0x8B, 0xD8, // mov bx,ax
            0xB4, 0x3E, // mov ah,3Eh
            0xCD, 0x21, // int 21h
        ])
        .call("KERNEL", YIELD)
        .emit(&[0xB0, 0x07]) // mov al,7
        .exit_al();
    let name = child.here();
    child.emit(b"C:\\MARK.TXT\0");
    child.patch(name_at, name);
    let child = program(&child, CODE);
    let (_, log) = run_with("first_turn", program(&code, CODE), |_, base| {
        fs::write(base.join("CHILD.EXE"), child).unwrap();
    });
    assert!(ended_with(&log, 7), "{log}");
    assert!(ended_with(&log, 1), "{log}");
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

/// KERNEL's GetModuleUsage.
const GET_MODULE_USAGE: u16 = 48;

#[test]
fn a_task_keeps_its_directories_when_the_task_it_started_changes_its_own() {
    // The program, started at C:\, starts CHILD.EXE, which changes to
    // C:\SUB and then to drive D: and ends with 7. On Windows 3.1 and
    // 9x each task has its own current drive and directory: once the
    // child has ended (GetModuleUsage of its instance is 0, the program
    // yielding until then), the program is still at C:\ and opens
    // MARK.TXT there, which C:\SUB and D:\ don't have. It ends with 1
    // when it can open the file, 2 when it can't, and 10h when WinExec
    // fails.
    let mut child = Code::new();
    let sub_at = child.here() + 3;
    child
        .emit(&[
            0x0E, 0x1F, // push cs; pop ds
            0xBA, 0xFF, 0xFF, // mov dx,sub
            0xB4, 0x3B, 0xCD, 0x21, // mov ah,3Bh; int 21h: CHDIR
            0xB4, 0x0E, 0xB2, 0x03, 0xCD, 0x21, // mov ah,0Eh; mov dl,3; int 21h
            0xB0, 0x07, // mov al,7
        ])
        .exit_al();
    let sub = child.here();
    child.emit(b"SUB\0");
    child.patch(sub_at, sub);

    let mut code = Code::new();
    let path_at = code.here() + 2;
    code.push_far(0).push(1).call("KERNEL", WIN_EXEC);
    code.emit(&[
        0x8B, 0xF0, // mov si,ax: the child's instance
        0x3D, 0x20, 0x00, // cmp ax,20h
        0x73, 0x06, // jae wait
        0xB0, 0x10, // mov al,10h
    ])
    .exit_al();
    let wait = code.here();
    code.emit(&[0x56]) // push si
        .call("KERNEL", GET_MODULE_USAGE)
        .emit(&[0x0B, 0xC0, 0x74, 0x07]) // or ax,ax; jz done
        .call("KERNEL", YIELD);
    let back = wait as i32 - (code.here() as i32 + 2);
    code.emit(&[0xEB, back as i8 as u8]); // jmp wait
    let mark_at = code.here() + 3;
    code.emit(&[
        0x0E, 0x1F, // done: push cs; pop ds
        0xBA, 0xFF, 0xFF, // mov dx,mark
        0xB8, 0x00, 0x3D, // mov ax,3D00h
        0xCD, 0x21, // int 21h
        0xB0, 0x01, // mov al,1
        0x73, 0x02, // jnc end
        0xB0, 0x02, // mov al,2
    ])
    .exit_al();
    let path = code.here();
    code.emit(b"CHILD.EXE\0");
    code.patch(path_at, path);
    let mark = code.here();
    code.emit(b"MARK.TXT\0");
    code.patch(mark_at, mark);
    let (exit, log) = run_with("own_directories", program(&code, CODE), |cpu, base| {
        fs::write(base.join("CHILD.EXE"), program_named("CHILD", &child, CODE)).unwrap();
        fs::write(base.join("MARK.TXT"), b"mark").unwrap();
        fs::create_dir_all(base.join("SUB")).unwrap();
        let d = base.join("d");
        fs::create_dir_all(&d).unwrap();
        cpu.bus
            .mount_drive(3, &d, MountOptions::default(), false)
            .unwrap();
    });
    assert!(log.contains("ended with exit code 7"), "{log}");
    assert_eq!(exit, 1, "{log}");
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
    library_importing_all(name, &[dll])
}

/// A DLL named `name` that imports the DLLs `dlls`, in that order, whose
/// LibMain returns 1.
fn library_importing_all(name: &str, dlls: &[&str]) -> Vec<u8> {
    let mut b = ne::Builder::new(name, ne::LIBMODULE);
    for dll in dlls {
        b.import_module(dll);
    }
    let text = b.segment(ne::SegmentSpec {
        data: vec![0xB8, 0x01, 0x00, 0xCB], // mov ax,1; retf
        flags: CODE,
        alloc: 0,
        relocations: Vec::new(),
    });
    b.entry_point(text, 0);
    b.build()
}

/// Code that ends the program with `fail` unless a module of the name
/// is loaded, when `loaded`, or isn't, when not. It returns where the
/// name's offset goes, which is patched once the name has its place.
fn check_loaded(code: &mut Code, loaded: bool, fail: u8) -> u16 {
    let name_at = code.here() + 2;
    code.push_far(0).call("KERNEL", GET_MODULE_HANDLE);
    let skip = if loaded { 0x75 } else { 0x74 }; // jnz or jz
    code.emit(&[
        0x0B, 0xC0, // or ax,ax
        skip, 0x06, // over the exit
        0xB0, fail, 0xB4, 0x4C, 0xCD, 0x21, // mov al,fail; exit
    ]);
    name_at
}

#[test]
fn dlls_that_import_each_other_go_with_their_last_user() {
    // A.DLL and B.DLL import each other, as MS Setup's MSSHLSTF and
    // MSCOMSTF do, and C.DLL imports B. The program loads A, then C,
    // then frees A: B and A stay, since C imports B and B imports A.
    // Then it frees C, and all three go, as KERNEL counts each module
    // once for each load that reaches it (IncExeUsage), not for each
    // import. It ends with 1, or with the number of the check that
    // failed.
    let mut code = Code::new();
    let a_at = code.here() + 2;
    code.push_far(0).call("KERNEL", LOAD_LIBRARY);
    code.emit(&[0x8B, 0xF0]); // mov si,ax
    let c_at = code.here() + 2;
    code.push_far(0).call("KERNEL", LOAD_LIBRARY);
    code.emit(&[0x8B, 0xF8]); // mov di,ax
    code.emit(&[0x56]).call("KERNEL", FREE_LIBRARY); // push si
    let names = [
        check_loaded(&mut code, true, 2),
        check_loaded(&mut code, true, 3),
    ];
    code.emit(&[0x57]).call("KERNEL", FREE_LIBRARY); // push di
    let gone = [
        check_loaded(&mut code, false, 4),
        check_loaded(&mut code, false, 5),
        check_loaded(&mut code, false, 6),
    ];
    code.emit(&[0xB0, 0x01]).exit_al(); // mov al,1
    for (at, text) in [(a_at, &b"C:\\A.DLL\0"[..]), (c_at, b"C:\\C.DLL\0")] {
        let here = code.here();
        code.patch(at, here);
        code.emit(text);
    }
    for (at, text) in names.iter().zip([b"A\0", b"B\0"]) {
        let here = code.here();
        code.patch(*at, here);
        code.emit(text);
    }
    for (at, text) in gone.iter().zip([b"A\0", b"B\0", b"C\0"]) {
        let here = code.here();
        code.patch(*at, here);
        code.emit(text);
    }
    let (exit, log) = run_with("circular_imports", program(&code, CODE), |_, base| {
        fs::write(base.join("A.DLL"), library_importing("A", "B")).unwrap();
        fs::write(base.join("B.DLL"), library_importing("B", "A")).unwrap();
        fs::write(base.join("C.DLL"), library_importing("C", "B")).unwrap();
    });
    assert_eq!(exit, 1, "{log}");
}

#[test]
fn a_load_that_fails_takes_away_the_modules_it_loaded() {
    // A.DLL imports B.DLL, then MISSING.DLL, which isn't there, and B
    // and C.DLL import each other. LoadLibrary of A fails, and A, B and
    // C go with it, as none of them has a user. It ends with 1, or with
    // the number of the check that failed.
    let mut code = Code::new();
    let a_at = code.here() + 2;
    code.push_far(0).call("KERNEL", LOAD_LIBRARY);
    code.emit(&[
        0x3D, 0x20, 0x00, // cmp ax,32
        0x72, 0x06, // jb over the exit
        0xB0, 0x02, 0xB4, 0x4C, 0xCD, 0x21, // mov al,2; exit
    ]);
    let gone = [
        check_loaded(&mut code, false, 3),
        check_loaded(&mut code, false, 4),
        check_loaded(&mut code, false, 5),
    ];
    code.emit(&[0xB0, 0x01]).exit_al(); // mov al,1
    let here = code.here();
    code.patch(a_at, here);
    code.emit(b"C:\\A.DLL\0");
    for (at, text) in gone.iter().zip([b"A\0", b"B\0", b"C\0"]) {
        let here = code.here();
        code.patch(*at, here);
        code.emit(text);
    }
    let (exit, log) = run_with("failed_import", program(&code, CODE), |_, base| {
        let a = library_importing_all("A", &["B", "MISSING"]);
        fs::write(base.join("A.DLL"), a).unwrap();
        fs::write(base.join("B.DLL"), library_importing("B", "C")).unwrap();
        fs::write(base.join("C.DLL"), library_importing("C", "B")).unwrap();
    });
    assert_eq!(exit, 1, "{log}");
}

/// USER's CreateWindow, RegisterClass and DefWindowProc.
const CREATE_WINDOW: u16 = 41;
const REGISTER_CLASS: u16 = 57;
const DEF_WINDOW_PROC: u16 = 107;

/// The places in a code segment that `register_probe` and
/// `create_probe` read: the class's name and its window procedure.
#[derive(Default)]
struct Probe {
    names_at: Vec<u16>,
    proc_at: Option<u16>,
}

/// Code that registers the class "Probe", global (CS_GLOBALCLASS) if
/// `global`, with the instance in AX, from a WNDCLASS on the stack: AX
/// is then RegisterClass's atom, or 0. `finish_probe` writes the name
/// and the procedure.
fn register_probe(code: &mut Code, probe: &mut Probe, global: bool) {
    let style: u16 = if global { 0x4000 } else { 0 };
    let [lo, hi] = style.to_le_bytes();
    code.emit(&[
        0x83, 0xEC, 0x1A, // sub sp,26
        0x8B, 0xDC, // mov bx,sp
        0x36, 0xC7, 0x07, lo, hi, // mov word [ss:bx],style
    ]);
    probe.proc_at = Some(code.here() + 4);
    code.emit(&[
        0x36, 0xC7, 0x47, 0x02, 0x00, 0x00, // mov word [ss:bx+2],proc
        0x36, 0x8C, 0x4F, 0x04, // mov [ss:bx+4],cs
        0x36, 0x89, 0x47, 0x0A, // mov [ss:bx+10],ax: the instance
    ]);
    // No extra bytes, icon, cursor, brush or menu.
    for at in [6, 8, 12, 14, 16, 18, 20] {
        code.emit(&[0x36, 0xC7, 0x47, at, 0x00, 0x00]);
    }
    probe.names_at.push(code.here() + 4);
    code.emit(&[
        0x36, 0xC7, 0x47, 0x16, 0x00, 0x00, // mov word [ss:bx+22],name
        0x36, 0x8C, 0x4F, 0x18, // mov [ss:bx+24],cs
        0x16, 0x53, // push ss; push bx
    ]);
    code.call("USER", REGISTER_CLASS);
    code.emit(&[0x83, 0xC4, 0x1A]); // add sp,26
}

/// Code that makes a popup window of the class "Probe" with the
/// instance in DI, and leaves it there: AX is then its handle, or 0.
fn create_probe(code: &mut Code, probe: &mut Probe) {
    create_probe_at(code, probe, false, [0, 0, 10, 10]);
}

/// `create_probe`, the window shown at once (WS_VISIBLE) if `visible`,
/// at `place`: x, y, width and height on the screen.
fn create_probe_at(code: &mut Code, probe: &mut Probe, visible: bool, place: [u16; 4]) {
    probe.names_at.push(code.here() + 2);
    code.push_far(0); // the class
    probe.names_at.push(code.here() + 2);
    code.push_far(0); // the title: the class's name too
    code.push(if visible { 0x9000 } else { 0x8000 }).push(0); // WS_POPUP
    for v in place.into_iter().chain([0, 0]) {
        code.push(v); // the place, the size, no parent, no menu
    }
    code.emit(&[0x57]); // push di: the instance
    code.push(0).push(0).call("USER", CREATE_WINDOW);
}

/// The class's name, and its window procedure (a jump to
/// DefWindowProc), at the end of `code`.
fn finish_probe(code: &mut Code, probe: Probe) {
    let here = code.here();
    for at in probe.names_at {
        code.patch(at, here);
    }
    code.emit(b"Probe\0");
    if let Some(at) = probe.proc_at {
        let here = code.here();
        code.patch(at, here);
        let user = code.module("USER");
        let target = ne::Target::Ordinal {
            module: user,
            ordinal: DEF_WINDOW_PROC,
        };
        let at = code.here() + 1;
        code.emit(&[0xEA, 0xFF, 0xFF, 0x00, 0x00]); // jmp far DefWindowProc
        code.fixup(at, ne::ADDR_FARPTR, target);
    }
}

/// `mov al,exit`, and the end of the program, if AX is 0.
fn exit_if_zero(code: &mut Code, exit: u8) {
    code.emit(&[
        0x0B, 0xC0, // or ax,ax
        0x75, 0x06, // jnz over the exit
        0xB0, exit, 0xB4, 0x4C, 0xCD, 0x21, // mov al,exit; exit
    ]);
}

/// A library's classes are unregistered when the library is freed, as
/// Windows 3.1's and Windows 95's USER does when KERNEL frees a module:
/// a global class registered with the instance of C:\PROBE.DLL is gone
/// once FreeLibrary has freed it, so the program can register its own
/// global class of that name. Exit code 1, 2 if LoadLibrary failed, 3 if
/// the library's class wasn't registered, 4 if the program's wasn't.
#[test]
fn a_freed_librarys_classes_are_unregistered() {
    let mut code = Code::new();
    let mut probe = Probe::default();
    let path_at = code.here() + 2;
    code.push_far(0).call("KERNEL", LOAD_LIBRARY);
    code.emit(&[
        0x3D, 0x20, 0x00, // cmp ax,20h
        0x73, 0x06, // jae over the exit
        0xB0, 0x02, 0xB4, 0x4C, 0xCD, 0x21, // mov al,2; exit
        0x8B, 0xF0, // mov si,ax: the library
    ]);
    register_probe(&mut code, &mut probe, true);
    exit_if_zero(&mut code, 3);
    code.emit(&[0x56]).call("KERNEL", FREE_LIBRARY); // push si
    code.emit(&[0x8B, 0xC7]); // mov ax,di: the program's instance
    register_probe(&mut code, &mut probe, true);
    exit_if_zero(&mut code, 4);
    code.emit(&[0xB0, 0x01]).exit_al();
    let path = code.here();
    code.patch(path_at, path);
    code.emit(b"C:\\PROBE.DLL\0");
    finish_probe(&mut code, probe);
    let (exit, log) = run_with("freed_library_classes", program(&code, CODE), |_, base| {
        fs::write(base.join("PROBE.DLL"), library("PROBE")).unwrap();
    });
    assert_eq!(exit, 1, "{log}");
}

/// C:\CHILD.EXE: it registers the class "Probe" with its instance,
/// global if `global`, makes a window of it if `window` and leaves it
/// there, and ends with 5; with 6 if RegisterClass failed, 7 if
/// CreateWindow did.
fn probe_child(global: bool, window: bool) -> Vec<u8> {
    let mut child = Code::new();
    let mut probe = Probe::default();
    child.emit(&[0x8B, 0xC7]); // mov ax,di: its instance
    register_probe(&mut child, &mut probe, global);
    exit_if_zero(&mut child, 6);
    if window {
        create_probe(&mut child, &mut probe);
        exit_if_zero(&mut child, 7);
    }
    child.emit(&[0xB0, 0x05]).exit_al();
    finish_probe(&mut child, probe);
    program(&child, CODE)
}

/// Code that starts C:\CHILD.EXE and waits until it has ended and its
/// module is gone (Yield, GetModuleUsage), or ends with 2 if WinExec
/// failed. The place of the path's offset is put in `paths_at`.
fn run_child(code: &mut Code, paths_at: &mut Vec<u16>) {
    paths_at.push(code.here() + 2);
    code.push_far(0).push(1).call("KERNEL", WIN_EXEC); // SW_SHOWNORMAL
    code.emit(&[
        0x3D, 0x20, 0x00, // cmp ax,20h
        0x73, 0x06, // jae over the exit
        0xB0, 0x02, 0xB4, 0x4C, 0xCD, 0x21, // mov al,2; exit
        0x8B, 0xF0, // mov si,ax: the child's instance
    ]);
    let wait = code.here();
    code.call("KERNEL", YIELD).emit(&[0x56]); // push si
    code.call("KERNEL", GET_MODULE_USAGE);
    let back = wait as i32 - (code.here() as i32 + 4);
    code.emit(&[0x0B, 0xC0, 0x75, back as u8]); // or ax,ax; jnz wait
}

/// A program runs C:\CHILD.EXE (`probe_child(true, window)`) to its
/// end, then registers a global class "Probe" itself: 1, or 4 if that
/// failed.
fn after_global_child(test: &str, window: bool) -> (u8, String) {
    let mut code = Code::new();
    let mut probe = Probe::default();
    let mut paths_at = Vec::new();
    run_child(&mut code, &mut paths_at);
    code.emit(&[0x8B, 0xC7]); // mov ax,di: the program's instance
    register_probe(&mut code, &mut probe, true);
    exit_if_zero(&mut code, 4);
    code.emit(&[0xB0, 0x01]).exit_al();
    let path = code.here();
    for at in paths_at {
        code.patch(at, path);
    }
    code.emit(b"C:\\CHILD.EXE\0");
    finish_probe(&mut code, probe);
    let child = probe_child(true, window);
    run_with(test, program(&code, CODE), |_, base| {
        fs::write(base.join("CHILD.EXE"), child).unwrap();
    })
}

/// A program's classes are unregistered when it ends while other
/// programs go on: once C:\CHILD.EXE, which registered the global class
/// "Probe", has ended, the program registers a global class of that
/// name.
#[test]
fn an_ended_programs_classes_are_unregistered() {
    let (exit, log) = after_global_child("ended_program_classes", false);
    assert_eq!(exit, 1, "{log}");
    assert!(log.contains("ended with exit code 5"), "{log}");
}

/// The windows a program leaves when it ends go with it, and so its
/// classes, which those windows kept, are unregistered too.
#[test]
fn an_ended_programs_classes_go_with_the_windows_it_left() {
    let (exit, log) = after_global_child("ended_program_window_classes", true);
    assert_eq!(exit, 1, "{log}");
    assert!(log.contains("ended with exit code 5"), "{log}");
}

/// USER's ShowWindow, ValidateRect, GetUpdateRect and SetWindowPos,
/// SW_SHOWNA, and SetWindowPos's flags SWP_NOSIZE, SWP_NOMOVE,
/// SWP_NOACTIVATE and SWP_SHOWWINDOW together.
const SHOW_WINDOW: u16 = 42;
const VALIDATE_RECT: u16 = 127;
const GET_UPDATE_RECT: u16 = 190;
const SET_WINDOW_POS: u16 = 232;
const SW_SHOWNA: u16 = 8;
const SWP_SHOW_IN_PLACE: u16 = 0x53;

/// Where the window C:\CHILD.EXE leaves is, against the program's at
/// (100, 100), 50 by 50.
#[derive(Clone, Copy, PartialEq)]
enum ChildWindow {
    /// No window.
    None,
    /// Over the program's, in the same place.
    Over,
    /// Beside it, at (200, 100): they don't overlap.
    Beside,
    /// Under it, at (80, 80), at the bottom of the windows: the
    /// program's covers part of it.
    Under,
}

/// A program shows a window at (100, 100) and validates it, runs
/// C:\CHILD.EXE to its end, and ends with 10 if its window has nothing
/// to paint then (GetUpdateRect), 11 if it has; 3 if RegisterClass
/// failed, 4 if CreateWindow did. The child registers "Probe", shows a
/// window of it where `window` says and leaves it there, and ends with
/// 5.
fn update_after_child(test: &str, window: ChildWindow) -> (u8, String) {
    let mut code = Code::new();
    let mut probe = Probe::default();
    let mut paths_at = Vec::new();
    code.emit(&[0x8B, 0xC7]); // mov ax,di: the program's instance
    register_probe(&mut code, &mut probe, false);
    exit_if_zero(&mut code, 3);
    create_probe_at(&mut code, &mut probe, true, [100, 100, 50, 50]);
    exit_if_zero(&mut code, 4);
    let [a, b] = HWND.to_le_bytes();
    code.emit(&[0xA3, a, b]); // mov [HWND],ax
    code.emit(&[0xFF, 0x36, a, b]) // push word [HWND]
        .push(0)
        .push(0) // the whole window
        .call("USER", VALIDATE_RECT);
    run_child(&mut code, &mut paths_at);
    code.emit(&[0xFF, 0x36, a, b]) // push word [HWND]
        .push(0)
        .push(0) // no rectangle
        .push(0) // no erasing
        .call("USER", GET_UPDATE_RECT);
    code.emit(&[0x04, 0x0A]).exit_al(); // add al,10
    let path = code.here();
    for at in paths_at {
        code.patch(at, path);
    }
    code.emit(b"C:\\CHILD.EXE\0");
    finish_probe(&mut code, probe);
    let mut child = Code::new();
    let mut probe = Probe::default();
    child.emit(&[0x8B, 0xC7]); // mov ax,di: its instance
    register_probe(&mut child, &mut probe, false);
    exit_if_zero(&mut child, 6);
    let place = match window {
        ChildWindow::None => None,
        ChildWindow::Over => Some([100, 100, 50, 50]),
        ChildWindow::Beside => Some([200, 100, 50, 50]),
        ChildWindow::Under => Some([80, 80, 50, 50]),
    };
    if let Some(place) = place {
        // Shown without taking the activation, which would wait for the
        // program, busy in its loop, to answer.
        create_probe_at(&mut child, &mut probe, false, place);
        exit_if_zero(&mut child, 7);
        child.emit(&[0x50]); // push ax: the window
        if window == ChildWindow::Under {
            child
                .push(1) // HWND_BOTTOM
                .push(0)
                .push(0)
                .push(0)
                .push(0)
                .push(SWP_SHOW_IN_PLACE)
                .call("USER", SET_WINDOW_POS);
        } else {
            child.push(SW_SHOWNA).call("USER", SHOW_WINDOW);
        }
    }
    child.emit(&[0xB0, 0x05]).exit_al();
    finish_probe(&mut child, probe);
    let child = program(&child, CODE);
    run_with(test, program(&code, CODE), |_, base| {
        fs::write(base.join("CHILD.EXE"), child).unwrap();
    })
}

/// A program that ends without windows changes nothing on the screen,
/// as on Windows 3.1 and 95: the other programs' windows have nothing
/// new to paint. The emulator painted the whole screen again whenever a
/// task ended, and a window painted then with a palette realized in the
/// background kept those colors once its palette came to the
/// foreground (MS Setup's background in Video for Windows' setup, after
/// its display profiler ended).
#[test]
fn an_ended_program_without_windows_leaves_the_others_painted() {
    let (exit, log) = update_after_child("ended_program_no_repaint", ChildWindow::None);
    assert_eq!(exit, 10, "{log}");
    assert!(log.contains("ended with exit code 5"), "{log}");
}

/// The window a program leaves when it ends goes with it, and what was
/// under it paints again: the window under it needs painting.
#[test]
fn an_ended_programs_window_uncovers_the_one_under_it() {
    let (exit, log) = update_after_child("ended_program_uncovers", ChildWindow::Over);
    assert_eq!(exit, 11, "{log}");
    assert!(log.contains("ended with exit code 5"), "{log}");
}

/// Only what an ended program's windows showed paints again: a window
/// beside them has nothing to paint.
#[test]
fn an_ended_programs_window_leaves_one_beside_it_painted() {
    let (exit, log) = update_after_child("ended_program_beside", ChildWindow::Beside);
    assert_eq!(exit, 10, "{log}");
    assert!(log.contains("ended with exit code 5"), "{log}");
}

/// A window over an ended program's window covered that part of it, so
/// nothing changes there as it goes, as when DestroyWindow hides a
/// window on Windows: the window above has nothing to paint.
#[test]
fn an_ended_programs_window_leaves_the_one_over_it_painted() {
    let (exit, log) = update_after_child("ended_program_under", ChildWindow::Under);
    assert_eq!(exit, 10, "{log}");
    assert!(log.contains("ended with exit code 5"), "{log}");
}

/// Where the class test keeps the library's instance and its WNDCLASS.
const LIB: u16 = 0x20;
const WNDCLASS: u16 = 0x40;

/// `mov word [at],value`. Where `value` is, to patch.
fn store_word(code: &mut Code, at: u16, value: u16) -> u16 {
    let [a, b] = at.to_le_bytes();
    let [v, w] = value.to_le_bytes();
    let operand = code.here() + 4;
    code.emit(&[0xC7, 0x06, a, b, v, w]);
    operand
}

/// `mov [at],cs`.
fn store_cs(code: &mut Code, at: u16) {
    let [a, b] = at.to_le_bytes();
    code.emit(&[0x8C, 0x0E, a, b]);
}

/// Code that registers a class for the instance in [LIB], with its
/// WNDCLASS at WNDCLASS: the atom in AX. Where the offsets of its window
/// procedure and its name (both in the code segment) go, to patch.
fn register_class(code: &mut Code) -> (u16, u16) {
    store_word(code, WNDCLASS, 0); // style
    let proc = store_word(code, WNDCLASS + 2, 0);
    store_cs(code, WNDCLASS + 4);
    store_word(code, WNDCLASS + 6, 0); // cbClsExtra
    store_word(code, WNDCLASS + 8, 0); // cbWndExtra
    let [a, b] = LIB.to_le_bytes();
    code.emit(&[0xA1, a, b]); // mov ax,[LIB]
    let [a, b] = (WNDCLASS + 10).to_le_bytes();
    code.emit(&[0xA3, a, b]); // mov [hInstance],ax
    for at in [12, 14, 16, 18, 20] {
        store_word(code, WNDCLASS + at, 0); // icon, cursor, brush, menu
    }
    let name = store_word(code, WNDCLASS + 22, 0);
    store_cs(code, WNDCLASS + 24);
    code.emit(&[0x1E])
        .push(WNDCLASS)
        .call("USER", REGISTER_CLASS);
    (proc, name)
}

/// Windows 3.1 frees a module's window classes with the module. A class
/// registered for a library's instance goes when FreeLibrary frees the
/// library, so the library loaded again registers its class again. A
/// class left behind kept the name, and the new instance could be the
/// old one's: its RegisterClass failed and its windows got the old
/// class, with a window procedure in code that had gone (Memphis Math's
/// MMAP after STORYBRD). The program ends with 1 if the first
/// RegisterClass fails, 2 if the second does, else with 0.
#[test]
fn a_freed_librarys_classes_go_with_it() {
    let mut code = Code::new();
    let mut paths = Vec::new();
    let mut operands = Vec::new();
    for fail in [1u8, 2] {
        paths.push(code.here() + 2);
        code.push_far(0).call("KERNEL", LOAD_LIBRARY);
        let [a, b] = LIB.to_le_bytes();
        code.emit(&[0xA3, a, b]); // mov [LIB],ax
        operands.push(register_class(&mut code));
        code.emit(&[0x85, 0xC0]) // test ax,ax
            .emit(&[0x75, 0x06]) // jnz over the exit
            .emit(&[0xB0, fail, 0xB4, 0x4C, 0xCD, 0x21]);
        if fail == 1 {
            code.emit(&[0xFF, 0x36, a, b]) // push word [LIB]
                .call("KERNEL", FREE_LIBRARY);
        }
    }
    code.emit(&[0xB0, 0x00]).exit_al();
    let proc = code.here();
    code.emit(&[0x33, 0xC0, 0x99, 0xCA, 0x0A, 0x00]); // xor ax,ax; cwd; retf 10
    let name = code.here();
    code.emit(b"MINE\0");
    let path = code.here();
    code.emit(b"C:\\LIB.DLL\0");
    for at in paths {
        code.patch(at, path);
    }
    for (proc_at, name_at) in operands {
        code.patch(proc_at, proc);
        code.patch(name_at, name);
    }
    let (exit, log) = run_with("freed_classes", program(&code, CODE), |_, base| {
        fs::write(base.join("LIB.DLL"), library("LIB")).unwrap();
    });
    assert_eq!(exit, 0, "{log}");
}

/// USER's GetClassName.
const GET_CLASS_NAME: u16 = 58;

/// Where the kept-class test keeps the program's instance, its window
/// and the name GetClassName gives.
const INSTANCE: u16 = 0x22;
const HWND: u16 = 0x24;
const CLASS_NAME: u16 = 0x60;

/// A freed library's class that still has a window stays, as
/// UnregisterClass leaves one. The window keeps its class, so a class
/// registered after the library went doesn't take its place: the
/// window's class is still MINE. The program ends with 1 to 3 when
/// RegisterClass, CreateWindow or the second RegisterClass fails, 4 when
/// GetClassName gives another name, else with 0.
#[test]
fn a_freed_librarys_class_with_a_window_stays() {
    let mut code = Code::new();
    let [a, b] = INSTANCE.to_le_bytes();
    code.emit(&[0x89, 0x3E, a, b]); // mov [INSTANCE],di
    let path_at = code.here() + 2;
    code.push_far(0).call("KERNEL", LOAD_LIBRARY);
    let [l, m] = LIB.to_le_bytes();
    code.emit(&[0xA3, l, m]); // mov [LIB],ax
    let mine = register_class(&mut code);
    exit_if_zero(&mut code, 1);
    let class_at = code.here() + 2;
    code.push_far(0);
    let title_at = code.here() + 2;
    code.push_far(0)
        .push(0x8000) // WS_POPUP
        .push(0)
        .push(0)
        .push(0)
        .push(10)
        .push(10)
        .push(0) // parent
        .push(0) // menu
        .emit(&[0xFF, 0x36, l, m]) // push word [LIB]
        .push(0)
        .push(0)
        .call("USER", CREATE_WINDOW);
    let [h, i] = HWND.to_le_bytes();
    code.emit(&[0xA3, h, i]); // mov [HWND],ax
    exit_if_zero(&mut code, 2);
    code.emit(&[0xFF, 0x36, l, m]) // push word [LIB]
        .call("KERNEL", FREE_LIBRARY);
    // OTHER, the program's own class.
    code.emit(&[0xA1, a, b, 0xA3, l, m]); // mov ax,[INSTANCE]; mov [LIB],ax
    let other = register_class(&mut code);
    exit_if_zero(&mut code, 3);
    code.emit(&[0xFF, 0x36, h, i]) // push word [HWND]
        .emit(&[0x1E]) // push ds
        .push(CLASS_NAME)
        .push(16)
        .call("USER", GET_CLASS_NAME);
    let [n0, n1] = CLASS_NAME.to_le_bytes();
    let [n2, n3] = (CLASS_NAME + 2).to_le_bytes();
    let [n4, n5] = (CLASS_NAME + 4).to_le_bytes();
    code.emit(&[
        0xB0, 0x04, // mov al,4
        0x81, 0x3E, n0, n1, b'M', b'I', // cmp word [CLASS_NAME],'MI'
        0x75, 0x11, // jne end
        0x81, 0x3E, n2, n3, b'N', b'E', // cmp word [CLASS_NAME+2],'NE'
        0x75, 0x09, // jne end
        0x80, 0x3E, n4, n5, 0x00, // cmp byte [CLASS_NAME+4],0
        0x75, 0x02, // jne end
        0xB0, 0x00, // mov al,0
    ])
    .exit_al();
    // The window procedure, in the program: DefWindowProc's.
    let proc = code.here();
    code.emit(&[
        0x55, 0x8B, 0xEC, // push bp; mov bp,sp
        0xFF, 0x76, 0x0E, // push word [bp+0Eh]: hwnd
        0xFF, 0x76, 0x0C, // push word [bp+0Ch]: msg
        0xFF, 0x76, 0x0A, // push word [bp+0Ah]: wParam
        0xFF, 0x76, 0x08, // push word [bp+8]: lParam
        0xFF, 0x76, 0x06, // push word [bp+6]
    ])
    .call("USER", DEF_WINDOW_PROC)
    .emit(&[0x5D, 0xCA, 0x0A, 0x00]); // pop bp; retf 10
    let mine_name = code.here();
    code.emit(b"MINE\0");
    let other_name = code.here();
    code.emit(b"OTHER\0");
    let title = code.here();
    code.emit(b"\0");
    let path = code.here();
    code.emit(b"C:\\LIB.DLL\0");
    code.patch(path_at, path);
    code.patch(class_at, mine_name);
    code.patch(title_at, title);
    for ((proc_at, name_at), name) in [(mine, mine_name), (other, other_name)] {
        code.patch(proc_at, proc);
        code.patch(name_at, name);
    }
    let (exit, log) = run_with("kept_class", program(&code, CODE), |_, base| {
        fs::write(base.join("LIB.DLL"), library("LIB")).unwrap();
    });
    assert_eq!(exit, 0, "{log}");
}
