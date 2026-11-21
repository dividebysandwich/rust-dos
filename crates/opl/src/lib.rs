//! The FM synthesizer: a Yamaha YMF262 (OPL3), as on the SB Pro 2 and SB16,
//! its OPL2 subset, as on an AdLib or SB 2.0, or two OPL2s, left and
//! right, as on the first SB Pro (CT1330). Synthesis is Nuked-OPL3 (the
//! `nuked-opl3` crate); this module adds the two timers and the status
//! register that programs use to detect the chip, which Nuked-OPL3 leaves
//! out.
//!
//! Register writes take effect at their emulated time: the audio mixer
//! renders up to the present before each write (see `Bus::audio_catch_up`).

use nuked_opl3::Opl3Chip;
pub use nuked_opl3::ChannelView;

/// Output sample rate of the mixer, which the chip renders at.
pub use rust_dos_audio_core::RATE;

/// The FM chips a card has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chip {
    /// One OPL2 (YM3812): an AdLib, a Sound Blaster 1.x or 2.0.
    Opl2,
    /// One OPL3 (YMF262): an SB Pro 2, an SB16.
    Opl3,
    /// Two OPL2s, the left one at register bank 0 and the right one at
    /// bank 1: the first Sound Blaster Pro (CT1330), DOSBox's
    /// `oplmode=dualopl2`.
    DualOpl2,
}

impl Chip {
    /// The name the `opl` setting gives it.
    pub fn name(self) -> &'static str {
        match self {
            Chip::Opl2 => "opl2",
            Chip::Opl3 => "opl3",
            Chip::DualOpl2 => "dualopl2",
        }
    }
}

/// An OPL3 for `true`, an OPL2 for `false`.
impl From<bool> for Chip {
    fn from(opl3: bool) -> Self {
        if opl3 { Chip::Opl3 } else { Chip::Opl2 }
    }
}

pub struct Opl {
    chip: Opl3Chip,
    /// OPL3 (two register banks, status without the OPL2's 06h bits).
    opl3: bool,
    /// Register address latched for each bank (0 and 1).
    address: [u8; 2],
    timer_value: [u8; 2],
    /// Emulated microseconds each timer was started at, if running.
    timer_start: [Option<u64>; 2],
    timer_mask: [bool; 2],
    timer_expired: [bool; 2],
    /// Whether a key may be sounding: set by the writes that key notes on
    /// or off, and cleared once every operator has been released to
    /// silence, so a silent chip skips synthesis.
    active: bool,
    /// Frames rendered since `render` last checked whether the chip has
    /// gone quiet.
    unchecked_frames: u32,
    /// The chip's registers as written, for a save state: the chip can't
    /// be saved, so a loaded state writes them to a new one (`after_load`).
    regs: [[u8; 256]; 2],
    /// The channels left out of the mix (the sound visualizer's), a bit
    /// each.
    mute: u32,
    /// The right OPL2 of a dual OPL2, which takes what is written to
    /// bank 1. This one is then the left OPL2.
    right: Option<Box<Opl>>,
}

impl Opl {
    /// Whether the chip is silent: no key written since it was reset, or
    /// every operator released to silence since (`render` then gives
    /// silence without running it).
    #[inline]
    pub fn is_idle(&self) -> bool {
        !self.active && self.right.as_ref().is_none_or(|r| r.is_idle())
    }

    /// The chips `chip` names.
    pub fn with_chip(chip: Chip) -> Self {
        match chip {
            Chip::Opl2 => Self::new(false),
            Chip::Opl3 => Self::new(true),
            Chip::DualOpl2 => Self {
                right: Some(Box::new(Self::new(false))),
                ..Self::new(false)
            },
        }
    }

    pub fn chip(&self) -> Chip {
        match (self.opl3, self.right.is_some()) {
            (true, _) => Chip::Opl3,
            (false, false) => Chip::Opl2,
            (false, true) => Chip::DualOpl2,
        }
    }

