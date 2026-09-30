//! The ATAPI CD-ROM drive a booted system finds on the secondary IDE
//! channel, driven through its ports as a driver does: the signature after
//! a reset, IDENTIFY PACKET DEVICE, packet commands with their data blocks
//! and interrupts, reads, the table of contents, CD audio, a new disc, and
//! save states.

mod cdimage;

use cdimage::{file, iso, mixed_disc};
use rust_dos::bus::Bus;
use rust_dos::disk::MountOptions;
use std::fs;

const DATA: u16 = 0x170;
const ERROR: u16 = 0x171;
const COUNT: u16 = 0x172;
const LBA_LOW: u16 = 0x173;
const LBA_MID: u16 = 0x174;
const LBA_HIGH: u16 = 0x175;
const SELECT: u16 = 0x176;
const COMMAND: u16 = 0x177;
const CONTROL: u16 = 0x376;

const BSY: u8 = 0x80;
const DRQ: u8 = 0x08;
const ERR: u8 = 0x01;

/// A machine with a disc of a data track and a 10-sector audio track in
/// D:, as a booted system has it on the IDE channel.
fn drive(name: &str) -> (Bus, Vec<u8>) {
    let dir = cdimage::scratch(&format!("ide_{}", name));
    fs::create_dir_all(dir.join("c")).unwrap();
    let mut bus = Bus::new(dir.join("c"));
    let image = iso("TESTDISC", &[file("HELLO.TXT", b"hello"), file("DATA\\BIG.DAT", &[7; 9000])]);
    let cue = mixed_disc(&dir, &image, 10);
    bus.mount_drive(3, &cue, MountOptions::default(), false).unwrap();
    bus.set_cycles_per_ms(1000);
    bus.attach_ide();
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
    for _ in 0..40_000 {
        if status(bus) & BSY == 0 {
            return;
        }
        wait_ms(bus, 0.1);
    }
    panic!("the drive stays busy");
}

/// IRQ 15's request in the slave PIC.
fn irq15(bus: &mut Bus) -> bool {
    bus.io_write(0xA0, 0x0A);
    bus.io_read(0xA0) & 0x80 != 0
}

/// A packet command, the host taking up to `limit` bytes a data block.
/// The data, or the error register.
fn packet(bus: &mut Bus, cdb: [u8; 12], limit: u16) -> Result<Vec<u8>, u8> {
    bus.io_write(SELECT, 0xA0);
    bus.io_write(ERROR, 0);
    bus.io_write(LBA_MID, limit as u8);
    bus.io_write(LBA_HIGH, (limit >> 8) as u8);
    bus.io_write(COMMAND, 0xA0);
    wait_ready(bus);
    assert_eq!((status(bus) & DRQ, bus.io_read(COUNT)), (DRQ, 1), "ready for the packet");
    for pair in cdb.chunks(2) {
        bus.io_write_wide(DATA, u16::from_le_bytes([pair[0], pair[1]]) as u32, 2);
    }
    let mut data = Vec::new();
    loop {
        wait_ready(bus);
        let st = bus.io_read(COMMAND);
        if st & ERR != 0 {
            return Err(bus.io_read(ERROR));
        }
        if st & DRQ == 0 {
            return Ok(data);
        }
        let n = bus.io_read(LBA_MID) as usize | (bus.io_read(LBA_HIGH) as usize) << 8;
        assert!(n <= limit as usize, "a block of {} bytes over the limit {}", n, limit);
        for _ in 0..n.div_ceil(2) {
            data.extend((bus.io_read_wide(DATA, 2) as u16).to_le_bytes());
        }
        if n % 2 == 1 {
            data.pop();
        }
    }
}

fn sense(bus: &mut Bus) -> (u8, u8, u8) {
    let s = packet(bus, [0x03, 0, 0, 0, 18, 0, 0, 0, 0, 0, 0, 0], 18).unwrap();
    (s[2] & 0xF, s[12], s[13])
}

fn read10(lba: u32, count: u16) -> [u8; 12] {
    let l = lba.to_be_bytes();
    let c = count.to_be_bytes();
    [0x28, 0, l[0], l[1], l[2], l[3], 0, c[0], c[1], 0, 0, 0]
}

