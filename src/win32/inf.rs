//! INF files, Windows 95's setup scripts, and what an install section of
//! one does: the engine of SETUPX.DLL's generic installer (GenInstall),
//! which the 16-bit SETUPX (win16/setupx.rs) runs for the programs that
//! call it, and which a SETUPAPI or ADVPACK of the built-in Windows can
//! run as well.
//!
//! An INF file is sections of lines. A line has fields separated by
//! commas, and may have a key before an '='; a field in quotes keeps its
//! commas, spaces and semicolons, and "" in it is a quote. ';' starts a
//! comment, and a '\' at the end of a line goes on to the next line.
//! Sections of the same name are one section. `%name%` in a field is the
//! string of that name in [Strings], `%number%` the directory of that
//! logical disk id (LDID), and `%%` a '%'.
//!
//! An install section names what to do in its lines:
//!
//! - CopyFiles, DelFiles and RenFiles: sections of files, copied from the
//!   source (LDID 1, which the caller sets) to the directory
//!   [DestinationDirs] gives each section, deleted from it or renamed in
//!   it. `CopyFiles=@name` copies the one file. These are queued
//!   (`FileOp`) and carried out by `commit`, as SETUPX's virtual copy
//!   queue does when VcpClose closes it.
//! - AddReg and DelReg: sections of registry values to write or delete.
//! - UpdateInis: lines of INI files to add, change or remove.
//!
//! A copy over a file that is there keeps the newer one as SETUPX's
//! version check does: when both have version resources and the file in
//! place is newer, it stays (with the flag 4, COPYFLG_NOVERSIONCHECK, the
//! source is copied whatever its version). Windows 98 asks the user
//! whether to keep it unless the flag 32 says to keep it quietly; the
//! answer it suggests is to keep it, which is what happens here. A file
//! that a program has open gets the copy under another name, put in its
//! place at the next start through WININIT.INI, as Windows does with a
//! file in use. The files of the built-in DLLs that the system folder
//! shows have the versions of the version of Windows the built-in one
//! is, so a setup that brings an older MSVCRT.DLL leaves the built-in
//! one in place, as on Windows 98 SE.
//!
//! What a section asks for that isn't done here (Ini2Reg, UpdateCfgSys,
//! LogConfig and the like, and the registry root HKR, which is a device's
//! key) is written to the log and left out.

use super::kernel32;
use super::registry::{self, Open};
use crate::cpu::Cpu;

/// GenInstall's flags: which of the section's parts to carry out.
pub const DO_FILES: u16 = 0x01;
pub const DO_INI: u16 = 0x02;
pub const DO_REG: u16 = 0x04;
pub const DO_INI2REG: u16 = 0x08;
pub const DO_CFGAUTO: u16 = 0x10;
pub const DO_LOGCONFIG: u16 = 0x20;

/// The logical disk id of the source of the files.
pub const LDID_SRCPATH: u16 = 1;

/// CopyFiles' flags: copy whatever the version of the file in place
/// (COPYFLG_NOVERSIONCHECK), and copy only if there is no file there
/// (COPYFLG_NO_OVERWRITE).
const COPY_NO_VERSION_CHECK: u16 = 0x04;
const COPY_NO_OVERWRITE: u16 = 0x10;
/// Copy only if there is a file there (COPYFLG_REPLACEONLY).
const COPY_REPLACE_ONLY: u16 = 0x400;

/// AddReg's flags (FLG_ADDREG_*): the value's type (BINVALUETYPE and
/// the type in the high word), don't replace a value that is there
/// (NOCLOBBER), delete the value (DELVAL), only make the key (KEYONLY),
/// only replace a value that is there (OVERWRITEONLY).
const ADDREG_BINARY: u32 = 0x0000_0001;
const ADDREG_NOCLOBBER: u32 = 0x0000_0002;
const ADDREG_DELVAL: u32 = 0x0000_0004;
const ADDREG_KEYONLY: u32 = 0x0000_0010;
const ADDREG_OVERWRITEONLY: u32 = 0x0000_0020;
const ADDREG_TYPE_MASK: u32 = 0xFFFF_0001;
const ADDREG_TYPE_MULTI_SZ: u32 = 0x0001_0000;
const ADDREG_TYPE_EXPAND_SZ: u32 = 0x0002_0000;
const ADDREG_TYPE_DWORD: u32 = 0x0001_0001;
const ADDREG_TYPE_NONE: u32 = 0x0002_0001;

