//! Verification of actual Codex JSONL calls and separately observed guest effects.
use super::acquisition::{confined, png_pixels, read_bounded, require, source_digest};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Reference {
    path: PathBuf,
    sha256: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Journey {
    program: String,
    root: PathBuf,
    files: BTreeMap<String, Reference>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    schema_version: u8,
    revision: String,
    sources: BTreeMap<String, String>,
    producer_root: PathBuf,
    native_build: super::native::BuildReceipt,
    journeys: Vec<Journey>,
}

fn result(call: &Value) -> io::Result<&Value> {
    require(
        call["status"] == "completed" && call["error"].is_null(),
        "Codex tool failed",
    )?;
    let result = &call["result"];
    require(
        result.is_object() && result["is_error"] != true,
        "Codex guest operation failed",
    )?;
    result
        .get("structured_content")
        .ok_or_else(|| io::Error::other("Codex structured result missing"))
}

fn capture(call: &Value) -> io::Result<(Value, Vec<u8>)> {
    let metadata = result(call)?;
    let content = call["result"]["content"]
        .as_array()
        .ok_or_else(|| io::Error::other("Codex image content missing"))?;
    let images: Vec<_> = content
        .iter()
        .filter(|item| item["type"] == "image")
        .collect();
    require(
        images.len() == 1 && images[0]["mimeType"] == "image/png",
        "Codex capture must contain one PNG",
    )?;
    let encoded = images[0]["data"]
        .as_str()
        .ok_or_else(|| io::Error::other("Codex image bytes missing"))?;
    require(
        encoded.len() <= 13 * 1024 * 1024,
        "Codex image exceeds bound",
    )?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(io::Error::other)?;
    let (size, pixels) = png_pixels(&bytes)?;
    require(
        metadata["width"] == size.0 && metadata["height"] == size.1,
        "Codex image dimensions disagree",
    )?;
    require(
        metadata["mouse_picture"]["instance"] == metadata["instance"],
        "Codex image instance disagrees",
    )?;
    Ok((metadata.clone(), pixels))
}

/// Parses only bounded, actual completed MCP calls; no replacement provider replies.
pub(super) fn verify_records(root: &Path, program: &str) -> io::Result<usize> {
    require(
        matches!(program, "shell" | "grafx2"),
        "unsupported Codex journey",
    )?;
    let bytes = read_bounded(&root.join("client.jsonl"), 16 * 1024 * 1024)?;
    let text = std::str::from_utf8(&bytes).map_err(io::Error::other)?;
    let mut calls = Vec::new();
    let mut completed = false;
    let mut previous_request = 0;
    for line in text.lines() {
        require(calls.len() < 128, "Codex call count exceeds bound")?;
        let event: Value = serde_json::from_str(line).map_err(io::Error::other)?;
        require(
            !matches!(event["type"].as_str(), Some("error" | "turn.failed")),
            "Codex turn failed",
        )?;
        if event["type"] == "turn.completed" {
            completed = true;
        }
        if event["type"] == "item.completed" {
            require(
                matches!(
                    event["item"]["type"].as_str(),
                    Some("mcp_tool_call" | "agent_message" | "reasoning")
                ),
                "Codex used an unexpected execution boundary",
            )?;
        }
        if event["type"] == "item.completed" && event["item"]["type"] == "mcp_tool_call" {
            let call = event["item"].clone();
            require(call["server"] == "rust_dos", "unexpected Codex MCP server")?;
            require(
                matches!(
                    call["tool"].as_str(),
                    Some(
                        "dos_capture"
                            | "dos_status"
                            | "dos_key"
                            | "dos_type"
                            | "dos_mouse"
                            | "dos_operation"
                    )
                ),
                "unexpected Codex tool",
            )?;
            let observed = result(&call)?;
            let request = observed["req"]
                .as_u64()
                .ok_or_else(|| io::Error::other("Codex request identity missing"))?;
            require(
                request > previous_request,
                "Codex response order or identity changed",
            )?;
            previous_request = request;
            if call["tool"] == "dos_capture" {
                capture(&call)?;
            }
            calls.push(call);
        }
    }
    require(
        completed && !calls.is_empty(),
        "Codex completed no verified calls",
    )?;
    require(
        read_bounded(&root.join("client-exit.txt"), 16)? == b"0\n",
        "Codex process failed",
    )?;
    let version = read_bounded(&root.join("client-version.txt"), 128)?;
    require(
        std::str::from_utf8(&version)
            .map_err(io::Error::other)?
            .trim()
            == "codex-cli 0.162.1",
        "unreviewed Codex version",
    )?;

    let down = calls
        .iter()
        .position(|call| call["tool"] == "dos_mouse" && call["arguments"]["action"] == "down")
        .ok_or_else(|| io::Error::other("Codex persistent down missing"))?;
    let down_result = result(&calls[down])?;
    let handle = down_result["hold"]["handle"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| io::Error::other("Codex down handle missing"))?;
    let instance = &down_result["instance"];
    let before = calls[..down]
        .iter()
        .rposition(|call| call["tool"] == "dos_capture")
        .ok_or_else(|| io::Error::other("Codex pre-down observation missing"))?;
    let (before_meta, before_pixels) = capture(&calls[before])?;
    require(
        calls[down]["arguments"]["picture"] == before_meta["mouse_picture"]
            && before_meta["instance"] == *instance,
        "Codex down picture does not match observation",
    )?;
    require(
        calls[down]["arguments"]["lease_ms"] == 30000 && down_result["acceptance"] == "accepted",
        "Codex leased down not accepted",
    )?;

    let delivery =
        |start: usize, end: usize, operation: &Value, held: Option<&str>| -> io::Result<()> {
            require(
                calls[start..end].iter().any(|call| {
                    call["tool"] == "dos_operation"
                        && call["arguments"]["instance"] == *instance
                        && call["arguments"]["op"] == operation["op"]
                        && result(call).is_ok_and(|observed| {
                            observed["delivery"] == "delivered"
                                && observed["instance"] == *instance
                                && held.is_none_or(|state| {
                                    observed["hold"]["state"] == state
                                        && observed["hold"]["handle"] == handle
                                })
                        })
                }),
                "Codex operation delivery missing",
            )
        };
    let find = |start: usize, action: &str| -> io::Result<usize> {
        calls
            .iter()
            .enumerate()
            .skip(start)
            .find_map(|(index, call)| {
                (call["tool"] == "dos_mouse"
                    && call["arguments"]["action"] == action
                    && call["arguments"]["handle"] == handle)
                    .then_some(index)
            })
            .ok_or_else(|| io::Error::other("Codex owned mouse action missing"))
    };
    let renew = find(down + 1, "renew")?;
    let movement = find(renew + 1, "move")?;
    let up = find(movement + 1, "up")?;
    delivery(down + 1, renew, down_result, Some("delivered"))?;
    require(
        calls[down + 1..renew]
            .iter()
            .any(|call| call["tool"] == "dos_capture"),
        "Codex did not observe while held",
    )?;
    let renewal = result(&calls[renew])?;
    require(
        renewal["hold"]["handle"] == handle
            && renewal["hold"]["state"] == "delivered"
            && renewal["hold"]["remaining_ms"] == 30000,
        "Codex lease renewal missing",
    )?;
    let latest = calls[..movement]
        .iter()
        .rposition(|call| call["tool"] == "dos_capture")
        .ok_or_else(|| io::Error::other("Codex motion observation missing"))?;
    let (motion_meta, _) = capture(&calls[latest])?;
    require(
        calls[movement]["arguments"]["picture"] == motion_meta["mouse_picture"],
        "Codex motion picture does not match observation",
    )?;
    delivery(movement + 1, up, result(&calls[movement])?, None)?;
    require(
        calls[movement + 1..up]
            .iter()
            .any(|call| call["tool"] == "dos_capture"),
        "Codex did not observe held drag effect",
    )?;
    let released = result(&calls[up])?;
    require(
        released["hold"]["state"] == "released" && released["hold"]["handle"] == handle,
        "Codex explicit release missing",
    )?;
    delivery(up + 1, calls.len(), released, Some("released"))?;
    let final_index = calls
        .iter()
        .rposition(|call| call["tool"] == "dos_capture")
        .ok_or_else(|| io::Error::other("Codex final capture missing"))?;
    require(final_index > up, "Codex post-release observation missing")?;
    let (final_meta, final_pixels) = capture(&calls[final_index])?;
    require(
        final_meta["instance"] == *instance
            && final_meta["mouse_picture"] == before_meta["mouse_picture"]
            && before_pixels != final_pixels,
        "Codex guest images show no stable-mode effect",
    )?;
    let status: Value =
        serde_json::from_slice(&read_bounded(&root.join("after-status.json"), 64 * 1024)?)
            .map_err(io::Error::other)?;
    require(status["mouse"]["buttons"] == 0, "Codex left input held")?;
    if program == "shell" {
        let text: Value =
            serde_json::from_slice(&read_bounded(&root.join("after-text.json"), 64 * 1024)?)
                .map_err(io::Error::other)?;
        require(
            text["lines"].as_array().is_some_and(|lines| {
                lines.iter().any(|line| {
                    line.as_str().is_some_and(|line| {
                        line.contains("MCP_REPLACED") && !line.contains("MCP DRAG SAMPLE")
                    })
                })
            }),
            "Codex editor selection replacement missing",
        )?;
    } else {
        require(
            final_meta["width"] == 640
                && final_meta["height"] == 400
                && status["program"] == "GRAFX2.EXE",
            "Codex Grafx2 mode/program mismatch",
        )?;
        // The DOS port's recorded coordinate convention is a retained limitation.
        let pixel = (110 * 640 + 350) * 3;
        require(
            before_pixels[pixel..pixel + 3] == [0, 0, 0]
                && final_pixels[pixel..pixel + 3] == [252, 252, 252],
            "Codex did not paint the observed canvas",
        )?;
        let (independent_size, independent_pixels) =
            png_pixels(&read_bounded(&root.join("after.png"), 1024 * 1024)?)?;
        require(
            independent_size == (640, 400)
                && independent_pixels[pixel..pixel + 3] == [252, 252, 252],
            "independent guest capture does not show Codex painting",
        )?;
    }
    Ok(calls.len())
}

pub(super) fn verify_catalog(path: &Path, revision: &str) -> io::Result<()> {
    let catalog: Catalog =
        serde_json::from_slice(&read_bounded(path, 1024 * 1024)?).map_err(io::Error::other)?;
    require(
        catalog.schema_version == 1 && catalog.revision == revision && catalog.journeys.len() == 2,
        "Codex catalog scope mismatch",
    )?;
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).canonicalize()?;
    for source in [
        "Cargo.toml",
        "Cargo.lock",
        "examples/mcp_check.rs",
        "examples/mcp_check/client.rs",
        "src/debug/mcp/mod.rs",
        "src/debug/mcp/mouse.rs",
    ] {
        require(
            catalog.sources.get(source).is_some_and(|digest| {
                source_digest(&repository.join(source)).is_ok_and(|actual| actual == *digest)
            }),
            "Codex source binding missing or stale",
        )?;
    }
    let producer_root = catalog.producer_root.canonicalize()?;
    require(
        catalog.producer_root.is_absolute() && !producer_root.starts_with(&repository),
        "Codex producer must remain private",
    )?;
    super::native::verify_build_receipt(&producer_root, &catalog.native_build)?;
    for (journey, expected) in catalog.journeys.iter().zip(["shell", "grafx2"]) {
        let root = journey.root.canonicalize()?;
        require(
            journey.program == expected
                && journey.root.is_absolute()
                && !root.starts_with(&repository)
                && journey.files.len() <= 32,
            "Codex journey scope mismatch",
        )?;
        for name in [
            "client.jsonl",
            "client-version.txt",
            "client-exit.txt",
            "client-command.json",
            "client-output.log",
            "prompt.txt",
            "after-status.json",
            "after.png",
            "before.png",
            "producer.sha256",
            "rust-dos",
        ] {
            let reference = journey
                .files
                .get(name)
                .ok_or_else(|| io::Error::other("Codex file binding missing"))?;
            let file = confined(&root, &reference.path)?;
            require(
                reference.path == Path::new(name) && source_digest(&file)? == reference.sha256,
                "Codex artifact changed",
            )?;
        }
        let producer = source_digest(&producer_root.join("native-rust-dos"))?;
        require(
            source_digest(&root.join("rust-dos"))? == producer,
            "Codex executable differs from reviewed producer",
        )?;
        if expected == "shell" {
            let reference = journey
                .files
                .get("after-text.json")
                .ok_or_else(|| io::Error::other("Codex text binding missing"))?;
            require(
                source_digest(&confined(&root, &reference.path)?)? == reference.sha256,
                "Codex text oracle changed",
            )?;
        }
        println!(
            "{} actual Codex calls verified for {expected}",
            verify_records(&root, expected)?
        );
    }
    Ok(())
}

