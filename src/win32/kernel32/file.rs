//! Files: CreateFileA and the functions on the handles it gives, and the
//! 16-bit era's _lopen, _lread and OpenFile on top of them. Files go
//! through DOS's file table (`DiskController`) as INT 21h's do: a file
//! object holds its entry there and closes it when the last handle goes
//! (object.rs). The standard handles are DOS's handles 0, 1 and 2 of the
//! DOS process the programs run in.

use super::path::{self, ERROR_FILE_EXISTS, ERROR_FILENAME_EXCED_RANGE};
use super::virtual_memory::allows;
use super::{
    ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_CALL_NOT_IMPLEMENTED, ERROR_INVALID_HANDLE,
    ERROR_INVALID_PARAMETER, FALSE, INVALID_HANDLE_VALUE, TRUE, set_error, string_at,
};
use crate::cpu::Cpu;
use crate::disk::CharDevice;
use crate::win32::object::{self, Kind};
use crate::win32::thread::{self, Wait};

pub(super) const GENERIC_READ: u32 = 0x8000_0000;
pub(super) const GENERIC_WRITE: u32 = 0x4000_0000;

/// CreateFileA's dispositions.
const CREATE_NEW: u32 = 1;
const CREATE_ALWAYS: u32 = 2;
const OPEN_EXISTING: u32 = 3;
const OPEN_ALWAYS: u32 = 4;
const TRUNCATE_EXISTING: u32 = 5;

/// FILE_SHARE_READ and FILE_SHARE_WRITE, and DOS's sharing modes.
const FILE_SHARE_READ: u32 = 1;
const FILE_SHARE_WRITE: u32 = 2;

/// GetFileType's answers.
const FILE_TYPE_UNKNOWN: u32 = 0;
const FILE_TYPE_DISK: u32 = 1;
const FILE_TYPE_CHAR: u32 = 2;

const FILE_ATTRIBUTE_READONLY: u32 = 0x01;

const ERROR_INVALID_FUNCTION: u32 = 1;
pub(super) const ERROR_SHARING_VIOLATION: u32 = 32;
/// FILE_SHARE_DELETE, which Windows 95 doesn't have.
const FILE_SHARE_DELETE: u32 = 4;
const ERROR_LOCK_VIOLATION: u32 = 33;
const ERROR_NEGATIVE_SEEK: u32 = 131;
const ERROR_NOT_LOCKED: u32 = 158;
const ERROR_BAD_NETPATH: u32 = 53;
const ERROR_IO_INCOMPLETE: u32 = 996;
const STATUS_PENDING: u32 = 0x103;

/// What _lopen, _lread and the others return when they fail.
const HFILE_ERROR: u32 = 0xFFFF_FFFF;

/// How much of a read or write goes to DOS at a time: DOS counts bytes
/// in 16 bits, and the program's length is its own.
const CHUNK: u32 = 0x8000;

/// How often a thread waiting for a line typed at the console looks for
/// keys, in microseconds.
const KEY_POLL: u64 = 10_000;

/// What a handle of a file or device is.
#[derive(Clone, Copy)]
pub(super) struct Open {
    /// Its entry in DOS's file table.
    pub sft: u16,
    /// GENERIC_READ and GENERIC_WRITE, as it was opened.
    pub access: u32,
    /// DOS's handle for a standard handle (0, 1 or 2), or 0FFFFh.
    pub dos: u16,
}

/// The open file or device `handle` is a handle of: a file CreateFileA
/// opened or one of DOS's handles.
pub(super) fn open_of(cpu: &mut Cpu, handle: u32) -> Option<Open> {
    match object::kind_of(cpu, handle)? {
        Kind::File { sft, access, .. } => Some(Open {
            sft,
            access,
            dos: 0xFFFF,
        }),
        Kind::Dos { handle: dos } => {
            let psp = cpu.bus.win32.psp;
            let sft =
                crate::win32::dos(cpu, |cpu| crate::dos_files::sft_of(&mut cpu.bus, psp, dos))?;
            Some(Open {
                sft,
                access: GENERIC_READ | GENERIC_WRITE,
                dos,
            })
        }
        _ => None,
    }
}

/// The open file of `handle`, or with the last error set, None.
fn open_or_fail(cpu: &mut Cpu, handle: u32) -> Option<Open> {
    let open = open_of(cpu, handle);
    if open.is_none() {
        set_error(cpu, ERROR_INVALID_HANDLE);
    }
    open
}

/// Whether the open file `sft` is the console.
fn is_console(cpu: &Cpu, sft: u16) -> bool {
    cpu.bus.disk.handle_device(sft) == Some(CharDevice::Con)
}

