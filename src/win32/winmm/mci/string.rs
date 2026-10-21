//! MCI's command strings (mciSendStringA): a verb, a device's name and
//! the verb's keywords, made into the command they stand for, and what
//! it returns made into text. And the errors' texts
//! (mciGetErrorStringA).
//!
//! A device's name is an alias or the name a device was opened by, or a
//! device type or file that the command opens for itself (auto-open):
//! such a device closes when the command is done, or when its play
//! ends, and can't take "notify".

use super::*;

/// What follows a keyword.
#[derive(Clone, Copy)]
enum Arg {
    /// Nothing: the keyword is a flag.
    None,
    /// A number, colon-separated bytes ("2:30:00") as one.
    Int(Field),
    /// A string.
    Str(Field),
    /// The item of MCI_STATUS or MCI_GETDEVCAPS.
    Item(u32),
    /// A value of a field.
    Value(Field, u32),
    /// A time format's name, or its number.
    TimeFormat,
}

/// The fields of `Parms` the keywords set.
#[derive(Clone, Copy)]
enum Field {
    From,
    To,
    Track,
    Number,
    Tempo,
    Port,
    Audio,
    Type,
    Alias,
    FormatTag,
    Channels,
    SamplesPerSec,
    BitsPerSample,
    /// A value the device works out itself (waveaudio's bytes a second
    /// and alignment, its input and output).
    Ignored,
}

/// A keyword: its words, the flag it sets, and what follows it. Of two
/// that start alike, the longer comes first.
type Keyword = (&'static [&'static str], u32, Arg);

const COMMON: &[Keyword] = &[
    (&["notify"], MCI_NOTIFY, Arg::None),
    (&["wait"], MCI_WAIT, Arg::None),
    (&["test"], MCI_TEST, Arg::None),
];

const OPEN: &[Keyword] = &[
    (&["type"], MCI_OPEN_TYPE, Arg::Str(Field::Type)),
    (&["alias"], MCI_OPEN_ALIAS, Arg::Str(Field::Alias)),
    (&["shareable"], MCI_OPEN_SHAREABLE, Arg::None),
];

const PLAY: &[Keyword] = &[
    (&["from"], MCI_FROM, Arg::Int(Field::From)),
    (&["to"], MCI_TO, Arg::Int(Field::To)),
];

const SEEK: &[Keyword] = &[
    (&["to", "start"], MCI_SEEK_TO_START, Arg::None),
    (&["to", "end"], MCI_SEEK_TO_END, Arg::None),
    (&["to"], MCI_TO, Arg::Int(Field::To)),
];

