use iced_x86::Register;
use rust_dos::bus::MEDIA_ID_TABLE;
use rust_dos::cpu::{Cpu, CpuFlags};
use rust_dos::disk::{DriveKind, MountOptions};
use rust_dos::interrupts::{int11, int13};
use std::fs;
use std::path::PathBuf;

fn scratch(name: &str, dirs: &[&str]) -> PathBuf {
    let base = PathBuf::from("target/test_bios_drive").join(name);
    let _ = fs::remove_dir_all(&base);
    for d in dirs {
        fs::create_dir_all(base.join(d)).unwrap();
    }
    base
}

fn kind(kind: DriveKind) -> MountOptions {
    MountOptions {
        kind,
        ..Default::default()
    }
}

fn int13(cpu: &mut Cpu, ah: u8, dl: u8) -> (bool, u8) {
    cpu.set_reg8(Register::AH, ah);
    cpu.set_reg8(Register::AL, 1);
    cpu.set_reg8(Register::DL, dl);
    int13::handle(cpu);
    (cpu.get_cpu_flag(CpuFlags::CF), cpu.get_reg8(Register::AH))
}

#[test]
fn equipment_word_and_hard_disk_count_follow_mounts() {
    let base = scratch("bda", &["c", "a", "b", "d"]);
    let mut cpu = Cpu::new(base.join("c"));

    // Only C: (hdd): no floppy bit, one fixed disk, video bits intact, and
    // the PS/2 mouse
    assert_eq!(cpu.bus.read_16(0x0410), 0x0024);
    assert_eq!(cpu.bus.read_8(0x0475), 1);

    cpu.bus
        .mount_drive(0, &base.join("a"), kind(DriveKind::Floppy), false)
        .unwrap();
    assert_eq!(cpu.bus.read_16(0x0410), 0x0025);

    cpu.bus
        .mount_drive(1, &base.join("b"), kind(DriveKind::Floppy), false)
        .unwrap();
    assert_eq!(cpu.bus.read_16(0x0410), 0x0065); // bits 6-7 = 2 drives - 1

    cpu.bus
        .mount_drive(3, &base.join("d"), MountOptions::default(), false)
        .unwrap();
    assert_eq!(cpu.bus.read_8(0x0475), 2);

    cpu.set_reg8(Register::AH, 0);
    int11::handle(&mut cpu);
    assert_eq!(cpu.ax(), 0x0065);

    // Survives the shell reload, which clears RAM from 0x500 up
    cpu.load_shell();
    assert_eq!(cpu.bus.read_16(0x0410), 0x0065);
    assert_eq!(cpu.bus.read_8(0x0475), 2);
    assert_eq!(cpu.bus.read_8(MEDIA_ID_TABLE), 0xF0);
    assert_eq!(cpu.bus.read_8(MEDIA_ID_TABLE + 3), 0xF8);

    cpu.bus.unmount_drive(1).unwrap();
    cpu.bus.unmount_drive(0).unwrap();
    cpu.bus.unmount_drive(3).unwrap();
    assert_eq!(cpu.bus.read_16(0x0410), 0x0024);
    assert_eq!(cpu.bus.read_8(0x0475), 1);
    assert_eq!(cpu.bus.read_8(MEDIA_ID_TABLE), 0);
}

#[test]
fn int13_units_map_to_mounted_drives() {
    let base = scratch("int13", &["c", "a", "cd", "e"]);
    let mut cpu = Cpu::new(base.join("c"));

    // No floppy mounted: unit 0 times out, AH=08h reports zero floppies
    assert_eq!(int13(&mut cpu, 0x02, 0x00), (true, 0x80));
    assert_eq!(int13(&mut cpu, 0x15, 0x00), (false, 0x00));
    assert_eq!(int13(&mut cpu, 0x08, 0x00), (false, 0x00));
    assert_eq!(cpu.get_reg8(Register::DL), 0);

    cpu.bus
        .mount_drive(0, &base.join("a"), kind(DriveKind::Floppy), false)
        .unwrap();
    assert_eq!(int13(&mut cpu, 0x02, 0x00), (false, 0x00));
    assert_eq!(int13(&mut cpu, 0x15, 0x00), (false, 0x02));
    assert_eq!(int13(&mut cpu, 0x08, 0x00), (false, 0x00));
    assert_eq!(cpu.get_reg8(Register::DL), 1);
    assert_eq!(cpu.get_reg8(Register::BL), 4);
    // B: isn't a floppy unit
    assert_eq!(int13(&mut cpu, 0x02, 0x01), (true, 0x80));

    // 80h is C:; 81h appears only once a second hdd is mounted. CD-ROMs
    // are not BIOS drives.
    assert_eq!(int13(&mut cpu, 0x15, 0x80), (false, 0x03));
    assert_eq!(int13(&mut cpu, 0x02, 0x81), (true, 0xAA));
    cpu.bus
        .mount_drive(3, &base.join("cd"), kind(DriveKind::CdRom), false)
        .unwrap();
    assert_eq!(int13(&mut cpu, 0x02, 0x81), (true, 0xAA));
    cpu.bus
        .mount_drive(4, &base.join("e"), MountOptions::default(), false)
        .unwrap();
    assert_eq!(int13(&mut cpu, 0x02, 0x81), (false, 0x00));
    assert_eq!(int13(&mut cpu, 0x08, 0x80), (false, 0x00));
    assert_eq!(cpu.get_reg8(Register::DL), 2);

    // DSWAP-style probe with DL=FFh must still fail
    assert_eq!(int13(&mut cpu, 0x08, 0xFF), (true, 0x01));
}

