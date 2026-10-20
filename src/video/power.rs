//! The monitor's power effects (`monitor_effects`): what a CRT's picture
//! does when it is switched on, when the card changes to another frequency,
//! when it is degaussed and when it is switched off. They run in wall time,
//! a moment each, and change the frame the window shows (`apply`), before
//! the CRT shader, which bends them as the glass did.
//!
//! - Power on: dark, then the picture comes up as the tube warms, slightly
//!   too big at first, wobbling with colour fringes while the degaussing
//!   coil's field dies away.
//! - Resync: the picture blanks while the monitor locks on, and comes back
//!   with a jolt.
//! - Degauss: the wobble alone.
//! - Power off: the picture collapses to a bright line, then to a dot that
//!   fades.

use super::Frame;
use std::time::{Duration, Instant};

/// Which of the monitor's effects there are (`monitor_effects`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MonitorEffects {
    #[default]
    Off,
    /// What the picture does, silently.
    Picture,
    /// The relay's clicks and the degaussing, with the picture as it is.
    Sound,
    On,
}

impl MonitorEffects {
    pub const ALL: [Self; 4] = [Self::Off, Self::Picture, Self::Sound, Self::On];

    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Picture => "picture",
            Self::Sound => "sound",
            Self::On => "on",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|e| e.name() == value)
    }

    /// As the settings window shows it.
    pub fn describe(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Picture => "picture only",
            Self::Sound => "sound only",
            Self::On => "picture and sound",
        }
    }

    pub fn picture(self) -> bool {
        matches!(self, Self::Picture | Self::On)
    }

    pub fn sound(self) -> bool {
        matches!(self, Self::Sound | Self::On)
    }
}

/// Which effect plays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    PowerOn,
    Resync,
    Degauss,
    PowerOff,
}

impl Effect {
    pub fn length(self) -> Duration {
        Duration::from_millis(match self {
            Effect::PowerOn => 1600,
            Effect::Resync => 380,
            Effect::Degauss => 1100,
            Effect::PowerOff => 480,
        })
    }
}

/// How the picture looks at a moment of an effect.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub brightness: f32,
    /// The picture's size, 1 for all of it.
    pub scale: (f32, f32),
    /// How far down it is pushed, in picture heights.
    pub offset_y: f32,
    /// How far rows swing sideways, in pixels, and how far the colours
    /// part.
    pub wobble: f32,
    /// The phase of the wobble, in radians.
    pub phase: f32,
}

impl Look {
    const PLAIN: Look = Look { brightness: 1.0, scale: (1.0, 1.0), offset_y: 0.0, wobble: 0.0, phase: 0.0 };
}

/// The effect playing, if any.
#[derive(Default)]
pub struct PowerFx {
    playing: Option<(Effect, Instant)>,
}

fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// `t` from `a` to `b`, as 0 to 1.
fn part(t: f32, a: f32, b: f32) -> f32 {
    ((t - a) / (b - a)).clamp(0.0, 1.0)
}

impl PowerFx {
    /// Start `effect`. A resync while one plays starts again; a power-on
    /// isn't cut short by one.
    pub fn trigger(&mut self, effect: Effect, now: Instant) {
        if let Some((playing, _)) = self.playing
            && matches!(playing, Effect::PowerOn | Effect::PowerOff)
            && effect == Effect::Resync
        {
            return;
        }
        self.playing = Some((effect, now));
    }

    /// Whether an effect plays at `now`: the window draws every frame then.
    pub fn active(&mut self, now: Instant) -> bool {
        if let Some((effect, start)) = self.playing
            && now.duration_since(start) >= effect.length()
        {
            self.playing = None;
        }
        self.playing.is_some()
    }

    /// The effect playing.
    pub fn playing(&self) -> Option<Effect> {
        self.playing.map(|(effect, _)| effect)
    }

    /// How the picture looks at `now`; None without an effect.
    pub fn look(&self, now: Instant) -> Option<Look> {
        let (effect, start) = self.playing?;
        let secs = now.saturating_duration_since(start).as_secs_f32();
        Some(look_at(effect, secs))
    }
}

/// The look of `effect` `t` seconds in.
pub fn look_at(effect: Effect, t: f32) -> Look {
    let wobble = |t: f32, from: f32, to: f32| {
        let p = part(t, from, to);
        if p <= 0.0 || p >= 1.0 { 0.0 } else { 7.0 * (1.0 - p).powi(2) }
    };
    match effect {
        Effect::PowerOn => {
            let warm = ease(part(t, 0.12, 1.3));
            Look {
                brightness: warm,
                scale: (1.0 + 0.04 * (1.0 - warm), 1.0 + 0.03 * (1.0 - warm)),
                offset_y: 0.0,
                wobble: wobble(t, 0.2, 1.5),
                phase: t * 40.0,
            }
        }
        Effect::Resync => {
            let back = part(t, 0.16, 0.38);
            Look {
                brightness: if t < 0.16 { 0.0 } else { ease(back) * (1.0 + 0.15 * (1.0 - back)) },
                scale: (1.0, 1.0),
                offset_y: 0.12 * (1.0 - ease(back)) * if t < 0.16 { 0.0 } else { 1.0 },
                wobble: 0.0,
                phase: 0.0,
            }
        }
        Effect::Degauss => Look { wobble: wobble(t, 0.0, 1.1), phase: t * 40.0, ..Look::PLAIN },
        Effect::PowerOff => {
            let line = ease(part(t, 0.0, 0.12));
            let dot = ease(part(t, 0.14, 0.3));
            let fade = part(t, 0.3, 0.48);
            Look {
                brightness: (1.0 + 1.5 * line) * (1.0 - fade),
                scale: (1.0 - 0.995 * dot, 1.0 - 0.997 * line),
                offset_y: 0.0,
                wobble: 0.0,
                phase: 0.0,
            }
        }
    }
}

