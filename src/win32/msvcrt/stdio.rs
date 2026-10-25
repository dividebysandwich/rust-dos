//! Streams: FILE structures in the program's memory, laid out as MSVC's
//! `_iobuf`, since programs' getc and putc macros work on their `_ptr`
//! and `_cnt` themselves and call `_filbuf` and `_flsbuf` when the buffer
//! runs out. The buffers are on the process heap; a stream on a device
//! (the console) has none and writes at once. The printf and scanf
//! families are here too, on format.rs.

use super::format::{self, Args, Input};
use super::io::{self, FdRead};
use super::*;

/// `_iobuf`'s fields that are used by address: `_ptr`, `_cnt` and
/// `_base` come before them, `_bufsiz` after (`File`).
pub const F_FLAG: u32 = 12;
pub const F_FILE: u32 = 16;
pub const F_CHARBUF: u32 = 20;
pub const F_TMPFNAME: u32 = 28;

/// `_flag`'s bits.
pub const IOREAD: u32 = 0x0001;
pub const IOWRT: u32 = 0x0002;
const IONBF: u32 = 0x0004;
const IOMYBUF: u32 = 0x0008;
const IOEOF: u32 = 0x0010;
const IOERR: u32 = 0x0020;
const IOSTRG: u32 = 0x0040;
const IORW: u32 = 0x0080;
const IOYOURBUF: u32 = 0x0100;

/// The size of the buffers streams get.
const BUFSIZ: u32 = 0x1000;
const EOF: u32 = u32::MAX;

/// A FILE as it is in memory.
#[derive(Clone, Copy, Default)]
struct File {
    ptr: u32,
    cnt: i32,
    base: u32,
    flag: u32,
    file: u32,
    charbuf: u32,
    bufsiz: u32,
    tmpfname: u32,
}

fn load(cpu: &mut Cpu, at: u32) -> Option<File> {
    let raw = bytes(cpu, at, FILE_SIZE)?;
    let w = |i: usize| u32::from_le_bytes(raw[4 * i..4 * i + 4].try_into().unwrap());
    Some(File {
        ptr: w(0),
        cnt: w(1) as i32,
        base: w(2),
        flag: w(3),
        file: w(4),
        charbuf: w(5),
        bufsiz: w(6),
        tmpfname: w(7),
    })
}

fn store(cpu: &mut Cpu, at: u32, f: &File) -> Option<()> {
    let words = [
        f.ptr,
        f.cnt as u32,
        f.base,
        f.flag,
        f.file,
        f.charbuf,
        f.bufsiz,
        f.tmpfname,
    ];
    let raw: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    put(cpu, at, &raw)
}

/// The stream of `_iob[index]`: 0 stdin, 1 stdout, 2 stderr.
pub fn iob(index: u32) -> u32 {
    var_at(V_IOB) + FILE_SIZE * index
}

/// Whether the descriptor `fd` is a device, which its stream doesn't
/// buffer writes for.
fn is_device(cpu: &mut Cpu, fd: u32) -> bool {
    let entry = var_at(V_FDS) + FD_SIZE * fd;
    fd < MAX_FDS && cpu.bus.guest_read_32(entry + 4) & io::FDEV != 0
}

/// Give the stream a buffer of its own, if it has none: one from the heap,
/// or for an unbuffered stream its `_charbuf`.
fn get_buffer(cpu: &mut Cpu, at: u32, f: &mut File) -> Option<()> {
    if f.base != 0 {
        return Some(());
    }
    if f.flag & IONBF == 0
        && let Some(buffer) = heap_alloc(cpu, BUFSIZ)
    {
        f.base = buffer;
        f.bufsiz = BUFSIZ;
        f.flag |= IOMYBUF;
    } else {
        f.flag |= IONBF;
        f.base = at + F_CHARBUF;
        f.bufsiz = 1;
    }
    f.ptr = f.base;
    f.cnt = 0;
    Some(())
}

/// What reading a character gives.
pub enum Got {
    Byte(u8),
    End,
    Fault,
    /// The console has no line yet.
    Wait,
}

/// Fill the stream's buffer from its file: its first byte, taken.
fn fill(cpu: &mut Cpu, at: u32) -> Got {
    let Some(mut f) = load(cpu, at) else {
        return Got::Fault;
    };
    if f.flag & (IOREAD | IORW) == 0 || f.flag & IOSTRG != 0 {
        f.flag |= IOERR;
        return store(cpu, at, &f).map_or(Got::Fault, |_| Got::End);
    }
    if f.flag & IOWRT != 0 {
        // A read straight after a write needs a seek or flush between.
        if f.flag & IORW == 0 || f.ptr != f.base {
            f.flag |= IOERR;
            return store(cpu, at, &f).map_or(Got::Fault, |_| Got::End);
        }
        f.flag &= !IOWRT;
    }
    f.flag |= IOREAD;
    if get_buffer(cpu, at, &mut f).is_none() {
        return Got::Fault;
    }
    // A device's stream reads what there is, a line from the console.
    let data = match io::read_fd(cpu, f.file, f.bufsiz) {
        FdRead::Data(data) => data,
        FdRead::Error => {
            f.flag |= IOERR;
            f.cnt = 0;
            return store(cpu, at, &f).map_or(Got::Fault, |_| Got::End);
        }
        FdRead::Wait => return Got::Wait,
    };
    if data.is_empty() {
        f.flag |= IOEOF;
        f.cnt = 0;
        f.ptr = f.base;
        return store(cpu, at, &f).map_or(Got::Fault, |_| Got::End);
    }
    if put(cpu, f.base, &data).is_none() {
        return Got::Fault;
    }
    f.ptr = f.base + 1;
    f.cnt = data.len() as i32 - 1;
    if store(cpu, at, &f).is_none() {
        return Got::Fault;
    }
    Got::Byte(data[0])
}

