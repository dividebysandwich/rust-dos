//! LZEXPAND: reading files that COMPRESS.EXE packed (SZDD), as the
//! setup programs of Windows 3.1's time do with files named like
//! SETUP.EX_. The format and the expansion are KERNEL32's (kernel32/
//! lz.rs, after Wine's lzexpand.c); here the files are the DOS handles
//! KERNEL's OpenFile gives.
//!
//! A handle LZInit gives is 400h and up, one of 16; other handles are
//! the files' own, which LZRead, LZSeek and LZClose pass on to _lread,
//! _llseek and _lclose. The file is expanded whole when LZInit opens it,
//! and reads come from the expanded bytes.

use super::{dos, global, kernel};
use crate::cpu::Cpu;
use crate::win32::kernel32::lz::{LzFile, expand, expanded_name, header};

/// LZERROR_*: what the functions return when they fail.
const LZERROR_BADINHANDLE: i32 = -1;
const LZERROR_READ: i32 = -3;
const LZERROR_WRITE: i32 = -4;
const LZERROR_GLOBALLOC: i32 = -5;
const LZERROR_BADVALUE: i32 = -7;
const LZERROR_UNKNOWNALG: i32 = -8;

/// The first LZ handle, and how many there may be.
const FIRST_HANDLE: u32 = 0x400;
const MAX_FILES: u32 = 16;
/// The header's size: the magic, the method, the name's last character
/// and the expanded size.
const HEADER: u32 = 14;
/// OpenFile's styles other than the sharing modes (70h): OF_READ is 0.
const OF_SHARE_MASK: u32 = 0x70;

/// The LZ file of the handle `handle`.
fn lz_file(cpu: &mut Cpu, handle: u32) -> Option<&mut LzFile> {
    let slot = handle.checked_sub(FIRST_HANDLE)?;
    cpu.bus
        .win32
        .win16
        .lz_files
        .iter_mut()
        .find(|f| f.slot == slot)
}

/// Move the pointer of the DOS file `handle` to `offset` from its start.
fn rewind(cpu: &mut Cpu, handle: u32, offset: u32) -> bool {
    kernel::llseek(cpu, &[handle, offset, 0]) != Some(0xFFFF_FFFF)
}

/// Up to `len` bytes of the DOS file `handle` from its pointer on, or
/// all of the rest with `len` None. None when it can't be read.
fn read(cpu: &mut Cpu, handle: u32, len: Option<u32>) -> Option<Vec<u8>> {
    const CHUNK: u32 = 0x8000;
    let block = global::alloc(cpu, 0, CHUNK, 0, false);
    if block == 0 {
        return None;
    }
    let at = super::lin(cpu, global::handle_to_sel(block), 0);
    let mut bytes = Vec::new();
    let result = loop {
        let want = len.map_or(CHUNK, |l| (l - bytes.len() as u32).min(CHUNK));
        if want == 0 {
            break Some(());
        }
        match dos::transfer(cpu, handle as u16, at, want, false) {
            Ok(n) => {
                bytes.extend(super::peek(cpu, at, n as usize).unwrap_or_default());
                if n < want {
                    break Some(());
                }
            }
            Err(_) => break None,
        }
    };
    global::free(cpu, block);
    result.map(|_| bytes)
}

/// Write `bytes` to the DOS file `handle`. False when not all of them
/// could be.
fn write(cpu: &mut Cpu, handle: u32, bytes: &[u8]) -> bool {
    const CHUNK: usize = 0x8000;
    let block = global::alloc(cpu, 0, CHUNK as u32, 0, false);
    if block == 0 {
        return false;
    }
    let at = super::lin(cpu, global::handle_to_sel(block), 0);
    let mut ok = true;
    for piece in bytes.chunks(CHUNK) {
        super::poke(cpu, at, piece);
        let n = dos::transfer(cpu, handle as u16, at, piece.len() as u32, true);
        if n != Ok(piece.len() as u32) {
            ok = false;
            break;
        }
    }
    global::free(cpu, block);
    ok
}

/// LZInit(file): an LZ handle for a compressed file; the file's own
/// handle, its pointer back at its start, for one that isn't.
fn init_handle(cpu: &mut Cpu, file: u32) -> i32 {
    // The header first: a file that isn't compressed isn't read whole.
    let head = if rewind(cpu, file, 0) {
        read(cpu, file, Some(HEADER))
    } else {
        None
    };
    let Some(head) = head else {
        return LZERROR_BADINHANDLE;
    };
    let Some((method, _, length)) = header(&head) else {
        rewind(cpu, file, 0);
        return file as i32;
    };
    if method != b'A' {
        rewind(cpu, file, 0);
        return LZERROR_UNKNOWNALG;
    }
    let Some(packed) = read(cpu, file, None) else {
        return LZERROR_BADINHANDLE;
    };
    let files = &mut cpu.bus.win32.win16.lz_files;
    let Some(slot) = (0..MAX_FILES).find(|s| files.iter().all(|f| f.slot != *s)) else {
        return LZERROR_GLOBALLOC;
    };
    files.push(LzFile {
        slot,
        file,
        length,
        wanted: 0,
        data: expand(&packed),
    });
    (FIRST_HANDLE + slot) as i32
}

