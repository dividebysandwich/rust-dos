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
use iced_x86::code_asm::*;
use rust_dos::cpu::Cpu;
use rust_dos::exec::{self, NoHook, StopReason};
use rust_dos::interrupts::int10;
use rust_dos::video::adapter::{Adapter, VideoSetup};
use rust_dos::video::composite::{CompositeEra, CompositeMode, CompositeSettings};
use rust_dos::video::{self, Frame, bios};
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

/// Emulated speed for the program benchmark: about a 486DX2-66's
/// instructions per millisecond.
const PROGRAM_CYCLES_PER_MS: u32 = 25_000;

/// A 16-colour game's frame end to end: a DOS program sets mode 12h,
/// paints a background, then at each vertical retrace erases a 32x32 box
/// and draws it one step further on. Each iteration runs a 70th of an
/// emulated second (the program's code, its port and memory accesses) and
/// renders the frame as the main loop does.
fn planar_program(c: &mut Criterion) {
    let dir = scratch("planar_program");
    std::fs::write(dir.join("T.COM"), sprite_program()).unwrap();
    let mut cpu = machine(&dir);
    cpu.bus.set_cycles_per_ms(PROGRAM_CYCLES_PER_MS);
    cpu.load_shell();
    cpu.pending_command = Some("T".to_string());
    let mut frame = Frame::new(1, 1);
    // The shell starts the program, which sets the mode and paints.
    for _ in 0..70 {
        emulated_frame(&mut cpu, &mut frame);
    }
    assert_eq!(cpu.bus.video_mode, video::VideoMode::Vga640x480, "the program didn't start");
    c.bench_function("video/planar_program/12h", |b| b.iter(|| emulated_frame(&mut cpu, &mut frame)));
}

/// A 70th of a second of emulated time, then the frame rendered.
fn emulated_frame(cpu: &mut Cpu, frame: &mut Frame) {
    let end = cpu.bus.clock.icount + PROGRAM_CYCLES_PER_MS as u64 * 1000 / 70;
    cpu.bus.start_batch(end);
    while cpu.bus.clock.icount < end {
        match exec::run_batch(cpu, &mut NoHook, false) {
            StopReason::BatchEnd => break,
            StopReason::Exit => panic!("the program ended"),
            _ => cpu.bus.start_batch(end),
        }
    }
    cpu.bus.sync_display();
    let (width, height) = video::frame_size(&cpu.bus);
    if frame.resize(width, height) {
        cpu.bus.vga.mark_dirty_full();
    }
    render(cpu, frame);
    black_box(frame.rgb[0]);
}

/// The program `planar_program` runs, as a .COM file.
fn sprite_program() -> Vec<u8> {
    let assemble = || -> Result<Vec<u8>, IcedError> {
        let mut a = CodeAssembler::new(16)?;
        a.mov(ax, 0x0012)?;
        a.int(0x10)?;
        a.mov(ax, 0xA000)?;
        a.mov(es, ax)?;
        a.cld()?;
        // The background: each plane filled with its own pattern.
        let mut plane = a.create_label();
        let mut fill = a.create_label();
        a.mov(bl, 1)?;
        a.set_label(&mut plane)?;
        a.mov(dx, 0x3C4)?;
        a.mov(al, 2)?;
        a.mov(ah, bl)?;
        a.out(dx, ax)?;
        a.xor(di, di)?;
        a.mov(cx, 38400)?;
        a.mov(al, bl)?;
        a.set_label(&mut fill)?;
        a.stosb()?;
        a.add(al, bl)?;
        a.rol(al, 1)?;
        a.loop_(fill)?;
        a.shl(bl, 1)?;
        a.cmp(bl, 16)?;
        a.jb(plane)?;
        // Each frame: wait for the retrace, erase the box, move it on, draw
        // it again.
        let mut frame = a.create_label();
        let mut active = a.create_label();
        let mut retrace = a.create_label();
        let mut on_screen = a.create_label();
        let mut boxed = a.create_label();
        let mut box_row = a.create_label();
        a.xor(si, si)?;
        a.set_label(&mut frame)?;
        a.mov(dx, 0x3DA)?;
        a.set_label(&mut active)?;
        a.in_(al, dx)?;
        a.test(al, 8)?;
        a.jnz(active)?;
        a.set_label(&mut retrace)?;
        a.in_(al, dx)?;
        a.test(al, 8)?;
        a.jz(retrace)?;
        a.mov(dx, 0x3C4)?;
        a.mov(ax, 0x0F02)?;
        a.out(dx, ax)?;
        a.mov(di, si)?;
        a.xor(al, al)?;
        a.call(boxed)?;
        a.add(si, 81)?;
        a.cmp(si, 80 * 440)?;
        a.jb(on_screen)?;
        a.xor(si, si)?;
        a.set_label(&mut on_screen)?;
        a.mov(ax, 0x0E02)?;
        a.out(dx, ax)?;
        a.mov(di, si)?;
        a.mov(al, 0xFF)?;
        a.call(boxed)?;
        a.jmp(frame)?;
        // A 32x32 box at DI in AL.
        a.set_label(&mut boxed)?;
        a.mov(bx, 32)?;
        a.set_label(&mut box_row)?;
        a.mov(cx, 4)?;
        a.rep().stosb()?;
        a.add(di, 76)?;
        a.dec(bx)?;
        a.jnz(box_row)?;
        a.ret()?;
        a.assemble(0x100)
    };
    assemble().unwrap()
}

/// A CGA on a composite monitor in mode 6 with the colour burst on, as
/// King's Quest and other artifact-colour games use it: the screen filled
/// with a pattern, then each frame a 32x16 box (four bytes, sixteen rows
/// in each bank) drawn one step further on, and the frame rendered.
fn cga_composite(c: &mut Criterion) {
    let mut cpu = machine(&scratch("cga_composite"));
    bios::install(&mut cpu.bus, VideoSetup { adapter: Adapter::Cga, ..Default::default() });
    int10::set_mode(&mut cpu, 0x06);
    cpu.bus.io_write(0x3D8, 0x1A);
    cpu.bus.vga.set_composite(CompositeSettings { mode: CompositeMode::On, era: CompositeEra::Old });
    assert!(cpu.bus.vga.composite_active());
    for offset in 0..0x4000 {
        cpu.bus.write_8(0xB8000 + offset, (offset * 13 + offset / 80) as u8);
    }
    let (width, height) = video::frame_size(&cpu.bus);
    let mut frame = Frame::new(width, height);
    render(&mut cpu, &mut frame);
    let mut step = 0usize;
    c.bench_function("video/cga_composite/sprite", |b| {
        b.iter(|| {
            step += 1;
            let (column, top) = (step % 76, (step * 3) % 84);
            for bank in [0, 0x2000] {
                for y in top..top + 16 {
                    for x in column..column + 4 {
                        cpu.bus.write_8(0xB8000 + bank + y * 80 + x, (step * 0x11) as u8);
                    }
                }
            }
            render(&mut cpu, &mut frame);
            black_box(frame.rgb[0]);
        })
    });
    // Every line new each frame: the start address a line further on.
    let mut line = 0usize;
    c.bench_function("video/cga_composite/scroll", |b| {
        b.iter(|| {
            line = (line + 1) % 50;
            let start = line * 40;
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

criterion_group!(benches, planar_sprite, planar_scroll, planar_program, cga_composite);
criterion_main!(benches);
