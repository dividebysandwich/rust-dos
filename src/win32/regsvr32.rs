//! C:\WINDOWS\SYSTEM\REGSVR32.EXE, which setups run to register a DLL:
//! Leadfoot's setup runs `regsvr32.exe /s ir50_32.dll` for the Indeo 5
//! codec, and says "Unable to install the Indeo codec." when it can't.
//!
//! Its command line is Windows 98's: `regsvr32 [/u] [/s] [/n]
//! [/i[:cmdline]] dllname`. It loads the DLL and calls its
//! DllRegisterServer, or DllUnregisterServer with `/u`. `/i` calls
//! DllInstall too (with TRUE, or FALSE with `/u`, and the text after
//! `/i:`), after the registration or, with `/u`, before it, and `/n`
//! with `/i` calls DllInstall alone. It says how that went in a message
//! box, which `/s` leaves out, and ends with Windows' exit codes: 0 when
//! it worked, 1 for a bad command line, 2 when OleInitialize fails, 3
//! when the DLL doesn't load, 4 when the function isn't there, and 5
//! when it fails.
//!
//! The file is a PE image the emulator builds (`file`), shown in the
//! system folder with the built-in DLLs (dll_files.rs). Its code is a
//! loop around Regsvr32Main, a function of ADVAPI32 that ADVAPI32
//! doesn't export: each call takes the result of the step before and
//! says which step the program takes next (`Step`), and the program
//! takes it: OleInitialize, LoadLibraryA, GetProcAddress, the call of
//! the DLL's function, the message box, FreeLibrary, OleUninitialize,
//! and at the end ExitProcess. The DLL's code runs in the program's own
//! process, as on Windows.

use crate::cpu::Cpu;
use rust_dos_pe::{self as pe, Builder, Imp};

/// Where the program is loaded.
const BASE: u32 = 0x40_0000;

/// The program's data, by offset: what Regsvr32Main keeps between its
/// calls, and what the program's steps read.
const D_STEP: u32 = 0;
/// The exit code.
const D_EXIT: u32 = 4;
/// The DLL's module handle, once it is loaded.
const D_MODULE: u32 = 8;
/// The address of the function to call.
const D_PROC: u32 = 12;
/// Its two arguments; DllRegisterServer takes none, and the program
/// puts its stack pointer back after the call.
const D_ARG0: u32 = 16;
const D_ARG1: u32 = 20;
/// The message box's type.
const D_TYPE: u32 = 24;
/// The calls still to make, DllInstall's place among them.
const D_CALLS: u32 = 28;
/// The DLL's name, ANSI.
const D_PATH: u32 = 32;
const PATH_SIZE: u32 = 260;
/// The name of the function to call.
const D_ENTRY: u32 = D_PATH + PATH_SIZE;
const ENTRY_SIZE: u32 = 32;
/// DllInstall's command line, UTF-16.
const D_WIDE: u32 = D_ENTRY + ENTRY_SIZE;
const WIDE_SIZE: u32 = 520;
const D_CAPTION: u32 = D_WIDE + WIDE_SIZE;
const CAPTION_SIZE: u32 = 64;
const D_TEXT: u32 = D_CAPTION + CAPTION_SIZE;
const TEXT_SIZE: u32 = 1024;
const DATA_SIZE: u32 = D_TEXT + TEXT_SIZE;

const MB_ICONERROR: u32 = 0x10;
const MB_ICONINFORMATION: u32 = 0x40;

/// The exit codes of Windows' REGSVR32.
const OK: u32 = 0;
const FAIL_ARGS: u32 = 1;
const FAIL_OLE: u32 = 2;
const FAIL_LOAD: u32 = 3;
const FAIL_ENTRY: u32 = 4;
const FAIL_REG: u32 = 5;

/// The caption of REGSVR32's message boxes.
const CAPTION: &str = "RegSvr32";

