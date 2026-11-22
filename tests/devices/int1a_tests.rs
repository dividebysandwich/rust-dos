//! INT 1Ah, the BIOS's time-of-day services: which functions the machine's
//! BIOS has, the carry flag of those it hasn't, and the clock's alarm.

use chrono::{NaiveDate, NaiveDateTime};
use rust_dos::cpu::{Cpu, CpuFlags};
use rust_dos::exec::{self, NoHook};
use rust_dos::interrupts::int1a;
use rust_dos::video::adapter::{Adapter, VideoSetup};
use rust_dos::video::bios;

fn machine(adapter: Adapter) -> Box<Cpu> {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    cpu.bus.set_cycles_per_ms(1000);
    bios::install(
        &mut cpu.bus,
        VideoSetup {
            adapter,
            ..Default::default()
        },
    );
    cpu
}

/// Call INT 1Ah with AX and the carry flag `cf` as the caller left it.
fn call(cpu: &mut Cpu, ax: u16, cf: bool) {
    cpu.set_ax(ax);
    cpu.set_cpu_flag(CpuFlags::CF, cf);
    int1a::handle(cpu);
}

fn cmos(cpu: &mut Cpu, index: u8) -> u8 {
    cpu.bus.io_write(0x70, index);
    cpu.bus.io_read(0x71)
}

#[test]
fn a_function_the_bios_lacks_sets_the_carry_flag() {
    // The AT's BIOS returns with CF set and AH as it was.
    for ah in [0x08u16, 0x0A, 0x55, 0xFF] {
        let mut cpu = machine(Adapter::Svga);
        call(&mut cpu, ah << 8 | 0x12, false);
        assert!(cpu.get_cpu_flag(CpuFlags::CF), "AH={ah:02X}h");
        assert_eq!(cpu.ax(), ah << 8 | 0x12);
    }
}

#[test]
fn the_clock_services_clear_the_carry_flag() {
    for ax in [0x0000, 0x0100, 0x0200, 0x0400, 0x0700] {
        let mut cpu = machine(Adapter::Svga);
        call(&mut cpu, ax, true);
        assert!(!cpu.get_cpu_flag(CpuFlags::CF), "AX={ax:04X}h");
    }
}

#[test]
fn the_tandy_dac_probe_finds_no_dac_and_is_not_logged() {
    // Elvira's Tandy driver: SUB BX,BX; MOV CX,1; MOV DX,3E8h;
    // MOV AX,8300h; STC; INT 1Ah; JC nodac. Without the STC a program
    // must still see no DAC.
    let log = std::env::temp_dir().join(format!("int1a-dac-{}.log", std::process::id()));
    for adapter in [Adapter::Svga, Adapter::Tandy, Adapter::Pcjr] {
        let mut cpu = machine(adapter);
        cpu.bus.log_file = rust_dos::log::LogFile::create(&log).ok();
        for ah in 0x81u16..=0x85 {
            cpu.set_bx(0);
            cpu.set_cx(1);
            cpu.set_dx(0x03E8);
            call(&mut cpu, ah << 8, false);
            assert!(cpu.get_cpu_flag(CpuFlags::CF), "{adapter:?} AH={ah:02X}h");
        }
        cpu.bus.log_file = None;
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        assert!(!text.contains("INT 1A"), "{adapter:?}: {text}");
    }
    let _ = std::fs::remove_file(&log);
}

#[test]
fn the_sound_multiplexer_is_the_pcjrs_and_tandys_alone() {
    // On the PCjr and the Tandy 1000 AH=80h selects the speaker's source
    // and returns with the caller's flags; an AT's BIOS lacks it.
    for (adapter, has_it) in [
        (Adapter::Pcjr, true),
        (Adapter::Tandy, true),
        (Adapter::Svga, false),
    ] {
        for cf in [false, true] {
            let mut cpu = machine(adapter);
            call(&mut cpu, 0x8003, cf);
            let expected = if has_it { cf } else { true };
            assert_eq!(
                cpu.get_cpu_flag(CpuFlags::CF),
                expected,
                "{adapter:?}, CF {cf}"
            );
        }
    }
}

