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
mod games;
mod help;
mod image;
pub mod osd;
mod perf;
mod rooms;
mod states;

use autoexec::AutoexecEditor;
use browser::{Browser, IMAGES, MT32_ROMS, Row, SOUNDFONTS};
use dialog::{Event, Field, MountDialog, TextField};
use draw::{Grid, Layout, Rgb};
pub use draw::cp437;
use games::{GameDialog, GameField};
use image::{ImageDialog, ImageField};
use rooms::{RoomBrowser, RoomButton, RoomField};
pub use states::SlotView;

use crate::games::{GameEntry, NewGame};

use crate::config::{MidiSynth, Settings};
use crate::cpu::{CoreMode, CpuModel};
use crate::disk::{DRIVE_C, DriveInfo, DriveKind, drive_letter};
use crate::diskio::{DiskClass, DiskSpeed, NoiseMode};
use crate::joystick::{JoystickType, MAX_DEADZONE};
use crate::mixer::{CHANNELS, Channel, ChorusPreset, DEFAULT_MIX, MAX_LEVEL, MAX_MIX, ReverbPreset};
use crate::mount::{MountSpec, contract_home, expand_host_path};
use crate::sb::SbModel;
use crate::timer::CpuSpeed;
use crate::video::Frame;
use crate::video::shader::{CrtSettings, MAX_AMOUNT, parse_amount};
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
    /// Show or hide the performance overlay (Ctrl+Shift+F12).
    Overlay,
    /// Show or hide the help on what is under the cursor (F1).
    Help,
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
    /// Boot the system on `drive`'s disk image now, as BOOT -l does.
    /// Returns what to tell the user.
    fn boot(&mut self, drive: u8) -> Result<String, String> {
        let _ = drive;
        Err("Disk images can't be booted here".to_string())
    }
    /// Have `drive` boot when Rust-DOS starts, or not; the drives' saving
    /// keeps it.
    fn set_boot(&mut self, drive: u8, boot: bool) -> Result<(), String> {
        let _ = (drive, boot);
        Err("Disk images can't be booted here".to_string())
    }
    /// Without the host's files: have the user pick a disk or CD image for
    /// `drive`, or for whichever drive suits it (None). The frontend mounts
    /// it once it is picked, and tells the window (`drives_changed`).
    fn choose_image(&mut self, drive: Option<u8>) -> Result<(), String> {
        let _ = drive;
        Err("Disk images are mounted from the host's files here".to_string())
    }
    /// The game profiles (games.rs), by name.
    fn games(&self) -> Vec<GameEntry> {
        Vec::new()
    }
    /// The game launched and not ended yet, by its id.
    fn active_game(&self) -> Option<String> {
        None
    }
    /// Launch the game `id` at the prompt: its settings, its drives and the
    /// commands that start it. Returns what to tell the user.
    fn launch_game(&mut self, id: &str) -> Result<String, String> {
        let _ = id;
        Err("There are no game profiles here".to_string())
    }
    /// Make a profile of `game` with `settings`. Returns its id.
    fn create_game(&mut self, game: &NewGame, settings: &Settings) -> Result<String, String> {
        let _ = (game, settings);
        Err("There are no game profiles here".to_string())
    }
    fn delete_game(&mut self, id: &str) -> Result<(), String> {
        let _ = id;
        Err("There are no game profiles here".to_string())
    }
    /// Make a profile of the game set up for DOSBox at `source` (games.rs's
    /// `import`). Returns its id, and what to tell the user.
    fn import_game(&mut self, source: &Path) -> Result<(String, String), String> {
        let _ = source;
        Err("Games are imported from the host's files".to_string())
    }
    /// The DOS directory the prompt is in, where a new game likely is.
    fn current_directory(&self) -> String {
        "C:\\".to_string()
    }
    /// The machine's RAM, for the Cheats page to search.
    fn memory(&self) -> &[u8] {
        &[]
    }
    /// Write `bytes` to the machine's memory at `addr`.
    fn poke(&mut self, addr: usize, bytes: &[u8]) {
        let _ = (addr, bytes);
    }
    /// Whether cheats may be used now: not in RetroAchievements' hardcore
    /// mode.
    fn cheats_allowed(&self) -> Result<(), String> {
        Ok(())
    }
    /// The values the machine keeps frozen, and new ones.
    fn freezes(&self) -> Vec<crate::cheats::Freeze> {
        Vec::new()
    }
    fn set_freezes(&mut self, freezes: Vec<crate::cheats::Freeze>) {
        let _ = freezes;
    }
    /// RetroAchievements, as the Achievements page shows it; None where
    /// there is none.
    fn achievements(&self) -> Option<crate::achievements::AchievementsView> {
        None
    }
    /// Log in to RetroAchievements; how it goes shows in `achievements`.
    fn achievements_login(&mut self, username: &str, password: &str) -> Result<(), String> {
        let _ = (username, password);
        Err("There is no RetroAchievements here".to_string())
    }
    fn achievements_logout(&mut self) {}
    /// Tell RetroAchievements which version the game playing is: the zip
    /// or .dosz `archive` it came in, whose hash its profile keeps.
    /// Returns what to tell the user.
    fn identify_game(&mut self, archive: &Path) -> Result<String, String> {
        let _ = archive;
        Err("There is no RetroAchievements here".to_string())
    }
    /// Whether save states can be kept (there is somewhere to keep them).
    fn states_available(&self) -> bool {
        false
    }
    /// The save state slots with a state in them, of the game playing or of
    /// the machine without one.
    fn states(&self) -> Vec<SlotView> {
        Vec::new()
    }
    /// The slot the hotkeys save to and load (Ctrl+F1, Ctrl+F2).
    fn current_slot(&self) -> u8 {
        1
    }
    /// Save the machine to `slot`, which the hotkeys use from then on.
    /// Returns what to tell the user.
    fn save_state(&mut self, slot: u8) -> Result<String, String> {
        let _ = slot;
        Err("There are no save states here".to_string())
    }
    /// Load the state in `slot`, which the hotkeys use from then on.
    /// Returns what to tell the user.
    fn load_state(&mut self, slot: u8) -> Result<String, String> {
        let _ = slot;
        Err("There are no save states here".to_string())
    }
    fn delete_state(&mut self, slot: u8) -> Result<(), String> {
        let _ = slot;
        Err("There are no save states here".to_string())
    }
    /// The lines of the `[autoexec]` section of the file `save` writes to,
    /// comments and blank lines too.
    fn autoexec(&self) -> Result<Vec<String>, String> {
        Err("There is no configuration file here".to_string())
    }
    /// Make `lines` that file's `[autoexec]` section.
    fn save_autoexec(&mut self, lines: &[String]) -> Result<(), String> {
        let _ = lines;
        Err("There is no configuration file here".to_string())
    }
    /// The LAN, as the room browser shows it, if there is a network.
    fn lan(&self) -> Option<crate::net::LanView> {
        None
    }
    /// Ask `relay` (`host[:port]`, None for the first that answers on this
    /// network) for its rooms with `filter` in their names; `lan` has them
    /// once they come.
    fn browse_rooms(&mut self, relay: Option<&str>, filter: &str) -> Result<(), String> {
        let _ = (relay, filter);
        Err("There is no network here".to_string())
    }
    /// Join `room` at `relay` with `password` (empty for none), making the
    /// room if it isn't there.
    fn join_room(&mut self, relay: Option<&str>, room: &str, password: &str) -> Result<(), String> {
        let _ = (relay, room, password);
        Err("There is no network here".to_string())
    }
    fn leave_room(&mut self) {}
    /// End the room this instance hosts for everyone in it, and leave it.
    fn disband_room(&mut self) {}
    /// Make `room` on this network, on a relay this instance runs, and
    /// join it.
    fn host_room(&mut self, room: &str, password: &str) -> Result<(), String> {
        let _ = (room, password);
        Err("There is no network here".to_string())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Drives,
    Display,
    Emulator,
    Sound,
    Mixer,
    Network,
    Serial,
    Games,
    States,
    Cheats,
    Achievements,
    Stats,
}

