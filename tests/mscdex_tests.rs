use rust_dos::cpu::Cpu;
use rust_dos::disk::{DriveKind, MountOptions};
use rust_dos::interrupts::int2f;
use std::fs;
use std::path::PathBuf;

fn scratch(name: &str, dirs: &[&str]) -> PathBuf {
    let base = PathBuf::from("target/test_mscdex").join(name);
    let _ = fs::remove_dir_all(&base);
    for d in dirs {
        fs::create_dir_all(base.join(d)).unwrap();
    }
    base
}

fn cdrom() -> MountOptions {
    MountOptions {
        kind: DriveKind::CdRom,
        ..Default::default()
    }
}

fn int2f(cpu: &mut Cpu, ax: u16, bx: u16, cx: u16) {
    cpu.ax = ax;
    cpu.bx = bx;
    cpu.cx = cx;
    int2f::handle(cpu);
}

#[test]
fn not_installed_without_cd_drives() {
    let base = scratch("absent", &["c"]);
    let mut cpu = Cpu::new(base.join("c"));
    int2f(&mut cpu, 0x1500, 0, 0);
    assert_eq!((cpu.ax, cpu.bx), (0x1500, 0));
    int2f(&mut cpu, 0x150B, 0, 3);
    assert_ne!(cpu.bx, 0xADAD);

    // Other multiplex install checks stay "not installed"
    for ax in [0x1600u16, 0x1687, 0x4300, 0x1100] {
        int2f(&mut cpu, ax, 0x1234, 0x5678);
        assert_eq!((cpu.ax, cpu.bx, cpu.cx), (ax, 0x1234, 0x5678));
    }
}

#[test]
fn mscdex_reports_mounted_cd_drives() {
    let base = scratch("present", &["c", "cd1", "cd2", "d"]);
    let mut cpu = Cpu::new(base.join("c"));
    cpu.bus
        .mount_drive(3, &base.join("d"), MountOptions::default(), false)
        .unwrap();
    cpu.bus.mount_drive(4, &base.join("cd1"), cdrom(), false).unwrap();
    cpu.bus.mount_drive(6, &base.join("cd2"), cdrom(), false).unwrap();

    int2f(&mut cpu, 0x1500, 0, 0);
    assert_eq!((cpu.bx, cpu.cx), (2, 4));

    int2f(&mut cpu, 0x150B, 0, 4);
    assert_eq!(cpu.bx, 0xADAD);
    assert_ne!(cpu.ax, 0);
    int2f(&mut cpu, 0x150B, 0, 3);
    assert_eq!((cpu.ax, cpu.bx), (0, 0xADAD));

    int2f(&mut cpu, 0x150C, 0, 0);
    assert_eq!(cpu.bx, 0x0217);

    cpu.es = 0x3000;
    int2f(&mut cpu, 0x150D, 0, 0);
    assert_eq!(cpu.bus.read_8(0x30000), 4);
    assert_eq!(cpu.bus.read_8(0x30001), 6);
}

#[test]
fn device_requests_answer_ioctl_queries() {
    let base = scratch("ioctl", &["c", "cd"]);
    let mut cpu = Cpu::new(base.join("c"));
    cpu.bus.mount_drive(3, &base.join("cd"), cdrom(), false).unwrap();

    let header = 0x30000; // 3000:0000
    let buffer = 0x31000; // 3100:0000
    let request = |cpu: &mut Cpu, command: u8, control: u8| {
        cpu.bus.write_8(header + 2, command);
        cpu.bus.write_16(header + 3, 0);
        cpu.bus.write_16(header + 0x0E, 0x0000);
        cpu.bus.write_16(header + 0x10, 0x3100);
        cpu.bus.write_8(buffer, control);
        cpu.es = 0x3000;
        int2f(cpu, 0x1510, 0, 3);
        cpu.bus.read_16(header + 3)
    };

    assert_eq!(request(&mut cpu, 0x03, 0x09), 0x0100); // media changed?
    assert_eq!(cpu.bus.read_8(buffer + 1), 1); // no
    assert_eq!(request(&mut cpu, 0x03, 0x07), 0x0100); // sector size
    assert_eq!(cpu.bus.read_16(buffer + 2), 2048);
    assert_eq!(request(&mut cpu, 0x03, 0x06), 0x0100); // device status
    assert_eq!(request(&mut cpu, 0x0D, 0), 0x0100); // device open
    assert_eq!(request(&mut cpu, 0x80, 0), 0x8103); // read long: unsupported
}
