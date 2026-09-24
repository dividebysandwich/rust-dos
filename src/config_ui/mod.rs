//! The settings window: a semi-transparent panel over the picture, opened
//! with Ctrl+F12 or the DOSCONFIG command. It changes the settings, mounts,
//! swaps and unmounts drives, and saves both to the configuration file.
//!
//! It knows nothing of SDL, the browser or the machine: the frontend feeds
//! it keys, typed text and clicks, and carries out what it asks for through
//! `Host`. What the frontend doesn't have (`Frontend`) isn't offered.

mod browser;
mod dialog;
mod draw;

use browser::{Browser, IMAGES, Row, SOUNDFONTS};
use dialog::{Event, Field, MountDialog, TextField};
use draw::{Grid, Layout, Rgb};
pub use draw::cp437;

use crate::config::{MidiSynth, Settings};
use crate::cpu::CpuModel;
use crate::disk::{DRIVE_C, DriveInfo, DriveKind, drive_letter};
use crate::diskio::{DiskClass, DiskSpeed, NoiseMode};
use crate::mount::{MountSpec, contract_home, expand_host_path};
use crate::sb::SbModel;
use crate::timer::CpuSpeed;
use crate::video::Frame;
use std::path::{Path, PathBuf};

/// A key the window acts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiKey {
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
    Enter,
    Esc,
    Tab,
    BackTab,
    Backspace,
    Delete,
    Insert,
    /// F2 or Ctrl+S.
    Save,
    Char(char),
}

/// What the frontend has that some of the settings need.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frontend {
    /// A window of its own, which can be scaled and made fullscreen.
    pub window: bool,
    /// The host's files: drives are mounted from its directories and
    /// images, and a SoundFont is picked from them. Without them (in the
    /// browser), the frontend picks disk images itself (`Host::choose_image`).
    pub host_files: bool,
}

impl Frontend {
    /// The rust-dos program.
    pub const DESKTOP: Frontend = Frontend { window: true, host_files: true };
}

/// What the window needs the emulator to do.
pub trait Host {
    /// Take on changed settings. Returns a note on when they take effect,
    /// or a problem.
    fn apply(&mut self, settings: &Settings) -> Result<Option<String>, String>;
    fn mount(&mut self, spec: MountSpec, replace: bool) -> Result<PathBuf, String>;
    fn unmount(&mut self, drive: u8) -> Result<(), String>;
    fn drives(&self) -> Vec<DriveInfo>;
    /// Save the settings and the drives to the configuration file.
    fn save(&mut self, settings: &Settings) -> Result<(), String>;
    /// Without the host's files: have the user pick a disk or CD image for
    /// `drive`, or for whichever drive suits it (None). The frontend mounts
    /// it once it is picked, and tells the window (`drives_changed`).
    fn choose_image(&mut self, drive: Option<u8>) -> Result<(), String> {
        let _ = drive;
        Err("Disk images are mounted from the host's files here".to_string())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Drives,
    Display,
    Emulator,
    Sound,
}

const PAGES: [Page; 4] = [Page::Drives, Page::Display, Page::Emulator, Page::Sound];

impl Page {
    fn title(self) -> &'static str {
        match self {
            Page::Drives => "Drives",
            Page::Display => "Display",
            Page::Emulator => "Emulator",
            Page::Sound => "Sound",
        }
    }

    fn items(self) -> &'static [Item] {
        use Item::*;
        match self {
            Page::Drives => &[],
            Page::Display => &[Scale, Fullscreen, Aspect, Filter, Shader, Monochrome],
            Page::Emulator => &[Cycles, Cpu, Memsize, HardDiskSpeed, FloppyDiskSpeed],
            Page::Sound => &[
                SbType, SbBase, SbIrq, SbDma, SbHdma, Opl, Gus, GusBase, GusIrq, GusDma, GusDrive, UltraDir, Midi,
                SoundFont, HardDiskNoise, FloppyDiskNoise,
            ],
        }
    }
}

/// When a changed setting takes effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Applies {
    Now,
    /// Once no program runs: changing the hardware under one would break it.
    AtPrompt,
    NextStart,
}

/// How a setting is changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Input {
    /// Left and right step through the values.
    Choice,
    /// Steps, and Enter types a value.
    ChoiceOrText,
    Text,
    /// Enter picks a host file.
    File,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Item {
    Scale,
    Fullscreen,
    Aspect,
    Filter,
    Shader,
    Monochrome,
    Cycles,
    Cpu,
    Memsize,
    SbType,
    SbBase,
    SbIrq,
    SbDma,
    SbHdma,
    Opl,
    Gus,
    GusBase,
    GusIrq,
    GusDma,
    GusDrive,
    UltraDir,
    Midi,
    SoundFont,
    HardDiskSpeed,
    FloppyDiskSpeed,
    HardDiskNoise,
    FloppyDiskNoise,
}

/// The value `dir` steps away from `current` in `values`, wrapping around.
fn cycle<T: PartialEq + Copy>(values: &[T], current: T, dir: isize) -> T {
    let at = match values.iter().position(|&v| v == current) {
        Some(i) => i as isize,
        None if dir > 0 => -1,
        None => 0,
    };
    values[(at + dir).rem_euclid(values.len() as isize) as usize]
}

/// The next of the ascending `values` above (or below) `current`.
fn step_number(values: &[u32], current: u32, dir: isize) -> u32 {
    if dir > 0 {
        values.iter().copied().find(|&v| v > current).unwrap_or(current)
    } else {
        values.iter().rev().copied().find(|&v| v < current).unwrap_or(current)
    }
}

