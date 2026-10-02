//! The host's files, as the drives, images and content reach them: through
//! `std::fs`, or, for paths `std::fs` can't open, through a backend the
//! embedding program installs. The libretro core installs the frontend's
//! VFS there, for Android's `saf://` and `content://` paths.
//!
//! A path is the backend's when it starts with a scheme, `name://`, and a
//! backend is installed on the thread. Others go to `std::fs` as before.
//! The backend's files have no dates or read-only flags; `modified` is
//! None for them, and setting either does nothing.
//!
//! A layer (`add_layer`) is a backend of its own, with a root of its own,
//! `layerN:/`: a drive's write overlay (`overlay`) is one. Its paths go
//! to it whatever backend the thread has.

use std::cell::RefCell;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

/// How a file is opened, as `std::fs::OpenOptions` has it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mode {
    pub read: bool,
    pub write: bool,
    pub create: bool,
    pub truncate: bool,
}

/// What a path is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Meta {
    pub is_dir: bool,
    pub len: u64,
    /// None where the backend doesn't know.
    pub modified: Option<SystemTime>,
    pub readonly: bool,
}

impl Meta {
    pub fn is_file(&self) -> bool {
        !self.is_dir
    }
}

/// An entry of a folder.
#[derive(Clone, Debug)]
pub struct DirEntry {
    pub name: OsString,
    pub path: PathBuf,
    /// Whether it is a folder; a link to one isn't (`metadata` follows
    /// links).
    pub is_dir: bool,
}

impl DirEntry {
    pub fn file_name(&self) -> OsString {
        self.name.clone()
    }

    pub fn path(&self) -> PathBuf {
        self.path.clone()
    }

    pub fn metadata(&self) -> io::Result<Meta> {
        metadata(&self.path)
    }
}

/// A file the backend opened.
#[allow(clippy::len_without_is_empty)]
pub trait Handle: Read + Write + Seek + Send + Sync {
    fn len(&mut self) -> io::Result<u64>;
    fn set_len(&mut self, len: u64) -> io::Result<()>;
    /// The file's date, where the backend has one.
    fn modified(&mut self) -> Option<SystemTime> {
        None
    }
    fn set_modified(&mut self, _time: SystemTime) -> io::Result<()> {
        Ok(())
    }
}

/// The files of the paths with a scheme.
pub trait Backend {
    fn open(&self, path: &Path, mode: Mode) -> io::Result<Box<dyn Handle>>;
    fn metadata(&self, path: &Path) -> io::Result<Meta>;
    /// The names in a folder, and which are folders.
    fn read_dir(&self, path: &Path) -> io::Result<Vec<(OsString, bool)>>;
    fn create_dir(&self, path: &Path) -> io::Result<()>;
    fn remove_file(&self, path: &Path) -> io::Result<()>;
    fn remove_dir(&self, path: &Path) -> io::Result<()>;
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;
    fn set_readonly(&self, _path: &Path, _readonly: bool) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "read-only flags aren't kept here"))
    }
}

thread_local! {
    static BACKEND: RefCell<Option<Arc<dyn Backend>>> = const { RefCell::new(None) };
    /// The layers, by their scheme (`layerN`).
    static LAYERS: RefCell<Vec<(String, Arc<dyn Backend>)>> = const { RefCell::new(Vec::new()) };
}

/// The numbers of the layers' schemes, which aren't used again.
static NEXT_LAYER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);

/// A layer on this thread: its paths are under `root`, and go to its
/// backend until it is dropped.
pub struct Layer {
    scheme: String,
    root: PathBuf,
}

impl Layer {
    /// `layerN:/`, the root of the layer's paths.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl std::fmt::Debug for Layer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.scheme)
    }
}

impl Drop for Layer {
    fn drop(&mut self) {
        let _ = LAYERS.try_with(|layers| layers.borrow_mut().retain(|(scheme, _)| *scheme != self.scheme));
    }
}

/// Hand the paths under a new root to `backend`.
pub fn add_layer(backend: Arc<dyn Backend>) -> Layer {
    let scheme = format!("layer{}", NEXT_LAYER.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    LAYERS.with(|layers| layers.borrow_mut().push((scheme.clone(), backend)));
    Layer { root: PathBuf::from(format!("{}:/", scheme)), scheme }
}

/// The path under a layer's root, `/`-separated, without the root: ""
/// for the root itself.
pub fn layer_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    let rest = s.split_once(':').map_or(&*s, |(_, rest)| rest);
    rest.split(['/', '\\']).filter(|c| !c.is_empty() && *c != ".").collect::<Vec<_>>().join("/")
}

