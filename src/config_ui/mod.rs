//! The settings window: a semi-transparent panel over the picture, opened
//! with Ctrl+F12 or the DOSCONFIG command. It changes the settings, mounts,
//! swaps and unmounts drives, and saves both to the configuration file.
//!
//! It knows nothing of SDL, the browser or the machine: the frontend feeds
//! it keys, typed text and clicks, and carries out what it asks for through
//! `Host`. What the frontend doesn't have (`Frontend`) isn't offered.

mod autoexec;
mod browser;
mod cheats;
mod dialog;
mod draw;
mod games;
pub mod osd;
mod states;

use autoexec::AutoexecEditor;
use browser::{Browser, IMAGES, MT32_ROMS, Row, SOUNDFONTS};
use dialog::{Event, Field, MountDialog, TextField};
use draw::{Grid, Layout, Rgb};
pub use draw::cp437;
use games::{GameDialog, GameField};
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
    /// The values the machine keeps frozen, and new ones.
    fn freezes(&self) -> Vec<crate::cheats::Freeze> {
        Vec::new()
    }
    fn set_freezes(&mut self, freezes: Vec<crate::cheats::Freeze>) {
        let _ = freezes;
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Drives,
    Display,
    Emulator,
    Sound,
    Mixer,
    Games,
    States,
    Cheats,
    Stats,
}

