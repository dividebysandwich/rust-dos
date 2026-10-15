//! Current-source branch evidence for the native observation boundary.
use super::acquisition::{confined, read_bounded, require, verify_source};
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    schema_version: u8,
    report: PathBuf,
    report_sha256: String,
    sources: BTreeMap<String, String>,
    exclusions: Vec<Exclusion>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Exclusion {
    source: String,
    line: u64,
    column: u64,
    side: usize,
    proof: String,
}

pub(super) const REQUIRED: &[&str] = &[
    "Cargo.toml",
    "Cargo.lock",
    "tests/fixtures/mcp/mouse.schema.json",
    "src/main.rs",
    "src/startup.rs",
    "src/debug/mod.rs",
    "src/debug/run.rs",
    "src/debug/input.rs",
    "src/debug/dispatch.rs",
    "src/debug/events.rs",
    "src/debug/state.rs",
    "src/debug/server.rs",
    "src/debug/control.rs",
    "src/debug/control/service.rs",
    "src/debug/control/metrics.rs",
    "src/debug/control/input.rs",
    "src/debug/control/mouse.rs",
    "src/debug/mcp/mod.rs",
    "src/debug/mcp/keyboard.rs",
    "src/debug/mcp/mouse.rs",
    "src/debug/mcp/schemas.rs",
    "src/debug/mcp/transport.rs",
    "src/debug/mcp/sessions.rs",
    "src/debug/mcp/diagnostics.rs",
    "src/debug/mcp/metrics.rs",
    "src/capture/png.rs",
    "src/debug/server/http_tests.rs",
    "tests/mcp_foundation.rs",
    "tests/mcp_metrics.rs",
];

// These producers support the reviewed invariants, rather than adding legacy
// keyboard code to the measured native runtime scope.
pub(super) const INVARIANT_SOURCES: &[&str] =
    &["src/keyboard.rs", "src/keylayout.rs", "src/debug/keys.rs"];

fn span(text: &str, first: &str, last: &str) -> io::Result<(u64, u64)> {
    let lines: Vec<_> = text.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.contains(first))
        .ok_or_else(|| io::Error::other("coverage scope start missing"))?;
    let end = lines
        .iter()
        .enumerate()
        .skip(start)
        .find(|(_, line)| line.contains(last))
        .map(|(line, _)| line)
        .ok_or_else(|| io::Error::other("coverage scope end missing"))?;
    Ok((start as u64 + 1, end as u64 + 1))
}

