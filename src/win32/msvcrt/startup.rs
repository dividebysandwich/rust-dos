//! The program's start and end, and the rest of the runtime around it:
//! the arguments and environment, `_initterm`, the `atexit` functions
//! and `exit`, structured exception handling for `__try` (`_except_
//! handler3` and the unwinds), C++ exceptions, setjmp and longjmp,
//! signals, threads, the heap functions and the locale.

use super::*;
use crate::win32::stubs::stub_address;
use crate::win32::{object, seh};

const EOF: u32 = u32::MAX;

/// More places in the `state` variable (see `S_APP_TYPE`): the new
/// handler, the locale (0 "C", 1 the system's), how many `__finally`
/// blocks an unwind is running, and the locale's texts and `lconv`.
const S_NEW_HANDLER: u32 = 0x34;
const S_LOCALE: u32 = 0x38;
const S_ABNORMAL: u32 = 0x3C;
const S_LOCALE_C: u32 = 0x100;
const S_LOCALE_NATIVE: u32 = 0x108;
const S_LCONV: u32 = 0x130;
const S_LCONV_TEXT: u32 = 0x160;

const NATIVE_LOCALE: &[u8] = b"English_United States.1252";

// Arguments and the environment.

/// The arguments of the command line `line`, as MSVCRT splits it: the
/// program's name up to its closing quote or a blank; the others at
/// blanks outside quotes, with 2n backslashes and a quote n backslashes
/// and a quote that opens or closes, 2n+1 and a quote n and a quote, and
/// two quotes within quotes one.
pub fn split_command_line(line: &[u8]) -> Vec<Vec<u8>> {
    let mut args = Vec::new();
    let mut i = 0;
    let blank = |b: u8| b == b' ' || b == b'\t';
    // The program's name.
    let mut name = Vec::new();
    if line.first() == Some(&b'"') {
        i = 1;
        while i < line.len() && line[i] != b'"' {
            name.push(line[i]);
            i += 1;
        }
        if i < line.len() {
            i += 1;
        }
    } else {
        while i < line.len() && !blank(line[i]) {
            name.push(line[i]);
            i += 1;
        }
    }
    args.push(name);
    loop {
        while i < line.len() && blank(line[i]) {
            i += 1;
        }
        if i >= line.len() {
            break;
        }
        let mut arg = Vec::new();
        let mut quoted = false;
        while i < line.len() && (quoted || !blank(line[i])) {
            let mut slashes = 0;
            while i < line.len() && line[i] == b'\\' {
                slashes += 1;
                i += 1;
            }
            if i < line.len() && line[i] == b'"' {
                arg.extend(std::iter::repeat_n(b'\\', slashes / 2));
                if slashes % 2 == 1 {
                    arg.push(b'"');
                } else if quoted && line.get(i + 1) == Some(&b'"') {
                    arg.push(b'"');
                    i += 1;
                } else {
                    quoted = !quoted;
                }
                i += 1;
            } else {
                arg.extend(std::iter::repeat_n(b'\\', slashes));
                if i < line.len() && (quoted || !blank(line[i])) {
                    arg.push(line[i]);
                    i += 1;
                }
            }
        }
        args.push(arg);
    }
    args
}

/// The strings of the NULL-ended array at `at`, with their addresses.
fn string_array(cpu: &mut Cpu, at: u32) -> Option<Vec<(u32, Vec<u8>)>> {
    let mut out = Vec::new();
    if at == 0 {
        return Some(out);
    }
    for i in 0.. {
        let s = read32(cpu, at + 4 * i)?;
        if s == 0 {
            break;
        }
        out.push((s, cstr(cpu, s)?));
    }
    Some(out)
}

/// Where the value of `name` is in `_environ`, and the value.
fn find_variable(cpu: &mut Cpu, name: &[u8]) -> Option<Option<(u32, Vec<u8>)>> {
    let environ = read32(cpu, var_at(V_ENVIRON))?;
    for (at, s) in string_array(cpu, environ)? {
        if s.len() > name.len()
            && s[name.len()] == b'='
            && s[..name.len()].eq_ignore_ascii_case(name)
        {
            let value = s[name.len() + 1..].to_vec();
            return Some(Some((at + name.len() as u32 + 1, value)));
        }
    }
    Some(None)
}

/// The value of the variable `name` in the C runtime's environment.
pub fn getenv_value(cpu: &mut Cpu, name: &str) -> Option<String> {
    let (_, value) = find_variable(cpu, name.as_bytes())??;
    Some(crate::dosstr::from_bytes(&value))
}

/// getenv(name): the value in `_environ`'s string, or NULL.
pub fn getenv(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let name = cstr(cpu, args[0])?;
    Some(find_variable(cpu, &name)?.map_or(0, |(at, _)| at))
}

/// _putenv("NAME=value"): the variable set, or with no value removed,
/// in `_environ` and the process's environment. 0, or -1.
pub fn putenv(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let text = cstr(cpu, args[0])?;
    let Some(eq) = text.iter().position(|&b| b == b'=') else {
        return Some(EOF);
    };
    if eq == 0 {
        return Some(EOF);
    }
    let name = &text[..eq];
    let value = &text[eq + 1..];
    let environ = read32(cpu, var_at(V_ENVIRON))?;
    let mut strings: Vec<u32> = string_array(cpu, environ)?
        .into_iter()
        .filter(|(_, s)| {
            !(s.len() > name.len()
                && s[name.len()] == b'='
                && s[..name.len()].eq_ignore_ascii_case(name))
        })
        .map(|(at, _)| at)
        .collect();
    if !value.is_empty() {
        let Some(copy) = heap_alloc(cpu, text.len() as u32 + 1) else {
            return Some(EOF);
        };
        put_cstr(cpu, copy, &text)?;
        strings.push(copy);
    }
    let Some(array) = heap_alloc(cpu, 4 * (strings.len() as u32 + 1)) else {
        return Some(EOF);
    };
    strings.push(0);
    let words: Vec<u8> = strings.iter().flat_map(|w| w.to_le_bytes()).collect();
    put(cpu, array, &words)?;
    write32(cpu, var_at(V_ENVIRON), array)?;
    // The process's own, as SetEnvironmentVariable changes it.
    let name = crate::dosstr::from_bytes(name);
    let value = crate::dosstr::from_bytes(value);
    let environment = &mut super::super::process(cpu).environment;
    let prefix = format!("{}=", name.to_ascii_uppercase());
    environment.retain(|s| !s.to_ascii_uppercase().starts_with(&prefix));
    if !value.is_empty() {
        environment.push(format!("{}={}", name, value));
    }
    Some(0)
}

/// The wide copies of the arguments, environment, command line and
/// path, made the first time a wide function asks.
fn make_wide(cpu: &mut Cpu) -> Option<()> {
    if read32(cpu, var_at(V_WARGV))? != 0 {
        return Some(());
    }
    let wide_copy = |cpu: &mut Cpu, s: &[u8]| -> Option<u32> {
        let w: Vec<u16> = s.iter().map(|&b| widen(b)).collect();
        let at = heap_alloc(cpu, 2 * w.len() as u32 + 2)?;
        put_wstr(cpu, at, &w)?;
        Some(at)
    };
    let wide_array = |cpu: &mut Cpu, at: u32| -> Option<u32> {
        let strings = string_array(cpu, at)?;
        let array = heap_alloc(cpu, 4 * (strings.len() as u32 + 1))?;
        for (i, (_, s)) in strings.iter().enumerate() {
            let copy = wide_copy(cpu, s)?;
            write32(cpu, array + 4 * i as u32, copy)?;
        }
        write32(cpu, array + 4 * strings.len() as u32, 0)?;
        Some(array)
    };
    let argv = read32(cpu, var_at(V_ARGV))?;
    let wargv = wide_array(cpu, argv)?;
    let environ = read32(cpu, var_at(V_ENVIRON))?;
    let wenviron = wide_array(cpu, environ)?;
    let line = read32(cpu, var_at(V_ACMDLN))?;
    let line = cstr(cpu, line)?;
    let wline = wide_copy(cpu, &line)?;
    let path = read32(cpu, var_at(V_PGMPTR))?;
    let path = cstr(cpu, path)?;
    let wpath = wide_copy(cpu, &path)?;
    write32(cpu, var_at(V_WARGV), wargv)?;
    write32(cpu, var_at(V_WENVIRON), wenviron)?;
    write32(cpu, var_at(V_WINITENV), wenviron)?;
    write32(cpu, var_at(V_WCMDLN), wline)?;
    write32(cpu, var_at(V_WPGMPTR), wpath)
}

/// __getmainargs(&argc, &argv, &env, wildcards, &startinfo): 0.
pub fn getmainargs(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let argc = read32(cpu, var_at(V_ARGC))?;
    let argv = read32(cpu, var_at(V_ARGV))?;
    let environ = read32(cpu, var_at(V_ENVIRON))?;
    write32(cpu, args[0], argc)?;
    write32(cpu, args[1], argv)?;
    write32(cpu, args[2], environ)?;
    if args[4] != 0 {
        let mode = read32(cpu, args[4])?;
        set_state(cpu, S_NEW_MODE, mode);
    }
    Some(0)
}

/// __wgetmainargs: the same, wide.
pub fn wgetmainargs(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if make_wide(cpu).is_none() {
        if cpu.bus.guest_faulted() {
            return None;
        }
        return Some(EOF);
    }
    let argc = read32(cpu, var_at(V_ARGC))?;
    let argv = read32(cpu, var_at(V_WARGV))?;
    let environ = read32(cpu, var_at(V_WENVIRON))?;
    write32(cpu, args[0], argc)?;
    write32(cpu, args[1], argv)?;
    write32(cpu, args[2], environ)?;
    Some(0)
}

/// __set_app_type(type): 1 console, 2 GUI.
pub fn set_app_type(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    set_state(cpu, S_APP_TYPE, args[0]);
    Some(0)
}

/// The __p_ functions: the address of a variable.
macro_rules! pointers {
    ($($name:ident => $var:expr),* $(,)?) => {
        $(
            pub fn $name(_: &mut Cpu, _: &[u32]) -> Option<u32> {
                Some(var_at($var))
            }
        )*
    };
}

pointers! {
    p_argc => V_ARGC,
    p_argv => V_ARGV,
    p_environ => V_ENVIRON,
    p_initenv => V_INITENV,
    p_acmdln => V_ACMDLN,
    p_pgmptr => V_PGMPTR,
    p_fmode => V_FMODE,
    p_commode => V_COMMODE,
    p_pctype => V_PCTYPE,
    p_pwctype => V_PWCTYPE,
    p_mb_cur_max => V_MB_CUR_MAX,
    p_mbctype => V_MBCTYPE,
    p_osver => V_OSVER,
    p_winver => V_WINVER,
    p_winmajor => V_WINMAJOR,
    p_winminor => V_WINMINOR,
    p_timezone => V_TIMEZONE,
    p_daylight => V_DAYLIGHT,
    p_tzname => V_TZNAME,
    p_mbcasemap => V_MBCASEMAP,
}

