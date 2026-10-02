//! Tab and Shift+Tab at the prompt, as in DOSBox: the name being typed
//! completed to each name that fits in turn.

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
/// it with the name being typed, the last word after its last '\', '/'
/// or ':', completed to the first (Shift+Tab: last) file or directory
/// beginning with it. Tab again goes on to the next one, Shift+Tab back to
/// the one before. None when no name fits.
pub fn complete(cpu: &mut Cpu, typed: &[u8], forward: bool) -> Option<Vec<u8>> {
    let mut completion = match cpu.shell_completion.take() {
        Some(mut completion) if completion.line == typed => {
            let count = completion.names.len();
            completion.index = (if forward { completion.index + 1 } else { completion.index + count - 1 }) % count;
            completion
        }
        _ => {
            let word = typed.iter().rposition(|&b| b == b' ').map_or(0, |i| i + 1);
            let start = typed[word..].iter().rposition(|&b| matches!(b, b'\\' | b'/' | b':')).map_or(word, |i| word + i + 1);
            let names = completion_names(cpu, typed, word);
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

/// The names Tab goes through for the word at `word` in the line `typed`:
/// the files and directories beginning with it, the programs (.BAT, .COM,
/// .EXE) first, then the others, each in order of name. After CD only the
/// directories.
fn completion_names(cpu: &Cpu, typed: &[u8], word: usize) -> Vec<String> {
    let command = typed.split(|&b| b == b' ').find(|w| !w.is_empty()).unwrap_or_default();
    let cd = word > 0 && (command.eq_ignore_ascii_case(b"CD") || command.eq_ignore_ascii_case(b"CHDIR"));
    // "GA" looks for "GA*.*", "GAME.E" for "GAME.E*".
    let name = dosstr::from_bytes(&typed[word..]);
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
        .filter(|e| e.filename != "." && e.filename != ".." && (e.is_dir || !cd))
        .map(|e| {
            let upper = e.filename.to_ascii_uppercase();
            let program = !e.is_dir && [".BAT", ".COM", ".EXE"].iter().any(|ext| upper.ends_with(ext));
            (!program, e.filename)
        })
        .collect();
    names.sort_by_cached_key(|(other, name)| (*other, name.to_ascii_uppercase()));
    names.into_iter().map(|(_, name)| name).collect()
}
