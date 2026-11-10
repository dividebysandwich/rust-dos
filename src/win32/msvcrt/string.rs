//! Strings, memory, characters and numbers: the mem*, str*, wcs* and
//! _mbs* functions, the character classes, the conversions between text
//! and numbers, and qsort, bsearch and _lfind, which call the program's
//! compare function.
//!
//! The multibyte functions treat every code page as a single-byte one:
//! Windows 95 in the West runs 1252, where they are the str* functions.

use super::format;
use super::*;

const EOF: u32 = u32::MAX;

// Memory.

/// memcpy(to, from, count) and memmove: copies that overlap work both.
pub fn memmove(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[to, from, count] = args else {
        unreachable!()
    };
    if !cpu.bus.guest_probe(to, count as usize, true) {
        return None;
    }
    let data = bytes(cpu, from, count)?;
    put(cpu, to, &data)?;
    Some(to)
}

/// memset(to, byte, count).
pub fn memset(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if !cpu.bus.guest_probe(args[0], args[2] as usize, true) {
        return None;
    }
    cpu.bus.guest_fill(args[0], args[2] as usize, args[1] as u8);
    (!cpu.bus.guest_faulted()).then_some(args[0])
}

/// The sign of a comparison as C returns it.
fn order(o: std::cmp::Ordering) -> u32 {
    o as i32 as u32
}

/// `count` bytes in pieces of a page or less, as (offset, length), so
/// that memchr and memcmp read little past where they stop, as C's do.
fn pieces(count: u32) -> impl Iterator<Item = (u32, u32)> {
    (0..count).step_by(0x1000).map(move |at| (at, (count - at).min(0x1000)))
}

/// memcmp(a, b, count).
pub fn memcmp(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    for (at, len) in pieces(args[2]) {
        let a = bytes(cpu, args[0].wrapping_add(at), len)?;
        let b = bytes(cpu, args[1].wrapping_add(at), len)?;
        if a != b {
            return Some(order(a.cmp(&b)));
        }
    }
    Some(0)
}

/// _memicmp(a, b, count).
pub fn memicmp(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    for (at, len) in pieces(args[2]) {
        let a = lower(&bytes(cpu, args[0].wrapping_add(at), len)?);
        let b = lower(&bytes(cpu, args[1].wrapping_add(at), len)?);
        if a != b {
            return Some(order(a.cmp(&b)));
        }
    }
    Some(0)
}

/// memchr(at, byte, count).
pub fn memchr(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    for (at, len) in pieces(args[2]) {
        let data = bytes(cpu, args[0].wrapping_add(at), len)?;
        if let Some(i) = data.iter().position(|&b| b == args[1] as u8) {
            return Some(args[0].wrapping_add(at + i as u32));
        }
    }
    Some(0)
}

/// _memccpy(to, from, byte, count): copies up to and with `byte`; the
/// address after it in `to`, or NULL if it wasn't there.
pub fn memccpy(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[to, from, byte, count] = args else {
        unreachable!()
    };
    let data = bytes(cpu, from, count)?;
    match data.iter().position(|&b| b == byte as u8) {
        Some(i) => {
            put(cpu, to, &data[..=i])?;
            Some(to + i as u32 + 1)
        }
        None => {
            put(cpu, to, &data)?;
            Some(0)
        }
    }
}

/// _swab(from, to, count): each pair of bytes swapped.
pub fn swab(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let count = args[2] as i32;
    if count <= 1 {
        return Some(0);
    }
    let mut data = bytes(cpu, args[0], count as u32 & !1)?;
    for pair in data.chunks_mut(2) {
        pair.swap(0, 1);
    }
    put(cpu, args[1], &data)?;
    Some(0)
}

// Narrow strings.

fn lower(s: &[u8]) -> Vec<u8> {
    s.iter().map(u8::to_ascii_lowercase).collect()
}

/// strlen(s).
pub fn strlen(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(cstr(cpu, args[0])?.len() as u32)
}

/// strcpy(to, from) and _mbscpy.
pub fn strcpy(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let s = cstr(cpu, args[1])?;
    put_cstr(cpu, args[0], &s)?;
    Some(args[0])
}

/// strncpy(to, from, count): zeros after the string to `count`.
pub fn strncpy(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if !cpu.bus.guest_probe(args[0], args[2] as usize, true) {
        return None;
    }
    let mut s = cstr_n(cpu, args[1], args[2] as usize)?;
    s.resize(args[2] as usize, 0);
    put(cpu, args[0], &s)?;
    Some(args[0])
}

/// strcat(to, from).
pub fn strcat(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let len = cstr(cpu, args[0])?.len() as u32;
    let s = cstr(cpu, args[1])?;
    put_cstr(cpu, args[0] + len, &s)?;
    Some(args[0])
}

/// strncat(to, from, count): at most `count` bytes, and a 0.
pub fn strncat(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let len = cstr(cpu, args[0])?.len() as u32;
    let s = cstr_n(cpu, args[1], args[2] as usize)?;
    put_cstr(cpu, args[0] + len, &s)?;
    Some(args[0])
}

/// strcmp(a, b).
pub fn strcmp(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let a = cstr(cpu, args[0])?;
    let b = cstr(cpu, args[1])?;
    Some(order(a.cmp(&b)))
}

/// strncmp(a, b, count).
pub fn strncmp(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let a = cstr_n(cpu, args[0], args[2] as usize)?;
    let b = cstr_n(cpu, args[1], args[2] as usize)?;
    Some(order(a.cmp(&b)))
}

/// _stricmp(a, b), _strcmpi and _mbsicmp: lowercase both.
pub fn stricmp(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let a = lower(&cstr(cpu, args[0])?);
    let b = lower(&cstr(cpu, args[1])?);
    Some(order(a.cmp(&b)))
}

/// _strnicmp(a, b, count).
pub fn strnicmp(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let a = lower(&cstr_n(cpu, args[0], args[2] as usize)?);
    let b = lower(&cstr_n(cpu, args[1], args[2] as usize)?);
    Some(order(a.cmp(&b)))
}

/// strchr(s, byte): the first, or the string's 0 for 0.
pub fn strchr(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let s = cstr(cpu, args[0])?;
    let c = args[1] as u8;
    if c == 0 {
        return Some(args[0] + s.len() as u32);
    }
    Some(
        s.iter()
            .position(|&b| b == c)
            .map_or(0, |i| args[0] + i as u32),
    )
}

/// strrchr(s, byte): the last.
pub fn strrchr(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let s = cstr(cpu, args[0])?;
    let c = args[1] as u8;
    if c == 0 {
        return Some(args[0] + s.len() as u32);
    }
    Some(
        s.iter()
            .rposition(|&b| b == c)
            .map_or(0, |i| args[0] + i as u32),
    )
}

/// strstr(s, part).
pub fn strstr(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let s = cstr(cpu, args[0])?;
    let part = cstr(cpu, args[1])?;
    if part.is_empty() {
        return Some(args[0]);
    }
    let found = s.windows(part.len()).position(|w| w == part.as_slice());
    Some(found.map_or(0, |i| args[0] + i as u32))
}

/// strspn(s, set): how many of the first bytes are in the set.
pub fn strspn(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let s = cstr(cpu, args[0])?;
    let set = cstr(cpu, args[1])?;
    Some(s.iter().take_while(|b| set.contains(b)).count() as u32)
}