/// Fail with the DOS error `code`, whose Win32 code is the same number
/// (2 file not found, 3 path not found, 5 access denied, ...).
pub(super) fn dos_error(cpu: &mut Cpu, code: u8) -> u32 {
    set_error(cpu, code as u32);
    FALSE
}

/// Open the file `name` through DOS as CreateFileA does, with `access`,
/// `share` and `disposition`, as its file table's entry and whether it was
/// there already; or the error.
pub(super) fn open_file(
    cpu: &mut Cpu,
    name: &str,
    access: u32,
    share: u32,
    disposition: u32,
) -> Result<(u16, bool), u32> {
    // There is no network, and Windows 95 has no \\?\ or \\.\ namespace
    // for drives; it opens no directory, so a name that ends in a slash
    // is a path that isn't found, but for a drive's root, which is denied.
    if name.starts_with("\\\\") || name.starts_with("//") {
        return Err(ERROR_BAD_NETPATH);
    }
    if let [_, b':', b'\\' | b'/'] = name.as_bytes() {
        return Err(ERROR_ACCESS_DENIED);
    }
    if name.is_empty() || name.ends_with(['\\', '/']) {
        return Err(super::ERROR_PATH_NOT_FOUND);
    }
    let name = path::dos_path(name);
    let psp = cpu.bus.win32.psp;
    let mut mode = match access & (GENERIC_READ | GENERIC_WRITE) {
        GENERIC_WRITE => 1,
        x if x == GENERIC_READ | GENERIC_WRITE => 2,
        _ => 0,
    };
    // DOS's sharing modes: deny both, deny write, deny read, deny none.
    mode |= match share & (FILE_SHARE_READ | FILE_SHARE_WRITE) {
        0 => 0x10,
        FILE_SHARE_READ => 0x20,
        FILE_SHARE_WRITE => 0x30,
        _ => 0x40,
    };
    let disk = &mut cpu.bus.disk;
    if disk.device(&name).is_some() {
        return disk
            .open_file(&name, mode, psp)
            .map(|sft| (sft, true))
            .map_err(|e| e as u32);
    }
    if disk.is_directory(&name) {
        return Err(ERROR_ACCESS_DENIED);
    }
    if path::has_wildcards(&name) {
        return Err(path::ERROR_INVALID_NAME);
    }
    let exists = disk.is_file(&name);
    if exists && sharing_violation(cpu, &name, access, share) {
        return Err(ERROR_SHARING_VIOLATION);
    }
    let disk = &mut cpu.bus.disk;
    let result = match disposition {
        CREATE_NEW if exists => return Err(ERROR_FILE_EXISTS),
        CREATE_NEW | CREATE_ALWAYS => disk.create_file(&name, psp),
        OPEN_ALWAYS if !exists => disk.create_file(&name, psp),
        OPEN_EXISTING | OPEN_ALWAYS => disk.open_file(&name, mode, psp),
        TRUNCATE_EXISTING => {
            let sft = disk.open_file(&name, mode, psp).map_err(|e| e as u32)?;
            if let Err(e) = disk.set_file_size(sft, 0) {
                disk.close_file(sft);
                return Err(e as u32);
            }
            Ok(sft)
        }
        _ => return Err(ERROR_INVALID_PARAMETER),
    };
    result.map(|sft| (sft, exists)).map_err(|e| e as u32)
}

/// A handle of a new file object for the open file `sft`.
fn new_file(cpu: &mut Cpu, sft: u16, access: u32, share: u32) -> u32 {
    let kind = Kind::File {
        sft,
        access: access & (GENERIC_READ | GENERIC_WRITE),
        share: share & (FILE_SHARE_READ | FILE_SHARE_WRITE),
        locks: Vec::new(),
    };
    object::create(cpu, kind, None)
}

/// Whether the file `name` is open with access or sharing that opening it
/// with `access` and `share` conflicts with: each handle's access must be
/// what the others share. Windows 95 takes no access as reading.
pub(super) fn sharing_violation(cpu: &Cpu, name: &str, access: u32, share: u32) -> bool {
    let reading = |access: u32| match access & (GENERIC_READ | GENERIC_WRITE) {
        0 => GENERIC_READ,
        access => access,
    };
    let conflicts = |access: u32, share: u32| {
        (access & GENERIC_READ != 0 && share & FILE_SHARE_READ == 0)
            || (access & GENERIC_WRITE != 0 && share & FILE_SHARE_WRITE == 0)
    };
    let disk = &cpu.bus.disk;
    let key = disk.file_key(name);
    let access = reading(access);
    cpu.bus
        .win32
        .objects
        .objects
        .iter()
        .flatten()
        .any(|o| match o.kind {
            Kind::File {
                sft,
                access: theirs,
                share: their_share,
                ..
            } if disk.handle_device(sft).is_none() && disk.handle_key(sft) == Some(key) => {
                conflicts(reading(theirs), share) || conflicts(access, their_share)
            }
            _ => false,
        })
}

