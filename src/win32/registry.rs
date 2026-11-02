//! The registry: a tree of keys holding named values, the system's,
//! shared by its processes. ADVAPI32's functions (advapi32.rs) work on it
//! through handles, which each belong to the process that opened them.
//!
//! As on Windows 95, there are three trees: HKEY_LOCAL_MACHINE,
//! HKEY_USERS and HKEY_DYN_DATA. The other predefined keys are keys in
//! them: HKEY_CLASSES_ROOT is HKLM\Software\Classes, HKEY_CURRENT_USER is
//! HKU\.Default (Windows 95 without user profiles), HKEY_CURRENT_CONFIG is
//! HKLM\Config\0001. Names are found whatever their case, values keep the
//! order they were first set in, and there is no security.
//!
//! The registry is a text file on drive C:, `C:\REGISTRY.REG`, in the
//! REGEDIT4 format of Windows 95's REGEDIT, so it can be read and edited
//! by hand. When a program first uses the registry, it starts from the
//! keys Windows 95 programs read (`SEED`), with the file's keys and
//! values put over them. Changes are written back to the file when a key
//! is closed or flushed and when the last process ends. HKEY_DYN_DATA,
//! which Windows 95 makes anew at each start, isn't saved. Without a
//! drive C:, or with one that can't be written, the registry lives until
//! the last process ends.

use crate::cpu::Cpu;

/// The predefined keys' handles.
pub const HKEY_CLASSES_ROOT: u32 = 0x8000_0000;
pub const HKEY_CURRENT_USER: u32 = 0x8000_0001;
pub const HKEY_LOCAL_MACHINE: u32 = 0x8000_0002;
pub const HKEY_USERS: u32 = 0x8000_0003;
pub const HKEY_CURRENT_CONFIG: u32 = 0x8000_0005;
pub const HKEY_DYN_DATA: u32 = 0x8000_0006;

/// Value types.
pub const REG_SZ: u32 = 1;
pub const REG_EXPAND_SZ: u32 = 2;
pub const REG_BINARY: u32 = 3;
pub const REG_DWORD: u32 = 4;

/// Where the registry is kept.
pub const FILE: &str = "C:\\REGISTRY.REG";

/// The trees, by the ids of their root keys.
const LOCAL_MACHINE: u32 = 0;
const USERS: u32 = 1;
const DYN_DATA: u32 = 2;
const ROOTS: [&str; 3] = ["HKEY_LOCAL_MACHINE", "HKEY_USERS", "HKEY_DYN_DATA"];
/// The parent of a root key.
const NO_PARENT: u32 = u32::MAX;

/// The handle of the first open key, and how far apart they are: small
/// multiples of four, as kernel objects' handles.
const FIRST_HANDLE: u32 = 0x100;

/// The keys Windows 95 programs read, as a fresh installation has them.
const SEED: &str = r#"REGEDIT4

[HKEY_LOCAL_MACHINE\Config\0001\Display\Settings]
"BitsPerPixel"="8"
"Resolution"="640,480"

[HKEY_LOCAL_MACHINE\Network\Logon]
"username"="User"

[HKEY_LOCAL_MACHINE\Software\Classes\CLSID]

[HKEY_LOCAL_MACHINE\Software\Microsoft\DirectX]
"Version"="4.06.00.0318"
"InstalledVersion"=hex:00,00,00,06,00,00,00,00

[HKEY_LOCAL_MACHINE\Software\Microsoft\Windows\CurrentVersion]
"VersionNumber"="4.00.950"
"Version"="Windows 95"
"ProductName"="Microsoft Windows 95"
"SubVersionNumber"=""
"ProductId"="12345-OEM-0012345-12345"
"RegisteredOwner"="User"
"RegisteredOrganization"=""
"SystemRoot"="C:\\WINDOWS"
"ProgramFilesDir"="C:\\Program Files"
"CommonFilesDir"="C:\\Program Files\\Common Files"
"MediaPath"="C:\\WINDOWS\\MEDIA"
"DevicePath"="C:\\WINDOWS\\INF"

[HKEY_LOCAL_MACHINE\Software\Microsoft\Windows\CurrentVersion\App Paths]

[HKEY_LOCAL_MACHINE\Software\Microsoft\Windows\CurrentVersion\Run]

[HKEY_LOCAL_MACHINE\Software\Microsoft\Windows\CurrentVersion\RunOnce]

[HKEY_LOCAL_MACHINE\Software\Microsoft\Windows\CurrentVersion\Uninstall]

[HKEY_LOCAL_MACHINE\Software\Microsoft\Windows\CurrentVersion\Setup]
"BootDir"="C:\\"
"WinDir"="C:\\WINDOWS"
"SysDir"="C:\\WINDOWS\\SYSTEM"

[HKEY_LOCAL_MACHINE\System\CurrentControlSet\Control\ComputerName\ComputerName]
"ComputerName"="RUSTDOS"

[HKEY_USERS\.Default\Control Panel\International]
"Locale"="00000409"

[HKEY_USERS\.Default\Software\Microsoft\Windows\CurrentVersion\Explorer\Shell Folders]
"Desktop"="C:\\WINDOWS\\Desktop"
"Fonts"="C:\\WINDOWS\\FONTS"
"Programs"="C:\\WINDOWS\\Start Menu\\Programs"
"Start Menu"="C:\\WINDOWS\\Start Menu"
"Startup"="C:\\WINDOWS\\Start Menu\\Programs\\StartUp"

[HKEY_LOCAL_MACHINE\Software\Classes\CLSID\{0BE35203-8F91-11CE-9DE3-00AA004BB851}]
@="Standard Font"

