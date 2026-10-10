//! The Ultrasound software on the emulated drives: the software built into
//! the `gus` crate on a read-only drive of its own, and the General MIDI
//! patch bank of the Ultrasound software in ULTRADIR on the mounted drives.

use std::borrow::Cow;

use crate::disk::{DiskController, FileData};
use crate::gus::builtin::{DIR, FILES};
use crate::gus::patch::{PatchBank, PatchFile};
use crate::memfs::{Bytes, MemFs};

/// A patch file on an emulated drive.
impl PatchFile for FileData {
    fn read(&self) -> std::io::Result<Cow<'static, [u8]>> {
        FileData::read(self)
    }
}

/// The drive with the built-in files for drive letter `letter`, where the
/// patch directories `ULTRASND.INI` names are `\ULTRASND\MIDI` on that
/// drive.
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

/// The bank of the Ultrasound software in DOS directory `ultradir`
/// (ULTRADIR) on the mounted drives: `ULTRASND.INI` there, and the
/// patches in the directory it names or else in its MIDI directory.
/// Also returns the DOS directory of the patches.
pub fn patch_bank(disk: &DiskController, ultradir: &str) -> Result<(PatchBank, String), String> {
    let dir = ultradir.trim_end_matches('\\');
    let ini_path = format!("{}\\ULTRASND.INI", dir);
    let ini = disk
        .file_data(&ini_path)
        .ok_or_else(|| format!("no {}", ini_path))?
        .read()
        .map_err(|e| format!("{}: {}", ini_path, e))?;
    let ini = String::from_utf8_lossy(&ini);
    let patches = PatchBank::patch_dir(&ini)
        .map(|d| d.trim_end_matches('\\').to_string())
        .filter(|d| disk.is_directory(d))
        .or_else(|| Some(format!("{}\\MIDI", dir)).filter(|d| disk.is_directory(d)))
        .ok_or_else(|| format!("no patch directory for {}", ini_path))?;
    // The patches as programs see them, by their DOS names.
    let files = disk
        .list_directory(&format!("{}\\*.PAT", patches), 0)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|entry| {
            let data = disk.file_data(&format!("{}\\{}", patches, entry.filename))?;
            let stem = entry.filename.split('.').next()?.to_ascii_lowercase();
            Some((stem, Box::new(data) as Box<dyn PatchFile>))
        })
        .collect();
    let bank = PatchBank::with_files(&ini, files);
    if bank.file_count() == 0 {
        return Err(format!("no patches in {}", patches));
    }
    Ok((bank, patches))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gus::builtin::{LABEL, file};
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
    fn the_bank_on_the_drive_is_the_builtin_one() {
        let mut disk = DiskController::new(std::path::PathBuf::from("."));
        disk.mount_memory(b'X' - b'A', drive('X'), LABEL).unwrap();
        let (mut bank, dir) = patch_bank(&disk, "X:\\ULTRASND\\").unwrap();
        assert_eq!(dir, "X:\\ULTRASND\\MIDI");
        assert_eq!(bank.file_count(), PatchBank::builtin().file_count());
        assert_eq!(bank.melodic(0).unwrap().samples.len(), PatchBank::builtin().melodic(0).unwrap().samples.len());
    }
}
