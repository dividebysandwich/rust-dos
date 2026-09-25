//! Batch files as COMMAND.COM runs them: the lines of a .BAT file, of the
//! configuration's `[autoexec]` or of a game's commands, one at a time at
//! the prompt, with the file's parameters (`%0` to `%9`), the environment's
//! variables (`%NAME%`) and labels to GOTO.
//!
//! The batch files running are a stack of frames, the top one running. A
//! batch file started from a line of another takes its place, as DOS
//! chains to it and never comes back; CALL (and a batch file typed at the
//! prompt) puts it on top, and the one below goes on after it. Lines
//! handed over as a list ([autoexec], AUTOEXEC.BAT after it, a game's
//! commands) go to the bottom, to run once everything before them has.
//!
//! Lines are kept as the file's bytes, code page 437, and come out as
//! strings of one char per byte (`dosstr`).

use crate::dosstr;

/// Where a frame's lines come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameKind {
    /// A .BAT file.
    File,
    /// Lines handed over as a list: the configuration's [autoexec], a
    /// game's commands, the web page's.
    Lines,
    /// The commands a FOR line runs, one per member of its set, with its
    /// variable and the file's parameters already in them.
    For,
}

#[derive(Clone, Debug)]
struct Frame {
    kind: FrameKind,
    lines: Vec<Vec<u8>>,
    /// The next line to run.
    pc: usize,
    /// `%0`, the name the file was started by, and its parameters.
    params: Vec<String>,
    /// How far SHIFT has moved the parameters along.
    shift: usize,
}

impl Frame {
    fn new(kind: FrameKind, lines: Vec<Vec<u8>>, params: Vec<String>) -> Self {
        Self { kind, lines, pc: 0, params, shift: 0 }
    }
}

/// A line to run, with its parameters and variables in place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchLine {
    pub text: String,
    /// Whether it is shown at the prompt before it runs: ECHO is on and
    /// it doesn't begin with '@'.
    pub echo: bool,
}

/// The batch files running, and COMMAND.COM's ECHO.
#[derive(Clone, Debug)]
pub struct Batch {
    /// The top one runs.
    frames: Vec<Frame>,
    /// ECHO ON or OFF: whether batch lines are shown before they run.
    pub echo: bool,
    /// ECHO as it was before the batch files began, which comes back
    /// when they have all ended.
    echo_before: bool,
    /// Set while a batch line runs: a batch file it starts takes the
    /// place of the one running.
    pub dispatching: bool,
}

impl Default for Batch {
    fn default() -> Self {
        Self { frames: Vec::new(), echo: true, echo_before: true, dispatching: false }
    }
}

impl Batch {
    /// Whether batch lines are waiting to run.
    pub fn is_active(&self) -> bool {
        self.frames.iter().any(|f| f.pc < f.lines.len())
    }

    /// Forget the batch files once their last line has run, and turn ECHO
    /// back to what it was before them. (Until then a GOTO on their last
    /// line may still go back into them.)
    pub fn settle(&mut self) {
        if !self.is_active() {
            self.clear();
        }
    }

    /// End every batch file.
    pub fn clear(&mut self) {
        if !self.frames.is_empty() {
            self.frames.clear();
            self.echo = self.echo_before;
        }
    }

    /// Run these lines once everything queued before them has run.
    pub fn append_lines<I, S>(&mut self, lines: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let lines = lines.into_iter().map(|l| dosstr::to_bytes(l.as_ref())).collect();
        self.insert_bottom(Frame::new(FrameKind::Lines, lines, vec![String::new()]));
    }

    /// Run the batch file `name`, holding `bytes`, with the parameters
    /// `args` once everything queued before it has run.
    pub fn append_file(&mut self, name: &str, bytes: &[u8], args: &str) {
        self.insert_bottom(Frame::new(FrameKind::File, file_lines(bytes), params(name, args)));
    }

