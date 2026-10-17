//! Reading configuration text into a `Config`.

use super::types::parse_bool;
use super::{AspectRatio, Config, DosVersion, Filter, Section, Settings, parse_mouse_sensitivity};
use crate::cpu::{CoreMode, CpuModel};
use crate::disk::DRIVE_Z;
use crate::diskio::{DiskSpeed, NoiseMode};
use crate::joystick::JoystickType;
use crate::keylayout::LayoutSetting;
use crate::mount::{parse_drive_name, parse_mount_spec, tokenize};
use crate::mixer::{Channel, ChorusPreset, ReverbPreset, SbFilter};
use crate::timer::CpuSpeed;
use crate::video::adapter::Adapter;
use crate::video::composite::{CompositeEra, CompositeMode};
use crate::video::mono::Monochrome;
use crate::video::shader::Shader;
use std::path::{Path, PathBuf};

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
    // A [vr] section of the settings before `auto` (without `msaa`).
    let mut vr_before_auto = true;

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
            | Section::Phonebook
            | Section::Printer
            | Section::Achievements
            | Section::Shell
            | Section::Vr
            | Section::Vfx1
            | Section::Game
            | Section::Gamepad => {
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
                        "fullscreen" => match parse_bool(value) {
                            Some(on) => config.fullscreen = Some(on),
                            None => warn(format!("invalid fullscreen '{}' (true or false)", value)),
                        },
                        "aspect" => match AspectRatio::parse(value) {
                            Some(aspect) => config.aspect = Some(aspect),
                            None => warn(format!("invalid aspect '{}' (none, 4:3, 5:4, 16:10 or 16:9)", value)),
                        },
                        "vrr" => match parse_bool(value) {
                            Some(on) => config.vrr = Some(on),
                            None => warn(format!("invalid vrr '{}' (true or false)", value)),
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
                            None => warn(format!(
                                "invalid machine '{}' (svga, svga_s3, svga_s3virge, \
                                 svga_s3virgevx, svga_verite, svga_voodoo3, svga_banshee, \
                                 svga_ati_3drage, svga_ati_3drage2, svga_et4000, vga, ega, \
                                 cga, tandy, pcjr or hercules)",
                                value
                            )),
                        },
                        "voodoo" => match parse_bool(value) {
                            Some(on) => config.voodoo = Some(on),
                            None => warn(format!("invalid voodoo '{}' (true or false)", value)),
                        },
                        "powervr" => match crate::powervr::Chip::parse(value) {
                            Some(chip) => config.powervr = Some(chip),
                            None => {
                                warn(format!("invalid powervr '{}' (off, pcx1 or pcx2)", value))
                            }
                        },
                        "verite_risc" => match crate::verite::Risc::parse(value) {
                            Some(risc) => config.verite_risc = Some(risc),
                            None => warn(format!("invalid verite_risc '{}' (lle or hle)", value)),
                        },
                        "verite_mhz" => match value.parse::<u32>() {
                            Ok(n) if (1..=1000).contains(&n) => config.verite_mhz = Some(n),
                            _ => warn(format!("invalid verite_mhz '{}' (1 to 1000)", value)),
                        },
                        "verite_rom" => {
                            let value = value.trim_matches('"');
                            let path = match (value.strip_prefix("~/"), home) {
                                (Some(rest), Some(h)) => h.join(rest),
                                _ => PathBuf::from(value),
                            };
                            config.verite_rom = (!value.is_empty()).then_some(path);
                        }
                        "verite_irq" => match value.parse::<u8>() {
                            Ok(n) if crate::verite::IRQS.contains(&n) => {
                                config.verite_irq = Some(n)
                            }
                            _ => warn(format!(
                                "invalid verite_irq '{}' (5, 7, 9, 10, 11, 12 or 15)",
                                value
                            )),
                        },
                        "powervr_filter" => match crate::powervr::tsp::Filter::parse(value) {
                            Some(filter) => config.powervr_filter = Some(filter),
                            None => warn(format!("invalid powervr_filter '{}' (auto, point or bilinear)", value)),
                        },
                        "voodoo_model" => match crate::voodoo::Model::parse(value) {
                            Some(model) => config.voodoo_model = Some(model),
                            None => warn(format!("invalid voodoo_model '{}' (voodoo1 or voodoo2)", value)),
                        },
                        "voodoo_memory" => match crate::voodoo::Board::parse(value) {
                            Some(board) => config.voodoo_memory = Some(board),
                            None => warn(format!("invalid voodoo_memory '{}' (4, 8 or 12)", value)),
                        },
                        "voodoo_renderer" => match crate::voodoo::Renderer::parse(value) {
                            Some(renderer) => config.voodoo_renderer = Some(renderer),
                            None => warn(format!("invalid voodoo_renderer '{}' (software or opengl)", value)),
                        },
                        "voodoo_texture_sampling" => match crate::voodoo::TextureSampling::parse(value) {
                            Some(sampling) => config.voodoo_texture_sampling = Some(sampling),
                            None => warn(format!("invalid voodoo_texture_sampling '{}' (default or unfiltered)", value)),
                        },
                        "voodoo_scale" => match value.parse::<u32>() {
                            Ok(n) if (1..=4).contains(&n) => config.voodoo_scale = Some(n),
                            _ => warn(format!("invalid voodoo_scale '{}' (1 to 4)", value)),
                        },
                        "voodoo_scale_shader" => match parse_bool(value) {
                            Some(on) => config.voodoo_scale_shader = Some(on),
                            None => warn(format!("invalid voodoo_scale_shader '{}' (true or false)", value)),
                        },
                        "voodoo_msaa" => match value.to_ascii_lowercase().as_str() {
                            "off" | "1" => config.voodoo_msaa = Some(1),
                            "2" | "4" | "8" => config.voodoo_msaa = value.parse().ok(),
                            _ => warn(format!("invalid voodoo_msaa '{}' (off, 2, 4 or 8)", value)),
                        },
                        "voodoo_anisotropy" => match value.to_ascii_lowercase().as_str() {
                            "off" | "1" => config.voodoo_anisotropy = Some(1),
                            "2" | "4" | "8" | "16" => config.voodoo_anisotropy = value.parse().ok(),
                            _ => warn(format!("invalid voodoo_anisotropy '{}' (off, 2, 4, 8 or 16)", value)),
                        },
                        "voodoo_fps_cap" => match value.to_ascii_lowercase().as_str() {
                            "off" | "0" => config.voodoo_fps_cap = Some(0),
                            _ => match value.parse::<u32>() {
                                Ok(n) if (10..=240).contains(&n) => config.voodoo_fps_cap = Some(n),
                                _ => warn(format!("invalid voodoo_fps_cap '{}' (off, or 10 to 240)", value)),
                            },
                        },
                        "voodoo_gamma" => match value.to_ascii_lowercase().as_str() {
                            "off" => config.voodoo_gamma = Some(None),
                            _ => match value.parse::<f32>() {
                                Ok(g) if (0.1..=2.0).contains(&g) => config.voodoo_gamma = Some(Some(g)),
                                _ => warn(format!("invalid voodoo_gamma '{}' (off, or 0.1 to 2)", value)),
                            },
                        },
                        "post" => match crate::post::PostMode::parse(value) {
                            Some(post) => config.post = Some(post),
                            None => warn(format!("invalid post '{}' (off, quick or full)", value)),
                        },
                        "monitor_effects" => match crate::video::power::MonitorEffects::parse(value) {
                            Some(effects) => config.monitor_effects = Some(effects),
                            None => warn(format!("invalid monitor_effects '{}' (off, picture, sound or on)", value)),
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
                        "auto_fps" => match crate::timer::parse_auto_fps(value) {
                            Ok(fps) => config.auto_fps = Some(fps),
                            Err(e) => warn(e),
                        },
                        "auto_busy_os" => match parse_bool(value) {
                            Some(on) => config.auto_busy_os = Some(on),
                            None => {
                                warn(format!("invalid auto_busy_os '{}' (true or false)", value))
                            }
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
                        "floppy_controller" => match parse_bool(value) {
                            Some(on) => config.floppy_controller = Some(on),
                            None => warn(format!("invalid floppy_controller '{}' (true or false)", value)),
                        },
                        "dos_version" => match DosVersion::parse(value) {
                            Some(version) => config.dos_version = Some(version),
                            None => warn(format!("invalid dos_version '{}' (such as 5.00, 6.22 or 7.10)", value)),
                        },
                        "core" => match CoreMode::parse(value) {
                            Ok(core) => config.core = Some(core),
                            Err(e) => warn(e),
                        },
                        "fpu" => match value.to_ascii_lowercase().as_str() {
                            "exact" => config.fpu_fast = Some(false),
                            "fast" => config.fpu_fast = Some(true),
                            _ => warn(format!("invalid fpu '{}' (exact or fast)", value)),
                        },
                        "mouse_capture_messages" => match parse_bool(value) {
                            Some(on) => config.mouse_capture_messages = Some(on),
                            None => warn(format!("invalid mouse_capture_messages '{}' (true or false)", value)),
                        },
                        "mouse_menu_hold" => match parse_bool(value) {
                            Some(on) => config.mouse_menu_hold = Some(on),
                            None => warn(format!("invalid mouse_menu_hold '{}' (true or false)", value)),
                        },
                        "pause_in_settings" => match parse_bool(value) {
                            Some(on) => config.pause_in_settings = Some(on),
                            None => warn(format!("invalid pause_in_settings '{}' (true or false)", value)),
                        },
                        "version_check" => match parse_bool(value) {
                            Some(on) => config.version_check = Some(on),
                            None => warn(format!("invalid version_check '{}' (true or false)", value)),
                        },
                        "mouse_autocapture" => match parse_bool(value) {
                            Some(on) => config.mouse_autocapture = Some(on),
                            None => warn(format!("invalid mouse_autocapture '{}' (true or false)", value)),
                        },
                        "mouse_sensitivity" => match parse_mouse_sensitivity(value) {
                            Ok(multiplier) => config.mouse_sensitivity = Some(multiplier),
                            Err(message) => warn(message),
                        },
                        "rewind" => match parse_bool(value) {
                            Some(on) => config.rewind = Some(on),
                            None => warn(format!("invalid rewind '{}' (true or false)", value)),
                        },
                        "idle_skip" => match parse_bool(value) {
                            Some(on) => config.idle_skip = Some(on),
                            None => warn(format!("invalid idle_skip '{}' (true or false)", value)),
                        },
                        "idle_hint" => match parse_bool(value) {
                            Some(on) => config.idle_hint = Some(on),
                            None => warn(format!("invalid idle_hint '{}' (true or false)", value)),
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
                            "k6_2" | "k6-2" | "k62" => config.cpu = Some(CpuModel::K6_2),
                            "pentium_pro" | "pentiumpro" | "ppro" | "686" => config.cpu = Some(CpuModel::PentiumPro),
                            "pentium_ii" | "pentiumii" | "pentium2" => config.cpu = Some(CpuModel::PentiumII),
                            "pentium_iii" | "pentiumiii" | "pentium3" => config.cpu = Some(CpuModel::PentiumIII),
                            _ => warn(format!(
                                "invalid cpu '{}' (386, 486, pentium, pentium_mmx, k6_2, pentium_pro, pentium_ii or pentium_iii)",
                                value
                            )),
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
                        "blocksize" => match crate::audio::parse_blocksize(value) {
                            Ok(frames) => config.audio_buffer.blocksize = frames,
                            Err(e) => warn(e),
                        },
                        "prebuffer" => match crate::audio::parse_prebuffer(value) {
                            Ok(ms) => config.audio_buffer.prebuffer = ms,
                            Err(e) => warn(e),
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

                if section == Section::Gamepad {
                    config.game_pad.push((key.to_ascii_lowercase(), value.to_string()));
                    continue;
                }

                if section == Section::Game {
                    match key.to_ascii_lowercase().as_str() {
                        "name" if !value.is_empty() => config.game_name = Some(value.to_string()),
                        "name" => warn("the game's name is empty".to_string()),
                        "achievements" if !value.is_empty() => config.game_achievements = Some(value.to_string()),
                        "achievements" => {}
                        "manual" if !value.is_empty() => config.game_manuals.push(value.to_string()),
                        "manual" => {}
                        "source" if !value.is_empty() => config.game_source = Some(value.to_string()),
                        "input" if !value.is_empty() => config.game_input = Some(value.to_string()),
                        "os" if !value.is_empty() => config.game_os = Some(value.to_string()),
                        "os" => {}
                        "input" => {}
                        "variants" => match parse_bool(value) {
                            Some(on) => config.game_variants = on,
                            None => warn(format!("invalid variants '{}' (true or false)", value)),
                        },
                        "source" => {}
                        "overlay" => match parse_bool(value) {
                            Some(on) => config.game_overlay = on,
                            None => warn(format!("invalid overlay '{}' (true or false)", value)),
                        },
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

                if section == Section::Phonebook {
                    match crate::serial::phonebook::parse_line(key, value) {
                        Ok(entry) => config.serial.phonebook.push(entry),
                        Err(e) => warn(e),
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

                if section == Section::Shell {
                    if let Err(e) = config.shell.set(key, value) {
                        warn(e);
                    }
                    continue;
                }

                if section == Section::Vfx1 {
                    if let Err(e) = config.vfx1.set(key, value) {
                        warn(e);
                    }
                    continue;
                }

                if section == Section::Vr {
                    vr_before_auto &= !key.eq_ignore_ascii_case("msaa");
                    if let Err(e) = config.vr.set(key, value, base_dir) {
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
    if vr_before_auto {
        config.vr.from_before_auto();
    }
    config.warnings = warnings;
    config
}