/// Take a byte from the stream.
pub fn getc(cpu: &mut Cpu, at: u32) -> Got {
    let Some(mut f) = load(cpu, at) else {
        return Got::Fault;
    };
    if f.cnt > 0 && f.flag & IOREAD != 0 {
        let byte = cpu.bus.guest_read_8(f.ptr);
        if cpu.bus.guest_faulted() {
            return Got::Fault;
        }
        f.ptr += 1;
        f.cnt -= 1;
        return store(cpu, at, &f).map_or(Got::Fault, |_| Got::Byte(byte));
    }
    fill(cpu, at)
}

/// Put back `byte` so the next read takes it. False if it can't be.
pub fn ungetc_byte(cpu: &mut Cpu, at: u32, byte: u8) -> bool {
    let Some(mut f) = load(cpu, at) else {
        return false;
    };
    if f.flag & IOREAD == 0 && !(f.flag & IORW != 0 && f.flag & IOWRT == 0) {
        return false;
    }
    if f.base == 0 && get_buffer(cpu, at, &mut f).is_none() {
        return false;
    }
    if f.ptr == f.base {
        if f.cnt != 0 {
            return false;
        }
        f.ptr += 1;
    }
    f.ptr -= 1;
    f.cnt += 1;
    f.flag &= !IOEOF;
    f.flag |= IOREAD;
    cpu.bus.guest_write_8(f.ptr, byte);
    store(cpu, at, &f).is_some()
}

/// Write out what the stream's buffer has. False on an error.
fn flush(cpu: &mut Cpu, at: u32) -> Option<bool> {
    let mut f = load(cpu, at)?;
    let mut ok = true;
    if f.flag & (IOREAD | IOWRT) == IOWRT && f.base != 0 && f.ptr > f.base {
        let data = bytes(cpu, f.base, f.ptr - f.base)?;
        match io::write_fd(cpu, f.file, &data) {
            Some(n) if n as usize == data.len() => {}
            _ => {
                f.flag |= IOERR;
                ok = false;
            }
        }
    }
    if f.flag & IORW != 0 {
        f.flag &= !(IOWRT | IOREAD);
    }
    f.ptr = f.base;
    f.cnt = 0;
    store(cpu, at, &f)?;
    Some(ok)
}

/// Write `data` to the stream: how many bytes went.
fn write_stream(cpu: &mut Cpu, at: u32, data: &[u8]) -> Option<usize> {
    let mut f = load(cpu, at)?;
    if f.flag & IOREAD != 0 && f.flag & IORW == 0 || f.flag & (IOWRT | IORW) == 0 {
        f.flag |= IOERR;
        store(cpu, at, &f)?;
        return Some(0);
    }
    if f.flag & IOREAD != 0 {
        // Reading switches to writing at the end of the file only.
        if f.flag & IOEOF == 0 && f.cnt > 0 {
            f.flag |= IOERR;
            store(cpu, at, &f)?;
            return Some(0);
        }
        f.flag &= !(IOREAD | IOEOF);
        f.ptr = f.base;
        f.cnt = 0;
    }
    f.flag |= IOWRT;
    if f.base == 0 && !is_device(cpu, f.file) {
        get_buffer(cpu, at, &mut f)?;
    }
    if f.base == 0 || f.flag & IONBF != 0 {
        store(cpu, at, &f)?;
        let n = io::write_fd(cpu, f.file, data);
        if n.is_none_or(|n| (n as usize) < data.len()) {
            f.flag |= IOERR;
            store(cpu, at, &f)?;
        }
        return Some(n.unwrap_or(0) as usize);
    }
    let mut done = 0;
    while done < data.len() {
        let used = f.ptr - f.base;
        let room = (f.bufsiz - used) as usize;
        if room == 0 {
            store(cpu, at, &f)?;
            if !flush(cpu, at)? {
                return Some(done);
            }
            f = load(cpu, at)?;
            f.flag |= IOWRT;
            continue;
        }
        let n = room.min(data.len() - done);
        put(cpu, f.ptr, &data[done..done + n])?;
        f.ptr += n as u32;
        done += n;
    }
    // The putc macro writes into the room left.
    f.cnt = (f.bufsiz - (f.ptr - f.base)) as i32;
    store(cpu, at, &f)?;
    Some(done)
}

/// Flush every stream: how many there are. For exit, fflush(NULL) and
/// _flushall.
pub fn flush_all(cpu: &mut Cpu) -> Option<u32> {
    let mut count = 0;
    for i in 0..MAX_STREAMS {
        let at = read32(cpu, var_at(V_STREAMS) + 4 * i)?;
        if at == 0 {
            continue;
        }
        let flag = read32(cpu, at + F_FLAG)?;
        if flag & (IOREAD | IOWRT | IORW) != 0 {
            flush(cpu, at)?;
            count += 1;
        }
    }
    Some(count)
}

