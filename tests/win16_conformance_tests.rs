//! Wine's conformance tests ported to Windows 3.1 programs, for Win16
//! (tests/conformance16): each test file is built with OpenWatcom into
//! a Windows 3.1 program, which runs under the built-in Windows as it
//! would on Windows 95. Its result lines must be those in
//! tests/conformance16/expected.txt, as with tests/conformance_tests.rs.
//!
//! The compiler is OpenWatcom's `wcl`, or the one WIN16_CC names, with
//! WATCOM naming OpenWatcom's folder. Without one the test says so and
//! passes, unless CONFORMANCE_REQUIRED is set, as in CI.
//! CONFORMANCE_BLESS=1 writes the results to expected.txt instead of
//! comparing them, CONFORMANCE_ONLY=kernel/atom runs only that test (or
//! a module's, or a comma-separated list of them) and prints its
//! results, and CONFORMANCE_LOG=1 writes the emulator's log to
//! target/conformance16/<module>/<test>.trace.

use rust_dos::cpu::{Cpu, CpuModel, HostConsole};
use rust_dos::exec::{NoHook, StopReason, run_batch};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const ROOT: &str = "tests/conformance16";
const OUT: &str = "target/conformance16";
/// The most batches of 100,000 instructions a test program may run for.
const BATCHES: usize = 20000;

const HEADER: &str = "\
# The results of Wine's conformance tests ported to Windows 3.1 programs,
# under the built-in Windows's Win16, as tests/win16_conformance_tests.rs
# makes them: for each test file its summary, the checks that failed and
# the tests skipped (by file and line), or how the program ended before
# its summary. Write them anew with
# CONFORMANCE_BLESS=1 cargo test --test win16_conformance_tests.
";

/// OpenWatcom's compiler and its folder, if there are.
fn compiler() -> Option<(String, PathBuf)> {
    let cc = std::env::var("WIN16_CC").unwrap_or_else(|_| "wcl".to_string());
    let watcom = PathBuf::from(std::env::var_os("WATCOM")?);
    let found = Command::new(&cc).env("WATCOM", &watcom).output().is_ok();
    found.then_some((cc, watcom))
}

/// The modules whose tests there are, and the test files of each.
fn units() -> Vec<(String, Vec<String>)> {
    let mut modules: Vec<(String, Vec<String>)> = fs::read_dir(format!("{ROOT}/wine"))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| {
            let mut units: Vec<String> = fs::read_dir(e.path())
                .unwrap()
                .filter_map(|f| {
                    f.ok()?
                        .file_name()
                        .to_str()?
                        .strip_suffix(".c")
                        .map(str::to_string)
                })
                .collect();
            units.sort();
            (e.file_name().to_string_lossy().into_owned(), units)
        })
        .collect();
    modules.sort();
    modules
}

/// The 8.3 name a test file's program has on drive C:.
fn program_name(unit: &str) -> String {
    format!("{}.EXE", unit[..unit.len().min(8)].to_ascii_uppercase())
}

/// Build the test file `unit` of `module` into `program` in `dir`: in
/// the large model, as a Windows program. The compiler's messages if it
/// fails.
fn build(
    cc: &str,
    watcom: &Path,
    module: &str,
    unit: &str,
    dir: &Path,
    program: &str,
) -> Result<(), String> {
    let crate_dir = std::env::current_dir().unwrap();
    let source = crate_dir.join(format!("{ROOT}/wine/{module}/{unit}.c"));
    let runtime = crate_dir.join(format!("{ROOT}/runtime"));
    let include = std::env::join_paths([watcom.join("h"), watcom.join("h").join("win")]).unwrap();
    let output = Command::new(cc)
        .current_dir(dir)
        .env("WATCOM", watcom)
        .env("INCLUDE", include)
        .args(["-q", "-bt=windows", "-ml", "-zW", "-l=windows"])
        .arg(format!("-I{}", runtime.display()))
        .arg(format!("-fe={}", program))
        .arg(&source)
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() && dir.join(program).exists() {
        Ok(())
    } else {
        let mut text = output.stdout;
        text.extend(output.stderr);
        Err(String::from_utf8_lossy(&text).into_owned())
    }
}

/// Run the program `name` in `dir`: what it printed, and its exit code
/// if it ended.
fn run(dir: &Path, name: &str, unit: &str) -> (String, Option<u8>) {
    fs::create_dir_all(dir.join("WINDOWS").join("SYSTEM")).unwrap();
    let mut cpu = Cpu::new(dir.to_path_buf());
    cpu.model = CpuModel::I486;
    cpu.bus.win32.enabled = true;
    cpu.host_console = Some(HostConsole::default());
    cpu.load_shell();
    assert!(cpu.load_executable(name, None), "{} didn't load", name);
    if std::env::var_os("CONFORMANCE_LOG").is_some() {
        let path = dir.join(format!("{}.trace", unit));
        let log = rust_dos::log::LogFile::create_with_limit(&path, 1 << 30);
        cpu.bus.log_file = Some(log.unwrap());
        // Every call, not the first few from each place.
        rust_dos::win32::trace::set_level(&mut cpu, rust_dos::config::Win32Trace::All);
    }
    let mut ended = false;
    for _ in 0..BATCHES {
        cpu.bus.start_batch(cpu.bus.clock.icount + 100_000);
        if run_batch(&mut cpu, &mut NoHook, false) == StopReason::ShellReloaded {
            ended = true;
            break;
        }
    }
    if let Some(log) = &mut cpu.bus.log_file {
        log.flush();
    }
    let console = cpu.host_console.as_ref().unwrap();
    let mut text = console.stdout.clone();
    text.extend(&console.stderr);
    (
        String::from_utf8_lossy(&text).into_owned(),
        ended.then_some(cpu.errorlevel),
    )
}

