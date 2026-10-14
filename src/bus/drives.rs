//! Mounting drives and images, the drives' activity, and the BIOS's view
//! of the drive set.

use super::*;

impl Bus {
    /// Mount a host directory as a DOS drive and refresh the BIOS view of
    /// the drive set. See `DiskController::mount`.
    pub fn mount_drive(
        &mut self,
        drive: u8,
        path: &std::path::Path,
        opts: MountOptions,
        replace: bool,
    ) -> Result<std::path::PathBuf, String> {
        self.shared_in_use(drive)?;
        let boot = opts.boot;
        // A folder mounted on the letter of a booted system's CD-ROM drive
        // goes in as a disc made from it.
        let mut opts = opts;
        if self.boot.is_some() && crate::hostfs::is_dir(path) && self.booted_cd_drive() == Some(drive) {
            opts.kind = DriveKind::CdRom;
        }
        let mounted = self.mount_with(drive, |disk| disk.mount(drive, path, opts, replace));
        for note in std::mem::take(&mut self.disk.notes).into_iter().chain(crate::diskdelta::take_notes()) {
            self.log_string(&format!("[DISK] {}", note));
        }
        let mounted = mounted?;
        // One drive boots at startup.
        if boot {
            self.disk.set_boot_drive(Some(drive));
        }
        Ok(mounted)
    }

    /// Mount a disk or CD image held in memory as a DOS drive, replacing
    /// what is there. See `DiskController::mount_memory_image`.
    pub fn mount_memory_image(
        &mut self,
        drive: u8,
        name: &str,
        data: crate::diskimage::MemoryImage,
        opts: MountOptions,
    ) -> Result<(), String> {
        self.mount_with(drive, |disk| disk.mount_memory_image(drive, name, data, opts))
    }

    /// Mount a disk image made in memory as a DOS drive, replacing what is
    /// there. See `DiskController::mount_disk_image`.
    pub fn mount_disk_image(
        &mut self,
        drive: u8,
        image: crate::diskimage::DiskImage,
        opts: MountOptions,
    ) -> Result<(), String> {
        self.mount_with(drive, |disk| disk.mount_disk_image(drive, image, opts))
    }

    /// Mount `drive` with `mount` and refresh the BIOS view of the drive set.
    fn mount_with<T>(
        &mut self,
        drive: u8,
        mount: impl FnOnce(&mut DiskController) -> Result<T, String>,
    ) -> Result<T, String> {
        let result = mount(&mut self.disk);
        if result.is_ok() && self.boot.is_some() {
            for line in self.disk.prepare_boot_cds() {
                self.log_string(&format!("[IDE] {}", line));
            }
            if self.disk.boot_cd_image(drive).is_some() {
                self.ide_cd_follows(drive);
            }
        }
        if result.is_ok() {
            // Another disc: whatever played stops, and MSCDEX and a booted
            // system's CD-ROM drive say so.
            self.cdaudio.stop_drive(drive);
            self.mscdex.disc_changed(drive);
            self.booted_media_changed(drive);
        }
        self.sync_drive_bda();
        result
    }

    /// Put the Ultrasound software built into rust-dos (`gus::builtin`) on
    /// `drive`, or with None nowhere, in place of where it was.
    pub fn mount_ultrasnd(&mut self, drive: Option<u8>) -> Result<(), String> {
        // Unless a MOUNT has taken its place.
        if let Some(old) = self.ultrasnd_drive.take()
            && self.disk.drive_kind(old) == Some(DriveKind::Virtual)
        {
            let _ = self.disk.unmount(old);
        }
        let result = match drive {
            Some(drive) => {
                let files = crate::ultrasnd::drive(crate::disk::drive_letter(drive));
                self.disk.mount_memory(drive, files, crate::gus::builtin::LABEL).map(|()| {
                    self.ultrasnd_drive = Some(drive);
                })
            }
            None => Ok(()),
        };
        self.sync_drive_bda();
        result
    }

