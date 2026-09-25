//! What a file or folder dropped onto the window becomes: a game imported,
//! a drive mounted, a disc or disk put in, or a program run.

use crate::disk::DriveKind;
use crate::diskimage::{self, ImageKind};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DropAction {
    /// A game set up for DOSBox (a GOG install's folder, a folder with
    /// DOSBox configuration files, or one of them): imported and launched.
    ImportGame(PathBuf),
    /// A folder, mounted as a hard disk on a free drive.
    MountFolder(PathBuf),
    /// A CD image, put in the CD-ROM drive (or mounted as one).
    Disc(PathBuf),
    /// A floppy image, put in A:.
    Floppy(PathBuf),
    /// A hard disk image, mounted on a free drive.
    HardDisk(PathBuf),
    /// A program or batch file: its folder mounted and it run.
    Run(PathBuf),
    /// A zip archive: unpacked into a game's folder (see zip.rs).
    Zip(PathBuf),
    /// Nothing rust-dos can use.
    Nothing(String),
}

/// What dropping `path` does.
pub fn drop_action(path: &Path) -> DropAction {
    let path = path.to_path_buf();
    if path.is_dir() {
        let has_confs = std::fs::read_dir(&path).into_iter().flatten().flatten().any(|e| {
            let name = e.file_name().to_string_lossy().to_ascii_lowercase();
            name.starts_with("dosbox") && name.ends_with(".conf")
        });
        return if has_confs || super::gog::find_info(&path).is_some_and(|info| info.parent() == Some(path.as_path())) {
            DropAction::ImportGame(path)
        } else {
            DropAction::MountFolder(path)
        };
    }
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "conf" => return DropAction::ImportGame(path),
        "exe" | "com" | "bat" => return DropAction::Run(path),
        "zip" => return DropAction::Zip(path),
        _ => {}
    }
    match diskimage::detect(&path, DriveKind::HardDisk) {
        Ok(ImageKind::Cd) => DropAction::Disc(path),
        Ok(ImageKind::Floppy) => DropAction::Floppy(path),
        Ok(ImageKind::HardDisk) => DropAction::HardDisk(path),
        Err(e) => DropAction::Nothing(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn dropped_things_do_what_they_are_for() {
        let dir = PathBuf::from("target/test_drop");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("gog")).unwrap();
        fs::create_dir_all(dir.join("plain")).unwrap();
        fs::write(dir.join("gog/dosboxGame.conf"), "").unwrap();
        fs::write(dir.join("game.cue"), "").unwrap();
        fs::write(dir.join("disk.img"), vec![0u8; 1_474_560]).unwrap();
        fs::write(dir.join("GAME.EXE"), "MZ").unwrap();
        fs::write(dir.join("readme.txt"), "hello").unwrap();
        assert_eq!(drop_action(&dir.join("gog")), DropAction::ImportGame(dir.join("gog")));
        assert_eq!(drop_action(&dir.join("plain")), DropAction::MountFolder(dir.join("plain")));
        assert_eq!(drop_action(&dir.join("gog/dosboxGame.conf")), DropAction::ImportGame(dir.join("gog/dosboxGame.conf")));
        assert_eq!(drop_action(&dir.join("game.cue")), DropAction::Disc(dir.join("game.cue")));
        assert_eq!(drop_action(&dir.join("disk.img")), DropAction::Floppy(dir.join("disk.img")));
        assert_eq!(drop_action(&dir.join("GAME.EXE")), DropAction::Run(dir.join("GAME.EXE")));
        assert_eq!(drop_action(&dir.join("x.zip")), DropAction::Zip(dir.join("x.zip")));
        assert!(matches!(drop_action(&dir.join("readme.txt")), DropAction::Nothing(_)));
    }
}
