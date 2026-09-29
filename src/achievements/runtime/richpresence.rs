//! Rich presence: the line the site shows of what a player is doing,
//! "Level 3, 2 lives". A script has lookups (numbers to names), formats,
//! and display strings, the first whose condition (`?…?`) holds, with
//! macros that show values: `@Level(0xH1234) with @Number(0xH1235) lives`.

use super::condset::Condset;
use super::format::Format;
use super::memref::Memrefs;
use super::operand::Operand;
use super::parse::{Cursor, Error, Parse};
use super::trigger::{Trigger, TriggerState};
use super::typed::Kind;
use super::value::{Value, Variable};

/// How a part of a display string shows.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Show {
    Text,
    Lookup(usize),
    Format(Format),
    UnknownMacro,
    AsciiChar,
    UnicodeChar,
}

#[derive(Clone, Debug)]
struct Part {
    show: Show,
    text: String,
    value: Operand,
}

#[derive(Clone, Debug)]
struct Display {
    trigger: Trigger,
    parts: Vec<Part>,
    has_required_hits: bool,
}

#[derive(Clone, Debug)]
struct Lookup {
    name: String,
    /// Disjoint ranges of values and their labels.
    items: Vec<(u32, u32, String)>,
    default_label: String,
    /// A lookup, or a format's.
    format: Option<Format>,
}

impl Lookup {
    fn label(&self, value: u32) -> &str {
        self.items
            .iter()
            .find(|(first, last, _)| (*first..=*last).contains(&value))
            .map_or(&self.default_label, |(_, _, l)| l)
    }

