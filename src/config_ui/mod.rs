//! The settings window: a semi-transparent panel over the picture, opened
//! with Ctrl+F12 or the DOSCONFIG command. It changes the settings, mounts,
//! swaps and unmounts drives, and saves both to the configuration file.
//!
//! It knows nothing of SDL, the browser or the machine: the frontend feeds
//! it keys, typed text and clicks, and carries out what it asks for through
//! `Host`. What the frontend doesn't have (`Frontend`) isn't offered.

mod achievements;
mod autoexec;
mod browser;
mod cheats;
mod dialog;
mod draw;
mod drives;
mod games;
mod glide;
mod help;
mod host;
mod image;
mod input;
mod item;
mod launch;
mod manual;
pub mod osd;
mod perf;
mod render;
mod rooms;
mod sc55;
mod phonebook;
mod scenes;
mod setup;
pub mod sound_viz;
mod states;
mod values;
pub mod wheel;

use autoexec::AutoexecEditor;
use browser::Browser;
use dialog::{Field, MountDialog, TextField};
use draw::{Layout, Rgb};
pub use draw::cp437;
use games::{GameDialog, GameField};
use image::{ImageDialog, ImageField};
use rooms::{RoomBrowser, RoomButton, RoomField};
use phonebook::PhoneList;
use scenes::SceneList;
pub use manual::Layer;
pub use states::SlotView;
pub use host::Host;
use item::{Input, Item};
use values::cycle;

use crate::games::GameEntry;

use crate::config::Settings;
use crate::disk::{DriveInfo, DriveKind};
use crate::mixer::{CHANNELS, Channel};
use crate::mount::contract_home;
use crate::video::Frame;
use std::path::PathBuf;

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
    /// Show or hide the performance overlay (Ctrl+Shift+F12).
    Overlay,
    /// Show or hide the help on what is under the cursor (F1).
    Help,
    /// Save and exit the BIOS setup (F10).
    F10,
    Char(char),
}

/// A layer scaled for a screenshot: its generation and size, and the frame.
type ScaledLayer = ((u64, (u32, u32)), Frame);

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

/// What a system booted from a disk image has of the drives.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BootView {
    /// The drive its CD-ROM drive shows, if it has one.
    pub cd_drive: Option<u8>,
    /// The drives whose host directories it has as hard disks, with their
    /// BIOS units.
    pub shared: Vec<(u8, u8)>,
}


#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Games,
    States,
    Drives,
    Display,
    Emulator,
    Sound,
    Mixer,
    Network,
    Serial,
    Cheats,
    Achievements,
    Vr,
    Stats,
}

const PAGES: [Page; 13] = [
    Page::Games,
    Page::States,
    Page::Drives,
    Page::Display,
    Page::Emulator,
    Page::Sound,
    Page::Mixer,
    Page::Network,
    Page::Serial,
    Page::Cheats,
    Page::Achievements,
    Page::Vr,
    Page::Stats,
];

