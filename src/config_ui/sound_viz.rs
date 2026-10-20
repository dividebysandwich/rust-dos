//! The sound visualizer (Ctrl+Shift+S): over the picture while the window
//! is closed, an oscilloscope of each source that plays (the Sound
//! Blaster, the FM synthesizer, the Ultrasound...), the FM chip's channels
//! with their notes and envelopes, the Ultrasound's voices, and the notes
//! held on each MIDI channel. A click on a source, channel or voice mutes
//! it, and on a source's S plays it alone; the mutes are for the moment
//! and end with the visualizer.

use super::ConfigUi;
use super::draw::{self, Grid, Layout, Rgb};
use crate::bus::Bus;
use crate::gus::VoiceView;
use crate::mixer::{CHANNELS, Channel, Scopes};
use crate::opl::ChannelView;
use crate::video::Frame;

/// What the sources and chips play, taken from the machine each frame.
pub struct SoundView {
    /// The sources heard lately, with their last frames, oldest first.
    scopes: Vec<(Channel, Vec<f32>)>,
    silenced: u16,
    opl: Option<([ChannelView; 18], bool, u32)>,
    gus: Option<(Vec<VoiceView>, u32)>,
    midi: Option<(Box<[[u8; 128]; 16]>, u16)>,
}

/// The frames of a scope drawn: about 23 ms.
const SCOPE_SHOWN: usize = 1024;

impl SoundView {
    pub fn of(bus: &Bus) -> Self {
        let scopes = bus
            .mixer
            .scopes
            .as_deref()
            .map(|scopes| {
                Channel::ALL
                    .iter()
                    .map(|&channel| (channel, ring(scopes, channel)))
                    .filter(|(channel, frames)| {
                        *channel == Channel::Master
                            || frames.iter().any(|s| s.abs() > 16.0)
                            || bus.mixer.silenced >> channel.index() & 1 != 0
                    })
                    .collect()
            })
            .unwrap_or_default();
        let opl = (!bus.opl.is_idle()).then(|| (bus.opl.channels(), bus.opl.is_opl3(), bus.opl.mute()));
        let gus = bus
            .gus
            .as_ref()
            .map(|gus| (gus.voice_views(), gus.mute()))
            .filter(|(voices, mute)| *mute != 0 || voices.iter().any(|v| v.playing && v.level > 0.0));
        let held = bus.mpu.held_notes();
        let midi = (held.iter().flatten().any(|&v| v > 0) || bus.mpu.mute() != 0).then(|| (Box::new(*held), bus.mpu.mute()));
        Self { scopes, silenced: bus.mixer.silenced, opl, gus, midi }
    }
}

/// `channel`'s scope, oldest frame first.
fn ring(scopes: &Scopes, channel: Channel) -> Vec<f32> {
    let frames = &scopes.frames[channel.index()];
    let (older, newer) = frames.split_at(scopes.at);
    newer.iter().chain(older).copied().skip(frames.len().saturating_sub(SCOPE_SHOWN)).collect()
}

/// What a click on the visualizer does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VizAction {
    /// Mute a source, or (`true`) play it alone.
    Source(Channel, bool),
    Opl(u8),
    Gus(u8),
    Midi(u8),
}

impl VizAction {
    /// Carry it out on the machine.
    pub fn apply(self, bus: &mut Bus) {
        match self {
            VizAction::Source(channel, solo) => {
                let bit = 1u16 << channel.index();
                let all = (1u16 << CHANNELS) - 1;
                let master = 1u16 << Channel::Master.index();
                let mixer = &mut bus.mixer;
                mixer.silenced = if !solo {
                    mixer.silenced ^ bit
                } else if mixer.silenced == all & !bit & !master {
                    0
                } else {
                    all & !bit & !master
                };
            }
            VizAction::Opl(ch) => {
                let mute = bus.opl.mute() ^ (1 << ch);
                bus.opl.set_mute(mute);
            }
            VizAction::Gus(voice) => {
                if let Some(gus) = &mut bus.gus {
                    let mute = gus.mute() ^ (1 << voice);
                    gus.set_mute(mute);
                }
            }
            VizAction::Midi(ch) => {
                let mute = bus.mpu.mute() ^ (1 << ch);
                bus.mpu.set_mute(mute);
            }
        }
    }
}

