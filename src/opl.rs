//! The FM synthesizer: a Yamaha YMF262 (OPL3), as on the SB Pro 2 and SB16,
//! or its OPL2 subset, as on an AdLib or SB 2.0. Synthesis is Nuked-OPL3
//! (the `nuked-opl3` crate); this module adds the two timers and the status
//! register that programs use to detect the chip, which Nuked-OPL3 leaves
//! out.
//!
//! Register writes take effect at their emulated time: the audio mixer
//! renders up to the present before each write (see `Bus::audio_catch_up`).

use nuked_opl3::Opl3Chip;

/// Output sample rate of the mixer, which the chip renders at.
pub const RATE: u32 = 44_100;

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
}

impl Opl {
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
        }
    }

    pub fn is_opl3(&self) -> bool {
        self.opl3
    }

    /// Latch a register address in `bank` (1 is the OPL3's second bank).
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
                    self.timer_value[(reg - 2) as usize] = value;
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
        self.chip.write_register((bank as u16) << 8 | reg as u16, value);
    }

    fn timer_control(&mut self, value: u8, now_us: u64) {
        if value & 0x80 != 0 {
            // Reset the timer flags.
            self.timer_expired = [false; 2];
            return;
        }
        self.timer_mask = [value & 0x40 != 0, value & 0x20 != 0];
        for (i, bit) in [0x01u8, 0x02].into_iter().enumerate() {
            let start = value & bit != 0;
            if start && self.timer_start[i].is_none() {
                self.timer_start[i] = Some(now_us);
                self.timer_expired[i] = false;
            } else if !start {
                self.timer_start[i] = None;
            }
        }
    }

    /// The status register: IRQ (bit 7) and the timer flags (6, 5). An
    /// OPL2 also reads 06h in the low bits, which is how programs tell the
    /// two apart.
    pub fn read_status(&mut self, now_us: u64) -> u8 {
        // Timer 1 counts 80 us steps, timer 2 320 us steps, up from the
        // value to 256.
        for (i, step) in [80u64, 320].into_iter().enumerate() {
            if let Some(start) = self.timer_start[i] {
                let period = (256 - self.timer_value[i] as u64) * step;
                if now_us.saturating_sub(start) >= period && !self.timer_mask[i] {
                    self.timer_expired[i] = true;
                }
            }
        }
        let mut status = 0;
        if self.timer_expired[0] {
            status |= 0x40;
        }
        if self.timer_expired[1] {
            status |= 0x20;
        }
        if status != 0 {
            status |= 0x80;
        }
        if self.opl3 { status } else { status | 0x06 }
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
