//! Tab and Shift+Tab at the prompt, as in DOSBox: the name being typed
//! completed to each name that fits in turn. The command a line begins
//! with completes to the built-in commands, the programs on the PATH and
//! the files and directories; the words after it to the files and
//! directories (only the directories after CD, MD and RD), the variables
//! after SET, and a variable after a '%'.

use crate::cpu::Cpu;
use crate::dosstr;

/// Where Tab left the line at the prompt.
#[derive(Clone, Debug)]
pub struct Completion {
    /// The line before the cursor as Tab left it: Tab again on it goes on
    /// to the next name, on any other line it starts over.
    line: Vec<u8>,
    /// Where the name being completed begins in it.
    start: usize,
    /// The names that fit, and the one in the line.
    names: Vec<String>,
    index: usize,
}

/// Tab (`forward`) or Shift+Tab for the line before the cursor, `typed`:
/// it with the name being typed completed to the first (Shift+Tab: last)
/// name that fits (`candidates`). Tab again goes on to the next one,
/// Shift+Tab back to the one before. None when no name fits.
pub fn complete(cpu: &mut Cpu, typed: &[u8], forward: bool) -> Option<Vec<u8>> {
    let mut completion = match cpu.shell_completion.take() {
        Some(mut completion) if completion.line == typed => {
            let count = completion.names.len();
            completion.index = (if forward { completion.index + 1 } else { completion.index + count - 1 }) % count;
            completion
        }
        _ => {
            let (start, names) = candidates(cpu, typed);
            let index = if forward { 0 } else { names.len().checked_sub(1)? };
            Completion { line: Vec::new(), start, names, index }
        }
    };
    let mut line = typed[..completion.start].to_vec();
    line.extend(dosstr::to_bytes(completion.names.get(completion.index)?));
    completion.line = line.clone();
    cpu.shell_completion = Some(completion);
    Some(line)
}

/// Where the name being typed at the end of `typed` begins, and the names
/// that complete it, in the order Tab goes through them.
pub fn candidates(cpu: &Cpu, typed: &[u8]) -> (usize, Vec<String>) {
    // The command after the last pipe.
    let segment = typed.iter().rposition(|&b| b == b'|').map_or(0, |i| i + 1);
    let word = typed.iter().rposition(|&b| matches!(b, b' ' | b'|' | b'<' | b'>')).map_or(0, |i| i + 1).max(segment);
    // A variable: after an odd '%' in the word.
    let percents: Vec<usize> = typed[word..].iter().enumerate().filter(|&(_, &b)| b == b'%').map(|(i, _)| word + i).collect();
    if percents.len() % 2 == 1 {
        let start = percents[percents.len() - 1] + 1;
        let prefix = dosstr::from_bytes(&typed[start..]);
        let names = variables(cpu, &prefix).into_iter().map(|n| format!("{}%", n)).collect();
        return (start, names);
    }
    let start = typed[word..].iter().rposition(|&b| matches!(b, b'\\' | b'/' | b':')).map_or(word, |i| word + i + 1);
    let mut words = typed[segment..word].split(|&b| b == b' ').filter(|w| !w.is_empty());
    let command = words.next();
    let name = dosstr::from_bytes(&typed[word..]);
    let is = |names: &[&str]| command.is_some_and(|c| names.iter().any(|n| c.eq_ignore_ascii_case(n.as_bytes())));
    let names = match command {
        // The command itself, unless it has a path.
        // The built-ins, the programs here and on the PATH, then the
        // directories and other files here.
        None if start == word => {
            let mut names: Vec<String> = crate::command::names().filter(|n| starts_with(n, &name)).map(str::to_string).collect();
            let (programs, others): (Vec<String>, Vec<String>) = files(cpu, &name, false).into_iter().partition(|n| is_program(n));
            // A program here again on the PATH (C:\ is on it) only once.
            let stems: Vec<String> = programs.iter().map(|p| stem(p).to_ascii_uppercase()).collect();
            names.extend(programs);
            names.extend(path_programs(cpu, &name).into_iter().filter(|p| !stems.contains(&p.to_ascii_uppercase())));
            names.extend(others);
            dedup(names)
        }
        None => files(cpu, &name, false),
        Some(_) if is(&["CD", "CHDIR", "MD", "MKDIR", "RD", "RMDIR"]) => files(cpu, &name, true),
        Some(_) if is(&["SET"]) && words.next().is_none() => variables(cpu, &name),
        Some(_) => files(cpu, &name, false),
    };
    (start, names)
}

/// Whether `name` begins with `prefix`, in any case.
fn starts_with(name: &str, prefix: &str) -> bool {
    name.len() >= prefix.len() && name.is_char_boundary(prefix.len()) && name[..prefix.len()].eq_ignore_ascii_case(prefix)
}

/// `names` without the later of two the same but for case.
fn dedup(names: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    names.into_iter().filter(|n| seen.insert(n.to_ascii_uppercase())).collect()
}

/// The environment's variables beginning with `prefix`, by name.
fn variables(cpu: &Cpu, prefix: &str) -> Vec<String> {
    let mut names: Vec<String> = cpu.environment.iter().map(|(n, _)| n.clone()).filter(|n| starts_with(n, prefix)).collect();
    names.sort_by_key(|n| n.to_ascii_uppercase());
    names
}

/// `name` without its extension.
fn stem(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(stem, _)| stem)
}

/// Whether `name` is a program's: .BAT, .COM or .EXE.
fn is_program(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    [".BAT", ".COM", ".EXE"].iter().any(|ext| upper.ends_with(ext))
}

/// The files and directories beginning with `name` (which may have a
/// path), the programs (.BAT, .COM, .EXE) first, then the others, each in
/// order of name; only the directories with `dirs`.
fn files(cpu: &Cpu, name: &str, dirs: bool) -> Vec<String> {
    // "GA" looks for "GA*.*", "GAME.E" for "GAME.E*".
    let mask = if name.rsplit(['\\', '/', ':']).next().is_some_and(|n| n.contains('.')) {
        format!("{}*", name)
    } else {
        format!("{}*.*", name)
    };
    let Ok(entries) = cpu.bus.disk.list_directory(&mask, 0x16) else {
        return Vec::new();
    };
    let mut names: Vec<(bool, String)> = entries
        .into_iter()
        .filter(|e| e.filename != "." && e.filename != ".." && (e.is_dir || !dirs))
        .map(|e| (e.is_dir || !is_program(&e.filename), e.filename))
        .collect();
    names.sort_by_cached_key(|(other, name)| (*other, name.to_ascii_uppercase()));
    names.into_iter().map(|(_, name)| name).collect()
}

/// The programs in the PATH's directories beginning with `name`, by
/// name, without their extensions.
fn path_programs(cpu: &Cpu, name: &str) -> Vec<String> {
    let Some(path) = cpu.get_env("PATH") else { return Vec::new() };
    let mut names = Vec::new();
    for dir in path.split(';').map(str::trim).filter(|d| !d.is_empty()) {
        let dir = dir.trim_end_matches('\\');
        let Ok(entries) = cpu.bus.disk.list_directory(&format!("{}\\{}*.*", dir, name), 0x06) else { continue };
        names.extend(
            entries
                .into_iter()
                .filter(|e| !e.is_dir && is_program(&e.filename))
                .map(|e| stem(&e.filename).to_string()),
        );
    }
    names.sort_by_key(|n| n.to_ascii_uppercase());
    names
}
