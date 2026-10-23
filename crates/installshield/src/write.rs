//! Writers of cabinet sets, InstallShield 3 archives and PKWARE DCL data,
//! for tests. No open tool writes these formats, so they follow the
//! layouts the readers read; a writer and a reader made from one reading
//! of a format only show that the two agree, which is why the tests also
//! decode data an independent decoder expanded, and real archives are
//! compared with Unshield's and idecomp's output.

use flate2::{Compress, Compression as Level, FlushCompress};
use md5::{Digest, Md5};

/// The volumes' data starts here, after their header.
pub const VOLUME_DATA_OFFSET: u64 = 0x200;
const DESCRIPTOR_OFFSET: usize = 0x200;
const FILE_TABLE_OFFSET: usize = 0x280;
const GROUP_LISTS_OFFSET: usize = 0x3e;
const CHUNK_INPUT: usize = 0x8000;

/// One file of a cabinet set. `stored` replaces the bytes written for it
/// (before obfuscation), to store bad data under a good header. An
/// `outside` file's bytes aren't written: its data offset is the length of
/// the volume that would hold it.
#[derive(Clone, Debug, Default)]
pub struct CabinetFile {
    pub directory: String,
    pub name: String,
    pub data: Vec<u8>,
    pub compressed: bool,
    pub obfuscated: bool,
    pub link_to: Option<usize>,
    pub invalid: bool,
    pub stored: Option<Vec<u8>>,
    pub outside: bool,
}

impl CabinetFile {
    /// A compressed file.
    pub fn new(directory: &str, name: &str, data: &[u8]) -> CabinetFile {
        CabinetFile {
            directory: directory.to_string(),
            name: name.to_string(),
            data: data.to_vec(),
            compressed: true,
            ..CabinetFile::default()
        }
    }
}

/// A file group: a name and a range of the file table, written as given.
/// Groups with the same `list` are chained in their order.
#[derive(Clone, Debug)]
pub struct CabinetGroup {
    pub name: String,
    pub first: u32,
    pub last: u32,
    pub list: usize,
}

impl CabinetGroup {
    pub fn new(name: &str, first: u32, last: u32) -> CabinetGroup {
        CabinetGroup {
            name: name.to_string(),
            first,
            last,
            list: 0,
        }
    }
}

/// How a set is written.
#[derive(Clone, Debug)]
pub struct CabinetOptions {
    /// The most bytes of member data a volume holds.
    pub volume_capacity: u64,
    /// Another version word than the major version's.
    pub version_word: Option<u32>,
    /// No `data1.hdr`: the header is at the start of `data1.cab`.
    pub header_in_cabinet: bool,
    pub groups: Vec<CabinetGroup>,
    /// Compressed members as marker-delimited chunks.
    pub markers: bool,
}

impl Default for CabinetOptions {
    fn default() -> CabinetOptions {
        CabinetOptions {
            volume_capacity: u64::MAX,
            version_word: None,
            header_in_cabinet: false,
            groups: Vec::new(),
            markers: false,
        }
    }
}

/// The version word of a major version.
pub fn version_word(major: u32) -> u32 {
    match major {
        0 => 0x0100_0004,
        5 => 0x0100_5000,
        _ => 0x0200_0000 | (major * 100),
    }
}

/// Raw deflate of `data` (a final block when `finish`, else a sync flush
/// ending in `00 00 FF FF`).
fn deflate(compress: &mut Compress, data: &[u8], flush: FlushCompress) -> Vec<u8> {
    // Room for all of it, so one call compresses and flushes the lot.
    let mut out = Vec::with_capacity(data.len() + data.len() / 2 + 1024);
    let start = compress.total_in();
    compress
        .compress_vec(data, &mut out, flush)
        .expect("deflate");
    assert_eq!(
        (compress.total_in() - start) as usize,
        data.len(),
        "deflate took all of its input"
    );
    out
}

/// The length-prefixed chunks of `data`.
pub fn chunks(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for part in data.chunks(CHUNK_INPUT) {
        let mut compress = Compress::new(Level::best(), false);
        let chunk = deflate(&mut compress, part, FlushCompress::Finish);
        out.extend_from_slice(&(chunk.len() as u16).to_le_bytes());
        out.extend_from_slice(&chunk);
    }
    out
}