/// Show `frame` as it looks with `look`.
pub fn apply(frame: &mut Frame, look: &Look) {
    if *look == Look::PLAIN {
        return;
    }
    let (w, h) = (frame.width as usize, frame.height as usize);
    if w == 0 || h == 0 {
        return;
    }
    frame.doubled = false;
    let source = frame.rgba.clone();
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    // At least a line of 1.5 pixels and a dot of 2: the beam's spot.
    let (sx, sy) = (look.scale.0.max(2.0 / w as f32), look.scale.1.max(1.5 / h as f32));
    let brightness = look.brightness.max(0.0);
    // A picture squeezed to a few lines or pixels is the beam's light of
    // all of it in them: brighter where it is smaller, up to a glow.
    let squeeze = (1.0 / (sx * sy)).clamp(1.0, 6.0).sqrt();
    let gain = (brightness * squeeze * 256.0) as u32;
    // The beam's own glow, white, in a picture squeezed to a line or dot.
    let collapse = (1.0 - sx.min(sy)).max(0.0).powi(6);
    let glow = (collapse * brightness.min(1.0) * 255.0) as u32;
    let offset = look.offset_y * h as f32;
    for y in 0..h {
        let src_y = (y as f32 + 0.5 - cy - offset) / sy + cy;
        let row = &mut frame.rgba[y * w * 4..(y + 1) * w * 4];
        if !(0.0..h as f32).contains(&src_y) || gain == 0 {
            row.as_chunks_mut::<4>().0.fill([0, 0, 0, 255]);
            continue;
        }
        let src_row = &source[src_y as usize * w * 4..][..w * 4];
        let swing = look.wobble * (look.phase + y as f32 * 0.045).sin();
        let fringe = look.wobble * 0.35;
        for x in 0..w {
            let src_x = (x as f32 + 0.5 - cx) / sx + cx - swing;
            let pixel = &mut row[x * 4..x * 4 + 3];
            for (c, shift) in [(0, -fringe), (1, 0.0), (2, fringe)] {
                let at = src_x + shift;
                pixel[c] = if (0.0..w as f32).contains(&at) {
                    (((src_row[at as usize * 4 + c] as u32 * gain) >> 8) + glow).min(255) as u8
                } else {
                    0
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn white(w: u32, h: u32) -> Frame {
        let mut frame = Frame::new(w, h);
        frame.clear([200; 3]);
        frame
    }

    fn lit(frame: &Frame, x: usize, y: usize) -> bool {
        frame.rgba[(y * frame.width as usize + x) * 4..][..3].iter().any(|&c| c > 0)
    }

    #[test]
    fn power_on_starts_dark_and_ends_as_the_picture() {
        let mut frame = white(64, 40);
        apply(&mut frame, &look_at(Effect::PowerOn, 0.05));
        assert!(frame.rgba.as_chunks::<4>().0.iter().all(|p| *p == [0, 0, 0, 255]));
        assert_eq!(look_at(Effect::PowerOn, 1.59).brightness, 1.0);
        assert_eq!(look_at(Effect::PowerOn, 1.59).wobble, 0.0);
        let mut frame = white(64, 40);
        apply(&mut frame, &look_at(Effect::PowerOn, 1.59));
        assert!(frame.rgba.as_chunks::<4>().0.iter().all(|p| *p == [200, 200, 200, 255]));
    }

    #[test]
    fn power_off_collapses_to_a_line_then_a_dot() {
        let mut frame = white(64, 40);
        apply(&mut frame, &look_at(Effect::PowerOff, 0.13));
        assert!(lit(&frame, 32, 20) && lit(&frame, 2, 20), "a line across");
        assert!(!lit(&frame, 32, 5) && !lit(&frame, 32, 35));
        let mut frame = white(64, 40);
        apply(&mut frame, &look_at(Effect::PowerOff, 0.29));
        assert!(lit(&frame, 32, 20) && !lit(&frame, 2, 20), "a dot in the middle");
        assert!(look_at(Effect::PowerOff, 0.48).brightness <= 0.0);
    }

    #[test]
    fn a_resync_blanks_then_brings_the_picture_back() {
        let mut frame = white(64, 40);
        apply(&mut frame, &look_at(Effect::Resync, 0.1));
        assert!(frame.rgba.as_chunks::<4>().0.iter().all(|p| *p == [0, 0, 0, 255]));
        let look = look_at(Effect::Resync, 0.2);
        assert!(look.brightness > 0.0 && look.offset_y > 0.0);
        let mut fx = PowerFx::default();
        let now = Instant::now();
        fx.trigger(Effect::PowerOn, now);
        fx.trigger(Effect::Resync, now);
        assert_eq!(fx.playing(), Some(Effect::PowerOn), "a power-on isn't cut short");
        assert!(fx.active(now));
        assert!(!fx.active(now + Duration::from_secs(2)));
    }
}
