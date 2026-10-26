//! The joystick functions on the emulated game port (Bus::joystick).
//!
//! Joysticks 1 and 2 are the ones the port has (`GamePort::joysticks`),
//! as Windows 95's standard driver has them configured: a four-axis
//! joystick takes stick B's axes as its Z and R and the port's four
//! buttons, two two-axis ones take the port in halves, and the mouse is
//! a two-axis one. An axis reads 0 to 65535 over the travel the port's
//! one-shots time.
//!
//! joySetCapture has the joystick's messages (MM_JOY1MOVE and the
//! others) posted to a window every so many milliseconds while it holds
//! the joystick, by the process's multimedia thread (`poll`).

use super::callback;
use super::*;
use crate::cpu::Cpu;
use crate::joystick::{TRIP_MIN, TRIP_RANGE};

/// The errors of the joystick functions.
const JOYERR_NOERROR: u32 = 0;
const JOYERR_PARMS: u32 = 165;
const JOYERR_NOCANDO: u32 = 166;
const JOYERR_UNPLUGGED: u32 = 167;

/// The window messages of a captured joystick.
const MM_JOY1MOVE: u32 = 0x3A0;
const MM_JOY1ZMOVE: u32 = 0x3A2;
const MM_JOY1BUTTONDOWN: u32 = 0x3B5;
const MM_JOY1BUTTONUP: u32 = 0x3B7;

/// JOYCAPS's flags, and its sizes.
const JOYCAPS_HASZ: u32 = 0x0001;
const JOYCAPS_HASR: u32 = 0x0002;
const JOYCAPS_SIZE: u32 = 404;

/// JOYINFOEX's size, and its flags.
const JOYINFOEX_SIZE: u32 = 52;
const JOY_POVCENTERED: u32 = 0xFFFF;
const JOY_RETURNX: u32 = 0x0001;
const JOY_RETURNY: u32 = 0x0002;
const JOY_RETURNZ: u32 = 0x0004;
const JOY_RETURNR: u32 = 0x0008;
const JOY_RETURNBUTTONS: u32 = 0x0080;

/// The shortest and longest capture period, in milliseconds.
const PERIOD_MIN: u32 = 10;
const PERIOD_MAX: u32 = 1000;

/// The joysticks there are.
const JOYSTICKS: u32 = 2;

/// A joystick's place: axes (X, Y, Z, R) and buttons.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Position {
    pub axes: [u32; 4],
    pub buttons: u32,
}

/// A capture of joySetCapture.
#[derive(Clone, Debug, Default)]
pub struct Capture {
    pub joystick: u32,
    pub hwnd: u32,
    pub pid: u32,
    /// The period, in microseconds, and when it is next due.
    pub period: u64,
    pub next: u64,
    /// Whether moves are posted only when the joystick moved more than
    /// its threshold.
    pub changed: bool,
    pub last: Position,
}

/// The joysticks' state.
#[derive(Clone, Debug, Default)]
pub struct Joy {
    pub captures: Vec<Capture>,
    pub thresholds: [u32; JOYSTICKS as usize],
}

fn joy(cpu: &mut Cpu) -> &mut Joy {
    &mut cpu.bus.win32.winmm.joy
}

/// An axis's count of the game port as 0 to 65535.
fn axis(count: u32) -> u32 {
    let c = count.clamp(TRIP_MIN, TRIP_MIN + TRIP_RANGE) - TRIP_MIN;
    c * 65535 / TRIP_RANGE
}

/// Where the joystick `id` is, or the error.
fn position(cpu: &mut Cpu, id: u32) -> Result<Position, u32> {
    if id >= JOYSTICKS {
        return Err(JOYERR_PARMS);
    }
    let (counts, buttons) = cpu
        .bus
        .joystick
        .joystick(&cpu.bus.mouse, id as usize)
        .ok_or(JOYERR_UNPLUGGED)?;
    // An axis the joystick doesn't have reads 0.
    let mut axes = [0; 4];
    for (a, &c) in axes.iter_mut().zip(&counts) {
        *a = axis(c);
    }
    Ok(Position { axes, buttons })
}

/// joyGetNumDevs: the joysticks the driver takes, none without a game
/// port.
pub fn joy_get_num_devs(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(if cpu.bus.joystick.present() {
        JOYSTICKS
    } else {
        0
    })
}

