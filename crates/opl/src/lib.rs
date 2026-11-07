//! The FM synthesizer: a Yamaha YMF262 (OPL3), as on the SB Pro 2 and SB16,
//! or its OPL2 subset, as on an AdLib or SB 2.0. Synthesis is Nuked-OPL3
//! (the `nuked-opl3` crate); this module adds the two timers and the status
//! register that programs use to detect the chip, which Nuked-OPL3 leaves
//! out.
//!
//! Register writes take effect at their emulated time: the audio mixer
//! renders up to the present before each write (see `Bus::audio_catch_up`).

use nuked_opl3::Opl3Chip;
pub use nuked_opl3::ChannelView;

/// Output sample rate of the mixer, which the chip renders at.
pub use rust_dos_audio_core::RATE;

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
    /// Whether any key has been played since the last reset, so a silent
    /// chip can skip synthesis.
    active: bool,
    /// The chip's registers as written, for a save state: the chip can't
    /// be saved, so a loaded state writes them to a new one (`after_load`).
    regs: [[u8; 256]; 2],
    /// The channels left out of the mix (the sound visualizer's), a bit
    /// each.
    mute: u32,
}

impl Opl {
    /// Whether the chip has made no sound since it was reset (`render`
    /// then gives silence without running it).
    #[inline]
    pub fn is_idle(&self) -> bool {
        !self.active
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
            regs: [[0; 256]; 2],
            mute: 0,
        }
    }

    #[inline]
    pub fn is_opl3(&self) -> bool {
        self.opl3
    }

    /// Latch a register address in `bank` (1 is the OPL3's second bank).
    #[inline]
    pub fn write_address(&mut self, bank: usize, value: u8) {
        if bank == 0 || self.opl3 {
            self.address[bank] = value;
        }
    }

    /// Write the latched register of `bank`. `now_us` is emulated time.
    pub fn write_data(&mut self, bank: usize, value: u8, now_us: u64) {
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

    /// The status register: IRQ (bit 7) and the timer flags (6, 5) that
    /// aren't masked. An OPL2 also reads 06h in the low bits, which is
    /// how programs tell the two apart.
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

    /// Leave the channels whose bits are set (0 to 17) out of the mix.
    pub fn set_mute(&mut self, mask: u32) {
        self.mute = mask;
        self.chip.set_mute(mask);
    }

    pub fn mute(&self) -> u32 {
        self.mute
    }

    /// What the 18 channels play (the first 9 on an OPL2).
    pub fn channels(&self) -> [ChannelView; 18] {
        self.chip.channel_views()
    }

    /// Render one stereo frame at `RATE`.
    #[inline]
    pub fn render(&mut self) -> (i16, i16) {
        if !self.active {
            return (0, 0);
        }
        let mut frame = [0i16; 2];
        let _ = self.chip.generate_resampled(&mut frame);
        (frame[0], frame[1])
    }
}

// Whether it is an OPL3 comes with the Sound Blaster's model; the chip is
// written anew from `regs` after a load.
rust_dos_savestate::state_fields!(Opl { address, timer_value, timer_start, timer_mask, timer_expired, active, regs } skip { chip, opl3, mute });

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