const STATUS: &[Keyword] = &[
    (&["length"], MCI_STATUS_ITEM, Arg::Item(MCI_STATUS_LENGTH)),
    (
        &["position"],
        MCI_STATUS_ITEM,
        Arg::Item(MCI_STATUS_POSITION),
    ),
    (
        &["number", "of", "tracks"],
        MCI_STATUS_ITEM,
        Arg::Item(MCI_STATUS_NUMBER_OF_TRACKS),
    ),
    (&["mode"], MCI_STATUS_ITEM, Arg::Item(MCI_STATUS_MODE)),
    (
        &["media", "present"],
        MCI_STATUS_ITEM,
        Arg::Item(MCI_STATUS_MEDIA_PRESENT),
    ),
    (
        &["time", "format"],
        MCI_STATUS_ITEM,
        Arg::Item(MCI_STATUS_TIME_FORMAT),
    ),
    (&["ready"], MCI_STATUS_ITEM, Arg::Item(MCI_STATUS_READY)),
    (
        &["current", "track"],
        MCI_STATUS_ITEM,
        Arg::Item(MCI_STATUS_CURRENT_TRACK),
    ),
    (&["type"], MCI_STATUS_ITEM, Arg::Item(0x4001)),
    (&["tempo"], MCI_STATUS_ITEM, Arg::Item(0x4002)),
    (&["port"], MCI_STATUS_ITEM, Arg::Item(0x4003)),
    (&["slave"], MCI_STATUS_ITEM, Arg::Item(0x4007)),
    (&["master"], MCI_STATUS_ITEM, Arg::Item(0x4008)),
    (&["offset"], MCI_STATUS_ITEM, Arg::Item(0x4009)),
    (&["division", "type"], MCI_STATUS_ITEM, Arg::Item(0x400A)),
    (&["start"], MCI_STATUS_START, Arg::None),
    (&["track"], MCI_TRACK, Arg::Int(Field::Track)),
    // waveaudio's.
    (&["format", "tag"], MCI_STATUS_ITEM, Arg::Item(0x4001)),
    (&["channels"], MCI_STATUS_ITEM, Arg::Item(0x4002)),
    (&["samplespersec"], MCI_STATUS_ITEM, Arg::Item(0x4003)),
    (&["bytespersec"], MCI_STATUS_ITEM, Arg::Item(0x4004)),
    (&["alignment"], MCI_STATUS_ITEM, Arg::Item(0x4005)),
    (&["bitspersample"], MCI_STATUS_ITEM, Arg::Item(0x4006)),
    (&["level"], MCI_STATUS_ITEM, Arg::Item(0x4007)),
    (&["input"], MCI_STATUS_ITEM, Arg::Item(0x0040_0000)),
    (&["output"], MCI_STATUS_ITEM, Arg::Item(0x0080_0000)),
];

const SET: &[Keyword] = &[
    (&["time", "format"], MCI_SET_TIME_FORMAT, Arg::TimeFormat),
    (&["door", "open"], MCI_SET_DOOR_OPEN, Arg::None),
    (&["door", "closed"], MCI_SET_DOOR_CLOSED, Arg::None),
    (
        &["audio", "all"],
        MCI_SET_AUDIO,
        Arg::Value(Field::Audio, 0),
    ),
    (
        &["audio", "left"],
        MCI_SET_AUDIO,
        Arg::Value(Field::Audio, 1),
    ),
    (
        &["audio", "right"],
        MCI_SET_AUDIO,
        Arg::Value(Field::Audio, 2),
    ),
    (&["video"], MCI_SET_VIDEO, Arg::None),
    (&["on"], MCI_SET_ON, Arg::None),
    (&["off"], MCI_SET_OFF, Arg::None),
    (&["tempo"], 0x0001_0000, Arg::Int(Field::Tempo)),
    (
        &["port", "mapper"],
        0x0002_0000,
        Arg::Value(Field::Port, 65535),
    ),
    (
        &["port", "none"],
        0x0002_0000,
        Arg::Value(Field::Port, 65533),
    ),
    (&["port"], 0x0002_0000, Arg::Int(Field::Port)),
    // waveaudio's (MCI_WAVE_SET_*, MCI_WAVE_INPUT and _OUTPUT).
    (
        &["format", "tag", "pcm"],
        0x0001_0000,
        Arg::Value(Field::FormatTag, 1),
    ),
    (&["format", "tag"], 0x0001_0000, Arg::Int(Field::FormatTag)),
    (&["channels"], 0x0002_0000, Arg::Int(Field::Channels)),
    (
        &["samplespersec"],
        0x0004_0000,
        Arg::Int(Field::SamplesPerSec),
    ),
    (&["bytespersec"], 0x0008_0000, Arg::Int(Field::Ignored)),
    (&["alignment"], 0x0010_0000, Arg::Int(Field::Ignored)),
    (
        &["bitspersample"],
        0x0020_0000,
        Arg::Int(Field::BitsPerSample),
    ),
    (&["any", "input"], 0x0400_0000, Arg::None),
    (&["any", "output"], 0x0800_0000, Arg::None),
    (&["input"], 0x0040_0000, Arg::Int(Field::Ignored)),
    (&["output"], 0x0080_0000, Arg::Int(Field::Ignored)),
];