/// The wide ones make the wide copies first.
macro_rules! wide_pointers {
    ($($name:ident => $var:expr),* $(,)?) => {
        $(
            pub fn $name(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
                let _ = make_wide(cpu);
                Some(var_at($var))
            }
        )*
    };
}

wide_pointers! {
    p_wargv => V_WARGV,
    p_wenviron => V_WENVIRON,
    p_winitenv => V_WINITENV,
    p_wcmdln => V_WCMDLN,
    p_wpgmptr => V_WPGMPTR,
}

/// _errno(), __doserrno() and __fpecode(): the thread's.
pub fn errno(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(thread_field(cpu, T_ERRNO).unwrap_or(0))
}

pub fn doserrno(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(thread_field(cpu, T_DOSERRNO).unwrap_or(0))
}

pub fn fpecode(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(thread_field(cpu, T_FPECODE).unwrap_or(0))
}

// _initterm, atexit and exit.

/// _initterm(begin, end): each function in the table that isn't NULL.
/// The call's data: the next entry and the end.
pub fn initterm(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let resume = call::here(cpu);
    initterm_step(cpu, resume, args[0], args[1]);
    None
}

fn initterm_step(cpu: &mut Cpu, resume: (u32, u32), mut at: u32, end: u32) {
    while at < end {
        let f = cpu.bus.guest_read_32(at);
        if cpu.bus.guest_faulted() {
            break;
        }
        at += 4;
        if f != 0 && call_program(cpu, f, &[], C_INITTERM, resume, &[at, end]) {
            return;
        }
    }
    call::finish(cpu, resume, 0);
}

pub fn initterm_returned(cpu: &mut Cpu, _: u32, call: Call) {
    let &[at, end] = call.data.as_slice() else {
        return;
    };
    initterm_step(cpu, call.resume, at, end);
}

/// _onexit(f): `f` runs at exit, after those added after it. `f`, or
/// NULL when there is no room.
pub fn onexit(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let count = state(cpu, S_ATEXIT_COUNT);
    let room = state(cpu, S_ATEXIT_ROOM);
    let mut table = state(cpu, S_ATEXIT);
    if count == room {
        let room = (room * 2).max(32);
        let grown = if table == 0 {
            heap_alloc(cpu, 4 * room)
        } else {
            heap_realloc(cpu, table, 4 * room)
        };
        let Some(grown) = grown else {
            return Some(0);
        };
        table = grown;
        set_state(cpu, S_ATEXIT, table);
        set_state(cpu, S_ATEXIT_ROOM, room);
    }
    write32(cpu, table + 4 * count, args[0])?;
    set_state(cpu, S_ATEXIT_COUNT, count + 1);
    Some(args[0])
}

/// atexit(f): 0, or -1.
pub fn atexit(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(if onexit(cpu, args)? == 0 { EOF } else { 0 })
}

/// __dllonexit(f, &begin, &end): `f` added to a DLL's own table.
pub fn dllonexit(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[f, begin_at, end_at] = args else {
        unreachable!()
    };
    let begin = read32(cpu, begin_at)?;
    let end = read32(cpu, end_at)?;
    let count = if begin == 0 {
        0
    } else {
        end.saturating_sub(begin) / 4
    };
    let table = if begin == 0 {
        heap_alloc(cpu, 4 * (count + 1))
    } else {
        heap_realloc(cpu, begin, 4 * (count + 1))
    };
    let Some(table) = table else {
        return Some(0);
    };
    write32(cpu, table + 4 * count, f)?;
    write32(cpu, begin_at, table)?;
    write32(cpu, end_at, table + 4 * (count + 1))?;
    Some(f)
}

/// What happens when the atexit functions have run.
const AFTER_EXIT: u32 = 0;
const AFTER_CEXIT: u32 = 1;

/// Run the atexit functions, the last added first, then flush the
/// streams and end the process (`AFTER_EXIT`) or return (`AFTER_CEXIT`).
/// The call's data: what happens after, and the exit code.
fn run_atexit(cpu: &mut Cpu, resume: (u32, u32), after: u32, code: u32) {
    let count = state(cpu, S_ATEXIT_COUNT);
    if count > 0 {
        set_state(cpu, S_ATEXIT_COUNT, count - 1);
        let table = state(cpu, S_ATEXIT);
        let f = cpu.bus.guest_read_32(table + 4 * (count - 1));
        if f != 0 && call_program(cpu, f, &[], C_ATEXIT, resume, &[after, code]) {
            return;
        }
        run_atexit(cpu, resume, after, code);
        return;
    }
    let _ = super::stdio::flush_all(cpu);
    if after == AFTER_EXIT {
        end_process(cpu, code);
    } else {
        call::finish(cpu, resume, 0);
    }
}

pub fn atexit_returned(cpu: &mut Cpu, _: u32, call: Call) {
    let &[after, code] = call.data.as_slice() else {
        return;
    };
    run_atexit(cpu, call.resume, after, code);
}

fn end_process(cpu: &mut Cpu, code: u32) {
    let _ = kernel32(cpu, "ExitProcess", &[code]);
}

/// exit(code): the atexit functions, the streams flushed, the end.
pub fn exit(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if state(cpu, S_EXITING) != 0 {
        // exit from an atexit function: the end, at once.
        end_process(cpu, args[0]);
        return None;
    }
    set_state(cpu, S_EXITING, 1);
    let resume = call::here(cpu);
    run_atexit(cpu, resume, AFTER_EXIT, args[0]);
    None
}

/// _cexit(): exit's work, then back to the caller.
pub fn cexit(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    set_state(cpu, S_EXITING, 1);
    let resume = call::here(cpu);
    run_atexit(cpu, resume, AFTER_CEXIT, 0);
    None
}

