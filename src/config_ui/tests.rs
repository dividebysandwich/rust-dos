use super::*;
use crate::disk::MountOptions;
use crate::games::{GameEntry, NewGame};
use crate::video::shader::Shader;

fn drive_info(spec: &MountSpec) -> DriveInfo {
    DriveInfo {
        drive: spec.drive,
        kind: spec.opts.kind,
        root: (!spec.path.is_file()).then(|| spec.path.clone()),
        image: spec.path.is_file().then(|| spec.path.clone()),
        label: "RUSTDOS".to_string(),
        read_only: spec.opts.read_only,
        current_dir: String::new(),
        mount: Some(spec.clone()),
        images: Vec::new(),
        image_index: 0,
    }
}

/// Records what the window asks for.
struct FakeHost {
    drives: Vec<DriveInfo>,
    applied: Vec<Settings>,
    mounts: Vec<(MountSpec, bool)>,
    unmounts: Vec<u8>,
    /// The drives booted from.
    booted: Vec<u8>,
    saved: Vec<Settings>,
    /// The drives `choose_image` was asked for.
    images: Vec<Option<u8>>,
    /// Whether CRT shaders are refused, as without OpenGL 3.
    no_shaders: bool,
    /// The game profiles, and the games launched and made.
    games: Vec<GameEntry>,
    launched: Vec<String>,
    created: Vec<NewGame>,
    /// The machine's memory for the Cheats page, and its frozen values.
    ram: Vec<u8>,
    frozen: Vec<crate::cheats::Freeze>,
    /// The save states in the slots, and the slot the hotkeys use.
    states: Vec<SlotView>,
    slot: u8,
    loaded: Vec<u8>,
    /// The configuration file's `[autoexec]`.
    autoexec: Vec<String>,
    /// The LAN as `lan` shows it, the rooms asked for and joined, and how
    /// often the room was left.
    lan: Option<crate::net::LanView>,
    browsed: Vec<(Option<String>, String)>,
    joined: Vec<(Option<String>, String, String)>,
    left: usize,
    disbanded: usize,
    /// The rooms made on this network, and their passwords.
    hosted: Vec<(String, String)>,
}

impl FakeHost {
    fn new() -> Self {
        let c = MountSpec { drive: 2, path: "/dos".into(), opts: MountOptions::default() };
        let mut z = drive_info(&MountSpec { drive: 25, path: "/".into(), opts: MountOptions::default() });
        z.kind = DriveKind::Virtual;
        z.mount = None;
        Self {
            drives: vec![drive_info(&c), z],
            applied: vec![],
            mounts: vec![],
            unmounts: vec![],
            booted: vec![],
            saved: vec![],
            images: vec![],
            no_shaders: false,
            games: vec![],
            launched: vec![],
            created: vec![],
            ram: vec![],
            frozen: vec![],
            states: vec![],
            slot: 1,
            loaded: vec![],
            autoexec: vec![],
            lan: None,
            browsed: vec![],
            joined: vec![],
            left: 0,
            disbanded: 0,
            hosted: vec![],
        }
    }
}

