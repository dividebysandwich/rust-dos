//! LZExpand: reading files that COMPRESS.EXE packed (SZDD, its 'A'
//! method), as setup programs of Windows 3.1's and 95's time do with
//! files named like SETUP.EX_. Windows 95's KERNEL32 has these
//! functions, and LZ32.DLL exports the same ones.
//!
//! A handle LZInit gives is 400h and up, one of 16 a process may have;
//! other handles are the files' own, which LZRead, LZSeek and LZClose
//! pass on to _lread, _llseek and _lclose. The file is expanded whole
//! when LZInit opens it, and reads come from the expanded bytes. The
//! format and the expansion follow Wine's lzexpand.c (LGPL 2.1 or
//! later):
//!
//! Copyright 1996 Marcus Meissner

use super::{ERROR_CALL_NOT_IMPLEMENTED, file, set_error, string_at};
use crate::cpu::Cpu;
use crate::win32::process;

/// LZERROR_*: what the functions return when they fail.
const LZERROR_BADINHANDLE: i32 = -1;
const LZERROR_READ: i32 = -3;
const LZERROR_WRITE: i32 = -4;
const LZERROR_GLOBALLOC: i32 = -5;
const LZERROR_BADVALUE: i32 = -7;
const LZERROR_UNKNOWNALG: i32 = -8;

const HFILE_ERROR: u32 = 0xFFFF_FFFF;
/// OpenFile's styles: OF_READ is 0, and the sharing modes are 70h.
const OF_SHARE_MASK: u32 = 0x70;
/// OFSTRUCT's size; its error code is at 2, its path at 8.
const OFSTRUCT: usize = 136;

/// The first LZ handle, and how many a process may have.
const FIRST_HANDLE: u32 = 0x400;
const MAX_FILES: u32 = 16;

/// What starts a compressed file, then the method ('A'), the last
/// character of its name, and the expanded size.
const MAGIC: [u8; 8] = [b'S', b'Z', b'D', b'D', 0x88, 0xF0, 0x27, 0x33];
const HEADER: usize = 14;

/// A compressed file LZInit opened.
#[derive(Clone, Debug, Default)]
pub struct LzFile {
    /// Its handle's place: the handle is FIRST_HANDLE + slot.
    pub slot: u32,
    /// The file's own handle, which LZClose closes.
    pub file: u32,
    /// The expanded size the header gives, and the place reads go on
    /// from.
    pub length: u32,
    pub wanted: u32,
    /// The expanded bytes.
    pub data: Vec<u8>,
}

/// Expand the compressed bytes after the header: a flag byte says of
/// each of the next eight items whether it is a byte (1) or a reference
/// (0) to up to 18 bytes of the 4 KB window, which starts as spaces with
/// its pointer 16 bytes before its end.
pub fn expand(packed: &[u8]) -> Vec<u8> {
    let mut window = [b' '; 0x1000];
    let mut at = 0xFF0usize;
    let mut out = Vec::with_capacity(packed.len() * 2);
    let mut input = packed.iter().copied();
    'outer: while let Some(flags) = input.next() {
        for bit in 0..8 {
            if flags >> bit & 1 != 0 {
                let Some(b) = input.next() else {
                    break 'outer;
                };
                out.push(b);
                window[at] = b;
                at = (at + 1) & 0xFFF;
            } else {
                let (Some(b1), Some(b2)) = (input.next(), input.next()) else {
                    break 'outer;
                };
                let mut from = b1 as usize | ((b2 as usize & 0xF0) << 4);
                for _ in 0..(b2 & 0xF) as usize + 3 {
                    let b = window[from];
                    from = (from + 1) & 0xFFF;
                    out.push(b);
                    window[at] = b;
                    at = (at + 1) & 0xFFF;
                }
            }
        }
    }
    out
}

