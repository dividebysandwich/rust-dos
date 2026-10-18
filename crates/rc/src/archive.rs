//! Zip archives as directories. A path that goes on past a `.ZIP` file
//! ("C:\GAMES\DOOM.ZIP\DATA\E1M1.WAD") is inside it: `Fs` wraps the
//! machine's `Host` and answers for those paths from the archive, so that
//! the panels, copying, moving, deleting and the viewer work in archives
//! as in directories. Changes are kept in memory and the archive is
//! written once an operation ends (`Fs::flush`).

use std::io::Cursor;

use rust_dos_zip::ZipEntry;

use crate::host::{Drive, Entry, Host, attr};
use crate::{cp437, path};

const NOT_FOUND: u8 = 0x02;
const NO_PATH: u8 = 0x03;
const DENIED: u8 = 0x05;
const INVALID: u8 = 0x0D;
const NOT_SAME: u8 = 0x11;
const PROTECTED: u8 = 0x13;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Data {
    /// As it is in the archive read.
    Kept(ZipEntry),
    /// Written since, unpacked.
    New(Vec<u8>),
}

/// A file or folder in an archive.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Item {
    /// Its path in the archive, '/' between the parts.
    name: String,
    /// Whether the name is still the one the archive has.
    same_name: bool,
    dir: bool,
    size: u64,
    time: u16,
    date: u16,
    data: Data,
}

/// An archive read, and what changed in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Archive {
    pub path: String,
    bytes: Vec<u8>,
    items: Vec<Item>,
    pub dirty: bool,
}

/// A DOS path inside an archive as the archive names it.
fn key(inner: &str) -> String {
    inner.trim_matches('\\').replace('\\', "/")
}

fn parent(name: &str) -> &str {
    name.rfind('/').map_or("", |i| &name[..i])
}

fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// Whether `name` is inside folder `dir` ("" the archive's root).
fn under(name: &str, dir: &str) -> bool {
    dir.is_empty()
        || (name.len() > dir.len()
            && name.as_bytes()[dir.len()] == b'/'
            && same(&name[..dir.len()], dir))
}

impl Archive {
    fn read(host: &mut dyn Host, path: &str) -> Result<Archive, u8> {
        let bytes = host.read(path)?;
        let entries =
            rust_dos_zip::central_directory(&mut Cursor::new(&bytes)).map_err(|_| INVALID)?;
        let items = entries
            .into_iter()
            .filter(|e| !e.path.is_empty())
            .map(|e| Item {
                name: e.path.clone(),
                same_name: true,
                dir: e.dir,
                size: e.size,
                time: e.time,
                date: e.date,
                data: Data::Kept(e),
            })
            .collect();
        Ok(Archive {
            path: path.to_string(),
            bytes,
            items,
            dirty: false,
        })
    }

    fn find(&self, name: &str) -> Option<usize> {
        self.items.iter().position(|i| same(&i.name, name))
    }

    /// Whether a folder is there: named, or in a file's path.
    fn is_dir(&self, name: &str) -> bool {
        name.is_empty()
            || self
                .items
                .iter()
                .any(|i| (i.dir && same(&i.name, name)) || under(&i.name, name))
    }

    fn list(&self, inner: &str) -> Result<Vec<Entry>, u8> {
        let dir = key(inner);
        if !self.is_dir(&dir) {
            return Err(NO_PATH);
        }
        let mut out: Vec<Entry> = Vec::new();
        for item in self.items.iter().filter(|i| under(&i.name, &dir)) {
            let rest = &item.name[if dir.is_empty() { 0 } else { dir.len() + 1 }..];
            let (name, deeper) = match rest.find('/') {
                Some(i) => (&rest[..i], true),
                None => (rest, false),
            };
            let entry = if deeper || item.dir {
                Entry {
                    name: name.to_string(),
                    attr: attr::DIRECTORY,
                    time: if deeper { 0 } else { item.time },
                    date: if deeper { 0 } else { item.date },
                    size: 0,
                }
            } else {
                Entry {
                    name: name.to_string(),
                    attr: attr::ARCHIVE,
                    size: item.size,
                    time: item.time,
                    date: item.date,
                }
            };
            match out.iter_mut().find(|e| same(&e.name, &entry.name)) {
                // A folder named in the archive has its own time.
                Some(e) if !deeper && item.dir => *e = entry,
                Some(_) => {}
                None => out.push(entry),
            }
        }
        Ok(out)
    }