const PAGES: [Page; 12] = [
    Page::Drives,
    Page::Display,
    Page::Emulator,
    Page::Sound,
    Page::Mixer,
    Page::Network,
    Page::Serial,
    Page::Games,
    Page::States,
    Page::Cheats,
    Page::Achievements,
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
            Page::Stats => "Stats",
        }
    }

    fn items(self) -> &'static [Item] {
        use Item::*;
        match self {
            Page::Drives | Page::Games | Page::States | Page::Cheats | Page::Achievements | Page::Stats => &[],
            Page::Display => {
                &[Scale, Fullscreen, Aspect, Filter, Shader, CrtCurvature, CrtGlow, Monochrome, Composite, CompositeEra]
            }
            Page::Emulator => &[
                Cycles, Core, Cpu, Machine, Voodoo, VoodooMemory, VoodooRenderer, VoodooScale, Memsize, Ems, Umb, Dpmi,
                DosVersion, IdeHardDisks, BootCdrom, HardDiskSpeed, FloppyDiskSpeed, Joystick,
                Deadzone, KeyboardLayout, Rewind, RewindMemory, CaptureDir, RecordUi, RecordShader, Autoexec,
            ],
            Page::Sound => &[
                SbType, SbPorts, Awe32Rom, Awe32Download, Awe32Ram, Opl, Gus, GusPorts, GusDrive, UltraDir, Midi,
                SoundFont, Mt32Roms, Mt32Model, MidiPort, LptDac, TandySound, HardDiskNoise, FloppyDiskNoise,
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
                Volume(Channel::Awe),
                SpeakerFilter,
                SbFilter,
                Reverb,
                ReverbMix,
                Chorus,
                ChorusMix,
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
                PrinterOutput,
                PrinterPaper,
                PrinterDpi,
                PrinterMultipage,
                PrinterTimeout,
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
    /// On the screen now, and for programs once none runs (the monochrome
    /// monitor).
    NowAndAtPrompt,
    NextStart,
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

/// How a setting is changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Input {
    /// Left and right step through the values, and Enter lists them to
    /// pick one (`Item::choices`).
    Choice,
    /// Left and right slide the value along a bar, and Enter types one.
    Slider,
    /// Left and right step through some values, and Enter types any.
    Presets,
    /// Enter types the value.
    Text,
    /// Enter picks a host file.
    File,
    /// Enter opens an editor of its own.
    Link,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Item {
    Scale,
    Fullscreen,
    Aspect,
    Filter,
    Shader,
    /// How far the CRT look's tube bends and how much it glows, shown
    /// with that look.
    CrtCurvature,
    CrtGlow,
    Monochrome,
    /// The CGA's composite monitor, and which CGA makes the signal.
    Composite,
    CompositeEra,
    Cycles,
    /// What runs the instructions: interpreter or dynamic recompiler.
    Core,
    Cpu,
    /// The display adapter.
    Machine,
    /// The 3dfx card, its memory, and how the host draws for it.
    Voodoo,
    VoodooMemory,
    VoodooRenderer,
    VoodooScale,
    Memsize,
    /// Expanded memory and upper memory blocks.
    Ems,
    Umb,
    /// The DPMI host for DOS extenders.
    Dpmi,
    /// The DOS version programs are told.
    DosVersion,
    /// A booted system's hard disks on the IDE channels.
    IdeHardDisks,
    /// A booted system's CD-ROM drive, with or without a CD.
    BootCdrom,
    KeyboardLayout,
    /// Rewind (held Alt+F11) and the memory it takes.
    Rewind,
    RewindMemory,
    SbType,
    /// The Sound Blaster's port, IRQ and DMA channels on one row, each a
    /// field of it (`fields`): the settings after it.
    SbPorts,
    SbBase,
    SbIrq,
    SbDma,
    SbHdma,
    /// The AWE32's sample ROM, its download, and its RAM.
    Awe32Rom,
    Awe32Download,
    Awe32Ram,
    Opl,
    Gus,
    /// The Ultrasound's, likewise.
    GusPorts,
    GusBase,
    GusIrq,
    GusDma,
    GusDrive,
    UltraDir,
    Midi,
    SoundFont,
    /// munt's MT-32: the directory with its ROMs and the model.
    Mt32Roms,
    Mt32Model,
    /// The host's MIDI port for `midisynth=host`.
    MidiPort,
    HardDiskSpeed,
    FloppyDiskSpeed,
    HardDiskNoise,
    FloppyDiskNoise,
    /// A volume in the host's mixer.
    Volume(Channel),
    /// Where screenshots and recordings go, and whether they show the
    /// settings window and the performance overlay, and the CRT shader.
    CaptureDir,
    RecordUi,
    RecordShader,
    /// What the game port has plugged in, and the controllers' deadzone.
    Joystick,
    Deadzone,
    /// The mixer's filters and effects.
    SpeakerFilter,
    SbFilter,
    Reverb,
    Chorus,
    /// The dry/wet mixes of the reverb and the chorus, shown while they
    /// are on.
    ReverbMix,
    ChorusMix,
    /// The DAC on the parallel port.
    LptDac,
    /// The Tandy's and PCjr's sound chip.
    TandySound,
    /// The configuration file's `[autoexec]` commands (autoexec.rs).
    Autoexec,
    /// The IPX driver, its IRQ and its frame type.
    Ipx,
    IpxIrq,
    IpxFrame,
    /// The NE2000 network card, its ports, IRQ and address.
    Ne2000,
    NicBase,
    NicIrq,
    MacAddr,
    /// Whether LAN rooms are on this network or online, at the relay,
    /// for the room browser (rooms.rs), and LAN JOIN and LAN LIST without
    /// an address.
    Online,
    Relay,
    Rooms,
    /// The name the player goes by in LAN rooms.
    Player,
    /// The LAN joined or hosted at startup, its room and password.
    Lan,
    LanHost,
    Room,
    Password,
    /// What each serial port has plugged in, and its IRQ.
    SerialPort(u8),
    SerialIrq(u8),
    /// The ports' chip, the serial mouse, and the modem's TCP calls.
    Uart,
    MouseType,
    ModemListen,
    ModemTelnet,
    /// The printer on LPT1: where its printing goes, its paper and
    /// resolution, whether a job's pages are one document, and how long
    /// a job waits for more.
    PrinterOutput,
    PrinterPaper,
    PrinterDpi,
    PrinterMultipage,
    PrinterTimeout,
}

/// The value `dir` steps away from `current` in `values`, wrapping around.
fn cycle<T: PartialEq + Clone>(values: &[T], current: &T, dir: isize) -> T {
    let at = match values.iter().position(|v| v == current) {
        Some(i) => i as isize,
        None if dir > 0 => -1,
        None => 0,
    };
    values[(at + dir).rem_euclid(values.len() as isize) as usize].clone()
}

/// `s` with each of `values` put in by `set`, in turn.
fn each<T>(s: &Settings, values: impl IntoIterator<Item = T>, set: impl Fn(&mut Settings, T)) -> Vec<Settings> {
    values
        .into_iter()
        .map(|value| {
            let mut s = s.clone();
            set(&mut s, value);
            s
        })
        .collect()
}

/// The next of the ascending `values` above (or below) `current`.
fn step_number<T: PartialOrd + Copy>(values: &[T], current: T, dir: isize) -> T {
    if dir > 0 {
        values.iter().copied().find(|&v| v > current).unwrap_or(current)
    } else {
        values.iter().rev().copied().find(|&v| v < current).unwrap_or(current)
    }
}

/// The speeds the slider steps through; after the fixed ones, max and then
/// auto.
const CYCLES: [u32; 9] = [1000, 3000, 5000, 10_000, 20_000, 50_000, 100_000, CYCLES_MAX, CYCLES_AUTO];
const CYCLES_MAX: u32 = u32::MAX - 1;
const CYCLES_AUTO: u32 = u32::MAX;
const REWIND_MEMORY: [usize; 7] = [64, 128, 256, 512, 1024, 2048, 4096];

/// The memory sizes the slider steps through, as far as the CPU takes
/// (`CpuModel::max_memsize`).
const MEMSIZES: [usize; 16] = [2, 4, 8, 12, 16, 20, 24, 32, 48, 64, 96, 128, 192, 256, 384, 512];

/// The deadzone's slider: a square for every 5%.
const DEADZONE_UNIT: u16 = 5;

/// The memory sizes up to what `cpu` takes.
fn memsizes(cpu: CpuModel) -> impl DoubleEndedIterator<Item = usize> + Clone {
    MEMSIZES.into_iter().filter(move |&mb| mb <= cpu.max_memsize())
}

/// The memory's slider: a square for every step above the least.
fn memsize_bar(mb: usize, cpu: CpuModel) -> String {
    let steps = memsizes(cpu);
    let (full, cells) = (steps.clone().filter(|&m| m <= mb).count().saturating_sub(1), steps.count() - 1);
    format!("{:>3} MB {}{}", mb, "■".repeat(full), "·".repeat(cells - full))
}

/// A value of up to `max` as `text` and a bar of small squares, one for
/// every `unit`, which stay apart from the next row's:
/// "120% ■■■■■■■■■■■■········".
fn bar(text: String, value: u16, max: u16, unit: u16) -> String {
    let (full, cells) = ((value.min(max) / unit) as usize, (max / unit) as usize);
    format!("{} {}{}", text, "■".repeat(full), "·".repeat(cells - full))
}

/// A percentage's bar, a square for every 10%.
fn percent_bar(percent: u16, max: u16) -> String {
    bar(format!("{:>3}%", percent), percent, max, 10)
}

/// A value stepped left or right to the next multiple of `unit`, from a
/// value in between to the multiple on that side, within 0 to `max`.
fn step_units(value: u16, dir: isize, unit: u16, max: u16) -> u16 {
    let units = if dir > 0 { value / unit + 1 } else { value.div_ceil(unit).saturating_sub(1) };
    (units * unit).min(max)
}

fn on_off(on: bool) -> String {
    if on { "on" } else { "off" }.to_string()
}

/// Whether General MIDI can play through a SoundFont: one picked from the
/// host's files, with the synthesizer built in.
fn soundfonts(frontend: Frontend) -> bool {
    cfg!(feature = "midi") && frontend.host_files
}

/// Whether the Sound Blaster is an AWE32.
fn awe32(s: &Settings) -> bool {
    s.sound.sb_installed && s.sound.sb.model == SbModel::Awe32
}

/// A memory size in KB, as the settings show it.
fn ram_size(kb: u32) -> String {
    match kb {
        0 => "none".to_string(),
        kb if kb >= 1024 => format!("{} MB", kb / 1024),
        kb => format!("{} KB", kb),
    }
}

/// Whether munt's MT-32 can play: its library and ROMs are the host's.
fn mt32(frontend: Frontend) -> bool {
    cfg!(not(target_arch = "wasm32")) && frontend.host_files
}

/// Whether MIDI can go out of the host's MIDI ports.
fn host_midi(frontend: Frontend) -> bool {
    cfg!(all(feature = "hostmidi", not(target_arch = "wasm32"))) && frontend.host_files
}

/// The host's MIDI ports, by name.
fn midi_ports() -> Vec<String> {
    #[cfg(all(feature = "hostmidi", not(target_arch = "wasm32")))]
    return crate::midiout::list_ports();
    #[cfg(not(all(feature = "hostmidi", not(target_arch = "wasm32"))))]
    Vec::new()
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
            CrtCurvature => "  Curvature",
            CrtGlow => "  Glow",
            Monochrome => "Monochrome monitor",
            Composite => "CGA composite colour",
            CompositeEra => "  CGA revision",
            Cycles => "CPU speed (cycles)",
            Core => "CPU core",
            Cpu => "Processor",
            Machine => "Video card",
            Memsize => "Memory",
            Voodoo => "3dfx Voodoo Graphics",
            VoodooMemory => "3dfx memory",
            VoodooRenderer => "3dfx drawn by",
            VoodooScale => "3dfx OpenGL size",
            Ems => "Expanded memory (EMS)",
            Umb => "Upper memory (UMB)",
            Dpmi => "DPMI host",
            DosVersion => "Reported DOS version",
            IdeHardDisks => "IDE hard disks (BOOT)",
            BootCdrom => "CD-ROM drive (BOOT)",
            KeyboardLayout => "Keyboard layout",
            Rewind => "Rewind (Alt+F11)",
            RewindMemory => "  Rewind memory",
            SbType => "Sound Blaster",
            // The fields of a row of several go by their names, after
            // the port the row's label names.
            SbPorts | GusPorts => "  Port",
            SbBase | GusBase => "",
            SbIrq | GusIrq => "IRQ",
            SbDma | GusDma => "DMA",
            SbHdma => "HDMA",
            Awe32Rom => "  AWE32 ROM",
            Awe32Download => "  Download the AWE32 ROM...",
            Awe32Ram => "  AWE32 RAM",
            Opl => "FM synthesizer",
            Gus => "Gravis Ultrasound",
            GusDrive => "  Software drive",
            UltraDir => "  ULTRADIR",
            Midi => "MIDI synthesizer",
            SoundFont => "SoundFont",
            Mt32Roms => "MT-32 ROMs",
            Mt32Model => "MT-32 model",
            MidiPort => "MIDI port",
            HardDiskSpeed => "Hard disk speed",
            FloppyDiskSpeed => "Floppy disk speed",
            HardDiskNoise => "Hard disk noise",
            FloppyDiskNoise => "Floppy disk noise",
            Volume(channel) => channel.label(),
            CaptureDir => "Capture folder",
            RecordUi => "Capture window & overlay",
            RecordShader => "Capture CRT shader",
            Joystick => "Joystick",
            Deadzone => "  Deadzone",
            SpeakerFilter => "PC speaker filter",
            SbFilter => "Sound Blaster filter",
            Reverb => "Reverb (FM, GUS, MIDI)",
            Chorus => "Chorus (FM, GUS, MIDI)",
            ReverbMix | ChorusMix => "  Dry/wet mix",
            LptDac => "Parallel port DAC",
            TandySound => "Tandy/PCjr sound",
            Autoexec => "Edit the [autoexec] commands...",
            Ipx => "IPX driver",
            IpxIrq => "  IRQ",
            IpxFrame => "  Frame type",
            Ne2000 => "NE2000 network card",
            NicBase => "  Port",
            NicIrq => "  IRQ",
            MacAddr => "  Ethernet address",
            Rooms => "Find or make a LAN room...",
            Online => "LAN rooms",
            Relay => "  Relay",
            Player => "LAN player name",
            Lan => "Join a LAN at startup",
            LanHost => "Host a LAN at startup",
            Room => "LAN room",
            Password => "LAN password",
            SerialPort(0) => "COM1",
            SerialPort(1) => "COM2",
            SerialPort(2) => "COM3",
            SerialPort(_) => "COM4",
            SerialIrq(_) => "  IRQ",
            Uart => "Serial chip (UART)",
            MouseType => "Serial mouse",
            ModemListen => "Modem takes calls on",
            ModemTelnet => "Modem speaks telnet",
            PrinterOutput => "Printer (LPT1)",
            PrinterPaper => "  Paper",
            PrinterDpi => "  Resolution",
            PrinterMultipage => "  Pages of a job",
            PrinterTimeout => "  Job ends after",
        }
    }

    /// Whether the frontend has what the setting needs.
    fn available(self, frontend: Frontend) -> bool {
        match self {
            Item::Scale | Item::Fullscreen => frontend.window,
            Item::SoundFont => soundfonts(frontend),
            Item::Mt32Roms | Item::Mt32Model => mt32(frontend),
            // The ROM is a host file; the program downloads it.
            Item::Awe32Rom => frontend.host_files,
            Item::Awe32Download => frontend.window && cfg!(all(feature = "sdl", not(target_arch = "wasm32"))),
            Item::MidiPort => host_midi(frontend),
            // The page records the canvas as it shows.
            Item::CaptureDir | Item::RecordUi | Item::RecordShader => frontend.host_files,
            Item::Core => crate::dynrec::AVAILABLE,
            // The browser draws with the emulator's own rasterizer only.
            Item::VoodooRenderer | Item::VoodooScale => frontend.window,
            // A thread of its own packs rewind's states.
            Item::Rewind | Item::RewindMemory => frontend.window,
            // The browser has no sockets for a LAN.
            Item::Online | Item::Rooms | Item::Relay | Item::Player => frontend.window,
            Item::Lan | Item::LanHost | Item::Room | Item::Password => frontend.window,
            Item::ModemListen | Item::ModemTelnet => frontend.window,
            // The browser has nowhere for printouts to go.
            Item::PrinterOutput
            | Item::PrinterPaper
            | Item::PrinterDpi
            | Item::PrinterMultipage
            | Item::PrinterTimeout => frontend.host_files,
            _ => true,
        }
    }

    /// Whether the setting means anything with the settings `s`: the CRT
    /// look's own with that look, the effects' mixes while they are on.
    fn shown(self, s: &Settings) -> bool {
        match self {
            Item::CrtCurvature | Item::CrtGlow => s.shader == crate::video::shader::Shader::Crt,
            Item::ReverbMix => s.mixer.reverb != ReverbPreset::Off,
            Item::ChorusMix => s.mixer.chorus != ChorusPreset::Off,
            Item::RewindMemory => s.rewind,
            Item::VoodooMemory | Item::VoodooRenderer => s.voodoo.enabled,
            Item::Awe32Rom | Item::Awe32Ram => awe32(s),
            // Until there is a ROM.
            Item::Awe32Download => awe32(s) && crate::awe32::rom::find(s.sound.awe32rom.as_deref()).is_none(),
            Item::VoodooScale => s.voodoo.enabled && s.voodoo.renderer == crate::voodoo::Renderer::OpenGl,
            Item::Relay => s.network.online,
            Item::SerialIrq(n) => s.serial.ports[n as usize] != crate::serial::PortType::Off,
            Item::MouseType => s.serial.ports.contains(&crate::serial::PortType::Mouse),
            Item::ModemListen | Item::ModemTelnet => s.serial.ports.contains(&crate::serial::PortType::Modem),
            // The pages' settings for the pages printed; the bytes of a
            // file are as they come.
            Item::PrinterPaper | Item::PrinterDpi => {
                !matches!(s.printer.output, crate::printer::PrinterOutput::None | crate::printer::PrinterOutput::File)
            }
            Item::PrinterMultipage => s.printer.output == crate::printer::PrinterOutput::Pdf,
            Item::PrinterTimeout => s.printer.output != crate::printer::PrinterOutput::None,
            _ => true,
        }
    }

    fn applies(self) -> Applies {
        use Item::*;
        match self {
            Scale | Fullscreen | Aspect | Filter | Shader | CrtCurvature | CrtGlow | Composite | CompositeEra => {
                Applies::Now
            }
            Cycles | Core | Dpmi | DosVersion | IdeHardDisks | BootCdrom | KeyboardLayout | Rewind | RewindMemory | VoodooRenderer | VoodooScale => Applies::Now,
            Monochrome => Applies::NowAndAtPrompt,
            HardDiskSpeed | FloppyDiskSpeed | HardDiskNoise | FloppyDiskNoise | Volume(_) | CaptureDir | RecordUi
            | RecordShader => Applies::Now,
            Joystick | Deadzone | SpeakerFilter | SbFilter | Reverb | Chorus | ReverbMix | ChorusMix => Applies::Now,
            Rooms => Applies::Now,
            Memsize | Autoexec | Lan | LanHost => Applies::NextStart,
            _ => Applies::AtPrompt,
        }
    }

    fn input(self) -> Input {
        match self {
            Item::Volume(_) | Item::ReverbMix | Item::ChorusMix | Item::CrtCurvature | Item::CrtGlow => Input::Slider,
            Item::Memsize | Item::Deadzone => Input::Slider,
            Item::Cycles => Input::Presets,
            Item::UltraDir | Item::CaptureDir => Input::Text,
            Item::MacAddr | Item::Relay | Item::Player | Item::Lan | Item::LanHost | Item::Room | Item::Password => {
                Input::Text
            }
            Item::ModemListen => Input::Text,
            Item::SoundFont | Item::Mt32Roms | Item::Awe32Rom => Input::File,
            Item::Autoexec | Item::Rooms | Item::Awe32Download => Input::Link,
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
            CrtCurvature => percent_bar(s.crt.curvature, MAX_AMOUNT),
            CrtGlow => percent_bar(s.crt.glow, MAX_AMOUNT),
            Monochrome => s.monochrome.describe().to_string(),
            Composite => s.composite.mode.describe().to_string(),
            CompositeEra => s.composite.era.describe().to_string(),
            Cycles => match s.cycles {
                CpuSpeed::Max => "max".to_string(),
                CpuSpeed::Fixed(n) => format!("{} per ms", n),
                CpuSpeed::Auto(n) if n == CpuSpeed::default().initial_cycles() => "auto".to_string(),
                CpuSpeed::Auto(n) => format!("auto (at least {})", n),
            },
            Core => match s.core {
                CoreMode::Auto => "auto (recomp. in prot. mode)",
                CoreMode::Dynamic => "dynamic recompiler",
                CoreMode::Normal => "normal (interpreter)",
            }
            .to_string(),
            Cpu => s.cpu.describe().to_string(),
            Machine => s.machine.describe().to_string(),
            Memsize => memsize_bar(s.memsize, s.cpu),
            Voodoo => on_off(s.voodoo.enabled),
            VoodooMemory => match s.voodoo.board {
                crate::voodoo::Board::Standard => "4 MB (one texture unit)".to_string(),
                crate::voodoo::Board::Max => "12 MB (two texture units)".to_string(),
            },
            VoodooRenderer => match s.voodoo.renderer {
                crate::voodoo::Renderer::Software => "Rust-DOS".to_string(),
                crate::voodoo::Renderer::OpenGl => "OpenGL".to_string(),
            },
            VoodooScale => format!("{}x", s.voodoo.scale),
            Ems => on_off(s.ems),
            Umb => on_off(s.umb),
            Dpmi => on_off(s.dpmi),
            DosVersion => s.dos_version.name(),
            IdeHardDisks => on_off(s.ide_hard_disks),
            BootCdrom => if s.boot_cdrom { "always" } else { "with a CD" }.to_string(),
            KeyboardLayout => s.keyboard_layout.describe(),
            Rewind => on_off(s.rewind),
            RewindMemory => format!("{} MB", s.rewind_memory),
            SbType if !s.sound.sb_installed => "none".to_string(),
            SbType => match sb.model {
                SbModel::Sb16 => "SB16",
                SbModel::Awe32 => "SB AWE32",
                SbModel::SbPro2 => "SB Pro 2",
                SbModel::Sb2 => "SB 2.0",
            }
            .to_string(),
            SbPorts | GusPorts => self
                .fields(s)
                .iter()
                .map(|field| format!("{} {}", field.label(), field.value(s, home)).trim_start().to_string())
                .collect::<Vec<_>>()
                .join(", "),
            SbBase => format!("{:X}h", sb.base),
            SbIrq => sb.irq.to_string(),
            SbDma => sb.dma8.to_string(),
            SbHdma => sb.dma16.to_string(),
            Awe32Rom => match (&s.sound.awe32rom, crate::awe32::rom::find(s.sound.awe32rom.as_deref())) {
                (Some(path), Some(_)) => contract_home(path, home),
                (Some(path), None) => format!("{} (missing)", contract_home(path, home)),
                (None, Some(found)) => format!("{} (default)", contract_home(&found, home)),
                (None, None) => "none found".to_string(),
            },
            Awe32Download => String::new(),
            Awe32Ram => ram_size(s.sound.awe32ram),
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
                MidiSynth::Mt32 => "MT-32 (munt)",
                MidiSynth::Host => "host MIDI port",
                MidiSynth::None => "none",
            }
            .to_string(),
            SoundFont => s.sound.soundfont.as_deref().map_or("none".to_string(), |p| contract_home(p, home)),
            Mt32Roms => s.sound.mt32roms.as_deref().map_or("default".to_string(), |p| contract_home(p, home)),
            Mt32Model => s.sound.mt32model.describe().to_string(),
            MidiPort if s.sound.midiport.is_empty() => "the first".to_string(),
            MidiPort => s.sound.midiport.clone(),
            HardDiskSpeed => s.disk.hard_disk_speed.describe(DiskClass::HardDisk),
            FloppyDiskSpeed => s.disk.floppy_disk_speed.describe(DiskClass::Floppy),
            HardDiskNoise => s.disk.hard_disk_noise.name().to_string(),
            FloppyDiskNoise => s.disk.floppy_disk_noise.name().to_string(),
            Volume(channel) => percent_bar(s.mixer.level(channel), MAX_LEVEL),
            CaptureDir => contract_home(&s.capture_dir, home),
            RecordUi => if s.record_ui { "on (window and overlay)" } else { "off (the picture alone)" }.to_string(),
            RecordShader => if s.record_shader { "on (as the window shows it)" } else { "off (the plain picture)" }.to_string(),
            Joystick => s.joystick.kind.describe().to_string(),
            Deadzone => {
                let dz = s.joystick.deadzone as u16;
                bar(format!("{:>3}%", dz), dz, MAX_DEADZONE as u16, DEADZONE_UNIT)
            }
            SpeakerFilter => on_off(s.mixer.speaker_filter),
            Item::SbFilter => match s.mixer.sb_filter {
                crate::mixer::SbFilter::Auto => "auto (model-dependent)",
                crate::mixer::SbFilter::Off => "off",
            }
            .to_string(),
            Reverb => s.mixer.reverb.name().to_string(),
            Chorus => s.mixer.chorus.name().to_string(),
            ReverbMix => percent_bar(s.mixer.reverb_mix, MAX_MIX),
            ChorusMix => percent_bar(s.mixer.chorus_mix, MAX_MIX),
            LptDac => s.sound.lpt_dac.describe().to_string(),
            TandySound => s.sound.tandy.describe().to_string(),
            Autoexec | Rooms => String::new(),
            Ipx => match s.network.ipx {
                crate::net::IpxMode::Auto => "auto (with LAN HOST or JOIN)",
                crate::net::IpxMode::On => "on",
                crate::net::IpxMode::Off => "off",
            }
            .to_string(),
            IpxIrq => s.network.ipx_irq.map_or("auto".to_string(), |irq| irq.to_string()),
            IpxFrame => match s.network.ipx_frame {
                crate::net::ipx::FrameType::EthernetII => "Ethernet II",
                crate::net::ipx::FrameType::Raw8023 => "802.3 (raw)",
                crate::net::ipx::FrameType::Llc8022 => "802.2",
                crate::net::ipx::FrameType::Snap => "SNAP",
            }
            .to_string(),
            Ne2000 => on_off(s.network.ne2000),
            NicBase => format!("{:X}h", s.network.nic_base),
            NicIrq => s.network.nic_irq.to_string(),
            MacAddr => s.network.mac.map_or("auto (new each start)".to_string(), |mac| mac.to_string()),
            Online => if s.network.online { "via Internet" } else { "on this network" }.to_string(),
            Relay => s.network.relay.clone(),
            Player if s.network.player.is_empty() => "none (\"Player\" and a number)".to_string(),
            Player => s.network.player.clone(),
            Lan => match &s.network.lan {
                None => "off".to_string(),
                Some(relay) if relay.is_empty() => "discover".to_string(),
                Some(relay) => relay.clone(),
            },
            LanHost => s.network.lan_host.map_or("off".to_string(), |port| format!("UDP port {}", port)),
            Room => s.network.room.clone(),
            Password => if s.network.password.is_empty() { "none" } else { "(set)" }.to_string(),
            SerialPort(n) => s.serial.ports[n as usize].describe().to_string(),
            SerialIrq(n) => s.serial.irqs[n as usize].to_string(),
            Uart => match s.serial.chip {
                crate::serial::uart::Chip::Ns16550 => "16550A (FIFOs)",
                crate::serial::uart::Chip::Ns8250 => "8250 (no FIFOs)",
            }
            .to_string(),
            MouseType => s.serial.mouse.describe().to_string(),
            ModemListen => s.serial.modem_listen.map_or("off".to_string(), |port| format!("TCP port {}", port)),
            ModemTelnet => on_off(s.serial.modem_telnet),
            PrinterOutput => s.printer.output.describe().to_string(),
            PrinterPaper => match s.printer.paper {
                crate::printer::Paper::Letter => "Letter (8.5 x 11 in)".to_string(),
                crate::printer::Paper::Legal => "Legal (8.5 x 14 in)".to_string(),
                crate::printer::Paper::A4 => "A4 (210 x 297 mm)".to_string(),
                paper => format!("{} in", paper.name()),
            },
            PrinterDpi => format!("{} dpi", s.printer.dpi),
            PrinterMultipage => if s.printer.multipage { "one document" } else { "a document each" }.to_string(),
            PrinterTimeout => match s.printer.timeout {
                0 => "only when ejected".to_string(),
                ms => format!("{} s without printing", ms as f64 / 1000.0),
            },
        }
    }

    /// The values the setting is picked from, in order, each as the
    /// settings `s` with it; none for one that is typed or picked some
    /// other way, and for a row of several. `drives` are the mounted
    /// drives, which the Ultrasound's drive can't take.
    fn choices(self, s: &Settings, drives: &[DriveInfo], frontend: Frontend) -> Vec<Settings> {
        use Item::*;
        let on_off = |set: fn(&mut Settings, bool)| each(s, [false, true], set);
        match self {
            Scale => each(s, 1..=16, |s, scale| s.scale = scale),
            Fullscreen => on_off(|s, on| s.fullscreen = on),
            Aspect => on_off(|s, on| s.aspect = on),
            Filter => each(s, [crate::config::Filter::Nearest, crate::config::Filter::Linear], |s, f| s.filter = f),
            Shader => each(s, crate::video::shader::Shader::ALL, |s, shader| s.shader = shader),
            Monochrome => each(s, crate::video::mono::Monochrome::ALL, |s, mono| s.monochrome = mono),
            Composite => each(s, crate::video::composite::CompositeMode::ALL, |s, mode| s.composite.mode = mode),
            CompositeEra => each(s, crate::video::composite::CompositeEra::ALL, |s, era| s.composite.era = era),
            Core => each(s, [CoreMode::Auto, CoreMode::Dynamic, CoreMode::Normal], |s, core| s.core = core),
            Cpu => each(s, [CpuModel::I386, CpuModel::I486, CpuModel::Pentium, CpuModel::PentiumMmx], |s, cpu| {
                s.cpu = cpu;
                s.memsize = s.memsize.min(cpu.max_memsize());
            }),
            Machine => each(s, crate::video::adapter::Adapter::ALL, |s, machine| s.machine = machine),
            Voodoo => on_off(|s, on| s.voodoo.enabled = on),
            VoodooMemory => {
                each(s, [crate::voodoo::Board::Standard, crate::voodoo::Board::Max], |s, board| s.voodoo.board = board)
            }
            VoodooRenderer => each(s, [crate::voodoo::Renderer::Software, crate::voodoo::Renderer::OpenGl], |s, r| {
                s.voodoo.renderer = r
            }),
            VoodooScale => each(s, [1, 2, 3, 4], |s, scale| s.voodoo.scale = scale),
            Ems => on_off(|s, on| s.ems = on),
            Umb => on_off(|s, on| s.umb = on),
            Dpmi => on_off(|s, on| s.dpmi = on),
            IdeHardDisks => on_off(|s, on| s.ide_hard_disks = on),
            BootCdrom => on_off(|s, on| s.boot_cdrom = on),
            DosVersion => {
                let mut versions = crate::config::DosVersion::PRESETS.to_vec();
                if !versions.contains(&s.dos_version) {
                    versions.push(s.dos_version);
                    versions.sort();
                }
                each(s, versions, |s, version| s.dos_version = version)
            }
            KeyboardLayout => each(s, crate::keylayout::LayoutSetting::all(), |s, layout| s.keyboard_layout = layout),
            Rewind => on_off(|s, on| s.rewind = on),
            RecordUi => on_off(|s, on| s.record_ui = on),
            RecordShader => on_off(|s, on| s.record_shader = on),
            RewindMemory => each(s, REWIND_MEMORY, |s, mb| s.rewind_memory = mb),
            SbType => {
                let models = [Some(SbModel::Sb16), Some(SbModel::Awe32), Some(SbModel::SbPro2), Some(SbModel::Sb2), None];
                each(s, models, |s, model| match model {
                    Some(model) => {
                        s.sound.sb.model = model;
                        s.sound.sb_installed = true;
                    }
                    None => s.sound.sb_installed = false,
                })
            }
            SbBase => each(s, [0x210, 0x220, 0x230, 0x240, 0x250, 0x260, 0x270, 0x280], |s, base| s.sound.sb.base = base),
            SbIrq => each(s, [2, 3, 5, 7, 9, 10, 11, 12, 15], |s, irq| s.sound.sb.irq = irq),
            SbDma => each(s, [0, 1, 3], |s, dma| s.sound.sb.dma8 = dma),
            SbHdma => each(s, [5, 6, 7], |s, dma| s.sound.sb.dma16 = dma),
            Awe32Ram => each(s, crate::awe32::RAM_SIZES, |s, kb| s.sound.awe32ram = kb),
            Opl => on_off(|s, opl3| s.sound.opl3 = opl3),
            Gus => on_off(|s, on| s.sound.gus.enabled = on),
            GusBase => each(s, [0x210, 0x220, 0x240, 0x250, 0x260], |s, base| s.sound.gus.base = base),
            GusIrq => each(s, [2, 3, 5, 7, 11, 12, 15], |s, irq| s.sound.gus.irq = irq),
            GusDma => each(s, [1, 3, 5, 6, 7], |s, dma| s.sound.gus.dma = dma),
            GusDrive => {
                // D: to Y:, where no drive of the user's own is.
                let mut letters = vec![None];
                letters.extend(
                    (3..25u8)
                        .filter(|&d| drives.iter().all(|i| i.drive != d || i.kind == DriveKind::Virtual))
                        .map(Some),
                );
                each(s, letters, |s, drive| s.sound.gus.drive = drive)
            }
            Midi => {
                let mut synths = vec![MidiSynth::Auto];
                if soundfonts(frontend) {
                    synths.push(MidiSynth::SoundFont);
                }
                synths.push(MidiSynth::Gus);
                if mt32(frontend) {
                    synths.push(MidiSynth::Mt32);
                }
                if host_midi(frontend) {
                    synths.push(MidiSynth::Host);
                }
                synths.push(MidiSynth::None);
                each(s, synths, |s, synth| s.sound.midisynth = synth)
            }
            Mt32Model => each(s, crate::config::Mt32Model::ALL, |s, model| s.sound.mt32model = model),
            // The ports there are now, and the first of them (empty).
            MidiPort => each(s, std::iter::once(String::new()).chain(midi_ports()), |s, port| s.sound.midiport = port),
            HardDiskSpeed => each(s, DiskSpeed::ALL, |s, speed| s.disk.hard_disk_speed = speed),
            FloppyDiskSpeed => each(s, DiskSpeed::ALL, |s, speed| s.disk.floppy_disk_speed = speed),
            HardDiskNoise => each(s, NoiseMode::ALL, |s, noise| s.disk.hard_disk_noise = noise),
            FloppyDiskNoise => each(s, NoiseMode::ALL, |s, noise| s.disk.floppy_disk_noise = noise),
            Joystick => each(s, JoystickType::ALL, |s, kind| s.joystick.kind = kind),
            SpeakerFilter => on_off(|s, on| s.mixer.speaker_filter = on),
            Item::SbFilter => {
                use crate::mixer::SbFilter as Filter;
                each(s, [Filter::Auto, Filter::Off], |s, filter| s.mixer.sb_filter = filter)
            }
            Reverb => each(s, ReverbPreset::ALL, |s, preset| s.mixer.reverb = preset),
            Chorus => each(s, ChorusPreset::ALL, |s, preset| s.mixer.chorus = preset),
            LptDac => each(s, crate::lpt_dac::LptDacType::ALL, |s, dac| s.sound.lpt_dac = dac),
            TandySound => each(s, crate::sn76489::TandySound::ALL, |s, tandy| s.sound.tandy = tandy),
            Ipx => each(s, crate::net::IpxMode::ALL, |s, mode| s.network.ipx = mode),
            IpxIrq => each(s, [None, Some(3), Some(4), Some(5), Some(7), Some(9), Some(10), Some(11), Some(15)], |s, irq| {
                s.network.ipx_irq = irq
            }),
            IpxFrame => each(s, crate::net::ipx::FrameType::ALL, |s, kind| s.network.ipx_frame = kind),
            Ne2000 => on_off(|s, on| s.network.ne2000 = on),
            NicBase => each(s, crate::net::NIC_BASES, |s, base| s.network.nic_base = base),
            NicIrq => each(s, [3, 4, 5, 7, 9, 10, 11, 15], |s, irq| s.network.nic_irq = irq),
            Online => on_off(|s, on| s.network.online = on),
            SerialPort(n) => each(s, crate::serial::PortType::ALL, |s, kind| s.serial.ports[n as usize] = kind),
            SerialIrq(n) => each(s, [3, 4, 5, 7, 9, 10, 11, 12, 15], |s, irq| s.serial.irqs[n as usize] = irq),
            Uart => {
                use crate::serial::uart::Chip;
                each(s, [Chip::Ns16550, Chip::Ns8250], |s, chip| s.serial.chip = chip)
            }
            MouseType => each(s, crate::serial::mouse::MouseType::ALL, |s, kind| s.serial.mouse = kind),
            ModemTelnet => on_off(|s, on| s.serial.modem_telnet = on),
            PrinterOutput => {
                use crate::printer::PrinterOutput as Output;
                let outputs = Output::ALL.into_iter().filter(|&o| o != Output::Printer || frontend.window);
                each(s, outputs, |s, output| s.printer.output = output)
            }
            PrinterPaper => each(s, crate::printer::Paper::ALL, |s, paper| s.printer.paper = paper),
            PrinterDpi => each(s, [180, 240, 300, 360, 600], |s, dpi| s.printer.dpi = dpi),
            PrinterMultipage => on_off(|s, on| s.printer.multipage = on),
            PrinterTimeout => each(s, [1000, 2000, 3000, 5000, 10_000, 30_000, 0], |s, ms| s.printer.timeout = ms),
            // Slid, typed, picked from the host's files or edited, and a
            // row of several, whose fields have their own.
            Cycles | CrtCurvature | CrtGlow | Memsize | Volume(_) | ReverbMix | ChorusMix | Deadzone | UltraDir
            | SoundFont | Mt32Roms | Awe32Rom | Awe32Download | CaptureDir | Autoexec | SbPorts | GusPorts | MacAddr
            | Rooms | Relay | Player
            | Lan | LanHost | Room | Password | ModemListen => Vec::new(),
        }
    }

    /// Step the setting left (-1) or right (1): a slider a square of its
    /// bar. `drives` are the mounted drives, which the Ultrasound's drive
    /// can't take.
    fn step(self, s: &mut Settings, dir: isize, drives: &[DriveInfo], frontend: Frontend) {
        use Item::*;
        match self {
            Scale => s.scale = (s.scale as isize + dir).clamp(1, 16) as u32,
            Cycles => {
                let current = match s.cycles {
                    CpuSpeed::Max => CYCLES_MAX,
                    CpuSpeed::Auto(_) => CYCLES_AUTO,
                    CpuSpeed::Fixed(n) => n,
                };
                // At the end, auto keeps its real-mode speed.
                let next = step_number(&CYCLES, current, dir);
                if next != current {
                    s.cycles = match next {
                        CYCLES_MAX => CpuSpeed::Max,
                        CYCLES_AUTO => CpuSpeed::default(),
                        n => CpuSpeed::Fixed(n),
                    };
                }
            }
            RewindMemory => s.rewind_memory = step_number(&REWIND_MEMORY, s.rewind_memory, dir),
            CrtCurvature => s.crt.curvature = step_units(s.crt.curvature, dir, 10, MAX_AMOUNT),
            CrtGlow => s.crt.glow = step_units(s.crt.glow, dir, 10, MAX_AMOUNT),
            Memsize => {
                let steps: Vec<usize> = memsizes(s.cpu).collect();
                s.memsize = step_number(&steps, s.memsize, dir);
            }
            Volume(channel) => s.mixer.set_level(channel, step_units(s.mixer.level(channel), dir, 10, MAX_LEVEL)),
            ReverbMix => s.mixer.reverb_mix = step_units(s.mixer.reverb_mix, dir, 10, MAX_MIX),
            ChorusMix => s.mixer.chorus_mix = step_units(s.mixer.chorus_mix, dir, 10, MAX_MIX),
            Deadzone => {
                let dz = step_units(s.joystick.deadzone as u16, dir, DEADZONE_UNIT, MAX_DEADZONE as u16);
                s.joystick.deadzone = dz as u8;
            }
            // Around the values it is picked from.
            _ => {
                let choices = self.choices(s, drives, frontend);
                if !choices.is_empty() {
                    *s = cycle(&choices, s, dir);
                }
            }
        }
    }

    /// The settings a row of several shows side by side, each a button of
    /// its own; none for a row of one. The high DMA channel is the SB16's
    /// alone.
    fn fields(self, s: &Settings) -> Vec<Item> {
        match self {
            Item::SbPorts if s.sound.sb.model.is_sb16() => {
                vec![Item::SbBase, Item::SbIrq, Item::SbDma, Item::SbHdma]
            }
            Item::SbPorts => vec![Item::SbBase, Item::SbIrq, Item::SbDma],
            Item::GusPorts => vec![Item::GusBase, Item::GusIrq, Item::GusDma],
            _ => Vec::new(),
        }
    }

    /// The text to edit.
    fn text(self, s: &Settings) -> String {
        match self {
            Item::Cycles => s.cycles.to_string(),
            Item::UltraDir => s.sound.gus.ultradir.clone().unwrap_or_default(),
            Item::Volume(channel) => s.mixer.level(channel).to_string(),
            Item::CaptureDir => s.capture_dir.display().to_string(),
            Item::Memsize => s.memsize.to_string(),
            Item::Deadzone => s.joystick.deadzone.to_string(),
            Item::CrtCurvature => s.crt.curvature.to_string(),
            Item::CrtGlow => s.crt.glow.to_string(),
            Item::ReverbMix => s.mixer.reverb_mix.to_string(),
            Item::ChorusMix => s.mixer.chorus_mix.to_string(),
            Item::MacAddr => s.network.mac.map_or(String::new(), |mac| mac.to_string()),
            Item::Relay => s.network.relay.clone(),
            Item::Player => s.network.player.clone(),
            Item::Lan => Item::Lan.value(s, None),
            Item::LanHost => s.network.lan_host.map_or("off".to_string(), |port| port.to_string()),
            Item::Room => s.network.room.clone(),
            Item::Password => s.network.password.clone(),
            Item::ModemListen => s.serial.modem_listen.map_or("off".to_string(), |port| port.to_string()),
            _ => String::new(),
        }
    }

    fn set_text(self, s: &mut Settings, text: &str) -> Result<(), String> {
        let text = text.trim();
        match self {
            Item::Cycles => s.cycles = CpuSpeed::parse(text)?,
            Item::UltraDir => s.sound.gus.ultradir = (!text.is_empty()).then(|| text.to_string()),
            Item::Volume(channel) => s.mixer.set_level(channel, crate::mixer::parse_level(text)?),
            Item::CaptureDir if text.is_empty() => return Err("A capture folder, please".to_string()),
            Item::CaptureDir => s.capture_dir = expand_host_path(text, Path::new(""), crate::hostdirs::home_dir().as_deref()),
            Item::Memsize => {
                let mb = crate::config::parse_memsize(text)?;
                if mb > s.cpu.max_memsize() {
                    return Err(format!("A {} takes up to {} MB", s.cpu.describe(), s.cpu.max_memsize()));
                }
                s.memsize = mb;
            }
            Item::Deadzone => s.joystick.deadzone = crate::joystick::parse_deadzone(text)?,
            Item::CrtCurvature => s.crt.curvature = parse_amount(text).ok_or("The curvature goes from 0 to 100%")?,
            Item::CrtGlow => s.crt.glow = parse_amount(text).ok_or("The glow goes from 0 to 100%")?,
            Item::ReverbMix => s.mixer.reverb_mix = crate::mixer::parse_mix(text)?,
            Item::ChorusMix => s.mixer.chorus_mix = crate::mixer::parse_mix(text)?,
            Item::MacAddr if text.is_empty() => s.network.mac = None,
            Item::MacAddr => s.network.set("macaddr", text)?,
            Item::Relay if text.is_empty() => s.network.relay = crate::net::DEFAULT_RELAY.into(),
            Item::Relay => s.network.set("relay", text)?,
            Item::Player => s.network.set("player", text)?,
            Item::Lan => s.network.set("lan", text)?,
            Item::LanHost => s.network.set("lanhost", text)?,
            Item::Room => s.network.set("room", text)?,
            Item::Password => s.network.set("password", text)?,
            Item::ModemListen => s.serial.set("modemlisten", text)?,
            _ => {}
        }
        Ok(())
    }

    /// Delete: back to the default. Returns whether that changed anything.
    fn clear(self, s: &mut Settings) -> bool {
        match self {
            Item::UltraDir => s.sound.gus.ultradir.take().is_some(),
            Item::SoundFont => s.sound.soundfont.take().is_some(),
            Item::Mt32Roms => s.sound.mt32roms.take().is_some(),
            Item::Awe32Rom => s.sound.awe32rom.take().is_some(),
            Item::MidiPort => !std::mem::take(&mut s.sound.midiport).is_empty(),
            Item::CaptureDir => {
                let default = Settings::default().capture_dir;
                std::mem::replace(&mut s.capture_dir, default.clone()) != default
            }
            Item::Volume(channel) => {
                let changed = s.mixer.level(channel) != 100;
                s.mixer.set_level(channel, 100);
                changed
            }
            Item::Memsize => {
                let default = Settings::default().memsize.min(s.cpu.max_memsize());
                std::mem::replace(&mut s.memsize, default) != default
            }
            Item::Deadzone => {
                let default = crate::joystick::JoystickSettings::default().deadzone;
                std::mem::replace(&mut s.joystick.deadzone, default) != default
            }
            Item::CrtCurvature => {
                let default = CrtSettings::default().curvature;
                std::mem::replace(&mut s.crt.curvature, default) != default
            }
            Item::CrtGlow => {
                let default = CrtSettings::default().glow;
                std::mem::replace(&mut s.crt.glow, default) != default
            }
            Item::ReverbMix => std::mem::replace(&mut s.mixer.reverb_mix, DEFAULT_MIX) != DEFAULT_MIX,
            Item::ChorusMix => std::mem::replace(&mut s.mixer.chorus_mix, DEFAULT_MIX) != DEFAULT_MIX,
            Item::MacAddr => s.network.mac.take().is_some(),
            Item::Relay => std::mem::replace(&mut s.network.relay, crate::net::DEFAULT_RELAY.into()) != crate::net::DEFAULT_RELAY,
            Item::Player => !std::mem::take(&mut s.network.player).is_empty(),
            Item::Lan => s.network.lan.take().is_some(),
            Item::LanHost => s.network.lan_host.take().is_some(),
            Item::Room => std::mem::replace(&mut s.network.room, crate::net::DEFAULT_ROOM.into()) != crate::net::DEFAULT_ROOM,
            Item::Password => !std::mem::take(&mut s.network.password).is_empty(),
            Item::ModemListen => s.serial.modem_listen.take().is_some(),
            _ => false,
        }
    }
}