/// Turn the visualizer's measuring on the machine on or off; off, every
/// mute ends.
pub fn watch(bus: &mut Bus, on: bool) {
    if on {
        bus.mixer.scopes.get_or_insert_with(Default::default);
    } else {
        bus.mixer.scopes = None;
        bus.mixer.silenced = 0;
        bus.opl.set_mute(0);
        if let Some(gus) = &mut bus.gus {
            gus.set_mute(0);
        }
        bus.mpu.set_mute(0);
    }
}

/// The visualizer while it shows: the machine's last view, how long each
/// MIDI note still glows, and where its controls were drawn.
pub struct SoundViz {
    view: Option<SoundView>,
    glow: Box<[[u8; 128]; 16]>,
    /// Frame pixel rectangles (x, y, width, height) and what a click there
    /// does.
    hits: Vec<((usize, usize, usize, usize), VizAction)>,
}

impl Default for SoundViz {
    fn default() -> Self {
        Self { view: None, glow: Box::new([[0; 128]; 16]), hits: Vec::new() }
    }
}

/// A note's name: C-1 to G9, from its MIDI number.
fn note_name(note: i32) -> String {
    const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    format!("{}{}", NAMES[note.rem_euclid(12) as usize], note.div_euclid(12) - 1)
}

/// The MIDI note an FM channel's frequency number and block play.
fn fm_note(f_num: u16, block: u8) -> Option<i32> {
    if f_num == 0 {
        return None;
    }
    let hz = f_num as f64 * 49_716.0 / (1u64 << (20 - block as u32)) as f64;
    Some((69.0 + 12.0 * (hz / 440.0).log2()).round() as i32)
}

/// A level from 0 to 1 as a bar `width` characters wide, in the shades
/// of the block characters.
fn bar(level: f32, width: usize) -> Vec<u8> {
    let eighths = (level.clamp(0.0, 1.0) * width as f32 * 4.0).round() as usize;
    (0..width)
        .map(|i| match eighths.saturating_sub(i * 4) {
            0 => b' ',
            1 => 0xB0,
            2 => 0xB1,
            3 => 0xB2,
            _ => 0xDB,
        })
        .collect()
}

fn source_label(channel: Channel) -> &'static str {
    match channel {
        Channel::Master => "Mix",
        Channel::Speaker => "Speaker",
        Channel::Sb => "Sound Blaster",
        Channel::Fm => "FM",
        Channel::Gus => "Ultrasound",
        Channel::Midi => "MIDI",
        Channel::CdAudio => "CD audio",
        Channel::DiskNoise => "Disk noise",
        Channel::LptDac => "LPT DAC",
        Channel::Tandy => "Tandy",
        Channel::Awe => "AWE32",
        Channel::Monitor => "Monitor",
        Channel::Modem => "Modem",
    }
}

/// What the panel holds, row by row, before it is drawn.
enum Part {
    Title,
    /// Scopes `from..to`, a label row and `rows` rows of trace.
    Scopes(usize, usize),
    Heading(&'static str),
    /// FM channels `from..to`.
    Opl(usize, usize),
    /// Ultrasound voices `from..to`.
    Gus(usize, usize),
    /// A MIDI channel's notes.
    Midi(u8),
}

impl ConfigUi {
    /// Ctrl+Shift+S: show or hide the visualizer. Returns what to tell
    /// the user over the picture.
    pub fn sound_viz_key(&mut self, bus: &mut Bus) -> &'static str {
        let on = self.sound_viz.is_none();
        watch(bus, on);
        if on {
            self.sound_viz = Some(SoundViz::default());
            "Sound visualizer: click to mute, S to solo (Ctrl+Shift+S hides it)"
        } else {
            self.sound_viz = None;
            "Sound visualizer off"
        }
    }

    pub fn sound_viz_shown(&self) -> bool {
        self.sound_viz.is_some()
    }

    /// Take the machine's sound as it is now, once a frame.
    pub fn set_sound(&mut self, bus: &Bus) {
        let Some(viz) = &mut self.sound_viz else { return };
        let view = SoundView::of(bus);
        let held = bus.mpu.held_notes();
        for (glow, held) in viz.glow.iter_mut().zip(held) {
            for (g, &v) in glow.iter_mut().zip(held) {
                *g = if v > 0 { (v as u32 * 2).min(255) as u8 } else { g.saturating_sub(10) };
            }
        }
        viz.view = Some(view);
    }

