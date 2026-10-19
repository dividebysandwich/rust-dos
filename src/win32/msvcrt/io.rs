//! Low-level I/O: the file descriptors (`_open`, `_read`, `_write`,
//! `_lseek`), each a handle and flags in the DLL's `fds` table; files,
//! directories and their names (`_stat`, `_findfirst`, `_mkdir`,
//! `_getcwd`).
//!
//! The handles are KERNEL32's: its file objects, opened as CreateFileA
//! opens them, or the standard handles, which are DOS's. They are read,
//! written and moved through DOS's file table (`dos_read`, `dos_write`)
//! as KERNEL32's ReadFile and WriteFile do.

use super::*;
use crate::disk::{CharDevice, DosDirEntry};
use crate::win32::kernel32;
use crate::win32::object::{self, Kind};

/// A descriptor's flags (`ioinfo.osfile`).
pub const FOPEN: u32 = 0x01;
pub const FEOFLAG: u32 = 0x02;
pub const FNOINHERIT: u32 = 0x10;
pub const FAPPEND: u32 = 0x20;
pub const FDEV: u32 = 0x40;
pub const FTEXT: u32 = 0x80;

/// `_open`'s flags.
const O_WRONLY: u32 = 0x0001;
const O_RDWR: u32 = 0x0002;
const O_APPEND: u32 = 0x0008;
const O_TEMPORARY: u32 = 0x0040;
const O_NOINHERIT: u32 = 0x0080;
const O_CREAT: u32 = 0x0100;
const O_TRUNC: u32 = 0x0200;
const O_EXCL: u32 = 0x0400;
const O_TEXT: u32 = 0x4000;
const O_BINARY: u32 = 0x8000;

/// `_stat`'s modes.
const S_IFDIR: u32 = 0x4000;
const S_IFCHR: u32 = 0x2000;
const S_IFREG: u32 = 0x8000;
const S_IREAD: u32 = 0x0100;
const S_IWRITE: u32 = 0x0080;
const S_IEXEC: u32 = 0x0040;

const EOF: u32 = u32::MAX;

/// Whether `handle` of `process` is a device (the console, NUL), for the
/// standard descriptors' flags when the process is made.
pub fn is_device(bus: &mut Bus, process: &super::super::Process, handle: u32) -> bool {
    let Some(id) = process.handles.of(handle) else {
        return false;
    };
    let sft = match bus.win32.objects.get(id).map(|o| o.kind.clone()) {
        Some(Kind::File { sft, .. }) => Some(sft),
        Some(Kind::Dos { handle: dos }) => {
            let psp = bus.win32.psp;
            let paging = bus.guest_paging.take();
            let sft = crate::dos_files::sft_of(bus, psp, dos);
            bus.guest_paging = paging;
            sft
        }
        _ => None,
    };
    sft.is_some_and(|sft| bus.disk.handle_device(sft).is_some())
}

/// The address of descriptor `fd`'s entry: its handle, then its flags.
fn entry(fd: u32) -> u32 {
    var_at(V_FDS) + FD_SIZE * fd
}

/// The handle and flags of the open descriptor `fd`; None, errno EBADF,
/// if it isn't one.
pub fn fd_info(cpu: &mut Cpu, fd: u32) -> Option<(u32, u32)> {
    if fd < MAX_FDS {
        let handle = cpu.bus.guest_read_32(entry(fd));
        let flags = cpu.bus.guest_read_32(entry(fd) + 4);
        if flags & FOPEN != 0 && !cpu.bus.guest_faulted() {
            return Some((handle, flags));
        }
    }
    set_errno(cpu, EBADF);
    set_doserrno(cpu, 0);
    None
}

fn set_flags(cpu: &mut Cpu, fd: u32, flags: u32) {
    cpu.bus.guest_write_32(entry(fd) + 4, flags);
}

/// A descriptor for `handle` with `flags`: the lowest free one at or
/// above `from`. None, errno EMFILE, when there is none.
fn new_fd(cpu: &mut Cpu, handle: u32, flags: u32, from: u32) -> Option<u32> {
    for fd in from..MAX_FDS {
        if cpu.bus.guest_read_32(entry(fd) + 4) & FOPEN == 0 {
            cpu.bus.guest_write_32(entry(fd), handle);
            cpu.bus.guest_write_32(entry(fd) + 4, flags | FOPEN);
            return Some(fd);
        }
    }
    set_errno(cpu, EMFILE);
    set_doserrno(cpu, 0);
    None
}

/// The open file (SFT entry) of the handle `handle`.
fn sft_of(cpu: &mut Cpu, handle: u32) -> Option<u16> {
    kernel32::open_of(cpu, handle).map(|open| open.sft)
}

/// A DOS path as programs write it: slashes either way.
fn dos_path(name: &[u8]) -> String {
    crate::dosstr::from_bytes(name).replace('/', "\\")
}

/// Open the file `path` as CreateFileA does, for reading (`access` 0),
/// writing (1) or both (2), shared as `share` (FILE_SHARE_READ and
/// FILE_SHARE_WRITE) allows; with `create` a missing one is made, with
/// `truncate` one there is emptied, with `exclusive` one there is an
/// error. A new handle of it, or the error.
fn dos_open(
    cpu: &mut Cpu,
    path: &str,
    (access, share): (u8, u32),
    create: bool,
    truncate: bool,
    exclusive: bool,
) -> Result<u32, u32> {
    let access = match access {
        0 => kernel32::GENERIC_READ,
        1 => kernel32::GENERIC_WRITE,
        _ => kernel32::GENERIC_READ | kernel32::GENERIC_WRITE,
    };
    let disposition = match (create, truncate) {
        (true, _) if exclusive => kernel32::CREATE_NEW,
        (true, true) => kernel32::CREATE_ALWAYS,
        (true, false) => kernel32::OPEN_ALWAYS,
        (false, true) => kernel32::TRUNCATE_EXISTING,
        (false, false) => kernel32::OPEN_EXISTING,
    };
    let (sft, _) = kernel32::open_file(cpu, path, access, share, disposition)?;
    Ok(kernel32::new_file(cpu, sft, access, share))
}

