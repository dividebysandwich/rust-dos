//! Remote debug / control interface.
//!
//! An HTTP + WebSocket server (see `server.rs`) runs on its own thread with
//! a private tokio runtime. It never touches the emulator directly: requests
//! are sent to the main thread through an mpsc channel and executed by
//! `DebugHub::poll` once per frame, and data flows back through oneshot
//! replies, broadcast channels, and a shared frame snapshot.

pub mod keys;
pub mod pm;
pub mod server;
pub mod trace;

use std::collections::{HashSet, VecDeque};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::sync::{broadcast, oneshot};

use crate::config_ui::UiKey;
use crate::cpu::{Cpu, CpuFlags, CpuState};
use crate::disk::{DriveKind, MountOptions, drive_letter};
use crate::keyboard;
use rust_dos::keylayout::Layout;
use crate::mount::{display_host_path, parse_drive_letter, parse_kind};
use crate::video::vbe::Vbe;
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
    /// JSON event stream: log lines, pause/resume, breakpoints, mode changes.
    pub events: broadcast::Sender<Arc<str>>,
    /// JSON trace batches (one per frame while subscribed).
    pub trace: broadcast::Sender<Arc<str>>,
    /// Mixed audio, 44.1 kHz stereo s16, interleaved.
    pub audio: broadcast::Sender<Arc<[i16]>>,
    pub log: Mutex<VecDeque<LogLine>>,
    pub start_time: Instant,
}

#[derive(Clone, serde::Serialize)]
pub struct LogLine {
    pub t_ms: u64,
    pub line: String,
}

impl Shared {
    fn emit(&self, v: Value) {
        if self.events.receiver_count() > 0 {
            let _ = self.events.send(Arc::from(v.to_string()));
        }
    }
}

pub enum Reply {
    Json(Value),
    Frame(video::Frame),
    Trace(Vec<TraceEntry>),
    Bytes { addr: usize, segoff: Option<(u16, u32)>, data: Vec<u8> },
    Error(u16, String),
}

impl Reply {
    fn bad(msg: impl Into<String>) -> Self {
        Reply::Error(400, msg.into())
    }
}

pub struct Request {
    pub cmd: Cmd,
    pub reply: oneshot::Sender<Reply>,
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
}

pub enum Cmd {
    Status,
    Stats,
    Screenshot,
    ScreenText,
    TraceQuery(TraceQuery),
    TraceControl { enabled: Option<bool>, clear: bool, stream_max: Option<usize> },
    Input { events: Vec<InputEvent>, wait: bool },
    InputClear,
    Pause,
    Resume { until: Option<String> },
    Step { count: u64 },
    /// Step, but run a call, interrupt, loop or repeated string instruction
    /// through to the instruction after it.
    StepOver,
    WaitPause,
    RebootShell,
    GetRegs,
    SetRegs(Map<String, Value>),
    ReadMem { addr: String, len: usize },
    WriteMem { addr: String, data: Vec<u8> },
    Disasm { addr: Option<String>, count: usize },
    ListBreakpoints,
    AddBreakpoint(String),
    RemoveBreakpoint(Option<String>),
    ListWatchpoints,
    /// Pause when the 1, 2 or 4 bytes at an address change.
    AddWatchpoint { addr: String, len: u8 },
    RemoveWatchpoint(Option<String>),
    /// Pause after the CPU raises one of these exceptions (bit n = vector
    /// n), or switches between real and protected mode.
    BreakOn { exceptions: Option<u32>, clear_exceptions: Option<u32>, mode_switch: Option<bool> },
    Ivt,
    Gdt,
    Ldt,
    Idt,
    Tss,
    PageWalk(String),
    Xms,
    Exceptions,
    Gus,
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
    /// Save the machine to a save state file, or load one, which the front
    /// end does (`take_state_requests`).
    SaveState { path: String },
    LoadState { path: String },
}

/// A save state to save or load, which the front end carries out and
/// answers (`done`).
pub struct StateRequest {
    pub load: bool,
    pub path: PathBuf,
    reply: oneshot::Sender<Reply>,
}

