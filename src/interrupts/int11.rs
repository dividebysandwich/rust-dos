use crate::cpu::Cpu;

pub fn handle(cpu: &mut Cpu) {
    // The equipment list lives in the BDA (0x0410): the coprocessor and
    // mouse bits (`bios::EQUIPMENT_FIXED`), the video adapter's mode, and
    // the floppies, ports and game port attached (see
    // `Bus::sync_drive_bda`). Programs may patch it.
    let equipment = cpu.bus.guest_read_16(0x0410);
    cpu.set_ax(equipment);
}
