//! Reading Windows help files (.HLP), after Wine's winhlp32 hlpfile.c
//! (Copyright 1996 Ulrich Schmid, 2002, 2008 Eric Pouech, 2007 Kirill K.
//! Smirnov; LGPL-2.1+): the file's own file system, its topics, their
//! paragraphs and fonts, and the links from one topic to another.
//!
//! A help file holds internal files in a B+ tree: |SYSTEM (the version
//! and title), |TOPIC (the topics' text, in blocks, LZ77-compressed from
//! Windows 3.1 on), |Phrases (common strings the text refers to by
//! number), |FONT, |TOMAP (Windows 3.0: topic numbers to offsets),
//! |CONTEXT (Windows 3.1: context string hashes to offsets), |CTXOMAP
//! (the program's context numbers) and the pictures, |bm0 and on.

use std::collections::HashMap;

fn u16_at(b: &[u8], i: usize) -> u16 {
    b.get(i..i + 2)
        .map_or(0, |s| u16::from_le_bytes([s[0], s[1]]))
}

fn i16_at(b: &[u8], i: usize) -> i16 {
    u16_at(b, i) as i16
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    b.get(i..i + 4)
        .map_or(0, |s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// The bytes of `b` from `i` up to the next zero, without it.
fn c_str(b: &[u8], i: usize) -> &[u8] {
    let s = b.get(i..).unwrap_or(&[]);
    &s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())]
}

/// A reader of the variable-length numbers paragraph formats are made of
/// (Wine's fetch_long, fetch_ushort and the like): a number is two or
/// four bytes, or one or two, by the low bit of its first byte.
struct Fetch<'a> {
    b: &'a [u8],
    at: usize,
}

impl Fetch<'_> {
    fn byte(&mut self) -> u8 {
        let v = self.b.get(self.at).copied().unwrap_or(0);
        self.at += 1;
        v
    }

    fn word(&mut self) -> u16 {
        let v = u16_at(self.b, self.at);
        self.at += 2;
        v
    }

    fn long(&mut self) -> i32 {
        if self.b.get(self.at).copied().unwrap_or(0) & 1 != 0 {
            let v = u32_at(self.b, self.at).wrapping_sub(0x8000_0000) as i32 / 2;
            self.at += 4;
            v
        } else {
            let v = (u16_at(self.b, self.at) as i32 - 0x8000) / 2;
            self.at += 2;
            v
        }
    }

    fn short(&mut self) -> i32 {
        if self.b.get(self.at).copied().unwrap_or(0) & 1 != 0 {
            let v = (u16_at(self.b, self.at) as i32 - 0x8000) / 2;
            self.at += 2;
            v
        } else {
            (self.byte() as i32 - 0x80) / 2
        }
    }

    fn ushort(&mut self) -> u32 {
        if self.b.get(self.at).copied().unwrap_or(0) & 1 != 0 {
            let v = u16_at(self.b, self.at) as u32 / 2;
            self.at += 2;
            v
        } else {
            self.byte() as u32 / 2
        }
    }
}

/// Undo the LZ77 compression of `src` (Wine's HLPFILE_UncompressLZ77):
/// a flag byte for each eight items, each a byte as it is or a 12-bit
/// distance back and a length.
pub fn lz77(src: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(src.len() * 2);
    let mut i = 0;
    while i < src.len() {
        let mask = src[i];
        i += 1;
        for bit in 0..8 {
            if i >= src.len() {
                break;
            }
            if mask >> bit & 1 != 0 {
                let code = u16_at(src, i) as usize;
                i += 2;
                let len = 3 + (code >> 12);
                let back = (code & 0xFFF) + 1;
                for _ in 0..len {
                    let c = out.len().checked_sub(back).map_or(0, |at| out[at]);
                    out.push(c);
                }
            } else {
                out.push(src[i]);
                i += 1;
            }
        }
    }
    out
}

/// Undo the run-length packing of `src` into `size` bytes (Wine's
/// HLPFILE_UncompressRLE).
fn rle(src: &[u8], size: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(size);
    let mut i = 0;
    while i < src.len() {
        let c = src[i];
        i += 1;
        let n = (c & 0x7F) as usize;
        if c & 0x80 != 0 {
            out.extend_from_slice(src.get(i..i + n).unwrap_or(&[]));
            i += n;
        } else {
            let b = src.get(i).copied().unwrap_or(0);
            out.extend(std::iter::repeat_n(b, n));
            i += 1;
        }
    }
    out.resize(size, 0);
    out
}

/// A picture: its size and the packed DIB that draws it (header,
/// colours and bits).
#[derive(Clone, Debug)]
pub struct Picture {
    pub width: i32,
    pub height: i32,
    pub dib: Vec<u8>,
}

