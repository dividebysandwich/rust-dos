use super::*;
use std::fs;

/// PSP that owns the files the tests open.
const PSP: u16 = 0x1000;

/// A fresh scratch directory under target/ for one test.
fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from("target/test_disk_unit").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn cdrom() -> MountOptions {
    MountOptions {
        kind: DriveKind::CdRom,
        ..Default::default()
    }
}

#[test]
fn parse_drive_prefix_handles_letters_and_non_ascii() {
    assert_eq!(parse_drive_prefix("d:\\X"), (Some(3), "\\X"));
    assert_eq!(parse_drive_prefix("FILE.TXT"), (None, "FILE.TXT"));
    assert_eq!(parse_drive_prefix("\u{FFFD}:X"), (None, "\u{FFFD}:X"));
    assert_eq!(parse_drive_prefix("é"), (None, "é"));
    assert_eq!(parse_drive_prefix(""), (None, ""));
}

#[test]
fn long_names_get_dosbox_short_names() {
    let names = [
        "Day Of The Tentacle.BIN",
        "Day Of The Tentacle.cue",
        "readme.txt",
        "Cargo.toml",
        "two.dots.txt",
        "ABCDEF~1.TXT",
        "abcdefghij.txt",
    ];
    assert_eq!(
        short_names(&names),
        [
            "DAYOFT~1.BIN",
            "DAYOFT~2.CUE",
            "README.TXT",
            "CARGO~1.TOM",
            "TWODOT~1.TXT",
            "ABCDEF~1.TXT",
            "ABCDEF~2.TXT",
        ]
    );
}

#[test]
fn host_files_are_found_by_long_and_short_name() {
    let base = scratch("short_names");
    fs::create_dir_all(base.join("CD")).unwrap();
    fs::write(base.join("CD/Day Of The Tentacle.BIN"), b"bin").unwrap();
    fs::write(base.join("CD/Day Of The Tentacle.cue"), b"cue").unwrap();
    let disk = DiskController::new(base.clone());

    let cue = disk.resolve_path(r"C:\CD\DAYOFT~2.CUE").unwrap();
    assert!(cue.ends_with("CD/Day Of The Tentacle.cue"));
    let bin = disk.resolve_path(r"\cd\day of the tentacle.bin").unwrap();
    assert!(bin.ends_with("CD/Day Of The Tentacle.BIN"));
    let names: Vec<String> = disk
        .list_directory(r"C:\CD\*.*", 0)
        .unwrap()
        .into_iter()
        .map(|e| e.filename)
        .collect();
    assert_eq!(names, ["..", ".", "DAYOFT~1.BIN", "DAYOFT~2.CUE"]);
}

#[test]
fn each_drive_has_its_own_current_directory() {
    let base = scratch("cwd");
    fs::create_dir_all(base.join("c/GAMES")).unwrap();
    fs::create_dir_all(base.join("d/DATA")).unwrap();
    let mut disk = DiskController::new(base.join("c"));
    disk.mount(3, &base.join("d"), MountOptions::default(), false)
        .unwrap();

    assert!(disk.set_current_directory("GAMES"));
    assert!(disk.set_current_directory("D:\\DATA"));
    assert_eq!(disk.get_current_drive(), DRIVE_C);
    assert_eq!(disk.get_current_directory(), "GAMES");
    assert_eq!(disk.get_current_directory_of(3).unwrap(), "DATA");

    fs::write(base.join("d/DATA/f.txt"), b"x").unwrap();
    let resolved = disk.resolve_path("D:F.TXT").unwrap();
    assert!(resolved.ends_with("DATA/f.txt"));
    assert_eq!(disk.qualify_directory("D:*.*").as_deref(), Some("D:\\DATA"));
    assert_eq!(disk.qualify_directory("\\*.*").as_deref(), Some("C:\\"));
}

#[test]
fn closing_a_program_resets_every_drive_to_its_root_and_c() {
    let base = scratch("reset_cwd");
    fs::create_dir_all(base.join("c/GAMES")).unwrap();
    fs::create_dir_all(base.join("d/DATA")).unwrap();
    let mut disk = DiskController::new(base.join("c"));
    disk.mount(3, &base.join("d"), MountOptions::default(), false).unwrap();
    assert!(disk.set_current_directory("GAMES"));
    assert!(disk.set_current_directory("D:\\DATA"));
    disk.set_current_drive(3);

    disk.reset_current_directories();

    assert_eq!(disk.get_current_drive(), DRIVE_C);
    assert_eq!(disk.get_current_directory_of(DRIVE_C).as_deref(), Some(""));
    assert_eq!(disk.get_current_directory_of(3).as_deref(), Some(""));
}

#[test]
fn unmounted_and_reserved_drives() {
    let base = scratch("reserved");
    let mut disk = DiskController::new(base.clone());
    assert!(disk.resolve_path("E:\\X").is_none());
    assert_eq!(disk.set_current_drive(4), LASTDRIVE);
    assert_eq!(disk.get_current_drive(), DRIVE_C);
    assert!(
        disk.mount(DRIVE_Z, &base, MountOptions::default(), true)
            .is_err()
    );
    assert!(disk.unmount(DRIVE_C).is_err());
    assert!(disk.unmount(DRIVE_Z).is_err());
    assert!(disk.unmount(4).is_err());
    assert!(
        disk.mount(3, &base.join("missing"), MountOptions::default(), false)
            .is_err()
    );
}

#[test]
fn mkdir_with_drive_prefix_lands_on_that_drive() {
    let base = scratch("mkdir");
    fs::create_dir_all(base.join("c")).unwrap();
    fs::create_dir_all(base.join("d")).unwrap();
    let mut disk = DiskController::new(base.join("c"));
    disk.mount(3, &base.join("d"), MountOptions::default(), false)
        .unwrap();

    disk.create_directory("D:NEWDIR").unwrap();
    disk.create_directory("D:\\ROOTDIR").unwrap();
    assert!(base.join("d/NEWDIR").is_dir());
    assert!(base.join("d/ROOTDIR").is_dir());
    assert!(!base.join("c/D:NEWDIR").exists());

    // Error 0x10 only applies to the target drive's current directory.
    assert!(disk.set_current_directory("D:\\NEWDIR"));
    assert_eq!(disk.remove_directory("D:\\NEWDIR"), Err(0x10));
    fs::create_dir_all(base.join("c/NEWDIR")).unwrap();
    assert_eq!(disk.remove_directory("C:\\NEWDIR"), Ok(()));
}

#[test]
fn read_only_media_rejects_writes() {
    let base = scratch("readonly");
    fs::create_dir_all(base.join("c")).unwrap();
    fs::create_dir_all(base.join("cd/SUB")).unwrap();
    fs::write(base.join("cd/DATA.DAT"), b"cd data").unwrap();
    let mut disk = DiskController::new(base.join("c"));
    disk.mount(3, &base.join("cd"), cdrom(), false).unwrap();

    assert_eq!(disk.create_file("D:\\NEW.TXT", PSP), Err(0x05));
    assert_eq!(disk.open_file("D:\\DATA.DAT", 1, PSP), Err(0x05));
    assert_eq!(disk.create_directory("D:\\X"), Err(0x05));
    assert_eq!(disk.remove_directory("D:\\SUB"), Err(0x05));
    assert_eq!(disk.set_file_attribute("D:\\DATA.DAT", 0), Err(0x05));
    assert_eq!(disk.get_file_attribute("D:\\DATA.DAT"), Ok(0x21));

    // Mode 2 is downgraded: reads work, writes fail.
    let h = disk.open_file("D:\\DATA.DAT", 2, PSP).unwrap();
    assert_eq!(disk.read_file(h, 7).unwrap(), b"cd data");
    assert_eq!(disk.write_file(h, b"x"), Err(0x05));
    assert!(!base.join("cd/NEW.TXT").exists());
    assert_eq!(fs::read(base.join("cd/DATA.DAT")).unwrap(), b"cd data");

    let entries = disk.list_directory("D:\\*.*", 0x10).unwrap();
    assert!(entries.iter().all(|e| e.attr & 0x01 != 0));
}

