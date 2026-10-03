//! The settings window's text fields and its dialog for mounting a drive,
//! and booting from it.

use super::UiKey;
use crate::disk::{DRIVE_C, DRIVE_Z, DriveInfo, DriveKind, FLOPPY_DRIVES, LASTDRIVE, MountOptions, drive_letter};
use crate::diskimage::{self, Chs, ImageKind};
use crate::mount::{MountSpec, contract_home, expand_host_path};
use std::path::{Path, PathBuf};

/// A line of text being edited.
#[derive(Clone, Debug, Default)]
pub struct TextField {
    chars: Vec<char>,
    /// Cursor position, in characters.
    cursor: usize,
}

impl TextField {
    pub fn new(text: &str) -> Self {
        let chars: Vec<char> = text.chars().collect();
        Self { cursor: chars.len(), chars }
    }

    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    /// Edit with `key`. Returns false for keys a text field doesn't take.
    pub fn key(&mut self, key: UiKey) -> bool {
        match key {
            UiKey::Char(c) if !c.is_control() => {
                self.chars.insert(self.cursor, c);
                self.cursor += 1;
            }
            UiKey::Left => self.cursor = self.cursor.saturating_sub(1),
            UiKey::Right => self.cursor = (self.cursor + 1).min(self.chars.len()),
            UiKey::Home => self.cursor = 0,
            UiKey::End => self.cursor = self.chars.len(),
            UiKey::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                self.chars.remove(self.cursor);
            }
            UiKey::Delete if self.cursor < self.chars.len() => {
                self.chars.remove(self.cursor);
            }
            UiKey::Backspace | UiKey::Delete => {}
            _ => return false,
        }
        true
    }

    /// The part of the text that shows in `width` columns, scrolled so the
    /// cursor is visible, and the cursor's column in it.
    pub fn view(&self, width: usize) -> (String, usize) {
        let width = width.max(1);
        let first = (self.cursor + 1).saturating_sub(width);
        let text = self.chars[first..].iter().take(width).collect();
        (text, self.cursor - first)
    }
}

/// The dialog's controls, in focus order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Drive,
    Path,
    Browse,
    Kind,
    Label,
    ReadOnly,
    /// The folder the drive's changes go to (`-overlay`), and the button
    /// that picks it.
    Overlay,
    OverlayBrowse,
    /// Whether the image boots when Rust-DOS starts.
    BootFlag,
    Mount,
    /// Mount, and boot from the drive now.
    Boot,
    Unmount,
    Cancel,
}

const KINDS: [DriveKind; 3] = [DriveKind::HardDisk, DriveKind::Floppy, DriveKind::CdRom];

/// What a key did in the dialog.
#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    None,
    Cancel,
    Browse,
    /// Pick the folder for the drive's changes.
    BrowseOverlay,
    Submit,
    Boot,
    Unmount,
}

/// Mounting a new drive, or changing (swapping) a mounted one.
pub struct MountDialog {
    /// The drive is mounted: its letter is fixed and mounting replaces it.
    pub existing: bool,
    pub drive: u8,
    /// Letters a new drive can take.
    free: Vec<u8>,
    pub path: TextField,
    pub kind: DriveKind,
    pub label: TextField,
    pub read_only: bool,
    /// The folder the drive's changes go to, which leave a directory or
    /// archive as it is: empty for none.
    pub overlay: TextField,
    pub boot: bool,
    pub focus: Field,
    /// What the dialog doesn't show of the mount it changes, kept while
    /// the path stays as it was: the drive's other images and a hard disk
    /// image's geometry.
    original: String,
    more_images: Vec<PathBuf>,
    geometry: Option<Chs>,
    /// Its IDE slot for a booted system, and whether a host directory is
    /// shared with one, which stay too.
    ide: Option<crate::ide::IdeSlot>,
    share: Option<bool>,
    /// The mount it changes.
    current: Option<MountSpec>,
}

impl MountDialog {
    /// A dialog for a new drive, on the first free letter from D:. None if
    /// every letter is taken.
    pub fn new_drive(drives: &[DriveInfo]) -> Option<Self> {
        let free: Vec<u8> =
            (0..LASTDRIVE).filter(|&d| d != DRIVE_Z && !drives.iter().any(|i| i.drive == d)).collect();
        let drive = free.iter().copied().find(|&d| d >= 3).or_else(|| free.first().copied())?;
        Some(Self {
            existing: false,
            drive,
            free,
            path: TextField::default(),
            kind: Self::default_kind(drive),
            label: TextField::default(),
            read_only: false,
            overlay: TextField::default(),
            boot: false,
            focus: Field::Path,
            original: String::new(),
            more_images: Vec::new(),
            geometry: None,
            ide: None,
            share: None,
            current: None,
        })
    }

