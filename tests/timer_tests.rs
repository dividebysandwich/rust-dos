use rust_dos::bus::Bus;
use rust_dos::cpu::Cpu;
use rust_dos::timer::{Clock, CpuSpeed, PIT_HZ, Pacer};
use std::time::{Duration, Instant};

fn bus_at(cycles_per_ms: u32) -> Bus {
    let mut bus = Bus::new(std::path::PathBuf::from("."));
    bus.set_cycles_per_ms(cycles_per_ms);
    bus
}

/// Program PIT channel 0 with a control word and a 16-bit count.
fn program_pit0(bus: &mut Bus, control: u8, count: u16) {
    bus.io_write(0x43, control);
    bus.io_write(0x40, count as u8);
    bus.io_write(0x40, (count >> 8) as u8);
}

/// Run `instructions` without executing any, the way the main loop services
/// timer deadlines, acknowledging each IRQ 0 at once. Returns the
/// instruction counts at which IRQ 0 was raised.
fn run_timer(bus: &mut Bus, instructions: u64) -> Vec<u64> {
    let end = bus.clock.icount + instructions;
    bus.start_batch(end);
    let mut edges = Vec::new();
    loop {
        bus.clock.icount = bus.clock.deadline;
        if bus.clock.icount >= end {
            break;
        }
        bus.service_timers();
        if bus.pic.master.irr & 0x01 != 0 {
            bus.pic.master.irr &= !0x01;
            edges.push(bus.clock.icount);
        }
    }
    edges
}

#[test]
fn clock_converts_instructions_to_pit_ticks() {
    let mut clock = Clock::new(1000); // one million instructions per second
    clock.icount = 1_000_000;
    assert_eq!(clock.now_ticks(), PIT_HZ);

    for ticks in [1, 7950, 65536, 1_000_003] {
        let at = clock.icount_at(ticks);
        clock.icount = at;
        assert!(clock.now_ticks() >= ticks);
        clock.icount = at - 1;
        assert!(
            clock.now_ticks() < ticks,
            "icount_at must be the first instruction"
        );
    }
}

#[test]
fn clock_speed_change_keeps_time_continuous() {
    let mut clock = Clock::new(1000);
    clock.icount = 500_000;
    let before = clock.now_ticks();
    clock.set_cycles_per_ms(4000);
    assert_eq!(clock.now_ticks(), before);
    clock.icount += 4_000_000; // one second at the new speed
    assert_eq!(clock.now_ticks(), before + PIT_HZ);
}

#[test]
fn nanoseconds_stay_continuous_across_speed_changes() {
    let mut clock = Clock::new(1000);
    clock.icount = 500_000;
    assert_eq!(clock.now_ns(), 500_000_000);
    clock.set_cycles_per_ms(4000);
    assert_eq!(clock.now_ns(), 500_000_000);
    clock.icount += 4_000; // one millisecond at the new speed
    assert_eq!(clock.now_ns(), 501_000_000);
}

#[test]
fn default_timer_fires_at_18_2_hz() {
    let mut bus = bus_at(1000);
    let edges = run_timer(&mut bus, 10_000_000); // ten emulated seconds
    assert_eq!(edges.len(), 182);
}

#[test]
fn reprogrammed_timer_fires_at_the_new_rate() {
    // Carrier Command: control word 34h (mode 2), count 1F0Eh = 150 Hz.
    let mut bus = bus_at(1000);
    program_pit0(&mut bus, 0x34, 0x1F0E);
    let edges = run_timer(&mut bus, 1_000_000);
    assert_eq!(edges.len(), 150);

    // Evenly spaced: 7950 ticks is 6663 instructions at this speed.
    for pair in edges.windows(2) {
        let gap = pair[1] - pair[0];
        assert!((6662..=6664).contains(&gap), "gap {}", gap);
    }
}

#[test]
fn count_written_while_counting_applies_at_the_next_reload() {
    let mut bus = bus_at(1000);
    program_pit0(&mut bus, 0x34, 10_000);
    let first = run_timer(&mut bus, 9_000); // IRQ at 10000 ticks = 8381 instructions
    assert_eq!(first.len(), 1);

    // Mode 2 without a new control word: the running period finishes first.
    bus.io_write(0x40, (20_000u16 & 0xFF) as u8);
    bus.io_write(0x40, (20_000u16 >> 8) as u8);
    let edges = run_timer(&mut bus, 40_000);
    let at = |ticks: u64| bus.clock.icount_at(ticks);
    assert_eq!(edges, vec![at(20_000), at(40_000)]);
}

#[test]
fn mode_0_fires_once_per_count() {
    let mut bus = bus_at(1000);
    program_pit0(&mut bus, 0x30, 1000); // mode 0, interrupt on terminal count
    assert_eq!(run_timer(&mut bus, 100_000).len(), 1);

    // Writing a new count re-arms it.
    bus.io_write(0x40, (1000u16 & 0xFF) as u8);
    bus.io_write(0x40, (1000u16 >> 8) as u8);
    assert_eq!(run_timer(&mut bus, 100_000).len(), 1);
}

