//! Text between code pages and Unicode (UTF-16), which Windows 95 converts
//! though it has few other Unicode functions.

use super::{ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_PARAMETER, set_error};
use crate::cpu::Cpu;
use crate::win32::convert::CodePage;

/// The `len` units of the string at `at`, or with -1 up to and with its
/// terminating zero. `unit` is its units' size, 1 or 2 bytes.
fn read_units(cpu: &mut Cpu, at: u32, len: u32, unit: u32) -> Option<Vec<u16>> {
    let read = |cpu: &mut Cpu, i: u32| match unit {
        1 => cpu.bus.guest_read_8(at + i) as u16,
        _ => cpu.bus.guest_read_16(at + 2 * i),
    };
    let mut units = Vec::new();
    if len == u32::MAX {
        loop {
            let c = read(cpu, units.len() as u32);
            units.push(c);
            if c == 0 || cpu.bus.guest_faulted() || units.len() > 0x10_0000 {
                break;
            }
        }
    } else {
        units.extend((0..len).map(|i| read(cpu, i)));
    }
    (!cpu.bus.guest_faulted()).then_some(units)
}

/// The UTF-16 string `args[2]` (`args[3]` units, or -1) in the code page
/// `args[0]`, into `args[4]` (`args[5]` bytes; 0 asks for the size). A
/// character the code page lacks becomes the default character `args[6]`
/// points to, or '?', and sets the BOOL `args[7]` points to.
pub(super) fn wide_char_to_multi_byte(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[code_page, _, from, len, to, size, default, used_default] = args else {
        unreachable!()
    };
    let Some(page) = CodePage::of(code_page) else {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    };
    let units = read_units(cpu, from, len, 2)?;
    let default = match default {
        0 => b'?',
        at => cpu.bus.guest_read_8(at),
    };
    let mut used = false;
    let bytes: Vec<u8> = units
        .iter()
        .map(|&c| {
            page.byte(c).unwrap_or_else(|| {
                used = true;
                default
            })
        })
        .collect();
    if used_default != 0 {
        cpu.bus.guest_write_32(used_default, used as u32);
    }
    if size == 0 {
        return Some(bytes.len() as u32);
    }
    if (size as usize) < bytes.len() {
        set_error(cpu, ERROR_INSUFFICIENT_BUFFER);
        return Some(0);
    }
    cpu.bus.guest_write_bytes(to, &bytes);
    Some(bytes.len() as u32)
}

/// The string `args[2]` (`args[3]` bytes, or -1) of the code page `args[0]`
/// as UTF-16, into `args[4]` (`args[5]` units; 0 asks for the size).
pub(super) fn multi_byte_to_wide_char(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[code_page, _, from, len, to, size] = args else {
        unreachable!()
    };
    let Some(page) = CodePage::of(code_page) else {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    };
    let units: Vec<u16> = read_units(cpu, from, len, 1)?
        .into_iter()
        .map(|b| page.char(b as u8))
        .collect();
    if size == 0 {
        return Some(units.len() as u32);
    }
    if (size as usize) < units.len() {
        set_error(cpu, ERROR_INSUFFICIENT_BUFFER);
        return Some(0);
    }
    for (i, &c) in units.iter().enumerate() {
        cpu.bus.guest_write_16(to + 2 * i as u32, c);
    }
    Some(units.len() as u32)
}
