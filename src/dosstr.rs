//! Text on the DOS side of the shell: code page 437 bytes, carried in Rust
//! strings one char per byte (U+0000 to U+00FF), so that every byte comes
//! back as it was: `video::print_string` prints `c as u8`, and `to_bytes`
//! turns the string back into the bytes a program reads. Command lines,
//! batch file lines and the environment are held this way, so a batch file
//! drawing a menu with box characters, or an ECHO of an umlaut, shows them
//! as DOS would.

/// The string holding these bytes, one char each.
pub fn from_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

/// The bytes a string made by `from_bytes` holds. Chars above U+00FF,
/// which only text from the Rust side can have, become '?'.
pub fn to_bytes(s: &str) -> Vec<u8> {
    s.chars().map(|c| u8::try_from(c as u32).unwrap_or(b'?')).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_survive_the_round_trip() {
        let bytes: Vec<u8> = (0..=255).collect();
        let text = from_bytes(&bytes);
        assert_eq!(text.chars().count(), 256);
        assert_eq!(to_bytes(&text), bytes);
        assert_eq!(to_bytes("a€b"), b"a?b");
    }
}
