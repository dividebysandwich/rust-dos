use clap::Parser;
use sdl2::event::{Event, WindowEvent};
use sdl2::keyboard::{Keycode, Mod, Scancode};
use sdl2::mouse::{MouseButton, MouseWheelDirection};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use crate::audio::pump_audio;
use crate::config::Settings;
use crate::config_ui::osd::Osd;
use crate::config_ui::{ConfigUi, Host};
use crate::cpu::Cpu;
use crate::display::Display;
use crate::capture::avi::VideoRecorder;
use crate::clipboard::Clipboard;
use crate::capture::wav::WavWriter;
use crate::recorder::ScreenRecorder;
use crate::timer::CpuSpeed;

mod args;
mod clipboard;
mod config_file;
mod debug;
mod display;
mod input;
mod main_almanac;
mod main_host;
mod monitor_power;
mod sdl_audio;
mod sdl_keys;
mod startup;
#[cfg(test)]
mod test_dir;

// The emulator itself is the library crate; the debug server and the
// window's display are private to the binary. These re-exports let the
// binary's modules refer to the library modules as `crate::...`.
use rust_dos::{
    audio, capture, config, config_ui, cpu, disk, exec, games, joystick, keyboard, mount, recorder, shell, timer,
    video,
};
use rust_dos::achievements::{Achievements, http::HttpTransport};
use rust_dos::games::ActiveGame;
use rust_dos::hardware::Hardware;
use rust_dos::profile::{self, Zone};
use rust_dos::savestate::{self, slots};

use crate::args::{Args, Command};
use crate::config_file::{Saved, config_warning, games_dir, load_config, mounted_drives};
use crate::input::{
    CAPTURED, apply_keyboard_layout, chord_key, pad_snapshot, pad_state, release_input, sdl_button_to_index, step_autoinput,
    sync_locks, ui_key,
};
use crate::main_host::MainHost;
use crate::monitor_power::{power_off, power_on, resync};
use crate::sdl_audio::open_audio;
use crate::startup::{apply_args, create_cpu, game_to_launch, start_debug_hub, start_dos};
#[cfg(windows)]
use crate::startup::leave_explorer_console;
#[cfg(test)]
use crate::startup::open_log_file;