[HKEY_LOCAL_MACHINE\Software\Classes\CLSID\{0BE35203-8F91-11CE-9DE3-00AA004BB851}\ProgID]
@="StdFont"

[HKEY_LOCAL_MACHINE\Software\Classes\StdFont]
@="Standard Font"

[HKEY_LOCAL_MACHINE\Software\Classes\StdFont\CLSID]
@="{0BE35203-8F91-11CE-9DE3-00AA004BB851}"

[HKEY_LOCAL_MACHINE\Software\Classes\Interface\{00000000-0000-0000-C000-000000000046}]
@="IUnknown"

[HKEY_LOCAL_MACHINE\Software\Classes\Interface\{00000000-0000-0000-C000-000000000046}\ProxyStubClsid32]
@="{00000320-0000-0000-C000-000000000046}"

[HKEY_LOCAL_MACHINE\Software\Classes\Interface\{00000001-0000-0000-C000-000000000046}]
@="IClassFactory"

[HKEY_LOCAL_MACHINE\Software\Classes\Interface\{00000001-0000-0000-C000-000000000046}\ProxyStubClsid32]
@="{00000320-0000-0000-C000-000000000046}"

[HKEY_LOCAL_MACHINE\Software\Classes\TypeLib\{00020430-0000-0000-C000-000000000046}\1.0]
@="OLE Automation"

[HKEY_LOCAL_MACHINE\Software\Classes\TypeLib\{00020430-0000-0000-C000-000000000046}\1.0\0\win32]
@="C:\\WINDOWS\\SYSTEM\\STDOLE32.TLB"

[HKEY_LOCAL_MACHINE\Software\Classes\TypeLib\{00020430-0000-0000-C000-000000000046}\1.0\FLAGS]
@="0"

[HKEY_LOCAL_MACHINE\Software\Classes\TypeLib\{00020430-0000-0000-C000-000000000046}\1.0\HELPDIR]
@="C:\\WINDOWS\\SYSTEM"

[HKEY_LOCAL_MACHINE\Software\Classes\TypeLib\{00020430-0000-0000-C000-000000000046}\2.0]
@="OLE Automation"

[HKEY_LOCAL_MACHINE\Software\Classes\TypeLib\{00020430-0000-0000-C000-000000000046}\2.0\0\win32]
@="C:\\WINDOWS\\SYSTEM\\STDOLE2.TLB"

[HKEY_LOCAL_MACHINE\Software\Classes\TypeLib\{00020430-0000-0000-C000-000000000046}\2.0\FLAGS]
@="0"

[HKEY_LOCAL_MACHINE\Software\Classes\TypeLib\{00020430-0000-0000-C000-000000000046}\2.0\HELPDIR]
@="C:\\WINDOWS\\SYSTEM"
"#;

/// A named value. The default value of a key has the empty name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Value {
    pub name: String,
    pub kind: u32,
    pub data: Vec<u8>,
}

impl Value {
    /// The string the data holds, up to its first zero.
    pub fn text(&self) -> String {
        let end = self.data.iter().position(|&b| b == 0);
        crate::dosstr::from_bytes(&self.data[..end.unwrap_or(self.data.len())])
    }
}

#[derive(Clone, Debug, Default)]
pub struct Key {
    pub name: String,
    parent: u32,
    pub children: Vec<u32>,
    pub values: Vec<Value>,
}

/// An open key: None once the key is deleted.
#[derive(Clone, Debug, Default)]
pub struct Handle {
    key: Option<u32>,
    /// The id of the process it belongs to.
    process: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Registry {
    /// Whether the keys are there: the registry is read when a program
    /// first uses it.
    loaded: bool,
    /// The keys, by their ids; the trees' roots first.
    keys: Vec<Option<Key>>,
    handles: Vec<Option<Handle>>,
    /// Whether something changed since it was last saved.
    dirty: bool,
}

/// What a handle names, or the error a function returns for it.
pub enum Open {
    Key(u32),
    Error(u32),
}

const ERROR_INVALID_HANDLE: u32 = 6;
const ERROR_BADKEY: u32 = 1010;
const ERROR_KEY_DELETED: u32 = 1018;

impl Registry {
    /// The three trees, empty.
    fn empty() -> Registry {
        let root = |name: &str| {
            Some(Key {
                name: name.to_string(),
                parent: NO_PARENT,
                ..Key::default()
            })
        };
        Registry {
            loaded: true,
            keys: ROOTS.iter().map(|n| root(n)).collect(),
            ..Registry::default()
        }
    }

    /// The registry of a fresh installation of `version`, with `text` (a
    /// .reg file) put over it, and the version's values over that.
    pub fn seeded(text: Option<&str>, version: super::version::WindowsVersion) -> Registry {
        let mut registry = Registry::empty();
        registry.import(SEED);
        if let Some(text) = text {
            registry.import(text);
        }
        registry.import(&super::version::registry_values(version));
        registry.dirty = false;
        registry
    }

    pub fn key(&self, id: u32) -> &Key {
        self.keys[id as usize].as_ref().expect("a key")
    }

    fn key_mut(&mut self, id: u32) -> &mut Key {
        self.keys[id as usize].as_mut().expect("a key")
    }

    /// The subkey `name` of `key`.
    pub fn child(&self, key: u32, name: &str) -> Option<u32> {
        self.key(key)
            .children
            .iter()
            .copied()
            .find(|&c| self.key(c).name.eq_ignore_ascii_case(name))
    }

    /// The key at `path` under `key`, its names separated by backslashes.
    pub fn find(&self, key: u32, path: &str) -> Option<u32> {
        path.split('\\')
            .filter(|n| !n.is_empty())
            .try_fold(key, |k, name| self.child(k, name))
    }

