//! Private, source-bound observations produced by the native graphics journey.
use crate::acquisition::{
    bios_clock_record, confined, png_pixels, read_bounded, require, source_digest,
};
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::{Path, PathBuf},
};

const SOURCES: &[&str] = &[
    "Cargo.toml",
    "Cargo.lock",
    "examples/mcp_check.rs",
    "examples/mcp_check/runtime_coverage.rs",
    "examples/mcp_check/coverage.rs",
    "src/main.rs",
    "src/capture/png.rs",
    "src/debug/mod.rs",
    "src/debug/input.rs",
    "src/debug/dispatch.rs",
    "src/debug/run.rs",
    "src/debug/state.rs",
    "src/debug/events.rs",
    "src/startup.rs",
    "src/args.rs",
    "src/debug/server.rs",
    "src/debug/control.rs",
    "src/debug/control/service.rs",
    "src/debug/mcp/mod.rs",
    "src/debug/mcp/transport.rs",
    "src/debug/mcp/sessions.rs",
    "src/debug/mcp/diagnostics.rs",
    "examples/mcp_check/native.rs",
    "examples/mcp_check/native/catalog.rs",
    "examples/mcp_check/replay.rs",
    "examples/mcp_check/replay/frontend.rs",
    "tests/fixtures/approved/mcp/manifest.json",
    "tests/fixtures/mcp/mouse.schema.json",
];
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileRef {
    path: PathBuf,
    sha256: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Sample {
    program: String,
    protocol: String,
    round: u8,
    png: FileRef,
    response: FileRef,
    status: FileRef,
    http_status: FileRef,
    http_png: FileRef,
    peer: FileRef,
    discovery: FileRef,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    schema_version: u8,
    revision: String,
    raw_root: PathBuf,
    sources: BTreeMap<String, String>,
    producer: FileRef,
    build_output: FileRef,
    build_artifacts: FileRef,
    samples: Vec<Sample>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BuildReceipt {
    sources: BTreeMap<String, String>,
    producer: String,
    output: FileRef,
    artifacts: FileRef,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MouseCatalog {
    schema_version: u8,
    revision: String,
    raw_root: PathBuf,
    sources: BTreeMap<String, String>,
    native_build: BuildReceipt,
    files: BTreeMap<String, FileRef>,
}

pub(crate) fn verify_mouse_catalog(path: &Path, revision: &str, program: &str) -> io::Result<()> {
    require(
        matches!(program, "seal" | "grafx2"),
        "unapproved native mouse program",
    )?;
    let catalog: MouseCatalog =
        serde_json::from_slice(&read_bounded(path, 1024 * 1024)?).map_err(io::Error::other)?;
    require(
        catalog.schema_version == 1
            && catalog.revision == revision
            && !catalog.files.is_empty()
            && catalog.files.len() <= 128,
        "native mouse catalog scope mismatch",
    )?;
    let root = catalog.raw_root.canonicalize()?;
    require(
        catalog.raw_root.is_absolute()
            && !root.starts_with(Path::new(env!("CARGO_MANIFEST_DIR")).canonicalize()?),
        "native mouse captures must remain private",
    )?;
    verify_producer(&root, &catalog.native_build)?;
    for (source, digest) in &catalog.sources {
        require(
            !Path::new(source).is_absolute(),
            "mouse source path must be relative",
        )?;
        let path = confined(Path::new(env!("CARGO_MANIFEST_DIR")), Path::new(source))?;
        require(source_digest(&path)? == *digest, "mouse source changed")?;
    }
    for reference in [
        &catalog.native_build.output,
        &catalog.native_build.artifacts,
    ] {
        require(
            source_digest(&confined(&root, &reference.path)?)? == reference.sha256,
            "mouse build receipt changed",
        )?;
    }
    build_executable(&read_bounded(
        &confined(&root, &catalog.native_build.artifacts.path)?,
        8 * 1024 * 1024,
    )?)?;
    for reference in catalog.files.values() {
        require(
            source_digest(&confined(&root, &reference.path)?)? == reference.sha256,
            "mouse evidence changed",
        )?;
    }
    let file = |label: &str, name: &str| -> io::Result<PathBuf> {
        let key = format!("{program}_mouse-{label}-{name}");
        let reference = catalog
            .files
            .get(&key)
            .ok_or_else(|| io::Error::other("required mouse witness missing"))?;
        confined(&root, &reference.path)
    };
    let response = |label: &str, name: &str| -> io::Result<serde_json::Value> {
        let value: serde_json::Value =
            serde_json::from_slice(&read_bounded(&file(label, name)?, 128 * 1024)?)
                .map_err(io::Error::other)?;
        require(
            value["isError"] != true && value["structuredContent"].is_object(),
            "mouse SDK result is not successful",
        )?;
        Ok(value["structuredContent"].clone())
    };
    let request = |label: &str, name: &str| -> io::Result<serde_json::Value> {
        serde_json::from_slice(&read_bounded(&file(label, name)?, 128 * 1024)?)
            .map_err(io::Error::other)
    };
    for label in ["legacy", "current"] {
        let before = png_pixels(&read_bounded(
            &file(label, "before.png")?,
            13 * 1024 * 1024,
        )?)?;
        let after = png_pixels(&read_bounded(&file(label, "after.png")?, 13 * 1024 * 1024)?)?;
        let finite = png_pixels(&read_bounded(
            &file(label, "finite-after.png")?,
            13 * 1024 * 1024,
        )?)?;
        let expected = if program == "seal" {
            (640, 480)
        } else {
            (640, 400)
        };
        require(
            before.0 == expected && after.0 == expected && finite.0 == expected,
            "mouse picture dimensions differ",
        )?;
        if program == "seal" {
            crate::acquisition::graphics_effect(program, &before.1, &after.1)?;
            crate::acquisition::graphics_effect(program, &finite.1, &after.1)?;
        } else {
            let shift = if label == "current" { 120 } else { 0 };
            for (y, image) in [(110 + shift, &after.1), (150 + shift, &finite.1)] {
                let offset = (y * 640 + 200) * 3;
                require(
                    before.1[offset..offset + 3] == [0, 0, 0]
                        && image[offset..offset + 3] == [252, 252, 252],
                    "mouse drag did not paint its measured pixel",
                )?;
            }
        }
        let down = response(label, "down-result.json")?;
        require(
            down["acceptance"] == "accepted" && down["hold"]["handle"].is_string(),
            "native mouse claim missing",
        )?;
        for name in ["down", "move", "up", "finite", "click", "relative"] {
            let expected_action = match name {
                "finite" => "drag",
                "relative" => "move",
                other => other,
            };
            let action = request(label, &format!("{name}-request.json"))?;
            let accepted = response(label, &format!("{name}-result.json"))?;
            let queried = request(label, &format!("{name}-operation-request.json"))?;
            let delivered = response(label, &format!("{name}-operation-result.json"))?;
            require(
                action["name"] == "mouse"
                    && action["arguments"]["action"] == expected_action
                    && accepted["acceptance"] == "accepted"
                    && accepted["instance"].is_string()
                    && accepted["op"].as_u64().is_some()
                    && queried["name"] == "operation"
                    && queried["arguments"]["instance"] == accepted["instance"]
                    && queried["arguments"]["op"] == accepted["op"]
                    && delivered["instance"] == accepted["instance"]
                    && delivered["op"] == accepted["op"]
                    && delivered["delivery"] == "delivered",
                "mouse operation does not match its accepted action",
            )?;
            if matches!(name, "move" | "up") {
                require(
                    action["arguments"]["handle"] == down["hold"]["handle"],
                    "mouse motion or release does not match the held button",
                )?;
            }
        }
        let previous = response(label, "relative-before-result.json")?;
        let observed = response(label, "relative-after-result.json")?;
        require(
            relative_motion_matches(&previous, &observed),
            "mouse relative motion or release mismatch",
        )?;
    }
    Ok(())
}

fn relative_motion_matches(previous: &serde_json::Value, observed: &serde_json::Value) -> bool {
    let (Some(x), Some(y), Some(actual_x), Some(actual_y)) = (
        previous["mouse"]["x"].as_i64(),
        previous["mouse"]["y"].as_i64(),
        observed["mouse"]["x"].as_i64(),
        observed["mouse"]["y"].as_i64(),
    ) else {
        return false;
    };
    x.checked_add(5) == Some(actual_x)
        && y.checked_sub(3) == Some(actual_y)
        && observed["mouse"]["buttons"] == 0
}

#[cfg(test)]
mod mouse_evidence_tests {
    use super::relative_motion_matches;
    use serde_json::json;

    #[test]
    fn producer_inventory_covers_verification_helper_source_modules() {
        let sources = super::source_snapshot().unwrap();
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/mcp_check");
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|extension| extension == "rs") {
                let name = path.file_name().unwrap().to_str().unwrap();
                let relative = format!("examples/mcp_check/{name}");
                assert!(
                    sources.contains_key(&relative),
                    "producer inventory omits compiled helper module {relative}"
                );
            }
        }
    }

    #[test]
    fn relative_motion_requires_integer_coordinates_in_both_observations() {
        let previous = json!({"mouse": {"x": 20, "y": 30, "buttons": 0}});
        let observed = json!({"mouse": {"x": 25, "y": 27, "buttons": 0}});
        assert!(relative_motion_matches(&previous, &observed));
        for coordinate in ["x", "y"] {
            for invalid in [serde_json::Value::Null, json!("missing"), json!(1.5)] {
                let mut before = previous.clone();
                let mut after = observed.clone();
                before["mouse"][coordinate] = invalid.clone();
                after["mouse"][coordinate] = invalid;
                assert!(!relative_motion_matches(&before, &after));
            }
            let mut before = previous.clone();
            let mut after = observed.clone();
            before["mouse"].as_object_mut().unwrap().remove(coordinate);
            after["mouse"].as_object_mut().unwrap().remove(coordinate);
            assert!(!relative_motion_matches(&before, &after));
        }
    }

    #[test]
    fn relative_motion_rejects_coordinate_overflow_and_wrong_release() {
        for (x, y) in [(i64::MAX, 30), (20, i64::MIN)] {
            let previous = json!({"mouse": {"x": x, "y": y, "buttons": 0}});
            let observed = json!({"mouse": {"x": 25, "y": 27, "buttons": 0}});
            assert!(!relative_motion_matches(&previous, &observed));
        }
        let previous = json!({"mouse": {"x": 20, "y": 30, "buttons": 0}});
        let observed = json!({"mouse": {"x": 25, "y": 27, "buttons": 1}});
        assert!(!relative_motion_matches(&previous, &observed));
    }
}

pub(crate) fn verify_mouse_evidence(path: &Path, revision: &str) -> io::Result<()> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Evidence {
        schema_version: u8,
        revision: String,
        catalogs: BTreeMap<String, FileRef>,
    }
    let evidence: Evidence =
        serde_json::from_slice(&read_bounded(path, 64 * 1024)?).map_err(io::Error::other)?;
    require(
        evidence.schema_version == 1
            && evidence.revision == revision
            && evidence.catalogs.len() == 2,
        "native mouse evidence scope differs",
    )?;
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).canonicalize()?;
    for program in ["seal", "grafx2"] {
        let reference = evidence
            .catalogs
            .get(program)
            .ok_or_else(|| io::Error::other("native mouse program witness missing"))?;
        require(
            reference.path.is_absolute()
                && !reference.path.canonicalize()?.starts_with(&repository),
            "native mouse catalog must remain private",
        )?;
        require(
            source_digest(&reference.path)? == reference.sha256,
            "native mouse catalog digest differs",
        )?;
        verify_mouse_catalog(&reference.path, revision, program)?;
    }
    Ok(())
}

