//! Deterministic mode (`rust_dos::deterministic`): a program that reads the
//! clock and waits for a key, run twice with the key pressed at the same
//! emulated time, ends in the same state after the same number of
//! instructions, however the host's frames cut the run up.

use chrono::{NaiveDate, NaiveDateTime};
use iced_x86::code_asm::*;
use rust_dos::cpu::Cpu;
use rust_dos::deterministic::Deterministic;
use rust_dos::exec::NoHook;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

/// Where the program leaves what it read: the BIOS's intra-application
/// communication area (0040:00F0), 16 bytes.
const RESULTS: usize = 0x4F0;
const CYCLES: u32 = 3000;
const RUN_MS: u64 = 3000;
/// The key goes down and up at these emulated milliseconds.
const KEY_DOWN_MS: u64 = 1200;
const KEY_UP_MS: u64 = 1300;

/// Read DOS's date and time into 0040:00F0, wait for a key and keep it,
/// read the time again, then write the PIT's count there forever.
fn program() -> Vec<u8> {
    let mut a = CodeAssembler::new(16).unwrap();
    let body = |a: &mut CodeAssembler| -> Result<(), IcedError> {
        a.mov(ax, 0x40)?;
        a.mov(es, ax)?;
        a.mov(ah, 0x2A)?;
        a.int(0x21)?;
        a.mov(word_ptr(0xF0).es(), cx)?;
        a.mov(word_ptr(0xF2).es(), dx)?;
        a.mov(ah, 0x2C)?;
        a.int(0x21)?;
        a.mov(word_ptr(0xF4).es(), cx)?;
        a.mov(word_ptr(0xF6).es(), dx)?;
        a.mov(ah, 0)?;
        a.int(0x16)?;
        a.mov(word_ptr(0xF8).es(), ax)?;
        a.mov(ah, 0x2C)?;
        a.int(0x21)?;
        a.mov(word_ptr(0xFA).es(), cx)?;
        a.mov(word_ptr(0xFC).es(), dx)?;
        let mut spin = a.create_label();
        a.set_label(&mut spin)?;
        a.in_(al, 0x40)?;
        a.mov(byte_ptr(0xFE).es(), al)?;
        a.jmp(spin)
    };
    body(&mut a).unwrap();
    a.assemble(0x100).unwrap()
}

struct Outcome {
    ram: String,
    executed: u64,
    results: [u8; 16],
}

/// Run the program from `start` for `RUN_MS`, in host frames that end
/// where the frame size `frame_ms` puts them, with the key pressed at its
/// times.
fn run(name: &str, start: NaiveDateTime, frame_ms: u64) -> Outcome {
    let dir = Path::new("target/deterministic_tests").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("T.COM"), program()).unwrap();
    let mut mode = Deterministic::new(start);
    let mut cpu = Cpu::new(dir);
    cpu.bus.set_cycles_per_ms(CYCLES);
    cpu.load_shell();
    cpu.pending_command = Some("T".to_string());
    let key = rust_dos::keyboard::lookup("a").unwrap();
    let end = cpu.bus.clock.icount_at_ns(RUN_MS * 1_000_000);
    let mut frame = 0;
    while cpu.bus.clock.icount < end {
        frame += 1;
        // An uneven frame size, as a host's frames have.
        let target = cpu.bus.clock.icount_at_ns((frame * frame_ms + frame % 3) * 1_000_000).min(end);
        let frame_end = mode.frame_end(&cpu, target);
        while cpu.bus.clock.icount < frame_end {
            let (_, reached) = mode.step(&mut cpu, &mut NoHook, false, frame_end);
            if let Some(b) = reached.filter(|b| b.tick) {
                if b.ms == KEY_DOWN_MS {
                    rust_dos::keyboard::apply_key(&mut cpu.bus, key, key.ascii, true);
                } else if b.ms == KEY_UP_MS {
                    rust_dos::keyboard::apply_key(&mut cpu.bus, key, 0, false);
                }
            }
        }
        // What a frame does at its end: drain the sound.
        cpu.bus.audio_catch_up();
        cpu.bus.audio_out.clear();
    }
    let mut results = [0u8; 16];
    results.copy_from_slice(&cpu.bus.ram()[RESULTS..RESULTS + 16]);
    let ram = Sha256::digest(cpu.bus.ram()).iter().map(|b| format!("{:02x}", b)).collect();
    Outcome { ram, executed: cpu.executed, results }
}

fn word(results: &[u8; 16], at: usize) -> u16 {
    u16::from_le_bytes([results[at], results[at + 1]])
}

fn start() -> NaiveDateTime {
    rust_dos::deterministic::default_start()
}

#[test]
fn two_runs_end_in_the_same_state() {
    // Frames of 16 and 37 ms, as two hosts of different speeds would run.
    let a = run("a", start(), 16);
    let b = run("b", start(), 37);
    assert_eq!(a.results, b.results);
    assert_eq!(a.executed, b.executed);
    assert_eq!(a.ram, b.ram);
    // The program got the key.
    assert_eq!(word(&a.results, 8) & 0xFF, b'a' as u16, "{:02X?}", a.results);
}

