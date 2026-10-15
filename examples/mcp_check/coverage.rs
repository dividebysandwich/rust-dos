//! Source-bound acquisition-tool coverage, separate from runtime fact thresholds.
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
    schema_version: u32,
    scope: String,
    report: PathBuf,
    report_sha256: String,
    sources: BTreeMap<String, String>,
}

pub(super) fn verify(path: &Path) -> io::Result<()> {
    let receipt: Receipt =
        serde_json::from_slice(&read_bounded(path, 64 * 1024)?).map_err(io::Error::other)?;
    require(
        receipt.schema_version == 1
            && receipt.scope == "acquisition tooling"
            && receipt.report.is_absolute()
            && receipt.sources.len() <= 16,
        "unsupported coverage receipt",
    )?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).canonicalize()?;
    let required = [
        "examples/mcp_check.rs",
        "examples/mcp_check/acquisition.rs",
        "examples/mcp_check/assets.rs",
        "examples/mcp_check/client.rs",
        "examples/mcp_check/protocol.rs",
        "examples/mcp_check/replay.rs",
        "examples/mcp_check/replay/frontend.rs",
        "examples/mcp_check/coverage.rs",
        "examples/mcp_check/native.rs",
        "examples/mcp_check/native/catalog.rs",
        "examples/mcp_check/runtime_coverage.rs",
        "Cargo.lock",
        "Cargo.toml",
    ];
    for path in required {
        require(
            receipt.sources.contains_key(path),
            "coverage source binding missing",
        )?;
    }
    for (path, digest) in receipt.sources {
        verify_source(&confined(&root, Path::new(&path))?, &digest)?;
    }
    verify_source(&receipt.report, &receipt.report_sha256)?;
    let report: Value = serde_json::from_slice(&read_bounded(&receipt.report, 64 * 1024 * 1024)?)
        .map_err(io::Error::other)?;
    require(
        report["type"] == "llvm.coverage.json.export" && report["version"] == "3.1.0",
        "unsupported LLVM coverage export",
    )?;
    let data = report["data"]
        .as_array()
        .filter(|data| data.len() == 1)
        .ok_or_else(|| io::Error::other("coverage data set missing/ambiguous"))?;
    let files = data[0]["files"]
        .as_array()
        .ok_or_else(|| io::Error::other("coverage files missing"))?;
    let mut total = 0u64;
    let mut covered = 0u64;
    for source in required.into_iter().filter(|path| path.ends_with(".rs")) {
        let path = root.join(source);
        let matches = files
            .iter()
            .filter(|file| {
                file["filename"]
                    .as_str()
                    .is_some_and(|name| Path::new(name) == path)
            })
            .collect::<Vec<_>>();
        require(
            matches.len() == 1,
            "coverage helper source missing/ambiguous",
        )?;
        let branches = &matches[0]["summary"]["branches"];
        let count = branches["count"]
            .as_u64()
            .ok_or_else(|| io::Error::other("coverage branch count missing"))?;
        let hits = branches["covered"]
            .as_u64()
            .ok_or_else(|| io::Error::other("coverage branch hits missing"))?;
        require(
            count > 0 && hits > 0 && hits <= count,
            "empty or inconsistent helper coverage",
        )?;
        total = total
            .checked_add(count)
            .ok_or_else(|| io::Error::other("coverage count overflow"))?;
        covered = covered
            .checked_add(hits)
            .ok_or_else(|| io::Error::other("coverage hit overflow"))?;
        println!("Tooling branches {source}: {hits}/{count}");
    }
    println!(
        "Acquisition tooling coverage: {covered}/{total}; native runtime thresholds remain separate"
    );
    // Preserve the accepted acquisition-only baseline, independently of runtime 95%.
    require(
        u128::from(covered) * 914 >= u128::from(total) * 612,
        "acquisition tooling coverage regressed below the accepted 612/914 baseline",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_coverage_and_actual_non_receipt_source_are_rejected() {
        assert!(verify(Path::new("src/debug/control.rs")).is_err());
        let temporary = crate::TemporaryCopy::new().unwrap();
        assert!(verify(&temporary.path.join("missing-coverage.json")).is_err());
    }
}