#[test]
fn mode_0_count_runs_on_through_zero() {
    // Pinball Fantasies' sound drivers chain one-shot timer interrupts
    // through a frame: in the handler they read the count, which has run
    // past zero by the interrupt latency, and subtract that from the next
    // delay. A count that restarted from the reload value would push the
    // next interrupt a whole period late.
    let mut bus = bus_at(1000);
    program_pit0(&mut bus, 0x30, 1000);
    assert_eq!(run_timer(&mut bus, 1000).len(), 1); // at 1000 ticks = 839 instructions
    bus.io_write(0x43, 0x00);
    let elapsed = bus.clock.now_ticks() - 1000;
    let count = bus.io_read(0x40) as u16 | (bus.io_read(0x40) as u16) << 8;
    assert_eq!(count, 0u16.wrapping_sub(elapsed as u16));
}

#[test]
fn control_word_stops_the_timer_until_a_count_is_written() {
    let mut bus = bus_at(1000);
    bus.io_write(0x43, 0x34);
    assert!(run_timer(&mut bus, 1_000_000).is_empty());
}

#[test]
fn a_control_word_stops_the_count_where_it_got_to() {
    // Second Reality times a frame so: count FFFFh in mode 3 from one
    // retrace, and at the next the control word again, and read the count.
    let mut bus = bus_at(1000);
    program_pit0(&mut bus, 0x36, 0xFFFF);
    let start = bus.clock.now_ticks();
    bus.clock.icount += 10_000;
    bus.io_write(0x43, 0x36);
    let elapsed = bus.clock.now_ticks() - start;
    let read = |bus: &mut Bus| bus.io_read(0x40) as u16 | (bus.io_read(0x40) as u16) << 8;
    let stopped = read(&mut bus);
    assert_eq!(stopped as u64, 0xFFFF - 2 * elapsed);
    bus.clock.icount += 10_000;
    assert_eq!(read(&mut bus), stopped, "still where it stopped");
}

#[test]
fn mode_3_counts_down_by_two_twice_a_period() {
    let mut bus = bus_at(1000);
    program_pit0(&mut bus, 0x36, 1000);
    let start = bus.clock.now_ticks();
    for wait in [100, 400, 700] {
        bus.clock.icount += wait;
        bus.io_write(0x43, 0x00); // latch channel 0
        let elapsed = bus.clock.now_ticks() - start;
        let count = bus.io_read(0x40) as u16 | (bus.io_read(0x40) as u16) << 8;
        assert_eq!(count as u64, 1000 - 2 * elapsed % 1000, "after {} ticks", elapsed);
    }
    // Still one IRQ 0 a period: 1000 ticks is 838 instructions at this speed.
    let edges = run_timer(&mut bus, 100_000);
    for pair in edges.windows(2) {
        let gap = pair[1] - pair[0];
        assert!((838..=839).contains(&gap), "gap {}", gap);
    }
}

#[test]
fn counter_reads_follow_emulated_time() {
    let mut bus = bus_at(1000);
    program_pit0(&mut bus, 0x34, 50_000);
    // Returns the latched count and the emulated time it was latched at.
    let read = |bus: &mut Bus| {
        bus.io_write(0x43, 0x00); // latch channel 0
        let latched_at = bus.clock.now_ticks();
        let lo = bus.io_read(0x40) as u16;
        let hi = bus.io_read(0x40) as u16;
        ((hi << 8) | lo, latched_at)
    };
    let (start, t0) = read(&mut bus);
    bus.clock.icount += 1000; // 1193 PIT ticks
    let (later, t1) = read(&mut bus);
    // The count ran down by exactly the elapsed time: the 1000 instructions
    // plus the time the port accesses took.
    assert_eq!((start - later) as u64, t1 - t0);
    assert!((1193..1200).contains(&(t1 - t0)), "{}", t1 - t0);
}

#[test]
fn port_accesses_take_bus_time() {
    // AdLib detection in the MicroProse driver: start the 80 us timer, then
    // wait by reading the status port 200 times. On an ISA bus that takes
    // about 200 us, however fast the CPU is.
    let mut bus = bus_at(50_000);
    bus.io_write(0x388, 0x02);
    bus.io_write(0x389, 0xFF);
    bus.io_write(0x388, 0x04);
    bus.io_write(0x389, 0x21);
    let start = bus.clock.now_micros();
    let mut status = 0;
    for _ in 0..200 {
        status = bus.io_read(0x388);
        bus.clock.icount += 2; // IN AL,DX / LOOP
    }
    assert!(bus.clock.now_micros() - start >= 200);
    assert_eq!(status & 0xE0, 0xC0, "timer 1 expired during the wait");
}

