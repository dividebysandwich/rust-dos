//! A monitor on the DDC lines of vidSerialParallelPort: an I2C EEPROM at
//! address A0h holding a 128-byte EDID, which the driver reads by
//! bit-banging the clock and data lines, so Windows finds a Plug and Play
//! monitor and the modes it takes. The EDID describes a CRT that takes up
//! to 1600x1200 at 85 Hz and prefers 1024x768 at 60 Hz.
//!
//! The protocol follows the I2C specification (start and stop conditions,
//! eight data bits, an acknowledge from the receiver) and the EEPROM's
//! random and sequential reads, as 86Box's i2c_gpio.c and i2c_eeprom.c
//! do; the EDID's layout is VESA's EDID 1.3.

/// The EEPROM's I2C address, for writes (reads are A1h).
const ADDRESS: u8 = 0xA0;

/// Where the transfer is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    /// Waiting for a start condition.
    #[default]
    Idle,
    /// Taking a byte from the host: the address, or with `data` an offset.
    Receive { data: bool },
    /// Acknowledging the byte just taken; then sending if `read`.
    Ack { read: bool },
    /// Sending the byte at the offset, a bit at a time.
    Send,
    /// Waiting for the host to acknowledge the byte sent.
    HostAck,
}

/// The EEPROM and the lines as the host drives them.
#[derive(Clone, Debug)]
pub struct Ddc {
    scl: bool,
    sda: bool,
    /// What the EEPROM does to the data line: false pulls it low.
    out: bool,
    state: State,
    bits: u8,
    byte: u8,
    offset: u8,
    acked: bool,
}

impl Default for Ddc {
    fn default() -> Self {
        Self {
            scl: true,
            sda: true,
            out: true,
            state: State::Idle,
            bits: 0,
            byte: 0,
            offset: 0,
            acked: false,
        }
    }
}

impl Ddc {
    /// The clock and the data line as they read: the host's levels, the
    /// data line low where either side pulls it.
    pub fn lines(&self) -> (bool, bool) {
        (self.scl, self.sda && self.out)
    }

    /// The host drives the lines to `scl` and `sda` (true: released).
    pub fn drive(&mut self, scl: bool, sda: bool) {
        let (old_scl, old_sda) = (self.scl, self.sda);
        self.scl = scl;
        self.sda = sda;
        if old_scl && scl && old_sda != sda {
            if sda {
                // Stop.
                self.state = State::Idle;
            } else {
                // Start, or a repeated start.
                self.state = State::Receive { data: false };
                self.bits = 0;
                self.byte = 0;
            }
            self.out = true;
            return;
        }
        if !old_scl && scl {
            self.rising();
        } else if old_scl && !scl {
            self.falling();
        }
    }

    /// The clock rose: the receiver samples the data line.
    fn rising(&mut self) {
        match self.state {
            State::Receive { .. } => {
                self.byte = self.byte << 1 | self.sda as u8;
                self.bits += 1;
            }
            State::HostAck => self.acked = !self.sda,
            _ => {}
        }
    }

    /// The clock fell: the transmitter puts out its next bit.
    fn falling(&mut self) {
        match self.state {
            State::Receive { data } if self.bits == 8 => {
                if data {
                    self.offset = self.byte;
                } else if self.byte & 0xFE != ADDRESS {
                    // Another device's address: let the lines be.
                    self.state = State::Idle;
                    return;
                }
                let read = !data && self.byte & 1 != 0;
                self.out = false;
                self.state = State::Ack { read };
            }
            State::Ack { read } => {
                if read {
                    self.load();
                } else {
                    self.out = true;
                    self.state = State::Receive { data: true };
                    self.bits = 0;
                    self.byte = 0;
                }
            }
            State::Send => {
                self.bits += 1;
                if self.bits == 8 {
                    self.out = true;
                    self.state = State::HostAck;
                } else {
                    self.out = (self.byte >> (7 - self.bits)) & 1 != 0;
                }
            }
            State::HostAck => {
                if self.acked {
                    self.load();
                } else {
                    self.out = true;
                    self.state = State::Idle;
                }
            }
            _ => {}
        }
    }

    /// Start sending the byte at the offset, which moves on.
    fn load(&mut self) {
        self.byte = edid()[self.offset as usize & 0x7F];
        self.offset = self.offset.wrapping_add(1);
        self.bits = 0;
        self.out = self.byte & 0x80 != 0;
        self.state = State::Send;
    }
}

