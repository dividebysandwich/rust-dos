//! The card's display: the mode registers, the mode shown, flips, the
//! palette of `P8` modes, and the frame's timing (chapter 6 of
//! `docs/toadforce-spec.md`).
//!
//! The card has no clock of its own. Whoever reaches it says what time
//! it is and what timing the VGA has (`Live`), and `sync` then counts
//! the vertical blanks that began since the last time. Every entry
//! point of the card syncs first, at an emulated time its caller fixes,
//! so what a guest reads doesn't depend on how often the host looks.

use super::caps;
use super::format::Format;
use super::regs::{self, Error};
use crate::video::VideoMode;
use crate::video::crt::CrtTiming;

/// The moment of an access to the card.
#[derive(Clone, Copy, Debug, Default)]
pub struct Live {
    /// The emulated clock, in nanoseconds.
    pub ns: u64,
    /// The VGA's timing, which is the display's while the card shows no
    /// mode of its own.
    pub vga: CrtTiming,
}

/// The mode the card shows: the mode registers as they were at the write
/// of 1 to `MODE_ENABLE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shown {
    pub width: u32,
    pub height: u32,
    pub format: Format,
    pub pitch: u32,
    pub refresh: u32,
}

impl Shown {
    /// The frame's timing: the mode's lines and one for every 16 of
    /// them and one more of blank, each a whole number of nanoseconds
    /// long, as many as fit `refresh` frames in a second. The vertical
    /// blank is the retrace too, from the line after the last one shown
    /// to the frame's end, and display enable is off for the last fifth
    /// of each line.
    pub fn timing(&self) -> CrtTiming {
        let total = self.height + self.height / 16 + 1;
        let line_ns = 1_000_000_000 / (self.refresh * total);
        CrtTiming {
            line_ns,
            hdisplay_ns: line_ns - line_ns / 5,
            total,
            display: self.height,
            retrace_start: self.height,
            retrace_end: total,
        }
    }

    /// The offset after the last byte of the surface at `start`.
    fn end(&self, start: u32) -> u64 {
        start as u64
            + (self.height as u64 - 1) * self.pitch as u64
            + self.width as u64 * self.format.bytes() as u64
    }
}

/// The VGA's side of the screen as it was when the card's mode took it:
/// what a write of 0 to `MODE_ENABLE` brings back. The VGA's registers
/// stay in the VGA; this is what the Super VGA keeps beside them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Legacy {
    /// The mode the VGA showed.
    pub video_mode: VideoMode,
    /// The VESA mode set, if one was: its number, width and height, and
    /// its bits a pixel.
    pub vesa: Option<(u16, u16, u16)>,
    pub vesa_bpp: u8,
    /// The timing the VGA was held to, if it was.
    pub timing: Option<CrtTiming>,
    /// Whether it was set with the linear frame buffer.
    pub lfb: bool,
    pub pitch: u32,
    pub start: u32,
}

/// What a write changed that the machine's display has to follow, the
/// least first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Change {
    #[default]
    None,
    /// The picture: the surface shown, or a colour of a `P8` mode's.
    Picture,
    /// The mode shown, or whether the card shows one.
    Mode,
}

/// What `sync` found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Synced {
    /// A vertical blank began.
    pub blank: bool,
    /// A pending flip took place.
    pub flipped: bool,
}

/// The part of the display that moves with time: where in its frames it
/// is, and what waits for the next one. A copy brought up to the moment
/// of a read answers the read, since reading can't change the card.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Beam {
    /// `MODE_START`.
    start: u32,
    /// The surface a `FLIP` shows at the next vertical blank.
    pending: Option<u32>,
    /// `FRAME`.
    frame: u32,
    /// The timing the vertical blanks up to `synced_ns` were counted
    /// with.
    timing: CrtTiming,
    synced_ns: u64,
}

/// Whether `timing` describes frames, which vertical blanks can be
/// counted of.
fn counts(timing: &CrtTiming) -> bool {
    timing.frame_ns() != 0
}

impl Beam {
    /// Count the vertical blanks that began up to `ns`, make the flip
    /// that waited for one, and go on with the timing `current`.
    fn sync(&mut self, ns: u64, current: CrtTiming) -> Synced {
        let mut synced = Synced::default();
        if counts(&self.timing) && ns > self.synced_ns {
            let began = self
                .timing
                .retraces(ns)
                .saturating_sub(self.timing.retraces(self.synced_ns));
            if began > 0 {
                self.frame = self.frame.wrapping_add(began as u32);
                synced.blank = true;
                if let Some(start) = self.pending.take() {
                    self.start = start;
                    synced.flipped = true;
                }
            }
        }
        // The VGA's registers may have changed its timing since the last
        // time: the blanks up to now were the old timing's.
        self.timing = current;
        self.synced_ns = self.synced_ns.max(ns);
        synced
    }
}

