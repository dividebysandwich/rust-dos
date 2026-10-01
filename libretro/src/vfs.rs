//! The frontend's files (its VFS), for the paths `std::fs` can't open:
//! Android's `saf://` and `content://` paths, of the folders and files the
//! system's file picker lets RetroArch at. rust-dos's `hostfs` hands the
//! paths with a scheme to it; other paths stay the host's own.

use std::ffi::{CStr, CString, OsString, c_void};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;

use rust_dos::hostfs::{self, Backend, Handle, Meta, Mode};

use crate::Callbacks;
use crate::ffi::*;

/// The version with folders and `stat`.
const VERSION: u32 = 3;

/// Ask the frontend for its files, and hand the paths with a scheme to
/// them. Without version 3, those paths are the host's.
pub fn install(cb: &Callbacks) {
    let mut info = retro_vfs_interface_info { required_interface_version: VERSION, iface: std::ptr::null_mut() };
    // SAFETY: GET_VFS_INTERFACE takes a retro_vfs_interface_info.
    let ok = unsafe { cb.env(RETRO_ENVIRONMENT_GET_VFS_INTERFACE, &mut info as *mut _ as *mut c_void) };
    // SAFETY: the frontend's table, of version 3 or later, lives as long
    // as the core is loaded; it is copied.
    let iface = if ok { unsafe { info.iface.as_ref() }.copied() } else { None };
    let Some(iface) = iface else {
        hostfs::set_backend(None);
        return;
    };
    match Vfs::new(iface) {
        Some(vfs) => {
            hostfs::set_backend(Some(Arc::new(vfs)));
            cb.log(RETRO_LOG_INFO, &format!("Using the frontend's files (VFS {}) for paths with a scheme", VERSION));
        }
        None => hostfs::set_backend(None),
    }
}

/// The functions of version 3 (truncate is version 2's).
#[derive(Clone, Copy)]
struct Fns {
    open: unsafe extern "C" fn(*const std::ffi::c_char, u32, u32) -> *mut retro_vfs_file_handle,
    close: unsafe extern "C" fn(*mut retro_vfs_file_handle) -> i32,
    size: unsafe extern "C" fn(*mut retro_vfs_file_handle) -> i64,
    tell: unsafe extern "C" fn(*mut retro_vfs_file_handle) -> i64,
    seek: unsafe extern "C" fn(*mut retro_vfs_file_handle, i64, i32) -> i64,
    read: unsafe extern "C" fn(*mut retro_vfs_file_handle, *mut c_void, u64) -> i64,
    write: unsafe extern "C" fn(*mut retro_vfs_file_handle, *const c_void, u64) -> i64,
    flush: unsafe extern "C" fn(*mut retro_vfs_file_handle) -> i32,
    remove: unsafe extern "C" fn(*const std::ffi::c_char) -> i32,
    rename: unsafe extern "C" fn(*const std::ffi::c_char, *const std::ffi::c_char) -> i32,
    truncate: unsafe extern "C" fn(*mut retro_vfs_file_handle, i64) -> i64,
    stat: unsafe extern "C" fn(*const std::ffi::c_char, *mut i32) -> i32,
    mkdir: unsafe extern "C" fn(*const std::ffi::c_char) -> i32,
    opendir: unsafe extern "C" fn(*const std::ffi::c_char, bool) -> *mut retro_vfs_dir_handle,
    readdir: unsafe extern "C" fn(*mut retro_vfs_dir_handle) -> bool,
    dirent_get_name: unsafe extern "C" fn(*mut retro_vfs_dir_handle) -> *const std::ffi::c_char,
    dirent_is_dir: unsafe extern "C" fn(*mut retro_vfs_dir_handle) -> bool,
    closedir: unsafe extern "C" fn(*mut retro_vfs_dir_handle) -> i32,
}

pub struct Vfs(Fns);

impl Vfs {
    /// The frontend's functions, if it has them all.
    pub fn new(i: retro_vfs_interface) -> Option<Self> {
        Some(Vfs(Fns {
            open: i.open?,
            close: i.close?,
            size: i.size?,
            tell: i.tell?,
            seek: i.seek?,
            read: i.read?,
            write: i.write?,
            flush: i.flush?,
            remove: i.remove?,
            rename: i.rename?,
            truncate: i.truncate?,
            stat: i.stat?,
            mkdir: i.mkdir?,
            opendir: i.opendir?,
            readdir: i.readdir?,
            dirent_get_name: i.dirent_get_name?,
            dirent_is_dir: i.dirent_is_dir?,
            closedir: i.closedir?,
        }))
    }

