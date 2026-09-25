//! The host's mixer (`[mixer]`): a volume for each sound source and one for
//! them all, the PC speaker's and the Sound Blaster's filters, and a reverb
//! and a chorus the synthesizers are sent to. It sits on top of the
//! emulated cards' own levels, such as the Sound Blaster's mixer chip,
//! which programs set and a new program resets.

use crate::dsp::{Chorus, Reverb, SpeakerFilter, StereoLowpass};
pub use crate::dsp::{ChorusPreset, ReverbPreset};

/// A sound source the mixer has a volume for, or `Master` for the mix.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Channel {
    Master,
    /// The PC speaker, and the beep of the BEL character.
    Speaker,
    /// The Sound Blaster's digital audio.
    Sb,
    /// The OPL2 or OPL3 FM synthesizer.
    Fm,
    Gus,
    /// General MIDI sent to the MPU-401.
    Midi,
    CdAudio,
    DiskNoise,
    /// The Covox or Disney Sound Source on the parallel port.
    LptDac,
    /// The Tandy's and PCjr's sound chip.
    Tandy,
}

pub const CHANNELS: usize = 10;

/// The loudest volume, in percent.
pub const MAX_LEVEL: u16 = 200;

impl Channel {
    pub const ALL: [Channel; CHANNELS] = [
        Channel::Master,
        Channel::Speaker,
        Channel::Sb,
        Channel::Fm,
        Channel::Gus,
        Channel::Midi,
        Channel::CdAudio,
        Channel::DiskNoise,
        Channel::LptDac,
        Channel::Tandy,
    ];

    /// Its key in `[mixer]`.
    pub fn key(self) -> &'static str {
        match self {
            Channel::Master => "master",
            Channel::Speaker => "speaker",
            Channel::Sb => "sb",
            Channel::Fm => "fm",
            Channel::Gus => "gus",
            Channel::Midi => "midi",
            Channel::CdAudio => "cdaudio",
            Channel::DiskNoise => "disknoise",
            Channel::LptDac => "lptdac",
            Channel::Tandy => "tandy",
        }
    }

    pub fn parse(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|channel| channel.key().eq_ignore_ascii_case(key.trim()))
    }

    /// Its name, as the settings window shows it.
    pub fn label(self) -> &'static str {
        match self {
            Channel::Master => "Master",
            Channel::Speaker => "PC speaker",
            Channel::Sb => "Sound Blaster",
            Channel::Fm => "FM synthesizer",
            Channel::Gus => "Gravis Ultrasound",
            Channel::Midi => "MIDI",
            Channel::CdAudio => "CD audio",
            Channel::DiskNoise => "Disk noise",
            Channel::LptDac => "Covox/Disney",
            Channel::Tandy => "Tandy/PCjr",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// The Sound Blaster's output filter (`sb_filter`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SbFilter {
    /// The low-pass of the model: 4.8 kHz on an SB 2.0, 3.2 kHz on an SB
    /// Pro, and half the sample rate on an SB16.
    Auto,
    Off,
}

impl SbFilter {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" | "on" | "true" => Some(SbFilter::Auto),
            "off" | "false" | "none" => Some(SbFilter::Off),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            SbFilter::Auto => "auto",
            SbFilter::Off => "off",
        }
    }
}

/// The volumes in percent, 0 to `MAX_LEVEL`, and the filters and effects.
/// At 100, the default, a source plays as loud as its card makes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MixerSettings {
    levels: [u16; CHANNELS],
    /// The PC speaker's filter, which gives it the sound of the small
    /// speaker of a PC (`speaker_filter`).
    pub speaker_filter: bool,
    pub sb_filter: SbFilter,
    /// The reverb and chorus the FM synthesizer, the Gravis Ultrasound and
    /// MIDI are sent to.
    pub reverb: ReverbPreset,
    pub chorus: ChorusPreset,
}

impl Default for MixerSettings {
    fn default() -> Self {
        Self {
            levels: [100; CHANNELS],
            speaker_filter: true,
            sb_filter: SbFilter::Auto,
            reverb: ReverbPreset::Off,
            chorus: ChorusPreset::Off,
        }
    }
}

impl MixerSettings {
    pub fn level(&self, channel: Channel) -> u16 {
        self.levels[channel.index()]
    }

