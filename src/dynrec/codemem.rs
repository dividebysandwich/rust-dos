//! Executable memory for translated code: one region reserved up front,
//! filled from the start and emptied all at once when it is full.
//!
//! Where the host allows it, the region is readable, writable and
//! executable at once. Where it doesn't (SELinux's execmem, OpenBSD), it
//! is executable and `write` makes the pages it writes writable for the
//! copy. On Apple Silicon the region is mapped with MAP_JIT, whose pages
//! the thread switches between writable and executable.

use std::io;

/// Code is placed at multiples of this.
const ALIGN: usize = 16;

pub struct CodeMemory {
    base: *mut u8,
    size: usize,
    /// Bytes in use from the start.
    used: usize,
    /// Bytes at the start that `clear` keeps (the trampoline).
    kept: usize,
    /// The pages can't be writable and executable at once. (On Apple
    /// Silicon the thread switches MAP_JIT pages instead.)
    #[cfg_attr(all(target_os = "macos", target_arch = "aarch64"), allow(dead_code))]
    toggle: bool,
}

impl CodeMemory {
    /// Reserve `size` bytes.
    pub fn new(size: usize) -> io::Result<Self> {
        let (base, toggle) = map(size)?;
        Ok(CodeMemory { base, size, used: 0, kept: 0, toggle })
    }

    /// Place `bytes` in the next free space and return where they went,
    /// or None if they don't fit.
    pub fn add(&mut self, bytes: &[u8]) -> Option<*const u8> {
        let at = self.used.next_multiple_of(ALIGN);
        if at + bytes.len() > self.size {
            return None;
        }
        // SAFETY: at..at+len is inside the mapping, and no translated code
        // runs while we write (the execution loop is in Rust here).
        unsafe {
            let dest = self.base.add(at);
            self.writable(dest, bytes.len(), true);
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), dest, bytes.len());
            self.writable(dest, bytes.len(), false);
            dynasmrt::cache_control::synchronize_icache(std::slice::from_raw_parts(dest, bytes.len()));
        }
        self.used = at + bytes.len();
        Some(unsafe { self.base.add(at) as *const u8 })
    }

    /// Keep what has been added so far through `clear`.
    pub fn keep(&mut self) {
        self.kept = self.used;
    }

    /// Forget everything added after `keep`.
    pub fn clear(&mut self) {
        self.used = self.kept;
    }

    /// Bytes in use.
    pub fn used(&self) -> usize {
        self.used
    }

    /// Make `len` bytes at `at` writable (or executable again).
    unsafe fn writable(&self, at: *mut u8, len: usize, write: bool) {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        unsafe {
            let _ = (at, len);
            libc::pthread_jit_write_protect_np(if write { 0 } else { 1 });
        }
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        if self.toggle {
            unsafe { protect(at, len, write) };
        }
    }
}

impl Drop for CodeMemory {
    fn drop(&mut self) {
        unsafe { unmap(self.base, self.size) };
    }
}

/// The page range covering `len` bytes at `at`.
#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
fn page_span(at: *mut u8, len: usize) -> (*mut u8, usize) {
    const PAGE: usize = 4096;
    let start = at as usize & !(PAGE - 1);
    let end = (at as usize + len).next_multiple_of(PAGE);
    (start as *mut u8, end - start)
}

#[cfg(unix)]
fn map(size: usize) -> io::Result<(*mut u8, bool)> {
    use libc::{MAP_ANONYMOUS, MAP_FAILED, MAP_PRIVATE, PROT_EXEC, PROT_READ, PROT_WRITE};
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    let flags = MAP_PRIVATE | MAP_ANONYMOUS | libc::MAP_JIT;
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    let flags = MAP_PRIVATE | MAP_ANONYMOUS;
    // SAFETY: an anonymous mapping at an address of the kernel's choice.
    unsafe {
        let p = libc::mmap(std::ptr::null_mut(), size, PROT_READ | PROT_WRITE | PROT_EXEC, flags, -1, 0);
        if p != MAP_FAILED {
            return Ok((p as *mut u8, false));
        }
        let p = libc::mmap(std::ptr::null_mut(), size, PROT_READ | PROT_EXEC, flags, -1, 0);
        if p == MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok((p as *mut u8, true))
    }
}

#[cfg(all(unix, not(all(target_os = "macos", target_arch = "aarch64"))))]
unsafe fn protect(at: *mut u8, len: usize, write: bool) {
    use libc::{PROT_EXEC, PROT_READ, PROT_WRITE};
    let (start, len) = page_span(at, len);
    let prot = if write { PROT_READ | PROT_WRITE } else { PROT_READ | PROT_EXEC };
    let ok = unsafe { libc::mprotect(start as *mut libc::c_void, len, prot) } == 0;
    assert!(ok, "mprotect of translated code failed: {}", io::Error::last_os_error());
}

#[cfg(unix)]
unsafe fn unmap(base: *mut u8, size: usize) {
    unsafe { libc::munmap(base as *mut libc::c_void, size) };
}

#[cfg(windows)]
fn map(size: usize) -> io::Result<(*mut u8, bool)> {
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE, VirtualAlloc,
    };
    // SAFETY: a fresh allocation at an address of the system's choice.
    unsafe {
        let p = VirtualAlloc(std::ptr::null(), size, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE);
        if !p.is_null() {
            return Ok((p as *mut u8, false));
        }
        let p = VirtualAlloc(std::ptr::null(), size, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READ);
        if p.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok((p as *mut u8, true))
    }
}

#[cfg(windows)]
unsafe fn protect(at: *mut u8, len: usize, write: bool) {
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READ, PAGE_READWRITE, VirtualProtect};
    let (start, len) = page_span(at, len);
    let mut old = 0;
    let prot = if write { PAGE_READWRITE } else { PAGE_EXECUTE_READ };
    let ok = unsafe { VirtualProtect(start as *const _, len, prot, &mut old) } != 0;
    assert!(ok, "VirtualProtect of translated code failed: {}", io::Error::last_os_error());
}

#[cfg(windows)]
unsafe fn unmap(base: *mut u8, _size: usize) {
    use windows_sys::Win32::System::Memory::{MEM_RELEASE, VirtualFree};
    unsafe { VirtualFree(base as *mut _, 0, MEM_RELEASE) };
}
