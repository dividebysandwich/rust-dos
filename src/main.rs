use clap::Parser;
use sdl2::event::{Event, WindowEvent};
use sdl2::keyboard::{Keycode, Mod, Scancode};
use sdl2::mouse::{MouseButton, MouseWheelDirection};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::Duration;

use crate::audio::pump_audio;
use crate::config::Settings;
use crate::config_ui::osd::Osd;
use crate::config_ui::{ConfigUi, Host, UiKey};
use crate::cpu::{CoreMode, Cpu};
use crate::disk::{DriveInfo, DriveKind, LASTDRIVE};
use crate::display::Display;
use crate::mount::{MountCmd, MountSpec};
use crate::capture::avi::VideoRecorder;
use crate::clipboard::Clipboard;
use crate::capture::wav::WavWriter;
use crate::recorder::ScreenRecorder;
use crate::timer::CpuSpeed;

mod clipboard;
mod debug;
mod display;
mod sdl_keys;

// The emulator itself is the library crate; the debug server and the
// window's display are private to the binary. These re-exports let the
// binary's modules refer to the library modules as `crate::...`.
use rust_dos::{
    audio, capture, config, config_ui, cpu, disk, exec, games, joystick, keyboard, mount, recorder, shell, timer,
    video,
};
use rust_dos::achievements::{Achievements, http::HttpTransport};
use rust_dos::boot::{Startup, StartupItem};
use rust_dos::games::{ActiveGame, GameEntry, NewGame};
use rust_dos::hardware::Hardware;
use rust_dos::savestate::{self, slots};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Window scale factor [default: 1, or the config file's scale]
    #[arg(short, long, value_parser = clap::value_parser!(u32).range(1..=16))]
    scale: Option<u32>,

    /// Show each frame when the machine's display draws it, for a display
    /// with a variable refresh rate (G-Sync, FreeSync) [default: the
    /// config file's vrr]
    #[arg(long)]
    vrr: bool,

    /// Root directory for Drive C: [default: the config file's C:, or "."]
    #[arg(short, long)]
    dir: Option<String>,

    /// Configuration file to use instead of ./rust-dos.conf, the one next
    /// to the executable or the per-user default
    #[arg(short, long, value_name = "FILE", conflicts_with = "no_config")]
    config: Option<std::path::PathBuf>,

    /// Don't read or create any configuration file
    #[arg(long)]
    no_config: bool,

    /// Start at the DOS prompt, without booting the disk image that boots
    /// at startup (`-boot` in [drives])
    #[arg(long)]
    no_boot: bool,

    /// Start the HTTP/WebSocket debug server (local-only, unauthenticated).
    /// Optionally takes the listen address.
    #[arg(long, value_name = "ADDR", num_args = 0..=1, default_missing_value = "127.0.0.1:8086")]
    debug_server: Option<std::net::SocketAddr>,

    /// Instruction trace ring buffer capacity (entries, ~64 bytes each).
    /// Allocated only once tracing is enabled via the debug server.
    #[arg(long, default_value_t = 1_000_000)]
    trace_capacity: usize,

    /// Emulated CPU speed in instructions per millisecond, "max" for as
    /// fast as the host keeps up with, or "auto" for the speed the running
    /// program's frames show it needs (optionally with the least speed,
    /// "auto 5000") [default: auto, or the config file's cycles]
    #[arg(long, value_name = "N|max|auto", value_parser = timer::CpuSpeed::parse)]
    cycles: Option<timer::CpuSpeed>,

    /// What runs the programs' instructions: auto (the interpreter, and the
    /// dynamic recompiler for protected-mode programs), dynamic or normal
    /// [default: auto, or the config file's core]
    #[arg(long, value_name = "auto|dynamic|normal", value_parser = CoreMode::parse)]
    core: Option<CoreMode>,

    /// Launch a game at startup: a profile in the games folder beside the
    /// configuration file, by its file name or its name
    #[arg(long, value_name = "NAME", conflicts_with = "import")]
    game: Option<String>,

    /// Import a game as a game profile, and launch it: its package (a zip
    /// or 7z archive, or a folder with a rust-dos.conf), or a game set up
    /// for DOSBox (a GOG install's folder, a folder with DOSBox
    /// configuration files, or one of them)
    #[arg(long, value_name = "PATH")]
    import: Option<std::path::PathBuf>,

    /// Relay LAN rooms for other rust-dos instances on this UDP port,
    /// without starting the emulator
    #[arg(long, value_name = "PORT", num_args = 0..=1, default_missing_value = "21213")]
    relay: Option<u16>,

    /// One password for all the rooms of --relay
    #[arg(long, value_name = "PASSWORD", requires = "relay")]
    relay_password: Option<String>,
}

/// The SDL sound device, where the mixed output goes.
struct SdlAudio(sdl2::audio::AudioQueue<i16>);

/// The sound device, playing.
fn open_audio(sdl_context: &sdl2::Sdl) -> Result<sdl2::audio::AudioQueue<i16>, String> {
    let desired_spec = sdl2::audio::AudioSpecDesired {
        freq: Some(44100),
        channels: Some(2),
        // SDL's default is 2048 frames, 46 ms of latency.
        samples: Some(512),
    };
    let device = sdl_context.audio()?.open_queue::<i16, _>(None, &desired_spec)?;
    device.resume();
    Ok(device)
}

impl audio::AudioOutput for SdlAudio {
    fn queued_frames(&self) -> usize {
        self.0.size() as usize / 4
    }

    fn queue(&mut self, samples: &[i16]) -> Result<(), String> {
        self.0.queue_audio(samples)
    }

    /// The device takes its buffer from the queue at once; 20 ms more is
    /// for a video frame that comes late.
    fn target_frames(&self) -> usize {
        self.0.spec().samples as usize + self.0.spec().freq as usize / 50
    }
}

/// What saving the settings compares against: the settings and drives as
/// the configuration file has them, from startup or the last save. Saving
/// writes what changed since and adds the settings the file has no line
/// for, so the command-line options that weren't changed, drives that
/// failed to mount and the file's own spelling of paths stay as they are.
struct Saved {
    file: Option<PathBuf>,
    /// The `[autoexec]` lines, whose MOUNTs aren't copied to `[drives]`.
    autoexec: Vec<String>,
    settings: Settings,
    drives: BTreeMap<u8, MountSpec>,
}

