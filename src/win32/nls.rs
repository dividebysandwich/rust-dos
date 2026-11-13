//! National language support: the code pages and locales of the built-in
//! Windows, and the system locale it runs under.
//!
//! The system locale is the `win32_locale` setting, the localized
//! Windows 98 the built-in Windows is: English (United States) by
//! default, or Russian, Polish, Czech, Hungarian, Greek, Turkish or
//! Lithuanian. One table (`LOCALES`) says what each of them is: its
//! locale id, its ANSI and OEM code pages (CP_ACP and CP_OEMCP, GetACP,
//! GetOEMCP), the character set of the system's fonts and of
//! DEFAULT_CHARSET, its keyboard layout's name, and GetLocaleInfo's
//! information and formats (locales.rs). Every function that converts
//! between the system's code pages and Unicode takes them from here.
//!
//! As with `win32_version`, the setting is read when the first program
//! starts (`start`), the processes see that locale until the last of them
//! ends, and saved states keep it with the rest of the built-in Windows,
//! so a state goes on under the locale it was saved with.
//!
//! The code pages are those of the locales here, ANSI and OEM, and 850
//! and 855, which the US and Russian Windows 98 install too: their tables
//! are generated from the Unicode Consortium's mapping files (tables.rs).
//! Every one of them is installed under every locale, as on a Windows 98
//! with multilanguage support, so a program can convert text of any of
//! them. The double-byte code pages of the East Asian Windows aren't
//! here.

pub mod locales;
pub mod tables;

use crate::cpu::Cpu;

pub use crate::config::Win32Locale;
pub use locales::Info;

crate::state_enum!(Win32Locale {
    Win32Locale::EnUs,
    Win32Locale::RuRu,
    Win32Locale::PlPl,
    Win32Locale::CsCz,
    Win32Locale::HuHu,
    Win32Locale::ElGr,
    Win32Locale::TrTr,
    Win32Locale::LtLt,
});

/// A code page of one byte a character: its number, the characters of
/// its bytes from 80h on, and whether it is a DOS (OEM) code page, whose
/// control characters have glyphs (MB_USEGLYPHCHARS).
#[derive(Clone, Copy)]
pub struct CodePage {
    number: u16,
    high: &'static [u16; 128],
    oem: bool,
}

impl PartialEq for CodePage {
    fn eq(&self, other: &Self) -> bool {
        self.number == other.number
    }
}

impl Eq for CodePage {}

impl std::fmt::Debug for CodePage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CodePage({})", self.number)
    }
}

const fn page(number: u16, high: &'static [u16; 128], oem: bool) -> CodePage {
    CodePage { number, high, oem }
}

/// The code pages there are.
const CODE_PAGES: [CodePage; 15] = [
    page(1252, &tables::CP1252, false),
    page(1250, &tables::CP1250, false),
    page(1251, &tables::CP1251, false),
    page(1253, &tables::CP1253, false),
    page(1254, &tables::CP1254, false),
    page(1257, &tables::CP1257, false),
    page(437, &tables::CP437, true),
    page(737, &tables::CP737, true),
    page(775, &tables::CP775, true),
    page(850, &tables::CP850, true),
    page(852, &tables::CP852, true),
    page(855, &tables::CP855, true),
    page(857, &tables::CP857, true),
    page(866, &tables::CP866, true),
    page(869, &tables::CP869, true),
];

/// CP_ACP, CP_OEMCP, CP_MACCP and CP_THREAD_ACP, which name the system's
/// code pages.
pub const CP_ACP: u32 = 0;
pub const CP_OEMCP: u32 = 1;
pub const CP_MACCP: u32 = 2;
pub const CP_THREAD_ACP: u32 = 3;

impl CodePage {
    /// US Windows' ANSI code page, 1252.
    pub const WESTERN: CodePage = CODE_PAGES[0];

    /// US Windows' OEM code page, 437: the VGA ROM font's.
    pub const US_OEM: CodePage = CODE_PAGES[6];

    /// The code page numbered `number`, if there is one.
    pub fn by_number(number: u32) -> Option<CodePage> {
        CODE_PAGES.into_iter().find(|p| p.number as u32 == number)
    }

    pub fn number(self) -> u32 {
        self.number as u32
    }

    /// Whether it is a DOS (OEM) code page.
    pub fn is_oem(self) -> bool {
        self.oem
    }

    /// The character of `byte`. In the OEM code pages the control
    /// characters stand for themselves, as Windows converts them unless
    /// asked for their glyphs (`glyph`).
    pub fn char(self, byte: u8) -> u16 {
        match byte {
            0..=0x7F => byte as u16,
            _ => self.high[byte as usize - 0x80],
        }
    }

