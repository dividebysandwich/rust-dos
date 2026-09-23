//! The emulator log file, `rust-dos.log` in the per-user configuration
//! directory: every `Bus::log_string` line, kept so users can attach it to
//! bug reports. It is recreated on every start, and stops growing at
//! `MAX_BYTES` so a program that keeps triggering a log line can't fill the
//! disk.

use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "rust-dos.log";
/// Size limit of the log file. Later lines only reach the debug server.
pub const MAX_BYTES: u64 = 64 << 20;

/// `<config dir>/rust-dos/rust-dos.log`, e.g.
/// `~/.config/rust-dos/rust-dos.log` on Linux.
pub fn default_path() -> Option<PathBuf> {
    crate::config::user_dir().map(|d| d.join(FILE_NAME))
}

pub struct LogFile {
    writer: BufWriter<File>,
    written: u64,
    limit: u64,
}

impl LogFile {
    /// Create the log file at `path`, replacing the previous run's, and
    /// start it with the emulator version and the time.
    pub fn create(path: &Path) -> io::Result<Self> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut log = Self::new(File::create(path)?, MAX_BYTES);
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
        }
    }

    pub fn write_line(&mut self, line: &str) {
        if self.written >= self.limit {
            return;
        }
        let len = line.len() as u64 + 1;
        if self.written + len > self.limit {
            let _ = writeln!(
                self.writer,
                "[LOG] Log file size limit reached, not logging more"
            );
            self.written = self.limit;
            return;
        }
        self.written += len;
        let _ = writeln!(self.writer, "{}", line);
    }

    pub fn flush(&mut self) {
        let _ = self.writer.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rust-dos-log-{}-{}", std::process::id(), name))
    }

    #[test]
    fn create_replaces_the_previous_log() {
        let path = temp_path("replace");
        fs::write(&path, "old run\n").unwrap();
        let mut log = LogFile::create(&path).unwrap();
        log.write_line("[DOS] new run");
        log.flush();
        let text = fs::read_to_string(&path).unwrap();
        fs::remove_file(&path).unwrap();
        assert!(text.starts_with("rust-dos "), "{text}");
        assert!(text.ends_with("[DOS] new run\n"), "{text}");
        assert!(!text.contains("old run"), "{text}");
    }

    #[test]
    fn lines_past_the_limit_are_dropped() {
        let path = temp_path("limit");
        let mut log = LogFile::new(File::create(&path).unwrap(), 10);
        log.write_line("12345");
        log.write_line("67890");
        log.write_line("abc");
        log.flush();
        let text = fs::read_to_string(&path).unwrap();
        fs::remove_file(&path).unwrap();
        assert_eq!(
            text,
            "12345\n[LOG] Log file size limit reached, not logging more\n"
        );
    }
}