#[test]
fn write_protected_floppy_rejects_bios_writes() {
    let base = scratch("wp", &["c", "a"]);
    let mut cpu = Cpu::new(base.join("c"));
    let ro_floppy = MountOptions {
        kind: DriveKind::Floppy,
        read_only: true,
        ..Default::default()
    };
    cpu.bus
        .mount_drive(0, &base.join("a"), ro_floppy, false)
        .unwrap();
    assert_eq!(int13(&mut cpu, 0x03, 0x00), (true, 0x03));
    assert_eq!(int13(&mut cpu, 0x02, 0x00), (false, 0x00));
}

#[test]
fn a_and_b_are_bios_floppies_whatever_their_type() {
    let base = scratch("ab", &["c", "a", "b"]);
    let mut cpu = Cpu::new(base.join("c"));
    assert_eq!((cpu.bus.cmos.get(0x10), cpu.bus.cmos.get(0x14) & 0xC1), (0x00, 0x00));

    // B: alone: two units, the first without a disk.
    cpu.bus
        .mount_drive(1, &base.join("b"), MountOptions::default(), false)
        .unwrap();
    assert_eq!(cpu.bus.read_16(0x0410), 0x0065);
    assert_eq!((cpu.bus.cmos.get(0x10), cpu.bus.cmos.get(0x14) & 0xC1), (0x44, 0x41));
    assert_eq!(cpu.bus.read_8(MEDIA_ID_TABLE + 1), 0xF0);
    assert_eq!(int13(&mut cpu, 0x15, 0x00), (false, 0x02));
    assert_eq!(int13(&mut cpu, 0x02, 0x00), (true, 0x80));
    assert_eq!(int13(&mut cpu, 0x02, 0x01), (false, 0x00));
    assert_eq!(int13(&mut cpu, 0x15, 0x02), (false, 0x00));

    // AH=08h: 1.44 MB geometry and ES:DI -> the table INT 1Eh points to.
    for unit in [0, 1] {
        assert_eq!(int13(&mut cpu, 0x08, unit), (false, 0x00));
        assert_eq!((cpu.get_reg8(Register::BL), cpu.get_reg8(Register::DL)), (4, 2));
        assert_eq!((cpu.get_reg8(Register::CH), cpu.get_reg8(Register::CL), cpu.get_reg8(Register::DH)), (79, 18, 1));
        assert_eq!((cpu.es(), cpu.get_reg16(Register::DI)), (cpu.bus.read_16(0x1E * 4 + 2), cpu.bus.read_16(0x1E * 4)));
    }
    let table = cpu.get_physical_addr(cpu.es(), cpu.get_reg16(Register::DI));
    assert_eq!((cpu.bus.read_8(table + 3), cpu.bus.read_8(table + 4)), (0x02, 18)); // 512-byte sectors, 18 per track

    cpu.bus.unmount_drive(1).unwrap();
    cpu.bus
        .mount_drive(0, &base.join("a"), kind(DriveKind::HardDisk), false)
        .unwrap();
    assert_eq!(cpu.bus.disk.drive_kind(0), Some(DriveKind::Floppy));
    assert_eq!(cpu.bus.read_16(0x0410), 0x0025);
    assert_eq!((cpu.bus.cmos.get(0x10), cpu.bus.cmos.get(0x14) & 0xC1), (0x40, 0x01));
    // Not a fixed disk: C: stays the only one.
    assert_eq!(cpu.bus.read_8(0x0475), 1);
    assert_eq!(int13(&mut cpu, 0x02, 0x81), (true, 0xAA));
    assert!(cpu.bus.mount_drive(1, &base.join("b"), kind(DriveKind::CdRom), false).is_err());
    assert_eq!(cpu.bus.read_16(0x0410), 0x0025);
}

#[test]
fn floppies_on_other_letters_are_not_bios_units() {
    let base = scratch("floppy_e", &["c", "e"]);
    let mut cpu = Cpu::new(base.join("c"));
    cpu.bus
        .mount_drive(4, &base.join("e"), kind(DriveKind::Floppy), false)
        .unwrap();
    assert_eq!(cpu.bus.read_16(0x0410), 0x0024);
    assert_eq!(int13(&mut cpu, 0x02, 0x04), (true, 0x80));
    assert_eq!(int13(&mut cpu, 0x15, 0x04), (false, 0x00));
}
