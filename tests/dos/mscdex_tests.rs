use crate::cdimage;

use cdimage::{audio_sample, file, high_sierra, iso, mixed_disc};
use rust_dos::cdrom::redbook;
use rust_dos::cpu::{Cpu, CpuFlags};
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
    cpu.set_ax(ax);
    cpu.set_bx(bx);
    cpu.set_cx(cx);
    int2f::handle(cpu);
}

#[test]
fn not_installed_without_cd_drives() {
    let base = scratch("absent", &["c"]);
    let mut cpu = Cpu::new(base.join("c"));
    int2f(&mut cpu, 0x1500, 0, 0);
    assert_eq!((cpu.ax(), cpu.bx()), (0x1500, 0));
    int2f(&mut cpu, 0x150B, 0, 3);
    assert_ne!(cpu.bx(), 0xADAD);

    // Other multiplex install checks stay "not installed" (XMS, 4300h,
    // is installed: see extender_support_tests, and DPMI, 1687h: see
    // dpmi_tests)
    for ax in [0x1600u16, 0x1680, 0x1100] {
        int2f(&mut cpu, ax, 0x1234, 0x5678);
        assert_eq!((cpu.ax(), cpu.bx(), cpu.cx()), (ax, 0x1234, 0x5678));
    }
}

#[test]
fn mscdex_reports_mounted_cd_drives() {
    let base = scratch("present", &["c", "cd1", "cd2", "d"]);
    let mut cpu = Cpu::new(base.join("c"));
    cpu.bus
        .mount_drive(3, &base.join("d"), MountOptions::default(), false)
        .unwrap();
    cpu.bus
        .mount_drive(4, &base.join("cd1"), cdrom(), false)
        .unwrap();
    cpu.bus
        .mount_drive(6, &base.join("cd2"), cdrom(), false)
        .unwrap();

    int2f(&mut cpu, 0x1500, 0, 0);
    assert_eq!((cpu.bx(), cpu.cx()), (2, 4));

    int2f(&mut cpu, 0x150B, 0, 4);
    assert_eq!(cpu.bx(), 0xADAD);
    assert_ne!(cpu.ax(), 0);
    int2f(&mut cpu, 0x150B, 0, 3);
    assert_eq!((cpu.ax(), cpu.bx()), (0, 0xADAD));

    int2f(&mut cpu, 0x150C, 0, 0);
    assert_eq!(cpu.bx(), 0x0217);

    cpu.set_es(0x3000);
    int2f(&mut cpu, 0x150D, 0, 0);
    assert_eq!(cpu.bus.read_8(0x30000), 4);
    assert_eq!(cpu.bus.read_8(0x30001), 6);
}

/// MSCDEX's check through the redirector's (INT 2Fh AX=1100h): the word
/// the caller pushed before the INT, above the INT's IP, CS and FLAGS.
fn redirector_check(cpu: &mut Cpu, pushed: u16) -> u16 {
    let frame = 0x30000; // 3000:0000
    cpu.set_ss(0x3000);
    cpu.set_sp(0x0000);
    cpu.bus.write_16(frame + 6, pushed);
    int2f(cpu, 0x1100, 0x1234, 0x5678);
    cpu.bus.read_16(frame + 6)
}

#[test]
fn the_redirector_check_finds_mscdex_with_a_cd_drive() {
    let base = scratch("redirector", &["c", "cd"]);
    let mut cpu = Cpu::new(base.join("c"));

    // Without a CD-ROM drive MSCDEX isn't there to answer.
    assert_eq!(redirector_check(&mut cpu, 0xDADA), 0xDADA);
    assert_eq!(cpu.ax(), 0x1100);

    cpu.bus
        .mount_drive(3, &base.join("cd"), cdrom(), false)
        .unwrap();
    assert_eq!(redirector_check(&mut cpu, 0xDADA), 0xADAD);
    assert_eq!((cpu.ax(), cpu.bx(), cpu.cx()), (0x11FF, 0x1234, 0x5678));

    // Without DADAh it is the redirector's own check, which stays
    // unanswered (#1303).
    assert_eq!(redirector_check(&mut cpu, 0x0000), 0x0000);
    assert_eq!(cpu.ax(), 0x1100);
}

