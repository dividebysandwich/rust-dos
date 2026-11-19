//! Raster fonts from files, as AddFontResourceA adds them: the FNT
//! resources (RT_FONT) of a .FON file, which is a 16-bit NE module, or a
//! bare .FNT file. Versions 2.0 and 3.0 of the format are read; vector
//! fonts and version 1.0's raster fonts aren't, as Windows 95 draws no
//! program's fonts of them either.
//!
//! An FNT keeps each glyph by columns of 8 pixels: the first column's
//! bytes for every row, then the next column's, the leftmost pixel of a
//! byte in bit 7.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use super::font::Glyphs;
use super::ttf::Ttf;

/// RT_FONT in a NE resource table, with the bit of a numbered type.
const RT_FONT: u16 = 0x8008;

/// A glyph of a file's font: how many pixels wide it is and its rows, the
/// leftmost pixel in bit 0.
pub struct FileGlyph {
    pub width: i32,
    pub rows: Vec<u64>,
}

/// A raster font of a file: the FNT resource's bytes, which a save state
/// keeps, and what GDI reads of them.
pub struct Fnt {
    pub data: Vec<u8>,
    pub face: String,
    pub points: i32,
    pub height: i32,
    pub ascent: i32,
    pub internal: i32,
    pub external: i32,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub weight: i32,
    pub charset: u8,
    /// dfPitchAndFamily: the family in the high bits, and bit 0 set for a
    /// font of variable pitch (as TEXTMETRIC's tmPitchAndFamily has it).
    pub pitch_family: u8,
    pub average: i32,
    pub first: u8,
    pub last: u8,
    /// The character drawn for a byte past `first` to `last`, as a byte.
    pub default: u8,
    pub break_char: u8,
    /// The glyphs of `first` to `last`.
    pub glyphs: Vec<FileGlyph>,
    /// The glyphs the font has been drawn with, made from `glyphs` once
    /// for each width and height it is stretched to and whether it is
    /// made bold.
    pub(super) drawn: RefCell<HashMap<(i32, i32, bool), Rc<Glyphs>>>,
}

impl std::fmt::Debug for Fnt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Fnt({:?}, {} pixels)", self.face, self.height)
    }
}

fn u16_at(d: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(at..at + 4)?.try_into().ok()?))
}

impl Fnt {
    /// The raster font of the FNT resource `data`, or None for a vector
    /// font, another version or bytes too short for what they say.
    pub fn parse(data: Vec<u8>) -> Option<Fnt> {
        let d = &data;
        let version = u16_at(d, 0)?;
        if version != 0x200 && version != 0x300 {
            return None;
        }
        // dfType: bit 0 is a vector font.
        if u16_at(d, 0x42)? & 1 != 0 {
            return None;
        }
        let height = u16_at(d, 0x58)? as i32;
        let (first, last) = (*d.get(0x5F)?, *d.get(0x60)?);
        // A glyph's column holds a byte a row, so a font higher than its
        // file is malformed: that also bounds the rows made for its
        // glyphs, each `height` long, by the file's size.
        if height == 0 || height as usize > d.len() || last < first {
            return None;
        }
        let face_at = u32_at(d, 0x69)? as usize;
        let face = d.get(face_at..)?;
        let end = face.iter().position(|&c| c == 0).unwrap_or(face.len());
        let face = String::from_utf8_lossy(&face[..end.min(31)]).into_owned();
        let (table, entry) = if version == 0x300 {
            (0x94, 6)
        } else {
            (0x76, 4)
        };
        let mut glyphs = Vec::with_capacity((last - first) as usize + 1);
        for i in 0..=(last - first) as usize {
            let at = table + i * entry;
            let width = u16_at(d, at)? as i32;
            let bits = if version == 0x300 {
                u32_at(d, at + 2)? as usize
            } else {
                u16_at(d, at + 2)? as usize
            };
            // A row holds 64 pixels; wider glyphs keep their width but
            // are drawn no wider.
            let drawn = width.min(64) as usize;
            let mut rows = vec![0u64; height as usize];
            for (y, row) in rows.iter_mut().enumerate() {
                for col in 0..drawn.div_ceil(8) {
                    let b = *d.get(bits + col * height as usize + y)?;
                    for k in 0..8 {
                        let x = col * 8 + k;
                        if x < drawn && b & (0x80 >> k) != 0 {
                            *row |= 1 << x;
                        }
                    }
                }
            }
            glyphs.push(FileGlyph { width, rows });
        }
        let default = first.saturating_add(*d.get(0x61)?);
        Some(Fnt {
            face,
            points: u16_at(d, 0x44)? as i32,
            height,
            ascent: u16_at(d, 0x4A)? as i32,
            internal: u16_at(d, 0x4C)? as i32,
            external: u16_at(d, 0x4E)? as i32,
            italic: *d.get(0x50)? != 0,
            underline: *d.get(0x51)? != 0,
            strike: *d.get(0x52)? != 0,
            weight: u16_at(d, 0x53)? as i32,
            charset: *d.get(0x55)?,
            pitch_family: *d.get(0x5A)?,
            average: u16_at(d, 0x5B)? as i32,
            first,
            last,
            default: if default > last { first } else { default },
            break_char: first.saturating_add(*d.get(0x62)?),
            glyphs,
            drawn: RefCell::new(HashMap::new()),
            data,
        })
    }

