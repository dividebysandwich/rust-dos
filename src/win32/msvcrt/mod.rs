//! MSVCRT.DLL: the C runtime of Visual C++ 4.2 to 6, which programs built
//! with `/MD` import instead of linking their own. Its functions are
//! written in Rust on what KERNEL32 and DOS have; its state is in the
//! DLL's data section and on the process heap, in the program's memory,
//! so it is per process and goes into saved states with the memory.
//!
//! The variables programs import as data (`_iob`, `_environ`, `_pctype`,
//! `__mb_cur_max` and the others) are in that section, filled for each
//! process when it is made (`init`). Per thread there is a block on the
//! heap with `errno`, `strtok`'s place, `rand`'s seed and the buffers
//! `asctime` and `_ecvt` return (`thread_block`).
//!
//! Functions that call back into the program (`qsort`, `_initterm`,
//! `exit`'s `atexit` functions, signal handlers, `_except_handler3`'s
//! filters) do so through `call` (call.rs): each step is a function in
//! `CONTS`, run when the program's function returns.

mod apis;
mod format;
mod io;
mod math;
mod mbcs;
mod startup;
mod stdio;
mod string;
mod time;

use super::call::{self, Call};
use super::stubs::{Dll, Handler, Var};
use crate::bus::Bus;
use crate::cpu::Cpu;

/// Windows 95's own DLLs are from BFB00000h up, KERNEL32 at the top;
/// MSVCRT goes below them, so the ones still to come get their places.
pub static DLL: Dll = Dll {
    name: "MSVCRT.DLL",
    base: 0xBF80_0000,
    apis: apis::APIS,
    vars: VARS,
    init: Some(init),
    vtables: &[],
};

/// Streams in `_iob`, and streams there can be at all (`_NSTREAM_`).
const IOB_ENTRIES: u32 = 20;
const MAX_STREAMS: u32 = 512;
/// A FILE's size, as `_iobuf` is laid out.
const FILE_SIZE: u32 = 32;
/// File descriptors there can be, and each one's entry: the handle and
/// the flags (`io.rs`).
const MAX_FDS: u32 = 256;
const FD_SIZE: u32 = 8;
/// Threads whose blocks are kept, as (thread id, block).
const MAX_THREADS: u32 = 64;
/// `_sys_errlist`'s messages.
const SYS_NERR: u32 = 43;

/// The variables, by their place in `VARS`.
const V_IOB: usize = 0;
const V_ENVIRON: usize = 1;
const V_INITENV: usize = 2;
const V_ARGC: usize = 3;
const V_ARGV: usize = 4;
const V_ACMDLN: usize = 5;
const V_PGMPTR: usize = 6;
const V_FMODE: usize = 7;
const V_COMMODE: usize = 8;
const V_OSVER: usize = 10;
const V_WINVER: usize = 11;
const V_WINMAJOR: usize = 12;
const V_WINMINOR: usize = 13;
const V_CTYPE: usize = 14;
const V_PCTYPE: usize = 15;
const V_PWCTYPE: usize = 16;
const V_MB_CUR_MAX: usize = 17;
const V_MBCTYPE: usize = 18;
const V_SYS_NERR: usize = 19;
const V_SYS_ERRLIST: usize = 20;
const V_TIMEZONE: usize = 21;
const V_DAYLIGHT: usize = 22;
const V_TZNAME: usize = 24;
const V_HUGE: usize = 25;
const V_WENVIRON: usize = 26;
const V_WINITENV: usize = 27;
const V_WARGV: usize = 28;
const V_WCMDLN: usize = 29;
const V_WPGMPTR: usize = 30;
const V_LC_CODEPAGE: usize = 31;
const V_MBCASEMAP: usize = 34;
const V_WCTYPE: usize = 35;
const V_ERRTEXT: usize = 36;
const V_TZTEXT: usize = 37;
const V_FDS: usize = 38;
const V_STREAMS: usize = 39;
const V_THREADS: usize = 40;
const V_STATE: usize = 41;

const fn var(name: &'static str, ordinal: u16, size: u32) -> Var {
    Var {
        name,
        ordinal,
        size,
    }
}

