//! The prompt's colours, as clink's: the prompt in its colour (or the
//! colours its ANSI escape sequences ask for, PROMPT $E[1;33m), and the
//! line typed at it coloured by what its words are.

use std::collections::HashMap;

use super::settings::DosColor;
use crate::cpu::Cpu;
use crate::dosstr;
use crate::shell::video_call;

/// The attribute of text in `color` on a screen of attribute `screen`. A
/// monochrome adapter's screen has only bright and normal text: the light
/// colours are bright.
pub fn attr(cpu: &Cpu, color: DosColor, screen: u8) -> u8 {
    match color.0 {
        Some(c) if cpu.bus.read_8(0x0449) == 7 => (screen & 0xF0) | if c >= 8 { 0x0F } else { 0x07 },
        _ => color.on(screen),
    }
}

/// How a word of the line is coloured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The line's own colour: blanks, `|`, `<` and `>`.
    Plain,
    Command,
    Executable,
    Unrecognized,
    Argument,
    Flag,
}

/// Whether `b` ends a command's name, as `command::split_command` has it.
fn ends_name(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'/' | b'=' | b',' | b';' | b'|' | b'<' | b'>')
}

/// What each byte of `line` is: the command after the line's start and
/// each `|`, its switches (`/X`) and its other words. `is_builtin` and
/// `is_program` say what a command's name is.
pub fn classify(line: &[u8], is_builtin: impl Fn(&str) -> bool, mut is_program: impl FnMut(&str) -> bool) -> Vec<Kind> {
    let mut kinds = vec![Kind::Plain; line.len()];
    let mut i = 0;
    let mut command = true;
    while i < line.len() {
        let b = line[i];
        if matches!(b, b' ' | b'\t' | b'<' | b'>' | b'|') {
            if b == b'|' {
                command = true;
            }
            i += 1;
            continue;
        }
        if command {
            // ECHO OFF's '@'.
            if b == b'@' {
                i += 1;
                continue;
            }
            command = false;
            let mut end = i;
            while end < line.len() && !ends_name(line[end]) {
                end += 1;
            }
            let name = dosstr::from_bytes(&line[i..end]);
            // A built-in's name also ends at '.', '\' or ':' (CD.., CD\).
            let short = line[i..end].iter().position(|&b| matches!(b, b'.' | b'\\' | b':')).map_or(end, |n| i + n);
            let kind = if is_builtin(&name) {
                Kind::Command
            } else if short > i && is_builtin(&dosstr::from_bytes(&line[i..short])) {
                end = short;
                Kind::Command
            } else if (name.len() == 2 && name.ends_with(':') && name.as_bytes()[0].is_ascii_alphabetic()) || is_program(&name) {
                Kind::Executable
            } else {
                Kind::Unrecognized
            };
            kinds[i..end].fill(kind);
            i = end;
            continue;
        }
        // A word: up to a blank, a pipe, a redirection or the next switch.
        let kind = if b == b'/' { Kind::Flag } else { Kind::Argument };
        let mut end = i + 1;
        while end < line.len() && !matches!(line[end], b' ' | b'\t' | b'|' | b'<' | b'>') && !(kind == Kind::Flag && line[end] == b'/') {
            end += 1;
        }
        kinds[i..end].fill(kind);
        i = end;
    }
    kinds
}

/// Whether the program `name` (with or without its extension, a path or
/// not) is there to run: in the current directory or the path given, or
/// in a directory on the PATH.
fn finds_program(cpu: &Cpu, name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    let exts: &[&str] = if [".COM", ".EXE", ".BAT"].iter().any(|e| upper.ends_with(e)) { &[""] } else { &[".COM", ".EXE", ".BAT"] };
    let disk = &cpu.bus.disk;
    if exts.iter().any(|ext| disk.is_file(&format!("{}{}", name, ext))) {
        return true;
    }
    if name.contains(['\\', '/', ':']) {
        return false;
    }
    let path = cpu.get_env("PATH").unwrap_or_default();
    path.split(';').map(str::trim).filter(|d| !d.is_empty()).any(|dir| {
        let dir = dir.trim_end_matches('\\');
        exts.iter().any(|ext| disk.is_file(&format!("{}\\{}{}", dir, name, ext)))
    })
}

/// The attributes of the line's characters on a screen of attribute
/// `screen`, programs looked for once a line (`known`).
pub fn highlight(cpu: &Cpu, line: &[u8], screen: u8, known: &mut HashMap<String, bool>) -> Vec<u8> {
    let palette = cpu.shell_settings.palette;
    let builtin = |name: &str| crate::command::names().any(|n| n.eq_ignore_ascii_case(name));
    let program = |name: &str| *known.entry(name.to_ascii_uppercase()).or_insert_with(|| finds_program(cpu, name));
    classify(line, builtin, program)
        .into_iter()
        .map(|kind| match kind {
            Kind::Plain => screen,
            Kind::Command => attr(cpu, palette.command, screen),
            Kind::Executable => attr(cpu, palette.executable, screen),
            Kind::Unrecognized => attr(cpu, palette.unrecognized, screen),
            Kind::Argument => attr(cpu, palette.argument, screen),
            Kind::Flag => attr(cpu, palette.flag, screen),
        })
        .collect()
}

/// The DOS colours of ANSI's 0 to 7.
const ANSI: [u8; 8] = [0, 4, 2, 6, 1, 5, 3, 7];