    pub fn new(opl3: bool) -> Self {
        Self {
            chip: Opl3Chip::new(RATE),
            opl3,
            address: [0; 2],
            timer_value: [0; 2],
            timer_start: [None; 2],
            timer_mask: [false; 2],
            timer_expired: [false; 2],
            active: false,
            unchecked_frames: 0,
            regs: [[0; 256]; 2],
            mute: 0,
            right: None,
        }
    }

    #[inline]
    pub fn is_opl3(&self) -> bool {
        self.opl3
    }

    /// Latch a register address in `bank` (1 is the OPL3's second bank,
    /// or the right OPL2 of a dual OPL2).
    #[inline]
    pub fn write_address(&mut self, bank: usize, value: u8) {
        if bank == 1
            && let Some(right) = &mut self.right
        {
            right.write_address(0, value);
        } else if bank == 0 || self.opl3 {
            self.address[bank] = value;
        }
    }

    /// Latch a register address through an AdLib port (388h, or a Sound
    /// Blaster's base+8): the first bank's, and on a dual OPL2 each
    /// chip's, as the card writes both there.
    #[inline]
    pub fn write_address_all(&mut self, value: u8) {
        self.write_address(0, value);
        if let Some(right) = &mut self.right {
            right.write_address(0, value);
        }
    }

    /// Write the latched register through an AdLib port (389h, or a
    /// Sound Blaster's base+9): the first bank's, and on a dual OPL2 each
    /// chip's.
    pub fn write_data_all(&mut self, value: u8, now_us: u64) {
        self.write_data(0, value, now_us);
        if let Some(right) = &mut self.right {
            right.write_data(0, value, now_us);
        }
    }

    /// Write the latched register of `bank`. `now_us` is emulated time.
    pub fn write_data(&mut self, bank: usize, value: u8, now_us: u64) {
        if bank == 1
            && let Some(right) = &mut self.right
        {
            right.write_data(0, value, now_us);
            return;
        }
        if bank == 1 && !self.opl3 {
            return;
        }
        let reg = self.address[bank];
        if bank == 0 {
            match reg {
                0x02 | 0x03 => {
                    self.set_timer_value((reg - 2) as usize, value, now_us);
                    return;
                }
                0x04 => {
                    self.timer_control(value, now_us);
                    return;
                }
                _ => {}
            }
        }
        if (0xA0..=0xB8).contains(&reg) || reg == 0xBD {
            self.active = true;
        }
        self.regs[bank][reg as usize] = value;
        self.chip.write_register((bank as u16) << 8 | reg as u16, value);
    }

    /// A new chip with the registers of the loaded state: the OPL3 mode
    /// and four-operator connections first, as they change what the
    /// other registers mean, then the rest, the frequencies and key-ons
    /// and the rhythm register last, so the notes playing start again
    /// with their instruments.
    pub fn after_load(&mut self) {
        if let Some(right) = &mut self.right {
            right.after_load();
        }
        self.chip = Opl3Chip::new(RATE);
        self.chip.set_mute(self.mute);
        let key_part = |reg: usize| (0xA0..=0xB8).contains(&reg) || reg == 0xBD;
        let mut order = Vec::with_capacity(512);
        if self.opl3 {
            order.extend([(1, 0x05), (1, 0x04)]);
        }
        for bank in 0..2 {
            for reg in 0..256 {
                let timer = bank == 0 && (0x02..=0x04).contains(&reg);
                let mode = bank == 1 && matches!(reg, 0x04 | 0x05);
                if !timer && !mode && !key_part(reg) {
                    order.push((bank, reg));
                }
            }
        }
        for bank in 0..2 {
            order.extend((0xA0..=0xA8).chain(0xB0..=0xB8).map(|reg| (bank, reg)));
        }
        order.push((0, 0xBD));
        // A new chip's registers are all 0, as are those never written
        // (some of which it has no slot for).
        for (bank, reg) in order {
            let value = self.regs[bank][reg];
            if value != 0 && (bank == 0 || self.opl3) {
                self.chip.write_register((bank as u16) << 8 | reg as u16, value);
            }
        }
    }