/// Install the backend for paths with a scheme on this thread, or (None)
/// take it out.
pub fn set_backend(backend: Option<Arc<dyn Backend>>) {
    BACKEND.with(|b| *b.borrow_mut() = backend);
}

/// Whether `path` starts with a scheme: two or more letters, then `:/`
/// (Windows' `C:\` is one letter). Path handling makes `//` one `/`, so
/// `saf:/x` is one too.
pub fn has_scheme(path: &Path) -> bool {
    let s = path.as_os_str().as_encoded_bytes();
    let Some(colon) = s.iter().position(|&c| c == b':') else { return false };
    colon >= 2
        && s[0].is_ascii_alphabetic()
        && s[..colon].iter().all(|&c| c.is_ascii_alphanumeric() || b"+.-".contains(&c))
        && s.get(colon + 1) == Some(&b'/')
}

/// The path as the backend takes it: `scheme://rest`, whatever became of
/// the `//`.
pub fn backend_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    match s.split_once(':') {
        Some((scheme, rest)) if has_scheme(path) => format!("{}://{}", scheme, rest.trim_start_matches('/')),
        _ => s.into_owned(),
    }
}

/// The backend `path` goes to, if it isn't `std::fs`'s.
fn backend(path: &Path) -> Option<Arc<dyn Backend>> {
    if !has_scheme(path) {
        return None;
    }
    let s = path.as_os_str().as_encoded_bytes();
    let scheme = &s[..s.iter().position(|&c| c == b':').unwrap_or(0)];
    if scheme.starts_with(b"layer") {
        let layer = LAYERS.with(|layers| {
            layers.borrow().iter().find(|(name, _)| name.as_bytes() == scheme).map(|(_, b)| b.clone())
        });
        if layer.is_some() {
            return layer;
        }
    }
    BACKEND.with(|b| b.borrow().clone())
}

/// Whether `path` is under a layer's root (`layerN:/`).
pub fn is_layer(path: &Path) -> bool {
    has_scheme(path) && path.as_os_str().as_encoded_bytes().starts_with(b"layer")
}

/// Whether `path` is the backend's.
pub fn is_foreign(path: &Path) -> bool {
    backend(path).is_some()
}

/// An open file. As with `std::fs::File`, a `&File` reads, writes and
/// seeks too.
pub enum File {
    Std(fs::File),
    Foreign(Mutex<Box<dyn Handle>>),
}

impl std::fmt::Debug for File {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            File::Std(file) => file.fmt(f),
            File::Foreign(_) => f.write_str("File::Foreign"),
        }
    }
}

#[allow(clippy::len_without_is_empty)]
impl File {
    pub fn open(path: impl AsRef<Path>) -> io::Result<File> {
        OpenOptions::new().read(true).open(path)
    }

    pub fn create(path: impl AsRef<Path>) -> io::Result<File> {
        OpenOptions::new().write(true).create(true).truncate(true).open(path)
    }

    /// The backend's handle, held.
    fn handle(file: &Mutex<Box<dyn Handle>>) -> MutexGuard<'_, Box<dyn Handle>> {
        file.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn len(&self) -> io::Result<u64> {
        match self {
            File::Std(file) => file.metadata().map(|m| m.len()),
            File::Foreign(file) => Self::handle(file).len(),
        }
    }

    pub fn set_len(&self, len: u64) -> io::Result<()> {
        match self {
            File::Std(file) => file.set_len(len),
            File::Foreign(file) => Self::handle(file).set_len(len),
        }
    }

    pub fn modified(&self) -> Option<SystemTime> {
        match self {
            File::Std(file) => file.metadata().and_then(|m| m.modified()).ok(),
            File::Foreign(file) => Self::handle(file).modified(),
        }
    }

    pub fn set_modified(&self, time: SystemTime) -> io::Result<()> {
        match self {
            File::Std(file) => file.set_modified(time),
            File::Foreign(file) => Self::handle(file).set_modified(time),
        }
    }

    pub fn sync_all(&self) -> io::Result<()> {
        match self {
            File::Std(file) => file.sync_all(),
            File::Foreign(file) => Self::handle(file).flush(),
        }
    }
}

impl Read for &File {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            File::Std(file) => (&*file).read(buf),
            File::Foreign(file) => File::handle(file).read(buf),
        }
    }
}

