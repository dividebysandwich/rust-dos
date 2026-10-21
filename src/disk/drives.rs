//! What is mounted where: drive queries, boot CDs, shared disks and the current drive.

use super::*;

impl DiskController {
    pub(super) fn drive(&self, drive: u8) -> Option<&Drive> {
        self.drives.get(drive as usize).and_then(|d| d.as_ref())
    }

    /// The drive that boots when Rust-DOS starts (`MountOptions::boot`),
    /// if one does.
    pub fn boot_drive(&self) -> Option<u8> {
        (0..DRIVE_SLOTS).find(|&d| self.drive(d).and_then(|d| d.mount.as_ref()).is_some_and(|spec| spec.opts.boot))
    }

    /// Make `drive` the one that boots when Rust-DOS starts, or none.
    pub fn set_boot_drive(&mut self, drive: Option<u8>) {
        for (d, slot) in self.drives.iter_mut().enumerate() {
            if let Some(spec) = slot.as_mut().and_then(|d| d.mount.as_mut()) {
                spec.opts.boot = drive == Some(d as u8);
            }
        }
    }

    /// Have `drive` boot when Rust-DOS starts, in place of any other, or
    /// not.
    pub fn set_boots(&mut self, drive: u8, boots: bool) {
        if boots {
            self.set_boot_drive(Some(drive));
        } else if let Some(spec) = self.drives.get_mut(drive as usize).and_then(|d| d.as_mut()?.mount.as_mut()) {
            spec.opts.boot = false;
        }
    }

    pub fn is_mounted(&self, drive: u8) -> bool {
        self.drive(drive).is_some()
    }

    pub fn drive_kind(&self, drive: u8) -> Option<DriveKind> {
        self.drive(drive).map(|d| d.kind)
    }

    pub fn is_writable(&self, drive: u8) -> bool {
        self.drive(drive).is_some_and(|d| d.writable())
    }

    pub fn volume_label(&self, drive: u8) -> Option<String> {
        self.drive(drive).map(|d| d.label.clone())
    }

    /// The CD image a drive shows.
    pub fn cd_image(&self, drive: u8) -> Option<Rc<CdImage>> {
        self.drive(drive)?.image().cloned()
    }

    /// The disc a booted system's CD-ROM drive reads for `drive`: its CD
    /// image, or the disc made from the host folder mounted as a CD.
    pub fn boot_cd_image(&self, drive: u8) -> Option<Rc<CdImage>> {
        let d = self.drive(drive)?;
        d.image().or(d.boot_cd.as_ref()).cloned()
    }

    /// Whether `drive` is a host folder mounted as a CD, which a booted
    /// system reads as a disc made from it.
    pub fn is_folder_cd(&self, drive: u8) -> bool {
        self.drive(drive).is_some_and(|d| d.kind == DriveKind::CdRom && d.host_root().is_some())
    }

    /// Make the discs of the host folders mounted as CDs that have none,
    /// for a booted system. Returns what the log should say.
    pub fn prepare_boot_cds(&mut self) -> Vec<String> {
        (0..LASTDRIVE)
            .filter(|&d| self.is_folder_cd(d) && self.drive(d).is_some_and(|d| d.boot_cd.is_none()))
            .collect::<Vec<_>>()
            .into_iter()
            .flat_map(|d| match self.make_boot_cd(d) {
                Ok(lines) => lines,
                Err(e) => vec![e],
            })
            .collect()
    }

    /// Make the disc of the host folder mounted as CD drive `drive` again,
    /// with what the folder holds now, as a new disc in the drive. Returns
    /// what the log should say.
    pub fn make_boot_cd(&mut self, drive: u8) -> Result<Vec<String>, String> {
        let letter = drive_letter(drive);
        let Some(d) = self.drives.get_mut(drive as usize).and_then(Option::as_mut) else {
            return Err(format!("Drive {}: is not mounted", letter));
        };
        let Some(root) = d.host_root().filter(|_| d.kind == DriveKind::CdRom).map(Path::to_path_buf) else {
            return Err(format!("Drive {}: is not a folder mounted as a CD", letter));
        };
        // The label the mount asks for, or the folder's name.
        let label = d.mount.as_ref().and_then(|m| m.opts.label.clone()).unwrap_or_else(|| {
            root.file_name().map_or_else(|| d.label.clone(), |n| n.to_string_lossy().into_owned())
        });
        let disc = crate::cdrom::folder::build(&root, &label)
            .map_err(|e| format!("Drive {}: can't be made a CD: {}", letter, e))?;
        let mut lines = vec![format!(
            "Drive {}: is a CD made from {} ({} MB)",
            letter,
            root.display(),
            (disc.sectors as u64 * crate::cdrom::DATA_SECTOR as u64).div_ceil(1 << 20)
        )];
        lines.extend(disc.skipped.iter().map(|s| format!("Drive {}: left off the CD: {}", letter, s)));
        d.boot_cd = Some(Rc::new(CdImage::from_folder(disc, &root)?));
        d.media_changed = true;
        Ok(lines)
    }

