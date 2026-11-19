//! The card as the AdLib Gold SDTK's drivers program it.

use super::*;
use rust_dos_audio_core::{PIT_HZ, RATE, mul_div_ceil};
use rust_dos_savestate::State;

/// A machine with the card at 388h and nothing else: the ports by
/// offset, at a PIT tick the test moves on.
struct Rig {
    card: AdlibGold,
    dma: Dma,
    ram: Vec<u8>,
    now: u64,
}

impl Rig {
    fn new(config: AdlibGoldConfig) -> Self {
        Self {
            card: AdlibGold::new(config, None, 0),
            dma: Dma::new(),
            ram: vec![0; 0x20000],
            now: 0,
        }
    }

    fn out(&mut self, offset: u16, value: u8) {
        match offset {
            2 => assert!(
                self.card.write_control_address(value),
                "the control chip took {value:02X}"
            ),
            3 => assert!(self.card.write_control_data(value)),
            4..=7 => self
                .card
                .write_mma(offset, value, self.now, &mut self.dma, &self.ram),
            _ => unreachable!(),
        }
    }

    fn inp(&mut self, offset: u16) -> u8 {
        match offset {
            2 => self.card.read_control_status(false).unwrap(),
            3 => self.card.read_control_data().unwrap(),
            4..=7 => self
                .card
                .read_mma(offset, self.now, &mut self.dma, &self.ram),
            _ => unreachable!(),
        }
    }

    /// The SDTK's `SetControlRegister`.
    fn set_control(&mut self, reg: u8, value: u8) {
        self.out(2, 0xFF);
        self.out(2, reg);
        self.out(3, value);
        assert_eq!(self.inp(2) & 0xC0, 0, "SB and RB clear");
        self.out(2, 0xFE);
    }

    /// The SDTK's `GetControlRegister`.
    fn get_control(&mut self, reg: u8) -> u8 {
        self.out(2, 0xFF);
        self.out(2, reg);
        let v = self.inp(3);
        self.out(2, 0xFE);
        v
    }

    fn mma(&mut self, ch: usize, reg: u8, value: u8) {
        self.out(4, reg);
        self.out(if ch == 0 { 5 } else { 7 }, value);
    }

    /// Run to the start of mixer frame `frame`.
    fn run_to_frame(&mut self, frame: u64) {
        self.now = mul_div_ceil(frame, PIT_HZ, RATE as u64);
        self.card.advance(self.now, &mut self.dma, &self.ram);
    }

    /// Program 8237 channel `ch` (0-3) to read `len` bytes at `addr`.
    fn dma_read(&mut self, ch: u8, addr: u32, len: u16, auto_init: bool) {
        let d = &mut self.dma;
        d.write(0x0A, 4 | ch);
        d.write(0x0B, 0x48 | if auto_init { 0x10 } else { 0 } | ch);
        d.write(0x0C, 0);
        let (a, c) = (ch as u16 * 2, ch as u16 * 2 + 1);
        d.write(a, addr as u8);
        d.write(a, (addr >> 8) as u8);
        d.write(c, (len - 1) as u8);
        d.write(c, ((len - 1) >> 8) as u8);
        let page = [0x87, 0x83, 0x81, 0x82][ch as usize];
        d.write(page, (addr >> 16) as u8);
        d.write(0x0A, ch);
    }
}

/// `CtGetGoldCardPresence`: the MMA's register 0Bh reads back what was
/// written, nothing answers at base + 8, and the control chip's register
/// 9 holds 54h, so the card is a level 2 card with the control chip as
/// phantom registers at base + 2.
#[test]
fn sdtk_detection_finds_a_level_2_card() {
    let mut rig = Rig::new(AdlibGoldConfig::default());
    rig.out(4, 0x0B);
    let prev = rig.inp(5);
    rig.out(5, 0x5A);
    assert_eq!(rig.inp(5), 0x5A);
    rig.out(5, prev);
    assert!(!rig.card.claims(0x390));
    rig.out(2, 0xFF);
    rig.out(2, 0x09);
    let prev = rig.inp(3);
    rig.out(3, 0x54);
    assert_eq!(rig.inp(3) & 0x7C, 0x54);
    rig.out(3, prev);
    rig.out(2, 0xFE);
    assert!(!rig.card.control_selected());
    assert!(
        !rig.card.write_control_address(0x05),
        "the OPL3's second bank again"
    );
}

