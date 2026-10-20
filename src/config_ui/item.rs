//! The settings rows: `Item`, how each is changed and when it applies,
//! its label, where it is offered and the value it shows.

use super::Frontend;
use super::values::{
    DEADZONE_UNIT, SCREEN_GLOW_UNIT, awe32, bar, describe_blocksize, host_midi, memsize_bar, mt32, on_off, percent_bar,
    ram_size, sc55_found, soundfonts,
};
use crate::config::{MidiSynth, Settings};
use crate::cpu::CoreMode;
use crate::disk::drive_letter;
use crate::diskio::DiskClass;
use crate::joystick::MAX_DEADZONE;
use crate::mixer::{Channel, ChorusPreset, MAX_LEVEL, MAX_MIX, ReverbPreset};
use crate::mount::contract_home;
use crate::sb::SbModel;
use crate::timer::CpuSpeed;
use crate::video::shader::MAX_AMOUNT;
use std::path::Path;

/// When a changed setting takes effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Applies {
    Now,
    /// Once no program runs: changing the hardware under one would break it.
    AtPrompt,
    /// On the screen now, and for programs once none runs (the monochrome
    /// monitor).
    NowAndAtPrompt,
    NextStart,
}

/// How a setting is changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Input {
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
pub(super) enum Item {
    Scale,
    Fullscreen,
    Aspect,
    /// Frames shown when the machine's are due, for a display with a
    /// variable refresh rate.
    Vrr,
    /// The picture in a 3D scene, the scene, what a headset's controllers
    /// do and whether the sound follows the viewer.
    VrMode,
    VrScene,
    VrControllers,
    VrSpatialAudio,
    VrScreenFit,
    /// How much of the scene's lighting is worked out, and how brightly
    /// the screen lights it.
    VrQuality,
    /// The ambient occlusion, and the samples a pixel takes for smooth
    /// edges.
    VrAmbientOcclusion,
    VrMsaa,
    VrScreenGlow,
    /// Center the headset's view where the head is now.
    VrCenter,
    /// How big the scene looks in the headset, and where its seat is from
    /// the scene's spawn: shifted right, up and ahead (`VrSettings::seat`'s
    /// axis), and turned.
    VrSceneScale,
    VrSeat(u8),
    VrSeatTurn,
    /// The real keyboard shown through the scene by the headset's cameras,
    /// and marking where it is with a controller.
    VrKeyboard,
    VrMarkKeyboard,
    /// The scene's time of day, how fast it goes and where on the earth
    /// it is (the sun's way); its weather, how loud that is and whether a
    /// storm has lightning.
    VrTime,
    VrTimeSpeed,
    VrLatitude,
    VrWeather,
    VrWeatherSound,
    VrLightning,
    /// The headset's eye images, in percent of the size its runtime
    /// recommends, and its refresh rate.
    VrResolution,
    VrFoveation,
    VrRefresh,
    /// How the headset's pictures get to its runtime (GLX, EGL, Vulkan).
    VrGraphics,
    /// The Forte VFX1 headset on the desk: its card's port, how its eyes'
    /// pictures are taken from the screen and swapped, how wide they look,
    /// and the head's turns made larger for games that steer by them.
    Vfx1,
    Vfx1Port,
    Vfx1Stereo,
    Vfx1SwapEyes,
    Vfx1ViewSize,
    Vfx1Steer,
    Vfx1StereoBios,
    Filter,
    Shader,
    /// How far the CRT look's tube bends and how much it glows, shown
    /// with that look.
    CrtCurvature,
    CrtGlow,
    Monochrome,
    /// The monitor's power, resync and degauss effects.
    MonitorEffects,
    /// The BIOS's power-on self test before DOS.
    Post,
    /// The CGA's composite monitor, and which CGA makes the signal.
    Composite,
    CompositeEra,
    Cycles,
    /// The frame rate auto speed aims for.
    AutoFps,
    /// Auto speed gives a program that keeps Windows busy all there is.
    AutoBusyOs,
    /// What runs the instructions: interpreter or dynamic recompiler.
    Core,
    /// Exact or fast FPU arithmetic.
    Fpu,
    Cpu,
    /// The display adapter.
    Machine,
    /// The 3dfx card, its memory, and how the host draws for it.
    Voodoo,
    VoodooMemory,
    VoodooRenderer,
    VoodooScale,
    VoodooScaleShader,
    VoodooMsaa,
    /// OpenGL's dithering to 16 bits, as the card's.
    VoodooDither,
    VoodooAnisotropy,
    VoodooTextureSampling,
    VoodooFpsCap,
    VoodooGamma,
    /// Fetch Glide's DOS overlay (GLIDE2X.OVL).
    VoodooOverlay,
    PowerVr,
    PowerVrFilter,
    /// The Rendition Vérité's RISC: emulated or its commands carried out.
    VeriteRisc,
    VeriteMhz,
    /// The dump of a Vérité board's ROM the RISC sees.
    VeriteRom,
    VeriteIrq,
    Memsize,
    /// Expanded memory and upper memory blocks.
    Ems,
    Umb,
    /// DOS's tables packed low, as DOS=HIGH has them.
    DosHigh,
    /// The DPMI host for DOS extenders.
    Dpmi,
    /// The DOS version programs are told.
    DosVersion,
    /// A booted system's hard disks on the IDE channels.
    IdeHardDisks,
    /// A booted system's CD-ROM drive, with or without a CD.
    BootCdrom,
    FloppyController,
    KeyboardLayout,
    /// The captured mouse's motion multiplier.
    MouseSensitivity,
    /// The mouse captured and let go by itself.
    MouseAutocapture,
    /// The messages that say the mouse was captured or let go.
    MouseCaptureMessages,
    /// The right button held still opening the settings window.
    MouseMenuHold,
    /// The machine waiting while the settings window is open.
    PauseInSettings,
    ShellSuggestions,
    ShellColors,
    SaveShellHistory,
    /// Ask rust-dos.com at startup whether there is a newer version.
    VersionCheck,
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
    /// The Sound Canvas: the directory with its ROMs, the model, and the
    /// ROMs' download.
    Sc55Roms,
    Sc55Model,
    Sc55Download,
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
    /// The sound waiting for the host's sound device: the device's buffer
    /// and the queue on top of it.
    AudioBlocksize,
    AudioPrebuffer,
    /// The DAC on the parallel port.
    LptDac,
    /// The Tandy's and PCjr's sound chip.
    TandySound,
    /// PIT channel 2's gate at startup (`pit2_gate_at_start`).
    Pit2GateAtStart,
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
    /// The numbers the modem dials bulletin boards with.
    Phonebook,
    /// The printer on LPT1: where its printing goes, its paper and
    /// resolution, whether a job's pages are one document, and how long
    /// a job waits for more.
    PrinterOutput,
    PrinterPaper,
    PrinterDpi,
    PrinterMultipage,
    PrinterTimeout,
}

