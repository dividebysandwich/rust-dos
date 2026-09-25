use rust_dos::command::{CommandDispatcher, split_command};
use rust_dos::cpu::Cpu;
use rust_dos::disk::{DriveKind, MountOptions};
use std::fs;
use std::path::PathBuf;

fn scratch(name: &str, dirs: &[&str]) -> PathBuf {
    let base = PathBuf::from("target/test_command").join(name);
    let _ = fs::remove_dir_all(&base);
    for d in dirs {
        fs::create_dir_all(base.join(d)).unwrap();
    }
    fs::canonicalize(&base).unwrap()
}

/// Run a shell command line and return what it printed. A row ends where
/// the printing did: the spaces the command printed stay, so a line that
/// wrapped at a space joins up again without the rows' line breaks.
fn run(cpu: &mut Cpu, line: &str) -> String {
    for b in cpu.bus.vga.vram_text.iter_mut() {
        *b = 0;
    }
    cpu.bus.cursor_x = 0;
    cpu.bus.cursor_y = 0;
    let (command, args) = split_command(line);
    assert!(
        CommandDispatcher::new().dispatch(cpu, command, args),
        "{} is not a built-in",
        command
    );
    cpu.bus
        .vga
        .vram_text
        .chunks(160)
        .take(25)
        .map(|row| {
            row.iter()
                .step_by(2)
                .map(|&b| b as char)
                .collect::<String>()
                .trim_end_matches('\0')
                .replace('\0', " ")
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_string()
}

#[test]
fn drive_letter_switches_drives() {
    let base = scratch("switch", &["c", "d"]);
    let mut cpu = Cpu::new(base.join("c"));
    cpu.bus
        .mount_drive(3, &base.join("d"), MountOptions::default(), false)
        .unwrap();

    assert_eq!(run(&mut cpu, "d:"), "");
    assert_eq!(cpu.bus.disk.get_current_drive(), 3);
    assert_eq!(run(&mut cpu, "Q:"), "Invalid drive specification");
    assert_eq!(cpu.bus.disk.get_current_drive(), 3);
    run(&mut cpu, "Z:");
    assert_eq!(cpu.bus.disk.get_current_drive(), 25);
}

#[test]
fn dosconfig_asks_for_the_settings_window() {
    let base = scratch("dosconfig", &["c"]);
    let mut cpu = Cpu::new(base.join("c"));
    assert!(!cpu.bus.config_ui_requested);
    assert_eq!(run(&mut cpu, "dosconfig"), "");
    assert!(cpu.bus.config_ui_requested);
}

#[test]
fn drives_keep_their_mount_options() {
    let base = scratch("options", &["c", "floppy"]);
    let mut cpu = Cpu::new(base.join("c"));
    let opts = MountOptions { kind: DriveKind::Floppy, label: Some("disk 1".into()), read_only: true, ..Default::default() };
    cpu.bus.mount_drive(0, &base.join("floppy"), opts.clone(), false).unwrap();
    let info = cpu.bus.disk.drive_info(0).unwrap();
    // The label as given, not as DOS shows it.
    let mount = info.mount.unwrap();
    assert_eq!((info.label.as_str(), mount.path, mount.opts), ("DISK 1", base.join("floppy"), opts));
    assert_eq!(cpu.bus.disk.drive_info(2).unwrap().mount.unwrap().opts, MountOptions::default());
    assert!(cpu.bus.disk.drive_info(25).unwrap().mount.is_none());
}

#[test]
fn mount_lists_mounts_and_unmounts() {
    let base = scratch("mount", &["c", "floppy", "cd"]);
    let mut cpu = Cpu::new(base.join("c"));

    let out = run(
        &mut cpu,
        &format!(
            "MOUNT a {} floppy -label disk1",
            base.join("floppy").display()
        ),
    );
    assert!(out.starts_with("Drive A: is mounted as floppy"), "{}", out);
    assert_eq!(cpu.bus.disk.drive_kind(0), Some(DriveKind::Floppy));
    assert_eq!(cpu.bus.read_16(0x0410) & 1, 1);

    let out = run(&mut cpu, &format!("MOUNT a {}", base.join("cd").display()));
    assert_eq!(out, "Drive A: is already mounted");

    let out = run(
        &mut cpu,
        &format!("mount D \"{}\" -t cdrom", base.join("cd").display()),
    );
    assert!(out.starts_with("Drive D: is mounted as cdrom"), "{}", out);

    // Long host paths wrap at 80 columns, so compare the unwrapped text.
    let listing = run(&mut cpu, "MOUNT").replace('\n', "");
    let floppy = format!(
        "A:    floppy  DISK1       {}",
        base.join("floppy").display()
    );
    assert!(listing.contains(&floppy), "{}", listing);
    let cd = format!(
        "D:    cdrom   RUSTDOS     {} (read-only)",
        base.join("cd").display()
    );
    assert!(listing.contains(&cd), "{}", listing);
    assert!(listing.contains("C:    hdd     RUSTDOS"), "{}", listing);
    assert!(
        listing.ends_with("Z:    virtual RUSTDOS     (built-in)"),
        "{}",
        listing
    );

    assert_eq!(run(&mut cpu, "MOUNT -u a"), "Drive A: has been unmounted");
    assert!(!cpu.bus.disk.is_mounted(0));
    assert_eq!(cpu.bus.read_16(0x0410) & 1, 0);
    assert_eq!(run(&mut cpu, "MOUNT -u c"), "Drive C: cannot be unmounted");
    assert_eq!(run(&mut cpu, "MOUNT -u z"), "Drive Z: cannot be unmounted");

    let out = run(&mut cpu, "MOUNT e");
    assert!(out.starts_with("Missing host directory"), "{}", out);
    let out = run(&mut cpu, &format!("MOUNT e {} zip", base.display()));
    assert!(out.starts_with("Unknown option 'zip'"), "{}", out);
    let out = run(
        &mut cpu,
        &format!("MOUNT e {}", base.join("nope").display()),
    );
    assert!(
        out.replace('\n', "").ends_with("is not a directory or a disk or CD image"),
        "{}",
        out
    );
}

#[test]
fn cd_is_per_drive() {
    let base = scratch("cd", &["c/CSUB", "d/SUB"]);
    let mut cpu = Cpu::new(base.join("c"));
    cpu.bus
        .mount_drive(3, &base.join("d"), MountOptions::default(), false)
        .unwrap();

    assert_eq!(run(&mut cpu, "CD D:\\SUB"), "");
    assert_eq!(cpu.bus.disk.get_current_drive(), 2);
    assert_eq!(run(&mut cpu, "CD"), "C:\\");
    assert_eq!(run(&mut cpu, "CD D:"), "D:\\SUB");
    assert_eq!(run(&mut cpu, "CD CSUB"), "");
    assert_eq!(run(&mut cpu, "CD"), "C:\\CSUB");
    assert_eq!(run(&mut cpu, "CD NOPE"), "Invalid directory");
    assert_eq!(run(&mut cpu, "CD Q:"), "Invalid drive specification");
}

#[test]
fn dir_lists_any_drive() {
    let base = scratch("dir", &["c", "d/SUB"]);
    fs::write(base.join("d/SUB/readme.txt"), b"hello").unwrap();
    fs::write(base.join("d/SUB/game.exe"), vec![0u8; 1234]).unwrap();
    fs::write(base.join("d/SUB/Long File Name.txt"), b"x").unwrap();
    let mut cpu = Cpu::new(base.join("c"));
    let opts = MountOptions {
        kind: DriveKind::Floppy,
        label: Some("DATA".to_string()),
        read_only: false,
        ..Default::default()
    };
    cpu.bus
        .mount_drive(3, &base.join("d"), opts, false)
        .unwrap();

    let out = run(&mut cpu, "DIR D:\\SUB\\*.TXT");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], " Volume in drive D is DATA");
    assert_eq!(lines[1], " Directory of D:\\SUB");
    assert!(
        lines[3].starts_with("LONGFI~1 TXT              1 "),
        "{}",
        lines[3]
    );
    assert!(
        lines[4].starts_with("README   TXT              5 "),
        "{}",
        lines[4]
    );
    assert_eq!(lines[5].trim(), "2 file(s)              6 bytes");
    // Two 512-byte clusters used on an empty 1.44 MB floppy + the SUB dir
    assert_eq!(lines[6].trim(), "1,454,592 bytes free");

    let out = run(&mut cpu, "DIR D:\\SUB");
    assert!(out.contains("GAME     EXE          1,234"), "{}", out);
    assert!(out.contains("\n.            <DIR>\n"), "{}", out);

    let out = run(&mut cpu, "DIR Z:");
    assert!(out.contains("COMMAND  COM"), "{}", out);

    assert_eq!(run(&mut cpu, "DIR Q:"), "Invalid drive specification");
    assert!(run(&mut cpu, "DIR D:\\SUB\\*.BAT").ends_with("File not found"));
}