/// CreateFileA(name, access, share, security, disposition, flags,
/// template): a handle of the file or device, or INVALID_HANDLE_VALUE.
/// The last error is ERROR_ALREADY_EXISTS when CREATE_ALWAYS or
/// OPEN_ALWAYS found the file there. Windows 95 does no overlapped I/O
/// on files; FILE_FLAG_OVERLAPPED is taken and does nothing.
pub(super) fn create_file(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[name, access, share, _, disposition, flags, _] = args else {
        unreachable!()
    };
    if name == 0 {
        set_error(cpu, super::ERROR_PATH_NOT_FOUND);
        return Some(INVALID_HANDLE_VALUE);
    }
    let name = string_at(cpu, name)?;
    if !(CREATE_NEW..=TRUNCATE_EXISTING).contains(&disposition)
        || (disposition == TRUNCATE_EXISTING && access & GENERIC_WRITE == 0)
        || share & FILE_SHARE_DELETE != 0
    {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(INVALID_HANDLE_VALUE);
    }
    match open_file(cpu, &name, access, share, disposition) {
        Ok((sft, existed)) => {
            if !existed && flags & FILE_ATTRIBUTE_READONLY != 0 {
                let name = path::dos_path(&name);
                let _ = cpu.bus.disk.set_file_attribute(&name, 0x01);
            }
            let handle = new_file(cpu, sft, access, share);
            let reported = matches!(disposition, CREATE_ALWAYS | OPEN_ALWAYS);
            set_error(
                cpu,
                if existed && reported {
                    ERROR_ALREADY_EXISTS
                } else {
                    0
                },
            );
            Some(handle)
        }
        Err(code) => {
            set_error(cpu, code);
            Some(INVALID_HANDLE_VALUE)
        }
    }
}

/// The running thread waits for keys typed at the console, then calls
/// the function again: `read_console` asked for that the DOS way
/// (`hle_wait`), which a Win32 thread does by sleeping.
pub(super) fn wait_for_keys(cpu: &mut Cpu) {
    cpu.hle_retry = false;
    cpu.idle = false;
    cpu.set_eip(cpu.eip().wrapping_sub(4));
    let deadline = Some(cpu.bus.clock.now_micros() + KEY_POLL);
    thread::block(
        cpu,
        Wait {
            deadline,
            ..Wait::default()
        },
    );
}

/// Read at most `n` bytes from the console as a line typed at it: the
/// bytes, or None when the thread waits for keys.
pub(super) fn read_console_line(cpu: &mut Cpu, n: u32) -> Option<Vec<u8>> {
    // The key DOS keeps the line being typed under: no DS:DX of a DOS
    // call is in the ROM.
    let key = (crate::win32::ROM_SEG, 0xFFFF);
    let line = crate::win32::dos(cpu, |cpu| {
        crate::interrupts::int21::read_console(cpu, n as usize, key)
    });
    if line.is_none() {
        wait_for_keys(cpu);
    }
    line
}

/// ReadFile's and WriteFile's answer to a buffer the program can't
/// reach: nothing done, ERROR_INVALID_PARAMETER.
fn bad_buffer(cpu: &mut Cpu, done: u32) -> Option<u32> {
    if done != 0 {
        cpu.bus.guest_write_32(done, 0);
        if cpu.bus.guest_faulted() {
            return None;
        }
    }
    set_error(cpu, ERROR_INVALID_PARAMETER);
    Some(FALSE)
}