fn main() -> Result<(), String> {
    let mut args = Args::parse();
    match args.command.take() {
        Some(Command::Run(run)) => {
            std::process::exit(rust_dos::cli::run(rust_dos::cli::RunOptions {
                command: run.command,
                dir: run.dir.unwrap_or_else(|| std::path::PathBuf::from(".")),
                cycles: run.cycles,
                memory_mb: run.mem as usize,
                screen: run.screen,
                win32: run.win32,
            }))
        }
        Some(Command::Unpack(unpack)) => {
            std::process::exit(rust_dos::unpack::run(rust_dos::unpack::UnpackOptions {
                archive: unpack.archive,
                list: unpack.list,
                to: unpack.to.unwrap_or_else(|| std::path::PathBuf::from(".")),
                groups: unpack.group,
                compression: match unpack.old_compression {
                    true => rust_dos_installshield::Compression::Markers,
                    false => rust_dos_installshield::Compression::Chunks,
                },
            }))
        }
        None => {}
    }
    if let Some(port) = args.relay {
        let bind = std::net::SocketAddr::from(([0, 0, 0, 0], port));
        let name = "rust-dos relay".to_string();
        return rust_dos::net::tunnel::relay::serve(bind, rust_dos::net::tunnel::relay::RelayConfig {
            name,
            password: args.relay_password,
            port,
        });
    }
    #[cfg(windows)]
    leave_explorer_console();
    let config = load_config(&args)?;
    let mut settings = Settings::from_config(&config);
    apply_args(&mut settings, &args)?;
    // Ask rust-dos.com whether there is a newer version while the window
    // opens; not in runs whose screens must come out the same.
    let mut update_check = (settings.version_check && !args.deterministic && args.debug_server.is_none())
        .then(rust_dos::version_check::Check::start);
    // A game to launch: its profile, read now so its memory size is the
    // machine's.
    let (startup_game, memory_mb) = game_to_launch(&args, &config, &settings)?;

    let mut cursor_visible = true;
    let mut last_blink = std::time::Instant::now();
    let blink_interval = Duration::from_millis(500);

    // Initialize Recorder
    // TODO: Make configurable
    let mut recorder = ScreenRecorder::new(15);

    // SDL2 Setup
    // SDL is told what the headset's session needs of the window's OpenGL
    // context (GLX's under X11, or EGL's), and Xlib is made ready for the
    // headset's thread either way, for a headset turned on in the settings
    // later.
    // (A probe of the headset asks SDL for what a headset would.)
    let mut headset = settings.vr.clone();
    if args.vr_probe {
        headset.mode = rust_dos::vr::VrMode::Headset;
    }
    display::before_headset(&headset);
    let sdl_context = sdl2::init()?;
    let video_subsystem = sdl_context.video()?;
    // Without a desktop session (started over SSH, say) SDL draws on the
    // display itself: a standalone headset's own, taken from its
    // compositor, which doesn't get it back when Rust-DOS quits.
    if headset.mode == rust_dos::vr::VrMode::Headset && video_subsystem.current_video_driver().eq_ignore_ascii_case("kmsdrm") {
        return Err("A VR headset needs a desktop session, and there is none here (no WAYLAND_DISPLAY or DISPLAY, \
                    as over SSH): SDL would take the display itself, a standalone headset's own away from its \
                    compositor. Start Rust-DOS in the desktop or as a Steam shortcut, or set WAYLAND_DISPLAY \
                    (desktop mode, often wayland-0) or DISPLAY (Steam's gaming mode, :0) to the session's."
            .into());
    }
    if args.vr_probe {
        display::vr_probe(&video_subsystem, &settings.vr);
        if !args.vr {
            return Ok(());
        }
    }
    // Without a sound device (as in a virtual machine without a sound
    // card) the emulator runs silent.
    let audio_device = sdl_context
        .audio()
        .and_then(|subsystem| Ok((open_audio(&subsystem, settings.audio_buffer.blocksize)?, subsystem)));
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
    let mut display = Display::open(
        &video_subsystem,
        "Rust DOS Emulator",
        &settings,
        &textures,
        args.hidden,
    )?;
    // The picture on a screen in a 3D scene, in a headset or the window.
    let stage_notes = if settings.vr.mode == rust_dos::vr::VrMode::Off {
        Vec::new()
    } else {
        display.open_stage(&settings.vr)
    };
    // Typed text is only wanted in the settings window.
    let text_input = video_subsystem.text_input();
    text_input.stop();
    let host_clipboard = video_subsystem.clipboard();

    // Deterministic mode sets the clock before the machine's BIOS reads it.
    let mut det = args
        .deterministic
        .then(|| rust_dos::deterministic::Deterministic::new(args.start_time.unwrap_or_else(rust_dos::deterministic::default_start)));
    let mut cpu = create_cpu(&args, &config, memory_mb)?;
    apply_keyboard_layout(&mut cpu, settings.keyboard_layout);
    for warning in rust_dos::hardware::configure(&mut cpu, &settings, sdl_keys::detect_layout()) {
        config_warning(&mut cpu, &warning);
    }
    sync_locks(&mut cpu, sdl_context.keyboard().mod_state());
    for warning in cpu.bus.start_lan() {
        config_warning(&mut cpu, &warning);
    }
    // How the sound's channels mix for the viewer in the 3D scene.
    let audio_mix = std::rc::Rc::new(std::cell::Cell::new(display::audio_mix::IDENTITY));
    sdl_audio::attach(&mut cpu, audio_device, &settings, &audio_mix);
    // The 3D scene's own sounds, on a device of their own.
    let mut room_sound = sdl_audio::RoomDevice::new();
    let room_audio = sdl_context.audio().ok();
    cpu.bus.log_string(&format!("[DISPLAY] {}", display.renderer()));
    if let Some(warning) = display.shader_warning() {
        config_warning(&mut cpu, warning);
    }
    for note in &stage_notes {
        eprintln!("{}", note);
        cpu.bus.log_string(note);
    }
    let mut machine = Hardware::of(&settings);
    let mut saved = Saved {
        file: config.source.clone(),
        autoexec: config.autoexec.clone(),
        settings: settings.clone(),
        drives: mounted_drives(&cpu),
    };
    let mut dbg = start_debug_hub(&mut cpu, &args, &det, &settings)?;
    // The keys a game's profile presses, as last stepped in deterministic
    // mode, which steps them in emulated time.
    let mut det_autoinput: Option<rust_dos::autoinput::Status> = None;
    let mut event_pump = sdl_context.event_pump()?;
    // RetroAchievements: the session with the site, and when the game's
    // logic is checked next (emulated PIT ticks, every 1/60 s). Hardcore
    // mode needs the debug server off, which could change memory.
    let mut achievements = Achievements::new(Box::new(HttpTransport::start()), settings.achievements.clone());
    achievements.hardcore_allowed = args.debug_server.is_none();
    let mut next_check = 0u64;

    start_dos(&mut cpu, &config, &args, &startup_game, &mut update_check);

    // The decoded VGA picture stays separate from its overlays. Carry
    // the rows repainted here into composition, which restores those
    // rows and the old and new overlays from this cached picture.
    let mut cached_frame = video::Frame::new(video::SCREEN_WIDTH, video::SCREEN_HEIGHT);
    // What the picture shown last was composed from (see `still` below).
    let mut last_screen_key = None;
    // Whether the OpenGL renderer drew the 3dfx card's picture last frame.
    let mut voodoo_gl_shown = false;
    // The cached render with the cursors on top, as the screen shows it.
    let mut screen = cached_frame.clone();
    let mut composition = video::compose::Composition::default();

    // Paces emulated time against the wall clock, one frame at a time.
    cpu.bus.set_cycles_per_ms(settings.cycles.initial_cycles());
    let mut pacer = timer::Pacer::for_settings(&settings, std::time::Instant::now());

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
    // Programs started with the almanac's settings (`almanac_auto`).
    let mut almanac_auto = main_almanac::AutoApply::default();
    // A game whose ways to start are to be offered (launch.rs).
    let mut choose: Option<String> = None;
    // A game requested outside the settings window while DOS is busy.
    let mut confirm_launch: Option<String> = None;
    // The keys the game's profile presses as it starts, and whether they
    // turned on fast forward to get through a wait.
    let mut autoinput: Option<rust_dos::autoinput::AutoInput> = None;
    let mut autoinput_fast = false;
    // The game's gamepad mapping at work on the first pad, and its action
    // wheel while it is open.
    let mut padmap: Option<rust_dos::padmap::PadMapper> = None;
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
    // The right button held still opening the settings window
    // (`mouse_menu_hold`), for a TV or a remote without F12.
    let mut menu_hold = rust_dos::menu_hold::MenuHold::default();
    // The text selected with the right button while the mouse isn't
    // captured (Ctrl+Shift+C copies it), and the text being pasted
    // (Ctrl+Shift+V).
    let mut clipboard = Clipboard::new();
    // A screenshot to take of the next frame (Ctrl+F5), and the sound being
    // recorded (Ctrl+F6).
    let mut screenshot = false;
    let mut sound_recording: Option<WavWriter> = None;
    // The video being recorded (Ctrl+F7), and when it last got a frame:
    // while the machine stands still it goes on in the host's time.
    let mut video_recording: Option<VideoRecorder> = None;
    let mut video_frame_at = std::time::Instant::now();
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
                choose: &mut choose,
                confirm_launch: &mut confirm_launch,
                autoinput: &mut autoinput,
                autoinput_fast: &mut autoinput_fast,
                padmap: &mut padmap,
                states: &states_root,
                picture: &cached_frame,
                state_done: &state_done,
                slot: &mut slot,
                state_loaded: &mut state_loaded,
                achievements: &mut achievements,
                deterministic: det.as_ref().map(|mode| mode.start),
                clipboard: &host_clipboard,
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
        // One with launch configurations offers them first.
        if games::launch_choices(dir, text).is_some() {
            choose = Some(entry.id.clone());
        } else {
            match host!().start_game(&entry.id, text, dir) {
                Ok(message) => osd.show(message),
                Err(e) => config_warning(&mut cpu, &e),
            }
        }
    }

    // What the settings window's Stats page shows, and the last frame's
    // start and times for it.
    let mut stats = rust_dos::stats::Stats::new();
    let mut last_frame: Option<(std::time::Instant, rust_dos::stats::FrameTimes)> = None;

    // What the VR headset's controllers did last: where the laser met the
    // screen, the mouse's buttons it held, and its menu button's presses.
    let mut vr_pointer: Option<(i32, i32)> = None;
    let mut vr_buttons = [false; 2];
    // The keys of the keyboard in the scene the machine has down.
    let mut vr_keys_held: std::collections::HashSet<(u8, bool)> = Default::default();
    let mut vr_menu = 0u32;
    let mut vfx1_host = rust_dos::vfx1::host::Host::default();
    // Until when the scene's PC shows its hard disk and floppy lights.
    let mut hdd_light = std::time::Instant::now();
    let mut floppy_light = std::time::Instant::now();
    // The running game's manuals, for the booklet in the 3D scene, and the
    // game they are of.
    let mut vr_manuals: (Option<String>, Vec<PathBuf>) = (None, Vec::new());
    // The mouse held in SDL's relative mode while Ctrl+Shift fly the 3D
    // scene's camera, and the start of the last frame's flight.
    let mut steering_mouse = false;
    // The host's pointer hidden over the window while the built-in
    // Windows draws its own cursor in the picture.
    let mut pointer_hidden = false;
    // A click the 3D scene took, whose release the machine doesn't see.
    let mut swallowed_click = false;
    let mut last_steer = std::time::Instant::now();
    // Ctrl+Shift held over the 3D scene: the mouse and Q, E and Home fly
    // its camera, and the machine doesn't see them.
    macro_rules! steering {
        () => {
            display.every_frame() && {
                let keys = sdl_context.keyboard().mod_state();
                keys.intersects(Mod::LCTRLMOD | Mod::RCTRLMOD) && keys.intersects(Mod::LSHIFTMOD | Mod::RSHIFTMOD)
            }
        };
    }
    for note in stage_notes.iter().filter(|n| n.contains("No headset") || n.contains("can't")) {
        osd.show(note.trim_start_matches("[VR] ").to_string());
    }

    // Why the loop ended, for the debug server's journal, and the speed
    // the journal told last.
    let mut shutdown = debug::ShutdownReason::Window;
    let mut told_cycles = settings.cycles;

    // The monitor's power effects: it comes on now, and again at each
    // reset, and syncs anew at each change of frequency.
    let mut power = video::power::PowerFx::default();
    let mut seen_power = (cpu.bus.power_ons, cpu.bus.vga.resyncs);
    cpu.bus.monitor_noise.enabled = settings.monitor_effects.sound();
    power_on(&mut power, &mut cpu.bus, settings.monitor_effects);

    // Main Loop
    'running: loop {
        // What the VR headset's thread has to say.
        for note in display.poll_headset() {
            cpu.bus.log_string(&format!("[VR] {}", note));
            osd.show(note);
        }
        // And the machine.
        for (title, detail) in std::mem::take(&mut cpu.bus.notices) {
            osd.notify(title, detail, true);
        }
        profile::set_host(Zone::Events);
        let frame_start = std::time::Instant::now();
        if let Some((start, times)) = last_frame {
            stats.record(&cpu.bus, rust_dos::stats::FrameTimes { wall: frame_start - start, ..times });
            dbg.set_frame_stats(stats.latest());
        }
        // Whether the window's keyboard or mouse reached the machine this
        // frame, which the debug server's journal tells while it holds it.
        let mouse_before = (cpu.bus.mouse.x, cpu.bus.mouse.y, cpu.bus.mouse.buttons);
        let mut host_keys = false;
        for event in event_pump.poll_iter() {
            // The right button held still for the settings window; its
            // release is swallowed once the window opened.
            match &event {
                Event::MouseButtonDown { mouse_btn: MouseButton::Right, .. }
                    if settings.mouse_menu_hold && !ui.is_open() && !paused && !steering!() =>
                {
                    menu_hold.press(std::time::Instant::now());
                }
                Event::MouseMotion { x, y, xrel, yrel, .. } => {
                    menu_hold.motion(*xrel, *yrel);
                    // A key of the scene's keyboard under the mouse shows
                    // lit, as a click would press it.
                    if !mouse_captured && !ui.is_open() && display.every_frame() {
                        display.hover_scene(*x, *y);
                    }
                }
                Event::MouseButtonUp { mouse_btn: MouseButton::Right, .. } if menu_hold.release() => continue,
                _ => {}
            }
            match event {
                Event::Quit { .. } => break 'running,
                // Ctrl+Shift and the mouse fly the 3D scene's camera: look
                // around, drag (left button) to slide, right button or the
                // wheel to move ahead and back; Home goes back to the start.
                Event::MouseMotion { xrel, yrel, mousestate, .. } if steering!() => {
                    if mousestate.left() {
                        display.camera_pan(xrel as f32, yrel as f32);
                    } else if mousestate.right() {
                        display.camera_dolly(-yrel as f32);
                    } else {
                        display.camera_look(xrel as f32, yrel as f32);
                    }
                }
                Event::MouseButtonDown { .. } | Event::MouseButtonUp { .. } if steering!() => {}
                Event::MouseWheel { y, direction, .. } if steering!() => {
                    let dy = if matches!(direction, MouseWheelDirection::Flipped) { -y } else { y };
                    display.camera_dolly(dy as f32 * 20.0);
                }
                Event::KeyDown { keycode: Some(keycode @ (Keycode::Home | Keycode::Q | Keycode::E)), keymod, repeat, .. }
                    if display.every_frame()
                        && keymod.intersects(Mod::LCTRLMOD | Mod::RCTRLMOD)
                        && keymod.intersects(Mod::LSHIFTMOD | Mod::RSHIFTMOD) =>
                {
                    if keycode == Keycode::Home && !repeat {
                        display.recenter();
                        osd.show(if display.has_headset() { "View centered" } else { "Camera back at the start" });
                    }
                }
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
                        // The machine goes on from where rewinding got to,
                        // as when F11 is released.
                        dbg.state_loaded_elsewhere(&cpu, "rewind");
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
                        display.vr_key_pressed();
                    }
                    // Ctrl+Shift+D degausses the monitor.
                    if keycode == Keycode::D && ctrl && shift && !alt {
                        if !repeat {
                            power.trigger(video::power::Effect::Degauss, std::time::Instant::now());
                            cpu.bus.audio_catch_up();
                            cpu.bus.monitor_noise.play(rust_dos::monitor_noise::Sound::Degauss, 0);
                        }
                        continue;
                    }
                    // Ctrl+Shift+S shows and hides the sound visualizer.
                    if keycode == Keycode::S && ctrl && shift && !alt {
                        if !repeat {
                            let message = ui.sound_viz_key(&mut cpu.bus);
                            osd.show(message);
                        }
                        continue;
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
                                            video_frame_at = std::time::Instant::now();
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
                        // The player takes over from the game's keys.
                        if let Some(mut input) = autoinput.take() {
                            input.stop(&mut cpu);
                        }
                        keyboard::key_event(&mut cpu.bus, scan, extended, true, None);
                        host_keys = true;
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
                        dbg.state_loaded_elsewhere(&cpu, "rewind");
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
                    host_keys = true;
                }

                Event::TextInput { text, .. } if ui.is_open() => ui.text(&text, &mut host!()),

                // A file or folder dropped onto the window.
                Event::DropFile { filename, .. } => {
                    let message = host!().dropped(std::path::Path::new(&filename));
                    if let Some(id) = confirm_launch.take() {
                        if !ui.is_open() {
                            toggle_ui!();
                        }
                        ui.confirm_game_launch(&id);
                    }
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

                // A click on a lamp of the 3D scene (or its camera), in front
                // of the screen.
                Event::MouseButtonDown { mouse_btn: MouseButton::Left, x, y, .. }
                    if !mouse_captured && !ui.is_open() && display.every_frame() && display.click_scene(x, y) =>
                {
                    swallowed_click = true;
                }
                Event::MouseButtonUp { mouse_btn: MouseButton::Left, .. } if std::mem::take(&mut swallowed_click) => {}
                // A click on the sound visualizer's controls.
                Event::MouseButtonDown { mouse_btn: MouseButton::Left, x, y, .. }
                    if !mouse_captured && ui.sound_viz_hit(display.to_frame(x, y)).is_some() =>
                {
                    if let Some(action) = ui.sound_viz_hit(display.to_frame(x, y)) {
                        action.apply(&mut cpu.bus);
                    }
                }
                Event::MouseButtonUp { mouse_btn: MouseButton::Left, x, y, .. }
                    if !mouse_captured && ui.sound_viz_hit(display.to_frame(x, y)).is_some() => {}
                Event::MouseButtonDown { mouse_btn: MouseButton::Left, x, y, .. } if ui.is_open() => {
                    let (fx, fy) = display.to_frame(x, y);
                    ui.click(fx, fy, &mut host!());
                }

                Event::MouseWheel { y, direction, .. } if ui.is_open() => {
                    let dy = if matches!(direction, MouseWheelDirection::Flipped) { -y } else { y };
                    ui.wheel(dy, &mut host!());
                }
                // The game's mapping can bind the mouse's wheel.
                Event::MouseWheel { y, direction, .. }
                    if y != 0
                        && padmap.as_mut().is_some_and(|m| {
                            let up = (y > 0) != matches!(direction, MouseWheelDirection::Flipped);
                            m.mouse_wheel(&mut cpu.bus, up)
                        }) => {}

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
                        let (dx, dy) = settings.mouse_motion(dx, dy);
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

        // The VR headset's controllers: the menu button opens and closes
        // the settings window; the laser is the mouse where it meets the
        // screen, and clicks in the settings window; the rest is a gamepad.
        let mut vr_pad: Option<rust_dos::padmap::PadSnapshot> = None;
        let mut vfx1_worn: Option<rust_dos::vfx1::Tracking> = None;
        match display.vr_input() {
            Some(vr) => {
                vfx1_worn = vr.vfx1;
                for _ in vr_menu..vr.menu_presses {
                    toggle_ui!();
                }
                vr_menu = vr.menu_presses;
                if ui.is_open() {
                    if vr.buttons[0]
                        && !vr_buttons[0]
                        && let Some((fx, fy)) = vr.pointer
                    {
                        ui.click(fx, fy, &mut host!());
                    }
                } else if !paused {
                    if let Some(at) = vr.pointer
                        && vr.pointer != vr_pointer
                    {
                        let (vx, vy) = video::overlay::frame_to_mouse(&cpu.bus, &cached_frame, at);
                        cpu.bus.mouse.set_position(vx, vy);
                    }
                    for (button, (&now, &before)) in vr.buttons.iter().zip(&vr_buttons).enumerate() {
                        if now && !before {
                            cpu.bus.mouse.button_down(button);
                        } else if before && !now {
                            cpu.bus.mouse.button_up(button);
                        }
                    }
                }
                vr_pointer = vr.pointer;
                vr_buttons = vr.buttons;
                vr_pad = vr.pad;
            }
            None => {
                for (button, held) in vr_buttons.iter_mut().enumerate() {
                    if std::mem::take(held) {
                        cpu.bus.mouse.button_up(button);
                    }
                }
            }
        }
        // The keys typed on the keyboard in the scene, as the host's: but
        // in the settings window or paused; those the machine saw go down
        // come up for it.
        for key in display.vr_keys() {
            let id = (key.scan, key.extended);
            if key.down {
                if ui.is_open() || paused {
                    continue;
                }
                if let Some(mut input) = autoinput.take() {
                    input.stop(&mut cpu);
                }
                if clipboard.pasting() {
                    clipboard.stop_paste(&mut cpu);
                }
                vr_keys_held.insert(id);
            } else if !vr_keys_held.remove(&id) {
                continue;
            }
            keyboard::key_event(&mut cpu.bus, key.scan, key.extended, key.down, None);
            host_keys = true;
        }

        if host_keys || (cpu.bus.mouse.x, cpu.bus.mouse.y, cpu.bus.mouse.buttons) != mouse_before {
            dbg.host_input(&cpu);
        }

        // The built-in Windows' cursor is drawn in the picture where the
        // host's pointer is, so that pointer would show twice. The settings
        // window and the 3D scene keep the host's.
        let hide_pointer = !ui.is_open()
            && !display.every_frame()
            && video::overlay::draws_windows_cursor(&cpu.bus);
        if hide_pointer != pointer_hidden {
            sdl_mouse.show_cursor(!hide_pointer);
            pointer_hidden = hide_pointer;
        }

        // The mouse in relative mode while flying, so that it doesn't stop
        // at the window's edges; Q and E move the camera down and up while
        // held.
        let steer = steering!();
        if steer != steering_mouse {
            if !mouse_captured {
                sdl_mouse.set_relative_mouse_mode(steer);
            }
            steering_mouse = steer;
        }
        if steer {
            let keys = event_pump.keyboard_state();
            let metres = last_steer.elapsed().as_secs_f32().min(0.1) * 1.5;
            if keys.is_scancode_pressed(Scancode::E) {
                display.camera_rise(metres);
            }
            if keys.is_scancode_pressed(Scancode::Q) {
                display.camera_rise(-metres);
            }
        }
        last_steer = std::time::Instant::now();

        // Remote debug requests and queued remote input, which goes to the
        // settings window while it is open.
        dbg.divert = ui.is_open();
        dbg.rewinding = rewinding.is_some();
        profile::set_host(Zone::Debug);
        dbg.poll(&mut cpu);
        profile::set_host(Zone::Frontend);
        if dbg.quit_requested() {
            shutdown = debug::ShutdownReason::Quit;
            break 'running;
        }
        if dbg.take_hotkey() {
            toggle_ui!();
        }
        if dbg.take_sound_viz_key() {
            let message = ui.sound_viz_key(&mut cpu.bus);
            osd.show(message);
        }
        if dbg.take_closed_program() {
            host!().release_program();
        }
        // A strict-match load brings a state of this machine and this
        // debugging session: see `load_for_debugger`.
        let mut strict_match_loaded = false;
        let mut debugger_loaded = false;
        while let Some(request) = dbg.next_state_request() {
            let path = request.path.clone();
            // The file layout's version, and this rust-dos's (the header has the one that saved it).
            let (format, emulator) = (slots::FORMAT, env!("CARGO_PKG_VERSION"));
            let result = if request.load {
                let loaded = host!().load_for_debugger(&path, request.strict_match, request.check_files, &request.folders);
                loaded.map(|(header, differences, unchecked)| {
                    let header = slots::reply_header(&header);
                    let mut reply = serde_json::json!({"loaded": path, "format": format, "emulator": emulator, "header": header});
                    if !request.folders.is_empty() {
                        reply["folders"] = debug::folders_json(&request.folders);
                    }
                    strict_match_loaded = request.strict_match;
                    debugger_loaded = true;
                    if !differences.is_empty() {
                        // Told, not refused: the state brought its own.
                        reply["differences"] = differences.into();
                    }
                    if !unchecked.is_empty() {
                        reply["unchecked"] = unchecked.into();
                    }
                    reply
                })
            } else {
                if request.strict_match {
                    savestate::media::forget(&cpu);
                }
                host!()
                    .save_file(&path, request.check_files)
                    .map(|header| {
                        let header = slots::reply_header(&header);
                        serde_json::json!({"saved": path, "format": format, "emulator": emulator, "header": header})
                    })
                    .map_err(|e| (400, serde_json::json!({"error": e})))
            };
            // The front end's part is done; the hub's (the debugger reset of
            // a strict-match load, the journal and the reply) follows.
            dbg.state_done(&cpu, request, result);
        }
        while let Some(request) = dbg.next_speed_request() {
            let result = CpuSpeed::parse(&request.cycles).and_then(|cycles| {
                if det.is_some() && !matches!(cycles, CpuSpeed::Fixed(_)) {
                    return Err("deterministic mode runs at a fixed speed: give a number of cycles".to_string());
                }
                let new = Settings { cycles, ..settings.clone() };
                host!().apply(&new)?;
                Ok(serde_json::json!({"cycles": cycles.to_string()}))
            });
            dbg.speed_done(&cpu, request, result);
            told_cycles = settings.cycles;
        }
        for input in dbg.take_ui_input() {
            match input {
                debug::UiInput::Key(key) => ui.key(key, &mut host!()),
                debug::UiInput::Click(x, y) => ui.click(x, y, &mut host!()),
            }
        }

        // A save state loaded: the keys held go up, and a video recording
        // stops, as its time would jump. A strict-match load keeps the keys
        // and mouse buttons the state was saved with: the keys held before it are
        // forgotten without a key-up reaching the restored machine.
        if std::mem::take(&mut state_loaded) {
            // The debug server's loads are in its journal already.
            if !debugger_loaded {
                dbg.state_loaded_elsewhere(&cpu, "window");
            }
            // Deterministic mode runs at the speed the state brought.
            if settings.cycles != told_cycles {
                told_cycles = settings.cycles;
                dbg.speed_changed(&cpu, &told_cycles.to_string(), "load");
            }
            // Deterministic mode's clock and ticks go on from the state's
            // emulated time.
            if let Some(mode) = &mut det {
                mode.resync(&cpu);
            }
            // The Stats page's emulated speed leaves out the jump.
            stats.clock_moved(&cpu.bus);
            rewinder.clear();
            achievements.reset();
            if strict_match_loaded {
                held.clear();
            } else {
                release_input(&mut cpu, &mut held);
                dbg.release_keys(&mut cpu);
            }
            clipboard.stop_paste(&mut cpu);
            if let Some(video) = video_recording.take() {
                match video.stop() {
                    Ok(frames) => osd.show(format!("The video recording stopped at the load ({} frames)", frames)),
                    Err(e) => osd.show(format!("The video recording failed: {}", e)),
                }
            }
        }

        // The right button held still long enough opens the settings
        // window, with the button let go on the machine.
        if menu_hold.poll(std::time::Instant::now()) && !ui.is_open() {
            if capturing_click == Some(MouseButton::Right) {
                capturing_click = None;
            } else if cpu.bus.mouse.buttons & 2 != 0 {
                cpu.bus.mouse.button_up(1);
            }
            toggle_ui!();
        }

        // The window opening or closing: it takes the keyboard and mouse
        // from the machine, which pauses but for the Mixer page, or while in
        // a LAN room (see the batch below).
        if ui.is_open() != ui_shown {
            ui_shown = ui.is_open();
            dbg.settings_window(&cpu, ui_shown);
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
        // The keys the game's profile presses, once its program has
        // started, run fast through their waits unless the player already
        // runs it fast.
        if !waiting && !ui.is_open() {
            // Deterministic mode steps them at its ticks (below).
            let status = match det {
                Some(_) => det_autoinput,
                None => step_autoinput(&mut cpu, &mut game, &mut autoinput),
            };
            let fast = status == Some(rust_dos::autoinput::Status::Busy { fast_forward: true });
            if fast && !cpu.bus.mixer.fast_forward {
                pacer.set_fast_forward(true, &cpu.bus.clock, std::time::Instant::now());
                cpu.bus.mixer.fast_forward = true;
                autoinput_fast = true;
            } else if !fast && autoinput_fast {
                pacer.set_fast_forward(false, &cpu.bus.clock, std::time::Instant::now());
                cpu.bus.mixer.fast_forward = false;
                autoinput_fast = false;
            }
        }
        // The values frozen on the Cheats page, as the program left them
        // (in deterministic mode, at its ticks).
        if det.is_none() {
            cpu.bus.apply_freezes();
        }
        // The controllers as they are now; at rest while the machine waits.
        // A game with a gamepad mapping plays the first pad with it.
        let mapping = game.as_ref().and_then(|g| g.pad.clone());
        if mapping.as_ref() != padmap.as_ref().map(|m| m.mapping()) {
            if let Some(mut old) = padmap.take() {
                old.release(&mut cpu.bus);
            }
            padmap = mapping.map(rust_dos::padmap::PadMapper::new);
        }
        let mut wheel_view: Option<rust_dos::padmap::WheelView> = None;
        // A VR headset's controllers, while they are a gamepad, are the
        // first pad, before the host's.
        // Deterministic mode has none: they would change the machine at
        // the host's frames.
        let first_pad = vr_pad.or_else(|| controllers.first().map(pad_snapshot)).filter(|_| det.is_none());
        for slot in 0..2 {
            if slot == 0
                && let Some(mapper) = &mut padmap
            {
                match first_pad {
                    Some(snapshot) if !waiting && !ui.is_open() => {
                        if snapshot.inputs() != 0
                            && let Some(mut input) = autoinput.take()
                        {
                            input.stop(&mut cpu);
                        }
                        wheel_view = mapper.update(&mut cpu.bus, snapshot.inputs(), snapshot.pointer());
                    }
                    _ => mapper.release(&mut cpu.bus),
                }
                cpu.bus.joystick.set_pad(0, mapper.joystick());
                continue;
            }
            let pad = match (slot, vr_pad) {
                (0, Some(vr)) => Some(rust_dos::vr::joystick(&vr)),
                _ => controllers.get(slot).map(pad_state),
            };
            let pad = pad.map(|pad| if waiting { joystick::PadState::default() } else { pad }).filter(|_| det.is_none());
            cpu.bus.joystick.set_pad(slot, pad);
        }
        // The VFX1 on the desk of the 3D scene, and the head wearing it.
        display.set_vfx1(settings.vfx1.enabled);
        vfx1_host.track(&mut cpu.bus, vfx1_worn, &settings.vfx1, waiting || det.is_some());
        cpu.bus.set_voodoo_fps_cap(settings.voodoo.fps_cap);
        cpu.bus.set_voodoo_texture_sampling(settings.voodoo.texture_sampling);
        cpu.bus.set_powervr_filter(settings.powervr_filter);
        // With a variable refresh rate, each frame runs to a vertical
        // retrace of the machine's display and is shown when it is due,
        // so the window refreshes at the machine's rate, where the host's
        // display goes that fast; under the 3dfx frame rate cap, at the
        // cap's frame times, which the card's swaps keep to.
        let refresh = (settings.vrr && !waiting && det.is_none())
            .then(|| cpu.bus.voodoo_cap_timing().unwrap_or_else(|| cpu.bus.refresh_timing()))
            .filter(|timing| display.shows_hz(timing.hz()));
        let batch_end = if waiting {
            cpu.bus.clock.icount
        } else if let Some(refresh) = &refresh {
            pacer.retrace_batch_end(&cpu.bus.clock, refresh, batch_start)
        } else {
            pacer.batch_end(&cpu.bus.clock, batch_start)
        };
        // Deterministic mode runs whole ticks of emulated time.
        let batch_end = match &det {
            Some(mode) => mode.frame_end(&cpu, batch_end),
            None => batch_end,
        };
        if det.is_none() {
            cpu.bus.start_batch(batch_end);
        }
        let batch_icount = cpu.bus.clock.icount;
        let batch_stalled = cpu.bus.clock.stalled;
        let batch_idle = cpu.bus.clock.idle;
        // The instructions the dynamic recompiler's code runs, of all.
        let batch_translated = cpu.dynrec.counts().executed;

        // Per-instruction debug hook (breakpoints / stepping / tracing) is
        // only consulted when something actually needs it.
        let dbg_hot = dbg.begin_batch(&cpu);
        if let Some(mode) = &mut det {
            // A millisecond of emulated time at a time, with the input, the
            // frozen values and the profile's keys at its ticks.
            // Achievements are checked at the first millisecond of each
            // 1/60 s, fast forwarding or not.
            let ui_open = ui.is_open();
            let checking = achievements.checking();
            if !waiting {
                dbg.run_deterministic(&mut cpu, mode, dbg_hot, batch_end, |cpu, dbg, reached| {
                    if reached.tick && !ui_open {
                        dbg.feed_input(cpu);
                        cpu.bus.apply_freezes();
                        det_autoinput = step_autoinput(cpu, &mut game, &mut autoinput);
                    }
                    let now = cpu.bus.clock.now_ticks();
                    if checking && now >= next_check {
                        achievements.do_frame(cpu.bus.ram(), cpu.bus.boot.is_some());
                        let frame = timer::frame_ticks();
                        next_check = if next_check + frame <= now { now + frame } else { next_check + frame };
                    }
                });
            }
        } else if achievements.checking() && !waiting {
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
        profile::set_host(Zone::Debug);
        dbg.end_batch(&cpu);
        profile::set_host(Zone::Frontend);
        let translated = cpu.dynrec.counts().executed.saturating_sub(batch_translated);

        // The scene's PC lights its lights as the machine's would: the hard
        // disk's and the floppy's while they are read or written (and a
        // little after, to be seen), the turbo light while the CPU is fast.
        // The sound comes from where the scene's speakers are.
        if display.every_frame() {
            let now = std::time::Instant::now();
            let active = std::mem::take(&mut cpu.bus.drives_active);
            for drive in (0..32u8).filter(|&d| active & 1 << d != 0) {
                match cpu.bus.disk.drive_kind(drive) {
                    Some(disk::DriveKind::Floppy) => floppy_light = now + Duration::from_millis(120),
                    Some(disk::DriveKind::CdRom | disk::DriveKind::Virtual) => {}
                    _ => hdd_light = now + Duration::from_millis(120),
                }
            }
            display.set_capture_dir(&settings.capture_dir);
            let locks = keyboard::lock_lights(&cpu.bus);
            display.set_leds(rust_dos::vr::Leds {
                power: true,
                turbo: cpu.bus.clock.cycles_per_ms() > rust_dos::vr::TURBO_CYCLES,
                hdd: now < hdd_light,
                floppy: now < floppy_light,
                num: locks & 0x02 != 0,
                caps: locks & 0x04 != 0,
                scroll: locks & 0x01 != 0,
                keys: cpu.bus.kbd.held_keys(),
            });
            if vr_manuals.0.as_deref() != game.as_ref().map(|g| g.id.as_str()) {
                vr_manuals.0 = game.as_ref().map(|g| g.id.clone());
                vr_manuals.1 = match (&game, games_dir(saved.file.as_deref())) {
                    (Some(game), Some(dir)) => {
                        let text = std::fs::read_to_string(dir.join(format!("{}.conf", game.id))).unwrap_or_default();
                        let home = rust_dos::hostdirs::home_dir();
                        games::manuals(&dir, &game.id, &text, home.as_deref()).into_iter().map(|m| m.path).collect()
                    }
                    _ => Vec::new(),
                };
            }
            display.set_manuals(&vr_manuals.1);
        }
        let mix = display.audio_mix().filter(|_| settings.vr.spatial_audio);
        audio_mix.set(mix.unwrap_or(display::audio_mix::IDENTITY));
        display.room_sound(&mut room_sound.sound.lock().unwrap_or_else(|e| e.into_inner()));
        room_sound.update(room_audio.as_ref());

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
            profile::set_host(Zone::Rewind);
            rewinder.offer(now_ns, savestate::machine::save(&cpu));
            profile::set_host(Zone::Frontend);
            next_capture = (now_ns + REWIND_INTERVAL_NS, std::time::Instant::now());
        }
        if let Some((started, frames)) = &mut rewinding {
            *frames += 1;
            if *frames % 3 == 1 {
                let back = |at: u64| (started.saturating_sub(at)) as f64 / 1e9;
                match rewinder.step_back() {
                    Some((at, state)) => match savestate::machine::load(&mut cpu, &state) {
                        Ok(()) => {
                            if let Some(mode) = &mut det {
                                mode.resync(&cpu);
                            }
                            osd.show_lasting(format!("Rewind -{:.1} s", back(at)))
                        }
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
            shutdown = debug::ShutdownReason::Exit;
            break 'running;
        }
        // A speed the settings window or a hotkey set.
        if settings.cycles != told_cycles {
            told_cycles = settings.cycles;
            dbg.speed_changed(&cpu, &told_cycles.to_string(), "window");
        }
        // Pages printed, and printing that failed.
        for notice in cpu.bus.printer_notices() {
            osd.show(notice);
        }
        for notice in std::mem::take(&mut cpu.bus.disk_notices) {
            osd.show(notice);
        }

        // Del during the POST asks for the BIOS setup.
        if std::mem::take(&mut cpu.bus.setup_requested) && !ui.is_open() {
            let current = settings.clone();
            ui.open_setup(&current, saved.file.clone(), &host!());
        }
        cpu.post_mode = settings.post;
        // DOSCONFIG asks for the settings window.
        if std::mem::take(&mut cpu.bus.config_ui_requested) && !ui.is_open() {
            toggle_ui!();
        }
        // MIXER changed the mixer: the settings have it, to show and save.
        if std::mem::take(&mut cpu.bus.mixer_changed) {
            settings.mixer = cpu.bus.mixer.settings();
            ui.sync_mixer(settings.mixer);
        }
        // A program from the prompt waiting for the almanac's settings, and
        // one that played with them and has ended.
        let window_open = ui.is_open();
        if let Some(notice) = almanac_auto.step(&mut host!(), window_open) {
            osd.show(notice);
        }
        // A launched game that has ended: the settings and drives before it
        // come back.
        if !ui.is_open()
            && let Some(ended) = game.take_if(|g| g.done(&cpu))
        {
            let name = ended.name.clone();
            if let Some(mut input) = autoinput.take() {
                input.stop(&mut cpu);
            }
            // After a tool of the game's, the ways to start it again.
            if ended.choose_after {
                choose = Some(ended.id.clone());
            }
            host!().end_game(ended);
            osd.show(format!("{} has ended: your settings are back", name));
        }
        // A game launched with ways to start it: the window offers them.
        if let Some(id) = choose.take() {
            if !ui.is_open() {
                toggle_ui!();
            }
            if !ui.show_launch(&id, &host!()) {
                ui.close();
            }
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
        // A newer rust-dos, once the website has answered.
        if let Some(check) = &mut update_check {
            if let Some(version) = check.news() {
                let (title, detail) = rust_dos::version_check::notification(&version);
                cpu.bus.log_string(&format!("[VERSION] {}. {}", title, detail));
                osd.notify(title, detail, true);
            }
            if check.finished() {
                update_check = None;
            }
        }
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
        profile::set_host(Zone::AudioPump);
        let samples = pump_audio(&mut cpu.bus, waiting);
        profile::set_host(Zone::Frontend);
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
        if let Some(mode) = &det {
            let visible = mode.blink_visible(&cpu);
            if visible != cursor_visible {
                cursor_visible = visible;
                cpu.bus.vga.set_blink(visible);
            }
        } else if last_blink.elapsed() >= blink_interval {
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
        // recordings, overlays that mix with it, and the debugger in
        // deterministic mode. Otherwise the debugger sees what OpenGL
        // drew, read back (`voodoo_picture` below).
        let voodoo_picture = !voodoo_gl_shown
            || screenshot
            || recorder.is_active()
            || video_recording.is_some()
            || ui.is_open()
            || ui.overlay_shown()
            || (dbg.wants_frame() && det.is_some())
            || settings.monochrome.phosphor().is_some();
        if let Some(v) = &mut cpu.bus.voodoo {
            v.set_software_picture(voodoo_picture);
        }
        profile::set_host(if cpu.bus.voodoo.is_some() {
            Zone::VoodooFrame
        } else {
            Zone::FrameCompose
        });
        cpu.bus.sync_display();
        let voodoo_gl = display.run_voodoo(&mut cpu.bus, &settings.voodoo, det.is_none());
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
        profile::set_host(Zone::VgaRender);
        let render_start = std::time::Instant::now();
        let rendered = cpu.bus.vga.dirty;
        let changed = if cpu.bus.vga.dirty {
            let rows = video::render_screen_rows(&mut cached_frame, &cpu.bus);
            cpu.bus.vga.clear_dirty();
            rows
        } else {
            0..0
        };
        let render_time = render_start.elapsed();
        profile::set_host(Zone::FrameCompose);
        // Each eye's picture, for the VFX1 worn in the headset.
        let worn = vfx1_worn.is_some();
        if let Some(eyes) = vfx1_host.eyes(&cpu.bus, &cached_frame, &settings.vfx1, worn) {
            display.show_vfx1_eyes(&eyes.left, &eyes.right, settings.vfx1.view_size as f32);
        }

        // The picture shown already, where nothing in it changed: neither
        // the machine's nor anything drawn over it. Composing it, comparing
        // it with the one shown and drawing it again would cost a weak
        // host's memory bandwidth every frame for nothing.
        let screen_key = (
            video::overlay::cursors_key(&cpu.bus, cursor_visible),
            settings.monochrome,
            clipboard.draws(&cpu.bus),
            osd.is_empty(),
        );
        // The monitor follows the machine: a reset is a power on, another
        // frequency a resync.
        cpu.bus.monitor_noise.enabled = settings.monitor_effects.sound();
        let now_power = (cpu.bus.power_ons, cpu.bus.vga.resyncs);
        if now_power.0 != seen_power.0 {
            power_on(&mut power, &mut cpu.bus, settings.monitor_effects);
        } else if now_power.1 != seen_power.1 {
            resync(&mut power, &mut cpu.bus, settings.monitor_effects);
        }
        seen_power = now_power;
        let power_now = std::time::Instant::now();
        let power_look = power.active(power_now).then(|| power.look(power_now)).flatten();

        let still = !rendered
            && power_look.is_none()
            && last_screen_key == Some(screen_key)
            && !voodoo_gl
            && !ui.is_open()
            && !ui.overlay_shown()
            && !ui.sound_viz_shown()
            && wheel_view.is_none()
            && osd.is_empty()
            && !screenshot
            && !recorder.is_active()
            && sound_recording.is_none()
            && video_recording.is_none()
            && !dbg.wants_frame()
            && !display.wants_redraw();
        ui.set_display(display.output_scale(), true);
        let mut composed_rows = 0..0;
        if !still {
            screen.track_overlays(settings.shader == video::shader::Shader::Xbrz);
            // Restore the affected base and overlay rows. A monochrome
            // monitor shows the cursors in its phosphor's colour, but not
            // the settings window.
            let overlay = if clipboard.draws(&cpu.bus)
                || ui.is_open()
                || ui.overlay_shown()
                || ui.sound_viz_shown()
                || power_look.is_some()
                || wheel_view.is_some()
                || !osd.is_empty()
                || recorder.is_active()
                || sound_recording.is_some()
                || video_recording.is_some()
            {
                0..height as usize
            } else {
                video::overlay::cursor_rows(&cached_frame, &cpu.bus, cursor_visible)
            };
            composed_rows = composition.restore(
                &mut screen,
                &cached_frame,
                changed,
                overlay,
                settings.monochrome,
            );
            // (The picture stays all there at half the size, which the
            // window may show instead, only where nothing goes over it.)
            screen.doubled &= !video::overlay::draws_cursors(&cpu.bus)
                && !clipboard.draws(&cpu.bus)
                && !ui.is_open()
                && !ui.overlay_shown()
                && !ui.sound_viz_shown()
                && power_look.is_none()
                && wheel_view.is_none()
                && osd.is_empty()
                && !recorder.is_active()
                && sound_recording.is_none()
                && video_recording.is_none();
            video::overlay::draw_cursors(&mut screen, &cpu.bus, cursor_visible);
            video::mono::apply_rows(&mut screen, settings.monochrome, composed_rows.clone());
            if let Some(look) = &power_look {
                video::power::apply(&mut screen, look);
                screen.mark_overlay_rect(0, 0, width as usize, height as usize);
            }
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
                        .then(|| display.shaded(base, &cached_frame))
                        .flatten();
                    let picture = shaded.as_ref().unwrap_or(base);
                    recorder.capture(base);
                    if let Some(video) = &mut video_recording {
                        let now = std::time::Instant::now();
                        let since = now.duration_since(std::mem::replace(&mut video_frame_at, now));
                        let recorded = if waiting {
                            video.record_still(picture, since.as_nanos() as u64)
                        } else {
                            video.record(picture, samples.clone(), cpu.bus.clock.now_ns())
                        };
                        if !recorded {
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
            // A stopped animation recording finishes writing on its own
            // thread; say so if that failed.
            if let Some(e) = recorder.finished() {
                osd.show(e);
            }
            let capturing = screenshot || recorder.is_active() || video_recording.is_some();
            if capturing && !record_ui {
                capture!();
            }
            profile::set_host(Zone::Ui);
            if ui.is_open() {
                ui.set_mixer_status(cpu.bus.mixer.muted, cpu.bus.mixer.take_peaks());
                ui.poll(&mut host!());
            }
            if ui.is_open() || ui.overlay_shown() {
                ui.set_stats(stats.view());
            }
            if ui.is_open() || ui.overlay_shown() || ui.sound_viz_shown() {
                composition.include_overlay(0..height as usize);
                composed_rows = 0..height as usize;
            }
            ui.draw(&mut screen);
            ui.draw_overlay(&mut screen);
            if ui.sound_viz_shown() {
                ui.set_sound(&cpu.bus);
                ui.draw_sound_viz(&mut screen);
            }
            profile::set_host(Zone::FrameCompose);
            if let Some(view) = &wheel_view {
                rust_dos::config_ui::wheel::draw(&mut screen, view);
            }
            if capturing && record_ui {
                capture!();
            }
            macro_rules! debug_frame {
                () => {
                    let paged = ui.with_layer(&screen, (1.0, 1.0));
                    let frame = paged.as_ref().unwrap_or(&screen);
                    if dbg.wants_shader_frame() {
                        let shaded = display.shaded(frame, &cached_frame);
                        dbg.capture_frame_with_shader(frame, shaded.as_ref());
                    } else {
                        dbg.capture_frame(frame);
                    }
                };
            }
            // In deterministic mode the debug server's pictures leave out
            // the messages, which come and go with the host's time.
            if det.is_some() {
                debug_frame!();
            }
            if !osd.is_empty() {
                composition.include_overlay(0..height as usize);
                composed_rows = 0..height as usize;
            }
            osd.draw(&mut screen);
            // While OpenGL shows the 3dfx card's picture, the debugger's
            // is read back from it once it is drawn (below).
            if det.is_none() && !voodoo_gl {
                debug_frame!();
            }

            // Draw Recording Indicator
            if recorder.is_active() || sound_recording.is_some() || video_recording.is_some() {
                draw_recording_indicator(&mut screen, frame_w);
            }
        }
        // Waiting for the deadline of a frame paced to the retrace is
        // neither the frame's work nor time the CPU could have had.
        profile::set_host(Zone::FrameWait);
        let waited = pacer.wait_to_present(&cpu.bus.clock);
        profile::set_host(Zone::FramePresent);
        if still {
            dbg.count_frame();
            // The 3D scene is drawn every frame: the viewer moves.
            display.refresh_stage();
        } else {
            display.present_rows(
                &screen,
                voodoo_gl.then_some(&cached_frame),
                ui.layer(),
                composed_rows,
                (settings.shader == video::shader::Shader::Xbrz).then_some(&cached_frame),
            )?;
            last_screen_key = Some(screen_key);
            if det.is_none() && voodoo_gl {
                // The frame counts for the debugger's fps whether or not
                // anything wants its picture.
                if !dbg.wants_frame() {
                    dbg.count_frame();
                } else {
                    match display.voodoo_picture() {
                        Some(picture) => dbg.capture_frame(&picture),
                        None => dbg.capture_frame(&screen),
                    }
                }
            }
        }

        profile::set_host(Zone::Frontend);
        let overhead = frame_start.elapsed().saturating_sub(exec_time + waited);
        if let Some(cycles) = pacer.end_cpu_frame(&mut cpu, executed, exec_time, overhead) {
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
        profile::set_host(Zone::FrameWait);
        pacer.wait_for_next_frame();
    }
    shut_down(
        &args,
        &mut dbg,
        &mut cpu,
        &settings,
        &mut power,
        &mut display,
        &screen,
        shutdown,
        video_recording,
        sound_recording,
    )
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

/// What happens once the machine stops: the profile is written, the
/// monitor goes off, the debug server's clients are told, and the printer's
/// page, the shared disks and the recordings are finished.
#[allow(clippy::too_many_arguments)]
fn shut_down(
    args: &Args,
    dbg: &mut debug::DebugHub,
    cpu: &mut Cpu,
    settings: &Settings,
    power: &mut video::power::PowerFx,
    display: &mut Display<'_>,
    screen: &video::Frame,
    shutdown: debug::ShutdownReason,
    video_recording: Option<VideoRecorder>,
    sound_recording: Option<WavWriter>,
) -> Result<(), String> {
    // The profile ends with the machine's last frame, before the monitor's
    // animation, which isn't the machine's work.
    if let Some(path) = &args.profile {
        match dbg.profile_write(cpu, path) {
            Ok(files) => {
                let names: Vec<String> = files.iter().map(|f| f.display().to_string()).collect();
                println!("Profile written to {}", names.join(", "));
            }
            Err(e) => eprintln!("The profile can't be written to {}: {}", path.display(), e),
        }
    }
    // The monitor goes off: the picture collapses to a dot, with a thunk.
    if settings.monitor_effects != video::power::MonitorEffects::Off {
        power_off(power, display, screen, &mut cpu.bus, settings.monitor_effects)?;
    }

    // The loops of a program still running get their summaries.
    rust_dos::win32::trace::flush(cpu);
    cpu.bus.flush_log();

    // The journal ends, and the debug server's clients get what they
    // waited for.
    dbg.shutdown(cpu, shutdown, 0);

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

/// The red dot in the top right corner that says something is being
/// recorded.
fn draw_recording_indicator(screen: &mut video::Frame, frame_w: usize) {
    let radius = 5;
    let center_x = frame_w - 15;
    let center_y = 15;

    for y in (center_y - radius)..=(center_y + radius) {
        for x in (center_x - radius)..=(center_x + radius) {
            let dx = x as isize - center_x as isize;
            let dy = y as isize - center_y as isize;
            if dx * dx + dy * dy <= (radius * radius) as isize {
                let idx = (y * frame_w + x) * 4;
                if idx + 2 < screen.rgba.len() {
                    screen.mark_overlay(x, y);
                    screen.rgba[idx] = 0xFF; // R
                    screen.rgba[idx + 1] = 0x00; // G
                    screen.rgba[idx + 2] = 0x00; // B
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Args, open_log_file};
    use clap::Parser;

    #[test]
    fn the_log_goes_to_the_file_log_names() {
        let path = std::env::temp_dir().join(format!("rust-dos-log-option-{}", std::process::id())).join("run.log");
        let args = Args::try_parse_from(["rust-dos", "--no-config", "--log", path.to_str().unwrap()]).unwrap();
        assert_eq!(args.log.as_deref(), Some(path.as_path()));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "previous run\n").unwrap();
        let mut log = open_log_file(args.log.as_deref()).unwrap().expect("log file");
        log.write_line("[DOS] this run");
        // Dropping the log flushes and closes the file, so the directory
        // can be removed on Windows.
        drop(log);
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
        assert!(text.starts_with("rust-dos "), "{text}");
        assert!(text.ends_with("[DOS] this run\n"), "{text}");
        assert!(!text.contains("previous run"), "{text}");
    }

    #[test]
    fn a_log_file_that_cannot_be_created_is_an_error() {
        // A directory where the file should be.
        let path = std::env::temp_dir().join(format!("rust-dos-log-option-dir-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        let result = open_log_file(Some(&path));
        std::fs::remove_dir_all(&path).unwrap();
        let error = result.err().expect("an error");
        assert!(error.contains("--log"), "{error}");
    }
}
