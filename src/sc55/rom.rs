//! The Sound Canvas's ROMs: which sets there are, finding them by their
//! SHA-256 hashes whatever the files are called (in folders, or in zip
//! archives as they are usually passed around), and loading them. The
//! table is the one in Nuked-SC55's `standard_romsets.cpp`, without the
//! JV-880, which is not a GS module.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

/// Where in the module a ROM goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Location {
    /// The H8/532's code.
    Rom1,
    /// More of its code and data.
    Rom2,
    /// The sub-MCU (M37450) of the mk2 and ST.
    SmRom,
    Wave1,
    Wave2,
    Wave3,
}

pub const LOCATIONS: usize = 6;

impl Location {
    fn index(self) -> usize {
        self as usize
    }

    pub fn is_wave(self) -> bool {
        matches!(self, Location::Wave1 | Location::Wave2 | Location::Wave3)
    }

    /// The largest the ROM can be.
    pub fn max_size(self) -> usize {
        match self {
            Location::Rom1 => 0x8000,
            Location::Rom2 => 0x80000,
            Location::SmRom => 0x1000,
            Location::Wave1 | Location::Wave2 => 0x200000,
            Location::Wave3 => 0x100000,
        }
    }
}

/// A line of hardware: the Nuked-SC55 name for it is what `sc55model`
/// takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    Mk2,
    St,
    Mk1,
    Cm300,
    Scb55,
    Rlp3237,
    Sc155,
    Sc155Mk2,
}

impl Family {
    pub fn name(self) -> &'static str {
        match self {
            Family::Mk2 => "mk2",
            Family::St => "st",
            Family::Mk1 => "mk1",
            Family::Cm300 => "cm300",
            Family::Scb55 => "scb55",
            Family::Rlp3237 => "rlp3237",
            Family::Sc155 => "sc155",
            Family::Sc155Mk2 => "sc155mk2",
        }
    }

    /// The module's name, for people.
    pub fn display_name(self) -> &'static str {
        match self {
            Family::Mk2 => "SC-55mkII",
            Family::St => "SC-55ST",
            Family::Mk1 => "SC-55",
            Family::Cm300 => "CM-300/SCC-1",
            Family::Scb55 => "SCB-55",
            Family::Rlp3237 => "RLP-3237",
            Family::Sc155 => "SC-155",
            Family::Sc155Mk2 => "SC-155mkII",
        }
    }

    /// The modules of the first generation, which have no sub-MCU and
    /// another memory map.
    pub fn is_mk1(self) -> bool {
        matches!(self, Family::Mk1 | Family::Cm300 | Family::Sc155)
    }

    /// Where it needs a ROM.
    pub fn needs(self, location: Location) -> bool {
        use Location::*;
        match self {
            Family::Mk2 | Family::St | Family::Sc155Mk2 => matches!(location, Rom1 | Rom2 | SmRom | Wave1 | Wave2),
            Family::Mk1 | Family::Cm300 | Family::Sc155 => matches!(location, Rom1 | Rom2 | Wave1 | Wave2 | Wave3),
            Family::Scb55 => matches!(location, Rom1 | Rom2 | Wave1 | Wave3),
            Family::Rlp3237 => matches!(location, Rom1 | Rom2 | Wave1),
        }
    }
}

/// One version of a family's ROMs.
#[derive(Debug, PartialEq, Eq)]
pub struct Romset {
    /// `family-version`, as Nuked-SC55 names it.
    pub name: &'static str,
    pub family: Family,
    pub roms: &'static [(Location, &'static str)],
}

impl Romset {
    pub fn by_name(name: &str) -> Option<&'static Romset> {
        ROMSETS.iter().find(|r| r.name.eq_ignore_ascii_case(name))
    }

    /// Its name for people: "SC-55 v1.21".
    pub fn display_name(&self) -> String {
        let version = self.name.split_once('-').map_or("", |(_, v)| v);
        format!("{} {}", self.family.display_name(), version)
    }
}

