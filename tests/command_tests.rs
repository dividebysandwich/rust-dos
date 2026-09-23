use rust_dos::command::CommandDispatcher;
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

/// Run a shell command line and return what it printed.
fn run(cpu: &mut Cpu, line: &str) -> String {
    for b in cpu.bus.vga.vram_text.iter_mut() {
        *b = 0;
    }
    cpu.bus.cursor_x = 0;
    cpu.bus.cursor_y = 0;
    let (command, args) = match line.split_once(' ') {
        Some((c, a)) => (c, a.trim()),
        None => (line, ""),
    };
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
                .map(|&b| if b == 0 { ' ' } else { b as char })
                .collect::<String>()
                .trim_end()
                .to_string()
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
fn mount_lists_mounts_and_unmounts() {
    let base = scratch("mount", &["c", "floppy", "cd"]);
    let mut cpu = Cpu::new(base.join("c"));

    let out = run(
        &mut cpu,
        &format!("MOUNT a {} floppy -label disk1", base.join("floppy").display()),
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
    let floppy = format!("A:    floppy  DISK1       {}", base.join("floppy").display());
    assert!(listing.contains(&floppy), "{}", listing);
    let cd = format!("D:    cdrom   RUSTDOS     {} (read-only)", base.join("cd").display());
    assert!(listing.contains(&cd), "{}", listing);
    assert!(listing.contains("C:    hdd     RUSTDOS"), "{}", listing);
    assert!(listing.ends_with("Z:    virtual RUSTDOS     (built-in)"), "{}", listing);

    assert_eq!(run(&mut cpu, "MOUNT -u a"), "Drive A: has been unmounted");
    assert!(!cpu.bus.disk.is_mounted(0));
    assert_eq!(cpu.bus.read_16(0x0410) & 1, 0);
    assert_eq!(run(&mut cpu, "MOUNT -u c"), "Drive C: cannot be unmounted");
    assert_eq!(run(&mut cpu, "MOUNT -u z"), "Drive Z: cannot be unmounted");

    let out = run(&mut cpu, "MOUNT e");
    assert!(out.starts_with("Missing host directory"), "{}", out);
    let out = run(&mut cpu, &format!("MOUNT e {} iso", base.display()));
    assert!(out.starts_with("Unknown option 'iso'"), "{}", out);
    let out = run(&mut cpu, &format!("MOUNT e {}", base.join("nope").display()));
    assert!(out.replace('\n', "").ends_with("is not a directory"), "{}", out);
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
    };
    cpu.bus.mount_drive(3, &base.join("d"), opts, false).unwrap();

    let out = run(&mut cpu, "DIR D:\\SUB\\*.TXT");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], " Volume in drive D is DATA");
    assert_eq!(lines[1], " Directory of D:\\SUB");
    assert!(lines[3].starts_with("LONGFILE TXT              1 "), "{}", lines[3]);
    assert!(lines[4].starts_with("README   TXT              5 "), "{}", lines[4]);
    assert_eq!(lines[5].trim(), "2 file(s)              6 bytes");
    // Two 512-byte clusters used on an empty 1.44 MB floppy + the SUB dir
    assert_eq!(lines[6].trim(), "1,454,592 bytes free");

    let out = run(&mut cpu, "DIR D:\\SUB");
    assert!(out.contains("GAME     EXE          1,234"), "{}", out);
    assert!(out.contains("\n.            <DIR>\n"), "{}", out);

    let out = run(&mut cpu, "DIR Z:");
    assert!(out.contains("COMMAND  COM          5,000"), "{}", out);

    assert_eq!(run(&mut cpu, "DIR Q:"), "Invalid drive specification");
    assert!(run(&mut cpu, "DIR D:\\SUB\\*.BAT").ends_with("File not found"));
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
