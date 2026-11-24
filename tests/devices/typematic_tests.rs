//! The keyboard's typematic repeat: a held key sends its make code again
//! after the delay and then at the rate that command F3h or INT 16h
//! AH=03h set, in emulated time, whatever the host's key repeat does.

use rust_dos::bus::Bus;
use rust_dos::cpu::Cpu;
use rust_dos::keyboard::key_event;
use std::path::PathBuf;

const A: u8 = 0x1E;
const B: u8 = 0x30;

fn cpu() -> Box<Cpu> {
    let mut cpu = Cpu::new(PathBuf::from("."));
    cpu.bus.set_cycles_per_ms(1000);
    cpu
}

/// Run the machine's timers for `ms` milliseconds of emulated time, a
/// tenth of a millisecond at a time.
fn wait_ms(bus: &mut Bus, ms: u64) {
    for _ in 0..ms * 10 {
        bus.clock.icount += 100;
        if bus.clock.icount >= bus.clock.deadline {
            bus.service_timers();
        }
    }
}

/// The make codes of `scan` the keyboard sent since the last call, read
/// from port 60h as a keyboard handler reads them.
fn makes(bus: &mut Bus, scan: u8) -> usize {
    let mut count = 0;
    loop {
        if bus.io_read(0x64) & 1 == 0 {
            bus.kbc.release();
            if bus.io_read(0x64) & 1 == 0 {
                return count;
            }
        }
        if bus.io_read(0x60) == scan {
            count += 1;
        }
    }
}

#[test]
fn a_held_key_repeats_after_half_a_second_at_eleven_a_second() {
    let mut cpu = cpu();
    let bus = &mut cpu.bus;
    key_event(bus, A, false, true, None);
    assert_eq!(makes(bus, A), 1);
    wait_ms(bus, 495);
    assert_eq!(makes(bus, A), 0, "no repeat before the 500 ms delay");
    wait_ms(bus, 10);
    assert_eq!(makes(bus, A), 1, "the first repeat at 500 ms");
    // 10.9 a second: one every 91.7 ms, the tenth at 1417 ms.
    wait_ms(bus, 1000);
    assert_eq!(makes(bus, A), 10);
    // Each repeat types the key again.
    assert_eq!(bus.keyboard_buffer.len(), 12);
    assert!(bus.keyboard_buffer.iter().all(|&k| k == 0x1E61));
    key_event(bus, A, false, false, None);
    assert_eq!(makes(bus, A), 0);
    wait_ms(bus, 1000);
    assert_eq!(makes(bus, A), 0, "a released key stops repeating");
}

#[test]
fn the_host_repeating_a_held_key_sends_nothing() {
    let mut cpu = cpu();
    let bus = &mut cpu.bus;
    key_event(bus, A, false, true, None);
    for _ in 0..20 {
        wait_ms(bus, 20);
        key_event(bus, A, false, true, None);
    }
    assert_eq!(makes(bus, A), 1, "400 ms in, before the keyboard's delay");
}

#[test]
fn the_last_key_down_repeats_and_another_releasing_leaves_it() {
    let mut cpu = cpu();
    let bus = &mut cpu.bus;
    key_event(bus, A, false, true, None);
    wait_ms(bus, 600);
    key_event(bus, B, false, true, None);
    makes(bus, A);
    makes(bus, B);
    wait_ms(bus, 400);
    assert_eq!(makes(bus, A), 0, "A, still held, no longer repeats");
    assert_eq!(makes(bus, B), 0, "B waits for its own delay");
    wait_ms(bus, 200);
    assert_eq!(makes(bus, B), 2, "B repeats at 500 and 592 ms");
    // A coming up doesn't stop B.
    key_event(bus, A, false, false, None);
    wait_ms(bus, 100);
    assert_eq!(makes(bus, B), 1);
}

#[test]
fn command_f3h_sets_the_rate_and_delay() {
    let mut cpu = cpu();
    let bus = &mut cpu.bus;
    // 250 ms, then 30 a second (33.4 ms).
    bus.io_write(0x60, 0xF3);
    bus.io_write(0x60, 0x00);
    makes(bus, 0xFA);
    assert_eq!(bus.kbc.typematic, 0x00);
    key_event(bus, A, false, true, None);
    wait_ms(bus, 255);
    assert_eq!(makes(bus, A), 2, "the press and the repeat at 250 ms");
    wait_ms(bus, 334);
    assert_eq!(makes(bus, A), 10);
    key_event(bus, A, false, false, None);
    // Set Default (F6h) goes back to 500 ms and 10.9 a second.
    bus.io_write(0x60, 0xF6);
    assert_eq!(bus.kbc.typematic, 0x2B);
}

