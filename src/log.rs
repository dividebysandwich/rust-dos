//! The emulator log file, `rust-dos.log` in the per-user configuration
//! directory or the file `--log` names: every `Bus::log_string` line, kept
//! so users can attach it to bug reports. It is recreated on every start.
//!
//! It holds at most `MAX_BYTES`, so a program that keeps triggering a log
//! line can't fill the disk, and keeps the lines that matter most: those
//! from the start, and the latest. Its first quarter, the head, stays as
//! written. When the file is full, the lines after the head are cut down
//! to the latest that fill half the room left, behind a line that says
//! how much was left out, and the log goes on after them. The file stays
//! one file with its lines in the order they came.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "rust-dos.log";
/// Size limit of the log file.
pub const MAX_BYTES: u64 = 64 << 20;

const STOPPED: &str = "[LOG] The log file can't be cut down, not logging more";

/// `<config dir>/rust-dos/rust-dos.log`, e.g.
/// `~/.config/rust-dos/rust-dos.log` on Linux.
pub fn default_path() -> Option<PathBuf> {
    crate::config::user_dir().map(|d| d.join(FILE_NAME))
}

pub struct LogFile {
    writer: BufWriter<File>,
    written: u64,
    limit: u64,
    /// Where the head ends once it is written: after the line that
    /// reaches a quarter of `limit`.
    head: Option<u64>,
    /// Where the lines after the head start: after the head and the line
    /// about the lines left out, once there is one.
    tail: u64,
    /// The bytes of lines left out so far.
    dropped: u64,
    /// Whether the log stopped, after it failed to cut the file down.
    stopped: bool,
}

impl LogFile {
    /// Create the log file at `path`, replacing the previous run's, and
    /// start it with the emulator version and the time.
    pub fn create(path: &Path) -> io::Result<Self> {
        Self::create_with_limit(path, MAX_BYTES)
    }

    /// The same, holding at most `limit` bytes.
    pub fn create_with_limit(path: &Path, limit: u64) -> io::Result<Self> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut log = Self::new(open(path)?, limit);
        log.write_line(&format!(
            "rust-dos {} started {}",
            env!("CARGO_PKG_VERSION"),
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
        ));
        Ok(log)
    }

    fn new(file: File, limit: u64) -> Self {
        Self {
            writer: BufWriter::new(file),
            written: 0,
            limit,
            head: None,
            tail: 0,
            dropped: 0,
            stopped: false,
        }
    }

    pub fn write_line(&mut self, line: &str) {
        if self.stopped {
            return;
        }
        let len = line.len() as u64 + 1;
        if self.written + len > self.limit {
            match self.cut(len) {
                Ok(true) => {}
                // The line is longer than a cut can make room for.
                Ok(false) => return,
                Err(_) => {
                    // At the end, wherever the failed cut left the
                    // file's position.
                    let _ = self.writer.seek(SeekFrom::End(0));
                    let _ = writeln!(self.writer, "{}", STOPPED);
                    self.stopped = true;
                    return;
                }
            }
        }
        self.written += len;
        let _ = writeln!(self.writer, "{}", line);
        if self.head.is_none() && self.written >= self.limit / 4 {
            self.head = Some(self.written);
            self.tail = self.written;
        }
    }

    /// Leave out the lines after the head but the latest, to make room
    /// for a line of `len` bytes. False when no cut can.
    fn cut(&mut self, len: u64) -> io::Result<bool> {
        let Some(head) = self.head else {
            return Ok(false);
        };
        // The note's count only grows: the room is measured with the
        // longest one.
        let room = self
            .limit
            .saturating_sub(head + note(u64::MAX).len() as u64);
        if room < len {
            return Ok(false);
        }
        self.writer.flush()?;
        let keep = (room / 2).min(room - len);
        let mut from = self.written.saturating_sub(keep).max(self.tail);
        // From the first whole line on: read the byte before `from` too,
        // so that a line starting at `from` counts as whole.
        let whole = from > self.tail;
        if whole {
            from -= 1;
        }
        let file = self.writer.get_mut();
        file.seek(SeekFrom::Start(from))?;
        let mut latest = Vec::new();
        file.read_to_end(&mut latest)?;
        let skip = if whole {
            latest
                .iter()
                .position(|&b| b == b'\n')
                .map_or(latest.len(), |i| i + 1)
        } else {
            0
        };
        self.dropped += from + skip as u64 - self.tail;
        let note = note(self.dropped);
        file.seek(SeekFrom::Start(head))?;
        file.write_all(note.as_bytes())?;
        file.write_all(&latest[skip..])?;
        self.tail = head + note.len() as u64;
        self.written = self.tail + (latest.len() - skip) as u64;
        file.set_len(self.written)?;
        Ok(true)
    }

    pub fn flush(&mut self) {
        let _ = self.writer.flush();
    }
}