/// UpdateInis' flags: match the old entry's value as well as its key,
/// and put the new entry in only when the old one was there (the flags
/// 2 and 3 of Microsoft's INF documentation).
const INI_MATCH_VALUE: u32 = 0x01;
const INI_ONLY_IF_FOUND: u32 = 0x02;

/// A line of a section: its key, if it has one, and its fields, the
/// quotes taken off.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Line {
    pub key: Option<String>,
    pub fields: Vec<String>,
}

#[derive(Clone, Debug, Default)]
struct Section {
    name: String,
    lines: Vec<Line>,
}

/// An INF file's sections, in their order.
#[derive(Clone, Debug, Default)]
pub struct Inf {
    sections: Vec<Section>,
}

/// Why an install stopped: the section isn't in the file, or a file
/// couldn't be read or written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    NoSection,
    Files,
}

/// A queued file operation (`kind` one of `COPY`, `DELETE`, `RENAME`):
/// the full path of the file it makes, deletes or renames, the source
/// (the file copied, or the old name renamed), and CopyFiles' flags.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileOp {
    pub kind: u8,
    pub dest: String,
    pub source: String,
    pub flags: u16,
}

pub const COPY: u8 = 0;
pub const DELETE: u8 = 1;
pub const RENAME: u8 = 2;

crate::state_fields!(FileOp {
    kind,
    dest,
    source,
    flags
});

/// The text of a line, the comment that ends it taken off: what is
/// before a ';' that isn't in quotes. Also whether it ends with a '\'
/// outside quotes, which joins the next line to it, taken off too.
fn strip_comment(line: &[u8]) -> (&[u8], bool) {
    let mut quoted = false;
    let mut end = line.len();
    for (i, &c) in line.iter().enumerate() {
        match c {
            b'"' => quoted = !quoted,
            b';' if !quoted => {
                end = i;
                break;
            }
            _ => {}
        }
    }
    let text = trim(&line[..end]);
    match text.split_last() {
        Some((b'\\', rest)) if !quoted => (rest, true),
        _ => (text, false),
    }
}

fn trim(text: &[u8]) -> &[u8] {
    let start = text.iter().position(|c| !matches!(c, b' ' | b'\t'));
    let Some(start) = start else {
        return &[];
    };
    let end = text
        .iter()
        .rposition(|c| !matches!(c, b' ' | b'\t'))
        .unwrap_or(start);
    &text[start..=end]
}

/// A field's text: the spaces around it gone, and the quotes in it taken
/// off ("" in quotes is a quote).
fn field(text: &[u8]) -> String {
    let text = trim(text);
    let mut out = Vec::with_capacity(text.len());
    let mut quoted = false;
    let mut i = 0;
    while i < text.len() {
        let c = text[i];
        if c == b'"' {
            if quoted && text.get(i + 1) == Some(&b'"') {
                out.push(b'"');
                i += 2;
                continue;
            }
            quoted = !quoted;
        } else {
            out.push(c);
        }
        i += 1;
    }
    crate::dosstr::from_bytes(&out)
}