const CYCLES: [u32; 8] = [1000, 3000, 5000, 10_000, 20_000, 50_000, 100_000, u32::MAX];
const MEMSIZES: [u32; 6] = [2, 4, 8, 16, 32, 64];

fn on_off(on: bool) -> String {
    if on { "on" } else { "off" }.to_string()
}

/// Whether General MIDI can play through a SoundFont: one picked from the
/// host's files, with the synthesizer built in.
fn soundfonts(frontend: Frontend) -> bool {
    cfg!(feature = "midi") && frontend.host_files
}

impl Item {
    fn label(self) -> &'static str {
        use Item::*;
        match self {
            Scale => "Window scale",
            Fullscreen => "Fullscreen",
            Aspect => "4:3 aspect correction",
            Filter => "Scaling filter",
            Shader => "CRT shader",
            Monochrome => "Monochrome monitor",
            Cycles => "CPU speed (cycles)",
            Cpu => "Processor",
            Memsize => "Memory",
            SbType => "Sound Blaster",
            SbBase => "  Base port",
            SbIrq => "  IRQ",
            SbDma => "  DMA",
            SbHdma => "  High DMA (SB16)",
            Opl => "FM synthesizer",
            Gus => "Gravis Ultrasound",
            GusBase => "  Base port",
            GusIrq => "  IRQ",
            GusDma => "  DMA",
            GusDrive => "  Software drive",
            UltraDir => "  ULTRADIR",
            Midi => "MIDI synthesizer",
            SoundFont => "SoundFont",
            HardDiskSpeed => "Hard disk speed",
            FloppyDiskSpeed => "Floppy disk speed",
            HardDiskNoise => "Hard disk noise",
            FloppyDiskNoise => "Floppy disk noise",
        }
    }

    /// Whether the frontend has what the setting needs.
    fn available(self, frontend: Frontend) -> bool {
        match self {
            Item::Scale | Item::Fullscreen => frontend.window,
            Item::SoundFont => soundfonts(frontend),
            _ => true,
        }
    }

    fn applies(self) -> Applies {
        use Item::*;
        match self {
            Scale | Fullscreen | Aspect | Filter | Shader | Monochrome | Cycles => Applies::Now,
            HardDiskSpeed | FloppyDiskSpeed | HardDiskNoise | FloppyDiskNoise => Applies::Now,
            Memsize => Applies::NextStart,
            _ => Applies::AtPrompt,
        }
    }

    fn input(self) -> Input {
        match self {
            Item::Cycles => Input::ChoiceOrText,
            Item::UltraDir => Input::Text,
            Item::SoundFont => Input::File,
            _ => Input::Choice,
        }
    }

    fn value(self, s: &Settings, home: Option<&Path>) -> String {
        use Item::*;
        let (sb, gus) = (&s.sound.sb, &s.sound.gus);
        match self {
            Scale => format!("{}x", s.scale),
            Fullscreen => on_off(s.fullscreen),
            Aspect => on_off(s.aspect),
            Filter => match s.filter {
                crate::config::Filter::Nearest => "nearest (sharp)",
                crate::config::Filter::Linear => "linear (smooth)",
            }
            .to_string(),
            Shader => s.shader.describe().to_string(),
            Monochrome => s.monochrome.describe().to_string(),
            Cycles => match s.cycles {
                CpuSpeed::Max => "max".to_string(),
                CpuSpeed::Fixed(n) => format!("{} per ms", n),
            },
            Cpu => match s.cpu {
                CpuModel::I386 => "386",
                CpuModel::I486 => "486",
            }
            .to_string(),
            Memsize => format!("{} MB", s.memsize),
            SbType if !s.sound.sb_installed => "none".to_string(),
            SbType => match sb.model {
                SbModel::Sb16 => "SB16",
                SbModel::SbPro2 => "SB Pro 2",
                SbModel::Sb2 => "SB 2.0",
            }
            .to_string(),
            SbBase => format!("{:X}h", sb.base),
            SbIrq => sb.irq.to_string(),
            SbDma => sb.dma8.to_string(),
            SbHdma => sb.dma16.to_string(),
            Opl => if s.sound.opl3 { "OPL3" } else { "OPL2" }.to_string(),
            Gus => on_off(gus.enabled),
            GusBase => format!("{:X}h", gus.base),
            GusIrq => gus.irq.to_string(),
            GusDma => gus.dma.to_string(),
            GusDrive => gus.drive.map_or("none".to_string(), |d| format!("{}:", drive_letter(d))),
            UltraDir => match &gus.ultradir {
                Some(dir) => dir.clone(),
                None => format!("{} (default)", gus.ultradir()),
            },
            Midi => match s.sound.midisynth {
                MidiSynth::Auto => "auto",
                MidiSynth::SoundFont => "SoundFont",
                MidiSynth::Gus => "Ultrasound patches",
                MidiSynth::None => "none",
            }
            .to_string(),
            SoundFont => s.sound.soundfont.as_deref().map_or("none".to_string(), |p| contract_home(p, home)),
            HardDiskSpeed => s.disk.hard_disk_speed.describe(DiskClass::HardDisk),
            FloppyDiskSpeed => s.disk.floppy_disk_speed.describe(DiskClass::Floppy),
            HardDiskNoise => s.disk.hard_disk_noise.name().to_string(),
            FloppyDiskNoise => s.disk.floppy_disk_noise.name().to_string(),
        }
    }

    /// Step the setting left (-1) or right (1). `drives` are the mounted
    /// drives, which the Ultrasound's drive can't take.
    fn step(self, s: &mut Settings, dir: isize, drives: &[DriveInfo], frontend: Frontend) {
        use Item::*;
        let sound = &mut s.sound;
        let (sb, gus) = (&mut sound.sb, &mut sound.gus);
        match self {
            Scale => s.scale = (s.scale as isize + dir).clamp(1, 16) as u32,
            Fullscreen => s.fullscreen = !s.fullscreen,
            Aspect => s.aspect = !s.aspect,
            Filter => s.filter = cycle(&[crate::config::Filter::Nearest, crate::config::Filter::Linear], s.filter, dir),
            Shader => s.shader = cycle(&crate::video::shader::Shader::ALL, s.shader, dir),
            Monochrome => s.monochrome = cycle(&crate::video::mono::Monochrome::ALL, s.monochrome, dir),
            Cycles => {
                let current = match s.cycles {
                    CpuSpeed::Max => u32::MAX,
                    CpuSpeed::Fixed(n) => n,
                };
                s.cycles = match step_number(&CYCLES, current, dir) {
                    u32::MAX => CpuSpeed::Max,
                    n => CpuSpeed::Fixed(n),
                };
            }
            Cpu => s.cpu = cycle(&[CpuModel::I386, CpuModel::I486], s.cpu, dir),
            Memsize => s.memsize = step_number(&MEMSIZES, s.memsize as u32, dir) as usize,
            SbType => {
                let models = [Some(SbModel::Sb16), Some(SbModel::SbPro2), Some(SbModel::Sb2), None];
                match cycle(&models, sound.sb_installed.then_some(sb.model), dir) {
                    Some(model) => {
                        sb.model = model;
                        sound.sb_installed = true;
                    }
                    None => sound.sb_installed = false,
                }
            }
            SbBase => sb.base = cycle(&[0x210, 0x220, 0x230, 0x240, 0x250, 0x260, 0x270, 0x280], sb.base, dir),
            SbIrq => sb.irq = cycle(&[2, 3, 5, 7, 9, 10, 11, 12, 15], sb.irq, dir),
            SbDma => sb.dma8 = cycle(&[0, 1, 3], sb.dma8, dir),
            SbHdma => sb.dma16 = cycle(&[5, 6, 7], sb.dma16, dir),
            Opl => sound.opl3 = !sound.opl3,
            Gus => gus.enabled = !gus.enabled,
            GusBase => gus.base = cycle(&[0x210, 0x220, 0x240, 0x250, 0x260], gus.base, dir),
            GusIrq => gus.irq = cycle(&[2, 3, 5, 7, 11, 12, 15], gus.irq, dir),
            GusDma => gus.dma = cycle(&[1, 3, 5, 6, 7], gus.dma, dir),
            GusDrive => {
                // D: to Y:, where no drive of the user's own is.
                let mut letters = vec![None];
                letters.extend(
                    (3..25u8)
                        .filter(|&d| drives.iter().all(|i| i.drive != d || i.kind == DriveKind::Virtual))
                        .map(Some),
                );
                gus.drive = cycle(&letters, gus.drive, dir);
            }
            Midi => {
                let synths: &[MidiSynth] = if soundfonts(frontend) {
                    &[MidiSynth::Auto, MidiSynth::SoundFont, MidiSynth::Gus, MidiSynth::None]
                } else {
                    &[MidiSynth::Auto, MidiSynth::Gus, MidiSynth::None]
                };
                sound.midisynth = cycle(synths, sound.midisynth, dir);
            }
            HardDiskSpeed => s.disk.hard_disk_speed = cycle(&DiskSpeed::ALL, s.disk.hard_disk_speed, dir),
            FloppyDiskSpeed => s.disk.floppy_disk_speed = cycle(&DiskSpeed::ALL, s.disk.floppy_disk_speed, dir),
            HardDiskNoise => s.disk.hard_disk_noise = cycle(&NoiseMode::ALL, s.disk.hard_disk_noise, dir),
            FloppyDiskNoise => s.disk.floppy_disk_noise = cycle(&NoiseMode::ALL, s.disk.floppy_disk_noise, dir),
            UltraDir | SoundFont => {}
        }
    }

    /// The text to edit.
    fn text(self, s: &Settings) -> String {
        match self {
            Item::Cycles => match s.cycles {
                CpuSpeed::Max => "max".to_string(),
                CpuSpeed::Fixed(n) => n.to_string(),
            },
            Item::UltraDir => s.sound.gus.ultradir.clone().unwrap_or_default(),
            _ => String::new(),
        }
    }

    fn set_text(self, s: &mut Settings, text: &str) -> Result<(), String> {
        let text = text.trim();
        match self {
            Item::Cycles => s.cycles = CpuSpeed::parse(text)?,
            Item::UltraDir => s.sound.gus.ultradir = (!text.is_empty()).then(|| text.to_string()),
            _ => {}
        }
        Ok(())
    }

    /// Delete: back to the default. Returns whether that changed anything.
    fn clear(self, s: &mut Settings) -> bool {
        match self {
            Item::UltraDir => s.sound.gus.ultradir.take().is_some(),
            Item::SoundFont => s.sound.soundfont.take().is_some(),
            _ => false,
        }
    }
}

