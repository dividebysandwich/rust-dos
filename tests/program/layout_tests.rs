//! The layout of tests/: with `autotests = false` in Cargo.toml, Cargo
//! builds only the test programs listed there, and a program compiles
//! only the files its modules declare. A test file left out of either
//! would never be built or run, and nothing else would say so.

use std::fs;
use std::path::{Path, PathBuf};

/// The folder of tests/ whose helpers several programs declare by path.
const COMMON: &str = "common";

fn tests_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests")
}

/// The folders directly in tests/, by name.
fn folders() -> Vec<String> {
    let mut folders: Vec<String> = fs::read_dir(tests_dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    folders.sort();
    folders
}

/// The folders of tests/ that hold a test program (a main.rs).
fn groups() -> Vec<String> {
    folders()
        .into_iter()
        .filter(|g| tests_dir().join(g).join("main.rs").is_file())
        .collect()
}

/// The `.rs` files in `dir` and the folders below it.
fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(rust_files(&path));
        } else if path.extension().is_some_and(|x| x == "rs") {
            files.push(path);
        }
    }
    files
}

/// Whether `source` declares the module `name` (`mod name;`, or with
/// `pub` before it).
fn declares(source: &str, name: &str) -> bool {
    source.lines().any(|line| {
        let line = line.trim();
        let line = line
            .strip_prefix("pub(crate) ")
            .or_else(|| line.strip_prefix("pub "))
            .unwrap_or(line);
        line.strip_prefix("mod ").and_then(|l| l.strip_suffix(';')) == Some(name)
    })
}

/// The file that declares the module in `file` (a `name.rs`, or a
/// `name/mod.rs`), and the module's name: the folder's main.rs or mod.rs,
/// or the `folder.rs` beside the folder.
fn declaring_file(file: &Path) -> (PathBuf, String) {
    let stem = file.file_stem().unwrap().to_string_lossy().into_owned();
    let (dir, name) = if stem == "mod" {
        let dir = file.parent().unwrap();
        (
            dir.parent().unwrap(),
            dir.file_name().unwrap().to_string_lossy().into_owned(),
        )
    } else {
        (file.parent().unwrap(), stem)
    };
    let declarer = ["main.rs", "mod.rs"]
        .iter()
        .map(|f| dir.join(f))
        .find(|f| f.is_file())
        .unwrap_or_else(|| dir.with_extension("rs"));
    (declarer, name)
}

#[test]
fn no_test_file_lies_directly_in_tests() {
    let stray: Vec<String> = fs::read_dir(tests_dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "rs"))
        .map(|p| p.display().to_string())
        .collect();
    assert!(
        stray.is_empty(),
        "{stray:?} would not be built: put each in the folder of the code it tests \
         and declare it in that folder's main.rs"
    );
}

#[test]
fn every_folder_with_rust_files_is_a_test_program_or_the_common_helpers() {
    let mut stray = Vec::new();
    for folder in folders() {
        let dir = tests_dir().join(&folder);
        if folder != COMMON && !dir.join("main.rs").is_file() && !rust_files(&dir).is_empty() {
            stray.push(format!("tests/{folder}"));
        }
    }
    assert!(
        stray.is_empty(),
        "{stray:?} hold Rust files but no main.rs, so nothing builds them: move the files \
         into the folder of the code they test"
    );
}

#[test]
fn every_folder_with_a_main_rs_is_a_test_program_in_cargo_toml() {
    let manifest =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap();
    for group in groups() {
        let path = format!("path = \"tests/{group}/main.rs\"");
        assert!(
            manifest.contains(&path),
            "tests/{group}/main.rs is not a [[test]] in Cargo.toml"
        );
    }
}

#[test]
fn every_file_of_a_program_is_a_module_of_it() {
    let mut missing = Vec::new();
    for folder in groups().into_iter().chain([COMMON.to_string()]) {
        let root = tests_dir().join(&folder);
        for file in rust_files(&root) {
            // A program's main.rs, and a common helper's mod.rs, which the
            // programs declare by its path.
            let helper = file.parent().and_then(Path::parent) == Some(&*root)
                && file.file_name().is_some_and(|f| f == "mod.rs");
            if file == root.join("main.rs") || (folder == COMMON && helper) {
                continue;
            }
            let (declarer, name) = declaring_file(&file);
            let source = fs::read_to_string(&declarer).unwrap_or_default();
            if !declares(&source, &name) {
                let shown = file
                    .strip_prefix(tests_dir())
                    .unwrap()
                    .display()
                    .to_string();
                missing.push(format!("tests/{}", shown.replace('\\', "/")));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "{missing:?} would not be built: declare each with `mod <file>;` in its folder's \
         main.rs or mod.rs"
    );
}