pub(super) fn lz_init(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(init_handle(cpu, args[0]) as u32)
}

/// LZOpenFile(name, ofstruct, style): OpenFile's handle, through LZInit
/// when the file is opened for reading, so a compressed one reads
/// expanded.
pub(super) fn lz_open_file(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let handle = kernel::open_file(cpu, args)?;
    if handle & 0xFFFF == 0xFFFF || args[2] & !OF_SHARE_MASK & 0xFFFF != 0 {
        return Some(handle);
    }
    let lz = init_handle(cpu, handle);
    Some(if lz <= 0 { handle } else { lz as u32 })
}

/// LZRead(handle, buffer, count): the count read, LZERROR_READ past the
/// expanded end.
pub(super) fn lz_read(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (handle, buffer, count) = (args[0], args[1], args[2] & 0xFFFF);
    let Some(f) = lz_file(cpu, handle) else {
        return kernel::lread(cpu, args);
    };
    let end = (f.length as usize).min(f.data.len());
    let from = (f.wanted as usize).min(end);
    let n = count as usize;
    if n > 0 && from == end {
        return Some(LZERROR_READ as u32);
    }
    let bytes = f.data[from..end.min(from + n)].to_vec();
    f.wanted += bytes.len() as u32;
    super::poke(cpu, buffer, &bytes);
    Some(bytes.len() as u32)
}

/// LZSeek(handle, offset, origin): the new place, which can't be past
/// the expanded end.
pub(super) fn lz_seek(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (handle, offset, origin) = (args[0], args[1], args[2] & 0xFFFF);
    let Some(f) = lz_file(cpu, handle) else {
        return kernel::llseek(cpu, args);
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
/// compressed, written to `dest`: the count written, or an LZERROR.
pub(super) fn lz_copy(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (source, dest) = (args[0] & 0xFFFF, args[1] & 0xFFFF);
    let mut src = source;
    let mut opened = false;
    if lz_file(cpu, src).is_none() {
        let handle = init_handle(cpu, src);
        if handle <= 0 {
            return Some(0);
        }
        opened = handle as u32 != src;
        src = handle as u32;
    }
    let bytes = match lz_file(cpu, src) {
        Some(f) => {
            let end = (f.length as usize).min(f.data.len());
            let from = (f.wanted as usize).min(end);
            f.wanted = end as u32;
            f.data[from..end].to_vec()
        }
        None => match read(cpu, src, None) {
            Some(bytes) => bytes,
            None => return Some(LZERROR_READ as u32),
        },
    };
    let written = write(cpu, dest, &bytes);
    if opened {
        // As Windows does, the LZ handle LZCopy made is closed, and the
        // source file with it.
        lz_close(cpu, &[src]);
    }
    if !written {
        return Some(LZERROR_WRITE as u32);
    }
    Some(bytes.len() as u32)
}

/// LZClose(handle): the LZ handle and its file, or the file `handle`.
pub(super) fn lz_close(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let handle = args[0] & 0xFFFF;
    let Some(slot) = lz_file(cpu, handle).map(|f| f.slot) else {
        kernel::lclose(cpu, &[handle]);
        return Some(0);
    };
    let files = &mut cpu.bus.win32.win16.lz_files;
    let i = files.iter().position(|f| f.slot == slot)?;
    let f = files.remove(i);
    kernel::lclose(cpu, &[f.file]);
    Some(0)
}

/// GetExpandedName(name, out): the name the compressed file `name` had,
/// from the last character its header keeps ("SETUP.EX_" with 'E' is
/// "SETUP.EXE"); `name` itself for a file that isn't compressed. 1, or
/// LZERROR_BADINHANDLE when the file can't be opened.
pub(super) fn get_expanded_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mut name = super::string_bytes(cpu, args[0], 0x100);
    let Ok((handle, _)) = dos::call21(cpu, [0x3D00, 0, 0, 0], Some(&name)) else {
        return Some(LZERROR_BADINHANDLE as u32);
    };
    let head = read(cpu, handle as u32, Some(HEADER)).unwrap_or_default();
    kernel::lclose(cpu, &[handle as u32]);
    if let Some((_, last, _)) = header(&head) {
        name = expanded_name(&name, last);
    }
    name.push(0);
    super::poke(cpu, args[1], &name);
    Some(1)
}
