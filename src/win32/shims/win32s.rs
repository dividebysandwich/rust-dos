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
//!
//! A Win32s program runs on Windows 3.1 and can call its 16-bit DLLs,
//! which see Windows 3.1 itself, so Win32s's set has Windows 3.1's
//! ([`super::win31`]) too.

use super::{Shim, Shims};

/// The shims of Win32s, for programs made for it: its own and Windows
/// 3.1's.
pub const WIN32S: Shims =
    Shims::of(&[Shim::GlobalFlagsWithoutLockCount]).union(super::win31::WIN31);