impl Page {
    fn title(self) -> &'static str {
        match self {
            Page::Drives => "Drives",
            Page::Display => "Display",
            Page::Emulator => "Emulator",
            Page::Sound => "Sound",
            Page::Mixer => "Mixer",
            Page::Network => "Network",
            Page::Serial => "Ports",
            Page::Games => "Games",
            Page::States => "States",
            Page::Cheats => "Cheats",
            Page::Achievements => "Achievements",
            Page::Vr => "VR",
            Page::Stats => "Stats",
        }
    }

    fn items(self) -> &'static [Item] {
        use Item::*;
        match self {
            Page::Drives | Page::Games | Page::States | Page::Cheats | Page::Achievements | Page::Stats => &[],
            Page::Vr => &[
                VrMode, VrScene, VrScreenFit, VrQuality, VrAmbientOcclusion, VrMsaa, VrScreenGlow, VrControllers, VrSpatialAudio,
                VrCenter, VrSceneScale, VrSeat(0), VrSeat(1), VrSeat(2), VrSeatTurn, VrKeyboard, VrMarkKeyboard, VrTime, VrTimeSpeed, VrLatitude,
                VrWeather, VrWeatherSound, VrLightning, Vfx1, Vfx1Port, Vfx1Stereo, Vfx1SwapEyes,
                Vfx1ViewSize, Vfx1Steer, Vfx1StereoBios,
                VrResolution, VrFoveation, VrRefresh, VrGraphics,
            ],
            Page::Display => {
                &[Scale, Fullscreen, Aspect, Vrr, Filter, Shader, CrtCurvature, CrtGlow, Monochrome, MonitorEffects, Composite, CompositeEra]
            }
            Page::Emulator => &[
                Cycles, AutoFps, AutoBusyOs, Core, Fpu, Cpu, Post, Machine, Voodoo, VoodooMemory, VoodooRenderer, VoodooScale, VoodooScaleShader, VoodooMsaa,
                VoodooDither,
                VoodooAnisotropy, VoodooTextureSampling, VoodooFpsCap, VoodooGamma, VoodooOverlay, PowerVr, PowerVrFilter,
                VeriteRisc, VeriteMhz, VeriteRom, VeriteIrq,
                Memsize, Ems, Umb, DosHigh, Dpmi,
                DosVersion, IdeHardDisks, BootCdrom, FloppyController, HardDiskSpeed, FloppyDiskSpeed, Joystick,
                Deadzone, MouseSensitivity, MouseAutocapture, MouseCaptureMessages, MouseMenuHold, PauseInSettings, KeyboardLayout, Rewind, RewindMemory, CaptureDir, RecordUi, RecordShader, Autoexec,
                ShellSuggestions, ShellColors, SaveShellHistory, VersionCheck,
            ],
            Page::Sound => &[
                SbType, SbPorts, Awe32Rom, Awe32Download, Awe32Ram, Opl, Gus, GusPorts, GusDrive, UltraDir, Midi,
                Sc55Roms, Sc55Model, Sc55Download, SoundFont, Mt32Roms, Mt32Model, MidiPort, LptDac, TandySound,
                Pit2GateAtStart, HardDiskNoise, FloppyDiskNoise,
            ],
            Page::Mixer => &[
                Volume(Channel::Master),
                Volume(Channel::Speaker),
                Volume(Channel::Sb),
                Volume(Channel::Fm),
                Volume(Channel::Gus),
                Volume(Channel::Midi),
                Volume(Channel::CdAudio),
                Volume(Channel::DiskNoise),
                Volume(Channel::LptDac),
                Volume(Channel::Tandy),
                Volume(Channel::Monitor),
                Volume(Channel::Awe),
                Volume(Channel::Modem),
                Volume(Channel::Windows),
                SpeakerFilter,
                SbFilter,
                Reverb,
                ReverbMix,
                Chorus,
                ChorusMix,
                AudioBlocksize,
                AudioPrebuffer,
            ],
            Page::Network => {
                &[Online, Relay, Rooms, Player, Ipx, IpxIrq, IpxFrame, Ne2000, NicBase, NicIrq, MacAddr, Lan, LanHost, Room, Password]
            }
            Page::Serial => &[
                SerialPort(0),
                SerialIrq(0),
                SerialPort(1),
                SerialIrq(1),
                SerialPort(2),
                SerialIrq(2),
                SerialPort(3),
                SerialIrq(3),
                Uart,
                MouseType,
                ModemListen,
                ModemTelnet,
                Phonebook,
                PrinterOutput,
                PrinterPaper,
                PrinterDpi,
                PrinterMultipage,
                PrinterTimeout,
            ],
        }
    }
}


/// What the window says of hardware changes waiting for the running
/// program to end, the video setup in place being `video`: a monitor that
/// changed alone has changed on the screen already.
pub fn pending_note(video: crate::video::adapter::VideoSetup, new: &Settings) -> &'static str {
    let setup = new.video_setup();
    if setup != video && setup.adapter == video.adapter {
        "The picture changed; programs see the monitor once the running program ends"
    } else {
        "Takes effect when the running program ends"
    }
}


/// What a file browser is picking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Pick {
    MountPath,
    SoundFont,
    /// The 3D scene, a glTF file.
    VrScene,
    /// The directory with the MT-32's ROMs.
    Mt32Roms,
    /// The AWE32's ROM file.
    Awe32Rom,
    /// A Vérité board's ROM dump.
    VeriteRom,
    /// The directory with the Sound Canvas's ROMs.
    Sc55Roms,
    /// A game set up for DOSBox, to import.
    ImportGame,
    /// Where a new disk image goes.
    ImagePath,
    /// The archive a game came in, for RetroAchievements.
    AchievementsArchive,
    /// A document or picture, one of a game's manuals.
    Manual,
    /// The folder a mounted drive's changes go to.
    OverlayPath,
}