/// The axes, buttons and JOYCAPS flags of the joystick `id`; one that
/// isn't plugged in has the driver's standard joystick's.
fn shape(cpu: &Cpu, id: u32) -> (u32, u32, u32) {
    let (axes, buttons) = cpu.bus.joystick.shape(id as usize);
    let mut flags = 0;
    if axes > 2 {
        flags |= JOYCAPS_HASZ;
    }
    if axes > 3 {
        flags |= JOYCAPS_HASR;
    }
    (axes, buttons, flags)
}

/// joyGetDevCapsA(id, caps, size).
pub fn joy_get_dev_caps(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[id, at, size] = args else {
        unreachable!()
    };
    if !cpu.bus.joystick.present() {
        return Some(MMSYSERR_NODRIVER);
    }
    if id >= JOYSTICKS {
        return Some(JOYERR_PARMS);
    }
    if at == 0 {
        return Some(MMSYSERR_INVALPARAM);
    }
    let (axes, buttons, flags) = shape(cpu, id);
    let mut caps = Vec::with_capacity(JOYCAPS_SIZE as usize);
    let put = |c: &mut Vec<u8>, v: u32| c.extend_from_slice(&v.to_le_bytes());
    caps.extend_from_slice(&MM_MICROSOFT.to_le_bytes());
    caps.extend_from_slice(&MM_PC_JOYSTICK.to_le_bytes());
    caps.extend_from_slice(&name_field("Microsoft PC-joystick driver", 32));
    // X, Y and Z's least and most.
    for _ in 0..3 {
        put(&mut caps, 0);
        put(&mut caps, 65535);
    }
    put(&mut caps, buttons);
    put(&mut caps, PERIOD_MIN);
    put(&mut caps, PERIOD_MAX);
    // R, U and V's least and most.
    for _ in 0..3 {
        put(&mut caps, 0);
        put(&mut caps, 65535);
    }
    put(&mut caps, flags);
    put(&mut caps, 4);
    put(&mut caps, axes);
    put(&mut caps, 4);
    caps.extend_from_slice(&name_field("DINPUT.DLL", 32));
    caps.extend_from_slice(&name_field("VJOYD.VXD", 260));
    write_struct(cpu, at, size, &caps)
}

/// The product id of the joystick driver.
const MM_PC_JOYSTICK: u16 = 12;

/// joyGetPos(id, info): JOYINFO.
pub fn joy_get_pos(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[id, at] = args else { unreachable!() };
    if at == 0 {
        return Some(MMSYSERR_INVALPARAM);
    }
    let p = match position(cpu, id) {
        Ok(p) => p,
        Err(e) => return Some(e),
    };
    if !cpu.bus.guest_probe(at, 16, true) {
        return None;
    }
    for (k, v) in [p.axes[0], p.axes[1], p.axes[2], p.buttons]
        .into_iter()
        .enumerate()
    {
        cpu.bus.guest_write_32(at + 4 * k as u32, v);
    }
    Some(JOYERR_NOERROR)
}

/// joyGetPosEx(id, info): JOYINFOEX, the axes and buttons its flags ask
/// for.
pub fn joy_get_pos_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[id, at] = args else { unreachable!() };
    if at == 0 {
        return Some(MMSYSERR_INVALPARAM);
    }
    if !cpu.bus.guest_probe(at, JOYINFOEX_SIZE as usize, true) {
        return None;
    }
    if cpu.bus.guest_read_32(at) != JOYINFOEX_SIZE {
        return Some(JOYERR_PARMS);
    }
    let flags = cpu.bus.guest_read_32(at + 4);
    let p = match position(cpu, id) {
        Ok(p) => p,
        Err(e) => return Some(e),
    };
    let returns = [JOY_RETURNX, JOY_RETURNY, JOY_RETURNZ, JOY_RETURNR];
    for (k, &bit) in returns.iter().enumerate() {
        if flags & bit != 0 {
            cpu.bus.guest_write_32(at + 8 + 4 * k as u32, p.axes[k]);
        }
    }
    if flags & JOY_RETURNBUTTONS != 0 {
        cpu.bus.guest_write_32(at + 32, p.buttons);
        let pressed = p.buttons.count_ones();
        cpu.bus.guest_write_32(at + 36, pressed);
    }
    cpu.bus.guest_write_32(at + 40, JOY_POVCENTERED);
    Some(JOYERR_NOERROR)
}

