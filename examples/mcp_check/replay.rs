//! Reproducible acquisition from the real native HTTP provider.
use super::acquisition::{read_bounded, require, source_digest};
use serde_json::{Value, json};
use std::{
    io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[path = "replay/frontend.rs"]
pub(super) mod frontend;

pub(super) fn validate_root(path: &Path) -> io::Result<()> {
    require(
        path.is_absolute() && !path.exists(),
        "replay output must be a new absolute directory",
    )?;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("missing replay parent"))?
        .canonicalize()?;
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).canonicalize()?;
    require(
        !parent.starts_with(repository),
        "replay output must remain outside repository",
    )?;
    require(path.file_name().is_some(), "missing replay directory name")?;
    Ok(())
}

fn listener_url(log: &str) -> io::Result<Option<String>> {
    let Some(url) = log
        .lines()
        .find_map(|line| line.split_once("listening on ").map(|(_, url)| url.trim()))
    else {
        return Ok(None);
    };
    let url = url.strip_suffix('/').unwrap_or(url);
    let address: std::net::SocketAddr = url
        .strip_prefix("http://")
        .ok_or_else(|| io::Error::other("unexpected native listener URL"))?
        .parse()
        .map_err(io::Error::other)?;
    require(
        address.ip() == std::net::Ipv4Addr::LOCALHOST && address.port() != 0,
        "native listener must use loopback",
    )?;
    Ok(Some(url.to_owned()))
}

struct Provider {
    root: PathBuf,
    url: String,
    agent: ureq::Agent,
    started: Instant,
    deadline: Duration,
}

impl Provider {
    fn request(
        &self,
        name: &str,
        route: &str,
        body: Option<Value>,
        png: bool,
    ) -> io::Result<Vec<u8>> {
        require(
            self.started.elapsed() < self.deadline,
            "replay journey deadline",
        )?;
        for log in ["stdout.log", "stderr.log"] {
            require(
                std::fs::metadata(self.root.join(log))?.len() <= 64 * 1024,
                "native process output bound",
            )?;
        }
        let url = format!("{}{route}", self.url);
        let mut response = if let Some(body) = body {
            let bytes = serde_json::to_vec(&body).map_err(io::Error::other)?;
            std::fs::write(self.root.join(format!("{name}.request.json")), &bytes)?;
            self.agent
                .post(&url)
                .header("Content-Type", "application/json")
                .send(bytes)
                .map_err(io::Error::other)?
        } else {
            self.agent.get(&url).call().map_err(io::Error::other)?
        };
        let bytes = response
            .body_mut()
            .with_config()
            .limit(1024 * 1024)
            .read_to_vec()
            .map_err(io::Error::other)?;
        require(!bytes.is_empty(), "empty native response")?;
        if png {
            super::acquisition::png_pixels(&bytes)?;
        } else {
            serde_json::from_slice::<Value>(&bytes).map_err(io::Error::other)?;
        }
        std::fs::write(
            self.root
                .join(format!("{name}.{}", if png { "png" } else { "json" })),
            &bytes,
        )?;
        Ok(bytes)
    }

    fn get(&self, name: &str, route: &str) -> io::Result<Value> {
        serde_json::from_slice(&self.request(name, route, None, false)?).map_err(io::Error::other)
    }

    fn post(&self, name: &str, route: &str, body: Value) -> io::Result<()> {
        self.request(name, route, Some(body), false)?;
        Ok(())
    }

    fn input(&self, name: &str, kind: &str, body: Value) -> io::Result<()> {
        self.post(name, &format!("/api/input/{kind}?wait=true"), body)
    }

    fn image(&self, name: &str) -> io::Result<()> {
        self.request(name, "/api/screenshot", None, true)?;
        Ok(())
    }
}

