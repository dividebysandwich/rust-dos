//! Profiles: INI files read and written through DOS's files. WIN.INI and
//! the files named without a directory are in the Windows directory.
//! Each call reads the file again, and a change writes it again whole,
//! as "[section]" and "key=value" lines.

use super::text::{ansi_to_wide, read_string, read_wide_string, wide_to_ansi};
use super::{ERROR_INVALID_PARAMETER, ERROR_PATH_NOT_FOUND, FALSE, TRUE, set_error};
use crate::cpu::Cpu;

/// The Windows directory, where WIN.INI is.
pub(super) const WINDOWS_DIRECTORY: &str = "C:\\WINDOWS";

const FILE_SHARE_READ: u32 = 1;
const FILE_SHARE_WRITE: u32 = 2;
const ERROR_SHARING_VIOLATION: u32 = 32;

/// A line of a section: a key and its value, or a line without '=',
/// whose value is None.
struct Entry {
    key: Vec<u8>,
    value: Option<Vec<u8>>,
}

/// A section, or with no name the lines before the first one.
struct Section {
    name: Option<Vec<u8>>,
    entries: Vec<Entry>,
}

fn trim(text: &[u8]) -> &[u8] {
    text.trim_ascii()
}

/// The sections of the file's text. A line ends at "\r\n", '\n' or
/// '\r'.
fn parse(text: &[u8]) -> Vec<Section> {
    let mut sections = vec![Section {
        name: None,
        entries: Vec::new(),
    }];
    // The text ends at a zero, as Windows reads it as a string.
    let end = text.iter().position(|&b| b == 0).unwrap_or(text.len());
    for line in text[..end].split(|&b| b == b'\n' || b == b'\r') {
        let line = trim(line);
        if line.is_empty() {
            continue;
        }
        if line[0] == b'[' {
            let end = line.iter().rposition(|&b| b == b']').unwrap_or(line.len());
            let name = trim(&line[1..end.max(1)]).to_vec();
            sections.push(Section {
                name: Some(name),
                entries: Vec::new(),
            });
            continue;
        }
        let entry = match line.iter().position(|&b| b == b'=') {
            Some(at) => Entry {
                key: trim(&line[..at]).to_vec(),
                value: Some(trim(&line[at + 1..]).to_vec()),
            },
            None => Entry {
                key: line.to_vec(),
                value: None,
            },
        };
        sections.last_mut().expect("a section").entries.push(entry);
    }
    sections
}

/// The file's text as it is written back.
fn unparse(sections: &[Section]) -> Vec<u8> {
    let mut out = Vec::new();
    for section in sections {
        if let Some(name) = &section.name {
            out.push(b'[');
            out.extend(name);
            out.extend(b"]\r\n");
        }
        for entry in &section.entries {
            out.extend(&entry.key);
            if let Some(value) = &entry.value {
                out.push(b'=');
                out.extend(value);
            }
            out.extend(b"\r\n");
        }
    }
    out
}

fn same(a: &[u8], b: &[u8]) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// The section named `name`, its case ignored.
fn find<'a>(sections: &'a [Section], name: &[u8]) -> Option<&'a Section> {
    sections
        .iter()
        .find(|s| s.name.as_deref().is_some_and(|n| same(n, name)))
}

/// The key's value, the quotes around it taken off.
fn value_of<'a>(section: &'a Section, key: &[u8]) -> Option<&'a [u8]> {
    let value = section
        .entries
        .iter()
        .find(|e| same(&e.key, key))?
        .value
        .as_deref()?;
    let quoted = value.len() >= 2
        && (value[0] == b'"' || value[0] == b'\'')
        && value[value.len() - 1] == value[0];
    Some(if quoted {
        &value[1..value.len() - 1]
    } else {
        value
    })
}

/// The DOS path of the profile `name`: WIN.INI for NULL, and in the
/// Windows directory when the name has no directory.
fn path_of(name: Option<&[u8]>) -> String {
    let name = crate::dosstr::from_bytes(name.unwrap_or(b"WIN.INI"));
    if name.contains(['\\', '/', ':']) {
        name
    } else {
        format!("{}\\{}", WINDOWS_DIRECTORY, name)
    }
}

