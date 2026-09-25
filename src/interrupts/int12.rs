use crate::cpu::Cpu;

/// The base memory in KB, as the BIOS data area has it (0413h): 640, or
/// less where the video takes the top of it (a Tandy 1000's 624).
pub fn handle(cpu: &mut Cpu) {
    let kb = cpu.bus.read_16(0x0413);
    cpu.set_ax(kb);
}
