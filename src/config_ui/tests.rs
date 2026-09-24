use super::*;
use crate::disk::MountOptions;

fn drive_info(spec: &MountSpec) -> DriveInfo {
    DriveInfo {
        drive: spec.drive,
        kind: spec.opts.kind,
        root: Some(spec.path.clone()),
        image: None,
        label: "RUSTDOS".to_string(),
        read_only: spec.opts.read_only,
        current_dir: String::new(),
        mount: Some(spec.clone()),
    }
}

/// Records what the window asks for.
struct FakeHost {
    drives: Vec<DriveInfo>,
    applied: Vec<Settings>,
    mounts: Vec<(MountSpec, bool)>,
    unmounts: Vec<u8>,
    saved: Vec<Settings>,
}

impl FakeHost {
    fn new() -> Self {
        let c = MountSpec { drive: 2, path: "/dos".into(), opts: MountOptions::default() };
        let mut z = drive_info(&MountSpec { drive: 25, path: "/".into(), opts: MountOptions::default() });
        z.kind = DriveKind::Virtual;
        z.mount = None;
        Self { drives: vec![drive_info(&c), z], applied: vec![], mounts: vec![], unmounts: vec![], saved: vec![] }
    }
}

impl Host for FakeHost {
    fn apply(&mut self, settings: &Settings) -> Result<Option<String>, String> {
        self.applied.push(settings.clone());
        Ok(None)
    }

    fn mount(&mut self, spec: MountSpec, replace: bool) -> Result<PathBuf, String> {
        self.drives.retain(|d| d.drive != spec.drive);
        self.drives.push(drive_info(&spec));
        self.drives.sort_by_key(|d| d.drive);
        self.mounts.push((spec.clone(), replace));
        Ok(spec.path)
    }

    fn unmount(&mut self, drive: u8) -> Result<(), String> {
        self.drives.retain(|d| d.drive != drive);
        self.unmounts.push(drive);
        Ok(())
    }

    fn drives(&self) -> Vec<DriveInfo> {
        self.drives.clone()
    }

    fn save(&mut self, settings: &Settings) -> Result<(), String> {
        self.saved.push(settings.clone());
        Ok(())
    }
}

fn opened(host: &FakeHost) -> ConfigUi {
    let mut ui = ConfigUi::new();
    ui.open(&Settings::default(), Some("/cfg/rust-dos.conf".into()), host);
    ui
}

fn keys(ui: &mut ConfigUi, host: &mut FakeHost, keys: &[UiKey]) {
    for &key in keys {
        ui.key(key, host);
    }
}

fn status(ui: &ConfigUi) -> (&str, bool) {
    ui.status.as_ref().map_or(("", false), |s| (s.text.as_str(), s.error))
}

#[test]
fn settings_change_live() {
    let mut host = FakeHost::new();
    let mut ui = opened(&host);
    use UiKey::*;
    // Display: scale up twice, fullscreen on.
    keys(&mut ui, &mut host, &[Tab, Right, Right, Down, Enter]);
    let last = host.applied.last().unwrap();
    assert_eq!((last.scale, last.fullscreen), (3, true));
    assert_eq!(host.applied.len(), 3);

    // Emulator: type a speed, then a bad one.
    keys(&mut ui, &mut host, &[Tab, Enter, End]);
    for _ in 0..3 {
        ui.key(Backspace, &mut host);
    }
    ui.text("3000", &mut host);
    ui.key(Enter, &mut host);
    assert_eq!(host.applied.last().unwrap().cycles, CpuSpeed::Fixed(3000));
    assert!(ui.edit.is_none());
    keys(&mut ui, &mut host, &[Enter, Home]);
    ui.text("x", &mut host);
    ui.key(Enter, &mut host);
    assert!(status(&ui).1, "{:?}", status(&ui));
    assert!(ui.edit.is_some());
    ui.key(Esc, &mut host);

    // Memory waits for the next start.
    keys(&mut ui, &mut host, &[End, Right]);
    assert_eq!(host.applied.last().unwrap().memsize, 32);
    assert!(status(&ui).0.contains("next time"), "{:?}", status(&ui));
}