/// What a file browser is picking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pick {
    MountPath,
    SoundFont,
}

struct Status {
    text: String,
    error: bool,
}

/// Something on the window a click acts on.
#[derive(Clone, Copy, Debug)]
enum Target {
    Tab(Page),
    /// A row of the page's list (absolute, not scrolled).
    Row(usize),
    /// The ◄ or ► of a setting.
    Step(usize, isize),
    Key(UiKey),
    Field(Field),
    BrowserRow(usize),
}

struct Hit {
    row: usize,
    col: usize,
    width: usize,
    target: Target,
}

pub struct ConfigUi {
    frontend: Frontend,
    open: bool,
    page: Page,
    /// Selected row of the page and the first one shown.
    row: usize,
    scroll: usize,
    settings: Settings,
    drives: Vec<DriveInfo>,
    config_file: Option<PathBuf>,
    home: Option<PathBuf>,
    status: Option<Status>,
    /// The selected setting's value being typed.
    edit: Option<TextField>,
    dialog: Option<MountDialog>,
    browser: Option<(Browser, Pick)>,
    browser_scroll: usize,
    /// Where the last frame put the panel, and what can be clicked on it.
    layout: Option<Layout>,
    hits: Vec<Hit>,
    /// Rows of the list the last frame showed, for Page Up and Down.
    visible: usize,
}