/// The text of the file at `path`, or None if it can't be read: also
/// when a program has it open without FILE_SHARE_READ.
fn read_file(cpu: &mut Cpu, path: &str) -> Option<Vec<u8>> {
    if super::path::open_sharing(cpu, path).is_some_and(|s| s & FILE_SHARE_READ == 0) {
        return None;
    }
    let owner = cpu.bus.win32.psp;
    let disk = &mut cpu.bus.disk;
    let handle = disk.open_file(path, 0, owner).ok()?;
    let mut text = Vec::new();
    loop {
        match disk.read_file(handle, 0x8000) {
            Ok(chunk) if !chunk.is_empty() => text.extend(chunk),
            _ => break,
        }
    }
    disk.close_file(handle);
    Some(text)
}

/// The sections of the profile `file` (NULL for WIN.INI; "" is none).
fn load(cpu: &mut Cpu, file: Option<&[u8]>) -> Vec<Section> {
    if file == Some(b"") {
        return Vec::new();
    }
    let path = path_of(file);
    parse(&read_file(cpu, &path).unwrap_or_default())
}

/// The guest string at `at`, or None for NULL; Err(()) if a page isn't
/// there.
fn optional(cpu: &mut Cpu, at: u32) -> Result<Option<Vec<u8>>, ()> {
    match at {
        0 => Ok(None),
        _ => read_string(cpu, at).map(Some).ok_or(()),
    }
}

/// Write `items`, each with its zero, into the buffer at `to` of `size`
/// bytes, with a zero after the last: the list GetPrivateProfileSection
/// and the key list return. Returns its length without the last zero;
/// when it doesn't fit, the last string is cut and the length is size - 2.
fn put_list(cpu: &mut Cpu, to: u32, size: u32, items: &[Vec<u8>]) -> Option<u32> {
    if size < 3 {
        return put_zeros(cpu, to, size);
    }
    let (out, len) = list(size as usize, items);
    if !cpu.bus.guest_probe(to, out.len(), true) {
        return None;
    }
    cpu.bus.guest_write_bytes(to, &out);
    Some(len)
}

/// `put_list` of UTF-16 strings, into a buffer of `size` characters.
fn put_wide_list(cpu: &mut Cpu, to: u32, size: u32, items: &[Vec<u16>]) -> Option<u32> {
    if size < 3 {
        return put_zeros(cpu, to, size.saturating_mul(2));
    }
    let (out, len) = list(size as usize, items);
    let out: Vec<u8> = out.iter().flat_map(|c| c.to_le_bytes()).collect();
    if !cpu.bus.guest_probe(to, out.len(), true) {
        return None;
    }
    cpu.bus.guest_write_bytes(to, &out);
    Some(len)
}

/// The list `put_list` writes, in a buffer of `size` units (3 or more),
/// and the length it returns.
fn list<T: Copy + Default>(size: usize, items: &[Vec<T>]) -> (Vec<T>, u32) {
    let mut out = Vec::new();
    let mut room = size - 1;
    let mut full = false;
    for item in items {
        let take = item.len().min(room.saturating_sub(1));
        out.extend(&item[..take]);
        out.push(T::default());
        room -= take + 1;
        if room < 2 {
            full = true;
            break;
        }
    }
    out.push(T::default());
    let len = if full || room <= 1 {
        let n = out.len();
        if n >= 2 {
            out[n - 2] = T::default();
        }
        size as u32 - 2
    } else {
        out.len() as u32 - 1
    };
    (out, len)
}

/// A buffer too small for a list: zeros.
fn put_zeros(cpu: &mut Cpu, to: u32, size: u32) -> Option<u32> {
    if !cpu.bus.guest_probe(to, size as usize, true) {
        return None;
    }
    cpu.bus.guest_fill(to, size as usize, 0);
    Some(0)
}