fn main() -> Result<(), String> {
    let args = Args::parse();
    if let Some(port) = args.relay {
        let bind = std::net::SocketAddr::from(([0, 0, 0, 0], port));
        let name = "rust-dos relay".to_string();
        return rust_dos::net::tunnel::relay::serve(bind, rust_dos::net::tunnel::relay::RelayConfig {
            name,
            password: args.relay_password,
            port,
        });
    }
    let config = load_config(&args)?;
    let mut settings = Settings::from_config(&config);
    if let Some(scale) = args.scale {
        settings.scale = scale;
    }
    if args.vrr {
        settings.vrr = true;
    }
    if let Some(cycles) = args.cycles {
        settings.cycles = cycles;
    }
    if let Some(core) = args.core {
        settings.core = core;
    }
    // A game to launch: its profile, read now so its memory size is the
    // machine's.
    let imported = match &args.import {
        Some(source) => {
            let dir = games_dir(config.source.as_deref()).ok_or("--import needs a configuration file, beside which the games folder is")?;
            let (id, name, warnings) = games::import(&dir, source, rust_dos::hostdirs::home_dir().as_deref())?;
            println!("Imported {} as the game profile {}", name, dir.join(format!("{}.conf", id)).display());
            for warning in warnings {
                println!("  {}", warning);
            }
            Some(id)
        }
        None => None,
    };
    let startup_game = match args.game.as_ref().or(imported.as_ref()) {
        Some(query) => Some(find_game(config.source.as_deref(), query)?),
        None => None,
    };
    let memory_mb = match &startup_game {
        Some((entry, text, dir)) => {
            games::prepare(&entry.id, &settings, text, dir, rust_dos::hostdirs::home_dir().as_deref())?.settings.memsize
        }
        None => settings.memsize,
    };

    let mut cursor_visible = true;
    let mut last_blink = std::time::Instant::now();
    let blink_interval = Duration::from_millis(500);

    // Initialize Recorder
    // TODO: Make configurable
    let mut recorder = ScreenRecorder::new(15);

    // SDL2 Setup
    let sdl_context = sdl2::init()?;
    let video_subsystem = sdl_context.video()?;
    // Without a sound device (as in a virtual machine without a sound
    // card) the emulator runs silent.
    let audio_device = open_audio(&sdl_context);
    // Game controllers for the game port; the emulator does without them.
    let controller_subsystem = match sdl_context.game_controller() {
        Ok(subsystem) => Some(subsystem),
        Err(e) => {
            eprintln!("[INPUT] No game controllers: {}", e);
            None
        }
    };
    // The controllers plugged in, in the order they came.
    let mut controllers: Vec<sdl2::controller::GameController> = Vec::new();

    // The picture has the size the video mode gives it; the display scales
    // it to the window. The textures are for SDL's renderer, which draws
    // where there is no OpenGL 3.
    let textures = std::cell::OnceCell::new();
    let mut display = Display::open(&video_subsystem, "Rust DOS Emulator", &settings, &textures)?;
    // Typed text is only wanted in the settings window.
    let text_input = video_subsystem.text_input();
    text_input.stop();
    let host_clipboard = video_subsystem.clipboard();

    let mut cpu = create_cpu(&args, &config, memory_mb);
    apply_keyboard_layout(&mut cpu, settings.keyboard_layout);
    for warning in rust_dos::hardware::configure(&mut cpu, &settings, sdl_keys::detect_layout()) {
        config_warning(&mut cpu, &warning);
    }
    sync_locks(&mut cpu, sdl_context.keyboard().mod_state());
    for warning in cpu.bus.start_lan() {
        config_warning(&mut cpu, &warning);
    }
    match audio_device {
        Ok(device) => cpu.bus.audio_device = Some(Box::new(SdlAudio(device))),
        Err(e) => {
            eprintln!("[AUDIO] No sound device, so no sound: {}", e);
            cpu.bus.log_string(&format!("[AUDIO] No sound device, so no sound: {}", e));
        }
    }
    cpu.bus.log_string(&format!("[DISPLAY] {}", display.renderer()));
    if let Some(warning) = display.shader_warning() {
        config_warning(&mut cpu, warning);
    }
    let mut machine = Hardware::of(&settings);
    let mut saved = Saved {
        file: config.source.clone(),
        autoexec: config.autoexec.clone(),
        settings: settings.clone(),
        drives: mounted_drives(&cpu),
    };
    let mut dbg = match args.debug_server {
        Some(addr) => debug::DebugHub::start(&mut cpu, addr, args.trace_capacity)?,
        None => debug::DebugHub::disabled(),
    };
    let mut event_pump = sdl_context.event_pump()?;
    // RetroAchievements: the session with the site, and when the game's
    // logic is checked next (emulated PIT ticks, every 1/60 s). Hardcore
    // mode needs the debug server off, which could change memory.
    let mut achievements = Achievements::new(Box::new(HttpTransport::start()), settings.achievements.clone());
    achievements.hardcore_allowed = args.debug_server.is_none();
    let mut next_check = 0u64;

    // Startup commands: the config's [autoexec] lines, then AUTOEXEC.BAT
    // from the C: root if there is one, like the startup sequence a real PC
    // would run. Each line runs as if typed at the prompt. A reboot runs
    // them again.
    cpu.bus.config_dir = config_dir(config.source.as_deref());
    let mut commands = vec![StartupItem::Lines(config.autoexec.clone())];
    // A disk image that boots at startup boots after the [autoexec] lines,
    // which may mount more for it, in place of AUTOEXEC.BAT: the system on
    // the disk has its own.
    commands.push(match startup_boot(&mut cpu, args.no_boot || startup_game.is_some()) {
        Some(drive) => StartupItem::Lines(vec![format!("BOOT -l {}", disk::drive_key(drive))]),
        None => StartupItem::BatchFile("C:\\AUTOEXEC.BAT".to_string()),
    });
    cpu.startup = Startup { notes: banner_notes(&config, args.no_config), commands };
    cpu.start_dos();

    // Cached render target. We re-render the full VGA surface only when
    // `cpu.bus.vga.dirty` is set — everything else (cursor blink, mouse
    // cursor, recording indicator) is overlaid on top of this buffer each
    // frame. For a program that isn't actively touching VRAM, the per-SDL-
    // frame cost drops from "640×400×3 zero fill + per-pixel palette/planar
    // lookup" to "one memcpy of the cached buffer + a tiny overlay pass".
    let mut cached_frame = video::Frame::new(video::SCREEN_WIDTH, video::SCREEN_HEIGHT);
    // Whether the OpenGL renderer drew the 3dfx card's picture last frame.
    let mut voodoo_gl_shown = false;
    // The cached render with the cursors on top, as the screen shows it.
    let mut screen = cached_frame.clone();

    // Paces emulated time against the wall clock, one frame at a time.
    cpu.bus.set_cycles_per_ms(settings.cycles.initial_cycles());
    let mut pacer = timer::Pacer::new(settings.cycles, std::time::Instant::now());

    // The settings window (Ctrl+F12, DOSCONFIG), and whether the machine
    // has been set aside for it: paused, its keys released.
    let mut ui = ConfigUi::new();
    let mut ui_shown = false;
    // Keys pressed on the machine and not released yet: their scan codes
    // and whether they have the E0 prefix.
    let mut held: HashMap<Scancode, (u8, bool)> = HashMap::new();
    // What the hotkeys did, over the picture, and whether the machine is
    // paused (Alt+Pause).
    let mut osd = Osd::new();
    let mut paused = false;
    // The game launched from its profile and not ended yet.
    let mut game: Option<ActiveGame> = None;
    // The mouse captured for a program (Ctrl+Alt, Ctrl+F10, or a click once
    // it has the mouse driver): SDL's relative mode, whose motion keeps
    // coming at the window's edges.
    let sdl_mouse = sdl_context.mouse();
    let mut mouse_captured = false;
    // Ctrl+Alt on their own capture the mouse and let it go, as in VMware.
    let mut ctrl_alt = rust_dos::mouse_capture::CtrlAlt::default();
    // With `mouse_autocapture`, the mouse captured as it moves over the
    // window and let go where the program's cursor leaves the screen.
    let mut auto_capture = rust_dos::mouse_capture::AutoCapture::default();
    let mut focused = true;
    // The button whose click captured the mouse, whose release the program
    // doesn't see either.
    let mut capturing_click: Option<MouseButton> = None;
    // The text selected with the right button while the mouse isn't
    // captured (Ctrl+Shift+C copies it), and the text being pasted
    // (Ctrl+Shift+V).
    let mut clipboard = Clipboard::new();
    // A screenshot to take of the next frame (Ctrl+F5), and the sound being
    // recorded (Ctrl+F6).
    let mut screenshot = false;
    let mut sound_recording: Option<WavWriter> = None;
    // The video being recorded (Ctrl+F7).
    let mut video_recording: Option<VideoRecorder> = None;
    // The save states, in `states` beside the configuration file: the slot
    // Ctrl+F1 saves to and Ctrl+F2 loads (Ctrl+F3 picks another), and the
    // results of the threads writing them.
    let states_root = saved.file.as_deref().and_then(std::path::Path::parent).map(|dir| dir.join("states"))
        .or_else(|| config::user_dir().map(|dir| dir.join("states")));
    let mut slot: u8 = 1;
    let mut state_loaded = false;
    // Rewind (held Alt+F11): the states of the last minutes, which a thread
    // packs, when the next is due (emulated and wall time), the rewinding
    // going on (the emulated time it started at, and the frames since),
    // and the game and hardware the states are of.
    let rewinder = savestate::rewind::Rewinder::start(settings.rewind_memory << 20);
    let mut rewind_memory = settings.rewind_memory;
    let mut next_capture = (0u64, std::time::Instant::now());
    let mut rewinding: Option<(u64, u32)> = None;
    let mut rewind_of: (Option<String>, Hardware, bool) = (None, machine.clone(), settings.rewind);
    let (state_done, state_results) = std::sync::mpsc::channel::<Result<String, String>>();
    macro_rules! capture_mouse {
        ($on:expr) => {{
            let on = $on;
            if on != mouse_captured {
                sdl_mouse.set_relative_mouse_mode(on);
                mouse_captured = on;
            }
            auto_capture.captured &= on;
        }};
    }
    // What says the mouse was captured or let go, unless the settings
    // turn it off.
    macro_rules! capture_message {
        ($text:expr) => {
            if settings.mouse_capture_messages {
                osd.show($text);
            }
        };
    }
    // Let the mouse go with the host's pointer over the program's cursor,
    // put there while the pointer is still locked so that Wayland takes
    // it as where to leave it.
    macro_rules! release_mouse_at_cursor {
        () => {{
            let cursor = (cpu.bus.mouse.x, cpu.bus.mouse.y);
            display.warp_mouse(&sdl_mouse, video::overlay::mouse_to_frame(&cpu.bus, &cached_frame, cursor));
            capture_mouse!(false);
            auto_capture.released();
        }};
    }
    // Whether the program follows the mouse driver's cursor, which then
    // decides where the mouse leaves: not with the PS/2 or a serial mouse
    // read by the program itself.
    macro_rules! follows_cursor {
        () => {
            settings.mouse_autocapture
                && cpu.bus.mouse.in_use(cpu.bus.clock.now_ns())
                && !cpu.bus.mouse.ps2.enabled
                && !cpu.bus.serial.mouse_in_use()
        };
    }

    // What the settings window changes: the machine, the display and the
    // speed, and what saving writes.
    macro_rules! host {
        () => {
            MainHost {
                cpu: &mut cpu,
                display: &mut display,
                pacer: &mut pacer,
                settings: &mut settings,
                machine: &mut machine,
                saved: &mut saved,
                game: &mut game,
                states: &states_root,
                picture: &cached_frame,
                state_done: &state_done,
                slot: &mut slot,
                state_loaded: &mut state_loaded,
                achievements: &mut achievements,
            }
        };
    }
    macro_rules! toggle_ui {
        () => {
            if ui.is_open() {
                ui.close();
            } else {
                // While a game plays, F2 saves to its profile.
                let file = match &game {
                    Some(game) => games_dir(saved.file.as_deref()).map(|dir| dir.join(format!("{}.conf", game.id))),
                    None => saved.file.clone(),
                };
                let current = settings.clone();
                ui.open(&current, file, &host!());
            }
        };
    }
    if let Some((entry, text, dir)) = &startup_game {
        match host!().start_game(&entry.id, text, dir) {
            Ok(message) => osd.show(message),
            Err(e) => config_warning(&mut cpu, &e),
        }
    }

    // What the settings window's Stats page shows, and the last frame's
    // start and times for it.
    let mut stats = rust_dos::stats::Stats::new();
    let mut last_frame: Option<(std::time::Instant, rust_dos::stats::FrameTimes)> = None;

    // Main Loop
    'running: loop {
        let frame_start = std::time::Instant::now();
        if let Some((start, times)) = last_frame {
            stats.record(&cpu.bus, rust_dos::stats::FrameTimes { wall: frame_start - start, ..times });
        }
        for event in event_pump.poll_iter() {
            match event {
                Event::Quit { .. } => break 'running,
                // Losing the keyboard lets go of what it held.
                // Coming back, the host's keyboard may have another
                // layout and other locks.
                Event::Window { win_event: WindowEvent::FocusGained, .. } => {
                    sync_locks(&mut cpu, sdl_context.keyboard().mod_state());
                    apply_keyboard_layout(&mut cpu, settings.keyboard_layout);
                    focused = true;
                    auto_capture.armed = true;
                }
                // The pointer left the window: coming back, it captures
                // the mouse again.
                Event::Window { win_event: WindowEvent::Leave, .. } if !mouse_captured => auto_capture.armed = true,
                // Uncovered, resized or moved to another display: the
                // picture is drawn again.
                Event::Window {
                    win_event:
                        WindowEvent::Exposed
                        | WindowEvent::Shown
                        | WindowEvent::Restored
                        | WindowEvent::Maximized
                        | WindowEvent::Resized(..)
                        | WindowEvent::SizeChanged(..)
                        | WindowEvent::DisplayChanged(..),
                    ..
                } => display.redraw(),
                Event::Window { win_event: WindowEvent::FocusLost, .. } => {
                    release_input(&mut cpu, &mut held);
                    ctrl_alt.reset();
                    focused = false;
                    capture_mouse!(false);
                    if pacer.fast_forward() {
                        pacer.set_fast_forward(false, &cpu.bus.clock, std::time::Instant::now());
                        cpu.bus.mixer.fast_forward = false;
                        osd.clear_lasting();
                    }
                    if rewinding.take().is_some() {
                        pacer.rebase(&cpu.bus.clock, std::time::Instant::now());
                        osd.clear_lasting();
                    }
                }
                Event::KeyDown {
                    keycode: Some(keycode),
                    scancode,
                    keymod,
                    repeat,
                    ..
                } => {
                    // Ctrl+F12 opens and closes the settings window. (Not
                    // with Alt: AltGr can arrive as Ctrl+Alt.)
                    // Ctrl+Shift+F12 shows and hides the performance
                    // overlay.
                    let ctrl = keymod.intersects(Mod::LCTRLMOD | Mod::RCTRLMOD);
                    let alt = keymod.intersects(Mod::LALTMOD | Mod::RALTMOD);
                    let shift = keymod.intersects(Mod::LSHIFTMOD | Mod::RSHIFTMOD);
                    if !repeat {
                        ctrl_alt.key_down(chord_key(scancode));
                    }
                    if keycode == Keycode::F12 && ctrl && shift && !alt {
                        if !repeat && let Some(message) = ui.overlay_key() {
                            osd.show(message);
                        }
                        continue;
                    }
                    if keycode == Keycode::F12 && ctrl && !alt {
                        if !repeat {
                            toggle_ui!();
                        }
                        continue;
                    }
                    // Ctrl+Shift+M shows the running game's manuals over
                    // the picture, and hides them again.
                    if keycode == Keycode::M && ctrl && shift && !alt {
                        if !repeat {
                            if ui.is_open() {
                                ui.close();
                            } else {
                                toggle_ui!();
                                if let Err(e) = ui.show_manuals(&host!()) {
                                    ui.close();
                                    osd.show(e);
                                }
                            }
                        }
                        continue;
                    }
                    // Ctrl+Shift+C copies the selected text, or the whole
                    // text screen, to the host's clipboard, and
                    // Ctrl+Shift+V types the clipboard's text on the
                    // machine (or into the settings window's field).
                    if keycode == Keycode::C && ctrl && shift && !alt {
                        if !repeat && !ui.is_open() {
                            let selected = clipboard.text(&cpu.bus);
                            clipboard.clear();
                            osd.show(match selected {
                                Some(text) => match host_clipboard.set_clipboard_text(&text) {
                                    Ok(()) => format!("Copied {} lines to the clipboard", text.lines().count().max(1)),
                                    Err(e) => format!("The text can't be copied: {}", e),
                                },
                                None => "Nothing to copy: the screen shows graphics".to_string(),
                            });
                        }
                        continue;
                    }
                    if keycode == Keycode::V && ctrl && shift && !alt {
                        if !repeat {
                            match host_clipboard.clipboard_text() {
                                Ok(text) if text.is_empty() => osd.show("The clipboard has no text"),
                                Ok(text) if ui.is_open() => ui.text(&text.replace(['\r', '\n'], " "), &mut host!()),
                                Ok(text) => {
                                    let (typed, skipped) = clipboard.paste(&cpu.bus, &text);
                                    osd.show(if skipped > 0 {
                                        format!("Pasting {} characters ({} no key types)", typed, skipped)
                                    } else {
                                        format!("Pasting {} characters (a key stops it)", typed)
                                    });
                                }
                                Err(e) => osd.show(format!("The clipboard can't be read: {}", e)),
                            }
                        }
                        continue;
                    }
                    // Ctrl+F1 saves the machine to the current slot,
                    // Ctrl+F2 loads it, and Ctrl+F3 and Ctrl+Shift+F3 pick
                    // the next and the previous slot.
                    if matches!(keycode, Keycode::F1 | Keycode::F2 | Keycode::F3) && ctrl && !alt {
                        if repeat || ui.is_open() {
                            continue;
                        }
                        let current = slot;
                        if keycode == Keycode::F1 {
                            if let Err(e) = host!().save_slot(current) {
                                osd.show(format!("Slot {} can't be saved: {}", current, e));
                            }
                        } else if keycode == Keycode::F2 {
                            match host!().load_slot(current) {
                                Ok(header) => osd.show(format!("Loaded slot {}: {}", current, describe_state(&header))),
                                Err(e) => osd.show(format!("Slot {} can't be loaded: {}", current, e)),
                            }
                        } else {
                            let back = keymod.intersects(Mod::LSHIFTMOD | Mod::RSHIFTMOD);
                            slot = if back { (slot + slots::SLOTS - 2) % slots::SLOTS + 1 } else { slot % slots::SLOTS + 1 };
                            let header = host!().slot_dir().ok().and_then(|dir| slots::read_file_header(&slots::slot_path(&dir, slot)));
                            osd.show(match header {
                                Some(header) => format!("Slot {}: {}", slot, describe_state(&header)),
                                None => format!("Slot {}: empty", slot),
                            });
                        }
                        continue;
                    }
                    // Ctrl+F9 opens the settings window on the save states.
                    if keycode == Keycode::F9 && ctrl && !alt {
                        if !repeat {
                            if !ui.is_open() {
                                toggle_ui!();
                            }
                            ui.show_states(&host!());
                        }
                        continue;
                    }
                    // Ctrl+F4 puts the next disk in the drives mounted
                    // from lists of images, as in DOSBox.
                    if keycode == Keycode::F4 && ctrl && !alt {
                        if !repeat {
                            let messages = cpu.bus.swap_images();
                            for message in &messages {
                                eprintln!("[DISK] {}", message);
                                cpu.bus.log_string(&format!("[DISK] {}", message));
                            }
                            if ui.is_open() && !messages.is_empty() {
                                ui.drives_changed(&host!(), &messages.join("; "));
                            }
                        }
                        continue;
                    }
                    // Alt+Pause pauses the machine and resumes it, as in
                    // DOSBox.
                    if keycode == Keycode::Pause && alt && !ctrl {
                        if !repeat {
                            paused = !paused;
                            if paused {
                                release_input(&mut cpu, &mut held);
                                capture_mouse!(false);
                                osd.show_lasting("Paused (Alt+Pause resumes)");
                            } else {
                                osd.clear_lasting();
                            }
                        }
                        continue;
                    }
                    // Ctrl+F11 slows the CPU down by a tenth and
                    // Ctrl+Shift+F11 speeds it up, but for in the settings
                    // window, which has the speed on its Emulator page.
                    if keycode == Keycode::F11 && ctrl && !alt && !ui.is_open() {
                        let faster = keymod.intersects(Mod::LSHIFTMOD | Mod::RSHIFTMOD);
                        let mut new = settings.clone();
                        new.cycles = settings.cycles.stepped(cpu.bus.clock.cycles_per_ms(), faster);
                        let _ = host!().apply(&new);
                        osd.show(speed_message(new.cycles));
                        continue;
                    }
                    // Holding Alt+F11 goes back in time, a step every
                    // third frame, until F11 comes up.
                    if keycode == Keycode::F11 && alt && !ctrl {
                        if !repeat && !paused && !ui.is_open() && rewinding.is_none() {
                            if achievements.hardcore_active() {
                                osd.show("Hardcore mode: no rewind while RetroAchievements plays");
                            } else if settings.rewind {
                                release_input(&mut cpu, &mut held);
                                let now = cpu.bus.clock.now_ns();
                                rewinder.push(now, savestate::machine::save(&cpu));
                                rewinding = Some((now, 0));
                                osd.show_lasting("Rewind");
                            } else {
                                osd.show("Rewind is off: the settings window's Emulator page turns it on");
                            }
                        }
                        continue;
                    }
                    // Holding Alt+F12 runs the machine fast, as in DOSBox
                    // Staging, until F12 comes up.
                    if keycode == Keycode::F12 && alt && !ctrl {
                        if !repeat && !paused {
                            pacer.set_fast_forward(true, &cpu.bus.clock, std::time::Instant::now());
                            cpu.bus.mixer.fast_forward = true;
                            osd.show_lasting("Fast forward");
                        }
                        continue;
                    }
                    // Ctrl+F10 captures the mouse and lets it go too, as in
                    // DOSBox.
                    if keycode == Keycode::F10 && ctrl && !alt {
                        if !repeat && !ui.is_open() && !paused {
                            capture_mouse!(!mouse_captured);
                            if !mouse_captured {
                                auto_capture.released();
                            }
                            capture_message!(if mouse_captured { CAPTURED } else { "Mouse released" });
                        }
                        continue;
                    }
                    // Alt+Enter switches between the window and fullscreen.
                    if keycode == Keycode::Return && alt && !ctrl {
                        if !repeat && !ui.is_open() {
                            let mut new = settings.clone();
                            new.fullscreen = !new.fullscreen;
                            if let Err(e) = host!().apply(&new) {
                                osd.show(e);
                            }
                        }
                        continue;
                    }
                    // Ctrl+Shift+F5 ejects the printer's page and ends the
                    // print job.
                    if keycode == Keycode::F5 && ctrl && shift && !alt {
                        if !repeat {
                            osd.show(cpu.bus.printer_eject().unwrap_or_else(|| "There is no printer".to_string()));
                        }
                        continue;
                    }
                    // Ctrl+F5 saves a screenshot, and Ctrl+F6 starts and stops
                    // recording the sound, as in DOSBox.
                    if keycode == Keycode::F5 && ctrl && !alt {
                        if !repeat {
                            screenshot = true;
                        }
                        continue;
                    }
                    if keycode == Keycode::F6 && ctrl && !alt {
                        if !repeat {
                            match sound_recording.take() {
                                Some(wav) => match wav.finish() {
                                    Ok(seconds) => osd.show(format!("Sound recording stopped ({:.1} s)", seconds)),
                                    Err(e) => osd.show(format!("The sound recording failed: {}", e)),
                                },
                                None => match capture::capture_path(&settings.capture_dir, "sound", "wav")
                                    .and_then(|path| WavWriter::create(&path).map(|wav| (path, wav)))
                                {
                                    Ok((path, wav)) => {
                                        sound_recording = Some(wav);
                                        osd.show(format!("Recording the sound to {}", path.display()));
                                    }
                                    Err(e) => osd.show(e),
                                },
                            }
                        }
                        continue;
                    }
                    // Ctrl+F7 starts and stops recording video with sound.
                    if keycode == Keycode::F7 && ctrl && !alt {
                        if !repeat {
                            match video_recording.take() {
                                Some(video) => match video.stop() {
                                    Ok(frames) => osd.show(format!("Video recording stopped ({} frames)", frames)),
                                    Err(e) => osd.show(format!("The video recording failed: {}", e)),
                                },
                                None => {
                                    // Through the shader, at the size the
                                    // window shows the picture.
                                    let (w, h) = (cached_frame.width, cached_frame.height);
                                    let shaded = settings.record_shader.then(|| display.shaded_size(w, h)).flatten();
                                    let (w, h) = shaded.unwrap_or((w, h));
                                    let (w, h) = (w as usize, h as usize);
                                    let now = cpu.bus.clock.now_ns();
                                    match capture::capture_path(&settings.capture_dir, "video", "avi")
                                        .and_then(|path| VideoRecorder::start(&path, w, h, now).map(|v| (path, v)))
                                    {
                                        Ok((path, video)) => {
                                            video_recording = Some(video);
                                            osd.show(format!("Recording video to {}", path.display()));
                                        }
                                        Err(e) => osd.show(e),
                                    }
                                }
                            }
                        }
                        continue;
                    }
                    // Ctrl+F8 turns the sound off and on.
                    if keycode == Keycode::F8 && ctrl && !alt {
                        if !repeat {
                            cpu.bus.mixer.muted = !cpu.bus.mixer.muted;
                            osd.show(if cpu.bus.mixer.muted { "Sound off (Ctrl+F8)" } else { "Sound on" });
                        }
                        continue;
                    }
                    if ui.is_open() {
                        if let Some(key) = ui_key(keycode, keymod) {
                            ui.key(key, &mut host!());
                        }
                        continue;
                    }
                    // The paused machine takes no keys.
                    if paused {
                        continue;
                    }
                    // A key still held from the settings window repeats
                    // for nobody.
                    let Some(scancode) = scancode else { continue };
                    if repeat && !held.contains_key(&scancode) {
                        continue;
                    }
                    // A key pressed stops a paste.
                    if clipboard.pasting() {
                        clipboard.stop_paste(&mut cpu);
                        osd.show("Paste stopped");
                    }

                    // Recorder Toggle
                    if keycode == Keycode::PrintScreen {
                        match recorder.toggle(&settings.capture_dir) {
                            Ok(Some(path)) => osd.show(format!("Recording an animation to {}", path.display())),
                            Ok(None) => osd.show("Animation recording stopped"),
                            Err(e) => osd.show(e),
                        }
                        continue;
                    }

                    // The PC key where the host's key is: its scan code
                    // for programs that read the keyboard themselves, and
                    // what the keyboard layout types with it for the BIOS.
                    if let Some((scan, extended)) = sdl_keys::pc_scan(scancode) {
                        keyboard::key_event(&mut cpu.bus, scan, extended, true, None);
                        held.insert(scancode, (scan, extended));
                    }
                }
                Event::KeyUp {
                    keycode: Some(keycode),
                    scancode,
                    ..
                } => {
                    // Ctrl+Alt: their keys still come up for the machine,
                    // which saw them go down.
                    if ctrl_alt.key_up(chord_key(scancode)) && !ui.is_open() && !paused {
                        capture_mouse!(!mouse_captured);
                        if !mouse_captured {
                            auto_capture.released();
                        }
                        capture_message!(if mouse_captured { CAPTURED } else { "Mouse released (Ctrl+Alt captures it)" });
                    }
                    if keycode == Keycode::F12 && pacer.fast_forward() {
                        pacer.set_fast_forward(false, &cpu.bus.clock, std::time::Instant::now());
                        cpu.bus.mixer.fast_forward = false;
                        osd.clear_lasting();
                        continue;
                    }
                    // The machine goes on from where rewinding got to.
                    if keycode == Keycode::F11 && rewinding.take().is_some() {
                        // The game's achievements wait for their conditions
                        // to be false again.
                        achievements.reset();
                        pacer.rebase(&cpu.bus.clock, std::time::Instant::now());
                        osd.clear_lasting();
                        next_capture = (cpu.bus.clock.now_ns() + REWIND_INTERVAL_NS, std::time::Instant::now());
                        continue;
                    }
                    // Only keys the machine saw go down come up for it,
                    // not those of the settings window.
                    let Some((scan, extended)) = scancode.and_then(|scancode| held.remove(&scancode)) else {
                        continue;
                    };
                    // Games that track held keys (arrow-key movement, etc.)
                    // need the break code to know when the key stops being
                    // pressed.
                    keyboard::key_event(&mut cpu.bus, scan, extended, false, None);
                }

                Event::TextInput { text, .. } if ui.is_open() => ui.text(&text, &mut host!()),

                // A file or folder dropped onto the window.
                Event::DropFile { filename, .. } => {
                    let message = host!().dropped(std::path::Path::new(&filename));
                    if ui.is_open() {
                        ui.drives_changed(&host!(), &message);
                    }
                    osd.show(message);
                }

                // Game controllers come and go; the first two are the
                // game port's.
                Event::ControllerDeviceAdded { which, .. } => {
                    let Some(subsystem) = &controller_subsystem else { continue };
                    match subsystem.open(which) {
                        Ok(pad) if controllers.iter().all(|c| c.instance_id() != pad.instance_id()) => {
                            cpu.bus.log_string(&format!("[INPUT] Game controller: {}", pad.name()));
                            osd.show(format!("Game controller: {}", pad.name()));
                            controllers.push(pad);
                        }
                        Ok(_) => {}
                        Err(e) => cpu.bus.log_string(&format!("[INPUT] Can't open game controller {}: {}", which, e)),
                    }
                }
                Event::ControllerDeviceRemoved { which, .. } => {
                    if let Some(i) = controllers.iter().position(|c| c.instance_id() == which) {
                        osd.show(format!("Game controller unplugged: {}", controllers[i].name()));
                        controllers.remove(i);
                    }
                }

                Event::MouseButtonDown { mouse_btn: MouseButton::Left, x, y, .. } if ui.is_open() => {
                    let (fx, fy) = display.to_frame(x, y);
                    ui.click(fx, fy, &mut host!());
                }

                Event::MouseWheel { y, direction, .. } if ui.is_open() => {
                    let dy = if matches!(direction, MouseWheelDirection::Flipped) { -y } else { y };
                    ui.wheel(dy, &mut host!());
                }

                // The right button dragged over a text screen while the
                // mouse isn't captured selects text; a click without a
                // drag is a click.
                Event::MouseButtonDown { mouse_btn: MouseButton::Right, x, y, .. }
                    if !ui.is_open() && !mouse_captured && clipboard.start(&cpu.bus, display.to_frame(x, y)) => {}
                Event::MouseMotion { x, y, .. } if clipboard.dragging() => clipboard.drag(&cpu.bus, display.to_frame(x, y)),
                Event::MouseButtonUp { mouse_btn: MouseButton::Right, x, y, .. } if clipboard.dragging() => {
                    if clipboard.finish(&cpu.bus, display.to_frame(x, y)) {
                        osd.show("Ctrl+Shift+C copies the selected text");
                    } else if !ui.is_open() && !paused {
                        if cpu.bus.mouse.installed || cpu.bus.mouse.ps2.enabled || cpu.bus.serial.mouse_in_use() {
                            capture_mouse!(true);
                            capture_message!(CAPTURED);
                        } else {
                            let (vx, vy) = video::overlay::frame_to_mouse(&cpu.bus, &cached_frame, display.to_frame(x, y));
                            cpu.bus.mouse.set_position(vx, vy);
                            cpu.bus.mouse.button_down(1);
                            cpu.bus.mouse.button_up(1);
                        }
                    }
                }

                // The machine's mouse is still while the window is open or
                // the machine paused.
                Event::MouseMotion { .. } | Event::MouseButtonDown { .. } | Event::MouseButtonUp { .. }
                    if ui.is_open() || paused => {}

                // Captured, the mouse moves the driver's cursor by its
                // motion; otherwise to where it is on the picture.
                Event::MouseMotion { x, y, xrel, yrel, .. } => {
                    if mouse_captured {
                        let (sx, sy) = display.frame_scale();
                        let frame_motion = (xrel as f64 * sx, yrel as f64 * sy);
                        let (dx, dy) = video::overlay::frame_motion_to_mouse(&cpu.bus, &cached_frame, frame_motion);
                        // The program's cursor pushed out of the screen
                        // takes the host's pointer out with it, unless the
                        // program steers with the mouse's motion or the
                        // window fills the screen.
                        if follows_cursor!() && !settings.fullscreen && !cpu.bus.mouse.moves_by_motion(cpu.bus.clock.now_ns()) {
                            let (screen, cursor) = (cpu.bus.mouse.virtual_screen(&cpu.bus), (cpu.bus.mouse.x, cpu.bus.mouse.y));
                            if auto_capture.motion(cursor, screen, (dx, dy)).is_some() {
                                cpu.bus.mouse.move_by(dx, dy);
                                release_mouse_at_cursor!();
                                continue;
                            }
                        }
                        cpu.bus.mouse.move_by(dx, dy);
                        // Captured by itself, the mouse goes once the
                        // program no longer uses it.
                        if auto_capture.captured && !follows_cursor!() {
                            release_mouse_at_cursor!();
                        }
                    } else {
                        let (vx, vy) = video::overlay::frame_to_mouse(&cpu.bus, &cached_frame, display.to_frame(x, y));
                        cpu.bus.mouse.set_position(vx, vy);
                        // Moving over the window captures it for a program
                        // that uses the mouse, with its cursor where the
                        // host's pointer came in.
                        if auto_capture.armed && focused && follows_cursor!() {
                            capture_mouse!(true);
                            auto_capture.captured = true;
                            capture_message!(CAPTURED);
                        }
                    }
                }

                Event::MouseButtonDown { mouse_btn, x, y, .. } => {
                    clipboard.clear();
                    // A program using the mouse (the INT 33h driver, or the
                    // BIOS's PS/2 mouse as Windows does) gets it captured by
                    // a click, which it doesn't see, as in DOSBox.
                    if !mouse_captured && (cpu.bus.mouse.installed || cpu.bus.mouse.ps2.enabled || cpu.bus.serial.mouse_in_use()) {
                        capture_mouse!(true);
                        capturing_click = Some(mouse_btn);
                        capture_message!(CAPTURED);
                        continue;
                    }
                    if !mouse_captured {
                        let (vx, vy) = video::overlay::frame_to_mouse(&cpu.bus, &cached_frame, display.to_frame(x, y));
                        cpu.bus.mouse.set_position(vx, vy);
                    }
                    if let Some(btn) = sdl_button_to_index(mouse_btn) {
                        cpu.bus.mouse.button_down(btn);
                    }
                }

                Event::MouseButtonUp { mouse_btn, x, y, .. } => {
                    if capturing_click == Some(mouse_btn) {
                        capturing_click = None;
                        continue;
                    }
                    if !mouse_captured {
                        let (vx, vy) = video::overlay::frame_to_mouse(&cpu.bus, &cached_frame, display.to_frame(x, y));
                        cpu.bus.mouse.set_position(vx, vy);
                    }
                    if let Some(btn) = sdl_button_to_index(mouse_btn) {
                        cpu.bus.mouse.button_up(btn);
                    }
                }

                _ => {}
            }
        }

        // Remote debug requests and queued remote input, which goes to the
        // settings window while it is open.
        dbg.divert = ui.is_open();
        dbg.poll(&mut cpu);
        if dbg.take_hotkey() {
            toggle_ui!();
        }
        for request in dbg.take_state_requests() {
            let path = request.path.clone();
            if request.load {
                let loaded = host!().load_file(&path);
                request.done(loaded.map(|header| serde_json::json!({"loaded": path, "header": header})));
            } else {
                let saved = host!().save_file(&path);
                request.done(saved.map(|()| serde_json::json!({"saved": path})));
            }
        }
        for request in dbg.take_speed_requests() {
            let result = CpuSpeed::parse(&request.cycles).and_then(|cycles| {
                let new = Settings { cycles, ..settings.clone() };
                host!().apply(&new)?;
                Ok(serde_json::json!({"cycles": cycles.to_string()}))
            });
            request.done(result);
        }
        for input in dbg.take_ui_input() {
            match input {
                debug::UiInput::Key(key) => ui.key(key, &mut host!()),
                debug::UiInput::Click(x, y) => ui.click(x, y, &mut host!()),
            }
        }

        // A save state loaded: the keys held go up, and a video recording
        // stops, as its time would jump.
        if std::mem::take(&mut state_loaded) {
            rewinder.clear();
            achievements.reset();
            release_input(&mut cpu, &mut held);
            dbg.release_keys(&mut cpu);
            clipboard.stop_paste(&mut cpu);
            if let Some(video) = video_recording.take() {
                match video.stop() {
                    Ok(frames) => osd.show(format!("The video recording stopped at the load ({} frames)", frames)),
                    Err(e) => osd.show(format!("The video recording failed: {}", e)),
                }
            }
        }

        // The window opening or closing: it takes the keyboard and mouse
        // from the machine, which pauses but for the Mixer page, or while in
        // a LAN room (see the batch below).
        if ui.is_open() != ui_shown {
            ui_shown = ui.is_open();
            if ui_shown {
                release_input(&mut cpu, &mut held);
                clipboard.stop_paste(&mut cpu);
                clipboard.clear();
                capture_mouse!(false);
                dbg.release_keys(&mut cpu);
                text_input.start();
            } else {
                text_input.stop();
                // Caps and Num Lock may have changed while the window had
                // the keys.
                sync_locks(&mut cpu, sdl_context.keyboard().mod_state());
            }
        }

        // Run the emulated machine up to the wall clock. Emulated time is
        // counted in instructions (see timer.rs), so timer interrupts land on
        // the right instructions however the work is batched between frames.
        let batch_start = std::time::Instant::now();
        // In a LAN room the machine plays on whatever the window shows:
        // the others' games would wait for it, or drop it.
        let ui_pauses = ui.pauses_machine() && cpu.bus.net.status().joined().is_none();
        let waiting = dbg.paused || ui_pauses || paused || rewinding.is_some();
        // Pasted keys go once the keys held are up: the Ctrl and Shift of
        // Ctrl+Shift+V would change them.
        if !waiting && !ui.is_open() && held.is_empty() {
            clipboard.feed(&mut cpu);
        }
        // The values frozen on the Cheats page, as the program left them.
        cpu.bus.apply_freezes();
        // The controllers as they are now; at rest while the machine waits.
        for slot in 0..2 {
            let pad = controllers.get(slot).map(|pad| if waiting { joystick::PadState::default() } else { pad_state(pad) });
            cpu.bus.joystick.set_pad(slot, pad);
        }
        // With a variable refresh rate, each frame runs to a vertical
        // retrace of the machine's display and is shown when it is due,
        // so the window refreshes at the machine's rate, where the host's
        // display goes that fast.
        let refresh = (settings.vrr && !waiting)
            .then(|| cpu.bus.refresh_timing())
            .filter(|timing| display.shows_hz(timing.hz()));
        let batch_end = if waiting {
            cpu.bus.clock.icount
        } else if let Some(refresh) = &refresh {
            pacer.retrace_batch_end(&cpu.bus.clock, refresh, batch_start)
        } else {
            pacer.batch_end(&cpu.bus.clock, batch_start)
        };
        cpu.bus.start_batch(batch_end);
        let batch_icount = cpu.bus.clock.icount;
        let batch_stalled = cpu.bus.clock.stalled;
        let batch_idle = cpu.bus.clock.idle;
        // The instructions the dynamic recompiler's code runs, of all.
        let batch_translated = cpu.dynrec.counts().executed;

        // Per-instruction debug hook (breakpoints / stepping / tracing) is
        // only consulted when something actually needs it.
        let dbg_hot = dbg.begin_batch(&cpu);
        if achievements.checking() && !waiting {
            // A game's achievements are checked every 1/60 s of emulated
            // time, as in the emulators the sets are made with, fast
            // forwarding or not: the batch runs to each check.
            loop {
                cpu.bus.start_batch(batch_end.min(cpu.bus.clock.icount_at(next_check)));
                let reason = exec::run_batch(&mut cpu, &mut dbg, dbg_hot);
                let now = cpu.bus.clock.now_ticks();
                if now >= next_check {
                    achievements.do_frame(cpu.bus.ram(), cpu.bus.boot.is_some());
                    let frame = timer::frame_ticks();
                    next_check = if next_check + frame <= now { now + frame } else { next_check + frame };
                }
                if reason != exec::StopReason::BatchEnd || cpu.bus.clock.icount >= batch_end {
                    break;
                }
            }
        } else {
            exec::run_batch(&mut cpu, &mut dbg, dbg_hot);
        }
        dbg.end_batch(&cpu);
        let translated = cpu.dynrec.counts().executed.saturating_sub(batch_translated);

        // Rewind: its states start over for another game or other
        // hardware, and go when it is turned off; a state is taken every
        // half second the machine runs, and while Alt+F11 is held, the
        // machine goes a state back every third frame.
        if rewind_of.0.as_deref() != game.as_ref().map(|g| g.id.as_str()) || rewind_of.1 != machine || rewind_of.2 != settings.rewind {
            rewinder.clear();
            rewind_of = (game.as_ref().map(|g| g.id.clone()), machine.clone(), settings.rewind);
        }
        if settings.rewind_memory != rewind_memory {
            rewind_memory = settings.rewind_memory;
            rewinder.set_budget(rewind_memory << 20);
        }
        let now_ns = cpu.bus.clock.now_ns();
        if settings.rewind && !waiting && now_ns >= next_capture.0 && next_capture.1.elapsed() >= Duration::from_millis(250) {
            rewinder.offer(now_ns, savestate::machine::save(&cpu));
            next_capture = (now_ns + REWIND_INTERVAL_NS, std::time::Instant::now());
        }
        if let Some((started, frames)) = &mut rewinding {
            *frames += 1;
            if *frames % 3 == 1 {
                let back = |at: u64| (started.saturating_sub(at)) as f64 / 1e9;
                match rewinder.step_back() {
                    Some((at, state)) => match savestate::machine::load(&mut cpu, &state) {
                        Ok(()) => osd.show_lasting(format!("Rewind -{:.1} s", back(at))),
                        Err(e) => osd.show_lasting(format!("Rewind: {}", e)),
                    },
                    None => osd.show_lasting(format!("Rewind -{:.1} s: as far back as it goes", back(cpu.bus.clock.now_ns()))),
                }
            }
        }

        let exec_time = batch_start.elapsed();
        let stalled = cpu.bus.clock.stalled - batch_stalled;
        let idle = cpu.bus.clock.idle - batch_idle;
        let executed = cpu.bus.clock.icount - batch_icount - idle - stalled;
        dbg.record_batch(
            executed,
            exec_time,
            cpu.decode_cache.hits,
            cpu.decode_cache.misses,
        );
        // EXIT turns the machine off.
        if cpu.bus.exit_requested {
            break 'running;
        }
        // Pages printed, and printing that failed.
        for notice in cpu.bus.printer_notices() {
            osd.show(notice);
        }
        for notice in std::mem::take(&mut cpu.bus.disk_notices) {
            osd.show(notice);
        }

        // DOSCONFIG asks for the settings window.
        if std::mem::take(&mut cpu.bus.config_ui_requested) && !ui.is_open() {
            toggle_ui!();
        }
        // MIXER changed the mixer: the settings have it, to show and save.
        if std::mem::take(&mut cpu.bus.mixer_changed) {
            settings.mixer = cpu.bus.mixer.settings();
            ui.sync_mixer(settings.mixer);
        }
        // A launched game that has ended: the settings and drives before it
        // come back.
        if !ui.is_open()
            && let Some(ended) = game.take_if(|g| g.done(&cpu))
        {
            let name = ended.name.clone();
            host!().end_game(ended);
            osd.show(format!("{} has ended: your settings are back", name));
        }
        if let Some(notice) = ui.take_notice() {
            osd.show(notice);
        }
        // What happened on the LAN.
        for notice in cpu.bus.net.take_notices() {
            cpu.bus.log_string(&format!("[LAN] {}", notice));
            osd.show(notice);
        }
        // RetroAchievements: the site's answers, what to tell the player,
        // the leaderboards being attempted, and a token to keep.
        achievements.poll();
        for notice in achievements.take_notices() {
            cpu.bus.log_string(&format!("[ACHIEVEMENTS] {}: {}", notice.title, notice.detail));
            osd.notify(notice.title, notice.detail, notice.big);
        }
        osd.set_corner(achievements.trackers());
        if let Some((username, token)) = achievements.take_new_token() {
            ui.sync_achievements(&username, &token);
            if let Err(e) = host!().keep_token(&username, &token) {
                osd.show(format!("The RetroAchievements login can't be saved: {}", e));
            }
        }
        // No cheats in hardcore mode.
        if achievements.hardcore_active() && !cpu.bus.freezes.is_empty() {
            cpu.bus.freezes.clear();
            osd.show("Hardcore mode: the frozen values are free again");
        }
        // Save states written.
        for result in state_results.try_iter() {
            match result {
                Ok(message) => osd.show(message),
                Err(e) => {
                    cpu.bus.log_string(&format!("[STATE] {}", e));
                    osd.show(format!("The state can't be saved: {}", e));
                }
            }
        }
        // Processor and sound changes wait for the running program to end.
        if !ui.is_open() && cpu.shell_idle() && machine.differs(&settings) {
            for warning in machine.apply(&mut cpu, &settings) {
                config_warning(&mut cpu, &warning);
            }
        }

        // Update Audio
        let samples = pump_audio(&mut cpu.bus, waiting);
        if let Some(wav) = &mut sound_recording
            && let Err(e) = wav.write(&samples)
        {
            osd.show(format!("The sound recording failed: {}", e));
            sound_recording = None;
        }
        // What a game shows on the MT-32's or Sound Canvas's display.
        if let Some(message) = cpu.bus.mpu.take_lcd_message() {
            let module = if cpu.bus.mpu.synth_name() == "sc55" { "Sound Canvas" } else { "MT-32" };
            cpu.bus.log_string(&format!("[MIDI] {} display: {}", module, message));
            osd.show(format!("{}: {}", module, message));
        }
        cpu.bus.flush_log();

        // Update Cursor Blink
        if last_blink.elapsed() >= blink_interval {
            cursor_visible = !cursor_visible;
            last_blink = std::time::Instant::now();
            // Blinking characters keep the cursor's time.
            cpu.bus.vga.set_blink(cursor_visible);
        }

        // Render Frame. The expensive part (the pixel fill driven by
        // palette/planar lookups inside `render_screen`) only happens when the
        // VGA state changed since last frame. On "clean" frames we reuse
        // `cached_frame` and just overlay the cursor/mouse/recording pip.
        // The CRTC picks up the Start Address the program flipped to at the
        // vertical retraces that passed, whether or not it polled port 3DAh.
        // The 3dfx card's picture from its memory, unless the OpenGL
        // renderer draws it and nothing looks at it: screenshots,
        // recordings, the debugger, and overlays that mix with it.
        let voodoo_picture = !voodoo_gl_shown
            || screenshot
            || recorder.is_active()
            || video_recording.is_some()
            || ui.is_open()
            || ui.overlay_shown()
            || dbg.wants_frame()
            || settings.monochrome.phosphor().is_some();
        if let Some(v) = &mut cpu.bus.voodoo {
            v.set_software_picture(voodoo_picture);
        }
        cpu.bus.sync_display();
        let voodoo_gl = display.run_voodoo(&mut cpu.bus, &settings.voodoo);
        if !voodoo_gl
            && !voodoo_picture
            && let Some(v) = &mut cpu.bus.voodoo
        {
            v.set_software_picture(true);
            cpu.bus.sync_display();
        }
        voodoo_gl_shown = voodoo_gl;
        let (width, height) = video::frame_size(&cpu.bus);
        if cached_frame.resize(width, height) {
            cpu.bus.vga.mark_dirty_full();
            display.set_frame_size(width, height)?;
        }
        let render_start = std::time::Instant::now();
        if cpu.bus.vga.dirty {
            video::render_screen(&mut cached_frame, &cpu.bus);
            cpu.bus.vga.clear_dirty();
        }
        let render_time = render_start.elapsed();

        // Start from the cached render; the overlays go on top. A
        // monochrome monitor shows them in its phosphor's colour, but not
        // the settings window.
        screen.clone_from(&cached_frame);
        video::overlay::draw_cursors(&mut screen, &cpu.bus, cursor_visible);
        video::mono::apply(&mut screen, settings.monochrome);
        clipboard.draw(&mut screen, &cpu.bus);
        let frame_w = width as usize;

        // Screenshots and recordings show the machine alone, or with the
        // settings window and the performance overlay (`record_ui`), and
        // plain or through the CRT shader (`record_shader`). None shows the
        // messages at the top; debug clients see those as well, but not
        // the recording indicator. Animations stay plain: a GIF's 256
        // colours can't hold the shader's, and quantizing a picture the
        // window's size would hold up the machine.
        // A manual's page is captured as the game is, with or without the
        // rest of the window.
        let record_ui = settings.record_ui || ui.manual_shown();
        macro_rules! capture {
            () => {
                // Drawing the shader's picture again and reading it back
                // takes time, so only when a capture wants it.
                // A manual's page, which the window draws over the
                // picture, with the settings window.
                let paged = if record_ui { ui.with_layer(&screen, (1.0, 1.0)) } else { None };
                let base = paged.as_ref().unwrap_or(&screen);
                let shaded = (settings.record_shader && (screenshot || video_recording.is_some()))
                    .then(|| display.shaded(base))
                    .flatten();
                let picture = shaded.as_ref().unwrap_or(base);
                recorder.capture(base);
                if let Some(video) = &mut video_recording {
                    if !video.record(picture, samples.clone(), cpu.bus.clock.now_ns()) {
                        if let Some(video) = video_recording.take() {
                            match video.stop() {
                                Ok(frames) => osd.show(format!("Video recording stopped: the file is full ({} frames)", frames)),
                                Err(e) => osd.show(format!("The video recording failed: {}", e)),
                            }
                        }
                    }
                }
                if std::mem::take(&mut screenshot) {
                    // The page as sharp as the window shows it.
                    let sharp = (record_ui && shaded.is_none())
                        .then(|| ui.with_layer(&screen, display.output_scale()))
                        .flatten();
                    let picture = sharp.as_ref().unwrap_or(picture);
                    let saved = capture::capture_path(&settings.capture_dir, "screenshot", "png")
                        .and_then(|path| capture::png::save(picture, &path).map(|()| path));
                    match saved {
                        Ok(path) => osd.show(format!("Screenshot saved to {}", path.display())),
                        Err(e) => osd.show(e),
                    }
                }
            };
        }
        let capturing = screenshot || recorder.is_active() || video_recording.is_some();
        if capturing && !record_ui {
            capture!();
        }
        if ui.is_open() {
            ui.set_mixer_status(cpu.bus.mixer.muted, cpu.bus.mixer.take_peaks());
            ui.poll(&mut host!());
        }
        if ui.is_open() || ui.overlay_shown() {
            ui.set_stats(stats.view());
        }
        ui.set_display(display.output_scale(), true);
        ui.draw(&mut screen);
        ui.draw_overlay(&mut screen);
        if capturing && record_ui {
            capture!();
        }
        osd.draw(&mut screen);
        match ui.with_layer(&screen, (1.0, 1.0)) {
            Some(paged) => dbg.capture_frame(&paged),
            None => dbg.capture_frame(&screen),
        }

        // Draw Recording Indicator
        if recorder.is_active() || sound_recording.is_some() || video_recording.is_some() {
            let radius = 5;
            let center_x = frame_w - 15;
            let center_y = 15;

            for y in (center_y - radius)..=(center_y + radius) {
                for x in (center_x - radius)..=(center_x + radius) {
                    let dx = x as isize - center_x as isize;
                    let dy = y as isize - center_y as isize;
                    if dx * dx + dy * dy <= (radius * radius) as isize {
                        let idx = (y * frame_w + x) * 3;
                        if idx + 2 < screen.rgb.len() {
                            screen.rgb[idx] = 0xFF; // R
                            screen.rgb[idx + 1] = 0x00; // G
                            screen.rgb[idx + 2] = 0x00; // B
                        }
                    }
                }
            }
        }
        // Waiting for the deadline of a frame paced to the retrace is
        // neither the frame's work nor time the CPU could have had.
        let waited = pacer.wait_to_present(&cpu.bus.clock);
        display.present(&mut screen, voodoo_gl.then_some(&cached_frame), ui.layer())?;

        let overhead = frame_start.elapsed().saturating_sub(exec_time + waited);
        if let Some(cycles) = pacer.end_frame(&cpu.bus, cpu.pm_latched, executed, exec_time, overhead) {
            cpu.bus.set_cycles_per_ms(cycles);
        }
        let busy = frame_start.elapsed().saturating_sub(waited);
        let code = cpu.dynrec.counts();
        last_frame = Some((
            frame_start,
            rust_dos::stats::FrameTimes {
                wall: busy,
                busy,
                render: render_time,
                executed,
                translated,
                halted: idle,
                blocks: code.live_blocks,
                code_bytes: code.code_bytes,
            },
        ));
        pacer.wait_for_next_frame();
    }

    // The page in the printer comes out, and the job's files are written.
    for notice in cpu.bus.finish_printing() {
        eprintln!("[PRINTER] {}", notice);
    }
    // A booted system still running: what it wrote on the host folders it
    // has as disks goes into them, but for deletions, which its caches may
    // not have finished.
    for line in cpu.bus.sync_shared(None) {
        eprintln!("[BOOT] {}", line);
    }
    // Recordings still going are finished, so their files play.
    if let Some(video) = video_recording
        && let Err(e) = video.stop()
    {
        eprintln!("The video recording failed: {}", e);
    }
    if let Some(wav) = sound_recording
        && let Err(e) = wav.finish()
    {
        eprintln!("The sound recording failed: {}", e);
    }

    Ok(())
}

