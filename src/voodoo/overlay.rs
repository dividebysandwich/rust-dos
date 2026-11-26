//! Glide's DOS overlay, GLIDE2X.OVL: the Glide 2 library as a DOS/4G DLL,
//! which games load from their folder or the PATH instead of linking
//! Glide in, such as Tomb Raider's Voodoo Rush version. Each board has
//! its own: the Voodoo Graphics driver's looks for a Voodoo Graphics on
//! the PCI bus and finds no Voodoo2, and the Voodoo2 driver's is the one a
//! Voodoo2's owner had. Either drives the emulated card of its board
//! whatever board the game was made for. They are 3dfx's and don't come
//! with rust-dos: the settings window fetches the add-on card's, once the
//! user agrees, from 3dfx's last driver for that board as the Internet
//! Archive keeps it, and it is then on Z:, which is on the PATH.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use super::Model;

pub const FILE_NAME: &str = "GLIDE2X.OVL";

/// A board's overlay and the driver it comes out of.
#[derive(Debug, PartialEq, Eq)]
pub struct Overlay {
    /// The board it drives.
    pub model: Model,
    /// What the user is told it comes from.
    pub driver: &'static str,
    /// Where the download comes from, for the question.
    pub source: &'static str,
    /// The driver: a zip, or a self-extracting zip, with the overlay in it
    /// or in an archive in it.
    #[cfg_attr(
        not(all(feature = "sdl", not(target_arch = "wasm32"))),
        allow(dead_code)
    )]
    url: &'static str,
    /// Its name in rust-dos's configuration directory.
    kept_as: &'static str,
    sha256: &'static str,
}

/// The overlay of 3dfx's Voodoo Graphics driver 3.01.00 (Glide 2.46). The
/// download is a zip of the driver's self-extracting archive.
pub const VOODOO1: Overlay = Overlay {
    model: Model::Voodoo1,
    driver: "Voodoo Graphics driver 3.01.00",
    source: "archive.org/details/voodoo1-30100_20251011_1715",
    url: "https://archive.org/download/voodoo1-30100_20251011_1715/voodoo1-30100.zip",
    kept_as: FILE_NAME,
    sha256: "9dadbeb79ddaa23d6125e0a84a4f2aafaec74bf1c2c18a07f5847931d2d42b43",
};

/// The overlay of 3dfx's Voodoo2 driver 3.02.02 (Glide 2.56), which its
/// readme calls "DOS Glide for Voodoo2". The download is the driver's
/// self-extracting zip as 3dfx shipped it.
pub const VOODOO2: Overlay = Overlay {
    model: Model::Voodoo2,
    driver: "Voodoo2 driver 3.02.02",
    source: "archive.org/details/v2-w9x-3.02.02",
    url: "https://archive.org/download/v2-w9x-3.02.02/v2-w9x-3.02.02.exe",
    kept_as: "GLIDE2X-VOODOO2.OVL",
    sha256: "1098ddd7adb25a4f95402544a8db704298d0ec10d0981ffa07bf0e1870e383bf",
};

/// The biggest archive taken, and looked into.
const LIMIT: u64 = 16 << 20;

