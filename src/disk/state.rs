//! The drives' and open files' part of a save state. The drives are saved
//! as they were mounted, with their current directories and the disk in
//! them; open files by their paths, access modes and positions, so a
//! loaded state opens them again. What the files and disk images hold is
//! the host's and isn't saved: a file changed since the state was saved
//! reads as it is now, and one gone leaves its handle closed.

use super::{CharDevice, DRIVE_C, DiskController, Drive, LASTDRIVE, OpenData, OpenFile, drive_letter};
use crate::mount::{mount_spec_value, parse_mount_spec, tokenize};
use crate::savestate::{Reader, Result, State, StateError, Writer};
use std::io::Seek;

crate::state_enum!(CharDevice { CharDevice::Nul, CharDevice::Con, CharDevice::Emm });

/// An open file as saved: its entry in the file table and how many
/// handles refer to it. The handles themselves are in the processes'
/// PSPs, in memory.
#[derive(Default)]
struct SavedFile {
    sft: u16,
    refs: u16,
    fcb: bool,
    drive: u8,
    owner: u16,
    key: u64,
    path: String,
    mode: u8,
    device: Option<CharDevice>,
    position: u64,
}

crate::state_fields!(SavedFile { sft, refs, fcb, drive, owner, key, path, mode, device, position });

impl DiskController {
    pub(crate) fn save_state(&self, w: &mut Writer) {
        // Whether EMMXXXX0 is there comes with the configuration.
        // The copy of the file table in memory is saved with the memory.
        let DiskController { open_files, sft_dirty: _, position_dirty: _, drives, current_drive, emm_device: _ } = self;
        current_drive.save(w);
        for drive in drives {
            drive.is_some().save(w);
            // The rest follows from how the drive was mounted.
            if let Some(Drive { kind: _, storage: _, current_dir, label: _, read_only: _, mount, images: _, image, media_changed }) =
                drive
            {
                mount.as_ref().map(|spec| mount_spec_value(spec, None)).save(w);
                current_dir.save(w);
                image.save(w);
                media_changed.save(w);
            }
        }
        let mut entries: Vec<u16> = open_files.keys().copied().collect();
        entries.sort_unstable();
        entries.len().save(w);
        for sft in entries {
            let OpenFile { data, drive, owner, key, path, mode, refs, fcb } = &open_files[&sft];
            let (device, position) = match data {
                OpenData::Host(file) => (None, (&*file).stream_position().unwrap_or(0)),
                OpenData::Memory(_, pos) | OpenData::Image(_, _, pos) | OpenData::Fat { pos, .. } => (None, pos.get()),
                OpenData::Device(device) => (Some(*device), 0),
            };
            let saved = SavedFile {
                sft,
                refs: *refs,
                fcb: *fcb,
                drive: *drive,
                owner: *owner,
                key: *key,
                path: path.clone(),
                mode: *mode,
                device,
                position,
            };
            saved.save(w);
        }
    }

    /// Mount the drives as the state has them, where they differ, and open
    /// its files again. Returns the files that couldn't be opened.
    pub(crate) fn load_state(&mut self, r: &mut Reader) -> Result<Vec<String>> {
        let mut current = 0u8;
        current.load(r)?;
        for drive in 0..LASTDRIVE {
            let letter = drive_letter(drive);
            let mut present = false;
            present.load(r)?;
            if !present {
                // Drives the machine has of itself (Z:, the Ultrasound's)
                // stay; mounted ones go.
                if drive != DRIVE_C && self.drive(drive).is_some_and(|d| d.mount.is_some()) {
                    self.unmount(drive).map_err(StateError::Mismatch)?;
                }
                continue;
            }
            let (mut mount, mut dir, mut image, mut changed) = (None::<String>, String::new(), 0usize, false);
            mount.load(r)?;
            dir.load(r)?;
            image.load(r)?;
            changed.load(r)?;
            let now = self.drive(drive).and_then(|d| d.mount.as_ref()).map(|spec| mount_spec_value(spec, None));
            if let Some(text) = mount.as_ref().filter(|&text| now.as_ref() != Some(text)) {
                // Relative paths are the emulator's working directory's, as
                // they were when the drive was mounted.
                let tokens = tokenize(text).map_err(StateError::Invalid)?;
                let cwd = std::env::current_dir().unwrap_or_else(|_| "/".into());
                let spec = parse_mount_spec(drive, &tokens, &cwd, None).map_err(StateError::Invalid)?;
                self.mount(drive, &spec.path, spec.opts, true)
                    .map_err(|e| StateError::Mismatch(format!("drive {}: can't be mounted: {}", letter, e)))?;
            }
            let images = match self.drive(drive) {
                Some(d) => d.images.len(),
                None => return Err(StateError::Mismatch(format!("it has a drive {}: and this machine hasn't", letter))),
            };
            for _ in 0..images {
                if self.drive(drive).is_some_and(|d| d.image == image) {
                    break;
                }
                self.swap_image(drive).map_err(StateError::Mismatch)?;
            }
            let d = self.drives[drive as usize].as_mut().expect("the drive is there");
            d.current_dir = dir;
            d.media_changed = changed;
        }
        self.current_drive = if self.is_mounted(current) { current } else { DRIVE_C };

        self.open_files.clear();
        let mut files = Vec::new();
        for _ in 0..r.count()? {
            let mut file = SavedFile::default();
            file.load(r)?;
            files.push(file);
        }

        // Each open again, in its entry.
        let mut lost = Vec::new();
        for file in files {
            let reopened = match file.device {
                Some(device) => {
                    Some(self.opened(OpenData::Device(device), file.drive, file.owner, 0, &file.path, file.mode))
                }
                None => match self.open_or_create(&file.path, file.mode, file.owner, false) {
                    Ok(at) => self.open_files.remove(&at),
                    Err(_) => None,
                },
            };
            let Some(mut open) = reopened else {
                lost.push(file.path.clone());
                continue;
            };
            open.drive = file.drive;
            open.owner = file.owner;
            open.key = file.key;
            open.path = file.path;
            open.mode = file.mode;
            open.refs = file.refs;
            open.fcb = file.fcb;
            self.open_files.insert(file.sft, open);
            let _ = self.seek_file(file.sft, file.position as i64, 0);
        }
        self.sft_dirty = u128::MAX;
        Ok(lost)
    }
}