/// _filbuf(file): fill the buffer, the first byte taken, or EOF.
pub fn filbuf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    match fill(cpu, args[0]) {
        Got::Byte(b) => Some(b as u32),
        Got::End => Some(EOF),
        Got::Fault => None,
        Got::Wait => io::wait_for_console(cpu),
    }
}

/// _flsbuf(ch, file): the buffer is full: write it out and put `ch`.
pub fn flsbuf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[ch, at] = args else { unreachable!() };
    let mut f = load(cpu, at)?;
    // The macro took _cnt below zero; what is in the buffer is right.
    f.cnt = 0;
    store(cpu, at, &f)?;
    if f.base != 0 && f.ptr > f.base && f.flag & IOWRT != 0 {
        flush(cpu, at)?;
        let mut f = load(cpu, at)?;
        f.flag |= IOWRT;
        store(cpu, at, &f)?;
    }
    let n = write_stream(cpu, at, &[ch as u8])?;
    Some(if n == 1 { ch & 0xFF } else { EOF })
}

/// fgetc(file) and getc(file).
pub fn fgetc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    match getc(cpu, args[0]) {
        Got::Byte(b) => Some(b as u32),
        Got::End => Some(EOF),
        Got::Fault => None,
        Got::Wait => io::wait_for_console(cpu),
    }
}

/// getchar() and _fgetchar(): fgetc(stdin).
pub fn getchar(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    fgetc(cpu, &[iob(0)])
}

/// fputc(ch, file) and putc.
pub fn fputc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let n = write_stream(cpu, args[1], &[args[0] as u8])?;
    Some(if n == 1 { args[0] & 0xFF } else { EOF })
}

/// putchar(ch) and _fputchar(ch): fputc to stdout.
pub fn putchar(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    fputc(cpu, &[args[0], iob(1)])
}

/// ungetc(ch, file): `ch`, or EOF if it can't go back.
pub fn ungetc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == EOF || !ungetc_byte(cpu, args[1], args[0] as u8) {
        return Some(EOF);
    }
    Some(args[0] & 0xFF)
}

/// fputs(text, file): 0, or EOF.
pub fn fputs(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let text = cstr(cpu, args[0])?;
    let n = write_stream(cpu, args[1], &text)?;
    Some(if n == text.len() { 0 } else { EOF })
}

/// puts(text): the text and a line feed on stdout.
pub fn puts(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mut text = cstr(cpu, args[0])?;
    text.push(b'\n');
    let n = write_stream(cpu, iob(1), &text)?;
    Some(if n == text.len() { 0 } else { EOF })
}

/// Read a line into `buffer` of `size` bytes: up to and with its line
/// feed, or what fits. None if a page isn't there; Some(None) for the
/// end of the file before anything, or a wait for the console.
fn read_line(
    cpu: &mut Cpu,
    at: u32,
    size: u32,
    keep_newline: bool,
) -> Option<Result<Vec<u8>, bool>> {
    let mut line = Vec::new();
    while (line.len() as u32) + 1 < size {
        match getc(cpu, at) {
            Got::Byte(b'\n') => {
                if keep_newline {
                    line.push(b'\n');
                }
                return Some(Ok(line));
            }
            Got::Byte(b) => line.push(b),
            Got::End => break,
            Got::Fault => return None,
            Got::Wait if line.is_empty() => return Some(Err(true)),
            Got::Wait => break,
        }
    }
    if line.is_empty() && size > 1 {
        return Some(Err(false));
    }
    Some(Ok(line))
}

/// fgets(buffer, size, file): the buffer, or NULL at the end.
pub fn fgets(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[buffer, size, at] = args else {
        unreachable!()
    };
    if size == 0 {
        return Some(0);
    }
    match read_line(cpu, at, size, true)? {
        Ok(line) => {
            put_cstr(cpu, buffer, &line)?;
            Some(buffer)
        }
        Err(true) => io::wait_for_console(cpu),
        Err(false) => Some(0),
    }
}

/// gets(buffer): a line from stdin without its line feed.
pub fn gets(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    match read_line(cpu, iob(0), u32::MAX, false)? {
        Ok(line) => {
            put_cstr(cpu, args[0], &line)?;
            Some(args[0])
        }
        Err(true) => io::wait_for_console(cpu),
        Err(false) => Some(0),
    }
}

/// fread(buffer, size, count, file): the items read whole.
pub fn fread(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[buffer, size, count, at] = args else {
        unreachable!()
    };
    let total = size.saturating_mul(count);
    if total == 0 {
        return Some(0);
    }
    if !cpu.bus.guest_probe(buffer, total as usize, true) {
        return None;
    }
    let mut done = 0u32;
    while done < total {
        let mut f = load(cpu, at)?;
        if f.cnt > 0 && f.flag & IOREAD != 0 {
            let n = (f.cnt as u32).min(total - done);
            let data = bytes(cpu, f.ptr, n)?;
            put(cpu, buffer + done, &data)?;
            f.ptr += n;
            f.cnt -= n as i32;
            store(cpu, at, &f)?;
            done += n;
            continue;
        }
        match fill(cpu, at) {
            Got::Byte(b) => {
                put(cpu, buffer + done, &[b])?;
                done += 1;
            }
            Got::End => break,
            Got::Fault => return None,
            Got::Wait if done == 0 => return io::wait_for_console(cpu),
            Got::Wait => break,
        }
    }
    Some(done / size)
}