    fn insert(&mut self, first: u32, last: u32, label: &str) -> Result<(), Error> {
        if self.items.iter().any(|(f, l, _)| first <= *l && last >= *f) {
            return Err(Error::DuplicatedValue);
        }
        self.items.push((first, last, label.to_string()));
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct RichPresence {
    displays: Vec<Display>,
    lookups: Vec<Lookup>,
    variables: Vec<Variable>,
}

/// A line of the script, without its line ending and any `//` comment,
/// and where the next starts.
fn line_at(script: &str, start: usize, parse: &mut Parse) -> (usize, usize, usize) {
    let bytes = script.as_bytes();
    let next_line = bytes[start..]
        .iter()
        .position(|&b| b == b'\n')
        .map_or(bytes.len(), |p| start + p);
    let mut end = start;
    while end < next_line
        && !(bytes[end] == b'/'
            && bytes.get(end + 1) == Some(&b'/')
            && !(end > start && bytes[end - 1] == b'\\'))
    {
        end += 1;
    }
    if end == next_line {
        // Trailing blanks may matter without a comment; the line ending
        // doesn't.
        if end > start && bytes[end - 1] == b'\r' {
            end -= 1;
        }
    } else {
        while end > start && bytes[end - 1].is_ascii_whitespace() {
            end -= 1;
        }
    }
    parse.lines_read += 1;
    let next = if next_line < bytes.len() {
        next_line + 1
    } else {
        next_line
    };
    (start, end, next)
}

/// The macros every script has.
const BUILTIN: [(&str, Show); 19] = [
    ("Number", Show::Format(Format::Value)),
    ("Score", Show::Format(Format::Score)),
    ("Centiseconds", Show::Format(Format::Centisecs)),
    ("Seconds", Show::Format(Format::Seconds)),
    ("Minutes", Show::Format(Format::Minutes)),
    ("SecondsAsMinutes", Show::Format(Format::SecondsAsMinutes)),
    ("ASCIIChar", Show::AsciiChar),
    ("UnicodeChar", Show::UnicodeChar),
    ("Float1", Show::Format(Format::Float(1))),
    ("Float2", Show::Format(Format::Float(2))),
    ("Float3", Show::Format(Format::Float(3))),
    ("Float4", Show::Format(Format::Float(4))),
    ("Float5", Show::Format(Format::Float(5))),
    ("Float6", Show::Format(Format::Float(6))),
    ("Fixed1", Show::Format(Format::Fixed(1))),
    ("Fixed2", Show::Format(Format::Fixed(2))),
    ("Fixed3", Show::Format(Format::Fixed(3))),
    ("Unsigned", Show::Format(Format::UnsignedValue)),
    ("Unformatted", Show::Format(Format::Unformatted)),
];

impl RichPresence {
    pub fn parse(script: &str, memrefs: &mut Memrefs) -> Result<RichPresence, Error> {
        let mut variables = Vec::new();
        let (displays, lookups) = {
            let mut parse = Parse::new(memrefs);
            parse.variables = Some(&mut variables);
            parse_script(script, &mut parse)?
        };
        Ok(RichPresence {
            displays,
            lookups,
            variables,
        })
    }

    /// The frame's work: the helper values, and the display conditions
    /// with hit counts, which count every frame.
    pub fn update(&mut self, memrefs: &mut Memrefs) {
        for variable in &mut self.variables {
            if let Some(result) = variable.value.evaluate_typed(memrefs) {
                variable.value.value.update(result.bits);
                variable.value.value.kind = result.kind;
                let memref = &mut memrefs.items[variable.memref].value;
                memref.update(result.bits);
                memref.kind = result.kind;
            }
        }
        for display in &mut self.displays {
            if display.has_required_hits {
                display.trigger.test(memrefs);
            }
        }
    }

    /// The line to show now.
    pub fn display(&mut self, memrefs: &Memrefs) -> String {
        let count = self.displays.len();
        for i in 0..count {
            if i + 1 == count {
                return self.show(i, memrefs);
            }
            let display = &mut self.displays[i];
            if !display.has_required_hits {
                display.trigger.test(memrefs);
            }
            if display.trigger.state == TriggerState::Triggered {
                return self.show(i, memrefs);
            }
        }
        String::new()
    }

    pub fn reset(&mut self) {
        for display in &mut self.displays {
            display.trigger.reset();
        }
        for variable in &mut self.variables {
            variable.value.reset();
        }
    }

    fn show(&self, display: usize, memrefs: &Memrefs) -> String {
        let parts = &self.displays[display].parts;
        let mut out = String::new();
        let mut i = 0;
        while i < parts.len() {
            let part = &parts[i];
            match part.show {
                Show::Text => out.push_str(&part.text),
                Show::Lookup(l) => {
                    let value = part.value.evaluate(memrefs).converted(Kind::Unsigned).u32();
                    out.push_str(self.lookups[l].label(value));
                }
                Show::AsciiChar | Show::UnicodeChar => {
                    // A run of them is a string, up to a zero.
                    let kind = part.show;
                    let mut text = String::new();
                    loop {
                        let c = parts[i].value.evaluate(memrefs).u32();
                        if c == 0 {
                            while i + 1 < parts.len() && parts[i + 1].show == kind {
                                i += 1;
                            }
                            break;
                        }
                        text.push(if kind == Show::AsciiChar {
                            if (32..127).contains(&c) {
                                c as u8 as char
                            } else {
                                '?'
                            }
                        } else if !(32..=0xFFFF).contains(&c) {
                            '\u{FFFD}'
                        } else {
                            char::from_u32(c).unwrap_or('\u{FFFD}')
                        });
                        if text.len() >= 255 || i + 1 >= parts.len() || parts[i + 1].show != kind {
                            break;
                        }
                        i += 1;
                    }
                    out.push_str(&text);
                }
                Show::UnknownMacro => {
                    out.push_str("[Unknown macro]");
                    out.push_str(&part.text);
                }
                Show::Format(format) => out.push_str(&format.show(part.value.evaluate(memrefs))),
            }
            i += 1;
        }
        out
    }
}

fn parse_script(script: &str, parse: &mut Parse) -> Result<(Vec<Display>, Vec<Lookup>), Error> {
    if script.is_empty() {
        parse.lines_read = 1;
        return Err(Error::MissingDisplayString);
    }
    let mut lookups: Vec<Lookup> = Vec::new();
    let mut display_start = None;
    let mut pos = 0;
    // First, the lookups and formats.
    while pos < script.len() {
        let (start, end, mut next) = line_at(script, pos, parse);
        let line = &script[start..end];
        if let Some(name) = line.strip_prefix("Lookup:") {
            let mut lookup = Lookup {
                name: name.to_string(),
                items: Vec::new(),
                default_label: String::new(),
                format: None,
            };
            next = parse_lookup(script, next, &mut lookup, parse)?;
            lookups.push(lookup);
        } else if let Some(name) = line.strip_prefix("Format:") {
            if name == "Unformatted" {
                // Old scripts defined it; it's built in now. Its
                // FormatType= line goes too.
                if next < script.len() {
                    next = line_at(script, next, parse).2;
                }
                pos = next;
                continue;
            }
            let (s2, e2, n2) = if next < script.len() {
                line_at(script, next, parse)
            } else {
                (next, next, next)
            };
            let type_line = &script[s2..e2];
            let format = match type_line.strip_prefix("FormatType=") {
                Some(name) => Format::parse(&name[..name.len().min(63)]),
                None => Format::Value,
            };
            lookups.push(Lookup {
                name: name.to_string(),
                items: Vec::new(),
                default_label: String::new(),
                format: Some(format),
            });
            next = n2;
        } else if line.starts_with("Display:") {
            display_start = Some((next, parse.lines_read));
            // Past its conditional lines and full-line comments.
            loop {
                if next >= script.len() {
                    break;
                }
                let (s2, _, n2) = line_at(script, next, parse);
                next = n2;
                let bytes = script.as_bytes();
                if !(bytes[s2] == b'?' || (bytes[s2] == b'/' && bytes.get(s2 + 1) == Some(&b'/'))) {
                    break;
                }
            }
        }
        pos = next;
    }

    let mut displays = Vec::new();
    let Some((start, display_line)) = display_start else {
        return Err(Error::MissingDisplayString);
    };
    let lines_read = parse.lines_read;
    parse.lines_read = display_line;
    let mut pos = start;
    loop {
        let (s, e, next) = if pos < script.len() {
            line_at(script, pos, parse)
        } else {
            (pos, pos, pos)
        };
        let line = &script[s..e];
        let comment = script[s..].starts_with("//");
        if let Some(rest) = line.strip_prefix('?') {
            // ?condition?text
            if let Some(q) = rest.find('?') {
                let mut display = parse_display(&rest[q + 1..], parse, &lookups)?;
                // Parsed in place: it must end at the '?'.
                let mut cursor = Cursor::new(rest);
                display.trigger = Trigger::parse(&mut cursor, parse)?;
                if cursor.pos != q {
                    return Err(Error::InvalidOperator);
                }
                display.has_required_hits = parse.has_required_hits;
                displays.push(display);
            }
        } else if !comment {
            // The line without a condition, the last.
            displays.push(parse_display(line, parse, &lookups)?);
            parse.lines_read = lines_read;
            break;
        }
        if next >= script.len() && pos >= script.len() {
            return Err(Error::MissingDisplayString);
        }
        pos = next;
    }
    Ok((displays, lookups))
}

/// A lookup's lines, `1=Level 1`, `2-4,7=Castle`, `0x10=Boss`, `*=?`, up
/// to an empty line.
fn parse_lookup(
    script: &str,
    mut pos: usize,
    lookup: &mut Lookup,
    parse: &mut Parse,
) -> Result<usize, Error> {
    while pos < script.len() {
        let (start, end, next) = line_at(script, pos, parse);
        pos = next;
        let line = &script[start..end];
        if line.len() < 2 {
            if script[start..].starts_with("//") {
                continue;
            }
            // An empty line ends it.
            break;
        }
        if let Some(label) = line.strip_prefix("*=") {
            lookup.default_label = label.to_string();
            continue;
        }
        let Some(eq) = line.find('=') else {
            return Err(Error::MissingValue);
        };
        let label = &line[eq + 1..];
        let mut s = Cursor::new(line);
        loop {
            let number = |s: &mut Cursor| -> u32 {
                let base = if s.at(0) == b'0' && s.at(1) == b'x' {
                    s.skip(2);
                    16
                } else {
                    10
                };
                s.strtoul(base).unwrap_or(0) as u32
            };
            let first = number(&mut s);
            let last = if s.at(0) == b'-' {
                s.skip(1);
                number(&mut s)
            } else {
                first
            };
            while s.at(0) == b' ' {
                s.skip(1);
            }
            match s.at(0) {
                b'=' => {
                    lookup.insert(first, last, label)?;
                    break;
                }
                b',' => {
                    lookup.insert(first, last, label)?;
                    s.skip(1);
                    if s.pos >= end - start {
                        break;
                    }
                }
                _ => return Err(Error::InvalidConstOperand),
            }
        }
    }
    Ok(pos)
}

/// A display string's parts: text, and `@Macro(value)`.
fn parse_display(line: &str, parse: &mut Parse, lookups: &[Lookup]) -> Result<Display, Error> {
    if line.is_empty() {
        return Err(Error::MissingDisplayString);
    }
    let bytes = line.as_bytes();
    let mut parts = Vec::new();
    let mut pos = 0;
    while pos < bytes.len() {
        let mut at = pos;
        while at < bytes.len() && !(bytes[at] == b'@' && (at == pos || bytes[at - 1] != b'\\')) {
            at += 1;
        }
        if at > pos {
            // Text, without the backslashes that escape.
            let raw = &line[pos..at];
            let mut text = String::new();
            let mut chars = raw.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    if let Some(next) = chars.next() {
                        text.push(next);
                    }
                } else {
                    text.push(c);
                }
            }
            parts.push(Part {
                show: Show::Text,
                text,
                value: Operand::NONE,
            });
        }
        if at >= bytes.len() {
            break;
        }
        // @Name(parameter)
        let name_start = at + 1;
        let Some(open) = line[name_start..].find('(').map(|p| name_start + p) else {
            return Err(Error::MissingValue);
        };
        let name = &line[name_start..open];
        let show = match lookups.iter().position(|l| l.name == name) {
            Some(l) => match lookups[l].format {
                Some(format) => Show::Format(format),
                None => Show::Lookup(l),
            },
            None => BUILTIN
                .iter()
                .find(|(n, _)| *n == name)
                .map_or(Show::UnknownMacro, |(_, show)| *show),
        };
        let param_start = open + 1;
        match line[param_start..].find(')').map(|p| param_start + p) {
            None => {
                // Not closed: the rest shows as it is.
                parts.push(Part {
                    show: Show::Text,
                    text: line[at..].to_string(),
                    value: Operand::NONE,
                });
                pos = bytes.len();
            }
            Some(close) if show != Show::UnknownMacro => {
                let value = helper_value(&line[param_start..], close - param_start, parse)?;
                parts.push(Part {
                    show,
                    text: name.to_string(),
                    value,
                });
                pos = close + 1;
            }
            Some(close) => {
                parts.push(Part {
                    show,
                    text: line[at + 1..close + 1].to_string(),
                    value: Operand::NONE,
                });
                pos = close + 1;
            }
        }
    }
    Ok(Display {
        trigger: Trigger {
            requirement: None,
            alternatives: Vec::new(),
            measured_value: 0,
            measured_target: 0,
            state: TriggerState::Waiting,
            has_hits: false,
            measured_as_percent: false,
        },
        parts,
        has_required_hits: false,
    })
}