/// What a read gives: bytes (none at the end of the file), DOS's error,
/// or that the console has no line yet.
pub enum Read {
    Data(Vec<u8>),
    Error(u32),
    Wait,
}

/// Read up to `count` bytes from `handle`.
fn dos_read(cpu: &mut Cpu, handle: u32, count: u32) -> Read {
    let Some(sft) = sft_of(cpu, handle) else {
        return Read::Error(6);
    };
    match cpu.bus.disk.handle_device(sft) {
        Some(CharDevice::Con) => {
            // The key DOS keeps the line being typed under, as KERNEL32's
            // console reads have it.
            let key = (crate::win32::ROM_SEG, 0xFFFF);
            match crate::interrupts::int21::read_console(cpu, count as usize, key) {
                Some(data) => Read::Data(data),
                None => Read::Wait,
            }
        }
        Some(_) => Read::Data(Vec::new()),
        None => match cpu.bus.disk.read_file(sft, count as usize) {
            Ok(data) => Read::Data(data),
            Err(code) => Read::Error(code as u32),
        },
    }
}

/// Write `data` to `handle`: how many bytes were, or DOS's error.
fn dos_write(cpu: &mut Cpu, handle: u32, data: &[u8]) -> Result<u32, u32> {
    let open = kernel32::open_of(cpu, handle).ok_or(6u32)?;
    if open.access & kernel32::GENERIC_WRITE == 0 {
        return Err(5);
    }
    let mut done = 0;
    for chunk in data.chunks(0x8000) {
        let (dos, sft) = (open.dos, open.sft);
        let result = crate::win32::dos(cpu, |cpu| {
            crate::interrupts::int21::write_open_file(cpu, dos, sft, chunk)
        });
        let n = result.map_err(u32::from)? as usize;
        done += n;
        if n < chunk.len() {
            break;
        }
    }
    Ok(done as u32)
}

/// Move `handle`'s position: from the start, the position or the end
/// (`whence` 0, 1 or 2). The new position, or DOS's error.
fn dos_seek(cpu: &mut Cpu, handle: u32, offset: i64, whence: u32) -> Result<u64, u32> {
    let sft = sft_of(cpu, handle).ok_or(6u32)?;
    if cpu.bus.disk.handle_device(sft).is_some() {
        return Ok(0);
    }
    if whence > 2 {
        return Err(1);
    }
    let base = match whence {
        0 => 0,
        1 => cpu.bus.disk.position(sft).unwrap_or(0) as i64,
        _ => cpu.bus.disk.sft_entry(sft).map_or(0, |e| e.size as i64),
    };
    if base + offset < 0 {
        return Err(131);
    }
    cpu.bus
        .disk
        .seek_file(sft, base + offset, 0)
        .map_err(u32::from)
}

/// The size of the file of `handle`.
fn dos_size(cpu: &mut Cpu, handle: u32) -> Option<u32> {
    let sft = sft_of(cpu, handle)?;
    cpu.bus.disk.sft_entry(sft).map(|e| e.size)
}

/// Close `handle`, and with its last handle its file: a standard one's
/// DOS handle here, a file object's in KERNEL32.
fn dos_close(cpu: &mut Cpu, handle: u32) -> bool {
    let Some(id) = object::resolve(cpu, handle) else {
        return false;
    };
    let last = object::objects(cpu).get(id).is_some_and(|o| o.refs == 1);
    if last && let Some(Kind::Dos { handle: dos }) = object::kind_of(cpu, handle) {
        let psp = cpu.bus.win32.psp;
        let _ = crate::win32::dos(cpu, |cpu| crate::dos_files::close(&mut cpu.bus, psp, dos));
    }
    object::close(cpu, handle)
}

/// _open(path, flags, mode) and _sopen(path, flags, share, mode): a
/// descriptor of the file, or -1 with errno set.
pub fn open(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let name = cstr(cpu, args[0])?;
    let fd = open_path(cpu, &name, args[1]);
    Some(fd.unwrap_or(EOF))
}

pub fn sopen(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let name = cstr(cpu, args[0])?;
    // _SH_DENYRW, _SH_DENYWR, _SH_DENYRD, and _SH_DENYNO.
    let share = match args[2] {
        0x10 => 0,
        0x20 => kernel32::FILE_SHARE_READ,
        0x30 => kernel32::FILE_SHARE_WRITE,
        _ => kernel32::FILE_SHARE_READ | kernel32::FILE_SHARE_WRITE,
    };
    Some(open_shared(cpu, &name, args[1], share).unwrap_or(EOF))
}

/// _creat(path, mode): _open with O_CREAT, O_TRUNC and O_RDWR.
pub fn creat(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let name = cstr(cpu, args[0])?;
    Some(open_path(cpu, &name, O_CREAT | O_TRUNC | O_RDWR).unwrap_or(EOF))
}