/// The variables. Append only, as the functions.
static VARS: &[Var] = &[
    var("_iob", 900, IOB_ENTRIES * FILE_SIZE),
    var("_environ", 901, 4),
    var("__initenv", 902, 4),
    var("__argc", 903, 4),
    var("__argv", 904, 4),
    var("_acmdln", 905, 4),
    var("_pgmptr", 906, 4),
    var("_fmode", 907, 4),
    var("_commode", 908, 4),
    var("_adjust_fdiv", 909, 4),
    var("_osver", 910, 4),
    var("_winver", 911, 4),
    var("_winmajor", 912, 4),
    var("_winminor", 913, 4),
    var("_ctype", 914, 257 * 2),
    var("_pctype", 915, 4),
    var("_pwctype", 916, 4),
    var("__mb_cur_max", 917, 4),
    var("_mbctype", 918, 257),
    var("_sys_nerr", 919, 4),
    var("_sys_errlist", 920, SYS_NERR * 4),
    var("_timezone", 921, 4),
    var("_daylight", 922, 4),
    var("_dstbias", 923, 4),
    var("_tzname", 924, 8),
    var("_HUGE", 925, 8),
    var("_wenviron", 926, 4),
    var("__winitenv", 927, 4),
    var("__wargv", 928, 4),
    var("_wcmdln", 929, 4),
    var("_wpgmptr", 930, 4),
    var("__lc_codepage", 931, 4),
    var("__lc_collate_cp", 932, 4),
    var("__lc_handle", 933, 24),
    var("_mbcasemap", 934, 256),
    // The DLL's own: the wide character classes `_pwctype` points at,
    // the text `_sys_errlist` and `_tzname` point at, the file
    // descriptors, the streams there are, the threads' blocks and the
    // rest (`S_*`).
    var("wctype", 0, 256 * 2),
    var("errtext", 0, 2048),
    var("tztext", 0, 64),
    var("fds", 0, MAX_FDS * FD_SIZE),
    var("streams", 0, MAX_STREAMS * 4),
    var("threads", 0, MAX_THREADS * 8),
    var("state", 0, 0x200),
];

/// The places in the `state` variable: the application type, the
/// `atexit` table (its address, count and room), `_set_new_mode`'s mode,
/// the handler `__setusermatherr` set, `signal`'s handlers (one for each
/// signal number below 32), `_set_error_mode`'s mode and `_onexit`'s
/// `__dllonexit` lock.
const S_APP_TYPE: u32 = 0x00;
const S_ATEXIT: u32 = 0x04;
const S_ATEXIT_COUNT: u32 = 0x08;
const S_ATEXIT_ROOM: u32 = 0x0C;
const S_NEW_MODE: u32 = 0x10;
const S_MATHERR: u32 = 0x14;
const S_ERROR_MODE: u32 = 0x18;
const S_EXITING: u32 = 0x1C;
const S_UMASK: u32 = 0x20;
const S_TEMP_COUNT: u32 = 0x24;
const S_MBCP: u32 = 0x28;
/// When the process started, by GetTickCount, for `clock`.
const S_START: u32 = 0x2C;
/// tmpnam's calls so far.
const S_TMPNAM_COUNT: u32 = 0x30;
const S_SIGNALS: u32 = 0x80;

/// A thread's block: its errno and _doserrno, rand's seed, strtok's
/// place (_mbstok's too) and wcstok's, the floating-point error signal's
/// code, and the buffers of gmtime, asctime, tmpnam, the ecvt family,
/// _wasctime, and room for what a function writes for a KERNEL32 one it
/// calls. 10h is free.
const T_ERRNO: u32 = 0x00;
const T_DOSERRNO: u32 = 0x04;
const T_RAND: u32 = 0x08;
const T_STRTOK: u32 = 0x0C;
const T_FPECODE: u32 = 0x14;
const T_WCSTOK: u32 = 0x18;
const T_TM: u32 = 0x20;
const T_ASCTIME: u32 = 0x50;
const T_TMPNAM: u32 = 0x80;
const T_CVT: u32 = 0x190;
const T_WASCTIME: u32 = 0x300;
const T_SCRATCH: u32 = 0x340;
const THREAD_BLOCK: u32 = 0x400;