#[test]
fn device_requests_answer_ioctl_queries() {
    let base = scratch("ioctl", &["c", "cd"]);
    let mut cpu = Cpu::new(base.join("c"));
    cpu.bus
        .mount_drive(3, &base.join("cd"), cdrom(), false)
        .unwrap();

    let header = 0x30000; // 3000:0000
    let buffer = 0x31000; // 3100:0000
    let request = |cpu: &mut Cpu, command: u8, control: u8| {
        cpu.bus.write_8(header + 2, command);
        cpu.bus.write_16(header + 3, 0);
        cpu.bus.write_16(header + 0x0E, 0x0000);
        cpu.bus.write_16(header + 0x10, 0x3100);
        cpu.bus.write_8(buffer, control);
        cpu.set_es(0x3000);
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

/// A disc with a data track (sectors 0-24) and a 10-sector audio track
/// after a two-second pregap (sectors 175-184), mounted as D:. The mixer
/// plays only the CD, at full volume.
fn disc(name: &str) -> (Box<Cpu>, u32) {
    let dir = cdimage::scratch(&format!("mscdex_{}", name));
    fs::create_dir_all(dir.join("c")).unwrap();
    let mut cpu = Cpu::new(dir.join("c"));
    let files = [file("HELLO.COM", &[0xB4, 0x4C, 0xCD, 0x21]), file("DATA\\BIG.DAT", &[7; 9000])];
    let image = iso("TESTDISC", &files);
    let data_sectors = (image.len() / 2048) as u32;
    let cue = mixed_disc(&dir, &image, 10);
    cpu.bus.mount_drive(3, &cue, MountOptions::default(), false).unwrap();
    cpu.bus.configure_sound(None, false);
    cpu.bus.gus = None;
    cpu.bus.set_cycles_per_ms(44_100);
    (cpu, data_sectors + 150)
}

const HEADER: usize = 0x30000; // 3000:0000
const BUFFER: usize = 0x31000; // 3100:0000

/// Send device request `command` for D: through AX=1510h, with the
/// parameters from +0Dh on. Returns the status word.
fn request(cpu: &mut Cpu, command: u8, params: &[u8]) -> u16 {
    for i in 0..0x20 {
        cpu.bus.write_8(HEADER + i, 0);
    }
    cpu.bus.write_8(HEADER, 0x1B);
    cpu.bus.write_8(HEADER + 2, command);
    for (i, &b) in params.iter().enumerate() {
        cpu.bus.write_8(HEADER + 0x0D + i, b);
    }
    cpu.set_es(0x3000);
    int2f(cpu, 0x1510, 0, 3);
    cpu.bus.read_16(HEADER + 3)
}

/// IOCTL input (03h) or output (0Ch) with the control block `block` at
/// 3100:0000.
fn ioctl(cpu: &mut Cpu, command: u8, block: &[u8]) -> u16 {
    for i in 0..16 {
        cpu.bus.write_8(BUFFER + i, 0);
    }
    for (i, &b) in block.iter().enumerate() {
        cpu.bus.write_8(BUFFER + i, b);
    }
    // Media byte, then the transfer address 3100:0000.
    request(cpu, command, &[0, 0x00, 0x00, 0x00, 0x31, block.len() as u8, 0])
}

fn le(v: u32) -> [u8; 4] {
    v.to_le_bytes()
}

/// Let exactly `frames` frames of audio time pass and mix them. At 44 100
/// instructions per millisecond a frame is 1000 instructions; steps of 100
/// render at most one frame each.
fn play_for(cpu: &mut Cpu, frames: usize) -> Vec<(i16, i16)> {
    cpu.bus.audio_catch_up();
    cpu.bus.audio_out.clear();
    while cpu.bus.audio_out.len() < frames * 2 {
        cpu.bus.clock.icount += 100;
        cpu.bus.audio_catch_up();
    }
    let out: Vec<i16> = cpu.bus.audio_out.drain(..).collect();
    out.chunks(2).map(|c| (c[0], c[1])).collect()
}

#[test]
fn images_answer_the_volume_calls() {
    let (mut cpu, _) = disc("volume");
    cpu.set_es(0x3100);

    // The device list points at the CD driver's header.
    int2f(&mut cpu, 0x1501, 0, 0);
    assert_eq!(cpu.bus.read_8(BUFFER), 0);
    let header = (cpu.bus.read_16(BUFFER + 3) as usize) << 4 | cpu.bus.read_16(BUFFER + 1) as usize;
    let name: Vec<u8> = (0..8).map(|i| cpu.bus.read_8(header + 0x0A + i)).collect();
    assert_eq!(&name, b"MSCD001 ");
    assert_eq!((cpu.bus.read_8(header + 0x14), cpu.bus.read_8(header + 0x15)), (4, 1));

    int2f(&mut cpu, 0x1502, 0, 3);
    assert!(!cpu.get_cpu_flag(CpuFlags::CF));
    let name: Vec<u8> = (0..15).map(|i| cpu.bus.read_8(BUFFER + i)).collect();
    assert_eq!(&name, b"COPYRIGHT.TXT;1");

    // Volume descriptors: the primary one, then the terminator.
    cpu.set_dx(0);
    int2f(&mut cpu, 0x1505, 0, 3);
    assert_eq!(cpu.ax(), 1);
    assert_eq!(cpu.bus.read_8(BUFFER + 1), b'C');
    cpu.set_dx(1);
    int2f(&mut cpu, 0x1505, 0, 3);
    assert_eq!(cpu.ax(), 0xFF);

    // Absolute read of sector 16.
    cpu.set_si(0);
    cpu.set_di(16);
    cpu.set_dx(1);
    int2f(&mut cpu, 0x1508, 0, 3);
    assert!(!cpu.get_cpu_flag(CpuFlags::CF));
    assert_eq!(cpu.bus.read_8(BUFFER + 5), b'1');
    int2f(&mut cpu, 0x1508, 0, 2);
    assert!(cpu.get_cpu_flag(CpuFlags::CF));
    assert_eq!(cpu.ax(), 15, "C: is no CD drive");

    // Directory entry: the ISO 9660 record.
    for (i, &b) in b"D:\\HELLO.COM\0".iter().enumerate() {
        cpu.bus.write_8(0x32000 + i, b);
    }
    cpu.set_es(0x3200);
    cpu.set_si(0x3300);
    cpu.set_di(0);
    int2f(&mut cpu, 0x150F, 0, 3);
    assert_eq!(cpu.ax(), 1);
    assert_eq!(cpu.bus.read_32(0x33000 + 10), 4, "file size");
}

#[test]
fn high_sierra_discs_answer_the_volume_calls() {
    let dir = cdimage::scratch("mscdex_high_sierra");
    fs::create_dir_all(dir.join("c")).unwrap();
    let mut cpu = Cpu::new(dir.join("c"));
    let image = high_sierra("OLDDISC", &[file("HELLO.COM", &[0xB4, 0x4C, 0xCD, 0x21])]);
    fs::write(dir.join("old.iso"), image).unwrap();
    let opts = MountOptions::default();
    cpu.bus
        .mount_drive(3, &dir.join("old.iso"), opts, false)
        .unwrap();
    cpu.set_es(0x3100);

    // High Sierra's copyright file name is 32 characters.
    int2f(&mut cpu, 0x1502, 0, 3);
    assert!(!cpu.get_cpu_flag(CpuFlags::CF));
    let name: Vec<u8> = (0..33).map(|i| cpu.bus.read_8(BUFFER + i)).collect();
    assert_eq!(&name[..15], b"COPYRIGHT.TXT;1");
    assert_eq!(name[32], 0);

    // The volume descriptors' types are at byte 8.
    cpu.set_dx(0);
    int2f(&mut cpu, 0x1505, 0, 3);
    assert_eq!(cpu.ax(), 1);
    cpu.set_dx(1);
    int2f(&mut cpu, 0x1505, 0, 3);
    assert_eq!(cpu.ax(), 0xFF);

    // The directory entry is the disc's record, and AX says High Sierra.
    for (i, &b) in b"D:\\HELLO.COM\0".iter().enumerate() {
        cpu.bus.write_8(0x32000 + i, b);
    }
    cpu.set_es(0x3200);
    cpu.set_si(0x3300);
    cpu.set_di(0);
    int2f(&mut cpu, 0x150F, 0, 3);
    assert!(!cpu.get_cpu_flag(CpuFlags::CF));
    assert_eq!(cpu.ax(), 0);
    assert_eq!(cpu.bus.read_32(0x33000 + 10), 4, "file size");
}

#[test]
fn read_long_cooked_and_raw() {
    let (mut cpu, _) = disc("read");
    // Cooked sector 16 in HSG addressing.
    let mut params = vec![0, 0x00, 0x00, 0x00, 0x31, 1, 0];
    params.extend(le(16));
    params.push(0);
    assert_eq!(request(&mut cpu, 0x80, &params), 0x0100);
    assert_eq!(cpu.bus.read_8(BUFFER + 1), b'C');
    // Raw, addressed in Red Book.
    let mut params = vec![1, 0x00, 0x00, 0x00, 0x31, 1, 0];
    params.extend(le(redbook(16)));
    params.push(1);
    assert_eq!(request(&mut cpu, 0x80, &params), 0x0100);
    assert_eq!(cpu.bus.read_8(BUFFER + 1), 0xFF, "sync");
    assert_eq!(cpu.bus.read_8(BUFFER + 15), 1, "mode 1");
    assert_eq!(cpu.bus.read_8(BUFFER + 16 + 1), b'C');
    // Past the end of the disc.
    let mut params = vec![0, 0x00, 0x00, 0x00, 0x31, 1, 0];
    params.extend(le(100_000));
    params.push(0);
    assert_eq!(request(&mut cpu, 0x80, &params), 0x8108);
}

/// Seek (83h) to sector `lba` in HSG addressing.
fn seek(cpu: &mut Cpu, lba: u32) -> u16 {
    let mut params = vec![0, 0, 0, 0, 0, 0, 0];
    params.extend(le(lba));
    request(cpu, 0x83, &params)
}

/// Location of the head (IOCTL input 01h) in addressing mode `mode`.
fn head(cpu: &mut Cpu, mode: u8) -> u32 {
    assert_eq!(ioctl(cpu, 0x03, &[0x01, mode]), 0x0100);
    cpu.bus.read_32(BUFFER + 2)
}

/// Read Long (80h) of `count` raw sectors from sector `lba`, in HSG
/// addressing, into 3100:0000.
fn read_raw(cpu: &mut Cpu, lba: u32, count: u8) -> u16 {
    let mut params = vec![0, 0x00, 0x00, 0x00, 0x31, count, 0];
    params.extend(le(lba));
    params.push(1);
    request(cpu, 0x80, &params)
}

#[test]
fn the_head_is_where_the_subchannel_of_the_sector_sought_says() {
    // The test disc with a CloneCD subchannel file beside its sheet: Q
    // names each sector's own place, apart from two sectors copy
    // protections master: one whose good Q names the next sector, and the
    // next one, whose Q has a bad CRC.
    let dir = cdimage::scratch("mscdex_head");
    fs::create_dir_all(dir.join("c")).unwrap();
    let mut cpu = Cpu::new(dir.join("c"));
    let image = iso("TESTDISC", &[file("HELLO.COM", &[0xB4, 0x4C, 0xCD, 0x21])]);
    let sectors = (image.len() / 2048) as u32 + 150;
    let cue = mixed_disc(&dir, &image, 10);
    let (ahead, bad) = (10u32, 11u32);
    let bcd = |v: u32| (((v / 10) << 4) | (v % 10)) as u8;
    let mut sub = Vec::new();
    for lba in 0..sectors + 10 {
        let at = if lba == ahead { lba + 1 } else { lba } + 150;
        let msf = [bcd(at / 75 / 60), bcd(at / 75 % 60), bcd(at % 75)];
        let mut q = [0x41, 0x01, 0x01, 0, 0, 0, 0, msf[0], msf[1], msf[2], 0, 0];
        let crc = cdimage::q_crc(&q) ^ if lba == bad { 0x0101 } else { 0 };
        q[10..12].copy_from_slice(&crc.to_be_bytes());
        sub.extend([0u8; 12]);
        sub.extend(q);
        sub.extend([0u8; 72]);
    }
    fs::write(dir.join("GAME.SUB"), sub).unwrap();
    cpu.bus.mount_drive(3, &cue, MountOptions::default(), false).unwrap();
    assert!(cpu.bus.disk.cd_image(3).is_some());

    assert_eq!(seek(&mut cpu, 5), 0x0100);
    assert_eq!(head(&mut cpu, 0), 5);
    assert_eq!(head(&mut cpu, 1), redbook(5));
    // The drive reports the place the sector's Q names.
    seek(&mut cpu, ahead);
    assert_eq!(head(&mut cpu, 0), ahead + 1);
    // A Q it can't use: the place the disc's layout has.
    seek(&mut cpu, bad);
    assert_eq!(head(&mut cpu, 0), bad);
    // The Q channel query agrees.
    seek(&mut cpu, ahead);
    assert_eq!(ioctl(&mut cpu, 0x03, &[0x0C]), 0x0100);
    let msf = [8, 9, 10].map(|i| cpu.bus.read_8(BUFFER + i) as u32);
    assert_eq!((msf[0] * 60 + msf[1]) * 75 + msf[2], ahead + 1 + 150);
    // A read leaves the head on the last sector read.
    assert_eq!(read_raw(&mut cpu, 7, 2), 0x0100);
    assert_eq!(head(&mut cpu, 0), 8);
}

/// The head stays on the disc, and after a play that ended it is where
/// the play ended until a read moves it. A save state keeps it, and one
/// saved before states kept it loads with it at sector 0.
#[test]
fn the_head_after_a_failed_read_a_play_a_disc_change_and_a_state() {
    use rust_dos::savestate::machine;
    let (mut cpu, audio_start) = disc("head_moves");
    let leadout = audio_start + 10;

    // A read that runs into the lead-out stops the head on the last
    // sector of the disc, and sector numbers past the disc don't move it.
    assert_eq!(read_raw(&mut cpu, leadout - 1, 3), 0x8108);
    assert_eq!(head(&mut cpu, 0), leadout - 1);
    assert_eq!(read_raw(&mut cpu, u32::MAX, 2), 0x8108);
    assert_eq!(seek(&mut cpu, u32::MAX), 0x0100);
    assert_eq!(head(&mut cpu, 1), redbook(leadout - 1));

    // A play that ended leaves the head at its end; a read then moves it.
    seek(&mut cpu, 3);
    let mut play = vec![0];
    play.extend(le(audio_start));
    play.extend(le(2));
    assert_eq!(request(&mut cpu, 0x84, &play), 0x0300);
    play_for(&mut cpu, 3 * 588);
    assert_eq!(head(&mut cpu, 0), audio_start + 2);
    assert_eq!(read_raw(&mut cpu, 5, 1), 0x0100);
    assert_eq!(head(&mut cpu, 0), 5);
    // The audio status still has the range of the last play.
    assert_eq!(ioctl(&mut cpu, 0x03, &[0x0F]), 0x0100);
    assert_eq!(cpu.bus.read_32(BUFFER + 3), redbook(audio_start));

    // A state keeps the head.
    seek(&mut cpu, 7);
    let (state, parts) = machine::save_parts(&cpu);
    seek(&mut cpu, 2);
    machine::load(&mut cpu, &state).unwrap();
    assert_eq!(head(&mut cpu, 0), 7);
    // One saved before it kept the heads, without their section.
    let heads = parts.iter().find(|(name, _)| name == "CDHD").expect("the section").1.clone();
    let mut old = state[..heads.start].to_vec();
    old.extend_from_slice(&state[heads.end..]);
    seek(&mut cpu, 2);
    machine::load(&mut cpu, &old).unwrap();
    assert_eq!(head(&mut cpu, 0), 0);

    // Another disc: the head is at its start.
    seek(&mut cpu, 7);
    cpu.bus.mscdex.disc_changed(3);
    assert_eq!(head(&mut cpu, 0), 0);
}

#[test]
fn catalog_number_and_track_flags_from_the_sheet() {
    let dir = cdimage::scratch("mscdex_catalog");
    fs::create_dir_all(dir.join("c")).unwrap();
    let mut cpu = Cpu::new(dir.join("c"));
    let image = iso("TESTDISC", &[file("HELLO.COM", &[0xC3])]);
    let cue = mixed_disc(&dir, &image, 10);
    let sheet = fs::read_to_string(&cue).unwrap().replace(
        "TRACK 02 AUDIO\r\n",
        "TRACK 02 AUDIO\r\n    FLAGS DCP PRE\r\n",
    );
    fs::write(&cue, format!("CATALOG 0123456789012\r\n{}", sheet)).unwrap();
    cpu.bus
        .mount_drive(3, &cue, MountOptions::default(), false)
        .unwrap();
    assert_eq!(ioctl(&mut cpu, 0x03, &[0x0B, 2]), 0x0100);
    assert_eq!(cpu.bus.read_8(BUFFER + 6), 0x30, "copy permitted, pre-emphasis");
    assert_eq!(ioctl(&mut cpu, 0x03, &[0x0E]), 0x0100);
    let upc: Vec<u8> = (2..9).map(|i| cpu.bus.read_8(BUFFER + i)).collect();
    assert_eq!(upc, [0x01, 0x23, 0x45, 0x67, 0x89, 0x01, 0x20]);
}

#[test]
fn table_of_contents() {
    let (mut cpu, audio_start) = disc("toc");
    assert_eq!(ioctl(&mut cpu, 0x03, &[0x0A]), 0x0100);
    assert_eq!((cpu.bus.read_8(BUFFER + 1), cpu.bus.read_8(BUFFER + 2)), (1, 2));
    assert_eq!(cpu.bus.read_32(BUFFER + 3), redbook(audio_start + 10));
    assert_eq!(ioctl(&mut cpu, 0x03, &[0x0B, 1]), 0x0100);
    assert_eq!((cpu.bus.read_32(BUFFER + 2), cpu.bus.read_8(BUFFER + 6)), (redbook(0), 0x40));
    assert_eq!(ioctl(&mut cpu, 0x03, &[0x0B, 2]), 0x0100);
    assert_eq!((cpu.bus.read_32(BUFFER + 2), cpu.bus.read_8(BUFFER + 6)), (redbook(audio_start), 0x00));
    assert_eq!(ioctl(&mut cpu, 0x03, &[0x0B, 3]), 0x8108);
    assert_eq!(ioctl(&mut cpu, 0x03, &[0x08]), 0x0100);
    assert_eq!(cpu.bus.read_32(BUFFER + 1), audio_start + 10);
    assert_eq!(ioctl(&mut cpu, 0x03, &[0x07, 1]), 0x0100);
    assert_eq!(cpu.bus.read_16(BUFFER + 2), 2352);
    // A new disc shows once.
    assert_eq!(ioctl(&mut cpu, 0x03, &[0x09]), 0x0100);
    assert_eq!(cpu.bus.read_8(BUFFER + 1), 0xFF);
    assert_eq!(ioctl(&mut cpu, 0x03, &[0x09]), 0x0100);
    assert_eq!(cpu.bus.read_8(BUFFER + 1), 1);
}

#[test]
fn audio_tracks_play_pause_and_resume() {
    let (mut cpu, audio_start) = disc("play");
    let mut play = vec![0];
    play.extend(le(audio_start));
    play.extend(le(10));
    assert_eq!(request(&mut cpu, 0x84, &play), 0x0300, "done and busy");

    let out = play_for(&mut cpu, 1000);
    assert_eq!(out.len(), 1000);
    assert!(out.iter().enumerate().all(|(i, &s)| s == audio_sample(i)), "{:?}", &out[..4]);

    // The Q channel follows the play position.
    assert_eq!(ioctl(&mut cpu, 0x03, &[0x0C]), 0x0300);
    assert_eq!((cpu.bus.read_8(BUFFER + 1), cpu.bus.read_8(BUFFER + 2)), (0x01, 2));
    assert_eq!(cpu.bus.read_8(BUFFER + 6), 1, "1000 frames is sector 1 of the track");

    // Stop pauses; Resume goes on where it stopped.
    assert_eq!(request(&mut cpu, 0x85, &[]), 0x0100);
    assert!(play_for(&mut cpu, 500).iter().all(|&s| s == (0, 0)));
    assert_eq!(ioctl(&mut cpu, 0x03, &[0x0F]), 0x0100);
    assert_eq!(cpu.bus.read_16(BUFFER + 1), 1, "paused");
    assert_eq!(cpu.bus.read_32(BUFFER + 3), redbook(audio_start));
    assert_eq!(request(&mut cpu, 0x88, &[]), 0x0300);
    let out = play_for(&mut cpu, 100);
    assert_eq!(out[0], audio_sample(1000));

    // Volume 0 on both channels is silence.
    assert_eq!(ioctl(&mut cpu, 0x0C, &[0x03, 0, 0, 1, 0]), 0x0300);
    assert!(play_for(&mut cpu, 100).iter().all(|&s| s == (0, 0)));
    assert_eq!(ioctl(&mut cpu, 0x0C, &[0x03, 0, 0xFF, 1, 0xFF]), 0x0300);

    // Playing to the end of the range ends it.
    play_for(&mut cpu, 10 * 588);
    assert_eq!(request(&mut cpu, 0x0D, &[]), 0x0100, "not busy any more");

    // Two stops forget the position: nothing to resume.
    assert_eq!(request(&mut cpu, 0x84, &play), 0x0300);
    assert_eq!(request(&mut cpu, 0x85, &[]), 0x0100);
    assert_eq!(request(&mut cpu, 0x85, &[]), 0x0100);
    assert_eq!(request(&mut cpu, 0x88, &[]), 0x810C);
}

#[test]
fn the_driver_takes_requests_through_its_own_entries() {
    let (mut cpu, _) = disc("driver");
    // CALL FAR strategy, CALL FAR interrupt, HLT, at 2000:0000, with the
    // request (IOCTL input 00h: header address) at ES:BX.
    let code = [0x9A, 0x80, 0x11, 0x00, 0xF0, 0x9A, 0x84, 0x11, 0x00, 0xF0, 0xF4];
    for (i, &b) in code.iter().enumerate() {
        cpu.bus.write_8(0x20000 + i, b);
    }
    for i in 0..0x20 {
        cpu.bus.write_8(HEADER + i, 0);
    }
    cpu.bus.write_8(HEADER + 2, 0x03);
    cpu.bus.write_16(HEADER + 0x0E, 0x0000);
    cpu.bus.write_16(HEADER + 0x10, 0x3100);
    cpu.bus.write_8(BUFFER, 0x00);
    cpu.set_cs(0x2000);
    cpu.set_ip(0);
    cpu.set_ss(0x4000);
    cpu.set_sp(0x1000);
    cpu.set_es(0x3000);
    cpu.set_bx(0);
    for _ in 0..20 {
        if cpu.cs() == 0x2000 && cpu.ip() == 10 {
            break;
        }
        cpu.step();
    }
    assert_eq!((cpu.cs(), cpu.ip()), (0x2000, 10));
    assert_eq!(cpu.bus.read_16(HEADER + 3), 0x0100);
    assert_eq!((cpu.bus.read_16(BUFFER + 1), cpu.bus.read_16(BUFFER + 3)), (0x1160, 0xF000));
}