/// Open the file `name` as `_open`'s `flags` say.
pub fn open_path(cpu: &mut Cpu, name: &[u8], flags: u32) -> Option<u32> {
    let share = kernel32::FILE_SHARE_READ | kernel32::FILE_SHARE_WRITE;
    open_shared(cpu, name, flags, share)
}

/// Open the file `name` as `_open`'s `flags` say, shared as `share`
/// allows.
fn open_shared(cpu: &mut Cpu, name: &[u8], flags: u32, share: u32) -> Option<u32> {
    if name.is_empty() {
        set_errno(cpu, ENOENT);
        set_doserrno(cpu, 2);
        return None;
    }
    let path = dos_path(name);
    let access = match flags & 3 {
        0 => 0,
        O_WRONLY => 1,
        _ => 2,
    };
    let opened = dos_open(
        cpu,
        &path,
        (access, share),
        flags & O_CREAT != 0,
        flags & O_TRUNC != 0,
        flags & O_CREAT != 0 && flags & O_EXCL != 0,
    );
    let handle = match opened {
        Ok(handle) => handle,
        Err(code) => {
            set_dos_error(cpu, code);
            return None;
        }
    };
    let text = if flags & O_BINARY != 0 {
        false
    } else if flags & O_TEXT != 0 {
        true
    } else {
        cpu.bus.guest_read_32(var_at(V_FMODE)) & O_BINARY == 0
    };
    let device = sft_of(cpu, handle).is_some_and(|sft| cpu.bus.disk.handle_device(sft).is_some());
    let mut fd_flags = 0;
    if text {
        fd_flags |= FTEXT;
    }
    if flags & O_APPEND != 0 {
        fd_flags |= FAPPEND;
    }
    if device {
        fd_flags |= FDEV;
    }
    if flags & O_NOINHERIT != 0 {
        fd_flags |= FNOINHERIT;
    }
    let _ = flags & O_TEMPORARY;
    match new_fd(cpu, handle, fd_flags, 0) {
        Some(fd) => Some(fd),
        None => {
            dos_close(cpu, handle);
            None
        }
    }
}

/// _close(fd): 0, or -1.
pub fn close(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(close_fd(cpu, args[0]).map_or(EOF, |_| 0))
}

pub fn close_fd(cpu: &mut Cpu, fd: u32) -> Option<()> {
    let (handle, _) = fd_info(cpu, fd)?;
    set_flags(cpu, fd, 0);
    cpu.bus.guest_write_32(entry(fd), u32::MAX);
    if !dos_close(cpu, handle) {
        set_errno(cpu, EBADF);
        return None;
    }
    Some(())
}

/// What `read_fd` gives.
pub enum FdRead {
    /// Bytes, none at the end of the file.
    Data(Vec<u8>),
    /// -1, with errno set.
    Error,
    /// The console has no line yet: the caller waits (`wait_for_console`).
    Wait,
}

/// Read up to `count` bytes from `fd`, in text mode with CR LF made LF and
/// a Ctrl-Z the end.
pub fn read_fd(cpu: &mut Cpu, fd: u32, count: u32) -> FdRead {
    let Some((handle, flags)) = fd_info(cpu, fd) else {
        return FdRead::Error;
    };
    if count == 0 || flags & FEOFLAG != 0 {
        return FdRead::Data(Vec::new());
    }
    let raw = match dos_read(cpu, handle, count) {
        Read::Data(data) => data,
        Read::Wait => return FdRead::Wait,
        Read::Error(code) => {
            set_dos_error(cpu, code);
            // Reading a file opened for writing only is EBADF.
            if code == 5 {
                set_errno(cpu, EBADF);
            }
            return FdRead::Error;
        }
    };
    if flags & FTEXT == 0 {
        return FdRead::Data(raw);
    }
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        match raw[i] {
            0x1A => {
                // The end, though the file's position stays past what
                // was read after it, as MSVCRT leaves it.
                set_flags(cpu, fd, flags | FEOFLAG);
                break;
            }
            b'\r' if i + 1 < raw.len() => {
                if raw[i + 1] == b'\n' {
                    out.push(b'\n');
                    i += 2;
                } else {
                    out.push(b'\r');
                    i += 1;
                }
                continue;
            }
            b'\r' => {
                // A CR at the end: the next byte says what it is.
                let next = match flags & FDEV {
                    0 => match dos_read(cpu, handle, 1) {
                        Read::Data(data) => data.first().copied(),
                        _ => None,
                    },
                    _ => None,
                };
                match next {
                    // The LF is read next time, unless it is all there is.
                    Some(b'\n') if out.is_empty() => out.push(b'\n'),
                    Some(b'\n') => {
                        let _ = dos_seek(cpu, handle, -1, 1);
                    }
                    Some(_) => {
                        out.push(b'\r');
                        let _ = dos_seek(cpu, handle, -1, 1);
                    }
                    None => out.push(b'\r'),
                }
            }
            byte => out.push(byte),
        }
        i += 1;
    }
    FdRead::Data(out)
}

/// The function running waits for a key and runs again once it comes:
/// its trap again, with the stack as it was.
pub fn wait_for_console(cpu: &mut Cpu) -> Option<u32> {
    cpu.set_eip(cpu.eip().wrapping_sub(4));
    cpu.key_wait();
    None
}

/// _read(fd, buffer, count): the bytes read, 0 at the end, -1 on errors.
pub fn read(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[fd, buffer, count] = args else {
        unreachable!()
    };
    if !cpu.bus.guest_probe(buffer, count as usize, true) {
        return None;
    }
    match read_fd(cpu, fd, count) {
        FdRead::Data(data) => {
            put(cpu, buffer, &data)?;
            Some(data.len() as u32)
        }
        FdRead::Error => Some(EOF),
        FdRead::Wait => wait_for_console(cpu),
    }
}

