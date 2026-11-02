//! Reading an AVI file's headers and index: its streams, their formats
//! and where each of their chunks is, as AVIFIL32's AVI file handler
//! does when it opens a file. The layout follows Wine's avifile.c (LGPL
//! 2.1 or later):
//!
//! Copyright 1999 Marcus Meissner
//! Copyright 2002-2003 Michael Günnewig
//!
//! A file is `RIFF 'AVI '` with a `LIST 'hdrl'` (the main header `avih`
//! and a `LIST 'strl'` for each stream: `strh`, `strf` and maybe `strn`),
//! a `LIST 'movi'` of the chunks (`00dc`, `01wb`, maybe in `LIST 'rec '`
//! records) and an `idx1` index. A file without an index has its `movi`
//! list read chunk by chunk. Everything comes from the file, so every
//! size and offset is checked against the file's length.

/// A chunk of a stream: where its data is in the file, its size and its
/// index flags (AVIIF_KEYFRAME and the rest).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Chunk {
    pub offset: u32,
    pub size: u32,
    pub flags: u32,
}

/// AVIIF_KEYFRAME, which a file without an index gives every chunk.
pub const AVIIF_KEYFRAME: u32 = 0x10;

/// A stream: its header (`strh`, 56 bytes), format (`strf`), name and
/// chunks.
#[derive(Clone, Debug, Default)]
pub struct Stream {
    pub header: Vec<u8>,
    pub format: Vec<u8>,
    pub name: String,
    pub chunks: Vec<Chunk>,
}

/// The file: its main header (`avih`, 56 bytes) and streams.
#[derive(Clone, Debug, Default)]
pub struct Avi {
    pub main: Vec<u8>,
    pub streams: Vec<Stream>,
}

/// Why a file can't be read: AVIERR_FILEREAD (a read failed or the file
/// ends early) or AVIERR_BADFORMAT (it isn't an AVI file).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Read,
    Format,
}

/// Streams a file may have; Windows' AVIFile handles 100 at most.
const MAX_STREAMS: usize = 100;
/// The largest header list read whole, and the most index entries.
const MAX_HEADERS: u32 = 1 << 20;
const MAX_ENTRIES: u32 = 1 << 22;
/// The size of AVIStreamHeader and MainAVIHeader.
const HEADER: usize = 56;

