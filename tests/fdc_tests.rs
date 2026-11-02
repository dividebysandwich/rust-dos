//! The floppy disk controller a booted system finds at 3F0h, driven
//! through its ports as a driver does: the disk INT 13h has, read and
//! written by DMA with IRQ 6, the disk change line after another disk
//! goes in, save states, and no controller for the built-in DOS or with
//! `floppy_controller` off.

use rust_dos::bus::Bus;
use rust_dos::cpu::Cpu;
use rust_dos::disk::MountOptions;
use rust_dos::diskimage::{DiskImage, MemoryImage, SECTOR_SIZE};
use rust_dos::exec;
use std::path::PathBuf;

const DOR: u16 = 0x3F2;
const MSR: u16 = 0x3F4;
const DATA: u16 = 0x3F5;
const DIR: u16 = 0x3F7;

/// A 1.44 MB FAT12 floppy whose boot sector loops, with the data sectors
/// starting with their own number and then `fill`.
fn floppy(fill: u8) -> DiskImage {
    let mut img = vec![0u8; 1_474_560];
    img[0..3].copy_from_slice(&[0xEB, 0x3C, 0x90]);
    img[3..11].copy_from_slice(b"RUSTDOS ");
    // 512 bytes a sector, 1 a cluster, 1 reserved, 2 FATs, 224 root
    // entries, 2880 sectors, F0h, 9 sectors a FAT, 18 a track, 2 heads.
    let bpb: [u8; 19] = [
        0x00, 0x02, 1, 1, 0, 2, 0xE0, 0, 0x40, 0x0B, 0xF0, 9, 0, 18, 0, 2, 0, 0, 0,
    ];
    img[11..30].copy_from_slice(&bpb);
    img[0x3E..0x40].copy_from_slice(&[0xEB, 0xFE]);
    img[510..512].copy_from_slice(&[0x55, 0xAA]);
    for fat in [1, 10] {
        img[fat * SECTOR_SIZE..][..3].copy_from_slice(&[0xF0, 0xFF, 0xFF]);
    }
    for lba in 33..2880 {
        let sector = &mut img[lba * SECTOR_SIZE..(lba + 1) * SECTOR_SIZE];
        sector.fill(fill);
        sector[..2].copy_from_slice(&(lba as u16).to_le_bytes());
    }
    DiskImage::from_memory("a.img", MemoryImage::from(img), true, None, false).unwrap()
}

/// A machine booted from the floppy in A:.
fn booted(name: &str, controller: bool) -> Cpu {
    let base = PathBuf::from("target/test_fdc").join(name);
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let mut cpu = Cpu::new(base);
    cpu.bus.set_cycles_per_ms(1000);
    cpu.bus.floppy_controller = controller;
    cpu.load_shell();
    cpu.bus
        .mount_disk_image(0, floppy(0x11), MountOptions::default())
        .unwrap();
    exec::run_command_line(&mut cpu, "BOOT -l A");
    assert!(cpu.bus.boot.is_some(), "booted");
    cpu
}

/// Let `ms` milliseconds of emulated time pass, running due timer events.
fn wait_ms(bus: &mut Bus, ms: f64) {
    let target = bus.clock.icount + (ms * 1000.0) as u64;
    while bus.clock.icount < target {
        let step = (target - bus.clock.icount).min(10);
        bus.clock.icount += step;
        if bus.clock.icount >= bus.clock.deadline {
            bus.service_timers();
        }
    }
}

/// IRQ 6's request in the master PIC.
fn irq6(bus: &mut Bus) -> bool {
    bus.io_write(0x20, 0x0A);
    bus.io_read(0x20) & 0x40 != 0
}

/// A command's bytes, the time it takes, then its result.
fn command(bus: &mut Bus, bytes: &[u8]) -> Vec<u8> {
    for &b in bytes {
        assert_eq!(bus.io_read(MSR) & 0xC0, 0x80, "ready for byte {:02X}h", b);
        bus.io_write(DATA, b);
    }
    // Until the result, or the controller is idle again.
    for _ in 0..1000 {
        let msr = bus.io_read(MSR);
        if msr & 0x10 == 0 || msr & 0xC0 == 0xC0 {
            break;
        }
        wait_ms(bus, 1.0);
    }
    let mut result = Vec::new();
    while bus.io_read(MSR) & 0xC0 == 0xC0 {
        result.push(bus.io_read(DATA));
    }
    result
}

/// DMA channel 2 set for `bytes` bytes at 20000h, into memory (`write`)
/// or out of it.
fn dma(bus: &mut Bus, bytes: usize, write: bool) {
    let count = (bytes - 1) as u16;
    for (port, value) in [
        (0x0A, 0x06),
        (0x0B, if write { 0x46 } else { 0x4A }),
        (0x0C, 0),
        (0x04, 0x00),
        (0x04, 0x00),
        (0x81, 0x02),
        (0x05, count as u8),
        (0x05, (count >> 8) as u8),
        (0x0A, 0x02),
    ] {
        bus.io_write(port, value);
    }
}

/// Reset the controller, read the drives' statuses, turn A:'s motor on,
/// recalibrate and seek to `cylinder`.
fn ready(bus: &mut Bus, cylinder: u8) {
    bus.io_write(DOR, 0x08);
    bus.io_write(DOR, 0x1C);
    assert!(irq6(bus), "the reset's interrupt");
    for d in 0..4 {
        assert_eq!(command(bus, &[0x08]), [0xC0 | d, 0]);
    }
    assert!(!irq6(bus));
    assert_eq!(command(bus, &[0x07, 0x00]), Vec::<u8>::new());
    wait_ms(bus, 500.0);
    assert_eq!(command(bus, &[0x08]), [0x20, 0]);
    assert_eq!(command(bus, &[0x0F, 0x00, cylinder]), Vec::<u8>::new());
    wait_ms(bus, 500.0);
    assert!(irq6(bus), "the seek's end");
    assert_eq!(command(bus, &[0x08]), [0x20, cylinder]);
}

