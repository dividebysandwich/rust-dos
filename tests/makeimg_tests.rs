//! MAKEIMG at the prompt: it asks, makes the image DOSBox Staging would,
//! which IMGMOUNT mounts, and refuses what it can't do; and the images'
//! boot code runs on the machine's BIOS.

use iced_x86::Register;
use rust_dos::cpu::Cpu;
use rust_dos::disk::{DriveKind, MountOptions};
use rust_dos::exec::{self, NoHook};
use rust_dos::makeimg::{self, ImageSpec};
use std::fs;
use std::path::{Path, PathBuf};

fn scratch(name: &str) -> PathBuf {
    let base = PathBuf::from("target/test_makeimg").join(name);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    base.canonicalize().unwrap()
}

fn machine(dir: &Path) -> Cpu {
    let mut cpu = Cpu::new(dir.to_path_buf());
    cpu.bus.set_cycles_per_ms(1000);
    cpu.load_shell();
    cpu
}

/// Run the machine for `ms` ms of emulated time.
fn run(cpu: &mut Cpu, ms: u64) {
    for _ in 0..ms {
        let end = cpu.bus.clock.icount + 1000;
        cpu.bus.start_batch(end);
        exec::run_batch(cpu, &mut NoHook, false);
    }
}

/// Run `line` at the prompt.
fn command(cpu: &mut Cpu, line: &str) {
    cpu.queue_batch_lines([format!("@{}", line)]);
    run(cpu, 200);
}