/// The emulated time between the states rewind takes.
const REWIND_INTERVAL_NS: u64 = 500_000_000;

/// What a slot's message says of the state in it: when it was saved,
/// and in which program.
fn describe_state(header: &slots::Header) -> String {
    match header.program.as_str() {
        "" => header.saved.clone(),
        program => format!("{}, {}", header.saved, program),
    }
}

/// What the on-screen message says of a new CPU speed.
fn speed_message(speed: CpuSpeed) -> String {
    match speed {
        CpuSpeed::Max => "CPU speed max".to_string(),
        CpuSpeed::Fixed(n) => format!("CPU speed {} cycles", n),
        CpuSpeed::Auto(n) if n == CpuSpeed::default().initial_cycles() => "CPU speed auto".to_string(),
        CpuSpeed::Auto(n) => format!("CPU speed auto, at least {} cycles", n),
    }
}

/// The drives the configuration file can hold, by letter: mounts of host
/// directories and CD images, not the drives held in memory.
fn mounted_drives(cpu: &Cpu) -> BTreeMap<u8, MountSpec> {
    cpu.bus
        .disk
        .mounted_drives()
        .into_iter()
        .filter(|info| info.kind != DriveKind::Virtual)
        .filter_map(|info| info.mount.map(|spec| (info.drive, spec)))
        .collect()
}

