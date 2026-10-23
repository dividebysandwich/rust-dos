//! VERSION.DLL: the version resources of files, and the functions setup
//! programs install files with (VerFindFileA, VerInstallFileA).
//!
//! A file's version resource (VS_VERSIONINFO) is read from its PE or NE
//! resources, expanded first when COMPRESS.EXE packed the file. 32-bit
//! files keep it in Unicode; Windows 9x's GetFileVersionInfoA gives it
//! in the ANSI layout 16-bit files have, which VerQueryValueA reads
//! directly, so the strings it returns point into the block. Blocks in
//! the Unicode layout, as a program's LoadResource gives them, are read
//! too: their strings are converted into memory of the process heap.
//! The built-in DLLs aren't files, so their version resources are made
//! from a table, with Windows 95's versions.
//!
//! The walk of the blocks follows Wine's version/info.c, and
//! VerFindFileA and VerInstallFileA its install.c (LGPL 2.1 or later):
//!
//! Copyright 1996 Marcus Meissner
//! Copyright 1997 David Cuthbert
//! Copyright 1999 Ulrich Weigand
//! Copyright 2004, 2005 Juan Lang

use super::stubs::{Api, Conv, Dll};
use super::{kernel32, process};
use crate::cpu::Cpu;

pub static DLL: Dll = Dll {
    name: "VERSION.DLL",
    base: 0xBFD0_0000,
    apis: APIS,
    vars: &[],
    init: None,
    vtables: &[],
};

const fn stdcall(
    name: &'static str,
    ordinal: u16,
    args: u8,
    handler: super::stubs::Handler,
) -> Api {
    Api {
        name,
        ordinal,
        args,
        conv: Conv::Stdcall,
        handler,
    }
}

/// The functions. Append only (see `stubs`). Windows 95 has the Unicode
/// ones as stubs that fail.
static APIS: &[Api] = &[
    stdcall("GetFileVersionInfoA", 1, 4, get_file_version_info),
    stdcall("GetFileVersionInfoSizeA", 2, 2, get_file_version_info_size),
    stdcall("GetFileVersionInfoSizeW", 3, 2, not_implemented),
    stdcall("GetFileVersionInfoW", 4, 4, not_implemented),
    stdcall("VerFindFileA", 5, 8, ver_find_file),
    stdcall("VerFindFileW", 6, 8, not_implemented),
    stdcall("VerInstallFileA", 7, 8, ver_install_file),
    stdcall("VerInstallFileW", 8, 8, not_implemented),
    stdcall("VerLanguageNameA", 9, 3, ver_language_name),
    stdcall("VerLanguageNameW", 10, 3, not_implemented),
    stdcall("VerQueryValueA", 11, 4, ver_query_value),
    stdcall("VerQueryValueW", 12, 4, not_implemented),
];

const TRUE: u32 = 1;
const FALSE: u32 = 0;

/// Error codes (GetLastError).
const ERROR_BAD_FORMAT: u32 = 11;
const ERROR_INVALID_DATA: u32 = 13;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_CALL_NOT_IMPLEMENTED: u32 = 120;
const ERROR_BAD_PATHNAME: u32 = 161;
const ERROR_RESOURCE_DATA_NOT_FOUND: u32 = 1812;

fn set_error(cpu: &mut Cpu, code: u32) {
    process(cpu).last_error = code;
}

fn not_implemented(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    set_error(cpu, ERROR_CALL_NOT_IMPLEMENTED);
    Some(FALSE)
}

/// The string at `at`, or None for a null pointer; Err(()) if a page
/// isn't there.
fn opt_string(cpu: &mut Cpu, at: u32) -> Result<Option<String>, ()> {
    if at == 0 {
        return Ok(None);
    }
    super::user32::string_at(cpu, at).map(Some).ok_or(())
}

/// A version resource as a file has it.
#[derive(Debug, PartialEq, Eq)]
pub enum Resource {
    /// A 32-bit file's, in Unicode.
    Unicode(Vec<u8>),
    /// A 16-bit file's, in ANSI.
    Ansi(Vec<u8>),
}

impl Resource {
    fn bytes(&self) -> &[u8] {
        match self {
            Resource::Unicode(b) | Resource::Ansi(b) => b,
        }
    }
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*b.get(at)?, *b.get(at + 1)?]))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// The version resource of the executable file `file`: Err with
/// ERROR_BAD_FORMAT for a file that isn't one, and
/// ERROR_RESOURCE_DATA_NOT_FOUND for one without it.
pub fn find_resource(file: &[u8]) -> Result<Resource, u32> {
    if file.get(..2) != Some(b"MZ") {
        return Err(ERROR_BAD_FORMAT);
    }
    let header = u32_at(file, 0x3C).ok_or(ERROR_BAD_FORMAT)? as usize;
    match file.get(header..header + 2) {
        Some(b"PE") => pe_resource(file, header)
            .map(Resource::Unicode)
            .ok_or(ERROR_RESOURCE_DATA_NOT_FOUND),
        Some(b"NE") => ne_resource(file, header)
            .map(Resource::Ansi)
            .ok_or(ERROR_RESOURCE_DATA_NOT_FOUND),
        _ => Err(ERROR_BAD_FORMAT),
    }
}