const CAPABILITY: &[Keyword] = &[
    (
        &["can", "record"],
        MCI_GETDEVCAPS_ITEM,
        Arg::Item(MCI_GETDEVCAPS_CAN_RECORD),
    ),
    (
        &["has", "audio"],
        MCI_GETDEVCAPS_ITEM,
        Arg::Item(MCI_GETDEVCAPS_HAS_AUDIO),
    ),
    (
        &["has", "video"],
        MCI_GETDEVCAPS_ITEM,
        Arg::Item(MCI_GETDEVCAPS_HAS_VIDEO),
    ),
    (
        &["device", "type"],
        MCI_GETDEVCAPS_ITEM,
        Arg::Item(MCI_GETDEVCAPS_DEVICE_TYPE),
    ),
    (
        &["uses", "files"],
        MCI_GETDEVCAPS_ITEM,
        Arg::Item(MCI_GETDEVCAPS_USES_FILES),
    ),
    (
        &["compound", "device"],
        MCI_GETDEVCAPS_ITEM,
        Arg::Item(MCI_GETDEVCAPS_COMPOUND_DEVICE),
    ),
    (
        &["can", "eject"],
        MCI_GETDEVCAPS_ITEM,
        Arg::Item(MCI_GETDEVCAPS_CAN_EJECT),
    ),
    (
        &["can", "play"],
        MCI_GETDEVCAPS_ITEM,
        Arg::Item(MCI_GETDEVCAPS_CAN_PLAY),
    ),
    (
        &["can", "save"],
        MCI_GETDEVCAPS_ITEM,
        Arg::Item(MCI_GETDEVCAPS_CAN_SAVE),
    ),
    // waveaudio's.
    (&["inputs"], MCI_GETDEVCAPS_ITEM, Arg::Item(0x4001)),
    (&["outputs"], MCI_GETDEVCAPS_ITEM, Arg::Item(0x4002)),
];

const INFO: &[Keyword] = &[
    (&["product"], MCI_INFO_PRODUCT, Arg::None),
    (&["file"], MCI_INFO_FILE, Arg::None),
    (&["upc"], MCI_INFO_MEDIA_UPC, Arg::None),
    (&["identity"], MCI_INFO_MEDIA_IDENTITY, Arg::None),
];

const SYSINFO: &[Keyword] = &[
    (&["quantity"], MCI_SYSINFO_QUANTITY, Arg::None),
    (&["open"], MCI_SYSINFO_OPEN, Arg::None),
    (&["name"], MCI_SYSINFO_NAME, Arg::Int(Field::Number)),
    (&["installname"], MCI_SYSINFO_INSTALLNAME, Arg::None),
];

const CUE: &[Keyword] = &[
    (&["input"], 0x0040_0000, Arg::None),
    (&["output"], 0x0080_0000, Arg::None),
];

/// The verbs: their commands and keywords.
const VERBS: &[(&str, u32, &[Keyword])] = &[
    ("open", MCI_OPEN, OPEN),
    ("close", MCI_CLOSE, &[]),
    ("play", MCI_PLAY, PLAY),
    ("seek", MCI_SEEK, SEEK),
    ("stop", MCI_STOP, &[]),
    ("pause", MCI_PAUSE, &[]),
    ("resume", MCI_RESUME, &[]),
    ("status", MCI_STATUS, STATUS),
    ("set", MCI_SET, SET),
    ("capability", MCI_GETDEVCAPS, CAPABILITY),
    ("info", MCI_INFO, INFO),
    ("sysinfo", MCI_SYSINFO, SYSINFO),
    ("break", MCI_BREAK, &[]),
    ("cue", MCI_CUE, CUE),
    ("record", MCI_RECORD, PLAY),
    ("save", MCI_SAVE, &[]),
    ("load", MCI_LOAD, &[]),
    ("delete", MCI_DELETE, PLAY),
    ("step", MCI_STEP, &[]),
    ("spin", MCI_SPIN, &[]),
    ("sound", MCI_SOUND, &[]),
];

