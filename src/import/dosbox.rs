//! DOSBox configuration files as a profile's settings, drives and commands.
//! Several files are read as DOSBox reads `-conf a -conf b`: the later
//! ones' settings over the earlier ones', and their `[autoexec]` sections
//! one after the other.

use super::{Imported, host_path};
use crate::keylayout::LayoutSetting;
use crate::mount::{MountCmd, parse_drive_letter, parse_imgmount_command, parse_mount_spec, tokenize};
use std::path::{Path, PathBuf};

/// A configuration file: its settings (section and key in lower case) and
/// its `[autoexec]` lines.
#[derive(Debug, Default)]
struct Conf {
    values: Vec<(String, String, String)>,
    autoexec: Vec<String>,
}

fn parse(text: &str) -> Conf {
    let mut conf = Conf::default();
    let mut section = String::new();
    for line in text.lines() {
        let trimmed = line.trim().trim_start_matches('\u{feff}');
        if let Some(name) = trimmed.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            section = name.trim().to_ascii_lowercase();
            continue;
        }
        if section == "autoexec" {
            if !trimmed.is_empty() && !trimmed.starts_with('#') {
                conf.autoexec.push(trimmed.to_string());
            }
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with(['#', ';']) {
            continue;
        }
        if let Some((key, value)) = trimmed.split_once('=') {
            conf.values.push((section.clone(), key.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }
    conf
}

/// A game whose DOSBox configuration is `texts` (in the order DOSBox reads
/// them), called `name`. Paths in them are relative to the directory DOSBox
/// runs in, the first of `bases` where they are (the files may have moved
/// since they were set up).
pub fn import(texts: &[&str], bases: &[PathBuf], name: &str, home: Option<&Path>) -> Imported {
    let mut imported = Imported { name: name.to_string(), ..Default::default() };
    let confs: Vec<Conf> = texts.iter().map(|t| parse(t)).collect();
    let mut gus_set = false;
    for conf in &confs {
        for (section, key, value) in &conf.values {
            gus_set |= section == "gus" && key == "gus";
            setting(&mut imported, section, key, value);
        }
    }
    // DOSBox has no Ultrasound unless asked for one.
    if !gus_set {
        imported.set("sound", "gus", "false");
    }
    let mut roots: Vec<(u8, PathBuf)> = Vec::new();
    // DOSBox starts on Z:, where GOG's lines "cd .." (and the like) go
    // nowhere; here they would be C:'s.
    let mut on_z = true;
    for line in confs.iter().flat_map(|c| &c.autoexec) {
        let command = line.trim_start_matches('@').trim();
        let verb = command.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
        if parse_drive_letter(&verb).is_some() && verb.ends_with(':') {
            on_z = verb == "z:";
        } else if on_z && (verb == "cd" || verb == "chdir" || verb.starts_with("cd.") || verb.starts_with("cd\\")) {
            continue;
        }
        autoexec_line(&mut imported, line, bases, home, &mut roots);
    }
    imported
}

/// One of DOSBox's settings, as rust-dos's.
fn setting(imported: &mut Imported, section: &str, key: &str, value: &str) {
    let lower = value.to_ascii_lowercase();
    let first = lower.split_whitespace().next().unwrap_or("");
    let bool_value = || match first {
        "true" | "on" | "1" | "yes" => Some("true"),
        "false" | "off" | "0" | "no" => Some("false"),
        _ => None,
    };
    let unknown = |imported: &mut Imported| {
        imported.warnings.push(format!("[{}] {}={} isn't imported", section, key, value));
    };
    match (section, key) {
        ("cpu", "cycles") => {
            let words: Vec<&str> = lower.split_whitespace().collect();
            let fixed = match words.as_slice() {
                ["fixed", n, ..] => n.parse::<u32>().ok(),
                [n] => n.parse::<u32>().ok(),
                _ => None,
            };
            match fixed {
                Some(n) => imported.set("emulator", "cycles", n.clamp(100, 2_000_000).to_string()),
                None if matches!(words.first(), Some(&"max" | &"auto")) => imported.set("emulator", "cycles", "max"),
                None => unknown(imported),
            }
        }
        ("cpu", "core") => match first {
            "auto" | "dynamic" => imported.set("emulator", "core", first),
            "normal" | "simple" | "full" => imported.set("emulator", "core", "normal"),
            _ => unknown(imported),
        },
        ("cpu", "cputype") => match first {
            f if f.starts_with("386") => imported.set("emulator", "cpu", "386"),
            f if f.starts_with("486") || f.starts_with("pentium") || f == "auto" => imported.set("emulator", "cpu", "486"),
            _ => unknown(imported),
        },
        ("dosbox", "machine") => match crate::video::adapter::Adapter::parse(first) {
            Some(adapter) => imported.set("emulator", "machine", adapter.name()),
            None => unknown(imported),
        },
        ("dosbox", "memsize") => match first.parse::<usize>() {
            Ok(mb) => imported.set("emulator", "memsize", mb.clamp(2, 64).to_string()),
            Err(_) => unknown(imported),
        },
        ("dos", "ems") => match first {
            "true" | "emsboard" | "emm386" => imported.set("emulator", "ems", "true"),
            "false" => imported.set("emulator", "ems", "false"),
            _ => unknown(imported),
        },
        ("dos", "umb") => match bool_value() {
            Some(b) => imported.set("emulator", "umb", b),
            None => unknown(imported),
        },
        ("dos", "keyboardlayout") => match LayoutSetting::parse(first) {
            Some(layout) => imported.set("emulator", "keyboard_layout", layout.name()),
            None if matches!(first, "auto" | "none" | "") => {}
            None => unknown(imported),
        },
        ("render", "aspect") => match bool_value() {
            Some(b) => imported.set("emulator", "aspect", b),
            None => unknown(imported),
        },
        ("sblaster", "sbtype") => match first {
            "sb1" | "sb2" => imported.set("sound", "sbtype", "sb2"),
            "sbpro1" | "sbpro2" => imported.set("sound", "sbtype", "sbpro2"),
            "sb16" | "sb16vibra" => imported.set("sound", "sbtype", "sb16"),
            "none" => imported.set("sound", "sbtype", "none"),
            _ => unknown(imported),
        },
        ("sblaster", "sbbase") => imported.set("sound", "sbbase", first.to_ascii_uppercase()),
        ("sblaster", "irq") => imported.set("sound", "irq", first),
        ("sblaster", "dma") => imported.set("sound", "dma", first),
        ("sblaster", "hdma") => imported.set("sound", "hdma", first),
        ("sblaster", "oplmode") => match first {
            "opl2" | "cms" => imported.set("sound", "opl", "opl2"),
            "dualopl2" | "opl3" | "opl3gold" => imported.set("sound", "opl", "opl3"),
            "auto" => {}
            _ => unknown(imported),
        },
        ("gus", "gus") => match bool_value() {
            Some(b) => imported.set("sound", "gus", b),
            None => unknown(imported),
        },
        ("gus", "gusbase") => imported.set("sound", "gusbase", first.to_ascii_uppercase()),
        ("gus", "gusirq") => imported.set("sound", "gusirq", first),
        ("gus", "gusdma") => imported.set("sound", "gusdma", first),
        ("gus", "ultradir") => imported.set("sound", "ultradir", value.to_string()),
        ("midi", "mpu401") if first == "none" => imported.set("sound", "midisynth", "none"),
        ("midi", "mididevice") => match first {
            "mt32" => imported.set("sound", "midisynth", "mt32"),
            "none" => imported.set("sound", "midisynth", "none"),
            _ => {}
        },
        ("speaker", "tandy") => match first {
            "auto" | "on" | "off" => imported.set("sound", "tandy", first),
            "true" => imported.set("sound", "tandy", "on"),
            "false" => imported.set("sound", "tandy", "off"),
            _ => unknown(imported),
        },
        ("speaker", "disney") if bool_value() == Some("true") => imported.set("sound", "lpt_dac", "disney"),
        ("joystick", "joysticktype") => match first {
            "auto" | "2axis" | "4axis" | "none" => imported.set("joystick", "joysticktype", first),
            "4axis_2" | "fcs" | "ch" => imported.set("joystick", "joysticktype", "4axis"),
            _ => unknown(imported),
        },
        _ => {}
    }
}

/// A line of `[autoexec]`: MOUNT and IMGMOUNT become the profile's drives,
/// with their host paths made absolute (a game's drives are mounted as it
/// starts and taken back as it ends); KEYB its keyboard layout; EXIT, which
/// would end rust-dos, and DOSBox's own commands go; the rest runs as it
/// is. `roots` are the directories of the drives mounted so far, for
/// images named by their DOS paths.
fn autoexec_line(imported: &mut Imported, line: &str, bases: &[PathBuf], home: Option<&Path>, roots: &mut Vec<(u8, PathBuf)>) {
    let working_dir = bases.first().map_or(Path::new("."), PathBuf::as_path);
    let command = line.trim_start_matches('@').trim();
    let tokens = tokenize(command).unwrap_or_default();
    let verb = tokens.first().map(|t| t.to_ascii_lowercase()).unwrap_or_default();
    let verb = verb.rsplit(['\\', ':']).next().unwrap_or(&verb).trim_end_matches(".com").to_string();
    match verb.as_str() {
        "mount" if tokens.get(1).is_some_and(|t| t.eq_ignore_ascii_case("-u")) => {}
        "mount" => {
            let Some(drive) = tokens.get(1).and_then(|t| parse_drive_letter(t)) else {
                imported.warnings.push(format!("[autoexec] {} isn't imported", line));
                return;
            };
            let mut rest = tokens[2..].to_vec();
            if let Some(path) = rest.first_mut() {
                *path = resolve(bases, path).to_string_lossy().into_owned();
            }
            match parse_mount_spec(drive, &rest, working_dir, home) {
                Ok(spec) => {
                    roots.push((drive, spec.path.clone()));
                    imported.drives.retain(|d| d.drive != drive);
                    imported.drives.push(spec);
                }
                Err(e) => imported.warnings.push(format!("[autoexec] {}: {}", line, e)),
            }
        }
        "imgmount" if tokens.get(1).is_some_and(|t| t.eq_ignore_ascii_case("-u")) => {}
        "imgmount" => {
            let mut args = Vec::new();
            let mut option_value = false;
            for (i, token) in tokens.iter().enumerate().skip(1) {
                // The images: host paths, or DOS paths on drives mounted
                // before.
                let image = i >= 2 && !option_value && !token.starts_with('-');
                option_value = token.starts_with('-') && !matches!(token.to_ascii_lowercase().as_str(), "-ro" | "-ioctl" | "-noioctl");
                let token = if image { image_path(token, bases, roots).to_string_lossy().into_owned() } else { token.clone() };
                args.push(if token.contains(char::is_whitespace) { format!("\"{}\"", token) } else { token });
            }
            match parse_imgmount_command(&args.join(" "), &|_| None, working_dir, home) {
                Ok(MountCmd::Mount(spec)) => {
                    imported.drives.retain(|d| d.drive != spec.drive);
                    imported.drives.push(spec);
                }
                Ok(_) => {}
                Err(e) => imported.warnings.push(format!("[autoexec] {}: {}", line, e)),
            }
        }
        "keyb" => match tokens.get(1).and_then(|t| LayoutSetting::parse(t.split(',').next().unwrap_or(t))) {
            Some(layout) => imported.set("emulator", "keyboard_layout", layout.name()),
            None => imported.warnings.push(format!("[autoexec] {} isn't imported", line)),
        },
        "exit" | "rescan" | "config" | "mixer" | "loadfix" | "boot" | "ipxnet" | "serial" | "intro" => {
            if !matches!(verb.as_str(), "exit" | "rescan") {
                imported.warnings.push(format!("[autoexec] {} isn't imported", line));
            }
        }
        _ => imported.autoexec.push(line.to_string()),
    }
}

/// An image IMGMOUNT names: a DOS path on a drive the lines before mounted
/// ("C:\GAME\CD.CUE"), or a host path from the working directory.
fn image_path(token: &str, bases: &[PathBuf], roots: &[(u8, PathBuf)]) -> PathBuf {
    if let (Some(drive), Some(rest)) = (token.get(..2).and_then(parse_drive_letter), token.get(2..))
        && let Some((_, root)) = roots.iter().rev().find(|(d, _)| *d == drive)
    {
        return host_path(root, rest.trim_start_matches(['\\', '/']));
    }
    resolve(bases, token)
}

/// A path relative to the first of `bases` it is found from, or else to the
/// first.
fn resolve(bases: &[PathBuf], rel: &str) -> PathBuf {
    let candidates = bases.iter().map(|base| host_path(base, rel));
    candidates.clone().find(|p| p.exists()).or_else(|| candidates.clone().next()).unwrap_or_else(|| PathBuf::from(rel))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk::DriveKind;
    use std::fs;

    const GOG_CONF: &str = "[sdl]\nfullscreen=true\n\n[dosbox]\nmachine=svga_s3\nmemsize=16\n\n[cpu]\ncore=auto\ncputype=auto\ncycles=fixed 12000\n\n[sblaster]\nsbtype=sb16\nsbbase=220\nirq=7\ndma=1\nhdma=5\noplmode=auto\n\n[dos]\nems=true\nkeyboardlayout=auto\n\n[autoexec]\n# Lines in this section will be run at startup.\n";
    const GOG_SINGLE: &str = "[autoexec]\n@echo off\ncd ..\ncd ..\nmount C \"..\"\nimgmount d \"..\\game.ins\" -t iso -fs iso\nc:\ncls\ncd game\ngame.exe\nexit\n";

    fn scratch(name: &str) -> PathBuf {
        let dir = PathBuf::from("target/test_import").join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("DOSBOX")).unwrap();
        fs::write(dir.join("GAME.INS"), "FILE \"GAME.GOG\" BINARY\n").unwrap();
        fs::canonicalize(&dir).unwrap()
    }

    #[test]
    fn a_gog_configuration_becomes_a_profile() {
        let dir = scratch("gog");
        let imported = import(&[GOG_CONF, GOG_SINGLE], &[dir.join("DOSBOX")], "The Game", None);
        let get = |key: &str| imported.settings.iter().find(|(_, k, _)| *k == key).map(|(_, _, v)| v.as_str());
        assert_eq!(get("cycles"), Some("12000"));
        assert_eq!(get("machine"), Some("svga"));
        assert_eq!(get("sbtype"), Some("sb16"));
        assert_eq!(get("gus"), Some("false"), "DOSBox has no Ultrasound unless asked");
        assert_eq!(get("fullscreen"), None, "a user's preference, not the game's");
        // The mounts are relative to DOSBox's directory; "cd .." on Z: goes.
        assert_eq!(imported.drives.len(), 2);
        assert_eq!(imported.drives[0].path, dir);
        assert_eq!(imported.drives[1].path, dir.join("GAME.INS"), "found in any case");
        assert_eq!(imported.drives[1].opts.kind, DriveKind::CdRom);
        assert_eq!(imported.autoexec, ["@echo off", "c:", "cls", "cd game", "game.exe"]);

        // The profile reads back as one.
        let text = imported.profile_text(None);
        let config = crate::config::parse(&text, Path::new("/"), None);
        assert!(config.warnings.is_empty(), "{:?}\n{}", config.warnings, text);
        assert_eq!(config.game_name.as_deref(), Some("The Game"));
        assert_eq!(config.drives.len(), 2);
        let prepared = crate::games::prepare("the-game", &crate::config::Settings::default(), &text, Path::new("/"), None).unwrap();
        assert_eq!(prepared.settings.cycles, crate::timer::CpuSpeed::Fixed(12000));
    }

    #[test]
    fn settings_map_onto_rust_dos_s() {
        let conf = "[cpu]\ncycles=max 80%\ncputype=386_prefetch\ncore=simple\n[dos]\nkeyboardlayout=de\numb=false\n[gus]\ngus=true\ngusbase=240\n[joystick]\njoysticktype=fcs\n[speaker]\ndisney=true\n[sblaster]\nsbtype=gb\n";
        let imported = import(&[conf], &[PathBuf::from("/")], "x", None);
        let get = |key: &str| imported.settings.iter().find(|(_, k, _)| *k == key).map(|(_, _, v)| v.as_str());
        assert_eq!(get("cycles"), Some("max"));
        assert_eq!(get("cpu"), Some("386"));
        assert_eq!(get("core"), Some("normal"));
        assert_eq!(get("keyboard_layout"), Some("gr"));
        assert_eq!(get("umb"), Some("false"));
        assert_eq!(get("gus"), Some("true"));
        assert_eq!(get("joysticktype"), Some("4axis"));
        assert_eq!(get("lpt_dac"), Some("disney"));
        assert_eq!(get("sbtype"), None);
        assert!(imported.warnings[0].contains("sbtype=gb"), "{:?}", imported.warnings);
    }

    #[test]
    fn images_named_by_their_dos_path_are_found_on_the_drives_mounted_before() {
        let dir = scratch("dospath");
        fs::create_dir_all(dir.join("cd")).unwrap();
        fs::write(dir.join("cd/game.cue"), "").unwrap();
        let conf = "[autoexec]\nmount c .\nimgmount d c:\\cd\\GAME.CUE -t cdrom\nkeyb fr\n";
        let imported = import(&[conf], std::slice::from_ref(&dir), "x", None);
        assert_eq!(imported.drives[1].path, dir.join("cd/game.cue"));
        assert!(imported.settings.iter().any(|(_, k, v)| *k == "keyboard_layout" && v == "fr"));
        assert!(imported.autoexec.is_empty());
    }
}