    /// Whether `drive` is a host directory a booted system gets as a hard
    /// disk: D: to Y: unless its mount says `-noshare`, C: with `-share`.
    pub fn is_shareable(&self, drive: u8) -> bool {
        let Some(d) = self.drive(drive).filter(|d| d.kind == DriveKind::HardDisk && d.host_root().is_some()) else {
            return false;
        };
        let share = d.mount.as_ref().and_then(|m| m.opts.share);
        share.unwrap_or((DRIVE_C + 1..DRIVE_Z).contains(&drive))
    }

    /// Whether `drive` has a disk made of its host directory for a booted
    /// system.
    pub fn is_shared(&self, drive: u8) -> bool {
        self.drive(drive).is_some_and(|d| d.shared.is_some())
    }

    /// The drives with disks made of their host directories, in
    /// drive-letter order.
    pub fn shared_drives(&self) -> Vec<u8> {
        (0..LASTDRIVE).filter(|&d| self.is_shared(d)).collect()
    }

    /// Make the disks of the host directories shared with a booted system
    /// that have none yet (a restart keeps what the system wrote). Returns
    /// what the log should say.
    pub fn prepare_shared_disks(&mut self) -> Vec<String> {
        let mut lines = Vec::new();
        for drive in 0..LASTDRIVE {
            if !self.is_shareable(drive) || self.is_shared(drive) {
                continue;
            }
            let d = self.drives[drive as usize].as_mut().expect("a shareable drive");
            let root = d.host_root().expect("a host directory").to_path_buf();
            let shown = d.shown_root().expect("a host directory").to_path_buf();
            match crate::shared_disk::SharedDisk::build_named(&root, &shown) {
                Ok((shared, skipped)) => {
                    lines.push(format!("Drive {}: is a hard disk made from {}", drive_letter(drive), shown.display()));
                    lines.extend(skipped.into_iter().map(|s| format!("Drive {}: left off the disk: {}", drive_letter(drive), s)));
                    d.shared = Some(shared);
                }
                Err(e) => lines.push(format!("Drive {}: can't be shared: {}", drive_letter(drive), e)),
            }
        }
        lines
    }

    /// Copy what a booted system changed on the shared disk of `drive`, or
    /// of every drive, into the host directories. `last` is the copy at
    /// its shutdown, which deletes files too. Returns a line for each.
    pub fn sync_shared(&mut self, drive: Option<u8>, last: bool) -> Vec<String> {
        let mut lines = Vec::new();
        for d in 0..LASTDRIVE {
            if drive.is_some_and(|only| only != d) {
                continue;
            }
            let Some(drive) = self.drives[d as usize].as_mut().filter(|d| d.shared.is_some()) else { continue };
            if drive.read_only {
                lines.push(format!("Drive {}: is read-only: what the system wrote stays on its disk", drive_letter(d)));
                continue;
            }
            // Under an overlay, the changes go to its folder.
            let target = drive.overlay.as_ref().and_then(|o| o.upper.clone());
            let shared = drive.shared.as_mut().expect("a shared disk");
            let report = shared.sync(last);
            let target = target.unwrap_or_else(|| shared.root.clone());
            lines.push(format!("Drive {}: {} copied to {}: {}", drive_letter(d), if last { "was" } else { "is" }, target.display(), report.summary()));
            lines.extend(report.conflicts.iter().map(|c| format!("Drive {}: {} changed on both sides", drive_letter(d), c)));
            lines.extend(report.errors.iter().map(|e| format!("Drive {}: {}", drive_letter(d), e)));
        }
        self.host_dirs.forget();
        lines
    }

