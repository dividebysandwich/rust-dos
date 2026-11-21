//! The first Sound Blaster Pro's FM part (CT1330): two OPL2s, the left one
//! at base+0/1 and the right one at base+2/3, written together through
//! base+8/9 and the AdLib ports 388h/389h, as DOSBox's `oplmode=dualopl2`
//! has them.

use rust_dos::bus::Bus;
use rust_dos::cpu::Cpu;
use rust_dos::opl::Chip;
use rust_dos::savestate::machine;
use rust_dos::sb::{SbConfig, SbModel};
use std::path::{Path, PathBuf};

fn sb_pro1() -> SbConfig {
    SbConfig {
        model: SbModel::SbPro1,
        ..SbConfig::default()
    }
}

/// A bus at 1000 instructions per emulated ms with an SB Pro 1 at 220h and
/// the FM chips `chip`.
fn bus_with(chip: Chip) -> Bus {
    let mut bus = Bus::new(PathBuf::from("."));
    bus.set_cycles_per_ms(1000);
    bus.configure_sound(Some(sb_pro1()), chip);
    bus
}

/// Write FM register `reg` through the address port `port` and the data
/// port after it.
fn fm(bus: &mut Bus, port: u16, reg: u8, value: u8) {
    bus.io_write(port, reg);
    bus.io_write(port + 1, value);
}

/// Let `ms` milliseconds of emulated time pass, running due timer events.
fn wait_ms(bus: &mut Bus, ms: u64) {
    let target = bus.clock.icount + ms * 1000;
    while bus.clock.icount < target {
        let step = (target - bus.clock.icount).min(10);
        bus.clock.icount += step;
        if bus.clock.icount >= bus.clock.deadline {
            bus.service_timers();
        }
    }
}

/// Start timer 1 at its shortest period (80 us) through `port`, and let
/// it run out.
fn run_timer_1(bus: &mut Bus, port: u16) {
    fm(bus, port, 0x02, 0xFF);
    fm(bus, port, 0x04, 0x21);
    bus.clock.icount += 100;
}

/// The status of the left chip (base+0) and the right chip (base+2), and
/// what base+8 and 388h read.
fn statuses(bus: &mut Bus) -> [u8; 4] {
    [
        bus.io_read(0x220),
        bus.io_read(0x222),
        bus.io_read(0x228),
        bus.io_read(0x388),
    ]
}

#[test]
fn each_chip_has_its_ports_and_its_status() {
    // Timer 1 of the left chip: base+2 reads the right chip, whose timer
    // never ran. base+8 and 388h read the left chip, as in DOSBox.
    let mut bus = bus_with(Chip::DualOpl2);
    run_timer_1(&mut bus, 0x220);
    assert_eq!(statuses(&mut bus), [0xC6, 0x06, 0xC6, 0xC6]);

    let mut bus = bus_with(Chip::DualOpl2);
    run_timer_1(&mut bus, 0x222);
    assert_eq!(statuses(&mut bus), [0x06, 0xC6, 0x06, 0x06]);

    // base+8/9 and 388h/389h write both chips.
    for port in [0x228, 0x388] {
        let mut bus = bus_with(Chip::DualOpl2);
        run_timer_1(&mut bus, port);
        assert_eq!(
            statuses(&mut bus),
            [0xC6, 0xC6, 0xC6, 0xC6],
            "through {:X}h",
            port
        );
        // Resetting the flags through the left chip's ports leaves the
        // right chip's up.
        fm(&mut bus, 0x220, 0x04, 0x80);
        assert_eq!(bus.io_read(0x220) & 0xE0, 0x00);
        assert_eq!(bus.io_read(0x222) & 0xE0, 0xC0);
    }

    // An OPL3 keeps one status at every port.
    let mut bus = bus_with(Chip::Opl3);
    run_timer_1(&mut bus, 0x220);
    assert_eq!(statuses(&mut bus), [0xC0, 0xC0, 0xC0, 0xC0]);
}

