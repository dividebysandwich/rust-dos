//! The Drives page's dialog for making a new disk image, as MAKEIMG does
//! (makeimg.rs), and mounting it: where it goes, the kind of disk (a
//! floppy format, a hard disk of a preset size or of any size), its label
//! and the drive to mount it on.

use super::dialog::TextField;
use super::draw::{self, Grid};
use super::{ConfigUi, Hit, Host, Target, UiKey, fit};
use crate::disk::{DRIVE_Z, DriveInfo, DriveKind, FLOPPY_DRIVES, LASTDRIVE, MountOptions, drive_letter};
use crate::makeimg::{self, ImageSpec, PRESETS, Plan};
use crate::mount::{MountSpec, contract_home, expand_host_path};
use std::path::{Path, PathBuf};

/// The dialog's controls, in focus order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageField {
    Path,
    Browse,
    Kind,
    /// The size in MB, of a hard disk of any size.
    Size,
    Label,
    Mount,
    Create,
    Cancel,
}

/// What a key did in the dialog.
#[derive(Debug, PartialEq, Eq)]
pub enum ImageEvent {
    None,
    Cancel,
    Browse,
    Create,
}

/// The kind of disk first offered: a 1.44 MB floppy.
const FIRST_KIND: &str = "fd_1440kb";

pub struct ImageDialog {
    pub path: TextField,
    /// Whether the path is still the one the dialog made up, whose name
    /// follows the kind of disk.
    made_up: bool,
    /// A disk type of `PRESETS`, or past them a hard disk of `size` MB.
    pub kind: usize,
    pub size: TextField,
    pub label: TextField,
    /// The drive to mount the image on, whether the user picked it, and
    /// the drives free.
    pub mount: Option<u8>,
    mount_picked: bool,
    free: Vec<u8>,
    pub focus: ImageField,
}

impl ImageDialog {
    /// A dialog for an image in `dir`, with `drives` mounted.
    pub fn new(drives: &[DriveInfo], dir: &Path, home: Option<&Path>) -> Self {
        let free = (0..LASTDRIVE).filter(|&d| d != DRIVE_Z && !drives.iter().any(|i| i.drive == d)).collect();
        let kind = PRESETS.iter().position(|p| p.name == FIRST_KIND).unwrap_or(0);
        let mut dialog = Self {
            path: TextField::default(),
            made_up: true,
            kind,
            size: TextField::new("500"),
            label: TextField::default(),
            mount: None,
            mount_picked: false,
            free,
            focus: ImageField::Path,
        };
        dialog.name_in(dir, home);
        dialog.mount = dialog.default_mount();
        dialog
    }

