//! Remote debug / control interface.
//!
//! An HTTP + WebSocket server (see `server.rs`) runs on its own thread with
//! a private tokio runtime. It never touches the emulator directly: requests
//! are sent to the main thread through an mpsc channel and executed by
//! `DebugHub::poll` once per frame, and data flows back through oneshot
//! replies, broadcast channels, and a shared frame snapshot.

pub mod keys;
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

use crate::cpu::{Cpu, CpuFlags, CpuState};
use crate::disk::{DriveKind, MountOptions, drive_letter};
use crate::keyboard;
use crate::mount::{display_host_path, parse_drive_letter, parse_kind};
use crate::video::{self, VideoMode};
use keys::PcKey;
use trace::{TraceEntry, TraceRing};

const LOG_RING_CAPACITY: usize = 5000;

/// State shared between the emulator thread and the server thread.
pub struct Shared {
    /// Latest composited 640x400 RGB24 frame. Only kept up to date while a
    /// screen stream is subscribed.
    pub frame: Mutex<Vec<u8>>,
    /// Bumped whenever `frame` changes.
    pub frame_seq: AtomicU64,
    pub screen_subscribers: AtomicUsize,
    /// JSON event stream: log lines, pause/resume, breakpoints, mode changes.
    pub events: broadcast::Sender<Arc<str>>,
    /// JSON trace batches (one per frame while subscribed).
    pub trace: broadcast::Sender<Arc<str>>,
    /// Mixed audio, 44.1 kHz mono s16.
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
    Frame(Vec<u8>),
    Trace(Vec<TraceEntry>),
    Bytes { addr: usize, segoff: Option<(u16, u16)>, data: Vec<u8> },
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
    Screenshot,
    ScreenText,
    TraceQuery(TraceQuery),
    TraceControl { enabled: Option<bool>, clear: bool, stream_max: Option<usize> },
    Input { events: Vec<InputEvent>, wait: bool },
    InputClear,
    Pause,
    Resume { until: Option<String> },
    Step { count: u64 },
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
    Ivt,
    Drives,
    Mount {
        drive: String,
        path: String,
        kind: Option<String>,
        label: Option<String>,
        read_only: bool,
    },
    Unmount { drive: String },
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
    KeyUp { key: PcKey },
    MouseTo { x: i32, y: i32, coords: Coords },
    MouseRel { dx: i32, dy: i32 },
    Button { idx: usize, down: bool },
    Wait(Duration),
    Notify(oneshot::Sender<Reply>),
}

const DEFAULT_HOLD_MS: u64 = 50;

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