    // The flags go up when the status is read, or before a write changes
    // the timers (`update_flags`): a timer's flag is up if it overflowed
    // since `timer_start`, which moves to the last overflow whenever the
    // flag is cleared. As on the chip (MAME's ymfm and fmopl), a masked
    // timer raises its flag too, and the status read leaves it out.

    fn timer_control(&mut self, value: u8, now_us: u64) {
        self.update_flags(now_us);
        if value & 0x80 != 0 {
            // Reset the timer flags. A running timer raises its flag
            // again at its next overflow.
            for i in 0..2 {
                self.clear_flag(i, now_us);
            }
            return;
        }
        self.timer_mask = [value & 0x40 != 0, value & 0x20 != 0];
        for i in 0..2 {
            if self.timer_mask[i] {
                // Setting a timer's mask bit clears its flag, as on the
                // YM3812 (MAME's ymfm and fmopl clear the status bits that
                // a write to 04h masks, and DOSBox does the same). Titus
                // the Fox and Moktar detect the AdLib by it: they mask
                // timer 1 after it ran out, and wait for status A0h, the
                // flag of timer 2 alone.
                self.clear_flag(i, now_us);
            }
        }
        for (i, bit) in [0x01u8, 0x02].into_iter().enumerate() {
            let start = value & bit != 0;
            if start && self.timer_start[i].is_none() {
                self.timer_start[i] = Some(now_us);
                self.timer_expired[i] = false;
            } else if !start {
                // A stopped timer keeps its flag.
                self.timer_start[i] = None;
            }
        }
    }

    /// Timer `i`'s period in microseconds: timer 1 counts 80 us steps,
    /// timer 2 320 us steps, up from its value to 256, and then starts
    /// again from the value.
    fn timer_period(&self, i: usize) -> u64 {
        (256 - self.timer_value[i] as u64) * [80, 320][i]
    }

    /// Raise the flag of each running timer that has overflowed since
    /// its `timer_start`.
    fn update_flags(&mut self, now_us: u64) {
        for i in 0..2 {
            if let Some(start) = self.timer_start[i]
                && now_us.saturating_sub(start) >= self.timer_period(i)
            {
                self.timer_expired[i] = true;
            }
        }
    }

    /// Clear timer `i`'s flag, and count a running timer from its last
    /// overflow, so that the flag goes up again at its next one.
    fn clear_flag(&mut self, i: usize, now_us: u64) {
        self.timer_expired[i] = false;
        if let Some(start) = self.timer_start[i] {
            let period = self.timer_period(i);
            self.timer_start[i] = Some(start + now_us.saturating_sub(start) / period * period);
        }
    }

    /// Write timer `i`'s value (register 02h or 03h). A running timer
    /// loads it at its next overflow (MAME's ymfm and fmopl): the count
    /// under way ends when it would have.
    fn set_timer_value(&mut self, i: usize, value: u8, now_us: u64) {
        self.update_flags(now_us);
        let Some(start) = self.timer_start[i] else {
            self.timer_value[i] = value;
            return;
        };
        let period = self.timer_period(i);
        let next = start + (now_us.saturating_sub(start) / period + 1) * period;
        self.timer_value[i] = value;
        // `timer_start` can't go below 0: in the first 82 ms of emulated
        // time, the next overflow may come up to a period late.
        self.timer_start[i] = Some(next.saturating_sub(self.timer_period(i)));
    }

    /// Whether the chip's IRQ output is up at `now_us`: the flag of an
    /// unmasked timer, which stays up until register 04h resets it. The
    /// status's bit 7 shows the same; the AdLib Gold puts it on its
    /// interrupt line.
    pub fn irq_at(&self, now_us: u64) -> bool {
        let own = (0..2).any(|i| {
            !self.timer_mask[i]
                && (self.timer_expired[i]
                    || self.timer_start[i]
                        .is_some_and(|start| now_us.saturating_sub(start) >= self.timer_period(i)))
        });
        own || self.right.as_ref().is_some_and(|r| r.irq_at(now_us))
    }

