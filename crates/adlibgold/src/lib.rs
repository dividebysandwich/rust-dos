//! The AdLib Gold 1000: a YMF262 (OPL3), a YMZ263 (MMA) for 12-bit
//! digital sound, a control chip with the card's mixer and settings in
//! EEPROM, a TDA8425 for the final volume, tone and stereo effect, and
//! the optional surround module's YM7128B.
//!
//! Its eight ports start at its base, 388h unless moved (control
//! register 15h moves them, the SDTK's "audio relocation"):
//!
//! - base + 0, 1: the OPL3's first register bank;
//! - base + 2, 3: the OPL3's second bank, or the control chip: writing
//!   FFh to base + 2 switches the two ports to it, FEh back to the OPL3.
//!   While switched, a write to base + 2 picks a control register, base
//!   + 3 reads and writes it, and a read of base + 2 gives the status;
//! - base + 4 to 7: the MMA (`mma`).
//!
//! The control chip's registers, from the AdLib Gold SDTK's register
//! reference:
//!
//! - 00h: reading gives the board, the model ID in bits 0-3 (1, the
//!   8-bit Gold 1000) and bits 4-6 set for the options not fitted
//!   (telephone, surround, CD-ROM); writing 1 restores every register
//!   from the EEPROM, 2 stores them there.
//! - 01h: the telephone; reading gives RING (bit 1, set while it doesn't
//!   ring) and the line state written to bit 0.
//! - 02h-03h: the sampling gains (recording, not emulated).
//! - 04h-08h: the TDA8425's volume, bass, treble and output mode
//!   (`tda8425`).
//! - 09h-10h: the mixer's levels of FM, sampled sound, aux, microphone
//!   and telephone, 80h silent to FFh full, below 80h the signal
//!   inverted.
//! - 11h: audio selection (filters, mono aux, PC speaker).
//! - 13h: AEN (bit 3) enables the interrupt, INT SEL (bits 0-2) picks
//!   IRQ 3, 4, 5, 7, 10, 11, 12 or 15; DEN0 (bit 7) and DMA SEL 0 (bits
//!   4-6) give sampling channel 0 its DMA channel.
//! - 14h: DEN1 and DMA SEL 1, for sampling channel 1.
//! - 15h: the base port divided by 8.
//! - 16h-17h: the SCSI interface's resources (no CD-ROM option here).
//! - 18h: the surround module's serial port (`ym7128b`).
//!
//! The status (a read of base + 2 while switched) has the interrupt
//! sources in bits 0-3, each clear while it asks for an interrupt (FM,
//! sampling, telephone, SCSI), and SB (6) and RB (7) while the chip is
//! busy with a write. Writes take effect at once here, so SB and RB stay
//! clear.

pub mod mma;
pub mod tda8425;
pub mod ym7128b;

use mma::{DmaLine, Mma};
use rust_dos_dma::Dma;
use tda8425::Tda8425;
use ym7128b::Ym7128b;

/// The control chip's registers, 00h to 18h.
pub const REGISTERS: usize = 0x19;

/// The surround module's output added to the dry sound: DOSBox Staging's
/// level, which matched recordings of the card.
const SURROUND_WET: f32 = 1.8;

/// The card's resources as installed (`adlibgold*` in `[sound]`): the
/// EEPROM's values at power-on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdlibGoldConfig {
    pub base: u16,
    pub irq: u8,
    pub dma: u8,
    /// Whether the surround module is fitted.
    pub surround: bool,
}

impl Default for AdlibGoldConfig {
    fn default() -> Self {
        Self {
            base: 0x388,
            irq: 7,
            dma: 1,
            surround: true,
        }
    }
}

/// The IRQs INT SEL picks.
pub const IRQS: [u8; 8] = [3, 4, 5, 7, 10, 11, 12, 15];

impl AdlibGoldConfig {
    /// The bases the card can take: a multiple of 8 among the ports an
    /// ISA card decodes.
    pub fn valid_base(base: u16) -> bool {
        (0x200..=0x3F8).contains(&base) && base & 7 == 0
    }

    /// The IRQs of the Gold 1000, an 8-bit card (the SDTK: "only IRQ 3,
    /// 4, 5 and 7 are available on 8-bit bus models").
    pub fn valid_irq(irq: u8) -> bool {
        matches!(irq, 3 | 4 | 5 | 7)
    }

