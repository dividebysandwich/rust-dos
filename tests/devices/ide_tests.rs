//! The ATAPI CD-ROM drive a booted system finds on the secondary IDE
//! channel, driven through its ports as a driver does: the signature after
//! a reset, IDENTIFY PACKET DEVICE, packet commands with their data blocks
//! and interrupts, reads, the table of contents, CD audio, a new disc, and
//! save states.

use crate::cdimage;

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
fn sectors_whose_edc_fails_are_read_errors() {
    let dir = cdimage::scratch("ide_unreadable");
    fs::create_dir_all(dir.join("c")).unwrap();
    // Three sectors after the file system, the middle one as copy
    // protections master it: an EDC that doesn't match its data.
    let mut image = iso("TESTDISC", &[file("HELLO.TXT", b"hello")]);
    let bad = (image.len() / 2048 + 1) as u32;
    image.extend(vec![0x55; 3 * 2048]);
    let mut raw = cdimage::mode1_2352(&image);
    let at = bad as usize * 2352 + 2064;
    raw[at..at + 4].copy_from_slice(&[0x55; 4]);
    fs::write(dir.join("GAME.BIN"), raw).unwrap();
    let cue = dir.join("GAME.CUE");
    fs::write(
        &cue,
        "FILE \"GAME.BIN\" BINARY\r\n  TRACK 01 MODE1/2352\r\n    INDEX 01 00:00:00\r\n",
    )
    .unwrap();
    let mut bus = Bus::new(dir.join("c"));
    bus.mount_drive(3, &cue, MountOptions::default(), false)
        .unwrap();
    bus.set_cycles_per_ms(1000);
    bus.attach_ide();

    // READ CD as protections send it: any sector type, sync, header, user
    // data, EDC and ECC.
    let read_cd = |lba: u32| {
        let l = lba.to_be_bytes();
        [0xBE, 0, l[0], l[1], l[2], l[3], 0, 0, 1, 0xF8, 0, 0]
    };
    for cdb in [read_cd(bad), read10(bad, 1), read10(bad - 1, 3)] {
        assert_eq!(
            packet(&mut bus, cdb, 0xFFFE),
            Err(0x34),
            "medium error, aborted: {:02X?}",
            cdb
        );
        assert_eq!(
            sense(&mut bus),
            (0x03, 0x11, 0x00),
            "unrecovered read error"
        );
    }
    // The information field names the sector that failed.
    packet(&mut bus, read10(bad - 1, 3), 0xFFFE).unwrap_err();
    let s = packet(&mut bus, [0x03, 0, 0, 0, 18, 0, 0, 0, 0, 0, 0, 0], 18).unwrap();
    assert_eq!(s[0], 0xF0, "valid, current error");
    assert_eq!(s[3..7], bad.to_be_bytes());
    // The sectors around it read.
    for lba in [bad - 1, bad + 1] {
        assert_eq!(
            packet(&mut bus, read10(lba, 1), 2048).unwrap(),
            vec![0x55; 2048]
        );
        assert_eq!(packet(&mut bus, read_cd(lba), 2352).unwrap().len(), 2352);
        assert_eq!(sense(&mut bus), (0, 0, 0));
    }
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

/// Let `ms` milliseconds pass a second at a time, with the CD audio
/// played as far as the time.
fn play_on(bus: &mut Bus, ms: f64) {
    let mut left = ms;
    while left > 0.0 {
        let step = left.min(1000.0);
        wait_ms(bus, step);
        bus.audio_catch_up();
        left -= step;
    }
}

/// How long, in ms, a packet command takes from the host's first write.
fn timed(bus: &mut Bus, cdb: [u8; 12], limit: u16) -> f64 {
    let start = bus.clock.icount;
    packet(bus, cdb, limit).unwrap();
    (bus.clock.icount - start) as f64 / 1000.0
}

#[test]
fn a_disc_playing_audio_keeps_turning_until_the_play_ends() {
    // A data track and 20 s of audio. The drive stops a disc 10 s after
    // the last command (1 s to spin it up again), but not while it plays
    // audio: the play keeps it turning, and the 10 s count from its end.
    let dir = cdimage::scratch("ide_play_spin");
    fs::create_dir_all(dir.join("c")).unwrap();
    let mut bus = Bus::new(dir.join("c"));
    let image = iso("TESTDISC", &[file("HELLO.TXT", b"hello")]);
    let cue = mixed_disc(&dir, &image, 1500);
    bus.mount_drive(3, &cue, MountOptions::default(), false)
        .unwrap();
    bus.set_cycles_per_ms(1000);
    bus.attach_ide();
    let track2 = (image.len() / 2048) as u32 + 150;
    let play = |bus: &mut Bus, sectors: u32| {
        let (m, s, f) = rust_dos::cdrom::lba_to_msf(track2);
        let (em, es, ef) = rust_dos::cdrom::lba_to_msf(track2 + sectors);
        packet(bus, [0x47, 0, 0, m, s, f, em, es, ef, 0, 0, 0], 0).unwrap();
    };
    // Spun up by a read.
    assert!(timed(&mut bus, read10(16, 1), 2048) > 900.0);
    // The whole track; a read 12 s into it finds the disc turning.
    play(&mut bus, 1500);
    play_on(&mut bus, 12_000.0);
    let during = timed(&mut bus, read10(17, 1), 2048);
    assert!(during < 500.0, "a read during the play took {during} ms");
    // Two seconds of it; 11 s after it began, 9 s after it ended, the
    // disc still turns, and 11 s after that read it has stopped.
    play_on(&mut bus, 10_000.0);
    play(&mut bus, 150);
    play_on(&mut bus, 11_000.0);
    let after = timed(&mut bus, read10(18, 1), 2048);
    assert!(after < 500.0, "a read 9 s after the play took {after} ms");
    play_on(&mut bus, 11_000.0);
    let stopped = timed(&mut bus, read10(19, 1), 2048);
    assert!(
        stopped > 900.0,
        "a read 11 s after the last took {stopped} ms"
    );
}

#[test]
fn a_new_disc_starts_with_an_empty_buffer() {
    // The drive reads ahead after a read of sector 16. A new disc goes in:
    // what the buffer held was the old disc's, so a read of sector 17 of
    // the new one moves the head and waits for the sector to come round.
    let (mut bus, _) = drive("change_buffer");
    let mut cd = rust_dos::ide::Atapi::new(3, true);
    let mut env = rust_dos::ide::Env {
        disks: &bus.disk,
        player: &mut bus.cdaudio,
        now: 0,
        hard_disk_ns_per_sector: 0,
        faked: false,
        moved: 0,
    };
    assert_eq!(cd.run_packet(&read10(16, 1), &[], &mut env).sense, None);
    cd.media_changed(true, env.now);
    // TEST UNIT READY, a second apart, until the drive has reported the
    // disc going in, spinning up and changed, and is ready.
    let mut reports = Vec::new();
    for _ in 0..10 {
        env.now += rust_dos::timer::PIT_HZ;
        match cd.run_packet(&[0; 12], &[], &mut env).sense {
            Some(s) => reports.push((s[2] & 0xF, s[12])),
            None => break,
        }
    }
    assert_eq!(reports.last(), Some(&(2, 0x28)), "{reports:?}");
    let before = env.now;
    let read = cd.run_packet(&read10(17, 1), &[], &mut env);
    assert_eq!(read.sense, None);
    let ms = (env.now - before) as f64 * 1000.0 / rust_dos::timer::PIT_HZ as f64;
    assert!(ms > 1.0, "the read took {ms} ms");
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

#[test]
fn a_packet_run_at_once_gives_what_the_channel_gives() {
    // The same commands through the channel's ports, and run at once on a
    // drive of its own (`Atapi::run_packet`, the built-in Windows's
    // ASPI): the same data, and the same errors.
    let (mut bus, _) = drive("run_packet");
    let mut cd = rust_dos::ide::Atapi::new(3, true);
    let mut now = bus.clock.now_ticks();
    let read_cd = |lba: u32| {
        let l = lba.to_be_bytes();
        [0xBE, 0, l[0], l[1], l[2], l[3], 0, 0, 2, 0xF8, 0, 0]
    };
    let commands = [
        read10(16, 40),
        [0x12, 0, 0, 0, 36, 0, 0, 0, 0, 0, 0, 0],
        [0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        [0x43, 0, 0, 0, 0, 0, 0, 0x03, 0x24, 0, 0, 0],
        read_cd(17),
        [0x5A, 0, 0x2A, 0, 0, 0, 0, 0, 0x40, 0, 0, 0],
        // Past the disc's end, then what the drive says of it.
        read10(100_000, 1),
        [0x03, 0, 0, 0, 18, 0, 0, 0, 0, 0, 0, 0],
        // A command the drive doesn't know.
        [0xFF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        [0x03, 0, 0, 0, 18, 0, 0, 0, 0, 0, 0, 0],
    ];
    for cdb in commands {
        let through_ports = packet(&mut bus, cdb, 0xFFFE);
        let mut env = rust_dos::ide::Env {
            disks: &bus.disk,
            player: &mut bus.cdaudio,
            now,
            hard_disk_ns_per_sector: 0,
            faked: false,
            moved: 0,
        };
        let outcome = cd.run_packet(&cdb, &[], &mut env);
        assert!(env.now >= now, "time doesn't go back");
        now = env.now;
        match through_ports {
            Ok(data) => {
                assert_eq!(outcome.sense, None, "{:02X?}", cdb);
                assert_eq!(outcome.data, data, "{:02X?}", cdb);
            }
            // The error register doesn't always carry the sense key (an
            // unknown command's is F4h): the REQUEST SENSE after it
            // compares the sense data.
            Err(_) => assert!(outcome.sense.is_some(), "an error: {:02X?}", cdb),
        }
    }
}

#[test]
fn a_packet_run_at_once_sees_a_disc_change_once_its_timers_have_run() {
    // A disc goes in while the drive has no channel. Its timers run between
    // commands, so the drive is ready for a read a minute later.
    let (mut bus, _) = drive("run_packet_change");
    let mut cd = rust_dos::ide::Atapi::new(3, true);
    cd.media_changed(true, 0);
    let mut env = rust_dos::ide::Env {
        disks: &bus.disk,
        player: &mut bus.cdaudio,
        now: 0,
        hard_disk_ns_per_sector: 0,
        faked: false,
        moved: 0,
    };
    let early = cd.run_packet(&read10(16, 1), &[], &mut env);
    assert!(early.sense.is_some(), "the disc is still going in");
    env.now = 60 * rust_dos::timer::PIT_HZ;
    // The first command after the change reports it (MEDIUM MAY HAVE
    // CHANGED, 28h), and the next one reads.
    let changed = cd.run_packet(&read10(16, 1), &[], &mut env);
    assert_eq!(changed.sense.map(|s| (s[2] & 0xF, s[12])), Some((2, 0x28)));
    let later = cd.run_packet(&read10(16, 1), &[], &mut env);
    assert_eq!(later.sense, None);
    assert_eq!(later.data.len(), 2048);
}

#[test]
fn blindwrite_subchannel_sessions_and_catalog_number() {
    let dir = cdimage::scratch("ide_blindwrite");
    fs::create_dir_all(dir.join("c")).unwrap();
    let image = iso("TESTDISC", &[file("HELLO.TXT", b"hello")]);
    let second = 12_000;
    let descriptor = cdimage::blindwrite_disc(&dir, &image, second, "0123456789012");
    let mut bus = Bus::new(dir.join("c"));
    bus.mount_drive(3, &descriptor, MountOptions::default(), false)
        .unwrap();
    bus.set_cycles_per_ms(1000);
    bus.attach_ide();

    // READ TOC's session format: the last session's first track.
    let toc = packet(&mut bus, [0x43, 0, 1, 0, 0, 0, 0, 0, 12, 0, 0, 0], 12).unwrap();
    assert_eq!((toc[2], toc[3], toc[6]), (1, 2, 2));
    assert_eq!(u32::from_be_bytes(toc[8..12].try_into().unwrap()), second);

    // READ CD of sector 16 with each subchannel selection: the image's.
    // Any sector type, but Mode 1 for its user data alone.
    let read_cd = |lba: u32, fields: u8, sub: u8| {
        let l = lba.to_be_bytes();
        let kind = if fields == 0x10 { 0x08 } else { 0 };
        [0xBE, kind, l[0], l[1], l[2], l[3], 0, 0, 1, fields, sub, 0]
    };
    let raw = packet(&mut bus, read_cd(16, 0xF8, 1), 0xFFFE).unwrap();
    assert_eq!(raw.len(), 2352 + 96);
    assert_eq!(raw[16..2064], image[16 * 2048..17 * 2048]);
    // Bit 7 of each byte is P, bit 6 Q, bits 5 to 0 R to W: the first bit
    // of A5h is 1, the second 0. Q starts with control 4, ADR 1.
    let sub = &raw[2352..];
    assert_eq!(sub[..2], [0x3F, 0x40]);
    assert!(sub.iter().all(|b| b & 0x80 == 0), "no P in a track");
    let rw = packet(&mut bus, read_cd(16, 0, 4), 0xFFFE).unwrap();
    assert_eq!(rw.len(), 96);
    assert!(rw.iter().zip(sub).all(|(&r, &s)| r == s & 0x3F));
    let q = packet(&mut bus, read_cd(16, 0x10, 2), 0xFFFE).unwrap();
    assert_eq!(q.len(), 2048 + 16);
    assert_eq!(
        q[2048..2058],
        [0x41, 0x07, 0x01, 0x00, 0x00, 0x16, 0, 0x00, 0x02, 0x16]
    );
    assert_eq!(q[2060..], [0; 4]);

    // READ SUB-CHANNEL after a read: the Q of the image at the head.
    packet(&mut bus, read10(17, 1), 2048).unwrap();
    let position = packet(&mut bus, [0x42, 0, 0x40, 1, 0, 0, 0, 0, 16, 0, 0, 0], 16).unwrap();
    assert_eq!((position[5], position[6], position[7]), (0x14, 7, 1));
    assert_eq!(u32::from_be_bytes(position[8..12].try_into().unwrap()), 17);
    assert_eq!(u32::from_be_bytes(position[12..16].try_into().unwrap()), 17);
    // SEEK moves the head too.
    packet(&mut bus, [0x2B, 0, 0, 0, 0, 20, 0, 0, 0, 0, 0, 0], 0).unwrap();
    let position = packet(&mut bus, [0x42, 0, 0x40, 1, 0, 0, 0, 0, 16, 0, 0, 0], 16).unwrap();
    assert_eq!(u32::from_be_bytes(position[8..12].try_into().unwrap()), 20);
    // A subchannel selection MMC reserves is an invalid field.
    assert!(packet(&mut bus, read_cd(16, 0xF8, 3), 0xFFFE).is_err());
    assert_eq!(sense(&mut bus), (0x05, 0x24, 0x00));
    // The media catalog number.
    let mcn = packet(&mut bus, [0x42, 0, 0x40, 2, 0, 0, 0, 0, 24, 0, 0, 0], 24).unwrap();
    assert_eq!((mcn[4], mcn[8]), (2, 0x80));
    assert_eq!(&mcn[9..22], b"0123456789012");

    // Session 2 has no subchannel: P and Q as the layout makes them.
    let q = packet(&mut bus, read_cd(second + 3, 0, 2), 0xFFFE).unwrap();
    assert_eq!(q[..3], [0x41, 0x02, 0x01]);
}

/// The Q a pressed disc has at sector `lba` of the disc of
/// `alcohol_split_disc`: a data track 1 to sector 31, then audio track 2
/// with a pregap to 181.
fn split_disc_q(lba: u32) -> [u8; 12] {
    let bcd = |v: u32| (((v / 10) << 4) | (v % 10)) as u8;
    let (control, track, index, relative) = match lba {
        0..31 => (0x41, 1, 1, lba),
        31..181 => (0x01, 2, 0, 180 - lba),
        _ => (0x01, 2, 1, lba - 181),
    };
    let abs = lba + 150;
    let mut q = [
        control,
        track,
        index,
        bcd(relative / 75 / 60),
        bcd(relative / 75 % 60),
        bcd(relative % 75),
        0,
        bcd(abs / 75 / 60),
        bcd(abs / 75 % 60),
        bcd(abs % 75),
        0,
        0,
    ];
    let crc = cdimage::q_crc(&q);
    q[10..12].copy_from_slice(&crc.to_be_bytes());
    q
}

/// The bincue-split copy of refurbished-dinosaurs-toolkit's disc
/// archiver: a data track holding `iso_image` in "Synth (Track 1).bin",
/// an audio track after a pregap of 150 in "Synth (Track 2).bin", and
/// the archiver's descriptor of them with its DPM data, Synth.mds. Beside
/// it, Synth.subcode as redumper writes it (raw P-W from sector -150 to
/// 100 past the lead-out), with the disc's Q, R holding the sector's low
/// byte, and the Q CRC of the sectors in `bad` broken. Returns the
/// descriptor's path.
fn alcohol_split_disc(dir: &std::path::Path, iso_image: &[u8], bad: &[u32]) -> std::path::PathBuf {
    let mut data = iso_image.to_vec();
    data.resize(31 * 2048, 0);
    fs::write(dir.join("Synth (Track 1).bin"), cdimage::mode1_2352(&data)).unwrap();
    fs::write(dir.join("Synth (Track 2).bin"), cdimage::audio(450)).unwrap();
    let mds = dir.join("Synth.mds");
    fs::write(
        &mds,
        include_bytes!("../../src/cdrom/testdata/toolkit-split.mds"),
    )
    .unwrap();
    let mut subcode = vec![0u8; 150 * 96];
    for lba in 0..481u32 {
        let mut linear = [0u8; 96];
        linear[12..24].copy_from_slice(&split_disc_q(lba));
        if bad.contains(&lba) {
            linear[22] ^= 0x80;
            linear[23] ^= 0x01;
        }
        linear[24] = lba as u8;
        subcode.extend(rust_dos::cdrom::interleave_subchannel(&linear));
    }
    subcode.extend(vec![0u8; 100 * 96]);
    fs::write(dir.join("Synth.subcode"), subcode).unwrap();
    mds
}

#[test]
fn an_alcohol_descriptor_with_a_subcode_file_beside_it() {
    let dir = cdimage::scratch("ide_alcohol_subcode");
    fs::create_dir_all(dir.join("c")).unwrap();
    let image = iso("TESTDISC", &[file("HELLO.TXT", b"hello")]);
    let mds = alcohol_split_disc(&dir, &image, &[20, 200]);
    let mut bus = Bus::new(dir.join("c"));
    bus.mount_drive(3, &mds, MountOptions::default(), false)
        .unwrap();
    bus.set_cycles_per_ms(1000);
    bus.attach_ide();
    // The descriptor's DPM data is in force with the file's subchannel.
    let disc = bus.disk.cd_image(3).unwrap();
    assert_eq!(
        disc.dpm().map(|d| (d.start, d.resolution, d.angles.len())),
        Some((0, 10, 48))
    );

    let read_cd = |lba: u32, sub: u8| {
        let l = lba.to_be_bytes();
        [0xBE, 0, l[0], l[1], l[2], l[3], 0, 0, 1, 0, sub, 0]
    };
    for lba in [16, 20, 100, 200] {
        // Formatted Q: the file's, a broken CRC as it is.
        let q = packet(&mut bus, read_cd(lba, 2), 16).unwrap();
        let mut want = split_disc_q(lba);
        if [20, 200].contains(&lba) {
            want[10] ^= 0x80;
            want[11] ^= 0x01;
        }
        assert_eq!(q[..12], want, "sector {}", lba);
        // Raw P-W, the file's: R holds the sector's low byte, and P is
        // clear even in track 2's pregap, where the layout would set it.
        let raw = packet(&mut bus, read_cd(lba, 1), 96).unwrap();
        let r: u8 = (0..8).fold(0, |r, i| (r << 1) | ((raw[i] >> 5) & 1));
        assert_eq!(r, lba as u8, "sector {}", lba);
        assert!(raw.iter().all(|b| b & 0x80 == 0), "sector {}", lba);
    }
    // READ SUB-CHANNEL after reading a sector whose Q is bad: the position
    // of the layout, as a drive reports the last good one.
    let l = 200u32.to_be_bytes();
    packet(
        &mut bus,
        [0xBE, 0, l[0], l[1], l[2], l[3], 0, 0, 1, 0xF8, 0, 0],
        2352,
    )
    .unwrap();
    let position = packet(&mut bus, [0x42, 0, 0x40, 1, 0, 0, 0, 0, 16, 0, 0, 0], 16).unwrap();
    assert_eq!((position[5], position[6], position[7]), (0x10, 2, 1));
    assert_eq!(u32::from_be_bytes(position[8..12].try_into().unwrap()), 200);
    assert_eq!(u32::from_be_bytes(position[12..16].try_into().unwrap()), 19);
}

#[test]
fn a_disc_without_subchannel_reads_the_layouts() {
    let (mut bus, image) = drive("made_subchannel");
    let track2 = (image.len() / 2048) as u32 + 150;
    let read_cd = |lba: u32| {
        let l = lba.to_be_bytes();
        [0xBE, 0, l[0], l[1], l[2], l[3], 0, 0, 1, 0, 2, 0]
    };
    // Track 2's pregap: index 0, counting down to 0 at its last sector,
    // with P set.
    let q = packet(&mut bus, read_cd(track2 - 2), 16).unwrap();
    assert_eq!(q[..6], [0x01, 0x02, 0x00, 0x00, 0x00, 0x01]);
    let crc = cdimage::q_crc(&q).to_be_bytes();
    assert_eq!(q[10..12], crc);
    let l = (track2 - 2).to_be_bytes();
    let raw = packet(
        &mut bus,
        [0xBE, 0, l[0], l[1], l[2], l[3], 0, 0, 1, 0, 1, 0],
        96,
    )
    .unwrap();
    assert!(raw.iter().all(|b| b & 0x80 != 0), "P in a pregap");
    // In LBA form READ SUB-CHANNEL counts a pregap back from the track.
    packet(
        &mut bus,
        [0xBE, 0, l[0], l[1], l[2], l[3], 0, 0, 1, 0xF8, 0, 0],
        2352,
    )
    .unwrap();
    let position = packet(&mut bus, [0x42, 0, 0x40, 1, 0, 0, 0, 0, 16, 0, 0, 0], 16).unwrap();
    assert_eq!((position[5], position[6], position[7]), (0x10, 2, 0));
    assert_eq!(i32::from_be_bytes(position[12..16].try_into().unwrap()), -2);
}
