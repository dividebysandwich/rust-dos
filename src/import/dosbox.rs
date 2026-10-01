//! DOSBox configuration files as a profile's settings, drives and commands.
//! Several files are read as DOSBox reads `-conf a -conf b`: the later
//! ones' settings over the earlier ones', and their `[autoexec]` sections
//! one after the other.

use super::{Imported, host_path};
use crate::keylayout::LayoutSetting;
use crate::mount::{MountCmd, PathContext, parse_drive_letter, parse_mount_tokens, tokenize};
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

/// Whether `text` is a DOSBox configuration rather than rust-dos's: it has
/// one of DOSBox's own sections.
pub fn is_dosbox_conf(text: &str) -> bool {
    text.lines().map(str::trim).any(|line| {
        let line = line.to_ascii_lowercase();
        ["[dosbox]", "[cpu]", "[sdl]", "[render]", "[sblaster]", "[dos]", "[speaker]", "[midi]"].contains(&line.as_str())
    })
}

/// A game whose DOSBox configuration is `texts` (in the order DOSBox reads
/// them), called `name`. Paths in them are relative to the directory DOSBox
/// runs in, the first of `bases` where they are (the files may have moved
/// since they were set up).
pub fn import(texts: &[&str], bases: &[PathBuf], name: &str, home: Option<&Path>) -> Imported {
    import_in(texts, bases, name, home, None)
}

/// `import`, with `c_root` as C: if the configuration mounts no C: of its
/// own, as DOSBox Pure has a game's zip or folder, and the commands run on
/// it rather than on Z:.
pub fn import_in(texts: &[&str], bases: &[PathBuf], name: &str, home: Option<&Path>, c_root: Option<&Path>) -> Imported {
    let imported = import_from(texts, bases, name, home, true);
    match c_root {
        Some(root) if !imported.drives.iter().any(|d| d.drive == crate::disk::DRIVE_C) => {
            let mut imported = import_from(texts, bases, name, home, false);
            let opts = Default::default();
            imported.drives.insert(0, crate::mount::MountSpec { drive: crate::disk::DRIVE_C, path: root.to_path_buf(), opts });
            imported.autoexec.insert(0, "C:".to_string());
            imported
        }
        _ => imported,
    }
}

