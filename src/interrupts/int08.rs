use crate::cpu::Cpu;

pub fn handle(cpu: &mut Cpu) {
    // cpu.bus.log_string("[INT08] Timer Tick");
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

    // Chain to User Timer Interrupt (INT 1Ch)
    // Since we are in HLE, we can just "Call" the vector.
    // However, INT 1Ch is usually dummy (IRET) unless hooked.
    // We'll emulate the behavior: Explicitly run the handler logic for 1Ch.
    // But since we are inside `handle_hle`, we can't easily recurse cleanly without
    // potentially messing up the stack IF we did a real CPU loop.
}
