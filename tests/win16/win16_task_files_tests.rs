//! The file handles of Windows 3.1 tasks: each task has a PDB of its own
//! with a handle table, a handle number means a file only in the PDB
//! that opened it, and INT 21h AH=50h switches to another PDB. The
//! programs are made byte by byte (tests/common/win16).

use crate::win16;

use std::fs;
use win16::{CODE, Code, program, program_named, run_with};

/// KERNEL's ordinals.
const GET_MODULE_HANDLE: u16 = 47;
const YIELD: u16 = 29;
const WAIT_EVENT: u16 = 30;
const WIN_EXEC: u16 = 166;

/// Whether a task ended with the exit code `code`, as the log says.
fn ended_with(log: &str, code: u8) -> bool {
    let line = format!(" ended with exit code {:X}", code);
    log.lines()
        .any(|l| l.starts_with("[W16] Task ") && l.ends_with(&line))
}

/// `WinExec` of the name at an offset patched later: where it goes.
fn win_exec(code: &mut Code) -> u16 {
    let at = code.here() + 2;
    code.push_far(0).push(1).call("KERNEL", WIN_EXEC);
    at
}

/// A short jump `op` (a Jcc's or JMP's opcode) to a place known later:
/// where its displacement is, for `land`.
fn jump(code: &mut Code, op: u8) -> usize {
    code.emit(&[op, 0]);
    code.bytes.len() - 1
}

/// Point the jumps at `jumps` here.
fn land(code: &mut Code, jumps: &[usize]) {
    let here = code.bytes.len();
    for &at in jumps {
        code.bytes[at] = (here - at - 1) as u8;
    }
}

/// Until GetModuleHandle of the name at an offset patched later gives
/// 0, Yield: where the name's offset goes.
fn wait_for_module(code: &mut Code) -> u16 {
    let wait = code.here();
    let at = code.here() + 2;
    code.push_far(0).call("KERNEL", GET_MODULE_HANDLE).emit(&[
        0x0B, 0xC0, // or ax,ax
        0x74, 0x07, // jz over the next two
    ]);
    code.call("KERNEL", YIELD);
    let back = wait.wrapping_sub(code.here() + 2) as u8;
    code.emit(&[0xEB, back]); // jmp wait
    at
}

/// Put the string `text` here and patch its offset in at `places`.
fn string_at(code: &mut Code, text: &[u8], places: &[u16]) {
    let at = code.here();
    code.emit(text);
    for &place in places {
        code.patch(place, at);
    }
}

#[test]
fn a_tasks_exit_closes_its_own_handles_and_not_another_tasks() {
    // A C runtime's exit closes every handle its table could have, 5 to
    // 19, whether it opened them or not. Memphis Math's path game did
    // that as it ended while the rooms screen had Btrieve files open
    // under those numbers in its own task, and Btrieve's next read failed
    // on a closed handle. In Windows 3.1 each task has its own handles,
    // and the closes reach only the ending task's.
    //
    // CHILD opens C:\DATA.TXT and yields; the program closes handles 5
    // to 19 with INT 21h AH=3Eh and waits for CHILD to end. CHILD then
    // reads a byte with its handle: it ends with 7 when it read the
    // file's "Z", 9 when the read failed or read something else.
    let mut code = Code::new();
    let child_at = win_exec(&mut code);
    code.emit(&[
        0xBB, 0x05, 0x00, // mov bx,5
        0xB4, 0x3E, // close: mov ah,3Eh
        0xCD, 0x21, // int 21h
        0x43, // inc bx
        0x83, 0xFB, 0x14, // cmp bx,20
        0x72, 0xF6, // jb close
    ]);
    let name_at = wait_for_module(&mut code);
    code.emit(&[0xB0, 0x01]).exit_al(); // mov al,1
    string_at(&mut code, b"C:\\CHILD.EXE\0", &[child_at]);
    string_at(&mut code, b"CHILD\0", &[name_at]);

    let mut child = Code::new();
    child.push(0).call("KERNEL", WAIT_EVENT);
    let data_at = child.here() + 3;
    child.emit(&[
        0x0E, 0x1F, // push cs; pop ds
        0xBA, 0xFF, 0xFF, // mov dx,data
        0xB8, 0x00, 0x3D, // mov ax,3D00h
        0xCD, 0x21, // int 21h
        0x8B, 0xF0, // mov si,ax
    ]);
    child.call("KERNEL", YIELD).emit(&[
        0x8B, 0xDE, // mov bx,si
        0x16, 0x1F, // push ss; pop ds
        0x83, 0xEC, 0x02, // sub sp,2
        0x8B, 0xD4, // mov dx,sp
        0xB9, 0x01, 0x00, // mov cx,1
        0xB4, 0x3F, // mov ah,3Fh
        0xCD, 0x21, // int 21h
        0xB0, 0x09, // mov al,9
        0x72, 0x08, // jc end
        0x59, // pop cx
        0x80, 0xF9, 0x5A, // cmp cl,'Z'
        0x75, 0x02, // jne end
        0xB0, 0x07, // mov al,7
    ]);
    child.exit_al();
    string_at(&mut child, b"C:\\DATA.TXT\0", &[data_at]);
    let child = program_named("CHILD", &child, CODE);

    let (exit, log) = run_with("task_exit_closes", program(&code, CODE), |_, base| {
        fs::write(base.join("CHILD.EXE"), child).unwrap();
        fs::write(base.join("DATA.TXT"), b"Z").unwrap();
    });
    assert!(ended_with(&log, 7), "{log}");
    assert_eq!(exit, 1, "{log}");
}

