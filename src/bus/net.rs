//! The network on the bus: the IPX driver of the built-in DOS put in place
//! with an IRQ no card uses, the frames the network thread passed on
//! handed to it at the start of each batch and every millisecond of
//! emulated time while the network runs, its events coming due among the
//! timer events, and its IRQ raised while completions wait.

use super::Bus;
use crate::net::ipx::Ipx;
use crate::net::switch::Port;
use crate::net::{IpxMode, NetSettings};

/// How often the frames of a running network are looked for: a
/// millisecond of emulated time.
const POLL_TICKS: u64 = crate::timer::PIT_HZ / 1000;
/// The IRQs the IPX driver takes, the first one free: those DOSBox's and
/// Novell's drivers are known for, on the second PIC, whose handler the
/// ROM has.
const IPX_IRQS: [u8; 4] = [11, 15, 10, 9];

impl Bus {
    /// Put the `[network]` settings in place: the IPX driver goes in or
    /// out, and takes a changed IRQ or frame type while no socket is open.
    pub fn configure_network(&mut self, settings: &NetSettings) {
        self.net.settings = settings.clone();
        self.configure_ne2000();
        let idle = self.net.ipx.as_ref().is_none_or(|ipx| ipx.sockets.is_empty());
        match settings.ipx {
            IpxMode::Off if idle => self.remove_ipx(),
            IpxMode::On => {
                let changed = self
                    .net
                    .ipx
                    .as_ref()
                    .is_some_and(|ipx| ipx.frame_type != settings.ipx_frame || Some(ipx.irq) != settings.ipx_irq);
                if changed && idle {
                    self.remove_ipx();
                }
                self.install_ipx();
            }
            _ => {}
        }
    }

    /// Host or join the LAN the settings name, at startup. Returns the
    /// problems.
    pub fn start_lan(&mut self) -> Vec<String> {
        let settings = self.net.settings.clone();
        let mut problems = Vec::new();
        if let Some(port) = settings.lan_host {
            self.install_ipx();
            problems.extend(self.net.host(port, &settings.room, &settings.password).err());
        } else if let Some(relay) = &settings.lan {
            self.install_ipx();
            let relay = (!relay.is_empty()).then_some(relay.as_str());
            problems.extend(self.net.join(relay, &settings.room, &settings.password).err());
        }
        problems.into_iter().map(|e| format!("[network]: {}", e)).collect()
    }

    /// The IRQ the IPX driver would take: the one set, or the first that
    /// no sound card uses.
    fn free_ipx_irq(&self) -> u8 {
        if let Some(irq) = self.net.settings.ipx_irq {
            return irq;
        }
        let sb = self.sb.as_ref().map(|sb| sb.config.irq);
        let gus = self.gus.as_ref().and_then(|gus| gus.irq());
        let nic = self.net.settings.ne2000.then_some(self.net.settings.nic_irq);
        IPX_IRQS.into_iter().find(|&irq| ![sb, gus, nic].contains(&Some(irq))).unwrap_or(IPX_IRQS[0])
    }

    /// Install the IPX driver if it isn't.
    pub fn install_ipx(&mut self) {
        if self.net.ipx.is_some() || self.net.settings.ipx == IpxMode::Off {
            return;
        }
        let ipx = Ipx::new(crate::net::frame::Mac::random_local(), self.free_ipx_irq(), self.net.settings.ipx_frame);
        self.log_string(&format!(
            "[IPX] The IPX driver is installed: node {}, IRQ {}, {} frames",
            ipx.node,
            ipx.irq,
            ipx.frame_type.name()
        ));
        self.net.ipx = Some(ipx);
        self.net.ipx_queue.clear();
        self.net.ipx_installed();
        self.arm_ipx_irq();
    }

    /// No program runs any more (the shell starts again, or a system
    /// boots): the IPX driver's sockets and ECBs go with them.
    pub fn reset_network(&mut self) {
        if let Some(ipx) = &mut self.net.ipx {
            ipx.reset();
            let irq = ipx.irq;
            self.pic.lower(irq);
            self.refresh_irq();
        }
    }

    fn remove_ipx(&mut self) {
        if let Some(ipx) = self.net.ipx.take() {
            self.pic.lower(ipx.irq);
            self.refresh_irq();
            self.log_string("[IPX] The IPX driver is removed");
        }
    }