/// _c_exit(): nothing to do, as the streams are left as they are.
pub fn c_exit(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// _exit(code): the end, without the atexit functions or flushing.
pub fn exit_now(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    end_process(cpu, args[0]);
    None
}

/// _amsg_exit(number): "runtime error R60nn" and the end, with 255.
pub fn amsg_exit(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let text = format!("\r\nruntime error R60{:02}\r\n", args[0] % 100);
    let _ = super::stdio::flush_all(cpu);
    super::io::write_fd(cpu, 2, text.as_bytes());
    end_process(cpu, 255);
    None
}

/// abort(): SIGABRT's handler if there is one, then "abnormal program
/// termination" and the end, with 3.
pub fn abort(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let handler = state(cpu, S_SIGNALS + 4 * SIGABRT);
    if handler > SIG_IGN {
        set_state(cpu, S_SIGNALS + 4 * SIGABRT, SIG_DFL);
        let resume = call::here(cpu);
        if call_program(cpu, handler, &[SIGABRT], C_SIGNAL, resume, &[0, 1]) {
            return None;
        }
    }
    abnormal_end(cpu);
    None
}

fn abnormal_end(cpu: &mut Cpu) {
    super::io::write_fd(cpu, 2, b"\r\nabnormal program termination\r\n");
    end_process(cpu, 3);
}

/// _purecall(): a pure virtual function was called.
pub fn purecall(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    amsg_exit(cpu, &[25])
}

/// __setusermatherr(f): the program's _matherr, which nothing here calls.
pub fn setusermatherr(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    set_state(cpu, S_MATHERR, args[0]);
    Some(0)
}

/// _set_error_mode(mode): the mode before; 3 asks.
pub fn set_error_mode(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let old = state(cpu, S_ERROR_MODE);
    match args[0] {
        0..=2 => set_state(cpu, S_ERROR_MODE, args[0]),
        3 => {}
        _ => return Some(EOF),
    }
    Some(old)
}

// Signals.

const SIGINT: u32 = 2;
const SIGILL: u32 = 4;
const SIGFPE: u32 = 8;
const SIGSEGV: u32 = 11;
const SIGTERM: u32 = 15;
const SIGBREAK: u32 = 21;
const SIGABRT: u32 = 22;
const SIG_DFL: u32 = 0;
const SIG_IGN: u32 = 1;
/// SIGFPE's code for raise.
const FPE_EXPLICITGEN: u32 = 0x8C;

fn valid_signal(sig: u32) -> bool {
    matches!(
        sig,
        SIGINT | SIGILL | SIGFPE | SIGSEGV | SIGTERM | SIGBREAK | SIGABRT
    )
}

/// signal(sig, handler): the handler before, or SIG_ERR (-1).
pub fn signal(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (sig, handler) = (args[0], args[1]);
    if !valid_signal(sig) || handler == 2 || handler == 3 || handler == 4 {
        set_errno(cpu, EINVAL);
        return Some(EOF);
    }
    let old = state(cpu, S_SIGNALS + 4 * sig);
    set_state(cpu, S_SIGNALS + 4 * sig, handler);
    Some(old)
}

/// raise(sig): its handler called, made SIG_DFL first; with SIG_DFL the
/// end, with 3. 0, or -1 for a signal that isn't one.
pub fn raise(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let sig = args[0];
    if !valid_signal(sig) {
        return Some(EOF);
    }
    let handler = state(cpu, S_SIGNALS + 4 * sig);
    match handler {
        SIG_IGN => Some(0),
        SIG_DFL => {
            if sig == SIGABRT {
                abnormal_end(cpu);
            } else {
                end_process(cpu, 3);
            }
            None
        }
        _ => {
            set_state(cpu, S_SIGNALS + 4 * sig, SIG_DFL);
            let resume = call::here(cpu);
            let call_args: &[u32] = if sig == SIGFPE {
                &[sig, FPE_EXPLICITGEN]
            } else {
                &[sig]
            };
            if call_program(cpu, handler, call_args, C_SIGNAL, resume, &[0, 0]) {
                None
            } else {
                Some(EOF)
            }
        }
    }
}

/// A signal handler returned: the value to return, and whether the
/// process ends now (abort's).
pub fn signal_returned(cpu: &mut Cpu, _: u32, call: Call) {
    let &[value, end] = call.data.as_slice() else {
        return;
    };
    if end != 0 {
        abnormal_end(cpu);
        return;
    }
    call::finish(cpu, call.resume, value);
}

/// The signal an exception is, for _XcptFilter.
fn signal_of(code: u32) -> Option<u32> {
    match code {
        0xC000_0005 => Some(SIGSEGV),
        0xC000_001D | 0xC000_0096 => Some(SIGILL),
        0xC000_008D..=0xC000_0093 | 0xC000_02B4 | 0xC000_02B5 => Some(SIGFPE),
        _ => None,
    }
}

/// _XcptFilter(code, pointers): the filter around main. An exception
/// with a signal handler calls it and goes on (-1); SIG_IGN goes on; the
/// rest go to UnhandledExceptionFilter.
pub fn xcpt_filter(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (code, pointers) = (args[0], args[1]);
    let handler = signal_of(code).map(|sig| (sig, state(cpu, S_SIGNALS + 4 * sig)));
    match handler {
        Some((_, SIG_IGN)) => Some(EOF),
        Some((sig, handler)) if handler != SIG_DFL => {
            set_state(cpu, S_SIGNALS + 4 * sig, SIG_DFL);
            let resume = call::here(cpu);
            let call_args: &[u32] = if sig == SIGFPE {
                let fpe = match code {
                    0xC000_008D => 0x82,
                    0xC000_008E => 0x83,
                    0xC000_008F => 0x86,
                    0xC000_0090 => 0x81,
                    0xC000_0091 => 0x84,
                    0xC000_0092 => 0x8A,
                    0xC000_0093 => 0x85,
                    _ => 0x8C,
                };
                if let Some(at) = thread_field(cpu, T_FPECODE) {
                    cpu.bus.guest_write_32(at, fpe);
                }
                &[sig, fpe]
            } else {
                &[sig]
            };
            if call_program(cpu, handler, call_args, C_SIGNAL, resume, &[EOF, 0]) {
                None
            } else {
                Some(0)
            }
        }
        _ => kernel32(cpu, "UnhandledExceptionFilter", &[pointers]),
    }
}

// Structured exception handling: _except_handler3.
//
// The frame a function with __try makes, at EBP - 10h, as `frame`:
//   frame - 8   the stack pointer of the function's body
//   frame - 4   EXCEPTION_POINTERS of the exception, for the filters
//   frame + 0   the next frame
//   frame + 4   the handler (_except_handler3)
//   frame + 8   the scope table: 12 bytes for each __try (the enclosing
//               level, the filter or 0 for __finally, the handler)
//   frame + 12  the level the function is in, -1 outside every __try

const UNWINDING: u32 = 0x06;
const TRYLEVEL_NONE: u32 = u32::MAX;
const CONTINUE_SEARCH: u32 = 1;
const CONTINUE_EXECUTION: u32 = 0;

/// What a local unwind does when its `__finally` blocks have run.
const M_RETURN: u32 = 0;
const M_HANDLER: u32 = 1;
const M_LONGJMP: u32 = 2;
/// After RtlUnwind for _global_unwind2: return, with no local unwind.
const M_GLOBAL: u32 = 3;

/// The registers a handler keeps for its caller.
fn saved_registers(cpu: &Cpu) -> [u32; 4] {
    [cpu.ebx(), cpu.esi(), cpu.edi(), cpu.ebp()]
}

fn restore_registers(cpu: &mut Cpu, regs: &[u32]) {
    cpu.set_ebx(regs[0]);
    cpu.set_esi(regs[1]);
    cpu.set_edi(regs[2]);
    cpu.set_ebp(regs[3]);
}

/// _except_handler3(record, frame, context, dispatcher).
pub fn except_handler3(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[record, frame, ctx, _] = args else {
        unreachable!()
    };
    let flags = read32(cpu, record + 4)?;
    let level = read32(cpu, frame + 12)?;
    read32(cpu, frame + 8)?;
    let resume = call::here(cpu);
    let regs = saved_registers(cpu);
    if flags & UNWINDING != 0 {
        // Unwinding: the frame's __finally blocks, then on.
        let mut d = vec![M_RETURN, frame, TRYLEVEL_NONE, CONTINUE_SEARCH];
        d.extend(regs);
        local_unwind(cpu, resume, d);
        return None;
    }
    // The filters see the exception through frame - 4.
    let pointers = cpu.esp().wrapping_sub(8);
    if !cpu.bus.guest_probe(pointers.wrapping_sub(64), 72, true) {
        return None;
    }
    write32(cpu, pointers, record)?;
    write32(cpu, pointers + 4, ctx)?;
    write32(cpu, frame.wrapping_sub(4), pointers)?;
    cpu.set_esp(pointers);
    let mut d = vec![record, frame, ctx, level];
    d.extend(regs);
    search(cpu, resume, d);
    None
}

/// Call the filter of the level d[3], or of an enclosing one; with none
/// left, return ContinueSearch. `d`: the record, frame, context, level
/// and the caller's registers.
fn search(cpu: &mut Cpu, resume: (u32, u32), mut d: Vec<u32>) {
    let frame = d[1];
    let scope = cpu.bus.guest_read_32(frame + 8);
    while d[3] != TRYLEVEL_NONE {
        let entry = scope.wrapping_add(12 * d[3]);
        let filter = cpu.bus.guest_read_32(entry + 4);
        if cpu.bus.guest_faulted() {
            break;
        }
        if filter != 0 {
            cpu.set_ebp(frame + 0x10);
            if call_program(cpu, filter, &[], C_FILTER, resume, &d) {
                return;
            }
            break;
        }
        d[3] = cpu.bus.guest_read_32(entry);
    }
    restore_registers(cpu, &d[4..8]);
    call::finish(cpu, resume, CONTINUE_SEARCH);
}

/// A filter returned: its handler runs (1), the program goes on where the
/// exception was (-1), or the search goes on (0).
pub fn filter_returned(cpu: &mut Cpu, value: u32, call: Call) {
    let mut d = call.data;
    if d.len() != 8 {
        return;
    }
    let (record, frame, level) = (d[0], d[1], d[3]);
    let _ = record;
    match value as i32 {
        v if v > 0 => {
            // Unwind the frames above this one, then this one's levels
            // within the __try, then the handler.
            let mut u = vec![M_HANDLER, frame, level, 0];
            u.extend_from_slice(&d[4..8]);
            if !global_unwind(cpu, frame, call.resume, &u) {
                restore_registers(cpu, &d[4..8]);
                call::finish(cpu, call.resume, CONTINUE_SEARCH);
            }
        }
        v if v < 0 => {
            restore_registers(cpu, &d[4..8]);
            call::finish(cpu, call.resume, CONTINUE_EXECUTION);
        }
        _ => {
            let scope = cpu.bus.guest_read_32(frame + 8);
            d[3] = cpu.bus.guest_read_32(scope.wrapping_add(12 * level));
            search(cpu, call.resume, d);
        }
    }
}

/// The address of KERNEL32's RtlUnwind.
fn rtl_unwind_address() -> u32 {
    let index = super::super::kernel32::DLL
        .apis
        .iter()
        .position(|api| api.name == "RtlUnwind")
        .expect("KERNEL32 has RtlUnwind");
    super::super::stubs::stub_address(0, index)
}

/// RtlUnwind(frame) called, the frames above `frame` unwound; then
/// `rtl_unwound` with `d`.
fn global_unwind(cpu: &mut Cpu, frame: u32, resume: (u32, u32), d: &[u32]) -> bool {
    call_program(
        cpu,
        rtl_unwind_address(),
        &[frame, 0, 0, 0],
        C_UNWOUND,
        resume,
        d,
    )
}

pub fn rtl_unwound(cpu: &mut Cpu, _: u32, call: Call) {
    let d = call.data;
    if d.first() == Some(&M_GLOBAL) {
        restore_registers(cpu, &d[4..8]);
        call::finish(cpu, call.resume, 0);
        return;
    }
    local_unwind(cpu, call.resume, d);
}

/// Run the `__finally` blocks of the frame d[1] from its level down to
/// the level d[2], then do what d[0] says (`M_RETURN` and the others).
/// `d`: the mode, the frame, the level, a value and four more words.
fn local_unwind(cpu: &mut Cpu, resume: (u32, u32), d: Vec<u32>) {
    let frame = d[1];
    let stop = d[2];
    loop {
        let level = cpu.bus.guest_read_32(frame + 12);
        let scope = cpu.bus.guest_read_32(frame + 8);
        if cpu.bus.guest_faulted() || level == stop || level == TRYLEVEL_NONE {
            break;
        }
        let entry = scope.wrapping_add(12 * level);
        let prev = cpu.bus.guest_read_32(entry);
        let filter = cpu.bus.guest_read_32(entry + 4);
        let handler = cpu.bus.guest_read_32(entry + 8);
        if cpu.bus.guest_faulted() {
            break;
        }
        cpu.bus.guest_write_32(frame + 12, prev);
        if filter == 0 {
            cpu.set_ebp(frame + 0x10);
            let depth = state(cpu, S_ABNORMAL);
            set_state(cpu, S_ABNORMAL, depth + 1);
            if call_program(cpu, handler, &[], C_FINALLY, resume, &d) {
                return;
            }
            set_state(cpu, S_ABNORMAL, depth);
        }
    }
    unwound(cpu, resume, &d);
}

pub fn finally_returned(cpu: &mut Cpu, _: u32, call: Call) {
    let depth = state(cpu, S_ABNORMAL);
    set_state(cpu, S_ABNORMAL, depth.saturating_sub(1));
    local_unwind(cpu, call.resume, call.data);
}

/// The local unwind is done.
fn unwound(cpu: &mut Cpu, resume: (u32, u32), d: &[u32]) {
    let frame = d[1];
    match d[0] {
        M_HANDLER => {
            // The handler of level d[2], in the function's frame, with
            // the level its enclosing one.
            let scope = cpu.bus.guest_read_32(frame + 8);
            let entry = scope.wrapping_add(12 * d[2]);
            let prev = cpu.bus.guest_read_32(entry);
            let handler = cpu.bus.guest_read_32(entry + 8);
            let esp = cpu.bus.guest_read_32(frame.wrapping_sub(8));
            cpu.bus.guest_write_32(frame + 12, prev);
            restore_registers(cpu, &d[4..8]);
            cpu.set_ebp(frame + 0x10);
            cpu.set_esp(esp);
            call::unwound_to(cpu, esp);
            cpu.set_eip(handler);
        }
        M_LONGJMP => long_jump(cpu, d[3], d[4]),
        _ => {
            restore_registers(cpu, &d[4..8]);
            call::finish(cpu, resume, d[3]);
        }
    }
}

/// _local_unwind2(frame, level): the frame's __finally blocks down to
/// `level`, for a return or goto out of a __try.
pub fn local_unwind2(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    read32(cpu, args[0] + 12)?;
    let resume = call::here(cpu);
    let mut d = vec![M_RETURN, args[0], args[1], 0];
    d.extend(saved_registers(cpu));
    local_unwind(cpu, resume, d);
    None
}

/// _global_unwind2(frame): RtlUnwind to `frame`.
pub fn global_unwind2(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let resume = call::here(cpu);
    let mut d = vec![M_GLOBAL, args[0], 0, 0];
    d.extend(saved_registers(cpu));
    if global_unwind(cpu, args[0], resume, &d) {
        None
    } else {
        Some(0)
    }
}

/// _abnormal_termination(): whether the __finally block running runs for
/// an unwind.
pub fn abnormal_termination(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some((state(cpu, S_ABNORMAL) != 0) as u32)
}

// setjmp and longjmp, on MSVC's _JUMP_BUFFER: EBP, EBX, EDI, ESI, ESP,
// EIP, the exception frame, its level, a cookie, the C++ unwind function
// and its data.

const JMP_REGISTRATION: u32 = 24;
const JMP_TRYLEVEL: u32 = 28;
const JMP_COOKIE: u32 = 32;
const JMP_UNWIND: u32 = 36;
const JMP_MAGIC: u32 = 0x5643_3230;

/// The running thread's first exception frame.
fn exception_list(cpu: &mut Cpu) -> u32 {
    let tib = super::super::process(cpu)
        .current_thread()
        .map_or(0, |t| t.tib);
    cpu.bus.guest_read_32(tib)
}

/// Save the registers as they will be when the call returns.
fn save_jump(cpu: &mut Cpu, buffer: u32) -> Option<()> {
    let esp = cpu.esp();
    let eip = read32(cpu, esp)?;
    let registration = exception_list(cpu);
    let words = [
        cpu.ebp(),
        cpu.ebx(),
        cpu.edi(),
        cpu.esi(),
        esp + 4,
        eip,
        registration,
    ];
    let raw: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    put(cpu, buffer, &raw)
}

/// _setjmp(buffer): 0.
pub fn setjmp(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    save_jump(cpu, args[0])?;
    let registration = read32(cpu, args[0] + JMP_REGISTRATION)?;
    let level = if registration == u32::MAX || registration == 0 {
        TRYLEVEL_NONE
    } else {
        read32(cpu, registration + 12)?
    };
    write32(cpu, args[0] + JMP_TRYLEVEL, level)?;
    Some(0)
}

/// _setjmp3(buffer, count, unwind function, level): 0.
pub fn setjmp3(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let buffer = args[0];
    save_jump(cpu, buffer)?;
    write32(cpu, buffer + JMP_COOKIE, JMP_MAGIC)?;
    write32(cpu, buffer + JMP_UNWIND, 0)?;
    let rest = varargs(cpu, 2);
    if args[1] == 0 {
        write32(cpu, buffer + JMP_TRYLEVEL, TRYLEVEL_NONE)?;
        return Some(0);
    }
    let unwind = read32(cpu, rest)?;
    write32(cpu, buffer + JMP_UNWIND, unwind)?;
    let level = if args[1] > 1 {
        read32(cpu, rest + 4)?
    } else {
        let registration = read32(cpu, buffer + JMP_REGISTRATION)?;
        read32(cpu, registration + 12)?
    };
    write32(cpu, buffer + JMP_TRYLEVEL, level)?;
    Some(0)
}

/// longjmp(buffer, value): the frames since setjmp unwound, then back to
/// setjmp's return, which returns `value` (1 for 0).
pub fn longjmp(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let buffer = args[0];
    let value = if args[1] == 0 { 1 } else { args[1] };
    let registration = read32(cpu, buffer + JMP_REGISTRATION)?;
    let level = read32(cpu, buffer + JMP_TRYLEVEL)?;
    read32(cpu, buffer + JMP_UNWIND)?;
    let resume = call::here(cpu);
    if registration == 0 || registration == u32::MAX {
        long_jump(cpu, buffer, value);
        return None;
    }
    let d = vec![M_LONGJMP, registration, level, buffer, value, 0, 0, 0];
    if registration != exception_list(cpu) {
        if !global_unwind(cpu, registration, resume, &d) {
            long_jump(cpu, buffer, value);
        }
        return None;
    }
    local_unwind(cpu, resume, d);
    None
}

/// Back to setjmp's return with the registers it saved.
fn long_jump(cpu: &mut Cpu, buffer: u32, value: u32) {
    let mut words = [0u32; 6];
    for (i, word) in words.iter_mut().enumerate() {
        *word = cpu.bus.guest_read_32(buffer + 4 * i as u32);
    }
    if cpu.bus.guest_faulted() {
        return;
    }
    let [ebp, ebx, edi, esi, esp, eip] = words;
    cpu.set_ebp(ebp);
    cpu.set_ebx(ebx);
    cpu.set_edi(edi);
    cpu.set_esi(esi);
    cpu.set_esp(esp);
    call::unwound_to(cpu, esp);
    cpu.set_eip(eip);
    cpu.set_eax(value);
}

// C++ exceptions, as MSVC lays them out for x86.
//
// A function with try blocks or objects to destroy makes, at EBP - 0Ch,
// as `frame`:
//   frame - 4   the stack pointer of the function's body
//   frame + 0   the next frame
//   frame + 4   its handler: a thunk that loads the function's FuncInfo
//               into EAX and jumps to __CxxFrameHandler
//   frame + 8   its state: which of its objects live, which try it is in
// FuncInfo: a magic number, the count of states, the unwind map (8 bytes
// a state: the state before it and the code that destroys its object),
// the count of try blocks and their map (20 bytes each: the lowest and
// highest state of the try, the highest of its catches, the count of
// catches and where their handlers are). A handler (16 bytes): its
// adjectives, the type it catches (none for "..."), where its object
// goes from EBP, and its code, which returns where the function goes on.
// The ThrowInfo of a throw: its attributes, the object's destructor,
// and the types it can be caught as (28 bytes each: properties, type,
// how to adjust the pointer, size and copy constructor).

const CXX_EXCEPTION: u32 = 0xE06D_7363;
const CXX_MAGIC: u32 = 0x1993_0520;
/// FuncInfo's magic numbers: MSVC 6's and the two later ones, which add
/// fields after those read here. MSVCRT.DLL of Windows 98 knows only the
/// first and calls terminate() for the others.
const CXX_MAGICS: std::ops::RangeInclusive<u32> = CXX_MAGIC..=0x1993_0522;

/// A handler's adjectives, a catchable type's properties and a throw's
/// attributes.
const HT_CONST: u32 = 0x01;
const HT_VOLATILE: u32 = 0x02;
const HT_REFERENCE: u32 = 0x08;
const CT_SIMPLE: u32 = 0x01;
const CT_BY_REFERENCE: u32 = 0x02;
const CT_VIRTUAL_BASE: u32 = 0x04;
const TI_CONST: u32 = 0x01;
const TI_VOLATILE: u32 = 0x02;

/// At most this many entries of a table are read.
const MAX_ENTRIES: u32 = 0x1000;

/// What `cxx_returned` does next, by the first word of its data.
/// X_RETURN: an unwind's objects destroyed, then ContinueSearch.
const X_RETURN: u32 = 0;
/// The catch's object copied: RtlUnwind.
const X_COPIED: u32 = 1;
/// The frames above unwound: this one's objects in the try destroyed.
const X_GLOBAL: u32 = 2;
/// An object of the frame destroyed: the next, then the catch block.
const X_LOCAL: u32 = 3;
/// The catch block returned: the exception's object destroyed.
const X_CAUGHT: u32 = 4;
/// The object destroyed: on after the try.
const X_DESTROYED: u32 = 5;
/// The thread's translator returned: look for a catch (...).
const X_TRANSLATED: u32 = 6;
/// The exception the translator threw, which nothing here catches, is
/// destroyed: the translator's frames unwind.
const X_DROPPED: u32 = 7;
/// The translator's frames unwound: on with the search for the fault.
const X_LEFT_TRANSLATOR: u32 = 8;
/// A `throw;` left the catch block of the frame `d[D_CATCH_GUARD]`: the
/// search that found that catch goes on.
const X_RETHROWN: u32 = 9;
/// The object of a catch block an exception left is destroyed.
const X_ABANDONED: u32 = 10;
/// The terminate handler returned: abort().
const X_TERMINATED: u32 = 11;
/// The unexpected handler returned: terminate().
const X_UNEXPECTED: u32 = 12;

/// The places in `cxx_returned`'s data: the step, the frame, its
/// FuncInfo, the state to unwind to, the try block and the handler that
/// catches, the exception's object, ThrowInfo and code, where the
/// function goes on, the caller's registers, the thread's exception
/// from before the catch, and the function's saved ESP from before it
/// (a try in the catch block saves the block's own there).
const D_FRAME: usize = 1;
const D_INFO: usize = 2;
const D_TARGET: usize = 3;
const D_TRY: usize = 4;
const D_HANDLER: usize = 5;
const D_OBJECT: usize = 6;
const D_THROW: usize = 7;
const D_CODE: usize = 8;
const D_GO_ON: usize = 9;
const D_REGS: usize = 10;
const D_OUTER: usize = 14;
const D_ESP: usize = 16;
/// The search: the record the handler was called with, the record and
/// context it is for, which differ after a `throw;`, the catch depth and
/// the frame's state, and the try blocks left, as MSVCRT's FindHandler
/// has them.
const D_RAW: usize = 17;
const D_RECORD: usize = 18;
const D_CONTEXT: usize = 19;
const D_DEPTH: usize = 20;
const D_STATE: usize = 21;
const D_NEXT: usize = 22;
const D_END: usize = 23;
/// The frame RtlUnwind stops at if not the function's: the guard of the
/// catch block the search started in.
const D_MARKER: usize = 24;
/// The guard frame of the translator whose exception this is, if any.
const D_TRANSLATOR: usize = 25;
/// The guard frame of the catch block this search runs for, if any.
const D_CATCH_GUARD: usize = 26;
/// Where the handler returns to, for a search that survives its stub.
const D_RESUME: usize = 27;
const D_LEN: usize = 29;

/// The guard frame around a call of the translator, on the stack: the
/// next frame and the handler, whether RtlUnwind passed the guard, then
/// the search that called the translator.
const G_NEXT: u32 = 0;
const G_UNWOUND: u32 = 8;
const G_SEARCH: u32 = 12;
const GUARD_SIZE: u32 = G_SEARCH + 4 * D_LEN as u32;
/// EXCEPTION_POINTERS, which go below the guard.
const POINTERS_SIZE: u32 = 8;
/// What a call of RtlUnwind pushes: four arguments and the return address.
const RTL_UNWIND_CALL: u32 = 20;

/// The guard frame around a catch block, on the stack: the next frame and
/// the handler, then the search that found the catch. MSVCRT has two
/// frames here, `_CallCatchBlock`'s __try and its guard, and this one does
/// the work of both, the guard's first.
const K_SEARCH: u32 = 8;
const CATCH_GUARD_SIZE: u32 = K_SEARCH + 4 * D_LEN as u32;

/// Write the search `d` at `at`.
fn store_search(cpu: &mut Cpu, at: u32, d: &[u32]) -> Option<()> {
    for (i, &word) in d.iter().enumerate() {
        write32(cpu, at + 4 * i as u32, word)?;
    }
    Some(())
}

/// The search written at `at`.
fn load_search(cpu: &mut Cpu, at: u32) -> Vec<u32> {
    (0..D_LEN as u32)
        .map(|i| cpu.bus.guest_read_32(at + 4 * i))
        .collect()
}

/// __CxxFrameHandler(record, frame, context, dispatcher), with the
/// function's FuncInfo in EAX: a catch of the function's that takes the
/// exception runs, after the frames above are unwound, and the function
/// goes on after it; otherwise ContinueSearch. Unwinding, the frame's
/// objects are destroyed. A catch (...) takes exceptions other than C++
/// ones too, as with MSVC 6. For one of those, in a function with a try
/// block, the thread's translator runs first, whatever state the function
/// is in: once for this frame, and once for the guard of each of its
/// catch blocks the exception is in.
pub fn cxx_frame_handler(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[record, frame, context, _] = args else {
        unreachable!()
    };
    let mut d = vec![0; D_LEN];
    d[D_FRAME] = frame;
    d[D_INFO] = cpu.eax();
    d[D_RAW] = record;
    d[D_RECORD] = record;
    d[D_CONTEXT] = context;
    frame_handler(cpu, d)
}

/// MSVCRT's __InternalCxxFrameHandler, for the frame, FuncInfo, record
/// and catch depth in `d`.
fn frame_handler(cpu: &mut Cpu, mut d: Vec<u32>) -> Option<u32> {
    let info = d[D_INFO];
    let record = d[D_RECORD];
    // A table that isn't one is terminate(), in an unwind too.
    let magic = read32(cpu, info)?;
    if !CXX_MAGICS.contains(&magic) {
        return terminate(cpu, &[]);
    }
    let code = read32(cpu, record)?;
    let flags = read32(cpu, record + 4)?;
    let resume = call::here(cpu);
    d[D_CODE] = code;
    d[D_RESUME] = resume.0;
    d[D_RESUME + 1] = resume.1;
    d[D_REGS..D_OUTER].copy_from_slice(&saved_registers(cpu));
    if flags & UNWINDING != 0 {
        d[0] = X_RETURN;
        d[D_TARGET] = TRYLEVEL_NONE;
        cxx_unwind(cpu, resume, d);
        return None;
    }
    // With no try block, nothing happens in this function.
    if read32(cpu, info + 12)? == 0 {
        return Some(CONTINUE_SEARCH);
    }
    // Built with /EHs, the function handles C++ exceptions only (FuncInfo's
    // EHFlags, in MSVC 8's and later): neither the translator nor its
    // catch (...) sees another.
    let foreign = thrown(cpu, record)?.is_none() && !rethrow(cpu, record)?;
    if foreign && magic >= 0x1993_0522 && read32(cpu, info + 32)? & 1 != 0 {
        return Some(CONTINUE_SEARCH);
    }
    find_handler(cpu, resume, d)
}

/// MSVCRT's FindHandler: on towards the catch that takes the exception,
/// or `end_search`. For a `throw;` the search is for the exception the
/// running catch block took, with its record and context.
fn find_handler(cpu: &mut Cpu, resume: (u32, u32), mut d: Vec<u32>) -> Option<u32> {
    let state = read32(cpu, d[D_FRAME] + 8)? as i32;
    let states = read32(cpu, d[D_INFO] + 4)? as i32;
    if state < -1 || state >= states {
        return terminate(cpu, &[]);
    }
    d[D_STATE] = state as u32;
    if rethrow(cpu, d[D_RECORD])? {
        let at = thread_field(cpu, T_CXX)?;
        let current = read32(cpu, at)?;
        if current == 0 {
            end_search(cpu, resume, d);
            return None;
        }
        d[D_RECORD] = current;
        d[D_CONTEXT] = read32(cpu, at + 4)?;
        if !cpu.bus.guest_probe(current, 1, false) || rethrow(cpu, current)? {
            return terminate(cpu, &[]);
        }
        d[D_CODE] = read32(cpu, current)?;
    }
    let found = thrown(cpu, d[D_RECORD])?;
    let Some(range) = try_range(cpu, d[D_INFO], d[D_DEPTH], state)? else {
        return terminate(cpu, &[]);
    };
    (d[D_NEXT], d[D_END]) = range;
    match found {
        Some((object, throw)) => {
            d[D_OBJECT] = object;
            d[D_THROW] = throw;
        }
        // In a translator's guard, an exception that isn't C++.
        None if d[D_TRANSLATOR] != 0 => return terminate(cpu, &[]),
        None => {
            let at = thread_field(cpu, T_SE_TRANSLATOR);
            let translator = at.map_or(Some(0), |at| read32(cpu, at))?;
            d[0] = X_TRANSLATED;
            if translator != 0 && translate(cpu, translator, resume, &d)? {
                return None;
            }
        }
    }
    if !search_tries(cpu, resume, d.clone())? {
        end_search(cpu, resume, d);
    }
    None
}

/// The search of the frame found no catch, or none that kept the
/// exception. In a translator's guard its exception is destroyed and the
/// translator is left. In a catch block's guard a `throw;` leaves the
/// block. Otherwise the handler returns ContinueSearch.
fn end_search(cpu: &mut Cpu, resume: (u32, u32), mut d: Vec<u32>) {
    if d[D_TRANSLATOR] != 0 {
        d[0] = X_DROPPED;
        let destructor = match d[D_THROW] {
            0 => 0,
            throw => cpu.bus.guest_read_32(throw + 4),
        };
        cpu.set_ecx(d[D_OBJECT]);
        if destructor == 0 || !call_program(cpu, destructor, &[], C_CXX, resume, &d) {
            unwind_translator(cpu, resume, d);
        }
        return;
    }
    if d[D_CATCH_GUARD] != 0 && rethrow(cpu, d[D_RAW]) == Some(true) {
        d[0] = X_RETHROWN;
        let args = [d[D_CATCH_GUARD], 0, 0, 0];
        if !call_program(cpu, rtl_unwind_address(), &args, C_CXX, resume, &d) {
            super::super::fatal(cpu, "a C++ exception, and its stack is gone", None);
        }
        return;
    }
    restore_registers(cpu, &d[D_REGS..D_OUTER]);
    call::finish(cpu, resume, CONTINUE_SEARCH);
}

/// Call the translator for the exception of the search `d`, which reach
/// it on the stack, inside a guard frame as MSVCRT's _CallSETranslator
/// makes: `translator_guard` deals with what the translator raises.
/// False, with nothing changed, if the stack isn't there.
fn translate(cpu: &mut Cpu, translator: u32, resume: (u32, u32), d: &[u32]) -> Option<bool> {
    let at = cpu.esp().wrapping_sub(POINTERS_SIZE + GUARD_SIZE);
    // Room for the call's arguments and return address too.
    let bottom = at.wrapping_sub(12);
    let size = POINTERS_SIZE + GUARD_SIZE + 12;
    if !cpu.bus.guest_probe(bottom, size as usize, true) {
        return Some(false);
    }
    let guard = at + POINTERS_SIZE;
    write32(cpu, at, d[D_RECORD])?;
    write32(cpu, at + 4, d[D_CONTEXT])?;
    let next = seh::handlers(cpu);
    write32(cpu, guard + G_NEXT, next)?;
    write32(cpu, guard + 4, own_handler("__TranslatorGuardHandler"))?;
    write32(cpu, guard + G_UNWOUND, 0)?;
    store_search(cpu, guard + G_SEARCH, d)?;
    cpu.set_esp(at);
    seh::set_handlers(cpu, guard);
    let args = [d[D_CODE], at];
    Some(call_program(cpu, translator, &args, C_CXX, resume, d))
}

/// Where the function of this DLL called `name` is.
fn own_handler(name: &str) -> u32 {
    let index = super::DLL
        .apis
        .iter()
        .position(|api| api.name == name)
        .expect("MSVCRT has its guard handlers");
    stub_address(dll_index(), index)
}

/// The handler of the frame around a translator, which the search for an
/// exception the translator raised gets to first. A C++ exception is
/// looked for in the function the translator runs for, at its catch
/// depth, and no further: a catch there takes it, and with none it is
/// destroyed, the translator's frames unwind and the search for the fault
/// ends in this frame, with no catch (...) of that depth taking the
/// fault. Anything else ends the program: terminate().
pub fn translator_guard(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[record, guard, context, _] = args else {
        unreachable!()
    };
    if read32(cpu, record + 4)? & UNWINDING != 0 {
        write32(cpu, guard + G_UNWOUND, 1)?;
        return Some(CONTINUE_SEARCH);
    }
    // RtlUnwind's call needs this stack, and the destructor's smaller one
    // part of it. A missing page runs this stub again from here; after the
    // search there is no stub left to run again.
    let bottom = cpu.esp().wrapping_sub(RTL_UNWIND_CALL);
    if !cpu.bus.guest_probe(bottom, RTL_UNWIND_CALL as usize, true) {
        return None;
    }
    let outer = load_search(cpu, guard + G_SEARCH);
    let mut d = vec![0; D_LEN];
    d[D_FRAME] = outer[D_FRAME];
    d[D_INFO] = outer[D_INFO];
    d[D_DEPTH] = outer[D_DEPTH];
    d[D_MARKER] = outer[D_MARKER];
    d[D_RAW] = record;
    d[D_RECORD] = record;
    d[D_CONTEXT] = context;
    d[D_TRANSLATOR] = guard;
    frame_handler(cpu, d)
}

/// RtlUnwind to the guard frame `d[D_TRANSLATOR]`, unless that was done,
/// then back to the search that called the translator. `translator_guard`
/// made sure of the stack.
fn unwind_translator(cpu: &mut Cpu, resume: (u32, u32), mut d: Vec<u32>) {
    let guard = d[D_TRANSLATOR];
    if cpu.bus.guest_read_32(guard + G_UNWOUND) != 0 {
        leave_translator(cpu, guard);
        return;
    }
    d[0] = X_LEFT_TRANSLATOR;
    let args = [guard, 0, 0, 0];
    if !call_program(cpu, rtl_unwind_address(), &args, C_CXX, resume, &d) {
        super::super::fatal(cpu, "a C++ exception, and its stack is gone", None);
    }
}

/// The translator's frames are gone: the frame it ran for has no catch
/// left for the exception it was called for.
fn leave_translator(cpu: &mut Cpu, guard: u32) {
    let next = cpu.bus.guest_read_32(guard + G_NEXT);
    let d = load_search(cpu, guard + G_SEARCH);
    let resume = (d[D_RESUME], d[D_RESUME + 1]);
    seh::set_handlers(cpu, next);
    call::unwound_to(cpu, resume.1);
    cpu.set_esp(resume.1);
    end_search(cpu, resume, d);
}

/// _set_se_translator(function): the thread's translator of exceptions
/// other than C++ ones; returns the one before it.
pub fn set_se_translator(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = thread_field(cpu, T_SE_TRANSLATOR)?;
    let old = read32(cpu, at)?;
    write32(cpu, at, args[0])?;
    Some(old)
}

/// set_terminate(function): the thread's terminate handler; returns the
/// one before it.
pub fn set_terminate(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = thread_field(cpu, T_TERMINATE)?;
    let old = read32(cpu, at)?;
    write32(cpu, at, args[0])?;
    Some(old)
}

/// set_unexpected(function): the thread's unexpected handler; returns the
/// one before it.
pub fn set_unexpected(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = thread_field(cpu, T_UNEXPECTED)?;
    let old = read32(cpu, at)?;
    write32(cpu, at, args[0])?;
    Some(old)
}

/// terminate(): the thread's terminate handler, inside a frame that makes
/// anything it raises the end of the program, then abort().
pub fn terminate(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let at = thread_field(cpu, T_TERMINATE)?;
    let handler = read32(cpu, at)?;
    if handler == 0 {
        return abort(cpu, &[]);
    }
    let at = cpu.esp().wrapping_sub(8);
    let next = seh::handlers(cpu);
    write32(cpu, at, next)?;
    write32(cpu, at + 4, own_handler("__TerminateGuardHandler"))?;
    cpu.set_esp(at);
    seh::set_handlers(cpu, at);
    let resume = call::here(cpu);
    let mut d = vec![0; D_LEN];
    d[0] = X_TERMINATED;
    if call_program(cpu, handler, &[], C_CXX, resume, &d) {
        return None;
    }
    abort(cpu, &[])
}

/// The handler of the frame `terminate` puts around its handler.
pub fn terminate_guard(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if read32(cpu, args[0] + 4)? & UNWINDING != 0 {
        return Some(CONTINUE_SEARCH);
    }
    abort(cpu, &[])
}

/// unexpected(): the thread's unexpected handler, if it has one, then
/// terminate().
pub fn unexpected(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let at = thread_field(cpu, T_UNEXPECTED)?;
    let handler = read32(cpu, at)?;
    let resume = call::here(cpu);
    let mut d = vec![0; D_LEN];
    d[0] = X_UNEXPECTED;
    if handler != 0 && call_program(cpu, handler, &[], C_CXX, resume, &d) {
        return None;
    }
    terminate(cpu, &[])
}

/// Where `cxx_unhandled_filter` is.
pub fn cxx_unhandled_filter_address() -> u32 {
    own_handler("__CxxUnhandledExceptionFilter")
}

/// The filter MSVCRT.DLL sets when it loads: terminate() for a C++
/// exception nothing took. For another it returns 0, and the process ends
/// as it does with no filter. (MSVCRT.DLL calls a filter set before it
/// loaded; here one set before stays in place of this one.)
pub fn cxx_unhandled_filter(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let record = read32(cpu, args[0])?;
    if read32(cpu, record)? == CXX_EXCEPTION
        && read32(cpu, record + 16)? == 3
        && read32(cpu, record + 20)? == CXX_MAGIC
    {
        return terminate(cpu, &[]);
    }
    Some(0)
}

/// Whether the record at `record` is a `throw;`: a C++ exception with no
/// ThrowInfo.
fn rethrow(cpu: &mut Cpu, record: u32) -> Option<bool> {
    Some(
        read32(cpu, record)? == CXX_EXCEPTION
            && read32(cpu, record + 16)? == 3
            && read32(cpu, record + 20)? == CXX_MAGIC
            && read32(cpu, record + 28)? == 0,
    )
}

/// The object and ThrowInfo of a C++ exception, None for another and for
/// a `throw;`. A record with code E06D7363h and another shape counts as
/// another.
fn thrown(cpu: &mut Cpu, record: u32) -> Option<Option<(u32, u32)>> {
    if read32(cpu, record)? != CXX_EXCEPTION || read32(cpu, record + 16)? != 3 {
        return Some(None);
    }
    if read32(cpu, record + 20)? != CXX_MAGIC {
        return Some(None);
    }
    let throw = read32(cpu, record + 28)?;
    Some((throw != 0).then_some((read32(cpu, record + 24)?, throw)))
}

/// The try blocks of the function, first and past the last, at catch
/// depth `depth` in `state`, as MSVCRT's GetRangeOfTrysToCheck counts
/// them. The map has inner try blocks first, so each try whose catch block
/// `state` is in divides it: the tries after the last of those are the
/// ones around all those catches (depth 0), those before it and after the
/// one before are in its catch block (depth 1), and so on. None if the
/// table has no such range, which is terminate().
fn try_range(cpu: &mut Cpu, info: u32, depth: u32, state: i32) -> Option<Option<(u32, u32)>> {
    let tries = read32(cpu, info + 12)?;
    let map = read32(cpu, info + 16)?;
    let (mut at, mut end, mut last) = (tries, tries, tries);
    let mut left = depth as i32;
    while left >= 0 {
        if at == u32::MAX {
            return Some(None);
        }
        at = at.wrapping_sub(1);
        let entry = map.wrapping_add(20u32.wrapping_mul(at));
        let divides = at == u32::MAX
            || (read32(cpu, entry + 4)? as i32) < state && state <= read32(cpu, entry + 8)? as i32;
        if divides {
            left -= 1;
            end = last;
            last = at;
        }
    }
    let start = at.wrapping_add(1);
    Some((end <= tries && start <= end).then_some((start, end)))
}

/// Go through the try blocks left in the search for one the frame's state
/// is in with a catch that takes the exception, and on towards it. False
/// if there is none.
fn search_tries(cpu: &mut Cpu, resume: (u32, u32), mut d: Vec<u32>) -> Option<bool> {
    let map = read32(cpu, d[D_INFO] + 16)?;
    let state = d[D_STATE] as i32;
    let throw = d[D_THROW];
    let mut types = Vec::new();
    if throw != 0 {
        let list = read32(cpu, throw + 12)?;
        let n = read32(cpu, list)?.min(MAX_ENTRIES);
        for i in 0..n {
            types.push(read32(cpu, list + 4 + 4 * i)?);
        }
    }
    for i in d[D_NEXT]..d[D_END].min(MAX_ENTRIES) {
        let entry = map.wrapping_add(20 * i);
        let low = read32(cpu, entry)? as i32;
        let high = read32(cpu, entry + 4)? as i32;
        if !(low..=high).contains(&state) {
            continue;
        }
        let catches = read32(cpu, entry + 12)?.min(MAX_ENTRIES);
        let handlers = read32(cpu, entry + 16)?;
        for c in 0..catches {
            let handler = handlers.wrapping_add(16 * c);
            // For an exception other than C++ only the try's last handler
            // counts, which is a catch (...) if the try has one.
            if throw == 0 {
                if c + 1 == catches && catches_all(cpu, handler)? {
                    d[D_NEXT] = i + 1;
                    return take_exception(cpu, resume, d, entry, handler, None);
                }
                continue;
            }
            for &catchable in &types {
                if type_matches(cpu, handler, catchable, throw)? {
                    d[D_NEXT] = i + 1;
                    let catchable = Some(catchable);
                    return take_exception(cpu, resume, d, entry, handler, catchable);
                }
            }
        }
    }
    Some(false)
}

/// Go on towards the catch at `handler` of the try at `entry`.
fn take_exception(
    cpu: &mut Cpu,
    resume: (u32, u32),
    mut d: Vec<u32>,
    entry: u32,
    handler: u32,
    catchable: Option<u32>,
) -> Option<bool> {
    d[D_TRY] = entry;
    d[D_HANDLER] = handler;
    d[D_TARGET] = read32(cpu, entry)?;
    if !catch_object(cpu, resume, &mut d, catchable)? {
        unwind_above(cpu, resume, d);
    }
    Some(true)
}

/// The name of the type whose descriptor is at `at`.
fn type_name(cpu: &mut Cpu, at: u32) -> Option<Vec<u8>> {
    cstr_n(cpu, at.wrapping_add(8), 0x400)
}

/// Whether the handler at `handler` is a catch (...).
fn catches_all(cpu: &mut Cpu, handler: u32) -> Option<bool> {
    let ty = read32(cpu, handler + 4)?;
    Some(ty == 0 || type_name(cpu, ty)?.is_empty())
}

/// Whether the handler at `handler` takes the type at `catchable` of
/// the throw whose ThrowInfo is at `throw`: the same type, by reference
/// if it can only be caught so, with the throw's const and volatile.
fn type_matches(cpu: &mut Cpu, handler: u32, catchable: u32, throw: u32) -> Option<bool> {
    if catches_all(cpu, handler)? {
        return Some(true);
    }
    let adjectives = read32(cpu, handler)?;
    let ty = read32(cpu, handler + 4)?;
    let properties = read32(cpu, catchable)?;
    let caught = read32(cpu, catchable + 4)?;
    if ty != caught && type_name(cpu, ty)? != type_name(cpu, caught)? {
        return Some(false);
    }
    let attributes = read32(cpu, throw)?;
    Some(
        (properties & CT_BY_REFERENCE == 0 || adjectives & HT_REFERENCE != 0)
            && (attributes & TI_CONST == 0 || adjectives & HT_CONST != 0)
            && (attributes & TI_VOLATILE == 0 || adjectives & HT_VOLATILE != 0),
    )
}

/// `object` as the type at `catchable`: moved by the type's offset in
/// it, and through the virtual base table where it is a virtual base.
fn adjust(cpu: &mut Cpu, object: u32, catchable: u32) -> Option<u32> {
    let member = read32(cpu, catchable + 8)?;
    let base = read32(cpu, catchable + 12)? as i32;
    let mut at = object.wrapping_add(member);
    if base >= 0 {
        let table = read32(cpu, object.wrapping_add(base as u32))?;
        let entry = read32(cpu, catchable + 16)?;
        let offset = read32(cpu, table.wrapping_add(entry))?;
        at = at.wrapping_add(offset).wrapping_add(base as u32);
    }
    Some(at)
}

/// Copy `len` bytes from `from` to `to`.
fn copy_bytes(cpu: &mut Cpu, from: u32, to: u32, len: u32) -> Option<()> {
    let data = bytes(cpu, from, len.min(0x1_0000))?;
    put(cpu, to, &data)
}

/// Put the exception's object where the catch has its variable: a
/// reference to it, a copy of its bytes, or one its copy constructor
/// makes. True when the constructor was called, which goes on in
/// `cxx_returned`.
fn catch_object(
    cpu: &mut Cpu,
    resume: (u32, u32),
    d: &mut [u32],
    catchable: Option<u32>,
) -> Option<bool> {
    let handler = d[D_HANDLER];
    let Some(catchable) = catchable else {
        return Some(false);
    };
    let adjectives = read32(cpu, handler)?;
    let place = read32(cpu, handler + 8)?;
    if catches_all(cpu, handler)? || place == 0 {
        return Some(false);
    }
    let to = d[D_FRAME].wrapping_add(12).wrapping_add(place);
    let object = d[D_OBJECT];
    let properties = read32(cpu, catchable)?;
    let size = read32(cpu, catchable + 20)?;
    if adjectives & HT_REFERENCE != 0 {
        let at = adjust(cpu, object, catchable)?;
        write32(cpu, to, at)?;
    } else if properties & CT_SIMPLE != 0 {
        copy_bytes(cpu, object, to, size)?;
        if size == 4 {
            let pointer = read32(cpu, to)?;
            if pointer != 0 {
                let at = adjust(cpu, pointer, catchable)?;
                write32(cpu, to, at)?;
            }
        }
    } else {
        let from = adjust(cpu, object, catchable)?;
        let copy = read32(cpu, catchable + 24)?;
        if copy == 0 {
            copy_bytes(cpu, from, to, size)?;
        } else {
            let args: &[u32] = if properties & CT_VIRTUAL_BASE != 0 {
                &[from, 1]
            } else {
                &[from]
            };
            d[0] = X_COPIED;
            cpu.set_ecx(to);
            return Some(call_program(cpu, copy, args, C_CXX, resume, d));
        }
    }
    Some(false)
}

/// RtlUnwind to the frame that catches or the guard the search started
/// in, then the function's objects in the try.
fn unwind_above(cpu: &mut Cpu, resume: (u32, u32), mut d: Vec<u32>) {
    d[0] = X_GLOBAL;
    let target = if d[D_MARKER] != 0 {
        d[D_MARKER]
    } else {
        d[D_FRAME]
    };
    if !call_program(
        cpu,
        rtl_unwind_address(),
        &[target, 0, 0, 0],
        C_CXX,
        resume,
        &d,
    ) {
        super::super::fatal(cpu, "a C++ exception, and its stack is gone", None);
    }
}

/// Destroy the frame's objects from its state down to d[D_TARGET], each
/// with the code its state's entry of the unwind map has, then return
/// ContinueSearch (X_RETURN) or call the catch block. A state outside the
/// map is terminate().
fn cxx_unwind(cpu: &mut Cpu, resume: (u32, u32), d: Vec<u32>) {
    let (frame, info) = (d[D_FRAME], d[D_INFO]);
    let target = d[D_TARGET] as i32;
    let states = cpu.bus.guest_read_32(info + 4) as i32;
    let map = cpu.bus.guest_read_32(info + 8);
    for _ in 0..MAX_ENTRIES {
        let state = cpu.bus.guest_read_32(frame + 8) as i32;
        if cpu.bus.guest_faulted() || state == target || states == 0 {
            break;
        }
        if state < 0 || state >= states {
            terminate(cpu, &[]);
            return;
        }
        let entry = map.wrapping_add((state as u32).wrapping_mul(8));
        let before = cpu.bus.guest_read_32(entry);
        let action = cpu.bus.guest_read_32(entry + 4);
        if cpu.bus.guest_faulted() {
            break;
        }
        cpu.bus.guest_write_32(frame + 8, before);
        if action != 0 {
            cpu.set_ebp(frame + 12);
            if call_program(cpu, action, &[], C_CXX, resume, &d) {
                return;
            }
            break;
        }
    }
    if d[0] == X_RETURN {
        restore_registers(cpu, &d[D_REGS..D_OUTER]);
        call::finish(cpu, resume, CONTINUE_SEARCH);
    } else {
        call_catch(cpu, resume, d);
    }
}

/// Call the catch block, in the state after its try, inside a guard
/// frame, with the thread's exception this one for a `throw;` in it.
fn call_catch(cpu: &mut Cpu, resume: (u32, u32), mut d: Vec<u32>) {
    let frame = d[D_FRAME];
    let high = cpu.bus.guest_read_32(d[D_TRY] + 4);
    let block = cpu.bus.guest_read_32(d[D_HANDLER] + 12);
    cpu.bus.guest_write_32(frame + 8, high.wrapping_add(1));
    d[D_ESP] = cpu.bus.guest_read_32(frame.wrapping_sub(4));
    if let Some(at) = thread_field(cpu, T_CXX) {
        d[D_OUTER] = cpu.bus.guest_read_32(at);
        d[D_OUTER + 1] = cpu.bus.guest_read_32(at + 4);
        cpu.bus.guest_write_32(at, d[D_RECORD]);
        cpu.bus.guest_write_32(at + 4, d[D_CONTEXT]);
    }
    d[0] = X_CAUGHT;
    cpu.set_ebp(frame + 12);
    let guard = cpu.esp().wrapping_sub(CATCH_GUARD_SIZE);
    let there = cpu.bus.guest_probe(guard, CATCH_GUARD_SIZE as usize, true);
    let next = seh::handlers(cpu);
    let handler = own_handler("__CatchGuardHandler");
    if there {
        cpu.bus.guest_write_32(guard, next);
        cpu.bus.guest_write_32(guard + 4, handler);
        store_search(cpu, guard + K_SEARCH, &d);
        cpu.set_esp(guard);
        seh::set_handlers(cpu, guard);
    }
    if !there || cpu.bus.guest_faulted() || !call_program(cpu, block, &[], C_CXX, resume, &d) {
        super::super::fatal(cpu, "a C++ exception, and its catch block is gone", None);
    }
}

/// The handler of the frame around a catch block. For an exception raised
/// in the block it looks at the try blocks in the catch block, one catch
/// depth in, with the translator's call that belongs to them, then a
/// `throw;` leaves the block and the search that found its catch goes on.
/// An unwind ends the block as its return does.
pub fn catch_guard(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[record, guard, context, _] = args else {
        unreachable!()
    };
    let outer = load_search(cpu, guard + K_SEARCH);
    if read32(cpu, record + 4)? & UNWINDING != 0 {
        leave_catch(cpu, &outer);
        let resume = call::here(cpu);
        let mut d = vec![0; D_LEN];
        d[0] = X_ABANDONED;
        d[D_REGS..D_OUTER].copy_from_slice(&saved_registers(cpu));
        let destructor = match outer[D_THROW] {
            0 => 0,
            throw => read32(cpu, throw + 4)?,
        };
        cpu.set_ecx(outer[D_OBJECT]);
        if destructor != 0 && call_program(cpu, destructor, &[], C_CXX, resume, &d) {
            return None;
        }
        restore_registers(cpu, &d[D_REGS..D_OUTER]);
        return Some(CONTINUE_SEARCH);
    }
    let mut d = vec![0; D_LEN];
    d[D_FRAME] = outer[D_FRAME];
    d[D_INFO] = outer[D_INFO];
    d[D_DEPTH] = outer[D_DEPTH] + 1;
    d[D_MARKER] = guard;
    d[D_CATCH_GUARD] = guard;
    d[D_RAW] = record;
    d[D_RECORD] = record;
    d[D_CONTEXT] = context;
    frame_handler(cpu, d)
}

/// What the end of a catch block restores: the function's saved ESP and
/// the thread's exception from before the catch.
fn leave_catch(cpu: &mut Cpu, d: &[u32]) {
    cpu.bus.guest_write_32(d[D_FRAME].wrapping_sub(4), d[D_ESP]);
    if let Some(at) = thread_field(cpu, T_CXX) {
        cpu.bus.guest_write_32(at, d[D_OUTER]);
        cpu.bus.guest_write_32(at + 4, d[D_OUTER + 1]);
    }
}

/// The catch block returned where the function goes on: the exception's
/// object destroyed, by its destructor where it has one.
fn destroy_object(cpu: &mut Cpu, resume: (u32, u32), mut d: Vec<u32>) {
    leave_catch(cpu, &d);
    d[0] = X_DESTROYED;
    if d[D_THROW] != 0 {
        let destructor = cpu.bus.guest_read_32(d[D_THROW] + 4);
        if !cpu.bus.guest_faulted() && destructor != 0 {
            cpu.set_ecx(d[D_OBJECT]);
            if call_program(cpu, destructor, &[], C_CXX, resume, &d) {
                return;
            }
        }
    }
    go_on(cpu, &d);
}

/// On in the function after its try, with its stack and frame.
fn go_on(cpu: &mut Cpu, d: &[u32]) {
    let frame = d[D_FRAME];
    let esp = d[D_ESP];
    let head = if d[D_MARKER] != 0 { d[D_MARKER] } else { frame };
    let tib = super::super::process(cpu)
        .current_thread()
        .map_or(0, |t| t.tib);
    cpu.bus.guest_write_32(tib, head);
    if let Some(thread) = super::super::process(cpu).current_thread_mut() {
        thread.fault.clear();
    }
    cpu.set_ebp(frame + 12);
    cpu.set_esp(esp);
    call::unwound_to(cpu, esp);
    cpu.set_eip(d[D_GO_ON]);
}

/// A `throw;` left the catch block of the guard `guard`, whose search goes
/// on where the catch was found, for the exception it was for: with the
/// next try block, then ContinueSearch for it, so that outer frames see
/// that exception's record. The block's object lives on with it.
fn left_by_rethrow(cpu: &mut Cpu, guard: u32) {
    let next = cpu.bus.guest_read_32(guard);
    let d = load_search(cpu, guard + K_SEARCH);
    leave_catch(cpu, &d);
    let resume = (d[D_RESUME], d[D_RESUME + 1]);
    seh::set_handlers(cpu, next);
    call::unwound_to(cpu, resume.1);
    cpu.set_esp(resume.1);
    if search_tries(cpu, resume, d.clone()) != Some(true) {
        end_search(cpu, resume, d);
    }
}

/// A call `cxx_frame_handler` made returned: its next step.
pub fn cxx_returned(cpu: &mut Cpu, value: u32, call: Call) {
    let mut d = call.data;
    if d.len() != D_LEN {
        return;
    }
    match d[0] {
        X_COPIED => unwind_above(cpu, call.resume, d),
        X_GLOBAL => {
            d[0] = X_LOCAL;
            cxx_unwind(cpu, call.resume, d);
        }
        X_CAUGHT => {
            d[D_GO_ON] = value;
            destroy_object(cpu, call.resume, d);
        }
        X_DESTROYED => go_on(cpu, &d),
        X_TRANSLATED => {
            let next = cpu.bus.guest_read_32(call.esp + POINTERS_SIZE + G_NEXT);
            seh::set_handlers(cpu, next);
            cpu.set_esp(call.resume.1);
            if search_tries(cpu, call.resume, d.clone()) != Some(true) {
                end_search(cpu, call.resume, d);
            }
        }
        X_DROPPED => unwind_translator(cpu, call.resume, d),
        X_LEFT_TRANSLATOR => leave_translator(cpu, d[D_TRANSLATOR]),
        X_RETHROWN => left_by_rethrow(cpu, d[D_CATCH_GUARD]),
        X_ABANDONED => {
            restore_registers(cpu, &d[D_REGS..D_OUTER]);
            call::finish(cpu, call.resume, CONTINUE_SEARCH);
        }
        X_TERMINATED => {
            abort(cpu, &[]);
        }
        X_UNEXPECTED => {
            terminate(cpu, &[]);
        }
        _ => cxx_unwind(cpu, call.resume, d),
    }
}

/// _CxxThrowException(object, type): the C++ exception, E06D7363h. With
/// no type it is a `throw;`, which `find_handler` looks up.
pub fn cxx_throw_exception(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let params = [CXX_MAGIC, args[0], args[1]];
    seh::raise_from_call(cpu, 2, CXX_EXCEPTION, 1, &params)
}

/// Functions with nothing to do: type_info's destructor, _lock, _unlock.
pub fn nothing(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

// Threads.

/// _beginthread(start, stack, argument): a thread running start(arg);
/// its handle, or -1.
pub fn beginthread(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = thread_field(cpu, T_SCRATCH)?;
    let handle = kernel32(cpu, "CreateThread", &[0, args[1], args[0], args[2], 0, id])?;
    if handle == 0 {
        set_errno(cpu, EAGAIN);
        return Some(EOF);
    }
    Some(handle)
}

/// _beginthreadex(security, stack, start, argument, flags, &id): its
/// handle, or 0.
pub fn beginthreadex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = if args[5] != 0 {
        args[5]
    } else {
        thread_field(cpu, T_SCRATCH)?
    };
    let handle = kernel32(
        cpu,
        "CreateThread",
        &[args[0], args[1], args[2], args[3], args[4], id],
    )?;
    if handle == 0 {
        set_errno(cpu, EAGAIN);
    }
    Some(handle)
}

/// _endthread() and _endthreadex(code).
pub fn endthread(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    kernel32(cpu, "ExitThread", &[0]);
    None
}

pub fn endthreadex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    kernel32(cpu, "ExitThread", &[args[0]]);
    None
}

