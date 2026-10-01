//! The rust-dos configuration file: a DOSBox-style INI file with
//! `[emulator]`, `[sound]`, `[mixer]`, `[joystick]`, `[network]`,
//! `[drives]` and `[autoexec]` sections. See
//! `rust-dos.conf.example` for the format.
//!
//! Lookup order, first match wins: `--config FILE`, `./rust-dos.conf`,
//! `rust-dos.conf` next to the executable (a portable install), then
//! `rust-dos.conf` in the per-user configuration directory, where a
//! commented template is written on first start.
//!
//! The settings window saves back into the file in use (`save`), changing
//! only the lines of the settings and drives and keeping everything else.

use crate::cpu::{CoreMode, CpuModel};
use crate::disk::DRIVE_Z;
use crate::diskio::{DiskSettings, DiskSpeed, NoiseMode};
use crate::joystick::{JoystickSettings, JoystickType};
use crate::keylayout::LayoutSetting;
use crate::lpt_dac::LptDacType;
use crate::mount::{
    MountSpec, contract_home, expand_host_path, mount_spec_value, parse_drive_letter, parse_drive_name, parse_mount_spec,
    tokenize,
};
use crate::mixer::{Channel, ChorusPreset, MixerSettings, ReverbPreset, SbFilter};
use crate::timer::CpuSpeed;
use crate::video::adapter::{Adapter, VideoSetup};
use crate::video::composite::{CompositeEra, CompositeMode, CompositeSettings};
use crate::video::mono::Monochrome;
use crate::video::shader::{CrtSettings, Shader};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "rust-dos.conf";
/// Written to the default location on first start.
pub const TEMPLATE: &str = include_str!("../rust-dos.conf.example");

/// The per-user directory for rust-dos's files, e.g. `~/.config/rust-dos`
/// on Linux.
pub fn user_dir() -> Option<PathBuf> {
    crate::hostdirs::config_dir().map(|d| d.join("rust-dos"))
}

/// Per-user default: `<config dir>/rust-dos/rust-dos.conf`, e.g.
/// `~/.config/rust-dos/rust-dos.conf` on Linux.
pub fn default_path() -> Option<PathBuf> {
    user_dir().map(|d| d.join(FILE_NAME))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigSource {
    CommandLine,
    WorkingDir,
    ExeDir,
    UserDefault,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Located {
    Found(PathBuf, ConfigSource),
    /// Nothing found; `default` is where a template should be written.
    NotFound {
        default: Option<PathBuf>,
    },
}

/// The directory holding the rust-dos executable, where a
/// `rust-dos.conf` makes the install portable.
pub fn exe_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe = fs::canonicalize(&exe).unwrap_or(exe);
    exe.parent().map(Path::to_path_buf)
}

/// Find the config file to use. An explicitly requested file must exist.
pub fn locate(
    cli: Option<&Path>,
    cwd: &Path,
    exe_dir: Option<&Path>,
    default: Option<PathBuf>,
) -> Result<Located, String> {
    if let Some(path) = cli {
        let path = cwd.join(path);
        if path.is_file() {
            return Ok(Located::Found(path, ConfigSource::CommandLine));
        }
        return Err(format!("Config file {} not found", path.display()));
    }
    let local = cwd.join(FILE_NAME);
    if local.is_file() {
        return Ok(Located::Found(local, ConfigSource::WorkingDir));
    }
    if let Some(dir) = exe_dir {
        let portable = dir.join(FILE_NAME);
        if portable.is_file() {
            return Ok(Located::Found(portable, ConfigSource::ExeDir));
        }
    }
    match default {
        Some(path) if path.is_file() => Ok(Located::Found(path, ConfigSource::UserDefault)),
        default => Ok(Located::NotFound { default }),
    }
}

#[derive(Debug, Default)]
pub struct Config {
    /// The file the settings came from, if any.
    pub source: Option<PathBuf>,
    /// True if `source` was just created from the template.
    pub created: bool,
    pub scale: Option<u32>,
    /// Desktop fullscreen (`fullscreen`).
    pub fullscreen: Option<bool>,
    /// Stretch the picture to 4:3 (`aspect`).
    pub aspect: Option<bool>,
    /// How the picture is scaled to the window (`filter`).
    pub filter: Option<Filter>,
    /// The CRT look (`shader`), how far its tube bends (`crt_curvature`)
    /// and how much it glows (`crt_glow`).
    pub shader: Option<Shader>,
    pub crt_curvature: Option<u16>,
    pub crt_glow: Option<u16>,
    /// A monochrome monitor's phosphor (`monochrome`).
    pub monochrome: Option<Monochrome>,
    /// The CGA's composite monitor (`composite`, `composite_era`).
    pub composite: Option<CompositeMode>,
    pub composite_era: Option<CompositeEra>,
    /// The display adapter (`machine`).
    pub machine: Option<Adapter>,
    /// The 3dfx card (`voodoo`), its memory (`voodoo_memory`), what draws
    /// for it (`voodoo_renderer`) and at what size (`voodoo_scale`).
    pub voodoo: Option<bool>,
    pub voodoo_memory: Option<crate::voodoo::Board>,
    pub voodoo_renderer: Option<crate::voodoo::Renderer>,
    pub voodoo_scale: Option<u32>,
    /// Where screenshots and recordings go (`capture_dir`), and whether
    /// they show the settings window and the performance overlay
    /// (`record_ui`) and the CRT shader (`record_shader`).
    pub capture_dir: Option<PathBuf>,
    pub record_ui: Option<bool>,
    pub record_shader: Option<bool>,
    /// Emulated CPU speed (`cycles`).
    pub cycles: Option<CpuSpeed>,
    /// Emulated processor (`cpu`).
    pub cpu: Option<CpuModel>,
    /// What runs the programs' instructions (`core`).
    pub core: Option<CoreMode>,
    /// RAM in MB (`memsize`).
    pub memsize: Option<usize>,
    /// Expanded memory (`ems`).
    pub ems: Option<bool>,
    /// Upper memory blocks (`umb`).
    pub umb: Option<bool>,
    /// The DPMI host (`dpmi`).
    pub dpmi: Option<bool>,
    /// DOS's tables packed low, as DOS=HIGH has them (`dos_high`).
    pub dos_high: Option<bool>,
    /// The DOS version programs are told (`dos_version`).
    pub dos_version: Option<DosVersion>,
    /// A booted system's hard disks on the IDE channels (`ide_hard_disks`).
    pub ide_hard_disks: Option<bool>,
    /// A booted system has a CD-ROM drive with no disc in it (`boot_cdrom`).
    pub boot_cdrom: Option<bool>,
    /// The keyboard layout (`keyboard_layout`).
    pub keyboard_layout: Option<LayoutSetting>,
    /// The mouse captured by itself (`mouse_autocapture`).
    pub mouse_autocapture: Option<bool>,
    /// The messages that say the mouse was captured or let go
    /// (`mouse_capture_messages`).
    pub mouse_capture_messages: Option<bool>,
    /// Rewind (`rewind`), and the memory its states may take in MB
    /// (`rewind_memory`).
    pub rewind: Option<bool>,
    pub rewind_memory: Option<usize>,
    /// `[sound]`: the Sound Blaster (None: `sbtype=none`), the FM chip,
    /// the Gravis Ultrasound, and the MPU-401's synthesizer.
    pub sound: SoundConfig,
    /// How fast the disks are (`[emulator]`) and the noises they make
    /// (`[sound]`).
    pub disk: DiskSettings,
    /// `[mixer]`: the volumes of the sound sources.
    pub mixer: MixerSettings,
    /// `[joystick]`: what the game port has plugged in.
    pub joystick: JoystickSettings,
    /// `[network]`: the IPX driver and the LAN.
    pub network: crate::net::NetSettings,
    /// `[serial]`: the serial ports.
    pub serial: crate::serial::SerialSettings,
    /// `[printer]`: the printer on LPT1.
    pub printer: crate::printer::PrinterSettings,
    /// `[achievements]`: RetroAchievements.
    pub achievements: crate::achievements::AchievementSettings,
    /// `[drives]` entries in file order, at most one per drive.
    pub drives: Vec<MountSpec>,
    /// `[autoexec]` command lines in file order.
    pub autoexec: Vec<String>,
    /// A game profile's name (`[game]`, see games.rs).
    pub game_name: Option<String>,
    /// The version of the game RetroAchievements knows (`[game]`'s
    /// `achievements`): its hash, or its zip or DOSZ archive.
    pub game_achievements: Option<String>,
    /// Problems worth telling the user about; none of them are fatal.
    pub warnings: Vec<String>,
}

impl Config {
    pub fn drive(&self, drive: u8) -> Option<&MountSpec> {
        self.drives.iter().find(|spec| spec.drive == drive)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    None,
    Emulator,
    Sound,
    Mixer,
    Joystick,
    Network,
    Serial,
    Printer,
    Achievements,
    Drives,
    Autoexec,
    /// A game profile's (games.rs).
    Game,
    Unknown,
}

impl Section {
    fn parse(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "emulator" => Section::Emulator,
            "sound" => Section::Sound,
            "mixer" => Section::Mixer,
            "joystick" => Section::Joystick,
            "network" => Section::Network,
            "serial" => Section::Serial,
            "printer" => Section::Printer,
            "achievements" => Section::Achievements,
            "drives" => Section::Drives,
            "autoexec" => Section::Autoexec,
            "game" => Section::Game,
            _ => Section::Unknown,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Section::Emulator => "emulator",
            Section::Sound => "sound",
            Section::Mixer => "mixer",
            Section::Joystick => "joystick",
            Section::Network => "network",
            Section::Serial => "serial",
            Section::Printer => "printer",
            Section::Achievements => "achievements",
            Section::Drives => "drives",
            Section::Autoexec => "autoexec",
            Section::Game => "game",
            Section::None | Section::Unknown => "",
        }
    }
}

/// How the picture is scaled up to the window (`filter`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Filter {
    /// Sharp pixels.
    #[default]
    Nearest,
    /// Smooth, interpolated pixels.
    Linear,
}

impl Filter {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "nearest" => Some(Filter::Nearest),
            "linear" => Some(Filter::Linear),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Filter::Nearest => "nearest",
            Filter::Linear => "linear",
        }
    }
}

/// The DOS version the built-in DOS reports (`dos_version`): INT 21h
/// AH=30h, AX=3306h and the PSP's, and the version the FAT32 functions of
/// MS-DOS 7 (AX=7302h to 7305h) need.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DosVersion {
    pub major: u8,
    pub minor: u8,
}

impl Default for DosVersion {
    fn default() -> Self {
        Self::new(5, 0)
    }
}

impl DosVersion {
    /// The versions the settings window offers.
    pub const PRESETS: [DosVersion; 4] = [Self::new(5, 0), Self::new(6, 22), Self::new(7, 0), Self::new(7, 10)];

    pub const fn new(major: u8, minor: u8) -> Self {
        Self { major, minor }
    }

    /// "5", "5.0", "6.22" or "7.1", as DOSBox's `ver`: a minor of one
    /// digit is tenths (7.1 is 7.10). 2.00 to 9.99.
    pub fn parse(s: &str) -> Option<Self> {
        let (major, minor) = s.trim().split_once('.').unwrap_or((s.trim(), "00"));
        let major: u8 = major.parse().ok().filter(|m| (2..=9).contains(m))?;
        if !(1..=2).contains(&minor.len()) || !minor.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let tenths = minor.len() == 1;
        let minor: u8 = minor.parse().ok()?;
        Some(Self::new(major, if tenths { minor * 10 } else { minor }))
    }

    /// "7.10".
    pub fn name(self) -> String {
        format!("{}.{:02}", self.major, self.minor)
    }

    pub fn at_least(self, major: u8, minor: u8) -> bool {
        self >= Self::new(major, minor)
    }
}

/// A yes/no setting: true, on, yes or 1, or false, off, no or 0.
fn parse_bool(value: &str) -> Option<bool> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "on" | "yes" | "1" => Some(true),
        "false" | "off" | "no" | "0" => Some(false),
        _ => None,
    }
}

/// The synthesizer that plays the MPU-401's MIDI (`midisynth`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MidiSynth {
    /// The SoundFont if one is set, else the Ultrasound patches.
    Auto,
    SoundFont,
    Gus,
    /// The Roland MT-32 (or CM-32L), played by munt.
    Mt32,
    /// A MIDI port of the host (`midiport`).
    Host,
    None,
}

/// The model the ROMs are for (`mt32model`): the MT-32, or the CM-32L with
/// its extra sound effects. `Auto` takes the CM-32L's ROMs when they are
/// there, as DOSBox Staging does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mt32Model {
    Auto,
    Mt32,
    Cm32l,
}

impl Mt32Model {
    pub const ALL: [Mt32Model; 3] = [Mt32Model::Auto, Mt32Model::Mt32, Mt32Model::Cm32l];

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Mt32Model::Auto),
            "mt32" | "mt-32" => Some(Mt32Model::Mt32),
            "cm32l" | "cm-32l" => Some(Mt32Model::Cm32l),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Mt32Model::Auto => "auto",
            Mt32Model::Mt32 => "mt32",
            Mt32Model::Cm32l => "cm32l",
        }
    }

    /// As the settings window shows it.
    pub fn describe(self) -> &'static str {
        match self {
            Mt32Model::Auto => "auto (CM-32L, else MT-32)",
            Mt32Model::Mt32 => "MT-32",
            Mt32Model::Cm32l => "CM-32L",
        }
    }
}