/// errno's values.
const ENOENT: u32 = 2;
const ENOEXEC: u32 = 8;
const EBADF: u32 = 9;
const EAGAIN: u32 = 11;
const ENOMEM: u32 = 12;
const EACCES: u32 = 13;
const EEXIST: u32 = 17;
const EXDEV: u32 = 18;
const EINVAL: u32 = 22;
const EMFILE: u32 = 24;
const ENOSPC: u32 = 28;
const EDOM: u32 = 33;
const ERANGE: u32 = 34;
const ENOTEMPTY: u32 = 41;

/// The address of the variable `index` in the running process.
fn var_at(index: usize) -> u32 {
    super::stubs::var_address(dll_index(), index)
}

/// MSVCRT's place in `stubs::DLLS`.
fn dll_index() -> usize {
    super::stubs::DLLS
        .iter()
        .position(|d| std::ptr::eq(*d, &DLL))
        .expect("MSVCRT is a built-in DLL")
}

fn state_at(offset: u32) -> u32 {
    var_at(V_STATE) + offset
}

/// The state value at `offset`.
fn state(cpu: &mut Cpu, offset: u32) -> u32 {
    cpu.bus.guest_read_32(state_at(offset))
}

fn set_state(cpu: &mut Cpu, offset: u32, value: u32) {
    cpu.bus.guest_write_32(state_at(offset), value);
}

/// The KERNEL32 function `name`'s Rust side, which functions here call as
/// the real MSVCRT calls KERNEL32.
fn kernel32_fn(name: &str) -> Option<Handler> {
    super::kernel32::DLL
        .apis
        .iter()
        .find(|api| api.name == name)
        .map(|api| api.handler)
}

/// The address of the stub of the KERNEL32 function `name`, for calling
/// it as the program does.
fn kernel32_stub(name: &str) -> Option<u32> {
    let api = super::kernel32::DLL
        .apis
        .iter()
        .position(|api| api.name == name)?;
    Some(super::stubs::stub_address(0, api))
}

/// Call the KERNEL32 function `name` with `args`. None if KERNEL32 doesn't
/// have it, or if it faulted.
fn kernel32(cpu: &mut Cpu, name: &str, args: &[u32]) -> Option<u32> {
    let result = kernel32_fn(name)?(cpu, args);
    if cpu.bus.guest_faulted() {
        return None;
    }
    result
}

/// The string at `at`, without its 0. None if a page of it isn't there.
fn cstr(cpu: &mut Cpu, at: u32) -> Option<Vec<u8>> {
    cstr_n(cpu, at, usize::MAX)
}

/// The string at `at`, at most `max` bytes of it.
fn cstr_n(cpu: &mut Cpu, at: u32, max: usize) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 0x1000];
    let mut lin = at;
    while bytes.len() < max {
        let run = ((0x1000 - (lin & 0xFFF)) as usize).min(max - bytes.len());
        cpu.bus.guest_read_bytes(lin, &mut chunk[..run]);
        if cpu.bus.guest_faulted() {
            return None;
        }
        match chunk[..run].iter().position(|&b| b == 0) {
            Some(end) => {
                bytes.extend_from_slice(&chunk[..end]);
                return Some(bytes);
            }
            None => bytes.extend_from_slice(&chunk[..run]),
        }
        lin = lin.wrapping_add(run as u32);
    }
    Some(bytes)
}

/// The wide string at `at`, without its 0.
fn wstr(cpu: &mut Cpu, at: u32) -> Option<Vec<u16>> {
    wstr_n(cpu, at, usize::MAX)
}

fn wstr_n(cpu: &mut Cpu, at: u32, max: usize) -> Option<Vec<u16>> {
    let mut chars = Vec::new();
    while chars.len() < max {
        let c = cpu
            .bus
            .guest_read_16(at.wrapping_add(2 * chars.len() as u32));
        if cpu.bus.guest_faulted() {
            return None;
        }
        if c == 0 {
            break;
        }
        chars.push(c);
    }
    Some(chars)
}