fn scopes(source: &str, text: &str) -> io::Result<Vec<(u64, u64)>> {
    let ranges = match source {
        "src/debug/mod.rs" => vec![],
        "src/debug/run.rs" => vec![
            span(text, "pub fn poll(&mut self", "self.update_heat(cpu);")?,
            span(
                text,
                "for (ctx, w) in std::mem::take(&mut self.frame_waiters)",
                "let meta = self.meta(&ctx);",
            )?,
        ],
        "src/debug/input.rs" => vec![
            span(
                text,
                "fn check_input_capacity(&self)",
                "fn pop_input(&mut self)",
            )?,
            span(
                text,
                "fn pop_input(&mut self)",
                "fn input_delivered(&mut self",
            )?,
            span(
                text,
                "if self.native_cancel_expired_borrow(cpu)",
                "let ev = self.input.front().unwrap();",
            )?,
            span(
                text,
                "let native = self.owned_input.spans.front()",
                "// One scan code per frame.",
            )?,
            {
                let (start, end) = span(
                    text,
                    "LowInput::Wait(d) if self.deterministic",
                    "LowInput::Wait(d) =>",
                )?;
                (start + 1, end - 1)
            },
            span(text, "LowInput::Wait(d) =>", "                _ => {}")?,
            span(
                text,
                "fn input_delivered(&mut self",
                "fn guest_key_down(&mut self",
            )?,
            span(
                text,
                "fn guest_key_down(&mut self",
                "fn forget_keys_after_load(&mut self)",
            )?,
        ],
        "src/debug/dispatch.rs" => vec![
            span(
                text,
                "fn handle(&mut self, cpu: &mut Cpu, req: Request)",
                "// A stop comes before",
            )?,
            span(text, "Cmd::NativeInput { input, job } =>", "Cmd::Status =>")?,
            span(text, "Cmd::Quit =>", "self.quit = self.cur.clone();")?,
            span(
                text,
                "if self.native_legacy_conflict(&low)",
                "self.input.push_back(LowInput::End { req });",
            )?,
            span(text, "Cmd::InputClear =>", "Cmd::Pause =>")?,
            span(text, "fn changes_machine(cmd: &Cmd)", "    matches!(")?,
        ],
        "src/debug/events.rs" => vec![span(
            text,
            "pub fn state_loaded_elsewhere",
            "self.told_starts = cpu.programs.started;",
        )?],
        "src/debug/state.rs" => vec![
            span(
                text,
                "if !request.strict_match { self.invalidate_owned_after_load(cpu); }",
                "self.told_starts = cpu.programs.started;",
            )?,
            span(
                text,
                "pub fn reset_after_load(&mut self, cpu: &Cpu)",
                "let break_on:",
            )?,
            span(
                text,
                "let mut cancelled: Vec<u64> = self.current_op.take()",
                "self.once.clear();",
            )?,
            span(
                text,
                "for event in self.input.drain(..)",
                "cancelled.sort_unstable();",
            )?,
        ],

        "src/debug/server.rs" => {
            let (start, end) = span(text, "let app = if mcp_enabled", "axum::serve(")?;
            vec![
                span(text, "pub fn spawn(", "// Bind synchronously")?,
                (start, end - 1),
            ]
        }
        "src/startup.rs" => vec![span(
            text,
            "let mcp_enabled = args.mcp;",
            "let mut dbg = match args.debug_server",
        )?],
        "src/main.rs" => vec![
            span(text, "if state_loaded {", "dbg.poll(&mut cpu);")?,
            span(
                text,
                "let preserve_keyboard = strict_match_loaded;",
                "clipboard.stop_paste(&mut cpu);",
            )?,
            span(
                text,
                "match rewinder.step_back()",
                "dbg.invalidate_owned_after_load(&cpu);",
            )?,
            span(
                text,
                "dbg.release_owned_input(cpu);",
                "dbg.shutdown(cpu, shutdown, 0);",
            )?,
        ],
        "src/capture/png.rs" => {
            vec![span(text, "pub fn encode_to(", "/// The picture in PNG")?]
        }
        _ => vec![(1, test_module_start(text))],
    };
    // Composition/reaper code follows the test module in this adapter.
    let ranges = if source == "src/debug/mcp/mod.rs" {
        let mut ranges = ranges;
        let (start, _) = span(text, "pub(super) fn attach(", "fn spawn_reaper(")?;
        ranges.push((start, text.lines().count() as u64));
        ranges
    } else if source == "src/debug/control/service.rs" {
        let mut ranges = ranges;
        let (start, _) = span(text, "pub(super) struct Inputs<'a>", "impl ControlClient {")?;
        ranges.push((start, text.lines().count() as u64));
        ranges
    } else {
        ranges
    };
    Ok(ranges)
}