/// What the program does next, as Regsvr32Main returns it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
enum Step {
    /// ExitProcess with the exit code.
    Exit = 0,
    OleInitialize = 1,
    /// LoadLibraryA of the DLL's name.
    Load = 2,
    /// GetProcAddress of the function's name in the DLL.
    Find = 3,
    /// The function, with its arguments.
    Call = 4,
    /// MessageBoxA with the caption and text.
    Message = 5,
    FreeLibrary = 6,
    OleUninitialize = 7,
}

/// Where Regsvr32Main is in its work, kept in the data between calls.
mod stage {
    pub const START: u32 = 0;
    pub const INITIALIZED: u32 = 1;
    pub const LOADED: u32 = 2;
    pub const FOUND: u32 = 3;
    pub const CALLED: u32 = 4;
    pub const SHOWN: u32 = 5;
    pub const FREED: u32 = 6;
    pub const DONE: u32 = 7;
}

/// The file of REGSVR32.EXE: a program for the GUI subsystem that calls
/// Regsvr32Main(data, result) and takes the step it returns, with the
/// step's result in the next call, until the step is ExitProcess.
pub fn file(bus: &crate::bus::Bus) -> Vec<u8> {
    let mut b = Builder::exe(BASE);
    b.subsystem(pe::SUBSYSTEM_GUI);
    let kernel32 = [
        Imp::Name("ExitProcess"),
        Imp::Name("LoadLibraryA"),
        Imp::Name("GetProcAddress"),
        Imp::Name("FreeLibrary"),
    ];
    let user32 = [Imp::Name("MessageBoxA")];
    let ole32 = [Imp::Name("OleInitialize"), Imp::Name("OleUninitialize")];
    // An import of ADVAPI32 puts it in the process, Regsvr32Main with it.
    let advapi32 = [Imp::Name("RegCloseKey")];
    let iat = b.imports(&[
        ("KERNEL32.DLL", &kernel32),
        ("USER32.DLL", &user32),
        ("OLE32.DLL", &ole32),
        ("ADVAPI32.DLL", &advapi32),
    ]);
    let [exit, load, find, free] = [iat[0][0], iat[0][1], iat[0][2], iat[0][3]];
    let message_box = iat[1][0];
    let [initialize, uninitialize] = [iat[2][0], iat[2][1]];
    let flags = pe::SCN_INITIALIZED_DATA | pe::SCN_READ | pe::SCN_WRITE;
    let data = BASE + b.section_sized(".data", &[], DATA_SIZE, flags);
    let main = super::advapi32::address_for_new(bus, "Regsvr32Main");
    let text_rva = b.section(
        ".text",
        &code(
            data,
            main,
            Imports {
                exit,
                load,
                find,
                free,
                message_box,
                initialize,
                uninitialize,
            },
        ),
        pe::SCN_CODE | pe::SCN_EXECUTE | pe::SCN_READ,
    );
    b.entry(text_rva);
    b.build()
}

/// The import address table slots the program calls through.
struct Imports {
    exit: u32,
    load: u32,
    find: u32,
    free: u32,
    message_box: u32,
    initialize: u32,
    uninitialize: u32,
}