#[test]
fn an_overlay_keeps_the_directory_as_it_was() {
    let base = scratch("overlay");
    fs::create_dir_all(base.join("c")).unwrap();
    fs::create_dir_all(base.join("game/saves")).unwrap();
    fs::write(base.join("game/GAME.CFG"), b"old").unwrap();
    fs::write(base.join("game/saves/slot1.sav"), b"one").unwrap();
    let mut disk = DiskController::new(base.join("c"));
    let opts = MountOptions { overlay: Some(base.join("upper")), ..MountOptions::default() };
    disk.mount(3, &base.join("game"), opts, false).unwrap();
    let info = disk.drive_info(3).unwrap();
    assert_eq!(info.root.as_deref(), Some(fs::canonicalize(base.join("game")).unwrap().as_path()));
    assert_eq!(info.overlay.as_deref(), Some(base.join("upper").as_path()));

    let h = disk.open_file(r"D:\GAME.CFG", 2, PSP).unwrap();
    assert_eq!(disk.write_file(h, b"new"), Ok(3));
    disk.close_file(h);
    let h = disk.create_file(r"D:\SAVES\SLOT2.SAV", PSP).unwrap();
    disk.write_file(h, b"two").unwrap();
    disk.close_file(h);
    disk.delete_file(r"D:\SAVES\SLOT1.SAV").unwrap();
    disk.create_directory(r"D:\NEW").unwrap();
    disk.rename_file(r"D:\SAVES\SLOT2.SAV", r"D:\NEW\SLOT2.SAV").unwrap();

    let h = disk.open_file(r"D:\GAME.CFG", 0, PSP).unwrap();
    assert_eq!(disk.read_file(h, 3).unwrap(), b"new");
    let names = |disk: &DiskController, spec: &str| -> Vec<String> {
        disk.list_directory(spec, 0x10).unwrap().into_iter().map(|e| e.filename).collect()
    };
    assert!(names(&disk, r"D:\SAVES\*.*").iter().all(|n| n.starts_with('.')));
    assert_eq!(names(&disk, r"D:\NEW\SLOT*.*"), ["SLOT2.SAV"]);
    // The directory as it was.
    assert_eq!(fs::read(base.join("game/GAME.CFG")).unwrap(), b"old");
    assert_eq!(fs::read(base.join("game/saves/slot1.sav")).unwrap(), b"one");
    assert_eq!(fs::read_dir(base.join("game")).unwrap().count(), 2);
    assert_eq!(fs::read(base.join("upper/GAME.CFG")).unwrap(), b"new");
    assert_eq!(fs::read(base.join("upper/NEW/SLOT2.SAV")).unwrap(), b"two");
}

#[test]
fn an_archive_is_a_drive_with_its_changes_beside_it() {
    let base = scratch("archive");
    fs::create_dir_all(base.join("c")).unwrap();
    let zip = crate::archive::zip::tests::zip(&[
        ("Game/GAME.EXE", b"MZ game", true),
        ("Game/SAVES/SLOT1.SAV", b"one", false),
    ]);
    fs::write(base.join("game.zip"), &zip).unwrap();
    let mut disk = DiskController::new(base.join("c"));

    // Without a folder for its changes, it is read-only.
    disk.mount(3, &base.join("game.zip"), MountOptions::default(), false).unwrap();
    assert!(disk.drive_info(3).unwrap().read_only);
    assert_eq!(disk.create_file(r"D:\NEW.TXT", PSP), Err(0x05));

    let opts = MountOptions { overlay: Some(base.join("saves")), ..MountOptions::default() };
    disk.mount(3, &base.join("game.zip"), opts, true).unwrap();
    let info = disk.drive_info(3).unwrap();
    assert!(!info.read_only);
    assert_eq!(info.root.as_deref(), Some(fs::canonicalize(base.join("game.zip")).unwrap().as_path()));
    let h = disk.open_file(r"D:\GAME.EXE", 0, PSP).unwrap();
    assert_eq!(disk.read_file(h, 16).unwrap(), b"MZ game");
    disk.close_file(h);
    let h = disk.open_file(r"D:\SAVES\SLOT1.SAV", 2, PSP).unwrap();
    disk.write_file(h, b"ONE").unwrap();
    disk.close_file(h);
    disk.delete_file(r"D:\GAME.EXE").unwrap();
    let names: Vec<String> = disk.list_directory(r"D:\*.*", 0x10).unwrap().into_iter().map(|e| e.filename).collect();
    assert_eq!(names, ["SAVES"]);
    assert_eq!(fs::read(base.join("saves/SAVES/SLOT1.SAV")).unwrap(), b"ONE");
    assert_eq!(fs::read(base.join("game.zip")).unwrap(), zip, "the archive as it was");
}

#[test]
fn a_patched_file_is_what_dos_reads_and_writes() {
    let base = scratch("archive_patch");
    fs::create_dir_all(base.join("c")).unwrap();
    fs::write(base.join("game.dosz"), crate::archive::zip::tests::zip(&[("GAME.EXE", b"MZ protected", false)])).unwrap();
    let mut patch = b"PATCH".to_vec();
    patch.extend([0, 0, 3, 0, 9]);
    patch.extend(b"cracked!!");
    patch.extend(b"EOF");
    fs::write(base.join("game.dosc"), crate::archive::zip::tests::zip(&[("GAME.EXE", &patch, false)])).unwrap();
    let mut disk = DiskController::new(base.join("c"));
    let opts = MountOptions { overlay: Some(base.join("saves")), ..MountOptions::default() };
    disk.mount(3, &base.join("game.dosz"), opts, false).unwrap();
    let h = disk.open_file(r"D:\GAME.EXE", 0, PSP).unwrap();
    assert_eq!(disk.read_file(h, 32).unwrap(), b"MZ cracked!!");
    disk.close_file(h);
    // Written to, it goes to the saves as it reads.
    let h = disk.open_file(r"D:\GAME.EXE", 2, PSP).unwrap();
    disk.write_file(h, b"MZ").unwrap();
    disk.close_file(h);
    assert_eq!(fs::read(base.join("saves/GAME.EXE")).unwrap(), b"MZ cracked!!");
}

#[test]
fn a_path_into_an_archive_mounts_what_is_there() {
    let base = scratch("archive_path");
    fs::create_dir_all(base.join("c")).unwrap();
    let zip = crate::archive::zip::tests::zip(&[
        ("Game/GAME.EXE", b"MZ game", false),
        ("Game/CD/DATA.DAT", b"cd data", false),
        ("Game/DISKS/disk1.img", &vec![0u8; 368_640], false),
        ("Game/DISKS/disk2.img", &vec![0u8; 368_640], false),
    ]);
    fs::write(base.join("game.zip"), &zip).unwrap();
    let mut disk = DiskController::new(base.join("c"));
    disk.mount(3, &base.join("game.zip/CD"), cdrom(), false).unwrap();
    let h = disk.open_file(r"D:\DATA.DAT", 0, PSP).unwrap();
    assert_eq!(disk.read_file(h, 16).unwrap(), b"cd data");
    assert!(disk.list_directory(r"D:\GAME.EXE", 0).unwrap().is_empty(), "only the folder");
    // In any case, as DOSBox configurations name them.
    disk.mount(5, &base.join("game.zip/cd"), MountOptions::default(), false).unwrap();
    let h = disk.open_file(r"F:\DATA.DAT", 0, PSP).unwrap();
    assert_eq!(disk.read_file(h, 16).unwrap(), b"cd data");
    let list = MountOptions { more_images: vec![base.join("game.zip/DISKS/disk2.img")], ..MountOptions::default() };
    disk.mount(numbered_drive(0), &base.join("game.zip/DISKS/DISK1.IMG"), list, false).unwrap();
    let archive = fs::canonicalize(base.join("game.zip")).unwrap();
    let info = disk.drive_info(numbered_drive(0)).unwrap();
    assert_eq!(info.image, Some(archive.join("DISKS/DISK1.IMG")));
    assert_eq!(info.images, [archive.join("DISKS/DISK1.IMG"), archive.join("DISKS/disk2.img")]);
    let elsewhere = MountOptions { more_images: vec![base.join("c/disk2.img")], ..MountOptions::default() };
    assert!(disk.mount(numbered_drive(1), &base.join("game.zip/DISKS/disk2.img"), elsewhere, false).is_err());
    assert!(disk.mount(4, &base.join("game.zip/NONE"), MountOptions::default(), false).is_err());
}

#[test]
fn a_cue_sheet_in_an_archive_mounts_its_disc() {
    let base = scratch("archive_cue");
    fs::create_dir_all(base.join("c")).unwrap();
    fs::create_dir_all(base.join("disc")).unwrap();
    fs::write(base.join("disc/DATA.DAT"), b"on the disc").unwrap();
    let folder = CdImage::from_folder(crate::cdrom::folder::build(&base.join("disc"), "GAMECD").unwrap(), &base.join("disc")).unwrap();
    let mut iso = Vec::new();
    for lba in 0..folder.leadout() {
        let mut sector = [0u8; crate::cdrom::DATA_SECTOR];
        folder.read_data(lba, &mut sector).unwrap();
        iso.extend_from_slice(&sector);
    }
    let cue = b"FILE \"GAME.ISO\" BINARY\r\n  TRACK 01 MODE1/2048\r\n    INDEX 01 00:00:00\r\n";
    let zip = crate::archive::zip::tests::zip(&[("Game/CD/GAME.CUE", cue, false), ("Game/CD/GAME.ISO", &iso, false)]);
    fs::write(base.join("game.zip"), &zip).unwrap();
    let mut disk = DiskController::new(base.join("c"));
    disk.mount(3, &base.join("game.zip/CD/GAME.CUE"), MountOptions::default(), false).unwrap();
    let info = disk.drive_info(3).unwrap();
    assert_eq!((info.kind, info.label.as_str()), (DriveKind::CdRom, "GAMECD"));
    let h = disk.open_file(r"D:\DATA.DAT", 0, PSP).unwrap();
    assert_eq!(disk.read_file(h, 32).unwrap(), b"on the disc");
}