/// `len` bytes at `at`, all of them there before any is read.
fn bytes(cpu: &mut Cpu, at: u32, len: u32) -> Option<Vec<u8>> {
    if !cpu.bus.guest_probe(at, len as usize, false) {
        return None;
    }
    let mut data = vec![0; len as usize];
    cpu.bus.guest_read_bytes(at, &mut data);
    (!cpu.bus.guest_faulted()).then_some(data)
}

/// Write `data` at `at`; None if a page isn't there.
fn put(cpu: &mut Cpu, at: u32, data: &[u8]) -> Option<()> {
    cpu.bus.guest_write_bytes(at, data);
    (!cpu.bus.guest_faulted()).then_some(())
}

/// Write `data` and a 0 at `at`.
fn put_cstr(cpu: &mut Cpu, at: u32, data: &[u8]) -> Option<()> {
    put(cpu, at, data)?;
    put(cpu, at.wrapping_add(data.len() as u32), &[0])
}

/// Write the wide string `data` and a 0 at `at`.
fn put_wstr(cpu: &mut Cpu, at: u32, data: &[u16]) -> Option<()> {
    let mut out: Vec<u8> = data.iter().flat_map(|c| c.to_le_bytes()).collect();
    out.extend([0, 0]);
    put(cpu, at, &out)
}

fn read32(cpu: &mut Cpu, at: u32) -> Option<u32> {
    let value = cpu.bus.guest_read_32(at);
    (!cpu.bus.guest_faulted()).then_some(value)
}

fn write32(cpu: &mut Cpu, at: u32, value: u32) -> Option<()> {
    cpu.bus.guest_write_32(at, value);
    (!cpu.bus.guest_faulted()).then_some(())
}

/// The double the arguments `args[i]` and `args[i + 1]` make.
fn f64_arg(args: &[u32], i: usize) -> f64 {
    f64::from_bits((args[i + 1] as u64) << 32 | args[i] as u64)
}

/// Return `value` as cdecl functions return a double: in ST(0).
fn ret_f64(cpu: &mut Cpu, value: f64) -> Option<u32> {
    cpu.fpu_push_f64(value);
    Some(0)
}

/// Return the 64-bit `value` in EDX:EAX.
fn ret_u64(cpu: &mut Cpu, value: u64) -> Option<u32> {
    cpu.set_edx((value >> 32) as u32);
    Some(value as u32)
}

/// Where the variable arguments of a cdecl function with `fixed`
/// arguments start: past its return address and those.
fn varargs(cpu: &Cpu, fixed: u32) -> u32 {
    cpu.esp().wrapping_add(4 + 4 * fixed)
}

/// The ANSI code page's character of `byte`, and the other way.
fn widen(byte: u8) -> u16 {
    super::convert::CodePage::Ansi.char(byte)
}

fn narrow(c: u16) -> Option<u8> {
    super::convert::CodePage::Ansi.byte(c)
}

/// The running thread's id.
fn thread_id(cpu: &mut Cpu) -> u32 {
    super::process(cpu).current_thread().map_or(0, |t| t.id)
}

/// The running thread's block (see `T_ERRNO`), made when it first needs
/// one. None when there is no memory for it, or a page isn't there.
fn thread_block(cpu: &mut Cpu) -> Option<u32> {
    let tid = thread_id(cpu);
    let table = var_at(V_THREADS);
    let mut free = None;
    for i in 0..MAX_THREADS {
        let at = table + 8 * i;
        let id = read32(cpu, at)?;
        if id == tid {
            return read32(cpu, at + 4);
        }
        if free.is_none() && (id == 0 || !thread_alive(cpu, id)) {
            free = Some(at);
        }
    }
    let slot = free?;
    let old = read32(cpu, slot + 4)?;
    let block = if old != 0 {
        old
    } else {
        heap_alloc(cpu, THREAD_BLOCK)?
    };
    cpu.bus.guest_fill(block, THREAD_BLOCK as usize, 0);
    write32(cpu, block + T_RAND, 1)?;
    write32(cpu, slot, tid)?;
    write32(cpu, slot + 4, block)?;
    Some(block)
}

