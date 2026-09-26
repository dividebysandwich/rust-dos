//! CD images mounted as CD-ROM drives: ISO, BIN/CUE and bare BIN images,
//! read through the DOS file calls, and mounted with MOUNT and IMGMOUNT.

mod cdimage;

use cdimage::{IsoFile, file, iso, mixed_disc, mode1_2352, scratch};
use rust_dos::command::CommandDispatcher;
use rust_dos::cpu::Cpu;
use rust_dos::disk::{DriveKind, MountOptions};
use std::fs;
use std::path::Path;

const PSP: u16 = 0x1000;
const D: u8 = 3;

/// Bytes that differ at every offset, to catch reads from the wrong place.
fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 7 + i / 256) as u8).collect()
}

fn files(big: &[u8]) -> Vec<IsoFile<'_>> {
    vec![
        file("HELLO.COM", &[0xB4, 0x4C, 0xCD, 0x21]),
        file("DATA\\BIG.DAT", big),
        file("DATA\\LEVELS\\L1.LVL", b"level one"),
        file("LONGNAME.TEXT", b"long"),
        IsoFile { path: "SECRET.DAT", data: b"hidden", hidden: true },
    ]
}

fn machine(name: &str) -> (Cpu, std::path::PathBuf) {
    let dir = scratch(name);
    fs::create_dir_all(dir.join("c")).unwrap();
    (Cpu::new(dir.join("c")), dir)
}

fn mount(cpu: &mut Cpu, image: &Path) {
    cpu.bus.mount_drive(D, image, MountOptions::default(), false).unwrap();
}

fn names(cpu: &Cpu, spec: &str, attr: u16) -> Vec<String> {
    cpu.bus.disk.list_directory(spec, attr).unwrap().into_iter().map(|e| e.filename).collect()
}

/// Mount checks shared by every kind of image.
fn check_files(cpu: &mut Cpu, big: &[u8]) {
    let info = cpu.bus.disk.drive_info(D).unwrap();
    assert_eq!((info.kind, info.label.as_str(), info.read_only), (DriveKind::CdRom, "TESTDISC", true));

    assert_eq!(names(cpu, "D:\\*.*", 0x10), ["DATA", "HELLO.COM", "LONGNA~1.TEX"]);
    assert_eq!(names(cpu, "D:\\*.DAT", 0x02), ["SECRET.DAT"]);
    assert_eq!(names(cpu, "D:\\DATA\\*.*", 0x10), ["..", ".", "BIG.DAT", "LEVELS"]);
    let entry = cpu.bus.disk.find_directory_entry("D:\\DATA\\BIG.DAT", 0, 0).unwrap();
    assert_eq!((entry.size as usize, entry.attr), (big.len(), 0x21));
    // 31 October 1995, 12:00.
    assert_eq!((entry.dos_date, entry.dos_time), ((15 << 9) | (10 << 5) | 31, 12 << 11));
    assert_eq!(cpu.bus.disk.get_file_attribute("D:\\SECRET.DAT"), Ok(0x23));

    // Reads across sector boundaries, seeks, and the end of the file.
    let h = cpu.bus.disk.open_file("D:\\DATA\\BIG.DAT", 0x02, PSP).unwrap();
    assert_eq!(cpu.bus.disk.read_file(h, 3000).unwrap(), big[..3000]);
    assert_eq!(cpu.bus.disk.seek_file(h, 4000, 0), Ok(4000));
    assert_eq!(cpu.bus.disk.read_file(h, 100_000).unwrap(), big[4000..]);
    assert_eq!(cpu.bus.disk.read_file(h, 10).unwrap(), Vec::<u8>::new());
    assert_eq!(cpu.bus.disk.seek_file(h, -10, 2), Ok(big.len() as u64 - 10));
    assert_eq!(cpu.bus.disk.read_file(h, 10).unwrap(), big[big.len() - 10..]);
    assert_eq!(cpu.bus.disk.write_file(h, b"x"), Err(0x05));
    assert_eq!(cpu.bus.disk.file_time(h), Ok((12 << 11, (15 << 9) | (10 << 5) | 31)));
    cpu.bus.disk.close_file(h);
    assert_eq!(cpu.bus.disk.open_file("D:\\NOPE.TXT", 0, PSP), Err(0x02));
    assert_eq!(cpu.bus.disk.open_file("D:\\NOPE\\X.TXT", 0, PSP), Err(0x03));
    assert_eq!(cpu.bus.disk.open_file("D:\\LONGNAME.TEXT", 0, PSP).map(|_| ()), Err(0x02));
    assert!(cpu.bus.disk.open_file("D:\\LONGNA~1.TEX", 0, PSP).is_ok());

    // Directories, and running a program from the disc.
    assert!(cpu.bus.disk.set_current_directory("D:\\DATA\\LEVELS"));
    assert!(cpu.bus.disk.is_file("D:L1.LVL"));
    assert!(cpu.load_executable("D:\\HELLO.COM", None));
}