/// RT_VERSION, and the name of the version resource.
const RT_VERSION: u16 = 16;
const VS_VERSION_INFO: u16 = 1;

/// The version resource of a PE file whose header is at `pe`: of its
/// first language.
fn pe_resource(file: &[u8], pe: usize) -> Option<Vec<u8>> {
    let sections = u16_at(file, pe + 6)? as usize;
    let optional = u16_at(file, pe + 20)? as usize;
    let opt = pe + 24;
    // The resource directory is the third of the data directories.
    let dirs = match u16_at(file, opt)? {
        0x10B => opt + 96,
        0x20B => opt + 112,
        _ => return None,
    };
    let rva = u32_at(file, dirs + 16)?;
    if u32_at(file, dirs + 20)? == 0 {
        return None;
    }
    let table = opt + optional;
    // The file offset of an RVA, by the section it is in.
    let offset = |rva: u32| -> Option<usize> {
        (0..sections).find_map(|i| {
            let s = table + 40 * i;
            let (va, raw_size, raw) = (
                u32_at(file, s + 12)?,
                u32_at(file, s + 16)?,
                u32_at(file, s + 20)?,
            );
            (rva >= va && rva < va + raw_size).then(|| (rva - va + raw) as usize)
        })
    };
    let root = offset(rva)?;
    // An entry of the directory at `dir` (offset from the root): by id,
    // or the first one.
    let entry = |dir: usize, id: Option<u16>| -> Option<u32> {
        let named = u16_at(file, root + dir + 12)? as usize;
        let ids = u16_at(file, root + dir + 14)? as usize;
        let first = root + dir + 16;
        (0..named + ids).find_map(|i| {
            let at = first + 8 * i;
            let name = u32_at(file, at)?;
            let matches = match id {
                Some(id) => i >= named && name == id as u32,
                None => true,
            };
            matches.then(|| u32_at(file, at + 4)).flatten()
        })
    };
    let names = entry(0, Some(RT_VERSION))?;
    let languages = entry((names & 0x7FFF_FFFF) as usize, Some(VS_VERSION_INFO))?;
    let data = entry((languages & 0x7FFF_FFFF) as usize, None)?;
    if data & 0x8000_0000 != 0 {
        return None;
    }
    let data = root + data as usize;
    let (rva, size) = (u32_at(file, data)?, u32_at(file, data + 4)? as usize);
    let at = offset(rva)?;
    file.get(at..at + size).map(<[u8]>::to_vec)
}

/// The version resource of an NE file whose header is at `ne`.
fn ne_resource(file: &[u8], ne: usize) -> Option<Vec<u8>> {
    let table = ne + u16_at(file, ne + 0x24)? as usize;
    let end = ne + u16_at(file, ne + 0x26)? as usize;
    if end <= table {
        return None;
    }
    let shift = u16_at(file, table)? as u32;
    let mut at = table + 2;
    while at < end {
        let kind = u16_at(file, at)?;
        if kind == 0 {
            return None;
        }
        let count = u16_at(file, at + 2)? as usize;
        if kind == RT_VERSION | 0x8000 {
            return (0..count).find_map(|i| {
                let info = at + 8 + 12 * i;
                if u16_at(file, info + 6)? != VS_VERSION_INFO | 0x8000 {
                    return None;
                }
                let offset = (u16_at(file, info)? as u32) << shift;
                let length = (u16_at(file, info + 2)? as u32) << shift;
                let start = offset as usize;
                file.get(start..start + length as usize).map(<[u8]>::to_vec)
            });
        }
        at += 8 + 12 * count;
    }
    None
}

/// A node of a version resource: its key, value (and whether it is
/// text) and children.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Node {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
    pub text: bool,
    pub children: Vec<Node>,
}

fn align(n: usize) -> usize {
    (n + 3) & !3
}

/// The node of the Unicode layout at `at` in `b`: its header (length,
/// value length in characters for text, type), its key, the value and
/// the children, each 32-bit aligned from the node's start. None when
/// it doesn't fit.
fn parse_unicode(b: &[u8], at: usize, depth: u32) -> Option<Node> {
    let len = u16_at(b, at)? as usize;
    let value_len = u16_at(b, at + 2)? as usize;
    let text = u16_at(b, at + 4)? != 0;
    let end = (at + len).min(b.len());
    if len < 6 || depth > 8 {
        return None;
    }
    let mut key = Vec::new();
    let mut p = at + 6;
    while p + 1 < end {
        let c = u16_at(b, p)?;
        p += 2;
        if c == 0 {
            break;
        }
        key.extend(wide_to_ansi(&[c]));
    }
    p = at + align(p - at);
    let bytes = if text { 2 * value_len } else { value_len };
    let value_end = (p + bytes).min(end);
    let value = b.get(p..value_end.max(p)).unwrap_or_default();
    let value = if text && value_len != 0 {
        let chars: Vec<u16> = value
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| u16::from_le_bytes(c))
            .take_while(|&c| c != 0)
            .collect();
        let mut v = wide_to_ansi(&chars);
        v.push(0);
        v
    } else {
        value.to_vec()
    };
    let mut children = Vec::new();
    p = at + align(value_end.max(p) - at);
    while p + 6 <= end {
        let child_len = u16_at(b, p)? as usize;
        if child_len == 0 {
            break;
        }
        children.push(parse_unicode(b, p, depth + 1)?);
        p += align(child_len);
    }
    Some(Node {
        key,
        value,
        text,
        children,
    })
}

