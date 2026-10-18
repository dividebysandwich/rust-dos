//! Copying, moving and deleting: an operation is a queue of steps, run a
//! few at a time between keys and ticks so that the machine goes on, with
//! a box showing what it does and questions where NC asks them.

use std::collections::VecDeque;

use crate::dialog::{Button, Dialog, Event, Item};
use crate::host::{Entry, Host, attr};
use crate::path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Copy,
    Move,
    Delete,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Step {
    /// The next source starts: what goes wrong from here on keeps it
    /// from being deleted after a move.
    Begin,
    MakeDir(String),
    /// A directory's contents, copied into `to` (made already).
    CopyDir {
        from: String,
        to: String,
    },
    CopyFile {
        from: String,
        to: String,
        entry: Entry,
    },
    Rename {
        from: String,
        to: String,
        is_dir: bool,
    },
    /// After a move across drives: the source goes, if all of it was
    /// copied.
    DeleteIfCopied {
        path: String,
        is_dir: bool,
    },
    /// A file to delete; asked about first with `ask`.
    DeleteFile {
        path: String,
        ask: bool,
    },
    /// A directory and what is in it; `asked` once it was confirmed.
    DeleteDir {
        path: String,
        asked: bool,
    },
    RemoveDir(String),
}

/// What the operation waits for an answer about.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Question {
    dialog: Dialog,
    step: Step,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub kind: Kind,
    steps: VecDeque<Step>,
    question: Option<Question>,
    pub progress: Dialog,
    overwrite_all: bool,
    delete_all: bool,
    /// Directories are deleted with what is in them without asking.
    subdirs: bool,
    /// The step put back after its question was answered goes ahead.
    answered: bool,
    /// Errors and skips since the last `Begin`.
    trouble: usize,
    /// Directories made or removed, so that trees are read again.
    pub dirs_changed: bool,
    pub cancelled: bool,
    /// Where a single file or directory went, for the cursor to follow.
    pub follow: Option<String>,
}

/// The text of a DOS error code.
pub fn error_text(code: u8) -> String {
    match code {
        0x02 => "File not found".to_string(),
        0x03 => "Path not found".to_string(),
        0x04 => "Too many open files".to_string(),
        0x05 => "Access denied".to_string(),
        0x10 => "Cannot remove the current directory".to_string(),
        0x11 => "Not the same device".to_string(),
        0x0D => "Not a ZIP archive that can be read".to_string(),
        0x13 => "Disk is write-protected".to_string(),
        0x15 => "Drive not ready".to_string(),
        0x1D | 0x27 => "Disk full".to_string(),
        0x50 | 0xB7 => "The file already exists".to_string(),
        code => format!("DOS error {:02X}h", code),
    }
}

/// A file to work on and its directory entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub path: String,
    pub entry: Entry,
}

/// A copy or a move of `sources` to what was typed, from directory `dir`.
pub fn plan_copy(
    host: &mut dyn Host,
    sources: &[Source],
    dir: &str,
    typed: &str,
    kind: Kind,
) -> Result<Job, String> {
    let current = |d: u8| host.current_dir(d).unwrap_or_else(|| path::root(d));
    let typed = typed.trim();
    if typed.is_empty() {
        return Err("No target was given".to_string());
    }
    let target = path::qualify(dir, typed, current);
    // A zip archive given is copied into, as a directory is.
    let into_dir = typed.ends_with('\\')
        || typed.ends_with(':')
        || host
            .attributes(&target)
            .is_ok_and(|a| a & attr::DIRECTORY != 0)
        || (crate::archive::is_zip_name(path::name(&target)) && host.attributes(&target).is_ok());
    let wild = path::has_wildcards(path::name(&target));
    let mut steps = VecDeque::new();
    let dest = |name: &str| -> String {
        if into_dir {
            path::join(&target, name)
        } else if wild {
            let parent = path::parent(&target).unwrap_or_else(|| target.clone());
            path::join(&parent, &path::template(path::name(&target), name))
        } else if sources.len() == 1 {
            target.clone()
        } else {
            path::join(&target, name)
        }
    };
    if !into_dir && !wild && sources.len() > 1 {
        steps.push_back(Step::MakeDir(target.clone()));
    }
    for src in sources {
        let to = dest(&src.entry.name);
        let is_dir = src.entry.is_dir();
        if is_dir && path::within(&to, &src.path) {
            let verb = if kind == Kind::Move { "move" } else { "copy" };
            return Err(format!("You cannot {} a directory into itself", verb));
        }
        steps.push_back(Step::Begin);
        let same_drive = path::drive(&to) == path::drive(&src.path);
        if kind == Kind::Move && same_drive {
            steps.push_back(Step::Rename {
                from: src.path.clone(),
                to,
                is_dir,
            });
            continue;
        }
        push_copy(&mut steps, src, to);
        if kind == Kind::Move {
            steps.push_back(Step::DeleteIfCopied {
                path: src.path.clone(),
                is_dir,
            });
        }
    }
    let title = if kind == Kind::Move { "Rename" } else { "Copy" };
    let mut job = Job::new(kind, steps, title);
    if let [one] = sources {
        job.follow = Some(dest(&one.entry.name));
    }
    Ok(job)
}

