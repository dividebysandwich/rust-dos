//! MORE, FIND and SORT, the filters of DOS: they read a pipe or `<file`
//! (or the files they are given), and write what they make of it.

use crate::command::ShellCommand;
use crate::cpu::Cpu;
use crate::shell::{ShellWait, enter_wait};
use crate::video::{print_cp437, print_string};

/// What a filter reads: the file its arguments name, else its redirected
/// input. None, with the message printed, if it has neither or the file
/// isn't there.
fn input(cpu: &mut Cpu, file: Option<&str>) -> Option<Vec<u8>> {
    match file {
        Some(name) => {
            let data = read_file(cpu, name);
            if data.is_none() {
                print_string(cpu, &format!("File not found - {}\r\n", name.to_ascii_uppercase()));
            }
            data
        }
        None => {
            let data = cpu.stdin_redirect.clone();
            if data.is_none() {
                print_string(cpu, "Required parameter missing\r\n");
            }
            data
        }
    }
}

fn read_file(cpu: &Cpu, name: &str) -> Option<Vec<u8>> {
    if crate::disk::char_device(name).is_some() {
        return Some(Vec::new());
    }
    cpu.bus.disk.file_data(name)?.read().ok().map(|b| b.to_vec())
}

/// The lines of a text, up to its end of file mark (^Z), without their
/// CR LF (or LF).
fn text_lines(text: &[u8]) -> Vec<Vec<u8>> {
    let text = text.split(|&b| b == 0x1A).next().unwrap_or_default();
    let mut lines: Vec<Vec<u8>> = text
        .split(|&b| b == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line).to_vec())
        .collect();
    if lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines
}

fn print_line(cpu: &mut Cpu, line: &[u8]) {
    print_cp437(cpu, line, 0x07);
    print_string(cpu, "\r\n");
}

/// The first argument that isn't a switch.
fn file_argument(args: &str) -> Option<&str> {
    args.split_whitespace().find(|a| !a.starts_with('/'))
}

/// MORE [file]: its input a screenful at a time, with "-- More --" and a
/// key between them.
pub struct MoreCommand;
impl ShellCommand for MoreCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        let Some(text) = input(cpu, file_argument(args)) else { return };
        let lines = text_lines(&text).into_iter().map(|line| expand_tabs(&line)).collect();
        if let Some(rest) = show_page(cpu, lines) {
            enter_wait(cpu, ShellWait::More(rest));
        }
    }
}

fn expand_tabs(line: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(line.len());
    for &b in line {
        if b == b'\t' {
            out.extend(std::iter::repeat_n(b' ', 8 - out.len() % 8));
        } else {
            out.push(b);
        }
    }
    out
}

const MORE_PROMPT: &str = "-- More --";

/// Print as many of `lines` as fit on the screen above a last row for
/// "-- More --", which is printed after them; the lines left, if there are
/// any. Output redirected to a file takes them all.
fn show_page(cpu: &mut Cpu, lines: Vec<Vec<u8>>) -> Option<Vec<Vec<u8>>> {
    if cpu.stdout_capture.is_some() {
        lines.iter().for_each(|line| print_line(cpu, line));
        return None;
    }
    let rows = cpu.bus.read_8(0x0484) as usize + 1;
    let rows = if rows < 2 { 25 } else { rows };
    let cols = (cpu.bus.read_16(0x044A) as usize).max(1);
    let mut used = 0;
    let mut shown = 0;
    for line in &lines {
        // A line as long as the row wraps, and CR LF goes on a row after it.
        let height = line.len() / cols + 1;
        if used > 0 && used + height > rows - 1 {
            break;
        }
        print_line(cpu, line);
        used += height;
        shown += 1;
    }
    if shown == lines.len() {
        return None;
    }
    print_string(cpu, MORE_PROMPT);
    Some(lines[shown..].to_vec())
}

/// A key while MORE waits: the "-- More --" is taken off, and the next
/// screenful shown; the lines left after it.
pub fn more_key(cpu: &mut Cpu, lines: Vec<Vec<u8>>) -> Option<Vec<Vec<u8>>> {
    print_string(cpu, &format!("\r{}\r", " ".repeat(MORE_PROMPT.len())));
    show_page(cpu, lines)
}

/// FIND [/V] [/C] [/N] [/I] "string" [file ...]: the lines with the string
/// in them (/V without it), numbered with /N, only counted with /C, in any
/// case with /I. ERRORLEVEL 0 if a line was found, 1 if none, 2 on an
/// error.
pub struct FindCommand;
impl ShellCommand for FindCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        let Some(find) = parse_find(args) else {
            print_string(cpu, "FIND: Parameter format not correct\r\n");
            cpu.errorlevel = 2;
            return;
        };
        let mut found = false;
        let mut error = false;
        if find.files.is_empty() {
            match input(cpu, None) {
                Some(text) => found = find.run(cpu, &text, None),
                None => error = true,
            }
        }
        for name in &find.files {
            match read_file(cpu, name) {
                Some(text) => found |= find.run(cpu, &text, Some(&name.to_ascii_uppercase())),
                None => {
                    print_string(cpu, &format!("File not found - {}\r\n", name.to_ascii_uppercase()));
                    error = true;
                }
            }
        }
        cpu.errorlevel = if error && !found { 2 } else if found { 0 } else { 1 };
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Find {
    text: Vec<u8>,
    invert: bool,
    count: bool,
    numbers: bool,
    ignore_case: bool,
    files: Vec<String>,
}

