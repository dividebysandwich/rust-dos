//! The Drives page: mounting, swapping, booting and unmounting drives, the
//! mount dialog and the file browser.

use super::browser::{Browser, IMAGES, MT32_ROMS, SOUNDFONTS};
use super::dialog::{self, Event, MountDialog};
use super::item::Item;
use super::{ConfigUi, Host, Pick, UiKey, bootable, shareable};
use crate::disk::{DRIVE_C, DriveInfo, DriveKind, drive_number, numbered_unit};
use crate::mount::{contract_home, expand_host_path};
use std::path::Path;

impl ConfigUi {
    pub(super) fn drives_key(&mut self, key: UiKey, host: &mut dyn Host) {
        let selected = self.drives.get(self.row).cloned();
        match (key, selected) {
            // The row after "+ Mount a drive".
            (UiKey::Enter, None) if self.row == self.drives.len() + 1 => self.open_image_dialog(),
            (UiKey::Insert, _) | (UiKey::Enter, None) => self.new_drive(host),
            (UiKey::Enter, Some(info)) if info.kind == DriveKind::Virtual => {
                self.info(format!("{} is built into Rust-DOS", dialog::drive_title(info.drive)));
            }
            (UiKey::Enter, Some(info)) if !self.frontend.host_files => self.choose_image(Some(info.drive), host),
            (UiKey::Enter, Some(info)) => {
                self.status = None;
                self.dialog = Some(MountDialog::change(&info, self.home.as_deref()));
            }
            (UiKey::Delete, Some(info)) if info.kind == DriveKind::Virtual || info.drive == DRIVE_C => {
                self.error(format!("{} can't be unmounted", dialog::drive_title(info.drive)));
            }
            (UiKey::Delete, Some(info)) => self.unmount(info.drive, host),
            (UiKey::Char('b' | 'B'), Some(info)) if bootable(&info) => self.boot(info.drive, host),
            (UiKey::Char('b' | 'B'), Some(info)) => {
                self.error(format!("{} can't be booted: it isn't a disk image", dialog::drive_title(info.drive)));
            }
            (UiKey::Char('s' | 'S'), Some(info)) if self.shared_unit(info.drive).is_some() => {
                match host.sync_shared(info.drive) {
                    Ok(message) => self.info(message),
                    Err(e) => self.error(e),
                }
            }
            (UiKey::Char('r' | 'R'), Some(info)) if self.folder_cd(&info) => {
                let result = host.reinsert(info.drive);
                self.refresh_drives(host, Some(info.drive));
                match result {
                    Ok(message) => self.info(message),
                    Err(e) => self.error(e),
                }
            }
            _ => {}
        }
    }

    /// The BIOS unit of the booted system's hard disk made of `drive`'s
    /// host directory, if it has one.
    pub(super) fn shared_unit(&self, drive: u8) -> Option<u8> {
        self.booted.as_ref()?.shared.iter().find(|&&(d, _)| d == drive).map(|&(_, unit)| unit)
    }

    /// Whether `info` is a host folder in the booted system's CD-ROM drive.
    pub(super) fn folder_cd(&self, info: &DriveInfo) -> bool {
        info.root.is_some() && self.booted.as_ref().is_some_and(|b| b.cd_drive == Some(info.drive))
    }

    /// What the drive list says of `info` after its label: whether it boots
    /// or is read-only, and what a booted system has of it, or will have.
    pub(super) fn drive_flags(&self, info: &DriveInfo) -> String {
        let mut flags = Vec::new();
        // A disk mounted by number is the BIOS's unit.
        if let Some(number) = drive_number(info.drive) {
            flags.push(format!("{:02X}h", numbered_unit(number)));
        }
        if info.mount.as_ref().is_some_and(|m| m.opts.boot) {
            flags.push("boot".to_string());
        }
        if let Some(unit) = self.shared_unit(info.drive) {
            flags.push(format!("{:02X}h", unit));
        } else if self.booted.as_ref().is_some_and(|b| b.cd_drive == Some(info.drive)) {
            flags.push("CD".to_string());
        } else if self.booted.is_none() && shareable(info) {
            flags.push("share".to_string());
        }
        if info.read_only && info.kind != DriveKind::Virtual && flags.len() < 2 {
            flags.push("ro".to_string());
        } else if info.overlay.is_some() && flags.len() < 2 {
            flags.push("ovl".to_string());
        }
        flags.join(" ")
    }