/// What a file browser is picking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Pick {
    MountPath,
    SoundFont,
    /// The directory with the MT-32's ROMs.
    Mt32Roms,
    /// The AWE32's ROM file.
    Awe32Rom,
    /// A game set up for DOSBox, to import.
    ImportGame,
    /// Where a new disk image goes.
    ImagePath,
    /// The archive a game came in, for RetroAchievements.
    AchievementsArchive,
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
    /// A line of the `[autoexec]` editor.
    EditorLine(usize),
    /// A row of the room browser, a control of its prompt, and a button of
    /// the room this instance hosts.
    RoomRow(usize),
    RoomField(RoomField),
    RoomButton(RoomButton),
    /// The room browser's tab of the rooms online (or on this network).
    RoomsOnline(bool),
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
    config_file: Option<PathBuf>,
    home: Option<PathBuf>,
    status: Option<Status>,
    /// The AWE32 ROM's download under way.
    awe32_download: Option<std::sync::mpsc::Receiver<Result<PathBuf, String>>>,
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
    /// made, and one asked to be deleted.
    games: Vec<GameEntry>,
    active_game: Option<String>,
    game_dialog: Option<GameDialog>,
    confirm_delete: Option<usize>,
    /// What to tell the user once the window has closed (a game launched).
    notice: Option<String>,
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
            page: Page::Drives,
            row: 0,
            scroll: 0,
            field: 0,
            settings: Settings::default(),
            drives: Vec::new(),
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
            notice: None,
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
            help: None,
            awe32_download: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Whether the machine waits while the window is open: it does,
    /// except on the Mixer page, where it plays on to be heard, and the
    /// Stats page, which shows it running.
    pub fn pauses_machine(&self) -> bool {
        self.open && !matches!(self.page, Page::Mixer | Page::Stats)
    }

    /// What the window keeps up to date while it is open, for the frontend
    /// to call every frame: the room browser and the Achievements page.
    pub fn poll(&mut self, host: &mut dyn Host) {
        self.poll_rooms(host);
        self.poll_achievements(host);
        self.poll_awe32_download(host);
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
        self.settings.mixer = mixer;
        // An effect turned off takes its mix off the page, and a list open
        // has the mixer as it was.
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
        self.help = None;
        self.confirm_delete = None;
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
        self.layout = None;
        self.hits.clear();
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
        if key == UiKey::Overlay {
            self.overlay_key();
        } else if key == UiKey::Help {
            self.toggle_help();
        } else if self.help.is_some() {
            self.help_key(key);
        } else if self.browser.is_some() {
            self.browser_key(key, host);
        } else if self.dialog.is_some() {
            self.dialog_key(key, host);
        } else if self.image_dialog.is_some() {
            self.image_dialog_key(key, host);
        } else if self.game_dialog.is_some() {
            self.game_dialog_key(key, host);
        } else if self.autoexec.is_some() {
            self.autoexec_key(key, host);
        } else if self.rooms.is_some() {
            self.rooms_key(key, host);
        } else if self.cheats.edit.is_some() {
            self.cheats_edit_key(key, host);
        } else if self.achievements.edit.is_some() {
            self.achievements_edit_key(key, host);
        } else if self.popup.is_some() {
            self.popup_key(key, host);
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
        let hit = self.layout.and_then(|l| l.cell_at(x, y)).and_then(|(col, row)| {
            self.hits
                .iter()
                .rev()
                .find(|h| h.row == row && (h.col..h.col + h.width).contains(&col))
                .map(|h| (h.target, col - h.col))
        });
        // A click off the help closes it, as one off the popup list does,
        // and does nothing else; the key hints still work.
        if self.help.is_some() && !matches!(hit, Some((Target::Help | Target::Key(_), _))) {
            self.help = None;
            return;
        }
        if self.popup.is_some() && !matches!(hit, Some((Target::PopupRow(_) | Target::Popup, _))) {
            self.popup = None;
            return;
        }
        let Some((target, into)) = hit else { return };
        match target {
            Target::Tab(page) => {
                if self.dialog.is_none()
                    && self.image_dialog.is_none()
                    && self.browser.is_none()
                    && self.game_dialog.is_none()
                    && self.autoexec.is_none()
                    && self.rooms.is_none()
                {
                    self.edit = None;
                    self.cheats.edit = None;
                    self.achievements.edit = None;
                    self.show_page(page);
                }
            }
            Target::Row(i) if i == self.row => self.key(UiKey::Enter, host),
            Target::Row(i) => {
                self.edit = None;
                self.cheats.edit = None;
                self.achievements.edit = None;
                self.select(i);
            }
            Target::Button(i) => {
                self.edit = None;
                self.cheats.edit = None;
                self.achievements.edit = None;
                self.select(i);
                self.key(UiKey::Enter, host);
            }
            Target::RowField(i, field) => {
                self.select(i);
                self.field = field;
                self.key(UiKey::Enter, host);
            }
            Target::Step(i, dir) => {
                self.edit = None;
                self.cheats.edit = None;
                self.achievements.edit = None;
                self.row = i;
                self.key(if dir < 0 { UiKey::Left } else { UiKey::Right }, host);
            }
            Target::Key(key) => self.key(key, host),
            Target::Field(field) => {
                if let Some(dialog) = &mut self.dialog {
                    let again = dialog.focus == field;
                    dialog.focus = field;
                    match field {
                        Field::Browse | Field::Mount | Field::Boot | Field::Unmount | Field::Cancel => self.key(UiKey::Enter, host),
                        Field::Drive | Field::Kind | Field::ReadOnly | Field::BootFlag if again => dialog.step(field, 1),
                        _ => {}
                    }
                }
            }
            Target::GameField(field) => self.game_field_clicked(field, host),
            Target::ImageField(field) => self.image_field_clicked(field, host),
            Target::BrowserRow(i) => {
                if let Some((browser, _)) = &mut self.browser {
                    if browser.selected == i {
                        self.key(UiKey::Enter, host);
                    } else {
                        browser.select(i);
                    }
                }
            }
            Target::EditorLine(i) => self.autoexec_clicked(i, into),
            Target::RoomRow(_) | Target::RoomField(_) | Target::RoomButton(_) | Target::RoomsOnline(_) => {
                self.room_clicked(target, host)
            }
            Target::PopupRow(i) => {
                if let Some(popup) = &mut self.popup {
                    popup.selected = i;
                }
                self.key(UiKey::Enter, host);
            }
            Target::Popup | Target::Help => {}
        }
    }

    fn show_page(&mut self, page: Page) {
        if page != self.page {
            self.page = page;
            self.select(0);
            self.scroll = 0;
            self.confirm_delete = None;
        }
    }

    /// Select row `row`, at its first field.
    fn select(&mut self, row: usize) {
        if row != self.row {
            self.row = row;
            self.field = 0;
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
        if self.confirm_delete.is_some() && self.page == Page::States {
            return self.states_key(key, host);
        }
        if self.confirm_delete.is_some() {
            return self.games_key(key, host);
        }
        if let Some(row) = Self::navigate(key, self.row, self.row_count(), self.visible) {
            self.select(row);
            return;
        }
        let at = PAGES.iter().position(|&p| p == self.page).unwrap_or(0);
        match key {
            UiKey::Esc => self.close(),
            UiKey::Tab => self.show_page(PAGES[(at + 1) % PAGES.len()]),
            UiKey::BackTab => self.show_page(PAGES[(at + PAGES.len() - 1) % PAGES.len()]),
            UiKey::Save => self.save(host),
            _ if self.page == Page::Drives => self.drives_key(key, host),
            _ if self.page == Page::Games => self.games_key(key, host),
            _ if self.page == Page::States => self.states_key(key, host),
            _ if self.page == Page::Cheats => self.cheats_key(key, host),
            _ if self.page == Page::Achievements => self.achievements_key(key, host),
            _ if self.page == Page::Stats => {}
            _ => self.setting_key(key, host),
        }
    }

    fn save(&mut self, host: &mut dyn Host) {
        let Some(path) = &self.config_file else {
            self.error("No configuration file to save to (Rust-DOS started with --no-config)");
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
        // A row of several: Left and Right go to the field beside, Enter
        // lists its values.
        let fields = item.fields(&self.settings);
        if !fields.is_empty() {
            let field = self.field.min(fields.len() - 1);
            match key {
                UiKey::Left | UiKey::Right => self.field = self.field_beside(fields.len(), key),
                UiKey::Enter => {
                    self.field = field;
                    self.open_popup(fields[field]);
                }
                _ => {}
            }
            return;
        }
        match (key, item.input()) {
            (UiKey::Left, Input::Choice | Input::Slider | Input::Presets) => self.step(item, -1, host),
            (UiKey::Right, Input::Choice | Input::Slider | Input::Presets) => self.step(item, 1, host),
            (UiKey::Enter, Input::Choice) => self.open_popup(item),
            (UiKey::Enter, Input::Slider | Input::Presets | Input::Text) => {
                self.edit = Some(TextField::new(&item.text(&self.settings)));
            }
            (UiKey::Enter, Input::File) if item == Item::Mt32Roms => self.open_browser(Pick::Mt32Roms),
            (UiKey::Enter, Input::File) if item == Item::Awe32Rom => self.open_browser(Pick::Awe32Rom),
            (UiKey::Enter, Input::File) => self.open_browser(Pick::SoundFont),
            (UiKey::Enter, Input::Link) if item == Item::Rooms => self.open_rooms(),
            (UiKey::Enter, Input::Link) if item == Item::Awe32Download => self.download_awe32_rom(),
            (UiKey::Enter, Input::Link) => self.open_autoexec(host),
            (UiKey::Delete | UiKey::Backspace, _) if item.clear(&mut self.settings) => self.changed(item, host),
            _ => {}
        }
    }

    fn step(&mut self, item: Item, dir: isize, host: &mut dyn Host) {
        item.step(&mut self.settings, dir, &self.drives, self.frontend);
        self.changed(item, host);
    }

    /// The field beside the one chosen that Left or Right goes to, in a
    /// row of `fields`; none past the ends.
    fn field_beside(&self, fields: usize, key: UiKey) -> usize {
        let field = self.field.min(fields - 1);
        if key == UiKey::Left { field.saturating_sub(1) } else { (field + 1).min(fields - 1) }
    }

    /// List `item`'s values over the page, the one it has selected.
    fn open_popup(&mut self, item: Item) {
        let choices = item.choices(&self.settings, &self.drives, self.frontend);
        if choices.is_empty() {
            return;
        }
        let selected = choices.iter().position(|c| *c == self.settings).unwrap_or(0);
        self.popup = Some(Popup { item, choices, selected, scroll: 0, visible: self.visible });
    }

    /// A setting's values listed: Enter picks the one selected, a letter
    /// selects the next one starting with it, and in a row of several,
    /// Left and Right go on to the next field's.
    fn popup_key(&mut self, key: UiKey, host: &mut dyn Host) {
        let home = self.home.as_deref();
        let Some(popup) = &mut self.popup else { return };
        if let Some(i) = Self::navigate(key, popup.selected, popup.choices.len(), popup.visible) {
            popup.selected = i;
            return;
        }
        match key {
            UiKey::Esc => self.popup = None,
            UiKey::Enter => {
                let Some(Popup { item, mut choices, selected, .. }) = self.popup.take() else { return };
                let chosen = choices.swap_remove(selected);
                if chosen != self.settings {
                    self.settings = chosen;
                    self.changed(item, host);
                }
            }
            UiKey::Left | UiKey::Right => {
                let fields = self.item().map_or(Vec::new(), |item| item.fields(&self.settings));
                if !fields.is_empty() {
                    let field = self.field_beside(fields.len(), key);
                    if field != self.field {
                        self.field = field;
                        self.open_popup(fields[field]);
                    }
                }
            }
            UiKey::Char(c) => {
                let c = c.to_lowercase().next().unwrap_or(c);
                let n = popup.choices.len();
                let starts = |i: &usize| popup.item.value(&popup.choices[*i], home).to_lowercase().starts_with(c);
                if let Some(i) = (1..=n).map(|k| (popup.selected + k) % n).find(starts) {
                    popup.selected = i;
                }
            }
            _ => {}
        }
    }

    /// Hand changed settings to the emulator and say when they take effect.
    fn changed(&mut self, item: Item, host: &mut dyn Host) {
        let result = host.apply(&self.settings);
        self.status = None;
        match result {
            Err(problem) => self.error(problem),
            Ok(Some(note)) => self.info(note),
            Ok(None) if item.applies() == Applies::NextStart => {
                self.info("Takes effect the next time Rust-DOS starts (F2 saves it)")
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
            // The row after "+ Mount a drive".
            (UiKey::Enter, None) if self.row == self.drives.len() + 1 => self.open_image_dialog(),
            (UiKey::Insert, _) | (UiKey::Enter, None) => self.new_drive(host),
            (UiKey::Enter, Some(info)) if info.kind == DriveKind::Virtual => {
                self.info(format!("Drive {}: is built into Rust-DOS", info.letter()));
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
            (UiKey::Char('b' | 'B'), Some(info)) if bootable(&info) => self.boot(info.drive, host),
            (UiKey::Char('b' | 'B'), Some(info)) => {
                self.error(format!("Drive {}: can't be booted: it isn't a disk image", info.letter()));
            }
            _ => {}
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
                        format!("Drive {}: boots when Rust-DOS starts; F2 saves it", drive_letter(drive))
                    } else {
                        format!("Drive {}: doesn't boot at startup any more; F2 saves it", drive_letter(drive))
                    }
                } else {
                    match host.mount(spec, replace) {
                        Ok(path) => {
                            self.dialog = None;
                            self.refresh_drives(host, Some(drive));
                            let kind = self.drives.get(self.row).map_or("", |d| d.kind.name());
                            let shown = contract_home(&path, self.home.as_deref());
                            let boot = if boots { ", booting at startup once saved (F2)" } else { "" };
                            format!("Drive {}: is mounted as {} {}{}", drive_letter(drive), kind, shown, boot)
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
            Pick::SoundFont => (
                "Pick a SoundFont (.sf2)",
                self.settings.sound.soundfont.as_deref().map(|p| p.display().to_string()).unwrap_or_default(),
                false,
                SOUNDFONTS,
            ),
            Pick::Mt32Roms => (
                "Pick the directory with the MT-32's ROMs",
                self.settings.sound.mt32roms.as_deref().map(|p| p.display().to_string()).unwrap_or_default(),
                true,
                MT32_ROMS,
            ),
            Pick::Awe32Rom => (
                "Pick the AWE32's ROM (awe32.raw)",
                self.settings.sound.awe32rom.as_deref().map(|p| p.display().to_string()).unwrap_or_default(),
                false,
                &["raw", "rom", "bin"][..],
            ),
            Pick::ImportGame => ("Pick a GOG game's folder or a DOSBox .conf", String::new(), true, &["conf"][..]),
            Pick::AchievementsArchive => ("Pick the zip or .dosz the game came in", String::new(), false, &["zip", "dosz"][..]),
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
                    Pick::ImagePath => {
                        if let Some(dialog) = &mut self.image_dialog {
                            dialog.picked(&path, self.home.as_deref());
                        }
                    }
                    Pick::SoundFont => {
                        self.settings.sound.soundfont = Some(path);
                        self.changed(Item::SoundFont, host);
                    }
                    Pick::Awe32Rom => match crate::awe32::rom::load(&path) {
                        Ok(_) => {
                            self.settings.sound.awe32rom = Some(path);
                            self.changed(Item::Awe32Rom, host);
                        }
                        Err(e) => self.error(e),
                    },
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
        let title = " Rust-DOS settings ";
        g.text((cols - title.len()) / 2, 0, title, draw::BRIGHT);
        for row in 1..rows - 1 {
            g.char(0, row, 0xBA, draw::BORDER);
            g.char(cols - 1, row, 0xBA, draw::BORDER);
        }
        g.line(2, 0xC7, 0xC4, 0xB6, draw::BORDER);
        g.line(rows - 4, 0xC7, 0xC4, 0xB6, draw::BORDER);
        g.line(rows - 1, 0xC8, 0xCD, 0xBC, draw::BORDER);
        self.draw_tabs(&mut g);

        // The page, or what is open over it.
        let content = 3..rows - 4;
        self.visible = content.len();
        if self.browser.is_some() {
            self.draw_browser(&mut g, content.clone());
        } else if self.dialog.is_some() {
            self.draw_dialog(&mut g, content.clone());
        } else if self.image_dialog.is_some() {
            self.draw_image_dialog(&mut g, content.clone());
        } else if self.game_dialog.is_some() {
            self.draw_game_dialog(&mut g, content.clone());
        } else if self.autoexec.is_some() {
            self.draw_autoexec(&mut g, content.clone());
        } else if self.rooms.is_some() {
            self.draw_rooms(&mut g, content.clone());
        } else if self.page == Page::Drives {
            self.draw_drives(&mut g, content.clone());
        } else if self.page == Page::Games {
            self.draw_games(&mut g, content.clone());
        } else if self.page == Page::States {
            self.draw_states(&mut g, content.clone());
        } else if self.page == Page::Cheats {
            self.draw_cheats(&mut g, content.clone());
        } else if self.page == Page::Achievements {
            self.draw_achievements(&mut g, content.clone());
        } else if self.page == Page::Stats {
            self.draw_stats(&mut g, content.clone());
        } else {
            self.draw_settings(&mut g, content.clone());
        }
        let help_area = self.draw_help(&mut g, content);

        self.draw_hints(&mut g, rows - 3);
        let status_row = rows - 2;
        match &self.status {
            Some(status) => {
                let color = if status.error { draw::ERROR } else { draw::GOOD };
                g.text_to(2, status_row, &fit(&status.text, cols - 4), color, cols - 2);
            }
            None => {
                let text = match &self.config_file {
                    Some(path) if self.active_game.is_some() => {
                        format!("Game profile: {}", contract_home(path, self.home.as_deref()))
                    }
                    Some(path) => format!("Configuration file: {}", contract_home(path, self.home.as_deref())),
                    None => "No configuration file (--no-config): the settings can't be saved".to_string(),
                };
                g.text_to(2, status_row, &fit(&text, cols - 4), draw::DIM, cols - 2);
            }
        }

        draw::render(&g, &layout, frame);
        for plot in std::mem::take(&mut self.plots) {
            draw::plot(frame, &layout, plot.cells, &plot.values, plot.max, plot.color, draw::OPAQUE);
        }
        self.draw_pictures(frame, &layout);
        if let Some(area) = help_area {
            draw::render_area(&g, &layout, frame, draw::PANEL_ALPHA, area);
        }
        self.layout = Some(layout);
    }

    /// The page tabs on row 1: with a space around each title where they
    /// fit, without where that fits, and else as many as fit around the
    /// selected one, with ◄ and ► for those left out.
    fn draw_tabs(&mut self, g: &mut Grid) {
        let end = g.cols - 1;
        let room = end - 2;
        let titles: Vec<&str> = PAGES.iter().map(|p| p.title()).collect();
        let width = |pad: usize, pages: &[&str]| pages.iter().map(|t| t.len() + 2 * pad + 1).sum::<usize>().saturating_sub(1);
        let pad = if width(1, &titles) <= room { 1 } else { 0 };
        let selected = PAGES.iter().position(|&p| p == self.page).unwrap_or(0);
        // The first tab shown: the selected one and the one after it (so
        // it shows there are more) fit after it, with room for the arrows.
        let last = (selected + 1).min(titles.len() - 1);
        let mut first = 0;
        if width(pad, &titles) > room {
            while first < selected && width(pad, &titles[first..=last]) + 4 > room {
                first += 1;
            }
        }
        let mut x = 2;
        if first > 0 {
            g.char(x, 1, 0x11, draw::KEY);
            self.hits.push(Hit { row: 1, col: x, width: 1, target: Target::Key(UiKey::BackTab) });
            x += 2;
        }
        for (i, &page) in PAGES.iter().enumerate().skip(first) {
            let label = format!("{:pad$}{}{:pad$}", "", page.title(), "", pad = pad);
            let more = i + 1 < PAGES.len();
            // Room for this one, and the ► if more follow.
            if x + label.len() + if more { 2 } else { 0 } > end && i > selected {
                g.char(end - 1, 1, 0x10, draw::KEY);
                self.hits.push(Hit { row: 1, col: end - 1, width: 1, target: Target::Key(UiKey::Tab) });
                break;
            }
            let is_selected = page == self.page;
            if is_selected {
                g.background(x, 1, label.len().min(end - x), draw::SELECT);
            }
            let after = g.text_to(x, 1, &label, if is_selected { draw::BRIGHT } else { draw::TEXT }, end);
            self.hits.push(Hit { row: 1, col: x, width: after - x, target: Target::Tab(page) });
            x = after + 1;
        }
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

    /// A scroll bar left of the right border, beside the `list` rows that
    /// show `total` rows from `scroll` on, if they don't all fit. A click
    /// above or below its thumb pages up or down.
    fn draw_scrollbar(&mut self, g: &mut Grid, list: std::ops::Range<usize>, scroll: usize, total: usize) {
        let height = list.len();
        if height == 0 || total <= height {
            return;
        }
        let col = g.cols - 2;
        let thumb = (height * height).div_ceil(total).clamp(1, height);
        let hidden = total - height;
        let top = (scroll.min(hidden) * (height - thumb) + hidden / 2) / hidden;
        for (i, row) in list.enumerate() {
            let key = match i {
                _ if i < top => UiKey::PageUp,
                _ if i >= top + thumb => UiKey::PageDown,
                _ => {
                    g.char(col, row, 0xDB, draw::BORDER);
                    continue;
                }
            };
            g.char(col, row, 0xB0, draw::DIM);
            self.hits.push(Hit { row, col, width: 1, target: Target::Key(key) });
        }
    }

    fn draw_drives(&mut self, g: &mut Grid, content: std::ops::Range<usize>) {
        let cols = g.cols;
        Self::keep_visible(&mut self.scroll, self.row, content.len());
        let label_col = cols - 21;
        let path_width = label_col.saturating_sub(15);
        for (i, row) in (self.scroll..self.row_count()).zip(content.clone()) {
            if i == self.row {
                self.select_row(g, row);
            }
            self.hits.push(Hit { row, col: 1, width: cols - 2, target: Target::Row(i) });
            let Some(info) = self.drives.get(i) else {
                let text = match i - self.drives.len() {
                    0 if self.frontend.host_files => "+ Mount a drive...",
                    0 => "+ Insert a disk or CD image...",
                    _ => "+ Create a disk image...",
                };
                g.text(2, row, text, draw::KEY);
                continue;
            };
            let builtin = info.kind == DriveKind::Virtual;
            let (fg, dim) = if builtin { (draw::DIM, draw::DIM) } else { (draw::TEXT, draw::BRIGHT) };
            g.text(2, row, &format!("{}:", info.letter()), dim);
            g.text(6, row, info.kind.name(), fg);
            let mut path = match info.image.as_ref().or(info.root.as_ref()) {
                Some(path) => contract_home(path, self.home.as_deref()),
                None => "(built into Rust-DOS)".to_string(),
            };
            if info.images.len() > 1 {
                path = format!("({}/{}) {}", info.image_index + 1, info.images.len(), path);
            }
            g.text_to(14, row, &fit(&path, path_width), fg, label_col - 1);
            g.text_to(label_col, row, &info.label, fg, cols - 9);
            let boots = info.mount.as_ref().is_some_and(|m| m.opts.boot);
            let flags = match (boots, info.read_only && !builtin) {
                (true, true) => "boot ro",
                (true, false) => "boot",
                (false, true) => "ro",
                (false, false) => "",
            };
            g.text(cols - 2 - flags.len(), row, flags, fg);
        }
        self.draw_scrollbar(g, content, self.scroll, self.row_count());
    }

    fn draw_settings(&mut self, g: &mut Grid, content: std::ops::Range<usize>) {
        let cols = g.cols;
        let items = self.items();
        Self::keep_visible(&mut self.scroll, self.row, content.len());
        let value_col = 27.min(cols / 2);
        let note_col = cols - 13;
        // Where the selected setting's value is, for its popup list.
        let mut anchor = None;
        for (i, row) in (self.scroll..items.len()).zip(content.clone()) {
            let item = items[i];
            let selected = i == self.row;
            if selected {
                self.select_row(g, row);
            }
            self.hits.push(Hit { row, col: 1, width: cols - 2, target: Target::Row(i) });
            if item.input() == Input::Link {
                g.text_to(2, row, item.label(), draw::KEY, note_col - 1);
            } else {
                g.text_to(2, row, item.label(), if selected { draw::BRIGHT } else { draw::TEXT }, value_col - 1);
            }
            let note = match item.applies() {
                Applies::Now => "",
                Applies::AtPrompt => "at prompt",
                Applies::NowAndAtPrompt => "now+prompt",
                Applies::NextStart => "next start",
            };
            match item {
                Item::Volume(Channel::Master) if self.muted => {
                    g.text(note_col + 1, row, "muted", draw::ERROR);
                }
                Item::Volume(channel) => self.draw_meter(g, note_col + 1, row, self.levels[channel as usize]),
                _ => {
                    g.text(note_col + 1, row, note, draw::NOTE);
                }
            }

            let end = note_col.saturating_sub(1);
            if item.input() == Input::Link {
                continue;
            }
            // Its list's values under its value.
            if selected {
                anchor = Some((value_col, row));
            }
            let fields = item.fields(&self.settings);
            if !fields.is_empty() {
                let focus = self.draw_fields(g, row, i, &fields, (value_col, end), selected);
                if selected && let Some(col) = focus {
                    anchor = Some((col - 1, row));
                }
                continue;
            }
            if selected && let Some(field) = &self.edit {
                let width = end.saturating_sub(value_col);
                let (text, cursor) = field.view(width);
                g.background(value_col, row, width, draw::FIELD);
                g.text_to(value_col, row, &text, draw::BRIGHT, end);
                g.background(value_col + cursor, row, 1, draw::SELECT);
                continue;
            }
            let value = fit(&item.value(&self.settings, self.home.as_deref()), end.saturating_sub(value_col + 4));
            // What Left and Right step has their arrows; a value picked
            // from a list or typed is a button too.
            if matches!(item.input(), Input::Choice | Input::Slider | Input::Presets) {
                g.char(value_col, row, 0x11, draw::KEY);
                self.hits.push(Hit { row, col: value_col, width: 1, target: Target::Step(i, -1) });
                let after = g.text_to(value_col + 2, row, &value, draw::BRIGHT, end);
                if item.input() != Input::Slider {
                    self.hits.push(Hit { row, col: value_col + 2, width: after - value_col - 2, target: Target::Button(i) });
                }
                g.char(after + 1, row, 0x10, draw::KEY);
                self.hits.push(Hit { row, col: after + 1, width: 1, target: Target::Step(i, 1) });
            } else {
                let after = Self::draw_button(g, value_col, row, &value, end, false);
                self.hits.push(Hit { row, col: value_col, width: after - value_col, target: Target::Button(i) });
            }
        }
        self.draw_scrollbar(g, content.clone(), self.scroll, items.len());
        if let Some(anchor) = anchor {
            self.draw_popup(g, content, anchor);
        }
    }

    /// The popup list of values from (`col`, `row`), where the value it
    /// picks is: below it, or above it where it doesn't fit below, else as
    /// low in the rows `content` as it fits, scrolled where it is longer
    /// than they are.
    fn draw_popup(&mut self, g: &mut Grid, content: std::ops::Range<usize>, (col, row): (usize, usize)) {
        let home = self.home.as_deref();
        let Some(popup) = &mut self.popup else { return };
        let labels: Vec<String> = popup.choices.iter().map(|s| popup.item.value(s, home)).collect();
        let height = (labels.len() + 2).min(content.len());
        let top = if row + 1 + height <= content.end {
            row + 1
        } else if row >= content.start + height {
            row - height
        } else {
            content.end - height
        };
        let list = height.saturating_sub(2);
        if list == 0 {
            return;
        }
        popup.visible = list;
        Self::keep_visible(&mut popup.scroll, popup.selected, list);

        // Framed, with a space either side of the values, left of the
        // window's right border.
        let longest = labels.iter().map(|l| l.chars().count()).max().unwrap_or(0);
        let width = (longest + 4).min(g.cols - 2);
        let left = col.min(g.cols - 1 - width);
        let right = left + width - 1;
        let bottom = top + height - 1;
        for r in top..=bottom {
            g.background(left, r, width, draw::FIELD);
            g.text_to(left, r, &" ".repeat(width), draw::TEXT, right + 1);
            g.char(left, r, 0xB3, draw::BORDER);
            g.char(right, r, 0xB3, draw::BORDER);
            self.hits.push(Hit { row: r, col: left, width, target: Target::Popup });
        }
        for x in left + 1..right {
            g.char(x, top, 0xC4, draw::BORDER);
            g.char(x, bottom, 0xC4, draw::BORDER);
        }
        g.char(left, top, 0xDA, draw::BORDER);
        g.char(right, top, 0xBF, draw::BORDER);
        g.char(left, bottom, 0xC0, draw::BORDER);
        g.char(right, bottom, 0xD9, draw::BORDER);
        // ▲ and ▼ where there are more above and below.
        if popup.scroll > 0 {
            g.char(right - 2, top, 0x1E, draw::KEY);
        }
        if popup.scroll + list < labels.len() {
            g.char(right - 2, bottom, 0x1F, draw::KEY);
        }

        for (i, r) in (popup.scroll..labels.len()).zip(top + 1..bottom) {
            let selected = i == popup.selected;
            if selected {
                g.background(left + 1, r, width - 2, draw::SELECT);
            }
            let text = fit(&labels[i], width.saturating_sub(4));
            g.text_to(left + 2, r, &text, if selected { draw::BRIGHT } else { draw::TEXT }, right - 1);
            self.hits.push(Hit { row: r, col: left + 1, width: width - 2, target: Target::PopupRow(i) });
        }
    }

    /// A button at (`col`, `row`), up to the column `end`: `value` in
    /// brackets, which Enter or a click changes. Returns the column after
    /// it.
    fn draw_button(g: &mut Grid, col: usize, row: usize, value: &str, end: usize, focused: bool) -> usize {
        if focused {
            g.background(col, row, (value.chars().count() + 2).min(end.saturating_sub(col)), draw::FIELD);
        }
        let after = g.text_to(col, row, "[", draw::KEY, end);
        let after = g.text_to(after, row, value, draw::BRIGHT, end);
        g.text_to(after, row, "]", draw::KEY, end)
    }

    /// The fields of row `i`, a row of several, side by side in the
    /// columns `cols` of grid row `row`: each by its name with its value
    /// on a button of its own, the one Left and Right go to marked while
    /// the row is selected. Returns the column of that one's button, if it
    /// fit.
    fn draw_fields(
        &mut self,
        g: &mut Grid,
        row: usize,
        i: usize,
        fields: &[Item],
        cols: (usize, usize),
        selected: bool,
    ) -> Option<usize> {
        let (mut x, end) = cols;
        let focus = self.field.min(fields.len() - 1);
        let mut focus_col = None;
        for (f, field) in fields.iter().enumerate() {
            if f > 0 {
                x += 2;
            }
            let name = field.label();
            if !name.is_empty() {
                x = g.text_to(x, row, name, draw::TEXT, end) + 1;
            }
            let value = field.value(&self.settings, self.home.as_deref());
            if x + value.chars().count() + 2 > end {
                break;
            }
            if f == focus {
                focus_col = Some(x);
            }
            let after = Self::draw_button(g, x, row, &value, end, selected && f == focus);
            self.hits.push(Hit { row, col: x, width: after - x, target: Target::RowField(i, f) });
            x = after;
        }
        focus_col
    }

    /// A level meter of `METER` cells at (`col`, `row`): 6 dB a cell
    /// from -60 dB, green, then yellow for the loudest, red where the
    /// sound clips.
    fn draw_meter(&self, g: &mut Grid, col: usize, row: usize, level: f32) {
        const METER: usize = 10;
        let lit = if level <= 0.0 {
            0
        } else {
            let db = 20.0 * level.log10();
            ((db + 60.0) / 6.0).ceil().clamp(0.0, METER as f32) as usize
        };
        for cell in 0..METER {
            let color = match cell {
                _ if cell >= lit => draw::DIM,
                _ if cell == METER - 1 && level >= 1.0 => draw::ERROR,
                _ if cell >= METER - 3 => draw::KEY,
                _ => draw::GOOD,
            };
            g.char(col + cell, row, if cell < lit { 0xFE } else { 0xFA }, color);
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
            g.text(value_col + 14, top + 5, "(default empty)", draw::DIM);
        }
        let buttons = dialog.fields();
        if buttons.contains(&Field::BootFlag) {
            let boot = if dialog.boot { "yes" } else { "no" }.to_string();
            put(g, Field::BootFlag, top + 7, "Auto-boot", &choice(boot, top + 7, true));
            if top + 7 < bottom {
                g.text(value_col + 10, top + 7, "(disk images only)", draw::DIM);
            }
        }
        let mut col = value_col;
        for (field, text) in [
            (Field::Mount, "[ Mount ]"),
            (Field::Boot, "[ Boot ]"),
            (Field::Unmount, "[ Unmount ]"),
            (Field::Cancel, "[ Cancel ]"),
        ] {
            if buttons.contains(&field) {
                put(g, field, top + 9, "", &button(text, top + 9, col));
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
        let (scroll, total) = (self.browser_scroll, browser.rows());
        let separator = std::path::MAIN_SEPARATOR;
        for (i, row) in (self.browser_scroll..browser.rows()).zip(list.clone()) {
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
        self.draw_scrollbar(g, list, scroll, total);
    }

    /// The help topic on what is under the cursor: the setting selected,
    /// or the page, or the dialog open over it.
    fn help_topic(&self) -> Option<&'static str> {
        if let Some((_, pick)) = &self.browser {
            return Some(pick.help());
        }
        if self.dialog.is_some() {
            Some("mount")
        } else if self.image_dialog.is_some() {
            Some("new-image")
        } else if self.game_dialog.is_some() {
            Some("new-game")
        } else if self.autoexec.is_some() {
            Some("autoexec")
        } else if self.rooms.is_some() {
            Some("rooms")
        } else {
            self.page.help().or_else(|| self.item().map(Item::help))
        }
    }

    /// F1: show the help on what is under the cursor, or close it.
    fn toggle_help(&mut self) {
        if self.help.take().is_none() {
            self.help = self.help_topic().and_then(help::topic).map(help::HelpView::new);
        }
    }

    /// The help's keys: Up, Down and the rest scroll it, and Esc, Enter
    /// and F1 close it.
    fn help_key(&mut self, key: UiKey) {
        let Some(help) = &mut self.help else { return };
        let (last, page) = (help.max_scroll(), help.visible.saturating_sub(1).max(1));
        help.scroll = match key {
            UiKey::Up => help.scroll.saturating_sub(1),
            UiKey::Down => (help.scroll + 1).min(last),
            UiKey::PageUp => help.scroll.saturating_sub(page),
            UiKey::PageDown | UiKey::Char(' ') => (help.scroll + page).min(last),
            UiKey::Home => 0,
            UiKey::End => last,
            UiKey::Esc | UiKey::Enter | UiKey::Backspace => {
                self.help = None;
                return;
            }
            _ => return,
        };
    }

    /// The help over the rows `content`, from their top: its topic's title
    /// in the frame, its text below, scrolled, with ▲ and ▼ where there is
    /// more. Returns the columns and rows it covers, for the pictures and
    /// graphs drawn in pixels to leave alone.
    fn draw_help(&mut self, g: &mut Grid, content: std::ops::Range<usize>) -> Option<(std::ops::Range<usize>, std::ops::Range<usize>)> {
        let help = self.help.as_mut()?;
        let width = (g.cols - 2).min(76);
        if content.len() < 3 || width < 12 {
            return None;
        }
        let left = (g.cols - width) / 2;
        let right = left + width - 1;
        // As high as its text, up to the page's rows.
        let lines = help::layout(help.topic.body, width - 4);
        let top = content.start;
        let bottom = (top + lines.len() + 1).min(content.end - 1);
        for r in top..=bottom {
            g.background(left, r, width, draw::FIELD);
            g.text_to(left, r, &" ".repeat(width), draw::TEXT, right + 1);
            g.char(left, r, 0xB3, draw::BORDER);
            g.char(right, r, 0xB3, draw::BORDER);
            self.hits.push(Hit { row: r, col: left, width, target: Target::Help });
        }
        for x in left + 1..right {
            g.char(x, top, 0xC4, draw::BORDER);
            g.char(x, bottom, 0xC4, draw::BORDER);
        }
        g.char(left, top, 0xDA, draw::BORDER);
        g.char(right, top, 0xBF, draw::BORDER);
        g.char(left, bottom, 0xC0, draw::BORDER);
        g.char(right, bottom, 0xD9, draw::BORDER);
        let title = fit(&format!(" {} ", help.topic.title), width.saturating_sub(8));
        g.text(left + 2, top, &title, draw::BRIGHT);

        help.lines = lines.len();
        help.visible = bottom - top - 1;
        help.scroll = help.scroll.min(help.max_scroll());
        for (line, r) in lines.iter().skip(help.scroll).zip(top + 1..bottom) {
            let mut x = left + 2;
            for span in line {
                x = g.text_to(x, r, &span.text, span.color, right - 1);
            }
        }
        if help.scroll > 0 {
            g.char(right - 2, top, 0x1E, draw::KEY);
            self.hits.push(Hit { row: top, col: right - 2, width: 1, target: Target::Key(UiKey::PageUp) });
        }
        if help.scroll < help.max_scroll() {
            g.char(right - 2, bottom, 0x1F, draw::KEY);
            self.hits.push(Hit { row: bottom, col: right - 2, width: 1, target: Target::Key(UiKey::PageDown) });
        }
        Some((left..right + 1, top..bottom + 1))
    }

    /// The key hints for what is showing, each clickable.
    fn draw_hints(&mut self, g: &mut Grid, row: usize) {
        use UiKey::*;
        let mut hints: Vec<(&str, &str, UiKey)> = if self.help.is_some() {
            vec![("\u{2191}\u{2193}", "Scroll", Down), ("Esc", "Close", Esc)]
        } else if self.browser.is_some() {
            vec![("Enter", "Open", Enter), ("Bksp", "Up", Backspace), ("Esc", "Cancel", Esc)]
        } else if self.dialog.is_some() {
            let boot = self.dialog.as_ref().is_some_and(|d| d.focus == Field::Boot);
            vec![("Tab", "Next", Tab), ("Enter", if boot { "Boot" } else { "Mount" }, Enter), ("Esc", "Cancel", Esc)]
        } else if self.game_dialog.is_some() || self.image_dialog.is_some() {
            vec![("Tab", "Next", Tab), ("Enter", "Create", Enter), ("Esc", "Cancel", Esc)]
        } else if self.autoexec.is_some() {
            vec![("F2", "Save", Save), ("Esc", "Cancel", Esc)]
        } else if self.rooms.is_some() {
            self.room_hints()
        } else if self.confirm_delete.is_some() {
            vec![("Enter", "Delete", Enter), ("Esc", "Keep", Esc)]
        } else if self.page == Page::States {
            vec![
                ("Enter", "Load", Enter),
                ("Ins", "Save", Insert),
                ("Del", "Delete", Delete),
                ("Tab", "Page", Tab),
                ("Esc", "Close", Esc),
            ]
        } else if self.popup.is_some() {
            let mut hints = vec![("\u{2191}\u{2193}", "Select", Down), ("Enter", "OK", Enter), ("Esc", "Cancel", Esc)];
            if self.item().is_some_and(|item| !item.fields(&self.settings).is_empty()) {
                hints.insert(1, ("\u{2190}\u{2192}", "Field", Right));
            }
            hints
        } else if self.edit.is_some() || self.cheats.edit.is_some() || self.achievements.edit.is_some() {
            vec![("Enter", "OK", Enter), ("Esc", "Cancel", Esc)]
        } else if self.page == Page::Cheats {
            self.cheats_hints()
        } else if self.page == Page::Achievements {
            self.achievements_hints()
        } else if self.page == Page::Stats {
            let overlay = if self.overlay { "Hide overlay" } else { "Show overlay" };
            vec![("Tab", "Page", Tab), ("Esc", "Close", Esc), ("Ctrl+Shift+F12", overlay, Overlay)]
        } else if self.page == Page::Games {
            vec![
                ("Enter", "Launch", Enter),
                ("Ins", "New", Insert),
                ("Del", "Delete", Delete),
                ("Tab", "Page", Tab),
                ("F2", "Save", Save),
                ("Esc", "Close", Esc),
            ]
        } else if self.page == Page::Drives {
            let (mount, unmount) = if self.frontend.host_files { ("Mount", "Unmount") } else { ("Insert", "Eject") };
            let mut hints = vec![
                ("Enter", "Change", Enter),
                ("Ins", mount, Insert),
                ("Del", unmount, Delete),
                ("Tab", "Page", Tab),
                ("F2", "Save", Save),
                ("Esc", "Close", Esc),
            ];
            if self.drives.get(self.row).is_some_and(bootable) {
                hints.insert(3, ("B", "Boot", Char('b')));
            }
            hints
        } else {
            let several = self.item().is_some_and(|item| !item.fields(&self.settings).is_empty());
            let mut hints = match self.item().map(Item::input) {
                _ if several => vec![("\u{2190}\u{2192}", "Field", Right), ("Enter", "List", Enter)],
                Some(Input::Choice) => vec![("\u{2190}\u{2192}", "Change", Right), ("Enter", "List", Enter)],
                Some(Input::Slider | Input::Presets) => {
                    vec![("\u{2190}\u{2192}", "Change", Right), ("Enter", "Type", Enter)]
                }
                Some(Input::Text) => vec![("Enter", "Type", Enter)],
                Some(Input::File) => vec![("Enter", "Pick", Enter), ("Del", "None", Delete)],
                Some(Input::Link) if self.item() == Some(Item::Rooms) => vec![("Enter", "Open", Enter)],
                Some(Input::Link) => vec![("Enter", "Edit", Enter)],
                None => Vec::new(),
            };
            hints.extend([("Tab", "Page", Tab), ("F2", "Save", Save), ("Esc", "Close", Esc)]);
            hints
        };
        if self.help.is_none() && self.help_topic().is_some() {
            hints.push(("F1", "Help", Help));
        }
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

/// Whether a drive is a disk image a system can boot from.
fn bootable(info: &DriveInfo) -> bool {
    info.image.is_some() && matches!(info.kind, DriveKind::HardDisk | DriveKind::Floppy)
}
