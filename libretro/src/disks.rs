//! The frontend's disk control: which disk of a list is in the content's
//! drive (floppy or CD), opening the drive and putting another in, and
//! adding disks to the list. Ctrl+F4 and the settings window change the
//! same drive, so the frontend always sees it as it is.

use std::ffi::{CStr, c_char, c_void};
use std::path::{Path, PathBuf};

use crate::ffi::*;
use crate::{Callbacks, INITIAL_IMAGE, with_core};

/// Tell the frontend the disk control's functions.
pub fn declare(cb: &Callbacks) {
    static EXT: retro_disk_control_ext_callback = retro_disk_control_ext_callback {
        set_eject_state,
        get_eject_state,
        get_image_index,
        set_image_index,
        get_num_images,
        replace_image_index,
        add_image_index,
        set_initial_image,
        get_image_path,
        get_image_label,
    };
    let mut version = 0u32;
    // SAFETY: the calls take what libretro.h says; the version 0 struct is
    // the start of the extended one.
    unsafe {
        let ext = cb.env(RETRO_ENVIRONMENT_GET_DISK_CONTROL_INTERFACE_VERSION, &mut version as *mut u32 as *mut c_void)
            && version >= 1;
        let cmd = if ext { RETRO_ENVIRONMENT_SET_DISK_CONTROL_EXT_INTERFACE } else { RETRO_ENVIRONMENT_SET_DISK_CONTROL_INTERFACE };
        cb.env(cmd, &EXT as *const _ as *mut c_void);
    }
}

unsafe extern "C" fn set_eject_state(ejected: bool) -> bool {
    with_core(|core| {
        if core.disk.ejected == ejected {
            return true;
        }
        core.disk.ejected = ejected;
        if ejected {
            return true;
        }
        // Closed with another disk picked: that disk is in.
        let (Some(drive), Some(index)) = (core.disk.drive, core.disk.pending.take()) else { return true };
        match core.m.cpu.bus.select_image(drive, index) {
            Ok(Some(message)) => {
                core.m.notices.push(message);
                true
            }
            Ok(None) => true,
            Err(e) => {
                core.m.notices.push(e);
                false
            }
        }
    })
    .unwrap_or(false)
}

unsafe extern "C" fn get_eject_state() -> bool {
    with_core(|core| core.disk.ejected).unwrap_or(false)
}

unsafe extern "C" fn get_image_index() -> u32 {
    with_core(|core| core.disk.pending.unwrap_or_else(|| core.disk_images().1) as u32).unwrap_or(0)
}

unsafe extern "C" fn set_image_index(index: u32) -> bool {
    with_core(|core| {
        if !core.disk.ejected {
            return false;
        }
        core.disk.pending = Some(index as usize);
        true
    })
    .unwrap_or(false)
}

unsafe extern "C" fn get_num_images() -> u32 {
    with_core(|core| (core.disk_images().0.len() + core.disk.placeholders) as u32).unwrap_or(0)
}

/// Put the image `info` has at `index`: in a place `add_image_index` made,
/// the image is added. Without `info`, the image at `index` is taken out
/// of the list.
unsafe extern "C" fn replace_image_index(index: u32, info: *const retro_game_info) -> bool {
    // SAFETY: the frontend hands over a retro_game_info, or none.
    let path = unsafe { info.as_ref() }.filter(|i| !i.path.is_null()).map(|i| {
        PathBuf::from(unsafe { CStr::from_ptr(i.path) }.to_string_lossy().into_owned())
    });
    with_core(|core| {
        let index = index as usize;
        let count = core.disk_images().0.len();
        let result = match (path, index < count) {
            (None, true) => {
                let Some(drive) = core.disk.drive else { return false };
                core.m.cpu.bus.disk.remove_image(drive, index)
            }
            (None, false) => {
                core.disk.placeholders = core.disk.placeholders.saturating_sub(1);
                Ok(())
            }
            (Some(path), false) => core.add_disk_image(&path).map(|()| {
                core.disk.placeholders = core.disk.placeholders.saturating_sub(1);
            }),
            // An image in the list can't change places.
            (Some(path), true) => {
                let same = core.disk_images().0.get(index).cloned() == std::fs::canonicalize(&path).ok();
                if same { Ok(()) } else { Err("A disk of the list can't be replaced".to_string()) }
            }
        };
        result.map_err(|e| core.m.notices.push(e)).is_ok()
    })
    .unwrap_or(false)
}

unsafe extern "C" fn add_image_index() -> bool {
    with_core(|core| core.disk.placeholders += 1).is_some()
}

/// The disk the content is to start with, as the frontend remembers it:
/// kept for `retro_load_game`, which the frontend calls next.
unsafe extern "C" fn set_initial_image(index: u32, path: *const c_char) -> bool {
    if path.is_null() {
        return false;
    }
    // SAFETY: the frontend hands over a C string.
    let path = PathBuf::from(unsafe { CStr::from_ptr(path) }.to_string_lossy().into_owned());
    INITIAL_IMAGE.with(|i| *i.borrow_mut() = Some((index as usize, path)));
    true
}

fn copy_out(text: &str, s: *mut c_char, len: usize) -> bool {
    if s.is_null() || len == 0 {
        return false;
    }
    let bytes = text.as_bytes();
    let n = bytes.len().min(len - 1);
    // SAFETY: the frontend hands over `len` bytes to write a C string in.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), s as *mut u8, n);
        *s.add(n) = 0;
    }
    true
}

fn image(index: u32) -> Option<PathBuf> {
    with_core(|core| core.disk_images().0.get(index as usize).cloned()).flatten()
}

unsafe extern "C" fn get_image_path(index: u32, s: *mut c_char, len: usize) -> bool {
    image(index).is_some_and(|path| copy_out(&path.to_string_lossy(), s, len))
}

unsafe extern "C" fn get_image_label(index: u32, s: *mut c_char, len: usize) -> bool {
    image(index).is_some_and(|path| {
        let name = Path::new(&path).file_name().map_or_else(|| path.to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned());
        copy_out(&name, s, len)
    })
}
