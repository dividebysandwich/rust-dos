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

/// Text from the host, a configuration's `[autoexec]` say, as the code
/// page 437 bytes that show it: ASCII as itself, control characters
/// included, and every other character as the byte of its glyph. The
/// ECHO of a box drawn with `╔═╗` prints the bytes C9h CDh BBh. A Latin-1
/// letter code page 437 lacks (ø, Â) becomes its Latin-1 byte, which a
/// host path given to MOUNT or BOOT is read back from
/// (`mount::parse_dos_mount_tokens`); any other character becomes '?'.
pub fn encode_cp437(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| {
            if c.is_ascii() {
                c as u8
            } else {
                crate::keylayout::cp437(c)
                    .or_else(|| u8::try_from(c as u32).ok())
                    .unwrap_or(b'?')
            }
        })
        .collect()
}

/// The text of a host file that DOS programs read as code page 437 bytes,
/// such as a DOSBox configuration, which DOSBox runs the `[autoexec]` of
/// byte for byte: the file as UTF-8 where it is valid UTF-8, else each
/// byte as the character code page 437 shows (ASCII as itself, control
/// characters included). `encode_cp437` gives the bytes back.
pub fn decode_cp437(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => bytes
            .iter()
            .map(|&b| {
                if b.is_ascii() {
                    b as char
                } else {
                    crate::video::CP437[b as usize]
                }
            })
            .collect(),
    }
}

/// A DOS string as host text, for the host paths given to MOUNT and BOOT:
/// each byte above 7Fh as the character code page 437 shows for it, so
/// `D:\Über` typed at the prompt (9Ah for the Ü) or queued from a
/// configuration (`encode_cp437`) names the folder `Über`.
pub fn host_text(s: &str) -> String {
    s.chars()
        .map(|c| {
            if (0x80..=0xFF).contains(&(c as u32)) {
                crate::video::CP437[c as usize]
            } else {
                c
            }
        })
        .collect()
}

/// `host_text` undone: host text as the DOS string of its code page 437
/// bytes, for a DOS path among the words of `host_text`.
pub fn from_host_text(s: &str) -> String {
    from_bytes(&encode_cp437(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_page_437_files_come_back_as_their_bytes() {
        // GOG's launcher menus: a double-line box, and ANSI colours.
        let bytes = b"ECHO \x1B[1;33m\xC9\xCD\xBB \x84\x1B[0m\r\n";
        let text = decode_cp437(bytes);
        assert_eq!(text, "ECHO \u{1B}[1;33m╔═╗ ä\u{1B}[0m\r\n");
        assert_eq!(encode_cp437(&text), bytes);
        // A file in UTF-8 is read as UTF-8.
        assert_eq!(decode_cp437("ECHO ╔═╗ ä".as_bytes()), "ECHO ╔═╗ ä");
        assert_eq!(encode_cp437("é€"), b"\x82?");
        // ø has no glyph in code page 437: its Latin-1 byte.
        assert_eq!(encode_cp437("Bjørn"), b"Bj\xF8rn");
        // A host path among DOS text, and back.
        let dos = from_bytes(b"D:\\\x9Aber");
        assert_eq!(host_text(&dos), "D:\\Über");
        assert_eq!(from_host_text(&host_text(&dos)), dos);
    }

    #[test]
    fn bytes_survive_the_round_trip() {
        let bytes: Vec<u8> = (0..=255).collect();
        let text = from_bytes(&bytes);
        assert_eq!(text.chars().count(), 256);
        assert_eq!(to_bytes(&text), bytes);
        assert_eq!(to_bytes("a€b"), b"a?b");
    }
}