impl Write for &File {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            File::Std(file) => (&*file).write(buf),
            File::Foreign(file) => File::handle(file).write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            File::Std(file) => (&*file).flush(),
            File::Foreign(file) => File::handle(file).flush(),
        }
    }
}

impl Seek for &File {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        match self {
            File::Std(file) => (&*file).seek(pos),
            File::Foreign(file) => File::handle(file).seek(pos),
        }
    }
}

/// A file is a backend's handle too, for a backend over other files.
impl Handle for File {
    fn len(&mut self) -> io::Result<u64> {
        File::len(self)
    }
    fn set_len(&mut self, len: u64) -> io::Result<()> {
        File::set_len(self, len)
    }
    fn modified(&mut self) -> Option<SystemTime> {
        File::modified(self)
    }
    fn set_modified(&mut self, time: SystemTime) -> io::Result<()> {
        File::set_modified(self, time)
    }
}

impl Read for File {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (&*self).read(buf)
    }
}

impl Write for File {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (&*self).write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        (&*self).flush()
    }
}

impl Seek for File {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        (&*self).seek(pos)
    }
}

/// `std::fs::OpenOptions`, for both kinds of path.
#[derive(Clone, Copy, Debug, Default)]
pub struct OpenOptions(Mode);

impl OpenOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn read(&mut self, on: bool) -> &mut Self {
        self.0.read = on;
        self
    }

    pub fn write(&mut self, on: bool) -> &mut Self {
        self.0.write = on;
        self
    }

    pub fn create(&mut self, on: bool) -> &mut Self {
        self.0.create = on;
        self
    }

    pub fn truncate(&mut self, on: bool) -> &mut Self {
        self.0.truncate = on;
        self
    }

    pub fn open(&self, path: impl AsRef<Path>) -> io::Result<File> {
        let path = path.as_ref();
        match backend(path) {
            Some(backend) => backend.open(path, self.0).map(|h| File::Foreign(Mutex::new(h))),
            None => fs::OpenOptions::new()
                .read(self.0.read)
                .write(self.0.write)
                .create(self.0.create)
                .truncate(self.0.truncate)
                .open(path)
                .map(File::Std),
        }
    }
}

fn std_meta(m: fs::Metadata) -> Meta {
    Meta { is_dir: m.is_dir(), len: m.len(), modified: m.modified().ok(), readonly: m.permissions().readonly() }
}

pub fn metadata(path: impl AsRef<Path>) -> io::Result<Meta> {
    let path = path.as_ref();
    match backend(path) {
        Some(backend) => backend.metadata(path),
        None => fs::metadata(path).map(std_meta),
    }
}

/// The entries of a folder, in no order.
pub fn read_dir(path: impl AsRef<Path>) -> io::Result<Vec<DirEntry>> {
    let path = path.as_ref();
    match backend(path) {
        Some(backend) => Ok(backend
            .read_dir(path)?
            .into_iter()
            .filter(|(name, _)| name != "." && name != "..")
            .map(|(name, is_dir)| DirEntry { path: path.join(&name), name, is_dir })
            .collect()),
        None => Ok(fs::read_dir(path)?
            .filter_map(Result::ok)
            .map(|e| {
                let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
                DirEntry { name: e.file_name(), path: e.path(), is_dir }
            })
            .collect()),
    }
}

pub fn is_dir(path: impl AsRef<Path>) -> bool {
    metadata(path).is_ok_and(|m| m.is_dir)
}

pub fn is_file(path: impl AsRef<Path>) -> bool {
    metadata(path).is_ok_and(|m| !m.is_dir)
}

pub fn exists(path: impl AsRef<Path>) -> bool {
    metadata(path).is_ok()
}

pub fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    let path = path.as_ref();
    if backend(path).is_none() {
        return fs::read(path);
    }
    let mut data = Vec::new();
    File::open(path)?.read_to_end(&mut data)?;
    Ok(data)
}

pub fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
    String::from_utf8(read(path)?).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

pub fn write(path: impl AsRef<Path>, data: impl AsRef<[u8]>) -> io::Result<()> {
    let path = path.as_ref();
    if backend(path).is_none() {
        return fs::write(path, data);
    }
    let mut file = File::create(path)?;
    file.write_all(data.as_ref())?;
    file.flush()
}

pub fn create_dir(path: impl AsRef<Path>) -> io::Result<()> {
    let path = path.as_ref();
    match backend(path) {
        Some(backend) => backend.create_dir(path),
        None => fs::create_dir(path),
    }
}

