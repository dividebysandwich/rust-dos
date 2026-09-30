//! INT 09h — Keyboard Hardware Interrupt (IRQ 1).
//!
//! Real PC BIOS INT 09h reads the scan code from port 0x60, translates it
//! to an ASCII+scancode pair if applicable, and stores it in the BIOS
//! keyboard buffer at BDA 0x041E..0x043D (circular). It also updates
//! modifier flags at 0x0417 and finally sends EOI (0x20) to the PIC.
//!
//! Our emulator already queues translated keys in `bus.keyboard_buffer`
//! directly from SDL events (so INT 16h still works), and latches the raw
//! scan code at port 0x60 whenever a physical key event happens. So this
//! default handler consumes the scan code by reading port 0x60 (which
//! programs expect the ISR to do), moves a make code's keystrokes from the
//! queue into the BDA buffer (`keyboard::bios_took_scan`) and sends EOI. It's
//! invoked automatically by the emulator loop when the IRQ1 pending flag
//! is set; games that install their own INT 09h ISR will get called
//! instead because the IVT entry points to their handler, not ours.

use crate::cpu::Cpu;

pub fn handle(cpu: &mut Cpu) {
    crate::keyboard::bios_saw_keys(&mut cpu.bus);
    if cpu.v86() {
        // Under a V86 monitor the processor reads the scan code and
        // acknowledges the interrupt on the way out (`return_from_hle`):
        // Windows' keyboard driver hands the machine its keys through the
        // port it traps and follows the machine's PIC through its ports,
        // and waits for it to take each key before it passes on the next,
        // or a hot key after them.
        use crate::bios::PortAccess::{In, Out};
        cpu.bus.port_accesses.extend([In(0x60), Out(0x20, 0x20)]);
        return;
    }
    // Consume the scan code so programs that read 0x64 see "no more data",
    // and put a make code's keystrokes in the BIOS's buffer.
    let scan = cpu.bus.io_read(0x60);
    crate::keyboard::bios_took_scan(&mut cpu.bus, scan);
    // Send end-of-interrupt to the 8259 master PIC. We don't model the PIC
    // in any meaningful way, but do it for completeness.
    cpu.bus.io_write(0x20, 0x20);
}