/// The `[sound]` section.
#[derive(Debug, Clone, PartialEq)]
pub struct SoundConfig {
    /// The Sound Blaster's resources, and whether there is one.
    pub sb: crate::sb::SbConfig,
    pub sb_installed: bool,
    /// OPL3 (as on an SB Pro 2 or SB16) rather than OPL2.
    pub opl3: bool,
    /// The AWE32's sample ROM, a file or a directory with `awe32.raw`
    /// (`awe32rom`); without it, the usual places (`awe32::rom::find`).
    pub awe32rom: Option<PathBuf>,
    /// The AWE32's sample RAM in KB (`awe32ram`).
    pub awe32ram: u32,
    pub soundfont: Option<PathBuf>,
    /// The Gravis Ultrasound; `enabled` says whether there is one.
    pub gus: crate::gus::GusConfig,
    pub midisynth: MidiSynth,
    /// The directory with the MT-32's ROMs (`mt32roms`); without it, the
    /// usual places (`mt32::default_rom_dirs`).
    pub mt32roms: Option<PathBuf>,
    pub mt32model: Mt32Model,
    /// munt's library, where the system doesn't find it (`mt32lib`).
    pub mt32lib: Option<PathBuf>,
    /// The host's MIDI port for `midisynth=host`: a part of its name or its
    /// number; empty for the first.
    pub midiport: String,
    /// The Covox or Disney Sound Source on LPT1 (`lpt_dac`).
    pub lpt_dac: LptDacType,
    /// The Tandy's and PCjr's sound chip (`tandy`).
    pub tandy: crate::sn76489::TandySound,
}

impl MidiSynth {
    pub fn name(self) -> &'static str {
        match self {
            MidiSynth::Auto => "auto",
            MidiSynth::SoundFont => "soundfont",
            MidiSynth::Gus => "gus",
            MidiSynth::Mt32 => "mt32",
            MidiSynth::Host => "host",
            MidiSynth::None => "none",
        }
    }
}

impl Default for SoundConfig {
    fn default() -> Self {
        Self {
            sb: crate::sb::SbConfig::default(),
            sb_installed: true,
            opl3: true,
            awe32rom: None,
            awe32ram: crate::awe32::DEFAULT_RAM_KB,
            soundfont: None,
            gus: crate::gus::GusConfig::default(),
            midisynth: MidiSynth::Auto,
            mt32roms: None,
            mt32model: Mt32Model::Auto,
            mt32lib: None,
            midiport: String::new(),
            lpt_dac: LptDacType::None,
            tandy: crate::sn76489::TandySound::Auto,
        }
    }
}

impl SoundConfig {
    /// The Sound Blaster to install, if any.
    pub fn card(&self) -> Option<crate::sb::SbConfig> {
        self.sb_installed.then_some(self.sb)
    }

    /// The Gravis Ultrasound to install, if any.
    pub fn ultrasound(&self) -> Option<crate::gus::GusConfig> {
        self.gus.enabled.then(|| self.gus.clone())
    }

    /// Problems between settings, once the section is read: an Ultrasound
    /// on the Sound Blaster's ports is left out; shared IRQs and DMA
    /// channels only work while programs use one card at a time.
    pub fn check(&mut self) -> Vec<String> {
        let mut warnings = Vec::new();
        let Some(sb) = self.card() else { return warnings };
        if !self.gus.enabled {
            return warnings;
        }
        if self.gus.base == sb.base {
            warnings.push(format!(
                "[sound]: gusbase {:X} is the Sound Blaster's base port; there is no Ultrasound",
                self.gus.base
            ));
            self.gus.enabled = false;
            return warnings;
        }
        if self.gus.irq == sb.irq {
            warnings.push(format!("[sound]: the Ultrasound and the Sound Blaster share IRQ {}", sb.irq));
        }
        if self.gus.dma == sb.dma8 || (sb.model.is_sb16() && self.gus.dma == sb.dma16) {
            warnings.push(format!("[sound]: the Ultrasound and the Sound Blaster share DMA {}", self.gus.dma));
        }
        warnings
    }

    fn set(&mut self, key: &str, value: &str, base_dir: &Path, home: Option<&Path>) -> Result<(), String> {
        let sb = &mut self.sb;
        let number = |min: u32, max: u32| match value.parse::<u32>() {
            Ok(n) if (min..=max).contains(&n) => Ok(n),
            _ => Err(format!("invalid {} '{}' ({} to {})", key, value, min, max)),
        };
        match key.to_ascii_lowercase().as_str() {
            "sbtype" => {
                if value.eq_ignore_ascii_case("none") {
                    self.sb_installed = false;
                } else {
                    sb.model = crate::sb::SbModel::parse(value)
                        .ok_or_else(|| format!("invalid sbtype '{}' (sb16, awe32, sbpro2, sb2 or none)", value))?;
                    self.sb_installed = true;
                }
            }
            "sbbase" => {
                let base = u16::from_str_radix(value.trim_start_matches("0x"), 16)
                    .ok()
                    .filter(|b| (0x210..=0x280).contains(b) && b & 0xF == 0)
                    .ok_or_else(|| format!("invalid sbbase '{}' (210 to 280, hex)", value))?;
                sb.base = base;
            }
            "irq" => {
                let irq = number(2, 15)?;
                if !matches!(irq, 2 | 3 | 5 | 7 | 9 | 10 | 11 | 12 | 15) {
                    return Err(format!("invalid irq '{}'", value));
                }
                sb.irq = irq as u8;
            }
            "dma" => sb.dma8 = number(0, 3).and_then(|d| if d == 2 { Err("dma 2 belongs to the floppy".to_string()) } else { Ok(d) })? as u8,
            "hdma" => sb.dma16 = number(5, 7)? as u8,
            "opl" => {
                self.opl3 = match value.to_ascii_lowercase().as_str() {
                    "opl3" => true,
                    "opl2" => false,
                    _ => return Err(format!("invalid opl '{}' (opl3 or opl2)", value)),
                }
            }
            "awe32rom" => self.awe32rom = Some(expand_host_path(value, base_dir, home)),
            "awe32ram" => {
                self.awe32ram = value
                    .trim()
                    .trim_end_matches(|c: char| c.eq_ignore_ascii_case(&'k') || c.eq_ignore_ascii_case(&'b'))
                    .parse::<u32>()
                    .ok()
                    .filter(|kb| crate::awe32::RAM_SIZES.contains(kb))
                    .ok_or_else(|| {
                        let sizes: Vec<String> = crate::awe32::RAM_SIZES.iter().map(u32::to_string).collect();
                        format!("invalid awe32ram '{}' (KB: {})", value, sizes.join(", "))
                    })?;
            }
            "soundfont" => self.soundfont = Some(expand_host_path(value, base_dir, home)),
            "mt32roms" => self.mt32roms = Some(expand_host_path(value, base_dir, home)),
            "mt32lib" => self.mt32lib = Some(expand_host_path(value, base_dir, home)),
            "mt32model" => {
                self.mt32model = Mt32Model::parse(value)
                    .ok_or_else(|| format!("invalid mt32model '{}' (auto, mt32 or cm32l)", value))?;
            }
            "midiport" => self.midiport = value.to_string(),
            "gus" => {
                self.gus.enabled =
                    parse_bool(value).ok_or_else(|| format!("invalid gus '{}' (true or false)", value))?;
            }
            "gusbase" => {
                // 230h would put 3X0h-3X7h on the MPU-401 at 330h.
                self.gus.base = u16::from_str_radix(value.trim_start_matches("0x"), 16)
                    .ok()
                    .filter(|b| matches!(b, 0x210 | 0x220 | 0x240 | 0x250 | 0x260))
                    .ok_or_else(|| format!("invalid gusbase '{}' (210, 220, 240, 250 or 260, hex)", value))?;
            }
            "gusirq" => {
                self.gus.irq = value
                    .parse::<u8>()
                    .ok()
                    .filter(|i| matches!(i, 2 | 3 | 5 | 7 | 11 | 12 | 15))
                    .ok_or_else(|| format!("invalid gusirq '{}' (2, 3, 5, 7, 11, 12 or 15)", value))?;
            }
            "gusdma" => {
                self.gus.dma = value
                    .parse::<u8>()
                    .ok()
                    .filter(|d| matches!(d, 1 | 3 | 5 | 6 | 7))
                    .ok_or_else(|| format!("invalid gusdma '{}' (1, 3, 5, 6 or 7)", value))?;
            }
            "gusdrive" => {
                self.gus.drive = match value.to_ascii_lowercase().as_str() {
                    "none" | "off" | "false" | "no" => None,
                    _ => Some(
                        parse_drive_letter(value)
                            .filter(|&d| (3..DRIVE_Z).contains(&d))
                            .ok_or_else(|| format!("invalid gusdrive '{}' (a letter from D to Y, or none)", value))?,
                    ),
                }
            }
            "ultradir" => {
                if value.is_empty() {
                    return Err("ultradir is empty".to_string());
                }
                self.gus.ultradir = Some(value.to_string());
            }
            "midisynth" => {
                self.midisynth = match value.to_ascii_lowercase().as_str() {
                    "auto" => MidiSynth::Auto,
                    "soundfont" => MidiSynth::SoundFont,
                    "gus" => MidiSynth::Gus,
                    "mt32" | "mt-32" | "munt" => MidiSynth::Mt32,
                    "host" | "hostmidi" | "external" => MidiSynth::Host,
                    "none" => MidiSynth::None,
                    _ => {
                        return Err(format!("invalid midisynth '{}' (auto, soundfont, gus, mt32, host or none)", value));
                    }
                }
            }
            "tandy" => {
                self.tandy = crate::sn76489::TandySound::parse(value)
                    .ok_or_else(|| format!("invalid tandy '{}' (auto, on or off)", value))?;
            }
            "lpt_dac" => {
                self.lpt_dac = LptDacType::parse(value)
                    .ok_or_else(|| format!("invalid lpt_dac '{}' (none, disney or covox)", value))?;
            }
            _ => return Err(format!("unknown setting '{}'", key)),
        }
        Ok(())
    }
}

/// The least and the most RAM there is, in MB (`memsize`): the most with
/// the CPU that takes the most (`CpuModel::max_memsize`).
pub const MIN_MEMSIZE: usize = 2;
pub const MAX_MEMSIZE: usize = 512;

/// A memory size as written: a number of MB, with or without the MB.
pub fn parse_memsize(value: &str) -> Result<usize, String> {
    let lower = value.trim().to_ascii_lowercase();
    match lower.strip_suffix("mb").unwrap_or(&lower).trim_end().parse::<usize>() {
        Ok(mb) if (MIN_MEMSIZE..=MAX_MEMSIZE).contains(&mb) => Ok(mb),
        _ => Err(format!("invalid memsize '{}' ({} to {} MB)", value.trim(), MIN_MEMSIZE, MAX_MEMSIZE)),
    }
}