    /// The key at `path` under `key`, made with the keys on the way to
    /// it where they aren't there; and whether it was made.
    pub fn create(&mut self, key: u32, path: &str) -> (u32, bool) {
        let mut key = key;
        let mut created = false;
        for name in path.split('\\').filter(|n| !n.is_empty()) {
            key = match self.child(key, name) {
                Some(child) => child,
                None => {
                    created = true;
                    self.add_key(key, name)
                }
            };
        }
        if created {
            self.dirty = true;
        }
        (key, created)
    }

    fn add_key(&mut self, parent: u32, name: &str) -> u32 {
        let key = Key {
            name: name.to_string(),
            parent,
            ..Key::default()
        };
        let id = match self.keys.iter().position(Option::is_none) {
            Some(i) => {
                self.keys[i] = Some(key);
                i as u32
            }
            None => {
                self.keys.push(Some(key));
                self.keys.len() as u32 - 1
            }
        };
        self.key_mut(parent).children.push(id);
        id
    }

    /// Whether `key` is a tree's root.
    pub fn is_root(&self, key: u32) -> bool {
        self.key(key).parent == NO_PARENT
    }

    /// Delete `key` with its subkeys, as Windows 95 does; the handles of
    /// them name a deleted key from now on.
    pub fn delete(&mut self, key: u32) {
        let parent = self.key(key).parent;
        if parent != NO_PARENT {
            self.key_mut(parent).children.retain(|&c| c != key);
        }
        let mut doomed = vec![key];
        let mut i = 0;
        while i < doomed.len() {
            doomed.extend(self.key(doomed[i]).children.iter().copied());
            i += 1;
        }
        for &id in &doomed {
            self.keys[id as usize] = None;
        }
        for handle in self.handles.iter_mut().flatten() {
            if handle.key.is_some_and(|k| doomed.contains(&k)) {
                handle.key = None;
            }
        }
        self.dirty = true;
    }

    /// Whether `key` is a Shell Folders key under Explorer (the user's,
    /// or the one under HKEY_LOCAL_MACHINE for the folders all users
    /// share), whose values are the paths of the special folders
    /// (shell32/folders.rs).
    pub fn is_shell_folders(&self, key: u32) -> bool {
        let k = self.key(key);
        k.name.eq_ignore_ascii_case("Shell Folders")
            && k.parent != NO_PARENT
            && self.key(k.parent).name.eq_ignore_ascii_case("Explorer")
    }

    /// The value `name` of `key`.
    pub fn value(&self, key: u32, name: &str) -> Option<&Value> {
        self.key(key)
            .values
            .iter()
            .find(|v| v.name.eq_ignore_ascii_case(name))
    }

    /// Set the value `name` of `key`, where it is if it is there already.
    pub fn set_value(&mut self, key: u32, name: &str, kind: u32, data: Vec<u8>) {
        let values = &mut self.key_mut(key).values;
        match values
            .iter_mut()
            .find(|v| v.name.eq_ignore_ascii_case(name))
        {
            Some(value) => {
                value.kind = kind;
                value.data = data;
            }
            None => values.push(Value {
                name: name.to_string(),
                kind,
                data,
            }),
        }
        self.dirty = true;
    }

    /// Delete the value `name` of `key`: false if there is none.
    pub fn delete_value(&mut self, key: u32, name: &str) -> bool {
        let values = &mut self.key_mut(key).values;
        let before = values.len();
        values.retain(|v| !v.name.eq_ignore_ascii_case(name));
        let deleted = values.len() < before;
        self.dirty |= deleted;
        deleted
    }

    /// The key a predefined key's handle names, if `hkey` is one Windows
    /// 95 has. Made if it isn't there.
    fn predefined(&mut self, hkey: u32) -> Option<u32> {
        let (root, path) = match hkey {
            HKEY_CLASSES_ROOT => (LOCAL_MACHINE, "Software\\Classes"),
            HKEY_CURRENT_USER => (USERS, ".Default"),
            HKEY_LOCAL_MACHINE => (LOCAL_MACHINE, ""),
            HKEY_USERS => (USERS, ""),
            HKEY_CURRENT_CONFIG => (LOCAL_MACHINE, "Config\\0001"),
            HKEY_DYN_DATA => (DYN_DATA, ""),
            _ => return None,
        };
        Some(match self.find(root, path) {
            Some(key) => key,
            None => self.create(root, path).0,
        })
    }

    /// The key the handle `hkey` of the process `process` names: a
    /// predefined key or one the process opened.
    pub fn open(&mut self, hkey: u32, process: u32) -> Open {
        if hkey == 0 {
            return Open::Error(ERROR_BADKEY);
        }
        if let Some(key) = self.predefined(hkey) {
            return Open::Key(key);
        }
        match self.handle(hkey, process) {
            Some(Handle { key: Some(key), .. }) => Open::Key(*key),
            Some(Handle { key: None, .. }) => Open::Error(ERROR_KEY_DELETED),
            None => Open::Error(ERROR_INVALID_HANDLE),
        }
    }

    fn handle(&self, hkey: u32, process: u32) -> Option<&Handle> {
        if hkey < FIRST_HANDLE || !hkey.is_multiple_of(4) {
            return None;
        }
        let handle = self.handles.get(((hkey - FIRST_HANDLE) / 4) as usize)?;
        handle.as_ref().filter(|h| h.process == process)
    }

    /// A new handle of `key` for the process `process`.
    pub fn new_handle(&mut self, key: u32, process: u32) -> u32 {
        let handle = Some(Handle {
            key: Some(key),
            process,
        });
        let i = match self.handles.iter().position(Option::is_none) {
            Some(i) => {
                self.handles[i] = handle;
                i
            }
            None => {
                self.handles.push(handle);
                self.handles.len() - 1
            }
        };
        FIRST_HANDLE + i as u32 * 4
    }

