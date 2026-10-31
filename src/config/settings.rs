//! The settings in effect, and every setting as the file writes it.

use super::{
    AspectRatio, Config, DesktopDepth, DosVersion, Filter, Section, SoundConfig, Win32Trace,
    WindowsVersion,
};
use crate::cpu::{CoreMode, CpuModel};
use crate::diskio::DiskSettings;
use crate::joystick::JoystickSettings;
use crate::keylayout::LayoutSetting;
use crate::mixer::{Channel, MixerSettings};
use crate::mount::contract_home;
use crate::timer::CpuSpeed;
use crate::video::adapter::{Adapter, VideoSetup};
use crate::video::composite::CompositeSettings;
use crate::video::mono::Monochrome;
use crate::video::shader::{CrtSettings, Shader};
use std::path::{Path, PathBuf};

/// The settings in effect: the configuration with every value that is not
/// set filled in with its default.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub scale: u32,
    pub fullscreen: bool,
    pub aspect: AspectRatio,
    pub vrr: bool,
    pub filter: Filter,
    pub shader: Shader,
    /// The CRT look's own settings.
    pub crt: CrtSettings,
    pub monochrome: Monochrome,
    pub monitor_effects: crate::video::power::MonitorEffects,
    pub post: crate::post::PostMode,
    /// The CGA's composite monitor.
    pub composite: CompositeSettings,
    pub machine: Adapter,
    /// The 3dfx card.
    pub voodoo: crate::voodoo::VoodooSettings,
    /// The PowerVR card, if there is one, and how it filters textures.
    pub powervr: Option<crate::powervr::Chip>,
    pub powervr_filter: crate::powervr::tsp::Filter,
    /// The Rendition Vérité's RISC, its speed, board ROM and interrupt.
    pub verite: crate::verite::VeriteSettings,
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
    /// The frame rate `cycles=auto` aims for, or None for the display's
    /// refresh rate.
    pub auto_fps: Option<u32>,
    /// `cycles=auto` gives a program that keeps an idling system busy all
    /// the host has (`autospeed`).
    pub auto_busy_os: bool,
    pub cpu: CpuModel,
    pub core: CoreMode,
    /// The FPU adds and subtracts as the host does (`fpu=fast`): faster,
    /// not exact (`Cpu::fpu_fast`).
    pub fpu_fast: bool,
    /// RAM in MB.
    pub memsize: usize,
    /// Expanded memory (EMS).
    pub ems: bool,
    /// Upper memory blocks.
    pub umb: bool,
    /// The DPMI host for DOS extenders.
    pub dpmi: bool,
    /// The built-in Windows runs Win32 console programs (`win32`), the
    /// start of the work in docs/native-windows.md.
    pub win32: bool,
    /// A host folder of Win32 DLLs (`win32_dlls`) that programs may load
    /// as guest code when no built-in DLL has the name, or when the
    /// built-in one is named in `win32_dlls_replace` (lowercase, without
    /// ".dll").
    pub win32_dlls: Option<PathBuf>,
    pub win32_dlls_replace: Vec<String>,
    /// The version of Windows the built-in Windows is.
    pub win32_version: WindowsVersion,
    /// The depth of the built-in Windows' desktop.
    pub win32_depth: DesktopDepth,
    /// Which calls of its functions the log gets a line for.
    pub win32_trace: Win32Trace,
    /// DOS's tables packed below the first MCB, as DOS=HIGH leaves
    /// conventional memory, rather than spread out up to 64 KB.
    pub dos_high: bool,
    /// The DOS version programs are told.
    pub dos_version: DosVersion,
    /// A booted system's hard disks are ATA disks on the IDE channels too.
    pub ide_hard_disks: bool,
    /// A booted system has a CD-ROM drive even with no CD mounted.
    pub boot_cdrom: bool,
    /// A booted system has a floppy disk controller for its floppy drives.
    pub floppy_controller: bool,
    pub keyboard_layout: LayoutSetting,
    /// The mouse captured as it moves over the window while a program
    /// uses the mouse driver, and let go as the program's cursor leaves
    /// the screen.
    pub mouse_autocapture: bool,
    /// Relative mouse motion multiplied by 0.1 to 10.0 (default 1.0).
    pub mouse_sensitivity: f64,
    /// Say over the picture when the mouse is captured or let go.
    pub mouse_capture_messages: bool,
    /// The right mouse button held still for one and a half seconds opens the
    /// settings window.
    pub mouse_menu_hold: bool,
    /// The machine waits while the settings window is open (but for the
    /// Mixer and Stats pages); never while in a LAN room.
    pub pause_in_settings: bool,
    /// Ask rust-dos.com at startup whether there is a newer version.
    pub version_check: bool,
    /// Ask almanac.rust-dos.com for a program's settings as it starts from
    /// the prompt, and play it with them (almanac.rs).
    pub almanac_auto: bool,
    /// Rewind with held Alt+F11, and the memory in MB its states may take.
    pub rewind: bool,
    pub rewind_memory: usize,
    /// Skip the passes of busy-wait loops that change nothing (`idle`).
    pub idle_skip: bool,
    /// Let Windows' idle calls wait for the next event (not exact).
    pub idle_hint: bool,
    /// Apply the fixes for particular games (`game_fixes`), but for the
    /// ones named in `game_fixes_off`.
    pub game_fixes: bool,
    pub game_fixes_off: Vec<String>,
    pub sound: SoundConfig,
    pub disk: DiskSettings,
    pub mixer: MixerSettings,
    /// The sound waiting for the host's sound device (`blocksize` and
    /// `prebuffer`).
    pub audio_buffer: crate::audio::AudioBuffer,
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
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            scale: 1,
            fullscreen: false,
            aspect: AspectRatio::None,
            vrr: false,
            filter: Filter::Nearest,
            shader: Shader::None,
            crt: CrtSettings::default(),
            monochrome: Monochrome::Off,
            monitor_effects: crate::video::power::MonitorEffects::Off,
            post: crate::post::PostMode::Off,
            composite: CompositeSettings::default(),
            machine: Adapter::Svga,
            voodoo: Default::default(),
            powervr: None,
            powervr_filter: Default::default(),
            verite: Default::default(),
            capture_dir: PathBuf::from("capture"),
            record_ui: false,
            record_shader: false,
            cycles: CpuSpeed::default(),
            auto_fps: None,
            auto_busy_os: true,
            cpu: CpuModel::I486,
            core: CoreMode::Auto,
            fpu_fast: false,
            memsize: crate::bus::DEFAULT_MEMORY_MB,
            ems: true,
            umb: true,
            dpmi: true,
            win32: false,
            win32_dlls: None,
            win32_dlls_replace: Vec::new(),
            win32_version: WindowsVersion::Win95,
            win32_depth: DesktopDepth::Adapter,
            win32_trace: Win32Trace::Collapsed,
            dos_high: true,
            dos_version: DosVersion::default(),
            ide_hard_disks: true,
            boot_cdrom: true,
            floppy_controller: true,
            keyboard_layout: LayoutSetting::Auto,
            mouse_autocapture: true,
            mouse_sensitivity: 1.0,
            mouse_capture_messages: true,
            mouse_menu_hold: true,
            pause_in_settings: true,
            version_check: true,
            almanac_auto: false,
            rewind: false,
            rewind_memory: 256,
            idle_skip: true,
            idle_hint: false,
            game_fixes: true,
            game_fixes_off: Vec::new(),
            sound: SoundConfig::default(),
            disk: DiskSettings::default(),
            mixer: MixerSettings::default(),
            audio_buffer: crate::audio::AudioBuffer::default(),
            joystick: JoystickSettings::default(),
            network: crate::net::NetSettings::default(),
            serial: crate::serial::SerialSettings::default(),
            printer: crate::printer::PrinterSettings::default(),
            achievements: Default::default(),
            shell: Default::default(),
            vr: Default::default(),
            vfx1: Default::default(),
        }
    }
}