/// The header of a compressed file: its method, the last character of
/// its name and its expanded size; None when `bytes` don't start with
/// one.
pub fn header(bytes: &[u8]) -> Option<(u8, u8, u32)> {
    if bytes.len() < HEADER || bytes[..8] != MAGIC {
        return None;
    }
    let size = u32::from_le_bytes([bytes[10], bytes[11], bytes[12], bytes[13]]);
    Some((bytes[8], bytes[9], size))
}

/// The LZ file of the handle `handle` in the running process.
fn lz_file(cpu: &mut Cpu, handle: u32) -> Option<&mut LzFile> {
    let slot = handle.checked_sub(FIRST_HANDLE)?;
    process(cpu).lz_files.iter_mut().find(|f| f.slot == slot)
}

/// The whole of the file `handle`, from its start.
fn read_all(cpu: &mut Cpu, handle: u32) -> Result<Vec<u8>, u32> {
    super::seek_file(cpu, handle, 0, 0)?;
    let mut bytes = Vec::new();
    loop {
        let chunk = super::read_file(cpu, handle, 0x8000)?;
        if chunk.is_empty() {
            break;
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// LZInit(file): an LZ handle for a compressed file; the file's own
/// handle, its pointer back at its start, for one that isn't.
fn lz_init_handle(cpu: &mut Cpu, file: u32) -> i32 {
    let Ok(bytes) = read_all(cpu, file) else {
        return LZERROR_BADINHANDLE;
    };
    let Some((method, _, length)) = header(&bytes) else {
        let _ = super::seek_file(cpu, file, 0, 0);
        return file as i32;
    };
    if method != b'A' {
        let _ = super::seek_file(cpu, file, 0, 0);
        return LZERROR_UNKNOWNALG;
    }
    let files = &mut process(cpu).lz_files;
    let Some(slot) = (0..MAX_FILES).find(|s| files.iter().all(|f| f.slot != *s)) else {
        return LZERROR_GLOBALLOC;
    };
    files.push(LzFile {
        slot,
        file,
        length,
        wanted: 0,
        data: expand(&bytes[HEADER..]),
    });
    (FIRST_HANDLE + slot) as i32
}

pub(in crate::win32) fn lz_init(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(lz_init_handle(cpu, args[0]) as u32)
}

/// LZStart(): 1. LZDone(): nothing to do.
pub(in crate::win32) fn lz_start(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(1)
}

pub(in crate::win32) fn lz_done(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// LZRead(handle, buffer, count): the count read, LZERROR_READ past the
/// expanded end.
pub(in crate::win32) fn lz_read(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, buffer, count] = args else {
        unreachable!()
    };
    let Some(f) = lz_file(cpu, handle) else {
        return file::lread(cpu, args);
    };
    let end = (f.length as usize).min(f.data.len());
    let from = (f.wanted as usize).min(end);
    let n = (count as i32).max(0) as usize;
    if n > 0 && from == end {
        return Some(LZERROR_READ as u32);
    }
    let bytes = f.data[from..end.min(from + n)].to_vec();
    if !cpu.bus.guest_probe(buffer, bytes.len(), true) {
        return None;
    }
    cpu.bus.guest_write_bytes(buffer, &bytes);
    if let Some(f) = lz_file(cpu, handle) {
        f.wanted += bytes.len() as u32;
    }
    Some(bytes.len() as u32)
}

/// LZSeek(handle, offset, origin): the new place, which can't be past
/// the expanded end.
pub(in crate::win32) fn lz_seek(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, offset, origin] = args else {
        unreachable!()
    };
    let Some(f) = lz_file(cpu, handle) else {
        return file::llseek(cpu, args);
    };
    let offset = offset as i32 as i64;
    let wanted = match origin {
        1 => f.wanted as i64 + offset,
        2 => f.length as i64 - offset,
        _ => offset,
    };
    if !(0..=f.length as i64).contains(&wanted) {
        return Some(LZERROR_BADVALUE as u32);
    }
    f.wanted = wanted as u32;
    Some(wanted as u32)
}

/// LZCopy(source, dest) and CopyLZFile: the source, expanded if it is
/// compressed, written to `dest`, which gets the source's time: the
/// count written, or an LZERROR.
pub(in crate::win32) fn lz_copy(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[source, dest] = args else {
        unreachable!()
    };
    let mut src = source;
    let mut opened = false;
    if lz_file(cpu, src).is_none() {
        let handle = lz_init_handle(cpu, src);
        if handle <= 0 {
            return Some(0);
        }
        opened = handle as u32 != src;
        src = handle as u32;
    }
    let (bytes, real) = match lz_file(cpu, src) {
        Some(f) => {
            let end = (f.length as usize).min(f.data.len());
            let from = (f.wanted as usize).min(end);
            let bytes = f.data[from..end].to_vec();
            f.wanted = end as u32;
            (bytes, f.file)
        }
        None => {
            let mut bytes = Vec::new();
            loop {
                match super::read_file(cpu, src, 0x8000) {
                    Ok(chunk) if chunk.is_empty() => break,
                    Ok(chunk) => bytes.extend_from_slice(&chunk),
                    Err(_) => return Some(LZERROR_READ as u32),
                }
            }
            (bytes, src)
        }
    };
    match super::write_file(cpu, dest, &bytes) {
        Ok(n) if n as usize == bytes.len() => {}
        _ => return Some(LZERROR_WRITE as u32),
    }
    copy_time(cpu, real, dest);
    if opened {
        lz_close(cpu, &[src]);
    }
    Some(bytes.len() as u32)
}

/// Give the file `to` the time of the last write of the file `from`.
fn copy_time(cpu: &mut Cpu, from: u32, to: u32) {
    let (Some(from), Some(to)) = (file::open_of(cpu, from), file::open_of(cpu, to)) else {
        return;
    };
    let disk = &mut cpu.bus.disk;
    if let Ok((time, date)) = disk.file_time(from.sft) {
        let _ = disk.set_file_time(to.sft, time, date);
    }
}

/// LZClose(handle): the LZ handle and its file, or the file `handle`.
pub(in crate::win32) fn lz_close(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let handle = args[0];
    let Some(slot) = lz_file(cpu, handle).map(|f| f.slot) else {
        file::lclose(cpu, args);
        return Some(0);
    };
    let files = &mut process(cpu).lz_files;
    let i = files.iter().position(|f| f.slot == slot)?;
    let f = files.remove(i);
    crate::win32::object::close(cpu, f.file);
    Some(0)
}

/// LZOpenFileA(name, ofstruct, style): OpenFile's handle, through LZInit
/// when the file is opened for reading, so a compressed one reads
/// expanded. As on Windows 9x, the name isn't tried again with its last
/// character "_", and when the file isn't there the OFSTRUCT keeps all
/// but its error code.
pub(in crate::win32) fn lz_open_file_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[_, of, style] = args else {
        unreachable!()
    };
    if !cpu.bus.guest_probe(of, OFSTRUCT, true) {
        return None;
    }
    let mut before = [0u8; OFSTRUCT];
    cpu.bus.guest_read_bytes(of, &mut before);
    let handle = file::open_file_16(cpu, args)?;
    if handle == HFILE_ERROR {
        let code = [cpu.bus.guest_read_8(of + 2), cpu.bus.guest_read_8(of + 3)];
        before[2..4].copy_from_slice(&code);
        cpu.bus.guest_write_bytes(of, &before);
        return Some(LZERROR_BADINHANDLE as u32);
    }
    if style & !OF_SHARE_MASK != 0 {
        return Some(handle);
    }
    let lz = lz_init_handle(cpu, handle);
    Some(if lz <= 0 { handle } else { lz as u32 })
}

/// LZOpenFileW and GetExpandedNameW: Windows 95 has no Unicode ones.
pub(in crate::win32) fn lz_open_file_w(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    set_error(cpu, ERROR_CALL_NOT_IMPLEMENTED);
    Some(LZERROR_BADINHANDLE as u32)
}

/// GetExpandedNameA(name, out): the name the compressed file `name` had,
/// from the last character its header keeps ("SETUP.EX_" with 'E' is
/// "SETUP.EXE"; a name ending in "." loses it), in the case of the
/// name's last letter; `name` itself for a file that isn't compressed.
/// 1, or LZERROR_BADINHANDLE when the file can't be opened.
pub(in crate::win32) fn get_expanded_name_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[name, out] = args else { unreachable!() };
    let text = string_at(cpu, name)?;
    let handle = match super::open_handle(cpu, &text, file::GENERIC_READ, 3, file::OPEN_EXISTING) {
        Ok(handle) => handle,
        Err(code) => {
            set_error(cpu, code);
            return Some(LZERROR_BADINHANDLE as u32);
        }
    };
    let head = super::read_file(cpu, handle, HEADER as u32).unwrap_or_default();
    crate::win32::object::close(cpu, handle);
    let mut bytes = crate::dosstr::to_bytes(&text);
    if let Some((_, last, _)) = header(&head) {
        bytes = expanded_name(&bytes, last);
    }
    bytes.push(0);
    if !cpu.bus.guest_probe(out, bytes.len(), true) {
        return None;
    }
    cpu.bus.guest_write_bytes(out, &bytes);
    Some(1)
}