pub(crate) fn verify_frontend_mouse_evidence(path: &Path, revision: &str) -> io::Result<()> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct JourneyEvidence {
        raw_root: PathBuf,
        files: BTreeMap<String, FileRef>,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Evidence {
        schema_version: u8,
        revision: String,
        journeys: BTreeMap<String, JourneyEvidence>,
    }
    let evidence: Evidence =
        serde_json::from_slice(&read_bounded(path, 64 * 1024)?).map_err(io::Error::other)?;
    require(
        evidence.schema_version == 1
            && evidence.revision == revision
            && evidence.journeys.len() == 2,
        "frontend mouse evidence scope differs",
    )?;
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).canonicalize()?;
    for mode in ["load", "rewind"] {
        let journey = evidence
            .journeys
            .get(mode)
            .ok_or_else(|| io::Error::other("frontend mouse journey missing"))?;
        let root = journey.raw_root.canonicalize()?;
        require(
            journey.raw_root.is_absolute()
                && !root.starts_with(&repository)
                && journey.files.len() <= 128,
            "frontend mouse evidence must remain private and bounded",
        )?;
        for reference in journey.files.values() {
            require(
                source_digest(&confined(&root, &reference.path)?)? == reference.sha256,
                "frontend mouse evidence changed",
            )?;
        }
        let record = |name: &str| -> io::Result<serde_json::Value> {
            let reference = journey
                .files
                .get(name)
                .ok_or_else(|| io::Error::other("required frontend mouse witness missing"))?;
            serde_json::from_slice(&read_bounded(
                &confined(&root, &reference.path)?,
                1024 * 1024,
            )?)
            .map_err(io::Error::other)
        };
        let build: BuildReceipt =
            serde_json::from_value(record("frontend-producer.json")?).map_err(io::Error::other)?;
        verify_producer(&root, &build)?;
        for reference in [&build.output, &build.artifacts] {
            require(
                source_digest(&confined(&root, &reference.path)?)? == reference.sha256,
                "frontend build receipt changed",
            )?;
        }
        build_executable(&read_bounded(
            &confined(&root, &build.artifacts.path)?,
            8 * 1024 * 1024,
        )?)?;
        let before = record(&format!("frontend-before-{mode}.json"))?;
        let restored = record(&format!("frontend-during-{mode}.json"))?;
        require(
            bios_clock_record(&restored)? < bios_clock_record(&before)?,
            "frontend guest clock did not roll back across replacement",
        )?;
        if mode == "load" {
            let slot = journey
                .files
                .get("states/dos/slot1.state")
                .ok_or_else(|| io::Error::other("frontend saved slot witness missing"))?;
            require(
                rust_dos::savestate::slots::read_file_header(&confined(&root, &slot.path)?)
                    .is_some(),
                "frontend saved slot header invalid",
            )?;
            let renewed = record("frontend-mouse-renew-before-load-response.json")?;
            require(
                renewed["isError"] != true
                    && renewed["structuredContent"]["hold"]["remaining_ms"] == 10
                    && renewed["structuredContent"]["hold"]["state"] == "delivered",
                "frontend load lacks the short old-lease expiry setup",
            )?;
        }
        require(
            record("frontend-mouse-down-delivered.json")?["mouse"]["buttons"] == 1,
            "frontend replacement lacks a delivered mouse button",
        )?;
        require(
            record("frontend-settings-mouse-retired.json")?["hold"]["state"] == "reset"
                && record("frontend-settings-mouse-released.json")?["mouse"]["buttons"] == 0,
            "frontend settings did not retire its mouse claim",
        )?;
        require(
            record("frontend-settings-operation-2.json")?["delivery"] == "cancelled",
            "settings did not cancel queued mouse down",
        )?;
        let observed = record(&format!("frontend-mouse-{mode}-operation.json"))?;
        require(
            matches!(
                observed["hold"]["state"].as_str(),
                Some("reset" | "expired")
            ),
            "frontend restore retained mouse authority",
        )?;
        for action in ["up", "renew"] {
            let rejected = record(&format!("frontend-mouse-{mode}-{action}-rejected.json"))?;
            require(
                rejected["isError"] == true
                    && matches!(
                        rejected["structuredContent"]["error"]["code"].as_str(),
                        Some("reset_hold" | "expired_hold" | "stale_instance")
                    ),
                "frontend accepted obsolete mouse control",
            )?;
            require(
                record(&format!("frontend-mouse-{mode}-preserved-{action}.json"))?["mouse"]["buttons"]
                    == 1,
                "old control changed restored mouse state",
            )?;
        }
        let drag = format!("frontend-drag-{mode}");
        let accepted = record(&format!("{drag}-accepted-response.json"))?;
        let queued = record(&format!("{drag}-queued-operation.json"))?;
        let cancelled = record(&format!("{drag}-restored-operation.json"))?;
        require(
            accepted["isError"] != true
                && accepted["structuredContent"]["op"].as_u64().is_some()
                && record(&format!("{drag}-accepted-request.json"))?["arguments"]["action"]
                    == "drag"
                && queued["op"] == accepted["structuredContent"]["op"]
                && cancelled["op"] == queued["op"]
                && queued["state"] == "pending"
                && cancelled["state"] == "cancelled"
                && record(&format!("{drag}-queued-status.json"))?["mouse"]["buttons"] == 0,
            "frontend replacement did not cancel a genuinely queued finite drag",
        )?;
        let before_drag_restore = record(&format!("{drag}-before-release.json"))?;
        let during_drag_restore = record(&format!("{drag}-during-restore.json"))?;
        require(
            bios_clock_record(&during_drag_restore)? < bios_clock_record(&before_drag_restore)?,
            "queued drag witness lacks actual frontend clock rollback",
        )?;
        let restored_mouse = record(&format!("{drag}-restored-status.json"))?;
        require(
            restored_mouse["mouse"]["buttons"] == 1
                && record(&format!("{drag}-after-status.json"))?["mouse"]
                    == restored_mouse["mouse"],
            "obsolete queued drag changed restored mouse state",
        )?;
        if mode == "load" {
            let slot = journey
                .files
                .get("states/dos/slot2.state")
                .ok_or_else(|| io::Error::other("queued drag slot witness missing"))?;
            require(
                rust_dos::savestate::slots::read_file_header(&confined(&root, &slot.path)?)
                    .is_some(),
                "queued drag saved slot header invalid",
            )?;
        }
    }
    Ok(())
}