const PAGES: [Page; 9] = [
    Page::Drives,
    Page::Display,
    Page::Emulator,
    Page::Sound,
    Page::Mixer,
    Page::Games,
    Page::States,
    Page::Cheats,
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
            Page::Games => "Games",
            Page::States => "States",
            Page::Cheats => "Cheats",
            Page::Stats => "Stats",
        }
    }

    fn items(self) -> &'static [Item] {
        use Item::*;
        match self {
            Page::Drives | Page::Games | Page::States | Page::Cheats | Page::Stats => &[],
            Page::Display => {
                &[Scale, Fullscreen, Aspect, Filter, Shader, CrtCurvature, CrtGlow, Monochrome, Composite, CompositeEra]
            }
            Page::Emulator => &[
                Cycles, Core, Cpu, Machine, Memsize, Ems, Umb, HardDiskSpeed, FloppyDiskSpeed, Joystick,
                Deadzone, KeyboardLayout, Rewind, RewindMemory, CaptureDir, Autoexec,
            ],
            Page::Sound => &[
                SbType, SbBase, SbIrq, SbDma, SbHdma, Opl, Gus, GusBase, GusIrq, GusDma, GusDrive, UltraDir, Midi,
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
                SpeakerFilter,
                SbFilter,
                Reverb,
                ReverbMix,
                Chorus,
                ChorusMix,
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
    /// Left and right step through the values.
    Choice,
    /// Steps, and Enter types a value.
    ChoiceOrText,
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
    Memsize,
    /// Expanded memory and upper memory blocks.
    Ems,
    Umb,
    KeyboardLayout,
    /// Rewind (held Alt+F11) and the memory it takes.
    Rewind,
    RewindMemory,
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
    /// Where screenshots and recordings go.
    CaptureDir,
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
const REWIND_MEMORY: [u32; 7] = [64, 128, 256, 512, 1024, 2048, 4096];

/// A percentage of up to `max` as a number and a bar of small squares, one
/// for every 10%, which stay apart from the next row's:
/// "120% ■■■■■■■■■■■■········".
fn percent_bar(percent: u16, max: u16) -> String {
    let (full, cells) = ((percent.min(max) / 10) as usize, (max / 10) as usize);
    format!("{:>3}% {}{}", percent, "■".repeat(full), "·".repeat(cells - full))
}

/// A percentage stepped left or right to the next ten, from a value in
/// between to the ten on that side, within 0 to `max`.
fn step_tens(percent: u16, dir: isize, max: u16) -> u16 {
    let tens = if dir > 0 { percent / 10 + 1 } else { percent.div_ceil(10).saturating_sub(1) };
    (tens * 10).min(max)
}

fn on_off(on: bool) -> String {
    if on { "on" } else { "off" }.to_string()
}

/// Whether General MIDI can play through a SoundFont: one picked from the
/// host's files, with the synthesizer built in.
fn soundfonts(frontend: Frontend) -> bool {
    cfg!(feature = "midi") && frontend.host_files
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
            Ems => "Expanded memory (EMS)",
            Umb => "Upper memory (UMB)",
            KeyboardLayout => "Keyboard layout",
            Rewind => "Rewind (Alt+F11)",
            RewindMemory => "  Rewind memory",
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
            Mt32Roms => "MT-32 ROMs",
            Mt32Model => "MT-32 model",
            MidiPort => "MIDI port",
            HardDiskSpeed => "Hard disk speed",
            FloppyDiskSpeed => "Floppy disk speed",
            HardDiskNoise => "Hard disk noise",
            FloppyDiskNoise => "Floppy disk noise",
            Volume(channel) => channel.label(),
            CaptureDir => "Capture folder",
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
        }
    }

    /// Whether the frontend has what the setting needs.
    fn available(self, frontend: Frontend) -> bool {
        match self {
            Item::Scale | Item::Fullscreen => frontend.window,
            Item::SoundFont => soundfonts(frontend),
            Item::Mt32Roms | Item::Mt32Model => mt32(frontend),
            Item::MidiPort => host_midi(frontend),
            Item::CaptureDir => frontend.host_files,
            Item::Core => crate::dynrec::AVAILABLE,
            // A thread of its own packs rewind's states.
            Item::Rewind | Item::RewindMemory => frontend.window,
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
            _ => true,
        }
    }

    fn applies(self) -> Applies {
        use Item::*;
        match self {
            Scale | Fullscreen | Aspect | Filter | Shader | CrtCurvature | CrtGlow | Composite | CompositeEra => {
                Applies::Now
            }
            Cycles | Core | KeyboardLayout | Rewind | RewindMemory => Applies::Now,
            Monochrome => Applies::NowAndAtPrompt,
            HardDiskSpeed | FloppyDiskSpeed | HardDiskNoise | FloppyDiskNoise | Volume(_) | CaptureDir => Applies::Now,
            Joystick | Deadzone | SpeakerFilter | SbFilter | Reverb | Chorus | ReverbMix | ChorusMix => Applies::Now,
            Memsize | Autoexec => Applies::NextStart,
            _ => Applies::AtPrompt,
        }
    }

    fn input(self) -> Input {
        match self {
            Item::Cycles | Item::Volume(_) | Item::Deadzone | Item::ReverbMix | Item::ChorusMix => Input::ChoiceOrText,
            Item::CrtCurvature | Item::CrtGlow => Input::ChoiceOrText,
            Item::UltraDir | Item::CaptureDir => Input::Text,
            Item::SoundFont | Item::Mt32Roms => Input::File,
            Item::Autoexec => Input::Link,
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
            },
            Core => match s.core {
                CoreMode::Auto => "auto (recomp. in prot. mode)",
                CoreMode::Dynamic => "dynamic recompiler",
                CoreMode::Normal => "normal (interpreter)",
            }
            .to_string(),
            Cpu => match s.cpu {
                CpuModel::I386 => "386",
                CpuModel::I486 => "486",
            }
            .to_string(),
            Machine => s.machine.describe().to_string(),
            Memsize => format!("{} MB", s.memsize),
            Ems => on_off(s.ems),
            Umb => on_off(s.umb),
            KeyboardLayout => s.keyboard_layout.describe(),
            Rewind => on_off(s.rewind),
            RewindMemory => format!("{} MB", s.rewind_memory),
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
            Joystick => s.joystick.kind.describe().to_string(),
            Deadzone => format!("{}%", s.joystick.deadzone),
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
            Autoexec => String::new(),
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
            CrtCurvature => s.crt.curvature = step_tens(s.crt.curvature, dir, MAX_AMOUNT),
            CrtGlow => s.crt.glow = step_tens(s.crt.glow, dir, MAX_AMOUNT),
            Monochrome => s.monochrome = cycle(&crate::video::mono::Monochrome::ALL, s.monochrome, dir),
            Composite => {
                s.composite.mode = cycle(&crate::video::composite::CompositeMode::ALL, s.composite.mode, dir)
            }
            CompositeEra => {
                s.composite.era = cycle(&crate::video::composite::CompositeEra::ALL, s.composite.era, dir)
            }
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
            Core => s.core = cycle(&[CoreMode::Auto, CoreMode::Dynamic, CoreMode::Normal], s.core, dir),
            Cpu => s.cpu = cycle(&[CpuModel::I386, CpuModel::I486], s.cpu, dir),
            Machine => s.machine = cycle(&crate::video::adapter::Adapter::ALL, s.machine, dir),
            Memsize => s.memsize = step_number(&MEMSIZES, s.memsize as u32, dir) as usize,
            Ems => s.ems = !s.ems,
            Umb => s.umb = !s.umb,
            KeyboardLayout => s.keyboard_layout = cycle(&crate::keylayout::LayoutSetting::all(), s.keyboard_layout, dir),
            Rewind => s.rewind = !s.rewind,
            RewindMemory => s.rewind_memory = step_number(&REWIND_MEMORY, s.rewind_memory as u32, dir) as usize,
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
                sound.midisynth = cycle(&synths, sound.midisynth, dir);
            }
            Mt32Model => sound.mt32model = cycle(&crate::config::Mt32Model::ALL, sound.mt32model, dir),
            // The ports there are now, and the first of them (empty).
            MidiPort => {
                let mut ports = vec![String::new()];
                ports.extend(midi_ports());
                let at = ports.iter().position(|p| *p == sound.midiport);
                let next = match at {
                    Some(i) => (i as isize + dir).rem_euclid(ports.len() as isize) as usize,
                    None => 0,
                };
                sound.midiport = ports.swap_remove(next);
            }
            HardDiskSpeed => s.disk.hard_disk_speed = cycle(&DiskSpeed::ALL, s.disk.hard_disk_speed, dir),
            FloppyDiskSpeed => s.disk.floppy_disk_speed = cycle(&DiskSpeed::ALL, s.disk.floppy_disk_speed, dir),
            HardDiskNoise => s.disk.hard_disk_noise = cycle(&NoiseMode::ALL, s.disk.hard_disk_noise, dir),
            FloppyDiskNoise => s.disk.floppy_disk_noise = cycle(&NoiseMode::ALL, s.disk.floppy_disk_noise, dir),
            // In tens of percent, from a value in between to the next ten.
            Volume(channel) => s.mixer.set_level(channel, step_tens(s.mixer.level(channel), dir, MAX_LEVEL)),
            ReverbMix => s.mixer.reverb_mix = step_tens(s.mixer.reverb_mix, dir, MAX_MIX),
            ChorusMix => s.mixer.chorus_mix = step_tens(s.mixer.chorus_mix, dir, MAX_MIX),
            Joystick => s.joystick.kind = cycle(&JoystickType::ALL, s.joystick.kind, dir),
            SpeakerFilter => s.mixer.speaker_filter = !s.mixer.speaker_filter,
            Item::SbFilter => {
                use crate::mixer::SbFilter as Filter;
                s.mixer.sb_filter = cycle(&[Filter::Auto, Filter::Off], s.mixer.sb_filter, dir)
            }
            Reverb => s.mixer.reverb = cycle(&ReverbPreset::ALL, s.mixer.reverb, dir),
            Chorus => s.mixer.chorus = cycle(&ChorusPreset::ALL, s.mixer.chorus, dir),
            LptDac => sound.lpt_dac = cycle(&crate::lpt_dac::LptDacType::ALL, sound.lpt_dac, dir),
            TandySound => sound.tandy = cycle(&crate::sn76489::TandySound::ALL, sound.tandy, dir),
            // In fives of percent.
            Deadzone => {
                let dz = s.joystick.deadzone as isize;
                let fives = if dir > 0 { dz / 5 + 1 } else { (dz + 4) / 5 - 1 };
                s.joystick.deadzone = (fives * 5).clamp(0, MAX_DEADZONE as isize) as u8;
            }
            UltraDir | SoundFont | Mt32Roms | CaptureDir | Autoexec => {}
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
            Item::Volume(channel) => s.mixer.level(channel).to_string(),
            Item::CaptureDir => s.capture_dir.display().to_string(),
            Item::Deadzone => s.joystick.deadzone.to_string(),
            Item::CrtCurvature => s.crt.curvature.to_string(),
            Item::CrtGlow => s.crt.glow.to_string(),
            Item::ReverbMix => s.mixer.reverb_mix.to_string(),
            Item::ChorusMix => s.mixer.chorus_mix.to_string(),
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
            Item::CaptureDir => s.capture_dir = expand_host_path(text, Path::new(""), dirs::home_dir().as_deref()),
            Item::Deadzone => s.joystick.deadzone = crate::joystick::parse_deadzone(text)?,
            Item::CrtCurvature => s.crt.curvature = parse_amount(text).ok_or("The curvature goes from 0 to 100%")?,
            Item::CrtGlow => s.crt.glow = parse_amount(text).ok_or("The glow goes from 0 to 100%")?,
            Item::ReverbMix => s.mixer.reverb_mix = crate::mixer::parse_mix(text)?,
            Item::ChorusMix => s.mixer.chorus_mix = crate::mixer::parse_mix(text)?,
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
    /// A game set up for DOSBox, to import.
    ImportGame,
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
    GameField(GameField),
    BrowserRow(usize),
    /// A line of the `[autoexec]` editor.
    EditorLine(usize),
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
    /// What the Stats page shows (`set_stats`), and the graphs the last
    /// frame drew as text, drawn over it in pixels.
    stats: Option<crate::stats::StatsView>,
    plots: Vec<Plot>,
    /// The States page: the slots, whether there is anywhere to keep
    /// states, and the pictures the last frame drew in pixels (the cell
    /// of their top left corner).
    states: Vec<SlotView>,
    states_available: bool,
    pictures: Vec<((usize, usize), Frame)>,
    /// The `[autoexec]` commands being edited.
    autoexec: Option<AutoexecEditor>,
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
            levels: [0.0; CHANNELS],
            muted: false,
            games: Vec::new(),
            active_game: None,
            game_dialog: None,
            confirm_delete: None,
            notice: None,
            cheats: cheats::Cheats::default(),
            stats: None,
            plots: Vec::new(),
            states: Vec::new(),
            states_available: false,
            pictures: Vec::new(),
            autoexec: None,
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

    /// The mixer's settings changed outside the window (the MIXER
    /// command): show them as they are now, and change them from there.
    pub fn sync_mixer(&mut self, mixer: crate::mixer::MixerSettings) {
        self.settings.mixer = mixer;
        // An effect turned off takes its mix off the page.
        self.row = self.row.min(self.row_count().saturating_sub(1));
    }

    /// What the Stats page shows, for the frontend to hand over every
    /// frame while the window is open.
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
        self.home = dirs::home_dir();
        self.status = None;
        self.edit = None;
        self.dialog = None;
        self.browser = None;
        self.game_dialog = None;
        self.autoexec = None;
        self.confirm_delete = None;
        self.cheats.edit = None;
        self.cheats.refresh(host);
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
            Page::Drives => self.drives.len() + 1,
            Page::Games => self.games.len() + 1 + self.frontend.host_files as usize,
            Page::States => self.states.len(),
            Page::Cheats => self.cheats.rows().len(),
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
        if self.browser.is_some() {
            self.browser_key(key, host);
        } else if self.dialog.is_some() {
            self.dialog_key(key, host);
        } else if self.game_dialog.is_some() {
            self.game_dialog_key(key, host);
        } else if self.autoexec.is_some() {
            self.autoexec_key(key, host);
        } else if self.cheats.edit.is_some() {
            self.cheats_edit_key(key, host);
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
        let Some((target, into)) = self
            .hits
            .iter()
            .rev()
            .find(|h| h.row == row && (h.col..h.col + h.width).contains(&col))
            .map(|h| (h.target, col - h.col))
        else {
            return;
        };
        match target {
            Target::Tab(page) => {
                if self.dialog.is_none() && self.browser.is_none() && self.game_dialog.is_none() && self.autoexec.is_none() {
                    self.edit = None;
                    self.cheats.edit = None;
                    self.show_page(page);
                }
            }
            Target::Row(i) if i == self.row => self.key(UiKey::Enter, host),
            Target::Row(i) => {
                self.edit = None;
                self.cheats.edit = None;
                self.row = i;
            }
            Target::Step(i, dir) => {
                self.edit = None;
                self.cheats.edit = None;
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
            Target::GameField(field) => self.game_field_clicked(field, host),
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
        }
    }

    fn show_page(&mut self, page: Page) {
        if page != self.page {
            self.page = page;
            self.row = 0;
            self.scroll = 0;
            self.confirm_delete = None;
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
            _ if self.page == Page::Games => self.games_key(key, host),
            _ if self.page == Page::States => self.states_key(key, host),
            _ if self.page == Page::Cheats => self.cheats_key(key, host),
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
        match (key, item.input()) {
            (UiKey::Left, Input::Choice | Input::ChoiceOrText) => self.step(item, -1, host),
            (UiKey::Right | UiKey::Enter, Input::Choice) | (UiKey::Right, Input::ChoiceOrText) => {
                self.step(item, 1, host)
            }
            (UiKey::Enter, Input::ChoiceOrText | Input::Text) => {
                self.edit = Some(TextField::new(&item.text(&self.settings)));
            }
            (UiKey::Enter, Input::File) if item == Item::Mt32Roms => self.open_browser(Pick::Mt32Roms),
            (UiKey::Enter, Input::File) => self.open_browser(Pick::SoundFont),
            (UiKey::Enter | UiKey::Right, Input::Link) => self.open_autoexec(host),
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
            Pick::ImportGame => ("Pick a GOG game's folder or a DOSBox .conf", String::new(), true, &["conf"][..]),
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
            self.draw_browser(&mut g, content);
        } else if self.dialog.is_some() {
            self.draw_dialog(&mut g, content);
        } else if self.game_dialog.is_some() {
            self.draw_game_dialog(&mut g, content);
        } else if self.autoexec.is_some() {
            self.draw_autoexec(&mut g, content);
        } else if self.page == Page::Drives {
            self.draw_drives(&mut g, content);
        } else if self.page == Page::Games {
            self.draw_games(&mut g, content);
        } else if self.page == Page::States {
            self.draw_states(&mut g, content);
        } else if self.page == Page::Cheats {
            self.draw_cheats(&mut g, content);
        } else if self.page == Page::Stats {
            self.draw_stats(&mut g, content);
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
            draw::plot(frame, &layout, plot.cells, &plot.values, crate::stats::HISTORY, plot.max, plot.color);
        }
        self.draw_pictures(frame, &layout);
        self.layout = Some(layout);
    }

    /// The Stats page: the numbers, and a graph each of the frames the
    /// program draws and of the host's time the emulator takes.
    fn draw_stats(&mut self, g: &mut Grid, content: std::ops::Range<usize>) {
        let cols = g.cols;
        let Some(view) = &self.stats else {
            g.text(2, content.start, "Measuring...", draw::DIM);
            return;
        };
        let value_col = 27.min(cols / 2);
        let lines = [
            ("Frames drawn", format!("{:.0} a second, on a {:.0} Hz display", view.fps, view.refresh_hz)),
            (
                "Emulated CPU",
                format!(
                    "{} cycles/ms, {:.1} MIPS{}",
                    view.cycles_per_ms,
                    view.mips,
                    match (crate::dynrec::AVAILABLE, view.recompiler) {
                        (false, _) => "",
                        (true, true) => ", recompiled",
                        (true, false) => ", interpreted",
                    }
                ),
            ),
            ("Host CPU use", format!("{:.0}%, frametime {:.1} ms", view.cpu_use, view.render_ms)),
        ];
        for (i, (label, value)) in lines.iter().enumerate() {
            let row = content.start + i;
            if row < content.end {
                g.text_to(2, row, label, draw::TEXT, value_col - 1);
                g.text_to(value_col, row, value, draw::BRIGHT, cols - 2);
            }
        }
        // Two graphs, each with its title above it, in what is left.
        let first = content.start + lines.len() + 1;
        let left = content.end.saturating_sub(first);
        if left < 4 {
            return;
        }
        let height = left / 2 - 1;
        let fps_max = view.fps_history.iter().copied().fold(view.refresh_hz, f32::max).max(1.0);
        let fps_max = (fps_max / 10.0).ceil() * 10.0;
        let graphs = [
            ("Frames a second, the last 30 s", format!("{:.0}", fps_max), view.fps_history.clone(), fps_max, draw::GOOD),
            ("Host CPU use, the last 30 s", "100%".to_string(), view.cpu_history.clone(), 100.0, draw::KEY),
        ];
        for (i, (title, scale, values, max, color)) in graphs.into_iter().enumerate() {
            let top = first + i * (height + 1);
            g.text(2, top, title, draw::TEXT);
            g.text(cols - 2 - scale.len(), top, &scale, draw::DIM);
            self.plots.push(Plot { cells: (2, top + 1, cols - 4, height), values, max, color });
        }
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
        // The first tab shown: the selected one fits after it, with room
        // for the arrows.
        let mut first = 0;
        if width(pad, &titles) > room {
            while first < selected && width(pad, &titles[first..=selected]) + 4 > room {
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
        let label_col = cols - 16;
        let path_width = label_col.saturating_sub(15);
        for (i, row) in (self.scroll..self.row_count()).zip(content.clone()) {
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
                None => "(built into Rust-DOS)".to_string(),
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
        self.draw_scrollbar(g, content, self.scroll, self.row_count());
    }

    fn draw_settings(&mut self, g: &mut Grid, content: std::ops::Range<usize>) {
        let cols = g.cols;
        let items = self.items();
        Self::keep_visible(&mut self.scroll, self.row, content.len());
        let value_col = 27.min(cols / 2);
        let note_col = cols - 13;
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
        self.draw_scrollbar(g, content, self.scroll, items.len());
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

    /// The key hints for what is showing, each clickable.
    fn draw_hints(&mut self, g: &mut Grid, row: usize) {
        use UiKey::*;
        let hints: Vec<(&str, &str, UiKey)> = if self.browser.is_some() {
            vec![("Enter", "Open", Enter), ("Bksp", "Up", Backspace), ("Esc", "Cancel", Esc)]
        } else if self.dialog.is_some() {
            vec![("Tab", "Next", Tab), ("Enter", "Mount", Enter), ("Esc", "Cancel", Esc)]
        } else if self.game_dialog.is_some() {
            vec![("Tab", "Next", Tab), ("Enter", "Create", Enter), ("Esc", "Cancel", Esc)]
        } else if self.autoexec.is_some() {
            vec![("F2", "Save", Save), ("Esc", "Cancel", Esc)]
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
        } else if self.edit.is_some() || self.cheats.edit.is_some() {
            vec![("Enter", "OK", Enter), ("Esc", "Cancel", Esc)]
        } else if self.page == Page::Cheats {
            self.cheats_hints()
        } else if self.page == Page::Stats {
            vec![("Tab", "Page", Tab), ("Esc", "Close", Esc)]
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
                Some(Input::Link) => hints = vec![("Enter", "Edit", Enter)],
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