    /// When after `now_us` an unmasked running timer raises its flag, in
    /// emulated microseconds, if one will. A flag already up stays up
    /// until register 04h resets it, which reschedules.
    pub fn next_irq_us(&self, now_us: u64) -> Option<u64> {
        let right = self.right.as_ref().and_then(|r| r.next_irq_us(now_us));
        (0..2)
            .filter(|&i| !self.timer_mask[i] && !self.timer_expired[i])
            .filter_map(|i| self.timer_start[i].map(|start| start + self.timer_period(i)))
            .filter(|&at| at > now_us)
            .chain(right)
            .min()
    }

    /// The status register read at the address port of `bank`: on a dual
    /// OPL2 the right chip's for bank 1 (a Sound Blaster's base+2), else
    /// the one status (`read_status`).
    pub fn read_status_of(&mut self, bank: usize, now_us: u64) -> u8 {
        match &mut self.right {
            Some(right) if bank == 1 => right.read_status(now_us),
            _ => self.read_status(now_us),
        }
    }

    /// The status register: IRQ (bit 7) and the timer flags (6, 5) that
    /// aren't masked. An OPL2 also reads 06h in the low bits, which is
    /// how programs tell the two apart. On a dual OPL2 this is the left
    /// chip's, which the AdLib ports (388h, base+8) read, as in DOSBox and
    /// 86Box.
    pub fn read_status(&mut self, now_us: u64) -> u8 {
        self.update_flags(now_us);
        let mut status = 0;
        if self.timer_expired[0] && !self.timer_mask[0] {
            status |= 0x40;
        }
        if self.timer_expired[1] && !self.timer_mask[1] {
            status |= 0x20;
        }
        if status != 0 {
            status |= 0x80;
        }
        if self.opl3 { status } else { status | 0x06 }
    }

    /// Leave the channels whose bits are set (0 to 17) out of the mix. On
    /// a dual OPL2, 0 to 8 are the left chip's and 9 to 17 the right's.
    pub fn set_mute(&mut self, mask: u32) {
        let own = if self.right.is_some() {
            mask & 0x1FF
        } else {
            mask
        };
        self.mute = own;
        self.chip.set_mute(own);
        if let Some(right) = &mut self.right {
            right.set_mute(mask >> 9 & 0x1FF);
        }
    }

    pub fn mute(&self) -> u32 {
        self.mute | self.right.as_ref().map_or(0, |r| r.mute() << 9)
    }

    /// What the 18 channels play: the first 9 on an OPL2, and on a dual
    /// OPL2 the left chip's 9 and then the right chip's.
    pub fn channels(&self) -> [ChannelView; 18] {
        let mut views = self.chip.channel_views();
        if let Some(right) = &self.right {
            views[9..].copy_from_slice(&right.chip.channel_views()[..9]);
        }
        views
    }

    /// Render one stereo frame at `RATE`. A dual OPL2's left chip plays
    /// on the left output only and its right chip on the right only, as
    /// the CT1330 wires them.
    #[inline]
    pub fn render(&mut self) -> (i16, i16) {
        let own = self.render_chip();
        match &mut self.right {
            None => own,
            Some(right) => (own.0, right.render_chip().1),
        }
    }

    /// One frame of this chip alone.
    #[inline]
    fn render_chip(&mut self) -> (i16, i16) {
        if !self.active {
            return (0, 0);
        }
        let mut frame = [0i16; 2];
        let _ = self.chip.generate_resampled(&mut frame);
        // Now and then, check whether every operator is keyed off and
        // released to full attenuation. Such a chip stays silent until a
        // key is written (which sets `active` again), whatever else
        // changes: a level or a rate applies to the next note. Its
        // operators' output is then only their sine's sign bit, a pattern
        // of 0 and -1 a few steps high at most, which `render` turns into
        // 0. The chip's clock stops meanwhile, so the vibrato and tremolo
        // go on from where they were rather than where a running chip's
        // would be, which no program can read and nobody hears.
        self.unchecked_frames += 1;
        if self.unchecked_frames == IDLE_CHECK_FRAMES {
            self.unchecked_frames = 0;
            if self.chip.active_voice_count() == 0 {
                self.active = false;
            }
        }
        (frame[0], frame[1])
    }
}