fn build_executable(output: &[u8]) -> io::Result<PathBuf> {
    let mut executable = None;
    for line in output
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let message: serde_json::Value = serde_json::from_slice(line).map_err(io::Error::other)?;
        if message["reason"] == "compiler-artifact"
            && message["target"]["name"] == "rust-dos"
            && message["target"]["kind"]
                .as_array()
                .is_some_and(|kinds| kinds.iter().any(|kind| kind == "bin"))
        {
            require(executable.is_none(), "duplicate native executable artifact")?;
            let path = message["executable"]
                .as_str()
                .ok_or_else(|| io::Error::other("native executable artifact missing"))?;
            require(
                message["features"]
                    .as_array()
                    .is_some_and(|features| features.iter().any(|feature| feature == "mcp")),
                "native artifact lacks MCP feature",
            )?;
            executable = Some(PathBuf::from(path));
        }
    }
    executable.ok_or_else(|| io::Error::other("Cargo did not report the native executable"))
}

fn source_snapshot() -> io::Result<BTreeMap<String, String>> {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut sources = BTreeMap::new();
    let mut pending = vec![
        repository.join("src"),
        repository.join("crates"),
        repository.join("examples/mcp_check"),
    ];
    for optional in [
        ".cargo",
        "build.rs",
        "rust-toolchain.toml",
        "packaging/windows",
    ] {
        let path = repository.join(optional);
        if path.exists() {
            pending.push(path);
        }
    }
    while let Some(path) = pending.pop() {
        let metadata = std::fs::symlink_metadata(&path)?;
        require(
            !metadata.file_type().is_symlink(),
            "build input cannot be a symlink",
        )?;
        if metadata.is_dir() {
            for entry in std::fs::read_dir(&path)? {
                let entry = entry?;
                if entry.file_name() != "target" && entry.file_name() != ".git" {
                    pending.push(entry.path());
                }
            }
        } else if metadata.is_file() {
            let relative = path.strip_prefix(repository).map_err(io::Error::other)?;
            sources.insert(
                relative.to_string_lossy().replace('\\', "/"),
                source_digest(&path)?,
            );
        }
    }
    for path in SOURCES {
        sources.insert((*path).into(), source_digest(&repository.join(path))?);
    }
    Ok(sources)
}