/// _getpid(), __threadid(), __threadhandle().
pub fn getpid(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(super::super::process(cpu).id)
}

pub fn threadid(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(thread_id(cpu))
}

pub fn threadhandle(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0xFFFF_FFFE)
}

/// _sleep(ms).
pub fn sleep(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    kernel32(cpu, "Sleep", &[args[0]])
}

/// system(command): no command interpreter runs from here; only whether
/// there is one (for NULL) is answered.
pub fn system(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        return Some(1);
    }
    set_errno(cpu, ENOENT);
    Some(EOF)
}

/// _spawn's modes.
const P_WAIT: u32 = 0;
const P_NOWAIT: u32 = 1;
const P_OVERLAY: u32 = 2;

/// WaitForSingleObject's wait without end.
const INFINITE: u32 = 0xFFFF_FFFF;

/// The strings of the NULL-ended list of pointers at `at`.
fn string_list(cpu: &mut Cpu, mut at: u32) -> Option<Vec<String>> {
    let mut list = Vec::new();
    loop {
        let s = read32(cpu, at)?;
        if s == 0 {
            return Some(list);
        }
        list.push(crate::dosstr::from_bytes(&cstr(cpu, s)?));
        at = at.wrapping_add(4);
    }
}

/// The C runtime's block of the descriptors a child inherits, which
/// goes in lpReserved2: their count, their flags, their handles. Those
/// opened with O_NOINHERIT are left out.
fn inherit_block(cpu: &mut Cpu) -> Vec<u8> {
    let fds: Vec<(u8, u32)> = (0..MAX_FDS)
        .map(|fd| match io::fd_info(cpu, fd) {
            Some((handle, flags)) if flags & io::FNOINHERIT == 0 => (flags as u8, handle),
            _ => (0, u32::MAX),
        })
        .collect();
    let count = fds
        .iter()
        .rposition(|&(flags, _)| flags != 0)
        .map_or(0, |i| i + 1);
    let mut block = (count as u32).to_le_bytes().to_vec();
    block.extend(fds[..count].iter().map(|&(flags, _)| flags));
    block.extend(
        fds[..count]
            .iter()
            .flat_map(|&(_, handle)| handle.to_le_bytes()),
    );
    block
}