/// The names of the sections into the buffer at `to` of `size` bytes, as
/// GetPrivateProfileSectionNames returns them.
fn put_section_names(cpu: &mut Cpu, sections: &[Section], to: u32, size: u32) -> Option<u32> {
    if size < 2 {
        return put_zeros(cpu, to, size);
    }
    let size = size as usize;
    let mut out = Vec::new();
    let mut room = size - 1;
    let mut len = None;
    for name in sections.iter().filter_map(|s| s.name.as_ref()) {
        if name.is_empty() {
            continue;
        }
        if name.len() + 1 >= room {
            if room > 0 {
                out.extend(&name[..room - 1]);
                out.push(0);
            }
            len = Some(size as u32 - 2);
            break;
        }
        out.extend(name);
        out.push(0);
        room -= name.len() + 1;
    }
    let len = len.unwrap_or(out.len() as u32);
    out.push(0);
    if !cpu.bus.guest_probe(to, out.len(), true) {
        return None;
    }
    cpu.bus.guest_write_bytes(to, &out);
    Some(len.min(size as u32))
}

/// The keys of `section` that have values, or with `values` its lines
/// as "key=value".
fn section_lines(section: &Section, values: bool) -> Vec<Vec<u8>> {
    let lines = section.entries.iter().filter(|e| !e.key.starts_with(b";"));
    if values {
        lines
            .map(|e| match &e.value {
                Some(v) => [&e.key[..], b"=", v].concat(),
                None => e.key.clone(),
            })
            .collect()
    } else {
        lines
            .filter(|e| e.value.is_some() && !e.key.is_empty())
            .map(|e| e.key.clone())
            .collect()
    }
}

/// What GetPrivateProfileStringA and GetProfileStringA do: the value of
/// `key` in `section` of `file`, or the default (its trailing spaces
/// cut), cut to fit the buffer; the section names for no section, the
/// section's keys for no key.
fn get_string(cpu: &mut Cpu, args: [u32; 5], file: Option<Vec<u8>>) -> Option<u32> {
    let [section, key, default, to, size] = args;
    if to == 0 || size == 0 {
        return Some(0);
    }
    let (Ok(section), Ok(key), Ok(default)) = (
        optional(cpu, section),
        optional(cpu, key),
        optional(cpu, default),
    ) else {
        return None;
    };
    let sections = load(cpu, file.as_deref());
    let Some(section) = section else {
        return put_section_names(cpu, &sections, to, size);
    };
    let found = find(&sections, &section);
    let Some(key) = key else {
        let lines = found.map_or(Vec::new(), |s| section_lines(s, false));
        return put_list(cpu, to, size, &lines);
    };
    let default = default.unwrap_or_default();
    let value = match found.and_then(|s| value_of(s, &key)) {
        Some(value) if !key.is_empty() => value.to_vec(),
        _ => default.trim_ascii_end().to_vec(),
    };
    let take = value.len().min(size as usize - 1);
    let mut out = value[..take].to_vec();
    out.push(0);
    if !cpu.bus.guest_probe(to, out.len(), true) {
        return None;
    }
    cpu.bus.guest_write_bytes(to, &out);
    Some(take as u32)
}

/// GetPrivateProfileStringA(section, key, default, buffer, size, file).
pub(super) fn get_private_profile_string_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Ok(file) = optional(cpu, args[5]) else {
        return None;
    };
    get_string(cpu, [args[0], args[1], args[2], args[3], args[4]], file)
}

/// GetProfileStringA(section, key, default, buffer, size): WIN.INI's.
pub(super) fn get_profile_string_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_string(cpu, [args[0], args[1], args[2], args[3], args[4]], None)
}

/// The number at the start of `text` as Windows 95 reads it: an
/// optional '-' and decimal digits, in 32 bits; 0 if there are none.
fn parse_int(text: &[u8]) -> u32 {
    let (negative, digits) = match text.split_first() {
        Some((b'-', rest)) => (true, rest),
        _ => (false, text),
    };
    let n = digits
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .fold(0u32, |n, &d| {
            n.wrapping_mul(10).wrapping_add((d - b'0') as u32)
        });
    if negative { n.wrapping_neg() } else { n }
}