fn expand_input(ev: &InputEvent, out: &mut Vec<LowInput>) -> Result<(), String> {
    match ev {
        InputEvent::Key { key, scancode, ascii, action, mods, hold_ms } => {
            let mod_keys = mods.iter().map(|m| modifier_key(m)).collect::<Result<Vec<_>, _>>()?;
            let mod_bits = mod_keys.iter().fold(0u8, |a, k| a | k.modifier);
            let pc = match (key, scancode) {
                (Some(name), _) => keys::lookup(name).ok_or_else(|| {
                    format!("unknown key '{}'; valid keys: {}", name, keys::names().join(", "))
                })?,
                (None, Some(sc)) => PcKey { scan: *sc, ascii: ascii.unwrap_or(0), shifted: ascii.unwrap_or(0), modifier: 0 },
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
            let shift = keys::lookup("lshift").unwrap();
            for c in text.chars() {
                let (key, needs_shift) =
                    keys::char_to_key(c).ok_or_else(|| format!("cannot type character {:?}", c))?;
                if needs_shift {
                    out.push(LowInput::KeyDown { key: shift, ascii: 0 });
                }
                let ascii = if needs_shift { key.shifted } else { key.ascii };
                out.push(LowInput::KeyDown { key, ascii });
                out.push(LowInput::KeyUp { key });
                if needs_shift {
                    out.push(LowInput::KeyUp { key: shift });
                }
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
}

pub struct DebugHub {
    rx: Option<mpsc::Receiver<Request>>,
    shared: Option<Arc<Shared>>,

    pub paused: bool,
    /// Instructions executed since start (only real instructions, not
    /// injected IRQ entries).
    pub icount: u64,
    step_budget: Option<u64>,
    breakpoints: HashSet<usize>,
    temp_breakpoint: Option<usize>,
    /// Skip the breakpoint check for the first instruction after resuming,
    /// so continuing from a breakpoint doesn't immediately re-trigger it.
    skip_bp_once: bool,
    pause_hit: Option<PauseReason>,
    pause_waiters: Vec<oneshot::Sender<Reply>>,

    trace: TraceRing,
    trace_enabled: bool,
    trace_stream_max: usize,
    trace_stream_cursor: u64,
    tracing_now: bool,
    batch_t_us: u64,

    input: VecDeque<LowInput>,
    input_wait_until: Option<Instant>,
    key_stall_frames: u32,

    frame_waiters: Vec<oneshot::Sender<Reply>>,
    last_mode: Option<VideoMode>,
    frames: u64,
    fps: f64,
    fps_mark: (Instant, u64),
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
            icount: 0,
            step_budget: None,
            breakpoints: HashSet::new(),
            temp_breakpoint: None,
            skip_bp_once: false,
            pause_hit: None,
            pause_waiters: Vec::new(),
            trace: TraceRing::new(trace_capacity),
            trace_enabled: false,
            trace_stream_max: 1000,
            trace_stream_cursor: 0,
            tracing_now: false,
            batch_t_us: 0,
            input: VecDeque::new(),
            input_wait_until: None,
            key_stall_frames: 0,
            frame_waiters: Vec::new(),
            last_mode: None,
            frames: 0,
            fps: 0.0,
            fps_mark: (Instant::now(), 0),
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
            frame: Mutex::new(vec![0; (video::SCREEN_WIDTH * video::SCREEN_HEIGHT * 3) as usize]),
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

        if self.last_mode != Some(cpu.bus.video_mode) {
            let (w, h) = cpu.bus.video_mode.dimensions();
            self.emit(json!({
                "type": "video_mode",
                "mode": cpu.bus.video_mode as u8,
                "name": format!("{:?}", cpu.bus.video_mode),
                "width": w, "height": h,
            }));
            self.last_mode = Some(cpu.bus.video_mode);
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
    }

    /// Per-instruction hook, called just before an instruction at `phys_ip`
    /// executes. Returns true if execution must stop (the hub is now paused).
    #[inline]
    pub fn before_exec(&mut self, cpu: &Cpu, phys_ip: usize, ram: &[u8]) -> bool {
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
                icount: self.icount,
                cs: cpu.cs,
                ip: cpu.ip,
                ax: cpu.ax,
                bx: cpu.bx,
                cx: cpu.cx,
                dx: cpu.dx,
                si: cpu.si,
                di: cpu.di,
                bp: cpu.bp,
                sp: cpu.sp,
                ds: cpu.ds,
                es: cpu.es,
                ss: cpu.ss,
                flags: cpu.get_cpu_flags().bits(),
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
            };
            self.emit(json!({"type": "paused", "reason": reason_str, "icount": self.icount, "registers": regs}));
            let reply = json!({"paused": true, "reason": reason_str, "icount": self.icount, "registers": regs});
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

    /// Whether `capture_frame` wants to see the composited frame this frame.
    pub fn wants_frame(&self) -> bool {
        !self.frame_waiters.is_empty()
            || self.shared.as_ref().is_some_and(|s| s.screen_subscribers.load(Ordering::Relaxed) > 0)
    }

    /// Hand the composited frame (cached VGA render + cursor overlays) to
    /// screenshot requests and the screen stream.
    pub fn capture_frame(&mut self, rgb: &[u8]) {
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
            let _ = w.send(Reply::Frame(rgb.to_vec()));
        }
        if let Some(shared) = &self.shared {
            if shared.screen_subscribers.load(Ordering::Relaxed) > 0 {
                if let Ok(mut frame) = shared.frame.lock() {
                    if frame.as_slice() != rgb {
                        frame.copy_from_slice(rgb);
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
                    // Port 0x60 latches one scan code. Wait for the previous
                    // IRQ1 to be taken before delivering the next one, but
                    // don't stall forever on programs that mask IRQ1 and
                    // poll the port instead.
                    if cpu.bus.irq1_pending && self.key_stall_frames < 2 {
                        self.key_stall_frames += 1;
                        return;
                    }
                    self.key_stall_frames = 0;
                    match self.input.pop_front().unwrap() {
                        LowInput::KeyDown { key, ascii } => apply_key(cpu, key, ascii, true),
                        LowInput::KeyUp { key } => apply_key(cpu, key, 0, false),
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

    // ----- request handling --------------------------------------------------

    fn handle(&mut self, cpu: &mut Cpu, req: Request) {
        let reply = match req.cmd {
            Cmd::Status => Reply::Json(self.status(cpu)),
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
                    if let Err(e) = expand_input(ev, &mut low) {
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
                    Reply::Json(json!({"paused": true, "icount": self.icount, "registers": regs_json(cpu)}))
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
                    match parse_addr(cpu, &a) {
                        Ok((phys, _)) => self.temp_breakpoint = Some(phys),
                        Err(e) => {
                            let _ = req.reply.send(Reply::bad(e));
                            return;
                        }
                    }
                }
                let was = self.paused;
                self.resume();
                if was {
                    self.emit(json!({"type": "resumed", "icount": self.icount}));
                }
                Reply::Json(json!({"ok": true, "paused": false}))
            }
            Cmd::Step { count } => {
                self.step_budget = Some(count.max(1));
                self.resume();
                self.pause_waiters.push(req.reply);
                return;
            }
            Cmd::WaitPause => {
                if self.paused && self.pause_hit.is_none() {
                    Reply::Json(json!({"paused": true, "icount": self.icount, "registers": regs_json(cpu)}))
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
                Ok((phys, segoff)) => {
                    let len = len.min(cpu.bus.ram.len().saturating_sub(phys));
                    let data = (phys..phys + len).map(|a| cpu.bus.peek_8(a)).collect();
                    Reply::Bytes { addr: phys, segoff, data }
                }
                Err(e) => Reply::bad(e),
            },
            Cmd::WriteMem { addr, data } => match parse_addr(cpu, &addr) {
                Ok((phys, _)) if phys + data.len() <= cpu.bus.ram.len() => {
                    for (i, b) in data.iter().enumerate() {
                        cpu.bus.write_8(phys + i, *b);
                    }
                    Reply::Json(json!({"ok": true, "addr": format!("{:05X}", phys), "written": data.len()}))
                }
                Ok(_) => Reply::bad("write extends past end of memory"),
                Err(e) => Reply::bad(e),
            },
            Cmd::Disasm { addr, count } => self.disasm(cpu, addr, count),
            Cmd::ListBreakpoints => Reply::Json(self.breakpoints_json()),
            Cmd::AddBreakpoint(a) => match parse_addr(cpu, &a) {
                Ok((phys, _)) => {
                    self.breakpoints.insert(phys);
                    Reply::Json(self.breakpoints_json())
                }
                Err(e) => Reply::bad(e),
            },
            Cmd::RemoveBreakpoint(a) => match a {
                None => {
                    self.breakpoints.clear();
                    Reply::Json(self.breakpoints_json())
                }
                Some(a) => match parse_addr(cpu, &a) {
                    Ok((phys, _)) => {
                        if self.breakpoints.remove(&phys) {
                            Reply::Json(self.breakpoints_json())
                        } else {
                            Reply::Error(404, format!("no breakpoint at {:05X}", phys))
                        }
                    }
                    Err(e) => Reply::bad(e),
                },
            },
            Cmd::Ivt => Reply::Json(ivt_json(cpu)),
            Cmd::Drives => Reply::Json(drives_json(cpu)),
            Cmd::Mount { drive, path, kind, label, read_only } => {
                match mount_drive(cpu, &drive, &path, kind.as_deref(), label, read_only) {
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
        let (w, h) = cpu.bus.video_mode.dimensions();
        json!({
            "paused": self.paused,
            "icount": self.icount,
            "uptime_ms": cpu.bus.start_time.elapsed().as_millis() as u64,
            "fps": (self.fps * 10.0).round() / 10.0,
            "cycles_per_ms": cpu.bus.clock.cycles_per_ms(),
            "cs_ip": format!("{:04X}:{:04X}", cpu.cs, cpu.ip),
            "cpu_state": format!("{:?}", cpu.state),
            "shell_idle": shell_idle(cpu),
            "process_depth": cpu.process_stack.len(),
            "current_psp": format!("{:04X}", cpu.current_psp),
            "video": {
                "mode": cpu.bus.video_mode as u8,
                "name": format!("{:?}", cpu.bus.video_mode),
                "width": w, "height": h,
            },
            "drive_c": display_host_path(cpu.bus.disk.root_path()),
            "current_drive": drive_letter(cpu.bus.disk.get_current_drive()).to_string(),
            "trace": self.trace_status(),
            "breakpoints": self.breakpoints.len(),
            "input_queue": self.input.len(),
            "keyboard_buffer": cpu.bus.keyboard_buffer.len(),
            "mouse": {
                "installed": cpu.bus.mouse.installed,
                "x": cpu.bus.mouse.x, "y": cpu.bus.mouse.y,
                "buttons": cpu.bus.mouse.buttons,
                "visible": cpu.bus.mouse.hide_counter <= 0,
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
        let (phys, segoff) = match addr {
            Some(a) => match parse_addr(cpu, &a) {
                Ok(v) => v,
                Err(e) => return Reply::bad(e),
            },
            None => (cpu.get_physical_addr(cpu.cs, cpu.ip), Some((cpu.cs, cpu.ip))),
        };
        let (seg, mut off) = segoff.unwrap_or(((phys >> 4) as u16, (phys & 0xF) as u16));
        let base = (seg as usize) << 4;
        let cur = cpu.get_physical_addr(cpu.cs, cpu.ip);
        let mut lines = Vec::new();
        for _ in 0..count.min(1000) {
            let p = (base + off as usize) & 0xFFFFF;
            let bytes: Vec<u8> = (0..15).map(|i| cpu.bus.peek_8((p + i) & 0xFFFFF)).collect();
            let (len, text) = if bytes[0] == 0xFE && bytes[1] == 0x38 {
                (3, format!("HLE INT {:02X}h", bytes[2]))
            } else {
                trace::disasm_one(&bytes, off)
            };
            let marker = match (p == cur, self.breakpoints.contains(&p)) {
                (true, true) => "=>*",
                (true, false) => "=> ",
                (false, true) => "  *",
                _ => "   ",
            };
            lines.push(format!(
                "{} {:04X}:{:04X}  {:<20} {}",
                marker,
                seg,
                off,
                trace::hex_bytes(&bytes[..len]),
                text
            ));
            off = off.wrapping_add(len as u16);
        }
        Reply::Json(json!({"lines": lines}))
    }

    fn breakpoints_json(&self) -> Value {
        let mut v: Vec<_> = self.breakpoints.iter().copied().collect();
        v.sort();
        json!({"breakpoints": v.iter().map(|p| format!("{:05X}", p)).collect::<Vec<_>>()})
    }
}

fn apply_key(cpu: &mut Cpu, key: PcKey, ascii: u8, down: bool) {
    if key.modifier != 0 {
        let mut flags = cpu.bus.read_8(0x0417);
        if down {
            flags |= key.modifier;
            keyboard::deliver_scan_only(&mut cpu.bus, key.scan);
        } else {
            flags &= !key.modifier;
            keyboard::deliver_key_up(&mut cpu.bus, key.scan);
        }
        cpu.bus.write_8(0x0417, flags);
    } else if down {
        keyboard::deliver_key_down(&mut cpu.bus, ((key.scan as u16) << 8) | ascii as u16);
    } else {
        keyboard::deliver_key_up(&mut cpu.bus, key.scan);
    }
}

/// Map 640x400 screenshot pixels into the mouse driver's virtual coordinate
/// system (same convention as the SDL path in main.rs).
fn screen_to_virtual_mouse(cpu: &Cpu, x: i32, y: i32) -> (i32, i32) {
    let (mode_w, mode_h) = cpu.bus.video_mode.dimensions();
    let px = x.clamp(0, video::SCREEN_WIDTH as i32 - 1);
    let py = y.clamp(0, video::SCREEN_HEIGHT as i32 - 1);
    let virt_w = if mode_w < 640 { 640 } else { mode_w as i32 };
    let virt_h = mode_h as i32;
    let vx = (px as i64 * virt_w as i64 / video::SCREEN_WIDTH as i64) as i32;
    let vy = (py as i64 * virt_h as i64 / video::SCREEN_HEIGHT as i64) as i32;
    (vx.clamp(0, virt_w - 1), vy.clamp(0, virt_h - 1))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse a hex number with optional `0x` prefix / `h` suffix.
pub fn parse_hex(s: &str) -> Result<u32, String> {
    let t = s.trim();
    let t = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")).unwrap_or(t);
    let t = t.strip_suffix('h').or_else(|| t.strip_suffix('H')).unwrap_or(t);
    u32::from_str_radix(t, 16).map_err(|_| format!("invalid hex number '{}'", s))
}

fn reg16(cpu: &Cpu, name: &str) -> Option<u16> {
    Some(match name.to_ascii_lowercase().as_str() {
        "ax" => cpu.ax,
        "bx" => cpu.bx,
        "cx" => cpu.cx,
        "dx" => cpu.dx,
        "si" => cpu.si,
        "di" => cpu.di,
        "bp" => cpu.bp,
        "sp" => cpu.sp,
        "cs" => cpu.cs,
        "ds" => cpu.ds,
        "es" => cpu.es,
        "ss" => cpu.ss,
        "ip" => cpu.ip,
        _ => return None,
    })
}

/// Parse an address: `SEG:OFF` (hex numbers or register names, e.g.
/// `CS:IP`, `DS:SI`, `B800:0`) or a hex linear address (`0x12345`, `B8000`).
/// Returns the physical address and, when given, the segment:offset pair.
pub fn parse_addr(cpu: &Cpu, s: &str) -> Result<(usize, Option<(u16, u16)>), String> {
    let part = |p: &str| -> Result<u16, String> {
        match reg16(cpu, p.trim()) {
            Some(v) => Ok(v),
            None => parse_hex(p).and_then(|v| u16::try_from(v).map_err(|_| format!("'{}' exceeds 16 bits", p))),
        }
    };
    if let Some((seg, off)) = s.split_once(':') {
        let (seg, off) = (part(seg)?, part(off)?);
        Ok((cpu.get_physical_addr(seg, off), Some((seg, off))))
    } else {
        let v = parse_hex(s)? as usize;
        if v > 0xFFFFF {
            return Err(format!("address {:X} beyond 1 MiB", v));
        }
        Ok((v, None))
    }
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
    json!({
        "ax": h(cpu.ax), "bx": h(cpu.bx), "cx": h(cpu.cx), "dx": h(cpu.dx),
        "si": h(cpu.si), "di": h(cpu.di), "bp": h(cpu.bp), "sp": h(cpu.sp),
        "cs": h(cpu.cs), "ds": h(cpu.ds), "es": h(cpu.es), "ss": h(cpu.ss),
        "ip": h(cpu.ip),
        "flags": h(flags.bits()),
        "flags_set": names,
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
        let val = u16::try_from(val).map_err(|_| format!("{}: value exceeds 16 bits", k))?;
        let key = k.to_ascii_lowercase();
        if key != "flags" && reg16(cpu, &key).is_none() {
            return Err(format!("unknown register '{}'", k));
        }
        updates.push((key, val));
    }
    for (k, v) in updates {
        match k.as_str() {
            "ax" => cpu.ax = v,
            "bx" => cpu.bx = v,
            "cx" => cpu.cx = v,
            "dx" => cpu.dx = v,
            "si" => cpu.si = v,
            "di" => cpu.di = v,
            "bp" => cpu.bp = v,
            "sp" => cpu.sp = v,
            "cs" => cpu.cs = v,
            "ds" => cpu.ds = v,
            "es" => cpu.es = v,
            "ss" => cpu.ss = v,
            "ip" => cpu.ip = v,
            "flags" => cpu.set_cpu_flags(CpuFlags::from_bits_truncate(v)),
            _ => unreachable!(),
        }
    }
    Ok(())
}

fn screen_text(cpu: &Cpu) -> Reply {
    let mode = cpu.bus.video_mode;
    let (cols, rows) = match mode {
        VideoMode::Text80x25 | VideoMode::Text80x25Color => (80, cpu.bus.peek_8(0x0484) as usize + 1),
        VideoMode::Text40x25 | VideoMode::Text40x25Color => (40, 25),
        _ => {
            return Reply::Error(
                409,
                format!("video mode {:?} is a graphics mode; use /api/screenshot", mode),
            );
        }
    };
    let vram = &cpu.bus.vga.vram_text;
    let lines: Vec<String> = (0..rows)
        .map(|r| {
            let s: String = (0..cols)
                .map(|c| {
                    let off = (r * cols + c) * 2;
                    vram.get(off).map_or(' ', |b| keys::CP437[*b as usize])
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
fn shell_idle(cpu: &Cpu) -> bool {
    if !cpu.process_stack.is_empty() {
        return false;
    }
    let caller_cs = || {
        let frame = cpu.get_physical_addr(cpu.ss, cpu.sp.wrapping_add(2));
        cpu.bus.read_16(frame)
    };
    cpu.cs == 0 || (cpu.cs == 0xF000 && caller_cs() == 0)
}

fn drives_json(cpu: &Cpu) -> Value {
    let drives: Map<String, Value> = cpu
        .bus
        .disk
        .mounted_drives()
        .into_iter()
        .map(|info| {
            let entry = json!({
                "type": info.kind.name(),
                "path": info.root.as_deref().map(display_host_path),
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
) -> Result<PathBuf, String> {
    let drive =
        parse_drive_letter(drive).ok_or_else(|| format!("invalid drive letter '{}'", drive))?;
    let kind = match kind {
        Some(k) => Some(parse_kind(k).ok_or_else(|| format!("unknown drive type '{}'", k))?),
        None => None,
    };
    let explicit = kind.is_some() || label.is_some() || read_only;
    let opts = match cpu.bus.disk.drive_info(drive) {
        Some(info) if !explicit => MountOptions {
            kind: info.kind,
            label: Some(info.label),
            read_only: info.read_only,
        },
        _ => MountOptions {
            kind: kind.unwrap_or(DriveKind::HardDisk),
            label,
            read_only,
        },
    };
    cpu.bus.mount_drive(drive, &PathBuf::from(path), opts, true)
}