    /// Hand the IPX driver and the network card the frames the network
    /// thread has for them.
    pub(crate) fn net_poll(&mut self) {
        if self.net.nic.is_some() {
            self.ne2000_service();
        } else {
            self.net.nic_queue.clear();
        }
        let now = self.clock.now_ticks();
        match &mut self.net.ipx {
            Some(ipx) => {
                while let Some(frame) = self.net.ipx_queue.pop() {
                    ipx.receive_frame(&frame, now);
                }
            }
            None => self.net.ipx_queue.clear(),
        }
        self.sync_ipx();
        self.serial_link_poll();
    }

    /// The next PIT tick the network needs attention at.
    pub(crate) fn net_next_event(&self) -> Option<u64> {
        let poll = self.net.active().then(|| self.clock.now_ticks() + POLL_TICKS);
        let ipx = self.net.ipx.as_ref().and_then(|ipx| ipx.next_event());
        [poll, ipx, self.ne2000_next_event()].into_iter().flatten().min()
    }

    /// Complete the IPX events that came due, and take the frames that
    /// came.
    pub(crate) fn net_service(&mut self) {
        let now = self.clock.now_ticks();
        if let Some(ipx) = &mut self.net.ipx {
            ipx.advance(now);
        }
        self.net_poll();
    }

    /// After the IPX driver did something: send its frames, log what it
    /// has to say, set up its IRQ for a new socket, and raise the IRQ
    /// while completions wait for its handler.
    pub(crate) fn sync_ipx(&mut self) {
        let Some(ipx) = &mut self.net.ipx else { return };
        let frames = std::mem::take(&mut ipx.outgoing);
        let log = std::mem::take(&mut ipx.log);
        let arm = std::mem::take(&mut ipx.arm);
        let (irq, raise) = (ipx.irq, ipx.wants_irq() && !ipx.sockets.is_empty());
        for frame in frames {
            self.net.send(Port::Ipx, frame);
        }
        for line in log {
            self.log_string(&line);
        }
        if arm {
            self.arm_ipx_irq();
        }
        if raise && !self.pic.busy(irq) {
            self.pic.raise(irq);
            self.refresh_irq();
        }
        self.clock.schedule(self.next_event());
    }

    /// Point the IPX IRQ's vector at the driver's handler where it has the
    /// BIOS's, and let the IRQ through the PICs: when the driver is
    /// installed, when the vectors are put back after a program, and when
    /// a socket opens. A program (a DOS extender) that hooks the vector
    /// later passes the interrupt on to the driver; one that hooked it
    /// through the DPMI host before gets the driver's handler beneath its
    /// own.
    pub(crate) fn arm_ipx_irq(&mut self) {
        let Some(irq) = self.net.ipx.as_ref().map(|ipx| ipx.irq) else { return };
        let vector = if irq < 8 { 0x08 + irq as usize } else { 0x70 + irq as usize - 8 };
        let entry = (self.read_16(vector * 4 + 2) as u32) << 16 | self.read_16(vector * 4) as u32;
        let handler = 0xF000_0000 | crate::bios::IPX_IRQ as u32;
        if crate::bios::is_default_irq_handler(entry) {
            self.write_16(vector * 4, handler as u16);
            self.write_16(vector * 4 + 2, (handler >> 16) as u16);
        } else if let Some(original) = self.dpmi.hooked_original_mut(vector as u8)
            && crate::bios::is_default_irq_handler(*original)
        {
            *original = handler;
        }
        if irq < 8 {
            self.pic.master.imr &= !(1 << irq);
        } else {
            self.pic.slave.imr &= !(1 << (irq - 8));
            self.pic.master.imr &= !0x04;
        }
        self.refresh_irq();
    }
}

/// The NE2000's data port, a word at a time, as the DP8390's remote DMA
/// moves it: 150 ns an ISA access.
const NIC_DATA_NS: u64 = 150;