#[test]
fn int_16h_ah_03h_sets_and_reads_the_typematic_rate() {
    let mut cpu = cpu();
    // AL=05h: a 1 s delay (BH=3) and 2 a second (BL=1Fh).
    cpu.set_ax(0x0305);
    cpu.set_bx(0x031F);
    rust_dos::interrupts::int16::handle(&mut cpu);
    assert_eq!(cpu.bus.kbc.typematic, 0x7F);
    cpu.set_ax(0x0306);
    cpu.set_bx(0);
    rust_dos::interrupts::int16::handle(&mut cpu);
    assert_eq!(cpu.bx(), 0x031F);
    let bus = &mut cpu.bus;
    key_event(bus, A, false, true, None);
    wait_ms(bus, 990);
    assert_eq!(makes(bus, A), 1);
    wait_ms(bus, 20);
    assert_eq!(makes(bus, A), 1, "the repeat at 1 s");
    wait_ms(bus, 480);
    assert_eq!(makes(bus, A), 0);
    wait_ms(bus, 20);
    assert_eq!(makes(bus, A), 1, "the next 500 ms later");
    // AL=00h: the defaults.
    cpu.set_ax(0x0300);
    rust_dos::interrupts::int16::handle(&mut cpu);
    assert_eq!(cpu.bus.kbc.typematic, 0x2B);
}

#[test]
fn a_save_state_keeps_the_rate_and_the_repeating_key() {
    let mut cpu = cpu();
    cpu.bus.io_write(0x60, 0xF3);
    cpu.bus.io_write(0x60, 0x20);
    makes(&mut cpu.bus, 0xFA);
    key_event(&mut cpu.bus, A, false, true, None);
    makes(&mut cpu.bus, A);
    wait_ms(&mut cpu.bus, 300);
    let state = rust_dos::savestate::machine::save(&cpu);
    let mut loaded = Cpu::new(PathBuf::from("."));
    rust_dos::savestate::machine::load(&mut loaded, &state).unwrap();
    assert_eq!(loaded.bus.kbc.typematic, 0x20);
    // 500 ms of delay: the repeat comes 200 ms after the load.
    wait_ms(&mut loaded.bus, 190);
    assert_eq!(makes(&mut loaded.bus, A), 0);
    wait_ms(&mut loaded.bus, 20);
    assert_eq!(makes(&mut loaded.bus, A), 1);
}

#[test]
fn the_built_in_windows_repeats_keys_at_its_keyboard_speed() {
    use rust_dos::win32::user32::sysparams::{program_keyboard, system_parameters_info};
    let mut cpu = cpu();
    // Windows' defaults: speed 31 (30 a second), delay 1 (500 ms).
    program_keyboard(&mut cpu);
    assert_eq!(cpu.bus.kbc.typematic, 0x20);
    // SPI_SETKEYBOARDSPEED 0 (2 a second), SPI_SETKEYBOARDDELAY 3 (1 s).
    system_parameters_info(&mut cpu, &[11, 0, 0, 0]);
    assert_eq!(cpu.bus.kbc.typematic, 0x3F);
    system_parameters_info(&mut cpu, &[23, 3, 0, 0]);
    assert_eq!(cpu.bus.kbc.typematic, 0x7F);
}

#[test]
fn key_repeat_delay_replaces_the_typematic_delay() {
    let mut cpu = cpu();
    let bus = &mut cpu.bus;
    // 30 ms, below any delay a keyboard has; the rate stays the
    // program's (10.9 a second here).
    bus.kbd.repeat_delay = Some(30);
    key_event(bus, A, false, true, None);
    assert_eq!(makes(bus, A), 1);
    wait_ms(bus, 25);
    assert_eq!(makes(bus, A), 0);
    wait_ms(bus, 10);
    assert_eq!(makes(bus, A), 1, "the first repeat at 30 ms");
    wait_ms(bus, 92);
    assert_eq!(makes(bus, A), 1, "the next a period later");
    key_event(bus, A, false, false, None);
    // Off: the typematic delay again.
    bus.kbd.repeat_delay = None;
    key_event(bus, A, false, true, None);
    makes(bus, A);
    wait_ms(bus, 495);
    assert_eq!(makes(bus, A), 0);
}
