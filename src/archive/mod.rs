//! Archives as drives: a zip (or .dosz) or 7z archive is what
//! is below a drive's write overlay (`overlay::Lower`), read where it is,
//! through `hostfs`, so the libretro frontend's files are read too.
//!
//! An archive whose files are all in one folder has that folder as the
//! drive's root, as archives of games often hold the game's folder,
//! unless it is a .dosz package (`keeps_its_root`), whose root is. A
//! stored zip file is read from the archive as it is wanted; a compressed
//! one is decompressed whole the first time it is opened, and a 7z file
//! with the others of its block. They are kept for when they are opened
//! again, up to `CACHE_BYTES`.
//!
//! A .dosz can name the archive it goes over with an empty
//! `<parent>.parent` file: the drive has the parent's files under its own.

pub mod patch;
mod variants;
pub use variants::{Entry as VariantEntry, Variants};
pub mod zip;
#[cfg(feature = "sevenz")]
pub mod sevenz;

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::ffi::OsString;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use crate::hostfs::{self, Handle, Meta};
use crate::overlay::Lower;

/// How much of the archives' decompressed files is kept.
const CACHE_BYTES: usize = 256 << 20;

/// The extensions of the archives a drive can be.
const EXTENSIONS: &[&str] = &["zip", "dosz", "7z"];

