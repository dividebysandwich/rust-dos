//! Program information files (PIFs): what Windows starts for a `.PIF`
//! and how. A PIF names a DOS program, the folder it starts in and its
//! parameters, and holds the settings of the DOS machine it runs in.
//!
//! The layout, as Windows 3.x and Windows 98 write it and Wine's
//! winevdm reads it: a basic section of 171h bytes, from Windows 2.x,
//! with the title at 02h (30 bytes padded with blanks), the program at
//! 24h (63 bytes), its flags at 63h (10h: close the window on exit), the
//! working folder at 65h (64 bytes) and the parameters at A5h (64
//! bytes), each ended by a NUL. After it come records, each with a
//! header of 22 bytes: a name of 16 bytes ("MICROSOFT PIFEX" first),
//! the offset of the next header (FFFFh for none), the offset of its
//! data and the data's size. The "WINDOWS 386 3.0" record's data has
//! eleven words of memory and priority settings and flags, nine of the
//! hot key, and then parameters (64 bytes at 28h), which Windows in 386
//! enhanced mode uses in place of the basic section's. The "WINDOWS VMM
//! 4.0" record Windows 95 added holds the icon, the fonts, the
//! keyboard, mouse and other settings of the Properties sheet, and no
//! program, folder or parameters.

/// The basic section's size, and where the records start.
const BASIC: usize = 0x171;
const TITLE: usize = 0x02;
const TITLE_LEN: usize = 30;
const PROGRAM: usize = 0x24;
const PROGRAM_LEN: usize = 63;
const FLAGS: usize = 0x63;
const DIRECTORY: usize = 0x65;
const PARAMETERS: usize = 0xA5;
const STRING_LEN: usize = 64;
/// The flag of the basic section's that closes the window on exit.
const CLOSE_ON_EXIT: u8 = 0x10;

/// A record's header: its name, the next header's offset, its data's
/// offset and size.
const HEADER_LEN: usize = 22;
const NAME_LEN: usize = 16;
const LAST: u16 = 0xFFFF;
const FIRST_RECORD: &[u8] = b"MICROSOFT PIFEX";
const ENHANCED_RECORD: &[u8] = b"WINDOWS 386";
/// The 386 enhanced record's parameters.
const ENHANCED_PARAMETERS: usize = 0x28;

/// What a PIF says about the program it starts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pif {
    /// The window's title.
    pub title: String,
    /// The program, as the PIF names it: a path, or a name looked for.
    pub program: String,
    /// The folder it starts in; empty for the one it is started from.
    pub directory: String,
    /// Its parameters: the 386 enhanced record's, else the basic
    /// section's.
    pub parameters: String,
    /// Whether its window closes when it ends.
    pub close_on_exit: bool,
}

/// The NUL-ended string of at most `len` bytes at `at`.
fn string(data: &[u8], at: usize, len: usize) -> String {
    let field = &data[at..at + len];
    let end = field.iter().position(|&b| b == 0).unwrap_or(len);
    crate::dosstr::from_bytes(&field[..end]).trim().to_string()
}

/// The PIF `data`, or None if it isn't one: shorter than the basic
/// section, or with no program.
pub fn parse(data: &[u8]) -> Option<Pif> {
    if data.len() < BASIC {
        return None;
    }
    let mut pif = Pif {
        title: string(data, TITLE, TITLE_LEN),
        program: string(data, PROGRAM, PROGRAM_LEN),
        directory: string(data, DIRECTORY, STRING_LEN),
        parameters: string(data, PARAMETERS, STRING_LEN),
        close_on_exit: data[FLAGS] & CLOSE_ON_EXIT != 0,
    };
    if pif.program.is_empty() {
        return None;
    }
    if let Some(parameters) = enhanced_parameters(data) {
        pif.parameters = parameters;
    }
    Some(pif)
}

