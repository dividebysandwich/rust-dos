//! The MIXER command, as DOSBox Staging has it: it shows the mixer, and
//! sets the volume, line-out, crossfeed, reverb and chorus of its channels,
//! several of them in one command line, as `[autoexec]` wants:
//!
//! ```text
//! MIXER [CHANNEL] COMMANDS [/NOSHOW]
//! MIXER CDAUDIO 50 SB REVERSE /NOSHOW
//! MIXER X30 MASTER 40 OPL 150 R50 C30 SB X10
//! ```
//!
//! The volumes and the reverb and chorus presets are the settings'
//! (`[mixer]`), which the frontends take over (`Bus::mixer_changed`), so
//! the settings window shows them and saves them; the sends, crossfeed and
//! line-out are the mixer's as it plays, as in DOSBox Staging.

use crate::command::ShellCommand;
use crate::cpu::Cpu;
use crate::mixer::{Channel, ChorusPreset, MAX_LEVEL, ReverbPreset};
use crate::video::{print_cp437, print_string};

/// The channels MIXER knows by name, DOSBox Staging's names among them.
fn channel_named(name: &str) -> Option<Channel> {
    Some(match name {
        "MASTER" => Channel::Master,
        "SPEAKER" | "PCSPEAKER" | "SPKR" => Channel::Speaker,
        "SB" | "SBLASTER" => Channel::Sb,
        "FM" | "OPL" => Channel::Fm,
        "GUS" => Channel::Gus,
        "MIDI" | "FSYNTH" | "MT32" | "SOUNDCANVAS" => Channel::Midi,
        "CDAUDIO" | "CDDA" => Channel::CdAudio,
        "DISKNOISE" | "HDDNOISE" | "FDDNOISE" => Channel::DiskNoise,
        "LPTDAC" | "DISNEY" | "COVOX" => Channel::LptDac,
        _ => return None,
    })
}

/// The name MIXER shows for a channel.
fn display_name(channel: Channel) -> &'static str {
    match channel {
        Channel::Master => "MASTER",
        Channel::Speaker => "PCSPEAKER",
        Channel::Sb => "SB",
        Channel::Fm => "OPL",
        Channel::Gus => "GUS",
        Channel::Midi => "MIDI",
        Channel::CdAudio => "CDAUDIO",
        Channel::DiskNoise => "DISKNOISE",
        Channel::LptDac => "LPTDAC",
    }
}

/// Whether a channel has two sides, which line-out and crossfeed need.
fn stereo(channel: Channel) -> bool {
    !matches!(channel, Channel::Master | Channel::Speaker | Channel::DiskNoise | Channel::LptDac)
}

/// The channels that play here: the master, the speaker, and the sound
/// devices there are.
pub fn active_channels(cpu: &Cpu) -> Vec<Channel> {
    let bus = &cpu.bus;
    Channel::ALL
        .into_iter()
        .filter(|&channel| match channel {
            Channel::Sb => bus.sb.is_some(),
            Channel::Gus => bus.gus.is_some(),
            Channel::LptDac => bus.lpt_dac.is_some(),
            Channel::DiskNoise => bus.disknoise.enabled(crate::diskio::DiskClass::HardDisk)
                || bus.disknoise.enabled(crate::diskio::DiskClass::Floppy),
            _ => true,
        })
        .collect()
}

/// What a command line asks for, one change at a time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Op {
    /// The channel the next commands are for; None for all of them.
    Select(Option<Channel>),
    Volume(u16),
    Reverse(bool),
    Crossfeed(f32),
    Reverb(f32),
    Chorus(f32),
}

/// A command line's changes, and whether to show the mixer after them.
#[derive(Clone, Debug, PartialEq)]
pub struct Parsed {
    pub ops: Vec<Op>,
    pub show: bool,
    pub help: bool,
}

/// A volume: percent (0 to 200) or decibels after a D, or two of them as
/// L:R, which take the average: a channel here has one volume.
fn parse_volume(text: &str) -> Option<u16> {
    let one = |s: &str| -> Option<f32> {
        let percent = match s.strip_prefix('D') {
            Some(db) => {
                let db: f32 = db.parse().ok().filter(|d: &f32| (-96.0..=40.0).contains(d))?;
                100.0 * 10f32.powf(db / 20.0)
            }
            None => s.parse::<f32>().ok().filter(|p| *p >= 0.0)?,
        };
        Some(percent.min(MAX_LEVEL as f32))
    };
    let parts: Vec<&str> = text.split(':').collect();
    let percent = match parts.as_slice() {
        [v] => one(v)?,
        [l, r] => (one(l)? + one(r)?) / 2.0,
        _ => return None,
    };
    Some(percent.round() as u16)
}