/// MCI_SOUND: a system sound, by its name in WIN.INI's [sounds].
const MCI_SOUND: u32 = 0x0812;

/// The words of a command string; a quoted one may have spaces.
fn words(text: &str) -> Result<Vec<String>, u32> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    loop {
        while chars.next_if(|c| c.is_ascii_whitespace()).is_some() {}
        let Some(&c) = chars.peek() else {
            return Ok(out);
        };
        let mut word = String::new();
        if c == '"' {
            chars.next();
            loop {
                match chars.next() {
                    Some('"') => break,
                    Some(c) => word.push(c),
                    None => return Err(MCIERR_NO_CLOSING_QUOTE),
                }
            }
            if chars.peek().is_some_and(|c| !c.is_ascii_whitespace()) {
                return Err(MCIERR_EXTRA_CHARACTERS);
            }
        } else {
            while let Some(c) = chars.next_if(|c| !c.is_ascii_whitespace()) {
                word.push(c);
            }
        }
        out.push(word);
    }
}

/// A number of a command string: decimal, or up to four bytes with
/// colons between them.
fn number(word: &str) -> Result<u32, u32> {
    let parts: Vec<&str> = word.split(':').collect();
    if parts.len() > 4
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(MCIERR_BAD_INTEGER);
    }
    if parts.len() == 1 {
        return word.parse().map_err(|_| MCIERR_BAD_INTEGER);
    }
    let mut value = 0u32;
    for (k, p) in parts.iter().enumerate() {
        let b: u32 = p.parse().map_err(|_| MCIERR_BAD_INTEGER)?;
        if b > 255 {
            return Err(MCIERR_BAD_INTEGER);
        }
        value |= b << (8 * k);
    }
    Ok(value)
}

/// The time formats' names.
const TIME_FORMATS: &[(&[&str], u32)] = &[
    (&["milliseconds"], 0),
    (&["ms"], 0),
    (&["hms"], 1),
    (&["msf"], 2),
    (&["frames"], 3),
    (&["smpte", "24"], 4),
    (&["smpte", "25"], 5),
    (&["smpte", "30", "drop"], 7),
    (&["smpte", "30"], 6),
    (&["bytes"], 8),
    (&["samples"], 9),
    (&["tmsf"], 10),
    (&["song", "pointer"], 0x4001),
];

/// The name of a time format.
fn time_format_name(format: u32) -> String {
    match TIME_FORMATS.iter().find(|&&(_, f)| f == format && f != 0) {
        Some((words, _)) => words.join(" "),
        None if format == 0 => "milliseconds".to_string(),
        None => format.to_string(),
    }
}

/// Whether `words` start with the keyword's words.
fn starts_with(words: &[String], keyword: &[&str]) -> bool {
    words.len() >= keyword.len()
        && keyword
            .iter()
            .zip(words)
            .all(|(k, w)| w.eq_ignore_ascii_case(k))
}