const MK2_ROM1: &str = "8a1eb33c7599b746c0c50283e4349a1bb1773b5c0ec0e9661219bf6c067d2042";
const MK2_SMROM: &str = "b0b5f865a403f7308b4be8d0ed3ba2ed1c22db881b8a8326769dea222f6431d8";
const MK2_WAVE1: &str = "c6429e21b9b3a02fbd68ef0b2053668433bee0bccd537a71841bc70b8874243b";
const MK2_WAVE2: &str = "5b753f6cef4cfc7fcafe1430fecbb94a739b874e55356246a46abe24097ee491";
const MK1_WAVE1: &str = "5655509a531804f97ea2d7ef05b8fec20ebf46216b389a84c44169257a4d2007";
const MK1_WAVE2: &str = "c655b159792d999b90df9e4fa782cf56411ba1eaa0bb3ac2bdaf09e1391006b1";
const MK1_WAVE3: &str = "334b2d16be3c2362210fdbec1c866ad58badeb0f84fd9bf5d0ac599baf077cc2";
const CM300_WAVE1: &str = "40c093cbfb4441a5c884e623f882a80b96b2527f9fd431e074398d206c0f073d";
const CM300_WAVE2: &str = "9bbbcac747bd6f7a2693f4ef10633db8ab626f17d3d9c47c83c3839d4dd2f613";

use Location::*;