impl StateRequest {
    /// Answer the request: what was saved or loaded, or why not.
    pub fn done(self, result: Result<Value, String>) {
        let _ = self.reply.send(match result {
            Ok(value) => Reply::Json(value),
            Err(e) => Reply::bad(e),
        });
    }
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
enum LowInput {
    KeyDown { key: PcKey, ascii: u8 },
    /// A character no key types, as Alt and the keypad type it.
    Char(u8),
    KeyUp { key: PcKey },
    MouseTo { x: i32, y: i32, coords: Coords },
    MouseRel { dx: i32, dy: i32 },
    Button { idx: usize, down: bool },
    Wait(Duration),
    Notify(oneshot::Sender<Reply>),
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

/// The keys that type `c` in `layout`: its key with Shift and AltGr as it
/// needs them. A character the
/// layout has on no key (or only as a dead key's accent) is typed as the
/// US keyboard types it, or else as Alt and the keypad type any.
fn keys_for_char(c: char, layout: &Layout, out: &mut Vec<LowInput>) -> Result<(), String> {
    let special = match c {
        '\n' | '\r' => Some("enter"),
        '\t' => Some("tab"),
        '\x08' => Some("backspace"),
        '\x1b' => Some("escape"),
        _ => None,
    };
    if let Some(name) = special {
        let key = keys::lookup(name).unwrap();
        out.push(LowInput::KeyDown { key, ascii: 0 });
        out.push(LowInput::KeyUp { key });
        return Ok(());
    }
    let byte = rust_dos::keylayout::cp437(c).ok_or_else(|| format!("cannot type character {:?}", c))?;
    let (key, shift, altgr, ascii) = match layout.reverse(byte) {
        Some((scan, shift, altgr)) => {
            (PcKey { scan, ascii: byte, shifted: byte, modifier: 0, extended: false }, shift, altgr, byte)
        }
        None => match keys::char_to_key(c) {
            Some((key, shift)) => (key, shift, false, if shift { key.shifted } else { key.ascii }),
            None => {
                out.push(LowInput::Char(byte));
                return Ok(());
            }
        },
    };
    let modifiers: Vec<PcKey> = [(shift, "lshift"), (altgr, "ralt")]
        .into_iter()
        .filter(|&(on, _)| on)
        .map(|(_, name)| keys::lookup(name).unwrap())
        .collect();
    for &m in &modifiers {
        out.push(LowInput::KeyDown { key: m, ascii: 0 });
    }
    out.push(LowInput::KeyDown { key, ascii });
    out.push(LowInput::KeyUp { key });
    for &m in modifiers.iter().rev() {
        out.push(LowInput::KeyUp { key: m });
    }
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
}

/// Memory the debugger watches: `len` bytes at physical address `phys`,
/// which held `value` when last looked at.
struct Watch {
    phys: usize,
    len: u8,
    value: u32,
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
    pause_waiters: Vec<oneshot::Sender<Reply>>,
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

    trace: TraceRing,
    trace_enabled: bool,
    trace_stream_max: usize,
    trace_stream_cursor: u64,
    tracing_now: bool,
    batch_t_us: u64,

    input: VecDeque<LowInput>,
    input_wait_until: Option<Instant>,
    key_stall_frames: u32,
    /// While the settings window is open, remote input goes to it
    /// (`take_ui_input`) instead of to the machine.
    pub divert: bool,
    ui_input: Vec<UiInput>,
    /// Where the remote mouse is, for clicks on the settings window.
    ui_pointer: (i32, i32),
    /// Ctrl+F12 came in: open or close the settings window.
    hotkey: bool,
    /// Save states to save or load, for the front end.
    state_requests: Vec<StateRequest>,
    /// Shift, Ctrl and Alt bits (as at 40:17h) the remote client holds.
    remote_mods: u8,
    /// Keys the remote client holds down on the machine.
    remote_held: Vec<PcKey>,

