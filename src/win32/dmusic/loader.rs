//! DMLOADER.DLL: the loader, which makes DirectMusic's objects of files
//! and memory (files.rs), finds files in the search directory of their
//! class, and keeps what it made in its cache, holding a reference on
//! each, so that a second GetObject gives the same object.
//!
//! The GUID of the default collection gives the General MIDI
//! instruments. Objects a file only refers to (a style's or a band's in
//! a segment) are not loaded, and ScanDirectory finds nothing.

use super::files::{self, Content};
use super::guids::*;
use super::{
    DESC_MEM_DATA, DESC_MEM_LENGTH, DESC_SIZE, DMUS_OBJ_CLASS, DMUS_OBJ_FILENAME,
    DMUS_OBJ_FULLPATH, DMUS_OBJ_LOADED, DMUS_OBJ_MEMORY, DMUS_OBJ_NAME, DMUS_OBJ_OBJECT, Desc,
    E_INVALIDARG, E_POINTER, Kind, LOADER, S_FALSE, S_OK, add, dmus_e, drop_ref, give, give_new,
    hold, object_of, read_u64, read_wide, state, this_of,
};
use crate::cpu::Cpu;
use crate::win32::com::{self, Guid};

pub const DMUS_E_UNSUPPORTED_STREAM: u32 = dmus_e(0x150);
pub const DMUS_E_LOADER_NOCLASSID: u32 = dmus_e(0x180);
pub const DMUS_E_LOADER_BADPATH: u32 = dmus_e(0x181);
pub const DMUS_E_LOADER_FAILEDOPEN: u32 = dmus_e(0x182);
pub const DMUS_E_LOADER_FORMATNOTSUPPORTED: u32 = dmus_e(0x183);
pub const DMUS_E_LOADER_OBJECTNOTFOUND: u32 = dmus_e(0x185);

/// The most a file can have that the loader reads.
const MAX_FILE: usize = 32 << 20;

/// An object in the cache: its id, class, GUID, file (full path, upper
/// case) and name.
#[derive(Clone, Debug, Default)]
pub struct Cached {
    pub id: u32,
    pub class: [u8; 16],
    pub object: [u8; 16],
    pub file: String,
    pub name: String,
}

/// The loader: search directories by class, the cache, the classes
/// whose objects it doesn't cache, and the descriptions SetObject gave.
#[derive(Clone, Debug, Default)]
pub struct Loader {
    pub dirs: Vec<([u8; 16], String)>,
    pub cache: Vec<Cached>,
    pub uncached: Vec<[u8; 16]>,
    pub known: Vec<Desc>,
}

impl Loader {
    pub fn held(&self) -> Vec<u32> {
        self.cache.iter().map(|c| c.id).collect()
    }

    /// Whether it caches objects of `class`.
    fn caches(&self, class: &[u8; 16]) -> bool {
        !self
            .uncached
            .iter()
            .any(|c| c == class || *c == GUID_ALL_TYPES.0)
    }

    /// The search directory for `class`.
    fn dir(&self, class: &[u8; 16]) -> Option<&str> {
        self.dirs
            .iter()
            .find(|d| d.0 == *class)
            .or_else(|| self.dirs.iter().find(|d| d.0 == GUID_ALL_TYPES.0))
            .map(|d| d.1.as_str())
    }
}

/// CoCreateInstance of the loader.
pub fn create(cpu: &mut Cpu, iid: &Guid, out: u32) -> u32 {
    let id = add(
        cpu,
        Kind::Loader {
            l: Loader::default(),
        },
    );
    give_new(cpu, id, iid, out)
}

fn loader(cpu: &mut Cpu, id: u32) -> Option<&mut Loader> {
    match &mut state(cpu).get_mut(id)?.kind {
        Kind::Loader { l } => Some(l),
        _ => None,
    }
}

/// The loader `this` is.
fn this_loader(cpu: &mut Cpu, this: u32) -> Option<u32> {
    this_of(cpu, this, LOADER)
}

/// Whether a class GUID stands for all classes.
fn all(class: &[u8; 16]) -> bool {
    *class == GUID_ALL_TYPES.0
}

/// The bytes of the file `path`, or None if it can't be read.
fn read_file(cpu: &mut Cpu, path: &str) -> Option<Vec<u8>> {
    use crate::win32::kernel32;
    let handle = kernel32::open_handle(cpu, path, 0x8000_0000, 1, 3).ok()?;
    let mut data = Vec::new();
    loop {
        match kernel32::read_file(cpu, handle, 0x10000) {
            Ok(chunk) if !chunk.is_empty() && data.len() < MAX_FILE => data.extend(chunk),
            Ok(_) => break,
            Err(_) => {
                crate::win32::object::close(cpu, handle);
                return None;
            }
        }
    }
    crate::win32::object::close(cpu, handle);
    Some(data)
}

