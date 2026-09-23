use iced_x86::Register;
use rust_dos::cpu::{Cpu, CpuFlags};
use rust_dos::disk::{DriveKind, MountOptions};
use rust_dos::interrupts::int21;
use std::fs;
use std::path::PathBuf;

const DRIVE_A: u8 = 0;
const DRIVE_C: u8 = 2;
const DRIVE_D: u8 = 3;
const DTA: usize = 0x40000; // 4000:0000

/// Fresh directory tree under target/ with the given subdirectories.
fn scratch(name: &str, dirs: &[&str]) -> PathBuf {
    let base = PathBuf::from("target/test_drive_int21").join(name);
    let _ = fs::remove_dir_all(&base);
    for d in dirs {
        fs::create_dir_all(base.join(d)).unwrap();
    }
    base
}

fn opts(kind: DriveKind, label: Option<&str>) -> MountOptions {
    MountOptions {
        kind,
        label: label.map(str::to_string),
        read_only: false,
    }
}

fn int21(cpu: &mut Cpu, ah: u8) {
    cpu.set_reg8(Register::AH, ah);
    int21::handle(cpu);
}

/// Put an ASCIIZ string at 2000:0000 and point DS:DX at it.
fn set_dsdx_string(cpu: &mut Cpu, s: &str) {
    let base = 0x20000;
    for (i, b) in s.bytes().chain(std::iter::once(0)).enumerate() {
        cpu.bus.write_8(base + i, b);
    }
    cpu.set_ds(0x2000);
    cpu.set_dx(0);
}

fn cf(cpu: &Cpu) -> bool {
    cpu.get_cpu_flag(CpuFlags::CF)
}

fn set_dta(cpu: &mut Cpu) {
    cpu.set_ds(0x4000);
    cpu.set_dx(0);
    int21(cpu, 0x1A);
}

fn dta_name(cpu: &Cpu) -> String {
    (0..13)
        .map(|i| cpu.bus.read_8(DTA + 0x1E + i))
        .take_while(|&b| b != 0)
        .map(|b| b as char)
        .collect()
}

fn get_cwd(cpu: &mut Cpu, dl: u8) -> Option<String> {
    cpu.set_reg8(Register::DL, dl);
    cpu.set_ds(0x3000);
    cpu.set_si(0);
    cpu.set_cpu_flag(CpuFlags::CF, false);
    int21(cpu, 0x47);
    if cf(cpu) {
        return None;
    }
    Some(
        (0..64)
            .map(|i| cpu.bus.read_8(0x30000 + i))
            .take_while(|&b| b != 0)
            .map(|b| b as char)
            .collect(),
    )
}

#[test]
fn drive_selection_and_per_drive_directories() {
    let base = scratch("select", &["c/CSUB", "d/DSUB"]);
    let mut cpu = Cpu::new(base.join("c"));
    cpu.bus
        .mount_drive(DRIVE_D, &base.join("d"), MountOptions::default(), false)
        .unwrap();

    cpu.set_reg8(Register::DL, DRIVE_D);
    int21(&mut cpu, 0x0E);
    assert_eq!(cpu.get_reg8(Register::AL), 26);
    assert_eq!(cpu.bus.disk.get_current_drive(), DRIVE_D);

    // Unmounted E: is refused
    cpu.set_reg8(Register::DL, 4);
    int21(&mut cpu, 0x0E);
    int21(&mut cpu, 0x19);
    assert_eq!(cpu.get_reg8(Register::AL), DRIVE_D);

    // CHDIR on C: while D: is current changes only C:'s directory
    set_dsdx_string(&mut cpu, "C:\\CSUB");
    int21(&mut cpu, 0x3B);
    assert!(!cf(&cpu));
    assert_eq!(cpu.bus.disk.get_current_drive(), DRIVE_D);
    assert_eq!(get_cwd(&mut cpu, 3).as_deref(), Some("CSUB"));
    assert_eq!(get_cwd(&mut cpu, 0).as_deref(), Some(""));

    set_dsdx_string(&mut cpu, "DSUB");
    int21(&mut cpu, 0x3B);
    assert!(!cf(&cpu));
    assert_eq!(get_cwd(&mut cpu, 4).as_deref(), Some("DSUB"));
    assert_eq!(get_cwd(&mut cpu, 3).as_deref(), Some("CSUB"));

    // Invalid drive: CF=1, AX=0Fh
    assert_eq!(get_cwd(&mut cpu, 6), None);
    assert_eq!(cpu.ax(), 0x0F);

    // Relative open resolves against D:'s directory
    fs::write(base.join("d/DSUB/FILE.TXT"), b"hi").unwrap();
    set_dsdx_string(&mut cpu, "D:FILE.TXT");
    cpu.set_reg8(Register::AL, 0);
    int21(&mut cpu, 0x3D);
    assert!(!cf(&cpu));
}

