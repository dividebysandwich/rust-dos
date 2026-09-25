//! Save states: the whole emulated machine written to bytes and read back
//! into the same machine, so a game goes on from where it was saved.
//!
//! Every part of the machine with state of its own implements `State`,
//! most of them with `state_fields!`, which names every field of a struct:
//! the ones saved, in order, and the ones skipped (caches rebuilt after a
//! load, and what belongs to the host, such as the audio device). It takes
//! the struct apart without `..`, so a field added to one fails to compile
//! until it is put in one list or the other.
//!
//! State is loaded in place, into the machine that runs: the RAM keeps its
//! allocation (the recompiler's code points into it), and the host's parts
//! stay as they are. A saved machine is a list of sections, each a tag,
//! a version and its length, so a section a newer rust-dos changed is
//! refused rather than misread.

pub mod machine;
pub mod slots;

use std::collections::{BTreeMap, HashMap, VecDeque};

/// Why a state can't be loaded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StateError {
    /// It ends early, or a count in it is impossible.
    Truncated,
    /// A value no field of this rust-dos can hold.
    Invalid(String),
    /// Its machine differs from this one in a way loading can't change.
    Mismatch(String),
}

impl std::fmt::Display for StateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StateError::Truncated => write!(f, "the saved state is cut short or damaged"),
            StateError::Invalid(what) => write!(f, "the saved state has {}", what),
            StateError::Mismatch(what) => write!(f, "the saved state is of another machine: {}", what),
        }
    }
}

pub type Result<T> = std::result::Result<T, StateError>;

/// Bytes being written.
#[derive(Default)]
pub struct Writer {
    pub buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn bytes(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// A section: `tag`, its `version` and the length of what `write` puts
    /// in it.
    pub fn section(&mut self, tag: &[u8; 4], version: u16, write: impl FnOnce(&mut Writer)) {
        self.bytes(tag);
        version.save(self);
        let at = self.buf.len();
        self.bytes(&[0; 4]);
        write(self);
        let len = (self.buf.len() - at - 4) as u32;
        self.buf[at..at + 4].copy_from_slice(&len.to_le_bytes());
    }
}

/// Bytes being read.
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(len).filter(|&end| end <= self.data.len()).ok_or(StateError::Truncated)?;
        let bytes = &self.data[self.pos..end];
        self.pos = end;
        Ok(bytes)
    }

    pub fn is_empty(&self) -> bool {
        self.pos >= self.data.len()
    }

    /// The section `tag` in the version `version`: a reader of it. Sections
    /// come in the order they were written.
    pub fn section(&mut self, tag: &[u8; 4], version: u16) -> Result<Reader<'a>> {
        let found: [u8; 4] = self.take(4)?.try_into().unwrap();
        if &found != tag {
            return Err(StateError::Invalid(format!(
                "section {} where {} should be",
                String::from_utf8_lossy(&found),
                String::from_utf8_lossy(tag)
            )));
        }
        let mut saved = 0u16;
        saved.load(self)?;
        if saved != version {
            return Err(StateError::Mismatch(format!(
                "its {} is version {}, this rust-dos reads version {}",
                String::from_utf8_lossy(tag).trim(),
                saved,
                version
            )));
        }
        let mut len = 0u32;
        len.load(self)?;
        Ok(Reader::new(self.take(len as usize)?))
    }

    /// A count of things to read, which can't be more than the bytes left.
    pub fn count(&mut self) -> Result<usize> {
        let mut n = 0u64;
        n.load(self)?;
        let n = usize::try_from(n).map_err(|_| StateError::Truncated)?;
        if n > self.data.len() - self.pos {
            return Err(StateError::Truncated);
        }
        Ok(n)
    }
}

/// Something with state to save and load in place.
pub trait State {
    fn save(&self, w: &mut Writer);
    fn load(&mut self, r: &mut Reader) -> Result<()>;

    /// A run of these, which bytes write at once.
    fn save_slice(items: &[Self], w: &mut Writer)
    where
        Self: Sized,
    {
        for item in items {
            item.save(w);
        }
    }

    fn load_slice(items: &mut [Self], r: &mut Reader) -> Result<()>
    where
        Self: Sized,
    {
        for item in items {
            item.load(r)?;
        }
        Ok(())
    }
}