/// ReadFile(handle, buffer, n, read, overlapped): up to `n` bytes, the
/// count read to `*read`. At the end of a file it reads none and
/// succeeds. From the console it reads a line, as DOS does. Windows 95
/// fails a read with an OVERLAPPED on a file with ERROR_INVALID_PARAMETER.
pub(super) fn read_file(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, buffer, n, read, overlapped] = args else {
        unreachable!()
    };
    // Windows 9x checks the buffer first, without a page fault.
    if n != 0 && (buffer == 0 || !allows(cpu, buffer, n, true)) {
        return bad_buffer(cpu, read);
    }
    let Some(open) = open_or_fail(cpu, handle) else {
        return Some(FALSE);
    };
    if open.access & GENERIC_READ == 0 {
        set_error(cpu, ERROR_ACCESS_DENIED);
        return Some(FALSE);
    }
    let device = cpu.bus.disk.handle_device(open.sft);
    if overlapped != 0 && device.is_none() {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    if read != 0 && !cpu.bus.guest_probe(read, 4, true) {
        return None;
    }
    let (count, error) = read_open(cpu, open, buffer, n)?;
    if read != 0 {
        cpu.bus.guest_write_32(read, count);
    }
    Some(result(cpu, error))
}

/// TRUE, or with the error `error` set, FALSE.
fn result(cpu: &mut Cpu, error: Option<u32>) -> u32 {
    match error {
        None => TRUE,
        Some(code) => {
            set_error(cpu, code);
            FALSE
        }
    }
}

/// Read up to `n` bytes of the open file `open` to `buffer`: the count
/// read, and the error that stopped it. None if a page isn't there, or
/// the thread waits for keys.
fn read_open(cpu: &mut Cpu, open: Open, buffer: u32, n: u32) -> Option<(u32, Option<u32>)> {
    if !cpu.bus.guest_probe(buffer, n as usize, true) {
        return None;
    }
    if is_console(cpu, open.sft) {
        let line = read_console_line(cpu, n)?;
        cpu.bus.guest_write_bytes(buffer, &line);
        return Some((line.len() as u32, None));
    }
    let mut count = 0u32;
    while count < n {
        let len = (n - count).min(CHUNK);
        match cpu.bus.disk.read_file(open.sft, len as usize) {
            Ok(bytes) => {
                cpu.bus
                    .guest_write_bytes(buffer.wrapping_add(count), &bytes);
                count += bytes.len() as u32;
                if (bytes.len() as u32) < len {
                    break;
                }
            }
            Err(code) => return Some((count, Some(code as u32))),
        }
    }
    Some((count, None))
}

/// WriteFile and WriteConsoleA: `n` bytes at `buffer` to `handle`, the
/// count written to `*written` (when it isn't NULL). Writing no bytes to
/// a file sets its end where the file pointer is, as DOS does. A full
/// disk writes fewer bytes, and succeeds.
pub(super) fn write_file(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, buffer, n, written, overlapped] = args else {
        unreachable!()
    };
    // Windows 9x checks the buffer first, without a page fault. It
    // reports writing all of a NULL buffer, and writes nothing here.
    if n != 0 && buffer == 0 && open_or_fail(cpu, handle).is_some() {
        if written != 0 {
            cpu.bus.guest_write_32(written, n);
        }
        return Some(TRUE);
    }
    if n != 0 && buffer != 0 && !allows(cpu, buffer, n, false) {
        return bad_buffer(cpu, written);
    }
    let Some(open) = open_or_fail(cpu, handle) else {
        return Some(FALSE);
    };
    if open.access & GENERIC_WRITE == 0 {
        set_error(cpu, ERROR_ACCESS_DENIED);
        return Some(FALSE);
    }
    let device = cpu.bus.disk.handle_device(open.sft);
    if overlapped != 0 && device.is_none() {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    if written != 0 && !cpu.bus.guest_probe(written, 4, true) {
        return None;
    }
    let (count, error) = write_open(cpu, open, buffer, n)?;
    if written != 0 {
        cpu.bus.guest_write_32(written, count);
    }
    Some(result(cpu, error))
}

/// Write `n` bytes at `buffer` to the open file `open`, or with none set
/// its end at its pointer: the count written, and the error that stopped
/// it. None if a page isn't there.
fn write_open(cpu: &mut Cpu, open: Open, buffer: u32, n: u32) -> Option<(u32, Option<u32>)> {
    // All of it is there before any of it is written.
    if !cpu.bus.guest_probe(buffer, n as usize, false) {
        return None;
    }
    let mut count = 0u32;
    let mut error = None;
    if n == 0 && cpu.bus.disk.handle_device(open.sft).is_none() {
        let disk = &mut cpu.bus.disk;
        let end = disk.position(open.sft).unwrap_or(0);
        if let Err(code) = disk.set_file_size(open.sft, end)
            && code != 0x27
        {
            error = Some(code as u32);
        }
    }
    while count < n {
        let len = (n - count).min(CHUNK);
        let mut data = vec![0u8; len as usize];
        cpu.bus
            .guest_read_bytes(buffer.wrapping_add(count), &mut data);
        if cpu.bus.guest_faulted() {
            return None;
        }
        let (dos, sft) = (open.dos, open.sft);
        let result = crate::win32::dos(cpu, |cpu| {
            crate::interrupts::int21::write_open_file(cpu, dos, sft, &data)
        });
        match result {
            Ok(done) => {
                count += done as u32;
                if (done as u32) < len {
                    break;
                }
            }
            Err(code) => {
                error = Some(code as u32);
                break;
            }
        }
    }
    Some((count, error))
}

