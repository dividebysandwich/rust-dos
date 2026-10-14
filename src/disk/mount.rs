//! Mounting and unmounting drives, and swapping the images in them.

use super::*;

impl DiskController {
    // ========================================================================
    // MOUNTS
    // ========================================================================

    /// Host directory currently backing drive C:.
    pub fn root_path(&self) -> &Path {
        self.drives[DRIVE_C as usize]
            .as_ref()
            .and_then(Drive::host_root)
            .unwrap_or(Path::new(""))
    }

    /// Mount a host directory, or a disk or CD image, as `drive`. A list of
    /// images (the path and `opts.more_images`) puts the first in, and
    /// Ctrl+F4 the next (`swap_image`). Unless `replace` is set, the drive
    /// must not already be mounted. Replacing closes the files open on it.
    /// On A: and B: the drive is a floppy whatever `opts.kind` says. A
    /// numbered drive (`numbered_drive`) takes floppy or hard disk images
    /// whatever is on them.
    pub fn mount(
        &mut self,
        drive: u8,
        path: &Path,
        opts: MountOptions,
        replace: bool,
    ) -> Result<PathBuf, String> {
        if drive >= DRIVE_SLOTS {
            return Err("Invalid drive letter".to_string());
        }
        let name = drive_name(drive);
        if drive == DRIVE_Z || opts.kind == DriveKind::Virtual {
            return Err(format!("Drive {} is reserved", name));
        }
        if self.is_mounted(drive) && !replace {
            return Err(format!("Drive {} is already mounted", name));
        }
        // A: and B: are floppies whatever the type asked for, but a CD
        // can't be one.
        let floppy_drive = drive < FLOPPY_DRIVES;
        if floppy_drive && opts.kind == DriveKind::CdRom {
            return Err(format!("Drive {} is a floppy drive and can't be a CD-ROM", name));
        }
        let spec = MountSpec { drive, path: path.to_path_buf(), opts: opts.clone() };
        // An archive: its files, or the disk or CD image in it (or the one
        // the path names in it), through an overlay; the changes go to
        // `-overlay`'s folder.
        let mut overlaid = None;
        let archive_path;
        let mut path = path;
        // A path into an archive (`game.zip/CD/GAME.CUE`): what is there.
        let inside = if hostfs::exists(path) { None } else { crate::archive::split(path) };
        let whole = hostfs::is_file(path) && crate::archive::is_archive_name(path);
        let mut more_images = opts.more_images.clone();
        if (whole && more_images.is_empty()) || inside.is_some() {
            let whole_archive = inside.is_none();
            let (archive, inner) = inside.unwrap_or_else(|| (path.to_path_buf(), String::new()));
            let canonical = hostfs::canonicalize(&archive).map_err(|e| e.to_string())?;
            let variant = opts.variant.as_deref().filter(|_| whole_archive);
            let stack = crate::archive::open_variant(&canonical, variant)?;
            self.notes.extend(stack.notes.take());
            let image = if inner.is_empty() { archive_image(&stack.files()) } else { Some(inner) };
            let upper = opts.overlay.clone().filter(|_| !opts.read_only && (image.is_some() || opts.kind != DriveKind::CdRom));
            // A list of images in it: all of them in it.
            for image in &mut more_images {
                match crate::archive::split(image) {
                    Some((other, inner)) if hostfs::canonicalize(&other).ok().as_ref() == Some(&canonical) => {
                        *image = PathBuf::from(inner);
                    }
                    _ => return Err(format!("{} isn't in {} with the first image", image.display(), canonical.display())),
                }
            }
            let o = Self::overlaid(Box::new(stack), canonical, upper)?;
            for image in &mut more_images {
                *image = o.layer.root().join(&*image);
            }
            archive_path = match image {
                Some(image) => o.layer.root().join(image),
                None => o.layer.root().to_path_buf(),
            };
            path = &archive_path;
            overlaid = Some(o);
        }
        if hostfs::is_file(path) {
            let mut images = Vec::new();
            for image in std::iter::once(path).chain(more_images.iter().map(PathBuf::as_path)) {
                if !hostfs::is_file(image) {
                    return Err(format!("{} is not a disk or CD image", image.display()));
                }
                let canonical = hostfs::canonicalize(image).map_err(|e| e.to_string())?;
                // Two drives on one image would each think they know
                // what's on it.
                let elsewhere = (0..DRIVE_SLOTS)
                    .find(|&d| d != drive && self.drive(d).is_some_and(|other| other.images.contains(&canonical)));
                if let Some(other) = elsewhere {
                    return Err(format!("{} is already mounted as {}", image.display(), drive_name(other)));
                }
                images.push(canonical);
            }
            let (kind, storage, volume_label, writable) = Self::open_image(drive, &images[0], &opts)?;
            self.close_drive_files(drive);
            let shown = overlaid.as_ref().and_then(|o| o.shown(&images[0])).unwrap_or_else(|| images[0].clone());
            self.drives[drive as usize] = Some(Drive {
                kind,
                storage,
                current_dir: String::new(),
                label: Self::label_for(&opts, &volume_label),
                read_only: opts.read_only || !writable,
                mount: Some(spec),
                images,
                image: 0,
                media_changed: true,
                boot_cd: None,
                shared: None,
                overlay: overlaid,
            });
            return Ok(shown);
        }
        if !opts.more_images.is_empty() {
            return Err("Only disk and CD images can be mounted as a list".to_string());
        }
        if drive_number(drive).is_some() {
            return Err(format!("{} is not a disk image", path.display()));
        }
        if !hostfs::is_dir(path) {
            return Err(format!("{} is not a directory or a disk or CD image", path.display()));
        }
        let kind = if floppy_drive { DriveKind::Floppy } else { opts.kind };
        // In an archive, the folder of it the path names.
        let in_archive = overlaid.as_ref().map(|_| path.to_path_buf());
        let canonical = match &overlaid {
            Some(o) => o.lower.clone(),
            None => hostfs::canonicalize(path).map_err(|e| e.to_string())?,
        };
        if overlaid.is_none()
            && let Some(upper) = opts.overlay.as_ref().filter(|_| kind != DriveKind::CdRom && !opts.read_only)
        {
            let lower = Box::new(crate::overlay::Folder(canonical.clone()));
            overlaid = Some(Self::overlaid(lower, canonical.clone(), Some(upper.clone()))?);
        }
        let root = in_archive.unwrap_or_else(|| overlaid.as_ref().map_or_else(|| canonical.clone(), |o| o.layer.root().to_path_buf()));
        // An archive without the folder for its changes can't be written.
        let unwritable = overlaid.as_ref().is_some_and(|o| o.upper.is_none());

        self.close_drive_files(drive);
        self.drives[drive as usize] = Some(Drive {
            kind,
            storage: Storage::Host(root),
            current_dir: String::new(),
            label: Self::label_for(&opts, DEFAULT_LABEL),
            read_only: opts.read_only || kind == DriveKind::CdRom || unwritable,
            mount: Some(spec),
            images: Vec::new(),
            image: 0,
            media_changed: floppy_drive,
            boot_cd: None,
            shared: None,
            overlay: overlaid,
        });
        Ok(canonical)
    }