#[test]
fn an_archive_of_a_disk_image_mounts_the_image() {
    let base = scratch("archive_image");
    fs::create_dir_all(base.join("c")).unwrap();
    let zip = crate::archive::zip::tests::zip(&[("booter.img", &vec![0u8; 368_640], true), ("README.TXT", b"hi", false)]);
    fs::write(base.join("booter.zip"), &zip).unwrap();
    let mut disk = DiskController::new(base.join("c"));
    let opts = MountOptions { overlay: Some(base.join("saves")), ..MountOptions::default() };
    disk.mount(numbered_drive(0), &base.join("booter.zip"), opts, false).unwrap();
    let info = disk.drive_info(numbered_drive(0)).unwrap();
    let archive = fs::canonicalize(base.join("booter.zip")).unwrap();
    assert_eq!(info.image, Some(archive.join("booter.img")));
    assert!(!info.read_only);
    // Its changes go to a delta file in the folder for them, made at
    // the first write; the image isn't copied out.
    assert_eq!(info.overlay, Some(base.join("saves/booter.img.rdelta")));
    assert!(!base.join("saves/booter.img.rdelta").exists());
    let image = disk.bios_image(numbered_drive(0)).unwrap();
    image.write(10, &[0xAB; 512]).unwrap();
    assert!(base.join("saves/booter.img.rdelta").exists());
    assert!(!base.join("saves/booter.img").exists());
    assert_eq!(fs::read(base.join("booter.zip")).unwrap(), zip);
    // Mounted again, the change is there.
    let opts = MountOptions { overlay: Some(base.join("saves")), ..MountOptions::default() };
    disk.mount(numbered_drive(0), &base.join("booter.zip"), opts, true).unwrap();
    let mut sector = [0u8; 512];
    disk.bios_image(numbered_drive(0)).unwrap().read(10, &mut sector).unwrap();
    assert_eq!(sector, [0xAB; 512]);
}

#[test]
fn an_archive_of_a_vhd_mounts_the_vhd() {
    let base = scratch("archive_vhd");
    fs::create_dir_all(base.join("c")).unwrap();
    let vhd = crate::vhd::make_dynamic(8 << 20, 2 << 20);
    let zip = crate::archive::zip::tests::zip(&[("STORE.VHD", &vhd, true)]);
    fs::write(base.join("store.dosz"), &zip).unwrap();
    assert_eq!(archive_hard_disk(&base.join("store.dosz")), Some("STORE.VHD".to_string()));
    let mut disk = DiskController::new(base.join("c"));
    let opts = MountOptions { overlay: Some(base.join("saves")), ..MountOptions::default() };
    disk.mount(numbered_drive(3), &base.join("store.dosz"), opts, false).unwrap();
    let image = disk.bios_image(numbered_drive(3)).unwrap();
    assert_eq!(image.sectors(), (8 << 20) / 512);
    image.write(5000, &[0xCD; 512]).unwrap();
    assert!(base.join("saves/STORE.VHD.rdelta").exists());
    assert_eq!(fs::read(base.join("store.dosz")).unwrap(), zip);
    let mut sector = [0u8; 512];
    image.read(5000, &mut sector).unwrap();
    assert_eq!(sector, [0xCD; 512]);
}

/// An image copied out whole into the folder for an archive's changes,
/// as before delta files, is the one written to.
#[test]
fn an_image_copied_out_before_stays_the_one() {
    let base = scratch("archive_image_copied");
    fs::create_dir_all(base.join("c")).unwrap();
    fs::create_dir_all(base.join("saves")).unwrap();
    let zip = crate::archive::zip::tests::zip(&[("booter.img", &vec![0u8; 368_640], true)]);
    fs::write(base.join("booter.zip"), &zip).unwrap();
    fs::write(base.join("saves/booter.img"), vec![0x11u8; 368_640]).unwrap();
    let mut disk = DiskController::new(base.join("c"));
    let opts = MountOptions { overlay: Some(base.join("saves")), ..MountOptions::default() };
    disk.mount(numbered_drive(0), &base.join("booter.zip"), opts, false).unwrap();
    let image = disk.bios_image(numbered_drive(0)).unwrap();
    assert_eq!(image.delta_path(), None);
    image.write(0, &[0x22; 512]).unwrap();
    assert_eq!(fs::read(base.join("saves/booter.img")).unwrap()[..2], [0x22, 0x22]);
}

/// With -overlay, a hard disk image is left as it is: one image is
/// under each game's changes.
#[test]
fn an_image_under_an_overlay_keeps_as_it_is() {
    let base = scratch("image_overlay");
    fs::create_dir_all(base.join("c")).unwrap();
    let blank = DiskImage::blank_hard_disk("base.img", 8 << 20, Some("BASE")).unwrap();
    blank.copy_to(&base.join("base.img")).unwrap();
    let before = fs::read(base.join("base.img")).unwrap();
    let mut disk = DiskController::new(base.join("c"));
    for (game, text) in [("one", b"first game".as_slice()), ("two", b"second game")] {
        let opts = MountOptions { overlay: Some(base.join(game)), ..MountOptions::default() };
        disk.mount(3, &base.join("base.img"), opts, true).unwrap();
        let h = disk.create_file(r"D:\GAME.TXT", PSP).unwrap();
        disk.write_file(h, text).unwrap();
        disk.close_file(h);
        assert!(base.join(game).join("base.img.rdelta").exists());
    }
    assert_eq!(fs::read(base.join("base.img")).unwrap(), before, "the image as it was");
    for (game, text) in [("one", b"first game".as_slice()), ("two", b"second game")] {
        let opts = MountOptions { overlay: Some(base.join(game)), ..MountOptions::default() };
        disk.mount(3, &base.join("base.img"), opts, true).unwrap();
        let h = disk.open_file(r"D:\GAME.TXT", 0, PSP).unwrap();
        assert_eq!(disk.read_file(h, 32).unwrap(), text);
        disk.close_file(h);
    }
    disk.mount(3, &base.join("base.img"), MountOptions::default(), true).unwrap();
    assert!(disk.open_file(r"D:\GAME.TXT", 0, PSP).is_err());
}

#[test]
fn names_padded_with_spaces_are_found_and_opened() {
    let base = scratch("pattern_padding");
    fs::write(base.join("LH66.RLE"), b"x").unwrap();
    let mut disk = DiskController::new(base);
    for spec in ["lh66.rle    ", "LH66    .RLE", "LH66.RLEX", "C:\\LH66.RLE  "] {
        let found = disk.list_directory(spec, 0).unwrap();
        assert_eq!(found.len(), 1, "{spec:?}");
        assert_eq!(found[0].filename, "LH66.RLE");
    }
    assert!(disk.list_directory("LH66.R  ", 0).unwrap().is_empty());
    assert!(disk.is_file("lh66.rle    "));
    let h = disk.open_file("C:\\LH66.RLE  ", 0, PSP).unwrap();
    assert_eq!(disk.read_file(h, 1).unwrap(), b"x");
}

#[test]
fn opening_a_missing_file_does_not_create_it() {
    let base = scratch("open_missing");
    fs::create_dir_all(base.join("c")).unwrap();
    let mut disk = DiskController::new(base.join("c"));
    for mode in [0, 1, 2] {
        assert_eq!(disk.open_file("C:\\PROBE.PAT", mode, PSP), Err(0x02));
    }
    assert!(!base.join("c/PROBE.PAT").exists());
    assert!(disk.create_file("C:\\PROBE.PAT", PSP).is_ok());
    assert!(base.join("c/PROBE.PAT").exists());
}

#[test]
fn unmount_and_remount_close_only_their_own_files() {
    let base = scratch("handles");
    for d in ["c", "c2", "d"] {
        fs::create_dir_all(base.join(d)).unwrap();
        fs::write(base.join(d).join("F.TXT"), b"1").unwrap();
    }
    let mut disk = DiskController::new(base.join("c"));
    disk.mount(3, &base.join("d"), MountOptions::default(), false)
        .unwrap();
    let hc = disk.open_file("C:\\F.TXT", 0, PSP).unwrap();
    let hd = disk.open_file("D:\\F.TXT", 0, PSP).unwrap();
    assert_eq!(disk.handle_drive(hd), Some(3));

    disk.mount(DRIVE_C, &base.join("c2"), MountOptions::default(), true)
        .unwrap();
    assert!(disk.read_file(hc, 1).is_err());
    assert!(disk.read_file(hd, 1).is_ok());

    disk.set_current_drive(3);
    disk.unmount(3).unwrap();
    assert!(disk.read_file(hd, 1).is_err());
    assert_eq!(disk.get_current_drive(), DRIVE_C);
}