pub(crate) async fn build_producer(root: &Path) -> io::Result<BuildReceipt> {
    build_producer_mode(root, false).await
}

pub(crate) async fn build_instrumented_producer(root: &Path) -> io::Result<BuildReceipt> {
    build_producer_mode(root, true).await
}

async fn build_producer_mode(root: &Path, instrumented: bool) -> io::Result<BuildReceipt> {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"));
    let sources = source_snapshot()?;
    let mut command = std::process::Command::new("cargo");
    command
        .current_dir(repository)
        .arg(if instrumented {
            "+nightly-2026-10-01"
        } else {
            "+1.98.1"
        })
        .args([
            "build",
            "--locked",
            "--features",
            "mcp",
            "--bin",
            "rust-dos",
            "--quiet",
            "--message-format=json",
        ]);
    if instrumented {
        let mut environment = std::process::Command::new("cargo");
        environment.current_dir(repository).args([
            "+nightly-2026-10-01",
            "llvm-cov",
            "show-env",
            "--branch",
        ]);
        // Use the same dedicated directory as the runtime measurement command.
        environment.env(
            "CARGO_TARGET_DIR",
            repository.join("target/llvm-cov-target"),
        );
        let environment =
            crate::run(environment, std::time::Duration::from_secs(60), 64 * 1024).await?;
        std::fs::write(root.join("coverage-env.log"), &environment.bytes)?;
        let text = std::str::from_utf8(&environment.stdout).map_err(io::Error::other)?;
        let mut found = std::collections::BTreeSet::new();
        for line in text.lines() {
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| io::Error::other("invalid llvm-cov environment record"))?;
            require(
                matches!(
                    key,
                    "LLVM_PROFILE_FILE"
                        | "RUSTC_WRAPPER"
                        | "CARGO_LLVM_COV"
                        | "CARGO_LLVM_COV_SHOW_ENV"
                        | "CARGO_LLVM_COV_TARGET_DIR"
                        | "CARGO_LLVM_COV_BUILD_DIR"
                        | "__CARGO_LLVM_COV_RUSTC_WRAPPER"
                        | "__CARGO_LLVM_COV_RUSTC_WRAPPER_RUSTFLAGS"
                        | "__CARGO_LLVM_COV_RUSTC_WRAPPER_CRATE_NAMES"
                ),
                "unsupported llvm-cov environment key",
            )?;
            require(found.insert(key), "duplicate llvm-cov environment key")?;
            // Decode the tool's literal POSIX quoting as data; never evaluate a shell command.
            let value = if value.starts_with('\'') {
                value
                    .strip_prefix('\'')
                    .and_then(|value| value.strip_suffix('\''))
                    .ok_or_else(|| io::Error::other("invalid llvm-cov literal quoting"))?
                    .replace("'\\''", "'")
            } else {
                value.to_owned()
            };
            if key == "LLVM_PROFILE_FILE" {
                std::fs::write(
                    root.join("coverage-profile.json"),
                    serde_json::to_vec(&value).map_err(io::Error::other)?,
                )?;
            }
            if key == "CARGO_LLVM_COV_BUILD_DIR" {
                command.arg("--target-dir").arg(&value);
            }
            command.env(key, value);
        }
        require(
            found.contains("LLVM_PROFILE_FILE")
                && found.contains("RUSTC_WRAPPER")
                && found.contains("__CARGO_LLVM_COV_RUSTC_WRAPPER_RUSTFLAGS"),
            "llvm-cov instrumentation environment incomplete",
        )?;
    }
    let output = crate::run(
        command,
        std::time::Duration::from_secs(600),
        8 * 1024 * 1024,
    )
    .await?;
    let executable = build_executable(&output.stdout)?;
    let artifacts = PathBuf::from("native-build.jsonl");
    std::fs::write(root.join(&artifacts), output.stdout)?;
    let path = PathBuf::from("native-build.log");
    std::fs::write(root.join(&path), output.bytes)?;
    require(
        source_snapshot()? == sources,
        "native source changed during build",
    )?;
    let producer = source_digest(&executable)?;
    let retained = root.join(if cfg!(windows) {
        "native-rust-dos.exe"
    } else {
        "native-rust-dos"
    });
    std::fs::copy(&executable, &retained)?;
    require(
        source_digest(&retained)? == producer,
        "native build artifact changed during retention",
    )?;
    Ok(BuildReceipt {
        sources,
        producer,
        output: FileRef {
            sha256: source_digest(&root.join(&path))?,
            path,
        },
        artifacts: FileRef {
            sha256: source_digest(&root.join(&artifacts))?,
            path: artifacts,
        },
    })
}

