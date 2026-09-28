//! The ATA hard disk a booted system finds on the primary IDE channel,
//! driven through its ports as a driver does: the signature after a reset,
//! IDENTIFY DEVICE, reads and writes by LBA and by cylinder, head and
//! sector, one sector or a block of them at a time, the interrupts, the
//! address registers counting along, errors, and save states.

use rust_dos::bus::Bus;
use rust_dos::diskimage::Chs;
use rust_dos::disk::{MountOptions, numbered_drive};
use rust_dos::ide::{Ata, Channel, ChannelId, Device};
use std::fs;
use std::path::PathBuf;

const DATA: u16 = 0x1F0;
const ERROR: u16 = 0x1F1;
const COUNT: u16 = 0x1F2;
const SECTOR: u16 = 0x1F3;
const CYL_LOW: u16 = 0x1F4;
const CYL_HIGH: u16 = 0x1F5;
const SELECT: u16 = 0x1F6;
const COMMAND: u16 = 0x1F7;
const CONTROL: u16 = 0x3F6;

const BSY: u8 = 0x80;
const DRQ: u8 = 0x08;
const ERR: u8 = 0x01;

/// 20 cylinders of 4 heads of 17 sectors.
const GEOMETRY: Chs = Chs { cylinders: 20, heads: 4, sectors: 17 };
const SECTORS: usize = 20 * 4 * 17;

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from("target/test_ata").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("c")).unwrap();
    fs::canonicalize(&dir).unwrap()
}

/// Sector `n`'s bytes on the test disk.
fn pattern(n: usize) -> Vec<u8> {
    (0..512).map(|i| (n * 7 + i) as u8 ^ (n >> 8) as u8).collect()
}