    frame_waiters: Vec<oneshot::Sender<Reply>>,
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
            pause_waiters: Vec::new(),
            break_exceptions: 0,
            break_mode_switch: false,
            seen_exceptions: 0,
            seen_mode_switches: 0,
            watchpoints: Vec::new(),
            watch_hit: None,
            trace: TraceRing::new(trace_capacity),
            trace_enabled: false,
            trace_stream_max: 1000,
            trace_stream_cursor: 0,
            tracing_now: false,
            batch_t_us: 0,
            input: VecDeque::new(),
            input_wait_until: None,
            key_stall_frames: 0,
            divert: false,
            ui_input: Vec::new(),
            ui_pointer: (0, 0),
            hotkey: false,
            state_requests: Vec::new(),
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
    pub fn start(cpu: &mut Cpu, addr: SocketAddr, trace_capacity: usize) -> Result<Self, String> {
        if !addr.ip().is_loopback() {
            eprintln!(
                "[DEBUG] WARNING: debug server bound to non-loopback address {}. It has no authentication!",
                addr
            );
        }
        let shared = Arc::new(Shared {
            frame: Mutex::new(video::Frame::new(video::SCREEN_WIDTH, video::SCREEN_HEIGHT)),
            frame_seq: AtomicU64::new(0),
            screen_subscribers: AtomicUsize::new(0),
            events: broadcast::channel(1024).0,
            trace: broadcast::channel(64).0,
            audio: broadcast::channel(256).0,
            log: Mutex::new(VecDeque::with_capacity(LOG_RING_CAPACITY)),
            start_time: cpu.bus.start_time,
        });
        let (tx, rx) = mpsc::channel();
        server::spawn(addr, tx, shared.clone())?;

        let log_shared = shared.clone();
        cpu.bus.log_hook = Some(Box::new(move |line: &str| {
            let t_ms = log_shared.start_time.elapsed().as_millis() as u64;
            if let Ok(mut log) = log_shared.log.lock() {
                if log.len() >= LOG_RING_CAPACITY {
                    log.pop_front();
                }
                log.push_back(LogLine { t_ms, line: line.to_string() });
            }
            log_shared.emit(json!({"type": "log", "t_ms": t_ms, "line": line}));
        }));
        let audio_tx = shared.audio.clone();
        cpu.bus.audio_hook = Some(Box::new(move |samples: &[i16]| {
            if audio_tx.receiver_count() > 0 {
                let _ = audio_tx.send(Arc::from(samples));
            }
        }));

        println!("[DEBUG] Debug server listening on http://{}/", addr);
        Ok(Self::new(Some(rx), Some(shared), trace_capacity))
    }

    fn emit(&self, v: Value) {
        if let Some(s) = &self.shared {
            s.emit(v);
        }
    }

    // ----- per-frame hooks ------------------------------------------------

    /// Drain pending requests and feed queued input. Call once per frame,
    /// before the execution batch.
    pub fn poll(&mut self, cpu: &mut Cpu) {
        let Some(rx) = self.rx.take() else { return };
        while let Ok(req) = rx.try_recv() {
            self.handle(cpu, req);
        }
        self.rx = Some(rx);

        self.process_input(cpu);

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
        if !self.skip_bp_once
            && (self.temp_breakpoint == Some(phys_ip) || self.breakpoints.contains(&phys_ip))
        {
            self.temp_breakpoint = None;
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
        }
        false
    }

    /// Call after the execution batch: delivers pause notifications and
    /// streams new trace entries.
    pub fn end_batch(&mut self, cpu: &Cpu) {
        if let Some(reason) = self.pause_hit.take() {
            let regs = regs_json(cpu);
            let reason_str = match reason {
                PauseReason::Request => "request",
                PauseReason::Breakpoint => "breakpoint",
                PauseReason::Step => "step",
                PauseReason::Exception => "exception",
                PauseReason::ModeSwitch => "mode_switch",
                PauseReason::Watchpoint => "watchpoint",
            };
            let mut reply = json!({"paused": true, "reason": reason_str, "icount": cpu.executed, "registers": regs});
            if reason == PauseReason::Exception
                && let Some(e) = pm::exceptions_json(cpu)["recent"].as_array().and_then(|a| a.last().cloned())
            {
                reply["exception"] = e;
            }
            if let Some((phys, len, old, new)) = self.watch_hit.take() {
                let digits = len as usize * 2;
                reply["watch"] = json!({
                    "addr": format!("{:05X}", phys),
                    "len": len,
                    "old": format!("{:0width$X}", old, width = digits),
                    "new": format!("{:0width$X}", new, width = digits),
                });
            }
            let mut event = reply.clone();
            event["type"] = "paused".into();
            self.emit(event);
            for w in self.pause_waiters.drain(..) {
                let _ = w.send(Reply::Json(reply.clone()));
            }
        }

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
            let hits = cache_hits - st.cache_mark.0;
            let misses = cache_misses - st.cache_mark.1;
            st.cache_hit_rate = if hits + misses > 0 { hits as f64 / (hits + misses) as f64 } else { 0.0 };
            st.cache_mark = (cache_hits, cache_misses);
            st.executed = 0;
            st.exec_time = Duration::ZERO;
            st.window_start = Instant::now();
        }
    }

