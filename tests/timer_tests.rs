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
fn max_speed_tracks_host_throughput() {
    let start = Instant::now();
    let clock = Clock::new(20_000);
    let mut pacer = Pacer::new(CpuSpeed::Max, start);
    // Plenty of headroom: speed goes up, by at most 10% per frame.
    let up = pacer.end_frame(
        &clock,
        333_000,
        Duration::from_millis(2),
        Duration::from_millis(1),
    );
    assert_eq!(up, Some(22_000));
    // Slower than real time: speed goes down.
    let down = pacer.end_frame(
        &clock,
        333_000,
        Duration::from_millis(30),
        Duration::from_millis(1),
    );
    assert_eq!(down, Some(18_000));
    // Fixed speed never changes.
    let mut fixed = Pacer::new(CpuSpeed::Fixed(20_000), start);
    assert_eq!(
        fixed.end_frame(&clock, 333_000, Duration::from_millis(2), Duration::ZERO),
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
