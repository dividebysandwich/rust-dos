use crate::cpu::Cpu;

pub fn handle(cpu: &mut Cpu) {
    // Equipment list lives in the BDA (0x0410): 80x25 color plus whatever
    // floppies are mounted (see Bus::sync_drive_bda). Programs may patch it.
    let equipment = cpu.bus.guest_read_16(0x0410);
    cpu.set_ax(equipment);
}