    /// The overlay of `lower` (`display` to show, the directory or archive)
    /// with the changes in `upper`, as a layer. Layers are the thread's
    /// own (`hostfs`), so theirs needn't be Send.
    #[allow(clippy::arc_with_non_send_sync)]
    fn overlaid(lower: Box<dyn crate::overlay::Lower>, display: PathBuf, upper: Option<PathBuf>) -> Result<Overlaid, String> {
        let overlay = crate::overlay::Overlay::new(lower, upper.clone())
            .map_err(|e| format!("{}: {}", upper.as_deref().unwrap_or(Path::new("")).display(), e))?;
        Ok(Overlaid { layer: hostfs::add_layer(std::sync::Arc::new(overlay)), lower: display, upper })
    }

    /// The label of a drive: the one the mount asks for, or else `default`.
    fn label_for(opts: &MountOptions, default: &str) -> String {
        opts.label
            .as_deref()
            .map(normalize_label)
            .filter(|l| !l.is_empty())
            .unwrap_or_else(|| normalize_label(default))
    }

    /// Open the disk or CD image at `path` for `drive`: the drive's type,
    /// its storage, the volume label and whether the image can be written.
    fn open_image(drive: u8, path: &Path, opts: &MountOptions) -> Result<(DriveKind, Storage, String, bool), String> {
        if let Some(number) = drive_number(drive) {
            return Self::raw_storage(number, path, opts);
        }
        let found = diskimage::detect(path, opts.kind)?;
        if found == ImageKind::Cd {
            Self::check_cd_drive(drive)?;
            return Self::cd_storage(CdImage::open(path)?);
        }
        // A: and B: take a hard disk image without a partition table as a
        // floppy of its size.
        let floppy = found == ImageKind::Floppy || drive < FLOPPY_DRIVES;
        let name = path.display().to_string();
        Self::fat_storage(drive, found, &name, || Self::open_disk(path, floppy, opts))
    }