/// The colours of a device-dependent bitmap of `bpp` bits: black and
/// white, or the VGA's sixteen.
fn ddb_colors(bpp: u32) -> Vec<u32> {
    const VGA: [u32; 16] = [
        0x000000, 0x800000, 0x008000, 0x808000, 0x000080, 0x800080, 0x008080, 0xC0C0C0, 0x808080,
        0xFF0000, 0x00FF00, 0xFFFF00, 0x0000FF, 0xFF00FF, 0x00FFFF, 0xFFFFFF,
    ];
    match bpp {
        1 => vec![0x000000, 0xFFFFFF],
        4 => VGA.to_vec(),
        _ => Vec::new(),
    }
}

/// The phrases text refers to, and how it refers to them.
#[derive(Debug, Default)]
struct Phrases {
    /// Where each phrase starts in `text`, and where the last ends.
    offsets: Vec<usize>,
    text: Vec<u8>,
    /// Windows 95's |PhrIndex and |PhrImage (Uncompress3), rather than
    /// |Phrases (Uncompress2).
    hall: bool,
}

impl Phrases {
    fn phrase(&self, i: usize) -> &[u8] {
        match (self.offsets.get(i), self.offsets.get(i + 1)) {
            (Some(&a), Some(&b)) if a <= b => self.text.get(a..b).unwrap_or(&[]),
            _ => &[],
        }
    }

    /// The text of `src`, `size` bytes, with the phrases put in.
    fn expand(&self, src: &[u8], size: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(size);
        let mut i = 0;
        if !self.hall {
            // HLPFILE_Uncompress2.
            while i < src.len() && out.len() < size {
                let c = src[i];
                if c == 0 || c >= 0x10 {
                    out.push(c);
                    i += 1;
                    continue;
                }
                let code = (c as usize) << 8 | src.get(i + 1).copied().unwrap_or(0) as usize;
                out.extend_from_slice(self.phrase((code - 0x100) / 2));
                if code & 1 != 0 {
                    out.push(b' ');
                }
                i += 2;
            }
        } else {
            // HLPFILE_Uncompress3.
            while i < src.len() {
                let c = src[i];
                if c & 1 == 0 {
                    out.extend_from_slice(self.phrase(c as usize / 2));
                } else if c & 3 == 1 {
                    i += 1;
                    let n = (c as usize + 1) * 64 + src.get(i).copied().unwrap_or(0) as usize;
                    out.extend_from_slice(self.phrase(n));
                } else if c & 7 == 3 {
                    let n = c as usize / 8 + 1;
                    out.extend_from_slice(src.get(i + 1..i + 1 + n).unwrap_or(&[]));
                    i += n;
                } else {
                    let n = c as usize / 16 + 1;
                    let fill = if c & 0x0F == 0x07 { b' ' } else { 0 };
                    out.extend(std::iter::repeat_n(fill, n));
                }
                i += 1;
            }
        }
        out.resize(size, 0);
        out
    }
}

/// A font of the file's |FONT table.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Font {
    pub face: String,
    /// The size in half points.
    pub half_points: u8,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    /// The family: 1 modern, 2 roman, 3 swiss, 4 script, 5 decorative.
    pub family: u8,
    /// The colour, as a COLORREF.
    pub color: u32,
}

/// A topic: its title and where its records are.
#[derive(Clone, Debug, Default)]
pub struct Page {
    pub title: String,
    /// The topic offset that links and |CONTEXT give.
    pub offset: u32,
    /// Where its first record is in the topic data, as the records'
    /// own links count.
    reference: u32,
    /// The topics before and after it in the browse sequence, by topic
    /// offset, or None.
    pub browse_back: Option<u32>,
    pub browse_next: Option<u32>,
    /// The macros the topic runs when it is shown.
    pub macros: Vec<String>,
}

/// What a link does.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// Show another topic, by its context hash (a topic number in a
    /// Windows 3.0 file), of the file named (None for this one).
    Jump { file: Option<String>, hash: u32 },
    /// Show a topic in a popup.
    Popup { file: Option<String>, hash: u32 },
    /// Run a macro.
    Macro(String),
}

/// A link: what it does, and whether its text is shown green and
/// underlined (dotted for a popup).
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub target: Target,
    pub marked: bool,
}

/// How a paragraph's lines line up.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Align {
    #[default]
    Left,
    Right,
    Center,
}

/// A piece of a paragraph.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Text(Vec<u8>),
    /// The text from here on is in the file's font with this number.
    Font(u16),
    LineBreak,
    Tab,
    /// A link starts: the text up to `LinkEnd` is it.
    Link(Link),
    LinkEnd,
    /// A picture: the number of its |bm file.
    Picture(u16),
}

/// A paragraph: its format and its pieces. Distances are in half
/// points.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Paragraph {
    pub space_before: i32,
    pub space_after: i32,
    pub line_spacing: i32,
    pub left: i32,
    pub right: i32,
    pub first: i32,
    pub align: Align,
    /// The tab stops, and for each 0 for left, 1 right, 2 centred.
    pub tabs: Vec<(i32, u8)>,
    /// The paragraph is in the topic's non-scrolling region.
    pub keep: bool,
    pub items: Vec<Item>,
}