    /// Close the handle `hkey` of the process `process`: false if it has
    /// no such handle.
    pub fn close(&mut self, hkey: u32, process: u32) -> bool {
        if self.handle(hkey, process).is_none() {
            return false;
        }
        self.handles[((hkey - FIRST_HANDLE) / 4) as usize] = None;
        true
    }

    /// The process `process` ended: its handles close.
    fn close_process(&mut self, process: u32) {
        for slot in &mut self.handles {
            if slot.as_ref().is_some_and(|h| h.process == process) {
                *slot = None;
            }
        }
    }

    /// The root key a .reg file's name of a tree names, and the path in it.
    fn tree(name: &str) -> Option<(u32, &'static str)> {
        let upper = name.to_ascii_uppercase();
        Some(match upper.as_str() {
            "HKEY_LOCAL_MACHINE" | "HKLM" => (LOCAL_MACHINE, ""),
            "HKEY_USERS" | "HKU" => (USERS, ""),
            "HKEY_DYN_DATA" => (DYN_DATA, ""),
            "HKEY_CLASSES_ROOT" | "HKCR" => (LOCAL_MACHINE, "Software\\Classes"),
            "HKEY_CURRENT_USER" | "HKCU" => (USERS, ".Default"),
            "HKEY_CURRENT_CONFIG" | "HKCC" => (LOCAL_MACHINE, "Config\\0001"),
            _ => return None,
        })
    }

    /// Put the keys and values of `text`, a .reg file (REGEDIT4, or
    /// REGEDIT5's ANSI form), into the registry. `[-key]` deletes a key
    /// and `"name"=-` a value. Lines it doesn't understand are left out.
    pub fn import(&mut self, text: &str) {
        let mut current = None;
        for line in logical_lines(text) {
            let line = line.trim();
            if line.is_empty() || line.starts_with(';') {
                continue;
            }
            if let Some(rest) = line.strip_prefix('[') {
                let path = rest.strip_suffix(']').unwrap_or(rest);
                let (delete, path) = match path.strip_prefix('-') {
                    Some(path) => (true, path),
                    None => (false, path),
                };
                let (tree, rest) = path.split_once('\\').unwrap_or((path, ""));
                current = None;
                let Some((root, base)) = Registry::tree(tree) else {
                    continue;
                };
                if delete {
                    let found = self.find(root, base).and_then(|k| self.find(k, rest));
                    if let Some(key) = found.filter(|&k| !self.is_root(k)) {
                        self.delete(key);
                    }
                } else {
                    let base = self.create(root, base).0;
                    current = Some(self.create(base, rest).0);
                }
                continue;
            }
            let Some(key) = current else {
                continue;
            };
            let (name, rest) = match line.strip_prefix('@') {
                Some(rest) => (String::new(), rest),
                None => match quoted(line) {
                    Some(parsed) => parsed,
                    None => continue,
                },
            };
            let Some(data) = rest.trim_start().strip_prefix('=') else {
                continue;
            };
            match parse_data(data.trim()) {
                Some(Some((kind, data))) => self.set_value(key, &name, kind, data),
                Some(None) => {
                    self.delete_value(key, &name);
                }
                None => {}
            }
        }
    }

    /// The registry as a REGEDIT4 file: HKEY_LOCAL_MACHINE's and
    /// HKEY_USERS's keys, each with its values.
    pub fn export(&self) -> String {
        let mut out = String::from("REGEDIT4\r\n");
        for root in [LOCAL_MACHINE, USERS] {
            let mut stack = vec![(root, ROOTS[root as usize].to_string())];
            while let Some((key, path)) = stack.pop() {
                let k = self.key(key);
                if key != root {
                    out.push_str(&format!("\r\n[{}]\r\n", path));
                    for value in &k.values {
                        out.push_str(&export_value(value));
                    }
                }
                for &child in k.children.iter().rev() {
                    stack.push((child, format!("{}\\{}", path, self.key(child).name)));
                }
            }
        }
        out
    }
}

/// The lines of a .reg file, a line that ends with a backslash joined
/// with the next, as REGEDIT wraps long hex values.
fn logical_lines(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut joined = String::new();
    for line in text.lines() {
        let line = line.trim_end();
        let part = if joined.is_empty() {
            line
        } else {
            line.trim_start()
        };
        match part.strip_suffix('\\') {
            Some(start) => joined.push_str(start),
            None => {
                joined.push_str(part);
                lines.push(std::mem::take(&mut joined));
            }
        }
    }
    if !joined.is_empty() {
        lines.push(joined);
    }
    lines
}

/// The quoted string `text` starts with, its `\\` and `\"` undone, and
/// what follows it.
fn quoted(text: &str) -> Option<(String, &str)> {
    let rest = text.strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = rest.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return Some((out, &rest[i + 1..])),
            '\\' => out.push(chars.next()?.1),
            c => out.push(c),
        }
    }
    None
}