#[test]
fn file_table_entries_are_reused_and_closed_with_their_last_reference() {
    let base = scratch("handle_reuse");
    fs::write(base.join("F.TXT"), b"1").unwrap();
    let mut disk = DiskController::new(base);
    let first = disk.open_file("F.TXT", 0, PSP).unwrap();
    assert_eq!(first, FIRST_FILE);
    let a = disk.open_file("F.TXT", 0, PSP).unwrap();
    // A second handle for it: the file stays open until both close.
    assert!(disk.add_ref(a));
    assert!(disk.close_file(a));
    assert_eq!(disk.read_file(a, 1).unwrap(), b"1");
    assert!(disk.close_file(a));
    assert!(!disk.close_file(a));
    assert_eq!(disk.open_file("Z:\\COMMAND.COM", 0, PSP), Ok(a));
    assert_eq!(disk.handle_drive(a), Some(DRIVE_Z));

    // The standard devices stay open, whatever closes them.
    assert!(disk.close_file(SFT_CON));
    assert_eq!(disk.handle_device(SFT_CON), Some(CharDevice::Con));

    // A process's FCB files close when it ends; its other files are
    // closed through its handles.
    let child = 0x2000;
    let f = disk.open_file("F.TXT", 0, child).unwrap();
    disk.set_fcb(f);
    assert!(!disk.inheritable(f));
    disk.close_fcb_files(child);
    assert!(!disk.is_open(f));
    assert_eq!(disk.read_file(first, 1).unwrap(), b"1");
    let entry = disk.sft_entry(first).unwrap();
    assert_eq!((&entry.name, entry.refs, entry.size, entry.owner), (b"F       TXT", 1, 1, PSP));
}

#[test]
fn floppy_free_space_tracks_usage() {
    let base = scratch("floppy");
    fs::create_dir_all(base.join("c")).unwrap();
    fs::create_dir_all(base.join("a")).unwrap();
    let mut disk = DiskController::new(base.join("c"));
    let floppy = MountOptions {
        kind: DriveKind::Floppy,
        ..Default::default()
    };
    disk.mount(0, &base.join("a"), floppy, false).unwrap();

    let (_, empty_free, _, total) = disk.get_disk_free_space(1).unwrap();
    assert_eq!(empty_free, total);
    fs::write(base.join("a/SAVE.DAT"), vec![0u8; 1024]).unwrap();
    let (spc, free, bps, _) = disk.get_disk_free_space(1).unwrap();
    assert_eq!((spc, bps), (1, 512));
    assert_eq!(free, total - 2);
    assert_eq!(disk.get_disk_free_space(5), Err(0x0F));
}

#[test]
fn a_and_b_are_always_floppies() {
    let base = scratch("floppy_letters");
    for d in ["c", "a", "b", "e"] {
        fs::create_dir_all(base.join(d)).unwrap();
    }
    fs::write(base.join("game.iso"), b"").unwrap();
    let mut disk = DiskController::new(base.join("c"));
    assert_eq!(disk.floppy_units(), 0);

    // B: alone makes two units, the first one empty.
    disk.mount(1, &base.join("b"), MountOptions::default(), false).unwrap();
    assert_eq!(disk.drive_kind(1), Some(DriveKind::Floppy));
    assert_eq!(disk.floppy_units(), 2);
    let hdd = MountOptions { kind: DriveKind::HardDisk, read_only: true, ..Default::default() };
    disk.mount(0, &base.join("a"), hdd.clone(), false).unwrap();
    let info = disk.drive_info(0).unwrap();
    assert_eq!((info.kind, info.read_only), (DriveKind::Floppy, true));
    // The mount stays as asked for, for the configuration file.
    assert_eq!(info.mount.unwrap().opts, hdd);
    disk.unmount(1).unwrap();
    assert_eq!(disk.floppy_units(), 1);

    assert!(disk.mount(1, &base.join("b"), cdrom(), false).unwrap_err().contains("floppy"));
    assert!(disk.mount(1, &base.join("game.iso"), MountOptions::default(), false).is_err());
    assert!(!disk.is_mounted(1));

    // Other letters keep the type they are given.
    disk.mount(4, &base.join("e"), MountOptions::default(), false).unwrap();
    assert_eq!(disk.drive_kind(4), Some(DriveKind::HardDisk));
}

#[test]
fn floppies_have_a_1_44_mb_diskette_layout() {
    let floppy = DriveKind::Floppy.layout();
    assert_eq!((floppy.sectors_per_fat, floppy.first_dir_sector(), floppy.first_data_sector()), (9, 19, 33));
    assert_eq!((floppy.total_sectors(), floppy.cylinders(), floppy.fs_type()), (2880, 80, b"FAT12   "));
    // The BPB of a DOS-formatted 1.44 MB diskette.
    assert_eq!(
        floppy.bpb()[..0x15],
        [0x00, 0x02, 0x01, 0x01, 0x00, 0x02, 0xE0, 0x00, 0x40, 0x0B, 0xF0, 0x09, 0x00, 0x12, 0x00, 0x02, 0x00, 0, 0, 0, 0]
    );
    assert_eq!(floppy.bpb()[0x15..], [0; 10]);

    let hdd = DriveKind::HardDisk.layout();
    assert_eq!((hdd.sectors_per_fat, hdd.fs_type()), (79, b"FAT16   "));
    assert_eq!(hdd.total_sectors(), 191 + 8 * 20000);
    let bpb = hdd.bpb();
    assert_eq!((bpb[0x08], bpb[0x09], bpb[0x0A]), (0, 0, 0xF8));
    assert_eq!(u32::from_le_bytes(bpb[0x15..0x19].try_into().unwrap()), hdd.total_sectors());
}

#[test]
fn disks_mounted_by_number_are_the_bioses_alone() {
    let base = scratch("numbered");
    // A blank hard disk, and floppies without a file system.
    fs::write(base.join("blank.img"), vec![0u8; 16 * 63 * 512 * 4]).unwrap();
    fs::write(base.join("booter1.img"), vec![0u8; 368_640]).unwrap();
    fs::write(base.join("booter2.img"), vec![0u8; 368_640]).unwrap();
    fs::create_dir_all(base.join("dir")).unwrap();
    let mut disk = DiskController::new(base.join("c"));
    let image = |name: &str| base.join(name);
    let list = MountOptions { more_images: vec![image("booter2.img")], ..MountOptions::default() };

    // Lettered drives need a file system; numbered ones take the sectors.
    assert!(disk.mount(3, &image("blank.img"), MountOptions::default(), false).is_err());
    disk.mount(numbered_drive(2), &image("blank.img"), MountOptions::default(), false).unwrap();
    disk.mount(numbered_drive(0), &image("booter1.img"), list, false).unwrap();
    let hdd = disk.bios_image(numbered_drive(2)).unwrap();
    assert_eq!((hdd.is_floppy(), hdd.geometry().heads, hdd.geometry().cylinders), (false, 16, 4));
    assert_eq!(disk.drive_kind(numbered_drive(0)), Some(DriveKind::Floppy));
    assert!(disk.bios_image(numbered_drive(0)).unwrap().is_floppy());

    // DOS doesn't have them; they take images only, and each once.
    assert_eq!(disk.mounted_drives().len(), 2, "C: and Z:");
    assert_eq!(disk.numbered_drives().iter().map(DriveInfo::name).collect::<Vec<_>>(), ["0", "2"]);
    assert!(disk.mount(numbered_drive(3), &base.join("dir"), MountOptions::default(), false).is_err());
    let twice = disk.mount(numbered_drive(3), &image("blank.img"), MountOptions::default(), false);
    assert!(twice.unwrap_err().contains("already mounted as 2"));
    assert!(disk.swap_image(numbered_drive(0)).unwrap().unwrap().starts_with("Drive 0 disk 2 of 2"));
    // Disk control picks any of them, and adds to and takes from them.
    fs::write(base.join("booter3.img"), vec![0u8; 368_640]).unwrap();
    disk.add_image(numbered_drive(0), &image("booter3.img")).unwrap();
    assert!(disk.add_image(numbered_drive(0), &image("booter3.img")).is_err(), "each image once");
    assert_eq!(disk.images(numbered_drive(0)).map(|(list, i)| (list.len(), i)), Some((3, 1)));
    assert!(disk.select_image(numbered_drive(0), 2).unwrap().unwrap().starts_with("Drive 0 disk 3 of 3"));
    assert_eq!(disk.select_image(numbered_drive(0), 2), Ok(None));
    assert!(disk.select_image(numbered_drive(0), 3).is_err());
    assert!(disk.remove_image(numbered_drive(0), 2).is_err(), "it is in");
    disk.remove_image(numbered_drive(0), 0).unwrap();
    assert_eq!(disk.images(numbered_drive(0)).map(|(list, i)| (list.len(), i)), Some((2, 1)));
    assert!(disk.images(DRIVE_C).is_none());

    // Floppy unit 00h is disk 0 before A:; hard disk 80h is disk 2,
    // and the hard disk drives follow in the units left.
    assert_eq!((disk.floppy_units(), disk.floppy_unit(0), disk.floppy_unit(1)), (1, numbered_drive(0), 1));
    assert_eq!(disk.hard_disk_units(|_| true), [numbered_drive(2), DRIVE_C]);
    assert_eq!(disk.hard_disk_units(|d| disk.bios_image(d).is_some()), [numbered_drive(2)]);
    disk.unmount(numbered_drive(2)).unwrap();
    disk.mount(numbered_drive(3), &image("blank.img"), MountOptions::default(), false).unwrap();
    assert_eq!(disk.hard_disk_units(|_| true), [DRIVE_C, numbered_drive(3)]);
    assert_eq!(disk.hard_disk_units(|d| disk.bios_image(d).is_some()), [numbered_drive(3)]);
}