fn fourcc(b: &[u8]) -> [u8; 4] {
    [b[0], b[1], b[2], b[3]]
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

impl Stream {
    fn field(&self, at: usize) -> u32 {
        le32(&self.header, at)
    }
    /// fccType: "vids", "auds", ...
    pub fn kind(&self) -> u32 {
        self.field(0)
    }
    pub fn scale(&self) -> u32 {
        self.field(20)
    }
    pub fn rate(&self) -> u32 {
        self.field(24)
    }
    pub fn start(&self) -> u32 {
        self.field(28)
    }
    pub fn length(&self) -> u32 {
        self.field(32)
    }
    pub fn sample_size(&self) -> u32 {
        self.field(44)
    }

    /// The chunk of sample `sample` (from the stream's start) and the
    /// byte in it where the sample starts. A stream of fixed-size samples
    /// (audio) runs on across its chunks; one of variable-size samples
    /// (video) has a chunk for each.
    pub fn locate(&self, sample: u32) -> Option<(usize, u32)> {
        let size = self.sample_size();
        if size == 0 {
            return ((sample as usize) < self.chunks.len()).then_some((sample as usize, 0));
        }
        let mut byte = sample as u64 * size as u64;
        for (i, c) in self.chunks.iter().enumerate() {
            if byte < c.size as u64 {
                return Some((i, byte as u32));
            }
            byte -= c.size as u64;
        }
        None
    }
}

/// Read the AVI file of `len` bytes that `read(offset, count)` reads
/// from (fewer bytes than asked at its end, or None when a read fails).
pub fn parse(len: u32, read: &mut dyn FnMut(u32, u32) -> Option<Vec<u8>>) -> Result<Avi, Error> {
    let head = read(0, 12).ok_or(Error::Read)?;
    if head.len() < 12 || &head[..4] != b"RIFF" || &head[8..12] != b"AVI " {
        return Err(Error::Format);
    }
    let end = le32(&head, 4).saturating_add(8).min(len);
    let mut avi = Avi::default();
    let mut movi = None;
    let mut index = None;
    let mut at = 12u32;
    while at.saturating_add(8) <= end {
        let h = read(at, 12).ok_or(Error::Read)?;
        if h.len() < 8 {
            break;
        }
        let id = fourcc(&h);
        let size = le32(&h, 4);
        let body = at + 8;
        let next = body.saturating_add(size).saturating_add(size & 1);
        match &id {
            b"LIST" if h.len() >= 12 => match &h[8..12] {
                b"hdrl" => {
                    let size = size.min(MAX_HEADERS).min(end.saturating_sub(body));
                    let list = read(body, size).ok_or(Error::Read)?;
                    headers(&list[4.min(list.len())..], &mut avi)?;
                }
                b"movi" => movi = Some((body, size.min(end.saturating_sub(body)))),
                _ => {}
            },
            b"idx1" => index = Some((body, size.min(end.saturating_sub(body)))),
            _ => {}
        }
        if next <= at {
            break;
        }
        at = next;
    }
    if avi.main.len() < HEADER || avi.streams.is_empty() {
        return Err(Error::Format);
    }
    let Some((movi_at, movi_size)) = movi else {
        return Err(Error::Format);
    };
    let indexed = match index {
        Some((at, size)) if size >= 16 => {
            let count = (size / 16).min(MAX_ENTRIES);
            let entries = read(at, count * 16).ok_or(Error::Read)?;
            read_index(&entries, movi_at, len, read, &mut avi)
        }
        _ => false,
    };
    if !indexed {
        for s in &mut avi.streams {
            s.chunks.clear();
        }
        scan(movi_at + 4, movi_at + movi_size, read, &mut avi)?;
    }
    Ok(avi)
}

/// The stream whose sample a chunk id ("00dc", "01wb") names, if it names
/// one. A palette change ("00pc") isn't a sample: counted as one, it
/// would move every frame after it. Changes aren't applied, so a stream
/// keeps the palette of its format.
fn stream_of(id: &[u8]) -> Option<usize> {
    if &id[2..4] == b"pc" {
        return None;
    }
    let tens = (id[0] as char).to_digit(16)?;
    let ones = (id[1] as char).to_digit(16)?;
    Some((tens * 16 + ones) as usize)
}

/// The chunks of `hdrl`'s list, after its type.
fn headers(list: &[u8], avi: &mut Avi) -> Result<(), Error> {
    for (id, body) in chunks(list) {
        match &id {
            b"avih" => {
                let mut main = body.to_vec();
                main.resize(HEADER, 0);
                avi.main = main;
            }
            b"LIST" if body.len() >= 4 && &body[..4] == b"strl" => {
                if avi.streams.len() == MAX_STREAMS {
                    continue;
                }
                let mut stream = Stream::default();
                for (id, body) in chunks(&body[4..]) {
                    match &id {
                        b"strh" => {
                            let mut header = body.to_vec();
                            header.resize(HEADER, 0);
                            stream.header = header;
                        }
                        b"strf" => stream.format = body.to_vec(),
                        b"strn" => {
                            let end = body.iter().position(|&b| b == 0).unwrap_or(body.len());
                            stream.name = crate::dosstr::from_bytes(&body[..end]);
                        }
                        _ => {}
                    }
                }
                if stream.header.is_empty() {
                    return Err(Error::Format);
                }
                avi.streams.push(stream);
            }
            _ => {}
        }
    }
    Ok(())
}

/// The chunks of a list's body: id and bytes, as far as they are whole.
fn chunks(mut list: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8])> {
    std::iter::from_fn(move || {
        if list.len() < 8 {
            return None;
        }
        let id = fourcc(list);
        let size = le32(list, 4) as usize;
        let body = &list[8..];
        let size = size.min(body.len());
        let (chunk, rest) = body.split_at(size);
        list = rest.get(size & 1..).unwrap_or(&[]);
        Some((id, chunk))
    })
}

/// Put the index's entries in the streams' lists. Entries' offsets are
/// from the `movi` list's type, or in some files from the file's start:
/// the first entry, whose chunk must be there, says which. False if
/// neither fits.
fn read_index(
    entries: &[u8],
    movi: u32,
    len: u32,
    read: &mut dyn FnMut(u32, u32) -> Option<Vec<u8>>,
    avi: &mut Avi,
) -> bool {
    let first = entries
        .as_chunks::<16>()
        .0
        .iter()
        .find(|e| stream_of(&e[..4]).is_some_and(|s| s < avi.streams.len()));
    let Some(first) = first else {
        return false;
    };
    let id = fourcc(first);
    let offset = le32(first, 8);
    let fits = |base: u32, read: &mut dyn FnMut(u32, u32) -> Option<Vec<u8>>| {
        let at = base.wrapping_add(offset);
        at.checked_add(8).is_some_and(|e| e <= len)
            && read(at, 4).is_some_and(|b| b.len() == 4 && fourcc(&b) == id)
    };
    let base = if fits(movi, read) {
        movi
    } else if fits(0, read) {
        0
    } else {
        return false;
    };
    for e in entries.as_chunks::<16>().0 {
        let Some(s) = stream_of(&e[..4]).filter(|&s| s < avi.streams.len()) else {
            continue;
        };
        let flags = le32(e, 4);
        let data = base.wrapping_add(le32(e, 8)).wrapping_add(8);
        let size = le32(e, 12);
        if data.checked_add(size).is_none_or(|e| e > len) {
            continue;
        }
        avi.streams[s].chunks.push(Chunk {
            offset: data,
            size,
            flags,
        });
    }
    true
}

