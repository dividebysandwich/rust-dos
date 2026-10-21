//! 3DF files, 3dfx's texture files, which gu3dfGetInfo and gu3dfLoad
//! read. A file starts with four lines of text: "3df v1.0", the texel
//! format's name, "lod range: <smallest> <largest>" (the sizes of the
//! mipmap's smallest and largest levels) and "aspect ratio: <w> <h>".
//! After them come the format's table, if it has one, and the levels
//! from the largest to the smallest. Numbers in the file are big-endian:
//! the NCC table of the YIQ formats as 16-bit values, the palette of the
//! P8 formats as 32-bit ones, and the texels of the 16-bit formats.
//!
//! Gu3dfInfo, which the program passes, is the header (width, height,
//! small and large LOD, aspect ratio and format, a word each), the table
//! (a GuNccTable or a GuTexPalette, 1024 bytes), the address of the
//! memory for the texels, which the program allocates, and the bytes
//! they take.

use super::api::{self, TEXFMT_16BIT, TEXFMT_AYIQ_8422, TEXFMT_P_8, TEXFMT_YIQ_422};
use crate::cpu::Cpu;
use crate::win32::kernel32;

const TRUE: u32 = 1;
const FALSE: u32 = 0;

/// Where Gu3dfInfo's fields are.
const INFO_HEADER_SIZE: u32 = 24;
const INFO_TABLE: u32 = INFO_HEADER_SIZE;
const INFO_DATA: u32 = INFO_TABLE + 1024;
const INFO_MEM_REQUIRED: u32 = INFO_DATA + 4;

/// GR_TEXFMT_AP_88, the last format.
const TEXFMT_AP_88: u32 = 0xE;

/// The formats by their names in a file.
const FORMATS: &[(&str, u32)] = &[
    ("RGB332", 0),
    ("YIQ", TEXFMT_YIQ_422),
    ("A8", 2),
    ("I8", 3),
    ("AI44", 4),
    ("P8", TEXFMT_P_8),
    ("ARGB8332", TEXFMT_16BIT),
    ("AYIQ8422", TEXFMT_AYIQ_8422),
    ("RGB565", 0xA),
    ("ARGB1555", 0xB),
    ("ARGB4444", 0xC),
    ("AI88", 0xD),
    ("AP88", TEXFMT_AP_88),
];

/// What a file's header says, as Gu3dfHeader has it.
#[derive(Debug, PartialEq)]
struct Header {
    width: u32,
    height: u32,
    small_lod: u32,
    large_lod: u32,
    aspect: u32,
    format: u32,
}

/// Where the data after the header's four lines starts.
fn data_start(file: &[u8]) -> Option<usize> {
    file.iter()
        .enumerate()
        .filter(|&(_, &b)| b == b'\n')
        .nth(3)
        .map(|(i, _)| i + 1)
}

/// The LOD of a level `size` texels on its longer side.
fn lod_of(size: u32) -> Option<u32> {
    (size.is_power_of_two() && size <= 256).then(|| api::LOD_1 - size.trailing_zeros())
}

/// GR_ASPECT_* of a texture `w` by `h`: 1x1 is 3, wider ones below it
/// and taller ones above.
fn aspect_of(w: u32, h: u32) -> Option<u32> {
    (0..4).find_map(|i| {
        if w != 0 && w << i == h {
            Some(api::ASPECT_1X1 + i)
        } else if h != 0 && h << i == w {
            Some(api::ASPECT_1X1 - i)
        } else {
            None
        }
    })
}

fn parse_header(file: &[u8]) -> Option<Header> {
    let text = String::from_utf8_lossy(&file[..data_start(file)?]);
    let words: Vec<&str> = text.split_whitespace().collect();
    let [magic, _version, format, _, _, small, large, _, _, w, h] = words[..] else {
        return None;
    };
    if !magic.eq_ignore_ascii_case("3df") {
        return None;
    }
    let number = |s: &str| s.parse::<u32>().ok();
    let (small, large) = (number(small)?, number(large)?);
    let (w, h) = (number(w)?, number(h)?);
    let aspect = aspect_of(w, h)?;
    let (width, height) = if w >= h {
        (large, large / w)
    } else {
        (large / h, large)
    };
    let format = FORMATS
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(format))?
        .1;
    Some(Header {
        width,
        height,
        small_lod: lod_of(small)?,
        large_lod: lod_of(large)?,
        aspect,
        format,
    })
}

/// The bytes a mipmap of `format` takes from `large_lod` to `small_lod`
/// in the program's memory.
fn mipmap_bytes(aspect: u32, large_lod: u32, small_lod: u32, format: u32) -> u32 {
    let texel = if format >= TEXFMT_16BIT { 2 } else { 1 };
    (large_lod..=small_lod)
        .map(|lod| {
            let (w, h) = api::level_size(aspect, lod);
            w * h * texel
        })
        .sum()
}

/// The file named by the string at `name`. Some(None) if it can't be
/// read; None if a page of the name isn't there.
fn read_file(cpu: &mut Cpu, name: u32) -> Option<Option<Vec<u8>>> {
    let Some((at, size)) = kernel32::read_whole_file(cpu, name)? else {
        return Some(None);
    };
    let mut bytes = vec![0; size as usize];
    cpu.bus.guest_read_bytes(at, &mut bytes);
    kernel32::free_pages(cpu, at);
    Some(Some(bytes))
}