/// Whether the thread `id` of the running process still runs.
fn thread_alive(cpu: &mut Cpu, id: u32) -> bool {
    super::process(cpu)
        .threads
        .iter()
        .any(|t| t.id == id && t.exit_code.is_none())
}

/// The address of the running thread's field `offset` of its block.
fn thread_field(cpu: &mut Cpu, offset: u32) -> Option<u32> {
    Some(thread_block(cpu)? + offset)
}

fn set_errno(cpu: &mut Cpu, errno: u32) {
    if let Some(at) = thread_field(cpu, T_ERRNO) {
        cpu.bus.guest_write_32(at, errno);
    }
}

fn set_doserrno(cpu: &mut Cpu, code: u32) {
    if let Some(at) = thread_field(cpu, T_DOSERRNO) {
        cpu.bus.guest_write_32(at, code);
    }
}

/// errno for the DOS or Win32 error `code`, as `_dosmaperr` maps them;
/// `_doserrno` gets the code.
fn set_dos_error(cpu: &mut Cpu, code: u32) {
    set_doserrno(cpu, code);
    let errno = match code {
        2 | 3 | 15 | 18 | 53 | 161 => ENOENT,
        4 => EMFILE,
        5 | 16 | 19 | 32 | 33 | 36 | 65 | 67 | 82 | 108 | 158 => EACCES,
        6 | 130 => EBADF,
        7..=9 => ENOMEM,
        17 => EXDEV,
        80 | 183 => EEXIST,
        89 => EAGAIN,
        112 => ENOSPC,
        145 => ENOTEMPTY,
        _ => EINVAL,
    };
    set_errno(cpu, errno);
}

/// A block of `size` bytes from the process heap, as malloc gives them.
fn heap_alloc(cpu: &mut Cpu, size: u32) -> Option<u32> {
    super::with_process(cpu, |p, bus| p.heap.alloc(&mut p.memory, bus, size))
}

fn heap_free(cpu: &mut Cpu, at: u32) -> bool {
    super::process(cpu).heap.free(at)
}

fn heap_size(cpu: &mut Cpu, at: u32) -> Option<u32> {
    super::process(cpu).heap.size(at)
}

/// The block at `at` made `size` bytes long, moved if it has to be: its
/// address, or None, the block as it was, when there is no room.
fn heap_realloc(cpu: &mut Cpu, at: u32, size: u32) -> Option<u32> {
    let old = heap_size(cpu, at)?;
    let in_place = super::with_process(cpu, |p, bus| {
        p.heap.resize_in_place(&mut p.memory, bus, at, size)
    });
    if in_place {
        return Some(at);
    }
    let to = heap_alloc(cpu, size)?;
    let data = bytes(cpu, at, old.min(size))?;
    put(cpu, to, &data)?;
    heap_free(cpu, at);
    Some(to)
}

/// What runs when a call into the program returns, by the first word of
/// the call's data. Append only.
static CONTS: &[call::Then] = &[
    startup::initterm_returned,
    startup::atexit_returned,
    string::qsort_returned,
    string::bsearch_returned,
    startup::signal_returned,
    startup::filter_returned,
    startup::finally_returned,
    startup::rtl_unwound,
    string::lfind_returned,
    startup::spawned,
];

const C_INITTERM: u32 = 0;
const C_ATEXIT: u32 = 1;
const C_QSORT: u32 = 2;
const C_BSEARCH: u32 = 3;
const C_SIGNAL: u32 = 4;
const C_FILTER: u32 = 5;
const C_FINALLY: u32 = 6;
const C_UNWOUND: u32 = 7;
const C_LFIND: u32 = 8;
const C_SPAWNED: u32 = 9;