/// joyGetThreshold(id, threshold).
pub fn joy_get_threshold(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[id, at] = args else { unreachable!() };
    if id >= JOYSTICKS {
        return Some(JOYERR_PARMS);
    }
    if at == 0 {
        return Some(MMSYSERR_INVALPARAM);
    }
    if !cpu.bus.guest_probe(at, 4, true) {
        return None;
    }
    let t = joy(cpu).thresholds[id as usize];
    cpu.bus.guest_write_32(at, t);
    Some(JOYERR_NOERROR)
}

/// joySetThreshold(id, threshold).
pub fn joy_set_threshold(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[id, threshold] = args else {
        unreachable!()
    };
    if id >= JOYSTICKS {
        return Some(JOYERR_PARMS);
    }
    joy(cpu).thresholds[id as usize] = threshold & 0xFFFF;
    Some(JOYERR_NOERROR)
}

/// joySetCapture(hwnd, id, period, changed).
pub fn joy_set_capture(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hwnd, id, period, changed] = args else {
        unreachable!()
    };
    if !cpu.bus.win32.user.is_window(hwnd) {
        return Some(JOYERR_PARMS);
    }
    let last = match position(cpu, id) {
        Ok(p) => p,
        Err(e) => return Some(e),
    };
    if joy(cpu).captures.iter().any(|c| c.joystick == id) {
        return Some(JOYERR_NOCANDO);
    }
    if callback::ensure_thread(cpu).is_none() {
        return Some(JOYERR_NOCANDO);
    }
    let period = period.clamp(PERIOD_MIN, PERIOD_MAX) as u64 * 1000;
    let now = cpu.bus.clock.now_micros();
    let pid = crate::win32::user32::current_pid(cpu);
    joy(cpu).captures.push(Capture {
        joystick: id,
        hwnd,
        pid,
        period,
        next: now + period,
        changed: changed != 0,
        last,
    });
    super::schedule(cpu);
    Some(JOYERR_NOERROR)
}

/// joyReleaseCapture(id).
pub fn joy_release_capture(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = args[0];
    if id >= JOYSTICKS {
        return Some(JOYERR_PARMS);
    }
    joy(cpu).captures.retain(|c| c.joystick != id);
    super::schedule(cpu);
    Some(JOYERR_NOERROR)
}

/// joyConfigChanged(flags).
pub fn joy_config_changed(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(JOYERR_NOERROR)
}

/// The captures of the process `pid` that are due at `now` post their
/// messages.
pub fn poll(cpu: &mut Cpu, pid: u32, now: u64) {
    let due: Vec<usize> = (0..joy(cpu).captures.len())
        .filter(|&i| {
            let c = &cpu.bus.win32.winmm.joy.captures[i];
            c.pid == pid && c.next <= now
        })
        .collect();
    for i in due {
        let c = joy(cpu).captures[i].clone();
        // A capture that fell behind goes on from now.
        let next = (c.next + c.period).max(now + 1);
        joy(cpu).captures[i].next = next;
        if !cpu.bus.win32.user.is_window(c.hwnd) {
            continue;
        }
        let Ok(p) = position(cpu, c.joystick) else {
            continue;
        };
        let threshold = joy(cpu).thresholds[c.joystick as usize];
        let moved = |a: u32, b: u32| a.abs_diff(b) > threshold;
        let (j, hwnd) = (c.joystick, c.hwnd);
        let xy = p.axes[0] & 0xFFFF | p.axes[1] << 16;
        let changes = (p.buttons ^ c.last.buttons) & 0xF;
        let wparam = p.buttons | changes << 8;
        let post = |cpu: &mut Cpu, msg: u32, w: u32, l: u32| {
            crate::win32::user32::message::post_message(cpu, &[hwnd, msg + j, w, l]);
        };
        if moved(p.axes[0], c.last.axes[0]) || moved(p.axes[1], c.last.axes[1]) || !c.changed {
            post(cpu, MM_JOY1MOVE, p.buttons, xy);
        }
        // Only a joystick with a Z axis has Z moves.
        let has_z = shape(cpu, j).2 & JOYCAPS_HASZ != 0;
        if has_z && (moved(p.axes[2], c.last.axes[2]) || !c.changed) {
            post(cpu, MM_JOY1ZMOVE, p.buttons, p.axes[2] & 0xFFFF);
        }
        if changes & p.buttons != 0 {
            post(cpu, MM_JOY1BUTTONDOWN, wparam, xy);
        }
        if changes & !p.buttons != 0 {
            post(cpu, MM_JOY1BUTTONUP, wparam, xy);
        }
        // Only a move past the threshold counts as the new place.
        let c = &mut joy(cpu).captures[i];
        if !c.changed || p.axes.iter().zip(c.last.axes).any(|(&a, b)| moved(a, b)) {
            c.last.axes = p.axes;
        }
        c.last.buttons = p.buttons;
    }
}

