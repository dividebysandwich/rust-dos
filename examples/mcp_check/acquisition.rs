//! Verify actual provider captures before publication approval.
use base64::Engine;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::{self, Read},
    path::{Component, Path, PathBuf},
};

const MAX_RECORD: usize = 1024 * 1024;
const MAX_TOTAL: usize = 32 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    schema_version: u32,
    revision: String,
    raw_root: PathBuf,
    sources: BTreeMap<String, String>,
    files: BTreeMap<String, Record>,
    #[serde(default)]
    native_producer: Option<Producer>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Producer {
    path: PathBuf,
    sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    path: PathBuf,
    sha256: String,
}

pub(super) fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
pub(super) fn require(condition: bool, message: &'static str) -> io::Result<()> {
    if condition {
        Ok(())
    } else {
        Err(invalid(message))
    }
}
pub(super) fn bios_clock_record(value: &Value) -> io::Result<u32> {
    let bytes = value["hex"]
        .as_str()
        .ok_or_else(|| invalid("BIOS clock missing"))?
        .split_whitespace()
        .map(|byte| u8::from_str_radix(byte, 16).map_err(io::Error::other))
        .collect::<io::Result<Vec<_>>>()?;
    let bytes: [u8; 4] = bytes
        .try_into()
        .map_err(|_| invalid("BIOS clock width mismatch"))?;
    Ok(u32::from_le_bytes(bytes))
}

pub(super) fn hex_digest(hash: Sha256) -> String {
    hash.finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(super) fn confined(root: &Path, relative: &Path) -> io::Result<PathBuf> {
    require(!relative.as_os_str().is_empty(), "empty capture path")?;
    let mut path = root.to_owned();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            return Err(invalid("capture path must stay within its root"));
        };
        path.push(part);
        require(
            !std::fs::symlink_metadata(&path)?.file_type().is_symlink(),
            "capture symlink refused",
        )?;
    }
    Ok(path)
}

pub(super) fn read_bounded(path: &Path, limit: usize) -> io::Result<Vec<u8>> {
    let metadata = std::fs::symlink_metadata(path)?;
    require(
        metadata.is_file() && metadata.len() > 0 && metadata.len() <= limit as u64,
        "capture is empty, non-file or oversized",
    )?;
    let file = std::fs::File::open(path)?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(metadata.len() as usize)
        .map_err(io::Error::other)?;
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    require(
        !bytes.is_empty() && bytes.len() <= limit,
        "capture changed size while reading",
    )?;
    Ok(bytes)
}

pub(super) fn source_digest(path: &Path) -> io::Result<String> {
    let metadata = std::fs::symlink_metadata(path)?;
    require(
        metadata.is_file() && metadata.len() > 0 && metadata.len() <= 512 * 1024 * 1024,
        "invalid producer source",
    )?;
    let mut file = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 8192];
    let mut total = 0usize;
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total = total
            .checked_add(count)
            .ok_or_else(|| invalid("producer size overflow"))?;
        require(
            total <= 512 * 1024 * 1024,
            "producer source grew past its bound",
        )?;
        hash.update(&buffer[..count]);
    }
    Ok(hex_digest(hash))
}

pub(super) fn verify_source(path: &Path, expected: &str) -> io::Result<()> {
    require(
        source_digest(path)? == expected,
        "producer changed: reacquisition required",
    )
}

fn png_size(bytes: &[u8]) -> io::Result<(u32, u32)> {
    Ok(png_pixels(bytes)?.0)
}

pub(super) fn png_pixels(bytes: &[u8]) -> io::Result<((u32, u32), Vec<u8>)> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_limits(png::Limits {
        bytes: 16 * 1024 * 1024,
    });
    let mut reader = decoder.read_info().map_err(io::Error::other)?;
    let info = reader.info();
    let size = (info.width, info.height);
    require(
        size.0 > 0
            && size.1 > 0
            && size.0 <= 2048
            && size.1 <= 2048
            && info.color_type == png::ColorType::Rgb
            && info.bit_depth == png::BitDepth::Eight,
        "capture must be bounded RGB24 PNG",
    )?;
    let length = reader
        .output_buffer_size()
        .ok_or_else(|| invalid("PNG decoded size overflow"))?;
    require(length <= 12 * 1024 * 1024, "PNG decoded byte bound")?;
    let mut pixels = Vec::new();
    pixels.try_reserve_exact(length).map_err(io::Error::other)?;
    pixels.resize(length, 0);
    reader.next_frame(&mut pixels).map_err(io::Error::other)?;
    reader.finish().map_err(io::Error::other)?;
    Ok((size, pixels))
}

