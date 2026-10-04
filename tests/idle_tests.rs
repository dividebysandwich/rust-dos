//! Busy-wait loops skipped exactly (`idle`): small programs that wait in
//! loops run in lockstep, one machine skipping passes and the other running
//! them all, and must come out the same after every batch. Loops that
//! change something every pass must not be skipped.

mod dyndiff;

use chrono::NaiveDate;
use dyndiff::lockstep;
use iced_x86::code_asm::*;
use rust_dos::cpu::{CoreMode, Cpu};
use rust_dos::idle::IdleStats;
use rust_dos::keyboard::PcKey;
use std::fs;
use std::path::Path;

/// Where the programs keep their data, past their code.
const DATA: u64 = 0x0F00;

/// Batches the programs run for, and the one before which a key is
/// pressed, which ends the keyboard loops.
const BATCHES: usize = 400;
const KEY_AT: usize = 250;

/// A machine that runs `T.COM`, the program `code` builds, from a
/// directory of its own.
fn machine(test: &str, name: &str, code: &[u8], core: CoreMode, skip: bool) -> Cpu {
    let dir = Path::new("target/idle_tests").join(test).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("T.COM"), code).unwrap();
    let mut cpu = Cpu::new(dir);
    cpu.core = core;
    cpu.bus.observe.enabled = skip;
    cpu.load_shell();
    cpu.pending_command = Some("T".to_string());
    cpu
}

fn assemble(body: impl Fn(&mut CodeAssembler) -> Result<(), IcedError>) -> Vec<u8> {
    let mut a = CodeAssembler::new(16).unwrap();
    body(&mut a).unwrap();
    a.assemble(0x100).unwrap()
}

/// Run the program on a machine that skips (on the recompiler) and one
/// that doesn't (on the interpreter), pressing a key before batch
/// `KEY_AT`. Returns what the skipping one skipped.
fn compare(test: &str, body: impl Fn(&mut CodeAssembler) -> Result<(), IcedError>) -> IdleStats {
    let at = NaiveDate::from_ymd_opt(1995, 4, 11).unwrap().and_hms_opt(12, 34, 56).unwrap();
    rust_dos::hosttime::fix(Some(at));
    let code = assemble(body);
    let mut a = machine(test, "a", &code, CoreMode::Normal, false);
    let mut b = machine(test, "b", &code, CoreMode::Dynamic, true);
    let key = rust_dos::keyboard::lookup("a").unwrap();
    let press = |n: usize, cpu: &mut Cpu| {
        let k: PcKey = key;
        if n == KEY_AT {
            rust_dos::keyboard::apply_key(&mut cpu.bus, k, k.ascii, true);
        } else if n == KEY_AT + 1 {
            rust_dos::keyboard::apply_key(&mut cpu.bus, k, k.ascii, false);
        }
    };
    if let Err(e) = lockstep(&mut a, &mut b, BATCHES, 100_000, press) {
        panic!("{}: {}\n  {:?}", test, e, b.bus.observe.stats);
    }
    println!("{}: {:?}", test, b.bus.observe.stats);
    b.bus.observe.stats
}

/// Read the key the loop waited for, leave it in the program's data, and
/// end.
fn finish(a: &mut CodeAssembler) -> Result<(), IcedError> {
    a.mov(ah, 0)?;
    a.int(0x16)?;
    a.mov(byte_ptr(DATA), al)?;
    a.mov(ax, 0x4C00)?;
    a.int(0x21)
}

#[test]
fn a_keyboard_poll_is_skipped() {
    let stats = compare("keyboard", |a| {
        let mut wait = a.create_label();
        a.set_label(&mut wait)?;
        a.mov(ah, 1)?;
        a.int(0x16)?;
        a.jz(wait)?;
        finish(a)
    });
    assert!(stats.proofs > 0 && stats.skipped > 0, "{:?}", stats);
}