/// The marker-delimited form of `data`: raw deflate without lengths,
/// flushed after each `chunk_input` bytes so each chunk ends with
/// `00 00 FF FF`, and no final block. Each chunk is compressed on its own
/// unless `continuous`, which keeps one stream whose chunks may refer back.
pub fn marker_chunks(data: &[u8], chunk_input: usize, continuous: bool) -> Vec<u8> {
    let mut out = Vec::new();
    let mut shared = Compress::new(Level::best(), false);
    for part in data.chunks(chunk_input) {
        let mut own = Compress::new(Level::best(), false);
        let compress = if continuous { &mut shared } else { &mut own };
        out.extend(deflate(compress, part, FlushCompress::Sync));
    }
    out
}

/// The bytes a set stores for `file`, compressed and obfuscated as a
/// volume holds them: what a test writes beside the header for a file
/// stored outside.
pub fn stored_bytes(file: &CabinetFile, markers: bool) -> Vec<u8> {
    let mut raw = match &file.stored {
        Some(stored) => stored.clone(),
        None if !file.compressed => file.data.clone(),
        None if markers => marker_chunks(&file.data, CHUNK_INPUT, false),
        None => chunks(&file.data),
    };
    if file.obfuscated {
        for (seed, byte) in raw.iter_mut().enumerate() {
            let value = byte.wrapping_add((seed as u32 % 0x47) as u8);
            *byte = value.rotate_left(2) ^ 0xd5;
        }
    }
    raw
}

#[derive(Clone, Copy, Debug)]
struct Part {
    volume: usize,
    offset: u64,
    start: u64,
    length: u64,
}