/// The node of the ANSI layout at `at`.
fn parse_ansi(b: &[u8], at: usize, depth: u32) -> Option<Node> {
    let len = u16_at(b, at)? as usize;
    let value_len = u16_at(b, at + 2)? as usize;
    let end = (at + len).min(b.len());
    if len < 4 || depth > 8 {
        return None;
    }
    let key_end = (at + 4..end).find(|&i| b[i] == 0).unwrap_or(end);
    let key = b[at + 4..key_end].to_vec();
    let p = at + align(key_end + 1 - at);
    let value_end = (p + value_len).min(end);
    let value = b.get(p..value_end.max(p)).unwrap_or_default().to_vec();
    let mut children = Vec::new();
    let mut p = at + align(value_end.max(p) - at);
    while p + 4 <= end {
        let child_len = u16_at(b, p)? as usize;
        if child_len == 0 {
            break;
        }
        children.push(parse_ansi(b, p, depth + 1)?);
        p += align(child_len);
    }
    Some(Node {
        key,
        value,
        text: false,
        children,
    })
}

/// The node in the ANSI layout, as Windows 9x's GetFileVersionInfoA
/// gives a 32-bit file's.
pub fn write_ansi(node: &Node) -> Vec<u8> {
    let mut b = vec![0u8; 4];
    b.extend_from_slice(&node.key);
    b.push(0);
    b.resize(align(b.len()), 0);
    b.extend_from_slice(&node.value);
    for child in &node.children {
        b.resize(align(b.len()), 0);
        b.extend(write_ansi(child));
    }
    let len = b.len() as u16;
    b[0..2].copy_from_slice(&len.to_le_bytes());
    b[2..4].copy_from_slice(&(node.value.len() as u16).to_le_bytes());
    b
}

/// The node in the Unicode layout 32-bit files have.
pub fn write_unicode(node: &Node) -> Vec<u8> {
    let mut b = vec![0u8; 6];
    for c in kernel32::ansi_to_wide(&node.key).into_iter().chain([0]) {
        b.extend_from_slice(&c.to_le_bytes());
    }
    b.resize(align(b.len()), 0);
    let value_len = if node.text {
        let text = node.value.strip_suffix(&[0]).unwrap_or(&node.value);
        let wide = kernel32::ansi_to_wide(text);
        for c in wide.iter().chain(&[0]) {
            b.extend_from_slice(&c.to_le_bytes());
        }
        wide.len() + 1
    } else {
        b.extend_from_slice(&node.value);
        node.value.len()
    };
    for child in &node.children {
        b.resize(align(b.len()), 0);
        b.extend(write_unicode(child));
    }
    let len = b.len() as u16;
    b[0..2].copy_from_slice(&len.to_le_bytes());
    b[2..4].copy_from_slice(&(value_len as u16).to_le_bytes());
    b[4..6].copy_from_slice(&(node.text as u16).to_le_bytes());
    b
}

/// Unicode text in ANSI, a character each, '?' for those it lacks.
fn wide_to_ansi(chars: &[u16]) -> Vec<u8> {
    let text = String::from_utf16_lossy(chars);
    crate::dosstr::to_bytes(&text)
}

/// The tree of a version resource.
pub fn parse(resource: &Resource) -> Option<Node> {
    match resource {
        Resource::Unicode(b) => parse_unicode(b, 0, 0),
        Resource::Ansi(b) => parse_ansi(b, 0, 0),
    }
}

/// VS_FIXEDFILEINFO's signature, and its size.
const VS_FFI_SIGNATURE: u32 = 0xFEEF_04BD;
const FIXED_SIZE: usize = 52;

