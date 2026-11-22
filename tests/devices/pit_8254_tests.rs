//! The 8254 behaviour the Intel data sheet gives, where a program can see
//! it: when a count written to a running mode 2 or 3 counter loads, mode
//! 3's odd counts, and the read-back command's count and status latches.

use rust_dos::bus::Bus;

fn bus() -> Bus {
    let mut bus = Bus::new(std::path::PathBuf::from("."));
    bus.set_cycles_per_ms(1000);
    bus
}

/// Write a control word and a 16-bit count to channel `channel` (0 or 2),
/// and return the tick the count was written at.
fn program(bus: &mut Bus, control: u8, count: u16) -> u64 {
    let port = 0x40 + (control >> 6) as u16;
    bus.io_write(0x43, control);
    write_count(bus, port, count)
}

/// Write a 16-bit count, LSB then MSB, and return the tick of the MSB.
fn write_count(bus: &mut Bus, port: u16, count: u16) -> u64 {
    bus.io_write(port, count as u8);
    bus.io_write(port, (count >> 8) as u8);
    bus.clock.now_ticks()
}

/// Move emulated time on to just before `tick`, so that a port write
/// that follows lands on it or a tick after.
fn wait_until(bus: &mut Bus, tick: u64) {
    bus.clock.icount = bus.clock.icount_at(tick).saturating_sub(1);
}

/// Read a 16-bit count from `port`, LSB then MSB.
fn read_count(bus: &mut Bus, port: u16) -> u16 {
    bus.io_read(port) as u16 | (bus.io_read(port) as u16) << 8
}

/// Run the timer without executing instructions up to tick `end`, and
/// return the instruction counts IRQ 0 was raised at.
fn irq0_edges(bus: &mut Bus, end: u64) -> Vec<u64> {
    let end = bus.clock.icount_at(end);
    bus.start_batch(end);
    let mut edges = Vec::new();
    loop {
        bus.clock.icount = bus.clock.deadline;
        if bus.clock.icount >= end {
            return edges;
        }
        bus.service_timers();
        if bus.pic.master.irr & 0x01 != 0 {
            bus.pic.master.irr &= !0x01;
            edges.push(bus.clock.icount);
        }
    }
}

/// Mode 3: "if a new count is written during counting, it will be loaded
/// at the end of the current half-cycle". Written in the high half, it
/// loads where the output goes low and runs the low half of its own
/// period first, so the next IRQ 0 (the output's rising edge) comes half
/// the new period after the old period's half.
#[test]
fn a_mode_3_count_written_in_the_high_half_loads_at_its_end() {
    let mut bus = bus();
    let start = program(&mut bus, 0x36, 1000);
    wait_until(&mut bus, start + 100);
    write_count(&mut bus, 0x40, 600);
    let edges = irq0_edges(&mut bus, start + 3000);
    // High until 500, then the new count's low half of 300 ticks, then
    // whole periods of 600.
    let at = |tick: u64| bus.clock.icount_at(start + tick);
    assert_eq!(edges, [at(800), at(1400), at(2000), at(2600)]);
}

/// Written in the low half, a mode 3 count loads at the end of that half,
/// which is the end of the period.
#[test]
fn a_mode_3_count_written_in_the_low_half_loads_at_the_period_end() {
    let mut bus = bus();
    let start = program(&mut bus, 0x36, 1000);
    wait_until(&mut bus, start + 700);
    write_count(&mut bus, 0x40, 600);
    let edges = irq0_edges(&mut bus, start + 2500);
    let at = |tick: u64| bus.clock.icount_at(start + tick);
    assert_eq!(edges, [at(1000), at(1600), at(2200)]);
}

/// A count written in the low half that a count loaded at the half-cycle
/// runs: it loads at the end of the new count's period, not the old one's.
#[test]
fn a_count_written_after_a_half_cycle_load_waits_for_the_new_period() {
    let mut bus = bus();
    let start = program(&mut bus, 0x36, 1000);
    wait_until(&mut bus, start + 100);
    write_count(&mut bus, 0x40, 600);
    wait_until(&mut bus, start + 600);
    write_count(&mut bus, 0x40, 400);
    let edges = irq0_edges(&mut bus, start + 2000);
    // 600's low half ends at 800, then whole periods of 400.
    let at = |tick: u64| bus.clock.icount_at(start + tick);
    assert_eq!(edges, [at(800), at(1200), at(1600)]);
}

/// Channel 0's count in mode 3 while a count written in the high half
/// loads: the old count's high half, then the new count from the top of
/// its low half.
#[test]
fn channel_0_counts_on_from_the_new_count_at_the_half_cycle() {
    let mut bus = bus();
    let start = program(&mut bus, 0x36, 1000);
    wait_until(&mut bus, start + 100);
    write_count(&mut bus, 0x40, 600);
    for wait in [150u64, 450, 520, 790, 830] {
        wait_until(&mut bus, start + wait);
        bus.io_write(0x43, 0x00);
        let t = bus.clock.now_ticks() - start;
        let count = read_count(&mut bus, 0x40) as u64;
        let expected = match t {
            ..500 => 1000 - 2 * t,
            500..800 => 600 - 2 * (t - 500),
            _ => 600 - 2 * (t - 800),
        };
        assert_eq!(count, expected, "{t} ticks in");
    }
}

