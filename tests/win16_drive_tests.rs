//! Win16 KERNEL's drives: what a Windows 3.1 program is told about the
//! machine's drives on the built-in Windows. The programs are made byte
//! by byte (tests/win16).

mod win16;

use rust_dos::disk::{DriveKind, MountOptions};
use std::fs;
use win16::{CODE, Code, program, run_with};

/// KERNEL's ordinals.
const GET_DRIVE_TYPE: u16 = 136;

#[test]
fn a_cd_rom_drive_is_remote_and_mscdex_says_it_is_one() {
    // GetDriveType(D:) in the low nibble of the exit code, and 10h more
    // if MSCDEX (INT 2Fh AX=150Bh) says D: is a CD-ROM drive, as setup
    // programs look for their disc: 14h on Windows 3.1, 95 and 98.
    let mut code = Code::new();
    code.push(3) // D:
        .call("KERNEL", GET_DRIVE_TYPE)
        .emit(&[
            0x8B, 0xF0, // mov si,ax
            0xB8, 0x0B, 0x15, // mov ax,150Bh: is it a CD-ROM drive?
            0xB9, 0x03, 0x00, // mov cx,3
            0xCD, 0x2F, // int 2Fh
            0x81, 0xFB, 0xAD, 0xAD, // cmp bx,0ADADh: MSCDEX is there
            0x75, 0x07, // jne done
            0x0B, 0xC0, // or ax,ax
            0x74, 0x03, // jz done
            0x83, 0xC6, 0x10, // add si,10h
            // done:
            0x8B, 0xC6, // mov ax,si
        ])
        .exit_al();
    let (exit, log) = run_with("cd_rom_drive", program(&code, CODE), |cpu, base| {
        let cd = base.join("cd");
        fs::create_dir_all(&cd).unwrap();
        fs::write(cd.join("DISC.TXT"), b"disc").unwrap();
        let opts = MountOptions {
            kind: DriveKind::CdRom,
            ..Default::default()
        };
        cpu.bus.mount_drive(3, &cd, opts, false).unwrap();
    });
    assert_eq!(exit, 0x14, "{log}");
}

#[test]
fn a_hard_disk_is_fixed_and_a_missing_drive_is_0() {
    // GetDriveType(C:) and GetDriveType(F:), there being no F:, as the
    // two nibbles of the exit code: 30h.
    let mut code = Code::new();
    code.push(2) // C:
        .call("KERNEL", GET_DRIVE_TYPE)
        .emit(&[0x8B, 0xF0]) // mov si,ax
        .push(5) // F:
        .call("KERNEL", GET_DRIVE_TYPE)
        .emit(&[
            0x8B, 0xCE, // mov cx,si
            0xC0, 0xE1, 0x04, // shl cl,4
            0x0A, 0xC1, // or al,cl
        ])
        .exit_al();
    let (exit, log) = run_with("hard_disk_drive", program(&code, CODE), |_, _| {});
    assert_eq!(exit, 0x30, "{log}");
}

#[test]
fn a_floppy_is_removable_and_z_is_fixed() {
    // GetDriveType(A:), a floppy, and GetDriveType(Z:), the drive held in
    // memory, as the two nibbles of the exit code: 23h. Windows 3.1 has
    // no DRIVE_RAMDISK, so Z: is fixed, as a RAM drive is.
    let mut code = Code::new();
    code.push(0) // A:
        .call("KERNEL", GET_DRIVE_TYPE)
        .emit(&[0x8B, 0xF0]) // mov si,ax
        .push(25) // Z:
        .call("KERNEL", GET_DRIVE_TYPE)
        .emit(&[
            0x8B, 0xCE, // mov cx,si
            0xC0, 0xE1, 0x04, // shl cl,4
            0x0A, 0xC1, // or al,cl
        ])
        .exit_al();
    let (exit, log) = run_with("floppy_drive", program(&code, CODE), |cpu, base| {
        let a = base.join("a");
        fs::create_dir_all(&a).unwrap();
        let opts = MountOptions {
            kind: DriveKind::Floppy,
            ..Default::default()
        };
        cpu.bus.mount_drive(0, &a, opts, false).unwrap();
    });
    assert_eq!(exit, 0x23, "{log}");
}