pub fn create_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
    let path = path.as_ref();
    if backend(path).is_none() {
        return fs::create_dir_all(path);
    }
    if is_dir(path) {
        return Ok(());
    }
    if let Some(parent) = path.parent().filter(|p| has_scheme(p)) {
        create_dir_all(parent)?;
    }
    match create_dir(path) {
        Err(_) if is_dir(path) => Ok(()),
        result => result,
    }
}

pub fn remove_file(path: impl AsRef<Path>) -> io::Result<()> {
    let path = path.as_ref();
    match backend(path) {
        Some(backend) => backend.remove_file(path),
        None => fs::remove_file(path),
    }
}

pub fn remove_dir(path: impl AsRef<Path>) -> io::Result<()> {
    let path = path.as_ref();
    match backend(path) {
        Some(backend) => backend.remove_dir(path),
        None => fs::remove_dir(path),
    }
}

/// Rename a file or folder. Where the backend can't (Android's SAF
/// can't), a file is copied to the new name and the old one deleted.
pub fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<()> {
    let (from, to) = (from.as_ref(), to.as_ref());
    let Some(backend) = backend(from).or_else(|| backend(to)) else { return fs::rename(from, to) };
    let error = match backend.rename(from, to) {
        Ok(()) => return Ok(()),
        Err(e) => e,
    };
    if !is_file(from) {
        return Err(error);
    }
    copy(from, to)?;
    remove_file(from).inspect_err(|_| {
        let _ = remove_file(to);
    })
}