#[test]
fn volume_label_comes_from_the_searched_drive() {
    let base = scratch("label");
    fs::create_dir_all(base.join("c")).unwrap();
    fs::create_dir_all(base.join("d")).unwrap();
    let mut disk = DiskController::new(base.join("c"));
    let opts = MountOptions {
        kind: DriveKind::CdRom,
        label: Some("gamecd_disk1".to_string()),
        read_only: false,
        ..Default::default()
    };
    disk.mount(3, &base.join("d"), opts, false).unwrap();

    let c = disk.find_directory_entry("*.*", 0, 0x08).unwrap();
    assert_eq!((c.filename.as_str(), c.attr), ("RUSTDOS", 0x08));
    let d = disk.find_directory_entry("D:\\*.*", 0, 0x08).unwrap();
    assert_eq!(d.filename, "GAMECD_D.ISK");
    assert_eq!(disk.volume_label(3).unwrap(), "GAMECD_DISK");
}

#[test]
fn creating_a_file_that_exists_empties_it_once_the_handle_is_used() {
    let base = scratch("create_truncates");
    fs::write(base.join("SAVE.DAT"), b"a long old save").unwrap();
    fs::write(base.join("KEEP.DAT"), b"kept").unwrap();
    fs::write(base.join("EMPTY.DAT"), b"old").unwrap();
    let mut disk = DiskController::new(base.clone());

    // Create, write something shorter, close: only the new bytes.
    let h = disk.create_file("C:\\SAVE.DAT", PSP).unwrap();
    assert_eq!(disk.seek_file(h, 0, 2), Ok(0));
    assert_eq!(disk.write_file(h, b"new"), Ok(3));
    disk.close_file(h);
    assert_eq!(fs::read(base.join("SAVE.DAT")).unwrap(), b"new");
    // Closed at once: empty.
    let h = disk.create_file("C:\\EMPTY.DAT", PSP).unwrap();
    disk.close_file(h);
    assert_eq!(fs::read(base.join("EMPTY.DAT")).unwrap(), b"");
    // Until the program uses the handle, the old bytes are kept, so a
    // crash between the create and the write loses nothing.
    let h = disk.create_file("C:\\KEEP.DAT", PSP).unwrap();
    assert_eq!(fs::read(base.join("KEEP.DAT")).unwrap(), b"kept");
    assert_eq!(disk.read_file(h, 10), Ok(Vec::new()));
    assert_eq!(fs::read(base.join("KEEP.DAT")).unwrap(), b"");
    disk.close_file(h);
    // Opened again by name, the file is empty, and stays as the second
    // handle writes it.
    fs::write(base.join("KEEP.DAT"), b"kept").unwrap();
    let created = disk.create_file("C:\\KEEP.DAT", PSP).unwrap();
    let again = disk.open_file("C:\\KEEP.DAT", 0x02, PSP).unwrap();
    assert_eq!(disk.write_file(again, b"xy"), Ok(2));
    disk.close_file(again);
    disk.close_file(created);
    assert_eq!(fs::read(base.join("KEEP.DAT")).unwrap(), b"xy");
    // A create the program got no handle for leaves the file alone.
    let h = disk.create_file("C:\\KEEP.DAT", PSP).unwrap();
    disk.close_unused(h);
    assert_eq!(fs::read(base.join("KEEP.DAT")).unwrap(), b"xy");

    // The same on a disk image.
    let blank = DiskImage::from_memory("F.IMA", MemoryImage::new(1_474_560), true, None, false).unwrap();
    fat::format(&blank, 0, 2880, None).unwrap();
    let bytes = blank.memory().unwrap().clone();
    disk.mount_memory_image(0, "DISK1.IMA", bytes, MountOptions::default()).unwrap();
    let h = disk.create_file("A:\\SAVE.DAT", PSP).unwrap();
    disk.write_file(h, b"a long old save").unwrap();
    disk.close_file(h);
    let h = disk.create_file("A:\\SAVE.DAT", PSP).unwrap();
    assert_eq!(disk.write_file(h, b"new"), Ok(3));
    disk.close_file(h);
    assert!(matches!(disk.file_data("A:\\SAVE.DAT").map(|f| f.read()), Some(Ok(d)) if &d[..] == b"new"));

    // Deleted or renamed on the image before the handle is used: the
    // file is emptied first, and closing the handle later leaves a file
    // made in the freed entry alone.
    let h = disk.create_file("A:\\SAVE.DAT", PSP).unwrap();
    disk.delete_file("A:\\SAVE.DAT").unwrap();
    let other = disk.create_file("A:\\OTHER.DAT", PSP).unwrap();
    assert_eq!(disk.write_file(other, b"other"), Ok(5));
    disk.close_file(other);
    disk.close_file(h);
    assert!(matches!(disk.file_data("A:\\OTHER.DAT").map(|f| f.read()), Some(Ok(d)) if &d[..] == b"other"));
    let h = disk.create_file("A:\\OTHER.DAT", PSP).unwrap();
    disk.rename_file("A:\\OTHER.DAT", "A:\\MOVED.DAT").unwrap();
    disk.close_file(h);
    assert!(matches!(disk.file_data("A:\\MOVED.DAT").map(|f| f.read()), Some(Ok(d)) if d.is_empty()));
}

#[test]
fn a_save_state_keeps_a_created_file_waiting_to_be_emptied() {
    let base = scratch("create_state");
    fs::write(base.join("SAVE.DAT"), b"old save").unwrap();
    let mut disk = DiskController::new(base.clone());
    let h = disk.create_file("C:\\SAVE.DAT", PSP).unwrap();
    // Saving leaves the old bytes on disk.
    let mut w = crate::savestate::Writer::new();
    disk.save_state(&mut w);
    assert_eq!(fs::read(base.join("SAVE.DAT")).unwrap(), b"old save");
    // Loaded back, the handle still empties the file when used.
    let mut loaded = DiskController::new(base.clone());
    let lost = loaded.load_state(&mut crate::savestate::Reader::new(&w.buf)).unwrap();
    assert!(lost.is_empty());
    assert_eq!(fs::read(base.join("SAVE.DAT")).unwrap(), b"old save");
    assert_eq!(loaded.write_file(h, b"new"), Ok(3));
    loaded.close_file(h);
    assert_eq!(fs::read(base.join("SAVE.DAT")).unwrap(), b"new");
}

#[test]
fn an_aborted_program_keeps_the_file_its_create_left_unused() {
    let base = scratch("create_abort");
    fs::write(base.join("SAVE.DAT"), b"old save").unwrap();
    let mut disk = DiskController::new(base.clone());
    disk.create_file("C:\\SAVE.DAT", PSP).unwrap();
    disk.close_all_files();
    assert_eq!(fs::read(base.join("SAVE.DAT")).unwrap(), b"old save");
}

#[test]
fn a_create_empties_the_file_for_a_handle_opened_before_it() {
    let base = scratch("create_shared");
    fs::write(base.join("SAVE.DAT"), b"old save").unwrap();
    let mut disk = DiskController::new(base.clone());
    let before = disk.open_file("C:\\SAVE.DAT", 0x02, PSP).unwrap();
    let created = disk.create_file("C:\\SAVE.DAT", PSP).unwrap();
    assert_eq!(disk.write_file(before, b"new"), Ok(3));
    disk.close_file(before);
    disk.close_file(created);
    assert_eq!(fs::read(base.join("SAVE.DAT")).unwrap(), b"new");
    // A whole-file write by name empties first, not after.
    let created = disk.create_file("C:\\SAVE.DAT", PSP).unwrap();
    disk.write_whole_file("C:\\SAVE.DAT", b"copied", None).unwrap();
    disk.close_file(created);
    assert_eq!(fs::read(base.join("SAVE.DAT")).unwrap(), b"copied");
}

