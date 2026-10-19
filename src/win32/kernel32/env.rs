//! The environment block and its expansion, the computer's name, the
//! debugger's output, the performance counter and MulDiv.

use super::text::read_string;
use super::{ERROR_ENVVAR_NOT_FOUND, ERROR_INVALID_PARAMETER, FALSE, TRUE, set_error};
use crate::cpu::Cpu;
use crate::win32::{process, with_process};

/// The computer's name, which Windows 95 keeps in the registry.
const COMPUTER_NAME: &[u8] = b"RUSTDOS";
const MAX_COMPUTERNAME_LENGTH: u32 = 15;
const ERROR_BUFFER_OVERFLOW: u32 = 111;

/// The rate of the performance counter: the PIT's, as on Windows 95.
const PERFORMANCE_FREQUENCY: u64 = 1_193_182;

/// The value of the environment variable `name`, its name's case
/// ignored.
fn value_of(cpu: &mut Cpu, name: &[u8]) -> Option<Vec<u8>> {
    let name = crate::dosstr::from_bytes(name);
    process(cpu).environment.iter().find_map(|s| {
        let (n, v) = s.split_once('=')?;
        n.eq_ignore_ascii_case(&name)
            .then(|| crate::dosstr::to_bytes(v))
    })
}

/// GetEnvironmentStrings(A): a copy of the environment, its strings one
/// after the other, each with its zero, and a zero after the last, in
/// the process's heap. NULL if there's no room.
pub(super) fn get_environment_strings(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let mut block = Vec::new();
    for s in &process(cpu).environment {
        block.extend(crate::dosstr::to_bytes(s));
        block.push(0);
    }
    block.push(0);
    if block.len() == 1 {
        block.push(0);
    }
    let size = block.len() as u32;
    let Some(at) = with_process(cpu, |p, bus| p.heap.alloc(&mut p.memory, bus, size)) else {
        set_error(cpu, super::ERROR_NOT_ENOUGH_MEMORY);
        return Some(0);
    };
    cpu.bus.guest_write_bytes(at, &block);
    Some(at)
}

/// FreeEnvironmentStringsA: the copy goes back to the heap.
pub(super) fn free_environment_strings_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    process(cpu).heap.free(args[0]);
    Some(TRUE)
}

/// The string `text` with each %NAME% of a variable there is replaced by
/// its value, once (values aren't expanded again); and whether a name
/// wasn't found.
pub(super) fn expand(cpu: &mut Cpu, text: &[u8]) -> (Vec<u8>, bool) {
    let mut out = Vec::new();
    let mut missing = false;
    let mut rest = text;
    while let Some(start) = rest.iter().position(|&b| b == b'%') {
        out.extend(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.iter().position(|&b| b == b'%') else {
            out.extend(&rest[start..]);
            rest = &[];
            break;
        };
        match value_of(cpu, &after[..end]) {
            Some(value) => {
                out.extend(value);
                rest = &after[end + 1..];
            }
            None => {
                missing = true;
                out.extend(&rest[start..start + end + 1]);
                rest = &after[end..];
            }
        }
    }
    out.extend(rest);
    (out, missing)
}

/// ExpandEnvironmentStringsA: the string `args[0]` expanded into
/// `args[1]` (`args[2]` bytes). Returns the size it needs with its zero,
/// and writes it only when it fits. Windows 95 returns 0 for no buffer,
/// and for no string.
pub(super) fn expand_environment_strings_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[src, to, size] = args else {
        unreachable!()
    };
    if src == 0 || to == 0 {
        return Some(0);
    }
    let text = read_string(cpu, src)?;
    let (mut out, missing) = expand(cpu, &text);
    out.push(0);
    if missing {
        set_error(cpu, ERROR_ENVVAR_NOT_FOUND);
    }
    let len = out.len() as u32;
    if len <= size {
        if !cpu.bus.guest_probe(to, out.len(), true) {
            return None;
        }
        cpu.bus.guest_write_bytes(to, &out);
    }
    Some(len)
}

/// GetComputerNameA: the name into `args[0]`, its length without the
/// zero into the DWORD at `args[1]`, which holds the buffer's size. With
/// too small a buffer, ERROR_BUFFER_OVERFLOW and the size as it was.
pub(super) fn get_computer_name_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[to, size_at] = args else { unreachable!() };
    let size = cpu.bus.guest_read_32(size_at);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let len = COMPUTER_NAME.len() as u32;
    if size <= len {
        set_error(cpu, ERROR_BUFFER_OVERFLOW);
        return Some(FALSE);
    }
    let mut name = COMPUTER_NAME.to_vec();
    name.push(0);
    if !cpu.bus.guest_probe(to, name.len(), true) {
        return None;
    }
    cpu.bus.guest_write_bytes(to, &name);
    cpu.bus.guest_write_32(size_at, len);
    Some(TRUE)
}

/// SetComputerNameA: Windows 95 takes a new name for the next start;
/// this checks it and keeps the old one.
pub(super) fn set_computer_name_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    let name = read_string(cpu, args[0])?;
    let bad = |b: &u8| b" \"/\\[]:|<>+=;,?*".contains(b) || *b < 0x20;
    if name.is_empty() || name.len() as u32 > MAX_COMPUTERNAME_LENGTH || name.iter().any(bad) {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    Some(TRUE)
}

/// OutputDebugStringA: with no debugger, the string goes to the log.
pub(super) fn output_debug_string_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] != 0 {
        let text = read_string(cpu, args[0])?;
        let text = crate::dosstr::from_bytes(&text);
        cpu.bus
            .log_string(&format!("[W32] debug: {}", text.trim_end()));
    }
    Some(0)
}

/// Beep: Windows 95 plays the default sound whatever the tone; there is
/// none here.
pub(super) fn beep(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(TRUE)
}

/// QueryPerformanceCounter: the machine's clock in PIT ticks.
pub(super) fn query_performance_counter(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let micros = cpu.bus.clock.now_micros() as u128;
    let ticks = (micros * PERFORMANCE_FREQUENCY as u128 / 1_000_000) as u64;
    write_u64(cpu, args[0], ticks)
}

pub(super) fn query_performance_frequency(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    write_u64(cpu, args[0], PERFORMANCE_FREQUENCY)
}

fn write_u64(cpu: &mut Cpu, at: u32, value: u64) -> Option<u32> {
    if !cpu.bus.guest_probe(at, 8, true) {
        return None;
    }
    cpu.bus.guest_write_32(at, value as u32);
    cpu.bus
        .guest_write_32(at.wrapping_add(4), (value >> 32) as u32);
    Some(TRUE)
}

/// MulDiv: a * b / c in 64 bits, rounded half away from zero; -1 when c
/// is 0 or the result doesn't fit.
pub(super) fn mul_div(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [a, b, c] = [args[0], args[1], args[2]].map(|v| v as i32 as i64);
    if c == 0 {
        return Some(u32::MAX);
    }
    let (a, c) = if c < 0 { (-a, -c) } else { (a, c) };
    let product = a * b;
    let result = if product < 0 {
        (product - c / 2) / c
    } else {
        (product + c / 2) / c
    };
    if !(-0x7FFF_FFFF..=0x7FFF_FFFF).contains(&result) {
        return Some(u32::MAX);
    }
    Some(result as i32 as u32)
}