/// `State` for a struct by its fields: those saved in the braces, and
/// those left as they are after `skip`. Every field has to be in one list.
#[macro_export]
macro_rules! state_fields {
    ($t:ty { $($f:ident),* $(,)? } $(skip { $($s:ident),* $(,)? })?) => {
        impl $crate::savestate::State for $t {
            #[allow(unused_variables)]
            fn save(&self, w: &mut $crate::savestate::Writer) {
                let Self { $($f,)* $($($s: _,)*)? } = self;
                $( $crate::savestate::State::save($f, w); )*
            }
            #[allow(unused_variables)]
            fn load(&mut self, r: &mut $crate::savestate::Reader) -> $crate::savestate::Result<()> {
                let Self { $($f,)* $($($s: _,)*)? } = self;
                $( $crate::savestate::State::load($f, r)?; )*
                Ok(())
            }
        }
    };
}

/// `State` for a fieldless enum, as the position of its variant in the list.
#[macro_export]
macro_rules! state_enum {
    ($t:ty { $($v:path),* $(,)? }) => {
        impl $crate::savestate::State for $t {
            fn save(&self, w: &mut $crate::savestate::Writer) {
                let variants = [$($v),*];
                let index = variants.iter().position(|v| v == self).unwrap_or(0) as u8;
                $crate::savestate::State::save(&index, w);
            }
            fn load(&mut self, r: &mut $crate::savestate::Reader) -> $crate::savestate::Result<()> {
                let mut index = 0u8;
                $crate::savestate::State::load(&mut index, r)?;
                let variants = [$($v),*];
                *self = variants.into_iter().nth(index as usize).ok_or_else(|| {
                    $crate::savestate::StateError::Invalid(format!("a {} it doesn't know", stringify!($t)))
                })?;
                Ok(())
            }
        }
    };
}

macro_rules! numbers {
    ($($t:ty),*) => {$(
        impl State for $t {
            fn save(&self, w: &mut Writer) {
                w.bytes(&self.to_le_bytes());
            }
            fn load(&mut self, r: &mut Reader) -> Result<()> {
                *self = <$t>::from_le_bytes(r.take(std::mem::size_of::<$t>())?.try_into().unwrap());
                Ok(())
            }
        }
    )*};
}
numbers!(u16, u32, u64, u128, i8, i16, i32, i64, f32, f64);

impl State for u8 {
    fn save(&self, w: &mut Writer) {
        w.buf.push(*self);
    }
    fn load(&mut self, r: &mut Reader) -> Result<()> {
        *self = r.take(1)?[0];
        Ok(())
    }
    fn save_slice(items: &[Self], w: &mut Writer) {
        w.bytes(items);
    }
    fn load_slice(items: &mut [Self], r: &mut Reader) -> Result<()> {
        items.copy_from_slice(r.take(items.len())?);
        Ok(())
    }
}

impl State for usize {
    fn save(&self, w: &mut Writer) {
        (*self as u64).save(w);
    }
    fn load(&mut self, r: &mut Reader) -> Result<()> {
        let mut v = 0u64;
        v.load(r)?;
        *self = usize::try_from(v).map_err(|_| StateError::Invalid("a size too large".into()))?;
        Ok(())
    }
}

impl State for bool {
    fn save(&self, w: &mut Writer) {
        w.buf.push(*self as u8);
    }
    fn load(&mut self, r: &mut Reader) -> Result<()> {
        *self = r.take(1)?[0] != 0;
        Ok(())
    }
}

impl State for char {
    fn save(&self, w: &mut Writer) {
        (*self as u32).save(w);
    }
    fn load(&mut self, r: &mut Reader) -> Result<()> {
        let mut v = 0u32;
        v.load(r)?;
        *self = char::from_u32(v).ok_or_else(|| StateError::Invalid("a character that isn't one".into()))?;
        Ok(())
    }
}

impl State for String {
    fn save(&self, w: &mut Writer) {
        self.len().save(w);
        w.bytes(self.as_bytes());
    }
    fn load(&mut self, r: &mut Reader) -> Result<()> {
        let len = r.count()?;
        *self = String::from_utf8(r.take(len)?.to_vec()).map_err(|_| StateError::Invalid("text that isn't UTF-8".into()))?;
        Ok(())
    }
}

