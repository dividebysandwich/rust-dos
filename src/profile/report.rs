//! What a profile found, as JSON, as folded stacks for flame graphs
//! (inferno, speedscope), and as a short text table.

use std::collections::HashMap;

use serde_json::{Value, json};

use super::{Engine, Entry, Loc, Thunk, ZONES, Zone};

/// What a report shows.
#[derive(Clone, Debug)]
pub struct Query {
    /// The guest places, ports and services listed.
    pub top: usize,
    /// Only samples in this zone, or in the zones of this group (`io`,
    /// `hle`, ...), for the places and the folded stacks.
    pub zone: Option<String>,
    /// Count guest code by blocks of this many bytes (a power of two), 1
    /// for each instruction.
    pub bucket: u32,
    /// Lines of disassembly for each place.
    pub disasm: usize,
    /// The samples by zone for each second.
    pub timeline: bool,
}

impl Default for Query {
    fn default() -> Self {
        Query { top: 25, zone: None, bucket: 1, disasm: 4, timeline: false }
    }
}

impl Query {
    /// Whether a sample in `zone` counts for the query.
    pub fn takes(&self, zone: Zone) -> bool {
        match &self.zone {
            None => true,
            Some(want) => zone.name() == want || zone.group() == want,
        }
    }

    /// Check what the query names.
    pub fn check(&self) -> Result<(), String> {
        if let Some(z) = &self.zone
            && !Zone::ALL.iter().any(|&zone| zone.name() == z || zone.group() == z)
        {
            let names: Vec<_> = Zone::ALL.iter().map(|z| z.name()).collect();
            return Err(format!("no zone or group '{}'; the zones are {}", z, names.join(", ")));
        }
        if !self.bucket.is_power_of_two() || self.bucket > 1 << 20 {
            return Err(format!("bucket {} isn't a power of two up to 1048576", self.bucket));
        }
        Ok(())
    }
}

/// A share of `whole`, in percent, to two places.
pub fn pct(part: u64, whole: u64) -> f64 {
    if whole == 0 { 0.0 } else { (part as f64 * 10000.0 / whole as f64).round() / 100.0 }
}

/// A guest place and its samples, by zone. With buckets, `hottest` is the
/// instruction in it with the most samples, which its disassembly starts
/// at (the bucket's start can be inside an instruction).
#[derive(Clone, Debug)]
pub struct Place {
    pub loc: Loc,
    pub hottest: Loc,
    pub samples: u64,
    pub zones: Vec<(Zone, u64)>,
}

/// The samples by zone.
pub fn by_zone(counts: &HashMap<u64, u64>) -> [u64; ZONES] {
    let mut zones = [0u64; ZONES];
    for (&word, &n) in counts {
        zones[Loc(word).zone() as usize] += n;
    }
    zones
}

/// The guest places with the most samples the query takes, most first.
pub fn places(counts: &HashMap<u64, u64>, query: &Query) -> Vec<Place> {
    let mut by: HashMap<Loc, (u64, [u64; ZONES])> = HashMap::new();
    let mut exact: HashMap<Loc, u64> = HashMap::new();
    for (&word, &n) in counts {
        let loc = Loc(word);
        if loc.cs_eip().is_none() || !query.takes(loc.zone()) {
            continue;
        }
        let place = loc.bucketed(query.bucket).place();
        let entry = by.entry(place).or_insert((0, [0; ZONES]));
        entry.0 += n;
        entry.1[loc.zone() as usize] += n;
        if query.bucket > 1 {
            *exact.entry(loc.place()).or_insert(0) += n;
        }
    }
    // The hottest instruction of each bucket.
    let mut hottest: HashMap<Loc, (u64, Loc)> = HashMap::new();
    for (&loc, &n) in &exact {
        let best = hottest.entry(loc.bucketed(query.bucket)).or_insert((0, loc));
        if (n, std::cmp::Reverse(loc.0)) > (best.0, std::cmp::Reverse(best.1.0)) {
            *best = (n, loc);
        }
    }
    let mut places: Vec<Place> = by
        .into_iter()
        .map(|(loc, (samples, zones))| {
            let mut zones: Vec<(Zone, u64)> =
                zones.iter().enumerate().filter(|&(_, &n)| n > 0).map(|(z, &n)| (Zone::ALL[z], n)).collect();
            zones.sort_by_key(|&(z, n)| (std::cmp::Reverse(n), z));
            let hottest = hottest.get(&loc).map_or(loc, |&(_, h)| h);
            Place { loc, hottest, samples, zones }
        })
        .collect();
    places.sort_by_key(|p| (std::cmp::Reverse(p.samples), p.loc.0));
    places
}