#[test]
fn unmounting_empties_a_created_file_waiting_for_its_first_use() {
    let base = scratch("create_unmount");
    fs::create_dir_all(base.join("D")).unwrap();
    fs::write(base.join("D").join("CFG.DAT"), b"old config").unwrap();
    let mut disk = DiskController::new(base.clone());
    disk.mount(3, &base.join("D"), MountOptions::default(), false).unwrap();
    disk.create_file("D:\\CFG.DAT", PSP).unwrap();
    disk.unmount(3).unwrap();
    assert_eq!(fs::read(base.join("D").join("CFG.DAT")).unwrap(), b"");
}

#[test]
fn a_read_only_file_opened_for_writing_is_access_denied() {
    let base = scratch("read_only_open");
    let save = base.join("SAVE.DAT");
    fs::write(&save, b"old").unwrap();
    let writable = fs::metadata(&save).unwrap().permissions();
    let mut perms = writable.clone();
    perms.set_readonly(true);
    fs::set_permissions(&save, perms).unwrap();
    // Root on Unix writes read-only files anyway: nothing to check.
    let root = fs::OpenOptions::new().write(true).open(&save).is_ok();
    let mut disk = DiskController::new(base.clone());
    // Reading is fine; writing is access denied (05h), as on a FAT
    // disk.
    let results = [
        disk.open_file("C:\\SAVE.DAT", 0, PSP).map(|_| ()),
        disk.open_file("C:\\SAVE.DAT", 1, PSP).map(|_| ()),
        disk.open_file("C:\\SAVE.DAT", 2, PSP).map(|_| ()),
        disk.create_file("C:\\SAVE.DAT", PSP).map(|_| ()),
    ];
    // Put the attribute back first, so a failure leaves a file the
    // next run can delete.
    fs::set_permissions(&save, writable).unwrap();
    if !root {
        assert_eq!(results, [Ok(()), Err(0x05), Err(0x05), Err(0x05)]);
    }
}

#[test]
fn a_missing_file_or_folder_on_a_host_drive() {
    let base = scratch("missing_open");
    fs::write(base.join("SAVE.DAT"), b"old").unwrap();
    let mut disk = DiskController::new(base.clone());
    // A missing file in a folder that's there is file not found (02h);
    // a missing folder on the way, or a file used as one, is path not
    // found (03h), as in DOS.
    assert_eq!(disk.open_file("C:\\NONE.DAT", 2, PSP), Err(0x02));
    assert_eq!(disk.open_file("C:\\NODIR\\SAVE.DAT", 0, PSP), Err(0x03));
    assert_eq!(disk.create_file("C:\\NODIR\\SAVE.DAT", PSP), Err(0x03));
    assert_eq!(disk.open_file("C:\\SAVE.DAT\\X.DAT", 0, PSP), Err(0x03));
    assert_eq!(disk.create_file("C:\\SAVE.DAT\\X.DAT", PSP), Err(0x03));
}

#[test]
fn images_held_in_memory() {
    let mut disk = DiskController::new(scratch("memimage"));
    // A hard disk made for C:, and files put on it.
    let c = DiskImage::blank_hard_disk("C.IMG", 8 << 20, Some("web")).unwrap();
    disk.mount_disk_image(DRIVE_C, c, MountOptions::default()).unwrap();
    disk.fat_volume(DRIVE_C).unwrap().put_file(&["GAMES", "KEEN", "KEEN1.EXE"], b"MZ", 0, 0x5021).unwrap();
    assert!(disk.is_file("C:\\GAMES\\KEEN\\KEEN1.EXE") && disk.is_directory("C:\\GAMES"));
    let info = disk.drive_info(DRIVE_C).unwrap();
    assert_eq!((info.kind, info.label.as_str(), info.read_only), (DriveKind::HardDisk, "WEB", false));
    assert!(info.mount.is_none() && info.root.is_none());

    // A floppy image's bytes as A:, written through DOS.
    let blank = DiskImage::from_memory("F.IMG", MemoryImage::new(1_474_560), true, None, false).unwrap();
    fat::format(&blank, 0, 2880, None).unwrap();
    let bytes = blank.memory().unwrap().clone();
    disk.mount_memory_image(0, "DISK1.IMA", bytes.clone(), MountOptions::default()).unwrap();
    assert_eq!(disk.drive_kind(0), Some(DriveKind::Floppy));
    disk.bios_image(0).unwrap().take_written();
    let h = disk.create_file("A:\\SAVE.DAT", PSP).unwrap();
    assert_eq!(disk.write_file(h, b"saved"), Ok(5));
    disk.close_file(h);
    assert!(!disk.bios_image(0).unwrap().take_written().is_empty());
    assert!(matches!(disk.file_data("A:\\SAVE.DAT").map(|f| f.read()), Some(Ok(d)) if &d[..] == b"saved"));

    // Not a CD in a floppy drive or as C:, nor junk anywhere.
    let cd = MountOptions { kind: DriveKind::CdRom, ..MountOptions::default() };
    assert!(disk.mount_memory_image(1, "B.ISO", bytes.clone(), cd.clone()).is_err());
    assert!(disk.mount_memory_image(DRIVE_C, "C.ISO", bytes, cd).is_err());
    assert!(disk.mount_memory_image(3, "junk.dat", vec![1; 5000].into(), MountOptions::default()).is_err());
    assert!(!disk.is_mounted(3) && disk.is_file("C:\\GAMES\\KEEN\\KEEN1.EXE"));
}

#[test]
fn drives_held_in_memory() {
    let base = scratch("memory");
    let mut disk = DiskController::new(base);
    let mut files = MemFs::new();
    files.insert("GUS\\MIDI\\PIANO.PAT", &b"0123456789"[..]);
    files.insert("GUS\\README.TXT", &b"hi"[..]);
    disk.mount_memory(23, files.clone(), "patches").unwrap();
    assert!(disk.mount_memory(23, files.clone(), "again").is_err());
    assert!(disk.mount_memory(DRIVE_C, files.clone(), "c").is_err());
    assert!(disk.mount_memory(DRIVE_Z, files, "z").is_err());
    let info = disk.drive_info(23).unwrap();
    assert_eq!((info.kind, info.root, info.label.as_str(), info.read_only), (DriveKind::Virtual, None, "PATCHES", true));

    // Directories, with their dot entries and only when asked for.
    let names = |disk: &DiskController, spec: &str, attr: u16| -> Vec<(String, u8)> {
        disk.list_directory(spec, attr).unwrap().into_iter().map(|e| (e.filename, e.attr)).collect()
    };
    assert_eq!(names(&disk, "X:\\*.*", 0x10), [("GUS".to_string(), 0x11)]);
    assert!(names(&disk, "X:\\*.*", 0).is_empty());
    assert_eq!(
        names(&disk, "X:\\GUS\\*.*", 0x10),
        [("..".to_string(), 0x10), (".".to_string(), 0x10), ("MIDI".to_string(), 0x11), ("README.TXT".to_string(), 0x21)]
    );
    assert_eq!(names(&disk, "X:\\GUS\\MIDI\\*.PAT", 0), [("PIANO.PAT".to_string(), 0x21)]);
    assert_eq!(disk.list_directory("X:\\NOPE\\*.*", 0).err(), Some(0x03));
    assert_eq!(disk.get_file_attribute("X:\\GUS"), Ok(0x11));
    assert_eq!(disk.get_file_attribute("X:\\GUS\\README.TXT"), Ok(0x21));
    assert_eq!(disk.get_file_attribute("X:\\GUS\\NOPE.TXT"), Err(0x02));
    assert!(disk.is_directory("X:\\GUS\\MIDI") && !disk.is_directory("X:\\GUS\\README.TXT"));
    assert!(disk.is_file("x:/gus/readme.txt") && !disk.is_file("X:\\GUS"));

    // Relative to the drive's current directory.
    assert!(disk.set_current_directory("X:\\GUS\\MIDI"));
    assert!(!disk.set_current_directory("X:\\GUS\\README.TXT"));
    assert_eq!(disk.get_current_directory_of(23).as_deref(), Some("GUS\\MIDI"));
    assert_eq!(disk.qualify_path("X:PIANO.PAT").as_deref(), Some("X:\\GUS\\MIDI\\PIANO.PAT"));
    assert!(matches!(disk.file_data("X:..\\README.TXT"), Some(FileData::Memory(d)) if &d[..] == b"hi"));

    // Read-only files, read from where the last read ended.
    let h = disk.open_file("X:PIANO.PAT", 2, PSP).unwrap();
    assert_eq!(disk.handle_drive(h), Some(23));
    assert_eq!(disk.read_file(h, 4).unwrap(), b"0123");
    assert_eq!(disk.read_file(h, 2).unwrap(), b"45");
    assert_eq!(disk.read_file(h, 100).unwrap(), b"6789");
    assert_eq!(disk.read_file(h, 1).unwrap(), b"");
    assert_eq!(disk.seek_file(h, -3, 2), Ok(7));
    assert_eq!(disk.position(h), Some(7));
    assert_eq!(disk.read_file(h, 5).unwrap(), b"789");
    assert_eq!(disk.seek_file(h, 20, 0), Ok(20));
    assert_eq!(disk.read_file(h, 1).unwrap(), b"");
    assert_eq!(disk.seek_file(h, -21, 1), Err(0x19));
    assert_eq!(disk.write_file(h, b"x"), Err(0x05));
    assert_eq!(disk.file_time(h), Ok((MEMORY_TIME, MEMORY_DATE)));
    assert_eq!(disk.open_file("X:PIANO.PAT", 1, PSP), Err(0x05));
    assert_eq!(disk.open_file("X:NOPE.PAT", 0, PSP), Err(0x02));
    assert_eq!(disk.open_file("X:\\NOPE\\PIANO.PAT", 0, PSP), Err(0x03));
    assert_eq!(disk.open_file("X:\\GUS", 0, PSP), Err(0x05));
    assert_eq!(disk.create_file("X:NEW.PAT", PSP), Err(0x05));
    assert_eq!(disk.create_directory("X:\\NEW"), Err(0x05));

    disk.unmount(23).unwrap();
    assert!(disk.read_file(h, 1).is_err());
    assert!(!disk.is_file("X:\\GUS\\README.TXT"));
}