fn test_module_start(text: &str) -> u64 {
    text.lines()
        .collect::<Vec<_>>()
        .windows(2)
        .position(|lines| {
            lines[0] == "#[cfg(test)]" && lines[1].trim_start().starts_with("mod tests")
        })
        .map_or(u64::MAX, |line| line as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_scopes_follow_upstream_modules() {
        for source in REQUIRED.iter().copied().filter(|p| {
            p.ends_with(".rs") && !p.ends_with("http_tests.rs") && !p.starts_with("tests/")
        }) {
            let text = std::fs::read_to_string(source).unwrap();
            scopes(source, &text).unwrap_or_else(|e| panic!("{source}: {e}"));
        }
    }

    #[test]
    fn actual_cfg_helper_does_not_hide_adapter_implementation() {
        let text = std::fs::read_to_string("src/debug/mcp/mod.rs").unwrap();
        let end = test_module_start(&text);
        let implementation = text
            .lines()
            .position(|line| line.contains("async fn call_tool("))
            .unwrap() as u64;
        assert!(
            end > implementation,
            "test-only helper must not truncate actual production handlers"
        );
        assert!(
            text.lines()
                .nth(end as usize + 1)
                .unwrap()
                .starts_with("mod tests")
        );
    }
}

pub(super) fn verify(path: &Path) -> io::Result<()> {
    let receipt: Receipt =
        serde_json::from_slice(&read_bounded(path, 64 * 1024)?).map_err(io::Error::other)?;
    require(
        receipt.schema_version == 1 && receipt.report.is_absolute(),
        "unsupported runtime coverage receipt",
    )?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).canonicalize()?;
    require(
        REQUIRED
            .iter()
            .chain(INVARIANT_SOURCES.iter())
            .all(|source| receipt.sources.contains_key(*source)),
        "runtime coverage source binding incomplete",
    )?;
    for (source, digest) in &receipt.sources {
        verify_source(&confined(&root, Path::new(source))?, digest)?;
    }
    require(
        std::fs::read_to_string(root.join("Cargo.lock"))?
            .contains("[[package]]\nname = \"rmcp\"\nversion = \"3.5.1\""),
        "runtime SDK invariants require pinned rmcp 3.5.1",
    )?;
    verify_source(&receipt.report, &receipt.report_sha256)?;
    let report: Value = serde_json::from_slice(&read_bounded(&receipt.report, 64 * 1024 * 1024)?)
        .map_err(io::Error::other)?;
    require(
        report["type"] == "llvm.coverage.json.export" && report["version"] == "3.1.0",
        "unsupported runtime LLVM report",
    )?;
    let datasets = report["data"]
        .as_array()
        .filter(|data| data.len() == 1)
        .ok_or_else(|| io::Error::other("runtime coverage dataset missing"))?;
    let files = datasets[0]["files"]
        .as_array()
        .ok_or_else(|| io::Error::other("runtime coverage files missing"))?;
    let mut used = vec![false; receipt.exclusions.len()];
    for source in REQUIRED.iter().copied().filter(|path| {
        path.ends_with(".rs") && !path.ends_with("http_tests.rs") && !path.starts_with("tests/")
    }) {
        // Keep the parent source-bound; its native guards now live in the child scopes.
        if source == "src/debug/mod.rs" {
            continue;
        }
        let text = std::fs::read_to_string(root.join(source))?;
        let ranges = scopes(source, &text)?;
        let matching: Vec<_> = files
            .iter()
            .filter(|file| {
                file["filename"]
                    .as_str()
                    .is_some_and(|name| Path::new(name) == root.join(source))
            })
            .collect();
        require(
            matching.len() == 1,
            "runtime coverage source missing or ambiguous",
        )?;
        let branches = matching[0]["branches"]
            .as_array()
            .ok_or_else(|| io::Error::other("runtime branch rows missing"))?;
        // LLVM repeats source rows for binary/test and generic instantiations.
        // Observe a source side once across those actual executions.
        let mut observed = BTreeMap::new();
        for branch in branches {
            let row = branch
                .as_array()
                .filter(|row| row.len() >= 6)
                .ok_or_else(|| io::Error::other("runtime branch row malformed"))?;
            let line = row[0]
                .as_u64()
                .ok_or_else(|| io::Error::other("runtime branch line missing"))?;
            let column = row[1]
                .as_u64()
                .ok_or_else(|| io::Error::other("runtime branch column missing"))?;
            if !ranges
                .iter()
                .any(|(start, end)| (*start..=*end).contains(&line))
            {
                continue;
            }
            let end_line = row[2]
                .as_u64()
                .ok_or_else(|| io::Error::other("runtime branch end line missing"))?;
            let end_column = row[3]
                .as_u64()
                .ok_or_else(|| io::Error::other("runtime branch end column missing"))?;
            let counts = observed
                .entry((line, column, end_line, end_column))
                .or_insert([false; 2]);
            for side in 0..2 {
                let count = row[4 + side]
                    .as_u64()
                    .ok_or_else(|| io::Error::other("runtime branch hit missing"))?;
                counts[side] |= count > 0;
            }
        }
        let mut total = 0u64;
        let mut hits = 0u64;
        for ((line, column, _, _), counts) in observed {
            for (side, covered) in counts.into_iter().enumerate() {
                if let Some((index, exclusion)) =
                    receipt.exclusions.iter().enumerate().find(|(_, excluded)| {
                        excluded.source == source
                            && excluded.line == line
                            && excluded.column == column
                            && excluded.side == side
                    })
                {
                    require(
                        !used[index] && !covered && exclusion.side < 2,
                        "invalid or repeated runtime exclusion",
                    )?;
                    validate_exclusion(source, &text, line, column, side, &exclusion.proof)?;
                    used[index] = true;
                    continue;
                }
                total += 1;
                hits += u64::from(covered);
            }
        }
        if total == 0 {
            // The relocated load notification is straight-line code: measure its regions.
            let measured_branchless_lifecycle = source == "src/debug/events.rs"
                && matching[0]["segments"].as_array().is_some_and(|segments| {
                    let regions: Vec<_> = segments
                        .iter()
                        .filter(|segment| {
                            segment[0].as_u64().is_some_and(|line| {
                                ranges
                                    .iter()
                                    .any(|(start, end)| (*start..=*end).contains(&line))
                            }) && segment[3] == true
                                && segment[4] == true
                        })
                        .collect();
                    !regions.is_empty()
                        && regions
                            .iter()
                            .all(|segment| segment[2].as_u64().is_some_and(|count| count > 0))
                });
            let measured_branchless_metrics = matches!(
                source,
                "src/debug/control/metrics.rs" | "src/debug/mcp/metrics.rs"
            ) && matching[0]["branches"]
                .as_array()
                .is_some_and(Vec::is_empty)
                && matching[0]["summary"]["functions"]["covered"]
                    .as_u64()
                    .is_some_and(|covered| covered > 0);
            require(
                measured_branchless_lifecycle
                    || measured_branchless_metrics
                    || matches!(
                        source,
                        "src/debug/control.rs"
                            | "src/debug/mcp/diagnostics.rs"
                            | "src/capture/png.rs"
                    ),
                "required runtime guard has no measured branches",
            )?;
            println!("Runtime {source}: no instrumented branch counters (not a percentage)");
        } else {
            println!("Critical runtime branches {source}: {hits}/{total}");
            require(
                hits * 100 >= total * 95,
                "critical runtime coverage below 95 percent",
            )?;
        }
    }
    require(
        used.into_iter().all(|used| used),
        "unused runtime coverage exclusion",
    )
}

