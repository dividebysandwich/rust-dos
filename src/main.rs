use clap::Parser;
use sdl2::event::Event;
use sdl2::keyboard::{Keycode, Mod};
use sdl2::mouse::{MouseButton, MouseWheelDirection};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::Duration;

use crate::audio::pump_audio;
use crate::config::{Settings, SoundConfig};
use crate::config_ui::{ConfigUi, Host, UiKey};
use crate::cpu::{Cpu, CpuModel};
use crate::disk::{DriveInfo, DriveKind, LASTDRIVE};
use crate::display::Display;
use crate::mount::{MountCmd, MountSpec};
use crate::recorder::ScreenRecorder;
use crate::timer::CpuSpeed;
use crate::video::adapter::VideoSetup;

mod debug;
mod display;
mod sdl_keys;

// The emulator itself is the library crate; the debug server and the
// window's display are private to the binary. These re-exports let the
// binary's modules refer to the library modules as `crate::...`.
use rust_dos::{audio, config, config_ui, cpu, disk, exec, keyboard, mount, recorder, shell, sound, timer, video};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Window scale factor [default: 1, or the config file's scale]
    #[arg(short, long, value_parser = clap::value_parser!(u32).range(1..=16))]
    scale: Option<u32>,

    /// Root directory for Drive C: [default: the config file's C:, or "."]
    #[arg(short, long)]
    dir: Option<String>,

    /// Configuration file to use instead of ./rust-dos.conf or the
    /// per-user default
    #[arg(short, long, value_name = "FILE", conflicts_with = "no_config")]
    config: Option<std::path::PathBuf>,

    /// Don't read or create any configuration file
    #[arg(long)]
    no_config: bool,

    /// Start the HTTP/WebSocket debug server (local-only, unauthenticated).
    /// Optionally takes the listen address.
    #[arg(long, value_name = "ADDR", num_args = 0..=1, default_missing_value = "127.0.0.1:8086")]
    debug_server: Option<std::net::SocketAddr>,

    /// Instruction trace ring buffer capacity (entries, ~64 bytes each).
    /// Allocated only once tracing is enabled via the debug server.
    #[arg(long, default_value_t = 1_000_000)]
    trace_capacity: usize,

    /// Emulated CPU speed in instructions per millisecond, or "max" for as
    /// fast as the host keeps up with [default: max, or the config file's
    /// cycles]
    #[arg(long, value_name = "N|max", value_parser = timer::CpuSpeed::parse)]
    cycles: Option<timer::CpuSpeed>,
}

/// The processor and sound hardware in place. Changed settings reach them
/// only while no program runs (see `apply_machine`).
struct Machine {
    cpu: CpuModel,
    sound: SoundConfig,
    video: VideoSetup,
}

impl Machine {
    fn differs(&self, settings: &Settings) -> bool {
        self.cpu != settings.cpu || self.sound != settings.sound || self.video != settings.video_setup()
    }
}

/// The SDL sound device, where the mixed output goes.
struct SdlAudio(sdl2::audio::AudioQueue<i16>);

impl audio::AudioOutput for SdlAudio {
    fn queued_frames(&self) -> usize {
        self.0.size() as usize / 4
    }

    fn queue(&mut self, samples: &[i16]) -> Result<(), String> {
        self.0.queue_audio(samples)
    }
}

