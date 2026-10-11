//! The VGA renderer: what drawing a frame of the guest's picture costs the
//! host after a program changed part of it.
//!
//! Each scenario sets a mode through the video BIOS, fills the screen with
//! a picture through the planes as a program would, renders it once, and
//! then measures one frame: the program's writes for that frame plus the
//! render, as the main loop does it (`render_screen`, then `clear_dirty`).

mod common;

use common::*;
use criterion::{Criterion, criterion_group, criterion_main};
use rust_dos::cpu::Cpu;
use rust_dos::interrupts::int10;
use rust_dos::video::{self, Frame};
use std::hint::black_box;

/// A machine in video mode `mode`, its 64 KB of each plane filled with
/// noise in all 16 colours (so no two rows are alike, the hardest picture
/// to spare work on), and a frame rendered from it.
fn planar_machine(name: &str, mode: u8) -> (Cpu, Frame) {
    let mut cpu = machine(&scratch(name));
    int10::set_mode(&mut cpu, mode);
    let mut seed = 0x2545_F491u32;
    for plane in 0..4u8 {
        map_mask(&mut cpu, 1 << plane);
        for at in 0..0x10000 {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            cpu.bus.write_8(0xA0000 + at, seed as u8);
        }
    }
    map_mask(&mut cpu, 0x0F);
    let (fw, fh) = video::frame_size(&cpu.bus);
    let mut frame = Frame::new(fw, fh);
    render(&mut cpu, &mut frame);
    (cpu, frame)
}

/// Sequencer Map Mask: the planes the next writes go to.
fn map_mask(cpu: &mut Cpu, planes: u8) {
    cpu.bus.io_write(0x3C4, 0x02);
    cpu.bus.io_write(0x3C5, planes);
}

/// One frame of the main loop: draw what changed, and start over.
fn render(cpu: &mut Cpu, frame: &mut Frame) {
    if cpu.bus.vga.dirty {
        video::render_screen(frame, &cpu.bus);
        cpu.bus.vga.clear_dirty();
    }
}

/// A 32x32 sprite at byte column `column` and row `top`, a plane at a time
/// in `colour`'s pattern, as a 16-colour game draws its cursor or a unit.
fn sprite(cpu: &mut Cpu, column: usize, top: usize, colour: u8) {
    let bytes_per_row = cpu.bus.vga.graphics_size().0 / 8;
    for plane in 0..4u8 {
        map_mask(cpu, 1 << plane);
        let fill = if colour & (1 << plane) != 0 { 0xFF } else { 0x00 };
        for y in top..top + 32 {
            for x in column..column + 4 {
                cpu.bus.write_8(0xA0000 + y * bytes_per_row + x, fill);
            }
        }
    }
}

/// A sprite moving across the screen: each frame it is drawn one step
/// further on, the rest of the picture stays. Mode 0Dh (320x200, as
/// Commander Keen 4-6 use) and mode 12h (640x480).
fn planar_sprite(c: &mut Criterion) {
    for (mode, label) in [(0x0Du8, "0Dh"), (0x12, "12h")] {
        let (mut cpu, mut frame) = planar_machine(&format!("planar_sprite_{label}"), mode);
        let (width, rows) = cpu.bus.vga.graphics_size();
        let (columns, tops) = (width / 8 - 4, rows - 32);
        let mut step = 0usize;
        c.bench_function(&format!("video/planar_sprite/{label}"), |b| {
            b.iter(|| {
                step += 1;
                sprite(&mut cpu, step % columns, (step * 3) % tops, (step % 15) as u8 + 1);
                render(&mut cpu, &mut frame);
                black_box(frame.rgb[0]);
            })
        });
    }
}

/// The whole picture changes each frame: a full-screen scroll by the CRTC
/// Start Address, one row further each frame. Every row is new, so this is
/// the most a frame costs.
fn planar_scroll(c: &mut Criterion) {
    let (mut cpu, mut frame) = planar_machine("planar_scroll", 0x12);
    let bytes_per_row = cpu.bus.vga.graphics_size().0 / 8;
    let mut row = 0usize;
    c.bench_function("video/planar_scroll/12h", |b| {
        b.iter(|| {
            row = (row + 1) % 32;
            let start = row * bytes_per_row;
            cpu.bus.io_write(0x3D4, 0x0C);
            cpu.bus.io_write(0x3D5, (start >> 8) as u8);
            cpu.bus.io_write(0x3D4, 0x0D);
            cpu.bus.io_write(0x3D5, start as u8);
            cpu.bus.vga.latch_start_address();
            render(&mut cpu, &mut frame);
            black_box(frame.rgb[0]);
        })
    });
}

criterion_group!(benches, planar_sprite, planar_scroll);
criterion_main!(benches);
