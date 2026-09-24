//! The host's mixer (`[mixer]`): a volume for each sound source and one for
//! them all. It sits on top of the emulated cards' own levels, such as the
//! Sound Blaster's mixer chip, which programs set and a new program resets.

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
}

pub const CHANNELS: usize = 8;

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
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// The volumes in percent, 0 to `MAX_LEVEL`. At 100, the default, a source
/// plays as loud as its card makes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MixerSettings {
    levels: [u16; CHANNELS],
}

impl Default for MixerSettings {
    fn default() -> Self {
        Self { levels: [100; CHANNELS] }
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

/// The mixer as it plays: the volumes as gains, whether the output is
/// muted, and how loud each source was lately.
#[derive(Clone, Debug)]
pub struct Mixer {
    settings: MixerSettings,
    gains: [f32; CHANNELS],
    /// Silence at the output device. Recordings still get the sound.
    pub muted: bool,
    /// The loudest sample of each source since `take_peaks`, after its
    /// volume, as a fraction of full scale; `Master`'s is the mix's.
    peaks: [f32; CHANNELS],
}

impl Default for Mixer {
    fn default() -> Self {
        Self { settings: MixerSettings::default(), gains: [1.0; CHANNELS], muted: false, peaks: [0.0; CHANNELS] }
    }
}

impl Mixer {
    pub fn set(&mut self, settings: MixerSettings) {
        self.settings = settings;
        self.gains = settings.levels.map(|percent| percent as f32 / 100.0);
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

/// Mix a source's stereo frame into `mix`, at its channel's gain, and
/// note its peak.
#[inline]
pub(crate) fn add(
    mix: &mut (f32, f32),
    peaks: &mut [f32; CHANNELS],
    gains: &[f32; CHANNELS],
    channel: Channel,
    (l, r): (f32, f32),
) {
    let gain = gains[channel.index()];
    let (l, r) = (l * gain, r * gain);
    let peak = &mut peaks[channel.index()];
    *peak = peak.max(l.abs()).max(r.abs());
    mix.0 += l;
    mix.1 += r;
}

/// Put the master volume on the mix, and note its peak.
#[inline]
pub(crate) fn master(mix: (f32, f32), peaks: &mut [f32; CHANNELS], gains: &[f32; CHANNELS]) -> (f32, f32) {
    let gain = gains[Channel::Master.index()];
    let (l, r) = (mix.0 * gain, mix.1 * gain);
    let peak = &mut peaks[Channel::Master.index()];
    *peak = peak.max(l.abs()).max(r.abs());
    (l, r)
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
        let gains = mixer.gains();
        let mut peaks = [0.0; CHANNELS];
        let mut mix = (0.0, 0.0);
        add(&mut mix, &mut peaks, &gains, Channel::Speaker, (1000.0, -1000.0));
        add(&mut mix, &mut peaks, &gains, Channel::Fm, (100.0, 300.0));
        assert_eq!(mix, (600.0, -200.0));
        assert_eq!(master(mix, &mut peaks, &gains), (1200.0, -400.0));
        assert_eq!(peaks[Channel::Speaker.index()], 500.0);
        assert_eq!(peaks[Channel::Fm.index()], 300.0);
        assert_eq!(peaks[Channel::Master.index()], 1200.0);
        mixer.add_peaks(peaks);
        let taken = mixer.take_peaks();
        assert_eq!(taken[Channel::Master.index()], 1200.0 / 32768.0);
        assert_eq!(mixer.take_peaks(), [0.0; CHANNELS]);
    }
}