    /// The floppy or hard disk image at `path`, its changes in a delta
    /// file in the mount's overlay folder if it has one (`diskdelta`).
    fn open_disk(path: &Path, floppy: bool, opts: &MountOptions) -> Result<DiskImage, String> {
        match Self::delta_for(path, opts) {
            Some(delta) => DiskImage::open_delta(path, &delta, floppy, opts.geometry, opts.read_only),
            None => DiskImage::open(path, floppy, opts.geometry, opts.read_only),
        }
    }

    /// Where the changes of the disk image at `path` go, under the mount's
    /// overlay folder: `<image>.rdelta`, beside where the image would be
    /// copied up to. An image in an archive that was copied up whole
    /// before deltas is written to as it is.
    fn delta_for(path: &Path, opts: &MountOptions) -> Option<PathBuf> {
        let upper = opts.overlay.as_ref()?;
        let name = match hostfs::is_layer(path) {
            true => hostfs::layer_path(path),
            false => path.file_name()?.to_string_lossy().into_owned(),
        };
        if hostfs::is_layer(path) && hostfs::is_file(upper.join(&name)) {
            return None;
        }
        Some(upper.join(format!("{}.rdelta", name)))
    }

    /// The type, storage, volume label and writability of the disk mounted
    /// as `number`: the image's sectors for the BIOS, a floppy's for 0 and 1
    /// and a hard disk's for 2 and 3, whatever file system they hold, or
    /// none.
    fn raw_storage(number: u8, path: &Path, opts: &MountOptions) -> Result<(DriveKind, Storage, String, bool), String> {
        if diskimage::detect(path, opts.kind) == Ok(ImageKind::Cd) {
            return Err(format!("{} is a CD image, which can't be mounted by number", path.display()));
        }
        let floppy = number < FLOPPY_DRIVES;
        let disk = Self::open_disk(path, floppy, opts)?;
        let kind = if floppy { DriveKind::Floppy } else { DriveKind::HardDisk };
        let writable = disk.writable();
        Ok((kind, Storage::Raw(Rc::new(disk)), String::new(), writable))
    }

    /// Whether a CD image can go in `drive`: not in a floppy drive, and not
    /// as C:.
    fn check_cd_drive(drive: u8) -> Result<(), String> {
        if drive < FLOPPY_DRIVES {
            return Err(format!("Drive {}: is a floppy drive and can't be a CD-ROM", drive_letter(drive)));
        }
        if drive == DRIVE_C {
            return Err("Drive C: can't be a CD-ROM".to_string());
        }
        Ok(())
    }

    /// The type, storage, volume label and writability of a CD drive with
    /// `image` in it.
    fn cd_storage(image: CdImage) -> Result<(DriveKind, Storage, String, bool), String> {
        // A disc of only audio tracks has no file system.
        let (files, volume_label) = match image.data_track() {
            Some(_) => {
                let volume = crate::cdrom::iso9660::read_volume(&image)?;
                (volume.files, volume.label)
            }
            None => (MemFs::new(), "AUDIO_CD".to_string()),
        };
        let volume_label = if volume_label.is_empty() { "CDROM".to_string() } else { volume_label };
        Ok((DriveKind::CdRom, Storage::Tree { files, image: Some(Rc::new(image)) }, volume_label, false))
    }

    /// The type, storage, volume label and writability of `drive` with the
    /// floppy or hard disk image `open` opens, which was `found` to be of
    /// that kind and `name` names in messages.
    fn fat_storage(
        drive: u8,
        found: ImageKind,
        name: &str,
        open: impl FnOnce() -> Result<DiskImage, String>,
    ) -> Result<(DriveKind, Storage, String, bool), String> {
        let open = || -> Result<(Rc<DiskImage>, FatVolume), String> {
            let disk = Rc::new(open()?);
            let (start, sectors) = disk.fat_volume()?;
            let volume = FatVolume::open(disk.clone(), start, sectors)?;
            Ok((disk, volume))
        };
        let (disk, volume) = open().map_err(|e| match found {
            ImageKind::HardDisk if drive < FLOPPY_DRIVES => {
                format!("Drive {}: is a floppy drive and can't hold a hard disk image", drive_letter(drive))
            }
            _ => format!("{}: {}", name, e),
        })?;
        let volume_label = volume.label().unwrap_or_default();
        let kind = if disk.is_floppy() { DriveKind::Floppy } else { DriveKind::HardDisk };
        Ok((kind, Storage::Fat(Rc::new(volume)), volume_label, disk.writable()))
    }