/// strcspn(s, set): how many are not.
pub fn strcspn(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let s = cstr(cpu, args[0])?;
    let set = cstr(cpu, args[1])?;
    Some(s.iter().take_while(|b| !set.contains(b)).count() as u32)
}

/// strpbrk(s, set): the first byte in the set.
pub fn strpbrk(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let s = cstr(cpu, args[0])?;
    let set = cstr(cpu, args[1])?;
    Some(
        s.iter()
            .position(|b| set.contains(b))
            .map_or(0, |i| args[0] + i as u32),
    )
}

/// strtok(s, delimiters) and _mbstok: the next token, its end made 0;
/// the place after it kept for the thread.
pub fn strtok(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let place = thread_field(cpu, T_STRTOK)?;
    let start = if args[0] != 0 {
        args[0]
    } else {
        read32(cpu, place)?
    };
    if start == 0 {
        return Some(0);
    }
    let set = cstr(cpu, args[1])?;
    let s = cstr(cpu, start)?;
    let skip = s.iter().take_while(|b| set.contains(b)).count();
    if skip == s.len() {
        write32(cpu, place, 0)?;
        return Some(0);
    }
    let token = start + skip as u32;
    let len = s[skip..].iter().take_while(|b| !set.contains(b)).count();
    let end = token + len as u32;
    if skip + len < s.len() {
        put(cpu, end, &[0])?;
        write32(cpu, place, end + 1)?;
    } else {
        write32(cpu, place, end)?;
    }
    Some(token)
}

/// _strdup(s): a copy from malloc's heap.
pub fn strdup(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        return Some(0);
    }
    let s = cstr(cpu, args[0])?;
    let Some(at) = heap_alloc(cpu, s.len() as u32 + 1) else {
        return Some(0);
    };
    put_cstr(cpu, at, &s)?;
    Some(at)
}

/// Change the string at `at` in place with `f`.
fn map_in_place(cpu: &mut Cpu, at: u32, f: impl Fn(u8) -> u8) -> Option<u32> {
    let s: Vec<u8> = cstr(cpu, at)?.into_iter().map(f).collect();
    put(cpu, at, &s)?;
    Some(at)
}

/// _strlwr(s), _strupr(s).
pub fn strlwr(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    map_in_place(cpu, args[0], |b| b.to_ascii_lowercase())
}

pub fn strupr(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    map_in_place(cpu, args[0], |b| b.to_ascii_uppercase())
}

/// _strset(s, byte): every byte made `byte`.
pub fn strset(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    map_in_place(cpu, args[0], |_| args[1] as u8)
}

/// _strnset(s, byte, count).
pub fn strnset(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let s = cstr_n(cpu, args[0], args[2] as usize)?;
    put(cpu, args[0], &vec![args[1] as u8; s.len()])?;
    Some(args[0])
}

/// _strrev(s).
pub fn strrev(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mut s = cstr(cpu, args[0])?;
    s.reverse();
    put(cpu, args[0], &s)?;
    Some(args[0])
}

/// strxfrm(to, from, count): in the "C" locale a copy; the length.
pub fn strxfrm(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let s = cstr(cpu, args[1])?;
    if (s.len() as u32) < args[2] {
        put_cstr(cpu, args[0], &s)?;
    }
    Some(s.len() as u32)
}

/// strerror(errno): the message, in the thread's buffer.
pub fn strerror(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let text = error_text(args[0]);
    let at = thread_field(cpu, T_SCRATCH)?;
    put_cstr(cpu, at, text.as_bytes())?;
    Some(at)
}

/// _strerror(prefix): "prefix: message\n", or the message and a line
/// feed.
pub fn strerror_prefixed(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let errno_at = thread_field(cpu, T_ERRNO)?;
    let errno = read32(cpu, errno_at)?;
    let mut text = Vec::new();
    if args[0] != 0 {
        let prefix = cstr_n(cpu, args[0], 94)?;
        text.extend_from_slice(&prefix);
        text.extend_from_slice(b": ");
    }
    text.extend_from_slice(error_text(errno).as_bytes());
    text.push(b'\n');
    let at = thread_field(cpu, T_SCRATCH)?;
    put_cstr(cpu, at, &text)?;
    Some(at)
}

/// perror(prefix): "prefix: message" on stderr.
pub fn perror(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let errno_at = thread_field(cpu, T_ERRNO)?;
    let errno = read32(cpu, errno_at)?;
    let mut text = Vec::new();
    if args[0] != 0 {
        let prefix = cstr(cpu, args[0])?;
        if !prefix.is_empty() {
            text.extend_from_slice(&prefix);
            text.extend_from_slice(b": ");
        }
    }
    text.extend_from_slice(error_text(errno).as_bytes());
    text.push(b'\n');
    super::io::write_fd(cpu, 2, &text);
    Some(0)
}

// Wide strings.

fn wlower(c: u16) -> u16 {
    match c {
        0x41..=0x5A | 0xC0..=0xD6 | 0xD8..=0xDE => c + 32,
        _ => c,
    }
}

fn wupper(c: u16) -> u16 {
    match c {
        0x61..=0x7A | 0xE0..=0xF6 | 0xF8..=0xFE => c - 32,
        0xFF => 0x178,
        _ => c,
    }
}

/// wcslen(s).
pub fn wcslen(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(wstr(cpu, args[0])?.len() as u32)
}

pub fn wcscpy(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let s = wstr(cpu, args[1])?;
    put_wstr(cpu, args[0], &s)?;
    Some(args[0])
}

pub fn wcsncpy(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let room = (args[2] as usize).saturating_mul(2);
    if !cpu.bus.guest_probe(args[0], room, true) {
        return None;
    }
    let mut s = wstr_n(cpu, args[1], args[2] as usize)?;
    s.resize(args[2] as usize, 0);
    let data: Vec<u8> = s.iter().flat_map(|c| c.to_le_bytes()).collect();
    put(cpu, args[0], &data)?;
    Some(args[0])
}

pub fn wcscat(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let len = wstr(cpu, args[0])?.len() as u32;
    let s = wstr(cpu, args[1])?;
    put_wstr(cpu, args[0] + 2 * len, &s)?;
    Some(args[0])
}

pub fn wcsncat(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let len = wstr(cpu, args[0])?.len() as u32;
    let s = wstr_n(cpu, args[1], args[2] as usize)?;
    put_wstr(cpu, args[0] + 2 * len, &s)?;
    Some(args[0])
}

pub fn wcscmp(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let a = wstr(cpu, args[0])?;
    let b = wstr(cpu, args[1])?;
    Some(order(a.cmp(&b)))
}

pub fn wcsncmp(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let a = wstr_n(cpu, args[0], args[2] as usize)?;
    let b = wstr_n(cpu, args[1], args[2] as usize)?;
    Some(order(a.cmp(&b)))
}

pub fn wcsicmp(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let a: Vec<u16> = wstr(cpu, args[0])?.into_iter().map(wlower).collect();
    let b: Vec<u16> = wstr(cpu, args[1])?.into_iter().map(wlower).collect();
    Some(order(a.cmp(&b)))
}

