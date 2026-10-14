//! Picking and typing setting values: the value tables and helpers, and
//! `Item`'s choices, steps and text entry.

use super::Frontend;
use super::item::Item;
use crate::config::{AspectRatio, MidiSynth, Settings};
use crate::cpu::{CoreMode, CpuModel};
use crate::disk::{DriveInfo, DriveKind};
use crate::diskio::{DiskSpeed, NoiseMode};
use crate::joystick::{JoystickType, MAX_DEADZONE};
use crate::mixer::{ChorusPreset, DEFAULT_MIX, MAX_LEVEL, MAX_MIX, ReverbPreset};
use crate::mount::expand_host_path;
use crate::sb::SbModel;
use crate::timer::CpuSpeed;
use crate::video::shader::{CrtSettings, MAX_AMOUNT, parse_amount};
use std::path::Path;

/// The value `dir` steps away from `current` in `values`, wrapping around.
pub(super) fn cycle<T: PartialEq + Clone>(values: &[T], current: &T, dir: isize) -> T {
    let at = match values.iter().position(|v| v == current) {
        Some(i) => i as isize,
        None if dir > 0 => -1,
        None => 0,
    };
    values[(at + dir).rem_euclid(values.len() as isize) as usize].clone()
}

/// `s` with each of `values` put in by `set`, in turn.
/// The 3dfx frame rate caps to pick from: none, and the rates games ran
/// at and that divide common refresh rates evenly.
const FPS_CAPS: [Option<u32>; 13] =
    [None, Some(20), Some(24), Some(25), Some(30), Some(35), Some(40), Some(45), Some(48), Some(50), Some(60), Some(72), Some(120)];

/// The 3dfx gammas to pick from: none, then 0.1 to 2.0 in steps of 0.1.
fn gammas() -> impl Iterator<Item = Option<f32>> {
    std::iter::once(None).chain((1..=20).map(|tenths| Some(tenths as f32 / 10.0)))
}

/// A `blocksize` with the time it plays for, and whether it is the default.
pub(super) fn describe_blocksize(frames: u16) -> String {
    let default = if frames == crate::audio::AudioBuffer::default().blocksize { ", default" } else { "" };
    format!("{} frames ({:.1} ms{})", frames, crate::audio::AudioBuffer::millis(frames), default)
}

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
/// The frame rates auto speed's target steps through: auto (the
/// display's refresh rate), then rates games cap themselves at and common
/// refresh rates.
const AUTO_FPS: [u32; 13] = [AUTO_FPS_AUTO, 20, 24, 25, 30, 35, 40, 50, 60, 70, 75, 120, 144];
const AUTO_FPS_AUTO: u32 = 0;
const REWIND_MEMORY: [usize; 7] = [64, 128, 256, 512, 1024, 2048, 4096];

/// The memory sizes the slider steps through, as far as the CPU takes
/// (`CpuModel::max_memsize`).
const MEMSIZES: [usize; 16] = [2, 4, 8, 12, 16, 20, 24, 32, 48, 64, 96, 128, 192, 256, 384, 512];

/// The deadzone's slider: a square for every 5%.
pub(super) const DEADZONE_UNIT: u16 = 5;

/// The memory sizes up to what `cpu` takes.
fn memsizes(cpu: CpuModel) -> impl DoubleEndedIterator<Item = usize> + Clone {
    MEMSIZES.into_iter().filter(move |&mb| mb <= cpu.max_memsize())
}

/// The memory's slider: a square for every step above the least.
pub(super) fn memsize_bar(mb: usize, cpu: CpuModel) -> String {
    let steps = memsizes(cpu);
    let (full, cells) = (steps.clone().filter(|&m| m <= mb).count().saturating_sub(1), steps.count() - 1);
    format!("{:>3} MB {}{}", mb, "■".repeat(full), "·".repeat(cells - full))
}

/// A value of up to `max` as `text` and a bar of small squares, one for
/// every `unit`, which stay apart from the next row's:
/// "120% ■■■■■■■■■■■■········".
pub(super) fn bar(text: String, value: u16, max: u16, unit: u16) -> String {
    let (full, cells) = ((value.min(max) / unit) as usize, (max / unit) as usize);
    format!("{} {}{}", text, "■".repeat(full), "·".repeat(cells - full))
}

