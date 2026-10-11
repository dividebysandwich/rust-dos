//! The modem's phonebook: numbers that dial bulletin board systems on the
//! internet. `ATDT 555-0101` calls the board listed under 5550101, by
//! telnet unless its entry says otherwise. The numbers come from three
//! places, the first that has a number winning:
//!
//! - the configuration's `[phonebook]` section, the user's own
//!   (`5550199 = bbs.example.org:2323 My board`);
//! - rust-dos.com's list (`CATALOG_URL`), fetched when the settings
//!   window's phonebook asks for it and kept in rust-dos's directory for
//!   the next start (`bbs.json`);
//! - the boards built into rust-dos (`BUILT_IN`).
//!
//! The list on the website is a JSON file:
//!
//! ```json
//! { "format": 1, "boards": [ { "number": "555-0101", "name": "Level 29",
//!   "address": "bbs.fozztexx.com:23", "telnet": true } ] }
//! ```

use serde::{Deserialize, Serialize};
use std::sync::RwLock;

/// The list of boards, on the website.
pub const CATALOG_URL: &str = "https://rust-dos.com/bbs.json";

/// The list's format this rust-dos reads.
const FORMAT: u32 = 1;

/// The list fetched last, in rust-dos's directory.
const CACHE: &str = "bbs.json";

/// The biggest list taken, and the most boards in it.
const CATALOG_LIMIT: u64 = 256 * 1024;
const MAX_BOARDS: usize = 500;

/// A number and the board it calls.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Entry {
    /// As it is shown and dialed, with any dashes and spaces.
    pub number: String,
    pub name: String,
    /// `host:port`.
    pub address: String,
    /// Whether the call speaks telnet; most boards do.
    #[serde(default = "yes")]
    pub telnet: bool,
}

fn yes() -> bool {
    true
}

/// Where an entry comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The configuration's `[phonebook]`.
    Yours,
    /// rust-dos.com's list.
    Website,
    BuiltIn,
}

/// Telnet bulletin boards that answered when they went in (October 2026),
/// under numbers from the 555-01xx range kept for fiction.
const BUILT_IN: &[(&str, &str, &str)] = &[
    ("555-0101", "Level 29", "bbs.fozztexx.com:23"),
    ("555-0102", "The Particles! BBS", "particlesbbs.dyndns.org:6400"),
    ("555-0103", "Vertrauen", "vert.synchro.net:23"),
    ("555-0104", "Retro Campus BBS", "bbs.retrocampus.com:23"),
    ("555-0105", "20 For Beers", "20forbeers.com:1337"),
    ("555-0106", "Electronic Chicken BBS", "bbs.electronicchicken.com:23"),
    ("555-0107", "Heatwave BBS", "heatwave.ddns.net:9640"),
    ("555-0108", "Dura-Europos BBS", "dura-bbs.net:6359"),
    ("555-0109", "Absinthe BBS", "absinthebbs.net:1940"),
    ("555-0110", "Xibalba", "xibalba.l33t.codes:44510"),
];

#[derive(Default)]
struct Book {
    yours: Vec<Entry>,
    /// rust-dos.com's list, once read from the cache or fetched.
    website: Option<Vec<Entry>>,
}

static BOOK: RwLock<Book> = RwLock::new(Book { yours: Vec::new(), website: None });

/// Held by tests that change the user's entries, which all tests share.
#[cfg(test)]
pub(crate) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A number's digits, which is what dialing compares: `555-0101` is
/// `5550101`.
pub fn digits(number: &str) -> String {
    number.chars().filter(char::is_ascii_digit).collect()
}

/// The user's entries, from the configuration.
pub fn set_yours(entries: &[Entry]) {
    if let Ok(mut book) = BOOK.write() {
        book.yours = entries.to_vec();
    }
}

/// rust-dos.com's list, as fetched: kept for the next start too.
pub fn set_website(entries: Vec<Entry>) {
    if let Some(path) = cache_path() {
        let text = serde_json::json!({ "format": FORMAT, "boards": entries });
        let _ = std::fs::write(path, text.to_string());
    }
    if let Ok(mut book) = BOOK.write() {
        book.website = Some(entries);
    }
}

fn cache_path() -> Option<std::path::PathBuf> {
    crate::config::user_dir().map(|dir| dir.join(CACHE))
}

/// rust-dos.com's list as it was fetched last, read once.
fn load_cache() {
    let loaded = BOOK.read().map(|book| book.website.is_some()).unwrap_or(true);
    if loaded {
        return;
    }
    let entries = cache_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| parse_catalog(&text).ok())
        .unwrap_or_default();
    if let Ok(mut book) = BOOK.write()
        && book.website.is_none()
    {
        book.website = Some(entries);
    }
}

/// Every number and its board, each number once (the first source's), in
/// order: the user's, the website's, the built-in ones.
pub fn entries() -> Vec<(Entry, Source)> {
    load_cache();
    let Ok(book) = BOOK.read() else { return Vec::new() };
    let built_in = BUILT_IN.iter().map(|&(number, name, address)| Entry {
        number: number.to_string(),
        name: name.to_string(),
        address: address.to_string(),
        telnet: true,
    });
    let all = book
        .yours
        .iter()
        .cloned()
        .map(|e| (e, Source::Yours))
        .chain(book.website.iter().flatten().cloned().map(|e| (e, Source::Website)))
        .chain(built_in.map(|e| (e, Source::BuiltIn)));
    let mut seen = std::collections::HashSet::new();
    all.filter(|(entry, _)| seen.insert(digits(&entry.number))).collect()
}

