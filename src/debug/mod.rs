//! Remote debug / control interface.
//!
//! An HTTP + WebSocket server (see `server.rs`) runs on its own thread with
//! a private tokio runtime. It never touches the emulator directly: requests
//! are sent to the main thread through an mpsc channel and executed by
//! `DebugHub::poll` once per frame, and data flows back through oneshot
//! replies, broadcast channels, and a shared frame snapshot.

pub mod heat;
pub mod journal;
pub mod keys;
pub mod pm;
pub mod profile;
pub mod server;
pub mod trace;
pub mod win32;

mod dispatch;
mod events;
mod input;
mod run;
mod state;
mod status;

use std::collections::{HashSet, VecDeque};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::sync::{broadcast, oneshot, watch};

use journal::{Journal, Kind, Stamp};

use crate::config_ui::UiKey;
use crate::cpu::Cpu;
use crate::video::{self, VideoMode};
use keys::PcKey;
pub use pm::{parse_addr, parse_hex};
pub(crate) use input::keys_for_char;
pub(crate) use status::stop_fields;
pub use status::{folders_json, regs_json};
use trace::{TraceEntry, TraceRing};

const LOG_RING_CAPACITY: usize = 5000;

/// State shared between the emulator thread and the server thread.
pub struct Shared {
    /// Latest composited frame (the screen with its cursors). Only kept up
    /// to date while a screen stream is subscribed.
    pub frame: Mutex<video::Frame>,
    /// Bumped whenever `frame` changes.
    pub frame_seq: AtomicU64,
    pub screen_subscribers: AtomicUsize,
    /// The memory heatmap's latest message (`heat.rs`), its number, and
    /// the streams that want it: the writes are watched only while one
    /// does.
    pub heat: Mutex<Arc<[u8]>>,
    pub heat_seq: AtomicU64,
    pub heat_subscribers: AtomicUsize,
    /// The written address ranges of the heatmap's last message.
    pub heat_ranges: Mutex<Vec<(usize, usize)>>,
    /// Events kept nowhere: log lines and video mode changes. What the
    /// machine and the debugger did is in `journal`.
    pub events: broadcast::Sender<Arc<str>>,
    /// JSON trace batches (one per frame while subscribed).
    pub trace: broadcast::Sender<Arc<str>>,
    /// Mixed audio, 44.1 kHz stereo s16, interleaved.
    pub audio: broadcast::Sender<Arc<[i16]>>,
    pub log: Mutex<VecDeque<LogLine>>,
    /// The number of the last log line (they count from 1).
    pub log_n: watch::Sender<u64>,
    pub start_time: Instant,
    /// The journal, which only the emulator thread appends to, and the
    /// sequence number of its last entry, which readers wait on.
    pub journal: Mutex<Journal>,
    pub journal_seq: watch::Sender<u64>,
    /// This process, so that a client can tell another took the port.
    pub instance: u64,
    /// The next request number (`req`).
    pub next_req: AtomicU64,
    /// The request the emulator thread is carrying out, which log lines
    /// name as their cause.
    pub current: Mutex<Option<(u64, Option<Arc<str>>)>>,
    /// Set once the emulator shut down: requests get 503.
    pub closed: AtomicBool,
    /// HTTP requests and event streams in flight, which the shutdown
    /// gives a moment to finish.
    pub busy: AtomicUsize,
}

#[derive(Clone, serde::Serialize)]
pub struct LogLine {
    /// The line's number, counting from 1.
    pub n: u64,
    pub t_ms: u64,
    /// The journal's last entry when the line was written.
    pub at: u64,
    /// The request being carried out, if the line came from one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    pub line: String,
}

impl LogLine {
    /// The line as the event streams send it: `{"type":"log",...}`.
    pub fn event_json(&self) -> String {
        let mut v = serde_json::to_value(self).unwrap_or(Value::Null);
        v["type"] = "log".into();
        v.to_string()
    }
}

impl Shared {
    pub fn new(start_time: Instant) -> Self {
        Self::with_capacity(start_time, journal::CAPACITY)
    }