/// Whether `path` is an archive's, by its extension.
pub fn is_archive_name(path: &Path) -> bool {
    path.extension().is_some_and(|e| EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// Where a file's contents are.
#[derive(Clone, Copy, Debug)]
enum Data {
    /// Zip: the entry of the central directory.
    Zip(usize),
    /// 7z: the entry, and the block it's in.
    #[cfg(feature = "sevenz")]
    SevenZ(usize, usize),
    Empty,
}

#[derive(Clone, Debug)]
struct Entry {
    /// Its name as the archive has it.
    name: OsString,
    is_dir: bool,
    size: u64,
    modified: Option<SystemTime>,
    data: Data,
}

enum Format {
    Zip(Vec<zip::ZipEntry>),
    #[cfg(feature = "sevenz")]
    SevenZ(Box<sevenz::SevenZ>),
}

/// The files decompressed, the last used last.
#[derive(Default)]
struct Cache {
    files: HashMap<usize, Arc<Vec<u8>>>,
    order: VecDeque<usize>,
    bytes: usize,
}

impl Cache {
    fn get(&mut self, key: usize) -> Option<Arc<Vec<u8>>> {
        let data = self.files.get(&key)?.clone();
        self.order.retain(|&k| k != key);
        self.order.push_back(key);
        Some(data)
    }

    fn put(&mut self, key: usize, data: Arc<Vec<u8>>) {
        if let Some(old) = self.files.insert(key, data.clone()) {
            self.bytes -= old.len();
            self.order.retain(|&k| k != key);
        }
        self.bytes += data.len();
        self.order.push_back(key);
        // The one just put stays, however big.
        while self.bytes > CACHE_BYTES && self.order.len() > 1 {
            let oldest = self.order.pop_front().expect("files in the cache");
            if let Some(old) = self.files.remove(&oldest) {
                self.bytes -= old.len();
            }
        }
    }
}

/// An archive, read.
pub struct Archive {
    path: PathBuf,
    format: Format,
    /// The files and folders, by their paths (from the drive's root) in
    /// lower case.
    entries: BTreeMap<String, Entry>,
    cache: RefCell<Cache>,
}

/// `path` in lower case, for finding it in any case.
fn key(path: &str) -> String {
    path.to_lowercase()
}

impl Archive {
    /// The archive at `path`: zip or 7z, whatever its extension says. A
    /// folder all the files are in is the root, unless `keep_root`.
    pub fn open(path: &Path, keep_root: bool) -> Result<Archive, String> {
        Self::open_part(path, keep_root, None)
    }

    /// The archive at `path`, or with `only`, the files of a .dosc's
    /// launch configuration of that name (its `[only]` folder) from its
    /// root.
    fn open_part(path: &Path, keep_root: bool, only: Option<&str>) -> Result<Archive, String> {
        let error = |e: String| format!("{}: {}", path.display(), e);
        let mut file = hostfs::File::open(path).map_err(|e| error(e.to_string()))?;
        let mut magic = [0u8; 6];
        file.read_exact(&mut magic).map_err(|e| error(e.to_string()))?;
        let (format, listed) = if magic == *b"7z\xBC\xAF\x27\x1C" {
            Self::sevenz(&mut file).map_err(error)?
        } else {
            let entries = zip::central_directory(&mut file).map_err(error)?;
            let listed = entries
                .iter()
                .enumerate()
                .map(|(i, e)| {
                    let modified = crate::disk::dos_to_system_time(e.time, e.date);
                    let data = if e.is_dir() { Data::Empty } else { Data::Zip(i) };
                    (e.name(), e.is_dir(), e.size, modified, data)
                })
                .collect();
            (Format::Zip(entries), listed)
        };
        let mut archive = Archive { path: path.to_path_buf(), format, entries: BTreeMap::new(), cache: RefCell::default() };
        archive.list(listed, keep_root, only);
        Ok(archive)
    }

    #[cfg(feature = "sevenz")]
    fn sevenz(file: &mut hostfs::File) -> Result<(Format, Vec<Listed>), String> {
        let archive = sevenz::SevenZ::read(file)?;
        let listed = archive
            .entries()
            .into_iter()
            .enumerate()
            .map(|(i, e)| {
                let data = match e.block {
                    Some(block) if !e.is_dir && e.size > 0 => Data::SevenZ(i, block),
                    _ => Data::Empty,
                };
                (e.name, e.is_dir, e.size, e.modified, data)
            })
            .collect();
        Ok((Format::SevenZ(Box::new(archive)), listed))
    }

    #[cfg(not(feature = "sevenz"))]
    fn sevenz(_file: &mut hostfs::File) -> Result<(Format, Vec<Listed>), String> {
        Err("7z archives can't be read in this build".to_string())
    }

    /// The entries by their paths, with the folders they are in, which
    /// archives needn't list; the folder all are in left out, unless
    /// `keep_root`. Unsafe paths and `.parent` markers aren't listed.
    fn list(&mut self, listed: Vec<Listed>, keep_root: bool, only: Option<&str>) {
        let safe = |name: &str| !name.is_empty() && name.split('/').all(|p| !p.is_empty() && p != "." && p != "..");
        // A .dosc's launch configurations ([Setup Program]/...) are other
        // ways to start the game, not its files, but for the one asked for.
        let changes = is_dosc(&self.path);
        let folder = only.map(|name| format!("[{}]/", name.to_lowercase()));
        let listed: Vec<Listed> = listed
            .into_iter()
            .filter_map(|mut l| match &folder {
                Some(folder) => {
                    let inner = l.0.get(folder.len()..).filter(|_| l.0.to_lowercase().starts_with(folder.as_str()))?;
                    l.0 = inner.to_string();
                    Some(l)
                }
                None => (!(changes && l.0.starts_with('['))).then_some(l),
            })
            .filter(|l| safe(&l.0) && parent_marker(&l.0).is_none())
            .collect();
        let top = listed.first().and_then(|l| l.0.split('/').next()).map(str::to_string);
        let strip = top.filter(|top| {
            !keep_root
                && listed.iter().all(|l| l.0 == *top && l.1 || l.0.strip_prefix(top.as_str()).is_some_and(|r| r.starts_with('/')))
        });
        for (name, is_dir, size, modified, data) in listed {
            let name = match &strip {
                Some(top) if name == *top => continue,
                Some(top) => name[top.len() + 1..].to_string(),
                None => name,
            };
            // Its folders, then it.
            let mut at = 0;
            while let Some(slash) = name[at..].find('/') {
                let folder = &name[..at + slash];
                self.entries.entry(key(folder)).or_insert_with(|| Entry {
                    name: folder.rsplit('/').next().unwrap_or(folder).into(),
                    is_dir: true,
                    size: 0,
                    modified,
                    data: Data::Empty,
                });
                at += slash + 1;
            }
            let leaf = name.rsplit('/').next().unwrap_or(&name).to_string();
            self.entries.insert(key(&name), Entry { name: leaf.into(), is_dir, size, modified, data });
        }
    }

    /// The paths of the files, from the root.
    pub fn files(&self) -> Vec<String> {
        let mut files = Vec::new();
        for (path, entry) in &self.entries {
            if !entry.is_dir {
                // The names as the archive has them, from the folders'.
                let mut parts: Vec<String> = Vec::new();
                let mut at = path.as_str();
                loop {
                    parts.push(self.entries[at].name.to_string_lossy().into_owned());
                    match at.rsplit_once('/') {
                        Some((parent, _)) => at = parent,
                        None => break,
                    }
                }
                parts.reverse();
                files.push(parts.join("/"));
            }
        }
        files
    }

    fn entry(&self, path: &str) -> io::Result<&Entry> {
        self.entries.get(&key(path)).ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }

    /// A decompressed file's contents.
    fn contents(&self, data: Data) -> io::Result<Arc<Vec<u8>>> {
        let error = |e: String| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {}", self.path.display(), e));
        let index = match data {
            Data::Zip(i) => i,
            #[cfg(feature = "sevenz")]
            Data::SevenZ(i, _) => i,
            Data::Empty => return Ok(Arc::new(Vec::new())),
        };
        if let Some(data) = self.cache.borrow_mut().get(index) {
            return Ok(data);
        }
        let mut file = hostfs::File::open(&self.path)?;
        match (&self.format, data) {
            (Format::Zip(entries), Data::Zip(i)) => {
                let contents = Arc::new(zip::read(&mut file, &entries[i]).map_err(error)?);
                self.cache.borrow_mut().put(i, contents.clone());
                Ok(contents)
            }
            #[cfg(feature = "sevenz")]
            (Format::SevenZ(archive), Data::SevenZ(i, block)) => {
                let mut wanted = None;
                let mut cache = self.cache.borrow_mut();
                for (at, contents) in archive.read_block(&mut file, block).map_err(error)? {
                    let contents = Arc::new(contents);
                    if at == i {
                        wanted = Some(contents);
                    } else {
                        cache.put(at, contents);
                    }
                }
                let wanted = wanted.ok_or_else(|| error("a file missing from its block".to_string()))?;
                cache.put(i, wanted.clone());
                Ok(wanted)
            }
            _ => Err(error("the wrong kind of entry".to_string())),
        }
    }
}

/// An entry as `Archive::open` reads it: its path, whether it is a
/// folder, its size and date, and where its contents are.
type Listed = (String, bool, u64, Option<SystemTime>, Data);

/// The archive a `<name>.parent` marker in an archive's root names.
fn parent_marker(name: &str) -> Option<&str> {
    let stem = name.len().checked_sub(7).filter(|&n| n > 0 && name.is_char_boundary(n))?;
    (name[stem..].eq_ignore_ascii_case(".parent") && !name.contains('/')).then(|| &name[..stem])
}

impl Lower for Archive {
    fn metadata(&self, path: &str) -> io::Result<Meta> {
        if path.is_empty() {
            return Ok(Meta { is_dir: true, len: 0, modified: None, readonly: false });
        }
        let entry = self.entry(path)?;
        Ok(Meta { is_dir: entry.is_dir, len: entry.size, modified: entry.modified, readonly: false })
    }

    fn read_dir(&self, path: &str) -> io::Result<Vec<(OsString, bool)>> {
        if !path.is_empty() && !self.entry(path)?.is_dir {
            return Err(io::Error::from(io::ErrorKind::NotFound));
        }
        let prefix = if path.is_empty() { String::new() } else { format!("{}/", key(path)) };
        Ok(self
            .entries
            .range(prefix.clone()..)
            .take_while(|(k, _)| k.starts_with(&prefix))
            .filter(|(k, _)| !k[prefix.len()..].contains('/'))
            .map(|(_, e)| (e.name.clone(), e.is_dir))
            .collect())
    }

    fn open(&self, path: &str) -> io::Result<Box<dyn Handle>> {
        let entry = self.entry(path)?;
        if entry.is_dir {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "a folder"));
        }
        let modified = entry.modified;
        // A stored zip file is read where it is.
        if let (Format::Zip(entries), Data::Zip(i)) = (&self.format, entry.data)
            && entries[i].method == 0
            && !entries[i].encrypted()
        {
            let file = hostfs::File::open(&self.path)?;
            let start = entries[i].data_at;
            return Ok(Box::new(Slice { file, start, len: entries[i].size, pos: 0, modified }));
        }
        Ok(Box::new(Bytes { data: self.contents(entry.data)?, pos: 0, modified }))
    }
}