    /// What a click at frame pixel (`x`, `y`) on the visualizer does, if
    /// it is on one of its controls.
    pub fn sound_viz_hit(&self, (x, y): (i32, i32)) -> Option<VizAction> {
        let viz = self.sound_viz.as_ref().filter(|_| !self.open)?;
        let (x, y) = (usize::try_from(x).ok()?, usize::try_from(y).ok()?);
        viz.hits.iter().find(|((hx, hy, w, h), _)| (*hx..hx + w).contains(&x) && (*hy..hy + h).contains(&y)).map(|&(_, a)| a)
    }

    /// Draw the visualizer over the bottom of `frame`, while the window is
    /// closed.
    pub fn draw_sound_viz(&mut self, frame: &mut Frame) {
        if self.open {
            return;
        }
        let Some(viz) = &mut self.sound_viz else { return };
        viz.hits.clear();
        let Some(view) = &viz.view else { return };
        let (width, height) = (frame.width as usize, frame.height as usize);
        let cols = (width / 8).saturating_sub(2);
        if cols < 40 || height < 100 {
            return;
        }

        // The parts, and the cell height that lets them fit in the lower
        // two thirds of the picture.
        let mut parts = vec![Part::Title];
        let per_row = (cols / 16).clamp(1, 6);
        for from in (0..view.scopes.len()).step_by(per_row) {
            parts.push(Part::Scopes(from, (from + per_row).min(view.scopes.len())));
        }
        if let Some((channels, opl3, _)) = &view.opl {
            parts.push(Part::Heading(if *opl3 { "FM synthesizer (OPL3)" } else { "FM synthesizer (OPL2)" }));
            let used = if *opl3 && channels[9..].iter().any(|c| c.key_on || c.level > 0) { 18 } else { 9 };
            for from in (0..used).step_by(9) {
                parts.push(Part::Opl(from, from + 9));
            }
        }
        if let Some((voices, _)) = &view.gus
            && !voices.is_empty()
        {
            parts.push(Part::Heading("Gravis Ultrasound"));
            for from in (0..voices.len()).step_by(16) {
                parts.push(Part::Gus(from, (from + 16).min(voices.len())));
            }
        }
        if let Some((held, mute)) = &view.midi {
            parts.push(Part::Heading("MIDI"));
            let sounding = (0..16u8).filter(|&ch| held[ch as usize].iter().any(|&v| v > 0) || viz.glow[ch as usize].iter().any(|&g| g > 0) || mute >> ch & 1 != 0);
            parts.extend(sounding.take(8).map(Part::Midi));
        }
        let scope_rows = |cell_h: usize| if cell_h == 16 { 2 } else { 3 };
        let rows_of = |cell_h: usize| -> usize {
            parts.iter().map(|p| if let Part::Scopes(..) = p { 1 + scope_rows(cell_h) } else { 1 }).sum()
        };
        let cell_h = if rows_of(16) * 16 <= height * 2 / 3 { 16 } else { 8 };
        let rows = rows_of(cell_h).min(height / cell_h - 1);
        let layout = Layout { x: (width - cols * 8) / 2, y: height - rows * cell_h - cell_h / 2, cell_h, cols, rows };

        let mut grid = Grid::new(cols, rows);
        let mut traces = Vec::new();
        let mut pianos = Vec::new();
        let mut hits = Vec::new();
        let cell = |col: usize, row: usize, w: usize, h: usize| (layout.x + col * 8, layout.y + row * cell_h, w * 8, h * cell_h);
        let silenced = |channel: Channel| view.silenced >> channel.index() & 1 != 0;
        let mut row = 0;
        for part in &parts {
            if row >= rows {
                break;
            }
            match *part {
                Part::Title => {
                    grid.text(1, row, "Sound", draw::BRIGHT);
                    grid.text_to(8, row, "click to mute, S to play alone, Ctrl+Shift+S to hide", draw::DIM, cols - 1);
                    row += 1;
                }
                Part::Scopes(from, to) => {
                    let w = (cols - 2) / per_row;
                    let h = scope_rows(cell_h).min(rows - row - 1);
                    for (i, (channel, frames)) in view.scopes[from..to].iter().enumerate() {
                        let col = 1 + i * w;
                        let muted = silenced(*channel);
                        let color = if muted { draw::DIM } else { draw::TEXT };
                        grid.text_to(col, row, source_label(*channel), color, col + w - 4);
                        if *channel != Channel::Master {
                            grid.text(col + w - 4, row, "M", if muted { draw::ERROR } else { draw::KEY });
                            grid.text(col + w - 2, row, "S", draw::KEY);
                            hits.push((cell(col, row, w - 5, 1 + h), VizAction::Source(*channel, false)));
                            hits.push((cell(col + w - 4, row, 1, 1), VizAction::Source(*channel, false)));
                            hits.push((cell(col + w - 2, row, 1, 1), VizAction::Source(*channel, true)));
                        }
                        let trace = if muted { draw::DIM } else if *channel == Channel::Master { draw::BRIGHT } else { draw::CODE };
                        traces.push((cell(col, row + 1, w - 1, h), frames.clone(), trace));
                    }
                    row += 1 + h;
                }
                Part::Heading(title) => {
                    grid.text(1, row, title, draw::NOTE);
                    row += 1;
                }
                Part::Opl(from, to) => {
                    let Some((channels, _, mute)) = &view.opl else { continue };
                    let w = (cols - 2) / 9;
                    for (ch, c) in channels.iter().enumerate().take(to).skip(from) {
                        let col = 1 + (ch - from) * w;
                        let muted = mute >> ch & 1 != 0;
                        let name = if c.drum && (6..=8).contains(&ch) { "drum".to_string() } else { fm_note(c.f_num, c.block).filter(|_| c.key_on || c.level > 0).map_or("-".into(), note_name) };
                        let color = if muted { draw::ERROR } else if c.key_on { draw::BRIGHT } else { draw::DIM };
                        let label = format!("{:>2} {}", ch + 1, name);
                        let end = grid.text_to(col, row, &label, color, col + w - 1);
                        let room = (col + w - 1).saturating_sub(end + 1);
                        for (i, b) in bar(c.level as f32 / 255.0, room).into_iter().enumerate() {
                            grid.char(end + 1 + i, row, b, if muted { draw::DIM } else { draw::GOOD });
                        }
                        hits.push((cell(col, row, w - 1, 1), VizAction::Opl(ch as u8)));
                    }
                    row += 1;
                }
                Part::Gus(from, to) => {
                    let Some((voices, mute)) = &view.gus else { continue };
                    let w = (cols - 2) / 16;
                    for (i, v) in voices[from..to].iter().enumerate() {
                        let voice = from + i;
                        let col = 1 + i * w;
                        let muted = mute >> voice & 1 != 0;
                        let color = if muted { draw::ERROR } else if v.playing { draw::BRIGHT } else { draw::DIM };
                        let end = grid.text(col, row, &format!("{:>2}", voice + 1), color);
                        let room = (col + w - 1).saturating_sub(end);
                        let level = if v.playing { v.level } else { 0.0 };
                        for (j, b) in bar(level, room).into_iter().enumerate() {
                            grid.char(end + j, row, b, if muted { draw::DIM } else { draw::CODE });
                        }
                        hits.push((cell(col, row, w - 1, 1), VizAction::Gus(voice as u8)));
                    }
                    row += 1;
                }
                Part::Midi(ch) => {
                    let Some((_, mute)) = &view.midi else { continue };
                    let muted = mute >> ch & 1 != 0;
                    grid.text(1, row, &format!("Ch {:>2}", ch + 1), if muted { draw::ERROR } else { draw::TEXT });
                    hits.push((cell(1, row, 5, 1), VizAction::Midi(ch)));
                    pianos.push((cell(8, row, cols - 9, 1), ch, muted));
                    row += 1;
                }
            }
        }
        draw::render_blended(&grid, &layout, frame, draw::PANEL_ALPHA * 3 / 4);
        for (rect, frames, color) in &traces {
            scope(frame, *rect, frames, *color);
        }
        for (rect, ch, muted) in pianos {
            piano(frame, rect, &viz.glow[ch as usize], muted);
        }
        viz.hits = hits;
    }
}

