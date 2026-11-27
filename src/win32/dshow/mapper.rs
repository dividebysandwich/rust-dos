//! The filter mapper (CLSID_FilterMapper2, IFilterMapper2), which keeps
//! the registry's list of DirectShow filters by category. A DLL that
//! holds filters registers them through it from its DllRegisterServer:
//! DirectShow's base classes (`AMovieDllRegisterServer2`) make the
//! mapper and call RegisterFilter for each of their filters. The Indeo
//! 5 codec, IR50_32.DLL, does this when a setup self-registers it, and
//! its registration fails without a mapper: Leadfoot's setup then ends
//! with an error and takes its install back (#2182).
//!
//! The filters go where DirectShow 6.1 on Windows 98 SE puts them, under
//! their category's key: `HKCR\CLSID\{category}\Instance\{instance}`,
//! with the values `FriendlyName`, `CLSID` and `FilterData`, the filter's
//! REGFILTER2 in the binary layout the filter mapper writes (its merit
//! and its pins, with the GUIDs they name after them, as Wine's
//! `FM2_WriteFilterData` lays it out). A category made with
//! CreateCategory goes under the categories' own category,
//! CLSID_ActiveMovieCategories. The built-in graph doesn't read these
//! keys: it plays the files it knows with its own decoders.

use super::{E_INVALIDARG, E_NOTIMPL, E_POINTER, FILE_NOT_FOUND, Kind, S_OK};
use crate::cpu::Cpu;
use crate::win32::com::Guid;
use crate::win32::registry::{self, HKEY_CLASSES_ROOT, Open, REG_BINARY, REG_DWORD, REG_SZ};

/// CLSID_LegacyAmFilterCategory, DirectShow filters: the category a
/// filter goes in when RegisterFilter names none.
pub const CLSID_LEGACY_AM_FILTER_CATEGORY: Guid = Guid::new(
    0x0838_63F1,
    0x70DE,
    0x11D0,
    [0xBD, 0x40, 0x00, 0xA0, 0xC9, 0x11, 0xCE, 0x86],
);

/// CLSID_ActiveMovieCategories, the category of the categories.
pub const CLSID_ACTIVE_MOVIE_CATEGORIES: Guid = Guid::new(
    0xDA4E_3DA0,
    0xD07D,
    0x11D0,
    [0xBD, 0x50, 0x00, 0xA0, 0xC9, 0x11, 0xCE, 0x86],
);

/// MEDIASUBTYPE_NULL, which a pin's media type without a minor type has.
const MEDIASUBTYPE_NULL: Guid = Guid([0; 16]);

/// A pin's flags (REGFILTERPINS2's dwFlags).
const REG_PINFLAG_B_ZERO: u32 = 0x1;
const REG_PINFLAG_B_RENDERER: u32 = 0x2;
const REG_PINFLAG_B_MANY: u32 = 0x4;
const REG_PINFLAG_B_OUTPUT: u32 = 0x8;

/// The version of the layout of `FilterData`, which the mapper writes
/// for filters of both versions of REGFILTER2.
const FILTER_DATA_VERSION: u32 = 2;

/// A new filter mapper, with one reference, to give as `iid` at `out`.
pub fn create(cpu: &mut Cpu, iid: &Guid, out: u32) -> u32 {
    let id = super::add(cpu, Kind::Mapper {});
    super::give_new(cpu, id, iid, out)
}

/// Whether `this` is an interface of a filter mapper.
fn is_mapper(cpu: &mut Cpu, this: u32) -> bool {
    super::object_of(cpu, this)
        .is_some_and(|id| matches!(super::kind(cpu, id), Some(Kind::Mapper {})))
}

/// The NUL-terminated UTF-16 string at `at` in the system's ANSI code
/// page, as the registry keeps its names and strings (a char for each
/// byte), or None if it can't be read there.
fn wide(cpu: &mut Cpu, at: u32) -> Option<String> {
    if at == 0 {
        return None;
    }
    let mut units = Vec::new();
    loop {
        let here = at.wrapping_add(2 * units.len() as u32);
        if units.len() >= 0x1000 || !cpu.bus.guest_probe(here, 2, false) {
            return None;
        }
        match cpu.bus.guest_read_16(here) {
            0 => break,
            c => units.push(c),
        }
    }
    let bytes = crate::win32::kernel32::wide_to_ansi(cpu, &units);
    Some(crate::dosstr::from_bytes(&bytes))
}