/// The EDID, version 1.3: an analog CRT, "RDS" from rust-dos, 1024x768 at
/// 60 Hz preferred, the VESA established modes to 1280x1024 and standard
/// ones to 1600x1200, 30-96 kHz and 50-120 Hz.
pub fn edid() -> [u8; 128] {
    let mut e = [0u8; 128];
    e[0..8].copy_from_slice(&[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
    // "RDS" in 5-bit letters, product 0001h, no serial number, week 1
    // of 2000.
    e[8..10].copy_from_slice(&[0x48, 0x93]);
    e[10..12].copy_from_slice(&[0x01, 0x00]);
    e[16] = 1;
    e[17] = 10;
    // EDID 1.3; analog input with separate syncs; 32 by 24 cm; gamma 2.2;
    // DPMS standby, suspend and off, RGB colour, the first timing
    // preferred, GTF.
    e[18] = 1;
    e[19] = 3;
    e[20] = 0x08;
    e[21] = 32;
    e[22] = 24;
    e[23] = 0x78;
    e[24] = 0xEB;
    // sRGB's chromaticity.
    e[25..35].copy_from_slice(&[0xEE, 0x91, 0xA3, 0x54, 0x4C, 0x99, 0x26, 0x0F, 0x50, 0x54]);
    // Established timings: 720x400 to 1024x768 at 75 Hz and 1280x1024 at
    // 75 Hz.
    e[35] = 0xFF;
    e[36] = 0xFF;
    e[37] = 0x00;
    // Standard timings: horizontal pixels / 8 - 31, the aspect ratio in
    // bits 7:6 and the refresh rate - 60 in bits 5:0.
    let standard: [(u16, u8, u8); 8] = [
        (1600, 1, 60),
        (1280, 2, 60),
        (1280, 1, 60),
        (1152, 1, 75),
        (1024, 1, 85),
        (800, 1, 85),
        (640, 1, 85),
        (1280, 2, 85),
    ];
    for (i, (width, aspect, hz)) in standard.into_iter().enumerate() {
        e[38 + 2 * i] = (width / 8 - 31) as u8;
        e[39 + 2 * i] = aspect << 6 | (hz - 60);
    }
    // The preferred timing: 1024x768 at 60 Hz, 65 MHz, negative syncs.
    e[54..72].copy_from_slice(&[
        0x64, 0x19, 0x00, 0x40, 0x41, 0x00, 0x26, 0x30, 0x18, 0x88, 0x36, 0x00, 0x40, 0xF0, 0x10,
        0x00, 0x00, 0x18,
    ]);
    // The range limits: 50-120 Hz, 30-96 kHz, 160 MHz, GTF.
    e[72..90].copy_from_slice(&[
        0x00, 0x00, 0x00, 0xFD, 0x00, 50, 120, 30, 96, 16, 0x00, 0x0A, 0x20, 0x20, 0x20, 0x20,
        0x20, 0x20,
    ]);
    // The monitor's name.
    e[90..95].copy_from_slice(&[0x00, 0x00, 0x00, 0xFC, 0x00]);
    e[95..108].copy_from_slice(b"rust-dos CRT\n");
    // An unused descriptor.
    e[108..113].copy_from_slice(&[0x00, 0x00, 0x00, 0x10, 0x00]);
    let sum = e[..127].iter().fold(0u8, |s, &b| s.wrapping_add(b));
    e[127] = 0u8.wrapping_sub(sum);
    e
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The host's side: bit-banging as a driver does.
    struct Host<'a>(&'a mut Ddc);

    impl Host<'_> {
        fn set(&mut self, scl: bool, sda: bool) {
            self.0.drive(scl, sda);
        }
        fn start(&mut self) {
            self.set(true, true);
            self.set(true, false);
            self.set(false, false);
        }
        fn stop(&mut self) {
            self.set(false, false);
            self.set(true, false);
            self.set(true, true);
        }
        /// Send a byte; whether the device acknowledged it.
        fn send(&mut self, byte: u8) -> bool {
            for i in (0..8).rev() {
                let bit = byte >> i & 1 != 0;
                self.set(false, bit);
                self.set(true, bit);
                self.set(false, bit);
            }
            self.set(false, true);
            self.set(true, true);
            let ack = !self.0.lines().1;
            self.set(false, true);
            ack
        }
        fn receive(&mut self, ack: bool) -> u8 {
            let mut byte = 0;
            self.set(false, true);
            for _ in 0..8 {
                self.set(true, true);
                byte = byte << 1 | self.0.lines().1 as u8;
                self.set(false, true);
            }
            self.set(false, !ack);
            self.set(true, !ack);
            self.set(false, !ack);
            byte
        }
    }

    #[test]
    fn the_edid_is_valid() {
        let e = edid();
        assert_eq!(e.iter().fold(0u8, |s, &b| s.wrapping_add(b)), 0);
        assert_eq!(&e[0..8], &[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
        // 1024 active pixels and 768 lines in the preferred timing.
        assert_eq!(e[56] as u32 | ((e[58] as u32 >> 4) << 8), 1024);
        assert_eq!(e[59] as u32 | ((e[61] as u32 >> 4) << 8), 768);
    }

    #[test]
    fn a_random_read_then_a_sequential_one() {
        let mut ddc = Ddc::default();
        let mut host = Host(&mut ddc);
        host.start();
        assert!(host.send(0xA0));
        assert!(host.send(0x08));
        host.start();
        assert!(host.send(0xA1));
        let e = edid();
        assert_eq!(host.receive(true), e[8]);
        assert_eq!(host.receive(true), e[9]);
        assert_eq!(host.receive(false), e[10]);
        host.stop();
        // From the start, a whole block.
        host.start();
        assert!(host.send(0xA0));
        assert!(host.send(0));
        host.start();
        assert!(host.send(0xA1));
        let block: Vec<u8> = (0..128).map(|i| host.receive(i < 127)).collect();
        host.stop();
        assert_eq!(block, e.to_vec());
        // Another address isn't answered.
        host.start();
        assert!(!host.send(0x50));
        host.stop();
    }
}
