//! The built-in Windows's keyboard speed and delay: read as Windows starts
//! from the registry (`HKEY_CURRENT_USER\Control Panel\Keyboard`, Windows
//! 95 and 98) or WIN.INI (`[windows]`, Windows 3.1), given to the
//! keyboard as its typematic rate, and written back to the registry by
//! SystemParametersInfo with SPIF_UPDATEINIFILE. The programs are PE
//! files made here with rust-dos-pe's builder, their code assembled with
//! iced.

use iced_x86::code_asm::*;
use rust_dos::config::WindowsVersion;
use rust_dos::cpu::{Cpu, HostConsole};
use rust_dos::exec::{NoHook, StopReason, run_batch};
use rust_dos_pe::{self as pe, Builder, Imp};
use std::fs;
use std::path::{Path, PathBuf};

const BASE: u32 = 0x40_0000;

const SPI_GETKEYBOARDSPEED: i32 = 10;
const SPI_SETKEYBOARDSPEED: i32 = 11;
const SPI_GETKEYBOARDDELAY: i32 = 22;
const SPI_SETKEYBOARDDELAY: i32 = 23;
const SPIF_UPDATEINIFILE: i32 = 1;

/// A program calling USER32's SystemParametersInfoA with `calls`, each
/// an action, its `ui` and its flags, and then ending with the keyboard
/// delay SPI_GETKEYBOARDDELAY gives in bits 5-6 of its exit code and the
/// speed SPI_GETKEYBOARDSPEED gives in bits 0-4.
fn program(calls: &[(i32, i32, i32)]) -> Vec<u8> {
    let mut b = Builder::exe(BASE);
    let imports = [Imp::Name("SystemParametersInfoA")];
    let iat = b.imports(&[("USER32.DLL", &imports)]).concat();
    let data_rva = b.section(
        ".data",
        &[0; 8],
        pe::SCN_INITIALIZED_DATA | pe::SCN_READ | pe::SCN_WRITE,
    );
    let (speed, delay) = ((BASE + data_rva) as u64, (BASE + data_rva + 4) as u64);
    let spi = dword_ptr(iat[0] as u64);
    let code_rva = b.next_rva();
    let mut a = CodeAssembler::new(32).unwrap();
    let call = |a: &mut CodeAssembler, action: i32, ui: i32, pv: u64, flags: i32| {
        a.push(flags).unwrap();
        a.push(pv as i32).unwrap();
        a.push(ui).unwrap();
        a.push(action).unwrap();
        a.call(spi).unwrap();
    };
    for &(action, ui, flags) in calls {
        call(&mut a, action, ui, 0, flags);
    }
    call(&mut a, SPI_GETKEYBOARDSPEED, 0, speed, 0);
    call(&mut a, SPI_GETKEYBOARDDELAY, 0, delay, 0);
    a.mov(eax, dword_ptr(delay)).unwrap();
    a.shl(eax, 5).unwrap();
    a.or(eax, dword_ptr(speed)).unwrap();
    a.ret().unwrap();
    let code = a.assemble((BASE + code_rva) as u64).unwrap();
    b.section(
        ".text",
        &code,
        pe::SCN_CODE | pe::SCN_EXECUTE | pe::SCN_READ,
    );
    b.entry(code_rva);
    b.build()
}

/// A folder for the test `name`, drive C:, with `registry` as its
/// REGISTRY.REG and `win_ini` as its WINDOWS\WIN.INI, where given.
fn scratch(name: &str, registry: Option<&str>, win_ini: Option<&str>) -> PathBuf {
    let base = PathBuf::from("target/test_win32_keyboard").join(name);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(base.join("WINDOWS")).unwrap();
    if let Some(text) = registry {
        fs::write(base.join("REGISTRY.REG"), text).unwrap();
    }
    if let Some(text) = win_ini {
        fs::write(base.join("WINDOWS").join("WIN.INI"), text).unwrap();
    }
    base
}