pub(crate) fn finish_instrumented_producer(root: &Path, build: BuildReceipt) -> io::Result<()> {
    verify_producer(root, &build)?;
    let native = root.join(if cfg!(windows) {
        "native-rust-dos.exe"
    } else {
        "native-rust-dos"
    });
    let record = serde_json::json!({"schema_version":1,"sources":build.sources,
        "producer":{"path":native,"sha256":build.producer},"build_output":build.output,"build_artifacts":build.artifacts});
    std::fs::write(
        root.join("coverage-producer.json"),
        serde_json::to_vec_pretty(&record).map_err(io::Error::other)?,
    )
}

pub(crate) fn verify_producer(root: &Path, build: &BuildReceipt) -> io::Result<()> {
    require(
        source_snapshot()? == build.sources,
        "native source changed during acquisition",
    )?;
    let native = root.join(if cfg!(windows) {
        "native-rust-dos.exe"
    } else {
        "native-rust-dos"
    });
    require(
        source_digest(&native)? == build.producer,
        "native producer changed during acquisition",
    )?;
    Ok(())
}

pub(crate) fn verify_build_receipt(root: &Path, build: &BuildReceipt) -> io::Result<()> {
    verify_producer(root, build)?;
    for reference in [&build.output, &build.artifacts] {
        require(
            source_digest(&confined(root, &reference.path)?)? == reference.sha256,
            "native build record changed",
        )?;
    }
    build_executable(&read_bounded(
        &confined(root, &build.artifacts.path)?,
        8 * 1024 * 1024,
    )?)?;
    Ok(())
}