/// What saving the settings compares against: the settings and drives as
/// the configuration file has them, from startup or the last save. Only
/// what changed since is written, so command-line options, drives that
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
    let config = load_config(&args)?;
    let mut settings = Settings::from_config(&config);
    if let Some(scale) = args.scale {
        settings.scale = scale;
    }
    if let Some(cycles) = args.cycles {
        settings.cycles = cycles;
    }

    let mut cursor_visible = true;
    let mut last_blink = std::time::Instant::now();
    let blink_interval = Duration::from_millis(500);

    // Initialize Recorder
    // TODO: Make configurable
    let mut recorder = ScreenRecorder::new(15);

    // SDL2 Setup
    let sdl_context = sdl2::init()?;
    let video_subsystem = sdl_context.video()?;
    let audio_subsystem = sdl_context.audio()?;
    let desired_spec = sdl2::audio::AudioSpecDesired {
        freq: Some(44100),
        channels: Some(2),
        samples: None,     // Default buffer size
    };
    let audio_device = audio_subsystem
        .open_queue::<i16, _>(None, &desired_spec)
        .map_err(|e| e.to_string())?;
    audio_device.resume();

    // The picture has the size the video mode gives it; the display scales
    // it to the window. The textures are for SDL's renderer, which draws
    // where there is no OpenGL 3.
    let textures = std::cell::OnceCell::new();
    let mut display = Display::open(&video_subsystem, "Rust DOS Emulator", &settings, &textures)?;
    // Typed text is only wanted in the settings window.
    let text_input = video_subsystem.text_input();
    text_input.stop();

    let mut cpu = create_cpu(&args, &config);
    cpu.model = settings.cpu;
    video::bios::install(&mut cpu.bus, settings.video_setup());
    cpu.bus.set_disk_settings(settings.disk);
    cpu.bus.set_mixer(settings.mixer);
    for warning in sound::apply_config(&mut cpu, &settings.sound, None) {
        config_warning(&mut cpu, &warning);
    }
    cpu.bus.audio_device = Some(Box::new(SdlAudio(audio_device)));
    cpu.bus.log_string(&format!("[DISPLAY] {}", display.renderer()));
    if let Some(warning) = display.shader_warning() {
        config_warning(&mut cpu, warning);
    }
    let mut machine = Machine { cpu: settings.cpu, sound: settings.sound.clone(), video: settings.video_setup() };
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

    // Load Shell Code into Memory
    cpu.load_shell();
    print_banner(&mut cpu, &config, args.no_config);

    // Startup commands: the config's [autoexec] lines, then AUTOEXEC.BAT
    // from the C: root if there is one, like the startup sequence a real PC
    // would run. Each line runs as if typed at the prompt.
    cpu.queue_batch_lines(&config.autoexec);
    cpu.queue_batch_file("C:\\AUTOEXEC.BAT");

    // Cached render target. We re-render the full VGA surface only when
    // `cpu.bus.vga.dirty` is set — everything else (cursor blink, mouse
    // cursor, recording indicator) is overlaid on top of this buffer each
    // frame. For a program that isn't actively touching VRAM, the per-SDL-
    // frame cost drops from "640×400×3 zero fill + per-pixel palette/planar
    // lookup" to "one memcpy of the cached buffer + a tiny overlay pass".
    let mut cached_frame = video::Frame::new(video::SCREEN_WIDTH, video::SCREEN_HEIGHT);
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
    let mut held: HashMap<Keycode, (u8, bool)> = HashMap::new();

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
            }
        };
    }
    macro_rules! toggle_ui {
        () => {
            if ui.is_open() {
                ui.close();
            } else {
                let file = saved.file.clone();
                let current = settings.clone();
                ui.open(&current, file, &host!());
            }
        };
    }

    // Main Loop
    'running: loop {
        let frame_start = std::time::Instant::now();
        for event in event_pump.poll_iter() {
            match event {
                Event::Quit { .. } => break 'running,
                Event::KeyDown {
                    keycode: Some(keycode),
                    keymod,
                    repeat,
                    ..
                } => {
                    // Ctrl+F12 opens and closes the settings window. (Not
                    // with Alt: AltGr can arrive as Ctrl+Alt.)
                    let ctrl = keymod.intersects(Mod::LCTRLMOD | Mod::RCTRLMOD);
                    let alt = keymod.intersects(Mod::LALTMOD | Mod::RALTMOD);
                    if keycode == Keycode::F12 && ctrl && !alt {
                        if !repeat {
                            toggle_ui!();
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
                    if ui.is_open() {
                        if let Some(key) = ui_key(keycode, keymod) {
                            ui.key(key, &mut host!());
                        }
                        continue;
                    }
                    // A key still held from the settings window repeats
                    // for nobody.
                    if repeat && !held.contains_key(&keycode) {
                        continue;
                    }

                    // Update BDA Shift Flags (0x0417)
                    // This lets INT 16h AH=02 report modifier state correctly
                    let mut flags = cpu.bus.read_8(0x0417);
                    match keycode {
                        Keycode::RShift => flags |= 0x01,
                        Keycode::LShift => flags |= 0x02,
                        Keycode::LCtrl | Keycode::RCtrl => flags |= 0x04,
                        Keycode::LAlt | Keycode::RAlt => flags |= 0x08,
                        Keycode::CapsLock => flags ^= 0x40, // Toggle on press
                        _ => {}
                    }
                    cpu.bus.write_8(0x0417, flags);

                    // Recorder Toggle
                    if keycode == Keycode::PrintScreen {
                        recorder.toggle();
                        continue;
                    }

                    // Map Key to PC Scancode/ASCII. High byte = scancode,
                    // low byte = ASCII. Keep pushing to the INT 16h buffer
                    // for BIOS-based input, and ALSO latch the raw scan code
                    // at port 0x60 + raise IRQ1 so games that poll the port
                    // or install a custom INT 09h ISR see the event.
                    let extended = sdl_keys::is_extended(keycode);
                    if let Some(scan) = sdl_keys::modifier_scan(keycode) {
                        keyboard::deliver_scan_only(&mut cpu.bus, scan, extended);
                        held.insert(keycode, (scan, extended));
                    } else if let Some(code) = sdl_keys::map_sdl_to_pc(keycode, keymod) {
                        keyboard::deliver_key_down(&mut cpu.bus, code, extended);
                        held.insert(keycode, ((code >> 8) as u8, extended));
                    }
                }
                Event::KeyUp {
                    keycode: Some(keycode),
                    ..
                } => {
                    // Only keys the machine saw go down come up for it,
                    // not those of the settings window.
                    let Some((scan, extended)) = held.remove(&keycode) else {
                        continue;
                    };

                    // Update BDA Shift Flags (Clear bits)
                    let mut flags = cpu.bus.read_8(0x0417);
                    match keycode {
                        Keycode::RShift => flags &= !0x01,
                        Keycode::LShift => flags &= !0x02,
                        Keycode::LCtrl | Keycode::RCtrl => flags &= !0x04,
                        Keycode::LAlt | Keycode::RAlt => flags &= !0x08,
                        _ => {}
                    }
                    cpu.bus.write_8(0x0417, flags);

                    // Deliver release scan code (scancode | 0x80) to port 0x60
                    // and fire IRQ1. Games that track held keys (arrow-key
                    // movement, etc.) need these to know when the key stops
                    // being pressed.
                    keyboard::deliver_key_up(&mut cpu.bus, scan, extended);
                }

                Event::TextInput { text, .. } if ui.is_open() => ui.text(&text, &mut host!()),

                Event::MouseButtonDown { mouse_btn: MouseButton::Left, x, y, .. } if ui.is_open() => {
                    let (fx, fy) = display.to_frame(x, y);
                    ui.click(fx, fy, &mut host!());
                }

                Event::MouseWheel { y, direction, .. } if ui.is_open() => {
                    let dy = if matches!(direction, MouseWheelDirection::Flipped) { -y } else { y };
                    ui.wheel(dy, &mut host!());
                }

                // The machine's mouse is still while the window is open.
                Event::MouseMotion { .. } | Event::MouseButtonDown { .. } | Event::MouseButtonUp { .. }
                    if ui.is_open() => {}

                Event::MouseMotion { x, y, .. } => {
                    let (vx, vy) = video::overlay::frame_to_mouse(&cpu.bus, &cached_frame, display.to_frame(x, y));
                    cpu.bus.mouse.set_position(vx, vy);
                }

                Event::MouseButtonDown { mouse_btn, x, y, .. } => {
                    let (vx, vy) = video::overlay::frame_to_mouse(&cpu.bus, &cached_frame, display.to_frame(x, y));
                    cpu.bus.mouse.set_position(vx, vy);
                    if let Some(btn) = sdl_button_to_index(mouse_btn) {
                        cpu.bus.mouse.button_down(btn);
                    }
                }

                Event::MouseButtonUp { mouse_btn, x, y, .. } => {
                    let (vx, vy) = video::overlay::frame_to_mouse(&cpu.bus, &cached_frame, display.to_frame(x, y));
                    cpu.bus.mouse.set_position(vx, vy);
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
        for input in dbg.take_ui_input() {
            match input {
                debug::UiInput::Key(key) => ui.key(key, &mut host!()),
                debug::UiInput::Click(x, y) => ui.click(x, y, &mut host!()),
            }
        }

        // The window opening or closing: it takes the keyboard and mouse
        // from the machine, which pauses but for the Mixer page (see the
        // batch below).
        if ui.is_open() != ui_shown {
            ui_shown = ui.is_open();
            if ui_shown {
                release_input(&mut cpu, &mut held);
                dbg.release_keys(&mut cpu);
                text_input.start();
            } else {
                text_input.stop();
                // Caps Lock may have changed while the window had the keys.
                let caps = sdl_context.keyboard().mod_state().contains(Mod::CAPSMOD);
                let flags = cpu.bus.read_8(0x0417) & !0x40;
                cpu.bus.write_8(0x0417, flags | if caps { 0x40 } else { 0 });
            }
        }

        // Run the emulated machine up to the wall clock. Emulated time is
        // counted in instructions (see timer.rs), so timer interrupts land on
        // the right instructions however the work is batched between frames.
        let batch_start = std::time::Instant::now();
        let batch_end = if dbg.paused || ui.pauses_machine() {
            cpu.bus.clock.icount
        } else {
            pacer.batch_end(&cpu.bus.clock, batch_start)
        };
        cpu.bus.start_batch(batch_end);
        let batch_icount = cpu.bus.clock.icount;
        let batch_stalled = cpu.bus.clock.stalled;
        let batch_idle = cpu.bus.clock.idle;

        // Per-instruction debug hook (breakpoints / stepping / tracing) is
        // only consulted when something actually needs it.
        let dbg_hot = dbg.begin_batch(&cpu);
        exec::run_batch(&mut cpu, &mut dbg, dbg_hot);
        dbg.end_batch(&cpu);

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

        // DOSCONFIG asks for the settings window.
        if std::mem::take(&mut cpu.bus.config_ui_requested) && !ui.is_open() {
            toggle_ui!();
        }
        // Processor and sound changes wait for the running program to end.
        if !ui.is_open() && cpu.shell_idle() && machine.differs(&settings) {
            for warning in apply_machine(&mut cpu, &mut machine, &settings) {
                config_warning(&mut cpu, &warning);
            }
        }

        // Update Audio
        pump_audio(&mut cpu.bus);
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
        cpu.bus.sync_display();
        let (width, height) = video::frame_size(&cpu.bus);
        if cached_frame.resize(width, height) {
            cpu.bus.vga.mark_dirty_full();
            display.set_frame_size(width, height)?;
        }
        if cpu.bus.vga.dirty {
            video::render_screen(&mut cached_frame, &cpu.bus);
            cpu.bus.vga.clear_dirty();
        }

        // Start from the cached render; the overlays go on top. A
        // monochrome monitor shows them in its phosphor's colour, but not
        // the settings window.
        screen.clone_from(&cached_frame);
        video::overlay::draw_cursors(&mut screen, &cpu.bus, cursor_visible);
        video::mono::apply(&mut screen, settings.monochrome);
        let frame_w = width as usize;

        // Recordings show the machine alone. Debug clients see the
        // settings window as well, but not the recording indicator.
        recorder.capture(&screen);
        if ui.is_open() {
            ui.set_mixer_status(cpu.bus.mixer.muted, cpu.bus.mixer.take_peaks());
        }
        ui.draw(&mut screen);
        dbg.capture_frame(&screen);

        // Draw Recording Indicator
        if recorder.is_active() {
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
        display.present(&screen)?;

        let overhead = frame_start.elapsed().saturating_sub(exec_time);
        if let Some(cycles) = pacer.end_frame(&cpu.bus.clock, executed, exec_time, overhead) {
            cpu.bus.set_cycles_per_ms(cycles);
        }
        pacer.wait_for_next_frame();
    }

    Ok(())
}

/// Put the settings' processor and sound hardware in place. Only while no
/// program runs: one would lose track of the hardware it set up.
fn apply_machine(cpu: &mut Cpu, machine: &mut Machine, settings: &Settings) -> Vec<String> {
    cpu.model = settings.cpu;
    let warnings = if settings.sound != machine.sound {
        cpu.bus.log_string("[CONFIG] The sound settings changed");
        sound::apply_config(cpu, &settings.sound, Some(&machine.sound))
    } else {
        Vec::new()
    };
    if settings.video_setup() != machine.video {
        change_adapter(cpu, settings.video_setup());
    }
    machine.cpu = settings.cpu;
    machine.sound = settings.sound.clone();
    machine.video = settings.video_setup();
    warnings
}

/// Put another display adapter in, at the prompt: its BIOS data, and the
/// text mode it starts in, keeping what the screen shows.
fn change_adapter(cpu: &mut Cpu, setup: VideoSetup) {
    let monitor = if setup.mono() { "monochrome" } else { "colour" };
    cpu.bus.log_string(&format!("[CONFIG] The display is now {} with a {} monitor", setup.adapter.describe(), monitor));
    video::bios::install(&mut cpu.bus, setup);
    rust_dos::interrupts::int10::set_mode(cpu, 0x80 | setup.prompt_mode());
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
/// mount with MOUNT or IMGMOUNT.
fn startup_mounts(cpu: &Cpu, autoexec: &[String]) -> Vec<MountSpec> {
    let cwd = std::env::current_dir().unwrap_or_default();
    let home = dirs::home_dir();
    let mut lines = autoexec.to_vec();
    if let Some(file) = cpu.bus.disk.file_data("C:\\AUTOEXEC.BAT")
        && let Ok(bytes) = file.read()
    {
        lines.extend(String::from_utf8_lossy(&bytes).lines().map(str::to_string));
    }
    let disk = &cpu.bus.disk;
    let locate = |path: &str| disk.resolve_path(path).filter(|p| p.is_file());
    lines
        .iter()
        .filter_map(|line| {
            let (command, args) = line.trim().trim_start_matches('@').split_once(char::is_whitespace)?;
            let parsed = if command.eq_ignore_ascii_case("MOUNT") {
                mount::parse_mount_command(args, &cwd, home.as_deref())
            } else if command.eq_ignore_ascii_case("IMGMOUNT") {
                mount::parse_imgmount_command(args, &locate, &cwd, home.as_deref())
            } else {
                return None;
            };
            match parsed {
                Ok(MountCmd::Mount(spec)) => Some(spec),
                _ => None,
            }
        })
        .collect()
}

/// Save the settings and the drives that changed since the file was read
/// or last saved.
fn save_config(cpu: &mut Cpu, saved: &mut Saved, settings: &Settings) -> Result<(), String> {
    let Some(path) = saved.file.clone() else {
        return Err("No configuration file".to_string());
    };
    let current = mounted_drives(cpu);
    let startup = startup_mounts(cpu, &saved.autoexec);
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
    config::save(&path, &saved.settings, settings, &changes, dirs::home_dir().as_deref())?;
    cpu.bus.log_string(&format!("[CONFIG] Saved the settings to {}", path.display()));
    saved.settings = settings.clone();
    saved.drives = current;
    Ok(())
}

/// The settings window's way to the machine, the display and the speed.
struct MainHost<'m, 'd> {
    cpu: &'m mut Cpu,
    display: &'m mut Display<'d>,
    pacer: &'m mut timer::Pacer,
    /// The settings in effect (or waiting, see `Machine`).
    settings: &'m mut Settings,
    machine: &'m mut Machine,
    saved: &'m mut Saved,
}

impl Host for MainHost<'_, '_> {
    fn apply(&mut self, new: &Settings) -> Result<Option<String>, String> {
        let old = std::mem::replace(self.settings, new.clone());
        let shown = |s: &Settings| (s.scale, s.fullscreen, s.aspect, s.filter, s.shader, s.monochrome);
        if shown(new) != shown(&old) {
            self.display.apply(new)?;
        }
        if new.cycles != old.cycles {
            self.pacer.set_speed(new.cycles);
            // At max, the pacer tunes the speed from the current one.
            if let CpuSpeed::Fixed(n) = new.cycles {
                self.cpu.bus.set_cycles_per_ms(n);
            }
        }
        if new.disk != old.disk {
            self.cpu.bus.set_disk_settings(new.disk);
        }
        if new.mixer != old.mixer {
            self.cpu.bus.set_mixer(new.mixer);
        }
        if !self.machine.differs(new) {
            return Ok(None);
        }
        if !self.cpu.shell_idle() {
            return Ok(Some(config_ui::pending_note(self.machine.video, new).to_string()));
        }
        match apply_machine(self.cpu, self.machine, new).into_iter().next() {
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

    fn save(&mut self, settings: &Settings) -> Result<(), String> {
        save_config(self.cpu, self.saved, settings)
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
        Keycode::F2 => UiKey::Save,
        Keycode::S if ctrl => UiKey::Save,
        _ => return None,
    })
}

/// Let go of everything the machine holds as the settings window takes the
/// keyboard and mouse: release the keys and buttons, so no game is left
/// with Ctrl or a fire button held down.
fn release_input(cpu: &mut Cpu, held: &mut HashMap<Keycode, (u8, bool)>) {
    for (_, (scan, extended)) in held.drain() {
        keyboard::deliver_key_up(&mut cpu.bus, scan, extended);
    }
    // Shift, Ctrl and Alt; the lock states stay.
    let flags = cpu.bus.read_8(0x0417);
    cpu.bus.write_8(0x0417, flags & !0x0F);
    for button in 0..3 {
        if cpu.bus.mouse.buttons & (1 << button) != 0 {
            cpu.bus.mouse.button_up(button);
        }
    }
}

/// A configuration problem: shown on the terminal and written to the log.
fn config_warning(cpu: &mut Cpu, msg: &str) {
    eprintln!("[CONFIG] Warning: {}", msg);
    cpu.bus.log_string(&format!("[CONFIG] Warning: {}", msg));
}

/// The box above the first DOS prompt: the emulator and its version, the
/// configuration file in use and the way to the settings window.
fn print_banner(cpu: &mut Cpu, config: &config::Config, no_config: bool) {
    // Bright cyan, white and yellow on blue.
    const FRAME: u8 = 0x1B;
    const TEXT: u8 = 0x1F;
    const HIGHLIGHT: u8 = 0x1E;
    // The widest text inside the frame: a line of 80 would wrap.
    const MAX_WIDTH: usize = 74;

    let label = "Config file: ";
    let file = match &config.source {
        Some(path) => {
            let path = mount::display_host_path(&std::path::absolute(path).unwrap_or_else(|_| path.clone()));
            if config.created { format!("{} (new)", path) } else { path }
        }
        None if no_config => "none (--no-config)".to_string(),
        None => "none".to_string(),
    };
    // A path too long for the box keeps its end.
    let room = MAX_WIDTH - label.len();
    let file = match file.chars().count() {
        n if n > room => format!("...{}", file.chars().skip(n - room + 3).collect::<String>()),
        _ => file,
    };
    let lines: [&[(&str, u8)]; 4] = [
        &[
            (&format!("Rust-DOS v{}", env!("CARGO_PKG_VERSION")), HIGHLIGHT),
            (&format!(" - {}", env!("CARGO_PKG_DESCRIPTION")), TEXT),
        ],
        &[],
        &[(label, TEXT), (&file, HIGHLIGHT)],
        &[
            ("Press ", TEXT),
            ("Ctrl+F12", HIGHLIGHT),
            (" or type ", TEXT),
            ("DOSCONFIG", HIGHLIGHT),
            (" to open the settings.", TEXT),
        ],
    ];
    let len = |line: &[(&str, u8)]| line.iter().map(|(text, _)| text.chars().count()).sum::<usize>();
    let width = lines.iter().map(|line| len(line)).max().unwrap_or(0);

    fn put(cpu: &mut Cpu, text: &str, attr: u8) {
        let cells: Vec<u8> = text.chars().map(config_ui::cp437).collect();
        video::print_cp437(cpu, &cells, attr);
    }
    put(cpu, &format!("╔{}╗", "═".repeat(width + 2)), FRAME);
    video::print_string(cpu, "\r\n");
    for line in lines {
        put(cpu, "║ ", FRAME);
        for (text, attr) in line {
            put(cpu, text, *attr);
        }
        put(cpu, &" ".repeat(width - len(line)), TEXT);
        put(cpu, " ║", FRAME);
        video::print_string(cpu, "\r\n");
    }
    put(cpu, &format!("╚{}╝", "═".repeat(width + 2)), FRAME);
    video::print_string(cpu, "\r\n\r\n");
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
        config::default_path(),
        dirs::home_dir().as_deref(),
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
fn create_cpu(args: &Args, config: &config::Config) -> Cpu {
    use crate::disk::{DRIVE_C, drive_letter};

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

    let memory_mb = config.memsize.unwrap_or(rust_dos::bus::DEFAULT_MEMORY_MB);
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
            warn(format!("cannot mount {}: {}", drive_letter(spec.drive), e));
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

fn sdl_button_to_index(button: MouseButton) -> Option<usize> {
    match button {
        MouseButton::Left => Some(0),
        MouseButton::Right => Some(1),
        MouseButton::Middle => Some(2),
        _ => None,
    }
}