    pub fn with_capacity(start_time: Instant, capacity: usize) -> Self {
        Self {
            frame: Mutex::new(video::Frame::new(video::SCREEN_WIDTH, video::SCREEN_HEIGHT)),
            frame_seq: AtomicU64::new(0),
            screen_subscribers: AtomicUsize::new(0),
            heat: Mutex::new(Arc::from([] as [u8; 0])),
            heat_seq: AtomicU64::new(0),
            heat_subscribers: AtomicUsize::new(0),
            heat_ranges: Mutex::new(Vec::new()),
            events: broadcast::channel(1024).0,
            trace: broadcast::channel(64).0,
            audio: broadcast::channel(256).0,
            log: Mutex::new(VecDeque::with_capacity(LOG_RING_CAPACITY)),
            log_n: watch::channel(0).0,
            start_time,
            journal: Mutex::new(Journal::new(capacity)),
            journal_seq: watch::channel(0).0,
            instance: getrandom::u64().unwrap_or_else(|_| {
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64)
            }),
            next_req: AtomicU64::new(1),
            current: Mutex::new(None),
            closed: AtomicBool::new(false),
            busy: AtomicUsize::new(0),
        }
    }

    fn emit(&self, v: Value) {
        if self.events.receiver_count() > 0 {
            let _ = self.events.send(Arc::from(v.to_string()));
        }
    }

    /// The instance as clients see it: 16 hex digits (a JSON number would
    /// lose bits in JavaScript).
    pub fn instance_hex(&self) -> String {
        format!("{:016x}", self.instance)
    }

    /// Add a line to the log, numbered, with the journal position and the
    /// request being carried out.
    pub fn log_line(&self, line: &str) {
        let t_ms = self.start_time.elapsed().as_millis() as u64;
        let at = *self.journal_seq.borrow();
        let (cause, request_id) = self.current.lock().ok().and_then(|c| c.clone()).map_or((None, None), |(r, t)| (Some(r), t));
        let entry = {
            let Ok(mut log) = self.log.lock() else { return };
            let n = log.back().map_or(0, |l| l.n) + 1;
            if log.len() >= LOG_RING_CAPACITY {
                log.pop_front();
            }
            let entry = LogLine { n, t_ms, at, cause, request_id: request_id.map(|t| t.to_string()), line: line.to_string() };
            log.push_back(entry.clone());
            entry
        };
        self.log_n.send_replace(entry.n);
        if self.events.receiver_count() > 0 {
            let _ = self.events.send(Arc::from(entry.event_json()));
        }
    }

    /// Wait up to `limit` for the requests and streams in flight to finish,
    /// so that they get to send what the shutdown told them.
    pub fn drain(&self, limit: Duration) {
        let until = Instant::now() + limit;
        while self.busy.load(Ordering::Acquire) > 0 && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
        // The last responses' bytes are on their way.
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// What the hub tells about the journal with every answer.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Meta {
    pub state_seq: u64,
    pub stop_seq: Option<u64>,
    /// The journal's last entry.
    pub last: u64,
    /// The last entry this request caused, if it caused any.
    pub seq: Option<u64>,
    /// The operation this request started, if it started one.
    pub op: Option<u64>,
}

/// A reply with the journal's state at the time.
pub struct Answer {
    pub reply: Reply,
    pub meta: Meta,
}

pub enum Reply {
    Json(Value),
    Frame(video::Frame),
    /// Trace entries, the cursor for the next page (`TraceQuery::since`)
    /// and how many entries after the requested cursor were overwritten.
    /// `truncated`: `limit` left out entries the query asked for (with
    /// `since`, the page stopped before the newest entry).
    Trace { entries: Vec<TraceEntry>, next: u64, dropped: u64, truncated: bool },
    Bytes { addr: usize, segoff: Option<(u16, u32)>, data: Vec<u8> },
    Error(u16, String),
    /// An error whose body has fields beside its message (`error`).
    ErrorJson(u16, Value),
}

impl Reply {
    fn bad(msg: impl Into<String>) -> Self {
        Reply::Error(400, msg.into())
    }
}

pub struct Request {
    pub cmd: Cmd,
    pub reply: oneshot::Sender<Answer>,
    /// The request's number (`req`), unique in this process.
    pub req: u64,
    /// The client's `X-Request-Id`, copied into the entries it causes.
    pub tag: Option<Arc<str>>,
    /// `If-State-Seq`: carry the request out only if the machine is still
    /// stopped in the state of that entry.
    pub if_state: Option<u64>,
}

/// The request being carried out, and what it did to the journal.
#[derive(Clone, Debug, Default)]
struct Ctx {
    req: u64,
    tag: Option<Arc<str>>,
    /// The last entry it caused.
    seq: Option<u64>,
    /// The operation it started.
    op: Option<u64>,
}