impl State for std::path::PathBuf {
    fn save(&self, w: &mut Writer) {
        self.to_string_lossy().into_owned().save(w);
    }
    fn load(&mut self, r: &mut Reader) -> Result<()> {
        let mut s = String::new();
        s.load(r)?;
        *self = s.into();
        Ok(())
    }
}

impl<T: State + Default> State for Vec<T> {
    fn save(&self, w: &mut Writer) {
        self.len().save(w);
        T::save_slice(self, w);
    }
    fn load(&mut self, r: &mut Reader) -> Result<()> {
        let len = r.count()?;
        self.clear();
        self.resize_with(len, T::default);
        T::load_slice(self, r)
    }
}

impl<T: State + Default> State for Box<[T]> {
    fn save(&self, w: &mut Writer) {
        self.len().save(w);
        T::save_slice(self, w);
    }
    fn load(&mut self, r: &mut Reader) -> Result<()> {
        let len = r.count()?;
        if len != self.len() {
            let mut v = Vec::new();
            v.resize_with(len, T::default);
            *self = v.into_boxed_slice();
        }
        T::load_slice(self, r)
    }
}

impl<T: State + Default> State for VecDeque<T> {
    fn save(&self, w: &mut Writer) {
        self.len().save(w);
        for item in self {
            item.save(w);
        }
    }
    fn load(&mut self, r: &mut Reader) -> Result<()> {
        let len = r.count()?;
        self.clear();
        for _ in 0..len {
            let mut item = T::default();
            item.load(r)?;
            self.push_back(item);
        }
        Ok(())
    }
}

impl<T: State, const N: usize> State for [T; N] {
    fn save(&self, w: &mut Writer) {
        T::save_slice(self, w);
    }
    fn load(&mut self, r: &mut Reader) -> Result<()> {
        T::load_slice(self, r)
    }
}

impl<T: State + Default> State for Option<T> {
    fn save(&self, w: &mut Writer) {
        match self {
            Some(v) => {
                true.save(w);
                v.save(w);
            }
            None => false.save(w),
        }
    }
    fn load(&mut self, r: &mut Reader) -> Result<()> {
        let mut some = false;
        some.load(r)?;
        if some {
            let v = self.get_or_insert_with(T::default);
            v.load(r)
        } else {
            *self = None;
            Ok(())
        }
    }
}

impl<A: State, B: State> State for (A, B) {
    fn save(&self, w: &mut Writer) {
        self.0.save(w);
        self.1.save(w);
    }
    fn load(&mut self, r: &mut Reader) -> Result<()> {
        self.0.load(r)?;
        self.1.load(r)
    }
}

impl<A: State, B: State, C: State> State for (A, B, C) {
    fn save(&self, w: &mut Writer) {
        self.0.save(w);
        self.1.save(w);
        self.2.save(w);
    }
    fn load(&mut self, r: &mut Reader) -> Result<()> {
        self.0.load(r)?;
        self.1.load(r)?;
        self.2.load(r)
    }
}

impl<T: State + Copy> State for std::cell::Cell<T> {
    fn save(&self, w: &mut Writer) {
        self.get().save(w);
    }
    fn load(&mut self, r: &mut Reader) -> Result<()> {
        self.get_mut().load(r)
    }
}

/// Maps are written sorted by key, so the same map always writes the same
/// bytes.
impl<K: State + Default + Ord + Clone + std::hash::Hash, V: State + Default> State for HashMap<K, V> {
    fn save(&self, w: &mut Writer) {
        let mut keys: Vec<&K> = self.keys().collect();
        keys.sort();
        keys.len().save(w);
        for k in keys {
            k.save(w);
            self[k].save(w);
        }
    }
    fn load(&mut self, r: &mut Reader) -> Result<()> {
        let len = r.count()?;
        self.clear();
        for _ in 0..len {
            let (mut k, mut v) = (K::default(), V::default());
            k.load(r)?;
            v.load(r)?;
            self.insert(k, v);
        }
        Ok(())
    }
}