fn read_only() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, "an archive's files can't be written")
}

/// A seek to `pos` in a file of `len` bytes, from `at`.
fn seek_to(at: u64, len: u64, pos: SeekFrom) -> io::Result<u64> {
    let to = match pos {
        SeekFrom::Start(n) => Some(n),
        SeekFrom::Current(n) => at.checked_add_signed(n),
        SeekFrom::End(n) => len.checked_add_signed(n),
    };
    to.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "a seek before the start"))
}

/// A file decompressed.
struct Bytes {
    data: Arc<Vec<u8>>,
    pos: u64,
    modified: Option<SystemTime>,
}

impl Read for Bytes {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let at = (self.pos as usize).min(self.data.len());
        let n = buf.len().min(self.data.len() - at);
        buf[..n].copy_from_slice(&self.data[at..at + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl Write for Bytes {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(read_only())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Seek for Bytes {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.pos = seek_to(self.pos, self.data.len() as u64, pos)?;
        Ok(self.pos)
    }
}

impl Handle for Bytes {
    fn len(&mut self) -> io::Result<u64> {
        Ok(self.data.len() as u64)
    }
    fn set_len(&mut self, _: u64) -> io::Result<()> {
        Err(read_only())
    }
    fn modified(&mut self) -> Option<SystemTime> {
        self.modified
    }
}

/// A stored file, read from the archive.
struct Slice {
    file: hostfs::File,
    start: u64,
    len: u64,
    pos: u64,
    modified: Option<SystemTime>,
}

impl Read for Slice {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.len.saturating_sub(self.pos);
        let n = (buf.len() as u64).min(left) as usize;
        if n == 0 {
            return Ok(0);
        }
        self.file.seek(SeekFrom::Start(self.start + self.pos))?;
        let n = self.file.read(&mut buf[..n])?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Write for Slice {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(read_only())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Seek for Slice {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.pos = seek_to(self.pos, self.len, pos)?;
        Ok(self.pos)
    }
}

impl Handle for Slice {
    fn len(&mut self) -> io::Result<u64> {
        Ok(self.len)
    }
    fn set_len(&mut self, _: u64) -> io::Result<()> {
        Err(read_only())
    }
    fn modified(&mut self) -> Option<SystemTime> {
        self.modified
    }
}

/// Archives over each other, the first over the rest: a .dosz and its
/// parents, each with its .dosc over it, and the files the .dosc patches.
pub struct Stack {
    layers: Vec<Archive>,
    /// Files a .dosc patches, by their paths in lower case.
    patched: BTreeMap<String, Patched>,
    /// The patches named after their files (`GAME.EXE.ips`), which aren't
    /// shown, in lower case.
    hidden: Vec<String>,
    /// What to tell the user: patches that can't be used.
    pub notes: RefCell<Vec<String>>,
}

/// A patched file's contents and the archive of its base; or, the patch
/// having failed, the base's archive, which has the file as it is.
type Patching<'a> = Result<(Arc<Vec<u8>>, &'a Archive), &'a Archive>;

/// A file a .dosc patches: the layer of its base, and the patch's layer,
/// path and kind; patched the first time it is wanted.
struct Patched {
    base: usize,
    patch: usize,
    patch_path: String,
    kind: patch::Kind,
    /// The patched contents (None: the patch failed, and the file is the
    /// base's).
    result: RefCell<Option<Option<Arc<Vec<u8>>>>>,
}

impl Stack {
    /// The topmost layer with `path`.
    fn layer_of(&self, path: &str) -> Option<usize> {
        self.layers.iter().position(|a| a.entry(path).is_ok())
    }

    /// The file at `path` as its patch makes it, if a patch is what the
    /// layers have there; Err for the base's when the patch fails.
    fn patched(&self, path: &str) -> Option<Patching<'_>> {
        let patched = self.patched.get(&key(path))?;
        let top = self.layer_of(path)?;
        if top != patched.patch && top != patched.base {
            return None;
        }
        let base = &self.layers[patched.base];
        let mut result = patched.result.borrow_mut();
        if result.is_none() {
            let read = |archive: &Archive, path: &str| -> io::Result<Vec<u8>> {
                let mut data = Vec::new();
                archive.open(path)?.read_to_end(&mut data)?;
                Ok(data)
            };
            let made = read(base, path)
                .and_then(|b| Ok((b, read(&self.layers[patched.patch], &patched.patch_path)?)))
                .map_err(|e| e.to_string())
                .and_then(|(b, p)| patch::apply(patched.kind, &b, &p));
            *result = Some(match made {
                Ok(data) => Some(Arc::new(data)),
                Err(e) => {
                    self.notes.borrow_mut().push(format!("{}: {}", patched.patch_path, e));
                    None
                }
            });
        }
        Some(result.clone().flatten().map(|data| (data, base)).ok_or(base))
    }

    /// The patches the .dosc at `patch` (a layer) has for its game at
    /// `base`: files of the game's name that are patches by their first
    /// bytes, and patches named after a file of the game.
    fn find_patches(&mut self, patch: usize, base: usize) {
        for file in self.layers[patch].files() {
            let lower = key(&file);
            let named = patch::EXTENSIONS.iter().find_map(|(ext, kind)| {
                let target = lower.strip_suffix(ext)?.strip_suffix('.')?;
                Some((target.to_string(), *kind))
            });
            let found = match named {
                Some((target, kind)) if self.layers[base].entry(&target).is_ok_and(|e| !e.is_dir) => {
                    self.hidden.push(lower.clone());
                    Some((target, kind))
                }
                _ if self.layers[base].entry(&file).is_ok_and(|e| !e.is_dir) => {
                    let mut magic = [0u8; 5];
                    let read = self.layers[patch].open(&file).and_then(|mut h| h.read(&mut magic));
                    read.ok().and_then(|n| patch::kind(&magic[..n])).map(|kind| (lower.clone(), kind))
                }
                _ => None,
            };
            match found {
                Some((_, patch::Kind::Xor)) => {
                    self.notes.borrow_mut().push(format!("{}: XOR patches aren't supported; the file is as the game has it", file))
                }
                Some((target, kind)) => {
                    let patched = Patched { base, patch, patch_path: file, kind, result: RefCell::new(None) };
                    self.patched.insert(target, patched);
                }
                None => {}
            }
        }
    }
}

impl Lower for Stack {
    fn metadata(&self, path: &str) -> io::Result<Meta> {
        match self.patched(path) {
            Some(Ok((data, base))) => Ok(Meta { len: data.len() as u64, ..base.metadata(path)? }),
            Some(Err(base)) => base.metadata(path),
            None => self
                .layers
                .iter()
                .find_map(|a| a.metadata(path).ok())
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound)),
        }
    }

