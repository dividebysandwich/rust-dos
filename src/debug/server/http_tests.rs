//! The journal through HTTP and WebSockets: a server on a free port, and a
//! real machine with its hub run frame by frame on a thread of its own,
//! as the front end runs them.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::super::{DebugHub, Shared, ShutdownReason};
use crate::cpu::Cpu;
use crate::exec::{self, StopReason};

#[cfg(feature = "mcp")]
#[path = "../../../tests/mcp_metrics.rs"]
mod metrics_contract;

#[cfg(feature = "mcp")]
pub(crate) mod recorded_sdk {
    use super::*;
    use axum::http::{HeaderName, HeaderValue};
    use rmcp::{model::ClientJsonRpcMessage, transport::streamable_http_client::{StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse, SseError}};
    use std::collections::HashMap;
    use futures_util::StreamExt;
    type Headers = HashMap<HeaderName, HeaderValue>;

    #[derive(Clone)]
    pub(crate) struct Post {
        pub uri: Arc<str>,
        pub message: ClientJsonRpcMessage,
        pub session: Option<Arc<str>>,
        pub headers: Headers,
    }
    #[derive(Clone, Default)]
    pub(super) struct Client {
        pub inner: reqwest::Client,
        pub capture: Arc<std::sync::Mutex<Option<Post>>>,
        pub last: Arc<std::sync::Mutex<Option<Post>>>,
        pub response: Arc<std::sync::Mutex<Option<rmcp::model::ServerJsonRpcMessage>>>,
        pub initialize: Arc<std::sync::Mutex<Option<(Post, rmcp::model::ServerJsonRpcMessage)>>>,
    }
    impl StreamableHttpClient for Client {
        type Error = reqwest::Error;
        async fn post_message(&self, uri: Arc<str>, message: ClientJsonRpcMessage, session: Option<Arc<str>>, auth: Option<String>, headers: Headers) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
            assert!(auth.is_none(), "local native tests use no credentials");
            let value = serde_json::to_value(&message).unwrap();
            let post = Post { uri: uri.clone(), message: message.clone(), session: session.clone(), headers: headers.clone() };
            *self.last.lock().unwrap() = Some(post.clone());
            if value["params"]["name"] == "capture" {
                *self.capture.lock().unwrap() = Some(post.clone());
            }
            let mut response = self.inner.post_message(uri, message, session, auth, headers).await?;
            let initializing = value["method"] == "initialize";
            if initializing || value["params"]["name"] == "capture" {
                response = match response {
                    StreamableHttpPostResponse::Json(reply, session) => {
                        *self.response.lock().unwrap() = Some(reply.clone());
                        if initializing {
                            *self.initialize.lock().unwrap() = Some((post, reply.clone()));
                        }
                        StreamableHttpPostResponse::Json(reply, session)
                    }
                    StreamableHttpPostResponse::Sse(stream, session) => {
                        let observed = self.initialize.clone();
                        let terminal = self.response.clone();
                        let stream = stream.map(move |event| {
                            if let Ok(event) = &event {
                                if let Some(data) = &event.data {
                                    if let Ok(reply @ rmcp::model::ServerJsonRpcMessage::Response(_)) = serde_json::from_str(data) {
                                        *terminal.lock().unwrap() = Some(reply.clone());
                                        if initializing {
                                            *observed.lock().unwrap() = Some((post.clone(), reply));
                                        }
                                    }
                                }
                            }
                            event
                        }).boxed();
                        StreamableHttpPostResponse::Sse(stream, session)
                    }
                    other => other,
                };
            }
            Ok(response)
        }
        async fn delete_session(&self, uri: Arc<str>, session: Arc<str>, auth: Option<String>, headers: Headers) -> Result<(), StreamableHttpError<Self::Error>> {
            self.inner.delete_session(uri, session, auth, headers).await
        }
        async fn get_stream(&self, uri: Arc<str>, session: Option<Arc<str>>, event: Option<String>, auth: Option<String>, headers: Headers) -> Result<futures_util::stream::BoxStream<'static, Result<sse_stream::Sse, SseError>>, StreamableHttpError<Self::Error>> {
            self.inner.get_stream(uri, session, event, auth, headers).await
        }
    }
}

#[cfg(feature = "mcp")]
pub(crate) async fn native_guard_sdk_capture_post() -> (recorded_sdk::Post, recorded_sdk::Post, rmcp::model::ServerJsonRpcMessage, rmcp::model::ServerJsonRpcMessage) {
    use rmcp::{model::{CallToolRequestParams, ClientConfig, ProtocolVersion}, service::{ClientServiceExt, ClientLifecycleMode}, transport::{StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig}};
    let rig = Rig::with("native-guard-sdk-post", Options { mcp: true, ..Options::default() });
    let recorded = recorded_sdk::Client::default();
    let transport = StreamableHttpClientTransport::with_client(recorded.clone(), StreamableHttpClientTransportConfig::with_uri(format!("http://{}/mcp", rig.addr)));
    let client = ClientConfig::default().with_protocol_version(ProtocolVersion::V_2025_11_25)
        .serve_with_lifecycle(transport, ClientLifecycleMode::Initialize).await.unwrap();
    let result = client.call_tool(CallToolRequestParams::new("capture")).await.unwrap();
    assert_ne!(result.is_error, Some(true));
    assert!(result.content.iter().any(|content| content.as_image().is_some()), "producer captured actual guest pixels");
    let post = recorded.capture.lock().unwrap().take().unwrap();
    let (initialize, reply) = recorded.initialize.lock().unwrap().take().unwrap();
    let terminal = recorded.response.lock().unwrap().take().expect("actual SDK capture reply recorded");
    client.cancel().await.unwrap();
    (post, initialize, reply, terminal)
}

#[cfg(feature = "mcp")]
pub(crate) fn native_guard_picture() -> Vec<u8> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let name = format!("native-guard-picture-{}", NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let rig = Rig::with(&name, Options { mcp: true, ..Options::default() });
    let mut response = ureq::get(&format!("http://{}/api/screenshot", rig.addr)).call().unwrap();
    response.body_mut().with_config().limit(2 * 1024 * 1024).read_to_vec().unwrap()
}

#[cfg(feature = "mcp")]
pub(crate) async fn native_guard_answer(command: super::super::Cmd) -> super::super::Answer {
    let name = match command { super::super::Cmd::Status => "native-guard-status-answer", super::super::Cmd::Screenshot => "native-guard-picture-answer", _ => panic!("observation command required") };
    let rig = Rig::with(name, Options { mcp: true, ..Options::default() });
    let context = rig.client.context(None, None);
    rig.client.submit(&context, command, Duration::from_secs(1)).await.unwrap()
}

/// A .COM program: MOV AX,1234h; MOV BX,5678h; MOV AX,[0110h];
/// MOV AX,4C00h; INT 21h; and the word ABCDh at 0110h.
const PROBE: &str = "B83412BB7856A11001B8004CCD210000CDAB";

/// A .COM program that runs forever: NOP; JMP 0100h.
const LOOP: [u8; 3] = [0x90, 0xEB, 0xFD];

/// A .COM program that counts forever: INC WORD [0200h]; JMP back.
const COUNT: [u8; 6] = [0xFF, 0x06, 0x00, 0x02, 0xEB, 0xFA];

/// How a rig's machine runs.
#[derive(Clone)]
struct Options {
    mcp: bool,
    capture_gate: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// Journal entries kept.
    capacity: usize,
    /// Deterministic mode, paused at the start, in host frames of this
    /// many emulated milliseconds.
    deterministic: Option<u64>,
    /// Instructions a frame (outside deterministic mode).
    frame: u64,
    /// Host time a frame waits.
    sleep: Duration,
    /// How long the emulator thread lives on after the shutdown, as the
    /// front end does (printing, saving settings).
    linger: Duration,
    shutdown_flags: Option<Arc<std::sync::atomic::AtomicUsize>>,
    shutdown_mouse: Option<Arc<std::sync::atomic::AtomicUsize>>,
    actor_gate: Option<Arc<std::sync::atomic::AtomicBool>>,
    input_steps: Option<Arc<std::sync::atomic::AtomicUsize>>,
    frontend_action: Option<Arc<std::sync::atomic::AtomicUsize>>,
    observed_modifiers: Option<Arc<std::sync::atomic::AtomicUsize>>,
    observed_power_ons: Option<Arc<std::sync::atomic::AtomicUsize>>,
    observed_keyboard_word: Option<Arc<std::sync::atomic::AtomicUsize>>,
    observed_hotkeys: Option<Arc<std::sync::atomic::AtomicUsize>>,
    observed_input_capacity: Option<Arc<std::sync::atomic::AtomicUsize>>,
    typing_layout: Option<&'static rust_dos::keylayout::Layout>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            mcp: false,
            capture_gate: None,
            shutdown_flags: None,
            shutdown_mouse: None,
            actor_gate: None,
            input_steps: None,
            frontend_action: None,
            observed_modifiers: None,
            observed_power_ons: None,
            observed_keyboard_word: None,
            observed_hotkeys: None,
            observed_input_capacity: None,
            typing_layout: None,
            capacity: super::journal::CAPACITY, deterministic: None, frame: 5000, sleep: Duration::from_millis(1), linger: Duration::ZERO,
        }
    }
}

/// A server and the machine behind it.
struct Rig {
    #[cfg(feature = "mcp")]
    client: super::AppState,
    addr: SocketAddr,
    shared: Arc<Shared>,
    emulator: Option<JoinHandle<()>>,
}

impl Rig {
    fn new(name: &str) -> Self {
        Self::with(name, Options::default())
    }

    /// A rig whose journal keeps `capacity` entries.
    fn with_capacity(name: &str, capacity: usize) -> Self {
        Self::with(name, Options { capacity, ..Options::default() })
    }