/// The samples by code segment (CS and mode), most first.
pub fn segments(counts: &HashMap<u64, u64>, query: &Query) -> Vec<((u16, super::Mode), u64)> {
    let mut by: HashMap<(u16, super::Mode), u64> = HashMap::new();
    for (&word, &n) in counts {
        let loc = Loc(word);
        if let Some((cs, _)) = loc.cs_eip()
            && query.takes(loc.zone())
        {
            *by.entry((cs, loc.mode())).or_insert(0) += n;
        }
    }
    let mut v: Vec<_> = by.into_iter().collect();
    v.sort_by_key(|&(k, n)| (std::cmp::Reverse(n), k));
    v
}

/// One sample's stack, root first: the engine, the guest place and the
/// zone inside it; or the front end's zone alone.
pub fn stack(loc: Loc) -> Vec<String> {
    match loc.cs_eip() {
        None => vec![loc.zone().name().to_string()],
        Some(_) => {
            let engine = loc.engine();
            let mut frames = vec![engine.name().to_string(), format!("{} {}", loc.mode().name(), loc.addr_text().unwrap_or_default())];
            let own = if engine == Engine::Jit { Zone::Jit } else { Zone::Interp };
            if loc.zone() != own {
                frames.push(loc.zone().name().to_string());
            }
            frames
        }
    }
}

/// The Win32 code a `Thunk` holds, root first: the farthest caller up the
/// EBP chain first, the place of the switch to 16-bit code last.
pub fn win32_frames(thunk: &Thunk, bucket: u32) -> Vec<Loc> {
    thunk.iter().rev().filter(|&&w| w != 0).map(|&w| Loc(w).bucketed(bucket)).collect()
}

/// The frames a chained sample's stack starts with, root first: the Win32
/// code that switched to 16-bit code (its callers, then the switch), then
/// the place that entered ring 0 and how.
pub fn chain_frames(entry: u64, thunk: &Thunk, bucket: u32) -> Vec<String> {
    let win32 = win32_frames(thunk, bucket);
    let mut frames: Vec<String> = win32
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let text = format!("{} {}", l.mode().name(), l.addr_text().unwrap_or_default());
            if i + 1 == win32.len() { text + " (thunk)" } else { text }
        })
        .collect();
    if let Some(e) = Entry::of(entry) {
        let place = e.bucketed(bucket).place();
        frames.push(format!("{} {} ({})", place.mode().name(), place.addr_text().unwrap_or_default(), e.via()));
    }
    frames
}

/// The samples as folded stacks, one `frame;frame;... count` line each, for
/// inferno's or speedscope's flame graphs. The front end's zones are a
/// stack of one frame. A chained sample's stack starts with the code it is
/// charged to (`chain_frames`).
pub fn folded(counts: &HashMap<u64, u64>, chains: &HashMap<(u64, u64, Thunk), u64>, query: &Query) -> String {
    let mut by: HashMap<String, u64> = HashMap::new();
    let mut chained: HashMap<u64, u64> = HashMap::new();
    for (&(word, entry, thunk), &n) in chains {
        *chained.entry(word).or_insert(0) += n;
        let loc = Loc(word);
        if !query.takes(loc.zone()) {
            continue;
        }
        let mut frames = chain_frames(entry, &thunk, query.bucket);
        frames.extend(stack(loc.bucketed(query.bucket)));
        *by.entry(frames.join(";")).or_insert(0) += n;
    }
    for (&word, &n) in counts {
        let loc = Loc(word);
        let n = n.saturating_sub(chained.get(&word).copied().unwrap_or(0));
        if n == 0 || !query.takes(loc.zone()) {
            continue;
        }
        *by.entry(stack(loc.bucketed(query.bucket)).join(";")).or_insert(0) += n;
    }
    let mut lines: Vec<_> = by.into_iter().collect();
    lines.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let mut out = String::with_capacity(lines.len() * 48);
    for (stack, n) in lines {
        out.push_str(&stack);
        out.push(' ');
        out.push_str(&n.to_string());
        out.push('\n');
    }
    out
}