/// The drives the startup commands (`[autoexec]` and C:\AUTOEXEC.BAT)
/// mount with MOUNT or IMGMOUNT, in the configuration file in `config_dir`.
fn startup_mounts(cpu: &Cpu, autoexec: &[String], config_dir: Option<&std::path::Path>) -> Vec<MountSpec> {
    let cwd = std::env::current_dir().unwrap_or_default();
    let home = rust_dos::hostdirs::home_dir();
    let mut lines = autoexec.to_vec();
    if let Some(file) = cpu.bus.disk.file_data("C:\\AUTOEXEC.BAT")
        && let Ok(bytes) = file.read()
    {
        lines.extend(String::from_utf8_lossy(&bytes).lines().map(str::to_string));
    }
    let disk = &cpu.bus.disk;
    let locate = |path: &str| disk.resolve_path(path);
    let paths = mount::PathContext { base: &cwd, config_dir, home: home.as_deref(), locate: &locate };
    lines
        .iter()
        .filter_map(|line| {
            let (command, args) = line.trim().trim_start_matches('@').split_once(char::is_whitespace)?;
            if !command.eq_ignore_ascii_case("MOUNT") && !command.eq_ignore_ascii_case("IMGMOUNT") {
                return None;
            }
            match mount::parse_mount_command(args, &paths) {
                Ok(MountCmd::Mount(spec)) => Some(spec),
                _ => None,
            }
        })
        .collect()
}