/// A value's data as a .reg file has it: Some(None) to delete the value,
/// None if it isn't understood.
fn parse_data(text: &str) -> Option<Option<(u32, Vec<u8>)>> {
    if text == "-" {
        return Some(None);
    }
    if text.starts_with('"') {
        let (s, _) = quoted(text)?;
        let mut data = crate::dosstr::to_bytes(&s);
        data.push(0);
        return Some(Some((REG_SZ, data)));
    }
    let lower = text.to_ascii_lowercase();
    if let Some(digits) = lower.strip_prefix("dword:") {
        let value = u32::from_str_radix(digits.trim(), 16).ok()?;
        return Some(Some((REG_DWORD, value.to_le_bytes().to_vec())));
    }
    let (kind, bytes) = if let Some(bytes) = lower.strip_prefix("hex:") {
        (REG_BINARY, bytes)
    } else {
        let rest = lower.strip_prefix("hex(")?;
        let (kind, bytes) = rest.split_once("):")?;
        (u32::from_str_radix(kind, 16).ok()?, bytes)
    };
    let data = bytes
        .split(',')
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .map(|b| u8::from_str_radix(b, 16).ok())
        .collect::<Option<Vec<u8>>>()?;
    Some(Some((kind, data)))
}

/// A value's line in a .reg file: a string in quotes when it is one
/// without zeros or line breaks in it, else hex bytes.
fn export_value(value: &Value) -> String {
    let escape = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let name = if value.name.is_empty() {
        "@".to_string()
    } else {
        format!("\"{}\"", escape(&value.name))
    };
    let text = match value.data.split_last() {
        Some((0, text)) if !text.iter().any(|b| matches!(b, 0 | b'\r' | b'\n')) => Some(text),
        _ => None,
    };
    let data = match (value.kind, text) {
        (REG_SZ, Some(text)) => format!("\"{}\"", escape(&crate::dosstr::from_bytes(text))),
        (REG_DWORD, _) if value.data.len() == 4 => {
            let dword = u32::from_le_bytes(value.data[..4].try_into().unwrap());
            format!("dword:{:08x}", dword)
        }
        (kind, _) => {
            let prefix = match kind {
                REG_BINARY => "hex:".to_string(),
                kind => format!("hex({:x}):", kind),
            };
            let mut line = format!("{}={}", name, prefix);
            let mut out = String::new();
            for (i, byte) in value.data.iter().enumerate() {
                let last = i + 1 == value.data.len();
                let part = format!("{:02x}{}", byte, if last { "" } else { "," });
                if line.len() + part.len() > 76 && !last {
                    out.push_str(&line);
                    out.push_str("\\\r\n");
                    line = "  ".to_string();
                }
                line.push_str(&part);
            }
            out.push_str(&line);
            out.push_str("\r\n");
            return out;
        }
    };
    format!("{}={}\r\n", name, data)
}

/// The registry, read when a program first uses it.
pub fn get(cpu: &mut Cpu) -> &mut Registry {
    if !cpu.bus.win32.registry.loaded {
        let file = cpu.bus.disk.file_data(FILE).and_then(|f| f.read().ok());
        let text = file.map(|bytes| crate::dosstr::from_bytes(&bytes));
        let version = cpu.bus.win32.os;
        // The CD-ROM drive ASPI shows, and where it is on the IDE
        // channels: from the secondary master on without MOUNT's -ide.
        let cd = super::wnaspi32::cd_drive(cpu).map(|(drive, _)| {
            let slot = cpu.bus.disk.ide_slot(drive);
            (drive, slot.unwrap_or(crate::ide::IdeSlot::CD_ORDER[0]))
        });
        let mut registry = Registry::seeded(text.as_deref(), version);
        registry.import(&devices(cd));
        registry.dirty = false;
        cpu.bus.win32.registry = registry;
    }
    &mut cpu.bus.win32.registry
}