#[test]
fn sound_conflicts_are_reported() {
    let mut host = FakeHost::new();
    let mut ui = opened(&host);
    use UiKey::*;
    ui.show_page(Page::Sound);
    // The Ultrasound's base port onto the Sound Blaster's 220h.
    ui.row = Page::Sound.items().iter().position(|&i| i == Item::GusBase).unwrap();
    keys(&mut ui, &mut host, &[Left]);
    assert_eq!(host.applied.last().unwrap().sound.gus.base, 0x220);
    assert!(status(&ui).1 && status(&ui).0.contains("gusbase 220"), "{:?}", status(&ui));

    // The Ultrasound's drive skips the letters that are taken.
    ui.row = Page::Sound.items().iter().position(|&i| i == Item::GusDrive).unwrap();
    ui.settings.sound.gus.drive = None;
    keys(&mut ui, &mut host, &[Right]);
    assert_eq!(ui.settings.sound.gus.drive, Some(3));
}

#[test]
fn drives_mount_and_unmount() {
    let dir = std::path::absolute("target/test_config_ui/mount").unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    let mut host = FakeHost::new();
    let mut ui = opened(&host);
    use UiKey::*;

    // Ins opens the dialog on D:, the path is typed and mounted.
    ui.key(Insert, &mut host);
    assert_eq!(ui.dialog.as_ref().unwrap().drive, 3);
    ui.text(&dir.display().to_string(), &mut host);
    ui.key(Enter, &mut host);
    let (spec, replace) = host.mounts.last().unwrap().clone();
    assert_eq!((spec.drive, spec.path.as_path(), replace), (3, dir.as_path(), false));
    assert!(ui.dialog.is_none());
    assert_eq!(ui.drives.len(), 3);
    assert_eq!(ui.row, 1, "the new drive is selected");
    assert!(status(&ui).0.starts_with("Drive D: is mounted"), "{:?}", status(&ui));

    // Enter changes it, keeping the path; Mount replaces it.
    keys(&mut ui, &mut host, &[Enter, Enter]);
    assert!(host.mounts.last().unwrap().1);

    // C: and Z: stay.
    keys(&mut ui, &mut host, &[Home, Delete]);
    assert!(status(&ui).1);
    keys(&mut ui, &mut host, &[Down, Delete]);
    assert_eq!(host.unmounts, [3]);
    assert_eq!(ui.drives.len(), 2);

    // An empty path is refused.
    keys(&mut ui, &mut host, &[End, Enter, Enter]);
    assert!(status(&ui).1);
    assert!(ui.dialog.is_some());
    ui.key(Esc, &mut host);
    assert!(ui.dialog.is_none());
}

#[test]
fn save_and_close() {
    let mut host = FakeHost::new();
    let mut ui = opened(&host);
    ui.key(UiKey::Save, &mut host);
    assert_eq!(host.saved.len(), 1);
    assert_eq!(status(&ui), ("Saved to /cfg/rust-dos.conf", false));

    ui.open(&Settings::default(), None, &host);
    ui.key(UiKey::Save, &mut host);
    assert_eq!(host.saved.len(), 1);
    assert!(status(&ui).1);

    ui.key(UiKey::Esc, &mut host);
    assert!(!ui.is_open());
}

#[test]
fn every_page_draws_and_clicks() {
    let mut host = FakeHost::new();
    let mut ui = opened(&host);
    for (width, height) in [(640, 400), (640, 350), (400, 300), (1024, 768)] {
        let mut frame = Frame::new(width, height);
        for page in PAGES {
            ui.show_page(page);
            ui.draw(&mut frame);
        }
        ui.show_page(Page::Drives);
        ui.key(UiKey::Insert, &mut host);
        ui.draw(&mut frame);
        ui.key(UiKey::Esc, &mut host);
    }

    // Clicking the Sound tab, then the ► of its first setting.
    let mut frame = Frame::new(640, 400);
    ui.show_page(Page::Drives);
    ui.draw(&mut frame);
    let layout = ui.layout.unwrap();
    let at = |col: usize, row: usize| ((layout.x + col * 8 + 4) as i32, (layout.y + row * layout.cell_h + 4) as i32);
    let tab = ui.hits.iter().find(|h| matches!(h.target, Target::Tab(Page::Sound))).unwrap();
    let (x, y) = at(tab.col, tab.row);
    ui.click(x, y, &mut host);
    assert_eq!(ui.page, Page::Sound);
    ui.draw(&mut frame);
    let step = ui.hits.iter().find(|h| matches!(h.target, Target::Step(0, 1))).unwrap();
    let (x, y) = at(step.col, step.row);
    ui.click(x, y, &mut host);
    assert_eq!(ui.settings.sound.sb.model, SbModel::SbPro2);
}

#[test]
fn long_text_is_shortened_in_the_middle() {
    assert_eq!(fit("/home/user/dos/games", 30), "/home/user/dos/games");
    assert_eq!(fit("/home/user/dos/games/doom.cue", 14), "/ho...doom.cue");
    assert_eq!(fit("abcdef", 2), "ab");
}