    /// Its DMA channels: 1, 2 and 3 on 8-bit models, and 2 is the
    /// floppy's.
    pub fn valid_dma(dma: u8) -> bool {
        matches!(dma, 1 | 3)
    }

    /// The EEPROM as the card leaves the factory with these resources.
    /// The levels are neutral: the outputs at 0 dB, the tone flat, linear
    /// stereo, the mixer's FM, sampling and aux inputs full, the
    /// microphone and telephone (which aren't fitted) silent, and the
    /// surround off.
    pub fn eeprom(&self) -> [u8; REGISTERS] {
        let intsel = IRQS.iter().position(|&i| i == self.irq).unwrap_or(3) as u8;
        let mut e = [0u8; REGISTERS];
        e[0x02] = 0x80;
        e[0x03] = 0x80;
        e[0x04] = 0xFC;
        e[0x05] = 0xFC;
        e[0x06] = 0xF6;
        e[0x07] = 0xF6;
        e[0x08] = 0xCE;
        e[0x09..=0x0E].fill(0xFF);
        e[0x0F] = 0x80;
        e[0x10] = 0x80;
        e[0x13] = 0x80 | (self.dma & 7) << 4 | 0x08 | intsel;
        e[0x14] = 0x80 | (self.dma & 7) << 4;
        e[0x15] = (self.base >> 3) as u8 & 0x7F;
        // The SCSI interface's default, 340h.
        e[0x17] = 0x68;
        e
    }
}

/// A mixer level register's gain: 80h silent, FFh full, below 80h the
/// signal inverted.
pub fn mixer_level(value: u8) -> f32 {
    (value as f32 - 128.0) / 127.0
}

/// The card's analog chain for one source: surround, then the TDA8425.
/// The FM and the sampled sound each have one, so the host's mixer can
/// set their volumes apart; the chain is linear, so the two outputs sum
/// to what the card's one chain makes of the two inputs mixed.
struct Chain {
    surround: Option<Ym7128b>,
    tda: Tda8425,
}

impl Chain {
    fn new(regs: &[u8; REGISTERS], surround: bool) -> Self {
        Self {
            surround: surround.then(Ym7128b::new),
            tda: Tda8425::new(regs),
        }
    }

    fn process(&mut self, (l, r): (f32, f32)) -> (f32, f32) {
        let (mut l, mut r) = (l, r);
        if let Some(s) = &mut self.surround
            && !(s.is_silent() && s.at_rest())
        {
            let (wl, wr) = s.process(l + r);
            l += wl * SURROUND_WET;
            r += wr * SURROUND_WET;
        }
        self.tda.process((l, r))
    }

    fn at_rest(&self) -> bool {
        self.tda.at_rest()
            && self
                .surround
                .as_ref()
                .is_none_or(|s| s.is_silent() || s.at_rest())
    }
}

pub struct AdlibGold {
    config: AdlibGoldConfig,
    /// The ports' base: the configured one, or where register 15h moved
    /// it.
    base: u16,
    /// Base + 2 and 3 reach the control chip (FFh written to base + 2).
    control: bool,
    index: u8,
    regs: [u8; REGISTERS],
    eeprom: [u8; REGISTERS],
    pub mma: Mma,
    /// The DAC levels of the frame the mixer had last.
    pcm: (i16, i16),
    fm_chain: Chain,
    pcm_chain: Chain,
}

impl AdlibGold {
    /// The card at power-on, at PIT tick `now`: the registers from
    /// `eeprom` (the configured resources' factory EEPROM without one).
    pub fn new(config: AdlibGoldConfig, eeprom: Option<[u8; REGISTERS]>, now: u64) -> Self {
        let eeprom = eeprom.unwrap_or_else(|| config.eeprom());
        let regs = eeprom;
        let mut card = Self {
            config,
            base: config.base,
            control: false,
            index: 0,
            regs,
            eeprom,
            mma: Mma::new(now),
            pcm: (0, 0),
            fm_chain: Chain::new(&regs, config.surround),
            pcm_chain: Chain::new(&regs, config.surround),
        };
        card.restore();
        card
    }

    pub fn config(&self) -> AdlibGoldConfig {
        self.config
    }

    /// The EEPROM's contents, which outlast a reset of the card.
    pub fn eeprom(&self) -> [u8; REGISTERS] {
        self.eeprom
    }

    pub fn base(&self) -> u16 {
        self.base
    }