fn load_catalog(path: &Path, revision: &str) -> io::Result<(Catalog, BTreeMap<String, Vec<u8>>)> {
    let catalog: Catalog =
        serde_json::from_slice(&read_bounded(path, 64 * 1024)?).map_err(io::Error::other)?;
    require(
        catalog.schema_version == 1 && catalog.revision == revision,
        "unsupported or stale acquisition catalog",
    )?;
    require(
        catalog.raw_root.is_absolute()
            && catalog.raw_root.is_dir()
            && catalog.sources.len() <= 32
            && catalog.files.len() <= 64,
        "invalid acquisition roots/counts",
    )?;
    let raw_root = catalog.raw_root.canonicalize()?;
    let producer = Path::new(env!("CARGO_MANIFEST_DIR")).canonicalize()?;
    for required in [
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
        require(
            catalog.sources.contains_key(required),
            "missing producer source binding",
        )?;
    }
    let binary = if cfg!(windows) {
        "target/debug/rust-dos.exe"
    } else {
        "target/debug/rust-dos"
    };
    if let Some(native) = &catalog.native_producer {
        require(
            native.path.is_absolute(),
            "native producer must be an absolute path",
        )?;
        verify_source(&native.path, &native.sha256)?;
    } else {
        require(
            catalog.sources.contains_key(binary),
            "missing actual native producer binary",
        )?;
    }
    for (path, hash) in &catalog.sources {
        verify_source(&confined(&producer, Path::new(path))?, hash)?;
    }
    let mut files = BTreeMap::new();
    let mut total = 0usize;
    for (name, record) in &catalog.files {
        let bytes = read_bounded(&confined(&raw_root, &record.path)?, MAX_RECORD)?;
        total = total
            .checked_add(bytes.len())
            .ok_or_else(|| invalid("capture size overflow"))?;
        require(total <= MAX_TOTAL, "aggregate acquisition byte bound")?;
        let mut hash = Sha256::new();
        hash.update(&bytes);
        require(hex_digest(hash) == record.sha256, "capture hash mismatch")?;
        files.insert(name.clone(), bytes);
    }
    Ok((catalog, files))
}