/// What a request needs of the machine to be carried out (`DebugHub::check`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Need {
    /// Paused (or held by the settings window): deterministic mode takes
    /// changes to the machine only then.
    Paused,
    /// Paused with no step or run pending, and with `input_ok` false, no
    /// input still to be typed: a strict-match save (false) or load (true).
    Quiescent { input_ok: bool },
    /// Still stopped in the state of entry N (`If-State-Seq`).
    State(u64),
}

#[derive(Deserialize, Default, Debug)]
pub struct TraceQuery {
    pub from_ms: Option<f64>,
    pub to_ms: Option<f64>,
    pub last_ms: Option<f64>,
    pub last_n: Option<usize>,
    pub limit: Option<usize>,
    /// Only entries whose CS equals this (hex).
    pub cs: Option<String>,
    /// Skip entries executing in the BIOS segment (F000), except HLE traps.
    #[serde(default)]
    pub no_bios: bool,
    /// Output format for the HTTP layer (`text` or `json`).
    pub format: Option<String>,
    /// Only entries recorded after this cursor (an earlier reply's `next`),
    /// oldest first, up to `limit`.
    pub since: Option<u64>,
}

pub enum Cmd {
    Status,
    Stats,
    Screenshot,
    ScreenText,
    TraceQuery(TraceQuery),
    /// `count`: record that many more instructions, then stop.
    TraceControl { enabled: Option<bool>, clear: bool, stream_max: Option<usize>, count: Option<u64> },
    /// Turn the port log on or off, empty it or size it (`/api/ports`).
    PortsControl { enabled: Option<bool>, clear: bool, capacity: Option<usize>, ports: Option<(u16, u16)> },
    /// The port log's accesses to ports `from..=to`, reads or writes or
    /// both, the last `last_n`.
    PortsQuery { from: u16, to: u16, write: Option<bool>, last_n: usize },
    Input { events: Vec<InputEvent>, wait: bool },
    InputClear,
    Pause,
    /// `until_ms`: pause once emulated time reaches that many
    /// milliseconds since power-on (deterministic mode only).
    Resume { until: Option<String>, until_ms: Option<u64> },
    Step { count: u64 },
    /// Step, but run a call, interrupt, loop or repeated string instruction
    /// through to the instruction after it.
    StepOver,
    WaitPause,
    RebootShell,
    Reboot,
    GetRegs,
    /// The FPU, MMX and SSE registers.
    GetFpu,
    SetRegs(Map<String, Value>),
    ReadMem { addr: String, len: usize },
    /// Write `data` at an address; with `expect`, only if the bytes there
    /// match it.
    WriteMem { addr: String, data: Vec<u8>, expect: Option<Vec<u8>> },
    /// `bits`: decode as 16 or 32-bit code instead of the address's
    /// segment's size (or, for a linear or physical address, the running
    /// code's).
    Disasm { addr: Option<String>, count: usize, bits: Option<u32> },
    ListBreakpoints,
    /// A breakpoint; with `once` it goes once it stops the machine.
    AddBreakpoint { addr: String, once: bool },
    RemoveBreakpoint(Option<String>),
    ListWatchpoints,
    /// Pause when the 1, 2 or 4 bytes at an address change.
    AddWatchpoint { addr: String, len: u8 },
    RemoveWatchpoint(Option<String>),
    /// Pause after the CPU raises one of these exceptions (bit n = vector
    /// n), or switches between real and protected mode.
    BreakOn {
        exceptions: Option<u32>,
        clear_exceptions: Option<u32>,
        mode_switch: Option<bool>,
        program_start: Option<bool>,
        program_exit: Option<bool>,
    },
    /// Run a command line at the DOS prompt, as typed, and answer once it
    /// started a program: stopped at its entry point with `stop_at_entry`.
    /// With `queue`, a busy prompt takes it after what keeps it busy.
    Run { command: String, stop_at_entry: bool, queue: bool },
    Ivt,
    Gdt,
    Ldt,
    Idt,
    Tss,
    PageWalk(String),
    Xms,
    Net,
    Serial,
    /// The printer; `eject` ends the job and waits for its files.
    Printer { eject: bool },
    Exceptions,
    /// The built-in Windows's processes, modules, threads and handles.
    Win32,
    /// DirectDraw's display mode and surfaces.
    Win32Ddraw,
    /// A DirectDraw surface's pixels; `keyed` makes those its source blit
    /// key matches transparent.
    Win32Surface { id: u32, keyed: bool },
    Gus,
    Sound,
    Awe32,
    Drives,
    Mount {
        drive: String,
        path: String,
        kind: Option<String>,
        label: Option<String>,
        read_only: bool,
        images: Vec<String>,
    },
    Unmount { drive: String },
    SwapImages,
    /// Copy what a booted system changed on its shared host folders'
    /// disks into the folders.
    SyncShared,
    /// Save the machine to a save state file, or load one, which the front
    /// end does: it takes each from `next_state_request` and answers it
    /// with `state_done`. A `strict_match` save or load is
    /// done only while paused, and its load must match this machine and
    /// resets the debugger.
    /// A `check_files` save records the files on the writable drives,
    /// and a `check_files` load compares them (savestate/media.rs).
    /// A load's `folders` takes the state's folder drives it names (by
    /// number, 2 for C:) as mounted from those host folders instead.
    SaveState { path: String, strict_match: bool, check_files: bool },
    LoadState { path: String, strict_match: bool, check_files: bool, folders: Vec<(u8, String)> },
    /// Change the CPU speed (`cycles`) as the settings window does, which
    /// the front end does (`next_speed_request`).
    Speed { cycles: String },
    /// Turn the emulator off: the front end leaves its loop, and the
    /// journal ends with `shutdown`.
    Quit,
    /// Start or stop a profile (`/api/profile`).
    ProfileControl(profile::Control),
    /// Whether a profile runs.
    ProfileStatus,
    /// The profile's report.
    ProfileGet { query: rust_dos::profile::report::Query, format: profile::Format },
}