    fn preset(&self) -> Option<&'static makeimg::Preset> {
        PRESETS.get(self.kind)
    }

    pub fn floppy(&self) -> bool {
        self.preset().is_some_and(|p| p.floppy)
    }

    /// What the kind of disk is called.
    pub fn kind_name(&self) -> &'static str {
        self.preset().map_or("Hard disk of any size", |p| p.description)
    }

    /// A made-up file name in `dir` for the kind of disk, one no file has.
    fn name_in(&mut self, dir: &Path, home: Option<&Path>) {
        let stem = if self.floppy() { "floppy" } else { "hdd" };
        let path = (1..)
            .map(|n| dir.join(if n == 1 { format!("{}.img", stem) } else { format!("{}{}.img", stem, n) }))
            .find(|p| !p.exists())
            .unwrap_or_default();
        self.path = TextField::new(&contract_home(&path, home));
        self.made_up = true;
    }

    /// The drives the image can be mounted on: none, or one free, but a
    /// hard disk not on A: and B:.
    fn letters(&self) -> Vec<Option<u8>> {
        let floppy = self.floppy();
        let mut letters = vec![None];
        letters.extend(self.free.iter().filter(|&&d| floppy || d >= FLOPPY_DRIVES).map(|&d| Some(d)));
        letters
    }

    /// A floppy's drive is A: or B:, else the first free from D:, as a
    /// hard disk's.
    fn default_mount(&self) -> Option<u8> {
        let floppies = self.free.iter().copied().find(|&d| self.floppy() && d < FLOPPY_DRIVES);
        floppies.or_else(|| self.free.iter().copied().find(|&d| d >= 3))
    }

    /// The controls that take the focus.
    pub fn fields(&self) -> Vec<ImageField> {
        use ImageField::*;
        let mut fields = vec![Path, Browse, Kind];
        if self.preset().is_none() {
            fields.push(Size);
        }
        fields.extend([Label, Mount, Create, Cancel]);
        fields
    }

    fn move_focus(&mut self, step: isize) {
        let fields = self.fields();
        let at = fields.iter().position(|&f| f == self.focus).unwrap_or(0) as isize;
        self.focus = fields[(at + step).rem_euclid(fields.len() as isize) as usize];
    }

    /// Step the kind of disk or the drive left or right. `dir` is where a
    /// made-up file name goes.
    pub fn step(&mut self, field: ImageField, step: isize, dir: &Path, home: Option<&Path>) {
        match field {
            ImageField::Kind => {
                let was_floppy = self.floppy();
                self.kind = (self.kind as isize + step).rem_euclid(PRESETS.len() as isize + 1) as usize;
                if self.floppy() != was_floppy {
                    if self.made_up {
                        self.name_in(dir, home);
                    }
                    if !self.mount_picked || !self.letters().contains(&self.mount) {
                        self.mount = self.default_mount();
                    }
                }
            }
            ImageField::Mount => {
                let letters = self.letters();
                let at = letters.iter().position(|&d| d == self.mount).unwrap_or(0) as isize;
                self.mount = letters[(at + step).rem_euclid(letters.len() as isize) as usize];
                self.mount_picked = true;
            }
            _ => {}
        }
    }

    /// Where a made-up file name goes: the path's directory.
    pub fn dir(&self, cwd: &Path, home: Option<&Path>) -> PathBuf {
        let path = expand_host_path(self.path.text().trim(), cwd, home);
        path.parent().map_or(cwd.to_path_buf(), Path::to_path_buf)
    }

    pub fn key(&mut self, key: UiKey, cwd: &Path, home: Option<&Path>) -> ImageEvent {
        let text = match self.focus {
            ImageField::Path => Some(&mut self.path),
            ImageField::Size => Some(&mut self.size),
            ImageField::Label => Some(&mut self.label),
            _ => None,
        };
        if let Some(field) = text
            && field.key(key)
        {
            if self.focus == ImageField::Path {
                self.made_up = false;
            }
            return ImageEvent::None;
        }
        let is_button = matches!(self.focus, ImageField::Browse | ImageField::Create | ImageField::Cancel);
        let dir = self.dir(cwd, home);
        match key {
            UiKey::Esc => return ImageEvent::Cancel,
            UiKey::Enter => {
                return match self.focus {
                    ImageField::Browse => ImageEvent::Browse,
                    ImageField::Cancel => ImageEvent::Cancel,
                    _ => ImageEvent::Create,
                };
            }
            UiKey::Tab | UiKey::Down => self.move_focus(1),
            UiKey::BackTab | UiKey::Up => self.move_focus(-1),
            UiKey::Left if is_button => self.move_focus(-1),
            UiKey::Right if is_button => self.move_focus(1),
            UiKey::Left => self.step(self.focus, -1, &dir, home),
            UiKey::Right | UiKey::Char(' ') => self.step(self.focus, 1, &dir, home),
            _ => {}
        }
        ImageEvent::None
    }

    /// Take a directory or file picked in the browser: the image goes in
    /// the directory, or is the file.
    pub fn picked(&mut self, path: &Path, home: Option<&Path>) {
        if path.is_dir() {
            if self.made_up {
                self.name_in(path, home);
            } else {
                let name = PathBuf::from(self.path.text().trim());
                let name = name.file_name().map_or_else(|| "disk.img".into(), |n| n.to_os_string());
                self.path = TextField::new(&contract_home(&path.join(name), home));
            }
        } else {
            self.path = TextField::new(&contract_home(path, home));
            self.made_up = false;
        }
        self.focus = ImageField::Create;
    }

    /// The image the dialog asks for.
    pub fn spec(&self) -> Result<ImageSpec, String> {
        let label = self.label.text().trim().to_string();
        let mut spec = ImageSpec { preset: self.preset(), label: (!label.is_empty()).then_some(label), ..Default::default() };
        if spec.preset.is_none() {
            let size = self.size.text();
            match size.trim().parse::<u64>() {
                Ok(mb) if mb > 0 => spec.size_mb = Some(mb),
                _ => return Err(format!("The size is in MB: '{}' isn't one", size.trim())),
            }
        }
        Ok(spec)
    }

    /// The image laid out, if the dialog asks for one that can be made.
    pub fn plan(&self) -> Result<Plan, String> {
        makeimg::plan(&self.spec()?)
    }
}

impl ConfigUi {
    pub(super) fn open_image_dialog(&mut self) {
        let cwd = std::env::current_dir().unwrap_or_default();
        self.status = None;
        self.image_dialog = Some(ImageDialog::new(&self.drives, &cwd, self.home.as_deref()));
    }