pub(super) fn verify_graphics(path: &Path, revision: &str) -> io::Result<()> {
    let (catalog, files) = load_catalog(path, revision)?;
    let manifest_path = Path::new("tests/fixtures/approved/mcp/manifest.json");
    require(
        catalog
            .sources
            .contains_key("tests/fixtures/approved/mcp/manifest.json"),
        "graphics acquisition manifest binding missing",
    )?;
    super::assets::approvals(manifest_path)?;
    let manifest: Value = serde_json::from_slice(&read_bounded(manifest_path, 64 * 1024)?)
        .map_err(io::Error::other)?;
    let get = |name: &str| -> io::Result<&[u8]> {
        files
            .get(name)
            .map(Vec::as_slice)
            .ok_or_else(|| invalid("required graphics witness missing"))
    };
    for (program, native_size, frame_size, held_position, released_position) in [
        ("seal", (640, 480), (640, 480), (2080, 592), (2560, 1440)),
        ("grafx2", (320, 200), (640, 400), (80, 50), (120, 60)),
    ] {
        let json = |suffix: &str| -> io::Result<Value> {
            serde_json::from_slice(get(&format!("{program}_{suffix}"))?).map_err(io::Error::other)
        };
        let held: Value =
            serde_json::from_slice(get(&format!("{program}_held"))?).map_err(io::Error::other)?;
        let released: Value = serde_json::from_slice(get(&format!("{program}_released"))?)
            .map_err(io::Error::other)?;
        require(
            held["instance"].as_str().is_some_and(|id| !id.is_empty())
                && held["instance"] == released["instance"],
            "graphics instance mismatch",
        )?;
        for (status, buttons, position) in
            [(&held, 1, held_position), (&released, 0, released_position)]
        {
            require(
                status["mouse"]["installed"] == true
                    && status["program"]
                        == if program == "seal" {
                            "SEAL.EXE"
                        } else {
                            "GRAFX2.EXE"
                        }
                    && status["mouse"]["buttons"] == buttons
                    && status["mouse"]["x"] == position.0
                    && status["mouse"]["y"] == position.1
                    && status["video"]["width"] == native_size.0
                    && status["video"]["height"] == native_size.1
                    && status["video"]["frame"]["width"] == frame_size.0
                    && status["video"]["frame"]["height"] == frame_size.1,
                "graphics mode or driver transition mismatch",
            )?;
        }
        let (before_size, before) = png_pixels(get(&format!("{program}_before_png"))?)?;
        let (after_size, after) = png_pixels(get(&format!("{program}_after_png"))?)?;
        require(
            before_size == frame_size && after_size == frame_size,
            "graphics capture dimensions mismatch",
        )?;
        graphics_effect(program, &before, &after)?;
        let moved = json("moved_held")?;
        require(
            moved["instance"] == held["instance"]
                && moved["mouse"]["buttons"] == 1
                && moved["mouse"]["x"] == released_position.0
                && moved["mouse"]["y"] == released_position.1,
            "incremental movement lost held button",
        )?;
        let finite = json("finite_released")?;
        let finite_position = if program == "seal" {
            held_position
        } else {
            (120, 80)
        };
        require(
            finite["instance"] == held["instance"]
                && finite["mouse"]["buttons"] == 0
                && finite["mouse"]["x"] == finite_position.0
                && finite["mouse"]["y"] == finite_position.1,
            "finite drag release or position mismatch",
        )?;
        let image = |suffix: &str| -> io::Result<Vec<u8>> {
            let (size, pixels) = png_pixels(get(&format!("{program}_{suffix}_png"))?)?;
            require(size == frame_size, "mouse matrix image dimensions mismatch")?;
            Ok(pixels)
        };
        let finite_before = image("finite_before")?;
        let finite_after = image("finite_after")?;
        let click_before = image("click_before")?;
        let click_after = image("click_after")?;
        if program == "seal" {
            graphics_effect("seal", &finite_after, &finite_before)?;
            let desktop = image("desktop")?;
            require(
                (65..83).any(|row| {
                    let offset = (row * 640 + 70) * 3;
                    desktop[offset..offset + 30 * 3] != before[offset..offset + 30 * 3]
                }),
                "independent clicked window opening missing",
            )?;
        } else {
            painted_pixel(&finite_before, &finite_after, 200, 150)?;
            painted_pixel(&click_before, &click_after, 280, 180)?;
        }
        let clicked = json("clicked")?;
        let clicked_position = if program == "seal" {
            (3600, 2400)
        } else {
            (140, 90)
        };
        require(
            clicked["instance"] == held["instance"]
                && clicked["mouse"]["buttons"] == 0
                && clicked["mouse"]["x"] == clicked_position.0
                && clicked["mouse"]["y"] == clicked_position.1,
            "click driver position/release mismatch",
        )?;
        let relative_request = json("relative_request")?;
        require(
            relative_request["action"] == "move"
                && relative_request["dx"] == 5
                && relative_request["dy"] == -3
                && relative_request.get("x").is_none()
                && relative_request.get("y").is_none(),
            "canonical relative motion missing",
        )?;
        let relative = json("relative_status")?;
        require(
            relative["instance"] == held["instance"]
                && relative["mouse"]["buttons"] == 0
                && relative["mouse"]["x"] == clicked_position.0 + 5
                && relative["mouse"]["y"] == clicked_position.1 - 3,
            "relative driver displacement mismatch",
        )?;
        let (start_picture, end_picture) = if program == "seal" {
            ((260, 74), (320, 180))
        } else {
            ((80, 100), (120, 120))
        };
        for (name, action, position) in [
            ("down_request", "down", start_picture),
            ("move_request", "move", end_picture),
        ] {
            let request = json(name)?;
            require(
                request["action"] == action
                    && request["coords"] == "screen"
                    && request["x"] == position.0
                    && request["y"] == position.1,
                "canonical picture mouse request missing",
            )?;
        }
        require(
            json("up_request")?["action"] == "up" && json("up_request")?["button"] == "left",
            "canonical mouse release missing",
        )?;
        let finite_request = json("finite_request")?;
        let events = finite_request["events"]
            .as_array()
            .ok_or_else(|| invalid("finite drag events missing"))?;
        let (finite_start, finite_end) = if program == "seal" {
            (end_picture, start_picture)
        } else {
            ((80, 140), (120, 160))
        };
        require(
            events.len() == 6
                && events[0]["type"] == "mouse"
                && events[0]["action"] == "down"
                && events[0]["button"] == "left"
                && events[0]["coords"] == "screen"
                && events[0]["x"] == finite_start.0
                && events[0]["y"] == finite_start.1
                && events[1]["type"] == "wait"
                && events[1]["ms"] == 150
                && events[2]["type"] == "mouse"
                && events[2]["action"] == "move"
                && events[2]["coords"] == "screen"
                && events[2]["x"] == finite_end.0
                && events[2]["y"] == finite_end.1
                && events[3]["type"] == "wait"
                && events[3]["ms"] == 300
                && events[4]["type"] == "mouse"
                && events[4]["action"] == "up"
                && events[4]["button"] == "left"
                && events[5]["type"] == "wait"
                && events[5]["ms"] == 100,
            "canonical finite drag order missing",
        )?;
        let click_request = json("click_request")?;
        let click_picture = if program == "seal" {
            (450, 300)
        } else {
            (140, 180)
        };
        require(
            click_request["action"] == "click"
                && click_request["button"] == "left"
                && click_request["coords"] == "screen"
                && click_request["x"] == click_picture.0
                && click_request["y"] == click_picture.1,
            "canonical click request missing",
        )?;
        let version = json("version")?;
        require(
            version["name"] == "rust-dos"
                && version["version"] == env!("CARGO_PKG_VERSION")
                && version["build"]["commit"] == revision,
            "graphics native provider version mismatch",
        )?;
        let journal = json("journal")?;
        let mut requests = std::collections::BTreeSet::new();
        if program == "seal" {
            let opened = json("open_request")?;
            let clicks = opened["events"]
                .as_array()
                .ok_or_else(|| invalid("window-opening click requests missing"))?;
            require(
                clicks.len() == 4
                    && [0, 2].iter().all(|&index| {
                        clicks[index]["type"] == "mouse"
                            && clicks[index]["action"] == "click"
                            && clicks[index]["x"] == 54
                            && clicks[index]["y"] == 65
                            && clicks[index]["coords"] == "screen"
                    }),
                "canonical window-opening clicks missing",
            )?;
            requests.insert(delivery_witness(&journal, &json("open")?, 4)?);
        }
        for (reply, events) in [
            ("down", 1),
            ("move", 1),
            ("up", 1),
            ("finite", 6),
            ("click", 1),
            ("relative", 1),
        ] {
            require(
                requests.insert(delivery_witness(&journal, &json(reply)?, events)?),
                "duplicate graphics request identity",
            )?;
        }
        let configuration = get(&format!("{program}_configuration"))?;
        let mut hash = Sha256::new();
        hash.update(configuration);
        let approved = manifest["programs"]
            .as_array()
            .and_then(|programs| programs.iter().find(|entry| entry["id"] == program))
            .ok_or_else(|| invalid("approved graphics configuration missing"))?;
        require(
            Some(hex_digest(hash).as_str()) == approved["configuration"]["sha256"].as_str(),
            "graphics configuration pin mismatch",
        )?;
    }
    Ok(())
}