/// What GetPrivateProfileIntA and GetProfileIntA do: the key's value as
/// a number, the default if there is no value, and 0 for no section or
/// key.
fn get_int(
    cpu: &mut Cpu,
    section: u32,
    key: u32,
    default: u32,
    file: Option<Vec<u8>>,
) -> Option<u32> {
    if section == 0 || key == 0 {
        return Some(0);
    }
    let section = read_string(cpu, section)?;
    let key = read_string(cpu, key)?;
    let sections = load(cpu, file.as_deref());
    match find(&sections, &section).and_then(|s| value_of(s, &key)) {
        Some(value) if !value.is_empty() && !key.is_empty() => Some(parse_int(value)),
        _ => Some(default),
    }
}

pub(super) fn get_private_profile_int_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Ok(file) = optional(cpu, args[3]) else {
        return None;
    };
    get_int(cpu, args[0], args[1], args[2], file)
}

pub(super) fn get_profile_int_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_int(cpu, args[0], args[1], args[2], None)
}

/// What GetPrivateProfileSectionA and GetProfileSectionA do: the lines of
/// the section, "key=value" each.
fn get_section(
    cpu: &mut Cpu,
    section: u32,
    to: u32,
    size: u32,
    file: Option<Vec<u8>>,
) -> Option<u32> {
    if section == 0 || to == 0 || size == 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    let section = read_string(cpu, section)?;
    let sections = load(cpu, file.as_deref());
    let lines = find(&sections, &section).map_or(Vec::new(), |s| section_lines(s, true));
    put_list(cpu, to, size, &lines)
}

pub(super) fn get_private_profile_section_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Ok(file) = optional(cpu, args[3]) else {
        return None;
    };
    get_section(cpu, args[0], args[1], args[2], file)
}

pub(super) fn get_profile_section_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_section(cpu, args[0], args[1], args[2], None)
}

/// GetPrivateProfileSectionW(section, buffer, size, file): as the A
/// function, in UTF-16, `size` in characters.
pub(super) fn get_private_profile_section_w(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[section, to, size, file] = args else {
        unreachable!()
    };
    if section == 0 || to == 0 || size == 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    let section = wide_to_ansi(&read_wide_string(cpu, section)?);
    let file = match file {
        0 => None,
        at => Some(wide_to_ansi(&read_wide_string(cpu, at)?)),
    };
    let sections = load(cpu, file.as_deref());
    let lines = find(&sections, &section).map_or(Vec::new(), |s| section_lines(s, true));
    let lines: Vec<Vec<u16>> = lines.iter().map(|line| ansi_to_wide(line)).collect();
    put_wide_list(cpu, to, size, &lines)
}

/// The value of `key` in `section` of the profile `file`, for the
/// system's own use: OpenDriver's aliases in SYSTEM.INI.
pub(in crate::win32) fn profile_value(
    cpu: &mut Cpu,
    file: &[u8],
    section: &[u8],
    key: &[u8],
) -> Option<Vec<u8>> {
    let sections = load(cpu, Some(file));
    find(&sections, section)
        .and_then(|s| value_of(s, key))
        .map(<[u8]>::to_vec)
}

/// GetPrivateProfileSectionNamesA(buffer, size, file).
pub(super) fn get_private_profile_section_names_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[to, size, file] = args else {
        unreachable!()
    };
    let Ok(file) = optional(cpu, file) else {
        return None;
    };
    if to == 0 {
        return Some(0);
    }
    let sections = load(cpu, file.as_deref());
    put_section_names(cpu, &sections, to, size)
}

/// Write the profile `file` back if `change` changed its sections:
/// FALSE with DOS's error if it can't be written, or with
/// ERROR_SHARING_VIOLATION if a program has it open without sharing it
/// for reading and writing.
fn store(
    cpu: &mut Cpu,
    file: Option<Vec<u8>>,
    change: impl FnOnce(&mut Vec<Section>) -> bool,
) -> Option<u32> {
    if file.as_deref() == Some(b"") {
        set_error(cpu, ERROR_PATH_NOT_FOUND);
        return Some(FALSE);
    }
    let path = path_of(file.as_deref());
    let both = FILE_SHARE_READ | FILE_SHARE_WRITE;
    if super::path::open_sharing(cpu, &path).is_some_and(|s| s & both != both) {
        set_error(cpu, ERROR_SHARING_VIOLATION);
        return Some(FALSE);
    }
    let mut sections = parse(&read_file(cpu, &path).unwrap_or_default());
    if !change(&mut sections) {
        return Some(TRUE);
    }
    match cpu
        .bus
        .disk
        .write_whole_file(&path, &unparse(&sections), None)
    {
        Ok(()) => Some(TRUE),
        Err(code) => {
            set_error(cpu, code as u32);
            Some(FALSE)
        }
    }
}

