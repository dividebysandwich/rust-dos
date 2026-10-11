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
}

impl Default for Options {
    fn default() -> Self {
        Options { capacity: super::journal::CAPACITY, deterministic: None, frame: 5000, sleep: Duration::from_millis(1), linger: Duration::ZERO }
    }
}

/// A server and the machine behind it.
struct Rig {
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
        let addr = super::spawn("127.0.0.1:0".parse().unwrap(), tx, shared.clone()).unwrap();
        let machine_shared = shared.clone();
        let emulator = std::thread::Builder::new()
            .stack_size(16 << 20)
            .spawn(move || run_machine(dir, rx, machine_shared, options))
            .unwrap();
        let rig = Rig { addr, shared, emulator: Some(emulator) };
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
    if mode.is_some() {
        cpu.bus.set_cycles_per_ms(3000);
    }
    cpu.load_shell();
    let hook = shared.clone();
    cpu.bus.log_hook = Some(Box::new(move |line: &str| hook.log_line(line)));
    let mut hub = DebugHub::new(Some(rx), Some(shared), 1000);
    hub.deterministic = mode.as_ref().map(|m| m.start);
    hub.announce(&cpu);
    if mode.is_some() {
        hub.pause_at_start(&cpu);
    }
    let mut states: std::collections::HashMap<std::path::PathBuf, Vec<u8>> = Default::default();
    loop {
        hub.poll(&mut cpu);
        if hub.quit_requested() {
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
        while let Some(request) = hub.next_speed_request() {
            let cycles = request.cycles.clone();
            hub.speed_done(&cpu, request, Ok(json!({"cycles": cycles})));
        }
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
                            StopReason::BatchEnd | StopReason::Paused | StopReason::Exit => break,
                            StopReason::ShellReloaded => cpu.bus.start_batch(end),
                        }
                    }
                }
            }
        }
        hub.end_batch(&cpu);
        std::thread::sleep(options.sleep);
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
            Ok(tungstenite::Message::Text(t)) => return Some(serde_json::from_str(t.as_str()).unwrap()),
            Ok(tungstenite::Message::Close(_)) => return None,
            Ok(_) => continue,
            Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => return None,
            Err(e) => panic!("{}", e),
        }
    }
}

/// The JSON keys of an object.
fn keys(v: &Value) -> Vec<String> {
    v.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default()
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
    // The client's ID is in the entries its request caused.
    let set = http(rig.addr, "POST", "/api/breakpoints", &[("X-Request-Id", "bp-1")], Some(&json!({"addr": "0100:0000"})));
    let entry = rig.get(&format!("/api/journal?after={}", set.json()["seq"].as_u64().unwrap() - 1)).json()["entries"][0].clone();
    assert_eq!((entry["type"].as_str(), entry["request_id"].as_str()), (Some("breakpoint_set"), Some("bp-1")));
    assert_eq!(entry["cause"], set.json()["req"]);
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
                let mut rand = Rand(0x9E37_79B9_7F4A_7C15 ^ (i + 1) * 0x1234_5678_9ABC);
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
        let queued = rig.get(&format!("/api/journal?after=0&type=input_queued")).json();
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
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => break,
            Err(e) => panic!("{}", e),
        }
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
        let done = writing.iter().all(|w| w.is_finished());
        if done && got.is_empty() {
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
    let got = read_entries(&mut socket, usize::MAX, Duration::from_millis(1000));
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
    let got = read_entries(&mut socket, usize::MAX, Duration::from_millis(1000));
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
    // The oldest ended operations are gone; the latest 1024 kept, and the
    // pending resume, however old.
    let kept = super::journal::OPS_CAPACITY;
    for &op in &inputs[..inputs.len() - kept] {
        assert_eq!(rig.get(&format!("/api/ops/{}", op)).status, 410, "{}", op);
    }
    for &op in &inputs[inputs.len() - kept..] {
        assert_eq!(rig.get(&format!("/api/ops/{}", op)).json()["state"], "done", "{}", op);
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
                let mut rand = Rand(0xD1B5_4A32_D192_ED03 ^ (i + 1) * 0x9E37_79B9);
                // (action, status, request, body, state seq header)
                let mut done: Vec<(u64, u16, u64, Value, Option<u64>)> = Vec::new();
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
    let done: Vec<Vec<(u64, u16, u64, Value, Option<u64>)>> = clients.into_iter().map(|c| c.join().expect("a client failed")).collect();
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