fn delivery_witness(journal: &Value, reply: &Value, count: u64) -> io::Result<u64> {
    require(
        reply["ok"] == true && reply["done"] == true,
        "actual graphics delivery outcome missing",
    )?;
    let request = reply["req"]
        .as_u64()
        .ok_or_else(|| invalid("graphics request identity missing"))?;
    let entries = journal["entries"]
        .as_array()
        .ok_or_else(|| invalid("graphics journal missing"))?;
    let queued = entries
        .iter()
        .find(|entry| entry["type"] == "input_queued" && entry["cause"] == request)
        .ok_or_else(|| invalid("graphics queued witness missing"))?;
    let delivered = entries
        .iter()
        .find(|entry| entry["type"] == "input_delivered" && entry["cause"] == request)
        .ok_or_else(|| invalid("graphics delivered witness missing"))?;
    require(
        queued["events"] == count
            && queued["seq"]
                .as_u64()
                .zip(delivered["seq"].as_u64())
                .is_some_and(|(queued, delivered)| queued < delivered),
        "graphics delivery order or event count mismatch",
    )?;
    Ok(request)
}

fn painted_pixel(before: &[u8], after: &[u8], x: usize, y: usize) -> io::Result<()> {
    require(
        before.len() == 640 * 400 * 3 && after.len() == before.len() && x < 640 && y < 400,
        "doubled graphics pixel length/position mismatch",
    )?;
    let offset = (y * 640 + x) * 3;
    require(
        before[offset..offset + 3] == [0, 0, 0] && after[offset..offset + 3] == [252, 252, 252],
        "independent canvas painting missing",
    )
}

pub(super) fn graphics_effect(program: &str, before: &[u8], after: &[u8]) -> io::Result<()> {
    match program {
        "seal" => {
            require(
                before.len() == 640 * 480 * 3 && after.len() == before.len(),
                "standard graphics pixel length mismatch",
            )?;
            let region = |pixels: &[u8], x: usize, y: usize| -> Vec<u8> {
                (y..y + 18)
                    .flat_map(|row| {
                        pixels[(row * 640 + x) * 3..(row * 640 + x + 30) * 3]
                            .iter()
                            .copied()
                    })
                    .collect()
            };
            let original = region(before, 70, 65);
            require(
                original == region(after, 130, 171) && original != region(after, 70, 65),
                "independent window movement missing",
            )
        }
        "grafx2" => painted_pixel(before, after, 200, 110),
        _ => Err(invalid("unknown graphics oracle")),
    }
}

pub(super) fn verify(path: &Path, revision: &str) -> io::Result<()> {
    verify_shell_and_protocol(path, revision, true)
}

pub(super) fn verify_public_assets(root: &Path) -> io::Result<()> {
    let mut files = std::fs::read_dir(root)?;
    let entry = files
        .next()
        .transpose()?
        .ok_or_else(|| invalid("public acquisition manifest missing"))?;
    require(
        entry.file_name() == "manifest.json"
            && entry.file_type()?.is_file()
            && files.next().transpose()?.is_none(),
        "only acquisition metadata may be durable",
    )
}

pub(super) fn challenge(path: &Path, revision: &str, graphics: bool) -> io::Result<()> {
    let check = |path: &Path| {
        if graphics {
            verify_graphics(path, revision)
        } else {
            verify(path, revision)
        }
    };
    check(path)?;
    let original: Value =
        serde_json::from_slice(&read_bounded(path, 64 * 1024)?).map_err(io::Error::other)?;
    let root = PathBuf::from(
        original["raw_root"]
            .as_str()
            .ok_or_else(|| invalid("challenge root missing"))?,
    );
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    let scratch = root.join(format!(
        "integrity-challenge-{}-{nonce}",
        std::process::id()
    ));
    super::replay::validate_root(&scratch)?;
    std::fs::create_dir(&scratch)?;
    let result = (|| {
        let file = scratch.join("catalog.json");
        let mut count = 0usize;
        let mut reject = |value: Value| -> io::Result<()> {
            std::fs::write(&file, serde_json::to_vec(&value).map_err(io::Error::other)?)?;
            if check(&file).is_ok() {
                let missing = original["files"]
                    .as_object()
                    .into_iter()
                    .flat_map(|files| files.keys())
                    .find(|name| value["files"].get(*name).is_none());
                return Err(io::Error::other(format!(
                    "invalid capture mutation survived; missing record: {missing:?}"
                )));
            }
            count += 1;
            Ok(())
        };
        let mut changed = original.clone();
        changed["revision"] = Value::String("stale-revision".to_owned());
        reject(changed)?;
        let mut changed = original.clone();
        changed["schema_version"] = Value::from(2);
        reject(changed)?;
        let mut changed = original.clone();
        changed["raw_root"] = Value::String("relative-root".to_owned());
        reject(changed)?;
        for name in original["sources"]
            .as_object()
            .ok_or_else(|| invalid("challenge sources missing"))?
            .keys()
        {
            let mut changed = original.clone();
            changed["sources"][name] = Value::String("0".repeat(64));
            reject(changed)?;
        }
        for name in original["files"]
            .as_object()
            .ok_or_else(|| invalid("challenge files missing"))?
            .keys()
        {
            let mut changed = original.clone();
            changed["files"]
                .as_object_mut()
                .ok_or_else(|| invalid("challenge files missing"))?
                .remove(name);
            reject(changed)?;
            let mut changed = original.clone();
            changed["files"][name]["sha256"] = Value::String("0".repeat(64));
            reject(changed)?;
        }
        let mut changed = original.clone();
        changed["native_producer"]["sha256"] = Value::String("0".repeat(64));
        reject(changed)?;
        let mut changed = original.clone();
        changed["files"]["escape"] =
            serde_json::json!({"path":"../Cargo.toml","sha256":"0".repeat(64)});
        reject(changed)?;
        println!(
            "Rejected {count} mutations of actual {} acquisition metadata",
            if graphics { "graphics" } else { "shell/SDK" }
        );
        Ok(())
    })();
    std::fs::remove_dir_all(scratch)?;
    result
}