fn screen(cpu: &Cpu) -> String {
    cpu.bus
        .vga
        .vram_text
        .chunks(160)
        .take(25)
        .map(|row| row.iter().step_by(2).map(|&b| if b == 0 { ' ' } else { b as char }).collect::<String>().trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn makeimg_asks_then_makes_an_image_imgmount_mounts() {
    let dir = scratch("ask");
    let mut cpu = machine(&dir);
    command(&mut cpu, "MAKEIMG C:\\FLOPPY.IMG -t fd_1440kb -label TestDisk -d");
    let text = screen(&cpu);
    assert!(text.contains("Image will be created on the DOS filesystem at:\n  C:\\FLOPPY.IMG"), "{}", text);
    assert!(text.replace('\n', "").contains(&format!("Host path: {}", dir.join("FLOPPY.IMG").display())), "{}", text);
    assert!(text.contains("Proceed? (Y/N)"), "{}", text);
    assert!(!dir.join("FLOPPY.IMG").exists());

    // Another key: it waits on.
    cpu.bus.keyboard_buffer.push_back(0x2D78); // x
    run(&mut cpu, 100);
    assert!(cpu.shell_wait.is_some());
    cpu.bus.keyboard_buffer.push_back(0x1579); // y
    run(&mut cpu, 200);
    let text = screen(&cpu);
    assert!(text.contains("Y\nCreated C:\\FLOPPY.IMG [CHS: 80, 2, 18]\nFormatted as FAT12"), "{}", text);
    assert_eq!(fs::metadata(dir.join("FLOPPY.IMG")).unwrap().len(), 1_474_560);

    command(&mut cpu, "IMGMOUNT A C:\\FLOPPY.IMG");
    command(&mut cpu, "DIR A:");
    let text = screen(&cpu);
    assert!(text.contains("TESTDISK"), "the label: {}", text);

    // Not over a mounted image, nor another file without -force.
    command(&mut cpu, "MAKEIMG C:\\FLOPPY.IMG -t fd_720kb -d -force");
    assert!(screen(&cpu).contains("is mounted as drive A: (IMGMOUNT -u A unmounts it)"), "{}", screen(&cpu));
    command(&mut cpu, "IMGMOUNT -u A");
    command(&mut cpu, "MAKEIMG C:\\FLOPPY.IMG -t fd_720kb -d");
    assert!(screen(&cpu).contains("already exists. Use -force to overwrite."), "{}", screen(&cpu));
}

#[test]
fn makeimg_leaves_it_at_no() {
    let dir = scratch("no");
    let mut cpu = machine(&dir);
    command(&mut cpu, "MAKEIMG HDD.IMG -t hd -size 10 -d");
    cpu.bus.keyboard_buffer.push_back(0x316E); // n
    run(&mut cpu, 200);
    assert!(screen(&cpu).contains("N\n\nOperation aborted."), "{}", screen(&cpu));
    assert!(!dir.join("HDD.IMG").exists());
    assert!(cpu.shell_wait.is_none());
}

#[test]
fn makeimg_explains_what_it_cant_do() {
    let dir = scratch("refuse");
    fs::create_dir_all(dir.join("ro")).unwrap();
    let image = dir.join("disk.img");
    makeimg::write(&image, &makeimg::plan(&ImageSpec { preset: makeimg::preset("fd_720kb"), ..Default::default() }).unwrap(), false).unwrap();
    let mut cpu = machine(&dir);
    let floppy = MountOptions { kind: DriveKind::Floppy, ..Default::default() };
    cpu.bus.mount_drive(0, &image, floppy, false).unwrap();
    let read_only = MountOptions { read_only: true, ..Default::default() };
    cpu.bus.mount_drive(3, &dir.join("ro"), read_only, false).unwrap();

    for (line, says) in [
        ("MAKEIMG", "Create a new empty disk image."),
        ("MAKEIMG A:\\NEW.IMG -t fd_360kb -d", "Cannot create image inside another disk image."),
        ("MAKEIMG D:\\NEW.IMG -t fd_360kb -d", "Target drive is read-only."),
        ("MAKEIMG Q:\\NEW.IMG -t fd_360kb -d", "Target drive is invalid."),
        ("MAKEIMG NEW.IMG -t hd", "needs -size or -chs"),
        ("MAKEIMG NEW.IMG -t hd_20mb -fat 32", "too small for FAT32"),
        ("MAKEIMG NEW.IMG -t zip_100mb", "Unknown disk type: zip_100mb"),
    ] {
        command(&mut cpu, "CLS");
        command(&mut cpu, line);
        assert!(screen(&cpu).contains(says), "{}: {}", line, screen(&cpu));
        assert!(cpu.shell_wait.is_none(), "{} doesn't ask", line);
    }
}

/// Run the boot sector at 0000:7C00 of `drive`'s BIOS unit `dl` as the
/// BIOS would, and return the screen after a while.
fn boot(cpu: &mut Cpu, sector: &[u8], dl: u8) -> String {
    for (i, &b) in sector.iter().enumerate() {
        cpu.bus.write_8(0x7C00 + i, b);
    }
    cpu.set_cs(0);
    cpu.set_ip(0x7C00);
    cpu.set_ss(0);
    cpu.set_sp(0x7C00);
    cpu.set_reg8(Register::DL, dl);
    run(cpu, 300);
    screen(cpu)
}

#[test]
fn new_disks_boot_to_their_non_system_message() {
    let dir = scratch("boot");
    let hdd = dir.join("hdd.img");
    let plan = makeimg::plan(&ImageSpec { preset: makeimg::preset("hd_40mb"), ..Default::default() }).unwrap();
    makeimg::write(&hdd, &plan, false).unwrap();
    let mut cpu = machine(&dir);
    let hard_disk = MountOptions { kind: DriveKind::HardDisk, ..Default::default() };
    cpu.bus.mount_drive(2, &hdd, hard_disk, true).unwrap();
    // The partition table's code loads the partition's boot sector, whose
    // code says it has no system.
    let mbr = fs::read(&hdd).unwrap()[..512].to_vec();
    let text = boot(&mut cpu, &mbr, 0x80);
    assert!(text.contains("Non-system disk or disk error\nReplace and press any key when ready"), "{}", text);

    let floppy = dir.join("floppy.img");
    let plan = makeimg::plan(&ImageSpec { preset: makeimg::preset("fd_1200kb"), ..Default::default() }).unwrap();
    makeimg::write(&floppy, &plan, false).unwrap();
    let mut cpu = machine(&dir);
    cpu.bus.mount_drive(0, &floppy, MountOptions { kind: DriveKind::Floppy, ..Default::default() }, false).unwrap();
    let boot_sector = fs::read(&floppy).unwrap()[..512].to_vec();
    let text = boot(&mut cpu, &boot_sector, 0x00);
    assert!(text.contains("Non-system disk or disk error"), "{}", text);

    // Without an active partition, the partition table's code says so.
    let mut mbr = mbr;
    mbr[0x1BE] = 0;
    let mut cpu = machine(&dir);
    let text = boot(&mut cpu, &mbr, 0x80);
    assert!(text.contains("No active partition"), "{}", text);
}

