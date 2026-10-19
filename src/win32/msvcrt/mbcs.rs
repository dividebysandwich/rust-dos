//! The multibyte code page of the _mbs functions, which `_setmbcp` sets:
//! its table in `_mbctype` says which bytes lead a double-byte character
//! and which may follow one. MSVCRT has its own tables for the East Asian
//! code pages, so they work on any Windows; the others are single-byte,
//! with only ASCII's letters marked.

use super::*;

/// `_mbctype`'s bits: upper and lower case single-byte letters, lead and
/// trail bytes.
const SBUP: u8 = 0x10;
const SBLOW: u8 = 0x20;
const M1: u8 = 0x04;
const M2: u8 = 0x08;

/// `_mbsbtype`'s answers.
const MBC_SINGLE: u32 = 0;
const MBC_LEAD: u32 = 1;
const MBC_TRAIL: u32 = 2;
const MBC_ILLEGAL: u32 = u32::MAX;

/// The tables of the double-byte code pages, as runs of (flags, count)
/// over `_mbctype`'s 257 entries (EOF's, then each byte's).
const CP932: &[(u8, u16)] = &[
    (0x00, 65),
    (0x08, 1),
    (0x18, 26),
    (0x08, 6),
    (0x28, 26),
    (0x08, 4),
    (0x00, 1),
    (0x08, 1),
    (0x0c, 31),
    (0x08, 1),
    (0x0a, 5),
    (0x09, 58),
    (0x0c, 29),
    (0x00, 3),
];
const CP936: &[(u8, u16)] = &[
    (0x00, 65),
    (0x08, 1),
    (0x18, 26),
    (0x08, 6),
    (0x28, 26),
    (0x08, 6),
    (0x0c, 126),
    (0x00, 1),
];
const CP949: &[(u8, u16)] = &[
    (0x00, 66),
    (0x18, 26),
    (0x08, 6),
    (0x28, 26),
    (0x08, 6),
    (0x0c, 126),
    (0x00, 1),
];
const CP950: &[(u8, u16)] = &[
    (0x00, 65),
    (0x08, 1),
    (0x18, 26),
    (0x08, 6),
    (0x28, 26),
    (0x08, 4),
    (0x00, 2),
    (0x04, 32),
    (0x0c, 94),
    (0x00, 1),
];

/// The single-byte code pages `_setmbcp` takes.
const SINGLE_BYTE: &[u32] = &[
    0, 437, 737, 775, 850, 852, 855, 857, 860, 861, 862, 863, 864, 865, 866, 869, 874, 1250, 1251,
    1252, 1253, 1254, 1255, 1256, 1257, 1258,
];

/// `_mbctype` for the code page `cp`, or None for one there isn't.
pub fn table(cp: u32) -> Option<[u8; 257]> {
    let mut t = [0u8; 257];
    let runs = match cp {
        932 => CP932,
        936 => CP936,
        949 => CP949,
        950 => CP950,
        1361 => {
            // Johab: its lead and trail bytes, and the letters.
            for b in 0..=255u8 {
                let mut flags = match b {
                    b'A'..=b'Z' => SBUP,
                    b'a'..=b'z' => SBLOW,
                    _ => 0,
                };
                if matches!(b, 0x81..=0xD3 | 0xD8..=0xDE | 0xE0..=0xF9) {
                    flags |= M1;
                }
                if matches!(b, 0x31..=0x7E | 0x81..=0xFE) {
                    flags |= M2;
                }
                t[b as usize + 1] = flags;
            }
            return Some(t);
        }
        cp if SINGLE_BYTE.contains(&cp) => {
            for b in b'A'..=b'Z' {
                t[b as usize + 1] = SBUP;
                t[b as usize + 1 + 32] = SBLOW;
            }
            return Some(t);
        }
        _ => return None,
    };
    let mut at = 0;
    for &(flags, count) in runs {
        t[at..at + count as usize].fill(flags);
        at += count as usize;
    }
    Some(t)
}

/// The flags of `byte` in the running process's `_mbctype`.
fn flags(cpu: &mut Cpu, byte: u8) -> u8 {
    cpu.bus.guest_read_8(var_at(V_MBCTYPE) + 1 + byte as u32)
}

fn is_lead(cpu: &mut Cpu, byte: u8) -> bool {
    flags(cpu, byte) & M1 != 0
}

fn is_trail(cpu: &mut Cpu, byte: u8) -> bool {
    flags(cpu, byte) & M2 != 0
}

/// _getmbcp(): the code page _setmbcp set.
pub fn getmbcp(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(state(cpu, S_MBCP))
}

