//! A null modem cable to the other player in a LAN room: what one sends
//! the other receives, and each one's DTR is the other's DSR and DCD. CTS
//! stays on while the cable is there: the link buffers what the other end
//! hasn't read yet, so no one needs to wait.

use super::uart::{MCR_DTR, MCR_RTS, MSR_CTS, MSR_DCD, MSR_DSR, Uart};
use super::{LinkCmd, LinkEvent};

#[derive(Clone, Debug, Default)]
pub struct NullModem {
    /// The other player is there.
    pub peer: bool,
    /// Their DTR.
    peer_dtr: bool,
    /// Ours.
    dtr: bool,
    rts: bool,
}

crate::state_fields!(NullModem { dtr, rts } skip { peer, peer_dtr });

impl NullModem {
    /// The lines from the other end.
    pub fn lines(&self) -> u8 {
        if !self.peer {
            return 0;
        }
        MSR_CTS | if self.peer_dtr { MSR_DSR | MSR_DCD } else { 0 }
    }

    pub fn control(&mut self, mcr: u8, link: &mut Vec<LinkCmd>) {
        self.dtr = mcr & MCR_DTR != 0;
        self.rts = mcr & MCR_RTS != 0;
        link.push(LinkCmd::Lines { dtr: self.dtr, rts: self.rts });
    }

    pub fn transmit(&mut self, bytes: &[u8], link: &mut Vec<LinkCmd>) {
        if self.peer {
            link.push(LinkCmd::Bytes(bytes.to_vec()));
        }
    }

    pub fn event(&mut self, uart: &mut Uart, event: LinkEvent, now: u64, link: &mut Vec<LinkCmd>) {
        match event {
            LinkEvent::Peer(there) => {
                self.peer = there;
                self.peer_dtr = false;
                if there {
                    link.push(LinkCmd::Lines { dtr: self.dtr, rts: self.rts });
                }
            }
            LinkEvent::Bytes(bytes) if self.peer => uart.receive(&bytes, now),
            LinkEvent::Lines { dtr, .. } => self.peer_dtr = dtr,
            _ => {}
        }
        uart.set_lines(self.lines());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serial::uart::Chip;

    #[test]
    fn lines_follow_the_other_end() {
        let mut uart = Uart::new(0x2F8, 3, Chip::Ns16550);
        let mut cable = NullModem::default();
        let mut link = Vec::new();
        cable.control(MCR_DTR | MCR_RTS, &mut link);
        cable.transmit(b"lost", &mut link);
        assert_eq!(link, [LinkCmd::Lines { dtr: true, rts: true }]);
        link.clear();
        cable.event(&mut uart, LinkEvent::Peer(true), 0, &mut link);
        assert_eq!(link, [LinkCmd::Lines { dtr: true, rts: true }]);
        assert_eq!(uart.lines(), MSR_CTS);
        cable.event(&mut uart, LinkEvent::Lines { dtr: true, rts: false }, 0, &mut link);
        assert_eq!(uart.lines(), MSR_CTS | MSR_DSR | MSR_DCD);
        cable.event(&mut uart, LinkEvent::Bytes(b"hi".to_vec()), 0, &mut link);
        assert_eq!(uart.receiving(), 2);
        cable.event(&mut uart, LinkEvent::Peer(false), 0, &mut link);
        assert_eq!(uart.lines(), 0);
    }
}
