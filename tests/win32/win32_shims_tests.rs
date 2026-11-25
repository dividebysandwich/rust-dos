//! Compatibility shims (src/win32/shims): a program a game fix names
//! (`game_fixes::Kind::Win32Shims`) runs with an older Windows's
//! behaviour of a function, and every other program with Windows 98's.
//! The programs are PE files made here with rust-dos-pe's builder, their
//! code assembled with iced, under the name and at the size of the
//! game's program file.

use iced_x86::code_asm::*;
use rust_dos::cpu::{Cpu, HostConsole};
use rust_dos::exec::{NoHook, StopReason, run_batch};
use rust_dos::game_fixes::{FIXES, Kind};
use rust_dos_pe::{self as pe, Builder, Imp};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const BASE: u32 = 0x40_0000;
const GMEM_MOVEABLE: i32 = 2;

/// The functions the program imports, in the order of its import table.
const NAMES: &[&str] = &[
    "GlobalAlloc",
    "GlobalLock",
    "GlobalUnlock",
    "GlobalFlags",
    "LocalFlags",
];

/// A program that makes a moveable block, locks it twice and unlocks it
/// once, and ends with GlobalFlags' lock count, LocalFlags' times 10h,
/// and 80h if GlobalUnlock said the block was still locked. On Windows
/// 98 that is 81h | 11h; with Win32s's GlobalFlags, 81h | 10h.
fn program(size: usize) -> Vec<u8> {
    let mut b = Builder::exe(BASE);
    b.subsystem(pe::SUBSYSTEM_CONSOLE);
    let imports: Vec<Imp> = NAMES.iter().map(|n| Imp::Name(n)).collect();
    let iat = b.imports(&[("KERNEL32.DLL", &imports)]).concat();
    let f = |name: &str| dword_ptr(iat[NAMES.iter().position(|n| *n == name).unwrap()] as u64);
    let code_rva = b.next_rva();
    let mut a = CodeAssembler::new(32).unwrap();
    a.push(0x200).unwrap();
    a.push(GMEM_MOVEABLE).unwrap();
    a.call(f("GlobalAlloc")).unwrap();
    a.mov(esi, eax).unwrap();
    for _ in 0..2 {
        a.push(esi).unwrap();
        a.call(f("GlobalLock")).unwrap();
    }
    a.push(esi).unwrap();
    a.call(f("GlobalUnlock")).unwrap();
    a.shl(eax, 7).unwrap();
    a.mov(edi, eax).unwrap();
    a.push(esi).unwrap();
    a.call(f("GlobalFlags")).unwrap();
    a.and(eax, 0xFF).unwrap();
    a.or(edi, eax).unwrap();
    a.push(esi).unwrap();
    a.call(f("LocalFlags")).unwrap();
    a.and(eax, 0xFF).unwrap();
    a.shl(eax, 4).unwrap();
    a.or(eax, edi).unwrap();
    a.ret().unwrap();
    let code = a.assemble((BASE + code_rva) as u64).unwrap();
    let rva = b.section(
        ".text",
        &code,
        pe::SCN_CODE | pe::SCN_EXECUTE | pe::SCN_READ,
    );
    assert_eq!(rva, code_rva);
    b.entry(code_rva);
    let mut file = b.build();
    assert!(file.len() <= size);
    file.resize(size, 0);
    file
}

/// Run `program`'s file of `size` bytes as `name`, with `game_fixes` at
/// `fixes`: its exit code, and the game fix lines of the log.
fn run(test: &str, name: &str, size: usize, fixes: bool) -> (u8, Vec<String>) {
    let dir = PathBuf::from("target/test_win32").join(test);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(name), program(size)).unwrap();
    let mut cpu = Cpu::new(dir);
    cpu.bus.win32.enabled = true;
    cpu.bus.game_fixes = fixes;
    cpu.host_console = Some(HostConsole::default());
    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink = lines.clone();
    cpu.bus.log_hook = Some(Box::new(move |line: &str| {
        if line.contains("[game fix]") {
            sink.lock().unwrap().push(line.to_string());
        }
    }));
    cpu.load_shell();
    assert!(cpu.load_executable(name, None));
    let mut ended = false;
    for _ in 0..200 {
        cpu.bus.start_batch(cpu.bus.clock.icount + 100_000);
        if run_batch(&mut cpu, &mut NoHook, false) == StopReason::ShellReloaded {
            ended = true;
            break;
        }
    }
    assert!(ended, "{name} didn't end");
    let lines = lines.lock().unwrap().clone();
    (cpu.errorlevel, lines)
}

/// The programs the game fixes give shims, with their fix's name and
/// their file's size.
fn shimmed() -> Vec<(&'static str, &'static str, usize)> {
    let found: Vec<_> = FIXES
        .iter()
        .filter_map(|fix| match fix.kind {
            Kind::Win32Shims { program, size, .. } => Some((fix.name, program, size)),
            _ => None,
        })
        .collect();
    assert!(!found.is_empty());
    found
}

#[test]
fn a_win32s_program_gets_global_flags_without_the_lock_count() {
    for (fix, program, size) in shimmed() {
        let (code, lines) = run(&format!("shims_{fix}"), program, size, true);
        // GlobalUnlock and LocalFlags still count the two locks less the
        // one unlock; GlobalFlags gives 0.
        assert_eq!(code, 0x90, "{program}");
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains(fix), "{lines:?}");
    }
}

#[test]
fn other_programs_get_windows_98_s_global_flags() {
    for (fix, program, size) in shimmed() {
        // With the game fixes off, of another size, and of another name.
        let runs = [
            (format!("shims_{fix}_off"), program, size, false),
            (format!("shims_{fix}_size"), program, size + 0x200, true),
            (format!("shims_{fix}_name"), "OTHER.EXE", size, true),
        ];
        for (test, name, size, fixes) in runs {
            let (code, lines) = run(&test, name, size, fixes);
            assert_eq!(code, 0x91, "{test}");
            assert!(lines.is_empty(), "{test}: {lines:?}");
        }
    }
}
