//! What RC needs of the machine it runs in, and what it is handed: the
//! emulator implements `Host` with its DOS file system, and gives RC the
//! keys, the mouse and the screen it found.

/// A file's or directory's attributes, as DOS keeps them.
pub mod attr {
    pub const READ_ONLY: u8 = 0x01;
    pub const HIDDEN: u8 = 0x02;
    pub const SYSTEM: u8 = 0x04;
    pub const DIRECTORY: u8 = 0x10;
    pub const ARCHIVE: u8 = 0x20;
}

/// A file or directory in a directory listing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub size: u64,
    pub attr: u8,
    /// The DOS time and date stamps.
    pub time: u16,
    pub date: u16,
}

impl Entry {
    pub fn is_dir(&self) -> bool {
        self.attr & attr::DIRECTORY != 0
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DriveKind {
    Floppy,
    #[default]
    Hard,
    Cdrom,
    Network,
}

/// A drive DOS has.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Drive {
    /// 0 for A:.
    pub drive: u8,
    pub kind: DriveKind,
    pub writable: bool,
}

/// The machine's side. Paths are qualified DOS paths ("C:\GAMES\X.EXE");
/// errors are DOS error codes (2 file not found, 3 path not found, 5
/// access denied, 13h write protected, ...).
pub trait Host {
    /// The drives there are, A: first.
    fn drives(&self) -> Vec<Drive>;
    /// The current drive, 0 for A:.
    fn current_drive(&self) -> u8;
    /// A drive's current directory, qualified.
    fn current_dir(&self, drive: u8) -> Option<String>;
    /// Make `dir` the current drive and directory, as RC's active panel
    /// is, so that commands run there.
    fn set_current(&mut self, dir: &str);
    /// A directory's files and directories, without "." and "..".
    fn list(&mut self, dir: &str) -> Result<Vec<Entry>, u8>;
    fn read(&mut self, path: &str) -> Result<Vec<u8>, u8>;
    /// Write a whole file, its time and date stamps `stamp` (time, date).
    fn write(&mut self, path: &str, data: &[u8], stamp: (u16, u16)) -> Result<(), u8>;
    fn delete(&mut self, path: &str) -> Result<(), u8>;
    /// Rename or move a file or a directory on one drive.
    fn rename(&mut self, from: &str, to: &str) -> Result<(), u8>;
    fn mkdir(&mut self, path: &str) -> Result<(), u8>;
    fn rmdir(&mut self, path: &str) -> Result<(), u8>;
    /// A file's or directory's attributes; Err(2) when there is none.
    fn attributes(&self, path: &str) -> Result<u8, u8>;
    fn set_attributes(&mut self, path: &str, attr: u8) -> Result<(), u8>;
    /// A drive's size and free space, in bytes.
    fn space(&self, drive: u8) -> Option<(u64, u64)>;
    fn label(&self, drive: u8) -> Option<String>;
    /// Conventional memory: the total and the largest block free.
    fn memory(&mut self) -> (u64, u64);
    /// The DOS prompt as PROMPT makes it, in code page 437.
    fn prompt(&self) -> Vec<u8>;
    /// The command lines typed before, oldest first.
    fn history(&self) -> Vec<String>;
    /// Milliseconds, for double clicks.
    fn now_ms(&self) -> u64;
    /// The time of day: hours, minutes, seconds.
    fn clock(&self) -> (u8, u8, u8);
}

/// The mouse, in text cells.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mouse {
    pub col: usize,
    pub row: usize,
    /// Bit 0 the left button, bit 1 the right one.
    pub buttons: u8,
    /// How often each button was pressed, and where last.
    pub presses: [u16; 2],
    pub pressed_at: [(usize, usize); 2],
}

/// What RC gets on each key, and on each tick between keys.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Input {
    /// The key as INT 16h AH=10h returns it, None on a tick.
    pub key: Option<u16>,
    /// The BIOS's shift flags (0040:0017h): Shift, Ctrl, Alt held.
    pub shift: u8,
    pub mouse: Mouse,
    /// The text screen's size now.
    pub cols: usize,
    pub rows: usize,
}

/// What comes after a key or a tick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// RC goes on; its screen is to be shown again if `redraw`.
    Stay { redraw: bool },
    /// RC has ended.
    Quit,
    /// RC leaves to run a command line, and wants to come back after it.
    /// With `echo` the prompt and the line are to be shown first.
    Run { line: String, echo: bool },
}

/// The DOS screen behind the panels: what Ctrl+O shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UserScreen {
    pub cols: usize,
    pub rows: usize,
    pub cells: Vec<(u8, u8)>,
}

impl UserScreen {
    /// The screen as it was, moved up or down so that the cursor's row
    /// comes to `line`, where RC's command line is.
    pub fn new(
        cols: usize,
        rows: usize,
        cells: Vec<(u8, u8)>,
        cursor_row: usize,
        line: usize,
    ) -> Self {
        let blank = (b' ', 0x07);
        let mut out = vec![blank; cols * rows];
        let shift = cursor_row as isize - line as isize;
        for r in 0..rows {
            let from = r as isize + shift;
            if from >= 0 && (from as usize) < rows {
                let from = from as usize;
                for c in 0..cols {
                    if let Some(&cell) = cells.get(from * cols + c) {
                        out[r * cols + c] = cell;
                    }
                }
            }
        }
        UserScreen {
            cols,
            rows,
            cells: out,
        }
    }

    pub fn get(&self, row: usize, col: usize) -> (u8, u8) {
        if row < self.rows && col < self.cols {
            self.cells[row * self.cols + col]
        } else {
            (b' ', 0x07)
        }
    }
}

rust_dos_savestate::state_fields!(UserScreen { cols, rows, cells });
