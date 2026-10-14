//! Writing settings, drives and `[autoexec]` back into configuration text and files.

use super::settings::entries;
use super::{Section, Settings, TEMPLATE, parse};
use crate::mount::{MountSpec, mount_spec_value, parse_drive_name};
use std::fs;
use std::path::Path;

/// What a line of the file is, for `update_text`.
#[derive(Debug, PartialEq)]
enum Line {
    Header(Section),
    /// `key=value` (the key in lower case), or a drive line in `[drives]`.
    Setting(String),
    /// A commented-out `#key=value`, the template's examples.
    Example(String),
    /// Blank lines, other comments, `[autoexec]` commands.
    Other,
}

/// The section and kind of every line.
fn classify(lines: &[String]) -> Vec<(Section, Line)> {
    let mut section = Section::None;
    let key_of = |text: &str| {
        let key = text.split_once('=')?.0.trim();
        (!key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == ':' || c == '_'))
            .then(|| key.to_ascii_lowercase())
    };
    lines
        .iter()
        .map(|raw| {
            let line = raw.trim();
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                section = Section::parse(name);
                return (section, Line::Header(section));
            }
            let kind = match section {
                Section::Emulator
                | Section::Sound
                | Section::Mixer
                | Section::Joystick
                | Section::Network
                | Section::Serial
                | Section::Printer
                | Section::Achievements
                | Section::Shell
                | Section::Vr
                | Section::Drives => {
                    if let Some(comment) = line.strip_prefix(['#', ';']) {
                        key_of(comment.trim_start_matches(['#', ';']).trim_start())
                            .map_or(Line::Other, Line::Example)
                    } else {
                        key_of(line).map_or(Line::Other, Line::Setting)
                    }
                }
                _ => Line::Other,
            };
            (section, kind)
        })
        .collect()
}

/// Index after the last non-blank line of the last `[section]` block, or
/// None if the file has no such section.
fn section_end(lines: &[String], layout: &[(Section, Line)], section: Section) -> Option<usize> {
    let header = layout.iter().rposition(|(_, l)| *l == Line::Header(section))?;
    let mut last = header;
    for (i, (_, line)) in layout.iter().enumerate().skip(header + 1) {
        if matches!(line, Line::Header(_)) {
            break;
        }
        if !lines[i].trim().is_empty() {
            last = i;
        }
    }
    Some(last + 1)
}