/// How often, in frames, `render` checks whether the chip has gone quiet.
const IDLE_CHECK_FRAMES: u32 = 64;

// Whether it is an OPL3 comes with the Sound Blaster's model; the chip is
// written anew from `regs` after a load.
// A dual OPL2's right chip is saved apart (`save_right`), so that the
// layout of the one chip stays as it was.
rust_dos_savestate::state_fields!(Opl { address, timer_value, timer_start, timer_mask, timer_expired, active, regs } skip { chip, opl3, mute, unchecked_frames, right });

impl Opl {
    /// Save the right chip of a dual OPL2, if there is one.
    pub fn save_right(&self, w: &mut rust_dos_savestate::Writer) {
        use rust_dos_savestate::State;
        self.right.is_some().save(w);
        if let Some(right) = &self.right {
            right.save(w);
        }
    }

    /// For a state saved before dual OPL2s were emulated, when the one chip
    /// (an SB Pro 1's OPL3) took the right OPL2's writes (base+2/3) in its
    /// second bank, and the writes to both chips (base+8/9, 388h/389h) in
    /// its first. With nothing in the second bank, the program wrote both
    /// chips alike, so the right chip becomes a copy of the left, timers
    /// and all; else the right chip gets that bank's registers, and the
    /// left keeps the first bank's. Nothing changes without a right chip.
    pub fn right_from_bank_1(&mut self) {
        let Some(right) = &mut self.right else {
            return;
        };
        let bank_1 = std::mem::replace(&mut self.regs[1], [0; 256]);
        if bank_1.iter().all(|&reg| reg == 0) {
            right.regs[0] = self.regs[0];
            right.address[0] = self.address[0];
            right.timer_value = self.timer_value;
            right.timer_start = self.timer_start;
            right.timer_mask = self.timer_mask;
            right.timer_expired = self.timer_expired;
        } else {
            right.regs[0] = bank_1;
            right.address[0] = self.address[1];
        }
        right.active = self.active;
    }

    /// Load what `save_right` saved. A state of a machine with a dual
    /// OPL2 doesn't load on one without, nor the other way round.
    pub fn load_right(
        &mut self,
        r: &mut rust_dos_savestate::Reader,
    ) -> rust_dos_savestate::Result<()> {
        use rust_dos_savestate::{State, StateError};
        let mut some = false;
        some.load(r)?;
        match (some, &mut self.right) {
            (true, Some(right)) => right.load(r),
            (false, None) => Ok(()),
            (true, None) => Err(StateError::Mismatch(
                "it has two OPL2s and this machine hasn't".into(),
            )),
            (false, Some(_)) => Err(StateError::Mismatch(
                "this machine has two OPL2s and it hasn't".into(),
            )),
        }
    }
}

#[cfg(test)]
mod mute_tests {
    use super::*;

    /// The loudest sample of `frames` frames.
    fn loudest(opl: &mut Opl, frames: usize) -> i32 {
        (0..frames).map(|_| opl.render()).map(|(l, r)| (l as i32).abs().max((r as i32).abs())).max().unwrap_or(0)
    }

    #[test]
    fn a_muted_channel_plays_on_unheard() {
        let mut opl = Opl::new(false);
        // Channel 0: a plain sine on both operators, full volume, fast
        // attack and no decay, then A4 keyed on.
        for (reg, value) in [(0x20, 0x01), (0x23, 0x01), (0x40, 0x10), (0x43, 0x00), (0x60, 0xF0), (0x63, 0xF0), (0x80, 0x00), (0x83, 0x00), (0xC0, 0x01), (0xA0, 0x44), (0xB0, 0x32)] {
            opl.write_address(0, reg);
            opl.write_data(0, value, 0);
        }
        assert!(loudest(&mut opl, 2000) > 1000);
        let view = opl.channels()[0];
        assert!(view.key_on && view.level > 200 && view.f_num == 0x244 && view.block == 4, "{:?}", view);
        opl.set_mute(1);
        // The resampler's last frames, then nothing.
        loudest(&mut opl, 4);
        assert_eq!(loudest(&mut opl, 2000), 0);
        assert!(opl.channels()[0].key_on, "it goes on playing");
        opl.set_mute(0);
        assert!(loudest(&mut opl, 2000) > 1000);
    }
}

