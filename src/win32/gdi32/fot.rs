//! TrueType resource files (.FOT), which CreateScalableFontResourceA
//! writes and AddFontResourceA reads: a NE module with no code whose
//! resources are a font directory (type 8007h) and the TrueType file's
//! name (type 80CCh), the way Windows 3.1 to 98 installed TrueType
//! fonts. `target` reads the name from any such file, Windows 98's too.
//!
//! The files `build` writes have Wine's layout: a DOS stub saying "This
//! is a TrueType resource file", the NE header, the resource table, the
//! resident names (the module's name), the non-resident names
//! ("FONTRES 100,96,72 : " and the face) and the two resources, the
//! font directory with only the fields GDI reads. Windows 98 SE's are
//! 1318 bytes whatever the font, with the stub "This is a TrueType font,
//! not a program.", "FONTDIR" and the module's name as resident names,
//! "FONTRES:" and the face as the non-resident one, and a font directory
//! with the font's metrics. Only GDI reads these files, so the
//! difference shows to no program.

/// The resource types of the font directory and of the file's name.
const RT_FONTDIR: u16 = 0x8007;
const RT_SCALABLE_NAME: u16 = 0x80CC;

/// Where the NE header is: after the DOS header and its stub's text.
const NE: usize = 0x80;

/// The resources' alignment: 16-byte units.
const SHIFT: u16 = 4;

/// FONTDIR's dfType for a TrueType font; bit 7 hides it from
/// enumerations (CreateScalableFontResourceA's `hidden`).
const TYPE_TRUETYPE: u16 = 0x0003;
const TYPE_HIDDEN: u16 = 0x0080;