/// Start the program `path` with the arguments `argv`, joined by spaces
/// as MSVCRT joins them, and the environment `envp` (NULL for the
/// running one's), as `mode` says: _P_WAIT waits for it and gives its
/// exit code, _P_NOWAIT gives its process handle, _P_OVERLAY ends this
/// process; the others start it and give 0. -1, errno set, when it
/// can't start.
fn spawn(cpu: &mut Cpu, mode: u32, path: u32, argv: u32, envp: u32) -> Option<u32> {
    let path = crate::dosstr::from_bytes(&cstr(cpu, path)?);
    let line = string_list(cpu, argv)?.join(" ");
    let environment = match envp {
        0 => None,
        at => Some(string_list(cpu, at)?),
    };
    if mode > 4 {
        set_errno(cpu, EINVAL);
        return Some(EOF);
    }
    let size = super::super::kernel32::STARTUPINFO_SIZE;
    let mut startup = vec![0u8; size as usize];
    startup[..4].copy_from_slice(&size.to_le_bytes());
    startup.extend(inherit_block(cpu));
    let started =
        super::super::kernel32::spawn(cpu, Some(&path), Some(&line), environment, true, 0, startup);
    let [process, thread, _, _] = match started {
        Ok(info) => info,
        Err(code) => {
            set_doserrno(cpu, code);
            let errno = match code {
                193 => ENOEXEC,
                8 => ENOMEM,
                _ => ENOENT,
            };
            set_errno(cpu, errno);
            return Some(EOF);
        }
    };
    object::close(cpu, thread);
    match mode {
        P_WAIT => {
            // KERNEL32's wait, as the program would call it, which lets
            // the child run; `spawned` goes on when it is done.
            let wait = kernel32_stub("WaitForSingleObject")?;
            let resume = call::here(cpu);
            if call_program(
                cpu,
                wait,
                &[process, INFINITE],
                C_SPAWNED,
                resume,
                &[process],
            ) {
                return None;
            }
            object::close(cpu, process);
            set_errno(cpu, ENOMEM);
            Some(EOF)
        }
        P_NOWAIT => Some(process),
        P_OVERLAY => {
            object::close(cpu, process);
            let _ = super::stdio::flush_all(cpu);
            exit_now(cpu, &[0])
        }
        _ => {
            object::close(cpu, process);
            Some(0)
        }
    }
}