fn import_from(texts: &[&str], bases: &[PathBuf], name: &str, home: Option<&Path>, mut on_z: bool) -> Imported {
    let mut imported = Imported { name: name.to_string(), ..Default::default() };
    let confs: Vec<Conf> = texts.iter().map(|t| parse(t)).collect();
    let mut gus_set = false;
    // DOSBox Staging's cycles, which the old `cycles` overrides, and the
    // palette of a monochrome machine.
    let (mut legacy_cycles, mut cpu_cycles, mut cpu_cycles_protected, mut palette) = (false, None, None, None);
    for conf in &confs {
        for (section, key, value) in &conf.values {
            gus_set |= section == "gus" && key == "gus";
            match (section.as_str(), key.as_str()) {
                ("cpu", "cycles") => legacy_cycles |= !value.is_empty(),
                ("cpu", "cpu_cycles") => cpu_cycles = Some(value.to_ascii_lowercase()),
                ("cpu", "cpu_cycles_protected") => cpu_cycles_protected = Some(value.to_ascii_lowercase()),
                ("render", "monochrome_palette") => palette = Some(value.to_ascii_lowercase()),
                _ => {}
            }
            setting(&mut imported, section, key, value);
        }
    }
    // DOSBox has no Ultrasound unless asked for one.
    if !gus_set {
        imported.set("sound", "gus", "false");
    }
    if !legacy_cycles && let Some(real) = cpu_cycles {
        let protected = cpu_cycles_protected.unwrap_or_else(|| "60000".to_string());
        let fixed = |n: &str| n.parse::<u32>().ok().map(|n| n.clamp(100, 2_000_000));
        // A real-mode speed and a faster one for protected mode is what
        // rust-dos's `auto N` does.
        let speed = match (real.as_str(), protected.as_str()) {
            ("max", _) => Some("max".to_string()),
            (n, "auto") => fixed(n).map(|n| n.to_string()),
            (n, _) => fixed(n).map(|n| format!("auto {}", n)),
        };
        match speed {
            Some(speed) => imported.set("emulator", "cycles", speed),
            None => imported.warnings.push(format!("[cpu] cpu_cycles={} isn't imported", real)),
        }
    }
    let machine = imported.settings.iter().find(|(_, k, _)| *k == "machine").map(|(_, _, v)| v.clone());
    let mono = imported.settings.iter().any(|(_, k, _)| *k == "monochrome");
    if let Some(palette) = palette
        && (mono || machine.as_deref() == Some("hercules"))
    {
        match palette.as_str() {
            "white" | "paperwhite" => imported.set("emulator", "monochrome", "white"),
            "amber" | "green" => imported.set("emulator", "monochrome", palette),
            _ => {}
        }
    }
    // A SoundFont of the game's, found where its other files are.
    if let Some(i) = imported.settings.iter().position(|(_, k, _)| *k == "soundfont") {
        let font = resolve(bases, &imported.settings[i].2);
        if crate::hostfs::is_file(&font) {
            imported.settings[i].2 = font.display().to_string();
        } else {
            let (_, _, name) = imported.settings.remove(i);
            imported.warnings.push(format!("[fluidsynth] soundfont={} isn't there; rust-dos's own is used", name));
        }
    }
    let mut roots: Vec<(u8, PathBuf)> = Vec::new();
    // DOSBox starts on Z:, where GOG's lines "cd .." (and the like) go
    // nowhere; here they would be C:'s.
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
    imported.drop_invalid();
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
    // A port, in hex with or without 0x.
    let port = || first.trim_start_matches("0x").to_ascii_uppercase();
    match (section, key) {
        ("cpu", "cycles") => {
            let words: Vec<&str> = lower.split_whitespace().collect();
            let cycles = |n: &str| n.parse::<u32>().ok().map(|n| n.clamp(100, 2_000_000));
            // `auto` may be followed by the real-mode speed, and like
            // `max`, by a share of the host (90%) and a limit, which
            // aren't imported.
            let speed = match words.as_slice() {
                ["fixed", n, ..] | [n] if cycles(n).is_some() => cycles(n).map(|n| n.to_string()),
                ["auto", n, ..] if cycles(n).is_some() => cycles(n).map(|n| format!("auto {}", n)),
                ["auto", ..] => Some("auto".to_string()),
                ["max", ..] => Some("max".to_string()),
                // DOSBox Staging leaves it empty for its cpu_cycles.
                [] => return,
                _ => None,
            };
            match speed {
                Some(speed) => imported.set("emulator", "cycles", speed),
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
            f if f.starts_with("486") || f == "auto" => imported.set("emulator", "cpu", "486"),
            // The Pentium MMX, and the later ones with MMX as the nearest
            // there is; the Pentium Pro had none.
            f if f.starts_with("pentium_mmx") || f.starts_with("pentium_ii") || f == "experimental" => {
                imported.set("emulator", "cpu", "pentium_mmx")
            }
            f if f.starts_with("pentium") || f.starts_with("ppro") => imported.set("emulator", "cpu", "pentium"),
            _ => unknown(imported),
        },
        // DOSBox's default, svga_s3, is what DOS games get from it: the
        // plain Super VGA, not the S3 whose registers Windows' drivers use.
        ("dosbox", "machine") => match crate::video::adapter::Adapter::parse(first) {
            Some(crate::video::adapter::Adapter::S3) => imported.set("emulator", "machine", "svga"),
            Some(adapter) => imported.set("emulator", "machine", adapter.name()),
            // DOSBox-X's other S3 chips and the like, as svga_s3.
            None if first.starts_with("svga_") || first.starts_with("vesa_") => imported.set("emulator", "machine", "svga"),
            None => match first {
                "cga_mono" => {
                    imported.set("emulator", "machine", "cga");
                    imported.set("emulator", "monochrome", "white");
                }
                "cga_composite" | "cga_composite2" | "pcjr_composite" => {
                    imported.set("emulator", "machine", &first[..first.find('_').unwrap_or(first.len())]);
                    imported.set("emulator", "composite", "on");
                }
                "cga_rgb" => imported.set("emulator", "machine", "cga"),
                "mda" => imported.set("emulator", "machine", "hercules"),
                "mcga" => imported.set("emulator", "machine", "vga"),
                "jega" => imported.set("emulator", "machine", "ega"),
                _ => unknown(imported),
            },
        },
        // DOSBox Staging's composite output of a CGA or PCjr.
        ("composite", "composite") => match first {
            "auto" | "on" | "off" => imported.set("emulator", "composite", first),
            _ => unknown(imported),
        },
        ("composite", "era") => match first {
            "old" | "new" => imported.set("emulator", "composite_era", first),
            "auto" => {}
            _ => unknown(imported),
        },
        ("dosbox", "memsize") => match first.parse::<usize>() {
            Ok(mb) => imported.set("emulator", "memsize", mb.clamp(crate::config::MIN_MEMSIZE, crate::config::MAX_MEMSIZE).to_string()),
            Err(_) => unknown(imported),
        },
        ("dos", "ems") => match first {
            "true" | "emsboard" | "emm386" => imported.set("emulator", "ems", "true"),
            "false" => imported.set("emulator", "ems", "false"),
            _ => unknown(imported),
        },
        ("dos", "ver") => match crate::config::DosVersion::parse(first) {
            Some(version) => imported.set("emulator", "dos_version", version.name()),
            None if first.is_empty() => {}
            None => unknown(imported),
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
        // DOSBox Staging's: auto, on, square-pixels or stretch.
        ("render", "aspect") => match (bool_value(), first) {
            (Some(b), _) => imported.set("emulator", "aspect", b),
            (None, "auto") => imported.set("emulator", "aspect", "true"),
            (None, "square-pixels" | "stretch") => imported.set("emulator", "aspect", "false"),
            _ => unknown(imported),
        },
        ("sblaster", "sbtype") => match first {
            "sb1" | "sb2" => imported.set("sound", "sbtype", "sb2"),
            // DOSBox Staging's ESS AudioDrive, a Sound Blaster Pro to games.
            "sbpro1" | "sbpro2" | "ess" => imported.set("sound", "sbtype", "sbpro2"),
            "sb16" | "sb16vibra" => imported.set("sound", "sbtype", "sb16"),
            "none" => imported.set("sound", "sbtype", "none"),
            _ => unknown(imported),
        },
        ("sblaster", "sbbase") => imported.set("sound", "sbbase", port()),
        ("sblaster", "irq") => imported.set("sound", "irq", first),
        ("sblaster", "dma") => imported.set("sound", "dma", first),
        // An SB16's 16-bit sound on its 8-bit channel, as DOSBox has it
        // with an hdma below 4, isn't; the card's own channel serves.
        ("sblaster", "hdma") if first.parse::<u8>().is_ok_and(|d| d < 4) => {}
        ("sblaster", "hdma") => imported.set("sound", "hdma", first),
        ("sblaster", "oplmode") => match first {
            "opl2" | "cms" => imported.set("sound", "opl", "opl2"),
            "dualopl2" | "opl3" | "opl3gold" | "esfm" => imported.set("sound", "opl", "opl3"),
            "auto" => {}
            _ => unknown(imported),
        },
        ("gus", "gus") => match bool_value() {
            Some(b) => imported.set("sound", "gus", b),
            None => unknown(imported),
        },
        ("gus", "gusbase") => imported.set("sound", "gusbase", port()),
        ("gus", "gusirq") => imported.set("sound", "gusirq", first),
        ("gus", "gusdma") => imported.set("sound", "gusdma", first),
        // DOSBox's own default is no directory of the game's: rust-dos's
        // built-in patches serve.
        ("gus", "ultradir") if !value.trim().eq_ignore_ascii_case("C:\\ULTRASND") => {
            imported.set("sound", "ultradir", value.to_string())
        }
        ("midi", "mpu401") if first == "none" => imported.set("sound", "midisynth", "none"),
        ("midi", "mididevice") => match first {
            "mt32" => imported.set("sound", "midisynth", "mt32"),
            "soundcanvas" => imported.set("sound", "midisynth", "sc55"),
            "fluidsynth" => imported.set("sound", "midisynth", "soundfont"),
            "none" => imported.set("sound", "midisynth", "none"),
            _ => {}
        },
        // DOSBox Staging's FluidSynth and Munt: a game's own SoundFont (DOSBox
        // looks for it in its soundfonts folder too, which rust-dos doesn't).
        ("fluidsynth", "soundfont") if !value.trim().is_empty() && !lower.starts_with("default") => {
            let font = value.split_whitespace().next().unwrap_or(value);
            imported.set("sound", "soundfont", font.to_string())
        }
        ("mt32", "model") => match first {
            "cm32l" | "cm32ln" | "cm32l_100" | "cm32l_101" | "cm32l_102" | "cm32ln_100" => imported.set("sound", "mt32model", "cm32l"),
            f if f.starts_with("mt32") => imported.set("sound", "mt32model", "mt32"),
            "auto" => {}
            _ => unknown(imported),
        },
        // DOSBox Staging's names for the Sound Canvas models.
        ("soundcanvas", "soundcanvas_model") => match first {
            "sc55" => imported.set("sound", "sc55model", "mk1"),
            "sc55mk2" | "sc55mk2_100" => imported.set("sound", "sc55model", "mk2"),
            "sc55mk2_101" => imported.set("sound", "sc55model", "mk2-v1.01"),
            "sc55_100" | "sc55_110" | "sc55_120" | "sc55_121" | "sc55_200" => {
                let version = &first["sc55_".len()..];
                imported.set("sound", "sc55model", format!("mk1-v{}.{}", &version[..1], &version[1..]))
            }
            "auto" => {}
            _ => unknown(imported),
        },
        ("speaker", "tandy") => match first {
            "auto" | "on" | "off" => imported.set("sound", "tandy", first),
            "true" => imported.set("sound", "tandy", "on"),
            "false" => imported.set("sound", "tandy", "off"),
            _ => unknown(imported),
        },
        ("speaker", "disney") if bool_value() == Some("true") => imported.set("sound", "lpt_dac", "disney"),
        // DOSBox-X's 3dfx card. Its default, auto, is in every config it
        // writes, so only a card asked for by name is put in.
        ("voodoo", "voodoo_card") | ("pci", "voodoo") => match first {
            "software" | "opengl" => {
                imported.set("emulator", "voodoo", "true");
                imported.set("emulator", "voodoo_renderer", first);
            }
            "auto" | "false" | "off" | "none" => {}
            _ => unknown(imported),
        },
        ("voodoo", "voodoo_maxmem") | ("pci", "voodoo_maxmem") => match bool_value() {
            Some("true") => imported.set("emulator", "voodoo_memory", "12"),
            Some(_) => imported.set("emulator", "voodoo_memory", "4"),
            None => unknown(imported),
        },
        ("ipx", "ipx") => match bool_value() {
            Some(b) => imported.set("network", "ipx", b),
            None => unknown(imported),
        },
        // DOSBox-X's NE2000, and DOSBox Staging's.
        ("ne2000" | "ethernet", "ne2000") => match bool_value() {
            Some(b) => imported.set("network", "ne2000", b),
            None => unknown(imported),
        },
        ("ne2000" | "ethernet", "nicbase") => imported.set("network", "nicbase", port()),
        ("ne2000" | "ethernet", "nicirq") => imported.set("network", "nicirq", first),
        // DOSBox's default address would be every imported game's.
        ("ne2000" | "ethernet", "macaddr") if !first.eq_ignore_ascii_case("ac:de:48:88:99:aa") => {
            imported.set("network", "macaddr", first)
        }
        // DOSBox-X's printer, where it differs from its defaults (on,
        // PNG pages) and rust-dos's (on, PDF documents) alike.
        ("printer", "printer") if bool_value() == Some("false") => imported.set("printer", "output", "none"),
        ("printer", "printoutput") if first == "printer" => imported.set("printer", "output", "printer"),
        ("parallel", "parallel1") => match first {
            "disabled" | "none" => imported.set("printer", "output", "none"),
            "file" => imported.set("printer", "output", "file"),
            _ => {}
        },
        ("joystick", "joysticktype") => match first {
            "auto" | "2axis" | "4axis" | "none" => imported.set("joystick", "joysticktype", first),
            "4axis_2" | "fcs" | "ch" => imported.set("joystick", "joysticktype", "4axis"),
            "hidden" | "disabled" => imported.set("joystick", "joysticktype", "none"),
            _ => unknown(imported),
        },
        _ => {}
    }
}

/// The sections of DOSBox's settings that are imported, for CONFIG -SET
/// with a setting's name alone.
const SECTIONS: [&str; 8] = ["cpu", "dosbox", "dos", "render", "sblaster", "gus", "midi", "speaker"];

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
        // DOSBox Staging's MOUNT took in IMGMOUNT; both take images, by
        // their host paths or their DOS paths on drives mounted before.
        "mount" | "imgmount" => {
            let locate = |path: &str| Some(image_path(path, bases, roots));
            let paths = PathContext { base: working_dir, config_dir: None, home, locate: &locate };
            match parse_mount_tokens(&tokens[1..], &paths) {
                Ok(MountCmd::Mount(spec)) => {
                    if !crate::hostfs::is_file(&spec.path) {
                        roots.push((spec.drive, spec.path.clone()));
                    }
                    imported.drives.retain(|d| d.drive != spec.drive);
                    imported.drives.push(spec);
                }
                // The changes go where DOSBox had them.
                Ok(MountCmd::Overlay(drive, dir)) => match imported.drives.iter_mut().find(|d| d.drive == drive) {
                    Some(spec) => spec.opts.overlay = Some(dir),
                    None => imported.warnings.push(format!("[autoexec] {}: no drive to overlay", line)),
                },
                Ok(_) => {}
                Err(e) => imported.warnings.push(format!("[autoexec] {}: {}", line, e)),
            }
        }
        "keyb" => match tokens.get(1).and_then(|t| LayoutSetting::parse(t.split(',').next().unwrap_or(t))) {
            Some(layout) => imported.set("emulator", "keyboard_layout", layout.name()),
            None => imported.warnings.push(format!("[autoexec] {} isn't imported", line)),
        },
        // BOOT runs as it is, with the images it names by their host paths.
        "boot" => {
            let mut line = vec![tokens[0].clone()];
            let mut args = tokens[1..].iter();
            while let Some(arg) = args.next() {
                if arg.starts_with('-') {
                    line.push(arg.clone());
                    if arg.eq_ignore_ascii_case("-l")
                        && let Some(drive) = args.next()
                    {
                        line.push(drive.clone());
                    }
                } else {
                    let path = image_path(arg, bases, roots).display().to_string();
                    line.push(if path.contains(' ') { format!("\"{}\"", path) } else { path });
                }
            }
            imported.autoexec.push(line.join(" "));
        }
        // DOSBox's IPX network is another protocol than rust-dos's LAN.
        "ipxnet" => imported
            .warnings
            .push(format!("[autoexec] {} isn't imported: rust-dos joins a LAN with LAN HOST and LAN JOIN", line)),
        // Programs load from 64 KB on, where LOADFIX would put them: the
        // program it names runs as it is.
        "loadfix" => {
            let mut args = tokens[1..].iter().skip_while(|t| t.starts_with(['-', '/']));
            if let Some(program) = args.next() {
                let rest: Vec<&str> = std::iter::once(program.as_str()).chain(args.map(String::as_str)).collect();
                let at = command.find(rest[0]).unwrap_or(0);
                imported.autoexec.push(command[at..].to_string());
            }
        }
        // CONFIG -SET "section key=value", as the game's setting.
        "config" => {
            let args: Vec<&str> = tokens[1..].iter().map(String::as_str).collect();
            match args.split_first() {
                Some((flag, rest)) if flag.eq_ignore_ascii_case("-set") && !rest.is_empty() => {
                    let joined = rest.join(" ");
                    let (head, value) = joined.split_once('=').unwrap_or((joined.as_str(), ""));
                    let words: Vec<&str> = head.split_whitespace().collect();
                    let (before, warnings) = (imported.settings.clone(), imported.warnings.len());
                    match words.as_slice() {
                        [section, key] => setting(imported, &section.to_ascii_lowercase(), &key.to_ascii_lowercase(), value.trim()),
                        [key] => {
                            for section in SECTIONS {
                                setting(imported, section, &key.to_ascii_lowercase(), value.trim());
                            }
                        }
                        _ => {}
                    }
                    if imported.settings == before && imported.warnings.len() == warnings {
                        imported.warnings.push(format!("[autoexec] {} isn't imported", line));
                    }
                }
                _ => imported.warnings.push(format!("[autoexec] {} isn't imported", line)),
            }
        }
        "exit" | "rescan" | "serial" | "intro" => {
            if !matches!(verb.as_str(), "exit" | "rescan") {
                imported.warnings.push(format!("[autoexec] {} isn't imported", line));
            }
        }
        _ => imported.autoexec.push(line.to_string()),
    }
}