impl Overlay {
    /// The overlay for a card of `model`. A Banshee or Voodoo3 is the
    /// display adapter, whose driver has no DOS overlay.
    pub fn of(model: Model) -> Option<&'static Overlay> {
        match model {
            Model::Voodoo1 => Some(&VOODOO1),
            Model::Voodoo2 => Some(&VOODOO2),
            Model::Banshee => None,
        }
    }

    /// The card's name, as the user is told it.
    pub fn board(&self) -> &'static str {
        match self.model {
            Model::Voodoo1 => "Voodoo Graphics",
            Model::Voodoo2 => "Voodoo2",
            Model::Banshee => "Voodoo Banshee",
        }
    }

    /// Where the download is kept, and found.
    pub fn path(&self) -> Option<PathBuf> {
        crate::config::user_dir().map(|dir| dir.join(self.kept_as))
    }

    /// The overlay, if it is there.
    pub fn find(&self) -> Option<PathBuf> {
        self.path().filter(|p| p.is_file())
    }

    /// The overlay in `zip`, looked for in the archives in it too, by its
    /// hash.
    pub fn extract(&self, zip: &[u8]) -> Result<Vec<u8>, String> {
        self.find_in(zip, 2)
            .ok_or_else(|| format!("the download has no {} of the {}", FILE_NAME, self.driver))
    }

    fn find_in(&self, zip: &[u8], depth: u32) -> Option<Vec<u8>> {
        let mut cursor = Cursor::new(zip);
        let entries = crate::archive::zip::central_directory(&mut cursor).ok()?;
        for entry in entries.iter().filter(|e| !e.is_dir() && e.size <= LIMIT) {
            let name = entry.name().to_ascii_lowercase();
            if name.ends_with(".ovl") {
                let bytes = crate::archive::zip::read(&mut cursor, entry).ok()?;
                if crate::sc55::rom::sha256_hex(&bytes) == self.sha256 {
                    return Some(bytes);
                }
            } else if depth > 0 && (name.ends_with(".zip") || name.ends_with(".exe")) {
                let bytes = crate::archive::zip::read(&mut cursor, entry).ok()?;
                if let Some(found) = self.find_in(&bytes, depth - 1) {
                    return Some(found);
                }
            }
        }
        None
    }

    /// Download the driver and keep its overlay at `dest`, checked against
    /// its hash and written to a temporary file first, so a failed
    /// download leaves nothing behind. It blocks: run it on a thread of
    /// its own.
    #[cfg(all(feature = "sdl", not(target_arch = "wasm32")))]
    pub fn download(&self, dest: &Path) -> Result<(), String> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(120)))
            .user_agent(format!("rust-dos/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        let zip = agent
            .get(self.url)
            .call()
            .and_then(|mut response| response.body_mut().with_config().limit(LIMIT).read_to_vec())
            .map_err(|e| format!("download failed: {}", e))?;
        let overlay = self.extract(&zip)?;
        if let Some(dir) = dest.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
        }
        let temp = dest.with_extension("part");
        std::fs::write(&temp, &overlay).map_err(|e| format!("{}: {}", temp.display(), e))?;
        std::fs::rename(&temp, dest).map_err(|e| format!("{}: {}", dest.display(), e))
    }

    #[cfg(not(all(feature = "sdl", not(target_arch = "wasm32"))))]
    pub fn download(&self, _dest: &Path) -> Result<(), String> {
        Err("downloads are only available in the rust-dos program".to_string())
    }
}

/// The overlay for the machine's 3dfx add-on card, if it has one.
fn of_card(bus: &crate::bus::Bus) -> Option<&'static Overlay> {
    bus.voodoo
        .as_ref()
        .and_then(|v| Overlay::of(v.board.model()))
}

/// Put the overlay of the machine's add-on card on Z:, if it has been
/// downloaded, and take off one of another board, which a card swapped at
/// the prompt leaves there: a Voodoo2 with the Voodoo Graphics overlay is
/// a machine with no card, to the game.
pub fn provide(bus: &mut crate::bus::Bus) {
    let Some(overlay) = of_card(bus) else {
        place(bus, None);
        return;
    };
    let bytes = match overlay.find().map(|path| (std::fs::read(&path), path)) {
        Some((Ok(bytes), _)) => Some((overlay, bytes)),
        Some((Err(e), path)) => {
            bus.log_string(&format!("[3DFX] {}: {}", path.display(), e));
            None
        }
        None => None,
    };
    place(bus, bytes);
}

/// Make Z:'s GLIDE2X.OVL `wanted`'s bytes, or take it off.
fn place(bus: &mut crate::bus::Bus, wanted: Option<(&Overlay, Vec<u8>)>) {
    let z = format!("Z:\\{}", FILE_NAME);
    let on_z = bus.disk.virtual_file_bytes(&z);
    if on_z == wanted.as_ref().map(|(_, bytes)| &bytes[..]) {
        return;
    }
    match wanted {
        Some((overlay, bytes)) => {
            bus.disk.add_virtual_file(FILE_NAME, bytes);
            bus.log_string(&format!(
                "[3DFX] {} for the {} is on Z:",
                FILE_NAME,
                overlay.board()
            ));
        }
        None => {
            bus.disk.remove_virtual_file(FILE_NAME);
            bus.log_string(&format!(
                "[3DFX] {} is off Z:, as there is none for the card in the machine",
                FILE_NAME
            ));
        }
    }
}