fn push_copy(steps: &mut VecDeque<Step>, src: &Source, to: String) {
    if src.entry.is_dir() {
        steps.push_back(Step::MakeDir(to.clone()));
        steps.push_back(Step::CopyDir {
            from: src.path.clone(),
            to,
        });
    } else {
        steps.push_back(Step::CopyFile {
            from: src.path.clone(),
            to,
            entry: src.entry.clone(),
        });
    }
}

/// Deleting `sources`; directories with what is in them when `subdirs`,
/// each file confirmed when `confirm_each`.
pub fn plan_delete(sources: &[Source], subdirs: bool, confirm_each: bool) -> Job {
    let mut steps = VecDeque::new();
    for src in sources {
        steps.push_back(if src.entry.is_dir() {
            Step::DeleteDir {
                path: src.path.clone(),
                asked: false,
            }
        } else {
            Step::DeleteFile {
                path: src.path.clone(),
                ask: confirm_each,
            }
        });
    }
    let mut job = Job::new(Kind::Delete, steps, "Delete");
    job.subdirs = subdirs;
    job
}

impl Job {
    fn new(kind: Kind, steps: VecDeque<Step>, title: &str) -> Job {
        let progress = Dialog::new(
            title,
            41,
            vec![
                Item::centred(""),
                Item::centred(""),
                Item::centred(""),
                Item::centred(""),
            ],
        );
        Job {
            kind,
            steps,
            question: None,
            progress,
            overwrite_all: false,
            delete_all: false,
            subdirs: false,
            answered: false,
            trouble: 0,
            dirs_changed: false,
            cancelled: false,
            follow: None,
        }
    }

    pub fn done(&self) -> bool {
        self.steps.is_empty() && self.question.is_none()
    }

    /// The question it waits on, if one.
    pub fn question(&mut self) -> Option<&mut Dialog> {
        self.question.as_mut().map(|q| &mut q.dialog)
    }

    /// An answer to the question.
    pub fn answer(&mut self, event: Event) {
        let Some(q) = self.question.take() else {
            return;
        };
        match event {
            Event::None => self.question = Some(q),
            Event::Cancel | Event::Button(Button::Cancel) => self.cancel(),
            Event::Button(Button::Skip) => self.trouble += 1,
            Event::Button(Button::Retry) => self.steps.push_front(q.step),
            Event::Button(b) => {
                if b == Button::All {
                    self.overwrite_all = true;
                    self.delete_all = true;
                }
                self.answered = true;
                self.steps.push_front(q.step);
            }
        }
    }

    /// Esc while it runs.
    pub fn cancel(&mut self) {
        self.steps.clear();
        self.question = None;
        self.cancelled = true;
    }

    /// Run steps until about `budget` bytes were copied or 32 steps went,
    /// or a question comes up.
    pub fn run(&mut self, host: &mut dyn Host, budget: usize) {
        let mut bytes = 0;
        let mut steps = 0;
        while self.question.is_none() && bytes < budget && steps < 32 {
            let Some(step) = self.steps.pop_front() else {
                break;
            };
            steps += 1;
            bytes += self.step(host, step);
        }
    }

    fn ask(&mut self, step: Step, dialog: Dialog) {
        self.question = Some(Question { dialog, step });
    }

    fn fail(&mut self, step: Step, what: &str, code: u8) {
        let text = error_text(code);
        let dialog = Dialog::message(
            "Error",
            &[what, &text],
            &[
                ("Retry", Button::Retry),
                ("Skip", Button::Skip),
                ("Cancel", Button::Cancel),
            ],
            true,
        );
        self.ask(step, dialog);
    }