/// Parse config text. Relative drive paths resolve against `base_dir`.
pub fn parse(text: &str, base_dir: &Path, home: Option<&Path>) -> Config {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let mut config = Config::default();
    let mut warnings = Vec::new();
    let mut section = Section::None;

    for (index, raw) in text.lines().enumerate() {
        let mut warn = |msg: String| warnings.push(format!("line {}: {}", index + 1, msg));
        let line = raw.trim();
        // Only whole-line comments: paths may contain '#' or ';'
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }

        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = Section::parse(name);
            if section == Section::Unknown {
                warn(format!("unknown section [{}]", name.trim().to_ascii_lowercase()));
            }
            continue;
        }

        match section {
            Section::Autoexec => config.autoexec.push(line.to_string()),
            Section::Unknown => {}
            Section::None => warn("setting outside of a section".to_string()),
            Section::Emulator
            | Section::Drives
            | Section::Sound
            | Section::Mixer
            | Section::Joystick
            | Section::Network
            | Section::Serial
            | Section::Printer
            | Section::Achievements
            | Section::Game => {
                let Some((key, value)) = line.split_once('=') else {
                    warn(format!("expected key=value, got '{}'", line));
                    continue;
                };
                let (key, value) = (key.trim(), value.trim());

                if section == Section::Emulator {
                    match key.to_ascii_lowercase().as_str() {
                        "scale" => match value.parse::<u32>() {
                            Ok(n) if (1..=16).contains(&n) => config.scale = Some(n),
                            _ => warn(format!("invalid scale '{}'", value)),
                        },
                        "fullscreen" | "aspect" => match parse_bool(value) {
                            Some(on) if key.eq_ignore_ascii_case("fullscreen") => config.fullscreen = Some(on),
                            Some(on) => config.aspect = Some(on),
                            None => warn(format!("invalid {} '{}' (true or false)", key, value)),
                        },
                        "filter" => match Filter::parse(value) {
                            Some(filter) => config.filter = Some(filter),
                            None => warn(format!("invalid filter '{}' (nearest or linear)", value)),
                        },
                        "shader" => match Shader::parse(value) {
                            Some(shader) => config.shader = Some(shader),
                            None => warn(format!("invalid shader '{}' (none, scanlines, aperture or crt)", value)),
                        },
                        "crt_curvature" | "crt_glow" => match crate::video::shader::parse_amount(value) {
                            Some(percent) if key.eq_ignore_ascii_case("crt_curvature") => {
                                config.crt_curvature = Some(percent)
                            }
                            Some(percent) => config.crt_glow = Some(percent),
                            None => warn(format!("invalid {} '{}' (0 to 100)", key.to_ascii_lowercase(), value)),
                        },
                        "capture_dir" => {
                            let value = value.trim_matches('"');
                            config.capture_dir = Some(match (value.strip_prefix("~/"), home) {
                                (Some(rest), Some(h)) => h.join(rest),
                                _ => PathBuf::from(value),
                            });
                        }
                        "machine" => match Adapter::parse(value) {
                            Some(adapter) => config.machine = Some(adapter),
                            None => warn(format!("invalid machine '{}' (svga, svga_s3, svga_s3virge, svga_s3virgevx, svga_et4000, vga, ega, cga, tandy, pcjr or hercules)", value)),
                        },
                        "voodoo" => match parse_bool(value) {
                            Some(on) => config.voodoo = Some(on),
                            None => warn(format!("invalid voodoo '{}' (true or false)", value)),
                        },
                        "voodoo_memory" => match crate::voodoo::Board::parse(value) {
                            Some(board) => config.voodoo_memory = Some(board),
                            None => warn(format!("invalid voodoo_memory '{}' (4 or 12)", value)),
                        },
                        "voodoo_renderer" => match crate::voodoo::Renderer::parse(value) {
                            Some(renderer) => config.voodoo_renderer = Some(renderer),
                            None => warn(format!("invalid voodoo_renderer '{}' (software or opengl)", value)),
                        },
                        "voodoo_scale" => match value.parse::<u32>() {
                            Ok(n) if (1..=4).contains(&n) => config.voodoo_scale = Some(n),
                            _ => warn(format!("invalid voodoo_scale '{}' (1 to 4)", value)),
                        },
                        "monochrome" => match Monochrome::parse(value) {
                            Some(mono) => config.monochrome = Some(mono),
                            None => warn(format!("invalid monochrome '{}' (off, white, amber or green)", value)),
                        },
                        "composite" => match CompositeMode::parse(value) {
                            Some(mode) => config.composite = Some(mode),
                            None => warn(format!("invalid composite '{}' (auto, on or off)", value)),
                        },
                        "composite_era" => match CompositeEra::parse(value) {
                            Some(era) => config.composite_era = Some(era),
                            None => warn(format!("invalid composite_era '{}' (old or new)", value)),
                        },
                        "cycles" => match CpuSpeed::parse(value) {
                            Ok(speed) => config.cycles = Some(speed),
                            Err(e) => warn(e),
                        },
                        "memsize" => match parse_memsize(value) {
                            Ok(mb) => config.memsize = Some(mb),
                            Err(e) => warn(e),
                        },
                        "ems" | "umb" | "dpmi" | "dos_high" => match parse_bool(value) {
                            Some(on) if key.eq_ignore_ascii_case("ems") => config.ems = Some(on),
                            Some(on) if key.eq_ignore_ascii_case("umb") => config.umb = Some(on),
                            Some(on) if key.eq_ignore_ascii_case("dos_high") => config.dos_high = Some(on),
                            Some(on) => config.dpmi = Some(on),
                            None => warn(format!("invalid {} '{}' (true or false)", key, value)),
                        },
                        "ide_hard_disks" => match parse_bool(value) {
                            Some(on) => config.ide_hard_disks = Some(on),
                            None => warn(format!("invalid ide_hard_disks '{}' (true or false)", value)),
                        },
                        "boot_cdrom" => match parse_bool(value) {
                            Some(on) => config.boot_cdrom = Some(on),
                            None => warn(format!("invalid boot_cdrom '{}' (true or false)", value)),
                        },
                        "dos_version" => match DosVersion::parse(value) {
                            Some(version) => config.dos_version = Some(version),
                            None => warn(format!("invalid dos_version '{}' (such as 5.00, 6.22 or 7.10)", value)),
                        },
                        "core" => match CoreMode::parse(value) {
                            Ok(core) => config.core = Some(core),
                            Err(e) => warn(e),
                        },
                        "mouse_capture_messages" => match parse_bool(value) {
                            Some(on) => config.mouse_capture_messages = Some(on),
                            None => warn(format!("invalid mouse_capture_messages '{}' (true or false)", value)),
                        },
                        "mouse_autocapture" => match parse_bool(value) {
                            Some(on) => config.mouse_autocapture = Some(on),
                            None => warn(format!("invalid mouse_autocapture '{}' (true or false)", value)),
                        },
                        "rewind" => match parse_bool(value) {
                            Some(on) => config.rewind = Some(on),
                            None => warn(format!("invalid rewind '{}' (true or false)", value)),
                        },
                        "record_ui" => match parse_bool(value) {
                            Some(on) => config.record_ui = Some(on),
                            None => warn(format!("invalid record_ui '{}' (true or false)", value)),
                        },
                        "record_shader" => match parse_bool(value) {
                            Some(on) => config.record_shader = Some(on),
                            None => warn(format!("invalid record_shader '{}' (true or false)", value)),
                        },
                        "rewind_memory" => match value.parse::<usize>() {
                            Ok(mb) if (16..=4096).contains(&mb) => config.rewind_memory = Some(mb),
                            _ => warn(format!("invalid rewind_memory '{}' (16 to 4096 MB)", value)),
                        },
                        "keyboard_layout" => match LayoutSetting::parse(value) {
                            Some(layout) => config.keyboard_layout = Some(layout),
                            None => warn(format!("invalid keyboard_layout '{}' (auto or a KEYB code such as us, gr, fr)", value)),
                        },
                        "cpu" => match value.to_ascii_lowercase().as_str() {
                            "386" => config.cpu = Some(CpuModel::I386),
                            "486" => config.cpu = Some(CpuModel::I486),
                            "pentium" | "586" => config.cpu = Some(CpuModel::Pentium),
                            "pentium_mmx" | "pentiummmx" | "mmx" => config.cpu = Some(CpuModel::PentiumMmx),
                            _ => warn(format!("invalid cpu '{}' (386, 486, pentium or pentium_mmx)", value)),
                        },
                        "hard_disk_speed" | "floppy_disk_speed" => match DiskSpeed::parse(value) {
                            Some(speed) if key.eq_ignore_ascii_case("hard_disk_speed") => {
                                config.disk.hard_disk_speed = speed
                            }
                            Some(speed) => config.disk.floppy_disk_speed = speed,
                            None => warn(format!("invalid {} '{}' (maximum, fast, medium or slow)", key, value)),
                        },
                        _ => warn(format!("unknown setting '{}'", key)),
                    }
                    continue;
                }

                if section == Section::Mixer {
                    let mixer = &mut config.mixer;
                    match key.to_ascii_lowercase().as_str() {
                        "speaker_filter" => match parse_bool(value) {
                            Some(on) => mixer.speaker_filter = on,
                            None => warn(format!("invalid speaker_filter '{}' (on or off)", value)),
                        },
                        "sb_filter" => match SbFilter::parse(value) {
                            Some(filter) => mixer.sb_filter = filter,
                            None => warn(format!("invalid sb_filter '{}' (auto or off)", value)),
                        },
                        "reverb" => match ReverbPreset::parse(value) {
                            Some(preset) => mixer.reverb = preset,
                            None => warn(format!("invalid reverb '{}' (off, tiny, small, medium, large or huge)", value)),
                        },
                        "chorus" => match ChorusPreset::parse(value) {
                            Some(preset) => mixer.chorus = preset,
                            None => warn(format!("invalid chorus '{}' (off, light, normal or strong)", value)),
                        },
                        "reverb_mix" | "chorus_mix" => match crate::mixer::parse_mix(value) {
                            Ok(mix) if key.eq_ignore_ascii_case("reverb_mix") => mixer.reverb_mix = mix,
                            Ok(mix) => mixer.chorus_mix = mix,
                            Err(e) => warn(format!("{}: {}", key.to_ascii_lowercase(), e)),
                        },
                        _ => match Channel::parse(key) {
                            Some(channel) => match crate::mixer::parse_level(value) {
                                Ok(percent) => mixer.set_level(channel, percent),
                                Err(e) => warn(format!("{}: {}", channel.key(), e)),
                            },
                            None => warn(format!("unknown setting '{}'", key)),
                        },
                    }
                    continue;
                }

                if section == Section::Game {
                    match key.to_ascii_lowercase().as_str() {
                        "name" if !value.is_empty() => config.game_name = Some(value.to_string()),
                        "name" => warn("the game's name is empty".to_string()),
                        "achievements" if !value.is_empty() => config.game_achievements = Some(value.to_string()),
                        "achievements" => {}
                        _ => warn(format!("unknown setting '{}'", key)),
                    }
                    continue;
                }

                if section == Section::Joystick {
                    match key.to_ascii_lowercase().as_str() {
                        "joysticktype" => match JoystickType::parse(value) {
                            Some(kind) => config.joystick.kind = kind,
                            None => warn(format!("invalid joysticktype '{}' (auto, 4axis, 2axis, mouse or none)", value)),
                        },
                        "deadzone" => match crate::joystick::parse_deadzone(value) {
                            Ok(percent) => config.joystick.deadzone = percent,
                            Err(e) => warn(e),
                        },
                        _ => warn(format!("unknown setting '{}'", key)),
                    }
                    continue;
                }

                if section == Section::Network {
                    if let Err(e) = config.network.set(key, value) {
                        warn(e);
                    }
                    continue;
                }

                if section == Section::Serial {
                    if let Err(e) = config.serial.set(key, value) {
                        warn(e);
                    }
                    continue;
                }

                if section == Section::Printer {
                    if let Err(e) = config.printer.set(key, value, home) {
                        warn(e);
                    }
                    continue;
                }

                if section == Section::Achievements {
                    if let Err(e) = config.achievements.set(key, value) {
                        warn(e);
                    }
                    continue;
                }

                if section == Section::Sound {
                    let lower = key.to_ascii_lowercase();
                    if matches!(lower.as_str(), "hard_disk_noise" | "floppy_disk_noise") {
                        match NoiseMode::parse(value) {
                            Some(mode) if lower == "hard_disk_noise" => config.disk.hard_disk_noise = mode,
                            Some(mode) => config.disk.floppy_disk_noise = mode,
                            None => warn(format!("invalid {} '{}' (off, seek-only or on)", key, value)),
                        }
                    } else if let Err(e) = config.sound.set(key, value, base_dir, home) {
                        warn(e);
                    }
                    continue;
                }

                let Some(drive) = parse_drive_name(key) else {
                    warn(format!("invalid drive letter '{}'", key));
                    continue;
                };
                let letter = key.trim_end_matches(':').to_ascii_uppercase();
                if drive == DRIVE_Z {
                    warn("drive Z: is reserved".to_string());
                    continue;
                }
                match tokenize(value).and_then(|t| parse_mount_spec(drive, &t, base_dir, home)) {
                    Ok(spec) => {
                        // -fs none makes C: the disk mounted as 2.
                        let drive = spec.drive;
                        if config.drive(drive).is_some() {
                            warn(format!(
                                "drive {} defined twice, using the last one",
                                crate::disk::drive_name(drive)
                            ));
                            config.drives.retain(|s| s.drive != drive);
                        }
                        config.drives.push(spec);
                    }
                    Err(e) => warn(format!("drive {}: {}", letter, e)),
                }
            }
        }
    }
    warnings.extend(config.sound.check());
    // A drive of the user's own takes the letter of the built-in Ultrasound
    // software.
    if let Some(drive) = config.sound.gus.drive.filter(|_| config.sound.gus.enabled)
        && config.drive(drive).is_some()
    {
        let letter = crate::disk::drive_letter(drive);
        warnings.push(format!(
            "[sound]: gusdrive {}: is mounted in [drives]; the built-in Ultrasound software is left out",
            letter
        ));
        config.sound.gus.drive = None;
    }
    // More memory than the CPU's machines took is as much as they did.
    let cpu = config.cpu.unwrap_or(Settings::default().cpu);
    if let Some(mb) = config.memsize.filter(|&mb| mb > cpu.max_memsize()) {
        warnings.push(format!("memsize {} MB is more than a {} takes: {} MB", mb, cpu.describe(), cpu.max_memsize()));
    }
    config.warnings = warnings;
    config
}

/// Write the commented template to `path`, creating parent directories.
/// Never overwrites an existing file.
pub fn write_template(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(TEMPLATE.as_bytes())
}

/// Locate, read and parse the configuration. Only a missing or unreadable
/// `--config` file is an error; everything else degrades to warnings.
pub fn load(
    cli: Option<&Path>,
    cwd: &Path,
    exe_dir: Option<&Path>,
    default: Option<PathBuf>,
    home: Option<&Path>,
) -> Result<Config, String> {
    let (path, source) = match locate(cli, cwd, exe_dir, default)? {
        Located::Found(path, source) => (path, source),
        Located::NotFound { default: None } => return Ok(Config::default()),
        Located::NotFound {
            default: Some(path),
        } => {
            return Ok(match write_template(&path) {
                Ok(()) => Config {
                    source: Some(path),
                    created: true,
                    ..parse(TEMPLATE, cwd, home)
                },
                Err(e) => Config {
                    warnings: vec![format!("could not create {}: {}", path.display(), e)],
                    ..Config::default()
                },
            });
        }
    };

    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) => {
            let msg = format!("cannot read {}: {}", path.display(), e);
            if source == ConfigSource::CommandLine {
                return Err(msg);
            }
            return Ok(Config {
                warnings: vec![msg],
                ..Config::default()
            });
        }
    };

    let absolute = std::path::absolute(&path).unwrap_or_else(|_| path.clone());
    let base_dir = absolute.parent().unwrap_or(cwd);
    let mut config = parse(&text, base_dir, home);
    config.warnings = config
        .warnings
        .into_iter()
        .map(|w| format!("{}: {}", path.display(), w))
        .collect();
    config.source = Some(path);
    Ok(config)
}