/// The drives that changed since the configuration file was read or last
/// saved, but for those the startup commands mount.
fn drive_changes(cpu: &Cpu, saved: &Saved) -> Vec<config::DriveChange> {
    let current = mounted_drives(cpu);
    let startup = startup_mounts(cpu, &saved.autoexec, config_dir(saved.file.as_deref()).as_deref());
    let same_place = |a: &MountSpec, b: &MountSpec| {
        a.drive == b.drive && std::fs::canonicalize(&a.path).ok() == std::fs::canonicalize(&b.path).ok()
    };
    let mut changes = Vec::new();
    for drive in 0..LASTDRIVE {
        let (before, now) = (saved.drives.get(&drive), current.get(&drive));
        match now {
            _ if before == now => {}
            None => changes.push((drive, None)),
            // A startup command mounts it again anyway.
            Some(spec) if before.is_none() && startup.iter().any(|s| same_place(s, spec)) => {}
            Some(spec) => changes.push((drive, Some(spec.clone()))),
        }
    }
    changes
}

/// Save every setting, and the drives that changed since the file was read
/// or last saved.
fn save_config(cpu: &mut Cpu, saved: &mut Saved, settings: &Settings) -> Result<(), String> {
    let Some(path) = saved.file.clone() else {
        return Err("No configuration file".to_string());
    };
    let changes = drive_changes(cpu, saved);
    let home = rust_dos::hostdirs::home_dir();
    config::save(&path, &saved.settings, settings, &changes, home.as_deref(), config::Saving::All)?;
    cpu.bus.log_string(&format!("[CONFIG] Saved the settings to {}", path.display()));
    saved.settings = settings.clone();
    saved.drives = mounted_drives(cpu);
    Ok(())
}