#[test]
fn ls_lists_names_in_columns() {
    let base = scratch("ls", &["c", "d/GAMES", "d/docs", "d/MANY"]);
    for name in ["readme.txt", "setup.exe", "install.bat", "config.sys", "cdrom.com", "notes"] {
        fs::write(base.join("d").join(name), b"x").unwrap();
    }
    fs::write(base.join("d/GAMES/doom.exe"), b"x").unwrap();
    for i in 1..=8 {
        fs::write(base.join(format!("d/MANY/file{:02}.txt", i)), b"x").unwrap();
    }
    let mut cpu = Cpu::new(base.join("c"));
    cpu.bus
        .mount_drive(3, &base.join("d"), MountOptions::default(), false)
        .unwrap();
    let attr = |cpu: &Cpu, row: usize, col: usize| cpu.bus.vga.vram_text[(row * 80 + col) * 2 + 1];

    // Directories first in capitals, then the files in lower case, as
    // many to a line as fit. Each column is as wide as its longest name.
    assert_eq!(
        run(&mut cpu, "LS D:"),
        "DOCS       GAMES  MANY  cdrom.com  config.sys  install.bat  notes  readme.txt\n\
         setup.exe"
    );
    assert_eq!(attr(&cpu, 0, 0), 0x09, "directories are blue");
    assert_eq!(attr(&cpu, 0, 24), 0x0A, "programs are green");
    assert_eq!(attr(&cpu, 0, 35), 0x07, "other files are gray");
    assert_eq!(attr(&cpu, 0, 47), 0x0A, "batch files are green");
    assert_eq!(attr(&cpu, 1, 0), 0x0A);

    // Twelve-column names, six to a line and filled row by row.
    assert_eq!(
        run(&mut cpu, "ls d:\\many"),
        "file01.txt  file02.txt  file03.txt  file04.txt  file05.txt  file06.txt\n\
         file07.txt  file08.txt"
    );

    assert_eq!(run(&mut cpu, "ls d:\\games"), "doom.exe");
    run(&mut cpu, "D:");
    run(&mut cpu, "CD GAMES");
    assert_eq!(run(&mut cpu, "ls"), "doom.exe");
    assert!(run(&mut cpu, "ls ..").starts_with("DOCS       GAMES  MANY"));
    // "c*" is "c*.*", and names two patterns both find are listed once.
    assert_eq!(run(&mut cpu, "ls d:\\c*"), "cdrom.com  config.sys");
    assert_eq!(run(&mut cpu, "ls d:\\*.exe d:\\s*"), "setup.exe");
    assert!(run(&mut cpu, "LS Z:").contains("command.com"));

    assert_eq!(run(&mut cpu, "ls d:*.zip"), "No files or subdirectories to display");
    assert_eq!(run(&mut cpu, "ls /x"), "Invalid switch - /x");
    assert_eq!(run(&mut cpu, "ls d:\\g*\\doom.exe"), "Unhandled wildcard pattern - d:\\g*\\doom.exe");
    assert!(run(&mut cpu, "ls /?").starts_with("Lists the files and directories"));
}

