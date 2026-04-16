//! INT 09h — Keyboard Hardware Interrupt (IRQ 1).
//!
//! Real PC BIOS INT 09h reads the scan code from port 0x60, translates it
//! to an ASCII+scancode pair if applicable, and stores it in the BIOS
//! keyboard buffer at BDA 0x041E..0x043D (circular). It also updates
//! modifier flags at 0x0417 and finally sends EOI (0x20) to the PIC.
//!
//! Our emulator already pushes translated keys into `bus.keyboard_buffer`
//! directly from SDL events (so INT 16h still works), and latches the raw
//! scan code at port 0x60 whenever a physical key event happens. So the
//! only job of this default handler is to consume the scan code by reading
//! port 0x60 (which programs expect the ISR to do) and send EOI. It's
//! invoked automatically by the emulator loop when the IRQ1 pending flag
//! is set; games that install their own INT 09h ISR will get called
//! instead because the IVT entry points to their handler, not ours.

use crate::cpu::Cpu;

pub fn handle(cpu: &mut Cpu) {
    // Consume the scan code so programs that read 0x64 see "no more data".
    let _scan = cpu.bus.io_read(0x60);
    // Send end-of-interrupt to the 8259 master PIC. We don't model the PIC
    // in any meaningful way, but do it for completeness.
    cpu.bus.io_write(0x20, 0x20);
}
