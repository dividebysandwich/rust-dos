//! The game port at 201h: the mouse as joystick A without a controller,
//! game controllers as one or two joysticks, the deadzone, and the BIOS's
//! INT 15h AH=84h.

use iced_x86::Register;
use rust_dos::bus::Bus;
use rust_dos::cpu::{Cpu, CpuFlags};
use rust_dos::interrupts::int15;
use rust_dos::joystick::{JoystickSettings, JoystickType, PAD_A, PAD_B, PAD_RIGHT, PAD_UP, PAD_X, PAD_Y, PadState};
use std::path::PathBuf;

/// Fire the one-shots, then count the reads until each axis bit goes low,
/// as games do: A x, A y, B x, B y, with 0 for an axis that is low from the
/// start. Also the buttons of the first read.
fn poll(bus: &mut Bus) -> ([u32; 4], u8) {
    bus.io_write(0x201, 0);
    let mut counts = [0u32; 4];
    let mut first = None;
    for reads in 1..=1000 {
        let value = bus.io_read(0x201);
        first.get_or_insert(value);
        for (bit, count) in counts.iter_mut().enumerate() {
            if value & (1 << bit) != 0 {
                *count = reads;
            }
        }
    }
    (counts, first.unwrap() >> 4)
}

/// A game port with `kind` plugged in, without a deadzone, so the sticks
/// read where they are.
fn bus_with(kind: JoystickType) -> Bus {
    let mut bus = Bus::new(PathBuf::from("."));
    bus.set_joystick(JoystickSettings { kind, deadzone: 0 });
    bus
}

fn pad(axes: [f32; 4], buttons: u16) -> Option<PadState> {
    Some(PadState { axes, buttons })
}

#[test]
fn the_mouse_drives_stick_a_without_a_gamepad() {
    let mut bus = bus_with(JoystickType::Auto);
    bus.mouse.set_position(0, 0);
    // At the top left, each axis is high for the first 10 reads; stick B
    // isn't there.
    assert_eq!(poll(&mut bus).0, [10, 10, 0, 0]);
    bus.mouse.set_position(639, 199);
    assert_eq!(poll(&mut bus).0, [450, 450, 0, 0]);
    bus.mouse.set_position(320, 100);
    assert_eq!(poll(&mut bus).0, [10 + 440 * 320 / 639, 10 + 440 * 100 / 199, 0, 0]);
    // The buttons are low while pressed: left is button 1, right button 2.
    assert_eq!(poll(&mut bus).1, 0xF);
    bus.mouse.button_down(0);
    assert_eq!(poll(&mut bus).1, 0xE);
    bus.mouse.button_down(1);
    assert_eq!(poll(&mut bus).1, 0xC);
}

#[test]
fn one_pad_is_four_axes_and_four_buttons() {
    let mut bus = bus_with(JoystickType::Auto);
    bus.joystick.set_pad(0, pad([-1.0, 0.0, 1.0, 0.5], PAD_A | PAD_Y));
    let (counts, buttons) = poll(&mut bus);
    assert_eq!(counts, [10, 230, 450, 10 + 330]);
    // A is button 1 (bit 4), Y button 4 (bit 7).
    assert_eq!(buttons, 0b0110);
    // The D-pad moves the left stick while it rests.
    bus.joystick.set_pad(0, pad([0.0; 4], PAD_UP | PAD_RIGHT | PAD_X));
    let (counts, buttons) = poll(&mut bus);
    assert_eq!(counts, [450, 10, 230, 230]);
    assert_eq!(buttons, 0b1011);
}

#[test]
fn two_pads_are_two_joysticks() {
    let mut bus = bus_with(JoystickType::Auto);
    bus.joystick.set_pad(0, pad([1.0, 1.0, -1.0, -1.0], PAD_B));
    bus.joystick.set_pad(1, pad([-1.0, -1.0, 1.0, 1.0], PAD_A | PAD_B));
    let (counts, buttons) = poll(&mut bus);
    // Each pad's left stick; the right sticks are left out.
    assert_eq!(counts, [450, 450, 10, 10]);
    // Pad 1's B is button 2; pad 2's A and B are buttons 3 and 4.
    assert_eq!(buttons, 0b0001);

    // 2axis with one pad: stick B is there, and centred.
    let mut bus = bus_with(JoystickType::TwoAxis);
    bus.joystick.set_pad(0, pad([1.0, 1.0, 0.0, 0.0], 0));
    assert_eq!(poll(&mut bus).0, [450, 450, 230, 230]);
}

#[test]
fn the_deadzone_centres_small_deflections() {
    let mut bus = bus_with(JoystickType::FourAxis);
    bus.set_joystick(JoystickSettings { kind: JoystickType::FourAxis, deadzone: 10 });
    bus.joystick.set_pad(0, pad([0.08, -0.05, 0.3, 0.0], 0));
    let (counts, _) = poll(&mut bus);
    assert_eq!(counts[..2], [230, 230]);
    assert!(counts[2] > 230 && counts[2] < 300, "{:?}", counts);
    bus.set_joystick(JoystickSettings { kind: JoystickType::FourAxis, deadzone: 40 });
    assert_eq!(poll(&mut bus).0, [230, 230, 230, 230]);
    // Without a controller, 4axis still has its sticks, centred.
    bus.joystick.set_pad(0, None);
    assert_eq!(poll(&mut bus).0, [230, 230, 230, 230]);
}

#[test]
fn no_game_port_reads_ff_and_clears_equipment_bit_12() {
    let mut bus = bus_with(JoystickType::Auto);
    assert_eq!(bus.read_16(0x0410) & 0x1000, 0x1000);
    bus.set_joystick(JoystickSettings { kind: JoystickType::None, deadzone: 10 });
    assert_eq!(bus.read_16(0x0410) & 0x1000, 0);
    bus.io_write(0x201, 0);
    assert_eq!(bus.io_read(0x201), 0xFF);
    assert_eq!(bus.io_read(0x201), 0xFF);
}

#[test]
fn int15_84_reads_switches_and_axes() {
    let mut cpu = Cpu::new(PathBuf::from("."));
    cpu.bus.set_joystick(JoystickSettings { kind: JoystickType::Auto, deadzone: 0 });
    cpu.bus.joystick.set_pad(0, pad([1.0, -1.0, 0.0, 0.0], PAD_B));
    cpu.set_reg8(Register::AH, 0x84);
    cpu.set_dx(0);
    int15::handle(&mut cpu);
    assert!(!cpu.get_cpu_flag(CpuFlags::CF));
    assert_eq!(cpu.get_al() & 0xF0, 0xD0, "button 2 pressed");
    cpu.set_reg8(Register::AH, 0x84);
    cpu.set_dx(1);
    int15::handle(&mut cpu);
    assert_eq!((cpu.ax(), cpu.bx(), cpu.cx(), cpu.dx()), (450, 10, 230, 230));

    cpu.bus.set_joystick(JoystickSettings { kind: JoystickType::None, deadzone: 10 });
    cpu.set_reg8(Register::AH, 0x84);
    cpu.set_dx(1);
    int15::handle(&mut cpu);
    assert!(cpu.get_cpu_flag(CpuFlags::CF));
}
