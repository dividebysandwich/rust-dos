//! Wine's conformance tests for the built-in Windows (tests/conformance):
//! each test file is built with MinGW into a program, which runs under
//! the built-in Windows as it would on Windows 95. What each prints comes
//! down to its result lines, which must be those in
//! tests/conformance/expected.txt: a change there, for better or worse,
//! shows in that file's diff.
//!
//! The compiler is `i686-w64-mingw32-gcc`, or the one CONFORMANCE_CC
//! names, and the resource compiler `i686-w64-mingw32-windres`, or the
//! one CONFORMANCE_WINDRES names. Without a compiler the test says so and
//! passes, unless CONFORMANCE_REQUIRED is set, as in CI.
//! CONFORMANCE_BLESS=1 writes the results to expected.txt instead of
//! comparing them. CONFORMANCE_ONLY=user32/static runs only that test
//! (or a DLL's, or a comma-separated list of them) and prints its
//! results. CONFORMANCE_LOG=1 writes the emulator's log, with every call
//! a program makes, to target/conformance/<dll>/<test>.trace.

use rust_dos::cpu::{Cpu, CpuModel, HostConsole};
use rust_dos::exec::{NoHook, StopReason, run_batch};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const ROOT: &str = "tests/conformance";
const OUT: &str = "target/conformance";
/// The most batches of 100,000 instructions a test program may run for.
const BATCHES: usize = 20000;

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

