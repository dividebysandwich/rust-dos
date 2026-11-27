//! `rust-dos report`: a report to the almanac from the command line,
//! sent to a stand-in for the almanac's API on a local port.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::mpsc::{self, Receiver};

/// A request the stand-in took: its method and path, its Authorization
/// header and its body.
struct Request {
    method: String,
    path: String,
    authorization: Option<String>,
    body: String,
}

/// An almanac that knows no game: a lookup and a search find nothing, a
/// new game gets the slug `test-game` and version 5, and a report gets
/// the number 42. Its address, and the requests as they come.
fn almanac() -> (String, Receiver<Request>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}/api/v1", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                continue;
            }
            let mut parts = line.split_whitespace();
            let method = parts.next().unwrap_or_default().to_string();
            let path = parts.next().unwrap_or_default().to_string();
            let (mut length, mut authorization) = (0, None);
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).unwrap();
                let header = header.trim_end();
                if header.is_empty() {
                    break;
                }
                let (name, value) = header.split_once(':').unwrap();
                match name.to_ascii_lowercase().as_str() {
                    "content-length" => length = value.trim().parse().unwrap(),
                    "authorization" => authorization = Some(value.trim().to_string()),
                    _ => {}
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            // Taken down before the answer, which the command waits for.
            let body = String::from_utf8(body).unwrap();
            let (m, p) = (method.clone(), path.clone());
            let _ = tx.send(Request {
                method: m,
                path: p,
                authorization,
                body,
            });
            let answer = match (method.as_str(), path.split('?').next().unwrap()) {
                ("POST", "/api/v1/games/lookup") | ("GET", "/api/v1/games") => "[]",
                ("POST", "/api/v1/games") => r#"{"slug":"test-game","versions":[{"id":5}]}"#,
                ("POST", "/api/v1/reports") => r#"{"id":42,"replaced":false}"#,
                _ => r#"{"error":"not found"}"#,
            };
            let status = if answer.contains("error") {
                "404 Not Found"
            } else {
                "200 OK"
            };
            let _ = write!(
                stream,
                "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{}",
                status,
                answer.len(),
                answer
            );
        }
    });
    (address, rx)
}

/// A folder with a configuration and a game's program in it.
fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from("target/test_report").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // The 3dfx card's renderer under the name it had before `renderer`.
    let conf = "[emulator]\nmachine=svga_voodoo3\nvoodoo_renderer=opengl\ncycles=20000\n";
    std::fs::write(dir.join("game.conf"), conf).unwrap();
    std::fs::write(dir.join("GAME.EXE"), b"MZ a game").unwrap();
    dir
}

/// `rust-dos -c game.conf report GAME.EXE` with `args`, against `api`.
fn report(dir: &Path, api: &str, token: Option<&str>, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rust-dos"));
    command
        .arg("-c")
        .arg(dir.join("game.conf"))
        .arg("report")
        .arg(dir.join("GAME.EXE"))
        .args(args)
        .env("RUST_DOS_ALMANAC_API", api)
        .env_remove("RUST_DOS_ALMANAC_TOKEN");
    if let Some(token) = token {
        command.env("RUST_DOS_ALMANAC_TOKEN", token);
    }
    command.output().unwrap()
}

