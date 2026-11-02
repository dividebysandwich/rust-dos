//! The command line: the window's options, and the `run` subcommand.

use clap::Parser;

use crate::cpu::CoreMode;
use crate::timer;

#[derive(Parser, Debug)]
#[command(author, version = rust_dos::build_info::VERSION, about, long_about = None)]
pub(crate) struct Args {
    /// Window scale factor [default: 1, or the config file's scale]
    #[arg(short, long, value_parser = clap::value_parser!(u32).range(1..=16))]
    pub(crate) scale: Option<u32>,

    /// Show each frame when the machine's display draws it, for a display
    /// with a variable refresh rate (G-Sync, FreeSync) [default: the
    /// config file's vrr]
    #[arg(long)]
    pub(crate) vrr: bool,

    /// Keep the window hidden, for runs driven through the debug server:
    /// the 3dfx card is still drawn with OpenGL (voodoo_renderer=opengl),
    /// on the host's GPU, where SDL_VIDEODRIVER=dummy has no OpenGL. A
    /// host without OpenGL 3 gets SDL's renderer and the software card
    #[arg(long)]
    pub(crate) hidden: bool,

    /// What plays the sound: auto, sdl (SDL's sound device) or null (the
    /// null output: nothing plays, and its queue empties at exactly the
    /// output's rate) [default: the config file's audio_output]
    #[arg(long, value_name = "OUTPUT", value_parser = parse_audio_output)]
    pub(crate) audio_output: Option<rust_dos::audio::OutputChoice>,

    /// Start with the sound off, as Ctrl+F8 does: the sound device gets
    /// silence, and the debug server's stream and recordings keep the sound
    #[arg(long)]
    pub(crate) mute: bool,

    /// Show the picture on a screen in a 3D scene in a VR headset, through
    /// OpenXR (SteamVR, Monado) [default: the config file's [vr] mode]
    #[arg(long, conflicts_with = "vr_desktop")]
    pub(crate) vr: bool,

    /// Show the picture on a screen in a 3D scene in the window, with a
    /// camera moved with the mouse while Ctrl+Shift is held
    #[arg(long)]
    pub(crate) vr_desktop: bool,

    /// The 3D scene: a glTF file (.glb or .gltf) exported from Blender, with
    /// the mesh the picture goes on named "screen" [default: the config
    /// file's, or the built-in room]
    #[arg(long, value_name = "FILE")]
    pub(crate) vr_scene: Option<std::path::PathBuf>,

    /// How the headset's pictures get to the OpenXR runtime: auto, gl
    /// (GLX/WGL), egl or vulkan [default: the config file's [vr] graphics]
    #[arg(long, value_name = "API", value_parser = parse_vr_graphics)]
    pub(crate) vr_graphics: Option<rust_dos::vr::VrGraphics>,

    /// The headset's eye images, in percent of the size its runtime
    /// recommends (30 to 150) [default: the config file's [vr] resolution]
    #[arg(long, value_name = "PERCENT", value_parser = clap::value_parser!(u32).range(30..=150))]
    pub(crate) vr_resolution: Option<u32>,

    /// Write what the OpenXR runtime and OpenGL offer the headset to
    /// vr-probe.log in Rust-DOS's folder and the console, then quit
    #[arg(long)]
    pub(crate) vr_probe: bool,

    /// Root directory for Drive C: [default: the config file's C:, or "."]
    #[arg(short, long)]
    pub(crate) dir: Option<String>,

    /// Configuration file to use instead of ./rust-dos.conf, the one next
    /// to the executable or the per-user default
    #[arg(short, long, value_name = "FILE", conflicts_with = "no_config")]
    pub(crate) config: Option<std::path::PathBuf>,

    /// Don't read or create any configuration file
    #[arg(long)]
    pub(crate) no_config: bool,

    /// Write the log to FILE, replacing it, instead of rust-dos.log in the
    /// configuration directory, so instances running at the same time keep
    /// separate logs
    #[arg(long, value_name = "FILE")]
    pub(crate) log: Option<std::path::PathBuf>,

    /// Start at the DOS prompt, without booting the disk image that boots
    /// at startup (`-boot` in [drives])
    #[arg(long)]
    pub(crate) no_boot: bool,

    /// Start the HTTP/WebSocket debug server (local-only, unauthenticated).
    /// Optionally takes the listen address. With port 0 the system picks a
    /// free port, and the "Debug server listening on" line gives it.
    #[arg(long, value_name = "ADDR", num_args = 0..=1, default_missing_value = "127.0.0.1:8086")]
    pub(crate) debug_server: Option<std::net::SocketAddr>,

    /// Instruction trace ring buffer capacity (entries, ~64 bytes each).
    /// Allocated only once tracing is enabled via the debug server.
    #[arg(long, default_value_t = 1_000_000)]
    pub(crate) trace_capacity: usize,

    /// Profile the emulator from startup, and write where its time went to
    /// FILE as JSON when it exits, with a table (FILE.txt) and folded
    /// stacks for flame graphs (FILE.folded) beside it (docs/profiling.md)
    #[arg(long, value_name = "FILE")]
    pub(crate) profile: Option<std::path::PathBuf>,