/// The double-word at `at`, or None if it can't be read there.
fn dword(cpu: &mut Cpu, at: u32) -> Option<u32> {
    cpu.bus
        .guest_probe(at, 4, false)
        .then(|| cpu.bus.guest_read_32(at))
}

/// A pin of a filter, as REGFILTERPINS2 has it.
struct Pin {
    flags: u32,
    instances: u32,
    /// Major and minor types.
    types: Vec<(Guid, Guid)>,
    /// REGPINMEDIUM: the medium's GUID and its two double-words.
    mediums: Vec<[u8; 24]>,
    category: Option<Guid>,
}

/// The media types of a pin: `count` REGPINTYPES at `at`.
fn pin_types(cpu: &mut Cpu, count: u32, at: u32) -> Option<Vec<(Guid, Guid)>> {
    (0..count)
        .map(|i| {
            let entry = at.wrapping_add(8 * i);
            let major = dword(cpu, entry)?;
            let major = Guid::read(cpu, major)?;
            let minor = match dword(cpu, entry.wrapping_add(4))? {
                0 => MEDIASUBTYPE_NULL,
                p => Guid::read(cpu, p)?,
            };
            Some((major, minor))
        })
        .collect()
}

/// The pins of the REGFILTER2 at `at`, version 1 (REGFILTERPINS) or 2
/// (REGFILTERPINS2), and its merit; None if it can't be read or has
/// another version.
fn read_filter(cpu: &mut Cpu, at: u32) -> Option<(u32, Vec<Pin>)> {
    let field = |cpu: &mut Cpu, base: u32, offset: u32| dword(cpu, base.wrapping_add(offset));
    let version = dword(cpu, at)?;
    // A filter of another version is refused even with no pins, which
    // the pins' own match below would let through.
    if version != 1 && version != 2 {
        return None;
    }
    let merit = field(cpu, at, 4)?;
    let count = field(cpu, at, 8)?;
    let pins = field(cpu, at, 12)?;
    let pins = (0..count).map(|i| match version {
        // REGFILTERPINS: strName, bRendered, bOutput, bZero, bMany,
        // clsConnectsToFilter, strConnectsToPin, nMediaTypes,
        // lpMediaType.
        1 => {
            let p = pins.wrapping_add(36u32.wrapping_mul(i));
            let flag = |cpu: &mut Cpu, offset: u32, bit: u32| {
                field(cpu, p, offset).map(|b| if b != 0 { bit } else { 0 })
            };
            let flags = flag(cpu, 4, REG_PINFLAG_B_RENDERER)?
                | flag(cpu, 8, REG_PINFLAG_B_OUTPUT)?
                | flag(cpu, 12, REG_PINFLAG_B_ZERO)?
                | flag(cpu, 16, REG_PINFLAG_B_MANY)?;
            let (n, types) = (field(cpu, p, 28)?, field(cpu, p, 32)?);
            Some(Pin {
                flags,
                instances: 0,
                types: pin_types(cpu, n, types)?,
                mediums: Vec::new(),
                category: None,
            })
        }
        // REGFILTERPINS2: dwFlags, cInstances, nMediaTypes, lpMediaType,
        // nMediums, lpMedium, clsPinCategory.
        2 => {
            let p = pins.wrapping_add(28u32.wrapping_mul(i));
            let (n, types) = (field(cpu, p, 8)?, field(cpu, p, 12)?);
            let (m, mediums) = (field(cpu, p, 16)?, field(cpu, p, 20)?);
            let mediums = (0..m)
                .map(|j| {
                    let at = mediums.wrapping_add(24u32.wrapping_mul(j));
                    cpu.bus.guest_probe(at, 24, false).then(|| {
                        let mut medium = [0; 24];
                        cpu.bus.guest_read_bytes(at, &mut medium);
                        medium
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            let category = match field(cpu, p, 24)? {
                0 => None,
                c => Some(Guid::read(cpu, c)?),
            };
            Some(Pin {
                flags: dword(cpu, p)?,
                instances: field(cpu, p, 4)?,
                types: pin_types(cpu, n, types)?,
                mediums,
                category,
            })
        }
        _ => None,
    });
    Some((merit, pins.collect::<Option<Vec<_>>>()?))
}

/// The `FilterData` of a filter of `merit` with `pins`: a header (the
/// layout's version, the merit, the pins' count and a zero), each pin
/// ("0pi3", with the pin's number in its first byte: its flags,
/// instances, media types' and mediums' counts, whether it has a
/// category and that GUID's offset), its media types ("0ty3", a zero and
/// the offsets of the major and minor types) and its mediums' offsets;
/// then each GUID and medium once, where the offsets point.
fn filter_data(merit: u32, pins: &[Pin]) -> Vec<u8> {
    let mut size = 16;
    for pin in pins {
        size += 24 + 4 * usize::from(pin.category.is_some());
        size += 16 * pin.types.len() + 4 * pin.mediums.len();
    }
    // The GUIDs and mediums after the pins, each once: (offset, bytes).
    let mut items: Vec<(usize, Vec<u8>)> = Vec::new();
    let mut pool: Vec<u8> = Vec::new();
    let mut place = |bytes: &[u8]| -> u32 {
        let offset = match items.iter().find(|(_, b)| b == bytes) {
            Some(&(at, _)) => at,
            None => {
                items.push((pool.len(), bytes.to_vec()));
                pool.extend_from_slice(bytes);
                pool.len() - bytes.len()
            }
        };
        (size + offset) as u32
    };
    let mut out = Vec::with_capacity(size);
    for d in [FILTER_DATA_VERSION, merit, pins.len() as u32, 0] {
        out.extend_from_slice(&d.to_le_bytes());
    }
    for (i, pin) in pins.iter().enumerate() {
        out.extend_from_slice(&[b'0'.wrapping_add(i as u8), b'p', b'i', b'3']);
        for d in [
            pin.flags,
            pin.instances,
            pin.types.len() as u32,
            pin.mediums.len() as u32,
            u32::from(pin.category.is_some()),
        ] {
            out.extend_from_slice(&d.to_le_bytes());
        }
        if let Some(category) = pin.category {
            out.extend_from_slice(&place(&category.0).to_le_bytes());
        }
        for (j, (major, minor)) in pin.types.iter().enumerate() {
            out.extend_from_slice(&[b'0'.wrapping_add(j as u8), b't', b'y', b'3']);
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&place(&major.0).to_le_bytes());
            out.extend_from_slice(&place(&minor.0).to_le_bytes());
        }
        for medium in &pin.mediums {
            out.extend_from_slice(&place(medium).to_le_bytes());
        }
    }
    out.extend_from_slice(&pool);
    out
}

/// The path under HKEY_CLASSES_ROOT of the instance `instance` of the
/// category `category`.
fn instance_key(category: &Guid, instance: &str) -> String {
    format!("CLSID\\{}\\Instance\\{}", category, instance)
}

/// Make the key `path` under HKEY_CLASSES_ROOT with `values` (name,
/// type, data); the registry is saved with the process's other changes.
fn write_key(cpu: &mut Cpu, path: &str, values: Vec<(&str, u32, Vec<u8>)>) {
    let registry = registry::get(cpu);
    let Open::Key(root) = registry.open(HKEY_CLASSES_ROOT, 0) else {
        return;
    };
    let (key, _) = registry.create(root, path);
    for (name, kind, data) in values {
        registry.set_value(key, name, kind, data);
    }
}

/// A string value's data: the ANSI text (from `wide`, or a GUID's)
/// with its NUL.
fn sz(text: &str) -> Vec<u8> {
    let mut data = crate::dosstr::to_bytes(text);
    data.push(0);
    data
}

/// The category at `at`, or CLSID_LegacyAmFilterCategory if it is NULL.
fn category_at(cpu: &mut Cpu, at: u32) -> Option<Guid> {
    match at {
        0 => Some(CLSID_LEGACY_AM_FILTER_CATEGORY),
        p => Guid::read(cpu, p),
    }
}

/// IFilterMapper2::CreateCategory(this, clsidCategory, dwCategoryMerit,
/// Description): the category among the categories, with its name and
/// merit.
pub fn create_category(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if !is_mapper(cpu, args[0]) {
        return Some(E_POINTER);
    }
    let Some(category) = Guid::read(cpu, args[1]) else {
        return Some(E_POINTER);
    };
    let Some(name) = wide(cpu, args[3]) else {
        return Some(E_POINTER);
    };
    let path = instance_key(&CLSID_ACTIVE_MOVIE_CATEGORIES, &category.to_string());
    let values = vec![
        ("FriendlyName", REG_SZ, sz(&name)),
        ("CLSID", REG_SZ, sz(&category.to_string())),
        ("Merit", REG_DWORD, args[2].to_le_bytes().to_vec()),
    ];
    write_key(cpu, &path, values);
    Some(S_OK)
}

/// IFilterMapper2::UnregisterFilter(this, pclsidCategory, szInstance,
/// Filter): the filter's key goes from its category, the instance named
/// by `szInstance` or else by the filter's CLSID; FILE_NOT_FOUND if it
/// isn't there.
pub fn unregister_filter(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if !is_mapper(cpu, args[0]) {
        return Some(E_POINTER);
    }
    let Some(category) = category_at(cpu, args[1]) else {
        return Some(E_POINTER);
    };
    let instance = match args[2] {
        0 => Guid::read(cpu, args[3]).map(|g| g.to_string()),
        p => wide(cpu, p),
    };
    let Some(instance) = instance else {
        return Some(E_POINTER);
    };
    let registry = registry::get(cpu);
    let Open::Key(root) = registry.open(HKEY_CLASSES_ROOT, 0) else {
        return Some(FILE_NOT_FOUND);
    };
    match registry.find(root, &instance_key(&category, &instance)) {
        Some(key) => {
            registry.delete(key);
            Some(S_OK)
        }
        None => Some(FILE_NOT_FOUND),
    }
}

/// IFilterMapper2::RegisterFilter(this, clsidFilter, Name, ppMoniker,
/// pclsidCategory, szInstance, prf2): the filter goes in its category
/// (DirectShow filters when NULL), as the instance `szInstance` or else
/// its CLSID. With `ppMoniker` and NULL at it, the device moniker of the
/// registration goes there (`device`), and the caller may add values of
/// its own through the moniker's property bag, as the Indeo 5 codec does
/// for its decoder among the video compressors. A moniker passed in at
/// `ppMoniker` names where the filter goes on Windows; that isn't taken
/// here (E_NOTIMPL), as no program seen passes one.
pub fn register_filter(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if !is_mapper(cpu, args[0]) {
        return Some(E_POINTER);
    }
    let given = match args[3] {
        0 => 0,
        p if cpu.bus.guest_probe(p, 4, false) => cpu.bus.guest_read_32(p),
        _ => return Some(E_POINTER),
    };
    if given != 0 {
        return Some(E_NOTIMPL);
    }
    let Some(filter) = Guid::read(cpu, args[1]) else {
        return Some(E_POINTER);
    };
    let Some(name) = wide(cpu, args[2]) else {
        return Some(E_POINTER);
    };
    let Some(category) = category_at(cpu, args[4]) else {
        return Some(E_POINTER);
    };
    let instance = match args[5] {
        0 => Some(filter.to_string()),
        p => wide(cpu, p),
    };
    let Some(instance) = instance else {
        return Some(E_POINTER);
    };
    if args[6] == 0 {
        return Some(E_POINTER);
    }
    let Some((merit, pins)) = read_filter(cpu, args[6]) else {
        return Some(E_INVALIDARG);
    };
    let values = vec![
        ("FriendlyName", REG_SZ, sz(&name)),
        ("CLSID", REG_SZ, sz(&filter.to_string())),
        ("FilterData", REG_BINARY, filter_data(merit, &pins)),
    ];
    write_key(cpu, &instance_key(&category, &instance), values);
    if args[3] != 0 {
        let moniker = super::device::new_moniker(cpu, &category, &instance);
        crate::win32::com::write_out(cpu, args[3], moniker);
        if moniker == 0 {
            return Some(super::E_OUTOFMEMORY);
        }
    }
    Some(S_OK)
}

/// IFilterMapper2::EnumMatchingFilters(this, ppEnum, ...): E_NOTIMPL,
/// as no program seen asks for it, with `*ppEnum` cleared as a failed
/// COM call leaves its out-pointer, so a caller that releases it
/// regardless finds NULL.
pub fn enum_matching_filters(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    crate::win32::com::write_out(cpu, args[1], 0);
    Some(E_NOTIMPL)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAJOR: Guid = Guid([1; 16]);
    const MINOR: Guid = Guid([2; 16]);

    /// An input pin and an output pin of one media type each, as a
    /// decoder's base classes register theirs (REGFILTERPINS, which the
    /// mapper writes in the layout of version 2), the output's type
    /// without a minor type.
    #[test]
    fn filter_data_lays_out_the_pins_then_their_guids() {
        let pins = [
            Pin {
                flags: 0,
                instances: 0,
                types: vec![(MAJOR, MINOR)],
                mediums: Vec::new(),
                category: None,
            },
            Pin {
                flags: REG_PINFLAG_B_OUTPUT,
                instances: 0,
                types: vec![(MAJOR, MEDIASUBTYPE_NULL)],
                mediums: Vec::new(),
                category: None,
            },
        ];
        let data = filter_data(0x0060_0000, &pins);
        let d = |at: usize| u32::from_le_bytes(data[at..at + 4].try_into().unwrap());
        assert_eq!([d(0), d(4), d(8), d(12)], [2, 0x0060_0000, 2, 0]);
        // The header, two pins of 24 bytes with a type of 16 each.
        let pool = 16 + 2 * (24 + 16);
        assert_eq!(&data[16..20], b"0pi3");
        assert_eq!([d(20), d(24), d(28), d(32), d(36)], [0, 0, 1, 0, 0]);
        assert_eq!(&data[40..44], b"0ty3");
        assert_eq!([d(44), d(48), d(52)], [0, pool as u32, pool as u32 + 16]);
        assert_eq!(&data[56..60], b"1pi3");
        assert_eq!(d(60), REG_PINFLAG_B_OUTPUT);
        // The major type is there once for both pins.
        assert_eq!([d(84), d(88), d(92)], [0, pool as u32, pool as u32 + 32]);
        assert_eq!(data.len(), pool + 48);
        assert_eq!(&data[pool..pool + 16], &MAJOR.0);
        assert_eq!(&data[pool + 16..pool + 32], &MINOR.0);
        assert_eq!(&data[pool + 32..], &[0; 16]);
    }

    #[test]
    fn a_pin_category_and_medium_follow_the_pin() {
        let mut medium = [0; 24];
        medium[..16].copy_from_slice(&MINOR.0);
        medium[16] = 7;
        let pins = [Pin {
            flags: REG_PINFLAG_B_RENDERER,
            instances: 1,
            types: Vec::new(),
            mediums: vec![medium],
            category: Some(MAJOR),
        }];
        let data = filter_data(0x0020_0000, &pins);
        let d = |at: usize| u32::from_le_bytes(data[at..at + 4].try_into().unwrap());
        let pool = 16 + 24 + 4 + 4;
        assert_eq!([d(20), d(24), d(28), d(32), d(36)], [2, 1, 0, 1, 1]);
        assert_eq!(d(40), pool as u32);
        assert_eq!(d(44), pool as u32 + 16);
        assert_eq!(&data[pool..pool + 16], &MAJOR.0);
        assert_eq!(&data[pool + 16..], &medium);
    }
}