/// The screen's light's slider steps by this many percent.
pub(super) const SCREEN_GLOW_UNIT: u16 = 25;

/// A percentage's bar, a square for every 10%.
pub(super) fn percent_bar(percent: u16, max: u16) -> String {
    bar(format!("{:>3}%", percent), percent, max, 10)
}

/// A value stepped left or right to the next multiple of `unit`, from a
/// value in between to the multiple on that side, within 0 to `max`.
fn step_units(value: u16, dir: isize, unit: u16, max: u16) -> u16 {
    let units = if dir > 0 { value / unit + 1 } else { value.div_ceil(unit).saturating_sub(1) };
    (units * unit).min(max)
}

pub(super) fn on_off(on: bool) -> String {
    if on { "on" } else { "off" }.to_string()
}

/// Whether General MIDI can play through a SoundFont: one picked from the
/// host's files, with the synthesizer built in.
pub(super) fn soundfonts(frontend: Frontend) -> bool {
    cfg!(feature = "midi") && frontend.host_files
}

/// Whether the Sound Blaster is an AWE32.
pub(super) fn awe32(s: &Settings) -> bool {
    s.sound.sb_installed && s.sound.sb.model == SbModel::Awe32
}

/// A memory size in KB, as the settings show it.
pub(super) fn ram_size(kb: u32) -> String {
    match kb {
        0 => "none".to_string(),
        kb if kb >= 1024 => format!("{} MB", kb / 1024),
        kb => format!("{} KB", kb),
    }
}

/// Whether munt's MT-32 can play: its library and ROMs are the host's.
pub(super) fn mt32(frontend: Frontend) -> bool {
    cfg!(not(target_arch = "wasm32")) && frontend.host_files
}

/// The Sound Canvas ROMs the settings `s` would play with.
pub(super) fn sc55_found(s: &Settings) -> Option<crate::sc55::rom::Found> {
    crate::sc55::rom::find_cached(s.sound.sc55roms.as_deref(), &s.sound.sc55model)
}

/// The Sound Canvas models to pick from: auto, the sets there are, and
/// the one set.
fn sc55_models(s: &Settings) -> Vec<String> {
    let mut models = vec!["auto".to_string()];
    let dirs = match &s.sound.sc55roms {
        Some(dir) => vec![dir.clone()],
        None => crate::sc55::rom::default_dirs(),
    };
    for found in crate::sc55::rom::scan(&dirs, "auto") {
        if !models.iter().any(|m| m == found.romset.name) {
            models.push(found.romset.name.to_string());
        }
    }
    if !models.contains(&s.sound.sc55model) {
        models.push(s.sound.sc55model.clone());
    }
    models
}