pub static ROMSETS: &[Romset] = &[
    Romset {
        name: "mk2-v1.01",
        family: Family::Mk2,
        roms: &[
            (Rom1, MK2_ROM1),
            (Rom2, "a4c9fd821059054c7e7681d61f49ce6f42ed2fe407a7ec1ba0dfdc9722582ce0"),
            (SmRom, MK2_SMROM),
            (Wave1, MK2_WAVE1),
            (Wave2, MK2_WAVE2),
        ],
    },
    Romset {
        name: "sc155mk2-v1.01",
        family: Family::Sc155Mk2,
        roms: &[
            (Rom1, MK2_ROM1),
            (Rom2, "a4c9fd821059054c7e7681d61f49ce6f42ed2fe407a7ec1ba0dfdc9722582ce0"),
            (SmRom, MK2_SMROM),
            (Wave1, MK2_WAVE1),
            (Wave2, MK2_WAVE2),
        ],
    },
    Romset {
        name: "st-v1.01",
        family: Family::St,
        roms: &[
            (Rom1, MK2_ROM1),
            (Rom2, "03517ac0a3b1ad8b69a1a4ee045e0c21da0170027bd1ba1bd3cf72cd017bbe6a"),
            (SmRom, MK2_SMROM),
            (Wave1, MK2_WAVE1),
            (Wave2, MK2_WAVE2),
        ],
    },
    Romset {
        name: "mk1-v1.00",
        family: Family::Mk1,
        roms: &[
            (Rom1, "b4ecf44bc0520322b0d114d397951d3bf92ca6fa51d0d27b2407df58a6be2efe"),
            (Rom2, "014e2e21ea30de7a1e4f1cdea14dd9a719960535e257a9e40e98dbb1a5870226"),
            (Wave1, MK1_WAVE1),
            (Wave2, MK1_WAVE2),
            (Wave3, MK1_WAVE3),
        ],
    },
    Romset {
        name: "mk1-v1.10",
        family: Family::Mk1,
        roms: &[
            (Rom1, "2fe88ec39f3ef4b1de8cdf74527419467975c47f7aacfcd07605e01d54bd89b5"),
            (Rom2, "ec064d6c4fc70ec990911089d966043cb819fba0e26e6f6afdd0a05e5301b91b"),
            (Wave1, MK1_WAVE1),
            (Wave2, MK1_WAVE2),
            (Wave3, MK1_WAVE3),
        ],
    },
    Romset {
        name: "mk1-v1.20",
        family: Family::Mk1,
        roms: &[
            (Rom1, "7e1bacd1d7c62ed66e465ba05597dcd60dfc13fc23de0287fdbce6cf906c6544"),
            (Rom2, "22ce6ca59e6332143b335525e81fab501ea6fccce4b7e2f3bfc2cc8bf6612ff6"),
            (Wave1, MK1_WAVE1),
            (Wave2, MK1_WAVE2),
            (Wave3, MK1_WAVE3),
        ],
    },
    Romset {
        name: "mk1-v1.21",
        family: Family::Mk1,
        roms: &[
            (Rom1, "7e1bacd1d7c62ed66e465ba05597dcd60dfc13fc23de0287fdbce6cf906c6544"),
            (Rom2, "effc6132d68f7e300aaef915ccdd08aba93606c22d23e580daf9ea6617913af1"),
            (Wave1, MK1_WAVE1),
            (Wave2, MK1_WAVE2),
            (Wave3, MK1_WAVE3),
        ],
    },
    Romset {
        name: "mk1-v2.00",
        family: Family::Mk1,
        roms: &[
            (Rom1, "24a65c97cdbaa847d6f59193523ce63c73394b4b693a6517ee79441f2fb8a3ee"),
            (Rom2, "f5dac35d450ab986570a209dff3816eec75cee669e161f54b51224b467dd0bcc"),
            (Wave1, MK1_WAVE1),
            (Wave2, MK1_WAVE2),
            (Wave3, MK1_WAVE3),
        ],
    },
    Romset {
        name: "cm300-v1.10",
        family: Family::Cm300,
        roms: &[
            (Rom1, "72ed35481efbf25b3c492b83183655d17a3b266ecb30ffbc6dc977e6a8d261b2"),
            (Rom2, "0283d32e6993a0265710c4206463deb937b0c3a4819b69f471a0eca5865719f9"),
            (Wave1, CM300_WAVE1),
            (Wave2, CM300_WAVE2),
            (Wave3, MK2_WAVE2),
        ],
    },
    Romset {
        name: "cm300-v1.20",
        family: Family::Cm300,
        roms: &[
            (Rom1, "72ed35481efbf25b3c492b83183655d17a3b266ecb30ffbc6dc977e6a8d261b2"),
            (Rom2, "fef1acb1969525d66238be5e7811108919b07a4df5fbab656ad084966373483f"),
            (Wave1, CM300_WAVE1),
            (Wave2, CM300_WAVE2),
            (Wave3, MK2_WAVE2),
        ],
    },
    Romset {
        name: "cm300-v1.30",
        family: Family::Cm300,
        roms: &[
            (Rom1, "9ec66abb5231b6c6f46f48b33d5412703041037d69a6803626ac402f25552af2"),
            (Rom2, "f89442734fdebacae87c7707c01b2d7fdbf5940abae738987aee912d34b5882e"),
            (Wave1, CM300_WAVE1),
            (Wave2, CM300_WAVE2),
            (Wave3, MK2_WAVE2),
        ],
    },
    Romset {
        name: "scb55-v2.00",
        family: Family::Scb55,
        roms: &[
            (Rom1, "00df835d3f97fc8b0059db63f36d608eec2bfd1f51ad54eb5af52c868c1111b1"),
            (Rom2, "541be4d0b1ef0d07bb042ba67ffd099c8a5d746aac4cd24ce8842c034379f213"),
            (Wave1, MK2_WAVE1),
            (Wave3, MK2_WAVE2),
        ],
    },
    Romset {
        name: "rlp3237-v2.01",
        family: Family::Rlp3237,
        roms: &[
            (Rom1, "00df835d3f97fc8b0059db63f36d608eec2bfd1f51ad54eb5af52c868c1111b1"),
            (Rom2, "e0a3d6d9b05e82374a0d289901273ce560ce1ead86459c75f844158b32d204a9"),
            (Wave1, "dae2a8bc0fd3bcaf3f5e3ab6c4c6fd30e2663bf26ca17afe52924874c0afc4e2"),
        ],
    },
    Romset {
        name: "sc155-rev1",
        family: Family::Sc155,
        roms: &[
            (Rom1, "24a65c97cdbaa847d6f59193523ce63c73394b4b693a6517ee79441f2fb8a3ee"),
            (Rom2, "ceb7b9d3d9d264efe5dc3ba992b94f3be35eb6d0451abc574b6f6b5dc3db237b"),
            (Wave1, MK1_WAVE1),
            (Wave2, MK1_WAVE2),
            (Wave3, MK1_WAVE3),
        ],
    },
];