    /// Set a volume, at most `MAX_LEVEL`.
    pub fn set_level(&mut self, channel: Channel, percent: u16) {
        self.levels[channel.index()] = percent.min(MAX_LEVEL);
    }
}

/// A volume as written: a number of percent, with or without the %.
pub fn parse_level(value: &str) -> Result<u16, String> {
    let number = value.trim().trim_end_matches('%').trim_end();
    match number.parse::<u16>() {
        Ok(percent) if percent <= MAX_LEVEL => Ok(percent),
        _ => Err(format!("invalid volume '{}' (0 to {})", value.trim(), MAX_LEVEL)),
    }
}

/// The filters' and effects' state.
#[derive(Clone, Debug, Default)]
struct Dsp {
    speaker: SpeakerFilter,
    sb: StereoLowpass,
    reverb: Option<Reverb>,
    chorus: Option<Chorus>,
}

/// The mixer as it plays: the volumes as gains, whether the output is
/// muted, how loud each source was lately, and each source's sends to the
/// effects, crossfeed and line-out, which the MIXER command sets.
#[derive(Clone, Debug)]
pub struct Mixer {
    settings: MixerSettings,
    gains: [f32; CHANNELS],
    /// Silence at the output device. Recordings still get the sound.
    pub muted: bool,
    /// Fast forwarding, whose sound, too fast and too much of it, the
    /// output device doesn't get either.
    pub fast_forward: bool,
    /// The loudest sample of each source since `take_peaks`, after its
    /// volume, as a fraction of full scale; `Master`'s is the mix's.
    peaks: [f32; CHANNELS],
    /// How much of each source goes into the reverb and the chorus, 0 to 1.
    reverb_sends: [f32; CHANNELS],
    chorus_sends: [f32; CHANNELS],
    /// How much of each source's left channel goes to the right and back,
    /// 0 to 1 (mono), and whether its channels are swapped.
    crossfeed: [f32; CHANNELS],
    reverse: [bool; CHANNELS],
    dsp: Dsp,
}

impl Default for Mixer {
    fn default() -> Self {
        Self {
            settings: MixerSettings::default(),
            gains: [1.0; CHANNELS],
            muted: false,
            fast_forward: false,
            peaks: [0.0; CHANNELS],
            reverb_sends: [0.0; CHANNELS],
            chorus_sends: [0.0; CHANNELS],
            crossfeed: [0.0; CHANNELS],
            reverse: [false; CHANNELS],
            dsp: Dsp::default(),
        }
    }
}

/// The sends of a preset: the synthesizers, FM, the Gravis Ultrasound and
/// MIDI, at `level`, the other sources dry.
fn synth_sends(level: f32) -> [f32; CHANNELS] {
    let mut sends = [0.0; CHANNELS];
    for channel in [Channel::Fm, Channel::Gus, Channel::Midi] {
        sends[channel.index()] = level;
    }
    sends
}

/// One frame of the mix on its way: the sources added up, and what they
/// send into the reverb and the chorus.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct MixFrame {
    dry: (f32, f32),
    reverb: (f32, f32),
    chorus: (f32, f32),
}

impl Mixer {
    /// Take new settings. A new reverb or chorus starts afresh, with the
    /// preset's sends; a filter switched on or off starts from silence.
    pub fn set(&mut self, settings: MixerSettings) {
        let old = std::mem::replace(&mut self.settings, settings);
        self.gains = settings.levels.map(|percent| percent as f32 / 100.0);
        if settings.reverb != old.reverb {
            self.dsp.reverb = (settings.reverb != ReverbPreset::Off).then(|| Reverb::new(settings.reverb));
            self.reverb_sends = synth_sends(settings.reverb.synth_send());
        }
        if settings.chorus != old.chorus {
            self.dsp.chorus = (settings.chorus != ChorusPreset::Off).then(Chorus::new);
            self.chorus_sends = synth_sends(settings.chorus.synth_send());
        }
        if settings.speaker_filter != old.speaker_filter {
            self.dsp.speaker = SpeakerFilter::default();
        }
        if settings.sb_filter != old.sb_filter {
            self.dsp.sb = StereoLowpass::default();
        }
    }

    /// A source's send to the reverb, 0 to 1.
    pub fn reverb_send(&self, channel: Channel) -> f32 {
        self.reverb_sends[channel.index()]
    }