async fn connect(provider: &mut Provider, child: &mut tokio::process::Child) -> io::Result<()> {
    let startup = Instant::now();
    while startup.elapsed() < Duration::from_secs(10) {
        require(
            child.try_wait()?.is_none(),
            "native provider exited during startup",
        )?;
        let path = provider.root.join("stdout.log");
        let log = if std::fs::metadata(&path)?.len() == 0 {
            String::new()
        } else {
            String::from_utf8(read_bounded(&path, 64 * 1024)?).map_err(io::Error::other)?
        };
        if let Some(url) = listener_url(&log)? {
            provider.url = url;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    require(!provider.url.is_empty(), "native listener startup deadline")?;
    let startup = Instant::now();
    loop {
        let status = provider.get("01-status", "/api/status")?;
        if status["shell_idle"] == true {
            break;
        }
        require(
            startup.elapsed() < Duration::from_secs(10),
            "shell readiness deadline",
        )?;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(())
}

async fn journey(provider: &mut Provider, child: &mut tokio::process::Child) -> io::Result<()> {
    connect(provider, child).await?;
    provider.get("02-shell-text", "/api/screen/text")?;
    provider.image("03-shell")?;
    provider.get("04-version", "/api/version")?;
    provider.input("05-type", "type", json!({"text":"echo MCP_SHELL_OBS_01"}))?;
    provider.input("06-enter", "key", json!({"key":"enter"}))?;
    provider.get("07-echo-text", "/api/screen/text")?;
    provider.get("08-echo-status", "/api/status")?;
    provider.input(
        "09-shift-down",
        "key",
        json!({"key":"shift","action":"down"}),
    )?;
    let bda = "/api/memory?addr=0040:0017&len=2&format=json";
    provider.get("10-shift-down-bda", bda)?;
    provider.input(
        "11-mouse-down",
        "mouse",
        json!({"action":"down","button":"left","x":160,"y":80}),
    )?;
    provider.get("12-mouse-down-status", "/api/status")?;
    provider.post("13-pause", "/api/control/pause", json!({}))?;
    let state = json!({"path":provider.root.join("held.state"),"strict_match":true});
    provider.post("14-save-held", "/api/state/save", state.clone())?;
    provider.post("15-resume", "/api/control/resume", json!({}))?;
    provider.input("16-shift-up", "key", json!({"key":"shift","action":"up"}))?;
    provider.input(
        "17-mouse-up",
        "mouse",
        json!({"action":"up","button":"left"}),
    )?;
    provider.get("18-released-bda", bda)?;
    provider.get("19-released-status", "/api/status")?;
    provider.post("20-pause", "/api/control/pause", json!({}))?;
    provider.post("21-load-held", "/api/state/load", state)?;
    provider.get("22-restored-bda", bda)?;
    provider.get("23-restored-status", "/api/status")?;
    provider.post("24-resume", "/api/control/resume", json!({}))?;
    provider.get("25-resumed-restored-bda", bda)?;
    provider.get("26-resumed-restored-status", "/api/status")?;
    provider.input(
        "27-mouse-move-held",
        "mouse",
        json!({"action":"move","x":320,"y":160}),
    )?;
    provider.get("28-moved-held-status", "/api/status")?;
    provider.input("29-shift-up", "key", json!({"key":"shift","action":"up"}))?;
    provider.input(
        "30-mouse-up",
        "mouse",
        json!({"action":"up","button":"left"}),
    )?;
    provider.get("31-journal", "/api/journal")?;
    provider.get("32-final-status", "/api/status")?;
    provider.image("33-echo")?;
    provider.input("34-edit-type", "type", json!({"text":"EDIT"}))?;
    provider.input("35-edit-enter", "key", json!({"key":"enter"}))?;
    provider.input("35-edit-wait", "wait", json!({"ms":100}))?;
    provider.input("38-edit-type", "type", json!({"text":"MCP DRAG SAMPLE"}))?;
    provider.image("39-edit-text")?;
    provider.input(
        "40-drag-down",
        "mouse",
        json!({"action":"down","button":"left","x":8,"y":40,"coords":"screen"}),
    )?;
    provider.input("40-down-wait", "wait", json!({"ms":100}))?;
    provider.input(
        "41-drag-move",
        "mouse",
        json!({"action":"move","x":128,"y":40,"coords":"screen"}),
    )?;
    provider.input("41-move-wait", "wait", json!({"ms":100}))?;
    provider.get("42-drag-held-status", "/api/status")?;
    provider.input(
        "43-drag-up",
        "mouse",
        json!({"action":"up","button":"left"}),
    )?;
    provider.image("44-drag-selection")?;
    provider.get("45-drag-selection-text", "/api/screen/text")?;
    provider.get("46-drag-released-status", "/api/status")?;
    provider.input(
        "47-replace-selection",
        "type",
        json!({"text":"MCP_REPLACED"}),
    )?;
    provider.get("48-replaced-text", "/api/screen/text")?;
    provider.get("49-edit-journal", "/api/journal")?;
    provider.input(
        "50-finite",
        "batch",
        json!({"events":[
            {"type":"mouse","action":"down","button":"left","x":8,"y":40,"coords":"screen"},
            {"type":"wait","ms":100},
            {"type":"mouse","action":"move","x":104,"y":40,"coords":"screen"},
            {"type":"wait","ms":100},
            {"type":"mouse","action":"up","button":"left"},
            {"type":"wait","ms":100}
        ]}),
    )?;
    provider.get("51-finite-selection-text", "/api/screen/text")?;
    provider.get("52-finite-released-status", "/api/status")?;
    provider.input("53-finite-replace", "type", json!({"text":"MCP_FINITE"}))?;
    provider.get("54-finite-replaced-text", "/api/screen/text")?;
    provider.input(
        "55-click",
        "mouse",
        json!({"action":"click","button":"left","x":40,"y":40,"coords":"screen"}),
    )?;
    provider.input("55-click-wait", "wait", json!({"ms":100}))?;
    provider.get("56-click-text", "/api/screen/text")?;
    provider.get("57-click-status", "/api/status")?;
    provider.input(
        "58-relative",
        "mouse",
        json!({"action":"move","dx":5,"dy":-3}),
    )?;
    provider.get("59-relative-status", "/api/status")?;
    provider.get("60-matrix-journal", "/api/journal")?;
    Ok(())
}

pub(super) async fn shell(root: &Path) -> io::Result<()> {
    validate_root(root)?;
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = repository.join(if cfg!(windows) {
        "target/debug/rust-dos.exe"
    } else {
        "target/debug/rust-dos"
    });
    let mut revision = Command::new("git");
    revision.current_dir(repository).args(["rev-parse", "HEAD"]);
    let revision = crate::run(revision, Duration::from_secs(10), 8192).await?;
    let revision = std::str::from_utf8(&revision.bytes)
        .map_err(io::Error::other)?
        .trim();
    let mut sources = serde_json::Map::new();
    for path in [
        "src/debug/server.rs",
        "src/debug/mod.rs",
        "src/debug/input.rs",
        "src/debug/dispatch.rs",
        "src/debug/run.rs",
        "src/debug/state.rs",
        "src/debug/events.rs",
        "src/startup.rs",
        "src/args.rs",
        "src/edit/mod.rs",
        "src/video/assets/README.md",
        "Cargo.lock",
        "LICENSE",
    ] {
        sources.insert(
            path.to_owned(),
            json!(source_digest(&repository.join(path))?),
        );
    }
    require(
        std::fs::metadata(&source)?.len() <= 512 * 1024 * 1024,
        "native executable size bound",
    )?;
    std::fs::create_dir(root)?;
    std::fs::create_dir(root.join("guest"))?;
    let native = root.join(if cfg!(windows) {
        "native-rust-dos.exe"
    } else {
        "native-rust-dos"
    });
    std::fs::copy(source, &native)?;
    let digest = source_digest(&native)?;
    std::fs::write(
        root.join("producer.json"),
        serde_json::to_vec_pretty(&json!({"path":native,"sha256":digest}))
            .map_err(io::Error::other)?,
    )?;
    run_guest(root, &native, &root.join("guest"), Journey::Shell).await?;
    let mut files = serde_json::Map::new();
    for (name, path) in [
        ("shell_status", "01-status.json"),
        ("shell_png", "03-shell.png"),
        ("shell_text", "02-shell-text.json"),
        ("echo_request", "05-type.request.json"),
        ("enter_request", "06-enter.request.json"),
        ("echo_response", "05-type.json"),
        ("enter_response", "06-enter.json"),
        ("echo_text", "07-echo-text.json"),
        ("shift_request", "09-shift-down.request.json"),
        ("shift_response", "09-shift-down.json"),
        ("held_flags", "10-shift-down-bda.json"),
        ("released_flags", "18-released-bda.json"),
        ("restored_flags", "22-restored-bda.json"),
        ("resumed_flags", "25-resumed-restored-bda.json"),
        ("mouse_down_response", "11-mouse-down.json"),
        ("mouse_up_response", "17-mouse-up.json"),
        ("held_mouse", "12-mouse-down-status.json"),
        ("released_mouse", "19-released-status.json"),
        ("restored_mouse", "23-restored-status.json"),
        ("resumed_mouse", "26-resumed-restored-status.json"),
        ("moved_mouse", "28-moved-held-status.json"),
        ("save_request", "14-save-held.request.json"),
        ("load_request", "21-load-held.request.json"),
        ("save_response", "14-save-held.json"),
        ("load_response", "21-load-held.json"),
        ("state_file", "held.state"),
        ("journal", "31-journal.json"),
        ("native_version", "04-version.json"),
        ("drag_selected", "45-drag-selection-text.json"),
        ("drag_replaced", "48-replaced-text.json"),
        ("drag_held", "42-drag-held-status.json"),
        ("drag_released", "46-drag-released-status.json"),
        ("drag_before_png", "39-edit-text.png"),
        ("drag_after_png", "44-drag-selection.png"),
        ("text_finite_request", "50-finite.request.json"),
        ("text_finite", "50-finite.json"),
        ("text_finite_selected", "51-finite-selection-text.json"),
        ("text_finite_released", "52-finite-released-status.json"),
        ("text_finite_replaced", "54-finite-replaced-text.json"),
        ("text_click_request", "55-click.request.json"),
        ("text_click", "55-click.json"),
        ("text_click_text", "56-click-text.json"),
        ("text_clicked", "57-click-status.json"),
        ("text_relative_request", "58-relative.request.json"),
        ("text_relative", "58-relative.json"),
        ("text_relative_status", "59-relative-status.json"),
        ("text_journal", "60-matrix-journal.json"),
    ] {
        files.insert(
            name.to_owned(),
            json!({"path":path,"sha256":source_digest(&root.join(path))?}),
        );
    }
    let producer: Value = serde_json::from_slice(&read_bounded(&root.join("producer.json"), 8192)?)
        .map_err(io::Error::other)?;
    let catalog = root.join("catalog.json");
    std::fs::write(
        &catalog,
        serde_json::to_vec_pretty(&json!({
            "schema_version":1,"revision":revision,"raw_root":root,"sources":sources,
            "native_producer":producer,"files":files
        }))
        .map_err(io::Error::other)?,
    )?;
    super::acquisition::verify_shell(&catalog, revision)?;
    println!(
        "Verified actual shell journey: {} (reference SDK and graphics readiness remain separate)",
        root.display()
    );
    Ok(())
}

enum Journey {
    Benchmark,
    Client {
        program: String,
    },
    Frontend {
        display: String,
        config: PathBuf,
        load: bool,
        profile: Option<String>,
        mcp: bool,
    },
    Shell,
    CoverageShell {
        mcp: bool,
        profile: String,
    },
    Graphics {
        program: String,
        directory: String,
        command: String,
        mcp: bool,
        mouse: bool,
    },
}

pub(super) async fn measure_overhead(root: &Path, baseline: &Path) -> io::Result<()> {
    validate_root(root)?;
    let baseline = baseline.canonicalize()?;
    require(
        baseline.is_file()
            && !baseline.starts_with(Path::new(env!("CARGO_MANIFEST_DIR")).canonicalize()?),
        "benchmark baseline must be a retained private executable",
    )?;
    std::fs::create_dir(root)?;
    let build = super::native::build_producer(root).await?;
    let current = root.join(if cfg!(windows) {
        "native-rust-dos.exe"
    } else {
        "native-rust-dos"
    });
    let retained_baseline = root.join("baseline-rust-dos");
    std::fs::copy(&baseline, &retained_baseline)?;
    require(
        source_digest(&retained_baseline)? == source_digest(&baseline)?,
        "baseline changed during retention",
    )?;
    let guest = root.join("guest");
    std::fs::create_dir(&guest)?;
    for (index, (name, native)) in [
        ("baseline", &retained_baseline),
        ("current", &current),
        ("current", &current),
        ("baseline", &retained_baseline),
        ("baseline", &retained_baseline),
        ("current", &current),
    ]
    .into_iter()
    .enumerate()
    {
        let sample = root.join(format!("{index}-{name}"));
        std::fs::create_dir(&sample)?;
        run_guest(&sample, native, &guest, Journey::Benchmark).await?;
    }
    super::native::verify_build_receipt(root, &build)?;
    std::fs::write(root.join("producer.json"), serde_json::to_vec_pretty(&json!({"schema_version":1,"current_build":build,"baseline":{"path":retained_baseline,"sha256":source_digest(&baseline)?},"order":["baseline","current","current","baseline","baseline","current"],"cycles":10000,"video_driver":"dummy","audio_driver":"dummy"})).map_err(io::Error::other)?)?;
    println!(
        "Measured six actual baseline/current SDK workloads; inspect distributions before any overhead claim"
    );
    Ok(())
}

async fn run_guest(root: &Path, native: &Path, guest: &Path, kind: Journey) -> io::Result<()> {
    let profile = match &kind {
        Journey::CoverageShell { profile, .. } => Some(profile.clone()),
        Journey::Frontend { profile, .. } => profile.clone(),
        _ => None,
    };
    let mut command = Command::new(native);
    if matches!(
        &kind,
        Journey::Graphics { mcp: true, .. }
            | Journey::Client { .. }
            | Journey::Benchmark
            | Journey::CoverageShell { mcp: true, .. }
            | Journey::Frontend { mcp: true, .. }
    ) {
        command.arg("--mcp");
    }
    if let Some(profile) = &profile {
        command.env("LLVM_PROFILE_FILE", profile);
    }
    if let Journey::Frontend {
        display, config, ..
    } = &kind
    {
        command
            .arg("--config")
            .arg(config)
            .env("DISPLAY", display)
            .env("SDL_VIDEODRIVER", "x11");
    } else {
        command.arg("--no-config").env("SDL_VIDEODRIVER", "dummy");
    }
    command
        .args([
            "--debug-server",
            "127.0.0.1:0",
            "--cycles",
            if matches!(kind, Journey::Shell | Journey::CoverageShell { .. }) {
                "3000"
            } else {
                "10000"
            },
            "-d",
        ])
        .arg(guest)
        .env("SDL_AUDIODRIVER", "dummy")
        .stdin(Stdio::null())
        .stdout(std::fs::File::create(root.join("stdout.log"))?)
        .stderr(std::fs::File::create(root.join("stderr.log"))?);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_SUSPENDED);
    }
    let mut command = tokio::process::Command::from(command);
    command.kill_on_drop(true);
    let profile_start = std::time::SystemTime::now();
    let mut child = command.spawn()?;
    let child_pid = child
        .id()
        .ok_or_else(|| io::Error::other("native child PID missing"))?;
    let tree = match crate::ProcessTree::attach(&child) {
        Ok(tree) => tree,
        Err(error) => {
            child.kill().await?;
            child.wait().await?;
            return Err(error);
        }
    };
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .max_redirects(0)
        .build();
    let coverage = profile.is_some();
    let deadline = if matches!(kind, Journey::Shell | Journey::CoverageShell { .. }) {
        Duration::from_secs(120)
    } else {
        Duration::from_secs(240)
    };
    let mut provider = Provider {
        root: root.to_owned(),
        url: String::new(),
        agent: ureq::Agent::new_with_config(config),
        started: Instant::now(),
        deadline,
    };
    let result = match kind {
        Journey::Benchmark => {
            async {
                connect(&mut provider, &mut child).await?;
                super::client::benchmark(&provider.url, root).await
            }
            .await
        }
        Journey::Client { program } => {
            async {
                connect(&mut provider, &mut child).await?;
                provider.image("before")?;
                super::client::run_client(&provider.url, root, guest, &program).await?;
                provider.get("after-status", "/api/status")?;
                provider.image("after")?;
                if program == "shell" {
                    provider.get("after-text", "/api/screen/text")?;
                }
                Ok(())
            }
            .await
        }
        Journey::Frontend {
            display, load, mcp, ..
        } => frontend::journey(&mut provider, &mut child, &display, load, mcp).await,
        Journey::Shell | Journey::CoverageShell { .. } => journey(&mut provider, &mut child).await,
        Journey::Graphics {
            program,
            directory,
            command,
            mcp,
            mouse,
        } => {
            graphics_journey(
                &mut provider,
                &mut child,
                &program,
                &directory,
                &command,
                mcp,
                mouse,
            )
            .await
        }
    };
    let result = if coverage && result.is_ok() {
        // LLVM profiles require orderly process exit; retain the same force-cleanup owner.
        async {
            provider.post("coverage-quit", "/api/control/quit", serde_json::json!({}))?;
            let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
                .await
                .map_err(|_| {
                    io::Error::other("instrumented native graceful shutdown deadline")
                })??;
            require(
                status.success(),
                "instrumented native exited unsuccessfully",
            )?;
            verify_profiles(root, profile.as_deref().unwrap(), child_pid, profile_start)
        }
        .await
    } else {
        result
    };
    tree.terminate();
    child.start_kill()?;
    tokio::time::timeout(Duration::from_secs(2), child.wait())
        .await
        .map_err(|_| io::Error::other("native replay cleanup deadline"))??;
    result
}

pub(super) async fn client_guest(
    root: &Path,
    native: &Path,
    guest: &Path,
    program: &str,
) -> io::Result<()> {
    run_guest(
        root,
        native,
        guest,
        Journey::Client {
            program: program.into(),
        },
    )
    .await
}

fn verify_profiles(
    root: &Path,
    pattern: &str,
    pid: u32,
    started: std::time::SystemTime,
) -> io::Result<()> {
    let pattern = Path::new(pattern);
    require(
        pattern.is_absolute(),
        "coverage profile pattern must be absolute",
    )?;
    let parent = pattern
        .parent()
        .ok_or_else(|| io::Error::other("coverage profile parent missing"))?;
    let name = pattern
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::other("coverage profile name missing"))?;
    let (prefix, _) = name
        .split_once("%p")
        .ok_or_else(|| io::Error::other("coverage profile requires owned child PID"))?;
    let prefix = format!("{prefix}{pid}-");
    let mut profiles = Vec::new();
    for (count, entry) in std::fs::read_dir(parent)?.enumerate() {
        require(count < 4096, "coverage profile directory bound")?;
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(&prefix) && name.ends_with(".profraw") {
            let metadata = entry.metadata()?;
            require(
                metadata.is_file()
                    && metadata.len() > 0
                    && metadata.len() <= 128 * 1024 * 1024
                    && metadata.modified()? >= started,
                "instrumented native profile missing, empty or stale",
            )?;
            profiles.push(
                serde_json::json!({"path":entry.path(),"sha256":source_digest(&entry.path())?}),
            );
        }
    }
    require(
        !profiles.is_empty(),
        "instrumented native produced no fresh owned profile",
    )?;
    std::fs::write(
        root.join("coverage-profiles.json"),
        serde_json::to_vec_pretty(&profiles).map_err(io::Error::other)?,
    )
}

