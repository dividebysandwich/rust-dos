//! Reading the emulated devices for DirectInput, where USER32 reads them
//! for its messages, so that both see every key and every move.
//!
//! USER32's `input::poll`, which runs when time passes, finds the keys
//! that went down and up on the 8042 keyboard and hands each to `key`.
//! Then `poll` reads the mouse (all its motion, which MouseState counts
//! whatever its cursor's range, and how often each button went down and
//! up) and the game port's joysticks. The changes become each acquired
//! device's state and buffered data, and set its event.
//!
//! A device acquired in the foreground loses acquisition when its window
//! is no longer the foreground window. While a device has the mouse
//! exclusively, USER32 gets no mouse messages and its cursor stays where
//! it is (`mouse_taken`), as on Windows; the keyboard's messages go on
//! whatever DirectInput has.

use super::device::{DISCL_EXCLUSIVE, DISCL_FOREGROUND};
use super::objects::{self, JOYSTICK, KEYBOARD, MOUSE};
use super::state;
use crate::cpu::Cpu;
use crate::win32::object::{self, Kind};

/// The mouse as DirectInput last read it: all its motion, and how often
/// each button went down and up.
#[derive(Clone, Debug, Default)]
pub struct MouseSeen {
    pub started: bool,
    pub motion: (i32, i32),
    pub presses: [u16; 3],
    pub releases: [u16; 3],
    pub buttons: u8,
}

fn now_ms(cpu: &Cpu) -> u32 {
    (cpu.bus.clock.now_micros() / 1000) as u32
}

/// Set the events of the devices whose state changed, by their objects.
fn signal(cpu: &mut Cpu, events: &[u32]) {
    for &id in events {
        if id == 0 {
            continue;
        }
        if let Some(o) = object::objects(cpu).get_mut(id)
            && let Kind::Event { signaled, .. } = &mut o.kind
        {
            *signaled = true;
        }
    }
}

/// The key with scan code `scan` (E0 first when `extended`) went down or
/// up on the keyboard: the acquired keyboards' data.
pub fn key(cpu: &mut Cpu, scan: u8, extended: bool, down: bool) {
    let dik = scan & 0x7F | (extended as u8) << 7;
    let Some(k) = objects::key_index(dik) else {
        return;
    };
    let time = now_ms(cpu);
    let s = state(cpu);
    if !s
        .devices_mut()
        .any(|(_, d)| d.kind == KEYBOARD && d.acquired)
    {
        return;
    }
    let sequence = s.next_sequence();
    let data = if down { 0x80 } else { 0 };
    let mut events = Vec::new();
    for (_, d) in s.devices_mut() {
        if d.kind == KEYBOARD && d.acquired {
            d.report(k, data, time, sequence);
            events.push(d.event);
        }
    }
    signal(cpu, &events);
}

/// Time passed: the foreground's devices that lost it are no longer
/// acquired, and the mouse and the joysticks are read.
pub fn poll(cpu: &mut Cpu) {
    if state(cpu).objects.is_empty() && state(cpu).mouse.started {
        // Without devices, only what the mouse did is noted, below.
        read_mouse(cpu);
        return;
    }
    check_foreground(cpu);
    read_mouse(cpu);
    read_joysticks(cpu);
}

/// Read the devices now, for a device's method that returns their state:
/// as when time passes, USER32's reading and DirectInput's.
pub fn refresh(cpu: &mut Cpu) {
    crate::win32::user32::input::poll(cpu);
    poll(cpu);
}

/// Whether `hwnd`'s top-level window is the foreground window.
pub fn is_foreground(cpu: &Cpu, hwnd: u32) -> bool {
    let user = &cpu.bus.win32.user;
    user.is_window(hwnd) && user.top_level(hwnd) == user.input.foreground
}

/// Devices acquired in the foreground whose window isn't the foreground
/// window lose acquisition, and their events are set.
pub fn check_foreground(cpu: &mut Cpu) {
    let foreground: Vec<(u32, u32)> = state(cpu)
        .devices_mut()
        .filter(|(_, d)| d.acquired && d.coop & DISCL_FOREGROUND != 0)
        .map(|(id, d)| (id, d.hwnd))
        .collect();
    let mut events = Vec::new();
    for (id, hwnd) in foreground {
        if is_foreground(cpu, hwnd) {
            continue;
        }
        let d = super::device::dev(cpu, id);
        d.acquired = false;
        d.lost = d.coop & DISCL_EXCLUSIVE != 0;
        events.push(d.event);
    }
    signal(cpu, &events);
}

/// Whether a device has the mouse exclusively: USER32 then leaves it
/// alone.
pub fn mouse_taken(cpu: &mut Cpu) -> bool {
    state(cpu)
        .devices_mut()
        .any(|(_, d)| d.kind == MOUSE && d.acquired && d.coop & DISCL_EXCLUSIVE != 0)
}