/// The program's code: the loop around Regsvr32Main at `main`, with
/// its data at `data`.
fn code(data: u32, main: u32, imp: Imports) -> Vec<u8> {
    let mut code: Vec<u8> = Vec::new();
    let op = |code: &mut Vec<u8>, bytes: &[u8], value: Option<u32>| {
        code.extend_from_slice(bytes);
        if let Some(v) = value {
            code.extend_from_slice(&v.to_le_bytes());
        }
    };
    // The step's result goes to the next call; the first has 0.
    op(&mut code, &[0x31, 0xC0], None); // xor eax, eax
    let top = code.len();
    op(&mut code, &[0x50], None); // push eax
    op(&mut code, &[0x68], Some(data)); // push data
    op(&mut code, &[0xB8], Some(main)); // mov eax, Regsvr32Main
    op(&mut code, &[0xFF, 0xD0], None); // call eax
    // A step for each value of eax, each ending with a jump to `top`.
    let mut steps: Vec<(Step, Vec<u8>)> = Vec::new();
    let mut s = Vec::new();
    op(&mut s, &[0xFF, 0x35], Some(data + D_EXIT)); // push dword [exit]
    op(&mut s, &[0xFF, 0x15], Some(imp.exit)); // call [ExitProcess]
    steps.push((Step::Exit, s));
    let mut s = Vec::new();
    op(&mut s, &[0x6A, 0], None); // push 0
    op(&mut s, &[0xFF, 0x15], Some(imp.initialize)); // call [OleInitialize]
    steps.push((Step::OleInitialize, s));
    let mut s = Vec::new();
    op(&mut s, &[0x68], Some(data + D_PATH)); // push path
    op(&mut s, &[0xFF, 0x15], Some(imp.load)); // call [LoadLibraryA]
    steps.push((Step::Load, s));
    let mut s = Vec::new();
    op(&mut s, &[0x68], Some(data + D_ENTRY)); // push entry
    op(&mut s, &[0xFF, 0x35], Some(data + D_MODULE)); // push dword [module]
    op(&mut s, &[0xFF, 0x15], Some(imp.find)); // call [GetProcAddress]
    steps.push((Step::Find, s));
    let mut s = Vec::new();
    op(&mut s, &[0x89, 0xE6], None); // mov esi, esp
    op(&mut s, &[0xFF, 0x35], Some(data + D_ARG1)); // push dword [arg1]
    op(&mut s, &[0xFF, 0x35], Some(data + D_ARG0)); // push dword [arg0]
    op(&mut s, &[0xFF, 0x15], Some(data + D_PROC)); // call [proc]
    op(&mut s, &[0x89, 0xF4], None); // mov esp, esi
    steps.push((Step::Call, s));
    let mut s = Vec::new();
    op(&mut s, &[0xFF, 0x35], Some(data + D_TYPE)); // push dword [type]
    op(&mut s, &[0x68], Some(data + D_CAPTION)); // push caption
    op(&mut s, &[0x68], Some(data + D_TEXT)); // push text
    op(&mut s, &[0x6A, 0], None); // push 0: no owner
    op(&mut s, &[0xFF, 0x15], Some(imp.message_box)); // call [MessageBoxA]
    steps.push((Step::Message, s));
    let mut s = Vec::new();
    op(&mut s, &[0xFF, 0x35], Some(data + D_MODULE)); // push dword [module]
    op(&mut s, &[0xFF, 0x15], Some(imp.free)); // call [FreeLibrary]
    steps.push((Step::FreeLibrary, s));
    let mut s = Vec::new();
    op(&mut s, &[0xFF, 0x15], Some(imp.uninitialize)); // call [OleUninitialize]
    steps.push((Step::OleUninitialize, s));
    // The dispatch: cmp eax, step; je its code (rel32), for each step.
    let dispatch_len = steps.len() * (5 + 6);
    let mut at = code.len() + dispatch_len;
    let mut bodies = Vec::new();
    for (step, body) in &steps {
        let here = code.len();
        op(&mut code, &[0x3D], Some(*step as u32)); // cmp eax, step
        let rel = at as i64 - (here + 5 + 6) as i64;
        op(&mut code, &[0x0F, 0x84], Some(rel as i32 as u32)); // je body
        at += body.len() + 5;
        bodies.push(body);
    }
    for body in bodies {
        code.extend_from_slice(body);
        // jmp top (rel32), with the step's result in eax.
        let rel = top as i64 - (code.len() + 5) as i64;
        op(&mut code, &[0xE9], Some(rel as i32 as u32));
    }
    code
}

/// What the command line asks for.
#[derive(Debug, Default, PartialEq, Eq)]
struct Command {
    /// `/u`: unregister.
    unregister: bool,
    /// `/s`: no message box.
    silent: bool,
    /// `/i`: call DllInstall, with the text after `/i:`.
    install: Option<String>,
    /// `/n`: don't call DllRegisterServer or DllUnregisterServer.
    no_register: bool,
    /// The DLL's name.
    dll: Option<String>,
    /// A switch Windows' REGSVR32 doesn't know.
    unknown: Option<String>,
}

