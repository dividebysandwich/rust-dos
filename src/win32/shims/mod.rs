//! Compatibility shims: API behaviours of an older Windows that some
//! programs need and Windows 98 no longer has. The built-in Windows
//! behaves as Windows 98 does for every program but those a game fix
//! names (`game_fixes::Kind::Win32Shims`) and, with the `win32_shims`
//! setting, every program a run starts: each of them gets a set of
//! [`Shim`]s when it starts, which lives on its process, so other
//! programs keep Windows 98's behaviour.
//!
//! A shim is checked at the functions whose behaviour it changes, where
//! a comment points back here. The sets of the systems the shims come
//! from are in their own files, which say what the system did:
//! [`win31::WIN31`] for programs made for Windows 3.1, and
//! [`win32s::WIN32S`] for programs made for Win32s on Windows 3.1, which
//! has Windows 3.1's too. A shim that keeps state keeps it on the
//! process, in its system's folder (`win31/gdi_heap.rs`).
//!
//! To add a shim: a variant of [`Shim`] with the next bit, its check in
//! the function it changes, a test that runs a program with and without
//! it (`tests/win32/win32_shims_tests.rs`), and the system's set.

pub mod win31;
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
    /// TOOLHELP's SystemHeapInfo gives GDI's heap, where each bitmap's
    /// object leads to its device BITMAP, which BitBlt then draws from,
    /// as Windows 3.1's did ([`win31`]).
    Win31GdiHeap = 1,
    /// UTLinearToSelectorOffset gives an address in a region of the
    /// program's memory a selector of the region's own tiled ones, as
    /// Win32s's did ([`win32s`]), not that of a 32 KB block.
    UtRegionSelectors = 2,
}

impl Shim {
    /// Every shim, in the order of their bits.
    pub const ALL: &[Shim] = &[
        Shim::GlobalFlagsWithoutLockCount,
        Shim::Win31GdiHeap,
        Shim::UtRegionSelectors,
    ];

    /// What the shim does, in a few words, for the log.
    pub fn describe(self) -> &'static str {
        match self {
            Shim::GlobalFlagsWithoutLockCount => "GlobalFlags without the lock count",
            Shim::Win31GdiHeap => "Windows 3.1's GDI heap",
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
    pub const fn union(self, other: Shims) -> Shims {
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

/// The sets the `win32_shims` setting names, by their names.
pub const SETS: &[(&str, Shims)] = &[
    ("none", Shims::NONE),
    ("win31", win31::WIN31),
    ("win32s", win32s::WIN32S),
];

/// The set called `name` (any case), for the `win32_shims` setting.
pub fn by_name(name: &str) -> Option<Shims> {
    let name = name.trim();
    SETS.iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|&(_, set)| set)
}

/// The name of `set`, if it is one of [`SETS`].
pub fn name_of(set: Shims) -> Option<&'static str> {
    SETS.iter().find(|&&(_, s)| s == set).map(|&(n, _)| n)
}

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

    #[test]
    fn win32s_has_windows_3_1s_shims() {
        for shim in win31::WIN31.iter() {
            assert!(win32s::WIN32S.has(shim), "{shim:?}");
        }
        assert!(win32s::WIN32S.has(Shim::GlobalFlagsWithoutLockCount));
        assert!(!win31::WIN31.has(Shim::GlobalFlagsWithoutLockCount));
    }

    #[test]
    fn sets_go_by_their_names() {
        assert_eq!(by_name("Win31"), Some(win31::WIN31));
        assert_eq!(by_name(" win32s "), Some(win32s::WIN32S));
        assert_eq!(by_name("none"), Some(Shims::NONE));
        assert_eq!(by_name("win95"), None);
        for &(name, set) in SETS {
            assert_eq!(name_of(set), Some(name));
        }
    }
}