    /// The glyph of the byte `b`: its own, or the default character's.
    pub fn glyph(&self, b: u8) -> &FileGlyph {
        let b = if (self.first..=self.last).contains(&b) {
            b
        } else {
            self.default
        };
        &self.glyphs[(b - self.first) as usize]
    }
}

/// The W32 section version whose states keep the fonts of the files
/// added. An older state has the files' names only: their fonts load as
/// none, so a program that added them draws in GDI's own faces after the
/// state is loaded, as it did when it was saved.
pub const FONTS_SAVED: u16 = 72;

/// The W32 section version whose states keep the TrueType fonts of the
/// files added too. An older state loads them as none, and the program
/// draws in GDI's own faces after it is loaded, as it did when it was
/// saved.
pub const TRUETYPE_SAVED: u16 = 81;

/// A file AddFontResourceA added: its name as the program gave it, and
/// its raster fonts or its TrueType fonts.
#[derive(Debug, Default)]
pub struct AddedFile {
    pub name: String,
    pub fonts: Vec<Rc<Fnt>>,
    pub truetype: Vec<Rc<Ttf>>,
}

impl crate::savestate::State for AddedFile {
    fn save(&self, w: &mut crate::savestate::Writer) {
        self.name.save(w);
        self.fonts.len().save(w);
        for f in &self.fonts {
            f.data.save(w);
        }
        // The file's bytes once, and the index of each of its fonts.
        let data = self
            .truetype
            .first()
            .map(|t| t.data.clone())
            .unwrap_or_default();
        data.save(w);
        self.truetype.len().save(w);
        for t in &self.truetype {
            t.index.save(w);
        }
    }

    fn load(&mut self, r: &mut crate::savestate::Reader) -> crate::savestate::Result<()> {
        self.name.load(r)?;
        self.fonts.clear();
        // Bytes outside a section are in this rust-dos's own layout.
        if r.version().is_some_and(|v| v < FONTS_SAVED) {
            return Ok(());
        }
        for _ in 0..r.count()? {
            let mut data = Vec::new();
            data.load(r)?;
            let f = Fnt::parse(data).ok_or_else(|| {
                crate::savestate::StateError::Invalid("an added font that isn't one".into())
            })?;
            self.fonts.push(Rc::new(f));
        }
        self.truetype.clear();
        if r.version().is_some_and(|v| v < TRUETYPE_SAVED) {
            return Ok(());
        }
        let mut data: Vec<u8> = Vec::new();
        data.load(r)?;
        for _ in 0..r.count()? {
            let mut index = 0u32;
            index.load(r)?;
            let t = Ttf::parse(data.clone(), index).ok_or_else(|| {
                crate::savestate::StateError::Invalid(
                    "an added TrueType font that isn't one".into(),
                )
            })?;
            self.truetype.push(Rc::new(t));
        }
        Ok(())
    }
}

/// The raster fonts of a file's bytes: each RT_FONT resource of a NE
/// module, or the file itself if it is an FNT.
pub fn fonts_of_file(file: &[u8]) -> Vec<Fnt> {
    if file.starts_with(b"MZ") {
        return ne_fonts(file).unwrap_or_default();
    }
    Fnt::parse(file.to_vec()).into_iter().collect()
}