    fn stats_json(&self, cpu: &Cpu) -> Value {
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
            // blocks entered from the execution loop, and blocks that
            // stopped at once for the timer deadline, or because their code
            // had changed (stale) or changed under them (smc).
            "dynrec": {
                "blocks": d.blocks,
                "instructions": d.instructions,
                "native": d.native,
                "live_blocks": d.live_blocks,
                "code_bytes": d.code_bytes,
                "links": d.links,
                "flushes": d.flushes,
                "runs": d.runs,
                "deadline": d.deadline,
                "stale": d.stale,
                "smc": d.smc,
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

    /// Whether `capture_frame` wants to see the composited frame this frame.
    pub fn wants_frame(&self) -> bool {
        !self.frame_waiters.is_empty()
            || self.shared.as_ref().is_some_and(|s| s.screen_subscribers.load(Ordering::Relaxed) > 0)
    }

    /// Hand the composited frame (cached VGA render + cursor overlays) to
    /// screenshot requests and the screen stream.
    pub fn capture_frame(&mut self, screen: &video::Frame) {
        self.frames += 1;
        let (mark_t, mark_n) = self.fps_mark;
        let dt = mark_t.elapsed().as_secs_f64();
        if dt >= 1.0 {
            self.fps = (self.frames - mark_n) as f64 / dt;
            self.fps_mark = (Instant::now(), self.frames);
        }

        if !self.wants_frame() {
            return;
        }
        for w in self.frame_waiters.drain(..) {
            let _ = w.send(Reply::Frame(screen.clone()));
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

    // ----- input ------------------------------------------------------------

    fn process_input(&mut self, cpu: &mut Cpu) {
        let now = Instant::now();
        if let Some(t) = self.input_wait_until {
            if now < t {
                return;
            }
            self.input_wait_until = None;
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
                    if cpu.bus.keyboard_buffer.len() < keyboard::BIOS_BUFFER_KEYS {
                        cpu.bus.keyboard_buffer.push_back(byte as u16);
                    }
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
                LowInput::MouseRel { dx, dy } => {
                    let (x, y) = (cpu.bus.mouse.x, cpu.bus.mouse.y);
                    cpu.bus.mouse.set_position(x + dx, y + dy);
                }
                LowInput::Button { idx, down } => {
                    if down {
                        cpu.bus.mouse.button_down(idx);
                    } else {
                        cpu.bus.mouse.button_up(idx);
                    }
                }
                LowInput::Notify(tx) => {
                    let _ = tx.send(Reply::Json(json!({"ok": true, "done": true})));
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

    /// The save states remote clients asked to save or load.
    pub fn take_state_requests(&mut self) -> Vec<StateRequest> {
        std::mem::take(&mut self.state_requests)
    }

    // ----- request handling --------------------------------------------------

    fn handle(&mut self, cpu: &mut Cpu, req: Request) {
        let cmd = match req.cmd {
            Cmd::SaveState { path } | Cmd::LoadState { path } if path.is_empty() => {
                let _ = req.reply.send(Reply::bad("a path is needed"));
                return;
            }
            Cmd::SaveState { path } => {
                self.state_requests.push(StateRequest { load: false, path: PathBuf::from(path), reply: req.reply });
                return;
            }
            Cmd::LoadState { path } => {
                self.state_requests.push(StateRequest { load: true, path: PathBuf::from(path), reply: req.reply });
                return;
            }
            cmd => cmd,
        };
        let reply = match cmd {
            Cmd::Status => {
                let status = self.status(cpu);
                cpu.bus.audio_peak = 0;
                Reply::Json(status)
            }
            Cmd::Stats => Reply::Json(self.stats_json(cpu)),
            Cmd::Screenshot => {
                // Answered by the next capture_frame call.
                self.frame_waiters.push(req.reply);
                return;
            }
            Cmd::ScreenText => screen_text(cpu),
            Cmd::TraceQuery(q) => self.trace_query(cpu, q),
            Cmd::TraceControl { enabled, clear, stream_max } => {
                if let Some(e) = enabled {
                    self.trace_enabled = e;
                }
                if clear {
                    self.trace.clear();
                }
                if let Some(m) = stream_max {
                    self.trace_stream_max = m.max(1);
                }
                Reply::Json(self.trace_status())
            }
            Cmd::Input { events, wait } => {
                let mut low = Vec::new();
                for ev in &events {
                    if let Err(e) = expand_input(ev, cpu.bus.kbd.layout, &mut low) {
                        let _ = req.reply.send(Reply::bad(e));
                        return;
                    }
                }
                self.input.extend(low);
                if wait {
                    self.input.push_back(LowInput::Notify(req.reply));
                    return;
                }
                Reply::Json(json!({"ok": true, "queued": self.input.len()}))
            }
            Cmd::InputClear => {
                for ev in self.input.drain(..) {
                    if let LowInput::Notify(tx) = ev {
                        let _ = tx.send(Reply::Json(json!({"ok": false, "cancelled": true})));
                    }
                }
                self.input_wait_until = None;
                Reply::Json(json!({"ok": true}))
            }
            Cmd::Pause => {
                if self.paused && self.pause_hit.is_none() {
                    Reply::Json(json!({"paused": true, "icount": cpu.executed, "registers": regs_json(cpu)}))
                } else {
                    if !self.paused {
                        self.enter_pause(PauseReason::Request);
                    }
                    // Pause notifications go out from end_batch.
                    self.pause_waiters.push(req.reply);
                    return;
                }
            }
            Cmd::Resume { until } => {
                if let Some(a) = until {
                    match parse_addr(cpu, &a).and_then(breakpoint_phys) {
                        Ok(phys) => self.temp_breakpoint = Some(phys),
                        Err(e) => {
                            let _ = req.reply.send(Reply::bad(e));
                            return;
                        }
                    }
                }
                let was = self.paused;
                self.resume();
                if was {
                    self.emit(json!({"type": "resumed", "icount": cpu.executed}));
                }
                Reply::Json(json!({"ok": true, "paused": false}))
            }
            Cmd::Step { count } => {
                self.step_budget = Some(count.max(1));
                self.resume();
                self.pause_waiters.push(req.reply);
                return;
            }
            Cmd::StepOver => {
                match step_over_target(cpu) {
                    Some(next) => self.temp_breakpoint = Some(next),
                    None => self.step_budget = Some(1),
                }
                let was = self.paused;
                self.resume();
                if was {
                    self.emit(json!({"type": "resumed", "icount": cpu.executed}));
                }
                self.pause_waiters.push(req.reply);
                return;
            }
            Cmd::WaitPause => {
                if self.paused && self.pause_hit.is_none() {
                    Reply::Json(json!({"paused": true, "icount": cpu.executed, "registers": regs_json(cpu)}))
                } else {
                    self.pause_waiters.push(req.reply);
                    return;
                }
            }
            Cmd::RebootShell => {
                cpu.state = CpuState::RebootShell;
                self.resume();
                Reply::Json(json!({"ok": true}))
            }
            Cmd::GetRegs => Reply::Json(regs_json(cpu)),
            Cmd::SetRegs(map) => match set_regs(cpu, &map) {
                Ok(()) => Reply::Json(regs_json(cpu)),
                Err(e) => Reply::bad(e),
            },
            Cmd::ReadMem { addr, len } => match parse_addr(cpu, &addr) {
                Ok(a) => {
                    let len = match (a.lin, a.phys) {
                        (Some(_), _) => len,
                        // The linear frame buffer, up to its end.
                        (None, Some(p)) if Vbe::lfb_offset(p, 1).is_some() => {
                            len.min(video::vbe::LFB_BASE + video::vbe::VRAM_SIZE - p)
                        }
                        (None, p) => len.min(cpu.bus.ram().len().saturating_sub(p.unwrap_or(0))),
                    };
                    let data = a.read(cpu, len);
                    Reply::Bytes { addr: a.lin.map_or(a.phys.unwrap_or(0), |l| l as usize), segoff: a.segoff, data }
                }
                Err(e) => Reply::bad(e),
            },
            Cmd::WriteMem { addr, data } => match parse_addr(cpu, &addr) {
                Ok(a) => {
                    let targets: Option<Vec<usize>> = (0..data.len()).map(|i| a.byte(cpu, i)).collect();
                    match targets {
                        Some(t) if t.iter().all(|&p| p < cpu.bus.ram().len() || Vbe::lfb_offset(p, 1).is_some()) => {
                            for (p, b) in t.iter().zip(&data) {
                                cpu.bus.write_8(*p, *b);
                            }
                            // The debugger's own changes don't stop the
                            // machine.
                            for w in &mut self.watchpoints {
                                w.value = Watch::read(cpu, w.phys, w.len);
                            }
                            Reply::Json(json!({
                                "ok": true,
                                "addr": format!("{:05X}", a.phys.unwrap_or(0)),
                                "written": data.len(),
                            }))
                        }
                        Some(_) => Reply::bad("write extends past end of memory"),
                        None => Reply::bad("write reaches an unmapped page"),
                    }
                }
                Err(e) => Reply::bad(e),
            },
            Cmd::Disasm { addr, count } => self.disasm(cpu, addr, count),
            Cmd::ListBreakpoints => Reply::Json(self.breakpoints_json()),
            Cmd::AddBreakpoint(a) => match parse_addr(cpu, &a).and_then(breakpoint_phys) {
                Ok(phys) => {
                    self.breakpoints.insert(phys);
                    Reply::Json(self.breakpoints_json())
                }
                Err(e) => Reply::bad(e),
            },
            Cmd::RemoveBreakpoint(a) => match a {
                None => {
                    self.breakpoints.clear();
                    self.break_exceptions = 0;
                    self.break_mode_switch = false;
                    Reply::Json(self.breakpoints_json())
                }
                Some(a) => match parse_addr(cpu, &a).and_then(breakpoint_phys) {
                    Ok(phys) => {
                        if self.breakpoints.remove(&phys) {
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
                    let _ = req.reply.send(Reply::bad("len is 1, 2 or 4"));
                    return;
                }
                match parse_addr(cpu, &addr).and_then(breakpoint_phys) {
                    Ok(phys) => {
                        self.watchpoints.retain(|w| w.phys != phys);
                        self.watchpoints.push(Watch { phys, len, value: Watch::read(cpu, phys, len) });
                        Reply::Json(self.watchpoints_json(cpu))
                    }
                    Err(e) => Reply::bad(e),
                }
            }
            Cmd::RemoveWatchpoint(addr) => match addr {
                None => {
                    self.watchpoints.clear();
                    Reply::Json(self.watchpoints_json(cpu))
                }
                Some(a) => match parse_addr(cpu, &a).and_then(breakpoint_phys) {
                    Ok(phys) => {
                        let before = self.watchpoints.len();
                        self.watchpoints.retain(|w| w.phys != phys);
                        if self.watchpoints.len() < before {
                            Reply::Json(self.watchpoints_json(cpu))
                        } else {
                            Reply::Error(404, format!("no watchpoint at {:05X}", phys))
                        }
                    }
                    Err(e) => Reply::bad(e),
                },
            },
            Cmd::BreakOn { exceptions, clear_exceptions, mode_switch } => {
                if let Some(mask) = exceptions {
                    self.break_exceptions |= mask;
                }
                if let Some(mask) = clear_exceptions {
                    self.break_exceptions &= !mask;
                }
                if let Some(m) = mode_switch {
                    self.break_mode_switch = m;
                }
                self.seen_exceptions = cpu.exceptions;
                self.seen_mode_switches = cpu.mode_switches;
                Reply::Json(self.breakpoints_json())
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
            Cmd::Gus => Reply::Json(match &cpu.bus.gus {
                Some(gus) => gus.snapshot(),
                None => serde_json::json!({ "installed": false }),
            }),
            Cmd::Exceptions => Reply::Json(pm::exceptions_json(cpu)),
            Cmd::Drives => Reply::Json(drives_json(cpu)),
            Cmd::SwapImages => {
                let messages = cpu.bus.swap_images();
                for message in &messages {
                    cpu.bus.log_string(&format!("[DEBUG] {}", message));
                }
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
                        Reply::Json(drives_json(cpu))
                    }
                    Err(e) => Reply::bad(e),
                }
            }
            Cmd::SaveState { .. } | Cmd::LoadState { .. } => unreachable!("handed to the front end above"),
            Cmd::Unmount { drive } => {
                let result = parse_drive_letter(&drive)
                    .ok_or_else(|| format!("invalid drive letter '{}'", drive))
                    .and_then(|d| cpu.bus.unmount_drive(d));
                match result {
                    Ok(()) => Reply::Json(drives_json(cpu)),
                    Err(e) => Reply::bad(e),
                }
            }
        };
        let _ = req.reply.send(reply);
    }

    fn status(&self, cpu: &Cpu) -> Value {
        let (w, h) = cpu.bus.display_size();
        let (frame_w, frame_h) = video::frame_size(&cpu.bus);
        let crtc = &cpu.bus.vga.crtc_regs;
        let timing = cpu.bus.vga.peek_timing();
        json!({
            "paused": self.paused,
            "settings_window": self.divert,
            "icount": cpu.executed,
            "uptime_ms": cpu.bus.start_time.elapsed().as_millis() as u64,
            "fps": (self.fps * 10.0).round() / 10.0,
            "cycles_per_ms": cpu.bus.clock.cycles_per_ms(),
            "cs_ip": if cpu.eip() > 0xFFFF {
                format!("{:04X}:{:08X}", cpu.cs(), cpu.eip())
            } else {
                format!("{:04X}:{:04X}", cpu.cs(), cpu.ip())
            },
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
            "input_queue": self.input.len(),
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
            "mouse": {
                "installed": cpu.bus.mouse.installed,
                "x": cpu.bus.mouse.x, "y": cpu.bus.mouse.y,
                "buttons": cpu.bus.mouse.buttons,
                "visible": cpu.bus.mouse.hide_counter <= 0,
                "event_handler": format!(
                    "{:04X}:{:04X} mask {:04X}",
                    cpu.bus.mouse.callback_cs, cpu.bus.mouse.callback_ip, cpu.bus.mouse.callback_mask
                ),
            },
        })
    }

    fn trace_status(&self) -> Value {
        json!({
            "enabled": self.trace_enabled,
            "active": self.tracing_now,
            "entries": self.trace.len(),
            "capacity": self.trace.capacity(),
            "total_recorded": self.trace.total(),
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
        // Newest-first collection so `limit` keeps the most recent entries.
        let mut out: Vec<TraceEntry> = self.trace.iter().rev().filter(|e| matches(e)).take(limit).copied().collect();
        out.reverse();
        Reply::Trace(out)
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
        let exceptions: Vec<String> =
            (0..32).filter(|i| self.break_exceptions & (1 << i) != 0).map(|i| format!("{:02X}", i)).collect();
        json!({
            "breakpoints": v.iter().map(|p| format!("{:05X}", p)).collect::<Vec<_>>(),
            "exceptions": exceptions,
            "mode_switch": self.break_mode_switch,
        })
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
    let drives: Map<String, Value> = cpu
        .bus
        .disk
        .mounted_drives()
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
            (info.letter().to_string(), entry)
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
        parse_drive_letter(drive).ok_or_else(|| format!("invalid drive letter '{}'", drive))?;
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
}

#[cfg(test)]
mod tests {
    use super::{LowInput, keys_for_char, step_over_len};
    use rust_dos::keylayout::Layout;

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
}