    /// Start the batch file `name`, holding `bytes`, with the parameters
    /// `args`: in place of the one running when a batch line starts it
    /// (DOS chains to it), else, and always with `call`, on top of the
    /// ones running.
    pub fn start_file(&mut self, name: &str, bytes: &[u8], args: &str, call: bool) {
        if self.dispatching && !call {
            self.drop_for_frames();
            self.frames.pop();
        }
        self.push(Frame::new(FrameKind::File, file_lines(bytes), params(name, args)));
    }

    /// Run these commands, a FOR line's, next.
    pub fn push_for(&mut self, lines: Vec<String>) {
        let lines = lines.iter().map(|l| dosstr::to_bytes(l)).collect();
        self.push(Frame::new(FrameKind::For, lines, Vec::new()));
    }

    /// The next line to run, if there is one: blank lines and labels are
    /// skipped, and the parameters and variables of `env` put in.
    pub fn next_line(&mut self, env: &[(String, String)]) -> Option<BatchLine> {
        loop {
            let frame = self.frames.last_mut()?;
            let Some(raw) = frame.lines.get(frame.pc) else {
                self.frames.pop();
                if self.frames.is_empty() {
                    self.echo = self.echo_before;
                }
                continue;
            };
            frame.pc += 1;
            let text = match frame.kind {
                FrameKind::For => dosstr::from_bytes(raw),
                _ => expand(raw, &frame.params[frame.shift.min(frame.params.len())..], env),
            };
            let text = text.trim_start_matches([' ', '\t']);
            if text.trim_end().is_empty() || text.starts_with(':') {
                continue;
            }
            return Some(match text.strip_prefix('@') {
                Some(rest) => BatchLine { text: rest.trim_start().to_string(), echo: false },
                None => BatchLine { text: text.to_string(), echo: self.echo },
            });
        }
    }

    /// GOTO: go on after the label `label` in the batch file running
    /// (ending the FOR it runs from). Labels are compared by their first
    /// eight characters, in any case. False if it has no such label.
    pub fn goto(&mut self, label: &str) -> bool {
        self.drop_for_frames();
        let want = label_key(label.as_bytes());
        let Some(frame) = self.frames.last_mut() else {
            return false;
        };
        let found = frame.lines.iter().position(|line| {
            let line = trim_start(line);
            line.first() == Some(&b':') && label_key(&line[1..]) == want
        });
        match found {
            Some(i) => {
                frame.pc = i + 1;
                true
            }
            None => false,
        }
    }

    /// End the batch file running, as a GOTO to a label it hasn't does.
    pub fn end_file(&mut self) {
        self.drop_for_frames();
        self.frames.pop();
        if self.frames.is_empty() {
            self.echo = self.echo_before;
        }
    }

    /// SHIFT: `%1` becomes `%0`, `%2` `%1` and so on, in the batch file
    /// running.
    pub fn shift(&mut self) {
        if let Some(frame) = self.frames.iter_mut().rev().find(|f| f.kind != FrameKind::For) {
            frame.shift += 1;
        }
    }

    /// Whether a batch file (or a list of lines) runs, for the commands
    /// that only mean something in one, such as GOTO.
    pub fn running(&self) -> bool {
        !self.frames.is_empty()
    }

    /// The lines waiting to run, in the order they will (as they are in
    /// their files, blank lines left out).
    pub fn pending_lines(&self) -> Vec<String> {
        self.frames
            .iter()
            .rev()
            .flat_map(|f| f.lines[f.pc.min(f.lines.len())..].iter())
            .filter(|l| !trim_start(l).is_empty())
            .map(|l| dosstr::from_bytes(l))
            .collect()
    }

    fn push(&mut self, frame: Frame) {
        if self.frames.is_empty() {
            self.echo_before = self.echo;
        }
        self.frames.push(frame);
    }

