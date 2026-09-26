//! INT 2Fh — DOS multiplex interrupt.
//!
//! Only the XMS driver (AX=43xxh), the MSCDEX CD-ROM extensions (AH=15h,
//! see `mscdex.rs`) and the DOS internal calls that find the file tables
//! (AX=1216h, 1220h) are there. Every other function leaves the
//! registers untouched, which callers read as "not installed" (AL stays 00h
//! for the usual install checks, AX stays 1687h for the DPMI check, ...).

use crate::cpu::{Cpu, CpuFlags};
use crate::dos_files;

/// ES:DI at the linear address `at`, or CF set for none.
fn point_es_di(cpu: &mut Cpu, at: Option<usize>) {
    if let Some(at) = at {
        cpu.set_es((at >> 4) as u16);
        cpu.set_di((at & 0x0F) as u16);
    }
    cpu.set_cpu_flag(CpuFlags::CF, at.is_none());
}

pub fn handle(cpu: &mut Cpu) {
    match cpu.ax() {
        // XMS driver installation check: AL=80h.
        0x4300 => cpu.set_ax(0x4380),
        // XMS driver entry point in ES:BX.
        0x4310 => {
            cpu.set_es(0xF000);
            cpu.set_bx(crate::bios::XMS_ENTRY);
        }
        // The System File Table entry BX in ES:DI.
        0x1216 => {
            let sft = cpu.bx();
            point_es_di(cpu, (sft < crate::disk::FILES).then(|| dos_files::entry_address(sft)));
        }
        // The slot of handle BX in the running process's job file table
        // in ES:DI, which holds the handle's System File Table entry.
        0x1220 => {
            let at = dos_files::slot_address(&cpu.bus, cpu.current_psp, cpu.bx());
            point_es_di(cpu, at);
        }
        _ if cpu.get_ah() == 0x15 => super::mscdex::handle(cpu, cpu.get_al()),
        // Everything else (DPMI 1687h, Windows 16xxh, ...) is not installed:
        // the registers come back unchanged.
        _ => {}
    }
}