/// _setmbcp(cp): -3 (_MB_CP_ANSI) and -4 (_MB_CP_LOCALE) are 1252, -2
/// (_MB_CP_OEM) 437, -1 (_MB_CP_SBCS) single bytes. 0, or -1 with errno
/// EINVAL for a code page there isn't.
pub fn setmbcp(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let cp = match args[0] as i32 {
        -3 | -4 => 1252,
        -2 => 437,
        -1 => 0,
        cp => cp as u32,
    };
    let Some(t) = table(cp) else {
        set_errno(cpu, EINVAL);
        return Some(u32::MAX);
    };
    put(cpu, var_at(V_MBCTYPE), &t)?;
    set_state(cpu, S_MBCP, cp);
    Some(0)
}

/// _ismbblead(c) and _ismbbtrail(c), on c's low byte: 1 or 0.
pub fn ismbblead(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(is_lead(cpu, args[0] as u8) as u32)
}

pub fn ismbbtrail(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(is_trail(cpu, args[0] as u8) as u32)
}

/// _ismbclegal(c): whether c is a lead and a trail byte.
pub fn ismbclegal(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let c = args[0];
    Some((c < 0x10000 && is_lead(cpu, (c >> 8) as u8) && is_trail(cpu, c as u8)) as u32)
}

/// Whether the byte at `at` leads a character, reading the string from
/// `start`, where lead bytes may also be trail bytes.
fn leads(cpu: &mut Cpu, start: u32, at: u32) -> Option<bool> {
    let mut lead = false;
    let mut p = start;
    while p <= at {
        let b = cpu.bus.guest_read_8(p);
        if cpu.bus.guest_faulted() {
            return None;
        }
        if b == 0 {
            return Some(false);
        }
        lead = !lead && is_lead(cpu, b);
        p += 1;
    }
    Some(lead)
}

/// _ismbslead(start, at): -1 if the byte at `at` leads a character.
pub fn ismbslead(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(if leads(cpu, args[0], args[1])? {
        u32::MAX
    } else {
        0
    })
}

/// _ismbstrail(start, at): -1 if it is the second byte of one.
pub fn ismbstrail(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[1] <= args[0] {
        return Some(0);
    }
    Some(if leads(cpu, args[0], args[1] - 1)? {
        u32::MAX
    } else {
        0
    })
}

/// _mbsbtype(s, index): what the byte at `index` is, past the string's
/// end _MBC_ILLEGAL.
pub fn mbsbtype(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mut lead = false;
    for i in 0..args[1] {
        let b = cpu.bus.guest_read_8(args[0] + i);
        if cpu.bus.guest_faulted() {
            return None;
        }
        if b == 0 {
            return Some(MBC_ILLEGAL);
        }
        lead = !lead && is_lead(cpu, b);
    }
    let b = cpu.bus.guest_read_8(args[0] + args[1]);
    if cpu.bus.guest_faulted() {
        return None;
    }
    Some(match (lead, is_trail(cpu, b), is_lead(cpu, b)) {
        (true, true, _) => MBC_TRAIL,
        (true, false, _) => MBC_ILLEGAL,
        (false, _, true) => MBC_LEAD,
        (false, _, false) => MBC_SINGLE,
    })
}

/// _mbsnextc(s): the character at s, a lead byte with what follows it.
pub fn mbsnextc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let b = bytes(cpu, args[0], 1)?[0];
    if is_lead(cpu, b) {
        let next = bytes(cpu, args[0] + 1, 1)?[0];
        return Some((b as u32) << 8 | next as u32);
    }
    Some(b as u32)
}

/// _mbclen(s): 2 for a lead byte, else 1.
pub fn mbclen(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let b = bytes(cpu, args[0], 1)?[0];
    Some(if is_lead(cpu, b) { 2 } else { 1 })
}

/// _mbsinc(s): the next character.
pub fn mbsinc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(args[0] + mbclen(cpu, args)?)
}

/// _mbsdec(start, s): the character before `s`, or NULL at the start.
pub fn mbsdec(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (start, at) = (args[0], args[1]);
    if at <= start {
        return Some(0);
    }
    let back = if at - 1 > start && leads(cpu, start, at - 2)? {
        2
    } else {
        1
    };
    Some(at - back)
}

/// The characters of the string at `at`: each one's bytes. A lead byte
/// at the end, with no byte after it, isn't one; `cut` says if there was.
fn chars(cpu: &mut Cpu, at: u32) -> Option<(Vec<Vec<u8>>, bool)> {
    let s = cstr(cpu, at)?;
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        if is_lead(cpu, s[i]) {
            if i + 1 >= s.len() {
                return Some((out, true));
            }
            out.push(s[i..i + 2].to_vec());
            i += 2;
        } else {
            out.push(vec![s[i]]);
            i += 1;
        }
    }
    Some((out, false))
}

