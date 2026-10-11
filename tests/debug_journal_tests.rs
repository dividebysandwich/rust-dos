//! The debug server's journal in the emulator itself: the binary with no
//! window (`SDL_VIDEODRIVER=dummy`), on a free port, driven over HTTP from
//! a program's entry to `quit`. The binary needs the `sdl` feature.
#![cfg(feature = "sdl")]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// A .COM program: MOV AX,1234h; MOV BX,5678h; MOV AX,[0110h];
/// MOV AX,4C00h; INT 21h; and the word ABCDh at 0110h.
const PROBE: [u8; 18] = [
    0xB8, 0x34, 0x12, 0xBB, 0x78, 0x56, 0xA1, 0x10, 0x01, 0xB8, 0x00, 0x4C, 0xCD, 0x21, 0x00, 0x00, 0xCD, 0xAB,
];

/// A response's status and JSON body.
fn http(addr: SocketAddr, method: &str, path: &str, headers: &[(&str, &str)], body: Option<Value>) -> (u16, Value) {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    let body = body.map(|b| b.to_string()).unwrap_or_default();
    let mut request = format!("{method} {path} HTTP/1.0\r\nHost: x\r\nContent-Length: {}\r\n", body.len());
    for (k, v) in headers {
        request.push_str(&format!("{k}: {v}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(&body);
    stream.write_all(request.as_bytes()).unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    let (head, rest) = raw.split_once("\r\n\r\n").expect("a response");
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    (status, serde_json::from_str(rest).unwrap_or(Value::Null))
}

/// The emulator, killed if the test fails before it quits.
struct Emulator(Child);

impl Drop for Emulator {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The emulator with `dir`'s `c` folder as C:, and where its debug server
/// listens.
fn start(dir: &std::path::Path) -> (Emulator, SocketAddr) {
    let exe = env!("CARGO_BIN_EXE_rust-dos");
    // What the emulator says on stderr, shown if it doesn't come up.
    let stderr_path = dir.join("stderr.txt");
    let mut child = Command::new(exe)
        .args(["--no-config", "-d"])
        .arg(dir.join("c"))
        .args(["--debug-server", "127.0.0.1:0"])
        .env("SDL_VIDEODRIVER", "dummy")
        .env("SDL_AUDIODRIVER", "dummy")
        .stdout(Stdio::piped())
        .stderr(std::fs::File::create(&stderr_path).unwrap())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut emulator = Emulator(child);
    // The address is on the line that says where the server listens.
    let (found, lines) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(at) = line.split("listening on http://").nth(1) {
                let _ = found.send(at.trim_end_matches('/').to_string());
            }
        }
    });
    let Ok(at) = lines.recv_timeout(Duration::from_secs(60)) else {
        let exited = emulator.0.try_wait().ok().flatten();
        let stderr = std::fs::read_to_string(&stderr_path).unwrap_or_default();
        panic!("the emulator didn't start its debug server within 60 s (exit: {exited:?}); stderr:\n{stderr}");
    };
    (emulator, at.parse().unwrap())
}

#[test]
fn the_journal_follows_a_session_to_its_shutdown() {
    let dir = std::env::temp_dir().join(format!("rust-dos-journal-bin-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("c")).unwrap();
    std::fs::write(dir.join("c").join("PROBE.COM"), PROBE).unwrap();
    let (mut emulator, addr) = start(&dir);
    let get = |path: &str| http(addr, "GET", path, &[], None);
    let post = |path: &str, body: Value| http(addr, "POST", path, &[], Some(body));

    // At the prompt, then at the probe's first instruction.
    let deadline = Instant::now() + Duration::from_secs(60);
    while !get("/api/status").1["program"].as_str().is_some_and(str::is_empty) {
        assert!(Instant::now() < deadline, "the shell starts");
        std::thread::sleep(Duration::from_millis(50));
    }
    let (status, stop) = post("/api/control/run", json!({"command": "PROBE.COM", "stop_at_entry": true}));
    assert_eq!(status, 200, "{}", stop);
    let cs = stop["registers"]["cs"].as_str().unwrap().to_string();
    let first = stop["stop"]["seq"].as_u64().unwrap();

    // A breakpoint once, and the resume's operation that ends there.
    let (status, listed) = post("/api/breakpoints", json!({"addr": format!("{}:0103", cs), "once": true}));
    assert_eq!((status, listed["once"].as_array().map(Vec::len)), (200, Some(1)), "{}", listed);
    let (_, resumed) = post("/api/control/resume", json!({}));
    let (_, op) = get(&format!("/api/ops/{}?wait=30000", resumed["op"]));
    assert_eq!(op["state"], "stopped", "{}", op);
    assert_eq!((op["stop"]["reason"].as_str(), op["stop"]["once"].as_bool()), (Some("breakpoint"), Some(true)));
    assert_eq!(get("/api/breakpoints").1["breakpoints"], json!([]));

    // A guarded write.
    let (_, status_now) = get("/api/status");
    let seq = status_now["state_seq"].as_u64().unwrap().to_string();
    let write = json!({"addr": "0200:0000", "hex": "12 34"});
    let (status, written) = http(addr, "PUT", "/api/memory", &[("If-State-Seq", &seq)], Some(write.clone()));
    assert_eq!(status, 200, "{}", written);
    let (status, refused) = http(addr, "PUT", "/api/memory", &[("If-State-Seq", &seq)], Some(write));
    assert_eq!((status, refused["precondition"].as_str()), (409, Some("state")), "{}", refused);

    // A strict-match save and load.
    let state = dir.join("probe.state").display().to_string();
    let (status, saved) = post("/api/state/save", json!({"path": state, "strict_match": true}));
    assert_eq!(status, 200, "{}", saved);
    let (status, loaded) = post("/api/state/load", json!({"path": state, "strict_match": true}));
    assert_eq!(status, 200, "{}", loaded);
    let (_, journal) = get(&format!("/api/journal?after={}", first));
    let types: Vec<&str> = journal["entries"].as_array().unwrap().iter().filter_map(|e| e["type"].as_str()).collect();
    let tail = &types[types.len() - 4..];
    assert_eq!(tail, ["state_saved", "state_loaded", "debugger_reset", "paused"], "{:?}", types);

    // Quit: a long poll in flight gets `shutdown`, and the emulator ends.
    let last = journal["last"].as_u64().unwrap();
    let poll = std::thread::spawn(move || http(addr, "GET", &format!("/api/journal?after={}&type=paused&wait=30000", last), &[], None));
    std::thread::sleep(Duration::from_millis(200));
    let (status, quit) = post("/api/control/quit", json!({}));
    assert_eq!((status, quit["shutting_down"].as_bool()), (200, Some(true)));
    let (_, polled) = poll.join().unwrap();
    let entries = polled["entries"].as_array().unwrap();
    assert_eq!(entries.last().unwrap()["type"], "shutdown", "{}", polled);
    assert_eq!(entries.last().unwrap()["reason"], "quit");
    let deadline = Instant::now() + Duration::from_secs(30);
    let code = loop {
        if let Some(code) = emulator.0.try_wait().unwrap() {
            break code;
        }
        assert!(Instant::now() < deadline, "the emulator ends");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(code.code(), Some(0));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_check_files_load_names_the_files_changed_since_the_save_and_one_without_it_reads_none() {
    let dir = std::env::temp_dir().join(format!("rust-dos-contents-bin-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("c").join("SAVES")).unwrap();
    std::fs::write(dir.join("c").join("CONFIG.DAT"), "sound=1").unwrap();
    let (_emulator, addr) = start(&dir);
    let get = |path: &str| http(addr, "GET", path, &[], None);
    let post = |path: &str, body: Value| http(addr, "POST", path, &[], Some(body));
    let deadline = Instant::now() + Duration::from_secs(60);
    while !get("/api/status").1["program"].as_str().is_some_and(str::is_empty) {
        assert!(Instant::now() < deadline, "the shell starts");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(post("/api/control/pause", json!({})).0, 200);
    // The states go beside C:'s folder, out of it.
    let state = |name: &str| dir.join(name).display().to_string();
    let checked = |name: &str, strict: bool| json!({"path": state(name), "strict_match": strict, "check_files": true});
    let (status, saved) = post("/api/state/save", checked("a.state", true));
    assert_eq!(status, 200, "{}", saved);
    let c = &saved["header"]["media"][0];
    assert_eq!((c["drive"].as_str(), c["files_recorded"].as_u64()), (Some("C"), Some(2)), "{}", saved);
    let (status, loaded) = post("/api/state/load", checked("a.state", true));
    assert_eq!(status, 200, "{}", loaded);
    assert!(loaded.get("differences").is_none() && loaded.get("unchecked").is_none(), "{}", loaded);
    // A save without check_files, strict or not, records no files.
    for (name, strict) in [("b.state", true), ("c.state", false)] {
        let (status, saved) = post("/api/state/save", json!({"path": state(name), "strict_match": strict}));
        assert_eq!(status, 200, "{}", saved);
        assert!(saved["header"]["media"][0].get("files_recorded").is_none(), "{}", saved);
    }

    // A save game written and the configuration changed after the save.
    std::fs::write(dir.join("c").join("SAVES").join("GAME1.SAV"), "saved game").unwrap();
    std::fs::write(dir.join("c").join("CONFIG.DAT"), "sound=0").unwrap();
    let (status, refused) = post("/api/state/load", checked("a.state", true));
    assert_eq!((status, refused["loaded"].as_bool()), (409, Some(false)), "{}", refused);
    let expected = json!([
        "drive C: file CONFIG.DAT: its contents differ from when it was saved",
        "drive C: file SAVES/GAME1.SAV: new since the save",
    ]);
    assert_eq!(refused["differences"], expected, "{}", refused);
    // Without strict_match it goes through and lists them.
    let (status, loaded) = post("/api/state/load", checked("a.state", false));
    assert_eq!((status, &loaded["differences"]), (200, &expected), "{}", loaded);
    // Without check_files no file is compared, even for a state with the
    // record and a strict match.
    for strict in [true, false] {
        let (status, loaded) = post("/api/state/load", json!({"path": state("a.state"), "strict_match": strict}));
        assert_eq!(status, 200, "{}", loaded);
        assert!(loaded.get("differences").is_none() && loaded.get("unchecked").is_none(), "{}", loaded);
    }

    // A state saved without check_files: a check_files load says it
    // couldn't check C: and goes through.
    let (status, loaded) = post("/api/state/load", checked("b.state", true));
    assert_eq!(status, 200, "{}", loaded);
    let unchecked = loaded["unchecked"].as_array().unwrap();
    assert!(unchecked.len() == 1 && unchecked[0].as_str().unwrap().starts_with("drive C: wasn't checked"), "{}", loaded);
    assert_eq!(post("/api/control/quit", json!({})).0, 200);
}