pub(super) async fn run_client(
    url: &str,
    root: &Path,
    guest: &Path,
    program: &str,
) -> io::Result<()> {
    use std::{process::Command, time::Duration};
    let client = std::env::var_os("RUST_DOS_CODEX_BIN").unwrap_or_else(|| "codex".into());
    let mut version_command = Command::new(&client);
    version_command.arg("--version");
    let version = crate::run(version_command, Duration::from_secs(10), 8192).await?;
    std::fs::write(root.join("client-version.txt"), &version.stdout)?;
    require(
        std::str::from_utf8(&version.stdout)
            .map_err(io::Error::other)?
            .trim()
            == "codex-cli 0.162.1",
        "unreviewed Codex version",
    )?;
    let setup = if program == "shell" {
        "Capture/read status. Type EDIT then Enter using DOS tools, query delivery, capture readiness and type MCP DRAG SAMPLE. Query delivery and capture. Use pixel (8,40) for down and (128,40) for motion. After release type MCP_REPLACED and capture the visible replacement."
    } else {
        "Type cd APPS\\GRAFX2\\BIN followed by newline, query delivery, then GRAFX2.EXE /mode 320x200 followed by newline and query delivery. Capture/read status until the actual painting canvas is ready, at most40 captures and120seconds. Screenshots can be doubled640x400. Capture the black canvas. Use picture pixels (140,110) for down and (260,110) for motion. After release move the cursor away to(630,390) using a fresh picture and capture the painted line. Do not save files."
    };
    let prompt = format!(
        "Operate this actual DOS guest only through rust_dos MCP tools, no shell/files/other services. Finish within150seconds, no indefinite retries. {setup} For all mouse movement/down use input field picture equal to the exact capture.mouse_picture object, not an input field named mouse_picture. Perform persistent left-button down with lease_ms30000, query dos_operation until delivered, retain the actual returned handle. Capture separately while held, renew that handle to30000, then handle-qualified move with a fresh picture to the specified motion point. Query delivered, capture held drag, explicitly up the samehandle, query delivered and capture released. Finish the setup's post-release action, read status and report only observed effects/limits. Stop on rejected/uncertain input without replay. Never invent pictures/handles/results."
    );
    std::fs::write(root.join("prompt.txt"), &prompt)?;
    let args = vec![
        "exec".to_owned(),
        "--ignore-user-config".into(),
        "--ephemeral".into(),
        "--json".into(),
        "--sandbox".into(),
        "read-only".into(),
        "--skip-git-repo-check".into(),
        "--cd".into(),
        guest
            .to_str()
            .ok_or_else(|| io::Error::other("client guest path must be UTF-8"))?
            .into(),
        "-c".into(),
        format!("mcp_servers.rust_dos.url=\"{url}/mcp\""),
        "-c".into(),
        "mcp_servers.rust_dos.required=true".into(),
        "-c".into(),
        "mcp_servers.rust_dos.tool_timeout_sec=15".into(),
        "-c".into(),
        "mcp_servers.rust_dos.default_tools_approval_mode=\"approve\"".into(),
        prompt,
    ];
    std::fs::write(
        root.join("client-command.json"),
        serde_json::to_vec(&serde_json::json!({"program":client,"args":args}))
            .map_err(io::Error::other)?,
    )?;
    let mut command = Command::new(client);
    command.args(args);
    // Reuse the harness's bounded output/deadline and owned process-tree cleanup.
    let output = crate::execute(command, Duration::from_secs(180), 16 * 1024 * 1024).await?;
    std::fs::write(root.join("client.jsonl"), &output.stdout)?;
    std::fs::write(root.join("client-output.log"), &output.bytes)?;
    std::fs::write(
        root.join("client-exit.txt"),
        if output.successful {
            "0\n"
        } else {
            "nonzero\n"
        },
    )?;
    require(output.successful, "Codex process failed")
}