/// Register 0 reads the board: model 1 and the options fitted, which
/// `CtGetBoardOptions` reads inverted from bits 4-6.
#[test]
fn board_id_and_options() {
    let mut rig = Rig::new(AdlibGoldConfig::default());
    let id = rig.get_control(0);
    assert_eq!(id, 0x51);
    assert_eq!(id & 0x0F, 1, "Gold 1000");
    assert_eq!(!((id & 0x70) >> 4) & 7, 0b010, "surround only");
    let mut rig = Rig::new(AdlibGoldConfig {
        surround: false,
        ..Default::default()
    });
    assert_eq!(rig.get_control(0), 0x71);
    // Register 1: not ringing, the line as set.
    assert_eq!(rig.get_control(1), 0x02);
    rig.set_control(1, 1);
    assert_eq!(rig.get_control(1), 0x03);
}

/// The EEPROM: ST stores the registers, RT brings them back, and a new
/// card made from the EEPROM starts with them.
#[test]
fn eeprom_stores_and_restores() {
    let mut rig = Rig::new(AdlibGoldConfig::default());
    assert_eq!(rig.get_control(0x09), 0xFF);
    rig.set_control(0x09, 0xC0);
    rig.set_control(0x00, 0x02);
    rig.set_control(0x09, 0x90);
    assert_eq!(rig.get_control(0x09), 0x90);
    rig.set_control(0x00, 0x01);
    assert_eq!(rig.get_control(0x09), 0xC0);
    let card = AdlibGold::new(AdlibGoldConfig::default(), Some(rig.card.eeprom()), 0);
    assert_eq!(card.registers()[0x09], 0xC0);
}

/// The factory EEPROM holds the configured resources, and register 15h
/// moves the card.
#[test]
fn resources_and_relocation() {
    let config = AdlibGoldConfig {
        base: 0x380,
        irq: 5,
        dma: 3,
        surround: true,
    };
    let mut rig = Rig::new(config);
    assert!(rig.card.claims(0x387) && !rig.card.claims(0x388));
    assert_eq!(rig.get_control(0x15), (0x380u16 >> 3) as u8);
    assert_eq!(rig.get_control(0x13), 0x80 | 3 << 4 | 0x08 | 2);
    assert_eq!(rig.card.irq_line(), Some(5));
    assert_eq!(
        rig.card.dma_lines()[0],
        DmaLine {
            enabled: true,
            channel: 3
        }
    );
    rig.set_control(0x15, (0x388u16 >> 3) as u8);
    assert!(rig.card.claims(0x388) && !rig.card.claims(0x380));
}

