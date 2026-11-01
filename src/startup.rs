//! Starting up: the settings from the command line, the game to launch,
//! the machine with its drives and log, the debug server, and DOS.

use std::path::PathBuf;

use crate::args::Args;
use crate::config::{self, Settings};
use crate::config_file::{banner_notes, config_dir, config_warning, find_game, games_dir};
use crate::cpu::Cpu;
use crate::debug;
use crate::disk;
use crate::games;
use crate::timer::CpuSpeed;
use rust_dos::boot::{Startup, StartupItem};
use rust_dos::games::GameEntry;

/// Closes the console window Windows opens next to the program's own when
/// it is started from Explorer or a shortcut: a console program gets a
/// console of its own then, which only it is attached to. Started from a
/// terminal, it shares the terminal's and keeps it for its output. Windows
/// 11 24H2 and later don't open one at all (the manifest's
/// consoleAllocationPolicy, packaging/windows/rust-dos.manifest).
#[cfg(windows)]
pub(crate) fn leave_explorer_console() {
    use windows_sys::Win32::System::Console::{FreeConsole, GetConsoleProcessList};
    let mut pids = [0u32; 2];
    // SAFETY: the list has room for the two entries it is given.
    if unsafe { GetConsoleProcessList(pids.as_mut_ptr(), pids.len() as u32) } == 1 {
        // SAFETY: nothing holds on to the console's handles.
        unsafe { FreeConsole() };
    }
}

/// The speed in deterministic mode: the one asked for, which has to be a
/// number of cycles on the command line; from the configuration, auto and
/// max become `deterministic::DEFAULT_CYCLES`.
pub(crate) fn deterministic_speed(speed: CpuSpeed, from_args: Option<CpuSpeed>) -> Result<CpuSpeed, String> {
    match (speed, from_args) {
        (CpuSpeed::Fixed(_), _) => Ok(speed),
        (_, Some(asked)) => Err(format!("--deterministic needs a fixed speed: --cycles N, not {}", asked)),
        _ => Ok(CpuSpeed::Fixed(rust_dos::deterministic::DEFAULT_CYCLES)),
    }
}

/// The drive to boot from at startup: the disk image mounted with -boot,
/// unless `skip` (--no-boot, or a game launched).
pub(crate) fn startup_boot(cpu: &mut Cpu, skip: bool) -> Option<u8> {
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

/// Build the CPU with drive C: from `-d`, the config file or the working
/// directory (in that order), then mount the config's other drives. Opens
/// the log file.
///
/// The CPU is boxed: it is about 136 KB, and in a debug build every move of
/// it gets its own copy in the frame of `main` and here. Unboxed, `main`'s
/// frame was about 630 KB and startup overflowed Windows's 1 MB main thread stack.
pub(crate) fn create_cpu(args: &Args, config: &config::Config, memory_mb: usize) -> Result<Box<Cpu>, String> {
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

    let mut cpu = Box::new(Cpu::with_memory(root_path.clone(), memory_mb));
    cpu.bus.log_file = open_log_file(args.log.as_deref())?;
    // The debug server's log takes the lines logged before it starts.
    if args.debug_server.is_some() {
        cpu.bus.log_backlog = Some(Vec::new());
    }
    cpu.shell_history.set_home(rust_dos::cmdline::default_history_file());
    rust_dos::cmdline::configure(&mut cpu, &config.shell);
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
    Ok(cpu)
}

/// Create the log file at `path` (`--log`), or in rust-dos's own directory
/// (`config::user_dir`), replacing the previous run's. Without `--log` the
/// emulator runs without a log file if it cannot create one. A `--log` file
/// that cannot be created stops it, as a missing `--config` file does: a
/// harness that asked for the log at that path would otherwise find none
/// there, with only a warning on stderr to say why.
pub(crate) fn open_log_file(path: Option<&std::path::Path>) -> Result<Option<rust_dos::log::LogFile>, String> {
    if let Some(path) = path {
        return rust_dos::log::LogFile::create(path)
            .map(Some)
            .map_err(|e| format!("cannot create the log file {} (--log): {}", path.display(), e));
    }
    let Some(path) = rust_dos::log::default_path() else {
        return Ok(None);
    };
    match rust_dos::log::LogFile::create(&path) {
        Ok(log) => Ok(Some(log)),
        Err(e) => {
            eprintln!("Warning: cannot create the log file {}: {}", path.display(), e);
            Ok(None)
        }
    }
}

/// A game to launch: its entry, its text and its folder.
type Launch = (GameEntry, String, PathBuf);

/// The game to launch at startup (`--game`, or the one `--import` made a
/// profile of): its entry, its text and its folder, and the memory size of
/// the machine, which is the game's.
pub(crate) fn game_to_launch(
    args: &Args,
    config: &config::Config,
    settings: &Settings,
) -> Result<(Option<Launch>, usize), String> {
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
            games::prepare(&entry.id, settings, text, dir, rust_dos::hostdirs::home_dir().as_deref())?.settings.memsize
        }
        None => settings.memsize,
    };
    Ok((startup_game, memory_mb))
}