#[test]
fn a_reset_leaves_the_atapi_signature() {
    let (mut bus, _) = drive("signature");
    bus.io_write(CONTROL, 0x04);
    assert_eq!(status(&mut bus) & BSY, BSY);
    bus.io_write(CONTROL, 0x00);
    let regs: Vec<u8> = [COUNT, LBA_LOW, LBA_MID, LBA_HIGH].iter().map(|&p| bus.io_read(p)).collect();
    assert_eq!(regs, [0x01, 0x01, 0x14, 0xEB]);
    assert_eq!(bus.io_read(ERROR), 0x01, "diagnostics passed");
    // IDENTIFY DEVICE is aborted with the signature: how Windows 95 tells
    // a packet device.
    bus.io_write(COMMAND, 0xEC);
    assert_eq!(status(&mut bus) & ERR, ERR);
    assert_eq!(bus.io_read(ERROR), 0x04);
    assert_eq!((bus.io_read(LBA_MID), bus.io_read(LBA_HIGH)), (0x14, 0xEB));
    // No slave: its registers read 0.
    bus.io_write(SELECT, 0xB0);
    assert_eq!((bus.io_read(COUNT), bus.io_read(COMMAND)), (0, 0));
}

#[test]
fn identify_packet_device_describes_a_cd_rom() {
    let (mut bus, _) = drive("identify");
    bus.io_write(SELECT, 0xA0);
    bus.io_write(COMMAND, 0xA1);
    wait_ready(&mut bus);
    assert_eq!(bus.io_read(COMMAND) & DRQ, DRQ);
    let words: Vec<u16> = (0..256).map(|_| bus.io_read_wide(DATA, 2) as u16).collect();
    assert_eq!(words[0], 0x85C0, "removable ATAPI CD-ROM, 12-byte packets");
    let model: String = words[27..47].iter().flat_map(|w| [(w >> 8) as u8 as char, *w as u8 as char]).collect();
    assert_eq!(model.trim(), "Rust-DOS ATAPI CD-ROM");
    assert_eq!(bus.io_read(COMMAND) & DRQ, 0, "all read");
}

#[test]
fn packet_commands_interrupt_on_irq_15() {
    let (mut bus, _) = drive("irq");
    let inquiry = packet(&mut bus, [0x12, 0, 0, 0, 36, 0, 0, 0, 0, 0, 0, 0], 36).unwrap();
    assert_eq!(inquiry[0], 0x05, "a CD-ROM");
    assert_eq!(&inquiry[8..16], b"RUST-DOS");
    // The data block raises IRQ 15; reading the status withdraws it.
    bus.io_write(SELECT, 0xA0);
    bus.io_write(LBA_MID, 36);
    bus.io_write(LBA_HIGH, 0);
    bus.io_write(COMMAND, 0xA0);
    wait_ready(&mut bus);
    assert!(!irq15(&mut bus), "no interrupt for the packet request");
    for w in [0x0012u16, 0, 36, 0, 0, 0] {
        bus.io_write_wide(DATA, w as u32, 2);
    }
    wait_ready(&mut bus);
    assert!(irq15(&mut bus));
    bus.io_read(COMMAND);
    assert!(!irq15(&mut bus), "a status read acknowledges it");
    // With nIEN, none.
    for _ in 0..18 {
        bus.io_read_wide(DATA, 2);
    }
    bus.io_read(COMMAND);
    bus.io_write(CONTROL, 0x02);
    let _ = packet(&mut bus, [0x12, 0, 0, 0, 36, 0, 0, 0, 0, 0, 0, 0], 36);
    assert!(!irq15(&mut bus));
}

#[test]
fn reads_come_in_blocks_the_host_allows() {
    let (mut bus, image) = drive("read");
    // The volume descriptor at sector 16.
    let pvd = packet(&mut bus, read10(16, 1), 2048).unwrap();
    assert_eq!(&pvd[1..6], b"CD001");
    // Four sectors in blocks of one sector each.
    let data = packet(&mut bus, read10(16, 4), 2048).unwrap();
    assert_eq!(data, &image[16 * 2048..20 * 2048]);
    // Or at once.
    let data = packet(&mut bus, read10(16, 4), 0xFFFE).unwrap();
    assert_eq!(data.len(), 4 * 2048);
    // Reading nothing is fine.
    assert_eq!(packet(&mut bus, read10(16, 0), 2048).unwrap(), Vec::<u8>::new());
}

