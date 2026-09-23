mod cdimage;

use cdimage::{audio_sample, file, iso, mixed_disc};
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
    // is installed: see extender_support_tests)
    for ax in [0x1600u16, 0x1687, 0x1100] {
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
fn disc(name: &str) -> (Cpu, u32) {
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