pub fn wcsnicmp(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let n = args[2] as usize;
    let a: Vec<u16> = wstr_n(cpu, args[0], n)?.into_iter().map(wlower).collect();
    let b: Vec<u16> = wstr_n(cpu, args[1], n)?.into_iter().map(wlower).collect();
    Some(order(a.cmp(&b)))
}

pub fn wcschr(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let s = wstr(cpu, args[0])?;
    let c = args[1] as u16;
    if c == 0 {
        return Some(args[0] + 2 * s.len() as u32);
    }
    Some(
        s.iter()
            .position(|&x| x == c)
            .map_or(0, |i| args[0] + 2 * i as u32),
    )
}

pub fn wcsrchr(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let s = wstr(cpu, args[0])?;
    let c = args[1] as u16;
    if c == 0 {
        return Some(args[0] + 2 * s.len() as u32);
    }
    Some(
        s.iter()
            .rposition(|&x| x == c)
            .map_or(0, |i| args[0] + 2 * i as u32),
    )
}

pub fn wcsstr(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let s = wstr(cpu, args[0])?;
    let part = wstr(cpu, args[1])?;
    if part.is_empty() {
        return Some(args[0]);
    }
    let found = s.windows(part.len()).position(|w| w == part.as_slice());
    Some(found.map_or(0, |i| args[0] + 2 * i as u32))
}

pub fn wcsspn(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let s = wstr(cpu, args[0])?;
    let set = wstr(cpu, args[1])?;
    Some(s.iter().take_while(|c| set.contains(c)).count() as u32)
}

pub fn wcscspn(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let s = wstr(cpu, args[0])?;
    let set = wstr(cpu, args[1])?;
    Some(s.iter().take_while(|c| !set.contains(c)).count() as u32)
}

pub fn wcspbrk(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let s = wstr(cpu, args[0])?;
    let set = wstr(cpu, args[1])?;
    Some(
        s.iter()
            .position(|c| set.contains(c))
            .map_or(0, |i| args[0] + 2 * i as u32),
    )
}

/// wcstok(s, delimiters).
pub fn wcstok(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let place = thread_field(cpu, T_WCSTOK)?;
    let start = if args[0] != 0 {
        args[0]
    } else {
        read32(cpu, place)?
    };
    if start == 0 {
        return Some(0);
    }
    let set = wstr(cpu, args[1])?;
    let s = wstr(cpu, start)?;
    let skip = s.iter().take_while(|c| set.contains(c)).count();
    if skip == s.len() {
        write32(cpu, place, 0)?;
        return Some(0);
    }
    let token = start + 2 * skip as u32;
    let len = s[skip..].iter().take_while(|c| !set.contains(c)).count();
    let end = token + 2 * len as u32;
    if skip + len < s.len() {
        put(cpu, end, &[0, 0])?;
        write32(cpu, place, end + 2)?;
    } else {
        write32(cpu, place, end)?;
    }
    Some(token)
}

pub fn wcsdup(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        return Some(0);
    }
    let s = wstr(cpu, args[0])?;
    let Some(at) = heap_alloc(cpu, 2 * s.len() as u32 + 2) else {
        return Some(0);
    };
    put_wstr(cpu, at, &s)?;
    Some(at)
}

fn wmap_in_place(cpu: &mut Cpu, at: u32, f: impl Fn(u16) -> u16) -> Option<u32> {
    let s: Vec<u16> = wstr(cpu, at)?.into_iter().map(f).collect();
    put_wstr(cpu, at, &s)?;
    Some(at)
}

pub fn wcslwr(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    wmap_in_place(cpu, args[0], wlower)
}

pub fn wcsupr(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    wmap_in_place(cpu, args[0], wupper)
}

pub fn wcsrev(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mut s = wstr(cpu, args[0])?;
    s.reverse();
    put_wstr(cpu, args[0], &s)?;
    Some(args[0])
}

pub fn wcsset(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    wmap_in_place(cpu, args[0], |_| args[1] as u16)
}

pub fn wcscoll(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    wcscmp(cpu, args)
}

pub fn towlower(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(wlower(args[0] as u16) as u32)
}

pub fn towupper(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(wupper(args[0] as u16) as u32)
}

// Character classes.

/// The classes of the character `c` (EOF, or 0 to 255).
fn class(c: u32) -> u16 {
    if c <= 0xFF { ctype_of(c as u8) } else { 0 }
}

/// The class functions, by the bits they test.
fn is(c: u32, mask: u16) -> Option<u32> {
    Some((class(c) & mask) as u32)
}

pub fn isalpha(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    is(args[0], UPPER | LOWER)
}

pub fn isupper(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    is(args[0], UPPER)
}

pub fn islower(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    is(args[0], LOWER)
}

pub fn isdigit(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    is(args[0], DIGIT)
}

pub fn isxdigit(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    is(args[0], HEX)
}

pub fn isspace(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    is(args[0], SPACE)
}

pub fn ispunct(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    is(args[0], PUNCT)
}

pub fn isalnum(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    is(args[0], UPPER | LOWER | DIGIT)
}

pub fn isprint(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    is(args[0], UPPER | LOWER | DIGIT | PUNCT | BLANK)
}

pub fn isgraph(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    is(args[0], UPPER | LOWER | DIGIT | PUNCT)
}

pub fn iscntrl(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    is(args[0], CONTROL)
}

/// _isctype(c, mask).
pub fn isctype(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    is(args[0], args[1] as u16)
}

pub fn isascii(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some((args[0] < 0x80) as u32)
}

pub fn toascii(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(args[0] & 0x7F)
}

/// __iscsym(c): a letter, digit or underscore; __iscsymf: no digit.
pub fn iscsym(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    let c = args[0];
    Some((c == b'_' as u32 || class(c) & (UPPER | LOWER | DIGIT) != 0) as u32)
}

pub fn iscsymf(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    let c = args[0];
    Some((c == b'_' as u32 || class(c) & (UPPER | LOWER) != 0) as u32)
}

/// toupper(c) and tolower(c): other characters as they are.
pub fn toupper(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    let c = args[0];
    Some(if (b'a' as u32..=b'z' as u32).contains(&c) {
        c - 32
    } else {
        c
    })
}

pub fn tolower(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    let c = args[0];
    Some(if (b'A' as u32..=b'Z' as u32).contains(&c) {
        c + 32
    } else {
        c
    })
}

/// _toupper(c) and _tolower(c): no check.
pub fn toupper_fast(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(args[0].wrapping_sub(32))
}

pub fn tolower_fast(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(args[0].wrapping_add(32))
}

/// The wide class of `c`.
fn wclass(c: u32) -> u16 {
    match c {
        0..=0xFF => wctype_of(c as u16),
        0x100..=0xFFFF => match char::from_u32(c) {
            Some(ch) if ch.is_uppercase() => UPPER | ALPHA,
            Some(ch) if ch.is_lowercase() => LOWER | ALPHA,
            Some(ch) if ch.is_alphabetic() => ALPHA,
            Some(ch) if ch.is_numeric() => DIGIT,
            Some(ch) if ch.is_whitespace() => SPACE,
            Some(_) => PUNCT,
            None => 0,
        },
        _ => 0,
    }
}