/// A level after its letter (X, R or C), 0 to 100, as a fraction.
fn parse_level(text: &str) -> Option<f32> {
    text.parse::<f32>().ok().filter(|l| (0.0..=100.0).contains(l)).map(|l| l / 100.0)
}

/// Whether `arg` is the command of letter `prefix`: the letter alone, or
/// followed by a number.
fn has_prefix(arg: &str, prefix: char) -> bool {
    let mut chars = arg.chars();
    chars.next() == Some(prefix) && chars.next().is_none_or(|c| c.is_ascii_digit() || c == '-' || c == '+' || c == '.')
}

/// Parse a command line, the channels there are being `active`. The
/// errors are DOSBox Staging's.
pub fn parse(args: &str, active: &[Channel]) -> Result<Parsed, String> {
    let mut parsed = Parsed { ops: Vec::new(), show: true, help: false };
    let mut current: Option<Channel> = None;
    let mut count = 0;
    let missing = |channel: Option<Channel>| match channel {
        Some(c) => format!("Missing command for the {} channel", display_name(c)),
        None => "Missing command".to_string(),
    };
    for arg in args.split_whitespace() {
        let arg = arg.to_ascii_uppercase();
        match arg.as_str() {
            "/NOSHOW" => {
                parsed.show = false;
                continue;
            }
            "/?" | "/HELP" | "-?" | "--HELP" => {
                parsed.help = true;
                continue;
            }
            _ => {}
        }
        let name = |c: Option<Channel>| c.map_or("", display_name);
        let invalid = |current: Option<Channel>, arg: &str| match current {
            None => format!("Invalid global command: {}", arg),
            Some(c) => format!("Invalid command for the {} channel: {}", display_name(c), arg),
        };
        if let Some(channel) = channel_named(&arg) {
            if !active.contains(&channel) {
                return Err(format!("Channel {} is not active", arg));
            }
            if current.is_some() && count == 0 {
                return Err(missing(current));
            }
            current = Some(channel);
            count = 0;
            parsed.ops.push(Op::Select(current));
            continue;
        }
        let first = arg.chars().next().unwrap_or(' ');
        let op = if first.is_ascii_digit() || first == '+' || first == '-' || first == '.' || (first == 'D' && arg.len() > 1) {
            if current.is_none() {
                return Err(invalid(current, &arg));
            }
            let volume = parse_volume(&arg).ok_or_else(|| format!("Invalid volume for the {} channel: {}", name(current), arg))?;
            Op::Volume(volume)
        } else if arg == "STEREO" || arg == "REVERSE" {
            match current {
                Some(c) if stereo(c) => Op::Reverse(arg == "REVERSE"),
                _ => return Err(invalid(current, &arg)),
            }
        } else if let Some((letter, what, make)) = [('X', "crossfeed strength", Op::Crossfeed as fn(f32) -> Op), ('R', "reverb level", Op::Reverb), ('C', "chorus level", Op::Chorus)]
            .into_iter()
            .find(|&(letter, _, _)| has_prefix(&arg, letter))
        {
            let level = &arg[1..];
            let (global, channel) = match current {
                None => ("global ", "".to_string()),
                Some(c) => ("", format!(" for the {} channel", display_name(c))),
            };
            if current == Some(Channel::Master) || (letter == 'X' && current.is_some_and(|c| !stereo(c))) {
                return Err(invalid(current, &arg));
            }
            if level.is_empty() {
                return Err(format!("Missing {}{} after {}{}; must provide a number between 0 and 100", global, what, letter, channel));
            }
            let level = parse_level(level)
                .ok_or_else(|| format!("Invalid {}{}{}: {}; must be a number between 0 and 100", global, what, channel, arg))?;
            make(level)
        } else {
            return Err(invalid(current, &arg));
        };
        parsed.ops.push(op);
        count += 1;
    }
    if current.is_some() && count == 0 {
        return Err(missing(current));
    }
    Ok(parsed)
}