    /// Whether `port` is one of the card's eight.
    #[inline]
    pub fn claims(&self, port: u16) -> bool {
        port & !7 == self.base
    }

    /// Whether base + 2 and 3 reach the control chip.
    pub fn control_selected(&self) -> bool {
        self.control
    }

    pub fn registers(&self) -> &[u8; REGISTERS] {
        &self.regs
    }

    /// Load every register from the EEPROM, as at power-on and register
    /// 0's RT.
    fn restore(&mut self) {
        self.regs = self.eeprom;
        self.base = (self.regs[0x15] as u16 & 0x7F) << 3;
        for reg in 4..=8 {
            self.fm_chain.tda.write(reg, self.regs[reg as usize]);
            self.pcm_chain.tda.write(reg, self.regs[reg as usize]);
        }
    }

    /// A write to base + 2: true if the control chip took it, false if it
    /// goes to the OPL3's second bank.
    pub fn write_control_address(&mut self, value: u8) -> bool {
        match value {
            0xFF => self.control = true,
            0xFE if self.control => self.control = false,
            _ if self.control => self.index = value,
            _ => return false,
        }
        true
    }

    /// A write to base + 3: true if the control chip took it.
    pub fn write_control_data(&mut self, value: u8) -> bool {
        if !self.control {
            return false;
        }
        let reg = self.index as usize;
        if reg >= REGISTERS {
            return true;
        }
        match reg {
            0 => {
                if value & 2 != 0 {
                    self.eeprom = self.regs;
                }
                if value & 1 != 0 {
                    self.restore();
                }
                return true;
            }
            4..=8 => {
                self.fm_chain.tda.write(reg as u8, value);
                self.pcm_chain.tda.write(reg as u8, value);
            }
            0x15 => {
                // Below 200h are the motherboard's own devices (DMA, PIC,
                // timer); the card isn't moved there.
                let base = (value as u16 & 0x7F) << 3;
                if AdlibGoldConfig::valid_base(base) {
                    self.base = base;
                }
            }
            0x18 => {
                for chain in [&mut self.fm_chain, &mut self.pcm_chain] {
                    if let Some(s) = &mut chain.surround {
                        s.write_serial(value);
                    }
                }
            }
            _ => {}
        }
        self.regs[reg] = value;
        true
    }

    /// A read of base + 2 while the control chip is selected: the status,
    /// with the FM's interrupt (`fm_irq`, the OPL3's status bit 7) and the
    /// MMA's.
    pub fn read_control_status(&self, fm_irq: bool) -> Option<u8> {
        self.control
            .then(|| 0x0F & !(fm_irq as u8) & !((self.mma.irq_pending() as u8) << 1))
    }

    /// A read of base + 3 while the control chip is selected.
    pub fn read_control_data(&self) -> Option<u8> {
        if !self.control {
            return None;
        }
        Some(match self.index as usize {
            0 => {
                let options = 0x10 | 0x40 | if self.config.surround { 0 } else { 0x20 };
                options | 1
            }
            1 => 0x02 | self.regs[1] & 1,
            r if r < REGISTERS => self.regs[r],
            _ => 0xFF,
        })
    }

    /// The DMA channels of the two sampling channels.
    pub fn dma_lines(&self) -> [DmaLine; 2] {
        let line = |r: u8| DmaLine {
            enabled: r & 0x80 != 0,
            channel: r >> 4 & 7,
        };
        [line(self.regs[0x13]), line(self.regs[0x14])]
    }

    /// The IRQ the card asks for now, if it does: AEN is set, and the MMA
    /// or the FM chip (`fm_irq`, the OPL3's IRQ output) wants one.
    pub fn irq(&self, fm_irq: bool) -> Option<u8> {
        let r = self.regs[0x13];
        (r & 0x08 != 0 && (fm_irq || self.mma.irq_pending())).then_some(IRQS[(r & 7) as usize])
    }

    /// The IRQ line the card drives, raised or not.
    pub fn irq_line(&self) -> Option<u8> {
        let r = self.regs[0x13];
        (r & 0x08 != 0).then_some(IRQS[(r & 7) as usize])
    }

    /// The MMA's ports, base + 4 to 7.
    pub fn write_mma(&mut self, offset: u16, value: u8, now: u64, dma: &mut Dma, ram: &[u8]) {
        let lines = self.dma_lines();
        match offset & 7 {
            4 | 6 => self.mma.write_index(value),
            5 => self.mma.write_data(0, value, now, dma, ram, lines),
            7 => self.mma.write_data(1, value, now, dma, ram, lines),
            _ => {}
        }
    }