/// Whether MIDI can go out of the host's MIDI ports.
pub(super) fn host_midi(frontend: Frontend) -> bool {
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
    /// The values the setting is picked from, in order, each as the
    /// settings `s` with it; none for one that is typed or picked some
    /// other way, and for a row of several. `drives` are the mounted
    /// drives, which the Ultrasound's drive can't take.
    pub(super) fn choices(self, s: &Settings, drives: &[DriveInfo], frontend: Frontend) -> Vec<Settings> {
        use Item::*;
        let on_off = |set: fn(&mut Settings, bool)| each(s, [false, true], set);
        match self {
            Scale => each(s, 1..=16, |s, scale| s.scale = scale),
            Fullscreen => on_off(|s, on| s.fullscreen = on),
            Aspect => each(s, AspectRatio::ALL, |s, aspect| s.aspect = aspect),
            Vrr => on_off(|s, on| s.vrr = on),
            VrMode => each(s, crate::vr::VrMode::ALL, |s, mode| s.vr.mode = mode),
            VrControllers => each(s, crate::vr::VrControllers::ALL, |s, c| s.vr.controllers = c),
            VrSpatialAudio => on_off(|s, on| s.vr.spatial_audio = on),
            VrScreenFit => each(s, crate::vr::ScreenFit::ALL, |s, fit| s.vr.screen_fit = fit),
            VrQuality => each(s, crate::vr::VrQuality::ALL, |s, q| s.vr.quality = q),
            VrAmbientOcclusion => each(s, crate::vr::VrSwitch::ALL, |s, ao| s.vr.ambient_occlusion = ao),
            VrMsaa => each(s, crate::vr::VrMsaa::ALL, |s, msaa| s.vr.msaa = msaa),
            VrRefresh => each(s, std::iter::once(None).chain(crate::vr::REFRESH_RATES.map(Some)), |s, hz| s.vr.refresh = hz),
            VrFoveation => each(s, crate::vr::VrFoveation::ALL, |s, f| s.vr.foveation = f),
            VrKeyboard => each(s, crate::vr::VrKeyboard::ALL, |s, k| s.vr.keyboard = k),
            VrTime => {
                let times = std::iter::once(crate::vr::VrTime::Now).chain((0..24).map(|h| crate::vr::VrTime::At(h * 60)));
                each(s, times, |s, t| s.vr.time = t)
            }
            VrTimeSpeed => each(s, crate::vr::TIME_SPEEDS, |s, speed| s.vr.time_speed = speed),
            VrWeather => each(s, crate::vr::VrWeather::ALL, |s, w| s.vr.weather = w),
            VrLightning => on_off(|s, on| s.vr.lightning = on),
            VrGraphics => each(s, crate::vr::VrGraphics::ALL, |s, g| s.vr.graphics = g),
            Filter => each(s, [crate::config::Filter::Nearest, crate::config::Filter::Linear], |s, f| s.filter = f),
            Shader => each(s, crate::video::shader::Shader::ALL, |s, shader| s.shader = shader),
            Monochrome => each(s, crate::video::mono::Monochrome::ALL, |s, mono| s.monochrome = mono),
            MonitorEffects => each(s, crate::video::power::MonitorEffects::ALL, |s, e| s.monitor_effects = e),
            Post => each(s, crate::post::PostMode::ALL, |s, post| s.post = post),
            Composite => each(s, crate::video::composite::CompositeMode::ALL, |s, mode| s.composite.mode = mode),
            CompositeEra => each(s, crate::video::composite::CompositeEra::ALL, |s, era| s.composite.era = era),
            Core => each(s, [CoreMode::Auto, CoreMode::Dynamic, CoreMode::Normal], |s, core| s.core = core),
            Fpu => on_off(|s, on| s.fpu_fast = on),
            Cpu => each(s, CpuModel::ALL, |s, cpu| {
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
            VoodooTextureSampling => {
                use crate::voodoo::TextureSampling;
                each(s, [TextureSampling::Default, TextureSampling::Unfiltered], |s, sampling| {
                    s.voodoo.texture_sampling = sampling
                })
            }
            VoodooScaleShader => on_off(|s, on| s.voodoo.scale_shader = on),
            VoodooMsaa => each(s, [1, 2, 4, 8], |s, samples| s.voodoo.msaa = samples),
            VoodooAnisotropy => each(s, [1, 2, 4, 8, 16], |s, n| s.voodoo.anisotropy = n),
            VoodooFpsCap => each(s, FPS_CAPS, |s, fps| s.voodoo.fps_cap = fps),
            VoodooGamma => each(s, gammas(), |s, gamma| s.voodoo.gamma = gamma),
            PowerVr => on_off(|s, on| s.powervr = on.then_some(crate::powervr::Chip::Pcx2)),
            PowerVrFilter => {
                use crate::powervr::tsp::Filter;
                each(s, [Filter::Auto, Filter::Point, Filter::Bilinear], |s, filter| s.powervr_filter = filter)
            }
            Ems => on_off(|s, on| s.ems = on),
            Umb => on_off(|s, on| s.umb = on),
            DosHigh => on_off(|s, on| s.dos_high = on),
            Dpmi => on_off(|s, on| s.dpmi = on),
            IdeHardDisks => on_off(|s, on| s.ide_hard_disks = on),
            BootCdrom => on_off(|s, on| s.boot_cdrom = on),
            FloppyController => on_off(|s, on| s.floppy_controller = on),
            DosVersion => {
                let mut versions = crate::config::DosVersion::PRESETS.to_vec();
                if !versions.contains(&s.dos_version) {
                    versions.push(s.dos_version);
                    versions.sort();
                }
                each(s, versions, |s, version| s.dos_version = version)
            }
            KeyboardLayout => each(s, crate::keylayout::LayoutSetting::all(), |s, layout| s.keyboard_layout = layout),
            MouseAutocapture => on_off(|s, on| s.mouse_autocapture = on),
            MouseCaptureMessages => on_off(|s, on| s.mouse_capture_messages = on),
            MouseMenuHold => on_off(|s, on| s.mouse_menu_hold = on),
            ShellSuggestions => on_off(|s, on| s.shell.autosuggest = on),
            ShellColors => on_off(|s, on| s.shell.colors = on),
            SaveShellHistory => on_off(|s, on| s.shell.save_history = on),
            VersionCheck => on_off(|s, on| s.version_check = on),
            Rewind => on_off(|s, on| s.rewind = on),
            RecordUi => on_off(|s, on| s.record_ui = on),
            RecordShader => on_off(|s, on| s.record_shader = on),
            RewindMemory => each(s, REWIND_MEMORY, |s, mb| s.rewind_memory = mb),
            SbType => {
                let models = [Some(SbModel::Sb16), Some(SbModel::Awe32), Some(SbModel::SbPro2), Some(SbModel::Sb2), Some(SbModel::Sb16Vibra), None];
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
                if frontend.host_files {
                    synths.push(MidiSynth::Sc55);
                }
                if host_midi(frontend) {
                    synths.push(MidiSynth::Host);
                }
                synths.push(MidiSynth::None);
                each(s, synths, |s, synth| s.sound.midisynth = synth)
            }
            Mt32Model => each(s, crate::config::Mt32Model::ALL, |s, model| s.sound.mt32model = model),
            Sc55Model => each(s, sc55_models(s), |s, model| s.sound.sc55model = model),
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
            AudioBlocksize => each(s, crate::audio::BLOCKSIZES, |s, frames| s.audio_buffer.blocksize = frames),
            AudioPrebuffer => each(s, crate::audio::PREBUFFERS, |s, ms| s.audio_buffer.prebuffer = ms),
            LptDac => each(s, crate::lpt_dac::LptDacType::ALL, |s, dac| s.sound.lpt_dac = dac),
            TandySound => each(s, crate::sn76489::TandySound::ALL, |s, tandy| s.sound.tandy = tandy),
            Pit2GateAtStart => on_off(|s, on| s.sound.pit2_gate_at_start = on),
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
            Cycles | AutoFps | CrtCurvature | CrtGlow | Memsize | Volume(_) | ReverbMix | ChorusMix | Deadzone | MouseSensitivity | UltraDir
            | SoundFont | Mt32Roms | Awe32Rom | Awe32Download | Sc55Roms | Sc55Download | VoodooOverlay | CaptureDir | Autoexec | SbPorts | GusPorts | MacAddr
            | Rooms | Relay | Player | VrScene | VrScreenGlow | VrCenter | VrMarkKeyboard | VrSceneScale | VrSeat(_) | VrSeatTurn | VrResolution
            | VrLatitude | VrWeatherSound
            | Lan | LanHost | Room | Password | ModemListen | Phonebook => Vec::new(),
        }
    }

    /// Step the setting left (-1) or right (1): a slider a square of its
    /// bar. `drives` are the mounted drives, which the Ultrasound's drive
    /// can't take.
    pub(super) fn step(self, s: &mut Settings, dir: isize, drives: &[DriveInfo], frontend: Frontend) {
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
            AutoFps => {
                let next = step_number(&AUTO_FPS, s.auto_fps.unwrap_or(AUTO_FPS_AUTO), dir);
                s.auto_fps = (next != AUTO_FPS_AUTO).then_some(next);
            }
            RewindMemory => s.rewind_memory = step_number(&REWIND_MEMORY, s.rewind_memory, dir),
            CrtCurvature => s.crt.curvature = step_units(s.crt.curvature, dir, 10, MAX_AMOUNT),
            CrtGlow => s.crt.glow = step_units(s.crt.glow, dir, 10, MAX_AMOUNT),
            VrScreenGlow => {
                let max = crate::vr::SCREEN_GLOW_MAX as u16;
                s.vr.screen_glow = step_units(s.vr.screen_glow.min(max as u32) as u16, dir, SCREEN_GLOW_UNIT, max) as u32;
            }
            VrSceneScale => {
                let (min, max) = (crate::vr::SCENE_SCALE_MIN as isize, crate::vr::SCENE_SCALE_MAX as isize);
                s.vr.scene_scale = (s.vr.scene_scale as isize + dir).clamp(min, max) as u32;
            }
            VrSeat(axis) => {
                let max = crate::vr::SEAT_SHIFT_MAX;
                let seat = &mut s.vr.seat[axis as usize];
                *seat = (*seat + dir as i32).clamp(-max, max);
            }
            VrSeatTurn => {
                let max = crate::vr::SEAT_TURN_MAX;
                s.vr.seat_turn = (s.vr.seat_turn + dir as i32).clamp(-max, max);
            }
            VrLatitude => {
                let max = crate::vr::LATITUDE_MAX;
                s.vr.latitude = (s.vr.latitude + dir as i32).clamp(-max, max);
            }
            VrWeatherSound => {
                let max = crate::vr::WEATHER_SOUND_MAX as u16;
                s.vr.weather_sound = step_units(s.vr.weather_sound.min(max as u32) as u16, dir, 10, max) as u32;
            }
            VrResolution => {
                let (min, max) = (crate::vr::RESOLUTION_MIN as isize, crate::vr::RESOLUTION_MAX as isize);
                // To the next ten that way, from wherever it was typed (or
                // from 100% for auto, which Delete puts back).
                let r = s.vr.resolution.unwrap_or(100) as isize;
                let next = if dir > 0 { (r / 10 + 1) * 10 } else { (r - 1) / 10 * 10 };
                s.vr.resolution = Some(next.clamp(min, max) as u32);
            }
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
            MouseSensitivity => {
                // Exact presets avoid drift and step typed values to the
                // nearest tenth on that side, from 0.1 to 10.0.
                let steps: Vec<f64> = (1..=100).map(|n| n as f64 / 10.0).collect();
                s.mouse_sensitivity = step_number(&steps, s.mouse_sensitivity, dir);
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
    pub(super) fn fields(self, s: &Settings) -> Vec<Item> {
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
    pub(super) fn text(self, s: &Settings) -> String {
        match self {
            Item::Cycles => s.cycles.to_string(),
            Item::AutoFps => crate::timer::auto_fps_text(s.auto_fps),
            Item::UltraDir => s.sound.gus.ultradir.clone().unwrap_or_default(),
            Item::Volume(channel) => s.mixer.level(channel).to_string(),
            Item::CaptureDir => s.capture_dir.display().to_string(),
            Item::Memsize => s.memsize.to_string(),
            Item::Deadzone => s.joystick.deadzone.to_string(),
            Item::MouseSensitivity => s.mouse_sensitivity.to_string(),
            Item::CrtCurvature => s.crt.curvature.to_string(),
            Item::CrtGlow => s.crt.glow.to_string(),
            Item::VrScreenGlow => s.vr.screen_glow.to_string(),
            Item::VrSceneScale => s.vr.scene_scale.to_string(),
            Item::VrSeat(axis) => s.vr.seat[axis as usize].to_string(),
            Item::VrSeatTurn => s.vr.seat_turn.to_string(),
            Item::VrLatitude => s.vr.latitude.to_string(),
            Item::VrWeatherSound => s.vr.weather_sound.to_string(),
            Item::VrResolution => s.vr.resolution.map_or("auto".to_string(), |r| r.to_string()),
            Item::ReverbMix => s.mixer.reverb_mix.to_string(),
            Item::ChorusMix => s.mixer.chorus_mix.to_string(),
            Item::MacAddr => s.network.mac.map_or(String::new(), |mac| mac.to_string()),
            Item::Relay => s.network.relay.clone(),
            Item::Player => s.network.player.clone(),
            Item::Lan => Item::Lan.value(s, None),
            Item::LanHost => s.network.lan_host.map_or("off".to_string(), |port| port.to_string()),
            Item::Room => s.network.room.clone(),
            Item::Password => s.network.password.clone(),
            Item::ModemListen => s.serial.modem_listen.map_or("off".to_string(), crate::serial::listen_text),
            _ => String::new(),
        }
    }

    pub(super) fn set_text(self, s: &mut Settings, text: &str) -> Result<(), String> {
        let text = text.trim();
        match self {
            Item::Cycles => s.cycles = CpuSpeed::parse(text)?,
            Item::AutoFps => s.auto_fps = crate::timer::parse_auto_fps(text)?,
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
            Item::MouseSensitivity => s.mouse_sensitivity = crate::config::parse_mouse_sensitivity(text)?,
            Item::CrtCurvature => s.crt.curvature = parse_amount(text).ok_or("The curvature goes from 0 to 100%")?,
            Item::CrtGlow => s.crt.glow = parse_amount(text).ok_or("The glow goes from 0 to 100%")?,
            Item::VrScreenGlow => s.vr.set("screen_glow", text, std::path::Path::new(""))?,
            Item::VrSceneScale => s.vr.set("scene_scale", text, std::path::Path::new(""))?,
            Item::VrSeat(axis) => s.vr.set(crate::vr::SEAT_AXES[axis as usize], text, std::path::Path::new(""))?,
            Item::VrSeatTurn => s.vr.set("seat_turn", text, std::path::Path::new(""))?,
            Item::VrLatitude => s.vr.set("latitude", text, std::path::Path::new(""))?,
            Item::VrWeatherSound => s.vr.set("weather_sound", text, std::path::Path::new(""))?,
            Item::VrResolution => s.vr.set("resolution", text, std::path::Path::new(""))?,
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
    pub(super) fn clear(self, s: &mut Settings) -> bool {
        match self {
            Item::UltraDir => s.sound.gus.ultradir.take().is_some(),
            Item::SoundFont => s.sound.soundfont.take().is_some(),
            Item::VrScene => s.vr.scene.take().is_some(),
            Item::Mt32Roms => s.sound.mt32roms.take().is_some(),
            Item::Sc55Roms => s.sound.sc55roms.take().is_some(),
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
            Item::MouseSensitivity => {
                std::mem::replace(&mut s.mouse_sensitivity, 1.0) != 1.0
            }
            Item::CrtCurvature => {
                let default = CrtSettings::default().curvature;
                std::mem::replace(&mut s.crt.curvature, default) != default
            }
            Item::CrtGlow => {
                let default = CrtSettings::default().glow;
                std::mem::replace(&mut s.crt.glow, default) != default
            }
            Item::VrScreenGlow => {
                let default = crate::vr::VrSettings::default().screen_glow;
                std::mem::replace(&mut s.vr.screen_glow, default) != default
            }
            Item::VrSceneScale => {
                let default = crate::vr::VrSettings::default().scene_scale;
                std::mem::replace(&mut s.vr.scene_scale, default) != default
            }
            Item::VrSeat(axis) => std::mem::take(&mut s.vr.seat[axis as usize]) != 0,
            Item::VrSeatTurn => std::mem::take(&mut s.vr.seat_turn) != 0,
            Item::VrLatitude => {
                let default = crate::vr::VrSettings::default().latitude;
                std::mem::replace(&mut s.vr.latitude, default) != default
            }
            Item::VrWeatherSound => {
                let default = crate::vr::VrSettings::default().weather_sound;
                std::mem::replace(&mut s.vr.weather_sound, default) != default
            }
            Item::VrResolution => {
                let default = crate::vr::VrSettings::default().resolution;
                std::mem::replace(&mut s.vr.resolution, default) != default
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
