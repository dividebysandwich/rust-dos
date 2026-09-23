//! The CD drive playing audio tracks, as MSCDEX's Play Audio asks: Red Book
//! audio is 44.1 kHz 16-bit stereo, the mixer's own rate, 588 sample pairs
//! per sector.

use super::image::CdImage;
use std::collections::VecDeque;
use std::rc::Rc;

/// Sample pairs per sector.
pub const FRAMES_PER_SECTOR: u64 = 588;
/// Sectors read from the image at a time: a third of a second.
const READ_AHEAD: u32 = 25;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayState {
    Stopped,
    Playing,
    Paused,
}

pub struct CdPlayer {
    image: Option<Rc<CdImage>>,
    /// The drive that plays (0 = A:).
    drive: u8,
    state: PlayState,
    /// The range of the last Play request: first sector, and the sector
    /// after its end.
    start: u32,
    end: u32,
    /// Sample pairs played since `start`.
    played: u64,
    /// Samples read ahead from the image.
    buffer: VecDeque<(i16, i16)>,
    /// Next sector to read into the buffer.
    next: u32,
    /// Audio channel control (IOCTL output 03h): for output channels 0
    /// and 1, the input channel they play and its volume.
    pub channels: [(u8, u8); 2],
}

impl Default for CdPlayer {
    fn default() -> Self {
        Self::new()
    }
}

impl CdPlayer {
    pub fn new() -> Self {
        Self {
            image: None,
            drive: 0,
            state: PlayState::Stopped,
            start: 0,
            end: 0,
            played: 0,
            buffer: VecDeque::new(),
            next: 0,
            channels: [(0, 0xFF), (1, 0xFF)],
        }
    }

    pub fn state(&self) -> PlayState {
        self.state
    }

    /// Whether `drive` is playing audio: MSCDEX reports it busy then.
    pub fn is_busy(&self, drive: u8) -> bool {
        self.state == PlayState::Playing && self.drive == drive
    }

    /// The drive this player last played on, if it still holds a range.
    pub fn drive(&self) -> Option<u8> {
        (self.state != PlayState::Stopped || self.image.is_some()).then_some(self.drive)
    }

    /// Start playing `count` sectors from `lba` of `image` on `drive`.
    pub fn play(&mut self, drive: u8, image: Rc<CdImage>, lba: u32, count: u32) {
        let end = lba.saturating_add(count).min(image.leadout());
        self.image = Some(image);
        self.drive = drive;
        self.start = lba;
        self.end = end;
        self.played = 0;
        self.next = lba;
        self.buffer.clear();
        self.state = if lba < end { PlayState::Playing } else { PlayState::Stopped };
    }

    /// MSCDEX Stop Audio: pause playing audio; stopping paused audio
    /// forgets it.
    pub fn stop(&mut self) {
        match self.state {
            PlayState::Playing => self.state = PlayState::Paused,
            _ => self.reset(),
        }
    }

    /// Forget the play range (a reset, an eject, a seek, or a read).
    pub fn reset(&mut self) {
        self.state = PlayState::Stopped;
        self.buffer.clear();
        self.image = None;
    }

    /// Stop what `drive` plays, when its disc goes away.
    pub fn stop_drive(&mut self, drive: u8) {
        if self.drive == drive {
            self.reset();
        }
    }

    /// MSCDEX Resume Audio. False if nothing was paused.
    pub fn resume(&mut self) -> bool {
        if self.state != PlayState::Paused {
            return false;
        }
        self.state = PlayState::Playing;
        true
    }

    /// The sector being played (or where the pause is).
    pub fn position(&self) -> u32 {
        (self.start as u64 + self.played / FRAMES_PER_SECTOR).min(self.end.max(self.start) as u64) as u32
    }

    /// The track being played.
    pub fn track(&self) -> Option<u8> {
        Some(self.image.as_ref()?.track_at(self.position())?.number)
    }

    /// The range of the last Play: first sector and the sector after it.
    pub fn range(&self) -> (u32, u32) {
        (self.start, self.end)
    }

    /// The next sample pair, as the mixer adds them up (16-bit scale).
    pub fn render(&mut self) -> (f32, f32) {
        if self.state != PlayState::Playing {
            return (0.0, 0.0);
        }
        if self.buffer.is_empty() {
            self.refill();
        }
        let Some((l, r)) = self.buffer.pop_front() else {
            self.state = PlayState::Stopped;
            return (0.0, 0.0);
        };
        self.played += 1;
        let input = [l as f32, r as f32];
        let out = |(channel, volume): (u8, u8)| match channel {
            0 | 1 => input[channel as usize] * volume as f32 / 255.0,
            _ => 0.0,
        };
        (out(self.channels[0]), out(self.channels[1]))
    }

    /// Let `frames` sample pairs pass unplayed, when the mixer skips ahead.
    pub fn skip(&mut self, frames: u64) {
        if self.state != PlayState::Playing {
            return;
        }
        let buffered = (self.buffer.len() as u64).min(frames);
        self.buffer.drain(..buffered as usize);
        self.played += frames;
        // Whole sectors past the buffer need no reading.
        let past = (frames - buffered) / FRAMES_PER_SECTOR;
        self.next = self.next.saturating_add(past as u32);
        if self.position() >= self.end {
            self.state = PlayState::Stopped;
            self.buffer.clear();
        }
    }

    fn refill(&mut self) {
        let Some(image) = &self.image else {
            return;
        };
        let last = self.end.min(self.next.saturating_add(READ_AHEAD));
        let mut samples = Vec::with_capacity(((last - self.next.min(last)) as u64 * FRAMES_PER_SECTOR) as usize);
        for lba in self.next..last {
            if image.read_audio(lba, &mut samples).is_err() {
                break;
            }
        }
        self.next = last;
        self.buffer.extend(samples);
    }
}