/// The SDTK's playback by CPU: bytes written to register 0Bh play at the
/// channel's rate on the outputs its L and R bits pick, and the FIFO flag
/// rises, with the interrupt, once the FIFO falls below its level.
#[test]
fn cpu_fed_playback_and_fifo_interrupt() {
    let mut rig = Rig::new(AdlibGoldConfig::default());
    rig.mma(0, 0x09, 0x80);
    rig.mma(0, 0x09, 0x00);
    // 8-bit PCM, FIFO INT 5 (32 bytes), interrupt unmasked, no DMA.
    rig.mma(0, 0x0C, 5 << 2);
    rig.mma(0, 0x0A, 0xFF);
    for n in 0..64u8 {
        rig.mma(0, 0x0B, n);
    }
    // Left only, 11.025 kHz PCM, play, go: a sample every 4 frames.
    rig.mma(0, 0x09, 0x20 | 2 << 3 | 0x04 | 0x02 | 0x01);
    let start = rig.card.mma.out.len() as u64;
    assert_eq!(start, 0);
    rig.run_to_frame(4 * 10);
    let out: Vec<(i16, i16)> = rig.card.mma.out.drain(..).collect();
    assert_eq!(out[3], (0, 0));
    assert_eq!(out[4 * 3 + 3], (3 << 8, 0));
    assert_eq!(rig.card.mma.status() & 1, 0);
    assert_eq!(rig.card.irq(), None);
    // 33 samples leave 31 bytes, below 32: the flag and the interrupt.
    let next = rig.card.next_event(&rig.dma).expect("an interrupt to come");
    rig.run_to_frame(4 * 32);
    assert_eq!(rig.card.irq(), None);
    rig.run_to_frame(4 * 33);
    assert!(rig.now >= next, "{} {}", rig.now, next);
    assert_eq!(rig.card.irq(), Some(7));
    rig.out(2, 0xFF);
    assert_eq!(
        rig.inp(2) & 0x02,
        0,
        "the control chip's status shows the sampling's interrupt"
    );
    rig.out(2, 0xFE);
    assert_eq!(rig.inp(4) & 1, 1);
    assert_eq!(rig.card.irq(), None, "the status read clears it");
    // Running dry: the level holds and OV rises.
    rig.run_to_frame(4 * 70);
    assert_eq!(rig.inp(4) & 0x80, 0x80);
}

/// DMA playback, as Jungle Book's sound driver runs it: 8-bit PCM at
/// 11.025 kHz from an auto-init DMA buffer, the DMA's count moving as the
/// channel plays.
#[test]
fn dma_playback_moves_the_dma_count() {
    let mut rig = Rig::new(AdlibGoldConfig::default());
    for (n, b) in rig.ram[0x10000..0x10400].iter_mut().enumerate() {
        *b = n as u8;
    }
    rig.dma_read(1, 0x10000, 0x400, true);
    rig.mma(0, 0x09, 0x80);
    rig.mma(0, 0x09, 0x00);
    rig.mma(0, 0x0C, 0x01);
    rig.mma(0, 0x0A, 0xFF);
    rig.mma(0, 0x09, 0x36);
    assert_eq!(
        rig.dma.channel(1).cur_count,
        0x3FF - 128,
        "the FIFO filled at once"
    );
    rig.mma(0, 0x09, 0x37);
    rig.run_to_frame(4 * 100);
    // 100 samples played, each topped up from the DMA.
    assert_eq!(rig.dma.channel(1).cur_count, 0x3FF - 128 - 100);
    let out: Vec<(i16, i16)> = rig.card.mma.out.drain(..).collect();
    assert_eq!(out[4 * 99 + 3], (99 << 8, 0));
    // Auto-init: no FIFO interrupt to schedule.
    assert_eq!(rig.card.next_event(&rig.dma), None);
}

/// Interleaved stereo: channel 0's DMA alternates samples between the
/// channels, which play on channel 0's settings.
#[test]
fn interleaved_dma_feeds_both_channels() {
    let mut rig = Rig::new(AdlibGoldConfig::default());
    for (n, b) in rig.ram[0x10000..0x10100].iter_mut().enumerate() {
        *b = if n % 2 == 0 { 0x10 } else { 0xF0 };
    }
    rig.dma_read(1, 0x10000, 0x100, false);
    rig.mma(0, 0x0A, 0xFF);
    rig.mma(1, 0x0A, 0xFF);
    rig.mma(1, 0x09, 0x40);
    rig.mma(1, 0x0C, 0x01);
    rig.mma(0, 0x0C, 0x81);
    rig.mma(0, 0x09, 0x20 | 0x04 | 0x02 | 0x01);
    rig.run_to_frame(8);
    let out: Vec<(i16, i16)> = rig.card.mma.out.drain(..).collect();
    assert_eq!(out[7], (0x1000, -0x1000));
}

