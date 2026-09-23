use crate::cpu::Cpu;

/// The HLE trap: count the tick and acknowledge the interrupt.
pub fn handle(cpu: &mut Cpu) {
    tick(cpu);
    // End of interrupt, as the BIOS handler does. Programs that hook INT 08h
    // and chain here rely on it.
    cpu.bus.io_write(0x20, 0x20);
}

/// Count a timer tick in the BIOS data area. The ROM timer handler calls
/// this, then INT 1Ch, then acknowledges the interrupt.
pub fn tick(cpu: &mut Cpu) {
    // Increment System Timer Count (0040:006C)
    // 32-bit value at 0x046C
    let mut ticks = cpu.bus.read_16(0x046C) as u32;
    let high = cpu.bus.read_16(0x046E) as u32;
    ticks |= high << 16;

    ticks = ticks.wrapping_add(1);

    // Check for 24-hour wraparound
    // 18.2065 Hz * 60 * 60 * 24 = 1,573,040 ticks
    if ticks >= 1573040 {
        ticks = 0;
        // set byte at 0040:0070 to 1 (Midnight Flag)
        cpu.bus.write_8(0x0470, 1);
    }

    // Write back
    cpu.bus.write_16(0x046C, (ticks & 0xFFFF) as u16);
    cpu.bus.write_16(0x046E, (ticks >> 16) as u16);

}