async fn graphics_journey(
    provider: &mut Provider,
    child: &mut tokio::process::Child,
    program: &str,
    directory: &str,
    command: &str,
    mcp: bool,
    mouse: bool,
) -> io::Result<()> {
    connect(provider, child).await?;
    provider.input(
        "cd",
        "type",
        json!({"text":format!("cd {}\n", directory.replace('/', "\\"))}),
    )?;
    provider.input("launch", "type", json!({"text":format!("{command}\n")}))?;
    provider.get("version", "/api/version")?;
    let startup = Instant::now();
    loop {
        require(
            child.try_wait()?.is_none(),
            "graphics provider exited during startup",
        )?;
        let status = provider.get("status", "/api/status")?;
        let size = if program == "seal" {
            (640, 480)
        } else {
            (320, 200)
        };
        if status["mouse"]["installed"] == true
            && status["video"]["width"] == size.0
            && status["video"]["height"] == size.1
        {
            if program != "seal" {
                break;
            }
            let bytes = provider.request("desktop", "/api/screenshot", None, true)?;
            let (size, pixels) = super::acquisition::png_pixels(&bytes)?;
            require(size == (640, 480), "SEAL desktop dimensions mismatch")?;
            let background = &pixels[(100 * 640 + 10) * 3..(100 * 640 + 10) * 3 + 3];
            let tile = &pixels[(460 * 640 + 123) * 3..(460 * 640 + 123) * 3 + 3];
            if background[0] < 100
                && background[1] > 50
                && background[2] > 100
                && background == tile
            {
                break;
            }
        }
        require(
            startup.elapsed() < Duration::from_secs(150),
            "graphics readiness deadline",
        )?;
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let (start, end) = if program == "seal" {
        provider.input("ready", "wait", json!({"ms":3000}))?;
        provider.input(
            "open",
            "batch",
            json!({"events":[
                {"type":"mouse","action":"click","x":54,"y":65,"coords":"screen"},
                {"type":"wait","ms":150},
                {"type":"mouse","action":"click","x":54,"y":65,"coords":"screen"},
                {"type":"wait","ms":1000}
            ]}),
        )?;
        ((260, 74), (320, 180))
    } else {
        provider.input("ready", "wait", json!({"ms":2000}))?;
        provider.input("dismiss", "key", json!({"key":"enter"}))?;
        provider.input("dismiss-wait", "wait", json!({"ms":300}))?;
        provider.input(
            "preposition",
            "mouse",
            json!({"action":"move","x":80,"y":100,"coords":"screen"}),
        )?;
        provider.input("preposition-wait", "wait", json!({"ms":150}))?;
        ((80, 100), (120, 120))
    };
    provider.image("before")?;
    if mouse {
        return super::native::mouse(
            &provider.url,
            &provider.root,
            program,
            start,
            end,
            |label| {
                provider.input(&format!("{label}-settle"), "wait", json!({"ms":150}))?;
                provider.request(label, "/api/screenshot", None, true)
            },
        )
        .await;
    }
    provider.input(
        "down",
        "mouse",
        json!({"action":"down","button":"left","x":start.0,"y":start.1,"coords":"screen"}),
    )?;
    provider.input("down-wait", "wait", json!({"ms":150}))?;
    provider.get("held", "/api/status")?;
    provider.input(
        "move",
        "mouse",
        json!({"action":"move","x":end.0,"y":end.1,"coords":"screen"}),
    )?;
    provider.input("move-wait", "wait", json!({"ms":300}))?;
    provider.get("moved-held", "/api/status")?;
    provider.input("up", "mouse", json!({"action":"up","button":"left"}))?;
    provider.get("released", "/api/status")?;
    provider.image("after")?;
    let (finite_start, finite_end) = if program == "seal" {
        (end, start)
    } else {
        ((80, 140), (120, 160))
    };
    provider.input(
        "finite-preposition",
        "mouse",
        json!({"action":"move","x":finite_start.0,"y":finite_start.1,"coords":"screen"}),
    )?;
    provider.input("finite-preposition-wait", "wait", json!({"ms":150}))?;
    provider.image("finite-before")?;
    provider.input("finite", "batch", json!({"events":[
        {"type":"mouse","action":"down","button":"left","x":finite_start.0,"y":finite_start.1,"coords":"screen"},
        {"type":"wait","ms":150},
        {"type":"mouse","action":"move","x":finite_end.0,"y":finite_end.1,"coords":"screen"},
        {"type":"wait","ms":300},
        {"type":"mouse","action":"up","button":"left"},
        {"type":"wait","ms":100}
    ]}))?;
    provider.get("finite-released", "/api/status")?;
    provider.image("finite-after")?;
    let click = if program == "seal" {
        (450, 300)
    } else {
        (140, 180)
    };
    provider.image("click-before")?;
    provider.input(
        "click-preposition",
        "mouse",
        json!({"action":"move","x":click.0,"y":click.1,"coords":"screen"}),
    )?;
    provider.input("click-preposition-wait", "wait", json!({"ms":150}))?;
    provider.input(
        "click",
        "mouse",
        json!({"action":"click","button":"left","x":click.0,"y":click.1,"coords":"screen","hold_ms":150}),
    )?;
    provider.input("click-wait", "wait", json!({"ms":150}))?;
    let clicked = provider.get("clicked", "/api/status")?;
    provider.input("relative", "mouse", json!({"action":"move","dx":5,"dy":-3}))?;
    let relative = provider.get("relative-status", "/api/status")?;
    require(
        relative["mouse"]["x"].as_i64() == clicked["mouse"]["x"].as_i64().map(|x| x + 5)
            && relative["mouse"]["y"].as_i64() == clicked["mouse"]["y"].as_i64().map(|y| y - 3)
            && relative["mouse"]["buttons"] == 0,
        "relative mouse transition mismatch",
    )?;
    provider.input("cursor-away", "mouse", json!({"action":"move","x":if program == "seal" { 2000 } else { 200 },"y":if program == "seal" { 1000 } else { 100 },"coords":"virtual"}))?;
    provider.input("cursor-away-wait", "wait", json!({"ms":150}))?;
    provider.image("click-after")?;
    provider.get("journal", "/api/journal")?;
    if mcp {
        provider.post("mcp-pause", "/api/control/pause", json!({}))?;
        let expected = provider.request("mcp-http-picture", "/api/screenshot", None, true)?;
        let mut round = 0;
        super::native::capture(&provider.url, &provider.root, &expected, || {
            round += 1;
            let name = format!("mcp-changed-{round}");
            provider.post(&format!("{name}-resume"), "/api/control/resume", json!({}))?;
            provider.input(
                &format!("{name}-move"),
                "mouse",
                json!({"action":"move","x":40 + round * 80,"y":40 + round * 60,"coords":"screen"}),
            )?;
            provider.input(&format!("{name}-wait"), "wait", json!({"ms":150}))?;
            provider.post(&format!("{name}-pause"), "/api/control/pause", json!({}))?;
            provider.request(
                &format!("{name}-http-picture"),
                "/api/screenshot",
                None,
                true,
            )
        })
        .await?;
    }
    Ok(())
}

pub(super) async fn coverage(root: &Path) -> io::Result<()> {
    validate_root(root)?;
    std::fs::create_dir(root)?;
    let build = super::native::build_instrumented_producer(root).await?;
    let profile: String =
        serde_json::from_slice(&read_bounded(&root.join("coverage-profile.json"), 8192)?)
            .map_err(io::Error::other)?;
    let native = root.join(if cfg!(windows) {
        "native-rust-dos.exe"
    } else {
        "native-rust-dos"
    });
    for enabled in [false, true] {
        let case = root.join(if enabled { "mcp-on" } else { "mcp-off" });
        std::fs::create_dir(&case)?;
        let guest = case.join("guest");
        std::fs::create_dir(&guest)?;
        run_guest(
            &case,
            &native,
            &guest,
            Journey::CoverageShell {
                mcp: enabled,
                profile: profile.clone(),
            },
        )
        .await?;
    }
    super::native::finish_instrumented_producer(root, build)?;
    println!(
        "Acquired actual CLI/startup profiles with MCP enabled and disabled: {}",
        root.display()
    );
    Ok(())
}

pub(super) async fn graphics(root: &Path) -> io::Result<()> {
    graphics_inner(root, false, None).await
}

pub(super) async fn native_graphics(root: &Path) -> io::Result<()> {
    graphics_inner(root, true, None).await
}

pub(super) async fn native_mouse(root: &Path, program: &str) -> io::Result<()> {
    graphics_inner(root, true, Some(program)).await
}

async fn graphics_inner(root: &Path, mcp: bool, mouse_program: Option<&str>) -> io::Result<()> {
    validate_root(root)?;
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest_path = repository.join("tests/fixtures/approved/mcp/manifest.json");
    let prepared = super::assets::prepare_packages(
        &manifest_path,
        &repository.join("docs/mcp/execution/programs.json"),
    )?;
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Prepared {
        schema_version: u32,
        guests: Vec<Guest>,
    }
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Guest {
        id: String,
        guest: PathBuf,
        working_directory: String,
        command: String,
    }
    let guests: Prepared =
        serde_json::from_slice(&read_bounded(&prepared.join("guests.json"), 64 * 1024)?)
            .map_err(io::Error::other)?;
    require(
        guests.schema_version == 1 && guests.guests.len() == 2,
        "prepared graphics guest metadata mismatch",
    )?;
    let manifest: Value = serde_json::from_slice(&read_bounded(&manifest_path, 64 * 1024)?)
        .map_err(io::Error::other)?;
    let mut command = Command::new("git");
    command.current_dir(repository).args(["rev-parse", "HEAD"]);
    let revision = crate::run(command, Duration::from_secs(10), 8192).await?;
    let revision = std::str::from_utf8(&revision.bytes)
        .map_err(io::Error::other)?
        .trim();
    let mut sources = serde_json::Map::new();
    for path in [
        "src/debug/server.rs",
        "src/debug/mod.rs",
        "src/debug/input.rs",
        "src/debug/dispatch.rs",
        "src/debug/run.rs",
        "src/debug/state.rs",
        "src/debug/events.rs",
        "src/startup.rs",
        "src/args.rs",
        "src/edit/mod.rs",
        "src/video/assets/README.md",
        "Cargo.lock",
        "LICENSE",
        "tests/fixtures/approved/mcp/manifest.json",
    ] {
        sources.insert(
            path.to_owned(),
            json!(source_digest(&repository.join(path))?),
        );
    }
    std::fs::create_dir(root)?;
    let native_build = if mcp {
        Some(super::native::build_producer(root).await?)
    } else {
        None
    };
    let native = root.join(if cfg!(windows) {
        "native-rust-dos.exe"
    } else {
        "native-rust-dos"
    });
    let source = repository.join(if cfg!(windows) {
        "target/debug/rust-dos.exe"
    } else {
        "target/debug/rust-dos"
    });
    if native_build.is_none() {
        source_digest(&source)?;
        std::fs::copy(source, &native)?;
    }
    let mut files = serde_json::Map::new();
    for guest in guests.guests {
        if mouse_program.is_some_and(|program| program != guest.id) {
            continue;
        }
        require(
            matches!(guest.id.as_str(), "seal" | "grafx2"),
            "unapproved graphics program",
        )?;
        let case = root.join(&guest.id);
        std::fs::create_dir(&case)?;
        let program = manifest["programs"]
            .as_array()
            .and_then(|programs| programs.iter().find(|program| program["id"] == guest.id))
            .ok_or_else(|| io::Error::other("graphics program manifest missing"))?;
        let configuration = guest
            .guest
            .join(
                program["application_root"]
                    .as_str()
                    .ok_or_else(|| io::Error::other("application root missing"))?,
            )
            .join(
                program["configuration"]["path"]
                    .as_str()
                    .ok_or_else(|| io::Error::other("application configuration missing"))?,
            );
        super::acquisition::verify_source(
            &configuration,
            program["configuration"]["sha256"]
                .as_str()
                .ok_or_else(|| io::Error::other("configuration digest missing"))?,
        )?;
        std::fs::copy(configuration, case.join("configuration.ini"))?;
        run_guest(
            &case,
            &native,
            &guest.guest,
            Journey::Graphics {
                program: guest.id.clone(),
                directory: guest.working_directory,
                command: guest.command,
                mcp,
                mouse: mouse_program.is_some(),
            },
        )
        .await?;
        if mouse_program.is_some() {
            continue;
        }
        let mut captures = vec![
            "held",
            "moved-held",
            "released",
            "before",
            "after",
            "finite-before",
            "finite-after",
            "finite-released",
            "finite.request",
            "finite",
            "click-before",
            "click-after",
            "clicked",
            "click.request",
            "click",
            "relative.request",
            "relative",
            "relative-status",
            "down.request",
            "down",
            "move.request",
            "move",
            "up.request",
            "up",
            "version",
            "journal",
            "configuration",
        ];
        if guest.id == "seal" {
            captures.extend(["open.request", "open", "desktop"]);
        }
        for name in captures {
            let suffix = if matches!(
                name,
                "before"
                    | "after"
                    | "desktop"
                    | "finite-before"
                    | "finite-after"
                    | "click-before"
                    | "click-after"
            ) {
                "png"
            } else if name == "configuration" {
                "ini"
            } else {
                "json"
            };
            let path = format!("{}/{name}.{suffix}", guest.id);
            let key = format!(
                "{}_{}{}",
                guest.id,
                name.replace(['-', '.'], "_"),
                if suffix == "png" { "_png" } else { "" }
            );
            files.insert(
                key,
                json!({"path":path,"sha256":source_digest(&root.join(path))?}),
            );
        }
    }
    let catalog = root.join("catalog.json");
    if let Some(program) = mouse_program {
        let case = root.join(program);
        let mut entries = 0usize;
        for entry in std::fs::read_dir(&case)? {
            let entry = entry?;
            entries += 1;
            require(entries <= 128, "native mouse evidence directory limit")?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| io::Error::other("native mouse evidence name is not UTF-8"))?;
            if name.starts_with("mouse-") {
                require(
                    entry.file_type()?.is_file(),
                    "native mouse evidence must be a real file",
                )?;
                files.insert(
                    format!("{program}_{name}"),
                    json!({
                        "path":format!("{program}/{name}"),"sha256":source_digest(&entry.path())?
                    }),
                );
            }
        }
        std::fs::write(
            root.join("mouse-catalog.json"),
            serde_json::to_vec_pretty(&json!({
                "schema_version":1,"revision":revision,"raw_root":root,"sources":sources,
                "native_build":native_build,"files":files
            }))
            .map_err(io::Error::other)?,
        )?;
        println!("Verified actual native mouse journey: {}", root.display());
        return Ok(());
    }
    std::fs::write(&catalog, serde_json::to_vec_pretty(&json!({"schema_version":1,"revision":revision,"raw_root":root,"sources":sources,"native_producer":{"path":native,"sha256":source_digest(&native)?},"files":files})).map_err(io::Error::other)?)?;
    super::acquisition::verify_graphics(&catalog, revision)?;
    if let Some(build) = native_build {
        super::native::write_catalog(root, revision, build)?;
    }
    println!(
        "Verified actual standard/doubled graphics journeys: {}",
        root.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_refuses_repository_output_and_existing_guest_data() {
        assert!(validate_root(Path::new(env!("CARGO_MANIFEST_DIR"))).is_err());
        let temporary = crate::TemporaryCopy::new().unwrap();
        assert!(validate_root(&temporary.path).is_err());
        assert!(validate_root(&temporary.path.join("new-capture")).is_ok());
        assert!(validate_root(&Path::new(env!("CARGO_MANIFEST_DIR")).join("new-capture")).is_err());
        assert!(validate_root(Path::new("relative-capture")).is_err());
    }

    #[test]
    fn native_listener_url_accepts_the_real_log_and_rejects_remote_hosts() {
        assert_eq!(
            listener_url("[DEBUG] Debug server listening on http://127.0.0.1:38013/\n").unwrap(),
            Some("http://127.0.0.1:38013".to_owned())
        );
        assert!(listener_url("starting").unwrap().is_none());
        for url in [
            "http://0.0.0.0:80/",
            "http://127.0.0.1:0/",
            "https://127.0.0.1:80/",
            "http://127.0.0.1:80/path",
        ] {
            assert!(listener_url(&format!("listening on {url}")).is_err());
        }
    }
}