/// _mbslen(s): its characters, a lone lead byte at the end not one.
pub fn mbslen(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(chars(cpu, args[0])?.0.len() as u32)
}

/// _mbsninc(s, count): `count` characters on, not past the end.
pub fn mbsninc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        return Some(0);
    }
    let (chars, _) = chars(cpu, args[0])?;
    let len: usize = chars.iter().take(args[1] as usize).map(Vec::len).sum();
    Some(args[0] + len as u32)
}

/// _mbccpy(to, from): one character.
pub fn mbccpy(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let n = mbclen(cpu, &[args[1]])?;
    let data = bytes(cpu, args[1], n)?;
    put(cpu, args[0], &data)?;
    Some(0)
}

/// _mbsncpy(to, from, count): `count` characters, then zeros to it. A
/// lone lead byte at the end is two zeros.
pub fn mbsncpy(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (to, from, mut n) = (args[0], args[1], args[2]);
    if n == 0 {
        return Some(to);
    }
    let (chars, cut) = chars(cpu, from)?;
    let mut out = Vec::new();
    for c in &chars {
        if n == 0 {
            break;
        }
        out.extend_from_slice(c);
        n -= 1;
    }
    if n > 0 && cut {
        out.extend([0, 0]);
        n -= 1;
    }
    out.extend(std::iter::repeat_n(0, n as usize));
    put(cpu, to, &out)?;
    Some(to)
}

/// _mbsnbcpy(to, from, count): `count` bytes, then zeros to it; a lead
/// byte cut from its trail byte at the end is made 0.
pub fn mbsnbcpy(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (to, from, n) = (args[0], args[1], args[2] as usize);
    if n == 0 {
        return Some(to);
    }
    if !cpu.bus.guest_probe(to, n, true) {
        return None;
    }
    let s = cstr_n(cpu, from, n)?;
    let mut lead = false;
    for &b in &s {
        lead = !lead && is_lead(cpu, b);
    }
    let mut out = s;
    if lead {
        *out.last_mut().unwrap() = 0;
    }
    out.resize(n, 0);
    put(cpu, to, &out)?;
    Some(to)
}

/// The characters of `s` that are in `set`, from its start.
fn span(cpu: &mut Cpu, s: u32, set: u32) -> Option<usize> {
    let (chars, _) = chars(cpu, s)?;
    let (set, _) = self::chars(cpu, set)?;
    Some(
        chars
            .iter()
            .take_while(|c| set.contains(c))
            .map(Vec::len)
            .sum(),
    )
}

/// _mbsspn(s, set): the bytes of the characters at its start that are in
/// the set.
pub fn mbsspn(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(span(cpu, args[0], args[1])? as u32)
}

/// _mbsspnp(s, set): where the characters not in the set start, or NULL.
pub fn mbsspnp(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let n = span(cpu, args[0], args[1])?;
    let at = args[0] + n as u32;
    Some(if bytes(cpu, at, 1)?[0] == 0 { 0 } else { at })
}

/// _mbschr(s, c): the first character c, which may be double-byte.
pub fn mbschr(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (chars, _) = chars(cpu, args[0])?;
    let mut at = args[0];
    for ch in &chars {
        if char_value(ch) == args[1] {
            return Some(at);
        }
        at += ch.len() as u32;
    }
    Some(if args[1] == 0 { at } else { 0 })
}

/// _mbsrchr(s, c): the last.
pub fn mbsrchr(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (chars, _) = chars(cpu, args[0])?;
    let mut at = args[0];
    let mut found = 0;
    for ch in &chars {
        if char_value(ch) == args[1] {
            found = at;
        }
        at += ch.len() as u32;
    }
    Some(if args[1] == 0 { at } else { found })
}

fn char_value(c: &[u8]) -> u32 {
    c.iter().fold(0, |v, &b| v << 8 | b as u32)
}

/// _mbcjistojms(c): JIS X 0208 to Shift JIS, in code page 932; 0 for a
/// character that isn't one. Elsewhere c as it is.
pub fn mbcjistojms(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mut c = args[0];
    if state(cpu, S_MBCP) != 932 {
        return Some(c);
    }
    let (hi, lo) = ((c >> 8) & 0xFF, c & 0xFF);
    if c > 0xFFFF || !(0x21..=0x7E).contains(&hi) || !(0x21..=0x7E).contains(&lo) {
        return Some(0);
    }
    c += if hi % 2 == 1 { 0x1F } else { 0x7D };
    if c & 0xFF >= 0x7F {
        c += 1;
    }
    c = (((c >> 8) - 0x21) / 2 + 0x81) << 8 | (c & 0xFF);
    if c >> 8 > 0x9F {
        c += 0x4000;
    }
    Some(c)
}

