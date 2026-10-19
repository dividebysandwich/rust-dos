//! Modules and the system's version.

use super::{ERROR_INVALID_HANDLE, ERROR_PROC_NOT_FOUND, FALSE, TRUE, set_error, string_at};
use crate::cpu::Cpu;
use crate::win32::process;

/// The address of the function `args[1]` (a name, or an ordinal below
/// 10000h) that the module at `args[0]` exports, read from its export
/// directory in memory as Windows reads it.
pub(super) fn get_proc_address(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[module, name] = args else {
        unreachable!()
    };
    if !process(cpu).modules.iter().any(|m| m.base == module) {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0);
    }
    let wanted = match name {
        0..0x1_0000 => Err(name as u16),
        _ => Ok(string_at(cpu, name)?),
    };
    let found = export(cpu, module, &wanted)?;
    if found.is_none() {
        set_error(cpu, ERROR_PROC_NOT_FOUND);
    }
    Some(found.unwrap_or(0))
}

/// The address of the export `wanted` (a name, or else an ordinal) of the
/// module at `base`, if it has one. None if a page isn't there.
fn export(cpu: &mut Cpu, base: u32, wanted: &Result<String, u16>) -> Option<Option<u32>> {
    let bus = &mut cpu.bus;
    let pe = base.wrapping_add(bus.guest_read_32(base + 0x3C));
    // The export directory: the optional header's first data directory.
    let dir = bus.guest_read_32(pe + 0x78);
    let dir_size = bus.guest_read_32(pe + 0x7C);
    if bus.guest_faulted() {
        return None;
    }
    if dir == 0 {
        return Some(None);
    }
    let at = |offset: u32| base.wrapping_add(dir).wrapping_add(offset);
    let ordinal_base = bus.guest_read_32(at(16));
    let functions = bus.guest_read_32(at(20));
    let names = bus.guest_read_32(at(24));
    let function_table = base.wrapping_add(bus.guest_read_32(at(28)));
    let name_table = base.wrapping_add(bus.guest_read_32(at(32)));
    let ordinal_table = base.wrapping_add(bus.guest_read_32(at(36)));
    let index = match wanted {
        Err(ordinal) => (*ordinal as u32).checked_sub(ordinal_base),
        Ok(name) => (0..names).find_map(|i| {
            let at = base.wrapping_add(bus.guest_read_32(name_table + 4 * i));
            (bus.guest_read_asciiz(at, 256) == crate::dosstr::to_bytes(name))
                .then(|| bus.guest_read_16(ordinal_table + 2 * i) as u32)
        }),
    };
    let rva = index
        .filter(|&i| i < functions)
        .map(|i| bus.guest_read_32(function_table + 4 * i));
    if bus.guest_faulted() {
        return None;
    }
    // An export forwarded to another DLL's isn't followed yet.
    Some(
        rva.filter(|&rva| rva != 0 && !(dir..dir + dir_size).contains(&rva))
            .map(|rva| base + rva),
    )
}

/// OSVERSIONINFOA's size, which its first field must give. Windows 95
/// doesn't take the larger OSVERSIONINFOEXA, and leaves the last error as
/// it was when the size is wrong.
const OSVERSIONINFOA: u32 = 148;

/// Windows 95: 4.00.950, a 9x platform (VER_PLATFORM_WIN32_WINDOWS), the
/// build number's high word the version, and no service pack.
pub(super) fn get_version_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let info = args[0];
    if cpu.bus.guest_read_32(info) != OSVERSIONINFOA {
        return Some(FALSE);
    }
    let mut data = [0u8; OSVERSIONINFOA as usize];
    for (i, value) in [OSVERSIONINFOA, 4, 0, 0x0400_03B6, 1]
        .into_iter()
        .enumerate()
    {
        data[i * 4..i * 4 + 4].copy_from_slice(&value.to_le_bytes());
    }
    cpu.bus.guest_write_bytes(info, &data);
    Some(TRUE)
}