/// Carry out the changes: the volumes and presets through the settings,
/// then the mixer's sends, crossfeed and line-out.
pub fn apply(cpu: &mut Cpu, ops: &[Op]) {
    let active = active_channels(cpu);
    let mut settings = cpu.bus.mixer.settings();
    // Reverb and chorus come on with their default presets when a channel
    // is sent to them, as in DOSBox Staging.
    if ops.iter().any(|op| matches!(op, Op::Reverb(_))) && settings.reverb == ReverbPreset::Off {
        settings.reverb = ReverbPreset::Medium;
    }
    if ops.iter().any(|op| matches!(op, Op::Chorus(_))) && settings.chorus == ChorusPreset::Off {
        settings.chorus = ChorusPreset::Normal;
    }
    let mut current = None;
    for &op in ops {
        match op {
            Op::Select(channel) => current = channel,
            Op::Volume(percent) => {
                if let Some(channel) = current {
                    settings.set_level(channel, percent);
                }
            }
            _ => {}
        }
    }
    if settings != cpu.bus.mixer.settings() {
        cpu.bus.set_mixer(settings);
        cpu.bus.mixer_changed = true;
    }
    // Without a channel, every channel that has it.
    let channels = |current: Option<Channel>| -> Vec<Channel> {
        match current {
            Some(channel) => vec![channel],
            None => active.iter().copied().filter(|&c| c != Channel::Master).collect(),
        }
    };
    let mixer = &mut cpu.bus.mixer;
    let mut current = None;
    for &op in ops {
        match op {
            Op::Select(channel) => current = channel,
            Op::Reverse(reverse) => current.into_iter().for_each(|c| mixer.set_reverse(c, reverse)),
            Op::Crossfeed(level) => channels(current).into_iter().filter(|&c| stereo(c)).for_each(|c| mixer.set_crossfeed(c, level)),
            Op::Reverb(level) => channels(current).into_iter().for_each(|c| mixer.set_reverb_send(c, level)),
            Op::Chorus(level) => channels(current).into_iter().for_each(|c| mixer.set_chorus_send(c, level)),
            Op::Volume(_) => {}
        }
    }
}

/// The mixer's channels as a table: volume in percent and decibels,
/// line-out, crossfeed, reverb and chorus.
pub fn table(cpu: &Cpu) -> Vec<(String, Channel)> {
    let mixer = &cpu.bus.mixer;
    let settings = mixer.settings();
    let row = |channel: Channel| {
        let level = settings.level(channel);
        let db = match level {
            0 => "-inf".to_string(),
            l => format!("{:+.2}", 20.0 * (l as f32 / 100.0).log10()),
        };
        let percent_or_off = |v: f32| if v > 0.0 { format!("{:.0}", v * 100.0) } else { "off".to_string() };
        let (mode, xfeed, reverb, chorus) = match channel {
            Channel::Master => ("Stereo".to_string(), "-".to_string(), "-".to_string(), "-".to_string()),
            c => (
                if !stereo(c) {
                    "Mono"
                } else if mixer.reverse(c) {
                    "Reverse"
                } else {
                    "Stereo"
                }
                .to_string(),
                if stereo(c) { percent_or_off(mixer.crossfeed(c)) } else { "-".to_string() },
                percent_or_off(mixer.reverb_send(c)),
                percent_or_off(mixer.chorus_send(c)),
            ),
        };
        format!(
            "{:<12} {:>4}:{:<4} {:>6}:{:<6}  {:<8} {:>5} {:>7} {:>7}",
            display_name(channel),
            level,
            level,
            db,
            db,
            mode,
            xfeed,
            reverb,
            chorus
        )
    };
    active_channels(cpu).into_iter().map(|c| (row(c), c)).collect()
}

const HELP: &str = "Displays or changes the sound mixer settings.\r\n\
\r\n\
MIXER [CHANNEL] COMMANDS [/NOSHOW]\r\n\
\r\n\
  CHANNEL   the channel to change: MASTER, PCSPEAKER, SB, OPL, GUS, MIDI,\r\n\
            CDAUDIO, DISKNOISE or LPTDAC\r\n\
  COMMANDS  one or more of these:\r\n\
    Volume     0 to 200 percent, or decibels after a D (D-6); L:R sets the\r\n\
               two sides, which have one volume here: their average\r\n\
    Line-out   STEREO or REVERSE (stereo channels only)\r\n\
    Crossfeed  X0 to X100 (stereo channels only)\r\n\
    Reverb     R0 to R100\r\n\
    Chorus     C0 to C100\r\n\
  /NOSHOW   makes the changes without showing the mixer\r\n\
\r\n\
MIXER alone shows the mixer. Several channels can change at once, and X, R\r\n\
and C without a channel change them all.\r\n\
\r\n\
Examples:\r\n\
  MIXER CDAUDIO 50 SB REVERSE /NOSHOW\r\n\
  MIXER X30 MASTER 40 OPL 150 R50 C30 SB X10\r\n";