    pub fn read_mma(&mut self, offset: u16, now: u64, dma: &mut Dma, ram: &[u8]) -> u8 {
        let lines = self.dma_lines();
        match offset & 7 {
            4 | 6 => {
                self.mma.advance(now, dma, ram, lines);
                self.mma.read_status()
            }
            5 => self.mma.read_data(0, now, dma, ram, lines),
            7 => self.mma.read_data(1, now, dma, ram, lines),
            _ => 0xFF,
        }
    }

    /// Run the MMA up to PIT tick `now`.
    pub fn advance(&mut self, now: u64, dma: &mut Dma, ram: &[u8]) {
        let lines = self.dma_lines();
        self.mma.advance(now, dma, ram, lines);
    }

    /// When the card may next raise its interrupt, for the scheduler.
    pub fn next_event(&self, dma: &Dma) -> Option<u64> {
        self.irq_line()?;
        self.mma.next_event(dma, self.dma_lines())
    }

    /// One frame of output: the FM chip's frame `fm` and the MMA's next
    /// through the card's mixer and output stage, as (FM, sampled sound).
    pub fn render(&mut self, fm: (f32, f32)) -> ((f32, f32), (f32, f32)) {
        if let Some(f) = self.mma.out.pop_front() {
            self.pcm = f;
        }
        let r = &self.regs;
        let fm = (fm.0 * mixer_level(r[9]), fm.1 * mixer_level(r[0xA]));
        let pcm = (
            self.pcm.0 as f32 * mixer_level(r[0xB]),
            self.pcm.1 as f32 * mixer_level(r[0xC]),
        );
        let fm = self.fm_chain.process(fm);
        let pcm = if pcm == (0.0, 0.0) && self.pcm_chain.at_rest() {
            pcm
        } else {
            self.pcm_chain.process(pcm)
        };
        (fm, pcm)
    }

    /// Whether the sampled sound gives silence: the MMA idle and the
    /// chain run out.
    pub fn pcm_quiet(&self) -> bool {
        self.mma.is_idle()
            && self.pcm == (0, 0)
            && self.mma.out.iter().all(|&f| f == (0, 0))
            && self.pcm_chain.at_rest()
    }

    /// Whether the FM's chain has run out (nothing left in its filters or
    /// delay line).
    pub fn fm_chain_at_rest(&self) -> bool {
        self.fm_chain.at_rest()
    }

    /// Rebuild what isn't saved after a state was loaded.
    fn after_load(&mut self) {
        for chain in [&mut self.fm_chain, &mut self.pcm_chain] {
            chain.tda = Tda8425::new(&self.regs);
            if let Some(s) = &mut chain.surround {
                s.after_load();
            }
        }
        self.pcm = (0, 0);
    }
}

impl rust_dos_savestate::State for AdlibGold {
    fn save(&self, w: &mut rust_dos_savestate::Writer) {
        let AdlibGold {
            config: _,
            base,
            control,
            index,
            regs,
            eeprom,
            mma,
            pcm: _,
            fm_chain,
            pcm_chain,
        } = self;
        base.save(w);
        control.save(w);
        index.save(w);
        regs.save(w);
        eeprom.save(w);
        mma.save(w);
        for chain in [fm_chain, pcm_chain] {
            chain.surround.is_some().save(w);
            if let Some(s) = &chain.surround {
                s.save(w);
            }
        }
    }

    fn load(&mut self, r: &mut rust_dos_savestate::Reader) -> rust_dos_savestate::Result<()> {
        self.base.load(r)?;
        self.control.load(r)?;
        self.index.load(r)?;
        self.regs.load(r)?;
        self.eeprom.load(r)?;
        self.mma.load(r)?;
        for n in 0..2 {
            let mut fitted = false;
            fitted.load(r)?;
            if fitted != self.config.surround {
                return Err(rust_dos_savestate::StateError::Mismatch(
                    "the AdLib Gold's surround module differs".into(),
                ));
            }
            let chain = if n == 0 {
                &mut self.fm_chain
            } else {
                &mut self.pcm_chain
            };
            if let Some(s) = &mut chain.surround {
                s.load(r)?;
            }
        }
        self.after_load();
        Ok(())
    }
}

#[cfg(test)]
mod tests;
