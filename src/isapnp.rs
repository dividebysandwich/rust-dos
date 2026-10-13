//! ISA Plug and Play (the ISA PnP specification 1.0a): the ADDRESS port
//! (279h), the WRITE_DATA port (A79h) and the READ_DATA port software
//! picks (203h-3FFh), with the initiation key, isolation by serial
//! identifier, card select numbers (CSNs), the resource data and the
//! logical devices' configuration registers.
//!
//! The one card is the Sound Blaster as Creative's ViBRA16C (CTL0070,
//! `sbtype=sb16vibra`), with the resource data DOSBox-X gives it
//! (sblaster.cpp, `ViBRA_PnP`; the protocol is in bios.cpp). As there, the
//! BIOS has isolated and configured the card at power-on, as a Plug and
//! Play BIOS does: it has CSN 1, the read port is 20Bh, and the card
//! answers at the resources the settings give it, which the BLASTER
//! variable tells DOS programs. An operating system that isolates the
//! cards again and gives the card other resources (Windows 95 does) moves
//! the Sound Blaster to them; with the audio device deactivated, its ports
//! and lines are gone.

use crate::savestate::{Reader, Result, State, StateError, Writer};

/// The ports every card listens on.
pub const ADDRESS: u16 = 0x279;
pub const WRITE_DATA: u16 = 0xA79;

/// The READ_DATA port the BIOS leaves set, where DOSBox-X's has it.
pub const BIOS_READ_PORT: u16 = 0x20B;

/// The initiation key: 32 writes to ADDRESS of the values a linear
/// feedback shift register goes through from 6Ah.
pub const KEY: [u8; 32] = {
    let mut key = [0u8; 32];
    let mut x = 0x6Au8;
    let mut i = 0;
    while i < 32 {
        key[i] = x;
        x = (x >> 1) | (((x ^ (x >> 1)) & 1) << 7);
        i += 1;
    }
    key
};

/// Where a card is in the protocol.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CardState {
    /// Deaf to WRITE_DATA until the initiation key comes.
    #[default]
    WaitForKey,
    Sleep,
    /// Taking part in the serial isolation (CSN 0, woken with Wake[0]).
    Isolation,
    /// Woken with its CSN: its registers are read and written.
    Config,
}
crate::state_enum!(CardState { CardState::WaitForKey, CardState::Sleep, CardState::Isolation, CardState::Config });

/// A logical device's registers, 00h-FFh (those below 30h are the card's
/// and unused here).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Registers([u8; 256]);

impl Default for Registers {
    /// The power-up values: inactive, no I/O ranges, no IRQ (an edge
    /// high one's type) and no DMA channels (4).
    fn default() -> Self {
        let mut regs = [0u8; 256];
        regs[0x71] = 0x02;
        regs[0x73] = 0x02;
        regs[0x74] = 0x04;
        regs[0x75] = 0x04;
        Self(regs)
    }
}

impl State for Registers {
    fn save(&self, w: &mut Writer) {
        self.0.save(w);
    }
    fn load(&mut self, r: &mut Reader) -> Result<()> {
        self.0.load(r)
    }
}

/// An ISA Plug and Play card.
pub struct Card {
    /// The serial identifier: vendor ID, serial number and checksum.
    ident: [u8; 9],
    /// What reading the resource data gives: the serial identifier, the
    /// resource data, and zeros after them, 256 bytes over and over as
    /// DOSBox-X found real ViBRA cards to repeat their ROM.
    rom: Vec<u8>,
    pub csn: u8,
    pub state: CardState,
    /// The serial identifier's bit the isolation is at (0-71), and
    /// whether its first read (55h) has been.
    serial_bit: u8,
    serial_second: bool,
    /// The next byte of `rom` reading the resource data gives.
    rom_pos: u8,
    /// The logical device the registers from 30h are of.
    logical: u8,
    devices: Vec<Registers>,
}