/// Write `data` to `fd`, in text mode with LF made CR LF: how many of
/// `data`'s bytes went, or None with errno set.
pub fn write_fd(cpu: &mut Cpu, fd: u32, data: &[u8]) -> Option<u32> {
    let (handle, flags) = fd_info(cpu, fd)?;
    if flags & FAPPEND != 0 {
        let _ = dos_seek(cpu, handle, 0, 2);
    }
    if data.is_empty() {
        return Some(0);
    }
    let raw: Vec<u8> = if flags & FTEXT != 0 {
        let mut out = Vec::with_capacity(data.len() + 16);
        for &b in data {
            if b == b'\n' {
                out.push(b'\r');
            }
            out.push(b);
        }
        out
    } else {
        data.to_vec()
    };
    match dos_write(cpu, handle, &raw) {
        Ok(n) if n as usize == raw.len() => Some(data.len() as u32),
        Ok(n) => {
            // A full disk.
            if n == 0 {
                set_errno(cpu, ENOSPC);
                set_doserrno(cpu, 0);
                return None;
            }
            let lfs = raw[..n as usize].iter().filter(|&&b| b == b'\r').count();
            Some(n - if flags & FTEXT != 0 { lfs as u32 } else { 0 })
        }
        Err(code) => {
            set_dos_error(cpu, code);
            if code == 5 {
                set_errno(cpu, EBADF);
            }
            None
        }
    }
}

/// _write(fd, buffer, count): the bytes written, or -1.
pub fn write(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let data = bytes(cpu, args[1], args[2])?;
    Some(write_fd(cpu, args[0], &data).unwrap_or(EOF))
}

/// Move `fd`'s position: the new one, or None with errno set.
pub fn seek_fd(cpu: &mut Cpu, fd: u32, offset: i64, whence: u32) -> Option<u64> {
    let (handle, flags) = fd_info(cpu, fd)?;
    match dos_seek(cpu, handle, offset, whence) {
        Ok(at) => {
            set_flags(cpu, fd, flags & !FEOFLAG);
            Some(at)
        }
        Err(code) => {
            set_dos_error(cpu, code);
            if code == 1 || code == 131 {
                set_errno(cpu, EINVAL);
            }
            None
        }
    }
}

/// _lseek(fd, offset, whence): the new position, or -1.
pub fn lseek(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = seek_fd(cpu, args[0], args[1] as i32 as i64, args[2]);
    Some(at.map_or(EOF, |at| at as u32))
}

/// _lseeki64(fd, offset, whence), the offset 64 bits.
pub fn lseeki64(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let offset = ((args[2] as u64) << 32 | args[1] as u64) as i64;
    let at = seek_fd(cpu, args[0], offset, args[3]);
    ret_u64(cpu, at.unwrap_or(u64::MAX))
}

/// _tell(fd): the position.
pub fn tell(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = seek_fd(cpu, args[0], 0, 1);
    Some(at.map_or(EOF, |at| at as u32))
}

pub fn telli64(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = seek_fd(cpu, args[0], 0, 1);
    ret_u64(cpu, at.unwrap_or(u64::MAX))
}

/// _eof(fd): 1 at the end of the file, 0 before it, -1 on errors.
pub fn eof(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((handle, flags)) = fd_info(cpu, args[0]) else {
        return Some(EOF);
    };
    if flags & FEOFLAG != 0 {
        return Some(1);
    }
    let Ok(at) = dos_seek(cpu, handle, 0, 1) else {
        return Some(EOF);
    };
    let size = dos_size(cpu, handle).unwrap_or(0) as u64;
    Some((at >= size) as u32)
}

/// _filelength(fd): the file's size, or -1.
pub fn filelength(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((handle, _)) = fd_info(cpu, args[0]) else {
        return Some(EOF);
    };
    Some(dos_size(cpu, handle).unwrap_or(EOF))
}

/// _chsize(fd, size): the file cut or grown (with zeros) to `size`
/// bytes, its position kept. 0, or -1.
pub fn chsize(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((handle, _)) = fd_info(cpu, args[0]) else {
        return Some(EOF);
    };
    let Some(sft) = sft_of(cpu, handle) else {
        set_errno(cpu, EBADF);
        return Some(EOF);
    };
    match cpu.bus.disk.set_file_size(sft, args[1] as u64) {
        Ok(()) => Some(0),
        Err(code) => {
            set_dos_error(cpu, code as u32);
            Some(EOF)
        }
    }
}

/// _commit(fd): the file's data is on the disk already.
pub fn commit(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(fd_info(cpu, args[0]).map_or(EOF, |_| 0))
}

/// _isatty(fd): whether it is a device.
pub fn isatty(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    match fd_info(cpu, args[0]) {
        Some((_, flags)) => Some((flags & FDEV != 0) as u32 * FDEV),
        None => Some(0),
    }
}

/// _setmode(fd, mode): text or binary; the mode before.
pub fn setmode(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[fd, mode] = args else { unreachable!() };
    let Some((_, flags)) = fd_info(cpu, fd) else {
        return Some(EOF);
    };
    let old = if flags & FTEXT != 0 { O_TEXT } else { O_BINARY };
    match mode {
        O_TEXT => set_flags(cpu, fd, flags | FTEXT),
        O_BINARY => set_flags(cpu, fd, flags & !FTEXT),
        _ => {
            set_errno(cpu, EINVAL);
            return Some(EOF);
        }
    }
    Some(old)
}

