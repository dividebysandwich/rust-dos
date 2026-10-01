//! The Sound page's Sound Canvas ROM download: what it fetches and from
//! where, asked first, as the firmware is Roland's; then the download on a
//! thread of its own.

use std::path::PathBuf;

use super::draw::{self, Grid};
use super::rooms::wrap;
use super::{ConfigUi, Host, Item, UiKey, contract_home};

/// Where the download comes from, for the question.
const SOURCE: &str = "archive.org/details/roland-sc-55-series-roms";

impl ConfigUi {
    /// Enter on *Download the Sound Canvas ROMs...*: ask first.
    pub(super) fn ask_sc55_download(&mut self) {
        if self.sc55_download.is_some() {
            self.info("The Sound Canvas ROMs are downloading...");
            return;
        }
        match crate::sc55::download::romset_for(&self.settings.sound.sc55model) {
            Some(romset) => {
                self.confirm_sc55 = Some(romset);
                self.status = None;
            }
            None => self.error(format!(
                "The Internet Archive has no ROMs for the {} model: pick auto, mk1, mk2, sc155 or cm300",
                self.settings.sound.sc55model
            )),
        }
    }

    /// The answer: Enter downloads, anything else doesn't.
    pub(super) fn sc55_confirm_key(&mut self, key: UiKey) {
        let Some(romset) = self.confirm_sc55.take() else { return };
        if key != UiKey::Enter {
            self.info("Nothing was downloaded");
            return;
        }
        let Some(dir) = crate::sc55::rom::download_dir() else {
            self.error("There is no directory for rust-dos's files to download the ROMs into");
            return;
        };
        let (done, result) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new().name("sc55-roms".to_string()).spawn(move || {
            let _ = done.send(crate::sc55::download::download(romset, &dir));
        });
        match spawned {
            Ok(_) => {
                self.sc55_download = Some(result);
                self.info(format!("Downloading the {} ROMs from the Internet Archive...", romset.display_name()));
            }
            Err(e) => self.error(format!("The download didn't start: {}", e)),
        }
    }

    /// The download's result, once it has one: the module gets the ROMs.
    pub(super) fn poll_sc55_download(&mut self, host: &mut dyn Host) {
        let Some(result) = &self.sc55_download else { return };
        let outcome: Result<PathBuf, String> = match result.try_recv() {
            Ok(outcome) => outcome,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("the download stopped".to_string()),
        };
        self.sc55_download = None;
        match outcome {
            Ok(dir) => {
                crate::sc55::rom::forget();
                let shown = contract_home(&dir, self.home.as_deref());
                self.settings.sound.sc55roms = Some(dir);
                self.changed(Item::Sc55Roms, host);
                self.info(format!("The Sound Canvas ROMs are in {}", shown));
                self.row = self.row.min(self.row_count().saturating_sub(1));
            }
            Err(e) => self.error(format!("The Sound Canvas ROMs: {}", e)),
        }
    }

    /// The question, over the page.
    pub(super) fn draw_sc55_question(&self, g: &mut Grid, content: std::ops::Range<usize>) {
        let Some(romset) = self.confirm_sc55 else { return };
        let width = g.cols.saturating_sub(6).min(76);
        let paragraphs = [
            ("Download the Sound Canvas ROMs?".to_string(), draw::BRIGHT),
            (
                "The Sound Canvas's firmware and sounds are Roland's. They don't come with rust-dos, and Roland \
                 doesn't offer them for download."
                    .to_string(),
                draw::TEXT,
            ),
            (
                format!(
                    "rust-dos can fetch a copy of the {}'s ROMs that the Internet Archive keeps ({}), check each \
                     file against the known dumps, and keep them in its configuration directory.",
                    romset.display_name(),
                    SOURCE
                ),
                draw::TEXT,
            ),
            (
                "Download them only if you have the right to use them, for instance because you own the module."
                    .to_string(),
                draw::TEXT,
            ),
            ("Enter downloads them, Esc doesn't.".to_string(), draw::GOOD),
        ];
        let mut row = content.start + 1;
        for (text, color) in paragraphs {
            for line in wrap(&text, width, content.end.saturating_sub(row)) {
                if row >= content.end {
                    return;
                }
                g.text_to(3, row, &line, color, g.cols - 2);
                row += 1;
            }
            row += 1;
        }
    }
}