/// The object of the content `c`: its id.
fn make(cpu: &mut Cpu, c: Content) -> u32 {
    match c {
        Content::Segment(data) => super::segment::new_segment(cpu, data),
        Content::Collection(list) => super::core::new_collection(cpu, list),
        Content::Band(inst) => super::content::new_band(cpu, inst),
        Content::Style { tempo, sig, bands } => {
            let mut list = Vec::new();
            for (desc, inst) in bands {
                let band = super::content::new_band(cpu, inst);
                if let Some(o) = state(cpu).get_mut(band) {
                    o.desc.merge(&desc);
                }
                list.push((desc.name, band));
            }
            add(
                cpu,
                Kind::Style {
                    s: super::content::Style {
                        tempo,
                        sig,
                        bands: list,
                    },
                },
            )
        }
        Content::ChordMap(scale) => add(cpu, Kind::ChordMap { scale }),
    }
}

/// IDirectMusicLoader::GetObject(this, desc, iid, out).
pub fn get_object(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at, iid, out] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let Some(id) = this_loader(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if at == 0 || out == 0 || iid == 0 {
        return Some(E_POINTER);
    }
    com::write_out(cpu, out, 0);
    // Windows 98 SE takes DMUS_OBJECTDESC of DirectX 6.1's size only.
    if !cpu.bus.guest_probe(at, DESC_SIZE as usize, false) || cpu.bus.guest_read_32(at) != DESC_SIZE
    {
        return Some(E_INVALIDARG);
    }
    let iid = Guid::read(cpu, iid)?;
    let mut d = Desc::read(cpu, at);
    if d.valid & DMUS_OBJ_CLASS == 0 {
        return Some(DMUS_E_LOADER_NOCLASSID);
    }
    // What SetObject said of the object adds to what the program gives.
    let known = loader(cpu, id)?.known.iter().find(|k| same(k, &d)).cloned();
    if let Some(k) = known {
        let mut merged = k;
        merged.merge(&d);
        d = merged;
    }
    // A file's full path, from the search directory of its class.
    let path = (d.valid & DMUS_OBJ_FILENAME != 0).then(|| {
        if d.valid & DMUS_OBJ_FULLPATH != 0 {
            crate::win32::kernel32::full_path(cpu, &d.file)
        } else {
            let dir = loader(cpu, id)
                .and_then(|l| l.dir(&d.class).map(str::to_string))
                .unwrap_or_default();
            let joined = if dir.is_empty() {
                d.file.clone()
            } else {
                format!("{}\\{}", dir.trim_end_matches('\\'), d.file)
            };
            crate::win32::kernel32::full_path(cpu, &joined)
        }
        .to_ascii_uppercase()
    });
    // The cache, by GUID, file or name.
    let cached = loader(cpu, id)?
        .cache
        .iter()
        .find(|c| {
            (all(&d.class) || c.class == d.class)
                && ((d.valid & DMUS_OBJ_OBJECT != 0 && c.object == d.object)
                    || path.as_ref().is_some_and(|p| *p == c.file)
                    || (d.valid & DMUS_OBJ_NAME != 0
                        && d.valid & (DMUS_OBJ_FILENAME | DMUS_OBJ_OBJECT) == 0
                        && !d.name.is_empty()
                        && c.name.eq_ignore_ascii_case(&d.name)))
        })
        .map(|c| c.id);
    if let Some(obj) = cached {
        return Some(give(cpu, obj, &iid, out));
    }
    let (fdesc, content) =
        if d.valid & DMUS_OBJ_OBJECT != 0 && d.object == GUID_DEFAULT_GM_COLLECTION.0 {
            (Desc::default(), Content::Collection(files::gm_collection()))
        } else {
            let data = if let Some(p) = &path {
                match read_file(cpu, p) {
                    Some(data) => data,
                    None => return Some(DMUS_E_LOADER_FAILEDOPEN),
                }
            } else if d.valid & DMUS_OBJ_MEMORY != 0 {
                let len = read_u64(cpu, at + DESC_MEM_LENGTH).min(MAX_FILE as u64) as usize;
                let from = cpu.bus.guest_read_32(at + DESC_MEM_DATA);
                if from == 0 || !cpu.bus.guest_probe(from, len, false) {
                    return Some(E_POINTER);
                }
                let mut data = vec![0; len];
                cpu.bus.guest_read_bytes(from, &mut data);
                data
            } else {
                return Some(DMUS_E_LOADER_OBJECTNOTFOUND);
            };
            match files::class_of(&data) {
                Some(class) if all(&d.class) || class == d.class => {}
                Some(_) => return Some(DMUS_E_UNSUPPORTED_STREAM),
                None => return Some(DMUS_E_LOADER_FORMATNOTSUPPORTED),
            }
            match files::read(&data) {
                Some(r) => r,
                None => return Some(DMUS_E_UNSUPPORTED_STREAM),
            }
        };
    let obj = make(cpu, content);
    // Its description: what the file says, the class, and the file's
    // name as the program gave it.
    let mut desc = fdesc;
    desc.valid |= DMUS_OBJ_CLASS | DMUS_OBJ_LOADED;
    if d.valid & DMUS_OBJ_FILENAME != 0 {
        desc.file = d.file.clone();
        desc.valid |= DMUS_OBJ_FILENAME;
    }
    if d.valid & DMUS_OBJ_OBJECT != 0 && desc.valid & DMUS_OBJ_OBJECT == 0 {
        desc.object = d.object;
        desc.valid |= DMUS_OBJ_OBJECT;
    }
    if let Some(o) = state(cpu).get_mut(obj) {
        o.desc = desc.clone();
    }
    let class = files_class(cpu, obj);
    if loader(cpu, id)?.caches(&class) {
        hold(cpu, obj);
        loader(cpu, id)?.cache.push(Cached {
            id: obj,
            class,
            object: desc.object,
            file: path.unwrap_or_default(),
            name: desc.name.clone(),
        });
    }
    Some(give_new(cpu, obj, &iid, out))
}