#[test]
fn the_table_of_contents_and_capacity() {
    let (mut bus, image) = drive("toc");
    let data_sectors = (image.len() / 2048) as u32;
    let toc = packet(&mut bus, [0x43, 0, 0, 0, 0, 0, 1, 0x03, 0x24, 0, 0, 0], 0x324).unwrap();
    assert_eq!((toc[2], toc[3]), (1, 2), "tracks 1 to 2");
    // Track 1: data, at LBA 0; track 2: audio, after its pregap.
    assert_eq!(&toc[4..12], &[0, 0x14, 1, 0, 0, 0, 0, 0]);
    assert_eq!(&toc[12..16], &[0, 0x10, 2, 0]);
    assert_eq!(u32::from_be_bytes(toc[16..20].try_into().unwrap()), data_sectors + 150);
    // The lead-out.
    assert_eq!(toc[21], 0x14);
    assert_eq!(toc[22], 0xAA);
    let leadout = u32::from_be_bytes(toc[24..28].try_into().unwrap());
    assert_eq!(leadout, data_sectors + 160);
    // In MSF: track 1 at 00:02:00.
    let toc = packet(&mut bus, [0x43, 2, 0, 0, 0, 0, 1, 0x03, 0x24, 0, 0, 0], 0x324).unwrap();
    assert_eq!(&toc[8..12], &[0, 0, 2, 0]);
    // Cut to the allocation length: the header and one entry.
    let toc = packet(&mut bus, [0x43, 0, 0, 0, 0, 0, 1, 0, 12, 0, 0, 0], 0x324).unwrap();
    assert_eq!(toc.len(), 12);
    // READ CAPACITY: the last sector, and 2048 bytes a sector.
    let cap = packet(&mut bus, [0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], 8).unwrap();
    assert_eq!(u32::from_be_bytes(cap[0..4].try_into().unwrap()), leadout - 1);
    assert_eq!(u32::from_be_bytes(cap[4..8].try_into().unwrap()), 2048);
}

#[test]
fn cd_audio_plays_pauses_and_stops() {
    let (mut bus, image) = drive("audio");
    let track2 = (image.len() / 2048) as u32 + 150;
    let sub = |bus: &mut Bus| packet(bus, [0x42, 0, 0x40, 1, 0, 0, 0, 0, 16, 0, 0, 0], 16).unwrap();
    // PLAY AUDIO MSF of track 2: from its start to the lead-out.
    let (m, s, f) = rust_dos::cdrom::lba_to_msf(track2);
    let (em, es, ef) = rust_dos::cdrom::lba_to_msf(track2 + 10);
    packet(&mut bus, [0x47, 0, 0, m, s, f, em, es, ef, 0, 0, 0], 0).unwrap();
    let q = sub(&mut bus);
    assert_eq!(q[1], 0x11, "playing");
    assert_eq!((q[5], q[6]), (0x10, 2), "audio, track 2");
    let before = u32::from_be_bytes(q[8..12].try_into().unwrap());
    wait_ms(&mut bus, 60.0);
    bus.audio_catch_up();
    let after = u32::from_be_bytes(sub(&mut bus)[8..12].try_into().unwrap());
    assert!(after > before, "the position goes on ({} to {})", before, after);
    // PAUSE, RESUME.
    packet(&mut bus, [0x4B, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], 0).unwrap();
    assert_eq!(sub(&mut bus)[1], 0x12);
    packet(&mut bus, [0x4B, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0], 0).unwrap();
    assert_eq!(sub(&mut bus)[1], 0x11);
    // SEEK stops it, as Windows 95's CD Player expects.
    packet(&mut bus, [0x2B, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], 0).unwrap();
    assert_eq!(sub(&mut bus)[1], 0x13);
}

#[test]
fn a_new_disc_goes_in_spins_up_and_says_it_changed() {
    let (mut bus, _) = drive("change");
    let tur = |bus: &mut Bus| packet(bus, [0; 12], 0);
    // Spun up by a read.
    packet(&mut bus, read10(16, 1), 2048).unwrap();
    assert_eq!(tur(&mut bus), Ok(vec![]));
    // The same disc again, as IMGMOUNT or Ctrl+F4 would put another.
    let dir = cdimage::scratch("ide_change_2");
    let cue = mixed_disc(&dir, &iso("SECOND", &[file("B.TXT", b"b")]), 5);
    bus.unmount_drive(3).unwrap();
    bus.mount_drive(3, &cue, MountOptions::default(), false).unwrap();
    assert!(tur(&mut bus).is_err());
    assert_eq!(sense(&mut bus), (0x02, 0x3A, 0x00), "not there while it goes in");
    wait_ms(&mut bus, 4100.0);
    assert!(tur(&mut bus).is_err());
    assert_eq!(sense(&mut bus), (0x02, 0x04, 0x01), "becoming ready");
    wait_ms(&mut bus, 1100.0);
    assert!(tur(&mut bus).is_err());
    assert_eq!(sense(&mut bus), (0x02, 0x28, 0x00), "changed");
    // A read clears the change and goes on.
    let pvd = packet(&mut bus, read10(16, 1), 2048).unwrap();
    assert_eq!(&pvd[1..6], b"CD001");
    assert_eq!(tur(&mut bus), Ok(vec![]));
}