    /// A drive of `class` moved `bytes`: charge the time that takes at the
    /// drive's speed, and make its noise.
    pub fn disk_activity(&mut self, class: crate::diskio::DiskClass, bytes: u32, access: crate::disknoise::Access) {
        self.activity.io_bytes += bytes as u64;
        let pending = self.disk_io.charge(class, bytes);
        self.disk_noise(class, access, pending);
    }

    /// The noise of a drive of `class` that goes on for `ns` from now.
    pub fn disk_noise(&mut self, class: crate::diskio::DiskClass, access: crate::disknoise::Access, ns: u64) {
        if self.disknoise.enabled(class) {
            self.audio_catch_up();
            let frames = crate::timer::mul_div(ns, crate::opl::RATE as u64, 1_000_000_000);
            self.disknoise.io(class, access, frames);
        }
    }

    /// `bytes` were moved on `drive`, if it is a floppy drive or a hard
    /// disk.
    pub fn drive_activity(&mut self, drive: u8, bytes: u32, access: crate::disknoise::Access) {
        self.drives_active |= 1 << drive;
        match self.disk.drive_kind(drive).and_then(crate::diskio::DiskClass::of) {
            Some(class) => self.disk_activity(class, bytes, access),
            None => self.activity.io_bytes += bytes as u64,
        }
    }

    /// Sectors of a drive's disk image were read or written through the
    /// BIOS or INT 25h/26h, from sector `lba` of the disk on.
    pub fn sector_activity(&mut self, drive: u8, lba: u64, sectors: u32, _write: bool) {
        let per_track = self.disk.bios_image(drive).map_or(1, |disk| disk.geometry().sectors.max(1) as u64);
        let bytes = sectors.saturating_mul(crate::diskimage::SECTOR_SIZE as u32);
        self.drive_activity(drive, bytes, crate::disknoise::Access::Track(lba / per_track));
    }

    /// Put the next image in every drive mounted from a list of images, as
    /// Ctrl+F4 does. Returns what changed, and what went wrong.
    pub fn swap_images(&mut self) -> Vec<String> {
        let mut messages = Vec::new();
        // A booted system's disc made from a host folder is made again,
        // with what the folder holds now.
        if let Some(drive) = self.booted_cd_drive().filter(|&d| self.boot.is_some() && self.disk.is_folder_cd(d)) {
            match self.reinsert_cd(drive) {
                Ok(message) => messages.push(message),
                Err(e) => messages.push(e),
            }
        }
        for drive in 0..DRIVE_SLOTS {
            match self.disk.swap_image(drive) {
                Ok(Some(message)) => {
                    self.image_changed(drive);
                    messages.push(message);
                }
                Ok(None) => {}
                Err(e) => messages.push(e),
            }
        }
        if !messages.is_empty() {
            self.sync_drive_bda();
        }
        messages
    }

    /// Put image `index` in a drive mounted from a list of images, as
    /// libretro's disk control does. Returns what changed, if anything.
    pub fn select_image(&mut self, drive: u8, index: usize) -> Result<Option<String>, String> {
        let changed = self.disk.select_image(drive, index)?;
        if changed.is_some() {
            self.image_changed(drive);
            self.sync_drive_bda();
        }
        Ok(changed)
    }

    /// Make the disc of the host folder mounted as CD drive `drive` again,
    /// with what the folder holds now, and put it in the booted system's
    /// CD-ROM drive as a new disc. Returns what changed.
    pub fn reinsert_cd(&mut self, drive: u8) -> Result<String, String> {
        let lines = self.disk.make_boot_cd(drive)?;
        for line in &lines {
            self.log_string(&format!("[IDE] {}", line));
        }
        self.ide_cd_follows(drive);
        self.image_changed(drive);
        Ok(lines.into_iter().next().unwrap_or_default())
    }

    /// Tell the CD audio, MSCDEX, the IDE drive and the floppy disk
    /// controller that another disk is in `drive`.
    fn image_changed(&mut self, drive: u8) {
        self.cdaudio.stop_drive(drive);
        self.mscdex.disc_changed(drive);
        self.booted_media_changed(drive);
    }