#[cfg(test)]
mod idle_tests {
    use super::*;

    fn write(opl: &mut Opl, reg: u8, value: u8) {
        opl.write_address(0, reg);
        opl.write_data(0, value, 0);
    }

    fn loudest(opl: &mut Opl, frames: usize) -> i32 {
        (0..frames)
            .map(|_| opl.render())
            .map(|(l, r)| (l as i32).abs().max((r as i32).abs()))
            .max()
            .unwrap_or(0)
    }

    /// Channel 0 playing A4: a sine on both operators at full volume,
    /// fast attack, held while the key is on, and a release of a few
    /// hundred milliseconds.
    fn key_a4(opl: &mut Opl) {
        for (reg, value) in [
            (0x20, 0x21),
            (0x23, 0x21),
            (0x40, 0x10),
            (0x43, 0x00),
            (0x60, 0xF0),
            (0x63, 0xF0),
            (0x80, 0x08),
            (0x83, 0x08),
            (0xC0, 0x01),
            (0xA0, 0x44),
            (0xB0, 0x32),
        ] {
            write(opl, reg, value);
        }
    }

    /// A note keyed off and released to silence leaves the chip idle, so
    /// it stops synthesizing until the next key; the next note plays as
    /// the first did.
    #[test]
    fn the_chip_goes_idle_once_its_notes_are_released() {
        let mut opl = Opl::new(false);
        assert!(opl.is_idle());
        key_a4(&mut opl);
        assert!(loudest(&mut opl, 2000) > 1000);
        assert!(!opl.is_idle());
        write(&mut opl, 0xB0, 0x12);
        assert!(!opl.is_idle(), "a key write keeps it running");
        assert!(loudest(&mut opl, 441) > 1000, "the release starts loud");
        assert!(!opl.is_idle());
        loudest(&mut opl, 2 * RATE as usize);
        assert!(opl.is_idle(), "released to silence");
        assert_eq!(loudest(&mut opl, 2000), 0);

        // A level written meanwhile applies to the next note only.
        write(&mut opl, 0x43, 0x01);
        assert!(opl.is_idle());
        write(&mut opl, 0xB0, 0x32);
        assert!(!opl.is_idle());
        assert!(loudest(&mut opl, 2000) > 1000);
    }

    /// In rhythm mode the drums are keyed by register BDh: a drum
    /// released to silence leaves the chip idle, and the next drum key
    /// wakes it.
    #[test]
    fn a_rhythm_key_wakes_the_idle_chip() {
        let mut opl = Opl::new(false);
        // The bass drum: channel 6, operators 12 and 15.
        for (reg, value) in [
            (0x30, 0x01),
            (0x33, 0x01),
            (0x50, 0x10),
            (0x53, 0x00),
            (0x70, 0xF0),
            (0x73, 0xF0),
            (0x90, 0x08),
            (0x93, 0x08),
            (0xC6, 0x00),
            (0xA6, 0x44),
            (0xB6, 0x12),
        ] {
            write(&mut opl, reg, value);
        }
        write(&mut opl, 0xBD, 0x30);
        assert!(loudest(&mut opl, 2000) > 1000);
        write(&mut opl, 0xBD, 0x20);
        loudest(&mut opl, 2 * RATE as usize);
        assert!(opl.is_idle(), "the drum released to silence");
        write(&mut opl, 0xBD, 0x30);
        assert!(!opl.is_idle());
        assert!(loudest(&mut opl, 2000) > 1000);
    }