/// SetFilePointer(handle, low, high, method): move the file pointer by
/// the distance `low` (with `*high` above it if `high` isn't NULL) from
/// the start, the pointer or the end. Its new place's low 32 bits, and
/// high ones to `*high`; 0FFFFFFFFh with an error.
pub(super) fn set_file_pointer(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, low, high, method] = args else {
        unreachable!()
    };
    let Some(open) = open_or_fail(cpu, handle) else {
        return Some(0xFFFF_FFFF);
    };
    let distance = if high == 0 {
        low as i32 as i64
    } else {
        let high = cpu.bus.guest_read_32(high);
        if cpu.bus.guest_faulted() {
            return None;
        }
        ((high as u64) << 32 | low as u64) as i64
    };
    if method > 2 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0xFFFF_FFFF);
    }
    let disk = &mut cpu.bus.disk;
    let base = match method {
        0 => 0,
        1 => disk.position(open.sft).unwrap_or(0) as i64,
        _ => size_of(disk, open.sft) as i64,
    };
    let Some(to) = base.checked_add(distance).filter(|&to| to >= 0) else {
        set_error(cpu, ERROR_NEGATIVE_SEEK);
        return Some(0xFFFF_FFFF);
    };
    match disk.seek_file(open.sft, to, 0) {
        Ok(at) => {
            if high != 0 {
                cpu.bus.guest_write_32(high, (at >> 32) as u32);
            }
            set_error(cpu, 0);
            Some(at as u32)
        }
        Err(code) => {
            set_error(cpu, code as u32);
            Some(0xFFFF_FFFF)
        }
    }
}

/// The size of the open file `sft`.
fn size_of(disk: &crate::disk::DiskController, sft: u16) -> u32 {
    disk.sft_entry(sft).map_or(0, |e| e.size)
}

/// GetFileSize(handle, high): the size's low 32 bits, its high ones (0)
/// to `*high`.
pub(super) fn get_file_size(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(open) = open_or_fail(cpu, args[0]) else {
        return Some(0xFFFF_FFFF);
    };
    if args[1] != 0 {
        cpu.bus.guest_write_32(args[1], 0);
    }
    set_error(cpu, 0);
    Some(size_of(&cpu.bus.disk, open.sft))
}

/// Disk files, and the character devices (the console, NUL, the ports).
pub(super) fn get_file_type(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(open) = open_or_fail(cpu, args[0]) else {
        return Some(FILE_TYPE_UNKNOWN);
    };
    Some(match cpu.bus.disk.handle_device(open.sft) {
        Some(_) => FILE_TYPE_CHAR,
        None => FILE_TYPE_DISK,
    })
}

/// Nothing is kept from the disk: there is nothing to write.
pub(super) fn flush_file_buffers(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(match open_or_fail(cpu, args[0]) {
        Some(_) => TRUE,
        None => FALSE,
    })
}

/// The file ends where its pointer is: cut there, or grown with zeros.
pub(super) fn set_end_of_file(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(open) = open_or_fail(cpu, args[0]) else {
        return Some(FALSE);
    };
    if open.access & GENERIC_WRITE == 0 {
        set_error(cpu, ERROR_ACCESS_DENIED);
        return Some(FALSE);
    }
    let disk = &mut cpu.bus.disk;
    if disk.handle_device(open.sft).is_some() {
        return Some(TRUE);
    }
    let at = disk.position(open.sft).unwrap_or(0);
    match disk.set_file_size(open.sft, at) {
        Ok(()) => Some(TRUE),
        Err(code) => Some(dos_error(cpu, code)),
    }
}

/// The FILETIME of the open file `sft`'s DOS time and date.
fn file_ticks(cpu: &Cpu, sft: u16) -> u64 {
    let (time, date) = cpu.bus.disk.file_time(sft).unwrap_or((0, 0x21));
    crate::win32::convert::dos_to_ticks(date, time).unwrap_or(0)
}

fn write_ticks(cpu: &mut Cpu, at: u32, ticks: u64) {
    cpu.bus.guest_write_32(at, ticks as u32);
    cpu.bus.guest_write_32(at + 4, (ticks >> 32) as u32);
}