#[test]
fn free_space_allocation_info_and_dpb() {
    let base = scratch("space", &["c", "a", "cd"]);
    let mut cpu = Cpu::new(base.join("c"));
    cpu.bus
        .mount_drive(
            DRIVE_A,
            &base.join("a"),
            opts(DriveKind::Floppy, None),
            false,
        )
        .unwrap();
    cpu.bus
        .mount_drive(
            DRIVE_D,
            &base.join("cd"),
            opts(DriveKind::CdRom, None),
            false,
        )
        .unwrap();

    cpu.set_reg8(Register::DL, 1); // A:
    int21(&mut cpu, 0x36);
    assert_eq!((cpu.ax(), cpu.cx(), cpu.dx(), cpu.bx()), (1, 512, 2847, 2847));

    fs::write(base.join("a/SAVE.DAT"), vec![0u8; 1024]).unwrap();
    cpu.set_reg8(Register::DL, 1);
    int21(&mut cpu, 0x36);
    assert_eq!(cpu.bx(), 2845);

    cpu.set_reg8(Register::DL, 4); // D: CD-ROM
    int21(&mut cpu, 0x36);
    assert_eq!((cpu.bx(), cpu.cx()), (0, 2048));

    cpu.set_reg8(Register::DL, 3); // C: unchanged fake 80 MB
    int21(&mut cpu, 0x36);
    assert_eq!((cpu.ax(), cpu.bx(), cpu.cx(), cpu.dx()), (8, 20000, 512, 20000));

    cpu.set_reg8(Register::DL, 7); // G: not mounted
    int21(&mut cpu, 0x36);
    assert_eq!(cpu.ax(), 0xFFFF);

    // AH=1Ch: DS:BX -> media descriptor
    cpu.set_reg8(Register::DL, 1);
    int21(&mut cpu, 0x1C);
    let media = cpu.bus.read_8(cpu.get_physical_addr(cpu.ds(), cpu.bx()));
    assert_eq!((cpu.get_reg8(Register::AL), cpu.cx(), media), (1, 512, 0xF0));
    int21(&mut cpu, 0x1B); // default drive (C:)
    let media = cpu.bus.read_8(cpu.get_physical_addr(cpu.ds(), cpu.bx()));
    assert_eq!((cpu.get_reg8(Register::AL), media), (8, 0xF8));
    cpu.set_reg8(Register::DL, 7);
    int21(&mut cpu, 0x1C);
    assert_eq!(cpu.get_reg8(Register::AL), 0xFF);

    // AH=32h: DPB for C:, none for the CD-ROM or unmounted drives
    cpu.set_reg8(Register::DL, 3);
    int21(&mut cpu, 0x32);
    assert_eq!(cpu.get_reg8(Register::AL), 0);
    let dpb = cpu.get_physical_addr(cpu.ds(), cpu.bx());
    assert_eq!(cpu.bus.read_8(dpb), DRIVE_C);
    assert_eq!(cpu.bus.read_16(dpb + 2), 512);
    assert_eq!(cpu.bus.read_8(dpb + 0x17), 0xF8);
    for dl in [4, 7] {
        cpu.set_reg8(Register::DL, dl);
        int21(&mut cpu, 0x32);
        assert_eq!(cpu.get_reg8(Register::AL), 0xFF, "DL={}", dl);
    }
}