fn put16(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

fn get16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

/// Pad `b` to the resources' alignment.
fn align(b: &mut Vec<u8>) {
    b.resize(b.len().next_multiple_of(1 << SHIFT), 0);
}

/// The font directory of one font named `face`: the count of fonts, the
/// font's resource number and the FNT header fields of a TrueType font,
/// with the face's name after them.
fn font_dir(face: &str, hidden: bool) -> Vec<u8> {
    let mut d = vec![0u8; 4 + 113];
    put16(&mut d, 0, 1);
    put16(&mut d, 2, 1);
    let fnt = 4;
    put16(&mut d, fnt, 0x0200);
    let ty = TYPE_TRUETYPE | if hidden { TYPE_HIDDEN } else { 0 };
    put16(&mut d, fnt + 66, ty);
    // dfFace: where the face's name is, from the FNT header.
    d[fnt + 105..fnt + 109].copy_from_slice(&113u32.to_le_bytes());
    d.extend_from_slice(face.as_bytes());
    d.push(0);
    d
}

/// The .FOT file for the TrueType file `ttf_name` whose family is
/// `face`.
pub fn build(module: &str, face: &str, ttf_name: &str, hidden: bool) -> Vec<u8> {
    let mut b = vec![0u8; NE];
    b[..2].copy_from_slice(b"MZ");
    put16(&mut b, 0x18, 0x40);
    b[0x3C..0x40].copy_from_slice(&(NE as u32).to_le_bytes());
    let stub = b"This is a TrueType resource file";
    b[0x40..0x40 + stub.len()].copy_from_slice(stub);

    let mut ne = vec![0u8; 0x40];
    ne[..2].copy_from_slice(b"NE");
    ne[2] = 5;
    ne[3] = 10;
    // NE_FFLAGS_LIBMODULE, single data; Windows; Windows 3.0 or later.
    put16(&mut ne, 0x0C, 0x8000);
    put16(&mut ne, 0x32, SHIFT);
    ne[0x36] = 2;
    put16(&mut ne, 0x3E, 0x0300);

    // The resource table: its alignment, the two types, the end.
    let rsrc = 0x40;
    let mut table = Vec::new();
    table.extend_from_slice(&SHIFT.to_le_bytes());
    let mut slots = Vec::new();
    for ty in [RT_FONTDIR, RT_SCALABLE_NAME] {
        table.extend_from_slice(&ty.to_le_bytes());
        table.extend_from_slice(&1u16.to_le_bytes());
        table.extend_from_slice(&[0; 4]);
        slots.push(table.len());
        // Offset, length, flags (moveable, pure, preload), id 1.
        table.extend_from_slice(&[0, 0, 0, 0, 0x70, 0x0C, 0x01, 0x80, 0, 0, 0, 0]);
    }
    table.extend_from_slice(&[0, 0]);
    let restab = rsrc + table.len();
    let mut names = Vec::new();
    let module = module.as_bytes();
    names.push(module.len().min(255) as u8);
    names.extend_from_slice(&module[..module.len().min(255)]);
    names.extend_from_slice(&[0, 0, 0]);
    let after = restab + names.len();
    put16(&mut ne, 0x04, after as u16);
    put16(&mut ne, 0x24, rsrc as u16);
    put16(&mut ne, 0x26, restab as u16);
    put16(&mut ne, 0x28, after as u16);
    put16(&mut ne, 0x2A, after as u16);
    put16(&mut ne, 0x34, 2);

    let mut nonres = Vec::new();
    let text = format!("FONTRES 100,96,72 : {face}");
    let text = &text.as_bytes()[..text.len().min(255)];
    nonres.push(text.len() as u8);
    nonres.extend_from_slice(text);
    nonres.extend_from_slice(&[0, 0, 0]);
    let nrestab = NE + after;
    ne[0x2C..0x30].copy_from_slice(&(nrestab as u32).to_le_bytes());
    put16(&mut ne, 0x20, nonres.len() as u16);

    b.extend_from_slice(&ne);
    b.extend_from_slice(&table);
    b.extend_from_slice(&names);
    b.extend_from_slice(&nonres);
    let mut name = ttf_name.as_bytes().to_vec();
    name.push(0);
    for (slot, data) in slots.into_iter().zip([font_dir(face, hidden), name]) {
        align(&mut b);
        let at = NE + rsrc + slot;
        let unit = (b.len() >> SHIFT) as u16;
        put16(&mut b, at, unit);
        let len = data.len().next_multiple_of(1 << SHIFT);
        put16(&mut b, at + 2, (len >> SHIFT) as u16);
        b.extend_from_slice(&data);
    }
    align(&mut b);
    b
}

/// The .FOT file that CreateScalableFontResourceA writes for the
/// TrueType file `ttf` named `ttf_name`: its module name is the
/// TrueType file's base name in capitals, as Windows 98 SE has it, its
/// face the family of the file's first font. None if `ttf` isn't a
/// TrueType font GDI can draw.
pub fn for_truetype(ttf_name: &str, ttf: &[u8], hidden: bool) -> Option<Vec<u8>> {
    let face = super::ttf::Ttf::fonts_of_file(ttf).into_iter().next()?.face;
    let base = ttf_name.rsplit(['\\', '/', ':']).next().unwrap_or("");
    let module = base.split('.').next().unwrap_or("").to_ascii_uppercase();
    Some(build(&module, &face, ttf_name, hidden))
}

/// The TrueType file a .FOT names, None for a file that isn't one.
pub fn target(file: &[u8]) -> Option<String> {
    if file.get(..2)? != b"MZ" {
        return None;
    }
    let ne = u32::from_le_bytes(file.get(0x3C..0x40)?.try_into().ok()?) as usize;
    if file.get(ne..ne + 2)? != b"NE" {
        return None;
    }
    let table = ne + get16(file, ne + 0x24)? as usize;
    let shift = get16(file, table)?;
    if shift > 16 {
        return None;
    }
    let mut at = table + 2;
    loop {
        let ty = get16(file, at)?;
        if ty == 0 {
            return None;
        }
        let count = get16(file, at + 2)? as usize;
        at += 8;
        if ty == RT_SCALABLE_NAME && count > 0 {
            let offset = (get16(file, at)? as usize) << shift;
            let len = (get16(file, at + 2)? as usize) << shift;
            let bytes = file.get(offset..(offset + len).min(file.len()))?;
            let end = bytes.iter().position(|&c| c == 0).unwrap_or(bytes.len());
            return Some(bytes[..end].iter().map(|&c| c as char).collect());
        }
        at += 12 * count;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fot_names_its_truetype_file() {
        let fot = build("QTFONT", "Geneva", "C:\\WINDOWS\\FONTS\\QT.TTF", false);
        assert_eq!(&fot[0x40..0x60], b"This is a TrueType resource file");
        assert_eq!(target(&fot).as_deref(), Some("C:\\WINDOWS\\FONTS\\QT.TTF"));
        // It has no RT_FONT, so no raster font is read from it.
        assert!(super::super::fnt::fonts_of_file(&fot).is_empty());
    }

    #[test]
    fn a_fot_for_a_truetype_file_has_its_family() {
        let ttf = super::super::ttf::tests::ttf("Quick Face", false);
        let fot = for_truetype("C:\\QTFONT.TTF", &ttf, true).unwrap();
        assert_eq!(target(&fot).as_deref(), Some("C:\\QTFONT.TTF"));
        // The module's name, then the face in the font directory, which
        // is marked hidden: its last copy, after the non-resident names'.
        let at = |s: &[u8]| fot.windows(s.len()).rposition(|w| w == s);
        assert!(at(b"\x06QTFONT").is_some());
        let face = at(b"Quick Face\0").unwrap();
        let dir = face - 4 - 113;
        assert_eq!(u16::from_le_bytes([fot[dir + 70], fot[dir + 71]]), 0x83);
        // A file that isn't TrueType gets none.
        assert!(for_truetype("A.TTF", b"not a font", false).is_none());
    }

    /// A .FOT that Windows 98 SE's CreateScalableFontResourceA wrote for
    /// "QX.TTF" in a folder given as `path`: its DOS header's pointer to
    /// the NE header, which is at 80h, the NE header and its resource
    /// table, and the name it keeps as it was given, at 400h.
    #[test]
    fn a_windows_98_fot_names_its_file() {
        let ne: [u8; 0x70] = [
            0x4E, 0x45, 0x05, 0x10, 0x84, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x17, 0x00, 0x40, 0x00, 0x40, 0x00, 0x74, 0x00, 0x7C, 0x00,
            0x7C, 0x00, 0x08, 0x01, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x02, 0x00, 0x02, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x04, 0x00, 0x07, 0x80, 0x01, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x48, 0x00, 0x0A, 0x00, 0x50, 0x0C, 0x2C, 0x00, 0x00, 0x00,
            0x00, 0x00, 0xCC, 0x80, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x08, 0x00,
            0x50, 0x0C, 0x01, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x07, 0x46, 0x4F, 0x4E,
        ];
        let mut fot = vec![0u8; 1318];
        fot[..2].copy_from_slice(b"MZ");
        fot[0x3C] = 0x80;
        fot[0x80..0xF0].copy_from_slice(&ne);
        fot[0x400..0x406].copy_from_slice(b"QX.TTF");
        assert_eq!(target(&fot).as_deref(), Some("QX.TTF"));
    }

    #[test]
    fn other_files_name_none() {
        assert_eq!(target(b"MZ"), None);
        assert_eq!(target(&[0u8; 0x100]), None);
    }
}