/// iswctype(c, mask) and is_wctype, and the isw* functions.
pub fn iswctype(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some((wclass(args[0] & 0xFFFF) & args[1] as u16) as u32)
}

fn wis(c: u32, mask: u16) -> Option<u32> {
    if c & 0xFFFF == 0xFFFF {
        return Some(0);
    }
    Some((wclass(c & 0xFFFF) & mask) as u32)
}

pub fn iswalpha(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    wis(args[0], ALPHA)
}

pub fn iswupper(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    wis(args[0], UPPER)
}

pub fn iswlower(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    wis(args[0], LOWER)
}

pub fn iswdigit(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    wis(args[0], DIGIT)
}

pub fn iswxdigit(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    wis(args[0], HEX)
}

pub fn iswspace(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    wis(args[0], SPACE)
}

pub fn iswpunct(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    wis(args[0], PUNCT)
}

pub fn iswalnum(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    wis(args[0], ALPHA | DIGIT)
}

pub fn iswprint(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    wis(args[0], ALPHA | DIGIT | PUNCT | BLANK)
}

pub fn iswgraph(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    wis(args[0], ALPHA | DIGIT | PUNCT)
}

pub fn iswcntrl(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    wis(args[0], CONTROL)
}

pub fn iswascii(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some((args[0] & 0xFFFF < 0x80) as u32)
}

// Numbers from text.

fn space16(c: u16) -> bool {
    c == b' ' as u16 || (9..=13).contains(&c)
}

/// An integer at the start of `s` in `base` (0: by its prefix): its value
/// as the bits of a u64 (negated if '-'), how many characters it took
/// (0 if none were digits), and whether it went past `limit`, the most
/// a positive or negative value of the type can be.
struct Parsed {
    value: u64,
    negative: bool,
    used: usize,
    overflow: bool,
}

fn parse_int(s: &[u16], base: u32, limit: u64) -> Parsed {
    let mut i = 0;
    while i < s.len() && space16(s[i]) {
        i += 1;
    }
    let mut negative = false;
    if i < s.len() && (s[i] == b'-' as u16 || s[i] == b'+' as u16) {
        negative = s[i] == b'-' as u16;
        i += 1;
    }
    let mut base = base;
    let x = |c: u16| c == b'x' as u16 || c == b'X' as u16;
    if (base == 0 || base == 16)
        && s.get(i) == Some(&(b'0' as u16))
        && s.get(i + 1).is_some_and(|&c| x(c))
    {
        i += 2;
        base = 16;
    } else if base == 0 {
        base = if s.get(i) == Some(&(b'0' as u16)) {
            8
        } else {
            10
        };
    }
    let mut value: u64 = 0;
    let mut overflow = false;
    let start = i;
    if !(2..=36).contains(&base) {
        return Parsed {
            value: 0,
            negative: false,
            used: 0,
            overflow: false,
        };
    }
    while i < s.len() {
        let Some(d) = char::from_u32(s[i] as u32).and_then(|c| c.to_digit(base)) else {
            break;
        };
        match value
            .checked_mul(base as u64)
            .and_then(|v| v.checked_add(d as u64))
        {
            Some(v) if v <= limit => value = v,
            _ => {
                overflow = true;
                value = limit;
            }
        }
        i += 1;
    }
    if i == start {
        return Parsed {
            value: 0,
            negative: false,
            used: 0,
            overflow: false,
        };
    }
    Parsed {
        value,
        negative,
        used: i,
        overflow,
    }
}

fn narrow_units(s: &[u8]) -> Vec<u16> {
    s.iter().map(|&b| b as u16).collect()
}

/// strtol and wcstol on `s` at `at` (characters of `size` bytes): the
/// value, the end written to `end`.
fn to_long(
    cpu: &mut Cpu,
    s: &[u16],
    at: u32,
    size: u32,
    end: u32,
    base: u32,
    signed: bool,
) -> Option<u32> {
    let limit = if signed { 0x8000_0000 } else { 0xFFFF_FFFF };
    let p = parse_int(s, base, limit);
    if end != 0 {
        write32(cpu, end, at + size * p.used as u32)?;
    }
    if signed {
        let max = if p.negative { 0x8000_0000 } else { 0x7FFF_FFFF };
        if p.overflow || p.value > max {
            set_errno(cpu, ERANGE);
            return Some(if p.negative { 0x8000_0000 } else { 0x7FFF_FFFF });
        }
        let v = p.value as i64;
        return Some((if p.negative { -v } else { v }) as u32);
    }
    if p.overflow {
        set_errno(cpu, ERANGE);
        return Some(u32::MAX);
    }
    let v = p.value as u32;
    Some(if p.negative { v.wrapping_neg() } else { v })
}

/// strtol(s, end, base).
pub fn strtol(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let s = narrow_units(&cstr(cpu, args[0])?);
    to_long(cpu, &s, args[0], 1, args[1], args[2], true)
}

pub fn strtoul(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let s = narrow_units(&cstr(cpu, args[0])?);
    to_long(cpu, &s, args[0], 1, args[1], args[2], false)
}

pub fn wcstol(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let s = wstr(cpu, args[0])?;
    to_long(cpu, &s, args[0], 2, args[1], args[2], true)
}

pub fn wcstoul(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let s = wstr(cpu, args[0])?;
    to_long(cpu, &s, args[0], 2, args[1], args[2], false)
}

/// The value atoi gives: digits past the type's range wrap.
fn wrapping_int(s: &[u16]) -> u64 {
    let p = parse_int(s, 10, u64::MAX);
    if p.negative {
        p.value.wrapping_neg()
    } else {
        p.value
    }
}

/// atoi(s), atol(s), _wtoi, _wtol.
pub fn atoi(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let s = narrow_units(&cstr(cpu, args[0])?);
    Some(wrapping_int(&s) as u32)
}

pub fn wtoi(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let s = wstr(cpu, args[0])?;
    Some(wrapping_int(&s) as u32)
}

/// _atoi64(s) and _wtoi64(s).
pub fn atoi64(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let s = narrow_units(&cstr(cpu, args[0])?);
    ret_u64(cpu, wrapping_int(&s))
}

pub fn wtoi64(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let s = wstr(cpu, args[0])?;
    ret_u64(cpu, wrapping_int(&s))
}