/// The settings in effect: the configuration with every value that is not
/// set filled in with its default.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub scale: u32,
    pub fullscreen: bool,
    pub aspect: bool,
    pub filter: Filter,
    pub shader: Shader,
    /// The CRT look's own settings.
    pub crt: CrtSettings,
    pub monochrome: Monochrome,
    /// The CGA's composite monitor.
    pub composite: CompositeSettings,
    pub machine: Adapter,
    /// The 3dfx card.
    pub voodoo: crate::voodoo::VoodooSettings,
    /// Where screenshots and recordings go; relative to the working
    /// directory.
    pub capture_dir: PathBuf,
    /// Whether screenshots and video and animation recordings show the
    /// settings window and the performance overlay, or the picture alone.
    pub record_ui: bool,
    /// Whether screenshots and video recordings show the picture through
    /// the CRT shader, as the window does, or plain.
    pub record_shader: bool,
    pub cycles: CpuSpeed,
    pub cpu: CpuModel,
    pub core: CoreMode,
    /// RAM in MB.
    pub memsize: usize,
    /// Expanded memory (EMS).
    pub ems: bool,
    /// Upper memory blocks.
    pub umb: bool,
    /// The DPMI host for DOS extenders.
    pub dpmi: bool,
    /// DOS's tables packed below the first MCB, as DOS=HIGH leaves
    /// conventional memory, rather than spread out up to 64 KB.
    pub dos_high: bool,
    /// The DOS version programs are told.
    pub dos_version: DosVersion,
    /// A booted system's hard disks are ATA disks on the IDE channels too.
    pub ide_hard_disks: bool,
    /// A booted system has a CD-ROM drive even with no CD mounted.
    pub boot_cdrom: bool,
    pub keyboard_layout: LayoutSetting,
    /// The mouse captured as it moves over the window while a program
    /// uses the mouse driver, and let go as the program's cursor leaves
    /// the screen.
    pub mouse_autocapture: bool,
    /// Say over the picture when the mouse is captured or let go.
    pub mouse_capture_messages: bool,
    /// Rewind with held Alt+F11, and the memory in MB its states may take.
    pub rewind: bool,
    pub rewind_memory: usize,
    pub sound: SoundConfig,
    pub disk: DiskSettings,
    pub mixer: MixerSettings,
    pub joystick: JoystickSettings,
    /// `[network]`: the IPX driver and the LAN.
    pub network: crate::net::NetSettings,
    /// `[serial]`: the serial ports.
    pub serial: crate::serial::SerialSettings,
    /// `[printer]`: the printer on LPT1.
    pub printer: crate::printer::PrinterSettings,
    /// `[achievements]`: RetroAchievements.
    pub achievements: crate::achievements::AchievementSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            scale: 1,
            fullscreen: false,
            aspect: false,
            filter: Filter::Nearest,
            shader: Shader::None,
            crt: CrtSettings::default(),
            monochrome: Monochrome::Off,
            composite: CompositeSettings::default(),
            machine: Adapter::Svga,
            voodoo: Default::default(),
            capture_dir: PathBuf::from("capture"),
            record_ui: false,
            record_shader: false,
            cycles: CpuSpeed::default(),
            cpu: CpuModel::I486,
            core: CoreMode::Auto,
            memsize: crate::bus::DEFAULT_MEMORY_MB,
            ems: true,
            umb: true,
            dpmi: true,
            dos_high: true,
            dos_version: DosVersion::default(),
            ide_hard_disks: true,
            boot_cdrom: true,
            keyboard_layout: LayoutSetting::Auto,
            mouse_autocapture: true,
            mouse_capture_messages: true,
            rewind: false,
            rewind_memory: 256,
            sound: SoundConfig::default(),
            disk: DiskSettings::default(),
            mixer: MixerSettings::default(),
            joystick: JoystickSettings::default(),
            network: crate::net::NetSettings::default(),
            serial: crate::serial::SerialSettings::default(),
            printer: crate::printer::PrinterSettings::default(),
            achievements: Default::default(),
        }
    }
}

impl Settings {

    /// The display adapter and monitor programs see: with `monochrome`, a
    /// VGA's or EGA's monitor is monochrome too (a CGA's stays colour).
    pub fn video_setup(&self) -> VideoSetup {
        let mono_monitor = match self.machine {
            Adapter::Cga | Adapter::Tandy | Adapter::Pcjr => false,
            Adapter::Hercules => true,
            _ => self.monochrome != Monochrome::Off,
        };
        VideoSetup { adapter: self.machine, mono_monitor }
    }

    pub fn from_config(config: &Config) -> Self {
        let default = Self::default();
        Self {
            scale: config.scale.unwrap_or(default.scale),
            fullscreen: config.fullscreen.unwrap_or(default.fullscreen),
            aspect: config.aspect.unwrap_or(default.aspect),
            filter: config.filter.unwrap_or(default.filter),
            shader: config.shader.unwrap_or(default.shader),
            crt: CrtSettings {
                curvature: config.crt_curvature.unwrap_or(default.crt.curvature),
                glow: config.crt_glow.unwrap_or(default.crt.glow),
            },
            monochrome: config.monochrome.unwrap_or(default.monochrome),
            composite: CompositeSettings {
                mode: config.composite.unwrap_or(default.composite.mode),
                era: config.composite_era.unwrap_or(default.composite.era),
            },
            machine: config.machine.unwrap_or(default.machine),
            voodoo: crate::voodoo::VoodooSettings {
                enabled: config.voodoo.unwrap_or(default.voodoo.enabled),
                board: config.voodoo_memory.unwrap_or(default.voodoo.board),
                renderer: config.voodoo_renderer.unwrap_or(default.voodoo.renderer),
                scale: config.voodoo_scale.unwrap_or(default.voodoo.scale),
            },
            capture_dir: config.capture_dir.clone().unwrap_or(default.capture_dir),
            record_ui: config.record_ui.unwrap_or(default.record_ui),
            record_shader: config.record_shader.unwrap_or(default.record_shader),
            cycles: config.cycles.unwrap_or(default.cycles),
            cpu: config.cpu.unwrap_or(default.cpu),
            core: config.core.unwrap_or(default.core),
            memsize: config.memsize.unwrap_or(default.memsize).min(config.cpu.unwrap_or(default.cpu).max_memsize()),
            ems: config.ems.unwrap_or(default.ems),
            umb: config.umb.unwrap_or(default.umb),
            dpmi: config.dpmi.unwrap_or(default.dpmi),
            dos_high: config.dos_high.unwrap_or(default.dos_high),
            dos_version: config.dos_version.unwrap_or(default.dos_version),
            ide_hard_disks: config.ide_hard_disks.unwrap_or(default.ide_hard_disks),
            boot_cdrom: config.boot_cdrom.unwrap_or(default.boot_cdrom),
            mouse_autocapture: config.mouse_autocapture.unwrap_or(default.mouse_autocapture),
            mouse_capture_messages: config.mouse_capture_messages.unwrap_or(default.mouse_capture_messages),
            rewind: config.rewind.unwrap_or(default.rewind),
            rewind_memory: config.rewind_memory.unwrap_or(default.rewind_memory),
            keyboard_layout: config.keyboard_layout.unwrap_or(default.keyboard_layout),
            sound: config.sound.clone(),
            disk: config.disk,
            mixer: config.mixer,
            joystick: config.joystick,
            network: config.network.clone(),
            serial: config.serial.clone(),
            printer: config.printer.clone(),
            achievements: config.achievements.clone(),
        }
    }
}

/// Every setting as the file writes it: section, key and value, or None
/// for a setting that is not set.
fn entries(settings: &Settings, home: Option<&Path>) -> Vec<(Section, &'static str, Option<String>)> {
    use Section::{Emulator, Sound};
    let yes_no = |on: bool| Some(if on { "true" } else { "false" }.to_string());
    let sound = &settings.sound;
    let (sb, gus) = (&sound.sb, &sound.gus);
    let mixer = Channel::ALL
        .map(|channel| (Section::Mixer, channel.key(), Some(settings.mixer.level(channel).to_string())));
    let mut entries = vec![
        (Emulator, "scale", Some(settings.scale.to_string())),
        (Emulator, "fullscreen", yes_no(settings.fullscreen)),
        (Emulator, "aspect", yes_no(settings.aspect)),
        (Emulator, "filter", Some(settings.filter.name().to_string())),
        (Emulator, "shader", Some(settings.shader.name().to_string())),
        (Emulator, "crt_curvature", Some(settings.crt.curvature.to_string())),
        (Emulator, "crt_glow", Some(settings.crt.glow.to_string())),
        (Emulator, "monochrome", Some(settings.monochrome.name().to_string())),
        (Emulator, "composite", Some(settings.composite.mode.name().to_string())),
        (Emulator, "composite_era", Some(settings.composite.era.name().to_string())),
        (Emulator, "machine", Some(settings.machine.name().to_string())),
        (Emulator, "voodoo", yes_no(settings.voodoo.enabled)),
        (Emulator, "voodoo_memory", Some(settings.voodoo.board.megabytes().to_string())),
        (Emulator, "voodoo_renderer", Some(settings.voodoo.renderer.name().to_string())),
        (Emulator, "voodoo_scale", Some(settings.voodoo.scale.to_string())),
        (Emulator, "capture_dir", Some(contract_home(&settings.capture_dir, home))),
        (Emulator, "record_ui", yes_no(settings.record_ui)),
        (Emulator, "record_shader", yes_no(settings.record_shader)),
        (
            Emulator,
            "cycles",
            Some(settings.cycles.to_string()),
        ),
        (
            Emulator,
            "cpu",
            Some(match settings.cpu {
                CpuModel::I386 => "386",
                CpuModel::I486 => "486",
                CpuModel::Pentium => "pentium",
                CpuModel::PentiumMmx => "pentium_mmx",
            }
            .to_string()),
        ),
        (Emulator, "core", Some(settings.core.name().to_string())),
        (Emulator, "memsize", Some(settings.memsize.to_string())),
        (Emulator, "ems", yes_no(settings.ems)),
        (Emulator, "umb", yes_no(settings.umb)),
        (Emulator, "dpmi", yes_no(settings.dpmi)),
        (Emulator, "dos_high", yes_no(settings.dos_high)),
        (Emulator, "dos_version", Some(settings.dos_version.name())),
        (Emulator, "ide_hard_disks", yes_no(settings.ide_hard_disks)),
        (Emulator, "boot_cdrom", yes_no(settings.boot_cdrom)),
        (Emulator, "keyboard_layout", Some(settings.keyboard_layout.name().to_string())),
        (Emulator, "mouse_autocapture", yes_no(settings.mouse_autocapture)),
        (Emulator, "mouse_capture_messages", yes_no(settings.mouse_capture_messages)),
        (Emulator, "rewind", yes_no(settings.rewind)),
        (Emulator, "rewind_memory", Some(settings.rewind_memory.to_string())),
        (Emulator, "hard_disk_speed", Some(settings.disk.hard_disk_speed.name().to_string())),
        (Emulator, "floppy_disk_speed", Some(settings.disk.floppy_disk_speed.name().to_string())),
        (Sound, "sbtype", Some(if sound.sb_installed { sb.model.name() } else { "none" }.to_string())),
        (Sound, "sbbase", Some(format!("{:X}", sb.base))),
        (Sound, "irq", Some(sb.irq.to_string())),
        (Sound, "dma", Some(sb.dma8.to_string())),
        (Sound, "hdma", Some(sb.dma16.to_string())),
        (Sound, "opl", Some(if sound.opl3 { "opl3" } else { "opl2" }.to_string())),
        (Sound, "awe32rom", sound.awe32rom.as_deref().map(|p| contract_home(p, home))),
        (Sound, "awe32ram", Some(sound.awe32ram.to_string())),
        (Sound, "soundfont", sound.soundfont.as_deref().map(|p| contract_home(p, home))),
        (Sound, "gus", yes_no(gus.enabled)),
        (Sound, "gusbase", Some(format!("{:X}", gus.base))),
        (Sound, "gusirq", Some(gus.irq.to_string())),
        (Sound, "gusdma", Some(gus.dma.to_string())),
        (
            Sound,
            "gusdrive",
            Some(gus.drive.map_or("none".to_string(), |d| crate::disk::drive_letter(d).to_string())),
        ),
        (Sound, "ultradir", gus.ultradir.clone()),
        (Sound, "midisynth", Some(sound.midisynth.name().to_string())),
        (Sound, "mt32roms", sound.mt32roms.as_deref().map(|p| contract_home(p, home))),
        (Sound, "mt32model", Some(sound.mt32model.name().to_string())),
        (Sound, "mt32lib", sound.mt32lib.as_deref().map(|p| contract_home(p, home))),
        (Sound, "midiport", (!sound.midiport.is_empty()).then(|| sound.midiport.clone())),
        (Sound, "lpt_dac", Some(sound.lpt_dac.name().to_string())),
        (Sound, "tandy", Some(sound.tandy.name().to_string())),
        (Sound, "hard_disk_noise", Some(settings.disk.hard_disk_noise.name().to_string())),
        (Sound, "floppy_disk_noise", Some(settings.disk.floppy_disk_noise.name().to_string())),
    ];
    entries.extend(mixer);
    let on_off = |on: bool| Some(if on { "on" } else { "off" }.to_string());
    entries.extend([
        (Section::Mixer, "speaker_filter", on_off(settings.mixer.speaker_filter)),
        (Section::Mixer, "sb_filter", Some(settings.mixer.sb_filter.name().to_string())),
        (Section::Mixer, "reverb", Some(settings.mixer.reverb.name().to_string())),
        (Section::Mixer, "reverb_mix", Some(settings.mixer.reverb_mix.to_string())),
        (Section::Mixer, "chorus", Some(settings.mixer.chorus.name().to_string())),
        (Section::Mixer, "chorus_mix", Some(settings.mixer.chorus_mix.to_string())),
    ]);
    entries.extend([
        (Section::Joystick, "joysticktype", Some(settings.joystick.kind.name().to_string())),
        (Section::Joystick, "deadzone", Some(settings.joystick.deadzone.to_string())),
    ]);
    entries.extend(settings.network.entries().into_iter().map(|(key, value)| (Section::Network, key, value)));
    entries.extend(settings.serial.entries().into_iter().map(|(key, value)| (Section::Serial, key, value)));
    entries.extend(settings.printer.entries(home).into_iter().map(|(key, value)| (Section::Printer, key, value)));
    entries.extend(settings.achievements.entries().into_iter().map(|(key, value)| (Section::Achievements, key, value)));
    entries
}