/// The flags and parameters of the words after the device's name.
fn parse(mut rest: &[String], table: &[Keyword], p: &mut Parms) -> Result<u32, u32> {
    let mut flags = 0;
    while !rest.is_empty() {
        let Some(&(kw, flag, arg)) = table
            .iter()
            .chain(COMMON)
            .find(|(kw, _, _)| starts_with(rest, kw))
        else {
            return Err(MCIERR_UNRECOGNIZED_KEYWORD);
        };
        if flags & flag != 0 && !matches!(arg, Arg::Item(_)) {
            return Err(MCIERR_DUPLICATE_FLAGS);
        }
        flags |= flag;
        rest = &rest[kw.len()..];
        let set = |p: &mut Parms, field: Field, v: u32| match field {
            Field::From => p.from = v,
            Field::To => p.to = v,
            Field::Track => p.track = v,
            Field::Number => p.number = v,
            Field::Tempo => p.tempo = v,
            Field::Port => p.port = v,
            Field::Audio => p.audio = v,
            Field::FormatTag => p.format_tag = v,
            Field::Channels => p.channels = v,
            Field::SamplesPerSec => p.samples_per_sec = v,
            Field::BitsPerSample => p.bits_per_sample = v,
            Field::Type | Field::Alias | Field::Ignored => {}
        };
        match arg {
            Arg::None => {}
            Arg::Item(item) => {
                if p.item != 0 && p.item != item {
                    return Err(MCIERR_FLAGS_NOT_COMPATIBLE);
                }
                p.item = item;
            }
            Arg::Value(field, v) => set(p, field, v),
            Arg::Int(field) => {
                let Some(word) = rest.first() else {
                    return Err(MCIERR_MISSING_PARAMETER);
                };
                set(p, field, number(word)?);
                rest = &rest[1..];
            }
            Arg::Str(field) => {
                let Some(word) = rest.first() else {
                    return Err(MCIERR_MISSING_STRING_ARGUMENT);
                };
                match field {
                    Field::Type => p.open_type = word.clone(),
                    _ => p.alias = word.clone(),
                }
                rest = &rest[1..];
            }
            Arg::TimeFormat => {
                if let Some(&(words, f)) = TIME_FORMATS.iter().find(|(w, _)| starts_with(rest, w)) {
                    p.time_format = f;
                    rest = &rest[words.len()..];
                } else if let Some(word) = rest.first() {
                    p.time_format = number(word).map_err(|_| MCIERR_BAD_CONSTANT)?;
                    rest = &rest[1..];
                } else {
                    return Err(MCIERR_MISSING_PARAMETER);
                }
            }
        }
    }
    Ok(flags)
}

/// The text of what a command returned.
fn return_text(p: &Parms) -> Option<String> {
    let [b0, b1, b2, b3] = p.ret.to_le_bytes();
    Some(match p.ret_kind {
        Ret::None => return None,
        Ret::Int => p.ret.to_string(),
        Ret::Bool => if p.ret != 0 { "true" } else { "false" }.to_string(),
        Ret::Constant => constant_name(p.ret).map_or_else(|| p.ret.to_string(), str::to_string),
        Ret::TimeFormat => time_format_name(p.ret),
        Ret::Colon3 => format!("{:02}:{:02}:{:02}", b0, b1, b2),
        Ret::Colon4 => format!("{:02}:{:02}:{:02}:{:02}", b0, b1, b2, b3),
        Ret::Text => p.text.clone(),
    })
}

/// The names of the constants commands return.
fn constant_name(value: u32) -> Option<&'static str> {
    Some(match value {
        524 => "not ready",
        525 => "stopped",
        526 => "playing",
        527 => "recording",
        528 => "seeking",
        529 => "paused",
        530 => "open",
        513 => "vcr",
        514 => "videodisc",
        515 => "overlay",
        516 => "cdaudio",
        517 => "dat",
        518 => "scanner",
        519 => "animation",
        520 => "digitalvideo",
        521 => "other",
        522 => "waveaudio",
        523 => "sequencer",
        1088 => "audio",
        1089 => "other",
        1216 => "PPQN",
        1217 => "SMPTE 24 Frame",
        1218 => "SMPTE 25 Frame",
        1219 => "SMPTE 30 Drop Frame",
        1220 => "file",
        1221 => "midi",
        1222 => "smpte",
        _ => return None,
    })
}

/// mciSendStringA(command, return, size, callback window).
pub fn mci_send_string(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[at, ret, size, hwnd] = args else {
        unreachable!()
    };
    if at == 0 {
        return Some(MCIERR_MISSING_COMMAND_STRING);
    }
    let text = read_string(cpu, at, 1024)?;
    send_string(cpu, &text, ret, size, hwnd)
}