/// The expanded name of the compressed file `name` whose header keeps
/// `last` as its name's last character.
pub fn expanded_name(name: &[u8], last: u8) -> Vec<u8> {
    let mut out = name.to_vec();
    let base = out
        .iter()
        .rposition(|&c| matches!(c, b'/' | b'\\' | b':'))
        .map_or(0, |i| i + 1);
    if base == out.len() {
        return out;
    }
    let lower = out
        .iter()
        .rev()
        .find(|c| c.is_ascii_alphabetic())
        .is_none_or(|c| c.is_ascii_lowercase());
    let last = if !last.is_ascii_alphabetic() {
        last
    } else if lower {
        last.to_ascii_lowercase()
    } else {
        last.to_ascii_uppercase()
    };
    if let Some(dot) = out[base..].iter().position(|&c| c == b'.') {
        let dot = base + dot;
        if dot + 1 == out.len() {
            out.truncate(dot);
        } else if out.last() == Some(&b'_') {
            *out.last_mut().unwrap() = last;
        }
    }
    out
}

crate::state_fields!(LzFile {
    slot,
    file,
    length,
    wanted,
    data
});

#[cfg(test)]
mod tests {
    use super::*;

    /// "This is a test file." as COMPRESS.EXE packed it, the file of
    /// Wine's lzexpand test.
    const PACKED: [u8; 36] = [
        0x53, 0x5A, 0x44, 0x44, 0x88, 0xF0, 0x27, 0x33, 0x41, 0x74, 0x75, 0x14, 0x00, 0x00, 0xDF,
        0x54, 0x68, 0x69, 0x73, 0x20, 0xF2, 0xF0, 0x61, 0x20, 0xFF, 0x74, 0x65, 0x73, 0x74, 0x20,
        0x66, 0x69, 0x6C, 0x03, 0x65, 0x2E,
    ];

    #[test]
    fn expands_compress_exe_output() {
        let (method, last, size) = header(&PACKED).unwrap();
        assert_eq!((method, last, size), (b'A', b't', 20));
        assert_eq!(expand(&PACKED[HEADER..]), b"This is a test file.");
    }

    #[test]
    fn expanded_names() {
        assert_eq!(expanded_name(b"SETUP.EX_", b'e'), b"SETUP.EXE");
        assert_eq!(
            expanded_name(b"c:\\dir.x\\setup.ex_", b'E'),
            b"c:\\dir.x\\setup.exe"
        );
        assert_eq!(expanded_name(b"file.", b'x'), b"file");
        assert_eq!(expanded_name(b"noext", b'x'), b"noext");
        assert_eq!(expanded_name(b"a.b_c", b'x'), b"a.b_c");
    }
}