/// Call the program's function at `target` with `args`; when it returns,
/// `CONTS[cont]` runs with the call, whose data is `data` after the
/// continuation's number. False if the stack isn't there.
fn call_program(
    cpu: &mut Cpu,
    target: u32,
    args: &[u32],
    cont: u32,
    resume: (u32, u32),
    data: &[u32],
) -> bool {
    let mut words = vec![cont];
    words.extend_from_slice(data);
    call::call(cpu, target, args, call::MSVCRT, resume, words)
}

/// A call `call_program` made returned (call.rs).
pub fn returned(cpu: &mut Cpu, value: u32, mut call: Call) {
    if call.data.is_empty() {
        return;
    }
    let cont = call.data.remove(0);
    match CONTS.get(cont as usize) {
        Some(then) => then(cpu, value, call),
        None => super::fatal(cpu, "a return to an unknown place in MSVCRT", None),
    }
}

/// The classes of the characters 0 to 255 in the "C" locale, as `_ctype`
/// has them after its entry for EOF.
const UPPER: u16 = 0x01;
const LOWER: u16 = 0x02;
const DIGIT: u16 = 0x04;
const SPACE: u16 = 0x08;
const PUNCT: u16 = 0x10;
const CONTROL: u16 = 0x20;
const BLANK: u16 = 0x40;
const HEX: u16 = 0x80;
const ALPHA: u16 = 0x100;

fn ctype_of(c: u8) -> u16 {
    match c {
        b'\t' => SPACE | CONTROL | BLANK,
        b'\n'..=b'\r' => SPACE | CONTROL,
        0..=0x1F | 0x7F => CONTROL,
        b' ' => SPACE | BLANK,
        b'0'..=b'9' => DIGIT | HEX,
        b'A'..=b'F' => UPPER | HEX | ALPHA,
        b'G'..=b'Z' => UPPER | ALPHA,
        b'a'..=b'f' => LOWER | HEX | ALPHA,
        b'g'..=b'z' => LOWER | ALPHA,
        0x21..=0x7E => PUNCT,
        _ => 0,
    }
}

/// The classes of the wide characters 0 to 255: the "C" locale's below
/// 128, Latin-1's above, as `_pwctype` has them.
fn wctype_of(c: u16) -> u16 {
    match c {
        0..=0x7F => ctype_of(c as u8),
        0x80..=0x9F => CONTROL,
        0xA0 => SPACE | BLANK,
        0xAA | 0xB5 | 0xBA | 0xDF..=0xF6 | 0xF8..=0xFF => LOWER | ALPHA,
        0xC0..=0xD6 | 0xD8..=0xDE => UPPER | ALPHA,
        0xB2 | 0xB3 | 0xB9 => DIGIT | PUNCT,
        _ => PUNCT,
    }
}

/// `_sys_errlist`'s messages, by errno, as MSVCRT has them.
const ERRORS: [&str; SYS_NERR as usize] = [
    "No error",
    "Operation not permitted",
    "No such file or directory",
    "No such process",
    "Interrupted function call",
    "Input/output error",
    "No such device or address",
    "Arg list too long",
    "Exec format error",
    "Bad file descriptor",
    "No child processes",
    "Resource temporarily unavailable",
    "Not enough space",
    "Permission denied",
    "Bad address",
    "Unknown error",
    "Resource device",
    "File exists",
    "Improper link",
    "No such device",
    "Not a directory",
    "Is a directory",
    "Invalid argument",
    "Too many open files in system",
    "Too many open files",
    "Inappropriate I/O control operation",
    "Unknown error",
    "File too large",
    "No space left on device",
    "Invalid seek",
    "Read-only file system",
    "Too many links",
    "Broken pipe",
    "Domain error",
    "Result too large",
    "Unknown error",
    "Resource deadlock avoided",
    "Unknown error",
    "Filename too long",
    "No locks available",
    "Function not implemented",
    "Directory not empty",
    "Illegal byte sequence",
];

/// The message of errno `errno`, as strerror gives it.
fn error_text(errno: u32) -> &'static str {
    ERRORS
        .get(errno as usize)
        .copied()
        .unwrap_or("Unknown error")
}