/// The board `number` dials, if any.
pub fn lookup(number: &str) -> Option<Entry> {
    let wanted = digits(number);
    if wanted.is_empty() {
        return None;
    }
    entries().into_iter().map(|(entry, _)| entry).find(|entry| digits(&entry.number) == wanted)
}

/// A `[phonebook]` line, `number = host[:port] [raw] [name...]`: the port
/// is telnet's (23) unless given, and `raw` makes the call plain TCP.
pub fn parse_line(number: &str, value: &str) -> Result<Entry, String> {
    if digits(number).is_empty() || !number.chars().all(|c| c.is_ascii_digit() || "-() ".contains(c)) {
        return Err(format!("the phonebook number '{}' isn't one (digits, with - if you like)", number));
    }
    let mut words = value.split_whitespace();
    let address = words.next().ok_or_else(|| format!("phonebook number {} has no address", number))?;
    let address = if address.contains(':') { address.to_string() } else { format!("{}:23", address) };
    let mut rest: Vec<&str> = words.collect();
    let raw = rest.first().is_some_and(|w| w.eq_ignore_ascii_case("raw"));
    if raw || rest.first().is_some_and(|w| w.eq_ignore_ascii_case("telnet")) {
        rest.remove(0);
    }
    let name = if rest.is_empty() { address.clone() } else { rest.join(" ") };
    Ok(Entry { number: number.trim().to_string(), name, address, telnet: !raw })
}

#[derive(Deserialize)]
struct Catalog {
    format: u32,
    boards: Vec<serde_json::Value>,
}

/// rust-dos.com's list: the boards in it that make sense.
pub fn parse_catalog(text: &str) -> Result<Vec<Entry>, String> {
    let catalog: Catalog = serde_json::from_str(text).map_err(|e| format!("the list doesn't read: {}", e))?;
    if catalog.format != FORMAT {
        return Err(format!("the list is in format {}; this rust-dos reads {}", catalog.format, FORMAT));
    }
    Ok(catalog
        .boards
        .into_iter()
        .take(MAX_BOARDS)
        .filter_map(|value| serde_json::from_value::<Entry>(value).ok())
        .filter(|entry| {
            !digits(&entry.number).is_empty()
                && !entry.name.trim().is_empty()
                && entry.address.rsplit_once(':').is_some_and(|(host, port)| !host.is_empty() && port.parse::<u16>().is_ok())
        })
        .collect())
}

/// Ask the website for its list. It blocks: run it on a thread of its
/// own.
#[cfg(all(feature = "sdl", not(target_arch = "wasm32")))]
pub fn fetch_catalog() -> Result<Vec<Entry>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(30)))
        .user_agent(format!("rust-dos/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let text = agent
        .get(CATALOG_URL)
        .call()
        .and_then(|mut response| response.body_mut().with_config().limit(CATALOG_LIMIT).read_to_string())
        .map_err(|e| format!("rust-dos.com didn't answer: {}", e))?;
    parse_catalog(&text)
}

#[cfg(not(all(feature = "sdl", not(target_arch = "wasm32"))))]
pub fn fetch_catalog() -> Result<Vec<Entry>, String> {
    let _ = CATALOG_LIMIT;
    Err("the list is only fetched by the rust-dos program".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_takes_a_port_raw_and_a_name() {
        let entry = parse_line("555-0199", "bbs.example.org Example Board").unwrap();
        assert_eq!(entry.address, "bbs.example.org:23");
        assert_eq!(entry.name, "Example Board");
        assert!(entry.telnet);

        let entry = parse_line("5550198", "10.0.0.2:2323 raw").unwrap();
        assert_eq!((entry.address.as_str(), entry.name.as_str(), entry.telnet), ("10.0.0.2:2323", "10.0.0.2:2323", false));

        assert!(parse_line("abc", "host").is_err());
        assert!(parse_line("123", "").is_err());
    }

    #[test]
    fn the_user_s_numbers_come_before_the_built_in_ones() {
        let _lock = TEST_LOCK.lock();
        assert_eq!(lookup("555 0103").unwrap().name, "Vertrauen");
        assert!(lookup("").is_none());
        set_yours(&[parse_line("555-0103", "localhost:2323 raw Mine").unwrap()]);
        let entry = lookup("5550103").unwrap();
        set_yours(&[]);
        assert_eq!((entry.name.as_str(), entry.telnet), ("Mine", false));
    }

    #[test]
    fn the_website_s_list_keeps_the_boards_that_make_sense() {
        let text = r#"{ "format": 1, "boards": [
            { "number": "555-0300", "name": "Good", "address": "bbs.example.org:23" },
            { "number": "555-0301", "name": "No port", "address": "bbs.example.org" },
            { "number": "", "name": "No number", "address": "a:1" },
            { "number": "555-0302", "name": "Raw", "address": "b:2", "telnet": false } ] }"#;
        let boards = parse_catalog(text).unwrap();
        assert_eq!(boards.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(), ["Good", "Raw"]);
        assert!(boards[0].telnet && !boards[1].telnet);
        assert!(parse_catalog(r#"{ "format": 2, "boards": [] }"#).is_err());
    }
}