/// GetFileInformationByHandle(handle, info): BY_HANDLE_FILE_INFORMATION
/// of a disk file. Its times are all the one DOS keeps; its index a
/// number that tells the file apart.
pub(super) fn get_file_information_by_handle(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(open) = open_or_fail(cpu, args[0]) else {
        return Some(FALSE);
    };
    let disk = &cpu.bus.disk;
    let Some(entry) = disk.sft_entry(open.sft).filter(|e| e.device.is_none()) else {
        set_error(cpu, ERROR_INVALID_FUNCTION);
        return Some(FALSE);
    };
    let attributes = disk
        .open_path(open.sft)
        .and_then(|p| disk.get_file_attribute(&p).ok())
        .map_or(0x20, |a| a as u32);
    let serial = disk
        .volume_serial(entry.drive)
        .unwrap_or(crate::interrupts::int21::VOLUME_SERIAL + entry.drive as u32);
    let key = disk.handle_key(open.sft).unwrap_or(0);
    let ticks = file_ticks(cpu, open.sft);
    let at = args[1];
    if !cpu.bus.guest_probe(at, 52, true) {
        return None;
    }
    cpu.bus.guest_write_32(at, attributes);
    for time in 0..3 {
        write_ticks(cpu, at + 4 + 8 * time, ticks);
    }
    let fields = [serial, 0, entry.size, 1, (key >> 32) as u32, key as u32];
    for (i, value) in fields.into_iter().enumerate() {
        cpu.bus.guest_write_32(at + 28 + 4 * i as u32, value);
    }
    Some(TRUE)
}

/// GetFileTime(handle, created, accessed, written): the file's one DOS
/// time, for each that isn't NULL.
pub(super) fn get_file_time(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(open) = open_or_fail(cpu, args[0]) else {
        return Some(FALSE);
    };
    let ticks = file_ticks(cpu, open.sft);
    for &at in &args[1..] {
        if at != 0 {
            write_ticks(cpu, at, ticks);
        }
    }
    Some(TRUE)
}

/// SetFileTime(handle, created, accessed, written): the time of the last
/// write is the DOS time, which files on disk images keep; host files
/// keep the host's, as with DOS. The others aren't kept.
pub(super) fn set_file_time(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(open) = open_or_fail(cpu, args[0]) else {
        return Some(FALSE);
    };
    if open.access & GENERIC_WRITE == 0 {
        set_error(cpu, ERROR_ACCESS_DENIED);
        return Some(FALSE);
    }
    if args[3] == 0 {
        return Some(TRUE);
    }
    let low = cpu.bus.guest_read_32(args[3]) as u64;
    let high = cpu.bus.guest_read_32(args[3] + 4) as u64;
    if cpu.bus.guest_faulted() {
        return None;
    }
    let Some((date, time)) = crate::win32::convert::ticks_to_dos(high << 32 | low) else {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    };
    match cpu.bus.disk.set_file_time(open.sft, time, date) {
        Ok(()) => Some(TRUE),
        Err(code) => Some(dos_error(cpu, code)),
    }
}

/// The locks of the file object `handle` names, changed by `f`, which
/// says what the function returns.
fn with_locks(
    cpu: &mut Cpu,
    handle: u32,
    f: impl FnOnce(&mut Vec<(u32, u32)>) -> Result<(), u32>,
) -> u32 {
    let id = object::resolve(cpu, handle);
    let object = id.and_then(|id| object::objects(cpu).get_mut(id));
    let result = match object.map(|o| &mut o.kind) {
        Some(Kind::File { locks, .. }) => f(locks),
        // The standard handles' files aren't locked.
        Some(Kind::Dos { .. }) => Ok(()),
        _ => Err(ERROR_INVALID_HANDLE),
    };
    match result {
        Ok(()) => TRUE,
        Err(code) => {
            set_error(cpu, code);
            FALSE
        }
    }
}

/// The range of a lock: Windows 95 locks only below 4 GB, and takes a
/// lock of no bytes and does nothing.
fn lock_range(args: &[u32]) -> Result<Option<(u32, u32)>, u32> {
    let &[_, low, high, len, len_high] = args else {
        unreachable!()
    };
    if high != 0 || len_high != 0 || low.checked_add(len).is_none() {
        return Err(ERROR_INVALID_PARAMETER);
    }
    Ok((len != 0).then_some((low, len)))
}