    /// Mount a disk or CD image held in memory as `drive`, replacing what is
    /// there, as `mount` mounts an image file named `name`: which kind of
    /// image it is comes from `opts.kind`, the name and the contents (see
    /// `diskimage::detect_memory`).
    pub fn mount_memory_image(
        &mut self,
        drive: u8,
        name: &str,
        data: MemoryImage,
        opts: MountOptions,
    ) -> Result<(), String> {
        Self::check_image_drive(drive, &opts)?;
        let found = diskimage::detect_memory(name, &data, opts.kind)?;
        let (kind, storage, volume_label, writable) = if found == ImageKind::Cd {
            Self::check_cd_drive(drive)?;
            Self::cd_storage(CdImage::from_memory(name, data)?)?
        } else {
            let floppy = found == ImageKind::Floppy || drive < FLOPPY_DRIVES;
            let open = || DiskImage::from_memory(name, data, floppy, opts.geometry, opts.read_only);
            Self::fat_storage(drive, found, name, open)?
        };
        self.insert_image_drive(drive, kind, storage, &volume_label, writable, &opts);
        Ok(())
    }

    /// Mount the floppy or hard disk image `disk` as `drive`, replacing what
    /// is there: one made in memory (`DiskImage::blank_hard_disk`).
    pub fn mount_disk_image(&mut self, drive: u8, disk: DiskImage, opts: MountOptions) -> Result<(), String> {
        Self::check_image_drive(drive, &opts)?;
        let found = if disk.is_floppy() { ImageKind::Floppy } else { ImageKind::HardDisk };
        let name = disk.path().display().to_string();
        let (kind, storage, volume_label, writable) = Self::fat_storage(drive, found, &name, || Ok(disk))?;
        self.insert_image_drive(drive, kind, storage, &volume_label, writable, &opts);
        Ok(())
    }

    /// Whether an image held in memory can be mounted as `drive`.
    fn check_image_drive(drive: u8, opts: &MountOptions) -> Result<(), String> {
        if drive >= LASTDRIVE {
            return Err("Invalid drive letter".to_string());
        }
        if drive == DRIVE_Z || opts.kind == DriveKind::Virtual {
            return Err(format!("Drive {}: is reserved", drive_letter(drive)));
        }
        Ok(())
    }

    /// Put an image held in memory in `drive`, closing the files open on
    /// what was there.
    fn insert_image_drive(
        &mut self,
        drive: u8,
        kind: DriveKind,
        storage: Storage,
        volume_label: &str,
        writable: bool,
        opts: &MountOptions,
    ) {
        self.close_drive_files(drive);
        self.drives[drive as usize] = Some(Drive {
            kind,
            storage,
            current_dir: String::new(),
            label: Self::label_for(opts, volume_label),
            read_only: opts.read_only || !writable,
            mount: None,
            images: Vec::new(),
            image: 0,
            media_changed: true,
            boot_cd: None,
            shared: None,
            overlay: None,
        });
    }

    /// Put the next image in a drive mounted from a list of them, as
    /// Ctrl+F4 does. Files open on the drive keep reading the disk they were
    /// opened on, and the current directory stays if the new disk has it.
    /// Returns what changed, or None for a drive with one image or none.
    pub fn swap_image(&mut self, drive: u8) -> Result<Option<String>, String> {
        let Some(d) = self.drive(drive).filter(|d| d.images.len() > 1) else {
            return Ok(None);
        };
        self.select_image(drive, (d.image + 1) % d.images.len())
    }

    /// The images of a drive mounted from images, and which one is in.
    pub fn images(&self, drive: u8) -> Option<(&[PathBuf], usize)> {
        self.drive(drive).filter(|d| !d.images.is_empty()).map(|d| (d.images.as_slice(), d.image))
    }

    /// Put image `index` of a drive mounted from images in, as `swap_image`
    /// does the next. Returns what changed, or None if it is in already.
    pub fn select_image(&mut self, drive: u8, index: usize) -> Result<Option<String>, String> {
        let name = drive_name(drive);
        let Some(d) = self.drive(drive).filter(|d| index < d.images.len()) else {
            return Err(format!("Drive {} has no disk {}", name, index + 1));
        };
        if d.image == index {
            return Ok(None);
        }
        let path = d.images[index].clone();
        let opts = d.mount.as_ref().map(|m| m.opts.clone()).unwrap_or_default();
        let (kind, storage, volume_label, writable) = Self::open_image(drive, &path, &opts)?;
        let d = self.drives[drive as usize].as_mut().unwrap();
        if kind != d.kind {
            return Err(format!("{} can't go in drive {}, which is a {}", path.display(), name, d.kind.name()));
        }
        d.storage = storage;
        d.image = index;
        d.label = Self::label_for(&opts, &volume_label);
        d.read_only = opts.read_only || !writable;
        d.media_changed = true;
        let count = d.images.len();
        let current = format!("{}\\{}", name, d.current_dir);
        if drive < LASTDRIVE
            && !self.is_directory(&current)
            && let Some(d) = self.drives[drive as usize].as_mut()
        {
            d.current_dir.clear();
        }
        let file = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
        Ok(Some(format!("Drive {} disk {} of {}: {}", name, index + 1, count, file)))
    }