/// A line's key and fields. The key is what comes before an '=' that is
/// before the first comma and outside quotes.
fn parse_line(text: &[u8]) -> Line {
    let mut parts = Vec::new();
    let mut key = None;
    let mut quoted = false;
    let mut start = 0;
    for (i, &c) in text.iter().enumerate() {
        match c {
            b'"' => quoted = !quoted,
            b'=' if !quoted && key.is_none() && parts.is_empty() => {
                key = Some(field(&text[..i]));
                start = i + 1;
            }
            b',' if !quoted => {
                parts.push(field(&text[start..i]));
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(field(&text[start..]));
    Line { key, fields: parts }
}

impl Inf {
    /// The sections of the file's text. A line ends at "\r\n", '\n' or
    /// '\r', and the file at Ctrl+Z.
    pub fn parse(text: &[u8]) -> Inf {
        let text = text.split(|&c| c == 0x1A).next().unwrap_or(&[]);
        let mut inf = Inf::default();
        let mut current: Option<usize> = None;
        let mut pending: Vec<u8> = Vec::new();
        for raw in text.split(|&c| c == b'\n') {
            let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
            let (line, more) = strip_comment(raw);
            pending.extend_from_slice(line);
            if more {
                continue;
            }
            let line = std::mem::take(&mut pending);
            if line.is_empty() {
                continue;
            }
            if line[0] == b'[' {
                let end = line.iter().position(|&c| c == b']').unwrap_or(line.len());
                let name = crate::dosstr::from_bytes(trim(&line[1..end]));
                current = Some(match inf.find(&name) {
                    Some(i) => i,
                    None => {
                        inf.sections.push(Section {
                            name,
                            lines: Vec::new(),
                        });
                        inf.sections.len() - 1
                    }
                });
                continue;
            }
            // Lines before the first section belong to none.
            if let Some(i) = current {
                inf.sections[i].lines.push(parse_line(&line));
            }
        }
        inf
    }

    fn find(&self, name: &str) -> Option<usize> {
        self.sections
            .iter()
            .position(|s| s.name.eq_ignore_ascii_case(name))
    }

    /// Whether the file has the section `name`.
    pub fn has_section(&self, name: &str) -> bool {
        self.find(name).is_some()
    }

    /// The lines of the section `name`, none if it isn't there.
    pub fn lines(&self, name: &str) -> &[Line] {
        self.find(name).map_or(&[], |i| &self.sections[i].lines)
    }

    /// The first line of the section `section` whose key is `key`.
    pub fn line(&self, section: &str, key: &str) -> Option<&Line> {
        self.lines(section).iter().find(|l| {
            l.key
                .as_deref()
                .is_some_and(|k| k.eq_ignore_ascii_case(key))
        })
    }

    /// The string `name` of [Strings].
    fn string(&self, name: &str) -> Option<&str> {
        let line = self.line("Strings", name)?;
        Some(line.fields.first().map_or("", String::as_str))
    }

    /// `text` with its %name% strings and %number% directories put in,
    /// `dirs` giving the directory of an LDID. One that is neither stays
    /// as it is. A directory that ends with '\' (a drive's root, "C:\")
    /// loses it before a '\' in the text, so "%30%\GAMES" is "C:\GAMES".
    pub fn expand(&self, text: &str, dirs: &dyn Fn(u16) -> Option<String>) -> String {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(start) = rest.find('%') {
            out.push_str(&rest[..start]);
            let after = &rest[start + 1..];
            let Some(end) = after.find('%') else {
                out.push_str(&rest[start..]);
                return out;
            };
            let name = &after[..end];
            let value = if name.is_empty() {
                Some("%".to_string())
            } else if let Some(s) = self.string(name) {
                Some(s.to_string())
            } else {
                let slash_after = after[end + 1..].starts_with('\\');
                name.parse::<u16>().ok().and_then(dirs).map(|mut dir| {
                    if slash_after && dir.ends_with('\\') {
                        dir.pop();
                    }
                    dir
                })
            };
            match value {
                Some(v) => out.push_str(&v),
                None => out.push_str(&rest[start..start + end + 2]),
            }
            rest = &after[end + 1..];
        }
        out.push_str(rest);
        out
    }
}

/// The directory of the LDID `ldid` that Windows 98 has without a setup
/// program setting it: the Windows directory's and its subdirectories',
/// and the boot drive's root.
pub fn standard_dir(ldid: u16) -> Option<String> {
    let win = super::loader::WINDOWS_DIR;
    let sys = super::loader::SYSTEM_DIR;
    Some(match ldid {
        10 | 14 | 16 | 19 | 25 | 26 => win.to_string(),
        11 | 15 => sys.to_string(),
        12 => format!("{sys}\\IOSUBSYS"),
        13 => format!("{win}\\COMMAND"),
        17 => format!("{win}\\INF"),
        18 => format!("{win}\\HELP"),
        20 => format!("{win}\\FONTS"),
        21 => format!("{sys}\\VIEWERS"),
        22 => format!("{sys}\\VMM32"),
        23 => format!("{sys}\\COLOR"),
        24 | 30 | 31 => "C:\\".to_string(),
        _ => return None,
    })
}

/// `name` in the directory `dir`.
fn join(dir: &str, name: &str) -> String {
    format!("{}\\{}", dir.trim_end_matches('\\'), name)
}

/// A number in an INF field: decimal, or hexadecimal after 0x; 0 for
/// none.
fn number(text: &str) -> u32 {
    let text = text.trim();
    match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16).unwrap_or(0),
        None => text.parse().unwrap_or(0),
    }
}

/// What an install of a section works with: the file, the directories of
/// the LDIDs, and the log.
pub struct Install<'a> {
    pub inf: &'a Inf,
    pub dirs: &'a dyn Fn(u16) -> Option<String>,
}