/// Blend `color` into the pixel at (`x`, `y`).
fn put(frame: &mut Frame, x: usize, y: usize, color: Rgb, alpha: u32) {
    let (width, height) = (frame.width as usize, frame.height as usize);
    if x < width && y < height {
        let at = (y * width + x) * 4;
        let px = &mut frame.rgba[at..at + 3];
        px.copy_from_slice(&draw::blend([px[0], px[1], px[2]], color, alpha));
    }
}

/// An oscilloscope of `frames` (full scale 32768) in `rect`.
fn scope(frame: &mut Frame, (x0, y0, w, h): (usize, usize, usize, usize), frames: &[f32], color: Rgb) {
    if w == 0 || h < 3 || frames.is_empty() {
        return;
    }
    for x in 0..w {
        for y in 0..h {
            put(frame, x0 + x, y0 + y, draw::FIELD, draw::PANEL_ALPHA);
        }
        put(frame, x0 + x, y0 + h / 2, draw::DIM, draw::OPAQUE / 3);
    }
    let y_of = |s: f32| {
        let half = (h - 1) as f32 / 2.0;
        (half - (s / 32768.0).clamp(-1.0, 1.0) * half).round() as usize
    };
    let mut last = None;
    for x in 0..w {
        let y = y_of(frames[x * frames.len() / w]);
        let (a, b) = last.map_or((y, y), |l: usize| (l.min(y), l.max(y)));
        for dy in a..=b {
            put(frame, x0 + x, y0 + dy, color, draw::OPAQUE);
        }
        last = Some(y);
    }
}