    pub fn set_reverb_send(&mut self, channel: Channel, level: f32) {
        self.reverb_sends[channel.index()] = level.clamp(0.0, 1.0);
    }

    /// A source's send to the chorus, 0 to 1.
    pub fn chorus_send(&self, channel: Channel) -> f32 {
        self.chorus_sends[channel.index()]
    }

    pub fn set_chorus_send(&mut self, channel: Channel, level: f32) {
        self.chorus_sends[channel.index()] = level.clamp(0.0, 1.0);
    }

    /// How much a source's channels are mixed into each other, 0 to 1.
    pub fn crossfeed(&self, channel: Channel) -> f32 {
        self.crossfeed[channel.index()]
    }

    pub fn set_crossfeed(&mut self, channel: Channel, strength: f32) {
        self.crossfeed[channel.index()] = strength.clamp(0.0, 1.0);
    }

    /// Whether a source's left and right are swapped.
    pub fn reverse(&self, channel: Channel) -> bool {
        self.reverse[channel.index()]
    }

    pub fn set_reverse(&mut self, channel: Channel, reverse: bool) {
        self.reverse[channel.index()] = reverse;
    }

    /// Mix a source's stereo frame into `frame`, at its channel's gain,
    /// with its line-out and crossfeed, sending it to the effects, and note
    /// its peak.
    #[inline]
    pub(crate) fn add(&self, frame: &mut MixFrame, peaks: &mut [f32; CHANNELS], channel: Channel, (l, r): (f32, f32)) {
        let i = channel.index();
        let gain = self.gains[i];
        let (mut l, mut r) = (l * gain, r * gain);
        if self.reverse[i] {
            std::mem::swap(&mut l, &mut r);
        }
        let x = self.crossfeed[i] / 2.0;
        if x > 0.0 {
            (l, r) = (l * (1.0 - x) + r * x, r * (1.0 - x) + l * x);
        }
        let peak = &mut peaks[i];
        *peak = peak.max(l.abs()).max(r.abs());
        frame.dry.0 += l;
        frame.dry.1 += r;
        let send = self.reverb_sends[i];
        if send > 0.0 {
            frame.reverb.0 += l * send;
            frame.reverb.1 += r * send;
        }
        let send = self.chorus_sends[i];
        if send > 0.0 {
            frame.chorus.0 += l * send;
            frame.chorus.1 += r * send;
        }
    }

    /// The PC speaker's sample through its filter, if it is on.
    #[inline]
    pub(crate) fn speaker_filter(&mut self, sample: f32) -> f32 {
        if self.settings.speaker_filter { self.dsp.speaker.process(sample) } else { sample }
    }

    /// The Sound Blaster's frame through its low-pass at `cutoff` Hz, of
    /// `order` 2 or 4.
    #[inline]
    pub(crate) fn sb_filter(&mut self, frame: (f32, f32), cutoff: f32, order: usize) -> (f32, f32) {
        self.dsp.sb.process(frame, cutoff, order)
    }

    /// The mix of a frame: the sources with the reverb and chorus of what
    /// they sent, at the master volume. Notes the mix's peak.
    #[inline]
    pub(crate) fn finish(&mut self, frame: MixFrame, peaks: &mut [f32; CHANNELS]) -> (f32, f32) {
        let (mut l, mut r) = frame.dry;
        if let Some(reverb) = &mut self.dsp.reverb {
            let (a, b) = reverb.process(frame.reverb);
            l += a;
            r += b;
        }
        if let Some(chorus) = &mut self.dsp.chorus {
            let (a, b) = chorus.process(frame.chorus);
            l += a;
            r += b;
        }
        let gain = self.gains[Channel::Master.index()];
        let (l, r) = (l * gain, r * gain);
        let peak = &mut peaks[Channel::Master.index()];
        *peak = peak.max(l.abs()).max(r.abs());
        (l, r)
    }

    pub fn settings(&self) -> MixerSettings {
        self.settings
    }

    /// The gains of all channels, by `Channel as usize`.
    pub fn gains(&self) -> [f32; CHANNELS] {
        self.gains
    }

    /// Take in the loudest samples of a stretch of output (full scale is
    /// 32768), as `gains` weighed them.
    pub fn add_peaks(&mut self, peaks: [f32; CHANNELS]) {
        for (peak, new) in self.peaks.iter_mut().zip(peaks) {
            *peak = peak.max(new / 32768.0);
        }
    }