/// The child _P_WAIT waited for ended: its exit code.
pub fn spawned(cpu: &mut Cpu, _: u32, call: Call) {
    let &[process] = call.data.as_slice() else {
        return;
    };
    let code = match object::kind_of(cpu, process) {
        Some(object::Kind::Process { exit_code, .. }) => exit_code.unwrap_or(0),
        _ => 0,
    };
    object::close(cpu, process);
    call::finish(cpu, call.resume, code);
}

/// _spawnv(mode, path, argv) and _spawnvp, which looks in PATH too.
pub fn spawnv(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    spawn(cpu, args[0], args[1], args[2], 0)
}

/// _spawnve(mode, path, argv, envp).
pub fn spawnve(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    spawn(cpu, args[0], args[1], args[2], args[3])
}

/// _spawnl(mode, path, arg0, ..., NULL) and _spawnlp.
pub fn spawnl(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let argv = varargs(cpu, 2);
    spawn(cpu, args[0], args[1], argv, 0)
}

/// _execv(path, argv) and _execvp: the program in this one's place.
pub fn execv(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    spawn(cpu, P_OVERLAY, args[0], args[1], 0)
}

/// _execl(path, arg0, ..., NULL) and _execlp.
pub fn execl(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let argv = varargs(cpu, 1);
    spawn(cpu, P_OVERLAY, args[0], argv, 0)
}