/// The resource compiler.
fn windres() -> String {
    std::env::var("CONFORMANCE_WINDRES").unwrap_or_else(|_| "i686-w64-mingw32-windres".to_string())
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

/// Run the program `name` in `dir` with the argument `unit`, the test to
/// run, as Wine's runner passes it, on a processor of `model`: what it
/// printed, and its exit code if it ended.
fn run(dir: &Path, name: &str, unit: &str, model: CpuModel) -> (String, Option<u8>) {
    // The Windows and system directories the built-in Windows names,
    // and REGEDIT.EXE there, which file.c opens as a file every Windows
    // has.
    fs::create_dir_all(dir.join("WINDOWS").join("SYSTEM")).unwrap();
    fs::write(dir.join("WINDOWS").join("REGEDIT.EXE"), b"").unwrap();
    let mut cpu = Cpu::new(dir.to_path_buf());
    cpu.model = model;
    cpu.bus.win32.enabled = true;
    cpu.host_console = Some(HostConsole::default());
    cpu.load_shell();
    assert!(cpu.load_executable(name, None), "{} didn't load", name);
    cpu.set_command_tail(cpu.current_psp, unit);
    if std::env::var_os("CONFORMANCE_LOG").is_some() {
        let path = dir.join(format!("{}.trace", unit));
        // A program's calls fill more than a log file usually holds.
        let log = rust_dos::log::LogFile::create_with_limit(&path, 1 << 30);
        cpu.bus.log_file = Some(log.unwrap());
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
/// libraries, and the GUIDs of DirectX's and COM's interfaces, which
/// are data in MinGW's libdxguid and libuuid.
const LIBS: &[&str] = &[
    "-lddraw",
    "-ldsound",
    "-ldinput",
    "-lwinmm",
    "-lole32",
    "-luser32",
    "-lgdi32",
    "-lkernel32",
    "-ladvapi32",
    "-ldxguid",
    "-luuid",
    "-lksuser",
];

/// The test files of a DLL that call each other's helpers (dsound.c
/// plays its buffers with ds3d.c's `test_buffer`): they are built into
/// one program, named after the DLL, as Wine builds all of a DLL's, and
/// it runs once for each, with its name as the argument.
const SHARED: &[(&str, &[&str])] = &[
    ("dsound", &["capture", "ds3d", "ds3d8", "dsound", "dsound8", "propset"]),
    ("winmm", &["capture", "wave"]),
];

/// The files built with `unit` of `dll` into one program: those
/// `SHARED` lists with it, or it alone.
fn program_units(dll: &str, unit: &str) -> Vec<String> {
    SHARED
        .iter()
        .find(|(d, units)| *d == dll && units.contains(&unit))
        .map_or_else(
            || vec![unit.to_string()],
            |(_, units)| units.iter().map(|u| u.to_string()).collect(),
        )
}

/// Build the test files `units` of `dll` into `program`, as Wine does:
/// the files without STANDALONE, which leaves the framework out of them,
/// and a list of their tests with STANDALONE, which brings the framework
/// and `main` in. Some files have their own copies of the framework's
/// helpers, which a STANDALONE build of them would have twice.
///
/// MSVCRT's tests test the built-in MSVCRT.DLL, so they are linked as
/// MinGW links any program, with its startup code (crt2.o) and MSVCRT.DLL
/// as the C library, and runtime.c left out. Their sources use Wine's
/// names for va_list, which MinGW's headers don't have. GCC isn't to
/// count what sprintf returns itself, as it counts C99's formats, and
/// MinGW's headers aren't to put its own C99 functions (strtod) in place
/// of MSVCRT's.
fn build(
    cc: &str,
    dll: &str,
    units: &[String],
    runtime: &str,
    resources: Option<&str>,
    program: &str,
) -> Result<(), String> {
    let list = format!("{OUT}/{dll}/{}_list.c", units[0]);
    let externs: String = units
        .iter()
        .map(|u| format!("extern void func_{u}(void);\n"))
        .collect();
    let entries: String = units
        .iter()
        .map(|u| format!("{{ \"{u}\", func_{u} }}, "))
        .collect();
    fs::write(
        &list,
        format!(
            "#define STANDALONE
#include \"wine/test.h\"
{externs}const struct test winetest_testlist[] = {{ {entries}{{ 0, 0 }} }};
"
        ),
    )
    .unwrap();
    let sources: Vec<String> = units
        .iter()
        .map(|u| format!("{ROOT}/wine/{dll}/{u}.c"))
        .collect();
    let include = format!("-I{ROOT}/wine/include");
    let compat = format!("{ROOT}/runtime/compat.h");
    let mut args = CFLAGS.to_vec();
    args.extend([include.as_str(), "-include", compat.as_str()]);
    if units.iter().any(|u| u == "win") {
        // msg.c's dump_region, which win.c calls, is runtime.c's here.
        args.push("-Ddump_region=conformance_dump_region");
    }
    if dll == "winmm" && units.iter().any(|u| u == "capture") {
        // capture.c declares two GUIDs of mmreg.h again, without const.
        args.extend([
            "-DSTATIC_KSDATAFORMAT_SUBTYPE_PCM",
            "-DSTATIC_KSDATAFORMAT_SUBTYPE_IEEE_FLOAT",
        ]);
    }
    args.extend(sources.iter().map(String::as_str));
    if dll == "msvcrt" {
        args.extend(["-fno-printf-return-value", "-D__NO_ISOCEXT"]);
        args.extend([list.as_str(), "-lmsvcrt"]);
    } else {
        args.extend(["-nostdlib", "-Wl,-e,_mainCRTStartup"]);
        args.extend([list.as_str(), runtime]);
    }
    args.extend(resources);
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

    // CONFORMANCE_ONLY=user32/static (or a list, comma-separated, of
    // DLLs and tests) runs only those and prints their results without
    // comparing them, for working on them.
    let only: Option<Vec<String>> = std::env::var("CONFORMANCE_ONLY")
        .ok()
        .map(|s| s.split(',').map(str::to_string).collect());
    let wanted = |dll: &str, unit: &str| {
        only.as_ref().is_none_or(|o| {
            o.iter().any(|w| w == dll || *w == format!("{}/{}", dll, unit))
        })
    };
    let mut actual = Vec::new();
    for (dll, units) in units() {
        if !units.iter().any(|u| wanted(&dll, u)) {
            continue;
        }
        let dir = PathBuf::from(format!("{OUT}/{dll}"));
        // Each run starts from an empty drive: files an earlier run left
        // (file.c makes WIN.INI) change what directory listings count.
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        // The resources the DLL's tests share (user32's resource.rc),
        // which each of its programs is linked with.
        let rc = format!("{ROOT}/wine/{dll}/resource.rc");
        let resources = Path::new(&rc).exists().then(|| {
            let object = format!("{OUT}/{dll}/resource.o");
            let include = format!("-I{ROOT}/wine/include");
            let args = [include.as_str(), rc.as_str(), "-O", "coff", "-o", object.as_str()];
            compile(&windres(), &args)
                .unwrap_or_else(|e| panic!("{} doesn't build:\n{}", rc, e));
            object
        });
        // The programs built so far, by name, and how their builds went.
        let mut built: Vec<(String, Result<(), String>)> = Vec::new();
        for unit in units {
            if !wanted(&dll, &unit) {
                continue;
            }
            let together = program_units(&dll, &unit);
            let name = if together.len() > 1 {
                program_name(&dll)
            } else {
                program_name(&unit)
            };
            if !built.iter().any(|(n, _)| *n == name) {
                let program = format!("{OUT}/{dll}/{name}");
                let made = build(&cc, &dll, &together, &runtime, resources.as_deref(), &program);
                built.push((name.clone(), made));
            }
            let made = &built.iter().find(|(n, _)| *n == name).unwrap().1;
            let lines = match made {
                Ok(()) => {
                    // MinGW's startup code is built for a Pentium Pro
                    // (CMOVcc); runtime.c for a 486.
                    let model = if dll == "msvcrt" {
                        CpuModel::PentiumPro
                    } else {
                        CpuModel::I486
                    };
                    let (output, exit) = run(&dir, &name, &unit, model);
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