pub(super) fn verify_shell(path: &Path, revision: &str) -> io::Result<()> {
    verify_shell_and_protocol(path, revision, false)
}

fn verify_shell_and_protocol(path: &Path, revision: &str, protocol: bool) -> io::Result<()> {
    let (catalog, files) = load_catalog(path, revision)?;
    let raw_root = catalog.raw_root.canonicalize()?;
    let get = |name: &str| -> io::Result<&[u8]> {
        files
            .get(name)
            .map(Vec::as_slice)
            .ok_or_else(|| invalid("required acquired witness missing"))
    };
    let json = |name: &str| -> io::Result<Value> {
        serde_json::from_slice(get(name)?).map_err(io::Error::other)
    };
    let status = json("shell_status")?;
    let version = json("native_version")?;
    require(
        version["name"] == "rust-dos"
            && version["version"] == env!("CARGO_PKG_VERSION")
            && version["build"]["commit"] == revision,
        "native provider version mismatch",
    )?;
    let dimensions = png_size(get("shell_png")?)?;
    require(
        status["shell_idle"] == true
            && status["video"]["mode"] == 3
            && status["video"]["frame"]["width"] == dimensions.0
            && status["video"]["frame"]["height"] == dimensions.1,
        "shell frame/status mismatch",
    )?;
    require(
        json("shell_text")?["lines"]
            .as_array()
            .is_some_and(|lines| lines.iter().any(|line| line == "C:\\>")),
        "actual shell prompt missing",
    )?;
    let typed = json("echo_request")?;
    let echoed = typed["text"]
        .as_str()
        .and_then(|text| text.strip_prefix("echo "))
        .ok_or_else(|| invalid("canonical echo request missing"))?;
    require(
        json("echo_text")?["lines"]
            .as_array()
            .is_some_and(|lines| lines.iter().any(|line| line == echoed)),
        "independent echo output missing",
    )?;
    require(
        json("enter_request")?["key"] == "enter",
        "canonical enter request missing",
    )?;
    for name in [
        "echo_response",
        "enter_response",
        "shift_response",
        "mouse_down_response",
        "mouse_up_response",
    ] {
        let reply = json(name)?;
        require(
            reply["ok"] == true && reply["done"] == true,
            "actual input delivery outcome missing",
        )?;
    }
    let shift = json("shift_request")?;
    require(
        shift["key"] == "shift" && shift["action"] == "down",
        "canonical Shift hold missing",
    )?;
    for name in ["held_flags", "restored_flags", "resumed_flags"] {
        let flags = json(name)?;
        require(
            flags["addr"] == "00417" && flags["len"] == 2 && flags["hex"] == "02 00",
            "held/restored Shift observation missing",
        )?;
    }
    require(
        json("released_flags")?["hex"] == "00 00",
        "released Shift observation missing",
    )?;
    for name in [
        "held_mouse",
        "restored_mouse",
        "resumed_mouse",
        "moved_mouse",
    ] {
        require(
            json(name)?["mouse"]["buttons"] == 1,
            "held/restored mouse observation missing",
        )?;
    }
    require(
        json("released_mouse")?["mouse"]["buttons"] == 0
            && json("restored_mouse")?["paused"] == true,
        "released/paused restore observation missing",
    )?;
    require(
        json("moved_mouse")?["mouse"]["x"] != json("held_mouse")?["mouse"]["x"],
        "cross-call movement missing",
    )?;
    let save_request = json("save_request")?;
    let load_request = json("load_request")?;
    let save = json("save_response")?;
    let load = json("load_response")?;
    let save_path = save_request["path"]
        .as_str()
        .filter(|path| !path.is_empty())
        .ok_or_else(|| invalid("canonical state path missing"))?;
    require(
        save_request["strict_match"] == true
            && load_request["strict_match"] == true
            && save["saved"] == save_path
            && load_request["path"] == save_path
            && load["loaded"] == save["saved"]
            && load["header"] == save["header"]
            && save["emulator"] == env!("CARGO_PKG_VERSION"),
        "actual strict save/load linkage missing",
    )?;
    get("state_file")?;
    let state_record = catalog
        .files
        .get("state_file")
        .ok_or_else(|| invalid("state artifact missing"))?;
    require(
        confined(&raw_root, &state_record.path)?.canonicalize()?
            == Path::new(save_path).canonicalize()?,
        "state artifact differs from the saved/loaded file",
    )?;
    let selected = json("drag_selected")?;
    let replaced = json("drag_replaced")?;
    require(
        selected["lines"][2]
            .as_str()
            .is_some_and(|line| line.starts_with("│MCP DRAG SAMPLE"))
            && selected["attrs"][2].as_array().is_some_and(|attrs| {
                attrs
                    .iter()
                    .any(|attr| attr["attr"] == "71" && attr["col"] == 1 && attr["len"] == 15)
            }),
        "independent editor selection missing",
    )?;
    require(
        replaced["lines"][2].as_str().is_some_and(|line| {
            line.starts_with("│MCP_REPLACED") && !line.contains("MCP DRAG SAMPLE")
        }),
        "independent editor replacement missing",
    )?;
    require(
        json("drag_held")?["mouse"]["buttons"] == 1
            && json("drag_released")?["mouse"]["buttons"] == 0,
        "editor button transition missing",
    )?;
    require(
        png_size(get("drag_before_png")?)? == dimensions
            && png_size(get("drag_after_png")?)? == dimensions
            && get("drag_before_png")? != get("drag_after_png")?,
        "actual editor drag frames missing",
    )?;
    let finite = json("text_finite_selected")?;
    require(
        finite["lines"][2]
            .as_str()
            .is_some_and(|line| line.starts_with("│MCP_REPLACED"))
            && finite["attrs"][2].as_array().is_some_and(|attrs| {
                attrs
                    .iter()
                    .any(|attr| attr["attr"] == "71" && attr["col"] == 1 && attr["len"] == 12)
            }),
        "independent finite editor selection missing",
    )?;
    require(
        json("text_finite_replaced")?["lines"][2]
            .as_str()
            .is_some_and(|line| line.starts_with("│MCP_FINITE") && !line.contains("MCP_REPLACED")),
        "independent finite editor replacement missing",
    )?;
    let clicked = json("text_clicked")?;
    require(
        json("text_finite_released")?["mouse"]["buttons"] == 0
            && clicked["mouse"]["buttons"] == 0
            && clicked["mouse"]["x"] == 40
            && clicked["mouse"]["y"] == 20
            && json("text_click_text")?["cursor"]["row"] == 2
            && json("text_click_text")?["cursor"]["col"] == 5,
        "independent editor click outcome missing",
    )?;
    let relative = json("text_relative_status")?;
    require(
        relative["instance"] == clicked["instance"]
            && relative["mouse"]["buttons"] == 0
            && relative["mouse"]["x"] == 45
            && relative["mouse"]["y"] == 17,
        "text relative displacement missing",
    )?;
    let finite_request = json("text_finite_request")?;
    let events = finite_request["events"]
        .as_array()
        .ok_or_else(|| invalid("text finite canonical events missing"))?;
    require(
        events.len() == 6
            && events[0]["action"] == "down"
            && events[0]["button"] == "left"
            && events[0]["coords"] == "screen"
            && events[0]["x"] == 8
            && events[0]["y"] == 40
            && events[1]["type"] == "wait"
            && events[1]["ms"] == 100
            && events[2]["action"] == "move"
            && events[2]["coords"] == "screen"
            && events[2]["x"] == 104
            && events[2]["y"] == 40
            && events[3]["type"] == "wait"
            && events[3]["ms"] == 100
            && events[4]["action"] == "up"
            && events[4]["button"] == "left"
            && events[5]["type"] == "wait"
            && events[5]["ms"] == 100,
        "text finite canonical order missing",
    )?;
    let click = json("text_click_request")?;
    require(
        click["action"] == "click"
            && click["button"] == "left"
            && click["coords"] == "screen"
            && click["x"] == 40
            && click["y"] == 40,
        "text canonical click missing",
    )?;
    let relative_request = json("text_relative_request")?;
    require(
        relative_request["action"] == "move"
            && relative_request["dx"] == 5
            && relative_request["dy"] == -3
            && relative_request.get("x").is_none()
            && relative_request.get("y").is_none(),
        "text canonical relative input missing",
    )?;
    let matrix = json("text_journal")?;
    for (reply, events) in [("text_finite", 6), ("text_click", 1), ("text_relative", 1)] {
        delivery_witness(&matrix, &json(reply)?, events)?;
    }
    let journal = json("journal")?;
    let entries = journal["entries"]
        .as_array()
        .ok_or_else(|| invalid("actual journal entries missing"))?;
    require(
        entries
            .iter()
            .any(|entry| entry["type"] == "started" && entry["build"]["commit"] == revision),
        "journal producer linkage missing",
    )?;
    for name in [
        "echo_response",
        "enter_response",
        "shift_response",
        "mouse_down_response",
        "mouse_up_response",
    ] {
        let reply = json(name)?;
        let cause = reply["req"]
            .as_u64()
            .ok_or_else(|| invalid("delivered request identity missing"))?;
        require(
            entries
                .iter()
                .any(|entry| entry["type"] == "input_delivered" && entry["cause"] == cause),
            "input delivery journal witness missing",
        )?;
    }
    for (event, reply) in [("state_saved", &save), ("state_loaded", &load)] {
        let cause = reply["req"]
            .as_u64()
            .ok_or_else(|| invalid("state request identity missing"))?;
        require(
            entries
                .iter()
                .any(|entry| entry["type"] == event && entry["cause"] == cause),
            "state lifecycle journal witness missing",
        )?;
    }
    if protocol {
        super::protocol::verify_reference(get("protocol_producer")?, get("protocol_wire")?)?;
    }
    println!(
        "Verified {} real acquisition records; native MCP runtime capabilities are not asserted by this check",
        files.len()
    );
    Ok(())
}