// The heap.

/// malloc(size): a block, or NULL with errno ENOMEM.
pub fn malloc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] > 0xFFFF_FFE0 {
        set_errno(cpu, ENOMEM);
        return Some(0);
    }
    match heap_alloc(cpu, args[0].max(1)) {
        Some(at) => Some(at),
        None => {
            set_errno(cpu, ENOMEM);
            Some(0)
        }
    }
}

/// calloc(count, size): zeroed.
pub fn calloc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(size) = args[0].checked_mul(args[1]) else {
        set_errno(cpu, ENOMEM);
        return Some(0);
    };
    let at = malloc(cpu, &[size])?;
    if at != 0 {
        cpu.bus.guest_fill(at, size as usize, 0);
    }
    Some(at)
}

/// free(block).
pub fn free(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] != 0 {
        heap_free(cpu, args[0]);
    }
    Some(0)
}

/// realloc(block, size): NULL makes one, 0 frees it.
pub fn realloc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        return malloc(cpu, &[args[1]]);
    }
    if args[1] == 0 {
        heap_free(cpu, args[0]);
        return Some(0);
    }
    if heap_size(cpu, args[0]).is_none() {
        return Some(0);
    }
    match heap_realloc(cpu, args[0], args[1]) {
        Some(at) => Some(at),
        None if cpu.bus.guest_faulted() => None,
        None => {
            set_errno(cpu, ENOMEM);
            Some(0)
        }
    }
}