pub(super) async fn acquire(root: &Path) -> io::Result<()> {
    use std::{process::Command, time::Duration};
    super::replay::validate_root(root)?;
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = repository.join("tests/fixtures/approved/mcp/manifest.json");
    let prepared = super::assets::prepare_packages(
        &manifest,
        &repository.join("docs/mcp/execution/programs.json"),
    )?;
    let guests: Value =
        serde_json::from_slice(&read_bounded(&prepared.join("guests.json"), 64 * 1024)?)
            .map_err(io::Error::other)?;
    let grafx = guests["guests"]
        .as_array()
        .and_then(|items| items.iter().find(|guest| guest["id"] == "grafx2"))
        .and_then(|guest| guest["guest"].as_str())
        .ok_or_else(|| io::Error::other("approved Grafx2 guest missing"))?;
    std::fs::create_dir(root)?;
    let build = super::native::build_producer(root).await?;
    let native = root.join(if cfg!(windows) {
        "native-rust-dos.exe"
    } else {
        "native-rust-dos"
    });
    let mut command = Command::new("git");
    command.args(["rev-parse", "HEAD"]);
    let revision = crate::run(command, Duration::from_secs(10), 8192).await?;
    let revision = std::str::from_utf8(&revision.stdout)
        .map_err(io::Error::other)?
        .trim()
        .to_owned();
    let mut sources = BTreeMap::new();
    for source in [
        "Cargo.toml",
        "Cargo.lock",
        "examples/mcp_check.rs",
        "examples/mcp_check/client.rs",
        "src/debug/mcp/mod.rs",
        "src/debug/mcp/mouse.rs",
    ] {
        sources.insert(source.to_owned(), source_digest(&repository.join(source))?);
    }
    let shell_guest = root.join("shell-guest");
    std::fs::create_dir(&shell_guest)?;
    let mut journeys = Vec::new();
    for (program, guest) in [
        ("shell", shell_guest.as_path()),
        ("grafx2", Path::new(grafx)),
    ] {
        let journey_root = root.join(program);
        std::fs::create_dir(&journey_root)?;
        std::fs::copy(&native, journey_root.join("rust-dos"))?;
        std::fs::write(
            journey_root.join("producer.sha256"),
            source_digest(&native)?,
        )?;
        super::replay::client_guest(&journey_root, &native, guest, program).await?;
        verify_records(&journey_root, program)?;
        let mut files = BTreeMap::new();
        let mut names = vec![
            "client.jsonl",
            "client-version.txt",
            "client-exit.txt",
            "client-command.json",
            "client-output.log",
            "prompt.txt",
            "after-status.json",
            "after.png",
            "before.png",
            "producer.sha256",
            "rust-dos",
        ];
        if program == "shell" {
            names.push("after-text.json");
        }
        for name in names {
            files.insert(
                name.to_owned(),
                Reference {
                    path: name.into(),
                    sha256: source_digest(&journey_root.join(name))?,
                },
            );
        }
        journeys.push(Journey {
            program: program.into(),
            root: journey_root,
            files,
        });
    }
    let catalog = Catalog {
        schema_version: 1,
        revision: revision.clone(),
        sources,
        producer_root: root.into(),
        native_build: build,
        journeys,
    };
    let path = root.join("client-catalog.json");
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&catalog).map_err(io::Error::other)?,
    )?;
    verify_catalog(&path, &revision)
}

