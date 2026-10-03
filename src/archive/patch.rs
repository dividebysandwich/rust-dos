//! Binary patches, as a .dosc carries them for the files of its game
//! (copy protection taken out, say): IPS, BPS and VCDIFF (RFC 3284, as
//! xdelta writes it), each from its published description. XOR patches,
//! whose format isn't published, are known by their name only.

/// What a patch is, by its first bytes, or by its name for those that say
/// nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Ips,
    Bps,
    Vcdiff,
    Xor,
}

/// The extensions a patch named after its file has (`GAME.EXE.ips`).
pub const EXTENSIONS: [(&str, Kind); 5] =
    [("ips", Kind::Ips), ("bps", Kind::Bps), ("vcdiff", Kind::Vcdiff), ("xdelta", Kind::Vcdiff), ("xor", Kind::Xor)];

/// The patch `data` is, by its magic.
pub fn kind(data: &[u8]) -> Option<Kind> {
    if data.starts_with(b"PATCH") {
        Some(Kind::Ips)
    } else if data.starts_with(b"BPS1") {
        Some(Kind::Bps)
    } else if data.starts_with(&[0xD6, 0xC3, 0xC4, 0x00]) {
        Some(Kind::Vcdiff)
    } else {
        None
    }
}

/// `base` with the patch `patch` (of the kind `kind`) applied.
pub fn apply(kind: Kind, base: &[u8], patch: &[u8]) -> Result<Vec<u8>, String> {
    match kind {
        Kind::Ips => ips(base, patch),
        Kind::Bps => bps(base, patch),
        Kind::Vcdiff => vcdiff(base, patch),
        Kind::Xor => Err("XOR patches aren't supported".to_string()),
    }
}

fn short() -> String {
    "the patch ends too soon".to_string()
}

/// IPS: records of a 3-byte offset and a 2-byte length with that many
/// bytes, or a length of 0 with a 2-byte count and a byte to repeat, to
/// `EOF`; after it, optionally, the length to cut the file to.
fn ips(base: &[u8], patch: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = base.to_vec();
    let mut at = 5;
    let take = |at: &mut usize, n: usize| -> Result<&[u8], String> {
        let bytes = patch.get(*at..*at + n).ok_or_else(short)?;
        *at += n;
        Ok(bytes)
    };
    let be = |b: &[u8]| b.iter().fold(0usize, |v, &x| v << 8 | x as usize);
    loop {
        let offset = take(&mut at, 3)?;
        if offset == b"EOF" {
            if let Ok(cut) = take(&mut at, 3) {
                out.truncate(be(cut));
            }
            return Ok(out);
        }
        let offset = be(offset);
        let len = be(take(&mut at, 2)?);
        let bytes: Vec<u8> = if len == 0 {
            let count = be(take(&mut at, 2)?);
            vec![take(&mut at, 1)?[0]; count]
        } else {
            take(&mut at, len)?.to_vec()
        };
        if out.len() < offset + bytes.len() {
            out.resize(offset + bytes.len(), 0);
        }
        out[offset..offset + bytes.len()].copy_from_slice(&bytes);
    }
}