#[test]
fn type_reads_drive_qualified_paths() {
    let base = scratch("type", &["c", "d/SUB"]);
    fs::write(base.join("d/SUB/a.txt"), b"line one\nline two").unwrap();
    let mut cpu = Cpu::new(base.join("c"));
    cpu.bus
        .mount_drive(3, &base.join("d"), MountOptions::default(), false)
        .unwrap();

    assert_eq!(run(&mut cpu, "TYPE D:\\SUB\\A.TXT"), "line one\nline two");
    assert_eq!(run(&mut cpu, "TYPE D:\\SUB\\NONE.TXT"), "File not found");
}

#[test]
fn autoexec_lines_queue_before_autoexec_bat() {
    let base = scratch("autoexec", &["c"]);
    fs::write(
        base.join("c/AUTOEXEC.BAT"),
        "@ECHO OFF\r\nREM setup\r\n\r\nGAME\r\n",
    )
    .unwrap();
    let mut cpu = Cpu::new(base.join("c"));
    cpu.bus.disk.set_current_drive(25); // AUTOEXEC.BAT is found on C: regardless

    cpu.queue_batch_lines(["MOUNT A floppy", "  ", "rem comment", "A:"]);
    assert!(cpu.queue_batch_file("C:\\AUTOEXEC.BAT"));
    assert_eq!(cpu.batch.pending_lines(), ["MOUNT A floppy", "rem comment", "A:", "@ECHO OFF", "REM setup", "GAME"]);
}