/// When the next capture is due, if there is one.
pub fn next_due(cpu: &Cpu, pid: Option<u32>) -> Option<u64> {
    let captures = cpu.bus.win32.winmm.joy.captures.iter();
    captures
        .filter(|c| pid.is_none_or(|p| c.pid == p))
        .map(|c| c.next)
        .min()
}

/// A process ended: its captures go.
pub fn process_ended(cpu: &mut Cpu, pid: u32) {
    joy(cpu).captures.retain(|c| c.pid != pid);
}

crate::state_fields!(Position { axes, buttons });
crate::state_fields!(Capture {
    joystick,
    hwnd,
    pid,
    period,
    next,
    changed,
    last
});
crate::state_fields!(Joy {
    captures,
    thresholds
});

#[cfg(test)]
mod tests {
    use super::*;
    use crate::joystick::{JoystickSettings, JoystickType, PAD_A, PadState};

    fn machine(kind: JoystickType) -> Cpu {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        cpu.bus.set_joystick(JoystickSettings { kind, deadzone: 0 });
        cpu
    }

    /// The mouse is a joystick of two axes: its caps say so, and it has
    /// no Z or R to read at one end of their travel.
    #[test]
    fn the_mouse_is_a_two_axis_joystick() {
        let mut cpu = machine(JoystickType::Mouse);
        cpu.bus.mouse.set_position(639, 0);
        assert_eq!(shape(&cpu, 0), (2, 2, 0));
        let p = position(&mut cpu, 0).expect("joystick 1");
        assert_eq!(p.axes, [65535, 0, 0, 0]);
        assert_eq!(position(&mut cpu, 1), Err(JOYERR_UNPLUGGED));
    }

    /// One controller is a joystick of four axes and four buttons; two
    /// are two joysticks of two, each with its own buttons.
    #[test]
    fn controllers_are_the_joysticks_the_port_has() {
        let pad = |x: f32, buttons| {
            Some(PadState {
                axes: [x, 0.0, 0.0, 0.0],
                buttons,
            })
        };
        let mut cpu = machine(JoystickType::FourAxis);
        cpu.bus.joystick.set_pad(0, pad(1.0, PAD_A));
        assert_eq!(shape(&cpu, 0), (4, 4, JOYCAPS_HASZ | JOYCAPS_HASR));
        assert_eq!(
            position(&mut cpu, 0).map(|p| (p.axes[0], p.buttons)),
            Ok((65535, 1))
        );
        assert_eq!(position(&mut cpu, 1), Err(JOYERR_UNPLUGGED));

        let mut cpu = machine(JoystickType::TwoAxis);
        cpu.bus.joystick.set_pad(0, pad(-1.0, 0));
        cpu.bus.joystick.set_pad(1, pad(1.0, PAD_A));
        assert_eq!((shape(&cpu, 0), shape(&cpu, 1)), ((2, 2, 0), (2, 2, 0)));
        let a = position(&mut cpu, 0).expect("joystick 1");
        let b = position(&mut cpu, 1).expect("joystick 2");
        assert_eq!((a.axes[0], a.buttons), (0, 0));
        assert_eq!((b.axes[0], b.buttons), (65535, 1));
        assert_eq!((a.axes[2], a.axes[3]), (0, 0), "no Z or R");
    }
}