/// The class of the object `id`, as its description has it.
fn files_class(cpu: &mut Cpu, id: u32) -> [u8; 16] {
    let class = match state(cpu).get(id).map(|o| &o.kind) {
        Some(Kind::Segment { .. }) => CLSID_SEGMENT,
        Some(Kind::Collection { .. }) => CLSID_COLLECTION,
        Some(Kind::Band { .. }) => CLSID_BAND,
        Some(Kind::Style { .. }) => CLSID_STYLE,
        Some(Kind::ChordMap { .. }) => CLSID_CHORD_MAP,
        _ => com::GUID_NULL,
    };
    class.0
}

/// Whether two descriptions are of one object: by GUID, or else by file
/// or name.
fn same(a: &Desc, b: &Desc) -> bool {
    if a.valid & b.valid & DMUS_OBJ_OBJECT != 0 {
        return a.object == b.object;
    }
    if a.valid & b.valid & DMUS_OBJ_FILENAME != 0 {
        return a.file.eq_ignore_ascii_case(&b.file);
    }
    a.valid & b.valid & DMUS_OBJ_NAME != 0 && a.name.eq_ignore_ascii_case(&b.name)
}

/// IDirectMusicLoader::SetObject(this, desc): what the loader is to know
/// of an object, to find it by.
pub fn set_object(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = this_loader(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    if args[1] == 0 || !cpu.bus.guest_probe(args[1], DESC_SIZE as usize, false) {
        return Some(E_POINTER);
    }
    let d = Desc::read(cpu, args[1]);
    let l = loader(cpu, id)?;
    match l.known.iter_mut().find(|k| same(k, &d)) {
        Some(k) => k.merge(&d),
        None => l.known.push(d),
    }
    Some(S_OK)
}

/// The characters of a path (MAX_PATH).
const MAX_PATH: usize = 260;

/// IDirectMusicLoader::SetSearchDirectory(this, class, path, clear):
/// S_FALSE when it already is the directory.
pub fn set_search_directory(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, class, path, clear] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let Some(id) = this_loader(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    let Some(class) = Guid::read(cpu, class) else {
        return Some(E_POINTER);
    };
    let dir = if path == 0 {
        crate::win32::kernel32::full_path(cpu, ".")
    } else {
        read_wide(cpu, path, MAX_PATH)
    };
    if !crate::win32::kernel32::is_directory(cpu, &dir) {
        return Some(DMUS_E_LOADER_BADPATH);
    }
    let full = crate::win32::kernel32::full_path(cpu, &dir);
    let l = loader(cpu, id)?;
    let same = l
        .dirs
        .iter()
        .any(|d| d.0 == class.0 && d.1.eq_ignore_ascii_case(&full));
    if clear != 0 {
        clear_cache_of(cpu, id, &class.0);
    }
    if same {
        return Some(S_FALSE);
    }
    let l = loader(cpu, id)?;
    l.dirs.retain(|d| d.0 != class.0);
    l.dirs.push((class.0, full));
    Some(S_OK)
}

/// IDirectMusicLoader::ScanDirectory(this, class, extension, log): no
/// files are looked at, so none is found.
pub fn scan_directory(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if this_loader(cpu, args[0]).is_none() {
        return Some(E_INVALIDARG);
    }
    if args[1] == 0 || args[2] == 0 {
        return Some(E_POINTER);
    }
    Some(S_FALSE)
}

/// IDirectMusicLoader::CacheObject(this, object): S_FALSE when it is in
/// the cache.
pub fn cache_object(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = this_loader(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let Some(obj) = object_of(cpu, args[1]) else {
        return Some(E_POINTER);
    };
    if loader(cpu, id)?.cache.iter().any(|c| c.id == obj) {
        return Some(S_FALSE);
    }
    let class = files_class(cpu, obj);
    let desc = state(cpu).get(obj)?.desc.clone();
    hold(cpu, obj);
    loader(cpu, id)?.cache.push(Cached {
        id: obj,
        class,
        object: desc.object,
        file: String::new(),
        name: desc.name,
    });
    Some(S_OK)
}

/// IDirectMusicLoader::ReleaseObject(this, object): out of the cache;
/// S_FALSE when it isn't in it.
pub fn release_object(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = this_loader(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    if args[1] == 0 {
        return Some(E_POINTER);
    }
    let Some(obj) = object_of(cpu, args[1]) else {
        return Some(E_POINTER);
    };
    let l = loader(cpu, id)?;
    let Some(i) = l.cache.iter().position(|c| c.id == obj) else {
        return Some(S_FALSE);
    };
    l.cache.remove(i);
    drop_ref(cpu, obj);
    Some(S_OK)
}

/// Take the objects of `class` out of the loader `id`'s cache.
fn clear_cache_of(cpu: &mut Cpu, id: u32, class: &[u8; 16]) {
    let Some(l) = loader(cpu, id) else {
        return;
    };
    let (gone, kept): (Vec<Cached>, Vec<Cached>) = l
        .cache
        .drain(..)
        .partition(|c| all(class) || c.class == *class);
    l.cache = kept;
    for c in gone {
        drop_ref(cpu, c.id);
    }
}

/// IDirectMusicLoader::ClearCache(this, class).
pub fn clear_cache(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = this_loader(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let Some(class) = Guid::read(cpu, args[1]) else {
        return Some(E_POINTER);
    };
    clear_cache_of(cpu, id, &class.0);
    Some(S_OK)
}

/// IDirectMusicLoader::EnableCache(this, class, enable): S_FALSE when it
/// already was so. Turning it off clears the cache of the class.
pub fn enable_cache(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = this_loader(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let Some(class) = Guid::read(cpu, args[1]) else {
        return Some(E_POINTER);
    };
    let enable = args[2] != 0;
    let l = loader(cpu, id)?;
    let was = l.caches(&class.0);
    if enable {
        if all(&class.0) {
            l.uncached.clear();
        } else {
            l.uncached.retain(|c| *c != class.0);
        }
    } else {
        if !l.uncached.contains(&class.0) {
            l.uncached.push(class.0);
        }
        clear_cache_of(cpu, id, &class.0);
    }
    Some(if was == enable { S_FALSE } else { S_OK })
}

/// IDirectMusicLoader::EnumObject(this, class, index, desc): the objects
/// of the class the loader knows, in its cache and from SetObject;
/// S_FALSE past the last.
pub fn enum_object(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, class, index, at] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let Some(id) = this_loader(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    let Some(class) = Guid::read(cpu, class) else {
        return Some(E_POINTER);
    };
    if at == 0 || !cpu.bus.guest_probe(at, DESC_SIZE as usize, true) {
        return Some(E_POINTER);
    }
    let l = loader(cpu, id)?.clone();
    let mut list: Vec<Desc> = Vec::new();
    for c in l
        .cache
        .iter()
        .filter(|c| all(&class.0) || c.class == class.0)
    {
        if let Some(o) = state(cpu).get(c.id) {
            let mut d = o.desc.clone();
            d.class = c.class;
            d.valid |= DMUS_OBJ_CLASS;
            list.push(d);
        }
    }
    for k in l
        .known
        .iter()
        .filter(|k| all(&class.0) || k.class == class.0)
    {
        if !list.iter().any(|d| same(d, k)) {
            list.push(k.clone());
        }
    }
    let Some(d) = list.get(index as usize) else {
        return Some(S_FALSE);
    };
    let size = cpu.bus.guest_read_32(at);
    cpu.bus.guest_fill(at, DESC_MEM_LENGTH as usize, 0);
    cpu.bus.guest_write_32(at, size);
    d.write(cpu, at);
    Some(S_OK)
}

crate::state_fields!(Cached {
    id,
    class,
    object,
    file,
    name
});
crate::state_fields!(Loader {
    dirs,
    cache,
    uncached,
    known
});