/// The version resource a built-in DLL has: Windows 95's version of the
/// file, with its name.
fn builtin_resource(name: &str) -> Resource {
    let base = name.trim_end_matches(".DLL").trim_end_matches(".dll");
    // Windows 95's own files are 4.00.950. DirectX's are 6.0's, as the
    // registry says (Windows 98 and DirectX 6.1 differ: see p5-win98).
    let version: [u16; 4] = match base.to_ascii_uppercase().as_str() {
        "DDRAW" | "DSOUND" | "DINPUT" | "DPLAYX" => [4, 6, 0, 318],
        "COMCTL32" => [4, 70, 0, 1146],
        "OLE32" => [2, 10, 35, 35],
        "MSVCRT" => [6, 0, 8168, 0],
        "GLIDE2X" => [2, 46, 0, 0],
        _ => [4, 0, 0, 950],
    };
    let text = match version {
        [4, 0, 0, 950] => "4.00.950".to_string(),
        [a, b, c, d] => format!("{}.{:02}.{:02}.{:04}", a, b, c, d),
    };
    let ms = (version[0] as u32) << 16 | version[1] as u32;
    let ls = (version[2] as u32) << 16 | version[3] as u32;
    let mut fixed = Vec::with_capacity(FIXED_SIZE);
    // Signature, structure version, file and product versions, flags
    // mask, flags, VOS_DOS_WINDOWS32, VFT_DLL, no subtype, no date.
    let words = [
        VS_FFI_SIGNATURE,
        0x0001_0000,
        ms,
        ls,
        ms,
        ls,
        0x3F,
        0,
        0x0001_0004,
        2,
        0,
        0,
        0,
    ];
    for w in words {
        fixed.extend_from_slice(&w.to_le_bytes());
    }
    let string = |key: &str, value: &str| Node {
        key: key.as_bytes().to_vec(),
        value: [value.as_bytes(), &[0]].concat(),
        text: true,
        children: Vec::new(),
    };
    let file = name.to_ascii_uppercase();
    let strings = Node {
        key: b"040904E4".to_vec(),
        text: true,
        children: vec![
            string(
                "FileDescription",
                &format!("{} (built into rust-dos)", base),
            ),
            string("FileVersion", &text),
            string("InternalName", base),
            string("OriginalFilename", &file),
            string("ProductVersion", &text),
        ],
        ..Node::default()
    };
    let root = Node {
        key: b"VS_VERSION_INFO".to_vec(),
        value: fixed,
        text: false,
        children: vec![
            Node {
                key: b"StringFileInfo".to_vec(),
                text: true,
                children: vec![strings],
                ..Node::default()
            },
            Node {
                key: b"VarFileInfo".to_vec(),
                text: true,
                children: vec![Node {
                    key: b"Translation".to_vec(),
                    value: vec![0x09, 0x04, 0xE4, 0x04],
                    ..Node::default()
                }],
                ..Node::default()
            },
        ],
    };
    Resource::Unicode(write_unicode(&root))
}

/// The version resource of the file `name` names: a file on disk, found
/// where OpenFile looks, or a built-in DLL; or the error.
fn file_resource(cpu: &mut Cpu, name: &str) -> Result<Resource, u32> {
    match kernel32::search_file(cpu, name) {
        Some(path) => {
            let (bytes, _, _) = kernel32::read_path(cpu, &path)?;
            let bytes = match super::kernel32::lz::header(&bytes) {
                Some((b'A', _, _)) => super::kernel32::lz::expand(&bytes[14..]),
                _ => bytes,
            };
            find_resource(&bytes)
        }
        None => {
            let file = name.rsplit(['\\', '/']).next().unwrap_or(name);
            let dir = &name[..name.len() - file.len()];
            let dir = dir.trim_end_matches(['\\', '/']);
            let system = dir.is_empty()
                || dir.eq_ignore_ascii_case(super::loader::SYSTEM_DIR)
                || dir.eq_ignore_ascii_case(super::loader::WINDOWS_DIR);
            match super::stubs::find_dll(file) {
                Some(i) if system && !file.is_empty() => {
                    Ok(builtin_resource(super::stubs::DLLS[i].name))
                }
                _ => Err(not_found(cpu, name)),
            }
        }
    }
}

/// The error for a file that can't be found: its directory isn't there
/// either (ERROR_PATH_NOT_FOUND), or it is (ERROR_FILE_NOT_FOUND).
fn not_found(cpu: &mut Cpu, name: &str) -> u32 {
    let full = kernel32::full_path(cpu, name);
    let dir = match full.rfind('\\') {
        Some(i) if i <= 2 => &full[..3],
        Some(i) => &full[..i],
        None => &full,
    };
    if cpu.bus.disk.is_directory(&kernel32::dos_path(dir)) {
        2
    } else {
        3
    }
}

/// The resource of the file at `name`, for the two functions that read
/// it: None with the last error set.
fn resource_arg(cpu: &mut Cpu, name: u32) -> Option<Option<Resource>> {
    let Ok(name) = opt_string(cpu, name) else {
        return None;
    };
    let Some(name) = name else {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(None);
    };
    if name.is_empty() {
        set_error(cpu, ERROR_BAD_PATHNAME);
        return Some(None);
    }
    match file_resource(cpu, &name) {
        Ok(resource) => Some(Some(resource)),
        Err(code) => {
            set_error(cpu, code);
            Some(None)
        }
    }
}

/// GetFileVersionInfoSizeA(name, handle): the size of the file's version
/// resource, which GetFileVersionInfoA needs room for; 0 with the error.
/// The handle is 0.
fn get_file_version_info_size(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[name, handle] = args else {
        unreachable!()
    };
    if handle != 0 && !super::com::write_out(cpu, handle, 0) {
        return None;
    }
    let Some(resource) = resource_arg(cpu, name)? else {
        return Some(0);
    };
    Some(resource.bytes().len() as u32)
}

/// GetFileVersionInfoA(name, handle, size, data): the file's version
/// resource in the ANSI layout, cut to `size` bytes.
fn get_file_version_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[name, _, size, data] = args else {
        unreachable!()
    };
    if data == 0 {
        set_error(cpu, ERROR_INVALID_DATA);
        return Some(FALSE);
    }
    let Some(resource) = resource_arg(cpu, name)? else {
        return Some(FALSE);
    };
    let bytes = match &resource {
        Resource::Ansi(b) => b.clone(),
        Resource::Unicode(_) => match parse(&resource) {
            Some(node) => write_ansi(&node),
            None => resource.bytes().to_vec(),
        },
    };
    let n = bytes.len().min(size as usize);
    if !cpu.bus.guest_probe(data, n, true) {
        return None;
    }
    cpu.bus.guest_write_bytes(data, &bytes[..n]);
    Some(TRUE)
}