/// _get_osfhandle(fd): its handle, or -1.
pub fn get_osfhandle(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(fd_info(cpu, args[0]).map_or(EOF, |(handle, _)| handle))
}

/// _open_osfhandle(handle, flags): a descriptor for the handle.
pub fn open_osfhandle(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, flags] = args else {
        unreachable!()
    };
    let mut fd_flags = if flags & O_TEXT != 0 { FTEXT } else { 0 };
    if flags & O_APPEND != 0 {
        fd_flags |= FAPPEND;
    }
    if sft_of(cpu, handle).is_some_and(|sft| cpu.bus.disk.handle_device(sft).is_some()) {
        fd_flags |= FDEV;
    }
    Some(new_fd(cpu, handle, fd_flags, 0).unwrap_or(EOF))
}

/// A second handle of the object `handle`, as DuplicateHandle makes.
fn duplicate_handle(cpu: &mut Cpu, handle: u32) -> Option<u32> {
    let id = object::resolve(cpu, handle)?;
    object::objects(cpu).hold(id);
    Some(object::new_handle(cpu, id))
}

/// _dup(fd): another descriptor of its file, sharing its position.
pub fn dup(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((handle, flags)) = fd_info(cpu, args[0]) else {
        return Some(EOF);
    };
    let Some(copy) = duplicate_handle(cpu, handle) else {
        set_errno(cpu, EBADF);
        return Some(EOF);
    };
    match new_fd(cpu, copy, flags & !FOPEN, 0) {
        Some(fd) => Some(fd),
        None => {
            dos_close(cpu, copy);
            Some(EOF)
        }
    }
}

/// _dup2(fd, fd2): fd2 made another descriptor of fd's file, closed
/// first if it was open. 0, or -1.
pub fn dup2(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[fd, fd2] = args else { unreachable!() };
    let Some((handle, flags)) = fd_info(cpu, fd) else {
        return Some(EOF);
    };
    if fd2 >= MAX_FDS {
        set_errno(cpu, EBADF);
        return Some(EOF);
    }
    if fd == fd2 {
        return Some(0);
    }
    let Some(copy) = duplicate_handle(cpu, handle) else {
        set_errno(cpu, EBADF);
        return Some(EOF);
    };
    if cpu.bus.guest_read_32(entry(fd2) + 4) & FOPEN != 0 {
        let _ = close_fd(cpu, fd2);
    }
    cpu.bus.guest_write_32(entry(fd2), copy);
    cpu.bus.guest_write_32(entry(fd2) + 4, flags);
    Some(0)
}

/// _locking(fd, mode, bytes): there is one program at a time on DOS's
/// files, so the region is the caller's.
pub fn locking(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(fd_info(cpu, args[0]).map_or(EOF, |_| 0))
}

/// _umask(mode): the mask before.
pub fn umask(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let old = state(cpu, S_UMASK);
    set_state(cpu, S_UMASK, args[0] & (S_IREAD | S_IWRITE));
    Some(old)
}

/// _pipe: there are no pipes on DOS.
pub fn pipe(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    set_errno(cpu, EMFILE);
    Some(EOF)
}

/// The entry of the file or directory `path`, as a directory search
/// finds it; a drive's root is a directory.
fn find_entry(cpu: &mut Cpu, path: &str) -> Option<DosDirEntry> {
    if path.contains(['*', '?']) {
        return None;
    }
    let trimmed = path.trim_end_matches('\\');
    let directory = DosDirEntry {
        filename: String::new(),
        size: 0,
        is_dir: true,
        is_readonly: false,
        dos_time: 0,
        dos_date: 0x21,
        attr: 0x10,
    };
    if trimmed.is_empty() || (trimmed.len() == 2 && trimmed.ends_with(':')) {
        return Some(directory);
    }
    let disk = &cpu.bus.disk;
    let found = disk
        .list_directory(trimmed, 0x16)
        .ok()
        .and_then(|entries| entries.into_iter().next());
    if disk.is_directory(trimmed) {
        return Some(found.filter(|e| e.is_dir).unwrap_or(directory));
    }
    found
}

/// The drive a path is on, 0 for A:.
fn drive_of(cpu: &Cpu, path: &str) -> u32 {
    let disk = &cpu.bus.disk;
    disk.drive_of(path).unwrap_or(disk.get_current_drive()) as u32
}

/// Fill a `struct _stat` at `at` for `entry`, on `drive`.
fn write_stat(cpu: &mut Cpu, at: u32, entry: &DosDirEntry, drive: u32, name: &str) -> Option<()> {
    let mut mode = if entry.is_dir {
        S_IFDIR | S_IEXEC
    } else {
        S_IFREG
    };
    mode |= S_IREAD;
    if !entry.is_readonly && entry.attr & 1 == 0 {
        mode |= S_IWRITE;
    }
    let upper = name.to_ascii_uppercase();
    if [".EXE", ".COM", ".BAT", ".CMD"]
        .iter()
        .any(|e| upper.ends_with(e))
    {
        mode |= S_IEXEC;
    }
    mode |= (mode & 0x1C0) >> 3 | (mode & 0x1C0) >> 6;
    let time = super::time::dos_to_time_t(entry.dos_date, entry.dos_time);
    let mut out = [0u8; 36];
    out[0..4].copy_from_slice(&drive.to_le_bytes());
    out[6..8].copy_from_slice(&(mode as u16).to_le_bytes());
    out[8..10].copy_from_slice(&1u16.to_le_bytes());
    out[16..20].copy_from_slice(&drive.to_le_bytes());
    out[20..24].copy_from_slice(&entry.size.to_le_bytes());
    for field in [24, 28, 32] {
        out[field..field + 4].copy_from_slice(&time.to_le_bytes());
    }
    put(cpu, at, &out)
}