/// The RT_FONT resources of a NE module.
fn ne_fonts(file: &[u8]) -> Option<Vec<Fnt>> {
    let ne = u32_at(file, 0x3C)? as usize;
    if file.get(ne..ne + 2)? != b"NE" {
        return None;
    }
    let table = ne + u16_at(file, ne + 0x24)? as usize;
    let shift = u16_at(file, table)?;
    if shift > 16 {
        return None;
    }
    let mut at = table + 2;
    let mut fonts = Vec::new();
    loop {
        let kind = u16_at(file, at)?;
        if kind == 0 {
            break;
        }
        let count = u16_at(file, at + 2)? as usize;
        at += 8;
        for _ in 0..count {
            if kind == RT_FONT {
                let offset = (u16_at(file, at)? as usize) << shift;
                let len = (u16_at(file, at + 2)? as usize) << shift;
                let bytes = file.get(offset..(offset + len).min(file.len()))?;
                fonts.extend(Fnt::parse(bytes.to_vec()));
            }
            at += 12;
        }
    }
    Some(fonts)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// A version 3.0 FNT of the face `face`, `height` pixels high, with
    /// the characters 20h to 22h: a blank 3 wide, a 2 wide bar and a 9
    /// wide glyph lit in its first and last columns.
    pub(in super::super) fn fnt(face: &str, height: u16) -> Vec<u8> {
        let mut d = vec![0u8; 0x94];
        d[0..2].copy_from_slice(&0x300u16.to_le_bytes());
        d[0x44..0x46].copy_from_slice(&10u16.to_le_bytes());
        d[0x4A..0x4C].copy_from_slice(&(height - 2).to_le_bytes());
        d[0x4C..0x4E].copy_from_slice(&1u16.to_le_bytes());
        d[0x53..0x55].copy_from_slice(&700u16.to_le_bytes());
        d[0x58..0x5A].copy_from_slice(&height.to_le_bytes());
        d[0x5A] = 0x21; // FF_SWISS, variable pitch
        d[0x5B..0x5D].copy_from_slice(&4u16.to_le_bytes());
        d[0x5F] = 0x20;
        d[0x60] = 0x22;
        d[0x61] = 1; // the default character is 21h
        let widths = [3u16, 2, 9, 0];
        let columns = [1usize, 1, 2, 1];
        let table = d.len();
        d.resize(table + 6 * widths.len(), 0);
        let mut bits = Vec::new();
        let start = table + 6 * widths.len();
        for (i, (&w, &c)) in widths.iter().zip(&columns).enumerate() {
            let at = table + i * 6;
            d[at..at + 2].copy_from_slice(&w.to_le_bytes());
            d[at + 2..at + 6].copy_from_slice(&((start + bits.len()) as u32).to_le_bytes());
            let mut glyph = vec![0u8; c * height as usize];
            match i {
                1 => glyph.fill(0xC0),
                2 => {
                    glyph[..height as usize].fill(0x80);
                    glyph[height as usize..].fill(0x80);
                }
                _ => {}
            }
            bits.extend(glyph);
        }
        d.extend(bits);
        let face_at = d.len() as u32;
        d[0x69..0x6D].copy_from_slice(&face_at.to_le_bytes());
        d.extend(face.as_bytes());
        d.push(0);
        let size = d.len() as u32;
        d[2..6].copy_from_slice(&size.to_le_bytes());
        d
    }

    /// A .FON file: a NE module whose resources are the FNTs `fonts`.
    fn fon(fonts: &[Vec<u8>]) -> Vec<u8> {
        let ne = 0x40usize;
        let mut d = vec![0u8; ne + 0x40];
        d[0..2].copy_from_slice(b"MZ");
        d[0x3C..0x40].copy_from_slice(&(ne as u32).to_le_bytes());
        d[ne..ne + 2].copy_from_slice(b"NE");
        let table = d.len();
        d[ne + 0x24..ne + 0x26].copy_from_slice(&((table - ne) as u16).to_le_bytes());
        // Alignment shift 4: offsets and lengths in 16-byte units.
        let mut t = 4u16.to_le_bytes().to_vec();
        t.extend(RT_FONT.to_le_bytes());
        t.extend((fonts.len() as u16).to_le_bytes());
        t.extend([0; 4]);
        let entries = t.len();
        t.resize(entries + 12 * fonts.len() + 2, 0);
        let mut at = (table + t.len()).next_multiple_of(16);
        let mut body = Vec::new();
        for (i, f) in fonts.iter().enumerate() {
            let e = entries + i * 12;
            t[e..e + 2].copy_from_slice(&((at >> 4) as u16).to_le_bytes());
            t[e + 2..e + 4].copy_from_slice(&(f.len().div_ceil(16) as u16).to_le_bytes());
            let mut padded = f.clone();
            padded.resize(f.len().next_multiple_of(16), 0);
            body.push((at, padded));
            at += f.len().next_multiple_of(16);
        }
        d.extend(t);
        for (at, bytes) in body {
            d.resize(at, 0);
            d.extend(bytes);
        }
        d
    }

    #[test]
    fn a_fnt_reads_its_metrics_and_glyphs() {
        let f = Fnt::parse(fnt("Test Face", 12)).unwrap();
        assert_eq!(f.face, "Test Face");
        assert_eq!((f.height, f.ascent, f.internal), (12, 10, 1));
        assert_eq!((f.first, f.last, f.default), (0x20, 0x22, 0x21));
        assert_eq!(f.glyph(b' ').width, 3);
        assert!(f.glyph(b' ').rows.iter().all(|&r| r == 0));
        assert!(f.glyph(b'!').rows.iter().all(|&r| r == 0b11));
        // The 9 wide glyph's second column of bytes holds its last pixel.
        assert!(f.glyph(b'"').rows.iter().all(|&r| r == 1 | 1 << 8));
        // A byte the font hasn't is the default character.
        assert_eq!(f.glyph(b'A').width, 2);
    }

    #[test]
    fn a_fon_file_has_the_fonts_of_its_resources() {
        let file = fon(&[fnt("Small", 8), fnt("Large", 16)]);
        let fonts = fonts_of_file(&file);
        let got: Vec<_> = fonts.iter().map(|f| (f.face.as_str(), f.height)).collect();
        assert_eq!(got, [("Small", 8), ("Large", 16)]);
        // A bare FNT is a font too, and other bytes none.
        assert_eq!(fonts_of_file(&fnt("Bare", 10)).len(), 1);
        assert!(fonts_of_file(b"\0\x01TrueType").is_empty());
    }

    #[test]
    fn a_state_keeps_the_added_fonts_and_an_older_one_their_files_names() {
        use crate::savestate::{Reader, State, Writer};
        let font = Fnt::parse(fnt("Kept", 10)).unwrap();
        let ttf = super::super::ttf::tests::ttf("Kept TT", false);
        let files = vec![
            AddedFile {
                name: "F.FON".into(),
                fonts: vec![Rc::new(font)],
                truetype: Vec::new(),
            },
            AddedFile {
                name: "F.TTF".into(),
                fonts: Vec::new(),
                truetype: Ttf::fonts_of_file(&ttf).into_iter().map(Rc::new).collect(),
            },
        ];
        let mut w = Writer::new();
        w.section(b"TEST", TRUETYPE_SAVED, |w| files.save(w));
        let mut back: Vec<AddedFile> = Vec::new();
        let mut r = Reader::new(&w.buf);
        back.load(&mut r.section(b"TEST", TRUETYPE_SAVED).unwrap())
            .unwrap();
        assert_eq!(back[0].name, "F.FON");
        assert_eq!(back[0].fonts[0].face, "Kept");
        assert_eq!(back[0].fonts[0].data, files[0].fonts[0].data);
        assert!(back[0].truetype.is_empty());
        assert_eq!(back[1].truetype[0].face, "Kept TT");
        assert_eq!(back[1].truetype[0].data, ttf);
        // Version 72 to 80 saved the raster fonts and no TrueType ones.
        let mut w = Writer::new();
        w.section(b"TEST", FONTS_SAVED, |w| {
            "OLD.TTF".to_string().save(w);
            0usize.save(w);
        });
        let mut r = Reader::new(&w.buf);
        let mut section = r
            .section_of(b"TEST", &[FONTS_SAVED, TRUETYPE_SAVED])
            .unwrap();
        let mut old = AddedFile::default();
        old.load(&mut section).unwrap();
        assert!(section.is_empty());
        assert_eq!(old.name, "OLD.TTF");
        assert!(old.truetype.is_empty());
        // Version 71 saved the names alone.
        let mut w = Writer::new();
        w.section(b"TEST", 71, |w| vec!["OLD.FON".to_string()].save(w));
        let mut r = Reader::new(&w.buf);
        let mut section = r.section_of(b"TEST", &[71, FONTS_SAVED]).unwrap();
        let mut old: Vec<AddedFile> = Vec::new();
        old.load(&mut section).unwrap();
        assert!(section.is_empty());
        assert_eq!(old[0].name, "OLD.FON");
        assert!(old[0].fonts.is_empty());
        // Bytes read outside a section are this build's layout, fonts
        // and all.
        let mut w = Writer::new();
        files.save(&mut w);
        let mut r = Reader::new(&w.buf);
        let mut bare: Vec<AddedFile> = Vec::new();
        bare.load(&mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!(bare[0].fonts[0].face, "Kept");
    }

    /// A font higher than its file can't hold its glyphs: it is refused
    /// before rows of that height are made for each of its characters.
    #[test]
    fn a_font_higher_than_its_file_is_refused() {
        let mut d = fnt("Tall", 12);
        assert!(Fnt::parse(d.clone()).is_some());
        d[0x58..0x5A].copy_from_slice(&60000u16.to_le_bytes());
        assert!(Fnt::parse(d).is_none());
    }
}