/// A program looked for `path` and didn't find it: if it is the overlay
/// and Z: hasn't it either, tell the user where to get it, or to put the
/// 3dfx card in, once for the program.
pub fn looked_for(cpu: &mut crate::cpu::Cpu, path: &str) {
    let name = path.rsplit(['\\', '/', ':']).next().unwrap_or(path);
    if !name.eq_ignore_ascii_case(FILE_NAME) || cpu.bus.disk.exists(&format!("Z:\\{}", FILE_NAME)) {
        return;
    }
    let psp = cpu.current_psp;
    if cpu.bus.glide_hint.replace(psp) == Some(psp) {
        return;
    }
    // The overlays drive a Voodoo Graphics or a Voodoo2, which a Voodoo3
    // or Banshee as the display adapter leaves no room for: the settings
    // window offers neither the download nor the add-on card's switch
    // then.
    let card = cpu
        .bus
        .voodoo
        .as_ref()
        .map(|v| Overlay::of(v.board.model()));
    // The settings window's key, as `[hotkeys]` has it.
    let key = match &cpu.startup.settings_key {
        Some(key) => format!(" ({})", key),
        None => String::new(),
    };
    let (title, detail) = match card {
        None => (
            "This game wants a 3dfx card".to_string(),
            format!("Turn on 3dfx Voodoo Graphics on the settings' Emulator page{}", key),
        ),
        Some(None) => (
            "This game wants a 3dfx Voodoo Graphics".to_string(),
            format!(
                "Pick another Video card, then turn on 3dfx Voodoo Graphics, on the settings' \
                 Emulator page{}",
                key
            ),
        ),
        Some(Some(overlay)) => (
            format!(
                "This game needs 3dfx's GLIDE2X.OVL for the {}",
                overlay.board()
            ),
            format!("Download it on the settings' Emulator page{}", key),
        ),
    };
    cpu.bus.log_string(&format!("[3DFX] {}: {}", title, detail));
    cpu.bus.notices.push((title, detail));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A driver's archive, when a copy is at hand (RUST_DOS_GLIDE_ZIP for
    /// the Voodoo Graphics driver's zip, RUST_DOS_GLIDE_V2_EXE for the
    /// Voodoo2 driver's self-extracting zip).
    #[test]
    fn the_overlays_come_out_of_the_drivers_archives() {
        for (var, overlay, len) in [
            ("RUST_DOS_GLIDE_ZIP", &VOODOO1, 195_815),
            ("RUST_DOS_GLIDE_V2_EXE", &VOODOO2, 228_196),
        ] {
            let Some(zip) = std::env::var_os(var).and_then(|p| std::fs::read(p).ok()) else {
                continue;
            };
            assert_eq!(overlay.extract(&zip).unwrap().len(), len, "{var}");
        }
    }

    #[test]
    fn other_archives_have_none() {
        for overlay in [&VOODOO1, &VOODOO2] {
            assert!(
                overlay
                    .extract(b"PK\x05\x06\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0")
                    .is_err()
            );
            assert!(overlay.extract(b"not a zip").is_err());
        }
    }

    #[test]
    fn each_add_on_board_has_its_own() {
        assert_eq!(Overlay::of(Model::Voodoo1), Some(&VOODOO1));
        assert_eq!(Overlay::of(Model::Voodoo2), Some(&VOODOO2));
        assert_eq!(Overlay::of(Model::Banshee), None);
        // The Voodoo Graphics overlay stays where earlier versions kept
        // it, so a download made by one is found.
        assert_eq!(VOODOO1.kept_as, FILE_NAME);
        assert_ne!(VOODOO2.kept_as, FILE_NAME);
    }

    /// Z: holds the overlay of the card in the machine, and none when
    /// there is none for it: the Voodoo Graphics one doesn't find a
    /// Voodoo2, and a game that loaded it would act as with no card.
    #[test]
    fn z_has_the_cards_overlay_or_none() {
        let mut bus = crate::bus::Bus::new(PathBuf::from("."));
        let z = format!("Z:\\{}", FILE_NAME);
        place(&mut bus, Some((&VOODOO1, b"voodoo1".to_vec())));
        assert_eq!(bus.disk.virtual_file_bytes(&z), Some(&b"voodoo1"[..]));
        place(&mut bus, Some((&VOODOO2, b"voodoo2".to_vec())));
        assert_eq!(bus.disk.virtual_file_bytes(&z), Some(&b"voodoo2"[..]));
        place(&mut bus, None);
        assert_eq!(bus.disk.virtual_file_bytes(&z), None);
        assert!(!bus.disk.exists(&z));
        // Nothing to take off.
        place(&mut bus, None);
        assert!(!bus.disk.exists(&z));
    }

    #[test]
    fn the_overlay_follows_the_card() {
        let mut bus = crate::bus::Bus::new(PathBuf::from("."));
        assert_eq!(of_card(&bus), None);
        bus.configure_voodoo(Some(super::super::Board::Max));
        assert_eq!(of_card(&bus), Some(&VOODOO1));
        bus.configure_voodoo(Some(super::super::Board::Voodoo2Max));
        assert_eq!(of_card(&bus), Some(&VOODOO2));
        bus.configure_voodoo(Some(super::super::Board::Voodoo3));
        assert_eq!(of_card(&bus), None);
    }

    /// Swapping the card takes the old card's overlay off Z:, whoever
    /// swaps it: `Hardware::apply` changes the card after the settings
    /// window has placed the overlay of the card that was in.
    #[test]
    fn a_card_swap_takes_the_old_overlay_off() {
        let mut bus = crate::bus::Bus::new(PathBuf::from("."));
        let z = format!("Z:\\{}", FILE_NAME);
        bus.configure_voodoo(Some(super::super::Board::Max));
        place(&mut bus, Some((&VOODOO1, b"voodoo1".to_vec())));
        bus.configure_voodoo(Some(super::super::Board::Voodoo3));
        assert!(!bus.disk.exists(&z));
    }
}