/// _mbcjmstojis(c): Shift JIS to JIS X 0208, in code page 932.
pub fn mbcjmstojis(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let c = args[0];
    if state(cpu, S_MBCP) != 932 {
        return Some(c);
    }
    let (mut hi, mut lo) = ((c >> 8) & 0xFF, c & 0xFF);
    if c > 0xFFFF || !is_lead(cpu, hi as u8) || !is_trail(cpu, lo as u8) {
        return Some(0);
    }
    if hi >= 0xE0 {
        hi -= 0x40;
    }
    hi = (hi - 0x81) * 2 + 0x21;
    if lo >= 0x9F {
        hi += 1;
        lo -= 0x7E;
    } else {
        if lo >= 0x80 {
            lo -= 1;
        }
        lo -= 0x1F;
    }
    Some(hi << 8 | lo)
}

/// The single-byte katakana of code page 932's katakana from 8340h (and
/// its hiragana from 829Fh, in the same order): voiced ones lose their
/// marks, small ones without a small form become large.
const KANA: [u8; 86] = [
    0xA7, 0xB1, 0xA8, 0xB2, 0xA9, 0xB3, 0xAA, 0xB4, 0xAB, 0xB5, 0xB6, 0xB6, 0xB7, 0xB7, 0xB8, 0xB8,
    0xB9, 0xB9, 0xBA, 0xBA, 0xBB, 0xBB, 0xBC, 0xBC, 0xBD, 0xBD, 0xBE, 0xBE, 0xBF, 0xBF, 0xC0, 0xC0,
    0xC1, 0xC1, 0xAF, 0xC2, 0xC2, 0xC3, 0xC3, 0xC4, 0xC4, 0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0xCA, 0xCA,
    0xCA, 0xCB, 0xCB, 0xCB, 0xCC, 0xCC, 0xCC, 0xCD, 0xCD, 0xCD, 0xCE, 0xCE, 0xCE, 0xCF, 0xD0, 0xD1,
    0xD2, 0xD3, 0xAC, 0xD4, 0xAD, 0xD5, 0xAE, 0xD6, 0xD7, 0xD8, 0xD9, 0xDA, 0xDB, 0xDC, 0xDC, 0xB2,
    0xB4, 0xA6, 0xDD, 0xB3, 0xB6, 0xB9,
];

/// The single-byte forms of code page 932's punctuation from 8140h to
/// 8197h; 0 for the ones without.
const PUNCT: [u8; 88] = [
    0x20, 0xA4, 0xA1, 0x2C, 0x2E, 0xA5, 0x3A, 0x3B, 0x3F, 0x21, 0xDE, 0xDF, 0x00, 0x60, 0x00, 0x5E,
    0x00, 0x5F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xB0, 0x00, 0x2D, 0x2F, 0x5C,
    0x00, 0x00, 0x7C, 0x00, 0x00, 0x00, 0x27, 0x00, 0x22, 0x28, 0x29, 0x00, 0x00, 0x5B, 0x5D, 0x7B,
    0x7D, 0x00, 0x00, 0x00, 0x00, 0xA2, 0xA3, 0x00, 0x00, 0x00, 0x00, 0x2B, 0x2D, 0x00, 0x00, 0x00,
    0x00, 0x3D, 0x00, 0x3C, 0x3E, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x5C,
    0x24, 0x00, 0x00, 0x25, 0x23, 0x26, 0x2A, 0x40,
];

/// _mbctombb(c): code page 932's double-byte kana, punctuation, digits
/// and letters as their single-byte forms; other characters, and other
/// code pages, as they are.
pub fn mbctombb(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let c = args[0];
    if state(cpu, S_MBCP) != 932 {
        return Some(c);
    }
    Some(match c {
        0x829F..=0x82F1 => KANA[(c - 0x829F) as usize] as u32,
        0x8340..=0x837E => KANA[(c - 0x8340) as usize] as u32,
        0x8380..=0x8396 => KANA[(c - 0x8341) as usize] as u32,
        0x8140..=0x8197 => match PUNCT[(c - 0x8140) as usize] {
            0 => c,
            b => b as u32,
        },
        0x824F..=0x8258 | 0x8260..=0x8279 => c - 0x821F,
        0x8281..=0x829A => c - 0x8220,
        _ => c,
    })
}

/// _mbbtombc(c): single-byte digits, letters and space as code page
/// 932's double-byte ones.
pub fn mbbtombc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let c = args[0];
    if state(cpu, S_MBCP) != 932 {
        return Some(c);
    }
    Some(match c {
        0x30..=0x39 | 0x41..=0x5A => c + 0x821F,
        0x61..=0x7A => c + 0x8220,
        0x20 => 0x8140,
        _ => c,
    })
}