/// Run `program` as T.EXE from `dir` on the built-in Windows 98 SE to its
/// end: its exit code, and the keyboard's typematic byte after it.
fn run(dir: &Path, program: Vec<u8>) -> (u8, u8) {
    fs::write(dir.join("T.EXE"), program).unwrap();
    let mut cpu = Cpu::new(dir.to_path_buf());
    cpu.bus.win32.enabled = true;
    cpu.bus.win32.version = WindowsVersion::Win98Se;
    cpu.host_console = Some(HostConsole::default());
    cpu.load_shell();
    assert!(cpu.load_executable("T.EXE", None));
    let mut ended = false;
    for _ in 0..400 {
        cpu.bus.start_batch(cpu.bus.clock.icount + 100_000);
        if run_batch(&mut cpu, &mut NoHook, false) == StopReason::ShellReloaded {
            ended = true;
            break;
        }
    }
    assert!(ended, "the program didn't end");
    (cpu.errorlevel, cpu.bus.kbc.typematic)
}

/// The exit code `program` ends with for `delay` and `speed`.
fn settings(delay: u8, speed: u8) -> u8 {
    delay << 5 | speed
}

#[test]
fn without_settings_the_keyboard_has_windows_defaults() {
    let dir = scratch("defaults", None, None);
    let (code, typematic) = run(&dir, program(&[]));
    assert_eq!(code, settings(1, 31));
    // 500 ms, then 30 a second.
    assert_eq!(typematic, 0x20);
}

/// A Windows 3.1 setup's WIN.INI: the registry has no keyboard key.
#[test]
fn the_keyboard_settings_come_from_win_ini() {
    let win_ini = "[windows]\r\nKeyboardSpeed=20\r\nKeyboardDelay=0\r\n";
    let dir = scratch("win_ini", None, Some(win_ini));
    let (code, typematic) = run(&dir, program(&[]));
    assert_eq!(code, settings(0, 20));
    // 250 ms, rate 31 - 20.
    assert_eq!(typematic, 0x0B);
}

/// Windows 95 and 98 read the registry. A value it doesn't have comes
/// from WIN.INI, as Windows 95's setup would have moved it there.
#[test]
fn the_registry_comes_before_win_ini() {
    let registry = "REGEDIT4\r\n\r\n\
                    [HKEY_CURRENT_USER\\Control Panel\\Keyboard]\r\n\
                    \"KeyboardDelay\"=\"2\"\r\n";
    let win_ini = "[windows]\r\nKeyboardSpeed=20\r\nKeyboardDelay=0\r\n";
    let dir = scratch("registry", Some(registry), Some(win_ini));
    let (code, typematic) = run(&dir, program(&[]));
    assert_eq!(code, settings(2, 20));
    assert_eq!(typematic, 0x4B);
}

/// Values out of range are the largest, as SystemParametersInfo takes
/// them; text that isn't a number is no value.
#[test]
fn out_of_range_settings_are_the_largest() {
    let registry = "REGEDIT4\r\n\r\n\
                    [HKEY_USERS\\.Default\\Control Panel\\Keyboard]\r\n\
                    \"KeyboardSpeed\"=\"40\"\r\n\
                    \"KeyboardDelay\"=\"fast\"\r\n";
    let win_ini = "[windows]\r\nKeyboardDelay=7\r\n";
    let dir = scratch("range", Some(registry), Some(win_ini));
    let (code, typematic) = run(&dir, program(&[]));
    assert_eq!(code, settings(3, 31));
    assert_eq!(typematic, 0x60);
}

/// SPIF_UPDATEINIFILE writes the value to the registry, which the next
/// start reads; a change without it lasts until Windows ends.
#[test]
fn spif_updateinifile_keeps_the_setting_for_the_next_start() {
    let dir = scratch("update", None, None);
    let calls = [
        (SPI_SETKEYBOARDDELAY, 0, SPIF_UPDATEINIFILE),
        (SPI_SETKEYBOARDSPEED, 10, 0),
    ];
    let (code, typematic) = run(&dir, program(&calls));
    assert_eq!(code, settings(0, 10));
    assert_eq!(typematic, 0x15);
    let saved = fs::read_to_string(dir.join("REGISTRY.REG")).unwrap();
    let key = "[HKEY_USERS\\.Default\\Control Panel\\Keyboard]\r\n\"KeyboardDelay\"=\"0\"\r\n";
    assert!(saved.contains(key), "{}", saved);
    assert!(!saved.contains("KeyboardSpeed"), "{}", saved);
    // The next start: the delay saved, the speed Windows' default.
    let (code, typematic) = run(&dir, program(&[]));
    assert_eq!(code, settings(0, 31));
    assert_eq!(typematic, 0x00);
}
