//! The BIOS's PS/2 mouse (INT 15h AH=C2h), as Windows' mouse driver uses
//! it: a handler installed with AX=C207h gets the motion and buttons on
//! IRQ 12, with the status, X, Y and a 0 word on its stack.

use rust_dos::cpu::{Cpu, CpuFlags};
use rust_dos::exec::{self, NoHook};
use rust_dos::interrupts::handle_hle;
use std::path::PathBuf;

/// A handler at 2000:0000 that keeps the status, X and Y it was called with
/// at 2000:0100, 0102 and 0104, and counts its calls at 2000:0106.
const HANDLER: [u8; 30] = [
    0x55, // PUSH BP
    0x8B, 0xEC, // MOV BP, SP
    0x8B, 0x46, 0x0C, // MOV AX, [BP+12]: the status
    0x2E, 0xA3, 0x00, 0x01, // MOV CS:[0100h], AX
    0x8B, 0x46, 0x0A, // MOV AX, [BP+10]: X
    0x2E, 0xA3, 0x02, 0x01, // MOV CS:[0102h], AX
    0x8B, 0x46, 0x08, // MOV AX, [BP+8]: Y
    0x2E, 0xA3, 0x04, 0x01, // MOV CS:[0104h], AX
    0x2E, 0xFF, 0x06, 0x06, 0x01, // INC WORD CS:[0106h]
    0x5D, // POP BP (RETF follows)
];

/// INT 15h with AX, BX (and ES): AH and CF.
fn bios(cpu: &mut Cpu, ax: u16, bx: u16) -> (u8, bool) {
    cpu.set_ax(ax);
    cpu.set_bx(bx);
    handle_hle(cpu, 0x15);
    ((cpu.ax() >> 8) as u8, cpu.get_cpu_flag(CpuFlags::CF))
}

/// Run a loop at 3000:0000 with interrupts on for `batches` batches.
fn run(cpu: &mut Cpu, batches: usize) {
    cpu.bus.load_bytes(0x30000, &[0xFB, 0xEB, 0xFE]); // STI; JMP $
    cpu.set_cs(0x3000);
    cpu.set_ip(0);
    cpu.set_ss(0x4000);
    cpu.set_sp(0x1000);
    for _ in 0..batches {
        let end = cpu.bus.clock.icount + 10_000;
        cpu.bus.start_batch(end);
        exec::run_batch(cpu, &mut NoHook, false);
    }
}

/// Run the loop until the handler has been called `calls` times.
fn run_until(cpu: &mut Cpu, calls: u16) {
    for _ in 0..1000 {
        if cpu.bus.read_16(0x20106) >= calls {
            return;
        }
        run(cpu, 1);
    }
    panic!("the handler was called {} times, not {}", cpu.bus.read_16(0x20106), calls);
}