    fn insert_bottom(&mut self, frame: Frame) {
        if self.frames.is_empty() {
            self.echo_before = self.echo;
        }
        self.frames.insert(0, frame);
    }

    fn drop_for_frames(&mut self) {
        while self.frames.last().is_some_and(|f| f.kind == FrameKind::For) {
            self.frames.pop();
        }
    }
}

/// The lines of a batch file: up to its end of file mark (^Z), split at
/// CR LF, LF or CR.
fn file_lines(bytes: &[u8]) -> Vec<Vec<u8>> {
    let end = bytes.iter().position(|&b| b == 0x1A).unwrap_or(bytes.len());
    let mut lines = Vec::new();
    let mut line = Vec::new();
    let mut after_cr = false;
    for &b in &bytes[..end] {
        match b {
            b'\n' if after_cr => {}
            b'\r' | b'\n' => lines.push(std::mem::take(&mut line)),
            _ => line.push(b),
        }
        after_cr = b == b'\r';
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// `%0` (`name`) and the parameters in `args`, which spaces, tabs, commas,
/// semicolons and equals signs separate. Double quotes keep what they hold
/// together, the quotes included.
pub fn params(name: &str, args: &str) -> Vec<String> {
    let mut params = vec![name.to_string()];
    let mut param = String::new();
    let mut quoted = false;
    for c in args.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                param.push(c);
            }
            ' ' | '\t' | ',' | ';' | '=' if !quoted => {
                if !param.is_empty() {
                    params.push(std::mem::take(&mut param));
                }
            }
            _ => param.push(c),
        }
    }
    if !param.is_empty() {
        params.push(param);
    }
    params
}

/// A batch line with its parameters and variables put in: `%0` to `%9`
/// from `params` (empty past the last), `%NAME%` from `env` (empty if it
/// isn't set), and `%%` as one `%`. A `%` without a closing one goes.
fn expand(line: &[u8], params: &[String], env: &[(String, String)]) -> String {
    let mut out = Vec::with_capacity(line.len());
    let mut i = 0;
    while i < line.len() {
        if line[i] != b'%' {
            out.push(line[i]);
            i += 1;
            continue;
        }
        match line.get(i + 1) {
            Some(b'%') => {
                out.push(b'%');
                i += 2;
            }
            Some(&d @ b'0'..=b'9') => {
                if let Some(param) = params.get((d - b'0') as usize) {
                    out.extend(dosstr::to_bytes(param));
                }
                i += 2;
            }
            Some(_) => match line[i + 1..].iter().position(|&b| b == b'%') {
                Some(len) => {
                    let name = dosstr::from_bytes(&line[i + 1..i + 1 + len]);
                    if let Some((_, value)) = env.iter().find(|(n, _)| n.eq_ignore_ascii_case(&name)) {
                        out.extend(dosstr::to_bytes(value));
                    }
                    i += len + 2;
                }
                None => i += 1,
            },
            None => i += 1,
        }
    }
    dosstr::from_bytes(&out)
}

fn trim_start(line: &[u8]) -> &[u8] {
    let start = line.iter().position(|b| !matches!(b, b' ' | b'\t')).unwrap_or(line.len());
    &line[start..]
}