/// Read the chunks of the `movi` list from `at` to `end`, records'
/// chunks included: a file without an index.
fn scan(
    mut at: u32,
    end: u32,
    read: &mut dyn FnMut(u32, u32) -> Option<Vec<u8>>,
    avi: &mut Avi,
) -> Result<(), Error> {
    while at.saturating_add(8) <= end {
        let h = read(at, 12).ok_or(Error::Read)?;
        if h.len() < 8 {
            break;
        }
        let size = le32(&h, 4);
        if &h[..4] == b"LIST" && h.len() >= 12 && &h[8..12] == b"rec " {
            at += 12;
            continue;
        }
        if let Some(s) = stream_of(&h[..4]).filter(|&s| s < avi.streams.len()) {
            avi.streams[s].chunks.push(Chunk {
                offset: at + 8,
                size: size.min(end.saturating_sub(at + 8)),
                flags: AVIIF_KEYFRAME,
            });
        }
        let next = (at + 8).saturating_add(size).saturating_add(size & 1);
        if next <= at {
            break;
        }
        at = next;
    }
    Ok(())
}

crate::state_fields!(Chunk {
    offset,
    size,
    flags
});
crate::state_fields!(Stream {
    header,
    format,
    name,
    chunks
});
crate::state_fields!(Avi { main, streams });

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(id: &[u8], body: &[u8]) -> Vec<u8> {
        let mut v = id.to_vec();
        v.extend((body.len() as u32).to_le_bytes());
        v.extend(body);
        if body.len() % 2 == 1 {
            v.push(0);
        }
        v
    }

    fn list(kind: &[u8], items: &[Vec<u8>]) -> Vec<u8> {
        let mut body = kind.to_vec();
        for i in items {
            body.extend(i);
        }
        chunk(b"LIST", &body)
    }

    fn strh(kind: &[u8], sample_size: u32, length: u32) -> Vec<u8> {
        let mut h = vec![0; 56];
        h[..4].copy_from_slice(kind);
        h[20..24].copy_from_slice(&1u32.to_le_bytes());
        h[24..28].copy_from_slice(&15u32.to_le_bytes());
        h[32..36].copy_from_slice(&length.to_le_bytes());
        h[44..48].copy_from_slice(&sample_size.to_le_bytes());
        chunk(b"strh", &h)
    }

    /// A file of a video stream of two frames and an audio stream of 6
    /// samples of 2 bytes in two chunks, with an index if `index`.
    fn file(index: bool, absolute: bool) -> Vec<u8> {
        let hdrl = list(
            b"hdrl",
            &[
                chunk(b"avih", &[0; 56]),
                list(
                    b"strl",
                    &[
                        strh(b"vids", 0, 2),
                        chunk(b"strf", &[1, 2, 3]),
                        chunk(b"strn", b"v\0"),
                    ],
                ),
                list(b"strl", &[strh(b"auds", 2, 6), chunk(b"strf", &[4; 16])]),
            ],
        );
        let frames = [
            chunk(b"00dc", b"frame"),
            chunk(b"01wb", &[1, 1, 2, 2]),
            list(
                b"rec ",
                &[
                    chunk(b"00dc", b"second"),
                    chunk(b"01wb", &[3, 3, 4, 4, 5, 5, 6, 6]),
                ],
            ),
        ];
        let movi = list(b"movi", &frames);
        let mut body = b"AVI ".to_vec();
        body.extend(&hdrl);
        let movi_at = 12 + hdrl.len() as u32 + 8;
        body.extend(&movi);
        if index {
            // Offsets of each chunk from the movi list's type.
            let mut rel = Vec::new();
            let mut at = 4u32;
            for f in &frames[..2] {
                rel.push((fourcc(f), at, le32(f, 4)));
                at += f.len() as u32;
            }
            at += 12;
            let first = chunk(b"00dc", b"second");
            rel.push((*b"00dc", at, 6));
            rel.push((*b"01wb", at + first.len() as u32, 8));
            let mut idx = Vec::new();
            for (id, off, size) in rel {
                idx.extend(id);
                idx.extend(AVIIF_KEYFRAME.to_le_bytes());
                let off = if absolute { off + movi_at } else { off };
                idx.extend(off.to_le_bytes());
                idx.extend(size.to_le_bytes());
            }
            body.extend(chunk(b"idx1", &idx));
        }
        chunk(b"RIFF", &body)
    }

    fn open(bytes: &[u8]) -> Result<Avi, Error> {
        let mut read = |at: u32, n: u32| {
            let at = (at as usize).min(bytes.len());
            let end = (at + n as usize).min(bytes.len());
            Some(bytes[at..end].to_vec())
        };
        parse(bytes.len() as u32, &mut read)
    }

    fn data<'a>(bytes: &'a [u8], c: &Chunk) -> &'a [u8] {
        &bytes[c.offset as usize..(c.offset + c.size) as usize]
    }

    #[test]
    fn reads_streams_and_chunks_with_and_without_an_index() {
        for (index, absolute) in [(true, false), (true, true), (false, false)] {
            let bytes = file(index, absolute);
            let avi = open(&bytes).unwrap();
            assert_eq!(avi.streams.len(), 2);
            let (v, a) = (&avi.streams[0], &avi.streams[1]);
            assert_eq!(v.kind(), u32::from_le_bytes(*b"vids"));
            assert_eq!(v.format, [1, 2, 3]);
            assert_eq!(v.name, "v");
            assert_eq!(v.chunks.len(), 2, "index {index} absolute {absolute}");
            assert_eq!(data(&bytes, &v.chunks[0]), b"frame");
            assert_eq!(data(&bytes, &v.chunks[1]), b"second");
            assert_eq!(data(&bytes, &a.chunks[1]), [3, 3, 4, 4, 5, 5, 6, 6]);
            assert_eq!(a.locate(0), Some((0, 0)));
            assert_eq!(a.locate(1), Some((0, 2)));
            assert_eq!(a.locate(2), Some((1, 0)));
            assert_eq!(a.locate(5), Some((1, 6)));
            assert_eq!(a.locate(6), None);
            assert_eq!(v.locate(1), Some((1, 0)));
            assert_eq!(v.locate(2), None);
        }
    }

    #[test]
    fn palette_changes_are_no_samples() {
        let frames = [
            chunk(b"00db", b"one"),
            chunk(b"00pc", &[0, 1, 0, 0, 9, 9, 9, 0]),
            chunk(b"00db", b"two"),
        ];
        let strl = list(b"strl", &[strh(b"vids", 0, 2), chunk(b"strf", &[0; 40])]);
        let hdrl = list(b"hdrl", &[chunk(b"avih", &[0; 56]), strl]);
        for index in [false, true] {
            let mut body = b"AVI ".to_vec();
            body.extend(&hdrl);
            body.extend(list(b"movi", &frames));
            if index {
                let mut idx = Vec::new();
                let mut at = 4u32;
                for f in &frames {
                    idx.extend(&f[..4]);
                    idx.extend(AVIIF_KEYFRAME.to_le_bytes());
                    idx.extend(at.to_le_bytes());
                    idx.extend(&f[4..8]);
                    at += f.len() as u32;
                }
                body.extend(chunk(b"idx1", &idx));
            }
            let bytes = chunk(b"RIFF", &body);
            let avi = open(&bytes).unwrap();
            let v = &avi.streams[0];
            assert_eq!(v.chunks.len(), 2, "index {index}");
            assert_eq!(data(&bytes, &v.chunks[1]), b"two");
        }
    }

    #[test]
    fn refuses_what_isnt_an_avi_file() {
        assert_eq!(open(b"RIFF\x04\0\0\0WAVE").unwrap_err(), Error::Format);
        assert_eq!(open(b"").unwrap_err(), Error::Format);
        let mut bytes = file(true, false);
        bytes.truncate(40);
        assert!(open(&bytes).is_err());
    }

    #[test]
    fn survives_damaged_files() {
        let bytes = file(true, false);
        for cut in 0..bytes.len() {
            let _ = open(&bytes[..cut]);
        }
        let mut seed = 12345u32;
        for _ in 0..2000 {
            let mut b = bytes.clone();
            for _ in 0..4 {
                seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
                let at = (seed >> 8) as usize % b.len();
                b[at] = (seed >> 16) as u8;
            }
            if let Ok(avi) = open(&b) {
                for s in &avi.streams {
                    for c in &s.chunks {
                        assert!(c.offset as usize + c.size as usize <= b.len());
                    }
                }
            }
        }
    }
}
