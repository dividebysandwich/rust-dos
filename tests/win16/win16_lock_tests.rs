//! Locks of byte ranges of files (INT 21h AH=5Ch) in Windows 3.1
//! programs: KERNEL's reads and writes, and the locks of a task that
//! ends. The programs are made byte by byte (tests/common/win16).

use crate::win16;

use std::fs;
use win16::{CODE, Code, program, run_with};

/// KERNEL's ordinals.
const GLOBAL_ALLOC: u16 = 15;
const GLOBAL_LOCK: u16 = 18;
const YIELD: u16 = 29;
const LREAD: u16 = 82;
const LOPEN: u16 = 85;
const WIN_EXEC: u16 = 166;

/// `_lopen` of the name at `name` (patched later), to read and write and
/// sharing it with everyone, the handle into SI: where the name's offset
/// goes.
fn open_into_si(code: &mut Code) -> u16 {
    let name = code.here() + 2;
    code.push_far(0).push(0x42).call("KERNEL", LOPEN);
    code.emit(&[0x8B, 0xF0]); // mov si, ax
    name
}

/// INT 21h AH=5Ch with AL `al`, of `len` bytes from `offset` (below 64
/// KB) of handle SI, keeping SI and DI: CF says how it went.
fn lock_call(code: &mut Code, al: u8, offset: u16, len: u16) {
    let [ol, oh] = offset.to_le_bytes();
    let [ll, lh] = len.to_le_bytes();
    code.emit(&[
        0x56, 0x57, // push si; push di
        0xB8, al, 0x5C, // mov ax, 5Cxxh
        0x8B, 0xDE, // mov bx, si
        0x33, 0xC9, // xor cx, cx
        0xBA, ol, oh, // mov dx, offset
        0x33, 0xF6, // xor si, si
        0xBF, ll, lh, // mov di, len
        0xCD, 0x21, // int 21h
        0x5F, 0x5E, // pop di; pop si
    ]);
}

/// A short jump with opcode `op`, to be aimed later with `land`: where
/// its displacement is.
fn jump(code: &mut Code, op: u8) -> u16 {
    code.emit(&[op, 0x00]);
    code.here() - 1
}

/// Aim the jump whose displacement is at `at` here.
fn land(code: &mut Code, at: u16) {
    let here = code.here();
    code.bytes[at as usize] = (here - (at + 1)) as u8;
}

/// `mov al, value` and end the program with it.
fn exit_with(code: &mut Code, value: u8) {
    code.emit(&[0xB0, value]).exit_al();
}

/// A name for `open_into_si`, at the end of the code.
fn name_at(code: &mut Code, at: u16, name: &[u8]) {
    let here = code.here();
    code.patch(at, here);
    code.emit(name);
}

#[test]
fn an_lread_over_anothers_lock_reads_nothing() {
    // DATA.BIN has 32 KB. The program opens it twice, locks byte 5000h
    // through the first handle and asks _lread for 6000h bytes through
    // the second: Windows makes that one DOS call, which fails whole
    // (HFILE_ERROR, FFFFh), so the exit code is AH, FFh. Read in pieces,
    // the bytes before the lock came back (4000h, 40h).
    let mut code = Code::new();
    let first = open_into_si(&mut code);
    code.emit(&[0x8B, 0xFE]); // mov di, si
    let second = open_into_si(&mut code);
    code.emit(&[0x87, 0xF7]); // xchg si, di: SI the first, DI the second
    lock_call(&mut code, 0x00, 0x5000, 1);
    let locked = jump(&mut code, 0x73); // jnc
    exit_with(&mut code, 1);
    land(&mut code, locked);
    // GlobalLock(GlobalAlloc(GMEM_FIXED, 8000h)): the buffer at DX:AX.
    code.push(0)
        .push(0)
        .push(0x8000)
        .call("KERNEL", GLOBAL_ALLOC);
    code.emit(&[0x50]).call("KERNEL", GLOBAL_LOCK); // push ax
    code.emit(&[0x57, 0x52, 0x50]); // push di; push dx; push ax
    code.push(0x6000).call("KERNEL", LREAD);
    code.emit(&[0x8A, 0xC4]).exit_al(); // mov al, ah
    name_at(&mut code, first, b"DATA.BIN\0");
    name_at(&mut code, second, b"DATA.BIN\0");
    let (exit, log) = run_with("lread_lock", program(&code, CODE), |_, base| {
        fs::write(base.join("DATA.BIN"), vec![0x55u8; 0x8000]).unwrap();
    });
    assert_eq!(exit, 0xFF, "{log}");
}

#[test]
fn a_tasks_locks_go_when_it_ends() {
    // CHILD.EXE locks the first 4 bytes of DATA.BIN and ends. The program
    // starts it and yields until its own lock of those bytes fails (the
    // child holds them), then until it succeeds again (the child ended):
    // exit code 42h. 2 if the child never locked, 3 if its lock stayed.
    let mut child = Code::new();
    let name = open_into_si(&mut child);
    lock_call(&mut child, 0x00, 0, 4);
    // Yields 20 times first, so that the program sees the lock.
    child.emit(&[0xBF, 0x14, 0x00]); // mov di, 20
    let wait = child.here();
    child.call("KERNEL", YIELD);
    child.emit(&[0x4F]); // dec di
    let back = jump(&mut child, 0x75); // jnz
    child.bytes[back as usize] = (wait as i32 - (back as i32 + 1)) as i8 as u8;
    exit_with(&mut child, 5);
    name_at(&mut child, name, b"DATA.BIN\0");

    let mut code = Code::new();
    let name = open_into_si(&mut code);
    let path = code.here() + 2;
    code.push_far(0).push(1).call("KERNEL", WIN_EXEC);
    code.emit(&[0xBF, 0xC8, 0x00]); // mov di, 200
    // Until the child has the bytes.
    let held = code.here();
    lock_call(&mut code, 0x00, 0, 4);
    let child_has = jump(&mut code, 0x72); // jc
    lock_call(&mut code, 0x01, 0, 4);
    code.call("KERNEL", YIELD);
    code.emit(&[0x4F]); // dec di
    let back = jump(&mut code, 0x75); // jnz
    code.bytes[back as usize] = (held as i32 - (back as i32 + 1)) as i8 as u8;
    exit_with(&mut code, 2);
    land(&mut code, child_has);
    // Until they are free again.
    code.emit(&[0xBF, 0xC8, 0x00]); // mov di, 200
    let free = code.here();
    code.call("KERNEL", YIELD);
    lock_call(&mut code, 0x00, 0, 4);
    let done = jump(&mut code, 0x73); // jnc
    code.emit(&[0x4F]); // dec di
    let back = jump(&mut code, 0x75); // jnz
    code.bytes[back as usize] = (free as i32 - (back as i32 + 1)) as i8 as u8;
    exit_with(&mut code, 3);
    land(&mut code, done);
    exit_with(&mut code, 0x42);
    name_at(&mut code, name, b"DATA.BIN\0");
    name_at(&mut code, path, b"CHILD.EXE\0");
    let child = program(&child, CODE);
    let (exit, log) = run_with("task_end_locks", program(&code, CODE), |_, base| {
        fs::write(base.join("DATA.BIN"), b"0123456789").unwrap();
        fs::write(base.join("CHILD.EXE"), child).unwrap();
    });
    assert_eq!(exit, 0x42, "{log}");
}