/// fwrite(buffer, size, count, file): the items written whole.
pub fn fwrite(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[buffer, size, count, at] = args else {
        unreachable!()
    };
    let total = size.saturating_mul(count);
    if total == 0 {
        return Some(0);
    }
    let data = bytes(cpu, buffer, total)?;
    let n = write_stream(cpu, at, &data)?;
    Some(n as u32 / size)
}

/// fflush(file): 0, or EOF on an error; with NULL every stream.
pub fn fflush(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        flush_all(cpu)?;
        return Some(0);
    }
    Some(if flush(cpu, args[0])? { 0 } else { EOF })
}

/// _flushall(): how many streams there are.
pub fn flushall(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    flush_all(cpu)
}

/// The position of the stream's next read or write: its file's, less
/// what is buffered and not read yet, or more what is buffered and not
/// written yet. In text mode each line feed was a CR LF in the file.
fn tell_stream(cpu: &mut Cpu, at: u32) -> Option<Option<u64>> {
    let f = load(cpu, at)?;
    let Some(pos) = io::seek_fd(cpu, f.file, 0, 1) else {
        return Some(None);
    };
    let text = io::fd_info(cpu, f.file).is_some_and(|(_, flags)| flags & io::FTEXT != 0);
    let lfs = |cpu: &mut Cpu, from: u32, len: u32| -> Option<u64> {
        if !text || len == 0 {
            return Some(0);
        }
        Some(
            bytes(cpu, from, len)?
                .iter()
                .filter(|&&b| b == b'\n')
                .count() as u64,
        )
    };
    if f.base == 0 || f.flag & IONBF != 0 && f.cnt == 0 && f.flag & IOWRT != 0 {
        return Some(Some(pos));
    }
    if f.flag & IOWRT != 0 && f.ptr > f.base {
        let len = f.ptr - f.base;
        let extra = lfs(cpu, f.base, len)?;
        return Some(Some(pos + len as u64 + extra));
    }
    if f.flag & IOREAD != 0 && f.cnt > 0 {
        let extra = lfs(cpu, f.ptr, f.cnt as u32)?;
        // Below 0 when the file had bare LFs, as MSVCRT counts it.
        return Some(Some(pos.wrapping_sub(f.cnt as u64 + extra)));
    }
    Some(Some(pos))
}

/// ftell(file): the position, or -1.
pub fn ftell(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(tell_stream(cpu, args[0])?.map_or(EOF, |p| p as u32))
}

/// Move the stream's position: what is written out first, what is read
/// ahead dropped. 0, or -1.
fn seek_stream(cpu: &mut Cpu, at: u32, offset: i64, whence: u32) -> Option<u32> {
    if whence > 2 {
        set_errno(cpu, EINVAL);
        return Some(EOF);
    }
    let (offset, whence) = if whence == 1 {
        match tell_stream(cpu, at)? {
            Some(pos) => (pos as i64 + offset, 0),
            None => return Some(EOF),
        }
    } else {
        (offset, whence)
    };
    flush(cpu, at)?;
    let mut f = load(cpu, at)?;
    f.flag &= !IOEOF;
    if f.flag & IORW != 0 {
        f.flag &= !(IOREAD | IOWRT);
    }
    store(cpu, at, &f)?;
    Some(io::seek_fd(cpu, f.file, offset, whence).map_or(EOF, |_| 0))
}

/// fseek(file, offset, whence).
pub fn fseek(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    seek_stream(cpu, args[0], args[1] as i32 as i64, args[2])
}

/// rewind(file): to the start, the error flag cleared.
pub fn rewind(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    seek_stream(cpu, args[0], 0, 0)?;
    let mut f = load(cpu, args[0])?;
    f.flag &= !IOERR;
    store(cpu, args[0], &f)?;
    Some(0)
}

/// fgetpos(file, pos): the position as a 64-bit fpos_t.
pub fn fgetpos(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    match tell_stream(cpu, args[0])? {
        Some(pos) => {
            put(cpu, args[1], &pos.to_le_bytes())?;
            Some(0)
        }
        None => Some(EOF),
    }
}

/// fsetpos(file, pos).
pub fn fsetpos(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let raw = bytes(cpu, args[1], 8)?;
    let pos = i64::from_le_bytes(raw.try_into().unwrap());
    seek_stream(cpu, args[0], pos, 0)
}

/// feof(file), ferror(file), clearerr(file) and _fileno(file).
pub fn feof(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(load(cpu, args[0])?.flag & IOEOF)
}

pub fn ferror(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(load(cpu, args[0])?.flag & IOERR)
}

pub fn clearerr(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mut f = load(cpu, args[0])?;
    f.flag &= !(IOERR | IOEOF);
    store(cpu, args[0], &f)?;
    Some(0)
}

pub fn fileno(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(load(cpu, args[0])?.file)
}