/// _stat(path, buffer): 0, or -1 with errno ENOENT.
pub fn stat(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let name = cstr(cpu, args[0])?;
    let path = dos_path(&name);
    let Some(entry) = find_entry(cpu, &path) else {
        set_errno(cpu, ENOENT);
        set_doserrno(cpu, 2);
        return Some(EOF);
    };
    let drive = drive_of(cpu, &path);
    write_stat(cpu, args[1], &entry, drive, &path)?;
    Some(0)
}

/// _fstat(fd, buffer): what its open file says.
pub fn fstat(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((handle, flags)) = fd_info(cpu, args[0]) else {
        return Some(EOF);
    };
    if !cpu.bus.guest_probe(args[1], 36, true) {
        return None;
    }
    let sft = sft_of(cpu, handle);
    let info = sft.and_then(|sft| cpu.bus.disk.sft_entry(sft));
    let mut out = [0u8; 36];
    match info {
        Some(info) if flags & FDEV == 0 => {
            let mode = S_IFREG | S_IREAD | S_IWRITE;
            let mode = mode | (mode & 0x1C0) >> 3 | (mode & 0x1C0) >> 6;
            let time = super::time::dos_to_time_t(info.date, info.time);
            out[6..8].copy_from_slice(&(mode as u16).to_le_bytes());
            out[8..10].copy_from_slice(&1u16.to_le_bytes());
            out[20..24].copy_from_slice(&info.size.to_le_bytes());
            for field in [24, 28, 32] {
                out[field..field + 4].copy_from_slice(&time.to_le_bytes());
            }
        }
        _ => {
            // A device's: its descriptor, as st_dev and st_rdev.
            let mode = S_IFCHR;
            out[0..4].copy_from_slice(&args[0].to_le_bytes());
            out[6..8].copy_from_slice(&(mode as u16).to_le_bytes());
            out[8..10].copy_from_slice(&1u16.to_le_bytes());
            out[16..20].copy_from_slice(&args[0].to_le_bytes());
        }
    }
    put(cpu, args[1], &out)?;
    Some(0)
}

/// _access(path, mode): 0 if the file is there and, for mode 2 or 6,
/// writable; else -1 with errno ENOENT or EACCES.
pub fn access(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let name = cstr(cpu, args[0])?;
    let path = dos_path(&name);
    let Some(entry) = find_entry(cpu, &path) else {
        set_errno(cpu, ENOENT);
        set_doserrno(cpu, 2);
        return Some(EOF);
    };
    if args[1] & !6 != 0 {
        set_errno(cpu, EINVAL);
        return Some(EOF);
    }
    if args[1] & 2 != 0 && entry.attr & 1 != 0 {
        set_errno(cpu, EACCES);
        set_doserrno(cpu, 5);
        return Some(EOF);
    }
    Some(0)
}

/// _chmod(path, mode): read only unless `mode` has _S_IWRITE.
pub fn chmod(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let name = cstr(cpu, args[0])?;
    let path = dos_path(&name);
    let disk = &cpu.bus.disk;
    let result = disk.get_file_attribute(&path).and_then(|attr| {
        let attr = if args[1] & S_IWRITE != 0 {
            attr & !1
        } else {
            attr | 1
        };
        disk.set_file_attribute(&path, attr)
    });
    match result {
        Ok(()) => Some(0),
        Err(code) => {
            set_dos_error(cpu, code as u32);
            Some(EOF)
        }
    }
}

/// The result of a DOS call on a path: 0, or -1 with errno set.
fn dos_result(cpu: &mut Cpu, result: Result<(), u8>) -> Option<u32> {
    match result {
        Ok(()) => Some(0),
        Err(code) => {
            set_dos_error(cpu, code as u32);
            Some(EOF)
        }
    }
}

/// _unlink(path) and remove(path).
pub fn unlink(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let name = cstr(cpu, args[0])?;
    let path = dos_path(&name);
    if cpu.bus.disk.is_directory(&path) {
        return dos_result(cpu, Err(5));
    }
    let result = cpu.bus.disk.delete_file(&path);
    dos_result(cpu, result)
}

/// rename(old, new).
pub fn rename(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let from = dos_path(&cstr(cpu, args[0])?);
    let to = dos_path(&cstr(cpu, args[1])?);
    if cpu.bus.disk.exists(&to) {
        return dos_result(cpu, Err(5));
    }
    let result = cpu.bus.disk.rename_file(&from, &to);
    dos_result(cpu, result)
}

/// _mkdir(path).
pub fn mkdir(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let path = dos_path(&cstr(cpu, args[0])?);
    if cpu.bus.disk.exists(&path) {
        set_errno(cpu, EEXIST);
        set_doserrno(cpu, 183);
        return Some(EOF);
    }
    let result = cpu.bus.disk.create_directory(&path);
    dos_result(cpu, result)
}

/// _rmdir(path).
pub fn rmdir(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let path = dos_path(&cstr(cpu, args[0])?);
    let result = cpu.bus.disk.remove_directory(&path);
    if result == Err(5) && cpu.bus.disk.is_directory(&path) {
        set_errno(cpu, ENOTEMPTY);
        return Some(EOF);
    }
    dos_result(cpu, result)
}