crate::state_fields!(Card { csn, state, serial_bit, serial_second, rom_pos, logical, devices } skip { ident, rom });

impl Card {
    /// A card at power-up, waiting for the key, with no CSN and its
    /// logical devices inactive.
    fn new(vendor: [u8; 4], serial: u32, resources: &[u8], logical_devices: usize) -> Self {
        let mut ident = [0u8; 9];
        ident[0..4].copy_from_slice(&vendor);
        ident[4..8].copy_from_slice(&serial.to_le_bytes());
        ident[8] = checksum(&ident[0..8]);
        let mut rom = ident.to_vec();
        rom.extend_from_slice(resources);
        rom.resize(256, 0);
        Self {
            ident,
            rom,
            csn: 0,
            state: CardState::WaitForKey,
            serial_bit: 0,
            serial_second: false,
            rom_pos: 0,
            logical: 0,
            devices: vec![Registers::default(); logical_devices],
        }
    }

    /// Whether the card takes the commands written to WRITE_DATA.
    fn listening(&self) -> bool {
        self.state != CardState::WaitForKey
    }

    /// The bit of the serial identifier the isolation is at.
    fn serial(&self) -> Option<bool> {
        (self.serial_bit < 72).then(|| self.ident[self.serial_bit as usize / 8] >> (self.serial_bit % 8) & 1 != 0)
    }

    /// A register of the selected logical device.
    fn register(&self, reg: u8) -> u8 {
        self.devices.get(self.logical as usize).map_or(0xFF, |d| d.0[reg as usize])
    }

    /// Logical device `device`'s register `reg`.
    pub fn device_register(&self, device: usize, reg: u8) -> u8 {
        self.devices.get(device).map_or(0xFF, |d| d.0[reg as usize])
    }

    fn set_device_register(&mut self, device: usize, reg: u8, value: u8) {
        if let Some(d) = self.devices.get_mut(device) {
            d.0[reg as usize] = value;
        }
    }
}

/// The checksum of a serial identifier's first 8 bytes: the key's shift
/// register run over its bits, least significant first.
fn checksum(bytes: &[u8]) -> u8 {
    let mut sum = 0x6Au8;
    for byte in bytes {
        for bit in 0..8 {
            let b = byte >> bit & 1;
            sum = ((((sum ^ (sum >> 1)) & 1) ^ b) << 7) | (sum >> 1);
        }
    }
    sum
}

/// The ISA Plug and Play cards and the ports they share.
pub struct IsaPnp {
    pub cards: Vec<Card>,
    /// The register the last write to ADDRESS selected.
    address: u8,
    /// How much of the initiation key has come.
    key_pos: u8,
    /// The READ_DATA port, if one is set.
    read_port: Option<u16>,
}

impl State for IsaPnp {
    fn save(&self, w: &mut Writer) {
        self.address.save(w);
        self.key_pos.save(w);
        self.read_port.save(w);
        self.cards.len().save(w);
        for card in &self.cards {
            card.save(w);
        }
    }
    fn load(&mut self, r: &mut Reader) -> Result<()> {
        self.address.load(r)?;
        self.key_pos.load(r)?;
        self.read_port.load(r)?;
        let mut count = 0usize;
        count.load(r)?;
        if count != self.cards.len() {
            return Err(StateError::Mismatch(format!(
                "it has {} ISA Plug and Play cards and this machine has {}",
                count,
                self.cards.len()
            )));
        }
        for card in &mut self.cards {
            let devices = card.devices.len();
            card.load(r)?;
            if card.devices.len() != devices || card.pointers_bad() {
                return Err(StateError::Invalid("an ISA Plug and Play card's logical devices".into()));
            }
        }
        if self.key_pos as usize >= KEY.len() {
            return Err(StateError::Invalid("an ISA Plug and Play key position".into()));
        }
        if self.read_port.is_some_and(|port| !(0x203..=0x3FF).contains(&port) || port & 3 != 3) {
            return Err(StateError::Invalid("an ISA Plug and Play read port".into()));
        }
        Ok(())
    }
}

