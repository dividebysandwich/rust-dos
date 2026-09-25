use super::*;
use crate::disk::MountOptions;
use crate::games::{GameEntry, NewGame};
use crate::video::shader::Shader;

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
    saved: Vec<Settings>,
    /// The drives `choose_image` was asked for.
    images: Vec<Option<u8>>,
    /// Whether CRT shaders are refused, as without OpenGL 3.
    no_shaders: bool,
    /// The game profiles, and the games launched and made.
    games: Vec<GameEntry>,
    launched: Vec<String>,
    created: Vec<NewGame>,
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
            saved: vec![],
            images: vec![],
            no_shaders: false,
            games: vec![],
            launched: vec![],
            created: vec![],
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
    ui.row = Page::Emulator.items().iter().position(|&i| i == Item::Memsize).unwrap();
    keys(&mut ui, &mut host, &[Right]);
    assert_eq!(host.applied.last().unwrap().memsize, 32);
    assert!(status(&ui).0.contains("next time"), "{:?}", status(&ui));

    // The disk speed changes at once.
    ui.row = ui.items().iter().position(|&i| i == Item::FloppyDiskSpeed).unwrap();
    keys(&mut ui, &mut host, &[Left]);
    assert_eq!(host.applied.last().unwrap().disk.floppy_disk_speed, crate::diskio::DiskSpeed::Slow);
    assert!(status(&ui).0.is_empty() || !status(&ui).0.contains("next time"), "{:?}", status(&ui));
    assert_eq!(ui.item().map(|i| i.value(&ui.settings, None)).as_deref(), Some("slow (~30 kB/s)"));
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
fn the_crt_shader_steps_through_the_looks() {
    let mut host = FakeHost::new();
    let mut ui = opened(&host);
    use UiKey::*;
    ui.show_page(Page::Display);
    ui.row = ui.items().iter().position(|&i| i == Item::Shader).unwrap();
    keys(&mut ui, &mut host, &[Right, Right, Right, Right]);
    let looks: Vec<Shader> = host.applied.iter().map(|s| s.shader).collect();
    assert_eq!(looks, [Shader::Scanlines, Shader::Aperture, Shader::Curved, Shader::None]);
    keys(&mut ui, &mut host, &[Left]);
    assert_eq!(host.applied.last().unwrap().shader, Shader::Curved);
    assert_eq!(ui.item().map(|i| i.value(&ui.settings, None)).as_deref(), Some("curved CRT"));

    // Without shaders the window says so, and keeps the setting to save.
    host.no_shaders = true;
    keys(&mut ui, &mut host, &[Left]);
    assert_eq!(status(&ui), ("CRT shaders need OpenGL 3", true));
    assert_eq!(ui.settings.shader, Shader::Aperture);
    keys(&mut ui, &mut host, &[Save]);
    assert_eq!(host.saved.last().unwrap().shader, Shader::Aperture);
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
    keys(&mut ui, &mut host, &[Right, Right, Right, Right, Left]);
    let phosphors: Vec<Monochrome> = host.applied.iter().map(|s| s.monochrome).collect();
    use Monochrome::*;
    assert_eq!(phosphors, [White, Amber, Green, Off, Green]);
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
    keys(&mut ui, &mut host, &[Right]);
    assert!(!host.applied.last().unwrap().mixer.speaker_filter);
    keys(&mut ui, &mut host, &[Down, Right]);
    assert_eq!(host.applied.last().unwrap().mixer.sb_filter, SbFilter::Off);
    keys(&mut ui, &mut host, &[Down, Right, Right, Right]);
    assert_eq!(host.applied.last().unwrap().mixer.reverb, ReverbPreset::Medium);
    keys(&mut ui, &mut host, &[Down, Left]);
    assert_eq!(host.applied.last().unwrap().mixer.chorus, ChorusPreset::Strong);
    assert_eq!(Item::Reverb.applies(), Applies::Now);
    ui.draw(&mut Frame::new(640, 400));
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
    for adapter in [Adapter::Vga, Adapter::Ega, Adapter::Cga, Adapter::Hercules, Adapter::Svga] {
        keys(&mut ui, &mut host, &[UiKey::Right]);
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

    // The deadzone in fives, typed, and back to 10 with Delete.
    keys(&mut ui, &mut host, &[Down, Right]);
    assert_eq!(host.applied.last().unwrap().joystick.deadzone, 15);
    keys(&mut ui, &mut host, &[Enter, End, Backspace, Backspace]);
    ui.text("12", &mut host);
    ui.key(Enter, &mut host);
    assert_eq!(host.applied.last().unwrap().joystick.deadzone, 12);
    keys(&mut ui, &mut host, &[Left]);
    assert_eq!(ui.item().map(|i| i.value(&ui.settings, None)).as_deref(), Some("10%"));
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
    keys(&mut ui, &mut host, &[UiKey::Right]);
    assert!(!host.applied.last().unwrap().ems);
    keys(&mut ui, &mut host, &[UiKey::Down, UiKey::Right]);
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
    keys(&mut ui, &mut host, &[UiKey::Right, UiKey::Right]);
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
    assert_eq!(ui.items(), [Item::Aspect, Item::Filter, Item::Shader, Item::Monochrome]);
    keys(&mut ui, &mut host, &[Right]);
    assert!(host.applied.last().unwrap().aspect);
    ui.show_page(Page::Sound);
    assert!(!ui.items().contains(&Item::SoundFont));
    ui.row = ui.items().iter().position(|&i| i == Item::Midi).unwrap();
    keys(&mut ui, &mut host, &[Right, Right, Right]);
    let synths: Vec<MidiSynth> = host.applied.iter().rev().take(3).map(|s| s.sound.midisynth).collect();
    assert_eq!(synths, [MidiSynth::Auto, MidiSynth::None, MidiSynth::Gus]);

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
    assert_eq!(ui.row_count(), 2, "the game and the new one");
    ui.draw(&mut Frame::new(640, 400));

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
            assert!(ui.hits.iter().filter(|h| h.row == 1).all(|h| h.col + h.width < ui.layout.unwrap().cols));
        }
    }
}