    /// A dialog for changing `info`'s drive, filled in with its mount.
    pub fn change(info: &DriveInfo, home: Option<&Path>) -> Self {
        let (path, opts) = match &info.mount {
            Some(spec) if spec.path.is_absolute() => (spec.path.clone(), spec.opts.clone()),
            spec => (
                info.root.clone().or(info.image.clone()).unwrap_or_default(),
                spec.as_ref().map(|s| s.opts.clone()).unwrap_or_default(),
            ),
        };
        let path = contract_home(&path, home);
        Self {
            existing: true,
            drive: info.drive,
            free: Vec::new(),
            path: TextField::new(&path),
            kind: info.kind,
            label: TextField::new(opts.label.as_deref().unwrap_or("")),
            read_only: opts.read_only,
            overlay: TextField::new(&opts.overlay.as_deref().map(|p| contract_home(p, home)).unwrap_or_default()),
            boot: opts.boot,
            focus: Field::Path,
            original: path,
            more_images: opts.more_images,
            geometry: opts.geometry,
            ide: opts.ide,
            share: opts.share,
            current: info.mount.clone(),
        }
    }

    fn default_kind(drive: u8) -> DriveKind {
        if drive < FLOPPY_DRIVES { DriveKind::Floppy } else { DriveKind::HardDisk }
    }

    /// A: and B: are always floppies.
    pub fn kind_fixed(&self) -> bool {
        self.drive < FLOPPY_DRIVES
    }

    /// A CD-ROM drive has nothing written to it to keep apart.
    pub fn can_overlay(&self) -> bool {
        self.kind != DriveKind::CdRom
    }

    /// Only disk images boot, and a CD image doesn't.
    pub fn can_boot(&self) -> bool {
        self.kind != DriveKind::CdRom
    }

    pub fn title(&self) -> String {
        if self.existing {
            format!("Change drive {}:", drive_letter(self.drive))
        } else {
            "Mount a drive".to_string()
        }
    }

    /// The controls that take the focus.
    pub fn fields(&self) -> Vec<Field> {
        use Field::*;
        let mut fields = vec![];
        if !self.existing {
            fields.push(Drive);
        }
        fields.extend([Path, Browse]);
        if !self.kind_fixed() {
            fields.push(Kind);
        }
        fields.extend([Label, ReadOnly]);
        if self.can_overlay() {
            fields.extend([Overlay, OverlayBrowse]);
        }
        if self.can_boot() {
            fields.push(BootFlag);
        }
        fields.push(Mount);
        if self.can_boot() {
            fields.push(Boot);
        }
        if self.existing && self.drive != DRIVE_C {
            fields.push(Unmount);
        }
        fields.push(Cancel);
        fields
    }

    fn move_focus(&mut self, step: isize) {
        let fields = self.fields();
        let at = fields.iter().position(|&f| f == self.focus).unwrap_or(0) as isize;
        self.focus = fields[(at + step).rem_euclid(fields.len() as isize) as usize];
    }

    fn is_button(field: Field) -> bool {
        matches!(field, Field::Browse | Field::OverlayBrowse | Field::Mount | Field::Boot | Field::Unmount | Field::Cancel)
    }

    /// Step a choice (drive letter, type, read-only) left or right.
    pub fn step(&mut self, field: Field, dir: isize) {
        match field {
            Field::Drive if !self.free.is_empty() => {
                let was_fixed = self.kind_fixed();
                let at = self.free.iter().position(|&d| d == self.drive).unwrap_or(0) as isize;
                self.drive = self.free[(at + dir).rem_euclid(self.free.len() as isize) as usize];
                if self.kind_fixed() != was_fixed {
                    self.kind = Self::default_kind(self.drive);
                }
            }
            Field::Kind if !self.kind_fixed() => {
                let at = KINDS.iter().position(|&k| k == self.kind).unwrap_or(0) as isize;
                self.kind = KINDS[(at + dir).rem_euclid(KINDS.len() as isize) as usize];
            }
            Field::ReadOnly => self.read_only = !self.read_only,
            Field::BootFlag => self.boot = !self.boot,
            _ => {}
        }
    }

