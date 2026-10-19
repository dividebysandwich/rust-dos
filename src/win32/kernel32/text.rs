//! Text between code pages and Unicode (UTF-16), which Windows 95 converts
//! though it has few other Unicode functions; the code pages themselves;
//! and the lstr functions on strings.

use super::{ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_PARAMETER, FALSE, TRUE, set_error};
use crate::cpu::Cpu;
use crate::win32::convert::{self, ANSI_CODE_PAGE, CodePage, OEM_CODE_PAGE};

/// Error codes of the text and locale functions.
pub(super) const ERROR_INVALID_FLAGS: u32 = 1004;

/// MultiByteToWideChar's flags.
const MB_PRECOMPOSED: u32 = 0x01;
const MB_COMPOSITE: u32 = 0x02;
const MB_USEGLYPHCHARS: u32 = 0x04;
const MB_ERR_INVALID_CHARS: u32 = 0x08;

/// WideCharToMultiByte's flags.
const WC_DISCARDNS: u32 = 0x10;
const WC_SEPCHARS: u32 = 0x20;
const WC_DEFAULTCHAR: u32 = 0x40;
const WC_COMPOSITECHECK: u32 = 0x200;
const WC_NO_BEST_FIT_CHARS: u32 = 0x400;

/// How far a string is read looking for its end before it counts as
/// having none.
const LONGEST: usize = 0x10_0000;

/// The `len` units of the string at `at`, or with -1 (any negative
/// length) up to and with its terminating zero. `unit` is its units'
/// size, 1 or 2 bytes.
pub(super) fn read_units(cpu: &mut Cpu, at: u32, len: u32, unit: u32) -> Option<Vec<u16>> {
    let read = |cpu: &mut Cpu, i: u32| match unit {
        1 => cpu.bus.guest_read_8(at.wrapping_add(i)) as u16,
        _ => cpu.bus.guest_read_16(at.wrapping_add(2 * i)),
    };
    let mut units = Vec::new();
    if (len as i32) < 0 {
        loop {
            let c = read(cpu, units.len() as u32);
            units.push(c);
            if c == 0 || cpu.bus.guest_faulted() || units.len() > LONGEST {
                break;
            }
        }
    } else {
        units.extend((0..len).map(|i| read(cpu, i)));
    }
    (!cpu.bus.guest_faulted()).then_some(units)
}

/// The bytes of the string at `at` up to its zero, without it.
pub(super) fn read_string(cpu: &mut Cpu, at: u32) -> Option<Vec<u8>> {
    let bytes = cpu.bus.guest_read_asciiz(at, LONGEST);
    (!cpu.bus.guest_faulted()).then_some(bytes)
}

/// The UTF-16 string at `at` up to its zero, without it.
pub(super) fn read_wide_string(cpu: &mut Cpu, at: u32) -> Option<Vec<u16>> {
    let mut units = read_units(cpu, at, u32::MAX, 2)?;
    if units.last() == Some(&0) {
        units.pop();
    }
    Some(units)
}

/// ANSI text (code page 1252) as UTF-16, and back, a character the code
/// page lacks becoming '?'.
pub(super) fn ansi_to_wide(bytes: &[u8]) -> Vec<u16> {
    bytes.iter().map(|&b| CodePage::Ansi.char(b)).collect()
}

pub(super) fn wide_to_ansi(units: &[u16]) -> Vec<u8> {
    units
        .iter()
        .map(|&c| CodePage::Ansi.byte(c).unwrap_or(b'?'))
        .collect()
}

/// Write `items` (bytes or UTF-16 units, `unit` bytes each) to the buffer
/// at `to` of `size` items, as the conversion functions do: with a size of
/// 0 only their count; when they don't fit, as many as fit, and an error.
pub(super) fn put_units(
    cpu: &mut Cpu,
    to: u32,
    size: u32,
    items: &[u16],
    unit: u32,
) -> Option<u32> {
    if size == 0 {
        return Some(items.len() as u32);
    }
    let fit = items.len().min(size as usize);
    for (i, &c) in items[..fit].iter().enumerate() {
        match unit {
            1 => cpu.bus.guest_write_8(to.wrapping_add(i as u32), c as u8),
            _ => cpu.bus.guest_write_16(to.wrapping_add(2 * i as u32), c),
        }
    }
    if cpu.bus.guest_faulted() {
        return None;
    }
    if fit < items.len() {
        set_error(cpu, ERROR_INSUFFICIENT_BUFFER);
        return Some(0);
    }
    Some(items.len() as u32)
}