    /// The character of `byte` as the screen shows it: in an OEM code
    /// page the control characters' glyphs (MB_USEGLYPHCHARS), which are
    /// code page 437's in every one.
    pub fn glyph(self, byte: u8) -> u16 {
        match byte {
            0..=0x1F | 0x7F if self.oem => crate::video::CP437[byte as usize] as u16,
            _ => self.char(byte),
        }
    }

    /// The byte of the character `c`, if the code page has it. The OEM
    /// code pages take the control characters' glyphs too.
    pub fn byte(self, c: u16) -> Option<u8> {
        if c < 0x80 {
            return Some(c as u8);
        }
        if let Some(i) = self.high.iter().position(|&x| x == c) {
            return Some(0x80 + i as u8);
        }
        if !self.oem {
            return None;
        }
        (0..0x20u8)
            .chain([0x7F])
            .find(|&b| crate::video::CP437[b as usize] as u16 == c)
    }

    /// The byte of `c`, or the best fit Windows gives a character the
    /// code page lacks: an accented letter's bare letter (ä is 'a' in
    /// code page 1251), as Windows 95 does already.
    pub fn best_fit(self, c: u16) -> Option<u8> {
        let mut c = c;
        loop {
            if let Some(b) = self.byte(c) {
                return Some(b);
            }
            c = super::convert::decompose(c)?.0;
        }
    }

    /// `bytes` as UTF-16.
    pub fn decode(self, bytes: &[u8]) -> Vec<u16> {
        bytes.iter().map(|&b| self.char(b)).collect()
    }

    /// `units` (UTF-16) in the code page, a character it lacks becoming
    /// '?'.
    pub fn encode(self, units: &[u16]) -> Vec<u8> {
        units
            .iter()
            .map(|&c| self.byte(c).unwrap_or(b'?'))
            .collect()
    }
}

/// A Windows character set (a LOGFONT's lfCharSet): its number, the code
/// page its fonts' bytes are in, its bit of a FONTSIGNATURE's code pages
/// (fsCsb[0]), and the name of its script (ENUMLOGFONTEX's elfScript).
#[derive(Clone, Copy, Debug)]
pub struct Charset {
    pub charset: u8,
    pub code_page: u32,
    pub csb: u32,
    pub script: &'static str,
}

pub const ANSI_CHARSET: u8 = 0;
pub const DEFAULT_CHARSET: u8 = 1;
pub const SYMBOL_CHARSET: u8 = 2;
pub const EASTEUROPE_CHARSET: u8 = 238;
pub const RUSSIAN_CHARSET: u8 = 204;
pub const GREEK_CHARSET: u8 = 161;
pub const TURKISH_CHARSET: u8 = 162;
pub const BALTIC_CHARSET: u8 = 186;
pub const OEM_CHARSET: u8 = 255;

const fn set(charset: u8, code_page: u32, csb: u32, script: &'static str) -> Charset {
    Charset {
        charset,
        code_page,
        csb,
        script,
    }
}

/// The character sets Windows 98 knows, as TranslateCharsetInfo has them.
pub const CHARSETS: &[Charset] = &[
    set(ANSI_CHARSET, 1252, 0x1, "Western"),
    set(EASTEUROPE_CHARSET, 1250, 0x2, "Central European"),
    set(RUSSIAN_CHARSET, 1251, 0x4, "Cyrillic"),
    set(GREEK_CHARSET, 1253, 0x8, "Greek"),
    set(TURKISH_CHARSET, 1254, 0x10, "Turkish"),
    set(177, 1255, 0x20, "Hebrew"),
    set(178, 1256, 0x40, "Arabic"),
    set(BALTIC_CHARSET, 1257, 0x80, "Baltic"),
    set(222, 874, 0x1_0000, "Thai"),
    set(128, 932, 0x2_0000, "Japanese"),
    set(134, 936, 0x4_0000, "CHINESE_GB2312"),
    set(129, 949, 0x8_0000, "Hangul"),
    set(136, 950, 0x10_0000, "CHINESE_BIG5"),
    set(SYMBOL_CHARSET, 42, 0x8000_0000, "Symbol"),
];

/// The character set numbered `charset`, if Windows has one.
pub fn charset(charset: u8) -> Option<&'static Charset> {
    CHARSETS.iter().find(|c| c.charset == charset)
}

