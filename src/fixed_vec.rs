//! A list of at most `N` elements kept in an array, for lists that hot
//! paths build again and again and that a `Vec` would allocate for each
//! time: the Direct3D pipeline's clip planes and clipped polygons, and
//! the parameters a Glide triangle writes.

use std::ops::Deref;

/// At most `N` elements of `T`, in place. Pushing one more than `N`
/// panics: each user sizes `N` to the most its list can hold.
#[derive(Clone, Copy)]
pub struct FixedVec<T: Copy, const N: usize> {
    items: [T; N],
    len: usize,
}

impl<T: Copy, const N: usize> FixedVec<T, N> {
    /// An empty list, its array filled with `fill`, which it never reads.
    pub fn new(fill: T) -> Self {
        Self {
            items: [fill; N],
            len: 0,
        }
    }

    pub fn push(&mut self, item: T) {
        self.items[self.len] = item;
        self.len += 1;
    }

    pub fn extend_from_slice(&mut self, items: &[T]) {
        self.items[self.len..self.len + items.len()].copy_from_slice(items);
        self.len += items.len();
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }
}

impl<T: Copy, const N: usize> Deref for FixedVec<T, N> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        &self.items[..self.len]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holds_what_was_pushed_up_to_its_room() {
        let mut v = FixedVec::<u8, 4>::new(0);
        v.push(1);
        v.extend_from_slice(&[2, 3]);
        v.push(4);
        assert_eq!(&v[..], &[1, 2, 3, 4]);
        v.clear();
        assert!(v.is_empty());
        v.push(5);
        assert_eq!(&v[..], &[5]);
    }

    #[test]
    #[should_panic]
    fn panics_past_its_room() {
        let mut v = FixedVec::<u8, 2>::new(0);
        v.extend_from_slice(&[1, 2]);
        v.push(3);
    }
}