/// Where a node's value is in its block, and how long VerQueryValueA
/// says it is: the place of the value and its length field.
struct Found {
    value: usize,
    len: u32,
    /// For a text value of the Unicode layout: the text in ANSI.
    text: Option<Vec<u8>>,
}

/// The value at the path `path` of the block `b`, walked in its layout.
fn query(b: &[u8], path: &[u8], unicode: bool) -> Option<Found> {
    let mut at = 0usize;
    for part in path.split(|&c| c == b'\\').filter(|p| !p.is_empty()) {
        at = children(b, at, unicode)?
            .into_iter()
            .find(|&child| key_of(b, child, unicode).eq_ignore_ascii_case(part))?;
    }
    let (value, len, text) = value_of(b, at, unicode)?;
    let text = (text && len != 0).then(|| {
        let chars: Vec<u16> = b
            .get(value..)
            .unwrap_or_default()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| u16::from_le_bytes(c))
            .take(len as usize)
            .take_while(|&c| c != 0)
            .collect();
        let mut ansi = wide_to_ansi(&chars);
        ansi.push(0);
        ansi
    });
    let len = text.as_ref().map_or(len, |t| t.len() as u32);
    Some(Found { value, len, text })
}

/// The key of the node at `at`.
fn key_of(b: &[u8], at: usize, unicode: bool) -> Vec<u8> {
    if unicode {
        let chars: Vec<u16> = (0..)
            .map_while(|i| u16_at(b, at + 6 + 2 * i))
            .take_while(|&c| c != 0)
            .collect();
        wide_to_ansi(&chars)
    } else {
        b[(at + 4).min(b.len())..]
            .iter()
            .copied()
            .take_while(|&c| c != 0)
            .collect()
    }
}

/// The place of the value of the node at `at`, its length field, and
/// whether it is Unicode text.
fn value_of(b: &[u8], at: usize, unicode: bool) -> Option<(usize, u32, bool)> {
    let value_len = u16_at(b, at + 2)? as u32;
    if unicode {
        let text = u16_at(b, at + 4)? != 0;
        let mut p = at + 6;
        while u16_at(b, p)? != 0 {
            p += 2;
        }
        Some((at + align(p + 2 - at), value_len, text))
    } else {
        let mut p = at + 4;
        while *b.get(p)? != 0 {
            p += 1;
        }
        Some((at + align(p + 1 - at), value_len, false))
    }
}

/// The places of the children of the node at `at`.
fn children(b: &[u8], at: usize, unicode: bool) -> Option<Vec<usize>> {
    let len = u16_at(b, at)? as usize;
    let end = (at + len).min(b.len());
    let (value, value_len, text) = value_of(b, at, unicode)?;
    let bytes = if unicode && text {
        2 * value_len as usize
    } else {
        value_len as usize
    };
    let mut p = at + align(value + bytes - at);
    let mut found = Vec::new();
    while p + 4 <= end {
        let child = u16_at(b, p)? as usize;
        if child == 0 {
            break;
        }
        found.push(p);
        p += align(child);
    }
    Some(found)
}

/// VerQueryValueA(block, path, out, length): the address of the value
/// at `path` ("\" for VS_FIXEDFILEINFO, "\VarFileInfo\Translation",
/// "\StringFileInfo\040904E4\FileVersion") and its length. When there
/// is none, Windows 9x leaves the last error as it was and the length
/// 0.
fn ver_query_value(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[block, path, out, length] = args else {
        unreachable!()
    };
    if block == 0 {
        return Some(FALSE);
    }
    let path = if path == 0 {
        b"\\".to_vec()
    } else {
        super::user32::bytes_at(cpu, path)?
    };
    if !cpu.bus.guest_probe(block, 6, false) {
        return None;
    }
    let size = cpu.bus.guest_read_16(block) as usize;
    let mut b = vec![0u8; size.max(6)];
    if !cpu.bus.guest_probe(block, b.len(), false) {
        return None;
    }
    cpu.bus.guest_read_bytes(block, &mut b);
    // The ANSI layout's key starts at 4, the Unicode one's type is there.
    let unicode = b[4] < b' ';
    let Some(found) = query(&b, &path, unicode) else {
        if length != 0 && !super::com::write_out(cpu, length, 0) {
            return None;
        }
        return Some(FALSE);
    };
    let at = match &found.text {
        Some(text) => match converted(cpu, block + found.value as u32, text) {
            Some(at) => at,
            None => return Some(FALSE),
        },
        None => block + found.value as u32,
    };
    if !super::com::write_out(cpu, out, at) {
        return None;
    }
    if length != 0 && !super::com::write_out(cpu, length, found.len) {
        return None;
    }
    Some(TRUE)
}

/// The address of `text`, the ANSI form of the Unicode value at
/// `from`: written over the value, which has room for it. None when the
/// block can't be written.
fn converted(cpu: &mut Cpu, from: u32, text: &[u8]) -> Option<u32> {
    if !cpu.bus.guest_probe(from, text.len(), true) {
        return None;
    }
    cpu.bus.guest_write_bytes(from, text);
    Some(from)
}