#[test]
fn the_controller_reads_and_writes_the_disk_int13h_has() {
    let mut cpu = booted("io", true);
    let bus = &mut cpu.bus;
    assert!(bus.fdc.is_some());
    ready(bus, 2);
    // Cylinder 2, head 1, sectors 1 and 2: sectors 90 and 91.
    dma(bus, 2 * SECTOR_SIZE, true);
    assert_eq!(
        command(bus, &[0xE6, 0x04, 2, 1, 1, 2, 18, 0x1B, 0xFF]),
        [0x04, 0, 0, 2, 1, 3, 2]
    );
    assert!(!irq6(bus), "reading the result acknowledged it");
    let image = bus.disk.bios_image(0).unwrap();
    let mut expected = vec![0u8; 2 * SECTOR_SIZE];
    image.read(90, &mut expected).unwrap();
    let ram: Vec<u8> = (0..2 * SECTOR_SIZE)
        .map(|i| bus.read_8(0x20000 + i))
        .collect();
    assert_eq!(ram, expected);
    // A sector written: INT 13h's image has it.
    for i in 0..SECTOR_SIZE {
        bus.write_8(0x20000 + i, 0x5A);
    }
    dma(bus, SECTOR_SIZE, false);
    assert_eq!(
        command(bus, &[0xC5, 0x00, 2, 0, 5, 2, 18, 0x1B, 0xFF]),
        [0x00, 0, 0, 2, 0, 6, 2]
    );
    let mut sector = [0u8; SECTOR_SIZE];
    image.read(76, &mut sector).unwrap();
    assert_eq!(sector, [0x5A; SECTOR_SIZE]);
}

#[test]
fn another_disk_sets_the_disk_change_line() {
    let mut cpu = booted("change", true);
    let bus = &mut cpu.bus;
    assert_eq!(bus.io_read(DIR) & 0x80, 0x80, "a disk at power-on");
    ready(bus, 1);
    assert_eq!(bus.io_read(DIR) & 0x80, 0, "the step cleared it");
    bus.mount_disk_image(0, floppy(0x22), MountOptions::default())
        .unwrap();
    assert_eq!(bus.io_read(DIR) & 0x80, 0x80);
    // The new disk is the one the controller reads.
    ready(bus, 3);
    dma(bus, SECTOR_SIZE, true);
    assert_eq!(
        command(bus, &[0x46, 0x00, 3, 0, 1, 2, 18, 0x1B, 0xFF])[..3],
        [0, 0, 0]
    );
    assert_eq!(bus.read_8(0x20000 + 2), 0x22);
}

#[test]
fn a_state_keeps_the_controller() {
    let mut cpu = booted("state", true);
    ready(&mut cpu.bus, 4);
    // A seek of 20 steps under way.
    command(&mut cpu.bus, &[0x03, 0xDF, 0x02]);
    cpu.bus.io_write(DATA, 0x0F);
    cpu.bus.io_write(DATA, 0x00);
    cpu.bus.io_write(DATA, 24);
    let state = rust_dos::savestate::machine::save(&cpu);
    wait_ms(&mut cpu.bus, 500.0);
    assert_eq!(command(&mut cpu.bus, &[0x08]), [0x20, 24]);
    rust_dos::savestate::machine::load(&mut cpu, &state).unwrap();
    assert_eq!(cpu.bus.io_read(MSR), 0x81, "drive 0 seeking");
    wait_ms(&mut cpu.bus, 500.0);
    assert_eq!(command(&mut cpu.bus, &[0x08]), [0x20, 24]);
}

#[test]
fn only_a_booted_system_has_the_controller() {
    let mut cpu = booted("off", false);
    assert!(cpu.bus.fdc.is_none());
    assert_eq!(cpu.bus.io_read(MSR), 0xFF);
    let mut cpu = booted("dos", true);
    rust_dos::boot::reboot_dos(&mut cpu);
    assert!(cpu.bus.fdc.is_none());
}

#[test]
fn the_built_in_dos_takes_controller_port_writes_quietly() {
    // With no controller, writes to its ports go nowhere and must not log
    // as unhandled. 3F6h is the IDE channel's, not the controller's, so it
    // still logs: that line shows the log is kept at all.
    let base = PathBuf::from("target/test_fdc/quiet");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let mut cpu = Cpu::new(base);
    cpu.bus.log_backlog = Some(Vec::new());
    assert!(cpu.bus.fdc.is_none());
    for port in [0x3F0, 0x3F1, DOR, 0x3F3, MSR, DATA, DIR] {
        cpu.bus.io_write(port, 0x0C);
    }
    cpu.bus.io_write(0x3F6, 0x0C);
    let lines: Vec<String> = cpu
        .bus
        .take_log_backlog()
        .into_iter()
        .map(|(_, line)| line)
        .collect();
    for port in [0x3F0, 0x3F1, DOR, 0x3F3, MSR, DATA, DIR] {
        let tag = format!("Port: {port:04X}");
        assert!(lines.iter().all(|line| !line.contains(&tag)), "{lines:?}");
    }
    assert!(
        lines.iter().any(|line| line.contains("Port: 03F6")),
        "{lines:?}"
    );
}
