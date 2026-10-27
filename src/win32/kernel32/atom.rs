//! The global atom table's functions, which DDE's messages name their
//! applications, topics and items with. The table is the one Win16's
//! GlobalAddAtom keeps (src/win16/kernel.rs), as Windows 95 has one
//! global table for 16-bit and 32-bit programs.

use super::{ERROR_INVALID_PARAMETER, set_error, string_at};
use crate::cpu::Cpu;

const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_INVALID_HANDLE: u32 = 6;

/// The atom `name` (a string, or MAKEINTATOM's integer atom) has, added
/// with `add` or used once more: 0 when it isn't there or can't be.
fn atom(cpu: &mut Cpu, name: u32, add: bool) -> Option<u32> {
    if name < 0x1_0000 {
        if name == 0 || name >= 0xC000 {
            set_error(cpu, ERROR_INVALID_PARAMETER);
            return Some(0);
        }
        return Some(name);
    }
    let text = string_at(cpu, name)?;
    let a = crate::win16::atom_of(cpu, text, add);
    if a == 0 {
        set_error(
            cpu,
            if add {
                ERROR_INVALID_PARAMETER
            } else {
                ERROR_FILE_NOT_FOUND
            },
        );
    }
    Some(a as u32)
}

/// GlobalAddAtomA(name).
pub fn global_add_atom(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    atom(cpu, args[0], true)
}

/// GlobalFindAtomA(name).
pub fn global_find_atom(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    atom(cpu, args[0], false)
}

/// GlobalDeleteAtom(atom): 0, or the atom when it isn't one.
pub fn global_delete_atom(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let a = args[0] as u16;
    let left = crate::win16::release_atom(cpu, a);
    if left != 0 {
        set_error(cpu, ERROR_INVALID_HANDLE);
    }
    Some(left as u32)
}

/// GlobalGetAtomNameA(atom, buffer, size): the length of the name
/// copied, cut to fit with its zero.
pub fn global_get_atom_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[a, buffer, size] = args else {
        return Some(0);
    };
    let Some(name) = crate::win16::atom_name(cpu, a as u16) else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0);
    };
    if size == 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    let bytes = crate::dosstr::to_bytes(&name);
    let n = bytes.len().min(size as usize - 1);
    if !cpu.bus.guest_probe(buffer, n + 1, true) {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    cpu.bus.guest_write_bytes(buffer, &bytes[..n]);
    cpu.bus.guest_write_8(buffer + n as u32, 0);
    Some(n as u32)
}