    fn attributes(&self, inner: &str) -> Result<u8, u8> {
        let name = key(inner);
        match self.find(&name) {
            Some(i) if !self.items[i].dir => Ok(attr::ARCHIVE),
            _ if self.is_dir(&name) => Ok(attr::DIRECTORY),
            _ => Err(NOT_FOUND),
        }
    }

    fn contents(&self, inner: &str) -> Result<Vec<u8>, u8> {
        let i = self.find(&key(inner)).ok_or(NOT_FOUND)?;
        match &self.items[i].data {
            _ if self.items[i].dir => Err(DENIED),
            Data::New(data) => Ok(data.clone()),
            Data::Kept(entry) => {
                rust_dos_zip::read(&mut Cursor::new(&self.bytes), entry).map_err(|_| INVALID)
            }
        }
    }

    fn write(&mut self, inner: &str, data: &[u8], (time, date): (u16, u16)) -> Result<(), u8> {
        let name = key(inner);
        if !self.is_dir(parent(&name)) {
            return Err(NO_PATH);
        }
        if let Some(i) = self.find(&name) {
            if self.items[i].dir {
                return Err(DENIED);
            }
            self.items.remove(i);
        } else if self.is_dir(&name) {
            return Err(DENIED);
        }
        self.items.push(Item {
            name,
            same_name: false,
            dir: false,
            size: data.len() as u64,
            time,
            date,
            data: Data::New(data.to_vec()),
        });
        self.dirty = true;
        Ok(())
    }

    fn delete(&mut self, inner: &str) -> Result<(), u8> {
        match self.find(&key(inner)) {
            Some(i) if !self.items[i].dir => {
                self.items.remove(i);
                self.dirty = true;
                Ok(())
            }
            Some(_) => Err(DENIED),
            None => Err(NOT_FOUND),
        }
    }

    fn mkdir(&mut self, inner: &str, (time, date): (u16, u16)) -> Result<(), u8> {
        let name = key(inner);
        if self.find(&name).is_some() || self.is_dir(&name) {
            return Err(DENIED);
        }
        if !self.is_dir(parent(&name)) {
            return Err(NO_PATH);
        }
        self.items.push(Item {
            name,
            same_name: false,
            dir: true,
            size: 0,
            time,
            date,
            data: Data::New(Vec::new()),
        });
        self.dirty = true;
        Ok(())
    }

    fn rmdir(&mut self, inner: &str) -> Result<(), u8> {
        let name = key(inner);
        if self.items.iter().any(|i| under(&i.name, &name)) {
            return Err(DENIED);
        }
        match self.find(&name) {
            Some(i) if self.items[i].dir => {
                self.items.remove(i);
                self.dirty = true;
                Ok(())
            }
            Some(_) => Err(NO_PATH),
            // A folder only its files' paths named went with the last.
            None => Ok(()),
        }
    }

    /// A file renamed, or a folder and what is in it.
    fn rename(&mut self, from: &str, to: &str) -> Result<(), u8> {
        let (from, to) = (key(from), key(to));
        if self.find(&to).is_some() || self.is_dir(&to) {
            return Err(DENIED);
        }
        if !self.is_dir(parent(&to)) {
            return Err(NO_PATH);
        }
        let dir = self.is_dir(&from);
        let mut found = false;
        for item in &mut self.items {
            let new = if same(&item.name, &from) {
                to.clone()
            } else if dir && under(&item.name, &from) {
                format!("{}{}", to, &item.name[from.len()..])
            } else {
                continue;
            };
            item.name = new;
            item.same_name = false;
            found = true;
        }
        if !found {
            return Err(NOT_FOUND);
        }
        self.dirty = true;
        Ok(())
    }

