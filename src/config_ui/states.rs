//! The settings window's States page: the save state slots of the game
//! playing (or of the machine without one), each with when it was saved
//! and in which program, and a picture of the selected one. Enter loads a
//! slot, Ins saves to it, Del empties it.

use super::draw::{self, Grid, Layout};
use super::{ConfigUi, Hit, Host, Target, UiKey, fit};
use crate::savestate::slots::{Header, SLOTS};
use crate::video::Frame;

/// A slot as the page shows it.
#[derive(Clone, Debug, Default)]
pub struct SlotView {
    pub slot: u8,
    /// What is in it: None when it is empty.
    pub header: Option<Header>,
    /// Its picture of the screen.
    pub picture: Option<Frame>,
}

impl ConfigUi {
    /// The slots as the frontend has them now, with the slot the hotkeys
    /// use selected.
    pub(super) fn refresh_states(&mut self, host: &dyn Host) {
        let saved = host.states();
        self.states = (1..=SLOTS)
            .map(|slot| saved.iter().find(|s| s.slot == slot).cloned().unwrap_or(SlotView { slot, ..Default::default() }))
            .collect();
        self.states_available = host.states_available();
    }

    /// Open on the States page (Ctrl+F9).
    pub fn show_states(&mut self, host: &dyn Host) {
        self.show_page(super::Page::States);
        self.refresh_states(host);
        self.row = (host.current_slot().max(1) - 1) as usize;
    }

    pub(super) fn states_key(&mut self, key: UiKey, host: &mut dyn Host) {
        let Some(view) = self.states.get(self.row).cloned() else { return };
        if let Some(i) = self.confirm_delete.take() {
            if key == UiKey::Enter && i == self.row {
                match host.delete_state(view.slot) {
                    Ok(()) => {
                        self.refresh_states(host);
                        self.info(format!("Slot {} is empty now", view.slot));
                    }
                    Err(e) => self.error(e),
                }
            } else {
                self.status = None;
            }
            return;
        }
        match key {
            UiKey::Enter if view.header.is_none() => self.error(format!("Slot {} is empty: Ins saves to it", view.slot)),
            UiKey::Enter => match host.load_state(view.slot) {
                Ok(message) => {
                    self.close();
                    self.notice = Some(message);
                }
                Err(e) => self.error(format!("Slot {} can't be loaded: {}", view.slot, e)),
            },
            UiKey::Insert => match host.save_state(view.slot) {
                Ok(message) => {
                    self.refresh_states(host);
                    self.info(message);
                }
                Err(e) => self.error(format!("Slot {} can't be saved: {}", view.slot, e)),
            },
            UiKey::Delete if view.header.is_some() => {
                self.confirm_delete = Some(self.row);
                self.error(format!("Empty slot {}? Enter deletes its state, Esc keeps it", view.slot));
            }
            _ => {}
        }
    }

    pub(super) fn draw_states(&mut self, g: &mut Grid, content: std::ops::Range<usize>) {
        let cols = g.cols;
        if !self.states_available {
            g.text_to(2, content.start, "Save states need a folder to keep them in, which there isn't.", draw::DIM, cols - 2);
            return;
        }
        Self::keep_visible(&mut self.scroll, self.row, content.len());
        // The picture of the selected slot at the right, where there is
        // room for it.
        let picture_cols = 160 / 8 + 2;
        let list_end = if cols > picture_cols + 40 { cols - picture_cols - 1 } else { cols - 2 };
        for (i, row) in (self.scroll..self.states.len()).zip(content.clone()) {
            let view = &self.states[i];
            if i == self.row {
                g.background(1, row, list_end - 1, draw::SELECT);
            }
            self.hits.push(Hit { row, col: 1, width: list_end - 1, target: Target::Row(i) });
            g.text(2, row, &view.slot.to_string(), draw::KEY);
            match &view.header {
                Some(header) => {
                    let end = g.text_to(5, row, &header.saved, draw::BRIGHT, list_end);
                    g.text_to(end + 2, row, &fit(&header.program, list_end.saturating_sub(end + 3)), draw::DIM, list_end);
                }
                None => {
                    g.text_to(5, row, "empty", draw::DIM, list_end);
                }
            }
        }
        if list_end < cols - 2
            && let Some(picture) = self.states.get(self.row).and_then(|v| v.picture.clone())
        {
            self.pictures.push(((list_end + 1, content.start), picture));
        }
    }

    /// Draw the pictures the page asked for, in pixels over the panel.
    pub(super) fn draw_pictures(&mut self, frame: &mut Frame, layout: &Layout) {
        for ((col, row), picture) in std::mem::take(&mut self.pictures) {
            let (x0, y0) = (layout.x + col * 8, layout.y + row * layout.cell_h);
            let (width, height) = (frame.width as usize, frame.height as usize);
            for y in 0..picture.height as usize {
                for x in 0..picture.width as usize {
                    let (fx, fy) = (x0 + x, y0 + y);
                    if fx < width && fy < height {
                        let from = (y * picture.width as usize + x) * 3;
                        let to = (fy * width + fx) * 3;
                        frame.rgb[to..to + 3].copy_from_slice(&picture.rgb[from..from + 3]);
                    }
                }
            }
        }
    }
}