    fn show(&mut self, lines: [&str; 4]) {
        for (i, l) in lines.iter().enumerate() {
            self.progress.set_text(i, l);
        }
    }

    /// One step; the bytes it copied.
    fn step(&mut self, host: &mut dyn Host, step: Step) -> usize {
        match step.clone() {
            Step::Begin => self.trouble = 0,
            Step::MakeDir(dir) => match host.attributes(&dir) {
                Ok(a) if a & attr::DIRECTORY != 0 => {}
                _ => {
                    if let Err(e) = make_dirs(host, &dir) {
                        self.fail(step, &format!("Cannot create the directory {}", dir), e);
                    } else {
                        self.dirs_changed = true;
                    }
                }
            },
            Step::CopyDir { from, to } => match host.list(&from) {
                Ok(list) => {
                    let mut add = Vec::new();
                    for entry in list.into_iter().filter(|e| e.name != "." && e.name != "..") {
                        let src = Source {
                            path: path::join(&from, &entry.name),
                            entry: entry.clone(),
                        };
                        let mut part = VecDeque::new();
                        push_copy(&mut part, &src, path::join(&to, &entry.name));
                        add.extend(part);
                    }
                    for s in add.into_iter().rev() {
                        self.steps.push_front(s);
                    }
                }
                Err(e) => self.fail(step, &format!("Cannot read the directory {}", from), e),
            },
            Step::CopyFile { from, to, entry } => {
                let verb = if self.kind == Kind::Move {
                    "Moving"
                } else {
                    "Copying"
                };
                self.show([
                    &format!("{} the file or directory", verb),
                    path::name(&from),
                    "to",
                    &to,
                ]);
                if from.eq_ignore_ascii_case(&to) {
                    let dialog = Dialog::message(
                        "Error",
                        &["You cannot copy a file to itself", &from],
                        &[("Skip", Button::Skip), ("Cancel", Button::Cancel)],
                        true,
                    );
                    self.question = Some(Question { dialog, step });
                    self.trouble += 1;
                    return 0;
                }
                if let Ok(a) = host.attributes(&to) {
                    if a & attr::DIRECTORY != 0 {
                        self.fail(step, &format!("Cannot create the file {}", to), 0x05);
                        return 0;
                    }
                    if !self.overwrite_all && !self.answered {
                        self.ask_overwrite(step, &to);
                        return 0;
                    }
                    if a & attr::READ_ONLY != 0 {
                        let _ = host.set_attributes(&to, a & !attr::READ_ONLY);
                    }
                }
                self.answered = false;
                let data = match host.read(&from) {
                    Ok(d) => d,
                    Err(e) => {
                        self.fail(step, &format!("Cannot read the file {}", from), e);
                        return 0;
                    }
                };
                if let Err(e) = host.write(&to, &data, (entry.time, entry.date)) {
                    self.fail(step, &format!("Cannot write the file {}", to), e);
                    return 0;
                }
                return data.len().max(1);
            }
            Step::Rename { from, to, is_dir } => {
                self.show(["Moving the file or directory", path::name(&from), "to", &to]);
                let exists = host.attributes(&to).is_ok() && !from.eq_ignore_ascii_case(&to);
                if exists && !is_dir {
                    if !self.overwrite_all && !self.answered {
                        self.ask_overwrite(step, &to);
                        return 0;
                    }
                    self.answered = false;
                    if let Err(e) = host.delete(&to) {
                        self.fail(step, &format!("Cannot delete the file {}", to), e);
                        return 0;
                    }
                }
                match host.rename(&from, &to) {
                    Ok(()) => {
                        if is_dir {
                            self.dirs_changed = true;
                        }
                    }
                    // Where a drive can't move it, it is copied and the
                    // source deleted.
                    // A file into or out of a zip archive likewise.
                    Err(code)
                        if code == 0x11 && (!is_dir || !exists)
                            || code == 0x05 && is_dir && !exists =>
                    {
                        let name = path::name(&from).to_string();
                        // A file's stamp goes with it.
                        let listed = path::parent(&from)
                            .and_then(|dir| host.list(&dir).ok())
                            .and_then(|l| {
                                l.into_iter().find(|e| e.name.eq_ignore_ascii_case(&name))
                            });
                        let entry = listed.unwrap_or(Entry {
                            name,
                            attr: if is_dir { attr::DIRECTORY } else { 0 },
                            ..Entry::default()
                        });
                        let src = Source {
                            path: from.clone(),
                            entry,
                        };
                        let mut part = VecDeque::new();
                        push_copy(&mut part, &src, to);
                        part.push_back(Step::DeleteIfCopied { path: from, is_dir });
                        for s in part.into_iter().rev() {
                            self.steps.push_front(s);
                        }
                    }
                    Err(e) => self.fail(step, &format!("Cannot move {}", from), e),
                }
            }
            Step::DeleteIfCopied { path, is_dir } => {
                if self.trouble == 0 {
                    self.steps.push_front(if is_dir {
                        Step::DeleteDir { path, asked: true }
                    } else {
                        Step::DeleteFile { path, ask: false }
                    });
                }
            }
            Step::DeleteFile { path: file, ask } => {
                self.show(["Deleting the file", path::name(&file), "", ""]);
                if ask && !self.delete_all && !self.answered {
                    let dialog = Dialog::message(
                        "Delete File",
                        &["The following file will be DELETED:", &file],
                        &[
                            ("Delete", Button::Delete),
                            ("All", Button::All),
                            ("Skip", Button::Skip),
                            ("Cancel", Button::Cancel),
                        ],
                        true,
                    );
                    self.ask(step, dialog);
                    return 0;
                }
                self.answered = false;
                if let Ok(a) = host.attributes(&file)
                    && a & attr::READ_ONLY != 0
                {
                    let _ = host.set_attributes(&file, a & !attr::READ_ONLY);
                }
                if let Err(e) = host.delete(&file) {
                    self.fail(step, &format!("Cannot delete the file {}", file), e);
                }
            }
            Step::DeleteDir { path: dir, asked } => {
                self.show(["Deleting the directory", path::name(&dir), "", ""]);
                let list = match host.list(&dir) {
                    Ok(l) => l,
                    Err(e) => {
                        self.fail(step, &format!("Cannot read the directory {}", dir), e);
                        return 0;
                    }
                };
                let list: Vec<Entry> = list
                    .into_iter()
                    .filter(|e| e.name != "." && e.name != "..")
                    .collect();
                if !list.is_empty() && !asked && !self.subdirs && !self.delete_all {
                    let dialog = Dialog::message(
                        "Delete",
                        &["The following directory will be DELETED:", &dir],
                        &[
                            ("Delete", Button::Delete),
                            ("All", Button::All),
                            ("Skip", Button::Skip),
                            ("Cancel", Button::Cancel),
                        ],
                        true,
                    );
                    self.ask(
                        Step::DeleteDir {
                            path: dir,
                            asked: true,
                        },
                        dialog,
                    );
                    return 0;
                }
                self.steps.push_front(Step::RemoveDir(dir.clone()));
                for e in list.into_iter().rev() {
                    let p = path::join(&dir, &e.name);
                    // What is in a directory goes without asking about
                    // each of its files.
                    self.steps.push_front(if e.is_dir() {
                        Step::DeleteDir {
                            path: p,
                            asked: true,
                        }
                    } else {
                        Step::DeleteFile {
                            path: p,
                            ask: false,
                        }
                    });
                }
            }
            Step::RemoveDir(dir) => {
                if let Err(e) = host.rmdir(&dir) {
                    self.fail(step, &format!("Cannot remove the directory {}", dir), e);
                } else {
                    self.dirs_changed = true;
                }
            }
        }
        0
    }

    fn ask_overwrite(&mut self, step: Step, to: &str) {
        let title = if self.kind == Kind::Move {
            "Rename"
        } else {
            "Copy"
        };
        let dialog = Dialog::message(
            title,
            &[
                &format!("The file {}", path::shown(to)),
                "already exists.",
                "Do you wish to write over the target file?",
            ],
            &[
                ("Overwrite", Button::Overwrite),
                ("All", Button::All),
                ("Skip", Button::Skip),
                ("Cancel", Button::Cancel),
            ],
            true,
        );
        self.ask(step, dialog);
    }
}

/// Make a directory and those above it it needs.
fn make_dirs(host: &mut dyn Host, dir: &str) -> Result<(), u8> {
    let mut missing = Vec::new();
    let mut at = dir.to_string();
    while host.attributes(&at).is_err() {
        missing.push(at.clone());
        match path::parent(&at) {
            Some(up) => at = up,
            None => break,
        }
    }
    for d in missing.into_iter().rev() {
        host.mkdir(&d)?;
    }
    Ok(())
}