impl Install<'_> {
    fn expand(&self, text: &str) -> String {
        self.inf.expand(text, self.dirs)
    }

    /// The field `i` of `line`, expanded; "" if there isn't one.
    fn field(&self, line: &Line, i: usize) -> String {
        line.fields.get(i).map_or(String::new(), |f| self.expand(f))
    }

    /// The names a line of the install section lists, as `CopyFiles =
    /// a, b` does.
    fn names(&self, section: &str, key: &str) -> Vec<String> {
        self.inf
            .lines(section)
            .iter()
            .filter(|l| {
                l.key
                    .as_deref()
                    .is_some_and(|k| k.eq_ignore_ascii_case(key))
            })
            .flat_map(|l| l.fields.iter())
            .map(|f| self.expand(f))
            .filter(|f| !f.is_empty())
            .collect()
    }

    /// The directory [DestinationDirs] gives the section of files
    /// `section`: its LDID and a subdirectory, else DefaultDestDir's,
    /// else the system directory, as SETUPAPI has it. None, written to
    /// the log, when the LDID has no directory: the section's files are
    /// then left out.
    fn dest_dir(&self, cpu: &mut Cpu, section: &str) -> Option<String> {
        let line = self
            .inf
            .line("DestinationDirs", section)
            .or_else(|| self.inf.line("DestinationDirs", "DefaultDestDir"));
        let (ldid, sub) = match line {
            Some(l) => (number(&self.field(l, 0)) as u16, self.field(l, 1)),
            None => (11, String::new()),
        };
        let Some(dir) = (self.dirs)(ldid) else {
            cpu.bus.log_string(&format!(
                "[INF] [{section}] left out: LDID {ldid} has no directory"
            ));
            return None;
        };
        Some(if sub.is_empty() {
            dir
        } else {
            join(&dir, &sub)
        })
    }

    /// Carry out the parts `parts` (`DO_` flags) of the install section
    /// `section`: the file operations go into `files`, for `commit`.
    pub fn run(
        &self,
        cpu: &mut Cpu,
        section: &str,
        parts: u16,
        files: &mut Vec<FileOp>,
    ) -> Result<(), Error> {
        if !self.inf.has_section(section) {
            return Err(Error::NoSection);
        }
        let mut skipped = Vec::new();
        for line in self.inf.lines(section) {
            let Some(key) = line.key.as_deref() else {
                continue;
            };
            let part = match key.to_ascii_lowercase().as_str() {
                "copyfiles" | "delfiles" | "renfiles" => DO_FILES,
                "addreg" | "delreg" => DO_REG,
                "updateinis" | "updateinifields" => DO_INI,
                "ini2reg" => DO_INI2REG,
                "updatecfgsys" | "updateautobat" => DO_CFGAUTO,
                "logconfig" => DO_LOGCONFIG,
                // ADVPACK's own (RegisterOCXs, RunPreSetupCommands and
                // the others), and the ones that say what the section
                // needs rather than does.
                _ => 0,
            };
            if part & parts == 0 {
                continue;
            }
            match key.to_ascii_lowercase().as_str() {
                "copyfiles" | "delfiles" | "renfiles" | "addreg" | "delreg" | "updateinis" => {}
                _ => skipped.push(key.to_string()),
            }
        }
        if parts & DO_FILES != 0 {
            self.queue_files(cpu, section, files);
        }
        if parts & DO_REG != 0 {
            for name in self.names(section, "DelReg") {
                self.del_reg(cpu, &name);
            }
            for name in self.names(section, "AddReg") {
                self.add_reg(cpu, &name);
            }
        }
        if parts & DO_INI != 0 {
            for name in self.names(section, "UpdateInis") {
                self.update_inis(cpu, &name);
            }
        }
        for key in skipped {
            cpu.bus.log_string(&format!(
                "[INF] [{section}] {key}= isn't carried out by the built-in Windows"
            ));
        }
        Ok(())
    }

    /// Queue the CopyFiles, RenFiles and DelFiles of `section`.
    fn queue_files(&self, cpu: &mut Cpu, section: &str, files: &mut Vec<FileOp>) {
        let source_dir = (self.dirs)(LDID_SRCPATH).unwrap_or_default();
        for name in self.names(section, "CopyFiles") {
            if let Some(file) = name.strip_prefix('@') {
                if let Some(dir) = self.dest_dir(cpu, section) {
                    files.push(FileOp {
                        kind: COPY,
                        dest: join(&dir, file),
                        source: join(&source_dir, file),
                        flags: 0,
                    });
                }
                continue;
            }
            let Some(dir) = self.dest_dir(cpu, &name) else {
                continue;
            };
            for line in self.inf.lines(&name) {
                let dest = self.field(line, 0);
                if dest.is_empty() {
                    continue;
                }
                let source = Some(self.field(line, 1)).filter(|s| !s.is_empty());
                files.push(FileOp {
                    kind: COPY,
                    source: join(&source_dir, source.as_deref().unwrap_or(&dest)),
                    dest: join(&dir, &dest),
                    flags: number(&self.field(line, 3)) as u16,
                });
            }
        }
        for name in self.names(section, "RenFiles") {
            let Some(dir) = self.dest_dir(cpu, &name) else {
                continue;
            };
            for line in self.inf.lines(&name) {
                let (new, old) = (self.field(line, 0), self.field(line, 1));
                if !new.is_empty() && !old.is_empty() {
                    files.push(FileOp {
                        kind: RENAME,
                        dest: join(&dir, &new),
                        source: join(&dir, &old),
                        flags: 0,
                    });
                }
            }
        }
        for name in self.names(section, "DelFiles") {
            let Some(dir) = self.dest_dir(cpu, &name) else {
                continue;
            };
            for line in self.inf.lines(&name) {
                let file = self.field(line, 0);
                if !file.is_empty() {
                    files.push(FileOp {
                        kind: DELETE,
                        dest: join(&dir, &file),
                        source: String::new(),
                        flags: 0,
                    });
                }
            }
        }
    }

    /// The predefined key a line of AddReg or DelReg starts with.
    fn root(&self, cpu: &mut Cpu, line: &Line, section: &str) -> Option<u32> {
        let root = self.field(line, 0);
        let hkey = match root.to_ascii_uppercase().as_str() {
            "HKCR" => registry::HKEY_CLASSES_ROOT,
            "HKCU" => registry::HKEY_CURRENT_USER,
            "HKLM" => registry::HKEY_LOCAL_MACHINE,
            "HKU" => registry::HKEY_USERS,
            _ => {
                cpu.bus.log_string(&format!(
                    "[INF] [{section}]: the registry root {root} isn't there"
                ));
                return None;
            }
        };
        match registry::get(cpu).open(hkey, 0) {
            Open::Key(key) => Some(key),
            Open::Error(_) => None,
        }
    }

    /// Write the values of the AddReg section `section`.
    fn add_reg(&self, cpu: &mut Cpu, section: &str) {
        for line in self.inf.lines(section) {
            let Some(root) = self.root(cpu, line, section) else {
                continue;
            };
            let path = self.field(line, 1);
            let name = self.field(line, 2);
            let flags = number(&self.field(line, 3));
            let reg = registry::get(cpu);
            if flags & ADDREG_DELVAL != 0 {
                if let Some(key) = reg.find(root, &path) {
                    reg.delete_value(key, &name);
                }
                continue;
            }
            if flags & ADDREG_OVERWRITEONLY != 0
                && reg
                    .find(root, &path)
                    .is_none_or(|k| reg.value(k, &name).is_none())
            {
                continue;
            }
            let (key, _) = reg.create(root, &path);
            // A line with no value name makes the key only.
            if flags & ADDREG_KEYONLY != 0 || line.fields.len() < 3 {
                continue;
            }
            if flags & ADDREG_NOCLOBBER != 0 && reg.value(key, &name).is_some() {
                continue;
            }
            let values: Vec<String> = (4..line.fields.len())
                .map(|i| self.field(line, i))
                .collect();
            let (kind, data) = reg_data(flags, &values);
            registry::get(cpu).set_value(key, &name, kind, data);
            let root = self.field(line, 0);
            cpu.bus
                .log_string(&format!("[INF] Set {root}\\{path} \"{name}\""));
        }
    }

    /// Delete the keys and values of the DelReg section `section`: a line
    /// with a value name deletes the value, one without the key and the
    /// keys under it.
    fn del_reg(&self, cpu: &mut Cpu, section: &str) {
        for line in self.inf.lines(section) {
            let Some(root) = self.root(cpu, line, section) else {
                continue;
            };
            let path = self.field(line, 1);
            let reg = registry::get(cpu);
            let Some(key) = reg.find(root, &path) else {
                continue;
            };
            if line.fields.len() >= 3 {
                let name = self.field(line, 2);
                if reg.delete_value(key, &name) {
                    let root = self.field(line, 0);
                    cpu.bus
                        .log_string(&format!("[INF] Deleted {root}\\{path} \"{name}\""));
                }
            } else if !reg.is_root(key) {
                reg.delete(key);
                let root = self.field(line, 0);
                cpu.bus.log_string(&format!("[INF] Deleted {root}\\{path}"));
            }
        }
    }

    /// Change the INI files as the UpdateInis section `section` says. A
    /// line is the file (in the Windows directory if it names no
    /// directory), the section, the entry to take out ("key=value" or
    /// "key", '*' matching anything), the entry to put in, in the old
    /// one's place, and flags.
    fn update_inis(&self, cpu: &mut Cpu, section: &str) {
        for line in self.inf.lines(section) {
            let file = self.field(line, 0);
            let ini_section = self.field(line, 1);
            let (old, new) = (self.field(line, 2), self.field(line, 3));
            let flags = number(&self.field(line, 4));
            if file.is_empty() || ini_section.is_empty() {
                continue;
            }
            cpu.bus.log_string(&format!(
                "[INF] {file} [{ini_section}]: out \"{old}\", in \"{new}\""
            ));
            let file = crate::dosstr::to_bytes(&file);
            let ini_section = crate::dosstr::to_bytes(&ini_section);
            let old = (!old.is_empty()).then(|| split_entry(&old));
            let new = (!new.is_empty()).then(|| split_entry(&new));
            if old.is_none() && new.is_none() {
                continue;
            }
            kernel32::update_profile(cpu, &file, &ini_section, |entries| {
                let matches = |(key, value): &(String, Option<String>),
                               (k, v): &(String, Option<String>)| {
                    wildcard(key, k)
                        && (flags & INI_MATCH_VALUE == 0
                            || value
                                .as_ref()
                                .is_none_or(|value| wildcard(value, v.as_deref().unwrap_or(""))))
                };
                // The new entry takes the place of the first old one.
                let first = old
                    .as_ref()
                    .and_then(|old| entries.iter().position(|e| matches(old, e)));
                if let Some(old) = &old {
                    entries.retain(|e| !matches(old, e));
                }
                let Some((key, value)) = new else {
                    return;
                };
                if old.is_some() && first.is_none() && flags & INI_ONLY_IF_FOUND != 0 {
                    return;
                }
                match first {
                    Some(at) => entries.insert(at, (key, value)),
                    None => match entries
                        .iter_mut()
                        .find(|(k, _)| k.eq_ignore_ascii_case(&key))
                    {
                        Some(entry) => entry.1 = value,
                        None => entries.push((key, value)),
                    },
                }
            });
        }
    }
}