/// A help file, read.
#[derive(Debug, Default)]
pub struct HelpFile {
    pub title: String,
    /// The |SYSTEM minor version: 15 for Windows 3.0, 21 for 3.1, 33
    /// for Windows 95's.
    version: u16,
    /// The size of a topic block in the decompressed topic data.
    dsize: u32,
    /// The topic data, its blocks one after the other, and where each
    /// starts.
    topic: Vec<u8>,
    blocks: Vec<usize>,
    phrases: Option<Phrases>,
    pub fonts: Vec<Font>,
    pub pages: Vec<Page>,
    /// Windows 3.0: the topics' offsets by number.
    tomap: Vec<u32>,
    /// Windows 3.1: the topics' offsets by context hash.
    context: HashMap<u32, u32>,
    /// The program's context numbers' topic offsets.
    map: Vec<(u32, u32)>,
    contents: Option<u32>,
    /// The file's own internal files, by name, as ranges of `data`.
    files: HashMap<String, (usize, usize)>,
    data: Vec<u8>,
}

/// The hash of a context string (Wine's HLPFILE_Hash), which links and
/// |CONTEXT know topics by.
pub fn hash(context: &str) -> u32 {
    let mut h: u32 = 0;
    for c in context.bytes() {
        let x = match c {
            b'A'..=b'Z' => c - b'A' + 17,
            b'a'..=b'z' => c - b'a' + 17,
            b'1'..=b'9' => c - b'0',
            b'0' => 10,
            b'.' => 12,
            b'_' => 13,
            _ => 0,
        };
        if x != 0 {
            h = h.wrapping_mul(43).wrapping_add(x as u32);
        }
    }
    h
}

/// The leaf entries of the B+ tree at the start of `b` (a file's header
/// skipped), as Wine's HLPFILE_BPTreeEnum walks them: `entry` gives the
/// size of the entry at an index in a leaf page.
fn tree_leaves(b: &[u8], entry: &dyn Fn(&[u8], usize) -> usize) -> Vec<usize> {
    let mut out = Vec::new();
    if u16_at(b, 0) != 0x293B {
        return out;
    }
    let page_size = u16_at(b, 4) as usize;
    let mut page = u16_at(b, 26) as usize;
    let levels = u16_at(b, 32);
    let pages = 38;
    for _ in 1..levels {
        page = u16_at(b, pages + page * page_size + 4) as usize;
    }
    let mut seen = 0;
    while page != 0xFFFF && seen < 0x10000 {
        seen += 1;
        let start = pages + page * page_size;
        if start + 8 > b.len() {
            break;
        }
        let entries = i16_at(b, start + 2).max(0);
        let mut at = start + 8;
        for _ in 0..entries {
            if at >= b.len() {
                break;
            }
            out.push(at);
            at += entry(b, at);
        }
        page = u16_at(b, start + 6) as usize;
    }
    out
}