/// What GOTO compares of a label: its first eight characters, up to a
/// space or separator, in upper case.
fn label_key(label: &[u8]) -> Vec<u8> {
    let label = trim_start(label);
    let label = label.strip_prefix(b":").unwrap_or(label);
    label
        .iter()
        .take_while(|b| !matches!(b, b' ' | b'\t' | b',' | b';' | b'=' | b'+'))
        .take(8)
        .map(u8::to_ascii_uppercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> Vec<(String, String)> {
        vec![("PATH".into(), "C:\\DOS".into()), ("GAME".into(), "KEEN".into())]
    }

    fn run_all(batch: &mut Batch) -> Vec<String> {
        std::iter::from_fn(|| batch.next_line(&env())).map(|l| l.text).collect()
    }

    #[test]
    fn parameters_and_variables_are_put_in() {
        let mut batch = Batch::default();
        batch.append_file("GO", b"echo %0 %1 %2 [%3]\r\necho %GAME% %path% %NONE%.\r\necho 100%% %\r\necho 50% off\r\n", "a b,c");
        assert_eq!(run_all(&mut batch), ["echo GO a b [c]", "echo KEEN C:\\DOS .", "echo 100% ", "echo 50 off"]);
    }

    #[test]
    fn blank_lines_labels_and_the_end_of_file_mark() {
        let mut batch = Batch::default();
        batch.append_file("X", b"\r\n  \r\n:start\r\n:: comment\r\n  echo one\nrem two\recho three\x1Aecho junk", "");
        assert_eq!(run_all(&mut batch), ["echo one", "rem two", "echo three"]);
        assert!(!batch.is_active());
    }

    #[test]
    fn at_hides_a_line_and_echo_comes_back_when_the_batch_ends() {
        let mut batch = Batch::default();
        batch.append_lines(["@echo off", "dir"]);
        assert_eq!(batch.next_line(&[]), Some(BatchLine { text: "echo off".into(), echo: false }));
        batch.echo = false;
        assert_eq!(batch.next_line(&[]), Some(BatchLine { text: "dir".into(), echo: false }));
        assert_eq!(batch.next_line(&[]), None);
        assert!(batch.echo, "ECHO is on again at the prompt");
    }

    #[test]
    fn chaining_replaces_and_call_returns() {
        let mut batch = Batch::default();
        batch.append_lines(["first", "GO", "never"]);
        batch.append_lines(["later"]);
        assert_eq!(batch.next_line(&[]).unwrap().text, "first");
        assert_eq!(batch.next_line(&[]).unwrap().text, "GO");
        batch.dispatching = true;
        batch.start_file("GO", b"in go\r\nCALL SUB\r\nafter sub\r\n", "", false);
        batch.dispatching = false;
        assert_eq!(batch.next_line(&[]).unwrap().text, "in go");
        assert_eq!(batch.next_line(&[]).unwrap().text, "CALL SUB");
        batch.dispatching = true;
        batch.start_file("SUB", b"in sub %1\r\n", "x", true);
        batch.dispatching = false;
        assert_eq!(run_all(&mut batch), ["in sub x", "after sub", "later"]);
    }

    #[test]
    fn goto_finds_labels_by_their_first_eight_characters() {
        let mut batch = Batch::default();
        batch.append_file("X", b"goto end\r\necho skipped\r\n:LOOP\r\n:endofthefile here\r\necho last\r\n", "");
        assert_eq!(batch.next_line(&[]).unwrap().text, "goto end");
        assert!(!batch.goto("nowhere"));
        assert!(batch.goto(":endofthe"));
        assert_eq!(run_all(&mut batch), ["echo last"]);
    }

    #[test]
    fn shift_moves_the_parameters_along() {
        let mut batch = Batch::default();
        batch.append_file("X", b"a %0 %1\r\nb %0 %1 %9\r\n", "one two");
        assert_eq!(batch.next_line(&[]).unwrap().text, "a X one");
        batch.shift();
        assert_eq!(batch.next_line(&[]).unwrap().text, "b one two ");
    }

    #[test]
    fn parameters_split_at_dos_separators_and_keep_quotes_together() {
        assert_eq!(params("X", " a,b;c=d  \"e f\" g"), ["X", "a", "b", "c", "d", "\"e f\"", "g"]);
    }

    #[test]
    fn code_page_437_bytes_come_through() {
        let mut batch = Batch::default();
        batch.append_file("X", b"echo \xC9\xCD\xBB %1\r\n", "\u{84}");
        assert_eq!(dosstr::to_bytes(&batch.next_line(&[]).unwrap().text), b"echo \xC9\xCD\xBB \x84");
    }
}