/// An INI entry of UpdateInis: "key=value", or a key alone.
fn split_entry(entry: &str) -> (String, Option<String>) {
    match entry.split_once('=') {
        Some((k, v)) => (k.trim().to_string(), Some(v.trim().to_string())),
        None => (entry.trim().to_string(), None),
    }
}

/// Whether `text` matches `pattern`, in which '*' matches any run of
/// characters, ignoring case.
fn wildcard(pattern: &str, text: &str) -> bool {
    let (p, t) = (pattern.to_ascii_lowercase(), text.to_ascii_lowercase());
    match p.split_once('*') {
        None => p == t,
        Some((head, tail)) => {
            t.starts_with(head)
                && (0..=t.len() - head.len()).any(|i| {
                    t.is_char_boundary(head.len() + i) && wildcard(tail, &t[head.len() + i..])
                })
        }
    }
}

/// The type and data of an AddReg value, from its flags and its fields.
fn reg_data(flags: u32, values: &[String]) -> (u32, Vec<u8>) {
    let first = values.first().map_or("", String::as_str);
    let text = |s: &str| {
        let mut data = crate::dosstr::to_bytes(s);
        data.push(0);
        data
    };
    match flags & ADDREG_TYPE_MASK {
        // A DWORD is a number in one field, or its bytes in hexadecimal
        // in several ("01,00,00,00"), as SETUPAPI reads them.
        ADDREG_TYPE_DWORD if values.len() <= 1 => {
            (registry::REG_DWORD, number(first).to_le_bytes().to_vec())
        }
        ADDREG_TYPE_DWORD => (registry::REG_DWORD, hex_bytes(values)),
        ADDREG_TYPE_EXPAND_SZ => (registry::REG_EXPAND_SZ, text(first)),
        ADDREG_TYPE_MULTI_SZ => {
            let mut data: Vec<u8> = values.iter().flat_map(|v| text(v)).collect();
            data.push(0);
            (REG_MULTI_SZ, data)
        }
        ADDREG_TYPE_NONE => (REG_NONE, hex_bytes(values)),
        t if t & ADDREG_BINARY != 0 => {
            // The type in the high word, or REG_BINARY.
            let kind = match t >> 16 {
                0 => registry::REG_BINARY,
                k => k,
            };
            (kind, hex_bytes(values))
        }
        _ => (registry::REG_SZ, text(first)),
    }
}