    /// What `stat` says of `path`: its flags and size.
    fn stat(&self, path: &CStr) -> (i32, i32) {
        let mut size = 0i32;
        // SAFETY: a C string and an int32_t to put the size in.
        let flags = unsafe { (self.0.stat)(path.as_ptr(), &mut size) };
        (flags, size)
    }

    /// An error for a call on `path` that failed: whether it isn't there.
    fn error(&self, path: &CStr, what: &str) -> io::Error {
        if self.stat(path).0 & RETRO_VFS_STAT_IS_VALID == 0 {
            io::Error::new(io::ErrorKind::NotFound, format!("{}: not found", path.to_string_lossy()))
        } else {
            io::Error::other(format!("{}: can't {}", path.to_string_lossy(), what))
        }
    }

    fn open_raw(&self, path: &CStr, mode: u32) -> Option<VfsFile> {
        // SAFETY: a C string and the access flags.
        let handle = unsafe { (self.0.open)(path.as_ptr(), mode, RETRO_VFS_FILE_ACCESS_HINT_NONE) };
        (!handle.is_null()).then_some(VfsFile { fns: self.0, handle })
    }
}

/// The path as the frontend takes it.
fn c_path(path: &Path) -> io::Result<CString> {
    CString::new(hostfs::backend_path(path)).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))
}

impl Backend for Vfs {
    fn open(&self, path: &Path, mode: Mode) -> io::Result<Box<dyn Handle>> {
        let c = c_path(path)?;
        let file = if !mode.write {
            self.open_raw(&c, RETRO_VFS_FILE_ACCESS_READ)
        } else if mode.truncate {
            // WRITE alone makes the file, or empties it.
            self.open_raw(&c, RETRO_VFS_FILE_ACCESS_WRITE)
        } else {
            // Kept as it is; made first if it isn't there and may be.
            let rw = RETRO_VFS_FILE_ACCESS_READ_WRITE | RETRO_VFS_FILE_ACCESS_UPDATE_EXISTING;
            self.open_raw(&c, rw).or_else(|| {
                let missing = self.stat(&c).0 & RETRO_VFS_STAT_IS_VALID == 0;
                if !(mode.create && missing) {
                    return None;
                }
                drop(self.open_raw(&c, RETRO_VFS_FILE_ACCESS_WRITE)?);
                self.open_raw(&c, rw)
            })
        };
        match file {
            Some(file) => Ok(Box::new(file)),
            None => Err(self.error(&c, "open it")),
        }
    }