/// A save state to save or load, which the front end carries out
/// (`DebugHub::next_state_request`) and answers (`DebugHub::state_done`).
pub struct StateRequest {
    pub load: bool,
    pub path: PathBuf,
    /// Asked for with `strict_match` (see `next_state_request`).
    pub strict_match: bool,
    /// Asked for with `check_files`: record or compare the files on the
    /// writable drives.
    pub check_files: bool,
    /// A load's folder drives to take as mounted from other host folders
    /// (see `Cmd::LoadState`).
    pub folders: Vec<(u8, PathBuf)>,
    ctx: Ctx,
    if_state: Option<u64>,
    reply: oneshot::Sender<Answer>,
}

/// A CPU speed to change to, which the front end carries out
/// (`DebugHub::next_speed_request`) and answers (`DebugHub::speed_done`).
pub struct SpeedRequest {
    pub cycles: String,
    ctx: Ctx,
    if_state: Option<u64>,
    reply: oneshot::Sender<Answer>,
}

/// Why the emulator turned off, for the `shutdown` entry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ShutdownReason {
    /// `POST /api/control/quit`.
    Quit,
    /// The machine ran EXIT.
    Exit,
    /// The window was closed.
    Window,
}

// ---------------------------------------------------------------------------
// Input events (the public JSON schema)
// ---------------------------------------------------------------------------

#[derive(Deserialize, Debug, Clone)]
// A field the event doesn't have is refused: dropping it would send other
// input than the client asked for ("modifiers" for "mods" pressed the key
// without Alt).
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum InputEvent {
    Key {
        key: Option<String>,
        scancode: Option<u8>,
        ascii: Option<u8>,
        #[serde(default)]
        action: KeyAction,
        #[serde(default)]
        mods: Vec<String>,
        hold_ms: Option<u64>,
    },
    Type {
        text: String,
        delay_ms: Option<u64>,
    },
    Mouse {
        #[serde(default)]
        action: MouseAction,
        x: Option<i32>,
        y: Option<i32>,
        dx: Option<i32>,
        dy: Option<i32>,
        button: Option<String>,
        #[serde(default)]
        coords: Coords,
        hold_ms: Option<u64>,
    },
    Wait {
        ms: u64,
    },
}

#[derive(Deserialize, Debug, Clone, Copy, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum KeyAction {
    #[default]
    Press,
    Down,
    Up,
}

#[derive(Deserialize, Debug, Clone, Copy, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MouseAction {
    #[default]
    Move,
    Down,
    Up,
    Click,
}

/// Coordinate system for mouse positions. `screen` = pixels of the 640x400
/// screenshot (what a client looking at `/api/screenshot` sees); `virtual`
/// = the INT 33h driver's own coordinates.
#[derive(Deserialize, Debug, Clone, Copy, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Coords {
    #[default]
    Screen,
    Virtual,
}