impl Bus {
    /// The NE2000 as the settings have it: put in, taken out, or put in
    /// anew with other ports, IRQ or address.
    fn configure_ne2000(&mut self) {
        let settings = &self.net.settings;
        let wanted = settings.ne2000.then_some((settings.nic_base, settings.nic_irq, settings.mac));
        let current = self.net.nic.as_ref().map(|nic| (nic.base, nic.irq, nic.mac));
        let same = match (wanted, current) {
            (None, None) => true,
            (Some((base, irq, mac)), Some((b, i, m))) => base == b && irq == i && mac.is_none_or(|mac| mac == m),
            _ => false,
        };
        if same {
            return;
        }
        if let Some(nic) = self.net.nic.take()
            && nic.pic_line
        {
            self.pic.lower(nic.irq);
        }
        if let Some((base, irq, mac)) = wanted {
            let mac = mac.unwrap_or_else(crate::net::frame::Mac::random_local);
            self.log_string(&format!("[NE2000] At {:X}h, IRQ {}, address {}", base, irq, mac));
            self.net.nic = Some(crate::net::ne2000::Ne2000::new(base, irq, mac));
        } else {
            self.log_string("[NE2000] Taken out");
        }
        self.net.nic_changed();
        self.refresh_irq();
    }

    /// A system boots: the card comes up as after a reset.
    pub fn reset_ne2000(&mut self) {
        if let Some(nic) = &mut self.net.nic {
            nic.reset();
        }
        self.sync_ne2000();
    }

    /// Whether `port` is the NE2000's.
    #[inline]
    pub(crate) fn ne2000_claims(&self, port: u16) -> bool {
        self.net
            .nic
            .as_ref()
            .is_some_and(|nic| port.wrapping_sub(nic.base) < crate::net::ne2000::PORTS)
    }

    /// Whether `port` is the NE2000's data port, which takes words.
    #[inline]
    pub(crate) fn ne2000_data_port(&self, port: u16) -> bool {
        self.net.nic.as_ref().is_some_and(|nic| port == nic.base + crate::net::ne2000::DATA)
    }

    pub(crate) fn ne2000_read(&mut self, port: u16) -> u8 {
        let Some(nic) = &mut self.net.nic else { return 0xFF };
        let value = nic.read(port - nic.base);
        self.sync_ne2000();
        value
    }

    pub(crate) fn ne2000_write(&mut self, port: u16, value: u8) {
        let now = self.clock.now_ticks();
        let Some(nic) = &mut self.net.nic else { return };
        nic.write(port - nic.base, value, now);
        self.sync_ne2000();
    }

    /// The data port, 2 or 4 bytes at a time (a doubleword is two words).
    pub(crate) fn ne2000_read_wide(&mut self, port: u16, len: u8) -> u32 {
        self.clock.stall(NIC_DATA_NS * (len as u64 / 2));
        let Some(nic) = &mut self.net.nic else { return 0xFFFF_FFFF };
        let mut value = nic.read_word() as u32;
        if len == 4 {
            value |= (nic.read_word() as u32) << 16;
        }
        self.sync_ne2000();
        self.log_port(port, value, len, false);
        value
    }

    pub(crate) fn ne2000_write_wide(&mut self, port: u16, value: u32, len: u8) {
        self.clock.stall(NIC_DATA_NS * (len as u64 / 2));
        self.log_port(port, value, len, true);
        let Some(nic) = &mut self.net.nic else { return };
        nic.write_word(value as u16);
        if len == 4 {
            nic.write_word((value >> 16) as u16);
        }
        self.sync_ne2000();
    }

    pub(crate) fn ne2000_next_event(&self) -> Option<u64> {
        self.net.nic.as_ref().and_then(|nic| nic.next_event())
    }

    /// After the card did something: send its frames, put waiting frames
    /// into its ring, log what it has to say, and follow its IRQ line.
    fn sync_ne2000(&mut self) {
        let Some(nic) = &mut self.net.nic else { return };
        if nic.has_waiting() {
            nic.drain_waiting();
        }
        let frames = std::mem::take(&mut nic.outgoing);
        let log = std::mem::take(&mut nic.log);
        let line = nic.irq_line();
        let irq = nic.irq;
        let changed = line != nic.pic_line;
        nic.pic_line = line;
        for frame in frames {
            self.net.send(Port::Nic, frame);
        }
        for line in log {
            self.log_string(&line);
        }
        if changed {
            if line {
                self.pic.raise(irq);
            } else {
                self.pic.lower(irq);
            }
            self.refresh_irq();
        }
        self.clock.schedule(self.next_event());
    }

    /// The card's frame is on the wire, and the frames that came are
    /// handed to it.
    pub(crate) fn ne2000_service(&mut self) {
        let now = self.clock.now_ticks();
        if let Some(nic) = &mut self.net.nic {
            nic.advance(now);
            while let Some(frame) = self.net.nic_queue.pop() {
                nic.deliver(frame);
            }
        }
        self.sync_ne2000();
    }
}