#[test]
fn ioctl_reports_drive_types() {
    let base = scratch("ioctl", &["c", "a", "cd"]);
    fs::write(base.join("cd/DATA.DAT"), b"x").unwrap();
    let mut cpu = Cpu::new(base.join("c"));
    cpu.bus
        .mount_drive(
            DRIVE_A,
            &base.join("a"),
            opts(DriveKind::Floppy, None),
            false,
        )
        .unwrap();
    cpu.bus
        .mount_drive(
            DRIVE_D,
            &base.join("cd"),
            opts(DriveKind::CdRom, None),
            false,
        )
        .unwrap();

    let removable = |cpu: &mut Cpu, bl: u8| {
        cpu.set_reg8(Register::BL, bl);
        cpu.set_reg8(Register::AL, 0x08);
        int21(cpu, 0x44);
        (cf(cpu), cpu.ax())
    };
    assert_eq!(removable(&mut cpu, 1), (false, 0)); // A: floppy
    assert_eq!(removable(&mut cpu, 3), (false, 1)); // C: fixed
    assert_eq!(removable(&mut cpu, 0), (false, 1)); // default = C:
    assert_eq!(removable(&mut cpu, 4), (true, 1)); // D: CD-ROM (unsupported)
    assert_eq!(removable(&mut cpu, 7), (true, 0x0F)); // invalid

    let remote = |cpu: &mut Cpu, bl: u8| {
        cpu.set_reg8(Register::BL, bl);
        cpu.set_reg8(Register::AL, 0x09);
        int21(cpu, 0x44);
        (cf(cpu), cpu.dx())
    };
    assert_eq!(remote(&mut cpu, 3), (false, 0x0802));
    assert_eq!(remote(&mut cpu, 4), (false, 0x1000));
    assert!(remote(&mut cpu, 7).0);

    // Device info for a file handle carries its drive number
    set_dsdx_string(&mut cpu, "D:\\DATA.DAT");
    cpu.set_reg8(Register::AL, 0);
    int21(&mut cpu, 0x3D);
    assert!(!cf(&cpu));
    cpu.set_bx(cpu.ax());
    cpu.set_reg8(Register::AL, 0x00);
    int21(&mut cpu, 0x44);
    assert_eq!(cpu.dx(), DRIVE_D as u16);
}

#[test]
fn read_only_drives_reject_writes() {
    let base = scratch("readonly", &["c", "cd", "ro"]);
    fs::write(base.join("cd/DATA.DAT"), b"cd").unwrap();
    let mut cpu = Cpu::new(base.join("c"));
    cpu.bus
        .mount_drive(
            DRIVE_D,
            &base.join("cd"),
            opts(DriveKind::CdRom, None),
            false,
        )
        .unwrap();
    let ro = MountOptions {
        read_only: true,
        ..Default::default()
    };
    cpu.bus.mount_drive(4, &base.join("ro"), ro, false).unwrap();

    for name in ["D:\\NEW.TXT", "E:\\NEW.TXT"] {
        set_dsdx_string(&mut cpu, name);
        cpu.set_cx(0);
        int21(&mut cpu, 0x3C);
        assert!(cf(&cpu), "{}", name);
        assert_eq!(cpu.ax(), 0x05);
    }

    set_dsdx_string(&mut cpu, "D:\\DATA.DAT");
    cpu.set_reg8(Register::AL, 0x01);
    int21(&mut cpu, 0x3D);
    assert_eq!((cf(&cpu), cpu.ax()), (true, 0x05));

    // Read/write open is downgraded; the write then fails with CF set
    cpu.set_reg8(Register::AL, 0x02);
    int21(&mut cpu, 0x3D);
    assert!(!cf(&cpu));
    cpu.set_bx(cpu.ax());
    cpu.set_cx(1);
    int21(&mut cpu, 0x40);
    assert_eq!((cf(&cpu), cpu.ax()), (true, 0x05));

    set_dsdx_string(&mut cpu, "D:\\NEWDIR");
    int21(&mut cpu, 0x39);
    assert_eq!((cf(&cpu), cpu.ax()), (true, 0x05));

    set_dsdx_string(&mut cpu, "D:\\DATA.DAT");
    cpu.set_reg8(Register::AL, 0x00);
    int21(&mut cpu, 0x43);
    assert!(!cf(&cpu));
    assert_eq!(cpu.cx() & 0x01, 0x01);

    assert_eq!(fs::read(base.join("cd/DATA.DAT")).unwrap(), b"cd");
    assert!(!base.join("cd/NEW.TXT").exists());
    assert!(!base.join("ro/NEW.TXT").exists());
}

