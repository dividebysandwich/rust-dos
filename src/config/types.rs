//! The value types of settings: aspect ratio, filter, DOS version, MIDI synthesizer and the `[sound]` section.

use crate::disk::DRIVE_Z;
use crate::lpt_dac::LptDacType;
use crate::mount::{expand_host_path, parse_drive_letter};
use std::path::{Path, PathBuf};

/// The target display ratio of the picture (`aspect`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AspectRatio {
    /// Keep the frame's native pixel ratio.
    #[default]
    None,
    FourThree,
    FiveFour,
    SixteenTen,
    SixteenNine,
}

impl AspectRatio {
    pub const ALL: [Self; 5] = [Self::None, Self::FourThree, Self::FiveFour, Self::SixteenTen, Self::SixteenNine];

    /// Parse a ratio name, including the former boolean values for old config files.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" => Some(Self::None),
            "4:3" => Some(Self::FourThree),
            "5:4" => Some(Self::FiveFour),
            "16:10" => Some(Self::SixteenTen),
            "16:9" => Some(Self::SixteenNine),
            other => parse_bool(other).map(|on| if on { Self::FourThree } else { Self::None }),
        }
    }

    /// The canonical configuration name.
    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::FourThree => "4:3",
            Self::FiveFour => "5:4",
            Self::SixteenTen => "16:10",
            Self::SixteenNine => "16:9",
        }
    }

    /// The target width-to-height dimensions, or none for the frame's native ratio.
    pub fn dimensions(self) -> Option<(u32, u32)> {
        match self {
            Self::None => None,
            Self::FourThree => Some((4, 3)),
            Self::FiveFour => Some((5, 4)),
            Self::SixteenTen => Some((16, 10)),
            Self::SixteenNine => Some((16, 9)),
        }
    }

    /// The target width-to-height ratio, or `native` for None.
    pub fn ratio(self, native: f32) -> f32 {
        self.dimensions().map_or(native, |(w, h)| w as f32 / h as f32)
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
/// MS-DOS 7 (AX=7302h to 7305h) need while no FAT32 drive is mounted.
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

/// The version of Windows the built-in Windows is (`win32_version`):
/// what GetVersion says, the registry's version values, and which
/// functions the built-in DLLs export.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WindowsVersion {
    #[default]
    Win95,
    Win98,
    Win98Se,
}

impl WindowsVersion {
    /// "95", "98" or "98se".
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "95" => Some(Self::Win95),
            "98" => Some(Self::Win98),
            "98se" => Some(Self::Win98Se),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Win95 => "95",
            Self::Win98 => "98",
            Self::Win98Se => "98se",
        }
    }

    /// Windows 98 or its Second Edition.
    pub fn is_98(self) -> bool {
        self >= Self::Win98
    }
}

/// The depth of the built-in Windows' desktop (`win32_depth`): the
/// display adapter's own (32 bits a pixel, 24 on an S3 ViRGE, 16 on a
/// Banshee or Voodoo3), 8 bits a pixel, 256 colours from a palette, or
/// 16 bits a pixel, High Color.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum DesktopDepth {
    #[default]
    Adapter,
    Palette,
    HighColor,
}

impl DesktopDepth {
    /// "auto", "8" or "16".
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Adapter),
            "8" => Some(Self::Palette),
            "16" => Some(Self::HighColor),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Adapter => "auto",
            Self::Palette => "8",
            Self::HighColor => "16",
        }
    }
}

/// Which calls of the built-in Windows' functions the log gets a line
/// for (`win32_trace`): every call; the first few of each call site's
/// calls a second and a count of the rest (src/win32/trace.rs); or none.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Win32Trace {
    All,
    #[default]
    Collapsed,
    Off,
}

impl Win32Trace {
    /// "all", "collapsed" or "off".
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "all" => Some(Self::All),
            "collapsed" => Some(Self::Collapsed),
            "off" => Some(Self::Off),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Collapsed => "collapsed",
            Self::Off => "off",
        }
    }
}

/// A yes/no setting: true, on, yes or 1, or false, off, no or 0.
pub(super) fn parse_bool(value: &str) -> Option<bool> {
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
    /// The Roland Sound Canvas SC-55 family, played by the port of
    /// Nuked-SC55 (`sc55`).
    Sc55,
    /// A MIDI port of the host (`midiport`).
    Host,
    None,
}

pub use rust_dos_mt32::Mt32Model;

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
    /// The directory with the Sound Canvas's ROMs (`sc55roms`); without it,
    /// the usual places (`sc55::rom::default_dirs`).
    pub sc55roms: Option<PathBuf>,
    /// Which Sound Canvas (`sc55model`): `auto`, a family such as `mk2`
    /// or a version such as `mk1-v1.21` (`sc55::rom`).
    pub sc55model: String,
    /// The host's MIDI port for `midisynth=host`: a part of its name or its
    /// number; empty for the first.
    pub midiport: String,
    /// The Covox or Disney Sound Source on LPT1 (`lpt_dac`).
    pub lpt_dac: LptDacType,
    /// The Tandy's and PCjr's sound chip (`tandy`).
    pub tandy: crate::sn76489::TandySound,
    /// Whether PIT channel 2 starts with its gate (port 61h bit 0) high and
    /// its count running (`pit2_gate_at_start`).
    pub pit2_gate_at_start: bool,
    /// Whether the Sound Blaster drivers of games that play badly on every
    /// card are patched when rust-dos recognises them (`sb_game_fix`).
    pub sb_game_fix: bool,
}

impl MidiSynth {
    pub fn name(self) -> &'static str {
        match self {
            MidiSynth::Auto => "auto",
            MidiSynth::SoundFont => "soundfont",
            MidiSynth::Gus => "gus",
            MidiSynth::Mt32 => "mt32",
            MidiSynth::Sc55 => "sc55",
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
            sc55roms: None,
            sc55model: "auto".to_string(),
            midiport: String::new(),
            lpt_dac: LptDacType::None,
            tandy: crate::sn76489::TandySound::Auto,
            pit2_gate_at_start: false,
            sb_game_fix: true,
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

    pub(super) fn set(&mut self, key: &str, value: &str, base_dir: &Path, home: Option<&Path>) -> Result<(), String> {
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
                        .ok_or_else(|| format!("invalid sbtype '{}' (sb16, sb16vibra, awe32, sbpro2, sb2 or none)", value))?;
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
            "sc55roms" => self.sc55roms = Some(expand_host_path(value, base_dir, home)),
            "sc55model" => {
                if !crate::sc55::rom::valid_model(value) {
                    return Err(format!(
                        "invalid sc55model '{}' (auto, a model such as mk1, mk2 or cm300, or a version such as mk1-v1.21)",
                        value
                    ));
                }
                self.sc55model = value.to_ascii_lowercase();
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
                    "sc55" | "sc-55" | "soundcanvas" => MidiSynth::Sc55,
                    "host" | "hostmidi" | "external" => MidiSynth::Host,
                    "none" => MidiSynth::None,
                    _ => {
                        return Err(format!(
                            "invalid midisynth '{}' (auto, soundfont, gus, mt32, sc55, host or none)",
                            value
                        ));
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
            "pit2_gate_at_start" => {
                self.pit2_gate_at_start = parse_bool(value)
                    .ok_or_else(|| format!("invalid pit2_gate_at_start '{}' (true or false)", value))?;
            }
            "sb_game_fix" => {
                self.sb_game_fix =
                    parse_bool(value).ok_or_else(|| format!("invalid sb_game_fix '{}' (on or off)", value))?;
            }
            _ => return Err(format!("unknown setting '{}'", key)),
        }
        Ok(())
    }
}