/// A double at the start of `s`: its value and how many characters it
/// took. MSVCRT takes 'd' and 'D' for the exponent too, and no "inf" or
/// "nan".
fn parse_double(s: &[u16]) -> (f64, usize) {
    let c = |i: usize| {
        s.get(i)
            .map_or(0u8, |&c| if c < 0x80 { c as u8 } else { 0xFF })
    };
    let mut i = 0;
    while i < s.len() && space16(s[i]) {
        i += 1;
    }
    let mut text = String::new();
    if c(i) == b'-' || c(i) == b'+' {
        text.push(c(i) as char);
        i += 1;
    }
    let mut digits = 0;
    while c(i).is_ascii_digit() {
        text.push(c(i) as char);
        i += 1;
        digits += 1;
    }
    if c(i) == b'.' {
        text.push('.');
        i += 1;
        while c(i).is_ascii_digit() {
            text.push(c(i) as char);
            i += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return (0.0, 0);
    }
    if matches!(c(i), b'e' | b'E' | b'd' | b'D') {
        let mut j = i + 1;
        let mut exp = String::from("e");
        if c(j) == b'-' || c(j) == b'+' {
            exp.push(c(j) as char);
            j += 1;
        }
        if c(j).is_ascii_digit() {
            while c(j).is_ascii_digit() {
                exp.push(c(j) as char);
                j += 1;
            }
            text += &exp;
            i = j;
        }
    }
    let text = text.trim_end_matches('.').to_string();
    let text = if text.ends_with(['-', '+']) || text.is_empty() {
        text + "0"
    } else {
        text
    };
    (text.replace(".e", "e").parse().unwrap_or(0.0), i)
}

/// strtod and wcstod: the double, ERANGE when it overflows.
fn to_double(cpu: &mut Cpu, s: &[u16], at: u32, size: u32, end: u32) -> Option<u32> {
    let (v, used) = parse_double(s);
    if end != 0 {
        write32(cpu, end, at + size * used as u32)?;
    }
    if v.is_infinite() {
        set_errno(cpu, ERANGE);
    }
    ret_f64(cpu, v)
}

pub fn strtod(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let s = narrow_units(&cstr(cpu, args[0])?);
    to_double(cpu, &s, args[0], 1, args[1])
}

pub fn wcstod(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let s = wstr(cpu, args[0])?;
    to_double(cpu, &s, args[0], 2, args[1])
}

pub fn atof(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let s = narrow_units(&cstr(cpu, args[0])?);
    ret_f64(cpu, parse_double(&s).0)
}

// Numbers to text.

/// The digits of `value` in `base`, '-' first if `negative`.
fn digits_of(mut value: u64, base: u32, negative: bool) -> Vec<u8> {
    let base = base.clamp(2, 36) as u64;
    let mut out = Vec::new();
    loop {
        let d = (value % base) as u8;
        out.push(if d < 10 { b'0' + d } else { b'a' + d - 10 });
        value /= base;
        if value == 0 {
            break;
        }
    }
    if negative {
        out.push(b'-');
    }
    out.reverse();
    out
}

/// Write the digits to `buffer`, narrow or wide.
fn give_digits(cpu: &mut Cpu, buffer: u32, digits: &[u8], wide: bool) -> Option<u32> {
    if wide {
        let w: Vec<u16> = digits.iter().map(|&b| b as u16).collect();
        put_wstr(cpu, buffer, &w)?;
    } else {
        put_cstr(cpu, buffer, digits)?;
    }
    Some(buffer)
}

/// The text of a signed 32-bit value: negative only in base 10.
fn signed_digits(value: u32, base: u32) -> Vec<u8> {
    let v = value as i32;
    if base == 10 && v < 0 {
        digits_of(v.unsigned_abs() as u64, 10, true)
    } else {
        digits_of(value as u64, base, false)
    }
}

/// _itoa(value, buffer, base) and _ltoa.
pub fn itoa(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    give_digits(cpu, args[1], &signed_digits(args[0], args[2]), false)
}

pub fn ultoa(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    give_digits(
        cpu,
        args[1],
        &digits_of(args[0] as u64, args[2], false),
        false,
    )
}

pub fn itow(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    give_digits(cpu, args[1], &signed_digits(args[0], args[2]), true)
}

pub fn ultow(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    give_digits(
        cpu,
        args[1],
        &digits_of(args[0] as u64, args[2], false),
        true,
    )
}

/// _i64toa(value (two words), buffer, base) and _ui64toa.
pub fn i64toa(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let v = (args[1] as u64) << 32 | args[0] as u64;
    let digits = if args[3] == 10 && (v as i64) < 0 {
        digits_of((v as i64).unsigned_abs(), 10, true)
    } else {
        digits_of(v, args[3], false)
    };
    give_digits(cpu, args[2], &digits, false)
}

pub fn ui64toa(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let v = (args[1] as u64) << 32 | args[0] as u64;
    give_digits(cpu, args[2], &digits_of(v, args[3], false), false)
}

pub fn i64tow(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let v = (args[1] as u64) << 32 | args[0] as u64;
    let digits = if args[3] == 10 && (v as i64) < 0 {
        digits_of((v as i64).unsigned_abs(), 10, true)
    } else {
        digits_of(v, args[3], false)
    };
    give_digits(cpu, args[2], &digits, true)
}

pub fn ui64tow(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let v = (args[1] as u64) << 32 | args[0] as u64;
    give_digits(cpu, args[2], &digits_of(v, args[3], false), true)
}

/// Room for _ecvt's and _fcvt's digits in the thread's block.
const CVT_ROOM: usize = (T_WASCTIME - T_CVT) as usize - 1;

/// Give the digits from _ecvt or _fcvt, with the point and sign.
fn give_cvt(cpu: &mut Cpu, digits: &[u8], decpt: i32, negative: bool, args: &[u32]) -> Option<u32> {
    write32(cpu, args[3], decpt as u32)?;
    write32(cpu, args[4], negative as u32)?;
    let at = thread_field(cpu, T_CVT)?;
    put_cstr(cpu, at, &digits[..digits.len().min(CVT_ROOM)])?;
    Some(at)
}

/// The count of _ecvt or _fcvt, within what printf would give.
fn cvt_count(args: &[u32]) -> i32 {
    let most = format::MAX_PRECISION as i32;
    (args[2] as i32).clamp(-most, most)
}

/// _ecvt or _fcvt of infinity or a NaN: printf's text without the point,
/// with the point after its first digit.
fn cvt_special(cpu: &mut Cpu, v: f64, count: i32, args: &[u32]) -> Option<u32> {
    let text = format::special(v, count.max(1) as usize - 1).replace('.', "");
    give_cvt(cpu, text.as_bytes(), 1, v.is_sign_negative(), args)
}

/// _ecvt(value, count, decpt, sign): `count` significant digits.
pub fn ecvt(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let v = f64_arg(args, 0);
    let count = cvt_count(args);
    if !v.is_finite() {
        return cvt_special(cpu, v, count, args);
    }
    let negative = v.is_sign_negative() && v != 0.0;
    if v == 0.0 {
        let digits = vec![b'0'; count.max(0) as usize];
        return give_cvt(cpu, &digits, 0, negative, args);
    }
    if count <= 0 {
        // No digits: where the point would be, after the first digit
        // rounds when there are none at all.
        let (digits, decpt) = format::exp_digits(v, 17);
        let decpt = if count == 0 && digits[0] >= b'5' {
            decpt + 1
        } else {
            decpt
        };
        return give_cvt(cpu, &[], decpt, negative, args);
    }
    let (digits, decpt) = format::exp_digits(v, count as usize);
    give_cvt(cpu, &digits, decpt, negative, args)
}

/// _fcvt(value, count, decpt, sign): digits to `count` after the point
/// (before it, when negative).
pub fn fcvt(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let v = f64_arg(args, 0);
    let count = cvt_count(args);
    if !v.is_finite() {
        return cvt_special(cpu, v, count, args);
    }
    let negative = v.is_sign_negative() && v != 0.0;
    if v == 0.0 {
        let digits = vec![b'0'; count.max(0) as usize];
        return give_cvt(cpu, &digits, 0, negative, args);
    }
    let (digits, decpt) = if count >= 0 {
        format::fixed_digits(v, count as usize)
    } else {
        let (digits, decpt) = format::exp_digits(v, 17);
        let keep = decpt + count;
        if keep < 0 {
            (Vec::new(), decpt)
        } else {
            format::round_digits(&digits, decpt, keep as usize)
        }
    };
    if digits.is_empty() {
        // Rounded to nothing: the point is where the value's is.
        let (_, magnitude) = format::exp_digits(v, 1);
        let decpt = if count < 0 { decpt } else { magnitude };
        return give_cvt(cpu, &[], decpt, negative, args);
    }
    give_cvt(cpu, &digits, decpt, negative, args)
}

/// _gcvt(value, count, buffer): as %.*g.
pub fn gcvt(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let v = f64_arg(args, 0);
    let count = args[2] as i32;
    if count < 0 {
        set_errno(cpu, ERANGE);
        return Some(0);
    }
    let count = (count as usize).min(format::MAX_PRECISION);
    let mut text = if v.is_finite() {
        format::g_text(v.abs(), count, false, false)
    } else {
        format::special(v, count.max(1) - 1)
    };
    if v.is_sign_negative() && v != 0.0 {
        text.insert(0, '-');
    }
    put_cstr(cpu, args[3], text.as_bytes())?;
    Some(args[3])
}

// Integer arithmetic.

pub fn abs(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some((args[0] as i32).wrapping_abs() as u32)
}

/// div(a, b) and ldiv: the struct's two words are returned in EDX:EAX.
pub fn div(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (a, b) = (args[0] as i32, args[1] as i32);
    if b == 0 {
        return ret_u64(cpu, 0);
    }
    let q = a.wrapping_div(b) as u32 as u64;
    let r = a.wrapping_rem(b) as u32 as u64;
    ret_u64(cpu, r << 32 | q)
}

/// _abs64 (VC 6 has llabs as it).
pub fn abs64(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let v = ((args[1] as u64) << 32 | args[0] as u64) as i64;
    ret_u64(cpu, v.wrapping_abs() as u64)
}

/// _rotl(value, shift), _rotr, _lrotl, _lrotr.
pub fn rotl(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(args[0].rotate_left(args[1] & 31))
}

pub fn rotr(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(args[0].rotate_right(args[1] & 31))
}

/// rand(): MSVCRT's generator, per thread.
pub fn rand(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let at = thread_field(cpu, T_RAND)?;
    let seed = read32(cpu, at)?.wrapping_mul(214013).wrapping_add(2531011);
    write32(cpu, at, seed)?;
    Some((seed >> 16) & 0x7FFF)
}

pub fn srand(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = thread_field(cpu, T_RAND)?;
    write32(cpu, at, args[0])?;
    Some(0)
}

// Multibyte characters of the locale, which has one byte each (mbcs.rs
// has the code page of the _mbs functions).

/// mblen(s, count): 0 for "" or NULL, 1 else, -1 if count is 0.
pub fn mblen(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        return Some(0);
    }
    let b = cstr_n(cpu, args[0], 1)?;
    if b.is_empty() {
        return Some(0);
    }
    Some(if args[1] == 0 { EOF } else { 1 })
}