/// mciSendStringA's work: None when the thread waits for a play, or a
/// page isn't there.
fn send_string(cpu: &mut Cpu, text: &str, ret: u32, size: u32, hwnd: u32) -> Option<u32> {
    if ret != 0 && size != 0 {
        if !cpu.bus.guest_probe(ret, 1, true) {
            return None;
        }
        cpu.bus.guest_write_8(ret, 0);
    }
    let words = match words(text) {
        Ok(w) => w,
        Err(e) => return Some(e),
    };
    let Some(verb) = words.first() else {
        return Some(MCIERR_MISSING_COMMAND_STRING);
    };
    let Some(&(_, msg, table)) = VERBS.iter().find(|(v, _, _)| v.eq_ignore_ascii_case(verb)) else {
        return Some(MCIERR_UNRECOGNIZED_COMMAND);
    };
    let Some(name) = words.get(1) else {
        return Some(MCIERR_MISSING_DEVICE_NAME);
    };
    let mut p = Parms {
        callback: hwnd,
        ..Parms::default()
    };
    let mut flags = match parse(&words[2..], table, &mut p) {
        Ok(f) => f,
        Err(e) => return Some(e),
    };
    let mut id = 0;
    let error = match msg {
        MCI_SOUND => {
            // The system sounds play silently (wave.rs's PlaySound), and
            // one that isn't defined plays the default sound.
            if flags & MCI_NOTIFY != 0 {
                post(cpu, hwnd, MCI_NOTIFY_SUCCESSFUL, 0);
            }
            0
        }
        MCI_OPEN => {
            if name.eq_ignore_ascii_case("new") {
                if flags & MCI_OPEN_ALIAS == 0 {
                    return Some(MCIERR_NEW_REQUIRES_ALIAS);
                }
                if flags & MCI_OPEN_SHAREABLE != 0 {
                    return Some(MCIERR_UNSUPPORTED_FUNCTION);
                }
                if flags & MCI_OPEN_TYPE == 0 {
                    return Some(MCIERR_DEVICE_TYPE_REQUIRED);
                }
            } else if flags & MCI_OPEN_TYPE == 0 && type_of_name(name).is_some() {
                flags |= MCI_OPEN_TYPE;
                p.open_type = name.clone();
            } else {
                flags |= MCI_OPEN_ELEMENT;
                p.element = name.clone();
            }
            open(cpu, flags, &mut p, false)
        }
        MCI_SYSINFO => {
            let kind = if name.eq_ignore_ascii_case("all") {
                None
            } else if let Some(t) = type_of_name(name) {
                Some(t)
            } else if let Some(i) = find_name(cpu, name) {
                Some(device(cpu, i).kind)
            } else {
                return Some(MCIERR_INVALID_DEVICE_NAME);
            };
            sysinfo(cpu, kind, flags, &mut p)
        }
        _ if name.eq_ignore_ascii_case("all") => {
            if msg != MCI_CLOSE {
                return Some(MCIERR_CANNOT_USE_ALL);
            }
            close_all(cpu, flags, &mut p);
            0
        }
        _ => {
            let i = match find_name(cpu, name) {
                Some(i) => i,
                None => match auto_open(cpu, name, msg, flags) {
                    Ok(i) => i,
                    Err(e) => return Some(e),
                },
            };
            id = device(cpu, i).id;
            let auto = device(cpu, i).auto;
            let error = device_command(cpu, i, msg, flags, &mut p);
            // A device opened for the command closes, unless it plays.
            if auto
                && msg != MCI_CLOSE
                && let Some(i) = find(cpu, id)
                && busy_until(cpu, i).is_none()
            {
                close_device(cpu, i);
            }
            error
        }
    };
    if error != 0 {
        return Some(error);
    }
    // SYSINFO's answer needs somewhere to go; the others' are dropped.
    if msg == MCI_SYSINFO && ret == 0 {
        return Some(MCIERR_PARAM_OVERFLOW);
    }
    if let Some(text) = return_text(&p)
        && ret != 0
    {
        if text.len() as u32 >= size {
            return Some(MCIERR_PARAM_OVERFLOW);
        }
        write_text(cpu, ret, size, &text)?;
    }
    if wait_for_play(cpu, id, msg, flags) {
        return None;
    }
    Some(0)
}