/// _chdir(path): the current directory of its drive, which becomes the
/// current drive too, as SetCurrentDirectory makes it.
pub fn chdir(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let name = crate::dosstr::from_bytes(&cstr(cpu, args[0])?);
    if crate::win32::kernel32::change_directory(cpu, &name).is_err() {
        set_errno(cpu, ENOENT);
        set_doserrno(cpu, 3);
        return Some(EOF);
    }
    Some(0)
}

/// The current directory of `drive` (0 for A:), as "C:\DIR", with the
/// long names SetCurrentDirectory or _chdir were given.
fn current_directory(cpu: &Cpu, drive: u8) -> Option<String> {
    cpu.bus.disk.get_current_directory_of(drive)?;
    Some(crate::win32::kernel32::current_directory(cpu, drive))
}

/// Copy `text` into the buffer `at` of `size` bytes, or with no buffer
/// into a block of malloc's at least that big: its address, or 0 with
/// errno ERANGE when it doesn't fit.
fn give_string(cpu: &mut Cpu, text: &[u8], at: u32, size: u32) -> Option<u32> {
    if at == 0 {
        let len = (text.len() as u32 + 1).max(size);
        let Some(block) = heap_alloc(cpu, len) else {
            set_errno(cpu, ENOMEM);
            return Some(0);
        };
        put_cstr(cpu, block, text)?;
        return Some(block);
    }
    if text.len() as u32 >= size {
        set_errno(cpu, ERANGE);
        return Some(0);
    }
    put_cstr(cpu, at, text)?;
    Some(at)
}

/// _getcwd(buffer, size).
pub fn getcwd(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let drive = cpu.bus.disk.get_current_drive();
    let dir = current_directory(cpu, drive).unwrap_or_default();
    give_string(cpu, dir.as_bytes(), args[0], args[1])
}

/// _getdcwd(drive, buffer, size): 1 is A:, 0 the current drive.
pub fn getdcwd(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let drive = match args[0] {
        0 => cpu.bus.disk.get_current_drive(),
        n => (n - 1) as u8,
    };
    let Some(dir) = current_directory(cpu, drive) else {
        set_errno(cpu, EACCES);
        set_doserrno(cpu, 15);
        return Some(0);
    };
    give_string(cpu, dir.as_bytes(), args[1], args[2])
}

/// _getdrive(): the current drive, 1 for A:.
pub fn getdrive(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(cpu.bus.disk.get_current_drive() as u32 + 1)
}

/// _chdrive(drive): 0, or -1 for a drive that isn't there.
pub fn chdrive(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let drive = args[0].wrapping_sub(1);
    if drive >= 26 || !cpu.bus.disk.is_mounted(drive as u8) {
        set_errno(cpu, EACCES);
        set_doserrno(cpu, 15);
        return Some(EOF);
    }
    cpu.bus.disk.set_current_drive(drive as u8);
    Some(0)
}

/// _fullpath(buffer, path, size): the path made absolute.
pub fn fullpath(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[buffer, path, size] = args else {
        unreachable!()
    };
    let name = if path == 0 {
        Vec::new()
    } else {
        cstr(cpu, path)?
    };
    // The current directory, or the path made absolute from it as
    // GetFullPathName makes it, in the case it was written in.
    let drive = match name.as_slice() {
        [d, b':', ..] if d.is_ascii_alphabetic() => d.to_ascii_uppercase() - b'A',
        _ => cpu.bus.disk.get_current_drive(),
    };
    let Some(current) = current_directory(cpu, drive) else {
        set_errno(cpu, ENOENT);
        return Some(0);
    };
    let full = if name.is_empty() {
        current
    } else {
        crate::win32::kernel32::full_path(cpu, &dos_path(&name))
    };
    let size = if buffer == 0 { 260 } else { size };
    give_string(cpu, &crate::dosstr::to_bytes(&full), buffer, size)
}

/// `_finddata_t`'s size, as the search's results are written.
const FINDDATA: u32 = 280;

/// Write the entry `entry` as a `_finddata_t` at `at`.
fn write_finddata(cpu: &mut Cpu, at: u32, entry: &DosDirEntry) -> Option<()> {
    let mut out = vec![0u8; FINDDATA as usize];
    let time = super::time::dos_to_time_t(entry.dos_date, entry.dos_time);
    out[0..4].copy_from_slice(&(entry.attr as u32).to_le_bytes());
    for field in [4, 8, 12] {
        out[field..field + 4].copy_from_slice(&time.to_le_bytes());
    }
    out[16..20].copy_from_slice(&entry.size.to_le_bytes());
    let name = crate::dosstr::to_bytes(&entry.filename);
    let len = name.len().min(259);
    out[20..20 + len].copy_from_slice(&name[..len]);
    put(cpu, at, &out)
}

/// The entries a search's spec finds, "." and ".." first in a
/// directory that isn't a root, as Windows lists them.
fn search(cpu: &mut Cpu, spec: &str) -> Vec<DosDirEntry> {
    cpu.bus.disk.list_directory(spec, 0x16).unwrap_or_default()
}