impl<K: State + Default + Ord, V: State + Default> State for BTreeMap<K, V> {
    fn save(&self, w: &mut Writer) {
        self.len().save(w);
        for (k, v) in self {
            k.save(w);
            v.save(w);
        }
    }
    fn load(&mut self, r: &mut Reader) -> Result<()> {
        let len = r.count()?;
        self.clear();
        for _ in 0..len {
            let (mut k, mut v) = (K::default(), V::default());
            k.load(r)?;
            v.load(r)?;
            self.insert(k, v);
        }
        Ok(())
    }
}

/// Load an optional device's state: the machine has to have the device if
/// the state does, as loading can't make one.
pub fn load_device<T: State>(device: &mut Option<T>, name: &str, r: &mut Reader) -> Result<()> {
    let mut some = false;
    some.load(r)?;
    match (some, device) {
        (true, Some(d)) => d.load(r),
        (false, None) => Ok(()),
        (true, None) => Err(StateError::Mismatch(format!("it has a {} and this machine hasn't", name))),
        (false, Some(_)) => Err(StateError::Mismatch(format!("this machine has a {} and it hasn't", name))),
    }
}

/// Save an optional device's state (see `load_device`).
pub fn save_device<T: State>(device: &Option<T>, w: &mut Writer) {
    device.is_some().save(w);
    if let Some(d) = device {
        d.save(w);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default, Debug, PartialEq)]
    struct Sample {
        a: u16,
        b: Vec<u8>,
        c: Option<String>,
        d: [u32; 3],
        e: VecDeque<(u8, bool)>,
        cache: u64,
    }
    crate::state_fields!(Sample { a, b, c, d, e } skip { cache });

    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    enum Colour {
        #[default]
        Red,
        Green,
    }
    crate::state_enum!(Colour { Colour::Red, Colour::Green });

    #[test]
    fn states_come_back_as_they_were_saved() {
        let sample = Sample {
            a: 0x1234,
            b: vec![1, 2, 3],
            c: Some("hello".into()),
            d: [7, 8, 9],
            e: [(1, true), (2, false)].into(),
            cache: 99,
        };
        let mut w = Writer::new();
        w.section(b"TEST", 1, |w| {
            sample.save(w);
            Colour::Green.save(w);
        });
        let mut loaded = Sample { cache: 5, ..Default::default() };
        let mut colour = Colour::Red;
        let mut r = Reader::new(&w.buf);
        let mut section = r.section(b"TEST", 1).unwrap();
        loaded.load(&mut section).unwrap();
        colour.load(&mut section).unwrap();
        assert_eq!(loaded, Sample { cache: 5, ..sample });
        assert_eq!(colour, Colour::Green);
        assert!(r.is_empty());

        // Another version, another section, or too few bytes are refused.
        assert!(matches!(Reader::new(&w.buf).section(b"TEST", 2), Err(StateError::Mismatch(_))));
        assert!(Reader::new(&w.buf).section(b"OTHR", 1).is_err());
        let mut short = Reader::new(&w.buf[..w.buf.len() - 1]);
        assert_eq!(short.section(b"TEST", 1).err(), Some(StateError::Truncated));
    }

    #[test]
    fn maps_write_the_same_bytes_whatever_their_order() {
        let a: HashMap<u32, String> = (0..50).map(|i| (i, i.to_string())).collect();
        let b: HashMap<u32, String> = (0..50).rev().map(|i| (i, i.to_string())).collect();
        let (mut wa, mut wb) = (Writer::new(), Writer::new());
        a.save(&mut wa);
        b.save(&mut wb);
        assert_eq!(wa.buf, wb.buf);
        let mut back = HashMap::new();
        back.load(&mut Reader::new(&wa.buf)).unwrap();
        assert_eq!(back, a);
    }

    #[test]
    fn devices_must_be_there_to_be_loaded() {
        let mut w = Writer::new();
        save_device(&Some(5u8), &mut w);
        let mut none: Option<u8> = None;
        assert!(matches!(load_device(&mut none, "thing", &mut Reader::new(&w.buf)), Err(StateError::Mismatch(_))));
        let mut some = Some(0u8);
        load_device(&mut some, "thing", &mut Reader::new(&w.buf)).unwrap();
        assert_eq!(some, Some(5));
    }
}