/// Parse the host mouse motion multiplier, rejecting non-finite values too.
pub fn parse_mouse_sensitivity(value: &str) -> Result<f64, String> {
    value.trim().parse::<f64>().ok()
        .filter(|n| (0.1..=10.0).contains(n))
        .ok_or_else(|| format!("invalid mouse_sensitivity '{}' (0.1 to 10.0)", value))
}

impl Settings {

    /// Scale relative host mouse motion, leaving absolute positioning alone.
    pub fn mouse_motion(&self, dx: f64, dy: f64) -> (f64, f64) {
        (dx * self.mouse_sensitivity, dy * self.mouse_sensitivity)
    }

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

    /// The 3dfx card the machine has: the display adapter itself with
    /// `machine=svga_voodoo3` or `svga_banshee`, which leaves no room for
    /// the add-on card `voodoo` asks for; otherwise that one, if any.
    pub fn voodoo_board(&self) -> Option<crate::voodoo::Board> {
        self.machine.voodoo_board().or(self.voodoo.board())
    }

    /// The guest environment variables every setting asks for. A name two
    /// settings give takes the value of the later one.
    pub fn environment(&self) -> Vec<crate::env_inject::Rule> {
        let mut rules = Vec::new();
        rules.extend(self.voodoo.environment());
        rules.extend(self.vfx1.environment());
        rules
    }