#[test]
fn pic_blocks_lower_priority_until_eoi() {
    let mut bus = bus_at(1000);
    bus.pic.raise(0);
    bus.pic.raise(1);

    assert_eq!(bus.pic_pending_irq(), Some(0));
    bus.pic_acknowledge(0);
    // Timer in service: the keyboard waits.
    assert_eq!(bus.pic_pending_irq(), None);
    assert_eq!(
        bus.io_read(0x20),
        0x02,
        "IRR shows the waiting keyboard IRQ"
    );

    bus.io_write(0x20, 0x20); // non-specific EOI
    assert_eq!(bus.pic.master.isr, 0);
    assert_eq!(bus.pic_pending_irq(), Some(1));
}

#[test]
fn pic_honours_the_mask_and_reads_it_back() {
    let mut bus = bus_at(1000);
    bus.pic.raise(0);
    bus.io_write(0x21, 0x01);
    assert_eq!(bus.pic_pending_irq(), None);
    assert_eq!(bus.io_read(0x21), 0x01);

    bus.io_write(0x21, 0x00);
    assert_eq!(bus.pic_pending_irq(), Some(0));
}

#[test]
fn pic_ocw3_selects_the_in_service_register() {
    let mut bus = bus_at(1000);
    bus.pic.raise(0);
    bus.pic_acknowledge(0);
    bus.io_write(0x20, 0x0B); // OCW3: read ISR
    assert_eq!(bus.io_read(0x20), 0x01);
    bus.io_write(0x20, 0x61); // specific EOI for IRQ 1: IRQ 0 stays in service
    assert_eq!(bus.io_read(0x20), 0x01);
    bus.io_write(0x20, 0x60); // specific EOI for IRQ 0
    assert_eq!(bus.io_read(0x20), 0x00);
}

#[test]
fn bios_timer_handler_sends_eoi() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    cpu.bus.pic.raise(0);
    cpu.bus.pic_acknowledge(0);
    rust_dos::interrupts::handle_hle(&mut cpu, 0x08);
    assert_eq!(cpu.bus.pic.master.isr, 0);
    assert_eq!(cpu.bus.read_16(0x046C), 1);
}

#[test]
fn bios_timer_handler_counts_down_the_diskette_motor() {
    // Lightspeed sets the motor-off count to 1 and waits for a tick to
    // clear it; when it runs out, the motors are off.
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    cpu.bus.write_8(0x0440, 2);
    cpu.bus.write_8(0x043F, 0x81);
    rust_dos::interrupts::handle_hle(&mut cpu, 0x08);
    assert_eq!((cpu.bus.read_8(0x0440), cpu.bus.read_8(0x043F)), (1, 0x81));
    rust_dos::interrupts::handle_hle(&mut cpu, 0x08);
    assert_eq!((cpu.bus.read_8(0x0440), cpu.bus.read_8(0x043F)), (0, 0x80));
    // Run out, it stays at 0.
    rust_dos::interrupts::handle_hle(&mut cpu, 0x08);
    assert_eq!(cpu.bus.read_8(0x0440), 0);
}

#[test]
fn bios_timer_handler_returns_with_the_interrupted_flags() {
    use rust_dos::cpu::CpuFlags;
    // A program's ISR chained to ours: the IRET must restore the flags of
    // the code the tick interrupted, whatever the handler left in them.
    for (vector, service) in [(0x08, false), (0x09, false), (0x21, true)] {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        cpu.set_ss(0x3000);
        cpu.set_sp(0x0100);
        let interrupted = CpuFlags::IF | CpuFlags::CF | CpuFlags::DF;
        cpu.set_cpu_flags(interrupted);
        cpu.push(cpu.flags16());
        cpu.push(0x1234);
        cpu.push(0x0100);
        cpu.set_cpu_flags(CpuFlags::ZF);

        rust_dos::interrupts::return_from_hle(&mut cpu, vector);

        assert_eq!((cpu.cs(), cpu.ip(), cpu.sp()), (0x1234, 0x0100, 0x0100));
        assert!(cpu.get_cpu_flag(CpuFlags::IF));
        // Services hand back their CF/ZF results and clear DF.
        assert_eq!(cpu.get_cpu_flag(CpuFlags::CF), !service, "CF, INT {vector:02X}h");
        assert_eq!(cpu.get_cpu_flag(CpuFlags::ZF), service, "ZF, INT {vector:02X}h");
        assert_eq!(cpu.get_cpu_flag(CpuFlags::DF), !service, "DF, INT {vector:02X}h");
    }
}

#[test]
fn shell_reload_resets_timer_and_pic() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    program_pit0(&mut cpu.bus, 0x34, 0x1F0E);
    cpu.bus.io_write(0x21, 0xFF);
    cpu.bus.pic.raise(0);
    cpu.bus.pic_acknowledge(0);

    cpu.bus.clock.icount += 5_000_000;

    cpu.load_shell();
    assert_eq!(cpu.bus.pit0.reload(), 0x10000);
    // The BIOS rate restarts from now.
    let now = cpu.bus.clock.now_ticks();
    assert_eq!(cpu.bus.pit0.next_event(), Some(now + 0x10000));
    assert_eq!(
        (cpu.bus.pic.master.imr, cpu.bus.pic.master.isr, cpu.bus.pic.master.irr),
        (0, 0, 0)
    );
}