/// LockFile(handle, offset, offset high, length, length high): no other
/// lock may have any of the bytes.
pub(super) fn lock_file(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let range = lock_range(args);
    Some(with_locks(cpu, args[0], |locks| {
        let Some((at, len)) = range? else {
            return Ok(());
        };
        if locks.iter().any(|&(a, l)| at < a + l && a < at + len) {
            return Err(ERROR_LOCK_VIOLATION);
        }
        locks.push((at, len));
        Ok(())
    }))
}

/// UnlockFile: a lock of just these bytes goes.
pub(super) fn unlock_file(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let range = lock_range(args);
    Some(with_locks(cpu, args[0], |locks| {
        let Some(range) = range? else {
            return Ok(());
        };
        let i = locks
            .iter()
            .position(|&l| l == range)
            .ok_or(ERROR_NOT_LOCKED)?;
        locks.remove(i);
        Ok(())
    }))
}

/// GetOverlappedResult(handle, overlapped, transferred, wait): Windows
/// 95 does overlapped I/O on no files, so only the devices' (none) have
/// results: the handle must be one, and the OVERLAPPED's status says the
/// rest.
pub(super) fn get_overlapped_result(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, overlapped, transferred, _] = args else {
        unreachable!()
    };
    if open_or_fail(cpu, handle).is_none() {
        return Some(FALSE);
    }
    let status = cpu.bus.guest_read_32(overlapped);
    let count = cpu.bus.guest_read_32(overlapped + 4);
    if cpu.bus.guest_faulted() {
        return None;
    }
    if status == STATUS_PENDING {
        set_error(cpu, ERROR_IO_INCOMPLETE);
        return Some(FALSE);
    }
    cpu.bus.guest_write_32(transferred, count);
    if status != 0 {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    }
    Some(TRUE)
}

/// ReadFileEx and WriteFileEx: Windows 95 has them for serial ports alone,
/// which have no overlapped I/O here.
pub(super) fn file_ex(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    set_error(cpu, ERROR_INVALID_PARAMETER);
    Some(FALSE)
}

/// LockFileEx and UnlockFileEx: Windows 95 has stubs that fail.
pub(super) fn lock_file_ex(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    set_error(cpu, ERROR_CALL_NOT_IMPLEMENTED);
    Some(FALSE)
}

/// Windows 95 gives a process as many handles as it asks for.
pub(super) fn set_handle_count(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(args[0])
}

/// The access _lopen's and OpenFile's modes ask for: OF_READ,
/// OF_WRITE, OF_READWRITE, and FILE_SHARE_* for their sharing modes.
fn of_access(mode: u32) -> (u32, u32) {
    let access = match mode & 3 {
        1 => GENERIC_WRITE,
        2 => GENERIC_READ | GENERIC_WRITE,
        _ => GENERIC_READ,
    };
    let share = match mode & 0x70 {
        0x10 => 0,
        0x20 => FILE_SHARE_READ,
        0x30 => FILE_SHARE_WRITE,
        _ => FILE_SHARE_READ | FILE_SHARE_WRITE,
    };
    (access, share)
}

/// Open `name` for _lopen, _lcreat and OpenFile: a handle, or
/// HFILE_ERROR with the last error set.
fn open_hfile(cpu: &mut Cpu, name: &str, mode: u32, disposition: u32) -> u32 {
    let (access, share) = of_access(mode);
    match open_file(cpu, name, access, share, disposition) {
        Ok((sft, _)) => new_file(cpu, sft, access, share),
        Err(code) => {
            set_error(cpu, code);
            HFILE_ERROR
        }
    }
}

/// _lopen(name, mode): an existing file, for OF_READ, OF_WRITE or
/// OF_READWRITE.
pub(super) fn lopen(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let name = string_at(cpu, args[0])?;
    Some(open_hfile(cpu, &name, args[1], OPEN_EXISTING))
}

/// _lcreat(name, attributes): the file made anew, empty, for reading and
/// writing; attribute 1 makes it read-only.
pub(super) fn lcreat(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let name = string_at(cpu, args[0])?;
    let handle = open_hfile(cpu, &name, 2, CREATE_ALWAYS);
    if handle != HFILE_ERROR && args[1] & 1 != 0 {
        let name = path::dos_path(&name);
        let _ = cpu.bus.disk.set_file_attribute(&name, 0x01);
    }
    Some(handle)
}

/// _lclose(handle): 0, or HFILE_ERROR.
pub(super) fn lclose(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if object::close(cpu, args[0]) {
        Some(0)
    } else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        Some(HFILE_ERROR)
    }
}