#[derive(Clone, Debug)]
pub struct Display {
    /// The `MODE` registers, but `MODE_START`.
    enable: bool,
    width: u32,
    height: u32,
    format: u32,
    pitch: u32,
    refresh: u32,
    shown: Option<Shown>,
    beam: Beam,
    palette: [u32; 256],
    /// Set while the card's mode has the screen (the bus keeps it).
    pub legacy: Option<Legacy>,
}

impl Default for Display {
    fn default() -> Self {
        Display {
            enable: false,
            width: 0,
            height: 0,
            format: 0,
            pitch: 0,
            refresh: 0,
            shown: None,
            beam: Beam::default(),
            palette: [0; 256],
            legacy: None,
        }
    }
}

crate::state_enum!(Format {
    Format::P8,
    Format::R5G6B5,
    Format::X8R8G8B8,
    Format::A8R8G8B8
});

impl Default for Shown {
    fn default() -> Self {
        Shown {
            width: 0,
            height: 0,
            format: Format::P8,
            pitch: 0,
            refresh: 0,
        }
    }
}

crate::state_fields!(Shown {
    width,
    height,
    format,
    pitch,
    refresh
});
impl Default for Legacy {
    fn default() -> Self {
        Legacy {
            video_mode: VideoMode::Text80x25Color,
            vesa: None,
            vesa_bpp: 0,
            timing: None,
            lfb: false,
            pitch: 0,
            start: 0,
        }
    }
}

crate::state_fields!(Legacy {
    video_mode,
    vesa,
    vesa_bpp,
    timing,
    lfb,
    pitch,
    start
});
crate::state_fields!(Beam {
    start,
    pending,
    frame,
    timing,
    synced_ns
});
crate::state_fields!(Display {
    enable,
    width,
    height,
    format,
    pitch,
    refresh,
    shown,
    beam,
    palette,
    legacy,
});

impl Display {
    /// The mode the card shows, if it shows one.
    pub fn shown(&self) -> Option<&Shown> {
        self.shown.as_ref()
    }

    /// `MODE_START`: where the surface shown is.
    pub fn start(&self) -> u32 {
        self.beam.start
    }

    /// The colours of a `P8` mode: red in bits 16 to 23, green in 8 to
    /// 15, blue in 0 to 7.
    pub fn palette(&self) -> &[u32; 256] {
        &self.palette
    }

    /// The display's timing at `live`: the mode shown's, or the VGA's.
    fn timing_at(&self, live: Live) -> CrtTiming {
        self.shown.map_or(live.vga, |s| s.timing())
    }

    /// Count the vertical blanks that began up to `live`, and make the
    /// flip that waited for one.
    pub fn sync(&mut self, live: Live) -> Synced {
        let current = self.timing_at(live);
        self.beam.sync(live.ns, current)
    }

    /// What `sync` would find at `live`, and the display's moving part as
    /// it would leave it.
    fn ahead(&self, live: Live) -> (Beam, Synced) {
        let mut beam = self.beam;
        let synced = beam.sync(live.ns, self.timing_at(live));
        (beam, synced)
    }

    /// What `sync` would find at `live`, for a read.
    pub fn due(&self, live: Live) -> Synced {
        self.ahead(live).1
    }

    /// When the first vertical blank after the last `sync` begins, in
    /// nanoseconds, if the display has frames.
    pub fn next_blank(&self) -> Option<u64> {
        let beam = &self.beam;
        counts(&beam.timing).then(|| beam.timing.next_retrace(beam.synced_ns))
    }

    /// Whether a flip waits for the next vertical blank.
    pub fn flip_pending(&self) -> bool {
        self.beam.pending.is_some()
    }

    /// The display's bits of `STATUS` at `live`.
    pub fn status(&self, live: Live) -> u32 {
        let (beam, _) = self.ahead(live);
        let mut status = 0;
        if counts(&beam.timing) && beam.timing.status(live.ns) & 0x08 != 0 {
            status |= regs::STATUS_VBLANK;
        }
        if beam.pending.is_some() {
            status |= regs::STATUS_FLIP_PENDING;
        }
        status
    }