    fn read_dir(&self, path: &str) -> io::Result<Vec<(OsString, bool)>> {
        let mut names: Vec<(OsString, bool)> = Vec::new();
        let mut found = false;
        let prefix = if path.is_empty() { String::new() } else { format!("{}/", key(path)) };
        for archive in &self.layers {
            let Ok(more) = archive.read_dir(path) else { continue };
            found = true;
            for (name, is_dir) in more {
                let lower = name.to_string_lossy().to_lowercase();
                if self.hidden.contains(&format!("{}{}", prefix, lower)) {
                    continue;
                }
                if !names.iter().any(|(n, _)| n.to_string_lossy().to_lowercase() == lower) {
                    names.push((name, is_dir));
                }
            }
        }
        if found { Ok(names) } else { Err(io::Error::from(io::ErrorKind::NotFound)) }
    }

    fn open(&self, path: &str) -> io::Result<Box<dyn Handle>> {
        match self.patched(path) {
            Some(Ok((data, base))) => {
                let modified = base.metadata(path)?.modified;
                Ok(Box::new(Bytes { data, pos: 0, modified }))
            }
            Some(Err(base)) => base.open(path),
            None => {
                let at = self.layer_of(path).ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
                self.layers[at].open(path)
            }
        }
    }
}

impl Stack {
    /// The paths of the files, from the root.
    pub fn files(&self) -> Vec<String> {
        let mut files: Vec<String> = Vec::new();
        for archive in &self.layers {
            for file in archive.files() {
                if !self.hidden.contains(&key(&file)) && !files.iter().any(|f| f.eq_ignore_ascii_case(&file)) {
                    files.push(file);
                }
            }
        }
        files
    }
}