    /// The archive as it is now.
    fn packed(&self) -> Result<Vec<u8>, String> {
        let mut entries = Vec::new();
        for item in &self.items {
            let name = match &item.data {
                Data::Kept(e) if item.same_name => e.raw_name.clone(),
                _ => cp437::bytes(&item.name),
            };
            let name: Vec<u8> = name.into_iter().rev().skip_while(|&b| b == b'/').collect();
            let name: Vec<u8> = name.into_iter().rev().collect();
            entries.push(match &item.data {
                Data::Kept(e) => rust_dos_zip::raw(&self.bytes, e, &name)?,
                Data::New(_) if item.dir => rust_dos_zip::folder(&name, item.time, item.date),
                Data::New(data) => rust_dos_zip::pack(&name, data, item.time, item.date),
            });
        }
        rust_dos_zip::write(&entries)
    }

    /// The newest time stamp in it, for the archive's own.
    fn stamp(&self) -> (u16, u16) {
        self.items
            .iter()
            .map(|i| (i.date, i.time))
            .max()
            .map_or((0, 0x21), |(date, time)| (time, date))
    }
}

/// Whether a name is a zip archive's, by its extension.
pub fn is_zip_name(name: &str) -> bool {
    path::split_name(name).1.eq_ignore_ascii_case("ZIP")
}

/// The machine's files, with zip archives as directories.
pub struct Fs<'a> {
    pub host: &'a mut dyn Host,
    pub zips: &'a mut Vec<Archive>,
}

/// The archive a path is in, and the path in it ("" for the archive
/// itself): the first `.ZIP` in the path that is a file. `known` are
/// archives already read.
pub fn split(host: &mut dyn Host, known: &[Archive], p: &str) -> Option<(String, String)> {
    if p.len() < 4 || !p.to_ascii_uppercase().contains(".ZIP") {
        return None;
    }
    let parts: Vec<&str> = p[3..].split('\\').filter(|s| !s.is_empty()).collect();
    let mut at = p[..3].to_string();
    for (i, part) in parts.iter().enumerate() {
        at = path::join(&at, part);
        if is_zip_name(part)
            && (known.iter().any(|z| same(&z.path, &at))
                || host.attributes(&at).is_ok_and(|a| a & attr::DIRECTORY == 0))
        {
            return Some((at, parts[i + 1..].join("\\")));
        }
    }
    None
}

/// Where DOS goes for a directory: an archive's own directory for what is
/// in it.
pub fn outside(host: &mut dyn Host, dir: &str) -> String {
    match split(host, &[], dir) {
        Some((zip, _)) => path::parent(&zip).unwrap_or(zip),
        None => dir.to_string(),
    }
}

impl Fs<'_> {
    fn split(&mut self, p: &str) -> Option<(String, String)> {
        split(self.host, self.zips, p)
    }

    /// An archive's place in the cache, read the first time.
    fn archive(&mut self, zip: &str) -> Result<&mut Archive, u8> {
        let at = match self.zips.iter().position(|z| same(&z.path, zip)) {
            Some(i) => i,
            None => {
                let archive = Archive::read(self.host, zip)?;
                self.zips.push(archive);
                self.zips.len() - 1
            }
        };
        Ok(&mut self.zips[at])
    }

    /// An archive written over or gone, read again when next asked for.
    fn forget(&mut self, p: &str) {
        self.zips.retain(|z| !same(&z.path, p));
    }

    fn writable(&self, p: &str) -> Result<(), u8> {
        let drive = path::drive(p);
        let ok = self
            .host
            .drives()
            .iter()
            .find(|d| d.drive == drive)
            .is_none_or(|d| d.writable);
        if ok { Ok(()) } else { Err(PROTECTED) }
    }

    /// The archives changed, written: an error's text if one can't be.
    pub fn flush(&mut self) -> Result<(), String> {
        let mut result = Ok(());
        for i in (0..self.zips.len()).rev() {
            if !self.zips[i].dirty {
                continue;
            }
            let archive = self.zips.remove(i);
            let written = archive.packed().and_then(|data| {
                self.host
                    .write(&archive.path, &data, archive.stamp())
                    .map_err(crate::ops::error_text)
            });
            if let Err(e) = written {
                result = Err(format!("Cannot write the archive {}: {}", archive.path, e));
            }
        }
        result
    }
}