    pub fn from_config(config: &Config) -> Self {
        let default = Self::default();
        Self {
            scale: config.scale.unwrap_or(default.scale),
            fullscreen: config.fullscreen.unwrap_or(default.fullscreen),
            aspect: config.aspect.unwrap_or(default.aspect),
            vrr: config.vrr.unwrap_or(default.vrr),
            filter: config.filter.unwrap_or(default.filter),
            shader: config.shader.unwrap_or(default.shader),
            crt: CrtSettings {
                curvature: config.crt_curvature.unwrap_or(default.crt.curvature),
                glow: config.crt_glow.unwrap_or(default.crt.glow),
            },
            monochrome: config.monochrome.unwrap_or(default.monochrome),
            monitor_effects: config.monitor_effects.unwrap_or(default.monitor_effects),
            post: config.post.unwrap_or(default.post),
            composite: CompositeSettings {
                mode: config.composite.unwrap_or(default.composite.mode),
                era: config.composite_era.unwrap_or(default.composite.era),
            },
            machine: config.machine.unwrap_or(default.machine),
            voodoo: crate::voodoo::VoodooSettings {
                enabled: config.voodoo.unwrap_or(default.voodoo.enabled),
                board: {
                    // The memory asked for on the chip asked for, or the
                    // one the memory comes with.
                    let memory = config.voodoo_memory.unwrap_or(default.voodoo.board);
                    let model = config.voodoo_model.unwrap_or(memory.model());
                    crate::voodoo::Board::of(model, memory.megabytes())
                },
                renderer: config.voodoo_renderer.unwrap_or(default.voodoo.renderer),
                texture_sampling: config.voodoo_texture_sampling.unwrap_or(default.voodoo.texture_sampling),
                scale: config.voodoo_scale.unwrap_or(default.voodoo.scale),
                scale_shader: config.voodoo_scale_shader.unwrap_or(default.voodoo.scale_shader),
                msaa: config.voodoo_msaa.unwrap_or(default.voodoo.msaa),
                anisotropy: config.voodoo_anisotropy.unwrap_or(default.voodoo.anisotropy),
                fps_cap: config.voodoo_fps_cap.map_or(default.voodoo.fps_cap, |n| (n > 0).then_some(n)),
                gamma: config.voodoo_gamma.unwrap_or(default.voodoo.gamma),
                dither: config.voodoo_dither.unwrap_or(default.voodoo.dither),
                gpu_reads: config
                    .voodoo_readback_gpu
                    .unwrap_or(default.voodoo.gpu_reads),
            },
            powervr: config.powervr.unwrap_or(default.powervr),
            powervr_filter: config.powervr_filter.unwrap_or(default.powervr_filter),
            verite: crate::verite::VeriteSettings {
                risc: config.verite_risc.unwrap_or(default.verite.risc),
                mhz: config.verite_mhz.unwrap_or(default.verite.mhz),
                rom: config.verite_rom.clone().or(default.verite.rom),
                irq: config.verite_irq.unwrap_or(default.verite.irq),
            },
            capture_dir: config.capture_dir.clone().unwrap_or(default.capture_dir),
            record_ui: config.record_ui.unwrap_or(default.record_ui),
            record_shader: config.record_shader.unwrap_or(default.record_shader),
            cycles: config.cycles.unwrap_or(default.cycles),
            auto_fps: config.auto_fps.unwrap_or(default.auto_fps),
            auto_busy_os: config.auto_busy_os.unwrap_or(default.auto_busy_os),
            cpu: config.cpu.unwrap_or(default.cpu),
            core: config.core.unwrap_or(default.core),
            fpu_fast: config.fpu_fast.unwrap_or(default.fpu_fast),
            memsize: config.memsize.unwrap_or(default.memsize).min(config.cpu.unwrap_or(default.cpu).max_memsize()),
            ems: config.ems.unwrap_or(default.ems),
            umb: config.umb.unwrap_or(default.umb),
            dpmi: config.dpmi.unwrap_or(default.dpmi),
            win32: config.win32.unwrap_or(default.win32),
            win32_dlls: config.win32_dlls.clone().or(default.win32_dlls),
            win32_dlls_replace: config
                .win32_dlls_replace
                .clone()
                .unwrap_or(default.win32_dlls_replace),
            win32_version: config.win32_version.unwrap_or(default.win32_version),
            win32_depth: config.win32_depth.unwrap_or(default.win32_depth),
            win32_trace: config.win32_trace.unwrap_or(default.win32_trace),
            dos_high: config.dos_high.unwrap_or(default.dos_high),
            dos_version: config.dos_version.unwrap_or(default.dos_version),
            ide_hard_disks: config.ide_hard_disks.unwrap_or(default.ide_hard_disks),
            boot_cdrom: config.boot_cdrom.unwrap_or(default.boot_cdrom),
            floppy_controller: config.floppy_controller.unwrap_or(default.floppy_controller),
            mouse_autocapture: config.mouse_autocapture.unwrap_or(default.mouse_autocapture),
            mouse_sensitivity: config.mouse_sensitivity.unwrap_or(default.mouse_sensitivity),
            mouse_capture_messages: config.mouse_capture_messages.unwrap_or(default.mouse_capture_messages),
            mouse_menu_hold: config.mouse_menu_hold.unwrap_or(default.mouse_menu_hold),
            pause_in_settings: config.pause_in_settings.unwrap_or(default.pause_in_settings),
            version_check: config.version_check.unwrap_or(default.version_check),
            almanac_auto: config.almanac_auto.unwrap_or(default.almanac_auto),
            rewind: config.rewind.unwrap_or(default.rewind),
            rewind_memory: config.rewind_memory.unwrap_or(default.rewind_memory),
            idle_skip: config.idle_skip.unwrap_or(default.idle_skip),
            idle_hint: config.idle_hint.unwrap_or(default.idle_hint),
            game_fixes: config.game_fixes.unwrap_or(default.game_fixes),
            game_fixes_off: config.game_fixes_off.clone().unwrap_or(default.game_fixes_off),
            keyboard_layout: config.keyboard_layout.unwrap_or(default.keyboard_layout),
            sound: config.sound.clone(),
            disk: config.disk,
            mixer: config.mixer,
            audio_buffer: config.audio_buffer,
            joystick: config.joystick,
            network: config.network.clone(),
            serial: config.serial.clone(),
            printer: config.printer.clone(),
            achievements: config.achievements.clone(),
            shell: config.shell,
            vr: config.vr.clone(),
            vfx1: config.vfx1,
        }
    }
}