    /// The drives with disks made of shared host directories and their
    /// manifests, as a state keeps them.
    pub(crate) fn shared_manifests(&self) -> Vec<(u8, String)> {
        self.shared_drives()
            .into_iter()
            .filter_map(|d| Some((d, self.drive(d)?.shared.as_ref()?.manifest.to_text())))
            .collect()
    }

    /// The disks of shared host directories a state being loaded has: an
    /// empty one for each drive that has none, which the state's
    /// checkpoint fills from the copy beside the state file, and the
    /// state's manifest.
    pub(crate) fn restore_shared(&mut self) -> Result<(), String> {
        let Some(list) = self.shared_from_state.take() else { return Ok(()) };
        for (drive, text) in list {
            let manifest = crate::shared_disk::Manifest::from_text(&text).ok_or("a broken manifest")?;
            let d = self.drives.get_mut(drive as usize).and_then(Option::as_mut);
            let Some(d) = d.filter(|d| d.kind == DriveKind::HardDisk && d.host_root().is_some()) else {
                return Err(format!("drive {} isn't a host directory, which the system had as a disk", drive_name(drive)));
            };
            if d.shared.is_none() {
                let root = d.host_root().expect("a host directory").to_path_buf();
                d.shared = Some(crate::shared_disk::SharedDisk::empty(&root)?);
            }
            d.shared.as_mut().expect("a shared disk").manifest = manifest;
        }
        Ok(())
    }

    /// At a booted system's shutdown (`last`): copy what it changed on the
    /// shared disks into the host directories, and let go of the disks.
    /// Otherwise, as it goes away without one (a state of the built-in DOS
    /// loaded over it), without deleting. Returns what the log should say.
    pub fn finish_shared_disks(&mut self, last: bool) -> Vec<String> {
        let lines = self.sync_shared(None, last);
        for d in self.drives.iter_mut().flatten() {
            d.shared = None;
        }
        lines
    }

    /// Let go of the discs made from host folders: the built-in DOS reads
    /// the folders themselves.
    pub fn drop_boot_cds(&mut self) {
        for d in self.drives.iter_mut().flatten() {
            d.boot_cd = None;
        }
    }

    /// The file system of a drive mounted from a disk image.
    pub fn fat_volume(&self, drive: u8) -> Option<Rc<FatVolume>> {
        self.drive(drive)?.fat().cloned()
    }

    /// The disk image of a drive mounted from one, as the BIOS reads it.
    pub fn bios_image(&self, drive: u8) -> Option<Rc<DiskImage>> {
        self.drive(drive)?.disk().cloned()
    }

    /// The host folder the drive's files are read from: the folder itself,
    /// or under an overlay the overlay's root (`layerN:/`), which shows
    /// the files below with the changes made to them. None for a drive
    /// that isn't a host folder.
    pub fn host_root(&self, drive: u8) -> Option<PathBuf> {
        self.drive(drive)?.host_root().map(Path::to_path_buf)
    }

    /// The IDE slot `drive` was mounted for (`-ide`), if any.
    pub fn ide_slot(&self, drive: u8) -> Option<crate::ide::IdeSlot> {
        self.drive(drive)?.mount.as_ref()?.opts.ide
    }

    /// Keep journals of the writes to the disk images in the drives, or
    /// stop: a booted system's states and rewind take its disks back with
    /// its memory (`DiskImage::revert_to`).
    pub fn keep_journals(&self, on: bool) {
        for drive in 0..DRIVE_SLOTS {
            if let Some(disk) = self.bios_image(drive) {
                disk.keep_journal(on);
            }
        }
    }

    /// Take the disks back to the checkpoints of the state just loaded.
    pub(crate) fn revert_disks(&mut self) -> Vec<String> {
        let mut failed = Vec::new();
        for (drive, id) in std::mem::take(&mut self.reverts) {
            let reverted = self.bios_image(drive).ok_or_else(|| "it's gone".to_string()).and_then(|disk| disk.revert_to(id));
            if let Err(e) = reverted {
                failed.push(format!("drive {}: {}", drive_name(drive), e));
            }
        }
        failed
    }