    /// A state saved while the chip is idle loads idle, without notes
    /// starting again, and the next key plays.
    #[test]
    fn an_idle_chip_loads_idle() {
        let mut opl = Opl::new(false);
        key_a4(&mut opl);
        loudest(&mut opl, 2000);
        write(&mut opl, 0xB0, 0x12);
        loudest(&mut opl, 2 * RATE as usize);
        assert!(opl.is_idle());
        let mut w = rust_dos_savestate::Writer::new();
        rust_dos_savestate::State::save(&opl, &mut w);
        let mut loaded = Opl::new(false);
        let mut r = rust_dos_savestate::Reader::new(&w.buf);
        rust_dos_savestate::State::load(&mut loaded, &mut r).unwrap();
        loaded.after_load();
        assert!(loaded.is_idle());
        assert_eq!(loudest(&mut loaded, 2000), 0);
        write(&mut loaded, 0xB0, 0x32);
        assert!(loudest(&mut loaded, 2000) > 1000);
    }
}

#[cfg(test)]
mod timer_tests {
    use super::*;

    fn write(opl: &mut Opl, reg: u8, value: u8, now_us: u64) {
        opl.write_address(0, reg);
        opl.write_data(0, value, now_us);
    }

    /// Titus the Fox's and Moktar's AdLib check: both timers run masked,
    /// then timer 1 alone is unmasked until it runs out, then masked again
    /// and timer 2 unmasked. The game waits for A0h, timer 2's flag alone.
    #[test]
    fn masking_a_timer_clears_its_flag() {
        let mut opl = Opl::new(false);
        write(&mut opl, 0x04, 0x63, 0);
        assert_eq!(opl.read_status(10_000), 0x06);
        write(&mut opl, 0x04, 0x23, 19_000);
        // Timer 1, at value 0, runs out 256 * 80 us after its start.
        assert_eq!(opl.read_status(20_000), 0x06);
        assert_eq!(opl.read_status(20_480), 0xC6);
        write(&mut opl, 0x04, 0x43, 20_500);
        assert_eq!(
            opl.read_status(20_500),
            0x06,
            "the mask cleared timer 1's flag"
        );
        // Timer 2 runs out 256 * 320 us after its start.
        assert_eq!(opl.read_status(81_910), 0x06);
        assert_eq!(opl.read_status(81_920), 0xA6);
    }

    /// A timer that overflowed while masked shows its flag once unmasked;
    /// the overflows before the write that masked it don't.
    #[test]
    fn an_unmasked_timer_shows_the_overflows_since_its_mask() {
        let mut opl = Opl::new(false);
        write(&mut opl, 0x02, 0xFF, 0);
        write(&mut opl, 0x04, 0x41, 0);
        assert_eq!(opl.read_status(100), 0x06, "masked");
        write(&mut opl, 0x04, 0x01, 1_000);
        assert_eq!(opl.read_status(1_000), 0xC6);

        // It overflows every 80 us: at 1040, then at 1120.
        write(&mut opl, 0x04, 0x41, 1_050);
        write(&mut opl, 0x04, 0x01, 1_060);
        assert_eq!(opl.read_status(1_119), 0x06);
        assert_eq!(opl.read_status(1_120), 0xC6);
    }

    /// Stopping a timer keeps the flag of an overflow that no status read
    /// has seen yet.
    #[test]
    fn a_stopped_timer_keeps_its_flag() {
        let mut opl = Opl::new(false);
        write(&mut opl, 0x02, 0xFF, 0);
        write(&mut opl, 0x04, 0x01, 0);
        write(&mut opl, 0x04, 0x00, 100);
        assert_eq!(opl.read_status(200), 0xC6);
    }