#[test]
fn find_next_stays_on_the_searched_drive() {
    let base = scratch("findnext", &["c", "d"]);
    for f in ["C1.TXT", "C2.TXT", "C3.TXT"] {
        fs::write(base.join("c").join(f), b"c").unwrap();
    }
    for f in ["D1.TXT", "D2.TXT"] {
        fs::write(base.join("d").join(f), b"d").unwrap();
    }
    let mut cpu = Cpu::new(base.join("c"));
    cpu.bus
        .mount_drive(DRIVE_D, &base.join("d"), MountOptions::default(), false)
        .unwrap();
    set_dta(&mut cpu);

    set_dsdx_string(&mut cpu, "D:*.*");
    cpu.set_cx(0x10);
    int21(&mut cpu, 0x4E);
    assert!(!cf(&cpu));
    assert_eq!(cpu.bus.read_8(DTA), 4); // D: (1-based)
    let mut names = vec![dta_name(&cpu)];

    // Switching the current drive must not redirect FindNext
    cpu.set_reg8(Register::DL, DRIVE_C);
    int21(&mut cpu, 0x0E);
    loop {
        int21(&mut cpu, 0x4F);
        if cf(&cpu) {
            break;
        }
        names.push(dta_name(&cpu));
    }
    assert_eq!(names, ["D1.TXT", "D2.TXT"]);

    // "C:*.*" used to store "C" as the directory and lose FindNext
    set_dsdx_string(&mut cpu, "C:*.*");
    cpu.set_cx(0x10);
    int21(&mut cpu, 0x4E);
    assert!(!cf(&cpu));
    let mut names = vec![dta_name(&cpu)];
    loop {
        int21(&mut cpu, 0x4F);
        if cf(&cpu) {
            break;
        }
        names.push(dta_name(&cpu));
    }
    assert_eq!(names, ["C1.TXT", "C2.TXT", "C3.TXT"]);
}

#[test]
fn volume_labels_are_per_drive() {
    let base = scratch("labels", &["c", "a"]);
    fs::write(base.join("c/RUSTDOS"), b"not a label").unwrap();
    let mut cpu = Cpu::new(base.join("c"));
    cpu.bus
        .mount_drive(
            DRIVE_A,
            &base.join("a"),
            opts(DriveKind::Floppy, Some("mydisk")),
            false,
        )
        .unwrap();
    set_dta(&mut cpu);

    set_dsdx_string(&mut cpu, "A:*.*");
    cpu.set_cx(0x08);
    int21(&mut cpu, 0x4E);
    assert!(!cf(&cpu));
    assert_eq!(dta_name(&cpu), "MYDISK");
    assert_eq!(cpu.bus.read_8(DTA + 21), 0x08);

    // A plain file that happens to be called RUSTDOS is just a file
    set_dsdx_string(&mut cpu, "C:RUSTDOS");
    cpu.set_cx(0x00);
    int21(&mut cpu, 0x4E);
    assert!(!cf(&cpu));
    assert_eq!(dta_name(&cpu), "RUSTDOS");
    assert_eq!(cpu.bus.read_8(DTA + 21), 0x20);
}

#[test]
fn fcb_search_honors_drive_byte_and_extended_fcbs() {
    let base = scratch("fcb", &["c", "a"]);
    fs::write(base.join("a/GAME.EXE"), b"x").unwrap();
    let mut cpu = Cpu::new(base.join("c"));
    cpu.bus
        .mount_drive(
            DRIVE_A,
            &base.join("a"),
            opts(DriveKind::Floppy, Some("DISK1")),
            false,
        )
        .unwrap();
    set_dta(&mut cpu);

    let fcb = 0x50000;
    let write_fcb = |cpu: &mut Cpu, at: usize, drive: u8| {
        cpu.bus.write_8(at, drive);
        for i in 0..11 {
            cpu.bus.write_8(at + 1 + i, b'?');
        }
    };

    write_fcb(&mut cpu, fcb, 1); // A:
    cpu.set_ds(0x5000);
    cpu.set_dx(0);
    int21(&mut cpu, 0x11);
    assert_eq!(cpu.get_reg8(Register::AL), 0);
    assert_eq!(cpu.bus.read_8(DTA), 1);
    let name: Vec<u8> = (1..12).map(|i| cpu.bus.read_8(DTA + i)).collect();
    assert_eq!(name, b"GAME    EXE");

    write_fcb(&mut cpu, fcb, 6); // F: not mounted
    int21(&mut cpu, 0x11);
    assert_eq!(cpu.get_reg8(Register::AL), 0xFF);

    // Extended FCB asking for the volume label of A:
    cpu.bus.write_8(fcb, 0xFF);
    for i in 1..6 {
        cpu.bus.write_8(fcb + i, 0);
    }
    cpu.bus.write_8(fcb + 6, 0x08);
    write_fcb(&mut cpu, fcb + 7, 1);
    int21(&mut cpu, 0x11);
    assert_eq!(cpu.get_reg8(Register::AL), 0);
    assert_eq!(cpu.bus.read_8(DTA), 0xFF);
    assert_eq!(cpu.bus.read_8(DTA + 6), 0x08);
    assert_eq!(cpu.bus.read_8(DTA + 7), 1);
    let label: Vec<u8> = (8..19).map(|i| cpu.bus.read_8(DTA + i)).collect();
    assert_eq!(label, b"DISK1      ");
}