/// What a line of the file is, for `update_text`.
#[derive(Debug, PartialEq)]
enum Line {
    Header(Section),
    /// `key=value` (the key in lower case), or a drive line in `[drives]`.
    Setting(String),
    /// A commented-out `#key=value`, the template's examples.
    Example(String),
    /// Blank lines, other comments, `[autoexec]` commands.
    Other,
}

/// The section and kind of every line.
fn classify(lines: &[String]) -> Vec<(Section, Line)> {
    let mut section = Section::None;
    let key_of = |text: &str| {
        let key = text.split_once('=')?.0.trim();
        (!key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == ':' || c == '_'))
            .then(|| key.to_ascii_lowercase())
    };
    lines
        .iter()
        .map(|raw| {
            let line = raw.trim();
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                section = Section::parse(name);
                return (section, Line::Header(section));
            }
            let kind = match section {
                Section::Emulator
                | Section::Sound
                | Section::Mixer
                | Section::Joystick
                | Section::Network
                | Section::Serial
                | Section::Printer
                | Section::Achievements
                | Section::Drives => {
                    if let Some(comment) = line.strip_prefix(['#', ';']) {
                        key_of(comment.trim_start_matches(['#', ';']).trim_start())
                            .map_or(Line::Other, Line::Example)
                    } else {
                        key_of(line).map_or(Line::Other, Line::Setting)
                    }
                }
                _ => Line::Other,
            };
            (section, kind)
        })
        .collect()
}

/// Index after the last non-blank line of the last `[section]` block, or
/// None if the file has no such section.
fn section_end(lines: &[String], layout: &[(Section, Line)], section: Section) -> Option<usize> {
    let header = layout.iter().rposition(|(_, l)| *l == Line::Header(section))?;
    let mut last = header;
    for (i, (_, line)) in layout.iter().enumerate().skip(header + 1) {
        if matches!(line, Line::Header(_)) {
            break;
        }
        if !lines[i].trim().is_empty() {
            last = i;
        }
    }
    Some(last + 1)
}

/// Where a new line of `section` goes: after the commented example of
/// `key` if there is one, else at the end of the section. Creates the
/// section, before `[autoexec]` or at the end, if the file has none.
fn insertion_point(lines: &mut Vec<String>, section: Section, key: Option<&str>) -> usize {
    let layout = classify(lines);
    if let Some(key) = key
        && let Some(i) = layout.iter().rposition(|(s, l)| *s == section && *l == Line::Example(key.to_string()))
    {
        return i + 1;
    }
    if let Some(end) = section_end(lines, &layout, section) {
        return end;
    }
    let header = format!("[{}]", section.name());
    match layout.iter().position(|(_, l)| *l == Line::Header(Section::Autoexec)) {
        Some(autoexec) => {
            lines.splice(autoexec..autoexec, [header, String::new()]);
            autoexec + 1
        }
        None => {
            if lines.last().is_some_and(|l| !l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(header);
            lines.len()
        }
    }
}

/// `line` (`key=value`) with a new value, keeping the key as written and
/// the spacing around the '='.
fn with_value(line: &str, value: &str) -> String {
    let eq = line.find('=').unwrap_or(line.len());
    let rest = line.get(eq + 1..).unwrap_or("");
    let gap = rest.len() - rest.trim_start().len();
    format!("{}={}{}", &line[..eq], &rest[..gap], value)
}

/// Set `key` in `section` to `value`: the last line that sets it gets the
/// new value, or a new line is added. With None, the lines that set it
/// are commented out.
fn set_key(lines: &mut Vec<String>, section: Section, key: &str, value: Option<&str>) {
    let layout = classify(lines);
    let setting = Line::Setting(key.to_string());
    let existing: Vec<usize> =
        (0..lines.len()).filter(|&i| layout[i].0 == section && layout[i].1 == setting).collect();
    match (existing.last(), value) {
        (Some(&i), Some(value)) => lines[i] = with_value(&lines[i], value),
        (Some(_), None) => {
            for i in existing {
                lines[i] = format!("#{}", lines[i]);
            }
        }
        (None, Some(value)) => {
            let at = insertion_point(lines, section, Some(key));
            lines.insert(at, format!("{}={}", key, value));
        }
        (None, None) => {}
    }
}

/// Set the `[drives]` line of `drive` to `spec`: the last line for the
/// drive gets the new value, or a line is added after the other drives.
/// With None, the drive's lines are removed.
fn set_drive(lines: &mut Vec<String>, drive: u8, spec: Option<&MountSpec>, home: Option<&Path>) {
    let layout = classify(lines);
    let is_drive = |(s, l): &(Section, Line)| {
        *s == Section::Drives && matches!(l, Line::Setting(k) if parse_drive_name(k).is_some())
    };
    let existing: Vec<usize> = (0..lines.len())
        .filter(|&i| {
            is_drive(&layout[i])
                && matches!(&layout[i].1, Line::Setting(k) if parse_drive_name(k) == Some(drive))
        })
        .collect();
    match (existing.last(), spec) {
        (Some(&i), Some(spec)) => lines[i] = with_value(&lines[i], &mount_spec_value(spec, home)),
        (Some(_), None) => {
            for &i in existing.iter().rev() {
                lines.remove(i);
            }
        }
        (None, Some(spec)) => {
            let line = format!("{}={}", crate::disk::drive_key(drive), mount_spec_value(spec, home));
            let at = match layout.iter().rposition(is_drive) {
                Some(last) => last + 1,
                None => insertion_point(lines, Section::Drives, None),
            };
            lines.insert(at, line);
        }
        (None, None) => {}
    }
}

/// Whether a line of `lines` sets `key` in `section`.
fn has_key(lines: &[String], section: Section, key: &str) -> bool {
    let setting = Line::Setting(key.to_string());
    classify(lines).into_iter().any(|(s, line)| s == section && line == setting)
}

/// A change to `[drives]`: the drive's new mount, or None for no drive.
pub type DriveChange = (u8, Option<MountSpec>);

/// `original` with `write` done to its lines and the drive changes written
/// into it, keeping its byte order mark and line ends.
fn edit_text(
    original: &str,
    drives: &[DriveChange],
    home: Option<&Path>,
    write: impl FnOnce(&mut Vec<String>),
) -> String {
    let (bom, text) = match original.strip_prefix('\u{FEFF}') {
        Some(rest) => ("\u{FEFF}", rest),
        None => ("", original),
    };
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let final_newline = text.is_empty() || text.ends_with('\n');
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();

    write(&mut lines);
    for (drive, spec) in drives {
        set_drive(&mut lines, *drive, spec.as_ref(), home);
    }

    let mut out = format!("{}{}", bom, lines.join(newline));
    if final_newline && !lines.is_empty() {
        out.push_str(newline);
    }
    out
}

/// `original` with what changed from `baseline` to `settings`, and the
/// drive changes, written into it. Everything else in the file (comments,
/// blank lines, `[autoexec]`, the settings that didn't change) stays as it
/// is.
pub fn update_text(
    original: &str,
    baseline: &Settings,
    settings: &Settings,
    drives: &[DriveChange],
    home: Option<&Path>,
) -> String {
    edit_text(original, drives, home, |lines| {
        let before = entries(baseline, home);
        for ((section, key, value), (_, _, old)) in entries(settings, home).into_iter().zip(before) {
            if value != old {
                set_key(lines, section, key, value.as_deref());
            }
        }
    })
}

/// `original`, the text of a configuration file whose relative paths are
/// from `base_dir`, with every setting in it: what changed from `baseline`
/// to `settings` is written as `update_text` does, and each setting the
/// file has no line for yet is added with the value the file stood for
/// without it, the default. So the lines already there that didn't change,
/// and the command-line options that stand in for them, stay as they are.
pub fn complete_text(
    original: &str,
    base_dir: &Path,
    baseline: &Settings,
    settings: &Settings,
    drives: &[DriveChange],
    home: Option<&Path>,
) -> String {
    let file = Settings::from_config(&parse(original, base_dir, home));
    edit_text(original, drives, home, |lines| {
        let before = entries(baseline, home).into_iter().zip(entries(&file, home));
        for ((section, key, value), ((_, _, old), (_, _, in_file))) in entries(settings, home).into_iter().zip(before) {
            if value != old {
                set_key(lines, section, key, value.as_deref());
            } else if !has_key(lines, section, key) {
                set_key(lines, section, key, in_file.as_deref());
            }
        }
    })
}

/// What `save` writes of the settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Saving {
    /// Every setting (`complete_text`): the configuration file.
    All,
    /// What changed (`update_text`): a game profile, which has only the
    /// game's own settings.
    Changes,
}

/// Save the settings, as `saving` says, and the drive changes into the
/// configuration file at `path`. The file is replaced in one step, so a
/// failed write leaves the old one.
pub fn save(
    path: &Path,
    baseline: &Settings,
    settings: &Settings,
    drives: &[DriveChange],
    home: Option<&Path>,
    saving: Saving,
) -> Result<(), String> {
    let original = read_or_template(path)?;
    let text = match saving {
        Saving::All => {
            let absolute = crate::hostfs::absolute(path).unwrap_or_else(|_| path.to_path_buf());
            let base_dir = absolute.parent().unwrap_or(Path::new(""));
            complete_text(&original, base_dir, baseline, settings, drives, home)
        }
        Saving::Changes => update_text(&original, baseline, settings, drives, home),
    };
    replace_file(path, &text)
}

/// The lines of `text`'s `[autoexec]` sections as they are written,
/// comments and blank lines too, without the blank lines at the end.
pub fn autoexec_lines(text: &str) -> Vec<String> {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut autoexec = Vec::new();
    for ((section, line), text) in classify(&lines).into_iter().zip(lines) {
        if matches!(line, Line::Header(_)) {
            trim_blank_end(&mut autoexec);
        } else if section == Section::Autoexec {
            autoexec.push(text);
        }
    }
    trim_blank_end(&mut autoexec);
    autoexec
}

fn trim_blank_end(lines: &mut Vec<String>) {
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
}

/// Make `autoexec` the lines of the `[autoexec]` section: the first one's,
/// with any later ones taken out, or a new section at the end.
fn replace_autoexec(lines: &mut Vec<String>, autoexec: &[String]) {
    let mut autoexec = autoexec.to_vec();
    trim_blank_end(&mut autoexec);
    let layout = classify(lines);
    let Some(header) = layout.iter().position(|(_, l)| *l == Line::Header(Section::Autoexec)) else {
        if lines.last().is_some_and(|l| !l.trim().is_empty()) {
            lines.push(String::new());
        }
        lines.push(format!("[{}]", Section::Autoexec.name()));
        lines.extend(autoexec);
        return;
    };
    let rest: Vec<String> = lines
        .drain(header + 1..)
        .zip(&layout[header + 1..])
        .filter(|(_, (section, _))| *section != Section::Autoexec)
        .map(|(line, _)| line)
        .collect();
    lines.append(&mut autoexec);
    // A blank line before the section after it.
    if rest.first().is_some_and(|l| !l.trim().is_empty()) {
        lines.push(String::new());
    }
    lines.extend(rest);
}

/// `original` with `autoexec` as the lines of its `[autoexec]` section,
/// and everything else as it is.
pub fn with_autoexec(original: &str, autoexec: &[String]) -> String {
    edit_text(original, &[], None, |lines| replace_autoexec(lines, autoexec))
}

/// The `[autoexec]` lines of the configuration file at `path` (see
/// `autoexec_lines`), which has the template's text if it isn't there yet.
pub fn load_autoexec(path: &Path) -> Result<Vec<String>, String> {
    read_or_template(path).map(|text| autoexec_lines(&text))
}

/// Make `autoexec` the `[autoexec]` section of the configuration file at
/// `path` (see `load_autoexec`).
pub fn save_autoexec(path: &Path, autoexec: &[String]) -> Result<(), String> {
    let original = read_or_template(path)?;
    replace_file(path, &with_autoexec(&original, autoexec))
}

/// The text of the configuration file at `path`, or the template's if it
/// isn't there yet.
fn read_or_template(path: &Path) -> Result<String, String> {
    match crate::hostfs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(TEMPLATE.to_string()),
        Err(e) => Err(format!("cannot read {}: {}", path.display(), e)),
    }
}