/// The drive to boot from at startup: the disk image mounted with -boot,
/// unless `skip` (--no-boot, or a game launched).
fn startup_boot(cpu: &mut Cpu, skip: bool) -> Option<u8> {
    let drive = cpu.bus.disk.boot_drive()?;
    if skip {
        cpu.bus.log_string(&format!("[BOOT] Not booting from drive {} at startup", disk::drive_name(drive)));
        return None;
    }
    if cpu.bus.disk.bios_image(drive).is_none() {
        config_warning(cpu, &format!("drive {} can't boot: it isn't a disk image", disk::drive_name(drive)));
        return None;
    }
    cpu.bus.log_string(&format!("[BOOT] Booting from drive {} at startup", disk::drive_name(drive)));
    Some(drive)
}

/// The folder of the game profiles: `games` beside the configuration
/// file `config`.
fn games_dir(config: Option<&std::path::Path>) -> Option<PathBuf> {
    config?.parent().map(|dir| dir.join("games"))
}

/// The folder of a configuration file, which MOUNT -pr takes relative paths
/// from.
fn config_dir(config: Option<&std::path::Path>) -> Option<PathBuf> {
    std::path::absolute(config?).ok()?.parent().map(std::path::Path::to_path_buf)
}

/// The game profile `query` names: its entry, its text and its folder.
fn find_game(config: Option<&std::path::Path>, query: &str) -> Result<(GameEntry, String, PathBuf), String> {
    let dir = games_dir(config).ok_or("--game needs a configuration file, beside which the games folder is")?;
    let games = games::list(&dir);
    let entries: Vec<GameEntry> = games.iter().map(|(e, _)| e.clone()).collect();
    let entry = games::find(&entries, query)
        .ok_or_else(|| format!("No game called {} in {}", query, dir.display()))?
        .clone();
    let text = games.into_iter().find(|(e, _)| e.id == entry.id).map(|(_, t)| t).unwrap_or_default();
    Ok((entry, text, dir))
}

/// The settings window's way to the machine, the display and the speed.
struct MainHost<'m, 'd> {
    cpu: &'m mut Cpu,
    display: &'m mut Display<'d>,
    pacer: &'m mut timer::Pacer,
    /// The settings in effect (or waiting, see `Machine`).
    settings: &'m mut Settings,
    machine: &'m mut Hardware,
    saved: &'m mut Saved,
    /// The game launched and not ended yet.
    game: &'m mut Option<ActiveGame>,
    /// The save states: their folder, the picture the machine shows, for
    /// theirs, and where the thread writing one says it is done.
    states: &'m Option<PathBuf>,
    picture: &'m video::Frame,
    state_done: &'m std::sync::mpsc::Sender<Result<String, String>>,
    /// The slot the hotkeys use, and whether a state was loaded (the
    /// machine's keys and a video recording go).
    slot: &'m mut u8,
    state_loaded: &'m mut bool,
    achievements: &'m mut Achievements,
}

impl MainHost<'_, '_> {
    /// The file the settings window saves to: the game's profile while one
    /// plays, else the configuration file.
    fn config_path(&self) -> Result<PathBuf, String> {
        let file = self.saved.file.as_deref().ok_or("No configuration file")?;
        Ok(match self.game.as_ref() {
            Some(game) => games_dir(Some(file)).ok_or("No games folder")?.join(format!("{}.conf", game.id)),
            None => file.to_path_buf(),
        })
    }

    /// Launch the game `id` from its profile `text` in the folder `dir`:
    /// its settings over these, its drives over theirs, and the commands
    /// that start it.
    fn start_game(&mut self, id: &str, text: &str, dir: &std::path::Path) -> Result<String, String> {
        if let Some(previous) = self.game.take() {
            self.end_game(previous);
        }
        let base = self.settings.clone();
        let mut prepared = games::prepare(id, &base, text, dir, rust_dos::hostdirs::home_dir().as_deref())?;
        for warning in &prepared.warnings {
            config_warning(self.cpu, &format!("games/{}.conf: {}", id, warning));
        }
        if let Some(saves) = self.saved.file.as_deref().and_then(|f| games_dir(Some(f))).map(|g| games::saves_dir(&g)) {
            games::overlay_drives(&mut prepared, id, &saves);
        }
        if let Err(e) = self.apply(&prepared.settings) {
            config_warning(self.cpu, &e);
        }
        let mut replaced = Vec::new();
        for spec in &prepared.drives {
            let before = self.cpu.bus.disk.drive_info(spec.drive).and_then(|d| d.mount);
            match self.cpu.bus.mount_drive(spec.drive, &spec.path, spec.opts.clone(), true) {
                Ok(_) => replaced.push((spec.drive, before)),
                Err(e) => config_warning(self.cpu, &format!("games/{}.conf: drive {}: {}", id, disk::drive_key(spec.drive), e)),
            }
        }
        self.cpu.bus.config_dir = std::path::absolute(dir).ok();
        self.cpu.queue_batch_lines(&prepared.autoexec);
        self.cpu.bus.log_string(&format!("[CONFIG] Launching the game {} (games/{}.conf)", prepared.name, id));
        // What RetroAchievements knows the game by.
        let hash = prepared.achievements.as_deref().and_then(|value| {
            games::achievements_hash(value, dir, rust_dos::hostdirs::home_dir().as_deref())
                .map_err(|e| {
                    if self.settings.achievements.enabled {
                        config_warning(self.cpu, &format!("games/{}.conf: achievements: {}", id, e));
                    }
                })
                .ok()
        });
        self.achievements.game_started(hash, &prepared.name);
        let message = format!("Starting {}", prepared.name);
        *self.game = Some(ActiveGame { id: id.to_string(), name: prepared.name, base, saved: prepared.settings, replaced });
        Ok(message)
    }

    /// A file or folder dropped onto the window: a game set up for DOSBox
    /// is imported and launched, a folder or disk image mounted, a CD image
    /// put in the CD-ROM drive, a floppy in A:, and a program run from its
    /// folder. Returns what to tell the user.
    fn dropped(&mut self, path: &std::path::Path) -> String {
        use rust_dos::import::drop::{DropAction, drop_action};
        let free = |cpu: &Cpu| (3..LASTDRIVE - 1).find(|&d| !cpu.bus.disk.is_mounted(d));
        let mount = |host: &mut Self, drive: Option<u8>, path: &std::path::Path, kind: DriveKind| -> Result<u8, String> {
            let drive = drive.ok_or("There is no free drive letter")?;
            let spec = MountSpec { drive, path: path.to_path_buf(), opts: disk::MountOptions { kind, ..Default::default() } };
            host.mount(spec, true).map(|_| drive)
        };
        let name = |path: &std::path::Path| path.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned());
        let result = match drop_action(path) {
            DropAction::ImportGame(source) => {
                self.import_game(&source).map(|(id, imported)| self.launch_game(&id).unwrap_or(imported))
            }
            DropAction::MountFolder(dir) => {
                mount(self, free(self.cpu), &dir, DriveKind::HardDisk).map(|d| format!("{}: is {}", disk::drive_letter(d), name(&dir)))
            }
            DropAction::Disc(image) => {
                let drive = self.cpu.bus.disk.drives_of_kind(DriveKind::CdRom).first().copied().or_else(|| free(self.cpu));
                mount(self, drive, &image, DriveKind::CdRom).map(|d| format!("{}: has {}", disk::drive_letter(d), name(&image)))
            }
            DropAction::Floppy(image) => mount(self, Some(0), &image, DriveKind::Floppy).map(|_| format!("A: has {}", name(&image))),
            DropAction::HardDisk(image) => {
                mount(self, free(self.cpu), &image, DriveKind::HardDisk).map(|d| format!("{}: is {}", disk::drive_letter(d), name(&image)))
            }
            DropAction::Run(program) => {
                let dir = program.parent().unwrap_or(std::path::Path::new(".")).to_path_buf();
                let mounted = self.cpu.bus.disk.mounted_drives().into_iter().find(|d| d.root.as_deref() == Some(dir.as_path()));
                let drive = match mounted {
                    Some(info) => Ok(info.drive),
                    None => mount(self, free(self.cpu), &dir, DriveKind::HardDisk),
                };
                drive.map(|d| {
                    if !self.cpu.shell_idle() || self.cpu.batch.is_active() || self.cpu.shell_wait.is_some() {
                        return format!("{}: is {}; quit the program running to start {}", disk::drive_letter(d), name(&dir), name(&program));
                    }
                    let letter = disk::drive_letter(d);
                    self.cpu.queue_batch_lines([format!("{}:", letter), "CD \\".to_string(), name(&program)]);
                    format!("Starting {} from {}:", name(&program), letter)
                })
            }
            DropAction::Package(package) => (|| {
                let dir = games_dir(self.saved.file.as_deref())
                    .ok_or("Packages become games in the games folder beside the configuration file, and there is none (--no-config)")?;
                let (id, game, warnings) = games::add_package(&dir, &package)?;
                for warning in &warnings {
                    self.cpu.bus.log_string(&format!("[CONFIG] Import of {}: {}", game, warning));
                }
                Ok(self.launch_game(&id).unwrap_or_else(|e| format!("{} is a game on the Games page: {}", game, e)))
            })(),
            DropAction::Nothing(e) => Err(e),
        };
        result.unwrap_or_else(|e| e)
    }

    /// A game has ended: the settings and drives from before it.
    /// The folder of the save states' slots: the game's, or the machine's
    /// without one.
    fn slot_dir(&self) -> Result<PathBuf, String> {
        let root = self.states.as_deref().ok_or("There is no folder for save states")?;
        Ok(slots::slot_dir(root, self.game.as_ref().map(|g| g.id.as_str())))
    }

    /// The machine's state now, with its header and picture.
    fn capture_state(&mut self) -> (slots::Header, video::Frame, Vec<u8>) {
        let game = self.game.as_ref().map(|g| (g.id.as_str(), g.name.as_str()));
        // The hardware in place, not a change waiting for the program to end.
        let hardware = self.machine.settings(self.settings);
        let mut picture = self.picture.clone();
        // The OpenGL renderer draws the 3dfx card's picture: the one in its
        // memory for the thumbnail.
        if let Some(v) = &mut self.cpu.bus.voodoo
            && v.output()
            && !v.picture_wanted()
        {
            v.set_software_picture(true);
            v.prepare_display();
            v.render(&mut picture.rgb, picture.width as usize);
            v.set_software_picture(false);
        }
        (slots::header(self.cpu, &hardware, game), picture, savestate::machine::save(self.cpu))
    }

    /// Save the machine to slot `slot`. Its state is taken now; a thread
    /// packs it and writes the file, and says on `state_done` when it has.
    fn save_slot(&mut self, slot: u8) -> Result<(), String> {
        let path = slots::slot_path(&self.slot_dir()?, slot);
        let (header, picture, state) = self.capture_state();
        // A booted system's disks as they are, beside its state.
        savestate::disks::delete_copies(&path);
        savestate::disks::save_copies(self.cpu, &path)?;
        let done = self.state_done.clone();
        self.cpu.bus.log_string(&format!("[STATE] Saving to {}", path.display()));
        std::thread::spawn(move || {
            let data = slots::encode(&header, &slots::thumbnail(&picture), &state);
            let result = slots::write_file(&path, &data).map(|()| format!("Saved to slot {}", slot));
            let _ = done.send(result);
        });
        Ok(())
    }

    /// Save the machine to the file `path`, now.
    fn save_file(&mut self, path: &std::path::Path) -> Result<(), String> {
        let (header, picture, state) = self.capture_state();
        savestate::disks::delete_copies(path);
        savestate::disks::save_copies(self.cpu, path)?;
        slots::write_file(path, &slots::encode(&header, &slots::thumbnail(&picture), &state))?;
        self.cpu.bus.log_string(&format!("[STATE] Saved to {}", path.display()));
        Ok(())
    }

    /// Load the save state file `path`: the hardware it was saved with
    /// first, as the settings have it, then the machine. Memory can't
    /// change its size, so a state of another memsize is refused.
    fn load_file(&mut self, path: &std::path::Path) -> Result<slots::Header, String> {
        if self.achievements.hardcore_active() {
            return Err("hardcore mode: no save states while RetroAchievements plays".to_string());
        }
        let data = std::fs::read(path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => "empty".to_string(),
            _ => format!("{}: {}", path.display(), e),
        })?;
        let (header, state) = slots::decode(&data)?;
        if let Some(why) = slots::refusal(&header, self.cpu.bus.ram().len() >> 20) {
            return Err(why);
        }
        let hardware = slots::machine_settings(&header.machine, self.settings);
        if let Err(e) = self.apply(&hardware) {
            config_warning(self.cpu, &e);
        }
        if self.machine.differs(self.settings) {
            for warning in self.machine.apply(self.cpu, self.settings) {
                config_warning(self.cpu, &warning);
            }
        }
        // A booted system's disks come back from their copies, if the
        // journals don't reach back to the state.
        savestate::disks::offer_copies(self.cpu, path);
        let loaded = savestate::machine::load(self.cpu, &state).map_err(|e| e.to_string());
        savestate::disks::withdraw(self.cpu);
        loaded?;
        self.pacer.rebase(&self.cpu.bus.clock, std::time::Instant::now());
        *self.state_loaded = true;
        self.cpu.bus.log_string(&format!("[STATE] Loaded {} (saved {})", path.display(), header.saved));
        Ok(header)
    }

    /// Load slot `slot`.
    fn load_slot(&mut self, slot: u8) -> Result<slots::Header, String> {
        let path = slots::slot_path(&self.slot_dir()?, slot);
        self.load_file(&path)
    }

    /// RetroAchievements' login: kept in the settings, and in the
    /// configuration file (not a game's profile), without the password.
    fn keep_token(&mut self, username: &str, token: &str) -> Result<(), String> {
        let set = |s: &mut Settings| {
            s.achievements.username = username.to_string();
            s.achievements.token = token.to_string();
        };
        set(self.settings);
        if let Some(game) = self.game.as_mut() {
            set(&mut game.base);
            set(&mut game.saved);
        }
        let before = self.saved.settings.clone();
        set(&mut self.saved.settings);
        let Some(path) = self.saved.file.clone() else { return Ok(()) };
        let home = rust_dos::hostdirs::home_dir();
        config::save(&path, &before, &self.saved.settings, &[], home.as_deref(), config::Saving::Changes)
    }

    fn end_game(&mut self, game: ActiveGame) {
        self.cpu.bus.log_string(&format!("[CONFIG] The game {} has ended", game.name));
        self.achievements.game_ended();
        self.cpu.bus.config_dir = config_dir(self.saved.file.as_deref());
        if let Err(e) = self.apply(&game.base) {
            config_warning(self.cpu, &e);
        }
        for (drive, before) in game.replaced.into_iter().rev() {
            let result = match before {
                Some(spec) => self.cpu.bus.mount_drive(drive, &spec.path, spec.opts, true).map(|_| ()),
                None => self.cpu.bus.unmount_drive(drive),
            };
            if let Err(e) = result {
                config_warning(self.cpu, &format!("drive {}: {}", disk::drive_key(drive), e));
            }
        }
    }
}

