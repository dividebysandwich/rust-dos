//! Win32s: the Win32 subset Windows 3.1 ran 32-bit programs with. Its
//! KERNEL32 thunked most functions down to its 16-bit WIN32S16.DLL, which
//! called Windows 3.1's own KERNEL, so programs made for it see some
//! answers of Windows 3.1 that Windows 95 and 98 don't give. Windows 98
//! runs Win32s programs as Win32 ones, and those that relied on such an
//! answer fail there (its SYSTEM.DAT lists several as incompatible).
//!
//! What Win32s did, function by function, from Win32s 1.15a (FileVersion
//! 1.15.103, `\WIN32S` on the Lunicus CD):
//!
//! - GlobalFlags: W32SCOMB.DLL's export thunks to WIN32S16.DLL (index
//!   60h). WIN32S16's export 109, at 1:829A, turns the handle into a
//!   16-bit one, calls KERNEL.GlobalFlags (KERNEL.22, its only call in
//!   the DLL) and clears AL before it returns: the lock count, the low
//!   byte, is always 0, and the flags in the high byte are Windows 3.1's.
//!   Windows 98's GlobalFlags (KERNEL32 BFF9A0F9) returns the count that
//!   its GlobalLock (BFF814A8) raises. Cyberflix's engine (Lunicus, Jump
//!   Raven) checks the count before each lock and stops with "bad lock
//!   use count" when it is odd. LocalFlags is a 32-bit function of
//!   W32SCOMB's own and isn't changed.
//! - UTLinearToSelectorOffset: WIN32S16's export 45 (1:6414) returns
//!   the selector of one of the thread's two last 32 KB blocks if the
//!   address is in one (FS:144 to FS:14C), and a 32 KB block's selector
//!   for an address in the thread's stack (FS:8 to FS:4). For any other
//!   address it asks entry 132 (1:7464), which looks the address up in
//!   its table of the program's memory (a table for each 4 MB at
//!   DS:1A06, each entry the region's base at +4 and the first of its
//!   tiled selectors at +0Ch); entry 158 (1:68B7) gives the selector of
//!   the 64 KB tile the address is in, `sel + ((lin - base) >> 16 <<
//!   __AHSHIFT)`, and the offset is `(lin - base) & 0FFFFh`. Only an
//!   address in no region gets a 32 KB block's selector (`and
//!   eax,0FFFF8000h`). Windows 98's WIN32S16 (2:037A) has no table of the
//!   program's memory: it gives a selector from KERNEL.494 (KRNL386
//!   1:40D1), which looks in a list of ranges of its own, or MapLS's of
//!   the 32 KB block. Cyberflix's LUNIFX.DLL (Lunicus) blits its frame
//!   in bands of up to 64 KB of rows from each band's pointer, and on a
//!   32 KB block's selector a band's rows run past the 64 KB the selector
//!   has. The thunk's translation list keeps the 32 KB blocks: which
//!   path Win32s takes for it isn't read out.
//!
//! A Win32s program runs on Windows 3.1 and can call its 16-bit DLLs,
//! which see Windows 3.1 itself, so Win32s's set has Windows 3.1's
//! ([`super::win31`]) too.

use super::{Shim, Shims};

/// The shims of Win32s, for programs made for it: its own and Windows
/// 3.1's.
pub const WIN32S: Shims = Shims::of(&[Shim::GlobalFlagsWithoutLockCount, Shim::UtRegionSelectors])
    .union(super::win31::WIN31);
