//! Where the outlines of Arial, Times New Roman and Courier New come
//! from, as Wine finds its fonts: the real fonts where they are there,
//! else the Liberation fonts that ship with the emulator. The setting
//! `win32_fonts` picks the places:
//!
//! - `auto` (the default): the emulated C:'s `C:\WINDOWS\FONTS`, where a
//!   game or the user may have put Windows' own files, then the host's
//!   fonts folder on Windows (`%WINDIR%\Fonts`) and macOS
//!   (`/System/Library/Fonts/Supplemental`, `/Library/Fonts`). The web
//!   and libretro builds, and the program on other systems, look on C:
//!   alone.
//! - `liberation`: the Liberation fonts alone.
//! - a host folder: that folder's files.
//!
//! Each of the twelve fonts (each face regular, bold, italic and bold
//! italic) is looked for on its own, by its file's name as Windows and
//! macOS have it, in any case. One that isn't found, isn't a TrueType
//! font GDI can draw, or is of another family or style, is drawn from
//! the Liberation font of its style, so a folder with only arial.ttf in
//! it gives the regular Arial and nothing else. Whatever draws a
//! character, it moves on by Windows 98's width (`truetype.bin`), so
//! text takes the same room with every font.
//!
//! A deterministic run doesn't read the host's fonts folder, which would
//! make its pictures depend on the machine it runs on. The emulated C:
//! and a folder the configuration names are inputs of the run like the
//! game's own files. A machine made without settings, as the tests make
//! them, draws with Liberation alone.
//!
//! The fonts are looked for the first time GDI draws or measures text in
//! one of the faces: a DOS program, or a Windows installed on the
//! emulated machine, never reads a font file. A file read from outside
//! is untrusted: it goes through the parser that reads a program's own
//! font files (`Ttf::parse`), and one it refuses falls back.

use std::cell::OnceCell;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::config::Win32Fonts;
use crate::cpu::Cpu;

use super::truetype;
use super::ttf::Ttf;

/// The largest font file read, in bytes. Windows 98's are under 400 KB;
/// modern Windows' Arial is about 1 MB.
const MOST_BYTES: u64 = 32 << 20;

/// The fonts' file names on Windows, in `truetype::style`'s order:
/// 8.3 names, which the emulated C: finds too.
const WINDOWS_NAMES: [&str; 12] = [
    "arial.ttf",
    "arialbd.ttf",
    "ariali.ttf",
    "arialbi.ttf",
    "times.ttf",
    "timesbd.ttf",
    "timesi.ttf",
    "timesbi.ttf",
    "cour.ttf",
    "courbd.ttf",
    "couri.ttf",
    "courbi.ttf",
];

/// The same files' names on macOS.
const MAC_NAMES: [&str; 12] = [
    "Arial.ttf",
    "Arial Bold.ttf",
    "Arial Italic.ttf",
    "Arial Bold Italic.ttf",
    "Times New Roman.ttf",
    "Times New Roman Bold.ttf",
    "Times New Roman Italic.ttf",
    "Times New Roman Bold Italic.ttf",
    "Courier New.ttf",
    "Courier New Bold.ttf",
    "Courier New Italic.ttf",
    "Courier New Bold Italic.ttf",
];

/// The families' names, as a font file's name table has them.
const FAMILIES: [&str; 3] = ["Arial", "Times New Roman", "Courier New"];

/// The outlines one of the twelve fonts is drawn with.
pub struct Outline {
    pub ttf: Ttf,
    /// 0 for the Liberation font, else a hash of the file's bytes: the
    /// glyphs drawn from it are kept by this (`font::Key`).
    pub id: u64,
    /// Where it was read: the file's path, or "Liberation".
    pub from: String,
}

impl std::fmt::Debug for Outline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Outline({})", self.from)
    }
}

/// The machine's setting, and the fonts read for it so far.
#[derive(Debug)]
pub struct Outlines {
    setting: Win32Fonts,
    fonts: [Option<Rc<Outline>>; 12],
}

impl Default for Outlines {
    /// Liberation alone: a machine set up from settings gets theirs
    /// (`set`).
    fn default() -> Self {
        Self {
            setting: Win32Fonts::Liberation,
            fonts: Default::default(),
        }
    }
}

impl Outlines {
    /// Take the setting `setting`. A change forgets the fonts read, which
    /// are looked for again as they are next drawn with.
    pub fn set(&mut self, setting: &Win32Fonts) {
        if *setting != self.setting {
            *self = Self {
                setting: setting.clone(),
                fonts: Default::default(),
            };
        }
    }
}

thread_local! {
    /// The Liberation fonts, each read when first drawn with.
    static LIBERATION: [OnceCell<Option<Rc<Outline>>>; 12] =
        const { [const { OnceCell::new() }; 12] };
}