/// The OPL3 check most programs make (the AdLib Programming Guide's
/// timer test, then the status's low bits) finds none on an SB Pro 1,
/// which is the card `sbtype=sbpro1` gives unless `opl` says otherwise.
#[test]
fn an_opl3_check_fails_on_an_sb_pro_1() {
    let chip = |text: &str| {
        rust_dos::config::parse(text, Path::new("/"), None)
            .sound
            .fm_chip()
    };
    assert_eq!(chip("[sound]\nsbtype=sbpro1\n"), Chip::DualOpl2);
    assert_eq!(chip("[sound]\nsbtype=sbpro1\nopl=auto\n"), Chip::DualOpl2);
    assert_eq!(chip("[sound]\nsbtype=sbpro1\nopl=opl3\n"), Chip::Opl3);
    assert_eq!(chip("[sound]\nsbtype=sbpro2\n"), Chip::Opl3);
    assert_eq!(
        chip("[sound]\nsbtype=sbpro2\nopl=dualopl2\n"),
        Chip::DualOpl2
    );

    for (chip, opl3) in [(Chip::DualOpl2, false), (Chip::Opl3, true)] {
        let mut bus = bus_with(chip);
        fm(&mut bus, 0x388, 0x04, 0x60);
        fm(&mut bus, 0x388, 0x04, 0x80);
        let before = bus.io_read(0x388);
        run_timer_1(&mut bus, 0x388);
        let after = bus.io_read(0x388);
        fm(&mut bus, 0x388, 0x04, 0x60);
        fm(&mut bus, 0x388, 0x04, 0x80);
        assert_eq!(
            (before & 0xE0, after & 0xE0),
            (0x00, 0xC0),
            "an AdLib is found"
        );
        assert_eq!(after & 0x06 == 0, opl3, "{:?}: status {:02X}h", chip, after);
    }
}

/// A plain sine on channel 0 through `port`, keyed on.
fn key_sine(bus: &mut Bus, port: u16) {
    for (reg, value) in [
        (0x20, 0x01),
        (0x40, 0x3F),
        (0x60, 0xF0),
        (0x80, 0x77),
        (0x23, 0x01),
        (0x43, 0x00),
        (0x63, 0xF0),
        (0x83, 0x77),
        (0xC0, 0x31),
        (0xA0, 0x98),
        (0xB0, 0x31),
    ] {
        fm(bus, port, reg, value);
    }
}

/// The loudest sample of the left and the right channel of the sound
/// mixed in the next 20 ms.
fn peaks(bus: &mut Bus) -> (u16, u16) {
    bus.audio_out.clear();
    wait_ms(bus, 20);
    bus.audio_catch_up();
    let peak = |side: usize| {
        bus.audio_out
            .iter()
            .skip(side)
            .step_by(2)
            .map(|s| s.unsigned_abs())
            .max()
            .unwrap_or(0)
    };
    (peak(0), peak(1))
}

#[test]
fn the_left_chip_plays_left_and_the_right_chip_right() {
    let mut bus = bus_with(Chip::DualOpl2);
    key_sine(&mut bus, 0x220);
    let (l, r) = peaks(&mut bus);
    assert!(l > 1000 && r == 0, "base+0/1: left {} right {}", l, r);

    let mut bus = bus_with(Chip::DualOpl2);
    key_sine(&mut bus, 0x222);
    let (l, r) = peaks(&mut bus);
    assert!(l == 0 && r > 1000, "base+2/3: left {} right {}", l, r);

    // An AdLib program's note plays on both, as loud on each side as one
    // OPL2 plays it.
    let mut bus = bus_with(Chip::DualOpl2);
    key_sine(&mut bus, 0x388);
    let (l, r) = peaks(&mut bus);
    let mut one = bus_with(Chip::Opl2);
    key_sine(&mut one, 0x388);
    let (ol, or) = peaks(&mut one);
    assert!(l > 1000 && r > 1000, "388h: left {} right {}", l, r);
    assert_eq!((l, r), (ol, or), "as one OPL2");

    // The SB Pro mixer's FM volume (26h) sets each side: the left at its
    // lowest, the right at its highest.
    let mut bus = bus_with(Chip::DualOpl2);
    bus.io_write(0x224, 0x26);
    bus.io_write(0x225, 0x1F);
    key_sine(&mut bus, 0x228);
    let (l, r) = peaks(&mut bus);
    assert!(
        r > 1000 && l < r / 8,
        "mixer 26h=1Fh: left {} right {}",
        l,
        r
    );
}

#[test]
fn states_keep_both_chips() {
    let machine_with = |chip: Chip| {
        let mut cpu = Cpu::new(PathBuf::from("."));
        cpu.bus.set_cycles_per_ms(1000);
        cpu.bus.configure_sound(Some(sb_pro1()), chip);
        cpu.load_shell();
        cpu
    };
    let mut a = machine_with(Chip::DualOpl2);
    key_sine(&mut a.bus, 0x222);
    run_timer_1(&mut a.bus, 0x222);
    let state = machine::save(&a);

    let mut b = machine_with(Chip::DualOpl2);
    machine::load(&mut b, &state).unwrap();
    assert!(
        machine::save(&b) == state,
        "a loaded state saves as the same bytes"
    );
    assert_eq!(b.bus.io_read(0x222), 0xC6, "the right chip's timer flag");
    assert_eq!(b.bus.io_read(0x220), 0x06);
    let (l, r) = peaks(&mut b.bus);
    assert!(
        l == 0 && r > 1000,
        "the right chip's note plays on: left {} right {}",
        l,
        r
    );

    // A machine with one chip doesn't take it.
    let mut c = machine_with(Chip::Opl3);
    assert!(machine::load(&mut c, &state).is_err());
}