/// Low-level queued input, applied by the main loop at frame boundaries.
pub(crate) enum LowInput {
    KeyDown { key: PcKey, ascii: u8 },
    /// A character no key types, as Alt and the keypad type it.
    Char(u8),
    KeyUp { key: PcKey },
    MouseTo { x: i32, y: i32, coords: Coords },
    MouseRel { dx: i32, dy: i32 },
    Button { idx: usize, down: bool },
    Wait(Duration),
    /// The end of input request `req`: its events before this one are
    /// delivered (`input_delivered`).
    End { req: u64 },
}

const DEFAULT_HOLD_MS: u64 = 50;

/// Remote input for the settings window while it is open.
pub enum UiInput {
    Key(UiKey),
    /// A left click at a screenshot (frame) pixel.
    Click(i32, i32),
}

// ---------------------------------------------------------------------------
// Main-thread side
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum PauseReason {
    Request,
    Breakpoint,
    Step,
    Exception,
    ModeSwitch,
    Watchpoint,
    ProgramStart,
    ProgramExit,
    /// Deterministic mode starts paused.
    Startup,
    /// A resume's `until_ms` reached.
    Time,
    /// A save state was loaded while paused: the machine is stopped in
    /// another state.
    Load,
}

impl PauseReason {
    fn name(self) -> &'static str {
        match self {
            PauseReason::Request => "request",
            PauseReason::Breakpoint => "breakpoint",
            PauseReason::Step => "step",
            PauseReason::Exception => "exception",
            PauseReason::ModeSwitch => "mode_switch",
            PauseReason::Watchpoint => "watchpoint",
            PauseReason::ProgramStart => "program_start",
            PauseReason::ProgramExit => "program_exit",
            PauseReason::Startup => "startup",
            PauseReason::Time => "time",
            PauseReason::Load => "load",
        }
    }
}

/// The breakpoint that stopped the machine: its address, whether a user
/// breakpoint is there (and was a one-shot one), and whether it was the
/// target of a resume `until` or a step-over.
#[derive(Clone, Copy)]
struct BreakpointHit {
    phys: usize,
    user: bool,
    once: bool,
    target: bool,
}

/// Memory the debugger watches: `len` bytes at physical address `phys`,
/// which held `value` when last looked at.
struct Watch {
    phys: usize,
    len: u8,
    value: u32,
}

/// A `run` operation waiting for its program to start, or with
/// `stop_at_entry` to stop there. Its command line is a batch line marked
/// with `op`; `before` is `cpu.programs.started` as the shell took it,
/// None until then.
struct RunWait {
    before: Option<u64>,
    stop_at_entry: bool,
    op: u64,
}

impl Watch {
    fn read(cpu: &Cpu, phys: usize, len: u8) -> u32 {
        (0..len as usize).fold(0, |v, i| v | (cpu.bus.peek_8(phys + i) as u32) << (8 * i))
    }
}

pub struct DebugHub {
    rx: Option<mpsc::Receiver<Request>>,
    shared: Option<Arc<Shared>>,

    pub paused: bool,
    step_budget: Option<u64>,
    breakpoints: HashSet<usize>,
    temp_breakpoint: Option<usize>,
    /// Skip the breakpoint check for the first instruction after resuming,
    /// so continuing from a breakpoint doesn't immediately re-trigger it.
    skip_bp_once: bool,
    pause_hit: Option<PauseReason>,
    /// The pause request that stopped the machine (reason `request`).
    pause_by: Option<u64>,
    breakpoint_hit: Option<BreakpointHit>,
    /// The breakpoints that go once they stop the machine.
    once: HashSet<usize>,
    /// The control request whose operation is under way.
    current_op: Option<u64>,
    /// The request being carried out.
    cur: Option<Ctx>,
    /// `cpu.programs.started` and `.ended` as the journal last told them.
    told_starts: u64,
    told_ends: u64,
    /// The last `program_started` entry.
    last_start_seq: Option<u64>,
    /// A quit request came in: the front end turns the emulator off.
    quit: Option<Ctx>,
    /// Window input reached the machine in this stop (`host_input`).
    host_input_told: bool,
    /// Exceptions (bit n = vector n) and mode switches to pause after, and
    /// the counts of them last seen.
    break_exceptions: u32,
    break_mode_switch: bool,
    seen_exceptions: u64,
    seen_mode_switches: u64,
    watchpoints: Vec<Watch>,
    /// The watchpoint that stopped the machine: its address and length,
    /// and the value before and after.
    watch_hit: Option<(usize, u8, u32, u32)>,
    /// Pause at the entry point of every program DOS starts, and after
    /// every program ends; `start_once` pauses at the next one's only.
    break_program_start: bool,
    break_program_exit: bool,
    start_once: bool,
    /// The entry point of the program just started, while the machine
    /// runs to it.
    start_breakpoint: Option<usize>,
    /// `cpu.programs.started` and `.ended` as last seen.
    seen_starts: u64,
    seen_ends: u64,
    /// A `run` command waiting for its program to start.
    run_wait: Option<RunWait>,
    /// The `paused` entry the machine is stopped in, kept here once the
    /// ring has dropped it.
    stop_entry: Option<Value>,