impl Host for FakeHost {
    fn apply(&mut self, settings: &Settings) -> Result<Option<String>, String> {
        self.applied.push(settings.clone());
        if self.no_shaders && settings.shader != Shader::None {
            return Err("CRT shaders need OpenGL 3".to_string());
        }
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

    fn boot(&mut self, drive: u8) -> Result<String, String> {
        self.booted.push(drive);
        Ok(format!("Booting from drive {}:", drive_letter(drive)))
    }

    fn set_boot(&mut self, drive: u8, boot: bool) -> Result<(), String> {
        for info in &mut self.drives {
            if let Some(spec) = &mut info.mount {
                spec.opts.boot = if info.drive == drive { boot } else { spec.opts.boot && !boot };
            }
        }
        Ok(())
    }

    fn save(&mut self, settings: &Settings) -> Result<(), String> {
        self.saved.push(settings.clone());
        Ok(())
    }

    fn choose_image(&mut self, drive: Option<u8>) -> Result<(), String> {
        if drive == Some(2) {
            return Err("C: stays".to_string());
        }
        self.images.push(drive);
        Ok(())
    }

    fn games(&self) -> Vec<GameEntry> {
        self.games.clone()
    }

    fn active_game(&self) -> Option<String> {
        self.launched.last().cloned()
    }

    fn launch_game(&mut self, id: &str) -> Result<String, String> {
        self.launched.push(id.to_string());
        Ok(format!("Starting {}", id))
    }

    fn create_game(&mut self, game: &NewGame, _settings: &Settings) -> Result<String, String> {
        if game.command.is_empty() {
            return Err("The game needs a command".to_string());
        }
        self.created.push(game.clone());
        let id = crate::games::slug(&game.name, &[]);
        self.games.push(GameEntry { id: id.clone(), name: game.name.clone(), command: game.command.clone() });
        Ok(id)
    }

    fn delete_game(&mut self, id: &str) -> Result<(), String> {
        self.games.retain(|g| g.id != id);
        Ok(())
    }

    fn current_directory(&self) -> String {
        "C:\\GAMES".to_string()
    }

    fn memory(&self) -> &[u8] {
        &self.ram
    }

    fn poke(&mut self, addr: usize, bytes: &[u8]) {
        self.ram[addr..addr + bytes.len()].copy_from_slice(bytes);
    }

    fn freezes(&self) -> Vec<crate::cheats::Freeze> {
        self.frozen.clone()
    }

    fn set_freezes(&mut self, freezes: Vec<crate::cheats::Freeze>) {
        self.frozen = freezes;
    }

    fn states_available(&self) -> bool {
        true
    }

    fn states(&self) -> Vec<SlotView> {
        self.states.clone()
    }

    fn current_slot(&self) -> u8 {
        self.slot
    }

    fn save_state(&mut self, slot: u8) -> Result<String, String> {
        self.states.retain(|s| s.slot != slot);
        let header = crate::savestate::slots::Header { saved: "2026-09-26 12:00:00".into(), program: "KEEN4E.EXE".into(), ..Default::default() };
        self.states.push(SlotView { slot, header: Some(header), picture: Some(Frame::new(160, 100)) });
        self.slot = slot;
        Ok(format!("Saved to slot {}", slot))
    }

    fn load_state(&mut self, slot: u8) -> Result<String, String> {
        self.loaded.push(slot);
        self.slot = slot;
        Ok(format!("Loaded slot {}", slot))
    }

    fn delete_state(&mut self, slot: u8) -> Result<(), String> {
        self.states.retain(|s| s.slot != slot);
        Ok(())
    }

    fn autoexec(&self) -> Result<Vec<String>, String> {
        Ok(self.autoexec.clone())
    }

    fn save_autoexec(&mut self, lines: &[String]) -> Result<(), String> {
        self.autoexec = lines.to_vec();
        Ok(())
    }

    fn lan(&self) -> Option<crate::net::LanView> {
        self.lan.clone()
    }

    fn browse_rooms(&mut self, relay: Option<&str>, filter: &str) -> Result<(), String> {
        self.browsed.push((relay.map(String::from), filter.to_string()));
        if let Some(lan) = &mut self.lan {
            lan.listing.asking = true;
        }
        Ok(())
    }

    fn join_room(&mut self, relay: Option<&str>, room: &str, password: &str) -> Result<(), String> {
        self.joined.push((relay.map(String::from), room.to_string(), password.to_string()));
        Ok(())
    }

    fn leave_room(&mut self) {
        self.left += 1;
    }

    fn disband_room(&mut self) {
        self.disbanded += 1;
    }

    fn host_room(&mut self, room: &str, password: &str) -> Result<(), String> {
        self.hosted.push((room.to_string(), password.to_string()));
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

/// The values the list Enter opens shows, closed again.
fn listed(ui: &mut ConfigUi, host: &mut FakeHost) -> Vec<String> {
    ui.key(UiKey::Enter, host);
    let popup = ui.popup.as_ref().expect("Enter lists the values");
    let values = popup.choices.iter().map(|s| popup.item.value(s, None)).collect();
    ui.key(UiKey::Esc, host);
    values
}

/// Pick `value`, as the window shows it, from the list Enter opens.
fn pick(ui: &mut ConfigUi, host: &mut FakeHost, value: &str) {
    let values = listed(ui, host);
    let at = values.iter().position(|v| v == value).unwrap_or_else(|| panic!("{:?} isn't in {:?}", value, values));
    ui.key(UiKey::Enter, host);
    ui.key(UiKey::Home, host);
    for _ in 0..at {
        ui.key(UiKey::Down, host);
    }
    ui.key(UiKey::Enter, host);
}

fn status(ui: &ConfigUi) -> (&str, bool) {
    ui.status.as_ref().map_or(("", false), |s| (s.text.as_str(), s.error))
}

#[test]
fn settings_change_live() {
    let mut host = FakeHost::new();
    let mut ui = opened(&host);
    use UiKey::*;
    // Display: scale up twice with Right, and fullscreen on from its list.
    keys(&mut ui, &mut host, &[Tab, Right, Right, Down]);
    pick(&mut ui, &mut host, "on");
    let last = host.applied.last().unwrap();
    assert_eq!((last.scale, last.fullscreen), (3, true));
    assert_eq!(host.applied.len(), 3);

    // Emulator: type a speed, then a bad one.
    keys(&mut ui, &mut host, &[Tab, Enter, End]);
    for _ in 0.."auto".len() {
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
    // Left and Right step through some speeds.
    keys(&mut ui, &mut host, &[Right]);
    assert_eq!(host.applied.last().unwrap().cycles, CpuSpeed::Fixed(5000));
    keys(&mut ui, &mut host, &[Left, Left, Left]);
    assert_eq!(host.applied.last().unwrap().cycles, CpuSpeed::Fixed(1000));

    // The CPU core changes at once.
    ui.row = Page::Emulator.items().iter().position(|&i| i == Item::Core).unwrap();
    pick(&mut ui, &mut host, "dynamic recompiler");
    assert_eq!(host.applied.last().unwrap().core, crate::cpu::CoreMode::Dynamic);
    assert!(!status(&ui).0.contains("prompt"), "{:?}", status(&ui));

    // Memory slides in fours of MB and waits for the next start.
    ui.row = ui.items().iter().position(|&i| i == Item::Memsize).unwrap();
    assert_eq!(ui.item().map(|i| i.value(&ui.settings, None)).as_deref(), Some("16 MB ■■■■············"));
    keys(&mut ui, &mut host, &[Right]);
    assert_eq!(host.applied.last().unwrap().memsize, 20);
    assert!(status(&ui).0.contains("next time"), "{:?}", status(&ui));
    // Typed, no further than 2 and 64 MB, and back to 16 with Delete.
    keys(&mut ui, &mut host, &[Enter, End, Backspace, Backspace]);
    ui.text("1", &mut host);
    ui.key(Enter, &mut host);
    assert_eq!(status(&ui), ("invalid memsize '1' (2 to 64 MB)", true));
    keys(&mut ui, &mut host, &[End, Backspace]);
    ui.text("62", &mut host);
    keys(&mut ui, &mut host, &[Enter, Right, Right]);
    assert_eq!(host.applied.last().unwrap().memsize, 64);
    for _ in 0..15 {
        ui.key(Left, &mut host);
    }
    assert_eq!(host.applied.last().unwrap().memsize, 4);
    keys(&mut ui, &mut host, &[Left, Left]);
    assert_eq!(host.applied.last().unwrap().memsize, 2);
    assert_eq!(ui.item().map(|i| i.value(&ui.settings, None)).as_deref(), Some(" 2 MB ················"));
    keys(&mut ui, &mut host, &[Delete]);
    assert_eq!(host.applied.last().unwrap().memsize, 16);

    // The disk speed changes at once.
    ui.row = ui.items().iter().position(|&i| i == Item::FloppyDiskSpeed).unwrap();
    pick(&mut ui, &mut host, "slow (~30 kB/s)");
    assert_eq!(host.applied.last().unwrap().disk.floppy_disk_speed, crate::diskio::DiskSpeed::Slow);
    assert!(status(&ui).0.is_empty() || !status(&ui).0.contains("next time"), "{:?}", status(&ui));
    assert_eq!(ui.item().map(|i| i.value(&ui.settings, None)).as_deref(), Some("slow (~30 kB/s)"));
}

#[test]
fn sound_conflicts_are_reported() {
    let mut host = FakeHost::new();
    let mut ui = opened(&host);
    ui.show_page(Page::Sound);
    // The Ultrasound's base port onto the Sound Blaster's 220h.
    ui.row = ui.items().iter().position(|&i| i == Item::GusPorts).unwrap();
    pick(&mut ui, &mut host, "220h");
    assert_eq!(host.applied.last().unwrap().sound.gus.base, 0x220);
    assert!(status(&ui).1 && status(&ui).0.contains("gusbase 220"), "{:?}", status(&ui));

    // The Ultrasound's drive skips the letters that are taken.
    ui.row = ui.items().iter().position(|&i| i == Item::GusDrive).unwrap();
    ui.settings.sound.gus.drive = None;
    assert_eq!(listed(&mut ui, &mut host)[..3], ["none", "D:", "E:"]);
    pick(&mut ui, &mut host, "D:");
    assert_eq!(ui.settings.sound.gus.drive, Some(3));
}

#[test]
fn a_cards_port_irq_and_dma_share_a_row() {
    let mut host = FakeHost::new();
    let mut ui = opened(&host);
    use UiKey::*;
    ui.show_page(Page::Sound);
    ui.row = ui.items().iter().position(|&i| i == Item::SbPorts).unwrap();
    let value = |ui: &ConfigUi| ui.item().map(|i| i.value(&ui.settings, None)).unwrap();
    assert_eq!(value(&ui), "220h, IRQ 7, DMA 1, HDMA 5");

    // Left and Right go to the field beside, no further than the ends,
    // changing none, and Enter lists its values.
    pick(&mut ui, &mut host, "230h");
    assert_eq!(host.applied.last().unwrap().sound.sb.base, 0x230);
    let applied = host.applied.len();
    keys(&mut ui, &mut host, &[Left, Right]);
    assert_eq!((ui.field, host.applied.len()), (1, applied));
    pick(&mut ui, &mut host, "9");
    assert_eq!(host.applied.last().unwrap().sound.sb.irq, 9);
    // In the list, Left and Right go on to the list of the field beside.
    keys(&mut ui, &mut host, &[Enter, Right, Right, Down, Enter]);
    assert_eq!((ui.field, host.applied.last().unwrap().sound.sb.dma16), (3, 6));
    let applied = host.applied.len();
    keys(&mut ui, &mut host, &[Right, Enter, Right, Esc]);
    assert_eq!((ui.field, host.applied.len()), (3, applied));

    // The high DMA channel is the SB16's alone.
    ui.settings.sound.sb.model = SbModel::SbPro2;
    assert_eq!(value(&ui), "230h, IRQ 9, DMA 1");

    // A click on a field's button lists its values, and a click picks one.
    let mut frame = Frame::new(640, 400);
    let mut hit = |ui: &mut ConfigUi, target: fn(&Target) -> bool| {
        ui.draw(&mut frame);
        let h = ui.hits.iter().find(|h| target(&h.target)).unwrap();
        let layout = Layout::for_frame(640, 400);
        ((layout.x + h.col * 8 + 4) as i32, (layout.y + h.row * layout.cell_h + 4) as i32)
    };
    let (x, y) = hit(&mut ui, |t| matches!(t, Target::RowField(_, 2)));
    ui.click(x, y, &mut host);
    assert_eq!((ui.field, ui.popup.as_ref().map(|p| p.item)), (2, Some(Item::SbDma)));
    let (x, y) = hit(&mut ui, |t| matches!(t, Target::PopupRow(2)));
    ui.click(x, y, &mut host);
    assert_eq!(host.applied.last().unwrap().sound.sb.dma8, 3);
    let (x, y) = hit(&mut ui, |t| matches!(t, Target::RowField(_, 1)));
    ui.click(x, y, &mut host);
    let (x, y) = hit(&mut ui, |t| matches!(t, Target::PopupRow(2)));
    ui.click(x, y, &mut host);
    assert!(ui.popup.is_none());
    assert_eq!((ui.field, host.applied.last().unwrap().sound.sb.irq), (1, 5));

    // Another row starts at its first field.
    keys(&mut ui, &mut host, &[Down, Up]);
    assert_eq!(ui.field, 0);
}

#[test]
fn enter_lists_a_settings_values_to_pick_from() {
    let mut host = FakeHost::new();
    let mut ui = opened(&host);
    use UiKey::*;
    ui.show_page(Page::Sound);
    ui.row = ui.items().iter().position(|&i| i == Item::Midi).unwrap();

    // Enter lists the synthesizers, the one set selected; moving about and
    // Esc leave it as it was.
    ui.key(Enter, &mut host);
    let popup = ui.popup.as_ref().unwrap();
    assert_eq!((popup.item, popup.selected), (Item::Midi, 0));
    keys(&mut ui, &mut host, &[Down, Esc]);
    assert!(ui.popup.is_none() && host.applied.is_empty());

    // Enter picks the one selected; a letter selects the next that starts
    // with it.
    keys(&mut ui, &mut host, &[Enter, End, Enter]);
    assert_eq!(host.applied.last().unwrap().sound.midisynth, MidiSynth::None);
    ui.key(Enter, &mut host);
    ui.text("u", &mut host);
    ui.key(Enter, &mut host);
    assert_eq!(host.applied.last().unwrap().sound.midisynth, MidiSynth::Gus);
    // Picking the value it has changes nothing.
    keys(&mut ui, &mut host, &[Enter, Enter]);
    assert_eq!(host.applied.len(), 2);
    // The CPU speed is still typed.
    ui.show_page(Page::Emulator);
    ui.row = ui.items().iter().position(|&i| i == Item::Cycles).unwrap();
    ui.key(Enter, &mut host);
    assert!(ui.popup.is_none() && ui.edit.is_some());
    ui.key(Esc, &mut host);

    // A list longer than the page has room for scrolls, in the page's rows.
    let layouts = crate::keylayout::LayoutSetting::all();
    ui.row = ui.items().iter().position(|&i| i == Item::KeyboardLayout).unwrap();
    ui.key(Enter, &mut host);
    let mut frame = Frame::new(640, 350);
    ui.draw(&mut frame);
    let layout = ui.layout.unwrap();
    let at = |col: usize, row: usize| ((layout.x + col * 8 + 4) as i32, (layout.y + row * layout.cell_h + 4) as i32);
    let shown = |ui: &ConfigUi| -> Vec<(usize, usize, usize)> {
        ui.hits
            .iter()
            .filter_map(|h| match h.target {
                Target::PopupRow(i) => Some((i, h.col, h.row)),
                _ => None,
            })
            .collect()
    };
    let rows = shown(&ui);
    assert!(rows.len() < layouts.len() && rows.len() == ui.popup.as_ref().unwrap().visible);
    assert!(rows.iter().all(|&(_, _, row)| (4..layout.rows - 5).contains(&row)), "{:?}", rows);
    ui.key(End, &mut host);
    ui.draw(&mut frame);
    assert_eq!(shown(&ui).last().unwrap().0, layouts.len() - 1);
    // The wheel moves the selection, and a click picks a value.
    ui.wheel(2, &mut host);
    assert_eq!(ui.popup.as_ref().unwrap().selected, layouts.len() - 3);
    ui.draw(&mut frame);
    let (i, col, row) = shown(&ui)[0];
    let (x, y) = at(col + 1, row);
    ui.click(x, y, &mut host);
    assert!(ui.popup.is_none());
    assert_eq!(host.applied.last().unwrap().keyboard_layout, layouts[i]);

    // A click off the list closes it, and does nothing else.
    ui.key(Enter, &mut host);
    ui.draw(&mut frame);
    let applied = host.applied.len();
    let tab = ui.hits.iter().find(|h| matches!(h.target, Target::Tab(Page::Sound))).unwrap();
    let (x, y) = at(tab.col, tab.row);
    ui.click(x, y, &mut host);
    assert!(ui.popup.is_none());
    assert_eq!((ui.page, host.applied.len()), (Page::Emulator, applied));
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

    // An empty path is refused. ("+ Mount a drive..." is above "+ Create
    // a disk image...".)
    keys(&mut ui, &mut host, &[End, Up, Enter, Enter]);
    assert!(status(&ui).1);
    assert!(ui.dialog.is_some());
    ui.key(Esc, &mut host);
    assert!(ui.dialog.is_none());
}

#[test]
fn drives_boot_now_or_at_startup() {
    let dir = std::path::absolute("target/test_config_ui/boot").unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    let image = dir.join("win95.img");
    std::fs::write(&image, [0u8; 512]).unwrap();
    let mut host = FakeHost::new();
    host.mount(MountSpec { drive: 3, path: image.clone(), opts: MountOptions::default() }, false).unwrap();
    let mut ui = opened(&host);
    use UiKey::*;

    // C: is a directory, which doesn't boot.
    ui.key(Char('b'), &mut host);
    assert!(status(&ui).1 && host.booted.is_empty());

    // The dialog's Auto-boot only marks D:, which stays mounted as it is.
    keys(&mut ui, &mut host, &[Down, Enter]);
    ui.dialog.as_mut().unwrap().focus = Field::BootFlag;
    keys(&mut ui, &mut host, &[Right, Enter]);
    assert!(ui.dialog.is_none());
    assert_eq!(host.mounts.len(), 1);
    assert!(host.drives[1].mount.as_ref().unwrap().opts.boot);
    assert!(ui.drives[1].mount.as_ref().unwrap().opts.boot);
    assert!(status(&ui).0.contains("boots when Rust-DOS starts"), "{:?}", status(&ui));

    // B boots it now, and the window closes on the booted system.
    ui.key(Char('b'), &mut host);
    assert_eq!(host.booted, [3]);
    assert!(!ui.is_open());
    assert_eq!(ui.take_notice().as_deref(), Some("Booting from drive D:"));

    // So does the dialog's Boot.
    let mut ui = opened(&host);
    keys(&mut ui, &mut host, &[Down, Enter]);
    ui.dialog.as_mut().unwrap().focus = Field::Boot;
    ui.key(Enter, &mut host);
    assert_eq!(host.booted, [3, 3]);
    assert!(!ui.is_open());
    assert_eq!(host.mounts.len(), 1, "D: stays mounted as it is");
}

#[test]
fn save_and_close() {
    let mut host = FakeHost::new();
    let mut ui = opened(&host);
    ui.key(UiKey::Save, &mut host);
    assert_eq!(host.saved.len(), 1);
    assert_eq!(status(&ui), ("Saved to /cfg/rust-dos.conf", false));

    // What changed on every page is saved, whichever page F2 is pressed on.
    use UiKey::*;
    ui.show_page(Page::Display);
    pick(&mut ui, &mut host, "2x");
    ui.show_page(Page::Sound);
    pick(&mut ui, &mut host, "SB Pro 2");
    ui.show_page(Page::Games);
    ui.key(Save, &mut host);
    let saved = host.saved.last().unwrap();
    assert_eq!((saved.scale, saved.sound.sb.model), (2, SbModel::SbPro2));

    ui.open(&Settings::default(), None, &host);
    ui.key(UiKey::Save, &mut host);
    assert_eq!(host.saved.len(), 2);
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
            // And every setting picked from values lists them.
            for row in 0..ui.items().len() {
                if ui.items()[row].input() == Input::Choice {
                    ui.row = row;
                    ui.key(UiKey::Enter, &mut host);
                    assert!(ui.popup.is_some(), "{:?}", ui.items()[row]);
                    ui.draw(&mut frame);
                    ui.key(UiKey::Esc, &mut host);
                }
            }
        }
        ui.show_page(Page::Drives);
        ui.key(UiKey::Insert, &mut host);
        ui.draw(&mut frame);
        ui.key(UiKey::Esc, &mut host);
    }

    // Clicking the Sound tab, then the button of its first setting and a
    // value in its list.
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
    let button = ui.hits.iter().find(|h| matches!(h.target, Target::Button(0))).unwrap();
    let (x, y) = at(button.col, button.row);
    ui.click(x, y, &mut host);
    assert_eq!(ui.popup.as_ref().map(|p| p.item), Some(Item::SbType));
    ui.draw(&mut frame);
    let value = ui.hits.iter().find(|h| matches!(h.target, Target::PopupRow(2))).unwrap();
    let (x, y) = at(value.col, value.row);
    ui.click(x, y, &mut host);
    assert_eq!(ui.settings.sound.sb.model, SbModel::SbPro2);
    // Its ► steps it.
    ui.draw(&mut frame);
    let step = ui.hits.iter().find(|h| matches!(h.target, Target::Step(0, 1))).unwrap();
    let (x, y) = at(step.col, step.row);
    ui.click(x, y, &mut host);
    assert_eq!(ui.settings.sound.sb.model, SbModel::Sb2);

    // And the ► of the Mixer page's first slider.
    ui.show_page(Page::Mixer);
    ui.draw(&mut frame);
    let step = ui.hits.iter().find(|h| matches!(h.target, Target::Step(0, 1))).unwrap();
    let (x, y) = at(step.col, step.row);
    ui.click(x, y, &mut host);
    assert_eq!(ui.settings.mixer.level(Channel::Master), 110);
}

#[test]
fn long_pages_have_a_scroll_bar() {
    let mut host = FakeHost::new();
    let mut ui = opened(&host);
    let mut frame = Frame::new(640, 400);
    let layout = Layout::for_frame(640, 400);
    let at = |col: usize, row: usize| ((layout.x + col * 8 + 4) as i32, (layout.y + row * layout.cell_h + 4) as i32);
    // The scroll bar's rows that page up and down (below the tabs, whose
    // arrow is in the same column).
    let bar = |ui: &ConfigUi| -> Vec<(usize, UiKey)> {
        ui.hits
            .iter()
            .filter(|h| h.col == layout.cols - 2 && h.row > 1)
            .filter_map(|h| match h.target {
                Target::Key(key) => Some((h.row, key)),
                _ => None,
            })
            .collect()
    };

    // The Display and Sound pages fit.
    for page in [Page::Display, Page::Sound] {
        ui.show_page(page);
        ui.draw(&mut frame);
        assert!(bar(&ui).is_empty(), "{:?}", page);
    }

    // The Emulator page doesn't: the thumb is at the top, and below it the
    // bar pages down.
    ui.show_page(Page::Emulator);
    ui.draw(&mut frame);
    let (visible, total) = (ui.visible, ui.items().len());
    assert!(total > visible, "{} rows in {}", total, visible);
    let rows = bar(&ui);
    assert!(rows.iter().all(|&(_, key)| key == UiKey::PageDown), "{:?}", rows);
    assert_eq!(rows.first().unwrap().0, 3 + visible - rows.len());
    let (x, y) = at(layout.cols - 2, rows.last().unwrap().0);
    ui.click(x, y, &mut host);
    assert_eq!(ui.row, visible);
    ui.draw(&mut frame);
    assert_eq!(ui.scroll, 1);
    ui.key(UiKey::End, &mut host);
    ui.draw(&mut frame);
    assert_eq!(ui.scroll, total - visible);

    // At the end, the thumb is at the bottom and the bar above pages up.
    let rows = bar(&ui);
    assert!(rows.iter().all(|&(_, key)| key == UiKey::PageUp), "{:?}", rows);
    assert_eq!(rows.first().unwrap().0, 3);
    let (x, y) = at(layout.cols - 2, 3);
    ui.click(x, y, &mut host);
    assert_eq!(ui.row, total - 1 - visible);
}

#[test]
fn the_crt_shader_steps_through_the_looks() {
    let mut host = FakeHost::new();
    let mut ui = opened(&host);
    use UiKey::*;
    ui.show_page(Page::Display);
    ui.row = ui.items().iter().position(|&i| i == Item::Shader).unwrap();
    keys(&mut ui, &mut host, &[Right, Right, Right, Right]);
    let looks: Vec<Shader> = host.applied.iter().map(|s| s.shader).collect();
    assert_eq!(looks, [Shader::Scanlines, Shader::Aperture, Shader::Crt, Shader::None]);
    assert_eq!(listed(&mut ui, &mut host), Shader::ALL.map(Shader::describe));
    pick(&mut ui, &mut host, "CRT");
    assert_eq!(host.applied.last().unwrap().shader, Shader::Crt);
    assert_eq!(ui.item().map(|i| i.value(&ui.settings, None)).as_deref(), Some("CRT"));

    // The CRT look has its curvature below it.
    ui.row = ui.items().iter().position(|&i| i == Item::Shader).unwrap();
    assert_eq!(ui.items()[ui.row + 1], Item::CrtCurvature);
    keys(&mut ui, &mut host, &[Down]);
    assert_eq!(ui.item().map(|i| i.value(&ui.settings, None)).as_deref(), Some(" 30% ■■■·······"));
    keys(&mut ui, &mut host, &[Right, Right]);
    assert_eq!(host.applied.last().unwrap().crt.curvature, 50);
    keys(&mut ui, &mut host, &[Enter, End, Backspace, Backspace]);
    ui.text("0", &mut host);
    ui.key(Enter, &mut host);
    assert_eq!(host.applied.last().unwrap().crt.curvature, 0);
    keys(&mut ui, &mut host, &[Delete]);
    assert_eq!(host.applied.last().unwrap().crt.curvature, 30);
    assert_eq!(Item::CrtCurvature.applies(), Applies::Now);
    // And its glow below that.
    keys(&mut ui, &mut host, &[Down]);
    assert_eq!(ui.item(), Some(Item::CrtGlow));
    assert_eq!(ui.item().map(|i| i.value(&ui.settings, None)).as_deref(), Some(" 20% ■■········"));
    for _ in 0..10 {
        ui.key(Right, &mut host);
    }
    assert_eq!(host.applied.last().unwrap().crt.glow, 100);
    keys(&mut ui, &mut host, &[Enter, End, Backspace, Backspace, Backspace]);
    ui.text("150", &mut host);
    ui.key(Enter, &mut host);
    assert_eq!(status(&ui), ("The glow goes from 0 to 100%", true));
    keys(&mut ui, &mut host, &[Esc, Left]);
    assert_eq!(host.applied.last().unwrap().crt.glow, 90);
    assert_eq!(Item::CrtGlow.applies(), Applies::Now);
    keys(&mut ui, &mut host, &[Save]);
    assert_eq!(host.saved.last().unwrap().crt, crate::video::shader::CrtSettings { curvature: 30, glow: 90 });
    keys(&mut ui, &mut host, &[Up, Up]);
    pick(&mut ui, &mut host, "aperture grille");
    assert_eq!(host.applied.last().unwrap().shader, Shader::Aperture);
    assert!(!ui.items().contains(&Item::CrtCurvature));

    // Without shaders the window says so, and keeps the setting to save.
    host.no_shaders = true;
    pick(&mut ui, &mut host, "scanlines");
    assert_eq!(status(&ui), ("CRT shaders need OpenGL 3", true));
    assert_eq!(ui.settings.shader, Shader::Scanlines);
    keys(&mut ui, &mut host, &[Save]);
    assert_eq!(host.saved.last().unwrap().shader, Shader::Scanlines);
}

#[test]
fn the_monochrome_monitor_steps_through_the_phosphors() {
    use crate::video::mono::Monochrome;
    let mut host = FakeHost::new();
    let mut ui = opened(&host);
    use UiKey::*;
    ui.show_page(Page::Display);
    ui.row = ui.items().iter().position(|&i| i == Item::Monochrome).unwrap();
    assert_eq!(ui.item().map(|i| i.value(&ui.settings, None)).as_deref(), Some("off (colour)"));
    assert_eq!(listed(&mut ui, &mut host), Monochrome::ALL.map(Monochrome::describe));
    pick(&mut ui, &mut host, "green");
    use Monochrome::*;
    assert_eq!(host.applied.last().unwrap().monochrome, Green);
    assert_eq!(ui.item().map(|i| i.value(&ui.settings, None)).as_deref(), Some("green"));
    keys(&mut ui, &mut host, &[Save]);
    assert_eq!(host.saved.last().unwrap().monochrome, Green);
    assert_eq!(Item::Monochrome.applies(), Applies::NowAndAtPrompt);

    // Programs see the new monitor at the prompt; the picture is new now.
    let colour = Settings::default().video_setup();
    assert!(pending_note(colour, &ui.settings).starts_with("The picture changed"));
    let cga = Settings { machine: crate::video::adapter::Adapter::Cga, ..ui.settings.clone() };
    assert_eq!(pending_note(colour, &cga), "Takes effect when the running program ends");
}

#[test]
fn the_mixer_page_sets_volumes() {
    let mut host = FakeHost::new();
    let mut ui = opened(&host);
    use UiKey::*;
    ui.show_page(Page::Mixer);
    assert_eq!(ui.items().iter().filter(|i| matches!(i, Item::Volume(_))).count(), crate::mixer::CHANNELS);
    ui.row = ui.items().iter().position(|&i| i == Item::Volume(Channel::Fm)).unwrap();
    let fm = |host: &FakeHost| host.applied.last().unwrap().mixer.level(Channel::Fm);

    // Tens up and down, and no further than 0 and 200.
    keys(&mut ui, &mut host, &[Right]);
    assert_eq!(fm(&host), 110);
    keys(&mut ui, &mut host, &[Left, Left]);
    assert_eq!(fm(&host), 90);
    assert_eq!(ui.item().map(|i| i.value(&ui.settings, None)).as_deref(), Some(" 90% ■■■■■■■■■···········"));

    // A typed volume, then steps to the tens around it.
    keys(&mut ui, &mut host, &[Enter, End, Backspace, Backspace]);
    ui.text("85", &mut host);
    ui.key(Enter, &mut host);
    assert_eq!(fm(&host), 85);
    keys(&mut ui, &mut host, &[Left]);
    assert_eq!(fm(&host), 80);
    keys(&mut ui, &mut host, &[Right, Right]);
    assert_eq!(fm(&host), 100);

    // Too loud is refused.
    keys(&mut ui, &mut host, &[Enter, End, Backspace, Backspace, Backspace]);
    ui.text("250", &mut host);
    ui.key(Enter, &mut host);
    assert!(status(&ui).1, "{:?}", status(&ui));
    assert_eq!(ui.settings.mixer.level(Channel::Fm), 100);
    ui.key(Esc, &mut host);

    // Delete puts it back to 100.
    for _ in 0..30 {
        ui.key(Left, &mut host);
    }
    assert_eq!(fm(&host), 0);
    keys(&mut ui, &mut host, &[Delete]);
    assert_eq!(fm(&host), 100);
    for _ in 0..30 {
        ui.key(Right, &mut host);
    }
    assert_eq!(fm(&host), 200);
    assert_eq!(ui.item().map(|i| i.value(&ui.settings, None)).as_deref(), Some(&*format!("200% {}", "■".repeat(20))));
    assert!(ui.items().iter().all(|i| i.applies() == Applies::Now));

    keys(&mut ui, &mut host, &[Save]);
    assert_eq!(host.saved.last().unwrap().mixer.level(Channel::Fm), 200);
}

#[test]
fn the_mixer_page_switches_filters_and_effects() {
    use crate::mixer::{ChorusPreset, ReverbPreset, SbFilter};
    let mut host = FakeHost::new();
    let mut ui = opened(&host);
    use UiKey::*;
    ui.show_page(Page::Mixer);
    ui.row = ui.items().iter().position(|&i| i == Item::SpeakerFilter).unwrap();
    assert_eq!(ui.item().map(|i| i.value(&ui.settings, None)).as_deref(), Some("on"));
    pick(&mut ui, &mut host, "off");
    assert!(!host.applied.last().unwrap().mixer.speaker_filter);
    ui.key(Down, &mut host);
    pick(&mut ui, &mut host, "off");
    assert_eq!(host.applied.last().unwrap().mixer.sb_filter, SbFilter::Off);
    // The effects' mixes show while they are on.
    assert!(!ui.items().contains(&Item::ReverbMix) && !ui.items().contains(&Item::ChorusMix));
    ui.key(Down, &mut host);
    pick(&mut ui, &mut host, ReverbPreset::Medium.name());
    assert_eq!(host.applied.last().unwrap().mixer.reverb, ReverbPreset::Medium);
    keys(&mut ui, &mut host, &[Down]);
    assert_eq!(ui.item(), Some(Item::ReverbMix));
    assert_eq!(ui.item().map(|i| i.value(&ui.settings, None)).as_deref(), Some(" 50% ■■■■■·····"));
    keys(&mut ui, &mut host, &[Right, Right, Right, Right, Right, Right]);
    assert_eq!(host.applied.last().unwrap().mixer.reverb_mix, 100);
    keys(&mut ui, &mut host, &[Enter, End, Backspace, Backspace, Backspace]);
    ui.text("35", &mut host);
    ui.key(Enter, &mut host);
    assert_eq!(host.applied.last().unwrap().mixer.reverb_mix, 35);
    keys(&mut ui, &mut host, &[Left]);
    assert_eq!(host.applied.last().unwrap().mixer.reverb_mix, 30);
    ui.key(Down, &mut host);
    pick(&mut ui, &mut host, ChorusPreset::Strong.name());
    assert_eq!(host.applied.last().unwrap().mixer.chorus, ChorusPreset::Strong);
    keys(&mut ui, &mut host, &[Down, Left, Left]);
    assert_eq!(host.applied.last().unwrap().mixer.chorus_mix, 30);
    keys(&mut ui, &mut host, &[Delete]);
    assert_eq!(host.applied.last().unwrap().mixer.chorus_mix, 50);
    assert_eq!(Item::Reverb.applies(), Applies::Now);
    assert_eq!(Item::ChorusMix.applies(), Applies::Now);
    ui.draw(&mut Frame::new(640, 400));
    keys(&mut ui, &mut host, &[Save]);
    let saved = host.saved.last().unwrap().mixer;
    assert_eq!((saved.reverb_mix, saved.chorus_mix), (30, 50));

    // The MIXER command turning the chorus off takes its mix away.
    let mut mixer = saved;
    mixer.chorus = ChorusPreset::Off;
    ui.sync_mixer(mixer);
    assert!(!ui.items().contains(&Item::ChorusMix));
    assert_eq!(ui.item(), Some(Item::Chorus));
}

#[test]
fn the_machine_plays_on_while_the_mixer_page_shows() {
    let host = FakeHost::new();
    let mut ui = ConfigUi::new();
    assert!(!ui.pauses_machine());
    ui.open(&Settings::default(), None, &host);
    assert!(ui.pauses_machine());
    ui.show_page(Page::Mixer);
    assert!(!ui.pauses_machine());
    ui.show_page(Page::Sound);
    assert!(ui.pauses_machine());
    ui.show_page(Page::Stats);
    assert!(!ui.pauses_machine(), "the Stats page shows it running");
}

#[test]
fn the_stats_page_draws_its_numbers_and_graphs() {
    let host = FakeHost::new();
    let mut ui = opened(&host);
    ui.show_page(Page::Stats);
    let mut frame = Frame::new(640, 400);
    ui.draw(&mut frame);
    ui.set_stats(crate::stats::StatsView {
        fps: 35.0,
        refresh_hz: 70.0,
        cycles_per_ms: 100_000,
        mips: 98.5,
        recompiled: 97.0,
        halted: 12.0,
        cpu_use: 40.0,
        render_ms: 0.4,
        blocks: 3456,
        code_bytes: 2 << 20,
        fps_history: vec![30.0, 35.0, 70.0],
        cpu_history: vec![40.0; 120],
    });
    let has = |frame: &Frame, c: Rgb| frame.rgb.chunks(3).any(|px| px == [c.0, c.1, c.2]);
    for (width, height) in [(640, 400), (640, 350), (320, 200), (1024, 768)] {
        let mut frame = Frame::new(width, height);
        ui.draw(&mut frame);
        assert!(has(&frame, draw::GOOD), "the frames graph and digits at {}x{}", width, height);
        assert!(has(&frame, draw::KEY), "the CPU graph at {}x{}", width, height);
    }

    // The values in big digits, labelled, and the details below them.
    let mut g = Grid::new(76, 23);
    ui.draw_stats(&mut g, 3..19);
    let row = |g: &Grid, r: usize| (0..g.cols).map(|c| g.cell(c, r) as char).collect::<String>();
    assert!(row(&g, 3).contains(" FPS ") && row(&g, 3).contains(" CPU ") && row(&g, 3).contains("70 Hz display"));
    assert!((4..8).any(|r| (0..g.cols).any(|c| g.cell(c, r) == 0xDB)), "block digits");
    if let Ok(dir) = std::env::var("RUST_DOS_UI_SHOTS") {
        for (width, height) in [(640, 400), (640, 350), (320, 200), (1024, 768)] {
            let mut frame = Frame::new(width, height);
            for (i, px) in frame.rgb.chunks_mut(3).enumerate() {
                px.copy_from_slice(&[(i % 97) as u8, 40, 90]);
            }
            ui.draw(&mut frame);
            crate::capture::png::save(&frame, std::path::Path::new(&format!("{}/stats_{}x{}.png", dir, width, height))).unwrap();
        }
    }
    assert!(row(&g, 4).contains("avg 45"), "{}", row(&g, 4));
    assert!(row(&g, 17).contains("Cycles") && row(&g, 17).contains("100000/ms") && row(&g, 17).contains("98.5"));
    assert!(row(&g, 18).contains("Render") && row(&g, 18).contains("0.4 ms"));
}

#[test]
fn the_meters_fall_slowly() {
    let host = FakeHost::new();
    let mut ui = opened(&host);
    let mut peaks = [0.0; crate::mixer::CHANNELS];
    peaks[Channel::Sb as usize] = 0.5;
    ui.set_mixer_status(false, peaks);
    ui.set_mixer_status(true, [0.0; crate::mixer::CHANNELS]);
    assert!((ui.levels[Channel::Sb as usize] - 0.48).abs() < 1e-6);
    assert!(ui.muted);
    // Loud, clipping and muted meters draw.
    peaks[Channel::Master as usize] = 1.5;
    ui.set_mixer_status(true, peaks);
    ui.show_page(Page::Mixer);
    ui.draw(&mut Frame::new(640, 400));
    ui.set_mixer_status(false, peaks);
    ui.draw(&mut Frame::new(640, 400));
}

#[test]
fn the_video_card_changes_at_the_prompt() {
    use crate::video::adapter::Adapter;
    let mut host = FakeHost::new();
    let mut ui = opened(&host);
    ui.show_page(Page::Emulator);
    ui.row = ui.items().iter().position(|&i| i == Item::Machine).unwrap();
    assert_eq!(Item::Machine.applies(), Applies::AtPrompt);
    assert_eq!(ui.item().map(|i| i.value(&ui.settings, None)).as_deref(), Some("Super VGA (VESA)"));
    assert_eq!(listed(&mut ui, &mut host), Adapter::ALL.map(Adapter::describe));
    for adapter in [Adapter::S3, Adapter::Vga, Adapter::Ega, Adapter::Cga, Adapter::Tandy, Adapter::Pcjr, Adapter::Hercules, Adapter::Svga] {
        pick(&mut ui, &mut host, adapter.describe());
        assert_eq!(host.applied.last().unwrap().machine, adapter);
    }
}

#[test]
fn the_joystick_steps_through_the_types() {
    use crate::joystick::JoystickType;
    let mut host = FakeHost::new();
    let mut ui = opened(&host);
    use UiKey::*;
    ui.show_page(Page::Emulator);
    ui.row = ui.items().iter().position(|&i| i == Item::Joystick).unwrap();
    assert_eq!(Item::Joystick.applies(), Applies::Now);
    keys(&mut ui, &mut host, &[Right, Right, Right, Right, Right]);
    let kinds: Vec<JoystickType> = host.applied.iter().map(|s| s.joystick.kind).collect();
    use JoystickType as J;
    assert_eq!(kinds, [J::FourAxis, J::TwoAxis, J::Mouse, J::None, J::Auto]);
    assert_eq!(listed(&mut ui, &mut host), JoystickType::ALL.map(JoystickType::describe));
    pick(&mut ui, &mut host, JoystickType::FourAxis.describe());
    assert_eq!(host.applied.last().unwrap().joystick.kind, JoystickType::FourAxis);

    // The deadzone slides in fives, is typed, and back to 10 with Delete.
    keys(&mut ui, &mut host, &[Down, Right]);
    assert_eq!(host.applied.last().unwrap().joystick.deadzone, 15);
    keys(&mut ui, &mut host, &[Enter, End, Backspace, Backspace]);
    ui.text("12", &mut host);
    ui.key(Enter, &mut host);
    assert_eq!(host.applied.last().unwrap().joystick.deadzone, 12);
    keys(&mut ui, &mut host, &[Left]);
    assert_eq!(ui.item().map(|i| i.value(&ui.settings, None)).as_deref(), Some(" 10% ■■················"));
    keys(&mut ui, &mut host, &[Left, Left, Left]);
    assert_eq!(host.applied.last().unwrap().joystick.deadzone, 0);
    keys(&mut ui, &mut host, &[Delete]);
    assert_eq!(host.applied.last().unwrap().joystick.deadzone, 10);
}

#[test]
fn expanded_memory_changes_at_the_prompt() {
    let mut host = FakeHost::new();
    let mut ui = opened(&host);
    ui.show_page(Page::Emulator);
    ui.row = ui.items().iter().position(|&i| i == Item::Ems).unwrap();
    assert_eq!(Item::Ems.applies(), Applies::AtPrompt);
    assert_eq!(ui.item().map(|i| i.value(&ui.settings, None)).as_deref(), Some("on"));
    pick(&mut ui, &mut host, "off");
    assert!(!host.applied.last().unwrap().ems);
    ui.key(UiKey::Down, &mut host);
    pick(&mut ui, &mut host, "off");
    assert_eq!(ui.item(), Some(Item::Umb));
    assert_eq!(Item::Umb.applies(), Applies::AtPrompt);
    assert!(!host.applied.last().unwrap().umb);
}

#[test]
fn the_parallel_port_dac_changes_at_the_prompt() {
    use crate::lpt_dac::LptDacType;
    let mut host = FakeHost::new();
    let mut ui = opened(&host);
    ui.show_page(Page::Sound);
    ui.row = ui.items().iter().position(|&i| i == Item::LptDac).unwrap();
    assert_eq!(Item::LptDac.applies(), Applies::AtPrompt);
    pick(&mut ui, &mut host, "Covox Speech Thing");
    assert_eq!(host.applied.last().unwrap().sound.lpt_dac, LptDacType::Covox);
    assert_eq!(ui.item().map(|i| i.value(&ui.settings, None)).as_deref(), Some("Covox Speech Thing"));
    ui.show_page(Page::Mixer);
    assert!(ui.items().contains(&Item::Volume(Channel::LptDac)));
}

#[test]
fn the_capture_folder_is_typed() {
    let mut host = FakeHost::new();
    let mut ui = opened(&host);
    use UiKey::*;
    ui.show_page(Page::Emulator);
    ui.row = ui.items().iter().position(|&i| i == Item::CaptureDir).unwrap();
    assert_eq!(ui.item().map(|i| i.value(&ui.settings, None)).as_deref(), Some("capture"));
    keys(&mut ui, &mut host, &[Enter, End]);
    for _ in 0..7 {
        ui.key(Backspace, &mut host);
    }
    ui.text("shots", &mut host);
    ui.key(Enter, &mut host);
    assert_eq!(host.applied.last().unwrap().capture_dir, PathBuf::from("shots"));
    keys(&mut ui, &mut host, &[Delete]);
    assert_eq!(host.applied.last().unwrap().capture_dir, PathBuf::from("capture"));
}

#[test]
fn a_browser_gets_what_it_has() {
    let browser = Frontend { window: false, host_files: false };
    let mut host = FakeHost::new();
    let mut ui = ConfigUi::for_frontend(browser);
    ui.open(&Settings::default(), Some("rust-dos.conf".into()), &host);
    use UiKey::*;

    // No window to scale or make fullscreen, no SoundFont to pick.
    ui.show_page(Page::Display);
    assert_eq!(
        ui.items(),
        [Item::Aspect, Item::Filter, Item::Shader, Item::Monochrome, Item::Composite, Item::CompositeEra]
    );
    pick(&mut ui, &mut host, "on");
    assert!(host.applied.last().unwrap().aspect);
    ui.show_page(Page::Sound);
    assert!(!ui.items().contains(&Item::SoundFont));
    ui.row = ui.items().iter().position(|&i| i == Item::Midi).unwrap();
    assert_eq!(listed(&mut ui, &mut host), ["auto", "Ultrasound patches", "none"]);
    pick(&mut ui, &mut host, "none");
    assert_eq!(host.applied.last().unwrap().sound.midisynth, MidiSynth::None);
    // The page records the canvas, into files of its own.
    ui.show_page(Page::Emulator);
    assert!(![Item::CaptureDir, Item::RecordUi, Item::RecordShader].iter().any(|i| ui.items().contains(i)));

    // Drives come from images the frontend picks, not a dialog of host paths.
    ui.show_page(Page::Drives);
    ui.drives.insert(1, drive_info(&MountSpec { drive: 0, path: "A.IMG".into(), opts: MountOptions::default() }));
    keys(&mut ui, &mut host, &[Insert, Down, Enter, End, Enter]);
    assert!(ui.dialog.is_none());
    assert_eq!(host.images, [None, Some(0), None]);
    // The frontend can refuse, and says why.
    keys(&mut ui, &mut host, &[Home, Enter]);
    assert_eq!(status(&ui), ("C: stays", true));
    keys(&mut ui, &mut host, &[Down, Delete]);
    assert_eq!(host.unmounts, [0]);

    let mut frame = Frame::new(640, 400);
    for page in PAGES {
        ui.show_page(page);
        ui.draw(&mut frame);
    }
}

#[test]
fn captures_show_the_window_overlay_and_shader_when_asked() {
    let mut host = FakeHost::new();
    let mut ui = opened(&host);
    ui.show_page(Page::Emulator);
    ui.row = ui.items().iter().position(|&i| i == Item::RecordUi).unwrap();
    assert_eq!(ui.item().map(|i| i.value(&ui.settings, None)).as_deref(), Some("off (the picture alone)"));
    pick(&mut ui, &mut host, "on (window and overlay)");
    assert!(host.applied.last().unwrap().record_ui);
    assert_eq!(ui.item().map(|i| i.value(&ui.settings, None)).as_deref(), Some("on (window and overlay)"));
    assert_eq!(Item::RecordUi.applies(), Applies::Now);

    ui.row = ui.items().iter().position(|&i| i == Item::RecordShader).unwrap();
    assert_eq!(ui.item().map(|i| i.value(&ui.settings, None)).as_deref(), Some("off (the plain picture)"));
    pick(&mut ui, &mut host, "on (as the window shows it)");
    assert!(host.applied.last().unwrap().record_shader);
    assert_eq!(ui.item().map(|i| i.value(&ui.settings, None)).as_deref(), Some("on (as the window shows it)"));
    assert_eq!(Item::RecordShader.applies(), Applies::Now);
}

#[test]
fn the_drives_page_makes_and_mounts_new_disk_images() {
    let dir = std::env::temp_dir().join(format!("rust-dos-ui-makeimg-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut host = FakeHost::new();
    let mut ui = opened(&host);
    use UiKey::*;
    // The last row, after "+ Mount a drive...".
    assert_eq!(ui.row_count(), 4);
    keys(&mut ui, &mut host, &[End, Enter]);
    let mut frame = Frame::new(640, 400);
    ui.draw(&mut frame);
    assert!(ui.image_dialog.is_some());
    assert!(ui.hits.iter().any(|h| matches!(h.target, Target::ImageField(ImageField::Create))), "its buttons can be clicked");

    // A 720 KB floppy, labelled, on A:.
    let path = dir.join("disk1.img");
    let dialog = ui.image_dialog.as_mut().unwrap();
    dialog.path = TextField::new(&path.display().to_string());
    dialog.focus = ImageField::Kind;
    keys(&mut ui, &mut host, &[Left, Left, Tab]);
    ui.text("Saves", &mut host);
    keys(&mut ui, &mut host, &[Enter]);
    assert!(ui.image_dialog.is_none(), "{:?}", status(&ui));
    assert_eq!(std::fs::metadata(&path).unwrap().len(), 737_280);
    let (spec, replace) = host.mounts.last().unwrap();
    assert_eq!((spec.drive, spec.opts.kind, spec.path.as_path(), *replace), (0, DriveKind::Floppy, path.as_path(), false));
    assert!(status(&ui).0.ends_with("disk1.img has been made and mounted as A:"), "{:?}", status(&ui));
    assert_eq!(ui.drives[ui.row].drive, 0, "the new drive is selected");

    // Not over a file; a FAT32 disk mounts.
    keys(&mut ui, &mut host, &[End, Enter]);
    let dialog = ui.image_dialog.as_mut().unwrap();
    dialog.path = TextField::new(&path.display().to_string());
    keys(&mut ui, &mut host, &[Enter]);
    assert_eq!(status(&ui), (format!("{} already exists: pick another name", path.display()).as_str(), true));
    let dialog = ui.image_dialog.as_mut().unwrap();
    dialog.path = TextField::new(&dir.join("big.img").display().to_string());
    dialog.kind = crate::makeimg::PRESETS.len();
    dialog.size = TextField::new("3000");
    dialog.mount = Some(3);
    ui.draw(&mut frame);
    keys(&mut ui, &mut host, &[Enter]);
    assert!(status(&ui).0.ends_with("big.img has been made and mounted as D:"), "{:?}", status(&ui));
    assert_eq!(host.mounts.last().map(|(spec, _)| (spec.drive, spec.opts.kind)), Some((3, DriveKind::HardDisk)));
    assert!(ui.image_dialog.is_none());

    // The browser doesn't offer it: it has no host files.
    let mut browser = ConfigUi::for_frontend(Frontend { window: false, host_files: false });
    browser.open(&Settings::default(), None, &host);
    assert_eq!(browser.row_count(), browser.drives.len() + 1);
}

#[test]
fn long_text_is_shortened_in_the_middle() {
    assert_eq!(fit("/home/user/dos/games", 30), "/home/user/dos/games");
    assert_eq!(fit("/home/user/dos/games/doom.cue", 14), "/ho...doom.cue");
    assert_eq!(fit("abcdef", 2), "ab");
}

#[test]
fn the_games_page_launches_makes_and_deletes_games() {
    let mut host = FakeHost::new();
    host.games.push(GameEntry { id: "stunts".into(), name: "Stunts".into(), command: "STUNTS".into() });
    let mut ui = opened(&host);
    use UiKey::*;
    ui.show_page(Page::Games);
    assert_eq!(ui.row_count(), 3, "the game, a new one and an imported one");
    ui.draw(&mut Frame::new(640, 400));

    // The last row imports a game set up for DOSBox, picked from the host's
    // files.
    keys(&mut ui, &mut host, &[End, Enter]);
    let (browser, _) = ui.browser.as_ref().expect("the file picker is open");
    assert_eq!(browser.title, "Pick a GOG game's folder or a DOSBox .conf");
    assert!(browser.pick_dirs);
    keys(&mut ui, &mut host, &[Esc, Home]);

    // Ins: a new game, in the prompt's directory; its command is needed.
    keys(&mut ui, &mut host, &[Insert]);
    assert_eq!(ui.game_dialog.as_ref().unwrap().directory.text(), "C:\\GAMES");
    ui.text("Commander Keen", &mut host);
    keys(&mut ui, &mut host, &[Tab, End, Backspace, Backspace, Backspace, Backspace, Backspace]);
    ui.text("KEEN", &mut host);
    keys(&mut ui, &mut host, &[Tab, Tab, Enter]);
    assert!(status(&ui).1, "{:?}", status(&ui));
    assert!(ui.game_dialog.is_some());
    ui.draw(&mut Frame::new(640, 400));
    keys(&mut ui, &mut host, &[BackTab]);
    ui.text("KEEN4E", &mut host);
    keys(&mut ui, &mut host, &[Enter]);
    assert!(ui.game_dialog.is_none(), "{:?}", status(&ui));
    assert_eq!(host.created[0], NewGame { name: "Commander Keen".into(), directory: "C:\\KEEN".into(), command: "KEEN4E".into() });
    assert_eq!(ui.games[ui.row].id, "commander-keen");

    // Del asks first; Esc keeps it, Enter deletes it.
    keys(&mut ui, &mut host, &[Delete]);
    assert!(status(&ui).0.starts_with("Delete Commander Keen?"));
    keys(&mut ui, &mut host, &[Esc]);
    assert_eq!(ui.games.len(), 2);
    assert!(ui.is_open());
    keys(&mut ui, &mut host, &[Delete, Enter]);
    assert_eq!(ui.games.len(), 1);

    // Enter launches the game and closes the window, with a notice.
    keys(&mut ui, &mut host, &[Home, Enter]);
    assert_eq!(host.launched, ["stunts"]);
    assert!(!ui.is_open());
    assert_eq!(ui.take_notice().as_deref(), Some("Starting stunts"));
    ui.open(&Settings::default(), Some("/cfg/games/stunts.conf".into()), &host);
    assert_eq!(ui.active_game.as_deref(), Some("stunts"));
    ui.show_page(Page::Games);
    ui.draw(&mut Frame::new(640, 400));
}

#[test]
fn the_tabs_fit_or_scroll() {
    let host = FakeHost::new();
    let mut ui = opened(&host);
    for (width, height) in [(640, 400), (400, 300), (320, 200)] {
        let mut frame = Frame::new(width, height);
        for page in PAGES {
            ui.show_page(page);
            ui.draw(&mut frame);
            if ui.layout.is_none() {
                continue;
            }
            let tab = ui.hits.iter().find(|h| matches!(h.target, Target::Tab(p) if p == page));
            assert!(tab.is_some(), "{:?} has its tab at {}x{}", page, width, height);
            // And the next one shows, so it's clear there are more.
            let at = PAGES.iter().position(|&p| p == page).unwrap();
            if let Some(&next) = PAGES.get(at + 1) {
                let tab = ui.hits.iter().find(|h| matches!(h.target, Target::Tab(p) if p == next));
                assert!(tab.is_some(), "{:?} shows the next tab, {:?}, at {}x{}", page, next, width, height);
            }
            assert!(ui.hits.iter().filter(|h| h.row == 1).all(|h| h.col + h.width < ui.layout.unwrap().cols));
        }
    }
}

#[test]
fn the_cheats_page_finds_sets_and_freezes() {
    use crate::cheats::{Freeze, Width};
    let mut host = FakeHost::new();
    host.ram = vec![0; 0x10000];
    host.ram[0x1234] = 3;
    host.ram[0x2000] = 3;
    let mut ui = opened(&host);
    use UiKey::*;
    ui.show_page(Page::Cheats);
    assert!(ui.pauses_machine());
    // A new search for 3 finds both.
    ui.row = 2;
    keys(&mut ui, &mut host, &[Enter]);
    ui.text("3", &mut host);
    keys(&mut ui, &mut host, &[Enter]);
    assert!(status(&ui).0.starts_with("2 addresses hold 3"), "{:?}", status(&ui));
    assert_eq!(ui.row_count(), 6);
    ui.draw(&mut Frame::new(640, 400));

    // The game changed one to 2: narrowing to 2 leaves it.
    host.ram[0x1234] = 2;
    keys(&mut ui, &mut host, &[Down, Enter]);
    ui.text("2", &mut host);
    keys(&mut ui, &mut host, &[Enter]);
    assert_eq!(ui.row_count(), 5);
    assert!(status(&ui).0.starts_with("One address is left"), "{:?}", status(&ui));

    // Set it to 9, then freeze it.
    keys(&mut ui, &mut host, &[Down, Enter, End, Backspace]);
    ui.text("9", &mut host);
    keys(&mut ui, &mut host, &[Enter]);
    assert_eq!(host.ram[0x1234], 9);
    keys(&mut ui, &mut host, &[Insert]);
    assert_eq!(host.frozen, [Freeze { addr: 0x1234, width: Width::Byte, value: 9 }]);
    assert_eq!(ui.row_count(), 6, "the frozen value is listed");
    ui.draw(&mut Frame::new(640, 400));
    keys(&mut ui, &mut host, &[End, Delete]);
    assert!(host.frozen.is_empty());

    // A typo is refused; a bigger value size starts again.
    keys(&mut ui, &mut host, &[Home, Down, Down, Enter]);
    ui.text("lots", &mut host);
    keys(&mut ui, &mut host, &[Enter]);
    assert!(status(&ui).1);
    keys(&mut ui, &mut host, &[Esc, Home, Right]);
    assert_eq!(ui.row_count(), 4);
    // Changed or not, without a search, is an error.
    keys(&mut ui, &mut host, &[Down, Down, Down, Right, Enter]);
    assert!(status(&ui).1, "{:?}", status(&ui));
}

#[test]
fn the_states_page_saves_loads_and_empties_slots() {
    let mut host = FakeHost::new();
    host.slot = 3;
    let mut ui = opened(&host);
    use UiKey::*;
    ui.show_states(&host);
    assert_eq!((ui.page, ui.row, ui.states.len()), (Page::States, 2, 9), "on the hotkeys' slot");
    // An empty slot doesn't load; Ins saves to it.
    ui.key(Enter, &mut host);
    assert!(status(&ui).1 && host.loaded.is_empty());
    ui.key(Insert, &mut host);
    assert_eq!(status(&ui), ("Saved to slot 3", false));
    assert!(ui.states[2].header.is_some());
    let mut frame = Frame::new(640, 400);
    ui.draw(&mut frame);
    assert!(ui.hits.iter().any(|h| matches!(h.target, Target::Row(2))));
    // Del asks, Enter empties it.
    ui.key(Delete, &mut host);
    assert!(status(&ui).1);
    ui.key(Enter, &mut host);
    assert!(ui.states[2].header.is_none() && host.states.is_empty());
    // Enter on a filled slot loads it and closes the window.
    ui.key(Down, &mut host);
    ui.key(Insert, &mut host);
    ui.key(Enter, &mut host);
    assert_eq!(host.loaded, [4]);
    assert!(!ui.is_open());
    assert_eq!(ui.take_notice().as_deref(), Some("Loaded slot 4"));
}

#[test]
fn the_emulator_page_edits_the_autoexec_commands() {
    let mut host = FakeHost::new();
    host.autoexec = vec!["# mine".into(), "MOUNT D ~/d".into()];
    let mut ui = opened(&host);
    use UiKey::*;
    ui.show_page(Page::Emulator);
    ui.row = ui.items().iter().position(|&i| i == Item::Autoexec).unwrap();
    ui.key(Enter, &mut host);
    assert!(ui.autoexec.is_some());

    // A command after the last line, the comment made a command, and a
    // line joined to the one above and split again. Tab stays here.
    keys(&mut ui, &mut host, &[Down, End, Enter]);
    ui.text("D:", &mut host);
    keys(&mut ui, &mut host, &[Up, Up, Home, Delete, Delete, Down, Home, Backspace, Enter, Tab]);
    assert_eq!(ui.page, Page::Emulator);
    ui.key(Save, &mut host);
    assert!(ui.autoexec.is_none());
    assert_eq!(host.autoexec, ["mine", "MOUNT D ~/d", "D:"]);
    assert!(status(&ui).0.contains("next time"), "{:?}", status(&ui));

    // Esc with changes asks first; without, it just closes.
    ui.key(Enter, &mut host);
    ui.text("x", &mut host);
    ui.key(Esc, &mut host);
    assert!(ui.autoexec.is_some() && status(&ui).1, "{:?}", status(&ui));
    ui.key(Esc, &mut host);
    assert!(ui.autoexec.is_none());
    assert_eq!(host.autoexec, ["mine", "MOUNT D ~/d", "D:"]);
    keys(&mut ui, &mut host, &[Enter, Esc]);
    assert!(ui.autoexec.is_none() && ui.is_open());

    // A click puts the cursor there; a long line scrolls to show it.
    ui.key(Enter, &mut host);
    let mut frame = Frame::new(640, 400);
    ui.draw(&mut frame);
    let layout = ui.layout.unwrap();
    let line = ui.hits.iter().find(|h| matches!(h.target, Target::EditorLine(1))).unwrap();
    let (x, y) = ((layout.x + (line.col + 6) * 8 + 4) as i32, (layout.y + line.row * layout.cell_h + 4) as i32);
    ui.click(x, y, &mut host);
    ui.text(&"y".repeat(200), &mut host);
    ui.draw(&mut frame);
    ui.key(Save, &mut host);
    assert_eq!(host.autoexec[1], format!("MOUNT {}D ~/d", "y".repeat(200)));

    // Without a configuration file there is none to edit.
    let mut ui = ConfigUi::new();
    ui.open(&Settings::default(), None, &host);
    ui.show_page(Page::Emulator);
    ui.row = ui.items().iter().position(|&i| i == Item::Autoexec).unwrap();
    ui.key(Enter, &mut host);
    assert!(ui.autoexec.is_none() && status(&ui).1, "{:?}", status(&ui));
}

#[test]
fn the_network_page_finds_joins_and_makes_rooms() {
    use crate::net::tunnel::wire::RoomInfo;
    use crate::net::{LanView, Listing, RoomList};
    use super::rooms::Row;
    use UiKey::*;
    let relay: std::net::SocketAddr = "203.0.113.5:21213".parse().unwrap();
    let mut host = FakeHost::new();
    host.lan = Some(LanView { state: "not joined".into(), ..Default::default() });
    let mut ui = opened(&host);
    ui.show_page(Page::Network);
    let at = |ui: &ConfigUi, item: Item| ui.items().iter().position(|&i| i == item);

    // Rooms are on this network unless online is picked, which shows the
    // relay: the public one unless typed, and Delete brings it back.
    ui.row = at(&ui, Item::Online).unwrap();
    assert_eq!(ui.item().unwrap().value(&ui.settings, None), "on this network");
    assert_eq!(at(&ui, Item::Relay), None);
    pick(&mut ui, &mut host, "via Internet");
    assert!(host.applied.last().unwrap().network.online);
    ui.row = at(&ui, Item::Relay).unwrap();
    assert_eq!(ui.item().unwrap().value(&ui.settings, None), "relay.rust-dos.com");
    ui.key(Enter, &mut host);
    keys(&mut ui, &mut host, &[Backspace; 40]);
    ui.text("192.0.2.9:4000", &mut host);
    ui.key(Enter, &mut host);
    assert_eq!(host.applied.last().unwrap().network.relay, "192.0.2.9:4000");
    ui.key(Delete, &mut host);
    assert_eq!(ui.settings.network.relay, "relay.rust-dos.com");

    // The browser asks the relay online for its rooms.
    ui.row = at(&ui, Item::Rooms).unwrap();
    ui.key(Enter, &mut host);
    assert!(ui.rooms.is_some());
    ui.poll(&mut host);
    assert_eq!(host.browsed, [(Some("relay.rust-dos.com".to_string()), String::new())]);
    let mut frame = Frame::new(640, 400);
    ui.draw(&mut frame);
    let room = |name: &str, members, password| RoomInfo { name: name.into(), members, password };
    let list = RoomList {
        relay,
        name: "rust-dos public relay".into(),
        password: false,
        rooms: vec![room("doom2 dm", 3, true), room("duke", 1, false)],
        total: 2,
    };
    host.lan.as_mut().unwrap().listing = Listing { asking: false, result: Some(Ok(vec![list.clone()])) };
    ui.poll(&mut host);
    ui.draw(&mut frame);
    assert_eq!(ui.rooms.as_ref().unwrap().rows().len(), 3);
    assert!(ui.hits.iter().any(|h| matches!(h.target, Target::RoomRow(2))));

    // A search narrows them down at once, and is asked for soon after.
    ui.text("du", &mut host);
    let rows = ui.rooms.as_ref().unwrap().rows();
    assert_eq!(rows, [Row::Room(room("duke", 1, false), relay), Row::Make(Some("du".into()))]);
    ui.poll(&mut host);
    assert_eq!(host.browsed.len(), 1, "not while it is typed");
    ui.rooms.as_mut().unwrap().age_last_question();
    ui.poll(&mut host);
    assert_eq!(host.browsed.last().unwrap().1, "du");

    // Enter joins an open room where it was listed, and the window says
    // when it is in it.
    ui.key(Enter, &mut host);
    assert_eq!(host.joined.last().unwrap(), &(Some("203.0.113.5:21213".into()), "duke".into(), String::new()));
    assert!(status(&ui).0.starts_with("Joining room \"duke\""), "{:?}", status(&ui));
    {
        let lan = host.lan.as_mut().unwrap();
        lan.joined = Some((relay, "duke".into()));
        lan.state = "room \"duke\" at 203.0.113.5:21213, member 2 of 2".into();
        lan.listing = Listing { asking: false, result: Some(Ok(vec![list.clone()])) };
    }
    ui.poll(&mut host);
    assert_eq!(status(&ui), ("In room \"duke\": start the game's network play", false));
    ui.draw(&mut frame);
    // Enter on it again stays; the last row leaves it.
    ui.key(Enter, &mut host);
    assert_eq!(host.joined.len(), 1);
    keys(&mut ui, &mut host, &[Down, Down, Enter]);
    assert_eq!(host.left, 1);

    // A room with a password asks for it.
    keys(&mut ui, &mut host, &[Backspace, Backspace, Up, Up, Up, Enter]);
    assert!(ui.rooms.as_ref().unwrap().prompt.is_some());
    ui.draw(&mut frame);
    ui.text("pw", &mut host);
    ui.key(Enter, &mut host);
    assert_eq!(host.joined.last().unwrap(), &(Some("203.0.113.5:21213".into()), "doom2 dm".into(), "pw".into()));
    assert!(ui.rooms.as_ref().unwrap().prompt.is_none());

    // Ins makes a room, which needs a name, at the relay online.
    ui.key(Insert, &mut host);
    ui.draw(&mut frame);
    keys(&mut ui, &mut host, &[Char(' '), Enter, Enter]);
    assert!(status(&ui).1, "{:?}", status(&ui));
    keys(&mut ui, &mut host, &[BackTab, Backspace]);
    ui.text("ctf", &mut host);
    ui.key(Tab, &mut host);
    ui.text("x", &mut host);
    ui.key(Enter, &mut host);
    assert_eq!(host.joined.last().unwrap(), &(Some("relay.rust-dos.com".into()), "ctf".into(), "x".into()));
    // A search for a room that isn't there makes it, open to all.
    ui.text("new room", &mut host);
    let make = ui.rooms.as_ref().unwrap().rows().iter().position(|r| *r == Row::Make(Some("new room".into())));
    assert_eq!(make, Some(0));
    keys(&mut ui, &mut host, &[Enter, Enter]);
    assert_eq!(host.joined.last().unwrap().1, "new room");
    assert_eq!(host.joined.last().unwrap().2, "");

    // A click selects a row, and a second one acts on it.
    keys(&mut ui, &mut host, &[Backspace; 8]);
    ui.draw(&mut frame);
    let layout = ui.layout.unwrap();
    let click = |ui: &mut ConfigUi, host: &mut FakeHost, target: fn(&Target) -> bool| {
        let hit = ui.hits.iter().find(|h| target(&h.target)).unwrap();
        let (x, y) = ((layout.x + hit.col * 8 + 4) as i32, (layout.y + hit.row * layout.cell_h + 4) as i32);
        ui.click(x, y, host);
    };
    click(&mut ui, &mut host, |t| matches!(t, Target::RoomRow(2)));
    assert_eq!(ui.rooms.as_ref().unwrap().selected, 2);
    assert!(ui.rooms.as_ref().unwrap().prompt.is_none());
    click(&mut ui, &mut host, |t| matches!(t, Target::RoomRow(2)));
    assert!(ui.rooms.as_ref().unwrap().prompt.as_ref().is_some_and(|p| p.making));
    ui.key(Esc, &mut host);
    assert!(ui.rooms.as_ref().unwrap().prompt.is_none());

    // A relay that can't be reached says so.
    host.lan.as_mut().unwrap().listing = Listing { asking: false, result: Some(Err("can't find relay".into())) };
    ui.rooms.as_mut().unwrap().age_last_question();
    ui.poll(&mut host);
    host.lan.as_mut().unwrap().listing.asking = false;
    ui.poll(&mut host);
    ui.draw(&mut frame);

    // Tab goes to the rooms on this network, and the settings with it.
    let asked = host.browsed.len();
    ui.key(Tab, &mut host);
    assert!(!ui.settings.network.online && !host.applied.last().unwrap().network.online);
    ui.poll(&mut host);
    assert_eq!(host.browsed.len(), asked + 1);
    assert_eq!(host.browsed.last().unwrap(), &(None, String::new()));
    ui.draw(&mut frame);
    assert_eq!(ui.room_hints()[2].1, "Online");
    // Those found there are joined where they were found, and made here.
    let local = RoomList { relay, name: "Ranger".into(), password: false, rooms: list.rooms.clone(), total: 2 };
    host.lan.as_mut().unwrap().listing = Listing { asking: false, result: Some(Ok(vec![local])) };
    ui.poll(&mut host);
    ui.draw(&mut frame);
    ui.key(Enter, &mut host);
    ui.text("pw", &mut host);
    ui.key(Enter, &mut host);
    assert_eq!(host.joined.last().unwrap(), &(Some("203.0.113.5:21213".into()), "doom2 dm".into(), "pw".into()));
    ui.key(Insert, &mut host);
    ui.text("lan party", &mut host);
    keys(&mut ui, &mut host, &[Enter, Enter]);
    assert_eq!(host.hosted, [("lan party".to_string(), String::new())]);
    // None found yet: the list says how to make one.
    host.lan.as_mut().unwrap().listing = Listing { asking: false, result: Some(Ok(vec![])) };
    ui.rooms.as_mut().unwrap().age_last_question();
    ui.poll(&mut host);
    host.lan.as_mut().unwrap().listing.asking = false;
    ui.poll(&mut host);
    ui.draw(&mut frame);
    assert_eq!(ui.rooms.as_ref().unwrap().rows().len(), 2, "making one, and leaving duke");
    // A click on the Online tab goes back there.
    ui.draw(&mut frame);
    click(&mut ui, &mut host, |t| matches!(t, Target::RoomsOnline(true)));
    assert!(ui.settings.network.online);
    for (width, height) in [(400, 300), (1024, 768)] {
        ui.draw(&mut Frame::new(width, height));
        ui.key(Insert, &mut host);
        ui.draw(&mut Frame::new(width, height));
        ui.key(Esc, &mut host);
    }
    // Esc goes back to the page.
    ui.key(Esc, &mut host);
    assert!(ui.rooms.is_none() && ui.is_open());
    assert_eq!(ui.page, Page::Network);
}

#[test]
fn the_room_this_instance_hosts_shows_who_is_in_it() {
    use crate::net::tunnel::wire::{Member, RoomInfo, Roster};
    use crate::net::{LanView, Listing, RoomList};
    use UiKey::*;
    let relay: std::net::SocketAddr = "203.0.113.5:21213".parse().unwrap();
    let member = |index, name: &str| Member { index, name: name.into() };
    let roster = Roster { version: 3, host: 1, total: 3, members: vec![member(1, "Toumal"), member(2, ""), member(3, "kate")] };
    let list = RoomList {
        relay,
        name: "rust-dos public relay".into(),
        password: false,
        rooms: vec![RoomInfo { name: "doom".into(), members: 3, password: true }],
        total: 1,
    };
    let mut host = FakeHost::new();
    host.lan = Some(LanView {
        state: "room \"doom\" at 203.0.113.5:21213, member 1 of 3".into(),
        joined: Some((relay, "doom".into())),
        index: Some(1),
        roster: Some(roster.clone()),
        listing: Listing { asking: false, result: Some(Ok(vec![list])) },
        serial: Some((2, "COM2 linked".into())),
    });
    let mut ui = opened(&host);
    ui.show_page(Page::Network);
    ui.row = ui.items().iter().position(|&i| i == Item::Rooms).unwrap();
    ui.key(Enter, &mut host);
    ui.poll(&mut host);
    host.lan.as_mut().unwrap().listing.asking = false;
    ui.poll(&mut host);
    let mut frame = Frame::new(640, 400);
    ui.draw(&mut frame);
    // The room instead of the list: its buttons, and no rooms to pick.
    assert!(ui.hits.iter().any(|h| matches!(h.target, Target::RoomButton(RoomButton::Disband))));
    assert!(!ui.hits.iter().any(|h| matches!(h.target, Target::RoomRow(_))));
    assert_eq!(ui.room_hints()[0].1, "Leave");

    // Disband asks first; anything but Enter keeps the room.
    keys(&mut ui, &mut host, &[Tab, Enter]);
    assert!(status(&ui).1 && status(&ui).0.starts_with("End room \"doom\""), "{:?}", status(&ui));
    assert_eq!(ui.room_hints()[0].1, "End it");
    ui.key(Esc, &mut host);
    assert_eq!((host.disbanded, status(&ui).0), (0, ""));
    assert!(ui.rooms.is_some(), "Esc only kept the room");
    keys(&mut ui, &mut host, &[Enter, Enter]);
    assert_eq!(host.disbanded, 1);
    assert_eq!(status(&ui), ("Ended room \"doom\" for everyone in it", false));
    // Leave says who hosts it next.
    keys(&mut ui, &mut host, &[Left, Enter]);
    assert_eq!(host.left, 1);
    assert_eq!(status(&ui), ("Left room \"doom\": Player 2 hosts it now", false));
    // A click on a button presses it.
    ui.draw(&mut frame);
    let layout = ui.layout.unwrap();
    let hit = ui.hits.iter().find(|h| matches!(h.target, Target::RoomButton(RoomButton::Disband))).unwrap();
    let (x, y) = ((layout.x + hit.col * 8 + 4) as i32, (layout.y + hit.row * layout.cell_h + 4) as i32);
    ui.click(x, y, &mut host);
    assert!(ui.rooms.as_ref().unwrap().confirm_disband);
    ui.key(Esc, &mut host);
    for (width, height) in [(400, 300), (1024, 768)] {
        ui.draw(&mut Frame::new(width, height));
    }

    // In a room someone else hosts, the list shows, with the room in it.
    host.lan.as_mut().unwrap().roster = Some(Roster { host: 2, ..roster });
    ui.poll(&mut host);
    ui.draw(&mut frame);
    assert!(ui.hits.iter().any(|h| matches!(h.target, Target::RoomRow(0))));
    assert!(!ui.hits.iter().any(|h| matches!(h.target, Target::RoomButton(_))));
}

#[test]
fn every_setting_page_and_dialog_has_help() {
    let mut used = vec!["mount", "new-image", "new-game", "autoexec", "rooms"];
    for page in PAGES {
        used.extend(page.help());
        for &item in page.items() {
            used.push(item.help());
            used.extend(item.fields(&Settings::default()).into_iter().map(Item::help));
        }
    }
    for pick in [Pick::MountPath, Pick::SoundFont, Pick::Mt32Roms, Pick::ImportGame, Pick::ImagePath, Pick::AchievementsArchive] {
        used.push(pick.help());
    }
    for id in &used {
        assert!(help::topic(id).is_some(), "no help on {}", id);
    }
    for topic in help::topics() {
        assert!(used.contains(&topic.id), "nothing shows the help on {}", topic.id);
    }
}

#[test]
fn f1_shows_help_on_the_setting_under_the_cursor() {
    let mut host = FakeHost::new();
    let mut ui = opened(&host);
    let mut frame = Frame::new(640, 350);
    ui.show_page(Page::Sound);
    let midi = ui.items().iter().position(|&i| i == Item::Midi).unwrap();
    ui.select(midi);
    ui.key(UiKey::Help, &mut host);
    assert_eq!(ui.help.as_ref().map(|h| h.topic.id), Some("midi"));
    ui.draw(&mut frame);

    // It scrolls, and takes the keys: Tab doesn't change the page.
    let help = ui.help.as_ref().unwrap();
    assert!(help.max_scroll() > 0, "a long topic scrolls in a small window");
    ui.key(UiKey::Down, &mut host);
    ui.key(UiKey::Tab, &mut host);
    assert_eq!((ui.help.as_ref().unwrap().scroll, ui.page, ui.row), (1, Page::Sound, midi));
    ui.key(UiKey::End, &mut host);
    ui.key(UiKey::Down, &mut host);
    let help = ui.help.as_ref().unwrap();
    let last = help.max_scroll();
    assert_eq!(help.scroll, last);
    ui.wheel(1, &mut host);
    assert_eq!(ui.help.as_ref().unwrap().scroll, last - 1);

    // Esc closes the help alone, and F1 opens and closes it.
    ui.key(UiKey::Esc, &mut host);
    assert!(ui.help.is_none() && ui.is_open());
    ui.key(UiKey::Help, &mut host);
    ui.key(UiKey::Help, &mut host);
    assert!(ui.help.is_none());

    // A click off it closes it; one on it doesn't.
    ui.key(UiKey::Help, &mut host);
    ui.draw(&mut frame);
    let layout = ui.layout.unwrap();
    let at = |col: usize, row: usize| ((layout.x + col * 8 + 4) as i32, (layout.y + row * layout.cell_h + 4) as i32);
    let inside = ui.hits.iter().rev().find(|h| matches!(h.target, Target::Help)).unwrap();
    let (x, y) = at(inside.col + 3, inside.row);
    ui.click(x, y, &mut host);
    assert!(ui.help.is_some());
    ui.click(layout.x as i32 + 4, layout.y as i32 + 4, &mut host);
    assert!(ui.help.is_none());
    assert_eq!(host.applied.len(), 0, "nothing changed underneath");
}

#[test]
fn f1_shows_help_on_pages_and_dialogs() {
    let mut host = FakeHost::new();
    let mut ui = opened(&host);
    let mut frame = Frame::new(640, 400);
    let mut topic = |ui: &mut ConfigUi, host: &mut FakeHost| {
        ui.key(UiKey::Help, host);
        ui.draw(&mut frame);
        let id = ui.help.as_ref().map(|h| h.topic.id);
        ui.key(UiKey::Esc, host);
        id
    };
    ui.show_page(Page::Drives);
    assert_eq!(topic(&mut ui, &mut host), Some("drives"));
    ui.key(UiKey::Insert, &mut host);
    assert!(ui.dialog.is_some());
    assert_eq!(topic(&mut ui, &mut host), Some("mount"));
    assert!(ui.dialog.is_some(), "Esc closed the help, not the dialog");
    ui.key(UiKey::Esc, &mut host);
    ui.show_page(Page::Stats);
    assert_eq!(topic(&mut ui, &mut host), Some("stats"));
    ui.show_page(Page::Display);
    assert_eq!(topic(&mut ui, &mut host), Some("scale"));
}

#[test]
fn the_help_covers_the_pictures_and_graphs_drawn_in_pixels() {
    let mut host = FakeHost::new();
    host.save_state(1).unwrap();
    host.states[0].picture.as_mut().unwrap().rgb.fill(0xFF);
    let mut ui = opened(&host);
    ui.show_page(Page::States);
    let mut frame = Frame::new(640, 400);
    ui.draw(&mut frame);
    // The picture's first pixel: text is never 16 white pixels in a row.
    let white = frame.rgb.windows(48).step_by(3).position(|run| run.iter().all(|&b| b == 0xFF)).expect("the slot's picture");
    ui.key(UiKey::Help, &mut host);
    let mut frame = Frame::new(640, 400);
    ui.draw(&mut frame);
    assert_eq!(&frame.rgb[white * 3..][..3], [draw::FIELD.0, draw::FIELD.1, draw::FIELD.2], "the help's background");
    // Below the help, the picture shows.
    assert!(frame.rgb.chunks(3).skip(white).any(|px| px == [0xFF; 3]));
}
