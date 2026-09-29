//! The serial ports on the bus: their UARTs at the standard addresses,
//! their IRQ lines (COM1 and COM3 share one, as do COM2 and COM4), their
//! characters coming due among the timer events, the host's mouse moving
//! a serial mouse at the start of each batch, and the cable or modem of
//! the port that goes to another machine talking to the network.

use super::Bus;
use crate::serial::{LinkCmd, PortType, SerialPort, SerialSettings};

/// Serial port I/O takes an ISA bus cycle.
const UART_NS: u64 = 1000;

/// The port index (0-3 for COM1-COM4) whose UART `port` would be one of.
#[inline]
fn port_index(port: u16) -> Option<usize> {
    (port & 0xFEE8 == 0x02E8).then(|| (port & 0x100 == 0) as usize + 2 * (port & 0x10 == 0) as usize)
}

impl Bus {
    /// Put the `[serial]` settings in place: ports come and go, and a port
    /// with something else plugged in, another IRQ or another chip starts
    /// anew.
    pub fn configure_serial(&mut self, settings: &SerialSettings) {
        for n in 0..4 {
            let kind = settings.ports[n];
            let current = self.serial.ports[n].as_ref().map(|p| (p.backend.kind(), p.uart.irq, p.uart.chip));
            let wanted = (kind != PortType::Off).then_some((kind, settings.irqs[n], settings.chip));
            let same = match (current, wanted) {
                (Some((k, irq, chip)), Some((wk, wirq, wchip))) => {
                    k == wk && irq == wirq && chip == wchip && (k != PortType::Mouse || settings.mouse == self.serial.settings.mouse)
                }
                (None, None) => true,
                _ => false,
            };
            if same {
                continue;
            }
            if let Some(port) = self.serial.ports[n].take()
                && port.backend.kind().links()
            {
                self.net.serial_command(n, LinkCmd::Hangup);
            }
            if let Some((kind, irq, _)) = wanted {
                self.log_string(&format!(
                    "[SERIAL] COM{} at {:X}h, IRQ {}: {}",
                    n + 1,
                    crate::serial::BASES[n],
                    irq,
                    kind.name()
                ));
                self.serial.ports[n] = Some(SerialPort::new(n, kind, settings));
            } else {
                self.log_string(&format!("[SERIAL] COM{} taken out", n + 1));
            }
        }
        self.serial.settings = settings.clone();
        self.net.set_serial(settings);
        self.sync_serial_irqs();
        if self.boot.is_none() {
            self.write_serial_bda();
        }
    }

    /// The serial ports in the BIOS data area (40:00-40:07) and their
    /// number in the equipment word (bits 9-11).
    pub(crate) fn write_serial_bda(&mut self) {
        let bases = self.serial.bios_ports();
        for n in 0..4 {
            self.write_16(0x0400 + 2 * n, bases.get(n).copied().unwrap_or(0));
        }
        let equipment = self.read_16(0x0410) & !0x0E00;
        self.write_16(0x0410, equipment | (bases.len() as u16) << 9);
    }

    /// Whether `port` is a serial port's.
    #[inline]
    pub(crate) fn serial_claims(&self, port: u16) -> bool {
        port_index(port).is_some_and(|n| self.serial.ports[n].is_some())
    }

    pub(crate) fn serial_read(&mut self, port: u16) -> u8 {
        self.clock.stall(UART_NS);
        let now = self.clock.now_ticks();
        let Some(n) = port_index(port) else { return 0xFF };
        let Some(p) = &mut self.serial.ports[n] else { return 0xFF };
        let value = p.uart.read(port & 7, now);
        p.sync(now);
        self.sync_serial(n);
        value
    }

    pub(crate) fn serial_write(&mut self, port: u16, value: u8) {
        self.clock.stall(UART_NS);
        let now = self.clock.now_ticks();
        let Some(n) = port_index(port) else { return };
        let Some(p) = &mut self.serial.ports[n] else { return };
        p.uart.write(port & 7, value, now);
        p.sync(now);
        self.sync_serial(n);
    }

    pub(crate) fn serial_next_event(&self) -> Option<u64> {
        self.serial.ports.iter().flatten().filter_map(|p| p.next_event()).min()
    }

    /// Bring the ports whose events came due up to now.
    pub(crate) fn serial_service(&mut self) {
        let now = self.clock.now_ticks();
        for n in 0..4 {
            if let Some(p) = &mut self.serial.ports[n]
                && p.next_event().is_some_and(|t| t <= now)
            {
                p.advance(now);
                self.sync_serial(n);
            }
        }
    }

    /// At the start of a batch: the host mouse's motion and buttons to a
    /// serial mouse in use.
    pub(crate) fn serial_mouse_poll(&mut self) {
        if !self.serial.any() {
            return;
        }
        let now = self.clock.now_ticks();
        let mut in_use = false;
        for n in 0..4 {
            let Some(p) = &mut self.serial.ports[n] else { continue };
            if let crate::serial::Backend::Mouse(mouse) = &mut p.backend
                && mouse.powered
            {
                in_use = true;
                let buttons = self.mouse.buttons;
                mouse.report(&mut p.uart, &mut self.mouse.serial_dx, &mut self.mouse.serial_dy, buttons, now);
                self.sync_serial(n);
            }
        }
        if !in_use {
            self.mouse.serial_dx = 0;
            self.mouse.serial_dy = 0;
        }
    }