const REG_NONE: u32 = 0;
const REG_MULTI_SZ: u32 = 7;

/// The bytes of binary value fields, each a hexadecimal byte.
fn hex_bytes(values: &[String]) -> Vec<u8> {
    values
        .iter()
        .filter_map(|v| u8::from_str_radix(v.trim(), 16).ok())
        .collect()
}

/// Carry out the queued file operations `ops`: deletions, then renames,
/// then copies, as SETUPX's queue does. Each is logged. Err if one of
/// them failed; the others are still done.
pub fn commit(cpu: &mut Cpu, ops: &[FileOp]) -> Result<(), Error> {
    let mut ok = true;
    for kind in [DELETE, RENAME, COPY] {
        for op in ops.iter().filter(|op| op.kind == kind) {
            ok &= match kind {
                DELETE => delete(cpu, op),
                RENAME => rename(cpu, op),
                _ => copy(cpu, op),
            };
        }
    }
    if ok { Ok(()) } else { Err(Error::Files) }
}

fn delete(cpu: &mut Cpu, op: &FileOp) -> bool {
    if kernel32::files::attributes(cpu, &op.dest).is_err() {
        return true;
    }
    let result = kernel32::files::delete_file(cpu, &op.dest);
    cpu.bus.log_string(&match result {
        Ok(()) => format!("[INF] Deleted {}", op.dest),
        Err(e) => format!("[INF] {} can't be deleted (error {e})", op.dest),
    });
    result.is_ok()
}