#[test]
fn parse_filename_handles_drive_prefixes() {
    let base = scratch("parse", &["c", "d"]);
    let mut cpu = Cpu::new(base.join("c"));
    cpu.bus
        .mount_drive(DRIVE_D, &base.join("d"), MountOptions::default(), false)
        .unwrap();
    let fcb = 0x30000;

    let parse = |cpu: &mut Cpu, text: &str, al: u8| {
        for (i, b) in text.bytes().chain(std::iter::once(0)).enumerate() {
            cpu.bus.write_8(0x20000 + i, b);
        }
        cpu.set_ds(0x2000);
        cpu.set_si(0);
        cpu.set_es(0x3000);
        cpu.set_di(0);
        cpu.set_reg8(Register::AL, al);
        int21(cpu, 0x29);
        cpu.get_reg8(Register::AL)
    };

    assert_eq!(parse(&mut cpu, "D:TEST.TXT", 0), 0);
    assert_eq!(cpu.bus.read_8(fcb), 4);
    let name: Vec<u8> = (1..12).map(|i| cpu.bus.read_8(fcb + i)).collect();
    assert_eq!(name, b"TEST    TXT");
    assert_eq!(cpu.si(), 10);

    assert_eq!(parse(&mut cpu, "E:X.Y", 0), 0xFF);

    // AL bit 1: keep the existing drive byte when none is given
    cpu.bus.write_8(fcb, 5);
    parse(&mut cpu, "NAME.EXT", 0x02);
    assert_eq!(cpu.bus.read_8(fcb), 5);
    parse(&mut cpu, "NAME.EXT", 0x00);
    assert_eq!(cpu.bus.read_8(fcb), 0);
}

#[test]
fn the_ultrasound_patches_are_on_a_drive_of_their_own() {
    let base = scratch("ultrasnd", &["c"]);
    let mut cpu = Cpu::new(base.join("c"));
    let ultradir = cpu.get_env("ULTRADIR").unwrap().to_string();
    assert_eq!(ultradir, "X:\\ULTRASND");
    set_dta(&mut cpu);

    // The patches, where a game looks for them.
    set_dsdx_string(&mut cpu, &format!("{}\\MIDI\\*.PAT", ultradir));
    cpu.set_cx(0);
    int21(&mut cpu, 0x4E);
    assert!(!cf(&cpu));
    assert_eq!(cpu.bus.read_8(DTA + 0x15), 0x21); // read-only
    let mut names = vec![dta_name(&cpu)];
    loop {
        int21(&mut cpu, 0x4F);
        if cf(&cpu) {
            break;
        }
        names.push(dta_name(&cpu));
    }
    let patches = rust_dos::gus::builtin::FILES.iter().filter(|(p, _)| p.ends_with(".PAT")).count();
    assert_eq!(names.len(), patches);
    assert!(names.iter().any(|n| n == "ACPIANO.PAT"));

    // Read one from its directory; read/write opens are read-only.
    set_dsdx_string(&mut cpu, "X:\\ULTRASND\\MIDI");
    int21(&mut cpu, 0x3B);
    assert!(!cf(&cpu));
    set_dsdx_string(&mut cpu, "X:ACPIANO.PAT");
    cpu.set_reg8(Register::AL, 0x02);
    int21(&mut cpu, 0x3D);
    assert!(!cf(&cpu));
    let handle = cpu.ax();
    cpu.set_bx(handle);
    cpu.set_cx(12);
    cpu.set_ds(0x5000);
    cpu.set_dx(0);
    int21(&mut cpu, 0x3F);
    assert!(!cf(&cpu));
    assert_eq!(cpu.ax(), 12);
    let magic: Vec<u8> = (0..12).map(|i| cpu.bus.read_8(0x50000 + i)).collect();
    assert_eq!(magic, b"GF1PATCH110\0");
    cpu.set_bx(handle);
    cpu.set_cx(1);
    int21(&mut cpu, 0x40);
    assert!(cf(&cpu));
    assert_eq!(cpu.ax(), 0x05);

    set_dsdx_string(&mut cpu, "X:NEW.PAT");
    cpu.set_cx(0);
    int21(&mut cpu, 0x3C);
    assert!(cf(&cpu));
    assert_eq!(cpu.ax(), 0x05);
}
