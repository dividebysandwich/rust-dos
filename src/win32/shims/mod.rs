//! Compatibility shims: API behaviours of an older Windows that some
//! programs need and Windows 98 no longer has. The built-in Windows
//! behaves as Windows 98 does for every program but those a game fix
//! names (`game_fixes::Kind::Win32Shims`): each of them gets a set of
//! [`Shim`]s when it starts, which lives on its process, so other
//! programs keep Windows 98's behaviour.
//!
//! A shim is checked at the one function whose behaviour it changes,
//! where a comment points back here. The sets of the systems the shims
//! come from are in their own files: [`win32s::WIN32S`] for programs
//! made for Win32s on Windows 3.1.
//!
//! To add a shim: a variant of [`Shim`] with the next bit, its check in
//! the function it changes, a test that runs a program with and without
//! it (`tests/win32/win32_shims_tests.rs`), and the system's set.

pub mod win32s;

/// The W32 section's version that added each process's shims.
pub const SHIMS_SAVED: u16 = 93;

/// An API behaviour that differs from Windows 98's for some programs.
/// The value is the shim's bit in a [`Shims`] set and in save states:
/// don't renumber them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Shim {
    /// GlobalFlags gives every block a lock count of 0, as Win32s's did
    /// ([`win32s`]); GlobalLock and GlobalUnlock still count.
    GlobalFlagsWithoutLockCount = 0,
    /// UTLinearToSelectorOffset gives an address in a region of the
    /// program's memory a selector of the region's own tiled ones, as
    /// Win32s's did ([`win32s`]), not that of a 32 KB block.
    UtRegionSelectors = 1,
}

impl Shim {
    /// Every shim, in the order of their bits.
    pub const ALL: &[Shim] = &[Shim::GlobalFlagsWithoutLockCount, Shim::UtRegionSelectors];

    /// What the shim does, in a few words, for the log.
    pub fn describe(self) -> &'static str {
        match self {
            Shim::GlobalFlagsWithoutLockCount => "GlobalFlags without the lock count",
            Shim::UtRegionSelectors => "UTLinearToSelectorOffset with region selectors",
        }
    }

    const fn bit(self) -> u32 {
        1 << self as u8
    }
}

/// A set of shims, one bit each.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Shims {
    bits: u32,
}

impl Shims {
    /// No shims: Windows 98's behaviour.
    pub const NONE: Shims = Shims { bits: 0 };

    /// The set of `shims`.
    pub const fn of(shims: &[Shim]) -> Shims {
        let mut bits = 0;
        let mut i = 0;
        while i < shims.len() {
            bits |= shims[i].bit();
            i += 1;
        }
        Shims { bits }
    }

    /// Whether the set has `shim`.
    pub fn has(self, shim: Shim) -> bool {
        self.bits & shim.bit() != 0
    }

    pub fn is_empty(self) -> bool {
        self.bits == 0
    }

    /// The shims of both sets.
    pub fn union(self, other: Shims) -> Shims {
        Shims {
            bits: self.bits | other.bits,
        }
    }

    /// The shims in the set, in the order of their bits.
    pub fn iter(self) -> impl Iterator<Item = Shim> {
        Shim::ALL.iter().copied().filter(move |&s| self.has(s))
    }
}

crate::state_fields!(Shims { bits });

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_follow_the_list() {
        for (i, shim) in Shim::ALL.iter().enumerate() {
            assert_eq!(*shim as usize, i, "{shim:?}");
        }
    }

    #[test]
    fn a_set_has_what_it_was_made_of() {
        let set = Shims::of(&[Shim::GlobalFlagsWithoutLockCount]);
        assert!(set.has(Shim::GlobalFlagsWithoutLockCount));
        assert!(!Shims::NONE.has(Shim::GlobalFlagsWithoutLockCount));
        assert!(Shims::NONE.is_empty());
        assert_eq!(Shims::NONE.union(set), set);
        let listed: Vec<_> = set.iter().collect();
        assert_eq!(listed, [Shim::GlobalFlagsWithoutLockCount]);
    }
}