    /// Samples a second for --profile (10 to 20000)
    #[arg(long, value_name = "HZ", default_value_t = rust_dos::profile::session::DEFAULT_HZ,
          value_parser = clap::value_parser!(u32).range(rust_dos::profile::session::MIN_HZ as i64..=rust_dos::profile::session::MAX_HZ as i64))]
    pub(crate) profile_hz: u32,

    /// Emulated CPU speed in instructions per millisecond, "max" for as
    /// fast as the host keeps up with, or "auto" for the speed the running
    /// program's frames show it needs (optionally with the least speed,
    /// "auto 5000") [default: auto, or the config file's cycles]
    #[arg(long, value_name = "N|max|auto", value_parser = timer::CpuSpeed::parse)]
    pub(crate) cycles: Option<timer::CpuSpeed>,

    /// Run on emulated time alone: the clock starts at --start-time, the
    /// speed is fixed (--cycles N, or 3000 when it would be auto or max),
    /// and the debug server's input arrives at emulated times. With
    /// --debug-server the machine starts paused
    #[arg(long)]
    pub(crate) deterministic: bool,

    /// The date and time the clock starts at with --deterministic
    /// [default: 1995-04-11 12:34:56]
    #[arg(long, value_name = "YYYY-MM-DD HH:MM:SS", requires = "deterministic", value_parser = rust_dos::deterministic::parse_start)]
    pub(crate) start_time: Option<chrono::NaiveDateTime>,

    /// What runs the programs' instructions: auto (the interpreter, and the
    /// dynamic recompiler for protected-mode programs), dynamic or normal
    /// [default: auto, or the config file's core]
    #[arg(long, value_name = "auto|dynamic|normal", value_parser = CoreMode::parse)]
    pub(crate) core: Option<CoreMode>,

    /// Launch a game at startup: a profile in the games folder beside the
    /// configuration file, by its file name or its name
    #[arg(long, value_name = "NAME", conflicts_with = "import")]
    pub(crate) game: Option<String>,

    /// Import a game as a game profile, and launch it: its package (a zip
    /// or 7z archive, or a folder with a rust-dos.conf), or a game set up
    /// for DOSBox (a GOG install's folder, a folder with DOSBox
    /// configuration files, or one of them)
    #[arg(long, value_name = "PATH")]
    pub(crate) import: Option<std::path::PathBuf>,

    /// Relay LAN rooms for other rust-dos instances on this UDP port,
    /// without starting the emulator
    #[arg(long, value_name = "PORT", num_args = 0..=1, default_missing_value = "21213")]
    pub(crate) relay: Option<u16>,

    /// One password for all the rooms of --relay
    #[arg(long, value_name = "PASSWORD", requires = "relay")]
    pub(crate) relay_password: Option<String>,

    #[command(subcommand)]
    pub(crate) command: Option<Command>,
}

#[derive(clap::Subcommand, Debug)]
pub(crate) enum Command {
    /// Run a DOS program, batch file or command without a window, as a
    /// tool of the host: what it prints goes to standard output, standard
    /// input is typed at its keyboard, and its exit code is rust-dos's
    Run(RunArgs),
    /// List or extract an InstallShield installer archive: a cabinet set
    /// by its header (data1.hdr, or a data1.cab holding one) or an
    /// InstallShield 3 archive (_SETUP.1, a .Z file)
    Unpack(UnpackArgs),
}

#[derive(clap::Args, Debug)]
pub(crate) struct UnpackArgs {
    /// The cabinet set's header, or the InstallShield 3 archive
    pub(crate) archive: std::path::PathBuf,

    /// List the files, the file groups and the entries that can't be read,
    /// instead of extracting
    #[arg(short, long)]
    pub(crate) list: bool,

    /// The folder to extract into [default: the current one]
    #[arg(short, long, value_name = "DIR")]
    pub(crate) to: Option<std::path::PathBuf>,

    /// Extract only the files of this file group (several may be given)
    #[arg(short, long, value_name = "NAME")]
    pub(crate) group: Vec<String>,

    /// The cabinet's compressed files are raw deflate ending each chunk
    /// with 00 00 FF FF, as in some early sets (Unshield's -O)
    #[arg(short = 'O', long)]
    pub(crate) old_compression: bool,
}

#[derive(clap::Args, Debug)]
pub(crate) struct RunArgs {
    /// The folder that is drive C: [default: the current one]
    #[arg(short, long)]
    pub(crate) dir: Option<std::path::PathBuf>,

    /// Instructions per emulated millisecond
    #[arg(long, default_value_t = 100_000)]
    pub(crate) cycles: u32,

    /// The machine's memory in MB
    #[arg(long, default_value_t = 16, value_parser = clap::value_parser!(u32).range(1..=256))]
    pub(crate) mem: u32,

    /// Print the text screen at the end when nothing was printed through
    /// DOS, for programs that write to the screen's memory
    #[arg(long)]
    pub(crate) screen: bool,

    /// Run Win32 console programs with the built-in Windows (the config
    /// file's win32)
    #[arg(long)]
    pub(crate) win32: bool,

    /// The command line, as typed at the DOS prompt
    #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    pub(crate) command: Vec<String>,
}

/// `--audio-output`'s value.
fn parse_audio_output(value: &str) -> Result<rust_dos::audio::OutputChoice, String> {
    rust_dos::audio::OutputChoice::parse(value).ok_or_else(|| "auto, sdl or null".to_string())
}

/// `--vr-graphics`'s value.
fn parse_vr_graphics(value: &str) -> Result<rust_dos::vr::VrGraphics, String> {
    rust_dos::vr::VrGraphics::parse(value).ok_or_else(|| "auto, gl, egl or vulkan".to_string())
}