/// The archive at `path` as a drive's files: with the parents a .dosz
/// names under it, each with its .dosc.
pub fn open(path: &Path) -> Result<Stack, String> {
    open_variant(path, None)
}

/// `open`, with the launch configuration `variant` of the .dosc files
/// (their `[variant]` folders) over all of it: the parents' under the
/// child's, as the format orders them.
pub fn open_variant(path: &Path, variant: Option<&str>) -> Result<Stack, String> {
    let mut chain = vec![path.to_path_buf()];
    loop {
        let last = chain.last().expect("an archive");
        let Some(parent) = parent_of(last)? else { break };
        if chain.contains(&parent) {
            return Err(format!("{}: the parent archives go round in a circle", path.display()));
        }
        if !hostfs::is_file(&parent) {
            return Err(format!("the parent archive {} isn't there", parent.display()));
        }
        chain.push(parent);
    }
    // A .dosz and its parents line up from their roots, and an archive
    // packaged as one has its root as C:.
    let keep_root = chain.len() > 1 || keeps_its_root(path);
    // Each with its .dosc over it: the changes made to it for running it
    // (its setup's configuration files, DOS.YML).
    let mut stack = Stack { layers: Vec::new(), patched: BTreeMap::new(), hidden: Vec::new(), notes: RefCell::default() };
    let mut pairs = Vec::new();
    let doscs: Vec<Option<PathBuf>> = chain.iter().map(|archive| dosc_of(archive)).collect();
    // The launch configuration's folders on top, each patching its own
    // game; they are layers numbered from the archives' after them.
    let variant_layers: Vec<(PathBuf, usize)> = match variant {
        Some(_) => doscs.iter().enumerate().filter_map(|(i, d)| Some((d.clone()?, i))).collect(),
        None => Vec::new(),
    };
    for (dosc, _) in &variant_layers {
        stack.layers.push(Archive::open_part(dosc, true, variant)?);
    }
    let mut base_of = Vec::new();
    for (archive, dosc) in chain.iter().zip(&doscs) {
        if let Some(changes) = dosc {
            stack.layers.push(Archive::open(changes, true)?);
            pairs.push((stack.layers.len() - 1, stack.layers.len()));
        }
        stack.layers.push(Archive::open(archive, keep_root)?);
        base_of.push(stack.layers.len() - 1);
    }
    for (layer, (_, archive)) in variant_layers.iter().enumerate() {
        pairs.insert(0, (layer, base_of[*archive]));
    }
    // A .dosc patches its own game's files only; the parents' first, so a
    // child's patch of the same file wins.
    for (patch, base) in pairs.into_iter().rev() {
        stack.find_patches(patch, base);
    }
    // Patched now, so the sizes are known and what fails is told.
    let targets: Vec<String> = stack.patched.keys().cloned().collect();
    for target in targets {
        stack.patched(&target);
    }
    Ok(stack)
}

fn is_dosc(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("dosc"))
}

/// The .dosc beside the archive at `path`, with its name: `Game.dosc` for
/// `Game.dosz` (or `Game.zip`).
pub fn dosc_of(path: &Path) -> Option<PathBuf> {
    let stem = path.file_stem()?.to_string_lossy().to_lowercase();
    hostfs::read_dir(path.parent()?).ok()?.into_iter().find_map(|e| {
        let name = e.name.to_string_lossy().to_lowercase();
        (!e.is_dir && name.strip_suffix(".dosc") == Some(stem.as_str())).then_some(e.path)
    })
}

/// The archive a .dosc goes over: the .dosz (or .zip) with its name.
pub fn dosz_of(dosc: &Path) -> Option<PathBuf> {
    let stem = dosc.file_stem()?.to_string_lossy().to_lowercase();
    let entries = hostfs::read_dir(dosc.parent()?).ok()?;
    ["dosz", "zip"].iter().find_map(|ext| {
        entries.iter().find_map(|e| {
            let name = e.name.to_string_lossy().to_lowercase();
            (!e.is_dir && name == format!("{}.{}", stem, ext)).then(|| e.path.clone())
        })
    })
}