#[test]
fn the_alarm_is_set_once_until_it_is_reset() {
    let mut cpu = machine(Adapter::Svga);
    cpu.bus.io_write(0xA1, 0xFF);
    cpu.set_cx(0x2359);
    cpu.set_dx(0x5800);
    call(&mut cpu, 0x0600, true);
    assert!(!cpu.get_cpu_flag(CpuFlags::CF));
    assert_eq!(
        (
            cmos(&mut cpu, 0x05),
            cmos(&mut cpu, 0x03),
            cmos(&mut cpu, 0x01)
        ),
        (0x23, 0x59, 0x58)
    );
    assert_eq!(cmos(&mut cpu, 0x0B) & 0x20, 0x20, "alarm interrupt enabled");
    assert_eq!(cpu.bus.io_read(0xA1) & 0x01, 0, "IRQ 8 unmasked");
    // Set already: CF set, the alarm as it was.
    cpu.set_cx(0x0101);
    call(&mut cpu, 0x0600, false);
    assert!(cpu.get_cpu_flag(CpuFlags::CF));
    assert_eq!(cmos(&mut cpu, 0x05), 0x23);
    // Reset, it can be set again.
    call(&mut cpu, 0x0700, true);
    assert!(!cpu.get_cpu_flag(CpuFlags::CF));
    assert_eq!(cmos(&mut cpu, 0x0B) & 0x20, 0);
    call(&mut cpu, 0x0600, true);
    assert!(!cpu.get_cpu_flag(CpuFlags::CF));
    assert_eq!(cmos(&mut cpu, 0x05), 0x01);
}

fn noon(seconds: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 1, 1)
        .unwrap()
        .and_hms_opt(12, 0, seconds)
        .unwrap()
}

/// Run a second of emulated time with no instructions, as the main loop
/// services the bus's events, and say whether IRQ 8 was raised.
fn run_a_second(cpu: &mut Cpu) -> bool {
    let end = cpu.bus.clock.icount + 1_000_000;
    cpu.bus.start_batch(end);
    let mut raised = false;
    while cpu.bus.clock.icount < end {
        cpu.bus.clock.icount = cpu.bus.clock.deadline.min(end);
        cpu.bus.service_timers();
        raised |= cpu.bus.pic.slave.irr & 0x01 != 0;
        cpu.bus.pic.slave.irr &= !0x01;
    }
    raised
}

#[test]
fn the_alarm_raises_irq_8_when_the_clock_gets_there() {
    rust_dos::hosttime::fix(Some(noon(0)));
    let mut cpu = machine(Adapter::Svga);
    cpu.set_cx(0x1200);
    cpu.set_dx(0x0300);
    call(&mut cpu, 0x0600, true);
    assert!(!run_a_second(&mut cpu), "12:00:00");
    rust_dos::hosttime::fix(Some(noon(2)));
    assert!(!run_a_second(&mut cpu), "12:00:02");
    // The look comes late, past the alarm's second: it still finds it.
    rust_dos::hosttime::fix(Some(noon(5)));
    assert!(run_a_second(&mut cpu), "12:00:05");
    // Status C: the interrupt and the alarm, cleared by the read.
    assert_eq!(cmos(&mut cpu, 0x0C), 0xA0);
    assert_eq!(cmos(&mut cpu, 0x0C), 0x00);
    rust_dos::hosttime::fix(Some(noon(7)));
    assert!(!run_a_second(&mut cpu), "once a day");
    rust_dos::hosttime::fix(None);
}

#[test]
fn the_bios_clock_interrupt_calls_int_4ah_for_the_alarm() {
    rust_dos::hosttime::fix(Some(noon(0)));
    let mut cpu = machine(Adapter::Svga);
    cpu.set_cx(0x1200);
    cpu.set_dx(0x0100);
    call(&mut cpu, 0x0600, true);
    rust_dos::hosttime::fix(Some(noon(1)));
    assert!(run_a_second(&mut cpu));
    // INT 4Ah: MOV BYTE [0500h],1; IRET. The program: INT 70h; HLT.
    cpu.bus
        .load_bytes(0x21000, &[0xC6, 0x06, 0x00, 0x05, 0x01, 0xCF]);
    cpu.bus.write_16(0x4A * 4, 0x0000);
    cpu.bus.write_16(0x4A * 4 + 2, 0x2100);
    cpu.bus.load_bytes(0x20000, &[0xCD, 0x70, 0xF4]);
    cpu.set_cs(0x2000);
    cpu.set_ip(0);
    cpu.set_ds(0);
    cpu.set_ss(0x3000);
    cpu.set_sp(0x0100);
    // Past the HLT.
    for _ in 0..100 {
        if cpu.cs() == 0x2000 && cpu.ip() == 0x03 {
            break;
        }
        let end = cpu.bus.clock.icount + 100;
        cpu.bus.start_batch(end);
        exec::run_batch(&mut cpu, &mut NoHook, false);
    }
    assert_eq!((cpu.cs(), cpu.ip()), (0x2000, 0x03));
    assert_eq!(cpu.bus.read_8(0x0500), 1, "INT 4Ah called");
    assert_eq!(cmos(&mut cpu, 0x0C), 0, "status C read by the handler");
    rust_dos::hosttime::fix(None);
}