/// The command the command line `line` gives, the program's name first.
fn parse(line: &str) -> Command {
    let mut command = Command::default();
    for word in super::regedit::words(line).into_iter().skip(1) {
        let Some(switch) = word.strip_prefix(['/', '-']) else {
            if command.dll.is_none() {
                command.dll = Some(word);
            }
            continue;
        };
        match switch.chars().next().map(|c| c.to_ascii_lowercase()) {
            Some('u') => command.unregister = true,
            Some('s') => command.silent = true,
            Some('n') => command.no_register = true,
            // /c, which wrote to the console on Windows NT, is taken.
            Some('c') => {}
            Some('i') => {
                let text = switch[1..].strip_prefix(':').unwrap_or("");
                command.install = Some(text.to_string());
            }
            _ => command.unknown = Some(word),
        }
    }
    command
}

/// A call of the DLL's to make.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Call {
    Register,
    Unregister,
    /// DllInstall(TRUE) or (FALSE).
    Install(bool),
}

impl Call {
    fn name(self) -> &'static str {
        match self {
            Call::Register => "DllRegisterServer",
            Call::Unregister => "DllUnregisterServer",
            Call::Install(_) => "DllInstall",
        }
    }

    /// Its number among the calls kept in the data.
    fn code(self) -> u32 {
        match self {
            Call::Register => 1,
            Call::Unregister => 2,
            Call::Install(true) => 3,
            Call::Install(false) => 4,
        }
    }

    fn from_code(code: u32) -> Option<Call> {
        match code {
            1 => Some(Call::Register),
            2 => Some(Call::Unregister),
            3 => Some(Call::Install(true)),
            4 => Some(Call::Install(false)),
            _ => None,
        }
    }
}

/// The calls `command` asks for, in their order: DllInstall after the
/// registration, and before the unregistration.
fn calls(command: &Command) -> Vec<Call> {
    let register =
        (!command.no_register || command.install.is_none()).then_some(if command.unregister {
            Call::Unregister
        } else {
            Call::Register
        });
    let install = command
        .install
        .is_some()
        .then_some(Call::Install(!command.unregister));
    if command.unregister {
        install.into_iter().chain(register).collect()
    } else {
        register.into_iter().chain(install).collect()
    }
}

/// The calls packed in a dword, a byte each, the next lowest.
fn pack(calls: &[Call]) -> u32 {
    calls
        .iter()
        .rev()
        .fold(0, |packed, call| packed << 8 | call.code())
}

/// The program's data, read and written field by field.
struct Data(u32);

impl Data {
    fn get(&self, cpu: &mut Cpu, offset: u32) -> u32 {
        cpu.bus.guest_read_32(self.0 + offset)
    }

    fn set(&self, cpu: &mut Cpu, offset: u32, value: u32) {
        cpu.bus.guest_write_32(self.0 + offset, value);
    }

    /// Put `text`, ended by a zero, at `offset`, cut to fit `size`.
    fn put(&self, cpu: &mut Cpu, offset: u32, size: u32, text: &str) {
        let mut bytes = crate::dosstr::to_bytes(text);
        bytes.truncate(size as usize - 1);
        bytes.push(0);
        cpu.bus.guest_write_bytes(self.0 + offset, &bytes);
    }

    /// The DLL's name as the command line gave it.
    fn path(&self, cpu: &mut Cpu) -> String {
        let bytes = cpu
            .bus
            .guest_read_asciiz(self.0 + D_PATH, PATH_SIZE as usize);
        crate::dosstr::from_bytes(&bytes)
    }
}

/// The program's command line.
fn command_line(cpu: &mut Cpu) -> String {
    match crate::win32::process(cpu).command_line {
        0 => String::new(),
        at => {
            let bytes = cpu.bus.guest_read_asciiz(at, 0x1_0000);
            crate::dosstr::from_bytes(&bytes)
        }
    }
}

