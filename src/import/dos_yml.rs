//! DOS.YML, the description of how a game runs that .dosz packages
//! carry: `key: value` lines in a game's .dosz or .dosc. Read as a profile's settings, drives and commands. The
//! gamepad mappings and the action wheel have no counterpart here.

use super::Imported;
use crate::disk::{DriveKind, MountOptions};
use crate::mount::MountSpec;
use std::path::Path;

/// The speed each year's PC had, in instructions a millisecond, from the
/// format's table of years.
const YEAR_SPEEDS: [(u32, u32); 20] = [
    (1981, 315),
    (1982, 900),
    (1983, 1500),
    (1984, 2100),
    (1985, 2750),
    (1986, 3800),
    (1987, 4800),
    (1988, 6300),
    (1989, 7800),
    (1990, 14000),
    (1991, 23800),
    (1992, 27000),
    (1993, 44000),
    (1994, 55000),
    (1995, 66800),
    (1996, 93000),
    (1997, 125000),
    (1998, 200000),
    (1999, 350000),
    (2000, 500000),
];

fn year_speed(year: u32) -> Option<u32> {
    let year = year.clamp(1981, 2000);
    YEAR_SPEEDS.iter().find(|(y, _)| *y == year).map(|(_, speed)| *speed)
}

/// The game in `package` as its DOS.YML files describe it, `texts` in the
/// order they apply (the .dosz's, then the .dosc's over it).
pub fn import(texts: &[String], package: &Path, name: &str) -> Imported {
    let mut keys: Vec<(String, String)> = Vec::new();
    for text in texts {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once(':') else { continue };
            let (key, value) = (key.trim().to_ascii_lowercase(), value.trim().to_string());
            keys.retain(|(k, _)| *k != key);
            keys.push((key, value));
        }
    }
    let mut imported = Imported { name: name.to_string(), ..Default::default() };
    for (key, value) in &keys {
        apply(&mut imported, key, value, package);
    }
    imported.drop_invalid();
    imported
}