    /// A value written to a running timer applies from its next overflow;
    /// an overflow before the write keeps its flag.
    #[test]
    fn a_new_timer_value_applies_from_the_next_overflow() {
        let mut opl = Opl::new(false);
        write(&mut opl, 0x02, 0xFE, 100_000);
        write(&mut opl, 0x04, 0x01, 100_000);
        // 160 us periods: overflows at 100_160, 100_320.
        write(&mut opl, 0x02, 0xFF, 100_200);
        write(&mut opl, 0x04, 0x80, 100_200);
        assert_eq!(opl.read_status(100_319), 0x06);
        assert_eq!(opl.read_status(100_320), 0xC6);
        // Then 80 us periods: 100_400.
        write(&mut opl, 0x04, 0x80, 100_320);
        assert_eq!(opl.read_status(100_399), 0x06);
        assert_eq!(opl.read_status(100_400), 0xC6);

        let mut opl = Opl::new(false);
        write(&mut opl, 0x02, 0xFF, 0);
        write(&mut opl, 0x04, 0x01, 0);
        write(&mut opl, 0x02, 0x00, 100);
        assert_eq!(opl.read_status(100), 0xC6, "the overflow at 80");
    }

    /// The reset bit clears the flags of running timers until their next
    /// overflow.
    #[test]
    fn a_reset_flag_comes_back_at_the_next_overflow() {
        let mut opl = Opl::new(false);
        write(&mut opl, 0x02, 0xFF, 0);
        write(&mut opl, 0x04, 0x01, 0);
        assert_eq!(opl.read_status(100), 0xC6);
        write(&mut opl, 0x04, 0x80, 100);
        assert_eq!(opl.read_status(100), 0x06);
        assert_eq!(opl.read_status(159), 0x06);
        assert_eq!(opl.read_status(160), 0xC6);
    }
}

#[cfg(test)]
mod old_state_tests {
    use super::*;

    /// The loudest sample of the left and the right output.
    fn loudest(opl: &mut Opl, frames: usize) -> (i32, i32) {
        (0..frames)
            .map(|_| opl.render())
            .fold((0, 0), |(l, r), (fl, fr)| {
                (l.max((fl as i32).abs()), r.max((fr as i32).abs()))
            })
    }

    /// Channel 0 keyed on with a plain sine, as `regs` of a bank hold it.
    fn note() -> [u8; 256] {
        let mut regs = [0; 256];
        for (reg, value) in [
            (0x20, 0x21),
            (0x23, 0x21),
            (0x40, 0x10),
            (0x60, 0xF0),
            (0x63, 0xF0),
            (0xC0, 0x31),
            (0xA0, 0x44),
            (0xB0, 0x32),
        ] {
            regs[reg] = value;
        }
        regs
    }

    /// A state of the old SB Pro 1, whose OPL3 took 388h's writes in its
    /// first bank and base+2/3's in its second: what `load` leaves in a
    /// dual OPL2's left chip.
    fn loaded(bank_0: [u8; 256], bank_1: [u8; 256]) -> Opl {
        let mut opl = Opl::with_chip(Chip::DualOpl2);
        opl.regs = [bank_0, bank_1];
        opl.active = true;
        opl
    }

    /// An AdLib program's notes, which went through 388h to the first
    /// bank only, play on both sides once the state loads.
    #[test]
    fn notes_written_to_both_chips_play_on_both_sides() {
        let mut opl = loaded(note(), [0; 256]);
        opl.timer_value[0] = 0xFF;
        opl.timer_start[0] = Some(0);
        opl.right_from_bank_1();
        opl.after_load();
        let (l, r) = loudest(&mut opl, 2000);
        assert!(l > 1000 && r > 1000, "left {} right {}", l, r);
        assert_eq!(opl.read_status_of(1, 100), 0xC6, "the right chip's timer");
    }

    /// Notes written to the right chip alone, which went to the second
    /// bank, play on the right only.
    #[test]
    fn notes_written_to_the_right_chip_play_on_the_right() {
        let mut opl = loaded([0; 256], note());
        opl.right_from_bank_1();
        opl.after_load();
        let (l, r) = loudest(&mut opl, 2000);
        assert!(l == 0 && r > 1000, "left {} right {}", l, r);
        assert_eq!(opl.regs[1], [0; 256]);
    }
}