/// The Liberation font of the style `style`; None only if the bundled
/// file couldn't be read, which a unit test rules out.
pub fn liberation(style: usize) -> Option<Rc<Outline>> {
    LIBERATION.with(|fonts| {
        fonts
            .get(style)?
            .get_or_init(|| {
                let ttf = Ttf::parse(truetype::outlines(style).to_vec(), 0)?;
                Some(Rc::new(Outline {
                    ttf,
                    id: 0,
                    from: "Liberation".to_string(),
                }))
            })
            .clone()
    })
}

/// The outlines the font `style` is drawn with on this machine, looked
/// for the first time it is asked for. The log gets a line naming the
/// file.
pub fn of(cpu: &mut Cpu, style: usize) -> Option<Rc<Outline>> {
    if let Some(o) = cpu.bus.win32.outlines.fonts.get(style)?.clone() {
        return Some(o);
    }
    let found = find(cpu, style);
    let o = found.or_else(|| liberation(style))?;
    let name = FAMILIES[style / 4];
    let kind = ["", " Bold", " Italic", " Bold Italic"][style % 4];
    let line = format!("[W32] GDI: {name}{kind} is drawn from {}", o.from);
    cpu.bus.log_string(&line);
    cpu.bus.win32.outlines.fonts[style] = Some(o.clone());
    Some(o)
}

/// The font `style` from the places the setting names, the first that
/// has a file GDI can draw of its family and style.
fn find(cpu: &mut Cpu, style: usize) -> Option<Rc<Outline>> {
    match cpu.bus.win32.outlines.setting.clone() {
        Win32Fonts::Liberation => None,
        Win32Fonts::Folder(dir) => from_host(&dir, style),
        Win32Fonts::Auto => from_drive(cpu, style).or_else(|| {
            if crate::deterministic::active() {
                return None;
            }
            host_folders().iter().find_map(|dir| from_host(dir, style))
        }),
    }
}

/// The font `style` from the emulated C:'s fonts folder.
fn from_drive(cpu: &mut Cpu, style: usize) -> Option<Rc<Outline>> {
    use crate::win32::kernel32::path::WINDOWS;
    let path = format!("{WINDOWS}\\FONTS\\{}", WINDOWS_NAMES[style].to_uppercase());
    let bytes = cpu.bus.disk.file_data(&path)?.read().ok()?.into_owned();
    if bytes.len() as u64 > MOST_BYTES {
        return None;
    }
    outline(style, bytes, path)
}

/// The host's fonts folders `auto` looks in after the emulated C:, in
/// that order: none but on the program for Windows and macOS.
fn host_folders() -> Vec<PathBuf> {
    #[cfg(all(feature = "sdl", windows))]
    {
        let windir = std::env::var_os("WINDIR")
            .or_else(|| std::env::var_os("SystemRoot"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("C:\\Windows"));
        vec![windir.join("Fonts")]
    }
    #[cfg(all(feature = "sdl", target_os = "macos"))]
    {
        vec![
            PathBuf::from("/System/Library/Fonts/Supplemental"),
            PathBuf::from("/Library/Fonts"),
        ]
    }
    #[cfg(not(all(feature = "sdl", any(windows, target_os = "macos"))))]
    {
        Vec::new()
    }
}

/// The font `style` from the host folder `dir`: the file of its Windows
/// or its macOS name, in any case.
fn from_host(dir: &Path, style: usize) -> Option<Rc<Outline>> {
    let names = [WINDOWS_NAMES[style], MAC_NAMES[style]];
    let entries = std::fs::read_dir(dir).ok()?;
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            names.iter().any(|n| n.eq_ignore_ascii_case(&name))
        })
        .map(|e| e.path())
        .collect();
    // The Windows name first, as the order of `names` has it.
    paths.sort_by_key(|p| {
        let name = p.file_name().map(|n| n.to_string_lossy().to_lowercase());
        name.as_deref() != Some(WINDOWS_NAMES[style])
    });
    paths.into_iter().find_map(|path| {
        let meta = std::fs::metadata(&path).ok()?;
        if !meta.is_file() || meta.len() > MOST_BYTES {
            return None;
        }
        let bytes = std::fs::read(&path).ok()?;
        outline(style, bytes, path.display().to_string())
    })
}

