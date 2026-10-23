//! Windows metafiles: a handle for a metafile's bytes
//! (SetMetaFileBitsEx), and PlayMetaFile drawing its records into a DC.
//! Wise's installer keeps the pictures of its dialogs as metafiles.

use super::{Kind, add, gdi, set_error};
use crate::cpu::Cpu;

const ERROR_INVALID_HANDLE: u32 = 6;
const ERROR_INVALID_DATA: u32 = 13;

/// A metafile header's size: 9 words.
const HEADER: usize = 18;

/// The records of `data`, a metafile with its header, as (function,
/// parameters); they stop at the end record or where one would run past
/// the data.
pub fn records(data: &[u8]) -> Vec<(u16, Vec<u16>)> {
    let mut out = Vec::new();
    let header_words = data
        .get(2..4)
        .map_or(9, |b| u16::from_le_bytes([b[0], b[1]]));
    let mut at = header_words as usize * 2;
    while let Some(b) = data.get(at..at + 6) {
        let words = u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize;
        let function = u16::from_le_bytes([b[4], b[5]]);
        if function == 0 || words < 3 {
            break;
        }
        let Some(end) = words.checked_mul(2).and_then(|n| at.checked_add(n)) else {
            break;
        };
        let Some(params) = data.get(at + 6..end) else {
            break;
        };
        let params = params
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        out.push((function, params));
        at = end;
    }
    out
}

/// Whether `data` starts with a metafile header: in memory (1) or on
/// disk (2), 9 words long.
fn is_metafile(data: &[u8]) -> bool {
    data.len() >= HEADER
        && matches!(u16::from_le_bytes([data[0], data[1]]), 1 | 2)
        && u16::from_le_bytes([data[2], data[3]]) == 9
}

fn metafile(cpu: &mut Cpu, handle: u32) -> Option<Vec<u8>> {
    match &gdi(cpu).get(handle)?.kind {
        Kind::MetaFile { data } => Some(data.clone()),
        _ => None,
    }
}

/// SetMetaFileBitsEx(size, data): a metafile handle for a copy of the
/// `size` bytes at `data`, or 0 when they don't start with a metafile
/// header.
pub fn set_meta_file_bits_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[size, at] = args else { unreachable!() };
    let mut data = vec![0u8; size as usize];
    cpu.bus.guest_read_bytes(at, &mut data);
    if cpu.bus.guest_faulted() {
        return None;
    }
    if !is_metafile(&data) {
        set_error(cpu, ERROR_INVALID_DATA);
        return Some(0);
    }
    Some(add(cpu, Kind::MetaFile { data }))
}

/// GetMetaFileBitsEx(metafile, size, buffer): the metafile's bytes, or
/// their number when `buffer` is 0.
pub fn get_meta_file_bits_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, size, buffer] = args else {
        unreachable!()
    };
    let Some(data) = metafile(cpu, handle) else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0);
    };
    if buffer == 0 {
        return Some(data.len() as u32);
    }
    let n = data.len().min(size as usize);
    cpu.bus.guest_write_bytes(buffer, &data[..n]);
    Some(n as u32)
}

/// DeleteMetaFile(metafile).
pub fn delete_meta_file(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if metafile(cpu, args[0]).is_none() {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0);
    }
    super::delete_object_now(cpu, args[0]);
    Some(1)
}

/// PlayMetaFile(hdc, metafile): draws the metafile's records into the DC
/// through GDI's own functions. The pens, brushes and fonts its records
/// make are deleted at the end, and the DC gets back the ones it had
/// selected. Records this doesn't know, palettes and regions among them,
/// are left out.
pub fn play_meta_file(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hdc, handle] = args else { unreachable!() };
    let Some(data) = metafile(cpu, handle) else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0);
    };
    let records = records(&data);
    // Guest memory for the records' structures, points, text and DIBs.
    let need = records
        .iter()
        .map(|(_, p)| p.len() * 4 + 64)
        .max()
        .unwrap_or(64);
    let scratch = crate::win32::ole32::task_alloc(cpu, need as u32);
    if scratch == 0 {
        return Some(0);
    }
    let mut player = Player {
        hdc,
        scratch,
        objects: Vec::new(),
        selected: Vec::new(),
    };
    for (function, p) in &records {
        player.play(cpu, *function, p);
    }
    for (_, original) in std::mem::take(&mut player.selected) {
        super::dc::select_object(cpu, &[hdc, original]);
    }
    for object in player.objects.into_iter().flatten() {
        if object != 0 {
            super::delete_object(cpu, &[object]);
        }
    }
    crate::win32::ole32::task_free(cpu, scratch);
    Some(1)
}