/// Mode 3 with an odd count N: "the initial count minus one (an even
/// number) is loaded" and counts down by two in each half, the output
/// high for (N + 1) / 2 ticks and low for (N - 1) / 2.
#[test]
fn an_odd_mode_3_count_reads_even_values() {
    let mut bus = bus();
    bus.io_write(0x61, 0x01);
    let start = program(&mut bus, 0xB6, 7);
    for t in 0..21u64 {
        let (count, out) = bus.pit2.read(start + t);
        let into = t % 7;
        let (expected, high) = if into < 4 {
            (6 - 2 * into, true)
        } else {
            (6 - 2 * (into - 4), false)
        };
        assert_eq!((count as u64, out), (expected, high), "{t} ticks in");
    }
}

/// The speaker's channel 2, mode 3: a count written in the high half takes
/// over where the output goes low, which is what the speaker plays.
#[test]
fn a_channel_2_square_wave_changes_at_the_half_cycle() {
    let mut bus = bus();
    bus.io_write(0x61, 0x01);
    let start = program(&mut bus, 0xB6, 1000);
    wait_until(&mut bus, start + 100);
    write_count(&mut bus, 0x42, 600);
    let out = |t: u64| bus.pit2.read(start + t).1;
    assert!(out(499));
    assert!(!out(500));
    assert!(!out(799));
    assert!(out(800), "the new count's period starts at 800");
    assert!(out(1099));
    assert!(!out(1100));
    assert_eq!(bus.pit2.read(start + 600).0, 400);
    // The speaker's level over the old count's period: its high half and
    // the new count's first high ticks.
    let high = bus
        .pit2
        .high_ticks(start as f64 + 100.0, start as f64 + 1000.0);
    assert_eq!(high, 400.0 + 200.0);
}

/// Mode 2: "if a trigger is received after writing a new count but before
/// the end of the current period, the counter will be loaded with the
/// new count on the next CLK pulse".
#[test]
fn a_gate_trigger_loads_a_count_written_before_it() {
    let mut bus = bus();
    bus.io_write(0x61, 0x01);
    let start = program(&mut bus, 0xB4, 1000);
    wait_until(&mut bus, start + 100);
    write_count(&mut bus, 0x42, 300);
    wait_until(&mut bus, start + 200);
    bus.io_write(0x61, 0x00);
    wait_until(&mut bus, start + 300);
    bus.io_write(0x61, 0x01);
    let trigger = bus.clock.now_ticks();
    assert_eq!(bus.pit2.read(trigger + 10).0, 290);
    assert!(bus.pit2.read(trigger + 298).1);
    assert!(
        !bus.pit2.read(trigger + 299).1,
        "low for the period's last tick"
    );
}

/// The read-back command's status byte: OUT, NULL COUNT and the control
/// word's bits 5-0 as written, BCD and all.
#[test]
fn read_back_gives_the_status_byte() {
    let mut bus = bus();
    // Channel 0, mode 0, BCD: the output goes low at the control word,
    // and the count is null until one is written.
    bus.io_write(0x43, 0x31);
    bus.io_write(0x43, 0xE2);
    assert_eq!(bus.io_read(0x40), 0x71);
    let start = write_count(&mut bus, 0x40, 0x100);
    bus.io_write(0x43, 0xE2);
    assert_eq!(bus.io_read(0x40), 0x31, "loaded, still low");
    wait_until(&mut bus, start + 0x102);
    bus.io_write(0x43, 0xE2);
    assert_eq!(bus.io_read(0x40), 0xB1, "high at the terminal count");

    // Channel 2, mode 3 (written as mode 7), its gate low.
    bus.io_write(0x43, 0xBE);
    bus.io_write(0x43, 0xE8);
    assert_eq!(bus.io_read(0x42), 0xC0 | 0x3E);
    write_count(&mut bus, 0x42, 1000);
    bus.io_write(0x43, 0xE8);
    assert_eq!(bus.io_read(0x42), 0x80 | 0x3E);
    // A count written while it runs is null until the half-cycle ends.
    bus.io_write(0x61, 0x01);
    let start = bus.clock.now_ticks();
    write_count(&mut bus, 0x42, 500);
    bus.io_write(0x43, 0xE8);
    assert_eq!(bus.io_read(0x42), 0xC0 | 0x3E);
    wait_until(&mut bus, start + 600);
    bus.io_write(0x43, 0xE8);
    assert_eq!(bus.io_read(0x42), 0x3E, "loaded at 500, low half");

    // Channel 1 as the BIOS leaves it: mode 2, LSB only.
    bus.io_write(0x43, 0xE4);
    assert_eq!(bus.io_read(0x41) & 0x7F, 0x14);
}