/// Fill the variables for `process`, which has just been made: the
/// standard streams and descriptors, the character tables, the version,
/// the command line split into arguments and the environment.
fn init(bus: &mut Bus, process: &mut super::Process) -> Option<()> {
    let mut w = Writer { bus, process };
    // The standard streams: stdin reads, stdout and stderr write.
    for (i, flag) in [stdio::IOREAD, stdio::IOWRT, stdio::IOWRT]
        .into_iter()
        .enumerate()
    {
        let file = var_at(V_IOB) + FILE_SIZE * i as u32;
        w.u32(file + stdio::F_FLAG, flag)?;
        w.u32(file + stdio::F_FILE, i as u32)?;
    }
    for i in 0..IOB_ENTRIES {
        let file = var_at(V_IOB) + FILE_SIZE * i;
        w.u32(var_at(V_STREAMS) + 4 * i, file)?;
        if i >= 3 {
            w.u32(file + stdio::F_FILE, u32::MAX)?;
        }
    }
    for fd in 0..MAX_FDS {
        w.u32(var_at(V_FDS) + FD_SIZE * fd, u32::MAX)?;
        w.u32(var_at(V_FDS) + FD_SIZE * fd + 4, 0)?;
    }
    // The descriptors the parent's C runtime passed in lpReserved2: their
    // count, a byte of flags each, then a handle each. The standard ones
    // it didn't pass are the standard handles.
    let size = super::kernel32::STARTUPINFO_SIZE as usize;
    let block = w.process.startup.get(size..).unwrap_or(&[]).to_vec();
    let word = |at: usize| {
        block
            .get(at..at + 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    };
    let count = word(0).unwrap_or(0) as usize;
    let mut passed = [false; 3];
    for fd in 0..count.min(MAX_FDS as usize) {
        let (Some(&flags), Some(handle)) = (block.get(4 + fd), word(4 + count + 4 * fd)) else {
            break;
        };
        let flags = flags as u32;
        if flags & io::FOPEN == 0 || w.process.handles.of(handle).is_none() {
            continue;
        }
        let mut flags = flags & !(io::FDEV | io::FNOINHERIT);
        if io::is_device(w.bus, w.process, handle) {
            flags |= io::FDEV;
        }
        let entry = var_at(V_FDS) + FD_SIZE * fd as u32;
        w.u32(entry, handle)?;
        w.u32(entry + 4, flags)?;
        if let Some(p) = passed.get_mut(fd) {
            *p = true;
        }
    }
    let std = w.process.std;
    for (fd, handle) in std.into_iter().enumerate() {
        if passed[fd] {
            continue;
        }
        let flags = io::FOPEN
            | io::FTEXT
            | if io::is_device(w.bus, w.process, handle) {
                io::FDEV
            } else {
                0
            };
        let entry = var_at(V_FDS) + FD_SIZE * fd as u32;
        w.u32(entry, handle)?;
        w.u32(entry + 4, flags)?;
    }

    // The character classes, and the case map of the multibyte code page.
    let ctype: Vec<u8> = std::iter::once(0)
        .chain((0..=255u8).map(ctype_of))
        .flat_map(u16::to_le_bytes)
        .collect();
    w.bytes(var_at(V_CTYPE), &ctype)?;
    w.u32(var_at(V_PCTYPE), var_at(V_CTYPE) + 2)?;
    let wctype: Vec<u8> = (0..=255u16)
        .map(wctype_of)
        .flat_map(u16::to_le_bytes)
        .collect();
    w.bytes(var_at(V_WCTYPE), &wctype)?;
    w.u32(var_at(V_PWCTYPE), var_at(V_WCTYPE))?;
    w.u32(var_at(V_MB_CUR_MAX), 1)?;
    w.bytes(var_at(V_MBCTYPE), &mbcs::table(1252).unwrap())?;
    let casemap: Vec<u8> = (0..=255u8)
        .map(|c| match c {
            b'a'..=b'z' => c - 32,
            b'A'..=b'Z' => c + 32,
            _ => 0,
        })
        .collect();
    w.bytes(var_at(V_MBCASEMAP), &casemap)?;

    // Windows 95, by GetVersion: its build in _osver.
    w.u32(var_at(V_OSVER), 0x4000)?;
    w.u32(var_at(V_WINVER), 0x0400)?;
    w.u32(var_at(V_WINMAJOR), 4)?;
    w.u32(var_at(V_WINMINOR), 0)?;

    // The error messages, and the time zone's names: there are no time
    // zones, local time is UTC (kernel32's time.rs).
    let mut text = Vec::new();
    for (i, message) in ERRORS.iter().enumerate() {
        w.u32(
            var_at(V_SYS_ERRLIST) + 4 * i as u32,
            var_at(V_ERRTEXT) + text.len() as u32,
        )?;
        text.extend_from_slice(message.as_bytes());
        text.push(0);
    }
    w.bytes(var_at(V_ERRTEXT), &text)?;
    w.u32(var_at(V_SYS_NERR), SYS_NERR)?;
    w.bytes(var_at(V_TZTEXT), b"GMT\0\0\0\0\0GMT\0")?;
    w.u32(var_at(V_TZNAME), var_at(V_TZTEXT))?;
    w.u32(var_at(V_TZNAME) + 4, var_at(V_TZTEXT) + 8)?;
    w.u32(state_at(S_START), (w.bus.clock.now_micros() / 1000) as u32)?;
    w.bytes(var_at(V_HUGE), &f64::INFINITY.to_le_bytes())?;
    w.u32(state_at(S_ATEXIT), 0)?;
    w.u32(state_at(S_MBCP), 1252)?;

    // The command line, the program's path, the arguments and the
    // environment.
    let line = w.process.command_line;
    w.u32(var_at(V_ACMDLN), line)?;
    let path = crate::dosstr::to_bytes(&w.process.path);
    let pgmptr = w.cstr(&path)?;
    w.u32(var_at(V_PGMPTR), pgmptr)?;
    let mut text = Vec::new();
    let mut at = line;
    loop {
        let b = w.process.memory.translate(w.bus, at)?;
        let byte = w.bus.read_8(b as usize);
        if byte == 0 {
            break;
        }
        text.push(byte);
        at += 1;
    }
    let args = startup::split_command_line(&text);
    let argv = w.array(&args)?;
    w.u32(var_at(V_ARGC), args.len() as u32)?;
    w.u32(var_at(V_ARGV), argv)?;
    let environment: Vec<Vec<u8>> = w
        .process
        .environment
        .iter()
        .map(|s| crate::dosstr::to_bytes(s))
        .collect();
    let environ = w.array(&environment)?;
    w.u32(var_at(V_ENVIRON), environ)?;
    w.u32(var_at(V_INITENV), environ)
}

/// Writes into a process that isn't the running one, through its own
/// page tables, and takes blocks from its heap.
struct Writer<'a> {
    bus: &'a mut Bus,
    process: &'a mut super::Process,
}

impl Writer<'_> {
    fn bytes(&mut self, at: u32, data: &[u8]) -> Option<()> {
        self.process.memory.write(self.bus, at, data)
    }

    fn u32(&mut self, at: u32, value: u32) -> Option<()> {
        self.process.memory.write_32(self.bus, at, value)
    }

    fn alloc(&mut self, size: u32) -> Option<u32> {
        let p = &mut *self.process;
        p.heap.alloc(&mut p.memory, self.bus, size)
    }

    /// A copy of `text` and a 0 on the heap.
    fn cstr(&mut self, text: &[u8]) -> Option<u32> {
        let at = self.alloc(text.len() as u32 + 1)?;
        self.bytes(at, text)?;
        self.bytes(at + text.len() as u32, &[0])?;
        Some(at)
    }

    /// An array of pointers to copies of `strings`, ended by NULL.
    fn array(&mut self, strings: &[Vec<u8>]) -> Option<u32> {
        let array = self.alloc(4 * (strings.len() as u32 + 1))?;
        for (i, s) in strings.iter().enumerate() {
            let at = self.cstr(s)?;
            self.u32(array + 4 * i as u32, at)?;
        }
        self.u32(array + 4 * strings.len() as u32, 0)?;
        Some(array)
    }
}