/// The device keys of the CD-ROM drive `cd` (its DOS drive and its IDE
/// slot), the one WNASPI32 shows, as Windows 98 SE has them for an ATAPI
/// drive on an IDE channel: the drive's key under HKLM\Enum\SCSI, named
/// after its INQUIRY data; its channel's controller under
/// HKLM\Enum\BIOS\*PNP0600; and their device nodes in
/// HKEY_DYN_DATA\Config Manager\Enum, which name those keys.
///
/// Copy protections find the drive letter of an ASPI target this way.
/// SafeDisc 2 goes through Config Manager's nodes to the CDROM-class keys,
/// reads their SCSITargetID, SCSILUN and CurrentDriveLetterAssignment, and
/// the DeviceDesc of the controller the instance name points to; with no
/// such key it finds no CD-ROM drive ("Missing CD-ROM").
///
/// The values are those Windows 98 SE made for rust-dos's drive on the
/// secondary master. ESDI_506 names a drive's instance after its
/// controller's (0F for the primary channel, 10 for the secondary), its
/// target (0 master, 1 slave) and its LUN. Windows remakes these keys at
/// each start, so they are made each time the registry is read.
///
/// The keys under HKLM\Enum go into the registry file with the rest, so
/// the drive's keys from an earlier run are deleted first: a drive that
/// moved to the other channel, or is gone, leaves no second CDROM-class
/// key with a stale drive letter behind. Without a CD-ROM drive that
/// deletion is all there is.
fn devices(cd: Option<(u8, crate::ide::IdeSlot)>) -> String {
    use crate::ide::ChannelId;
    use crate::ide::atapi::{INQUIRY_PRODUCT, INQUIRY_REVISION, INQUIRY_VENDOR};
    // The SCSI hardware id: vendor (8) and product (16), spaces as
    // underscores, and the revision's first character.
    let id: String = format!("{INQUIRY_VENDOR:<8.8}{INQUIRY_PRODUCT:<16.16}")
        .replace(' ', "_")
        + &INQUIRY_REVISION[..1];
    let stale = format!("REGEDIT4\n\n[-HKEY_LOCAL_MACHINE\\Enum\\SCSI\\{id}]\n");
    let Some((drive, slot)) = cd else {
        return stale;
    };
    let (controller, hdc) = match slot.channel {
        ChannelId::Primary => ("0F", "0000"),
        ChannelId::Secondary => ("10", "0001"),
    };
    let target = u8::from(slot.slave);
    let instance = format!("BIOS&*PNP0600&{controller}{target}0");
    let letter = (b'A' + drive) as char;
    let master = u8::from(!slot.slave);
    format!(
        r#"{stale}
[HKEY_LOCAL_MACHINE\Enum\BIOS\*PNP0600\{controller}]
"Capabilities"=hex:14,00,00,00
"HardwareID"="BIOS\\*PNP0600,*PNP0600"
"ClassGUID"="{{4d36e96a-e325-11ce-bfc1-08002be10318}}"
"Class"="hdc"
"Driver"="hdc\\{hdc}"
"Mfg"="(Standard hard disk drivers)"
"DeviceDesc"="Standard IDE/ESDI Hard Disk Controller"
"ConfigFlags"=hex:00,00,00,00

[HKEY_LOCAL_MACHINE\Enum\SCSI\{id}\{instance}]
"Capabilities"=hex:14,00,00,00
"AutoInsertNotification"=hex:01
"SCSITargetID"="{target}"
"SCSILUN"="0"
"RevisionLevel"="{INQUIRY_REVISION:<4.4}"
"ProductId"="{INQUIRY_PRODUCT:<16.16}"
"Manufacturer"="{INQUIRY_VENDOR}"
"DeviceType"=hex:05
"Removable"=hex:01
"CurrentDriveLetterAssignment"="{letter}"
"IDEMaster"=hex:{master:02x}
"HardwareID"="{id},GenCD,SCSI\\{id}"
"Class"="CDROM"
"ClassGUID"="{{4d36e965-e325-11ce-bfc1-08002be10318}}"
"Driver"="CDROM\\0000"
"Mfg"="(Standard CD-ROM device)"
"DeviceDesc"="{INQUIRY_VENDOR} {INQUIRY_PRODUCT:<16.16}"
"ConfigFlags"=hex:00,00,00,00

[HKEY_DYN_DATA\Config Manager\Enum\{HDC_DEVNODE}]
"HardWareKey"="BIOS\\*PNP0600\\{controller}"
"Problem"=hex:00,00,00,00
"Status"=hex:0a,60,00,00

[HKEY_DYN_DATA\Config Manager\Enum\{CD_DEVNODE}]
"HardWareKey"="SCSI\\{id}\\{instance}"
"Problem"=hex:00,00,00,00
"Status"=hex:0a,60,00,00
"#
    )
}

/// The names of the controller's and the CD-ROM drive's device nodes
/// under Config Manager\Enum. Windows 98 names a node by its address in
/// the configuration manager's memory; these are two such addresses.
const HDC_DEVNODE: &str = "C29FF000";
const CD_DEVNODE: &str = "C2A01000";

/// Write the registry to its file, if it changed.
pub fn save(cpu: &mut Cpu) {
    let registry = &mut cpu.bus.win32.registry;
    if !registry.loaded || !registry.dirty {
        return;
    }
    registry.dirty = false;
    let bytes = crate::dosstr::to_bytes(&registry.export());
    if cpu.bus.disk.write_whole_file(FILE, &bytes, None).is_err() {
        cpu.bus
            .log_string(&format!("[W32] The registry can't be saved to {}", FILE));
    }
}

/// The string the value `name` ("" for the default) of the key `path`
/// under the predefined key `hkey` holds, as RegQueryValueA reads it;
/// None if the key or the value isn't there.
pub fn string_value(cpu: &mut Cpu, hkey: u32, path: &str, name: &str) -> Option<String> {
    let registry = get(cpu);
    let Open::Key(root) = registry.open(hkey, 0) else {
        return None;
    };
    let key = registry.find(root, path)?;
    Some(registry.value(key, name)?.text())
}

/// Whether the key `path` is under the predefined key `hkey`.
pub fn key_exists(cpu: &mut Cpu, hkey: u32, path: &str) -> bool {
    let registry = get(cpu);
    let Open::Key(root) = registry.open(hkey, 0) else {
        return false;
    };
    registry.find(root, path).is_some()
}

/// Put the .reg file `text` into the registry and save it.
pub fn import_file(cpu: &mut Cpu, text: &str) -> Result<(), String> {
    let ((), saved) = change_and_save(cpu, |registry| registry.import(text));
    saved.map_err(|_| format!("the registry can't be saved to {}", FILE))
}

/// Change the registry with `f` and save it at once, whether a program
/// runs or not. With no program running, the registry is read from its
/// file for this and let go after, so the next program reads it again,
/// as after `process_ended`. What `f` returns, and whether the file was
/// written.
fn change_and_save<T>(cpu: &mut Cpu, f: impl FnOnce(&mut Registry) -> T) -> (T, Result<(), u8>) {
    let idle = cpu.bus.win32.processes.is_empty();
    let registry = get(cpu);
    let result = f(registry);
    registry.dirty = false;
    let bytes = crate::dosstr::to_bytes(&registry.export());
    let saved = cpu.bus.disk.write_whole_file(FILE, &bytes, None);
    if idle {
        cpu.bus.win32.registry = Registry::default();
    } else if saved.is_err() {
        cpu.bus.win32.registry.dirty = true;
    }
    (result, saved)
}