/// Samples charged to the code that caused them: ring-0 samples to the
/// entry that reached ring 0, and 16-bit ring-3 samples (and the ring-0
/// ones they entered) to the Win32 code that switched to 16-bit code.
#[derive(Clone, Debug)]
pub struct Chain {
    /// The Win32 code, root first (`win32_frames`); empty for an entry
    /// from other code.
    pub win32: Vec<Loc>,
    pub entry: Option<Entry>,
    pub samples: u64,
    /// Where the samples are, most first.
    pub places: Vec<(Loc, u64)>,
}

/// The chains the query takes, most samples first, with places and the
/// code they are charged to bucketed as the query asks.
pub fn chains(chains: &HashMap<(u64, u64, Thunk), u64>, query: &Query) -> Vec<Chain> {
    // By the Win32 code and the entry: the samples, and where they were.
    type Row = (u64, HashMap<Loc, u64>);
    let mut by: HashMap<(Vec<Loc>, u64), Row> = HashMap::new();
    for (&(word, entry, thunk), &n) in chains {
        let loc = Loc(word);
        if !query.takes(loc.zone()) {
            continue;
        }
        let entry = Entry::of(entry).map_or(0, |e| e.bucketed(query.bucket).0);
        let row = by.entry((win32_frames(&thunk, query.bucket), entry)).or_default();
        row.0 += n;
        *row.1.entry(loc.bucketed(query.bucket).place()).or_insert(0) += n;
    }
    let mut out: Vec<Chain> = by
        .into_iter()
        .map(|((win32, entry), (samples, places))| {
            let mut places: Vec<(Loc, u64)> = places.into_iter().collect();
            places.sort_by_key(|&(l, n)| (std::cmp::Reverse(n), l.0));
            Chain { win32, entry: Entry::of(entry), samples, places }
        })
        .collect();
    out.sort_by(|a, b| {
        b.samples.cmp(&a.samples).then_with(|| {
            let key = |c: &Chain| (c.win32.iter().map(|l| l.0).collect::<Vec<_>>(), c.entry.map(|e| e.0));
            key(a).cmp(&key(b))
        })
    });
    out
}

/// The samples outside the frame wait: the time the emulator works, which
/// the busy shares (`busy_pct`) are of.
pub fn busy(zones: &[u64; ZONES]) -> u64 {
    zones.iter().enumerate().filter(|&(z, _)| !Zone::ALL[z].is_wait()).map(|(_, &n)| n).sum()
}

/// The zones with samples, most first, as JSON: their share of all the
/// samples, and of the busy ones but for the wait.
pub fn zones_json(zones: &[u64; ZONES], total: u64) -> Value {
    let busy = busy(zones);
    let mut v: Vec<(Zone, u64)> = zones.iter().enumerate().filter(|&(_, &n)| n > 0).map(|(z, &n)| (Zone::ALL[z], n)).collect();
    v.sort_by_key(|&(z, n)| (std::cmp::Reverse(n), z));
    Value::Array(
        v.iter()
            .map(|&(z, n)| {
                let busy_pct = if z.is_wait() { Value::Null } else { json!(pct(n, busy)) };
                json!({"zone": z.name(), "samples": n, "pct": pct(n, total), "busy_pct": busy_pct})
            })
            .collect(),
    )
}

/// The zones' groups with samples, most first, as JSON, with their busy
/// shares as for the zones.
pub fn groups_json(zones: &[u64; ZONES], total: u64) -> Value {
    let busy = busy(zones);
    let mut by: Vec<(&str, u64, u64)> = Vec::new();
    for (z, &n) in zones.iter().enumerate().filter(|&(_, &n)| n > 0) {
        let zone = Zone::ALL[z];
        let working = if zone.is_wait() { 0 } else { n };
        match by.iter_mut().find(|(g, _, _)| *g == zone.group()) {
            Some(entry) => {
                entry.1 += n;
                entry.2 += working;
            }
            None => by.push((zone.group(), n, working)),
        }
    }
    by.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    Value::Array(by.iter().map(|&(g, n, w)| json!({"group": g, "samples": n, "pct": pct(n, total), "busy_pct": pct(w, busy)})).collect())
}

