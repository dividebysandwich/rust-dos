//! A Microsoft serial mouse on a serial port: 1200 baud, 7 data bits, no
//! parity, one stop bit, powered by the port's DTR and RTS lines. When the
//! driver turns them on it answers with its ID, `M` (or `M3` for
//! Logitech's three-button mouse), and then reports each motion and button
//! change in three bytes:
//!
//! ```text
//! 1st: 0 1 L R Y7 Y6 X7 X6
//! 2nd: 0 0 X5 X4 X3 X2 X1 X0
//! 3rd: 0 0 Y5 Y4 Y3 Y2 Y1 Y0
//! ```
//!
//! X and Y are the motion since the last report, down positive. Logitech's
//! mouse adds a fourth byte, 20h, while the middle button is down, and a 0
//! when it comes up.

use super::uart::{MCR_DTR, MCR_RTS, Uart};
use crate::mouse::{BUTTON_LEFT, BUTTON_MIDDLE, BUTTON_RIGHT};

/// The mice there are.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MouseType {
    /// Two buttons, ID `M`.
    #[default]
    Microsoft,
    /// Three buttons, ID `M3`.
    Logitech,
}

crate::state_enum!(MouseType { MouseType::Microsoft, MouseType::Logitech });

impl MouseType {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "microsoft" | "ms" | "2button" => Some(MouseType::Microsoft),
            "logitech" | "3button" => Some(MouseType::Logitech),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            MouseType::Microsoft => "microsoft",
            MouseType::Logitech => "logitech",
        }
    }

    fn id(self) -> &'static [u8] {
        match self {
            MouseType::Microsoft => b"M",
            MouseType::Logitech => b"M3",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SerialMouse {
    pub kind: MouseType,
    /// DTR and RTS are both on: the mouse has power.
    pub powered: bool,
    /// The buttons last reported.
    reported: u8,
}

crate::state_fields!(SerialMouse { kind, powered, reported });

impl SerialMouse {
    pub fn new(kind: MouseType) -> Self {
        Self { kind, powered: false, reported: 0 }
    }

    /// The port's modem control lines are now `mcr`: turned on, the mouse
    /// starts and says what it is.
    pub fn control(&mut self, uart: &mut Uart, mcr: u8, now: u64) {
        let powered = mcr & (MCR_DTR | MCR_RTS) == MCR_DTR | MCR_RTS;
        if powered && !self.powered {
            self.reported = 0;
            uart.receive(self.kind.id(), now);
        }
        self.powered = powered;
    }

    /// Report the motion (`dx`, `dy`, taken from as far as a report holds)
    /// and the buttons, if either changed and the last report went through.
    pub fn report(&mut self, uart: &mut Uart, dx: &mut i32, dy: &mut i32, buttons: u8, now: u64) {
        let buttons = match self.kind {
            MouseType::Microsoft => buttons & (BUTTON_LEFT | BUTTON_RIGHT),
            MouseType::Logitech => buttons & (BUTTON_LEFT | BUTTON_RIGHT | BUTTON_MIDDLE),
        };
        if !self.powered || !uart.rx_idle() || (*dx == 0 && *dy == 0 && buttons == self.reported) {
            return;
        }
        let x = (*dx).clamp(-128, 127);
        let y = (*dy).clamp(-128, 127);
        *dx -= x;
        *dy -= y;
        let packet = encode(x as i8, y as i8, buttons);
        let middle = buttons & BUTTON_MIDDLE != 0;
        let was_middle = self.reported & BUTTON_MIDDLE != 0;
        self.reported = buttons;
        if middle || was_middle {
            let mut long = packet.to_vec();
            long.push(if middle { 0x20 } else { 0 });
            uart.receive(&long, now);
        } else {
            uart.receive(&packet, now);
        }
    }
}

/// The three bytes of a report.
pub fn encode(dx: i8, dy: i8, buttons: u8) -> [u8; 3] {
    let (x, y) = (dx as u8, dy as u8);
    let left = (buttons & BUTTON_LEFT != 0) as u8;
    let right = (buttons & BUTTON_RIGHT != 0) as u8;
    [0x40 | left << 5 | right << 4 | (y >> 6) << 2 | x >> 6, x & 0x3F, y & 0x3F]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serial::uart::Chip;

    #[test]
    fn packet_bits() {
        assert_eq!(encode(1, -1, BUTTON_LEFT), [0x40 | 0x20 | 0x0C, 0x01, 0x3F]);
        assert_eq!(encode(-128, 127, BUTTON_RIGHT), [0x40 | 0x10 | 0x04 | 0x02, 0x00, 0x3F]);
    }

    fn drain(uart: &mut Uart, now: &mut u64) -> Vec<u8> {
        let mut bytes = Vec::new();
        for _ in 0..100 {
            *now += uart.char_ticks();
            while let Some(b) = uart.take(*now) {
                bytes.push(b);
            }
        }
        bytes
    }

    #[test]
    fn id_when_powered_then_reports() {
        let mut uart = Uart::new(0x3F8, 4, Chip::Ns16550);
        let mut mouse = SerialMouse::new(MouseType::Logitech);
        let mut now = 0;
        mouse.control(&mut uart, MCR_DTR, now);
        assert!(uart.rx_idle());
        mouse.control(&mut uart, MCR_DTR | MCR_RTS, now);
        assert_eq!(drain(&mut uart, &mut now), b"M3");
        let (mut dx, mut dy) = (150, 0);
        mouse.report(&mut uart, &mut dx, &mut dy, BUTTON_MIDDLE, now);
        assert_eq!(dx, 23);
        assert_eq!(drain(&mut uart, &mut now), [0x41, 0x3F, 0x00, 0x20]);
        mouse.report(&mut uart, &mut dx, &mut dy, 0, now);
        assert_eq!(drain(&mut uart, &mut now), [0x40, 0x17, 0x00, 0x00]);
        // Nothing changed: nothing sent.
        mouse.report(&mut uart, &mut dx, &mut dy, 0, now);
        assert!(uart.rx_idle());
    }
}