/// The notes of a MIDI channel on a keyboard 128 keys wide in `rect`, a
/// held note bright by its velocity, one just let go fading.
fn piano(frame: &mut Frame, (x0, y0, w, h): (usize, usize, usize, usize), glow: &[u8; 128], muted: bool) {
    let lit = if muted { draw::DIM } else { draw::GOOD };
    for x in 0..w {
        let note = x * 128 / w;
        let black = matches!(note % 12, 1 | 3 | 6 | 8 | 10);
        let g = glow[note] as u32;
        for y in 1..h.saturating_sub(1) {
            let base = if black { Rgb(0x04, 0x08, 0x18) } else { draw::FIELD };
            put(frame, x0 + x, y0 + y, base, draw::PANEL_ALPHA);
            if g > 0 {
                put(frame, x0 + x, y0 + y, lit, g);
            }
        }
        // A tick at each C.
        if note % 12 == 0 && x * 128 / w != (x.max(1) - 1) * 128 / w {
            put(frame, x0 + x, y0 + h - 1, draw::DIM, draw::OPAQUE);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_and_bars() {
        assert_eq!(note_name(69), "A4");
        assert_eq!(note_name(60), "C4");
        // A4 on the OPL: F-number 580 in block 4 is 440 Hz.
        assert_eq!(fm_note(580, 4), Some(69));
        assert_eq!(fm_note(0, 4), None);
        assert_eq!(bar(1.0, 3), vec![0xDB; 3]);
        assert_eq!(bar(0.0, 2), vec![b' '; 2]);
        assert_eq!(bar(0.5, 2), vec![0xDB, b' ']);
    }

    #[test]
    fn a_source_mutes_and_plays_alone() {
        let mut bus = Bus::new(std::path::PathBuf::from("."));
        watch(&mut bus, true);
        VizAction::Source(Channel::Fm, false).apply(&mut bus);
        assert_eq!(bus.mixer.silenced, 1 << Channel::Fm.index());
        VizAction::Source(Channel::Fm, false).apply(&mut bus);
        assert_eq!(bus.mixer.silenced, 0);
        VizAction::Source(Channel::Sb, true).apply(&mut bus);
        let sb = 1 << Channel::Sb.index();
        let master = 1 << Channel::Master.index();
        assert_eq!(bus.mixer.silenced & (sb | master), 0);
        assert_ne!(bus.mixer.silenced & (1 << Channel::Fm.index()), 0);
        VizAction::Source(Channel::Sb, true).apply(&mut bus);
        assert_eq!(bus.mixer.silenced, 0, "solo again ends it");
        VizAction::Opl(3).apply(&mut bus);
        VizAction::Midi(9).apply(&mut bus);
        assert_eq!((bus.opl.mute(), bus.mpu.mute()), (1 << 3, 1 << 9));
        watch(&mut bus, false);
        assert_eq!((bus.opl.mute(), bus.mpu.mute(), bus.mixer.silenced), (0, 0, 0));
        assert!(bus.mixer.scopes.is_none());
    }
}