/// _lread and _hread(handle, buffer, n): the count read, or HFILE_ERROR.
pub(super) fn lread(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, buffer, n] = args else {
        unreachable!()
    };
    let Some(open) = open_or_fail(cpu, handle) else {
        return Some(HFILE_ERROR);
    };
    if open.access & GENERIC_READ == 0 {
        set_error(cpu, ERROR_ACCESS_DENIED);
        return Some(HFILE_ERROR);
    }
    match read_open(cpu, open, buffer, n)? {
        (count, None) => Some(count),
        (_, Some(code)) => {
            set_error(cpu, code);
            Some(HFILE_ERROR)
        }
    }
}

/// _lwrite and _hwrite(handle, buffer, n): the count written, or
/// HFILE_ERROR. No bytes set the file's end at its pointer.
pub(super) fn lwrite(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, buffer, n] = args else {
        unreachable!()
    };
    let Some(open) = open_or_fail(cpu, handle) else {
        return Some(HFILE_ERROR);
    };
    if open.access & GENERIC_WRITE == 0 {
        set_error(cpu, ERROR_ACCESS_DENIED);
        return Some(HFILE_ERROR);
    }
    match write_open(cpu, open, buffer, n)? {
        (count, None) => Some(count),
        (_, Some(code)) => {
            set_error(cpu, code);
            Some(HFILE_ERROR)
        }
    }
}

/// _llseek(handle, offset, origin): the new place, or HFILE_ERROR.
pub(super) fn llseek(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    set_file_pointer(cpu, &[args[0], args[1], 0, args[2]])
}

/// OpenFile's styles.
const OF_PARSE: u32 = 0x0100;
const OF_DELETE: u32 = 0x0200;
const OF_CREATE: u32 = 0x1000;
const OF_EXIST: u32 = 0x4000;
/// OFSTRUCT's size, and the longest path it holds.
const OFSTRUCT: u32 = 136;
const OFS_MAXPATHNAME: usize = 128;

/// OpenFile(name, ofstruct, style): open, make, delete or look for the
/// file, the full path it has to the OFSTRUCT. A name without a
/// directory is looked for where SearchPathA looks. A handle; 1 for
/// OF_EXIST and OF_DELETE, which close the file again; HFILE_ERROR with
/// the error in the OFSTRUCT too.
pub(super) fn open_file_16(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[name, of, style] = args else {
        unreachable!()
    };
    let name = string_at(cpu, name)?;
    let found = if style & OF_CREATE == 0 && !name.contains(['\\', '/', ':']) {
        path::search(cpu, None, &name)
    } else {
        None
    };
    let full = found.unwrap_or_else(|| path::full_path(cpu, &name));
    if full.len() >= OFS_MAXPATHNAME {
        set_error(cpu, ERROR_FILENAME_EXCED_RANGE);
        cpu.bus
            .guest_write_16(of + 2, ERROR_FILENAME_EXCED_RANGE as u16);
        return Some(HFILE_ERROR);
    }
    if !cpu.bus.guest_probe(of, OFSTRUCT as usize, true) {
        return None;
    }
    let drive = full.as_bytes()[0].to_ascii_uppercase().wrapping_sub(b'A');
    let fixed = cpu.bus.disk.drive_kind(drive) == Some(crate::disk::DriveKind::HardDisk);
    let mut bytes = crate::dosstr::to_bytes(&full);
    bytes.push(0);
    cpu.bus.guest_write_bytes(of + 8, &bytes);
    let result = if style & OF_PARSE != 0 {
        Ok(0)
    } else if style & OF_DELETE != 0 {
        let path = path::dos_path(&full);
        cpu.bus
            .disk
            .delete_file(&path)
            .map(|()| 1)
            .map_err(|e| e as u32)
    } else {
        let disposition = if style & OF_CREATE != 0 {
            CREATE_ALWAYS
        } else {
            OPEN_EXISTING
        };
        let handle = open_hfile(cpu, &full, style, disposition);
        if handle == HFILE_ERROR {
            Err(crate::win32::process(cpu).last_error)
        } else if style & OF_EXIST != 0 {
            object::close(cpu, handle);
            Ok(1)
        } else {
            Ok(handle)
        }
    };
    match result {
        Ok(handle) => {
            cpu.bus.guest_write_8(of, OFSTRUCT as u8);
            cpu.bus.guest_write_8(of + 1, fixed as u8);
            cpu.bus.guest_write_16(of + 2, 0);
            Some(handle)
        }
        Err(code) => {
            // The structure's size and drive stay as they were.
            set_error(cpu, code);
            cpu.bus.guest_write_16(of + 2, code as u16);
            Some(HFILE_ERROR)
        }
    }
}