/// A machine with a hard disk image, mounted as disk 80h for the BIOS,
/// as the primary channel's master (and `slave`, the same image, as its
/// slave).
fn machine(name: &str, slave: bool) -> (Bus, PathBuf) {
    let dir = scratch(name);
    let image = dir.join("hdd.img");
    fs::write(&image, (0..SECTORS).flat_map(pattern).collect::<Vec<u8>>()).unwrap();
    let mut bus = Bus::new(dir.join("c"));
    let drive = numbered_drive(2);
    let opts = MountOptions { geometry: Some(GEOMETRY), ..Default::default() };
    bus.mount_drive(drive, &image, opts, false).unwrap();
    bus.set_cycles_per_ms(1000);
    let mut channel = Channel::new(ChannelId::Primary);
    channel.devices[0] = Some(Device::Ata(Ata::new(drive, GEOMETRY, SECTORS as u64)));
    if slave {
        channel.devices[1] = Some(Device::Ata(Ata::new(drive, GEOMETRY, SECTORS as u64)));
    }
    bus.ide[0] = Some(channel);
    (bus, image)
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

/// The alternate status, which leaves the interrupt alone.
fn status(bus: &mut Bus) -> u8 {
    bus.io_read(CONTROL)
}

fn wait_ready(bus: &mut Bus) {
    for _ in 0..10_000 {
        if status(bus) & BSY == 0 {
            return;
        }
        wait_ms(bus, 0.05);
    }
    panic!("the disk stays busy");
}

/// IRQ 14's request in the slave PIC.
fn irq14(bus: &mut Bus) -> bool {
    bus.io_write(0xA0, 0x0A);
    bus.io_read(0xA0) & 0x40 != 0
}

/// Set the task file to `count` sectors from LBA `lba` of the master.
fn lba(bus: &mut Bus, lba: u32, count: u8) {
    bus.io_write(SELECT, 0xE0 | (lba >> 24) as u8 & 0x0F);
    bus.io_write(COUNT, count);
    bus.io_write(SECTOR, lba as u8);
    bus.io_write(CYL_LOW, (lba >> 8) as u8);
    bus.io_write(CYL_HIGH, (lba >> 16) as u8);
}

/// Read a data block of `sectors` from the data port.
fn read_block(bus: &mut Bus, sectors: usize) -> Vec<u8> {
    (0..sectors * 256).flat_map(|_| (bus.io_read_wide(DATA, 2) as u16).to_le_bytes()).collect()
}

/// Read `count` sectors with READ SECTORS or (`multiple`) READ MULTIPLE,
/// a block at each interrupt.
fn read(bus: &mut Bus, command: u8, block: usize, count: usize) -> Vec<u8> {
    bus.io_write(COMMAND, command);
    let mut data = Vec::new();
    while data.len() < count * 512 {
        wait_ready(bus);
        assert!(irq14(bus), "an interrupt for each block");
        let st = bus.io_read(COMMAND);
        assert_eq!(st & (DRQ | ERR), DRQ, "status {:02X}", st);
        assert!(!irq14(bus), "reading the status clears it");
        let n = block.min(count - data.len() / 512);
        data.extend(read_block(bus, n));
    }
    wait_ready(bus);
    assert!(!irq14(bus), "no interrupt after the last block");
    assert_eq!(status(bus) & (DRQ | ERR), 0);
    data
}

#[test]
fn a_reset_leaves_the_ata_signature() {
    let (mut bus, _) = machine("signature", false);
    bus.io_write(CONTROL, 0x04);
    assert_eq!(status(&mut bus) & BSY, BSY);
    bus.io_write(CONTROL, 0x00);
    let regs: Vec<u8> = [COUNT, SECTOR, CYL_LOW, CYL_HIGH].iter().map(|&p| bus.io_read(p)).collect();
    assert_eq!(regs, [0x01, 0x01, 0x00, 0x00]);
    assert_eq!(bus.io_read(ERROR), 0x01, "diagnostics passed");
    // Asked as a packet device, it says it's none.
    bus.io_write(COMMAND, 0xA1);
    assert_eq!((status(&mut bus) & ERR, bus.io_read(ERROR)), (ERR, 0x04));
    assert_eq!((bus.io_read(CYL_LOW), bus.io_read(CYL_HIGH)), (0, 0));
    // No slave: its registers read 0.
    bus.io_write(SELECT, 0xB0);
    assert_eq!((bus.io_read(COUNT), bus.io_read(COMMAND), status(&mut bus)), (0, 0, 0));
}

#[test]
fn identify_device_describes_the_disk() {
    let (mut bus, _) = machine("identify", false);
    bus.io_write(SELECT, 0xA0);
    bus.io_write(COMMAND, 0xEC);
    wait_ready(&mut bus);
    assert!(irq14(&mut bus));
    assert_eq!(bus.io_read(COMMAND) & DRQ, DRQ);
    let words: Vec<u16> = (0..256).map(|_| bus.io_read_wide(DATA, 2) as u16).collect();
    assert_eq!((words[0], words[1], words[3], words[6]), (0x0040, 20, 4, 17));
    assert_eq!(words[60] as usize | (words[61] as usize) << 16, SECTORS);
    assert_eq!(words[49] & 0x0200, 0x0200, "LBA");
    let model: String = words[27..47].iter().flat_map(|w| [(w >> 8) as u8 as char, *w as u8 as char]).collect();
    assert_eq!(model.trim(), "Rust-DOS ATA hard disk");
    let sum = words.iter().flat_map(|w| w.to_le_bytes()).fold(0u8, |s, b| s.wrapping_add(b));
    assert_eq!(sum, 0, "the checksum");
    assert_eq!(bus.io_read(COMMAND) & DRQ, 0, "all read");
}

#[test]
fn sectors_read_by_lba_and_chs() {
    let (mut bus, _) = machine("read", false);
    lba(&mut bus, 100, 3);
    let data = read(&mut bus, 0x20, 1, 3);
    assert_eq!(data, (100..103).flat_map(pattern).collect::<Vec<u8>>());
    // The registers point past the last sector read.
    assert_eq!((bus.io_read(COUNT), bus.io_read(SECTOR)), (0, 102));

    // Cylinder 2, head 3, sector 17: LBA (2 * 4 + 3) * 17 + 16, and on
    // to the next cylinder.
    bus.io_write(SELECT, 0xA3);
    bus.io_write(COUNT, 2);
    bus.io_write(SECTOR, 17);
    bus.io_write(CYL_LOW, 2);
    bus.io_write(CYL_HIGH, 0);
    let first = (2 * 4 + 3) * 17 + 16;
    let data = read(&mut bus, 0x20, 1, 2);
    assert_eq!(data, (first..first + 2).flat_map(pattern).collect::<Vec<u8>>());
    assert_eq!((bus.io_read(SECTOR), bus.io_read(SELECT) & 0x0F, bus.io_read(CYL_LOW)), (1, 0, 3));
}

#[test]
fn read_multiple_moves_blocks() {
    let (mut bus, _) = machine("multiple", false);
    bus.io_write(SELECT, 0xA0);
    bus.io_write(COUNT, 4);
    bus.io_write(COMMAND, 0xC6);
    wait_ready(&mut bus);
    assert_eq!(bus.io_read(COMMAND) & ERR, 0);
    lba(&mut bus, 10, 10);
    let data = read(&mut bus, 0xC4, 4, 10);
    assert_eq!(data, (10..20).flat_map(pattern).collect::<Vec<u8>>());
    // 3 is not a block size.
    bus.io_write(COUNT, 3);
    bus.io_write(COMMAND, 0xC6);
    assert_eq!((bus.io_read(COMMAND) & ERR, bus.io_read(ERROR)), (ERR, 0x04));
}

#[test]
fn writes_go_into_the_image() {
    let (mut bus, image) = machine("write", false);
    lba(&mut bus, 500, 2);
    bus.io_write(COMMAND, 0x30);
    assert_eq!(status(&mut bus) & DRQ, DRQ, "the data first");
    assert!(!irq14(&mut bus), "without an interrupt");
    for block in 0..2 {
        for i in 0..256u32 {
            bus.io_write_wide(DATA, 0xA500 + i + block * 0x10, 2);
        }
        wait_ready(&mut bus);
        assert!(irq14(&mut bus), "an interrupt for each sector written");
        bus.io_read(COMMAND);
    }
    assert_eq!(status(&mut bus) & (DRQ | ERR | BSY), 0);
    let bytes = fs::read(&image).unwrap();
    assert_eq!(&bytes[500 * 512..500 * 512 + 4], &[0x00, 0xA5, 0x01, 0xA5]);
    assert_eq!(&bytes[501 * 512..501 * 512 + 2], &[0x10, 0xA5]);
    assert_eq!(&bytes[502 * 512..503 * 512], &pattern(502)[..]);
    // Doublewords are two words.
    lba(&mut bus, 3, 1);
    bus.io_write(COMMAND, 0xC5);
    for i in 0..128u32 {
        bus.io_write_wide(DATA, i * 0x0001_0001, 4);
    }
    wait_ready(&mut bus);
    assert_eq!(status(&mut bus) & (DRQ | ERR), 0);
    lba(&mut bus, 3, 1);
    bus.io_write(COMMAND, 0x20);
    wait_ready(&mut bus);
    let words: Vec<u32> = (0..128).map(|_| bus.io_read_wide(DATA, 4)).collect();
    assert_eq!(words[5], 0x0005_0005);
}

#[test]
fn sectors_that_are_not_there() {
    let (mut bus, _) = machine("idnf", false);
    lba(&mut bus, SECTORS as u32, 1);
    bus.io_write(COMMAND, 0x20);
    wait_ready(&mut bus);
    assert_eq!((bus.io_read(COMMAND) & ERR, bus.io_read(ERROR)), (ERR, 0x10));
    // Sector 0 of a cylinder.
    bus.io_write(SELECT, 0xA0);
    bus.io_write(SECTOR, 0);
    bus.io_write(COMMAND, 0x20);
    wait_ready(&mut bus);
    assert_eq!((bus.io_read(COMMAND) & ERR, bus.io_read(ERROR)), (ERR, 0x10));
    // An unknown command is aborted.
    bus.io_write(COMMAND, 0x50);
    assert_eq!((bus.io_read(COMMAND) & ERR, bus.io_read(ERROR)), (ERR, 0x04));
}

#[test]
fn a_new_geometry_for_chs() {
    let (mut bus, _) = machine("geometry", false);
    // 16 heads of 63 sectors.
    bus.io_write(SELECT, 0xAF);
    bus.io_write(COUNT, 63);
    bus.io_write(COMMAND, 0x91);
    assert!(irq14(&mut bus));
    assert_eq!(bus.io_read(COMMAND) & ERR, 0);
    // Cylinder 0, head 1, sector 1 is LBA 63.
    bus.io_write(SELECT, 0xA1);
    bus.io_write(COUNT, 1);
    bus.io_write(SECTOR, 1);
    bus.io_write(CYL_LOW, 0);
    assert_eq!(read(&mut bus, 0x20, 1, 1), pattern(63));
}

#[test]
fn master_and_slave_answer_by_turns() {
    let (mut bus, _) = machine("slave", true);
    bus.io_write(SELECT, 0xF0);
    bus.io_write(COUNT, 1);
    bus.io_write(SECTOR, 42);
    bus.io_write(CYL_LOW, 0);
    bus.io_write(CYL_HIGH, 0);
    assert_eq!(read(&mut bus, 0x20, 1, 1), pattern(42));
    // The master's registers are its own.
    bus.io_write(SELECT, 0xE0);
    assert_eq!(bus.io_read(SECTOR), 0x01);
    // nIEN keeps the interrupt off the line.
    bus.io_write(CONTROL, 0x02);
    bus.io_write(COMMAND, 0x08);
    assert!(!irq14(&mut bus));
    bus.io_write(CONTROL, 0x00);
    assert!(irq14(&mut bus));
}

#[test]
fn a_state_saved_mid_transfer_goes_on() {
    let (bus, _) = machine("state", false);
    let mut cpu = rust_dos::cpu::Cpu::new(PathBuf::from("."));
    cpu.bus = bus;
    lba(&mut cpu.bus, 200, 2);
    cpu.bus.io_write(COMMAND, 0x20);
    wait_ready(&mut cpu.bus);
    let first: Vec<u8> = (0..128).flat_map(|_| (cpu.bus.io_read_wide(DATA, 2) as u16).to_le_bytes()).collect();
    let state = rust_dos::savestate::machine::save(&cpu);
    let finish = |bus: &mut Bus| -> Vec<u8> {
        let mut data: Vec<u8> = (0..128).flat_map(|_| (bus.io_read_wide(DATA, 2) as u16).to_le_bytes()).collect();
        wait_ready(bus);
        bus.io_read(COMMAND);
        data.extend(read_block(bus, 1));
        data
    };
    let rest = finish(&mut cpu.bus);
    rust_dos::savestate::machine::load(&mut cpu, &state).unwrap();
    assert!(cpu.bus.ide[0].is_some());
    let again = finish(&mut cpu.bus);
    assert_eq!(rest, again);
    let all: Vec<u8> = first.into_iter().chain(rest).collect();
    assert_eq!(all, (200..202).flat_map(pattern).collect::<Vec<u8>>());
}

/// Windows for Workgroups' WDCTRL calls INT 13h and reads the task file
/// back: the BIOS leaves it pointing at the sector it read, and resets
/// the disk for AH=00h.
#[test]
fn int13_leaves_the_task_file_as_a_bios_does() {
    use iced_x86::Register;
    let dir = scratch("int13");
    let image = dir.join("hdd.img");
    fs::write(&image, (0..SECTORS).flat_map(pattern).collect::<Vec<u8>>()).unwrap();
    let mut cpu = rust_dos::cpu::Cpu::new(dir.join("c"));
    let opts = MountOptions { geometry: Some(GEOMETRY), ..Default::default() };
    cpu.bus.mount_drive(numbered_drive(2), &image, opts, false).unwrap();
    cpu.bus.boot = Some(Default::default());
    cpu.bus.attach_ide();
    // Cylinder 1, head 2, sector 5: 2 sectors to 5000:0000.
    cpu.set_es(0x5000);
    cpu.set_bx(0);
    cpu.set_ax(0x0202);
    cpu.set_cx(0x0105);
    cpu.set_dx(0x0280);
    rust_dos::interrupts::int13::handle(&mut cpu);
    assert!(!cpu.get_cpu_flag(rust_dos::cpu::CpuFlags::CF));
    let first = (4 + 2) * 17 + 4;
    assert_eq!(cpu.bus.read_8(0x50000), pattern(first)[0]);
    let bus = &mut cpu.bus;
    // The second sector's address, by the disk's own geometry.
    let regs: Vec<u8> = [ERROR, COUNT, SECTOR, CYL_LOW, CYL_HIGH, SELECT].iter().map(|&p| bus.io_read(p)).collect();
    assert_eq!(regs, [0, 0, 6, 1, 0, 0xA2]);
    assert_eq!(bus.io_read(COMMAND), 0x50);
    // AH=00h: DEVICE RESET, its interrupt taken.
    cpu.set_reg8(Register::AH, 0x00);
    rust_dos::interrupts::int13::handle(&mut cpu);
    assert_eq!((cpu.bus.io_read(COUNT), cpu.bus.io_read(SECTOR)), (1, 1));
    assert!(!irq14(&mut cpu.bus));
}