    fn metadata(&self, path: &Path) -> io::Result<Meta> {
        let c = c_path(path)?;
        let (flags, size) = self.stat(&c);
        if flags & RETRO_VFS_STAT_IS_VALID == 0 {
            return Err(io::Error::new(io::ErrorKind::NotFound, format!("{}: not found", path.display())));
        }
        let is_dir = flags & RETRO_VFS_STAT_IS_DIRECTORY != 0;
        // stat's size is 32 bits; a file of 2 GB or more says its size open.
        let len = match size {
            _ if is_dir => 0,
            0.. if size < i32::MAX => size as u64,
            _ => self.open_raw(&c, RETRO_VFS_FILE_ACCESS_READ).and_then(|mut f| f.len().ok()).unwrap_or(0),
        };
        Ok(Meta { is_dir, len, modified: None, readonly: false })
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<(OsString, bool)>> {
        let c = c_path(path)?;
        // SAFETY: a C string; the hidden files too, as the host's.
        let dir = unsafe { (self.0.opendir)(c.as_ptr(), true) };
        if dir.is_null() {
            return Err(self.error(&c, "list it"));
        }
        let mut entries = Vec::new();
        // SAFETY: the handle is open until closedir; each name is the
        // frontend's until the next readdir, and is copied.
        unsafe {
            while (self.0.readdir)(dir) {
                let name = (self.0.dirent_get_name)(dir);
                if name.is_null() {
                    continue;
                }
                let name = CStr::from_ptr(name).to_string_lossy().into_owned();
                // Some frontends give the entry's path rather than its name.
                let name = name.rsplit('/').next().unwrap_or(&name).to_string();
                entries.push((OsString::from(name), (self.0.dirent_is_dir)(dir)));
            }
            (self.0.closedir)(dir);
        }
        Ok(entries)
    }

    fn create_dir(&self, path: &Path) -> io::Result<()> {
        let c = c_path(path)?;
        // SAFETY: a C string.
        match unsafe { (self.0.mkdir)(c.as_ptr()) } {
            0 => Ok(()),
            -2 => Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{}: already there", path.display()))),
            _ => Err(io::Error::other(format!("{}: can't make it", path.display()))),
        }
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        let c = c_path(path)?;
        // SAFETY: a C string.
        match unsafe { (self.0.remove)(c.as_ptr()) } {
            0 => Ok(()),
            _ => Err(self.error(&c, "delete it")),
        }
    }

    fn remove_dir(&self, path: &Path) -> io::Result<()> {
        self.remove_file(path)
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        let (c_from, c_to) = (c_path(from)?, c_path(to)?);
        // SAFETY: two C strings.
        match unsafe { (self.0.rename)(c_from.as_ptr(), c_to.as_ptr()) } {
            0 => Ok(()),
            _ => Err(self.error(&c_from, "rename it")),
        }
    }
}

/// A file the frontend has open, closed when dropped.
struct VfsFile {
    fns: Fns,
    handle: *mut retro_vfs_file_handle,
}

// SAFETY: the handle is the frontend's, used through `&mut self` alone
// (hostfs holds it behind a lock), and libretro's VFS doesn't tie a
// handle to the thread that opened it.
unsafe impl Send for VfsFile {}
unsafe impl Sync for VfsFile {}

impl Drop for VfsFile {
    fn drop(&mut self) {
        // SAFETY: the handle is open, and closed once.
        unsafe { (self.fns.close)(self.handle) };
    }
}

fn failed(what: &str) -> io::Error {
    io::Error::other(format!("the frontend's file can't {}", what))
}

impl Read for VfsFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // SAFETY: the handle is open, and `buf` takes `buf.len()` bytes.
        let n = unsafe { (self.fns.read)(self.handle, buf.as_mut_ptr() as *mut c_void, buf.len() as u64) };
        if n < 0 { Err(failed("be read")) } else { Ok(n as usize) }
    }
}

impl Write for VfsFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // SAFETY: the handle is open, and `buf` holds `buf.len()` bytes.
        let n = unsafe { (self.fns.write)(self.handle, buf.as_ptr() as *const c_void, buf.len() as u64) };
        if n < 0 { Err(failed("be written")) } else { Ok(n as usize) }
    }

    fn flush(&mut self) -> io::Result<()> {
        // SAFETY: the handle is open.
        match unsafe { (self.fns.flush)(self.handle) } {
            0 => Ok(()),
            _ => Err(failed("be flushed")),
        }
    }
}

impl Seek for VfsFile {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let (offset, whence) = match pos {
            SeekFrom::Start(n) => (n as i64, RETRO_VFS_SEEK_POSITION_START),
            SeekFrom::Current(n) => (n, RETRO_VFS_SEEK_POSITION_CURRENT),
            SeekFrom::End(n) => (n, RETRO_VFS_SEEK_POSITION_END),
        };
        // Frontends answer a seek with the position or with 0, so the
        // position comes from tell.
        // SAFETY: the handle is open.
        let at = unsafe {
            if (self.fns.seek)(self.handle, offset, whence) < 0 {
                return Err(failed("seek there"));
            }
            (self.fns.tell)(self.handle)
        };
        if at < 0 { Err(failed("tell where it is")) } else { Ok(at as u64) }
    }
}

impl Handle for VfsFile {
    fn len(&mut self) -> io::Result<u64> {
        // SAFETY: the handle is open.
        let n = unsafe { (self.fns.size)(self.handle) };
        if n < 0 { Err(failed("say its size")) } else { Ok(n as u64) }
    }

    fn set_len(&mut self, len: u64) -> io::Result<()> {
        // SAFETY: the handle is open.
        match unsafe { (self.fns.truncate)(self.handle, len as i64) } {
            0 => Ok(()),
            _ => Err(failed("change its size")),
        }
    }
}