/// VerLanguageNameA(language, buffer, size): the language's name, as
/// much as fits; its length.
fn ver_language_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[lang, buffer, size] = args else {
        unreachable!()
    };
    let name = language_name(lang as u16);
    if buffer == 0 || size == 0 {
        return Some(name.len() as u32);
    }
    let n = name.len().min(size as usize - 1);
    if !cpu.bus.guest_probe(buffer, n + 1, true) {
        return None;
    }
    cpu.bus
        .guest_write_bytes(buffer, &[&name.as_bytes()[..n], &[0]].concat());
    Some(n as u32)
}

/// The names of languages, by LANGID, as VERSION.DLL gives them.
fn language_name(lang: u16) -> &'static str {
    match lang {
        0x0000 => "Language Neutral",
        0x0401 => "Arabic",
        0x0402 => "Bulgarian",
        0x0403 => "Catalan",
        0x0404 => "Traditional Chinese",
        0x0405 => "Czech",
        0x0406 => "Danish",
        0x0407 => "German",
        0x0408 => "Greek",
        0x0409 => "English (United States)",
        0x040A => "Spanish (Traditional Sort)",
        0x040B => "Finnish",
        0x040C => "French",
        0x040D => "Hebrew",
        0x040E => "Hungarian",
        0x040F => "Icelandic",
        0x0410 => "Italian",
        0x0411 => "Japanese",
        0x0412 => "Korean",
        0x0413 => "Dutch",
        0x0414 => "Norwegian (Bokmal)",
        0x0415 => "Polish",
        0x0416 => "Portuguese (Brazil)",
        0x0418 => "Romanian",
        0x0419 => "Russian",
        0x041A => "Croatian",
        0x041B => "Slovak",
        0x041D => "Swedish",
        0x041F => "Turkish",
        0x0804 => "Simplified Chinese",
        0x0807 => "German (Switzerland)",
        0x0809 => "English (United Kingdom)",
        0x080A => "Spanish (Mexico)",
        0x080C => "French (Belgium)",
        0x0810 => "Italian (Switzerland)",
        0x0813 => "Dutch (Belgium)",
        0x0814 => "Norwegian (Nynorsk)",
        0x0816 => "Portuguese",
        0x0C07 => "German (Austria)",
        0x0C09 => "English (Australia)",
        0x0C0A => "Spanish (Modern Sort)",
        0x0C0C => "French (Canada)",
        0x1009 => "English (Canada)",
        0x100C => "French (Switzerland)",
        _ => match lang & 0x3FF {
            0x07 => "German",
            0x09 => "English",
            0x0A => "Spanish",
            0x0C => "French",
            0x10 => "Italian",
            _ => "Language Neutral",
        },
    }
}

/// VerFindFileA's flag, and its results.
const VFFF_ISSHAREDFILE: u32 = 1;
const VFF_CURNEDEST: u32 = 1;
const VFF_FILEINUSE: u32 = 2;
const VFF_BUFFTOOSMALL: u32 = 4;

/// The directory the file `name` is in where Windows looks for it: in
/// `first`, then where OpenFile looks. "" when it isn't anywhere.
fn current_location(cpu: &mut Cpu, name: &str, first: &str) -> String {
    if !first.is_empty() {
        let path = format!("{}\\{}", first.trim_end_matches('\\'), name);
        let full = kernel32::full_path(cpu, &path);
        if cpu.bus.disk.is_file(&kernel32::dos_path(&full)) {
            return first.trim_end_matches('\\').to_string();
        }
    }
    let Some(found) = kernel32::search_file(cpu, name) else {
        return String::new();
    };
    match found.rfind('\\') {
        Some(2) => found[..3].to_string(),
        Some(i) => found[..i].to_string(),
        None => String::new(),
    }
}

/// VerFindFileA(flags, name, windows dir, app dir, cur, cur size, dest,
/// dest size): where the file should go (the system directory for a
/// shared file, the application's otherwise) and where a copy of it is
/// now; VFF_CURNEDEST when the two differ.
fn ver_find_file(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[flags, name, _, app, cur, cur_len, dest, dest_len] = args else {
        unreachable!()
    };
    let (Ok(name), Ok(app)) = (opt_string(cpu, name), opt_string(cpu, app)) else {
        return None;
    };
    let dest_dir = if flags & VFFF_ISSHAREDFILE != 0 {
        super::loader::SYSTEM_DIR.to_string()
    } else {
        app.unwrap_or_default()
    };
    let mut result = 0;
    let cur_dir = match &name {
        Some(name) if !name.is_empty() => {
            let found = current_location(cpu, name, &dest_dir);
            if !found.is_empty() {
                let path = format!("{}\\{}", found.trim_end_matches('\\'), name);
                if kernel32::path_in_use(cpu, &path) {
                    result |= VFF_FILEINUSE;
                }
            }
            found
        }
        _ => String::new(),
    };
    if !cur_dir.is_empty() && !cur_dir.eq_ignore_ascii_case(dest_dir.trim_end_matches('\\')) {
        result |= VFF_CURNEDEST;
    }
    for (buffer, len, text) in [(dest, dest_len, &dest_dir), (cur, cur_len, &cur_dir)] {
        if buffer == 0 || len == 0 {
            continue;
        }
        if !cpu.bus.guest_probe(len, 4, true) {
            return None;
        }
        let size = cpu.bus.guest_read_32(len);
        let bytes = crate::dosstr::to_bytes(text);
        if size < bytes.len() as u32 + 1 {
            result |= VFF_BUFFTOOSMALL;
        }
        if size > 0 {
            if !cpu
                .bus
                .guest_probe(buffer, size.min(bytes.len() as u32 + 1) as usize, true)
            {
                return None;
            }
            super::user32::copy_out(cpu, buffer, size, &bytes);
        }
        cpu.bus.guest_write_32(len, bytes.len() as u32 + 1);
    }
    Some(result)
}