/// The attribute after an SGR sequence's parameters `params` (ESC [ ...
/// m), from `attr`; `reset` is what 0 goes back to.
pub fn sgr(params: &[u8], attr: u8, reset: u8) -> u8 {
    let mut attr = attr;
    let text = String::from_utf8_lossy(params);
    for p in text.split(';') {
        let n: u32 = if p.is_empty() { 0 } else { p.parse().unwrap_or(u32::MAX) };
        attr = match n {
            0 => reset,
            1 => attr | 0x08,
            22 => attr & !0x08,
            5 => attr | 0x80,
            25 => attr & !0x80,
            30..=37 => (attr & 0xF8) | ANSI[n as usize - 30],
            39 => (attr & 0xF0) | (reset & 0x0F),
            40..=47 => (attr & 0x8F) | ANSI[n as usize - 40] << 4,
            49 => (attr & 0x0F) | (reset & 0xF0),
            90..=97 => (attr & 0xF0) | ANSI[n as usize - 90] | 0x08,
            100..=107 => (attr & 0x0F) | (ANSI[n as usize - 100] | 0x08) << 4,
            _ => attr,
        };
    }
    attr
}

/// The prompt's text in pieces of one attribute, from `base`: its ANSI
/// escape sequences change the attribute (when `colors`) and are left out.
pub fn prompt_runs(text: &[u8], base: u8, colors: bool) -> Vec<(Vec<u8>, u8)> {
    let mut runs: Vec<(Vec<u8>, u8)> = Vec::new();
    let mut attr = base;
    let mut i = 0;
    while i < text.len() {
        if text[i] == 0x1B && text.get(i + 1) == Some(&b'[') {
            let end = text[i + 2..].iter().position(|b| b.is_ascii_alphabetic()).map(|n| i + 2 + n);
            if let Some(end) = end {
                if text[end] == b'm' && colors {
                    attr = sgr(&text[i + 2..end], attr, base);
                }
                i = end + 1;
                continue;
            }
        }
        match runs.last_mut() {
            Some((bytes, a)) if *a == attr => bytes.push(text[i]),
            _ => runs.push((vec![text[i]], attr)),
        }
        i += 1;
    }
    runs
}

/// Print the prompt `text` from the cursor, as teletype output, in the
/// prompt's colour; the registers stay as they were.
pub fn print_prompt(cpu: &mut Cpu, text: &[u8]) {
    let colors = cpu.shell_settings.colors;
    let saved = (cpu.ax(), cpu.bx(), cpu.cx(), cpu.dx());
    let screen = super::render::attribute_at_cursor(cpu);
    let base = if colors { attr(cpu, cpu.shell_settings.palette.prompt, screen) } else { screen };
    let runs = prompt_runs(text, base, colors);
    let page = cpu.bus.read_8(0x0462) as u16;
    let graphics = crate::video::pixels::graphics_mode(&cpu.bus);
    for (bytes, attr) in runs {
        for b in bytes {
            if colors && b >= 0x20 && !graphics {
                // The character in its colour, then the teletype moves on
                // (and scrolls) keeping it.
                video_call(cpu, 0x0900 | b as u16, page << 8 | attr as u16, 1, 0);
            }
            let color = if graphics { (attr & 0x0F) as u16 } else { 0 };
            video_call(cpu, 0x0E00 | b as u16, page << 8 | color, 0, 0);
        }
    }
    cpu.set_ax(saved.0);
    cpu.set_reg16(iced_x86::Register::BX, saved.1);
    cpu.set_cx(saved.2);
    cpu.set_dx(saved.3);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(line: &str) -> String {
        let builtin = |n: &str| ["DIR", "CD", "ECHO", "SORT"].iter().any(|b| b.eq_ignore_ascii_case(n));
        let program = |n: &str| n.eq_ignore_ascii_case("game") || n.eq_ignore_ascii_case("game.exe");
        classify(line.as_bytes(), builtin, program)
            .into_iter()
            .map(|k| match k {
                Kind::Plain => ' ',
                Kind::Command => 'c',
                Kind::Executable => 'x',
                Kind::Unrecognized => 'u',
                Kind::Argument => 'a',
                Kind::Flag => 'f',
            })
            .collect()
    }

    #[test]
    fn colours_the_words_by_what_they_are() {
        assert_eq!(kinds("dir /w *.txt"), "ccc ff aaaaa");
        assert_eq!(kinds("DIR/W/P"), "cccffff");
        assert_eq!(kinds("cd..\\x"), "ccaaaa");
        assert_eq!(kinds("game.exe -x | sort > out"), "xxxxxxxx aa   cccc   aaa");
        assert_eq!(kinds("nope a"), "uuuu a");
        assert_eq!(kinds("@echo off"), " cccc aaa");
        assert_eq!(kinds("d: x"), "xx a");
    }

    #[test]
    fn sgr_sets_colours_as_ansi_has_them() {
        assert_eq!(sgr(b"1;33", 0x07, 0x07), 0x0E);
        assert_eq!(sgr(b"0", 0x1E, 0x07), 0x07);
        assert_eq!(sgr(b"", 0x1E, 0x07), 0x07);
        assert_eq!(sgr(b"31;44", 0x07, 0x07), 0x14);
        assert_eq!(sgr(b"92", 0x07, 0x07), 0x0A);
        assert_eq!(sgr(b"22;39", 0x0E, 0x07), 0x07);
    }

    #[test]
    fn the_prompt_s_escape_sequences_change_its_colour() {
        let runs = prompt_runs(b"\x1b[1;33mC:\\\x1b[0m>", 0x0A, true);
        assert_eq!(runs, [(b"C:\\".to_vec(), 0x0E), (b">".to_vec(), 0x0A)]);
        // Without colours they are left out all the same.
        assert_eq!(prompt_runs(b"\x1b[1;33mC:\\\x1b[0m>", 0x07, false), [(b"C:\\>".to_vec(), 0x07)]);
    }
}