    pub(super) fn image_dialog_key(&mut self, key: UiKey, host: &mut dyn Host) {
        let Some(dialog) = &mut self.image_dialog else { return };
        let cwd = std::env::current_dir().unwrap_or_default();
        match dialog.key(key, &cwd, self.home.as_deref()) {
            ImageEvent::None => {}
            ImageEvent::Cancel => {
                self.image_dialog = None;
                self.status = None;
            }
            ImageEvent::Browse => self.open_browser(super::Pick::ImagePath),
            ImageEvent::Create => self.create_image(host),
        }
    }

    /// Make the image the dialog asks for, and mount it where it asks.
    fn create_image(&mut self, host: &mut dyn Host) {
        let Some(dialog) = &self.image_dialog else { return };
        let home = self.home.clone();
        let cwd = std::env::current_dir().unwrap_or_default();
        let raw = dialog.path.text();
        if raw.trim().is_empty() {
            return self.error("Where does the image go? Enter a file");
        }
        let path = expand_host_path(raw.trim(), &cwd, home.as_deref());
        let shown = contract_home(&path, home.as_deref());
        if path.exists() {
            return self.error(format!("{} already exists: pick another name", shown));
        }
        let plan = match dialog.plan() {
            Ok(plan) => plan,
            Err(e) => return self.error(e),
        };
        let fat32 = plan.volume.as_ref().is_some_and(|v| v.bits == 32);
        if fat32 && dialog.mount.is_some() {
            return self.error("A disk of 2 GB or more is FAT32, which Rust-DOS can't mount: don't mount it");
        }
        if let Err(e) = makeimg::write(&path, &plan, false) {
            return self.error(e);
        }
        let (mount, floppy) = (dialog.mount, dialog.floppy());
        self.image_dialog = None;
        let Some(drive) = mount else {
            return self.info(format!("{} has been made", shown));
        };
        let kind = if floppy { DriveKind::Floppy } else { DriveKind::HardDisk };
        let spec = MountSpec { drive, path, opts: MountOptions { kind, ..Default::default() } };
        match host.mount(spec, false) {
            Ok(_) => {
                self.refresh_drives(host, Some(drive));
                self.info(format!("{} has been made and mounted as {}:", shown, drive_letter(drive)));
            }
            Err(e) => self.error(format!("{} has been made, but can't be mounted: {}", shown, e)),
        }
    }

    pub(super) fn draw_image_dialog(&mut self, g: &mut Grid, content: std::ops::Range<usize>) {
        let Some(dialog) = &self.image_dialog else { return };
        let cols = g.cols;
        let (top, bottom) = (content.start, content.end);
        let (label_col, value_col) = (4, 16.min(cols / 3));
        let end = cols - 3;
        g.text(2, top, "Create a disk image", draw::BRIGHT);
        let mut hits = Vec::new();
        let mut row = top + 1;
        let fields = dialog.fields();
        for field in fields.iter().copied().filter(|f| !matches!(f, ImageField::Create | ImageField::Cancel)) {
            if row >= bottom {
                break;
            }
            let focused = dialog.focus == field;
            let label = match field {
                ImageField::Path => "File",
                ImageField::Kind => "Type",
                ImageField::Size => "Size (MB)",
                ImageField::Label => "Label",
                ImageField::Mount => "Mount as",
                _ => "",
            };
            g.text(label_col, row, label, if focused { draw::BRIGHT } else { draw::TEXT });
            let (col, width) = match field {
                ImageField::Path | ImageField::Size | ImageField::Label => {
                    let (text, width) = match field {
                        ImageField::Path => (&dialog.path, end.saturating_sub(value_col)),
                        ImageField::Size => (&dialog.size, 8),
                        _ => (&dialog.label, 12),
                    };
                    g.background(value_col, row, width, draw::FIELD);
                    let (shown, cursor) = text.view(width);
                    g.text_to(value_col, row, &shown, draw::BRIGHT, value_col + width);
                    if focused {
                        g.background(value_col + cursor, row, 1, draw::SELECT);
                    }
                    if field == ImageField::Label {
                        g.text(value_col + width + 2, row, "(empty: none)", draw::DIM);
                    }
                    (value_col, width)
                }
                ImageField::Browse => {
                    let text = "[ Browse... ]";
                    if focused {
                        g.background(value_col, row, text.len(), draw::SELECT);
                    }
                    g.text(value_col, row, text, if focused { draw::BRIGHT } else { draw::KEY });
                    (value_col, text.len())
                }
                _ => {
                    let text = match field {
                        ImageField::Kind => dialog.kind_name().to_string(),
                        _ => dialog.mount.map_or("no".to_string(), |d| format!("{}:", drive_letter(d))),
                    };
                    let width = text.chars().count() + 4;
                    if focused {
                        g.background(value_col, row, width, draw::SELECT);
                    }
                    g.char(value_col, row, 0x11, draw::KEY);
                    g.text(value_col + 2, row, &text, draw::BRIGHT);
                    g.char(value_col + width - 1, row, 0x10, draw::KEY);
                    (value_col, width)
                }
            };
            hits.push(Hit { row, col, width, target: Target::ImageField(field) });
            row += 1;
        }
        // What it will be.
        row += 1;
        if row < bottom {
            let (text, color) = match dialog.plan() {
                Ok(plan) => (plan.describe(), draw::DIM),
                Err(e) => (e, draw::ERROR),
            };
            g.text_to(label_col, row, &fit(&text, end - label_col), color, end);
        }
        row += 2;
        if row < bottom {
            let mut col = value_col;
            for (field, text) in [(ImageField::Create, "[ Create ]"), (ImageField::Cancel, "[ Cancel ]")] {
                let focused = dialog.focus == field;
                if focused {
                    g.background(col, row, text.len(), draw::SELECT);
                }
                g.text(col, row, text, if focused { draw::BRIGHT } else { draw::KEY });
                hits.push(Hit { row, col, width: text.len(), target: Target::ImageField(field) });
                col += text.len() + 2;
            }
        }
        self.hits.extend(hits);
    }