/// mbtowc(to, s, count).
pub fn mbtowc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[1] == 0 {
        return Some(0);
    }
    if args[2] == 0 {
        return Some(EOF);
    }
    let b = cstr_n(cpu, args[1], 1)?;
    let c = b.first().map_or(0, |&b| widen(b));
    if args[0] != 0 {
        put(cpu, args[0], &c.to_le_bytes())?;
    }
    Some((c != 0) as u32)
}

/// wctomb(to, c).
pub fn wctomb(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        return Some(0);
    }
    match narrow(args[1] as u16) {
        Some(b) => {
            put(cpu, args[0], &[b])?;
            Some(1)
        }
        None => {
            set_errno(cpu, 42);
            Some(EOF)
        }
    }
}

/// mbstowcs(to, from, count): the characters, without the 0.
pub fn mbstowcs(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let s = cstr(cpu, args[1])?;
    let w: Vec<u16> = s.iter().map(|&b| widen(b)).collect();
    if args[0] == 0 {
        return Some(w.len() as u32);
    }
    let n = (args[2] as usize).min(w.len());
    let mut out: Vec<u8> = w[..n].iter().flat_map(|c| c.to_le_bytes()).collect();
    if n < args[2] as usize {
        out.extend([0, 0]);
    }
    put(cpu, args[0], &out)?;
    Some(n as u32)
}

/// wcstombs(to, from, count): the bytes, without the 0; -1 if one
/// character has none.
pub fn wcstombs(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let w = wstr(cpu, args[1])?;
    let mut s = Vec::with_capacity(w.len());
    for &c in &w {
        match narrow(c) {
            Some(b) => s.push(b),
            None => return Some(EOF),
        }
    }
    if args[0] == 0 {
        return Some(s.len() as u32);
    }
    let n = (args[2] as usize).min(s.len());
    if n < args[2] as usize {
        put_cstr(cpu, args[0], &s[..n])?;
    } else {
        put(cpu, args[0], &s[..n])?;
    }
    Some(n as u32)
}

/// _mbsupr and _mbslwr.
pub fn mbsupr(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    strupr(cpu, args)
}

// Sorting and searching, with the program's compare function.

/// qsort(base, count, width, compare): a quicksort whose comparisons are
/// calls into the program, made in the order Visual C++ 6's qsort makes
/// them. It isn't stable, so elements that compare equal end up in the
/// order its steps leave them, and a program that sorts such elements
/// sees the same order as with Microsoft's DLL. A range of more than 8
/// elements is split around its middle element, which goes to its start
/// first; the smaller part goes on at once and the larger waits on a
/// stack. A range of up to 8 is sorted by moving its largest element to
/// its end, then the largest of the rest, and so on (`QSort`). The order
/// of comparisons and the results were checked against a log of
/// MSVCRT.DLL 6.00.8337's comparisons: 518 sorts and searches of 0 to 257
/// elements, all the same.
pub fn qsort(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[base, count, width, compare] = args else {
        unreachable!()
    };
    if count < 2 || width == 0 {
        return Some(0);
    }
    // An array that reaches the end of the address space isn't taken:
    // the split's high end starts one element past the last, which would
    // wrap to 0 there.
    let size = count as u64 * width as u64;
    if base as u64 + size >= 1 << 32 || !cpu.bus.guest_probe(base, size as usize, true) {
        return None;
    }
    let resume = call::here(cpu);
    let last = base + (count - 1) * width;
    let mut s = QSort {
        width,
        compare,
        step: QSort::START_RANGE,
        first: base,
        last,
        low_scan: 0,
        high_scan: 0,
        largest: 0,
        candidate: 0,
        stack: Vec::new(),
    };
    s.run(cpu, resume, None);
    None
}

pub fn qsort_returned(cpu: &mut Cpu, value: u32, call: Call) {
    let Some(mut s) = QSort::from_words(&call.data) else {
        return;
    };
    s.run(cpu, call.resume, Some(value as i32));
}