/// _findfirst(spec, data): a search's handle (a heap block with the
/// entry it is at and the spec), the first entry in `data`; -1, errno
/// ENOENT, when nothing matches.
pub fn findfirst(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let name = cstr(cpu, args[0])?;
    let spec = dos_path(&name);
    let entries = search(cpu, &spec);
    let Some(first) = entries.first() else {
        set_errno(cpu, ENOENT);
        set_doserrno(cpu, 2);
        return Some(EOF);
    };
    if !cpu.bus.guest_probe(args[1], FINDDATA as usize, true) {
        return None;
    }
    let Some(handle) = heap_alloc(cpu, 8 + name.len() as u32 + 1) else {
        set_errno(cpu, ENOMEM);
        return Some(EOF);
    };
    write32(cpu, handle, 1)?;
    write32(cpu, handle + 4, 0x4644_4E46)?;
    put_cstr(cpu, handle + 8, &name)?;
    write_finddata(cpu, args[1], first)?;
    Some(handle)
}

/// Whether `handle` is a search `findfirst` made.
fn is_search(cpu: &mut Cpu, handle: u32) -> bool {
    handle != EOF
        && heap_size(cpu, handle).is_some()
        && cpu.bus.guest_read_32(handle + 4) == 0x4644_4E46
}

/// _findnext(handle, data): the next entry; -1, errno ENOENT, past the
/// last.
pub fn findnext(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let handle = args[0];
    if !is_search(cpu, handle) {
        set_errno(cpu, EINVAL);
        return Some(EOF);
    }
    let index = read32(cpu, handle)?;
    let name = cstr(cpu, handle + 8)?;
    let entries = search(cpu, &dos_path(&name));
    let Some(entry) = entries.get(index as usize) else {
        set_errno(cpu, ENOENT);
        set_doserrno(cpu, 18);
        return Some(EOF);
    };
    write_finddata(cpu, args[1], entry)?;
    write32(cpu, handle, index + 1)?;
    Some(0)
}

/// _findclose(handle).
pub fn findclose(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if !is_search(cpu, args[0]) {
        set_errno(cpu, EINVAL);
        return Some(EOF);
    }
    write32(cpu, args[0] + 4, 0)?;
    heap_free(cpu, args[0]);
    Some(0)
}

/// A name for a temporary file in `dir` that no file has, with `prefix`
/// (of at most 5 characters for an 8.3 name).
fn temp_name(cpu: &mut Cpu, dir: &str, prefix: &str) -> String {
    let prefix: String = prefix.chars().take(5).collect();
    loop {
        let n = state(cpu, S_TEMP_COUNT).wrapping_add(1);
        set_state(cpu, S_TEMP_COUNT, n);
        let name = if dir.is_empty() || dir.ends_with('\\') {
            format!("{}{}{}.", dir, prefix, base32(n))
        } else {
            format!("{}\\{}{}.", dir, prefix, base32(n))
        };
        let name = name.trim_end_matches('.').to_string() + ".";
        if !cpu.bus.disk.exists(name.trim_end_matches('.')) {
            return name;
        }
    }
}

fn base32(mut n: u32) -> String {
    let digits = b"0123456789abcdefghijklmnopqrstuv";
    let mut out = Vec::new();
    loop {
        out.push(digits[(n % 32) as usize]);
        n /= 32;
        if n == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).unwrap()
}

/// tmpnam(buffer): a file in the root that isn't there, in `buffer` or
/// the thread's own: "\s", the process id and a dot, as MSVCRT names
/// it, with a number after the dot from the second call on.
pub fn tmpnam(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let base = format!("\\s{}.",base32(super::super::process(cpu).id));
    let name = loop {
        let n = state(cpu, S_TMPNAM_COUNT);
        set_state(cpu, S_TMPNAM_COUNT, n.wrapping_add(1));
        let name = match n {
            0 => base.clone(),
            n => format!("{}{}", base, base32(n)),
        };
        if !cpu.bus.disk.exists(name.trim_end_matches('.')) {
            break name;
        }
    };
    let at = match args[0] {
        0 => thread_field(cpu, T_TMPNAM)?,
        at => at,
    };
    put_cstr(cpu, at, name.as_bytes())?;
    Some(at)
}

/// _tempnam(dir, prefix): a name in TMP's directory, or `dir`, in a
/// block of malloc's.
pub fn tempnam(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let dir = if args[0] == 0 {
        String::new()
    } else {
        dos_path(&cstr(cpu, args[0])?)
    };
    let prefix = if args[1] == 0 {
        String::new()
    } else {
        crate::dosstr::from_bytes(&cstr(cpu, args[1])?)
    };
    let tmp = super::startup::getenv_value(cpu, "TMP");
    let dir = match tmp {
        Some(tmp) if cpu.bus.disk.is_directory(&tmp) => tmp,
        _ if !dir.is_empty() && cpu.bus.disk.is_directory(&dir) => dir,
        _ => "\\".to_string(),
    };
    let name = temp_name(cpu, &dir, &prefix);
    let name = name.trim_end_matches('.');
    give_string(cpu, name.as_bytes(), 0, 0)
}

/// _mktemp(template): the "XXXXXX" at its end made a letter and the
/// process's number, the first letter that gives a name no file has.
pub fn mktemp(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let template = cstr(cpu, args[0])?;
    if template.len() < 6 || !template.ends_with(b"XXXXXX") {
        set_errno(cpu, EINVAL);
        return Some(0);
    }
    let id = super::super::process(cpu).id;
    let digits = format!("{:05}", id % 100_000);
    let mut name = template.clone();
    let at = name.len() - 6;
    name[at + 1..].copy_from_slice(digits.as_bytes());
    for letter in b'a'..=b'z' {
        name[at] = letter;
        if !cpu.bus.disk.exists(&dos_path(&name)) {
            put(cpu, args[0], &name)?;
            return Some(args[0]);
        }
    }
    Some(0)
}