    fn with(name: &str, options: Options) -> Self {
        let dir = std::env::temp_dir().join(format!("rust-dos-http-{}-{}", name, std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let code: Vec<u8> = (0..PROBE.len()).step_by(2).map(|i| u8::from_str_radix(&PROBE[i..i + 2], 16).unwrap()).collect();
        std::fs::write(dir.join("PROBE.COM"), code).unwrap();
        std::fs::write(dir.join("LOOP.COM"), LOOP).unwrap();
        std::fs::write(dir.join("COUNT.COM"), COUNT).unwrap();
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared::with_capacity(Instant::now(), options.capacity));
        #[cfg(feature = "mcp")]
        let client = super::AppState { tx: tx.clone(), shared: shared.clone() };
        let addr = super::spawn("127.0.0.1:0".parse().unwrap(), tx, shared.clone(),
            options.mcp,
        ).unwrap();
        let machine_shared = shared.clone();
        let emulator = std::thread::Builder::new()
            .stack_size(16 << 20)
            .spawn(move || run_machine(dir, rx, machine_shared, options))
            .unwrap();
        let rig = Rig { addr, shared, emulator: Some(emulator), #[cfg(feature = "mcp")] client };
        // At the prompt (paused there in deterministic mode).
        let deadline = Instant::now() + Duration::from_secs(20);
        while !rig.get("/api/status").json()["program"].as_str().is_some_and(str::is_empty) {
            assert!(Instant::now() < deadline, "the shell starts");
            std::thread::sleep(Duration::from_millis(10));
        }
        rig
    }

    fn get(&self, path: &str) -> Resp {
        http(self.addr, "GET", path, &[], None)
    }

    fn post(&self, path: &str, body: Value) -> Resp {
        http(self.addr, "POST", path, &[], Some(&body))
    }

    /// The journal's last entry.
    fn last(&self) -> u64 {
        self.get("/api/status").json()["seq"].as_u64().unwrap()
    }

    /// Every entry, which the journal must still have.
    fn journal(&self) -> Vec<Value> {
        let mut all = Vec::new();
        let mut after = 0;
        loop {
            let page = self.get(&format!("/api/journal?after={}&limit=4096", after));
            assert_eq!(page.status, 200, "{}", page.body);
            let page = page.json();
            let entries = page["entries"].as_array().unwrap().clone();
            if entries.is_empty() {
                return all;
            }
            after = page["next"].as_u64().unwrap();
            all.extend(entries);
        }
    }

    /// Paused at PROBE.COM's first instruction: its code segment.
    fn at_probe_entry(&self) -> String {
        self.at_entry("PROBE.COM")
    }

    /// Paused at `program`'s first instruction: its code segment.
    fn at_entry(&self, program: &str) -> String {
        let stop = self.post("/api/control/run", json!({"command": program, "stop_at_entry": true}));
        assert_eq!(stop.status, 200, "{}", stop.body);
        stop.json()["registers"]["cs"].as_str().unwrap().to_string()
    }

    /// Wait for the machine to be paused.
    fn until_paused(&self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.get("/api/status").json()["stop"].is_null() {
            assert!(Instant::now() < deadline, "the machine stops");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Wait until no request or stream is in flight on the server.
    fn until_idle(&self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.shared.busy.load(std::sync::atomic::Ordering::Acquire) > 0 {
            assert!(Instant::now() < deadline, "requests in flight: {}", self.shared.busy.load(std::sync::atomic::Ordering::Acquire));
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Quit, and wait for the emulator thread to end.
    fn quit(&mut self) -> Resp {
        let quit = self.post("/api/control/quit", json!({}));
        if let Some(emulator) = self.emulator.take() {
            emulator.join().unwrap();
        }
        quit
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        if self.emulator.is_none() {
            return;
        }
        let addr = self.addr;
        let _ = std::panic::catch_unwind(move || http(addr, "POST", "/api/control/quit", &[], Some(&json!({}))));
        if let Some(emulator) = self.emulator.take() {
            let _ = emulator.join();
        }
    }
}

/// The machine's loop, as the front end runs it: requests, save states and
/// speeds (kept in memory), then a batch, until a quit.
fn run_machine(dir: std::path::PathBuf, rx: mpsc::Receiver<super::super::Request>, shared: Arc<Shared>, options: Options) {
    // Made before the machine, whose BIOS reads the clock.
    let mut mode = options.deterministic.map(|_| rust_dos::deterministic::Deterministic::new(rust_dos::deterministic::default_start()));
    let mut cpu = Cpu::new(dir);
    if let Some(layout) = options.typing_layout {
        cpu.bus.kbd.layout = layout;
    }
    if mode.is_some() {
        cpu.bus.set_cycles_per_ms(3000);
    }
    cpu.load_shell();
    let hook = shared.clone();
    cpu.bus.log_hook = Some(Box::new(move |line: &str| hook.log_line(line)));
    let mut hub = DebugHub::new(Some(rx), Some(shared), 1000);
    #[cfg(feature = "mcp")]
    { hub.input_steps = options.input_steps.clone(); }
    hub.deterministic = mode.as_ref().map(|m| m.start);
    hub.announce(&cpu);
    if mode.is_some() {
        hub.pause_at_start(&cpu);
    }
    let mut states: std::collections::HashMap<std::path::PathBuf, Vec<u8>> = Default::default();
    // The Stats page's numbers, recorded each frame as the front end does.
    let mut stats = rust_dos::stats::Stats::new();
    let mut frame_start = Instant::now();
    let mut playback_cycles = crate::timer::CpuSpeed::Fixed(3000);
    let mut playback_pacer = crate::timer::Pacer::new(playback_cycles, None, Instant::now());
    let mut fast_forward = crate::playback::FastForward::default();
    loop {
        if options.actor_gate.as_ref().is_some_and(|gate| !gate.load(std::sync::atomic::Ordering::Acquire)) {
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        if let Some(action) = &options.frontend_action {
            match action.swap(0, std::sync::atomic::Ordering::AcqRel) {
                #[cfg(feature = "mcp")]
                8 => {
                    for hold in &mut hub.owned_input.holds {
                        hold.deadline = Instant::now();
                    }
                }
                1 => { hub.divert = true; hub.release_keys(&mut cpu); }
                3 => { hub.divert = true; }
                4 => { hub.rewinding = true; }
                5 => { rust_dos::interrupts::int10::set_mode(&mut cpu, 0x13); }
                mode @ (6 | 7) => {
                    let number = if mode == 6 { 0x101 } else { 0x111 };
                    let mode = rust_dos::video::vbe::find_mode(number, false).unwrap();
                    rust_dos::interrupts::vbe::enter_mode(&mut cpu.bus, &mode, false, false);
                }
                2 => {
                    let state = rust_dos::savestate::machine::save(&cpu);
                    rust_dos::savestate::machine::load(&mut cpu, &state).unwrap();
                    hub.state_loaded_elsewhere(&cpu, "rewind");
                }
                _ => {}
            }
        }
        hub.poll(&mut cpu);
        if let Some(observed) = &options.observed_keyboard_word {
            if let Some(word) = cpu.bus.keyboard_buffer.back() {
                observed.store(usize::from(*word), std::sync::atomic::Ordering::Release);
            }
        }
        if let Some(capacity) = &options.observed_input_capacity {
            capacity.store(hub.input.capacity(), std::sync::atomic::Ordering::Release);
        }
        if hub.quit_requested() {
            if let Some(buttons) = &options.shutdown_mouse {
                buttons.store(cpu.bus.mouse.buttons as usize, std::sync::atomic::Ordering::Release);
            }
            if let Some(flags) = &options.shutdown_flags {
                flags.store(cpu.bus.read_8(0x0417) as usize, std::sync::atomic::Ordering::Release);
            }
            hub.shutdown(&cpu, ShutdownReason::Quit, 0);
            std::thread::sleep(options.linger);
            return;
        }
        while let Some(request) = hub.next_state_request() {
            let path = request.path.clone();
            let result = if request.load {
                match states.get(&path) {
                    Some(data) => match rust_dos::savestate::machine::load(&mut cpu, data) {
                        Ok(()) => {
                            if let Some(mode) = &mut mode {
                                mode.resync(&cpu);
                            }
                            Ok(json!({"loaded": path}))
                        }
                        Err(e) => Err((400, json!({"error": e.to_string()}))),
                    },
                    None => Err((404, json!({"error": "no such state", "unchanged": true}))),
                }
            } else {
                states.insert(path.clone(), rust_dos::savestate::machine::save(&cpu));
                Ok(json!({"saved": path}))
            };
            hub.state_done(&cpu, request, result);
        }
        while let Some(request) = hub.next_playback_request() {
            let result = match &request.action {
                crate::playback::Action::Speed(cycles) => {
                    let previous = playback_cycles;
                    cycles.as_deref().map(crate::timer::CpuSpeed::parse).transpose().and_then(|cycles| {
                        if let Some(cycles) = cycles {
                            if mode.is_some() && !matches!(cycles, crate::timer::CpuSpeed::Fixed(_)) {
                                return Err("deterministic mode runs at a fixed speed".into());
                            }
                            playback_cycles = cycles;
                            cpu.bus.set_cycles_per_ms(cycles.initial_cycles());
                            playback_pacer.set_speed(cycles);
                        }
                        Ok(json!({"previous_cycles":previous.to_string(),"cycles":playback_cycles.to_string(),"cycles_per_ms":cpu.bus.clock.cycles_per_ms()}))
                    })
                }
                crate::playback::Action::FastForward(action) => fast_forward.request(*action, &mut cpu, &mut playback_pacer, Instant::now(), hub.paused || hub.divert || hub.rewinding, mode.is_some()),
            };
            hub.playback_done(&cpu, request, result);
        }
        fast_forward.poll(&mut cpu, &mut playback_pacer, Instant::now(), hub.paused || hub.divert || hub.rewinding);
        if !hub.paused {
            let hot = hub.begin_batch(&cpu);
            match (&mut mode, options.deterministic) {
                (Some(mode), Some(frame_ms)) => {
                    let target = cpu.bus.clock.icount_at_ns(cpu.bus.clock.now_ns() + frame_ms * 1_000_000);
                    let end = mode.frame_end(&cpu, target);
                    hub.run_deterministic(&mut cpu, mode, hot, end, |cpu, hub, reached| {
                        if reached.tick {
                            hub.feed_input(cpu);
                        }
                    });
                }
                _ => {
                    let end = cpu.bus.clock.icount + options.frame;
                    cpu.bus.start_batch(end);
                    while cpu.bus.clock.icount < end {
                        match exec::run_batch(&mut cpu, &mut hub, hot) {
                            StopReason::BatchEnd | StopReason::Paused | StopReason::Exit | StopReason::Hook => break,
                            StopReason::ShellReloaded => cpu.bus.start_batch(end),
                        }
                    }
                }
            }
        }
        hub.end_batch(&cpu);
        let busy = frame_start.elapsed();
        if let Some(observed) = &options.observed_hotkeys {
            observed.fetch_or(hub.hotkey as usize | ((hub.sound_viz_key as usize) << 1), std::sync::atomic::Ordering::Release);
        }
        if let Some(observed) = &options.observed_modifiers {
            observed.fetch_or(1 << (cpu.bus.read_8(0x0417) & 15), std::sync::atomic::Ordering::Release);
        }
        if let Some(observed) = &options.observed_power_ons {
            observed.store(cpu.bus.power_ons as usize, std::sync::atomic::Ordering::Release);
        }
        if hub.wants_frame()
            && options
                .capture_gate
                .as_ref()
                .is_none_or(|gate| !gate.load(std::sync::atomic::Ordering::Acquire))
        {
            let (width, height) = crate::video::frame_size(&cpu.bus);
            let mut frame = crate::video::Frame::new(width, height);
            crate::video::render_screen(&mut frame, &cpu.bus);
            crate::video::overlay::draw_cursors(&mut frame, &cpu.bus, true);
            hub.capture_frame(&frame);
        }
        std::thread::sleep(options.sleep);
        let wall = frame_start.elapsed();
        frame_start = Instant::now();
        stats.record(&cpu.bus, rust_dos::stats::FrameTimes { wall, busy, ..Default::default() });
        hub.set_frame_stats(stats.latest());
    }
}

/// An HTTP response.
struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Resp {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("{}: {}", e, self.body))
    }
}

/// `method path` with `headers` and a JSON `body`.
fn http(addr: SocketAddr, method: &str, path: &str, headers: &[(&str, &str)], body: Option<&Value>) -> Resp {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let body = body.map(Value::to_string).unwrap_or_default();
    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Length: {}\r\n", body.len());
    if !body.is_empty() {
        request.push_str("Content-Type: application/json\r\n");
    }
    for (k, v) in headers {
        request.push_str(&format!("{k}: {v}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(&body);
    stream.write_all(request.as_bytes()).unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    let raw = String::from_utf8_lossy(&raw).to_string();
    let (head, rest) = raw.split_once("\r\n\r\n").expect("a response");
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
    let headers: Vec<(String, String)> =
        lines.filter_map(|l| l.split_once(": ")).map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let chunked = headers.iter().any(|(k, v)| k.eq_ignore_ascii_case("transfer-encoding") && v.contains("chunked"));
    let body = if chunked { dechunk(rest) } else { rest.to_string() };
    Resp { status, headers, body }
}

fn dechunk(mut s: &str) -> String {
    let mut out = String::new();
    while let Some((size, rest)) = s.split_once("\r\n") {
        let n = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
        if n == 0 {
            break;
        }
        out.push_str(&rest[..n]);
        s = &rest[n + 2..];
    }
    out
}

type Socket = tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>;

fn ws(addr: SocketAddr, path: &str) -> Socket {
    let (socket, _) = tungstenite::connect(format!("ws://{}{}", addr, path)).unwrap();
    if let tungstenite::stream::MaybeTlsStream::Plain(s) = socket.get_ref() {
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    }
    socket
}

/// The next JSON message, or None once the socket closed.
fn next(socket: &mut Socket) -> Option<Value> {
    loop {
        match socket.read() {
            Ok(tungstenite::Message::Text(t)) => {
                return Some(serde_json::from_str(t.as_str()).unwrap());
            }
            Ok(tungstenite::Message::Close(_)) => return None,
            Ok(_) => continue,
            Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                return None;
            }
            Err(e) => panic!("{}", e),
        }
    }
}

/// The JSON keys of an object.
fn keys(v: &Value) -> Vec<String> {
    v.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default()
}

#[test]
fn input_with_a_field_it_does_not_have_is_refused() {
    let rig = Rig::new("input-fields");
    let before = rig.last();
    // "modifiers" for "mods": taking it would press the key without Alt.
    let refused = rig.post("/api/input/key?wait=false", json!({"key": "o", "modifiers": ["alt"]}));
    assert_eq!(refused.status, 400, "{}", refused.body);
    let error = refused.json()["error"].as_str().unwrap().to_string();
    assert!(error.contains("`modifiers`") && error.contains("(did you mean `mods`?)"), "{}", error);
    let batch = json!({"events": [{"type": "type", "text": "x"}, {"type": "mouse", "action": "click", "x": 1, "y": 2, "z": 3}]});
    let refused = rig.post("/api/input/batch?wait=false", batch);
    assert_eq!(refused.status, 400, "{}", refused.body);
    assert!(refused.json()["error"].as_str().unwrap().starts_with("events[1]: "), "{}", refused.body);
    assert_eq!(rig.post("/api/input/wait?wait=false", json!({"ms": 1, "for": 2})).status, 400);
    // A field beside the events.
    let beside = json!({"events": [{"type": "type", "text": "x"}], "wiat": false});
    assert_eq!(rig.post("/api/input/batch?wait=false", beside).status, 400);
    // The socket takes off its own fields and refuses the rest.
    let mut input = ws(rig.addr, "/ws/input");
    for msg in [
        json!({"events": [{"type": "key", "key": "o", "modifiers": ["alt"]}], "wait": false, "request_id": "r"}),
        json!({"type": "key", "key": "o", "wiat": false}),
        json!({"events": [{"type": "key", "key": "o"}], "wait": "false"}),
    ] {
        input.send(tungstenite::Message::Text(msg.to_string().into())).unwrap();
        let reply = read_entries(&mut input, 1, Duration::from_secs(5));
        assert!(reply[0]["ok"] == false && reply[0]["error"].is_string(), "{}: {:?}", msg, reply);
    }
    let queued = || rig.get(&format!("/api/journal?after={}&type=input_queued", before)).json()["entries"].as_array().map(Vec::len);
    assert_eq!(queued(), Some(0), "nothing was queued");
    let taken = rig.post("/api/input/key?wait=false", json!({"key": "o", "mods": ["alt"]}));
    assert_eq!(taken.status, 200, "{}", taken.body);
    assert_eq!(queued(), Some(1), "the key with mods was queued");
}

#[test]
fn other_bodies_with_a_field_they_do_not_have_are_refused() {
    let rig = Rig::new("body-fields");
    // "adr" for "addr" would otherwise be a breakpoint request without an
    // address, and a DELETE without one removes them all.
    let refused = rig.post("/api/breakpoints", json!({"adr": "1234:0100"}));
    assert_eq!(refused.status, 400, "{}", refused.body);
    assert!(refused.json()["error"].as_str().unwrap().ends_with("(did you mean `addr`?)"), "{}", refused.body);
    let added = rig.post("/api/breakpoints", json!({"addr": "1234:0100"}));
    assert_eq!(added.status, 200, "{}", added.body);
    assert_eq!(http(rig.addr, "DELETE", "/api/breakpoints?adr=1234:0100", &[], None).status, 400);
    assert_eq!(rig.get("/api/breakpoints").json()["breakpoints"].as_array().map(Vec::len), Some(1), "the breakpoint stays");
    let refused = rig.post("/api/control/step", json!({"cnt": 5}));
    assert_eq!(refused.status, 400, "{}", refused.body);
    assert_eq!(rig.post("/api/speed", json!({"cycles": "max", "turbo": true})).status, 400);
}

#[test]
fn every_response_has_its_request_number_and_the_client_s_id() {
    let rig = Rig::new("ids");
    let a = rig.get("/api/status");
    let b = http(rig.addr, "GET", "/api/status", &[("X-Request-Id", "agent-7 step 3")], None);
    let (ra, rb): (u64, u64) = (a.header("x-debug-req").unwrap().parse().unwrap(), b.header("x-debug-req").unwrap().parse().unwrap());
    assert!(rb > ra);
    assert_eq!(b.header("x-request-id"), Some("agent-7 step 3"));
    assert_eq!(b.json()["req"].as_u64(), Some(rb));
    assert!(b.json()["state_seq"].is_u64() && b.header("x-debug-state-seq").is_some());
    assert!(b.body.starts_with(&format!("{{\"req\":{},", rb)), "req first: {}", b.body);
    let long = "x".repeat(129);
    assert_eq!(http(rig.addr, "GET", "/api/status", &[("X-Request-Id", &long)], None).status, 400);
    assert_eq!(http(rig.addr, "GET", "/api/status", &[("If-State-Seq", "soon")], None).status, 400);
    // The server's own endpoints have no machine state to hold it to.
    assert_eq!(http(rig.addr, "GET", "/api/journal", &[("If-State-Seq", "3")], None).status, 400);
    let version = rig.get("/api/version").json();
    assert_eq!(version["api_version"], 1);
    assert_eq!(version["instance"].as_str().map(str::len), Some(16));
    assert!(version["features"].as_array().unwrap().contains(&json!("journal")));
    assert_eq!(version["build"], rust_dos::build_info::json());
    for key in ["commit", "commit_short", "dirty"] {
        assert!(version["build"].get(key).is_some(), "build.{} even when unknown", key);
    }
    // The client's ID is in the entries its request caused.
    let set = http(rig.addr, "POST", "/api/breakpoints", &[("X-Request-Id", "bp-1")], Some(&json!({"addr": "0100:0000"})));
    let entry = rig.get(&format!("/api/journal?after={}", set.json()["seq"].as_u64().unwrap() - 1)).json()["entries"][0].clone();
    assert_eq!((entry["type"].as_str(), entry["request_id"].as_str()), (Some("breakpoint_set"), Some("bp-1")));
    assert_eq!(entry["cause"], set.json()["req"]);
}

#[test]
fn native_observation_http_parity() {
    let mut rig = Rig::new("observation-parity");
    let status = http(
        rig.addr,
        "GET",
        "/api/status",
        &[("X-Request-Id", "observation-parity")],
        None,
    );
    assert_eq!(status.status, 200);
    assert_eq!(status.header("x-request-id"), Some("observation-parity"));
    let request: u64 = status.header("x-debug-req").unwrap().parse().unwrap();
    assert_eq!(status.json()["req"], request);
    assert_eq!(
        status.json()["state_seq"].as_u64(),
        status.header("x-debug-state-seq").unwrap().parse().ok()
    );

    let typed = rig.post(
        "/api/input/type?wait=true",
        json!({"text":"echo MCP_SHELL_OBS_01"}),
    );
    let entered = rig.post("/api/input/key?wait=true", json!({"key":"enter"}));
    assert_eq!((typed.status, entered.status), (200, 200));
    assert_eq!(typed.json()["done"], true);
    assert_eq!(entered.json()["done"], true);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let screen = rig.get("/api/screen/text").json();
        if screen["lines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|line| line == "MCP_SHELL_OBS_01")
        {
            break;
        }
        assert!(Instant::now() < deadline, "actual shell echo appears");
        std::thread::sleep(Duration::from_millis(5));
    }
    let journal = rig.journal();
    for response in [typed.json(), entered.json()] {
        let operation = response["op"].as_u64().unwrap();
        assert!(
            journal
                .iter()
                .any(|entry| entry["type"] == "input_queued" && entry["cause"] == operation)
        );
        assert!(
            journal
                .iter()
                .any(|entry| entry["type"] == "input_delivered" && entry["cause"] == operation)
        );
    }
    let mut image = ureq::get(&format!("http://{}/api/screenshot", rig.addr))
        .call()
        .unwrap();
    assert_eq!(image.headers().get("content-type").unwrap(), "image/png");
    let png = image
        .body_mut()
        .with_config()
        .limit(2 * 1024 * 1024)
        .read_to_vec()
        .unwrap();
    let mut decoder = png::Decoder::new(std::io::Cursor::new(png));
    decoder.set_limits(png::Limits {
        bytes: 2 * 1024 * 1024,
    });
    let mut reader = decoder.read_info().unwrap();
    assert_eq!((reader.info().width, reader.info().height), (640, 400));
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    reader.next_frame(&mut pixels).unwrap();
    assert!(pixels.iter().any(|&byte| byte != 0), "actual text pixels");
    rig.until_idle();
    assert_eq!(
        rig.shared.busy.load(std::sync::atomic::Ordering::Acquire),
        0
    );
    assert_eq!(rig.quit().json()["shutting_down"], true);
    assert!(rig.shared.closed.load(std::sync::atomic::Ordering::Acquire));
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_discovery_and_shell_capture() {
    use base64::Engine;
    use rmcp::{
        model::{CallToolRequestParams, ClientConfig, ProtocolVersion},
        service::{ClientLifecycleMode, ClientServiceExt},
        transport::StreamableHttpClientTransport,
    };
    let rig = Rig::with(
        "native-capture",
        Options {
            mcp: true,
            ..Options::default()
        },
    );
    assert_eq!(rig.post("/api/control/pause", json!({})).status, 200);
    rig.until_paused();
    let mut http_image = ureq::get(&format!("http://{}/api/screenshot", rig.addr))
        .call()
        .unwrap();
    let expected = http_image
        .body_mut()
        .with_config()
        .limit(2 * 1024 * 1024)
        .read_to_vec()
        .unwrap();
    let decode = |bytes: Vec<u8>| {
        let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        decoder.set_limits(png::Limits {
            bytes: 2 * 1024 * 1024,
        });
        let mut reader = decoder.read_info().unwrap();
        let size = (reader.info().width, reader.info().height);
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        reader.next_frame(&mut pixels).unwrap();
        (size, pixels)
    };
    let expected = decode(expected);
    assert_eq!(expected.0, (640, 400));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        for version in [ProtocolVersion::V_2025_11_25, ProtocolVersion::V_2026_07_28] {
            let lifecycle = if version.has_initialize() {
                ClientLifecycleMode::Initialize
            } else {
                ClientLifecycleMode::Discover {
                    preferred_versions: vec![version.clone()],
                }
            };
            let client = ClientConfig::default()
                .with_protocol_version(version.clone())
                .serve_with_lifecycle(
                    StreamableHttpClientTransport::from_uri(format!("http://{}/mcp", rig.addr)),
                    lifecycle,
                )
                .await
                .unwrap();
            assert_eq!(client.peer_info().unwrap().protocol_version, version);
            let tools = client.list_all_tools().await.unwrap();
            for tool in &tools {
                if matches!(tool.name.as_ref(), "status" | "capture") {
                    assert_eq!(tool.input_schema.len(), 2, "observation schema is an empty object");
                }
                assert_eq!(
                    tool.input_schema.get("type").and_then(serde_json::Value::as_str),
                    Some("object")
                );
                assert_eq!(
                    tool.input_schema
                        .get("additionalProperties")
                        .and_then(serde_json::Value::as_bool),
                    Some(false)
                );
            }
            let mut names: Vec<_> = tools
                .iter()
                .map(|tool| tool.name.as_ref())
                .collect();
            names.sort();
            assert_eq!(
                names,
                ["capture", "fast_forward", "key", "mouse", "operation", "speed", "status", "type"],
                "only implemented native tools are advertised"
            );
            let status = client
                .call_tool(CallToolRequestParams::new("status"))
                .await
                .unwrap();
            assert_eq!(
                status.structured_content.as_ref().unwrap()["instance"],
                rig.shared.instance_hex()
            );
            assert_eq!(status.structured_content.as_ref().unwrap()["paused"], true);
            let capture = client
                .call_tool(CallToolRequestParams::new("capture"))
                .await
                .unwrap();
            assert_ne!(capture.is_error, Some(true));
            let image = capture
                .content
                .iter()
                .find_map(|content| content.as_image())
                .expect("a native image, not a host path");
            assert_eq!(image.mime_type, "image/png");
            let actual = decode(
                base64::engine::general_purpose::STANDARD
                    .decode(&image.data)
                    .unwrap(),
            );
            assert_eq!(
                actual, expected,
                "the real paused DOS picture matches HTTP capture"
            );
            let metadata = capture.structured_content.as_ref().unwrap();
            assert_eq!(metadata["width"], actual.0.0);
            assert_eq!(metadata["height"], actual.0.1);
            assert_eq!(metadata["instance"], rig.shared.instance_hex());
            assert!(metadata["req"].is_u64() && metadata["state_seq"].is_u64());
            client.cancel().await.unwrap();
        }
    });
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_incomplete_uploads_expire_and_restore_admission() {
    let rig = Rig::with(
        "native-upload-timeout",
        Options {
            mcp: true,
            ..Options::default()
        },
    );
    let mut uploads = Vec::new();
    for _ in 0..16 {
        let mut stream = TcpStream::connect(rig.addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(4)))
            .unwrap();
        stream.write_all(format!("POST /mcp HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{{", rig.addr).as_bytes()).unwrap();
        uploads.push(stream);
    }
    for mut stream in uploads {
        let mut reply = String::new();
        stream
            .read_to_string(&mut reply)
            .expect("incomplete upload must terminate within its native deadline");
        assert!(reply.starts_with("HTTP/1.1 408"), "{reply}");
    }
    use rmcp::{
        model::{CallToolRequestParams, ClientConfig, ProtocolVersion},
        service::{ClientLifecycleMode, ClientServiceExt},
        transport::StreamableHttpClientTransport,
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let client = ClientConfig::default()
            .with_protocol_version(ProtocolVersion::V_2026_07_28)
            .serve_with_lifecycle(
                StreamableHttpClientTransport::from_uri(format!("http://{}/mcp", rig.addr)),
                ClientLifecycleMode::Discover { preferred_versions: vec![ProtocolVersion::V_2026_07_28] },
            )
            .await
            .expect("admission recovers after uploads expire");
        assert_eq!(client.peer_info().unwrap().protocol_version, ProtocolVersion::V_2026_07_28);
        let status = client
            .call_tool(CallToolRequestParams::new("status"))
            .await
            .unwrap();
        assert_ne!(status.is_error, Some(true));
        client.cancel().await.unwrap();
    });
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_invalid_observation_calls_reject_before_actor_allocation() {
    use rmcp::{model::{CallToolRequestParams, ClientConfig, ProtocolVersion}, service::{ClientServiceExt, ClientLifecycleMode}, transport::StreamableHttpClientTransport};
    let rig = Rig::with("native-invalid-observation", Options { mcp: true, ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = ClientConfig::default().with_protocol_version(ProtocolVersion::V_2026_07_28)
            .serve_with_lifecycle(StreamableHttpClientTransport::from_uri(format!("http://{}/mcp", rig.addr)), ClientLifecycleMode::Discover { preferred_versions: vec![ProtocolVersion::V_2026_07_28] }).await.unwrap();
        assert_eq!(client.peer_info().unwrap().protocol_version, ProtocolVersion::V_2026_07_28);
        let tools = client.list_all_tools().await.unwrap();
        let actual = client.call_tool(CallToolRequestParams::new("status")).await.unwrap();
        let mut arguments = actual.structured_content.unwrap().as_object().unwrap().clone();
        arguments.retain(|key, _| key == "req");
        assert_eq!(arguments.len(), 1, "derive invalid arguments from actual observation");
        let before = rig.shared.next_req.load(std::sync::atomic::Ordering::Acquire);
        for tool in &tools {
            let mut request = CallToolRequestParams::new(tool.name.clone());
            request.arguments = Some(arguments.clone());
            assert!(matches!(client.call_tool(request).await, Err(rmcp::ServiceError::McpError(error)) if error.code == rmcp::model::ErrorCode::INVALID_PARAMS));
            assert_eq!(rig.shared.next_req.load(std::sync::atomic::Ordering::Acquire), before, "invalid arguments allocate no actor request");
        }
        let unknown = format!("{}x", tools[0].name);
        assert!(matches!(client.call_tool(CallToolRequestParams::new(unknown)).await, Err(rmcp::ServiceError::McpError(error)) if error.code == rmcp::model::ErrorCode::INVALID_PARAMS));
        assert_eq!(rig.shared.next_req.load(std::sync::atomic::Ordering::Acquire), before, "unknown tool allocates no actor request");
        client.cancel().await.unwrap();
    });
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_duplicate_legacy_request_preserves_original_capture() {
    use rmcp::{model::{CallToolRequestParams, ClientConfig, ProtocolVersion}, service::{ClientServiceExt, ClientLifecycleMode}, transport::{StreamableHttpClientTransport, streamable_http_client::{StreamableHttpClient, StreamableHttpClientTransportConfig}}};
    let gate = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let rig = Rig::with("native-duplicate-request", Options { mcp: true, capture_gate: Some(gate.clone()), ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let recorded = recorded_sdk::Client::default();
        let transport = StreamableHttpClientTransport::with_client(recorded.clone(), StreamableHttpClientTransportConfig::with_uri(format!("http://{}/mcp", rig.addr)));
        let client = ClientConfig::default().with_protocol_version(ProtocolVersion::V_2025_11_25)
            .serve_with_lifecycle(transport, ClientLifecycleMode::Initialize).await.unwrap();
        let before = rig.shared.next_req.load(std::sync::atomic::Ordering::Acquire);
        let peer = client.peer().clone();
        let original = tokio::spawn(async move { peer.call_tool(CallToolRequestParams::new("capture")).await });
        let deadline = Instant::now() + Duration::from_secs(1);
        while rig.shared.next_req.load(std::sync::atomic::Ordering::Acquire) == before {
            assert!(Instant::now() < deadline, "original capture enters actor service");
            tokio::task::yield_now().await;
        }
        let admitted = rig.shared.next_req.load(std::sync::atomic::Ordering::Acquire);
        let post = recorded.capture.lock().unwrap().clone().expect("actual SDK capture request recorded");
        assert!(post.session.is_some(), "actual legacy session identity");
        let duplicate = recorded.inner.post_message(post.uri, post.message, post.session, None, post.headers).await;
        assert!(duplicate.is_err(), "active duplicate SDK request must be rejected");
        assert_eq!(rig.shared.next_req.load(std::sync::atomic::Ordering::Acquire), admitted, "duplicate admits no second actor operation");
        gate.store(false, std::sync::atomic::Ordering::Release);
        let capture = original.await.unwrap().unwrap();
        assert_ne!(capture.is_error, Some(true), "original capture ticket survives duplicate rejection");
        assert!(capture.content.iter().any(|content| content.as_image().is_some()));
        let next = client.call_tool(CallToolRequestParams::new("capture")).await.unwrap();
        assert_ne!(next.is_error, Some(true), "terminal delivery restores capture admission: {:?}", next.content.iter().filter_map(|content| content.as_text()).map(|text| text.text.as_str()).collect::<Vec<_>>());
        client.cancel().await.unwrap();
    });
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_continuous_capture_pressure_preserves_status_and_guest_progress() {
    use rmcp::{model::{CallToolRequestParams, ClientConfig, ProtocolVersion}, service::{ClientServiceExt, ClientLifecycleMode}, transport::StreamableHttpClientTransport};
    let rig = Rig::with("native-capture-fairness", Options { mcp: true, ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build().unwrap();
    runtime.block_on(async {
        for version in [ProtocolVersion::V_2025_11_25, ProtocolVersion::V_2026_07_28] {
            let lifecycle = if version.has_initialize() {
                ClientLifecycleMode::Initialize
            } else {
                ClientLifecycleMode::Discover { preferred_versions: vec![version.clone()] }
            };
            let client = ClientConfig::default().with_protocol_version(version.clone())
                .serve_with_lifecycle(StreamableHttpClientTransport::from_uri(format!("http://{}/mcp", rig.addr)), lifecycle).await.unwrap();
            assert_eq!(client.peer_info().unwrap().protocol_version, version, "pressure must exercise the requested protocol");
            let first = client.call_tool(CallToolRequestParams::new("status")).await.unwrap();
            let before = first.structured_content.unwrap()["instructions"].as_u64().unwrap();
            let end = Instant::now() + Duration::from_millis(500);
            let mut workers = Vec::new();
            for _ in 0..8 {
                let peer = client.peer().clone();
                workers.push(tokio::spawn(async move {
                    let mut completed = 0;
                    let mut busy = 0;
                    while Instant::now() < end {
                        let capture = peer.call_tool(CallToolRequestParams::new("capture")).await.unwrap();
                        if capture.is_error == Some(true) {
                            assert_eq!(capture.structured_content.unwrap()["error"]["code"], "capture_busy", "pressure rejects before actor admission");
                            busy += 1;
                        } else {
                            assert!(capture.content.iter().any(|content| content.as_image().is_some()), "actual guest capture under pressure");
                            completed += 1;
                        }
                    }
                    (completed, busy)
                }));
            }
            let mut samples = 0;
            let mut max_status = Duration::ZERO;
            let mut after = before;
            while Instant::now() < end {
                let start = Instant::now();
                let status = tokio::time::timeout(Duration::from_secs(1), client.call_tool(CallToolRequestParams::new("status"))).await.expect("status stays within ADR caller budget under continuous capture pressure").unwrap();
                assert_ne!(status.is_error, Some(true), "status under capture pressure: {:?}; service timings: {}", status.structured_content, rig.shared.metrics.snapshot());
                max_status = max_status.max(start.elapsed());
                after = status.structured_content.unwrap()["instructions"].as_u64().unwrap();
                samples += 1;
            }
            let mut completed = 0;
            let mut busy = 0;
            for worker in workers { let (ok, rejected) = worker.await.unwrap(); completed += ok; busy += rejected; }
            assert!(samples > 1 && completed > 0 && busy > 0, "exercise simultaneous actual status, capture and quota rejection");
            assert!(after > before, "guest instructions advance under continuously ready capture traffic");
            println!("native fairness {version}: status_samples={samples} max_status_us={} captures={completed} busy={busy} guest_delta={}", max_status.as_micros(), after - before);
            client.cancel().await.unwrap();
        }
    });
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_diagnostics_keep_context_without_guest_payloads() {
    let root = std::env::temp_dir().join(format!("rust-dos-native-privacy-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    for mode in ["filtered", "raw-sdk-sensitivity"] {
        let stdout = root.join(format!("{mode}.stdout"));
        let stderr = root.join(format!("{mode}.stderr"));
        let payload = root.join(format!("{mode}.payload"));
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "debug::server::http_tests::native_mcp_diagnostic_privacy_worker", "--nocapture"])
            .env("RUST_DOS_MCP_PRIVACY_MODE", mode)
            .env("RUST_DOS_MCP_PRIVACY_PAYLOAD", &payload)
            .stdout(std::fs::File::create(&stdout).unwrap())
            .stderr(std::fs::File::create(&stderr).unwrap())
            .spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        let status = loop {
            let output_size = std::fs::metadata(&stderr).unwrap().len()
                .saturating_add(std::fs::metadata(&stdout).unwrap().len());
            if Instant::now() >= deadline || output_size > 4 * 1024 * 1024 {
                let _ = child.kill();
                child.wait().unwrap();
                panic!("privacy worker exceeded owned time/output budget: {root:?}");
            }
            if let Some(status) = child.try_wait().unwrap() { break status; }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(status.success(), "privacy worker failed: {root:?}");
        assert!(std::fs::metadata(&stderr).unwrap().len().saturating_add(std::fs::metadata(&stdout).unwrap().len()) <= 4 * 1024 * 1024, "terminal privacy output exceeded budget: {root:?}");
        let logs = std::fs::read_to_string(&stderr).unwrap() + &std::fs::read_to_string(&stdout).unwrap();
        let image = std::fs::read_to_string(&payload).unwrap();
        assert!(image.len() > 128, "actual guest image must exist");
        let private = !logs.contains("MCP_PRIVACY_GUEST_01") && !logs.contains("MCP_PRIVACY_INPUT_01") && !logs.contains(&image[..128]);
        if mode == "filtered" {
            assert!(private, "diagnostics exposed actual guest payload: {root:?}");
            assert!(logs.contains("observation completed") && logs.contains("capture") && logs.contains("request="), "context diagnostics missing");
        } else {
            assert!(!private, "raw SDK tracing mutation must expose the actual capture and fail the same privacy oracle");
            assert!(logs.contains("MCP_PRIVACY_INPUT_01"), "raw SDK tracing must also expose actual native input to prove input sensitivity");
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(feature = "mcp")]
#[test]
#[ignore = "run by the bounded privacy subprocess test"]
fn native_mcp_diagnostic_privacy_worker() {
    use rmcp::{model::{CallToolRequestParams, ClientConfig, ProtocolVersion}, service::{ClientServiceExt, ClientLifecycleMode}, transport::StreamableHttpClientTransport};
    if std::env::var("RUST_DOS_MCP_PRIVACY_MODE").unwrap() == "filtered" {
        super::super::init_mcp_diagnostics().unwrap();
    } else {
        tracing::subscriber::set_global_default(tracing_subscriber::fmt().without_time().with_ansi(false).with_max_level(tracing::Level::TRACE).with_writer(std::io::stderr).finish()).unwrap();
    }
    let rig = Rig::with("native-diagnostic-privacy", Options { mcp: true, ..Options::default() });
    assert_eq!(rig.post("/api/input/type?wait=true", json!({"text":"echo MCP_PRIVACY_GUEST_01"})).status, 200);
    assert_eq!(rig.post("/api/input/key?wait=true", json!({"key":"enter"})).status, 200);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !rig.get("/api/screen/text?format=json").body.contains("MCP_PRIVACY_GUEST_01") {
        assert!(Instant::now() < deadline, "actual guest text not observed");
        std::thread::sleep(Duration::from_millis(10));
    }
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = ClientConfig::default().with_protocol_version(ProtocolVersion::V_2026_07_28)
            .serve_with_lifecycle(StreamableHttpClientTransport::from_uri(format!("http://{}/mcp", rig.addr)), ClientLifecycleMode::Discover { preferred_versions: vec![ProtocolVersion::V_2026_07_28] }).await.unwrap();
        assert_eq!(client.peer_info().unwrap().protocol_version, ProtocolVersion::V_2026_07_28);
        native_keyboard_accepted(&client, "type", json!({"text":"echo MCP_PRIVACY_INPUT_01"})).await;
        native_keyboard_accepted(&client, "key", json!({"key":"enter"})).await;
        let deadline = Instant::now() + Duration::from_secs(5);
        while !rig.get("/api/screen/text?format=json").body.contains("MCP_PRIVACY_INPUT_01") {
            assert!(Instant::now() < deadline, "actual native input not observed");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let capture = client.call_tool(CallToolRequestParams::new("capture")).await.unwrap();
        assert_ne!(capture.is_error, Some(true));
        let image = capture.content.iter().find_map(|content| content.as_image()).unwrap();
        std::fs::write(std::env::var_os("RUST_DOS_MCP_PRIVACY_PAYLOAD").unwrap(), &image.data).unwrap();
        client.cancel().await.unwrap();
    });
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_rejects_deep_actual_sdk_metadata_before_actor_admission() {
    use rmcp::{
        model::{
            CallToolRequestParams, ClientConfig, MetaObject, ProtocolVersion, RequestMetaObject,
        },
        service::{ClientLifecycleMode, ClientServiceExt},
        transport::{
            StreamableHttpClientTransport,
            streamable_http_client::StreamableHttpClientTransportConfig,
        },
    };
    let rig = Rig::with(
        "native-parser-depth",
        Options {
            mcp: true,
            ..Options::default()
        },
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let recorded = recorded_sdk::Client::default();
        let transport = StreamableHttpClientTransport::with_client(
            recorded.clone(),
            StreamableHttpClientTransportConfig::with_uri(format!("http://{}/mcp", rig.addr)),
        );
        let client = ClientConfig::default()
            .with_protocol_version(ProtocolVersion::V_2026_07_28)
            .serve_with_lifecycle(
                transport,
                ClientLifecycleMode::Discover {
                    preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                },
            )
            .await
            .unwrap();
        assert_eq!(
            client.peer_info().unwrap().protocol_version,
            ProtocolVersion::V_2026_07_28
        );
        let mut request = CallToolRequestParams::new("status");
        request.meta = Some(RequestMetaObject(MetaObject(
            json!({"validation-marker": "native-depth-probe"})
                .as_object()
                .unwrap()
                .clone(),
        )));
        assert_ne!(
            client.call_tool(request).await.unwrap().is_error,
            Some(true)
        );
        let post = recorded
            .last
            .lock()
            .unwrap()
            .clone()
            .expect("actual SDK request recorded");
        let mut headers: reqwest::header::HeaderMap = post.headers.into_iter().collect();
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            reqwest::header::HeaderValue::from_static("application/json"),
        );
        headers.insert(
            reqwest::header::ACCEPT,
            reqwest::header::HeaderValue::from_static("application/json, text/event-stream"),
        );
        if let Some(session) = post.session {
            headers.insert(
                "mcp-session-id",
                reqwest::header::HeaderValue::from_str(&session).unwrap(),
            );
        }
        let mut actual = serde_json::to_value(&post.message).unwrap();
        let original = serde_json::to_vec(&actual).unwrap();
        let control = recorded
            .inner
            .post(post.uri.as_ref())
            .headers(headers.clone())
            .body(original)
            .timeout(Duration::from_secs(3))
            .send()
            .await
            .unwrap();
        assert_eq!(
            control.status(),
            reqwest::StatusCode::OK,
            "original recorded request is accepted"
        );
        let control: Value = control.json().await.unwrap();
        assert!(
            control.get("error").is_none(),
            "control replay succeeds through the same ingress"
        );
        let before = rig.last();
        let allocated = rig
            .shared
            .next_req
            .load(std::sync::atomic::Ordering::Relaxed);
        let mut nested = actual["params"]["_meta"]["validation-marker"].take();
        assert!(
            nested.is_string(),
            "mutation starts from actual recorded metadata"
        );
        for _ in 0..160 {
            nested = Value::Array(vec![nested]);
        }
        actual["params"]["_meta"]["validation-marker"] = nested;
        let damaged = serde_json::to_vec(&actual).unwrap();
        assert!(
            damaged.len() < 64 * 1024,
            "depth fault stays below the body-byte limit"
        );
        let rejected = recorded
            .inner
            .post(post.uri.as_ref())
            .headers(headers)
            .body(damaged)
            .timeout(Duration::from_secs(3))
            .send()
            .await
            .unwrap();
        assert_eq!(
            rejected.status(),
            reqwest::StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "SDK rejects excessive JSON parser depth"
        );
        let rejection = rejected.text().await.unwrap();
        assert!(
            rejection.contains("recursion limit exceeded"),
            "rejection identifies the parser depth limit: {rejection}"
        );
        assert_eq!(
            rig.shared
                .next_req
                .load(std::sync::atomic::Ordering::Relaxed),
            allocated
        );
        assert_eq!(
            rig.last(),
            before,
            "parser rejection produces no actor journal event"
        );
        client.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_rejects_oversized_sdk_metadata() {
    use rmcp::{
        model::{
            CallToolRequestParams, ClientConfig, MetaObject, ProtocolVersion, RequestMetaObject,
        },
        service::{ClientLifecycleMode, ClientServiceExt},
        transport::StreamableHttpClientTransport,
    };
    let rig = Rig::with(
        "native-body-limit",
        Options {
            mcp: true,
            ..Options::default()
        },
    );
    let before = rig.last();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let client = ClientConfig::default()
            .with_protocol_version(ProtocolVersion::V_2026_07_28)
            .serve_with_lifecycle(
                StreamableHttpClientTransport::from_uri(format!("http://{}/mcp", rig.addr)),
                ClientLifecycleMode::Discover {
                    preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                },
            )
            .await
            .unwrap();
        let mut request = CallToolRequestParams::new("status");
        let admitted_before = rig
            .shared
            .next_req
            .load(std::sync::atomic::Ordering::Relaxed);
        request.meta = Some(RequestMetaObject(MetaObject(
            json!({"validation-marker":"MCP_SHELL_OBS_01".repeat(5000)})
                .as_object()
                .unwrap()
                .clone(),
        )));
        let response = client.call_tool(request).await;
        client.cancel().await.unwrap();
        assert!(
            response.is_err(),
            "an SDK-generated request above 64 KiB is rejected before actor admission"
        );
        assert_eq!(
            rig.shared
                .next_req
                .load(std::sync::atomic::Ordering::Relaxed),
            admitted_before
        );
    });
    assert_eq!(
        rig.last(),
        before,
        "the rejected request produces no journal event"
    );
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_is_opt_in_and_rejects_foreign_host_and_origin() {
    let ordinary = Rig::new("native-disabled");
    let disabled = ureq::get(&format!("http://{}/mcp", ordinary.addr)).call();
    assert!(matches!(disabled, Err(ureq::Error::StatusCode(404))));
    let native = Rig::with(
        "native-access",
        Options {
            mcp: true,
            ..Options::default()
        },
    );
    for (header, value) in [
        ("Origin", "http://example.invalid"),
        ("Host", "example.invalid"),
    ] {
        let result = ureq::get(&format!("http://{}/mcp", native.addr))
            .header(header, value)
            .call();
        assert!(
            matches!(result, Err(ureq::Error::StatusCode(403))),
            "{header} rejected before native service"
        );
    }
    assert_eq!(
        http(
            native.addr,
            "GET",
            "/api/status",
            &[("Host", "example.invalid")],
            None
        )
        .status,
        200,
        "legacy HTTP host policy is preserved"
    );
    let (tx, _receive) = mpsc::channel();
    assert!(
        super::spawn(
            "0.0.0.0:0".parse().unwrap(),
            tx,
            Arc::new(Shared::new(Instant::now())),
            true
        )
        .is_err()
    );
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_caps_live_legacy_sessions() {
    use rmcp::{
        model::{ClientConfig, ProtocolVersion},
        service::{ClientLifecycleMode, ClientServiceExt},
        transport::StreamableHttpClientTransport,
    };
    let rig = Rig::with(
        "native-session-limit",
        Options {
            mcp: true,
            ..Options::default()
        },
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut clients = Vec::new();
        for _ in 0..8 {
            clients.push(
                ClientConfig::default()
                    .with_protocol_version(ProtocolVersion::V_2025_11_25)
                    .serve_with_lifecycle(
                        StreamableHttpClientTransport::from_uri(format!("http://{}/mcp", rig.addr)),
                        ClientLifecycleMode::Initialize,
                    )
                    .await
                    .unwrap(),
            );
        }
        let ninth = ClientConfig::default()
            .with_protocol_version(ProtocolVersion::V_2025_11_25)
            .serve_with_lifecycle(
                StreamableHttpClientTransport::from_uri(format!("http://{}/mcp", rig.addr)),
                ClientLifecycleMode::Initialize,
            )
            .await;
        let refused = ninth.is_err();
        if let Ok(client) = ninth {
            client.cancel().await.unwrap();
        }
        for client in clients {
            client.cancel().await.unwrap();
        }
        assert!(refused, "the ninth live legacy session is refused");
    });
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_rejects_encoded_and_excessive_headers_before_sdk_work() {
    let rig = Rig::with(
        "native-header-limits",
        Options {
            mcp: true,
            ..Options::default()
        },
    );
    let uri = format!("http://{}/mcp", rig.addr);
    let encoded = ureq::get(&uri).header("Content-Encoding", "gzip").call();
    assert!(matches!(encoded, Err(ureq::Error::StatusCode(415))));
    let large = ureq::get(&uri)
        .header("X-Control-Marker", "MCP_SHELL_OBS_01".repeat(600))
        .call();
    assert!(matches!(large, Err(ureq::Error::StatusCode(431))));
    let mut request = ureq::get(&uri);
    for number in 0..33 {
        request = request.header(format!("X-Control-{number}"), "1");
    }
    assert!(matches!(request.call(), Err(ureq::Error::StatusCode(431))));
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_capture_admission_precedes_a_second_frame_request() {
    use rmcp::{
        model::{CallToolRequestParams, ClientConfig, ProtocolVersion},
        service::{ClientLifecycleMode, ClientServiceExt},
        transport::StreamableHttpClientTransport,
    };
    let gate = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let rig = Rig::with(
        "native-capture-admission",
        Options {
            mcp: true,
            capture_gate: Some(gate.clone()),
            ..Options::default()
        },
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let client = ClientConfig::default()
            .with_protocol_version(ProtocolVersion::V_2025_11_25)
            .serve_with_lifecycle(
                StreamableHttpClientTransport::from_uri(format!("http://{}/mcp", rig.addr)),
                ClientLifecycleMode::Initialize,
            )
            .await
            .unwrap();
        let first_peer = client.peer().clone();
        let before = rig
            .shared
            .next_req
            .load(std::sync::atomic::Ordering::Relaxed);
        let first = tokio::spawn(async move {
            first_peer
                .call_tool(CallToolRequestParams::new("capture"))
                .await
        });
        let wait = tokio::time::Instant::now() + Duration::from_secs(1);
        while rig
            .shared
            .next_req
            .load(std::sync::atomic::Ordering::Relaxed)
            == before
        {
            assert!(
                tokio::time::Instant::now() < wait,
                "first capture reaches the live actor"
            );
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        let admitted = rig
            .shared
            .next_req
            .load(std::sync::atomic::Ordering::Relaxed);
        let active_metrics = super::super::mcp::metrics::snapshot(&rig.shared).unwrap();
        assert_eq!(active_metrics["capture"]["inflight"], 1, "actual capture pipeline owns its slot");
        assert_eq!(active_metrics["capture"]["peak"], 1);
        let second = tokio::time::timeout(
            Duration::from_millis(500),
            client.call_tool(CallToolRequestParams::new("capture")),
        )
        .await;
        gate.store(false, std::sync::atomic::Ordering::Release);
        let first = first.await.unwrap().unwrap();
        client.cancel().await.unwrap();
        assert_ne!(
            first.is_error,
            Some(true),
            "first request still obtains the real frame"
        );
        assert!(
            matches!(second, Ok(Ok(ref result)) if result.is_error == Some(true)),
            "overlapping capture is rejected immediately"
        );
        assert_eq!(
            rig.shared
                .next_req
                .load(std::sync::atomic::Ordering::Relaxed),
            admitted,
            "rejected capture never enters the actor"
        );
        assert_eq!(super::super::mcp::metrics::snapshot(&rig.shared).unwrap()["capture"]["inflight"], 0, "actual capture capacity retired");
    });
}

#[cfg(feature = "mcp")]
#[test]
fn native_capture_encoder_rejects_png_overflow() {
    let rig = Rig::new("native-encoding-bounds");
    let mut response = ureq::get(&format!("http://{}/api/screenshot", rig.addr))
        .call()
        .unwrap();
    let bytes = response
        .body_mut()
        .with_config()
        .limit(2 * 1024 * 1024)
        .read_to_vec()
        .unwrap();
    let frame = rust_dos::capture::png::decode(&bytes).unwrap();
    assert!(
        super::super::mcp::encode_capture(&frame, Instant::now() + Duration::from_secs(2), 8)
            .is_err(),
        "actual PNG cannot grow past its sink budget"
    );
}

#[cfg(feature = "mcp")]
#[test]
fn native_capture_encoder_rejects_expired_start() {
    let rig = Rig::new("native-encoding-expiry");
    let mut response = ureq::get(&format!("http://{}/api/screenshot", rig.addr))
        .call()
        .unwrap();
    let bytes = response
        .body_mut()
        .with_config()
        .limit(2 * 1024 * 1024)
        .read_to_vec()
        .unwrap();
    let frame = rust_dos::capture::png::decode(&bytes).unwrap();
    assert!(
        super::super::mcp::encode_capture(
            &frame,
            Instant::now() - Duration::from_millis(1),
            13 * 1024 * 1024
        )
        .is_err(),
        "expired work never starts encoding"
    );
}

#[test]
fn every_json_body_has_its_request_number_first() {
    let rig = Rig::new("req-first");
    rig.post("/api/control/pause", json!({}));
    let op = rig.post("/api/input/type", json!({"text": "x"})).json()["op"].clone();
    let large = "/api/memory?addr=0&len=1048576&format=base64";
    let paths = [
        "/api/status",
        "/api/version",
        "/api/journal?after=0",
        &format!("/api/ops/{}", op),
        "/api/ops/999999",
        "/api/log?limit=5",
        "/api/trace?format=json&last_n=5",
        "/api/registers",
        "/api/registers/fpu",
        "/api/control/wait",
        "/api/journal?type=nonsense",
        large,
    ];
    for path in paths {
        let r = rig.get(path);
        let req = r.header("x-debug-req").unwrap();
        assert!(r.body.starts_with(&format!("{{\"req\":{},", req)), "{} {}: {}", path, r.status, &r.body[..r.body.len().min(200)]);
        assert_eq!(r.header("content-type"), Some("application/json"), "{}", path);
    }
    assert_eq!(rig.get(large).json()["len"], 1 << 20);
    // A refusal from the emulator thread, and one the middleware makes.
    let refused = http(rig.addr, "PUT", "/api/memory", &[("If-State-Seq", "1")], Some(&json!({"addr": "0", "hex": "00"})));
    assert_eq!(refused.status, 409);
    let bad = http(rig.addr, "GET", "/api/status", &[("If-State-Seq", "x")], None);
    assert_eq!(bad.status, 400);
    for r in [refused, bad] {
        assert!(r.body.starts_with(&format!("{{\"req\":{},", r.header("x-debug-req").unwrap())), "{}", r.body);
    }
}

#[test]
fn an_operation_ends_when_another_client_pauses_the_machine() {
    let rig = Rig::new("ops");
    let resumed = rig.post("/api/control/resume", json!({}));
    let op = resumed.json()["op"].as_u64().expect("an operation");
    assert_eq!(resumed.json()["seq"].as_u64(), rig.get(&format!("/api/ops/{}", op)).json()["start_seq"].as_u64());
    let addr = rig.addr;
    let waiter = std::thread::spawn(move || http(addr, "GET", &format!("/api/ops/{}?wait=10000", op), &[], None).json());
    std::thread::sleep(Duration::from_millis(200));
    let paused = rig.post("/api/control/pause", json!({}));
    let pause_req = paused.json()["req"].clone();
    let ended = waiter.join().unwrap();
    assert_eq!(ended["state"], "stopped", "{}", ended);
    assert_eq!(ended["stop"]["by"], pause_req);
    assert_eq!(ended["stop"]["op"].as_u64(), Some(op));
    // The pause's reply is the stop.
    assert_eq!(paused.json()["stop"]["seq"], ended["stop"]["seq"]);

    // A long poll with nothing to tell times out.
    let start = Instant::now();
    let polled = rig.get(&format!("/api/journal?after={}&wait=300", rig.last())).json();
    assert!(start.elapsed() >= Duration::from_millis(250));
    assert_eq!((polled["timed_out"].as_bool(), polled["entries"].as_array().map(Vec::len)), (Some(true), Some(0)));
    assert_eq!(rig.get("/api/ops/999999").status, 404);
}

#[test]
fn a_stop_before_the_wait_is_answered_at_once() {
    let rig = Rig::new("early");
    let cs = rig.at_probe_entry();
    rig.post("/api/breakpoints", json!({"addr": format!("{}:0103", cs), "once": true}));
    let resumed = rig.post("/api/control/resume", json!({})).json();
    rig.until_paused();
    let start = Instant::now();
    let stop = rig.get(&format!("/api/control/wait?after={}&timeout_ms=5000", resumed["seq"])).json();
    assert!(start.elapsed() < Duration::from_secs(2));
    assert_eq!((stop["reason"].as_str(), stop["once"].as_bool()), (Some("breakpoint"), Some(true)), "{}", stop);
    assert_eq!(stop["stop"]["op"], resumed["op"]);
    // So is a wait on the operation.
    let by_op = rig.get(&format!("/api/control/wait?op={}", resumed["op"])).json();
    assert_eq!(by_op["stop"]["seq"], stop["stop"]["seq"]);
}

#[test]
fn a_superseded_step_answers_with_the_stop_that_ended_the_run() {
    let rig = Rig::new("supersede");
    let cs = rig.at_entry("LOOP.COM");
    let addr = rig.addr;
    // A step longer than the test, which a resume takes over.
    let stepper = std::thread::spawn(move || {
        http(addr, "POST", "/api/control/step", &[], Some(&json!({"count": 1_000_000_000u64, "timeout_ms": 10000})))
    });
    // Wait for the step's operation to be under way.
    let deadline = Instant::now() + Duration::from_secs(10);
    while rig.get("/api/status").json()["op"].is_null() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    let resumed = rig.post("/api/control/resume", json!({"until": format!("{}:0101", cs)})).json();
    let step = stepper.join().unwrap();
    assert_eq!(step.status, 200, "{}", step.body);
    let step = step.json();
    assert_eq!(step["superseded_by"], resumed["op"], "{}", step);
    assert!(step["target"].is_string(), "the resume's stop: {}", step);
    let op = rig.get(&format!("/api/ops/{}", step["op"])).json();
    assert_eq!((op["state"].as_str(), op["superseded_by"].clone()), (Some("superseded"), resumed["op"].clone()));
}

#[test]
fn a_guarded_write_is_refused_once_the_state_moved() {
    let rig = Rig::new("guard");
    rig.post("/api/control/pause", json!({}));
    let read = rig.get("/api/memory?addr=0200:0000&len=2");
    let seq = read.header("x-debug-state-seq").unwrap().to_string();
    let write = |seq: &str, hex: &str| {
        http(rig.addr, "PUT", "/api/memory", &[("If-State-Seq", seq)], Some(&json!({"addr": "0200:0000", "hex": hex})))
    };
    let done = write(&seq, "11 22");
    assert_eq!(done.status, 200, "{}", done.body);
    let chained = done.json()["state_seq"].as_u64().unwrap().to_string();
    assert_eq!(write(&chained, "33 44").status, 200, "the reply's state_seq chains");
    let read = rig.get("/api/memory?addr=0200:0000&len=2").header("x-debug-state-seq").unwrap().to_string();
    rig.post("/api/control/resume", json!({}));
    rig.post("/api/control/pause", json!({}));
    let refused = write(&read, "55 66");
    assert_eq!(refused.status, 409);
    let body = refused.json();
    assert_eq!(body["precondition"], "state", "{}", body);
    for key in ["error", "paused", "stop_seq", "state_seq", "pending_ops", "input_queued", "settings_window"] {
        assert!(body.get(key).is_some(), "{} in {}", key, body);
    }
    assert!(rig.get("/api/memory?addr=0200:0000&len=2&format=base64").json()["base64"] == "M0Q=", "unchanged");
}

#[test]
fn every_memory_format_has_the_state_seq_and_json_has_the_hex_put_takes() {
    let rig = Rig::new("mem-formats");
    rig.post("/api/control/pause", json!({}));
    assert_eq!(http(rig.addr, "PUT", "/api/memory", &[], Some(&json!({"addr": "0200:0000", "hex": "41 1F"}))).status, 200);
    let read = rig.get("/api/memory?addr=0200:0000&len=2&format=json");
    let body = read.json();
    assert_eq!((body["addr"].as_str(), body["len"].as_u64(), body["hex"].as_str()), (Some("02000"), Some(2), Some("41 1F")), "{}", body);
    let seq = read.header("x-debug-state-seq").unwrap().to_string();
    assert_eq!(body["state_seq"].as_u64().map(|s| s.to_string()), Some(seq.clone()));
    // What it read goes back as it is.
    let again = http(rig.addr, "PUT", "/api/memory", &[("If-State-Seq", &seq)], Some(&json!({"addr": "0200:0000", "hex": body["hex"]})));
    assert_eq!(again.status, 200, "{}", again.body);
    for format in ["hex", "base64", "raw", "json"] {
        let r = rig.get(&format!("/api/memory?addr=0200:0000&len=2&format={}", format));
        assert!(r.header("x-debug-state-seq").is_some(), "{}", format);
        if format == "base64" {
            assert_eq!(r.json()["state_seq"].as_u64().map(|s| s.to_string()).as_deref(), r.header("x-debug-state-seq"));
        }
    }
    let unknown = rig.get("/api/memory?addr=0&len=2&format=yaml");
    assert_eq!(unknown.status, 400);
    assert!(unknown.body.contains("json"), "{}", unknown.body);
}

#[test]
fn an_event_stream_goes_on_where_it_left_off() {
    let rig = Rig::new("replay");
    rig.post("/api/control/pause", json!({}));
    let last = rig.last();
    let mut socket = ws(rig.addr, "/ws/events?after=0&volatile=false");
    let mut seen = Vec::new();
    while seen.last() != Some(&last) {
        seen.push(next(&mut socket).unwrap()["seq"].as_u64().unwrap());
    }
    assert_eq!(seen, (1..=last).collect::<Vec<_>>());
    socket.close(None).unwrap();
    // While nobody listens.
    for i in 0..3 {
        rig.post("/api/breakpoints", json!({"addr": format!("0100:000{}", i)}));
    }
    let now = rig.last();
    let mut socket = ws(rig.addr, &format!("/ws/events?after={}&volatile=false", last));
    let missed: Vec<Value> = (last..now).map(|_| next(&mut socket).unwrap()).collect();
    assert_eq!(missed.iter().map(|e| e["seq"].as_u64().unwrap()).collect::<Vec<_>>(), (last + 1..=now).collect::<Vec<_>>());
    assert!(missed.iter().all(|e| e["type"] == "breakpoint_set"));
    // Then live, with no repeat.
    rig.post("/api/breakpoints", json!({"addr": "0100:0009"}));
    assert_eq!(next(&mut socket).unwrap()["seq"].as_u64(), Some(now + 1));
}

#[test]
fn an_event_stream_from_an_evicted_entry_says_what_is_gone() {
    let rig = Rig::with_capacity("gone", 8);
    rig.post("/api/control/pause", json!({}));
    for i in 0..10 {
        rig.post("/api/breakpoints", json!({"addr": format!("0100:00{:02}", i)}));
    }
    let journal = rig.get("/api/journal?after=0");
    assert_eq!(journal.status, 410);
    let first = journal.json()["first"].as_u64().unwrap();
    assert_eq!(rig.get("/api/journal?after=0&allow_gap=true").json()["entries"][0]["seq"].as_u64(), Some(first));
    // The first stop after entry 1 may be among those gone.
    let waited = rig.get("/api/control/wait?after=1&timeout_ms=100");
    assert_eq!((waited.status, waited.json()["first"].as_u64()), (410, Some(first)), "{}", waited.body);
    let mut socket = ws(rig.addr, "/ws/events?after=1&volatile=false");
    assert_eq!(next(&mut socket).unwrap(), json!({"type": "gone", "first": first}));
    assert_eq!(next(&mut socket).unwrap()["seq"].as_u64(), Some(first));
}

#[test]
fn a_quit_ends_long_polls_and_streams_with_shutdown() {
    let rig = Rig::new("quit");
    let last = rig.last();
    let addr = rig.addr;
    let poll = std::thread::spawn(move || http(addr, "GET", &format!("/api/journal?after={}&type=paused&wait=20000", last), &[], None));
    let mut socket = ws(rig.addr, &format!("/ws/events?after={}&volatile=false", last));
    std::thread::sleep(Duration::from_millis(200));
    let quit = rig.post("/api/control/quit", json!({}));
    assert_eq!(quit.json()["shutting_down"], true);
    let polled = poll.join().unwrap().json();
    assert_eq!(polled["entries"][0]["type"], "shutdown", "{}", polled);
    assert_eq!(polled["entries"][0]["by"], quit.json()["req"]);
    let mut types = Vec::new();
    while let Some(e) = next(&mut socket) {
        types.push(e["type"].as_str().unwrap().to_string());
    }
    assert_eq!(types.last().map(String::as_str), Some("shutdown"), "then the socket closes");
    assert_eq!(rig.get("/api/status").status, 503);
}

#[test]
fn replies_keep_the_keys_they_had() {
    let rig = Rig::new("legacy");
    let cs = rig.at_probe_entry();
    let stepped = rig.post("/api/control/step", json!({"count": 1})).json();
    for key in ["paused", "reason", "icount", "registers"] {
        assert!(stepped.get(key).is_some(), "{} in {}", key, stepped);
    }
    assert_eq!(stepped["reason"], "step");
    assert!(stepped["op"].is_u64() && stepped["seq"].is_u64() && stepped["stop"]["type"] == "paused");
    let waited = rig.get("/api/control/wait").json();
    let paused = rig.post("/api/control/pause", json!({})).json();
    for reply in [&waited, &paused] {
        for key in keys(&stepped).iter().filter(|k| !["op", "seq", "req"].contains(&k.as_str())) {
            assert!(reply.get(key).is_some(), "{} in {}", key, reply);
        }
    }
    let resumed = rig.post("/api/control/resume", json!({})).json();
    assert_eq!((resumed["ok"].as_bool(), resumed["paused"].as_bool()), (Some(true), Some(false)));
    // Breakpoints: once goes with an address only.
    assert_eq!(rig.post("/api/breakpoints", json!({"exception": "0D", "once": true})).status, 400);
    let listed = rig.post("/api/breakpoints", json!({"addr": format!("{}:0103", cs), "once": true})).json();
    assert_eq!(listed["once"].as_array().map(Vec::len), Some(1), "{}", listed);
    // Input that waits is answered once it is delivered.
    let typed = rig.post("/api/input/type", json!({"text": "x"}));
    assert_eq!(typed.status, 200, "{}", typed.body);
    assert_eq!((typed.json()["ok"].as_bool(), typed.json()["done"].as_bool()), (Some(true), Some(true)), "{}", typed.body);
    let op = rig.get(&format!("/api/ops/{}", typed.json()["op"])).json();
    assert_eq!((op["kind"].as_str(), op["state"].as_str()), (Some("input"), Some("done")));
}

#[test]
fn the_other_replies_keep_their_keys_too() {
    let rig = Rig::new("legacy-more");
    let has = |r: &Resp, keys: &[&str]| {
        assert!(r.status < 300, "{}", r.body);
        let v = r.json();
        for key in keys {
            assert!(v.get(*key).is_some(), "{} in {}", key, v);
        }
        v
    };
    let ran = rig.post("/api/control/run", json!({"command": "PROBE.COM"}));
    assert_eq!(has(&ran, &["ok", "program"])["ok"], true);
    rig.until_idle();
    rig.post("/api/control/pause", json!({}));
    has(&rig.post("/api/input/type?wait=false", json!({"text": "x"})), &["ok", "queued"]);
    // Input waited for and cleared: answered as before, 200 and cancelled.
    let addr = rig.addr;
    let waiting = std::thread::spawn(move || http(addr, "POST", "/api/input/wait", &[], Some(&json!({"ms": 60000}))));
    let deadline = Instant::now() + Duration::from_secs(10);
    while rig.get("/api/journal?after=0&type=input_queued").json()["entries"].as_array().unwrap().len() < 2 {
        assert!(Instant::now() < deadline, "the input is queued");
        std::thread::sleep(Duration::from_millis(5));
    }
    has(&http(rig.addr, "DELETE", "/api/input", &[], None), &["ok"]);
    let cancelled = waiting.join().unwrap();
    assert_eq!((cancelled.status, cancelled.json()["ok"].as_bool(), cancelled.json()["cancelled"].as_bool()), (200, Some(false), Some(true)), "{}", cancelled.body);
    has(&http(rig.addr, "PUT", "/api/memory", &[], Some(&json!({"addr": "0200:0000", "hex": "01"}))), &["ok", "addr", "written", "old", "new"]);
    has(&rig.get("/api/screen/text?format=json"), &["lines", "rows", "attrs"]);
    has(&rig.post("/api/drive/sync", json!({})), &["messages"]);
    has(&rig.post("/api/control/reboot_shell", json!({})), &["ok"]);
}

/// The status of a request with raw header lines.
fn raw_status(addr: SocketAddr, request: &[u8]) -> u16 {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    stream.write_all(request).unwrap();
    // The status line and headers are enough; a refused upgrade keeps the
    // connection open.
    let mut raw = Vec::new();
    let mut buf = [0u8; 4096];
    while !raw.windows(4).any(|w| w == b"\r\n\r\n") {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => raw.extend_from_slice(&buf[..n]),
        }
    }
    let raw = String::from_utf8_lossy(&raw);
    raw.split(' ').nth(1).and_then(|s| s.parse().ok()).unwrap_or(0)
}

#[test]
fn malformed_requests_get_a_clear_error_and_change_nothing() {
    let rig = Rig::new("malformed");
    rig.post("/api/control/pause", json!({}));
    let before = rig.last();
    let status = |path: &str, headers: &[(&str, &str)]| http(rig.addr, "GET", path, headers, None).status;
    // Request IDs.
    assert_eq!(status("/api/status", &[("X-Request-Id", "")]), 400);
    assert_eq!(status("/api/status", &[("X-Request-Id", &"y".repeat(129))]), 400);
    assert_eq!(status("/api/status", &[("X-Request-Id", &"y".repeat(128))]), 200);
    assert_eq!(raw_status(rig.addr, b"GET /api/status HTTP/1.1\r\nHost: x\r\nConnection: close\r\nX-Request-Id: caf\xc3\xa9\r\n\r\n"), 400);
    // If-State-Seq values, and the header twice.
    for bad in ["abc", "-1", "1.5", "99999999999999999999", "0x10"] {
        assert_eq!(status("/api/registers", &[("If-State-Seq", bad)]), 400, "{}", bad);
    }
    let twice = b"GET /api/registers HTTP/1.1\r\nHost: x\r\nConnection: close\r\nIf-State-Seq: 1\r\nIf-State-Seq: 2\r\n\r\n";
    assert_eq!(raw_status(rig.addr, twice), 400, "two preconditions");
    let twice = b"GET /api/status HTTP/1.1\r\nHost: x\r\nConnection: close\r\nX-Request-Id: a\r\nX-Request-Id: b\r\n\r\n";
    assert_eq!(raw_status(rig.addr, twice), 400, "two request IDs");
    // If-State-Seq where the server answers itself.
    let seq = before.to_string();
    for path in ["/api/version", "/api/journal", "/api/ops/1", "/api/log", "/api/control/wait?after=1", "/api/control/wait?op=1"] {
        assert_eq!(status(path, &[("If-State-Seq", &seq)]), 400, "{}", path);
    }
    let upgrade = format!(
        "GET /ws/events HTTP/1.1\r\nHost: x\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nIf-State-Seq: {}\r\n\r\n",
        seq
    );
    assert_eq!(raw_status(rig.addr, upgrade.as_bytes()), 400, "on a stream");
    // Cursors past the journal's end: another instance's, or garbage.
    for after in [u64::MAX.to_string(), (before + 1000).to_string()] {
        let r = rig.get(&format!("/api/journal?after={}", after));
        assert_eq!(r.status, 400, "{}: {}", after, r.body);
        assert_eq!(r.json()["last"].as_u64(), Some(before), "{}", r.body);
        assert_eq!(rig.get(&format!("/api/journal?after={}&wait=100", after)).status, 400);
        assert_eq!(rig.get(&format!("/api/control/wait?after={}&timeout_ms=100", after)).status, 400);
        assert!(tungstenite::connect(format!("ws://{}/ws/events?after={}", rig.addr, after)).is_err());
    }
    // Negative and non-numeric parameters.
    for path in ["/api/journal?after=-1", "/api/journal?limit=abc", "/api/journal?wait=-5", "/api/ops/abc", "/api/ops/-1", "/api/control/wait?op=x", "/api/log?after_n=-2"] {
        assert_eq!(rig.get(path).status, 400, "{}", path);
    }
    // Operations that never were.
    for op in ["0", "999999", &u64::MAX.to_string()] {
        assert_eq!(rig.get(&format!("/api/ops/{}", op)).status, 404, "{}", op);
        assert_eq!(rig.get(&format!("/api/ops/{}?wait=100", op)).status, 404, "{}", op);
        assert_eq!(rig.get(&format!("/api/control/wait?op={}", op)).status, 404, "{}", op);
    }
    // Bodies: not JSON, the wrong shape, too large.
    let put = |body: &str| {
        let request = format!("PUT /api/memory HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}", body.len(), body);
        raw_status(rig.addr, request.as_bytes())
    };
    assert_eq!(put("{not json"), 400);
    assert_eq!(put(r#"{"addr": 5, "hex": "00"}"#), 400);
    // A body past the limit (2 MB) is refused: a 413, or on Windows often
    // a reset, since the server closes with the body's rest unread.
    let mut stream = TcpStream::connect(rig.addr).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut writer = stream.try_clone().unwrap();
    let sender = std::thread::spawn(move || {
        let head = "PUT /api/memory HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: 4194304\r\n\r\n";
        let _ = writer.write_all(head.as_bytes());
        let chunk = vec![b' '; 64 << 10];
        for _ in 0..64 {
            if writer.write_all(&chunk).is_err() {
                break;
            }
        }
    });
    let mut first = [0u8; 12];
    match stream.read_exact(&mut first) {
        Ok(()) => assert_eq!(&first[9..12], b"413", "{}", String::from_utf8_lossy(&first)),
        Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::ConnectionReset, "{}", e),
    }
    drop(stream);
    let _ = sender.join();
    assert_eq!(rig.get("/api/status").status, 200, "the server goes on");
    // None of it reached the machine.
    assert_eq!(rig.last(), before, "{:?}", rig.get(&format!("/api/journal?after={}", before)).body);
}

/// `http`, or None if the connection failed.
fn try_http(addr: SocketAddr, method: &str, path: &str, body: Option<&Value>) -> Option<Resp> {
    let body = body.cloned();
    let path = path.to_string();
    let method = method.to_string();
    std::panic::catch_unwind(move || http(addr, &method, &path, &[], body.as_ref())).ok()
}

#[test]
fn a_quit_answers_everything_in_flight_and_after() {
    let mut rig = Rig::with("quit-busy", Options { linger: Duration::from_secs(4), ..Options::default() });
    let cs = rig.at_entry("LOOP.COM");
    let last = rig.last();
    let addr = rig.addr;
    // Waits of every kind, open as the quit comes.
    let poll = std::thread::spawn(move || http(addr, "GET", &format!("/api/journal?after={}&type=paused&wait=60000", last), &[], None));
    let op = rig.post("/api/control/resume", json!({})).json()["op"].as_u64().unwrap();
    let op_wait = std::thread::spawn(move || http(addr, "GET", &format!("/api/ops/{}?wait=60000", op), &[], None));
    let stop_wait = std::thread::spawn(move || http(addr, "GET", "/api/control/wait?timeout_ms=60000", &[], None));
    let step = std::thread::spawn(move || http(addr, "POST", "/api/control/step", &[], Some(&json!({"count": 1u64 << 40, "timeout_ms": 60000}))));
    let log = std::thread::spawn(move || http(addr, "GET", "/api/log?after_n=999999999&wait=60000", &[], None));
    let mut socket = ws(rig.addr, &format!("/ws/events?after={}&volatile=false", last));
    // Clients that go on sending as it quits.
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let clients: Vec<_> = (0..6)
        .map(|i| {
            let stop = stop.clone();
            let cs = cs.clone();
            std::thread::spawn(move || {
                let mut seen = Vec::new();
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let start = Instant::now();
                    let r = match i % 3 {
                        0 => try_http(addr, "GET", "/api/registers", None),
                        1 => try_http(addr, "POST", "/api/breakpoints", Some(&json!({"addr": format!("{}:0300", cs)}))),
                        _ => try_http(addr, "POST", "/api/input/type?wait=false", Some(&json!({"text": "a"}))),
                    };
                    seen.push((r.map_or(0, |r| r.status), start.elapsed()));
                }
                seen
            })
        })
        .collect();
    std::thread::sleep(Duration::from_millis(300));
    let start = Instant::now();
    let quit = rig.quit();
    assert_eq!(quit.status, 200);
    let shut_down_in = start.elapsed();
    std::thread::sleep(Duration::from_millis(500));
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(shut_down_in < Duration::from_secs(8), "the emulator thread ends: {:?}", shut_down_in);
    for client in clients {
        for (status, took) in client.join().unwrap() {
            assert!(matches!(status, 200 | 409 | 429 | 503), "status {}", status);
            assert!(took < Duration::from_secs(3), "a request took {:?} (status {})", took, status);
        }
    }
    let polled = poll.join().unwrap().json();
    assert_eq!(polled["entries"].as_array().unwrap().last().unwrap()["type"], "shutdown", "{}", polled);
    let ended = op_wait.join().unwrap().json();
    // The step took the resume over; the step's own end is the shutdown.
    assert_eq!(ended["state"], "superseded", "{}", ended);
    for r in [stop_wait.join().unwrap(), step.join().unwrap()] {
        assert_eq!(r.status, 503, "{}", r.body);
        assert_eq!(r.json()["shutdown"]["type"], "shutdown", "{}", r.body);
    }
    assert_eq!(log.join().unwrap().status, 200);
    let mut types = Vec::new();
    while let Some(e) = next(&mut socket) {
        types.push(e["type"].as_str().unwrap_or("").to_string());
    }
    assert_eq!(types.last().map(String::as_str), Some("shutdown"));
    // After it, at once.
    let start = Instant::now();
    assert_eq!(rig.get("/api/registers").status, 503);
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn run_answers_once_its_program_started_without_another_entry() {
    let rig = Rig::new("run-started");
    // LOOP.COM runs forever and makes no entry after its start.
    let start = Instant::now();
    let started = rig.post("/api/control/run", json!({"command": "LOOP.COM", "timeout_ms": 10000}));
    assert_eq!(started.status, 200, "{}", started.body);
    assert!(start.elapsed() < Duration::from_secs(3), "answered as the program started: {:?}", start.elapsed());
    assert_eq!(started.json()["program"]["name"], "LOOP.COM");
    let op = rig.get(&format!("/api/ops/{}", started.json()["op"])).json();
    assert_eq!(op["state"], "done", "{}", op);
    let end = op["end"].clone();
    assert_eq!((end["type"].as_str(), end["op"].clone()), (Some("program_started"), started.json()["op"].clone()), "{}", op);
}

#[test]
fn a_run_with_queue_is_taken_while_the_prompt_is_busy() {
    let rig = Rig::new("run-queued");
    let started = rig.post("/api/control/run", json!({"command": "LOOP.COM", "timeout_ms": 10000}));
    assert_eq!(started.status, 200, "{}", started.body);
    // LOOP.COM keeps the prompt busy.
    let refused = rig.post("/api/control/run", json!({"command": "PROBE.COM"}));
    assert_eq!(refused.status, 409, "{}", refused.body);
    let queued = rig.post("/api/control/run", json!({"command": "PROBE.COM", "queue": true, "wait": false}));
    assert_eq!(queued.status, 202, "{}", queued.body);
    let op = rig.get(&format!("/api/ops/{}", queued.json()["op"])).json();
    assert_eq!(op["state"], "pending", "waits for LOOP.COM to end: {}", op);
}

// ----- consistency under load ------------------------------------------------

/// Every operation the journal names, as `/api/ops` tells it (those evicted
/// left out).
fn ops_of(rig: &Rig, entries: &[Value]) -> std::collections::BTreeMap<u64, Value> {
    let mut ids = std::collections::BTreeSet::new();
    for e in entries {
        match e["type"].as_str() {
            Some("resumed") => ids.extend(e["op"].as_u64()),
            Some("input_queued") => ids.extend(e["cause"].as_u64()),
            _ => {}
        }
    }
    let mut ops = std::collections::BTreeMap::new();
    for id in ids {
        let r = rig.get(&format!("/api/ops/{}", id));
        match r.status {
            200 => {
                ops.insert(id, r.json());
            }
            410 => {}
            _ => panic!("operation {}: {} {}", id, r.status, r.body),
        }
    }
    ops
}

/// Check that the journal holds together and agrees with the operations:
/// numbered without gaps, a resume between two stops, every stop the end
/// of the operation it names and every ended operation's end entry naming
/// it. Returns the state and stop the journal says the machine is in.
fn check_journal(entries: &[Value], ops: &std::collections::BTreeMap<u64, Value>) -> (u64, Option<u64>) {
    for (i, e) in entries.iter().enumerate() {
        assert_eq!(e["seq"].as_u64(), Some(i as u64 + 1), "gapless and in order: {}", e);
    }
    let entry = |seq: &Value| -> &Value { &entries[seq.as_u64().expect("an entry") as usize - 1] };
    let lists = |e: &Value, op: u64| e["cancelled"].as_array().is_some_and(|c| c.contains(&json!(op)));
    let (mut stop, mut state_seq) = (None, 0);
    for e in entries {
        let seq = e["seq"].as_u64().unwrap();
        let kind = e["type"].as_str().unwrap();
        let op = |n: &Value| n.as_u64().and_then(|n| ops.get(&n));
        match kind {
            "paused" => {
                assert!(stop.is_none() || e["reason"] == "load", "two stops with no resume between: {}", e);
                stop = Some(seq);
                if let Some(o) = op(&e["op"]) {
                    assert_eq!((o["state"].as_str(), o["end_seq"].as_u64()), (Some("stopped"), Some(seq)), "{} ended by {}", o, e);
                }
                match e["reason"].as_str() {
                    Some("request") => assert_eq!(e["by"], e["cause"], "{}", e),
                    Some("startup" | "load") => {}
                    _ => assert_eq!(e["cause"], e["op"], "{}", e),
                }
            }
            "resumed" => {
                assert_eq!(e["from_paused"].as_bool(), Some(stop.is_some()), "{}", e);
                stop = None;
                assert_eq!(e["cause"], e["op"], "{}", e);
                if let Some(o) = op(&e["op"]) {
                    assert_eq!(o["start_seq"].as_u64(), Some(seq), "{}", o);
                }
                if let Some(old) = op(&e["supersedes"]) {
                    assert_eq!((old["state"].as_str(), &old["superseded_by"]), (Some("superseded"), &e["op"]), "{} by {}", old, e);
                }
            }
            "input_queued" => {
                if let Some(o) = op(&e["cause"]) {
                    assert_eq!((o["kind"].as_str(), o["start_seq"].as_u64()), (Some("input"), Some(seq)), "{}", o);
                }
            }
            "input_delivered" => {
                if let Some(o) = op(&e["cause"]) {
                    assert_eq!((o["state"].as_str(), o["end_seq"].as_u64()), (Some("done"), Some(seq)), "{}", o);
                }
            }
            "input_cleared" | "debugger_reset" | "shutdown" => {
                for c in e["cancelled"].as_array().unwrap() {
                    if let Some(o) = op(c) {
                        assert_eq!((o["state"].as_str(), o["end_seq"].as_u64()), (Some("cancelled"), Some(seq)), "{}", o);
                    }
                }
            }
            _ => {}
        }
        if super::journal::Kind::parse(kind).unwrap().changes_state() {
            state_seq = seq;
        }
    }
    let mut pending_control = Vec::new();
    for (&n, o) in ops {
        let end = &o["end_seq"];
        match o["state"].as_str().unwrap() {
            "pending" => {
                assert!(end.is_null(), "{}", o);
                if o["kind"] != "input" {
                    pending_control.push(n);
                }
            }
            "stopped" => assert_eq!((entry(end)["type"].as_str(), entry(end)["op"].as_u64()), (Some("paused"), Some(n)), "{}", o),
            "done" => {
                let e = entry(end);
                let by_input = e["type"] == "input_delivered" && e["cause"].as_u64() == Some(n);
                let by_start = e["type"] == "program_started" && e["op"].as_u64() == Some(n);
                assert!(by_input || by_start, "{} ended by {}", o, e);
            }
            "superseded" => {
                let by = o["superseded_by"].as_u64().unwrap();
                let took = entries.iter().find(|e| e["type"] == "resumed" && e["op"].as_u64() == Some(by)).expect("the resume that took over");
                assert_eq!(took["supersedes"].as_u64(), Some(n), "{} by {}", o, took);
            }
            "cancelled" => assert!(lists(entry(end), n), "{} ended by {}", o, entry(end)),
            "failed" => assert_eq!((entry(end)["type"].as_str(), entry(end)["cause"].as_u64()), (Some("op_failed"), Some(n)), "{}", o),
            other => panic!("state {}", other),
        }
    }
    assert!(pending_control.len() <= 1, "control operations pending at once: {:?}", pending_control);
    (state_seq, stop)
}

/// A small generator of numbers for the clients' choices.
struct Rand(u64);

impl Rand {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n
    }
}

#[test]
fn many_clients_at_once_leave_a_consistent_journal() {
    let mut rig = Rig::with("stress", Options { capacity: 1 << 18, ..Options::default() });
    let cs = rig.at_entry("COUNT.COM");
    let addr = rig.addr;
    // A stream from the start, read throughout.
    let mut socket = ws(addr, "/ws/events?after=0&volatile=false");
    let streamed = std::thread::spawn(move || {
        let mut seqs = Vec::new();
        while let Some(e) = next(&mut socket) {
            seqs.push(e["seq"].as_u64().expect("an entry"));
        }
        seqs
    });
    let clients: Vec<_> = (0..16u64)
        .map(|i| {
            let cs = cs.clone();
            std::thread::spawn(move || {
                let mut rand = Rand(0x9E37_79B9_7F4A_7C15 ^ ((i + 1) * 0x1234_5678_9ABC));
                let mut seen: Vec<(u64, u16, u64)> = Vec::new();
                let mut last_op = None;
                for _ in 0..400 {
                    let action = match rand.below(13) {
                        7 if last_op.is_none() => 0,
                        a => a,
                    };
                    let r = match action {
                        0 => http(addr, "POST", "/api/control/resume", &[], Some(&json!({}))),
                        1 => http(addr, "POST", "/api/control/pause", &[], Some(&json!({}))),
                        2 => http(addr, "POST", "/api/control/step", &[], Some(&json!({"count": rand.below(20) + 1, "timeout_ms": 2000}))),
                        3 => http(addr, "POST", "/api/control/step_over", &[], Some(&json!({"timeout_ms": 2000}))),
                        4 => {
                            let read = http(addr, "GET", &format!("/api/memory?addr={}:0300&len=2", cs), &[], None);
                            let seq = read.header("x-debug-state-seq").unwrap().to_string();
                            let body = json!({"addr": format!("{}:0300", cs), "hex": format!("{:04X}", rand.below(0x10000))});
                            http(addr, "PUT", "/api/memory", &[("If-State-Seq", &seq)], Some(&body))
                        }
                        5 => {
                            let wait = if rand.below(2) == 0 { "true" } else { "false" };
                            http(addr, "POST", &format!("/api/input/type?wait={}", wait), &[], Some(&json!({"text": "ab"})))
                        }
                        // Now and then, so that some input is delivered.
                        6 if rand.below(8) == 0 => http(addr, "DELETE", "/api/input", &[], None),
                        6 => http(addr, "GET", "/api/status", &[], None),
                        7 => http(addr, "GET", &format!("/api/ops/{}?wait=200", last_op.unwrap_or(0)), &[], None),
                        8 => {
                            let last = http(addr, "GET", "/api/status", &[], None).json()["seq"].as_u64().unwrap();
                            http(addr, "GET", &format!("/api/control/wait?after={}&timeout_ms=200", last.saturating_sub(rand.below(5))), &[], None)
                        }
                        9 => http(addr, "GET", "/api/journal?after=0&limit=1&wait=100", &[], None),
                        10 => http(addr, "POST", "/api/breakpoints", &[], Some(&json!({"addr": format!("{}:0000", cs), "once": rand.below(2) == 0}))),
                        11 => http(addr, "POST", "/api/control/resume", &[], Some(&json!({"until": format!("{}:0104", cs)}))),
                        _ => {
                            let seq = http(addr, "GET", "/api/registers", &[], None).header("x-debug-state-seq").unwrap().to_string();
                            http(addr, "PUT", "/api/registers", &[("If-State-Seq", &seq)], Some(&json!({"cx": format!("{:04X}", rand.below(0x10000))})))
                        }
                    };
                    let allowed: &[u16] = match action {
                        2 | 3 => &[200, 504],
                        4 | 12 => &[200, 409],
                        5 => &[200, 429],
                        7 => &[200, 410],
                        8 => &[200, 410, 504],
                        _ => &[200],
                    };
                    assert!(allowed.contains(&r.status), "action {}: {} {}", action, r.status, r.body);
                    if let Some(op) = r.json().get("op").and_then(Value::as_u64) {
                        last_op = Some(op);
                    }
                    seen.push((action, r.status, r.header("x-debug-req").unwrap().parse().unwrap()));
                }
                seen
            })
        })
        .collect();
    let deadline = Instant::now() + Duration::from_secs(240);
    let mut reqs = Vec::new();
    for client in clients {
        reqs.extend(client.join().expect("a client failed").into_iter().map(|(_, _, req)| req));
    }
    assert!(Instant::now() < deadline, "no client hung");
    let count = reqs.len();
    reqs.sort_unstable();
    reqs.dedup();
    assert_eq!(reqs.len(), count, "every request has its own number");

    // Settled: paused, the input delivered or cleared.
    rig.post("/api/control/pause", json!({}));
    http(addr, "DELETE", "/api/input", &[], None);
    let entries = rig.journal();
    let ops = ops_of(&rig, &entries);
    let (state_seq, stop) = check_journal(&entries, &ops);
    let status = rig.get("/api/status").json();
    assert_eq!((status["state_seq"].as_u64(), status["stop"]["seq"].as_u64()), (Some(state_seq), stop), "the journal's state is the machine's");
    assert!(entries.iter().filter(|e| e["type"] == "paused").count() > 20, "the clients stopped the machine often");
    let mut states = std::collections::BTreeMap::new();
    for o in ops.values() {
        *states.entry(format!("{}/{}", o["kind"].as_str().unwrap(), o["state"].as_str().unwrap())).or_insert(0) += 1;
    }
    eprintln!("{:?}", states);
    // Every way an operation ends came up.
    for state in ["stopped", "done", "superseded", "cancelled"] {
        assert!(ops.values().any(|o| o["state"] == state), "no operation {}", state);
    }
    eprintln!("{} entries, {} operations kept", entries.len(), ops.len());

    // The stream had every entry once, in order, to the shutdown.
    rig.quit();
    let seqs = streamed.join().unwrap();
    assert_eq!(seqs, (1..=entries.len() as u64 + 1).collect::<Vec<_>>());
}

// ----- waits -------------------------------------------------------------------

#[test]
fn every_wait_times_out_with_what_it_waited_for() {
    let rig = Rig::new("wait-timeouts");
    rig.post("/api/control/pause", json!({}));
    let last = rig.last();
    let timed = |path: &str| {
        let start = Instant::now();
        let r = rig.get(path);
        (r, start.elapsed())
    };
    let (r, took) = timed(&format!("/api/journal?after={}&wait=300", last));
    assert_eq!((r.status, r.json()["timed_out"].as_bool()), (200, Some(true)));
    assert!(took >= Duration::from_millis(280), "{:?}", took);
    let (r, took) = timed(&format!("/api/control/wait?after={}&timeout_ms=300", last));
    assert_eq!(r.status, 504, "{}", r.body);
    assert!(took >= Duration::from_millis(280) && took < Duration::from_secs(5), "{:?}", took);
    // An operation that doesn't end in the wait: pending, at once after it.
    let typed = rig.post("/api/input/wait?wait=false", json!({"ms": 60000})).json();
    let op = typed["op"].as_u64().unwrap();
    let (r, took) = timed(&format!("/api/ops/{}?wait=300", op));
    assert_eq!((r.status, r.json()["state"].as_str()), (200, Some("pending")), "{}", r.body);
    assert!(took >= Duration::from_millis(280), "{:?}", took);
    let (r, _) = timed(&format!("/api/control/wait?op={}&timeout_ms=300", op));
    assert_eq!((r.status, r.json()["op"].as_u64()), (504, Some(op)), "{}", r.body);
    let (r, _) = timed("/api/log?after_n=999999999&wait=300");
    assert_eq!((r.status, r.json()["lines"].as_array().map(Vec::len)), (200, Some(0)));
    // The longest wait is bounded.
    let resumed = rig.post("/api/control/resume", json!({})).json();
    let step = http(rig.addr, "POST", "/api/control/step", &[], Some(&json!({"count": 1u64 << 40, "timeout_ms": 300})));
    assert_eq!(step.status, 504, "{}", step.body);
    assert!(step.json()["error"].as_str().unwrap().contains("timed out"), "the message clients look for");
    assert!(resumed["op"].is_u64());
    rig.until_idle();
}

#[test]
fn a_strict_match_load_ends_the_waits_on_what_it_cancelled() {
    let rig = Rig::new("wait-load");
    rig.post("/api/control/pause", json!({}));
    let saved = rig.post("/api/state/save", json!({"path": "a.state", "strict_match": true}));
    assert_eq!(saved.status, 200, "{}", saved.body);
    let addr = rig.addr;
    // Input that waits a minute, waited for three ways.
    let typing = std::thread::spawn(move || http(addr, "POST", "/api/input/wait", &[], Some(&json!({"ms": 60000}))));
    let deadline = Instant::now() + Duration::from_secs(10);
    let op = loop {
        let queued = rig.get("/api/journal?after=0&type=input_queued").json();
        if let Some(e) = queued["entries"].as_array().unwrap().last() {
            break e["cause"].as_u64().unwrap();
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    };
    let op_wait = std::thread::spawn(move || http(addr, "GET", &format!("/api/ops/{}?wait=60000", op), &[], None));
    let stop_wait = std::thread::spawn(move || http(addr, "GET", &format!("/api/control/wait?op={}&timeout_ms=60000", op), &[], None));
    std::thread::sleep(Duration::from_millis(200));
    let start = Instant::now();
    let loaded = rig.post("/api/state/load", json!({"path": "a.state", "strict_match": true}));
    assert_eq!(loaded.status, 200, "{}", loaded.body);
    assert_eq!(loaded.json()["debugger_reset"]["input"], 1, "{}", loaded.body);
    let typed = typing.join().unwrap();
    assert_eq!(typed.status, 409, "{}", typed.body);
    assert_eq!(typed.json()["error"], "a save state was loaded");
    let ended = op_wait.join().unwrap().json();
    assert_eq!((ended["state"].as_str(), ended["end"]["type"].as_str()), (Some("cancelled"), Some("debugger_reset")), "{}", ended);
    assert_eq!(stop_wait.join().unwrap().status, 409);
    assert!(start.elapsed() < Duration::from_secs(5));
    // The journal says it in order.
    let tail: Vec<String> = rig.journal().iter().rev().take(3).map(|e| e["type"].as_str().unwrap().to_string()).collect();
    assert_eq!(tail, ["paused", "debugger_reset", "state_loaded"]);
    rig.until_idle();
}

#[test]
fn clients_that_go_away_mid_wait_leave_nothing_behind() {
    let rig = Rig::new("wait-gone");
    let cs = rig.at_entry("LOOP.COM");
    let last = rig.last();
    let op = rig.post("/api/input/wait?wait=false", json!({"ms": 60000})).json()["op"].as_u64().unwrap();
    let waits = [
        format!("/api/journal?after={}&type=paused&wait=60000", last),
        format!("/api/ops/{}?wait=60000", op),
        format!("/api/control/wait?op={}&timeout_ms=60000", op),
        format!("/api/control/wait?after={}&timeout_ms=60000", last),
        "/api/log?after_n=999999999&wait=60000".to_string(),
    ];
    let mut sockets = Vec::new();
    for _ in 0..20 {
        for path in &waits {
            let mut stream = TcpStream::connect(rig.addr).unwrap();
            stream.write_all(format!("GET {} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n", path).as_bytes()).unwrap();
            sockets.push(stream);
        }
    }
    // Requests that wait for their operation, then go away.
    for _ in 0..10 {
        let mut stream = TcpStream::connect(rig.addr).unwrap();
        let body = json!({"count": 1u64 << 40, "timeout_ms": 60000}).to_string();
        let request = format!("POST /api/control/step HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}", body.len(), body);
        stream.write_all(request.as_bytes()).unwrap();
        sockets.push(stream);
    }
    std::thread::sleep(Duration::from_millis(300));
    assert!(rig.shared.busy.load(std::sync::atomic::Ordering::Acquire) >= 100);
    drop(sockets);
    // The server lets go of them, and the hub goes on.
    rig.until_idle();
    rig.post("/api/control/pause", json!({}));
    let resumed = rig.post("/api/control/resume", json!({"until": format!("{}:0101", cs)})).json();
    let stop = rig.get(&format!("/api/control/wait?op={}", resumed["op"]));
    assert_eq!((stop.status, stop.json()["reason"].as_str()), (200, Some("breakpoint")), "{}", stop.body);
}

// ----- streams -----------------------------------------------------------------

/// Threads that append entries as fast as requests go (breakpoints set),
/// `n` each; joined, they return the longest a request took.
fn writers(addr: SocketAddr, threads: u64, n: u64) -> Vec<JoinHandle<Duration>> {
    (0..threads)
        .map(|t| {
            std::thread::spawn(move || {
                let mut longest = Duration::ZERO;
                for i in 0..n {
                    let start = Instant::now();
                    let r = http(addr, "POST", "/api/breakpoints", &[], Some(&json!({"addr": format!("{:05X}", 0x10000 + t * 0x1000 + i)})));
                    assert_eq!(r.status, 200, "{}", r.body);
                    longest = longest.max(start.elapsed());
                }
                longest
            })
        })
        .collect()
}

/// Read a stream's entries (the `gone` message as seq 0), up to `n` of them
/// or until nothing comes for `quiet`.
fn read_entries(socket: &mut Socket, n: usize, quiet: Duration) -> Vec<Value> {
    if let tungstenite::stream::MaybeTlsStream::Plain(s) = socket.get_ref() {
        s.set_read_timeout(Some(quiet)).unwrap();
    }
    let mut out = Vec::new();
    while out.len() < n {
        match socket.read() {
            Ok(tungstenite::Message::Text(t)) => out.push(serde_json::from_str(t.as_str()).unwrap()),
            Ok(tungstenite::Message::Close(_)) => break,
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) =>
            {
                break;
            }
            Err(e) => panic!("{}", e),
        }
    }
    out
}

/// Read a stream's messages until one is entry `last`. A socket that was
/// left unread long enough to fill its buffers can be quiet for seconds
/// once read again: TCP finds the reopened window with its zero-window
/// probes, whose interval doubled while the reader slept (on Linux). So
/// this waits up to a minute, not for a quiet spell.
fn read_through(socket: &mut Socket, last: u64) -> Vec<Value> {
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut out: Vec<Value> = Vec::new();
    while out.last().is_none_or(|e| e["seq"].as_u64() != Some(last)) {
        assert!(Instant::now() < deadline, "entry {} not streamed; the last of {} messages: {:?}", last, out.len(), out.last());
        out.extend(read_entries(socket, 1, Duration::from_secs(1)));
    }
    out
}

#[test]
fn a_stream_reconnected_at_any_point_misses_and_repeats_nothing() {
    let rig = Rig::new("ws-reconnect");
    rig.post("/api/control/pause", json!({}));
    let writing = writers(rig.addr, 4, 300);
    let mut seen: Vec<u64> = Vec::new();
    let mut rand = Rand(0x5EED);
    let mut connections = 0;
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let after = seen.last().copied().unwrap_or(0);
        let mut socket = ws(rig.addr, &format!("/ws/events?after={}&volatile=false", after));
        connections += 1;
        // A few entries, or many, then away.
        let take = 1 + rand.below(60) as usize;
        let got = read_entries(&mut socket, take, Duration::from_millis(500));
        let _ = socket.close(None);
        seen.extend(got.iter().map(|e| e["seq"].as_u64().expect("no gone: the ring keeps them all")));
        // Done once the writers are and the stream caught up with them.
        if writing.iter().all(|w| w.is_finished()) && seen.last().copied() == Some(rig.last()) {
            break;
        }
        assert!(Instant::now() < deadline);
    }
    for w in writing {
        w.join().unwrap();
    }
    let last = rig.last();
    assert_eq!(seen, (1..=last).collect::<Vec<_>>(), "every entry once, in order, over {} connections", connections);
    assert!(connections > 20);
}

#[test]
fn a_slow_reader_holds_up_nothing_and_loses_nothing_the_ring_keeps() {
    let rig = Rig::new("ws-slow");
    rig.post("/api/control/pause", json!({}));
    let from = rig.last();
    // Connected, then not read while 2000 entries come.
    let mut socket = ws(rig.addr, &format!("/ws/events?after={}", from));
    let longest = writers(rig.addr, 4, 500).into_iter().map(|w| w.join().unwrap()).max().unwrap();
    assert!(longest < Duration::from_secs(1), "a request took {:?} while the reader slept", longest);
    let last = rig.last();
    let got = read_through(&mut socket, last);
    let seqs: Vec<u64> = got.iter().filter(|e| e["type"] != "log" && e["type"] != "video_mode").map(|e| e["seq"].as_u64().unwrap()).collect();
    assert_eq!(seqs, (from + 1..=last).collect::<Vec<_>>());
}

#[test]
fn a_reader_slower_than_the_ring_is_told_what_it_lost() {
    let rig = Rig::with_capacity("ws-slower", 64);
    rig.post("/api/control/pause", json!({}));
    let from = rig.last();
    let mut socket = ws(rig.addr, &format!("/ws/events?after={}&volatile=false", from));
    // A large backlog of large entries (256-byte writes, kept as hex): the
    // socket's buffers fill and the ring moves on.
    let addr = rig.addr;
    let big: Vec<_> = (0..4u64)
        .map(|t| {
            std::thread::spawn(move || {
                for i in 0..1000u64 {
                    let hex = format!("{:02X}", (t * 1000 + i) % 256).repeat(256);
                    let r = http(addr, "PUT", "/api/memory", &[], Some(&json!({"addr": format!("{:05X}", 0x20000 + t * 0x1000), "hex": hex})));
                    assert_eq!(r.status, 200, "{}", r.body);
                }
            })
        })
        .collect();
    for w in big {
        w.join().unwrap();
    }
    let last = rig.last();
    let got = read_through(&mut socket, last);
    // In order, no repeat; a gap only right after a `gone`, which says
    // where it goes on.
    let mut prev = from;
    for e in &got {
        if e["type"] == "gone" {
            let first = e["first"].as_u64().unwrap();
            assert!(first > prev + 1, "{} after {}", e, prev);
            prev = first - 1;
            continue;
        }
        let seq = e["seq"].as_u64().unwrap();
        assert_eq!(seq, prev + 1, "after {}: {}", prev, e);
        prev = seq;
    }
    assert_eq!(prev, last, "caught up");
    assert!(got.iter().any(|e| e["type"] == "gone"), "the reader fell behind the ring");
}

// ----- bounds ------------------------------------------------------------------

#[test]
fn the_ring_and_the_operations_stay_bounded_and_say_what_went() {
    let rig = Rig::new("bounds");
    rig.at_entry("LOOP.COM");
    // A resume that ends only when paused: pending throughout.
    let resume = rig.post("/api/control/resume", json!({})).json()["op"].as_u64().unwrap();
    // 2600 input requests, each an operation of two entries.
    let addr = rig.addr;
    let threads: Vec<_> = (0..4)
        .map(|_| {
            std::thread::spawn(move || {
                (0..650)
                    .map(|_| {
                        let r = http(addr, "POST", "/api/input/type?wait=false", &[], Some(&json!({"text": ""})));
                        assert_eq!(r.status, 200, "{}", r.body);
                        r.json()["op"].as_u64().unwrap()
                    })
                    .collect::<Vec<u64>>()
            })
        })
        .collect();
    let mut inputs: Vec<u64> = threads.into_iter().flat_map(|t| t.join().unwrap()).collect();
    inputs.sort_unstable();
    // Delivered, all of them.
    let deadline = Instant::now() + Duration::from_secs(20);
    while rig.get("/api/status").json()["input_queue"] != 0 || rig.get(&format!("/api/ops/{}", inputs.last().unwrap())).json()["state"] != "done" {
        assert!(Instant::now() < deadline, "the input is delivered");
        std::thread::sleep(Duration::from_millis(20));
    }
    let status = rig.get("/api/status").json();
    let (first, last) = (status["first_seq"].as_u64().unwrap(), status["seq"].as_u64().unwrap());
    assert!(last > 5000, "{}", last);
    assert_eq!(last - first + 1, super::journal::CAPACITY as u64, "the ring is full and no larger");
    // Entries before the first: 410 with where it starts, and the streams say so.
    let gone = rig.get("/api/journal?after=0");
    assert_eq!((gone.status, gone.json()["first"].as_u64()), (410, Some(first)));
    let page = rig.get(&format!("/api/journal?after={}&limit=10", first - 1)).json();
    assert_eq!(page["entries"][0]["seq"].as_u64(), Some(first));
    let mut socket = ws(rig.addr, "/ws/events?after=10&volatile=false");
    assert_eq!(next(&mut socket).unwrap(), json!({"type": "gone", "first": first}));
    // The operations that ended first are gone; the latest 1024 to end
    // kept, and the pending resume, however old. Numbers are handed out
    // before the requests reach the machine, so the order they ended in is
    // the order of their `input_delivered` entries, not their numbers.
    let kept = super::journal::OPS_CAPACITY;
    let ring = rig.get(&format!("/api/journal?after={}&limit={}", first - 1, super::journal::CAPACITY)).json();
    let delivered: Vec<u64> =
        ring["entries"].as_array().unwrap().iter().filter(|e| e["type"] == "input_delivered").map(|e| e["cause"].as_u64().unwrap()).collect();
    assert!(delivered.len() >= kept, "{}", delivered.len());
    let latest: std::collections::HashSet<u64> = delivered[delivered.len() - kept..].iter().copied().collect();
    for &op in &inputs {
        let r = rig.get(&format!("/api/ops/{}", op));
        if latest.contains(&op) {
            assert_eq!(r.json()["state"], "done", "{}", op);
        } else {
            assert_eq!(r.status, 410, "{}", op);
        }
    }
    assert_eq!(rig.get(&format!("/api/ops/{}", resume)).json()["state"], "pending");
    {
        let j = rig.shared.journal.lock().unwrap();
        assert_eq!(j.ops_len(), kept + 1, "the ended ones and the one pending");
    }
    // Its waiter gets its end.
    let addr = rig.addr;
    let waiter = std::thread::spawn(move || http(addr, "GET", &format!("/api/ops/{}?wait=10000", resume), &[], None).json());
    std::thread::sleep(Duration::from_millis(100));
    rig.post("/api/control/pause", json!({}));
    assert_eq!(waiter.join().unwrap()["state"], "stopped");
}

// ----- deferred requests -------------------------------------------------------

/// Bytes as hex with no spaces, upper case.
fn hex_of(v: &Value) -> String {
    v.as_str().unwrap_or_else(|| panic!("hex: {}", v)).replace(' ', "").to_uppercase()
}

/// The two bytes of a one-line hex dump.
fn dumped(text: &str) -> String {
    text.split_whitespace().skip(1).take(2).collect::<String>().to_uppercase()
}

/// What a client did: (action, status, request, body, state seq header).
type Done = (u64, u16, u64, Value, Option<u64>);

#[test]
fn saves_loads_and_speed_changes_take_effect_in_the_order_of_the_journal() {
    // Long frames, so that requests pile up in one.
    let rig = Rig::with("deferred", Options { sleep: Duration::from_millis(25), ..Options::default() });
    let cs = rig.at_entry("COUNT.COM");
    let addr = rig.addr;
    // COUNT.COM counts at 0200; 0300 changes only by what the clients do.
    let at = format!("{}:0300", cs);
    let first = dumped(&rig.get(&format!("/api/memory?addr={}&len=2", at)).body);
    let clients: Vec<_> = (0..8u64)
        .map(|i| {
            let at = at.clone();
            std::thread::spawn(move || {
                let mut rand = Rand(0xD1B5_4A32_D192_ED03 ^ ((i + 1) * 0x9E37_79B9));
                let mut done: Vec<Done> = Vec::new();
                for _ in 0..80 {
                    let action = rand.below(10);
                    let path = format!("s{}.state", rand.below(3));
                    let strict = rand.below(2) == 0;
                    let r = match action {
                        0 | 1 => http(addr, "PUT", "/api/memory", &[], Some(&json!({"addr": at, "hex": format!("{:04X}", rand.below(0x10000))}))),
                        2 => {
                            let read = http(addr, "GET", &format!("/api/memory?addr={}&len=2", at), &[], None);
                            let seq = read.header("x-debug-state-seq").unwrap().to_string();
                            http(addr, "PUT", "/api/memory", &[("If-State-Seq", &seq)], Some(&json!({"addr": at, "hex": format!("{:04X}", rand.below(0x10000))})))
                        }
                        3 => http(addr, "POST", "/api/state/save", &[], Some(&json!({"path": path, "strict_match": strict}))),
                        4 => http(addr, "POST", "/api/state/load", &[], Some(&json!({"path": path, "strict_match": strict}))),
                        5 => http(addr, "POST", "/api/speed", &[], Some(&json!({"cycles": format!("{}", 1000 + rand.below(9000))}))),
                        6 => http(addr, "POST", "/api/control/resume", &[], Some(&json!({}))),
                        7 => http(addr, "POST", "/api/control/pause", &[], Some(&json!({}))),
                        _ => http(addr, "GET", &format!("/api/memory?addr={}&len=2", at), &[], None),
                    };
                    let allowed: &[u16] = match action {
                        2 => &[200, 409],
                        3 => &[200, 409],
                        4 => &[200, 404, 409],
                        _ => &[200],
                    };
                    assert!(allowed.contains(&r.status), "action {}: {} {}", action, r.status, r.body);
                    let seq = r.header("x-debug-state-seq").and_then(|s| s.parse().ok());
                    let body = if action >= 8 { json!({"hex": dumped(&r.body)}) } else { r.json() };
                    done.push((action, r.status, r.header("x-debug-req").unwrap().parse().unwrap(), body, seq));
                }
                done
            })
        })
        .collect();
    let done: Vec<Vec<Done>> = clients.into_iter().map(|c| c.join().expect("a client failed")).collect();
    rig.post("/api/control/pause", json!({}));
    let entries = rig.journal();

    // Replayed in the journal's order, the value at 0300 after each entry.
    let mut value = first.clone();
    let mut states: std::collections::HashMap<String, String> = Default::default();
    let mut after: Vec<String> = vec![first];
    let mut by_cause: std::collections::HashMap<u64, Vec<&Value>> = Default::default();
    for e in &entries {
        if let Some(c) = e["cause"].as_u64() {
            by_cause.entry(c).or_default().push(e);
        }
        match e["type"].as_str().unwrap() {
            "memory_written" => {
                assert_eq!(hex_of(&e["old"]), value, "a write replaced what the journal says was there: {}", e);
                value = hex_of(&e["new"]);
            }
            "state_saved" => {
                states.insert(e["path"].as_str().unwrap().to_string(), value.clone());
            }
            "state_loaded" => {
                value = states.get(e["path"].as_str().unwrap()).unwrap_or_else(|| panic!("loaded before saved: {}", e)).clone();
            }
            _ => {}
        }
        after.push(value.clone());
    }
    assert_eq!(dumped(&rig.get(&format!("/api/memory?addr={}&len=2", at)).body), value, "the end is where the journal says");

    let (mut saves, mut loads, mut speeds) = (0, 0, 0);
    for client in &done {
        let mut last_seq = 0;
        for (action, status, req, body, state_seq) in client {
            let mine = by_cause.get(req).cloned().unwrap_or_default();
            let kinds: Vec<&str> = mine.iter().map(|e| e["type"].as_str().unwrap()).collect();
            // Each client's requests, one after the other, are in the
            // journal in that order.
            if let Some(e) = mine.first() {
                let seq = e["seq"].as_u64().unwrap();
                assert!(seq > last_seq, "request {} at {} after {}", req, seq, last_seq);
                last_seq = mine.last().unwrap()["seq"].as_u64().unwrap();
            }
            match (action, status) {
                (0..=2, 200) => {
                    assert_eq!(kinds, ["memory_written"], "{}", req);
                    assert_eq!((hex_of(&body["old"]), hex_of(&body["new"])), (hex_of(&mine[0]["old"]), hex_of(&mine[0]["new"])));
                }
                (3, 200) => {
                    saves += 1;
                    assert_eq!(kinds, ["state_saved"], "{}", req);
                }
                (4, 200) => {
                    loads += 1;
                    assert_eq!(kinds[0], "state_loaded", "{}", req);
                }
                (5, 200) => {
                    speeds += 1;
                    assert_eq!(kinds, ["speed_changed"], "{}", req);
                }
                (2..=4, 400..) => assert!(kinds.is_empty() || kinds == ["op_failed"], "a refused request changed nothing: {} {:?}", req, kinds),
                (8.., 200) => {
                    // A read is the value after the entry its state seq names.
                    let seq = state_seq.expect("a read's state seq") as usize;
                    assert_eq!(hex_of(&body["hex"]), after[seq], "read {} at state {}", req, seq);
                }
                _ => {}
            }
        }
    }
    assert!(saves > 20 && loads > 20 && speeds > 20, "saves {} loads {} speeds {}", saves, loads, speeds);
}

// ----- If-State-Seq ------------------------------------------------------------

#[test]
fn if_state_seq_guards_every_request_the_emulator_carries_out() {
    let rig = Rig::new("if-state-all");
    rig.post("/api/control/pause", json!({}));
    let mount = std::env::temp_dir().join(format!("rust-dos-http-if-state-mount-{}", std::process::id()));
    std::fs::create_dir_all(&mount).unwrap();
    let saved = rig.post("/api/state/save", json!({"path": "if.state"}));
    assert_eq!(saved.status, 200, "{}", saved.body);
    let mount = mount.display().to_string();
    // Every request the emulator carries out (the screenshot, which the
    // front end draws, aside).
    let requests: Vec<(&str, String, Option<Value>)> = vec![
        ("GET", "/api/status".into(), None),
        ("GET", "/api/stats".into(), None),
        ("GET", "/api/screen/text".into(), None),
        ("GET", "/api/trace".into(), None),
        ("POST", "/api/trace".into(), Some(json!({"clear": true}))),
        ("POST", "/api/ports".into(), Some(json!({"enabled": true}))),
        ("GET", "/api/ports".into(), None),
        ("POST", "/api/input/type?wait=false".into(), Some(json!({"text": ""}))),
        ("DELETE", "/api/input".into(), None),
        ("GET", "/api/drive".into(), None),
        ("PUT", "/api/drive/E".into(), Some(json!({"path": mount}))),
        ("DELETE", "/api/drive/E".into(), None),
        ("POST", "/api/drive/swap".into(), None),
        ("POST", "/api/drive/sync".into(), None),
        ("POST", "/api/state/save".into(), Some(json!({"path": "if2.state", "strict_match": true}))),
        ("POST", "/api/state/load".into(), Some(json!({"path": "if.state", "strict_match": true}))),
        ("POST", "/api/speed".into(), Some(json!({"cycles": "4000"}))),
        ("GET", "/api/control/wait".into(), None),
        ("POST", "/api/control/step".into(), Some(json!({"count": 1}))),
        ("POST", "/api/control/step_over".into(), Some(json!({}))),
        ("POST", "/api/control/resume".into(), Some(json!({}))),
        ("POST", "/api/control/pause".into(), Some(json!({}))),
        ("GET", "/api/registers".into(), None),
        ("GET", "/api/registers/fpu".into(), None),
        ("PUT", "/api/registers".into(), Some(json!({"cx": "1234"}))),
        ("GET", "/api/memory?addr=0200:0000&len=4".into(), None),
        ("PUT", "/api/memory".into(), Some(json!({"addr": "0200:0000", "hex": "01"}))),
        ("GET", "/api/disasm".into(), None),
        ("GET", "/api/breakpoints".into(), None),
        ("POST", "/api/breakpoints".into(), Some(json!({"addr": "1234:0000"}))),
        ("DELETE", "/api/breakpoints?addr=1234:0000".into(), None),
        ("GET", "/api/watchpoints".into(), None),
        ("POST", "/api/watchpoints".into(), Some(json!({"addr": "0200:0000", "len": 2}))),
        ("DELETE", "/api/watchpoints?addr=0200:0000".into(), None),
        ("GET", "/api/ivt".into(), None),
        ("GET", "/api/gdt".into(), None),
        ("GET", "/api/ldt".into(), None),
        ("GET", "/api/idt".into(), None),
        ("GET", "/api/tss".into(), None),
        ("GET", "/api/pagewalk?addr=0".into(), None),
        ("GET", "/api/xms".into(), None),
        ("GET", "/api/net".into(), None),
        ("GET", "/api/serial".into(), None),
        ("GET", "/api/printer".into(), None),
        ("POST", "/api/printer/eject".into(), None),
        ("GET", "/api/gus".into(), None),
        ("GET", "/api/awe32".into(), None),
        ("GET", "/api/exceptions".into(), None),
    ];
    for (method, path, body) in &requests {
        if rig.get("/api/status").json()["paused"] != true {
            rig.post("/api/control/pause", json!({}));
        }
        let status = rig.get("/api/status").json();
        let (state, last) = (status["state_seq"].as_u64().unwrap(), status["seq"].as_u64().unwrap());
        // Held against a state the machine left: refused, nothing done.
        let stale = (state - 1).to_string();
        let refused = http(rig.addr, method, path, &[("If-State-Seq", &stale)], body.as_ref());
        assert_eq!(refused.status, 409, "{} {} with an old state: {}", method, path, refused.body);
        assert_eq!(refused.json()["precondition"], "state", "{} {}: {}", method, path, refused.body);
        assert_eq!(rig.last(), last, "{} {} refused, yet the journal moved", method, path);
        // Held against the state it is in: carried out.
        let current = state.to_string();
        let done = http(rig.addr, method, path, &[("If-State-Seq", &current)], body.as_ref());
        assert!(done.status < 300, "{} {} in its state: {} {}", method, path, done.status, done.body);
        // Input is typed while paused too, and its `input_delivered` entry
        // would move the journal under the next request's check.
        if path.starts_with("/api/input/") {
            let op = done.json()["op"].as_u64().unwrap();
            let ended = rig.get(&format!("/api/ops/{}?wait=10000", op));
            assert_eq!(ended.status, 200, "{} {}: {}", method, path, ended.body);
        }
    }
    // The server's own: no machine state to hold them against.
    let state = rig.get("/api/status").json()["state_seq"].as_u64().unwrap().to_string();
    for path in ["/", "/ui", "/api", "/api/version", "/api/journal?after=0", "/api/ops/1", "/api/log", "/api/control/wait?after=0", "/api/control/wait?op=1"] {
        let r = http(rig.addr, "GET", path, &[("If-State-Seq", &state)], None);
        assert_eq!(r.status, 400, "{}: {}", path, r.body);
        assert!(r.json()["error"].as_str().unwrap().contains("If-State-Seq"), "{}: {}", path, r.body);
    }
    for path in ["/ws/events", "/ws/trace", "/ws/screen", "/ws/audio", "/ws/input"] {
        let upgrade = format!(
            "GET {} HTTP/1.1\r\nHost: x\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nIf-State-Seq: {}\r\n\r\n",
            path, state
        );
        assert_eq!(raw_status(rig.addr, upgrade.as_bytes()), 400, "{}", path);
    }
}

// ----- determinism -------------------------------------------------------------

/// A deterministic session driven over HTTP, in host frames of `frame_ms`
/// emulated milliseconds that each wait `sleep`: the journal with the
/// host's time left out and request numbers counted from the first.
fn deterministic_session(name: &str, frame_ms: u64, sleep: Duration) -> Vec<Value> {
    let rig = Rig::with(name, Options { deterministic: Some(frame_ms), sleep, ..Options::default() });
    let go = |until_ms: u64| {
        let op = rig.post("/api/control/resume", json!({"until_ms": until_ms})).json()["op"].as_u64().unwrap();
        let stop = rig.get(&format!("/api/control/wait?op={}&timeout_ms=60000", op));
        assert_eq!((stop.status, stop.json()["reason"].as_str()), (200, Some("time")), "{}", stop.body);
    };
    let input = |events: Value| {
        let r = rig.post("/api/input/batch?wait=false", json!({"events": events}));
        assert_eq!(r.status, 200, "{}", r.body);
    };
    input(json!([{"type": "wait", "ms": 100}, {"type": "type", "text": "PROBE\n", "delay_ms": 25}]));
    go(700);
    assert_eq!(rig.post("/api/state/save", json!({"path": "d.state", "strict_match": true})).status, 200);
    input(json!([{"type": "type", "text": "COUNT\n"}]));
    go(1300);
    // Guarded: a write in the state just read.
    let seq = rig.get("/api/memory?addr=0200:0000&len=2").header("x-debug-state-seq").unwrap().to_string();
    let w = http(rig.addr, "PUT", "/api/memory", &[("If-State-Seq", &seq)], Some(&json!({"addr": "0200:0000", "hex": "AA55"})));
    assert_eq!(w.status, 200, "{}", w.body);
    let loaded = rig.post("/api/state/load", json!({"path": "d.state", "strict_match": true}));
    assert_eq!(loaded.status, 200, "{}", loaded.body);
    input(json!([{"type": "type", "text": "PROBE\n"}]));
    go(1800);
    let mut numbers: std::collections::HashMap<u64, u64> = Default::default();
    let mut renumber = |v: &mut Value| {
        if let Some(n) = v.as_u64() {
            let next = numbers.len() as u64 + 1;
            *v = (*numbers.entry(n).or_insert(next)).into();
        }
    };
    rig.journal()
        .into_iter()
        .map(|mut e| {
            let o = e.as_object_mut().unwrap();
            o.remove("t_ms");
            for key in ["cause", "by", "op", "supersedes"] {
                if let Some(v) = o.get_mut(key) {
                    renumber(v);
                }
            }
            if let Some(Value::Array(list)) = o.get_mut("cancelled") {
                list.iter_mut().for_each(&mut renumber);
            }
            // Where the rig keeps its states.
            if let Some(path) = o.get_mut("path") {
                *path = path.as_str().map(|p| p.rsplit(['/', '\\']).next().unwrap().to_string()).into();
            }
            e
        })
        .collect()
}

#[test]
fn a_deterministic_session_over_http_is_the_same_whatever_the_host_does() {
    let a = deterministic_session("det-a", 16, Duration::from_millis(1));
    let b = deterministic_session("det-b", 41, Duration::from_millis(9));
    assert_eq!(a.len(), b.len(), "as many entries");
    for (x, y) in a.iter().zip(&b) {
        assert_eq!(x, y, "the same journal");
    }
    let kinds: Vec<&str> = a.iter().map(|e| e["type"].as_str().unwrap()).collect();
    for kind in ["input_delivered", "state_saved", "memory_written", "state_loaded", "debugger_reset", "program_started"] {
        assert!(kinds.contains(&kind), "{} in {:?}", kind, kinds);
    }
}

/// `/api/stats` carries the Stats page's numbers, which tell whether the
/// machine keeps real time and how much of the host it takes.
#[test]
fn stats_tell_the_stats_page_s_numbers() {
    let rig = Rig::new("frame-stats");
    rig.post("/api/control/run", json!({"command": "COUNT.COM"}));
    let deadline = Instant::now() + Duration::from_secs(20);
    let frame = loop {
        let stats = rig.get("/api/stats").json();
        if stats["frame"]["emulated_speed"].as_f64().is_some_and(|s| s > 0.0) {
            break stats["frame"].clone();
        }
        assert!(Instant::now() < deadline, "the machine runs: {}", stats);
        std::thread::sleep(Duration::from_millis(20));
    };
    for key in ["fps", "refresh_hz", "emulated_speed", "cpu_use", "halted", "recompiled", "render_ms"] {
        assert!(frame[key].is_number(), "{} in {}", key, frame);
    }
    let cpu_use = frame["cpu_use"].as_f64().unwrap();
    assert!(cpu_use > 0.0 && cpu_use <= 100.0, "{}", frame);
    assert!((frame["refresh_hz"].as_f64().unwrap() - 70.0).abs() < 1.0, "text mode at 70 Hz: {}", frame);
}

// ----- the debugger page -------------------------------------------------------

/// What the page (ui.html) reads, as it reads it: its requests, and the
/// stream it opens with no parameters.
#[test]
fn the_debugger_page_gets_what_it_reads() {
    let rig = Rig::new("ui");
    let page = rig.get("/ui");
    assert_eq!(page.status, 200);
    assert!(page.body.contains("new WebSocket(`ws://${location.host}/ws/events`)"), "the page this test follows");
    let cs = rig.at_entry("COUNT.COM");
    let mut events = ws(rig.addr, "/ws/events");
    let mut input = ws(rig.addr, "/ws/input");
    // Its polls.
    let status = rig.get("/api/status").json();
    for key in ["paused", "cs_ip", "cpu_mode", "shell_idle", "video", "fps", "trace"] {
        assert!(status.get(key).is_some(), "{} in {}", key, status);
    }
    assert!(status["trace"]["enabled"].is_boolean());
    assert!(rig.get("/api/stats").json()["mips"].is_number());
    assert!(rig.get("/api/disasm?format=json&count=1&addr=CS:EIP").json()["rows"][0]["phys"].is_string());
    assert!(rig.get("/api/memory?format=base64&len=16&addr=SS:FFF0").json()["base64"].is_string());
    assert!(rig.get("/api/log?limit=500").json()["lines"].is_array());
    let trace = rig.get("/api/trace?format=json&last_n=200");
    assert!(trace.json()["entries"].is_array() && trace.json()["truncated"] == false);
    assert_eq!(trace.header("x-trace-truncated"), Some("false"));
    assert!(rig.get("/api/breakpoints").json().is_object());
    assert!(rig.get("/api/watchpoints").json().is_object());
    // The stream: a stop with its reason and what the watchpoint saw, a
    // resume, a log line.
    rig.post("/api/watchpoints", json!({"addr": format!("{}:0200", cs), "len": 2}));
    rig.post("/api/control/resume", json!({}));
    rig.until_paused();
    let mount = std::env::temp_dir().join(format!("rust-dos-http-ui-mount-{}", std::process::id()));
    std::fs::create_dir_all(&mount).unwrap();
    assert_eq!(http(rig.addr, "PUT", "/api/drive/E", &[], Some(&json!({"path": mount.display().to_string()}))).status, 200);
    let seen = read_entries(&mut events, usize::MAX, Duration::from_millis(500));
    let paused = seen.iter().find(|e| e["type"] == "paused").expect("a stop");
    assert_eq!(paused["reason"], "watchpoint");
    for key in ["addr", "old", "new"] {
        assert!(paused["watch"][key].is_string(), "{} in {}", key, paused);
    }
    assert!(seen.iter().any(|e| e["type"] == "resumed"));
    let log = seen.iter().find(|e| e["type"] == "log").expect("a log line");
    assert!(log["t_ms"].is_number() && log["line"].is_string(), "{}", log);
    // The input it sends, and the error it shows.
    input.send(tungstenite::Message::Text(json!({"events": [{"type": "key", "key": "a"}], "wait": false}).to_string().into())).unwrap();
    let reply = read_entries(&mut input, 1, Duration::from_secs(5));
    assert_eq!(reply[0]["ok"], true, "{:?}", reply);
    input.send(tungstenite::Message::Text(json!({"events": [{"type": "key", "key": "nokey"}], "wait": false}).to_string().into())).unwrap();
    let reply = read_entries(&mut input, 1, Duration::from_secs(5));
    assert!(reply[0]["ok"] == false && reply[0]["error"].is_string(), "{:?}", reply);
}

// ----- what outlives the ring --------------------------------------------------

#[test]
fn a_stop_and_an_operation_s_end_outlive_their_entries_in_the_ring() {
    let rig = Rig::with_capacity("outlive", 64);
    rig.at_probe_entry();
    let stepped = rig.post("/api/control/step", json!({"count": 1})).json();
    let op = stepped["op"].as_u64().unwrap();
    // Input waited for at the machine's level.
    let typed = rig.post("/api/input/type?wait=false", json!({"text": ""})).json()["op"].as_u64().unwrap();
    for i in 0..100 {
        rig.post("/api/breakpoints", json!({"addr": format!("{:05X}", 0x30000 + i)}));
    }
    assert!(rig.get("/api/status").json()["first_seq"].as_u64() > stepped["stop"]["seq"].as_u64(), "the stop's entry is gone");
    // Still paused there, as every reply says.
    let waited = rig.get("/api/control/wait?timeout_ms=2000");
    assert_eq!((waited.status, waited.json()["paused"].as_bool(), waited.json()["reason"].as_str()), (200, Some(true), Some("step")), "{}", waited.body);
    let paused = rig.post("/api/control/pause", json!({})).json();
    assert_eq!(paused["stop"]["seq"], stepped["stop"]["seq"], "{}", paused);
    assert_eq!(rig.get("/api/status").json()["stop"]["seq"], stepped["stop"]["seq"]);
    // The operation's end too.
    let ended = rig.get(&format!("/api/ops/{}", op)).json();
    assert_eq!((ended["state"].as_str(), ended["stop"]["reason"].as_str()), (Some("stopped"), Some("step")), "{}", ended);
    let again = rig.get(&format!("/api/control/wait?op={}", op)).json();
    assert_eq!((again["reason"].as_str(), again["registers"].is_object()), (Some("step"), true), "{}", again);
    // An input operation waited for with wait?op= answers as input does.
    let input = rig.get(&format!("/api/control/wait?op={}", typed));
    assert_eq!((input.json()["ok"].as_bool(), input.json()["done"].as_bool()), (Some(true), Some(true)), "{}", input.body);
}

/// `/api/profile`: a profile of a counting loop finds the loop, ends at its
/// duration, gives folded stacks as text, and refuses what it can't do.
#[test]
fn a_profile_finds_the_loop_the_machine_runs() {
    let _one = crate::debug::profile::ONE_PROFILE.lock().unwrap_or_else(|e| e.into_inner());
    let rig = Rig::new("profile");
    assert_eq!(rig.get("/api/profile").status, 404, "none yet");
    assert_eq!(rig.post("/api/profile", json!({"enabled": false})).status, 404, "nothing to stop");
    assert_eq!(rig.post("/api/profile", json!({"hz": 100})).status, 400, "hz without enabled");
    assert_eq!(rig.post("/api/profile", json!({"enabled": true, "hz": 1})).status, 400, "hz out of range");
    assert_eq!(rig.post("/api/profile", json!({"enabled": true, "timer": "nap"})).status, 400);
    rig.post("/api/control/run", json!({"command": "COUNT.COM"}));
    let started = rig.post("/api/profile", json!({"enabled": true, "hz": 1000, "duration_ms": 300})).json();
    assert_eq!(started["running"], true, "{}", started);
    let r = rig.get("/api/profile?wait_ms=10000&top=5").json();
    assert_eq!(r["running"], false, "stopped at its duration: {}", r["sampler"]);
    assert!(r["sampler"]["samples"].as_u64().unwrap() > 50, "{}", r["sampler"]);
    assert!(r["window"]["instructions"].as_u64().unwrap() > 0, "{}", r["window"]);
    // The loop is INC WORD [0200h] at 0100h and JMP at 0104h.
    let top: Vec<&str> = r["top"].as_array().unwrap().iter().filter_map(|p| p["addr"].as_str()).collect();
    assert!(top.iter().any(|a| a.ends_with(":0100") || a.ends_with(":0104")), "{:?}", top);
    let place = &r["top"][0];
    // (The interpreter's, or translated code's with RUST_DOS_CORE=dynamic.)
    assert_eq!(place["mode"].as_str(), Some("real"), "{}", place);
    assert!(matches!(place["engine"].as_str(), Some("cpu.interp" | "cpu.jit")), "{}", place);
    assert!(place["disasm"][0].as_str().is_some_and(|d| d.contains("inc") || d.contains("jmp")), "{}", place);
    let folded = rig.get("/api/profile?format=folded");
    assert_eq!(folded.status, 200);
    assert!(folded.header("content-type").is_some_and(|t| t.starts_with("text/plain")), "{:?}", folded.headers);
    assert!(folded.body.lines().any(|l| l.starts_with("cpu.interp;real ") || l.starts_with("cpu.jit;real ")), "{}", folded.body);
    let text = rig.get("/api/profile?format=text");
    assert!(text.body.starts_with("Profile stopped"), "{}", text.body);
    assert_eq!(rig.get("/api/profile?zone=nope").status, 400);
    assert_eq!(rig.get("/api/profile?bucket=3").status, 400);
    assert_eq!(rig.get("/api/profile?bogus=1").status, 400);
    let only = rig.get("/api/profile?zone=cpu&bucket=16").json();
    assert!(only["top"][0]["hottest"].is_string(), "{}", only["top"][0]);
    assert_eq!(rig.post("/api/profile", json!({"enabled": false})).status, 200, "a stopped profile can be stopped");
}
#[cfg(feature = "mcp")]
#[test]
fn native_mcp_enter_delivers_owned_operation_to_real_shell() {
    use rmcp::{
        model::{CallToolRequestParams, ClientConfig, ProtocolVersion},
        service::{ClientLifecycleMode, ClientServiceExt},
        transport::StreamableHttpClientTransport,
    };
    let rig = Rig::with("native-key-enter", Options { mcp: true, ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        for version in [ProtocolVersion::V_2025_11_25, ProtocolVersion::V_2026_07_28] {
            let lifecycle = if version.has_initialize() {
                ClientLifecycleMode::Initialize
            } else {
                ClientLifecycleMode::Discover { preferred_versions: vec![version.clone()] }
            };
            let client = ClientConfig::default().with_protocol_version(version.clone())
                .serve_with_lifecycle(StreamableHttpClientTransport::from_uri(format!("http://{}/mcp", rig.addr)), lifecycle)
                .await.unwrap();
            assert_eq!(client.peer_info().unwrap().protocol_version, version);
            let before_echoed = rig.get("/api/screen/text").json()["lines"]
                .as_array().unwrap().iter()
                .filter(|line| line.as_str().is_some_and(|line| line.trim() == "MCP_SHELL_OBS_01"))
                .count();
            let typed = rig.post("/api/input/type?wait=true", json!({"text":"echo MCP_SHELL_OBS_01"}));
            assert_eq!(typed.status, 200, "approved shell setup succeeds");
            let mut request = CallToolRequestParams::new("key");
            request.arguments = Some(json!({"key":"enter","hold_ms":50}).as_object().unwrap().clone());
            let response = client.call_tool(request).await;
            assert!(response.is_ok(), "native Enter must be available and reach the actual guest: {response:?}");
            let response = response.unwrap();
            assert_ne!(response.is_error, Some(true), "native Enter is accepted");
            let value = response.structured_content.unwrap();
            let operation = value["op"].as_u64().expect("authoritative input operation");
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let delivered = rig.get(&format!("/api/ops/{operation}")).json()["state"] == "done";
                let screen = rig.get("/api/screen/text").json();
                let echoed = screen["lines"].as_array().unwrap().iter()
                    .filter(|line| line.as_str().is_some_and(|line| line.trim() == "MCP_SHELL_OBS_01"))
                    .count();
                if delivered && echoed > before_echoed { break; }
                assert!(Instant::now() < deadline, "native key delivery produces real shell output");
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            assert!(rig.journal().iter().any(|entry| entry["type"] == "input_delivered" && entry["cause"] == operation));
            client.cancel().await.unwrap();
        }
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
async fn native_keyboard_client(
    rig: &Rig,
    version: rmcp::model::ProtocolVersion,
) -> rmcp::service::RunningService<rmcp::RoleClient, rmcp::model::ClientConfig> {
    use rmcp::service::{ClientLifecycleMode, ClientServiceExt};
    let lifecycle = if version.has_initialize() {
        ClientLifecycleMode::Initialize
    } else {
        ClientLifecycleMode::Discover { preferred_versions: vec![version.clone()] }
    };
    let client = rmcp::model::ClientConfig::default().with_protocol_version(version.clone())
        .serve_with_lifecycle(rmcp::transport::StreamableHttpClientTransport::from_uri(format!("http://{}/mcp", rig.addr)), lifecycle)
        .await.unwrap();
    assert_eq!(client.peer_info().unwrap().protocol_version, version);
    client
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_typing_delivers_real_shell_output_and_journal() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    let rig = Rig::with("native-type-shell", Options { mcp: true, ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        for version in [ProtocolVersion::V_2025_11_25, ProtocolVersion::V_2026_07_28] {
            let client = native_keyboard_client(&rig, version).await;
            let before_echoed = rig.get("/api/screen/text").json()["lines"].as_array().unwrap().iter()
                .filter(|line| line.as_str().is_some_and(|line| line.trim() == "MCP_SHELL_OBS_01")).count();
            let mut request = CallToolRequestParams::new("type");
            request.arguments = Some(json!({"text":"echo MCP_SHELL_OBS_01"}).as_object().unwrap().clone());
            let response = client.call_tool(request).await;
            assert!(response.is_ok(), "native typing must reach the actual guest: {response:?}");
            let response = response.unwrap();
            assert_ne!(response.is_error, Some(true), "native typing is accepted");
            let operation = response.structured_content.unwrap()["op"].as_u64().expect("authoritative typing operation");
            let entered = rig.post("/api/input/key?wait=true", json!({"key":"enter"}));
            assert_eq!(entered.status, 200);
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let delivered = rig.get(&format!("/api/ops/{operation}")).json()["state"] == "done";
                let echoed = rig.get("/api/screen/text").json()["lines"].as_array().unwrap().iter()
                    .filter(|line| line.as_str().is_some_and(|line| line.trim() == "MCP_SHELL_OBS_01")).count();
                if delivered && echoed > before_echoed { break; }
                assert!(Instant::now() < deadline, "native typing produces new real shell output");
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            assert!(rig.journal().iter().any(|entry| entry["type"] == "input_delivered" && entry["cause"] == operation));
            client.cancel().await.unwrap();
        }
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_ctrl_hold_survives_protocol_disconnect_and_renews_then_releases() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    let rig = Rig::with("native-key-hold", Options { mcp: true, ..Options::default() });
    let flags = || {
        let response = rig.get("/api/memory?addr=0040:0017&len=1&format=json");
        assert_eq!(response.status, 200, "actual guest keyboard flags are readable");
        let value = response.json();
        u8::from_str_radix(value["hex"].as_str().unwrap(), 16).unwrap()
    };
    assert_eq!(flags() & 0x04, 0, "actual guest starts without Ctrl held");
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let legacy = native_keyboard_client(&rig, ProtocolVersion::V_2025_11_25).await;
        let mut down = CallToolRequestParams::new("key");
        down.arguments = Some(json!({"action":"down","key":"ctrl","lease_ms":30000}).as_object().unwrap().clone());
        let response = legacy.call_tool(down).await;
        assert!(response.is_ok(), "persistent Ctrl down must reach the actual guest: {response:?}");
        let response = response.unwrap();
        assert_ne!(response.is_error, Some(true));
        let handle = response.structured_content.unwrap()["hold"]["handle"].as_str().expect("owned hold handle").to_owned();
        let deadline = Instant::now() + Duration::from_secs(10);
        while flags() & 0x04 == 0 {
            assert!(Instant::now() < deadline, "actual Ctrl state is delivered");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        legacy.cancel().await.unwrap();
        assert_ne!(flags() & 0x04, 0, "session loss does not release the hold");
        let current = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let mut renew = CallToolRequestParams::new("key");
        renew.arguments = Some(json!({"action":"renew","handle":handle,"lease_ms":30000}).as_object().unwrap().clone());
        let renewed = current.call_tool(renew).await.unwrap();
        assert_ne!(renewed.is_error, Some(true));
        assert_ne!(flags() & 0x04, 0, "renewal retains actual Ctrl state");
        let mut up = CallToolRequestParams::new("key");
        up.arguments = Some(json!({"action":"up","handle":handle}).as_object().unwrap().clone());
        let released = current.call_tool(up).await.unwrap();
        assert_ne!(released.is_error, Some(true));
        let deadline = Instant::now() + Duration::from_secs(10);
        while flags() & 0x04 != 0 {
            assert!(Instant::now() < deadline, "matching up releases actual Ctrl state");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        current.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
fn native_keyboard_bytes(rig: &Rig, addr: &str, len: usize) -> Vec<u8> {
    let response = rig.get(&format!("/api/memory?addr={addr}&len={len}&format=json"));
    assert_eq!(response.status, 200, "actual guest memory is readable");
    response.json()["hex"].as_str().unwrap().split_whitespace()
        .map(|byte| u8::from_str_radix(byte, 16).unwrap()).collect()
}

#[cfg(feature = "mcp")]
fn native_queued_input(rig: &Rig) -> u64 {
    let blocked = rig.post("/api/state/save", json!({"path":"pending-input.state","strict_match":true}));
    assert_eq!(blocked.status, 409, "strict save must reject pending input");
    blocked.json()["input_queued"].as_u64().expect("actual precondition reports queued input")
}

#[cfg(feature = "mcp")]
async fn native_keyboard_accepted(
    client: &rmcp::service::RunningService<rmcp::RoleClient, rmcp::model::ClientConfig>,
    tool: &str,
    arguments: Value,
) -> rmcp::model::CallToolResult {
    let mut request = rmcp::model::CallToolRequestParams::new(tool.to_owned());
    request.arguments = Some(arguments.as_object().unwrap().clone());
    let response = client.call_tool(request).await;
    assert!(response.is_ok(), "native {tool} must reach the actual guest: {response:?}");
    let response = response.unwrap();
    assert_ne!(response.is_error, Some(true), "native {tool} accepts the approved action: {response:?}");
    response
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_mouse_full_job_capacity_allows_up_and_renew_without_clearing_keys() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    let rig = Rig::with("native-mouse-full-job-control", Options { mcp: true, ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let picture = native_keyboard_accepted(&client, "capture", json!({})).await
            .structured_content.unwrap()["mouse_picture"].clone();
        let down = native_keyboard_accepted(&client, "mouse",
            json!({"action":"down","picture":picture,"button":"left","lease_ms":30000})).await.structured_content.unwrap();
        rig.until_idle();
        assert_eq!(rig.get("/api/status").json()["mouse"]["buttons"], 1);
        let tail = native_keyboard_bytes(&rig, "0040:001C", 2);
        assert_eq!(rig.post("/api/input/wait?wait=false", json!({"ms":8000})).status, 200);
        let mut operations = Vec::new();
        for _ in 0..8 {
            let queued = native_keyboard_accepted(&client, "key", json!({"key":"b"})).await.structured_content.unwrap();
            operations.push(queued["op"].as_u64().unwrap());
        }
        assert_eq!(rig.shared.native_jobs.available_permits(), 0);
        let queued = native_queued_input(&rig);
        let mut excess = CallToolRequestParams::new("key");
        excess.arguments = Some(json!({"key":"b"}).as_object().unwrap().clone());
        let refused = client.call_tool(excess).await.unwrap();
        assert_eq!(refused.structured_content.as_ref().unwrap()["error"]["code"], "capacity");
        native_keyboard_accepted(&client, "mouse", json!({"action":"renew","handle":down["hold"]["handle"],"lease_ms":30000})).await;
        native_keyboard_accepted(&client, "mouse", json!({"action":"up","handle":down["hold"]["handle"]})).await;
        assert_eq!(rig.get("/api/status").json()["mouse"]["buttons"], 0, "release is available with all eight native jobs occupied");
        assert_eq!(native_queued_input(&rig), queued, "targeted mouse release preserves unrelated queued keys");
        assert_eq!(rig.shared.native_jobs.available_permits(), 0);
        assert_eq!(native_keyboard_bytes(&rig, "0040:001C", 2), tail, "no queued key bypasses the independent wait");
        for operation in operations {
            assert_eq!(rig.get(&format!("/api/ops/{operation}")).json()["state"], "pending");
        }
        assert_eq!(http(rig.addr, "DELETE", "/api/input", &[], None).status, 200);
        assert_eq!(rig.shared.native_jobs.available_permits(), 8);
        client.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_mouse_stale_partial_drag_releases_only_its_button() {
    use rmcp::model::ProtocolVersion;
    let action = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let rig = Rig::with("native-mouse-stale-partial-drag", Options {
        mcp: true, frontend_action: Some(action.clone()), ..Options::default()
    });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let ctrl = native_keyboard_accepted(&client, "key",
            json!({"action":"down","key":"ctrl","lease_ms":30000})).await.structured_content.unwrap();
        rig.until_idle();
        let picture = native_keyboard_accepted(&client, "capture", json!({})).await
            .structured_content.unwrap()["mouse_picture"].clone();
        let dragged = native_keyboard_accepted(&client, "mouse",
            json!({"action":"drag","picture":picture,"path":[
                {"x":20,"y":20,"delay_ms":1000},{"x":40,"y":40}]})).await.structured_content.unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while rig.get("/api/status").json()["mouse"]["buttons"] != 1 {
            assert!(Instant::now() < deadline, "the actual guest receives the finite drag's down");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 4, 4);
        action.store(5, std::sync::atomic::Ordering::Release);
        loop {
            let operation = native_keyboard_accepted(&client, "operation",
                json!({"instance":dragged["instance"],"op":dragged["op"]})).await.structured_content.unwrap();
            if operation["delivery"] == "cancelled" { break; }
            assert!(Instant::now() < deadline, "mode change cancels the old finite drag");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(rig.get("/api/status").json()["mouse"]["buttons"], 0,
            "stale finite drag cleanup releases its delivered button");
        assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 4, 4,
            "targeted drag cleanup preserves an independent modifier");
        native_keyboard_accepted(&client, "key", json!({"action":"up","handle":ctrl["hold"]["handle"]})).await;
        rig.until_idle();
        let current = native_keyboard_accepted(&client, "capture", json!({})).await
            .structured_content.unwrap()["mouse_picture"].clone();
        native_keyboard_accepted(&client, "mouse", json!({"action":"click","picture":current,"hold_ms":20})).await;
        client.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_mouse_invalid_requests_have_no_queued_or_guest_prefix() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    let rig = Rig::with("native-mouse-invalid-atomic", Options { mcp: true, ..Options::default() },
    );
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        for version in [ProtocolVersion::V_2025_11_25, ProtocolVersion::V_2026_07_28] {
            let client = native_keyboard_client(&rig, version).await;
            let picture = native_keyboard_accepted(&client, "capture", json!({})).await
                .structured_content.unwrap()["mouse_picture"].clone();
            let before = rig.get("/api/status").json()["mouse"].clone();
            let queued = rig.journal().iter().filter(|entry| entry["type"] == "input_queued").count();
            for args in [
                json!({"action":"move","x":-1,"y":0}), json!({"action":"move","x":picture["width"], "y":0}),
                json!({"action":"move","x":0,"y":picture["height"]}), json!({"action":"move","dx":32768,"dy":0}), json!({"action":"move","dx":0,"dy":-32768}), json!({"action":"move","x":0,"y":0,"dx":1,"dy":1}), json!({"action":"click","button":"wheel"}), json!({"action":"down","lease_ms":30001}), json!({"action":"drag","path":[]}),
                json!({"action":"drag", "path":vec![json!({"x":0,"y":0});257]}),
                json!({"action":"drag", "path":[{"x":0,"y":0},{"dx":1,"dy":1}]}),
                json!({"action":"drag","path":[{"x":0, "y":0},{"x":-1,"y":0}]}),
                json!({"action":"drag","path":[{"x":0,"y":0,"delay_ms":3000},{"x":1,"y":1,"delay_ms":2001}]}),
            ] {
                let mut args = args;
                args["picture"] = picture.clone();
                let mut request = CallToolRequestParams::new("mouse");
                request.arguments = Some(args.as_object().unwrap().clone());
                match client.call_tool(request).await {
                    Ok(refused) => {
                        assert_eq!(refused.is_error, Some(true), "invalid mouse action rejects before acceptance");
                        assert_eq!(refused.structured_content.as_ref().unwrap()["error"]["code"],"invalid_input");
                    }
                    Err(rmcp::ServiceError::McpError(error)) => assert_eq!(error.code, rmcp::model::ErrorCode::INVALID_PARAMS),
                    Err(error) => panic!("invalid input must not fail transport: {error}"),
                }
                assert_eq!(rig.journal().iter().filter(|entry| entry["type"] == "input_queued").count(), queued, "the actor rejects the entire gesture before queuing its valid prefix");
                assert_eq!(rig.get("/api/status").json()["mouse"], before,"a rejected gesture neither moves the actual pointer nor presses a button");
        }
            client.cancel().await.unwrap();
    }
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_output_schemas_validate_real_guest_results_and_errors() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    let rig = Rig::with("native-output-schemas", Options { mcp: true, ..Options::default() },
    );
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        for version in [ProtocolVersion::V_2025_11_25, ProtocolVersion::V_2026_07_28] {
            let client = native_keyboard_client(&rig, version).await;
        let validators = client.list_all_tools().await.unwrap().into_iter().map(|tool| {
                let schema = tool.output_schema.expect("every native tool publishes its structured result contract");
                let validator = jsonschema::validator_for(&Value::Object((*schema).clone())).unwrap();
                (tool.name.to_string(), validator)
            }).collect::<std::collections::HashMap<_, _>>();
        let capture = native_keyboard_accepted(&client, "capture", json!({})).await;
        let picture = capture.structured_content.as_ref().unwrap()["mouse_picture"].clone();
            validators["capture"].validate(capture.structured_content.as_ref().unwrap()).unwrap();
            let status = native_keyboard_accepted(&client, "status", json!({})).await;
            validators["status"].validate(status.structured_content.as_ref().unwrap()).unwrap();
            let mut wrong_dimensions = capture.structured_content.clone().unwrap();
            wrong_dimensions["width"] = "640".into();
            assert!(!validators["capture"].is_valid(&wrong_dimensions), "the schema rejects a typed mutation of an actual capture");
            for (tool, args) in [
                ("key", json!({"action":"press","key":"b", "hold_ms":20})),
                ("type", json!({"text":"b","delay_ms":0})),
                ("mouse", json!({"action":"click","picture":picture,"hold_ms":20})),
            ] {
                let accepted = native_keyboard_accepted(&client, tool, args).await;
                let value = accepted.structured_content.as_ref().unwrap();
                validators[tool].validate(value).unwrap();
                let operation = native_keyboard_accepted(&client, "operation", json!({"instance":value["instance"],"op":value["op"]})).await;
                validators["operation"].validate(operation.structured_content.as_ref().unwrap()).unwrap();
                rig.until_idle();
            }
            for (tool, args) in [
                ("mouse", json!({"action":"move","picture":{"instance":picture["instance"],"mode_epoch":picture["mode_epoch"].as_u64().unwrap()+1,"width":picture["width"],"height":picture["height"]},"x":0,"y":0})),
                ("operation", json!({"instance":picture["instance"],"op":u64::MAX})),
            ] {
            let mut request = CallToolRequestParams::new(tool);
            request.arguments = Some(args.as_object().unwrap().clone());
            let refused = client.call_tool(request).await.unwrap();
            assert_eq!(refused.is_error, Some(true));
                let value = refused.structured_content.as_ref().expect("execution errors conform to the advertised output schema");
                validators[tool].validate(value).unwrap();
                assert!(refused.content.iter().filter_map(|item| item.as_text()).any(|text| serde_json::from_str::<Value>(&text.text).ok().as_ref() == Some(value)), "structured failures also include their serialized JSON text");
            }
            client.cancel().await.unwrap();
    }
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_mouse_restore_detaches_old_authority_without_button_up() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    for path in ["ordinary", "strict", "frontend"] {
        let action = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let rig = Rig::with(&format!("native-mouse-restore-{path}"), Options {
            mcp: true, frontend_action: Some(action.clone()), ..Options::default()
        });
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        runtime.block_on(async {
            let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
            let picture = native_keyboard_accepted(&client, "capture", json!({})).await
                .structured_content.unwrap()["mouse_picture"].clone();
            let held = native_keyboard_accepted(&client, "mouse",
                json!({"action":"down","picture":picture,"button":"left","lease_ms":2000})).await
                .structured_content.unwrap();
            rig.until_idle();
            assert_eq!(rig.get("/api/status").json()["mouse"]["buttons"], 1, "the snapshot contains the actual held button");
            let before_reset = rig.journal().iter().filter(|entry| entry["type"] == "state_loaded").count();
            if path == "frontend" {
                action.store(2, std::sync::atomic::Ordering::Release);
                let deadline = Instant::now() + Duration::from_secs(10);
                while rig.journal().iter().filter(|entry| entry["type"] == "state_loaded").count() == before_reset {
                    assert!(Instant::now() < deadline, "actual frontend restore hook completes");
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            } else {
                assert_eq!(rig.post("/api/control/pause", json!({})).status, 200);
                rig.until_paused();
                let args = json!({"path":"mouse-held.state","strict_match":path == "strict"});
                let saved = rig.post("/api/state/save", args.clone());
                assert_eq!(saved.status, 200, "{}", saved.body);
                let loaded = rig.post("/api/state/load", args);
                assert_eq!(loaded.status, 200, "{}", loaded.body);
            }
            for action in ["up", "renew"] {
                let mut args = json!({"action":action,"handle":held["hold"]["handle"]});
                if action == "renew" { args["lease_ms"] = 30000.into(); }
                let mut request = CallToolRequestParams::new("mouse");
                request.arguments = Some(args.as_object().unwrap().clone());
                let refused = client.call_tool(request).await.unwrap();
                assert_eq!(refused.is_error, Some(true), "restoration invalidates old mouse authority");
                assert_eq!(rig.get("/api/status").json()["mouse"]["buttons"], 1, "old control sends no release into the restored guest");
            }
            tokio::time::sleep(Duration::from_millis(2250)).await;
            assert_eq!(rig.get("/api/status").json()["mouse"]["buttons"], 1, "old lease expiry sends no release into restored mouse state");
            let current = native_keyboard_accepted(&client, "capture", json!({})).await
                .structured_content.unwrap()["mouse_picture"].clone();
            assert_ne!((&current["instance"], &current["mode_epoch"]), (&picture["instance"], &picture["mode_epoch"]));
            assert_eq!(rig.post("/api/input/mouse", json!({"action":"up","button":"left"})).status, 200);
            client.cancel().await.unwrap();
        });
        rig.until_idle();
    }
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_mouse_restore_cancels_queued_drag_and_preserves_saved_buttons() {
    use rmcp::model::ProtocolVersion;
    for strict in [false, true] {
        let rig = Rig::with(&format!("native-drag-restore-{strict}"), Options {
            mcp: true, ..Options::default()
        });
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        runtime.block_on(async {
            let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
            assert_eq!(rig.post("/api/input/mouse", json!({"action":"down","button":"left"})).status, 200);
            rig.until_idle();
            assert_eq!(rig.get("/api/status").json()["mouse"]["buttons"], 1);
            assert_eq!(rig.post("/api/control/pause", json!({})).status, 200);
            rig.until_paused();
            let state = json!({"path":"queued-drag.state","strict_match":strict});
            assert_eq!(rig.post("/api/state/save", state.clone()).status, 200);
            assert_eq!(rig.post("/api/control/resume", json!({})).status, 200);
            assert_eq!(rig.post("/api/input/mouse", json!({"action":"up","button":"left"})).status, 200);
            rig.until_idle();
            assert_eq!(rig.get("/api/status").json()["mouse"]["buttons"], 0);
            assert_eq!(rig.post("/api/control/pause", json!({})).status, 200);
            rig.until_paused();
            let picture = native_keyboard_accepted(&client, "capture", json!({})).await
                .structured_content.unwrap()["mouse_picture"].clone();
            assert_eq!(rig.post("/api/input/wait?wait=false", json!({"ms":1000})).status, 200);
            let drag = native_keyboard_accepted(&client, "mouse", json!({
                "action":"drag","picture":picture,"button":"left",
                "path":[{"x":3,"y":4,"delay_ms":1},{"x":20,"y":20,"delay_ms":1}]
            })).await.structured_content.unwrap();
            let request = drag["op"].as_u64().unwrap();
            assert_eq!(rig.get(&format!("/api/ops/{request}")).json()["state"], "pending");
            assert_eq!(rig.get("/api/status").json()["mouse"]["buttons"], 0);
            assert_eq!(rig.post("/api/state/load", state).status, 200);
            assert_eq!(rig.get(&format!("/api/ops/{request}")).json()["state"], "cancelled", "restore cancels the queued finite drag");
            let restored = rig.get("/api/status").json()["mouse"].clone();
            assert_eq!(restored["buttons"], 1, "obsolete drag cleanup cannot release the restored button");
            assert_eq!(rig.shared.native_jobs.available_permits(), 8);
            assert_eq!(rig.post("/api/control/resume", json!({})).status, 200);
            tokio::time::sleep(Duration::from_millis(1150)).await;
            rig.until_idle();
            assert_eq!(rig.get("/api/status").json()["mouse"], restored, "obsolete drag points and releases never execute in the restored guest");
            client.cancel().await.unwrap();
        });
    }
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_mouse_and_key_handles_remain_device_specific_after_release() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    let rig = Rig::with("native-mouse-device-handle", Options { mcp: true, ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let picture = native_keyboard_accepted(&client, "capture", json!({})).await
            .structured_content.unwrap()["mouse_picture"].clone();
        for (owner, other, args) in [
            ("mouse", "key", json!({"action":"down","picture":picture,"button":"left","lease_ms":30000})),
            ("key", "mouse", json!({"action":"down","key":"ctrl","lease_ms":30000})),
        ] {
            let held = native_keyboard_accepted(&client, owner, args).await.structured_content.unwrap();
            rig.until_idle();
            let before_mouse = rig.get("/api/status").json()["mouse"]["buttons"].clone();
            let before_keys = native_keyboard_bytes(&rig, "0040:0017", 1);
            for ended in [false, true] {
                for action in ["up", "renew"] {
                    let mut args = json!({"action":action,"handle":held["hold"]["handle"]});
                    if action == "renew" { args["lease_ms"] = 30000.into(); }
                    let mut request = CallToolRequestParams::new(other);
                    request.arguments = Some(args.as_object().unwrap().clone());
                    let refused = client.call_tool(request).await.unwrap();
                    assert_eq!(refused.is_error, Some(true), "a {owner} handle cannot authorize {other}, even after release");
                    assert_eq!(refused.structured_content.as_ref().unwrap()["error"]["code"], "invalid_input");
                    if !ended {
                        assert_eq!(rig.get("/api/status").json()["mouse"]["buttons"], before_mouse);
                        assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1), before_keys);
                    }
                }
                native_keyboard_accepted(&client, owner, json!({"action":"up","handle":held["hold"]["handle"]})).await;
                rig.until_idle();
            }
        }
        client.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_mouse_expiry_preserves_an_independent_modifier_hold() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    let rig = Rig::with("native-mouse-key-expiry", Options { mcp: true, ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2025_11_25).await;
        let picture = native_keyboard_accepted(&client, "capture", json!({})).await
            .structured_content.unwrap()["mouse_picture"].clone();
        let down = native_keyboard_accepted(&client, "mouse",
            json!({"action":"down","picture":picture,"button":"left","lease_ms":750})).await
            .structured_content.unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while rig.get("/api/status").json()["mouse"]["buttons"] == 0 {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let mut request = CallToolRequestParams::new("key");
        request.arguments = Some(json!({"action":"down","key":"ctrl","lease_ms":30000}).as_object().unwrap().clone());
        let ctrl = client.call_tool(request).await.unwrap();
        assert_ne!(ctrl.is_error, Some(true), "a mouse hold must not reserve keyboard modifier state");
        let ctrl = ctrl.structured_content.unwrap();
        while native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 4 == 0 {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        while rig.get("/api/status").json()["mouse"]["buttons"] != 0 {
            assert!(Instant::now() < deadline, "only the leased mouse button expires");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 4, 4,
            "mouse expiry preserves the actual guest's independently held Ctrl");
        let ended = native_keyboard_accepted(&client, "operation",
            json!({"instance":down["instance"],"op":down["op"]})).await.structured_content.unwrap();
        assert_eq!(ended["hold"]["state"], "expired");
        native_keyboard_accepted(&client, "key", json!({"action":"up","handle":ctrl["hold"]["handle"]})).await;
        client.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_mouse_mode_change_retires_a_reserved_button_claim() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    let action = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let rig = Rig::with(
        "native-mouse-stale-reserved-claim",
        Options {
            mcp: true,
            deterministic: Some(10),
            frontend_action: Some(action.clone()),
            ..Options::default()
        },
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let picture = native_keyboard_accepted(&client, "capture", json!({}))
            .await
            .structured_content
            .unwrap()["mouse_picture"]
            .clone();
        let accepted = native_keyboard_accepted(
            &client,
            "mouse",
            json!({"action":"down","picture":picture,"lease_ms":30000}),
        )
        .await
        .structured_content
        .unwrap();
        assert_eq!(accepted["held"], true);
        assert_eq!(rig.get("/api/status").json()["mouse"]["buttons"], 0);
        action.store(5, std::sync::atomic::Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let operation = native_keyboard_accepted(
                &client,
                "operation",
                json!({"instance":accepted["instance"],"op":accepted["op"]}),
            )
            .await
            .structured_content
            .unwrap();
            if operation["hold"]["state"] == "reset" {
                assert_eq!(operation["delivery"], "cancelled");
                break;
            }
            assert!(
                Instant::now() < deadline,
                "mode replacement retires a queued claim before any down"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(rig.shared.native_jobs.available_permits(), 8);
        assert_eq!(rig.get("/api/status").json()["mouse"]["buttons"], 0);
        let mut request = CallToolRequestParams::new("mouse");
        request.arguments = Some(
            json!({"action":"up","handle":accepted["hold"]["handle"]})
                .as_object()
                .unwrap()
                .clone(),
        );
        assert_eq!(
            client.call_tool(request).await.unwrap().is_error,
            Some(true)
        );
        client.cancel().await.unwrap();
    });
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_mouse_mode_change_cancels_queued_picture_before_delivery() {
    use rmcp::model::ProtocolVersion;
    let action = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let rig = Rig::with("native-mouse-stale-queue", Options {
        mcp: true, deterministic: Some(10), frontend_action: Some(action.clone()), ..Options::default()
    });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let picture = native_keyboard_accepted(&client, "capture", json!({})).await
            .structured_content.unwrap()["mouse_picture"].clone();
        let accepted = native_keyboard_accepted(&client, "mouse",
            json!({"action":"click","picture":picture,"x":100,"y":80,"hold_ms":1000})).await
            .structured_content.unwrap();
        assert_eq!(accepted["held"], true, "the actual deterministic guest retains queued input");
        action.store(5, std::sync::atomic::Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let state = native_keyboard_accepted(&client, "operation",
                json!({"instance":accepted["instance"],"op":accepted["op"]})).await
                .structured_content.unwrap();
            if state["delivery"] == "cancelled" { break; }
            assert!(Instant::now() < deadline, "a BIOS mode change must cancel stale queued picture input before delivery");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let current = native_keyboard_accepted(&client, "capture", json!({})).await
            .structured_content.unwrap()["mouse_picture"].clone();
        assert!(current["mode_epoch"].as_u64().unwrap() > picture["mode_epoch"].as_u64().unwrap());
        assert_eq!(rig.get("/api/status").json()["mouse"]["buttons"], 0);
        client.cancel().await.unwrap();
    });
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_mouse_rejects_null_path_fields_before_acceptance() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    let rig = Rig::with("native-mouse-null-path", Options { mcp: true, ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2025_11_25).await;
        let picture = native_keyboard_accepted(&client, "capture", json!({})).await
            .structured_content.unwrap()["mouse_picture"].clone();
        let queued = rig.journal().iter().filter(|entry| entry["type"] == "input_queued").count();
        let mut request = CallToolRequestParams::new("mouse");
        request.arguments = Some(json!({"action":"drag","picture":picture,
            "path":[{"x":40,"y":40,"delay_ms":null}]}).as_object().unwrap().clone());
        assert!(client.call_tool(request).await.is_err(), "explicit null must not become a default delay");
        assert_eq!(rig.journal().iter().filter(|entry| entry["type"] == "input_queued").count(), queued);
        client.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_mouse_stale_picture_reports_recapture_without_queue_effects() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    let rig = Rig::with("native-mouse-stale-picture", Options { mcp: true, ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2025_11_25).await;
        let mut picture = native_keyboard_accepted(&client, "capture", json!({})).await
            .structured_content.unwrap()["mouse_picture"].clone();
        picture["mode_epoch"] = (picture["mode_epoch"].as_u64().unwrap() + 1).into();
        let before = rig.get("/api/status").json()["mouse"].clone();
        let queued = rig.journal().iter().filter(|entry| entry["type"] == "input_queued").count();
        let mut request = CallToolRequestParams::new("mouse");
        request.arguments = Some(json!({"action":"move","picture":picture,"x":100,"y":80}).as_object().unwrap().clone());
        let rejected = client.call_tool(request).await.unwrap();
        assert_eq!(rejected.is_error, Some(true));
        let failure = rejected.structured_content.expect("tool failures are actionable structured results");
        assert_eq!(failure["error"]["code"], "stale_picture");
        assert_eq!(failure["error"]["recovery"], "capture_again");
        assert_eq!(rig.journal().iter().filter(|entry| entry["type"] == "input_queued").count(), queued);
        assert_eq!(rig.get("/api/status").json()["mouse"], before);
        client.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_mouse_shutdown_releases_only_native_buttons_before_immutable_shutdown() {
    use rmcp::model::ProtocolVersion;
    let buttons = Arc::new(std::sync::atomic::AtomicUsize::new(usize::MAX));
    let mut rig = Rig::with("native-mouse-mutable-shutdown", Options {
        mcp: true, shutdown_mouse: Some(buttons.clone()), ..Options::default()
    });
    assert_eq!(rig.post("/api/input/mouse", json!({"action":"down","button":"right"})).status, 200);
    rig.until_idle();
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let picture = native_keyboard_accepted(&client, "capture", json!({})).await.structured_content.unwrap()["mouse_picture"].clone();
        native_keyboard_accepted(&client, "mouse", json!({"action":"down","picture":picture,"button":"left","lease_ms":30000})).await;
        rig.until_idle();
        assert_eq!(rig.get("/api/status").json()["mouse"]["buttons"], 3,
            "both independent button states reach the actual guest");
        client.cancel().await.unwrap();
    });
    assert_eq!(rig.quit().status, 200);
    let observed = buttons.load(std::sync::atomic::Ordering::Acquire);
    assert_ne!(observed, usize::MAX, "observe the actual mutable guest before shutdown");
    assert_eq!(observed, 2, "shutdown releases the native left button and preserves the unrelated legacy right button");
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_mouse_delivered_hold_survives_mode_change_and_requires_recapture() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    let action = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let rig = Rig::with("native-mouse-held-mode-change", Options {
        mcp: true, frontend_action: Some(action.clone()), ..Options::default()
    });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let picture = native_keyboard_accepted(&client, "capture", json!({})).await.structured_content.unwrap()["mouse_picture"].clone();
        let held = native_keyboard_accepted(&client, "mouse",
            json!({"action":"down","picture":picture,"lease_ms":30000})).await.structured_content.unwrap();
        rig.until_idle();
        assert_eq!(rig.get("/api/status").json()["mouse"]["buttons"], 1);
        action.store(5, std::sync::atomic::Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(5);
        let current = loop {
            let capture = native_keyboard_accepted(&client, "capture", json!({})).await.structured_content.unwrap()["mouse_picture"].clone();
            if capture["mode_epoch"] != picture["mode_epoch"] { break capture; }
            assert!(Instant::now() < deadline, "BIOS mode change is observed before recapture");
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        let mut stale = CallToolRequestParams::new("mouse");
        stale.arguments = Some(json!({"action":"move","picture":picture,"handle":held["hold"]["handle"],"x":100,"y":80}).as_object().unwrap().clone());
        let refused = client.call_tool(stale).await.unwrap();
        assert_eq!(refused.structured_content.as_ref().unwrap()["error"]["code"], "stale_picture");
        assert_eq!(rig.get("/api/status").json()["mouse"]["buttons"], 1,
            "mode change preserves an already delivered button on the same guest");
        native_keyboard_accepted(&client, "mouse", json!({"action":"renew","handle":held["hold"]["handle"],"lease_ms":30000})).await;
        native_keyboard_accepted(&client, "mouse", json!({"action":"move","picture":current,"handle":held["hold"]["handle"],"x":100,"y":80})).await;
        rig.until_idle();
        assert_eq!(rig.get("/api/status").json()["mouse"]["buttons"], 1);
        native_keyboard_accepted(&client, "mouse", json!({"action":"up","handle":held["hold"]["handle"]})).await;
        assert_eq!(rig.get("/api/status").json()["mouse"]["buttons"], 0);
        client.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_mouse_vbe_mode_identity_invalidates_same_geometry_picture() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    let action = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let rig = Rig::with("native-mouse-vbe-mode-identity", Options {
        mcp: true, deterministic: Some(10), frontend_action: Some(action.clone()), ..Options::default()
    });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        action.store(6, std::sync::atomic::Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(5);
        let first = loop {
            let capture = native_keyboard_accepted(&client, "capture", json!({})).await.structured_content.unwrap();
            if capture["width"] == 640 && capture["height"] == 480 { break capture["mouse_picture"].clone(); }
            assert!(Instant::now() < deadline, "the actual BIOS enters 640x480 VBE mode");
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        let queued = native_keyboard_accepted(&client, "mouse",
            json!({"action":"move","picture":first,"x":100,"y":80})).await.structured_content.unwrap();
        action.store(7, std::sync::atomic::Ordering::Release);
        while action.load(std::sync::atomic::Ordering::Acquire) != 0 {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let next = native_keyboard_accepted(&client, "capture", json!({})).await.structured_content.unwrap()["mouse_picture"].clone();
        assert_eq!((&first["width"], &first["height"]), (&next["width"], &next["height"]));
        assert!(next["mode_epoch"].as_u64().unwrap() > first["mode_epoch"].as_u64().unwrap(),
            "different VBE mode identities invalidate a picture even at identical geometry");
        let operation = native_keyboard_accepted(&client, "operation",
            json!({"instance":queued["instance"],"op":queued["op"]})).await.structured_content.unwrap();
        assert_eq!(operation["delivery"], "cancelled");
        let mut stale = CallToolRequestParams::new("mouse");
        stale.arguments = Some(json!({"action":"move","picture":first,"x":100,"y":80}).as_object().unwrap().clone());
        let rejected = client.call_tool(stale).await.unwrap();
        assert_eq!(rejected.structured_content.as_ref().unwrap()["error"]["code"], "stale_picture");
        client.cancel().await.unwrap();
    });
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_mouse_maximum_path_admission_and_storage_are_bounded() {
    use rmcp::model::ProtocolVersion;
    let capacity = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let rig = Rig::with("native-mouse-maximum-path", Options {
        mcp: true, deterministic: Some(10), observed_input_capacity: Some(capacity.clone()), ..Options::default()
    });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        for version in [ProtocolVersion::V_2025_11_25, ProtocolVersion::V_2026_07_28] {
            let client = native_keyboard_client(&rig, version.clone()).await;
            let picture = native_keyboard_accepted(&client, "capture", json!({})).await
                .structured_content.unwrap()["mouse_picture"].clone();
            let path = (0..256).map(|index| json!({"x":20 + index % 2,"y":20,
                "delay_ms":if index == 255 {155} else {19}})).collect::<Vec<_>>();
            let mut samples = Vec::new();
            let mut retained_capacity = None;
            for _ in 0..32 {
                let started = Instant::now();
                let accepted = native_keyboard_accepted(&client, "mouse",
                    json!({"action":"drag","picture":picture,"path":path})).await.structured_content.unwrap();
                samples.push(started.elapsed().as_micros());
                assert_eq!(accepted["held"], true);
                assert_eq!(native_queued_input(&rig), 514, "256 motions, 256 waits and two button events; the API excludes the end marker");
                assert_eq!(rig.shared.native_jobs.available_permits(), 7);
                let deadline = Instant::now() + Duration::from_secs(2);
                while capacity.load(std::sync::atomic::Ordering::Acquire) < 515 {
                    assert!(Instant::now() < deadline);
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
                let allocated = capacity.load(std::sync::atomic::Ordering::Acquire);
                assert_eq!(allocated, *retained_capacity.get_or_insert(allocated), "repeated maximum paths do not grow the retained queue");
                assert_eq!(http(rig.addr, "DELETE", "/api/input", &[], None).status, 200);
                assert_eq!(rig.shared.native_jobs.available_permits(), 8);
                assert_eq!(rig.get(&format!("/api/ops/{}", accepted["op"].as_u64().unwrap())).json()["state"], "cancelled");
                assert_eq!(rig.get("/api/status").json()["mouse"]["buttons"], 0);
            }
            samples.sort_unstable();
            let allocated = capacity.load(std::sync::atomic::Ordering::Acquire);
            println!("mouse path measured: protocol={version}, points=256, total_delay_ms=5000, samples=32, events=514, end_markers=1, queue_capacity={allocated}, queue_storage_bytes={}, admission_median_us={}, admission_max_us={}",
                allocated * std::mem::size_of::<super::super::LowInput>(), samples[16], samples[31]);
            client.cancel().await.unwrap();
        }
    });
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_mouse_maximum_path_plays_once_after_backlog_and_disconnect() {
    use rmcp::model::ProtocolVersion;
    let rig = Rig::with("native-mouse-maximum-playback", Options { mcp: true, ..Options::default() });
    assert_eq!(rig.post("/api/input/mouse", json!({"action":"move","x":275,"y":20,"coords":"screen"})).status, 200);
    rig.until_idle();
    let expected = rig.get("/api/status").json()["mouse"].clone();
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        for version in [ProtocolVersion::V_2025_11_25, ProtocolVersion::V_2026_07_28] {
            assert_eq!(rig.post("/api/input/mouse", json!({"action":"move","x":20,"y":20,"coords":"screen"})).status, 200);
            rig.until_idle();
            let producer = native_keyboard_client(&rig, version.clone()).await;
            let observer = native_keyboard_client(&rig, version).await;
            let picture = native_keyboard_accepted(&producer, "capture", json!({})).await
                .structured_content.unwrap()["mouse_picture"].clone();
            let backlog = rig.post("/api/input/wait?wait=false", json!({"ms":1000})).json()["op"].as_u64().unwrap();
            let path = (0..256).map(|index| json!({"x":20 + index,"y":20,
                "delay_ms":if index == 255 {155} else {19}})).collect::<Vec<_>>();
            let started = Instant::now();
            let drag = native_keyboard_accepted(&producer, "mouse",
                json!({"action":"drag","picture":picture,"path":path})).await.structured_content.unwrap();
            let trailing = native_keyboard_accepted(&observer, "key", json!({"key":"b"})).await
                .structured_content.unwrap();
            assert_eq!(rig.get("/api/status").json()["mouse"]["buttons"], 0,
                "the existing HTTP wait keeps the drag queued before its first button event");
            producer.cancel().await.unwrap();
            let deadline = Instant::now() + Duration::from_secs(15);
            let mut positions = std::collections::BTreeSet::new();
            let mut last_x = 20;
            loop {
                let operation = native_keyboard_accepted(&observer, "operation",
                    json!({"instance":drag["instance"],"op":drag["op"]})).await.structured_content.unwrap();
                let mouse = rig.get("/api/status").json()["mouse"].clone();
                let x = mouse["x"].as_i64().unwrap();
                assert!(x >= last_x && x <= expected["x"].as_i64().unwrap(), "ordered motion cannot jump backwards or beyond the path");
                last_x = x;
                if mouse["buttons"] == 1 { positions.insert(x); }
                if operation["delivery"] == "delivered" { break; }
                assert_ne!(operation["delivery"], "cancelled", "disconnect must not replay or cancel accepted input");
                assert!(Instant::now() < deadline, "the maximum admitted drag finishes through the real actor");
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            assert!(started.elapsed() >= Duration::from_secs(5), "the admitted path retains its five seconds of waits");
            assert!(positions.len() > 2, "actual guest motion is observed with the button held across the path");
            let mouse = rig.get("/api/status").json()["mouse"].clone();
            assert_eq!((&mouse["x"], &mouse["y"], &mouse["buttons"]), (&expected["x"], &expected["y"], &json!(0)));
            while rig.get(&format!("/api/ops/{}", trailing["op"].as_u64().unwrap())).json()["state"] != "done" {
                assert!(Instant::now() < deadline, "the other client's trailing keyboard operation survives the drag");
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            for op in [backlog, drag["op"].as_u64().unwrap(), trailing["op"].as_u64().unwrap()] {
                assert_eq!(rig.journal().iter().filter(|entry| entry["type"] == "input_delivered" && entry["cause"] == op).count(), 1,
                    "each accepted operation delivers exactly once");
            }
            rig.until_idle();
            assert_eq!(rig.shared.native_jobs.available_permits(), 8);
            observer.cancel().await.unwrap();
        }
    });
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_mouse_input_schema_expresses_action_and_coordinate_requirements() {
    use rmcp::model::ProtocolVersion;
    let rig = Rig::with("native-mouse-input-schema", Options { mcp: true, ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        for version in [ProtocolVersion::V_2025_11_25, ProtocolVersion::V_2026_07_28] {
            let client = native_keyboard_client(&rig, version).await;
            let tool = client.list_all_tools().await.unwrap().into_iter().find(|tool| tool.name == "mouse").unwrap();
            println!("native mouse schema={}", Value::Object((*tool.input_schema).clone()));
            let frozen: Value = serde_json::from_str(include_str!("../../../tests/fixtures/mcp/mouse.schema.json")).unwrap();
            assert_eq!(Value::Object((*tool.input_schema).clone()), frozen, "native discovery preserves the frozen mouse input contract");
            let validator = jsonschema::validator_for(&Value::Object((*tool.input_schema).clone())).unwrap();
            let picture = native_keyboard_accepted(&client, "capture", json!({})).await
                .structured_content.unwrap()["mouse_picture"].clone();
            let held = native_keyboard_accepted(&client, "mouse",
                json!({"action":"down","picture":picture,"lease_ms":30000})).await.structured_content.unwrap();
            let handle = held["hold"]["handle"].clone();
            for valid in [
                json!({"picture":picture,"dx":1,"dy":0}),
                json!({"action":"move","picture":picture,"x":1,"y":1,"handle":handle}),
                json!({"action":"down","picture":picture,"lease_ms":30000}),
                json!({"action":"click","picture":picture,"hold_ms":20}),
                json!({"action":"up","handle":handle}),
                json!({"action":"renew","handle":handle,"lease_ms":1000}),
                json!({"action":"drag","picture":picture,"path":[{"x":1,"y":1},{"x":2,"y":2}]}),
                json!({"action":"drag","picture":picture,"path":[{"dx":1,"dy":0},{"dx":1,"dy":1}]}),
            ] {
                assert!(validator.is_valid(&valid), "the advertised schema must accept a supported action: {valid}");
            }
            for invalid in [
                json!({"action":"down","picture":picture}),
                json!({"action":"move","x":1,"y":1}),
                json!({"action":"move","picture":picture}),
                json!({"action":"move","picture":picture,"x":1,"y":1,"lease_ms":1000}),
                json!({"action":"move","picture":picture,"x":1,"y":1,"dx":1,"dy":1}),
                json!({"action":"click","picture":picture,"handle":handle}),
                json!({"action":"up","handle":handle,"picture":picture}),
                json!({"action":"renew","handle":handle}),
                json!({"action":"drag","picture":picture}),
                json!({"action":"drag","picture":picture,"path":[{"x":1,"y":1},{"dx":1,"dy":1}]}),
                json!({"action":"drag","picture":picture,"path":[{"x":1,"y":1,"dx":1}]}),
            ] {
                assert!(!validator.is_valid(&invalid), "the advertised schema must reject an unsupported action form: {invalid}");
            }
            native_keyboard_accepted(&client, "mouse", json!({"action":"up","handle":handle})).await;
            client.cancel().await.unwrap();
        }
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_mouse_rejects_motion_during_an_http_button_hold() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    let rig = Rig::with("native-mouse-legacy-pointer-conflict", Options { mcp: true, ..Options::default() });
    assert_eq!(rig.post("/api/input/mouse", json!({"action":"down","button":"left"})).status, 200);
    rig.until_idle();
    let before = rig.get("/api/status").json()["mouse"].clone();
    assert_eq!(before["buttons"], 1, "HTTP owns an actual delivered button");
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        for version in [ProtocolVersion::V_2025_11_25, ProtocolVersion::V_2026_07_28] {
            let client = native_keyboard_client(&rig, version).await;
            let picture = native_keyboard_accepted(&client, "capture", json!({})).await
                .structured_content.unwrap()["mouse_picture"].clone();
            let queued = rig.journal().iter().filter(|entry| entry["type"] == "input_queued").count();
            for motion in [json!({"x":100,"y":80}), json!({"dx":5,"dy":-3})] {
                let mut args = motion;
                args["action"] = "move".into();
                args["picture"] = picture.clone();
                let mut request = CallToolRequestParams::new("mouse");
                request.arguments = Some(args.as_object().unwrap().clone());
                let refused = client.call_tool(request).await.unwrap();
                assert_eq!(refused.is_error, Some(true), "native movement must not take over an HTTP-owned drag");
                assert_eq!(refused.structured_content.as_ref().unwrap()["error"]["code"], "input_conflict");
                assert_eq!(rig.journal().iter().filter(|entry| entry["type"] == "input_queued").count(), queued);
                assert_eq!(rig.get("/api/status").json()["mouse"], before);
            }
            client.cancel().await.unwrap();
        }
    });
    assert_eq!(rig.post("/api/input/mouse", json!({"action":"up","button":"left"})).status, 200);
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_mouse_hold_rejects_unowned_http_pointer_events() {
    use rmcp::model::ProtocolVersion;
    let rig = Rig::with("native-mouse-http-conflict", Options { mcp: true, ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2025_11_25).await;
        let picture = native_keyboard_accepted(&client, "capture", json!({})).await
            .structured_content.unwrap()["mouse_picture"].clone();
        let down = native_keyboard_accepted(&client, "mouse",
            json!({"action":"down","picture":picture,"button":"left","lease_ms":30000})).await;
        let deadline = Instant::now() + Duration::from_secs(10);
        while rig.get("/api/status").json()["mouse"]["buttons"].as_u64().unwrap() & 1 == 0 {
            assert!(Instant::now() < deadline, "owned button reaches the actual guest");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let queued = rig.journal().iter().filter(|entry| entry["type"] == "input_queued").count();
        for input in [
            json!({"action":"move","x":20,"y":20,"coords":"screen"}),
            json!({"action":"up","button":"left"}),
            json!({"action":"click","button":"left","hold_ms":20}),
        ] {
            assert_eq!(rig.post("/api/input/mouse", input).status, 409,
                "HTTP must not move or release the owned native pointer");
            assert_eq!(rig.journal().iter().filter(|entry| entry["type"] == "input_queued").count(), queued,
                "rejected HTTP input has no queued prefix");
            assert_ne!(rig.get("/api/status").json()["mouse"]["buttons"].as_u64().unwrap() & 1, 0);
        }
        assert_eq!(rig.post("/api/input/key", json!({"key":"b","mods":["ctrl"],"hold_ms":20})).status, 200,
            "an owned mouse button permits independent keyboard input");
        native_keyboard_accepted(&client, "mouse",
            json!({"action":"up","handle":down.structured_content.unwrap()["hold"]["handle"]})).await;
        client.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_mouse_uses_real_captured_text_picture_coordinates() {
    use rmcp::model::ProtocolVersion;
    let rig = Rig::with("native-mouse-text", Options { mcp: true, ..Options::default() });
    let reference = json!({"action":"move","x":100,"y":80,"coords":"screen"});
    assert_eq!(rig.post("/api/input/mouse", reference.clone()).status, 200,
        "approved HTTP motion reaches the actual text-mode guest");
    rig.until_idle();
    let expected = rig.get("/api/status").json()["mouse"].clone();
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        for version in [ProtocolVersion::V_2025_11_25, ProtocolVersion::V_2026_07_28] {
            let client = native_keyboard_client(&rig, version).await;
            let tools = client.list_all_tools().await.unwrap();
            assert!(tools.iter().any(|tool| tool.name == "mouse"),
                "native mouse must be advertised for the actual text-mode guest");
            let capture = native_keyboard_accepted(&client, "capture", json!({})).await;
            let picture = capture.structured_content.unwrap()["mouse_picture"].clone();
            assert!(picture["mode_epoch"].as_u64().is_some(), "actual capture supplies the mouse picture basis");
            let reset = json!({"action":"move","x":20,"y":20,"coords":"screen"});
            assert_eq!(rig.post("/api/input/mouse", reset).status, 200);
            rig.until_idle();
            let moved = native_keyboard_accepted(&client, "mouse",
                json!({"action":"move","picture":picture,"x":reference["x"],"y":reference["y"]})).await;
            let operation = moved.structured_content.unwrap()["op"].as_u64().unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            while rig.get(&format!("/api/ops/{operation}")).json()["state"] != "done" {
                assert!(Instant::now() < deadline, "native absolute mouse motion actually delivers");
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            let observed = rig.get("/api/status").json()["mouse"].clone();
            assert_eq!((&observed["x"], &observed["y"]), (&expected["x"], &expected["y"]),
                "native picture pixels match the real HTTP-backed guest position");
            client.cancel().await.unwrap();
        }
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_mouse_button_claim_survives_disconnect_and_qualifies_motion() {
    use rmcp::model::ProtocolVersion;
    let rig = Rig::with("native-button-hold", Options { mcp: true, ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let legacy = native_keyboard_client(&rig, ProtocolVersion::V_2025_11_25).await;
        assert!(legacy.list_all_tools().await.unwrap().iter().any(|tool| tool.name == "mouse"),
            "persistent native mouse control must be advertised");
        let captured = native_keyboard_accepted(&legacy, "capture", json!({})).await;
        let picture = captured.structured_content.unwrap()["mouse_picture"].clone();
        assert!(picture["mode_epoch"].as_u64().is_some());
        let down = native_keyboard_accepted(&legacy, "mouse",
            json!({"action":"down","button":"left","lease_ms":30000,"picture":picture,"x":100,"y":80})).await;
        let handle = down.structured_content.unwrap()["hold"]["handle"].as_str().unwrap().to_owned();
        let deadline = Instant::now() + Duration::from_secs(10);
        while rig.get("/api/status").json()["mouse"]["buttons"].as_u64().unwrap() & 1 == 0 {
            assert!(Instant::now() < deadline, "actual guest receives held left button");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        legacy.cancel().await.unwrap();
        assert_ne!(rig.get("/api/status").json()["mouse"]["buttons"].as_u64().unwrap() & 1, 0,
            "disconnect retains the actual owned button");
        let current = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let renewed = native_keyboard_accepted(&current, "mouse",
            json!({"action":"renew","handle":handle,"lease_ms":30000})).await;
        assert_eq!(renewed.structured_content.unwrap()["hold"]["state"], "delivered");
        native_keyboard_accepted(&current, "mouse",
            json!({"action":"move","handle":handle,"picture":picture,"x":140,"y":100})).await;
        assert_ne!(rig.get("/api/status").json()["mouse"]["buttons"].as_u64().unwrap() & 1, 0,
            "handle-qualified movement preserves its button down");
        native_keyboard_accepted(&current, "mouse", json!({"action":"up","handle":handle})).await;
        let deadline = Instant::now() + Duration::from_secs(10);
        while rig.get("/api/status").json()["mouse"]["buttons"].as_u64().unwrap() & 1 != 0 {
            assert!(Instant::now() < deadline, "matching handle releases the real button");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        current.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_mouse_supported_buttons_and_clicks_observe_real_guest_state() {
    use rmcp::model::ProtocolVersion;
    let rig = Rig::with("native-mouse-buttons", Options { mcp: true, ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        for version in [ProtocolVersion::V_2025_11_25, ProtocolVersion::V_2026_07_28] {
            let client = native_keyboard_client(&rig, version).await;
            assert!(client.list_all_tools().await.unwrap().iter().any(|tool| tool.name == "mouse"),
                "all approved native mouse buttons must be advertised");
            let capture = native_keyboard_accepted(&client, "capture", json!({})).await;
            let picture = capture.structured_content.unwrap()["mouse_picture"].clone();
            assert!(picture["mode_epoch"].as_u64().is_some());
            for (button, bit) in [("left",1), ("right",2), ("middle",4)] {
                let down = native_keyboard_accepted(&client, "mouse",
                    json!({"action":"down","picture":picture,"button":button,"lease_ms":30000})).await;
                let handle = down.structured_content.unwrap()["hold"]["handle"].as_str().unwrap().to_owned();
                let deadline = Instant::now() + Duration::from_secs(10);
                loop {
                    let state = rig.get("/api/status").json()["mouse"]["buttons"].as_u64().unwrap();
                    if state == bit { break; }
                    assert!(Instant::now() < deadline, "native {button} reaches only its real guest button bit");
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                native_keyboard_accepted(&client, "mouse", json!({"action":"up","handle":handle})).await;
                rig.until_idle();
                assert_eq!(rig.get("/api/status").json()["mouse"]["buttons"], 0);
                let clicked = native_keyboard_accepted(&client, "mouse",
                    json!({"action":"click","picture":picture,"button":button,"hold_ms":150})).await;
                let op = clicked.structured_content.unwrap()["op"].as_u64().unwrap();
                let deadline = Instant::now() + Duration::from_secs(10);
                while rig.get(&format!("/api/ops/{op}")).json()["state"] != "done" {
                    assert!(Instant::now() < deadline, "native {button} click actually completes once");
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                assert_eq!(rig.get("/api/status").json()["mouse"]["buttons"], 0,
                    "the completed click leaves no real guest button held");
                assert!(rig.journal().iter().any(|entry| entry["type"] == "input_delivered" && entry["cause"] == op));
            }
            client.cancel().await.unwrap();
        }
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_canonical_dir_and_enter_show_actual_guest_files() {
    use rmcp::model::ProtocolVersion;
    let rig = Rig::with("native-dir", Options { mcp: true, ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        for version in [ProtocolVersion::V_2025_11_25, ProtocolVersion::V_2026_07_28] {
            let client = native_keyboard_client(&rig, version).await;
            let before = rig.get("/api/screen/text").json()["lines"].as_array().unwrap().iter()
                .filter(|line| line.as_str().is_some_and(|line| line.contains("PROBE"))).count();
            let typed = native_keyboard_accepted(&client, "type", json!({"text":"dir"})).await;
            let entered = native_keyboard_accepted(&client, "key", json!({"key":"enter"})).await;
            let operations = [typed.structured_content.unwrap()["op"].as_u64().unwrap(), entered.structured_content.unwrap()["op"].as_u64().unwrap()];
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let listed = rig.get("/api/screen/text").json()["lines"].as_array().unwrap().iter()
                    .filter(|line| line.as_str().is_some_and(|line| line.contains("PROBE"))).count();
                let delivered = operations.iter().all(|operation| rig.get(&format!("/api/ops/{operation}")).json()["state"] == "done");
                if delivered && listed > before { break; }
                assert!(Instant::now() < deadline, "actual dir output lists the rig's PROBE.COM");
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            let journal = rig.journal();
            for operation in operations {
                assert!(journal.iter().any(|entry| entry["type"] == "input_delivered" && entry["cause"] == operation));
            }
            client.cancel().await.unwrap();
        }
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_selected_modifiers_reach_real_bios_and_release() {
    use rmcp::model::ProtocolVersion;
    let rig = Rig::with("native-modifiers", Options { mcp: true, ..Options::default() });
    assert_eq!(rig.post("/api/control/run", json!({"command":"LOOP.COM"})).status, 200);
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        for (modifier, expected) in [("ctrl", 0x1e01u16), ("shift", 0x1e41), ("alt", 0x1e00)] {
            let pointers = native_keyboard_bytes(&rig, "0040:001A", 4);
            let tail = u16::from_le_bytes([pointers[2], pointers[3]]);
            let result = native_keyboard_accepted(&client, "key", json!({"key":"a","mods":[modifier],"hold_ms":50})).await;
            let operation = result.structured_content.unwrap()["op"].as_u64().unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if rig.get(&format!("/api/ops/{operation}")).json()["state"] == "done" { break; }
                assert!(Instant::now() < deadline, "selected-modifier input delivers");
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            let code = native_keyboard_bytes(&rig, &format!("0040:{tail:04X}"), 2);
            assert_eq!(u16::from_le_bytes([code[0], code[1]]), expected, "actual BIOS keystroke preserves {modifier}");
            assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 0x0f, 0, "balanced press releases its modifiers");
        }
        client.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_foreign_plain_input_conflicts_with_owned_ctrl_and_shift() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    let rig = Rig::with("native-modifier-conflicts", Options { mcp: true, ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let owner = native_keyboard_client(&rig, ProtocolVersion::V_2025_11_25).await;
        let foreign = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        for (key, bit) in [("ctrl", 0x04u8), ("shift", 0x02)] {
            let down = native_keyboard_accepted(&owner, "key", json!({"action":"down","key":key,"lease_ms":30000})).await;
            let handle = down.structured_content.unwrap()["hold"]["handle"].as_str().unwrap().to_owned();
            let deadline = Instant::now() + Duration::from_secs(10);
            while native_keyboard_bytes(&rig, "0040:0017", 1)[0] & bit == 0 {
                assert!(Instant::now() < deadline, "owned modifier is delivered");
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            let queued = rig.journal().iter().filter(|entry| entry["type"] == "input_queued").count();
            for (tool, args) in [("key", json!({"key":"a"})), ("type", json!({"text":"a"}))] {
                let mut request = CallToolRequestParams::new(tool);
                request.arguments = Some(args.as_object().unwrap().clone());
                let refused = foreign.call_tool(request).await.unwrap();
                assert_eq!(refused.is_error, Some(true), "foreign native plain input must preserve effective {key}");
                assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & bit, bit);
            }
            for (path, args) in [("/api/input/key", json!({"key":"a"})), ("/api/input/type", json!({"text":"a"})), ("/api/input/type", json!({"text":"é"})), ("/api/input/key", json!({"key":key,"action":"up"})), ("/api/input/key", json!({"key":key,"action":"down"})), ("/api/input/key", json!({"key":"alt","action":"up"})), ("/api/input/key", json!({"key":"alt","action":"down"}))] {
                assert_eq!(rig.post(path, args).status, 409, "foreign HTTP plain input rejects before queue mutation");
                assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & bit, bit);
            }
            assert_eq!(rig.journal().iter().filter(|entry| entry["type"] == "input_queued").count(), queued);
            native_keyboard_accepted(&foreign, "key", json!({"key":"a","mods":[key]})).await;
            assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & bit, bit, "explicitly compatible press neither releases nor reasserts the retained modifier");
            if key == "ctrl" {
                let mut request = CallToolRequestParams::new("key");
                request.arguments = Some(json!({"action":"down","key":"shift","mods":["ctrl"],"lease_ms":30000}).as_object().unwrap().clone());
                assert_eq!(foreign.call_tool(request).await.unwrap().is_error, Some(true), "a second persistent modifier claim cannot borrow the first owner's modifier");
                let mut request = CallToolRequestParams::new("key");
                request.arguments = Some(json!({"action":"down","key":"a","mods":["ctrl"],"lease_ms":30000}).as_object().unwrap().clone());
                assert_eq!(foreign.call_tool(request).await.unwrap().is_error, Some(true), "a persistent chord cannot claim another owner's retained modifier");
                let balanced = native_keyboard_accepted(&foreign, "key", json!({"key":"a","mods":["ctrl","shift"]})).await.structured_content.unwrap();
                let deadline = Instant::now() + Duration::from_secs(5);
                while rig.get(&format!("/api/ops/{}", balanced["op"].as_u64().unwrap())).json()["state"] != "done" {
                    assert!(Instant::now() < deadline);
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 6, 4, "the compatible press releases only its own added Shift");
            }
            native_keyboard_accepted(&owner, "key", json!({"action":"up","handle":handle})).await;
        }
        foreign.cancel().await.unwrap();
        owner.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_queued_holds_and_legacy_modifier_reservations_conflict_before_effects() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    let rig = Rig::with("native-queued-conflicts", Options { mcp: true, ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let wait = rig.post("/api/input/wait?wait=false", json!({"ms":8000}));
        assert_eq!(wait.status, 200);
        let down = native_keyboard_accepted(&client, "key", json!({"action":"down","key":"ctrl","lease_ms":30000})).await;
        let handle = down.structured_content.unwrap()["hold"]["handle"].as_str().unwrap().to_owned();
        assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 0x04, 0, "owned down is still behind the actual wait");
        assert_eq!(rig.post("/api/input/type", json!({"text":"a"})).status, 409, "queued native modifier already reserves its effective state");
        native_keyboard_accepted(&client, "key", json!({"action":"up","handle":handle})).await;
        let legacy = rig.post("/api/input/key?wait=false", json!({"key":"ctrl","action":"down"}));
        assert_eq!(legacy.status, 200);
        assert_eq!(rig.post("/api/input/key?wait=false", json!({"key":"ctrl","action":"down"})).status, 200, "legacy repeated makes retain their upstream queue behavior");
        let queued = native_queued_input(&rig);
        for args in [json!({"action":"down","key":"ctrl","lease_ms":30000}), json!({"key":"a"})] {
            let mut request = CallToolRequestParams::new("key");
            request.arguments = Some(args.as_object().unwrap().clone());
            let refused = client.call_tool(request).await.unwrap();
            assert_eq!(refused.is_error, Some(true), "ordered queued legacy Ctrl prevents conflicting native input");
        }
        assert_eq!(native_queued_input(&rig), queued, "native rejection preserves the legacy queue");
        client.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_explicit_null_keyboard_fields_allocate_no_actor_work() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    let rig = Rig::with("native-null-keyboard-fields", Options { mcp: true, ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        native_keyboard_accepted(&client, "key", json!({"key":"enter"})).await;
        let down = native_keyboard_accepted(&client, "key", json!({"action":"down","key":"ctrl","lease_ms":30000})).await.structured_content.unwrap();
        for (tool, args) in [
            ("key", json!({"key":"enter","mods":["ctrl"],"action":null})),
            ("key", json!({"key":"enter","mods":["ctrl"],"hold_ms":null})),
            ("key", json!({"key":"enter","mods":["ctrl"],"lease_ms":null})),
            ("key", json!({"key":"enter","mods":["ctrl"],"handle":null})),
            ("key", json!({"action":"up","handle":down["hold"]["handle"],"key":null})),
            ("key", json!({"action":"up","handle":down["hold"]["handle"],"hold_ms":null})),
            ("type", json!({"text":"dir","delay_ms":null})),
        ] {
            let queued = rig.journal().iter().filter(|entry| entry["type"] == "input_queued").count();
            let before = rig.shared.next_req.load(std::sync::atomic::Ordering::Relaxed);
            let mut request = CallToolRequestParams::new(tool);
            request.arguments = Some(args.as_object().unwrap().clone());
            assert!(client.call_tool(request).await.is_err(), "explicit null violates the advertised keyboard schema");
            assert_eq!(rig.shared.next_req.load(std::sync::atomic::Ordering::Relaxed), before, "null fields allocate no actor request");
            assert_eq!(rig.journal().iter().filter(|entry| entry["type"] == "input_queued").count(), queued, "null fields enqueue no guest input");
        }
        native_keyboard_accepted(&client, "key", json!({"action":"up","handle":down["hold"]["handle"]})).await;
        client.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_invalid_keyboard_requests_allocate_no_actor_work() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    let rig = Rig::with("native-input-validation", Options { mcp: true, ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        native_keyboard_accepted(&client, "key", json!({"key":"enter"})).await;
        for (tool, args) in [
            ("key", json!({"key":"not-a-key"})),
            ("key", json!({"key":"a","mods":["not-a-modifier"]})),
            ("key", json!({"key":"a","mods":["ctrl","ctrl"]})),
            ("key", json!({"key":"ctrl","mods":["ctrl"]})),
            ("key", json!({"key":"a","mods":["a"]})),
            ("key", json!({"key":"a","mods":["ctrl","shift","alt","ctrl","shift"]})),
            ("key", json!({"action":"press"})),
            ("key", json!({"action":"click","key":"a"})),
            ("key", json!({"key":"a","lease_ms":50})),
            ("key", json!({"action":"down","key":"ctrl","lease_ms":100,"hold_ms":50})),
            ("key", json!({"action":"down","key":"ctrl"})),
            ("key", json!({"key":"a","hold_ms":0})),
            ("key", json!({"key":"a","hold_ms":5001})),
            ("key", json!({"action":"down","key":"ctrl","lease_ms":0})),
            ("key", json!({"action":"down","key":"ctrl","lease_ms":30001})),
            ("type", json!({"text":"dir🙂"})),
            ("type", json!({"text":"dir".repeat(1366)})),
            ("type", json!({"text":"dir","unexpected":true})),
            ("type", json!({"text":"dir","delay_ms":5001})),
            ("type", json!({"text":1})),
        ] {
            let before = rig.shared.next_req.load(std::sync::atomic::Ordering::Relaxed);
            let mut request = CallToolRequestParams::new(tool);
            request.arguments = Some(args.as_object().unwrap().clone());
            let refused = client.call_tool(request).await;
            assert!(refused.is_err(), "invalid native input fails syntax/canonical validation");
            assert_eq!(rig.shared.next_req.load(std::sync::atomic::Ordering::Relaxed), before, "invalid native input allocates no actor request");
        }
        client.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_invalid_hold_and_operation_requests_preserve_live_authority() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    let rig = Rig::with("native-handle-validation", Options { mcp: true, ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let down = native_keyboard_accepted(&client, "key", json!({"action":"down","key":"ctrl","lease_ms":30000})).await.structured_content.unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while rig.get(&format!("/api/ops/{}", down["op"].as_u64().unwrap())).json()["state"] != "done" {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let handle = down["hold"]["handle"].as_str().unwrap();
        let instance = down["instance"].as_str().unwrap();
        let mut invalid_hex = handle.to_owned();
        invalid_hex.replace_range(..1, "g");
        let mut misplaced_separator = handle.to_owned();
        misplaced_separator.remove(16);
        misplaced_separator.insert(17, '.');
        for (tool, args) in [
            ("key", json!({"action":"up"})),
            ("key", json!({"action":"up","handle":&handle[..49]})),
            ("key", json!({"action":"up","handle":handle.replace('.', "_")})),
            ("key", json!({"action":"up","handle":invalid_hex})),
            ("key", json!({"action":"up","handle":misplaced_separator})),
            ("key", json!({"action":"up","handle":handle,"key":"ctrl"})),
            ("key", json!({"action":"up","handle":handle,"mods":["ctrl"]})),
            ("key", json!({"action":"up","handle":handle,"hold_ms":1})),
            ("key", json!({"action":"up","handle":handle,"lease_ms":1})),
            ("key", json!({"action":"renew","handle":handle})),
            ("key", json!({"action":"renew","handle":handle,"lease_ms":0})),
            ("key", json!({"action":"renew","handle":handle,"lease_ms":30001})),
            ("key", json!({"key":"a","handle":handle})),
            ("operation", json!({"instance":&instance[..15],"op":down["op"]})),
            ("operation", json!({"instance":format!("g{}", &instance[1..]),"op":down["op"]})),
            ("operation", json!({"instance":instance,"op":0})),
            ("operation", json!({"instance":instance,"op":down["op"],"unexpected":true})),
        ] {
            let before = rig.shared.next_req.load(std::sync::atomic::Ordering::Relaxed);
            let mut request = CallToolRequestParams::new(tool);
            request.arguments = Some(args.as_object().unwrap().clone());
            assert!(client.call_tool(request).await.is_err(), "invalid request rejects before the actor: {args}");
            assert_eq!(rig.shared.next_req.load(std::sync::atomic::Ordering::Relaxed), before);
            assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 4, 4);
            assert_eq!(rig.shared.native_jobs.available_permits(), 8);
        }
        let mut stale = handle.to_owned();
        stale.replace_range(..1, if &handle[..1] == "0" { "1" } else { "0" });
        let mut unknown = handle.to_owned();
        unknown.replace_range(49..50, if &handle[49..50] == "0" { "1" } else { "0" });
        for (handle, expected) in [(stale, "stale_instance"), (unknown, "unknown_hold")] {
            let mut request = CallToolRequestParams::new("key");
            request.arguments = Some(json!({"action":"up","handle":handle}).as_object().unwrap().clone());
            let refused = client.call_tool(request).await.unwrap();
            assert_eq!(refused.is_error, Some(true));
            assert_eq!(refused.structured_content.as_ref().unwrap()["error"]["code"], expected);
            assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 4, 4);
        }
        let status = native_keyboard_accepted(&client, "status", json!({})).await.structured_content.unwrap();
        let run = rig.post("/api/control/run", json!({"command":"LOOP.COM"}));
        assert_eq!(run.status, 200, "{}", run.body);
        for (instance, op, expected) in [
            (format!("{}{}", if &instance[..1] == "0" { "1" } else { "0" }, &instance[1..]), down["op"].clone(), "stale_instance"),
            (instance.to_owned(), status["req"].clone(), "unknown_operation"),
            (instance.to_owned(), run.json()["op"].clone(), "unknown_operation"),
        ] {
            let mut request = CallToolRequestParams::new("operation");
            request.arguments = Some(json!({"instance":instance,"op":op}).as_object().unwrap().clone());
            let refused = client.call_tool(request).await.unwrap();
            assert_eq!(refused.is_error, Some(true));
            assert_eq!(refused.structured_content.as_ref().unwrap()["error"]["code"], expected, "operation lookup preserves its declared error");
            assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 4, 4);
        }
        native_keyboard_accepted(&client, "key", json!({"action":"up","handle":down["hold"]["handle"]})).await;
        client.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_live_hold_capacity_is_separate_from_delivered_job_capacity() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    let rig = Rig::with("native-live-hold-capacity", Options { mcp: true, ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let mut holds = Vec::new();
        for key in ["f1", "f2", "f3", "f4", "f5", "f6", "f7", "f8"] {
            let down = native_keyboard_accepted(&client, "key", json!({"action":"down","key":key,"lease_ms":30000})).await.structured_content.unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while rig.get(&format!("/api/ops/{}", down["op"].as_u64().unwrap())).json()["state"] != "done" {
                assert!(Instant::now() < deadline);
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            holds.push(down);
            assert_eq!(rig.shared.native_jobs.available_permits(), 8, "delivered down frees its job while retaining its hold");
        }
        let before = rig.journal().iter().filter(|entry| entry["type"] == "input_queued").count();
        let mut request = CallToolRequestParams::new("key");
        request.arguments = Some(json!({"action":"down","key":"f9","lease_ms":30000}).as_object().unwrap().clone());
        let refused = client.call_tool(request).await.unwrap();
        assert_eq!(refused.is_error, Some(true));
        assert_eq!(refused.structured_content.as_ref().unwrap()["error"]["code"], "capacity");
        assert_eq!(rig.journal().iter().filter(|entry| entry["type"] == "input_queued").count(), before);
        assert_eq!(rig.shared.native_jobs.available_permits(), 8);
        assert_eq!(rig.post("/api/input/key", json!({"key":"f1","action":"up"})).status, 409, "legacy releases cannot target a native owned key");
        assert_eq!(rig.post("/api/input/key", json!({"key":"f1","action":"down"})).status, 409, "legacy makes cannot reassert a native owned key");
        assert_eq!(rig.post("/api/input/key", json!({"key":"a"})).status, 200, "unrelated balanced legacy input remains available");
        assert_eq!(rig.post("/api/input/key", json!({"key":"a","action":"up"})).status, 200, "an unrelated legacy release preserves native claims");
        assert_eq!(rig.post("/api/input/type", json!({"text":"é"})).status, 200, "supported character fallback preserves unrelated holds");
        for hold in &holds {
            let observed = native_keyboard_accepted(&client, "operation", json!({"instance":hold["instance"],"op":hold["op"]})).await.structured_content.unwrap();
            assert_eq!(observed["hold"]["state"], "delivered", "capacity rejection preserves each existing hold");
            native_keyboard_accepted(&client, "key", json!({"action":"up","handle":hold["hold"]["handle"]})).await;
        }
        native_keyboard_accepted(&client, "key", json!({"action":"down","key":"f9","lease_ms":30000})).await;
        assert_eq!(http(rig.addr, "DELETE", "/api/input", &[], None).status, 200);
        client.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_ended_hold_eviction_cannot_release_a_new_owner() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    let rig = Rig::with("native-ended-hold-eviction", Options { mcp: true, ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let mut first = None;
        for _ in 0..=super::super::journal::OPS_CAPACITY {
            let down = native_keyboard_accepted(&client, "key", json!({"action":"down","key":"f1","lease_ms":30000})).await.structured_content.unwrap();
            if first.is_none() { first = Some(down.clone()); }
            native_keyboard_accepted(&client, "key", json!({"action":"up","handle":down["hold"]["handle"]})).await;
        }
        let live = native_keyboard_accepted(&client, "key", json!({"action":"down","key":"ctrl","lease_ms":30000})).await.structured_content.unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while rig.get(&format!("/api/ops/{}", live["op"].as_u64().unwrap())).json()["state"] != "done" {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let mut request = CallToolRequestParams::new("key");
        request.arguments = Some(json!({"action":"up","handle":first.unwrap()["hold"]["handle"]}).as_object().unwrap().clone());
        let refused = client.call_tool(request).await.unwrap();
        assert_eq!(refused.is_error, Some(true));
        assert_eq!(refused.structured_content.as_ref().unwrap()["error"]["code"], "unknown_hold", "ended history has a bounded retention window");
        assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 4, 4, "evicted authority cannot release a current owner's Ctrl");
        assert_eq!(rig.shared.native_jobs.available_permits(), 8);
        native_keyboard_accepted(&client, "key", json!({"action":"up","handle":live["hold"]["handle"]})).await;
        client.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_wrong_and_old_released_handles_preserve_new_same_key_hold() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    let rig = Rig::with("native-handle-conflicts", Options { mcp: true, ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let owner = native_keyboard_client(&rig, ProtocolVersion::V_2025_11_25).await;
        let foreign = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let down = native_keyboard_accepted(&owner, "key", json!({"action":"down","key":"ctrl","lease_ms":30000})).await;
        let handle = down.structured_content.unwrap()["hold"]["handle"].as_str().unwrap().to_owned();
        let deadline = Instant::now() + Duration::from_secs(10);
        while native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 0x04 == 0 {
            assert!(Instant::now() < deadline, "initial owned Ctrl delivers");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let mut wrong = handle.as_bytes().to_vec();
        let last = wrong.last_mut().unwrap();
        *last = if *last == b'0' { b'1' } else { b'0' };
        let wrong = String::from_utf8(wrong).unwrap();
        for args in [json!({"action":"up","handle":wrong}), json!({"action":"renew","handle":wrong,"lease_ms":30000}), json!({"action":"down","key":"ctrl","lease_ms":30000})] {
            let mut request = CallToolRequestParams::new("key");
            request.arguments = Some(args.as_object().unwrap().clone());
            let refused = foreign.call_tool(request).await.unwrap();
            assert_eq!(refused.is_error, Some(true), "wrong ownership or duplicate key claim rejects");
            assert_ne!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 0x04, 0, "rejection preserves the real owned Ctrl");
        }
        native_keyboard_accepted(&owner, "key", json!({"action":"up","handle":handle})).await;
        let deadline = Instant::now() + Duration::from_secs(10);
        while native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 0x04 != 0 {
            assert!(Instant::now() < deadline, "matching original up delivers");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let next = native_keyboard_accepted(&foreign, "key", json!({"action":"down","key":"ctrl","lease_ms":30000})).await;
        let next_handle = next.structured_content.unwrap()["hold"]["handle"].as_str().unwrap().to_owned();
        assert_ne!(next_handle, handle, "new claim has a distinct authoritative handle");
        let deadline = Instant::now() + Duration::from_secs(10);
        while native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 0x04 == 0 {
            assert!(Instant::now() < deadline, "new owned Ctrl delivers");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let duplicate = native_keyboard_accepted(&owner, "key", json!({"action":"up","handle":handle})).await;
        assert_eq!(duplicate.structured_content.unwrap()["hold"]["state"], "released");
        assert_ne!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 0x04, 0, "old duplicate up never releases a newer same-key owner");
        native_keyboard_accepted(&foreign, "key", json!({"action":"up","handle":next_handle})).await;
        foreign.cancel().await.unwrap();
        owner.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_modifier_sides_cannot_share_or_toggle_an_owned_effect() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    for (held, other, mask) in [("ctrl", "rctrl", 4), ("rctrl", "ctrl", 4), ("alt", "ralt", 8), ("ralt", "alt", 8)] {
        let rig = Rig::with(&format!("native-modifier-side-{held}"), Options { mcp: true, ..Options::default() });
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        runtime.block_on(async {
            let owner = native_keyboard_client(&rig, ProtocolVersion::V_2025_11_25).await;
            let foreign = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
            let down = native_keyboard_accepted(&owner, "key", json!({"action":"down","key":held,"lease_ms":30000})).await.structured_content.unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while native_keyboard_bytes(&rig, "0040:0017", 1)[0] & mask == 0 {
                assert!(Instant::now() < deadline);
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            let before = native_keyboard_bytes(&rig, "0040:0017", 2);
            let enhanced = native_keyboard_bytes(&rig, "0040:0096", 1);
            for args in [json!({"action":"down","key":other,"lease_ms":30000}), json!({"key":"a","mods":[other]})] {
                let mut request = CallToolRequestParams::new("key");
                request.arguments = Some(args.as_object().unwrap().clone());
                assert_eq!(foreign.call_tool(request).await.unwrap().is_error, Some(true), "same modifier effect cannot be independently owned or toggled");
                assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 2), before);
                assert_eq!(native_keyboard_bytes(&rig, "0040:0096", 1), enhanced);
            }
            let borrowed = native_keyboard_accepted(&foreign, "key", json!({"key":"a","mods":[held]})).await.structured_content.unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while rig.get(&format!("/api/ops/{}", borrowed["op"].as_u64().unwrap())).json()["state"] != "done" {
                assert!(Instant::now() < deadline, "exact physical modifier borrowing must deliver");
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 2), before);
            assert_eq!(native_keyboard_bytes(&rig, "0040:0096", 1), enhanced);
            native_keyboard_accepted(&owner, "key", json!({"action":"up","handle":down["hold"]["handle"]})).await;
            foreign.cancel().await.unwrap();
            owner.cancel().await.unwrap();
        });
        rig.until_idle();
    }
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_projects_remaining_legacy_modifier_sides_after_a_release() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    let rig = Rig::with("native-legacy-modifier-sides", Options { mcp: true, ..Options::default() });
    for key in ["ctrl", "rctrl"] {
        assert_eq!(rig.post("/api/input/key?wait=true", json!({"key":key,"action":"down"})).status, 200);
    }
    assert_eq!(rig.post("/api/input/key?wait=true", json!({"key":"rctrl","action":"up"})).status, 200);
    assert_ne!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 4, 0, "the real guest still holds left Ctrl");
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let mut request = CallToolRequestParams::new("key");
        request.arguments = Some(json!({"key":"a"}).as_object().unwrap().clone());
        assert_eq!(client.call_tool(request).await.unwrap().is_error, Some(true), "remaining physical Ctrl prevents unmodified native input");
        client.cancel().await.unwrap();
    });
    assert_eq!(rig.post("/api/input/key?wait=true", json!({"key":"ctrl","action":"up"})).status, 200);
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_queued_legacy_modifier_sides_keep_exact_borrowing() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    for (held, released, mask) in [("ctrl", "rctrl", 4), ("rctrl", "ctrl", 4), ("alt", "ralt", 8), ("ralt", "alt", 8)] {
        let rig = Rig::with(&format!("native-queued-modifier-side-{held}"), Options { mcp: true, deterministic: Some(rust_dos::deterministic::TICK_MS), ..Options::default() });
        for key in [held, released] {
            assert_eq!(rig.post("/api/input/key?wait=false", json!({"key":key,"action":"down"})).status, 200);
        }
        assert_eq!(rig.post("/api/input/key?wait=false", json!({"key":released,"action":"up"})).status, 200);
        assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & mask, 0, "queued modifiers have not reached the paused guest");
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        runtime.block_on(async {
            let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
            let before = native_queued_input(&rig);
            for (tool, args) in [("key", json!({"key":"a"})), ("type", json!({"text":"dir"}))] {
                let mut request = CallToolRequestParams::new(tool);
                request.arguments = Some(args.as_object().unwrap().clone());
                assert_eq!(client.call_tool(request).await.unwrap().is_error, Some(true), "queued remaining modifier preserves unmodified key/text semantics");
            }
            assert_eq!(native_queued_input(&rig), before);
            let borrowed = native_keyboard_accepted(&client, "key", json!({"key":"a","mods":[held]})).await.structured_content.unwrap();
            assert_eq!(borrowed["held"], true);
            assert_eq!(rig.post("/api/control/resume", json!({})).status, 200);
            let deadline = Instant::now() + Duration::from_secs(5);
            while rig.get(&format!("/api/ops/{}", borrowed["op"].as_u64().unwrap())).json()["state"] != "done" {
                assert!(Instant::now() < deadline, "queued exact borrowing must deliver");
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            assert_ne!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & mask, 0, "borrowed native input must not release the legacy modifier");
            client.cancel().await.unwrap();
        });
        assert_eq!(rig.post("/api/control/pause", json!({})).status, 200);
        assert_eq!(rig.post("/api/input/key?wait=false", json!({"key":held,"action":"up"})).status, 200);
        assert_eq!(rig.post("/api/control/resume", json!({})).status, 200);
        rig.until_idle();
    }
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_typing_cannot_release_a_retained_character_key() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    let rig = Rig::with("native-type-owned-character", Options { mcp: true, ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let owner = native_keyboard_client(&rig, ProtocolVersion::V_2025_11_25).await;
        let foreign = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let down = native_keyboard_accepted(&owner, "key", json!({"action":"down","key":"a","lease_ms":30000})).await.structured_content.unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while rig.get(&format!("/api/ops/{}", down["op"].as_u64().unwrap())).json()["state"] != "done" {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let queued = rig.journal().iter().filter(|entry| entry["type"] == "input_queued").count();
        let mut request = CallToolRequestParams::new("type");
        request.arguments = Some(json!({"text":"ba"}).as_object().unwrap().clone());
        assert_eq!(foreign.call_tool(request).await.unwrap().is_error, Some(true), "typing must not make and release another owner's character key");
        assert_eq!(rig.journal().iter().filter(|entry| entry["type"] == "input_queued").count(), queued, "the whole text must reject before its prefix");
        native_keyboard_accepted(&foreign, "type", json!({"text":"b"})).await;
        native_keyboard_accepted(&owner, "key", json!({"action":"up","handle":down["hold"]["handle"]})).await;
        foreign.cancel().await.unwrap();
        owner.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_keyboard_discovery_advertises_exact_implemented_contracts() {
    use rmcp::model::ProtocolVersion;
    let rig = Rig::with("native-keyboard-discovery", Options { mcp: true, ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        for version in [ProtocolVersion::V_2025_11_25, ProtocolVersion::V_2026_07_28] {
            let client = native_keyboard_client(&rig, version).await;
            let tools = client.list_all_tools().await.unwrap();
            let mut names = tools.iter().map(|tool| tool.name.as_ref()).collect::<Vec<_>>();
            names.sort_unstable();
            assert_eq!(names, ["capture", "fast_forward", "key", "mouse", "operation", "speed", "status", "type"], "native discovery exposes exactly its real contracts");
            for tool in &tools {
                let hints = tool.annotations.as_ref().expect("native tools describe their side effects");
                let read_only = matches!(tool.name.as_ref(), "status" | "capture" | "operation");
                assert_eq!(hints.read_only_hint, Some(read_only));
                assert_eq!(hints.open_world_hint, Some(false));
                if !read_only {
                    assert_eq!(hints.destructive_hint, Some(true), "guest input can delete mounted files");
                    assert_eq!(hints.idempotent_hint, Some(false), "uncertain input acceptance must not be replayed");
                }
                assert_eq!(tool.input_schema.get("type").and_then(Value::as_str), Some("object"));
                assert_eq!(tool.input_schema.get("additionalProperties"), Some(&Value::Bool(false)));
                if matches!(tool.name.as_ref(), "status" | "capture") {
                    assert_eq!(tool.input_schema.len(), 2, "observation inputs remain empty objects");
                }
            }
            let key = tools.iter().find(|tool| tool.name == "key").unwrap();
            assert_eq!(key.input_schema["properties"]["action"]["enum"], json!(["press", "down", "up", "renew"]));
            assert_eq!(key.input_schema["properties"]["hold_ms"]["maximum"], 5000);
            assert_eq!(key.input_schema["properties"]["lease_ms"]["maximum"], 30000);
            client.cancel().await.unwrap();
        }
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_keyboard_oracles_observe_real_http_directory_and_bios_modifiers() {
    let rig = Rig::with("keyboard-oracle-baseline", Options { mcp: true, ..Options::default() });
    assert_eq!(rig.post("/api/input/type?wait=true", json!({"text":"dir"})).status, 200);
    assert_eq!(rig.post("/api/input/key?wait=true", json!({"key":"enter"})).status, 200);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let screen = rig.get("/api/screen/text").json();
        if screen["lines"].as_array().unwrap().iter().any(|line| line.as_str().is_some_and(|line| line.contains("PROBE"))) { break; }
        assert!(Instant::now() < deadline, "actual baseline dir lists PROBE.COM");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(rig.post("/api/control/run", json!({"command":"LOOP.COM"})).status, 200);
    for (modifier, expected) in [("ctrl", 0x1e01u16), ("shift", 0x1e41), ("alt", 0x1e00)] {
        let pointers = native_keyboard_bytes(&rig, "0040:001A", 4);
        let tail = u16::from_le_bytes([pointers[2], pointers[3]]);
        let delivered = rig.post("/api/input/key?wait=true", json!({"key":"a","mods":[modifier],"hold_ms":50}));
        assert_eq!(delivered.status, 200);
        assert_eq!(delivered.json()["done"], true);
        let code = native_keyboard_bytes(&rig, &format!("0040:{tail:04X}"), 2);
        assert_eq!(u16::from_le_bytes([code[0], code[1]]), expected, "actual baseline BIOS preserves {modifier}");
        assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 0x0f, 0);
    }
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_operation_distinguishes_delivered_down_from_retained_hold() {
    use rmcp::model::ProtocolVersion;
    let rig = Rig::with("native-operation-hold", Options { mcp: true, ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let down = native_keyboard_accepted(&client, "key", json!({"action":"down","key":"ctrl","lease_ms":30000})).await.structured_content.unwrap();
        let operation = down["op"].as_u64().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while rig.get(&format!("/api/ops/{operation}")).json()["state"] != "done" {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let observed = native_keyboard_accepted(&client, "operation", json!({"instance":down["instance"],"op":operation})).await.structured_content.unwrap();
        assert_eq!(observed["delivery"], "delivered");
        assert_eq!(observed["hold"]["state"], "delivered");
        assert_eq!(observed["hold"]["handle"], down["hold"]["handle"]);
        assert_ne!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 0x04, 0, "delivered down does not mean released");
        let up = native_keyboard_accepted(&client, "key", json!({"action":"up","handle":down["hold"]["handle"]})).await.structured_content.unwrap();
        let released = native_keyboard_accepted(&client, "operation", json!({"instance":up["instance"],"op":up["op"]})).await.structured_content.unwrap();
        assert_eq!(released["delivery"], "delivered", "release has its own truthful operation");
        assert_eq!(released["hold"]["state"], "released");
        client.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_strict_restore_reports_all_input_without_inventing_pause_waits() {
    use rmcp::model::ProtocolVersion;
    let rig = Rig::with("native-restore-counts", Options { mcp: true, ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let down = native_keyboard_accepted(&client, "key", json!({"action":"down","key":"ctrl","lease_ms":30000})).await.structured_content.unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while rig.get(&format!("/api/ops/{}", down["op"].as_u64().unwrap())).json()["state"] != "done" {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(rig.post("/api/control/pause", json!({})).status, 200);
        rig.until_paused();
        assert_eq!(rig.post("/api/state/save", json!({"path":"counts.state","strict_match":true})).status, 200);
        for _ in 0..2 {
            assert_eq!(rig.post("/api/input/wait?wait=false", json!({"ms":60000})).status, 200);
        }
        native_keyboard_accepted(&client, "key", json!({"key":"a","mods":["ctrl"]})).await;
        let blocked = rig.post("/api/state/save", json!({"path":"counts-blocked.state","strict_match":true}));
        assert_eq!(blocked.status, 409);
        let queued = blocked.json()["input_queued"].as_u64().unwrap();
        let loaded = rig.post("/api/state/load", json!({"path":"counts.state","strict_match":true}));
        assert_eq!(loaded.status, 200);
        let cleared = loaded.json()["debugger_reset"].clone();
        assert_eq!(cleared["pause_waits"], 0, "native input cancellation is not a pause wait");
        assert_eq!(cleared["input"], queued, "strict reset counts every native and legacy pending event");
        assert_eq!(cleared["held_keys"], 1, "strict reset counts the delivered native key before forgetting ownership");
        assert_ne!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 0x04, 0, "reset accounting sends no break codes into restored keyboard");
        client.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_strict_restore_clears_legacy_spans_before_new_native_input() {
    use rmcp::model::ProtocolVersion;
    let rig = Rig::with("native-restore-legacy-spans", Options { mcp: true, ..Options::default() });
    assert_eq!(rig.post("/api/control/pause", json!({})).status, 200);
    rig.until_paused();
    assert_eq!(rig.post("/api/state/save", json!({"path":"legacy-spans.state","strict_match":true})).status, 200);
    for _ in 0..2 {
        assert_eq!(rig.post("/api/input/wait?wait=false", json!({"ms":60000})).status, 200);
    }
    assert_eq!(rig.post("/api/state/load", json!({"path":"legacy-spans.state","strict_match":true})).status, 200);
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let down = native_keyboard_accepted(&client, "key", json!({"action":"down","key":"ctrl","lease_ms":30000})).await.structured_content.unwrap();
        assert_eq!(rig.post("/api/control/resume", json!({})).status, 200);
        let deadline = Instant::now() + Duration::from_secs(10);
        while rig.get(&format!("/api/ops/{}", down["op"].as_u64().unwrap())).json()["state"] != "done" {
            assert!(Instant::now() < deadline, "new native input completes after strict restore");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let operation = native_keyboard_accepted(&client, "operation", json!({"instance":down["instance"],"op":down["op"]})).await.structured_content.unwrap();
        assert_eq!(operation["hold"]["state"], "delivered", "strict restore discards legacy ownership spans before new native input");
        assert_ne!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 0x04, 0, "new native Ctrl reaches actual guest keyboard");
        assert_eq!(rig.shared.native_jobs.available_permits(), 8, "completed native input recovers every job permit");
        native_keyboard_accepted(&client, "key", json!({"action":"up","handle":down["hold"]["handle"]})).await;
        assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 0x04, 0, "matching up releases the new hold");
        client.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_saved_keyboard_restore_invalidates_old_handle_without_break_codes() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    for strict in [false, true] {
        let rig = Rig::with(if strict { "native-strict-restore" } else { "native-restore" }, Options { mcp: true, ..Options::default() });
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        runtime.block_on(async {
            let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
            let down = native_keyboard_accepted(&client, "key", json!({"action":"down","key":"ctrl","lease_ms":30000})).await.structured_content.unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            while native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 0x04 == 0 {
                assert!(Instant::now() < deadline);
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            assert_eq!(rig.post("/api/control/pause", json!({})).status, 200);
            rig.until_paused();
            let saved_flags = native_keyboard_bytes(&rig, "0040:0017", 1);
            let saved = rig.post("/api/state/save", json!({"path":"native-held.state","strict_match":strict}));
            assert_eq!(saved.status, 200, "{}", saved.body);
            let loaded = rig.post("/api/state/load", json!({"path":"native-held.state","strict_match":strict}));
            assert_eq!(loaded.status, 200, "{}", loaded.body);
            assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1), saved_flags, "restored keyboard bytes remain intact");
            for args in [json!({"action":"up","handle":down["hold"]["handle"]}), json!({"action":"renew","handle":down["hold"]["handle"],"lease_ms":30000})] {
                let mut request = CallToolRequestParams::new("key");
                request.arguments = Some(args.as_object().unwrap().clone());
                let refused = client.call_tool(request).await.unwrap();
                assert_eq!(refused.is_error, Some(true), "same-instance restore invalidates old authority");
                assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1), saved_flags, "old up/renew cannot mutate restored keyboard");
            }
            client.cancel().await.unwrap();
        });
        rig.until_idle();
    }
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_expired_modifier_borrow_never_delivers_a_late_key() {
    use rmcp::model::ProtocolVersion;
    let rig = Rig::with("native-borrow-expiry", Options { mcp: true, ..Options::default() });
    assert_eq!(rig.post("/api/control/run", json!({"command":"LOOP.COM"})).status, 200);
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let owner = native_keyboard_client(&rig, ProtocolVersion::V_2025_11_25).await;
        let foreign = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        native_keyboard_accepted(&owner, "key", json!({"action":"down","key":"ctrl","lease_ms":250})).await;
        let deadline = Instant::now() + Duration::from_secs(10);
        while native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 0x04 == 0 {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let tail = native_keyboard_bytes(&rig, "0040:001C", 2);
        assert_eq!(rig.post("/api/input/wait?wait=false", json!({"ms":1000})).status, 200);
        let accepted = native_keyboard_accepted(&foreign, "key", json!({"key":"a","mods":["ctrl"]})).await.structured_content.unwrap();
        let operation = accepted["op"].as_u64().unwrap();
        tokio::time::sleep(Duration::from_millis(1200)).await;
        let state = rig.get(&format!("/api/ops/{operation}")).json()["state"].clone();
        assert!(state == "cancelled" || state == "failed", "expired borrowed modifier cannot silently deliver the queued key: {state}");
        assert_eq!(native_keyboard_bytes(&rig, "0040:001C", 2), tail, "actual BIOS queue sees no late borrowed key");
        assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 0x04, 0);
        foreign.cancel().await.unwrap();
        owner.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_admin_clear_invalidates_queued_hold_and_releases_job_capacity() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    let rig = Rig::with("native-clear-queued", Options { mcp: true, ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        assert_eq!(rig.post("/api/input/wait?wait=false", json!({"ms":8000})).status, 200);
        let down = native_keyboard_accepted(&client, "key", json!({"action":"down","key":"ctrl","lease_ms":30000})).await.structured_content.unwrap();
        assert_eq!(down["hold"]["state"], "reserved");
        let cleared = http(rig.addr, "DELETE", "/api/input", &[], None);
        assert_eq!(cleared.status, 200);
        assert!(cleared.json()["cancelled"].as_array().unwrap().contains(&down["op"]), "admin clear reports the native operation it cancelled");
        assert_eq!(rig.shared.native_jobs.available_permits(), 8, "admin clear retires actual queued job ownership");
        let mut request = CallToolRequestParams::new("key");
        request.arguments = Some(json!({"action":"renew","handle":down["hold"]["handle"],"lease_ms":30000}).as_object().unwrap().clone());
        let refused = client.call_tool(request).await.unwrap();
        assert_eq!(refused.is_error, Some(true), "cleared queued handle is invalidated");
        assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 0x04, 0);
        client.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_deterministic_held_input_survives_client_wait_and_is_queryable() {
    use rmcp::model::ProtocolVersion;
    let rig = Rig::with("native-held-operation", Options { mcp: true, deterministic: Some(rust_dos::deterministic::TICK_MS), ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let first = native_keyboard_client(&rig, ProtocolVersion::V_2025_11_25).await;
        let accepted = native_keyboard_accepted(&first, "type", json!({"text":"dir"})).await.structured_content.unwrap();
        assert_eq!(accepted["held"], true);
        assert_eq!(accepted["acceptance"], "accepted");
        assert_eq!(accepted["wait"], "pending", "held input is pending until actual delivery");
        let operation = accepted["op"].as_u64().unwrap();
        assert_eq!(rig.get(&format!("/api/ops/{operation}")).json()["state"], "pending");
        first.cancel().await.unwrap();
        let next = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let pending = native_keyboard_accepted(&next, "operation", json!({"instance":accepted["instance"],"op":operation})).await.structured_content.unwrap();
        assert_eq!(pending["delivery"], "pending", "dropping caller does not clear accepted input");
        assert_eq!(rig.post("/api/control/resume", json!({})).status, 200);
        let deadline = Instant::now() + Duration::from_secs(10);
        while rig.get(&format!("/api/ops/{operation}")).json()["state"] != "done" {
            assert!(Instant::now() < deadline, "operation: {}; guest: {}", rig.get(&format!("/api/ops/{operation}")).body, rig.get("/api/status").body);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let delivered = native_keyboard_accepted(&next, "operation", json!({"instance":accepted["instance"],"op":operation})).await.structured_content.unwrap();
        assert_eq!(delivered["delivery"], "delivered");
        next.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_shutdown_releases_owned_keyboard_before_immutable_shutdown() {
    use rmcp::model::ProtocolVersion;
    let flags = Arc::new(std::sync::atomic::AtomicUsize::new(usize::MAX));
    let mut rig = Rig::with("native-mutable-shutdown", Options { mcp: true, shutdown_flags: Some(flags.clone()), ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        native_keyboard_accepted(&client, "key", json!({"action":"down","key":"ctrl","lease_ms":30000})).await;
        let deadline = Instant::now() + Duration::from_secs(10);
        while native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 0x04 == 0 {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        client.cancel().await.unwrap();
    });
    assert_eq!(rig.quit().status, 200);
    let observed = flags.load(std::sync::atomic::Ordering::Acquire);
    assert_ne!(observed, usize::MAX, "actual mutable-guest shutdown boundary was observed");
    assert_eq!(observed & 0x04, 0, "owned Ctrl is released before immutable shutdown");
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_mouse_running_deterministic_input_respects_existing_pause_policy() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    let rig = Rig::with("native-mouse-deterministic-policy", Options { mcp: true,
        deterministic: Some(rust_dos::deterministic::TICK_MS), ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        for version in [ProtocolVersion::V_2025_11_25, ProtocolVersion::V_2026_07_28] {
            let client = native_keyboard_client(&rig, version).await;
            let picture = native_keyboard_accepted(&client, "capture", json!({})).await
                .structured_content.unwrap()["mouse_picture"].clone();
            assert_eq!(rig.post("/api/control/resume", json!({"until_ms":100000})).status, 200);
            let deadline = Instant::now() + Duration::from_secs(10);
            while !rig.get("/api/status").json()["stop"].is_null() {
                assert!(Instant::now() < deadline, "actual deterministic guest runs");
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            let before = rig.get("/api/status").json()["mouse"].clone();
            let queued = rig.journal().iter().filter(|entry| entry["type"] == "input_queued").count();
            for args in [json!({"action":"move","x":100,"y":80}),
                json!({"action":"click","x":100,"y":80,"hold_ms":20}),
                json!({"action":"down","lease_ms":30000}),
                json!({"action":"drag","path":[{"x":100,"y":80},{"x":120,"y":80}]})] {
                let mut args = args;
                args["picture"] = picture.clone();
                let mut request = CallToolRequestParams::new("mouse");
                request.arguments = Some(args.as_object().unwrap().clone());
                let refused = client.call_tool(request.clone()).await.unwrap();
                assert_eq!(refused.is_error, Some(true), "native mouse preserves running deterministic pause policy");
                assert_eq!(rig.journal().iter().filter(|entry| entry["type"] == "input_queued").count(), queued);
                assert_eq!(rig.get("/api/status").json()["mouse"], before, "refusal leaves no guest prefix");
            }
            assert_eq!(rig.post("/api/control/pause", json!({})).status, 200);
            rig.until_paused();
            let accepted = native_keyboard_accepted(&client, "mouse",
                json!({"action":"down","picture":picture,"lease_ms":30000})).await.structured_content.unwrap();
            assert_eq!(accepted["held"], true, "paused deterministic mouse input is held for resume");
            assert_eq!(rig.post("/api/control/resume", json!({"until_ms":100000})).status, 200);
            while rig.get("/api/status").json()["mouse"]["buttons"] != 1 {
                assert!(Instant::now() < deadline, "paused admission reaches the guest after resume");
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            native_keyboard_accepted(&client, "mouse",
                json!({"action":"renew","handle":accepted["hold"]["handle"],"lease_ms":30000})).await;
            native_keyboard_accepted(&client, "mouse",
                json!({"action":"up","handle":accepted["hold"]["handle"]})).await;
            while rig.get("/api/status").json()["mouse"]["buttons"] != 0 {
                assert!(Instant::now() < deadline, "up remains available while deterministic mode runs");
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            assert_eq!(rig.post("/api/control/pause", json!({})).status, 200);
            rig.until_paused();
            client.cancel().await.unwrap();
        }
    });
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_running_deterministic_input_respects_existing_pause_policy() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    let rig = Rig::with("native-deterministic-policy", Options { mcp: true, deterministic: Some(rust_dos::deterministic::TICK_MS), ..Options::default() });
    assert_eq!(rig.post("/api/input/type?wait=false", json!({"text":"LOOP\n"})).status, 200);
    assert_eq!(rig.post("/api/control/resume", json!({"until_ms":100000})).status, 200);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !rig.get("/api/status").json()["stop"].is_null() {
        assert!(Instant::now() < deadline, "actual deterministic guest is running");
        std::thread::sleep(Duration::from_millis(5));
    }
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let queued = rig.journal().iter().filter(|entry| entry["type"] == "input_queued").count();
        let mut request = CallToolRequestParams::new("type");
        request.arguments = Some(json!({"text":"dir"}).as_object().unwrap().clone());
        let refused = client.call_tool(request).await.unwrap();
        assert_eq!(refused.is_error, Some(true), "native mutation preserves deterministic paused-only admission");
        assert_eq!(rig.journal().iter().filter(|entry| entry["type"] == "input_queued").count(), queued);
        client.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_late_actor_timeout_reports_unknown_without_replaying_input() {
    use rmcp::model::ProtocolVersion;
    let gate = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let rig = Rig::with("native-unknown-acceptance", Options { mcp: true, actor_gate: Some(gate.clone()), ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let schema = client.list_all_tools().await.unwrap().into_iter().find(|tool| tool.name == "key").unwrap().output_schema.unwrap();
        let validator = jsonschema::validator_for(&Value::Object((*schema).clone())).unwrap();
        let before = rig.journal().iter().filter(|entry| entry["type"] == "input_queued").count();
        gate.store(false, std::sync::atomic::Ordering::Release);
        let mut request = rmcp::model::CallToolRequestParams::new("key");
        request.arguments = Some(json!({"action":"down","key":"ctrl","lease_ms":30000}).as_object().unwrap().clone());
        let response = client.call_tool(request).await;
        gate.store(true, std::sync::atomic::Ordering::Release);
        let response = response.unwrap();
        assert_ne!(response.is_error, Some(true));
        let value = response.structured_content.unwrap();
        assert_eq!(value["acceptance"], "unknown", "a timed-out actor reply does not prove acceptance");
        validator.validate(&value).unwrap();
        assert!(value["op"].as_u64().is_some());
        let deadline = Instant::now() + Duration::from_secs(10);
        while rig.shared.native_jobs.available_permits() != 8 {
            assert!(Instant::now() < deadline, "expired queued request retires capacity");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 0x04, 0, "no timed-out request is replayed into the guest");
        assert_eq!(rig.journal().iter().filter(|entry| entry["type"] == "input_queued").count(), before);
        client.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_settings_handoff_resets_owned_jobs_and_handles() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    let action = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let rig = Rig::with("native-settings-handoff", Options { mcp: true, frontend_action: Some(action.clone()), ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let down = native_keyboard_accepted(&client, "key", json!({"action":"down","key":"ctrl","lease_ms":30000})).await.structured_content.unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 0x04 == 0 {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(rig.post("/api/input/wait?wait=false", json!({"ms":8000})).status, 200);
        let queued = native_keyboard_accepted(&client, "key", json!({"key":"a","mods":["ctrl"]})).await.structured_content.unwrap();
        assert_eq!(rig.shared.native_jobs.available_permits(), 7);
        action.store(1, std::sync::atomic::Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(10);
        while action.load(std::sync::atomic::Ordering::Acquire) != 0 {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 0x04, 0);
        assert_eq!(rig.shared.native_jobs.available_permits(), 8, "settings handoff cancels only owned queued jobs");
        let operation = native_keyboard_accepted(&client, "operation", json!({"instance":queued["instance"],"op":queued["op"]})).await.structured_content.unwrap();
        assert_eq!(operation["delivery"], "cancelled", "settings handoff is a cancellation, not a failed delivery");
        let mut request = CallToolRequestParams::new("key");
        request.arguments = Some(json!({"action":"renew","handle":down["hold"]["handle"],"lease_ms":30000}).as_object().unwrap().clone());
        assert_eq!(client.call_tool(request).await.unwrap().is_error, Some(true), "settings handoff invalidates old hold authority");
        client.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_target_transition_cancels_pending_input_before_dispatch() {
    use rmcp::model::ProtocolVersion;
    for transition in [3, 4] {
        let action = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let rig = Rig::with(&format!("native-target-transition-{transition}"), Options {
            mcp: true, frontend_action: Some(action.clone()), ..Options::default()
        });
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        runtime.block_on(async {
            let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
            let down = native_keyboard_accepted(&client, "key", json!({"action":"down","key":"f1","lease_ms":30000})).await.structured_content.unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            while rig.get(&format!("/api/ops/{}", down["op"].as_u64().unwrap())).json()["state"] != "done" {
                assert!(Instant::now() < deadline);
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            let legacy = rig.post("/api/input/wait?wait=false", json!({"ms":8000})).json();
            let key = native_keyboard_accepted(&client, "key", json!({"key":"a"})).await.structured_content.unwrap();
            let text = native_keyboard_accepted(&client, "type", json!({"text":"dir"})).await.structured_content.unwrap();
            action.store(transition, std::sync::atomic::Ordering::Release);
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                let operation = native_keyboard_accepted(&client, "operation", json!({"instance":key["instance"],"op":key["op"]})).await.structured_content.unwrap();
                if operation["delivery"] == "cancelled" { break; }
                assert!(Instant::now() < deadline, "native input is cancelled before a target transition can dispatch it: {operation}");
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            let operation = native_keyboard_accepted(&client, "operation", json!({"instance":text["instance"],"op":text["op"]})).await.structured_content.unwrap();
            assert_eq!(operation["delivery"], "cancelled");
            let operation = native_keyboard_accepted(&client, "operation", json!({"instance":down["instance"],"op":down["op"]})).await.structured_content.unwrap();
            assert_eq!(operation["hold"]["state"], "reset");
            assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 0x04, 0);
            assert_eq!(rig.shared.native_jobs.available_permits(), 8);
            let before = rig.journal().iter().filter(|entry| entry["type"] == "input_queued").count();
            let mut request = rmcp::model::CallToolRequestParams::new("key");
            request.arguments = Some(json!({"key":"a"}).as_object().unwrap().clone());
            assert_eq!(client.call_tool(request).await.unwrap().is_error, Some(true), "new native work cannot be admitted to an unavailable guest target");
            assert_eq!(rig.journal().iter().filter(|entry| entry["type"] == "input_queued").count(), before);
            assert_eq!(rig.get(&format!("/api/ops/{}", legacy["op"].as_u64().unwrap())).json()["state"], "pending", "unrelated legacy wait remains owned by its caller");
            assert_eq!(http(rig.addr, "DELETE", "/api/input", &[], None).status, 200);
            client.cancel().await.unwrap();
        });
        rig.until_idle();
    }
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_target_transition_preserves_an_already_delivered_down() {
    use rmcp::model::ProtocolVersion;
    let action = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let steps = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let rig = Rig::with("native-delivered-transition", Options {
        mcp: true, input_steps: Some(steps.clone()), frontend_action: Some(action.clone()),
        observed_modifiers: Some(observed.clone()), ..Options::default()
    });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let down = native_keyboard_accepted(&client, "key", json!({"action":"down","key":"ctrl","lease_ms":30000})).await.structured_content.unwrap();
            steps.store(1, std::sync::atomic::Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(10);
        while observed.load(std::sync::atomic::Ordering::Acquire) & (1 << 4) == 0 {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        assert_eq!(rig.shared.native_jobs.available_permits(), 7, "the make was applied while the End marker is still queued");
        action.store(3, std::sync::atomic::Ordering::Release);
        let operation = native_keyboard_accepted(&client, "operation", json!({"instance":down["instance"],"op":down["op"]})).await.structured_content.unwrap();
        assert_eq!(operation["hold"]["state"], "reset");
        assert_eq!(operation["delivery"], "delivered", "reset must not relabel an already applied make as cancellation");
        assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 4, 0);
        assert_eq!(rig.shared.native_jobs.available_permits(), 8);
        client.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_restore_detaches_an_active_native_wait_without_break_codes() {
    use rmcp::model::ProtocolVersion;
    let action = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let rig = Rig::with("native-active-wait-restore", Options { mcp: true, frontend_action: Some(action.clone()), ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let press = native_keyboard_accepted(&client, "key", json!({"key":"ctrl","hold_ms":5000})).await.structured_content.unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 4 == 0 {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(rig.shared.native_jobs.available_permits(), 7);
        action.store(2, std::sync::atomic::Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(5);
        while action.load(std::sync::atomic::Ordering::Acquire) != 0 {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let observed = native_keyboard_accepted(&client, "operation", json!({"instance":press["instance"],"op":press["op"]})).await.structured_content.unwrap();
        assert_eq!(observed["delivery"], "cancelled");
        assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 4, 4, "the restored guest keeps its Ctrl state");
        assert_eq!(rig.shared.native_jobs.available_permits(), 8);
        let typed = native_keyboard_accepted(&client, "type", json!({"text":"é"})).await.structured_content.unwrap();
        let deadline = Instant::now() + Duration::from_millis(500);
        while rig.get(&format!("/api/ops/{}", typed["op"].as_u64().unwrap())).json()["state"] != "done" {
            assert!(Instant::now() < deadline, "the removed native timer cannot delay new input after restore");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 4, 4);
        client.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_admin_clear_preserves_an_already_delivered_down() {
    use rmcp::model::ProtocolVersion;
    let observed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let steps = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let rig = Rig::with("native-delivered-admin-clear", Options {
        mcp: true, input_steps: Some(steps.clone()), observed_modifiers: Some(observed.clone()), ..Options::default()
    });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let down = native_keyboard_accepted(&client, "key", json!({"action":"down","key":"ctrl","lease_ms":30000})).await.structured_content.unwrap();
            steps.store(1, std::sync::atomic::Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(5);
        while observed.load(std::sync::atomic::Ordering::Acquire) & (1 << 4) == 0 {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        assert_eq!(rig.shared.native_jobs.available_permits(), 7, "actual make applied before the End marker");
        let cleared = http(rig.addr, "DELETE", "/api/input", &[], None);
        assert_eq!(cleared.status, 200);
        let operation = native_keyboard_accepted(&client, "operation", json!({"instance":down["instance"],"op":down["op"]})).await.structured_content.unwrap();
        assert_eq!(operation["delivery"], "delivered", "admin clear cannot undo an already applied down");
        assert_eq!(operation["hold"]["state"], "reset");
        assert_eq!(cleared.json()["cancelled"], json!([]), "the delivered down is not listed as cancelled");
        assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 4, 0);
        assert_eq!(rig.shared.native_jobs.available_permits(), 8);
        client.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_guest_ctrl_alt_delete_invalidates_old_hold_authority() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    let power = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let rig = Rig::with("native-guest-ctrl-alt-delete", Options { mcp: true, observed_power_ons: Some(power.clone()), ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2025_11_25).await;
        let down = native_keyboard_accepted(&client, "key", json!({"action":"down","key":"ctrl","mods":["alt"],"lease_ms":30000})).await.structured_content.unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while rig.get(&format!("/api/ops/{}", down["op"].as_u64().unwrap())).json()["state"] != "done" {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let before = power.load(std::sync::atomic::Ordering::Acquire);
        native_keyboard_accepted(&client, "key", json!({"key":"delete","mods":["ctrl","alt"]})).await;
        while power.load(std::sync::atomic::Ordering::Acquire) == before {
            assert!(Instant::now() < deadline, "real guest Ctrl+Alt+Delete reboots through the BIOS");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let held = native_keyboard_accepted(&client, "operation", json!({"instance":down["instance"],"op":down["op"]})).await.structured_content.unwrap();
        assert_eq!(held["hold"]["state"], "reset", "guest-triggered reset uses the same ownership invalidation boundary");
        assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 12, 0);
        let fresh = native_keyboard_accepted(&client, "key", json!({"action":"down","key":"ctrl","lease_ms":30000})).await.structured_content.unwrap();
        while native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 4 == 0 {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let mut request = CallToolRequestParams::new("key");
        request.arguments = Some(json!({"action":"up","handle":down["hold"]["handle"]}).as_object().unwrap().clone());
        assert_eq!(client.call_tool(request).await.unwrap().is_error, Some(true));
        assert_ne!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 4, 0, "old authority cannot release a new same-key owner after reboot");
        native_keyboard_accepted(&client, "key", json!({"action":"up","handle":fresh["hold"]["handle"]})).await;
        assert_eq!(rig.shared.native_jobs.available_permits(), 8);
        client.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_bios_reboot_forgets_legacy_physical_modifiers() {
    use rmcp::model::ProtocolVersion;
    let power = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let word = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let rig = Rig::with("native-reboot-legacy-modifier", Options { mcp: true, observed_power_ons: Some(power.clone()), observed_keyboard_word: Some(word.clone()), ..Options::default() });
    assert_eq!(rig.post("/api/input/key?wait=true", json!({"key":"ctrl","action":"down"})).status, 200);
    assert_ne!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 4, 0);
    let before = power.load(std::sync::atomic::Ordering::Acquire);
    assert_eq!(rig.post("/api/control/reboot", json!({})).status, 200);
    let deadline = Instant::now() + Duration::from_secs(10);
    while power.load(std::sync::atomic::Ordering::Acquire) == before {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 4, 0, "the actual reboot cleared guest Ctrl");
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let enter = native_keyboard_accepted(&client, "key", json!({"key":"enter"})).await.structured_content.unwrap();
        while rig.get(&format!("/api/ops/{}", enter["op"].as_u64().unwrap())).json()["state"] != "done" {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        for (args, expected) in [(json!({"key":"a","mods":["ctrl"]}), 0x1e01u16), (json!({"key":"a"}), 0x1e61u16)] {
            let result = native_keyboard_accepted(&client, "key", args).await.structured_content.unwrap();
            while rig.get(&format!("/api/ops/{}", result["op"].as_u64().unwrap())).json()["state"] != "done" {
                assert!(Instant::now() < deadline);
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            assert_eq!(word.load(std::sync::atomic::Ordering::Acquire), usize::from(expected), "native input cannot borrow a legacy modifier that vanished on reboot");
            assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 4, 0);
        }
        client.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_bios_reboot_invalidates_old_holds_and_queued_input() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    let power = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let rig = Rig::with("native-bios-reboot", Options { mcp: true, observed_power_ons: Some(power.clone()), ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let down = native_keyboard_accepted(&client, "key", json!({"action":"down","key":"ctrl","lease_ms":30000})).await.structured_content.unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while rig.get(&format!("/api/ops/{}", down["op"].as_u64().unwrap())).json()["state"] != "done" {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let before = power.load(std::sync::atomic::Ordering::Acquire);
        assert_eq!(rig.post("/api/input/wait?wait=false", json!({"ms":60000})).status, 200);
        let queued = native_keyboard_accepted(&client, "key", json!({"key":"a","mods":["ctrl"]})).await.structured_content.unwrap();
        assert_eq!(rig.post("/api/control/reboot", json!({})).status, 200);
        while power.load(std::sync::atomic::Ordering::Acquire) == before {
            assert!(Instant::now() < deadline, "actual BIOS reboot increments the machine power counter");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let operation = native_keyboard_accepted(&client, "operation", json!({"instance":queued["instance"],"op":queued["op"]})).await.structured_content.unwrap();
        assert_eq!(operation["delivery"], "cancelled", "BIOS reboot detaches native work queued for the old guest");
        let held = native_keyboard_accepted(&client, "operation", json!({"instance":down["instance"],"op":down["op"]})).await.structured_content.unwrap();
        assert_eq!(held["delivery"], "delivered");
        assert_eq!(held["hold"]["state"], "reset");
        for args in [json!({"action":"renew","handle":down["hold"]["handle"],"lease_ms":30000}), json!({"action":"up","handle":down["hold"]["handle"]})] {
            let mut request = CallToolRequestParams::new("key");
            request.arguments = Some(args.as_object().unwrap().clone());
            assert_eq!(client.call_tool(request).await.unwrap().is_error, Some(true), "old authority cannot reach the rebooted guest");
        }
        assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 4, 0, "rebooted guest receives no stale owned input");
        assert_eq!(rig.shared.native_jobs.available_permits(), 8);
        assert_eq!(http(rig.addr, "DELETE", "/api/input", &[], None).status, 200);
        client.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_restore_preserves_delivery_before_the_hold_end_marker() {
    use rmcp::model::ProtocolVersion;
    for strict in [false, true] {
        let action = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let steps = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let rig = Rig::with(if strict { "native-delivered-strict-restore" } else { "native-delivered-frontend-restore" }, Options {
            mcp: true, input_steps: Some(steps.clone()), frontend_action: Some(action.clone()),
            observed_modifiers: Some(observed.clone()), ..Options::default()
        });
        assert_eq!(rig.post("/api/control/pause", json!({})).status, 200);
        rig.until_paused();
        assert_eq!(rig.post("/api/state/save", json!({"path":"delivery.state","strict_match":true})).status, 200);
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        runtime.block_on(async {
            let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
            let down = native_keyboard_accepted(&client, "key", json!({"action":"down","key":"ctrl","lease_ms":30000})).await.structured_content.unwrap();
            steps.store(1, std::sync::atomic::Ordering::Release);
            let deadline = Instant::now() + Duration::from_secs(10);
            while observed.load(std::sync::atomic::Ordering::Acquire) & (1 << 4) == 0 {
                assert!(Instant::now() < deadline);
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
            assert_eq!(rig.shared.native_jobs.available_permits(), 7, "actual make observed while its End is still pending");
            if strict {
                assert_eq!(rig.post("/api/state/load", json!({"path":"delivery.state","strict_match":true})).status, 200);
            } else {
                action.store(2, std::sync::atomic::Ordering::Release);
                while action.load(std::sync::atomic::Ordering::Acquire) != 0 {
                    assert!(Instant::now() < deadline);
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            }
            let operation = native_keyboard_accepted(&client, "operation", json!({"instance":down["instance"],"op":down["op"]})).await.structured_content.unwrap();
            assert_eq!(operation["delivery"], "delivered", "restore preserves a key already delivered before its End marker");
            assert_eq!(operation["hold"]["state"], "reset");
            assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 0x04, if strict { 0 } else { 4 }, "restored keyboard receives no old-owner break code");
            assert_eq!(rig.shared.native_jobs.available_permits(), 8);
            client.cancel().await.unwrap();
        });
        rig.until_idle();
        assert_eq!(super::super::mcp::metrics::snapshot(&rig.shared).unwrap()["input"], json!({"accepted":1,"completed":1,"failed":0}), "delivered hold must complete metrics before its pending End is retired");
    }
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_frontend_restore_hook_detaches_old_keys_without_break_codes() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    let action = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let rig = Rig::with("native-frontend-restore", Options { mcp: true, frontend_action: Some(action.clone()), ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let down = native_keyboard_accepted(&client, "key", json!({"action":"down","key":"ctrl","lease_ms":1000})).await.structured_content.unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 0x04 == 0 {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(rig.post("/api/input/wait?wait=false", json!({"ms":8000})).status, 200);
        let queued = native_keyboard_accepted(&client, "key", json!({"key":"a","mods":["ctrl"]})).await.structured_content.unwrap();
        let restored_flags = native_keyboard_bytes(&rig, "0040:0017", 1);
        action.store(2, std::sync::atomic::Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(10);
        while action.load(std::sync::atomic::Ordering::Acquire) != 0 {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1), restored_flags);
        assert_eq!(rig.shared.native_jobs.available_permits(), 8);
        let operation = native_keyboard_accepted(&client, "operation", json!({"instance":queued["instance"],"op":queued["op"]})).await.structured_content.unwrap();
        assert_eq!(operation["delivery"], "cancelled");
        for args in [json!({"action":"up","handle":down["hold"]["handle"]}), json!({"action":"renew","handle":down["hold"]["handle"],"lease_ms":1000})] {
            let mut request = CallToolRequestParams::new("key");
            request.arguments = Some(args.as_object().unwrap().clone());
            assert_eq!(client.call_tool(request).await.unwrap().is_error, Some(true));
        }
        tokio::time::sleep(Duration::from_millis(1100)).await;
        assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1), restored_flags, "old expiry must not release keys in restored CPU state");
        client.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[test]
fn debug_hub_disabled_input_ownership_has_no_heap_reservations() {
    let hub = DebugHub::disabled();
    assert_eq!(hub.input.capacity(), 0);
    #[cfg(feature = "mcp")]
    {
        assert_eq!(hub.owned_input.holds.capacity(), 0);
        assert_eq!(hub.owned_input.ended.capacity(), 0);
        assert_eq!(hub.owned_input.spans.capacity(), 0);
        assert_eq!(hub.owned_input.controls.capacity(), 0);
        println!("disabled owned input inline size={} bytes", std::mem::size_of_val(&hub.owned_input));
    }
    println!("disabled input ownership: compiled_mcp={}, DebugHub={} bytes, reserved_queue_capacity=0", cfg!(feature = "mcp"), std::mem::size_of::<DebugHub>());
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_maximum_text_jobs_remain_bounded_and_clear_reclaims_capacity() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    let capacity = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let rig = Rig::with("native-maximum-text-jobs", Options { mcp: true, deterministic: Some(rust_dos::deterministic::TICK_MS), observed_input_capacity: Some(capacity.clone()), typing_layout: Some(rust_dos::keylayout::Layout::by_code("it").unwrap()), ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let before = native_keyboard_bytes(&rig, "0040:0017", 1);
        let mut operations = Vec::new();
        for _ in 0..8 {
            let accepted = native_keyboard_accepted(&client, "type", json!({"text":"{".repeat(4096),"delay_ms":5000})).await.structured_content.unwrap();
            assert_eq!(accepted["held"], true);
            operations.push(accepted["op"].as_u64().unwrap());
        }
        let queued = native_queued_input(&rig);
        assert_eq!(queued, 8 * 4096 * 7, "the measured Italian brace expansion exercises both implicit modifiers and maximum delay");
        let allocated = capacity.load(std::sync::atomic::Ordering::Acquire);
        assert!(allocated >= queued as usize && allocated <= 2 * 8 * 28673, "actual queue capacity: {allocated}");
        assert_eq!(rig.shared.native_jobs.available_permits(), 0);
        let mut extra = CallToolRequestParams::new("type");
        extra.arguments = Some(json!({"text":"dir"}).as_object().unwrap().clone());
        assert_eq!(client.call_tool(extra).await.unwrap().is_error, Some(true));
        assert_eq!(native_queued_input(&rig), queued);
        assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1), before, "paused admission must not emit Shift makes");
        println!("maximum text admission: queued={queued}, LowInput={} bytes, logical payload={} bytes", std::mem::size_of::<super::super::LowInput>(), queued * std::mem::size_of::<super::super::LowInput>() as u64);
        println!("actual queue allocation: capacity={allocated}, bytes={}", allocated * std::mem::size_of::<super::super::LowInput>());
        assert_eq!(http(rig.addr, "DELETE", "/api/input", &[], None).status, 200);
        assert_eq!(rig.get("/api/status").json()["quiescent"], true);
        assert_eq!(rig.shared.native_jobs.available_permits(), 8);
        for operation in operations {
            assert_eq!(rig.get(&format!("/api/ops/{operation}")).json()["state"], "cancelled");
        }
        assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1), before);
        client.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_input_respects_shared_pending_operation_capacity() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    let rig = Rig::with("native-shared-pending-limit", Options { mcp: true, deterministic: Some(rust_dos::deterministic::TICK_MS), ..Options::default() });
    for _ in 0..super::super::journal::PENDING_INPUT_CAPACITY {
        assert_eq!(rig.post("/api/input/wait?wait=false", json!({"ms":1})).status, 200);
    }
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let before = rig.journal().iter().filter(|entry| entry["type"] == "input_queued").count();
        let mut request = CallToolRequestParams::new("key");
        request.arguments = Some(json!({"key":"a"}).as_object().unwrap().clone());
        assert_eq!(client.call_tool(request).await.unwrap().is_error, Some(true), "native admission must not exceed the shared pending operation bound");
        assert_eq!(rig.shared.native_jobs.available_permits(), 8);
        assert_eq!(rig.journal().iter().filter(|entry| entry["type"] == "input_queued").count(), before);
        client.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_full_job_capacity_still_allows_targeted_hold_control() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    let rig = Rig::with("native-full-job-control", Options { mcp: true, ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let down = native_keyboard_accepted(&client, "key", json!({"action":"down","key":"ctrl","lease_ms":30000})).await.structured_content.unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 4 == 0 {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let tail = native_keyboard_bytes(&rig, "0040:001C", 2);
        assert_eq!(rig.post("/api/input/wait?wait=false", json!({"ms":8000})).status, 200);
        for _ in 0..8 {
            native_keyboard_accepted(&client, "key", json!({"key":"a","mods":["ctrl"]})).await;
        }
        assert_eq!(rig.shared.native_jobs.available_permits(), 0);
        let queued = rig.journal().iter().filter(|entry| entry["type"] == "input_queued").count();
        let mut extra = CallToolRequestParams::new("key");
        extra.arguments = Some(json!({"key":"a","mods":["ctrl"]}).as_object().unwrap().clone());
        assert_eq!(client.call_tool(extra).await.unwrap().is_error, Some(true));
        assert_eq!(rig.journal().iter().filter(|entry| entry["type"] == "input_queued").count(), queued);
        native_keyboard_accepted(&client, "key", json!({"action":"renew","handle":down["hold"]["handle"],"lease_ms":30000})).await;
        assert_ne!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 4, 0);
        native_keyboard_accepted(&client, "key", json!({"action":"up","handle":down["hold"]["handle"]})).await;
        assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 4, 0, "targeted up remains available at full job capacity");
        let deadline = Instant::now() + Duration::from_secs(10);
        while rig.shared.native_jobs.available_permits() != 8 {
            assert!(Instant::now() < deadline, "queued borrowers retire after their owned modifier was released");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(native_keyboard_bytes(&rig, "0040:001C", 2), tail, "no borrowed key is delivered after targeted modifier release");
        client.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_evicted_operation_does_not_revoke_a_live_hold() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    let rig = Rig::with("native-operation-eviction", Options { mcp: true, ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let down = native_keyboard_accepted(&client, "key", json!({"action":"down","key":"ctrl","lease_ms":30000})).await.structured_content.unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 4 == 0 {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let mut latest = Value::Null;
        for _ in 0..super::super::journal::OPS_CAPACITY {
            latest = native_keyboard_accepted(&client, "key", json!({"action":"renew","handle":down["hold"]["handle"],"lease_ms":30000})).await.structured_content.unwrap();
        }
        assert_eq!(rig.get(&format!("/api/ops/{}", down["op"].as_u64().unwrap())).status, 410);
        let mut query = CallToolRequestParams::new("operation");
        query.arguments = Some(json!({"instance":down["instance"],"op":down["op"]}).as_object().unwrap().clone());
        let evicted = client.call_tool(query).await.unwrap();
        assert_eq!(evicted.is_error, Some(true));
        assert_eq!(evicted.structured_content.as_ref().unwrap()["error"]["code"], "evicted_operation", "native operation query preserves authoritative eviction");
        let retained = native_keyboard_accepted(&client, "operation", json!({"instance":latest["instance"],"op":latest["op"]})).await.structured_content.unwrap();
        assert_eq!(retained["delivery"], "delivered");
        assert_eq!(retained["hold"]["handle"], down["hold"]["handle"]);
        assert_eq!(retained["hold"]["state"], "delivered");
        assert_ne!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 4, 0);
        native_keyboard_accepted(&client, "key", json!({"action":"up","handle":down["hold"]["handle"]})).await;
        assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 4, 0);
        client.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_expiry_after_all_downs_preserves_delivered_operation() {
    use rmcp::model::ProtocolVersion;
    let observed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let steps = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let action = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let rig = Rig::with("native-delivered-expiry", Options { mcp: true, input_steps: Some(steps.clone()), frontend_action: Some(action.clone()), observed_modifiers: Some(observed.clone()), ..Options::default() });
    assert_eq!(rig.post("/api/control/run", json!({"command":"LOOP.COM"})).status, 200);
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let down = native_keyboard_accepted(&client, "key", json!({"action":"down","key":"a","mods":["ctrl"],"lease_ms":30000})).await.structured_content.unwrap();
        steps.store(2, std::sync::atomic::Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(10);
        while steps.load(std::sync::atomic::Ordering::Acquire) != 0 {
            assert!(Instant::now() < deadline, "two actual make events must retire before forced lease expiry");
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        while native_keyboard_bytes(&rig, "0040:001E", 2) != [1, 0x1e] {
            assert!(Instant::now() < deadline, "the real BIOS must consume Ctrl+A before expiry");
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        action.store(8, std::sync::atomic::Ordering::Release);
        loop {
            let expired = native_keyboard_accepted(&client, "operation", json!({"instance":down["instance"],"op":down["op"]})).await.structured_content.unwrap();
            if expired["hold"]["state"] == "expired" { break; }
            assert!(Instant::now() < deadline, "actual expiry must retire the hold");
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        assert_ne!(observed.load(std::sync::atomic::Ordering::Acquire) & (1 << 4), 0, "the real Ctrl make reached the guest");
        assert_eq!(native_keyboard_bytes(&rig, "0040:001E", 2), [1, 0x1e], "the real Ctrl+A make reached the BIOS");
        assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 4, 0);
        let operation = native_keyboard_accepted(&client, "operation", json!({"instance":down["instance"],"op":down["op"]})).await.structured_content.unwrap();
        assert_eq!(operation["hold"]["state"], "expired");
        assert_eq!(operation["delivery"], "delivered", "expiry after every make must not relabel actual delivery as cancellation");
        assert_eq!(rig.shared.native_jobs.available_permits(), 8);
        client.cancel().await.unwrap();
    });
    rig.until_idle();
        assert_eq!(super::super::mcp::metrics::snapshot(&rig.shared).unwrap()["input"], json!({"accepted":1,"completed":1,"failed":0}), "delivered hold must complete metrics before its pending End is retired");
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_partial_down_expiry_releases_only_delivered_modifiers() {
    use rmcp::model::ProtocolVersion;
    let observed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let steps = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let action = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let rig = Rig::with("native-partial-expiry", Options { mcp: true, input_steps: Some(steps.clone()), frontend_action: Some(action.clone()), observed_modifiers: Some(observed.clone()), ..Options::default() });
    assert_eq!(rig.post("/api/control/run", json!({"command":"LOOP.COM"})).status, 200);
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let tail = native_keyboard_bytes(&rig, "0040:001C", 2);
        let down = native_keyboard_accepted(&client, "key", json!({"action":"down","key":"a","mods":["ctrl","shift"],"lease_ms":30000})).await.structured_content.unwrap();
        steps.store(2, std::sync::atomic::Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(10);
        while steps.load(std::sync::atomic::Ordering::Acquire) != 0 {
            assert!(Instant::now() < deadline, "two actual make events must retire before forced lease expiry");
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        action.store(8, std::sync::atomic::Ordering::Release);
        loop {
            let expired = native_keyboard_accepted(&client, "operation", json!({"instance":down["instance"],"op":down["op"]})).await.structured_content.unwrap();
            if expired["hold"]["state"] == "expired" { break; }
            assert!(Instant::now() < deadline, "actual expiry must retire the hold");
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        let seen = observed.load(std::sync::atomic::Ordering::Acquire);
        assert_ne!(seen & (1 << 4), 0, "the real Ctrl make was observed");
        assert_ne!(seen & (1 << 6), 0, "the real Shift make was observed before expiry");
        assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 6, 0);
        assert_eq!(native_keyboard_bytes(&rig, "0040:001C", 2), tail, "the queued letter never reached the real BIOS");
        let operation = native_keyboard_accepted(&client, "operation", json!({"instance":down["instance"],"op":down["op"]})).await.structured_content.unwrap();
        assert_eq!(operation["hold"]["state"], "expired");
        assert_eq!(operation["delivery"], "cancelled");
        assert_eq!(rig.shared.native_jobs.available_permits(), 8);
        client.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_ended_hold_errors_distinguish_expired_and_released() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    let rig = Rig::with("native-ended-errors", Options { mcp: true, ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let expired = native_keyboard_accepted(&client, "key", json!({"action":"down","key":"ctrl","lease_ms":50})).await.structured_content.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        for action in ["up", "renew"] {
            let mut args = json!({"action":action,"handle":expired["hold"]["handle"]});
            if action == "renew" { args["lease_ms"] = 30000.into(); }
            let mut request = CallToolRequestParams::new("key");
            request.arguments = Some(args.as_object().unwrap().clone());
            let result = client.call_tool(request).await.unwrap();
            assert_eq!(result.is_error, Some(true));
            assert_eq!(result.structured_content.as_ref().unwrap()["error"]["code"], "expired_hold", "expired authority has its declared typed error");
        }
        let released = native_keyboard_accepted(&client, "key", json!({"action":"down","key":"ctrl","lease_ms":30000})).await.structured_content.unwrap();
        native_keyboard_accepted(&client, "key", json!({"action":"up","handle":released["hold"]["handle"]})).await;
        native_keyboard_accepted(&client, "key", json!({"action":"up","handle":released["hold"]["handle"]})).await;
        let mut renew = CallToolRequestParams::new("key");
        renew.arguments = Some(json!({"action":"renew","handle":released["hold"]["handle"],"lease_ms":30000}).as_object().unwrap().clone());
        let result = client.call_tool(renew).await.unwrap();
        assert_eq!(result.is_error, Some(true));
        assert_eq!(result.structured_content.as_ref().unwrap()["error"]["code"], "released_hold", "released authority has its declared typed error");
        assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 4, 0);
        client.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_mouse_respects_key_hold_capacity_and_projected_legacy_buttons() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    let rig = Rig::with(
        "native-mouse-capacity-projection",
        Options {
            mcp: true,
            ..Options::default()
        },
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let picture = native_keyboard_accepted(&client, "capture", json!({}))
            .await
            .structured_content
            .unwrap()["mouse_picture"]
            .clone();
        let mut holds = Vec::new();
        for key in ["f1", "f2", "f3", "f4", "f5", "f6", "f7", "f8"] {
            let down = native_keyboard_accepted(
                &client,
                "key",
                json!({"action":"down","key":key,"lease_ms":30000}),
            )
            .await
            .structured_content
            .unwrap();
            holds.push(down["hold"]["handle"].clone());
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while rig.shared.native_jobs.available_permits() != 8 {
            assert!(
                Instant::now() < deadline,
                "delivered keys release job permits while keeping their claims"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let mut request = CallToolRequestParams::new("mouse");
        request.arguments = Some(
            json!({"action":"down","picture":picture,"lease_ms":30000})
                .as_object()
                .unwrap()
                .clone(),
        );
        assert_eq!(
            client.call_tool(request).await.unwrap().is_error,
            Some(true),
            "mouse claims share the eight-hold bound with delivered keyboard claims"
        );
        assert_eq!(rig.get("/api/status").json()["mouse"]["buttons"], 0);
        for handle in holds {
            native_keyboard_accepted(&client, "key", json!({"action":"up","handle":handle}))
                .await;
        }
        rig.until_idle();
        assert_eq!(
            rig.post("/api/input/wait?wait=false", json!({"ms":400}))
                .status,
            200
        );
        assert_eq!(
            rig.post(
                "/api/input/batch?wait=false",
                json!({"events":[
            {"type":"mouse","action":"down","button":"right"},
            {"type":"mouse","action":"up","button":"right"}]})
            )
            .status,
            200
        );
        let down = native_keyboard_accepted(
            &client,
            "mouse",
            json!({"action":"down","picture":picture,"button":"right","lease_ms":30000}),
        )
        .await
        .structured_content
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while rig.get("/api/status").json()["mouse"]["buttons"] != 2 {
            assert!(
                Instant::now() < deadline,
                "queued legacy down/up balances before the native right down"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        native_keyboard_accepted(
            &client,
            "mouse",
            json!({"action":"up","handle":down["hold"]["handle"]}),
        )
        .await;
        assert_eq!(rig.get("/api/status").json()["mouse"]["buttons"], 0);
        assert_eq!(
            rig.post("/api/input/wait?wait=false", json!({"ms":400}))
                .status,
            200
        );
        assert_eq!(
            rig.post(
                "/api/input/mouse?wait=false",
                json!({"action":"down","button":"left"})
            )
            .status,
            200
        );
        let mut request = CallToolRequestParams::new("mouse");
        request.arguments = Some(
            json!({"action":"down","picture":picture,"lease_ms":30000})
                .as_object()
                .unwrap()
                .clone(),
        );
        assert_eq!(
            client.call_tool(request).await.unwrap().is_error,
            Some(true),
            "a queued legacy down reserves its button before actual delivery"
        );
        assert_eq!(
            http(rig.addr, "DELETE", "/api/input", &[], None).status,
            200
        );
        client.cancel().await.unwrap();
    });
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_mouse_owners_and_queued_spans_reject_foreign_motion() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    let rig = Rig::with(
        "native-mouse-owner-reservations",
        Options {
            mcp: true,
            ..Options::default()
        },
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let first = native_keyboard_client(&rig, ProtocolVersion::V_2025_11_25).await;
        let second = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let picture = native_keyboard_accepted(&first, "capture", json!({})).await
            .structured_content.unwrap()["mouse_picture"].clone();
        let left = native_keyboard_accepted(&first, "mouse",
            json!({"action":"down","picture":picture,"lease_ms":30000})).await.structured_content.unwrap();
        rig.until_idle();
        assert_eq!(rig.get("/api/status").json()["mouse"]["buttons"], 1);
        let queued = native_queued_input(&rig);
        for args in [json!({"action":"move","picture":picture,"x":100,"y":80}),
            json!({"action":"click","picture":picture,"button":"right","x":100,"y":80}),
            json!({"action":"down","picture":picture,"lease_ms":30000})] {
            let mut request = CallToolRequestParams::new("mouse");
            request.arguments = Some(args.as_object().unwrap().clone());
            let rejected = second.call_tool(request).await.unwrap();
            assert_eq!(rejected.is_error, Some(true));
            assert_eq!(rejected.structured_content.unwrap()["error"]["code"], "input_conflict");
            assert_eq!(native_queued_input(&rig), queued);
            assert_eq!(rig.get("/api/status").json()["mouse"]["buttons"], 1);
        }
        let right = native_keyboard_accepted(&second, "mouse",
            json!({"action":"down","picture":picture,"button":"right","lease_ms":30000})).await.structured_content.unwrap();
        rig.until_idle();
        assert_eq!(rig.get("/api/status").json()["mouse"]["buttons"], 3);
        let mut request = CallToolRequestParams::new("mouse");
        request.arguments = Some(json!({"action":"move","picture":picture,
            "handle":right["hold"]["handle"],"x":120,"y":80}).as_object().unwrap().clone());
        assert_eq!(second.call_tool(request).await.unwrap().is_error, Some(true),
            "one handle cannot authorize movement affecting another button claim");
        native_keyboard_accepted(&second, "mouse", json!({"action":"up","handle":right["hold"]["handle"]})).await;
        assert_eq!(rig.get("/api/status").json()["mouse"]["buttons"], 1);
        assert_eq!(rig.post("/api/input/wait?wait=false", json!({"ms":400})).status, 200);
        native_keyboard_accepted(&first, "mouse",
            json!({"action":"move","picture":picture,"handle":left["hold"]["handle"],"x":100,"y":80})).await;
        native_keyboard_accepted(&first, "mouse", json!({"action":"up","handle":left["hold"]["handle"]})).await;
        assert_eq!(rig.get("/api/status").json()["mouse"]["buttons"], 0);
        assert_eq!(http(rig.addr, "DELETE", "/api/input", &[], None).status, 200);
        assert_eq!(rig.post("/api/input/wait?wait=false", json!({"ms":400})).status, 200);
        native_keyboard_accepted(&first, "mouse", json!({"action":"move","picture":picture,"x":40,"y":40})).await;
        let queued = native_queued_input(&rig);
        let mut request = CallToolRequestParams::new("mouse");
        request.arguments = Some(json!({"action":"move","picture":picture,"x":60,"y":40}).as_object().unwrap().clone());
        assert_eq!(second.call_tool(request).await.unwrap().is_error, Some(true),
            "queued pointer motion reserves the pointer before any button is held");
        assert_eq!(native_queued_input(&rig), queued);
        assert_eq!(http(rig.addr, "DELETE", "/api/input", &[], None).status, 200);
        first.cancel().await.unwrap();
        second.cancel().await.unwrap();
    });
}

#[cfg(feature = "mcp")]
#[test]
fn native_mouse_port_rejects_unbounded_gestures_before_any_guest_effect() {
    use rmcp::model::ProtocolVersion;
    use rust_dos::control::{
        GuestInput, GuestInputService, InputError, MouseButton, MouseDragStep, MouseInput,
        MouseMotion, MousePicture, PictureSize,
    };
    let rig = Rig::with(
        "native-mouse-port-validation",
        Options {
            mcp: true,
            ..Options::default()
        },
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let observed = native_keyboard_accepted(&client, "capture", json!({}))
            .await
            .structured_content
            .unwrap()["mouse_picture"]
            .clone();
        let picture = MousePicture {
            instance: observed["instance"].as_str().unwrap().into(),
            mode_epoch: observed["mode_epoch"].as_u64().unwrap(),
            size: PictureSize {
                width: observed["width"].as_u64().unwrap().try_into().unwrap(),
                height: observed["height"].as_u64().unwrap().try_into().unwrap(),
            },
        };
        let motion = MouseMotion::Absolute { x: 10, y: 20 };
        let step = MouseDragStep {
            motion,
            delay: Duration::ZERO,
        };
        let mut invalid = Vec::new();
        for lease in [Duration::ZERO, Duration::from_millis(30001)] {
            invalid.push(MouseInput::Down {
                picture: picture.clone(),
                motion: None,
                button: MouseButton::Left,
                lease,
            });
        }
        for duration in [Duration::ZERO, Duration::from_millis(5001)] {
            invalid.push(MouseInput::Click {
                picture: picture.clone(),
                motion: None,
                button: MouseButton::Left,
                duration,
            });
        }
        for motion in [
            MouseMotion::Absolute { x: -1, y: 20 },
            MouseMotion::Absolute { x: 10, y: -1 },
            MouseMotion::Absolute {
                x: picture.size.width as i32,
                y: 20,
            },
            MouseMotion::Absolute {
                x: 10,
                y: picture.size.height as i32,
            },
            MouseMotion::Relative { dx: 32768, dy: 0 },
            MouseMotion::Relative {
                dx: 0,
                dy: i32::MIN,
            },
        ] {
            invalid.push(MouseInput::Move {
                picture: picture.clone(),
                motion,
                handle: None,
            });
        }
        for path in [
            vec![],
            vec![step.clone(); 257],
            vec![
                step.clone(),
                MouseDragStep {
                    motion: MouseMotion::Relative { dx: 1, dy: 1 },
                    delay: Duration::ZERO,
                },
            ],
            vec![
                MouseDragStep {
                    delay: Duration::from_millis(3000),
                    ..step.clone()
                },
                MouseDragStep {
                    delay: Duration::from_millis(2001),
                    ..step.clone()
                },
            ],
            vec![
                MouseDragStep {
                    delay: Duration::from_millis(1),
                    ..step.clone()
                },
                MouseDragStep {
                    delay: Duration::MAX,
                    ..step.clone()
                },
            ],
            vec![
                step.clone(),
                MouseDragStep {
                    motion: MouseMotion::Absolute { x: 10, y: -1 },
                    delay: Duration::ZERO,
                },
            ],
        ] {
            invalid.push(MouseInput::Drag {
                picture: picture.clone(),
                button: MouseButton::Left,
                path,
            });
        }
        let before = rig.get("/api/status").json()["mouse"].clone();
        let queued = rig
            .journal()
            .iter()
            .filter(|entry| entry["type"] == "input_queued")
            .count();
        for input in invalid {
            let result = rig
                .client
                .inputs(Instant::now() + Duration::from_secs(1))
                .input(GuestInput::Mouse(input))
                .await;
            assert!(
                matches!(result, Err(InputError::Invalid(_))), "typed mouse port validates independently of MCP: {result:?}");
        assert_eq!(
                rig.journal()
                    .iter()
                    .filter(|entry| entry["type"] == "input_queued")
                    .count(),
                queued
            );
            assert_eq!(rig.get("/api/status").json()["mouse"], before);
            assert_eq!(rig.shared.native_jobs.available_permits(), 8);
        }
        for size in [
            PictureSize {
                width: picture.size.width + 1,
                ..picture.size
            },
            PictureSize {
                height: picture.size.height + 1,
                ..picture.size
            },
        ] {
            let result = rig
                .client
                .inputs(Instant::now() + Duration::from_secs(1))
                .input(GuestInput::Mouse(MouseInput::Move {
                    picture: MousePicture {
                        size,
                        ..picture.clone()
                    },
                    motion,
                    handle: None,
                }))
                .await;
            assert!(matches!(result, Err(InputError::StalePicture)));
            assert_eq!(rig.get("/api/status").json()["mouse"], before);
        }
        let key = native_keyboard_accepted(
            &client,
            "key",
            json!({"action":"down","key":"ctrl","lease_ms":30000}),
        )
        .await
        .structured_content
        .unwrap();
        rig.until_idle();
        let mut request = rmcp::model::CallToolRequestParams::new("mouse");
        request.arguments = Some(
            json!({"action":"move","picture":observed,
            "handle":key["hold"]["handle"],"x":10,"y":20})
            .as_object()
            .unwrap()
            .clone(),
        );
        let rejected = client.call_tool(request).await.unwrap();
        assert_eq!(
            rejected.is_error,
            Some(true),
            "a keyboard authority cannot authorize mouse motion"
        );
        assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 4, 4);
        native_keyboard_accepted(
            &client,
            "key",
            json!({"action":"up","handle":key["hold"]["handle"]}),
        )
        .await;
        let mouse = native_keyboard_accepted(
            &client,
            "mouse",
            json!({"action":"down","picture":observed,"lease_ms":30000}),
        ).await.structured_content
        .unwrap();
        native_keyboard_accepted(
            &client,
            "mouse",
            json!({"action":"up","handle":mouse["hold"]["handle"]}),
        )
        .await;
        let mut request = rmcp::model::CallToolRequestParams::new("mouse");
        request.arguments = Some(
            json!({"action":"move","picture":observed,
            "handle":mouse["hold"]["handle"],"x":10,"y":20})
            .as_object()
            .unwrap()
            .clone(),
        );
        assert_eq!(
            client.call_tool(request).await.unwrap().is_error,
            Some(true),
            "released mouse authority cannot authorize later motion"
        );
        assert_eq!(rig.get("/api/status").json()["mouse"]["buttons"], 0);
        client.cancel().await.unwrap();
});
}

#[cfg(feature = "mcp")]
#[test]
fn native_input_port_rejects_noncanonical_keys_and_unbounded_delay_before_effects() {
    use rust_dos::control::{GuestInput, GuestInputService, InputError, InputOutcome, KeyChord};
    let rig = Rig::with("native-port-validation", Options { mcp: true, ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let plain = rust_dos::keyboard::lookup("a").unwrap();
    let ctrl = rust_dos::keyboard::lookup("ctrl").unwrap();
    let mut forged = plain;
    forged.modifier = ctrl.modifier;
    let mut forged_modifier = ctrl;
    forged_modifier.ascii = plain.ascii;
    let mut forged_shifted = plain;
    forged_shifted.shifted ^= 1;
    runtime.block_on(async {
        for input in [
            GuestInput::Press { chord: KeyChord { key: forged, modifiers: vec![] }, duration: Duration::from_millis(1) },
            GuestInput::Press { chord: KeyChord { key: forged_shifted, modifiers: vec![] }, duration: Duration::from_millis(1) },
            GuestInput::Press { chord: KeyChord { key: plain, modifiers: vec![forged_modifier] }, duration: Duration::from_millis(1) },
            GuestInput::Press { chord: KeyChord { key: plain, modifiers: vec![ctrl, ctrl] }, duration: Duration::from_millis(1) },
            GuestInput::Press { chord: KeyChord { key: ctrl, modifiers: vec![ctrl] }, duration: Duration::from_millis(1) },
            GuestInput::Press { chord: KeyChord { key: plain, modifiers: vec![ctrl; 5] }, duration: Duration::from_millis(1) },
            GuestInput::Press { chord: KeyChord { key: ctrl, modifiers: vec![plain] }, duration: Duration::from_millis(1) },
            GuestInput::Press { chord: KeyChord { key: plain, modifiers: vec![] }, duration: Duration::ZERO },
            GuestInput::Press { chord: KeyChord { key: plain, modifiers: vec![] }, duration: Duration::from_millis(5001) },
            GuestInput::Down { chord: KeyChord { key: ctrl, modifiers: vec![] }, lease: Duration::from_millis(30001) },
            GuestInput::Type { text: "dir🙂".into(), delay: Duration::ZERO },
            GuestInput::Type { text: "dir".repeat(1366), delay: Duration::ZERO },
            GuestInput::Type { text: "a".into(), delay: Duration::from_millis(5001) },
        ] {
            let queued = rig.journal().iter().filter(|entry| entry["type"] == "input_queued").count();
            let result = rig.client.inputs(Instant::now() + Duration::from_secs(1)).input(input).await;
            assert!(matches!(result, Err(InputError::Invalid(_))), "public input port enforces the canonical bounded contract: {result:?}");
            assert_eq!(rig.journal().iter().filter(|entry| entry["type"] == "input_queued").count(), queued);
            assert_eq!(rig.shared.native_jobs.available_permits(), 8);
            assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 15, 0);
        }
        let down = rig.client.inputs(Instant::now() + Duration::from_secs(1)).input(GuestInput::Down {
            chord: KeyChord { key: ctrl, modifiers: vec![] }, lease: Duration::from_secs(30)
        }).await.unwrap();
        let InputOutcome::Accepted { hold: Some(hold), .. } = down else { panic!("the actual actor returns its retained hold"); };
        let deadline = Instant::now() + Duration::from_secs(5);
        while native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 4 == 0 {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        for lease in [Duration::ZERO, Duration::from_millis(30001)] {
            let queued = rig.journal().iter().filter(|entry| entry["type"] == "input_queued").count();
            let result = rig.client.inputs(Instant::now() + Duration::from_secs(1)).input(GuestInput::Renew { handle: hold.handle.clone(), lease }).await;
            assert!(matches!(result, Err(InputError::Invalid(_))));
            assert_eq!(rig.journal().iter().filter(|entry| entry["type"] == "input_queued").count(), queued);
            assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 4, 4, "invalid port renewal preserves the actual hold");
        }
        rig.client.inputs(Instant::now() + Duration::from_secs(1)).input(GuestInput::Up { handle: hold.handle }).await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_guest_shortcuts_reach_bios_without_frontend_hotkeys() {
    use rmcp::model::ProtocolVersion;
    for (key, mods, expected) in [("s", vec!["ctrl", "shift"], [0x13, 0x1f]), ("f12", vec!["ctrl"], [0, 0x8a])] {
        let observed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let rig = Rig::with(&format!("native-guest-shortcut-{key}"), Options { mcp: true, observed_hotkeys: Some(observed.clone()), ..Options::default() });
        assert_eq!(rig.post("/api/control/run", json!({"command":"LOOP.COM"})).status, 200);
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        runtime.block_on(async {
            let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
            let down = native_keyboard_accepted(&client, "key", json!({"action":"down","key":key,"mods":mods,"lease_ms":30000})).await.structured_content.unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            while rig.get(&format!("/api/ops/{}", down["op"].as_u64().unwrap())).json()["state"] != "done" {
                assert!(Instant::now() < deadline);
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            assert_eq!(observed.load(std::sync::atomic::Ordering::Acquire), 0, "native guest delivery must not invoke frontend hotkeys");
            assert_eq!(native_keyboard_bytes(&rig, "0040:001E", 2), expected, "native key chord reached the real BIOS");
            let operation = native_keyboard_accepted(&client, "operation", json!({"instance":down["instance"],"op":down["op"]})).await.structured_content.unwrap();
            assert_eq!(operation["hold"]["state"], "delivered");
            native_keyboard_accepted(&client, "key", json!({"action":"up","handle":down["hold"]["handle"]})).await;
            assert_eq!(native_keyboard_bytes(&rig, "0040:0017", 1)[0] & 15, 0);
            client.cancel().await.unwrap();
        });
        rig.until_idle();
    }
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_metrics_observe_actual_outcomes_and_bounded_labels() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    use rust_dos::control::{GuestInput, GuestInputService, InputError};
    let rig = Rig::with("native-metrics", Options { mcp: true, ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, ProtocolVersion::V_2026_07_28).await;
        let accepted = native_keyboard_accepted(&client, "type", json!({"text":"echo MCP_METRICS\n"})).await.structured_content.unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let observed = native_keyboard_accepted(&client, "operation", json!({"instance":accepted["instance"],"op":accepted["op"]})).await.structured_content.unwrap();
            if observed["delivery"] == "delivered" { break; }
            assert!(Instant::now() < deadline, "actual input completed");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(rig.get("/api/screen/text").json()["lines"].as_array().unwrap().iter().any(|line| line.as_str().is_some_and(|line| line.contains("MCP_METRICS"))));
        let capture = client.call_tool(CallToolRequestParams::new("capture")).await.unwrap();
        assert_ne!(capture.is_error, Some(true));
        assert!(capture.content.iter().any(|content| content.as_image().is_some()));
        let mut malformed = CallToolRequestParams::new("mouse");
        malformed.arguments = Some(json!({"action":"move","x":1}).as_object().unwrap().clone());
        assert!(client.call_tool(malformed).await.is_err(), "real decoder rejects malformed input");
        let expired = rig.client.inputs(Instant::now() - Duration::from_millis(1)).input(GuestInput::Type { text: "EXPIRED_METRIC_INPUT".into(), delay: Duration::ZERO }).await;
        assert!(matches!(expired, Err(InputError::ExpiredBeforeAcceptance)), "real pre-acceptance expiry: {expired:?}");
        assert_eq!(rig.post("/api/input/wait?wait=false", json!({"ms":8000})).status, 200);
        let cancelled = native_keyboard_accepted(&client, "type", json!({"text":"CANCELLED_METRIC_INPUT"})).await.structured_content.unwrap();
        assert_eq!(http(rig.addr, "DELETE", "/api/input", &[], None).status, 200);
        let observed = native_keyboard_accepted(&client, "operation", json!({"instance":cancelled["instance"],"op":cancelled["op"]})).await.structured_content.unwrap();
        assert_eq!(observed["delivery"], "cancelled");
        client.cancel().await.unwrap();
    });
    rig.until_idle();
    let snapshot = super::super::mcp::metrics::snapshot(&rig.shared).expect("operation metrics snapshot is absent after real success/rejection/expiry/cancellation/capture calls");
    metrics_contract::assert_snapshot(&snapshot);
}

#[cfg(feature = "mcp")]
#[test]
fn transport_neutral_input_without_a_job_preserves_actual_core_delivery() {
    use rust_dos::control::{GuestInput, MouseInput, MouseMotion, ObservationService};
    let rig = Rig::with("shared-unmetered-input", Options { mcp: true, ..Options::default() });
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let context = rig.client.context(None, None);
        let answer = rig.client.submit(&context, super::super::Cmd::NativeInput {
            input: GuestInput::Type { text: "echo MCP_CORE_JOBLESS\n".into(), delay: Duration::ZERO },
            job: None,
        }, Duration::from_secs(1)).await.unwrap();
        assert!(matches!(answer.reply, super::super::Reply::Json(_)));
        let deadline = Instant::now() + Duration::from_secs(10);
        while rig.get(&format!("/api/ops/{}", context.req)).json()["state"] != "done" {
            assert!(Instant::now() < deadline, "unmetered trusted-core input delivers");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(rig.get("/api/screen/text").json()["lines"].as_array().unwrap().iter().any(|line| line.as_str().is_some_and(|line| line.contains("MCP_CORE_JOBLESS"))));
        let picture = rig.client.observations().status().await.unwrap().context.mouse_picture.unwrap();
        let mouse_context = rig.client.context(None, None);
        let mouse = rig.client.submit(&mouse_context, super::super::Cmd::NativeInput {
            input: GuestInput::Mouse(MouseInput::Move { picture, motion: MouseMotion::Absolute { x: 80, y: 40 }, handle: None }),
            job: None,
        }, Duration::from_secs(1)).await.unwrap();
        assert!(matches!(mouse.reply, super::super::Reply::Json(_)));
        let deadline = Instant::now() + Duration::from_secs(10);
        while rig.get(&format!("/api/ops/{}", mouse_context.req)).json()["state"] != "done" {
            assert!(Instant::now() < deadline, "unmetered trusted-core mouse input delivers");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(rig.get("/api/status").json()["mouse"]["x"].as_i64().unwrap() > 0);
    });
    let snapshot = super::super::mcp::metrics::snapshot(&rig.shared).unwrap();
    assert_eq!(snapshot["input"]["accepted"], 0, "only actual admitted native jobs are counted");
    assert_eq!(snapshot["input"]["completed"], 0);
    assert_eq!(snapshot["input"]["failed"], 0);
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_execution_controls_advertise_strict_contracts() {
    let rig = Rig::with("native-execution-discovery", Options { mcp: true, ..Options::default() });
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, rmcp::model::ProtocolVersion::V_2026_07_28).await;
        let tools = client.list_all_tools().await.unwrap();
        for name in ["speed", "fast_forward"] {
            let tool = tools.iter().find(|tool| tool.name == name).expect("execution control must be discoverable");
            assert_eq!(tool.input_schema["additionalProperties"], false);
            assert_eq!(tool.annotations.as_ref().unwrap().read_only_hint, Some(false));
            assert!(tool.output_schema.is_some());
        }
        client.cancel().await.unwrap();
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_execution_controls_apply_speed_and_bound_fast_forward() {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    let rig = Rig::with("native-execution-controls", Options { mcp: true, ..Options::default() });
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    runtime.block_on(async {
        for version in [ProtocolVersion::V_2025_11_25, ProtocolVersion::V_2026_07_28] {
            let client = native_keyboard_client(&rig, version).await;
            let tools = client.list_all_tools().await.unwrap();
            let validators = tools.into_iter().filter(|tool| matches!(tool.name.as_ref(), "speed" | "fast_forward")).map(|tool| (tool.name.to_string(), jsonschema::validator_for(&Value::Object((*tool.output_schema.unwrap()).clone())).unwrap())).collect::<std::collections::HashMap<_, _>>();
            for args in [json!({}), json!({"cycles":"max"}), json!({"cycles":"auto 5000"}), json!({"cycles":"20000"})] {
                let reply = native_keyboard_accepted(&client, "speed", args).await.structured_content.unwrap();
                assert!(validators["speed"].is_valid(&reply), "actual speed reply matches discovery: {reply}");
            }
            assert_eq!(rig.get("/api/status").json()["cycles_per_ms"], 20000, "the actor changed the actual guest clock rate");
            let before = rig.shared.next_req.load(std::sync::atomic::Ordering::Acquire);
            for (name, args) in [
                ("speed", json!({"cycles":null})), ("speed", json!({"cycles":20000})),
                ("speed", json!({"cycles":"99"})), ("speed", json!({"cycles":"2000001"})),
                ("speed", json!({"cycles":"invalid"})), ("speed", json!({"extra":true})),
                ("fast_forward", json!({})), ("fast_forward", json!({"action":"start"})),
                ("fast_forward", json!({"action":"start","duration_ms":0})),
                ("fast_forward", json!({"action":"start","duration_ms":5001})),
                ("fast_forward", json!({"action":"get","duration_ms":1})),
                ("fast_forward", json!({"action":"stop","duration_ms":1})),
                ("fast_forward", json!({"action":"start","duration_ms":null})),
            ] {
                let mut request = CallToolRequestParams::new(name);
                request.arguments = Some(args.as_object().unwrap().clone());
                assert!(client.call_tool(request).await.is_err(), "invalid {name} input must fail before actor submission");
            }
            assert_eq!(rig.shared.next_req.load(std::sync::atomic::Ordering::Acquire), before);
            let started = native_keyboard_accepted(&client, "fast_forward", json!({"action":"start","duration_ms":5000})).await.structured_content.unwrap();
            assert!(validators["fast_forward"].is_valid(&started));
            assert_eq!(started["enabled"], true);
            assert_eq!(started["agent_active"], true);
            assert_eq!(started["multiplier"], crate::timer::Pacer::fast_forward_multiplier());
            let mut conflict = CallToolRequestParams::new("fast_forward");
            conflict.arguments = Some(json!({"action":"start","duration_ms":5000}).as_object().unwrap().clone());
            let rejected = client.call_tool(conflict).await.unwrap();
            assert_eq!(rejected.is_error, Some(true));
            assert!(validators["fast_forward"].is_valid(rejected.structured_content.as_ref().unwrap()));
            let stopped = native_keyboard_accepted(&client, "fast_forward", json!({"action":"stop"})).await.structured_content.unwrap();
            assert_eq!(stopped["enabled"], false);
            assert_eq!(stopped["agent_active"], false);
            native_keyboard_accepted(&client, "fast_forward", json!({"action":"start","duration_ms":25})).await;
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let state = native_keyboard_accepted(&client, "fast_forward", json!({"action":"get"})).await.structured_content.unwrap();
                assert!(validators["fast_forward"].is_valid(&state));
                if state["agent_active"] == false { assert_eq!(state["enabled"], false); break; }
                assert!(Instant::now() < deadline);
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
            native_keyboard_accepted(&client, "fast_forward", json!({"action":"start","duration_ms":5000})).await;
            assert_eq!(rig.post("/api/control/pause", json!({})).status, 200);
            rig.until_paused();
            let paused = native_keyboard_accepted(&client, "fast_forward", json!({"action":"get"})).await.structured_content.unwrap();
            assert_eq!(paused["agent_active"], false);
            assert_eq!(paused["enabled"], false);
            assert_eq!(rig.post("/api/control/resume", json!({})).status, 200);
            client.cancel().await.unwrap();
        }
    });
    rig.until_idle();
}

#[cfg(feature = "mcp")]
#[test]
fn native_mcp_execution_controls_preserve_deterministic_pause_policy() {
    let rig = Rig::with("native-execution-deterministic", Options { mcp: true, deterministic: Some(10), ..Options::default() });
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let client = native_keyboard_client(&rig, rmcp::model::ProtocolVersion::V_2026_07_28).await;
        let fixed = native_keyboard_accepted(&client, "speed", json!({"cycles":"20000"})).await.structured_content.unwrap();
        assert_eq!(fixed["cycles"], "20000");
        for (name, args) in [("speed", json!({"cycles":"max"})), ("fast_forward", json!({"action":"start","duration_ms":100}))] {
            let mut request = rmcp::model::CallToolRequestParams::new(name);
            request.arguments = Some(args.as_object().unwrap().clone());
            assert_eq!(client.call_tool(request).await.unwrap().is_error, Some(true));
        }
        assert_eq!(rig.post("/api/control/resume", json!({})).status, 200);
        let mut change = rmcp::model::CallToolRequestParams::new("speed");
        change.arguments = Some(json!({"cycles":"30000"}).as_object().unwrap().clone());
        assert_eq!(client.call_tool(change).await.unwrap().is_error, Some(true));
        let state = native_keyboard_accepted(&client, "speed", json!({})).await.structured_content.unwrap();
        assert_eq!(state["cycles"], "20000", "a running deterministic guest cannot be mutated by speed control");
        client.cancel().await.unwrap();
    });
    rig.until_idle();
}