#[test]
fn the_clock_starts_at_the_start_time_and_runs_with_emulated_time() {
    let a = run("clock", start(), 16);
    let r = &a.results;
    // DOS's date: 1995-04-11.
    assert_eq!((word(r, 0), word(r, 2)), (1995, 0x040B), "{:02X?}", r);
    // DOS's time before the key: 12:34:56, plus the start-up's
    // fraction of a second.
    assert_eq!(word(r, 4), 0x0C22, "{:02X?}", r);
    assert_eq!(word(r, 6) >> 8, 56, "{:02X?}", r);
    // After the key, pressed 1.2 s of emulated time in: 12:34:57.
    assert_eq!((word(r, 10), word(r, 12) >> 8), (0x0C22, 57), "{:02X?}", r);
}

#[test]
fn another_start_time_gives_another_date() {
    let at = NaiveDate::from_ymd_opt(1991, 12, 31).unwrap().and_hms_opt(23, 0, 0).unwrap();
    let a = run("other", at, 16);
    assert_eq!((word(&a.results, 0), word(&a.results, 2)), (1991, 0x0C1F), "{:02X?}", a.results);
}

/// The machine with T.COM started, at the start of its run.
fn machine(name: &str) -> (Deterministic, Cpu) {
    let dir = Path::new("target/deterministic_tests").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("T.COM"), program()).unwrap();
    let mode = Deterministic::new(start());
    let mut cpu = Cpu::new(dir);
    cpu.bus.set_cycles_per_ms(CYCLES);
    cpu.load_shell();
    cpu.pending_command = Some("T".to_string());
    (mode, cpu)
}

/// Run to `ms` of emulated time in 16 ms frames, with the key pressed at
/// its times. Returns the ticks reached, in emulated ms.
fn run_to(mode: &mut Deterministic, cpu: &mut Cpu, ms: u64) -> Vec<u64> {
    let key = rust_dos::keyboard::lookup("a").unwrap();
    let end = cpu.bus.clock.icount_at_ns(ms * 1_000_000);
    let mut ticks = Vec::new();
    while cpu.bus.clock.icount < end {
        let target = cpu.bus.clock.icount_at_ns(cpu.bus.clock.now_ns() + 16_000_000).min(end);
        let frame_end = mode.frame_end(cpu, target);
        while cpu.bus.clock.icount < frame_end {
            let (_, reached) = mode.step(cpu, &mut NoHook, false, frame_end);
            if let Some(b) = reached.filter(|b| b.tick) {
                ticks.push(b.ms);
                if b.ms == KEY_DOWN_MS {
                    rust_dos::keyboard::apply_key(&mut cpu.bus, key, key.ascii, true);
                } else if b.ms == KEY_UP_MS {
                    rust_dos::keyboard::apply_key(&mut cpu.bus, key, 0, false);
                }
            }
        }
        cpu.bus.audio_catch_up();
        cpu.bus.audio_out.clear();
    }
    ticks
}

fn ram_hash(cpu: &Cpu) -> String {
    Sha256::digest(cpu.bus.ram()).iter().map(|b| format!("{:02x}", b)).collect()
}

/// A state saved at 1 s and loaded at 2.5 s brings back the clock of 1 s,
/// the ticks go on every 10 ms from there, and the run from it ends where
/// a run without the load does.
#[test]
fn a_loaded_state_brings_its_clock_and_ticks_back() {
    const SAVE_MS: u64 = 1000;
    let (mut mode, mut cpu) = machine("load");
    run_to(&mut mode, &mut cpu, SAVE_MS);
    let state = rust_dos::savestate::machine::save(&cpu);
    let clock_at_save = cpu.bus.cmos.now();
    assert_eq!(rust_dos::hosttime::now().naive_local(), start() + chrono::TimeDelta::milliseconds(SAVE_MS as i64));

    run_to(&mut mode, &mut cpu, 2500);
    assert!(cpu.bus.cmos.now() > clock_at_save);
    rust_dos::savestate::machine::load(&mut cpu, &state).unwrap();
    mode.resync(&cpu);
    assert_eq!(cpu.bus.clock.now_ns() / 1_000_000, SAVE_MS);
    assert_eq!(cpu.bus.cmos.now(), clock_at_save, "the machine's clock is the state's");

    // The first tick after the load is the one after the save's.
    let ticks = run_to(&mut mode, &mut cpu, RUN_MS);
    assert_eq!(ticks.first(), Some(&(SAVE_MS + 10)), "{:?}", &ticks[..ticks.len().min(5)]);
    assert!(ticks.windows(2).all(|w| w[1] == w[0] + 10));

    let (mut mode, mut straight) = machine("straight");
    let ticks = run_to(&mut mode, &mut straight, RUN_MS);
    assert!(ticks.windows(2).all(|w| w[1] == w[0] + 10));
    assert_eq!(cpu.bus.clock.icount, straight.bus.clock.icount);
    assert_eq!(cpu.bus.cmos.now(), straight.bus.cmos.now());
    assert_eq!(ram_hash(&cpu), ram_hash(&straight), "the same memory");
    // The program got the key after the load.
    assert_eq!(cpu.bus.ram()[RESULTS + 8], b'a');
}