/// The font file `bytes`, read at `from`, as the outlines of the font
/// `style`: None unless it is a TrueType font GDI can draw, of the
/// style's family, bold and italic as the style is.
fn outline(style: usize, bytes: Vec<u8>, from: String) -> Option<Rc<Outline>> {
    let mut hasher = std::hash::DefaultHasher::new();
    bytes.hash(&mut hasher);
    let id = hasher.finish() | 1;
    let ttf = Ttf::parse(bytes, 0)?;
    let (bold, italic) = (style & 1 != 0, style & 2 != 0);
    if !ttf.face.eq_ignore_ascii_case(FAMILIES[style / 4])
        || (ttf.weight >= 600) != bold
        || ttf.italic != italic
    {
        return None;
    }
    Some(Rc::new(Outline { ttf, id, from }))
}


#[cfg(test)]
mod tests {
    use super::super::ttf::{Metrics, SizeKey, sample};
    use super::*;

    /// A folder of the host for a test, made anew with `files`.
    fn folder(name: &str, files: &[(&str, &[u8])]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rust-dos-outlines-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (file, bytes) in files {
            let path = dir.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        dir
    }

    #[test]
    fn every_liberation_font_reads_in_its_style() {
        for style in 0..12 {
            let o = liberation(style).unwrap();
            assert_eq!(o.id, 0);
            assert_eq!(o.ttf.weight >= 600, style & 1 != 0, "{style}");
            assert_eq!(o.ttf.italic, style & 2 != 0, "{style}");
        }
    }

    /// A folder with files for some fonts: its regular Arial, in
    /// capitals, is read; a bold Arial that is a regular font, a Times
    /// New Roman that isn't a font, and the fonts with no file, aren't.
    #[test]
    fn each_file_of_a_folder_is_taken_or_left_on_its_own() {
        let arial = sample("Arial", false);
        let dir = folder(
            "folder",
            &[
                ("ARIAL.TTF", &arial),
                ("arialbd.ttf", &arial),
                ("times.ttf", b"not a font"),
            ],
        );
        let regular = from_host(&dir, 0).unwrap();
        assert_ne!(regular.id, 0);
        assert_eq!(regular.ttf.face, "Arial");
        assert!(regular.from.ends_with("ARIAL.TTF"), "{}", regular.from);
        for style in [1, 2, 3, 4, 8] {
            assert!(from_host(&dir, style).is_none(), "{style}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `auto` reads the emulated C:'s C:\WINDOWS\FONTS, and in a
    /// deterministic run nothing of the host's: the fonts C: lacks are
    /// Liberation's. The setting outlasts the end of a program. A machine
    /// without settings, as the tests make them, draws with Liberation
    /// whatever C: has, and so does `liberation`.
    #[test]
    fn auto_reads_c_and_a_deterministic_run_not_the_host() {
        let arial = sample("Arial", false);
        let c = folder("drive", &[("WINDOWS/FONTS/ARIAL.TTF", &arial)]);
        let _mode = crate::deterministic::Deterministic::new(crate::deterministic::default_start());
        let mut cpu = Cpu::new(c.clone());
        assert_eq!(of(&mut cpu, 0).unwrap().id, 0);
        cpu.bus.win32.outlines.set(&Win32Fonts::Auto);
        let drawn = of(&mut cpu, 0).unwrap();
        assert_ne!(drawn.id, 0);
        assert!(drawn.from.ends_with("ARIAL.TTF"), "{}", drawn.from);
        for style in 1..12 {
            assert_eq!(of(&mut cpu, style).unwrap().id, 0, "{style}");
        }
        // The setting and the fonts read outlast the programs.
        cpu.bus.win32.reset();
        assert_eq!(of(&mut cpu, 0).unwrap().id, drawn.id);
        cpu.bus.win32.outlines.set(&Win32Fonts::Liberation);
        assert_eq!(of(&mut cpu, 0).unwrap().id, 0);
        let _ = std::fs::remove_dir_all(&c);
    }

    /// The bundled file cut short at many places, or with a run of bytes
    /// changed, is refused or drawn, without a panic.
    #[test]
    fn a_broken_font_file_is_refused_or_drawn_without_a_panic() {
        let whole = truetype::outlines(0);
        let s = truetype::at(0, 13, 13);
        let m = Metrics {
            key: SizeKey {
                ppem: 13,
                x_ppem: 13,
                bold: false,
            },
            height: s.height,
            ascent: s.ascent,
            internal: s.internal,
            external: s.external,
            average: s.average,
            max: s.max,
        };
        let mut files: Vec<Vec<u8>> = (0..64).map(|i| whole[..whole.len() * i / 64].to_vec()).collect();
        for i in 0..64 {
            let mut b = whole.to_vec();
            let at = b.len() * i / 64;
            let end = (at + 256).min(b.len());
            for x in &mut b[at..end] {
                *x ^= 0xA5;
            }
            files.push(b);
        }
        for bytes in files {
            if let Some(t) = Ttf::parse(bytes, 0) {
                t.draw(&m, |b| Some(b as u32));
            }
        }
    }
}