/// A 16-bit parameter as the signed number it is.
fn signed(word: u16) -> u32 {
    word as i16 as i32 as u32
}

/// Words as the bytes they hold.
fn bytes(words: &[u16]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

struct Player {
    hdc: u32,
    /// Guest memory big enough for any record's parameters, widened.
    scratch: u32,
    /// The metafile's object table: the handles its records made, by
    /// index; Some(0) for an object this doesn't make (a palette).
    objects: Vec<Option<u32>>,
    /// The objects the DC had selected before a record selected one of
    /// that type: (type, handle).
    selected: Vec<(u32, u32)>,
}

impl Player {
    /// Put `bytes` in the scratch memory, from its start plus `offset`.
    fn put(&self, cpu: &mut Cpu, offset: u32, bytes: &[u8]) -> u32 {
        cpu.bus.guest_write_bytes(self.scratch + offset, bytes);
        self.scratch + offset
    }

    /// The object made by a record goes in the first free place of the
    /// table.
    fn keep(&mut self, handle: u32) {
        match self.objects.iter().position(Option::is_none) {
            Some(i) => self.objects[i] = Some(handle),
            None => self.objects.push(Some(handle)),
        }
    }

    /// Points as (x, y) pairs of signed words, widened to POINTs.
    fn points(&self, cpu: &mut Cpu, words: &[u16]) -> u32 {
        let wide: Vec<u8> = words
            .iter()
            .flat_map(|&w| signed(w).to_le_bytes())
            .collect();
        self.put(cpu, 0, &wide)
    }

    /// A DIB that starts at `p[from]`: its BITMAPINFO and its bits, put
    /// in the scratch memory.
    fn dib(&self, cpu: &mut Cpu, p: &[u16], from: usize) -> Option<(u32, u32)> {
        let dib = bytes(p.get(from..)?);
        let word = |at: usize| {
            dib.get(at..at + 2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
        };
        let dword = |at: usize| {
            dib.get(at..at + 4)
                .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        };
        let header = dword(0)?;
        let (bpp, used) = if header == 12 {
            (word(10)?, 0)
        } else {
            (word(14)?, dword(32)?)
        };
        let compression = if header == 12 { 0 } else { dword(16)? };
        let entries = if used != 0 {
            used
        } else if bpp <= 8 {
            1 << bpp
        } else if compression == 3 {
            3
        } else {
            0
        };
        let entry = if header == 12 { 3 } else { 4 };
        let bits = header.checked_add(entries.checked_mul(entry)?)?;
        if bits as usize > dib.len() {
            return None;
        }
        let at = self.put(cpu, 0, &dib);
        Some((at, at + bits))
    }

    fn play(&mut self, cpu: &mut Cpu, function: u16, p: &[u16]) {
        let hdc = self.hdc;
        let w = |i: usize| p.get(i).copied().unwrap_or(0);
        let s = |i: usize| signed(w(i));
        let dword = |i: usize| w(i) as u32 | (w(i + 1) as u32) << 16;
        use super::{bitmap, dc, draw, font};
        match function {
            0x001E => {
                dc::save_dc(cpu, &[hdc]);
            }
            0x0127 => {
                dc::restore_dc(cpu, &[hdc, s(0)]);
            }
            0x0102 => {
                dc::set_bk_mode(cpu, &[hdc, w(0) as u32]);
            }
            0x0103 => {
                dc::set_map_mode(cpu, &[hdc, w(0) as u32]);
            }
            0x0104 => {
                dc::set_rop2(cpu, &[hdc, w(0) as u32]);
            }
            0x0106 => {
                dc::set_poly_fill_mode(cpu, &[hdc, w(0) as u32]);
            }
            0x0107 => {
                dc::set_stretch_blt_mode(cpu, &[hdc, w(0) as u32]);
            }
            0x012E => {
                dc::set_text_align(cpu, &[hdc, w(0) as u32]);
            }
            0x0201 => {
                dc::set_bk_color(cpu, &[hdc, dword(0)]);
            }
            0x0209 => {
                dc::set_text_color(cpu, &[hdc, dword(0)]);
            }
            0x020B => {
                dc::set_window_org_ex(cpu, &[hdc, s(1), s(0), 0]);
            }
            0x020C => {
                dc::set_window_ext_ex(cpu, &[hdc, s(1), s(0), 0]);
            }
            0x020D => {
                dc::set_viewport_org_ex(cpu, &[hdc, s(1), s(0), 0]);
            }
            0x020E => {
                dc::set_viewport_ext_ex(cpu, &[hdc, s(1), s(0), 0]);
            }
            0x0214 => {
                draw::move_to_ex(cpu, &[hdc, s(1), s(0), 0]);
            }
            0x0213 => {
                draw::line_to(cpu, &[hdc, s(1), s(0)]);
            }
            0x041B => {
                draw::rectangle(cpu, &[hdc, s(3), s(2), s(1), s(0)]);
            }
            0x0418 => {
                draw::ellipse(cpu, &[hdc, s(3), s(2), s(1), s(0)]);
            }
            0x0324 | 0x0325 => {
                let count = w(0) as usize;
                let Some(xy) = p.get(1..1 + count * 2) else {
                    return;
                };
                let at = self.points(cpu, xy);
                if function == 0x0324 {
                    draw::polygon(cpu, &[hdc, at, count as u32]);
                } else {
                    draw::polyline(cpu, &[hdc, at, count as u32]);
                }
            }
            0x061D => {
                draw::pat_blt(cpu, &[hdc, s(5), s(4), s(3), s(2), dword(0)]);
            }
            0x0F43 => {
                if let Some((info, bits)) = self.dib(cpu, p, 11) {
                    bitmap::stretch_di_bits(
                        cpu,
                        &[
                            hdc,
                            s(10),
                            s(9),
                            s(8),
                            s(7),
                            s(6),
                            s(5),
                            s(4),
                            s(3),
                            bits,
                            info,
                            w(2) as u32,
                            dword(0),
                        ],
                    );
                }
            }
            0x0B41 => {
                if let Some((info, bits)) = self.dib(cpu, p, 10) {
                    bitmap::stretch_di_bits(
                        cpu,
                        &[
                            hdc,
                            s(9),
                            s(8),
                            s(7),
                            s(6),
                            s(5),
                            s(4),
                            s(3),
                            s(2),
                            bits,
                            info,
                            0,
                            dword(0),
                        ],
                    );
                }
            }
            0x0940 => {
                if let Some((info, bits)) = self.dib(cpu, p, 8) {
                    bitmap::stretch_di_bits(
                        cpu,
                        &[
                            hdc,
                            s(7),
                            s(6),
                            s(5),
                            s(4),
                            s(3),
                            s(2),
                            s(5),
                            s(4),
                            bits,
                            info,
                            0,
                            dword(0),
                        ],
                    );
                }
            }
            0x0521 => {
                let count = w(0) as usize;
                let words = count.div_ceil(2);
                let text = bytes(p.get(1..1 + words).unwrap_or(&[]));
                let at = self.put(cpu, 0, &text[..count.min(text.len())]);
                font::text_out(cpu, &[hdc, s(2 + words), s(1 + words), at, count as u32]);
            }
            0x0A32 => {
                let count = w(2) as usize;
                let options = w(3) as u32;
                let mut at = 4;
                let mut rect = 0;
                if options & 0x6 != 0 {
                    let r: Vec<u8> = (0..4).flat_map(|i| s(4 + i).to_le_bytes()).collect();
                    rect = self.put(cpu, 0, &r);
                    at += 4;
                }
                let words = count.div_ceil(2);
                let text = bytes(p.get(at..at + words).unwrap_or(&[]));
                let text = self.put(cpu, 16, &text[..count.min(text.len())]);
                font::ext_text_out(
                    cpu,
                    &[hdc, s(1), s(0), options, rect, text, count as u32, 0],
                );
            }
            0x02FA => {
                // LOGPEN16 to LOGPEN: style, width, color.
                let pen: Vec<u8> = [w(0) as u32, s(1), s(2), dword(3)]
                    .iter()
                    .flat_map(|v| v.to_le_bytes())
                    .collect();
                let at = self.put(cpu, 0, &pen);
                let handle = super::create_pen_indirect(cpu, &[at]).unwrap_or(0);
                self.keep(handle);
            }
            0x02FC => {
                // LOGBRUSH16 to LOGBRUSH: style, color, hatch.
                let brush: Vec<u8> = [w(0) as u32, dword(1), w(3) as u32]
                    .iter()
                    .flat_map(|v| v.to_le_bytes())
                    .collect();
                let at = self.put(cpu, 0, &brush);
                let handle = super::create_brush_indirect(cpu, &[at]).unwrap_or(0);
                self.keep(handle);
            }
            0x02FB => {
                // LOGFONT16 to LOGFONTA: five numbers widened, then the
                // bytes and the face name as they are.
                let mut logfont: Vec<u8> = (0..5).flat_map(|i| s(i).to_le_bytes()).collect();
                let mut rest = bytes(p.get(5..).unwrap_or(&[]));
                rest.resize(40, 0);
                rest[39] = 0;
                logfont.extend(rest);
                let at = self.put(cpu, 0, &logfont);
                let handle = font::create_font_indirect(cpu, &[at]).unwrap_or(0);
                self.keep(handle);
            }
            0x00F7 | 0x01F9 | 0x0142 | 0x06FF => self.keep(0),
            0x012D => {
                let Some(&Some(handle)) = self.objects.get(w(0) as usize) else {
                    return;
                };
                if handle == 0 {
                    return;
                }
                let kind = super::gdi(cpu).get(handle).map(|o| o.type_of());
                let old = dc::select_object(cpu, &[hdc, handle]).unwrap_or(0);
                if let Some(kind) = kind
                    && !self.selected.iter().any(|&(k, _)| k == kind)
                {
                    self.selected.push((kind, old));
                }
            }
            0x01F0 => {
                if let Some(slot) = self.objects.get_mut(w(0) as usize)
                    && let Some(handle) = slot.take()
                    && handle != 0
                {
                    super::delete_object(cpu, &[handle]);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A metafile of `records` (function, parameters), with its header
    /// and the end record.
    fn metafile(records: &[(u16, &[u16])]) -> Vec<u8> {
        let mut body = Vec::new();
        for (function, params) in records.iter().chain([(0u16, &[][..])].iter()) {
            body.extend((3 + params.len() as u32).to_le_bytes());
            body.extend(function.to_le_bytes());
            for p in params.iter() {
                body.extend(p.to_le_bytes());
            }
        }
        let mut data = vec![1, 0, 9, 0, 0, 3];
        data.extend((((HEADER + body.len()) / 2) as u32).to_le_bytes());
        data.extend([0u8; 8]);
        data.extend(body);
        data
    }

    #[test]
    fn records_are_read_up_to_the_end_record() {
        let data = metafile(&[(0x020C, &[200, 100]), (0x0107, &[3])]);
        assert!(is_metafile(&data));
        assert_eq!(
            records(&data),
            vec![(0x020C, vec![200, 100]), (0x0107, vec![3])]
        );
    }

    #[test]
    fn a_record_past_the_end_stops_the_list() {
        let mut data = metafile(&[(0x0107, &[3]), (0x020C, &[200, 100])]);
        // The second record says it is longer than what is left.
        let at = HEADER + 8;
        data[at..at + 4].copy_from_slice(&1000u32.to_le_bytes());
        assert_eq!(records(&data), vec![(0x0107, vec![3])]);
        assert!(!is_metafile(&data[..10]));
        assert!(!is_metafile(&[
            3, 0, 9, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
        ]));
    }
}
