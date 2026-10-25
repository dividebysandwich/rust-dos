//! RIFF files, as DirectMusic keeps its objects in them: a chunk is a
//! four-character id, a little-endian length and the data, padded to an
//! even length; "RIFF" and "LIST" chunks hold a form type and then
//! chunks of their own.

/// A chunk: its id, its form type for "RIFF" and "LIST" (0 otherwise),
/// and its data, without the form type.
#[derive(Clone, Copy, Debug)]
pub struct Chunk<'a> {
    pub id: [u8; 4],
    pub form: [u8; 4],
    pub data: &'a [u8],
}

impl<'a> Chunk<'a> {
    /// Whether it is a "RIFF" or "LIST" chunk, which holds chunks.
    pub fn is_list(&self) -> bool {
        &self.id == b"RIFF" || &self.id == b"LIST"
    }

    /// The chunks it holds, if it is a list.
    pub fn children(&self) -> Chunks<'a> {
        Chunks {
            data: if self.is_list() { self.data } else { &[] },
        }
    }

    /// The first chunk it holds with the id `id` (not a list).
    pub fn find(&self, id: &[u8; 4]) -> Option<Chunk<'a>> {
        self.children().find(|c| &c.id == id)
    }

    /// The first list it holds with the form type `form`.
    pub fn find_list(&self, form: &[u8; 4]) -> Option<Chunk<'a>> {
        self.children().find(|c| c.is_list() && &c.form == form)
    }

    /// Whether it is a list of the form `form`, or a chunk with the id.
    pub fn is(&self, id: &[u8; 4]) -> bool {
        if self.is_list() {
            &self.form == id
        } else {
            &self.id == id
        }
    }
}

/// The chunks one after another in some data.
pub struct Chunks<'a> {
    data: &'a [u8],
}

impl<'a> Chunks<'a> {
    pub fn new(data: &'a [u8]) -> Chunks<'a> {
        Chunks { data }
    }
}

impl<'a> Iterator for Chunks<'a> {
    type Item = Chunk<'a>;

    fn next(&mut self) -> Option<Chunk<'a>> {
        if self.data.len() < 8 {
            return None;
        }
        let id: [u8; 4] = self.data[..4].try_into().ok()?;
        let len = u32::from_le_bytes(self.data[4..8].try_into().ok()?) as usize;
        // A chunk longer than what is left is cut short to it, as
        // DirectMusic reads what there is of a file cut short.
        let end = 8usize.saturating_add(len).min(self.data.len());
        let body = &self.data[8..end];
        let next = 8usize.saturating_add(len).saturating_add(len & 1);
        self.data = self.data.get(next..).unwrap_or(&[]);
        let mut chunk = Chunk {
            id,
            form: [0; 4],
            data: body,
        };
        if chunk.is_list() {
            chunk.form = body.get(..4)?.try_into().ok()?;
            chunk.data = &body[4..];
        }
        Some(chunk)
    }
}

/// The file `data` as one chunk, if it is a RIFF file.
pub fn parse(data: &[u8]) -> Option<Chunk<'_>> {
    Chunks::new(data).next().filter(|c| &c.id == b"RIFF")
}

/// Little-endian numbers at `at` of `data`.
pub fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

pub fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

pub fn i32_at(data: &[u8], at: usize) -> Option<i32> {
    u32_at(data, at).map(|v| v as i32)
}

pub fn f64_at(data: &[u8], at: usize) -> Option<f64> {
    Some(f64::from_le_bytes(data.get(at..at + 8)?.try_into().ok()?))
}

/// A string of 16-bit characters up to its 0 or the end of `data`.
pub fn wide_string(data: &[u8]) -> String {
    let units: Vec<u16> = data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .take_while(|&u| u != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

/// A string of bytes up to its 0 or the end of `data`.
pub fn narrow_string(data: &[u8]) -> String {
    data.iter()
        .take_while(|&&b| b != 0)
        .map(|&b| b as char)
        .collect()
}

/// The items of an array chunk that starts with its items' size, as
/// DirectMusic writes its track data: each item's bytes, as many as the
/// size says, whatever the version of the structure that wrote them.
pub fn items(data: &[u8]) -> impl Iterator<Item = &[u8]> {
    let size = u32_at(data, 0).unwrap_or(0) as usize;
    let body = data.get(4..).unwrap_or(&[]);
    let size = if size == 0 { body.len().max(1) } else { size };
    body.chunks_exact(size)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A chunk's bytes.
    pub fn chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = id.to_vec();
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
        if data.len() % 2 == 1 {
            out.push(0);
        }
        out
    }

    fn list(id: &[u8; 4], form: &[u8; 4], children: &[Vec<u8>]) -> Vec<u8> {
        let mut data = form.to_vec();
        for c in children {
            data.extend_from_slice(c);
        }
        chunk(id, &data)
    }

    #[test]
    fn chunks_nest_and_pad_to_even_lengths() {
        let file = list(
            b"RIFF",
            b"DMSG",
            &[
                chunk(b"odd ", b"abc"),
                list(b"LIST", b"UNFO", &[chunk(b"UNAM", b"N\0a\0\0\0")]),
                chunk(b"last", b"z"),
            ],
        );
        let riff = parse(&file).unwrap();
        assert_eq!(&riff.form, b"DMSG");
        let ids: Vec<[u8; 4]> = riff.children().map(|c| c.id).collect();
        assert_eq!(ids, [*b"odd ", *b"LIST", *b"last"]);
        assert_eq!(riff.find(b"odd ").unwrap().data, b"abc");
        let unfo = riff.find_list(b"UNFO").unwrap();
        assert_eq!(wide_string(unfo.find(b"UNAM").unwrap().data), "Na");
        assert_eq!(riff.find(b"last").unwrap().data, b"z");
    }

    #[test]
    fn a_chunk_cut_short_gives_what_is_there() {
        let mut file = list(b"RIFF", b"DMSG", &[chunk(b"data", &[1, 2, 3, 4])]);
        file.truncate(file.len() - 2);
        let riff = parse(&file).unwrap();
        assert_eq!(riff.find(b"data").unwrap().data, [1, 2]);
        assert!(parse(b"RIF").is_none());
    }

    #[test]
    fn items_follow_the_size_the_chunk_gives() {
        let mut data = 6u32.to_le_bytes().to_vec();
        data.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13]);
        let all: Vec<&[u8]> = items(&data).collect();
        assert_eq!(all, [&[1, 2, 3, 4, 5, 6][..], &[7, 8, 9, 10, 11, 12]]);
    }
}