/// VerInstallFileA's flags and results.
const VIFF_FORCEINSTALL: u32 = 1;
const VIFF_DONTDELETEOLD: u32 = 2;
const VIF_TEMPFILE: u32 = 0x1;
const VIF_MISMATCH: u32 = 0x2;
const VIF_SRCOLD: u32 = 0x4;
const VIF_DIFFTYPE: u32 = 0x20;
const VIF_WRITEPROT: u32 = 0x40;
const VIF_OUTOFSPACE: u32 = 0x100;
const VIF_ACCESSVIOLATION: u32 = 0x200;
const VIF_SHARINGVIOLATION: u32 = 0x400;
const VIF_CANNOTCREATE: u32 = 0x800;
const VIF_CANNOTDELETE: u32 = 0x1000;
const VIF_CANNOTRENAME: u32 = 0x2000;
const VIF_CANNOTDELETECUR: u32 = 0x4000;
const VIF_CANNOTREADSRC: u32 = 0x10000;
const VIF_BUFFTOOSMALL: u32 = 0x40000;

/// The VIF_ bit of a file error.
fn vif_error(code: u32) -> u32 {
    match code {
        5 => VIF_ACCESSVIOLATION,
        32 => VIF_SHARINGVIOLATION,
        _ => 0,
    }
}

/// The DOS path of a Win32 path.
fn dos(cpu: &mut Cpu, path: &str) -> String {
    let full = kernel32::full_path(cpu, path);
    kernel32::dos_path(&full)
}

/// The file version (MS, LS), type and subtype of a version resource.
fn fixed_info(resource: &Resource) -> Option<(u64, u32, u32)> {
    let node = parse(resource)?;
    let v = &node.value;
    if u32_at(v, 0)? != VS_FFI_SIGNATURE {
        return None;
    }
    let version = (u32_at(v, 8)? as u64) << 32 | u32_at(v, 12)? as u64;
    Some((version, u32_at(v, 36)?, u32_at(v, 40)?))
}