#[test]
fn a_report_adds_the_game_with_the_configurations_settings() {
    let dir = scratch("new_game");
    let (api, requests) = almanac();
    let out = report(
        &dir,
        &api,
        Some("secret"),
        &[
            "--name",
            "Test Game",
            "--rating",
            "yellow",
            "--year",
            "1997",
        ],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{}{}",
        stdout,
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("Report #42"), "{}", stdout);
    assert!(stdout.contains("/games/test-game"), "{}", stdout);
    // A report without a profile gets a key of its own, which it prints.
    assert!(stdout.contains("--key "), "{}", stdout);

    let requests: Vec<Request> = requests.try_iter().collect();
    let paths: Vec<String> = requests
        .iter()
        .map(|r| format!("{} {}", r.method, r.path))
        .collect();
    assert_eq!(
        paths,
        [
            "POST /api/v1/games/lookup",
            "GET /api/v1/games?q=Test%20Game",
            "POST /api/v1/games",
            "POST /api/v1/reports"
        ]
    );
    // The token goes with every request.
    assert!(
        requests
            .iter()
            .all(|r| r.authorization.as_deref() == Some("Bearer secret"))
    );

    let game: serde_json::Value = serde_json::from_str(&requests[2].body).unwrap();
    assert_eq!(game["name"], "Test Game");
    assert_eq!(game["year"], 1997);
    assert_eq!(game["version"]["files"][0]["name"], "game.exe");
    let sent: serde_json::Value = serde_json::from_str(&requests[3].body).unwrap();
    assert_eq!(sent["game"], "test-game");
    assert_eq!(sent["version_id"], 5);
    assert_eq!(sent["rating"], "yellow");
    let emulator = &sent["settings"]["emulator"];
    assert_eq!(emulator["machine"], "svga_voodoo3");
    assert_eq!(emulator["cycles"], "20000");
    // The renderer goes under its current name only.
    assert_eq!(emulator["renderer"], "opengl");
    assert!(emulator.get("voodoo_renderer").is_none());
    assert_eq!(sent["key"], game["key"]);
    assert!(sent["key"].as_str().is_some_and(|k| k.len() == 36));
}

#[test]
fn a_dry_run_and_an_unknown_file_without_a_name_send_no_report() {
    let dir = scratch("dry_run");
    let (api, requests) = almanac();

    let out = report(&dir, &api, None, &["--name", "Test Game", "--dry-run"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("Would add the game Test Game"),
        "{}",
        stdout
    );
    assert!(stdout.contains("renderer=opengl"), "{}", stdout);

    let out = report(&dir, &api, None, &[]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--name"));

    let requests: Vec<Request> = requests.try_iter().collect();
    assert!(requests.iter().all(|r| r.authorization.is_none()));
    assert!(
        requests
            .iter()
            .all(|r| r.method == "GET" || r.path == "/api/v1/games/lookup")
    );
}

#[test]
fn a_report_from_a_profile_keeps_its_notes_on_one_line_and_the_key_given() {
    let dir = scratch("profile");
    std::fs::create_dir_all(dir.join("games")).unwrap();
    let profile = dir.join("games").join("test.conf");
    std::fs::write(&profile, "[game]\nname=Test Game\n\n[autoexec]\nGAME\n").unwrap();
    let (api, requests) = almanac();
    let key = "0123abcd-0000-4000-8000-00000000ABCD";
    let out = Command::new(env!("CARGO_BIN_EXE_rust-dos"))
        .arg("-c")
        .arg(dir.join("game.conf"))
        .args(["--game", "Test Game", "report"])
        .arg(dir.join("GAME.EXE"))
        .args([
            "--notes",
            "Needs EMS\n[autoexec]\nDEL GAME.EXE",
            "--key",
            key,
        ])
        .env("RUST_DOS_ALMANAC_API", &api)
        .env_remove("RUST_DOS_ALMANAC_TOKEN")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{}{}",
        stdout,
        String::from_utf8_lossy(&out.stderr)
    );
    // The key given isn't new, so it isn't printed.
    assert!(!stdout.contains("--key "), "{}", stdout);

    // The notes stay on the profile's one line, and the profile, which
    // had no key, takes the one given, in lower case as it keeps keys.
    let kept = std::fs::read_to_string(&profile).unwrap();
    let lowered = key.to_ascii_lowercase();
    assert!(
        kept.contains("notes=Needs EMS [autoexec] DEL GAME.EXE\n"),
        "{}",
        kept
    );
    assert!(kept.contains(&format!("uuid={}\n", lowered)), "{}", kept);
    assert!(kept.ends_with("\n[autoexec]\nGAME\n"), "{}", kept);

    let requests: Vec<Request> = requests.try_iter().collect();
    let report = requests.last().unwrap();
    assert_eq!(report.path, "/api/v1/reports");
    let sent: serde_json::Value = serde_json::from_str(&report.body).unwrap();
    assert_eq!(sent["key"], lowered.as_str());
    assert_eq!(sent["notes"], "Needs EMS [autoexec] DEL GAME.EXE");
}