/// FIND's arguments: its switches, the string in quotes (in which "" is a
/// quote) and the files. None without the string.
fn parse_find(args: &str) -> Option<Find> {
    let mut find = Find::default();
    let mut text = None;
    let mut chars = args.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => {}
            '/' => match chars.next().map(|c| c.to_ascii_uppercase()) {
                Some('V') => find.invert = true,
                Some('C') => find.count = true,
                Some('N') => find.numbers = true,
                Some('I') => find.ignore_case = true,
                _ => return None,
            },
            '"' if text.is_none() => {
                let mut s = String::new();
                loop {
                    match chars.next() {
                        Some('"') if chars.next_if_eq(&'"').is_some() => s.push('"'),
                        Some('"') | None => break,
                        Some(c) => s.push(c),
                    }
                }
                text = Some(s);
            }
            _ => {
                let mut name = c.to_string();
                while let Some(c) = chars.next_if(|c| !matches!(c, ' ' | '\t' | '/')) {
                    name.push(c);
                }
                find.files.push(name);
            }
        }
    }
    find.text = crate::dosstr::to_bytes(&text?);
    Some(find)
}

impl Find {
    fn matches(&self, line: &[u8]) -> bool {
        let n = self.text.len();
        let found = n == 0
            || line.windows(n).any(|w| if self.ignore_case { w.eq_ignore_ascii_case(&self.text) } else { w == self.text });
        found != self.invert
    }

    /// Print what it finds in `text`, under a header with the file's name
    /// if it has one. Whether a line was found.
    fn run(&self, cpu: &mut Cpu, text: &[u8], name: Option<&str>) -> bool {
        let lines = text_lines(text);
        let matching: Vec<(usize, &Vec<u8>)> = lines.iter().enumerate().filter(|(_, line)| self.matches(line)).collect();
        if self.count {
            match name {
                Some(name) => print_string(cpu, &format!("\r\n---------- {}: {}\r\n", name, matching.len())),
                None => print_string(cpu, &format!("{}\r\n", matching.len())),
            }
            return !matching.is_empty();
        }
        if let Some(name) = name {
            print_string(cpu, &format!("\r\n---------- {}\r\n", name));
        }
        for (i, line) in &matching {
            if self.numbers {
                print_string(cpu, &format!("[{}]", i + 1));
            }
            print_line(cpu, line);
        }
        !matching.is_empty()
    }
}

/// SORT [/R] [/+n] [file]: its input's lines in order, from their n-th
/// column (/+n), in any case, and backwards with /R.
pub struct SortCommand;
impl ShellCommand for SortCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        let mut reverse = false;
        let mut column = 1;
        for switch in args.split_whitespace().filter(|a| a.starts_with('/')) {
            let upper = switch.to_ascii_uppercase();
            match upper.strip_prefix("/+").map(str::parse::<usize>) {
                Some(Ok(n)) if n > 0 => column = n,
                _ if upper == "/R" => reverse = true,
                _ => return print_string(cpu, &format!("Invalid switch - {}\r\n", switch)),
            }
        }
        let Some(text) = input(cpu, file_argument(args)) else { return };
        for line in sorted(text_lines(&text), column, reverse) {
            print_line(cpu, &line);
        }
    }
}

fn sorted(mut lines: Vec<Vec<u8>>, column: usize, reverse: bool) -> Vec<Vec<u8>> {
    lines.sort_by_cached_key(|line| line.get(column - 1..).unwrap_or_default().to_ascii_uppercase());
    if reverse {
        lines.reverse();
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_takes_switches_a_quoted_string_and_files() {
        let find = parse_find(r#"/I /n "say ""hi""" A.TXT B.TXT"#).unwrap();
        assert_eq!(find.text, b"say \"hi\"");
        assert!(find.ignore_case && find.numbers && !find.count && !find.invert);
        assert_eq!(find.files, ["A.TXT", "B.TXT"]);
        assert_eq!(parse_find("/V/C \"x\"").map(|f| (f.invert, f.count)), Some((true, true)));
        assert!(parse_find("A.TXT").is_none());
        assert!(parse_find("/X \"a\"").is_none());
    }

    #[test]
    fn find_matches_in_any_case_or_the_other_lines() {
        let mut find = parse_find("\"Key\"").unwrap();
        assert!(find.matches(b"a Key here") && !find.matches(b"a KEY here"));
        find.ignore_case = true;
        assert!(find.matches(b"a KEY here"));
        find.invert = true;
        assert!(!find.matches(b"a KEY here") && find.matches(b"nothing"));
    }

    #[test]
    fn lines_end_at_the_end_of_file_mark() {
        assert_eq!(text_lines(b"one\r\ntwo\nthree\r\n\x1Ajunk"), [b"one".to_vec(), b"two".to_vec(), b"three".to_vec()]);
        assert_eq!(text_lines(b""), Vec::<Vec<u8>>::new());
        assert_eq!(expand_tabs(b"a\tb"), b"a       b");
    }

    #[test]
    fn sort_goes_by_column_in_any_case() {
        let lines = text_lines(b"b2\nA3\nc1\n");
        assert_eq!(sorted(lines.clone(), 1, false), [b"A3".to_vec(), b"b2".to_vec(), b"c1".to_vec()]);
        assert_eq!(sorted(lines.clone(), 1, true), [b"c1".to_vec(), b"b2".to_vec(), b"A3".to_vec()]);
        assert_eq!(sorted(lines, 2, false), [b"c1".to_vec(), b"b2".to_vec(), b"A3".to_vec()]);
    }
}