    pub fn key(&mut self, key: UiKey) -> Event {
        let text = match self.focus {
            Field::Path => Some(&mut self.path),
            Field::Label => Some(&mut self.label),
            Field::Overlay => Some(&mut self.overlay),
            _ => None,
        };
        if let Some(field) = text
            && field.key(key)
        {
            return Event::None;
        }
        match key {
            UiKey::Esc => return Event::Cancel,
            UiKey::Enter => {
                return match self.focus {
                    Field::Browse => Event::Browse,
                    Field::OverlayBrowse => Event::BrowseOverlay,
                    Field::Boot => Event::Boot,
                    Field::Unmount => Event::Unmount,
                    Field::Cancel => Event::Cancel,
                    _ => Event::Submit,
                };
            }
            UiKey::Tab | UiKey::Down => self.move_focus(1),
            UiKey::BackTab | UiKey::Up => self.move_focus(-1),
            // Between the buttons, left and right move the focus.
            UiKey::Left if Self::is_button(self.focus) => self.move_focus(-1),
            UiKey::Right if Self::is_button(self.focus) => self.move_focus(1),
            UiKey::Left => self.step(self.focus, -1),
            UiKey::Right | UiKey::Char(' ') => self.step(self.focus, 1),
            _ => {}
        }
        Event::None
    }

    /// Take a path picked in the browser. An image sets the drive type to
    /// its own, except on A: and B:, where mounting a CD will fail.
    pub fn picked(&mut self, path: &Path, home: Option<&Path>) {
        self.path = TextField::new(&contract_home(path, home));
        if path.is_file() && !self.kind_fixed() {
            self.kind = match diskimage::detect(path, DriveKind::HardDisk) {
                Ok(ImageKind::Cd) => DriveKind::CdRom,
                Ok(ImageKind::Floppy) => DriveKind::Floppy,
                _ => DriveKind::HardDisk,
            };
        }
        self.focus = Field::Mount;
    }

    /// Take a folder picked in the browser for the drive's changes.
    pub fn picked_overlay(&mut self, path: &Path, home: Option<&Path>) {
        self.overlay = TextField::new(&contract_home(path, home));
        self.focus = Field::Mount;
    }

    /// The mount the dialog asks for. Relative paths are taken from `cwd`,
    /// as MOUNT does.
    pub fn spec(&self, cwd: &Path, home: Option<&Path>) -> Result<MountSpec, String> {
        let raw = self.path.text();
        let raw = raw.trim();
        if raw.is_empty() {
            return Err("Enter a host directory or a disk or CD image".to_string());
        }
        let label = self.label.text().trim().to_string();
        let overlay = self.overlay.text().trim().to_string();
        if raw.contains('"') || label.contains('"') || overlay.contains('"') {
            return Err("The configuration file can't hold a '\"'".to_string());
        }
        let path = expand_host_path(raw, cwd, home);
        let boot = self.boot && self.can_boot();
        if boot && path.is_dir() {
            return Err("Only a disk image can boot".to_string());
        }
        let overlay = (!overlay.is_empty() && self.can_overlay()).then(|| expand_host_path(&overlay, cwd, home));
        if overlay.is_some() {
            if self.read_only {
                return Err("A read-only drive has no changes to keep apart: leave \"Write to\" empty".to_string());
            }
            if path.is_file() && !crate::archive::is_archive_name(&path) {
                return Err("Only a directory or a zip or 7z archive keeps its changes apart".to_string());
            }
            if overlay.as_deref() == Some(path.as_path()) {
                return Err("The changes need a folder of their own".to_string());
            }
        }
        let unchanged = raw == self.original;
        Ok(MountSpec {
            drive: self.drive,
            path,
            opts: MountOptions {
                kind: self.kind,
                label: (!label.is_empty()).then_some(label),
                read_only: self.read_only,
                more_images: if unchanged { self.more_images.clone() } else { Vec::new() },
                geometry: if unchanged { self.geometry } else { None },
                overlay,
                ide: self.ide,
                boot,
                share: self.share,
                variant: None,
            },
        })
    }