struct Status {
    text: String,
    error: bool,
}

/// A setting's values listed over the page, for one to be picked.
struct Popup {
    /// The setting, or the field of a row of several.
    item: Item,
    /// The settings with each value (`Item::choices`), the one selected,
    /// the first one shown and how many the last frame showed.
    choices: Vec<Settings>,
    selected: usize,
    scroll: usize,
    visible: usize,
}

/// Something on the window a click acts on.
#[derive(Clone, Copy, Debug)]
enum Target {
    Tab(Page),
    /// A row of the page's list (absolute, not scrolled).
    Row(usize),
    /// The ◄ or ► of a setting.
    Step(usize, isize),
    /// A setting's value, which a click changes as Enter does, and the
    /// button of a field of a row of several (`Item::fields`).
    Button(usize),
    RowField(usize, usize),
    Key(UiKey),
    Field(Field),
    GameField(GameField),
    ImageField(ImageField),
    BrowserRow(usize),
    /// A drive's button over the listing (Windows).
    BrowserDrive(usize),
    /// A line of the `[autoexec]` editor.
    EditorLine(usize),
    /// A row of the room browser, a control of its prompt, and a button of
    /// the room this instance hosts.
    RoomRow(usize),
    RoomField(RoomField),
    RoomButton(RoomButton),
    /// The room browser's tab of the rooms online (or on this network).
    RoomsOnline(bool),
    /// A line of the VR page's scene list.
    SceneRow(usize),
    PhoneRow(usize),
    /// A value of the popup list, and the rest of it.
    PopupRow(usize),
    Popup,
    /// The help's text.
    Help,
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
    /// The field of a row of several that Left and Right change.
    field: usize,
    settings: Settings,
    drives: Vec<DriveInfo>,
    /// What a booted system has of them, while one runs.
    booted: Option<BootView>,
    config_file: Option<PathBuf>,
    home: Option<PathBuf>,
    status: Option<Status>,
    /// The AWE32 ROM's download under way.
    awe32_download: Option<std::sync::mpsc::Receiver<Result<PathBuf, String>>>,
    /// The Sound Canvas ROMs' download: the set asked about, and the
    /// download once the user agreed to it.
    confirm_sc55: Option<&'static crate::sc55::rom::Romset>,
    sc55_download: Option<std::sync::mpsc::Receiver<Result<PathBuf, String>>>,
    /// The question before Glide's DOS overlay is downloaded, and the
    /// download going on.
    confirm_glide: bool,
    glide_download: Option<std::sync::mpsc::Receiver<Result<PathBuf, String>>>,
    /// The selected setting's value being typed, or picked from a list.
    edit: Option<TextField>,
    popup: Option<Popup>,
    dialog: Option<MountDialog>,
    /// Making a new disk image.
    image_dialog: Option<ImageDialog>,
    browser: Option<(Browser, Pick)>,
    browser_scroll: usize,
    /// Where the last frame put the panel, and what can be clicked on it.
    layout: Option<Layout>,
    hits: Vec<Hit>,
    /// Rows of the list the last frame showed, for Page Up and Down.
    visible: usize,
    /// What the Mixer page's meters show (`set_mixer_status`): how loud
    /// each source is, falling slowly, and whether the output is muted.
    levels: [f32; CHANNELS],
    muted: bool,
    /// The Games page: the profiles, the one running, a new one being
    /// made, and one asked to be deleted, or reset (`confirm_reset`).
    games: Vec<GameEntry>,
    active_game: Option<String>,
    game_dialog: Option<GameDialog>,
    confirm_delete: Option<usize>,
    confirm_reset: bool,
    /// What to tell the user once the window has closed (a game launched).
    notice: Option<String>,
    /// A game's manuals, the one open shown over the picture
    /// (manual.rs), and the layer its page is drawn in where the frontend
    /// has one.
    manual: Option<manual::ManualView>,
    /// The ways to start a game being launched (launch.rs).
    chooser: Option<launch::Chooser>,
    /// A game launch awaiting confirmation to close the running program.
    confirm_launch: Option<launch::PendingLaunch>,
    /// Where the manuals were left, for opening them there again.
    manual_places: manual::Places,
    pixel_scale: (f32, f32),
    layered: bool,
    layer: Option<Layer>,
    layer_generation: u64,
    /// The layer scaled for a screenshot or recording (`with_layer`): by
    /// its generation and size.
    layer_scaled: Option<ScaledLayer>,
    /// The Cheats page's search.
    cheats: cheats::Cheats,
    /// The Achievements page.
    achievements: achievements::Achievements,
    /// What the Stats page shows (`set_stats`), and the graphs the last
    /// frame drew as text, drawn over it in pixels.
    stats: Option<crate::stats::StatsView>,
    plots: Vec<Plot>,
    /// Whether the performance overlay shows over the picture while the
    /// window is closed.
    overlay: bool,
    /// The States page: the slots, whether there is anywhere to keep
    /// states, and the pictures the last frame drew in pixels (the cell
    /// of their top left corner).
    states: Vec<SlotView>,
    states_available: bool,
    pictures: Vec<((usize, usize), Frame)>,
    /// The `[autoexec]` commands being edited.
    autoexec: Option<AutoexecEditor>,
    /// The LAN's rooms being browsed.
    rooms: Option<RoomBrowser>,
    /// The VR scenes to choose from, rust-dos.com's list of them once
    /// asked for, and the one downloading (scenes.rs).
    scenes: Option<SceneList>,
    scene_catalog: scenes::Catalog,
    /// The modem's phonebook, and rust-dos.com's list of boards once asked
    /// for (phonebook.rs).
    phonebook: Option<PhoneList>,
    phone_catalog: phonebook::Catalog,
    /// The sound visualizer while it shows (sound_viz.rs).
    sound_viz: Option<sound_viz::SoundViz>,
    /// The settings as a BIOS setup, while it shows (setup.rs).
    setup: Option<setup::Setup>,
    scene_download: Option<scenes::SceneDownload>,
    /// The help shown over everything (F1).
    help: Option<help::HelpView>,
}