/// The sets `auto` tries first: the SC-55 most games were written for,
/// then the mkII. The others follow in the table's order.
const PREFERRED: [&str; 2] = ["mk1-v1.21", "mk2-v1.01"];

/// Whether `model` (`auto`, a family or a `family-version`) names
/// something.
pub fn valid_model(model: &str) -> bool {
    model.eq_ignore_ascii_case("auto")
        || Romset::by_name(model).is_some()
        || ROMSETS.iter().any(|r| r.family.name().eq_ignore_ascii_case(model))
}

/// The sets `model` accepts, the preferred first.
pub fn candidates(model: &str) -> Vec<&'static Romset> {
    let mut order: Vec<&'static Romset> = PREFERRED.iter().filter_map(|n| Romset::by_name(n)).collect();
    order.extend(ROMSETS.iter().filter(|r| !PREFERRED.contains(&r.name)));
    order.retain(|r| {
        model.eq_ignore_ascii_case("auto")
            || r.name.eq_ignore_ascii_case(model)
            || r.family.name().eq_ignore_ascii_case(model)
    });
    order
}

/// Directories a frontend adds to the search, such as a libretro
/// frontend's system directory.
static SEARCH_DIRS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Look in `dir` too, before the usual places.
pub fn add_search_dir(dir: PathBuf) {
    let mut dirs = SEARCH_DIRS.lock().unwrap_or_else(|e| e.into_inner());
    if !dirs.contains(&dir) {
        dirs.push(dir);
    }
}

/// The folder in rust-dos's own directory, where the download goes.
pub fn download_dir() -> Option<PathBuf> {
    crate::config::user_dir().map(|dir| dir.join("sc55-roms"))
}

/// Where DOSBox Staging keeps its Sound Canvas ROMs.
fn dosbox_staging_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    return std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("DOSBox").join("soundcanvas-roms"));
    #[cfg(target_os = "macos")]
    return dirs::home_dir().map(|h| h.join("Library/Preferences/DOSBox/soundcanvas-roms"));
    #[cfg(not(any(windows, target_os = "macos")))]
    return crate::hostdirs::config_dir().map(|d| d.join("dosbox").join("soundcanvas-roms"));
}

/// Where the ROMs are looked for without an `sc55roms` setting: the
/// frontend's directories, rust-dos's own (where the download goes),
/// DOSBox Staging's and the working directory. Each is searched with
/// its subfolders, and zip archives in them.
pub fn default_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for dir in SEARCH_DIRS.lock().unwrap_or_else(|e| e.into_inner()).iter() {
        dirs.push(dir.join("sc55-roms"));
        dirs.push(dir.join("soundcanvas-roms"));
    }
    dirs.extend(download_dir());
    dirs.extend(dosbox_staging_dir());
    dirs.push(PathBuf::from("sc55-roms"));
    dirs
}

/// Where a ROM file is: a file, or a file in a zip archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    File(PathBuf),
    Zipped(PathBuf, String),
}

impl Source {
    fn read(&self) -> Result<Vec<u8>, String> {
        match self {
            Source::File(path) => std::fs::read(path).map_err(|e| format!("{}: {}", path.display(), e)),
            Source::Zipped(zip, name) => {
                let mut file = std::fs::File::open(zip).map_err(|e| format!("{}: {}", zip.display(), e))?;
                let entries = crate::archive::zip::central_directory(&mut file)?;
                let entry = entries
                    .iter()
                    .find(|e| &e.name() == name)
                    .ok_or_else(|| format!("{}: no {}", zip.display(), name))?;
                crate::archive::zip::read(&mut file, entry)
            }
        }
    }
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Source::File(path) => write!(f, "{}", path.display()),
            Source::Zipped(zip, name) => write!(f, "{} in {}", name, zip.display()),
        }
    }
}