#[test]
fn adlib_timer_runs_on_emulated_time() {
    // The usual AdLib detection: start timer 1 at FFh (80 us), wait, check
    // the status register for the expired bits.
    let mut bus = bus_at(1000);
    let adlib_write = |bus: &mut Bus, reg: u8, value: u8| {
        bus.io_write(0x388, reg);
        bus.io_write(0x389, value);
    };
    adlib_write(&mut bus, 0x04, 0x60);
    adlib_write(&mut bus, 0x04, 0x80);
    assert_eq!(bus.io_read(0x388) & 0xE0, 0x00);
    adlib_write(&mut bus, 0x02, 0xFF);
    adlib_write(&mut bus, 0x04, 0x21);

    std::thread::sleep(Duration::from_millis(2)); // wall time doesn't count
    assert_eq!(bus.io_read(0x388) & 0xE0, 0x00);
    bus.clock.icount += 100; // 100 us at 1000 instructions per ms
    assert_eq!(bus.io_read(0x388) & 0xE0, 0xC0);
}

#[test]
fn pacer_runs_emulated_time_up_to_the_wall_clock() {
    let start = Instant::now();
    let clock = Clock::new(1000);
    let mut pacer = Pacer::new(CpuSpeed::Fixed(1000), start);
    // 20 ms of wall time at 1000 instructions per ms.
    let end = pacer.batch_end(&clock, start + Duration::from_millis(20));
    assert!((19_999..=20_001).contains(&end), "end {}", end);
}

#[test]
fn pacer_drops_a_large_backlog() {
    let start = Instant::now();
    let clock = Clock::new(1000);
    let mut pacer = Pacer::new(CpuSpeed::Fixed(1000), start);
    // A two second stall: run one frame, not two seconds' worth.
    let end = pacer.batch_end(&clock, start + Duration::from_secs(2));
    assert!((16_000..=17_000).contains(&end), "end {}", end);
}

#[test]
fn auto_speed_follows_the_program() {
    let start = Instant::now();
    let fast = Duration::from_millis(2);
    let mut bus = bus_at(3000);
    let mut pacer = Pacer::new(CpuSpeed::Auto(3000), start);
    // Run `ms` of emulated time, a frame at a time, moving `io` bytes on
    // a drive in each, returning the speed the pacer asks for last.
    let run = |bus: &mut Bus, pacer: &mut Pacer, protected: bool, io: u64, ms: u64| {
        let mut asked = None;
        for _ in 0..ms / 16 {
            bus.clock.icount += bus.clock.cycles_per_ms() as u64 * 16;
            bus.activity.io_bytes += io;
            if let Some(speed) = pacer.end_frame(bus, protected, 333_000, fast, Duration::ZERO) {
                bus.set_cycles_per_ms(speed);
                asked = Some(speed);
            }
        }
        asked
    };
    // A real-mode program that shows nothing stays at the real-mode speed.
    assert_eq!(run(&mut bus, &mut pacer, false, 0, 2000), None);
    assert_eq!(bus.clock.cycles_per_ms(), 3000);
    // A protected-mode one at work (loading) gets what the host has.
    run(&mut bus, &mut pacer, true, 10_000, 2000);
    assert!(bus.clock.cycles_per_ms() > 100_000, "{}", bus.clock.cycles_per_ms());
    // Done, it goes back at once, before a delay loop it times next can
    // run at that speed.
    assert_eq!(run(&mut bus, &mut pacer, true, 0, 160), Some(20_000));
    run(&mut bus, &mut pacer, true, 10_000, 2000);
    assert!(bus.clock.cycles_per_ms() > 100_000, "{}", bus.clock.cycles_per_ms());
    // One that does nothing goes down to a 486's speed.
    run(&mut bus, &mut pacer, true, 0, 20_000);
    assert_eq!(bus.clock.cycles_per_ms(), 20_000);
    // Once it ends, whatever runs next starts from the real-mode speed.
    assert_eq!(run(&mut bus, &mut pacer, false, 0, 16), Some(3000));
}

#[test]
fn auto_speed_parses_and_prints() {
    assert_eq!(CpuSpeed::default(), CpuSpeed::Auto(3000));
    assert_eq!(CpuSpeed::parse("auto"), Ok(CpuSpeed::Auto(3000)));
    assert_eq!(CpuSpeed::parse(" Auto  8000 "), Ok(CpuSpeed::Auto(8000)));
    assert!(CpuSpeed::parse("auto 0").is_err());
    assert!(CpuSpeed::parse("auto 3000 max").is_err());
    assert!(CpuSpeed::parse("3000 3000").is_err());
    for speed in [CpuSpeed::Max, CpuSpeed::Fixed(1234), CpuSpeed::Auto(3000), CpuSpeed::Auto(8000)] {
        assert_eq!(CpuSpeed::parse(&speed.to_string()), Ok(speed));
    }
    assert_eq!(CpuSpeed::Auto(3000).to_string(), "auto");
}