fn validate_exclusion(
    source: &str,
    text: &str,
    line: u64,
    column: u64,
    side: usize,
    proof: &str,
) -> io::Result<()> {
    let actual = text
        .lines()
        .nth(line as usize - 1)
        .ok_or_else(|| io::Error::other("exclusion line missing"))?;
    let (marker, expected_side) = match proof {
        "bounded-png-envelope" => ("base64_size", 0),
        "sdk-request-arm-only" => ("ClientJsonRpcMessage::Request", 1),
        "ingress-permit-ticket-bound" => ("book.len() >= 16", 0),
        "sdk-channel-id-always-present" => ("Some(channel)", 1),
        "native-feature-compiled-in" => ("!cfg!(", 0),
        "native-target-guard-before-delivery" => ("!self.divert", 1),
        "native-down-span-survives-key" => ("Some(span)", 1),
        "native-up-span-survives-key" => ("Some(span)", 1),
        "native-producer-distinct-downs" => ("!span.delivered.contains(&key)", 1),
        _ => return Err(io::Error::other("unreviewed runtime exclusion")),
    };
    require(
        side == expected_side
            && actual
                .find(marker)
                .is_some_and(|offset| offset as u64 + 1 == column),
        "runtime exclusion targets a different guard or side",
    )?;
    let valid = match (source, proof) {
        ("src/debug/server.rs", "native-feature-compiled-in") => {
            actual.contains("!cfg!(feature = \"mcp\")")
        }
        ("src/debug/mcp/mod.rs", "bounded-png-envelope") => {
            actual.contains("if base64_size")
                && text.contains("24 * 1024 * 1024")
                && text.contains("13 * 1024 * 1024")
                && ((13u64 * 1024 * 1024).div_ceil(3) * 4 + 1024 * 1024 < 24 * 1024 * 1024)
        }
        ("src/debug/mcp/sessions.rs", "sdk-request-arm-only") => {
            actual.contains("ClientJsonRpcMessage::Request")
        }
        ("src/debug/mcp/sessions.rs", "ingress-permit-ticket-bound") => {
            actual.contains("book.len() >= 16")
        }
        ("src/debug/mcp/sessions.rs", "sdk-channel-id-always-present") => {
            actual.contains("receiver.http_request_id")
        }
        ("src/debug/input.rs", "native-target-guard-before-delivery") => {
            actual.contains("if native && !self.divert")
                && text.contains("self.native_guard_target(cpu);")
        }
        ("src/debug/input.rs", "native-down-span-survives-key") => {
            actual.contains("if let Some(span) = self.owned_input.spans.front_mut()")
                && text.contains("let native = self.owned_input.spans.front()")
        }
        ("src/debug/input.rs", "native-up-span-survives-key") => {
            actual.contains("if native && let Some(span) = self.owned_input.spans.front_mut()")
        }
        ("src/debug/input.rs", "native-producer-distinct-downs") => {
            actual.contains("if !span.delivered.contains(&key)")
                && text.contains("span.delivered.retain(|held| held != &key)")
        }
        _ => false,
    };
    require(
        valid,
        "runtime exclusion is not one of the independently reviewed pinned invariants",
    )
}
