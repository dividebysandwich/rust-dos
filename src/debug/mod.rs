//! Remote debug / control interface.
//!
//! An HTTP + WebSocket server (see `server.rs`) runs on its own thread with
//! a private tokio runtime. It never touches the emulator directly: requests
//! are sent to the main thread through an mpsc channel and executed by
//! `DebugHub::poll` once per frame, and data flows back through oneshot
//! replies, broadcast channels, and a shared frame snapshot.

pub mod journal;
pub mod keys;
pub mod pm;
pub mod server;
pub mod trace;

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

use journal::{Journal, Kind, OpKind, OpState, Stamp};

use crate::config_ui::UiKey;
use crate::cpu::{Cpu, CpuFlags};
use crate::disk::{DriveKind, MountOptions, drive_letter};
use crate::keyboard;
use rust_dos::keylayout::Layout;
use crate::mount::{display_host_path, parse_drive_name, parse_kind};
use crate::video::{self, VideoMode};
use keys::PcKey;
pub use pm::{parse_addr, parse_hex};
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
    Trace { entries: Vec<TraceEntry>, next: u64, dropped: u64 },
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
    SetRegs(Map<String, Value>),
    ReadMem { addr: String, len: usize },
    /// Write `data` at an address; with `expect`, only if the bytes there
    /// match it.
    WriteMem { addr: String, data: Vec<u8>, expect: Option<Vec<u8>> },
    Disasm { addr: Option<String>, count: usize },
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
    Run { command: String, stop_at_entry: bool },
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
    Gus,
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
    SaveState { path: String, strict_match: bool, check_files: bool },
    LoadState { path: String, strict_match: bool, check_files: bool },
    /// Change the CPU speed (`cycles`) as the settings window does, which
    /// the front end does (`next_speed_request`).
    Speed { cycles: String },
    /// Turn the emulator off: the front end leaves its loop, and the
    /// journal ends with `shutdown`.
    Quit,
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
#[serde(tag = "type", rename_all = "snake_case")]
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

/// F12 in the key table: its make code.
const F12_SCAN: u8 = 0x58;

fn modifier_key(name: &str) -> Result<PcKey, String> {
    match keys::lookup(name) {
        Some(k) if k.modifier != 0 => Ok(k),
        _ => Err(format!("unknown modifier '{}' (use shift, ctrl, alt)", name)),
    }
}

/// Compute the ASCII byte a key produces under the given BDA modifier bits.
fn ascii_for(key: PcKey, mods: u8) -> u8 {
    if mods & keys::MOD_ALT != 0 {
        return 0;
    }
    if mods & keys::MOD_CTRL != 0 {
        return if key.ascii.is_ascii_alphabetic() { key.ascii & 0x1F } else { key.ascii };
    }
    if mods & (keys::MOD_LSHIFT | keys::MOD_RSHIFT) != 0 { key.shifted } else { key.ascii }
}

/// The keys that type `c` in `layout` (`keyboard::keys_for_char`), as
/// queued input.
pub(crate) fn keys_for_char(c: char, layout: &Layout, out: &mut Vec<LowInput>) -> Result<(), String> {
    let mut typed = Vec::new();
    keyboard::keys_for_char(c, layout, &mut typed)?;
    out.extend(typed.into_iter().map(|t| match t {
        keyboard::TypeStep::Down { key, ascii } => LowInput::KeyDown { key, ascii },
        keyboard::TypeStep::Up { key } => LowInput::KeyUp { key },
        keyboard::TypeStep::Char(byte) => LowInput::Char(byte),
    }));
    Ok(())
}

