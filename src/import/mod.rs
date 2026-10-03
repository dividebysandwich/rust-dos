//! Games set up for DOSBox, made into game profiles (games.rs): a DOSBox
//! configuration file's settings, the drives its `[autoexec]` mounts and
//! the commands that start the game (dosbox.rs), and GOG's installs, which
//! run DOSBox with such files (gog.rs).

pub mod dos_yml;
pub mod dosbox;
pub mod drop;
pub mod gog;

use crate::mount::{MountSpec, mount_spec_value};
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
        let mut text = format!("[game]\nname={}\noverlay=true\n", self.name);
        text.push_str(&self.settings_text());
        if !self.drives.is_empty() {
            text.push_str("\n[drives]\n");
            for spec in &self.drives {
                let key = crate::disk::drive_key(spec.drive);
                text.push_str(&format!("{}={}\n", key, mount_spec_value(spec, home)));
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

impl Imported {
    /// `over`'s settings over these, its drives in place of these on the
    /// same letters, and its commands if it has any but C: (which a
    /// configuration without commands comes to).
    pub fn merge(&mut self, over: Imported) {
        for (section, key, value) in over.settings {
            self.set(section, key, value);
        }
        for spec in over.drives {
            self.drives.retain(|d| d.drive != spec.drive);
            self.drives.push(spec);
        }
        if over.autoexec.iter().any(|l| !l.eq_ignore_ascii_case("C:")) || self.autoexec.is_empty() {
            self.autoexec = over.autoexec;
        }
        self.warnings.extend(over.warnings);
    }

    /// The settings as a profile's sections, each after a blank line.
    pub fn settings_text(&self) -> String {
        let mut text = String::new();
        for section in ["emulator", "sound", "mixer", "joystick", "network", "serial", "printer"] {
            let lines: Vec<String> =
                self.settings.iter().filter(|(s, _, _)| *s == section).map(|(_, k, v)| format!("{}={}\n", k, v)).collect();
            if !lines.is_empty() {
                text.push_str(&format!("\n[{}]\n", section));
                text.extend(lines);
            }
        }
        text
    }

    /// Leave out the settings rust-dos wouldn't take (an IRQ its card
    /// can't have, say), with a warning, so the profile has none.
    fn drop_invalid(&mut self) {
        let mut warnings = Vec::new();
        self.settings.retain(|(section, key, value)| {
            let text = format!("[{}]\n{}={}\n", section, key, value);
            // The line's own errors, not those between it and the defaults.
            let config = crate::config::parse(&text, Path::new("/"), None);
            let errors: Vec<&str> = config.warnings.iter().filter_map(|w| w.strip_prefix("line 2: ")).collect();
            for error in &errors {
                warnings.push(format!("{}={} isn't imported: {}", key, value, error));
            }
            errors.is_empty()
        });
        self.warnings.extend(warnings);
    }
}

/// `rel` on the host from `base`: a path written for DOSBox on Windows,
/// with backslashes, and with its names in any case, as the files may
/// have been renamed in another. Names that aren't there stay as written.
pub fn host_path(base: &Path, rel: &str) -> PathBuf {
    let rel = rel.trim().trim_matches('"');
    let normalized = rel.replace('\\', "/");
    let path = Path::new(&normalized);
    if path.is_absolute() || crate::hostfs::has_scheme(path) {
        return path.to_path_buf();
    }
    let mut out = base.to_path_buf();
    for part in normalized.split('/').filter(|p| !p.is_empty() && *p != ".") {
        if part == ".." {
            out.pop();
            continue;
        }
        let exact = out.join(part);
        if crate::hostfs::exists(&exact) {
            out = exact;
            continue;
        }
        let found = crate::hostfs::read_dir(&out)
            .into_iter()
            .flatten()
            .find(|entry| entry.name.to_string_lossy().eq_ignore_ascii_case(part))
            .map(|entry| entry.path);
        out = found.unwrap_or(exact);
    }
    out
}