    /// Whether another disk went in `drive` since the last time this was
    /// asked (INT 13h AH=16h's disk change line).
    pub fn take_media_changed(&mut self, drive: u8) -> bool {
        self.drives
            .get_mut(drive as usize)
            .and_then(Option::as_mut)
            .is_some_and(|d| std::mem::take(&mut d.media_changed))
    }

    pub fn drive_info(&self, drive: u8) -> Option<DriveInfo> {
        self.drive(drive).map(|d| DriveInfo {
            drive,
            kind: d.kind,
            root: d.shown_root().map(Path::to_path_buf),
            overlay: d
                .disk()
                .and_then(|disk| disk.delta_path().map(Path::to_path_buf))
                .or_else(|| d.overlay.as_ref().and_then(|o| o.upper.clone())),
            image: d
                .image()
                .map(|image| image.path().to_path_buf())
                .or_else(|| d.mounted_disk().map(|disk| disk.path().to_path_buf()))
                .map(|path| d.shown(path)),
            images: d.images.iter().map(|path| d.shown(path.clone())).collect(),
            image_index: d.image,
            label: d.label.clone(),
            read_only: !d.writable(),
            current_dir: d.current_dir.to_ascii_uppercase(),
            mount: d.mount.clone(),
        })
    }

    /// The lettered drives: the ones DOS has.
    pub fn mounted_drives(&self) -> Vec<DriveInfo> {
        (0..LASTDRIVE).filter_map(|d| self.drive_info(d)).collect()
    }

    /// The disks mounted by number, which only the BIOS has.
    pub fn numbered_drives(&self) -> Vec<DriveInfo> {
        (LASTDRIVE..DRIVE_SLOTS).filter_map(|d| self.drive_info(d)).collect()
    }

    /// The lettered drives, then the disks mounted by number.
    pub fn all_drives(&self) -> Vec<DriveInfo> {
        (0..DRIVE_SLOTS).filter_map(|d| self.drive_info(d)).collect()
    }

    /// Floppy drives the BIOS reports: one for A:, two for B: even with A:
    /// empty, as a machine with two drives and one disk.
    pub fn floppy_units(&self) -> u8 {
        (0..FLOPPY_DRIVES)
            .rev()
            .find(|&unit| self.drive_kind(self.floppy_unit(unit)) == Some(DriveKind::Floppy))
            .map_or(0, |unit| unit + 1)
    }

    /// The drive behind the BIOS's floppy unit `unit` (0 or 1): the disk
    /// mounted as that number, or else A: or B:.
    pub fn floppy_unit(&self, unit: u8) -> u8 {
        let numbered = numbered_drive(unit);
        if self.is_mounted(numbered) { numbered } else { unit }
    }

    /// The drives behind the BIOS's hard disk units, 80h up: the disks
    /// mounted as 2 and 3 are 80h and 81h, and the hard disk drives that
    /// `lettered` takes fill the units they leave, in drive-letter order.
    pub fn hard_disk_units(&self, lettered: impl Fn(u8) -> bool) -> Vec<u8> {
        let mut drives = self.drives_of_kind(DriveKind::HardDisk).into_iter().filter(|&d| lettered(d));
        let mut units: Vec<u8> = (FLOPPY_DRIVES..NUMBERED_DRIVES)
            .filter_map(|number| {
                let numbered = numbered_drive(number);
                if self.is_mounted(numbered) { Some(numbered) } else { drives.next() }
            })
            .collect();
        units.extend(drives);
        units
    }

    /// Mounted lettered drives of the given kind, in drive-letter order.
    pub fn drives_of_kind(&self, kind: DriveKind) -> Vec<u8> {
        (0..LASTDRIVE)
            .filter(|&d| self.drive_kind(d) == Some(kind))
            .collect()
    }

    /// Drive a file handle was opened on (Z: for virtual handles).
    pub fn handle_drive(&self, handle: u16) -> Option<u8> {
        self.open_files.get(&handle).map(|f| f.drive)
    }

    /// What tells an open file apart from others, for telling sequential
    /// disk access from random (the disk noises).
    pub fn handle_key(&self, handle: u16) -> Option<u64> {
        self.open_files.get(&handle).map(|f| f.key)
    }

    /// Which drive a DOS path is on.
    pub fn drive_of(&self, dos_path: &str) -> Option<u8> {
        self.split_drive(&dos_path.replace('/', "\\")).map(|(drive, _)| drive)
    }

