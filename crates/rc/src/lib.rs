//! Rat Commander: a file manager in the style of Norton Commander 4, for
//! rust-dos's DOS prompt. Two panels of files, marking, copying, moving
//! and deleting, a viewer, the directory tree, the pull-down menus, and a
//! command line that runs programs and comes back when they end.
//!
//! RC knows nothing of the machine: it reads and writes files through
//! [`Host`], takes keys, ticks and the mouse as [`Input`], and draws into
//! a [`Screen`] of text cells for the emulator to show. To run a program
//! or open the editor it ends with [`Outcome::Run`]; the emulator runs the
//! line and calls [`Commander::resume`] once the prompt would come back.

mod actions;
pub mod cp437;
mod dialog;
mod draw;
mod host;
mod keys;
mod menu;
mod ops;
mod panel;
mod path;
mod screen;
mod tree;
mod viewer;

#[cfg(test)]
mod tests;

use std::collections::VecDeque;

pub use host::{Drive, DriveKind, Entry, Host, Input, Mouse, Outcome, UserScreen, attr};
pub use screen::Screen;

use dialog::Dialog;
use menu::Bar;
use ops::{Job, Kind, Source};
use panel::Panel;
use tree::Tree;
use viewer::Viewer;

const CTRL: u8 = 0x04;
const ALT: u8 = 0x08;

/// How many bytes an operation copies between keys and ticks.
const JOB_BUDGET: usize = 512 * 1024;

/// The settings of the Options menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Options {
    auto_menus: bool,
    path_prompt: bool,
    key_bar: bool,
    full_screen: bool,
    mini_status: bool,
    clock: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            auto_menus: false,
            path_prompt: true,
            key_bar: true,
            full_screen: true,
            mini_status: true,
            clock: true,
        }
    }
}

rust_dos_savestate::state_fields!(Options {
    auto_menus,
    path_prompt,
    key_bar,
    full_screen,
    mini_status,
    clock
});

/// What a dialog is for.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Purpose {
    Copy {
        sources: Vec<Source>,
        kind: Kind,
    },
    Mkdir,
    Delete {
        sources: Vec<Source>,
        marked: bool,
    },
    Attributes {
        paths: Vec<String>,
    },
    Select {
        on: bool,
    },
    Filter {
        panel: usize,
    },
    Drive {
        panel: usize,
        drives: Vec<u8>,
    },
    Quit,
    History {
        lines: Vec<String>,
    },
    FindAsk,
    /// The NCD tree's directories; with a copy or move's dialog to go back
    /// to with the one chosen.
    Ncd {
        tree: Vec<String>,
        back: Option<Box<(Dialog, Purpose)>>,
    },
    Message,
}

/// Find file going through a drive's directories.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Find {
    mask: String,
    queue: VecDeque<String>,
    found: Vec<String>,
    dialog: Dialog,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum Mode {
    #[default]
    Panels,
    Menu(Bar),
    Dialog(Box<Dialog>, Box<Purpose>),
    /// Alt and letters: the panel's cursor goes to the first name that
    /// starts with what was typed.
    Search(String),
    View(Box<Viewer>),
    Job(Box<Job>),
    Help(usize),
    Find(Box<Find>),
}

/// Rat Commander.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commander {
    panels: [Panel; 2],
    active: usize,
    options: Options,
    cmdline: Vec<u8>,
    cmd_cursor: usize,
    /// Ctrl+O: the panels off, the DOS screen shown.
    panels_off: bool,
    user: UserScreen,
    mode: Mode,
    history_at: Option<usize>,
    trees: Vec<Tree>,
    tree_pos: [(usize, usize); 2],
    /// The file Quick view shows, read once.
    quick: Option<(String, Option<Vec<u8>>)>,
    presses: Option<[u16; 2]>,
    last_click: Option<(u64, usize, usize)>,
    shift: u8,
    shown_second: Option<u8>,
    synced: String,
    screen: Screen,
    cols: usize,
    rows: usize,
}

