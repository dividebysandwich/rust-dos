//! The Emulator page's download of Glide's DOS overlay (GLIDE2X.OVL),
//! which games made for the Voodoo Rush and later boards load: asked
//! first, as it is 3dfx's; then the download on a thread of its own.

use std::path::PathBuf;

use super::draw::{self, Grid};
use super::rooms::wrap;
use super::{ConfigUi, Host, Item, UiKey, contract_home};

impl ConfigUi {
    /// Enter on *Download Glide's DOS overlay...*: ask first.
    pub(super) fn ask_glide_download(&mut self) {
        if self.glide_download.is_some() {
            self.info("Glide's DOS overlay is downloading...");
            return;
        }
        self.confirm_glide = true;
        self.status = None;
    }

    /// The answer: Enter downloads, anything else doesn't.
    pub(super) fn glide_confirm_key(&mut self, key: UiKey) {
        self.confirm_glide = false;
        if key != UiKey::Enter {
            self.info("Nothing was downloaded");
            return;
        }
        let Some(dest) = crate::voodoo::overlay::path() else {
            self.error("There is no directory for rust-dos's files to download the overlay into");
            return;
        };
        let (done, result) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new().name("glide-overlay".to_string()).spawn(move || {
            let _ = done.send(crate::voodoo::overlay::download(&dest).map(|()| dest));
        });
        match spawned {
            Ok(_) => {
                self.glide_download = Some(result);
                self.info("Downloading 3dfx's Voodoo Graphics driver from the Internet Archive...");
            }
            Err(e) => self.error(format!("The download didn't start: {}", e)),
        }
    }

    /// The download's result, once it has one: Z: gets the overlay.
    pub(super) fn poll_glide_download(&mut self, host: &mut dyn Host) {
        let Some(result) = &self.glide_download else { return };
        let outcome: Result<PathBuf, String> = match result.try_recv() {
            Ok(outcome) => outcome,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("the download stopped".to_string()),
        };
        self.glide_download = None;
        match outcome {
            Ok(path) => {
                self.changed(Item::VoodooOverlay, host);
                let shown = contract_home(&path, self.home.as_deref());
                self.info(format!("GLIDE2X.OVL is in {} and on Z:", shown));
                self.row = self.row.min(self.row_count().saturating_sub(1));
            }
            Err(e) => self.error(format!("Glide's DOS overlay: {}", e)),
        }
    }

    /// The question, over the page.
    pub(super) fn draw_glide_question(&self, g: &mut Grid, content: std::ops::Range<usize>) {
        let width = g.cols.saturating_sub(6).min(76);
        let paragraphs = [
            ("Download Glide's DOS overlay?".to_string(), draw::BRIGHT),
            (
                "Games made for the Voodoo Rush and later 3dfx boards, such as Tomb Raider's Rush version, load \
                 Glide from GLIDE2X.OVL, which came with 3dfx's drivers. It is 3dfx's and doesn't come with \
                 rust-dos."
                    .to_string(),
                draw::TEXT,
            ),
            (
                format!(
                    "rust-dos can fetch 3dfx's last Voodoo Graphics driver (3.01.00) as the Internet Archive keeps \
                     it ({}), keep only its GLIDE2X.OVL, checked against the known file, in its configuration \
                     directory, and put it on Z:, where games find it.",
                    crate::voodoo::overlay::SOURCE
                ),
                draw::TEXT,
            ),
            (
                "Download it only if you have the right to use it, for instance because you own a 3dfx board."
                    .to_string(),
                draw::TEXT,
            ),
            ("Enter downloads it, Esc doesn't.".to_string(), draw::GOOD),
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