pub(super) fn verify_protocol(bytes: &[u8]) -> io::Result<()> {
    let text = std::str::from_utf8(bytes).map_err(io::Error::other)?;
    let mut requests = BTreeMap::new();
    let mut responses = BTreeMap::<u64, Vec<u8>>::new();
    let mut statuses = BTreeMap::new();
    require(text.lines().count() <= 64, "protocol record count bound")?;
    for line in text.lines() {
        let value: Value = serde_json::from_str(line).map_err(io::Error::other)?;
        let id = value["id"]
            .as_u64()
            .ok_or_else(|| invalid("protocol exchange ID missing"))?;
        match value["kind"].as_str() {
            Some("request") | Some("response_chunk") => {
                let encoded = value["base64"]
                    .as_str()
                    .ok_or_else(|| invalid("protocol wire bytes missing"))?;
                require(encoded.len() <= 64 * 1024, "protocol body bound")?;
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .map_err(io::Error::other)?;
                if value["kind"] == "request" {
                    require(
                        requests.insert(id, decoded).is_none(),
                        "duplicate protocol request",
                    )?;
                } else {
                    let body = responses.entry(id).or_default();
                    require(
                        decoded.len() <= (64 * 1024usize).saturating_sub(body.len()),
                        "protocol response bound",
                    )?;
                    body.extend_from_slice(&decoded);
                }
            }
            Some("response") => {
                require(
                    statuses.insert(id, value["status"].as_u64()).is_none(),
                    "duplicate protocol response",
                )?;
            }
            _ => return Err(invalid("unknown protocol wire record")),
        }
    }
    let mut initialize = false;
    let mut discover = false;
    let mut legacy_listing = false;
    let mut stateless_listing = false;
    for (id, request) in requests {
        if request.is_empty() {
            continue;
        }
        let request: Value = serde_json::from_slice(&request).map_err(io::Error::other)?;
        require(
            request["jsonrpc"] == "2.0",
            "reference RPC envelope missing",
        )?;
        let method = request["method"]
            .as_str()
            .ok_or_else(|| invalid("protocol method missing"))?;
        if method == "notifications/initialized" {
            continue;
        }
        require(
            request["id"].as_u64().is_some(),
            "reference RPC identity missing",
        )?;
        require(
            statuses.get(&id) == Some(&Some(200)),
            "reference RPC response not successful",
        )?;
        let body = std::str::from_utf8(
            responses
                .get(&id)
                .ok_or_else(|| invalid("reference reply bytes missing"))?,
        )
        .map_err(io::Error::other)?;
        let mut result = None;
        for line in body.lines() {
            if let Some(json) = line
                .strip_prefix("data: ")
                .filter(|json| json.starts_with('{'))
            {
                let reply: Value = serde_json::from_str(json).map_err(io::Error::other)?;
                require(
                    reply["jsonrpc"] == "2.0",
                    "reference reply envelope missing",
                )?;
                if reply["id"] == request["id"] {
                    require(
                        result.is_none()
                            && reply.get("error").is_none()
                            && reply.get("result").is_some(),
                        "reference result is duplicate, missing or mixed with an error",
                    )?;
                    result = Some(reply["result"].clone());
                }
            }
        }
        let result = result.ok_or_else(|| invalid("matching reference RPC result missing"))?;
        match method {
            "initialize" => {
                require(
                    request["params"]["protocolVersion"] == "2025-11-25"
                        && request["params"]["clientInfo"]["version"] == "3.5.1"
                        && result["protocolVersion"] == "2025-11-25"
                        && result["serverInfo"]["version"] == "3.5.1",
                    "legacy negotiation mismatch",
                )?;
                initialize = true;
            }
            "server/discover" => {
                require(
                    request["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"]
                        == "2026-07-28"
                        && request["params"]["_meta"]["io.modelcontextprotocol/clientInfo"]["version"]
                            == "3.5.1"
                        && result["supportedVersions"]
                            .as_array()
                            .is_some_and(|versions| {
                                versions.iter().any(|version| version == "2026-07-28")
                            })
                        && result["_meta"]["io.modelcontextprotocol/serverInfo"]["version"]
                            == "3.5.1",
                    "stateless discovery mismatch",
                )?;
                discover = true;
            }
            "tools/list" => {
                require(
                    result["tools"].as_array().is_some_and(Vec::is_empty),
                    "reference provider unexpectedly exposes guest tools",
                )?;
                match request["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"].as_str()
                {
                    Some("2026-07-28") => {
                        require(
                            !stateless_listing && result["resultType"] == "complete",
                            "stateless tool-list witness mismatch",
                        )?;
                        stateless_listing = true;
                    }
                    None => {
                        require(!legacy_listing, "duplicate legacy tool-list witness")?;
                        legacy_listing = true;
                    }
                    _ => return Err(invalid("unbound tool-list protocol")),
                }
            }
            _ => return Err(invalid("unbound reference RPC")),
        }
    }
    require(
        initialize && discover && legacy_listing && stateless_listing,
        "required actual protocol exchanges missing",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bios_clock_records_require_exactly_four_hex_bytes() {
        assert_eq!(
            bios_clock_record(&serde_json::json!({"hex":"3D 00 00 00"})).unwrap(),
            61
        );
        for value in [
            serde_json::json!({}),
            serde_json::json!({"hex":null}),
            serde_json::json!({"hex":"3D 00 00"}),
            serde_json::json!({"hex":"3D 00 00 00 00"}),
            serde_json::json!({"hex":"GG 00 00 00"}),
            serde_json::json!({"hex":"100 00 00 00"}),
        ] {
            assert!(bios_clock_record(&value).is_err());
        }
    }

    #[test]
    fn unavailable_capture_catalog_and_non_metadata_artifacts_fail_closed() {
        let temporary = crate::TemporaryCopy::new().unwrap();
        assert!(verify(&temporary.path.join("missing-catalog.json"), "missing").is_err());
        assert!(verify_public_assets(&temporary.path).is_err());
        verify_public_assets(Path::new("tests/fixtures/approved/mcp")).unwrap();
    }

    #[test]
    fn graphics_oracles_reject_missing_effects_and_wrong_dimensions() {
        for (program, size) in [("seal", 640 * 480 * 3), ("grafx2", 640 * 400 * 3)] {
            let unchanged = vec![0; size];
            assert!(graphics_effect(program, &unchanged, &unchanged).is_err());
            assert!(graphics_effect(program, &unchanged[..size - 1], &unchanged).is_err());
            assert!(graphics_effect(program, &unchanged, &unchanged[..size - 1]).is_err());
        }
        assert!(graphics_effect("unregistered", &[], &[]).is_err());
    }

    #[test]
    fn canvas_oracle_rejects_a_changed_pixel_outside_the_selected_stroke() {
        let before = vec![0; 640 * 400 * 3];
        let mut after = before.clone();
        after[..3].copy_from_slice(&[252, 252, 252]);
        assert!(graphics_effect("grafx2", &before, &after).is_err());
    }

    #[test]
    fn a_real_source_file_obeys_the_capture_reader_byte_bound() {
        let path = Path::new("src/debug/control.rs");
        assert_eq!(
            read_bounded(path, 1024 * 1024).unwrap(),
            std::fs::read(path).unwrap()
        );
        assert!(read_bounded(path, 1).is_err());
    }

    #[test]
    fn confinement_rejects_escaping_paths() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        assert!(confined(root, Path::new("../Cargo.toml")).is_err());
        assert!(confined(root, root).is_err());
        assert!(confined(root, Path::new("")).is_err());
        assert_eq!(
            confined(root, Path::new("src/debug/control.rs")).unwrap(),
            root.join("src/debug/control.rs")
        );
    }

    #[test]
    fn unrelated_real_bytes_cannot_prove_a_png_or_sdk_exchange() {
        let bytes = std::fs::read("src/debug/control.rs").unwrap();
        assert!(png_size(&bytes).is_err());
        assert!(verify_protocol(&bytes).is_err());
    }

    #[test]
    fn producer_integrity_rejects_empty_and_changed_files() {
        let temporary = crate::TemporaryCopy::new().unwrap();
        let path = temporary.path.join("producer.rs");
        std::fs::copy("src/debug/control.rs", &path).unwrap();
        let mut hash = Sha256::new();
        hash.update(std::fs::read(&path).unwrap());
        let digest = hex_digest(hash);
        verify_source(&path, &digest).unwrap();
        assert!(verify_source(&path, "changed").is_err());
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_len(0).unwrap();
        assert!(verify_source(&path, &hex_digest(Sha256::new())).is_err());
    }

    #[test]
    fn file_bounds_reject_directories_and_sparse_oversized_sources() {
        let temporary = crate::TemporaryCopy::new().unwrap();
        assert!(read_bounded(&temporary.path, MAX_RECORD).is_err());
        assert!(verify_source(&temporary.path, "changed").is_err());
        let path = temporary.path.join("source.rs");
        std::fs::copy("src/debug/control.rs", &path).unwrap();
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_len(0).unwrap();
        assert!(read_bounded(&path, MAX_RECORD).is_err());
        file.set_len(512 * 1024 * 1024 + 1).unwrap();
        assert!(read_bounded(&path, MAX_RECORD).is_err());
        assert!(verify_source(&path, "changed").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn acquisition_readers_refuse_file_and_directory_symlinks() {
        let temporary = crate::TemporaryCopy::new().unwrap();
        std::fs::copy("src/debug/control.rs", temporary.path.join("source.rs")).unwrap();
        let link = temporary.path.join("link.rs");
        std::os::unix::fs::symlink("source.rs", &link).unwrap();
        assert!(confined(&temporary.path, Path::new("link.rs")).is_err());
        assert!(read_bounded(&link, MAX_RECORD).is_err());
        assert!(verify_source(&link, "changed").is_err());
        std::os::unix::fs::symlink(&temporary.path, temporary.path.join("alias")).unwrap();
        assert!(confined(&temporary.path, Path::new("alias/source.rs")).is_err());
    }
}