/// A locale of the built-in Windows: its id, its code pages, the
/// character set of its code page, its country's code and its
/// information (GetLocaleInfo's).
pub struct Locale {
    pub id: Win32Locale,
    pub lcid: u32,
    pub ansi: u32,
    pub oem: u32,
    pub mac: u32,
    pub charset: u8,
    pub country: u32,
    pub info: &'static Info,
}

impl Locale {
    pub fn ansi(&self) -> CodePage {
        CodePage::by_number(self.ansi).expect("a locale's ANSI code page")
    }

    pub fn oem(&self) -> CodePage {
        CodePage::by_number(self.oem).expect("a locale's OEM code page")
    }

    /// Its language id (the locale id's low word, the sort id's high bits
    /// being 0 here).
    pub fn lang_id(&self) -> u32 {
        self.lcid & 0xFFFF
    }
}

const fn locale(
    id: Win32Locale,
    lcid: u32,
    (ansi, oem, mac): (u32, u32, u32),
    charset: u8,
    country: u32,
    info: &'static Info,
) -> Locale {
    Locale {
        id,
        lcid,
        ansi,
        oem,
        mac,
        charset,
        country,
        info,
    }
}

/// The locales, the default first.
pub const LOCALES: [Locale; 8] = [
    locale(
        Win32Locale::EnUs,
        0x0409,
        (1252, 437, 10000),
        ANSI_CHARSET,
        1,
        &locales::EN_US,
    ),
    locale(
        Win32Locale::RuRu,
        0x0419,
        (1251, 866, 10007),
        RUSSIAN_CHARSET,
        7,
        &locales::RU_RU,
    ),
    locale(
        Win32Locale::PlPl,
        0x0415,
        (1250, 852, 10029),
        EASTEUROPE_CHARSET,
        48,
        &locales::PL_PL,
    ),
    locale(
        Win32Locale::CsCz,
        0x0405,
        (1250, 852, 10029),
        EASTEUROPE_CHARSET,
        42,
        &locales::CS_CZ,
    ),
    locale(
        Win32Locale::HuHu,
        0x040E,
        (1250, 852, 10029),
        EASTEUROPE_CHARSET,
        36,
        &locales::HU_HU,
    ),
    locale(
        Win32Locale::ElGr,
        0x0408,
        (1253, 737, 10006),
        GREEK_CHARSET,
        30,
        &locales::EL_GR,
    ),
    locale(
        Win32Locale::TrTr,
        0x041F,
        (1254, 857, 10081),
        TURKISH_CHARSET,
        90,
        &locales::TR_TR,
    ),
    locale(
        Win32Locale::LtLt,
        0x0427,
        (1257, 775, 10029),
        BALTIC_CHARSET,
        370,
        &locales::LT_LT,
    ),
];

/// The locale of a `win32_locale` value.
pub fn of(id: Win32Locale) -> &'static Locale {
    LOCALES
        .iter()
        .find(|l| l.id == id)
        .expect("every locale has its table")
}

/// The locale whose id is `lcid`, if it is one here.
pub fn by_lcid(lcid: u32) -> Option<&'static Locale> {
    LOCALES.iter().find(|l| l.lcid == lcid)
}

/// The system locale the running programs see.
pub fn current(cpu: &Cpu) -> &'static Locale {
    of(cpu.bus.win32.os_locale)
}

/// The system's ANSI code page (CP_ACP).
pub fn ansi(cpu: &Cpu) -> CodePage {
    current(cpu).ansi()
}

/// The system's OEM code page (CP_OEMCP).
pub fn oem(cpu: &Cpu) -> CodePage {
    current(cpu).oem()
}

/// The code page a conversion function's `number` names: CP_ACP,
/// CP_THREAD_ACP and CP_OEMCP the system's, or a number. None for one
/// there isn't, CP_MACCP among them.
pub fn code_page(cpu: &Cpu, number: u32) -> Option<CodePage> {
    match number {
        CP_ACP | CP_THREAD_ACP => Some(ansi(cpu)),
        CP_OEMCP => Some(oem(cpu)),
        CP_MACCP => None,
        _ => CodePage::by_number(number),
    }
}

/// The locale ids that stand for others, and the flag of the A
/// functions that takes the system's code page for a locale's.
pub const LOCALE_NEUTRAL: u32 = 0;
pub const LOCALE_USER_DEFAULT: u32 = 0x0400;
pub const LOCALE_SYSTEM_DEFAULT: u32 = 0x0800;
pub const LOCALE_USE_CP_ACP: u32 = 0x4000_0000;