impl Card {
    /// Whether a loaded state's pointers are out of range.
    fn pointers_bad(&self) -> bool {
        self.serial_bit > 72
    }
}

/// The ViBRA16C's logical devices: the audio (Sound Blaster, MPU-401 and
/// OPL3) and the game port.
const AUDIO: usize = 0;
const GAME: usize = 1;

/// Where the logical device registers' resources are.
const ACTIVATE: u8 = 0x30;
const IO_BASE: u8 = 0x60;
const IRQ: u8 = 0x70;
const DMA: u8 = 0x74;

/// The Sound Blaster's resources as the audio device's registers have
/// them, while it is active.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Audio {
    pub base: u16,
    pub irq: Option<u8>,
    pub dma8: Option<u8>,
    pub dma16: Option<u8>,
}

impl IsaPnp {
    /// The bus with the Sound Blaster as a ViBRA16C on it, as the BIOS
    /// leaves it: CSN 1, the audio device active at the configured base,
    /// IRQ and DMA channels with the MPU-401 at 330h and the OPL3 at 388h,
    /// the game port at 200h, and the read port at 20Bh.
    pub fn vibra16c(sb: &crate::sb::SbConfig) -> Self {
        // CTL0070, serial number FFFFFFFFh as DOSBox-X's.
        let mut card = Card::new(crate::pnpbios::eisa_id(b"CTL0070"), 0xFFFF_FFFF, &vibra16c_resources(), 2);
        card.csn = 1;
        let ports = |card: &mut Card, device: usize, n: u8, port: u16| {
            card.set_device_register(device, IO_BASE + 2 * n, (port >> 8) as u8);
            card.set_device_register(device, IO_BASE + 2 * n + 1, port as u8);
        };
        ports(&mut card, AUDIO, 0, sb.base);
        ports(&mut card, AUDIO, 1, 0x330);
        ports(&mut card, AUDIO, 2, 0x388);
        card.set_device_register(AUDIO, IRQ, sb.irq);
        card.set_device_register(AUDIO, DMA, sb.dma8);
        card.set_device_register(AUDIO, DMA + 1, sb.dma16);
        card.set_device_register(AUDIO, ACTIVATE, 1);
        ports(&mut card, GAME, 0, 0x200);
        card.set_device_register(GAME, ACTIVATE, 1);
        Self { cards: vec![card], address: 0, key_pos: 0, read_port: Some(BIOS_READ_PORT) }
    }

    /// The READ_DATA port, if one is set.
    pub fn read_port(&self) -> Option<u16> {
        self.read_port
    }

    /// How many CSNs the BIOS gave out, for the Plug and Play BIOS's ISA
    /// configuration structure.
    pub fn csns(&self) -> u8 {
        self.cards.len() as u8
    }

    /// The ViBRA16C's Sound Blaster resources, None while its audio device
    /// is inactive. IRQ 0 and DMA channel 4 are none.
    pub fn audio(&self) -> Option<Audio> {
        let card = self.cards.first()?;
        let reg = |r| card.device_register(AUDIO, r);
        if reg(ACTIVATE) & 1 == 0 {
            return None;
        }
        let dma = |r| Some(reg(r) & 7).filter(|&ch| ch != 4);
        Some(Audio {
            base: (reg(IO_BASE) as u16) << 8 | reg(IO_BASE + 1) as u16,
            irq: Some(reg(IRQ) & 0x0F).filter(|&irq| irq != 0),
            dma8: dma(DMA),
            dma16: dma(DMA + 1),
        })
    }