fn apply(imported: &mut Imported, key: &str, value: &str, package: &Path) {
    let lower = value.to_ascii_lowercase();
    let flag = match lower.as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    };
    let number = lower.parse::<u32>().ok();
    let skipped = |imported: &mut Imported| imported.warnings.push(format!("DOS.YML {}: {} isn't imported", key, value));
    match key {
        "cpu_type" => match lower.as_str() {
            "generic_386" => imported.set("emulator", "cpu", "386"),
            "generic_486" => imported.set("emulator", "cpu", "486"),
            "generic_pentium" => imported.set("emulator", "cpu", "pentium"),
            "auto" => {}
            _ => skipped(imported),
        },
        // A year's speed, or a speed the game mustn't go over: the
        // speed, held.
        "cpu_year" | "cpu_max_year" => match number.and_then(year_speed) {
            Some(speed) => imported.set("emulator", "cycles", speed.to_string()),
            None => skipped(imported),
        },
        "cpu_cycles" | "cpu_max_cycles" => match number {
            Some(n) => imported.set("emulator", "cycles", n.clamp(100, 2_000_000).to_string()),
            None => skipped(imported),
        },
        "mem_size" => match number {
            Some(kb) => imported.set("emulator", "memsize", kb.div_ceil(1024).max(1).to_string()),
            None => skipped(imported),
        },
        "mem_ems" | "mem_umb" => match flag {
            Some(on) => imported.set("emulator", if key == "mem_ems" { "ems" } else { "umb" }, on.to_string()),
            None => skipped(imported),
        },
        // Extended memory is always there.
        "mem_xms" if flag == Some(true) => {}
        "video_card" => {
            let machine = match lower.as_str() {
                "generic_svga" => "svga",
                "generic_vga" => "vga",
                "generic_ega" => "ega",
                "generic_cga" => "cga",
                "generic_hercules" => "hercules",
                "tandy" => "tandy",
                "pcjr" => "pcjr",
                "svga_s3_trio" => "svga_s3",
                "svga_tseng_et4000" => "svga_et4000",
                // The nearest rust-dos has.
                "svga_tseng_et3000" | "svga_paradise_pvga1a" => "svga",
                _ => return skipped(imported),
            };
            imported.set("emulator", "machine", machine);
        }
        "video_voodoo" => match lower.as_str() {
            "v1_8mb" | "v1_4mb" => {
                imported.set("emulator", "voodoo", "true");
                imported.set("emulator", "voodoo_memory", if lower == "v1_4mb" { "4" } else { "8" });
            }
            "none" => imported.set("emulator", "voodoo", "false"),
            _ => skipped(imported),
        },
        "video_cga_composite" => match flag {
            Some(on) => imported.set("emulator", "composite", if on { "on" } else { "off" }),
            None => skipped(imported),
        },
        "video_memory" => {}
        "sound_card" => match lower.as_str() {
            "sb16" => imported.set("sound", "sbtype", "sb16"),
            "sb1" | "sb2" => imported.set("sound", "sbtype", "sb2"),
            "sbpro1" | "sbpro2" => imported.set("sound", "sbtype", "sbpro2"),
            "none" => imported.set("sound", "sbtype", "none"),
            _ => skipped(imported),
        },
        "sound_port" => imported.set("sound", "sbbase", lower.trim_start_matches("0x").to_ascii_uppercase()),
        "sound_irq" => imported.set("sound", "irq", lower),
        "sound_dma" => imported.set("sound", "dma", lower),
        "sound_hdma" => imported.set("sound", "hdma", lower),
        "sound_gus" => match flag {
            Some(on) => imported.set("sound", "gus", on.to_string()),
            None => skipped(imported),
        },
        "sound_tandy" => match flag {
            Some(on) => imported.set("sound", "tandy", if on { "on" } else { "off" }),
            None => skipped(imported),
        },
        // A synthesizer, or the name of a package with its ROMs or
        // SoundFont, which rust-dos finds in its own places instead.
        "sound_mt32" | "sound_sc55" if flag != Some(false) => {
            imported.set("sound", "midisynth", if key == "sound_mt32" { "mt32" } else { "sc55" });
            if flag.is_none() {
                skipped(imported);
            }
        }
        "sound_mt32" | "sound_sc55" => {}
        "sound_midi" => match flag {
            Some(false) => imported.set("sound", "midisynth", "none"),
            Some(true) => {}
            None => skipped(imported),
        },
        "run_path" => {
            let path = value.replace('/', "\\");
            let (drive, rest) = match path.split_once(':') {
                Some((d, rest)) if d.len() == 1 => (format!("{}:", d.to_ascii_uppercase()), rest.to_string()),
                _ => ("C:".to_string(), path.clone()),
            };
            let rest = rest.trim_start_matches('\\');
            let (dir, program) = rest.rsplit_once('\\').unwrap_or(("", rest));
            imported.autoexec = vec![drive, format!("CD \\{}", dir), program.to_string()];
        }
        "run_boot" => {
            let image = super::host_path(package, value);
            imported.autoexec = vec![format!("BOOT \"{}\"", image.display())];
        }
        "run_mount" => {
            let path = super::host_path(package, value);
            let cd = ["iso", "cue", "chd", "ins", "inst"].iter().any(|e| lower.ends_with(&format!(".{}", e)));
            let (drive, kind) = if cd { (3, DriveKind::CdRom) } else { (0, DriveKind::Floppy) };
            imported.drives.retain(|d| d.drive != drive);
            imported.drives.push(MountSpec { drive, path, opts: MountOptions { kind, ..MountOptions::default() } });
        }
        // The root's configuration starts on its own; only other ones can
        // be utilities.
        "run_utility" => {}
        "run_input" => imported.input = Some(value.to_string()),
        "input_directmouse" if flag != Some(true) => {}
        "input_directmouse" => skipped(imported),
        // The gamepad's buttons and the action wheel, and the mouse's
        // speeds and wheel (padmap.rs).
        k if k.starts_with("input_") => {
            let key = k.trim_start_matches("input_").trim_start_matches("pad_").to_string();
            imported.pad.retain(|(k, _)| *k != key);
            imported.pad.push((key, value.to_string()));
        }
        _ => skipped(imported),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get<'a>(imported: &'a Imported, key: &str) -> Option<&'a str> {
        imported.settings.iter().find(|(_, k, _)| *k == key).map(|(_, _, v)| v.as_str())
    }

    #[test]
    fn a_dos_yml_is_settings_and_a_program() {
        let dosz = "cpu_max_year: 1995\r\nvideo_card: generic_vga\r\n".to_string();
        let dosc = "run_path: C:\\GAMES\\LIERO.EXE\r\nrun_input: (WAIT:200)(ENTER)\r\ncpu_year: 1990\r\nsound_card: sbpro2\r\nmem_size: 4096\r\ninput_pad_x: space Fire\r\n".to_string();
        let imported = import(&[dosz, dosc], Path::new("/games/liero.dosz"), "Liero");
        assert_eq!(get(&imported, "cycles"), Some("14000"), "the .dosc's year over the .dosz's");
        assert_eq!(get(&imported, "machine"), Some("vga"));
        assert_eq!(get(&imported, "sbtype"), Some("sbpro2"));
        assert_eq!(get(&imported, "memsize"), Some("4"));
        assert_eq!(imported.autoexec, ["C:", "CD \\GAMES", "LIERO.EXE"]);
        assert!(imported.warnings.is_empty(), "{:?}", imported.warnings);
        assert_eq!(imported.input.as_deref(), Some("(WAIT:200)(ENTER)"));
        assert_eq!(imported.pad, [("x".to_string(), "space Fire".to_string())]);
    }

    #[test]
    fn images_are_mounted_and_booted_from_the_package() {
        let imported = import(&["run_mount: CD\\GAME.CUE\nrun_boot: DISKS\\DISK1.IMG\n".to_string()], Path::new("/g/game.dosz"), "G");
        assert_eq!(imported.drives[0].drive, 3);
        assert_eq!(imported.drives[0].path, Path::new("/g/game.dosz/CD/GAME.CUE"));
        assert_eq!(imported.drives[0].opts.kind, DriveKind::CdRom);
        assert_eq!(imported.autoexec, ["BOOT \"/g/game.dosz/DISKS/DISK1.IMG\""]);
    }
}