fn rename(cpu: &mut Cpu, op: &FileOp) -> bool {
    let result = kernel32::files::move_file(cpu, &op.source, &op.dest);
    cpu.bus.log_string(&match result {
        Ok(()) => format!("[INF] Renamed {} to {}", op.source, op.dest),
        Err(e) => format!(
            "[INF] {} can't be renamed to {} (error {e})",
            op.source, op.dest
        ),
    });
    result.is_ok()
}

/// Make the directory `dir` and those above it that aren't there.
fn make_dirs(cpu: &mut Cpu, dir: &str) {
    let parts: Vec<&str> = dir.split('\\').collect();
    for end in 2..=parts.len() {
        let path = parts[..end].join("\\");
        if kernel32::files::attributes(cpu, &path).is_err() {
            let _ = kernel32::files::create_directory(cpu, &path);
        }
    }
}

/// Copy a queued file, as its flags and the versions say.
fn copy(cpu: &mut Cpu, op: &FileOp) -> bool {
    let Ok((bytes, stamp, _)) = kernel32::read_path(cpu, &op.source) else {
        cpu.bus
            .log_string(&format!("[INF] {} isn't there to copy", op.source));
        return false;
    };
    // SETUPX expands what COMPRESS.EXE packed.
    let bytes = match kernel32::lz::header(&bytes) {
        Some((b'A', _, _)) => kernel32::lz::expand(&bytes[14..]),
        _ => bytes,
    };
    let exists = kernel32::files::attributes(cpu, &op.dest).is_ok();
    if exists && op.flags & COPY_NO_OVERWRITE != 0 {
        cpu.bus.log_string(&format!(
            "[INF] Kept {}: the flags don't replace it",
            op.dest
        ));
        return true;
    }
    if !exists && op.flags & COPY_REPLACE_ONLY != 0 {
        cpu.bus.log_string(&format!(
            "[INF] Left out {}: the flags copy it only over a file",
            op.dest
        ));
        return true;
    }
    if exists && op.flags & COPY_NO_VERSION_CHECK == 0 {
        let old = super::verdll::file_version(cpu, &op.dest);
        let new = super::verdll::bytes_version(&bytes);
        if let Some(old) = old
            && new.is_none_or(|new| new < old)
        {
            cpu.bus.log_string(&format!(
                "[INF] Kept {}: it is newer ({}) than {} ({})",
                op.dest,
                version_text(old),
                op.source,
                new.map_or("none".to_string(), version_text)
            ));
            return true;
        }
    }
    let dir = op.dest.rsplit_once('\\').map_or("", |(d, _)| d);
    make_dirs(cpu, dir);
    let full = kernel32::files::full_path(cpu, &op.dest);
    // A file a program has open gets the copy under another name, and
    // WININIT.INI puts it in place at the next start.
    let (target, deferred) = if kernel32::path_in_use(cpu, &op.dest) {
        let seed = cpu.bus.clock.icount as u32;
        let free = (0..0x1000u32)
            .map(|n| join(dir, &format!("INF{:04X}.TMP", (seed + n) & 0xFFFF)))
            .find(|p| kernel32::files::attributes(cpu, p).is_err());
        match free {
            Some(t) => (kernel32::files::full_path(cpu, &t), true),
            None => return false,
        }
    } else {
        (full.clone(), false)
    };
    let written = cpu
        .bus
        .disk
        .long_name_call(|disk| disk.write_whole_file(&target, &bytes, Some(stamp)));
    if let Err(e) = written {
        cpu.bus
            .log_string(&format!("[INF] {} can't be written (error {e})", op.dest));
        return false;
    }
    if deferred {
        let windows = crate::dosstr::to_bytes(super::boot::WININIT);
        let line = (full.clone(), Some(target.clone()));
        kernel32::update_profile(cpu, &windows, b"rename", |entries| entries.push(line));
        cpu.bus.log_string(&format!(
            "[INF] Copied {} to {target}, which replaces {} at the next start",
            op.source, op.dest
        ));
    } else {
        cpu.bus
            .log_string(&format!("[INF] Copied {} to {}", op.source, op.dest));
    }
    true
}