    /// How loud each source was since the last call, 0 to 1 and above
    /// where it clips.
    pub fn take_peaks(&mut self) -> [f32; CHANNELS] {
        std::mem::take(&mut self.peaks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_parse_back() {
        for channel in Channel::ALL {
            assert_eq!(Channel::parse(channel.key()), Some(channel));
        }
        assert_eq!(Channel::parse(" CDAudio "), Some(Channel::CdAudio));
        assert_eq!(Channel::parse("opl"), None);
    }

    #[test]
    fn levels() {
        assert_eq!(parse_level("80"), Ok(80));
        assert_eq!(parse_level(" 150 % "), Ok(150));
        assert_eq!(parse_level("0"), Ok(0));
        assert!(parse_level("201").is_err());
        assert!(parse_level("-5").is_err());
        assert!(parse_level("loud").is_err());
        let mut settings = MixerSettings::default();
        settings.set_level(Channel::Fm, 500);
        assert_eq!(settings.level(Channel::Fm), MAX_LEVEL);
        assert_eq!(settings.level(Channel::Sb), 100);
    }

    #[test]
    fn gains_and_peaks() {
        let mut mixer = Mixer::default();
        let mut settings = MixerSettings::default();
        settings.set_level(Channel::Speaker, 50);
        settings.set_level(Channel::Master, 200);
        mixer.set(settings);
        let mut peaks = [0.0; CHANNELS];
        let mut mix = MixFrame::default();
        mixer.add(&mut mix, &mut peaks, Channel::Speaker, (1000.0, -1000.0));
        mixer.add(&mut mix, &mut peaks, Channel::Fm, (100.0, 300.0));
        assert_eq!(mix.dry, (600.0, -200.0));
        assert_eq!(mixer.finish(mix, &mut peaks), (1200.0, -400.0));
        assert_eq!(peaks[Channel::Speaker.index()], 500.0);
        assert_eq!(peaks[Channel::Fm.index()], 300.0);
        assert_eq!(peaks[Channel::Master.index()], 1200.0);
        mixer.add_peaks(peaks);
        let taken = mixer.take_peaks();
        assert_eq!(taken[Channel::Master.index()], 1200.0 / 32768.0);
        assert_eq!(mixer.take_peaks(), [0.0; CHANNELS]);
    }

    #[test]
    fn presets_send_the_synthesizers_and_line_out_changes_sides() {
        let mut mixer = Mixer::default();
        let mut settings = MixerSettings::default();
        settings.reverb = ReverbPreset::Large;
        settings.chorus = ChorusPreset::Light;
        mixer.set(settings);
        assert_eq!(mixer.reverb_send(Channel::Fm), 0.70);
        assert_eq!(mixer.reverb_send(Channel::Gus), 0.70);
        assert_eq!(mixer.reverb_send(Channel::Midi), 0.70);
        assert_eq!(mixer.reverb_send(Channel::Sb), 0.0);
        assert_eq!(mixer.chorus_send(Channel::Gus), 0.33);
        assert_eq!(mixer.chorus_send(Channel::Midi), 0.33);
        // Sends set by hand stay while the preset does.
        mixer.set_reverb_send(Channel::Sb, 0.2);
        settings.set_level(Channel::Sb, 50);
        mixer.set(settings);
        assert_eq!(mixer.reverb_send(Channel::Sb), 0.2);

        let mut peaks = [0.0; CHANNELS];
        let mut mix = MixFrame::default();
        mixer.set_reverse(Channel::Gus, true);
        mixer.add(&mut mix, &mut peaks, Channel::Gus, (100.0, 0.0));
        assert_eq!(mix.dry, (0.0, 100.0));
        let mut mix = MixFrame::default();
        mixer.set_reverse(Channel::Gus, false);
        mixer.set_crossfeed(Channel::Gus, 1.0);
        mixer.add(&mut mix, &mut peaks, Channel::Gus, (100.0, 0.0));
        assert_eq!(mix.dry, (50.0, 50.0));
    }

    #[test]
    fn filter_names_parse_back() {
        for filter in [SbFilter::Auto, SbFilter::Off] {
            assert_eq!(SbFilter::parse(filter.name()), Some(filter));
        }
        assert_eq!(SbFilter::parse("on"), Some(SbFilter::Auto));
    }
}