/// Copy a file, returning the bytes copied.
pub fn copy(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<u64> {
    let (from, to) = (from.as_ref(), to.as_ref());
    if backend(from).is_none() && backend(to).is_none() {
        return fs::copy(from, to);
    }
    let mut source = File::open(from)?;
    let mut target = File::create(to)?;
    let n = io::copy(&mut source, &mut target)?;
    target.flush()?;
    Ok(n)
}

/// `std::fs::canonicalize`; the backend's paths are as they are.
pub fn canonicalize(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    let path = path.as_ref();
    match backend(path) {
        Some(backend) => backend.metadata(path).map(|_| path.to_path_buf()),
        None => fs::canonicalize(path),
    }
}

/// `std::path::absolute`; a path with a scheme is as it is.
pub fn absolute(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    let path = path.as_ref();
    if has_scheme(path) { Ok(path.to_path_buf()) } else { std::path::absolute(path) }
}

/// Make a file read-only or writable; the backend's stay as they are.
pub fn set_readonly(path: impl AsRef<Path>, readonly: bool) -> io::Result<()> {
    let path = path.as_ref();
    if let Some(backend) = backend(path) {
        return backend.set_readonly(path, readonly);
    }
    let mut permissions = fs::metadata(path)?.permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(readonly);
    fs::set_permissions(path, permissions)
}

/// Remove a folder and everything in it.
pub fn remove_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
    let path = path.as_ref();
    if backend(path).is_none() {
        return fs::remove_dir_all(path);
    }
    for entry in read_dir(path)? {
        if entry.is_dir { remove_dir_all(&entry.path)? } else { remove_file(&entry.path)? }
    }
    remove_dir(path)
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A backend over a folder: `mem://a/b` is `<root>/a/b`. It can't
    /// rename, as SAF can't, and has no dates.
    pub struct Mapped(pub PathBuf);

    struct MappedFile(fs::File);

    impl Read for MappedFile {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.0.read(buf)
        }
    }
    impl Write for MappedFile {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.write(buf)
        }
        fn flush(&mut self) -> io::Result<()> {
            self.0.flush()
        }
    }
    impl Seek for MappedFile {
        fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
            self.0.seek(pos)
        }
    }
    impl Handle for MappedFile {
        fn len(&mut self) -> io::Result<u64> {
            self.0.metadata().map(|m| m.len())
        }
        fn set_len(&mut self, len: u64) -> io::Result<()> {
            self.0.set_len(len)
        }
    }

    impl Mapped {
        fn host(&self, path: &Path) -> PathBuf {
            let s = backend_path(path);
            self.0.join(s.strip_prefix("mem://").expect("a mem:// path"))
        }
    }

    impl Backend for Mapped {
        fn open(&self, path: &Path, mode: Mode) -> io::Result<Box<dyn Handle>> {
            let file = fs::OpenOptions::new()
                .read(mode.read)
                .write(mode.write)
                .create(mode.create)
                .truncate(mode.truncate)
                .open(self.host(path))?;
            Ok(Box::new(MappedFile(file)))
        }
        fn metadata(&self, path: &Path) -> io::Result<Meta> {
            let m = fs::metadata(self.host(path))?;
            Ok(Meta { is_dir: m.is_dir(), len: m.len(), modified: None, readonly: false })
        }
        fn read_dir(&self, path: &Path) -> io::Result<Vec<(OsString, bool)>> {
            Ok(fs::read_dir(self.host(path))?
                .filter_map(Result::ok)
                .map(|e| (e.file_name(), e.path().is_dir()))
                .collect())
        }
        fn create_dir(&self, path: &Path) -> io::Result<()> {
            fs::create_dir(self.host(path))
        }
        fn remove_file(&self, path: &Path) -> io::Result<()> {
            fs::remove_file(self.host(path))
        }
        fn remove_dir(&self, path: &Path) -> io::Result<()> {
            fs::remove_dir(self.host(path))
        }
        fn rename(&self, _from: &Path, _to: &Path) -> io::Result<()> {
            Err(io::Error::new(io::ErrorKind::Unsupported, "no rename"))
        }
    }

    /// A scratch folder with a `Mapped` backend over it installed.
    pub fn mapped(name: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/test-hostfs").join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        set_backend(Some(Arc::new(Mapped(dir.clone()))));
        dir
    }

    #[test]
    fn schemes_are_told_from_drive_letters() {
        assert!(has_scheme(Path::new("saf://tree/a")));
        assert!(has_scheme(Path::new("saf:/tree/a")));
        assert!(has_scheme(Path::new("content://com.android/x")));
        assert!(!has_scheme(Path::new("C:/games")));
        assert!(!has_scheme(Path::new("C:\\games")));
        assert!(!has_scheme(Path::new("/home/user/games")));
        assert!(!has_scheme(Path::new("games/a:b")));
        assert_eq!(backend_path(Path::new("saf://t%2Fx").join("GAME").as_path()), "saf://t%2Fx/GAME");
        let collapsed: PathBuf = Path::new("saf://t/GAME").components().collect();
        assert_eq!(backend_path(&collapsed), "saf://t/GAME");
    }

    #[test]
    fn paths_with_a_scheme_go_to_the_backend() {
        let dir = mapped("dispatch");
        create_dir_all("mem://a/b").unwrap();
        assert!(dir.join("a/b").is_dir());
        write("mem://a/b/FILE.TXT", "hello").unwrap();
        assert_eq!(read_to_string("mem://a/b/FILE.TXT").unwrap(), "hello");
        let meta = metadata("mem://a/b/FILE.TXT").unwrap();
        assert_eq!((meta.is_dir, meta.len, meta.modified), (false, 5, None));
        let names: Vec<_> = read_dir("mem://a/b").unwrap().into_iter().map(|e| e.path).collect();
        assert_eq!(names, [PathBuf::from("mem://a/b/FILE.TXT")]);

        let mut file = OpenOptions::new().read(true).write(true).open("mem://a/b/FILE.TXT").unwrap();
        file.seek(SeekFrom::End(0)).unwrap();
        file.write_all(b", world").unwrap();
        file.set_len(9).unwrap();
        assert_eq!(file.len().unwrap(), 9);
        drop(file);
        assert_eq!(fs::read_to_string(dir.join("a/b/FILE.TXT")).unwrap(), "hello, wo");
        assert_eq!(canonicalize("mem://a/b").unwrap(), PathBuf::from("mem://a/b"));
        assert_eq!(absolute("mem://a/b").unwrap(), PathBuf::from("mem://a/b"));
        assert!(set_readonly("mem://a/b/FILE.TXT", true).is_err());

        // Other paths are std::fs's.
        assert!(is_dir(&dir));
        assert!(!exists("mem://nothing"));
    }

    #[test]
    fn renames_the_backend_cant_do_copy_the_file() {
        let dir = mapped("rename");
        write("mem://OLD.TXT", "data").unwrap();
        rename("mem://OLD.TXT", "mem://NEW.TXT").unwrap();
        assert!(!dir.join("OLD.TXT").exists());
        assert_eq!(fs::read_to_string(dir.join("NEW.TXT")).unwrap(), "data");
        create_dir("mem://SUB").unwrap();
        assert!(rename("mem://SUB", "mem://SUB2").is_err(), "folders aren't copied");
    }

    #[test]
    fn without_a_backend_paths_with_a_scheme_are_std_fs_paths() {
        set_backend(None);
        assert!(!is_foreign(Path::new("mem://a")));
        assert!(!exists("mem://a"));
    }
}