/// A device that a command names by its type or file and that isn't
/// open: opened for the command.
fn auto_open(cpu: &mut Cpu, name: &str, msg: u32, flags: u32) -> Result<usize, u32> {
    let known = type_of_name(name).is_some()
        || name.contains('!')
        || name.rsplit_once('.').is_some_and(|(_, e)| !e.is_empty());
    if !known {
        return Err(MCIERR_INVALID_DEVICE_NAME);
    }
    if msg == MCI_CLOSE {
        return Err(MCIERR_INVALID_DEVICE_NAME);
    }
    if flags & MCI_NOTIFY != 0 {
        return Err(MCIERR_NOTIFY_ON_AUTO_OPEN);
    }
    let mut p = Parms::default();
    let open_flags = if type_of_name(name).is_some() {
        p.open_type = name.to_string();
        MCI_OPEN_TYPE
    } else {
        p.element = name.to_string();
        MCI_OPEN_ELEMENT
    };
    match open(cpu, open_flags, &mut p, true) {
        0 => find(cpu, p.ret).ok_or(MCIERR_INTERNAL),
        e => Err(e),
    }
}

const MCIERR_INTERNAL: u32 = 277;
const MCIERR_MISSING_DEVICE_NAME: u32 = 292;
const MCIERR_DEVICE_TYPE_REQUIRED: u32 = 287;

/// mciExecute(command): mciSendStringA without a return, TRUE when it
/// worked.
pub fn mci_execute(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let text = read_string(cpu, args[0], 1024)?;
    let error = send_string(cpu, &text, 0, 0, 0)?;
    Some((error == 0) as u32)
}

/// mciGetErrorStringA(error, text, size): TRUE when the error has a
/// text.
pub fn mci_get_error_string(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[error, at, size] = args else {
        unreachable!()
    };
    if at == 0 || size == 0 {
        return Some(FALSE);
    }
    let text = match error {
        0..=255 => super::super::error_text(error),
        _ => error_string(error),
    };
    let Some(text) = text else {
        write_text(cpu, at, size, "")?;
        return Some(FALSE);
    };
    write_text(cpu, at, size, text)?;
    Some(TRUE)
}