/// Every setting as the file writes it: section, key and value, or None
/// for a setting that is not set.
pub(super) fn entries(settings: &Settings, home: Option<&Path>) -> Vec<(Section, &'static str, Option<String>)> {
    use Section::{Emulator, Sound};
    let yes_no = |on: bool| Some(if on { "true" } else { "false" }.to_string());
    let sound = &settings.sound;
    let (sb, gus) = (&sound.sb, &sound.gus);
    let mixer = Channel::ALL
        .map(|channel| (Section::Mixer, channel.key(), Some(settings.mixer.level(channel).to_string())));
    let mut entries = vec![
        (Emulator, "scale", Some(settings.scale.to_string())),
        (Emulator, "fullscreen", yes_no(settings.fullscreen)),
        (Emulator, "aspect", Some(settings.aspect.name().to_string())),
        (Emulator, "vrr", yes_no(settings.vrr)),
        (Emulator, "filter", Some(settings.filter.name().to_string())),
        (Emulator, "shader", Some(settings.shader.name().to_string())),
        (Emulator, "crt_curvature", Some(settings.crt.curvature.to_string())),
        (Emulator, "crt_glow", Some(settings.crt.glow.to_string())),
        (Emulator, "monochrome", Some(settings.monochrome.name().to_string())),
        (Emulator, "monitor_effects", Some(settings.monitor_effects.name().to_string())),
        (Emulator, "post", Some(settings.post.name().to_string())),
        (Emulator, "composite", Some(settings.composite.mode.name().to_string())),
        (Emulator, "composite_era", Some(settings.composite.era.name().to_string())),
        (Emulator, "machine", Some(settings.machine.name().to_string())),
        (Emulator, "voodoo", yes_no(settings.voodoo.enabled)),
        (Emulator, "voodoo_model", Some(settings.voodoo.board.model().name().to_string())),
        (Emulator, "voodoo_memory", Some(settings.voodoo.board.megabytes().to_string())),
        (Emulator, "voodoo_renderer", Some(settings.voodoo.renderer.name().to_string())),
        (Emulator, "voodoo_texture_sampling", Some(settings.voodoo.texture_sampling.name().to_string())),
        (Emulator, "voodoo_scale", Some(settings.voodoo.scale.to_string())),
        (Emulator, "voodoo_scale_shader", yes_no(settings.voodoo.scale_shader)),
        (Emulator, "voodoo_dither", yes_no(settings.voodoo.dither)),
        (
            Emulator,
            "voodoo_readback",
            Some(if settings.voodoo.gpu_reads { "gpu" } else { "exact" }.to_string()),
        ),
        (Emulator, "voodoo_msaa", Some(match settings.voodoo.msaa {
            1 => "off".to_string(),
            n => n.to_string(),
        })),
        (Emulator, "voodoo_anisotropy", Some(match settings.voodoo.anisotropy {
            1 => "off".to_string(),
            n => n.to_string(),
        })),
        (Emulator, "voodoo_fps_cap", Some(settings.voodoo.fps_cap.map_or("off".to_string(), |n| n.to_string()))),
        (Emulator, "voodoo_gamma", Some(settings.voodoo.gamma.map_or("off".to_string(), |g| g.to_string()))),
        (Emulator, "powervr", Some(settings.powervr.map_or("off", |chip| chip.name()).to_string())),
        (Emulator, "powervr_filter", Some(settings.powervr_filter.name().to_string())),
        (Emulator, "verite_risc", Some(settings.verite.risc.name().to_string())),
        (Emulator, "verite_mhz", Some(settings.verite.mhz.to_string())),
        (Emulator, "verite_rom", settings.verite.rom.as_deref().map(|p| contract_home(p, home))),
        (Emulator, "verite_irq", Some(settings.verite.irq.to_string())),
        (Emulator, "capture_dir", Some(contract_home(&settings.capture_dir, home))),
        (Emulator, "record_ui", yes_no(settings.record_ui)),
        (Emulator, "record_shader", yes_no(settings.record_shader)),
        (
            Emulator,
            "cycles",
            Some(settings.cycles.to_string()),
        ),
        (Emulator, "auto_fps", Some(crate::timer::auto_fps_text(settings.auto_fps))),
        (Emulator, "auto_busy_os", yes_no(settings.auto_busy_os)),
        (
            Emulator,
            "cpu",
            Some(match settings.cpu {
                CpuModel::I386 => "386",
                CpuModel::I486 => "486",
                CpuModel::Pentium => "pentium",
                CpuModel::PentiumMmx => "pentium_mmx",
                CpuModel::K6_2 => "k6_2",
                CpuModel::PentiumPro => "pentium_pro",
                CpuModel::PentiumII => "pentium_ii",
                CpuModel::PentiumIII => "pentium_iii",
            }
            .to_string()),
        ),
        (Emulator, "core", Some(settings.core.name().to_string())),
        (Emulator, "fpu", Some(if settings.fpu_fast { "fast" } else { "exact" }.to_string())),
        (Emulator, "memsize", Some(settings.memsize.to_string())),
        (Emulator, "ems", yes_no(settings.ems)),
        (Emulator, "umb", yes_no(settings.umb)),
        (Emulator, "dpmi", yes_no(settings.dpmi)),
        (Emulator, "win32", yes_no(settings.win32)),
        (Emulator, "win32_dlls", settings.win32_dlls.as_deref().map(|p| contract_home(p, home))),
        (
            Emulator,
            "win32_dlls_replace",
            Some(settings.win32_dlls_replace.join(",")).filter(|names| !names.is_empty()),
        ),
        (Emulator, "win32_version", Some(settings.win32_version.name().to_string())),
        (Emulator, "win32_depth", Some(settings.win32_depth.name().to_string())),
        (Emulator, "win32_trace", Some(settings.win32_trace.name().to_string())),
        (Emulator, "dos_high", yes_no(settings.dos_high)),
        (Emulator, "dos_version", Some(settings.dos_version.name())),
        (Emulator, "ide_hard_disks", yes_no(settings.ide_hard_disks)),
        (Emulator, "boot_cdrom", yes_no(settings.boot_cdrom)),
        (Emulator, "floppy_controller", yes_no(settings.floppy_controller)),
        (Emulator, "keyboard_layout", Some(settings.keyboard_layout.name().to_string())),
        (Emulator, "mouse_autocapture", yes_no(settings.mouse_autocapture)),
        (Emulator, "mouse_sensitivity", Some(settings.mouse_sensitivity.to_string())),
        (Emulator, "mouse_capture_messages", yes_no(settings.mouse_capture_messages)),
        (Emulator, "mouse_menu_hold", yes_no(settings.mouse_menu_hold)),
        (Emulator, "pause_in_settings", yes_no(settings.pause_in_settings)),
        (Emulator, "version_check", yes_no(settings.version_check)),
        (Emulator, "almanac_auto", yes_no(settings.almanac_auto)),
        (Emulator, "rewind", yes_no(settings.rewind)),
        (Emulator, "rewind_memory", Some(settings.rewind_memory.to_string())),
        (Emulator, "idle_skip", yes_no(settings.idle_skip)),
        (Emulator, "idle_hint", yes_no(settings.idle_hint)),
        (Emulator, "game_fixes", yes_no(settings.game_fixes)),
        (
            Emulator,
            "game_fixes_off",
            Some(settings.game_fixes_off.join(",")).filter(|names| !names.is_empty()),
        ),
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
        (Sound, "sc55roms", sound.sc55roms.as_deref().map(|p| contract_home(p, home))),
        (Sound, "sc55model", Some(sound.sc55model.clone())),
        (Sound, "midiport", (!sound.midiport.is_empty()).then(|| sound.midiport.clone())),
        (Sound, "lpt_dac", Some(sound.lpt_dac.name().to_string())),
        (Sound, "tandy", Some(sound.tandy.name().to_string())),
        (Sound, "pit2_gate_at_start", yes_no(sound.pit2_gate_at_start)),
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
        (Section::Mixer, "blocksize", Some(settings.audio_buffer.blocksize.to_string())),
        (Section::Mixer, "prebuffer", Some(settings.audio_buffer.prebuffer.to_string())),
    ]);
    entries.extend([
        (Section::Joystick, "joysticktype", Some(settings.joystick.kind.name().to_string())),
        (Section::Joystick, "deadzone", Some(settings.joystick.deadzone.to_string())),
    ]);
    entries.extend(settings.network.entries().into_iter().map(|(key, value)| (Section::Network, key, value)));
    entries.extend(settings.serial.entries().into_iter().map(|(key, value)| (Section::Serial, key, value)));
    entries.extend(settings.printer.entries(home).into_iter().map(|(key, value)| (Section::Printer, key, value)));
    entries.extend(settings.achievements.entries().into_iter().map(|(key, value)| (Section::Achievements, key, value)));
    entries.extend(settings.shell.entries().into_iter().map(|(key, value)| (Section::Shell, key, value)));
    entries.extend(settings.vr.entries().into_iter().map(|(key, value)| (Section::Vr, key, value)));
    let vfx1 = settings.vfx1.entries().into_iter();
    entries.extend(vfx1.map(|(key, value)| (Section::Vfx1, key, value)));
    entries
}

/// The names in a list setting: separated by commas or spaces, without
/// quotes, lowercase.
fn names(value: &str) -> impl Iterator<Item = String> + '_ {
    value
        .split([',', ' ', '\t'])
        .map(|name| name.trim().trim_matches('"').to_ascii_lowercase())
}

/// The fix names of a `game_fixes_off` list: separated by commas or
/// spaces, lowercase.
pub fn fix_names(value: &str) -> Vec<String> {
    names(value).filter(|name| !name.is_empty()).collect()
}

/// The DLL names of a `win32_dlls_replace` list: separated by commas or
/// spaces, lowercase, without ".dll".
pub fn dll_names(value: &str) -> Vec<String> {
    names(value)
        .map(|name| name.strip_suffix(".dll").map(str::to_string).unwrap_or(name))
        .filter(|name| !name.is_empty())
        .collect()
}