/// BPS: the sizes and metadata, then actions (read from the source where
/// the output is, read from the patch, copy from the source or from the
/// output so far, at offsets relative to the last), and the CRC32s of the
/// source, the target and the patch.
fn bps(base: &[u8], patch: &[u8]) -> Result<Vec<u8>, String> {
    if patch.len() < 4 + 12 {
        return Err(short());
    }
    let crc = |data: &[u8]| crc32fast::hash(data);
    let footer = &patch[patch.len() - 12..];
    let word = |b: &[u8]| u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    if crc(&patch[..patch.len() - 4]) != word(&footer[8..]) {
        return Err("the patch is damaged".to_string());
    }
    if crc(base) != word(footer) {
        return Err("the patch is for another version of the file".to_string());
    }
    let actions_end = patch.len() - 12;
    let mut at = 4;
    let number = |at: &mut usize| -> Result<u64, String> {
        let (mut data, mut shift) = (0u64, 1u64);
        loop {
            let x = *patch.get(*at).filter(|_| *at < actions_end).ok_or_else(short)? as u64;
            *at += 1;
            data += (x & 0x7F) * shift;
            if x & 0x80 != 0 {
                return Ok(data);
            }
            shift <<= 7;
            data += shift;
        }
    };
    let _source_size = number(&mut at)?;
    let target_size = number(&mut at)? as usize;
    let metadata = number(&mut at)? as usize;
    at += metadata;
    let mut out: Vec<u8> = Vec::with_capacity(target_size);
    let (mut source_at, mut target_at) = (0i64, 0i64);
    let relative = |data: u64| if data & 1 != 0 { -((data >> 1) as i64) } else { (data >> 1) as i64 };
    while at < actions_end {
        let command = number(&mut at)?;
        let len = (command >> 2) as usize + 1;
        match command & 3 {
            0 => {
                let from = out.len();
                out.extend_from_slice(base.get(from..from + len).ok_or_else(short)?);
            }
            1 => {
                out.extend_from_slice(patch.get(at..at + len).filter(|_| at + len <= actions_end).ok_or_else(short)?);
                at += len;
            }
            2 => {
                source_at += relative(number(&mut at)?);
                let from = usize::try_from(source_at).map_err(|_| short())?;
                out.extend_from_slice(base.get(from..from + len).ok_or_else(short)?);
                source_at += len as i64;
            }
            _ => {
                target_at += relative(number(&mut at)?);
                let from = usize::try_from(target_at).map_err(|_| short())?;
                // It may copy what it is writing, a byte at a time.
                for k in from..from + len {
                    let byte = *out.get(k).ok_or_else(short)?;
                    out.push(byte);
                }
                target_at += len as i64;
            }
        }
    }
    if out.len() != target_size || crc(&out) != word(&footer[4..]) {
        return Err("the patched file isn't what the patch says".to_string());
    }
    Ok(out)
}

/// VCDIFF's instruction types.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    Noop,
    Add,
    Run,
    Copy(u8),
}

/// An entry of the code table: up to two instructions with their sizes (0:
/// the size follows in the instructions).
type Code = [(Op, u8); 2];

/// RFC 3284's default code table, built as its section 5.6 lays it out.
fn default_code_table() -> Vec<Code> {
    let none = (Op::Noop, 0);
    let mut table = vec![[(Op::Run, 0), none]];
    for size in 0..=17 {
        table.push([(Op::Add, size), none]);
    }
    for mode in 0..=8 {
        table.push([(Op::Copy(mode), 0), none]);
        for size in 4..=18 {
            table.push([(Op::Copy(mode), size), none]);
        }
    }
    for mode in 0..=5 {
        for add in 1..=4 {
            for copy in 4..=6 {
                table.push([(Op::Add, add), (Op::Copy(mode), copy)]);
            }
        }
    }
    for mode in 6..=8 {
        for add in 1..=4 {
            table.push([(Op::Add, add), (Op::Copy(mode), 4)]);
        }
    }
    for mode in 0..=8 {
        table.push([(Op::Copy(mode), 4), (Op::Add, 1)]);
    }
    debug_assert_eq!(table.len(), 256);
    table
}

/// A VCDIFF integer: big-endian groups of 7 bits, all but the last with
/// the top bit set.
fn varint(data: &[u8], at: &mut usize) -> Result<u64, String> {
    let mut value = 0u64;
    loop {
        let b = *data.get(*at).ok_or_else(short)?;
        *at += 1;
        value = value.checked_mul(128).ok_or("a number too large in the patch")? | (b & 0x7F) as u64;
        if b & 0x80 == 0 {
            return Ok(value);
        }
    }
}