#[test]
fn mixer_sets_volumes_and_shows_the_table() {
    let mut cpu = Cpu::new(scratch("mixer", &["c"]).join("c"));
    let out = run(&mut cpu, "MIXER x30 opl 150 pcspeaker d-6 r40");
    assert!(out.starts_with("Channel"), "{}", out);
    let opl = out.lines().find(|l| l.starts_with("OPL")).unwrap();
    assert!(opl.contains(" 150:150") && opl.contains("+3.52"), "{}", opl);
    assert!(opl.contains("Stereo") && opl.contains("30"), "crossfeed on every stereo channel: {}", opl);
    let speaker = out.lines().find(|l| l.starts_with("PCSPEAKER")).unwrap();
    assert!(speaker.contains("  50:50") && speaker.contains("Mono") && speaker.contains("40"), "{}", speaker);
    // The volumes and the reverb that came on are the settings', for the
    // frontend to take over.
    let settings = cpu.bus.mixer.settings();
    assert_eq!(settings.level(rust_dos::mixer::Channel::Fm), 150);
    assert_eq!(settings.reverb, rust_dos::mixer::ReverbPreset::Medium);
    assert!(std::mem::take(&mut cpu.bus.mixer_changed));
    assert_eq!(cpu.bus.mixer.reverb_send(rust_dos::mixer::Channel::Speaker), 0.4);
}

#[test]
fn mixer_noshow_prints_nothing_and_errors_say_why() {
    let mut cpu = Cpu::new(scratch("mixer_noshow", &["c"]).join("c"));
    assert_eq!(run(&mut cpu, "MIXER sb reverse /noshow"), "");
    assert!(cpu.bus.mixer.reverse(rust_dos::mixer::Channel::Sb));
    assert!(!cpu.bus.mixer_changed, "line-out isn't a setting");
    assert_eq!(run(&mut cpu, "MIXER disney 50"), "MIXER: Channel DISNEY is not active");
    assert!(run(&mut cpu, "MIXER /?").starts_with("Displays or changes the sound mixer settings."));
}

