//! The Ultrasound software built into rust-dos: the Gravis General MIDI
//! patch set (`MIDI\*.PAT`) and the files that map it (`ULTRASND.INI`,
//! `MIDI\ULTRAMID.INI`, ...), as the Gravis installer leaves them in
//! `C:\ULTRASND`. With the Ultrasound installed they are on a read-only
//! drive of their own, in `\ULTRASND`, and ULTRADIR points there, so
//! programs find the patches without a Gravis installation.
//!
//! The files are in `assets/ultrasnd`; `build.rs` lists them.

use crate::memfs::{Bytes, MemFs};

/// The files, by path in the Ultrasound directory ("MIDI\ACPIANO.PAT").
pub static FILES: &[(&str, &[u8])] = include!(concat!(env!("OUT_DIR"), "/ultrasnd.rs"));

/// The directory on the drive that holds them.
pub const DIR: &str = "ULTRASND";

/// Volume label of the drive.
pub const LABEL: &str = "ULTRASND";

/// The file at `path` in the Ultrasound directory.
pub fn file(path: &str) -> Option<&'static [u8]> {
    FILES.iter().find(|(p, _)| p.eq_ignore_ascii_case(path)).map(|&(_, data)| data)
}

/// The drive with the files for drive letter `letter`, where the patch
/// directories `ULTRASND.INI` names are `\ULTRASND\MIDI` on that drive.
pub fn drive(letter: char) -> MemFs {
    let dir = format!("{}:\\{}", letter, DIR);
    let mut fs = MemFs::new();
    for &(path, data) in FILES {
        let data = if path.eq_ignore_ascii_case("ULTRASND.INI") {
            Bytes::Owned(with_patch_dir(data, &format!("{}\\MIDI\\", dir)))
        } else {
            Bytes::Borrowed(data)
        };
        fs.insert(&format!("{}\\{}", DIR, path), data);
    }
    fs
}

/// `ini` with every `PatchDir` set to `patch_dir`.
fn with_patch_dir(ini: &[u8], patch_dir: &str) -> Vec<u8> {
    String::from_utf8_lossy(ini)
        .split_inclusive('\n')
        .map(|line| match line.split_once('=') {
            Some((key, value)) if key.trim().eq_ignore_ascii_case("PatchDir") => {
                let ending = &value[value.trim_end_matches(['\r', '\n']).len()..];
                format!("{}={}{}", key, patch_dir, ending)
            }
            _ => line.to_string(),
        })
        .collect::<String>()
        .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gus::patch::PatchBank;
    use crate::memfs::Node;

    #[test]
    fn the_ini_points_at_the_drive() {
        let fs = drive('X');
        let Some(Node::Bytes(ini)) = fs.file("ULTRASND\\ULTRASND.INI") else {
            panic!("no ULTRASND.INI");
        };
        let ini = String::from_utf8(ini.to_vec()).unwrap();
        assert_eq!(PatchBank::patch_dir(&ini).as_deref(), Some("X:\\ULTRASND\\MIDI\\"));
        assert!(!ini.to_ascii_uppercase().contains("C:\\ULTRASND"));
        assert!(ini.contains("PatchDir=X:\\ULTRASND\\MIDI\\\r\n0=acpiano\r\n"));
        assert!(fs.is_dir("ULTRASND\\MIDI"));
        assert_eq!(
            fs.file("ULTRASND\\MIDI\\ACPIANO.PAT").map(Node::len),
            file("MIDI\\ACPIANO.PAT").map(|d| d.len() as u64)
        );
    }

    #[test]
    fn every_patch_the_bank_names_is_there() {
        let mut bank = PatchBank::builtin();
        for program in 0..128 {
            assert!(bank.melodic(program).is_some(), "program {}", program);
        }
        // The General MIDI drum keys.
        for key in 35..=81 {
            assert!(bank.drum(key).is_some(), "drum {}", key);
        }
        assert!(bank.missing.is_empty(), "{:?}", bank.missing);
    }

    #[test]
    fn the_bank_on_the_drive_is_the_builtin_one() {
        let mut disk = crate::disk::DiskController::new(std::path::PathBuf::from("."));
        disk.mount_memory(b'X' - b'A', drive('X'), LABEL).unwrap();
        let (mut bank, dir) = PatchBank::from_dos_dir(&disk, "X:\\ULTRASND\\").unwrap();
        assert_eq!(dir, "X:\\ULTRASND\\MIDI");
        assert_eq!(bank.file_count(), PatchBank::builtin().file_count());
        assert_eq!(bank.melodic(0).unwrap().samples.len(), PatchBank::builtin().melodic(0).unwrap().samples.len());
    }
}
