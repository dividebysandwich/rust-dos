//! Busy-wait loops skipped exactly (`idle`): small programs that wait in
//! loops run in lockstep, one machine skipping passes and the other running
//! them all, and must come out the same after every batch. Loops that
//! change something every pass must not be skipped.

mod dyndiff;
mod win16;

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
    let mut cpu = machine_with(test, name, ("T.COM", code), core, skip);
    cpu.load_shell();
    cpu.pending_command = Some("T".to_string());
    cpu
}

/// A machine with the file `file` (its name and bytes) alone in a
/// directory of its own, its shell not loaded yet. The host's time is
/// fixed first, so machines built a moment apart start with the same
/// BIOS tick count.
fn machine_with(test: &str, name: &str, file: (&str, &[u8]), core: CoreMode, skip: bool) -> Cpu {
    let at = NaiveDate::from_ymd_opt(1995, 4, 11).unwrap().and_hms_opt(12, 34, 56).unwrap();
    rust_dos::hosttime::fix(Some(at));
    let dir = Path::new("target/idle_tests").join(test).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(file.0), file.1).unwrap();
    let mut cpu = Cpu::new(dir);
    cpu.core = core;
    cpu.bus.observe.enabled = skip;
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
fn watching_is_timed_within_the_batches() {
    // The host's time between batches isn't watching's, even where a
    // batch ends in the middle of a proof (`Observe::take_watched`).
    let code = assemble(|a| {
        let mut wait = a.create_label();
        a.set_label(&mut wait)?;
        a.mov(ah, 1)?;
        a.int(0x16)?;
        a.jz(wait)?;
        finish(a)
    });
    let mut a = machine("watched", "a", &code, CoreMode::Normal, false);
    let mut b = machine("watched", "b", &code, CoreMode::Dynamic, true);
    let mut watched = 0;
    let each = |_: usize, cpu: &mut Cpu| {
        assert!(!cpu.bus.observe.in_watch(), "a span of watching outlived its batch");
        watched += cpu.bus.observe.take_watched().1;
    };
    if let Err(e) = lockstep(&mut a, &mut b, 50, 100_000, each) {
        panic!("{}", e);
    }
    assert!(watched > 0, "nothing was watched: {:?}", b.bus.observe.stats);
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
fn a_loop_that_counts_down_is_skipped_until_its_count_runs_out() {
    // As MS-DOS 7's IO.SYS polls the keyboard: a count down each pass,
    // whose flags POPF puts back, with something done where it reaches 0.
    let stats = compare("count_down", |a| {
        let (mut wait, mut left) = (a.create_label(), a.create_label());
        a.mov(word_ptr(DATA + 2), 3000u32)?;
        a.set_label(&mut wait)?;
        a.pushf()?;
        a.dec(word_ptr(DATA + 2))?;
        a.jnz(left)?;
        a.inc(word_ptr(DATA + 4))?;
        a.mov(word_ptr(DATA + 2), 3000u32)?;
        a.set_label(&mut left)?;
        a.popf()?;
        a.mov(ah, 1)?;
        a.int(0x16)?;
        a.jz(wait)?;
        finish(a)
    });
    assert!(stats.proofs > 0 && stats.skipped > 0, "{:?}", stats);
}

#[test]
fn a_count_compared_each_pass_is_skipped_until_it_reaches_the_value() {
    // As MS-DOS 7 does between keyboard polls: a byte compared with FFh,
    // then counted up, with flags that a CMP sets again after it.
    let stats = compare("count_up", |a| {
        let (mut wait, mut below) = (a.create_label(), a.create_label());
        a.set_label(&mut wait)?;
        a.cmp(byte_ptr(DATA + 6), 0xFF)?;
        a.jne(below)?;
        a.inc(word_ptr(DATA + 8))?;
        a.set_label(&mut below)?;
        a.inc(word_ptr(DATA + 6))?;
        a.mov(al, 0)?;
        a.cmp(al, 2)?;
        a.mov(ah, 1)?;
        a.int(0x16)?;
        a.jz(wait)?;
        finish(a)
    });
    assert!(stats.proofs > 0 && stats.skipped > 0, "{:?}", stats);
}

#[test]
fn a_count_read_into_a_register_is_not_skipped() {
    let stats = compare("count_read", |a| {
        let mut wait = a.create_label();
        a.set_label(&mut wait)?;
        a.inc(word_ptr(DATA + 2))?;
        a.mov(bx, word_ptr(DATA + 2))?;
        a.and(bx, 0)?;
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

/// Wait for 200 ticks of the timer at 1 kHz as a Windows 98 game paces its
/// frames (#160), then end.
fn frame_limiter(a: &mut CodeAssembler) -> Result<(), IcedError> {
    limiter(a, 1193, 200)
}

/// Wait for `ticks` ticks of the timer counting `count`: a loop around a
/// call that reads the tick count, until the time since the start reaches
/// the frame's. The count moves at every timer interrupt, and with it AX
/// at the loop's head. Then end.
fn limiter(a: &mut CodeAssembler, count: u16, ticks: u32) -> Result<(), IcedError> {
    let (mut wait, mut read) = (a.create_label(), a.create_label());
    pit(a, count)?;
    a.xor(ax, ax)?;
    a.mov(es, ax)?;
    a.mov(ax, word_ptr(0x46C).es())?;
    a.mov(word_ptr(DATA), ax)?;
    a.set_label(&mut wait)?;
    a.call(read)?;
    a.sub(ax, word_ptr(DATA))?;
    a.cmp(ax, ticks)?;
    a.jb(wait)?;
    pit(a, 0)?;
    a.mov(ax, 0x4C00)?;
    a.int(0x21)?;
    // GetTickCount.
    a.set_label(&mut read)?;
    a.mov(ax, word_ptr(0x46C).es())?;
    a.xor(dx, dx)?;
    a.ret()
}

/// Instructions a frame limiter's tests skip at the least. A detector
/// that only arms where two events in a row find the same registers, with
/// one backoff for all loops (as before #160), skips under 150,000 in each
/// of them, as it arms the loop only where an event lands between two
/// ticks; it now skips millions.
const LIMITER_SKIPS: u64 = 1_000_000;

#[test]
fn a_frame_limiter_that_calls_for_the_time_is_skipped() {
    let stats = compare("tick_call", frame_limiter);
    assert!(stats.skipped > LIMITER_SKIPS, "{:?}", stats);
}

#[test]
fn a_frame_limiter_on_a_fast_timer_is_skipped() {
    // The timer at 5 kHz: nearly every event comes after a tick, so the
    // loop is seldom at an instruction with the registers it had at the
    // event before.
    let stats = compare("tick_call_fast", |a| limiter(a, 239, 2000));
    assert!(stats.skipped > LIMITER_SKIPS, "{:?}", stats);
}

#[test]
fn a_loop_that_failed_before_does_not_keep_another_from_being_skipped() {
    // As Windows 98 boots before a game: a keyboard poll that counts each
    // pass, whose proofs all fail, for 10 ticks of the BIOS's count, then
    // the frame limiter.
    let stats = compare("tick_call_after", |a| {
        let mut busy = a.create_label();
        a.xor(ax, ax)?;
        a.mov(es, ax)?;
        a.mov(ax, word_ptr(0x46C).es())?;
        a.mov(word_ptr(DATA + 4), ax)?;
        a.set_label(&mut busy)?;
        a.inc(word_ptr(DATA + 2))?;
        a.mov(ah, 1)?;
        a.int(0x16)?;
        a.mov(ax, word_ptr(0x46C).es())?;
        a.sub(ax, word_ptr(DATA + 4))?;
        a.cmp(ax, 10)?;
        a.jb(busy)?;
        frame_limiter(a)
    });
    assert!(stats.failures > 4, "{:?}", stats);
    assert!(stats.skipped > LIMITER_SKIPS, "{:?}", stats);
}

#[test]
fn a_loop_whose_proofs_fail_does_not_keep_another_from_being_skipped() {
    // A keyboard poll that writes a port every pass, for 20 ticks of the
    // timer at 1 kHz, then the frame limiter. The poll is armed every few
    // calls and each proof of it fails, so a backoff shared by all loops
    // would grow to let the limiter's chances pass.
    let stats = compare("port_poll_then_tick_call", |a| {
        let mut busy = a.create_label();
        pit(a, 1193)?;
        a.xor(ax, ax)?;
        a.mov(es, ax)?;
        a.mov(ax, word_ptr(0x46C).es())?;
        a.mov(word_ptr(DATA + 4), ax)?;
        a.set_label(&mut busy)?;
        a.out(0x80, al)?;
        a.mov(ah, 1)?;
        a.int(0x16)?;
        a.mov(ax, word_ptr(0x46C).es())?;
        a.sub(ax, word_ptr(DATA + 4))?;
        a.cmp(ax, 20)?;
        a.jb(busy)?;
        frame_limiter(a)
    });
    assert!(stats.failures >= 8, "{:?}", stats);
    assert!(stats.skipped > LIMITER_SKIPS, "{:?}", stats);
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

/// The PIT's channel 0 count Windows 98 runs it at, 200 times a second.
const VTD_COUNT: u16 = 5965;

/// Read the time as Windows 98's VTD does for QueryPerformanceCounter:
/// latch channel 0's count and read it, and add the counts gone since the
/// last reload to the BIOS's ticks times the reload, in EAX. The last
/// count read is in the program's data at `DATA + 4`, which the timer's
/// interrupt sets to FFFFh: a count above it means the counter reloaded
/// before the interrupt came, and reads as the end of the period.
fn vtd_read(a: &mut CodeAssembler) -> Result<(), IcedError> {
    let (mut wrapped, mut store) = (a.create_label(), a.create_label());
    a.pushfd()?;
    a.xor(eax, eax)?;
    a.cli()?;
    a.out(0x43, al)?;
    a.in_(al, 0x40)?;
    a.mov(ah, al)?;
    a.in_(al, 0x40)?;
    a.xchg(ah, al)?;
    a.cmp(eax, dword_ptr(DATA + 4))?;
    a.ja(wrapped)?;
    a.set_label(&mut store)?;
    a.mov(dword_ptr(DATA + 4), eax)?;
    a.sub(eax, VTD_COUNT as u32)?;
    a.neg(eax)?;
    a.movzx(ebx, word_ptr(0x46C).es())?;
    a.imul_3(ebx, ebx, VTD_COUNT as i32)?;
    a.add(eax, ebx)?;
    a.popfd()?;
    a.ret()?;
    a.set_label(&mut wrapped)?;
    a.xor(eax, eax)?;
    a.jmp(store)
}

/// Wait with `vtd_read` until `length` counts have gone since the start,
/// `frames` times over, as a game paces its frames with
/// QueryPerformanceCounter. The PIT's channel 0 counts `VTD_COUNT` in
/// `mode`. Then end.
fn pit_limiter(a: &mut CodeAssembler, mode: u8, frames: u16, length: u32) -> Result<(), IcedError> {
    const OLD: u64 = DATA + 0x10;
    let (mut start, mut frame) = (a.create_label(), a.create_label());
    let (mut wait, mut read) = (a.create_label(), a.create_label());
    a.jmp(start)?;
    // The timer's handler, at 102h.
    a.mov(dword_ptr(DATA + 4).cs(), 0xFFFF)?;
    a.jmp(dword_ptr(OLD).cs())?;
    a.set_label(&mut start)?;
    a.mov(dword_ptr(DATA + 4), 0xFFFF)?;
    a.mov(ax, 0x3508)?;
    a.int(0x21)?;
    a.mov(word_ptr(OLD), bx)?;
    a.mov(word_ptr(OLD + 2), es)?;
    a.mov(ax, 0x2508)?;
    a.mov(dx, 0x102)?;
    a.int(0x21)?;
    a.mov(al, (0x30 | mode << 1) as u32)?;
    a.out(0x43, al)?;
    a.mov(ax, VTD_COUNT as u32)?;
    a.out(0x40, al)?;
    a.mov(al, ah)?;
    a.out(0x40, al)?;
    a.xor(ax, ax)?;
    a.mov(es, ax)?;
    a.mov(cx, frames as u32)?;
    a.set_label(&mut frame)?;
    a.call(read)?;
    a.mov(dword_ptr(DATA + 8), eax)?;
    a.set_label(&mut wait)?;
    a.call(read)?;
    a.sub(eax, dword_ptr(DATA + 8))?;
    a.mov(dword_ptr(DATA + 12), eax)?;
    a.cmp(eax, length)?;
    a.jb(wait)?;
    a.loop_(frame)?;
    pit(a, 0)?;
    a.lds(dx, dword_ptr(OLD))?;
    a.mov(ax, 0x2508)?;
    a.int(0x21)?;
    a.mov(ax, 0x4C00)?;
    a.int(0x21)?;
    a.set_label(&mut read)?;
    vtd_read(a)
}

#[test]
fn a_frame_limiter_that_reads_the_pit_is_skipped() {
    // 60 frames of 20,000 counts (about 17 ms each), the time read from
    // the PIT in every pass (#219): no pass leaves the machine as it was,
    // each leaves it the same function of the count it read.
    let stats = compare("pit_limiter", |a| pit_limiter(a, 2, 60, 20_000));
    assert!(stats.skipped > LIMITER_SKIPS, "{:?}", stats);
}

#[test]
fn a_frame_limiter_on_a_square_wave_count_is_skipped() {
    // In mode 3 the count runs down by two, twice a period: it reloads
    // halfway between two interrupts, where the skipped passes stop.
    let stats = compare("pit_limiter_mode_3", |a| pit_limiter(a, 3, 60, 20_000));
    assert!(stats.skipped > LIMITER_SKIPS, "{:?}", stats);
}

#[test]
fn a_delay_on_the_pit_count_is_skipped() {
    // A delay as DOS programs time short waits: latch the count, and wait
    // until it has gone down by the delay's counts, in 16 bits, with the
    // counts in registers.
    let stats = compare("pit_delay", |a| {
        let (mut delay, mut wait) = (a.create_label(), a.create_label());
        pit(a, 0)?;
        a.mov(cx, 300)?;
        a.set_label(&mut delay)?;
        a.xor(al, al)?;
        a.out(0x43, al)?;
        a.in_(al, 0x40)?;
        a.mov(bl, al)?;
        a.in_(al, 0x40)?;
        a.mov(bh, al)?;
        a.set_label(&mut wait)?;
        a.xor(al, al)?;
        a.out(0x43, al)?;
        a.in_(al, 0x40)?;
        a.mov(dl, al)?;
        a.in_(al, 0x40)?;
        a.mov(dh, al)?;
        a.mov(ax, bx)?;
        a.sub(ax, dx)?;
        a.cmp(ax, 10_000)?;
        a.jb(wait)?;
        a.loop_(delay)?;
        a.mov(ax, 0x4C00)?;
        a.int(0x21)
    });
    assert!(stats.skipped > LIMITER_SKIPS, "{:?}", stats);
}

#[test]
fn a_loop_that_adds_up_the_pit_count_is_not_skipped() {
    // Each pass adds the count it read to a sum: the machine after a pass
    // depends on every count read before, not on the last alone.
    let stats = compare("pit_sum", |a| {
        let mut wait = a.create_label();
        pit(a, VTD_COUNT)?;
        a.set_label(&mut wait)?;
        a.xor(al, al)?;
        a.out(0x43, al)?;
        a.in_(al, 0x40)?;
        a.mov(ah, al)?;
        a.in_(al, 0x40)?;
        a.xchg(ah, al)?;
        a.add(word_ptr(DATA + 16), ax)?;
        a.adc(word_ptr(DATA + 18), 0)?;
        a.mov(ah, 1)?;
        a.int(0x16)?;
        a.jz(wait)?;
        pit(a, 0)?;
        finish(a)
    });
    assert_eq!(stats.proofs, 0, "{:?}", stats);
}

/// A machine that runs the Windows 3.1 program `program` as T.EXE on the
/// built-in Windows, as `machine` runs a DOS one.
fn win16_machine(test: &str, name: &str, program: &[u8], core: CoreMode, skip: bool) -> Cpu {
    let mut cpu = machine_with(test, name, ("T.EXE", program), core, skip);
    cpu.bus.win32.enabled = true;
    cpu.host_console = Some(rust_dos::cpu::HostConsole::default());
    cpu.load_shell();
    assert!(cpu.load_executable("T.EXE", None));
    cpu
}

#[test]
fn a_windows_3_1_peek_message_loop_is_skipped() {
    // As Johnny Castaway's animation loop (#358): GetCurrentTime, and
    // PeekMessage(PM_NOREMOVE) finding nothing, until 300 ms passed, then
    // the exit code 1. Every service of the loop changes nothing but RAM
    // and registers, and the time it reads holds until the next
    // millisecond, which the passes skipped stop at.
    use win16::{CODE, Code, program};
    let mut code = Code::new();
    code.call("USER", 15).emit(&[0x8B, 0xF0]); // GetCurrentTime; mov si,ax
    let top = code.here();
    code.emit(&[0x1E]) // push ds
        .push(0x10) // the MSG at DS:10h
        .push(0)
        .push(0)
        .push(0)
        .push(0) // PM_NOREMOVE
        .call("USER", 109) // PeekMessage
        .call("USER", 15)
        .emit(&[0x2B, 0xC6]) // sub ax,si
        .emit(&[0x3D, 0x2C, 0x01]); // cmp ax,300
    let back = top as i32 - (code.here() as i32 + 2);
    code.emit(&[0x72, back as i8 as u8]) // jb top
        .emit(&[0xB0, 0x01])
        .exit_al();
    let program = program(&code, CODE);
    let test = "win16_peek";
    let mut a = win16_machine(test, "a", &program, CoreMode::Normal, false);
    let mut b = win16_machine(test, "b", &program, CoreMode::Dynamic, true);
    // What the skip moves on outside RAM and the registers, as the passes
    // would have, compared before every batch (`a` comes first): the count
    // of calls and when the task's turn started.
    let host = |cpu: &Cpu| (cpu.bus.win32.calls, cpu.bus.win32.slice_start);
    let mut first = None;
    let each = |n: usize, cpu: &mut Cpu| match first.take() {
        None => first = Some(host(cpu)),
        Some(a) => assert_eq!(a, host(cpu), "{test}: calls, turn before batch {n}"),
    };
    if let Err(e) = lockstep(&mut a, &mut b, 600, 100_000, each) {
        panic!("{}: {}\n  {:?}", test, e, b.bus.observe.stats);
    }
    assert_eq!(host(&a), host(&b), "{test}: calls, turn at the end");
    let stats = b.bus.observe.stats;
    println!("{}: {:?}", test, stats);
    assert_eq!((a.errorlevel, b.errorlevel), (1, 1), "the loop ended");
    // A skip ends at the next millisecond, where GetCurrentTime gives
    // another value, and at the default speed a millisecond holds only a
    // few passes, so less is skipped than in the frame limiters.
    assert!(stats.proofs >= 10 && stats.skipped > 200_000, "{:?}", stats);
}