#[test]
fn command_names_end_where_command_com_ends_them() {
    assert_eq!(split_command("DIR /W"), ("DIR", " /W"));
    assert_eq!(split_command("DIR/W"), ("DIR", "/W"));
    assert_eq!(split_command("cd.."), ("cd", ".."));
    assert_eq!(split_command("CD\\GAMES"), ("CD", "\\GAMES"));
    assert_eq!(split_command("ECHO."), ("ECHO", "."));
    assert_eq!(split_command("echo  two spaces"), ("echo", "  two spaces"));
    assert_eq!(split_command("PATH=C:\\DOS"), ("PATH", "=C:\\DOS"));
    // Programs keep their dots, backslashes and colons.
    assert_eq!(split_command("GAME.EXE -x"), ("GAME.EXE", " -x"));
    assert_eq!(split_command("CDPLAYER.EXE"), ("CDPLAYER.EXE", ""));
    assert_eq!(split_command("C:\\GAMES\\GO.BAT 1"), ("C:\\GAMES\\GO.BAT", " 1"));
    assert_eq!(split_command("D:"), ("D:", ""));
    assert_eq!(split_command("  VER"), ("VER", ""));
}

#[test]
fn echo_prints_its_text_as_it_is() {
    let base = scratch("echo", &["c"]);
    let mut cpu = Cpu::new(base.join("c"));
    assert_eq!(run(&mut cpu, "ECHO    1. Play"), "   1. Play");
    assert_eq!(run(&mut cpu, "ECHO."), "");
    assert_eq!(run(&mut cpu, "ECHO.hi"), "hi");
    assert_eq!(run(&mut cpu, "ECHO [x]"), "[x]");
    assert_eq!(run(&mut cpu, "ECHO .x"), ".x");
    assert_eq!(run(&mut cpu, "ECHO"), "ECHO is on");
    // Code page 437 characters (a box corner and an umlaut) print as they are.
    run(&mut cpu, "ECHO \u{C9}\u{84}");
    assert_eq!(&cpu.bus.vga.vram_text[..4], &[0xC9, 0x07, 0x84, 0x07]);
    assert_eq!(run(&mut cpu, "REM nothing"), "");
}

#[test]
fn cd_dot_dot_and_type_stop_at_the_end_of_file_mark() {
    let base = scratch("cddotdot", &["c/sub"]);
    fs::write(base.join("c/sub/a.txt"), b"one\ntwo\x1Ajunk").unwrap();
    let mut cpu = Cpu::new(base.join("c"));
    run(&mut cpu, "CD SUB");
    assert_eq!(run(&mut cpu, "CD"), "C:\\SUB");
    assert_eq!(run(&mut cpu, "TYPE A.TXT"), "one\ntwo");
    run(&mut cpu, "CD..");
    assert_eq!(run(&mut cpu, "CD"), "C:\\");
    run(&mut cpu, "CD\\SUB");
    assert_eq!(run(&mut cpu, "CD"), "C:\\SUB");
}