impl Host for Fs<'_> {
    fn drives(&self) -> Vec<Drive> {
        self.host.drives()
    }

    fn current_drive(&self) -> u8 {
        self.host.current_drive()
    }

    fn current_dir(&self, drive: u8) -> Option<String> {
        self.host.current_dir(drive)
    }

    fn set_current(&mut self, dir: &str) {
        let dir = match self.split(dir) {
            Some((zip, _)) => path::parent(&zip).unwrap_or(zip),
            None => dir.to_string(),
        };
        self.host.set_current(&dir);
    }

    fn list(&mut self, dir: &str) -> Result<Vec<Entry>, u8> {
        match self.split(dir) {
            Some((zip, inner)) => self.archive(&zip)?.list(&inner),
            None => self.host.list(dir),
        }
    }

    fn read(&mut self, p: &str) -> Result<Vec<u8>, u8> {
        match self.split(p) {
            Some((zip, inner)) if !inner.is_empty() => self.archive(&zip)?.contents(&inner),
            _ => self.host.read(p),
        }
    }

    fn write(&mut self, p: &str, data: &[u8], stamp: (u16, u16)) -> Result<(), u8> {
        match self.split(p) {
            Some((zip, inner)) if !inner.is_empty() => {
                self.writable(&zip)?;
                self.archive(&zip)?.write(&inner, data, stamp)
            }
            _ => {
                self.forget(p);
                self.host.write(p, data, stamp)
            }
        }
    }

    fn delete(&mut self, p: &str) -> Result<(), u8> {
        match self.split(p) {
            Some((zip, inner)) if !inner.is_empty() => {
                self.writable(&zip)?;
                self.archive(&zip)?.delete(&inner)
            }
            _ => {
                self.forget(p);
                self.host.delete(p)
            }
        }
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), u8> {
        match (self.split(from), self.split(to)) {
            (None, None) => self.host.rename(from, to),
            (Some((zip, inner)), None) if inner.is_empty() => {
                self.forget(&zip);
                self.host.rename(from, to)
            }
            (Some((a, from)), Some((b, to)))
                if same(&a, &b) && !from.is_empty() && !to.is_empty() =>
            {
                self.writable(&a)?;
                self.archive(&a)?.rename(&from, &to)
            }
            // Into or out of an archive: copied, and the source deleted.
            _ => Err(NOT_SAME),
        }
    }

    fn mkdir(&mut self, p: &str) -> Result<(), u8> {
        match self.split(p) {
            Some((zip, inner)) if !inner.is_empty() => {
                self.writable(&zip)?;
                let (h, m, s) = self.host.clock();
                let time = ((h as u16) << 11) | ((m as u16) << 5) | (s as u16 / 2);
                let date = self.archive(&zip)?.stamp().1;
                self.archive(&zip)?.mkdir(&inner, (time, date))
            }
            _ => self.host.mkdir(p),
        }
    }

    fn rmdir(&mut self, p: &str) -> Result<(), u8> {
        match self.split(p) {
            Some((zip, inner)) if !inner.is_empty() => {
                self.writable(&zip)?;
                self.archive(&zip)?.rmdir(&inner)
            }
            _ => self.host.rmdir(p),
        }
    }

    fn attributes(&mut self, p: &str) -> Result<u8, u8> {
        match self.split(p) {
            Some((zip, inner)) if !inner.is_empty() => self.archive(&zip)?.attributes(&inner),
            _ => self.host.attributes(p),
        }
    }

    fn set_attributes(&mut self, p: &str, a: u8) -> Result<(), u8> {
        match self.split(p) {
            // An archive keeps no attributes of its own.
            Some((_, inner)) if !inner.is_empty() => Ok(()),
            _ => self.host.set_attributes(p, a),
        }
    }

    fn space(&self, drive: u8) -> Option<(u64, u64)> {
        self.host.space(drive)
    }

    fn label(&self, drive: u8) -> Option<String> {
        self.host.label(drive)
    }

    fn memory(&mut self) -> (u64, u64) {
        self.host.memory()
    }

    fn prompt(&self) -> Vec<u8> {
        self.host.prompt()
    }

    fn history(&self) -> Vec<String> {
        self.host.history()
    }

    fn now_ms(&self) -> u64 {
        self.host.now_ms()
    }

    fn clock(&self) -> (u8, u8, u8) {
        self.host.clock()
    }
}