/// The command line's options over the configuration's settings.
pub(crate) fn apply_args(settings: &mut Settings, args: &Args) -> Result<(), String> {
    if let Some(scale) = args.scale {
        settings.scale = scale;
    }
    if args.vrr {
        settings.vrr = true;
    }
    if let Some(output) = args.audio_output {
        settings.audio_output = output;
    }
    if args.vr {
        settings.vr.mode = rust_dos::vr::VrMode::Headset;
    } else if args.vr_desktop {
        settings.vr.mode = rust_dos::vr::VrMode::Desktop;
    }
    if let Some(graphics) = args.vr_graphics {
        settings.vr.graphics = graphics;
    }
    if let Some(resolution) = args.vr_resolution {
        settings.vr.resolution = Some(resolution);
    }
    if let Some(scene) = &args.vr_scene {
        settings.vr.scene = Some(scene.clone());
        if settings.vr.mode == rust_dos::vr::VrMode::Off {
            settings.vr.mode = rust_dos::vr::VrMode::Desktop;
        }
    }
    if let Some(cycles) = args.cycles {
        settings.cycles = cycles;
    }
    if args.deterministic {
        settings.cycles = deterministic_speed(settings.cycles, args.cycles)?;
    }
    if let Some(core) = args.core {
        settings.core = core;
    }
    Ok(())
}

/// Start DOS with its startup commands, and the notes and the newer
/// version for the box above the first prompt.
pub(crate) fn start_dos(
    cpu: &mut Cpu,
    config: &config::Config,
    args: &Args,
    startup_game: &Option<(GameEntry, String, PathBuf)>,
    update_check: &mut Option<rust_dos::version_check::Check>,
) {
    // Startup commands: the config's [autoexec] lines, then AUTOEXEC.BAT
    // from the C: root if there is one, like the startup sequence a real PC
    // would run. Each line runs as if typed at the prompt. A reboot runs
    // them again.
    cpu.bus.config_dir = config_dir(config.source.as_deref());
    let mut commands = vec![StartupItem::Lines(config.autoexec.clone())];
    // A disk image that boots at startup boots after the [autoexec] lines,
    // which may mount more for it, in place of AUTOEXEC.BAT: the system on
    // the disk has its own.
    commands.push(match startup_boot(cpu, args.no_boot || startup_game.is_some()) {
        Some(drive) => StartupItem::Lines(vec![format!("BOOT -l {}", disk::drive_key(drive))]),
        None => StartupItem::BatchFile("C:\\AUTOEXEC.BAT".to_string()),
    });
    // A newer version under the box: the answer if it comes in a moment,
    // else what the last check found.
    let update = update_check.as_mut().and_then(|check| {
        check.wait(std::time::Duration::from_secs(1));
        check.banner()
    });
    let update = update.as_deref().map(rust_dos::version_check::console_line);
    cpu.startup = Startup { notes: banner_notes(config, args.no_config), update, commands };
    cpu.start_dos();
}

/// The debug server (`--debug-server`), and the profiler started with the
/// machine (`--profile`); in deterministic mode, paused before anything runs.
pub(crate) fn start_debug_hub(
    cpu: &mut Cpu,
    args: &Args,
    det: &Option<rust_dos::deterministic::Deterministic>,
    settings: &Settings,
) -> Result<debug::DebugHub, String> {
    let mut dbg = match args.debug_server {
        Some(addr) => debug::DebugHub::start(cpu, addr, args.trace_capacity, det.as_ref().map(|mode| mode.start))?,
        None => debug::DebugHub::disabled(),
    };
    if args.profile.is_some() {
        dbg.profile_at_start(cpu, args.profile_hz).map_err(|e| format!("The profiler's thread can't start: {}", e))?;
    }
    if let Some(mode) = det {
        dbg.deterministic = Some(mode.start);
        // A client sends its input before anything runs.
        if args.debug_server.is_some() {
            dbg.pause_at_start(cpu);
        }
        cpu.bus.log_string(&format!(
            "[DETERMINISTIC] Clock from {}, {} cycles",
            mode.start.format("%Y-%m-%d %H:%M:%S"),
            settings.cycles.initial_cycles()
        ));
    }
    Ok(dbg)
}