#[test]
fn copy_copies_files_to_names_and_directories() {
    let base = scratch("copy", &["c/sub", "c/dest"]);
    fs::write(base.join("c/A.TXT"), b"alpha").unwrap();
    fs::write(base.join("c/B.TXT"), b"beta\x1Ajunk").unwrap();
    fs::write(base.join("c/LONG.TXT"), b"a much longer file than alpha").unwrap();
    let mut cpu = Cpu::new(base.join("c"));

    assert_eq!(run(&mut cpu, "COPY A.TXT NEW.TXT"), "        1 file(s) copied");
    assert_eq!(fs::read(base.join("c/NEW.TXT")).unwrap(), b"alpha");
    // Over a longer file only the source's bytes are left.
    run(&mut cpu, "COPY A.TXT LONG.TXT");
    assert_eq!(fs::read(base.join("c/LONG.TXT")).unwrap(), b"alpha");
    // Into a directory, with wildcards, keeping the date and time.
    let when = fs::metadata(base.join("c/A.TXT")).unwrap().modified().unwrap();
    assert_eq!(run(&mut cpu, "COPY *.TXT DEST"), "A.TXT\nB.TXT\nLONG.TXT\nNEW.TXT\n        4 file(s) copied");
    assert_eq!(fs::read(base.join("c/dest/B.TXT")).unwrap(), b"beta\x1Ajunk", "binary by default");
    let copied = fs::metadata(base.join("c/dest/A.TXT")).unwrap().modified().unwrap();
    assert!(copied.duration_since(when).unwrap_or_else(|e| e.duration()).as_secs() < 3);
    // A destination with wildcards takes the names.
    run(&mut cpu, "COPY DEST\\*.TXT SUB\\*.BAK");
    assert_eq!(fs::read(base.join("c/sub/A.BAK")).unwrap(), b"alpha");
    // Joining, as text up to the end of file mark.
    assert_eq!(run(&mut cpu, "COPY A.TXT+B.TXT AB.TXT"), "A.TXT\nB.TXT\n        1 file(s) copied");
    assert_eq!(fs::read(base.join("c/AB.TXT")).unwrap(), b"alphabeta");
    assert_eq!(run(&mut cpu, "COPY A.TXT A.TXT"), "File cannot be copied onto itself\n        0 file(s) copied");
    assert_eq!(run(&mut cpu, "COPY NONE.TXT X.TXT"), "File not found - NONE.TXT\n        0 file(s) copied");
    assert_eq!(run(&mut cpu, "COPY A.TXT CON"), "alpha        1 file(s) copied");
}

#[test]
fn del_ren_md_rd_and_vol() {
    let base = scratch("files", &["c"]);
    for name in ["ONE.TXT", "TWO.TXT", "KEEP.DAT"] {
        fs::write(base.join("c").join(name), name).unwrap();
    }
    let mut cpu = Cpu::new(base.join("c"));
    run(&mut cpu, "REN *.TXT *.BAK");
    assert!(base.join("c/ONE.BAK").is_file() && base.join("c/TWO.BAK").is_file());
    assert_eq!(run(&mut cpu, "REN NONE.X Y.X"), "Duplicate file name or file not found");
    run(&mut cpu, "DEL *.BAK");
    assert!(!base.join("c/ONE.BAK").exists() && base.join("c/KEEP.DAT").exists());
    assert_eq!(run(&mut cpu, "ERASE *.BAK"), "File not found");

    run(&mut cpu, "MD GAMES");
    assert!(base.join("c/GAMES").is_dir());
    run(&mut cpu, "COPY KEEP.DAT GAMES");
    assert_eq!(run(&mut cpu, "RD GAMES"), "Invalid path, not directory,\nor directory not empty");
    run(&mut cpu, "DEL GAMES");
    assert_eq!(run(&mut cpu, "RMDIR GAMES"), "");
    assert!(!base.join("c/GAMES").exists());

    assert_eq!(run(&mut cpu, "VOL"), " Volume in drive C is RUSTDOS\n Volume Serial Number is 1234-0002");
    assert_eq!(run(&mut cpu, "VOL Q:"), "Invalid drive specification");
}

#[test]
fn keyb_changes_the_keyboard_layout() {
    let base = scratch("keyb", &["c"]);
    let mut cpu = Cpu::new(base.join("c"));
    assert_eq!(run(&mut cpu, "KEYB"), "Current keyboard code: US (United States)");
    run(&mut cpu, "KEYB GR,437");
    assert_eq!(cpu.bus.kbd.layout.code, "gr");
    assert_eq!(run(&mut cpu, "KEYB de"), "");
    assert_eq!(run(&mut cpu, "KEYB XX"), "Invalid keyboard code specified");
    assert_eq!((cpu.errorlevel, cpu.bus.kbd.layout.code), (1, "gr"));
}