/// setvbuf(file, buffer, mode, size): the stream's buffer, its own if
/// `buffer` is NULL; _IONBF (4) for none. 0, or -1 for a bad mode.
pub fn setvbuf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[at, buffer, mode, size] = args else {
        unreachable!()
    };
    if !matches!(mode, 0 | 0x40 | IONBF) || (mode != IONBF && !(2..=0x7FFF_FFFF).contains(&size)) {
        return Some(EOF);
    }
    flush(cpu, at)?;
    let mut f = load(cpu, at)?;
    if f.flag & IOMYBUF != 0 && f.base != 0 {
        heap_free(cpu, f.base);
    }
    f.flag &= !(IOMYBUF | IOYOURBUF | IONBF);
    f.ptr = 0;
    f.base = 0;
    f.cnt = 0;
    f.bufsiz = 0;
    if mode == IONBF {
        f.flag |= IONBF;
        f.base = at + F_CHARBUF;
        f.bufsiz = 1;
    } else if buffer != 0 {
        f.flag |= IOYOURBUF;
        f.base = buffer;
        f.bufsiz = size & !1;
    } else if let Some(block) = heap_alloc(cpu, size) {
        f.flag |= IOMYBUF;
        f.base = block;
        f.bufsiz = size;
    } else {
        return Some(EOF);
    }
    f.ptr = f.base;
    store(cpu, at, &f)?;
    Some(0)
}

/// setbuf(file, buffer): setvbuf with BUFSIZ, or no buffer for NULL.
pub fn setbuf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mode = if args[1] == 0 { IONBF } else { 0 };
    setvbuf(cpu, &[args[0], args[1], mode, 512])
}

/// A stream not in use: one of `_iob`'s, or one from the heap. None if
/// there are as many as there can be.
fn free_stream(cpu: &mut Cpu) -> Option<Option<u32>> {
    for i in 3..MAX_STREAMS {
        let slot = var_at(V_STREAMS) + 4 * i;
        let at = read32(cpu, slot)?;
        if at == 0 {
            let Some(at) = heap_alloc(cpu, FILE_SIZE) else {
                return Some(None);
            };
            cpu.bus.guest_fill(at, FILE_SIZE as usize, 0);
            write32(cpu, slot, at)?;
            return Some(Some(at));
        }
        if read32(cpu, at + F_FLAG)? & (IOREAD | IOWRT | IORW) == 0 {
            return Some(Some(at));
        }
    }
    Some(None)
}

/// The `_open` flags and the stream flags of a fopen mode: None for a
/// mode that isn't one.
fn parse_mode(mode: &[u8]) -> Option<(u32, u32)> {
    let (mut flags, mut stream) = match mode.first()? {
        b'r' => (0, IOREAD),
        b'w' => (0x0001 | 0x0100 | 0x0200, IOWRT),
        b'a' => (0x0001 | 0x0100 | 0x0008, IOWRT),
        _ => return None,
    };
    for &c in &mode[1..] {
        match c {
            b'+' => {
                flags = flags & !3 | 0x0002;
                stream = IORW;
            }
            b't' => flags |= 0x4000,
            b'b' => flags |= 0x8000,
            b'c' | b'n' | b'S' | b'R' | b'T' | b'D' | b' ' => {}
            _ => break,
        }
    }
    Some((flags, stream))
}

/// Set up the stream at `at` on `fd` with the stream flags `flag`.
fn attach(cpu: &mut Cpu, at: u32, fd: u32, flag: u32) -> Option<u32> {
    let f = File {
        flag,
        file: fd,
        ..File::default()
    };
    store(cpu, at, &f)?;
    Some(at)
}

/// fopen(name, mode) and _fsopen(name, mode, share): the stream, or
/// NULL with errno set.
pub fn fopen(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        set_errno(cpu, EINVAL);
        return Some(0);
    }
    let name = cstr(cpu, args[0])?;
    let mode = cstr(cpu, args[1])?;
    let Some((flags, flag)) = parse_mode(&mode) else {
        set_errno(cpu, EINVAL);
        return Some(0);
    };
    let Some(at) = free_stream(cpu)? else {
        set_errno(cpu, EMFILE);
        return Some(0);
    };
    let Some(fd) = io::open_path(cpu, &name, flags) else {
        return Some(0);
    };
    attach(cpu, at, fd, flag)
}

/// _fdopen(fd, mode): a stream on the descriptor.
pub fn fdopen(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mode = cstr(cpu, args[1])?;
    let Some((flags, flag)) = parse_mode(&mode) else {
        set_errno(cpu, EINVAL);
        return Some(0);
    };
    if io::fd_info(cpu, args[0]).is_none() {
        return Some(0);
    }
    let Some(at) = free_stream(cpu)? else {
        set_errno(cpu, EMFILE);
        return Some(0);
    };
    if flags & 0x8000 != 0 {
        io::setmode(cpu, &[args[0], 0x8000])?;
    } else if flags & 0x4000 != 0 {
        io::setmode(cpu, &[args[0], 0x4000])?;
    }
    attach(cpu, at, args[0], flag)
}

/// Close the stream at `at`: its buffer written and freed, its file
/// closed and, for tmpfile's, deleted. 0, or EOF.
fn close_stream(cpu: &mut Cpu, at: u32) -> Option<u32> {
    let f = load(cpu, at)?;
    if f.flag & (IOREAD | IOWRT | IORW) == 0 {
        return Some(EOF);
    }
    let flushed = flush(cpu, at)?;
    let f = load(cpu, at)?;
    if f.flag & IOMYBUF != 0 && f.base != 0 {
        heap_free(cpu, f.base);
    }
    let closed = io::close_fd(cpu, f.file).is_some();
    if f.tmpfname != 0 {
        let name = cstr(cpu, f.tmpfname)?;
        let path = crate::dosstr::from_bytes(&name);
        let _ = cpu.bus.disk.delete_file(&path);
        heap_free(cpu, f.tmpfname);
    }
    store(cpu, at, &File::default())?;
    Some(if flushed && closed { 0 } else { EOF })
}