/// Regsvr32Main(data, result): the next step of REGSVR32's work, given
/// the result of the step before (`Step`).
pub fn main(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (d, result) = (Data(args[0]), args[1]);
    let step = match d.get(cpu, D_STEP) {
        stage::START => start(cpu, &d),
        stage::INITIALIZED => {
            // OleInitialize's HRESULT: S_FALSE, already initialized,
            // is a success too.
            if result & 0x8000_0000 != 0 {
                let text = format!("OleInitialize failed (0x{:08x}).", result);
                fail(cpu, &d, FAIL_OLE, &text)
            } else {
                d.set(cpu, D_STEP, stage::LOADED);
                Step::Load
            }
        }
        stage::LOADED => {
            let path = d.path(cpu);
            if result == 0 {
                let error = crate::win32::process(cpu).last_error;
                let text = format!(
                    "LoadLibrary(\"{}\") failed.\nGetLastError returns 0x{:08x}.",
                    path, error
                );
                cpu.bus
                    .log_string(&format!("[W32] REGSVR32: {} doesn't load", path));
                fail(cpu, &d, FAIL_LOAD, &text)
            } else {
                d.set(cpu, D_MODULE, result);
                next_call(cpu, &d)
            }
        }
        stage::FOUND => {
            let call = current(cpu, &d);
            let path = d.path(cpu);
            if result == 0 {
                let name = call.name();
                let text = format!(
                    "{path} was loaded, but the {name} entry point was not found.\n\n\
                     {name} may not be exported, or a corrupt version of {path} may be in memory."
                );
                cpu.bus
                    .log_string(&format!("[W32] REGSVR32: {} has no {}", path, name));
                fail(cpu, &d, FAIL_ENTRY, &text)
            } else {
                d.set(cpu, D_PROC, result);
                let (arg0, arg1) = match call {
                    Call::Install(install) => (install as u32, d.0 + D_WIDE),
                    _ => (0, 0),
                };
                d.set(cpu, D_ARG0, arg0);
                d.set(cpu, D_ARG1, arg1);
                d.set(cpu, D_STEP, stage::CALLED);
                Step::Call
            }
        }
        stage::CALLED => {
            let call = current(cpu, &d);
            let path = d.path(cpu);
            cpu.bus.log_string(&format!(
                "[W32] REGSVR32: {} in {} = {:08X}",
                call.name(),
                path,
                result
            ));
            if result & 0x8000_0000 != 0 {
                let text = format!(
                    "{} in {} failed.\nReturn code was: 0x{:08x}",
                    call.name(),
                    path,
                    result
                );
                fail(cpu, &d, FAIL_REG, &text)
            } else {
                let rest = d.get(cpu, D_CALLS) >> 8;
                d.set(cpu, D_CALLS, rest);
                if rest != 0 {
                    next_call(cpu, &d)
                } else {
                    let text = format!("{} in {} succeeded.", call.name(), path);
                    report(cpu, &d, OK, MB_ICONINFORMATION, &text)
                }
            }
        }
        stage::SHOWN => after_message(cpu, &d),
        stage::FREED => {
            d.set(cpu, D_STEP, stage::DONE);
            Step::OleUninitialize
        }
        _ => Step::Exit,
    };
    Some(step as u32)
}

/// The first call: the command line read, and the first step.
fn start(cpu: &mut Cpu, d: &Data) -> Step {
    let line = command_line(cpu);
    cpu.bus
        .log_string(&format!("[W32] REGSVR32: {}", line.trim()));
    let command = parse(&line);
    if command.silent {
        d.set(cpu, D_TYPE, u32::MAX);
    }
    if let Some(switch) = &command.unknown {
        let text = format!("Unrecognized flag: {}", switch);
        return report(cpu, d, FAIL_ARGS, MB_ICONERROR, &text);
    }
    let Some(dll) = &command.dll else {
        return report(cpu, d, FAIL_ARGS, MB_ICONERROR, "No DLL name specified.");
    };
    d.put(cpu, D_PATH, PATH_SIZE, dll);
    let text = command.install.as_deref().unwrap_or("");
    let units: Vec<u8> = text
        .encode_utf16()
        .take(WIDE_SIZE as usize / 2 - 1)
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect();
    cpu.bus.guest_write_bytes(d.0 + D_WIDE, &units);
    d.set(cpu, D_CALLS, pack(&calls(&command)));
    d.set(cpu, D_STEP, stage::INITIALIZED);
    Step::OleInitialize
}