/// VCDIFF (RFC 3284) without secondary compression or a code table of its
/// own: windows of target data made of ADD, RUN and COPY instructions, a
/// COPY reading from a segment of the source (or of the target so far) or
/// from the window itself, its address coded through the near and same
/// caches.
fn vcdiff(base: &[u8], patch: &[u8]) -> Result<Vec<u8>, String> {
    const NEAR: usize = 4;
    const SAME: usize = 3;
    let table = default_code_table();
    let byte = |at: &mut usize| -> Result<u8, String> {
        let b = *patch.get(*at).ok_or_else(short)?;
        *at += 1;
        Ok(b)
    };
    let mut at = 4;
    let header = byte(&mut at)?;
    if header & 0x01 != 0 {
        return Err("patches with secondary compression aren't supported".to_string());
    }
    if header & 0x02 != 0 {
        return Err("patches with a code table of their own aren't supported".to_string());
    }
    // xdelta's application header.
    if header & 0x04 != 0 {
        let len = varint(patch, &mut at)? as usize;
        at += len;
    }
    let mut out: Vec<u8> = Vec::new();
    while at < patch.len() {
        let window = byte(&mut at)?;
        let segment = if window & 0x03 != 0 {
            let len = varint(patch, &mut at)? as usize;
            let pos = varint(patch, &mut at)? as usize;
            let from: &[u8] = if window & 0x01 != 0 { base } else { &out };
            from.get(pos..pos + len).ok_or("the patch reads past its source")?.to_vec()
        } else {
            Vec::new()
        };
        let _delta_len = varint(patch, &mut at)?;
        let target_len = varint(patch, &mut at)? as usize;
        if byte(&mut at)? != 0 {
            return Err("patches with compressed sections aren't supported".to_string());
        }
        let data_len = varint(patch, &mut at)? as usize;
        let inst_len = varint(patch, &mut at)? as usize;
        let addr_len = varint(patch, &mut at)? as usize;
        // xdelta's checksum of the window.
        if window & 0x04 != 0 {
            at += 4;
        }
        let section = |at: usize, len: usize| patch.get(at..at + len).ok_or_else(short);
        let data = section(at, data_len)?;
        let inst = section(at + data_len, inst_len)?;
        let addrs = section(at + data_len + inst_len, addr_len)?;
        at += data_len + inst_len + addr_len;

        let mut target: Vec<u8> = Vec::with_capacity(target_len);
        let (mut d, mut i, mut a) = (0usize, 0usize, 0usize);
        let (mut near, mut next_near, mut same) = ([0usize; NEAR], 0usize, vec![0usize; SAME * 256]);
        while i < inst.len() {
            let code = table[inst[i] as usize];
            i += 1;
            for (op, size) in code {
                if op == Op::Noop {
                    continue;
                }
                let size = if size == 0 { varint(inst, &mut i)? as usize } else { size as usize };
                match op {
                    Op::Add => {
                        target.extend_from_slice(data.get(d..d + size).ok_or_else(short)?);
                        d += size;
                    }
                    Op::Run => {
                        let b = *data.get(d).ok_or_else(short)?;
                        d += 1;
                        target.resize(target.len() + size, b);
                    }
                    Op::Copy(mode) => {
                        let here = segment.len() + target.len();
                        let addr = match mode as usize {
                            0 => varint(addrs, &mut a)? as usize,
                            1 => here.checked_sub(varint(addrs, &mut a)? as usize).ok_or("a bad address in the patch")?,
                            m if m < 2 + NEAR => near[m - 2] + varint(addrs, &mut a)? as usize,
                            m => {
                                let b = *addrs.get(a).ok_or_else(short)? as usize;
                                a += 1;
                                same[(m - 2 - NEAR) * 256 + b]
                            }
                        };
                        near[next_near] = addr;
                        next_near = (next_near + 1) % NEAR;
                        same[addr % (SAME * 256)] = addr;
                        // From the segment, then from the window as it is
                        // made, which a copy may run into.
                        for k in 0..size {
                            let from = addr + k;
                            let b = if from < segment.len() {
                                segment[from]
                            } else {
                                *target.get(from - segment.len()).ok_or("a copy from past the window")?
                            };
                            target.push(b);
                        }
                    }
                    Op::Noop => {}
                }
            }
        }
        if target.len() != target_len {
            return Err("a window of the patch isn't the size it says".to_string());
        }
        out.extend_from_slice(&target);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ips_writes_records_and_runs() {
        let base = b"Hello, world!".to_vec();
        let mut patch = b"PATCH".to_vec();
        patch.extend([0, 0, 7, 0, 5]);
        patch.extend(b"there");
        // A run of three '!' past the end.
        patch.extend([0, 0, 13, 0, 0, 0, 3, b'!']);
        patch.extend(b"EOF");
        assert_eq!(apply(Kind::Ips, &base, &patch).unwrap(), b"Hello, there!!!!");
        // Cut short after EOF.
        patch.extend([0, 0, 5]);
        assert_eq!(apply(Kind::Ips, &base, &patch).unwrap(), b"Hello");
        assert_eq!(kind(&patch), Some(Kind::Ips));
    }

    /// A BPS number.
    fn bps_number(mut n: u64, out: &mut Vec<u8>) {
        loop {
            let x = (n & 0x7F) as u8;
            n >>= 7;
            if n == 0 {
                out.push(x | 0x80);
                return;
            }
            out.push(x);
            n -= 1;
        }
    }

    #[test]
    fn bps_does_all_four_actions() {
        let base = b"ABCDEFGH".to_vec();
        let target = b"ABCDxyzEFGHxyzz".to_vec();
        let mut patch = b"BPS1".to_vec();
        for n in [base.len() as u64, target.len() as u64, 0] {
            bps_number(n, &mut patch);
        }
        let action = |kind: u64, len: u64, out: &mut Vec<u8>| bps_number(((len - 1) << 2) | kind, out);
        action(0, 4, &mut patch); // ABCD from the source where the output is
        action(1, 3, &mut patch); // xyz
        patch.extend(b"xyz");
        action(2, 4, &mut patch); // EFGH: the source from 4
        bps_number(4 << 1, &mut patch);
        action(3, 3, &mut patch); // xyz: the output from 4
        bps_number(4 << 1, &mut patch);
        action(3, 1, &mut patch); // z: back from 7 to 6
        bps_number((1 << 1) | 1, &mut patch);
        patch.extend(crc32fast::hash(&base).to_le_bytes());
        patch.extend(crc32fast::hash(&target).to_le_bytes());
        let whole = crc32fast::hash(&patch);
        patch.extend(whole.to_le_bytes());
        assert_eq!(String::from_utf8(apply(Kind::Bps, &base, &patch).unwrap()).unwrap(), "ABCDxyzEFGHxyzz");
        assert!(apply(Kind::Bps, b"other", &patch).is_err(), "another file");
    }

    #[test]
    fn the_default_code_table_is_rfc_3284_s() {
        let table = default_code_table();
        assert!(table[0] == [(Op::Run, 0), (Op::Noop, 0)]);
        assert!(table[1] == [(Op::Add, 0), (Op::Noop, 0)]);
        assert!(table[19] == [(Op::Copy(0), 0), (Op::Noop, 0)]);
        assert!(table[162] == [(Op::Copy(8), 18), (Op::Noop, 0)]);
        assert!(table[163] == [(Op::Add, 1), (Op::Copy(0), 4)]);
        assert!(table[235] == [(Op::Add, 1), (Op::Copy(6), 4)]);
        assert!(table[255] == [(Op::Copy(8), 4), (Op::Add, 1)]);
    }

    #[test]
    fn vcdiff_copies_adds_and_runs() {
        let base = b"0123456789".to_vec();
        // One window over the whole source: COPY 4 from 2 (mode 0), ADD
        // "ab", RUN 3 of 'z', COPY 4 from the window itself ("2345"), with
        // the here mode.
        let data = *b"abz";
        let inst = [
            19 + 1, // COPY mode 0, size 4
            1 + 2,  // ADD size 2
            0, 3,   // RUN, size 3 following
            19 + 16 + 1, // COPY mode 1 (here), size 4
        ];
        // The second address: here is 10 + 9 = 19, the window's "2345"
        // starts at 10, so 9 back.
        let addrs = [2, 9];
        let mut window = vec![0x01, 10, 0];
        let mut delta = vec![13, 0, data.len() as u8, inst.len() as u8, addrs.len() as u8];
        delta.extend(data);
        delta.extend(inst);
        delta.extend(addrs);
        window.push(delta.len() as u8);
        window.extend(delta);
        let mut patch = vec![0xD6, 0xC3, 0xC4, 0x00, 0x00];
        patch.extend(window);
        assert_eq!(kind(&patch), Some(Kind::Vcdiff));
        assert_eq!(String::from_utf8(apply(Kind::Vcdiff, &base, &patch).unwrap()).unwrap(), "2345abzzz2345");
    }
}