    /// Tell a booted system's CD-ROM drive and floppy disk controller that
    /// the disk in `drive` changed.
    fn booted_media_changed(&mut self, drive: u8) {
        self.ide_media_changed(drive);
        self.fdc_media_changed(drive);
    }

    /// Copy what the booted system changed on the disks made of shared
    /// host directories into them (`drive`'s, or every one's), as it runs.
    /// Returns what happened, a line for each drive.
    pub fn sync_shared(&mut self, drive: Option<u8>) -> Vec<String> {
        let lines = self.disk.sync_shared(drive, false);
        for line in &lines {
            self.log_string(&format!("[BOOT] {}", line));
        }
        lines
    }

    /// Why `drive` can't be unmounted or mounted anew: it is a booted
    /// system's hard disk made of a host directory.
    fn shared_in_use(&self, drive: u8) -> Result<(), String> {
        match crate::boot::drive_unit(self, drive) {
            Some(unit) if self.boot.is_some() && self.disk.is_shared(drive) => Err(format!(
                "Drive {}: is the booted system's hard disk {:02X}h; shut it down first",
                crate::disk::drive_letter(drive),
                unit
            )),
            _ => Ok(()),
        }
    }

    /// Unmount a DOS drive and refresh the BIOS view of the drive set.
    pub fn unmount_drive(&mut self, drive: u8) -> Result<(), String> {
        self.shared_in_use(drive)?;
        let result = self.disk.unmount(drive);
        self.cdaudio.stop_drive(drive);
        self.booted_media_changed(drive);
        self.sync_drive_bda();
        result
    }

    /// The drive `from` as `to` (REMOUNT): the same
    /// folder or images, with their changes where they went. C:, which
    /// stays, is left empty.
    pub fn remount_drive(&mut self, from: u8, to: u8) -> Result<std::path::PathBuf, String> {
        if from == to {
            return Err("The drives are the same".to_string());
        }
        if self.disk.is_mounted(to) {
            return Err(format!("Drive {}: is already mounted", crate::disk::drive_letter(to)));
        }
        let spec = self.disk.drive_info(from).and_then(|info| info.mount).ok_or("Drive not mounted, or not from the host")?;
        self.shared_in_use(from)?;
        let current = self.disk.get_current_drive() == from;
        if from == crate::disk::DRIVE_C {
            self.disk.empty_drive_c();
        } else {
            self.disk.unmount(from)?;
        }
        let mounted = self.mount_drive(to, &spec.path, spec.opts.clone(), false);
        if mounted.is_err() {
            // Back where it was.
            let _ = self.mount_drive(from, &spec.path, spec.opts, from == crate::disk::DRIVE_C);
        } else if current {
            self.disk.set_current_drive(to);
        }
        self.cdaudio.stop_drive(from);
        self.booted_media_changed(from);
        self.sync_drive_bda();
        mounted
    }

    /// Mirror the mounted drives into the BIOS data area and DOS's data
    /// segment (`dos_data`). Must run whenever the drive set changes.
    pub fn sync_drive_bda(&mut self) {
        // A booted system's BIOS has its units from the start, and no
        // built-in DOS.
        if self.boot.is_some() {
            return;
        }
        // Equipment word: bit 0 = floppy present, bits 6-7 = floppy count - 1.
        // Only A: and B: are BIOS floppy units. Other bits are left alone.
        let floppies = self.disk.floppy_units();
        let mut equipment = self.read_16(0x0410) & !0x00C1;
        if floppies > 0 {
            equipment |= 0x0001 | ((floppies as u16 - 1) << 6);
        }
        self.write_16(0x0410, equipment);
        self.cmos.set_floppies(floppies);

        // 0x0475: number of fixed disks (INT 13h units 80h+).
        let hard_disks = self.disk.hard_disk_units(|_| true).len();
        self.write_8(0x0475, hard_disks.min(0xFF) as u8);

        crate::dos_data::write(self);
    }
}