    /// A write to ADDRESS: selects a register, and counts towards the
    /// initiation key, which puts the cards waiting for it to sleep. A
    /// value out of the key's sequence starts it again.
    pub fn write_address(&mut self, value: u8) {
        self.address = value;
        if value == KEY[self.key_pos as usize] {
            self.key_pos += 1;
            if self.key_pos as usize == KEY.len() {
                self.key_pos = 0;
                for card in &mut self.cards {
                    if card.state == CardState::WaitForKey {
                        card.state = CardState::Sleep;
                    }
                }
            }
        } else {
            self.key_pos = (value == KEY[0]) as u8;
        }
    }

    /// A write to WRITE_DATA, into the register ADDRESS selected.
    pub fn write_data(&mut self, value: u8) {
        let reg = self.address;
        match reg {
            // Set RD_DATA port: bits 9-2 of the port, whose bits 1-0 are
            // 11b, in 203h-3FFh.
            0x00 if self.cards.iter().any(Card::listening) => {
                let port = (value as u16) << 2 | 3;
                self.read_port = (0x203..=0x3FF).contains(&port).then_some(port);
            }
            // Config control: reset the logical devices, wait for the
            // key, forget the CSN.
            0x02 => {
                for card in self.cards.iter_mut().filter(|c| c.listening()) {
                    if value & 0x01 != 0 {
                        card.devices.iter_mut().for_each(|d| *d = Registers::default());
                    }
                    if value & 0x04 != 0 {
                        card.csn = 0;
                    }
                }
                if value & 0x02 != 0 {
                    for card in &mut self.cards {
                        card.state = CardState::WaitForKey;
                    }
                }
            }
            // Wake[CSN]: the card with it wakes, to isolation for CSN 0
            // and to configuration otherwise, from the start of its serial
            // identifier and resource data; the others sleep.
            0x03 => {
                for card in self.cards.iter_mut().filter(|c| c.listening()) {
                    if card.csn == value {
                        card.state = if value == 0 { CardState::Isolation } else { CardState::Config };
                        card.serial_bit = 0;
                        card.serial_second = false;
                        card.rom_pos = 0;
                    } else {
                        card.state = CardState::Sleep;
                    }
                }
            }
            // Card Select Number: the isolated card takes it and is
            // configured from now on.
            0x06 => {
                for card in &mut self.cards {
                    if matches!(card.state, CardState::Isolation | CardState::Config) {
                        card.csn = value;
                        card.state = CardState::Config;
                    }
                }
            }
            0x07 => {
                if let Some(card) = self.configured() {
                    card.logical = value;
                }
            }
            0x30.. => {
                if let Some(card) = self.configured() {
                    let logical = card.logical as usize;
                    card.set_device_register(logical, reg, value);
                }
            }
            _ => {}
        }
    }

    fn configured(&mut self) -> Option<&mut Card> {
        self.cards.iter_mut().find(|c| c.state == CardState::Config)
    }

    /// Whether a read of `port` is one of READ_DATA a card answers: one
    /// is isolated or configured.
    pub fn reads(&self, port: u16) -> bool {
        self.read_port == Some(port) && self.cards.iter().any(|c| matches!(c.state, CardState::Isolation | CardState::Config))
    }

    /// A read of READ_DATA, from the register ADDRESS selected.
    pub fn read_data(&mut self) -> u8 {
        match self.address {
            0x01 => self.read_serial(),
            reg => match self.cards.iter_mut().find(|c| c.state == CardState::Config) {
                None => 0xFF,
                Some(card) => match reg {
                    // Resource data, a byte at a time, always ready.
                    0x04 => {
                        let byte = card.rom[card.rom_pos as usize];
                        card.rom_pos = card.rom_pos.wrapping_add(1);
                        byte
                    }
                    0x05 => 0x01,
                    0x06 => card.csn,
                    0x07 => card.logical,
                    0x30.. => card.register(reg),
                    _ => 0x00,
                },
            },
        }
    }