/// What a BIOS or DOS function is, for the common ones.
pub fn service_name(vector: u8, ah: u8) -> Option<&'static str> {
    Some(match (vector, ah) {
        (0x10, 0x00) => "set video mode",
        (0x10, 0x02) => "set cursor position",
        (0x10, 0x03) => "get cursor position",
        (0x10, 0x06) => "scroll up",
        (0x10, 0x09) => "write character and attribute",
        (0x10, 0x0E) => "teletype output",
        (0x10, 0x0F) => "get video mode",
        (0x10, 0x10) => "palette",
        (0x10, 0x4F) => "VESA BIOS",
        (0x13, 0x02) => "read sectors",
        (0x13, 0x03) => "write sectors",
        (0x13, 0x42) => "extended read",
        (0x15, 0x86) => "wait",
        (0x15, 0x87) => "move extended memory",
        (0x15, 0x88) => "extended memory size",
        (0x16, 0x00) | (0x16, 0x10) => "read key",
        (0x16, 0x01) | (0x16, 0x11) => "check for key",
        (0x16, 0x02) | (0x16, 0x12) => "shift flags",
        (0x1A, 0x00) => "read tick count",
        (0x21, 0x02) => "write character",
        (0x21, 0x06) => "direct console I/O",
        (0x21, 0x09) => "write string",
        (0x21, 0x0B) => "check input status",
        (0x21, 0x0E) => "select drive",
        (0x21, 0x19) => "current drive",
        (0x21, 0x1A) => "set DTA",
        (0x21, 0x25) => "set interrupt vector",
        (0x21, 0x2A) => "get date",
        (0x21, 0x2C) => "get time",
        (0x21, 0x30) => "DOS version",
        (0x21, 0x35) => "get interrupt vector",
        (0x21, 0x36) => "free disk space",
        (0x21, 0x3B) => "change directory",
        (0x21, 0x3C) => "create file",
        (0x21, 0x3D) => "open file",
        (0x21, 0x3E) => "close file",
        (0x21, 0x3F) => "read file",
        (0x21, 0x40) => "write file",
        (0x21, 0x41) => "delete file",
        (0x21, 0x42) => "seek",
        (0x21, 0x43) => "file attributes",
        (0x21, 0x44) => "IOCTL",
        (0x21, 0x47) => "current directory",
        (0x21, 0x48) => "allocate memory",
        (0x21, 0x49) => "free memory",
        (0x21, 0x4A) => "resize memory",
        (0x21, 0x4B) => "exec",
        (0x21, 0x4C) => "exit",
        (0x21, 0x4E) => "find first",
        (0x21, 0x4F) => "find next",
        (0x21, 0x56) => "rename",
        (0x21, 0x57) => "file date and time",
        (0x2F, 0x15) => "CD-ROM (MSCDEX)",
        (0x2F, 0x16) => "Windows and DPMI",
        (0x2F, 0x43) => "XMS",
        (0x33, 0x00) => "mouse: reset",
        (0x33, 0x01) => "mouse: show pointer",
        (0x33, 0x02) => "mouse: hide pointer",
        (0x33, 0x03) => "mouse: position and buttons",
        (0x33, 0x04) => "mouse: move pointer",
        (0x33, 0x05) => "mouse: button presses",
        (0x33, 0x06) => "mouse: button releases",
        (0x33, 0x07) => "mouse: horizontal range",
        (0x33, 0x08) => "mouse: vertical range",
        (0x33, 0x0B) => "mouse: motion",
        (0x33, 0x0C) => "mouse: event handler",
        (0x67, _) => "EMS",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::Mode;

    fn counts() -> HashMap<u64, u64> {
        let a = Loc::guest(Engine::Jit, Mode::Pm32, 0x168, 0x1000);
        let a_io = Loc(a.0 & !super::super::ZONE_MASK | (Zone::IoVga as u64) << super::super::ZONE_SHIFT);
        let b = Loc::guest(Engine::Interp, Mode::Real, 0x1234, 0x0105);
        let b2 = Loc::guest(Engine::Interp, Mode::Real, 0x1234, 0x0108);
        HashMap::from([
            (a.0, 50),
            (a_io.0, 30),
            (b.0, 10),
            (b2.0, 5),
            (Loc::host(Zone::FrameWait).0, 100),
        ])
    }

    #[test]
    fn places_add_up_their_zones() {
        let places = places(&counts(), &Query::default());
        assert_eq!(places.len(), 3);
        assert_eq!(places[0].samples, 80);
        assert_eq!(places[0].zones, vec![(Zone::Jit, 50), (Zone::IoVga, 30)]);
        assert_eq!(places[0].loc.addr_text().as_deref(), Some("0168:00001000"));
    }

    #[test]
    fn buckets_and_zone_filters() {
        let q = Query { bucket: 16, ..Query::default() };
        let p = places(&counts(), &q);
        assert_eq!(p.len(), 2, "1234:0105 and 1234:0108 in one bucket");
        assert_eq!(p[1].samples, 15);
        assert_eq!(p[1].loc.addr_text().as_deref(), Some("1234:0100"));
        assert_eq!(p[1].hottest.addr_text().as_deref(), Some("1234:0105"), "the one with 10 of the 15");
        let io = Query { zone: Some("io".into()), ..Query::default() };
        let p = places(&counts(), &io);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].samples, 30);
        assert!(Query { zone: Some("nope".into()), ..Query::default() }.check().is_err());
        assert!(Query { bucket: 3, ..Query::default() }.check().is_err());
    }

    #[test]
    fn folded_stacks_root_first() {
        let text = folded(&counts(), &HashMap::new(), &Query::default());
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "frame.wait 100");
        assert!(lines.contains(&"cpu.jit;pm32 0168:00001000 50"));
        assert!(lines.contains(&"cpu.jit;pm32 0168:00001000;io.vga 30"));
        assert!(lines.contains(&"cpu.interp;real 1234:0105 10"));
    }

    #[test]
    fn chained_samples_go_to_their_callers() {
        use crate::profile::{Cause, Entry};
        let ring0 = Loc::guest(Engine::Jit, Mode::Pm32, 0x0028, 0xC000_13AC);
        let win16 = Loc::guest(Engine::Jit, Mode::Pm16, 0x003F, 0x03D0);
        let entry = Entry::new(Mode::Pm16, 0x003F, 0x03D2, Cause::Int, 0x30).0;
        let win32 = |eip| Loc::guest(Engine::None, Mode::Pm32, 0x0137, eip).0;
        // The switch in KERNEL32, called from USER32, called from the game.
        let thunk = [win32(0xBFF7_13D9), win32(0xBFF5_2010), win32(0x0040_3000), 0];
        let counts = HashMap::from([(ring0.0, 10), (win16.0, 4)]);
        let chains_in = HashMap::from([((ring0.0, entry, thunk), 6), ((win16.0, 0, thunk), 4)]);

        let rows = chains(&chains_in, &Query::default());
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].samples, 6);
        assert_eq!(rows[0].entry.map(|e| e.via()).as_deref(), Some("int 30h"));
        let names: Vec<String> = rows[0].win32.iter().filter_map(|l| l.addr_text()).collect();
        assert_eq!(names, ["0137:00403000", "0137:BFF52010", "0137:BFF713D9"], "root first");
        assert_eq!(rows[0].places, vec![(ring0, 6)]);
        assert_eq!((rows[1].samples, rows[1].entry), (4, None));

        let modules = chains(&chains_in, &Query { bucket: 0x10000, ..Query::default() });
        assert_eq!(modules[0].win32[0].addr_text().as_deref(), Some("0137:00400000"));

        let text = folded(&counts, &chains_in, &Query::default());
        let lines: Vec<&str> = text.lines().collect();
        let game = "pm32 0137:00403000;pm32 0137:BFF52010;pm32 0137:BFF713D9 (thunk)";
        assert!(lines.contains(&format!("{};pm16 003F:03D2 (int 30h);cpu.jit;pm32 0028:C00013AC 6", game).as_str()), "{}", text);
        assert!(lines.contains(&"cpu.jit;pm32 0028:C00013AC 4"), "the samples before the first entry: {}", text);
        assert!(lines.contains(&format!("{};cpu.jit;pm16 003F:03D0 4", game).as_str()), "{}", text);
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn groups_and_segments() {
        let c = counts();
        let zones = by_zone(&c);
        let groups = groups_json(&zones, 195);
        assert_eq!(groups[0]["group"], "frame");
        assert_eq!(groups[0]["busy_pct"], 0.0, "only the wait");
        assert_eq!(groups[1]["group"], "cpu");
        assert_eq!(groups[1]["samples"], 65);
        assert_eq!(busy(&zones), 95);
        assert_eq!(groups[1]["busy_pct"], 68.42);
        let zones_v = zones_json(&zones, 195);
        assert_eq!(zones_v[0]["zone"], "frame.wait");
        assert!(zones_v[0]["busy_pct"].is_null());
        let segs = segments(&c, &Query::default());
        assert_eq!(segs[0], ((0x168, Mode::Pm32), 80));
        assert_eq!(pct(1, 3), 33.33);
    }
}