/// fclose(file).
pub fn fclose(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    close_stream(cpu, args[0])
}

/// _fcloseall(): every stream but the standard three: how many.
pub fn fcloseall(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let mut count = 0;
    for i in 3..MAX_STREAMS {
        let at = read32(cpu, var_at(V_STREAMS) + 4 * i)?;
        if at != 0 && read32(cpu, at + F_FLAG)? & (IOREAD | IOWRT | IORW) != 0 {
            close_stream(cpu, at)?;
            count += 1;
        }
    }
    Some(count)
}

/// freopen(name, mode, file): the stream closed and opened again on
/// another file.
pub fn freopen(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let name = cstr(cpu, args[0])?;
    let mode = cstr(cpu, args[1])?;
    let at = args[2];
    close_stream(cpu, at)?;
    let Some((flags, flag)) = parse_mode(&mode) else {
        set_errno(cpu, EINVAL);
        return Some(0);
    };
    let Some(fd) = io::open_path(cpu, &name, flags) else {
        return Some(0);
    };
    attach(cpu, at, fd, flag)
}

/// tmpfile(): a new file, read and written in binary, deleted when it is
/// closed.
pub fn tmpfile(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let Some(at) = free_stream(cpu)? else {
        set_errno(cpu, EMFILE);
        return Some(0);
    };
    let name = io::tmpnam(cpu, &[0])?;
    let name = cstr(cpu, name)?;
    let Some(fd) = io::open_path(cpu, &name, 0x0002 | 0x0100 | 0x0400 | 0x8000) else {
        return Some(0);
    };
    attach(cpu, at, fd, IORW)?;
    let Some(copy) = heap_alloc(cpu, name.len() as u32 + 1) else {
        return Some(at);
    };
    put_cstr(cpu, copy, &name)?;
    write32(cpu, at + F_TMPFNAME, copy)?;
    Some(at)
}

/// _getw(file): an int's four bytes.
pub fn getw(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mut word = [0u8; 4];
    for byte in &mut word {
        match getc(cpu, args[0]) {
            Got::Byte(b) => *byte = b,
            Got::Fault => return None,
            _ => return Some(EOF),
        }
    }
    Some(u32::from_le_bytes(word))
}

/// _putw(word, file).
pub fn putw(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let n = write_stream(cpu, args[1], &args[0].to_le_bytes())?;
    Some(if n == 4 { args[0] } else { EOF })
}

/// Whether the stream's descriptor is in text mode.
fn text_mode(cpu: &mut Cpu, at: u32) -> Option<bool> {
    let fd = load(cpu, at)?.file;
    Some(io::fd_info(cpu, fd).is_some_and(|(_, flags)| flags & io::FTEXT != 0))
}

/// fputwc(ch, file): in text mode the character's byte in the code page,
/// in binary mode its two bytes.
pub fn fputwc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let c = args[0] as u16;
    let data = if text_mode(cpu, args[1])? {
        vec![narrow(c).unwrap_or(b'?')]
    } else {
        c.to_le_bytes().to_vec()
    };
    let n = write_stream(cpu, args[1], &data)?;
    Some(if n == data.len() { c as u32 } else { 0xFFFF })
}

/// fgetwc(file): the next character, read as fputwc writes it.
pub fn fgetwc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let text = text_mode(cpu, args[0])?;
    let first = match getc(cpu, args[0]) {
        Got::Byte(b) => b,
        Got::Fault => return None,
        Got::Wait => return io::wait_for_console(cpu),
        Got::End => return Some(0xFFFF),
    };
    if text {
        return Some(widen(first) as u32);
    }
    match getc(cpu, args[0]) {
        Got::Byte(b) => Some(u16::from_le_bytes([first, b]) as u32),
        Got::Fault => None,
        _ => Some(0xFFFF),
    }
}

/// ungetwc(ch, file).
pub fn ungetwc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let c = args[0] as u16;
    if c == 0xFFFF {
        return Some(0xFFFF);
    }
    let ok = if text_mode(cpu, args[1])? {
        ungetc_byte(cpu, args[1], narrow(c).unwrap_or(b'?'))
    } else {
        let [lo, hi] = c.to_le_bytes();
        ungetc_byte(cpu, args[1], hi) && ungetc_byte(cpu, args[1], lo)
    };
    Some(if ok { c as u32 } else { 0xFFFF })
}

/// fputws(text, file).
pub fn fputws(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let text = wstr(cpu, args[0])?;
    let data: Vec<u8> = if text_mode(cpu, args[1])? {
        text.iter().map(|&c| narrow(c).unwrap_or(b'?')).collect()
    } else {
        text.iter().flat_map(|c| c.to_le_bytes()).collect()
    };
    let n = write_stream(cpu, args[1], &data)?;
    Some(if n == data.len() { 0 } else { EOF })
}

/// fgetws(buffer, count, file): a line of wide characters.
pub fn fgetws(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[buffer, count, at] = args else {
        unreachable!()
    };
    let mut line: Vec<u16> = Vec::new();
    while (line.len() as u32) + 1 < count {
        let c = fgetwc(cpu, &[at])?;
        if c == 0xFFFF {
            break;
        }
        line.push(c as u16);
        if c == b'\n' as u32 {
            break;
        }
    }
    if line.is_empty() {
        return Some(0);
    }
    put_wstr(cpu, buffer, &line)?;
    Some(buffer)
}