/// The DOS.YML files of the archive at `path` and of what goes with it
/// (its parents, its .dosc), in the order they apply: each one's keys over
/// the ones before.
pub fn dos_yml(path: &Path, variant: Option<&str>) -> Vec<String> {
    let Ok(stack) = open_variant(path, variant) else { return Vec::new() };
    stack
        .layers
        .iter()
        .rev()
        .filter_map(|archive| {
            let mut text = Vec::new();
            archive.open("DOS.YML").ok()?.read_to_end(&mut text).ok()?;
            Some(String::from_utf8_lossy(&text).into_owned())
        })
        .collect()
}

/// The launch configurations the .dosc files of the archive at `path`
/// (and of its parents) have: their `[...]` folders' names, the child's
/// first.
pub fn variants(path: &Path) -> Vec<String> {
    let mut chain = vec![path.to_path_buf()];
    while let Ok(Some(parent)) = parent_of(chain.last().expect("an archive")) {
        if chain.contains(&parent) || !hostfs::is_file(&parent) {
            break;
        }
        chain.push(parent);
    }
    let mut names: Vec<String> = Vec::new();
    for dosc in chain.iter().filter_map(|a| dosc_of(a)) {
        let Ok(mut file) = hostfs::File::open(&dosc) else { continue };
        let Ok(entries) = zip::central_directory(&mut file) else { continue };
        for entry in entries {
            let name = entry.name();
            let top = name.split('/').next().unwrap_or("");
            if let Some(inner) = top.strip_prefix('[').and_then(|t| t.strip_suffix(']'))
                && !names.iter().any(|n| n.eq_ignore_ascii_case(inner))
            {
                names.push(inner.to_string());
            }
        }
    }
    names
}

/// Whether the archive at `path` is packaged as a .dosz is: a .dosz, or
/// one with its `<name>.conf` or `<name>.dosc` beside it. Those have the
/// archive's
/// root as C: (and so the commands of the configuration too), where
/// rust-dos takes the one folder all the files are in.
pub fn keeps_its_root(path: &Path) -> bool {
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("dosz")) {
        return true;
    }
    let (Some(stem), Some(dir)) = (path.file_stem(), path.parent()) else { return false };
    let stem = stem.to_string_lossy().to_lowercase();
    hostfs::read_dir(dir).into_iter().flatten().any(|e| {
        let name = e.name.to_string_lossy().to_lowercase();
        !e.is_dir && [".conf", ".dosc"].iter().any(|ext| name.strip_suffix(ext) == Some(stem.as_str()))
    })
}

/// The parent archive a .dosz names, beside it.
fn parent_of(path: &Path) -> Result<Option<PathBuf>, String> {
    if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("dosz")) {
        return Ok(None);
    }
    let mut file = hostfs::File::open(path).map_err(|e| format!("{}: {}", path.display(), e))?;
    let entries = zip::central_directory(&mut file).map_err(|e| format!("{}: {}", path.display(), e))?;
    Ok(entries.iter().find_map(|e| {
        let name = e.name();
        (e.size == 0 && !e.is_dir()).then(|| parent_marker(&name).map(|p| path.with_file_name(p))).flatten()
    }))
}

/// A path into an archive, `<archive>/<path in it>`: the archive and the
/// path in it (from the root the drive has, `/`-separated). None if no
/// folder the path is in is an archive.
pub fn split(path: &Path) -> Option<(PathBuf, String)> {
    let mut archive = path.parent()?;
    loop {
        if is_archive_name(archive) && hostfs::is_file(archive) {
            let inner = path.strip_prefix(archive).ok()?;
            let inner: Vec<String> = inner.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
            return Some((archive.to_path_buf(), inner.join("/")));
        }
        archive = archive.parent()?;
    }
}

/// The contents of a file in an archive, by its path into the archive
/// (`split`). None if the path isn't into one.
pub fn read_member(path: &Path) -> Option<Result<Vec<u8>, String>> {
    let (archive, inner) = split(path)?;
    let read = || -> Result<Vec<u8>, String> {
        let mut data = Vec::new();
        let mut file = open(&archive)?.open(&inner).map_err(|e| format!("{}: {}", path.display(), e))?;
        file.read_to_end(&mut data).map_err(|e| format!("{}: {}", path.display(), e))?;
        Ok(data)
    };
    Some(read())
}