/// With both latched, the first read gives the status and the next two
/// the count as it was at the command, however long they wait; a second
/// count latch before they are read changes nothing.
#[test]
fn read_back_latches_the_status_then_the_count() {
    let mut bus = bus();
    let start = program(&mut bus, 0x34, 50_000);
    wait_until(&mut bus, start + 1000);
    bus.io_write(0x43, 0xC2);
    let latched_at = bus.clock.now_ticks();
    wait_until(&mut bus, start + 5000);
    bus.io_write(0x43, 0xC2);
    assert_eq!(bus.io_read(0x40), 0xB4);
    let count = read_count(&mut bus, 0x40) as u64;
    assert_eq!(count, 50_000 - (latched_at - start));
    // Read out: a read now gives the live count.
    let live = read_count(&mut bus, 0x40) as u64;
    assert!(live < count - 3000, "{live}");
}

/// One read-back command can latch all three counters.
#[test]
fn read_back_latches_every_counter_selected() {
    let mut bus = bus();
    let start_0 = program(&mut bus, 0x34, 40_000);
    bus.io_write(0x61, 0x01);
    let start = program(&mut bus, 0xB4, 30_000);
    bus.io_write(0x43, 0xDE); // count only, counters 0, 1 and 2
    let latched_at = bus.clock.now_ticks();
    bus.clock.icount += 5000;
    let channel_2 = read_count(&mut bus, 0x42) as u64;
    assert_eq!(channel_2, 30_000 - (latched_at - start));
    let channel_0 = read_count(&mut bus, 0x40) as u64;
    assert_eq!(channel_0, 40_000 - (latched_at - start_0));
    let channel_1 = bus.io_read(0x41);
    assert!((1..=18).contains(&channel_1), "{channel_1}");
}

/// Channel 1 counts as the BIOS set it, and takes a new mode and count as
/// the others do.
#[test]
fn channel_1_counts() {
    let mut bus = bus();
    let seen: std::collections::BTreeSet<u8> = (0..40).map(|_| bus.io_read(0x41)).collect();
    assert!(seen.iter().all(|c| (1..=18).contains(c)), "{seen:?}");
    assert!(seen.len() > 1, "{seen:?}");
    let start = program(&mut bus, 0x74, 10_000);
    wait_until(&mut bus, start + 2000);
    bus.io_write(0x43, 0x40);
    let t = bus.clock.now_ticks() - start;
    assert_eq!(read_count(&mut bus, 0x41) as u64, 10_000 - t);
}

/// A count waiting for its half-cycle, the latches and channel 1 come
/// back from a save state, and a state saved without the 8254 section
/// loads with channel 1 as the BIOS leaves it and nothing latched.
#[test]
fn the_8254_state_survives_a_save_state() {
    use rust_dos::cpu::Cpu;
    use rust_dos::savestate::machine;
    let machine = || {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        cpu.bus.set_cycles_per_ms(1000);
        cpu.load_shell();
        cpu
    };
    let mut a = machine();
    let start = program(&mut a.bus, 0x36, 1000);
    wait_until(&mut a.bus, start + 100);
    write_count(&mut a.bus, 0x40, 600);
    program(&mut a.bus, 0x74, 5000);
    a.bus.io_write(0x43, 0xEE);
    let (bytes, parts) = machine::save_parts(&a);
    let mut b = machine();
    machine::load(&mut b, &bytes).unwrap();
    assert!(machine::save(&b) == bytes);
    for port in [0x40, 0x41, 0x42] {
        assert_eq!(
            b.bus.io_read(port),
            a.bus.io_read(port),
            "status of {port:02X}h"
        );
    }
    // A read-back after the load gives channel 1's mode and access as
    // the program wrote them.
    for bus in [&mut a.bus, &mut b.bus] {
        bus.io_write(0x43, 0xE4);
    }
    assert_eq!(b.bus.io_read(0x41), a.bus.io_read(0x41), "status of 41h");
    let tick = start + 790;
    assert_eq!(b.bus.pit0.count(tick), a.bus.pit0.count(tick));
    assert_eq!(b.bus.pit0.count(tick), 20, "the new count's low half");

    let range = |name: &str| {
        parts
            .iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("no {name}"))
            .1
            .clone()
    };
    // The section's header and its fields, up to the next section.
    let section = range("8254").start..range("TSC").start;
    let mut old = bytes[..section.start].to_vec();
    old.extend_from_slice(&bytes[section.end..]);
    let mut c = machine();
    machine::load(&mut c, &old).unwrap();
    assert!(c.bus.pit0_status.is_none());
    c.bus.io_write(0x43, 0xE4);
    assert_eq!(
        c.bus.io_read(0x41) & 0x7F,
        0x14,
        "channel 1 in mode 2, LSB only"
    );
}