#[test]
fn cd_directories_show_their_records() {
    let mut disk = DiskController::new(scratch("dir-records"));
    let record = |time: u16, hidden| crate::cdrom::Extent { lba: 20, size: 2048, time, date: 0x1F5F, hidden };
    let mut files = MemFs::new();
    files.insert_dir_record("", record(1 << 11, false));
    files.insert_dir_record("DATA", record(2 << 11, false));
    files.insert_dir_record("DATA\\SECRET", record(3 << 11, true));
    files.insert("DATA\\SECRET\\KEY.DAT", &b"key"[..]);
    files.insert("DATA\\PLAIN\\A.DAT", &b"a"[..]);
    disk.mount_memory(23, files, "disc").unwrap();

    let times = |spec: &str, attr: u16| -> Vec<(String, u8, u16)> {
        disk.list_directory(spec, attr).unwrap().into_iter().map(|e| (e.filename, e.attr, e.dos_time)).collect()
    };
    // ".." has the parent's record (the root's), "." the directory's own;
    // a directory without a record shows the drive's default.
    assert_eq!(
        times("X:\\DATA\\*.*", 0x10),
        [("..".to_string(), 0x10, 1 << 11), (".".to_string(), 0x10, 2 << 11), ("PLAIN".to_string(), 0x11, MEMORY_TIME)]
    );
    // A hidden directory is listed only when hidden entries are asked for.
    assert_eq!(times("X:\\DATA\\S*.*", 0x12), [("SECRET".to_string(), 0x13, 3 << 11)]);
    assert_eq!(disk.get_file_attribute("X:\\DATA\\SECRET"), Ok(0x13));
    assert_eq!(times("X:\\DATA\\SECRET\\*.*", 0x10)[..2], [("..".to_string(), 0x10, 2 << 11), (".".to_string(), 0x10, 3 << 11)]);
    let plain = disk.list_directory("X:\\DATA\\PLAIN\\*.*", 0x10).unwrap();
    assert_eq!((plain[0].dos_time, plain[0].dos_date), (2 << 11, 0x1F5F));
    assert_eq!((plain[1].dos_time, plain[1].dos_date), (MEMORY_TIME, MEMORY_DATE));
}

#[test]
fn z_drive_is_only_used_when_named() {
    let base = scratch("zdrive");
    fs::write(base.join("HOST.TXT"), b"x").unwrap();
    let mut disk = DiskController::new(base.clone());
    disk.set_current_drive(DRIVE_Z);
    assert!(disk.is_virtual_file("COMMAND.COM"));
    assert!(!disk.is_virtual_file("C:\\COMMAND.COM"));
    let host = disk.list_directory("C:\\*.*", 0x10).unwrap();
    assert!(host.iter().any(|e| e.filename == "HOST.TXT"));
    let z = disk.list_directory("*.*", 0x10).unwrap();
    assert_eq!(z.len(), 1);
    assert_eq!(z[0].filename, "COMMAND.COM");
}

/// Sets a folder's modification time to one in 2020, as a game folder
/// installed earlier has it, so that `HostDirCache` keeps its entries.
/// Always the same time, as a coarse clock gives a folder changed twice
/// in a tick.
fn age(dir: &Path) {
    let mut options = fs::OpenOptions::new();
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_WRITE_ATTRIBUTES, FILE_FLAG_BACKUP_SEMANTICS (to open a
        // folder).
        options.access_mode(0x100).custom_flags(0x0200_0000);
    }
    #[cfg(not(windows))]
    options.read(true);
    let at = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
    options.open(dir).unwrap().set_modified(at).unwrap();
}

/// Files enough for a folder's entries to be kept (`MIN_ENTRIES`).
fn fill(dir: &Path) {
    for i in 0..HostDirCache::MIN_ENTRIES {
        fs::write(dir.join(format!("FILL{:02}.DAT", i)), b"x").unwrap();
    }
}

#[test]
fn host_folder_entries_are_kept_until_the_folder_changes() {
    let root = scratch("host_dir_cache");
    fs::create_dir(root.join("DATA")).unwrap();
    fs::write(root.join("DATA").join("RGN001.GFF"), b"x").unwrap();
    fill(&root.join("DATA"));
    age(&root.join("DATA"));
    let disk = DiskController::new(root.clone());
    let data = disk.resolve_path(r"C:\DATA").unwrap();
    assert!(disk.is_file(r"C:\DATA\RGN001.GFF"));
    assert!(disk.host_dirs.dirs.borrow().contains_key(&data), "an old folder is kept");

    // Another program adds a file: the folder's time moves, and it is
    // read again.
    fs::write(data.join("Long Name.txt"), b"x").unwrap();
    assert!(disk.is_file(r"C:\DATA\LONGNA~1.TXT"));

    // The folder changed within the last two seconds: not kept, as a
    // change in the same tick of the clock wouldn't show.
    assert!(!disk.host_dirs.dirs.borrow().contains_key(&data), "a recent folder isn't kept");
}