fn read_mouse(cpu: &mut Cpu) {
    let m = &cpu.bus.mouse;
    let (motion, presses, releases, buttons) =
        (m.motion, m.press_count, m.release_count, m.buttons);
    let time = now_ms(cpu);
    let s = state(cpu);
    if !s.mouse.started {
        s.mouse = MouseSeen {
            started: true,
            motion,
            presses,
            releases,
            buttons,
        };
        return;
    }
    let seen = &mut s.mouse;
    let moved = [
        motion.0.wrapping_sub(seen.motion.0),
        motion.1.wrapping_sub(seen.motion.1),
    ];
    seen.motion = motion;
    // Each button's presses and releases in turn, as USER32 takes them.
    let mut changes = Vec::new();
    for b in 0..3 {
        let mut downs = presses[b].wrapping_sub(seen.presses[b]);
        let mut ups = releases[b].wrapping_sub(seen.releases[b]);
        seen.presses[b] = presses[b];
        seen.releases[b] = releases[b];
        let mut down = seen.buttons & 1 << b != 0;
        while downs > 0 || ups > 0 {
            if !down && downs > 0 {
                downs -= 1;
                down = true;
            } else if down && ups > 0 {
                ups -= 1;
                down = false;
            } else {
                break;
            }
            changes.push((b, down));
        }
        if down {
            seen.buttons |= 1 << b;
        } else {
            seen.buttons &= !(1 << b);
        }
    }
    if moved == [0, 0] && changes.is_empty() {
        return;
    }
    if !s.devices_mut().any(|(_, d)| d.kind == MOUSE && d.acquired) {
        return;
    }
    let mut events = Vec::new();
    if moved != [0, 0] {
        let sequence = s.next_sequence();
        for (_, d) in s.devices_mut() {
            if d.kind == MOUSE && d.acquired {
                d.mouse_moved(moved, time, sequence);
                events.push(d.event);
            }
        }
    }
    for (b, down) in changes {
        let sequence = s.next_sequence();
        for (_, d) in s.devices_mut() {
            if d.kind == MOUSE && d.acquired {
                // The buttons come after the two axes.
                d.report(2 + b, if down { 0x80 } else { 0 }, time, sequence);
                events.push(d.event);
            }
        }
    }
    signal(cpu, &events);
}

/// The game port's counts for the axes of the joystick `index` (0 for
/// one that isn't there), and its buttons held, bit n for button n.
pub fn joystick_raw(cpu: &Cpu, index: u32) -> (Vec<u32>, u32) {
    let port = &cpu.bus.joystick;
    let counts = port.bios_axes(&cpu.bus.mouse).map(u32::from);
    let held = !(port.bios_switches(&cpu.bus.mouse) as u32 >> 4) & 0xF;
    let sticks = port.joysticks();
    let (axes, buttons) = sticks.get(index as usize).copied().unwrap_or((2, 2));
    if sticks.len() == 2 && index == 1 {
        // The second of two takes the port's second half.
        return (counts[2..4].to_vec(), held >> 2 & 3);
    }
    let axes = (axes as usize).min(4);
    (counts[..axes].to_vec(), held & ((1 << buttons) - 1))
}

/// The acquired joysticks whose axes or buttons changed: their buffered
/// data and events.
fn read_joysticks(cpu: &mut Cpu) {
    let joysticks: Vec<(u32, u32)> = state(cpu)
        .devices_mut()
        .filter(|(_, d)| d.kind >= JOYSTICK && d.acquired && (d.buffer_size > 0 || d.event != 0))
        .map(|(id, d)| (id, d.kind - JOYSTICK))
        .collect();
    if joysticks.is_empty() {
        return;
    }
    let time = now_ms(cpu);
    let mut events = Vec::new();
    for (id, index) in joysticks {
        let (counts, buttons) = joystick_raw(cpu, index);
        let d = super::device::dev(cpu, id);
        let values = d.joystick_values(&counts);
        let changed: Vec<(usize, i32)> = values
            .iter()
            .enumerate()
            .filter(|&(k, &v)| d.axes[k].last != v)
            .map(|(k, &v)| (k, v))
            .collect();
        let pressed = buttons ^ d.buttons;
        if changed.is_empty() && pressed == 0 {
            continue;
        }
        let sequence = state(cpu).next_sequence();
        let d = super::device::dev(cpu, id);
        for (k, v) in changed {
            let last = std::mem::replace(&mut d.axes[k].last, v);
            let data = if d.relative { v.wrapping_sub(last) } else { v };
            d.report(k, data as u32, time, sequence);
        }
        let axes = d.axes.len();
        for b in 0..d.shape.1 {
            if pressed >> b & 1 != 0 {
                let data = if buttons >> b & 1 != 0 { 0x80 } else { 0 };
                d.report(axes + b as usize, data, time, sequence);
            }
        }
        d.buttons = buttons;
        events.push(d.event);
    }
    signal(cpu, &events);
}

crate::state_fields!(MouseSeen {
    started,
    presses,
    releases,
    buttons
} skip { motion });