/// putwchar and getwchar, on stdout and stdin.
pub fn putwchar(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    fputwc(cpu, &[args[0], iob(1)])
}

pub fn getwchar(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    fgetwc(cpu, &[iob(0)])
}

// The printf family.

/// The format at `at`, narrow, as format.rs reads it.
fn narrow_format(cpu: &mut Cpu, at: u32) -> Option<Vec<u16>> {
    Some(cstr(cpu, at)?.into_iter().map(u16::from).collect())
}

/// The output of a narrow function as bytes.
fn to_bytes(text: &[u16]) -> Vec<u8> {
    text.iter().map(|&c| c as u8).collect()
}

/// Print the narrow format at `format` with the arguments at `args` to
/// the stream at `file`: the bytes written, or -1.
fn print_to(cpu: &mut Cpu, file: u32, format: u32, args: u32) -> Option<u32> {
    let format = narrow_format(cpu, format)?;
    let text = format::printf(cpu, &format, &mut Args { at: args }, false)?;
    let data = to_bytes(&text);
    let n = write_stream(cpu, file, &data)?;
    Some(if n == data.len() { n as u32 } else { EOF })
}

pub fn printf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    print_to(cpu, iob(1), args[0], varargs(cpu, 1))
}

pub fn fprintf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    print_to(cpu, args[0], args[1], varargs(cpu, 2))
}

pub fn vprintf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    print_to(cpu, iob(1), args[0], args[1])
}

pub fn vfprintf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    print_to(cpu, args[0], args[1], args[2])
}

/// Print into the buffer `buffer`: all of it and a 0 with no limit, else
/// at most `limit` bytes and -1 when it didn't fit (no 0 then, nor when
/// it fits exactly), as _snprintf does.
fn print_into(
    cpu: &mut Cpu,
    buffer: u32,
    limit: Option<u32>,
    format: u32,
    args: u32,
) -> Option<u32> {
    let format = narrow_format(cpu, format)?;
    let text = format::printf(cpu, &format, &mut Args { at: args }, false)?;
    let data = to_bytes(&text);
    match limit {
        None => {
            put_cstr(cpu, buffer, &data)?;
            Some(data.len() as u32)
        }
        Some(limit) if data.len() as u32 > limit => {
            put(cpu, buffer, &data[..limit as usize])?;
            Some(EOF)
        }
        Some(limit) if data.len() as u32 == limit => {
            put(cpu, buffer, &data)?;
            Some(limit)
        }
        Some(_) => {
            put_cstr(cpu, buffer, &data)?;
            Some(data.len() as u32)
        }
    }
}

pub fn sprintf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    print_into(cpu, args[0], None, args[1], varargs(cpu, 2))
}

pub fn vsprintf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    print_into(cpu, args[0], None, args[1], args[2])
}

pub fn snprintf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    print_into(cpu, args[0], Some(args[1]), args[2], varargs(cpu, 3))
}

pub fn vsnprintf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    print_into(cpu, args[0], Some(args[1]), args[2], args[3])
}

/// The wide functions: formats and output of 16-bit characters.
fn wide_print_into(
    cpu: &mut Cpu,
    buffer: u32,
    limit: Option<u32>,
    format: u32,
    args: u32,
) -> Option<u32> {
    let format = wstr(cpu, format)?;
    let text = format::printf(cpu, &format, &mut Args { at: args }, true)?;
    match limit {
        Some(limit) if text.len() as u32 > limit => {
            let data: Vec<u8> = text[..limit as usize]
                .iter()
                .flat_map(|c| c.to_le_bytes())
                .collect();
            put(cpu, buffer, &data)?;
            Some(EOF)
        }
        Some(limit) if text.len() as u32 == limit => {
            let data: Vec<u8> = text.iter().flat_map(|c| c.to_le_bytes()).collect();
            put(cpu, buffer, &data)?;
            Some(limit)
        }
        _ => {
            put_wstr(cpu, buffer, &text)?;
            Some(text.len() as u32)
        }
    }
}

pub fn swprintf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    wide_print_into(cpu, args[0], None, args[1], varargs(cpu, 2))
}

pub fn vswprintf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    wide_print_into(cpu, args[0], None, args[1], args[2])
}

pub fn snwprintf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    wide_print_into(cpu, args[0], Some(args[1]), args[2], varargs(cpu, 3))
}

pub fn vsnwprintf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    wide_print_into(cpu, args[0], Some(args[1]), args[2], args[3])
}

/// Print a wide format to a stream, as its bytes in the code page.
fn wide_print_to(cpu: &mut Cpu, file: u32, format: u32, args: u32) -> Option<u32> {
    let format = wstr(cpu, format)?;
    let text = format::printf(cpu, &format, &mut Args { at: args }, true)?;
    let data: Vec<u8> = text.iter().map(|&c| narrow(c).unwrap_or(b'?')).collect();
    let n = write_stream(cpu, file, &data)?;
    Some(if n == data.len() {
        text.len() as u32
    } else {
        EOF
    })
}

pub fn wprintf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    wide_print_to(cpu, iob(1), args[0], varargs(cpu, 1))
}