impl Host for MainHost<'_, '_> {
    fn apply(&mut self, new: &Settings) -> Result<Option<String>, String> {
        let old = std::mem::replace(self.settings, new.clone());
        let shown = |s: &Settings| (s.scale, s.fullscreen, s.aspect, s.filter, s.shader, s.crt, s.monochrome);
        if shown(new) != shown(&old) {
            self.display.apply(new)?;
        }
        if new.cycles != old.cycles {
            self.pacer.set_speed(new.cycles);
            // At max, the pacer tunes the speed from the current one, and
            // at auto, it picks the speed at the end of the frame.
            if let CpuSpeed::Fixed(n) = new.cycles {
                self.cpu.bus.set_cycles_per_ms(n);
            }
        }
        if new.core != old.core {
            self.cpu.core = new.core;
        }
        // Programs look for the host as they start.
        self.cpu.bus.dpmi.enabled = new.dpmi;
        self.cpu.bus.ide_hard_disks = new.ide_hard_disks;
        self.cpu.bus.boot_cdrom = new.boot_cdrom;
        if new.dos_version != old.dos_version {
            rust_dos::dos_data::set_version(&mut self.cpu.bus, new.dos_version);
        }
        if new.disk != old.disk {
            self.cpu.bus.set_disk_settings(new.disk);
        }
        if new.mixer != old.mixer {
            self.cpu.bus.set_mixer(new.mixer);
        }
        if new.joystick != old.joystick {
            self.cpu.bus.set_joystick(new.joystick);
        }
        if new.composite != old.composite {
            self.cpu.bus.vga.set_composite(new.composite);
        }
        if new.keyboard_layout != old.keyboard_layout {
            apply_keyboard_layout(self.cpu, new.keyboard_layout);
        }
        if new.achievements != old.achievements {
            self.achievements.apply(&new.achievements);
        }
        if !self.machine.differs(new) {
            return Ok(None);
        }
        if !self.cpu.shell_idle() {
            return Ok(Some(config_ui::pending_note(self.machine.video, new).to_string()));
        }
        match self.machine.apply(self.cpu, new).into_iter().next() {
            Some(problem) => Err(problem),
            None => Ok(None),
        }
    }

    fn mount(&mut self, spec: MountSpec, replace: bool) -> Result<PathBuf, String> {
        self.cpu.bus.log_string(&format!(
            "[CONFIG] Settings window: mount {}: {}",
            disk::drive_letter(spec.drive),
            spec.path.display()
        ));
        self.cpu.bus.mount_drive(spec.drive, &spec.path, spec.opts, replace)
    }

    fn unmount(&mut self, drive: u8) -> Result<(), String> {
        self.cpu.bus.unmount_drive(drive)
    }

    fn drives(&self) -> Vec<DriveInfo> {
        self.cpu.bus.disk.mounted_drives()
    }

    fn booted(&self) -> Option<config_ui::BootView> {
        let bus = &self.cpu.bus;
        bus.boot.as_ref()?;
        let shared =
            bus.disk.shared_drives().into_iter().filter_map(|d| Some((d, rust_dos::boot::drive_unit(bus, d)?))).collect();
        Some(config_ui::BootView { cd_drive: bus.booted_cd_drive(), shared })
    }

    fn sync_shared(&mut self, drive: u8) -> Result<String, String> {
        self.cpu.bus.sync_shared(Some(drive)).into_iter().next().ok_or_else(|| "Nothing is shared".to_string())
    }

    fn reinsert(&mut self, drive: u8) -> Result<String, String> {
        self.cpu.bus.reinsert_cd(drive)
    }

    fn boot(&mut self, drive: u8) -> Result<String, String> {
        self.cpu.bus.log_string(&format!("[CONFIG] Settings window: boot {}", disk::drive_name(drive)));
        rust_dos::boot::boot_drive(self.cpu, drive)?;
        Ok(format!("Booting from drive {}", disk::drive_name(drive)))
    }

    fn set_boot(&mut self, drive: u8, boot: bool) -> Result<(), String> {
        self.cpu.bus.disk.set_boots(drive, boot);
        Ok(())
    }

    fn save(&mut self, settings: &Settings) -> Result<(), String> {
        // While a game plays, into its profile.
        if let Some(game) = self.game.as_mut() {
            let dir = games_dir(self.saved.file.as_deref()).ok_or("No configuration file")?;
            let path = dir.join(format!("{}.conf", game.id));
            config::save(&path, &game.saved, settings, &[], rust_dos::hostdirs::home_dir().as_deref(), config::Saving::Changes)?;
            game.saved = settings.clone();
            return Ok(());
        }
        save_config(self.cpu, self.saved, settings)
    }

    fn autoexec(&self) -> Result<Vec<String>, String> {
        config::load_autoexec(&self.config_path()?)
    }

    fn save_autoexec(&mut self, lines: &[String]) -> Result<(), String> {
        let path = self.config_path()?;
        config::save_autoexec(&path, lines)?;
        self.cpu.bus.log_string(&format!("[CONFIG] Saved the [autoexec] commands to {}", path.display()));
        Ok(())
    }

    fn lan(&self) -> Option<rust_dos::net::LanView> {
        Some(self.cpu.bus.net.view())
    }

    fn browse_rooms(&mut self, relay: Option<&str>, filter: &str) -> Result<(), String> {
        self.cpu.bus.net.browse(relay, filter)
    }

    fn join_room(&mut self, relay: Option<&str>, room: &str, password: &str) -> Result<(), String> {
        // As LAN JOIN does, for the games started next.
        self.cpu.bus.install_ipx();
        self.cpu.bus.net.join(relay, room, password)
    }

    fn leave_room(&mut self) {
        self.cpu.bus.net.leave();
    }

    fn disband_room(&mut self) {
        self.cpu.bus.net.disband();
    }

    fn host_room(&mut self, room: &str, password: &str) -> Result<(), String> {
        self.cpu.bus.install_ipx();
        self.cpu.bus.net.make_room(room, password)
    }

    fn games(&self) -> Vec<GameEntry> {
        games_dir(self.saved.file.as_deref()).map_or_else(Vec::new, |dir| games::list(&dir).into_iter().map(|(e, _)| e).collect())
    }

    fn active_game(&self) -> Option<String> {
        self.game.as_ref().map(|g| g.id.clone())
    }

    fn launch_game(&mut self, id: &str) -> Result<String, String> {
        if !self.cpu.shell_idle() || self.cpu.batch.is_active() || self.cpu.shell_wait.is_some() {
            return Err("A program is running: quit it to launch a game".to_string());
        }
        let dir = games_dir(self.saved.file.as_deref()).ok_or("There is no configuration file for the games folder")?;
        let path = dir.join(format!("{}.conf", id));
        let text = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
        self.start_game(id, &text, &dir)
    }

    fn create_game(&mut self, new: &NewGame, settings: &Settings) -> Result<String, String> {
        let dir = games_dir(self.saved.file.as_deref())
            .ok_or("Game profiles go beside the configuration file, and there is none (--no-config)")?;
        let taken: Vec<String> = games::list(&dir).into_iter().map(|(e, _)| e.id).collect();
        let id = games::slug(&new.name, &taken);
        let base = self.game.as_ref().map_or(&self.saved.settings, |g| &g.base);
        let drives = drive_changes(self.cpu, self.saved);
        let text = games::profile_text(new, base, settings, &drives, rust_dos::hostdirs::home_dir().as_deref())?;
        let path = dir.join(format!("{}.conf", id));
        std::fs::create_dir_all(&dir)
            .and_then(|()| std::fs::write(&path, text))
            .map_err(|e| format!("cannot write {}: {}", path.display(), e))?;
        self.cpu.bus.log_string(&format!("[CONFIG] Made the game profile {}", path.display()));
        Ok(id)
    }

    fn delete_game(&mut self, id: &str) -> Result<(), String> {
        let dir = games_dir(self.saved.file.as_deref()).ok_or("There is no games folder")?;
        let path = dir.join(format!("{}.conf", id));
        std::fs::remove_file(&path).map_err(|e| format!("cannot delete {}: {}", path.display(), e))
    }

    fn reset_game(&mut self, id: &str) -> Result<(), String> {
        if self.game.as_ref().is_some_and(|g| g.id == id) {
            return Err("The game is running: reset it once it has ended".to_string());
        }
        let dir = games_dir(self.saved.file.as_deref()).ok_or("There is no games folder")?;
        games::reset(&games::saves_dir(&dir), id)
    }

    fn import_game(&mut self, source: &std::path::Path) -> Result<(String, String), String> {
        let dir = games_dir(self.saved.file.as_deref())
            .ok_or("Game profiles go beside the configuration file, and there is none (--no-config)")?;
        let (id, name, warnings) = games::import(&dir, source, rust_dos::hostdirs::home_dir().as_deref())?;
        for warning in &warnings {
            self.cpu.bus.log_string(&format!("[CONFIG] Import of {}: {}", name, warning));
        }
        let message = match warnings.len() {
            0 => format!("{} is imported: Enter launches it", name),
            n => format!("{} is imported; {} settings didn't come across (see the log)", name, n),
        };
        Ok((id, message))
    }

    fn current_directory(&self) -> String {
        games::prompt_directory(self.cpu)
    }

    fn memory(&self) -> &[u8] {
        self.cpu.bus.ram()
    }

    fn cheats_allowed(&self) -> Result<(), String> {
        if self.achievements.hardcore_active() {
            return Err("Hardcore mode: no cheats while RetroAchievements plays".to_string());
        }
        Ok(())
    }

    fn achievements(&self) -> Option<rust_dos::achievements::AchievementsView> {
        Some(self.achievements.view())
    }