/// The texts of MCI's errors, as Windows 95 has them.
fn error_string(error: u32) -> Option<&'static str> {
    Some(match error {
        257 => "Invalid device ID. Use the ID given to the device when the device was opened.",
        259 => "The driver cannot recognize the specified command parameter.",
        261 => "The driver cannot recognize the specified command.",
        262 => {
            "There is a problem with your media device. Make sure it is working correctly \
             or contact the device manufacturer."
        }
        263 => "The specified device is not open or is not recognized by MCI.",
        264 => {
            "Not enough memory available for this task. Quit one or more applications to \
             increase available memory, and then try again."
        }
        265 => {
            "The device name is already being used as an alias by this application. Use a \
             unique alias."
        }
        266 => "There is an undetectable problem in loading the specified device driver.",
        267 => "No command was specified.",
        268 => {
            "The output string was too large to fit in the return buffer. Increase the size \
             of the buffer."
        }
        269 => {
            "The specified command requires a character-string parameter. Please provide \
             one."
        }
        270 => "The specified integer is invalid for this command.",
        271 => {
            "The device driver returned an invalid return type. Check with the device \
             manufacturer about obtaining a new driver."
        }
        272 => {
            "There is a problem with the device driver. Check with the device manufacturer \
             about obtaining a new driver."
        }
        273 => "The specified command requires a parameter. Please supply one.",
        274 => "The MCI device you are using does not support the specified command.",
        275 => "Cannot find the specified file. Make sure the path and filename are correct.",
        276 => "The device driver is not ready.",
        277 => "A problem occurred in initializing MCI. Try restarting Windows.",
        278 => {
            "There is a problem with the device driver. The driver has closed. Cannot \
             access error."
        }
        279 => "Cannot use 'all' as the device name with the specified command.",
        280 => {
            "Errors occurred in more than one device. Specify each command and device \
             separately to determine which devices caused the error."
        }
        281 => "Cannot determine the device type from the given filename extension.",
        282 => "The specified parameter is out of range for the specified command.",
        284 => "The specified parameters cannot be used together.",
        286 => {
            "Cannot save the specified file. Make sure you have enough disk space or are \
             still connected to the network."
        }
        287 => {
            "Cannot find the specified device. Make sure it is installed or that the device \
             name is spelled correctly."
        }
        288 => {
            "The specified device is now being closed. Wait a few seconds, and then try \
             again."
        }
        289 => "The specified alias is already being used in this application. Use a unique alias.",
        290 => "The specified parameter is invalid for this command.",
        291 => {
            "The device driver is already in use. To share it, use the 'shareable' \
             parameter with each 'open' command."
        }
        292 => {
            "The specified command requires an alias, file, driver, or device name. Please \
             supply one."
        }
        293 => {
            "The specified value for the time format is invalid. Refer to the MCI \
             documentation for valid formats."
        }
        294 => {
            "A closing double-quotation mark is missing from the parameter value. Please \
             supply one."
        }
        295 => "A parameter or value was specified twice. Only specify it once.",
        296 => {
            "The specified file cannot be played on the specified MCI device. The file may \
             be corrupt, or not in the correct format."
        }
        297 => "A null parameter block was passed to MCI.",
        298 => "Cannot save an unnamed file. Supply a filename.",
        299 => "You must specify an alias when using the 'new' parameter.",
        300 => "Cannot use the 'notify' flag with auto-opened devices.",
        301 => "Cannot use a filename with the specified device.",
        302 => {
            "Cannot carry out the commands in the order specified. Correct the command \
             sequence, and then try again."
        }
        303 => {
            "Cannot carry out the specified command on an auto-opened device. Wait until \
             the device is closed, and then try again."
        }
        304 => {
            "The filename is invalid. Make sure the filename is not longer than 8 \
             characters, followed by a period and an extension."
        }
        305 => "Cannot specify extra characters after a string enclosed in quotation marks.",
        306 => {
            "The specified device is not installed on the system. Use the Drivers option in \
             Control Panel to install the device."
        }
        307 => {
            "Cannot access the specified file or MCI device. Try changing directories or \
             restarting your computer."
        }
        308 => {
            "Cannot access the specified file or MCI device because the application cannot \
             change directories."
        }
        309 => {
            "Cannot access specified file or MCI device because the application cannot \
             change drives."
        }
        310 => "Specify a device or driver name that is less than 79 characters.",
        311 => "Specify a device or driver name that is less than 69 characters.",
        312 => "The specified command requires an integer parameter. Please provide one.",
        336 => {
            "The time formats of the 'song pointer' and SMPTE are mutually exclusive. You \
             can't use them together."
        }
        337 => "The specified MIDI port is already in use. Wait until it is free; then try again.",
        338 => {
            "The specified MIDI device is not installed on your system. Use the Drivers \
             option from the Control Panel to install a MIDI device."
        }
        339 => {
            "The current MIDI Mapper setup refers to a MIDI device that is not installed \
             on the system. Use the MIDI Mapper option from the Control Panel to edit the \
             setup."
        }
        340 => "An error occurred with the specified port.",
        341 => {
            "All multimedia timers are being used by other applications. Quit one of these \
             applications; then try again."
        }
        342 => "There is no current MIDI port.",
        343 => {
            "There are no MIDI devices installed on your system. Use the Drivers option \
             from the Control Panel to install a MIDI driver."
        }
        350 => "No identity.",
        _ => return None,
    })
}