pub fn fwprintf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    wide_print_to(cpu, args[0], args[1], varargs(cpu, 2))
}

pub fn vwprintf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    wide_print_to(cpu, iob(1), args[0], args[1])
}

pub fn vfwprintf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    wide_print_to(cpu, args[0], args[1], args[2])
}

// The scanf family.

pub fn sscanf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let text: Vec<u16> = cstr(cpu, args[0])?.into_iter().map(u16::from).collect();
    let format = narrow_format(cpu, args[1])?;
    let mut input = Input::Text {
        chars: text,
        pos: 0,
    };
    let at = varargs(cpu, 2);
    format::scanf(cpu, &format, &mut input, &mut Args { at }, false)
}

pub fn swscanf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let text = wstr(cpu, args[0])?;
    let format = wstr(cpu, args[1])?;
    let mut input = Input::Text {
        chars: text,
        pos: 0,
    };
    let at = varargs(cpu, 2);
    format::scanf(cpu, &format, &mut input, &mut Args { at }, true)
}

/// Read a stream as the format at `format` says.
fn scan_stream(cpu: &mut Cpu, file: u32, format: Vec<u16>, args: u32, wide: bool) -> Option<u32> {
    // The console's line is waited for before anything is read, so the
    // call can run again from its start when it comes. One that runs out
    // within a conversion ends there.
    let f = load(cpu, file)?;
    if f.cnt <= 0 || f.flag & IOREAD == 0 {
        match fill(cpu, file) {
            Got::Byte(b) => {
                ungetc_byte(cpu, file, b);
            }
            Got::Wait => return io::wait_for_console(cpu),
            Got::Fault => return None,
            Got::End => {}
        }
    }
    let mut input = Input::Stream { file };
    format::scanf(cpu, &format, &mut input, &mut Args { at: args }, wide)
}

pub fn fscanf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let format = narrow_format(cpu, args[1])?;
    let at = varargs(cpu, 2);
    scan_stream(cpu, args[0], format, at, false)
}

pub fn scanf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let format = narrow_format(cpu, args[0])?;
    let at = varargs(cpu, 1);
    scan_stream(cpu, iob(0), format, at, false)
}

pub fn fwscanf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let format = wstr(cpu, args[1])?;
    let at = varargs(cpu, 2);
    scan_stream(cpu, args[0], format, at, true)
}

pub fn wscanf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let format = wstr(cpu, args[0])?;
    let at = varargs(cpu, 1);
    scan_stream(cpu, iob(0), format, at, true)
}

/// __p__iob() and __iob_func(): `_iob`'s address.
pub fn p_iob(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(var_at(V_IOB))
}

/// UCRT's __acrt_iob_func(index): the address of `_iob[index]`, the
/// stream UCRT's stdin, stdout and stderr name.
pub fn acrt_iob_func(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(iob(args[0]))
}

/// UCRT's printf options (the low half of their 64-bit first argument):
/// a buffer too small gets no terminator and the call returns -1, or the
/// call returns the length the whole output needs.
const LEGACY_VSPRINTF_NULL_TERMINATION: u32 = 0x1;
const STANDARD_SNPRINTF_BEHAVIOR: u32 = 0x2;

/// `len` characters of output, `data` in units of `size` bytes, put in
/// `buffer` of `count` characters as UCRT's __stdio_common_vsprintf and
/// __stdio_common_vswprintf put it, and what they return, as Wine's
/// ucrtbase has them. Without a buffer they return the length.
fn ucrt_print_into(
    cpu: &mut Cpu,
    options: u32,
    buffer: u32,
    count: u32,
    data: &[u8],
    size: u32,
) -> Option<u32> {
    let len = data.len() as u32 / size;
    if buffer == 0 {
        return Some(len);
    }
    let kept = len.min(count);
    put(cpu, buffer, &data[..(kept * size) as usize])?;
    let nul = vec![0; size as usize];
    if len < count {
        put(cpu, buffer + len * size, &nul)?;
    }
    if options & LEGACY_VSPRINTF_NULL_TERMINATION != 0 {
        return Some(if len > count { EOF } else { len });
    }
    if len < count {
        return Some(len);
    }
    if count > 0 {
        put(cpu, buffer + (count - 1) * size, &nul)?;
    }
    Some(match (options & STANDARD_SNPRINTF_BEHAVIOR != 0, count) {
        (true, _) => len,
        (false, 0) => EOF,
        (false, _) => (-2i32) as u32,
    })
}

/// __stdio_common_vsprintf(options, buffer, count, format, locale,
/// args): UCRT's sprintf family. The locale is the C locale's.
pub fn stdio_common_vsprintf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let format = narrow_format(cpu, args[4])?;
    let text = format::printf(cpu, &format, &mut Args { at: args[6] }, false)?;
    ucrt_print_into(cpu, args[0], args[2], args[3], &to_bytes(&text), 1)
}

/// __stdio_common_vswprintf: the wide one. %s takes a wide string, as
/// MSVCRT's has it and as Wine's DLLs, built with the legacy wide
/// specifiers, expect.
pub fn stdio_common_vswprintf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let format = wstr(cpu, args[4])?;
    let text = format::printf(cpu, &format, &mut Args { at: args[6] }, true)?;
    let data: Vec<u8> = text.iter().flat_map(|c| c.to_le_bytes()).collect();
    ucrt_print_into(cpu, args[0], args[2], args[3], &data, 2)
}