/// A complete set found on disk.
#[derive(Clone, Debug)]
pub struct Found {
    pub romset: &'static Romset,
    pub sources: Vec<(Location, Source)>,
}

impl Found {
    /// The folder (or archive) it is in, for the log.
    pub fn place(&self) -> String {
        let path = match &self.sources.first().map(|s| &s.1) {
            Some(Source::File(path)) => path.parent().unwrap_or(Path::new("")).to_path_buf(),
            Some(Source::Zipped(zip, _)) => zip.clone(),
            None => PathBuf::new(),
        };
        path.display().to_string()
    }
}

/// Files bigger than the biggest ROM aren't hashed, nor archives bigger
/// than a set of them could be.
const MAX_ROM: u64 = 0x200000;
const MAX_ZIP: u64 = 64 << 20;
/// How deep below a search directory to look.
const MAX_DEPTH: usize = 3;

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|b| format!("{:02x}", b)).collect()
}

/// The hashes of the files (or the files in an archive) at a path, kept
/// while the file's size and time stay the same: the settings window
/// asks often.
type Hashes = Vec<(String, Option<String>)>;
type HashCache = HashMap<PathBuf, (u64, Option<SystemTime>, Hashes)>;
static HASH_CACHE: Mutex<Option<HashCache>> = Mutex::new(None);

/// The hashes of the ROM-sized files at `path`: (hash, member of the
/// archive).
fn hashes_of(path: &Path, len: u64, modified: Option<SystemTime>) -> Hashes {
    {
        let cache = HASH_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((l, m, hashes)) = cache.as_ref().and_then(|c| c.get(path))
            && *l == len
            && *m == modified
        {
            return hashes.clone();
        }
    }
    let is_zip = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip"));
    let mut hashes = Vec::new();
    if is_zip && len <= MAX_ZIP {
        if let Ok(mut file) = std::fs::File::open(path)
            && let Ok(entries) = crate::archive::zip::central_directory(&mut file)
        {
            for entry in entries.iter().filter(|e| !e.is_dir() && e.size <= MAX_ROM) {
                if let Ok(bytes) = crate::archive::zip::read(&mut file, entry) {
                    hashes.push((sha256_hex(&bytes), Some(entry.name())));
                }
            }
        }
    } else if len <= MAX_ROM
        && let Ok(bytes) = std::fs::read(path)
    {
        hashes.push((sha256_hex(&bytes), None));
    }
    let mut cache = HASH_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    cache.get_or_insert_with(HashMap::new).insert(path.to_path_buf(), (len, modified, hashes.clone()));
    hashes
}

/// The ROM files below `dir`, by hash.
fn collect(dir: &Path, depth: usize, found: &mut HashMap<String, Source>) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = read.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        let Ok(meta) = std::fs::metadata(&path) else { continue };
        if meta.is_dir() {
            if depth < MAX_DEPTH {
                collect(&path, depth + 1, found);
            }
            continue;
        }
        if meta.len() < 0x1000 {
            continue;
        }
        for (hash, member) in hashes_of(&path, meta.len(), meta.modified().ok()) {
            let source = match member {
                Some(name) => Source::Zipped(path.clone(), name),
                None => Source::File(path.clone()),
            };
            found.entry(hash).or_insert(source);
        }
    }
}

/// The complete sets in `dirs`, in the order `model` prefers them.
pub fn scan(dirs: &[PathBuf], model: &str) -> Vec<Found> {
    let mut files = HashMap::new();
    for dir in dirs {
        collect(dir, 0, &mut files);
    }
    complete_sets(&files, model)
}

fn complete_sets(files: &HashMap<String, Source>, model: &str) -> Vec<Found> {
    candidates(model)
        .into_iter()
        .filter_map(|romset| {
            let sources: Option<Vec<_>> =
                romset.roms.iter().map(|(loc, hash)| files.get(*hash).map(|s| (*loc, s.clone()))).collect();
            sources.map(|sources| Found { romset, sources })
        })
        .collect()
}