/// Where a qsort is: the range it sorts (`first` to `last`, the addresses
/// of its first and last elements), the step it takes, the elements the
/// split has reached scanning up from the low end and down from the high
/// end (`low_scan`, `high_scan`), the largest element found yet and the
/// one it is compared with (`largest`, `candidate`), and the ranges
/// waiting (`stack`, first and last each).
struct QSort {
    width: u32,
    compare: u32,
    step: u32,
    first: u32,
    last: u32,
    low_scan: u32,
    high_scan: u32,
    largest: u32,
    candidate: u32,
    stack: Vec<u32>,
}

impl QSort {
    /// Sort the range `first` to `last`.
    const START_RANGE: u32 = 0;
    /// Up to 8 elements: compare element `candidate` with the largest yet.
    const FIND_LARGEST: u32 = 1;
    /// Splitting: element `low_scan` was compared with the middle one,
    /// which is at `first`.
    const SCAN_UP: u32 = 2;
    /// Element `high_scan` was compared with it.
    const SCAN_DOWN: u32 = 3;
    /// The array is sorted.
    const FINISHED: u32 = 4;

    fn words(&self) -> Vec<u32> {
        let mut words = vec![
            self.width,
            self.compare,
            self.step,
            self.first,
            self.last,
            self.low_scan,
            self.high_scan,
            self.largest,
            self.candidate,
        ];
        words.extend_from_slice(&self.stack);
        words
    }

    fn from_words(words: &[u32]) -> Option<QSort> {
        let (head, stack) = words.split_at_checked(9)?;
        let &[
            width,
            compare,
            step,
            first,
            last,
            low_scan,
            high_scan,
            largest,
            candidate,
        ] = head
        else {
            return None;
        };
        Some(QSort {
            width,
            compare,
            step,
            first,
            last,
            low_scan,
            high_scan,
            largest,
            candidate,
            stack: stack.to_vec(),
        })
    }

    /// Go on to the next comparison, with the result of the last one
    /// (`result`), and call the program's compare function for it, or
    /// return from qsort when the array is sorted.
    fn run(&mut self, cpu: &mut Cpu, resume: (u32, u32), mut result: Option<i32>) {
        let w = self.width;
        loop {
            let compared = result.take();
            let pair = match self.step {
                Self::START_RANGE => {
                    let size = (self.last - self.first) / w + 1;
                    if size <= 8 {
                        // The largest of first..=last goes to last, then
                        // the largest of the rest.
                        if self.last > self.first {
                            (self.largest, self.candidate) = (self.first, self.first + w);
                            self.step = Self::FIND_LARGEST;
                            Some((self.candidate, self.largest))
                        } else {
                            self.pop_range()
                        }
                    } else {
                        swap(cpu, self.first + size / 2 * w, self.first, w);
                        (self.low_scan, self.high_scan) = (self.first + w, self.last + w);
                        self.step = Self::SCAN_UP;
                        self.scan_up(cpu)
                    }
                }
                Self::FIND_LARGEST => {
                    if compared.is_some_and(|c| c > 0) {
                        self.largest = self.candidate;
                    }
                    self.candidate += w;
                    if self.candidate <= self.last {
                        Some((self.candidate, self.largest))
                    } else {
                        swap(cpu, self.largest, self.last, w);
                        self.last -= w;
                        if self.last > self.first {
                            (self.largest, self.candidate) = (self.first, self.first + w);
                            Some((self.candidate, self.largest))
                        } else {
                            self.pop_range()
                        }
                    }
                }
                Self::SCAN_UP => {
                    if compared.is_some_and(|c| c <= 0) {
                        self.low_scan += w;
                        self.scan_up(cpu)
                    } else {
                        self.high_scan -= w;
                        self.step = Self::SCAN_DOWN;
                        self.scan_down(cpu)
                    }
                }
                Self::SCAN_DOWN => {
                    if compared.is_some_and(|c| c >= 0) {
                        self.high_scan -= w;
                        self.scan_down(cpu)
                    } else {
                        self.scans_stopped(cpu)
                    }
                }
                // FINISHED, or a step no qsort saves: there is nothing to do.
                _ => {
                    self.step = Self::FINISHED;
                    None
                }
            };
            match pair {
                Some((a, b)) => {
                    let words = self.words();
                    if !call_program(cpu, self.compare, &[a, b], C_QSORT, resume, &words) {
                        call::finish(cpu, resume, 0);
                    }
                    return;
                }
                None if self.step == Self::START_RANGE => continue,
                None => {
                    call::finish(cpu, resume, 0);
                    return;
                }
            }
        }
    }

    /// Splitting, from the low end: the next element to compare with the
    /// middle one, or once past `last`, on from the high end.
    fn scan_up(&mut self, cpu: &mut Cpu) -> Option<(u32, u32)> {
        if self.low_scan <= self.last {
            return Some((self.low_scan, self.first));
        }
        self.high_scan -= self.width;
        self.step = Self::SCAN_DOWN;
        self.scan_down(cpu)
    }

    /// Splitting, from the high end: the next element to compare with the
    /// middle one, or once at `first`, `scans_stopped`.
    fn scan_down(&mut self, cpu: &mut Cpu) -> Option<(u32, u32)> {
        if self.high_scan > self.first {
            return Some((self.high_scan, self.first));
        }
        self.scans_stopped(cpu)
    }

    /// Both scans stopped. If they haven't crossed, the elements where
    /// they stopped trade places and the scan up goes on; if they have,
    /// the middle element goes between the two parts and the next range
    /// is sorted.
    fn scans_stopped(&mut self, cpu: &mut Cpu) -> Option<(u32, u32)> {
        let w = self.width;
        if self.high_scan >= self.low_scan {
            swap(cpu, self.low_scan, self.high_scan, w);
            self.low_scan += w;
            self.step = Self::SCAN_UP;
            return self.scan_up(cpu);
        }
        swap(cpu, self.first, self.high_scan, w);
        let (first, last, low_scan, high_scan) =
            (self.first, self.last, self.low_scan, self.high_scan);
        // Sizes in bytes, as the pointers' differences: last - low_scan
        // is negative when the scan up went past last.
        let low_part = high_scan as i64 - 1 - first as i64;
        let high_part = last as i64 - low_scan as i64;
        if low_part >= high_part {
            if first + w < high_scan {
                self.stack.extend([first, high_scan - w]);
            }
            if low_scan < last {
                self.first = low_scan;
                self.step = Self::START_RANGE;
                return None;
            }
        } else {
            if low_scan < last {
                self.stack.extend([low_scan, last]);
            }
            if first + w < high_scan {
                self.last = high_scan - w;
                self.step = Self::START_RANGE;
                return None;
            }
        }
        self.pop_range()
    }

    /// The range that waited longest, to sort next, or None (and a step
    /// that ends qsort) when there is none.
    fn pop_range(&mut self) -> Option<(u32, u32)> {
        match (self.stack.pop(), self.stack.pop()) {
            (Some(last), Some(first)) => {
                (self.first, self.last) = (first, last);
                self.step = Self::START_RANGE;
            }
            _ => self.step = Self::FINISHED,
        }
        None
    }
}

/// Swap the `width` bytes at `a` with those at `b`.
fn swap(cpu: &mut Cpu, a: u32, b: u32, width: u32) {
    if a == b {
        return;
    }
    if let (Some(x), Some(y)) = (bytes(cpu, a, width), bytes(cpu, b, width)) {
        put(cpu, a, &y);
        put(cpu, b, &x);
    }
}

