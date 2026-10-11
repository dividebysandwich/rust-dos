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

/// A server and the machine behind it.
struct Rig {
    addr: SocketAddr,
    emulator: Option<JoinHandle<()>>,
}

impl Rig {
    fn new(name: &str) -> Self {
        Self::with_capacity(name, super::journal::CAPACITY)
    }

    /// A rig whose journal keeps `capacity` entries.
    fn with_capacity(name: &str, capacity: usize) -> Self {
        let dir = std::env::temp_dir().join(format!("rust-dos-http-{}-{}", name, std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let code: Vec<u8> = (0..PROBE.len()).step_by(2).map(|i| u8::from_str_radix(&PROBE[i..i + 2], 16).unwrap()).collect();
        std::fs::write(dir.join("PROBE.COM"), code).unwrap();
        std::fs::write(dir.join("LOOP.COM"), LOOP).unwrap();
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared::with_capacity(Instant::now(), capacity));
        let addr = super::spawn("127.0.0.1:0".parse().unwrap(), tx, shared.clone()).unwrap();
        let emulator = std::thread::Builder::new()
            .stack_size(16 << 20)
            .spawn(move || {
                let mut cpu = Cpu::new(dir);
                cpu.load_shell();
                let hook = shared.clone();
                cpu.bus.log_hook = Some(Box::new(move |line: &str| hook.log_line(line)));
                let mut hub = DebugHub::new(Some(rx), Some(shared), 1000);
                hub.announce(&cpu);
                loop {
                    hub.poll(&mut cpu);
                    if hub.quit_requested() {
                        hub.shutdown(&cpu, ShutdownReason::Quit, 0);
                        return;
                    }
                    if !hub.paused {
                        let hot = hub.begin_batch(&cpu);
                        let end = cpu.bus.clock.icount + 5000;
                        cpu.bus.start_batch(end);
                        while cpu.bus.clock.icount < end {
                            match exec::run_batch(&mut cpu, &mut hub, hot) {
                                StopReason::BatchEnd | StopReason::Paused | StopReason::Exit => break,
                                StopReason::ShellReloaded => cpu.bus.start_batch(end),
                            }
                        }
                        hub.end_batch(&cpu);
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
            })
            .unwrap();
        let rig = Rig { addr, emulator: Some(emulator) };
        // Running, at the prompt.
        let deadline = Instant::now() + Duration::from_secs(10);
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
}

impl Drop for Rig {
    fn drop(&mut self) {
        let addr = self.addr;
        let _ = std::panic::catch_unwind(move || http(addr, "POST", "/api/control/quit", &[], Some(&json!({}))));
        if let Some(emulator) = self.emulator.take() {
            let _ = emulator.join();
        }
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