fn version_text(v: u64) -> String {
    format!(
        "{}.{}.{}.{}",
        v >> 48,
        (v >> 32) & 0xFFFF,
        (v >> 16) & 0xFFFF,
        v & 0xFFFF
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &[u8] = b"[Version]\r\n\
Signature=\"$Chicago$\" ; a comment\r\n\
\r\n\
[DefaultInstall]\r\n\
CopyFiles = Files.A, Files.B\r\n\
AddReg=Reg\r\n\
[Files.A]\r\n\
one.dll,,,97\r\n\
[Reg]\r\n\
HKLM,\"Software\\A, B\",\"Name\",,\"say \"\"hi\"\"; now\"\r\n\
HKLM,Key,Long,,first,\\\r\n\
  second\r\n\
[defaultinstall]\r\n\
DelFiles=Gone\r\n\
[Strings]\r\n\
Product=\"Media\"\r\n\
\x1A[Ignored]\r\n";

    #[test]
    fn lines_have_keys_fields_quotes_and_comments() {
        let inf = Inf::parse(TEXT);
        let sig = inf.line("version", "signature").unwrap();
        assert_eq!(sig.fields, ["$Chicago$"]);
        let reg = inf.lines("Reg");
        assert_eq!(reg[0].key, None);
        assert_eq!(
            reg[0].fields,
            ["HKLM", "Software\\A, B", "Name", "", "say \"hi\"; now"]
        );
        assert_eq!(
            reg[1].fields,
            ["HKLM", "Key", "Long", "", "first", "second"]
        );
        assert_eq!(inf.lines("Files.A")[0].fields, ["one.dll", "", "", "97"]);
        assert!(!inf.has_section("Ignored"));
    }

    #[test]
    fn sections_of_one_name_are_one_section() {
        let inf = Inf::parse(TEXT);
        let keys: Vec<_> = inf
            .lines("DefaultInstall")
            .iter()
            .map(|l| l.key.clone().unwrap())
            .collect();
        assert_eq!(keys, ["CopyFiles", "AddReg", "DelFiles"]);
    }

    #[test]
    fn strings_and_directories_are_put_in() {
        let inf = Inf::parse(TEXT);
        let dirs = |ldid| standard_dir(ldid);
        assert_eq!(
            inf.expand("%11%\\%product%.dll 100%% %none% %", &dirs),
            "C:\\WINDOWS\\SYSTEM\\Media.dll 100% %none% %"
        );
        assert_eq!(inf.expand("%30%\\GAMES %30%", &dirs), "C:\\GAMES C:\\");
    }

    #[test]
    fn registry_data_follows_the_flags() {
        let values = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            reg_data(0, &values(&["a"])),
            (registry::REG_SZ, b"a\0".to_vec())
        );
        assert_eq!(
            reg_data(1, &values(&["01", "ff"])),
            (registry::REG_BINARY, vec![1, 0xFF])
        );
        assert_eq!(
            reg_data(0x10001, &values(&["0x10"])),
            (registry::REG_DWORD, vec![0x10, 0, 0, 0])
        );
        assert_eq!(
            reg_data(0x10001, &values(&["00", "10", "00", "00"])),
            (registry::REG_DWORD, vec![0, 0x10, 0, 0])
        );
        assert_eq!(
            reg_data(0x10000, &values(&["a", "b"])),
            (REG_MULTI_SZ, b"a\0b\0\0".to_vec())
        );
    }

    #[test]
    fn wildcards_match_runs() {
        assert!(wildcard("msacm.*", "MSACM.MSAUDIO1"));
        assert!(wildcard("*", ""));
        assert!(!wildcard("a*c", "abd"));
        assert!(wildcard("a*c*e", "abcde"));
    }
}