impl Default for ConfigUi {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfigUi {
    /// The rust-dos program's window.
    pub fn new() -> Self {
        Self::for_frontend(Frontend::DESKTOP)
    }

    pub fn for_frontend(frontend: Frontend) -> Self {
        Self {
            frontend,
            open: false,
            page: Page::Drives,
            row: 0,
            scroll: 0,
            settings: Settings::default(),
            drives: Vec::new(),
            config_file: None,
            home: None,
            status: None,
            edit: None,
            dialog: None,
            browser: None,
            browser_scroll: 0,
            layout: None,
            hits: Vec::new(),
            visible: 10,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Open on the current settings and drives. `config_file` is where
    /// Save writes, if there is a file.
    pub fn open(&mut self, settings: &Settings, config_file: Option<PathBuf>, host: &dyn Host) {
        self.open = true;
        self.settings = settings.clone();
        self.drives = host.drives();
        self.config_file = config_file;
        self.home = dirs::home_dir();
        self.status = None;
        self.edit = None;
        self.dialog = None;
        self.browser = None;
        self.row = self.row.min(self.row_count().saturating_sub(1));
    }

    pub fn close(&mut self) {
        self.open = false;
        self.layout = None;
        self.hits.clear();
    }

    fn row_count(&self) -> usize {
        match self.page {
            Page::Drives => self.drives.len() + 1,
            _ => self.items().len(),
        }
    }

    /// The page's settings that the frontend has.
    fn items(&self) -> Vec<Item> {
        self.page.items().iter().copied().filter(|item| item.available(self.frontend)).collect()
    }

    fn item(&self) -> Option<Item> {
        self.items().get(self.row).copied()
    }

    fn info(&mut self, text: impl Into<String>) {
        self.status = Some(Status { text: text.into(), error: false });
    }

    fn error(&mut self, text: impl Into<String>) {
        self.status = Some(Status { text: text.into(), error: true });
    }

    /// Typed text, from the keyboard's text input.
    pub fn text(&mut self, text: &str, host: &mut dyn Host) {
        for c in text.chars().filter(|c| !c.is_control()) {
            self.key(UiKey::Char(c), host);
        }
    }

    pub fn key(&mut self, key: UiKey, host: &mut dyn Host) {
        if !self.open {
            return;
        }
        if self.browser.is_some() {
            self.browser_key(key, host);
        } else if self.dialog.is_some() {
            self.dialog_key(key, host);
        } else if self.edit.is_some() {
            self.edit_key(key, host);
        } else {
            self.page_key(key, host);
        }
    }

    /// Mouse wheel: `dy` > 0 is up.
    pub fn wheel(&mut self, dy: i32, host: &mut dyn Host) {
        let key = if dy > 0 { UiKey::Up } else { UiKey::Down };
        for _ in 0..dy.unsigned_abs().min(5) {
            self.key(key, host);
        }
    }

    /// A left click at frame pixel (`x`, `y`).
    pub fn click(&mut self, x: i32, y: i32, host: &mut dyn Host) {
        let Some((col, row)) = self.layout.and_then(|l| l.cell_at(x, y)) else { return };
        let Some(target) = self
            .hits
            .iter()
            .rev()
            .find(|h| h.row == row && (h.col..h.col + h.width).contains(&col))
            .map(|h| h.target)
        else {
            return;
        };
        match target {
            Target::Tab(page) => {
                if self.dialog.is_none() && self.browser.is_none() {
                    self.edit = None;
                    self.show_page(page);
                }
            }
            Target::Row(i) if i == self.row => self.key(UiKey::Enter, host),
            Target::Row(i) => {
                self.edit = None;
                self.row = i;
            }
            Target::Step(i, dir) => {
                self.edit = None;
                self.row = i;
                self.key(if dir < 0 { UiKey::Left } else { UiKey::Right }, host);
            }
            Target::Key(key) => self.key(key, host),
            Target::Field(field) => {
                if let Some(dialog) = &mut self.dialog {
                    let again = dialog.focus == field;
                    dialog.focus = field;
                    match field {
                        Field::Browse | Field::Mount | Field::Unmount | Field::Cancel => self.key(UiKey::Enter, host),
                        Field::Drive | Field::Kind | Field::ReadOnly if again => dialog.step(field, 1),
                        _ => {}
                    }
                }
            }
            Target::BrowserRow(i) => {
                if let Some((browser, _)) = &mut self.browser {
                    if browser.selected == i {
                        self.key(UiKey::Enter, host);
                    } else {
                        browser.select(i);
                    }
                }
            }
        }
    }

    fn show_page(&mut self, page: Page) {
        if page != self.page {
            self.page = page;
            self.row = 0;
            self.scroll = 0;
        }
    }

    /// Up, Down, Page Up and Down, Home and End on a list of `rows` rows.
    fn navigate(key: UiKey, selected: usize, rows: usize, page: usize) -> Option<usize> {
        let last = rows.saturating_sub(1);
        Some(match key {
            UiKey::Up => selected.saturating_sub(1),
            UiKey::Down => (selected + 1).min(last),
            UiKey::PageUp => selected.saturating_sub(page.max(1)),
            UiKey::PageDown => (selected + page.max(1)).min(last),
            UiKey::Home => 0,
            UiKey::End => last,
            _ => return None,
        })
    }

    fn page_key(&mut self, key: UiKey, host: &mut dyn Host) {
        if let Some(row) = Self::navigate(key, self.row, self.row_count(), self.visible) {
            self.row = row;
            return;
        }
        let at = PAGES.iter().position(|&p| p == self.page).unwrap_or(0);
        match key {
            UiKey::Esc => self.close(),
            UiKey::Tab => self.show_page(PAGES[(at + 1) % PAGES.len()]),
            UiKey::BackTab => self.show_page(PAGES[(at + PAGES.len() - 1) % PAGES.len()]),
            UiKey::Save => self.save(host),
            _ if self.page == Page::Drives => self.drives_key(key, host),
            _ => self.setting_key(key, host),
        }
    }

    fn save(&mut self, host: &mut dyn Host) {
        let Some(path) = &self.config_file else {
            self.error("No configuration file to save to (rust-dos started with --no-config)");
            return;
        };
        let shown = contract_home(path, self.home.as_deref());
        match host.save(&self.settings) {
            Ok(()) => self.info(format!("Saved to {}", shown)),
            Err(e) => self.error(e),
        }
    }

    fn setting_key(&mut self, key: UiKey, host: &mut dyn Host) {
        let Some(item) = self.item() else { return };
        match (key, item.input()) {
            (UiKey::Left, Input::Choice | Input::ChoiceOrText) => self.step(item, -1, host),
            (UiKey::Right | UiKey::Enter, Input::Choice) | (UiKey::Right, Input::ChoiceOrText) => {
                self.step(item, 1, host)
            }
            (UiKey::Enter, Input::ChoiceOrText | Input::Text) => {
                self.edit = Some(TextField::new(&item.text(&self.settings)));
            }
            (UiKey::Enter, Input::File) => self.open_browser(Pick::SoundFont),
            (UiKey::Delete | UiKey::Backspace, _) if item.clear(&mut self.settings) => self.changed(item, host),
            _ => {}
        }
    }

    fn step(&mut self, item: Item, dir: isize, host: &mut dyn Host) {
        item.step(&mut self.settings, dir, &self.drives, self.frontend);
        self.changed(item, host);
    }

    /// Hand changed settings to the emulator and say when they take effect.
    fn changed(&mut self, item: Item, host: &mut dyn Host) {
        let result = host.apply(&self.settings);
        self.status = None;
        match result {
            Err(problem) => self.error(problem),
            Ok(Some(note)) => self.info(note),
            Ok(None) if item.applies() == Applies::NextStart => {
                self.info("Takes effect the next time rust-dos starts (F2 saves it)")
            }
            Ok(None) => {}
        }
        // Conflicts between the cards, as the configuration file reports them.
        if self.page == Page::Sound
            && let Some(problem) = self.settings.sound.clone().check().into_iter().next()
        {
            self.error(problem.trim_start_matches("[sound]: ").to_string());
        }
    }

    fn edit_key(&mut self, key: UiKey, host: &mut dyn Host) {
        let item = self.item();
        let (Some(field), Some(item)) = (&mut self.edit, item) else {
            self.edit = None;
            return;
        };
        if field.key(key) {
            return;
        }
        match key {
            UiKey::Esc => self.edit = None,
            UiKey::Enter => {
                let text = field.text();
                let mut settings = self.settings.clone();
                match item.set_text(&mut settings, &text) {
                    Ok(()) => {
                        self.edit = None;
                        if settings != self.settings {
                            self.settings = settings;
                            self.changed(item, host);
                        }
                    }
                    Err(e) => self.error(e),
                }
            }
            _ => {}
        }
    }

    fn drives_key(&mut self, key: UiKey, host: &mut dyn Host) {
        let selected = self.drives.get(self.row).cloned();
        match (key, selected) {
            (UiKey::Insert, _) | (UiKey::Enter, None) => self.new_drive(host),
            (UiKey::Enter, Some(info)) if info.kind == DriveKind::Virtual => {
                self.info(format!("Drive {}: is built into rust-dos", info.letter()));
            }
            (UiKey::Enter, Some(info)) if !self.frontend.host_files => self.choose_image(Some(info.drive), host),
            (UiKey::Enter, Some(info)) => {
                self.status = None;
                self.dialog = Some(MountDialog::change(&info, self.home.as_deref()));
            }
            (UiKey::Delete, Some(info)) if info.kind == DriveKind::Virtual || info.drive == DRIVE_C => {
                self.error(format!("Drive {}: can't be unmounted", info.letter()));
            }
            (UiKey::Delete, Some(info)) => self.unmount(info.drive, host),
            _ => {}
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

    fn refresh_drives(&mut self, host: &dyn Host, select: Option<u8>) {
        self.drives = host.drives();
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
                self.info(format!("Drive {}: has been unmounted", drive_letter(drive)));
            }
            Err(e) => self.error(e),
        }
    }

    fn dialog_key(&mut self, key: UiKey, host: &mut dyn Host) {
        let Some(dialog) = &mut self.dialog else { return };
        match dialog.key(key) {
            Event::None => {}
            Event::Cancel => {
                self.dialog = None;
                self.status = None;
            }
            Event::Browse => self.open_browser(Pick::MountPath),
            Event::Unmount => {
                let drive = dialog.drive;
                self.unmount(drive, host);
            }
            Event::Submit => {
                let cwd = std::env::current_dir().unwrap_or_default();
                let replace = dialog.existing;
                let spec = match dialog.spec(&cwd, self.home.as_deref()) {
                    Ok(spec) => spec,
                    Err(e) => return self.error(e),
                };
                let drive = spec.drive;
                match host.mount(spec, replace) {
                    Ok(path) => {
                        self.dialog = None;
                        self.refresh_drives(host, Some(drive));
                        let kind = self.drives.get(self.row).map_or("", |d| d.kind.name());
                        let shown = contract_home(&path, self.home.as_deref());
                        self.info(format!("Drive {}: is mounted as {} {}", drive_letter(drive), kind, shown));
                    }
                    Err(e) => self.error(e),
                }
            }
        }
    }

    fn open_browser(&mut self, pick: Pick) {
        let cwd = std::env::current_dir().unwrap_or_default();
        let home = self.home.as_deref();
        let (title, current, pick_dirs, extensions) = match pick {
            Pick::MountPath => (
                "Pick a directory or disk image",
                self.dialog.as_ref().map(|d| d.path.text()).unwrap_or_default(),
                true,
                IMAGES,
            ),
            Pick::SoundFont => (
                "Pick a SoundFont (.sf2)",
                self.settings.sound.soundfont.as_deref().map(|p| p.display().to_string()).unwrap_or_default(),
                false,
                SOUNDFONTS,
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

    fn browser_key(&mut self, key: UiKey, host: &mut dyn Host) {
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
                    Pick::SoundFont => {
                        self.settings.sound.soundfont = Some(path);
                        self.changed(Item::SoundFont, host);
                    }
                }
            }
            Ok(None) => self.status = None,
            Err(e) => self.error(e),
        }
    }

    // ----- drawing ----------------------------------------------------------

    /// Draw the window over `frame`, if it is open.
    pub fn draw(&mut self, frame: &mut Frame) {
        if !self.open {
            return;
        }
        let layout = Layout::for_frame(frame.width as usize, frame.height as usize);
        let (cols, rows) = (layout.cols, layout.rows);
        if cols < 20 || rows < 8 {
            return;
        }
        let mut g = Grid::new(cols, rows);
        self.hits.clear();

        // Frame, title, tabs and separators.
        g.line(0, 0xC9, 0xCD, 0xBB, draw::BORDER);
        let title = " rust-dos settings ";
        g.text((cols - title.len()) / 2, 0, title, draw::BRIGHT);
        for row in 1..rows - 1 {
            g.char(0, row, 0xBA, draw::BORDER);
            g.char(cols - 1, row, 0xBA, draw::BORDER);
        }
        g.line(2, 0xC7, 0xC4, 0xB6, draw::BORDER);
        g.line(rows - 4, 0xC7, 0xC4, 0xB6, draw::BORDER);
        g.line(rows - 1, 0xC8, 0xCD, 0xBC, draw::BORDER);
        let mut x = 2;
        for page in PAGES {
            let label = format!(" {} ", page.title());
            let selected = page == self.page;
            if selected {
                g.background(x, 1, label.len(), draw::SELECT);
            }
            let end = g.text_to(x, 1, &label, if selected { draw::BRIGHT } else { draw::TEXT }, cols - 1);
            self.hits.push(Hit { row: 1, col: x, width: end - x, target: Target::Tab(page) });
            x = end + 1;
        }

        // The page, or what is open over it.
        let content = 3..rows - 4;
        self.visible = content.len();
        if self.browser.is_some() {
            self.draw_browser(&mut g, content);
        } else if self.dialog.is_some() {
            self.draw_dialog(&mut g, content);
        } else if self.page == Page::Drives {
            self.draw_drives(&mut g, content);
        } else {
            self.draw_settings(&mut g, content);
        }

        self.draw_hints(&mut g, rows - 3);
        let status_row = rows - 2;
        match &self.status {
            Some(status) => {
                let color = if status.error { draw::ERROR } else { draw::GOOD };
                g.text_to(2, status_row, &fit(&status.text, cols - 4), color, cols - 2);
            }
            None => {
                let text = match &self.config_file {
                    Some(path) => format!("Configuration file: {}", contract_home(path, self.home.as_deref())),
                    None => "No configuration file (--no-config): the settings can't be saved".to_string(),
                };
                g.text_to(2, status_row, &fit(&text, cols - 4), draw::DIM, cols - 2);
            }
        }

        draw::render(&g, &layout, frame);
        self.layout = Some(layout);
    }

    /// Scroll `scroll` so that `selected` is among the `visible` rows.
    fn keep_visible(scroll: &mut usize, selected: usize, visible: usize) {
        if selected < *scroll {
            *scroll = selected;
        } else if visible > 0 && selected >= *scroll + visible {
            *scroll = selected + 1 - visible;
        }
    }

    fn select_row(&self, g: &mut Grid, row: usize) {
        g.background(1, row, g.cols - 2, draw::SELECT);
    }

    fn draw_drives(&mut self, g: &mut Grid, content: std::ops::Range<usize>) {
        let cols = g.cols;
        Self::keep_visible(&mut self.scroll, self.row, content.len());
        let label_col = cols - 16;
        let path_width = label_col.saturating_sub(15);
        for (i, row) in (self.scroll..self.row_count()).zip(content) {
            if i == self.row {
                self.select_row(g, row);
            }
            self.hits.push(Hit { row, col: 1, width: cols - 2, target: Target::Row(i) });
            let Some(info) = self.drives.get(i) else {
                let text = if self.frontend.host_files { "+ Mount a drive..." } else { "+ Insert a disk or CD image..." };
                g.text(2, row, text, draw::KEY);
                continue;
            };
            let builtin = info.kind == DriveKind::Virtual;
            let (fg, dim) = if builtin { (draw::DIM, draw::DIM) } else { (draw::TEXT, draw::BRIGHT) };
            g.text(2, row, &format!("{}:", info.letter()), dim);
            g.text(6, row, info.kind.name(), fg);
            let mut path = match info.image.as_ref().or(info.root.as_ref()) {
                Some(path) => contract_home(path, self.home.as_deref()),
                None => "(built into rust-dos)".to_string(),
            };
            if info.images.len() > 1 {
                path = format!("({}/{}) {}", info.image_index + 1, info.images.len(), path);
            }
            g.text_to(14, row, &fit(&path, path_width), fg, label_col - 1);
            g.text_to(label_col, row, &info.label, fg, cols - 4);
            if info.read_only && !builtin {
                g.text(cols - 4, row, "ro", fg);
            }
        }
    }

    fn draw_settings(&mut self, g: &mut Grid, content: std::ops::Range<usize>) {
        let cols = g.cols;
        let items = self.items();
        Self::keep_visible(&mut self.scroll, self.row, content.len());
        let value_col = 27.min(cols / 2);
        let note_col = cols - 13;
        for (i, row) in (self.scroll..items.len()).zip(content) {
            let item = items[i];
            let selected = i == self.row;
            if selected {
                self.select_row(g, row);
            }
            self.hits.push(Hit { row, col: 1, width: cols - 2, target: Target::Row(i) });
            g.text_to(2, row, item.label(), if selected { draw::BRIGHT } else { draw::TEXT }, value_col - 1);
            let note = match item.applies() {
                Applies::Now => "",
                Applies::AtPrompt => "at prompt",
                Applies::NextStart => "next start",
            };
            g.text(note_col + 1, row, note, draw::NOTE);

            let end = note_col.saturating_sub(1);
            if selected && let Some(field) = &self.edit {
                let width = end.saturating_sub(value_col);
                let (text, cursor) = field.view(width);
                g.background(value_col, row, width, draw::FIELD);
                g.text_to(value_col, row, &text, draw::BRIGHT, end);
                g.background(value_col + cursor, row, 1, draw::SELECT);
                continue;
            }
            let value = fit(&item.value(&self.settings, self.home.as_deref()), end.saturating_sub(value_col + 4));
            if matches!(item.input(), Input::Choice | Input::ChoiceOrText) {
                g.char(value_col, row, 0x11, draw::KEY);
                self.hits.push(Hit { row, col: value_col, width: 1, target: Target::Step(i, -1) });
                let after = g.text_to(value_col + 2, row, &value, draw::BRIGHT, end);
                g.char(after + 1, row, 0x10, draw::KEY);
                self.hits.push(Hit { row, col: after + 1, width: 1, target: Target::Step(i, 1) });
            } else {
                g.text_to(value_col + 2, row, &value, draw::BRIGHT, end);
            }
        }
    }

    fn draw_dialog(&mut self, g: &mut Grid, content: std::ops::Range<usize>) {
        let Some(dialog) = &self.dialog else { return };
        let cols = g.cols;
        let top = content.start;
        let bottom = content.end;
        let label_col = 4;
        let value_col = 16.min(cols / 3);
        let end = cols - 3;
        g.text(2, top, &dialog.title(), draw::BRIGHT);
        let mut hits = Vec::new();
        let mut put = |g: &mut Grid, field: Field, row: usize, label: &str, draw: &dyn Fn(&mut Grid, bool) -> (usize, usize)| {
            if row >= bottom {
                return;
            }
            let focused = dialog.focus == field;
            if !label.is_empty() {
                g.text(label_col, row, label, if focused { draw::BRIGHT } else { draw::TEXT });
            }
            let (col, width) = draw(g, focused);
            hits.push(Hit { row, col, width, target: Target::Field(field) });
        };
        let choice = |text: String, row: usize, editable: bool| {
            move |g: &mut Grid, focused: bool| {
                let width = text.chars().count() + 4;
                if focused {
                    g.background(value_col, row, width, draw::SELECT);
                }
                if editable {
                    g.char(value_col, row, 0x11, draw::KEY);
                    g.char(value_col + width - 1, row, 0x10, draw::KEY);
                }
                g.text(value_col + 2, row, &text, draw::BRIGHT);
                (value_col, width)
            }
        };
        let text_field = |field: &TextField, row: usize, width: usize| {
            let field = field.clone();
            move |g: &mut Grid, focused: bool| {
                let width = width.min(end.saturating_sub(value_col));
                g.background(value_col, row, width, draw::FIELD);
                let (text, cursor) = field.view(width);
                g.text_to(value_col, row, &text, draw::BRIGHT, value_col + width);
                if focused {
                    g.background(value_col + cursor, row, 1, draw::SELECT);
                }
                (value_col, width)
            }
        };
        let letter = format!("{}:", drive_letter(dialog.drive));
        put(g, Field::Drive, top + 1, "Drive", &choice(letter, top + 1, !dialog.existing));
        put(g, Field::Path, top + 2, "Path", &text_field(&dialog.path, top + 2, cols));
        let button = |text: &'static str, row: usize, col: usize| {
            move |g: &mut Grid, focused: bool| {
                if focused {
                    g.background(col, row, text.len(), draw::SELECT);
                }
                g.text(col, row, text, if focused { draw::BRIGHT } else { draw::KEY });
                (col, text.len())
            }
        };
        put(g, Field::Browse, top + 3, "", &button("[ Browse... ]", top + 3, value_col));
        put(g, Field::Kind, top + 4, "Type", &choice(dialog.kind.name().to_string(), top + 4, !dialog.kind_fixed()));
        put(g, Field::Label, top + 5, "Label", &text_field(&dialog.label, top + 5, 12));
        let ro = if dialog.read_only { "yes" } else { "no" }.to_string();
        put(g, Field::ReadOnly, top + 6, "Read-only", &choice(ro, top + 6, true));
        if top + 5 < bottom {
            g.text(value_col + 14, top + 5, "(empty: the default)", draw::DIM);
        }
        let mut col = value_col;
        let buttons = dialog.fields();
        for (field, text) in [(Field::Mount, "[ Mount ]"), (Field::Unmount, "[ Unmount ]"), (Field::Cancel, "[ Cancel ]")] {
            if buttons.contains(&field) {
                put(g, field, top + 8, "", &button(text, top + 8, col));
                col += text.len() + 2;
            }
        }
        self.hits.extend(hits);
    }

    fn draw_browser(&mut self, g: &mut Grid, content: std::ops::Range<usize>) {
        let Some((browser, _)) = &self.browser else { return };
        let cols = g.cols;
        g.text(2, content.start, browser.title, draw::BRIGHT);
        let dir = contract_home(&browser.dir, self.home.as_deref());
        g.text_to(2, content.start + 1, &fit(&dir, cols - 4), draw::DIM, cols - 2);
        let list = content.start + 2..content.end;
        Self::keep_visible(&mut self.browser_scroll, browser.selected, list.len());
        let separator = std::path::MAIN_SEPARATOR;
        for (i, row) in (self.browser_scroll..browser.rows()).zip(list) {
            if i == browser.selected {
                g.background(1, row, cols - 2, draw::SELECT);
            }
            self.hits.push(Hit { row, col: 1, width: cols - 2, target: Target::BrowserRow(i) });
            let (text, color): (String, Rgb) = match browser.row(i) {
                Some(Row::UseThisDirectory) => ("[ Use this directory ]".to_string(), draw::KEY),
                Some(Row::Entry(e)) if e.is_dir && e.name != ".." => (format!("{}{}", e.name, separator), draw::TEXT),
                Some(Row::Entry(e)) if e.is_dir => (e.name.clone(), draw::TEXT),
                Some(Row::Entry(e)) => (e.name.clone(), draw::BRIGHT),
                None => continue,
            };
            g.text_to(4, row, &fit(&text, cols - 7), color, cols - 2);
        }
    }

    /// The key hints for what is showing, each clickable.
    fn draw_hints(&mut self, g: &mut Grid, row: usize) {
        use UiKey::*;
        let hints: Vec<(&str, &str, UiKey)> = if self.browser.is_some() {
            vec![("Enter", "Open", Enter), ("Bksp", "Up", Backspace), ("Esc", "Cancel", Esc)]
        } else if self.dialog.is_some() {
            vec![("Tab", "Next", Tab), ("Enter", "Mount", Enter), ("Esc", "Cancel", Esc)]
        } else if self.edit.is_some() {
            vec![("Enter", "OK", Enter), ("Esc", "Cancel", Esc)]
        } else if self.page == Page::Drives {
            let (mount, unmount) = if self.frontend.host_files { ("Mount", "Unmount") } else { ("Insert", "Eject") };
            vec![
                ("Enter", "Change", Enter),
                ("Ins", mount, Insert),
                ("Del", unmount, Delete),
                ("Tab", "Page", Tab),
                ("F2", "Save", Save),
                ("Esc", "Close", Esc),
            ]
        } else {
            let mut hints = vec![("\u{2190}\u{2192}", "Change", Right)];
            match self.item().map(Item::input) {
                Some(Input::ChoiceOrText | Input::Text) => hints.push(("Enter", "Type", Enter)),
                Some(Input::File) => hints.extend([("Enter", "Pick", Enter), ("Del", "None", Delete)]),
                _ => {}
            }
            hints.extend([("Tab", "Page", Tab), ("F2", "Save", Save), ("Esc", "Close", Esc)]);
            hints
        };
        let end = g.cols - 2;
        let mut x = 2;
        for (key, action, ui_key) in hints {
            let width = key.chars().count() + 1 + action.len();
            if x + width > end {
                break;
            }
            let after = g.text(x, row, key, draw::KEY);
            g.text(after + 1, row, action, draw::TEXT);
            self.hits.push(Hit { row, col: x, width, target: Target::Key(ui_key) });
            x += width + 2;
        }
    }
}

/// `text` shortened to `width` columns, with "..." in the middle, keeping
/// more of the end (a path's file name).
fn fit(text: &str, width: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= width {
        return text.to_string();
    }
    if width <= 3 {
        return chars[..width].iter().collect();
    }
    let head = (width - 3) / 3;
    let tail = width - 3 - head;
    format!(
        "{}...{}",
        chars[..head].iter().collect::<String>(),
        chars[chars.len() - tail..].iter().collect::<String>()
    )
}

#[cfg(test)]
mod tests;
