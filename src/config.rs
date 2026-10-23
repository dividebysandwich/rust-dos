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
use crate::diskio::DiskSettings;
use crate::joystick::JoystickSettings;
use crate::keylayout::LayoutSetting;
use crate::mount::MountSpec;
use crate::mixer::MixerSettings;
use crate::timer::CpuSpeed;
use crate::video::adapter::Adapter;
use crate::video::composite::{CompositeEra, CompositeMode};
use crate::video::mono::Monochrome;
use crate::video::shader::Shader;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub use rust_dos_hostdirs::{FILE_NAME, exe_dir, user_dir};
use rust_dos_hostdirs::profile_dir;
#[cfg(test)]
use rust_dos_hostdirs::user_dir_for;

mod edit;
mod parse;
mod settings;
mod types;

pub use edit::{
    DriveChange, Saving, autoexec_lines, complete_text, load_autoexec, save, save_autoexec, update_text, with_autoexec,
};
pub use parse::{MAX_MEMSIZE, MIN_MEMSIZE, parse, parse_memsize};
pub use settings::{Settings, parse_mouse_sensitivity};
pub use types::{AspectRatio, DosVersion, Filter, MidiSynth, Mt32Model, SoundConfig};

/// Written to the default location on first start.
pub const TEMPLATE: &str = include_str!("../rust-dos.conf.example");

