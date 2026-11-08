//! Counting the frames a program draws (the Stats page's frames per
//! second): a retrace after which the picture had changed is a frame, an
//! unchanged screen draws none, and a program that flips pages draws a
//! frame at each flip, however much it draws out of sight between them.

use rust_dos::bus::Bus;
use std::path::PathBuf;

/// A bus at 1000 instructions per emulated ms, in 80x25 text at 70 Hz,
/// its first retrace behind it.
fn bus() -> Bus {
    let mut bus = Bus::new(PathBuf::from("."));
    bus.set_cycles_per_ms(1000);
    bus.sync_display();
    bus
}

/// Let a retrace's worth of time pass, as the frontend does before it
/// draws.
fn next_retrace(bus: &mut Bus) {
    bus.clock.icount += 15_000;
    bus.sync_display();
}

#[test]
fn drawn_frames_count_at_retraces() {
    let mut bus = bus();
    let before = bus.frames_drawn;
    for i in 0..10 {
        bus.write_8(0xB8000, b'0' + i);
        next_retrace(&mut bus);
    }
    assert_eq!(bus.frames_drawn - before, 10);
}

#[test]
fn an_unchanged_screen_draws_no_frames() {
    let mut bus = bus();
    bus.write_8(0xB8000, b'A');
    next_retrace(&mut bus);
    let before = bus.frames_drawn;
    for i in 0..70 {
        // The cursor's and blinking characters' blink is not the program's.
        bus.vga.set_blink(i % 2 == 0);
        next_retrace(&mut bus);
    }
    assert_eq!(bus.frames_drawn, before);
}

#[test]
fn page_flipping_counts_only_the_flips() {
    let mut bus = bus();
    let before = bus.frames_drawn;
    for i in 0..20u16 {
        // Drawing into the page out of sight at every retrace...
        bus.write_8(0xB8000 + 0x1000 * (i as usize % 2), b'x');
        // ...and showing it at every other one.
        if i % 2 == 0 {
            let start = if i % 4 == 0 { 0x800 } else { 0 };
            bus.io_write(0x3D4, 0x0C);
            bus.io_write(0x3D5, (start >> 8) as u8);
            bus.io_write(0x3D4, 0x0D);
            bus.io_write(0x3D5, start as u8);
        }
        next_retrace(&mut bus);
    }
    assert_eq!(bus.frames_drawn - before, 10);
}