/// KERNEL's GetCurrentDirectory: Windows 3.1's PROGMAN.EXE calls it.
const GET_CURRENT_DIRECTORY: u16 = 411;

/// A program that calls GetCurrentDirectory(size, buffer) with a 64-byte
/// buffer on its stack, then leaves the buffer's address in BX. Its exit
/// code is the function's return value, and the buffer's bytes are left
/// for the caller's checks.
fn get_current_directory_call(size: u16) -> Code {
    let mut code = Code::new();
    code.emit(&[0x83, 0xEC, 0x40]) // sub sp,40h
        .emit(&[0x8B, 0xDC]) // mov bx,sp: the buffer
        .push(0) // length, high word
        .push(size) // length, low word
        .emit(&[0x16]) // push ss
        .emit(&[0x53]) // push bx
        .call("KERNEL", GET_CURRENT_DIRECTORY)
        .emit(&[0x8B, 0xDC]); // mov bx,sp: the arguments are popped
    code
}

#[test]
fn get_current_directory_copies_the_drive_path() {
    // The current directory is C:\GAMES\DOS. The buffer holds it with
    // its NUL, and the return value is its length, 12. Exit code 12 if
    // the buffer's bytes are as expected (C, the backslash after C:, S
    // and NUL), 0xEE if not.
    let mut code = get_current_directory_call(64);
    let checks: [&[u8]; 4] = [
        &[0x36, 0x80, 0x3F, 0x43],       // cmp ss:[bx],'C'
        &[0x36, 0x80, 0x7F, 0x02, 0x5C], // cmp ss:[bx+2],'\'
        &[0x36, 0x80, 0x7F, 0x0B, 0x53], // cmp ss:[bx+11],'S'
        &[0x36, 0x80, 0x7F, 0x0C, 0x00], // cmp ss:[bx+12],0
    ];
    for (i, check) in checks.iter().enumerate() {
        // Past this jump: the checks after it, and the 4-byte exit.
        let rest: usize = checks[i + 1..].iter().map(|c| c.len() + 2).sum::<usize>() + 4;
        code.emit(check).emit(&[0x75, rest as u8]); // jne fail
    }
    code.exit_al() // AL = 12
        .emit(&[0xB0, 0xEE]) // fail: mov al,0EEh
        .exit_al();
    let (exit, log) = run_with(
        "get_current_directory",
        program(&code, CODE),
        |cpu, base| {
            fs::create_dir_all(base.join("GAMES").join("DOS")).unwrap();
            assert!(cpu.bus.disk.set_current_directory("GAMES\\DOS"));
        },
    );
    assert_eq!(exit, 12, "{log}");
}

#[test]
fn get_current_directory_at_the_root_and_a_small_buffer() {
    // At the root the path is "C:\", 3 bytes, and a 4-byte buffer holds
    // it: the first return value is 3. A 2-byte buffer is too small, so
    // the second call returns the 4 needed with the NUL. Exit code: the
    // first value in the high nibble, the second in the low one: 34h.
    let mut code = get_current_directory_call(4);
    code.emit(&[0x8B, 0xF0]) // mov si,ax
        .emit(&[0x83, 0xEC, 0x40]) // sub sp,40h
        .push(0) // length, high word
        .push(2) // length, low word
        .emit(&[0x16, 0x54]) // push ss; push sp: the buffer is not written
        .call("KERNEL", GET_CURRENT_DIRECTORY)
        .emit(&[
            0xB1, 0x04, // mov cl,4
            0xD3, 0xE6, // shl si,cl
            0x0B, 0xC6, // or ax,si
        ])
        .exit_al();
    let (exit, log) = run_with(
        "get_current_directory_root",
        program(&code, CODE),
        |cpu, _| {
            assert!(cpu.bus.disk.set_current_directory("\\"));
        },
    );
    assert_eq!(exit, 0x34, "{log}");
}