/// bsearch(key, base, count, width, compare): the matching element, or
/// NULL, with the comparisons Visual C++ 6's bsearch makes, so that of
/// elements that compare equal it finds the same one: each step compares
/// the key with the middle one of the elements left, the lower of the two
/// middle ones when an even number is left, and the last one left is
/// compared on its own. The comparisons and the elements found were
/// checked against a log of MSVCRT.DLL 6.00.8337's, in the 518 sorts and
/// searches qsort's comment counts, all the same. The call's data: key,
/// the first and last elements left, how many are left, width and
/// compare.
pub fn bsearch(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[key, base, count, width, compare] = args else {
        unreachable!()
    };
    let resume = call::here(cpu);
    let last = base.wrapping_add(count.wrapping_sub(1).wrapping_mul(width));
    bsearch_step(cpu, resume, [key, base, last, count, width, compare]);
    None
}

/// Of the `left` elements from `first` on, the one `bsearch` compares the
/// key with next, and how many come before it. The address wraps as a C
/// pointer's does, for a count and width whose array doesn't fit in the
/// address space.
fn bsearch_next(first: u32, left: u32, width: u32) -> (u32, u32) {
    let below = left.saturating_sub(1) / 2;
    (first.wrapping_add(below.wrapping_mul(width)), below)
}

fn bsearch_step(cpu: &mut Cpu, resume: (u32, u32), s: [u32; 6]) {
    let [key, first, last, left, width, compare] = s;
    if left == 0 || first > last {
        call::finish(cpu, resume, 0);
        return;
    }
    let (middle, _) = bsearch_next(first, left, width);
    if !call_program(cpu, compare, &[key, middle], C_BSEARCH, resume, &s) {
        call::finish(cpu, resume, 0);
    }
}

pub fn bsearch_returned(cpu: &mut Cpu, value: u32, call: Call) {
    let Ok(mut s) = <[u32; 6]>::try_from(call.data.as_slice()) else {
        return;
    };
    let [_, first, _, left, width, _] = s;
    let (middle, below) = bsearch_next(first, left, width);
    match (value as i32).cmp(&0) {
        std::cmp::Ordering::Equal => {
            call::finish(cpu, call.resume, middle);
            return;
        }
        // The last one left wasn't it.
        _ if left == 1 => s[3] = 0,
        std::cmp::Ordering::Less => {
            s[2] = middle.wrapping_sub(width);
            s[3] = below;
        }
        std::cmp::Ordering::Greater => {
            s[1] = middle.wrapping_add(width);
            s[3] = left.saturating_sub(below + 1);
        }
    }
    bsearch_step(cpu, call.resume, s);
}

/// _lfind(key, base, count, width, compare): the first element compare
/// says is equal (returns 0), or NULL. _lsearch adds the key at the end
/// when there is none. The call's data: key, base, the element, the
/// count's address, width, compare, and 1 for _lsearch.
pub fn lfind(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    lfind_start(cpu, args, 0)
}

pub fn lsearch(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    lfind_start(cpu, args, 1)
}

fn lfind_start(cpu: &mut Cpu, args: &[u32], add: u32) -> Option<u32> {
    let &[key, base, count_at, width, compare] = args else {
        unreachable!()
    };
    read32(cpu, count_at)?;
    let resume = call::here(cpu);
    lfind_step(cpu, resume, [key, base, 0, count_at, width, compare, add]);
    None
}

fn lfind_step(cpu: &mut Cpu, resume: (u32, u32), s: [u32; 7]) {
    let [key, base, i, count_at, width, compare, add] = s;
    let count = cpu.bus.guest_read_32(count_at);
    if i >= count {
        let mut found = 0;
        if add != 0 {
            let to = base + count * width;
            if let Some(data) = bytes(cpu, key, width) {
                put(cpu, to, &data);
                cpu.bus.guest_write_32(count_at, count + 1);
                found = to;
            }
        }
        call::finish(cpu, resume, found);
        return;
    }
    if !call_program(cpu, compare, &[key, base + i * width], C_LFIND, resume, &s) {
        call::finish(cpu, resume, 0);
    }
}

pub fn lfind_returned(cpu: &mut Cpu, value: u32, call: Call) {
    let Ok(mut s) = <[u32; 7]>::try_from(call.data.as_slice()) else {
        return;
    };
    if value == 0 {
        call::finish(cpu, call.resume, s[1] + s[2] * s[4]);
        return;
    }
    s[2] += 1;
    lfind_step(cpu, call.resume, s);
}

// Paths.

/// _makepath(path, drive, dir, name, ext) and the wide one.
fn make_path(parts: [Option<Vec<u16>>; 4]) -> Vec<u16> {
    let [drive, dir, name, ext] = parts;
    let mut out: Vec<u16> = Vec::new();
    let slash = |c: u16| c == b'\\' as u16 || c == b'/' as u16;
    if let Some(drive) = drive.filter(|d| !d.is_empty()) {
        out.push(drive[0]);
        out.push(b':' as u16);
    }
    if let Some(dir) = dir.filter(|d| !d.is_empty()) {
        out.extend_from_slice(&dir);
        if !slash(*dir.last().unwrap()) {
            out.push(b'\\' as u16);
        }
    }
    if let Some(name) = name {
        out.extend_from_slice(&name);
    }
    if let Some(ext) = ext.filter(|e| !e.is_empty()) {
        if ext[0] != b'.' as u16 {
            out.push(b'.' as u16);
        }
        out.extend_from_slice(&ext);
    }
    out
}

pub fn makepath(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mut parts: [Option<Vec<u16>>; 4] = Default::default();
    for (i, part) in parts.iter_mut().enumerate() {
        if args[i + 1] != 0 {
            *part = Some(narrow_units(&cstr(cpu, args[i + 1])?));
        }
    }
    let path: Vec<u8> = make_path(parts).iter().map(|&c| c as u8).collect();
    put_cstr(cpu, args[0], &path)?;
    Some(0)
}

pub fn wmakepath(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mut parts: [Option<Vec<u16>>; 4] = Default::default();
    for (i, part) in parts.iter_mut().enumerate() {
        if args[i + 1] != 0 {
            *part = Some(wstr(cpu, args[i + 1])?);
        }
    }
    put_wstr(cpu, args[0], &make_path(parts))?;
    Some(0)
}

/// _splitpath(path, drive, dir, name, ext): each part that has a buffer.
pub fn splitpath(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let path = cstr(cpu, args[0])?;
    let (drive, rest) = if path.len() >= 2 && path[1] == b':' {
        (&path[..2], &path[2..])
    } else {
        (&path[..0], &path[..])
    };
    let cut = rest
        .iter()
        .rposition(|&b| b == b'\\' || b == b'/')
        .map_or(0, |i| i + 1);
    let (dir, file) = rest.split_at(cut);
    let dot = file.iter().rposition(|&b| b == b'.').unwrap_or(file.len());
    let (name, ext) = file.split_at(dot);
    for (at, part) in [
        (args[1], drive),
        (args[2], dir),
        (args[3], name),
        (args[4], ext),
    ] {
        if at != 0 {
            put_cstr(cpu, at, part)?;
        }
    }
    Some(0)
}
