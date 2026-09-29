//! INT 14h: the BIOS's serial port services, on the UARTs of the ports
//! the machine has (DX: 0 for COM1). AH=00h sets a port up, 01h sends a
//! character, 02h takes one, 03h reads the status. AH comes back with the
//! line status (bit 7 set for a timeout), AL with the modem status.

use crate::cpu::Cpu;
use iced_x86::Register;

pub fn handle(cpu: &mut Cpu) {
    let n = cpu.dx() as usize;
    let ah = cpu.get_reg8(Register::AH);
    let al = cpu.get_reg8(Register::AL);
    if n >= 4 || cpu.bus.serial.ports[n].is_none() {
        cpu.set_reg8(Register::AH, 0x80);
        return;
    }
    match ah {
        0x00 => {
            cpu.bus.serial_init(n, al);
            status(cpu, n);
        }
        0x01 => {
            cpu.bus.serial_send(n, al);
            let (lsr, _) = cpu.bus.serial_status(n).unwrap_or((0x80, 0));
            cpu.set_reg8(Register::AH, lsr & 0x7F);
        }
        0x02 => match cpu.bus.serial_receive(n) {
            Some(byte) => {
                let (lsr, _) = cpu.bus.serial_status(n).unwrap_or((0, 0));
                cpu.set_reg8(Register::AL, byte);
                cpu.set_reg8(Register::AH, lsr & 0x1E);
            }
            None => cpu.set_reg8(Register::AH, 0x80),
        },
        0x03 => status(cpu, n),
        _ => cpu.set_reg8(Register::AH, 0x80),
    }
}

fn status(cpu: &mut Cpu, n: usize) {
    let (lsr, msr) = cpu.bus.serial_status(n).unwrap_or((0x80, 0));
    cpu.set_reg8(Register::AH, lsr);
    cpu.set_reg8(Register::AL, msr);
}