    fn achievements_login(&mut self, username: &str, password: &str) -> Result<(), String> {
        self.achievements.login(username, password);
        Ok(())
    }

    fn achievements_logout(&mut self) {
        self.achievements.logout();
    }

    fn identify_game(&mut self, archive: &std::path::Path) -> Result<String, String> {
        let Some(game) = self.game.as_ref() else {
            return Err("Launch the game from its profile first (the Games page)".to_string());
        };
        let hash = rust_dos::achievements::hash::hash_archive(archive)?;
        let dir = games_dir(self.saved.file.as_deref()).ok_or("There is no games folder")?;
        let path = dir.join(format!("{}.conf", game.id));
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {}", path.display(), e))?;
        std::fs::write(&path, games::set_achievements(&text, &hash)).map_err(|e| format!("{}: {}", path.display(), e))?;
        let name = game.name.clone();
        self.achievements.game_started(Some(hash.clone()), &name);
        Ok(format!("{} is known by {} now (hash {})", name, archive.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned()), hash))
    }

    fn manuals(&self, id: &str) -> Vec<rust_dos::manuals::Manual> {
        let Some(dir) = games_dir(self.saved.file.as_deref()) else { return Vec::new() };
        let text = std::fs::read_to_string(dir.join(format!("{}.conf", id))).unwrap_or_default();
        games::manuals(&dir, id, &text, rust_dos::hostdirs::home_dir().as_deref())
    }

    fn add_manual(&mut self, id: &str, path: &std::path::Path) -> Result<(), String> {
        let dir = games_dir(self.saved.file.as_deref()).ok_or("There is no games folder")?;
        games::add_manual_file(&dir, id, path, rust_dos::hostdirs::home_dir().as_deref())
    }

    fn poke(&mut self, addr: usize, bytes: &[u8]) {
        for (i, &byte) in bytes.iter().enumerate() {
            self.cpu.bus.write_8(addr + i, byte);
        }
    }

    fn freezes(&self) -> Vec<rust_dos::cheats::Freeze> {
        self.cpu.bus.freezes.clone()
    }

    fn set_freezes(&mut self, freezes: Vec<rust_dos::cheats::Freeze>) {
        self.cpu.bus.freezes = freezes;
        self.cpu.bus.apply_freezes();
    }

    fn states_available(&self) -> bool {
        self.states.is_some()
    }

    fn states(&self) -> Vec<config_ui::SlotView> {
        let Ok(dir) = self.slot_dir() else { return Vec::new() };
        slots::list(&dir)
            .into_iter()
            .map(|(slot, header, picture)| config_ui::SlotView {
                slot,
                header: Some(header),
                picture: capture::png::decode(&picture),
            })
            .collect()
    }

    fn current_slot(&self) -> u8 {
        *self.slot
    }

    fn save_state(&mut self, slot: u8) -> Result<String, String> {
        let path = slots::slot_path(&self.slot_dir()?, slot);
        self.save_file(&path)?;
        *self.slot = slot;
        Ok(format!("Saved to slot {}", slot))
    }

    fn load_state(&mut self, slot: u8) -> Result<String, String> {
        let header = self.load_slot(slot)?;
        *self.slot = slot;
        Ok(format!("Loaded slot {}: {}", slot, describe_state(&header)))
    }

    fn delete_state(&mut self, slot: u8) -> Result<(), String> {
        let path = slots::slot_path(&self.slot_dir()?, slot);
        savestate::disks::delete_copies(&path);
        std::fs::remove_file(&path).map_err(|e| format!("{}: {}", path.display(), e))
    }
}

/// The settings window's key for a key press, if it takes it. Characters
/// come from text input, which follows the keyboard layout.
fn ui_key(keycode: Keycode, keymod: Mod) -> Option<UiKey> {
    let shift = keymod.intersects(Mod::LSHIFTMOD | Mod::RSHIFTMOD);
    let ctrl = keymod.intersects(Mod::LCTRLMOD | Mod::RCTRLMOD) && !keymod.intersects(Mod::LALTMOD | Mod::RALTMOD);
    Some(match keycode {
        Keycode::Up => UiKey::Up,
        Keycode::Down => UiKey::Down,
        Keycode::Left => UiKey::Left,
        Keycode::Right => UiKey::Right,
        Keycode::PageUp => UiKey::PageUp,
        Keycode::PageDown => UiKey::PageDown,
        Keycode::Home => UiKey::Home,
        Keycode::End => UiKey::End,
        Keycode::Return | Keycode::KpEnter => UiKey::Enter,
        Keycode::Escape => UiKey::Esc,
        Keycode::Tab if shift => UiKey::BackTab,
        Keycode::Tab => UiKey::Tab,
        Keycode::Backspace => UiKey::Backspace,
        Keycode::Delete => UiKey::Delete,
        Keycode::Insert => UiKey::Insert,
        Keycode::F1 => UiKey::Help,
        Keycode::F2 => UiKey::Save,
        Keycode::S if ctrl => UiKey::Save,
        _ => return None,
    })
}

/// Let go of everything the machine holds as the settings window takes the
/// keyboard and mouse: release the keys and buttons, so no game is left
/// with Ctrl or a fire button held down.
fn release_input(cpu: &mut Cpu, held: &mut HashMap<Scancode, (u8, bool)>) {
    held.clear();
    // Shift, Ctrl and Alt too; the lock states stay.
    keyboard::release_all(&mut cpu.bus);
    for button in 0..3 {
        if cpu.bus.mouse.buttons & (1 << button) != 0 {
            cpu.bus.mouse.button_up(button);
        }
    }
}

/// Caps Lock and Num Lock as the host's keyboard has them.
fn sync_locks(cpu: &mut Cpu, mods: Mod) {
    // A booted system's BIOS keeps its own locks, from the keys.
    if cpu.bus.boot.is_some() {
        return;
    }
    let mut flags = cpu.bus.read_8(0x0417) & !0x60;
    if mods.contains(Mod::CAPSMOD) {
        flags |= 0x40;
    }
    if mods.contains(Mod::NUMMOD) {
        flags |= 0x20;
    }
    cpu.bus.write_8(0x0417, flags);
}

/// The keyboard layout the machine types in: the setting's, or the host
/// keyboard's for auto.
fn apply_keyboard_layout(cpu: &mut Cpu, setting: rust_dos::keylayout::LayoutSetting) {
    let layout = setting.layout(sdl_keys::detect_layout());
    if !std::ptr::eq(cpu.bus.kbd.layout, layout) {
        cpu.bus.log_string(&format!("[INPUT] Keyboard layout: {} ({})", layout.name, layout.code));
        cpu.bus.kbd.layout = layout;
    }
}

/// A configuration problem: shown on the terminal and written to the log.
fn config_warning(cpu: &mut Cpu, msg: &str) {
    eprintln!("[CONFIG] Warning: {}", msg);
    cpu.bus.log_string(&format!("[CONFIG] Warning: {}", msg));
}

/// The note in the box above the first DOS prompt: the configuration
/// file in use.
fn banner_notes(config: &config::Config, no_config: bool) -> Vec<(String, String)> {
    let file = match &config.source {
        Some(path) => {
            let path = mount::display_host_path(&std::path::absolute(path).unwrap_or_else(|_| path.clone()));
            if config.created { format!("{} (new)", path) } else { path }
        }
        None if no_config => "none (--no-config)".to_string(),
        None => "none".to_string(),
    };
    vec![("Config file: ".to_string(), file)]
}

/// Find and parse the configuration file (see config.rs for the lookup
/// order). Problems in the file are reported but never stop the emulator;
/// only a missing `--config` file does. The file used goes to the log once
/// there is one (see `create_cpu`).
fn load_config(args: &Args) -> Result<config::Config, String> {
    if args.no_config {
        return Ok(config::Config::default());
    }
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let config = config::load(
        args.config.as_deref(),
        &cwd,
        config::exe_dir().as_deref(),
        config::default_path(),
        rust_dos::hostdirs::home_dir().as_deref(),
    )?;
    if config.created
        && let Some(path) = &config.source
    {
        eprintln!("[CONFIG] Created default configuration file {}", path.display());
    }
    for warning in &config.warnings {
        eprintln!("[CONFIG] Warning: {}", warning);
    }
    Ok(config)
}

/// Build the CPU with drive C: from `-d`, the config file or the working
/// directory (in that order), then mount the config's other drives. Opens
/// the log file.
fn create_cpu(args: &Args, config: &config::Config, memory_mb: usize) -> Cpu {
    use crate::disk::DRIVE_C;

    let mut warnings = config.warnings.clone();
    let mut warn = |msg: String| {
        eprintln!("[CONFIG] Warning: {}", msg);
        warnings.push(msg);
    };

    let config_c = config.drive(DRIVE_C);
    let c_spec = match (&args.dir, config_c) {
        (Some(_), Some(_)) => {
            warn("-d/--dir overrides drive C: from the config file".to_string());
            None
        }
        (None, Some(spec)) if !spec.path.is_dir() && !spec.path.is_file() => {
            warn(format!(
                "C: {} is not a directory or a disk image, using the current directory",
                spec.path.display()
            ));
            None
        }
        (_, spec) => spec,
    };
    // A disk image goes in once C: is there.
    let root_path = match (&args.dir, c_spec) {
        (Some(dir), _) => std::path::PathBuf::from(dir),
        (None, Some(spec)) if spec.path.is_dir() => spec.path.clone(),
        _ => std::path::PathBuf::from("."),
    };

    let mut cpu = Cpu::with_memory(root_path.clone(), memory_mb);
    cpu.bus.log_file = open_log_file();
    if let Some(spec) = c_spec {
        // Remount C: to apply the config's drive type, label and -ro, or
        // with its disk image.
        if let Err(e) = cpu.bus.mount_drive(DRIVE_C, &spec.path, spec.opts.clone(), true) {
            warn(format!("cannot set up drive C: {}", e));
        }
    }
    for spec in config.drives.iter().filter(|s| s.drive != DRIVE_C) {
        if let Err(e) = cpu.bus.mount_drive(spec.drive, &spec.path, spec.opts.clone(), false) {
            warn(format!("cannot mount {}: {}", disk::drive_key(spec.drive), e));
        }
    }

    if let Some(path) = &config.source {
        let action = if config.created { "Created default" } else { "Using" };
        cpu.bus
            .log_string(&format!("[CONFIG] {} configuration file {}", action, path.display()));
    }
    for warning in &warnings {
        cpu.bus.log_string(&format!("[CONFIG] Warning: {}", warning));
    }
    cpu
}

/// Create the log file in the per-user configuration directory, replacing
/// the previous run's. The emulator runs without one if that fails.
fn open_log_file() -> Option<rust_dos::log::LogFile> {
    let path = rust_dos::log::default_path()?;
    match rust_dos::log::LogFile::create(&path) {
        Ok(log) => Some(log),
        Err(e) => {
            eprintln!("Warning: cannot create the log file {}: {}", path.display(), e);
            None
        }
    }
}

/// A game controller's sticks and buttons, as the game port takes them.
fn pad_state(pad: &sdl2::controller::GameController) -> joystick::PadState {
    use sdl2::controller::{Axis, Button};
    let axis = |axis| pad.axis(axis) as f32 / 32767.0;
    let buttons = [
        (Button::A, joystick::PAD_A),
        (Button::B, joystick::PAD_B),
        (Button::X, joystick::PAD_X),
        (Button::Y, joystick::PAD_Y),
        (Button::DPadUp, joystick::PAD_UP),
        (Button::DPadDown, joystick::PAD_DOWN),
        (Button::DPadLeft, joystick::PAD_LEFT),
        (Button::DPadRight, joystick::PAD_RIGHT),
    ]
    .into_iter()
    .filter(|&(button, _)| pad.button(button))
    .fold(0, |bits, (_, bit)| bits | bit);
    joystick::PadState {
        axes: [axis(Axis::LeftX), axis(Axis::LeftY), axis(Axis::RightX), axis(Axis::RightY)],
        buttons,
    }
}

fn sdl_button_to_index(button: MouseButton) -> Option<usize> {
    match button {
        MouseButton::Left => Some(0),
        MouseButton::Right => Some(1),
        MouseButton::Middle => Some(2),
        _ => None,
    }
}

/// What the mouse captured says, with how to let it go.
const CAPTURED: &str = "Mouse captured (Ctrl+Alt releases)";

/// The host's key as Ctrl+Alt sees it.
fn chord_key(scancode: Option<Scancode>) -> rust_dos::mouse_capture::ChordKey {
    use rust_dos::mouse_capture::ChordKey;
    match scancode {
        Some(Scancode::LCtrl) => ChordKey::LeftCtrl,
        Some(Scancode::RCtrl) => ChordKey::RightCtrl,
        Some(Scancode::LAlt) => ChordKey::LeftAlt,
        Some(Scancode::RAlt) => ChordKey::RightAlt,
        _ => ChordKey::Other,
    }
}
