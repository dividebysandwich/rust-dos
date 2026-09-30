//! INT 17h: the BIOS's printer services, on the printer on LPT1 (DX: 0).
//! AH=00h prints AL, 01h initializes the printer, 02h reads its status.
//! AH comes back with the status: not busy (bit 7), acknowledge (6),
//! out of paper (5), selected (4), I/O error (3) and timeout (0). Other
//! ports have no printer.

use crate::cpu::Cpu;
use iced_x86::Register;

/// No printer: busy, out of paper, timed out.
const NO_PRINTER: u8 = 0x31;

pub fn handle(cpu: &mut Cpu) {
    let ah = cpu.get_reg8(Register::AH);
    let al = cpu.get_reg8(Register::AL);
    if cpu.dx() != 0 || cpu.bus.printer.is_none() {
        if ah <= 2 {
            cpu.set_reg8(Register::AH, NO_PRINTER);
        }
        return;
    }
    match ah {
        0x00 => {
            cpu.bus.printer_put(al);
        }
        0x01 => {
            if let Some(p) = &mut cpu.bus.printer {
                p.init();
            }
        }
        0x02 => {}
        _ => return,
    }
    status(cpu);
}

/// The status port as the BIOS reports it: acknowledge and error the
/// other way round, and the low bits clear.
fn status(cpu: &mut Cpu) {
    let port = cpu.bus.printer.as_mut().map_or(0, |p| p.read_status());
    cpu.set_reg8(Register::AH, (port ^ 0x48) & 0xF8);
}