impl Default for Commander {
    fn default() -> Self {
        Commander {
            panels: [Panel::default(), Panel::default()],
            active: 0,
            options: Options::default(),
            cmdline: Vec::new(),
            cmd_cursor: 0,
            panels_off: false,
            user: UserScreen::default(),
            mode: Mode::Panels,
            history_at: None,
            trees: Vec::new(),
            tree_pos: [(0, 0); 2],
            quick: None,
            presses: None,
            last_click: None,
            shift: 0,
            shown_second: None,
            synced: String::new(),
            screen: Screen::default(),
            cols: 80,
            rows: 25,
        }
    }
}

rust_dos_savestate::state_fields!(Commander {
    panels, active, options, cmdline, cmd_cursor, panels_off, user,
} skip {
    // A menu or dialog open goes, and the panels are read again.
    mode, history_at, trees, tree_pos, quick, presses, last_click, shift,
    shown_second, synced, screen, cols, rows,
});

impl Commander {
    /// RC started on a screen `cols` by `rows`, with the DOS screen `user`
    /// behind its panels; both panels in the current directory.
    pub fn new(host: &mut dyn Host, cols: usize, rows: usize, user: UserScreen) -> Commander {
        let dir = current(host);
        let mut rc = Commander {
            panels: [Panel::at(&dir), Panel::at(&dir)],
            user,
            cols,
            rows,
            ..Commander::default()
        };
        rc.synced = dir;
        rc.sync(host);
        rc.draw(host);
        rc
    }

    /// Back after a command line ran: the DOS screen as it is now, the
    /// panels read again, the active one where a CD took the prompt.
    pub fn resume(&mut self, host: &mut dyn Host, cols: usize, rows: usize, user: UserScreen) {
        self.user = user;
        self.cols = cols;
        self.rows = rows;
        self.mode = Mode::Panels;
        self.presses = None;
        let dir = current(host);
        if !dir.eq_ignore_ascii_case(&self.synced) && self.file_view(self.active) {
            self.panels[self.active].go(&dir, None);
        }
        self.synced = dir;
        for p in &mut self.panels {
            p.stale = true;
        }
        self.trees.clear();
        self.quick = None;
        self.sync(host);
        self.draw(host);
    }

    /// The screen to show.
    pub fn screen(&self) -> &Screen {
        &self.screen
    }

    /// The DOS screen, to put back when RC leaves.
    pub fn user_screen(&self) -> &UserScreen {
        &self.user
    }

    /// The row of the command line, where the DOS cursor goes on leaving.
    pub fn command_row(&self) -> usize {
        self.rows
            .saturating_sub(if self.options.key_bar { 2 } else { 1 })
    }

    /// A key, or a tick with `input.key` None.
    pub fn step(&mut self, host: &mut dyn Host, input: &Input) -> Outcome {
        let mut redraw = false;
        if input.cols != self.cols || input.rows != self.rows {
            self.cols = input.cols.max(40);
            self.rows = input.rows.max(10);
            redraw = true;
        }
        if input.shift & (CTRL | ALT) != self.shift & (CTRL | ALT) {
            redraw = true;
        }
        self.shift = input.shift;
        let outcome;
        if let Some(word) = input.key {
            outcome = self.key(host, keys::decode(word));
            redraw = true;
        } else {
            redraw |= self.run_jobs(host);
            let (o, clicked) = self.mouse(host, &input.mouse);
            outcome = o;
            redraw |= clicked;
            let (_, _, second) = host.clock();
            if self.options.clock && self.shown_second != Some(second) {
                redraw = true;
            }
        }
        if let Some(o) = outcome {
            if matches!(o, Outcome::Run { .. }) {
                self.cmdline.clear();
                self.cmd_cursor = 0;
                self.history_at = None;
            }
            self.sync(host);
            return o;
        }
        if input.key.is_some() {
            // Keys run an operation on too, so that one goes on even when
            // they come faster than the ticks.
            self.run_jobs(host);
        }
        self.sync(host);
        if redraw || self.panels.iter().any(|p| p.stale) {
            self.draw(host);
            redraw = true;
        }
        Outcome::Stay { redraw }
    }
}

/// The current drive's current directory.
fn current(host: &dyn Host) -> String {
    let drive = host.current_drive();
    host.current_dir(drive).unwrap_or_else(|| path::root(drive))
}