/// Where a new line of `section` goes: after the commented example of
/// `key` if there is one, else at the end of the section. Creates the
/// section, before `[autoexec]` or at the end, if the file has none.
fn insertion_point(lines: &mut Vec<String>, section: Section, key: Option<&str>) -> usize {
    let layout = classify(lines);
    if let Some(key) = key
        && let Some(i) = layout.iter().rposition(|(s, l)| *s == section && *l == Line::Example(key.to_string()))
    {
        return i + 1;
    }
    if let Some(end) = section_end(lines, &layout, section) {
        return end;
    }
    let header = format!("[{}]", section.name());
    match layout.iter().position(|(_, l)| *l == Line::Header(Section::Autoexec)) {
        Some(autoexec) => {
            lines.splice(autoexec..autoexec, [header, String::new()]);
            autoexec + 1
        }
        None => {
            if lines.last().is_some_and(|l| !l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(header);
            lines.len()
        }
    }
}

/// `line` (`key=value`) with a new value, keeping the key as written and
/// the spacing around the '='.
fn with_value(line: &str, value: &str) -> String {
    let eq = line.find('=').unwrap_or(line.len());
    let rest = line.get(eq + 1..).unwrap_or("");
    let gap = rest.len() - rest.trim_start().len();
    format!("{}={}{}", &line[..eq], &rest[..gap], value)
}

/// Set `key` in `section` to `value`: the last line that sets it gets the
/// new value, or a new line is added. With None, the lines that set it
/// are commented out.
fn set_key(lines: &mut Vec<String>, section: Section, key: &str, value: Option<&str>) {
    let layout = classify(lines);
    let setting = Line::Setting(key.to_string());
    let existing: Vec<usize> =
        (0..lines.len()).filter(|&i| layout[i].0 == section && layout[i].1 == setting).collect();
    match (existing.last(), value) {
        (Some(&i), Some(value)) => lines[i] = with_value(&lines[i], value),
        (Some(_), None) => {
            for i in existing {
                lines[i] = format!("#{}", lines[i]);
            }
        }
        (None, Some(value)) => {
            let at = insertion_point(lines, section, Some(key));
            lines.insert(at, format!("{}={}", key, value));
        }
        (None, None) => {}
    }
}

/// Set the `[drives]` line of `drive` to `spec`: the last line for the
/// drive gets the new value, or a line is added after the other drives.
/// With None, the drive's lines are removed.
fn set_drive(lines: &mut Vec<String>, drive: u8, spec: Option<&MountSpec>, home: Option<&Path>) {
    let layout = classify(lines);
    let is_drive = |(s, l): &(Section, Line)| {
        *s == Section::Drives && matches!(l, Line::Setting(k) if parse_drive_name(k).is_some())
    };
    let existing: Vec<usize> = (0..lines.len())
        .filter(|&i| {
            is_drive(&layout[i])
                && matches!(&layout[i].1, Line::Setting(k) if parse_drive_name(k) == Some(drive))
        })
        .collect();
    match (existing.last(), spec) {
        (Some(&i), Some(spec)) => lines[i] = with_value(&lines[i], &mount_spec_value(spec, home)),
        (Some(_), None) => {
            for &i in existing.iter().rev() {
                lines.remove(i);
            }
        }
        (None, Some(spec)) => {
            let line = format!("{}={}", crate::disk::drive_key(drive), mount_spec_value(spec, home));
            let at = match layout.iter().rposition(is_drive) {
                Some(last) => last + 1,
                None => insertion_point(lines, Section::Drives, None),
            };
            lines.insert(at, line);
        }
        (None, None) => {}
    }
}

/// Whether a line of `lines` sets `key` in `section`.
pub(super) fn has_key(lines: &[String], section: Section, key: &str) -> bool {
    let setting = Line::Setting(key.to_string());
    classify(lines).into_iter().any(|(s, line)| s == section && line == setting)
}

/// A change to `[drives]`: the drive's new mount, or None for no drive.
pub type DriveChange = (u8, Option<MountSpec>);

/// `original` with `write` done to its lines and the drive changes written
/// into it, keeping its byte order mark and line ends.
fn edit_text(
    original: &str,
    drives: &[DriveChange],
    home: Option<&Path>,
    write: impl FnOnce(&mut Vec<String>),
) -> String {
    let (bom, text) = match original.strip_prefix('\u{FEFF}') {
        Some(rest) => ("\u{FEFF}", rest),
        None => ("", original),
    };
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let final_newline = text.is_empty() || text.ends_with('\n');
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();

    write(&mut lines);
    for (drive, spec) in drives {
        set_drive(&mut lines, *drive, spec.as_ref(), home);
    }

    let mut out = format!("{}{}", bom, lines.join(newline));
    if final_newline && !lines.is_empty() {
        out.push_str(newline);
    }
    out
}

/// `original` with what changed from `baseline` to `settings`, and the
/// drive changes, written into it. Everything else in the file (comments,
/// blank lines, `[autoexec]`, the settings that didn't change) stays as it
/// is.
pub fn update_text(
    original: &str,
    baseline: &Settings,
    settings: &Settings,
    drives: &[DriveChange],
    home: Option<&Path>,
) -> String {
    edit_text(original, drives, home, |lines| {
        let before = entries(baseline, home);
        for ((section, key, value), (_, _, old)) in entries(settings, home).into_iter().zip(before) {
            if value != old {
                set_key(lines, section, key, value.as_deref());
            }
        }
    })
}

/// `original`, the text of a configuration file whose relative paths are
/// from `base_dir`, with every setting in it: what changed from `baseline`
/// to `settings` is written as `update_text` does, and each setting the
/// file has no line for yet is added with the value the file stood for
/// without it, the default. So the lines already there that didn't change,
/// and the command-line options that stand in for them, stay as they are.
pub fn complete_text(
    original: &str,
    base_dir: &Path,
    baseline: &Settings,
    settings: &Settings,
    drives: &[DriveChange],
    home: Option<&Path>,
) -> String {
    let file = Settings::from_config(&parse(original, base_dir, home));
    edit_text(original, drives, home, |lines| {
        let before = entries(baseline, home).into_iter().zip(entries(&file, home));
        for ((section, key, value), ((_, _, old), (_, _, in_file))) in entries(settings, home).into_iter().zip(before) {
            if value != old {
                set_key(lines, section, key, value.as_deref());
            } else if !has_key(lines, section, key) {
                set_key(lines, section, key, in_file.as_deref());
            }
        }
    })
}

/// What `save` writes of the settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Saving {
    /// Every setting (`complete_text`): the configuration file.
    All,
    /// What changed (`update_text`): a game profile, which has only the
    /// game's own settings.
    Changes,
}