/// The result lines of a program's output, as tests/conformance_tests.rs
/// makes them: its summary, the places of the checks that failed and the
/// tests skipped, and the message it ended with if it ended early.
fn results(output: &str, exit: Option<u8>) -> Vec<String> {
    let mut lines = Vec::new();
    let mut places: Vec<(String, usize)> = Vec::new();
    let mut summary = false;
    for line in output.lines().map(str::trim_end) {
        let place = |what: &str| {
            line.split_once(what)
                .map(|(at, _)| at.trim_end_matches(':').to_string())
        };
        let found = place(": Test failed:")
            .map(|at| format!("failed {}", at))
            .or_else(|| {
                place(": Test succeeded inside todo block:")
                    .map(|at| format!("todo succeeded {}", at))
            })
            .or_else(|| place(": Tests skipped:").map(|at| format!("skipped {}", at)));
        if let Some(found) = found {
            match places.iter_mut().find(|(p, _)| *p == found) {
                Some((_, n)) => *n += 1,
                None => places.push((found, 1)),
            }
        } else if line.contains(" tests executed (") {
            summary = true;
            lines.push(line.to_string());
        } else if line.contains("which the built-in Windows doesn't have yet")
            || line.contains(" ended: ")
        {
            let message = line.split(" (called from ").next().unwrap_or(line);
            let message = message.split(" at 0").next().unwrap_or(message);
            lines.push(format!("ended: {}", message.trim_end_matches('.')));
        }
    }
    lines.extend(
        places
            .into_iter()
            .map(|(p, n)| if n > 1 { format!("{} (x{})", p, n) } else { p }),
    );
    match exit {
        None => lines.push("did not end".to_string()),
        Some(code) if !summary => lines.push(format!("exit code {}", code)),
        Some(_) => {}
    }
    lines
}

#[test]
fn win16_conformance_tests_give_the_expected_results() {
    let Some((cc, watcom)) = compiler() else {
        assert!(
            std::env::var_os("CONFORMANCE_REQUIRED").is_none(),
            "CONFORMANCE_REQUIRED is set, but there is no OpenWatcom (WATCOM and wcl, or WIN16_CC)"
        );
        eprintln!(
            "No OpenWatcom (WATCOM and wcl, or WIN16_CC): the Win16 conformance tests didn't run."
        );
        return;
    };
    let only: Option<Vec<String>> = std::env::var("CONFORMANCE_ONLY")
        .ok()
        .map(|s| s.split(',').map(str::to_string).collect());
    let wanted = |module: &str, unit: &str| {
        only.as_ref().is_none_or(|o| {
            o.iter()
                .any(|w| w == module || *w == format!("{}/{}", module, unit))
        })
    };
    let mut actual = Vec::new();
    for (module, units) in units() {
        if !units.iter().any(|u| wanted(&module, u)) {
            continue;
        }
        let dir = PathBuf::from(format!("{OUT}/{module}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for unit in units {
            if !wanted(&module, &unit) {
                continue;
            }
            let name = program_name(&unit);
            let lines = match build(&cc, &watcom, &module, &unit, &dir, &name) {
                Ok(()) => {
                    let (output, exit) = run(&dir, &name, &unit);
                    fs::write(dir.join(format!("{}.log", unit)), &output).unwrap();
                    results(&output, exit)
                }
                Err(e) => {
                    eprintln!("{}/{} doesn't build:\n{}", module, unit, e);
                    vec!["does not build".to_string()]
                }
            };
            actual.extend(
                lines
                    .into_iter()
                    .map(|l| format!("{}/{}: {}", module, unit, l)),
            );
        }
    }

    let path = format!("{ROOT}/expected.txt");
    if only.is_some() {
        println!("{}", actual.join("\n"));
        return;
    }
    if std::env::var_os("CONFORMANCE_BLESS").is_some() {
        fs::write(&path, format!("{}{}\n", HEADER, actual.join("\n"))).unwrap();
        return;
    }
    let expected: Vec<String> = fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .map(str::to_string)
        .collect();
    if actual != expected {
        let gone: Vec<&String> = expected.iter().filter(|l| !actual.contains(l)).collect();
        let new: Vec<&String> = actual.iter().filter(|l| !expected.contains(l)).collect();
        panic!(
            "The Win16 conformance results differ from {}.\n\nOnly expected:\n{}\n\nOnly now:\n{}\n\n\
             The programs' output is in {}/<module>/<test>.log. If the change is right, \
             write the results anew with CONFORMANCE_BLESS=1.",
            path,
            gone.iter()
                .map(|l| format!("  {}", l))
                .collect::<Vec<_>>()
                .join("\n"),
            new.iter()
                .map(|l| format!("  {}", l))
                .collect::<Vec<_>>()
                .join("\n"),
            OUT
        );
    }
}