/// A path MOUNT names: a DOS path on a drive the lines before mounted
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
    candidates.clone().find(|p| crate::hostfs::exists(p)).or_else(|| candidates.clone().next()).unwrap_or_else(|| PathBuf::from(rel))
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
    fn dosbox_staging_s_sound_canvas_is_ours() {
        let imported = import(&["[midi]\nmididevice=soundcanvas\n[soundcanvas]\nsoundcanvas_model=sc55_121\n"], &[PathBuf::from("/")], "x", None);
        let get = |key: &str| imported.settings.iter().find(|(_, k, _)| *k == key).map(|(_, _, v)| v.as_str());
        assert_eq!(get("midisynth"), Some("sc55"));
        assert_eq!(get("sc55model"), Some("mk1-v1.21"));
        let config = crate::config::parse(&imported.profile_text(None), Path::new("/"), None);
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
        assert_eq!(config.sound.sc55model, "mk1-v1.21");
    }

    #[test]
    fn a_dosbox_x_virge_stays_a_virge() {
        let machine = |conf: &str| {
            let imported = import(&[conf], &[PathBuf::from("/")], "x", None);
            imported.settings.iter().find(|(_, k, _)| *k == "machine").map(|(_, _, v)| v.clone())
        };
        assert_eq!(machine("[dosbox]\nmachine=svga_s3virge\n").as_deref(), Some("svga_s3virge"));
        assert_eq!(machine("[dosbox]\nmachine=svga_s3virgevx\n").as_deref(), Some("svga_s3virgevx"));
        assert_eq!(machine("[dosbox]\nmachine=svga_et4000\n").as_deref(), Some("svga_et4000"));
    }

    #[test]
    fn network_settings_map_onto_rust_dos_s() {
        let conf = "[ipx]\nipx=true\n[ne2000]\nne2000=true\nnicbase=280\nnicirq=5\nmacaddr=AC:DE:48:88:99:AA\n[autoexec]\nipxnet connect 10.0.0.1\n";
        let imported = import(&[conf], &[PathBuf::from("/")], "x", None);
        let get = |key: &str| imported.settings.iter().find(|(_, k, _)| *k == key).map(|(s, _, v)| (*s, v.as_str()));
        assert_eq!(get("ipx"), Some(("network", "true")));
        assert_eq!(get("ne2000"), Some(("network", "true")));
        assert_eq!(get("nicbase"), Some(("network", "280")));
        assert_eq!(get("nicirq"), Some(("network", "5")));
        assert_eq!(get("macaddr"), None, "DOSBox's default address");
        assert!(imported.warnings.iter().any(|w| w.contains("LAN JOIN")), "{:?}", imported.warnings);
        let staging = import(&["[ethernet]\nne2000=true\nmacaddr=02:00:5e:00:00:01\n"], &[PathBuf::from("/")], "x", None);
        let text = staging.profile_text(None);
        let config = crate::config::parse(&text, Path::new("/"), None);
        assert!(config.warnings.is_empty(), "{:?}\n{}", config.warnings, text);
        assert!(config.network.ne2000);
        assert_eq!(config.network.mac.map(|m| m.to_string()).as_deref(), Some("02:00:5E:00:00:01"));
    }

    #[test]
    fn dosbox_x_printers_map_where_they_differ() {
        let output = |conf: &str| {
            let imported = import(&[conf], &[PathBuf::from("/")], "x", None);
            imported.settings.iter().find(|(_, k, _)| *k == "output").map(|(_, _, v)| v.clone())
        };
        assert_eq!(output("[printer]\nprinter=true\nprintoutput=png\n[parallel]\nparallel1=printer\n"), None);
        assert_eq!(output("[printer]\nprinter=false\n").as_deref(), Some("none"));
        assert_eq!(output("[printer]\nprintoutput=printer\n").as_deref(), Some("printer"));
        assert_eq!(output("[parallel]\nparallel1=file dev:lpt1\n").as_deref(), Some("file"));
        assert_eq!(output("[parallel]\nparallel1=disabled\n").as_deref(), Some("none"));
    }

    #[test]
    fn settings_map_onto_rust_dos_s() {
        let conf = "[cpu]\ncycles=max 80%\ncputype=386_prefetch\ncore=simple\n[dos]\nkeyboardlayout=de\numb=false\nver=7.1\n[gus]\ngus=true\ngusbase=240\n[joystick]\njoysticktype=fcs\n[speaker]\ndisney=true\n[sblaster]\nsbtype=gb\n";
        let imported = import(&[conf], &[PathBuf::from("/")], "x", None);
        let get = |key: &str| imported.settings.iter().find(|(_, k, _)| *k == key).map(|(_, _, v)| v.as_str());
        assert_eq!(get("cycles"), Some("max"));
        for (cycles, imported) in [("auto", "auto"), ("auto 5000 max 90% limit 60000", "auto 5000"), ("auto max", "auto"), ("fixed 12000", "12000"), ("8000", "8000")] {
            let conf = format!("[cpu]\ncycles={}\n", cycles);
            let got = import(&[conf.as_str()], &[PathBuf::from("/")], "x", None);
            let got = got.settings.iter().find(|(_, k, _)| *k == "cycles").map(|(_, _, v)| v.as_str());
            assert_eq!(got, Some(imported), "cycles={}", cycles);
        }
        assert_eq!(get("cpu"), Some("386"));
        assert_eq!(get("core"), Some("normal"));
        let pentium = import(&["[cpu]\ncputype=pentium_slow\n"], &[PathBuf::from("/")], "x", None);
        assert_eq!(pentium.settings.iter().find(|(_, k, _)| *k == "cpu").map(|(_, _, v)| v.as_str()), Some("pentium"));
        let mmx = import(&["[cpu]\ncputype=pentium_mmx\n"], &[PathBuf::from("/")], "x", None);
        assert_eq!(mmx.settings.iter().find(|(_, k, _)| *k == "cpu").map(|(_, _, v)| v.as_str()), Some("pentium_mmx"));
        assert_eq!(get("keyboard_layout"), Some("gr"));
        assert_eq!(get("umb"), Some("false"));
        assert_eq!(get("dos_version"), Some("7.10"));
        assert_eq!(get("gus"), Some("true"));
        assert_eq!(get("joysticktype"), Some("4axis"));
        assert_eq!(get("lpt_dac"), Some("disney"));
        assert_eq!(get("sbtype"), None);
        assert!(imported.warnings[0].contains("sbtype=gb"), "{:?}", imported.warnings);
    }

    #[test]
    fn a_3dfx_card_asked_for_by_name_is_put_in() {
        let get = |conf: &str, key: &str| {
            let imported = import(&[conf], &[PathBuf::from("/")], "x", None);
            imported.settings.iter().find(|(_, k, _)| *k == key).map(|(_, _, v)| v.clone())
        };
        let x = "[voodoo]\nvoodoo_card=software\nvoodoo_maxmem=false\n";
        assert_eq!(get(x, "voodoo").as_deref(), Some("true"));
        assert_eq!(get(x, "voodoo_renderer").as_deref(), Some("software"));
        assert_eq!(get(x, "voodoo_memory").as_deref(), Some("4"));
        assert_eq!(get("[pci]\nvoodoo=opengl\n", "voodoo_renderer").as_deref(), Some("opengl"));
        // DOSBox-X's default, auto, doesn't.
        assert_eq!(get("[voodoo]\nvoodoo_card=auto\n", "voodoo"), None);
        let text = import(&[x], &[PathBuf::from("/")], "x", None).profile_text(None);
        let config = crate::config::parse(&text, Path::new("/"), None);
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
        assert_eq!(config.voodoo_memory, Some(crate::voodoo::Board::Standard));
    }

    /// A system booted from a disk image, as DOSBox runs Windows 95.
    #[test]
    fn boot_runs_with_its_images_found() {
        let dir = scratch("boot");
        fs::write(dir.join("floppy.img"), "").unwrap();
        let conf = "[autoexec]\nimgmount c win95.img\nboot -l c\nboot floppy.img\n";
        let imported = import(&[conf], std::slice::from_ref(&dir), "x", None);
        assert_eq!(imported.drives[0].path, dir.join("win95.img"));
        assert_eq!(imported.autoexec, ["boot -l c".to_string(), format!("boot {}", dir.join("floppy.img").display())]);
        assert!(imported.warnings.is_empty(), "{:?}", imported.warnings);

        // A disk mounted by number is the profile's too, by its number.
        let conf = "[autoexec]\nimgmount 2 win95.img -size 512,63,64,520 -fs none\nboot -l c\n";
        let imported = import(&[conf], std::slice::from_ref(&dir), "x", None);
        assert_eq!(imported.drives[0].drive, crate::disk::numbered_drive(2));
        let config = crate::config::parse(&imported.profile_text(None), Path::new("/"), None);
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
        assert_eq!(config.drives, imported.drives);
    }

    #[test]
    fn images_named_by_their_dos_path_are_found_on_the_drives_mounted_before() {
        let dir = scratch("dospath");
        fs::create_dir_all(dir.join("cd")).unwrap();
        fs::write(dir.join("cd/game.cue"), "").unwrap();
        let conf = "[autoexec]\nmount c .\nimgmount d c:\\cd\\GAME.CUE -t cdrom\nmount -t cdrom e c:\\cd\\GAME.CUE\nkeyb fr\n";
        let imported = import(&[conf], std::slice::from_ref(&dir), "x", None);
        assert_eq!(imported.drives[0].path, dir);
        assert_eq!(imported.drives[1].path, dir.join("cd/game.cue"));
        // DOSBox Staging's MOUNT, which took in IMGMOUNT.
        assert_eq!((imported.drives[2].drive, &imported.drives[2].path), (4, &dir.join("cd/game.cue")));
        assert_eq!(imported.drives[2].opts.kind, DriveKind::CdRom);
        assert!(imported.settings.iter().any(|(_, k, v)| *k == "keyboard_layout" && v == "fr"));
        assert!(imported.autoexec.is_empty());
    }

    fn get(imported: &Imported, key: &str) -> Option<String> {
        imported.settings.iter().find(|(_, k, _)| *k == key).map(|(_, _, v)| v.clone())
    }

    #[test]
    fn dosbox_staging_s_cpu_cycles_are_ours() {
        let cycles = |conf: &str| get(&import(&[conf], &[PathBuf::from("/")], "x", None), "cycles");
        assert_eq!(cycles("[cpu]\ncpu_cycles=3000\ncpu_cycles_protected=60000\n").as_deref(), Some("auto 3000"));
        assert_eq!(cycles("[cpu]\ncpu_cycles=12000\ncpu_cycles_protected=auto\n").as_deref(), Some("12000"));
        assert_eq!(cycles("[cpu]\ncpu_cycles=8000\ncpu_cycles_protected=max\n").as_deref(), Some("auto 8000"));
        assert_eq!(cycles("[cpu]\ncpu_cycles=max\n").as_deref(), Some("max"));
        // The old setting, which DOSBox Staging leaves empty, wins if set.
        assert_eq!(cycles("[cpu]\ncycles=\ncpu_cycles=5000\ncpu_cycles_protected=auto\n").as_deref(), Some("5000"));
        assert_eq!(cycles("[cpu]\ncycles=fixed 20000\ncpu_cycles=5000\n").as_deref(), Some("20000"));
        assert!(import(&["[cpu]\ncycles=\n"], &[PathBuf::from("/")], "x", None).warnings.is_empty());
    }

    #[test]
    fn other_machines_are_the_nearest_there_is() {
        let imported = |conf: &str| import(&[conf], &[PathBuf::from("/")], "x", None);
        for (machine, ours) in [("svga_s3trio64", "svga"), ("vesa_nolfb", "svga"), ("mcga", "vga"), ("mda", "hercules"), ("cga_rgb", "cga")] {
            assert_eq!(get(&imported(&format!("[dosbox]\nmachine={}\n", machine)), "machine").as_deref(), Some(ours), "{}", machine);
        }
        let mono = imported("[dosbox]\nmachine=cga_mono\n[render]\nmonochrome_palette=amber\n");
        assert_eq!((get(&mono, "machine").as_deref(), get(&mono, "monochrome").as_deref()), (Some("cga"), Some("amber")));
        let composite = imported("[dosbox]\nmachine=pcjr_composite\n[composite]\nera=old\n");
        assert_eq!(get(&composite, "machine").as_deref(), Some("pcjr"));
        assert_eq!(get(&composite, "composite").as_deref(), Some("on"));
        assert_eq!(get(&composite, "composite_era").as_deref(), Some("old"));
        // A colour machine stays in colour.
        assert_eq!(get(&imported("[render]\nmonochrome_palette=green\n"), "monochrome"), None);
        assert_eq!(get(&imported("[render]\naspect=auto\n"), "aspect").as_deref(), Some("true"));
        assert_eq!(get(&imported("[render]\naspect=square-pixels\n"), "aspect").as_deref(), Some("false"));
        let config = crate::config::parse(&composite.profile_text(None), Path::new("/"), None);
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
    }

    #[test]
    fn sound_settings_rust_dos_can_t_have_are_left_out() {
        let conf = "[sblaster]\nsbtype=ess\nsbbase=0x240\nirq=4\ndma=1\nhdma=1\noplmode=esfm\n[gus]\ngus=true\ngusbase=0x260\n[midi]\nmididevice=fluidsynth\n[fluidsynth]\nsoundfont=missing.sf2\n[mt32]\nmodel=cm32ln_100\n";
        let imported = import(&[conf], &[PathBuf::from("/")], "x", None);
        assert_eq!(get(&imported, "sbtype").as_deref(), Some("sbpro2"));
        assert_eq!(get(&imported, "sbbase").as_deref(), Some("240"), "{:?}", imported.warnings);
        assert_eq!(get(&imported, "gusbase").as_deref(), Some("260"));
        assert_eq!(get(&imported, "opl").as_deref(), Some("opl3"));
        assert_eq!(get(&imported, "midisynth").as_deref(), Some("soundfont"));
        assert_eq!(get(&imported, "mt32model").as_deref(), Some("cm32l"));
        assert_eq!(get(&imported, "irq"), None, "an SB can't have IRQ 4");
        assert_eq!(get(&imported, "hdma"), None);
        assert_eq!(get(&imported, "soundfont"), None);
        assert!(imported.warnings.iter().any(|w| w.starts_with("irq=4")), "{:?}", imported.warnings);
        assert!(imported.warnings.iter().any(|w| w.contains("missing.sf2")), "{:?}", imported.warnings);
        let config = crate::config::parse(&imported.profile_text(None), Path::new("/"), None);
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);

        let dir = scratch("soundfont");
        fs::write(dir.join("GM.SF2"), "").unwrap();
        let imported = import(&["[fluidsynth]\nsoundfont=gm.sf2 70\n"], std::slice::from_ref(&dir), "x", None);
        assert_eq!(get(&imported, "soundfont"), Some(dir.join("GM.SF2").display().to_string()));
    }

    #[test]
    fn dosbox_s_commands_become_rust_dos_s() {
        let conf = "[autoexec]\nmount c .\nc:\nconfig -set cpu cycles=9000\nconfig -set \"sblaster irq=5\"\nconfig -set memsize=32\nconfig -get cpu\nmixer sb 50:50 /noshow\nloadfix -64 game.exe -nosound\nloadfix\n";
        let imported = import(&[conf], &[PathBuf::from("/")], "x", None);
        assert_eq!(get(&imported, "cycles").as_deref(), Some("9000"));
        assert_eq!(get(&imported, "irq").as_deref(), Some("5"), "{:?}", imported.warnings);
        assert_eq!(get(&imported, "memsize").as_deref(), Some("32"));
        assert_eq!(imported.autoexec, ["c:", "mixer sb 50:50 /noshow", "game.exe -nosound"]);
        assert_eq!(imported.warnings.len(), 1, "{:?}", imported.warnings);
        assert!(imported.warnings[0].contains("config -get"));
    }

    /// DOSBox Pure's configuration, in a game's zip or folder, which is
    /// its C: already.
    #[test]
    fn a_configuration_without_c_gets_the_game_s_folder() {
        let dir = scratch("pure");
        let conf = "[cpu]\ncycles=10000\n[autoexec]\ncd game\ngame.exe\n";
        let imported = import_in(&[conf], std::slice::from_ref(&dir), "x", None, Some(&dir));
        assert_eq!(imported.drives[0].drive, crate::disk::DRIVE_C);
        assert_eq!(imported.drives[0].path, dir);
        assert_eq!(imported.autoexec, ["C:", "cd game", "game.exe"], "on C:, so its CD stays");
        // One that mounts its own C: keeps it.
        let imported = import_in(&["[autoexec]\nmount c sub\nc:\ngame\n"], std::slice::from_ref(&dir), "x", None, Some(&dir));
        assert_eq!(imported.drives.len(), 1);
        assert_eq!(imported.drives[0].path, dir.join("sub"));
        assert_eq!(imported.autoexec, ["c:", "game"]);
    }
}