/// Save the settings, as `saving` says, and the drive changes into the
/// configuration file at `path`. The file is replaced in one step, so a
/// failed write leaves the old one.
pub fn save(
    path: &Path,
    baseline: &Settings,
    settings: &Settings,
    drives: &[DriveChange],
    home: Option<&Path>,
    saving: Saving,
) -> Result<(), String> {
    let original = read_or_template(path)?;
    let text = match saving {
        Saving::All => {
            let absolute = crate::hostfs::absolute(path).unwrap_or_else(|_| path.to_path_buf());
            let base_dir = absolute.parent().unwrap_or(Path::new(""));
            complete_text(&original, base_dir, baseline, settings, drives, home)
        }
        Saving::Changes => update_text(&original, baseline, settings, drives, home),
    };
    replace_file(path, &text)
}

/// The lines of `text`'s `[autoexec]` sections as they are written,
/// comments and blank lines too, without the blank lines at the end.
pub fn autoexec_lines(text: &str) -> Vec<String> {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut autoexec = Vec::new();
    for ((section, line), text) in classify(&lines).into_iter().zip(lines) {
        if matches!(line, Line::Header(_)) {
            trim_blank_end(&mut autoexec);
        } else if section == Section::Autoexec {
            autoexec.push(text);
        }
    }
    trim_blank_end(&mut autoexec);
    autoexec
}

fn trim_blank_end(lines: &mut Vec<String>) {
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
}

/// Make `autoexec` the lines of the `[autoexec]` section: the first one's,
/// with any later ones taken out, or a new section at the end.
fn replace_autoexec(lines: &mut Vec<String>, autoexec: &[String]) {
    let mut autoexec = autoexec.to_vec();
    trim_blank_end(&mut autoexec);
    let layout = classify(lines);
    let Some(header) = layout.iter().position(|(_, l)| *l == Line::Header(Section::Autoexec)) else {
        if lines.last().is_some_and(|l| !l.trim().is_empty()) {
            lines.push(String::new());
        }
        lines.push(format!("[{}]", Section::Autoexec.name()));
        lines.extend(autoexec);
        return;
    };
    let rest: Vec<String> = lines
        .drain(header + 1..)
        .zip(&layout[header + 1..])
        .filter(|(_, (section, _))| *section != Section::Autoexec)
        .map(|(line, _)| line)
        .collect();
    lines.append(&mut autoexec);
    // A blank line before the section after it.
    if rest.first().is_some_and(|l| !l.trim().is_empty()) {
        lines.push(String::new());
    }
    lines.extend(rest);
}

/// `original` with `autoexec` as the lines of its `[autoexec]` section,
/// and everything else as it is.
pub fn with_autoexec(original: &str, autoexec: &[String]) -> String {
    edit_text(original, &[], None, |lines| replace_autoexec(lines, autoexec))
}

/// The `[autoexec]` lines of the configuration file at `path` (see
/// `autoexec_lines`), which has the template's text if it isn't there yet.
pub fn load_autoexec(path: &Path) -> Result<Vec<String>, String> {
    read_or_template(path).map(|text| autoexec_lines(&text))
}

/// Make `autoexec` the `[autoexec]` section of the configuration file at
/// `path` (see `load_autoexec`).
pub fn save_autoexec(path: &Path, autoexec: &[String]) -> Result<(), String> {
    let original = read_or_template(path)?;
    replace_file(path, &with_autoexec(&original, autoexec))
}

/// The text of the configuration file at `path`, or the template's if it
/// isn't there yet.
fn read_or_template(path: &Path) -> Result<String, String> {
    match crate::hostfs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(TEMPLATE.to_string()),
        Err(e) => Err(format!("cannot read {}: {}", path.display(), e)),
    }
}

/// Replace the file at `path` with `text` in one step, so a failed write
/// leaves the old one. Where the frontend keeps the files (`hostfs`), it
/// is written as it is.
fn replace_file(path: &Path, text: &str) -> Result<(), String> {
    if crate::hostfs::is_foreign(path) {
        return crate::hostfs::write(path, text).map_err(|e| format!("cannot write {}: {}", path.display(), e));
    }
    // Write through a symbolic link rather than replacing it, and keep the
    // file's permissions.
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut temp_name = target.file_name().unwrap_or_default().to_os_string();
    temp_name.push(".tmp");
    let temp = target.with_file_name(temp_name);
    let permissions = fs::metadata(&target).map(|m| m.permissions()).ok();
    fs::write(&temp, text)
        .and_then(|()| permissions.map_or(Ok(()), |p| fs::set_permissions(&temp, p)))
        .and_then(|()| fs::rename(&temp, &target))
        .map_err(|e| {
            let _ = fs::remove_file(&temp);
            format!("cannot write {}: {}", target.display(), e)
        })
}