#[test]
fn max_speed_tracks_host_throughput() {
    let start = Instant::now();
    let bus = bus_at(20_000);
    let mut pacer = Pacer::new(CpuSpeed::Max, start);
    // Plenty of headroom: speed goes up, by at most 10% per frame, once
    // frames took long enough to time.
    let fast = |pacer: &mut Pacer| {
        pacer.end_frame(&bus, false, 333_000, Duration::from_millis(2), Duration::from_millis(1))
    };
    for _ in 0..4 {
        assert_eq!(fast(&mut pacer), None);
    }
    assert_eq!(fast(&mut pacer), Some(22_000));
    // Slower than real time: speed goes down.
    let down = pacer.end_frame(
        &bus,
        false,
        333_000,
        Duration::from_millis(30),
        Duration::from_millis(1),
    );
    assert_eq!(down, Some(18_000));
    // Fixed speed never changes.
    let mut fixed = Pacer::new(CpuSpeed::Fixed(20_000), start);
    assert_eq!(
        fixed.end_frame(&bus, true, 333_000, Duration::from_millis(2), Duration::ZERO),
        None
    );
}

#[test]
fn cpu_speed_parses_max_and_numbers() {
    assert_eq!(CpuSpeed::parse("max"), Ok(CpuSpeed::Max));
    assert_eq!(CpuSpeed::parse(" MAX "), Ok(CpuSpeed::Max));
    assert_eq!(CpuSpeed::parse("3000"), Ok(CpuSpeed::Fixed(3000)));
    assert!(CpuSpeed::parse("0").is_err());
    assert!(CpuSpeed::parse("fast").is_err());
}

#[test]
fn int_1a_reads_the_bios_tick_count() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    cpu.bus.write_16(0x046C, 0x1234);
    cpu.bus.write_16(0x046E, 0x0005);
    cpu.bus.write_8(0x0470, 1);
    cpu.set_ax(0x0000);
    rust_dos::interrupts::int1a::handle(&mut cpu);
    assert_eq!((cpu.cx(), cpu.dx(), cpu.ax() & 0xFF), (0x0005, 0x1234, 1));
    // The midnight flag is reported once.
    rust_dos::interrupts::int1a::handle(&mut cpu);
    assert_eq!(cpu.ax() & 0xFF, 0);
}

#[test]
fn sound_blaster_irq_stays_raised_until_the_driver_acks_it() {
    let mut bus = bus_at(1000);
    bus.io_write(0x21, 0x01); // mask the timer
    bus.io_write(0x22C, 0xF2); // DSP: force an 8-bit IRQ (IRQ 7)
    assert_eq!(bus.pic_pending_irq(), Some(7));
    bus.pic_acknowledge(7);
    // In service: not delivered again, although the card still raises it.
    assert_eq!(bus.pic_pending_irq(), None);
    assert!(bus.sb.as_ref().unwrap().irq_pending());

    bus.io_read(0x22E); // ISR acknowledges the card...
    bus.io_write(0x20, 0x20); // ...and the PIC
    assert_eq!(bus.pic_pending_irq(), None);
    assert_eq!(bus.pic.master.isr, 0);
}

#[test]
fn sound_blaster_irq_ended_only_at_the_pic_comes_once() {
    // DMX finds the card's IRQ with a handler that notes the IRQ and sends
    // the EOI, leaving the card unacknowledged: the PICs are edge-triggered.
    let mut bus = bus_at(1000);
    bus.io_write(0x21, 0x01); // mask the timer
    bus.io_write(0x22C, 0xF2);
    assert_eq!(bus.pic_pending_irq(), Some(7));
    bus.pic_acknowledge(7);
    bus.io_write(0x20, 0x20);
    assert_eq!(bus.pic_pending_irq(), None);
    // Another interrupt of the same kind before the acknowledge: the line
    // is up already.
    bus.io_write(0x22C, 0xF2);
    assert_eq!(bus.pic_pending_irq(), None);
    // Acknowledged, the next one comes.
    bus.io_read(0x22E);
    bus.io_write(0x22C, 0xF2);
    assert_eq!(bus.pic_pending_irq(), Some(7));
}

#[test]
fn sound_blaster_irq_acknowledged_before_it_is_taken_is_withdrawn() {
    let mut bus = bus_at(1000);
    bus.io_write(0x21, 0x81); // mask the timer and IRQ 7
    bus.io_write(0x22C, 0xF2);
    assert_eq!(bus.pic.master.irr & 0x80, 0x80);
    // The driver polls the card and acknowledges it.
    bus.io_read(0x22E);
    bus.io_write(0x21, 0x01);
    assert_eq!(bus.pic_pending_irq(), None);
}

