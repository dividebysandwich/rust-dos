//! Extended and expanded memory: what the XMS driver's and the EMS
//! driver's block moves cost, as real-mode programs use them to keep their
//! data above 1 MB and fetch it into conventional memory (a level's
//! graphics, a sound bank, a disk cache's sectors).

mod common;

use common::*;
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use iced_x86::Register;
use rust_dos::cpu::Cpu;
use rust_dos::{ems, xms};
use std::hint::black_box;

/// Where the move's parameter block goes (1000:0000).
const PARAMS: usize = 0x10000;
/// The conventional memory buffer the moves fill or empty (2000:0000).
const BUFFER_SEGMENT: u16 = 0x2000;

/// XMS function `ah` with DX as given.
fn xms_call(cpu: &mut Cpu, ah: u8, dx: u16) {
    cpu.set_reg8(Register::AH, ah);
    cpu.set_dx(dx);
    xms::call(cpu);
    assert_eq!(cpu.ax(), 1, "XMS function {ah:02X}h failed with {:02X}h", cpu.bx() as u8);
}

/// A new extended memory block of `kb` KB; returns its handle.
fn xms_allocate(cpu: &mut Cpu, kb: u16) -> u16 {
    xms_call(cpu, 0x09, kb);
    cpu.dx()
}

/// XMS function 0Bh: move `length` bytes from (`src_handle`, `src`) to
/// (`dst_handle`, `dst`); handle 0 takes a segment:offset.
fn xms_move(cpu: &mut Cpu, length: u32, src_handle: u16, src: u32, dst_handle: u16, dst: u32) {
    cpu.bus.write_32(PARAMS, length);
    cpu.bus.write_16(PARAMS + 4, src_handle);
    cpu.bus.write_32(PARAMS + 6, src);
    cpu.bus.write_16(PARAMS + 10, dst_handle);
    cpu.bus.write_32(PARAMS + 12, dst);
    cpu.set_ds((PARAMS >> 4) as u16);
    cpu.set_reg16(Register::SI, 0);
    xms_call(cpu, 0x0B, 0);
}

/// Fills `len` bytes from `addr` with a pattern.
fn fill(cpu: &mut Cpu, addr: usize, len: usize) {
    for i in 0..len {
        cpu.bus.write_8(addr + i, (i * 7 + i / 251) as u8);
    }
}

/// XMS moves from an extended memory block into conventional memory, and
/// from one block to another, in the sizes programs move: a 16 KB chunk,
/// 64 KB (a segment's worth), and 1 MB between blocks.
fn xms_moves(c: &mut Criterion) {
    let mut cpu = machine(&scratch("xms_moves"));
    let from = xms_allocate(&mut cpu, 1024);
    let to = xms_allocate(&mut cpu, 1024);
    fill(&mut cpu, 0x10_0000 + 0x1_0000, 0x10_0000);
    let buffer = (BUFFER_SEGMENT as u32) << 16;
    let mut group = c.benchmark_group("memory/xms_to_conventional");
    for len in [0x4000u32, 0x1_0000 - 2] {
        group.bench_with_input(BenchmarkId::from_parameter(len), &len, |b, &len| {
            b.iter(|| {
                xms_move(&mut cpu, len, from, 0x1000, 0, buffer);
                black_box(cpu.bus.read_8(0x20000));
            })
        });
    }
    group.finish();
    let mut group = c.benchmark_group("memory/conventional_to_xms");
    let len = 0x1_0000 - 2;
    group.bench_with_input(BenchmarkId::from_parameter(len), &len, |b, &len| {
        b.iter(|| xms_move(&mut cpu, len, 0, buffer, to, 0x2000))
    });
    group.finish();
    c.bench_function("memory/xms_to_xms/1048576", |b| {
        b.iter(|| xms_move(&mut cpu, 0x10_0000, from, 0, to, 0))
    });
}

/// INT 67h with AX; the status back.
fn ems_call(cpu: &mut Cpu, ax: u16) -> u8 {
    cpu.set_ax(ax);
    ems::handle(cpu);
    cpu.get_ah()
}

/// EMS function 57h, AL=0: move 64 KB from logical page 0 of `handle`
/// into conventional memory, as an EMS game fetches a level's data without
/// mapping it.
fn ems_moves(c: &mut Criterion) {
    let mut cpu = machine(&scratch("ems_moves"));
    ems::set_enabled(&mut cpu.bus, true);
    cpu.set_bx(16);
    assert_eq!(ems_call(&mut cpu, 0x4300), 0);
    let handle = cpu.dx();
    let len = 0x1_0000u32;
    // Length, then the source (type 1: expanded, handle, offset, page) and
    // the destination (type 0: conventional, offset, segment).
    cpu.bus.write_32(PARAMS, len);
    cpu.bus.write_8(PARAMS + 4, 1);
    cpu.bus.write_16(PARAMS + 5, handle);
    cpu.bus.write_16(PARAMS + 7, 0);
    cpu.bus.write_16(PARAMS + 9, 0);
    cpu.bus.write_8(PARAMS + 0x0B, 0);
    cpu.bus.write_16(PARAMS + 0x0C, 0);
    cpu.bus.write_16(PARAMS + 0x0E, 0);
    cpu.bus.write_16(PARAMS + 0x10, BUFFER_SEGMENT);
    cpu.set_ds((PARAMS >> 4) as u16);
    cpu.set_reg16(Register::SI, 0);
    assert_eq!(ems_call(&mut cpu, 0x5700), 0);
    c.bench_function("memory/ems_to_conventional/65536", |b| {
        b.iter(|| {
            cpu.set_ds((PARAMS >> 4) as u16);
            cpu.set_reg16(Register::SI, 0);
            black_box(ems_call(&mut cpu, 0x5700))
        })
    });
}

criterion_group!(benches, xms_moves, ems_moves);
criterion_main!(benches);