/// Show the mixer's table: the heading in white, the channels' names in
/// light cyan.
fn show(cpu: &mut Cpu) {
    let heading = format!(
        "{:<12} {:>9} {:>13}  {:<8} {:>5} {:>7} {:>7}",
        "Channel", "Volume", "Volume (dB)", "Mode", "Xfeed", "Reverb", "Chorus"
    );
    print_cp437(cpu, heading.as_bytes(), 0x0F);
    print_string(cpu, "\r\n");
    for (line, channel) in table(cpu) {
        let name = display_name(channel);
        print_cp437(cpu, name.as_bytes(), 0x0B);
        print_string(cpu, &line[name.len()..]);
        print_string(cpu, "\r\n");
    }
}

/// MIXER at the prompt.
pub struct MixerCommand;

impl ShellCommand for MixerCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        let active = active_channels(cpu);
        match parse(args, &active) {
            Err(e) => {
                print_cp437(cpu, format!("MIXER: {}", e).as_bytes(), 0x0C);
                print_string(cpu, "\r\n");
            }
            Ok(parsed) if parsed.help => print_string(cpu, HELP),
            Ok(parsed) => {
                apply(cpu, &parsed.ops);
                if parsed.show {
                    show(cpu);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Channel; 9] = Channel::ALL;

    #[test]
    fn volumes_in_percent_decibels_and_sides() {
        assert_eq!(parse_volume("50"), Some(50));
        assert_eq!(parse_volume("250"), Some(200));
        assert_eq!(parse_volume("D-6"), Some(50));
        assert_eq!(parse_volume("D0"), Some(100));
        assert_eq!(parse_volume("10:30"), Some(20));
        assert_eq!(parse_volume("150:D6"), Some(175));
        assert_eq!(parse_volume("D-100"), None);
        assert_eq!(parse_volume("1:2:3"), None);
        assert_eq!(parse_volume("-5"), None);
    }

    #[test]
    fn channels_by_their_names_and_dosbox_staging_s() {
        let parsed = parse("opl 150 pcspeaker 40 cdda d-6 disney 20 master 80", &ALL).unwrap();
        assert_eq!(
            parsed.ops,
            [
                Op::Select(Some(Channel::Fm)),
                Op::Volume(150),
                Op::Select(Some(Channel::Speaker)),
                Op::Volume(40),
                Op::Select(Some(Channel::CdAudio)),
                Op::Volume(50),
                Op::Select(Some(Channel::LptDac)),
                Op::Volume(20),
                Op::Select(Some(Channel::Master)),
                Op::Volume(80),
            ]
        );
        assert!(parsed.show);
        assert!(!parse("sb 50 /noshow", &ALL).unwrap().show);
        assert!(parse("/?", &ALL).unwrap().help);
    }

    #[test]
    fn global_and_channel_effects() {
        let parsed = parse("x30 r50 opl 150 r20 c30 sb x10 reverse", &ALL).unwrap();
        assert_eq!(
            parsed.ops,
            [
                Op::Crossfeed(0.3),
                Op::Reverb(0.5),
                Op::Select(Some(Channel::Fm)),
                Op::Volume(150),
                Op::Reverb(0.2),
                Op::Chorus(0.3),
                Op::Select(Some(Channel::Sb)),
                Op::Crossfeed(0.1),
                Op::Reverse(true),
            ]
        );
    }

    #[test]
    fn mistakes_say_what_is_wrong() {
        let err = |args: &str| parse(args, &[Channel::Master, Channel::Speaker, Channel::Fm]).unwrap_err();
        assert_eq!(err("sb 50"), "Channel SB is not active");
        assert_eq!(err("opl"), "Missing command for the OPL channel");
        assert_eq!(err("opl pcspeaker 50"), "Missing command for the OPL channel");
        assert_eq!(err("50"), "Invalid global command: 50");
        assert_eq!(err("reverse"), "Invalid global command: REVERSE");
        assert_eq!(err("pcspeaker reverse"), "Invalid command for the PCSPEAKER channel: REVERSE");
        assert_eq!(err("pcspeaker x20"), "Invalid command for the PCSPEAKER channel: X20");
        assert_eq!(err("master r20"), "Invalid command for the MASTER channel: R20");
        assert_eq!(err("opl loud"), "Invalid command for the OPL channel: LOUD");
        assert_eq!(err("opl d-200"), "Invalid volume for the OPL channel: D-200");
        assert!(err("opl r").starts_with("Missing reverb level after R for the OPL channel"));
        assert!(err("c200").starts_with("Invalid global chorus level: C200"));
    }
}
