//! Values that say when they changed, for what is derived from them
//! and kept: the card's state for a draw (`d3d::hal::toadforce::bind`)
//! is derived from a device's states and from the surfaces, and is the
//! same for the next draw unless one of them changed.
//!
//! A `Stamped<T>` holds a value and a stamp, a number no other value
//! in the process has or had. Reading goes through `Deref` and leaves
//! the stamp. Every way to change the value goes through `DerefMut`,
//! which gives it a new stamp first: an assignment to a field, a
//! method that takes `&mut self`, a loaded save state. So what a
//! reader derived from the value under one stamp is still right for as
//! long as the value has that stamp, and code that changes the value
//! can't forget to say so, since it has no other way in.
//!
//! The stamp changes when the value is borrowed to change, whether or
//! not it then changes: a writer that often writes the value already
//! there compares first and leaves it alone (`opengl32::draw::apply`).
//!
//! Stamps come from one counter for the process and are never saved:
//! a loaded value has a new one. They are no part of the machine, and
//! nothing the program can see depends on them.

use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicU64, Ordering};

/// The next stamp. It starts at 1, so 0 is no value's.
static NEXT: AtomicU64 = AtomicU64::new(1);

fn fresh() -> u64 {
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// A value with a stamp that changes whenever the value may have.
#[derive(Debug)]
pub struct Stamped<T> {
    value: T,
    stamp: u64,
}

impl<T> Stamped<T> {
    pub fn new(value: T) -> Stamped<T> {
        Stamped {
            value,
            stamp: fresh(),
        }
    }

    /// The stamp of the value as it is now. An associated function, as
    /// the two below, so that it hides no method of the value's.
    pub fn stamp(this: &Stamped<T>) -> u64 {
        this.stamp
    }

    /// The value to change, with its stamp kept: for a change to a part
    /// of it that nothing derives anything from under the stamp, or
    /// that its readers check by another way. Say which at the call.
    pub fn unstamped(this: &mut Stamped<T>) -> &mut T {
        &mut this.value
    }
}

impl<T> Deref for Stamped<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.value
    }
}

impl<T> DerefMut for Stamped<T> {
    fn deref_mut(&mut self) -> &mut T {
        self.stamp = fresh();
        &mut self.value
    }
}

impl<T: Default> Default for Stamped<T> {
    fn default() -> Stamped<T> {
        Stamped::new(T::default())
    }
}

/// A copy has the value of what it was copied from, and its stamp with
/// it, until one of the two changes.
impl<T: Clone> Clone for Stamped<T> {
    fn clone(&self) -> Stamped<T> {
        Stamped {
            value: self.value.clone(),
            stamp: self.stamp,
        }
    }
}

impl<T: crate::savestate::State> crate::savestate::State for Stamped<T> {
    fn save(&self, w: &mut crate::savestate::Writer) {
        self.value.save(w);
    }

    fn load(&mut self, r: &mut crate::savestate::Reader) -> crate::savestate::Result<()> {
        let value: &mut T = self;
        value.load(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_keeps_the_stamp_and_a_write_changes_it() {
        let mut v = Stamped::new(vec![1, 2, 3]);
        let first = Stamped::stamp(&v);
        assert_eq!(v[1], 2);
        assert_eq!(v.len(), 3);
        assert_eq!(Stamped::stamp(&v), first);
        v[1] = 5;
        let second = Stamped::stamp(&v);
        assert_ne!(second, first);
        v.push(4);
        assert_ne!(Stamped::stamp(&v), second);
    }

    #[test]
    fn a_copy_has_the_stamp_until_it_changes() {
        let v = Stamped::new(7u32);
        let mut copy = v.clone();
        assert_eq!(Stamped::stamp(&copy), Stamped::stamp(&v));
        *copy = 8;
        assert_ne!(Stamped::stamp(&copy), Stamped::stamp(&v));
    }

    #[test]
    fn two_values_never_share_a_stamp() {
        let (a, b) = (Stamped::new(1u32), Stamped::<u32>::default());
        assert_ne!(Stamped::stamp(&a), Stamped::stamp(&b));
    }

    #[test]
    fn a_change_kept_unstamped_keeps_the_stamp() {
        let mut v = Stamped::new(1u32);
        let stamp = Stamped::stamp(&v);
        *Stamped::unstamped(&mut v) = 2;
        assert_eq!((*v, Stamped::stamp(&v)), (2, stamp));
    }

    #[test]
    fn a_loaded_value_has_a_new_stamp() {
        use crate::savestate::{Reader, State, Writer};
        let mut v = Stamped::new(3u32);
        let mut w = Writer::default();
        v.save(&mut w);
        let stamp = Stamped::stamp(&v);
        v.load(&mut Reader::new(&w.buf)).unwrap();
        assert_eq!(*v, 3);
        assert_ne!(Stamped::stamp(&v), stamp);
    }
}