    trace: TraceRing,
    trace_enabled: bool,
    /// Stop recording once the trace's total reaches this.
    trace_until: Option<u64>,
    trace_stream_max: usize,
    trace_stream_cursor: u64,
    tracing_now: bool,
    batch_t_us: u64,

    input: VecDeque<LowInput>,
    input_wait_until: Option<Instant>,
    /// In deterministic mode: the date and time the clock started at.
    /// Input is then delivered by `feed_input` at emulated times, and its
    /// waits end in emulated time (`input_wait_ns`).
    pub deterministic: Option<chrono::NaiveDateTime>,
    input_wait_ns: Option<u64>,
    /// Pause once emulated time reaches this (`until_ms`).
    stop_at_ns: Option<u64>,
    key_stall_frames: u32,
    /// While the settings window is open, remote input goes to it
    /// (`take_ui_input`) instead of to the machine.
    pub divert: bool,
    /// While Alt+F11 rewinds, the front end loads a state every few frames,
    /// which the journal hears about only once rewinding ends: until then
    /// `If-State-Seq` and strict-match requests are refused.
    pub rewinding: bool,
    ui_input: Vec<UiInput>,
    /// Where the remote mouse is, for clicks on the settings window.
    ui_pointer: (i32, i32),
    /// Ctrl+F12 came in: open or close the settings window.
    hotkey: bool,
    /// Ctrl+Shift+S came in: show or hide the sound visualizer.
    sound_viz_key: bool,
    /// reboot_shell closed the program: the front end releases its part.
    closed_program: bool,
    /// Save states to save or load, for the front end.
    state_requests: Vec<StateRequest>,
    speed_requests: Vec<SpeedRequest>,
    /// Shift, Ctrl and Alt bits (as at 40:17h) the remote client holds.
    remote_mods: u8,
    /// Keys the remote client holds down on the machine.
    remote_held: Vec<PcKey>,

    frame_waiters: Vec<(Ctx, oneshot::Sender<Answer>)>,
    /// The video mode and picture size last reported as an event.
    last_mode: Option<(VideoMode, (u32, u32))>,
    frames: u64,
    fps: f64,
    fps_mark: (Instant, u64),
    stats: ExecStats,
    /// What the settings window's Stats page shows, as the front end last
    /// told it.
    frame_stats: Option<rust_dos::stats::StatsView>,
    /// The memory heatmap while a stream wants it.
    heat: Option<heat::Heat>,
    /// The last profile started (`/api/profile`), running or stopped.
    profile: Option<rust_dos::profile::session::Session>,
}

/// Execution speed, measured over windows of about a second of wall time.
struct ExecStats {
    window_start: Instant,
    /// Instructions executed and time spent executing them in the current window.
    executed: u64,
    exec_time: Duration,
    cache_mark: (u64, u64),
    /// Results of the last complete window.
    mips: f64,
    emulated_mips: f64,
    cache_hit_rate: f64,
    total_executed: u64,
}

impl ExecStats {
    fn new() -> Self {
        Self {
            window_start: Instant::now(),
            executed: 0,
            exec_time: Duration::ZERO,
            cache_mark: (0, 0),
            mips: 0.0,
            emulated_mips: 0.0,
            cache_hit_rate: 0.0,
            total_executed: 0,
        }
    }
}

impl DebugHub {
    /// A hub with no server attached. Every hook reduces to a couple of
    /// boolean checks.
    pub fn disabled() -> Self {
        Self::new(None, None, 1)
    }