#[test]
fn host_folders_are_read_again_after_the_guest_changes_one() {
    let root = scratch("host_dir_cache_guest");
    fs::write(root.join("OLD.DAT"), b"x").unwrap();
    fill(&root);
    age(&root);
    let mut disk = DiskController::new(root.clone());
    assert!(disk.is_file(r"C:\OLD.DAT"));
    let host_root = disk.resolve_path(r"C:\").unwrap();
    assert!(disk.host_dirs.dirs.borrow().contains_key(&host_root));

    // Each change through DOS is seen at once, even where the folder's
    // time comes out the same: set back after each.
    disk.rename_file(r"C:\OLD.DAT", r"C:\NEW.DAT").unwrap();
    age(&root);
    assert!(!disk.is_file(r"C:\OLD.DAT"));
    assert!(disk.is_file(r"C:\NEW.DAT"));

    let handle = disk.create_file(r"C:\MADE.DAT", PSP).unwrap();
    disk.close_file(handle);
    age(&root);
    assert!(disk.is_file(r"C:\MADE.DAT"));

    disk.delete_file(r"C:\MADE.DAT").unwrap();
    age(&root);
    assert!(!disk.is_file(r"C:\MADE.DAT"));

    disk.create_directory(r"C:\SAVES").unwrap();
    age(&root);
    assert!(disk.is_directory(r"C:\SAVES"));
    disk.remove_directory(r"C:\SAVES").unwrap();
    age(&root);
    assert!(!disk.is_directory(r"C:\SAVES"));
}

/// The names FindFirst and FindNext give for `spec`, until the end.
fn find_all(disk: &DiskController, spec: &str) -> Vec<String> {
    (0..).map_while(|i| disk.find_directory_entry(spec, i, 0x16).ok()).map(|e| e.filename).collect()
}

#[test]
fn find_next_goes_on_from_the_listing_find_first_made() {
    let root = scratch("find_next_kept");
    for name in ["A.DAT", "B.DAT", "C.DAT", "Long Name.txt"] {
        fs::write(root.join(name), b"x").unwrap();
    }
    fs::create_dir(root.join("SUB")).unwrap();
    age(&root);
    let disk = DiskController::new(root.clone());
    let listed: Vec<String> = disk.list_directory(r"C:\*.*", 0x16).unwrap().into_iter().map(|e| e.filename).collect();
    assert_eq!(listed, ["A.DAT", "B.DAT", "C.DAT", "LONGNA~1.TXT", "SUB"]);
    assert_eq!(find_all(&disk, r"C:\*.*"), listed);
    assert_eq!(disk.host_dirs.searches.borrow().len(), 1, "kept for FindNext");
    // FindNext spells the pattern as the DTA keeps it.
    assert_eq!(disk.find_directory_entry(r"C:\*.dat", 0, 0x16).unwrap().filename, "A.DAT");
    assert_eq!(disk.find_directory_entry(r"C:\????????.DAT", 1, 0x16).unwrap().filename, "B.DAT");
    assert_eq!(disk.host_dirs.searches.borrow().len(), 2, "one search per pattern");

    // Another program adds a file mid-search: the folder's time moves,
    // and the next FindNext lists it again.
    assert_eq!(disk.find_directory_entry(r"C:\*.*", 0, 0x16).unwrap().filename, "A.DAT");
    fs::write(root.join("AA.DAT"), b"x").unwrap();
    assert_eq!(disk.find_directory_entry(r"C:\*.*", 1, 0x16).unwrap().filename, "AA.DAT");
}

#[test]
fn find_next_sees_the_guest_s_changes_mid_search() {
    let root = scratch("find_next_guest");
    for name in ["A.DAT", "B.DAT", "C.DAT"] {
        fs::write(root.join(name), b"x").unwrap();
    }
    age(&root);
    let mut disk = DiskController::new(root.clone());
    // A program deleting what it finds, as before: the listing after
    // the delete is the folder as it is then.
    assert_eq!(disk.find_directory_entry(r"C:\*.DAT", 0, 0).unwrap().filename, "A.DAT");
    disk.delete_file(r"C:\A.DAT").unwrap();
    age(&root);
    assert_eq!(disk.find_directory_entry(r"C:\*.DAT", 1, 0).unwrap().filename, "C.DAT");

    // A write changes the size a search reports.
    let handle = disk.open_file(r"C:\C.DAT", 2, PSP).unwrap();
    assert_eq!(disk.find_directory_entry(r"C:\C.DAT", 0, 0).unwrap().size, 1);
    disk.write_file(handle, b"longer").unwrap();
    disk.close_file(handle);
    age(&root);
    let found = disk.find_directory_entry(r"C:\*.DAT", 1, 0).unwrap();
    assert_eq!((found.filename.as_str(), found.size), ("C.DAT", 6));

    // Copying what it finds into another folder, as an installer does,
    // keeps the search of the source.
    fs::create_dir(root.join("DEST")).unwrap();
    age(&root);
    assert_eq!(disk.find_directory_entry(r"C:\*.DAT", 0, 0).unwrap().filename, "B.DAT");
    let handle = disk.create_file(r"C:\DEST\B.DAT", PSP).unwrap();
    disk.write_file(handle, b"x").unwrap();
    disk.close_file(handle);
    let source = disk.resolve_path(r"C:\").unwrap();
    assert!(disk.host_dirs.searches.borrow().iter().any(|s| s.key.dir == source), "kept");
    assert_eq!(disk.find_directory_entry(r"C:\*.DAT", 1, 0).unwrap().filename, "C.DAT");
}

#[test]
fn small_host_folders_are_read_each_time() {
    let root = scratch("host_dir_cache_small");
    fs::write(root.join("GAME.EXE"), b"x").unwrap();
    age(&root);
    let disk = DiskController::new(root.clone());
    assert!(disk.is_file(r"C:\GAME.EXE"));
    assert!(disk.host_dirs.dirs.borrow().is_empty());
}

#[test]
fn a_name_missing_from_a_kept_folder_is_looked_for_again() {
    let root = scratch("host_dir_cache_unmoved");
    fill(&root);
    age(&root);
    let disk = DiskController::new(root.clone());
    assert!(!disk.is_file(r"C:\PATCH.DAT"));
    let host_root = disk.resolve_path(r"C:\").unwrap();
    assert!(disk.host_dirs.dirs.borrow().contains_key(&host_root));

    // Another program adds a file and the folder's time stays where it
    // was, as on FAT under Windows or after an unpacker restores it.
    fs::write(root.join("Patch.dat"), b"x").unwrap();
    age(&root);
    assert!(disk.is_file(r"C:\PATCH.DAT"));
    let found: Vec<String> = disk.list_directory(r"C:\PATCH2.DAT", 0).unwrap().into_iter().map(|e| e.filename).collect();
    assert!(found.is_empty());
    fs::write(root.join("PATCH2.DAT"), b"x").unwrap();
    age(&root);
    let found: Vec<String> = disk.list_directory(r"C:\PATCH2.DAT", 0).unwrap().into_iter().map(|e| e.filename).collect();
    assert_eq!(found, ["PATCH2.DAT"]);

    // A wildcard search lists the folder as it is.
    fs::write(root.join("PATCH3.DAT"), b"x").unwrap();
    age(&root);
    let found: Vec<String> = disk.list_directory(r"C:\PATCH*.DAT", 0).unwrap().into_iter().map(|e| e.filename).collect();
    assert_eq!(found, ["PATCH.DAT", "PATCH2.DAT", "PATCH3.DAT"]);
}

#[test]
fn a_create_over_an_existing_file_keeps_the_folders() {
    let root = scratch("host_dir_cache_recreate");
    fs::write(root.join("CONFIG.DAT"), b"x").unwrap();
    fill(&root);
    age(&root);
    let mut disk = DiskController::new(root.clone());
    assert!(disk.is_file(r"C:\CONFIG.DAT"));
    let changes = disk.host_dirs.changes.get();
    let handle = disk.create_file(r"C:\CONFIG.DAT", PSP).unwrap();
    disk.close_file(handle);
    assert_eq!(disk.host_dirs.changes.get(), changes, "no name was added");
    let handle = disk.create_file(r"C:\NEW.DAT", PSP).unwrap();
    disk.close_file(handle);
    assert_ne!(disk.host_dirs.changes.get(), changes);
}

#[test]
fn a_state_whose_cd_image_moved_says_the_image_is_gone() {
    let base = scratch("moved_cue_state");
    fs::create_dir_all(base.join("c")).unwrap();
    fs::create_dir_all(base.join("disc")).unwrap();
    fs::write(base.join("disc/DATA.DAT"), b"on the disc").unwrap();
    let folder = CdImage::from_folder(
        crate::cdrom::folder::build(&base.join("disc"), "GAMECD").unwrap(),
        &base.join("disc"),
    )
    .unwrap();
    let mut iso = Vec::new();
    for lba in 0..folder.leadout() {
        let mut sector = [0u8; crate::cdrom::DATA_SECTOR];
        folder.read_data(lba, &mut sector).unwrap();
        iso.extend_from_slice(&sector);
    }
    fs::create_dir_all(base.join("cd")).unwrap();
    fs::write(base.join("cd/GAME.ISO"), &iso).unwrap();
    fs::write(
        base.join("cd/GAME.CUE"),
        b"FILE \"GAME.ISO\" BINARY\r\n  TRACK 01 MODE1/2048\r\n    INDEX 01 00:00:00\r\n",
    )
    .unwrap();
    let mut disk = DiskController::new(base.join("c"));
    disk.mount(3, &base.join("cd/GAME.CUE"), cdrom(), false)
        .unwrap();
    let mut w = crate::savestate::Writer::new();
    disk.save_state(&mut w);
    // Windows won't move the files while the image is open.
    drop(disk);
    let load = || {
        DiskController::new(base.join("c"))
            .load_state(&mut crate::savestate::Reader::new(&w.buf))
            .map(|_| ())
    };
    assert_eq!(
        load(),
        Ok(()),
        "a fresh machine mounts the disc as the state has it"
    );

    // The sheet moved with its track: the load names the sheet.
    fs::rename(base.join("cd"), base.join("moved")).unwrap();
    let e = load().unwrap_err();
    assert!(
        matches!(e, crate::savestate::StateError::Media(_)),
        "{:?}",
        e
    );
    let message = e.to_string();
    assert!(
        message.starts_with(
            "the saved state's drives can't be mounted here: drive D: can't be mounted: "
        ),
        "{}",
        message
    );
    assert!(
        message.ends_with("GAME.CUE isn't there (moved or deleted since the state was saved?)"),
        "{}",
        message
    );

    // The sheet is there, its track isn't.
    fs::create_dir_all(base.join("cd")).unwrap();
    fs::copy(base.join("moved/GAME.CUE"), base.join("cd/GAME.CUE")).unwrap();
    let e = load().unwrap_err();
    assert!(
        matches!(e, crate::savestate::StateError::Media(_)),
        "{:?}",
        e
    );
    let message = e.to_string();
    assert!(
        message.ends_with("GAME.ISO not found (moved or deleted since the state was saved?)"),
        "{}",
        message
    );
}