/// A macro's value, parsed in place from `text` (the first `len` bytes
/// its parameter): a plain memory read (with its AddSources and
/// AddAddresses) if it is one, else a helper value worked out every frame.
fn helper_value(text: &str, len: usize, parse: &mut Parse) -> Result<Operand, Error> {
    let name = &text[..len];
    {
        let before = parse.memrefs.items.len();
        let mut test = Parse::new(parse.memrefs);
        let mut cursor = Cursor::new(text);
        let value = Value::parse(&mut cursor, &mut test)?;
        // rcheevos can't copy a derived value of an unresolved {recall}.
        let unresolved = |o: &Operand| {
            o.ty == super::operand::OperandType::Recall
                && o.access.is_memref()
                && o.memref.is_none()
        };
        let made = &parse.memrefs.items[before..];
        if made.iter().any(|m| matches!(&m.source, super::memref::Source::Derived(d) if unresolved(&d.parent) || unresolved(&d.modifier))) {
            return Err(Error::InvalidMemoryOperand);
        }
        if let [set] = value.conditions.as_slice()
            && simple(set)
            && let Some(measured) = set
                .in_order()
                .find(|c| c.ty == super::condition::CondType::Measured && c.required_hits == 0)
        {
            return Ok(measured.operand1);
        }
    }
    let Some(variables) = parse.variables.as_deref_mut() else {
        return Err(Error::InvalidValue);
    };
    let memref = match variables.iter().find(|v| v.name == name) {
        Some(existing) => existing.memref,
        None => {
            let measured_target = parse.measured_target;
            let mut cursor = Cursor::new(text);
            let was_value = parse.is_value;
            let value = Value::parse(&mut cursor, parse);
            parse.is_value = was_value;
            parse.measured_target = measured_target;
            let value = value?;
            let memref = parse.memrefs.variable();
            let variables = parse.variables.as_deref_mut().expect("variables");
            variables.push(Variable {
                name: name.to_string(),
                value,
                memref,
            });
            memref
        }
    };
    Ok(Operand {
        ty: super::operand::OperandType::Address,
        access: super::operand::OperandType::Address,
        size: super::memref::Size::Bits32,
        memref: Some(memref),
        ..Operand::NONE
    })
}

/// One Measured and what feeds it, nothing else.
fn simple(set: &Condset) -> bool {
    set.num_measured == 1
        && set.num_pause == 0
        && set.num_reset == 0
        && set.num_other == 0
        && set.num_hittarget == 0
}