    /// A display register as it reads at `live`.
    pub fn read(&self, offset: u32, live: Live) -> u32 {
        let (beam, _) = self.ahead(live);
        match offset {
            regs::MODE_ENABLE => self.enable as u32,
            regs::MODE_WIDTH => self.width,
            regs::MODE_HEIGHT => self.height,
            regs::MODE_FORMAT => self.format,
            regs::MODE_PITCH => self.pitch,
            regs::MODE_START => beam.start,
            regs::MODE_REFRESH => self.refresh,
            regs::FRAME => beam.frame,
            regs::SCANLINE if counts(&beam.timing) => beam.timing.position(live.ns).0,
            regs::PALETTE..regs::PALETTE_END => {
                self.palette[((offset - regs::PALETTE) / 4) as usize]
            }
            _ => 0,
        }
    }

    /// The mode the registers describe, if it is one the card takes with
    /// `memory` bytes of card memory.
    fn mode(&self, memory: u32) -> Option<Shown> {
        let format = Format::from_code(self.format).filter(|f| f.in_mask(caps::MODE_FORMATS))?;
        let shown = Shown {
            width: self.width,
            height: self.height,
            format,
            pitch: self.pitch,
            refresh: self.refresh,
        };
        let sizes = 1..=caps::SURFACE_MAX;
        let valid = sizes.contains(&self.width)
            && sizes.contains(&self.height)
            && self.pitch as u64 >= self.width as u64 * format.bytes() as u64
            && (caps::REFRESH_MIN..=caps::REFRESH_MAX).contains(&self.refresh)
            && shown.end(self.beam.start) <= memory as u64;
        valid.then_some(shown)
    }

    /// A display register written at `live`, after `sync`, on a card
    /// with `memory` bytes of card memory.
    pub fn write(
        &mut self,
        offset: u32,
        value: u32,
        memory: u32,
        live: Live,
    ) -> Result<Change, Error> {
        match offset {
            regs::MODE_ENABLE if value & 1 != 0 => {
                self.shown = Some(self.mode(memory).ok_or(Error::BadMode)?);
                self.enable = true;
                self.beam.pending = None;
                self.beam.timing = self.timing_at(live);
                return Ok(Change::Mode);
            }
            regs::MODE_ENABLE => {
                let was = self.shown.take().is_some();
                self.enable = false;
                self.beam.pending = None;
                self.beam.timing = self.timing_at(live);
                return Ok(if was { Change::Mode } else { Change::None });
            }
            regs::MODE_WIDTH => self.width = value,
            regs::MODE_HEIGHT => self.height = value,
            regs::MODE_FORMAT => self.format = value,
            regs::MODE_PITCH => self.pitch = value,
            regs::MODE_REFRESH => self.refresh = value,
            regs::MODE_START => {
                if let Some(shown) = &self.shown {
                    if shown.end(value) > memory as u64 {
                        return Err(Error::BadRange);
                    }
                    self.beam.pending = None;
                }
                self.beam.start = value;
                return Ok(Change::Picture);
            }
            regs::PALETTE..regs::PALETTE_END => {
                self.palette[((offset - regs::PALETTE) / 4) as usize] = value & 0x00FF_FFFF;
                return Ok(if self.shown.is_some_and(|s| s.format == Format::P8) {
                    Change::Picture
                } else {
                    Change::None
                });
            }
            _ => {}
        }
        Ok(Change::None)
    }

    /// `FLIP`: show the surface at `offset`, at the next vertical blank
    /// with `wait` while the card shows its mode. True if the flip took
    /// place at once.
    pub fn flip(&mut self, offset: u32, wait: bool, memory: u32) -> Result<bool, Error> {
        let end = match &self.shown {
            Some(shown) => shown.end(offset),
            None => offset as u64 + 1,
        };
        if end > memory as u64 {
            return Err(Error::BadRange);
        }
        if wait && self.shown.is_some() {
            self.beam.pending = Some(offset);
            return Ok(false);
        }
        self.beam.pending = None;
        self.beam.start = offset;
        Ok(true)
    }

