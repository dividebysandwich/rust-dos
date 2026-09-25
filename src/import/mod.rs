//! Games set up for DOSBox, made into game profiles (games.rs): a DOSBox
//! configuration file's settings, the drives its `[autoexec]` mounts and
//! the commands that start the game (dosbox.rs), and GOG's installs, which
//! run DOSBox with such files (gog.rs).

pub mod dosbox;
pub mod drop;
pub mod gog;

use crate::mount::{MountSpec, mount_spec_value};
use std::fs;
use std::path::{Path, PathBuf};

/// A game as an imported configuration has it, for a profile.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Imported {
    pub name: String,
    /// (section, key, value) of the settings the configuration sets.
    pub settings: Vec<(&'static str, &'static str, String)>,
    pub drives: Vec<MountSpec>,
    pub autoexec: Vec<String>,
    /// What didn't come across, to tell the user.
    pub warnings: Vec<String>,
}

impl Imported {
    /// Set a setting, in place of what an earlier file set it to.
    fn set(&mut self, section: &'static str, key: &'static str, value: impl Into<String>) {
        self.settings.retain(|(s, k, _)| (*s, *k) != (section, key));
        self.settings.push((section, key, value.into()));
    }

    /// The profile: the game's name, its settings, drives and commands, as
    /// the games folder keeps them. The settings are all written, not only
    /// those that differ from the configuration's, so the game gets the
    /// ones it was set up with.
    pub fn profile_text(&self, home: Option<&Path>) -> String {
        let mut text = format!("[game]\nname={}\n", self.name);
        for section in ["emulator", "sound", "joystick"] {
            let lines: Vec<String> =
                self.settings.iter().filter(|(s, _, _)| *s == section).map(|(_, k, v)| format!("{}={}\n", k, v)).collect();
            if !lines.is_empty() {
                text.push_str(&format!("\n[{}]\n", section));
                text.extend(lines);
            }
        }
        if !self.drives.is_empty() {
            text.push_str("\n[drives]\n");
            for spec in &self.drives {
                let letter = crate::disk::drive_letter(spec.drive);
                text.push_str(&format!("{}={}\n", letter, mount_spec_value(spec, home)));
            }
        }
        text.push_str("\n[autoexec]\n");
        for line in &self.autoexec {
            text.push_str(line);
            text.push('\n');
        }
        text
    }
}

/// `rel` on the host from `base`: a path written for DOSBox on Windows,
/// with backslashes, and with its names in any case, as the files may
/// have been renamed in another. Names that aren't there stay as written.
pub fn host_path(base: &Path, rel: &str) -> PathBuf {
    let rel = rel.trim().trim_matches('"');
    let normalized = rel.replace('\\', "/");
    let path = Path::new(&normalized);
    if path.is_absolute() {
        return path.to_path_buf();
    }
    let mut out = base.to_path_buf();
    for part in normalized.split('/').filter(|p| !p.is_empty() && *p != ".") {
        if part == ".." {
            out.pop();
            continue;
        }
        let exact = out.join(part);
        if exact.exists() {
            out = exact;
            continue;
        }
        let found = fs::read_dir(&out)
            .into_iter()
            .flatten()
            .flatten()
            .find(|entry| entry.file_name().to_string_lossy().eq_ignore_ascii_case(part))
            .map(|entry| entry.path());
        out = found.unwrap_or(exact);
    }
    out
}