#[test]
fn bios_wait_passes_emulated_time_with_interrupts_enabled() {
    use rust_dos::cpu::CpuFlags;
    use rust_dos::exec::{self, NoHook};
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    cpu.bus.set_cycles_per_ms(1000);
    // MOV AH,86h; XOR CX,CX; MOV DX,20000 (20 ms); INT 15h; HLT
    cpu.bus.load_bytes(0x20000, &[0xB4, 0x86, 0x31, 0xC9, 0xBA, 0x20, 0x4E, 0xCD, 0x15, 0xF4]);
    cpu.set_cs(0x2000);
    cpu.set_ip(0);
    cpu.set_ss(0x3000);
    cpu.set_sp(0x0100);
    cpu.set_cpu_flag(CpuFlags::IF, true);
    let start = cpu.bus.clock.now_micros();
    while cpu.ip() != 0x0A {
        let end = cpu.bus.clock.icount + 1000;
        cpu.bus.start_batch(end);
        exec::run_batch(&mut cpu, &mut NoHook, false);
        assert!(cpu.bus.clock.now_micros() - start < 100_000, "the wait ends");
    }
    let waited = cpu.bus.clock.now_micros() - start;
    assert!((20_000..25_000).contains(&waited), "waited {} us", waited);
    assert!(!cpu.get_cpu_flag(CpuFlags::CF));
}

#[test]
fn an_adlib_delay_reads_channel_2_down() {
    // Bloodnet's AdLib driver waits after each register write by reading
    // channel 2 in mode 4: the LSB, then MSB and LSB pairs until the LSB
    // has dropped by more than 4.
    let mut bus = bus_at(20_000);
    bus.io_write(0x43, 0xB8);
    let ppi = bus.io_read(0x61);
    bus.io_write(0x61, ppi | 1);
    bus.io_write(0x42, 0);
    bus.io_write(0x42, 0);
    let start = bus.clock.now_ticks();
    let first = bus.io_read(0x42);
    for reads in 0.. {
        assert!(reads < 1000, "the wait doesn't end");
        bus.io_read(0x42);
        let lsb = bus.io_read(0x42);
        bus.clock.icount += 4;
        if first.wrapping_sub(lsb) > 4 {
            break;
        }
    }
    assert!(bus.clock.now_ticks() - start >= 5);
}

#[test]
fn channel_2_square_wave_follows_its_gate() {
    let mut bus = bus_at(1000);
    let out = |bus: &mut Bus| bus.io_read(0x61) & 0x20 != 0;
    // A tone of 1000 ticks with the gate high: the output is high for the
    // first 500 ticks, low for the next.
    bus.io_write(0x61, 0x01);
    bus.io_write(0x43, 0xB6);
    bus.io_write(0x42, 0xE8);
    bus.io_write(0x42, 0x03);
    assert!(out(&mut bus));
    bus.clock.icount += 600; // about 716 ticks
    assert!(!out(&mut bus));
    // The gate low forces it high and stops the count; up again, the
    // period starts over.
    bus.io_write(0x61, 0x00);
    assert!(out(&mut bus));
    bus.io_write(0x43, 0x80);
    let held = bus.io_read(0x42) as u16 | (bus.io_read(0x42) as u16) << 8;
    bus.clock.icount += 300;
    bus.io_write(0x43, 0x80);
    let later = bus.io_read(0x42) as u16 | (bus.io_read(0x42) as u16) << 8;
    assert_eq!(held, later);
    bus.io_write(0x61, 0x01);
    assert!(out(&mut bus));
    bus.io_write(0x43, 0x80);
    let restarted = bus.io_read(0x42) as u16 | (bus.io_read(0x42) as u16) << 8;
    assert!(restarted > 990, "{}", restarted);
}

#[test]
fn channel_2_mode_1_waits_for_the_gate() {
    let mut bus = bus_at(1000);
    let out = |bus: &mut Bus| bus.io_read(0x61) & 0x20 != 0;
    bus.io_write(0x61, 0x00);
    // Mode 1, count 100: nothing happens until the gate rises.
    bus.io_write(0x43, 0xB2);
    bus.io_write(0x42, 100);
    bus.io_write(0x42, 0);
    bus.clock.icount += 1000;
    assert!(out(&mut bus));
    bus.io_write(0x61, 0x01);
    assert!(!out(&mut bus));
    bus.clock.icount += 200; // about 239 ticks
    assert!(out(&mut bus));
    // Another rising edge fires it again.
    bus.io_write(0x61, 0x00);
    bus.io_write(0x61, 0x01);
    assert!(!out(&mut bus));
}

/// Where the channel 2 program keeps its results, in its segment.
const PIT2_TICKS: u16 = 0x400;
const PIT2_FRESH: u16 = 0x402;
const PIT2_HELD: u16 = 0x404;
const PIT2_OUT_HELD: u16 = 0x406;

