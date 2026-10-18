//! Code page 437, the text screen's characters: Rust strings to the bytes
//! the screen shows, and back for what is typed.

/// The characters of bytes 80h to FFh.
const HIGH: &str = "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜ¢£¥₧ƒáíóúñÑªº¿⌐¬½¼¡«»\
░▒▓│┤╡╢╖╕╣║╗╝╜╛┐└┴┬├─┼╞╟╚╔╩╦╠═╬╧╨╤╥╙╘╒╓╫╪┘┌█▄▌▐▀\
αßΓπΣσµτΦΘΩδ∞φε∩≡±≥≤⌠⌡÷≈°∙·√ⁿ²■\u{a0}";

/// The characters of bytes 01h to 1Fh that RC shows.
const LOW: [(u8, char); 8] = [
    (0x07, '•'),
    (0x10, '►'),
    (0x11, '◄'),
    (0x18, '↑'),
    (0x19, '↓'),
    (0x1A, '→'),
    (0x1B, '←'),
    (0x1E, '▲'),
];

/// The byte for a character, '?' for one the code page doesn't have.
pub fn byte(c: char) -> u8 {
    if (c as u32) < 0x80 {
        return c as u8;
    }
    if let Some(i) = HIGH.chars().position(|h| h == c) {
        return 0x80 + i as u8;
    }
    LOW.iter().find(|(_, l)| *l == c).map_or(b'?', |(b, _)| *b)
}

/// The bytes of a string.
pub fn bytes(s: &str) -> Vec<u8> {
    s.chars().map(byte).collect()
}

/// The character of a byte.
pub fn char_of(b: u8) -> char {
    if b < 0x80 {
        return LOW
            .iter()
            .find(|(l, _)| *l == b)
            .map_or(b as char, |(_, c)| *c);
    }
    HIGH.chars().nth(b as usize - 0x80).unwrap_or('?')
}

/// The string of bytes.
pub fn string(b: &[u8]) -> String {
    b.iter().map(|&b| char_of(b)).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn round_trips() {
        assert_eq!(super::HIGH.chars().count(), 128);
        for b in 0x80..=0xFFu8 {
            assert_eq!(super::byte(super::char_of(b)), b);
        }
        assert_eq!(super::bytes("╔═►"), vec![0xC9, 0xCD, 0x10]);
    }
}