/// gu3dfGetInfo(filename, info): the file's header and the bytes its
/// texels take, in `info`. FALSE if the file can't be read or its header
/// isn't a 3DF file's.
pub fn get_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [name, info] = args[..] else {
        return Some(FALSE);
    };
    if !cpu
        .bus
        .guest_probe(info, INFO_MEM_REQUIRED as usize + 4, true)
    {
        return None;
    }
    let Some(file) = read_file(cpu, name)? else {
        return Some(FALSE);
    };
    let Some(h) = parse_header(&file) else {
        return Some(FALSE);
    };
    let words = [
        h.width,
        h.height,
        h.small_lod,
        h.large_lod,
        h.aspect,
        h.format,
    ];
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    cpu.bus.guest_write_bytes(info, &bytes);
    let size = mipmap_bytes(h.aspect, h.large_lod, h.small_lod, h.format);
    cpu.bus.guest_write_32(info + INFO_MEM_REQUIRED, size);
    Some(TRUE)
}

/// The NCC table at `at` in the file, as GuNccTable has it: Y, I and Q
/// as the file has them (8 and 9 bits of each value), and the three
/// packed as the card's NCC table registers take them.
fn ncc_table(file: &[u8], at: usize) -> Vec<u8> {
    let value = |i: usize| {
        file.get(at + 2 * i..at + 2 * i + 2)
            .map_or(0, |b| u16::from_be_bytes([b[0], b[1]]))
    };
    let y: Vec<u32> = (0..16).map(|i| u32::from(value(i) & 0xFF)).collect();
    let iq: Vec<u32> = (16..40).map(|i| u32::from(value(i) & 0x1FF)).collect();
    let mut out = Vec::with_capacity(112);
    out.extend(y.iter().map(|&v| v as u8));
    out.extend(iq.iter().flat_map(|&v| (v as u16).to_le_bytes()));
    let mut packed = Vec::with_capacity(12);
    packed.extend(
        y.chunks(4)
            .map(|c| c[0] | c[1] << 8 | c[2] << 16 | c[3] << 24),
    );
    packed.extend(iq.chunks(3).map(|c| c[0] << 18 | c[1] << 9 | c[2]));
    out.extend(packed.iter().flat_map(|v| v.to_le_bytes()));
    out
}

/// gu3dfLoad(filename, info): the file's table into `info` and its
/// texels into the memory at info's data address, by the header
/// gu3dfGetInfo put in `info`. What the file lacks reads as zeros.
pub fn load(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [name, info] = args[..] else {
        return Some(FALSE);
    };
    let mut head = [0u8; INFO_DATA as usize + 4];
    cpu.bus.guest_read_bytes(info, &mut head);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let word = |i: usize| u32::from_le_bytes([head[i], head[i + 1], head[i + 2], head[i + 3]]);
    let (small_lod, large_lod, aspect, format) = (word(8), word(12), word(16), word(20));
    let data = word(INFO_DATA as usize);
    let known = format <= TEXFMT_P_8 || (TEXFMT_16BIT..=TEXFMT_AP_88).contains(&format);
    if !known || large_lod > small_lod || small_lod > api::LOD_1 || aspect > api::ASPECT_1X8 {
        return Some(FALSE);
    }
    let Some(file) = read_file(cpu, name)? else {
        return Some(FALSE);
    };
    let Some(mut at) = data_start(&file) else {
        return Some(FALSE);
    };
    let table = if format == TEXFMT_YIQ_422 || format == TEXFMT_AYIQ_8422 {
        let table = ncc_table(&file, at);
        at += 2 * 40;
        Some(table)
    } else if format == TEXFMT_P_8 || format == TEXFMT_AP_88 {
        let table: Vec<u8> = (0..256)
            .flat_map(|i| {
                let v = file
                    .get(at + 4 * i..at + 4 * i + 4)
                    .map_or(0, |b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
                v.to_le_bytes()
            })
            .collect();
        at += 4 * 256;
        Some(table)
    } else {
        None
    };
    let size = mipmap_bytes(aspect, large_lod, small_lod, format) as usize;
    let mut texels: Vec<u8> = (0..size)
        .map(|i| file.get(at + i).copied().unwrap_or(0))
        .collect();
    if format >= TEXFMT_16BIT {
        for pair in texels.as_chunks_mut::<2>().0 {
            pair.swap(0, 1);
        }
    }
    if !cpu.bus.guest_probe(data, size, true) {
        return None;
    }
    if let Some(table) = table {
        cpu.bus.guest_write_bytes(info + INFO_TABLE, &table);
    }
    cpu.bus.guest_write_bytes(data, &texels);
    Some(TRUE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_header_gives_the_mipmap() {
        let file = b"3df v1.0\nargb1555\nlod range: 1 256\naspect ratio: 2 1\nDATA";
        let h = parse_header(file).expect("a 3DF header");
        assert_eq!(
            h,
            Header {
                width: 256,
                height: 128,
                small_lod: api::LOD_1,
                large_lod: api::LOD_256,
                aspect: api::ASPECT_1X1 - 1,
                format: 0xB,
            }
        );
        assert_eq!(&file[data_start(file).unwrap_or(0)..], b"DATA");
        // A tall one, and what is no 3DF file.
        let tall = parse_header(b"3df v1.0\np8\nlod range: 8 64\naspect ratio: 1 4\n");
        assert_eq!(
            tall.map(|h| (h.width, h.height, h.aspect)),
            Some((16, 64, 5))
        );
        assert_eq!(
            parse_header(b"3df v1.0\nxyz\nlod range: 1 8\naspect ratio: 1 1\n"),
            None
        );
        assert_eq!(parse_header(b"BM\n"), None);
    }

    #[test]
    fn a_mipmap_takes_its_levels() {
        // 8x8 to 1x1 of 16-bit texels: 64 + 16 + 4 + 1 texels.
        let bytes = mipmap_bytes(api::ASPECT_1X1, api::LOD_1 - 3, api::LOD_1, 0xA);
        assert_eq!(bytes, 2 * (64 + 16 + 4 + 1));
    }
}