    /// With the network's frames: the characters the ports sent go out,
    /// and what came for them goes in.
    pub(crate) fn serial_link_poll(&mut self) {
        if !self.serial.any() {
            return;
        }
        let now = self.clock.now_ticks();
        self.net.serial_flush();
        for (n, event) in self.net.serial_events() {
            if let Some(p) = &mut self.serial.ports[n] {
                p.link_event(event, now);
                p.sync(now);
                self.sync_serial(n);
            }
        }
    }

    /// After port `n` did something: pass what its cable or modem asks on
    /// to the network, follow the IRQ lines, and schedule its next event.
    fn sync_serial(&mut self, n: usize) {
        if let Some(p) = &mut self.serial.ports[n]
            && !p.link.is_empty()
        {
            let commands = std::mem::take(&mut p.link);
            for command in commands {
                self.net.serial_command(n, command);
            }
        }
        self.sync_serial_irqs();
        self.clock.schedule(self.next_event());
    }

    /// Raise the IRQs a port holds its line up on, and lower those none
    /// does any more.
    fn sync_serial_irqs(&mut self) {
        let mut lines = 0u16;
        for p in self.serial.ports.iter().flatten() {
            if p.uart.irq_line() {
                lines |= 1 << p.uart.irq;
            }
        }
        let changed = lines ^ self.serial.pic_lines;
        if changed == 0 {
            return;
        }
        for irq in 0..16u8 {
            if changed & 1 << irq != 0 {
                if lines & 1 << irq != 0 {
                    self.pic.raise(irq);
                } else {
                    self.pic.lower(irq);
                }
            }
        }
        self.serial.pic_lines = lines;
        self.refresh_irq();
    }

    /// No program runs any more: the ports whose IRQ handler went with it
    /// stop interrupting (their lines and what they hold stay as they
    /// are, as a modem keeps its call). The PICs are new: no line is up.
    pub fn reset_serial_irqs(&mut self) {
        let resident = |bus: &Bus, irq: u8| {
            let vector = if irq < 8 { 0x08 + irq as usize } else { 0x70 + irq as usize - 8 };
            let entry = (bus.read_16(vector * 4 + 2) as u32) << 16 | bus.read_16(vector * 4) as u32;
            !crate::bios::is_default_irq_handler(entry)
        };
        let now = self.clock.now_ticks();
        for n in 0..4 {
            let Some(irq) = self.serial.ports[n].as_ref().map(|p| p.uart.irq) else { continue };
            if !resident(self, irq)
                && let Some(p) = &mut self.serial.ports[n]
            {
                p.uart.write(1, 0, now);
            }
        }
        self.serial.pic_lines = 0;
        self.sync_serial_irqs();
    }

    /// A system boots: the UARTs come up as after a reset.
    pub fn reset_serial(&mut self) {
        let now = self.clock.now_ticks();
        for p in self.serial.ports.iter_mut().flatten() {
            p.uart.reset();
            p.sync(now);
        }
        for n in 0..4 {
            self.sync_serial(n);
        }
        self.serial.pic_lines = 0;
        self.sync_serial_irqs();
    }

    /// Send a character through port `n` the way the BIOS does (INT 14h,
    /// DOS's COM devices). Returns false without the port.
    pub fn serial_send(&mut self, n: usize, byte: u8) -> bool {
        let now = self.clock.now_ticks();
        let Some(p) = self.serial.ports.get_mut(n).and_then(|p| p.as_mut()) else { return false };
        p.uart.send(byte, now);
        p.sync(now);
        self.sync_serial(n);
        true
    }

    /// Take a character from port `n` the way the BIOS does, if one came.
    pub fn serial_receive(&mut self, n: usize) -> Option<u8> {
        let now = self.clock.now_ticks();
        let p = self.serial.ports.get_mut(n)?.as_mut()?;
        let byte = p.uart.take(now);
        p.sync(now);
        self.sync_serial(n);
        byte
    }

    /// Port `n`'s line and modem status (INT 14h AH=03h), reading them as
    /// a program would.
    pub fn serial_status(&mut self, n: usize) -> Option<(u8, u8)> {
        let now = self.clock.now_ticks();
        let p = self.serial.ports.get_mut(n)?.as_mut()?;
        let lsr = p.uart.read(5, now);
        let msr = p.uart.read(6, now);
        self.sync_serial(n);
        Some((lsr, msr))
    }

    /// Set port `n` up the way INT 14h AH=00h does: `params` has the baud
    /// rate in bits 5-7, the parity in 3-4, the stop bits in 2 and the
    /// word length in 0-1.
    pub fn serial_init(&mut self, n: usize, params: u8) -> bool {
        const DIVISORS: [u16; 8] = [1047, 768, 384, 192, 96, 48, 24, 12];
        let now = self.clock.now_ticks();
        let Some(p) = self.serial.ports.get_mut(n).and_then(|p| p.as_mut()) else { return false };
        let divisor = DIVISORS[(params >> 5) as usize];
        let lcr = params & 0x1F;
        p.uart.write(3, 0x80, now);
        p.uart.write(0, divisor as u8, now);
        p.uart.write(1, (divisor >> 8) as u8, now);
        p.uart.write(3, lcr, now);
        p.sync(now);
        self.sync_serial(n);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::port_index;

    #[test]
    fn port_indexes() {
        assert_eq!(port_index(0x3F8), Some(0));
        assert_eq!(port_index(0x3FF), Some(0));
        assert_eq!(port_index(0x2F8), Some(1));
        assert_eq!(port_index(0x3E8), Some(2));
        assert_eq!(port_index(0x2EC), Some(3));
        assert_eq!(port_index(0x3F0), None);
        assert_eq!(port_index(0x378), None);
        assert_eq!(port_index(0x2E0), None);
        assert_eq!(port_index(0x1F8), None);
    }
}