    fn new(rx: Option<mpsc::Receiver<Request>>, shared: Option<Arc<Shared>>, trace_capacity: usize) -> Self {
        Self {
            rx,
            shared,
            paused: false,
            step_budget: None,
            breakpoints: HashSet::new(),
            temp_breakpoint: None,
            skip_bp_once: false,
            pause_hit: None,
            pause_by: None,
            breakpoint_hit: None,
            once: HashSet::new(),
            current_op: None,
            cur: None,
            told_starts: 0,
            told_ends: 0,
            last_start_seq: None,
            quit: None,
            host_input_told: false,
            break_exceptions: 0,
            break_mode_switch: false,
            seen_exceptions: 0,
            seen_mode_switches: 0,
            watchpoints: Vec::new(),
            watch_hit: None,
            break_program_start: false,
            break_program_exit: false,
            start_once: false,
            start_breakpoint: None,
            seen_starts: 0,
            seen_ends: 0,
            run_wait: None,
            stop_entry: None,
            trace: TraceRing::new(trace_capacity),
            trace_enabled: false,
            trace_until: None,
            trace_stream_max: 1000,
            trace_stream_cursor: 0,
            tracing_now: false,
            batch_t_us: 0,
            input: VecDeque::new(),
            input_wait_until: None,
            deterministic: None,
            input_wait_ns: None,
            stop_at_ns: None,
            key_stall_frames: 0,
            divert: false,
            rewinding: false,
            ui_input: Vec::new(),
            ui_pointer: (0, 0),
            hotkey: false,
            sound_viz_key: false,
            closed_program: false,
            state_requests: Vec::new(),
            speed_requests: Vec::new(),
            remote_mods: 0,
            remote_held: Vec::new(),
            frame_waiters: Vec::new(),
            last_mode: None,
            frames: 0,
            fps: 0.0,
            fps_mark: (Instant::now(), 0),
            stats: ExecStats::new(),
            frame_stats: None,
            heat: None,
            profile: None,
        }
    }

    /// Start the server thread and install the log/audio hooks on the bus.
    /// `deterministic` is the clock's start in deterministic mode, which
    /// the journal's `started` entry tells.
    pub fn start(
        cpu: &mut Cpu,
        addr: SocketAddr,
        trace_capacity: usize,
        deterministic: Option<chrono::NaiveDateTime>,
    ) -> Result<Self, String> {
        let shared = Arc::new(Shared::new(cpu.bus.start_time));
        let (tx, rx) = mpsc::channel();
        // With port 0 in `addr`, this is the free port the system picked.
        let addr = server::spawn(addr, tx, shared.clone())?;
        if !addr.ip().is_loopback() {
            eprintln!(
                "[DEBUG] WARNING: debug server bound to non-loopback address {}. It has no authentication!",
                addr
            );
        }

        let log_shared = shared.clone();
        cpu.bus.log_hook = Some(Box::new(move |line: &str| log_shared.log_line(line)));
        let audio_tx = shared.audio.clone();
        cpu.bus.audio_hook = Some(Box::new(move |samples: &[i16]| {
            if audio_tx.receiver_count() > 0 {
                let _ = audio_tx.send(Arc::from(samples));
            }
        }));

        println!("[DEBUG] Debug server listening on http://{}/", addr);
        let mut hub = Self::new(Some(rx), Some(shared), trace_capacity);
        hub.deterministic = deterministic;
        hub.announce(cpu);
        Ok(hub)
    }

    /// Append the journal's first entry, `started`, and take the program
    /// counts as they are.
    fn announce(&mut self, cpu: &Cpu) {
        let fields = json!({
            "version": env!("CARGO_PKG_VERSION"),
            "build": rust_dos::build_info::json(),
            "api_version": journal::API_VERSION,
            "features": journal::FEATURES,
            "deterministic": self.deterministic.map(|s| s.format("%Y-%m-%d %H:%M:%S").to_string()),
        });
        self.append(cpu, Kind::Started, None, fields);
        self.told_starts = cpu.programs.started;
        self.told_ends = cpu.programs.ended;
    }

    /// A hub with a journal and no server, which takes its requests from
    /// `request` and `handle`.
    #[cfg(test)]
    fn for_tests(cpu: &Cpu) -> Self {
        let shared = Arc::new(Shared::new(Instant::now()));
        let mut hub = Self::new(Some(mpsc::channel().1), Some(shared), 1000);
        hub.announce(cpu);
        hub
    }