/// A graph for `draw::plot`.
struct Plot {
    cells: (usize, usize, usize, usize),
    values: Vec<f32>,
    max: f32,
    color: Rgb,
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
            page: Page::Games,
            row: 0,
            scroll: 0,
            field: 0,
            settings: Settings::default(),
            drives: Vec::new(),
            booted: None,
            config_file: None,
            home: None,
            status: None,
            edit: None,
            popup: None,
            dialog: None,
            image_dialog: None,
            browser: None,
            browser_scroll: 0,
            layout: None,
            hits: Vec::new(),
            visible: 10,
            levels: [0.0; CHANNELS],
            muted: false,
            games: Vec::new(),
            active_game: None,
            game_dialog: None,
            confirm_delete: None,
            confirm_reset: false,
            notice: None,
            manual: None,
            chooser: None,
            confirm_launch: None,
            manual_places: manual::Places::default(),
            pixel_scale: (1.0, 1.0),
            layered: false,
            layer: None,
            layer_generation: 0,
            layer_scaled: None,
            cheats: cheats::Cheats::default(),
            achievements: achievements::Achievements::default(),
            stats: None,
            plots: Vec::new(),
            overlay: false,
            states: Vec::new(),
            states_available: false,
            pictures: Vec::new(),
            autoexec: None,
            rooms: None,
            scenes: None,
            scene_catalog: scenes::Catalog::default(),
            phonebook: None,
            phone_catalog: phonebook::Catalog::default(),
            sound_viz: None,
            setup: None,
            scene_download: None,
            help: None,
            awe32_download: None,
            confirm_sc55: None,
            sc55_download: None,
            confirm_glide: false,
            glide_download: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Whether the machine waits while the window is open: it does,
    /// except on the Mixer page, where it plays on to be heard, and the
    /// Stats page, which shows it running, unless a manual is open over
    /// them; never with `pause_in_settings` off.
    pub fn pauses_machine(&self) -> bool {
        self.open && self.settings.pause_in_settings && (self.manual.is_some() || self.chooser.is_some() || self.confirm_launch.is_some() || !matches!(self.page, Page::Mixer | Page::Stats))
    }

    /// What the window keeps up to date while it is open, for the frontend
    /// to call every frame: the room browser and the Achievements page.
    pub fn poll(&mut self, host: &mut dyn Host) {
        self.poll_rooms(host);
        self.poll_achievements(host);
        self.poll_awe32_download(host);
        self.poll_sc55_download(host);
        self.poll_glide_download(host);
        self.poll_scenes(host);
        self.poll_phonebook();
    }