/// The set to play: the first that `model` accepts in the configured
/// directory, or else in the default ones.
pub fn find(configured: Option<&Path>, model: &str) -> Option<Found> {
    let dirs = match configured {
        Some(dir) => vec![dir.to_path_buf()],
        None => default_dirs(),
    };
    scan(&dirs, model).into_iter().next()
}

/// What `find` found lately, and when: the settings window asks every
/// frame, and looking through folders isn't free.
type Lately = (std::time::Instant, Option<PathBuf>, String, Option<Found>);
static FIND_CACHE: Mutex<Option<Lately>> = Mutex::new(None);

/// `find`, its answer kept for a couple of seconds.
pub fn find_cached(configured: Option<&Path>, model: &str) -> Option<Found> {
    let mut cache = FIND_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((when, dir, m, found)) = cache.as_ref()
        && when.elapsed() < std::time::Duration::from_secs(2)
        && dir.as_deref() == configured
        && m == model
    {
        return found.clone();
    }
    let found = find(configured, model);
    *cache = Some((std::time::Instant::now(), configured.map(Path::to_path_buf), model.to_string(), found.clone()));
    found
}

/// Look again next time: ROMs were added.
pub fn forget() {
    *FIND_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// A set's contents, as the module has them: the wave ROMs unscrambled.
pub struct Loaded {
    pub romset: &'static Romset,
    pub roms: [Vec<u8>; LOCATIONS],
}

impl Loaded {
    pub fn get(&self, location: Location) -> &[u8] {
        &self.roms[location.index()]
    }
}

/// Read a found set, checking it again.
pub fn load(found: &Found) -> Result<Loaded, String> {
    let mut roms: [Vec<u8>; LOCATIONS] = Default::default();
    for (location, source) in &found.sources {
        let bytes = source.read()?;
        let expected = found.romset.roms.iter().find(|(l, _)| l == location).map(|r| r.1);
        if expected != Some(sha256_hex(&bytes).as_str()) {
            return Err(format!("{}: changed since it was found", source));
        }
        roms[location.index()] = prepare(*location, bytes)?;
    }
    Ok(Loaded { romset: found.romset, roms })
}

/// Load a set from the bytes of its files, as the tests and the
/// download have them.
pub fn from_bytes(romset: &'static Romset, files: &[Vec<u8>]) -> Result<Loaded, String> {
    let mut roms: [Vec<u8>; LOCATIONS] = Default::default();
    for (location, hash) in romset.roms {
        let bytes = files
            .iter()
            .find(|b| sha256_hex(b) == *hash)
            .ok_or_else(|| format!("{}: no {:?} ROM", romset.name, location))?;
        roms[location.index()] = prepare(*location, bytes.clone())?;
    }
    Ok(Loaded { romset, roms })
}

fn prepare(location: Location, bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    if bytes.len() > location.max_size() {
        return Err(format!("{:?} ROM: {} bytes, more than the {} it can be", location, bytes.len(), location.max_size()));
    }
    if location == Location::Rom2 && !bytes.len().is_power_of_two() {
        return Err(format!("ROM2: {} bytes, not a power of two", bytes.len()));
    }
    Ok(if location.is_wave() { unscramble(&bytes) } else { bytes })
}

/// The wave ROMs' address and data lines are wired out of order.
pub fn unscramble(src: &[u8]) -> Vec<u8> {
    const AA: [u32; 20] = [2, 0, 3, 4, 1, 9, 13, 10, 18, 17, 6, 15, 11, 16, 8, 5, 12, 7, 14, 19];
    const DD: [u32; 8] = [2, 0, 4, 5, 7, 6, 3, 1];
    let mut data_map = [0u8; 256];
    for (raw, out) in data_map.iter_mut().enumerate() {
        for (j, &d) in DD.iter().enumerate() {
            if raw & (1 << d) != 0 {
                *out |= 1 << j;
            }
        }
    }
    (0..src.len())
        .map(|i| {
            let mut address = i & !0xfffff;
            for (j, &a) in AA.iter().enumerate() {
                if i & (1 << j) != 0 {
                    address |= 1 << a;
                }
            }
            data_map[src[address] as usize]
        })
        .collect()
}

/// The sets in a zip archive's bytes, as the download has them: (set,
/// file name, bytes) of each file that is one of a set's ROMs.
pub fn roms_in_zip(zip: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut cursor = Cursor::new(zip);
    let entries = crate::archive::zip::central_directory(&mut cursor)?;
    let known: Vec<&str> = ROMSETS.iter().flat_map(|r| r.roms.iter().map(|x| x.1)).collect();
    let mut out = Vec::new();
    for entry in entries.iter().filter(|e| !e.is_dir() && e.size <= MAX_ROM) {
        let bytes = crate::archive::zip::read(&mut cursor, entry)?;
        if known.contains(&sha256_hex(&bytes).as_str()) {
            let name = entry.name().rsplit('/').next().unwrap_or_default().to_string();
            out.push((name, bytes));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_set_has_the_roms_its_family_needs() {
        for set in ROMSETS {
            for loc in [Rom1, Rom2, SmRom, Wave1, Wave2, Wave3] {
                assert_eq!(
                    set.family.needs(loc),
                    set.roms.iter().any(|(l, _)| *l == loc),
                    "{} {:?}",
                    set.name,
                    loc
                );
            }
            assert!(set.roms.iter().all(|(_, h)| h.len() == 64));
        }
    }

    #[test]
    fn models_choose_sets() {
        assert_eq!(candidates("auto")[0].name, "mk1-v1.21");
        assert_eq!(candidates("auto")[1].name, "mk2-v1.01");
        assert_eq!(candidates("auto").len(), ROMSETS.len());
        assert_eq!(candidates("MK1")[0].name, "mk1-v1.21");
        assert!(candidates("mk1").iter().all(|r| r.family == Family::Mk1));
        assert_eq!(candidates("cm300-v1.20").len(), 1);
        assert!(valid_model("sc155mk2") && valid_model("mk2-v1.01") && valid_model("auto"));
        assert!(!valid_model("jv880") && !valid_model("mt32"));
    }

    #[test]
    fn unscrambling_moves_address_and_data_bits() {
        // Bit j of an address comes from bit AA[j] of the scrambled one,
        // and bit j of the data from bit DD[j].
        let mut src = vec![0u8; 0x100000];
        src[1 << 2] = 1 << 2; // address bit 0, data bit 0
        let out = unscramble(&src);
        assert_eq!(out[1], 1);
        assert_eq!(out.iter().filter(|&&b| b != 0).count(), 1);
    }

    #[test]
    fn sets_are_found_by_hash_in_folders_and_zips() {
        // Fake ROMs whose hashes stand in for a set's.
        let dir = std::env::temp_dir().join(format!("rust-dos-sc55-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let files: Vec<Vec<u8>> = (0..3u8).map(|i| vec![i; 0x2000]).collect();
        std::fs::write(dir.join("sub/a.bin"), &files[0]).unwrap();
        let zip = crate::archive::zip::tests::zip(&[("x/b.bin", &files[1], true), ("c.bin", &files[2], false)]);
        std::fs::write(dir.join("set.ZIP"), zip).unwrap();
        let mut found = HashMap::new();
        collect(&dir, 0, &mut found);
        let hash = |b: &[u8]| sha256_hex(b);
        assert_eq!(found.get(&hash(&files[0])), Some(&Source::File(dir.join("sub/a.bin"))));
        assert_eq!(found.get(&hash(&files[1])), Some(&Source::Zipped(dir.join("set.ZIP"), "x/b.bin".to_string())));
        assert_eq!(Source::Zipped(dir.join("set.ZIP"), "c.bin".to_string()).read().unwrap(), files[2]);
        // No set is complete with these.
        assert!(complete_sets(&found, "auto").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
