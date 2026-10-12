//! The profiler's view of the bus: which device has each port.

use super::{Bus, Device};
use crate::profile::Zone;

impl Bus {
    /// The zone of port `port`: the device that has it, in the order
    /// `write_port` looks for one.
    pub fn port_zone(&self, port: u16) -> Zone {
        match port {
            _ if self.verite_port(port).is_some() => Zone::IoVerite,
            p if self.ide_claims(p) => Zone::IoIde,
            p if self.ne2000_claims(p) => Zone::IoNe2000,
            p if self.serial_claims(p) => Zone::IoSerial,
            0x3BF | 0x3CD if self.et4000() => Zone::IoEt4000,
            0x20 | 0x21 | 0xA0 | 0xA1 => Zone::IoPic,
            0x60 | 0x64 => Zone::IoKbc,
            0x70 | 0x71 => Zone::IoCmos,
            0x92 | 0xCF9 | 0x80 | 0x190 | 0xE9 | 0xED | 0xF0 | 0xF1 => Zone::IoSystem,
            0x40..=0x43 => Zone::IoPit,
            0x61 => Zone::IoSpeaker,
            0x388..=0x38B => Zone::IoOpl,
            p if self.sb_base().is_some_and(|b| p & 0xFFF0 == b) => Zone::IoSb,
            p if self.gus_claims(p) => Zone::IoGus,
            p if self.awe_claims(p) => Zone::IoAwe,
            0x330 | 0x331 => Zone::IoMpu,
            0xC0 | 0xC1 if self.tandy_sound_enabled() => Zone::IoTandy,
            p if crate::dma::Dma::owns(p) => Zone::IoDma,
            0x201 => Zone::IoJoystick,
            p if self.printer_claims(p) => Zone::IoPrinter,
            crate::lpt_dac::DATA | crate::lpt_dac::CONTROL if self.lpt_dac.is_some() => Zone::IoLptDac,
            p if self.s3() && super::s3::is_engine_port(p) => Zone::IoS3,
            0xCF8..=0xCFF if self.pci_present() => Zone::IoPci,
            p if self.vga.ports().contains(&p) => Zone::IoVga,
            _ => Zone::IoUnhandled,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_go_to_their_devices() {
        let bus = Bus::new(std::path::PathBuf::from("."));
        assert_eq!(bus.port_zone(0x3DA), Zone::IoVga);
        assert_eq!(bus.port_zone(0x3C9), Zone::IoVga);
        assert_eq!(bus.port_zone(0x20), Zone::IoPic);
        assert_eq!(bus.port_zone(0x40), Zone::IoPit);
        assert_eq!(bus.port_zone(0x60), Zone::IoKbc);
        assert_eq!(bus.port_zone(0x388), Zone::IoOpl);
        assert_eq!(bus.port_zone(0x0C), Zone::IoDma);
        assert_eq!(bus.port_zone(0x201), Zone::IoJoystick);
        assert_eq!(bus.port_zone(0x1234), Zone::IoUnhandled);
    }
}
