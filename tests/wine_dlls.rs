//! Wine's DLLs as guest code (the `win32_dlls` setting): a program built
//! with MinGW (tests/wine_dlls/acm.c) runs under the built-in Windows
//! with Wine's MSACM32.DLL and IMA ADPCM codec, loaded from the folder
//! WINE_DLLS names (Wine's lib/wine/i386-windows of a 32-bit build), and
//! encodes and decodes audio through them.
//!
//! Wine's DLLs aren't in the repository: without WINE_DLLS, or without a
//! compiler (`i686-w64-mingw32-gcc`, or the one CONFORMANCE_CC names),
//! the test says so and passes. WINE_DLLS_LOG=1 writes the emulator's
//! log to target/wine-dlls/acm.trace.

use rust_dos::cpu::{Cpu, CpuModel, HostConsole};
use rust_dos::exec::{NoHook, StopReason, run_batch};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Where the program is built and run: drive C:. A path relative to the
/// crate, which a compiler run through WSL can take as well.
const OUT: &str = "target/wine-dlls";
/// The most batches of 100,000 instructions the program may run for.
const BATCHES: usize = 20000;

/// The compiler, if there is one.
fn compiler() -> Option<String> {
    let cc = std::env::var("CONFORMANCE_CC").unwrap_or_else(|_| "i686-w64-mingw32-gcc".to_string());
    let found = Command::new(&cc)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    found.then_some(cc)
}

/// Run the program `name` on drive C: at `dir`, with the DLLs of
/// `dlls` as guest code: what it printed, and its exit code if it ended.
fn run(dir: &Path, name: &str, dlls: &Path) -> (String, Option<u8>) {
    let mut cpu = Cpu::new(dir.to_path_buf());
    // Wine's DLLs are built for the i686: they use CMOV.
    cpu.model = CpuModel::PentiumII;
    cpu.bus.win32.enabled = true;
    cpu.bus.win32.guest_dlls = Some(dlls.to_path_buf());
    cpu.host_console = Some(HostConsole::default());
    cpu.load_shell();
    assert!(cpu.load_executable(name, None), "{} didn't load", name);
    if std::env::var_os("WINE_DLLS_LOG").is_some() {
        let log = rust_dos::log::LogFile::create_with_limit(&dir.join("acm.trace"), 1 << 30);
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

#[test]
fn wine_msacm32_converts_ima_adpcm() {
    let Some(dlls) = std::env::var_os("WINE_DLLS").map(PathBuf::from) else {
        eprintln!("No WINE_DLLS: Wine's DLLs didn't run.");
        return;
    };
    let Some(cc) = compiler() else {
        eprintln!("No i686-w64-mingw32-gcc (or CONFORMANCE_CC): Wine's DLLs didn't run.");
        return;
    };
    let dir = Path::new(OUT);
    let system = dir.join("WINDOWS").join("SYSTEM");
    fs::create_dir_all(&system).unwrap();
    // The codec is an installable driver, which MSACM32 finds in
    // SYSTEM.INI.
    fs::write(
        dir.join("WINDOWS").join("SYSTEM.INI"),
        b"[drivers32]\r\nmsacm.imaadpcm=imaadp32.acm\r\n",
    )
    .unwrap();
    let exe = format!("{OUT}/ACM.EXE");
    let output = Command::new(&cc)
        .args(["-O1", "-march=i486", "-w", "-D__USE_MINGW_ANSI_STDIO=0"])
        .args(["-o", &exe, "tests/wine_dlls/acm.c", "-lmsacm32", "-lwinmm"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let (text, exit) = run(dir, "ACM.EXE", &dlls);
    println!("{}", text);
    assert!(text.contains("all ok"), "{}", text);
    assert_eq!(exit, Some(0), "{}", text);
}