fn expand_input(ev: &InputEvent, layout: &Layout, out: &mut Vec<LowInput>) -> Result<(), String> {
    match ev {
        InputEvent::Key { key, scancode, ascii, action, mods, hold_ms } => {
            let mod_keys = mods.iter().map(|m| modifier_key(m)).collect::<Result<Vec<_>, _>>()?;
            let mod_bits = mod_keys.iter().fold(0u8, |a, k| a | k.modifier);
            let pc = match (key, scancode) {
                (Some(name), _) => keys::lookup(name).ok_or_else(|| {
                    format!("unknown key '{}'; valid keys: {}", name, keys::names().join(", "))
                })?,
                (None, Some(sc)) => PcKey { scan: *sc, ascii: ascii.unwrap_or(0), shifted: ascii.unwrap_or(0), modifier: 0, extended: false },
                (None, None) => return Err("key event needs 'key' or 'scancode'".into()),
            };
            let ch = match ascii {
                Some(a) if key.is_some() => *a,
                _ => ascii_for(pc, mod_bits),
            };
            if matches!(action, KeyAction::Press | KeyAction::Down) {
                for m in &mod_keys {
                    out.push(LowInput::KeyDown { key: *m, ascii: 0 });
                }
                out.push(LowInput::KeyDown { key: pc, ascii: ch });
            }
            if *action == KeyAction::Press {
                out.push(LowInput::Wait(Duration::from_millis(hold_ms.unwrap_or(DEFAULT_HOLD_MS))));
            }
            if matches!(action, KeyAction::Press | KeyAction::Up) {
                out.push(LowInput::KeyUp { key: pc });
                for m in mod_keys.iter().rev() {
                    out.push(LowInput::KeyUp { key: *m });
                }
            }
        }
        InputEvent::Type { text, delay_ms } => {
            for c in text.chars() {
                keys_for_char(c, layout, out)?;
                if let Some(d) = delay_ms.filter(|d| *d > 0) {
                    out.push(LowInput::Wait(Duration::from_millis(d)));
                }
            }
        }
        InputEvent::Mouse { action, x, y, dx, dy, button, coords, hold_ms } => {
            match (x, y) {
                (Some(x), Some(y)) => out.push(LowInput::MouseTo { x: *x, y: *y, coords: *coords }),
                (None, None) => {}
                _ => return Err("mouse event needs both x and y".into()),
            }
            if dx.is_some() || dy.is_some() {
                out.push(LowInput::MouseRel { dx: dx.unwrap_or(0), dy: dy.unwrap_or(0) });
            }
            let idx = match button.as_deref().unwrap_or("left") {
                "left" | "l" | "0" => 0,
                "right" | "r" | "1" => 1,
                "middle" | "m" | "2" => 2,
                b => return Err(format!("unknown mouse button '{}'", b)),
            };
            match action {
                MouseAction::Move => {}
                MouseAction::Down => out.push(LowInput::Button { idx, down: true }),
                MouseAction::Up => out.push(LowInput::Button { idx, down: false }),
                MouseAction::Click => {
                    out.push(LowInput::Button { idx, down: true });
                    out.push(LowInput::Wait(Duration::from_millis(hold_ms.unwrap_or(DEFAULT_HOLD_MS))));
                    out.push(LowInput::Button { idx, down: false });
                }
            }
        }
        InputEvent::Wait { ms } => out.push(LowInput::Wait(Duration::from_millis(*ms))),
    }
    Ok(())
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

/// A `run` operation waiting for its program to start (`before` is
/// `cpu.programs.started` as it was queued), or with `stop_at_entry` to
/// stop there.
struct RunWait {
    before: u64,
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

    /// Append the `paused` entry of the stop the batch or a request made,
    /// if there is one, ending the operation under way. Returns the entry.
    fn flush_stop(&mut self, cpu: &Cpu) -> Option<Value> {
        let reason = self.pause_hit.take()?;
        let mut stop = json!({"reason": reason.name()});
        if let Some(hit) = self.breakpoint_hit.take() {
            if hit.user {
                stop["breakpoint"] = format!("{:05X}", hit.phys).into();
                stop["once"] = hit.once.into();
            }
            if hit.target {
                stop["target"] = format!("{:05X}", hit.phys).into();
            }
        }
        match reason {
            PauseReason::ProgramStart => stop["program"] = program_json(cpu),
            PauseReason::ProgramExit => stop["exit"] = exit_json(cpu),
            _ => {}
        }
        if reason == PauseReason::Exception
            && let Some(e) = pm::exceptions_json(cpu)["recent"].as_array().and_then(|a| a.last().cloned())
        {
            stop["exception"] = e;
        }
        if let Some((phys, len, old, new)) = self.watch_hit.take() {
            let digits = len as usize * 2;
            stop["watch"] = json!({
                "addr": format!("{:05X}", phys),
                "len": len,
                "old": format!("{:0width$X}", old, width = digits),
                "new": format!("{:0width$X}", new, width = digits),
            });
        }
        let by = self.pause_by.take();
        if reason == PauseReason::Request {
            stop["by"] = by.into();
        }
        stop["cs_ip"] = cs_ip(cpu).into();
        stop["registers"] = regs_json(cpu);
        if self.trace.total() > 0 {
            stop["trace"] = self.trace.total().into();
        }
        if reason == PauseReason::ProgramStart && self.run_wait.as_ref().is_some_and(|r| r.stop_at_entry) {
            self.run_wait = None;
        }
        self.host_input_told = false;
        let op = self.current_op;
        // A `run` whose operation this stop ends waits no more: it no
        // longer stops at its program's entry or blocks another `run`.
        if op.is_some() && self.run_wait.as_ref().is_some_and(|r| Some(r.op) == op) {
            self.run_wait = None;
            self.start_once = false;
        }
        let cause = match reason {
            PauseReason::Request => by,
            PauseReason::Startup => None,
            PauseReason::Load => self.cur.as_ref().map(|c| c.req),
            _ => op,
        };
        let mut fallback = stop.clone();
        let seq = self.append_then(cpu, Kind::Paused, cause, stop, |j, seq| {
            if let Some(op) = op {
                j.end_op(op, OpState::Stopped, Some(seq));
            }
        });
        self.current_op = None;
        // The whole entry, as the journal gives it.
        if let Some(entry) = seq.and_then(|s| self.with_journal(|j| j.get(s).map(|e| e.value())).flatten()) {
            self.stop_entry = Some(entry.clone());
            return Some(entry);
        }
        fallback["icount"] = cpu.executed.into();
        Some(fallback)
    }

    /// The `paused` entry the machine is stopped in, if it is.
    fn current_stop(&self) -> Option<Value> {
        let seq = self.with_journal(|j| j.stop_seq).flatten()?;
        self.stop_entry.as_ref().filter(|e| e["seq"].as_u64() == Some(seq)).cloned()
    }

    /// Start a control request's operation of `kind`: it takes over the
    /// machine from the one under way, which is superseded, and resumes
    /// it, appending `resumed` with `fields`.
    fn start_op(&mut self, cpu: &Cpu, kind: OpKind, mut fields: Value) {
        let Some((req, tag)) = self.cur.as_ref().map(|c| (c.req, c.tag.clone())) else { return };
        let supersedes = self.current_op.replace(req);
        fields["kind"] = kind.name().into();
        fields["from_paused"] = self.paused.into();
        fields["supersedes"] = supersedes.into();
        self.resume();
        self.append_then(cpu, Kind::Resumed, Some(req), fields, |j, seq| {
            if let Some(old) = supersedes {
                j.supersede_op(old, req);
            }
            j.begin_op(req, kind, seq, tag);
        });
        if let Some(ctx) = self.cur.as_mut() {
            ctx.op = Some(req);
        }
    }

    /// A control request takes over the machine: the step count, the
    /// run-to target and the time stop (`until_ms`) of the operation it
    /// supersedes stop nothing more.
    /// A `run` whose operation is superseded doesn't stop at its program's
    /// entry and no longer blocks another `run` or a strict-match save; the
    /// command line it queued still runs.
    fn take_over(&mut self) {
        self.step_budget = None;
        self.temp_breakpoint = None;
        self.stop_at_ns = None;
        if self.run_wait.as_ref().is_some_and(|r| self.current_op == Some(r.op)) {
            self.run_wait = None;
            self.start_once = false;
        }
    }

    /// Program starts and ends since the journal last told them, as
    /// `program_started` and `program_exited` (with `count` when there were
    /// several in the batch). When both came in one batch, the start goes
    /// first if the program that started no longer runs (its PSP isn't the
    /// current one): it ended in the batch too.
    fn tell_programs(&mut self, cpu: &Cpu) {
        let (started, ended) = (cpu.programs.started, cpu.programs.ended);
        let new_start = started > self.told_starts;
        let new_end = ended > self.told_ends;
        let start_first = new_start && new_end && cpu.current_psp != cpu.programs.psp;
        if new_end && !start_first {
            self.tell_exit(cpu, ended);
        }
        if new_start {
            let mut fields = json!({"program": program_json(cpu)});
            if started - self.told_starts > 1 {
                fields["count"] = (started - self.told_starts).into();
            }
            // A `run` that doesn't stop at the entry is done with this entry.
            let run = self.run_wait.as_ref().filter(|r| !r.stop_at_entry && started > r.before).map(|r| r.op);
            let seq = self.append_then(cpu, Kind::ProgramStarted, None, fields, |j, seq| {
                if let Some(op) = run {
                    j.end_op(op, OpState::Done, Some(seq));
                }
            });
            self.last_start_seq = seq.or(self.last_start_seq);
            if let Some(op) = run.filter(|_| seq.is_some()) {
                self.run_wait = None;
                if self.current_op == Some(op) {
                    self.current_op = None;
                }
            }
        }
        if start_first {
            self.tell_exit(cpu, ended);
        }
        // A reboot or a load can take the counts back.
        self.told_starts = started;
        self.told_ends = ended;
    }

    /// The `program_exited` entry of the ends since the journal last told
    /// them, `ended` now.
    fn tell_exit(&mut self, cpu: &Cpu, ended: u64) {
        let mut fields = json!({"exit": exit_json(cpu)});
        if ended - self.told_ends > 1 {
            fields["count"] = (ended - self.told_ends).into();
        }
        self.append(cpu, Kind::ProgramExited, None, fields);
    }

    /// Whether `need` holds now; if not, the 409 body that says why.
    pub fn check(&self, need: Need) -> Result<(), Value> {
        let (stop_seq, state_seq, pending_ops) = self
            .with_journal(|j| (j.stop_seq, j.state_seq, j.pending_ops(|k| k != OpKind::Input)))
            .unwrap_or((None, 0, Vec::new()));
        let input_queued = self.input_pending();
        let why: Option<(&str, String)> = match need {
            Need::Paused => self.deterministic_refuses().then(|| {
                (
                    "paused",
                    "deterministic mode takes input and changes to the machine only while it is paused: \
                     pause it, or resume with until_ms, then send the request"
                        .to_string(),
                )
            }),
            Need::Quiescent { input_ok } => {
                let why = if !self.paused {
                    Some("the machine runs; pause it first (POST /api/control/pause)")
                } else if self.rewinding {
                    Some("the machine is being rewound")
                } else if self.step_budget.is_some() {
                    Some("a step is pending")
                } else if self.run_wait.is_some() {
                    Some("a run command waits for its program")
                } else if !input_ok && input_queued > 0 {
                    Some("queued input is still being typed")
                } else {
                    None
                };
                why.map(|w| ("quiescent", w.to_string()))
            }
            Need::State(n) => {
                // Input goes on being typed while paused, but in
                // deterministic mode, where it waits for emulated time.
                let typing = input_queued > 0 && (self.deterministic.is_none() || self.divert);
                let why = if !self.paused || stop_seq.is_none() {
                    Some("the machine runs; pause it, then read its state again".to_string())
                } else if self.divert {
                    Some("the settings window is open".to_string())
                } else if self.rewinding {
                    Some("the machine is being rewound".to_string())
                } else if !pending_ops.is_empty() {
                    Some("an operation is under way".to_string())
                } else if typing {
                    Some("queued input is being typed".to_string())
                } else if state_seq != n {
                    Some(format!("the state changed since entry {}: it is now at entry {}", n, state_seq))
                } else {
                    None
                };
                why.map(|w| ("state", w))
            }
        };
        match why {
            None => Ok(()),
            Some((precondition, error)) => Err(json!({
                "error": error,
                "precondition": precondition,
                "paused": self.paused,
                "stop_seq": stop_seq,
                "state_seq": state_seq,
                "pending_ops": pending_ops,
                "input_queued": input_queued,
                "settings_window": self.divert,
            })),
        }
    }

    /// The input events still to be delivered, counting a wait under way:
    /// an input request whose last event is a wait isn't delivered until
    /// the wait ends.
    fn input_pending(&self) -> usize {
        let waiting = self.input_wait_until.is_some() || self.input_wait_ns.is_some();
        self.input.iter().filter(|e| !matches!(e, LowInput::End { .. })).count() + waiting as usize
    }

    /// Whether a remote client asked to turn the emulator off.
    pub fn quit_requested(&self) -> bool {
        self.quit.is_some()
    }

    /// The emulator turns off: what is pending is cancelled, the journal
    /// ends with `shutdown`, requests still coming get 503, and the server
    /// gets a moment to send what it has.
    pub fn shutdown(&mut self, cpu: &Cpu, reason: ShutdownReason, exit_code: i32) {
        let Some(shared) = self.shared.clone() else { return };
        let by = self.quit.take();
        let cancelled = self.with_journal(|j| j.pending_ops(|_| true)).unwrap_or_default();
        self.current_op = None;
        let fields = json!({
            "reason": match reason {
                ShutdownReason::Quit => "quit",
                ShutdownReason::Exit => "exit",
                ShutdownReason::Window => "window",
            },
            "by": by.as_ref().map(|c| c.req),
            "cancelled": cancelled,
            "exit_code": exit_code,
        });
        if let Some(ctx) = by {
            self.begin(ctx);
        }
        let ids = cancelled.clone();
        self.append_then(cpu, Kind::Shutdown, self.cur.as_ref().map(|c| c.req), fields, move |j, seq| {
            for req in ids {
                j.end_op(req, OpState::Cancelled, Some(seq));
            }
        });
        self.finish();
        shared.closed.store(true, Ordering::Release);
        let gone = || Reply::Error(503, "the emulator is shutting down".into());
        for (ctx, tx) in std::mem::take(&mut self.frame_waiters) {
            self.answer(&ctx, tx, gone());
        }
        // Requests still queued get 503, and the channel closes: one sent
        // later fails at once, and the server answers it with 503 too.
        if let Some(rx) = self.rx.take() {
            while let Ok(req) = rx.try_recv() {
                let _ = req.reply.send(Answer { reply: gone(), meta: Meta::default() });
            }
        }
        shared.drain(Duration::from_secs(2));
    }

    /// Window input reached the machine while the debugger holds it: one
    /// `host_input` entry a stop.
    pub fn host_input(&mut self, cpu: &Cpu) {
        if self.paused && !self.host_input_told {
            self.host_input_told = true;
            self.append(cpu, Kind::HostInput, None, json!({}));
        }
    }

    /// The settings window opened or closed.
    pub fn settings_window(&mut self, cpu: &Cpu, open: bool) {
        self.append(cpu, Kind::SettingsWindow, None, json!({"open": open}));
    }

    /// The speed changed otherwise than through the debug server.
    pub fn speed_changed(&mut self, cpu: &Cpu, cycles: &str, source: &str) {
        self.append(cpu, Kind::SpeedChanged, None, json!({"cycles": cycles, "source": source}));
    }

    /// A save state was loaded otherwise than through the debug server:
    /// from the window (`source` "window") or by rewinding ("rewind").
    pub fn state_loaded_elsewhere(&mut self, cpu: &Cpu, source: &str) {
        self.told_starts = cpu.programs.started;
        self.told_ends = cpu.programs.ended;
        self.append(cpu, Kind::StateLoaded, None, json!({"source": source}));
        if self.paused {
            self.pause_hit = Some(PauseReason::Load);
            self.flush_stop(cpu);
        }
    }

    // ----- per-frame hooks ------------------------------------------------

    /// Drain pending requests and feed queued input. Call once per frame,
    /// before the execution batch.
    pub fn poll(&mut self, cpu: &mut Cpu) {
        let Some(rx) = self.rx.take() else { return };
        // A save state or speed request is carried out by the front end
        // after this, before any request behind it, so that requests take
        // effect in the order they came: those behind it wait a frame. After
        // a quit nothing more is carried out.
        while self.state_requests.is_empty() && self.speed_requests.is_empty() && self.quit.is_none() {
            let Ok(req) = rx.try_recv() else { break };
            self.handle(cpu, req);
        }
        self.rx = Some(rx);

        // Deterministic mode delivers the machine's input at its ticks
        // (`feed_input`); the settings window's goes as it comes.
        if self.deterministic.is_none() || self.divert {
            self.process_input(cpu);
        }
        self.check_run(cpu);

        let mode = (cpu.bus.video_mode, video::frame_size(&cpu.bus));
        if self.last_mode != Some(mode) {
            let (w, h) = cpu.bus.video_mode.dimensions();
            self.emit(json!({
                "type": "video_mode",
                "mode": cpu.bus.video_mode as u8,
                "name": format!("{:?}", cpu.bus.video_mode),
                "width": w, "height": h,
                "frame": {"width": mode.1.0, "height": mode.1.1},
            }));
            self.last_mode = Some(mode);
        }
    }

    /// Prepare for an execution batch. Returns true when the per-instruction
    /// hook (`before_exec`) must run.
    #[inline]
    pub fn begin_batch(&mut self, cpu: &Cpu) -> bool {
        let streaming = self.shared.as_ref().is_some_and(|s| s.trace.receiver_count() > 0);
        self.tracing_now = self.trace_enabled || streaming;
        if self.tracing_now {
            self.batch_t_us = cpu.bus.start_time.elapsed().as_micros() as u64;
        }
        self.tracing_now
            || self.step_budget.is_some()
            || self.temp_breakpoint.is_some()
            || !self.breakpoints.is_empty()
            || self.break_exceptions != 0
            || self.break_mode_switch
            || !self.watchpoints.is_empty()
            || self.break_program_start
            || self.break_program_exit
            || self.start_once
            || self.start_breakpoint.is_some()
    }

    /// Per-instruction hook, called just before an instruction at `phys_ip`
    /// executes. Returns true if execution must stop (the hub is now paused).
    #[inline]
    fn check_before_exec(&mut self, cpu: &Cpu, phys_ip: usize, ram: &[u8]) -> bool {
        // Memory the last instruction (or an interrupt handler of the
        // emulator's) changed.
        for w in &mut self.watchpoints {
            let now = Watch::read(cpu, w.phys, w.len);
            if now != w.value {
                self.watch_hit = Some((w.phys, w.len, w.value, now));
                w.value = now;
                self.enter_pause(PauseReason::Watchpoint);
                return true;
            }
        }
        if cpu.exceptions != self.seen_exceptions {
            self.seen_exceptions = cpu.exceptions;
            let hit = cpu.exception_log.back().is_some_and(|e| e.vector < 32 && self.break_exceptions & (1 << e.vector) != 0);
            if hit {
                self.enter_pause(PauseReason::Exception);
                return true;
            }
        }
        if cpu.mode_switches != self.seen_mode_switches {
            self.seen_mode_switches = cpu.mode_switches;
            if self.break_mode_switch {
                self.enter_pause(PauseReason::ModeSwitch);
                return true;
            }
        }
        if cpu.programs.ended != self.seen_ends {
            self.seen_ends = cpu.programs.ended;
            if self.break_program_exit {
                self.enter_pause(PauseReason::ProgramExit);
                return true;
            }
        }
        // A program just started: run to its entry point, through any
        // interrupt that comes first.
        if cpu.programs.started != self.seen_starts {
            self.seen_starts = cpu.programs.started;
            // This start uses up `run`'s one stop, even when every start
            // stops anyway.
            let once = std::mem::take(&mut self.start_once);
            if self.break_program_start || once {
                self.start_breakpoint = Some(cpu.programs.entry);
            }
        }
        if self.start_breakpoint == Some(phys_ip) {
            self.start_breakpoint = None;
            self.enter_pause(PauseReason::ProgramStart);
            return true;
        }
        if !self.skip_bp_once
            && (self.temp_breakpoint == Some(phys_ip) || self.breakpoints.contains(&phys_ip))
        {
            let target = self.temp_breakpoint.take() == Some(phys_ip);
            let user = self.breakpoints.contains(&phys_ip);
            let once = user && self.once.remove(&phys_ip);
            if once {
                self.breakpoints.remove(&phys_ip);
            }
            self.breakpoint_hit = Some(BreakpointHit { phys: phys_ip, user, once, target });
            self.enter_pause(PauseReason::Breakpoint);
            return true;
        }
        self.skip_bp_once = false;

        if let Some(n) = self.step_budget {
            if n == 0 {
                self.step_budget = None;
                self.enter_pause(PauseReason::Step);
                return true;
            }
            self.step_budget = Some(n - 1);
        }

        if self.tracing_now {
            let mut bytes = [0u8; 15];
            let end = (phys_ip + 15).min(ram.len());
            let len = end.saturating_sub(phys_ip);
            bytes[..len].copy_from_slice(&ram[phys_ip..end]);
            self.trace.push(TraceEntry {
                t_us: self.batch_t_us,
                icount: cpu.executed,
                cs: cpu.cs(),
                eip: cpu.eip(),
                gpr: [
                    cpu.eax(),
                    cpu.ecx(),
                    cpu.edx(),
                    cpu.ebx(),
                    cpu.esp(),
                    cpu.ebp(),
                    cpu.esi(),
                    cpu.edi(),
                ],
                ds: cpu.ds(),
                es: cpu.es(),
                ss: cpu.ss(),
                fs: cpu.fs(),
                gs: cpu.gs(),
                eflags: cpu.get_cpu_flags().bits(),
                code32: cpu.seg_cache(crate::cpu::Seg::CS).attr & crate::cpu::ATTR_DB != 0,
                bytes,
                len: len as u8,
            });
            if self.trace_until.is_some_and(|n| self.trace.total() >= n) {
                self.trace_until = None;
                self.trace_enabled = false;
                self.tracing_now = self.shared.as_ref().is_some_and(|s| s.trace.receiver_count() > 0);
            }
        }
        false
    }

    /// Call after the execution batch: delivers pause notifications and
    /// streams new trace entries.
    pub fn end_batch(&mut self, cpu: &Cpu) {
        // Programs that started or ended in the batch, then its stop.
        if self.shared.is_some() {
            self.tell_programs(cpu);
        }
        self.flush_stop(cpu);

        if let Some(shared) = &self.shared {
            if shared.trace.receiver_count() > 0 {
                let (entries, dropped) = self.trace.since(self.trace_stream_cursor, self.trace_stream_max);
                self.trace_stream_cursor = self.trace.total();
                if !entries.is_empty() || dropped > 0 {
                    let msg = json!({
                        "type": "trace",
                        "dropped": dropped,
                        "entries": entries.iter().map(|e| e.to_json()).collect::<Vec<_>>(),
                    });
                    let _ = shared.trace.send(Arc::from(msg.to_string()));
                }
            } else {
                self.trace_stream_cursor = self.trace.total();
            }
        }
    }

    /// Account for one execution batch: `executed` instructions (not counting
    /// time skipped while halted) took `exec_time` of host time. `cache_hits`
    /// and `cache_misses` are the decode cache's running totals.
    pub fn record_batch(&mut self, executed: u64, exec_time: Duration, cache_hits: u64, cache_misses: u64) {
        let st = &mut self.stats;
        st.executed += executed;
        st.exec_time += exec_time;
        st.total_executed += executed;
        let wall = st.window_start.elapsed();
        if wall >= Duration::from_secs(1) {
            let exec_secs = st.exec_time.as_secs_f64();
            st.mips = if exec_secs > 0.0 { st.executed as f64 / exec_secs / 1e6 } else { 0.0 };
            st.emulated_mips = st.executed as f64 / wall.as_secs_f64() / 1e6;
            // A loaded save state starts the decode cache's counts again.
            let hits = cache_hits.checked_sub(st.cache_mark.0).unwrap_or(cache_hits);
            let misses = cache_misses.checked_sub(st.cache_mark.1).unwrap_or(cache_misses);
            st.cache_hit_rate = if hits + misses > 0 { hits as f64 / (hits + misses) as f64 } else { 0.0 };
            st.cache_mark = (cache_hits, cache_misses);
            st.executed = 0;
            st.exec_time = Duration::ZERO;
            st.window_start = Instant::now();
        }
    }

    fn stats_json(&self, cpu: &Cpu) -> Value {
        fn dynrec_exits(d: &rust_dos::dynrec::DynStats) -> Value {
            const NAMES: [&str; 16] = [
                "", "fault", "smc", "panic", "next", "deadline", "stale", "gp0", "limit", "unlinked", "de", "watched",
                "after", "next_page", "env", "far_env",
            ];
            let mut map = serde_json::Map::new();
            for (k, name) in NAMES.iter().enumerate().skip(1) {
                if d.exits[k] != 0 {
                    map.insert(name.to_string(), json!(d.exits[k]));
                }
            }
            Value::Object(map)
        }
        fn dynrec_calls(calls: Option<&rust_dos::dynrec::Calls>) -> Value {
            let Some(c) = calls else { return Value::Null };
            let mut top: Vec<(usize, u64)> = c.fallback.iter().copied().enumerate().filter(|&(_, n)| n != 0).collect();
            top.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
            let names: Vec<_> = iced_x86::Mnemonic::values().collect();
            let fallback: Vec<Value> =
                top.iter().take(30).map(|&(m, n)| json!([format!("{:?}", names[m]), n])).collect();
            json!({
                "fallback": c.fallback.iter().sum::<u64>(),
                "fallback_top": fallback,
                "revalidate": c.revalidate,
                "memref": c.memref,
                "slow": c.slow,
                "dev": c.dev,
                "port": c.port,
                "load_seg": c.load_seg,
                "fpu": c.fpu,
            })
        }
        let st = &self.stats;
        let d = cpu.dynrec.stats();
        json!({
            // The `core` setting, and whether the dynamic recompiler runs
            // the instructions now.
            "core": cpu.core.name(),
            "recompiler": cpu.dynamic_active(),
            // The recompiler's counts since start: blocks translated and
            // their instructions (native: into host code; the others call
            // their interpreter handlers), blocks translated now, their
            // host code and the links between them, flushes of all of it,
            // blocks entered from the execution loop and the instructions
            // the translated code ran, and blocks that stopped at once for
            // the timer deadline, or because their code had changed
            // (stale) or changed under them (smc).
            "dynrec": {
                "blocks": d.blocks,
                "instructions": d.instructions,
                "native": d.native,
                "live_blocks": d.live_blocks,
                "code_bytes": d.code_bytes,
                "links": d.links,
                "flushes": d.flushes,
                "runs": d.runs,
                "executed": d.executed,
                "deadline": d.deadline,
                "stale": d.stale,
                "smc": d.smc,
                "watched": d.watched,
                // Exits of translated code by their kind, and the calls it
                // made into Rust, with the instructions that went through
                // their handlers most (for profiling).
                "exits": dynrec_exits(&d),
                "return_misses": d.return_misses,
                "calls": dynrec_calls(cpu.dynrec.calls()),
            },
            // Busy-wait loops proven to change nothing, proofs that failed,
            // and the instructions skipped (`idle`).
            "idle_skip": {
                "proofs": cpu.bus.observe.stats.proofs,
                "failures": cpu.bus.observe.stats.failures,
                "skipped": cpu.bus.observe.stats.skipped,
                "failure": cpu.bus.observe.stats.failure,
            },
            // Host speed while executing guest code, excluding idle skips,
            // rendering and frame pacing.
            "mips": (st.mips * 100.0).round() / 100.0,
            // Guest instructions per wall-clock second, as the program sees it.
            "emulated_mips": (st.emulated_mips * 100.0).round() / 100.0,
            "decode_cache_hit_rate": (st.cache_hit_rate * 10000.0).round() / 10000.0,
            "instructions": st.total_executed,
            "fps": (self.fps * 10.0).round() / 10.0,
        })
    }

    /// Count a frame of the window's for `fps`, shown anew or not.
    pub fn count_frame(&mut self) {
        self.frames += 1;
        let (mark_t, mark_n) = self.fps_mark;
        let dt = mark_t.elapsed().as_secs_f64();
        if dt >= 1.0 {
            self.fps = (self.frames - mark_n) as f64 / dt;
            self.fps_mark = (Instant::now(), self.frames);
        }
    }

    /// Whether `capture_frame` wants to see the composited frame this frame.
    pub fn wants_frame(&self) -> bool {
        !self.frame_waiters.is_empty()
            || self.shared.as_ref().is_some_and(|s| s.screen_subscribers.load(Ordering::Relaxed) > 0)
    }

    /// Hand the composited frame (cached VGA render + cursor overlays) to
    /// screenshot requests and the screen stream.
    pub fn capture_frame(&mut self, screen: &video::Frame) {
        self.count_frame();
        if !self.wants_frame() {
            return;
        }
        for (ctx, w) in std::mem::take(&mut self.frame_waiters) {
            let meta = self.meta(&ctx);
            let _ = w.send(Answer { reply: Reply::Frame(screen.clone()), meta });
        }
        if let Some(shared) = &self.shared {
            if shared.screen_subscribers.load(Ordering::Relaxed) > 0 {
                if let Ok(mut frame) = shared.frame.lock() {
                    if *frame != *screen {
                        frame.clone_from(screen);
                        shared.frame_seq.fetch_add(1, Ordering::Release);
                    }
                }
            }
        }
    }

    /// End a `run` operation whose program started, or whose command line
    /// ended at the prompt without starting one. A `run` goes on when its
    /// client stops waiting: how it ends is in the journal.
    fn check_run(&mut self, cpu: &Cpu) {
        let idle = at_idle_prompt(cpu);
        if idle {
            // No program runs, so drop an entry point it never reached
            // (it was closed first).
            self.start_breakpoint = None;
        }
        let Some(run) = &self.run_wait else { return };
        let (op, before, stop_at_entry) = (run.op, run.before, run.stop_at_entry);
        if cpu.programs.started > before {
            if !stop_at_entry {
                // A start the journal didn't tell (a load brought it): the
                // run is done with the last start told.
                self.run_wait = None;
                let end = self.last_start_seq;
                self.with_journal(|j| j.end_op(op, OpState::Done, end));
                self.wake();
                if self.current_op == Some(op) {
                    self.current_op = None;
                }
            } else if idle {
                self.run_wait = None;
                self.fail_op(cpu, op, 409, "the program ended before it reached its entry point");
            }
        } else if idle {
            self.start_once = false;
            self.run_wait = None;
            self.fail_op(cpu, op, 422, "the command line started no program");
        }
    }

    /// End operation `op` as failed with `status`, appending `op_failed`.
    /// An operation that already ended (a `run` another control request
    /// superseded, or a pause stopped) is left as it is.
    fn fail_op(&mut self, cpu: &Cpu, op: u64, status: u16, error: &str) {
        let pending = self.with_journal(|j| matches!(j.op(op), journal::OpLookup::Found(o) if o.state == OpState::Pending));
        if pending == Some(true) {
            let message = error.to_string();
            let fields = json!({"status": status, "error": error});
            self.append_then(cpu, Kind::OpFailed, Some(op), fields, move |j, seq| j.fail_op(op, Some(seq), status, message));
        }
        if self.current_op == Some(op) {
            self.current_op = None;
        }
    }

    fn enter_pause(&mut self, reason: PauseReason) {
        self.paused = true;
        self.step_budget = None;
        self.pause_hit = Some(reason);
    }

    fn resume(&mut self) {
        if self.paused {
            self.paused = false;
            self.skip_bp_once = true;
        }
    }

    /// Deterministic mode with a client: the machine waits for it before
    /// running anything.
    pub fn pause_at_start(&mut self, cpu: &Cpu) {
        self.enter_pause(PauseReason::Startup);
        self.flush_stop(cpu);
    }

    /// Pause if emulated time reached a resume's `until_ms`. The
    /// deterministic front end asks at every emulated millisecond.
    pub fn time_stop(&mut self, cpu: &Cpu) -> bool {
        if self.stop_at_ns.is_some_and(|at| cpu.bus.clock.now_ns() >= at) {
            self.stop_at_ns = None;
            self.enter_pause(PauseReason::Time);
            return true;
        }
        false
    }

    /// Deliver queued input, in deterministic mode: the front end calls
    /// this at fixed points in emulated time.
    pub fn feed_input(&mut self, cpu: &mut Cpu) {
        self.process_input(cpu);
    }

    /// Deterministic mode: run a millisecond of emulated time at a time up
    /// to `end`, a pause, the program's exit, or the time a client asked
    /// to stop at. That time is checked before `end`, so that a frame
    /// ending at it pauses there. `at_ms` runs at every whole millisecond
    /// reached; at a tick it delivers the input.
    pub fn run_deterministic(
        &mut self,
        cpu: &mut Cpu,
        mode: &mut rust_dos::deterministic::Deterministic,
        hot: bool,
        end: u64,
        mut at_ms: impl FnMut(&mut Cpu, &mut DebugHub, rust_dos::deterministic::Boundary),
    ) {
        while !self.paused && !self.time_stop(cpu) && cpu.bus.clock.icount < end {
            let (reason, reached) = mode.step(cpu, self, hot, end);
            if let Some(boundary) = reached {
                // Program starts and ends are told at the millisecond, so
                // that the journal doesn't depend on the host's frames.
                if self.shared.is_some() {
                    self.tell_programs(cpu);
                }
                at_ms(cpu, self, boundary);
            }
            if matches!(reason, crate::exec::StopReason::Paused | crate::exec::StopReason::Exit) {
                break;
            }
        }
    }

    // ----- input ------------------------------------------------------------

    fn process_input(&mut self, cpu: &mut Cpu) {
        let now = Instant::now();
        let now_ns = cpu.bus.clock.now_ns();
        if let Some(t) = self.input_wait_until {
            if now < t {
                return;
            }
            self.input_wait_until = None;
        }
        // Emulated time stands still while the settings window pauses the
        // machine: a wait in emulated time would never end.
        if self.divert {
            self.input_wait_ns = None;
        }
        if let Some(t) = self.input_wait_ns {
            if now_ns < t {
                return;
            }
            self.input_wait_ns = None;
        }
        while let Some(ev) = self.input.front() {
            match ev {
                LowInput::KeyDown { .. } | LowInput::KeyUp { .. } => {
                    // Let the program catch up with the keyboard controller's
                    // queue before adding more, but don't stall forever on
                    // programs that never read the port. The settings window
                    // takes keys at once.
                    if !self.divert && cpu.bus.kbc.pending() > 0 && self.key_stall_frames < 2 {
                        self.key_stall_frames += 1;
                        return;
                    }
                    self.key_stall_frames = 0;
                    match self.input.pop_front().unwrap() {
                        LowInput::KeyDown { key, ascii } => self.key_down(cpu, key, ascii),
                        LowInput::KeyUp { key } => self.key_up(cpu, key),
                        _ => unreachable!(),
                    }
                    // One scan code per frame.
                    return;
                }
                LowInput::Wait(d) if self.deterministic.is_some() && !self.divert => {
                    self.input_wait_ns = Some(now_ns + d.as_nanos() as u64);
                    self.input.pop_front();
                    return;
                }
                LowInput::Wait(d) => {
                    self.input_wait_until = Some(now + *d);
                    self.input.pop_front();
                    return;
                }
                _ => {}
            }
            match self.input.pop_front().unwrap() {
                // Typed as Alt and the keypad type it: a keystroke with no
                // scan code.
                LowInput::Char(byte) if !self.divert => {
                    keyboard::queue_keystroke(&mut cpu.bus, byte as u16);
                }
                LowInput::Char(_) => {}
                LowInput::MouseTo { x, y, coords: Coords::Screen } if self.divert => self.ui_pointer = (x, y),
                LowInput::Button { idx: 0, down: true } if self.divert => {
                    self.ui_input.push(UiInput::Click(self.ui_pointer.0, self.ui_pointer.1));
                }
                LowInput::MouseTo { .. } | LowInput::MouseRel { .. } | LowInput::Button { .. } if self.divert => {}
                LowInput::MouseTo { x, y, coords } => {
                    let (vx, vy) = match coords {
                        Coords::Virtual => (x, y),
                        Coords::Screen => screen_to_virtual_mouse(cpu, x, y),
                    };
                    cpu.bus.mouse.set_position(vx, vy);
                }
                // As a captured mouse moves: all of the motion counts, at
                // the edges of the cursor's window too.
                LowInput::MouseRel { dx, dy } => cpu.bus.mouse.move_by(dx as f64, dy as f64),
                LowInput::Button { idx, down } => {
                    if down {
                        cpu.bus.mouse.button_down(idx);
                    } else {
                        cpu.bus.mouse.button_up(idx);
                    }
                }
                LowInput::End { req } => {
                    self.append_then(cpu, Kind::InputDelivered, Some(req), json!({}), |j, seq| {
                        j.end_op(req, OpState::Done, Some(seq));
                    });
                }
                _ => unreachable!(),
            }
        }
    }

    /// A remote key press: to the settings window while it is open, else
    /// to the machine. Ctrl+F12 toggles the window, as on the keyboard.
    fn key_down(&mut self, cpu: &mut Cpu, key: PcKey, ascii: u8) {
        self.remote_mods |= key.modifier;
        if key.scan == F12_SCAN && self.remote_mods & (keys::MOD_CTRL | keys::MOD_ALT) == keys::MOD_CTRL {
            self.hotkey = true;
        } else if self.divert {
            self.ui_input.extend(ui_key(key, ascii, self.remote_mods).map(UiInput::Key));
        } else {
            keyboard::apply_key(&mut cpu.bus, key, ascii, true);
            if !self.remote_held.iter().any(|k| same_key(k, &key)) {
                self.remote_held.push(key);
            }
        }
    }

    /// A remote key release. Only keys the machine saw go down come up.
    fn key_up(&mut self, cpu: &mut Cpu, key: PcKey) {
        self.remote_mods &= !key.modifier;
        if let Some(i) = self.remote_held.iter().position(|k| same_key(k, &key)) {
            self.remote_held.remove(i);
            keyboard::apply_key(&mut cpu.bus, key, 0, false);
        }
    }

    /// Release the keys the remote client holds on the machine, as the
    /// settings window opens and takes the keyboard.
    pub fn release_keys(&mut self, cpu: &mut Cpu) {
        for key in std::mem::take(&mut self.remote_held) {
            keyboard::apply_key(&mut cpu.bus, key, 0, false);
        }
    }

    /// Remote input for the settings window since the last call.
    pub fn take_ui_input(&mut self) -> Vec<UiInput> {
        std::mem::take(&mut self.ui_input)
    }

    /// Whether a remote Ctrl+F12 came in since the last call.
    pub fn take_hotkey(&mut self) -> bool {
        std::mem::take(&mut self.hotkey)
    }

    /// Whether reboot_shell closed the program since the last call. The
    /// front end then stops its pad mapper, autoinput and game, as the
    /// settings window's close does.
    pub fn take_closed_program(&mut self) -> bool {
        std::mem::take(&mut self.closed_program)
    }

    /// The next save state a remote client asked to save or load, for the
    /// front end to carry out and answer with `state_done`. Requests that
    /// can't be carried out now are refused here (409, from `check`): a
    /// strict-match save or load while the machine isn't stopped with
    /// nothing pending (they are only done while the debugger holds the
    /// machine between two instructions), a load in deterministic mode
    /// while the machine runs (a `resume` sent in the same frame starts it
    /// before the load is carried out), and one whose `If-State-Seq` no
    /// longer holds. The others are carried out at the end of the frame,
    /// running or not.
    pub fn next_state_request(&mut self) -> Option<StateRequest> {
        while !self.state_requests.is_empty() {
            let request = self.state_requests.remove(0);
            let mut refusal = Ok(());
            if request.load {
                refusal = self.check(Need::Paused);
            }
            if request.strict_match {
                refusal = refusal.and_then(|_| {
                    self.check(Need::Quiescent { input_ok: request.load }).map_err(|mut body| {
                        let what = if request.load { "load" } else { "save" };
                        body["error"] = format!("can't do a strict-match {}: {}", what, body["error"].as_str().unwrap_or("")).into();
                        body
                    })
                });
            }
            if let Some(n) = request.if_state {
                refusal = refusal.and_then(|_| self.check(Need::State(n)));
            }
            match refusal {
                Err(body) => self.answer(&request.ctx, request.reply, Reply::ErrorJson(409, body)),
                Ok(()) => {
                    self.begin(request.ctx.clone());
                    return Some(request);
                }
            }
        }
        None
    }

    /// The front end carried out `request` from `next_state_request`:
    /// `result` is its reply, or the status and body of its failure. A
    /// save appends `state_saved`; a load `state_loaded`, and when strict,
    /// resets the debugger (`debugger_reset`); a load while paused stops
    /// the machine in the loaded state (`paused`, reason `load`). A failed
    /// load appends nothing.
    pub fn state_done(&mut self, cpu: &Cpu, request: StateRequest, result: Result<Value, (u16, Value)>) {
        let path = request.path.display().to_string();
        let reply = match result {
            Err((status, body)) => Reply::ErrorJson(status, body),
            Ok(mut value) if request.load => {
                self.told_starts = cpu.programs.started;
                self.told_ends = cpu.programs.ended;
                let reset = request.strict_match.then(|| self.reset_after_load(cpu));
                let differences = value.get("differences").cloned().unwrap_or_else(|| json!([]));
                let fields = json!({"path": path, "strict_match": request.strict_match, "source": "debugger", "differences": differences});
                self.record(cpu, Kind::StateLoaded, fields);
                if let Some((cleared, cancelled)) = reset {
                    value["debugger_reset"] = cleared.clone();
                    let ids = cancelled.clone();
                    let cause = Some(request.ctx.req);
                    self.append_then(cpu, Kind::DebuggerReset, cause, json!({"cleared": cleared, "cancelled": cancelled}), |j, seq| {
                        for req in ids {
                            j.end_op(req, OpState::Cancelled, Some(seq));
                        }
                    });
                }
                if self.paused {
                    self.pause_hit = Some(PauseReason::Load);
                    self.flush_stop(cpu);
                }
                Reply::Json(value)
            }
            Ok(value) => {
                let fields = json!({
                    "path": path,
                    "strict_match": request.strict_match,
                    "format": value.get("format"),
                    "emulator": value.get("emulator"),
                });
                self.record(cpu, Kind::StateSaved, fields);
                Reply::Json(value)
            }
        };
        let ctx = self.finish();
        self.answer(&ctx, request.reply, reply);
    }

    /// Whether deterministic mode refuses a change to the machine now: it
    /// runs, and the settings window doesn't hold it.
    fn deterministic_refuses(&self) -> bool {
        self.deterministic.is_some() && !self.paused && !self.divert
    }

    /// Forget what the debugger was told before a save state was loaded,
    /// so none of it stops the machine the state brought: breakpoints,
    /// watchpoints, the stops on exceptions, mode switches and program
    /// starts and ends, a step or the target of a step-over or run-to, and
    /// queued input. Returns what was cleared, and the requests whose
    /// operations it cancelled: the control operation under way, a `run`
    /// waiting for its program, and queued input.
    pub fn reset_after_load(&mut self, cpu: &Cpu) -> (Value, Vec<u64>) {
        let break_on: Vec<&str> = [
            (self.break_exceptions != 0, "exceptions"),
            (self.break_mode_switch, "mode_switch"),
            (self.break_program_start, "program_start"),
            (self.break_program_exit, "program_exit"),
        ]
        .into_iter()
        .filter_map(|(on, name)| on.then_some(name))
        .collect();
        let mut cancelled: Vec<u64> = self.current_op.take().into_iter().collect();
        let cleared = json!({
            "breakpoints": std::mem::take(&mut self.breakpoints).len(),
            "watchpoints": std::mem::take(&mut self.watchpoints).len(),
            "break_on": break_on,
            "run_to": self.temp_breakpoint.take().is_some(),
            "step": self.step_budget.take().is_some(),
            // The control operation that was to end with a stop.
            "pause_waits": cancelled.len(),
            "run": self.run_wait.is_some(),
            "input": self.input_pending(),
            "held_keys": self.remote_held.len(),
            // Deterministic mode: a resume's stop at an emulated time.
            "until_ms": self.stop_at_ns.take().is_some(),
        });
        self.once.clear();
        // Keys held before the load are forgotten without a release: the
        // restored keyboard has the keys of the moment it was saved, and
        // a key-up now would reach the restored machine.
        self.remote_held.clear();
        self.remote_mods = 0;
        if let Some(run) = self.run_wait.take() {
            cancelled.push(run.op);
        }
        for event in self.input.drain(..) {
            if let LowInput::End { req } = event {
                cancelled.push(req);
            }
        }
        cancelled.sort_unstable();
        cancelled.dedup();
        self.input_wait_until = None;
        self.input_wait_ns = None;
        self.break_exceptions = 0;
        self.break_mode_switch = false;
        self.break_program_start = false;
        self.break_program_exit = false;
        self.start_once = false;
        self.start_breakpoint = None;
        self.skip_bp_once = false;
        self.pause_hit = None;
        self.pause_by = None;
        self.breakpoint_hit = None;
        self.watch_hit = None;
        // The loaded machine's counts are where the debugger starts from.
        self.seen_exceptions = cpu.exceptions;
        self.seen_mode_switches = cpu.mode_switches;
        self.seen_starts = cpu.programs.started;
        self.seen_ends = cpu.programs.ended;
        (cleared, cancelled)
    }

    /// The next CPU speed a remote client asked for, for the front end to
    /// change to and answer with `speed_done`. One that can't be changed to
    /// now (deterministic mode running, or its `If-State-Seq` no longer
    /// holding) is refused here.
    pub fn next_speed_request(&mut self) -> Option<SpeedRequest> {
        while !self.speed_requests.is_empty() {
            let request = self.speed_requests.remove(0);
            let refusal = self.check(Need::Paused).and_then(|_| request.if_state.map_or(Ok(()), |n| self.check(Need::State(n))));
            match refusal {
                Err(body) => self.answer(&request.ctx, request.reply, Reply::ErrorJson(409, body)),
                Ok(()) => {
                    self.begin(request.ctx.clone());
                    return Some(request);
                }
            }
        }
        None
    }

    /// The front end carried out `request`: `result` is the speed now, or
    /// why it can't be. A change appends `speed_changed`.
    pub fn speed_done(&mut self, cpu: &Cpu, request: SpeedRequest, result: Result<Value, String>) {
        let reply = match result {
            Ok(value) => {
                self.record(cpu, Kind::SpeedChanged, json!({"cycles": value.get("cycles"), "source": "debugger"}));
                Reply::Json(value)
            }
            Err(e) => Reply::bad(e),
        };
        let ctx = self.finish();
        self.answer(&ctx, request.reply, reply);
    }

    // ----- request handling --------------------------------------------------

    fn handle(&mut self, cpu: &mut Cpu, req: Request) {
        // A stop comes before anything this request does.
        self.flush_stop(cpu);
        let Request { cmd, reply: tx, req, tag, if_state } = req;
        let ctx = Ctx { req, tag, seq: None, op: None };
        if self.quit.is_some() {
            self.answer(&ctx, tx, Reply::Error(503, "the emulator is shutting down".into()));
            return;
        }
        // In deterministic mode, a change to the machine that came while it
        // runs would land at whatever emulated time the host got to. The
        // settings window, which holds the machine, takes its input at once.
        // Save states and speeds are checked again as they are carried out.
        let need = changes_machine(&cmd).then_some(Need::Paused);
        let refusal = need.map_or(Ok(()), |n| self.check(n)).and_then(|_| if_state.map_or(Ok(()), |n| self.check(Need::State(n))));
        if let Err(body) = refusal {
            self.answer(&ctx, tx, Reply::ErrorJson(409, body));
            return;
        }
        let cmd = match cmd {
            Cmd::SaveState { path, .. } | Cmd::LoadState { path, .. } if path.is_empty() => {
                self.answer(&ctx, tx, Reply::bad("a path is needed"));
                return;
            }
            Cmd::SaveState { path, strict_match, check_files } => {
                let path = PathBuf::from(path);
                self.state_requests.push(StateRequest { load: false, path, strict_match, check_files, ctx, if_state, reply: tx });
                return;
            }
            Cmd::LoadState { path, strict_match, check_files } => {
                let path = PathBuf::from(path);
                self.state_requests.push(StateRequest { load: true, path, strict_match, check_files, ctx, if_state, reply: tx });
                return;
            }
            Cmd::Speed { cycles } => {
                self.speed_requests.push(SpeedRequest { cycles, ctx, if_state, reply: tx });
                return;
            }
            Cmd::Screenshot => {
                // Answered by the next capture_frame call.
                self.frame_waiters.push((ctx, tx));
                return;
            }
            cmd => cmd,
        };
        self.begin(ctx);
        let reply = self.dispatch(cpu, cmd);
        let ctx = self.finish();
        self.answer(&ctx, tx, reply);
    }

    /// Carry out a command the hub answers at once.
    fn dispatch(&mut self, cpu: &mut Cpu, cmd: Cmd) -> Reply {
        match cmd {
            Cmd::Status => {
                let status = self.status(cpu);
                cpu.bus.audio_peak = 0;
                Reply::Json(status)
            }
            Cmd::Stats => Reply::Json(self.stats_json(cpu)),
            Cmd::Screenshot | Cmd::SaveState { .. } | Cmd::LoadState { .. } | Cmd::Speed { .. } => {
                unreachable!("answered later, by `handle`'s arrangement")
            }
            Cmd::Quit => {
                self.quit = self.cur.clone();
                Reply::Json(json!({"ok": true, "shutting_down": true}))
            }
            Cmd::ScreenText => screen_text(cpu),
            Cmd::TraceQuery(q) => self.trace_query(cpu, q),
            Cmd::TraceControl { enabled, clear, stream_max, count } => {
                if let Some(e) = enabled {
                    self.trace_enabled = e;
                    self.trace_until = None;
                }
                // A count of 0 records nothing more: the trace stops now.
                if let Some(n) = count {
                    self.trace_enabled = n > 0;
                    self.trace_until = (n > 0).then(|| self.trace.total().saturating_add(n));
                }
                if clear {
                    self.trace.clear();
                }
                if let Some(m) = stream_max {
                    self.trace_stream_max = m.max(1);
                }
                // As the next batch will, so the reply's `active` is current.
                let streaming = self.shared.as_ref().is_some_and(|s| s.trace.receiver_count() > 0);
                self.tracing_now = self.trace_enabled || streaming;
                Reply::Json(self.trace_status())
            }
            Cmd::PortsControl { enabled, clear, capacity, ports } => {
                let log = &mut cpu.bus.port_log;
                if let Some(capacity) = capacity {
                    *log = Some(rust_dos::bus::port_log::PortLog::new(capacity.clamp(1, 10_000_000)));
                }
                match enabled {
                    Some(true) if log.is_none() => *log = Some(rust_dos::bus::port_log::PortLog::new(100_000)),
                    Some(false) => *log = None,
                    _ => {}
                }
                if let Some(log) = log {
                    if clear {
                        log.clear();
                    }
                    if let Some(ports) = ports {
                        log.ports = ports;
                    }
                }
                Reply::Json(json!({
                    "enabled": log.is_some(),
                    "entries": log.as_ref().map_or(0, |l| l.len()),
                    "capacity": log.as_ref().map_or(0, |l| l.capacity()),
                    "ports": log.as_ref().map(|l| format!("{:04X}-{:04X}", l.ports.0, l.ports.1)),
                }))
            }
            Cmd::PortsQuery { from, to, write, last_n } => match &cpu.bus.port_log {
                None => Reply::bad("the port log is off: POST /api/ports {\"enabled\":true} first"),
                Some(log) => {
                    let mut picked: Vec<Value> = log
                        .iter()
                        .rev()
                        .filter(|a| (from..=to).contains(&a.port) && write.is_none_or(|w| w == a.write))
                        .take(last_n)
                        .map(|a| {
                            json!({
                                "icount": a.icount,
                                "port": format!("{:04X}", a.port),
                                "dir": if a.write { "out" } else { "in" },
                                "value": format!("{:0width$X}", a.value, width = a.len as usize * 2),
                            })
                        })
                        .collect();
                    picked.reverse();
                    Reply::Json(json!({"count": picked.len(), "accesses": picked}))
                }
            },
            Cmd::Input { events, wait } => {
                // Pending operations are never evicted: this bounds them.
                let pending = self.with_journal(|j| j.pending_ops(|k| k == OpKind::Input).len()).unwrap_or(0);
                if pending >= journal::PENDING_INPUT_CAPACITY {
                    return Reply::Error(
                        429,
                        format!("{} input requests are still being typed: wait for some (or DELETE /api/input)", pending),
                    );
                }
                let mut low = Vec::new();
                for ev in &events {
                    if let Err(e) = expand_input(ev, cpu.bus.typing_layout(), &mut low) {
                        return Reply::bad(e);
                    }
                }
                self.input.extend(low);
                // Every input request is an operation, done once its last
                // event is delivered (`input_delivered`).
                let req = self.cur.as_ref().map_or(0, |c| c.req);
                self.input.push_back(LowInput::End { req });
                let tag = self.cur.as_ref().and_then(|c| c.tag.clone());
                let fields = json!({"events": events.len(), "wait": wait});
                self.append_then(cpu, Kind::InputQueued, Some(req), fields, |j, seq| j.begin_op(req, OpKind::Input, seq, tag));
                if let Some(ctx) = self.cur.as_mut() {
                    ctx.op = Some(req);
                }
                // Paused in deterministic mode, the input is delivered only
                // once the machine resumes: a wait for it is answered at once
                // (`held`), and its operation shows when it was delivered.
                let held = self.deterministic.is_some() && self.paused && !self.divert;
                Reply::Json(json!({"ok": true, "queued": self.input_pending(), "held": held}))
            }
            Cmd::InputClear => {
                let mut cancelled = Vec::new();
                for ev in self.input.drain(..) {
                    if let LowInput::End { req } = ev {
                        cancelled.push(req);
                    }
                }
                self.input_wait_until = None;
                self.input_wait_ns = None;
                let ids = cancelled.clone();
                let cause = self.cur.as_ref().map(|c| c.req);
                self.append_then(cpu, Kind::InputCleared, cause, json!({"cancelled": cancelled}), |j, seq| {
                    for req in ids {
                        j.end_op(req, OpState::Cancelled, Some(seq));
                    }
                });
                Reply::Json(json!({"ok": true, "cancelled": cancelled}))
            }
            Cmd::Pause => {
                if !self.paused {
                    self.enter_pause(PauseReason::Request);
                    self.pause_by = self.cur.as_ref().map(|c| c.req);
                }
                // A stop the batch made comes first; the stop is the reply.
                let stop = self.flush_stop(cpu).or_else(|| self.current_stop());
                Reply::Json(stop_reply(stop, cpu))
            }
            Cmd::Resume { until_ms: Some(_), .. } if self.deterministic.is_none() => {
                Reply::bad("until_ms needs deterministic mode (--deterministic)")
            }
            Cmd::Resume { until_ms: Some(ms), .. } if ms.saturating_mul(1_000_000) <= cpu.bus.clock.now_ns() => {
                Reply::bad(format!("until_ms {} has passed: emulated time is {} ms", ms, cpu.bus.clock.now_ns() / 1_000_000))
            }
            Cmd::Resume { until, until_ms } => {
                let target = match until.as_deref().map(|a| parse_addr(cpu, a).and_then(breakpoint_phys)) {
                    Some(Err(e)) => return Reply::bad(e),
                    Some(Ok(phys)) => Some(phys),
                    None => None,
                };
                self.take_over();
                self.temp_breakpoint = target;
                self.stop_at_ns = until_ms.map(|ms| ms.saturating_mul(1_000_000));
                self.start_op(cpu, OpKind::Resume, json!({"until": until, "until_ms": until_ms}));
                Reply::Json(json!({"ok": true, "paused": false}))
            }
            Cmd::Step { count } => {
                self.take_over();
                self.step_budget = Some(count.max(1));
                self.start_op(cpu, OpKind::Step, json!({"count": count.max(1)}));
                Reply::Json(json!({"ok": true, "paused": false}))
            }
            Cmd::StepOver => {
                let target = step_over_target(cpu);
                self.take_over();
                match target {
                    Some(next) => self.temp_breakpoint = Some(next),
                    None => self.step_budget = Some(1),
                }
                self.start_op(cpu, OpKind::StepOver, json!({"until": target.map(|p| format!("{:05X}", p))}));
                Reply::Json(json!({"ok": true, "paused": false}))
            }
            Cmd::WaitPause => match self.paused.then(|| self.current_stop()).flatten() {
                // Stopped: the stop, with the registers as they are now.
                Some(stop) => Reply::Json(stop_reply(Some(stop), cpu)),
                // The server waits for the next stop in the journal.
                None => Reply::Json(json!({"paused": false})),
            },
            Cmd::RebootShell => {
                // As the settings window's close does: the batch file and
                // commands queued go too, or their next line (often EXIT)
                // would run. The front end does the rest (take_closed_program).
                self.release_keys(cpu);
                cpu.close_program();
                self.closed_program = true;
                self.take_over();
                self.start_op(cpu, OpKind::RebootShell, json!({}));
                Reply::Json(json!({"ok": true}))
            }
            Cmd::Reboot => {
                cpu.bus.reset_requested = true;
                self.take_over();
                self.start_op(cpu, OpKind::Reboot, json!({}));
                Reply::Json(json!({"ok": true}))
            }
            Cmd::GetRegs => Reply::Json(regs_json(cpu)),
            Cmd::SetRegs(map) => match set_regs(cpu, &map) {
                Ok(()) => {
                    let changed: Map<String, Value> = map.iter().map(|(k, v)| (k.to_ascii_lowercase(), v.clone())).collect();
                    self.record(cpu, Kind::RegistersWritten, json!({"changed": changed}));
                    Reply::Json(regs_json(cpu))
                }
                Err(e) => Reply::bad(e),
            },
            Cmd::ReadMem { addr, len } => match parse_addr(cpu, &addr) {
                Ok(a) => {
                    let len = match (a.lin, a.phys) {
                        (Some(_), _) => len,
                        // The linear frame buffer, up to its end.
                        (None, Some(p)) if cpu.bus.vbe.lfb_offset(p, 1).is_some() => {
                            len.min(cpu.bus.vbe.lfb_base.unwrap_or(0) as usize + video::vbe::VRAM_SIZE - p)
                        }
                        // The 3dfx card's window, up to its end.
                        (None, Some(p)) if cpu.bus.voodoo_at(p).is_some() => {
                            len.min(rust_dos::voodoo::WINDOW as usize - cpu.bus.voodoo_at(p).unwrap_or(0) as usize)
                        }
                        // A PowerVR card's window, up to its end.
                        (None, Some(p)) if let Some((window, offset)) = cpu.bus.powervr_at(p) => {
                            let size = match window {
                                rust_dos::powervr::Window::Registers => rust_dos::powervr::REGISTER_WINDOW,
                                rust_dos::powervr::Window::Textures => rust_dos::powervr::TEXTURE_MEMORY,
                            };
                            len.min((size - offset) as usize)
                        }
                        (None, p) => len.min(cpu.bus.ram().len().saturating_sub(p.unwrap_or(0))),
                    };
                    let data = a.read(cpu, len);
                    Reply::Bytes { addr: a.lin.map_or(a.phys.unwrap_or(0), |l| l as usize), segoff: a.segoff, data }
                }
                Err(e) => Reply::bad(e),
            },
            Cmd::WriteMem { expect: Some(expect), data, .. } if expect.len() != data.len() => {
                Reply::bad(format!("expect has {} bytes, the data {}", expect.len(), data.len()))
            }
            Cmd::WriteMem { addr, data, expect } => match parse_addr(cpu, &addr) {
                Ok(a) => {
                    let targets: Option<Vec<usize>> = (0..data.len()).map(|i| a.byte(cpu, i)).collect();
                    let peek = |cpu: &Cpu, t: &[usize]| t.iter().map(|&p| cpu.bus.peek_8(p)).collect::<Vec<u8>>();
                    match targets {
                        Some(t) if t.iter().all(|&p| p < cpu.bus.ram().len() || cpu.bus.vbe.lfb_offset(p, 1).is_some()) => {
                            let old = peek(cpu, &t);
                            let guarded = expect.is_some();
                            if let Some(expect) = expect.filter(|e| *e != old) {
                                let (found, expected) = (trace::hex_bytes(&old), trace::hex_bytes(&expect));
                                Reply::ErrorJson(
                                    409,
                                    json!({
                                        "error": format!(
                                            "the memory holds {found}, not the expected {expected}; nothing was written"
                                        ),
                                        "found": found,
                                        "expected": expected,
                                    }),
                                )
                            } else {
                                for (p, b) in t.iter().zip(&data) {
                                    // The debugger patches the ROMs too.
                                    if rust_dos::bus::Bus::is_rom(*p) {
                                        cpu.bus.write_rom(*p, &[*b]);
                                    } else {
                                        cpu.bus.write_8(*p, *b);
                                    }
                                }
                                // The debugger's own changes don't stop the
                                // machine.
                                for w in &mut self.watchpoints {
                                    w.value = Watch::read(cpu, w.phys, w.len);
                                }
                                // The bytes replaced and what reads back,
                                // which can differ from the data: VGA memory
                                // reads through the read mode.
                                let new = peek(cpu, &t);
                                let addr = format!("{:05X}", a.phys.unwrap_or(0));
                                self.record(cpu, Kind::MemoryWritten, json!({
                                    "addr": addr,
                                    "len": data.len(),
                                    "old": bytes_record(&old),
                                    "new": bytes_record(&new),
                                    "expect": guarded,
                                }));
                                Reply::Json(json!({
                                    "ok": true,
                                    "addr": addr,
                                    "written": data.len(),
                                    "old": trace::hex_bytes(&old),
                                    "new": trace::hex_bytes(&new),
                                }))
                            }
                        }
                        Some(_) => Reply::bad("write extends past end of memory"),
                        None => Reply::bad("write reaches an unmapped page"),
                    }
                }
                Err(e) => Reply::bad(e),
            },
            Cmd::Disasm { addr, count } => self.disasm(cpu, addr, count),
            Cmd::ListBreakpoints => Reply::Json(self.breakpoints_json()),
            Cmd::AddBreakpoint { addr, once } => match parse_addr(cpu, &addr).and_then(breakpoint_phys) {
                Ok(phys) => {
                    self.breakpoints.insert(phys);
                    if once {
                        self.once.insert(phys);
                    } else {
                        self.once.remove(&phys);
                    }
                    self.record(cpu, Kind::BreakpointSet, json!({"addr": format!("{:05X}", phys), "once": once}));
                    Reply::Json(self.breakpoints_json())
                }
                Err(e) => Reply::bad(e),
            },
            Cmd::RemoveBreakpoint(a) => match a {
                None => {
                    self.breakpoints.clear();
                    self.once.clear();
                    self.break_exceptions = 0;
                    self.break_mode_switch = false;
                    self.break_program_start = false;
                    self.break_program_exit = false;
                    self.record(cpu, Kind::BreakpointRemoved, json!({"all": true}));
                    Reply::Json(self.breakpoints_json())
                }
                Some(a) => match parse_addr(cpu, &a).and_then(breakpoint_phys) {
                    Ok(phys) => {
                        if self.breakpoints.remove(&phys) {
                            self.once.remove(&phys);
                            self.record(cpu, Kind::BreakpointRemoved, json!({"addr": format!("{:05X}", phys)}));
                            Reply::Json(self.breakpoints_json())
                        } else {
                            Reply::Error(404, format!("no breakpoint at {:05X}", phys))
                        }
                    }
                    Err(e) => Reply::bad(e),
                },
            },
            Cmd::ListWatchpoints => Reply::Json(self.watchpoints_json(cpu)),
            Cmd::AddWatchpoint { addr, len } => {
                if !matches!(len, 1 | 2 | 4) {
                    return Reply::bad("len is 1, 2 or 4");
                }
                match parse_addr(cpu, &addr).and_then(breakpoint_phys) {
                    Ok(phys) => {
                        self.watchpoints.retain(|w| w.phys != phys);
                        self.watchpoints.push(Watch { phys, len, value: Watch::read(cpu, phys, len) });
                        self.record(cpu, Kind::WatchpointSet, json!({"addr": format!("{:05X}", phys), "len": len}));
                        Reply::Json(self.watchpoints_json(cpu))
                    }
                    Err(e) => Reply::bad(e),
                }
            }
            Cmd::RemoveWatchpoint(addr) => match addr {
                None => {
                    self.watchpoints.clear();
                    self.record(cpu, Kind::WatchpointRemoved, json!({"all": true}));
                    Reply::Json(self.watchpoints_json(cpu))
                }
                Some(a) => match parse_addr(cpu, &a).and_then(breakpoint_phys) {
                    Ok(phys) => {
                        let before = self.watchpoints.len();
                        self.watchpoints.retain(|w| w.phys != phys);
                        if self.watchpoints.len() < before {
                            self.record(cpu, Kind::WatchpointRemoved, json!({"addr": format!("{:05X}", phys)}));
                            Reply::Json(self.watchpoints_json(cpu))
                        } else {
                            Reply::Error(404, format!("no watchpoint at {:05X}", phys))
                        }
                    }
                    Err(e) => Reply::bad(e),
                },
            },
            Cmd::BreakOn { exceptions, clear_exceptions, mode_switch, program_start, program_exit } => {
                // The counts go stale while nothing watches them. While
                // something does, a program that started or ended in the
                // last instruction of the batch is still to be seen.
                if !self.break_program_start && !self.start_once {
                    self.seen_starts = cpu.programs.started;
                }
                if !self.break_program_exit {
                    self.seen_ends = cpu.programs.ended;
                }
                if let Some(mask) = exceptions {
                    self.break_exceptions |= mask;
                }
                if let Some(mask) = clear_exceptions {
                    self.break_exceptions &= !mask;
                }
                if let Some(m) = mode_switch {
                    self.break_mode_switch = m;
                }
                if let Some(b) = program_start {
                    self.break_program_start = b;
                }
                if let Some(b) = program_exit {
                    self.break_program_exit = b;
                }
                self.seen_exceptions = cpu.exceptions;
                self.seen_mode_switches = cpu.mode_switches;
                let list = self.breakpoints_json();
                let fields = json!({
                    "exceptions": list["exceptions"],
                    "mode_switch": self.break_mode_switch,
                    "program_start": self.break_program_start,
                    "program_exit": self.break_program_exit,
                });
                self.record(cpu, Kind::BreakOn, fields);
                Reply::Json(list)
            }
            Cmd::Run { command, stop_at_entry } => {
                if self.run_wait.is_some() {
                    Reply::Error(409, "a run command is already waiting for its program".into())
                } else if !at_idle_prompt(cpu) {
                    Reply::Error(409, "the DOS prompt is busy with a program, a batch file, PAUSE, CHOICE or EDIT".into())
                } else if command.trim().is_empty() || command.contains(['\r', '\n']) {
                    Reply::bad("command: one command line")
                } else {
                    cpu.queue_batch_lines([command.trim()]);
                    self.take_over();
                    self.seen_starts = cpu.programs.started;
                    self.start_once = stop_at_entry;
                    let op = self.cur.as_ref().map_or(0, |c| c.req);
                    self.run_wait = Some(RunWait { before: cpu.programs.started, stop_at_entry, op });
                    self.start_op(cpu, OpKind::Run, json!({"command": command.trim(), "stop_at_entry": stop_at_entry}));
                    Reply::Json(json!({"ok": true}))
                }
            }
            Cmd::Ivt => Reply::Json(ivt_json(cpu)),
            Cmd::Gdt => Reply::Json(pm::table_json(cpu, false, 1024)),
            Cmd::Ldt => Reply::Json(pm::table_json(cpu, true, 1024)),
            Cmd::Idt => Reply::Json(pm::idt_json(cpu)),
            Cmd::Tss => Reply::Json(pm::tss_json(cpu)),
            Cmd::PageWalk(addr) => match parse_addr(cpu, &addr) {
                Ok(a) => Reply::Json(pm::pagewalk_json(cpu, a.lin.unwrap_or(a.phys.unwrap_or(0) as u32))),
                Err(e) => Reply::bad(e),
            },
            Cmd::Xms => Reply::Json(pm::xms_json(cpu)),
            Cmd::Net => Reply::Json(net_json(cpu)),
            Cmd::Serial => Reply::Json(serial_json(cpu)),
            Cmd::Printer { eject } => Reply::Json(printer_json(cpu, eject)),
            Cmd::Gus => Reply::Json(match &cpu.bus.gus {
                Some(gus) => gus.snapshot(),
                None => serde_json::json!({ "installed": false }),
            }),
            Cmd::Awe32 => Reply::Json(match &cpu.bus.awe {
                Some(awe) => awe.snapshot(),
                None => serde_json::json!({ "installed": false }),
            }),
            Cmd::Exceptions => Reply::Json(pm::exceptions_json(cpu)),
            Cmd::Drives => Reply::Json(drives_json(cpu)),
            Cmd::SyncShared => {
                let messages = cpu.bus.sync_shared(None);
                self.record(cpu, Kind::SharedSynced, json!({"messages": messages}));
                Reply::Json(json!({"messages": messages}))
            }
            Cmd::SwapImages => {
                let messages = cpu.bus.swap_images();
                for message in &messages {
                    cpu.bus.log_string(&format!("[DEBUG] {}", message));
                }
                self.record(cpu, Kind::ImagesSwapped, json!({"messages": messages}));
                Reply::Json(json!({"messages": messages, "drives": drives_json(cpu)}))
            }
            Cmd::Mount { drive, path, kind, label, read_only, images } => {
                match mount_drive(cpu, &drive, &path, kind.as_deref(), label, read_only, images) {
                    Ok(root) => {
                        cpu.bus.log_string(&format!(
                            "[DEBUG] Drive {} mounted to {}",
                            drive,
                            root.display()
                        ));
                        let drives = drives_json(cpu);
                        let key = parse_drive_name(&drive).map(crate::disk::drive_key);
                        let mounted = key.as_ref().and_then(|k| drives["drives"].get(k).cloned()).unwrap_or(Value::Null);
                        self.record(cpu, Kind::DriveMounted, json!({"drive": key, "mounted": mounted}));
                        Reply::Json(drives)
                    }
                    Err(e) => Reply::bad(e),
                }
            }
            Cmd::Unmount { drive } => {
                let parsed = parse_drive_name(&drive);
                let result = parsed
                    .ok_or_else(|| format!("invalid drive letter '{}'", drive))
                    .and_then(|d| cpu.bus.unmount_drive(d));
                match result {
                    Ok(()) => {
                        self.record(cpu, Kind::DriveUnmounted, json!({"drive": parsed.map(crate::disk::drive_key)}));
                        Reply::Json(drives_json(cpu))
                    }
                    Err(e) => Reply::bad(e),
                }
            }
        }
    }

    fn status(&self, cpu: &Cpu) -> Value {
        let (w, h) = cpu.bus.display_size();
        let (frame_w, frame_h) = video::frame_size(&cpu.bus);
        let crtc = &cpu.bus.vga.crtc_regs;
        let timing = cpu.bus.vga.peek_timing();
        let mut status = json!({
            "paused": self.paused,
            "settings_window": self.divert,
            "icount": cpu.executed,
            "uptime_ms": cpu.bus.start_time.elapsed().as_millis() as u64,
            "fps": (self.fps * 10.0).round() / 10.0,
            "cycles_per_ms": cpu.bus.clock.cycles_per_ms(),
            // Deterministic mode: the clock's start, the machine's clock
            // now, emulated time, and every how many emulated ms input is
            // delivered. Null when off.
            "deterministic": self.deterministic.map(|start| json!({
                "start_time": start.format("%Y-%m-%d %H:%M:%S").to_string(),
                "clock": cpu.bus.cmos.now().format("%Y-%m-%d %H:%M:%S").to_string(),
                "emulated_ms": cpu.bus.clock.now_ns() / 1_000_000,
                "input_tick_ms": rust_dos::deterministic::TICK_MS,
                "until_ms": self.stop_at_ns.map(|ns| ns / 1_000_000),
            })),
            // What `cycles=auto` goes by, counted since start: emulated
            // time, the frames the program drew (at most one a retrace),
            // its bursts of writes to video memory the size of a frame and
            // its page flips (the frames it draws, however many), its
            // checks for keys that weren't there and idle calls, and its
            // reads of the input status register (retrace polling).
            "activity": {
                "emulated_ns": cpu.bus.clock.now_ns(),
                "frames_drawn": cpu.bus.frames_drawn,
                "bursts": cpu.bus.activity.bursts,
                "flips": cpu.bus.activity.flips,
                "polls": cpu.bus.activity.polls,
                "status_reads": cpu.bus.activity.status_reads,
            },
            "cs_ip": cs_ip(cpu),
            "cpu_mode": pm::mode_name(cpu),
            "cpu_state": format!("{:?}", cpu.state),
            "shell_idle": cpu.shell_idle(),
            "process_depth": cpu.process_stack.len(),
            "current_psp": format!("{:04X}", cpu.current_psp),
            "video": {
                // The display adapter programs see (`machine`).
                "adapter": cpu.bus.vga.adapter.name(),
                "mode": cpu.bus.video_mode as u8,
                "name": format!("{:?}", cpu.bus.video_mode),
                "width": w, "height": h,
                // The picture as screenshots have it.
                "frame": {"width": frame_w, "height": frame_h},
                // Page flipping: the Start Address the program set and the
                // one latched for display, plus the registers that decide
                // the memory layout.
                "vga": {
                    "start_address": format!("{:02X}{:02X}", crtc[0x0C], crtc[0x0D]),
                    "displayed_start": format!("{:04X}", cpu.bus.vga.latched_start_addr),
                    "seq_memory_mode": format!("{:02X}", cpu.bus.vga.sequencer_regs[0x04]),
                    "crtc_offset": format!("{:02X}", crtc[0x13]),
                    "crtc_underline": format!("{:02X}", crtc[0x14]),
                    "crtc_mode_control": format!("{:02X}", crtc[0x17]),
                    "crtc": crtc.iter().map(|r| format!("{:02X}", r)).collect::<Vec<_>>().join(" "),
                    "attribute": cpu.bus.vga.attribute_regs.iter().map(|r| format!("{:02X}", r)).collect::<Vec<_>>().join(" "),
                    // The DAC's first 64 colours, six bits a component.
                    "dac": cpu.bus.vga.palette[..192].chunks(3).map(|c| format!("{:02X}{:02X}{:02X}", c[0], c[1], c[2])).collect::<Vec<_>>().join(" "),
                },
                // The VESA mode, when one is set.
                "vbe": cpu.bus.vbe.mode.filter(|_| cpu.bus.video_mode == VideoMode::Vesa).map(|mode| json!({
                    "mode": format!("{:03X}", mode.number),
                    "width": mode.width, "height": mode.height, "bpp": mode.bpp,
                    "lfb": cpu.bus.vbe.lfb,
                    "bank": cpu.bus.vbe.bank,
                    "pitch": cpu.bus.vbe.pitch,
                    "display_start": format!("{:X}", cpu.bus.vbe.latched_start),
                    "dac_bits": if cpu.bus.vga.dac_8bit { 8 } else { 6 },
                })),
                // An S3's extended CRTC registers (30h-6Fh, a row of 16
                // each) and its hardware cursor.
                "voodoo": cpu.bus.voodoo_status(),
                "powervr": cpu.bus.powervr_status(),
                "verite": cpu.bus.verite().then(|| cpu.bus.verite.describe()),
                "s3": cpu.bus.vga.adapter.is_s3().then(|| {
                    let s3 = &cpu.bus.vga.s3;
                    json!({
                        "crtc": (0x30..0x70usize).step_by(16).map(|row| {
                            (row..row + 16).map(|i| format!("{:02X}", s3.crtc(i as u8))).collect::<Vec<_>>().join(" ")
                        }).collect::<Vec<_>>(),
                        "cursor": s3.cursor().map(|c| json!({
                            "x": c.x, "y": c.y, "skip_x": c.skip_x, "skip_y": c.skip_y,
                            "address": format!("{:X}", c.address), "x11": c.x11,
                        })),
                        // A ViRGE's engines: the commands they ran, and the
                        // last 3D command.
                        // A ViRGE's engines.
                        "virge": cpu.bus.vga.adapter.is_virge().then(|| virge_status(&cpu.bus.virge, cpu.bus.vga.adapter == crate::video::adapter::Adapter::S3VirgeVx)),
                    })
                }),
                // An ET4000's extended registers: CR30-3F, the KEY, the
                // segment select (write bank low, read bank high) and the
                // Sierra DAC's command register.
                "et4000": cpu.bus.vga.adapter.is_et4000().then(|| {
                    let chip = &cpu.bus.vga.et4000;
                    json!({
                        "crtc": (0x30..0x40u8).map(|i| format!("{:02X}", chip.crtc(i))).collect::<Vec<_>>().join(" "),
                        "keyed": chip.keyed,
                        "segment": format!("{:02X}", chip.segment),
                        "dac_command": format!("{:02X}", chip.dac_command),
                        "hicolor": chip.hicolor(),
                        "vbe_mode": (chip.vbe_mode != 0).then(|| format!("{:03X}", chip.vbe_mode)),
                    })
                }),
                // The display timing programs see through port 3DAh.
                "crt": {
                    "hz": (timing.hz() * 100.0).round() / 100.0,
                    "lines": timing.total,
                    "display_lines": timing.display,
                },
            },
            "drive_c": cpu
                .bus
                .disk
                .drive_info(crate::disk::DRIVE_C)
                .and_then(|info| info.root.or(info.image))
                .map(|path| display_host_path(&path)),
            "current_drive": drive_letter(cpu.bus.disk.get_current_drive()).to_string(),
            "trace": self.trace_status(),
            "breakpoints": self.breakpoints.len(),
            "watchpoints": self.watchpoints.len(),
            "input_queue": self.input_pending(),
            "keyboard_buffer": cpu.bus.keyboard_buffer.len(),
            // The interrupt controllers: requests waiting, masked lines and
            // lines in service (bit n = line n of that chip).
            "pic": {
                "vectors": [format!("{:02X}", cpu.bus.pic.master.base), format!("{:02X}", cpu.bus.pic.slave.base)],
                "irr": [format!("{:02X}", cpu.bus.pic.master.irr), format!("{:02X}", cpu.bus.pic.slave.irr)],
                "imr": [format!("{:02X}", cpu.bus.pic.master.imr), format!("{:02X}", cpu.bus.pic.slave.imr)],
                "isr": [format!("{:02X}", cpu.bus.pic.master.isr), format!("{:02X}", cpu.bus.pic.slave.isr)],
            },
            "audio": {
                "peak": cpu.bus.audio_peak,
                "underruns": cpu.bus.audio_underruns,
                "frames": cpu.bus.audio_frames(),
                "queued_frames": cpu.bus.audio_device.as_ref().map_or(0, |d| d.queued_frames()),
                "sound_blaster": cpu.bus.sb.as_ref().map(|sb| sb.config.blaster()),
                "opl3": cpu.bus.opl.is_opl3(),
                "ultrasound": cpu.bus.gus.as_ref().map(|gus| gus.config.ultrasnd()),
                "midi_synth": cpu.bus.mpu.synth_name(),
                "midi_voices": cpu.bus.mpu.gus_synth().map(|s| s.active_voices()),
                "missing_patches": cpu.bus.mpu.gus_synth().map(|s| s.missing_patches()),
                // CD audio from an image drive (MSCDEX Play Audio).
                "cd": cpu.bus.cdaudio.drive().map(|drive| {
                    let (m, s, f) = rust_dos::cdrom::lba_to_msf(cpu.bus.cdaudio.position());
                    json!({
                        "drive": drive_letter(drive).to_string(),
                        "state": format!("{:?}", cpu.bus.cdaudio.state()).to_ascii_lowercase(),
                        "track": cpu.bus.cdaudio.track(),
                        "position": format!("{:02}:{:02}:{:02}", m, s, f),
                    })
                }),
            },
            // The keyboard controller: its output buffer and queues.
            "kbc": cpu.bus.kbc.debug_json(),
            "mouse": {
                "installed": cpu.bus.mouse.installed,
                "x": cpu.bus.mouse.x, "y": cpu.bus.mouse.y,
                "buttons": cpu.bus.mouse.buttons,
                "visible": cpu.bus.mouse.hide_counter <= 0,
                "event_handler": format!(
                    "{:04X}:{:04X} mask {:04X}",
                    cpu.bus.mouse.callback_cs, cpu.bus.mouse.callback_ip, cpu.bus.mouse.callback_mask
                ),
                "ps2": {
                    "enabled": cpu.bus.mouse.ps2.enabled,
                    "handler": format!("{:04X}:{:04X}", cpu.bus.mouse.ps2.handler.0, cpu.bus.mouse.ps2.handler.1),
                    "rate": cpu.bus.mouse.ps2.rate,
                },
            },
        });
        // The program running (empty at the prompt), and how the last one
        // to end did.
        status["program"] = cpu.program.clone().into();
        status["last_exit"] = exit_json(cpu);
        // The journal: its last and oldest entries, the last that changed
        // the machine, the stop it is in, the control operation under way,
        // and whether a strict-match save could be done now.
        if let Some(shared) = &self.shared {
            let (last, first, state_seq) = self.with_journal(|j| (j.last(), j.first(), j.state_seq)).unwrap_or_default();
            status["seq"] = last.into();
            status["first_seq"] = first.into();
            status["state_seq"] = state_seq.into();
            status["stop"] = self.current_stop().unwrap_or(Value::Null);
            status["op"] = self.current_op.into();
            status["quiescent"] = self.check(Need::Quiescent { input_ok: false }).is_ok().into();
            status["instance"] = shared.instance_hex().into();
        }
        status
    }

    fn trace_status(&self) -> Value {
        json!({
            "enabled": self.trace_enabled,
            "active": self.tracing_now,
            "entries": self.trace.len(),
            "capacity": self.trace.capacity(),
            "total_recorded": self.trace.total(),
            // The instructions a `count` still has to record.
            "remaining": self.trace_until.map(|n| n.saturating_sub(self.trace.total())),
            "stream_max_per_frame": self.trace_stream_max,
        })
    }

    fn trace_query(&self, cpu: &Cpu, q: TraceQuery) -> Reply {
        let cs_filter = match q.cs.as_deref().map(parse_hex) {
            Some(Ok(v)) => Some(v as u16),
            Some(Err(e)) => return Reply::bad(e),
            None => None,
        };
        let now_us = cpu.bus.start_time.elapsed().as_micros() as u64;
        let (from_us, to_us) = if let Some(last) = q.last_ms {
            (now_us.saturating_sub((last * 1000.0) as u64), u64::MAX)
        } else {
            (
                q.from_ms.map_or(0, |v| (v * 1000.0) as u64),
                q.to_ms.map_or(u64::MAX, |v| (v * 1000.0) as u64),
            )
        };
        let limit = q.limit.unwrap_or(10_000);
        let limit = q.last_n.map_or(limit, |n| n.min(limit));
        let matches = |e: &TraceEntry| {
            e.t_us >= from_us
                && e.t_us <= to_us
                && cs_filter.is_none_or(|cs| e.cs == cs)
                && !(q.no_bios && e.cs >= 0xF000 && !(e.bytes[0] == 0xFE && e.bytes[1] == 0x38))
        };
        if let Some(since) = q.since {
            // A cursor past the end (from an earlier run of the emulator)
            // reads from the end, so new entries are not skipped until the
            // total catches up with it.
            let since = since.min(self.trace.total());
            // Oldest first, so the next page goes on where this one ends.
            let (after, dropped) = self.trace.after(since);
            let mut next = since.max(self.trace.total() - self.trace.len() as u64);
            let mut entries = Vec::new();
            for (n, e) in after {
                if entries.len() == limit {
                    break;
                }
                next = n;
                if matches(e) {
                    entries.push(*e);
                }
            }
            return Reply::Trace { entries, next, dropped };
        }
        // Newest-first collection so `limit` keeps the most recent entries.
        let mut entries: Vec<TraceEntry> = self.trace.iter().rev().filter(|e| matches(e)).take(limit).copied().collect();
        entries.reverse();
        Reply::Trace { entries, next: self.trace.total(), dropped: 0 }
    }

    fn disasm(&self, cpu: &Cpu, addr: Option<String>, count: usize) -> Reply {
        let addr = addr.unwrap_or_else(|| "CS:EIP".to_string());
        let start = match parse_addr(cpu, &addr) {
            Ok(a) => a,
            Err(e) => return Reply::bad(e),
        };
        let code32 = start.segoff.is_some_and(|(sel, _)| pm::code32(cpu, sel));
        let cur = cpu.peek_translate(cpu.seg_cache(crate::cpu::Seg::CS).base.wrapping_add(cpu.eip())).map(|p| p as usize);
        let mut lines = Vec::new();
        let mut rows = Vec::new();
        let mut at = 0usize;
        for _ in 0..count.min(1000) {
            let p = start.byte(cpu, at);
            let bytes: Vec<u8> = (0..15)
                .map(|i| start.byte(cpu, at + i).map_or(0xFF, |a| cpu.bus.peek_8(a)))
                .collect();
            let (len, text) = if bytes[0] == 0xFE && bytes[1] == 0x38 {
                (3, format!("HLE INT {:02X}h", bytes[2]))
            } else if bytes[0] == 0xFE && bytes[1] == 0x39 {
                (3, format!("HLE service {:02X}h", bytes[2]))
            } else {
                let ip = start.segoff.map_or(at as u32, |(_, off)| off.wrapping_add(at as u32));
                trace::disasm_one(&bytes, ip, code32)
            };
            let current = p.is_some() && p == cur;
            let breakpoint = p.is_some_and(|p| self.breakpoints.contains(&p));
            let marker = match (current, breakpoint) {
                (true, true) => "=>*",
                (true, false) => "=> ",
                (false, true) => "  *",
                _ => "   ",
            };
            let label = match start.segoff {
                Some((sel, off)) if code32 || off > 0xFFFF => format!("{:04X}:{:08X}", sel, off.wrapping_add(at as u32)),
                Some((sel, off)) => format!("{:04X}:{:04X}", sel, (off as usize + at) as u16),
                None => format!("{:08X}", start.lin.map_or(start.phys.unwrap_or(0) + at, |l| l as usize + at)),
            };
            let hex = trace::hex_bytes(&bytes[..len]);
            lines.push(format!("{} {}  {:<20} {}", marker, label, hex, text));
            rows.push(json!({
                "label": label,
                "phys": p.map(|p| format!("{:05X}", p)),
                "bytes": hex,
                "asm": text,
                "len": len,
                "current": current,
                "breakpoint": breakpoint,
            }));
            at += len;
        }
        Reply::Json(json!({"lines": lines, "rows": rows}))
    }

    fn watchpoints_json(&self, cpu: &Cpu) -> Value {
        let watches: Vec<Value> = self
            .watchpoints
            .iter()
            .map(|w| {
                json!({
                    "addr": format!("{:05X}", w.phys),
                    "len": w.len,
                    "value": format!("{:0width$X}", Watch::read(cpu, w.phys, w.len), width = w.len as usize * 2),
                })
            })
            .collect();
        json!({"watchpoints": watches})
    }

    fn breakpoints_json(&self) -> Value {
        let mut v: Vec<_> = self.breakpoints.iter().copied().collect();
        v.sort();
        let mut once: Vec<_> = self.once.iter().copied().collect();
        once.sort();
        let exceptions: Vec<String> =
            (0..32).filter(|i| self.break_exceptions & (1 << i) != 0).map(|i| format!("{:02X}", i)).collect();
        json!({
            "breakpoints": v.iter().map(|p| format!("{:05X}", p)).collect::<Vec<_>>(),
            // The breakpoints among them that go once they stop the machine.
            "once": once.iter().map(|p| format!("{:05X}", p)).collect::<Vec<_>>(),
            "exceptions": exceptions,
            "mode_switch": self.break_mode_switch,
            "program_start": self.break_program_start,
            "program_exit": self.break_program_exit,
        })
    }
}

/// The fields of a stop as replies have them since before the journal
/// (`paused`, `reason`, `icount`, `registers` and the rest), and the whole
/// `paused` entry as `stop`.
pub(crate) fn stop_fields(stop: &Value) -> Value {
    let mut reply = json!({"paused": true});
    if let Value::Object(entry) = stop {
        for (k, v) in entry {
            if !matches!(k.as_str(), "seq" | "type" | "t_ms" | "emu_ns" | "cause" | "request_id" | "op") {
                reply[k] = v.clone();
            }
        }
    }
    reply["stop"] = stop.clone();
    reply
}

/// The reply to a pause or a wait: `stop_fields`, with the instruction
/// count and the registers as they are now, which a write after the stop
/// may have changed.
fn stop_reply(stop: Option<Value>, cpu: &Cpu) -> Value {
    let mut reply = stop_fields(&stop.unwrap_or(Value::Null));
    reply["icount"] = cpu.executed.into();
    reply["registers"] = regs_json(cpu);
    reply
}

/// Bytes as a `memory_written` entry keeps them: hex up to 256 bytes, else
/// their SHA-256.
fn bytes_record(bytes: &[u8]) -> Value {
    use sha2::Digest;
    if bytes.len() <= 256 {
        trace::hex_bytes(bytes).into()
    } else {
        let digest = sha2::Sha256::digest(bytes);
        json!({"sha256": digest.iter().map(|b| format!("{:02x}", b)).collect::<String>()})
    }
}

/// Commands that change the machine (its input, memory, registers, drives,
/// speed or state), which deterministic mode takes only while it is paused.
fn changes_machine(cmd: &Cmd) -> bool {
    matches!(
        cmd,
        Cmd::Input { .. }
            | Cmd::Run { .. }
            | Cmd::WriteMem { .. }
            | Cmd::SetRegs(_)
            | Cmd::Mount { .. }
            | Cmd::Unmount { .. }
            | Cmd::SwapImages
            | Cmd::LoadState { .. }
            | Cmd::Speed { .. }
            | Cmd::Reboot
            | Cmd::RebootShell
    )
}

/// CS:IP, or CS:EIP past FFFFh.
fn cs_ip(cpu: &Cpu) -> String {
    if cpu.eip() > 0xFFFF {
        format!("{:04X}:{:08X}", cpu.cs(), cpu.eip())
    } else {
        format!("{:04X}:{:04X}", cpu.cs(), cpu.ip())
    }
}

/// Whether the shell waits at its prompt with nothing to run: no program,
/// no batch line or command queued, no PAUSE, CHOICE or EDIT open.
fn at_idle_prompt(cpu: &Cpu) -> bool {
    cpu.shell_idle() && !cpu.batch.is_active() && cpu.pending_command.is_none() && cpu.shell_wait.is_none()
}

/// The program started last: its name and entry point.
fn program_json(cpu: &Cpu) -> Value {
    let p = &cpu.programs;
    json!({
        "name": p.name,
        "entry": format!("{:04X}:{:04X}", p.entry_cs_ip.0, p.entry_cs_ip.1),
        "psp": format!("{:04X}", p.psp),
    })
}

/// How the program that ended last did, or null before any did.
fn exit_json(cpu: &Cpu) -> Value {
    match &cpu.programs.last_exit {
        Some(e) => json!({"name": e.name, "code": e.code, "resident": e.resident, "aborted": e.code.is_none()}),
        None => Value::Null,
    }
}

fn same_key(a: &PcKey, b: &PcKey) -> bool {
    (a.scan, a.extended) == (b.scan, b.extended)
}

/// What a remote key does in the settings window: `ascii` is the character
/// it types under the modifiers `mods`.
fn ui_key(key: PcKey, ascii: u8, mods: u8) -> Option<UiKey> {
    use UiKey::*;
    let shift = mods & (keys::MOD_LSHIFT | keys::MOD_RSHIFT) != 0;
    let ctrl = mods & keys::MOD_CTRL != 0;
    Some(match key.scan {
        _ if key.modifier != 0 => return None,
        0x48 => Up,
        0x50 => Down,
        0x4B => Left,
        0x4D => Right,
        0x49 => PageUp,
        0x51 => PageDown,
        0x47 => Home,
        0x4F => End,
        0x1C => Enter,
        0x01 => Esc,
        0x0F if shift => BackTab,
        0x0F => Tab,
        0x0E => Backspace,
        0x53 => Delete,
        0x52 => Insert,
        0x3B if !ctrl => Help,
        0x3C => Save,
        0x1F if ctrl => Save,
        _ if (0x20..0x7F).contains(&ascii) => Char(ascii as char),
        _ => return None,
    })
}

/// Map screenshot pixels (the picture's own size) into the mouse driver's
/// virtual coordinate system (same convention as the SDL path in main.rs).
fn screen_to_virtual_mouse(cpu: &Cpu, x: i32, y: i32) -> (i32, i32) {
    let (w, h) = video::frame_size(&cpu.bus);
    let (w, h) = (w as i32, h as i32);
    let px = x.clamp(0, w - 1);
    let py = y.clamp(0, h - 1);
    let (virt_w, virt_h) = cpu.bus.mouse.virtual_screen(&cpu.bus);
    let vx = (px as i64 * virt_w as i64 / w as i64) as i32;
    let vy = (py as i64 * virt_h as i64 / h as i64) as i32;
    (vx.clamp(0, virt_w - 1), vy.clamp(0, virt_h - 1))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse a hex number with optional `0x` prefix / `h` suffix.
/// Where stepping over the instruction at CS:EIP stops: the physical
/// address of the next instruction, when the instruction is one that runs
/// code or repeats before getting there (a call, an interrupt, a loop, a
/// repeated string instruction). None for the others, which a single step
/// does.
fn step_over_target(cpu: &Cpu) -> Option<usize> {
    let cs = cpu.seg_cache(crate::cpu::Seg::CS);
    let code32 = cs.attr & crate::cpu::ATTR_DB != 0;
    let eip = cpu.eip();
    let bytes: Vec<u8> = (0..15u32)
        .map(|i| cpu.peek_translate(cs.base.wrapping_add(eip.wrapping_add(i))).map_or(0xFF, |p| cpu.bus.peek_8(p as usize)))
        .collect();
    let len = step_over_len(&bytes, code32)?;
    let next = eip.wrapping_add(len as u32);
    let next = if code32 { next } else { next & 0xFFFF };
    cpu.peek_translate(cs.base.wrapping_add(next)).map(|p| p as usize)
}

/// The length of the instruction in `bytes` if stepping over it runs to
/// the instruction after it, else None.
fn step_over_len(bytes: &[u8], code32: bool) -> Option<usize> {
    use iced_x86::{Decoder, DecoderOptions, FlowControl, Mnemonic};
    // The emulator's own interrupt handlers run in one step.
    if bytes.len() >= 2 && bytes[0] == 0xFE && matches!(bytes[1], 0x38 | 0x39) {
        return None;
    }
    let mut decoder = Decoder::new(if code32 { 32 } else { 16 }, bytes, DecoderOptions::NONE);
    let instr = decoder.decode();
    if instr.is_invalid() {
        return None;
    }
    let over = matches!(instr.flow_control(), FlowControl::Call | FlowControl::IndirectCall | FlowControl::Interrupt)
        || matches!(instr.mnemonic(), Mnemonic::Loop | Mnemonic::Loope | Mnemonic::Loopne)
        || (instr.is_string_instruction() && (instr.has_rep_prefix() || instr.has_repe_prefix() || instr.has_repne_prefix()));
    over.then_some(instr.len())
}

/// Where a breakpoint at an address goes: its physical address.
fn breakpoint_phys(a: pm::DebugAddr) -> Result<usize, String> {
    a.phys.ok_or_else(|| "address is on an unmapped page".to_string())
}

/// 16-bit registers and segment registers `set_regs` accepts.
fn reg16(name: &str) -> bool {
    matches!(name, "ax" | "bx" | "cx" | "dx" | "si" | "di" | "bp" | "sp" | "cs" | "ds" | "es" | "ss" | "ip")
}

pub fn regs_json(cpu: &Cpu) -> Value {
    let flags = cpu.get_cpu_flags();
    let names: Vec<&str> = [
        (CpuFlags::OF, "OF"),
        (CpuFlags::DF, "DF"),
        (CpuFlags::IF, "IF"),
        (CpuFlags::TF, "TF"),
        (CpuFlags::SF, "SF"),
        (CpuFlags::ZF, "ZF"),
        (CpuFlags::AF, "AF"),
        (CpuFlags::PF, "PF"),
        (CpuFlags::CF, "CF"),
    ]
    .iter()
    .filter(|(f, _)| flags.contains(*f))
    .map(|(_, n)| *n)
    .collect();
    let h = |v: u16| format!("{:04X}", v);
    let h32 = |v: u32| format!("{:08X}", v);
    json!({
        "ax": h(cpu.ax()), "bx": h(cpu.bx()), "cx": h(cpu.cx()), "dx": h(cpu.dx()),
        "si": h(cpu.si()), "di": h(cpu.di()), "bp": h(cpu.bp()), "sp": h(cpu.sp()),
        "eax": h32(cpu.eax()), "ebx": h32(cpu.ebx()), "ecx": h32(cpu.ecx()), "edx": h32(cpu.edx()),
        "esi": h32(cpu.esi()), "edi": h32(cpu.edi()), "ebp": h32(cpu.ebp()), "esp": h32(cpu.esp()),
        "cs": h(cpu.cs()), "ds": h(cpu.ds()), "es": h(cpu.es()), "ss": h(cpu.ss()),
        "fs": h(cpu.fs()), "gs": h(cpu.gs()),
        "ip": h(cpu.ip()), "eip": h32(cpu.eip()),
        "flags": h(flags.bits() as u16), "eflags": h32(flags.bits()),
        "flags_set": names,
        "cr0": h32(cpu.cr0),
        "system": pm::system_regs(cpu),
    })
}

fn set_regs(cpu: &mut Cpu, map: &Map<String, Value>) -> Result<(), String> {
    // Validate everything first so a bad entry doesn't leave a partial update.
    let mut updates = Vec::new();
    for (k, v) in map {
        let val = match v {
            Value::Number(n) => n.as_u64().ok_or_else(|| format!("{}: not an unsigned integer", k))? as u32,
            Value::String(s) => parse_hex(s)?,
            _ => return Err(format!("{}: value must be a number or hex string", k)),
        };
        let key = k.to_ascii_lowercase();
        let wide = matches!(
            key.as_str(),
            "eax" | "ebx" | "ecx" | "edx" | "esi" | "edi" | "ebp" | "esp" | "eip" | "eflags"
        );
        if !wide && !matches!(key.as_str(), "flags" | "fs" | "gs") && !reg16(&key) {
            return Err(format!("unknown register '{}'", k));
        }
        if !wide && val > 0xFFFF {
            return Err(format!("{}: value exceeds 16 bits", k));
        }
        updates.push((key, val));
    }
    for (k, v) in updates {
        match k.as_str() {
            "ax" => cpu.set_ax(v as u16),
            "bx" => cpu.set_bx(v as u16),
            "cx" => cpu.set_cx(v as u16),
            "dx" => cpu.set_dx(v as u16),
            "si" => cpu.set_si(v as u16),
            "di" => cpu.set_di(v as u16),
            "bp" => cpu.set_bp(v as u16),
            "sp" => cpu.set_sp(v as u16),
            "eax" => cpu.set_eax(v),
            "ebx" => cpu.set_ebx(v),
            "ecx" => cpu.set_ecx(v),
            "edx" => cpu.set_edx(v),
            "esi" => cpu.set_esi(v),
            "edi" => cpu.set_edi(v),
            "ebp" => cpu.set_ebp(v),
            "esp" => cpu.set_esp(v),
            "cs" => cpu.set_cs(v as u16),
            "ds" => cpu.set_ds(v as u16),
            "es" => cpu.set_es(v as u16),
            "ss" => cpu.set_ss(v as u16),
            "fs" => cpu.set_fs(v as u16),
            "gs" => cpu.set_gs(v as u16),
            "ip" => cpu.set_ip(v as u16),
            "eip" => cpu.set_eip(v),
            "flags" => cpu.load_flags16(v as u16),
            "eflags" => cpu.load_eflags(v),
            _ => unreachable!(),
        }
    }
    Ok(())
}

fn screen_text(cpu: &Cpu) -> Reply {
    let mode = cpu.bus.video_mode;
    let Some(geometry) = crate::video::text::geometry(&cpu.bus) else {
        return Reply::Error(409, format!("video mode {:?} is a graphics mode; use /api/screenshot", mode));
    };
    let (cols, rows) = (geometry.cols, geometry.rows);
    let vram = cpu.bus.display_mem();
    let lines: Vec<String> = (0..rows)
        .map(|r| {
            let s: String = (0..cols)
                .map(|c| {
                    let off = (geometry.start + r * geometry.row_bytes + c * 2) & geometry.wrap;
                    keys::CP437[vram[off] as usize]
                })
                .collect();
            s.trim_end().to_string()
        })
        .collect();
    Reply::Json(json!({
        "mode": format!("{:?}", mode),
        "cols": cols,
        "rows": rows,
        "cursor": {"col": cpu.bus.peek_8(0x0450), "row": cpu.bus.peek_8(0x0451)},
        "lines": lines,
    }))
}

fn ivt_json(cpu: &Cpu) -> Value {
    let entries: Vec<Value> = (0..256usize)
        .map(|v| {
            let off = cpu.bus.peek_8(v * 4) as u16 | (cpu.bus.peek_8(v * 4 + 1) as u16) << 8;
            let seg = cpu.bus.peek_8(v * 4 + 2) as u16 | (cpu.bus.peek_8(v * 4 + 3) as u16) << 8;
            let phys = cpu.get_physical_addr(seg, off);
            let hle = cpu.bus.peek_8(phys) == 0xFE && cpu.bus.peek_8(phys + 1) == 0x38;
            json!({"int": format!("{:02X}", v), "vector": format!("{:04X}:{:04X}", seg, off), "hle": hle})
        })
        .collect();
    json!({"ivt": entries})
}

/// True while the built-in shell (segment 0000) is running with no program
/// loaded. At the prompt the shell spends half its time inside the INT 16h
/// BIOS trap at F000, so a trap whose caller (the CS in the IRET frame on
/// top of the stack) is the shell counts as well.
fn drives_json(cpu: &Cpu) -> Value {
    let disk = &cpu.bus.disk;
    let drives: Map<String, Value> = disk
        .all_drives()
        .into_iter()
        .map(|info| {
            let entry = json!({
                "type": info.kind.name(),
                "path": info.root.as_ref().or(info.image.as_ref()).map(|p| display_host_path(p)),
                "image": info.image.is_some(),
                "images": info.images.iter().map(|p| display_host_path(p)).collect::<Vec<_>>(),
                "image_index": info.image_index,
                "label": info.label,
                "read_only": info.read_only,
                "current_dir": info.current_dir,
            });
            (crate::disk::drive_key(info.drive), entry)
        })
        .collect();
    json!({
        "current_drive": drive_letter(cpu.bus.disk.get_current_drive()).to_string(),
        "current_dir": cpu.bus.disk.get_current_directory(),
        "drives": drives,
    })
}

/// Mount (or replace) a drive from the debug API. Without explicit options
/// a remount keeps the drive's current type, label and read-only flag.
fn mount_drive(
    cpu: &mut Cpu,
    drive: &str,
    path: &str,
    kind: Option<&str>,
    label: Option<String>,
    read_only: bool,
    images: Vec<String>,
) -> Result<PathBuf, String> {
    let drive =
        parse_drive_name(drive).ok_or_else(|| format!("invalid drive letter '{}'", drive))?;
    let kind = match kind {
        Some(k) => Some(parse_kind(k).ok_or_else(|| format!("unknown drive type '{}'", k))?),
        None => None,
    };
    let explicit = kind.is_some() || label.is_some() || read_only;
    let path = PathBuf::from(path);
    let mut opts = match cpu.bus.disk.drive_info(drive) {
        // An image brings its own type and label.
        Some(info) if !explicit && !path.is_file() => MountOptions {
            kind: info.kind,
            label: info.image.is_none().then_some(info.label),
            read_only: info.read_only,
            ..Default::default()
        },
        _ => MountOptions {
            kind: kind.unwrap_or(DriveKind::HardDisk),
            label,
            read_only,
            ..Default::default()
        },
    };
    opts.more_images = images.into_iter().map(PathBuf::from).collect();
    cpu.bus.mount_drive(drive, &path, opts, true)
}

impl crate::exec::ExecHook for DebugHub {
    #[inline]
    fn before_exec(&mut self, cpu: &Cpu, phys_ip: usize, ram: &[u8]) -> bool {
        self.check_before_exec(cpu, phys_ip, ram)
    }

    /// A program that ended stops the machine before the shell goes on to
    /// the next line of its batch file (another program, or EXIT).
    fn before_shell(&mut self, cpu: &Cpu) -> bool {
        if cpu.programs.ended != self.seen_ends {
            self.seen_ends = cpu.programs.ended;
            if self.break_program_exit {
                self.enter_pause(PauseReason::ProgramExit);
                return true;
            }
        }
        false
    }
}

/// `/api/printer`: the printer on LPT1, what it printed and where it
/// went. With `eject`, the job ends first and its files are written.
fn printer_json(cpu: &mut Cpu, eject: bool) -> serde_json::Value {
    let output = cpu.bus.printer_output_settings().output;
    let lpt_dac = cpu.bus.lpt_dac.is_some();
    let Some(p) = &mut cpu.bus.printer else {
        return json!({"present": false, "output": output.name(), "lpt_dac": lpt_dac});
    };
    if eject {
        p.eject();
    }
    p.sync();
    json!({
        "present": true,
        "output": p.settings.output.name(),
        "dpi": p.settings.dpi,
        "paper": p.settings.paper.name(),
        "docpath": p.settings.docpath.display().to_string(),
        "printing": p.busy(),
        "bytes": p.bytes,
        "pages": p.pages,
        "jobs": p.jobs,
        "last": p.last,
    })
}

/// `/api/net`: the IPX driver's sockets, ECBs and packets, and the LAN.
fn serial_json(cpu: &Cpu) -> serde_json::Value {
    use rust_dos::serial::Backend;
    let ports: Vec<_> = cpu
        .bus
        .serial
        .ports
        .iter()
        .enumerate()
        .map(|(n, port)| {
            let Some(port) = port else { return json!({"port": format!("COM{}", n + 1), "present": false}) };
            let uart: serde_json::Map<_, _> =
                port.uart.describe().into_iter().map(|(k, v)| (k.to_string(), serde_json::Value::String(v))).collect();
            let backend = match &port.backend {
                Backend::Empty => json!({"type": "empty"}),
                Backend::Mouse(m) => json!({"type": "mouse", "kind": m.kind.name(), "powered": m.powered}),
                Backend::NullModem(c) => json!({"type": "nullmodem", "peer": c.peer, "lines": format!("{:02X}", c.lines())}),
                Backend::Modem(m) => json!({
                    "type": "modem",
                    "call": format!("{:?}", m.call),
                    "online": m.online,
                    "direct": m.direct,
                    "peer": m.peer,
                }),
            };
            json!({"port": format!("COM{}", n + 1), "present": true, "uart": uart, "backend": backend})
        })
        .collect();
    json!({
        "ports": ports,
        "pic_lines": format!("{:04X}", cpu.bus.serial.pic_lines),
        "link": cpu.bus.net.serial_status(),
    })
}

fn net_json(cpu: &Cpu) -> serde_json::Value {
    let far = |(segment, offset): (u16, u16)| format!("{:04X}:{:04X}", segment, offset);
    let ipx = cpu.bus.net.ipx.as_ref().map(|ipx| {
        use rust_dos::net::ipx::Done;
        json!({
            "node": ipx.node.to_string(),
            "irq": ipx.irq,
            "frame_type": ipx.frame_type.name(),
            "sockets": ipx.sockets.iter().map(|s| json!({
                "socket": format!("{:04X}", s.number),
                "long_lived": s.long_lived,
                "owner": format!("{:04X}", s.owner),
            })).collect::<Vec<_>>(),
            "listening": ipx.listens.iter().map(|l| json!({"ecb": far(l.ecb), "socket": format!("{:04X}", l.socket)})).collect::<Vec<_>>(),
            "events": ipx.events.iter().map(|e| json!({"ecb": far(e.ecb), "due": e.due})).collect::<Vec<_>>(),
            "completions": ipx.completions.iter().map(|c| json!({
                "ecb": far(c.ecb),
                "done": match &c.done {
                    Done::Sent => "sent".to_string(),
                    Done::Event => "event".to_string(),
                    Done::Received { packet, from } => format!("received {} bytes from {}", packet.len(), from),
                },
            })).collect::<Vec<_>>(),
            "held": ipx.held.iter().map(|h| json!({"socket": format!("{:04X}", h.socket), "bytes": h.packet.len(), "from": h.from.to_string()})).collect::<Vec<_>>(),
            "stats": {
                "sent": ipx.stats.sent,
                "received": ipx.stats.received,
                "not_ours": ipx.stats.not_ours,
                "closed_socket": ipx.stats.closed_socket,
                "unheard": ipx.stats.unheard,
            },
        })
    });
    let lan = cpu.bus.net.status().hub.map(|h| json!({
        "state": format!("{:?}", h.lan),
        "room": h.room,
        "hosting": h.hosting.map(|a| a.to_string()),
        "hosted_rooms": h.hosted_rooms.iter().map(|r| json!({"room": r.name, "members": r.members})).collect::<Vec<_>>(),
        "frames_out": h.frames_out,
        "frames_in": h.frames_in,
    }));
    json!({ "settings": format!("{:?}", cpu.bus.net.settings), "ipx": ipx, "lan": lan })
}

#[cfg(test)]
mod determinism_tests;

#[cfg(test)]
mod tests {
    use super::journal::{OpLookup, OpState};
    use super::{Answer, Cmd, DebugHub, InputEvent, LowInput, Reply, StateRequest, TraceQuery, keys_for_char, step_over_len};
    use rust_dos::deterministic::Deterministic;
    use crate::cpu::{Cpu, CpuState};
    use crate::exec::{self, StopReason};
    use rust_dos::keylayout::Layout;
    use serde_json::Value;
    use tokio::sync::oneshot;

    fn handle(cpu: &mut Cpu, hub: &mut DebugHub, cmd: Cmd) -> Reply {
        let (request, mut rx) = hub.request(cmd);
        hub.handle(cpu, request);
        rx.try_recv().expect("an answer at once").reply
    }

    /// The journal's entries, as JSON.
    fn entries(hub: &DebugHub) -> Vec<Value> {
        hub.with_journal(|j| j.after(0).map(|it| it.map(|e| e.value()).collect()).unwrap_or_default()).unwrap_or_default()
    }

    /// The journal's entries of type `kind`.
    fn entries_of(hub: &DebugHub, kind: &str) -> Vec<Value> {
        entries(hub).into_iter().filter(|e| e["type"] == kind).collect()
    }

    /// A trace page from cursor `since`: its instruction counts, the next
    /// cursor and the number dropped.
    fn page(cpu: &mut Cpu, hub: &mut DebugHub, since: u64, limit: usize) -> (Vec<u64>, u64, u64) {
        let q = TraceQuery { since: Some(since), limit: Some(limit), ..Default::default() };
        match handle(cpu, hub, Cmd::TraceQuery(q)) {
            Reply::Trace { entries, next, dropped } => (entries.iter().map(|e| e.icount).collect(), next, dropped),
            _ => panic!("not a trace"),
        }
    }

    #[test]
    fn a_trace_with_a_count_records_that_many_and_stops() {
        let mut cpu = Cpu::new(".".into());
        cpu.load_shell();
        let mut hub = DebugHub::new(None, None, 1000);
        handle(&mut cpu, &mut hub, Cmd::TraceControl { enabled: None, clear: false, stream_max: None, count: Some(50) });
        // The shell idles at its prompt waiting for a key, running only a
        // few instructions a batch.
        for _ in 0..100 {
            let hot = hub.begin_batch(&cpu);
            cpu.bus.start_batch(cpu.bus.clock.icount + 2000);
            crate::exec::run_batch(&mut cpu, &mut hub, hot);
            hub.end_batch(&cpu);
        }
        assert_eq!(hub.trace.total(), 50);
        assert!(!hub.trace_enabled && hub.trace_until.is_none());
        // Paged through: two pages of 30 and 20, then nothing new.
        let (first, next, dropped) = page(&mut cpu, &mut hub, 0, 30);
        assert_eq!((first.len(), next, dropped), (30, 30, 0));
        let (second, next, _) = page(&mut cpu, &mut hub, next, 30);
        assert_eq!((second.len(), next), (20, 50));
        assert!(first.last() < second.first(), "oldest first, in order");
        assert_eq!(page(&mut cpu, &mut hub, next, 30), (vec![], 50, 0));
    }

    #[test]
    fn a_trace_reply_shows_whether_it_records_now() {
        let mut cpu = Cpu::new(".".into());
        let mut hub = DebugHub::new(None, None, 1000);
        let mut control = |enabled, count| {
            let cmd = Cmd::TraceControl { enabled, clear: false, stream_max: None, count };
            match handle(&mut cpu, &mut hub, cmd) {
                Reply::Json(v) => v["active"].as_bool(),
                _ => panic!("not JSON"),
            }
        };
        assert_eq!(control(Some(true), None), Some(true));
        assert_eq!(control(None, Some(0)), Some(false));
        assert_eq!(control(None, Some(10)), Some(true));
        assert_eq!(control(Some(false), None), Some(false));
    }

    #[test]
    fn a_count_shows_what_is_left_and_a_count_of_0_stops_the_trace() {
        let mut cpu = Cpu::new(".".into());
        let mut hub = DebugHub::new(None, None, 1000);
        let control = |count| Cmd::TraceControl { enabled: None, clear: false, stream_max: None, count: Some(count) };
        let Reply::Json(status) = handle(&mut cpu, &mut hub, control(50)) else { panic!("not json") };
        assert_eq!((status["enabled"].as_bool(), status["remaining"].as_u64()), (Some(true), Some(50)));
        let Reply::Json(status) = handle(&mut cpu, &mut hub, control(0)) else { panic!("not json") };
        assert_eq!(status["enabled"].as_bool(), Some(false));
        assert!(status["remaining"].is_null());
    }

    /// The reply to a command the hub answers at once.
    fn reply(cpu: &mut Cpu, hub: &mut DebugHub, cmd: Cmd) -> Result<Value, (u16, String)> {
        match handle(cpu, hub, cmd) {
            Reply::Json(v) => Ok(v),
            Reply::Error(code, e) => Err((code, e)),
            Reply::ErrorJson(code, v) => Err((code, v.to_string())),
            _ => panic!("no JSON reply"),
        }
    }

    fn write(addr: &str, data: &[u8], expect: Option<&[u8]>) -> Cmd {
        Cmd::WriteMem { addr: addr.into(), data: data.to_vec(), expect: expect.map(<[u8]>::to_vec) }
    }

    #[test]
    fn a_write_reports_the_bytes_it_replaced() {
        let (mut cpu, mut hub) = (Cpu::new(".".into()), DebugHub::disabled());
        cpu.bus.write_8(0x2000, 0xCD);
        cpu.bus.write_8(0x2001, 0xAB);
        let done = reply(&mut cpu, &mut hub, write("0200:0000", &[0x21, 0x43], None)).unwrap();
        assert_eq!((done["old"].as_str(), done["new"].as_str()), (Some("CD AB"), Some("21 43")));
    }

    #[test]
    fn a_write_expecting_other_bytes_writes_nothing() {
        let (mut cpu, mut hub) = (Cpu::new(".".into()), DebugHub::disabled());
        cpu.bus.write_8(0x2000, 0xCD);
        cpu.bus.write_8(0x2001, 0xAB);
        let refused = reply(&mut cpu, &mut hub, write("2000", &[0x21, 0x43], Some(&[0x34, 0x12]))).unwrap_err();
        assert_eq!(refused.0, 409);
        assert!(refused.1.contains(r#""found":"CD AB""#), "{}", refused.1);
        assert!(refused.1.contains(r#""expected":"34 12""#), "{}", refused.1);
        assert_eq!((cpu.bus.peek_8(0x2000), cpu.bus.peek_8(0x2001)), (0xCD, 0xAB));
        let done = reply(&mut cpu, &mut hub, write("2000", &[0x21, 0x43], Some(&[0xCD, 0xAB]))).unwrap();
        assert_eq!(done["new"], "21 43");
        let short = reply(&mut cpu, &mut hub, write("2000", &[0x21, 0x43], Some(&[0x21]))).unwrap_err();
        assert_eq!(short.0, 400, "expect is as long as the data");
    }

    /// A .COM program: MOV AX,1234h; MOV BX,5678h; MOV AX,[0110h];
    /// MOV AX,4C00h; INT 21h; and the word ABCDh at 0110h.
    const PROBE: &str = "B83412BB7856A11001B8004CCD210000CDAB";

    /// A .COM program that divides by zero: XOR AX,AX; DIV AL.
    const CRASH: [u8; 4] = [0x31, 0xC0, 0xF6, 0xF0];

    /// A machine at the prompt with PROBE.COM on C:, and a hub with no
    /// server, which takes its commands from `send`.
    fn machine(name: &str) -> (Cpu, DebugHub) {
        let dir = std::env::temp_dir().join(format!("rust-dos-debug-{}-{}", name, std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let code: Vec<u8> = (0..PROBE.len()).step_by(2).map(|i| u8::from_str_radix(&PROBE[i..i + 2], 16).unwrap()).collect();
        std::fs::write(dir.join("PROBE.COM"), code).unwrap();
        std::fs::write(dir.join("CRASH.COM"), CRASH).unwrap();
        let mut cpu = Cpu::new(dir);
        cpu.load_shell();
        // Polled as the server's is; this one's commands come from `send`.
        let mut hub = DebugHub::for_tests(&cpu);
        run(&mut cpu, &mut hub, 20);
        assert!(cpu.shell_idle(), "the shell waits at its prompt");
        (cpu, hub)
    }

    /// Run `frames` frames of a few thousand instructions, as the main loop
    /// does: the hub's requests first, nothing while it is paused.
    fn run(cpu: &mut Cpu, hub: &mut DebugHub, frames: usize) {
        for _ in 0..frames {
            hub.poll(cpu);
            if hub.paused {
                continue;
            }
            let hot = hub.begin_batch(cpu);
            let end = cpu.bus.clock.icount + 5000;
            cpu.bus.start_batch(end);
            while cpu.bus.clock.icount < end {
                match exec::run_batch(cpu, hub, hot) {
                    StopReason::BatchEnd | StopReason::Paused | StopReason::Exit => break,
                    StopReason::ShellReloaded => cpu.bus.start_batch(end),
                }
            }
            hub.end_batch(cpu);
        }
    }

    /// A request sent to the hub: what the server waits for after the
    /// hub's answer, and the answer.
    struct Sent {
        wait: Wait,
        rx: oneshot::Receiver<Answer>,
    }

    enum Wait {
        /// Nothing: the answer is the reply.
        No,
        /// The end of the operation the request started.
        Op,
        /// The stop the machine is in, or the next.
        Stop,
    }

    fn send(cpu: &mut Cpu, hub: &mut DebugHub, cmd: Cmd) -> Sent {
        let wait = match cmd {
            Cmd::Step { .. } | Cmd::StepOver | Cmd::Run { .. } => Wait::Op,
            Cmd::WaitPause => Wait::Stop,
            _ => Wait::No,
        };
        let (request, rx) = hub.request(cmd);
        hub.handle(cpu, request);
        Sent { wait, rx }
    }

    /// How operation `op` ended, as the server tells it: the stop, a run's
    /// program, or its failure. None while it is pending.
    fn op_reply(hub: &DebugHub, op: u64) -> Option<Result<Value, (u16, String)>> {
        hub.with_journal(|j| {
            let OpLookup::Found(found) = j.op(op) else { panic!("no operation {}", op) };
            let end = found.end_seq.and_then(|s| j.get(s)).map(|e| e.value()).unwrap_or(Value::Null);
            match found.state {
                OpState::Pending => None,
                OpState::Stopped => Some(Ok(end)),
                OpState::Done => Some(Ok(serde_json::json!({"ok": true, "program": end["program"]}))),
                OpState::Failed => Some(Err(found.error.clone().unwrap())),
                OpState::Cancelled => Some(Err((409, end.to_string()))),
                OpState::Superseded => Some(Err((409, "superseded".into()))),
            }
        })
        .unwrap()
    }

    /// The first `paused` entry after `after`.
    fn stop_after(hub: &DebugHub, after: u64) -> Option<Value> {
        entries(hub).into_iter().find(|e| e["type"] == "paused" && e["seq"].as_u64() > Some(after))
    }

    /// The reply to a command, after running up to `frames` frames for it
    /// and what the server waits for after the hub's answer.
    fn answer(cpu: &mut Cpu, hub: &mut DebugHub, mut sent: Sent, frames: usize) -> Result<Value, (u16, String)> {
        let mut frame = 0;
        let answer = loop {
            match sent.rx.try_recv() {
                Ok(answer) => break answer,
                Err(_) if frame < frames => run(cpu, hub, 1),
                Err(_) => panic!("no answer in {} frames", frames),
            }
            frame += 1;
        };
        let value = match answer.reply {
            Reply::Json(v) => v,
            Reply::Error(code, e) => return Err((code, e)),
            Reply::ErrorJson(code, v) => return Err((code, v.to_string())),
            _ => panic!("not a JSON reply"),
        };
        loop {
            let done = match sent.wait {
                Wait::No => Some(Ok(value.clone())),
                Wait::Op => op_reply(hub, answer.meta.op.expect("an operation")),
                Wait::Stop if value["paused"] == true => Some(Ok(value.clone())),
                Wait::Stop => stop_after(hub, answer.meta.last).map(Ok),
            };
            match done {
                Some(done) => return done,
                None if frame < frames => run(cpu, hub, 1),
                None => panic!("no end in {} frames", frames),
            }
            frame += 1;
        }
    }

    fn ask(cpu: &mut Cpu, hub: &mut DebugHub, cmd: Cmd, frames: usize) -> Result<Value, (u16, String)> {
        let rx = send(cpu, hub, cmd);
        answer(cpu, hub, rx, frames)
    }

    fn break_on(program_start: Option<bool>, program_exit: Option<bool>) -> Cmd {
        Cmd::BreakOn { exceptions: None, clear_exceptions: None, mode_switch: None, program_start, program_exit }
    }

    #[test]
    fn run_stops_at_the_program_s_entry_point() {
        let (mut cpu, mut hub) = machine("entry");
        let rx = send(&mut cpu, &mut hub, Cmd::Run { command: "PROBE.COM".into(), stop_at_entry: true });
        let stop = answer(&mut cpu, &mut hub, rx, 50).unwrap();
        assert_eq!(stop["reason"], "program_start");
        assert_eq!(stop["program"]["name"], "PROBE.COM");
        assert_eq!(stop["registers"]["ip"], "0100");
        assert_eq!(stop["program"]["entry"], format!("{}:0100", stop["registers"]["cs"].as_str().unwrap()));
        assert!(hub.paused);
        // Nothing of the program ran: AX is what DOS starts it with.
        assert_eq!(cpu.ax(), 0);
    }

    #[test]
    fn a_program_s_end_stops_the_machine_with_its_exit_code() {
        let (mut cpu, mut hub) = machine("exit");
        ask(&mut cpu, &mut hub, break_on(None, Some(true)), 0).unwrap();
        ask(&mut cpu, &mut hub, Cmd::Run { command: "PROBE.COM".into(), stop_at_entry: true }, 50).unwrap();
        ask(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: None }, 0).unwrap();
        let stop = ask(&mut cpu, &mut hub, Cmd::WaitPause, 50).unwrap();
        assert_eq!(stop["reason"], "program_exit");
        assert!(hub.paused);
        assert_eq!(stop["exit"], serde_json::json!({"name": "PROBE.COM", "code": 0, "resident": false, "aborted": false}));
    }

    #[test]
    fn a_program_the_emulator_ends_stops_the_machine_with_no_exit_code() {
        let (mut cpu, mut hub) = machine("abort");
        ask(&mut cpu, &mut hub, break_on(None, Some(true)), 0).unwrap();
        ask(&mut cpu, &mut hub, Cmd::Run { command: "CRASH.COM".into(), stop_at_entry: true }, 50).unwrap();
        ask(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: None }, 0).unwrap();
        let stop = ask(&mut cpu, &mut hub, Cmd::WaitPause, 50).unwrap();
        assert_eq!(stop["reason"], "program_exit");
        assert_eq!(stop["exit"], serde_json::json!({"name": "CRASH.COM", "code": null, "resident": false, "aborted": true}));
        // The next program's own exit is told apart again.
        ask(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: None }, 0).unwrap();
        run(&mut cpu, &mut hub, 20);
        ask(&mut cpu, &mut hub, Cmd::Run { command: "PROBE.COM".into(), stop_at_entry: true }, 50).unwrap();
        ask(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: None }, 0).unwrap();
        let stop = ask(&mut cpu, &mut hub, Cmd::WaitPause, 50).unwrap();
        assert_eq!(stop["exit"]["aborted"], false);
    }

    #[test]
    fn a_batch_file_s_program_stops_at_its_end_before_the_next_line() {
        let (mut cpu, mut hub) = machine("batch");
        ask(&mut cpu, &mut hub, break_on(None, Some(true)), 0).unwrap();
        // The shell runs the next line right after the first program ends,
        // with no instruction of the machine's in between.
        cpu.queue_batch_lines(["PROBE.COM", "CRASH.COM"]);
        let stop = ask(&mut cpu, &mut hub, Cmd::WaitPause, 50).unwrap();
        assert_eq!(stop["reason"], "program_exit");
        assert_eq!(stop["exit"]["name"], "PROBE.COM");
        assert!(cpu.shell_idle() && cpu.program.is_empty(), "back at the prompt, CRASH.COM not started");
    }

    #[test]
    fn run_without_stopping_answers_once_the_program_started() {
        let (mut cpu, mut hub) = machine("started");
        let started = ask(&mut cpu, &mut hub, Cmd::Run { command: "PROBE.COM".into(), stop_at_entry: false }, 50).unwrap();
        assert_eq!(started["program"]["name"], "PROBE.COM");
        assert!(!hub.paused);
        run(&mut cpu, &mut hub, 20);
        assert_eq!(super::exit_json(&cpu)["name"], "PROBE.COM", "the status shows how it ended");
    }

    #[test]
    fn every_program_start_stops_while_asked_to() {
        let (mut cpu, mut hub) = machine("every");
        ask(&mut cpu, &mut hub, break_on(Some(true), None), 0).unwrap();
        cpu.queue_batch_lines(["PROBE.COM"]);
        let stop = ask(&mut cpu, &mut hub, Cmd::WaitPause, 50).unwrap();
        assert_eq!((stop["reason"].as_str(), stop["registers"]["ip"].as_str()), (Some("program_start"), Some("0100")));
    }

    #[test]
    fn run_refuses_a_command_line_that_starts_no_program() {
        let (mut cpu, mut hub) = machine("none");
        let rx = send(&mut cpu, &mut hub, Cmd::Run { command: "NOSUCH.EXE".into(), stop_at_entry: true });
        assert_eq!(answer(&mut cpu, &mut hub, rx, 50).unwrap_err().0, 422);
        assert!(!hub.paused && !hub.start_once, "no stop is left armed");
    }

    #[test]
    fn run_waits_for_the_prompt() {
        let (mut cpu, mut hub) = machine("busy");
        let rx = send(&mut cpu, &mut hub, Cmd::Run { command: "PROBE.COM".into(), stop_at_entry: true });
        answer(&mut cpu, &mut hub, rx, 50).unwrap();
        let rx = send(&mut cpu, &mut hub, Cmd::Run { command: "PROBE.COM".into(), stop_at_entry: true });
        assert_eq!(answer(&mut cpu, &mut hub, rx, 0).unwrap_err().0, 409, "a program is running");
    }

    #[test]
    fn reboot_shell_drops_the_batch_file_too() {
        let (mut cpu, mut hub) = (Cpu::new(".".into()), DebugHub::disabled());
        cpu.load_shell();
        cpu.queue_batch_lines(["GAME.EXE", "EXIT"]);
        let reply = handle(&mut cpu, &mut hub, Cmd::RebootShell);
        assert!(matches!(reply, Reply::Json(_)));
        assert_eq!(cpu.state, CpuState::RebootShell);
        assert!(hub.take_closed_program(), "the front end releases its part");
        // Run the shell's reload and what would come after it: without the
        // fix, the batch's EXIT runs and asks to turn the machine off.
        let mut reloaded = false;
        for _ in 0..200 {
            let end = cpu.bus.clock.icount + 1000;
            cpu.bus.start_batch(end);
            match exec::run_batch(&mut cpu, &mut exec::NoHook, false) {
                StopReason::ShellReloaded => reloaded = true,
                StopReason::Exit => panic!("EXIT ran after the reload"),
                _ => {}
            }
        }
        assert!(reloaded, "the shell reloads");
        assert!(!cpu.bus.exit_requested);
        assert!(!cpu.batch.is_active(), "EXIT must not run after the reload");
    }

    /// Pause the machine, as the main loop does: its stop is told at the
    /// end of the frame.
    fn pause(cpu: &mut Cpu, hub: &mut DebugHub) {
        let rx = send(cpu, hub, Cmd::Pause);
        hub.end_batch(cpu);
        answer(cpu, hub, rx, 0).unwrap();
        assert!(hub.paused);
    }

    /// Ask to save a state, with strict_match or not, and the requests the
    /// front end gets to carry out, with the reply of one refused.
    fn state_request(cpu: &mut Cpu, hub: &mut DebugHub, strict_match: bool) -> (usize, Option<(u16, Value)>) {
        let mut sent = send(cpu, hub, Cmd::SaveState { path: "/tmp/x.state".into(), strict_match, check_files: false });
        let taken = take_state_requests(hub).len();
        let refused = match sent.rx.try_recv().map(|a| a.reply) {
            Ok(Reply::ErrorJson(code, body)) => Some((code, body)),
            Ok(_) => panic!("not a refusal"),
            Err(_) => None,
        };
        (taken, refused)
    }

    /// The save states the front end gets to save or load now.
    fn take_state_requests(hub: &mut DebugHub) -> Vec<StateRequest> {
        std::iter::from_fn(|| {
            let request = hub.next_state_request();
            hub.finish();
            request
        })
        .collect()
    }

    #[test]
    fn a_strict_match_save_or_load_is_done_only_while_the_machine_is_paused() {
        let (mut cpu, mut hub) = machine("strict-match");
        let (taken, refused) = state_request(&mut cpu, &mut hub, true);
        let (code, body) = refused.expect("refused while running");
        assert_eq!((taken, code), (0, 409));
        assert!(body["error"].as_str().unwrap().contains("pause it first"), "{}", body);

        pause(&mut cpu, &mut hub);
        let (taken, refused) = state_request(&mut cpu, &mut hub, true);
        assert_eq!((taken, refused.is_none()), (1, true), "paused with nothing pending");

        // A step sent with it, in the same frame, starts the machine first.
        let strict = Cmd::SaveState { path: "/tmp/x.state".into(), strict_match: true, check_files: false };
        let _save = send(&mut cpu, &mut hub, strict);
        let _step = send(&mut cpu, &mut hub, Cmd::Step { count: 1 });
        assert!(take_state_requests(&mut hub).is_empty(), "refused: the step is under way");

        // Queued input goes on being typed while paused: a save waits for
        // it, and a load drops it.
        let (mut cpu, mut hub) = machine("strict-match-input");
        pause(&mut cpu, &mut hub);
        hub.input.push_back(LowInput::Wait(std::time::Duration::from_secs(60)));
        let (taken, refused) = state_request(&mut cpu, &mut hub, true);
        let (code, body) = refused.expect("refused with input queued");
        assert_eq!((taken, code), (0, 409));
        assert!(body["error"].as_str().unwrap().contains("queued input"), "{}", body);
        let _load = send(&mut cpu, &mut hub, Cmd::LoadState { path: "/tmp/x.state".into(), strict_match: true, check_files: false });
        assert_eq!(take_state_requests(&mut hub).len(), 1, "a load drops the input");
    }

    #[test]
    fn input_whose_last_wait_is_under_way_is_still_being_typed() {
        let (mut cpu, mut hub) = machine("strict-match-wait");
        pause(&mut cpu, &mut hub);
        let op = started_op(&mut cpu, &mut hub, Cmd::Input { events: vec![InputEvent::Wait { ms: 60_000 }], wait: false });
        // The wait starts: only the end of the request is left in the queue.
        run(&mut cpu, &mut hub, 3);
        assert_eq!(op_state(&hub, op).state, OpState::Pending);
        let (taken, refused) = state_request(&mut cpu, &mut hub, true);
        let (code, body) = refused.expect("refused while the input's operation is pending");
        assert_eq!((taken, code, body["input_queued"].as_u64()), (0, 409, Some(1)), "{}", body);
        let read = journal_state(&hub).0;
        let (refused, _) = ask_guarded(&mut cpu, &mut hub, Cmd::GetRegs, Some(read));
        assert_eq!(refused.unwrap_err().1["precondition"], "state");
    }

    #[test]
    fn a_plain_save_or_load_is_done_running_or_not() {
        let (mut cpu, mut hub) = machine("no-strict-match");
        let (taken, refused) = state_request(&mut cpu, &mut hub, false);
        assert_eq!((taken, refused.is_none()), (1, true), "taken while running");

        // Sent with a strict-match save while running: only that save is refused.
        let mut strict = send(&mut cpu, &mut hub, Cmd::SaveState { path: "/tmp/a.state".into(), strict_match: true, check_files: false });
        let _plain = send(&mut cpu, &mut hub, Cmd::LoadState { path: "/tmp/b.state".into(), strict_match: false, check_files: false });
        let taken = take_state_requests(&mut hub);
        assert_eq!(taken.iter().map(|r| (r.load, r.strict_match)).collect::<Vec<_>>(), [(true, false)]);
        assert!(matches!(strict.rx.try_recv().map(|a| a.reply), Ok(Reply::ErrorJson(409, _))));
    }

    #[test]
    fn a_loaded_state_leaves_no_breakpoint_watchpoint_or_stop_from_before() {
        let (mut cpu, mut hub) = machine("reset");
        pause(&mut cpu, &mut hub);
        ask(&mut cpu, &mut hub, Cmd::AddBreakpoint { addr: "0100:0000".into(), once: false }, 0).unwrap();
        ask(&mut cpu, &mut hub, Cmd::AddWatchpoint { addr: "0200:0000".into(), len: 2 }, 0).unwrap();
        let on = Cmd::BreakOn { exceptions: Some(1), clear_exceptions: None, mode_switch: Some(true), program_start: None, program_exit: Some(true) };
        ask(&mut cpu, &mut hub, on, 0).unwrap();
        hub.temp_breakpoint = Some(0x1234);
        let typed = Cmd::Input { events: vec![InputEvent::Wait { ms: 60_000 }], wait: true };
        let sent = send(&mut cpu, &mut hub, typed);
        answer(&mut cpu, &mut hub, sent, 0).unwrap();
        hub.key_down(&mut cpu, super::keys::lookup("a").unwrap(), b'a');

        let (cleared, cancelled) = hub.reset_after_load(&cpu);
        assert_eq!(
            cleared,
            serde_json::json!({
                "breakpoints": 1, "watchpoints": 1, "break_on": ["exceptions", "mode_switch", "program_exit"],
                "run_to": true, "step": false, "pause_waits": 0, "run": false, "input": 1, "held_keys": 1, "until_ms": false,
            })
        );
        let queued = entries_of(&hub, "input_queued");
        assert_eq!(cancelled, [queued[0]["cause"].as_u64().unwrap()], "the input's operation is cancelled");
        // The key is forgotten: no key-up reaches the restored machine.
        let before = cpu.bus.kbc.pending();
        hub.release_keys(&mut cpu);
        assert_eq!(cpu.bus.kbc.pending(), before, "no break code after the load");
        assert!(!hub.begin_batch(&cpu), "nothing is left to stop the machine");
        assert!(hub.paused, "the machine stays paused");
        let listed = ask(&mut cpu, &mut hub, Cmd::ListBreakpoints, 0).unwrap();
        assert_eq!(listed["breakpoints"].as_array().map(Vec::len), Some(0), "{}", listed);
    }

    #[test]
    fn the_cache_hit_rate_starts_again_after_a_load() {
        let (_cpu, mut hub) = machine("cache-rate");
        hub.stats.cache_mark = (100, 50);
        hub.stats.window_start = std::time::Instant::now() - std::time::Duration::from_secs(2);
        // A loaded state put in a new decode cache, counting from zero.
        hub.record_batch(0, std::time::Duration::ZERO, 10, 5);
        assert!((hub.stats.cache_hit_rate - 10.0 / 15.0).abs() < 1e-9, "{}", hub.stats.cache_hit_rate);
        assert_eq!(hub.stats.cache_mark, (10, 5));
    }

    /// The scan codes of the keys going down to type `c`.
    fn scans(c: char, layout: &str) -> Vec<u8> {
        let mut out = Vec::new();
        keys_for_char(c, Layout::by_code(layout).unwrap(), &mut out).unwrap();
        out.iter()
            .filter_map(|ev| match ev {
                LowInput::KeyDown { key, .. } => Some(key.scan),
                LowInput::Char(byte) => Some(*byte),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn text_is_typed_on_the_layout_s_keys() {
        assert_eq!(scans('y', "gr"), [0x2C]);
        assert_eq!(scans('y', "us"), [0x15]);
        assert_eq!(scans(':', "gr"), [0x2A, 0x34], "Shift and the period key");
        assert_eq!(scans('\\', "gr"), [0x38, 0x0C], "AltGr and the ß key");
        assert_eq!(scans('ä', "gr"), [0x28]);
        // A character no key types comes as a keystroke of its own.
        assert_eq!(scans('é', "us"), [0x82]);
        assert_eq!(scans('\n', "fr"), [0x1C]);
    }

    #[test]
    fn step_over_runs_calls_interrupts_loops_and_repeats() {
        // CALL rel16, CALL [BX], INT 21h, INT3, LOOP, REP MOVSB, REPNE SCASB.
        assert_eq!(step_over_len(&[0xE8, 0x10, 0x00, 0x90], false), Some(3));
        assert_eq!(step_over_len(&[0xFF, 0x17, 0x90], false), Some(2));
        assert_eq!(step_over_len(&[0xCD, 0x21, 0x90], false), Some(2));
        assert_eq!(step_over_len(&[0xCC, 0x90], false), Some(1));
        assert_eq!(step_over_len(&[0xE2, 0xFE, 0x90], false), Some(2));
        assert_eq!(step_over_len(&[0xF3, 0xA4, 0x90], false), Some(2));
        assert_eq!(step_over_len(&[0xF2, 0xAE, 0x90], false), Some(2));
        // CALL rel32 in 32-bit code.
        assert_eq!(step_over_len(&[0xE8, 0, 0, 0, 0, 0x90], true), Some(5));
    }

    #[test]
    fn other_instructions_just_step() {
        // MOV AX,1234h; JMP short; MOVSB without REP; the emulator's traps.
        assert_eq!(step_over_len(&[0xB8, 0x34, 0x12], false), None);
        assert_eq!(step_over_len(&[0xEB, 0xFE], false), None);
        assert_eq!(step_over_len(&[0xA4], false), None);
        assert_eq!(step_over_len(&[0xFE, 0x38, 0x21], false), None);
        assert_eq!(step_over_len(&[0xFE, 0x39, 0x08], false), None);
    }

    /// A machine at the prompt in deterministic mode, paused at the start
    /// as the front end has it, with PROBE.COM on C:.
    fn deterministic_machine(name: &str) -> (Cpu, DebugHub, Deterministic) {
        let mode = Deterministic::new(rust_dos::deterministic::default_start());
        let dir = std::env::temp_dir().join(format!("rust-dos-det-{}-{}", name, std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let code: Vec<u8> = (0..PROBE.len()).step_by(2).map(|i| u8::from_str_radix(&PROBE[i..i + 2], 16).unwrap()).collect();
        std::fs::write(dir.join("PROBE.COM"), code).unwrap();
        let mut cpu = Cpu::new(dir);
        cpu.bus.set_cycles_per_ms(3000);
        cpu.load_shell();
        let mut hub = DebugHub::for_tests(&cpu);
        hub.deterministic = Some(mode.start);
        hub.pause_at_start(&cpu);
        (cpu, hub, mode)
    }

    /// Run `frames` host frames of about `frame_ms` emulated milliseconds,
    /// as the main loop does in deterministic mode.
    fn run_deterministic(cpu: &mut Cpu, hub: &mut DebugHub, mode: &mut Deterministic, frames: usize, frame_ms: u64) {
        for _ in 0..frames {
            hub.poll(cpu);
            if hub.paused {
                continue;
            }
            let hot = hub.begin_batch(cpu);
            let target = cpu.bus.clock.icount_at_ns(cpu.bus.clock.now_ns() + frame_ms * 1_000_000);
            let end = mode.frame_end(cpu, target);
            hub.run_deterministic(cpu, mode, hot, end, |cpu, hub, reached| {
                if reached.tick {
                    hub.feed_input(cpu);
                }
            });
            hub.end_batch(cpu);
        }
    }

    fn typed(text: &str) -> Cmd {
        Cmd::Input { events: vec![InputEvent::Type { text: text.into(), delay_ms: None }], wait: false }
    }

    /// Type PROBE at the prompt and run to 2 s of emulated time, in host
    /// frames of `frame_ms`: the instructions executed, and memory.
    fn typed_run(name: &str, frame_ms: u64) -> (u64, Vec<u8>, Vec<Value>) {
        let (mut cpu, mut hub, mut mode) = deterministic_machine(name);
        reply(&mut cpu, &mut hub, typed("PROBE\n")).unwrap();
        let resumed = reply(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: Some(2000) }).unwrap();
        assert_eq!(resumed["paused"], false);
        run_deterministic(&mut cpu, &mut hub, &mut mode, 1000, frame_ms);
        assert!(hub.paused, "stopped at until_ms");
        assert_eq!(cpu.bus.clock.now_ns() / 1_000_000, 2000);
        assert_eq!(cpu.programs.started, 1, "PROBE ran");
        let stop = entries_of(&hub, "paused").pop().unwrap();
        // At the first instruction boundary past 2 s.
        assert_eq!((stop["reason"].as_str(), stop["emu_ns"].as_u64().map(|ns| ns / 1_000_000)), (Some("time"), Some(2000)));
        // The journal, but for the host's time.
        let journal = entries(&hub)
            .into_iter()
            .map(|mut e| {
                e.as_object_mut().unwrap().remove("t_ms");
                e
            })
            .collect();
        (cpu.executed, cpu.bus.ram().to_vec(), journal)
    }

    #[test]
    fn deterministic_input_gives_the_same_run_whatever_the_frames() {
        let a = typed_run("a", 16);
        let b = typed_run("b", 41);
        assert_eq!(a.0, b.0);
        assert!(a.1 == b.1, "the same memory");
        assert_eq!(a.2, b.2, "the same journal");
        assert!(a.2.iter().any(|e| e["type"] == "input_delivered") && a.2.iter().any(|e| e["type"] == "program_started"));
    }

    /// Carry out the save states asked for, in memory, as the
    /// deterministic front end does.
    fn carry_out_states(cpu: &mut Cpu, hub: &mut DebugHub, mode: &mut Deterministic, saved: &mut Option<Vec<u8>>) {
        while let Some(request) = hub.next_state_request() {
            let result = if request.load {
                rust_dos::savestate::machine::load(cpu, saved.as_ref().unwrap()).unwrap();
                mode.resync(cpu);
                Ok(serde_json::json!({"loaded": true}))
            } else {
                *saved = Some(rust_dos::savestate::machine::save(cpu));
                Ok(serde_json::json!({"saved": true}))
            };
            hub.state_done(cpu, request, result);
        }
    }

    /// A session in deterministic mode, in host frames whose emulated
    /// lengths come from `frames`: typed input with waits, a strict-match
    /// save and load, and a rewind. The instructions executed, memory and
    /// the journal but for the host's time.
    fn session(name: &str, frames: &[u64]) -> (u64, Vec<u8>, Vec<Value>) {
        let (mut cpu, mut hub, mut mode) = deterministic_machine(name);
        let mut saved = None;
        let mut next = frames.iter().copied().cycle();
        let mut go = |cpu: &mut Cpu, hub: &mut DebugHub, mode: &mut Deterministic, until_ms: u64| {
            reply(cpu, hub, Cmd::Resume { until: None, until_ms: Some(until_ms) }).unwrap();
            for _ in 0..100_000 {
                if hub.paused {
                    break;
                }
                run_deterministic(cpu, hub, mode, 1, next.next().unwrap());
            }
            assert!(hub.paused && cpu.bus.clock.now_ns() / 1_000_000 == until_ms, "{}: at {} ms", name, until_ms);
        };
        let wait = |ms| InputEvent::Wait { ms };
        let ty = |text: &str| InputEvent::Type { text: text.into(), delay_ms: Some(30) };
        reply(&mut cpu, &mut hub, Cmd::Input { events: vec![wait(120), ty("PROBE"), wait(45), ty("\n")], wait: false }).unwrap();
        go(&mut cpu, &mut hub, &mut mode, 900);
        let sent = send(&mut cpu, &mut hub, Cmd::SaveState { path: "s".into(), strict_match: true, check_files: false });
        carry_out_states(&mut cpu, &mut hub, &mut mode, &mut saved);
        answer(&mut cpu, &mut hub, sent, 0).unwrap();
        reply(&mut cpu, &mut hub, Cmd::Input { events: vec![ty("DIR"), wait(70), ty("\n")], wait: false }).unwrap();
        go(&mut cpu, &mut hub, &mut mode, 1600);
        let sent = send(&mut cpu, &mut hub, Cmd::LoadState { path: "s".into(), strict_match: true, check_files: false });
        carry_out_states(&mut cpu, &mut hub, &mut mode, &mut saved);
        answer(&mut cpu, &mut hub, sent, 0).unwrap();
        reply(&mut cpu, &mut hub, Cmd::Input { events: vec![ty("VER\n")], wait: false }).unwrap();
        go(&mut cpu, &mut hub, &mut mode, 1500);
        // A rewind: the front end loads a state and tells the hub after.
        rust_dos::savestate::machine::load(&mut cpu, saved.as_ref().unwrap()).unwrap();
        mode.resync(&cpu);
        hub.state_loaded_elsewhere(&cpu, "rewind");
        reply(&mut cpu, &mut hub, Cmd::Input { events: vec![ty("PROBE\n")], wait: false }).unwrap();
        go(&mut cpu, &mut hub, &mut mode, 2400);
        let journal = entries(&hub)
            .into_iter()
            .map(|mut e| {
                e.as_object_mut().unwrap().remove("t_ms");
                e
            })
            .collect();
        (cpu.executed, cpu.bus.ram().to_vec(), journal)
    }

    #[test]
    fn a_deterministic_session_with_loads_and_a_rewind_is_the_same_whatever_the_frames() {
        let a = session("session-a", &[16]);
        let b = session("session-b", &[41]);
        let c = session("session-c", &[3, 29, 1, 70, 12]);
        for other in [&b, &c] {
            assert_eq!(a.0, other.0, "the same instructions");
            assert!(a.1 == other.1, "the same memory");
            assert_eq!(a.2.len(), other.2.len(), "as many entries");
            for (x, y) in a.2.iter().zip(&other.2) {
                assert_eq!(x, y, "the same journal");
            }
        }
        let kinds: Vec<&str> = a.2.iter().map(|e| e["type"].as_str().unwrap()).collect();
        for kind in ["input_delivered", "state_saved", "state_loaded", "debugger_reset", "program_started"] {
            assert!(kinds.contains(&kind), "{} in {:?}", kind, kinds);
        }
        assert_eq!(a.2.iter().filter(|e| e["type"] == "state_loaded").count(), 2, "the load and the rewind");
    }

    #[test]
    fn deterministic_mode_takes_input_only_while_paused() {
        let (mut cpu, mut hub, _mode) = deterministic_machine("refuse");
        assert!(reply(&mut cpu, &mut hub, typed("x")).is_ok(), "paused at the start");
        let waited = Cmd::Input { events: vec![InputEvent::Type { text: "y".into(), delay_ms: None }], wait: true };
        let held = reply(&mut cpu, &mut hub, waited).unwrap();
        assert_eq!(held["held"], true, "answered at once while paused, not when delivered");
        reply(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: None }).unwrap();
        assert_eq!(reply(&mut cpu, &mut hub, typed("x")).unwrap_err().0, 409);
        let run = Cmd::Run { command: "PROBE.COM".into(), stop_at_entry: false };
        assert_eq!(reply(&mut cpu, &mut hub, run).unwrap_err().0, 409, "run while running");
        assert_eq!(reply(&mut cpu, &mut hub, write("0050:0000", &[1], None)).unwrap_err().0, 409, "a memory write while running");
        assert!(reply(&mut cpu, &mut hub, Cmd::GetRegs).is_ok(), "reads still work");
        let passed = reply(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: Some(0) }).unwrap_err();
        assert_eq!(passed.0, 400, "a time that has passed");
        let (mut cpu, mut hub) = (Cpu::new(".".into()), DebugHub::disabled());
        let refused = reply(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: Some(5) }).unwrap_err();
        assert_eq!(refused.0, 400, "until_ms needs deterministic mode");
    }

    /// The body of a 409 refusal, from the request's reply.
    fn refusal(sent: &mut Sent) -> Value {
        match sent.rx.try_recv().map(|a| a.reply) {
            Ok(Reply::ErrorJson(409, body)) => body,
            _ => panic!("not refused with 409"),
        }
    }

    #[test]
    fn deterministic_mode_loads_a_state_only_while_paused_and_refuses_as_a_strict_match_save_does() {
        let (mut cpu, mut hub, _mode) = deterministic_machine("state-refuse");
        reply(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: None }).unwrap();

        // A load while running is refused as it arrives, strict match or
        // not, with the body a refused strict-match save has.
        for strict_match in [false, true] {
            let mut load = send(&mut cpu, &mut hub, Cmd::LoadState { path: "/tmp/x.state".into(), strict_match, check_files: false });
            let body = refusal(&mut load);
            assert_eq!(body["paused"], false, "{}", body);
            assert_eq!(body["precondition"], "paused", "{}", body);
            assert!(body["error"].as_str().unwrap().contains("deterministic mode"), "{}", body);
        }
        // A strict-match save is refused by its own rule, in the same shape.
        let mut save = send(&mut cpu, &mut hub, Cmd::SaveState { path: "/tmp/x.state".into(), strict_match: true, check_files: false });
        assert!(take_state_requests(&mut hub).is_empty());
        let body = refusal(&mut save);
        assert_eq!(body["paused"], false, "{}", body);
        assert!(body["error"].as_str().unwrap().contains("pause it first"), "{}", body);
        // A plain save changes nothing and is taken while running.
        let _save = send(&mut cpu, &mut hub, Cmd::SaveState { path: "/tmp/x.state".into(), strict_match: false, check_files: false });
        assert_eq!(take_state_requests(&mut hub).len(), 1);

        // A load sent while paused, with a resume in the same frame: the
        // machine runs when the front end would load it, so it is refused.
        pause(&mut cpu, &mut hub);
        let mut load = send(&mut cpu, &mut hub, Cmd::LoadState { path: "/tmp/x.state".into(), strict_match: false, check_files: false });
        reply(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: None }).unwrap();
        assert!(take_state_requests(&mut hub).is_empty());
        assert_eq!(refusal(&mut load)["paused"], false);

        // Paused with nothing pending, both loads are taken.
        pause(&mut cpu, &mut hub);
        let _plain = send(&mut cpu, &mut hub, Cmd::LoadState { path: "/tmp/x.state".into(), strict_match: false, check_files: false });
        let _strict = send(&mut cpu, &mut hub, Cmd::LoadState { path: "/tmp/x.state".into(), strict_match: true, check_files: false });
        assert_eq!(take_state_requests(&mut hub).len(), 2);
    }

    #[test]
    fn a_strict_match_load_drops_a_pending_until_ms_and_input_waiting_on_emulated_time() {
        let (mut cpu, mut hub, mut mode) = deterministic_machine("reset-until");
        reply(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: Some(5000) }).unwrap();
        run_deterministic(&mut cpu, &mut hub, &mut mode, 10, 16);
        pause(&mut cpu, &mut hub);
        let status = reply(&mut cpu, &mut hub, Cmd::Status).unwrap();
        assert_eq!(status["deterministic"]["until_ms"], 5000, "still pending while paused");
        // Input typed on emulated time: a wait under way and keys after it.
        hub.input_wait_ns = Some(cpu.bus.clock.now_ns() + 500_000_000);
        reply(&mut cpu, &mut hub, typed("x")).unwrap();

        let (cleared, _) = hub.reset_after_load(&cpu);
        assert_eq!(cleared["until_ms"], true, "{}", cleared);
        assert!(cleared["input"].as_u64().unwrap() > 0, "{}", cleared);
        assert_eq!((hub.stop_at_ns, hub.input_wait_ns), (None, None));
        let status = reply(&mut cpu, &mut hub, Cmd::Status).unwrap();
        assert_eq!(status["deterministic"]["until_ms"], serde_json::Value::Null);
        assert!(!hub.begin_batch(&cpu), "nothing is left to stop the machine");

        // Run on past 5 s: no stop at the old time.
        reply(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: None }).unwrap();
        run_deterministic(&mut cpu, &mut hub, &mut mode, 700, 16);
        assert!(cpu.bus.clock.now_ns() > 5_000_000_000);
        assert!(!hub.paused, "the dropped until_ms didn't stop it");
    }

    // ----- the journal --------------------------------------------------------

    /// The answer to `cmd` sent with `If-State-Seq: if_state`: the reply's
    /// JSON or the error's status and body, and the journal's state.
    fn ask_guarded(cpu: &mut Cpu, hub: &mut DebugHub, cmd: Cmd, if_state: Option<u64>) -> (Result<Value, (u16, Value)>, super::Meta) {
        let (mut request, mut rx) = hub.request(cmd);
        request.if_state = if_state;
        hub.handle(cpu, request);
        let answer = rx.try_recv().expect("an answer at once");
        let result = match answer.reply {
            Reply::Json(v) => Ok(v),
            Reply::Error(code, e) => Err((code, serde_json::json!({"error": e}))),
            Reply::ErrorJson(code, v) => Err((code, v)),
            _ => panic!("not a JSON reply"),
        };
        (result, answer.meta)
    }

    /// The journal's state now.
    fn journal_state(hub: &DebugHub) -> (u64, Option<u64>, u64) {
        hub.with_journal(|j| (j.state_seq, j.stop_seq, j.last())).unwrap()
    }

    fn op_state(hub: &DebugHub, op: u64) -> super::journal::Op {
        hub.with_journal(|j| match j.op(op) {
            OpLookup::Found(op) => op.clone(),
            _ => panic!("no operation {}", op),
        })
        .unwrap()
    }

    /// The request number the answer to a request that started an
    /// operation names.
    fn started_op(cpu: &mut Cpu, hub: &mut DebugHub, cmd: Cmd) -> u64 {
        let (request, mut rx) = hub.request(cmd);
        let req = request.req;
        hub.handle(cpu, request);
        let answer = rx.try_recv().expect("an answer at once");
        assert!(matches!(answer.reply, Reply::Json(_)), "accepted");
        assert_eq!(answer.meta.op, Some(req));
        req
    }

    /// Stopped at PROBE.COM's first instruction: its code segment.
    fn at_probe_entry(cpu: &mut Cpu, hub: &mut DebugHub) -> String {
        let stop = ask(cpu, hub, Cmd::Run { command: "PROBE.COM".into(), stop_at_entry: true }, 50).unwrap();
        stop["registers"]["cs"].as_str().unwrap().to_string()
    }

    #[test]
    fn the_journal_numbers_its_entries_and_follows_the_state() {
        let (mut cpu, mut hub) = machine("journal-numbers");
        let cs = at_probe_entry(&mut cpu, &mut hub);
        ask(&mut cpu, &mut hub, write("0200:0000", &[1, 2], None), 0).unwrap();
        ask(&mut cpu, &mut hub, Cmd::AddBreakpoint { addr: format!("{}:0103", cs), once: false }, 0).unwrap();
        ask(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: None }, 0).unwrap();
        ask(&mut cpu, &mut hub, Cmd::WaitPause, 50).unwrap();
        let all = entries(&hub);
        assert_eq!(all[0]["type"], "started");
        assert_eq!(all[0]["seq"], 1);
        for (i, e) in all.iter().enumerate() {
            assert_eq!(e["seq"].as_u64(), Some(i as u64 + 1), "gapless: {}", e);
        }
        let changed = all.iter().rfind(|e| super::Kind::parse(e["type"].as_str().unwrap()).unwrap().changes_state());
        assert_eq!(journal_state(&hub).0, changed.unwrap()["seq"].as_u64().unwrap());
        // A breakpoint is the debugger's own: it changes nothing.
        let set = &entries_of(&hub, "breakpoint_set")[0];
        assert!(set["seq"].as_u64().unwrap() > entries_of(&hub, "memory_written")[0]["seq"].as_u64().unwrap());
    }

    #[test]
    fn a_pause_and_a_resume_in_one_drain_are_told_in_order() {
        let (mut cpu, mut hub) = machine("journal-order");
        let pause = hub.request(Cmd::Pause);
        let resume = hub.request(Cmd::Resume { until: None, until_ms: None });
        let resume_req = resume.0.req;
        hub.handle(&mut cpu, pause.0);
        hub.handle(&mut cpu, resume.0);
        let types: Vec<Value> = entries(&hub).iter().skip(1).map(|e| e["type"].clone()).collect();
        assert_eq!(types, ["paused", "resumed"]);
        assert_eq!(op_state(&hub, resume_req).state, OpState::Pending);
        // Another client pauses: the resume ends stopped, by that pause.
        let (request, _rx) = hub.request(Cmd::Pause);
        let pause_req = request.req;
        hub.handle(&mut cpu, request);
        let op = op_state(&hub, resume_req);
        assert_eq!(op.state, OpState::Stopped);
        let stop = &entries_of(&hub, "paused")[1];
        assert_eq!((stop["by"].as_u64(), stop["cause"].as_u64(), stop["op"].as_u64()), (Some(pause_req), Some(pause_req), Some(resume_req)));
        assert_eq!(op.end_seq, stop["seq"].as_u64());
    }

    #[test]
    fn a_stop_before_the_wait_is_found_after_the_resume() {
        let (mut cpu, mut hub) = machine("journal-early-stop");
        let cs = at_probe_entry(&mut cpu, &mut hub);
        ask(&mut cpu, &mut hub, Cmd::AddBreakpoint { addr: format!("{}:0103", cs), once: false }, 0).unwrap();
        let op = started_op(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: None });
        let resumed = entries_of(&hub, "resumed").last().unwrap()["seq"].as_u64().unwrap();
        // The machine stops before anyone asks.
        run(&mut cpu, &mut hub, 5);
        assert!(hub.paused);
        let stop = stop_after(&hub, resumed).expect("the stop is in the journal");
        assert_eq!((stop["reason"].as_str(), stop["op"].as_u64()), (Some("breakpoint"), Some(op)));
        assert_eq!(op_reply(&hub, op).unwrap().unwrap()["seq"], stop["seq"]);
    }

    #[test]
    fn a_breakpoint_once_goes_once_it_stops_the_machine() {
        let (mut cpu, mut hub) = machine("journal-once");
        let cs = at_probe_entry(&mut cpu, &mut hub);
        let at = format!("{}:0103", cs);
        let listed = ask(&mut cpu, &mut hub, Cmd::AddBreakpoint { addr: at.clone(), once: true }, 0).unwrap();
        assert_eq!(listed["once"].as_array().map(Vec::len), Some(1));
        ask(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: None }, 0).unwrap();
        run(&mut cpu, &mut hub, 5);
        // A wait that comes late, and the journal, show the same stop.
        let late = ask(&mut cpu, &mut hub, Cmd::WaitPause, 0).unwrap();
        let stop = entries_of(&hub, "paused").pop().unwrap();
        for e in [&late, &stop] {
            assert_eq!((e["reason"].as_str(), e["once"].as_bool()), (Some("breakpoint"), Some(true)), "{}", e);
            assert_eq!(e["breakpoint"], stop["breakpoint"]);
        }
        assert_eq!(late["stop"], stop);
        let listed = ask(&mut cpu, &mut hub, Cmd::ListBreakpoints, 0).unwrap();
        assert_eq!(listed["breakpoints"].as_array().map(Vec::len), Some(0), "{}", listed);

        // A run to the same address names no breakpoint.
        ask(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: None }, 0).unwrap();
        run(&mut cpu, &mut hub, 20);
        let cs = at_probe_entry(&mut cpu, &mut hub);
        ask(&mut cpu, &mut hub, Cmd::Resume { until: Some(format!("{}:0103", cs)), until_ms: None }, 0).unwrap();
        run(&mut cpu, &mut hub, 5);
        let stop = entries_of(&hub, "paused").pop().unwrap();
        assert!(stop.get("breakpoint").is_none() && stop["target"].is_string(), "{}", stop);
    }

    #[test]
    fn a_step_another_resume_takes_over_is_superseded() {
        let (mut cpu, mut hub) = machine("journal-supersede");
        let cs = at_probe_entry(&mut cpu, &mut hub);
        ask(&mut cpu, &mut hub, Cmd::AddBreakpoint { addr: format!("{}:0106", cs), once: false }, 0).unwrap();
        let step = started_op(&mut cpu, &mut hub, Cmd::Step { count: 1 });
        let resume = started_op(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: None });
        let op = op_state(&hub, step);
        assert_eq!((op.state, op.superseded_by), (OpState::Superseded, Some(resume)));
        assert_eq!(entries_of(&hub, "resumed").last().unwrap()["supersedes"].as_u64(), Some(step));
        run(&mut cpu, &mut hub, 5);
        // Where the step's chain ends: the resume's stop.
        let stop = op_reply(&hub, resume).unwrap().unwrap();
        assert_eq!((stop["reason"].as_str(), stop["op"].as_u64()), (Some("breakpoint"), Some(resume)));
    }

    #[test]
    fn a_run_another_resume_takes_over_does_not_stop_at_entry() {
        let (mut cpu, mut hub) = machine("journal-supersede-run");
        let run_op = started_op(&mut cpu, &mut hub, Cmd::Run { command: "PROBE.COM".into(), stop_at_entry: true });
        let resume = started_op(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: None });
        assert_eq!(op_state(&hub, run_op).superseded_by, Some(resume));
        assert!(hub.run_wait.is_none() && !hub.start_once, "the run's stop at entry is dropped");
        run(&mut cpu, &mut hub, 50);
        assert!(!hub.paused, "the program ran through its entry");
        assert!(entries_of(&hub, "paused").is_empty(), "{:?}", entries_of(&hub, "paused"));
        assert_eq!(op_state(&hub, run_op).state, OpState::Superseded);
        assert!(entries_of(&hub, "program_started").iter().any(|e| e["program"]["name"] == "PROBE.COM"), "the command line still ran");
    }

    #[test]
    fn if_state_seq_holds_only_in_the_state_read() {
        let (mut cpu, mut hub) = machine("journal-if-state");
        pause(&mut cpu, &mut hub);
        let read = journal_state(&hub).0;
        let (done, meta) = ask_guarded(&mut cpu, &mut hub, write("0200:0000", &[1], None), Some(read));
        done.unwrap();
        assert!(meta.state_seq > read, "the write moves the state");
        // Another client wrote since.
        let mine = meta.state_seq;
        ask(&mut cpu, &mut hub, write("0200:0000", &[2], None), 0).unwrap();
        let (refused, _) = ask_guarded(&mut cpu, &mut hub, write("0200:0000", &[3], None), Some(mine));
        let (code, body) = refused.unwrap_err();
        assert_eq!((code, body["precondition"].as_str()), (409, Some("state")), "{}", body);
        assert_eq!(body["state_seq"].as_u64(), Some(journal_state(&hub).0));
        assert_eq!(cpu.bus.peek_8(0x2000), 2, "nothing was written");

        // A resume and a stop since.
        let read = journal_state(&hub).0;
        ask(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: None }, 0).unwrap();
        let (refused, _) = ask_guarded(&mut cpu, &mut hub, write("0200:0000", &[3], None), Some(read));
        assert_eq!(refused.unwrap_err().1["paused"], false, "refused while running");
        pause(&mut cpu, &mut hub);
        let (refused, _) = ask_guarded(&mut cpu, &mut hub, write("0200:0000", &[3], None), Some(read));
        assert_eq!(refused.unwrap_err().0, 409, "refused after the stop");

        // Input being typed while paused, and the settings window open.
        let read = journal_state(&hub).0;
        ask(&mut cpu, &mut hub, Cmd::Input { events: vec![InputEvent::Wait { ms: 60_000 }], wait: false }, 0).unwrap();
        let (refused, _) = ask_guarded(&mut cpu, &mut hub, Cmd::GetRegs, Some(read));
        assert_eq!(refused.unwrap_err().1["input_queued"], 1);
        ask(&mut cpu, &mut hub, Cmd::InputClear, 0).unwrap();
        let read = journal_state(&hub).0;
        hub.divert = true;
        let (refused, _) = ask_guarded(&mut cpu, &mut hub, Cmd::GetRegs, Some(read));
        assert_eq!(refused.unwrap_err().1["settings_window"], true);
        hub.divert = false;
        assert!(ask_guarded(&mut cpu, &mut hub, Cmd::GetRegs, Some(read)).0.is_ok());
        // Rewinding loads states the journal hears about only at its end.
        hub.rewinding = true;
        let (refused, _) = ask_guarded(&mut cpu, &mut hub, Cmd::GetRegs, Some(read));
        assert_eq!(refused.unwrap_err().1["error"], "the machine is being rewound");
        hub.rewinding = false;
        assert!(ask_guarded(&mut cpu, &mut hub, Cmd::GetRegs, Some(read)).0.is_ok());

        // Deterministic mode types nothing while paused: queued input is no
        // obstacle there.
        let (mut cpu, mut hub, _mode) = deterministic_machine("journal-if-state-det");
        reply(&mut cpu, &mut hub, typed("x")).unwrap();
        let read = journal_state(&hub).0;
        assert!(ask_guarded(&mut cpu, &mut hub, write("0200:0000", &[1], None), Some(read)).0.is_ok());
    }

    #[test]
    fn a_strict_match_load_cancels_the_operations_under_way() {
        let (mut cpu, mut hub) = machine("journal-strict");
        pause(&mut cpu, &mut hub);
        hub.input.push_back(LowInput::Wait(std::time::Duration::from_secs(60)));
        let (_, refused) = state_request(&mut cpu, &mut hub, true);
        assert_eq!(refused.unwrap().1["precondition"], "quiescent", "a save waits for the input");
        hub.input.clear();

        let input = started_op(&mut cpu, &mut hub, Cmd::Input { events: vec![InputEvent::Wait { ms: 60_000 }], wait: true });
        let (load, _rx) = hub.request(Cmd::LoadState { path: "/tmp/x.state".into(), strict_match: true, check_files: false });
        let load_req = load.req;
        hub.handle(&mut cpu, load);
        let request = hub.next_state_request().expect("taken");
        hub.state_done(&cpu, request, Ok(serde_json::json!({"loaded": true})));
        let tail: Vec<Value> = entries(&hub).into_iter().rev().take(3).collect();
        assert_eq!(tail.iter().rev().map(|e| e["type"].as_str().unwrap()).collect::<Vec<_>>(), ["state_loaded", "debugger_reset", "paused"]);
        assert_eq!(tail[1]["cancelled"], serde_json::json!([input]));
        assert_eq!((tail[0]["reason"].as_str(), tail[0]["cause"].as_u64()), (Some("load"), Some(load_req)));
        assert_eq!(op_state(&hub, input).state, OpState::Cancelled);
        assert_eq!(op_state(&hub, input).end_seq, tail[1]["seq"].as_u64());
    }

    #[test]
    fn eviction_tells_what_is_gone() {
        let cpu = Cpu::new(".".into());
        let shared = std::sync::Arc::new(super::Shared::with_capacity(std::time::Instant::now(), 8));
        let mut hub = DebugHub::new(None, Some(shared), 10);
        hub.announce(&cpu);
        for i in 0..20 {
            hub.append(&cpu, super::Kind::BreakpointSet, None, serde_json::json!({"i": i}));
        }
        hub.with_journal(|j| {
            assert_eq!((j.first(), j.last()), (14, 21));
            assert_eq!(j.after(5).err(), Some(super::journal::Gone { first: 14, last: 21 }));
            assert_eq!(j.after(13).map(|it| it.count()).ok(), Some(8));
        });
    }

    #[test]
    fn a_pending_operation_outlives_any_number_of_others() {
        let (mut cpu, mut hub) = machine("journal-pending");
        pause(&mut cpu, &mut hub);
        // Input queued with a client's ID, which nothing delivers while
        // only requests are carried out.
        let (mut request, _rx) = hub.request(typed("x"));
        let input = request.req;
        request.tag = Some(std::sync::Arc::from("typed-1"));
        hub.handle(&mut cpu, request);
        // More operations than the journal keeps, each ended at once.
        for _ in 0..super::journal::OPS_CAPACITY + 100 {
            started_op(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: None });
            ask(&mut cpu, &mut hub, Cmd::Pause, 0).unwrap();
        }
        assert_eq!(op_state(&hub, input).state, OpState::Pending, "still there");
        assert!(hub.with_journal(|j| j.ops_len()).unwrap() <= super::journal::OPS_CAPACITY + 1);
        // Delivered now, its entry has the request's ID.
        run(&mut cpu, &mut hub, 20);
        assert_eq!(op_state(&hub, input).state, OpState::Done);
        let delivered = entries_of(&hub, "input_delivered").pop().unwrap();
        assert_eq!((delivered["cause"].as_u64(), delivered["request_id"].as_str()), (Some(input), Some("typed-1")));
    }

    #[test]
    fn input_past_the_pending_limit_is_refused() {
        let (mut cpu, mut hub) = machine("journal-pending-limit");
        pause(&mut cpu, &mut hub);
        for _ in 0..super::journal::PENDING_INPUT_CAPACITY {
            ask(&mut cpu, &mut hub, typed("x"), 0).unwrap();
        }
        let refused = reply(&mut cpu, &mut hub, typed("x")).unwrap_err();
        assert_eq!(refused.0, 429, "{}", refused.1);
        // Once some are delivered, input is taken again.
        ask(&mut cpu, &mut hub, Cmd::InputClear, 0).unwrap();
        assert!(reply(&mut cpu, &mut hub, typed("x")).is_ok());
    }

    #[test]
    fn a_write_after_a_load_in_the_same_frame_comes_after_it() {
        let mut cpu = Cpu::new(".".into());
        let (tx, rx) = std::sync::mpsc::channel();
        let shared = std::sync::Arc::new(super::Shared::new(std::time::Instant::now()));
        let mut hub = DebugHub::new(Some(rx), Some(shared), 10);
        hub.announce(&cpu);
        let (load, _load_rx) = hub.request(Cmd::LoadState { path: "/tmp/x.state".into(), strict_match: false, check_files: false });
        let (write, mut write_rx) = hub.request(write("0200:0000", &[9], None));
        tx.send(load).unwrap();
        tx.send(write).unwrap();
        hub.poll(&mut cpu);
        assert!(write_rx.try_recv().is_err(), "the write waits for the load");
        let request = hub.next_state_request().unwrap();
        hub.state_done(&cpu, request, Ok(serde_json::json!({"loaded": true})));
        hub.poll(&mut cpu);
        assert!(write_rx.try_recv().is_ok());
        let types: Vec<Value> = entries(&hub).iter().skip(1).map(|e| e["type"].clone()).collect();
        assert_eq!(types, ["state_loaded", "memory_written"]);
    }

    #[test]
    fn programs_are_told_without_a_stop_on_them() {
        let (mut cpu, mut hub) = machine("journal-programs");
        cpu.queue_batch_lines(["PROBE.COM"]);
        run(&mut cpu, &mut hub, 50);
        let started = entries_of(&hub, "program_started");
        let exited = entries_of(&hub, "program_exited");
        assert_eq!(started.last().unwrap()["program"]["name"], "PROBE.COM");
        assert_eq!(exited.last().unwrap()["exit"]["name"], "PROBE.COM");
        assert!(started.last().unwrap()["seq"].as_u64() < exited.last().unwrap()["seq"].as_u64());
        assert!(entries_of(&hub, "paused").is_empty());
    }

    #[test]
    fn a_quit_cancels_what_is_pending_and_ends_the_journal() {
        let mut cpu = Cpu::new(".".into());
        cpu.load_shell();
        let (tx, rx) = std::sync::mpsc::channel();
        let shared = std::sync::Arc::new(super::Shared::new(std::time::Instant::now()));
        let mut hub = DebugHub::new(Some(rx), Some(shared.clone()), 10);
        hub.announce(&cpu);
        let resume = started_op(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: None });
        let (quit, mut quit_rx) = hub.request(Cmd::Quit);
        let quit_req = quit.req;
        let (late, mut late_rx) = hub.request(write("0200:0000", &[9], None));
        tx.send(quit).unwrap();
        tx.send(late).unwrap();
        hub.poll(&mut cpu);
        assert!(hub.quit_requested());
        assert!(matches!(quit_rx.try_recv().map(|a| a.reply), Ok(Reply::Json(_))));
        assert!(late_rx.try_recv().is_err(), "nothing is carried out after a quit");
        let (refused, _) = ask_guarded(&mut cpu, &mut hub, Cmd::GetRegs, None);
        assert_eq!(refused.unwrap_err().0, 503);

        hub.shutdown(&cpu, super::ShutdownReason::Quit, 0);
        assert!(matches!(late_rx.try_recv().map(|a| a.reply), Ok(Reply::Error(503, _))));
        assert_ne!(cpu.bus.peek_8(0x2000), 9);
        let last = entries(&hub).pop().unwrap();
        assert_eq!((last["type"].as_str(), last["reason"].as_str()), (Some("shutdown"), Some("quit")));
        assert_eq!((last["by"].as_u64(), last["cause"].as_u64()), (Some(quit_req), Some(quit_req)));
        assert_eq!(last["cancelled"], serde_json::json!([resume]));
        assert_eq!(op_state(&hub, resume).state, OpState::Cancelled);
        assert!(shared.closed.load(std::sync::atomic::Ordering::Acquire));
        hub.append(&cpu, super::Kind::BreakpointSet, None, serde_json::json!({}));
        assert_eq!(entries(&hub).pop().unwrap()["type"], "shutdown", "nothing after it");
        // A request that passed the server's check just before the shutdown
        // isn't left waiting: the channel is closed.
        let (after, mut after_rx) = hub.request(Cmd::GetRegs);
        assert!(tx.send(after).is_err(), "nobody takes requests any more");
        assert!(matches!(after_rx.try_recv(), Err(oneshot::error::TryRecvError::Closed)));
    }

    #[test]
    fn requests_sent_together_take_effect_in_the_order_sent() {
        let mut cpu = Cpu::new(".".into());
        cpu.load_shell();
        let (tx, rx) = std::sync::mpsc::channel();
        let shared = std::sync::Arc::new(super::Shared::new(std::time::Instant::now()));
        let mut hub = DebugHub::new(Some(rx), Some(shared), 10);
        hub.announce(&cpu);
        let (p, _p) = hub.request(Cmd::Pause);
        tx.send(p).unwrap();
        hub.poll(&mut cpu);
        assert!(hub.paused);
        let before = journal_state(&hub).2;
        let state = journal_state(&hub).0;
        // All in the channel at once, as from clients in one frame.
        let mut cmds = vec![
            (write("0200:0000", &[1], None), None),
            (Cmd::SaveState { path: "s".into(), strict_match: true, check_files: false }, None),
            (write("0200:0000", &[2], None), None),
            (Cmd::Speed { cycles: "5000".into() }, None),
            // Guarded by the state before the first write: refused.
            (write("0200:0000", &[3], None), Some(state)),
            (Cmd::LoadState { path: "s".into(), strict_match: true, check_files: false }, None),
            // Holds only once the load brought back the 1.
            (write("0200:0000", &[4], Some(&[1])), None),
            (Cmd::Resume { until: None, until_ms: None }, None),
            (Cmd::Pause, None),
        ];
        let mut answers = Vec::new();
        for (cmd, if_state) in cmds.drain(..) {
            let (mut request, rx) = hub.request(cmd);
            request.if_state = if_state;
            tx.send(request).unwrap();
            answers.push(rx);
        }
        let mut saved: Option<Vec<u8>> = None;
        for _ in 0..10 {
            hub.poll(&mut cpu);
            while let Some(request) = hub.next_state_request() {
                let result = if request.load {
                    rust_dos::savestate::machine::load(&mut cpu, saved.as_ref().unwrap()).unwrap();
                    Ok(serde_json::json!({"loaded": true}))
                } else {
                    saved = Some(rust_dos::savestate::machine::save(&cpu));
                    Ok(serde_json::json!({"saved": true}))
                };
                hub.state_done(&cpu, request, result);
            }
            while let Some(request) = hub.next_speed_request() {
                let cycles = request.cycles.clone();
                hub.speed_done(&cpu, request, Ok(serde_json::json!({"cycles": cycles})));
            }
        }
        let statuses: Vec<u16> = answers
            .iter_mut()
            .map(|rx| match rx.try_recv().expect("answered").reply {
                Reply::Json(_) => 200,
                Reply::Error(code, _) | Reply::ErrorJson(code, _) => code,
                _ => panic!("not a JSON reply"),
            })
            .collect();
        assert_eq!(statuses, [200, 200, 200, 200, 409, 200, 200, 200, 200]);
        let types: Vec<String> = entries(&hub).iter().filter(|e| e["seq"].as_u64() > Some(before)).map(|e| e["type"].as_str().unwrap().to_string()).collect();
        assert_eq!(
            types,
            ["memory_written", "state_saved", "memory_written", "speed_changed", "state_loaded", "debugger_reset", "paused", "memory_written", "resumed", "paused"]
        );
        assert_eq!(cpu.bus.peek_8(0x2000), 4);
    }

    /// A program that stops the machine every way a program can: it writes
    /// memory, calls, switches to protected mode and back, then divides by
    /// zero.
    const STOPS: [u8; 34] = [
        0xC7, 0x06, 0x00, 0x02, 0x34, 0x12, // 0100 mov word [0200], 1234
        0xB8, 0x01, 0x00, // 0106 mov ax, 1
        0xE8, 0x04, 0x00, // 0109 call 0110
        0x90, // 010C nop
        0xEB, 0x02, // 010D jmp 0111
        0x90, // 010F
        0xC3, // 0110 ret
        0x0F, 0x20, 0xC0, // 0111 mov eax, cr0
        0x0C, 0x01, // 0114 or al, 1
        0x0F, 0x22, 0xC0, // 0116 mov cr0, eax
        0x24, 0xFE, // 0119 and al, FE
        0x0F, 0x22, 0xC0, // 011B mov cr0, eax
        0x31, 0xC0, 0xF6, 0xF0, // 011E xor ax, ax; div al
    ];

    /// Start an operation and run until the machine stops: the stop's
    /// entry, checked against the operation it ended.
    fn stopped_by(cpu: &mut Cpu, hub: &mut DebugHub, cmd: Cmd, reason: &str) -> Value {
        let op = started_op(cpu, hub, cmd);
        for _ in 0..200 {
            if hub.paused {
                break;
            }
            run(cpu, hub, 1);
        }
        assert!(hub.paused, "stopped: {}", reason);
        let stop = entries_of(hub, "paused").pop().unwrap();
        assert_eq!(stop["reason"].as_str(), Some(reason), "{}", stop);
        assert_eq!((stop["op"].as_u64(), stop["cause"].as_u64()), (Some(op), Some(op)), "{}", stop);
        let ended = op_state(hub, op);
        assert_eq!((ended.state, ended.end_seq), (OpState::Stopped, stop["seq"].as_u64()), "{}", stop);
        let (state_seq, stop_seq, _) = journal_state(hub);
        assert_eq!((Some(state_seq), stop_seq), (stop["seq"].as_u64(), stop["seq"].as_u64()), "the state is the stop's");
        stop
    }

    #[test]
    fn every_stop_names_its_reason_and_ends_its_operation() {
        let (mut cpu, mut hub) = machine("journal-stops");
        let dir = std::env::temp_dir().join(format!("rust-dos-debug-journal-stops-{}", std::process::id()));
        std::fs::write(dir.join("STOPS.COM"), STOPS).unwrap();
        let entry = stopped_by(&mut cpu, &mut hub, Cmd::Run { command: "STOPS.COM".into(), stop_at_entry: true }, "program_start");
        let cs = entry["registers"]["cs"].as_str().unwrap().to_string();
        let at = |stop: &Value| stop["registers"]["ip"].as_str().unwrap().to_string();

        ask(&mut cpu, &mut hub, Cmd::AddWatchpoint { addr: format!("{}:0200", cs), len: 2 }, 0).unwrap();
        let resume = || Cmd::Resume { until: None, until_ms: None };
        let stop = stopped_by(&mut cpu, &mut hub, resume(), "watchpoint");
        assert_eq!(at(&stop), "0106");
        let stop = stopped_by(&mut cpu, &mut hub, Cmd::Step { count: 1 }, "step");
        assert_eq!(at(&stop), "0109");
        // A call is stepped over by running to the instruction after it.
        let stop = stopped_by(&mut cpu, &mut hub, Cmd::StepOver, "breakpoint");
        assert_eq!((at(&stop).as_str(), stop["target"].is_string()), ("010C", true), "the call ran through");
        ask(&mut cpu, &mut hub, Cmd::AddBreakpoint { addr: format!("{}:010D", cs), once: true }, 0).unwrap();
        let stop = stopped_by(&mut cpu, &mut hub, resume(), "breakpoint");
        assert_eq!((at(&stop).as_str(), stop["once"].as_bool()), ("010D", Some(true)));
        let stop = stopped_by(&mut cpu, &mut hub, Cmd::Resume { until: Some(format!("{}:0114", cs)), until_ms: None }, "breakpoint");
        assert_eq!(at(&stop), "0114");
        let on = |mode_switch, exceptions, program_exit| Cmd::BreakOn { exceptions, clear_exceptions: None, mode_switch, program_start: None, program_exit };
        ask(&mut cpu, &mut hub, on(Some(true), None, None), 0).unwrap();
        stopped_by(&mut cpu, &mut hub, resume(), "mode_switch");
        ask(&mut cpu, &mut hub, on(Some(false), Some(1), None), 0).unwrap();
        stopped_by(&mut cpu, &mut hub, resume(), "exception");
        ask(&mut cpu, &mut hub, Cmd::BreakOn { exceptions: None, clear_exceptions: Some(1), mode_switch: None, program_start: None, program_exit: Some(true) }, 0).unwrap();
        let stop = stopped_by(&mut cpu, &mut hub, resume(), "program_exit");
        assert_eq!(stop["exit"]["name"], "STOPS.COM");

        // A pause: by the request, ending the resume it stops.
        let resumed = started_op(&mut cpu, &mut hub, resume());
        run(&mut cpu, &mut hub, 2);
        let (pause, _rx) = hub.request(Cmd::Pause);
        let by = pause.req;
        hub.handle(&mut cpu, pause);
        hub.end_batch(&cpu);
        let stop = entries_of(&hub, "paused").pop().unwrap();
        assert_eq!(stop["reason"], "request");
        assert_eq!((stop["by"].as_u64(), stop["cause"].as_u64(), stop["op"].as_u64()), (Some(by), Some(by), Some(resumed)), "{}", stop);
        assert_eq!((op_state(&hub, resumed).state, op_state(&hub, resumed).end_seq), (OpState::Stopped, stop["seq"].as_u64()));

        // Deterministic mode starts stopped, by nothing.
        let (_cpu, hub, _mode) = deterministic_machine("journal-startup");
        let stop = entries_of(&hub, "paused").pop().unwrap();
        assert_eq!((stop["reason"].as_str(), &stop["op"], &stop["cause"]), (Some("startup"), &Value::Null, &Value::Null), "{}", stop);
    }

    #[test]
    fn a_run_whose_operation_a_stop_ended_waits_no_more() {
        let (mut cpu, mut hub) = machine("journal-run-stopped");
        let dir = std::env::temp_dir().join(format!("rust-dos-debug-journal-run-stopped-{}", std::process::id()));
        std::fs::write(dir.join("HOLD.BAT"), "@pause\r\nPROBE.COM\r\n").unwrap();
        let op = started_op(&mut cpu, &mut hub, Cmd::Run { command: "HOLD.BAT".into(), stop_at_entry: true });
        run(&mut cpu, &mut hub, 20);
        assert!(hub.run_wait.is_some(), "waiting at PAUSE");
        pause(&mut cpu, &mut hub);
        assert_eq!(op_state(&hub, op).state, OpState::Stopped);
        assert!(hub.run_wait.is_none() && !hub.start_once, "the stop ended the run's wait");
        let (taken, refused) = state_request(&mut cpu, &mut hub, true);
        assert_eq!((taken, refused.map(|r| r.0)), (1, None), "a strict-match save goes ahead");
        // Its program starts with nobody stopping it at its entry.
        ask(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: None }, 0).unwrap();
        ask(&mut cpu, &mut hub, Cmd::Input { events: vec![InputEvent::Type { text: " ".into(), delay_ms: None }], wait: false }, 0).unwrap();
        run(&mut cpu, &mut hub, 100);
        assert!(cpu.programs.started > 0 && entries_of(&hub, "paused").iter().all(|e| e["reason"] != "program_start"));
    }

    #[test]
    fn a_superseded_resume_s_time_stops_nothing() {
        let (mut cpu, mut hub, mut mode) = deterministic_machine("superseded-time");
        reply(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: Some(300) }).unwrap();
        run_deterministic(&mut cpu, &mut hub, &mut mode, 2, 16);
        // A step takes over, then a plain resume runs past 300 ms.
        let step = send(&mut cpu, &mut hub, Cmd::Step { count: 1 });
        answer(&mut cpu, &mut hub, step, 5).unwrap();
        reply(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: None }).unwrap();
        run_deterministic(&mut cpu, &mut hub, &mut mode, 40, 16);
        assert!(cpu.bus.clock.now_ns() > 400_000_000);
        assert!(!hub.paused, "{:?}", entries_of(&hub, "paused").pop());
    }

    #[test]
    fn log_lines_name_their_request_and_the_journal_s_entry() {
        let (mut cpu, mut hub) = machine("journal-log");
        let shared = hub.shared.clone().unwrap();
        let hook = shared.clone();
        cpu.bus.log_hook = Some(Box::new(move |line: &str| hook.log_line(line)));
        let dir = std::env::temp_dir().join(format!("rust-dos-debug-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let before = journal_state(&hub).2;
        let mount = Cmd::Mount { drive: "D".into(), path: dir.display().to_string(), kind: None, label: None, read_only: true, images: Vec::new() };
        let (request, _rx) = hub.request(mount);
        let req = request.req;
        hub.handle(&mut cpu, request);
        let log = shared.log.lock().unwrap();
        let line = log.iter().find(|l| l.line.contains("mounted")).expect("the mount's line");
        assert_eq!((line.cause, line.at), (Some(req), before));
        assert!(log.iter().zip(log.iter().skip(1)).all(|(a, b)| b.n == a.n + 1), "numbered in order");
        assert_eq!(entries_of(&hub, "drive_mounted")[0]["cause"].as_u64(), Some(req));
    }
}

/// A ViRGE's engines for `/api/status`: the commands they ran, the last 3D
/// command, the status and the overlay.
fn virge_status(v: &crate::video::s3::virge::Virge, vx: bool) -> serde_json::Value {
    json!({
        "bitblts": v.counts[0], "rects": v.counts[1], "lines": v.counts[2], "polygons": v.counts[3],
        "lines_3d": v.s3d.lines, "triangles": v.s3d.triangles,
        "cmd_3d": format!("{:08X}", v.s3d.tri[0x40]),
        "status": format!("{:04X}", v.status()), "advfunc": format!("{:02X}", v.advfunc),
        "transfer": v.transfer.active, "command_dma": v.dma.enable != 0,
        "overlay": v.streams.overlay(vx).map(|o| format!("{:?}", o)),
    })
}