#[test]
fn unknown_commands_say_why() {
    let (mut bus, _) = drive("unknown");
    assert!(packet(&mut bus, [0x46, 0, 0, 0, 0, 0, 0, 0, 8, 0, 0, 0], 8).is_err());
    assert_eq!(sense(&mut bus), (0x05, 0x20, 0x00), "illegal request, invalid opcode");
    // MODE SENSE's capabilities page.
    let page = packet(&mut bus, [0x5A, 0, 0x2A, 0, 0, 0, 0, 0, 30, 0, 0, 0], 30).unwrap();
    assert_eq!(page[8] & 0x3F, 0x2A);
    assert_eq!(page[12], 0x71, "audio play, multisession");
}

#[test]
fn a_state_saved_mid_transfer_goes_on() {
    let (bus, image) = drive("state");
    let mut cpu = rust_dos::cpu::Cpu::new(std::path::PathBuf::from("."));
    cpu.bus = bus;
    let bus = &mut cpu.bus;
    bus.io_write(SELECT, 0xA0);
    bus.io_write(LBA_MID, 0);
    bus.io_write(LBA_HIGH, 8);
    bus.io_write(COMMAND, 0xA0);
    wait_ready(bus);
    for w in read10(16, 2).chunks(2) {
        bus.io_write_wide(DATA, u16::from_le_bytes([w[0], w[1]]) as u32, 2);
    }
    wait_ready(bus);
    // Half the first sector (the first block) read.
    let first: Vec<u16> = (0..512).map(|_| bus.io_read_wide(DATA, 2) as u16).collect();
    let state = rust_dos::savestate::machine::save(&cpu);
    // The rest of the block, and after the drive's pause the second.
    let finish = |bus: &mut Bus| -> Vec<u16> {
        let mut words: Vec<u16> = (0..512).map(|_| bus.io_read_wide(DATA, 2) as u16).collect();
        wait_ready(bus);
        bus.io_read(COMMAND);
        words.extend((0..1024).map(|_| bus.io_read_wide(DATA, 2) as u16));
        words
    };
    let rest = finish(&mut cpu.bus);
    rust_dos::savestate::machine::load(&mut cpu, &state).unwrap();
    let again = finish(&mut cpu.bus);
    assert_eq!(rest, again);
    let bytes: Vec<u8> = first.iter().chain(&rest).flat_map(|w| w.to_le_bytes()).collect();
    assert_eq!(bytes, &image[16 * 2048..18 * 2048]);
}

#[test]
fn the_channel_comes_with_a_cd_image_and_goes() {
    let (mut bus, _) = drive("pnp");
    assert!(bus.ide[1].is_some());
    bus.detach_ide();
    assert!(bus.ide[1].is_none());
    // Without a CD image, an empty drive, or with `boot_cdrom` off none.
    let mut plain = Bus::new(std::path::PathBuf::from("."));
    plain.attach_ide();
    assert!(plain.ide[1].is_some());
    plain.boot_cdrom = false;
    plain.attach_ide();
    assert!(plain.ide[1].is_none());
}

#[test]
fn a_sound_card_on_irq_15_keeps_the_channel_out() {
    let (mut bus, _) = drive("irq15");
    bus.configure_sound(Some(rust_dos::sb::SbConfig { irq: 15, ..Default::default() }), true);
    bus.attach_ide();
    assert!(bus.ide[1].is_none());
    assert_eq!(bus.io_read(COMMAND), 0xFF, "the ports are nobody's");
    bus.configure_sound(Some(rust_dos::sb::SbConfig::default()), true);
    bus.attach_ide();
    assert!(bus.ide[1].is_some());
}