/// The MMA's timer 0 counts 1.89 us ticks and raises T0.
#[test]
fn timer0_runs_out() {
    let mut rig = Rig::new(AdlibGoldConfig::default());
    // 1000 ticks: 1.89 ms.
    rig.mma(0, 0x02, 0xE8);
    rig.mma(0, 0x03, 0x03);
    rig.mma(0, 0x08, 0x01);
    let due = rig.card.next_event(&rig.dma).unwrap();
    assert_eq!(due, mul_div_ceil(1000, PIT_HZ, mma::CLOCK_HZ));
    rig.now = due - 2;
    assert_eq!(rig.inp(4) & 0x10, 0);
    rig.now = due;
    assert_eq!(rig.inp(4) & 0x10, 0x10);
    assert_eq!(rig.inp(4) & 0x10, 0, "cleared by the read");
}

/// The output stage: FM at the mixer's full level and the TDA8425 flat
/// passes the FM as it is; muting (register 8 bit 5) silences it.
#[test]
fn neutral_settings_pass_fm_and_mute_silences() {
    let mut rig = Rig::new(AdlibGoldConfig::default());
    for n in 0..100 {
        let x = (n as f32 * 0.3).sin() * 8000.0;
        let ((l, r), _) = rig.card.render((x, x / 2.0));
        assert!(
            (l - x).abs() < 0.5 && (r - x / 2.0).abs() < 0.5,
            "{n}: {l} {r}"
        );
    }
    rig.set_control(8, 0xEE);
    assert_eq!(rig.card.render((1000.0, 1000.0)).0, (0.0, 0.0));
    rig.set_control(8, 0xCE);
    rig.set_control(9, 0x80);
    assert_eq!(rig.card.render((1000.0, 1000.0)).0.0, 0.0);
}

/// The surround module, programmed over register 18h's serial lines,
/// adds a delayed copy of the input.
#[test]
fn surround_adds_an_echo() {
    let mut rig = Rig::new(AdlibGoldConfig::default());
    let send = |rig: &mut Rig, address: u8, value: u8| {
        for bit in (0..5).rev() {
            let d = address >> bit & 1;
            rig.set_control(0x18, d);
            rig.set_control(0x18, d | 2);
        }
        for bit in (0..6).rev() {
            let d = value >> bit & 1;
            rig.set_control(0x18, 4 | d);
            rig.set_control(0x18, 4 | d | 2);
        }
        rig.set_control(0x18, 0);
    };
    send(&mut rig, 0x10, 0x3F);
    send(&mut rig, 0x12, 0x3F);
    send(&mut rig, 0x13, 0x3F);
    send(&mut rig, 0x17, 10);
    send(&mut rig, 0x00, 0x3F);
    send(&mut rig, 0x08, 0x3F);
    let delay = ym7128b::tap(10);
    let mut first = None;
    for n in 0..delay + 10 {
        let x = if n == 0 { 1000.0 } else { 0.0 };
        let ((l, _), _) = rig.card.render((x, x));
        if n > 0 && l != 0.0 && first.is_none() {
            first = Some((n, l));
        }
    }
    let (n, l) = first.expect("an echo");
    assert_eq!(n, delay);
    assert!((l - 1000.0 * SURROUND_WET).abs() < 1.0, "{l}");
}

/// A saved card loads back into one of the same make.
#[test]
fn state_round_trip() {
    let mut rig = Rig::new(AdlibGoldConfig::default());
    rig.set_control(0x09, 0xA0);
    rig.mma(0, 0x0B, 0x12);
    let mut w = rust_dos_savestate::Writer::new();
    rig.card.save(&mut w);
    let mut other = AdlibGold::new(AdlibGoldConfig::default(), None, 0);
    other
        .load(&mut rust_dos_savestate::Reader::new(&w.buf))
        .unwrap();
    assert_eq!(other.registers()[0x09], 0xA0);
    assert_eq!(other.mma.channels[0].fifo_len(), 1);
    let mut plain = AdlibGold::new(
        AdlibGoldConfig {
            surround: false,
            ..Default::default()
        },
        None,
        0,
    );
    assert!(
        plain
            .load(&mut rust_dos_savestate::Reader::new(&w.buf))
            .is_err()
    );
}