/// The locale `lcid` stands for, as ConvertDefaultLocale says: the system
/// locale for the user's and the system's defaults and the neutral one,
/// SUBLANG_DEFAULT for a language's neutral sublanguage.
pub fn convert_default(cpu: &Cpu, lcid: u32) -> u32 {
    match lcid {
        LOCALE_SYSTEM_DEFAULT | LOCALE_USER_DEFAULT | LOCALE_NEUTRAL => current(cpu).lcid,
        _ if lcid >> 10 & 0x3F == 0 => lcid & !0xFC00 | 1 << 10,
        _ => lcid,
    }
}

/// The locale `lcid` names, if it is one here.
pub fn locale_of(cpu: &Cpu, lcid: u32) -> Option<&'static Locale> {
    by_lcid(convert_default(cpu, lcid))
}

/// The code page an A function given the locale `lcid` and `flags` reads
/// and writes its text in: the locale's ANSI code page, or the system's
/// with LOCALE_USE_CP_ACP or for a locale that isn't here.
pub fn locale_code_page(cpu: &Cpu, lcid: u32, flags: u32) -> CodePage {
    match locale_of(cpu, lcid) {
        Some(l) if flags & LOCALE_USE_CP_ACP == 0 => l.ansi(),
        _ => ansi(cpu),
    }
}

/// The first program starts: the setting becomes the locale the
/// processes see.
pub fn start(win32: &mut super::Win32) {
    if win32.processes.is_empty() {
        win32.os_locale = win32.locale;
    }
}

/// The registry's values of the locale `id`, as a .reg file: the code
/// pages and locale of HKLM\System\CurrentControlSet\Control\Nls, and
/// the user's locale. The keyboard layout stays the US one, which is what
/// the keys type (GetKeyboardLayout).
pub fn registry_values(id: Win32Locale) -> String {
    let l = of(id);
    let klid = format!("{:08X}", l.lcid);
    format!(
        "REGEDIT4\r\n\r\n\
         [HKEY_LOCAL_MACHINE\\System\\CurrentControlSet\\Control\\Nls\\CodePage]\r\n\
         \"ACP\"=\"{}\"\r\n\"OEMCP\"=\"{}\"\r\n\"MACCP\"=\"{}\"\r\n\r\n\
         [HKEY_LOCAL_MACHINE\\System\\CurrentControlSet\\Control\\Nls\\Locale]\r\n\
         @=\"{klid}\"\r\n\r\n\
         [HKEY_USERS\\.Default\\Control Panel\\International]\r\n\
         \"Locale\"=\"{klid}\"\r\n\r\n\
         [HKEY_USERS\\.Default\\Keyboard Layout\\Preload\\1]\r\n\
         @=\"00000409\"\r\n\r\n",
        l.ansi, l.oem, l.mac,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_page_437_is_the_vga_fonts() {
        let p = CodePage::by_number(437).unwrap();
        // All but FFh, which the VGA font draws as a space and Windows
        // reads as the no-break space.
        for b in 0x80..=0xFEu8 {
            assert_eq!(p.char(b), crate::video::CP437[b as usize] as u16, "{b:#X}");
        }
        assert_eq!(p.char(0xFF), 0xA0);
        // The control characters' glyphs, and back.
        assert_eq!(p.glyph(1), 0x263A);
        assert_eq!(p.char(1), 1);
        assert_eq!(p.byte(0x263A), Some(1));
    }

    #[test]
    fn code_pages_go_there_and_back() {
        for p in CODE_PAGES {
            for b in 0x80..=0xFFu8 {
                assert_eq!(p.byte(p.char(b)), Some(b), "{p:?} {b:#X}");
            }
        }
    }

    #[test]
    fn cyrillic_in_1251_and_866() {
        let ansi = CodePage::by_number(1251).unwrap();
        let oem = CodePage::by_number(866).unwrap();
        // "Привет" in each.
        let text: Vec<u16> = "Привет".encode_utf16().collect();
        assert_eq!(ansi.encode(&text), [0xCF, 0xF0, 0xE8, 0xE2, 0xE5, 0xF2]);
        assert_eq!(oem.encode(&text), [0x8F, 0xE0, 0xA8, 0xA2, 0xA5, 0xE2]);
        assert_eq!(
            ansi.decode(&[0xA8, 0xB8]),
            "Ёё".encode_utf16().collect::<Vec<_>>()
        );
    }

    #[test]
    fn every_locale_has_its_code_pages_and_charset() {
        for l in &LOCALES {
            assert_eq!(charset(l.charset).unwrap().code_page, l.ansi, "{:?}", l.id);
            assert!(l.oem().is_oem() && !l.ansi().is_oem());
            assert_eq!(by_lcid(l.lcid).unwrap().id, l.id);
        }
    }
}