    /// A number that tells the file a DOS path names apart from others.
    pub fn file_key(&self, dos_path: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.qualify_path(dos_path).unwrap_or_else(|| dos_path.to_ascii_uppercase()).hash(&mut hasher);
        hasher.finish()
    }

    /// The character device an open handle refers to, if any.
    pub fn handle_device(&self, handle: u16) -> Option<CharDevice> {
        match self.open_files.get(&handle)?.data {
            OpenData::Device(device) => Some(device),
            _ => None,
        }
    }

    /// What DOS keeps in the file table entry `sft` about the file open
    /// there, but its position (`position`).
    pub fn sft_entry(&self, sft: u16) -> Option<SftEntry> {
        let open = self.open_files.get(&sft)?;
        let size = match &open.data {
            _ if open.empty_on_use => 0,
            OpenData::Host(file, _) => file.len().unwrap_or(0),
            OpenData::Memory(data, _) => data.len() as u64,
            OpenData::Image(_, extent, _) => extent.size as u64,
            OpenData::Fat { volume, at, .. } => volume.reload(*at).map_or(0, |entry| entry.size as u64),
            OpenData::Device(_) => 0,
        };
        let (time, date) = self.file_time(sft).unwrap_or((0, 0));
        let leaf = open.path.rsplit(['\\', '/', ':']).next().unwrap_or("");
        let (stem, ext) = match open.data {
            OpenData::Device(_) => (leaf.split('.').next().unwrap_or(""), ""),
            _ => leaf.rsplit_once('.').unwrap_or((leaf, "")),
        };
        let mut name = [b' '; 11];
        for (slot, b) in name[..8].iter_mut().zip(stem.trim().bytes()) {
            *slot = b.to_ascii_uppercase();
        }
        for (slot, b) in name[8..].iter_mut().zip(ext.bytes()) {
            *slot = b.to_ascii_uppercase();
        }
        Some(SftEntry {
            refs: open.refs,
            mode: open.mode as u16 | if open.fcb { 0x8000 } else { 0 },
            device: self.handle_device(sft),
            drive: open.drive,
            owner: open.owner,
            name,
            size: size.min(u32::MAX as u64) as u32,
            time,
            date,
            raw: open.raw,
        })
    }

    /// Whether the device open at `sft` is in raw (binary) mode.
    pub fn device_raw(&self, sft: u16) -> bool {
        self.open_files.get(&sft).is_some_and(|f| f.raw)
    }

    /// Put the device open at `sft` in raw (binary) or cooked mode, as
    /// IOCTL 4401h does. A file has no such mode.
    pub fn set_device_raw(&mut self, sft: u16, raw: bool) {
        if let Some(open) = self.open_files.get_mut(&sft)
            && matches!(open.data, OpenData::Device(_))
        {
            open.raw = raw;
            self.mark_dirty(sft);
        }
    }

    /// The fully qualified DOS path the open file `sft` was opened by.
    pub fn open_path(&self, sft: u16) -> Option<String> {
        self.open_files.get(&sft).map(|f| f.path.clone())
    }

    /// Where in the open file `sft` the next read or write goes.
    pub fn position(&self, sft: u16) -> Option<u64> {
        match &self.open_files.get(&sft)?.data {
            OpenData::Host(file, _) => (&*file).stream_position().ok(),
            OpenData::Memory(_, pos) | OpenData::Image(_, _, pos) | OpenData::Fat { pos, .. } => Some(pos.get()),
            OpenData::Device(_) => Some(0),
        }
    }

    pub fn set_current_drive(&mut self, drive: u8) -> u8 {
        // Only switch to drives that exist; always report LASTDRIVE.
        if self.is_mounted(drive) {
            self.current_drive = drive;
        }
        LASTDRIVE
    }

    pub fn get_current_drive(&self) -> u8 {
        self.current_drive
    }

    /// Return to the fresh-session working directories after a running
    /// program is closed: every mounted drive at its root, and C: current.
    pub fn reset_current_directories(&mut self) {
        for drive in self.drives.iter_mut().flatten() {
            drive.current_dir.clear();
        }
        self.current_drive = if self.is_mounted(DRIVE_C) {
            DRIVE_C
        } else {
            self.drives.iter().position(Option::is_some).unwrap_or(DRIVE_C as usize) as u8
        };
    }
}
