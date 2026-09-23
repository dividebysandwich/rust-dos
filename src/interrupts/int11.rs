use crate::cpu::Cpu;

pub fn handle(cpu: &mut Cpu) {
    // Equipment list lives in the BDA (0x0410): 80x25 color plus whatever
    // floppies are mounted (see Bus::sync_drive_bda). Programs may patch it.
    cpu.set_ax(cpu.bus.read_16(0x0410));
}