    /// Add `path` to the end of the images of a drive mounted from images,
    /// for `select_image` or Ctrl+F4 to put in later.
    pub fn add_image(&mut self, drive: u8, path: &Path) -> Result<(), String> {
        if !hostfs::is_file(path) {
            return Err(format!("{} is not a disk or CD image", path.display()));
        }
        let canonical = hostfs::canonicalize(path).map_err(|e| e.to_string())?;
        let elsewhere = (0..DRIVE_SLOTS).find(|&d| self.drive(d).is_some_and(|other| other.images.contains(&canonical)));
        if let Some(other) = elsewhere {
            return Err(format!("{} is already mounted as {}", path.display(), drive_name(other)));
        }
        let Some(d) = self.drives.get_mut(drive as usize).and_then(Option::as_mut).filter(|d| !d.images.is_empty())
        else {
            return Err(format!("Drive {} isn't mounted from disk images", drive_name(drive)));
        };
        // The mount as saved states have it lists them all.
        if let Some(mount) = d.mount.as_mut() {
            mount.opts.more_images.push(canonical.clone());
        }
        d.images.push(canonical);
        Ok(())
    }

    /// Take image `index`, which must not be the one in, out of the images
    /// of a drive mounted from images.
    pub fn remove_image(&mut self, drive: u8, index: usize) -> Result<(), String> {
        let name = drive_name(drive);
        let Some(d) = self.drives.get_mut(drive as usize).and_then(Option::as_mut).filter(|d| index < d.images.len())
        else {
            return Err(format!("Drive {} has no disk {}", name, index + 1));
        };
        if d.image == index {
            return Err(format!("Disk {} is in drive {}", index + 1, name));
        }
        d.images.remove(index);
        if d.image > index {
            d.image -= 1;
        }
        if let Some(mount) = d.mount.as_mut() {
            mount.path = d.images[0].clone();
            mount.opts.more_images = d.images[1..].to_vec();
        }
        Ok(())
    }

    /// Mount `files` as the read-only drive `drive`, which must not be
    /// mounted yet. C: and Z: are taken.
    pub fn mount_memory(&mut self, drive: u8, files: MemFs, label: &str) -> Result<(), String> {
        if drive >= LASTDRIVE || drive == DRIVE_C || drive == DRIVE_Z {
            return Err(format!("Drive {}: is reserved", drive_letter(drive.min(LASTDRIVE - 1))));
        }
        if self.is_mounted(drive) {
            return Err(format!("Drive {}: is already mounted", drive_letter(drive)));
        }
        self.drives[drive as usize] = Some(Self::memory_drive(files, label));
        Ok(())
    }

    /// C: empty and read-only in place of what was there, which is mounted
    /// elsewhere now (REMOUNT): C: is always there.
    pub fn empty_drive_c(&mut self) {
        self.close_drive_files(DRIVE_C);
        self.drives[DRIVE_C as usize] = Some(Self::memory_drive(MemFs::new(), ""));
    }

    /// Unmount `drive`, closing its open files. C: and Z: cannot be removed.
    /// If it was the current drive, C: becomes current.
    pub fn unmount(&mut self, drive: u8) -> Result<(), String> {
        if drive >= DRIVE_SLOTS || !self.is_mounted(drive) {
            return Err("Drive not mounted".to_string());
        }
        if drive == DRIVE_C || drive == DRIVE_Z {
            return Err(format!(
                "Drive {}: cannot be unmounted",
                drive_letter(drive)
            ));
        }
        self.close_drive_files(drive);
        self.drives[drive as usize] = None;
        if self.current_drive == drive {
            self.current_drive = DRIVE_C;
        }
        Ok(())
    }

    fn close_drive_files(&mut self, drive: u8) {
        // Closed by DOS, which emptied them at the create.
        self.settle_where(|sft, f| sft >= FIRST_FILE && f.drive == drive);
        let gone: Vec<u16> = self
            .open_files
            .iter()
            .filter(|&(&sft, f)| sft >= FIRST_FILE && f.drive == drive)
            .map(|(&sft, _)| sft)
            .collect();
        for sft in gone {
            self.open_files.remove(&sft);
            self.mark_dirty(sft);
        }
    }
}