/// The line in place of the lines left out, `dropped` bytes of them.
fn note(dropped: u64) -> String {
    format!(
        "[LOG] {} bytes of lines left out here: the log file keeps its start and its latest lines\n",
        dropped
    )
}

/// The file at `path`, empty, to write and read back.
fn open(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        crate::test_dir::path(&format!("log-{name}"))
    }

    /// What `log` wrote to `path`, which goes.
    fn text(mut log: LogFile, path: &Path) -> String {
        log.flush();
        drop(log);
        let text = fs::read_to_string(path).unwrap();
        fs::remove_file(path).unwrap();
        text
    }

    #[test]
    fn create_replaces_the_previous_log() {
        let path = temp_path("replace");
        fs::write(&path, "old run\n").unwrap();
        let mut log = LogFile::create(&path).unwrap();
        log.write_line("[DOS] new run");
        let text = text(log, &path);
        assert!(text.starts_with("rust-dos "), "{text}");
        assert!(text.ends_with("[DOS] new run\n"), "{text}");
        assert!(!text.contains("old run"), "{text}");
    }

    #[test]
    fn a_full_log_keeps_its_head_and_its_latest_lines() {
        let path = temp_path("rolling");
        let limit = 4000;
        let mut log = LogFile::new(open(&path).unwrap(), limit);
        let lines: Vec<String> = (0..2000).map(|i| format!("line {i:04}")).collect();
        for line in &lines {
            log.write_line(line);
            assert!(log.written <= limit);
        }
        let text = text(log, &path);
        assert!(text.len() as u64 <= limit, "{}", text.len());
        let kept: Vec<&str> = text.lines().collect();
        // The head: the lines up to a quarter of the limit, 10 bytes each.
        assert_eq!(kept[..100], lines[..100]);
        let note = kept[100];
        assert!(
            note.starts_with("[LOG] ") && note.contains(" bytes of lines left out"),
            "{note}"
        );
        // Then the latest lines, one after the other up to the last.
        let tail = &kept[101..];
        assert!(tail.len() > 100, "{}", tail.len());
        assert_eq!(tail[..], lines[lines.len() - tail.len()..]);
        // The note counts the bytes of the lines between them.
        let dropped: u64 = note[6..].split(' ').next().unwrap().parse().unwrap();
        assert_eq!(dropped, (lines.len() - 100 - tail.len()) as u64 * 10);
    }

    #[test]
    fn a_line_longer_than_a_cut_makes_room_for_is_left_out() {
        let path = temp_path("long");
        let mut log = LogFile::new(open(&path).unwrap(), 1000);
        for i in 0..30 {
            log.write_line(&format!("line {i:04}"));
        }
        log.write_line(&"x".repeat(2000));
        log.write_line("after");
        let text = text(log, &path);
        assert!(!text.contains("xxx"), "{text}");
        assert!(text.ends_with("line 0029\nafter\n"), "{text}");
    }

    #[test]
    fn a_log_too_small_to_cut_leaves_out_what_does_not_fit() {
        let path = temp_path("small");
        let mut log = LogFile::new(open(&path).unwrap(), 10);
        log.write_line("12345");
        log.write_line("67890");
        log.write_line("abc");
        assert_eq!(text(log, &path), "12345\nabc\n");
    }
}
