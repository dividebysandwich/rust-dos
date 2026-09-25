//! Lists the files of the Ultrasound software built into rust-dos
//! (`assets/ultrasnd`) for `src/gus/builtin.rs`, which embeds them.

use std::path::{Path, PathBuf};

fn main() {
    // The hosts the dynamic recompiler (src/dynrec) generates code for.
    println!("cargo::rustc-check-cfg=cfg(dynrec)");
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("x86_64") {
        println!("cargo::rustc-cfg=dynrec");
    }

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest.join("assets/ultrasnd");
    println!("cargo::rerun-if-changed=assets/ultrasnd");

    let mut files = Vec::new();
    collect(&root, &root, &mut files);
    files.sort();
    let mut out = String::from("&[\n");
    for (dos, host) in &files {
        let host = host.to_str().expect("asset paths are UTF-8");
        out.push_str(&format!("    ({:?}, include_bytes!({:?})),\n", dos, host));
    }
    out.push_str("]\n");
    let dest = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("ultrasnd.rs");
    std::fs::write(dest, out).unwrap();
}

/// Every file under `dir`, as its DOS path from `root` ("MIDI\ACPIANO.PAT")
/// and its host path.
fn collect(root: &Path, dir: &Path, files: &mut Vec<(String, PathBuf)>) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {}", dir.display(), e));
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect(root, &path, files);
            continue;
        }
        let dos = path
            .strip_prefix(root)
            .unwrap()
            .components()
            .map(|c| c.as_os_str().to_str().expect("asset names are ASCII").to_ascii_uppercase())
            .collect::<Vec<_>>()
            .join("\\");
        files.push((dos, path));
    }
}
