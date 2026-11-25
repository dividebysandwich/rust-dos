//! Windows 3.1: what its 16-bit GDI and TOOLHELP gave programs that
//! Windows 98's don't. Programs made for Windows 3.1, and the 16-bit
//! DLLs of Win32s programs ([`super::win32s`]), sometimes read GDI's
//! own data, which Windows 98 keeps elsewhere.
//!
//! From GDI.EXE and TOOLHELP.DLL of Windows 3.11 (FileVersion 3.11, on
//! its install disks):
//!
//! - TOOLHELP's SystemHeapInfo (1:11D4) checks that dwSize is 0Ch and
//!   fills hUserSegment (+08h) and hGDISegment (+0Ah) with USER's and
//!   GDI's data segments, the local heaps their objects are in.
//! - A GDI object's handle is a local handle of that heap: it is the
//!   offset of a handle table entry, whose first word is the offset of
//!   the object, the second its flags and lock count (0 for an unlocked
//!   object). GDI goes to an object through it (1:508A, `mov bx,
//!   es:[bx]`).
//! - Every object starts with a header of 0Ah bytes: the next object's
//!   handle (+00h, 0), a magic (+02h; 4F4Bh for a bitmap, checked at
//!   1:4FCF), a serial number from a count GDI keeps (+04h, a dword)
//!   and the object's metafile data (+08h, 0).
//! - CreateBitmap (1:4929) goes on to CreateBitmapIndirect's code
//!   (1:0BCB), which allocates the bitmap object at 1:0D28, 1Ch bytes,
//!   and fills it at 1:0D39: the header, then at +0Ah the global handle
//!   of a block with the device's BITMAP (GMEM_DDESHARE, 24h bytes more
//!   than the bits), +0Ch to +13h zero, +14h the handle of a block of
//!   its own for bits that don't fit in that one's 64K (0 otherwise),
//!   +16h and +18h a device's data (0 for a bitmap of GDI's), and +1Ah
//!   its flags (1: its bits cross 64K).
//! - The device's BITMAP, at 1:0C9D: bmType (+00h, 0), bmWidth (+02h),
//!   bmHeight (+04h), bmWidthBytes (+06h), bmPlanes (+08h, a byte),
//!   bmBitsPixel (+09h, a byte), bmBits (+0Ah, 16:16), bmWidthPlanes
//!   (+0Eh, a dword: bmWidthBytes times bmHeight), bmlpPDevice (+12h,
//!   0), bmSegmentIndex (+16h, __AHINCR for bits that cross 64K, else
//!   0), bmScanSegment (+18h, the rows in 64K), bmFillBytes (+1Ah, the
//!   bytes left over in 64K) and two zero words. The bits follow it at
//!   +20h.
//! - GDI locks a bitmap (1:0F95) with GlobalLock of +0Ah, and +14h when
//!   it is there, and writes bmBits with the bits' address: +20h of the
//!   block, or the bits' own block. Selecting it into a DC locks it; the
//!   display driver's BitBlt then reads the device's BITMAP as it is,
//!   bmBits too, so a program that changes it between the two (Lunicus's
//!   LUNIFX.DLL, which points bmBits at each band of its frame and sets
//!   the size) has BitBlt draw from what it wrote.
//!
//! The shim ([`super::Shim::Win31GdiHeap`]) keeps such a view of GDI's
//! heap for the process: [`gdi_heap`].

pub mod gdi_heap;

use super::{Shim, Shims};

/// The shims of Windows 3.1, for programs made for it.
pub const WIN31: Shims = Shims::of(&[Shim::Win31GdiHeap]);