    /// Download the AWE32's ROM on a thread of its own, into rust-dos's
    /// directory, where the card finds it.
    fn download_awe32_rom(&mut self) {
        if self.awe32_download.is_some() {
            self.info("The AWE32 ROM is downloading...");
            return;
        }
        let Some(dest) = crate::awe32::rom::download_path() else {
            self.error("There is no directory for rust-dos's files to download the ROM into");
            return;
        };
        let (done, result) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new().name("awe32-rom".to_string()).spawn(move || {
            let _ = done.send(crate::awe32::rom::download(&dest).map(|()| dest));
        });
        match spawned {
            Ok(_) => {
                self.awe32_download = Some(result);
                self.info(format!("Downloading the AWE32 ROM from {}...", crate::awe32::rom::DOWNLOAD_URL));
            }
            Err(e) => self.error(format!("The download didn't start: {}", e)),
        }
    }

    /// The download's result, once it has one: the card gets the ROM.
    fn poll_awe32_download(&mut self, host: &mut dyn Host) {
        let Some(result) = &self.awe32_download else { return };
        let outcome = match result.try_recv() {
            Ok(outcome) => outcome,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("the download stopped".to_string()),
        };
        self.awe32_download = None;
        match outcome {
            Ok(path) => {
                let shown = contract_home(&path, self.home.as_deref());
                self.settings.sound.awe32rom = Some(path);
                self.changed(Item::Awe32Rom, host);
                self.info(format!("The AWE32 ROM is in {}", shown));
                self.row = self.row.min(self.row_count().saturating_sub(1));
            }
            Err(e) => self.error(format!("The AWE32 ROM: {}", e)),
        }
    }

    /// RetroAchievements' account changed outside the window (logging in
    /// gave a token): the settings have it, to show and save.
    pub fn sync_achievements(&mut self, username: &str, token: &str) {
        self.settings.achievements.username = username.to_string();
        self.settings.achievements.token = token.to_string();
    }

    /// The mixer's settings changed outside the window (the MIXER
    /// command): show them as they are now, and change them from there.
    pub fn sync_mixer(&mut self, mixer: crate::mixer::MixerSettings) {
        let before = self.items();
        self.settings.mixer = mixer;
        // An effect turned off takes its mix off the page: the cursor stays
        // on its setting, or on the one above the setting that went. A list
        // open has the mixer as it was.
        if before.get(self.row).is_some() {
            let after = self.items();
            self.row = before[..=self.row].iter().rev().find_map(|i| after.iter().position(|a| a == i)).unwrap_or(0);
        }
        self.row = self.row.min(self.row_count().saturating_sub(1));
        self.popup = None;
    }

    /// What the Stats page and the performance overlay show, for the
    /// frontend to hand over every frame while the window is open or the
    /// overlay shows (`overlay_shown`).
    pub fn set_stats(&mut self, view: crate::stats::StatsView) {
        self.stats = Some(view);
    }

    /// How loud each sound source was since the last frame (see
    /// `Mixer::take_peaks`), and whether the output is muted, for the
    /// Mixer page's meters.
    pub fn set_mixer_status(&mut self, muted: bool, peaks: [f32; CHANNELS]) {
        // The meters fall about 20 dB a second.
        for (level, peak) in self.levels.iter_mut().zip(peaks) {
            *level = peak.max(*level * 0.96);
        }
        self.muted = muted;
    }

    /// Open on the current settings and drives. `config_file` is where
    /// Save writes, if there is a file.
    pub fn open(&mut self, settings: &Settings, config_file: Option<PathBuf>, host: &dyn Host) {
        self.open = true;
        self.settings = settings.clone();
        self.drives = host.drives();
        self.booted = host.booted();
        self.config_file = config_file;
        self.home = crate::hostdirs::home_dir();
        self.status = None;
        self.edit = None;
        self.popup = None;
        self.dialog = None;
        self.image_dialog = None;
        self.browser = None;
        self.game_dialog = None;
        self.autoexec = None;
        self.rooms = None;
        self.scenes = None;
        self.phonebook = None;
        self.help = None;
        self.put_away_manual();
        self.manual = None;
        self.chooser = None;
        self.confirm_launch = None;
        self.confirm_delete = None;
        self.confirm_sc55 = None;
        self.confirm_glide = false;
        self.cheats.edit = None;
        self.achievements.edit = None;
        self.cheats.refresh(host);
        self.achievements.edit = None;
        self.achievements.refresh(host);
        self.refresh_games(host);
        self.refresh_states(host);
    }