/// A COM program timing 50 ms twenty times on PIT channel 2 as Memtest86+'s
/// `cpuspeed()` and Linux's TSC calibration do: gate high, speaker off,
/// count 59659 in mode 0, then poll port 61h bit 5 until the output goes
/// high at terminal count. It keeps the BIOS ticks the twenty waits took,
/// then a count of FFFFh latched just after it was written, and one of
/// 1000h latched with the gate low (with the output then).
fn pit2_program() -> Vec<u8> {
    use iced_x86::code_asm::*;
    let mut a = CodeAssembler::new(16).unwrap();
    let asm = |a: &mut CodeAssembler| -> Result<(), IcedError> {
        a.mov(ax, 0x40)?;
        a.mov(es, ax)?;
        let mut sync = a.create_label();
        a.mov(bx, word_ptr(0x6C).es())?;
        a.set_label(&mut sync)?;
        a.cmp(bx, word_ptr(0x6C).es())?;
        a.je(sync)?;
        a.mov(bx, word_ptr(0x6C).es())?;
        a.mov(si, 20)?;
        let mut measure = a.create_label();
        let mut poll = a.create_label();
        a.set_label(&mut measure)?;
        a.in_(al, 0x61)?;
        a.and(al, 0xFD)?;
        a.or(al, 1)?;
        a.out(0x61, al)?;
        a.mov(al, 0xB0)?;
        a.out(0x43, al)?;
        a.mov(al, 0x0B)?;
        a.out(0x42, al)?;
        a.mov(al, 0xE9)?;
        a.out(0x42, al)?;
        a.set_label(&mut poll)?;
        a.in_(al, 0x61)?;
        a.test(al, 0x20)?;
        a.jz(poll)?;
        a.dec(si)?;
        a.jnz(measure)?;
        a.mov(ax, word_ptr(0x6C).es())?;
        a.sub(ax, bx)?;
        a.mov(word_ptr(PIT2_TICKS), ax)?;
        // A fresh count reads back from where it starts.
        a.mov(al, 0xB0)?;
        a.out(0x43, al)?;
        a.mov(al, 0xFF)?;
        a.out(0x42, al)?;
        a.out(0x42, al)?;
        a.mov(al, 0x80)?;
        a.out(0x43, al)?;
        a.in_(al, 0x42)?;
        a.mov(byte_ptr(PIT2_FRESH), al)?;
        a.in_(al, 0x42)?;
        a.mov(byte_ptr(PIT2_FRESH + 1), al)?;
        // With the gate low, the count holds and the output stays low.
        a.in_(al, 0x61)?;
        a.and(al, 0xFC)?;
        a.out(0x61, al)?;
        a.mov(al, 0xB0)?;
        a.out(0x43, al)?;
        a.mov(al, 0x00)?;
        a.out(0x42, al)?;
        a.mov(al, 0x10)?;
        a.out(0x42, al)?;
        a.mov(cx, 5000)?;
        let mut idle = a.create_label();
        a.set_label(&mut idle)?;
        a.loop_(idle)?;
        a.mov(al, 0x80)?;
        a.out(0x43, al)?;
        a.in_(al, 0x42)?;
        a.mov(byte_ptr(PIT2_HELD), al)?;
        a.in_(al, 0x42)?;
        a.mov(byte_ptr(PIT2_HELD + 1), al)?;
        a.in_(al, 0x61)?;
        a.mov(byte_ptr(PIT2_OUT_HELD), al)?;
        // The gate up again, it runs out.
        a.or(al, 1)?;
        a.out(0x61, al)?;
        let mut out = a.create_label();
        a.set_label(&mut out)?;
        a.in_(al, 0x61)?;
        a.test(al, 0x20)?;
        a.jz(out)?;
        a.mov(ax, 0x4C2A)?;
        a.int(0x21)
    };
    asm(&mut a).unwrap();
    let mut image = a.assemble(0x100).unwrap();
    image.resize(0x500 - 0x100, 0);
    image
}

#[test]
fn pit_channel_2_times_a_calibration_loop() {
    use rust_dos::exec::{NoHook, StopReason, run_batch};
    let dir = std::path::PathBuf::from("target/test_timer/pit2");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("T.COM"), pit2_program()).unwrap();
    let mut cpu = Cpu::new(dir);
    cpu.load_shell();
    assert!(cpu.load_executable("T.COM", None));
    cpu.bus.set_cycles_per_ms(20_000);
    let psp = cpu.current_psp as usize;
    let ended = (0..2000).any(|_| {
        cpu.bus.start_batch(cpu.bus.clock.icount + 100_000);
        run_batch(&mut cpu, &mut NoHook, false) == StopReason::ShellReloaded
    });
    assert!(ended, "the program didn't end");
    assert_eq!(cpu.errorlevel, 0x2A);
    let word = |offset: u16| cpu.bus.read_16(psp * 16 + offset as usize);
    // Twenty times 50 ms is a second: 18.2 BIOS ticks.
    assert!((17..=19).contains(&word(PIT2_TICKS)), "{} ticks", word(PIT2_TICKS));
    assert!(word(PIT2_FRESH) >= 0xFF00, "{:04X}", word(PIT2_FRESH));
    assert_eq!(word(PIT2_HELD), 0x1000);
    assert_eq!(word(PIT2_OUT_HELD) & 0x20, 0);
}