fn put16(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

fn put32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn put64(b: &mut [u8], at: usize, v: u64) {
    b[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

fn latin1(text: &str) -> Vec<u8> {
    text.chars().map(|c| c as u32 as u8).collect()
}

/// A cabinet set of major version 0, 5 or 6: each file by name
/// (`data1.hdr`, `data1.cab`, `data2.cab`...). Volumes record the first
/// and last file they hold, split ones included. Version 0 is written with
/// version 5's layout, its descriptors ending after the data offset and
/// its split files flagged.
pub fn cabinet(
    major: u32,
    files: &[CabinetFile],
    options: &CabinetOptions,
) -> Vec<(String, Vec<u8>)> {
    let word = options.version_word.unwrap_or_else(|| version_word(major));
    let raws: Vec<Vec<u8>> = files
        .iter()
        .map(|f| {
            if f.link_to.is_none() && !f.invalid {
                stored_bytes(f, options.markers)
            } else {
                Vec::new()
            }
        })
        .collect();
    let capacity = options.volume_capacity;
    // The header's length doesn't depend on where the members are, so a
    // first pass sizes it.
    let data_start = if options.header_in_cabinet {
        let (parts, _) = layout(files, &raws, capacity, VOLUME_DATA_OFFSET);
        header(major, files, &raws, &parts, word, &options.groups).len() as u64
    } else {
        VOLUME_DATA_OFFSET
    };
    let (parts, count) = layout(files, &raws, capacity, data_start);
    let header_bytes = header(major, files, &raws, &parts, word, &options.groups);
    let mut set = Vec::new();
    if !options.header_in_cabinet {
        set.push(("data1.hdr".to_string(), header_bytes.clone()));
    }
    for volume in 1..=count {
        let own_header = (volume == 1 && options.header_in_cabinet).then_some(&header_bytes[..]);
        let start = if volume == 1 {
            data_start
        } else {
            VOLUME_DATA_OFFSET
        };
        set.push((
            format!("data{}.cab", volume),
            volume_bytes(major, files, &raws, &parts, volume, word, own_header, start),
        ));
    }
    set
}

fn layout(
    files: &[CabinetFile],
    raws: &[Vec<u8>],
    capacity: u64,
    first_start: u64,
) -> (Vec<Vec<Part>>, usize) {
    let mut parts: Vec<Vec<Part>> = vec![Vec::new(); files.len()];
    let mut volume = 1;
    let mut used = 0u64;
    let mut outside = Vec::new();
    let base = |volume: usize| {
        if volume == 1 {
            first_start
        } else {
            VOLUME_DATA_OFFSET
        }
    };
    for (index, file) in files.iter().enumerate() {
        if file.link_to.is_some() || file.invalid {
            continue;
        }
        if file.outside {
            outside.push((index, volume));
            continue;
        }
        let mut remaining = raws[index].len() as u64;
        let mut start = 0;
        loop {
            if used == capacity {
                volume += 1;
                used = 0;
            }
            let take = remaining.min(capacity - used);
            parts[index].push(Part {
                volume,
                offset: base(volume) + used,
                start,
                length: take,
            });
            used += take;
            start += take;
            remaining -= take;
            if remaining == 0 {
                break;
            }
        }
    }
    // A file stored outside gets an empty part at the end of its volume,
    // so its data offset is the volume's length and the volume's index
    // range covers it.
    for (index, held) in outside {
        let length: u64 = parts
            .iter()
            .flatten()
            .filter(|p| p.volume == held)
            .map(|p| p.length)
            .sum();
        parts[index].push(Part {
            volume: held,
            offset: base(held) + length,
            start: 0,
            length: 0,
        });
    }
    (parts, volume)
}

fn header(
    major: u32,
    files: &[CabinetFile],
    raws: &[Vec<u8>],
    parts: &[Vec<Part>],
    word: u32,
    groups: &[CabinetGroup],
) -> Vec<u8> {
    let v5 = major != 6;
    let mut directories: Vec<&str> = Vec::new();
    for f in files {
        if !directories.contains(&f.directory.as_str()) {
            directories.push(&f.directory);
        }
    }
    let offsets_count = directories.len() + if v5 { files.len() } else { 0 };
    let mut table = vec![0u8; offsets_count * 4];
    let mut offsets = vec![0u32; offsets_count];
    let mut descriptors = 0;
    if !v5 {
        descriptors = table.len();
        table.resize(table.len() + files.len() * 0x57, 0);
    }
    for (i, d) in directories.iter().enumerate() {
        offsets[i] = table.len() as u32;
        table.extend(latin1(d));
        table.push(0);
    }
    let mut names = Vec::new();
    for f in files {
        names.push(table.len() as u32);
        table.extend(latin1(&f.name));
        table.push(0);
    }
    for (index, file) in files.iter().enumerate() {
        let target = file.link_to.unwrap_or(index);
        let data = &files[target];
        let raw = &raws[target];
        let first = parts[target].first();
        // Version 5 descriptors have no split flag: readers tell a split
        // file from the volume headers.
        let flags = (if data.compressed { 4 } else { 0 })
            | (if data.obfuscated { 2 } else { 0 })
            | (if major != 5 && parts[target].len() > 1 {
                1
            } else {
                0
            })
            | (if file.invalid { 8 } else { 0 });
        let md5: [u8; 16] = Md5::digest(&data.data).into();
        let data_offset = if file.invalid {
            0
        } else {
            first.map_or(VOLUME_DATA_OFFSET, |p| p.offset)
        };
        let directory = directories
            .iter()
            .position(|d| *d == file.directory)
            .unwrap_or(0) as u16;
        if v5 {
            offsets[directories.len() + index] = table.len() as u32;
            let mut d = vec![0u8; if major == 5 { 0x3a } else { 0x2a }];
            put32(&mut d, 0, names[index]);
            put16(&mut d, 4, directory);
            put16(&mut d, 8, flags);
            put32(&mut d, 10, data.data.len() as u32);
            put32(&mut d, 14, raw.len() as u32);
            put32(&mut d, 0x26, data_offset as u32);
            if major == 5 {
                d[0x2a..0x3a].copy_from_slice(&md5);
            }
            table.extend(d);
        } else {
            let at = descriptors + index * 0x57;
            let d = &mut table[at..at + 0x57];
            put16(d, 0, flags);
            put64(d, 2, data.data.len() as u64);
            put64(d, 0x0a, raw.len() as u64);
            put64(d, 0x12, data_offset);
            d[0x1a..0x2a].copy_from_slice(&md5);
            put32(d, 0x3a, names[index]);
            put16(d, 0x3e, directory);
            put32(d, 0x4c, file.link_to.unwrap_or(0) as u32);
            d[0x54] = if file.link_to.is_some() { 1 } else { 0 };
            put16(d, 0x55, first.map_or(1, |p| p.volume as u16));
        }
    }
    for (i, offset) in offsets.iter().enumerate() {
        put32(&mut table, i * 4, *offset);
    }
    let (group_bytes, heads) = group_lists(major, groups, FILE_TABLE_OFFSET + table.len());
    let mut header =
        vec![0u8; DESCRIPTOR_OFFSET + FILE_TABLE_OFFSET + table.len() + group_bytes.len()];
    put32(&mut header, 0, crate::cabinet::SIGNATURE);
    put32(&mut header, 4, word);
    put32(&mut header, 12, DESCRIPTOR_OFFSET as u32);
    let declared = (header.len() - DESCRIPTOR_OFFSET) as u32;
    put32(&mut header, 16, declared);
    let d = DESCRIPTOR_OFFSET;
    put32(&mut header, d + 0x0c, FILE_TABLE_OFFSET as u32);
    put32(&mut header, d + 0x14, table.len() as u32);
    put32(&mut header, d + 0x18, table.len() as u32);
    put32(&mut header, d + 0x1c, directories.len() as u32);
    put32(&mut header, d + 0x28, files.len() as u32);
    put32(&mut header, d + 0x2c, descriptors as u32);
    header[d + FILE_TABLE_OFFSET..d + FILE_TABLE_OFFSET + table.len()].copy_from_slice(&table);
    let groups_at = d + FILE_TABLE_OFFSET + table.len();
    header[groups_at..groups_at + group_bytes.len()].copy_from_slice(&group_bytes);
    for (list, head) in heads.iter().enumerate() {
        put32(&mut header, d + GROUP_LISTS_OFFSET + 4 * list, *head);
    }
    header
}

/// The group lists' entries, the group descriptors and their names, placed
/// `start` bytes into the cabinet descriptor, and the head of each list.
fn group_lists(major: u32, groups: &[CabinetGroup], start: usize) -> (Vec<u8>, [u32; 71]) {
    let (first_at, size) = if major != 6 {
        (0x4c, 0x54)
    } else {
        (0x16, 0x1e)
    };
    let mut heads = [0u32; 71];
    let mut bytes = Vec::new();
    let mut previous: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for group in groups {
        let entry = start + bytes.len();
        let descriptor = entry + 12;
        let name = descriptor + size;
        match previous.get(&group.list) {
            Some(&before) => put32(&mut bytes, before - start + 8, entry as u32),
            None => heads[group.list] = entry as u32,
        }
        previous.insert(group.list, entry);
        bytes.extend((name as u32).to_le_bytes());
        bytes.extend((descriptor as u32).to_le_bytes());
        bytes.extend(0u32.to_le_bytes());
        let mut fields = vec![0u8; size];
        put32(&mut fields, 0, name as u32);
        put32(&mut fields, first_at, group.first);
        put32(&mut fields, first_at + 4, group.last);
        bytes.extend(fields);
        bytes.extend(latin1(&group.name));
        bytes.push(0);
    }
    (bytes, heads)
}

#[allow(clippy::too_many_arguments)]
fn volume_bytes(
    major: u32,
    files: &[CabinetFile],
    raws: &[Vec<u8>],
    parts: &[Vec<Part>],
    volume: usize,
    word: u32,
    header: Option<&[u8]>,
    data_start: u64,
) -> Vec<u8> {
    let held: Vec<usize> = (0..files.len())
        .filter(|&i| parts[i].iter().any(|p| p.volume == volume))
        .collect();
    let length = data_start
        + parts
            .iter()
            .flatten()
            .filter(|p| p.volume == volume)
            .map(|p| p.length)
            .sum::<u64>();
    let mut bytes = vec![0u8; length as usize];
    // The header's bytes between the common header and the descriptor are
    // zero, so the volume's fields written below fit there.
    if let Some(header) = header {
        bytes[..header.len()].copy_from_slice(header);
    }
    put32(&mut bytes, 0, crate::cabinet::SIGNATURE);
    put32(&mut bytes, 4, word);
    for &i in &held {
        for p in parts[i].iter().filter(|p| p.volume == volume) {
            let (s, o, l) = (p.start as usize, p.offset as usize, p.length as usize);
            bytes[o..o + l].copy_from_slice(&raws[i][s..s + l]);
        }
    }
    let record = |i: usize| {
        let Some(p) = parts[i].iter().find(|p| p.volume == volume) else {
            return (0, 0, 0);
        };
        // A file stored outside records its whole sizes, as an unsplit one.
        if files[i].outside {
            return (p.offset, files[i].data.len() as u64, raws[i].len() as u64);
        }
        let expanded = if files[i].compressed {
            files[i].data.len() as u64
        } else {
            p.length
        };
        (p.offset, expanded, p.length)
    };
    let first = held.first().map_or((0, 0, 0), |&i| record(i));
    let last = held.last().map_or((0, 0, 0), |&i| record(i));
    let first_index = held.first().copied().unwrap_or(0) as u32;
    let last_index = held.last().copied().unwrap_or(0) as u32;
    let f = 20;
    if major != 6 {
        let values = [
            data_start as u32,
            0,
            first_index,
            last_index,
            first.0 as u32,
            first.1 as u32,
            first.2 as u32,
            last.0 as u32,
            last.1 as u32,
            last.2 as u32,
        ];
        for (i, v) in values.iter().enumerate() {
            put32(&mut bytes, f + i * 4, *v);
        }
    } else {
        put64(&mut bytes, f, data_start);
        put32(&mut bytes, f + 8, first_index);
        put32(&mut bytes, f + 12, last_index);
        for (i, v) in [first.0, first.1, first.2, last.0, last.1, last.2]
            .iter()
            .enumerate()
        {
            put64(&mut bytes, f + 16 + i * 8, *v);
        }
    }
    bytes
}

/// One member of an InstallShield 3 archive.
#[derive(Clone, Debug)]
pub struct ArchiveFile {
    pub directory: String,
    pub name: String,
    pub data: Vec<u8>,
    pub stored: bool,
    pub invalid: bool,
    pub coded_literals: bool,
    pub dictionary_bits: u32,
}

impl ArchiveFile {
    /// A member imploded with coded literals and a 4 KB window.
    pub fn new(directory: &str, name: &str, data: &[u8]) -> ArchiveFile {
        ArchiveFile {
            directory: directory.to_string(),
            name: name.to_string(),
            data: data.to_vec(),
            stored: false,
            invalid: false,
            coded_literals: true,
            dictionary_bits: 6,
        }
    }
}

/// The data of an InstallShield 3 archive's first member starts here.
pub const ARCHIVE_DATA_START: usize = 255;

/// An InstallShield 3 archive: a 255-byte header region, the members'
/// data, then the directory table and the file table. Files of one
/// directory must be given together, for the directory counts to place
/// them. Every entry names part 1 as its first and last part.
pub fn archive(files: &[ArchiveFile], flags: u16, total_parts: u8, part: u8) -> Vec<u8> {
    let mut directories: Vec<&str> = Vec::new();
    for f in files {
        if !directories.contains(&f.directory.as_str()) {
            directories.push(&f.directory);
        }
    }
    let mut out = vec![0u8; ARCHIVE_DATA_START];
    let mut offsets = Vec::new();
    let mut stored = Vec::new();
    for f in files {
        let data = if f.stored {
            f.data.clone()
        } else {
            implode(&f.data, f.coded_literals, f.dictionary_bits, true)
        };
        offsets.push(out.len());
        out.extend_from_slice(&data);
        stored.push(data);
    }
    let directories_offset = out.len();
    for d in &directories {
        let name = latin1(d);
        let mut entry = vec![0u8; 11 + name.len()];
        put16(
            &mut entry,
            0,
            files.iter().filter(|f| f.directory == *d).count() as u16,
        );
        let len = entry.len() as u16;
        put16(&mut entry, 2, len);
        put16(&mut entry, 4, name.len() as u16);
        entry[6..6 + name.len()].copy_from_slice(&name);
        out.extend(entry);
    }
    let files_offset = out.len();
    for (i, f) in files.iter().enumerate() {
        let name = latin1(&f.name);
        let mut entry = vec![0u8; 43 + name.len()];
        entry[0] = 1;
        put16(
            &mut entry,
            1,
            directories
                .iter()
                .position(|d| *d == f.directory)
                .unwrap_or(0) as u16,
        );
        put32(&mut entry, 3, f.data.len() as u32);
        put32(&mut entry, 7, stored[i].len() as u32);
        put32(&mut entry, 11, offsets[i] as u32);
        // 1 January 1996, 12:00, as a DOS date and time.
        put32(&mut entry, 15, 0x6000_2021);
        put32(&mut entry, 19, 0x20);
        let len = entry.len() as u16;
        put16(&mut entry, 23, len);
        put16(
            &mut entry,
            25,
            (if f.stored { 0x10 } else { 0 }) | (if f.invalid { 0x20 } else { 0 }),
        );
        entry[28] = 1;
        entry[29] = name.len() as u8;
        entry[30..30 + name.len()].copy_from_slice(&name);
        out.extend(entry);
    }
    let total = out.len();
    put32(&mut out, 0, crate::archive::SIGNATURE);
    out[4] = 0x3a;
    out[5] = 1;
    out[6] = 2;
    put16(&mut out, 10, flags);
    put16(&mut out, 12, files.len() as u16);
    put32(&mut out, 14, 0x6000_2021);
    put32(&mut out, 18, total as u32);
    put32(
        &mut out,
        22,
        files.iter().map(|f| f.data.len() as u32).sum(),
    );
    out[30] = total_parts;
    out[31] = part;
    put32(&mut out, 41, directories_offset as u32);
    put32(&mut out, 45, (files_offset - directories_offset) as u32);
    put16(&mut out, 49, directories.len() as u16);
    put32(&mut out, 51, files_offset as u32);
    put32(&mut out, 55, (total - files_offset) as u32);
    out
}

/// Where the file table entry of file `index` starts in `archive`.
pub fn archive_file_entry(archive: &[u8], index: usize) -> usize {
    let mut offset = u32::from_le_bytes(archive[51..55].try_into().unwrap_or([0; 4])) as usize;
    for _ in 0..index {
        offset += u16::from_le_bytes([archive[offset + 23], archive[offset + 24]]) as usize;
    }
    offset
}

/// Canonical codes (code, length) in the order a reader that inverts each
/// bit counts them: by length, then symbol.
fn canonical(runs: &[u8]) -> Vec<(u32, u32)> {
    let lengths = crate::explode::Huffman::lengths(runs);
    let mut codes = vec![(0, 0); lengths.len()];
    let mut next = 0u32;
    for length in 1..=13u8 {
        for (symbol, &l) in lengths.iter().enumerate() {
            if l == length {
                codes[symbol] = (next, length as u32);
                next += 1;
            }
        }
        next <<= 1;
    }
    codes
}

#[derive(Default)]
struct BitWriter {
    bytes: Vec<u8>,
    buffer: u32,
    count: u32,
}

impl BitWriter {
    fn bit(&mut self, bit: u32) {
        self.buffer |= bit << self.count;
        self.count += 1;
        if self.count == 8 {
            self.bytes.push(self.buffer as u8);
            self.buffer = 0;
            self.count = 0;
        }
    }

    fn bits(&mut self, value: u32, width: u32) {
        for bit in 0..width {
            self.bit((value >> bit) & 1);
        }
    }

    /// A code goes most significant bit first, each bit inverted.
    fn code(&mut self, (code, length): (u32, u32)) {
        for bit in (0..length).rev() {
            self.bit(((code >> bit) & 1) ^ 1);
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.count > 0 {
            self.bytes.push(self.buffer as u8);
        }
        self.bytes
    }
}

/// PKWARE DCL data of `data` ending with the end code, with greedy
/// matching (or none, without `matches`).
pub fn implode(data: &[u8], coded_literals: bool, dictionary_bits: u32, matches: bool) -> Vec<u8> {
    let [literal_runs, length_runs, distance_runs] = crate::explode::runs();
    let (literals, lengths, distances) = (
        canonical(literal_runs),
        canonical(length_runs),
        canonical(distance_runs),
    );
    let (base, extra) = crate::explode::LENGTHS;
    let write_length = |w: &mut BitWriter, length: usize| {
        let symbol = (0..16)
            .find(|&i| length >= base[i] && length < base[i] + (1 << extra[i]))
            .unwrap_or(0);
        w.code(lengths[symbol]);
        w.bits((length - base[symbol]) as u32, extra[symbol]);
    };
    let mut w = BitWriter::default();
    w.bits(coded_literals as u32, 8);
    w.bits(dictionary_bits, 8);
    let max_distance = 64usize << dictionary_bits;
    let mut position = 0;
    while position < data.len() {
        let (length, distance) = if matches {
            longest_match(data, position, max_distance)
        } else {
            (0, 0)
        };
        if length >= 2 {
            w.bit(1);
            write_length(&mut w, length);
            let low = if length == 2 { 2 } else { dictionary_bits };
            w.code(distances[(distance - 1) >> low]);
            w.bits(((distance - 1) & ((1 << low) - 1)) as u32, low);
            position += length;
            continue;
        }
        w.bit(0);
        if coded_literals {
            w.code(literals[data[position] as usize]);
        } else {
            w.bits(data[position] as u32, 8);
        }
        position += 1;
    }
    w.bit(1);
    write_length(&mut w, 519);
    w.finish()
}

/// The longest earlier match of up to 518 bytes; one of two bytes must be
/// within 256 bytes.
fn longest_match(data: &[u8], position: usize, max_distance: usize) -> (usize, usize) {
    let mut best = (0, 0);
    let limit = 518.min(data.len() - position);
    for distance in 1..=max_distance.min(position) {
        let mut length = 0;
        while length < limit && data[position + length] == data[position + length - distance] {
            length += 1;
        }
        if length == 2 && distance > 256 {
            continue;
        }
        if length > best.0 {
            best = (length, distance);
        }
        if length == limit {
            break;
        }
    }
    best
}