    /// What mounting `drive` did for a booted system, for the message.
    fn booted_note(&self, drive: u8) -> &'static str {
        let Some(booted) = &self.booted else { return "" };
        let info = self.drives.iter().find(|d| d.drive == drive);
        if booted.cd_drive == Some(drive) {
            if info.is_some_and(|i| i.root.is_some()) {
                "; the booted system has it as a CD (R makes it again)"
            } else {
                "; the booted system has the disc"
            }
        } else if info.is_some_and(shareable) {
            "; the booted system gets it as a hard disk when it boots again"
        } else if info.is_some_and(|i| i.kind == DriveKind::CdRom) && booted.cd_drive.is_some() {
            "; the booted system's CD-ROM drive shows another drive"
        } else {
            ""
        }
    }

    /// Boot from `drive` now, and close the window on the booted system.
    fn boot(&mut self, drive: u8, host: &mut dyn Host) {
        match host.boot(drive) {
            Ok(message) => {
                self.dialog = None;
                self.close();
                self.notice = Some(message);
            }
            Err(e) => self.error(e),
        }
    }

    fn new_drive(&mut self, host: &mut dyn Host) {
        if !self.frontend.host_files {
            return self.choose_image(None, host);
        }
        match MountDialog::new_drive(&self.drives) {
            Some(dialog) => {
                self.status = None;
                self.dialog = Some(dialog);
            }
            None => self.error("Every drive letter is taken"),
        }
    }

    /// Without the host's files, the frontend picks the image.
    fn choose_image(&mut self, drive: Option<u8>, host: &mut dyn Host) {
        match host.choose_image(drive) {
            Ok(()) => self.status = None,
            Err(e) => self.error(e),
        }
    }

    /// The drives changed outside the window (Ctrl+F4, an image the
    /// frontend picked): show them as they are now, and what happened.
    pub fn drives_changed(&mut self, host: &dyn Host, message: &str) {
        self.refresh_drives(host, None);
        self.info(message);
    }

    pub(super) fn refresh_drives(&mut self, host: &dyn Host, select: Option<u8>) {
        self.drives = host.drives();
        self.booted = host.booted();
        if let Some(drive) = select
            && let Some(i) = self.drives.iter().position(|d| d.drive == drive)
        {
            self.row = i;
        }
        self.row = self.row.min(self.row_count().saturating_sub(1));
    }

    fn unmount(&mut self, drive: u8, host: &mut dyn Host) {
        match host.unmount(drive) {
            Ok(()) => {
                self.dialog = None;
                self.refresh_drives(host, None);
                self.info(format!("{} has been unmounted", dialog::drive_title(drive)));
            }
            Err(e) => self.error(e),
        }
    }

    pub(super) fn dialog_key(&mut self, key: UiKey, host: &mut dyn Host) {
        let Some(dialog) = &mut self.dialog else { return };
        match dialog.key(key) {
            Event::None => {}
            Event::Cancel => {
                self.dialog = None;
                self.status = None;
            }
            Event::Browse => self.open_browser(Pick::MountPath),
            Event::BrowseOverlay => self.open_browser(Pick::OverlayPath),
            Event::Unmount => {
                let drive = dialog.drive;
                self.unmount(drive, host);
            }
            event @ (Event::Submit | Event::Boot) => {
                let cwd = std::env::current_dir().unwrap_or_default();
                let replace = dialog.existing;
                let spec = match dialog.spec(&cwd, self.home.as_deref()) {
                    Ok(spec) => spec,
                    Err(e) => return self.error(e),
                };
                let (drive, boots) = (spec.drive, spec.opts.boot);
                let was_boot = self.drives.iter().any(|d| d.drive == drive && d.mount.as_ref().is_some_and(|m| m.opts.boot));
                // Only whether it boots at startup changed, or it boots
                // now as it is: the drive (a booted system's disk, maybe)
                // isn't mounted again.
                let message = if (boots != was_boot || event == Event::Boot) && dialog.same_mount(&spec) {
                    if boots != was_boot
                        && let Err(e) = host.set_boot(drive, boots)
                    {
                        return self.error(e);
                    }
                    self.dialog = None;
                    self.refresh_drives(host, Some(drive));
                    if boots {
                        format!("{} boots when Rust-DOS starts; F2 saves it", dialog::drive_title(drive))
                    } else {
                        format!("{} doesn't boot at startup any more; F2 saves it", dialog::drive_title(drive))
                    }
                } else {
                    match host.mount(spec, replace) {
                        Ok(path) => {
                            self.dialog = None;
                            self.refresh_drives(host, Some(drive));
                            let kind = self.drives.get(self.row).map_or("", |d| d.kind.name());
                            let shown = contract_home(&path, self.home.as_deref());
                            let boot = if boots { ", booting at startup once saved (F2)" } else { "" };
                            let note = self.booted_note(drive);
                            format!("{} is mounted as {} {}{}{}", dialog::drive_title(drive), kind, shown, boot, note)
                        }
                        Err(e) => return self.error(e),
                    }
                };
                if event == Event::Boot {
                    self.boot(drive, host);
                } else {
                    self.info(message);
                }
            }
        }
    }

    pub(super) fn open_browser(&mut self, pick: Pick) {
        let cwd = std::env::current_dir().unwrap_or_default();
        let home = self.home.as_deref();
        let (title, current, pick_dirs, extensions) = match pick {
            Pick::MountPath => (
                "Pick a directory or disk image",
                self.dialog.as_ref().map(|d| d.path.text()).unwrap_or_default(),
                true,
                IMAGES,
            ),
            Pick::OverlayPath => (
                "Pick the folder for the drive's changes",
                self.dialog.as_ref().map(|d| d.overlay.text()).unwrap_or_default(),
                true,
                &[][..],
            ),
            Pick::SoundFont => (
                "Pick a SoundFont (.sf2)",
                self.settings.sound.soundfont.as_deref().map(|p| p.display().to_string()).unwrap_or_default(),
                false,
                SOUNDFONTS,
            ),
            Pick::VrScene => (
                "Pick a 3D scene exported from Blender (.glb, .gltf)",
                self.settings.vr.scene.as_deref().map(|p| p.display().to_string()).unwrap_or_default(),
                false,
                &["glb", "gltf"][..],
            ),
            Pick::Mt32Roms => (
                "Pick the directory with the MT-32's ROMs",
                self.settings.sound.mt32roms.as_deref().map(|p| p.display().to_string()).unwrap_or_default(),
                true,
                MT32_ROMS,
            ),
            Pick::Sc55Roms => (
                "Pick the directory with the Sound Canvas's ROMs",
                self.settings.sound.sc55roms.as_deref().map(|p| p.display().to_string()).unwrap_or_default(),
                true,
                &["bin", "rom", "zip"][..],
            ),
            Pick::Awe32Rom => (
                "Pick the AWE32's ROM (awe32.raw)",
                self.settings.sound.awe32rom.as_deref().map(|p| p.display().to_string()).unwrap_or_default(),
                false,
                &["raw", "rom", "bin"][..],
            ),
            Pick::ImportGame => (
                "Pick a game's package, a GOG game's folder or a DOSBox .conf",
                String::new(),
                true,
                &["conf", "zip", "7z", "dosz", "dosc"][..],
            ),
            Pick::AchievementsArchive => ("Pick the zip or .dosz the game came in", String::new(), false, &["zip", "dosz"][..]),
            Pick::Manual => ("Pick a manual: a PDF or a picture", String::new(), false, &["pdf", "png", "jpg", "jpeg", "gif"][..]),
            Pick::ImagePath => (
                "Pick the directory for the new image",
                self.image_dialog.as_ref().map(|d| d.path.text()).unwrap_or_default(),
                true,
                IMAGES,
            ),
        };
        let start = if current.trim().is_empty() {
            self.config_file.as_deref().and_then(Path::parent).map_or(cwd.clone(), Path::to_path_buf)
        } else {
            expand_host_path(current.trim(), &cwd, home)
        };
        self.browser = Some((Browser::new(title, &start, pick_dirs, extensions), pick));
        self.browser_scroll = 0;
        self.status = None;
    }

    pub(super) fn browser_key(&mut self, key: UiKey, host: &mut dyn Host) {
        let Some((browser, pick)) = &mut self.browser else { return };
        let pick = *pick;
        if let Some(row) = Self::navigate(key, browser.selected, browser.rows(), self.visible) {
            browser.select(row);
            return;
        }
        let result = match key {
            UiKey::Esc => {
                self.browser = None;
                return;
            }
            UiKey::Enter => browser.activate(),
            UiKey::Backspace => browser.parent().map(|()| None),
            UiKey::Left => browser.step_drive(-1).map(|()| None),
            UiKey::Right => browser.step_drive(1).map(|()| None),
            UiKey::Char(c) => {
                browser.jump(c);
                Ok(None)
            }
            _ => Ok(None),
        };
        match result {
            Ok(Some(path)) => {
                self.browser = None;
                match pick {
                    Pick::MountPath => {
                        if let Some(dialog) = &mut self.dialog {
                            dialog.picked(&path, self.home.as_deref());
                        }
                    }
                    Pick::OverlayPath => {
                        if let Some(dialog) = &mut self.dialog {
                            dialog.picked_overlay(&path, self.home.as_deref());
                        }
                    }
                    Pick::ImagePath => {
                        if let Some(dialog) = &mut self.image_dialog {
                            dialog.picked(&path, self.home.as_deref());
                        }
                    }
                    Pick::SoundFont => {
                        self.settings.sound.soundfont = Some(path);
                        self.changed(Item::SoundFont, host);
                    }
                    Pick::VrScene => {
                        self.settings.vr.scene = Some(path);
                        self.changed(Item::VrScene, host);
                    }
                    Pick::Awe32Rom => match crate::awe32::rom::load(&path) {
                        Ok(_) => {
                            self.settings.sound.awe32rom = Some(path);
                            self.changed(Item::Awe32Rom, host);
                        }
                        Err(e) => self.error(e),
                    },
                    Pick::Manual => self.add_manual(&path, host),
                    Pick::AchievementsArchive => match host.identify_game(&path) {
                        Ok(message) => {
                            self.info(message);
                            self.achievements.refresh(host);
                        }
                        Err(e) => self.error(e),
                    },
                    Pick::ImportGame => match host.import_game(&path) {
                        Ok((id, message)) => {
                            self.refresh_games(host);
                            if let Some(i) = self.games.iter().position(|g| g.id == id) {
                                self.row = i;
                            }
                            self.info(message);
                        }
                        Err(e) => self.error(e),
                    },
                    // A ROM picked stands for its directory.
                    Pick::Mt32Roms => {
                        let dir = if path.is_dir() { path } else { path.parent().map(Path::to_path_buf).unwrap_or(path) };
                        self.settings.sound.mt32roms = Some(dir);
                        self.changed(Item::Mt32Roms, host);
                    }
                    Pick::Sc55Roms => {
                        let dir = if path.is_dir() { path } else { path.parent().map(Path::to_path_buf).unwrap_or(path) };
                        self.settings.sound.sc55roms = Some(dir);
                        self.changed(Item::Sc55Roms, host);
                    }
                }
            }
            Ok(None) => self.status = None,
            Err(e) => self.error(e),
        }
    }
}