pub(crate) fn write_catalog(root: &Path, revision: &str, build: BuildReceipt) -> io::Result<()> {
    let reference = |path: String| -> io::Result<FileRef> {
        Ok(FileRef {
            sha256: source_digest(&root.join(&path))?,
            path: path.into(),
        })
    };
    let mut samples = Vec::new();
    for program in ["grafx2", "seal"] {
        for protocol in ["legacy", "current"] {
            for round in 0..2 {
                let expected = match (protocol, round) {
                    ("legacy", 0) => "mcp-http-picture",
                    ("legacy", 1) | ("current", 0) => "mcp-changed-1-http-picture",
                    _ => "mcp-changed-2-http-picture",
                };
                samples.push(Sample {
                    program: program.into(),
                    protocol: protocol.into(),
                    round,
                    png: reference(format!("{program}/mcp-{protocol}-{round}-capture.png"))?,
                    response: reference(format!("{program}/mcp-{protocol}-{round}-capture.json"))?,
                    status: reference(format!("{program}/mcp-{protocol}-{round}-status.json"))?,
                    http_status: reference(format!(
                        "{program}/mcp-{protocol}-{round}-http-status.json"
                    ))?,
                    http_png: reference(format!("{program}/{expected}.png"))?,
                    peer: reference(format!("{program}/mcp-{protocol}-peer.json"))?,
                    discovery: reference(format!("{program}/mcp-{protocol}-discovery.json"))?,
                });
            }
        }
    }
    require(
        source_snapshot()? == build.sources,
        "native source changed during acquisition",
    )?;
    let producer = reference(
        if cfg!(windows) {
            "native-rust-dos.exe"
        } else {
            "native-rust-dos"
        }
        .into(),
    )?;
    require(
        producer.sha256 == build.producer,
        "native producer differs from completed build",
    )?;
    let catalog = Catalog {
        schema_version: 4,
        revision: revision.into(),
        raw_root: root.into(),
        sources: build.sources,
        producer,
        build_output: build.output,
        build_artifacts: build.artifacts,
        samples,
    };
    let path = root.join("native-catalog.json");
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&catalog).map_err(io::Error::other)?,
    )?;
    challenge_metadata(&path, revision)
}