/// KERNEL's SetCurrentDirectory, the next export Windows 3.1's PROGMAN.EXE
/// calls after GetCurrentDirectory.
const SET_CURRENT_DIRECTORY: u16 = 412;

/// A program that calls SetCurrentDirectory(path) with the path's bytes
/// in its code segment, past a jump over them. Its exit code is the
/// function's return value, in AL.
fn set_current_directory_call(path: &str) -> Code {
    let mut bytes = path.as_bytes().to_vec();
    bytes.push(0);
    let mut code = Code::new();
    code.emit(&[0xEB, bytes.len() as u8]); // jmp over the path
    let at = code.here();
    code.emit(&bytes)
        .push_far(at) // the path, as a far pointer into this segment
        .call("KERNEL", SET_CURRENT_DIRECTORY);
    code
}

#[test]
fn set_current_directory_changes_the_drive_path() {
    // SetCurrentDirectory("GAMES\DOS") returns TRUE (kept in SI), and then
    // GetCurrentDirectory returns 12, the length of C:\GAMES\DOS. Exit
    // code 12 if SI was TRUE, 0EEh if the call failed.
    let mut code = set_current_directory_call("GAMES\\DOS");
    code.emit(&[0x8B, 0xF0]) // mov si,ax
        .emit(&[0x83, 0xEC, 0x40, 0x8B, 0xDC]) // sub sp,40h; mov bx,sp
        .push(0) // length, high word
        .push(64) // length, low word
        .emit(&[0x16, 0x53]) // push ss; push bx
        .call("KERNEL", GET_CURRENT_DIRECTORY)
        .emit(&[
            0x0B, 0xF6, // or si,si
            0x75, 0x02, // jnz exit: the path was set
            0xB0, 0xEE, // mov al,0EEh
        ])
        .exit_al();
    let (exit, log) = run_with("set_current_directory", program(&code, CODE), |_, base| {
        fs::create_dir_all(base.join("GAMES").join("DOS")).unwrap();
    });
    assert_eq!(exit, 12, "{log}");
}

#[test]
fn set_current_directory_of_a_missing_directory_is_false() {
    // There is no NOPE directory, so SetCurrentDirectory returns FALSE.
    let mut code = set_current_directory_call("NOPE");
    code.exit_al();
    let (exit, log) = run_with(
        "set_current_directory_missing",
        program(&code, CODE),
        |_, _| {},
    );
    assert_eq!(exit, 0, "{log}");
}

#[test]
fn set_current_directory_of_an_empty_path_is_false() {
    // An empty path names no directory, so the call returns FALSE, as
    // Win32's does, and the current directory stays C:\.
    let mut code = set_current_directory_call("");
    code.exit_al();
    let (exit, log) = run_with(
        "set_current_directory_empty",
        program(&code, CODE),
        |_, _| {},
    );
    assert_eq!(exit, 0, "{log}");
}

#[test]
fn set_current_directory_on_another_drive_makes_it_current() {
    // SetCurrentDirectory("D:\DATA") makes D: the current drive, so
    // GetCurrentDirectory returns "D:\DATA", 7 bytes. While C: were
    // current it would return "C:\", 3. Exit code 7.
    let mut code = set_current_directory_call("D:\\DATA");
    code.emit(&[0x83, 0xEC, 0x40, 0x8B, 0xDC]) // sub sp,40h; mov bx,sp
        .push(0) // length, high word
        .push(64) // length, low word
        .emit(&[0x16, 0x53]) // push ss; push bx
        .call("KERNEL", GET_CURRENT_DIRECTORY)
        .exit_al();
    let (exit, log) = run_with(
        "set_current_directory_drive",
        program(&code, CODE),
        |cpu, base| {
            let data = base.join("d");
            fs::create_dir_all(data.join("DATA")).unwrap();
            cpu.bus
                .mount_drive(3, &data, MountOptions::default(), false)
                .unwrap();
        },
    );
    assert_eq!(exit, 7, "{log}");
}