#[test]
fn iso_images_mount_as_cd_drives() {
    let (mut cpu, dir) = machine("iso");
    let big = pattern(5000);
    fs::write(dir.join("game.iso"), iso("TESTDISC", &files(&big))).unwrap();
    mount(&mut cpu, &dir.join("game.iso"));
    check_files(&mut cpu, &big);
    let info = cpu.bus.disk.drive_info(D).unwrap();
    assert_eq!(info.root, None);
    assert_eq!(info.image.as_deref(), Some(dir.join("game.iso").as_path()));
}

#[test]
fn bin_cue_images_with_audio_tracks_mount() {
    let (mut cpu, dir) = machine("cue");
    let big = pattern(5000);
    let cue = mixed_disc(&dir, &iso("TESTDISC", &files(&big)), 10);
    mount(&mut cpu, &cue);
    check_files(&mut cpu, &big);
    let image = cpu.bus.disk.cd_image(D).unwrap();
    assert_eq!(image.tracks().len(), 2);
    assert!(image.tracks()[1].is_audio());
}

#[test]
fn bare_raw_images_are_recognised() {
    let (mut cpu, dir) = machine("bare");
    let big = pattern(6000);
    fs::write(dir.join("GAME.BIN"), mode1_2352(&iso("TESTDISC", &files(&big)))).unwrap();
    mount(&mut cpu, &dir.join("GAME.BIN"));
    check_files(&mut cpu, &big);
}

#[test]
fn only_cd_images_mount() {
    let (mut cpu, dir) = machine("refused");
    fs::write(dir.join("junk.img"), vec![0u8; 100_000]).unwrap();
    assert!(cpu.bus.mount_drive(D, &dir.join("junk.img"), MountOptions::default(), false).is_err());
    fs::write(dir.join("game.iso"), iso("TESTDISC", &[])).unwrap();
    let floppy = MountOptions { kind: DriveKind::Floppy, ..Default::default() };
    assert!(cpu.bus.mount_drive(0, &dir.join("game.iso"), floppy, false).is_err());
    assert!(cpu.bus.mount_drive(2, &dir.join("game.iso"), MountOptions::default(), true).is_err());
}

fn run(cpu: &mut Cpu, line: &str) {
    let (command, args) = line.split_once(' ').unwrap_or((line, ""));
    assert!(CommandDispatcher::new().dispatch(cpu, command, args));
}

#[test]
fn imgmount_finds_images_by_their_dos_path() {
    let (mut cpu, dir) = machine("imgmount");
    fs::create_dir_all(dir.join("c/TIECD/cd")).unwrap();
    let big = pattern(5000);
    mixed_disc(&dir.join("c/TIECD/cd"), &iso("TESTDISC", &files(&big)), 1);
    fs::rename(dir.join("c/TIECD/cd/GAME.CUE"), dir.join("c/TIECD/cd/Game Disc.cue")).unwrap();

    // As in a DOSBox batch file: a DOS path in 8.3 names.
    run(&mut cpu, "IMGMOUNT d C:\\TIECD\\CD\\GAMEDI~1.CUE -t cdrom");
    let info = cpu.bus.disk.drive_info(D).expect("mounted");
    assert!(info.image.unwrap().ends_with("Game Disc.cue"));
    run(&mut cpu, "IMGMOUNT -u d");
    assert!(!cpu.bus.disk.is_mounted(D));

    // Relative to the current directory, and refusing hard disk images.
    assert!(cpu.bus.disk.set_current_directory("TIECD\\CD"));
    run(&mut cpu, "IMGMOUNT E GAME.BIN -label MYDISC");
    assert_eq!(cpu.bus.disk.volume_label(4).as_deref(), Some("MYDISC"));
    run(&mut cpu, "IMGMOUNT F GAME.BIN -t hdd");
    assert!(!cpu.bus.disk.is_mounted(5));
}

#[test]
fn mount_takes_images_the_way_dosbox_staging_does() {
    let (mut cpu, dir) = machine("mount_images");
    fs::create_dir_all(dir.join("c/CDS")).unwrap();
    let big = pattern(5000);
    mixed_disc(&dir.join("c/CDS"), &iso("TESTDISC", &files(&big)), 1);
    fs::copy(dir.join("c/CDS/GAME.CUE"), dir.join("c/CDS/DISC2.CUE")).unwrap();

    // The options anywhere, and the image by its DOS path.
    run(&mut cpu, "MOUNT -t cdrom d C:\\CDS\\GAME.CUE -label MYDISC");
    assert_eq!(cpu.bus.disk.volume_label(D).as_deref(), Some("MYDISC"));
    run(&mut cpu, "MOUNT d -u");
    assert!(!cpu.bus.disk.is_mounted(D));

    // A wildcard makes a list, in natural order.
    run(&mut cpu, "MOUNT D C:\\CDS\\*.CUE");
    let info = cpu.bus.disk.drive_info(D).expect("mounted");
    assert_eq!(info.images.len(), 2);
    assert!(info.images[0].ends_with("DISC2.CUE"), "{:?}", info.images);
}