/// Whether a conversion's arguments are wrong: no source, an empty one, a
/// negative size or no buffer for a size.
fn bad_arguments(from: u32, len: u32, to: u32, size: u32) -> bool {
    from == 0 || len == 0 || (size as i32) < 0 || (size != 0 && to == 0)
}

/// The UTF-16 string `args[2]` (`args[3]` units, or -1) in the code page
/// `args[0]`, into `args[4]` (`args[5]` bytes; 0 asks for the size). A
/// character the code page lacks becomes the default character `args[6]`
/// points to, or '?', and sets the BOOL `args[7]` points to. With
/// WC_COMPOSITECHECK a letter and a combining mark become the accented
/// letter, if the code page has it; the mark is dropped with WC_DISCARDNS.
pub(super) fn wide_char_to_multi_byte(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[code_page, flags, from, len, to, size, default, used_default] = args else {
        unreachable!()
    };
    let Some(page) = CodePage::of(code_page) else {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    };
    let known =
        WC_DISCARDNS | WC_SEPCHARS | WC_DEFAULTCHAR | WC_COMPOSITECHECK | WC_NO_BEST_FIT_CHARS;
    if flags & !known != 0 {
        set_error(cpu, ERROR_INVALID_FLAGS);
        return Some(0);
    }
    if bad_arguments(from, len, to, size) {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    let mut units = read_units(cpu, from, len, 2)?;
    if flags & WC_COMPOSITECHECK != 0 {
        let mut composed: Vec<u16> = Vec::with_capacity(units.len());
        for c in units {
            let previous = composed.last().copied();
            if convert::is_combining(c) {
                if let Some(p) = previous.and_then(|p| convert::compose(p, c)) {
                    *composed.last_mut().unwrap() = p;
                    continue;
                }
                if flags & WC_DISCARDNS != 0 {
                    continue;
                }
            }
            composed.push(c);
        }
        units = composed;
    }
    let default = match default {
        0 => b'?',
        at => cpu.bus.guest_read_8(at),
    };
    if cpu.bus.guest_faulted() {
        return None;
    }
    // Only the characters written count for the default's use.
    let written = match size {
        0 => units.len(),
        _ => units.len().min(size as usize),
    };
    let mut used = false;
    let bytes: Vec<u16> = units
        .iter()
        .enumerate()
        .map(|(i, &c)| {
            page.byte(c).unwrap_or_else(|| {
                used |= i < written;
                default
            }) as u16
        })
        .collect();
    if used_default != 0 {
        cpu.bus.guest_write_32(used_default, used as u32);
    }
    put_units(cpu, to, size, &bytes, 1)
}

/// The string `args[2]` (`args[3]` bytes, or -1) of the code page `args[0]`
/// as UTF-16, into `args[4]` (`args[5]` units; 0 asks for the size). With
/// MB_COMPOSITE accented letters become a letter and a combining mark;
/// with MB_USEGLYPHCHARS the OEM control characters become their glyphs.
pub(super) fn multi_byte_to_wide_char(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[code_page, flags, from, len, to, size] = args else {
        unreachable!()
    };
    let Some(page) = CodePage::of(code_page) else {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    };
    let known = MB_PRECOMPOSED | MB_COMPOSITE | MB_USEGLYPHCHARS | MB_ERR_INVALID_CHARS;
    let both = MB_PRECOMPOSED | MB_COMPOSITE;
    if flags & !known != 0 || flags & both == both {
        set_error(cpu, ERROR_INVALID_FLAGS);
        return Some(0);
    }
    if bad_arguments(from, len, to, size) {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    let mut units = Vec::new();
    for b in read_units(cpu, from, len, 1)? {
        let c = match flags & MB_USEGLYPHCHARS {
            0 => page.char(b as u8),
            _ => page.glyph(b as u8),
        };
        match convert::decompose(c).filter(|_| flags & MB_COMPOSITE != 0) {
            Some((base, mark)) => units.extend([base, mark]),
            None => units.push(c),
        }
    }
    put_units(cpu, to, size, &units, 2)
}

/// The ANSI code page, as on a US Windows 95.
pub(super) fn get_acp(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(ANSI_CODE_PAGE)
}

/// The OEM code page, DOS's.
pub(super) fn get_oem_cp(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(OEM_CODE_PAGE)
}

/// Code pages 437, 850 and 1252, the ones there are.
pub(super) fn is_valid_code_page(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some((args[0] > 1 && CodePage::of(args[0]).is_some()) as u32)
}

/// CPINFO: characters of one byte, '?' for those the code page lacks, and
/// no lead bytes.
pub(super) fn get_cp_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if CodePage::of(args[0]).is_none() || args[1] == 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    let mut info = [0u8; 18];
    info[0] = 1;
    info[4] = b'?';
    cpu.bus.guest_write_bytes(args[1], &info);
    Some(TRUE)
}

/// The code pages there are have no double-byte characters.
pub(super) fn is_dbcs_lead_byte(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(FALSE)
}

pub(super) fn is_dbcs_lead_byte_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if CodePage::of(args[0]).is_none() {
        set_error(cpu, ERROR_INVALID_PARAMETER);
    }
    Some(FALSE)
}

