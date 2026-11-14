//! STGMEDIUMs: ReleaseStgMedium. A STGMEDIUM is 12 bytes: the type
//! (TYMED_), what it holds (a global handle, a GDI object, a file name,
//! an interface), and an object that owns that (pUnkForRelease), or 0.
//!
//! What it frees follows the table in the documentation of
//! ReleaseStgMedium and Wine's ole32/ole2.c (LGPL 2.1 or later):
//!
//! Copyright 1995 Martin von Loewis
//! Copyright 1999 Francis Beaudet
//! Copyright 1999 Noel Borthwick

use super::task_free;
use crate::cpu::Cpu;
use crate::win32::{gdi32, kernel32};

const TYMED_NULL: u32 = 0;
const TYMED_HGLOBAL: u32 = 1;
const TYMED_FILE: u32 = 2;
const TYMED_ISTREAM: u32 = 4;
const TYMED_ISTORAGE: u32 = 8;
const TYMED_GDI: u32 = 16;
const TYMED_MFPICT: u32 = 32;
const TYMED_ENHMF: u32 = 64;

const STGMEDIUM: u32 = 12;
/// METAFILEPICT's metafile handle, after its mapping mode and extents.
const MFP_HMF: u32 = 12;
/// IUnknown::Release's place in a vtable.
const RELEASE: u32 = 2;

/// ReleaseStgMedium(medium): what the medium holds is freed as its type
/// says: a global handle, a GDI object or a metafile picture only when no
/// object owns it, a file deleted only then and its name freed either way,
/// a stream or storage released. Then the type is TYMED_NULL and the
/// owning object, if there is one, is released and cleared.
///
/// What it reads of the program's memory it reads before it frees
/// anything: a missing page runs it again from the start.
pub fn release_stg_medium(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[0];
    if at == 0 || !cpu.bus.guest_probe(at, STGMEDIUM as usize, true) {
        return Some(0);
    }
    let tymed = cpu.bus.guest_read_32(at);
    let handle = cpu.bus.guest_read_32(at + 4);
    let owner = cpu.bus.guest_read_32(at + 8);
    let own = owner == 0 && handle != 0;
    let name = (tymed == TYMED_FILE && own).then(|| wide_name(cpu, handle));
    let metafile = if tymed == TYMED_MFPICT && own {
        let picture = kernel32::global_address(cpu, handle);
        (picture != 0).then(|| cpu.bus.guest_read_32(picture + MFP_HMF))
    } else {
        None
    };
    if cpu.bus.guest_faulted() {
        return None;
    }
    let mut releases = Vec::new();
    match tymed {
        TYMED_HGLOBAL if own => kernel32::global_free(cpu, handle),
        TYMED_FILE if handle != 0 => {
            if let Some(name) = name {
                let _ = kernel32::files::delete_file(cpu, &name);
            }
            task_free(cpu, handle);
        }
        TYMED_ISTREAM | TYMED_ISTORAGE if handle != 0 => {
            if let Some(release) = super::stream::method(cpu, handle, RELEASE) {
                releases.push((release, handle));
            }
        }
        TYMED_GDI if own => {
            gdi32::delete_object(cpu, &[handle]);
        }
        TYMED_MFPICT if own => {
            if let Some(metafile) = metafile {
                gdi32::metafile::delete_meta_file(cpu, &[metafile]);
            }
            kernel32::global_free(cpu, handle);
        }
        // GDI32 here makes no enhanced metafiles, so the handle is another
        // object's or none, and DeleteEnhMetaFile fails for those without
        // deleting anything.
        TYMED_ENHMF => {}
        _ => {}
    }
    cpu.bus.guest_write_32(at, TYMED_NULL);
    if owner != 0 {
        if let Some(release) = super::stream::method(cpu, owner, RELEASE) {
            releases.push((release, owner));
        }
        cpu.bus.guest_write_32(at + 8, 0);
    }
    super::propvariant::call_all(cpu, 0, &releases)
}

/// The file name at `at`, a Unicode string, in the ANSI code page.
fn wide_name(cpu: &mut Cpu, at: u32) -> String {
    let mut units = Vec::new();
    for i in 0..260u32 {
        let c = cpu.bus.guest_read_16(at + 2 * i);
        if c == 0 || cpu.bus.guest_faulted() {
            break;
        }
        units.push(c);
    }
    crate::dosstr::from_bytes(&kernel32::wide_to_ansi(cpu, &units))
}