#[test]
fn a_wait_for_the_next_tick_is_skipped() {
    // Wait for 20 ticks of the BIOS's count, in a tight loop.
    let stats = compare("ticks", |a| {
        let mut tick = a.create_label();
        let mut same = a.create_label();
        a.xor(ax, ax)?;
        a.mov(es, ax)?;
        a.mov(cx, 20)?;
        a.set_label(&mut tick)?;
        a.mov(ax, word_ptr(0x46C).es())?;
        a.set_label(&mut same)?;
        a.cmp(ax, word_ptr(0x46C).es())?;
        a.je(same)?;
        a.loop_(tick)?;
        a.mov(ax, 0x4C00)?;
        a.int(0x21)
    });
    assert!(stats.proofs > 0 && stats.skipped > 0, "{:?}", stats);
}

#[test]
fn a_loop_that_flips_a_byte_is_skipped_by_twos() {
    // Each pass changes the byte, every second one puts it back.
    let stats = compare("flip", |a| {
        let mut wait = a.create_label();
        a.set_label(&mut wait)?;
        a.xor(byte_ptr(DATA), 1)?;
        a.mov(ah, 1)?;
        a.int(0x16)?;
        a.jz(wait)?;
        finish(a)
    });
    assert!(stats.proofs > 0, "{:?}", stats);
}

#[test]
fn a_loop_that_counts_is_not_skipped() {
    let stats = compare("count", |a| {
        let mut wait = a.create_label();
        a.set_label(&mut wait)?;
        a.inc(word_ptr(DATA))?;
        a.mov(ah, 1)?;
        a.int(0x16)?;
        a.jz(wait)?;
        finish(a)
    });
    assert_eq!(stats.proofs, 0, "{:?}", stats);
}

#[test]
fn a_loop_that_reads_a_port_is_not_skipped() {
    let stats = compare("port", |a| {
        let mut wait = a.create_label();
        a.set_label(&mut wait)?;
        a.in_(al, 0x61)?;
        a.mov(ah, 1)?;
        a.int(0x16)?;
        a.jz(wait)?;
        finish(a)
    });
    assert_eq!(stats.proofs, 0, "{:?}", stats);
}

/// Wait for 30 vertical retraces as programs do: for the end of one, then
/// for the start of the next.
fn retraces(a: &mut CodeAssembler, look: impl Fn(&mut CodeAssembler) -> Result<(), IcedError>) -> Result<(), IcedError> {
    let mut end = a.create_label();
    let mut start = a.create_label();
    a.mov(dx, 0x3DA)?;
    a.mov(cx, 30)?;
    a.set_label(&mut end)?;
    a.in_(al, dx)?;
    look(a)?;
    a.jnz(end)?;
    a.set_label(&mut start)?;
    a.in_(al, dx)?;
    look(a)?;
    a.jz(start)?;
    a.loop_(end)?;
    a.mov(ax, 0x4C00)?;
    a.int(0x21)
}

#[test]
fn a_wait_for_the_retrace_is_skipped() {
    let stats = compare("retrace", |a| retraces(a, |a| a.test(al, 8)));
    assert!(stats.proofs > 0 && stats.skipped > 0, "{:?}", stats);
}

#[test]
fn a_wait_that_masks_the_status_is_skipped() {
    let stats = compare("retrace_and", |a| retraces(a, |a| a.and(al, 8)));
    assert!(stats.proofs > 0 && stats.skipped > 0, "{:?}", stats);
}

#[test]
fn a_wait_that_looks_at_the_whole_status_is_not_skipped() {
    // Bit 0 changes along every line: comparing all of AL tells them apart.
    let stats = compare("retrace_cmp", |a| retraces(a, |a| a.cmp(al, 8)));
    assert_eq!(stats.proofs, 0, "{:?}", stats);
}

#[test]
fn a_wait_that_keeps_the_status_is_not_skipped() {
    let stats = compare("retrace_store", |a| {
        retraces(a, |a| {
            a.mov(byte_ptr(DATA), al)?;
            a.test(al, 8)
        })
    });
    assert_eq!(stats.proofs, 0, "{:?}", stats);
}