/// The length of the string at `args[0]`, 0 for NULL.
pub(super) fn lstrlen_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        return Some(0);
    }
    Some(read_string(cpu, args[0])?.len() as u32)
}

pub(super) fn lstrlen_w(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        return Some(0);
    }
    Some(read_wide_string(cpu, args[0])?.len() as u32)
}

/// Copy the string at `args[1]` to `args[0]`: `args[0]`, or NULL when
/// either is NULL.
pub(super) fn lstrcpy_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[to, from] = args else { unreachable!() };
    if to == 0 || from == 0 {
        return Some(0);
    }
    let mut bytes = read_string(cpu, from)?;
    bytes.push(0);
    cpu.bus.guest_write_bytes(to, &bytes);
    Some(to)
}

/// Copy at most `args[2]` - 1 characters of the string at `args[1]` to
/// `args[0]`, and a zero if there is room for one.
pub(super) fn lstrcpyn_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[to, from, n] = args else { unreachable!() };
    if to == 0 || from == 0 {
        return Some(0);
    }
    if (n as i32) <= 0 {
        return Some(to);
    }
    let mut bytes = cpu.bus.guest_read_asciiz(from, n as usize - 1);
    if cpu.bus.guest_faulted() {
        return None;
    }
    bytes.push(0);
    cpu.bus.guest_write_bytes(to, &bytes);
    Some(to)
}

/// Add the string at `args[1]` to the end of the one at `args[0]`.
pub(super) fn lstrcat_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[to, from] = args else { unreachable!() };
    if to == 0 || from == 0 {
        return Some(0);
    }
    let end = read_string(cpu, to)?.len() as u32;
    let mut bytes = read_string(cpu, from)?;
    bytes.push(0);
    cpu.bus.guest_write_bytes(to.wrapping_add(end), &bytes);
    Some(to)
}

/// lstrcmpA and lstrcmpiA: CompareStringA's answer for the system's
/// locale, less 2: negative, 0 or positive. Windows 95 can't compare
/// NULL, which makes it -2.
fn lstrcmp(cpu: &mut Cpu, args: &[u32], flags: u32) -> Option<u32> {
    if args[0] == 0 || args[1] == 0 {
        return Some(-2i32 as u32);
    }
    let a = ansi_to_wide(&read_string(cpu, args[0])?);
    let b = ansi_to_wide(&read_string(cpu, args[1])?);
    Some(super::sort::compare(flags, &a, &b) as i32 as u32)
}

pub(super) fn lstrcmp_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    lstrcmp(cpu, args, 0)
}

pub(super) fn lstrcmpi_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    lstrcmp(cpu, args, super::sort::NORM_IGNORECASE)
}