/// The place of the section `name`, made at the end if there is none.
fn section_index(sections: &mut Vec<Section>, name: &[u8]) -> usize {
    let found = sections
        .iter()
        .position(|s| s.name.as_deref().is_some_and(|n| same(n, name)));
    found.unwrap_or_else(|| {
        sections.push(Section {
            name: Some(name.to_vec()),
            entries: Vec::new(),
        });
        sections.len() - 1
    })
}

/// Set `key` of `section` to `value`, or remove it (no value) or the
/// section (no key). Whether that changed anything.
fn set_string(
    sections: &mut Vec<Section>,
    section: &[u8],
    key: Option<&[u8]>,
    value: Option<&[u8]>,
) -> bool {
    let found = sections
        .iter()
        .position(|s| s.name.as_deref().is_some_and(|n| same(n, section)));
    let Some(key) = key else {
        if let Some(i) = found {
            sections.remove(i);
        }
        return found.is_some();
    };
    let Some(value) = value else {
        let Some(s) = found.map(|i| &mut sections[i]) else {
            return false;
        };
        let before = s.entries.len();
        s.entries.retain(|e| !same(&e.key, key));
        return s.entries.len() != before;
    };
    let i = section_index(sections, section);
    let entries = &mut sections[i].entries;
    match entries.iter_mut().find(|e| same(&e.key, key)) {
        Some(e) if e.value.as_deref() == Some(value) => found.is_none(),
        Some(e) => {
            e.value = Some(value.to_vec());
            true
        }
        None => {
            entries.push(Entry {
                key: key.to_vec(),
                value: Some(value.to_vec()),
            });
            true
        }
    }
}

/// What WritePrivateProfileStringA and WriteProfileStringA do.
fn write_string(cpu: &mut Cpu, args: [u32; 3], file: Option<Vec<u8>>) -> Option<u32> {
    let [section, key, value] = args;
    let (Ok(section), Ok(key), Ok(value)) = (
        optional(cpu, section),
        optional(cpu, key),
        optional(cpu, value),
    ) else {
        return None;
    };
    let Some(section) = section else {
        // With nothing but the file, Windows writes its cached copy: there
        // is none.
        return Some((key.is_none() && value.is_none()) as u32);
    };
    store(cpu, file, |sections| {
        set_string(sections, &section, key.as_deref(), value.as_deref())
    })
}

pub(super) fn write_private_profile_string_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Ok(file) = optional(cpu, args[3]) else {
        return None;
    };
    write_string(cpu, [args[0], args[1], args[2]], file)
}

pub(super) fn write_profile_string_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    write_string(cpu, [args[0], args[1], args[2]], None)
}

/// What WritePrivateProfileSectionA and WriteProfileSectionA do: the
/// section's lines replaced by the strings at `lines` ("key=value", each
/// with its zero, a zero after the last), or with NULL the section
/// removed.
fn write_section(cpu: &mut Cpu, section: u32, lines: u32, file: Option<Vec<u8>>) -> Option<u32> {
    if section == 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    let section = read_string(cpu, section)?;
    let mut entries = Vec::new();
    if lines != 0 {
        let mut at = lines;
        loop {
            let line = read_string(cpu, at)?;
            if line.is_empty() {
                break;
            }
            at = at.wrapping_add(line.len() as u32 + 1);
            entries.extend(parse(&line).into_iter().flat_map(|s| s.entries));
        }
    }
    store(cpu, file, |sections| {
        if lines == 0 {
            return set_string(sections, &section, None, None);
        }
        let i = section_index(sections, &section);
        sections[i].entries = entries;
        true
    })
}

pub(super) fn write_private_profile_section_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Ok(file) = optional(cpu, args[2]) else {
        return None;
    };
    write_section(cpu, args[0], args[1], file)
}

pub(super) fn write_profile_section_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    write_section(cpu, args[0], args[1], None)
}