/// The string values (REG_SZ and REG_EXPAND_SZ) of each key of `keys`,
/// a path under a predefined key, as (name, text), in the order they
/// were set: the order RegEnumValueA gives them in. Nothing for a key
/// that isn't there. The registry is read once for all of them, and with
/// no program running it is let go after, as by `change_and_save`.
pub fn string_values(cpu: &mut Cpu, keys: &[(u32, &str)]) -> Vec<Vec<(String, String)>> {
    let idle = cpu.bus.win32.processes.is_empty();
    let registry = get(cpu);
    let values = keys
        .iter()
        .map(|&(hkey, path)| {
            let Open::Key(root) = registry.open(hkey, 0) else {
                return Vec::new();
            };
            registry.find(root, path).map_or_else(Vec::new, |key| {
                registry
                    .key(key)
                    .values
                    .iter()
                    .filter(|v| v.kind == REG_SZ || v.kind == REG_EXPAND_SZ)
                    .map(|v| (v.name.clone(), v.text()))
                    .collect()
            })
        })
        .collect();
    if idle {
        cpu.bus.win32.registry = Registry::default();
    }
    values
}

/// Delete the value `name` of the key `path` under the predefined key
/// `hkey`, and save the registry at once: false if there was no such
/// value.
pub fn delete_value_now(cpu: &mut Cpu, hkey: u32, path: &str, name: &str) -> bool {
    let (deleted, saved) = change_and_save(cpu, |registry| {
        let Open::Key(root) = registry.open(hkey, 0) else {
            return false;
        };
        registry
            .find(root, path)
            .is_some_and(|key| registry.delete_value(key, name))
    });
    if saved.is_err() {
        cpu.bus
            .log_string(&format!("[W32] The registry can't be saved to {}", FILE));
    }
    deleted
}

/// The process `id` ended: its keys close. After the last process, the
/// registry is saved and read again by the next program, which sees what
/// was changed in the file meanwhile.
pub fn process_ended(cpu: &mut Cpu, id: u32) {
    cpu.bus.win32.registry.close_process(id);
    if cpu.bus.win32.processes.is_empty() {
        save(cpu);
        cpu.bus.win32.registry = Registry::default();
    }
}

crate::state_fields!(Value { name, kind, data });
crate::state_fields!(Key {
    name,
    parent,
    children,
    values
});
crate::state_fields!(Handle { key, process });
crate::state_fields!(Registry {
    loaded,
    keys,
    handles,
    dirty
});

#[cfg(test)]
mod tests {
    use super::super::version::WindowsVersion::{Win95, Win98Se};
    use super::*;

    #[test]
    fn the_seed_has_windows_95s_version() {
        let registry = Registry::seeded(None, Win95);
        let key = registry
            .find(
                LOCAL_MACHINE,
                "SOFTWARE\\Microsoft\\Windows\\CurrentVersion",
            )
            .unwrap();
        let value = registry.value(key, "versionnumber").unwrap();
        assert_eq!(value.data, b"4.00.950\0");
        assert!(registry.find(USERS, ".Default\\Software").is_some());
    }

    #[test]
    fn the_seed_says_where_windows_is() {
        let registry = Registry::seeded(None, Win95);
        let key = registry
            .find(
                LOCAL_MACHINE,
                "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Setup",
            )
            .unwrap();
        let value = |name| registry.value(key, name).unwrap().data.clone();
        assert_eq!(value("WinDir"), b"C:\\WINDOWS\0");
        assert_eq!(value("SysDir"), b"C:\\WINDOWS\\SYSTEM\0");
        assert_eq!(value("BootDir"), b"C:\\\0");
    }

    /// Every installed Windows has a product id. Moto Racer's setup keeps
    /// a checksum of it, and the game says it is "incorrectly installed"
    /// when the checksum of the id it reads (or of "12345", without one)
    /// differs. A different id breaks the games installed with this one.
    #[test]
    fn the_seed_has_a_product_id() {
        let registry = Registry::seeded(None, Win98Se);
        let path = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion";
        let key = registry.find(LOCAL_MACHINE, path).unwrap();
        let id = &registry.value(key, "ProductId").unwrap().data;
        assert_eq!(id, b"12345-OEM-0012345-12345\0");
    }

    #[test]
    fn the_version_values_follow_the_setting_over_the_file() {
        let file = "REGEDIT4\r\n\r\n\
                    [HKEY_LOCAL_MACHINE\\Software\\Microsoft\\Windows\\CurrentVersion]\r\n\
                    \"VersionNumber\"=\"4.00.950\"\r\n\"RegisteredOwner\"=\"Someone\"\r\n";
        let registry = Registry::seeded(Some(file), Win98Se);
        let path = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion";
        let key = registry.find(LOCAL_MACHINE, path).unwrap();
        let value = |name| registry.value(key, name).unwrap().data.clone();
        assert_eq!(value("VersionNumber"), b"4.10.2222\0");
        assert_eq!(value("SubVersionNumber"), b" A \0");
        assert_eq!(value("RegisteredOwner"), b"Someone\0");
        // The values keep their places.
        assert_eq!(registry.key(key).values[0].name, "VersionNumber");
        // DirectX 6.1 of Windows 98 SE, which has no InstalledVersion.
        let dx = registry
            .find(LOCAL_MACHINE, "SOFTWARE\\Microsoft\\DirectX")
            .unwrap();
        assert_eq!(registry.value(dx, "Version").unwrap().data, b"4.06.03.0518\0");
        assert!(registry.value(dx, "InstalledVersion").is_none());
    }

    #[test]
    fn shell_folders_is_known_by_its_name_under_explorer() {
        let mut registry = Registry::seeded(None, Win95);
        let explorer = ".Default\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer";
        let parent = registry.find(USERS, explorer).unwrap();
        let key = registry.find(parent, "Shell Folders").unwrap();
        assert!(registry.is_shell_folders(key));
        assert!(!registry.is_shell_folders(parent));
        let (other, _) = registry.create(USERS, ".Default\\Software\\Shell Folders");
        assert!(!registry.is_shell_folders(other));
        let value = registry.value(key, "Start Menu").unwrap();
        assert_eq!(value.text(), "C:\\WINDOWS\\Start Menu");
    }