/// The call of the DLL's being made.
fn current(cpu: &mut Cpu, d: &Data) -> Call {
    Call::from_code(d.get(cpu, D_CALLS) & 0xFF).unwrap_or(Call::Register)
}

/// The next of the calls: its function looked for.
fn next_call(cpu: &mut Cpu, d: &Data) -> Step {
    let call = current(cpu, d);
    d.put(cpu, D_ENTRY, ENTRY_SIZE, call.name());
    d.set(cpu, D_STEP, stage::FOUND);
    Step::Find
}

/// A failure with the exit code `exit` and the message `text`.
fn fail(cpu: &mut Cpu, d: &Data, exit: u32, text: &str) -> Step {
    report(cpu, d, exit, MB_ICONERROR, text)
}

/// The end of the work: the exit code, then the message box unless `/s`
/// (a type of u32::MAX in the data), then the DLL freed and OLE ended.
fn report(cpu: &mut Cpu, d: &Data, exit: u32, kind: u32, text: &str) -> Step {
    d.set(cpu, D_EXIT, exit);
    d.set(cpu, D_STEP, stage::SHOWN);
    if d.get(cpu, D_TYPE) == u32::MAX {
        return after_message(cpu, d);
    }
    d.set(cpu, D_TYPE, kind);
    d.put(cpu, D_CAPTION, CAPTION_SIZE, CAPTION);
    d.put(cpu, D_TEXT, TEXT_SIZE, text);
    Step::Message
}

/// After the message box, or in its place: the DLL freed if it was
/// loaded, OLE ended if it was started, and the exit.
fn after_message(cpu: &mut Cpu, d: &Data) -> Step {
    if d.get(cpu, D_MODULE) != 0 {
        d.set(cpu, D_STEP, stage::FREED);
        return Step::FreeLibrary;
    }
    // OleInitialize came first of all, so it was called if the DLL's
    // name was read.
    if d.get(cpu, D_PATH) != 0 {
        d.set(cpu, D_STEP, stage::DONE);
        return Step::OleUninitialize;
    }
    d.set(cpu, D_STEP, stage::DONE);
    Step::Exit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_line_gives_the_switches_and_the_dll() {
        let command = parse("regsvr32.exe /s C:\\WINDOWS\\SYSTEM\\ir50_32.dll");
        assert!(command.silent && !command.unregister);
        assert_eq!(
            command.dll.as_deref(),
            Some("C:\\WINDOWS\\SYSTEM\\ir50_32.dll")
        );
        assert_eq!(calls(&command), [Call::Register]);

        let command = parse("\"C:\\WINDOWS\\SYSTEM\\REGSVR32.EXE\" -U \"C:\\A B\\X.DLL\"");
        assert_eq!(command.dll.as_deref(), Some("C:\\A B\\X.DLL"));
        assert_eq!(calls(&command), [Call::Unregister]);

        let command = parse("regsvr32 /i:per-user x.dll");
        assert_eq!(command.install.as_deref(), Some("per-user"));
        assert_eq!(calls(&command), [Call::Register, Call::Install(true)]);
        let command = parse("regsvr32 /u /i x.dll");
        assert_eq!(calls(&command), [Call::Install(false), Call::Unregister]);
        let command = parse("regsvr32 /n /i x.dll");
        assert_eq!(calls(&command), [Call::Install(true)]);
        // /n alone still registers, as Windows' REGSVR32 does.
        assert_eq!(calls(&parse("regsvr32 /n x.dll")), [Call::Register]);

        assert_eq!(parse("regsvr32 /x x.dll").unknown.as_deref(), Some("/x"));
        assert!(parse("regsvr32 /s").dll.is_none());
    }

    #[test]
    fn the_calls_pack_in_their_order() {
        let calls = [Call::Install(false), Call::Unregister];
        let packed = pack(&calls);
        assert_eq!(Call::from_code(packed & 0xFF), Some(Call::Install(false)));
        assert_eq!(Call::from_code(packed >> 8 & 0xFF), Some(Call::Unregister));
        assert_eq!(packed >> 16, 0);
    }
}