/// Corrupt only observations acquired from actual guests; no generated fixture data.
pub(crate) fn challenge_metadata(path: &Path, revision: &str) -> io::Result<()> {
    verify_catalog(path, revision)?;
    let catalog: Catalog =
        serde_json::from_slice(&read_bounded(path, 1024 * 1024)?).map_err(io::Error::other)?;
    let mut killed = 0;
    for sample in catalog.samples {
        if sample.round == 0 {
            let discovery: rmcp::model::ListToolsResult = serde_json::from_slice(&read_bounded(
                &confined(&catalog.raw_root, &sample.discovery.path)?,
                64 * 1024,
            )?)
            .map_err(io::Error::other)?;
            killed += super::challenge_discovery(&discovery)?;
        }
        let read = |file: &FileRef| -> io::Result<rmcp::model::CallToolResult> {
            serde_json::from_slice(&read_bounded(
                &confined(&catalog.raw_root, &file.path)?,
                2 * 1024 * 1024,
            )?)
            .map_err(io::Error::other)
        };
        let status = read(&sample.status)?;
        let http_status: serde_json::Value = serde_json::from_slice(&read_bounded(
            &confined(&catalog.raw_root, &sample.http_status.path)?,
            32 * 1024,
        )?)
        .map_err(io::Error::other)?;
        super::status_parity(&status, &http_status)?;
        for field in ["program", "instructions", "mouse"] {
            let mut damaged = status.clone();
            damaged
                .structured_content
                .as_mut()
                .unwrap()
                .as_object_mut()
                .unwrap()
                .remove(field);
            require(
                super::status_parity(&damaged, &http_status).is_err(),
                "actual status field removal escaped parity validation",
            )?;
            killed += 1;
        }
        for field in ["x", "y"] {
            let mut damaged = status.clone();
            let value = damaged.structured_content.as_ref().unwrap()["mouse"][field]
                .as_i64()
                .unwrap();
            damaged.structured_content.as_mut().unwrap()["mouse"][field] =
                value.wrapping_add(1).into();
            require(
                super::status_parity(&damaged, &http_status).is_err(),
                "actual driver position corruption escaped parity validation",
            )?;
            killed += 1;
        }
        let mut damaged = status.clone();
        let count = damaged.structured_content.as_ref().unwrap()["instructions"]
            .as_u64()
            .unwrap();
        damaged.structured_content.as_mut().unwrap()["instructions"] = count.wrapping_add(1).into();
        require(
            super::status_parity(&damaged, &http_status).is_err(),
            "actual instruction count corruption escaped parity validation",
        )?;
        killed += 1;
        let capture = read(&sample.response)?;
        let dimensions = png_pixels(&read_bounded(
            &confined(&catalog.raw_root, &sample.png.path)?,
            2 * 1024 * 1024,
        )?)?
        .0;
        let (identity, _) = super::observation_metadata(&status, &capture, dimensions)?;
        let rejected = |status: &rmcp::model::CallToolResult,
                        capture: &rmcp::model::CallToolResult|
         -> io::Result<()> {
            let result = super::observation_metadata(status, capture, dimensions).and_then(
                |(observed, _)| {
                    super::require_same_instance(&mut Some(identity.to_owned()), observed)
                },
            );
            require(
                result.is_err(),
                "native metadata corruption survived its protecting oracle",
            )
        };
        for field in ["instance", "req", "state_seq", "journal_seq", "stop_seq"] {
            let mut damaged_status = status.clone();
            damaged_status
                .structured_content
                .as_mut()
                .unwrap()
                .as_object_mut()
                .unwrap()
                .remove(field);
            rejected(&damaged_status, &capture)?;
            let mut damaged_capture = capture.clone();
            damaged_capture
                .structured_content
                .as_mut()
                .unwrap()
                .as_object_mut()
                .unwrap()
                .remove(field);
            rejected(&status, &damaged_capture)?;
            killed += 2;
        }
        let mut missing_status = status.clone();
        let mut missing_capture = capture.clone();
        missing_status
            .structured_content
            .as_mut()
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("instance");
        missing_capture
            .structured_content
            .as_mut()
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("instance");
        rejected(&missing_status, &missing_capture)?;
        let mut changed = identity.as_bytes().to_vec();
        changed[0] = if changed[0] == b'0' { b'1' } else { b'0' };
        let changed = String::from_utf8(changed).map_err(io::Error::other)?;
        missing_status.structured_content.as_mut().unwrap()["instance"] = changed.clone().into();
        missing_capture.structured_content.as_mut().unwrap()["instance"] = changed.into();
        rejected(&missing_status, &missing_capture)?;
        let mut error_status = status.clone();
        error_status.is_error = Some(true);
        rejected(&error_status, &capture)?;
        let mut wrong_dimensions = status.clone();
        wrong_dimensions.structured_content.as_mut().unwrap()["video"]
            .as_object_mut()
            .unwrap()
            .remove("width");
        rejected(&wrong_dimensions, &capture)?;
        let mut reordered = capture.clone();
        reordered.structured_content.as_mut().unwrap()["req"] =
            status.structured_content.as_ref().unwrap()["req"].clone();
        rejected(&status, &reordered)?;
        killed += 5;
    }
    println!("Rejected {killed} minimal corruptions of actual native observation metadata");
    Ok(())
}