/// The program that starts the game among an archive's files (paths from
/// its root): the one program or batch file in its root, leaving out those
/// that set the game up or install it. None if there isn't exactly one.
pub fn start_program(files: &[String]) -> Option<String> {
    const NOT_GAMES: &[&str] = &["SETUP", "INSTALL", "INSTALLER", "CONFIG", "SETSOUND", "SOUND", "SETMAIN", "UNINST", "UNINSTALL", "README", "CATALOG"];
    let programs: Vec<&String> = files
        .iter()
        .filter(|f| !f.contains('/'))
        .filter(|name| {
            let upper = name.to_ascii_uppercase();
            let (stem, ext) = upper.rsplit_once('.').unwrap_or((&upper, ""));
            matches!(ext, "EXE" | "COM" | "BAT") && !NOT_GAMES.contains(&stem)
        })
        .collect();
    match programs.as_slice() {
        [one] => Some((*one).clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str, file: &str, data: &[u8]) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/test-archive").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(file), data).unwrap();
        dir.join(file)
    }

    fn read(lower: &dyn Lower, path: &str) -> Vec<u8> {
        let mut data = Vec::new();
        lower.open(path).unwrap().read_to_end(&mut data).unwrap();
        data
    }

    fn names(lower: &dyn Lower, path: &str) -> Vec<String> {
        let mut names: Vec<String> = lower.read_dir(path).unwrap().into_iter().map(|(n, _)| n.to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    #[test]
    fn a_zip_is_a_tree_from_its_common_folder() {
        let data = zip::tests::zip(&[
            ("KEEN/KEEN4E.EXE", b"MZ the game", true),
            ("KEEN/SETUP.EXE", b"MZ setup", false),
            ("KEEN/Data/Level1.dat", &[7u8; 5000], true),
            ("KEEN/../../evil", b"no", false),
        ]);
        let path = scratch("zip", "keen.zip", &data);
        let archive = Archive::open(&path, false).unwrap();
        assert_eq!(names(&archive, ""), ["Data", "KEEN4E.EXE", "SETUP.EXE"]);
        assert_eq!(names(&archive, "DATA"), ["Level1.dat"]);
        assert!(archive.metadata("data").unwrap().is_dir);
        assert_eq!(archive.metadata("Data/Level1.dat").unwrap().len, 5000);
        assert_eq!(read(&archive, "KEEN4E.EXE"), b"MZ the game");
        assert_eq!(read(&archive, "data/level1.dat"), vec![7u8; 5000]);
        // Stored: read where it is, and seekable.
        let mut setup = archive.open("SETUP.EXE").unwrap();
        setup.seek(SeekFrom::Start(3)).unwrap();
        let mut rest = String::new();
        setup.read_to_string(&mut rest).unwrap();
        assert_eq!((rest.as_str(), setup.len().unwrap()), ("setup", 8));
        assert!(setup.write(b"x").is_err());
        assert_eq!(start_program(&archive.files()).as_deref(), Some("KEEN4E.EXE"), "not SETUP");
        assert!(Archive::open(&scratch("bad", "bad.zip", b"not a zip"), false).is_err());
    }

    #[test]
    fn dosz_packages_keep_their_root() {
        let data = zip::tests::zip(&[("TYRIAN/TYRIAN.EXE", b"MZ", false), ("TYRIAN/SETUP.EXE", b"MZ", false)]);
        let path = scratch("dosz-root", "Tyrian.zip", &data);
        assert_eq!(names(&open(&path).unwrap(), ""), ["SETUP.EXE", "TYRIAN.EXE"], "a plain zip: its folder");
        std::fs::write(path.with_file_name("tyrian.CONF"), "[autoexec]\ncd tyrian\ntyrian\n").unwrap();
        assert_eq!(names(&open(&path).unwrap(), ""), ["TYRIAN"], "with its configuration beside it");
        std::fs::remove_file(path.with_file_name("tyrian.CONF")).unwrap();
        let dosz = path.with_extension("dosz");
        std::fs::rename(&path, &dosz).unwrap();
        assert_eq!(names(&open(&dosz).unwrap(), ""), ["TYRIAN"], "a .dosz");
    }

    #[test]
    fn a_dosc_goes_over_its_game() {
        let game = zip::tests::zip(&[("GAME.EXE", b"MZ", false), ("GAME.CFG", b"defaults", false), ("DOS.YML", b"cpu_year: 1990\r\n", false)]);
        let path = scratch("dosc", "Game (1990).dosz", &game);
        let changes = zip::tests::zip(&[
            ("GAME.CFG", b"set up", false),
            ("DOS.YML", b"run_path: C:\\GAME.EXE\r\n", false),
            ("[Setup Program]/DOS.YML", b"run_path: C:\\SETUP.EXE\r\n", false),
        ]);
        std::fs::write(path.with_extension("dosc"), &changes).unwrap();
        let stack = open(&path).unwrap();
        assert_eq!(read(&stack, "GAME.CFG"), b"set up");
        assert_eq!(names(&stack, ""), ["DOS.YML", "GAME.CFG", "GAME.EXE"], "no launch configurations");
        assert_eq!(dos_yml(&path, None), ["cpu_year: 1990\r\n", "run_path: C:\\GAME.EXE\r\n"]);
        assert_eq!(dosz_of(&path.with_extension("dosc")), Some(path.clone()));
    }

    #[test]
    fn a_dosc_patches_its_game_s_files() {
        let ips = |offset: u8, bytes: &[u8]| {
            let mut patch = b"PATCH".to_vec();
            patch.extend([0, 0, offset, 0, bytes.len() as u8]);
            patch.extend(bytes);
            patch.extend(b"EOF");
            patch
        };
        let game = zip::tests::zip(&[
            ("GAME.EXE", b"MZ protected", false),
            ("DATA/LEVEL.DAT", b"level one", false),
            ("SOUND.DAT", b"noise", false),
        ]);
        let path = scratch("dosc-patch", "Game.dosz", &game);
        let changes = zip::tests::zip(&[
            // A patch with the file's name, and one named after its file.
            ("GAME.EXE", &ips(3, b"cracked!!"), false),
            ("DATA/LEVEL.DAT.ips", &ips(6, b"two"), false),
            ("SOUND.DAT.xor", b"whatever", false),
        ]);
        std::fs::write(path.with_extension("dosc"), &changes).unwrap();
        let stack = open(&path).unwrap();
        assert_eq!(read(&stack, "GAME.EXE"), b"MZ cracked!!");
        assert_eq!(stack.metadata("GAME.EXE").unwrap().len, 12);
        assert_eq!(read(&stack, "data/level.dat"), b"level two");
        assert_eq!(names(&stack, "DATA"), ["LEVEL.DAT"], "the patch isn't shown");
        assert_eq!(read(&stack, "SOUND.DAT"), b"noise");
        let notes = stack.notes.take();
        assert_eq!(notes.len(), 1, "{:?}", notes);
        assert!(notes[0].contains("XOR"));
        assert!(!stack.files().iter().any(|f| f.ends_with(".ips")));
    }

    #[test]
    fn a_launch_configuration_goes_over_the_rest() {
        let game = zip::tests::zip(&[("GAME.EXE", b"MZ", false), ("GAME.CFG", b"defaults", false)]);
        let path = scratch("dosc-variant", "Game.dosz", &game);
        let changes = zip::tests::zip(&[
            ("GAME.CFG", b"set up", false),
            ("DOS.YML", b"run_path: C:\\GAME.EXE\r\n", false),
            ("[2#Setup Program]/DOS.YML", b"run_path: C:\\SETUP.EXE\r\nrun_utility: true\r\n", false),
            ("[2#Setup Program]/SETUP.EXE", b"MZ setup", false),
            ("[MIDI]/GAME.CFG", b"midi", false),
        ]);
        std::fs::write(path.with_extension("dosc"), &changes).unwrap();
        assert_eq!(variants(&path), ["2#Setup Program", "MIDI"]);
        let midi = open_variant(&path, Some("midi")).unwrap();
        assert_eq!(read(&midi, "GAME.CFG"), b"midi");
        let setup = open_variant(&path, Some("2#Setup Program")).unwrap();
        assert_eq!(read(&setup, "GAME.CFG"), b"set up");
        assert_eq!(names(&setup, ""), ["DOS.YML", "GAME.CFG", "GAME.EXE", "SETUP.EXE"]);
        let yml = dos_yml(&path, Some("2#Setup Program"));
        assert_eq!(yml.len(), 2);
        assert!(yml[1].contains("SETUP.EXE"), "the launch configuration's last: {:?}", yml);
    }

    #[test]
    fn paths_go_into_archives() {
        let data = zip::tests::zip(&[("GAME/EXTRAS/Manual.pdf", b"%PDF", false), ("GAME/GAME.EXE", b"MZ", false)]);
        let path = scratch("split", "game.zip", &data);
        assert_eq!(split(&path.join("EXTRAS/Manual.pdf")), Some((path.clone(), "EXTRAS/Manual.pdf".to_string())));
        assert_eq!(split(&path), None);
        assert_eq!(read_member(&path.join("extras/manual.pdf")), Some(Ok(b"%PDF".to_vec())));
        assert!(read_member(&path.join("NONE.PDF")).unwrap().is_err());
        assert_eq!(read_member(&path.with_file_name("other.pdf")), None);
    }

    #[test]
    fn a_dosz_goes_over_its_parent() {
        let parent = zip::tests::zip(&[("GAME.EXE", b"MZ old", false), ("DATA.DAT", b"data", false)]);
        let path = scratch("dosz", "base.dosz", &parent);
        let child = zip::tests::zip(&[("base.dosz.parent", b"", false), ("GAME.EXE", b"MZ patched", false), ("MOD.DAT", b"mod", false)]);
        let child_path = path.with_file_name("mod.dosz");
        std::fs::write(&child_path, child).unwrap();
        let stack = open(&child_path).unwrap();
        assert_eq!(names(&stack, ""), ["DATA.DAT", "GAME.EXE", "MOD.DAT"]);
        assert_eq!(read(&stack, "GAME.EXE"), b"MZ patched");
        assert_eq!(read(&stack, "DATA.DAT"), b"data");
    }

    #[cfg(feature = "sevenz")]
    #[test]
    fn a_7z_is_read_by_its_blocks() {
        let data = sevenz::tests::sevenz(&[("GAME/A.TXT", b"first"), ("GAME/SUB/B.TXT", &[9; 3000])]);
        let path = scratch("7z", "game.7z", &data);
        let archive = Archive::open(&path, false).unwrap();
        assert_eq!(names(&archive, ""), ["A.TXT", "SUB"]);
        assert_eq!(read(&archive, "SUB/B.TXT"), vec![9; 3000]);
        assert_eq!(read(&archive, "A.TXT"), b"first");
        assert_eq!(archive.cache.borrow().files.len(), 2, "the block's files are kept together");
    }
}