    /// A click on a control of the dialog: it takes the focus, a button
    /// is pressed, and a choice clicked again steps.
    pub(super) fn image_field_clicked(&mut self, field: ImageField, host: &mut dyn Host) {
        let Some(dialog) = &mut self.image_dialog else { return };
        let again = dialog.focus == field;
        dialog.focus = field;
        match field {
            ImageField::Browse | ImageField::Create | ImageField::Cancel => self.key(UiKey::Enter, host),
            ImageField::Kind | ImageField::Mount if again => self.key(UiKey::Right, host),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive(drive: u8) -> DriveInfo {
        DriveInfo {
            drive,
            kind: DriveKind::HardDisk,
            root: Some("/x".into()),
            image: None,
            images: Vec::new(),
            image_index: 0,
            label: String::new(),
            read_only: false,
            current_dir: String::new(),
            mount: None,
        }
    }

    #[test]
    fn the_name_and_the_drive_follow_the_kind_of_disk() {
        let dir = std::env::temp_dir().join(format!("rust-dos-image-dialog-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("floppy.img"), b"").unwrap();
        let drives = [drive(1), drive(2), drive(25)];
        let mut d = ImageDialog::new(&drives, &dir, None);
        assert_eq!((d.kind_name(), d.mount), ("1.44 MB floppy (3\u{00BD}\")", Some(0)));
        assert_eq!(d.path.text(), dir.join("floppy2.img").display().to_string(), "a name no file has");
        assert_eq!(d.fields(), [ImageField::Path, ImageField::Browse, ImageField::Kind, ImageField::Label, ImageField::Mount, ImageField::Create, ImageField::Cancel]);

        // To the hard disks: the name and the drive change with it.
        for _ in 0..2 {
            d.step(ImageField::Kind, 1, &dir, None);
        }
        assert_eq!((d.kind_name(), d.mount), ("20 MB hard disk", Some(3)));
        assert!(d.path.text().ends_with("hdd.img"));
        // A hard disk isn't offered A:.
        d.step(ImageField::Mount, -1, &dir, None);
        assert_eq!(d.mount, None);
        d.step(ImageField::Mount, -1, &dir, None);
        assert_eq!(d.mount, Some(24));

        // Any size: the size is typed.
        d.kind = PRESETS.len();
        assert!(d.fields().contains(&ImageField::Size));
        assert_eq!(d.spec().unwrap().size_mb, Some(500));
        d.size = TextField::new("lots");
        assert!(d.plan().unwrap_err().contains("'lots'"));

        // A name typed stays.
        d.focus = ImageField::Path;
        d.key(UiKey::Char('x'), &dir, None);
        d.step(ImageField::Kind, -1, &dir, None);
        d.step(ImageField::Kind, 1 - PRESETS.len() as isize, &dir, None);
        assert!(d.floppy() && d.path.text().ends_with(".imgx"));
        d.picked(&dir, None);
        assert_eq!((d.path.text(), d.focus), (dir.join("hdd.imgx").display().to_string(), ImageField::Create));
    }
}