/// Per-user default: `<config dir>/rust-dos/rust-dos.conf`, e.g.
/// `~/.config/rust-dos/rust-dos.conf` on Linux.
pub fn default_path() -> Option<PathBuf> {
    profile_dir().map(|d| d.join(FILE_NAME))
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
    /// The picture's display ratio (`aspect`).
    pub aspect: Option<AspectRatio>,
    /// Show each of the machine's frames when it is due, for a display
    /// with a variable refresh rate (`vrr`).
    pub vrr: Option<bool>,
    /// How the picture is scaled to the window (`filter`).
    pub filter: Option<Filter>,
    /// The CRT look (`shader`), how far its tube bends (`crt_curvature`)
    /// and how much it glows (`crt_glow`).
    pub shader: Option<Shader>,
    pub crt_curvature: Option<u16>,
    pub crt_glow: Option<u16>,
    /// A monochrome monitor's phosphor (`monochrome`).
    pub monochrome: Option<Monochrome>,
    /// The monitor's power-on, resync and power-off effects
    /// (`monitor_effects`).
    pub monitor_effects: Option<crate::video::power::MonitorEffects>,
    /// The BIOS's power-on self test before DOS (`post`).
    pub post: Option<crate::post::PostMode>,
    /// The CGA's composite monitor (`composite`, `composite_era`).
    pub composite: Option<CompositeMode>,
    pub composite_era: Option<CompositeEra>,
    /// The display adapter (`machine`).
    pub machine: Option<Adapter>,
    /// The 3dfx card (`voodoo`), its chip (`voodoo_model`), its memory
    /// (`voodoo_memory`), what draws
    /// for it (`voodoo_renderer`), at what size (`voodoo_scale`) and with
    /// how many samples a pixel (`voodoo_msaa`), texture anisotropy
    /// (`voodoo_anisotropy`), and the most frames a second programs may show
    /// on it (`voodoo_fps_cap`, 0 for no cap).
    pub voodoo: Option<bool>,
    pub voodoo_model: Option<crate::voodoo::Model>,
    pub voodoo_memory: Option<crate::voodoo::Board>,
    pub voodoo_renderer: Option<crate::voodoo::Renderer>,
    pub voodoo_texture_sampling: Option<crate::voodoo::TextureSampling>,
    pub voodoo_scale: Option<u32>,
    /// Whether the CRT look's lines follow `voodoo_scale`
    /// (`voodoo_scale_shader`).
    pub voodoo_scale_shader: Option<bool>,
    /// Whether OpenGL dithers as the card does (`voodoo_dither`).
    pub voodoo_dither: Option<bool>,
    pub voodoo_msaa: Option<u32>,
    pub voodoo_anisotropy: Option<u32>,
    pub voodoo_fps_cap: Option<u32>,
    /// The gamma Glide applies to the guest, or off.
    pub voodoo_gamma: Option<Option<f32>>,
    /// The PowerVR card (`powervr`): its chip, or None for no card; and
    /// its texture filtering (`powervr_filter`).
    pub powervr: Option<Option<crate::powervr::Chip>>,
    pub powervr_filter: Option<crate::powervr::tsp::Filter>,
    /// The Vérité's RISC (`verite_risc`), its speed (`verite_mhz`), the
    /// dump of a board's ROM it sees (`verite_rom`) and its interrupt line
    /// (`verite_irq`).
    pub verite_risc: Option<crate::verite::Risc>,
    pub verite_mhz: Option<u32>,
    pub verite_rom: Option<PathBuf>,
    pub verite_irq: Option<u8>,
    /// Where screenshots and recordings go (`capture_dir`), and whether
    /// they show the settings window and the performance overlay
    /// (`record_ui`) and the CRT shader (`record_shader`).
    pub capture_dir: Option<PathBuf>,
    pub record_ui: Option<bool>,
    pub record_shader: Option<bool>,
    /// Emulated CPU speed (`cycles`).
    pub cycles: Option<CpuSpeed>,
    /// The frame rate `cycles=auto` aims for (`auto_fps`), or None for the
    /// display's refresh rate.
    pub auto_fps: Option<Option<u32>>,
    /// Whether `cycles=auto` gives a program that keeps an idling system
    /// (Windows) busy all the host has (`auto_busy_os`).
    pub auto_busy_os: Option<bool>,
    /// Emulated processor (`cpu`).
    pub cpu: Option<CpuModel>,
    /// What runs the programs' instructions (`core`).
    pub core: Option<CoreMode>,
    /// The FPU's adds and subtracts on doubles (`fpu=fast`), not exact.
    pub fpu_fast: Option<bool>,
    /// RAM in MB (`memsize`).
    pub memsize: Option<usize>,
    /// Expanded memory (`ems`).
    pub ems: Option<bool>,
    /// Upper memory blocks (`umb`).
    pub umb: Option<bool>,
    /// The DPMI host (`dpmi`).
    pub dpmi: Option<bool>,
    /// The built-in Windows (`win32`).
    pub win32: Option<bool>,
    /// A host folder of Win32 DLLs programs may load as guest code
    /// (`win32_dlls`), and the built-in DLLs they replace
    /// (`win32_dlls_replace`).
    pub win32_dlls: Option<PathBuf>,
    pub win32_dlls_replace: Option<Vec<String>>,
    /// DOS's tables packed low, as DOS=HIGH has them (`dos_high`).
    pub dos_high: Option<bool>,
    /// The DOS version programs are told (`dos_version`).
    pub dos_version: Option<DosVersion>,
    /// A booted system's hard disks on the IDE channels (`ide_hard_disks`).
    pub ide_hard_disks: Option<bool>,
    /// A booted system has a CD-ROM drive with no disc in it (`boot_cdrom`).
    pub boot_cdrom: Option<bool>,
    /// A booted system's floppy disk controller (`floppy_controller`).
    pub floppy_controller: Option<bool>,
    /// The keyboard layout (`keyboard_layout`).
    pub keyboard_layout: Option<LayoutSetting>,
    /// The mouse captured by itself (`mouse_autocapture`).
    pub mouse_autocapture: Option<bool>,
    /// The captured mouse's motion multiplier (`mouse_sensitivity`).
    pub mouse_sensitivity: Option<f64>,
    /// The messages that say the mouse was captured or let go
    /// (`mouse_capture_messages`).
    pub mouse_capture_messages: Option<bool>,
    /// The right button held still opening the settings window
    /// (`mouse_menu_hold`).
    pub mouse_menu_hold: Option<bool>,
    /// The machine pausing while the settings window is open
    /// (`pause_in_settings`).
    pub pause_in_settings: Option<bool>,
    /// The check for a newer rust-dos at startup (`version_check`).
    pub version_check: Option<bool>,
    /// Rewind (`rewind`), and the memory its states may take in MB
    /// (`rewind_memory`).
    pub rewind: Option<bool>,
    pub rewind_memory: Option<usize>,
    /// Busy-wait loops skipped (`idle_skip`), and Windows' idle calls
    /// waiting for the next event (`idle_hint`).
    pub idle_skip: Option<bool>,
    pub idle_hint: Option<bool>,
    /// `[sound]`: the Sound Blaster (None: `sbtype=none`), the FM chip,
    /// the Gravis Ultrasound, and the MPU-401's synthesizer.
    pub sound: SoundConfig,
    /// How fast the disks are (`[emulator]`) and the noises they make
    /// (`[sound]`).
    pub disk: DiskSettings,
    /// `[mixer]`: the volumes of the sound sources.
    pub mixer: MixerSettings,
    /// `[mixer]`'s `blocksize` and `prebuffer`: the sound waiting for the
    /// host's sound device.
    pub audio_buffer: crate::audio::AudioBuffer,
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
    /// `[shell]`: the prompt's suggestions, colours and history.
    pub shell: crate::cmdline::settings::ShellSettings,
    /// `[vr]`: the picture in a 3D scene.
    pub vr: crate::vr::VrSettings,
    /// `[vfx1]`: the Forte VFX1 headset.
    pub vfx1: crate::vfx1::Vfx1Settings,
    /// `[drives]` entries in file order, at most one per drive.
    pub drives: Vec<MountSpec>,
    /// `[autoexec]` command lines in file order.
    pub autoexec: Vec<String>,
    /// A game profile's name (`[game]`, see games.rs).
    pub game_name: Option<String>,
    /// The version of the game RetroAchievements knows (`[game]`'s
    /// `achievements`): its hash, or its zip or DOSZ archive.
    pub game_achievements: Option<String>,
    /// Whether a game profile's drives keep its changes apart, in a folder
    /// of its own (`[game]`'s `overlay`, see games.rs).
    pub game_overlay: bool,
    /// A game profile's manuals and extras, as `[game]`'s `manual=` lines
    /// have them (`manuals::Manual::parse`).
    pub game_manuals: Vec<String>,
    /// What a profile made from a package was made from, fingerprinted
    /// (`[game]`'s `source`, see `games::add_package`).
    pub game_source: Option<String>,
    /// The keys a game's profile presses as it starts (`[game]`'s `input`,
    /// autoinput.rs).
    pub game_input: Option<String>,
    /// Whether the game's package has launch configurations to choose
    /// from as it starts (`[game]`'s `variants`).
    pub game_variants: bool,
    /// The operating system a game's package runs in (`[game]`'s `os`,
    /// see `games::add_package_in`).
    pub game_os: Option<String>,
    /// A game profile's `[gamepad]` lines, as written (padmap.rs reads
    /// them).
    pub game_pad: Vec<(String, String)>,
    /// Which Windows a game profile runs in (`[game]`'s `windows`, see
    /// games.rs): `builtin` or `image`.
    pub game_windows: Option<String>,
    /// A game profile's `[install]` lines, keys lowercased (install.rs
    /// reads them).
    pub game_install: Vec<(String, String)>,
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
    /// The modem's numbers of bulletin boards (serial/phonebook.rs).
    Phonebook,
    Printer,
    Achievements,
    Shell,
    Vr,
    Vfx1,
    Drives,
    Autoexec,
    /// A game profile's (games.rs).
    Game,
    /// A game profile's gamepad mapping (padmap.rs).
    Gamepad,
    /// A game profile's installer archives (install.rs).
    Install,
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
            "phonebook" => Section::Phonebook,
            "printer" => Section::Printer,
            "achievements" => Section::Achievements,
            "shell" => Section::Shell,
            "vr" => Section::Vr,
            "vfx1" => Section::Vfx1,
            "drives" => Section::Drives,
            "autoexec" => Section::Autoexec,
            "game" => Section::Game,
            "gamepad" => Section::Gamepad,
            "install" => Section::Install,
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
            Section::Phonebook => "phonebook",
            Section::Printer => "printer",
            Section::Achievements => "achievements",
            Section::Shell => "shell",
            Section::Vr => "vr",
            Section::Vfx1 => "vfx1",
            Section::Drives => "drives",
            Section::Autoexec => "autoexec",
            Section::Game => "game",
            Section::Gamepad => "gamepad",
            Section::Install => "install",
            Section::None | Section::Unknown => "",
        }
    }
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

#[cfg(test)]
mod tests;