#[test]
fn a_wait_for_the_display_after_the_retrace_is_skipped() {
    // After the retrace begins, wait for the first line shown (bit 0 clear):
    // the blanking runs to the end of the frame.
    let stats = compare("display", |a| {
        let mut end = a.create_label();
        let mut start = a.create_label();
        let mut shown = a.create_label();
        a.mov(dx, 0x3DA)?;
        a.mov(cx, 30)?;
        a.set_label(&mut end)?;
        a.in_(al, dx)?;
        a.test(al, 8)?;
        a.jnz(end)?;
        a.set_label(&mut start)?;
        a.in_(al, dx)?;
        a.test(al, 8)?;
        a.jz(start)?;
        a.set_label(&mut shown)?;
        a.in_(al, dx)?;
        a.test(al, 1)?;
        a.jnz(shown)?;
        a.loop_(end)?;
        a.mov(ax, 0x4C00)?;
        a.int(0x21)
    });
    assert!(stats.proofs > 0 && stats.skipped > 0, "{:?}", stats);
}

#[test]
fn an_interrupt_in_a_retrace_wait_sees_the_status_it_would_have() {
    // A timer handler keeps the AL of the code it interrupted, the status
    // the wait read last, in a ring of bytes, and goes on to the BIOS's.
    const OLD: u64 = 0x0F10;
    const RING: u64 = 0x0F14;
    let stats = compare("retrace_irq", |a| {
        let mut start = a.create_label();
        a.jmp(start)?;
        // The handler, at 102h.
        a.push(bx)?;
        a.mov(bx, word_ptr(RING).cs())?;
        a.mov(byte_ptr(bx).cs(), al)?;
        a.inc(bx)?;
        a.and(bx, 0x0FFF)?;
        a.or(bx, 0x1000)?;
        a.mov(word_ptr(RING).cs(), bx)?;
        a.pop(bx)?;
        a.jmp(dword_ptr(OLD).cs())?;
        a.set_label(&mut start)?;
        a.mov(word_ptr(RING), 0x1000)?;
        a.mov(ax, 0x3508)?;
        a.int(0x21)?;
        a.mov(word_ptr(OLD), bx)?;
        a.mov(word_ptr(OLD + 2), es)?;
        a.mov(ax, 0x2508)?;
        a.mov(dx, 0x102)?;
        a.int(0x21)?;
        // The timer at 500 Hz, so that it comes in the waits often.
        pit(a, 2386)?;
        retraces_then(a)?;
        pit(a, 0)?;
        a.lds(dx, dword_ptr(OLD))?;
        a.mov(ax, 0x2508)?;
        a.int(0x21)?;
        a.mov(ax, 0x4C00)?;
        a.int(0x21)
    });
    assert!(stats.proofs > 0 && stats.skipped > 0, "{:?}", stats);
}

/// Program the PIT's channel 0 to count `count` (0: 65536).
fn pit(a: &mut CodeAssembler, count: u16) -> Result<(), IcedError> {
    a.mov(al, 0x36)?;
    a.out(0x43, al)?;
    a.mov(ax, count as u32)?;
    a.out(0x40, al)?;
    a.mov(al, ah)?;
    a.out(0x40, al)
}

/// `retraces`' loops, without the exit.
fn retraces_then(a: &mut CodeAssembler) -> Result<(), IcedError> {
    let mut end = a.create_label();
    let mut start = a.create_label();
    a.mov(dx, 0x3DA)?;
    a.mov(cx, 60)?;
    a.set_label(&mut end)?;
    a.in_(al, dx)?;
    a.test(al, 8)?;
    a.jnz(end)?;
    a.set_label(&mut start)?;
    a.in_(al, dx)?;
    a.test(al, 8)?;
    a.jz(start)?;
    a.loop_(end)
}