#[test]
fn the_handler_gets_the_motion_and_buttons_on_irq_12() {
    let mut cpu = Cpu::new(PathBuf::from("."));
    handle_hle(&mut cpu, 0x11);
    assert_ne!(cpu.ax() & 0x04, 0, "the equipment word has a pointing device");

    let mut handler = HANDLER.to_vec();
    handler.push(0xCB); // RETF
    cpu.bus.load_bytes(0x20000, &handler);
    // Windows' driver: initialize, reset, then the handler, and enable.
    assert_eq!(bios(&mut cpu, 0xC205, 0x0300), (0, false));
    assert_eq!(bios(&mut cpu, 0xC201, 0), (0, false));
    assert_eq!(cpu.bx(), 0x00AA, "a mouse that passed its test");
    assert_eq!(bios(&mut cpu, 0xC200, 0x0100), (0x05, true), "no handler yet");
    cpu.set_es(0x2000);
    assert_eq!(bios(&mut cpu, 0xC207, 0x0000), (0, false));
    assert_eq!(bios(&mut cpu, 0xC202, 0x0200), (0, false));
    assert_eq!(bios(&mut cpu, 0xC202, 0x0700), (0x02, true), "no such rate");
    assert_eq!(bios(&mut cpu, 0xC200, 0x0100), (0, false));

    // Nothing moved, nothing to report.
    run(&mut cpu, 20);
    assert_eq!(cpu.bus.read_16(0x20106), 0);

    // Right and up, with the left button down.
    cpu.bus.mouse.move_by(5.0, -3.0);
    cpu.bus.mouse.button_down(0);
    run(&mut cpu, 20);
    assert_eq!(cpu.bus.read_16(0x20106), 1, "one report");
    assert_eq!(cpu.bus.read_16(0x20100), 0x09, "left button, positive X and Y");
    assert_eq!(cpu.bus.read_16(0x20102), 5);
    assert_eq!(cpu.bus.read_16(0x20104), 3, "up is positive");
    assert_eq!(cpu.sp(), 0x1000, "the stack as it was");
    assert!(!cpu.bus.pic.busy(12), "acknowledged");

    // Left and down past what one report holds: the sign bits, and the
    // rest in the next report.
    cpu.bus.mouse.move_by(-300.0, 2.0);
    run_until(&mut cpu, 2);
    assert_eq!(cpu.bus.read_16(0x20100), 0x39, "left button, negative X and Y");
    assert_eq!(cpu.bus.read_16(0x20102), (-255i16 as u16) & 0xFF);
    assert_eq!(cpu.bus.read_16(0x20104), (-2i16 as u16) & 0xFF);
    run_until(&mut cpu, 3);
    assert_eq!(cpu.bus.read_16(0x20100), 0x19, "the rest: negative X");
    assert_eq!(cpu.bus.read_16(0x20102), (-45i16 as u16) & 0xFF);
    assert_eq!(cpu.bus.read_16(0x20104), 0);

    // Disabled, it stays quiet.
    assert_eq!(bios(&mut cpu, 0xC200, 0x0000), (0, false));
    cpu.bus.mouse.button_up(0);
    run(&mut cpu, 20);
    assert_eq!(cpu.bus.read_16(0x20106), 3);
}

#[test]
fn the_mouse_talks_through_the_keyboard_controller() {
    let mut cpu = Cpu::new(PathBuf::from("."));
    let bus = &mut cpu.bus;
    // A byte for the mouse (D4h): enable reporting. Its ACK comes with the
    // mouse's status bit and IRQ 12.
    bus.io_write(0x64, 0xD4);
    bus.io_write(0x60, 0xF4);
    assert_eq!(bus.io_read(0x64) & 0x21, 0x21, "a mouse byte waits");
    assert!(bus.pic.busy(12));
    assert_eq!(bus.io_read(0x60), 0xFA);
    assert_eq!(bus.io_read(0x64) & 0x01, 0);
    assert!(bus.mouse.ps2.enabled);

    // Motion: a report of three bytes, one IRQ 12 each, as each moves in.
    bus.pic.slave.isr = 0;
    bus.pic.slave.irr = 0;
    bus.mouse.move_by(3.0, 4.0);
    bus.start_batch(bus.clock.icount + 1000);
    let mut report = Vec::new();
    for _ in 0..3 {
        assert!(bus.pic.busy(12), "IRQ 12 for byte {}", report.len());
        assert_eq!(bus.io_read(0x64) & 0x21, 0x21);
        bus.pic.slave.irr = 0;
        report.push(bus.io_read(0x60));
    }
    assert_eq!(report, [0x28, 3, (-4i8) as u8], "X right, Y down");
    assert!(!bus.pic.busy(12));

    // Identify, through the controller: the ACK, then a standard mouse.
    bus.io_write(0x64, 0xD4);
    bus.io_write(0x60, 0xF2);
    assert_eq!((bus.io_read(0x60), bus.io_read(0x60)), (0xFA, 0x00));

    // A key already in the output buffer goes first, with no mouse bit.
    bus.kbc.push_scancodes(&[0x1E]);
    bus.io_write(0x64, 0xD4);
    bus.io_write(0x60, 0xF5);
    assert_eq!(bus.io_read(0x64) & 0x21, 0x01);
    assert_eq!(bus.io_read(0x60), 0x1E);
    assert_eq!(bus.io_read(0x64) & 0x21, 0x21);
    assert_eq!(bus.io_read(0x60), 0xFA);
}
