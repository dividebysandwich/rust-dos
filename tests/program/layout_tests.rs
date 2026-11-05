//! The layout of tests/: with `autotests = false` in Cargo.toml, Cargo
//! builds only the test programs listed there, and a program compiles
//! only the files its main.rs declares. A test file left out of either
//! would never be built or run, and nothing else would say so.

use std::fs;
use std::path::{Path, PathBuf};

fn tests_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests")
}

/// The folders of tests/ that hold a test program (a main.rs).
fn groups() -> Vec<String> {
    let mut groups: Vec<String> = fs::read_dir(tests_dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.join("main.rs").is_file())
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    groups.sort();
    groups
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
fn every_file_of_a_folder_is_a_module_of_its_main_rs() {
    let mut missing = Vec::new();
    for group in groups() {
        let dir = tests_dir().join(&group);
        let main = fs::read_to_string(dir.join("main.rs")).unwrap();
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let Some(stem) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
                continue;
            };
            if path.extension().is_none_or(|x| x != "rs") || stem == "main" {
                continue;
            }
            if !main.lines().any(|l| l.trim() == format!("mod {stem};")) {
                missing.push(format!("tests/{group}/{stem}.rs"));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "{missing:?} would not be built: add `mod <file>;` to its folder's main.rs"
    );
}