/// VerInstallFileA(flags, source name, dest name, source dir, dest dir,
/// current dir, temp name, temp size): copy the source, expanded if
/// COMPRESS.EXE packed it, over the file in the destination directory.
/// Unless forced, a destination file with a newer version or another
/// type stays, and the copy is left as a temporary file whose name is
/// returned with VIF_TEMPFILE; a later forced call with that name puts
/// it in place.
fn ver_install_file(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[
        flags,
        src_name,
        dest_name,
        src_dir,
        dest_dir,
        cur_dir,
        tmp,
        tmp_len,
    ] = args
    else {
        unreachable!()
    };
    let mut strings = [src_name, dest_name, src_dir, dest_dir, cur_dir]
        .into_iter()
        .map(|at| opt_string(cpu, at));
    let mut next = || strings.next().unwrap_or(Ok(None));
    let (Ok(src_name), Ok(dest_name), Ok(src_dir), Ok(dest_dir), Ok(cur_dir)) =
        (next(), next(), next(), next(), next())
    else {
        return None;
    };
    let (Some(src_name), Some(dest_name), Some(src_dir)) = (src_name, dest_name, src_dir) else {
        return Some(VIF_CANNOTREADSRC);
    };
    let dest_dir = match dest_dir {
        Some(d) if !d.is_empty() => d,
        _ => src_dir.clone(),
    };
    let join = |dir: &str, name: &str| format!("{}\\{}", dir.trim_end_matches('\\'), name);
    let source = join(&src_dir, &src_name);
    let dest = join(&dest_dir, &dest_name);
    let Ok((bytes, stamp, _)) = kernel32::read_path(cpu, &source) else {
        return Some(VIF_CANNOTREADSRC);
    };
    let bytes = match super::kernel32::lz::header(&bytes) {
        Some((b'A', _, _)) => super::kernel32::lz::expand(&bytes[14..]),
        Some(_) => return Some(VIF_CANNOTREADSRC),
        None => bytes,
    };
    let dest_dos = dos(cpu, &dest);
    if cpu
        .bus
        .disk
        .get_file_attribute(&dest_dos)
        .is_ok_and(|a| a & 0x01 != 0)
    {
        return Some(VIF_WRITEPROT);
    }
    // A forced install of a temporary file an earlier call left.
    let mut tmp_file = None;
    if flags & VIFF_FORCEINSTALL != 0 && tmp != 0 {
        let name = super::user32::string_at(cpu, tmp)?;
        if !name.is_empty() {
            let path = join(&dest_dir, &name);
            let path_dos = dos(cpu, &path);
            if cpu.bus.disk.is_file(&path_dos) {
                tmp_file = Some((path, name));
            }
        }
    }
    let (tmp_path, tmp_name) = match tmp_file {
        Some(found) => found,
        None => {
            let seed = cpu.bus.clock.icount as u32;
            let Some(name) = (0..0x1000u32)
                .map(|n| format!("VER{:04X}.TMP", (seed + n) & 0xFFFF))
                .find(|n| {
                    let path = dos(cpu, &join(&dest_dir, n));
                    !cpu.bus.disk.exists(&path)
                })
            else {
                return Some(VIF_CANNOTCREATE);
            };
            let path = join(&dest_dir, &name);
            let path_dos = dos(cpu, &path);
            match cpu
                .bus
                .disk
                .write_whole_file(&path_dos, &bytes, Some(stamp))
            {
                Ok(()) => {}
                Err(0x27) => return Some(VIF_OUTOFSPACE),
                Err(_) => return Some(VIF_CANNOTCREATE),
            }
            (path, name)
        }
    };
    let tmp_dos = dos(cpu, &tmp_path);
    let mut result = 0;
    if flags & VIFF_FORCEINSTALL == 0
        && let Ok(old) = file_resource(cpu, &dest)
    {
        let new = file_resource(cpu, &tmp_path);
        match (fixed_info(&old), new.as_ref().ok().and_then(fixed_info)) {
            (Some((old_version, old_type, old_sub)), Some((new_version, new_type, new_sub))) => {
                if old_version > new_version {
                    result |= VIF_MISMATCH | VIF_SRCOLD;
                }
                if (old_type, old_sub) != (new_type, new_sub) {
                    result |= VIF_MISMATCH | VIF_DIFFTYPE;
                }
            }
            (Some(_), None) => result |= VIF_MISMATCH | VIF_SRCOLD,
            _ => {}
        }
    }
    if result != 0 {
        let size = if tmp_len == 0 {
            0
        } else {
            if !cpu.bus.guest_probe(tmp_len, 4, true) {
                return None;
            }
            cpu.bus.guest_read_32(tmp_len)
        };
        if size < tmp_name.len() as u32 + 1 || tmp == 0 {
            let _ = cpu.bus.disk.delete_file(&tmp_dos);
            return Some(result | VIF_BUFFTOOSMALL);
        }
        super::user32::copy_out(cpu, tmp, size, tmp_name.as_bytes());
        cpu.bus.guest_write_32(tmp_len, tmp_name.len() as u32 + 1);
        return Some(result | VIF_TEMPFILE);
    }
    if cpu.bus.disk.exists(&dest_dos) {
        if kernel32::path_in_use(cpu, &dest) {
            let _ = cpu.bus.disk.delete_file(&tmp_dos);
            return Some(vif_error(32) | VIF_CANNOTDELETE);
        }
        if let Err(code) = cpu.bus.disk.delete_file(&dest_dos) {
            let _ = cpu.bus.disk.delete_file(&tmp_dos);
            return Some(vif_error(code as u32) | VIF_CANNOTDELETE);
        }
    }
    if flags & VIFF_DONTDELETEOLD == 0
        && let Some(cur) = cur_dir.filter(|c| !c.is_empty())
        && !cur
            .trim_end_matches('\\')
            .eq_ignore_ascii_case(dest_dir.trim_end_matches('\\'))
    {
        let old = dos(cpu, &join(&cur, &dest_name));
        if cpu.bus.disk.is_file(&old)
            && let Err(code) = cpu.bus.disk.delete_file(&old)
        {
            result |= vif_error(code as u32) | VIF_CANNOTDELETECUR;
        }
    }
    let (from, to) = (
        kernel32::full_path(cpu, &tmp_path),
        kernel32::full_path(cpu, &dest),
    );
    if let Err(code) = cpu
        .bus
        .disk
        .rename_file(&kernel32::dos_path(&from), &kernel32::dos_path(&to))
    {
        result |= vif_error(code as u32) | VIF_CANNOTRENAME;
        let _ = cpu.bus.disk.delete_file(&tmp_dos);
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_resource_converts_to_the_ansi_layout() {
        let resource = builtin_resource("KERNEL32.DLL");
        let node = parse(&resource).unwrap();
        assert_eq!(node.key, b"VS_VERSION_INFO");
        assert_eq!(u32_at(&node.value, 0), Some(VS_FFI_SIGNATURE));
        let ansi = write_ansi(&node);
        let found = query(&ansi, b"\\StringFileInfo\\040904e4\\FileVersion", false).unwrap();
        assert_eq!(
            &ansi[found.value..found.value + found.len as usize],
            b"4.00.950\0"
        );
        let root = query(&ansi, b"\\", false).unwrap();
        assert_eq!(root.len, FIXED_SIZE as u32);
        let translation = query(&ansi, b"\\VarFileInfo\\Translation", false).unwrap();
        assert_eq!(translation.len, 4);
        // The Unicode layout answers the same, its text converted.
        let b = resource.bytes();
        let found = query(b, b"\\StringFileInfo\\040904E4\\InternalName", true).unwrap();
        assert_eq!(found.text.as_deref(), Some(&b"KERNEL32\0"[..]));
        assert!(query(&ansi, b"\\StringFileInfo\\040904E4\\FileVers", false).is_none());
    }
}