    /// The registers at their reset values, with no mode shown and no
    /// flip pending. What the VGA's side was (`legacy`) stays for the
    /// bus to bring back.
    pub fn reset(&mut self, live: Live) {
        *self = Display {
            legacy: self.legacy,
            beam: Beam {
                synced_ns: self.beam.synced_ns,
                ..Beam::default()
            },
            ..Display::default()
        };
        self.beam.timing = self.timing_at(live);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MEMORY: u32 = 32 << 20;

    fn at(ns: u64) -> Live {
        Live {
            ns,
            vga: CrtTiming::VGA_400,
        }
    }

    fn mode(
        d: &mut Display,
        width: u32,
        height: u32,
        format: u32,
        refresh: u32,
    ) -> Result<Change, Error> {
        let bytes = Format::from_code(format).map_or(1, |f| f.bytes());
        for (reg, value) in [
            (regs::MODE_WIDTH, width),
            (regs::MODE_HEIGHT, height),
            (regs::MODE_FORMAT, format),
            (regs::MODE_PITCH, width * bytes),
            (regs::MODE_REFRESH, refresh),
        ] {
            d.write(reg, value, MEMORY, at(0)).unwrap();
        }
        d.write(regs::MODE_ENABLE, 1, MEMORY, at(0))
    }

    #[test]
    fn a_frame_has_the_modes_lines_and_a_sixteenth_more() {
        let shown = Shown {
            width: 640,
            height: 480,
            format: Format::R5G6B5,
            pitch: 1280,
            refresh: 60,
        };
        let t = shown.timing();
        assert_eq!(t.total, 511);
        assert_eq!(t.line_ns, 1_000_000_000 / (60 * 511));
        assert_eq!((t.display, t.retrace_start, t.retrace_end), (480, 480, 511));
        // The blank is the retrace: both status bits from line 480 on.
        let line = t.line_ns as u64;
        assert_eq!(t.status(479 * line) & 0x08, 0);
        assert_eq!(t.status(480 * line) & 0x09, 0x09);
        assert_eq!(t.status(510 * line + line - 1) & 0x08, 0x08);
        assert_eq!(t.status(511 * line) & 0x09, 0);
        // The largest mode at the highest rate still has lines.
        let big = Shown {
            height: 4096,
            refresh: 200,
            ..shown
        };
        assert!(big.timing().line_ns > 1000);
    }

    #[test]
    fn a_mode_is_checked_when_it_is_enabled() {
        let mut d = Display::default();
        assert_eq!(mode(&mut d, 640, 480, 2, 60), Ok(Change::Mode));
        assert_eq!(d.shown().unwrap().pitch, 1280);
        for (width, height, format, refresh) in [
            (0, 480, 2, 60),
            (640, 4097, 2, 60),
            (640, 480, 4, 60),
            (640, 480, 0, 60),
            (640, 480, 2, 49),
            (640, 480, 2, 201),
        ] {
            let mut d = Display::default();
            assert_eq!(
                mode(&mut d, width, height, format, refresh),
                Err(Error::BadMode),
                "{width}x{height}, format {format}, {refresh} Hz"
            );
            assert!(d.shown().is_none());
            assert_eq!(d.read(regs::MODE_ENABLE, at(0)), 0);
        }
        // A surface past the end of card memory, and a pitch too small.
        let mut d = Display::default();
        d.write(regs::MODE_START, MEMORY - 1280 * 480 + 1, MEMORY, at(0))
            .unwrap();
        assert_eq!(mode(&mut d, 640, 480, 2, 60), Err(Error::BadMode));
        let mut d = Display::default();
        mode(&mut d, 640, 480, 2, 60).unwrap();
        d.write(regs::MODE_PITCH, 1279, MEMORY, at(0)).unwrap();
        assert_eq!(
            d.write(regs::MODE_ENABLE, 1, MEMORY, at(0)),
            Err(Error::BadMode)
        );
        // The mode shown stays, with its own pitch.
        assert_eq!(d.shown().unwrap().pitch, 1280);
    }

    #[test]
    fn the_registers_change_nothing_shown_until_enabled_again() {
        let mut d = Display::default();
        mode(&mut d, 640, 480, 2, 60).unwrap();
        d.write(regs::MODE_WIDTH, 800, MEMORY, at(0)).unwrap();
        d.write(regs::MODE_HEIGHT, 600, MEMORY, at(0)).unwrap();
        d.write(regs::MODE_PITCH, 1600, MEMORY, at(0)).unwrap();
        assert_eq!(d.read(regs::MODE_WIDTH, at(0)), 800);
        assert_eq!(
            (d.shown().unwrap().width, d.shown().unwrap().height),
            (640, 480)
        );
        assert_eq!(
            d.write(regs::MODE_ENABLE, 1, MEMORY, at(0)),
            Ok(Change::Mode)
        );
        assert_eq!(
            (d.shown().unwrap().width, d.shown().unwrap().height),
            (800, 600)
        );
        assert_eq!(
            d.write(regs::MODE_ENABLE, 0, MEMORY, at(0)),
            Ok(Change::Mode)
        );
        assert!(d.shown().is_none());
        assert_eq!(
            d.write(regs::MODE_ENABLE, 0, MEMORY, at(0)),
            Ok(Change::None)
        );
    }

    #[test]
    fn blanks_are_counted_and_a_waiting_flip_takes_place_at_one() {
        let mut d = Display::default();
        mode(&mut d, 640, 480, 2, 60).unwrap();
        let t = d.shown().unwrap().timing();
        let (line, frame) = (t.line_ns as u64, t.frame_ns());
        // The first blank begins after 480 lines.
        let mut now = 480 * line - 1;
        assert_eq!(d.sync(at(now)), Synced::default());
        assert_eq!(d.read(regs::SCANLINE, at(now)), 479);
        assert_eq!(d.status(at(now)), 0);
        assert_eq!(d.next_blank(), Some(480 * line));
        now += 1;
        assert!(d.sync(at(now)).blank);
        assert_eq!(d.read(regs::FRAME, at(now)), 1);
        assert_eq!(d.read(regs::SCANLINE, at(now)), 480);
        assert_eq!(d.status(at(now)), regs::STATUS_VBLANK);
        assert_eq!(d.next_blank(), Some(480 * line + frame));

        // A flip that waits shows at the next blank and not before.
        assert_eq!(d.flip(0x10_0000, true, MEMORY), Ok(false));
        assert_eq!(
            d.status(at(now)) & regs::STATUS_FLIP_PENDING,
            regs::STATUS_FLIP_PENDING
        );
        assert_eq!(d.start(), 0);
        now += frame - 1;
        assert_eq!(d.sync(at(now)), Synced::default());
        // A second flip replaces the first.
        assert_eq!(d.flip(0x20_0000, true, MEMORY), Ok(false));
        now += 1;
        assert_eq!(
            d.sync(at(now)),
            Synced {
                blank: true,
                flipped: true
            }
        );
        assert_eq!(d.start(), 0x20_0000);
        assert_eq!(d.status(at(now)) & regs::STATUS_FLIP_PENDING, 0);
        // Several frames at once count each of their blanks.
        now += 5 * frame;
        assert!(d.sync(at(now)).blank);
        assert_eq!(d.read(regs::FRAME, at(now)), 7);
    }

    #[test]
    fn a_flip_is_checked_against_the_mode_shown() {
        let mut d = Display::default();
        // With the VGA showing, a flip is made at once whatever it says.
        assert_eq!(d.flip(0x1000, true, MEMORY), Ok(true));
        assert_eq!(d.start(), 0x1000);
        assert_eq!(d.flip(MEMORY, false, MEMORY), Err(Error::BadRange));
        d.write(regs::MODE_START, 0, MEMORY, at(0)).unwrap();
        mode(&mut d, 640, 480, 3, 60).unwrap();
        let last = MEMORY - 640 * 4 * 480;
        assert_eq!(d.flip(last, false, MEMORY), Ok(true));
        assert_eq!(d.flip(last + 1, false, MEMORY), Err(Error::BadRange));
        assert_eq!(d.start(), last);
        // A write of MODE_START is checked the same way, and cancels a
        // flip that waits.
        assert_eq!(d.flip(0, true, MEMORY), Ok(false));
        assert_eq!(
            d.write(regs::MODE_START, last + 1, MEMORY, at(0)),
            Err(Error::BadRange)
        );
        assert!(d.flip_pending());
        assert_eq!(
            d.write(regs::MODE_START, 64, MEMORY, at(0)),
            Ok(Change::Picture)
        );
        assert!(!d.flip_pending());
        assert_eq!(d.start(), 64);
    }

    #[test]
    fn with_the_vga_showing_its_retraces_are_counted() {
        let mut d = Display::default();
        let t = CrtTiming::VGA_400;
        d.sync(at(0));
        let first = t.retrace_start as u64 * t.line_ns as u64;
        assert!(!d.sync(at(first - 1)).blank);
        assert!(d.sync(at(first)).blank);
        assert_eq!(d.status(at(first)), regs::STATUS_VBLANK);
        assert_eq!(d.read(regs::FRAME, at(first)), 1);
        // After the retrace's two lines the bit clears.
        let after = first + 2 * t.line_ns as u64;
        d.sync(at(after));
        assert_eq!(d.status(at(after)), 0);
        assert_eq!(d.read(regs::SCANLINE, at(after)), 414);
    }

    #[test]
    fn the_palette_keeps_24_bits() {
        let mut d = Display::default();
        d.write(regs::PALETTE + 4 * 255, 0xFF12_3456, MEMORY, at(0))
            .unwrap();
        assert_eq!(d.read(regs::PALETTE + 4 * 255, at(0)), 0x12_3456);
        assert_eq!(d.palette()[255], 0x12_3456);
    }
}