    #[test]
    fn a_reg_file_survives_the_round_trip() {
        let mut registry = Registry::seeded(None, Win95);
        let (key, created) = registry.create(USERS, ".Default\\Software\\Test\\Sub");
        assert!(created);
        registry.set_value(key, "", REG_SZ, b"default\0".to_vec());
        registry.set_value(key, "Quote\"d", REG_SZ, b"a\\b\"c\0".to_vec());
        registry.set_value(key, "Dword", REG_DWORD, vec![1, 2, 3, 4]);
        registry.set_value(key, "Zeros", REG_SZ, b"a\0b\0".to_vec());
        registry.set_value(key, "Long", REG_BINARY, (0..100).collect());
        registry.set_value(key, "Multi", 7, b"a\0b\0\0".to_vec());
        let text = registry.export();
        assert!(text.contains("\"Dword\"=dword:04030201\r\n"));
        assert!(text.contains("@=\"default\"\r\n"));
        let again = Registry::seeded(Some(&text), Win95);
        let key = again.find(USERS, ".Default\\Software\\Test\\Sub").unwrap();
        assert_eq!(again.key(key).values, registry.key(key).values);
        assert_eq!(again.export(), text);
    }

    #[test]
    fn a_reg_file_deletes_keys_and_values() {
        let text = "REGEDIT4\r\n\r\n[HKEY_CURRENT_USER\\A\\B]\r\n\"x\"=\"1\"\r\n\"y\"=\"2\"\r\n\
                    \r\n[HKEY_CURRENT_USER\\A]\r\n\"z\"=hex:01,\\\r\n  02\r\n\
                    [HKEY_CURRENT_USER\\A\\B]\r\n\"x\"=-\r\n";
        let registry = Registry::seeded(Some(text), Win95);
        let a = registry.find(USERS, ".Default\\A").unwrap();
        assert_eq!(registry.value(a, "z").unwrap().data, [1, 2]);
        let b = registry.find(a, "b").unwrap();
        assert!(registry.value(b, "x").is_none());
        assert!(registry.value(b, "y").is_some());
        let deleted = Registry::seeded(Some(&format!("{}[-HKCU\\A]\r\n", text)), Win95);
        assert!(deleted.find(USERS, ".Default\\A").is_none());
    }

    #[test]
    fn handles_of_a_deleted_key_name_a_deleted_key() {
        let mut registry = Registry::seeded(None, Win95);
        let (key, _) = registry.create(LOCAL_MACHINE, "Software\\Doomed\\Child");
        let handle = registry.new_handle(key, 7);
        assert!(matches!(registry.open(handle, 7), Open::Key(k) if k == key));
        assert!(matches!(
            registry.open(handle, 8),
            Open::Error(ERROR_INVALID_HANDLE)
        ));
        let doomed = registry.find(LOCAL_MACHINE, "software\\doomed").unwrap();
        registry.delete(doomed);
        assert!(matches!(
            registry.open(handle, 7),
            Open::Error(ERROR_KEY_DELETED)
        ));
        assert!(registry.close(handle, 7));
        assert!(!registry.close(handle, 7));
        assert!(matches!(registry.open(0, 7), Open::Error(ERROR_BADKEY)));
    }

    /// A string value's text, without its terminating zero.
    fn text(registry: &Registry, key: u32, name: &str) -> String {
        let data = &registry.value(key, name).unwrap().data;
        String::from_utf8(data.strip_suffix(b"\0").unwrap_or(data).to_vec()).unwrap()
    }

    #[test]
    fn a_cd_drive_is_found_as_safedisc_looks_for_it() {
        use crate::ide::{ChannelId, IdeSlot};
        let mut registry = Registry::seeded(None, Win98Se);
        registry.import(&devices(Some((3, IdeSlot::new(ChannelId::Primary, true)))));
        // Config Manager's nodes name the device keys under HKLM\Enum.
        let nodes = registry.find(DYN_DATA, r"Config Manager\Enum").unwrap();
        let cd = registry
            .key(nodes)
            .children
            .iter()
            .filter(|&&node| registry.value(node, "HardWareKey").is_some())
            .map(|&node| text(&registry, node, "HardWareKey"))
            .filter_map(|path| registry.find(LOCAL_MACHINE, &format!(r"Enum\{path}")))
            .find(|&key| registry.value(key, "Class").is_some_and(|v| v.data.starts_with(b"CDROM")))
            .unwrap();
        assert_eq!(text(&registry, cd, "CurrentDriveLetterAssignment"), "D");
        assert_eq!(text(&registry, cd, "SCSITargetID"), "1");
        assert_eq!(text(&registry, cd, "SCSILUN"), "0");
        // The instance names its controller: the primary channel's, 0F.
        let instance = registry.key(cd).name.clone();
        assert_eq!(instance, "BIOS&*PNP0600&0F10");
        let hdc = registry.find(LOCAL_MACHINE, r"Enum\BIOS\*PNP0600\0F").unwrap();
        assert_eq!(text(&registry, hdc, "Class"), "hdc");
        // A run without the drive deletes the drive's key the registry file
        // kept, and Config Manager's node, made anew each time, isn't there.
        let mut later = Registry::seeded(Some(&registry.export()), Win98Se);
        later.import(&devices(None));
        let scsi = later.find(LOCAL_MACHINE, r"Enum\SCSI").unwrap();
        assert!(later.key(scsi).children.is_empty());
        assert!(later.find(DYN_DATA, r"Config Manager\Enum").is_none());
    }
}
