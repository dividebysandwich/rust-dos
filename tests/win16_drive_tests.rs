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