/// Replace the file at `path` with `text` in one step, so a failed write
/// leaves the old one. Where the frontend keeps the files (`hostfs`), it
/// is written as it is.
fn replace_file(path: &Path, text: &str) -> Result<(), String> {
    if crate::hostfs::is_foreign(path) {
        return crate::hostfs::write(path, text).map_err(|e| format!("cannot write {}: {}", path.display(), e));
    }
    // Write through a symbolic link rather than replacing it, and keep the
    // file's permissions.
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut temp_name = target.file_name().unwrap_or_default().to_os_string();
    temp_name.push(".tmp");
    let temp = target.with_file_name(temp_name);
    let permissions = fs::metadata(&target).map(|m| m.permissions()).ok();
    fs::write(&temp, text)
        .and_then(|()| permissions.map_or(Ok(()), |p| fs::set_permissions(&temp, p)))
        .and_then(|()| fs::rename(&temp, &target))
        .map_err(|e| {
            let _ = fs::remove_file(&temp);
            format!("cannot write {}: {}", target.display(), e)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk::DriveKind;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::path::absolute(PathBuf::from("target/test_config_unit").join(name)).unwrap();
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn locate_priority() {
        let base = scratch("locate");
        let cwd = base.join("work");
        let exe = base.join("bin");
        let default = base.join("home/rust-dos/rust-dos.conf");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(&exe).unwrap();
        fs::create_dir_all(default.parent().unwrap()).unwrap();
        fs::write(base.join("custom.conf"), "").unwrap();

        // Nothing yet: the default path is where the template goes
        assert_eq!(
            locate(None, &cwd, None, Some(default.clone())),
            Ok(Located::NotFound {
                default: Some(default.clone())
            })
        );

        fs::write(&default, "").unwrap();
        assert_eq!(
            locate(None, &cwd, None, Some(default.clone())),
            Ok(Located::Found(default.clone(), ConfigSource::UserDefault))
        );

        // A file next to the executable beats the per-user one
        fs::write(exe.join(FILE_NAME), "").unwrap();
        assert_eq!(
            locate(None, &cwd, Some(&exe), Some(default.clone())),
            Ok(Located::Found(exe.join(FILE_NAME), ConfigSource::ExeDir))
        );

        fs::write(cwd.join(FILE_NAME), "").unwrap();
        assert_eq!(
            locate(None, &cwd, Some(&exe), Some(default.clone())),
            Ok(Located::Found(
                cwd.join(FILE_NAME),
                ConfigSource::WorkingDir
            ))
        );

        // Relative --config paths are taken from the working directory
        assert_eq!(
            locate(
                Some(Path::new("../custom.conf")),
                &cwd,
                None,
                Some(default.clone())
            ),
            Ok(Located::Found(
                cwd.join("../custom.conf"),
                ConfigSource::CommandLine
            ))
        );
        assert!(locate(Some(Path::new("missing.conf")), &cwd, None, Some(default)).is_err());
    }

    #[test]
    fn parses_sections_drives_and_autoexec() {
        let text = "\u{FEFF}# comment\r\n\
            [Emulator]\r\n\
            scale = 3\r\n\
            cycles = 3000\r\n\
            \r\n\
            [DRIVES]\r\n\
            c = ~/dos\r\n\
            A: = floppy floppy -label disk1\r\n\
            D=\"My CD\" cdrom\r\n\
            E=C:\\Games\\Dos -ro\r\n\
            ; another comment\r\n\
            [autoexec]\r\n\
            @echo off\r\n\
            MOUNT F ~/x\r\n\
            # skipped\r\n\
            D:\r\n";
        let base = Path::new("/cfg");
        let home = Path::new("/home/u");
        let config = parse(text, base, Some(home));
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
        assert_eq!(config.scale, Some(3));
        assert_eq!(config.cycles, Some(CpuSpeed::Fixed(3000)));

        let c = config.drive(2).unwrap();
        assert_eq!(c.path, home.join("dos"));
        assert_eq!(c.opts.kind, DriveKind::HardDisk);
        let a = config.drive(0).unwrap();
        assert_eq!(a.path, base.join("floppy"));
        assert_eq!(a.opts.kind, DriveKind::Floppy);
        assert_eq!(a.opts.label.as_deref(), Some("disk1"));
        let d = config.drive(3).unwrap();
        assert_eq!(d.path, base.join("My CD"));
        assert_eq!(d.opts.kind, DriveKind::CdRom);
        let e = config.drive(4).unwrap();
        assert!(e.path.to_string_lossy().ends_with(r"C:\Games\Dos"));
        assert!(e.opts.read_only);

        assert_eq!(config.autoexec, ["@echo off", "MOUNT F ~/x", "D:"]);
    }

    #[test]
    fn problems_become_warnings() {
        let text = "orphan=1\n\
            [emulator]\n\
            scale=0\n\
            colour=blue\n\
            [drives]\n\
            Z=/tmp\n\
            7=/tmp\n\
            D=/one\n\
            D=/two\n\
            E=/x zip\n\
            nonsense\n\
            [modem]\n\
            ignored=1\n";
        let config = parse(text, Path::new("/cfg"), None);
        let joined = config.warnings.join("\n");
        for expected in [
            "line 1: setting outside of a section",
            "line 3: invalid scale '0'",
            "line 4: unknown setting 'colour'",
            "line 6: drive Z: is reserved",
            "line 7: invalid drive letter '7'",
            "line 9: drive D: defined twice",
            "line 10: drive E: Unknown option 'zip'",
            "line 11: expected key=value",
            "line 12: unknown section [modem]",
        ] {
            assert!(
                joined.contains(expected),
                "missing '{}' in:\n{}",
                expected,
                joined
            );
        }
        assert_eq!(config.warnings.len(), 9, "{}", joined);
        assert_eq!(config.drive(3).unwrap().path, Path::new("/two"));
        assert_eq!(config.scale, None);
    }

    #[test]
    fn sound_section() {
        let config = parse(
            "[sound]\nsbtype=sbpro2\nsbbase=240\nirq=5\ndma=3\nopl=opl2\nsoundfont=gm.sf2\ngus=false\n",
            Path::new("/cfg"),
            None,
        );
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
        let sb = config.sound.card().unwrap();
        assert_eq!((sb.model, sb.base, sb.irq, sb.dma8), (crate::sb::SbModel::SbPro2, 0x240, 5, 3));
        assert!(!config.sound.opl3);
        assert_eq!(config.sound.soundfont.as_deref(), Some(Path::new("/cfg/gm.sf2")));
        assert_eq!(sb.blaster(), "A240 I5 D3 T4");

        let config = parse("[sound]\nsbtype=none\nirq=4\n", Path::new("/cfg"), None);
        assert_eq!(config.sound.card(), None);
        assert!(config.warnings[0].contains("invalid irq '4'"));
    }

    #[test]
    fn a_dac_on_the_parallel_port() {
        let config = parse("[sound]\nlpt_dac=Disney\n", Path::new("/cfg"), None);
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
        assert_eq!(config.sound.lpt_dac, LptDacType::Disney);
        let config = parse("[sound]\nlpt_dac=ston1\n", Path::new("/cfg"), None);
        assert_eq!((config.sound.lpt_dac, config.warnings.len()), (LptDacType::None, 1));
    }

    #[test]
    fn ultrasound_settings() {
        let config = parse(
            "[sound]\ngusbase=260\ngusirq=11\ngusdma=6\nultradir=D:\\GUS\nmidisynth=gus\n",
            Path::new("/cfg"),
            None,
        );
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
        let gus = config.sound.ultrasound().unwrap();
        assert_eq!((gus.base, gus.irq, gus.dma, gus.ultradir().as_str()), (0x260, 11, 6, "D:\\GUS"));
        assert_eq!(gus.ultrasnd(), "260,6,6,11,11");
        assert!(!gus.builtin());
        assert_eq!(config.sound.midisynth, MidiSynth::Gus);

        let config = parse("[sound]\ngus=off\n", Path::new("/cfg"), None);
        assert_eq!(config.sound.ultrasound(), None);

        let config = parse(
            "[sound]\ngusbase=230\ngusirq=4\ngusdma=2\nmidisynth=mt64\ngusdrive=Z\n",
            Path::new("/cfg"),
            None,
        );
        assert_eq!(config.warnings.len(), 5, "{:?}", config.warnings);
        assert_eq!(config.sound.ultrasound(), Some(crate::gus::GusConfig::default()));
    }

    #[test]
    fn mt32_and_host_midi_settings() {
        let text = "[sound]\nmidisynth=mt32\nmt32roms=~/roms\nmt32model=CM-32L\nmt32lib=lib/libmt32emu.so\n";
        let config = parse(text, Path::new("/cfg"), Some(Path::new("/home/u")));
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
        let sound = &config.sound;
        assert_eq!(sound.midisynth, MidiSynth::Mt32);
        assert_eq!(sound.mt32roms.as_deref(), Some(Path::new("/home/u/roms")));
        assert_eq!(sound.mt32model, Mt32Model::Cm32l);
        assert_eq!(sound.mt32lib.as_deref(), Some(Path::new("/cfg/lib/libmt32emu.so")));

        let config = parse("[sound]\nmidisynth=host\nmidiport=UM-ONE\n", Path::new("/cfg"), None);
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
        assert_eq!((config.sound.midisynth, config.sound.midiport.as_str()), (MidiSynth::Host, "UM-ONE"));

        let config = parse("[sound]\nmt32model=sc55\n", Path::new("/cfg"), None);
        assert_eq!(config.warnings.len(), 1, "{:?}", config.warnings);
        assert_eq!(config.sound.mt32model, Mt32Model::Auto);
    }

    #[test]
    fn the_builtin_ultrasound_drive() {
        // X:, where ULTRADIR points unless it is set.
        let gus = parse("", Path::new("/cfg"), None).sound.gus;
        assert_eq!((gus.drive, gus.ultradir().as_str(), gus.builtin()), (Some(23), "X:\\ULTRASND", true));
        let gus = parse("[sound]\ngusdrive=u:\n", Path::new("/cfg"), None).sound.gus;
        assert_eq!((gus.drive, gus.ultradir().as_str()), (Some(20), "U:\\ULTRASND"));
        let gus = parse("[sound]\ngusdrive=none\n", Path::new("/cfg"), None).sound.gus;
        assert_eq!((gus.drive, gus.ultradir().as_str(), gus.builtin()), (None, "C:\\ULTRASND", false));
        let gus = parse("[sound]\nultradir=D:\\GUS\n", Path::new("/cfg"), None).sound.gus;
        assert_eq!((gus.drive, gus.ultradir().as_str(), gus.builtin()), (Some(23), "D:\\GUS", false));

        for bad in ["C", "Z", "A:", "XY", ""] {
            let config = parse(&format!("[sound]\ngusdrive={}\n", bad), Path::new("/cfg"), None);
            assert_eq!(config.warnings.len(), 1, "{}: {:?}", bad, config.warnings);
            assert_eq!(config.sound.gus.drive, Some(23));
        }

        // A drive of the user's own on X: wins.
        let config = parse("[sound]\n[drives]\nX=/games\n", Path::new("/cfg"), None);
        assert!(config.warnings[0].contains("gusdrive X:"), "{:?}", config.warnings);
        assert_eq!(config.sound.gus.ultradir(), "C:\\ULTRASND");
        let config = parse("[sound]\ngus=false\n[drives]\nX=/games\n", Path::new("/cfg"), None);
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
    }

    #[test]
    fn ultrasound_conflicts_with_the_sound_blaster() {
        let config = parse("[sound]\ngusbase=220\n", Path::new("/cfg"), None);
        assert_eq!(config.sound.ultrasound(), None);
        assert!(config.warnings[0].contains("gusbase 220"), "{:?}", config.warnings);

        let config = parse("[sound]\ngusirq=7\ngusdma=1\n", Path::new("/cfg"), None);
        assert!(config.sound.ultrasound().is_some());
        assert_eq!(config.warnings.len(), 2, "{:?}", config.warnings);

        // Without a Sound Blaster, 220h is free.
        let config = parse("[sound]\nsbtype=none\ngusbase=220\n", Path::new("/cfg"), None);
        assert_eq!(config.sound.ultrasound().map(|g| g.base), Some(0x220));
    }

    #[test]
    fn memsize_range() {
        let config = parse("[emulator]\nmemsize=32\n", Path::new("/cfg"), None);
        assert_eq!(config.memsize, Some(32));
        let config = parse("[emulator]\nmemsize=1024\n", Path::new("/cfg"), None);
        assert_eq!(config.memsize, None);
        assert!(config.warnings[0].contains("invalid memsize '1024'"));
    }

    #[test]
    fn memsize_goes_as_far_as_the_cpu_takes() {
        let config = parse("[emulator]\ncpu=pentium_mmx\nmemsize=512\n", Path::new("/cfg"), None);
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
        assert_eq!(Settings::from_config(&config).memsize, 512);
        let config = parse("[emulator]\ncpu=pentium\nmemsize=512\n", Path::new("/cfg"), None);
        assert_eq!(config.warnings, ["memsize 512 MB is more than a Pentium takes: 256 MB"]);
        assert_eq!(Settings::from_config(&config).memsize, 256);
        // Without a cpu, the default 486's.
        let config = parse("[emulator]\nmemsize=256\n", Path::new("/cfg"), None);
        assert_eq!(Settings::from_config(&config).memsize, 128);
        let config = parse("[emulator]\ncpu=386\nmemsize=128\n", Path::new("/cfg"), None);
        assert_eq!(Settings::from_config(&config).memsize, 64);
    }

    #[test]
    fn a_game_section_names_the_profile() {
        let config = parse("[game]\nname = Commander Keen 4\n[autoexec]\nKEEN4E\n", Path::new("/cfg"), None);
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
        assert_eq!(config.game_name.as_deref(), Some("Commander Keen 4"));
        let config = parse("[game]\nname=\nyear=1991\n", Path::new("/cfg"), None);
        assert_eq!((config.game_name, config.warnings.len()), (None, 2));
    }

    #[test]
    fn memory_settings() {
        let config = parse("[emulator]\nems=off\nUMB=no\ndpmi=false\n", Path::new("/cfg"), None);
        assert_eq!((config.ems, config.umb, config.dpmi), (Some(false), Some(false), Some(false)));
        let settings = Settings::from_config(&config);
        assert!(!settings.ems && !settings.umb && !settings.dpmi);
        let settings = Settings::from_config(&parse("", Path::new("/cfg"), None));
        assert!(settings.ems && settings.umb && settings.dpmi);
        let config = parse("[emulator]\nems=lots\n", Path::new("/cfg"), None);
        assert_eq!((config.ems, config.warnings.len()), (None, 1));
    }

    #[test]
    fn dos_versions() {
        let version = |s: &str| DosVersion::parse(s).map(DosVersion::name);
        assert_eq!(version("5"), Some("5.00".to_string()));
        assert_eq!(version("5.0"), Some("5.00".to_string()));
        assert_eq!(version("7.1"), Some("7.10".to_string()));
        assert_eq!(version("7.10"), Some("7.10".to_string()));
        assert_eq!(version(" 6.22 "), Some("6.22".to_string()));
        for bad in ["", "1.0", "10", "7.", "7.100", "7.x", "seven"] {
            assert_eq!(version(bad), None, "{}", bad);
        }
        assert!(DosVersion::new(7, 10).at_least(7, 0) && !DosVersion::new(6, 22).at_least(7, 0));
        let config = parse("[emulator]\ndos_version=7.1\n", Path::new("/cfg"), None);
        assert_eq!(Settings::from_config(&config).dos_version, DosVersion::new(7, 10));
        assert_eq!(Settings::from_config(&parse("", Path::new("/cfg"), None)).dos_version, DosVersion::new(5, 0));
        let config = parse("[emulator]\ndos_version=12\n", Path::new("/cfg"), None);
        assert_eq!((config.dos_version, config.warnings.len()), (None, 1));
    }

    #[test]
    fn cpu_model() {
        let config = parse("[emulator]\ncpu=386\n", Path::new("/cfg"), None);
        assert_eq!(config.cpu, Some(crate::cpu::CpuModel::I386));
        let config = parse("[emulator]\ncpu=Pentium\n", Path::new("/cfg"), None);
        assert_eq!(config.cpu, Some(crate::cpu::CpuModel::Pentium));
        let config = parse("[emulator]\ncpu=8086\n", Path::new("/cfg"), None);
        assert_eq!(config.cpu, None);
        assert!(config.warnings[0].contains("invalid cpu '8086'"));
    }

    #[test]
    fn cycles_accepts_max_and_rejects_nonsense() {
        let config = parse("[emulator]\ncycles=Max\n", Path::new("/cfg"), None);
        assert_eq!(config.cycles, Some(CpuSpeed::Max));

        let config = parse("[emulator]\ncycles=fast\n", Path::new("/cfg"), None);
        assert_eq!(config.cycles, None);
        assert_eq!(config.warnings.len(), 1);
        assert!(config.warnings[0].starts_with("line 2: invalid cycles 'fast'"));
    }

    #[test]
    fn core_takes_auto_dynamic_or_normal() {
        let config = parse("[emulator]\ncore=Dynamic\n", Path::new("/cfg"), None);
        assert_eq!(config.core, Some(CoreMode::Dynamic));
        assert_eq!(Settings::from_config(&config).core, CoreMode::Dynamic);
        assert_eq!(Settings::default().core, CoreMode::Auto);

        let config = parse("[emulator]\ncore=fast\n", Path::new("/cfg"), None);
        assert_eq!(config.core, None);
        assert!(config.warnings[0].starts_with("line 2: invalid core 'fast'"), "{:?}", config.warnings);
    }

    #[test]
    fn template_is_all_comments() {
        let config = parse(TEMPLATE, Path::new("/cfg"), None);
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
        assert!(config.drives.is_empty());
        assert!(config.autoexec.is_empty());
        assert_eq!(config.scale, None);
        assert_eq!((config.cycles, config.core), (None, None));
        assert_eq!((config.ems, config.umb), (None, None));
        assert_eq!((config.fullscreen, config.aspect, config.filter), (None, None, None));
        assert_eq!((config.shader, config.monochrome, config.machine), (None, None, None));
        assert_eq!(config.sound, SoundConfig::default());
        assert_eq!(config.mixer, MixerSettings::default());
        assert_eq!(config.joystick, JoystickSettings::default());
    }

    #[test]
    fn display_settings() {
        let text = "[emulator]\nfullscreen=yes\naspect=off\nfilter=Linear\nshader=CRT\nmonochrome=Amber\ncrt_curvature=0\nCRT_Glow=45%\n";
        let config = parse(text, Path::new("/cfg"), None);
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
        assert_eq!((config.fullscreen, config.aspect, config.filter), (Some(true), Some(false), Some(Filter::Linear)));
        assert_eq!((config.shader, config.monochrome), (Some(Shader::Crt), Some(Monochrome::Amber)));
        assert_eq!(Settings::from_config(&config).crt, CrtSettings { curvature: 0, glow: 45 });
        let text = "[emulator]\nfullscreen=maybe\nfilter=blur\nshader=bent\nmonochrome=blue\ncrt_curvature=200\ncrt_glow=lots\n";
        let config = parse(text, Path::new("/cfg"), None);
        assert_eq!(config.warnings.len(), 6, "{:?}", config.warnings);
        assert!(config.warnings[5].contains("invalid crt_glow 'lots' (0 to 100)"), "{:?}", config.warnings);
        assert_eq!(Settings::from_config(&config), Settings::default());
    }

    #[test]
    fn machine_settings() {
        let config = parse("[emulator]\nmachine=VGAonly\n", Path::new("/cfg"), None);
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
        assert_eq!(config.machine, Some(Adapter::Vga));
        assert_eq!(parse("[emulator]\nmachine=svga_s3\n", Path::new("/cfg"), None).machine, Some(Adapter::S3));
        assert_eq!(parse("[emulator]\nmachine=svga_et4000\n", Path::new("/cfg"), None).machine, Some(Adapter::Et4000));
        assert_eq!(parse("[emulator]\nmachine=svga_paradise\n", Path::new("/cfg"), None).machine, Some(Adapter::Svga));
        assert_eq!(parse("[emulator]\nmachine=PCjr\n", Path::new("/cfg"), None).machine, Some(Adapter::Pcjr));
        assert_eq!(parse("[emulator]\nmachine=tandy\n", Path::new("/cfg"), None).machine, Some(Adapter::Tandy));
        let config = parse("[emulator]\nmachine=mcga\n", Path::new("/cfg"), None);
        assert_eq!((config.machine, config.warnings.len()), (None, 1));
    }

    #[test]
    fn composite_settings() {
        let config = parse("[emulator]\ncomposite=on\ncomposite_era=NEW\n", Path::new("/cfg"), None);
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
        let settings = Settings::from_config(&config);
        assert_eq!(settings.composite, CompositeSettings { mode: CompositeMode::On, era: CompositeEra::New });
        let config = parse("[emulator]\ncomposite=rgb\ncomposite_era=1985\n", Path::new("/cfg"), None);
        assert_eq!(config.warnings.len(), 2, "{:?}", config.warnings);
        assert_eq!(Settings::from_config(&config).composite, CompositeSettings::default());
    }

    #[test]
    fn mixer_settings() {
        let text = "[mixer]\nmaster=80\nFM = 150%\ncdaudio=0\nsb=300\nbass=10\n";
        let config = parse(text, Path::new("/cfg"), None);
        assert_eq!(config.warnings.len(), 2, "{:?}", config.warnings);
        assert!(config.warnings[0].starts_with("line 5: sb: invalid volume '300'"), "{:?}", config.warnings);
        assert!(config.warnings[1].starts_with("line 6: unknown setting 'bass'"), "{:?}", config.warnings);
        let mixer = Settings::from_config(&config).mixer;
        let levels = Channel::ALL.map(|channel| mixer.level(channel));
        assert_eq!(levels, [80, 100, 100, 150, 100, 100, 0, 100, 100, 100, 100]);
        assert!(mixer.speaker_filter);
        assert_eq!((mixer.sb_filter, mixer.reverb, mixer.chorus), (SbFilter::Auto, ReverbPreset::Off, ChorusPreset::Off));

        assert_eq!((mixer.reverb_mix, mixer.chorus_mix), (50, 50));

        let text = "[mixer]\nspeaker_filter=off\nsb_filter=OFF\nreverb=Large\nchorus=light\nreverb_mix=80%\nChorus_Mix=0\n";
        let config = parse(text, Path::new("/cfg"), None);
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
        let mixer = config.mixer;
        assert!(!mixer.speaker_filter);
        assert_eq!((mixer.sb_filter, mixer.reverb, mixer.chorus), (SbFilter::Off, ReverbPreset::Large, ChorusPreset::Light));
        assert_eq!((mixer.reverb_mix, mixer.chorus_mix), (80, 0));
        let text = "[mixer]\nspeaker_filter=maybe\nsb_filter=sb1\nreverb=hall\nchorus=heavy\nreverb_mix=150\nchorus_mix=wet\n";
        let config = parse(text, Path::new("/cfg"), None);
        assert_eq!(config.warnings.len(), 6, "{:?}", config.warnings);
        assert!(config.warnings[4].contains("reverb_mix: invalid mix '150' (0 to 100)"), "{:?}", config.warnings);
    }

    #[test]
    fn joystick_settings() {
        let text = "[Joystick]\njoysticktype=4AXIS\ndeadzone=25%\n";
        let config = parse(text, Path::new("/cfg"), None);
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
        assert_eq!(config.joystick, JoystickSettings { kind: JoystickType::FourAxis, deadzone: 25 });
        let text = "[joystick]\njoysticktype=fcs\ndeadzone=95\ntimed=false\n";
        let config = parse(text, Path::new("/cfg"), None);
        assert_eq!(config.warnings.len(), 3, "{:?}", config.warnings);
        assert_eq!(config.joystick, JoystickSettings::default());
    }

    /// Settings with every value away from its default.
    fn changed_settings() -> Settings {
        let sound = SoundConfig {
            sb: crate::sb::SbConfig { model: crate::sb::SbModel::SbPro2, base: 0x240, irq: 5, dma8: 3, dma16: 6 },
            sb_installed: true,
            opl3: false,
            awe32rom: Some(PathBuf::from("/home/u/roms/awe32.raw")),
            awe32ram: 2048,
            soundfont: Some(PathBuf::from("/home/u/sf/General User.sf2")),
            gus: crate::gus::GusConfig {
                enabled: true,
                base: 0x260,
                irq: 11,
                dma: 6,
                drive: Some(20),
                ultradir: Some("D:\\GUS".to_string()),
            },
            midisynth: MidiSynth::Gus,
            mt32roms: Some(PathBuf::from("/home/u/roms")),
            mt32model: Mt32Model::Cm32l,
            mt32lib: Some(PathBuf::from("/opt/munt/libmt32emu.so")),
            midiport: "FLUID".to_string(),
            lpt_dac: LptDacType::Disney,
            tandy: crate::sn76489::TandySound::On,
        };
        Settings {
            scale: 3,
            fullscreen: true,
            aspect: true,
            filter: Filter::Linear,
            shader: Shader::Crt,
            crt: CrtSettings { curvature: 70, glow: 90 },
            monochrome: Monochrome::Green,
            composite: CompositeSettings { mode: CompositeMode::On, era: CompositeEra::New },
            machine: Adapter::Vga,
            voodoo: crate::voodoo::VoodooSettings {
                enabled: true,
                board: crate::voodoo::Board::Standard,
                renderer: crate::voodoo::Renderer::OpenGl,
                scale: 3,
            },
            capture_dir: PathBuf::from("/home/u/dos captures"),
            record_ui: true,
            record_shader: true,
            cycles: CpuSpeed::Fixed(3000),
            cpu: CpuModel::I386,
            core: CoreMode::Dynamic,
            memsize: 32,
            ems: false,
            umb: false,
            dpmi: false,
            dos_high: false,
            dos_version: DosVersion::new(7, 10),
            ide_hard_disks: false,
            boot_cdrom: false,
            keyboard_layout: LayoutSetting::Named("gr"),
            mouse_autocapture: false,
            mouse_capture_messages: false,
            rewind: true,
            rewind_memory: 512,
            sound,
            disk: DiskSettings {
                hard_disk_speed: DiskSpeed::Medium,
                floppy_disk_speed: DiskSpeed::Slow,
                hard_disk_noise: NoiseMode::On,
                floppy_disk_noise: NoiseMode::SeekOnly,
            },
            mixer: {
                let mut mixer = MixerSettings::default();
                for (i, channel) in Channel::ALL.into_iter().enumerate() {
                    mixer.set_level(channel, 10 * i as u16 + 5);
                }
                mixer.speaker_filter = false;
                mixer.sb_filter = SbFilter::Off;
                mixer.reverb = ReverbPreset::Medium;
                mixer.chorus = ChorusPreset::Strong;
                mixer.reverb_mix = 70;
                mixer.chorus_mix = 20;
                mixer
            },
            joystick: JoystickSettings { kind: JoystickType::TwoAxis, deadzone: 20 },
            network: crate::net::NetSettings {
                lan: Some("relay.example.com".into()),
                room: "doom".into(),
                ..Default::default()
            },
            serial: crate::serial::SerialSettings {
                ports: [
                    crate::serial::PortType::Empty,
                    crate::serial::PortType::NullModem,
                    crate::serial::PortType::Mouse,
                    crate::serial::PortType::Off,
                ],
                modem_listen: Some(2323),
                ..Default::default()
            },
            printer: crate::printer::PrinterSettings {
                output: crate::printer::PrinterOutput::Png,
                paper: crate::printer::Paper::A4,
                docpath: PathBuf::from("/home/u/printouts"),
                device: Some("Office".into()),
                ..Default::default()
            },
            achievements: crate::achievements::AchievementSettings {
                enabled: true,
                username: "Player".to_string(),
                token: "abc123".to_string(),
                hardcore: true,
            },
        }
    }

    fn drive_specs() -> Vec<MountSpec> {
        use crate::disk::MountOptions;
        vec![
            MountSpec { drive: 2, path: "/home/u/dos".into(), opts: MountOptions::default() },
            MountSpec {
                drive: 3,
                path: "/home/u/cd images/game.cue".into(),
                opts: MountOptions { kind: DriveKind::CdRom, label: Some("GAME".into()), read_only: false, ..Default::default() },
            },
            MountSpec {
                drive: 0,
                path: "/home/u/disks/disk 1.img".into(),
                opts: MountOptions {
                    kind: DriveKind::Floppy,
                    more_images: vec!["/home/u/disks/disk 2.img".into()],
                    ..Default::default()
                },
            },
        ]
    }

    #[test]
    fn saved_settings_parse_back() {
        let home = Path::new("/home/u");
        let settings = changed_settings();
        let changes: Vec<DriveChange> = drive_specs().into_iter().map(|s| (s.drive, Some(s))).collect();
        let text = update_text(TEMPLATE, &Settings::default(), &settings, &changes, Some(home));
        let config = parse(&text, Path::new("/cfg"), Some(home));
        assert!(config.warnings.is_empty(), "{:?}\n{}", config.warnings, text);
        assert_eq!(Settings::from_config(&config), settings);
        assert_eq!(config.drives, drive_specs());

        // The template's examples and comments stay, each new line right
        // after its example.
        for line in TEMPLATE.lines() {
            assert!(text.contains(line), "lost '{}'", line);
        }
        assert!(text.contains("#scale=2\nscale=3\n"), "{}", text);
        assert!(text.contains("#shader=none\nshader=crt\n"), "{}", text);
        assert!(text.contains("#monochrome=off\nmonochrome=green\n"), "{}", text);
        assert!(text.contains("#machine=svga\nmachine=vga\n"), "{}", text);
        assert!(text.contains("#capture_dir=capture\ncapture_dir=~/dos captures\n"), "{}", text);
        assert!(text.contains("#record_ui=false\nrecord_ui=true\n"), "{}", text);
        assert!(text.contains("#record_shader=false\nrecord_shader=true\n"), "{}", text);
        assert!(text.contains("#master=100\nmaster=5\n"), "{}", text);
        assert!(text.contains("#disknoise=100\ndisknoise=75\n"), "{}", text);
        assert!(text.contains("#joysticktype=auto\njoysticktype=2axis\n"), "{}", text);
        assert!(text.contains("#sbtype=sb16\nsbtype=sbpro2\n"), "{}", text);
        assert!(text.contains("#E=~/dos/images/game.cue\nC=~/dos\nD=\"~/cd images/game.cue\" cdrom -label GAME\n"), "{}", text);
        assert!(text.contains("soundfont=~/sf/General User.sf2\n"), "{}", text);

        // Saving the same again changes nothing.
        assert_eq!(update_text(&text, &settings, &settings, &changes, Some(home)), text);
    }

    #[test]
    fn unchanged_settings_leave_the_file_alone() {
        let settings = changed_settings();
        assert_eq!(update_text(TEMPLATE, &settings, &settings, &[], None), TEMPLATE);
        assert_eq!(update_text("", &settings, &settings, &[], None), "");
        assert_eq!(update_text("[emulator]\nscale=2", &settings, &settings, &[], None), "[emulator]\nscale=2");
    }

    #[test]
    fn existing_lines_change_in_place() {
        let text = "\u{FEFF}[Emulator]\r\nScale = 2\r\ncycles=max\r\n[sound]\r\nsoundfont=gm.sf2\r\n\r\n[drives]\r\nc: = /old\r\n# keep\r\nD=/cd cdrom\r\nd=/cd2\r\n[autoexec]\r\nC:\r\n";
        let sound = SoundConfig { soundfont: Some("gm.sf2".into()), ..SoundConfig::default() };
        let baseline = Settings { scale: 2, sound, ..Settings::default() };
        let settings = Settings { scale: 4, ..Settings::default() };
        let spec = |drive, path: &str| Some(MountSpec { drive, path: path.into(), opts: Default::default() });
        let changes = [(2, spec(2, "/new")), (3, None), (4, spec(4, "/e"))];
        assert_eq!(
            update_text(text, &baseline, &settings, &changes, None),
            // A cleared setting is commented out; a removed drive loses all
            // its lines; a new one goes after the other drives.
            "\u{FEFF}[Emulator]\r\nScale = 4\r\ncycles=max\r\n[sound]\r\n#soundfont=gm.sf2\r\n\r\n[drives]\r\nc: = /new\r\nE=/e\r\n# keep\r\n[autoexec]\r\nC:\r\n"
        );
    }

    #[test]
    fn the_autoexec_section_is_edited_as_written() {
        let text = "\u{FEFF}[emulator]\r\nscale=2\r\n[AUTOEXEC]\r\n# mine\r\nMOUNT D ~/d\r\n\r\nD:\r\n\r\n[sound]\r\nopl=opl3\r\n[autoexec]\r\nDIR\r\n";
        assert_eq!(autoexec_lines(text), ["# mine", "MOUNT D ~/d", "", "D:", "DIR"]);

        // In the first section, the header as written, the later one gone.
        let lines = ["# mine".to_string(), "C:".to_string(), String::new()];
        assert_eq!(
            with_autoexec(text, &lines),
            "\u{FEFF}[emulator]\r\nscale=2\r\n[AUTOEXEC]\r\n# mine\r\nC:\r\n\r\n[sound]\r\nopl=opl3\r\n"
        );
        // Emptied, it stays; without one, it goes at the end.
        assert_eq!(with_autoexec("[autoexec]\nDIR\n", &[]), "[autoexec]\n");
        assert_eq!(with_autoexec("[emulator]\nscale=2\n", &["DIR".to_string()]), "[emulator]\nscale=2\n\n[autoexec]\nDIR\n");
        assert_eq!(with_autoexec("", &["DIR".to_string()]), "[autoexec]\nDIR\n");

        // The template's commented examples are there to edit.
        assert_eq!(autoexec_lines(TEMPLATE).last().map(String::as_str), Some("#CD GAMES"));
    }

    #[test]
    fn missing_sections_go_before_autoexec() {
        let settings = Settings { memsize: 8, ..Settings::default() };
        let base = Settings::default();
        let changes = [(3, Some(MountSpec { drive: 3, path: "/d".into(), opts: Default::default() }))];
        assert_eq!(
            update_text("[autoexec]\nDIR\n", &base, &settings, &changes, None),
            "[emulator]\nmemsize=8\n\n[drives]\nD=/d\n\n[autoexec]\nDIR\n"
        );
        assert_eq!(update_text("# mine\n", &base, &settings, &[], None), "# mine\n\n[emulator]\nmemsize=8\n");
        // A second [emulator] block gets the new line.
        assert_eq!(
            update_text("[emulator]\nscale=2\n[sound]\n[emulator]\ncpu=386\n", &base, &settings, &[], None),
            "[emulator]\nscale=2\n[sound]\n[emulator]\ncpu=386\nmemsize=8\n"
        );
    }

    #[test]
    fn save_replaces_the_file() {
        let base = scratch("save");
        let path = base.join(FILE_NAME);
        fs::write(&path, "# my settings\n[emulator]\nscale=2\n").unwrap();
        let before = Settings { scale: 2, ..Settings::default() };
        let settings = Settings { scale: 3, ..Settings::default() };
        save(&path, &before, &settings, &[], None, Saving::Changes).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "# my settings\n[emulator]\nscale=3\n");
        assert_eq!(fs::read_dir(&base).unwrap().count(), 1);

        // A file that has gone missing starts over from the template.
        fs::remove_file(&path).unwrap();
        save(&path, &before, &settings, &[], None, Saving::Changes).unwrap();
        assert!(fs::read_to_string(&path).unwrap().contains("#scale=2\nscale=3\n"));

        // All of them: the file gets every setting, and parses back to them.
        fs::write(&path, "# my settings\n[emulator]\nscale=2\n").unwrap();
        save(&path, &before, &settings, &[], None, Saving::All).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# my settings\n[emulator]\nscale=3\nfullscreen=false\n"), "{}", text);
        assert!(text.contains("\n[mixer]\nmaster=100\n"), "{}", text);
        assert_eq!(Settings::from_config(&parse(&text, &base, None)), settings);
    }

    #[test]
    fn saving_all_fills_in_every_setting() {
        let home = Path::new("/home/u");
        let text = "[emulator]\nscale = 2\ncycles=max\n[sound]\nsoundfont=sf/gm.sf2\n\n[autoexec]\nDIR\n";
        let file = Settings::from_config(&parse(text, Path::new("/cfg"), Some(home)));
        assert_eq!(file.sound.soundfont.as_deref(), Some(Path::new("/cfg/sf/gm.sf2")));
        // The command line asked for more speed, and the window for a
        // shader and no Gravis Ultrasound.
        let baseline = Settings { cycles: CpuSpeed::Fixed(20000), ..file.clone() };
        let mut settings = Settings { shader: Shader::Aperture, ..baseline.clone() };
        settings.sound.gus.enabled = false;
        let saved = complete_text(text, Path::new("/cfg"), &baseline, &settings, &[], Some(home));

        // Every setting has a line, the lines that were there stay as they
        // were written, and the command line's speed isn't kept.
        for (section, key, _) in entries(&settings, Some(home)) {
            // (The file's SoundFont; no Ultrasound directory, MT-32, LAN
            // password, RetroAchievements account or printer paths and
            // programs of its own.)
            let wanted = !matches!(
                key,
                "ultradir" | "awe32rom" | "mt32roms" | "mt32lib" | "midiport" | "password" | "username" | "token"
                    | "docpath" | "fontpath" | "device" | "print_command" | "open_with"
            );
            assert_eq!(has_key(&saved.lines().map(str::to_string).collect::<Vec<_>>(), section, key), wanted, "{}\n{}", key, saved);
        }
        assert!(saved.starts_with("[emulator]\nscale = 2\ncycles=max\nfullscreen=false\n"), "{}", saved);
        assert!(saved.contains("shader=aperture\n") && saved.contains("gus=false\n"), "{}", saved);
        assert!(saved.contains("soundfont=sf/gm.sf2\n"), "{}", saved);
        assert!(saved.contains("\n[joystick]\njoysticktype=auto\ndeadzone=10\n\n[network]\nipx=auto\n"), "{}", saved);
        assert!(saved.contains("\nroom=lobby\n\n[serial]\nserial1=mouse\n"), "{}", saved);
        assert!(saved.contains("\nmodemtelnet=off\n\n[printer]\noutput=pdf\n"), "{}", saved);
        assert!(saved.contains("\ntimeout=3000\n\n[achievements]\nenabled=false\nhardcore=false\n\n[autoexec]\nDIR\n"), "{}", saved);
        let config = parse(&saved, Path::new("/cfg"), Some(home));
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
        assert_eq!(Settings::from_config(&config), Settings { cycles: CpuSpeed::Max, ..settings.clone() });

        // Saving again changes nothing; a speed the window changed is saved.
        assert_eq!(complete_text(&saved, Path::new("/cfg"), &settings, &settings, &[], Some(home)), saved);
        let faster = Settings { cycles: CpuSpeed::Fixed(30000), ..settings.clone() };
        let again = complete_text(&saved, Path::new("/cfg"), &settings, &faster, &[], Some(home));
        assert!(again.starts_with("[emulator]\nscale = 2\ncycles=30000\n"), "{}", again);
    }

    #[test]
    fn load_writes_template_once_and_reads_local_files() {
        let base = scratch("load");
        let cwd = base.join("work");
        fs::create_dir_all(&cwd).unwrap();
        let default = base.join("cfg/rust-dos/rust-dos.conf");

        let config = load(None, &cwd, None, Some(default.clone()), None).unwrap();
        assert!(config.created);
        assert_eq!(config.source.as_deref(), Some(default.as_path()));
        assert_eq!(fs::read_to_string(&default).unwrap(), TEMPLATE);

        // Second start uses the (edited) file and doesn't rewrite it
        fs::write(&default, "[emulator]\nscale=2\n").unwrap();
        let config = load(None, &cwd, None, Some(default.clone()), None).unwrap();
        assert!(!config.created);
        assert_eq!(config.scale, Some(2));

        // A file in the working directory wins, with paths relative to it
        fs::write(cwd.join(FILE_NAME), "[drives]\nA=disks/a floppy\nscale=9\n").unwrap();
        let config = load(None, &cwd, None, Some(default.clone()), None).unwrap();
        assert_eq!(config.scale, None);
        assert_eq!(config.drive(0).unwrap().path, cwd.join("disks/a"));
        assert_eq!(config.warnings.len(), 1);
        assert!(config.warnings[0].contains("rust-dos.conf: line 3"));

        assert!(load(Some(Path::new("nope.conf")), &cwd, None, Some(default), None).is_err());
    }

    #[test]
    fn unwritable_default_is_only_a_warning() {
        let base = scratch("unwritable");
        let blocker = base.join("file");
        fs::write(&blocker, "").unwrap();
        // The parent "directory" is a file, so creating it fails
        let config = load(None, &base, None, Some(blocker.join("rust-dos.conf")), None).unwrap();
        assert!(!config.created);
        assert_eq!(config.warnings.len(), 1);
        assert!(config.drives.is_empty());
    }
}