pub(super) async fn benchmark(url: &str, root: &Path) -> io::Result<()> {
    use rmcp::model::{CallToolRequestParams, ProtocolVersion};
    use std::time::{Duration, Instant};
    let client = super::native::connect_client(url, ProtocolVersion::V_2025_11_25).await?;
    let mut captures = Vec::with_capacity(24);
    let mut inputs = Vec::with_capacity(12);
    let mut first_instructions = None;
    let mut final_instructions = 0;
    for round in 0..28 {
        let start = Instant::now();
        let capture = tokio::time::timeout(
            Duration::from_secs(10),
            client.call_tool(CallToolRequestParams::new("dos_capture")),
        )
        .await
        .map_err(io::Error::other)?
        .map_err(io::Error::other)?;
        let elapsed = u64::try_from(start.elapsed().as_nanos()).map_err(io::Error::other)?;
        require(capture.is_error != Some(true), "benchmark capture failed")?;
        let image = capture
            .content
            .iter()
            .find_map(|item| item.as_image())
            .ok_or_else(|| io::Error::other("benchmark actual image missing"))?;
        let png = base64::engine::general_purpose::STANDARD
            .decode(&image.data)
            .map_err(io::Error::other)?;
        require(
            png_pixels(&png)?.0 == (640, 400),
            "benchmark guest mode changed",
        )?;
        std::fs::write(
            root.join(format!("capture-{round}.json")),
            serde_json::to_vec(&capture).map_err(io::Error::other)?,
        )?;
        if round >= 4 {
            captures.push(elapsed);
        }
        if round % 2 == 0 {
            let mut request = CallToolRequestParams::new("dos_key");
            request.arguments = Some(
                serde_json::json!({"key":"shift","hold_ms":20})
                    .as_object()
                    .unwrap()
                    .clone(),
            );
            std::fs::write(
                root.join(format!("input-{round}-request.json")),
                serde_json::to_vec(&request).map_err(io::Error::other)?,
            )?;
            let started = Instant::now();
            let accepted = tokio::time::timeout(Duration::from_secs(10), client.call_tool(request))
                .await
                .map_err(io::Error::other)?
                .map_err(io::Error::other)?;
            require(accepted.is_error != Some(true), "benchmark input rejected")?;
            let value = accepted
                .structured_content
                .as_ref()
                .ok_or_else(|| io::Error::other("benchmark acceptance missing"))?;
            std::fs::write(
                root.join(format!("input-{round}-accepted.json")),
                serde_json::to_vec(&accepted).map_err(io::Error::other)?,
            )?;
            let mut query = CallToolRequestParams::new("dos_operation");
            query.arguments = Some(
                serde_json::json!({"instance":value["instance"],"op":value["op"]})
                    .as_object()
                    .unwrap()
                    .clone(),
            );
            let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
            loop {
                let reply = tokio::time::timeout_at(deadline, client.call_tool(query.clone()))
                    .await
                    .map_err(io::Error::other)?
                    .map_err(io::Error::other)?;
                require(
                    reply.is_error != Some(true),
                    "benchmark delivery query failed",
                )?;
                let delivery = reply
                    .structured_content
                    .as_ref()
                    .ok_or_else(|| io::Error::other("benchmark delivery missing"))?;
                if delivery["delivery"] == "delivered" {
                    std::fs::write(
                        root.join(format!("input-{round}-delivered.json")),
                        serde_json::to_vec(&reply).map_err(io::Error::other)?,
                    )?;
                    break;
                }
                require(delivery["delivery"] == "pending", "benchmark input failed")?;
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
            if round >= 4 {
                inputs.push(u64::try_from(started.elapsed().as_nanos()).map_err(io::Error::other)?);
            }
        }
        let status = tokio::time::timeout(
            Duration::from_secs(10),
            client.call_tool(CallToolRequestParams::new("dos_status")),
        )
        .await
        .map_err(io::Error::other)?
        .map_err(io::Error::other)?;
        require(status.is_error != Some(true), "benchmark status failed")?;
        let value = status
            .structured_content
            .as_ref()
            .ok_or_else(|| io::Error::other("benchmark actual status missing"))?;
        let instructions = value["instructions"]
            .as_u64()
            .ok_or_else(|| io::Error::other("benchmark guest progress missing"))?;
        first_instructions.get_or_insert(instructions);
        final_instructions = instructions;
        std::fs::write(
            root.join(format!("status-{round}.json")),
            serde_json::to_vec(&status).map_err(io::Error::other)?,
        )?;
    }
    require(
        captures.len() == 24
            && inputs.len() == 12
            && final_instructions > first_instructions.unwrap_or(u64::MAX),
        "benchmark did not execute its actual workload",
    )?;
    std::fs::write(root.join("measurements.json"), serde_json::to_vec_pretty(&serde_json::json!({"schema_version":1,"sdk":"rmcp 3.5.1","protocol":"2025-11-25","unit":"nanoseconds","warmup_rounds":4,"capture_roundtrip":captures,"key_acceptance_to_delivered":inputs,"first_instructions":first_instructions,"final_instructions":final_instructions})).map_err(io::Error::other)?)?;
    client.cancel().await.map_err(io::Error::other)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    #[test]
    #[ignore = "requires retained real Codex editor, Grafx2 and rejected-input journeys"]
    fn actual_client_images_holds_drag_and_rejection() {
        let actual =
            |variable| PathBuf::from(std::env::var_os(variable).expect("actual evidence path"));
        assert!(
            super::verify_records(&actual("RUST_DOS_CLIENT_SHELL_RECORDS"), "shell").unwrap() > 0
        );
        assert!(
            super::verify_records(&actual("RUST_DOS_CLIENT_GRAFX_RECORDS"), "grafx2").unwrap() > 0
        );
        let rejected = super::verify_records(&actual("RUST_DOS_CLIENT_REJECTED_RECORDS"), "shell")
            .unwrap_err();
        assert_eq!(rejected.to_string(), "Codex tool failed");
    }
}