#[test]
fn int_21h_50h_switches_to_another_tasks_pdb_and_its_handles() {
    // Btrieve's WBTRCALL.DLL saves the current PDB (AH=51h) and makes
    // its own current (AH=50h) around its DOS calls, as Windows 3.1
    // allows: a handle then means the file it is in that PDB.
    //
    // CHILD opens C:\DATA.TXT, writes its PDB's selector (AH=51h) and the
    // handle to C:\PDB.BIN, and yields until PDB.BIN is gone. The program
    // reads them and switches to CHILD's PDB. It ends with:
    // 1 when all went as in Windows;
    // 2 when AH=51h then didn't give CHILD's PDB;
    // 3 when the read of CHILD's handle failed there;
    // 4 when it read something other than DATA.TXT's "Z";
    // 5 when, back in its own PDB, the same handle could still be read.
    // It deletes PDB.BIN as it ends, and CHILD ends with 7.
    let mut code = Code::new();
    let child_at = win_exec(&mut code);
    let pdb_at = code.here() + 3;
    code.emit(&[
        0x0E, 0x1F, // push cs; pop ds
        0xBA, 0xFF, 0xFF, // mov dx,pdb
        0xB8, 0x00, 0x3D, // mov ax,3D00h
        0xCD, 0x21, // int 21h
        0x8B, 0xD8, // mov bx,ax
        0x83, 0xEC, 0x04, // sub sp,4
        0x16, 0x1F, // push ss; pop ds
        0x8B, 0xD4, // mov dx,sp
        0xB9, 0x04, 0x00, // mov cx,4
        0xB4, 0x3F, // mov ah,3Fh
        0xCD, 0x21, // int 21h
        0xB4, 0x3E, // mov ah,3Eh
        0xCD, 0x21, // int 21h
        0x5F, // pop di (CHILD's PDB)
        0x5E, // pop si (CHILD's handle)
        0xB4, 0x51, // mov ah,51h
        0xCD, 0x21, // int 21h
        0x8B, 0xEB, // mov bp,bx (the program's own PDB)
        0x8B, 0xDF, // mov bx,di
        0xB4, 0x50, // mov ah,50h
        0xCD, 0x21, // int 21h
        0xB4, 0x51, // mov ah,51h
        0xCD, 0x21, // int 21h
        0xB0, 0x02, // mov al,2
        0x3B, 0xDF, // cmp bx,di
    ]);
    let mut ends = vec![jump(&mut code, 0x75)]; // jne end
    let read = [
        0x6A, 0x00, // push 0
        0x8B, 0xD4, // mov dx,sp
        0x8B, 0xDE, // mov bx,si
        0xB9, 0x01, 0x00, // mov cx,1
        0xB4, 0x3F, // mov ah,3Fh
        0xCD, 0x21, // int 21h
    ];
    code.emit(&read).emit(&[0xB0, 0x03]); // mov al,3
    ends.push(jump(&mut code, 0x72)); // jc end
    code.emit(&[
        0x59, // pop cx
        0xB0, 0x04, // mov al,4
        0x80, 0xF9, 0x5A, // cmp cl,'Z'
    ]);
    ends.push(jump(&mut code, 0x75)); // jne end
    code.emit(&[
        0x8B, 0xDD, // mov bx,bp
        0xB4, 0x50, // mov ah,50h
        0xCD, 0x21, // int 21h
    ]);
    code.emit(&read).emit(&[0xB0, 0x05]); // mov al,5
    ends.push(jump(&mut code, 0x73)); // jnc end
    code.emit(&[0xB0, 0x01]); // mov al,1
    land(&mut code, &ends);
    let delete_at = code.here() + 5;
    code.emit(&[
        0x8A, 0xE8, // mov ch,al
        0x0E, 0x1F, // push cs; pop ds
        0xBA, 0xFF, 0xFF, // mov dx,pdb
        0xB4, 0x41, // mov ah,41h
        0xCD, 0x21, // int 21h
        0x8A, 0xC5, // mov al,ch
    ]);
    code.exit_al();
    string_at(&mut code, b"C:\\CHILD.EXE\0", &[child_at]);
    string_at(&mut code, b"C:\\PDB.BIN\0", &[pdb_at, delete_at]);

    let mut child = Code::new();
    child.push(0).call("KERNEL", WAIT_EVENT);
    let data_at = child.here() + 3;
    let pdb_at = child.here() + 17;
    child.emit(&[
        0x0E, 0x1F, // push cs; pop ds
        0xBA, 0xFF, 0xFF, // mov dx,data
        0xB8, 0x00, 0x3D, // mov ax,3D00h
        0xCD, 0x21, // int 21h
        0x50, // push ax (the handle)
        0xB4, 0x51, // mov ah,51h
        0xCD, 0x21, // int 21h
        0x53, // push bx (the PDB)
        0xBA, 0xFF, 0xFF, // mov dx,pdb
        0x33, 0xC9, // xor cx,cx
        0xB4, 0x3C, // mov ah,3Ch
        0xCD, 0x21, // int 21h
        0x8B, 0xD8, // mov bx,ax
        0x16, 0x1F, // push ss; pop ds
        0x8B, 0xD4, // mov dx,sp
        0xB9, 0x04, 0x00, // mov cx,4
        0xB4, 0x40, // mov ah,40h
        0xCD, 0x21, // int 21h
        0xB4, 0x3E, // mov ah,3Eh
        0xCD, 0x21, // int 21h
    ]);
    let wait = child.here();
    let wait_at = child.here() + 8;
    child.call("KERNEL", YIELD).emit(&[
        0x0E, 0x1F, // push cs; pop ds
        0xBA, 0xFF, 0xFF, // mov dx,pdb
        0xB8, 0x00, 0x43, // mov ax,4300h
        0xCD, 0x21, // int 21h
    ]);
    let back = wait.wrapping_sub(child.here() + 2) as u8;
    child.emit(&[0x73, back]); // jnc wait
    child.emit(&[0xB0, 0x07]).exit_al(); // mov al,7
    string_at(&mut child, b"C:\\DATA.TXT\0", &[data_at]);
    string_at(&mut child, b"C:\\PDB.BIN\0", &[pdb_at, wait_at]);
    let child = program_named("CHILD", &child, CODE);

    // CHILD ends last, so the process's exit code is its own.
    let (_, log) = run_with("set_pdb", program(&code, CODE), |_, base| {
        fs::write(base.join("CHILD.EXE"), child).unwrap();
        fs::write(base.join("DATA.TXT"), b"Z").unwrap();
    });
    assert!(ended_with(&log, 1), "{log}");
    assert!(ended_with(&log, 7), "{log}");
}