#[test]
fn a_mode_0_control_word_still_stops_channel_2() {
    // Tony & Friends in Kellogg's Land times its frames so: mode 0 with the
    // gate low, a count, a latch, then the gate up. The count it reads is
    // the one it wrote, and the control word stops a running count.
    let mut bus = bus_at(20_000);
    let read = |bus: &mut Bus| {
        bus.io_write(0x43, 0x80);
        bus.io_read(0x42) as u16 | (bus.io_read(0x42) as u16) << 8
    };
    bus.io_write(0x61, 0x00);
    bus.io_write(0x43, 0xB0);
    let stopped = read(&mut bus);
    bus.clock.icount += 1_000;
    assert_eq!(read(&mut bus), stopped);
    bus.io_write(0x42, 0xB2);
    bus.io_write(0x42, 0x41);
    bus.clock.icount += 1_000;
    assert_eq!(read(&mut bus), 0x41B2);
    bus.io_write(0x61, 0x01);
    bus.clock.icount += 1_000;
    assert!(read(&mut bus) < 0x41B2);
}

/// Bao Xiao San Guo Zhi's random numbers (SANGO.EXE, DOSBox-X issue 1274):
/// a counter latch command, a square wave control word, then the count's
/// two bytes, never writing a count. Returns how many of eight calls
/// differ.
fn sango_rolls(bus: &mut Bus) -> usize {
    let mut rolls = Vec::new();
    for call in 0..8u64 {
        bus.clock.icount += 1_000 + 37 * call;
        bus.io_write(0x43, 0x86);
        bus.io_write(0x43, 0xB6);
        let lsb = bus.io_read(0x42) as u16;
        let msb = bus.io_read(0x42) as u16;
        rolls.push(msb << 8 | lsb);
    }
    rolls.sort_unstable();
    rolls.dedup();
    rolls.len()
}

#[test]
fn channel_2_keeps_counting_after_a_square_wave_control_word() {
    // A square wave already runs with the gate high; the game's control
    // words leave it running, so every roll differs.
    let mut bus = bus_at(20_000);
    bus.io_write(0x61, 0x01);
    bus.io_write(0x43, 0xB6);
    bus.io_write(0x42, 0x00);
    bus.io_write(0x42, 0x00);
    assert_eq!(sango_rolls(&mut bus), 8);
}

/// Channel 2's count, through a counter latch command.
fn latched_pit2(bus: &mut Bus) -> u16 {
    bus.io_write(0x43, 0x80);
    bus.io_read(0x42) as u16 | (bus.io_read(0x42) as u16) << 8
}

#[test]
fn a_count_after_a_square_wave_control_word_loads_at_once() {
    // The control word leaves the rate generator running, but the count
    // written after it starts over at once, as after any control word,
    // without waiting for the running period to end.
    let mut bus = bus_at(20_000);
    bus.io_write(0x61, 0x01);
    bus.io_write(0x43, 0xB4);
    bus.io_write(0x42, 0x00);
    bus.io_write(0x42, 0x00);
    bus.clock.icount += 1_000;
    bus.io_write(0x43, 0xB4);
    bus.io_write(0x42, 0x00);
    bus.io_write(0x42, 0x10);
    let count = latched_pit2(&mut bus);
    assert!((0xF00..=0x1000).contains(&count), "{:04X}", count);
    // A count written while it runs, with no control word before it,
    // waits for the period to end.
    bus.io_write(0x42, 0x00);
    bus.io_write(0x42, 0x20);
    assert!(latched_pit2(&mut bus) <= 0x1000);
}

#[test]
fn a_due_count_survives_a_save_state() {
    use rust_dos::savestate::machine;
    let machine = || {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        cpu.bus.set_cycles_per_ms(20_000);
        cpu.load_shell();
        cpu
    };
    let mut a = machine();
    a.bus.io_write(0x61, 0x01);
    a.bus.io_write(0x43, 0xB6);
    a.bus.io_write(0x42, 0x00);
    a.bus.io_write(0x42, 0x00);
    a.bus.clock.icount += 1_000;
    a.bus.io_write(0x43, 0xB6);
    let (bytes, parts) = machine::save_parts(&a);
    let mut b = machine();
    machine::load(&mut b, &bytes).unwrap();
    assert!(b.bus.pit2.count_due);
    assert!(machine::save(&b) == bytes);

    // A PIT2 section that ends before the flag, as states saved before it
    // was kept, loads without it.
    let range = |name: &str| parts.iter().find(|(n, _)| n == name).unwrap_or_else(|| panic!("no {}", name)).1.clone();
    let flag = range("PIT2.pit2_count_due");
    let mut old = bytes[..flag.start].to_vec();
    old.extend_from_slice(&bytes[flag.end..]);
    // The section's length follows its tag and version.
    let len_at = range("PIT2").start + 6;
    let len = u32::from_le_bytes(old[len_at..len_at + 4].try_into().unwrap()) - flag.len() as u32;
    old[len_at..len_at + 4].copy_from_slice(&len.to_le_bytes());
    let mut c = machine();
    machine::load(&mut c, &old).unwrap();
    assert!(!c.bus.pit2.count_due);
}