impl HelpFile {
    /// Read the help file `data`, or say why it can't be.
    pub fn parse(data: Vec<u8>) -> Result<HelpFile, &'static str> {
        if u32_at(&data, 0) != 0x0003_5F3F {
            return Err("not a help file");
        }
        let mut f = HelpFile {
            data,
            ..HelpFile::default()
        };
        f.read_directory();
        f.read_system()?;
        if f.version <= 16 {
            let tomap = f.file("|TOMAP").ok_or("no |TOMAP")?;
            f.tomap = tomap
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| u32::from_le_bytes(*c))
                .collect();
        }
        f.phrases = f.read_phrases();
        f.read_topic()?;
        f.read_fonts();
        f.read_pages();
        if let Some(m) = f.file("|CTXOMAP") {
            let n = u16_at(m, 0) as usize;
            f.map = (0..n)
                .map(|i| (u32_at(m, 2 + i * 8), u32_at(m, 2 + i * 8 + 4)))
                .collect();
        }
        if f.version > 16
            && let Some(c) = f.file("|CONTEXT")
        {
            let leaves = tree_leaves(c, &|_, _| 8);
            let context = leaves
                .iter()
                .map(|&at| (u32_at(c, at), u32_at(c, at + 4)))
                .collect();
            f.context = context;
        }
        if f.pages.is_empty() {
            return Err("no topics");
        }
        Ok(f)
    }

    fn read_directory(&mut self) {
        let dir = u32_at(&self.data, 4) as usize;
        let Some(tree) = self.data.get(dir + 9..) else {
            return;
        };
        let leaves = tree_leaves(tree, &|b, at| c_str(b, at).len() + 5);
        for at in leaves {
            let name = String::from_utf8_lossy(c_str(tree, at)).into_owned();
            let off = u32_at(tree, at + name.len() + 1) as usize;
            // A file's header: the space it has, the space it uses and
            // a flag byte.
            let used = u32_at(&self.data, off + 4) as usize;
            let start = off + 9;
            let end = (start + used).min(self.data.len());
            if start <= end {
                self.files.insert(name, (start, end));
            }
        }
    }

    /// The internal file `name`, its header skipped.
    pub fn file(&self, name: &str) -> Option<&[u8]> {
        let &(a, b) = self.files.get(name)?;
        self.data.get(a..b)
    }

    fn read_system(&mut self) -> Result<(), &'static str> {
        let s = self.file("|SYSTEM").ok_or("no |SYSTEM")?;
        let (magic, minor, major) = (u16_at(s, 0), u16_at(s, 2), u16_at(s, 4));
        let flags = u16_at(s, 10);
        if magic != 0x036C || major != 1 {
            return Err("unknown |SYSTEM");
        }
        let (tbsize, compressed) = match (minor, flags) {
            (0..=16, _) => (0x800, false),
            (_, 0) => (0x1000, false),
            (_, 4) => (0x1000, true),
            _ => (0x800, true),
        };
        let dsize = if compressed { 0x4000 } else { tbsize - 0x0C };
        let mut title = String::new();
        let mut contents = None;
        if minor <= 16 {
            title = String::from_utf8_lossy(c_str(s, 12)).into_owned();
        } else {
            let mut at = 12;
            while at + 4 <= s.len() {
                let (kind, len) = (u16_at(s, at), u16_at(s, at + 2) as usize);
                match kind {
                    1 if title.is_empty() => {
                        title = String::from_utf8_lossy(c_str(s, at + 4)).into_owned();
                    }
                    3 if len == 4 => contents = Some(u32_at(s, at + 4)),
                    _ => {}
                }
                at += len + 4;
            }
        }
        self.version = minor;
        self.dsize = dsize;
        self.title = title;
        self.contents = contents;
        // Kept for read_topic.
        self.blocks = vec![tbsize as usize, compressed as usize];
        Ok(())
    }

    fn read_phrases(&self) -> Option<Phrases> {
        if let Some(p) = self.file("|Phrases") {
            let n = u16_at(p, 0) as usize;
            let head = if self.version <= 16 { 4 } else { 8 };
            let data = head + 2 * (n + 1);
            if data > p.len() {
                return None;
            }
            let text = if self.version <= 16 {
                p[data..].to_vec()
            } else {
                lz77(&p[data..])
            };
            let offsets = (0..=n)
                .map(|i| (u16_at(p, head + 2 * i) as usize).saturating_sub(2 * n + 2))
                .collect();
            return Some(Phrases {
                offsets,
                text,
                hall: false,
            });
        }
        let index = self.file("|PhrIndex")?;
        let image = self.file("|PhrImage")?;
        let n = u16_at(index, 4) as usize;
        let bc = u16_at(index, 24) & 0x0F;
        let (dec, cpr) = (u32_at(index, 12) as usize, u32_at(index, 16) as usize);
        let bits = index.get(28..).unwrap_or(&[]);
        let mut bit = 0usize;
        let mut next = || {
            let word = u32_at(bits, bit / 32 * 4);
            let set = word >> (bit % 32) & 1 != 0;
            bit += 1;
            set
        };
        let mut offsets = vec![0usize];
        for i in 0..n {
            let mut len = 1usize;
            while next() {
                len += 1 << bc;
            }
            if next() {
                len += 1;
            }
            for (min, add) in [(1, 2), (2, 4), (3, 8), (4, 16)] {
                if bc > min && next() {
                    len += add;
                }
            }
            offsets.push(offsets[i] + len);
        }
        let text = if dec == cpr {
            image.to_vec()
        } else {
            lz77(image)
        };
        Some(Phrases {
            offsets,
            text,
            hall: true,
        })
    }

    fn read_topic(&mut self) -> Result<(), &'static str> {
        let (tbsize, compressed) = (self.blocks[0], self.blocks[1] != 0);
        let t = self.file("|TOPIC").ok_or("no |TOPIC")?;
        let n = (t.len().max(1) - 1) / tbsize + 1;
        let mut topic = Vec::new();
        let mut blocks = Vec::with_capacity(n);
        for i in 0..n {
            blocks.push(topic.len());
            if compressed {
                let mut at = i * tbsize;
                if at + 0x44 > t.len() {
                    at = t.len().saturating_sub(0x44);
                }
                let end = (at + tbsize).min(t.len());
                topic.extend(lz77(t.get(at + 0x0C..end).unwrap_or(&[])));
            } else {
                let at = i * tbsize + 0x0C;
                let dsize = self.dsize as usize;
                let mut block = t.get(at..(at + dsize).min(t.len())).unwrap_or(&[]).to_vec();
                block.resize(dsize, 0);
                topic.extend(block);
            }
        }
        self.topic = topic;
        self.blocks = blocks;
        Ok(())
    }

    fn read_fonts(&mut self) {
        let Some(f) = self.file("|FONT") else {
            return;
        };
        let faces = u16_at(f, 0) as usize;
        let count = u16_at(f, 2) as usize;
        let face_at = u16_at(f, 4) as usize;
        let desc_at = u16_at(f, 6) as usize;
        let len = desc_at.saturating_sub(face_at) / faces.max(1);
        let mut fonts = Vec::with_capacity(count);
        for i in 0..count {
            let d = desc_at + i * 11;
            let flags = f.get(d).copied().unwrap_or(0);
            let face = u16_at(f, d + 3) as usize;
            let face = if face < faces {
                let at = face_at + face * len;
                let name = f.get(at..at + len).unwrap_or(&[]);
                String::from_utf8_lossy(c_str(name, 0)).into_owned()
            } else {
                "Helv".to_string()
            };
            let byte = |k: usize| f.get(d + k).copied().unwrap_or(0) as u32;
            fonts.push(Font {
                face,
                half_points: byte(1) as u8,
                bold: flags & 1 != 0,
                italic: flags & 2 != 0,
                underline: flags & 4 != 0,
                strike: flags & 8 != 0,
                family: byte(2) as u8,
                color: byte(5) | byte(6) << 8 | byte(7) << 16,
            });
        }
        self.fonts = fonts;
    }

    /// Walk the topic records from `reference` on (both of Wine's walks,
    /// in HLPFILE_DoReadHlpFile and HLPFILE_BrowsePage): `each` gets each
    /// record, its block, its reference and the record's bytes up to its
    /// end, and says whether to go on.
    fn walk(&self, reference: u32, each: &mut dyn FnMut(usize, u32, &[u8]) -> bool) {
        let mut r = reference;
        let mut old_index = usize::MAX;
        let mut steps = 0;
        loop {
            steps += 1;
            if steps > 0x10_0000 {
                break;
            }
            let at = r.wrapping_sub(0x0C);
            let (index, mut offset) = if self.version <= 16 {
                let d = self.dsize.max(1);
                ((at / d) as usize, (at % d) as i64)
            } else {
                ((at >> 14) as usize, (at & 0x3FFF) as i64)
            };
            if self.version <= 16 && index != old_index && old_index != usize::MAX {
                // The next block: its header isn't in the topic data.
                r = r.wrapping_sub(12);
                offset -= 12;
            }
            let Some(&block) = self.blocks.get(index) else {
                break;
            };
            let pos = block as i64 + offset;
            if pos < 0 || pos as usize + 0x15 >= self.topic.len() {
                break;
            }
            let pos = pos as usize;
            let size = u32_at(&self.topic, pos) as usize;
            let end = pos.saturating_add(size).min(self.topic.len());
            let rec = &self.topic[pos..end.max(pos)];
            old_index = index;
            if !each(index, r, rec) {
                break;
            }
            let next = u32_at(rec, 0x0C);
            if self.version <= 16 {
                if next == 0 {
                    break;
                }
                r = r.wrapping_add(next);
            } else {
                r = next;
                if r == 0xFFFF_FFFF {
                    break;
                }
            }
        }
    }

    /// The text of a record, with its phrases put in.
    fn record_text(&self, rec: &[u8]) -> Vec<u8> {
        let block = u32_at(rec, 0) as usize;
        let size = u32_at(rec, 4) as usize;
        let data = (u32_at(rec, 0x10) as usize).min(rec.len());
        let src = &rec[data..];
        if size > block.saturating_sub(data) {
            if let Some(p) = &self.phrases {
                return p.expand(src, size);
            }
            return src.to_vec();
        }
        src.get(..size).unwrap_or(src).to_vec()
    }

    fn read_pages(&mut self) {
        let mut pages = Vec::new();
        let mut offs = 0u32;
        let mut old_index = usize::MAX;
        let version = self.version;
        let tomap = &self.tomap;
        let browse = |v: u32| -> Option<u32> {
            if v == 0xFFFF || v == 0xFFFF_FFFF {
                None
            } else if version <= 16 {
                tomap.get(v as usize).copied()
            } else {
                Some(v)
            }
        };
        self.walk(0x0C, &mut |index, r, rec| {
            if index != old_index {
                offs = 0;
                old_index = index;
            }
            match rec.get(0x14).copied().unwrap_or(0) {
                0x02 => {
                    let offset = if version <= 16 {
                        r.wrapping_add(index as u32 * 12)
                    } else {
                        index as u32 * 0x8000 + offs
                    };
                    let text = self.record_text(rec);
                    let mut parts = text.split(|&c| c == 0);
                    let title = parts.next().unwrap_or(&[]);
                    let macros = parts
                        .filter(|m| !m.is_empty())
                        .map(|m| String::from_utf8_lossy(m).into_owned())
                        .collect();
                    pages.push(Page {
                        title: String::from_utf8_lossy(title).into_owned(),
                        offset,
                        reference: r,
                        browse_back: browse(u32_at(rec, 0x19)),
                        browse_next: browse(u32_at(rec, 0x1D)),
                        macros,
                    });
                }
                0x01 | 0x20 | 0x23 => {
                    let len = if matches!(rec[0x14], 0x20 | 0x23) {
                        let mut f = Fetch { b: rec, at: 0x15 };
                        f.long();
                        f.ushort()
                    } else {
                        (rec.len() as u32).saturating_sub(15)
                    };
                    offs = offs.wrapping_add(len);
                }
                _ => {}
            }
            true
        });
        self.pages = pages;
    }

    /// The picture with the number `n` (Wine's HLPFILE_RtfAddGfxByIndex
    /// and HLPFILE_RtfAddBitmap): the first of its forms, if it is a
    /// bitmap. Colours are RGB; a transparent picture's last colour is
    /// `background`.
    pub fn picture(&self, n: u16, background: u32) -> Option<Picture> {
        let name = format!("|bm{n}");
        let b = self.file(&name).or_else(|| self.file(&name[1..]))?;
        let beg = u32_at(b, 4) as usize;
        let (kind, pack) = (*b.get(beg)?, *b.get(beg + 1)?);
        if kind != 5 && kind != 6 {
            return None;
        }
        let mut f = Fetch { b, at: beg + 2 };
        let ulong = |f: &mut Fetch| -> u32 {
            if f.b.get(f.at).copied().unwrap_or(0) & 1 != 0 {
                let v = u32_at(f.b, f.at) / 2;
                f.at += 4;
                v
            } else {
                f.word() as u32 / 2
            }
        };
        ulong(&mut f);
        ulong(&mut f);
        let planes = f.ushort();
        let bpp = f.ushort();
        let width = ulong(&mut f) as i32;
        let height = ulong(&mut f) as i32;
        let used = ulong(&mut f);
        let important = ulong(&mut f);
        let csize = ulong(&mut f) as usize;
        ulong(&mut f);
        let off = u32_at(b, f.at) as usize;
        f.at += 8;
        if planes != 1 || !matches!(bpp, 1 | 4 | 8 | 24) || width <= 0 || height <= 0 {
            return None;
        }
        if width > 4096 || height > 4096 {
            return None;
        }
        let mut colors = if kind == 6 {
            let n = if used == 0 && bpp <= 8 {
                1 << bpp
            } else {
                used
            } as usize;
            (0..n.min(256))
                .map(|i| {
                    let c = f.at + i * 4;
                    let byte = |k: usize| b.get(c + k).copied().unwrap_or(0) as u32;
                    byte(2) | byte(1) << 8 | byte(0) << 16
                })
                .collect()
        } else {
            ddb_colors(bpp)
        };
        if bpp <= 8 && colors.is_empty() {
            return None;
        }
        if important == 1
            && let Some(last) = colors.last_mut()
        {
            *last = background;
        }
        let stride = (width as usize * bpp as usize).div_ceil(32) * 4;
        let size = stride * height as usize;
        let at = beg + off;
        let src = b.get(at..(at + csize).min(b.len())).unwrap_or(&[]);
        let mut bits = match pack {
            0 => src.to_vec(),
            1 => rle(src, size),
            2 => lz77(src),
            3 => rle(&lz77(src), size),
            _ => return None,
        };
        if kind == 5 {
            // A DDB's rows are top down and 16-bit aligned.
            let ddb_stride = (width as usize * bpp as usize).div_ceil(16) * 2;
            let mut dib = vec![0u8; size];
            for y in 0..height as usize {
                let from = y * ddb_stride;
                let to = (height as usize - 1 - y) * stride;
                let row = bits.get(from..from + ddb_stride.min(stride)).unwrap_or(&[]);
                dib[to..to + row.len()].copy_from_slice(row);
            }
            bits = dib;
        }
        bits.resize(size, 0);
        let mut dib = Vec::with_capacity(40 + colors.len() * 4 + size);
        for v in [40u32, width as u32, height as u32] {
            dib.extend_from_slice(&v.to_le_bytes());
        }
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&(bpp as u16).to_le_bytes());
        for v in [0u32, size as u32, 0, 0, colors.len() as u32, 0] {
            dib.extend_from_slice(&v.to_le_bytes());
        }
        for c in &colors {
            dib.extend_from_slice(&[(c >> 16) as u8, (c >> 8) as u8, *c as u8, 0]);
        }
        dib.extend_from_slice(&bits);
        Some(Picture { width, height, dib })
    }

    /// The topic that the topic offset `offset` is in (Wine's
    /// HLPFILE_PageByOffset).
    pub fn page_by_offset(&self, offset: u32) -> Option<usize> {
        if offset == 0xFFFF_FFFF {
            return None;
        }
        let mut found: Option<usize> = None;
        for (i, p) in self.pages.iter().enumerate() {
            if p.offset <= offset && found.is_none_or(|f| self.pages[f].offset < p.offset) {
                found = Some(i);
            }
        }
        found
    }

    /// The contents topic: the one |SYSTEM names, or in a Windows 3.0
    /// file the first one |TOMAP has (topic number 0), or else the first
    /// topic.
    pub fn contents(&self) -> usize {
        let start = match self.contents {
            Some(c) => Some(c),
            None if self.version <= 16 => self.tomap.first().copied(),
            None => None,
        };
        start.and_then(|c| self.page_by_offset(c)).unwrap_or(0)
    }

    /// The topic a link's hash names, 0 for the contents.
    pub fn page_by_hash(&self, hash: u32) -> Option<usize> {
        if hash == 0 {
            return Some(self.contents());
        }
        if self.version <= 16 {
            return self
                .tomap
                .get(hash as usize)
                .and_then(|&o| self.page_by_offset(o));
        }
        self.context
            .get(&hash)
            .and_then(|&o| self.page_by_offset(o))
    }

    /// The topic of the program's context number `number`.
    pub fn page_by_map(&self, number: u32) -> Option<usize> {
        let &(_, offset) = self.map.iter().find(|m| m.0 == number)?;
        self.page_by_offset(offset)
    }

    /// The paragraphs of the topic `page`.
    pub fn paragraphs(&self, page: usize) -> Vec<Paragraph> {
        let mut out = Vec::new();
        let Some(p) = self.pages.get(page) else {
            return out;
        };
        let mut topics = 0;
        self.walk(p.reference, &mut |_, _, rec| {
            match rec.get(0x14).copied().unwrap_or(0) {
                0x02 => {
                    topics += 1;
                    if topics > 1 {
                        return false;
                    }
                }
                0x01 | 0x20 | 0x23 => self.paragraph(rec, &mut out),
                _ => {}
            }
            true
        });
        out
    }

    /// The paragraphs of the record `rec` (Wine's
    /// HLPFILE_BrowseParagraph), added to `out`. A table's cells come
    /// one after the other.
    fn paragraph(&self, rec: &[u8], out: &mut Vec<Paragraph>) {
        if rec.len() < 0x19 {
            return;
        }
        let kind = rec[0x14];
        let text = self.record_text(rec);
        let format_end = (u32_at(rec, 0x10) as usize).min(rec.len());
        let mut f = Fetch { b: rec, at: 0x15 };
        if kind == 0x20 || kind == 0x23 {
            f.long();
            f.ushort();
        }
        let mut columns = 1;
        let table = kind == 0x23;
        if table {
            columns = f.byte() as i32;
            let kind = f.byte();
            if kind == 0 || kind == 2 {
                f.word();
            }
            f.at += columns as usize * 4;
        }
        let mut t = 0usize;
        let mut column = 0;
        while column < columns {
            if table {
                let c = i16_at(rec, f.at);
                if c == -1 {
                    break;
                }
                column = c as i32;
                f.at += 5;
            } else {
                column += 1;
            }
            f.at += if kind == 0x01 { 6 } else { 4 };
            let mut par = Paragraph::default();
            let bits = f.word();
            if bits & 0x0001 != 0 {
                f.long();
            }
            if bits & 0x0002 != 0 {
                par.space_before = f.short();
            }
            if bits & 0x0004 != 0 {
                par.space_after = f.short();
            }
            if bits & 0x0008 != 0 {
                par.line_spacing = f.short();
            }
            if bits & 0x0010 != 0 {
                par.left = f.short();
            }
            if bits & 0x0020 != 0 {
                par.right = f.short();
            }
            if bits & 0x0040 != 0 {
                par.first = f.short();
            }
            if bits & 0x0100 != 0 {
                // The border: its sides and width.
                f.byte();
                f.word();
            }
            if bits & 0x0200 != 0 {
                let n = f.short();
                for _ in 0..n.max(0) {
                    let tab = f.ushort();
                    let kind = if tab & 0x4000 != 0 { f.ushort() } else { 0 };
                    par.tabs.push(((tab & 0x3FFF) as i32, kind as u8));
                }
            }
            par.align = match bits & 0xC00 {
                0x400 => Align::Right,
                0x800 => Align::Center,
                _ => Align::Left,
            };
            par.keep = bits & 0x1000 != 0;
            let format = par.clone();
            while t < text.len() && f.at < format_end {
                let s = c_str(&text, t);
                if !s.is_empty() {
                    par.items.push(Item::Text(s.to_vec()));
                }
                t += s.len() + 1;
                let code = f.byte();
                if code == 0xFF {
                    break;
                }
                match code {
                    0x20 => f.at += 4,
                    0x21 => f.at += 2,
                    0x80 => par.items.push(Item::Font(f.word())),
                    0x81 => par.items.push(Item::LineBreak),
                    0x82 => {
                        let next = Paragraph {
                            items: Vec::new(),
                            ..format.clone()
                        };
                        out.push(std::mem::replace(&mut par, next));
                    }
                    0x83 => par.items.push(Item::Tab),
                    0x86..=0x88 => {
                        let kind = f.byte();
                        let size = f.long();
                        if kind == 0x22 {
                            // The number of hot spots.
                            f.ushort();
                        }
                        let start = f.at;
                        if (kind == 0x03 || kind == 0x22) && i16_at(rec, f.at) == 0 {
                            par.items.push(Item::Picture(u16_at(rec, f.at + 2)));
                        }
                        f.at = start + size.max(0) as usize;
                    }
                    0x89 => par.items.push(Item::LinkEnd),
                    0x8B => par.items.push(Item::Text(vec![0xA0])),
                    0x8C => par.items.push(Item::Text(vec![b'-'])),
                    0xC8 | 0xCC => {
                        let len = f.word() as usize;
                        let text = rec.get(f.at..(f.at + len).min(rec.len())).unwrap_or(&[]);
                        let text = String::from_utf8_lossy(c_str(text, 0)).into_owned();
                        par.items.push(Item::Link(Link {
                            target: Target::Macro(text),
                            marked: code & 4 == 0,
                        }));
                        f.at += len;
                    }
                    0xE0..=0xE3 | 0xE6 | 0xE7 => {
                        let hash = u32_at(rec, f.at);
                        f.at += 4;
                        let target = if code & 1 != 0 {
                            Target::Jump { file: None, hash }
                        } else {
                            Target::Popup { file: None, hash }
                        };
                        let marked = code <= 0xE1 || code & 4 == 0;
                        par.items.push(Item::Link(Link { target, marked }));
                    }
                    0xEA | 0xEB | 0xEE | 0xEF => {
                        let len = f.word() as usize;
                        let start = f.at;
                        let ty = rec.get(start).copied().unwrap_or(0);
                        let hash = u32_at(rec, start + 1);
                        let mut name = start + 5;
                        let file = match ty {
                            0 | 1 => None,
                            6 => {
                                name += c_str(rec, name).len() + 1;
                                Some(String::from_utf8_lossy(c_str(rec, name)).into_owned())
                            }
                            4 => Some(String::from_utf8_lossy(c_str(rec, name)).into_owned()),
                            _ => None,
                        };
                        let target = if code & 1 != 0 {
                            Target::Jump { file, hash }
                        } else {
                            Target::Popup { file, hash }
                        };
                        par.items.push(Item::Link(Link {
                            target,
                            marked: code & 4 == 0,
                        }));
                        f.at = start + len;
                    }
                    _ => {}
                }
            }
            if !par.items.is_empty() {
                out.push(par);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_3_0_contents_is_topic_0() {
        // Shadowgate's SHADOW.HLP: an empty topic first, and |TOMAP's
        // topic 0 (its "Index") after it.
        let page = |offset| Page {
            offset,
            ..Page::default()
        };
        let mut f = HelpFile {
            version: 15,
            pages: vec![page(0xC), page(0x51), page(0x24D)],
            tomap: vec![0x51, 0x24D],
            ..HelpFile::default()
        };
        assert_eq!(f.contents(), 1);
        assert_eq!(f.page_by_hash(0), Some(1));
        // The contents |SYSTEM names goes first.
        f.contents = Some(0x24D);
        assert_eq!(f.contents(), 2);
        // A Windows 3.1 file without one starts at its first topic.
        f.contents = None;
        f.version = 21;
        assert_eq!(f.contents(), 0);
    }

    #[test]
    fn hashes_are_winhelps() {
        // Wine's HLPFILE_Hash: letters 17 on, digits, '.' and '_'.
        assert_eq!(hash(""), 0);
        assert_eq!(hash("a"), 17);
        assert_eq!(hash("A1"), 17 * 43 + 1);
        assert_eq!(hash("a b"), 17 * 43 + 18);
    }

    #[test]
    fn lz77_copies_back_references() {
        // "ab", then three bytes from two back: "aba", then "c".
        let src = [0b0000_0100, b'a', b'b', 0x01, 0x00, b'c'];
        assert_eq!(lz77(&src), b"ababac");
    }

    #[test]
    fn phrases_expand_as_windows_3_0_does() {
        let p = Phrases {
            offsets: vec![0, 5, 10],
            text: b"Hellothere".to_vec(),
            hall: false,
        };
        // Phrase 1 with a space after, then "!".
        let src = [0x01, 0x03, b'!'];
        assert_eq!(p.expand(&src, 7), b"there !");
    }
}
