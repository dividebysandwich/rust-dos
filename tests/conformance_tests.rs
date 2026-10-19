//! Wine's conformance tests for the built-in Windows (tests/conformance):
//! each test file is built with MinGW into a program, which runs under
//! the built-in Windows as it would on Windows 95. What each prints comes
//! down to its result lines, which must be those in
//! tests/conformance/expected.txt: a change there, for better or worse,
//! shows in that file's diff.
//!
//! The compiler is `i686-w64-mingw32-gcc`, or the one CONFORMANCE_CC
//! names. Without one the test says so and passes, unless
//! CONFORMANCE_REQUIRED is set, as in CI. CONFORMANCE_BLESS=1 writes the
//! results to expected.txt instead of comparing them.

use rust_dos::cpu::{Cpu, HostConsole};
use rust_dos::exec::{NoHook, StopReason, run_batch};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const ROOT: &str = "tests/conformance";
const OUT: &str = "target/conformance";
/// The most batches of 100,000 instructions a test program may run for.
const BATCHES: usize = 5000;

const HEADER: &str = "\
# The results of Wine's conformance tests under the built-in Windows, as
# tests/conformance_tests.rs makes them: for each test file its summary,
# the checks that failed and the tests skipped (by file and line), or how
# the program ended before its summary. Write them anew with
# CONFORMANCE_BLESS=1 cargo test --test conformance_tests.
";

/// The compiler, if there is one.
fn compiler() -> Option<String> {
    let cc = std::env::var("CONFORMANCE_CC").unwrap_or_else(|_| "i686-w64-mingw32-gcc".to_string());
    let found = Command::new(&cc)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    found.then_some(cc)
}

/// Run the compiler with `args`; its messages if it fails.
fn compile(cc: &str, args: &[&str]) -> Result<(), String> {
    let output = Command::new(cc)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned())
    }
}

/// The options both the runtime and the tests are built with: code for
/// a 486, which the machine is, as programs of Windows 95's time are;
/// MSVCRT's plain printf and ctype functions, which the runtime has,
/// rather than MinGW's own; and no warnings, which old Wine sources have
/// plenty of.
const CFLAGS: &[&str] = &[
    "-O1",
    "-march=i486",
    "-w",
    "-D__USE_MINGW_ANSI_STDIO=0",
    "-D__NO_CTYPE_INLINES",
];

/// The DLLs whose tests there are, and the test files of each, by name.
fn units() -> Vec<(String, Vec<String>)> {
    let mut dlls: Vec<(String, Vec<String>)> = fs::read_dir(format!("{ROOT}/wine"))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir() && e.file_name() != "include")
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
    dlls.sort();
    dlls
}

/// The 8.3 name a test file's program has on drive C:.
fn program_name(unit: &str) -> String {
    format!("{}.EXE", unit[..unit.len().min(8)].to_ascii_uppercase())
}

/// Run the program `name` in `dir` until it ends: what it printed, and
/// its exit code if it ended.
fn run(dir: &Path, name: &str) -> (String, Option<u8>) {
    let mut cpu = Cpu::new(dir.to_path_buf());
    cpu.bus.win32.enabled = true;
    cpu.host_console = Some(HostConsole::default());
    cpu.load_shell();
    assert!(cpu.load_executable(name, None), "{} didn't load", name);
    let mut ended = false;
    for _ in 0..BATCHES {
        cpu.bus.start_batch(cpu.bus.clock.icount + 100_000);
        if run_batch(&mut cpu, &mut NoHook, false) == StopReason::ShellReloaded {
            ended = true;
            break;
        }
    }
    let console = cpu.host_console.as_ref().unwrap();
    let mut text = console.stdout.clone();
    text.extend(&console.stderr);
    (
        String::from_utf8_lossy(&text).into_owned(),
        ended.then_some(cpu.errorlevel),
    )
}

/// The result lines of a program's output: its summary, the places of the
/// checks that failed and the tests skipped (with how many times, when
/// more than once), and the message it ended with if it ended early.
/// Values in the messages are left out, as they may differ between runs.
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
            // Without the addresses, which change with the build.
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

/// The DLLs the tests may import from, as MinGW names their import
/// libraries.
const LIBS: &[&str] = &["-lkernel32", "-ladvapi32"];

/// Build the test file `unit` of `dll` into `program`, as Wine does: the
/// file without STANDALONE, which leaves the framework out of it, and a
/// list of its one test with STANDALONE, which brings the framework and
/// `main` in. Some files have their own copies of the framework's
/// helpers, which a STANDALONE build of them would have twice.
fn build(cc: &str, dll: &str, unit: &str, runtime: &str, program: &str) -> Result<(), String> {
    let list = format!("{OUT}/{dll}/{unit}_list.c");
    fs::write(
        &list,
        format!(
            "#define STANDALONE
#include \"wine/test.h\"
             extern void func_{unit}(void);
             const struct test winetest_testlist[] = {{ {{ \"{unit}\", func_{unit} }}, {{ 0, 0 }} }};
"
        ),
    )
    .unwrap();
    let source = format!("{ROOT}/wine/{dll}/{unit}.c");
    let include = format!("-I{ROOT}/wine/include");
    let mut args = CFLAGS.to_vec();
    args.extend([include.as_str(), "-nostdlib", "-Wl,-e,_mainCRTStartup"]);
    args.extend([source.as_str(), list.as_str(), runtime]);
    args.extend(LIBS);
    args.extend(["-lgcc", "-o", program]);
    compile(cc, &args)
}

#[test]
fn wine_conformance_tests_give_the_expected_results() {
    let Some(cc) = compiler() else {
        assert!(
            std::env::var_os("CONFORMANCE_REQUIRED").is_none(),
            "CONFORMANCE_REQUIRED is set, but there is no i686-w64-mingw32-gcc (or CONFORMANCE_CC)"
        );
        eprintln!("No i686-w64-mingw32-gcc (or CONFORMANCE_CC): the conformance tests didn't run.");
        return;
    };
    fs::create_dir_all(OUT).unwrap();
    let runtime = format!("{OUT}/runtime.o");
    let source = format!("{ROOT}/runtime/runtime.c");
    let mut args = CFLAGS.to_vec();
    args.extend([
        "-ffreestanding",
        "-fno-tree-loop-distribute-patterns",
        "-c",
        &source,
        "-o",
        &runtime,
    ]);
    compile(&cc, &args).unwrap_or_else(|e| panic!("the runtime doesn't build:\n{}", e));

    let mut actual = Vec::new();
    for (dll, units) in units() {
        let dir = PathBuf::from(format!("{OUT}/{dll}"));
        fs::create_dir_all(&dir).unwrap();
        for unit in units {
            let name = program_name(&unit);
            let program = format!("{OUT}/{dll}/{name}");
            let lines = match build(&cc, &dll, &unit, &runtime, &program) {
                Ok(()) => {
                    let (output, exit) = run(&dir, &name);
                    fs::write(dir.join(format!("{}.log", unit)), &output).unwrap();
                    results(&output, exit)
                }
                Err(e) => {
                    eprintln!("{}/{} doesn't build:\n{}", dll, unit, e);
                    vec!["does not build".to_string()]
                }
            };
            actual.extend(
                lines
                    .into_iter()
                    .map(|l| format!("{}/{}: {}", dll, unit, l)),
            );
        }
    }

    let path = format!("{ROOT}/expected.txt");
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
            "The conformance results differ from {}.\n\nOnly expected:\n{}\n\nOnly now:\n{}\n\n\
             The programs' output is in {}/<dll>/<test>.log. If the change is right, \
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