    /// Whether `spec` mounts the drive as it is, but for whether it boots
    /// at startup: it needn't be mounted again.
    pub fn same_mount(&self, spec: &MountSpec) -> bool {
        let Some(current) = &self.current else { return false };
        let same_path = current.path == spec.path
            || std::fs::canonicalize(&current.path).is_ok_and(|p| std::fs::canonicalize(&spec.path).is_ok_and(|q| p == q));
        same_path && MountOptions { boot: spec.opts.boot, ..current.opts.clone() } == spec.opts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn text_fields_edit_at_the_cursor() {
        let mut f = TextField::new("dos");
        for key in [UiKey::Home, UiKey::Char('~'), UiKey::Char('/'), UiKey::End, UiKey::Backspace] {
            assert!(f.key(key));
        }
        assert_eq!(f.text(), "~/do");
        assert!(!f.key(UiKey::Enter));
        // The cursor after the end takes a column of its own.
        assert_eq!(f.view(3), ("do".to_string(), 2));
        f.key(UiKey::Home);
        assert_eq!(f.view(3), ("~/d".to_string(), 0));
    }

    fn drive(drive: u8, kind: DriveKind) -> DriveInfo {
        DriveInfo {
            drive,
            kind,
            root: Some(PathBuf::from("/x")),
            overlay: None,
            image: None,
            label: String::new(),
            read_only: false,
            current_dir: String::new(),
            mount: Some(MountSpec { drive, path: "/x".into(), opts: MountOptions::default() }),
            images: Vec::new(),
            image_index: 0,
        }
    }

    #[test]
    fn new_drives_take_free_letters() {
        let drives = [drive(2, DriveKind::HardDisk), drive(3, DriveKind::CdRom), drive(25, DriveKind::Virtual)];
        let mut d = MountDialog::new_drive(&drives).unwrap();
        assert_eq!(d.drive, 4);
        assert_eq!(d.focus, Field::Path);
        d.focus = Field::Drive;
        d.key(UiKey::Left);
        assert_eq!(d.drive, 1);
        d.key(UiKey::Left);
        d.key(UiKey::Left);
        // Past A: to Y:, skipping Z:
        assert_eq!(d.drive, 24);

        d.focus = Field::Path;
        for c in "/tmp".chars() {
            d.key(UiKey::Char(c));
        }
        d.key(UiKey::Tab);
        d.key(UiKey::Tab);
        d.key(UiKey::Right);
        assert_eq!(d.kind, DriveKind::Floppy);
        assert_eq!(d.key(UiKey::Enter), Event::Submit);
        let spec = d.spec(Path::new("/"), None).unwrap();
        assert_eq!((spec.drive, spec.path.as_path(), spec.opts.kind), (24, Path::new("/tmp"), DriveKind::Floppy));
    }

    #[test]
    fn a_and_b_are_only_floppies() {
        let drives = [drive(2, DriveKind::HardDisk), drive(25, DriveKind::Virtual)];
        let mut d = MountDialog::new_drive(&drives).unwrap();
        d.kind = DriveKind::CdRom;
        d.focus = Field::Drive;
        d.key(UiKey::Left);
        assert_eq!((d.drive, d.kind), (1, DriveKind::Floppy));
        assert!(!d.fields().contains(&Field::Kind));
        d.step(Field::Kind, 1);
        d.key(UiKey::Left);
        assert_eq!((d.drive, d.kind), (0, DriveKind::Floppy));
        d.key(UiKey::Right);
        d.key(UiKey::Right);
        assert_eq!((d.drive, d.kind), (3, DriveKind::HardDisk));

        // With C: to Z: taken, a new drive is A:.
        let taken: Vec<DriveInfo> = (2..26).map(|n| drive(n, DriveKind::HardDisk)).collect();
        let d = MountDialog::new_drive(&taken).unwrap();
        assert_eq!((d.drive, d.kind), (0, DriveKind::Floppy));
    }

    #[test]
    fn changing_a_drive_keeps_its_mount() {
        let mut info = drive(3, DriveKind::Floppy);
        info.mount.as_mut().unwrap().opts = MountOptions {
            kind: DriveKind::Floppy,
            label: Some("D1".into()),
            read_only: true,
            more_images: vec!["/y".into()],
            geometry: None,
            overlay: None,
            ide: None,
            boot: false,
            share: None,
            variant: None,
        };
        let mut d = MountDialog::change(&info, None);
        assert_eq!((d.path.text(), d.label.text(), d.read_only, d.kind), ("/x".into(), "D1".into(), true, DriveKind::Floppy));
        assert_eq!(
            d.fields(),
            [
                Field::Path,
                Field::Browse,
                Field::Kind,
                Field::Label,
                Field::ReadOnly,
                Field::Overlay,
                Field::OverlayBrowse,
                Field::BootFlag,
                Field::Mount,
                Field::Boot,
                Field::Unmount,
                Field::Cancel
            ]
        );
        // The drive's other images stay while the path does.
        assert_eq!(d.spec(Path::new("/"), None).unwrap().opts.more_images, [PathBuf::from("/y")]);
        d.path = TextField::new("/z");
        assert!(d.spec(Path::new("/"), None).unwrap().opts.more_images.is_empty());
        d.focus = Field::Mount;
        d.key(UiKey::Right);
        assert_eq!(d.key(UiKey::Enter), Event::Boot);
        d.key(UiKey::Right);
        assert_eq!(d.key(UiKey::Enter), Event::Unmount);
        assert_eq!(d.key(UiKey::Esc), Event::Cancel);
        // C: can't be unmounted.
        assert!(!MountDialog::change(&drive(2, DriveKind::HardDisk), None).fields().contains(&Field::Unmount));
        d.path = TextField::default();
        assert!(d.spec(Path::new("/"), None).is_err());
    }

    #[test]
    fn a_drive_keeps_its_changes_in_the_folder_given() {
        let mut info = drive(3, DriveKind::HardDisk);
        info.mount.as_mut().unwrap().opts.overlay = Some("/home/u/saves/d".into());
        let mut d = MountDialog::change(&info, Some(Path::new("/home/u")));
        assert_eq!(d.overlay.text(), "~/saves/d");
        let spec = d.spec(Path::new("/"), Some(Path::new("/home/u"))).unwrap();
        assert_eq!(spec.opts.overlay, Some(PathBuf::from("/home/u/saves/d")));
        // Typed, relative to where MOUNT takes them from.
        d.focus = Field::Overlay;
        for _ in 0..9 {
            d.key(UiKey::Backspace);
        }
        for c in "changes".chars() {
            d.key(UiKey::Char(c));
        }
        assert_eq!(d.spec(Path::new("/w"), None).unwrap().opts.overlay, Some(PathBuf::from("/w/changes")));
        // Not on a read-only drive, nor on a CD-ROM, which has none.
        d.read_only = true;
        assert!(d.spec(Path::new("/w"), None).is_err());
        d.read_only = false;
        d.kind = DriveKind::CdRom;
        assert!(!d.fields().contains(&Field::Overlay));
        assert_eq!(d.spec(Path::new("/w"), None).unwrap().opts.overlay, None);
        // Emptied: written in place.
        d.kind = DriveKind::HardDisk;
        d.overlay = TextField::default();
        assert_eq!(d.spec(Path::new("/w"), None).unwrap().opts.overlay, None);
        d.focus = Field::OverlayBrowse;
        assert_eq!(d.key(UiKey::Enter), Event::BrowseOverlay);
    }

    #[test]
    fn disk_images_boot_now_or_at_startup() {
        let dir = std::env::temp_dir().join(format!("rust-dos-dialog-boot-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let image = dir.join("win95.img");
        std::fs::write(&image, [0u8; 512]).unwrap();
        let mut info = drive(2, DriveKind::HardDisk);
        info.mount.as_mut().unwrap().path = image.clone();
        let mut d = MountDialog::change(&info, None);
        assert!(!d.boot);
        d.focus = Field::BootFlag;
        d.key(UiKey::Right);
        let spec = d.spec(&dir, None).unwrap();
        assert!(spec.opts.boot);
        // Only whether it boots changed: the drive needn't be mounted again.
        assert!(d.same_mount(&spec));
        d.read_only = true;
        assert!(!d.same_mount(&d.spec(&dir, None).unwrap()));

        // A directory doesn't boot, and a CD has no say.
        d.path = TextField::new(&dir.display().to_string());
        assert!(d.spec(&dir, None).is_err());
        d.kind = DriveKind::CdRom;
        assert!(!d.fields().contains(&Field::BootFlag) && !d.fields().contains(&Field::Boot));
        assert!(!d.spec(&dir, None).unwrap().opts.boot);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