/// _msize(block): its size, -1 if it isn't one.
pub fn msize(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(heap_size(cpu, args[0]).unwrap_or(EOF))
}

/// _expand(block, size): made `size` bytes long where it is, or NULL.
pub fn expand(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    heap_size(cpu, args[0])?;
    let (at, size) = (args[0], args[1]);
    Some(if heap_resize(cpu, at, size) { at } else { 0 })
}

/// _heapchk() and _heapset(): _HEAPOK. _heapmin(): 0.
pub fn heapchk(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(-2i32 as u32)
}

/// _heapwalk(entry): no walking the heap; _HEAPEND.
pub fn heapwalk(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(-5i32 as u32)
}

/// _set_new_handler(f) and _query_new_handler(): the handler, which
/// nothing calls, as allocation fails only when memory is gone.
pub fn set_new_handler(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let old = state(cpu, S_NEW_HANDLER);
    set_state(cpu, S_NEW_HANDLER, args[0]);
    Some(old)
}

pub fn query_new_handler(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(state(cpu, S_NEW_HANDLER))
}

/// _set_new_mode(mode) and _query_new_mode().
pub fn set_new_mode(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let old = state(cpu, S_NEW_MODE);
    set_state(cpu, S_NEW_MODE, args[0]);
    Some(old)
}

pub fn query_new_mode(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(state(cpu, S_NEW_MODE))
}

/// _callnewh(size): no handler is called; 0.
pub fn callnewh(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

// The locale: "C", or the system's by name, whose tables are the same.

/// setlocale(category, name): the locale's name, or NULL for one there
/// isn't.
pub fn setlocale(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] > 5 {
        return Some(0);
    }
    put_cstr(cpu, state_at(S_LOCALE_C), b"C")?;
    put_cstr(cpu, state_at(S_LOCALE_NATIVE), NATIVE_LOCALE)?;
    if args[1] != 0 {
        let name = cstr(cpu, args[1])?;
        let native = name.is_empty()
            || name.eq_ignore_ascii_case(NATIVE_LOCALE)
            || name.eq_ignore_ascii_case(b"English_United States")
            || name.eq_ignore_ascii_case(b"English")
            || name.eq_ignore_ascii_case(b".1252");
        if native {
            set_state(cpu, S_LOCALE, 1);
        } else if name == b"C" {
            set_state(cpu, S_LOCALE, 0);
        } else {
            return Some(0);
        }
    }
    Some(if state(cpu, S_LOCALE) == 0 {
        state_at(S_LOCALE_C)
    } else {
        state_at(S_LOCALE_NATIVE)
    })
}

/// localeconv(): the "C" locale's lconv.
pub fn localeconv(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let text = state_at(S_LCONV_TEXT);
    put(cpu, text, b".\0\0")?;
    let at = state_at(S_LCONV);
    for i in 0..10 {
        write32(cpu, at + 4 * i, if i == 0 { text } else { text + 2 })?;
    }
    put(cpu, at + 40, &[0x7F; 8])?;
    Some(at)
}

/// __lconv_init(): 0.
pub fn lconv_init(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// ___lc_codepage_func(), ___lc_handle_func(), ___mb_cur_max_func().
pub fn lc_codepage(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    read32(cpu, var_at(V_LC_CODEPAGE))
}

pub fn mb_cur_max(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    read32(cpu, var_at(V_MB_CUR_MAX))
}

/// __unDName and friends: no names demangled.
pub fn zero(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// _setmaxstdio(count): the streams there can be, at most 2048 as
/// MSVCRT 6 allows; past the 512 there are, fopen fails with EMFILE.
pub fn setmaxstdio(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(if (20..=2048).contains(&args[0]) {
        args[0]
    } else {
        EOF
    })
}

pub fn getmaxstdio(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(MAX_STREAMS)
}

/// _assert(expression, file, line): an assertion failed. It is said on
/// standard error, and the program aborts as with abort.
pub fn assert(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let expression = cstr(cpu, args[0]).unwrap_or_default();
    let file = cstr(cpu, args[1]).unwrap_or_default();
    let mut message = b"Assertion failed: ".to_vec();
    message.extend_from_slice(&expression);
    message.extend_from_slice(b", file ");
    message.extend_from_slice(&file);
    message.extend_from_slice(format!(", line {}\r\n", args[2]).as_bytes());
    super::io::write_fd(cpu, 2, &message);
    abort(cpu, args)
}