    /// A request for `cmd` with the next request number, and its answer.
    #[cfg(test)]
    fn request(&self, cmd: Cmd) -> (Request, oneshot::Receiver<Answer>) {
        let req = self.shared.as_ref().map_or(1, |s| s.next_req.fetch_add(1, Ordering::Relaxed));
        let (reply, rx) = oneshot::channel();
        (Request { cmd, reply, req, tag: None, if_state: None }, rx)
    }

    fn emit(&self, v: Value) {
        if let Some(s) = &self.shared {
            s.emit(v);
        }
    }

    // ----- the journal -------------------------------------------------------

    /// Append an entry of `kind` with the object `fields`, caused by
    /// request `cause` (None: the machine or the window). Returns its
    /// sequence number; None without a server, or after the shutdown.
    fn append(&mut self, cpu: &Cpu, kind: Kind, cause: Option<u64>, fields: Value) -> Option<u64> {
        self.append_then(cpu, kind, cause, fields, |_, _| {})
    }

    /// `append`, then `then` with the journal and the entry's number, under
    /// the same lock, so that no reader sees one without the other.
    fn append_then(
        &mut self,
        cpu: &Cpu,
        kind: Kind,
        cause: Option<u64>,
        fields: Value,
        then: impl FnOnce(&mut Journal, u64),
    ) -> Option<u64> {
        let shared = self.shared.as_ref()?;
        let current = cause.and_then(|c| self.cur.as_ref().filter(|ctx| ctx.req == c));
        let fields = match fields {
            Value::Object(m) => m,
            _ => Map::new(),
        };
        let seq = {
            let mut j = shared.journal.lock().ok()?;
            // The request being carried out, or the operation it started.
            let request_id = match current {
                Some(ctx) => ctx.tag.clone(),
                None => cause.and_then(|c| j.op_tag(c)),
            };
            let stamp = Stamp {
                t_ms: shared.start_time.elapsed().as_millis() as u64,
                icount: cpu.executed,
                emu_ns: cpu.bus.clock.now_ns(),
                cause,
                request_id,
                op: self.current_op,
            };
            let seq = j.append(kind, stamp, fields)?;
            then(&mut j, seq);
            seq
        };
        shared.journal_seq.send_replace(seq);
        if let Some(ctx) = self.cur.as_mut().filter(|ctx| Some(ctx.req) == cause) {
            ctx.seq = Some(seq);
        }
        Some(seq)
    }

    /// Append an entry caused by the request being carried out.
    fn record(&mut self, cpu: &Cpu, kind: Kind, fields: Value) -> Option<u64> {
        let cause = self.cur.as_ref().map(|c| c.req);
        self.append(cpu, kind, cause, fields)
    }

    /// Wake the server's waits on the journal after a change that appended
    /// no entry (an operation that ended at an earlier one).
    fn wake(&self) {
        if let Some(shared) = &self.shared {
            shared.journal_seq.send_modify(|_| {});
        }
    }

    fn with_journal<T>(&self, f: impl FnOnce(&mut Journal) -> T) -> Option<T> {
        let shared = self.shared.as_ref()?;
        let mut j = shared.journal.lock().ok()?;
        Some(f(&mut j))
    }

    /// The journal's state for an answer to `ctx`'s request.
    fn meta(&self, ctx: &Ctx) -> Meta {
        let (state_seq, stop_seq, last) = self.with_journal(|j| (j.state_seq, j.stop_seq, j.last())).unwrap_or_default();
        Meta { state_seq, stop_seq, last, seq: ctx.seq, op: ctx.op }
    }

    /// Start carrying out a request: log lines name it from now on.
    fn begin(&mut self, ctx: Ctx) {
        if let Some(shared) = &self.shared
            && let Ok(mut current) = shared.current.lock()
        {
            *current = Some((ctx.req, ctx.tag.clone()));
        }
        self.cur = Some(ctx);
    }

    /// Done with the request being carried out: its context, with what it
    /// did to the journal.
    fn finish(&mut self) -> Ctx {
        if let Some(shared) = &self.shared
            && let Ok(mut current) = shared.current.lock()
        {
            *current = None;
        }
        self.cur.take().unwrap_or_default()
    }

    /// Answer `ctx`'s request with `reply` and the journal's state.
    fn answer(&self, ctx: &Ctx, tx: oneshot::Sender<Answer>, reply: Reply) {
        let _ = tx.send(Answer { reply, meta: self.meta(ctx) });
    }
}

#[cfg(test)]
mod determinism_tests;

#[cfg(test)]
mod tests;
