//! INT 2Fh — DOS multiplex interrupt.
//!
//! Only the XMS driver (AX=43xxh) and the MSCDEX CD-ROM extensions
//! (AH=15h, see `mscdex.rs`) are there. Every other function leaves the
//! registers untouched, which callers read as "not installed" (AL stays 00h
//! for the usual install checks, AX stays 1687h for the DPMI check, ...).

use crate::cpu::Cpu;

pub fn handle(cpu: &mut Cpu) {
    match cpu.ax() {
        // XMS driver installation check: AL=80h.
        0x4300 => cpu.set_ax(0x4380),
        // XMS driver entry point in ES:BX.
        0x4310 => {
            cpu.set_es(0xF000);
            cpu.set_bx(crate::bios::XMS_ENTRY);
        }
        _ if cpu.get_ah() == 0x15 => super::mscdex::handle(cpu, cpu.get_al()),
        // Everything else (DPMI 1687h, Windows 16xxh, ...) is not installed:
        // the registers come back unchanged.
        _ => {}
    }
}