impl Item {
    pub(super) fn label(self) -> &'static str {
        use Item::*;
        match self {
            Scale => "Window scale",
            Fullscreen => "Fullscreen",
            Aspect => "Fixed aspect ratio",
            Vrr => "Variable refresh rate",
            VrMode => "3D scene",
            VrScene => "Scene",
            VrControllers => "Headset controllers",
            VrSpatialAudio => "Sound from the screen",
            VrScreenFit => "Picture on the screen",
            VrQuality => "Lighting",
            VrAmbientOcclusion => "  Ambient occlusion",
            VrMsaa => "Antialiasing",
            VrScreenGlow => "Light from the screen",
            VrCenter => "  Center the view where you sit now",
            VrSceneScale => "Scene scale",
            VrSeat(0) => "  Seat to the right",
            VrSeat(1) => "  Seat higher",
            VrSeat(_) => "  Seat closer to the screen",
            VrSeatTurn => "  Seat turned to the left",
            VrKeyboard => "Show the real keyboard",
            VrMarkKeyboard => "  Mark the keyboard with a controller",
            VrTime => "Time of day",
            VrTimeSpeed => "  Time goes",
            VrLatitude => "  Latitude",
            VrWeather => "Weather",
            VrWeatherSound => "  Rain and thunder volume",
            VrLightning => "  Lightning",
            VrResolution => "Headset resolution",
            VrFoveation => "Foveated rendering",
            VrRefresh => "Headset refresh rate",
            VrGraphics => "Headset graphics",
            Vfx1 => "VFX1 headset on the desk",
            Vfx1Port => "  Card port (VIPPORT)",
            Vfx1Stereo => "  Stereo",
            Vfx1SwapEyes => "  Swap the eyes",
            Vfx1ViewSize => "  Picture size",
            Vfx1Steer => "  Head turns steer",
            Vfx1StereoBios => "  Shutter glasses' 3D BIOS",
            Filter => "Scaling filter",
            Shader => "CRT shader",
            CrtCurvature => "  Curvature",
            CrtGlow => "  Glow",
            Monochrome => "Monochrome monitor",
            MonitorEffects => "Monitor effects",
            Post => "BIOS POST screen",
            Composite => "CGA composite colour",
            CompositeEra => "  CGA revision",
            Cycles => "CPU speed (cycles)",
            AutoFps => "  Auto speed target",
            AutoBusyOs => "  Auto: busy Windows at full",
            Core => "CPU core",
            Fpu => "FPU arithmetic",
            Cpu => "Processor",
            Machine => "Video card",
            Memsize => "Memory",
            Voodoo => "3dfx Voodoo Graphics",
            VoodooMemory => "3dfx card",
            VoodooRenderer => "3dfx drawn by",
            VoodooScale => "3dfx OpenGL size",
            VoodooScaleShader => "  Sized for the CRT look",
            VoodooMsaa => "3dfx antialiasing",
            VoodooDither => "3dfx dithering",
            VoodooAnisotropy => "3dfx anisotropic filtering",
            VoodooTextureSampling => "3dfx texture sampling",
            VoodooFpsCap => "3dfx frame rate cap",
            VoodooGamma => "3dfx gamma",
            VoodooOverlay => "  Download Glide's DOS overlay...",
            PowerVr => "PowerVR card",
            PowerVrFilter => "PowerVR filtering",
            VeriteRisc => "Vérité RISC",
            VeriteMhz => "  RISC speed",
            VeriteRom => "  Board ROM",
            VeriteIrq => "  Vérité IRQ",
            Ems => "Expanded memory (EMS)",
            Umb => "Upper memory (UMB)",
            DosHigh => "DOS high",
            Dpmi => "DPMI host",
            DosVersion => "Reported DOS version",
            IdeHardDisks => "IDE hard disks (BOOT)",
            BootCdrom => "CD-ROM drive (BOOT)",
            FloppyController => "Floppy controller (BOOT)",
            KeyboardLayout => "Keyboard layout",
            MouseSensitivity => "Mouse sensitivity",
            MouseAutocapture => "Mouse auto capture",
            MouseCaptureMessages => "Mouse capture messages",
            MouseMenuHold => "Hold right button for settings",
            PauseInSettings => "Pause in settings window",
            ShellSuggestions => "Shell suggestions",
            ShellColors => "Shell colors",
            SaveShellHistory => "Save shell history",
            VersionCheck => "Check for new versions",
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
            Sc55Roms => "  Sound Canvas ROMs",
            Sc55Model => "  Sound Canvas model",
            Sc55Download => "  Download the Sound Canvas ROMs...",
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
            AudioBlocksize => "Sound buffer",
            AudioPrebuffer => "Sound prebuffer",
            LptDac => "Parallel port DAC",
            TandySound => "Tandy/PCjr sound",
            Pit2GateAtStart => "Timer 2 gate at startup",
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
            Phonebook => "Modem phonebook...",
            PrinterOutput => "Printer (LPT1)",
            PrinterPaper => "  Paper",
            PrinterDpi => "  Resolution",
            PrinterMultipage => "  Pages of a job",
            PrinterTimeout => "  Job ends after",
        }
    }

    /// Whether the frontend has what the setting needs.
    pub(super) fn available(self, frontend: Frontend) -> bool {
        match self {
            Item::Scale | Item::Fullscreen | Item::Vrr => frontend.window,
            // The window's OpenGL draws the scene.
            Item::VrMode
            | Item::VrScene
            | Item::VrControllers
            | Item::VrSpatialAudio
            | Item::VrScreenFit
            | Item::VrQuality
            | Item::VrAmbientOcclusion
            | Item::VrMsaa
            | Item::VrScreenGlow
            | Item::VrCenter
            | Item::VrSceneScale
            | Item::VrSeat(_)
            | Item::VrSeatTurn
            | Item::VrKeyboard
            | Item::VrMarkKeyboard
            | Item::VrTime
            | Item::VrTimeSpeed
            | Item::VrLatitude
            | Item::VrWeather
            | Item::VrWeatherSound
            | Item::VrLightning
            | Item::VrResolution
            | Item::VrFoveation
            | Item::VrRefresh
            | Item::VrGraphics
            | Item::Vfx1
            | Item::Vfx1Port
            | Item::Vfx1Stereo
            | Item::Vfx1SwapEyes
            | Item::Vfx1ViewSize
            | Item::Vfx1Steer
            | Item::Vfx1StereoBios => frontend.window && cfg!(feature = "vr"),
            Item::SoundFont => soundfonts(frontend),
            Item::Mt32Roms | Item::Mt32Model => mt32(frontend),
            Item::Sc55Roms | Item::Sc55Model => frontend.host_files,
            Item::Sc55Download | Item::VoodooOverlay => frontend.window && cfg!(all(feature = "sdl", not(target_arch = "wasm32"))),
            // The ROM is a host file; the program downloads it.
            Item::Awe32Rom | Item::VeriteRom => frontend.host_files,
            Item::Awe32Download => frontend.window && cfg!(all(feature = "sdl", not(target_arch = "wasm32"))),
            Item::MidiPort => host_midi(frontend),
            // The page records the canvas as it shows.
            Item::CaptureDir | Item::RecordUi | Item::RecordShader => frontend.host_files,
            Item::Core => crate::dynrec::AVAILABLE,
            // The browser draws with the emulator's own rasterizer only.
            Item::VoodooRenderer
            | Item::VoodooScale
            | Item::VoodooScaleShader
            | Item::VoodooMsaa
            | Item::VoodooDither
            | Item::VoodooAnisotropy => frontend.window,
            // Only the window plays the monitor's effects.
            Item::MonitorEffects | Item::Post => frontend.window,
            // A thread of its own packs rewind's states.
            Item::Rewind | Item::RewindMemory => frontend.window,
            // The frontend's own sound output has its own buffers.
            Item::AudioBlocksize | Item::AudioPrebuffer => frontend.window,
            // The browser has no sockets for a LAN.
            Item::Online | Item::Rooms | Item::Relay | Item::Player => frontend.window,
            Item::Lan | Item::LanHost | Item::Room | Item::Password => frontend.window,
            Item::ModemListen | Item::ModemTelnet | Item::Phonebook => frontend.window,
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
    pub(super) fn shown(self, s: &Settings) -> bool {
        match self {
            Item::CrtCurvature | Item::CrtGlow => s.shader == crate::video::shader::Shader::Crt,
            Item::VrScene
            | Item::VrSpatialAudio
            | Item::VrScreenFit
            | Item::VrQuality
            | Item::VrAmbientOcclusion
            | Item::VrMsaa
            | Item::VrScreenGlow
            | Item::VrTime
            | Item::VrTimeSpeed
            | Item::VrLatitude
            | Item::VrWeather => {
                s.vr.mode != crate::vr::VrMode::Off
            }
            Item::VrWeatherSound => s.vr.mode != crate::vr::VrMode::Off && s.vr.weather != crate::vr::VrWeather::Clear,
            Item::VrLightning => s.vr.mode != crate::vr::VrMode::Off && s.vr.weather == crate::vr::VrWeather::Storm,
            Item::VrControllers
            | Item::VrCenter
            | Item::VrSceneScale
            | Item::VrSeat(_)
            | Item::VrSeatTurn
            | Item::VrKeyboard
            | Item::VrResolution
            | Item::VrFoveation
            | Item::VrRefresh
            | Item::VrGraphics => s.vr.mode == crate::vr::VrMode::Headset,
            Item::VrMarkKeyboard => s.vr.mode == crate::vr::VrMode::Headset && s.vr.keyboard != crate::vr::VrKeyboard::Off,
            Item::Vfx1 => s.vr.mode == crate::vr::VrMode::Headset,
            Item::Vfx1Port
            | Item::Vfx1Stereo
            | Item::Vfx1SwapEyes
            | Item::Vfx1ViewSize
            | Item::Vfx1Steer
            | Item::Vfx1StereoBios => s.vr.mode == crate::vr::VrMode::Headset && s.vfx1.enabled,
            Item::ReverbMix => s.mixer.reverb != ReverbPreset::Off,
            Item::ChorusMix => s.mixer.chorus != ChorusPreset::Off,
            Item::RewindMemory => s.rewind,
            Item::AutoFps | Item::AutoBusyOs => matches!(s.cycles, CpuSpeed::Auto(_)),
            Item::VoodooMemory | Item::VoodooRenderer | Item::VoodooTextureSampling | Item::VoodooFpsCap | Item::VoodooGamma => s.voodoo.enabled,
            Item::PowerVrFilter => s.powervr.is_some(),
            Item::VeriteRisc | Item::VeriteIrq => s.machine.is_verite(),
            Item::VeriteMhz | Item::VeriteRom => {
                s.machine.is_verite() && s.verite.risc == crate::verite::Risc::Lle
            }
            Item::Awe32Rom | Item::Awe32Ram => awe32(s),
            // Until there is a ROM.
            Item::Awe32Download => awe32(s) && crate::awe32::rom::find(s.sound.awe32rom.as_deref()).is_none(),
            Item::Sc55Roms | Item::Sc55Model => s.sound.midisynth == MidiSynth::Sc55,
            // Until there are ROMs.
            Item::Sc55Download => s.sound.midisynth == MidiSynth::Sc55 && sc55_found(s).is_none(),
            // Until it is there.
            Item::VoodooOverlay => s.voodoo.enabled && crate::voodoo::overlay::find().is_none(),
            Item::VoodooScale | Item::VoodooMsaa | Item::VoodooDither | Item::VoodooAnisotropy => {
                s.voodoo.enabled && s.voodoo.renderer == crate::voodoo::Renderer::OpenGl
            }
            // At 1x the lines are the card's own, whatever the look.
            Item::VoodooScaleShader => {
                s.voodoo.enabled && s.voodoo.renderer == crate::voodoo::Renderer::OpenGl && s.voodoo.scale > 1
            }
            Item::Relay => s.network.online,
            Item::SerialIrq(n) => s.serial.ports[n as usize] != crate::serial::PortType::Off,
            Item::MouseType => s.serial.ports.contains(&crate::serial::PortType::Mouse),
            Item::ModemListen | Item::ModemTelnet | Item::Phonebook => s.serial.ports.contains(&crate::serial::PortType::Modem),
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

    pub(super) fn applies(self) -> Applies {
        use Item::*;
        match self {
            Scale | Fullscreen | Aspect | Vrr | Filter | Shader | CrtCurvature | CrtGlow | Composite | CompositeEra => {
                Applies::Now
            }
            Cycles | AutoFps | AutoBusyOs | Core | Fpu | Dpmi | DosVersion | IdeHardDisks | BootCdrom | FloppyController | KeyboardLayout | MouseAutocapture | MouseCaptureMessages | MouseMenuHold | PauseInSettings | ShellSuggestions | ShellColors | SaveShellHistory | Rewind | RewindMemory
            | VoodooRenderer | VoodooScale | VoodooScaleShader | VoodooMsaa | VoodooTextureSampling | VoodooAnisotropy | VoodooFpsCap | VoodooOverlay | PowerVrFilter => Applies::Now,
            VoodooDither => Applies::Now,
            Monochrome => Applies::NowAndAtPrompt,
            MonitorEffects => Applies::Now,
            Post => Applies::NextStart,
            HardDiskSpeed | FloppyDiskSpeed | HardDiskNoise | FloppyDiskNoise | Volume(_) | CaptureDir | RecordUi
            | RecordShader => Applies::Now,
            Joystick | Deadzone | MouseSensitivity | SpeakerFilter | SbFilter | Reverb | Chorus | ReverbMix | ChorusMix => Applies::Now,
            AudioBlocksize | AudioPrebuffer => Applies::Now,
            Rooms | Phonebook => Applies::Now,
            Memsize | Autoexec | Lan | LanHost | VersionCheck => Applies::NextStart,
            VrMode | VrQuality | VrAmbientOcclusion | VrMsaa | VrScene | VrControllers | VrSpatialAudio | VrScreenFit | VrScreenGlow
            | VrCenter | VrSceneScale | VrSeat(_) | VrSeatTurn | VrKeyboard | VrMarkKeyboard | VrResolution | VrRefresh => Applies::Now,
            VrTime | VrTimeSpeed | VrLatitude | VrWeather | VrWeatherSound | VrLightning => Applies::Now,
            // The card is hardware; how its pictures are shown is the host's.
            Vfx1Stereo | Vfx1SwapEyes | Vfx1ViewSize | Vfx1Steer | Vfx1StereoBios => Applies::Now,
            // SDL makes the window's context as the headset needs it as it
            // starts.
            VrGraphics | VrFoveation => Applies::NextStart,
            _ => Applies::AtPrompt,
        }
    }

    pub(super) fn input(self) -> Input {
        match self {
            Item::Volume(_) | Item::ReverbMix | Item::ChorusMix | Item::CrtCurvature | Item::CrtGlow | Item::VrScreenGlow => {
                Input::Slider
            }
            Item::Memsize | Item::Deadzone | Item::MouseSensitivity => Input::Slider,
            Item::VrSceneScale | Item::VrSeat(_) | Item::VrSeatTurn | Item::VrResolution => Input::Slider,
            Item::VrLatitude | Item::VrWeatherSound | Item::Vfx1ViewSize => Input::Slider,
            Item::Cycles | Item::AutoFps => Input::Presets,
            Item::UltraDir | Item::CaptureDir => Input::Text,
            Item::MacAddr | Item::Relay | Item::Player | Item::Lan | Item::LanHost | Item::Room | Item::Password => {
                Input::Text
            }
            Item::ModemListen => Input::Text,
            Item::SoundFont | Item::Mt32Roms | Item::Awe32Rom | Item::Sc55Roms | Item::VrScene => Input::File,
            Item::VeriteRom => Input::File,
            Item::Autoexec | Item::Rooms | Item::Phonebook | Item::Awe32Download | Item::Sc55Download | Item::VoodooOverlay | Item::VrCenter
            | Item::VrMarkKeyboard => {
                Input::Link
            }
            _ => Input::Choice,
        }
    }

    pub(super) fn value(self, s: &Settings, home: Option<&Path>) -> String {
        use Item::*;
        let (sb, gus) = (&s.sound.sb, &s.sound.gus);
        match self {
            Scale => format!("{}x", s.scale),
            Fullscreen => on_off(s.fullscreen),
            Aspect => s.aspect.name().to_string(),
            Vrr => on_off(s.vrr),
            VrMode => match s.vr.mode {
                crate::vr::VrMode::Off => "off",
                crate::vr::VrMode::Desktop => "3d in 2d window",
                crate::vr::VrMode::Headset => "VR headset",
            }
            .to_string(),
            VrScene => s.vr.scene.as_deref().map_or("the test room".to_string(), |p| contract_home(p, home)),
            VrControllers => s.vr.controllers.describe().to_string(),
            VrSpatialAudio => on_off(s.vr.spatial_audio),
            VrScreenFit => s.vr.screen_fit.describe().to_string(),
            VrQuality => s.vr.quality.describe().to_string(),
            VrAmbientOcclusion => match s.vr.ambient_occlusion {
                crate::vr::VrSwitch::Auto => "auto: as the lighting, off on a standalone headset",
                crate::vr::VrSwitch::On => "on",
                crate::vr::VrSwitch::Off => "off",
            }
            .to_string(),
            VrMsaa => s.vr.msaa.describe().to_string(),
            VrScreenGlow => {
                let glow = s.vr.screen_glow.min(crate::vr::SCREEN_GLOW_MAX) as u16;
                bar(format!("{:>3}%", glow), glow, crate::vr::SCREEN_GLOW_MAX as u16, SCREEN_GLOW_UNIT)
            }
            VrCenter | VrMarkKeyboard => String::new(),
            VrKeyboard => s.vr.keyboard.describe().to_string(),
            VrSceneScale => format!("{}%", s.vr.scene_scale),
            VrSeat(axis) => format!("{:+} cm", s.vr.seat[axis as usize]),
            VrSeatTurn => format!("{:+}°", s.vr.seat_turn),
            VrTime => match s.vr.time {
                crate::vr::VrTime::Now => "now (the computer's clock)".to_string(),
                time => time.name(),
            },
            VrTimeSpeed => describe_time_speed(s.vr.time_speed),
            VrLatitude => match s.vr.latitude {
                0 => "0° (the equator)".to_string(),
                l if l > 0 => format!("{}° north", l),
                l => format!("{}° south", -l),
            },
            VrWeather => s.vr.weather.describe().to_string(),
            VrWeatherSound => {
                let level = s.vr.weather_sound.min(crate::vr::WEATHER_SOUND_MAX) as u16;
                bar(format!("{:>3}%", level), level, crate::vr::WEATHER_SOUND_MAX as u16, 10)
            }
            VrLightning => on_off(s.vr.lightning),
            VrResolution => match s.vr.resolution {
                Some(percent) => format!("{}%", percent),
                None => "auto: adapts on a standalone headset".to_string(),
            },
            VrRefresh => match s.vr.refresh {
                Some(hz) => format!("{} Hz", hz),
                None => "auto: 72 Hz on a standalone headset".to_string(),
            },
            VrFoveation => s.vr.foveation.describe().to_string(),
            VrGraphics => s.vr.graphics.describe().to_string(),
            Vfx1 => on_off(s.vfx1.enabled),
            Vfx1Port => format!("{:X}h", s.vfx1.port),
            Vfx1Stereo => s.vfx1.stereo.describe().to_string(),
            Vfx1SwapEyes => on_off(s.vfx1.swap_eyes),
            Vfx1ViewSize => match s.vfx1.view_size {
                36 => "36° (as the VFX1)".to_string(),
                degrees => format!("{}°", degrees),
            },
            Vfx1Steer => on_off(s.vfx1.steer),
            Vfx1StereoBios => on_off(s.vfx1.stereo_bios),
            Filter => match s.filter {
                crate::config::Filter::Nearest => "nearest (sharp)",
                crate::config::Filter::Linear => "linear (smooth)",
            }
            .to_string(),
            Shader => s.shader.describe().to_string(),
            CrtCurvature => percent_bar(s.crt.curvature, MAX_AMOUNT),
            CrtGlow => percent_bar(s.crt.glow, MAX_AMOUNT),
            Monochrome => s.monochrome.describe().to_string(),
            MonitorEffects => s.monitor_effects.describe().to_string(),
            Post => s.post.describe().to_string(),
            Composite => s.composite.mode.describe().to_string(),
            CompositeEra => s.composite.era.describe().to_string(),
            Cycles => match s.cycles {
                CpuSpeed::Max => "max".to_string(),
                CpuSpeed::Fixed(n) => format!("{} per ms", n),
                CpuSpeed::Auto(n) if n == CpuSpeed::default().initial_cycles() => "auto".to_string(),
                CpuSpeed::Auto(n) => format!("auto (at least {})", n),
            },
            AutoFps => match s.auto_fps {
                None => "auto (display refresh)".to_string(),
                Some(n) => format!("{} fps", n),
            },
            AutoBusyOs => if s.auto_busy_os { "on" } else { "off" }.to_string(),
            Core => match s.core {
                CoreMode::Auto => "auto (recomp. in prot. mode)",
                CoreMode::Dynamic => "dynamic recompiler",
                CoreMode::Normal => "normal (interpreter)",
            }
            .to_string(),
            Fpu => if s.fpu_fast { "fast (not exact)" } else { "exact" }.to_string(),
            Cpu => s.cpu.describe().to_string(),
            Machine => s.machine.describe().to_string(),
            Memsize => memsize_bar(s.memsize, s.cpu),
            Voodoo => on_off(s.voodoo.enabled),
            VoodooMemory => s.voodoo.board.describe().to_string(),
            VoodooRenderer => match s.voodoo.renderer {
                crate::voodoo::Renderer::Software => "Rust-DOS".to_string(),
                crate::voodoo::Renderer::OpenGl => "OpenGL".to_string(),
            },
            VoodooScale => format!("{}x", s.voodoo.scale),
            VoodooTextureSampling => match s.voodoo.texture_sampling {
                crate::voodoo::TextureSampling::Default => "Default",
                crate::voodoo::TextureSampling::Unfiltered => "Unfiltered",
            }
            .to_string(),
            VoodooScaleShader => on_off(s.voodoo.scale_shader),
            VoodooDither => on_off(s.voodoo.dither),
            VoodooMsaa => match s.voodoo.msaa {
                1 => "Off".to_string(),
                n => format!("{}x MSAA", n),
            },
            VoodooAnisotropy => match s.voodoo.anisotropy {
                1 => "Off".to_string(),
                n => format!("{}x anisotropic", n),
            },
            VoodooFpsCap => match s.voodoo.fps_cap {
                None => "Off".to_string(),
                Some(n) => format!("{} fps", n),
            },
            VoodooGamma => match s.voodoo.gamma {
                None => "Off".to_string(),
                Some(g) => format!("{}", g),
            },
            PowerVr => match s.powervr {
                None => "Off",
                Some(crate::powervr::Chip::Pcx1) => "PCX1 (Apocalypse 3D)",
                Some(crate::powervr::Chip::Pcx2) => "PCX2 (m3D, Apocalypse 3Dx)",
            }
            .to_string(),
            VeriteRisc => match s.verite.risc {
                crate::verite::Risc::Lle => "Emulated (runs any microcode)",
                crate::verite::Risc::Hle => "Commands only (faster)",
            }
            .to_string(),
            VeriteMhz => format!("{} MHz", s.verite.mhz),
            VeriteRom => match &s.verite.rom {
                Some(path) if path.exists() => contract_home(path, home),
                Some(path) => format!("{} (missing)", contract_home(path, home)),
                None => "Built-in loader".to_string(),
            },
            VeriteIrq => s.verite.irq.to_string(),
            PowerVrFilter => match s.powervr_filter {
                crate::powervr::tsp::Filter::Auto => "As the game sets it",
                crate::powervr::tsp::Filter::Point => "Point (blocky)",
                crate::powervr::tsp::Filter::Bilinear => "Bilinear (smooth)",
            }
            .to_string(),
            Ems => on_off(s.ems),
            Umb => on_off(s.umb),
            DosHigh => on_off(s.dos_high),
            Dpmi => on_off(s.dpmi),
            DosVersion => s.dos_version.name(),
            IdeHardDisks => on_off(s.ide_hard_disks),
            BootCdrom => if s.boot_cdrom { "always" } else { "with a CD" }.to_string(),
            FloppyController => on_off(s.floppy_controller),
            KeyboardLayout => s.keyboard_layout.describe(),
            MouseSensitivity => bar(format!("{:.2}x", s.mouse_sensitivity), (s.mouse_sensitivity * 100.0).round() as u16, 1000, 50),
            MouseAutocapture => on_off(s.mouse_autocapture),
            MouseCaptureMessages => on_off(s.mouse_capture_messages),
            MouseMenuHold => on_off(s.mouse_menu_hold),
            PauseInSettings => on_off(s.pause_in_settings),
            ShellSuggestions => on_off(s.shell.autosuggest),
            ShellColors => on_off(s.shell.colors),
            SaveShellHistory => on_off(s.shell.save_history),
            VersionCheck => on_off(s.version_check),
            Rewind => on_off(s.rewind),
            RewindMemory => format!("{} MB", s.rewind_memory),
            SbType if !s.sound.sb_installed => "none".to_string(),
            SbType => match sb.model {
                SbModel::Sb16 => "SB16",
                SbModel::Awe32 => "SB AWE32",
                SbModel::SbPro2 => "SB Pro 2",
                SbModel::Sb2 => "SB 2.0",
                SbModel::Sb16Vibra => "SB16 ViBRA PnP",
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
                MidiSynth::Sc55 => "Sound Canvas",
                MidiSynth::Host => "host MIDI port",
                MidiSynth::None => "none",
            }
            .to_string(),
            SoundFont => s.sound.soundfont.as_deref().map_or("none".to_string(), |p| contract_home(p, home)),
            Mt32Roms => s.sound.mt32roms.as_deref().map_or("default".to_string(), |p| contract_home(p, home)),
            Mt32Model => s.sound.mt32model.describe().to_string(),
            Sc55Roms => {
                let found = sc55_found(s).map_or("none found".to_string(), |f| f.romset.display_name());
                match &s.sound.sc55roms {
                    Some(dir) => format!("{} ({})", contract_home(dir, home), found),
                    None => format!("default ({})", found),
                }
            }
            Sc55Model => match (s.sound.sc55model.as_str(), sc55_found(s)) {
                ("auto", Some(found)) => format!("auto ({})", found.romset.display_name()),
                ("auto", None) => "auto".to_string(),
                (model, _) => crate::sc55::rom::Romset::by_name(model).map_or(model.to_string(), |r| r.display_name()),
            },
            Sc55Download | VoodooOverlay => String::new(),
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
            AudioBlocksize => describe_blocksize(s.audio_buffer.blocksize),
            AudioPrebuffer => format!("{} ms", s.audio_buffer.prebuffer),
            LptDac => s.sound.lpt_dac.describe().to_string(),
            TandySound => s.sound.tandy.describe().to_string(),
            Pit2GateAtStart => on_off(s.sound.pit2_gate_at_start),
            Autoexec | Rooms | Phonebook => String::new(),
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
            ModemListen => s.serial.modem_listen.map_or("off".to_string(), crate::serial::describe_listen),
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
}

/// How fast the scene's day goes, in words.
fn describe_time_speed(speed: u32) -> String {
    match speed {
        0 => "stands still".to_string(),
        1 => "as the clock (real time)".to_string(),
        60 => "60x (an hour a minute)".to_string(),
        3600 => "3600x (an hour a second)".to_string(),
        n => format!("{}x", n),
    }
}