    /// What the window has to tell the user after it closed, for the
    /// frontend to show over the picture.
    pub fn take_notice(&mut self) -> Option<String> {
        self.notice.take()
    }

    pub fn close(&mut self) {
        self.open = false;
        self.setup = None;
        self.layout = None;
        self.hits.clear();
        self.put_away_manual();
        self.manual = None;
        self.chooser = None;
        self.confirm_launch = None;
        self.layer = None;
    }

    /// Ask before launching `id` would close the program currently running.
    pub fn confirm_launch(&mut self, id: &str, variant: Option<String>, tool: bool) {
        self.confirm_launch_request(id, variant, tool, true);
    }

    fn confirm_launch_request(&mut self, id: &str, variant: Option<String>, tool: bool, variant_selected: bool) {
        self.chooser = None;
        self.status = None;
        self.confirm_launch = Some(launch::PendingLaunch { id: id.to_string(), variant, tool, variant_selected });
    }

    /// Ask before a game launch without a selected variant would close the
    /// program currently running.
    pub fn confirm_game_launch(&mut self, id: &str) {
        self.confirm_launch_request(id, None, false, false);
    }

    /// Whether a Y/N launch confirmation is waiting for input.
    pub fn awaiting_launch_confirmation(&self) -> bool {
        self.confirm_launch.is_some()
    }

    fn confirm_launch_key(&mut self, key: UiKey, host: &mut dyn Host) {
        match key {
            UiKey::Enter | UiKey::Char('y' | 'Y') => {
                let Some(pending) = self.confirm_launch.take() else { return };
                host.close_program();
                let launch = if pending.variant_selected && host.launch_choices(&pending.id).is_some() {
                    host.launch_variant(&pending.id, pending.variant.as_deref(), pending.tool)
                } else {
                    host.launch_game(&pending.id)
                };
                match launch {
                    Ok(message) => {
                        self.close();
                        self.notice = Some(message);
                    }
                    Err(e) => self.error(e),
                }
            }
            UiKey::Char('n' | 'N') | UiKey::Esc => {
                self.confirm_launch = None;
                self.status = None;
            }
            _ => {}
        }
    }

    fn row_count(&self) -> usize {
        match self.page {
            // "+ Mount a drive", and "+ Create a disk image" with the
            // host's files.
            Page::Drives => self.drives.len() + 1 + self.frontend.host_files as usize,
            Page::Games => self.games.len() + 1 + self.frontend.host_files as usize,
            Page::States => self.states.len(),
            Page::Cheats => self.cheats.rows().len(),
            Page::Achievements => self.achievements.rows(self.active_game.is_some()).len(),
            Page::Stats => 0,
            _ => self.items().len(),
        }
    }

    /// The page's settings that the frontend has and that mean something
    /// as the settings are.
    fn items(&self) -> Vec<Item> {
        let shown = |item: &Item| item.available(self.frontend) && item.shown(&self.settings);
        self.page.items().iter().copied().filter(shown).collect()
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

}

/// The columns `hints` take with `gap` columns between them.
fn hints_width(hints: &[(&str, &str, UiKey)], gap: usize) -> usize {
    let words: usize = hints.iter().map(|(key, action, _)| key.chars().count() + 1 + action.len()).sum();
    words + gap * hints.len().saturating_sub(1)
}

/// Drops hints, rightmost first, until they fit in `width` columns at the
/// narrow gap, keeping the first one and the ones every page shares (Tab,
/// F2, F1 and above all Esc), so a crowded page still shows how to leave.
fn fit_hints(hints: &mut Vec<(&str, &str, UiKey)>, width: usize) {
    use UiKey::*;
    while hints_width(hints, 1) > width {
        let Some(at) = (1..hints.len()).rev().find(|&i| !matches!(hints[i].2, Esc | Help | Tab | Save)) else {
            break;
        };
        hints.remove(at);
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

/// Whether a drive is a disk image a system can boot from.
/// Whether a booted system gets `info`'s host directory as a hard disk:
/// D: to Y:, unless mounted with `-noshare`, and C: with `-share`.
fn shareable(info: &DriveInfo) -> bool {
    let share = info.mount.as_ref().and_then(|m| m.opts.share);
    info.kind == DriveKind::HardDisk && info.root.is_some() && share.unwrap_or((3..25).contains(&info.drive))
}

fn bootable(info: &DriveInfo) -> bool {
    info.image.is_some() && matches!(info.kind, DriveKind::HardDisk | DriveKind::Floppy)
}