/// The 386 enhanced record's parameters, if the PIF has the record.
fn enhanced_parameters(data: &[u8]) -> Option<String> {
    let word = |at: usize| Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?));
    if !data.get(BASIC..)?.starts_with(FIRST_RECORD) {
        return None;
    }
    let mut at = BASIC;
    // Each record's header comes after the one before it, so a chain
    // that loops ends.
    let mut seen = 0;
    while seen < data.len() / HEADER_LEN {
        seen += 1;
        let name = data.get(at..at + NAME_LEN)?;
        let next = word(at + NAME_LEN)?;
        let start = word(at + NAME_LEN + 2)? as usize;
        let size = word(at + NAME_LEN + 4)? as usize;
        if name.starts_with(ENHANCED_RECORD) {
            if size < ENHANCED_PARAMETERS + STRING_LEN || data.len() < start + size {
                return None;
            }
            return Some(string(data, start + ENHANCED_PARAMETERS, STRING_LEN));
        }
        if next == LAST {
            return None;
        }
        at = next as usize;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PIF as Windows writes one: the basic section, the first record,
    /// the 386 enhanced record with `enhanced` as its parameters if
    /// given, and the Windows 95 record.
    fn pif(program: &str, directory: &str, parameters: &str, enhanced: Option<&str>) -> Vec<u8> {
        let mut data = vec![0u8; BASIC];
        data[TITLE..TITLE + TITLE_LEN].fill(b' ');
        data[TITLE..TITLE + 1].copy_from_slice(b"Z");
        data[PROGRAM..PROGRAM + program.len()].copy_from_slice(program.as_bytes());
        data[DIRECTORY..DIRECTORY + directory.len()].copy_from_slice(directory.as_bytes());
        data[PARAMETERS..PARAMETERS + parameters.len()].copy_from_slice(parameters.as_bytes());
        let header = |data: &mut Vec<u8>, name: &[u8], next: u16, start: u16, size: u16| {
            let mut h = [0u8; HEADER_LEN];
            h[..name.len()].copy_from_slice(name);
            h[16..18].copy_from_slice(&next.to_le_bytes());
            h[18..20].copy_from_slice(&start.to_le_bytes());
            h[20..22].copy_from_slice(&size.to_le_bytes());
            data.extend_from_slice(&h);
        };
        let enhanced_at = (BASIC + HEADER_LEN) as u16;
        let vmm_at = enhanced_at + HEADER_LEN as u16 + 0x68;
        header(&mut data, FIRST_RECORD, enhanced_at, 0, BASIC as u16);
        header(
            &mut data,
            b"WINDOWS 386 3.0",
            vmm_at,
            enhanced_at + 22,
            0x68,
        );
        let mut record = [0u8; 0x68];
        if let Some(p) = enhanced {
            record[ENHANCED_PARAMETERS..ENHANCED_PARAMETERS + p.len()]
                .copy_from_slice(p.as_bytes());
        }
        data.extend_from_slice(&record);
        header(&mut data, b"WINDOWS VMM 4.0", LAST, vmm_at + 22, 0x1AC);
        data.extend_from_slice(&[0u8; 0x1AC]);
        data
    }

    #[test]
    fn reads_program_folder_and_parameters() {
        let data = pif("C:\\GAME\\GAME.EXE", "C:\\GAME", "/fast", None);
        let pif = parse(&data).unwrap();
        assert_eq!(pif.title, "Z");
        assert_eq!(pif.program, "C:\\GAME\\GAME.EXE");
        assert_eq!(pif.directory, "C:\\GAME");
        assert_eq!(pif.parameters, "");
        assert!(!pif.close_on_exit);
    }

    #[test]
    fn enhanced_parameters_take_the_basic_ones_place() {
        let data = pif("GAME.EXE", "", "/basic", Some("/enhanced"));
        assert_eq!(parse(&data).unwrap().parameters, "/enhanced");
    }

    #[test]
    fn basic_parameters_without_an_enhanced_record() {
        let mut data = pif("GAME.EXE", "", "/basic", None);
        data.truncate(BASIC);
        assert_eq!(parse(&data).unwrap().parameters, "/basic");
    }

    /// What follows a field's NUL is left from an earlier value, as in
    /// the PIFs Z (1996)'s setup writes: "z.bat", then "EST\ZED.EXE".
    #[test]
    fn strings_end_at_their_nul() {
        let data = pif(
            "z.bat\0EST\\ZED.EXE",
            "\0:\\Z_TEST",
            "\0cd:d",
            Some("\0cd:d"),
        );
        let pif = parse(&data).unwrap();
        assert_eq!(pif.program, "z.bat");
        assert_eq!(pif.directory, "");
        assert_eq!(pif.parameters, "");
    }

    #[test]
    fn not_a_pif() {
        assert_eq!(parse(&[0u8; 0x100]), None);
        assert_eq!(parse(&[0u8; BASIC]), None);
    }
}