pub(crate) fn verify_catalog(path: &Path, revision: &str) -> io::Result<()> {
    let catalog: Catalog =
        serde_json::from_slice(&read_bounded(path, 1024 * 1024)?).map_err(io::Error::other)?;
    require(
        catalog.schema_version == 4 && catalog.revision == revision && catalog.samples.len() == 8,
        "native capture catalog version/revision/sample count mismatch",
    )?;
    let root = catalog.raw_root.canonicalize()?;
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).canonicalize()?;
    require(
        !root.starts_with(&repository) && catalog.raw_root.is_absolute(),
        "native observations must remain outside repository",
    )?;
    require(
        catalog.sources == source_snapshot()?,
        "native source snapshot mismatch",
    )?;
    require(
        source_digest(&confined(&root, &catalog.build_output.path)?)?
            == catalog.build_output.sha256,
        "native build output digest mismatch",
    )?;
    require(
        source_digest(&confined(&root, &catalog.build_artifacts.path)?)?
            == catalog.build_artifacts.sha256,
        "native build artifact output digest mismatch",
    )?;
    build_executable(&read_bounded(
        &confined(&root, &catalog.build_artifacts.path)?,
        8 * 1024 * 1024,
    )?)?;
    require(
        source_digest(&confined(&root, &catalog.producer.path)?)? == catalog.producer.sha256,
        "native producer digest mismatch",
    )?;
    let read = |file: &FileRef| -> io::Result<Vec<u8>> {
        let path = confined(&root, &file.path)?;
        require(
            source_digest(&path)? == file.sha256,
            "native observation digest mismatch",
        )?;
        read_bounded(&path, 2 * 1024 * 1024)
    };
    let mut seen = BTreeSet::new();
    let mut previous = BTreeMap::new();
    let mut instances = BTreeMap::new();
    for sample in catalog.samples {
        require(
            matches!(sample.program.as_str(), "grafx2" | "seal")
                && matches!(sample.protocol.as_str(), "legacy" | "current")
                && sample.round < 2,
            "unknown native observation case",
        )?;
        require(
            seen.insert((
                sample.program.clone(),
                sample.protocol.clone(),
                sample.round,
            )),
            "duplicate native observation case",
        )?;
        let png = read(&sample.png)?;
        let discovery: rmcp::model::ListToolsResult =
            serde_json::from_slice(&read(&sample.discovery)?).map_err(io::Error::other)?;
        super::discovery_contract(&discovery)?;
        let peer: serde_json::Value =
            serde_json::from_slice(&read(&sample.peer)?).map_err(io::Error::other)?;
        require(
            peer["protocolVersion"]
                == if sample.protocol == "legacy" {
                    "2025-11-25"
                } else {
                    "2026-07-28"
                },
            "native negotiated protocol mismatch",
        )?;
        let actual = png_pixels(&png)?;
        let expected = png_pixels(&read(&sample.http_png)?)?;
        super::pixel_parity(&actual, &expected)?;
        require(
            actual.0
                == if sample.program == "grafx2" {
                    (640, 400)
                } else {
                    (640, 480)
                },
            "native program picture dimensions mismatch",
        )?;
        let response: rmcp::model::CallToolResult =
            serde_json::from_slice(&read(&sample.response)?).map_err(io::Error::other)?;
        require(
            response.is_error != Some(true),
            "native image observation failed",
        )?;
        let image = response
            .content
            .iter()
            .find_map(|content| content.as_image())
            .ok_or_else(|| io::Error::other("native image missing"))?;
        require(
            image.mime_type == "image/png"
                && base64::engine::general_purpose::STANDARD
                    .decode(&image.data)
                    .map_err(io::Error::other)?
                    == png,
            "native wire image differs from observed PNG",
        )?;
        let status: rmcp::model::CallToolResult =
            serde_json::from_slice(&read(&sample.status)?).map_err(io::Error::other)?;
        let http_status: serde_json::Value =
            serde_json::from_slice(&read(&sample.http_status)?).map_err(io::Error::other)?;
        super::status_parity(&status, &http_status)?;
        let (identity, request) = super::observation_metadata(&status, &response, actual.0)?;
        super::require_same_instance(
            instances.entry(sample.program.clone()).or_insert(None),
            identity,
        )?;
        let key = (sample.program, sample.protocol);
        if let Some((before, picture)) = previous.insert(key, (request, actual.clone())) {
            require(
                sample.round == 1 && request > before && actual != picture,
                "native observation replay/order mismatch",
            )?;
        } else {
            require(
                sample.round == 0,
                "native observation order missing first frame",
            )?;
        }
    }
    println!(
        "Verified 8 source-bound actual native graphics captures, wire images and paused HTTP pixel oracles"
    );
    Ok(())
}