    /// A read of the serial isolation register. Each bit of the serial
    /// identifiers takes two reads: the cards whose bit is 1 drive 55h,
    /// then AAh; a card whose bit is 0 while another's is 1 drops out of
    /// the isolation to sleep.
    fn read_serial(&mut self) -> u8 {
        let isolating = |c: &&mut Card| c.state == CardState::Isolation;
        let mut cards: Vec<&mut Card> = self.cards.iter_mut().filter(isolating).collect();
        let Some(second) = cards.first().map(|c| c.serial_second) else { return 0xFF };
        let one = cards.iter().any(|c| c.serial() == Some(true));
        for card in &mut cards {
            if second {
                if one && card.serial() == Some(false) {
                    card.state = CardState::Sleep;
                }
                card.serial_bit = (card.serial_bit + 1).min(72);
            }
            card.serial_second = !second;
        }
        match (one, second) {
            (true, false) => 0x55,
            (true, true) => 0xAA,
            _ => 0xFF,
        }
    }
}

/// The ViBRA16C's resource data, DOSBox-X's from a real card: the Plug
/// and Play version, the card's name, the audio device (CTL0001, which
/// Windows 95's Sound Blaster 16 driver knows) with its choices of
/// resources, best first, and the game port (CTL7001, compatible with
/// PNPB02F), and the end tag with its checksum.
fn vibra16c_resources() -> Vec<u8> {
    let mut data = Vec::new();
    let id = |data: &mut Vec<u8>, id: &[u8; 7]| data.extend(crate::pnpbios::eisa_id(id));
    let name = |data: &mut Vec<u8>, name: &str| {
        data.push(0x82);
        data.extend((name.len() as u16).to_le_bytes());
        data.extend(name.bytes());
    };
    let irq = |data: &mut Vec<u8>, irqs: &[u8]| {
        let mask = irqs.iter().fold(0u16, |m, &irq| m | 1 << irq);
        data.push(0x22);
        data.extend(mask.to_le_bytes());
    };
    // A DMA channel for 8-bit transfers in byte mode, or for 16-bit ones
    // in word mode, at ISA compatible speed.
    let dma = |data: &mut Vec<u8>, channels: &[u8], word: bool| {
        let mask = channels.iter().fold(0u8, |m, &ch| m | 1 << ch);
        data.extend([0x2A, mask, if word { 0x12 } else { 0x08 }]);
    };
    // I/O ports with 16-bit decode: lowest and highest base, alignment and
    // count.
    let io = |data: &mut Vec<u8>, min: u16, max: u16, align: u8, count: u8| {
        data.push(0x47);
        data.push(0x01);
        data.extend(min.to_le_bytes());
        data.extend(max.to_le_bytes());
        data.extend([align, count]);
    };
    // Start of a dependent function: 0 good, 1 acceptable, 2 sub-optimal.
    let start = |data: &mut Vec<u8>, priority: u8| data.extend([0x31, priority]);

    data.extend([0x0A, 0x10, 0x10]);
    name(&mut data, "Creative ViBRA16C PnP");
    data.push(0x15);
    id(&mut data, b"CTL0001");
    data.push(0x00);
    name(&mut data, "Audio");

    start(&mut data, 0);
    irq(&mut data, &[5]);
    dma(&mut data, &[1], false);
    dma(&mut data, &[5], true);
    io(&mut data, 0x220, 0x220, 0x01, 0x10);
    io(&mut data, 0x330, 0x330, 0x01, 0x02);
    io(&mut data, 0x388, 0x388, 0x01, 0x04);

    for (priority, dma16, mpu, opl) in
        [(1, true, true, true), (1, true, true, false), (2, true, false, false), (2, false, true, true), (2, false, false, false)]
    {
        start(&mut data, priority);
        irq(&mut data, &[5, 7, 9, 10]);
        dma(&mut data, &[1, 3], false);
        if dma16 {
            dma(&mut data, &[5, 7], true);
        }
        io(&mut data, 0x220, 0x280, 0x20, 0x10);
        if mpu {
            io(&mut data, 0x300, 0x330, 0x30, 0x02);
        }
        if opl {
            io(&mut data, 0x388, 0x388, 0x01, 0x04);
        }
    }
    data.push(0x38);

    data.push(0x15);
    id(&mut data, b"CTL7001");
    data.push(0x00);
    data.push(0x1C);
    id(&mut data, b"PNPB02F");
    name(&mut data, "Game");
    io(&mut data, 0x200, 0x200, 0x01, 0x08);

    let sum = data.iter().fold(0u8, |s, &b| s.wrapping_add(b));
    data.extend([0x79, 0u8.wrapping_sub(sum.wrapping_add(0x79))]);
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    fn send_key(pnp: &mut IsaPnp) {
        pnp.write_address(0);
        pnp.write_address(0);
        for k in KEY {
            pnp.write_address(k);
        }
    }

    fn write(pnp: &mut IsaPnp, reg: u8, value: u8) {
        pnp.write_address(reg);
        pnp.write_data(value)
    }

    fn read(pnp: &mut IsaPnp, reg: u8) -> u8 {
        pnp.write_address(reg);
        pnp.read_data()
    }

    fn vibra() -> IsaPnp {
        IsaPnp::vibra16c(&crate::sb::SbConfig { model: crate::sb::SbModel::Sb16Vibra, ..Default::default() })
    }

    #[test]
    fn the_key_is_the_specifications() {
        assert_eq!(&KEY[..4], &[0x6A, 0xB5, 0xDA, 0xED]);
        assert_eq!(&KEY[28..], &[0xCE, 0xE7, 0x73, 0x39]);
    }

    #[test]
    fn the_card_starts_configured_as_the_bios_leaves_it() {
        let pnp = vibra();
        assert_eq!(pnp.audio(), Some(Audio { base: 0x220, irq: Some(7), dma8: Some(1), dma16: Some(5) }));
        assert_eq!(pnp.csns(), 1);
        assert_eq!(pnp.read_port, Some(BIOS_READ_PORT));
        // Waiting for the key, the card answers nothing.
        assert!(!pnp.reads(BIOS_READ_PORT));
    }

    #[test]
    fn writes_before_the_key_are_ignored() {
        let mut pnp = vibra();
        write(&mut pnp, 0x02, 0x04);
        assert_eq!(pnp.cards[0].csn, 1);
        // A key with a wrong byte in it doesn't count.
        pnp.write_address(KEY[0]);
        pnp.write_address(KEY[1]);
        pnp.write_address(0x00);
        for k in &KEY[2..] {
            pnp.write_address(*k);
        }
        assert_eq!(pnp.cards[0].state, CardState::WaitForKey);
    }

    /// Isolation as an operating system runs it: reset the CSNs, wake
    /// the cards without one, set the read port and read 72 bit pairs.
    #[test]
    fn isolation_reads_the_serial_identifier_and_assigns_a_csn() {
        let mut pnp = vibra();
        send_key(&mut pnp);
        assert_eq!(pnp.cards[0].state, CardState::Sleep);
        write(&mut pnp, 0x02, 0x04);
        write(&mut pnp, 0x03, 0x00);
        write(&mut pnp, 0x00, 0x80);
        assert_eq!(pnp.read_port, Some(0x203));
        assert!(pnp.reads(0x203));
        pnp.write_address(0x01);
        let mut ident = [0u8; 9];
        for bit in 0..72 {
            let (a, b) = (pnp.read_data(), pnp.read_data());
            match (a, b) {
                (0x55, 0xAA) => ident[bit / 8] |= 1 << (bit % 8),
                (0xFF, 0xFF) => {}
                other => panic!("bit {}: {:?}", bit, other),
            }
        }
        assert_eq!(&ident[0..4], &crate::pnpbios::eisa_id(b"CTL0070"));
        assert_eq!(&ident[4..8], &[0xFF; 4]);
        assert_eq!(ident[8], checksum(&ident[0..8]));
        write(&mut pnp, 0x06, 0x01);
        assert_eq!(pnp.cards[0].state, CardState::Config);
        assert_eq!(read(&mut pnp, 0x06), 0x01);
        // Another isolation finds no card.
        write(&mut pnp, 0x03, 0x00);
        assert!(!pnp.reads(0x203));
    }

    #[test]
    fn the_resource_data_follows_the_serial_identifier() {
        let mut pnp = vibra();
        send_key(&mut pnp);
        write(&mut pnp, 0x03, 0x01);
        let mut rom = Vec::new();
        for _ in 0..256 {
            assert_eq!(read(&mut pnp, 0x05), 0x01);
            rom.push(read(&mut pnp, 0x04));
        }
        assert_eq!(&rom[0..4], &crate::pnpbios::eisa_id(b"CTL0070"));
        // The tags from the version on, to the end tag with its checksum.
        let mut at = 9;
        let mut ids = Vec::new();
        loop {
            let tag = rom[at];
            if tag & 0x80 != 0 {
                at += 3 + u16::from_le_bytes([rom[at + 1], rom[at + 2]]) as usize;
                continue;
            }
            let len = (tag & 7) as usize;
            if tag >> 3 == 0x02 || tag >> 3 == 0x03 {
                ids.push(rom[at + 1..at + 5].to_vec());
            }
            if tag >> 3 == 0x0F {
                let sum = rom[9..at + 2].iter().fold(0u8, |s, &b| s.wrapping_add(b));
                assert_eq!(sum, 0);
                break;
            }
            at += 1 + len;
        }
        let expect: Vec<Vec<u8>> = [b"CTL0001", b"CTL7001", b"PNPB02F"].iter().map(|id| crate::pnpbios::eisa_id(id).to_vec()).collect();
        assert_eq!(ids, expect);
        // The ROM repeats.
        assert_eq!(read(&mut pnp, 0x04), rom[0]);
    }

    #[test]
    fn configuring_the_audio_device_moves_the_sound_blaster() {
        let mut pnp = vibra();
        send_key(&mut pnp);
        write(&mut pnp, 0x03, 0x01);
        write(&mut pnp, 0x07, 0x00);
        write(&mut pnp, 0x30, 0x00);
        assert_eq!(pnp.audio(), None);
        write(&mut pnp, 0x60, 0x02);
        write(&mut pnp, 0x61, 0x40);
        write(&mut pnp, 0x70, 0x05);
        write(&mut pnp, 0x74, 0x03);
        write(&mut pnp, 0x75, 0x07);
        write(&mut pnp, 0x30, 0x01);
        assert_eq!(pnp.audio(), Some(Audio { base: 0x240, irq: Some(5), dma8: Some(3), dma16: Some(7) }));
        assert_eq!(read(&mut pnp, 0x70), 0x05);
        // The game port's registers are its own.
        write(&mut pnp, 0x07, 0x01);
        assert_eq!(read(&mut pnp, 0x61), 0x00);
        assert_eq!(read(&mut pnp, 0x60), 0x02);
        // Reset puts the logical devices at their power-up values.
        write(&mut pnp, 0x02, 0x01);
        assert_eq!(pnp.audio(), None);
        assert_eq!(pnp.cards[0].device_register(AUDIO, 0x74), 0x04);
        // Wait for Key: the card leaves the read port alone.
        write(&mut pnp, 0x02, 0x02);
        assert!(!pnp.reads(0x20B));
    }

    #[test]
    fn the_state_round_trips() {
        let mut pnp = vibra();
        send_key(&mut pnp);
        write(&mut pnp, 0x03, 0x01);
        write(&mut pnp, 0x70, 0x0A);
        let mut w = Writer::new();
        pnp.save(&mut w);
        let bytes = w.buf;
        let mut other = vibra();
        other.load(&mut Reader::new(&bytes)).unwrap();
        assert_eq!(other.audio().unwrap().irq, Some(10));
        assert_eq!(other.cards[0].state, CardState::Config);
        assert_eq!(other.address, 0x70);
    }
}
