//! The program's start and end, and the rest of the runtime around it:
//! the arguments and environment, `_initterm`, the `atexit` functions
//! and `exit`, structured exception handling for `__try` (`_except_
//! handler3` and the unwinds), setjmp and longjmp, signals, threads, the
//! heap functions and the locale.

use super::*;
use crate::win32::object;

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

// C++: no exceptions yet.

/// __CxxFrameHandler(record, frame, context, dispatcher): the frames of
/// C++ functions don't catch anything; on.
pub fn cxx_frame_handler(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(CONTINUE_SEARCH)
}

/// _CxxThrowException(object, type): the C++ exception, E06D7363h.
pub fn cxx_throw_exception(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let params = thread_field(cpu, T_SCRATCH)?;
    write32(cpu, params, 0x1993_0520)?;
    write32(cpu, params + 4, args[0])?;
    write32(cpu, params + 8, args[1])?;
    kernel32(cpu, "RaiseException", &[0xE06D_7363, 1, 3, params])
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
    let count = fds.iter().rposition(|&(flags, _)| flags != 0).map_or(0, |i| i + 1);
    let mut block = (count as u32).to_le_bytes().to_vec();
    block.extend(fds[..count].iter().map(|&(flags, _)| flags));
    block.extend(fds[..count].iter().flat_map(|&(_, handle)| handle.to_le_bytes()));
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
    let started = super::super::kernel32::spawn(
        cpu,
        Some(&path),
        Some(&line),
        environment,
        true,
        0,
        startup,
    );
    let [process, thread, _, _] = match started {
        Ok(info) => info,
        Err(code) => {
            set_doserrno(cpu, code);
            set_errno(cpu, if code == 193 { ENOEXEC } else { ENOENT });
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
            if call_program(cpu, wait, &[process, INFINITE], C_SPAWNED, resume, &[process]) {
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
    let ok = super::super::with_process(cpu, |p, bus| {
        p.heap.resize_in_place(&mut p.memory, bus, at, size)
    });
    Some(if ok { at } else { 0 })
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
